//! The transfer worker (the MP3 Player contract's clauses 5, 25, 44–63;
//! the transfer plan's part B4): the cards in this computer's readers, as
//! the MP3 Player tab draws them, and the one sync that runs at a time.
//!
//! It is the GUI's, not the tab's (clause 55): started once, at launch, it
//! outlives the tab and its page, so leaving the tab, another screen or the
//! mini player never stops a sync. The page borrows its channels each visit
//! ([`Link`], and the GUI's fold of its reports, [`Fold`]); the command
//! line runs the same jobs without it.
//!
//! **What it does** (the desk worker's pattern: whole cards out, commands
//! in):
//! - **Watches the readers every 2 s** ([`Volumes::find`], part A7's rule).
//!   A card put in is read with nothing written — its root, `state.json`,
//!   `device.txt`, `/music` in numbers — and its server asked once what a
//!   sync would do (a dry run: one manifest request, a 304 when nothing
//!   changed). A volume the player can't use is listed with why (clause
//!   63); the computer's own disks and fixed disks never are.
//! - **Runs one sync at a time** (clause 56): another card's Sync waits its
//!   turn. The run is the sync module's; this worker gives it the card, the
//!   card's own server (clause 3a: the saved sign-in whose URL key the card
//!   names, never necessarily the session's), the answers to its gates, and
//!   the listener's stop ([`sync::Ask::stop`]), and tells its progress.
//! - **Set-up, the settings, the plays, eject and hide**, each a job of its
//!   own on the card, one job a card at a time.
//!
//! **What it never does:** write a card the drive-safety rule refused, hold
//! a serial port (the boards are the desk's), or decide for the listener:
//! a gate's answer comes in with the command that asks for the sync.

use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::budget::{Budget, PeerKey};
use super::cards::Cards;
use super::contract::device_txt::DeviceTxt;
use super::contract::server::server_url_key;
use super::engine::{Answers, Env, Event as RunEvent, Guard, Removal, Stop};
use super::history::{self, Tally};
use super::plan::Fit;
use super::setup;
use super::state::{self, Convert, State};
use super::sync::{self, Federation, PeerStatus};
use super::volume::{self, CardWrite, Found, Mounted, Music, Refusal, Space};
use crate::api::Client;
use crate::api::urls::{TransferBitrate, TransferCodec};

/// How often the readers are looked at (clause 30).
pub(crate) const EVERY: Duration = Duration::from_secs(2);
/// A run's progress is told at most this often.
const TELL_EVERY: Duration = Duration::from_millis(200);
/// A server that didn't answer is asked again after 1 min, doubling to 30
/// (clause 52).
const RETRY_FIRST: u64 = 60;
const RETRY_MOST: u64 = 30 * 60;
/// The removals a gate names before "and N more" (clause 58).
pub(crate) const GATE_PATHS: usize = 5;

// ── What the worker is given ────────────────────────────────────────────────

/// A server saved on this computer, as the GUI reaches it now (clause 3a):
/// what a card's sync and its look are made with. No card ever holds any
/// of it but the nickname and the URL key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerLink {
    /// The config's key for it: its URL, or a tunnel's id.
    pub(crate) id: String,
    /// As this terminal shows it ("Home").
    pub(crate) name: String,
    /// The base its requests go to, or why it can't be reached now.
    pub(crate) base: Result<String, String>,
    pub(crate) token: Option<String>,
    pub(crate) self_signed: bool,
    /// Over a tunnel's bridge: its loopback token.
    pub(crate) local_token: Option<String>,
    /// Reached through Quick Connect's relay, not directly (clause 58).
    pub(crate) relay: bool,
    /// The GUI's session is on it: a new card takes it (clause 3a).
    pub(crate) session: bool,
}

impl Default for ServerLink {
    fn default() -> ServerLink {
        ServerLink {
            id: String::new(),
            name: String::new(),
            base: Err(String::new()),
            token: None,
            self_signed: false,
            local_token: None,
            relay: false,
            session: false,
        }
    }
}

impl ServerLink {
    /// A client for it, as the GUI's pages build one.
    pub(crate) fn client(&self) -> Result<Client, String> {
        let base = self.base.clone()?;
        let client = Client::new_with(&base, self.self_signed).map_err(|e| e.to_string())?;
        Ok(client.with_token(self.token.clone()).with_local_token(self.local_token.clone()))
    }

    /// The URL key a sync of it stores on a card: of the address it reads
    /// the server at, as [`sync::sync`] computes it.
    pub(crate) fn url_key(&self) -> Option<u64> {
        self.client().ok().map(|c| server_url_key(&c.server()))
    }

    /// Every key a card may name it by: the address it is read at, and its
    /// saved URL.
    fn keys(&self) -> Vec<u64> {
        let mut keys: Vec<u64> = self.url_key().into_iter().collect();
        if self.id.contains("://") {
            keys.push(server_url_key(&self.id));
        }
        keys
    }

    /// The local-disk source's condition: a loopback address, directly.
    fn reached(&self) -> super::source::local::Reached {
        use super::source::local::Reached;
        if self.local_token.is_some() {
            return Reached::Elsewhere;
        }
        let Ok(base) = &self.base else { return Reached::Elsewhere };
        let Ok(url) = url::Url::parse(base) else { return Reached::Elsewhere };
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
}

/// The volumes the worker looks at and opens: the OS's ([`System`]), or a
/// test's.
pub(crate) trait Volumes: Send + Sync {
    fn find(&self) -> io::Result<Vec<Found>>;
    /// The card of an accepted volume; None for a refused one.
    fn open(&self, found: &Found) -> Option<Box<dyn CardWrite>>;
    fn eject(&self, found: &Found) -> Result<(), String>;
}

/// What the jobs keep on this computer: `cards.json`, the plays' outbox,
/// the peers' budget, each session's clock and ids.
pub(crate) trait Home: Send + Sync {
    fn cards(&self) -> Cards;
    fn save_cards(&self, cards: &Cards) -> Result<(), String>;
    fn plays(&self) -> Rc<dyn history::Store>;
    fn federation(&self, cards: Vec<PathBuf>) -> Result<Federation, String>;
    fn envs(&self) -> Box<dyn FnMut() -> Env>;
    /// Unix seconds now.
    fn now(&self) -> u64;
}

/// This computer's volumes.
pub(crate) struct System;

impl Volumes for System {
    fn find(&self) -> io::Result<Vec<Found>> {
        volume::find()
    }

    fn open(&self, found: &Found) -> Option<Box<dyn CardWrite>> {
        Mounted::of(found).map(|m| Box::new(m) as Box<dyn CardWrite>)
    }

    fn eject(&self, found: &Found) -> Result<(), String> {
        volume::eject::eject(found)
    }
}

/// This computer's files, beside the player's config.
pub(crate) struct ConfigHome;

impl Home for ConfigHome {
    fn cards(&self) -> Cards {
        Cards::load()
    }

    fn save_cards(&self, cards: &Cards) -> Result<(), String> {
        cards.save()
    }

    fn plays(&self) -> Rc<dyn history::Store> {
        Rc::new(history::ConfigStore)
    }

    fn federation(&self, cards: Vec<PathBuf>) -> Result<Federation, String> {
        Federation::system(cards)
    }

    fn envs(&self) -> Box<dyn FnMut() -> Env> {
        Box::new(Env::system)
    }

    fn now(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

/// What the worker runs on.
pub(crate) struct Setup {
    pub(crate) volumes: Arc<dyn Volumes>,
    pub(crate) home: Arc<dyn Home>,
    /// How often the readers are looked at.
    pub(crate) every: Duration,
}

impl Setup {
    /// This computer's.
    pub(crate) fn system() -> Setup {
        Setup { volumes: Arc::new(System), home: Arc::new(ConfigHome), every: EVERY }
    }
}

// ── What the listener asks ──────────────────────────────────────────────────

/// A card's settings as the Settings sheet or the command line chose them
/// (clauses 47–50, 65): what Set up and Apply write to `state.json`, and
/// what a fit is worked out for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Want {
    pub(crate) name: String,
    /// The saved server it is filled from, by the config's key; None: the
    /// card's own, or no server at all yet (clause 3a).
    pub(crate) server: Option<String>,
    /// The server's libraries; None: as the card has them.
    pub(crate) libraries: Option<Vec<String>>,
    /// Each peer's libraries, by its id on the server.
    pub(crate) peers: Vec<(u32, Vec<String>)>,
    pub(crate) convert: Convert,
    pub(crate) codec: TransferCodec,
    pub(crate) bitrate: TransferBitrate,
    pub(crate) removal: Removal,
    pub(crate) covers: bool,
    pub(crate) auto_dj: bool,
    pub(crate) for_someone_else: bool,
    /// MiB a day per peer, kept on this computer; None: as it is.
    pub(crate) budget_mb: Option<u64>,
}

impl Want {
    /// The settings a card has now, as the sheet opens on them.
    pub(crate) fn of(facts: &Facts, server: Option<&ServerLink>) -> Want {
        let settings = facts.state.as_ref().map(|s| s.settings.clone()).unwrap_or_default();
        let sources = &settings.sources;
        let source = facts.server.and_then(|key| sources.iter().find(|s| s.url_key == key));
        let source = source.or_else(|| settings.sources.first());
        let libraries = source.map(|s| s.libraries.clone()).filter(|l| !l.is_empty());
        let c = settings.conversion;
        Want {
            name: facts.state.as_ref().map(|s| s.label.clone()).unwrap_or_default(),
            server: server.map(|s| s.id.clone()),
            libraries,
            peers: settings.peers.iter().map(|p| (p.peer_id, p.libraries.clone())).collect(),
            convert: c.convert,
            codec: c.codec,
            bitrate: c.bitrate,
            removal: settings.removal,
            covers: settings.covers,
            auto_dj: settings.auto_dj,
            for_someone_else: settings.for_someone_else,
            budget_mb: None,
        }
    }

    /// The sync's settings changes for this choice.
    fn change(&self, nickname: Option<&str>) -> sync::Change {
        sync::Change {
            layout: None,
            convert: Some(self.convert),
            codec: Some(self.codec),
            bitrate: Some(self.bitrate),
            covers: Some(self.covers),
            auto_dj: Some(self.auto_dj),
            for_someone_else: Some(self.for_someone_else),
            removal: Some(self.removal),
            nickname: nickname.map(str::to_string),
        }
    }
}

/// The answers a sync is asked with: the gates' (clause 58).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Allow {
    pub(crate) answers: Answers,
    /// The card's server becomes this one's (the another-server gate).
    pub(crate) take_over: bool,
    /// Peers whose bytes may come through their parent's proxy.
    pub(crate) proxy_copies: Vec<u32>,
    /// Peers left out of this sync (the proxy gate's no).
    pub(crate) leave_out: Vec<u32>,
}

/// What the page asks of the worker.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// The servers saved on this computer, as the GUI reaches them now.
    Servers(Vec<ServerLink>),
    /// Read the card again and ask its server again (*Try now*).
    Look { key: String },
    /// A sync's plan for the sheet's settings, written nowhere: the fit
    /// line (clause 49). `seq` names the answer.
    Fit { key: String, want: Box<Want>, seq: u64 },
    /// Set the card up with these settings (clause 50).
    SetUp { key: String, want: Box<Want> },
    /// Write the card's settings, and nothing else (clause 48's Apply).
    Apply { key: String, want: Box<Want> },
    /// Sync, past these gates (clause 51); after the running one, if one
    /// runs (clause 56).
    Sync { key: String, allow: Box<Allow> },
    /// A sync waiting its turn is forgotten (*Don't wait*).
    DontWait { key: String },
    /// Stop at the next save (clause 52).
    Stop { key: String },
    /// A pause given up (*Stop for now*).
    StopPause { key: String },
    /// Post the card's plays, without a sync (clause 61).
    Plays { key: String },
    /// Ask the OS to let the card go (clause 54).
    Eject { key: String },
    /// Keep a volume off the row (clause 63), and show them all again.
    Hide { key: String },
    ShowHidden,
    /// A copied card takes an identity of its own.
    NewIdentity { key: String },
    /// A card taken out during its sync is forgotten here (clause 53).
    GiveUp { key: String },
    /// The gate the last sync stopped for is put away unanswered.
    Dismiss { key: String },
    /// Stop the run at its next save and end once nothing runs.
    Quit,
}

// ── What it says ────────────────────────────────────────────────────────────

/// A card's identity, settings and contents as read from it (part A7's
/// inspection), with nothing written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Facts {
    /// Its own `state.json` (its card id the root's), when it has one.
    pub(crate) state: Option<State>,
    /// The root: its card id, generation and commit, and when it was made.
    pub(crate) root: Option<(u64, u32, u64)>,
    pub(crate) committed: Option<u64>,
    /// Files its ledger lists: the tracks the transfer put there.
    pub(crate) tracks: usize,
    /// Its tracks by library, from the ledger's server paths.
    pub(crate) by_library: Vec<(String, usize)>,
    /// Files of a newer format: read, never written (clause 45).
    pub(crate) newer: Vec<String>,
    /// A rename may have been cut: a disk check first.
    pub(crate) check: bool,
    /// A cut run's plan is on it, for the next sync to take up.
    pub(crate) pending: bool,
    pub(crate) music: Music,
    pub(crate) device: DeviceTxt,
    /// Its server: the key the root names it by, else its settings'.
    pub(crate) server: Option<u64>,
    /// The server's nickname as the card keeps it.
    pub(crate) nickname: String,
    /// A copy of another card (2.10): the same id on another volume.
    pub(crate) copied: bool,
    /// Its multi-card set: its place, and how many cards.
    pub(crate) set: Option<(usize, usize)>,
    /// The software that made its last commit (`mstream-terminal 0.13.0`).
    pub(crate) producer: String,
}

impl Default for Facts {
    fn default() -> Facts {
        Facts {
            state: None,
            root: None,
            committed: None,
            tracks: 0,
            by_library: Vec::new(),
            newer: Vec::new(),
            check: false,
            pending: false,
            music: Music::default(),
            device: DeviceTxt::read(None),
            server: None,
            nickname: String::new(),
            copied: false,
            set: None,
            producer: String::new(),
        }
    }
}

impl Facts {
    /// Set up by the transfer: a root, and a state file of its own.
    pub(crate) fn set_up(&self) -> bool {
        self.root.is_some() && self.state.is_some()
    }

    /// The card's name: its state file's label.
    pub(crate) fn name(&self) -> Option<&str> {
        self.state.as_ref().map(|s| s.label.as_str()).filter(|l| !l.is_empty())
    }

    /// The libraries it holds of its server, as its settings select them.
    pub(crate) fn libraries(&self) -> Vec<String> {
        let Some(state) = &self.state else { return Vec::new() };
        let sources = &state.settings.sources;
        let source = self.server.and_then(|k| sources.iter().find(|s| s.url_key == k));
        source.or_else(|| sources.first()).map(|s| s.libraries.clone()).unwrap_or_default()
    }
}

/// How far the card has been read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Read {
    /// Being read (`…` on its tab).
    Reading,
    Read(Box<Facts>),
    /// It didn't read: the OS's words.
    Failed(String),
    /// A volume the drive-safety rule refused, listed with why: never read.
    Refused,
}

/// Where a card's server stands for this computer (clause 3a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Server {
    /// A saved server answers its key.
    Saved(ServerLink),
    /// No saved server has its key: the card says so (its nickname kept).
    Unknown(String),
    /// A new card takes the session's server, or the first saved; none:
    /// no server saved on this computer at all.
    New(Option<ServerLink>),
}

impl Server {
    pub(crate) fn link(&self) -> Option<&ServerLink> {
        match self {
            Server::Saved(link) | Server::New(Some(link)) => Some(link),
            _ => None,
        }
    }
}

/// What the next sync would do, from the server's last answer (clauses 44
/// and 46): never claimed fresher than that answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Next {
    /// Not asked: not set up, or no server to ask.
    #[default]
    Unasked,
    Asking,
    /// The plan as a dry run made it, `asked` then (Unix seconds).
    Plan(Box<Planned>),
    /// The server didn't answer.
    Unreached { asked: u64, why: String },
    /// The saved sign-in was refused.
    SignIn,
    /// What a sync won't do yet, in the sync's words.
    Refused(String),
}

/// A dry run's plan, as the Card view counts it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Planned {
    pub(crate) asked: u64,
    /// Files to fetch, and their bytes; conversions among them.
    pub(crate) new: usize,
    pub(crate) bytes: u64,
    pub(crate) conversions: usize,
    /// Every path a sync would remove.
    pub(crate) removals: Vec<String>,
    /// Plays waiting for this server.
    pub(crate) plays: usize,
    pub(crate) tally: Option<Tally>,
    pub(crate) fit: Option<Fit>,
    pub(crate) guards: Vec<Guard>,
    /// The server's list stood (a 304): unchanged since its last whole read.
    pub(crate) unchanged: bool,
    /// Every library the account sees, and the listed ones' files and bytes.
    pub(crate) offered: Vec<String>,
    pub(crate) totals: Vec<(String, usize, u64)>,
    /// The peers the card names that only a parent's proxy reaches (clause
    /// 58's proxy gate), by id.
    pub(crate) ask_proxy: Vec<u32>,
}

/// A sync under way, as its progress says it (clause 51).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Run {
    pub(crate) phase: Phase,
    /// The plan's files and bytes, once made.
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) done_files: usize,
    pub(crate) done_bytes: u64,
    /// The file being fetched, as the card will name it.
    pub(crate) now: Option<String>,
    /// Bytes a second, once known.
    pub(crate) rate: Option<f64>,
    /// When the last commit was made (Unix seconds).
    pub(crate) saved: Option<u64>,
    /// *Stop at the next save* was asked.
    pub(crate) stopping: bool,
    /// Files skipped so far.
    pub(crate) skipped: usize,
    /// When it started (Unix seconds).
    pub(crate) started: u64,
}

/// A sync's phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Waiting its turn behind another card's (clause 56).
    Waiting,
    /// Asking the server, listing its libraries.
    #[default]
    Listing,
    /// Working out what changes.
    Planning,
    /// Copying: the bar.
    Copying,
    /// The card's index, covers and Auto DJ.
    Index,
    Verifying,
}

/// A sync that waits by itself (clause 52): the run parked, to be taken up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pause {
    /// A peer's day is spent: goes on after `until` (Unix seconds); the
    /// days its files take at this pace.
    Budget { until: u64, peer: String, days: u64 },
    /// The server didn't answer: asked again at `until`, the `tries`th time.
    Server { until: u64, tries: u32 },
    /// The sign-in was refused: Sign in ▸.
    SignIn,
}

/// What the last sync of the card came to, this session (clauses 53, 54).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ended {
    /// Done and verified: Eject ▸.
    Done { tracks: usize, took: u64, skipped: Vec<(String, String)>, plays: usize },
    /// Stopped at a save, the plan on the card: `at` of `of`.
    Stopped { at: usize, of: usize },
    Paused { pause: Pause, at: usize, of: usize, bytes: (u64, u64) },
    /// The card filled up: stopped at the last save.
    Full { at: usize, of: usize },
    /// Write-protected mid-run.
    Locked { at: usize, of: usize },
    /// Taken out: put it back to go on.
    Out { at: usize, of: usize },
    /// Another card is in the reader now.
    OtherCard,
    /// The plan doesn't fit, or passes the track cap.
    Fit(Fit),
    /// A refused source, its peer's id and why.
    Refused { peer: u32, why: String },
    /// Anything else, in the run's own words.
    Failed(String),
}

/// A sync's question, from a plan's guards (clause 58).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// More than 50 removals: their count, the first paths.
    Removals { n: usize, first: Vec<String> },
    /// More than half the ledger: the count, and each library's.
    Half { n: usize, total: usize, by_library: Vec<(String, usize, usize)> },
    /// Files the listener has that the server also has.
    Adopt { n: usize, all: usize },
    /// The card's server would change.
    Server { rows: usize },
    /// A peer reachable only through its parent's proxy.
    Proxy { peer: u32 },
    /// A disk check first: Check again ▸ answers it.
    DiskCheck,
}

/// A card as the worker knows it now: what the page draws.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CardView {
    pub(crate) key: String,
    pub(crate) found: Found,
    pub(crate) read: Read,
    pub(crate) server: Server,
    pub(crate) next: Next,
    pub(crate) run: Option<Run>,
    /// What the last sync stopped to ask.
    pub(crate) gate: Option<Vec<Gate>>,
    pub(crate) ended: Option<Ended>,
    /// Taken out during its sync: kept, marked `out` (clause 53).
    pub(crate) out: bool,
    /// A job on it other than a sync: set-up, settings, plays, eject.
    pub(crate) busy: Option<Job>,
    /// The sheet's fit, by its request's seq.
    pub(crate) fit: Option<(u64, Next)>,
    /// The listener chose another server in its settings: the next sync
    /// takes it over (the another-server gate's yes).
    pub(crate) take_over: bool,
}

/// A job other than a sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Job {
    Look,
    SetUp,
    Apply,
    Plays,
    Eject,
}

impl CardView {
    fn new(found: Found) -> CardView {
        let read = if found.accepted() { Read::Reading } else { Read::Refused };
        CardView {
            key: key_of(&found),
            found,
            read,
            server: Server::New(None),
            next: Next::Unasked,
            run: None,
            gate: None,
            ended: None,
            out: false,
            busy: None,
            fit: None,
            take_over: false,
        }
    }

    /// The facts read off it, once read.
    pub(crate) fn facts(&self) -> Option<&Facts> {
        match &self.read {
            Read::Read(facts) => Some(facts),
            _ => None,
        }
    }

    /// The card's name for a tab or a sentence: its label, else the
    /// volume's, else none.
    pub(crate) fn name(&self) -> Option<String> {
        let own = self.facts().and_then(Facts::name).map(str::to_string);
        own.or_else(|| Some(self.found.volume.label.trim().to_string()).filter(|l| !l.is_empty()))
    }

    /// Where it is mounted, short: `E:` on Windows, the mount's last part
    /// elsewhere.
    pub(crate) fn drive(&self) -> String {
        drive_of(&self.found)
    }

    /// Its sync's percentage of the plan's bytes, while one runs.
    pub(crate) fn percent(&self) -> Option<u8> {
        let run = self.run.as_ref().filter(|r| r.phase != Phase::Waiting)?;
        if run.bytes == 0 {
            return Some(if run.phase >= Phase::Index { 100 } else { 0 });
        }
        Some(((run.done_bytes.min(run.bytes) * 100) / run.bytes) as u8)
    }

    /// A sync runs on it (not one waiting its turn).
    pub(crate) fn syncing(&self) -> bool {
        self.run.as_ref().is_some_and(|r| r.phase != Phase::Waiting)
    }

    /// Paused by itself, waiting to go on.
    pub(crate) fn paused(&self) -> Option<&Pause> {
        match &self.ended {
            Some(Ended::Paused { pause, .. }) => Some(pause),
            _ => None,
        }
    }
}

impl PartialOrd for Phase {
    fn partial_cmp(&self, other: &Phase) -> Option<std::cmp::Ordering> {
        let rank = |p: &Phase| *p as u8;
        rank(self).partial_cmp(&rank(other))
    }
}

/// A line of a card's log, in Details (clause 44): what happened, the
/// page's to word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LogItem {
    /// Read when put in: its name, revision and tracks.
    PutIn { card: String, revision: u32, tracks: usize },
    /// Its play log read: new plays, and those to send.
    Plays { new: usize, send: usize },
    /// Its server asked: new, removed, and whether its list stood.
    Asked { server: String, new: usize, removed: usize, unchanged: bool },
    /// A file the listener changed was kept; the server's copy beside it.
    KeptYours { file: String, server: String, copy: String },
    /// A file skipped, and why.
    Skipped { path: String, why: String },
    /// Anything else, in the run's own words.
    Said(String),
}

/// A passing line on the busy row (clauses 29, 54).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    PutIn { drive: String },
    /// Read: what the next sync would do.
    Read { key: String },
    Out { drive: String, card: String },
    OutRun { drive: String, card: String, at: usize, of: usize },
    Synced { card: String, took: u64, tracks: usize },
    Ejected { card: String },
    EjectFailed { card: String, why: String },
    PlaysSent { n: usize, server: String },
    Failed { why: String },
}

/// What the worker tells.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The readers looked at; `failed`: the volumes couldn't be listed.
    Watched { failed: Option<String> },
    /// A card, whole.
    Card(Box<CardView>),
    /// A card's run's progress alone.
    Progress { key: String, run: Run },
    Gone { key: String },
    Log { key: String, item: LogItem },
    Note(Note),
    /// Nothing runs: a quit may end.
    Idle,
}

/// A volume's key: where it is mounted.
pub(crate) fn key_of(found: &Found) -> String {
    found.volume.path.display().to_string()
}

/// `E:` on Windows; the mount point's last part elsewhere.
pub(crate) fn drive_of(found: &Found) -> String {
    let path = found.volume.path.to_string_lossy().to_string();
    let trimmed = path.trim_end_matches(['\\', '/']);
    if cfg!(windows) || (trimmed.len() == 2 && trimmed.ends_with(':')) {
        return trimmed.to_string();
    }
    trimmed.rsplit('/').find(|p| !p.is_empty()).unwrap_or(trimmed).to_string()
}

/// A removable volume worth listing (clause 63): every accepted card, and a
/// refused one only where it is card-shaped — never a system disk or a
/// fixed one.
pub(crate) fn listed(found: &Found) -> bool {
    !found.refusals.iter().any(|r| matches!(r, Refusal::System(_) | Refusal::Fixed))
}

// ── The link and the fold ───────────────────────────────────────────────────

/// The worker's inbox: the page's commands, and its jobs' reports.
pub(crate) enum In {
    Cmd(Cmd),
    Job(String, JobDone),
    RunEvent(String, RunEvent),
}

/// The page's way to the worker: a clone of its inbox's sender.
#[derive(Clone)]
pub(crate) struct Link {
    tx: Sender<In>,
}

impl Link {
    /// False when the worker is gone.
    pub(crate) fn send(&self, cmd: Cmd) -> bool {
        self.tx.send(In::Cmd(cmd)).is_ok()
    }

    /// A link whose far end the caller holds (the tests').
    pub(crate) fn pair() -> (Link, Receiver<In>) {
        let (tx, rx) = channel();
        (Link { tx }, rx)
    }
}

/// The commands sent down a test's link.
pub(crate) fn sent(rx: &Receiver<In>) -> Vec<Cmd> {
    rx.try_iter()
        .filter_map(|i| match i {
            In::Cmd(cmd) => Some(cmd),
            _ => None,
        })
        .collect()
}

/// The worker's reports folded: what the GUI's top bar and the page read.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fold {
    /// Every card, by drive.
    pub(crate) cards: Vec<CardView>,
    /// The readers were looked at once at least.
    pub(crate) watched: bool,
    /// The volumes couldn't be listed: why.
    pub(crate) failed: Option<String>,
    /// Busy-line notes and log lines not yet taken by a page.
    pub(crate) notes: Vec<Note>,
    pub(crate) logs: Vec<(String, LogItem)>,
    /// The worker said nothing runs.
    pub(crate) idle: bool,
    /// The servers saved on this computer, as the worker was last told
    /// them: the Settings sheet's From (clause 48).
    pub(crate) servers: Vec<ServerLink>,
}

impl Fold {
    pub(crate) fn apply(&mut self, event: Event) {
        match event {
            Event::Watched { failed } => {
                self.watched = true;
                self.failed = failed;
            }
            Event::Card(card) => {
                self.idle = false;
                match self.cards.iter_mut().find(|c| c.key == card.key) {
                    Some(known) => *known = *card,
                    None => {
                        self.cards.push(*card);
                        self.cards.sort_by_key(|c| c.drive().to_lowercase());
                    }
                }
            }
            Event::Progress { key, run } => {
                if let Some(card) = self.cards.iter_mut().find(|c| c.key == key) {
                    card.run = Some(run);
                }
            }
            Event::Gone { key } => self.cards.retain(|c| c.key != key),
            Event::Log { key, item } => {
                self.logs.push((key, item));
                // A page reads them as they come; with none up, the last few.
                if self.logs.len() > 400 {
                    self.logs.drain(..200);
                }
            }
            Event::Note(note) => {
                self.notes.push(note);
                if self.notes.len() > 8 {
                    self.notes.remove(0);
                }
            }
            Event::Idle => self.idle = true,
        }
    }

    /// The card whose sync runs, if one does.
    pub(crate) fn syncing(&self) -> Option<&CardView> {
        self.cards.iter().find(|c| c.syncing())
    }

    /// The card a sync is under way or paused on: what the tab's
    /// percentage and the mini player's line follow (clauses 16, 55).
    pub(crate) fn sync_mark(&self) -> Option<(&CardView, u8, bool)> {
        if let Some(card) = self.syncing() {
            return Some((card, card.percent().unwrap_or(0), false));
        }
        let paused = self.cards.iter().find(|c| c.paused().is_some())?;
        let pct = match &paused.ended {
            Some(Ended::Paused { bytes: (done, all), .. }) if *all > 0 => (done * 100 / all) as u8,
            _ => 0,
        };
        Some((paused, pct.min(100), true))
    }
}

// ── The worker ──────────────────────────────────────────────────────────────

/// A job's answer.
pub(crate) enum JobDone {
    /// The card read, and its server's answer to a dry run.
    Read(Result<Box<Facts>, String>),
    Looked(Next),
    Fit(u64, Next),
    SetUp(Result<(), String>),
    Applied(Result<(), String>),
    Plays(Result<Tally, String>),
    Ejected(Result<(), String>),
    Synced(Box<Finished>),
}

/// What a sync's thread came back with.
pub(crate) struct Finished {
    synced: Option<sync::Synced>,
    /// Why it couldn't start (the server, the card), when it didn't.
    why: Option<String>,
    /// The tracks by library on the card, for the stronger gate's counts.
    by_library: Vec<(String, usize)>,
    /// The removals' libraries: each path's vpath, from the ledger.
    removal_libs: Vec<String>,
    /// The listener stopped it.
    stopped: bool,
}

/// Starts the worker on its own thread: the link to it and its reports.
pub(crate) fn spawn(setup: Setup) -> (Link, Receiver<Event>) {
    let (tx, rx) = channel();
    let (out, events) = channel();
    let link = Link { tx: tx.clone() };
    let worker = Worker::new(setup, tx, out);
    std::thread::Builder::new()
        .name("card-transfer".into())
        .spawn(move || worker.run(rx))
        .expect("the transfer worker's thread");
    (link, events)
}

/// The one sync: its card and its stop.
struct Running {
    key: String,
    stop: Arc<AtomicBool>,
}

struct Slot {
    view: CardView,
    /// Read again once the job on it is done.
    look_again: bool,
    /// Its last tell of progress.
    told: Option<Instant>,
    /// A paused sync's retry: when, and its gates.
    retry: Option<(u64, Allow)>,
    /// What was last asked of its server, for a retry's backoff.
    tries: u32,
    /// The run's files written beside one of the same name (` (2)`): where
    /// the server's copy of a file the listener changed went.
    copies: Vec<String>,
    /// Commands for it that came while a job ran on it: run after.
    deferred: Vec<Cmd>,
}

struct Worker {
    setup: Setup,
    me: Sender<In>,
    out: Sender<Event>,
    servers: Vec<ServerLink>,
    slots: Vec<Slot>,
    running: Option<Running>,
    /// Syncs waiting their turn, with their gates' answers.
    queue: Vec<(String, Allow)>,
    quitting: bool,
}

impl Worker {
    fn new(setup: Setup, me: Sender<In>, out: Sender<Event>) -> Worker {
        Worker {
            setup,
            me,
            out,
            servers: Vec::new(),
            slots: Vec::new(),
            running: None,
            queue: Vec::new(),
            quitting: false,
        }
    }

    fn run(mut self, inbox: Receiver<In>) {
        let mut next_watch = Instant::now();
        loop {
            if Instant::now() >= next_watch {
                self.watch();
                self.retries();
                next_watch = Instant::now() + self.setup.every;
            }
            let wait = next_watch.saturating_duration_since(Instant::now());
            match inbox.recv_timeout(wait) {
                Ok(In::Cmd(cmd)) => self.cmd(cmd),
                Ok(In::Job(key, done)) => self.done(&key, done),
                Ok(In::RunEvent(key, event)) => self.progress(&key, event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.quitting && self.running.is_none() {
                let _ = self.out.send(Event::Idle);
                return;
            }
        }
    }

    fn tell(&self, event: Event) {
        let _ = self.out.send(event);
    }

    fn slot(&mut self, key: &str) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|s| s.view.key == key)
    }

    fn told(&self, key: &str) {
        if let Some(slot) = self.slots.iter().find(|s| s.view.key == key) {
            self.tell(Event::Card(Box::new(slot.view.clone())));
        }
    }

    // ── The readers ─────────────────────────────────────────────────────────

    /// The readers looked at: cards put in are read, cards taken out go —
    /// but for a card in its sync, which keeps its tab (clause 53).
    fn watch(&mut self) {
        let found = match self.setup.volumes.find() {
            Ok(found) => found,
            Err(e) => {
                self.tell(Event::Watched { failed: Some(e.to_string()) });
                return;
            }
        };
        let hidden = self.setup.home.cards();
        let shown: Vec<Found> = found
            .into_iter()
            .filter(listed)
            .filter(|f| f.volume.serial.as_ref().is_none_or(|s| !hidden.is_hidden(&s.to_string())))
            .collect();
        // Gone, or another card in that reader now (another serial).
        let gone: Vec<String> = self
            .slots
            .iter()
            .filter(|s| !s.view.out)
            .filter(|s| {
                !shown.iter().any(|f| key_of(f) == s.view.key && same_volume(f, &s.view.found))
            })
            .map(|s| s.view.key.clone())
            .collect();
        for key in gone {
            self.taken_out(&key);
        }
        // A card taken out mid-run, back in any reader: the run goes on
        // from its last save at the next Sync.
        for f in &shown {
            let key = key_of(f);
            let back = self.slots.iter().position(|s| s.view.out && same_volume(f, &s.view.found));
            if let Some(at) = back {
                let slot = self.slots.remove(at);
                self.tell(Event::Gone { key: slot.view.key.clone() });
            }
            if self.slots.iter().any(|s| s.view.key == key) {
                continue;
            }
            let view = CardView::new(f.clone());
            let accepted = f.accepted();
            let slot = Slot {
                view,
                look_again: false,
                told: None,
                retry: None,
                tries: 0,
                copies: Vec::new(),
                deferred: Vec::new(),
            };
            self.slots.push(slot);
            self.told(&key);
            if accepted {
                self.tell(Event::Note(Note::PutIn { drive: drive_of(f) }));
                self.look(&key);
            }
        }
        self.tell(Event::Watched { failed: None });
    }

    fn taken_out(&mut self, key: &str) {
        let running = self.running.as_ref().is_some_and(|r| r.key == key);
        let Some(at) = self.slots.iter().position(|s| s.view.key == key) else { return };
        let drive = self.slots[at].view.drive();
        let card = self.slots[at].view.name().unwrap_or_else(|| drive.clone());
        if running {
            // Its run stops on its own at its next write (the card is gone);
            // the tab stays, marked out, until it is back or given up.
            let slot = &mut self.slots[at];
            slot.view.out = true;
            let (at_n, of) = slot.view.run.as_ref().map_or((0, 0), |r| (r.done_files, r.files));
            self.tell(Event::Note(Note::OutRun { drive, card, at: at_n, of }));
            self.told(key);
            return;
        }
        let slot = self.slots.remove(at);
        self.queue.retain(|(k, _)| k != key);
        if slot.view.found.accepted() {
            self.tell(Event::Note(Note::Out { drive, card }));
        }
        self.tell(Event::Gone { key: key.to_string() });
    }

    /// Paused syncs whose time has come go on.
    fn retries(&mut self) {
        let now = self.setup.home.now();
        let due: Vec<(String, Allow)> = self
            .slots
            .iter_mut()
            .filter_map(|s| match s.retry.take() {
                Some((at, allow)) if at <= now => Some((s.view.key.clone(), allow)),
                other => {
                    s.retry = other;
                    None
                }
            })
            .collect();
        for (key, allow) in due {
            self.sync(&key, allow);
        }
    }

    // ── Commands ────────────────────────────────────────────────────────────

    fn cmd(&mut self, cmd: Cmd) {
        // A job on a card waits for the one under way on it, a sync's too.
        let job_key = match &cmd {
            Cmd::SetUp { key, .. }
            | Cmd::Apply { key, .. }
            | Cmd::Plays { key }
            | Cmd::Eject { key }
            | Cmd::NewIdentity { key } => Some(key.clone()),
            _ => None,
        };
        if let Some(key) = job_key {
            let running = self.running.as_ref().is_some_and(|r| r.key == key);
            if let Some(slot) = self.slot(&key)
                && (slot.view.busy.is_some() || running)
            {
                slot.deferred.push(cmd);
                return;
            }
        }
        match cmd {
            Cmd::Servers(servers) => {
                let changed = servers != self.servers;
                self.servers = servers;
                if changed {
                    // Each card's server again; a card that couldn't be asked
                    // is asked now (one being read is read again after).
                    let keys: Vec<String> = self.slots.iter().map(|s| s.view.key.clone()).collect();
                    for key in keys {
                        self.resolve(&key);
                        let ask = self.slot(&key).is_some_and(|s| {
                            let unasked = matches!(
                                s.view.next,
                                Next::Unasked | Next::SignIn | Next::Unreached { .. }
                            );
                            s.view.found.accepted() && unasked
                        });
                        if ask {
                            self.look(&key);
                        }
                    }
                }
            }
            Cmd::Look { key } => self.look(&key),
            Cmd::Fit { key, want, seq } => self.fit(&key, *want, seq),
            Cmd::SetUp { key, want } => self.set_up(&key, *want),
            Cmd::Apply { key, want } => self.apply(&key, *want),
            Cmd::Sync { key, allow } => {
                if let Some(slot) = self.slot(&key) {
                    slot.retry = None;
                    slot.view.gate = None;
                }
                self.sync(&key, *allow)
            }
            Cmd::DontWait { key } => {
                self.queue.retain(|(k, _)| *k != key);
                if let Some(slot) = self.slot(&key) {
                    slot.view.run = None;
                }
                self.told(&key);
            }
            Cmd::Stop { key } => {
                if let Some(running) = self.running.as_ref().filter(|r| r.key == key) {
                    running.stop.store(true, Ordering::SeqCst);
                    if let Some(slot) = self.slot(&key)
                        && let Some(run) = slot.view.run.as_mut()
                    {
                        run.stopping = true;
                    }
                    self.told(&key);
                }
            }
            Cmd::StopPause { key } => {
                if let Some(slot) = self.slot(&key) {
                    slot.retry = None;
                    if let Some(Ended::Paused { at, of, .. }) = slot.view.ended.clone() {
                        slot.view.ended = Some(Ended::Stopped { at, of });
                    }
                }
                self.told(&key);
            }
            Cmd::Plays { key } => self.plays(&key),
            Cmd::Eject { key } => self.eject(&key),
            Cmd::Hide { key } => {
                let serial = self.slot(&key).and_then(|s| s.view.found.volume.serial.clone());
                if let Some(serial) = serial {
                    let mut cards = self.setup.home.cards();
                    cards.hide(&serial.to_string());
                    let _ = self.setup.home.save_cards(&cards);
                    self.slots.retain(|s| s.view.key != key);
                    self.tell(Event::Gone { key });
                }
            }
            Cmd::ShowHidden => {
                let mut cards = self.setup.home.cards();
                if !cards.hidden.is_empty() {
                    cards.show_hidden();
                    let _ = self.setup.home.save_cards(&cards);
                    self.watch();
                }
            }
            Cmd::NewIdentity { key } => {
                let facts = self.slot(&key).and_then(|s| s.view.facts().cloned());
                if let Some(name) = facts.map(|f| f.name().unwrap_or("").to_string()) {
                    self.job(&key, Job::SetUp, move |card, home, _| {
                        let ask = setup::Ask { label: name, new_identity: true };
                        let mut cards = home.cards();
                        let mut envs = home.envs();
                        let done = setup::set_up(card, &ask, &mut *envs, &mut cards, &mut |_| {});
                        let done = done.map(|_| ()).map_err(|stop| stop.to_string());
                        if done.is_ok() {
                            let _ = home.save_cards(&cards);
                        }
                        JobDone::SetUp(done)
                    });
                }
            }
            Cmd::GiveUp { key } => {
                if self.slot(&key).is_some_and(|s| s.view.out) {
                    self.slots.retain(|s| s.view.key != key);
                    self.tell(Event::Gone { key });
                }
            }
            Cmd::Dismiss { key } => {
                if let Some(slot) = self.slot(&key) {
                    slot.view.gate = None;
                }
                self.told(&key);
            }
            Cmd::Quit => {
                self.quitting = true;
                self.queue.clear();
                if let Some(running) = &self.running {
                    running.stop.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// The card's server, by the key it names (clause 3a).
    fn resolve(&mut self, key: &str) {
        let servers = self.servers.clone();
        let Some(slot) = self.slot(key) else { return };
        let Some(facts) = slot.view.facts().cloned() else { return };
        slot.view.server = server_of(&facts, &servers, slot.view.take_over);
    }

    /// A job on `key`'s card, on a thread of its own: `work` gets the card,
    /// the home and its server's link; one job a card at a time.
    fn job<F>(&mut self, key: &str, job: Job, work: F)
    where
        F: FnOnce(&dyn CardWrite, &dyn Home, Option<ServerLink>) -> JobDone + Send + 'static,
    {
        let running = self.running.as_ref().is_some_and(|r| r.key == key);
        let Some(slot) = self.slot(key) else { return };
        if slot.view.busy.is_some() || running || !slot.view.found.accepted() {
            if job == Job::Look {
                slot.look_again = true;
            }
            return;
        }
        slot.view.busy = Some(job);
        let found = slot.view.found.clone();
        let link = slot.view.server.link().cloned();
        let (volumes, home) = (self.setup.volumes.clone(), self.setup.home.clone());
        let (me, key) = (self.me.clone(), key.to_string());
        self.told(&key);
        std::thread::spawn(move || {
            let done = match volumes.open(&found) {
                Some(card) => work(&*card, &*home, link),
                None => JobDone::Read(Err("the volume can't be opened".into())),
            };
            let _ = me.send(In::Job(key, done));
        });
    }

    /// Reads the card, then asks its server what a sync would do.
    fn look(&mut self, key: &str) {
        let servers = self.servers.clone();
        let take_over = self.slot(key).is_some_and(|s| s.view.take_over);
        let space = self.slot(key).and_then(|s| s.view.found.volume.space);
        let me = self.me.clone();
        let key_owned = key.to_string();
        self.job(key, Job::Look, move |card, home, _| {
            let facts = match read(card, &home.cards()) {
                Ok(facts) => facts,
                Err(e) => return JobDone::Read(Err(e)),
            };
            let _ = me.send(In::Job(key_owned.clone(), JobDone::Read(Ok(Box::new(facts.clone())))));
            let server = server_of(&facts, &servers, take_over);
            match (facts.set_up(), server.link()) {
                (true, Some(link)) if link.base.is_ok() => {
                    JobDone::Looked(dry_run(card, home, link, &sync::Ask::default(), space))
                }
                _ => JobDone::Looked(Next::Unasked),
            }
        });
    }

    /// A dry run with the sheet's settings: the fit line.
    fn fit(&mut self, key: &str, want: Want, seq: u64) {
        let servers = self.servers.clone();
        let Some(slot) = self.slot(key) else { return };
        let Some(facts) = slot.view.facts().cloned() else { return };
        let link = match &want.server {
            Some(id) => servers.iter().find(|s| s.id == *id).cloned(),
            None => slot.view.server.link().cloned(),
        };
        slot.view.fit = Some((seq, Next::Asking));
        let Some(link) = link.filter(|l| l.base.is_ok() && facts.set_up()) else {
            slot.view.fit = Some((seq, Next::Unasked));
            self.told(key);
            return;
        };
        let found = slot.view.found.clone();
        let (volumes, home) = (self.setup.volumes.clone(), self.setup.home.clone());
        let (me, key) = (self.me.clone(), key.to_string());
        self.told(&key);
        // A fit is a read alone (a dry run): it runs beside another job.
        std::thread::spawn(move || {
            let next = match volumes.open(&found) {
                Some(card) => {
                    let ask = sync::Ask {
                        libraries: want.libraries.clone(),
                        change: want.change(None),
                        ..sync::Ask::default()
                    };
                    dry_run(&*card, &*home, &link, &ask, found.volume.space)
                }
                None => Next::Refused("the volume can't be opened".into()),
            };
            let _ = me.send(In::Job(key, JobDone::Fit(seq, next)));
        });
    }

    fn set_up(&mut self, key: &str, want: Want) {
        let servers = self.servers.clone();
        self.job(key, Job::SetUp, move |card, home, _| {
            let ask = setup::Ask { label: want.name.trim().to_string(), new_identity: false };
            let mut cards = home.cards();
            let mut envs = home.envs();
            if let Err(stop) = setup::set_up(card, &ask, &mut *envs, &mut cards, &mut |_| {}) {
                return JobDone::SetUp(Err(stop.to_string()));
            }
            let _ = home.save_cards(&cards);
            let link = want.server.as_ref().and_then(|id| servers.iter().find(|s| s.id == *id));
            JobDone::SetUp(store(card, home, &want, link))
        });
    }

    fn apply(&mut self, key: &str, want: Want) {
        let servers = self.servers.clone();
        let current = self.slot(key).and_then(|s| s.view.server.link().map(|l| l.id.clone()));
        if let Some(slot) = self.slot(key) {
            // Another server chosen: the next sync takes the card over.
            slot.view.take_over = want.server.is_some() && want.server != current;
        }
        self.job(key, Job::Apply, move |card, home, _| {
            let link = want.server.as_ref().and_then(|id| servers.iter().find(|s| s.id == *id));
            JobDone::Applied(store(card, home, &want, link))
        });
    }

    fn plays(&mut self, key: &str) {
        self.job(key, Job::Plays, move |card, home, link| {
            let Some(link) = link else { return JobDone::Plays(Err("no server".into())) };
            let client = match link.client() {
                Ok(client) => client,
                Err(why) => return JobDone::Plays(Err(why)),
            };
            let (tally, _) = history::plays_of(card, &client, false, home.plays());
            if tally.failed {
                return JobDone::Plays(Err(tally.notes.join("; ")));
            }
            JobDone::Plays(Ok(tally))
        });
    }

    fn eject(&mut self, key: &str) {
        let running = self.running.as_ref().is_some_and(|r| r.key == key);
        let Some(slot) = self.slot(key) else { return };
        if running || slot.view.busy.is_some() {
            return;
        }
        slot.view.busy = Some(Job::Eject);
        let found = slot.view.found.clone();
        let (volumes, me, key) = (self.setup.volumes.clone(), self.me.clone(), key.to_string());
        self.told(&key);
        std::thread::spawn(move || {
            let done = volumes.eject(&found);
            let _ = me.send(In::Job(key, JobDone::Ejected(done)));
        });
    }

    /// A sync: started, or queued behind the running one (clause 56).
    fn sync(&mut self, key: &str, allow: Allow) {
        let running_here = self.running.as_ref().is_some_and(|r| r.key == key);
        let another = self.running.is_some();
        let started = self.setup.home.now();
        let accepted: Vec<PathBuf> = self
            .slots
            .iter()
            .filter(|s| s.view.found.accepted())
            .map(|s| s.view.found.volume.path.clone())
            .collect();
        let Some(slot) = self.slots.iter_mut().find(|s| s.view.key == key) else { return };
        if running_here || !slot.view.found.accepted() || slot.view.out {
            return;
        }
        if another || slot.view.busy.is_some() {
            slot.view.run = Some(Run { phase: Phase::Waiting, ..Run::default() });
            self.queue.retain(|(k, _)| k != key);
            self.queue.push((key.to_string(), allow));
            self.told(key);
            return;
        }
        let Some(link) = slot.view.server.link().cloned() else {
            slot.view.ended = Some(Ended::Failed("no server saved for this card".into()));
            self.told(key);
            return;
        };
        let facts = slot.view.facts().cloned().unwrap_or_default();
        slot.view.ended = None;
        slot.view.gate = None;
        slot.view.run = Some(Run { phase: Phase::Listing, started, ..Run::default() });
        let found = slot.view.found.clone();
        let stop = Arc::new(AtomicBool::new(false));
        self.running = Some(Running { key: key.to_string(), stop: stop.clone() });
        let (volumes, home) = (self.setup.volumes.clone(), self.setup.home.clone());
        let (me, key) = (self.me.clone(), key.to_string());
        self.told(&key);
        std::thread::spawn(move || {
            let (volumes, home) = (&*volumes, &*home);
            let on = Card { found: &found, volumes, home, link: &link, facts: &facts };
            let finished = run_sync(&on, &allow, stop.clone(), accepted, &me, &key);
            let _ = me.send(In::Job(key, JobDone::Synced(Box::new(finished))));
        });
    }

    /// The next sync waiting its turn, if any, once the running one ends.
    fn next_in_queue(&mut self) {
        if self.running.is_some() || self.queue.is_empty() || self.quitting {
            return;
        }
        let (key, allow) = self.queue.remove(0);
        self.sync(&key, allow);
    }

    // ── Reports ─────────────────────────────────────────────────────────────

    fn progress(&mut self, key: &str, event: RunEvent) {
        let now = self.setup.home.now();
        let Some(slot) = self.slot(key) else { return };
        let Some(run) = slot.view.run.as_mut() else { return };
        let mut log = None;
        let phase_was = run.phase;
        match &event {
            RunEvent::Listed => run.phase = Phase::Planning,
            RunEvent::Planned { files, bytes } => {
                run.phase = if *files == 0 { Phase::Index } else { Phase::Copying };
                (run.files, run.bytes) = (*files, *bytes);
            }
            RunEvent::Progress { path, bytes } => {
                run.now = Some(path.clone());
                let elapsed = now.saturating_sub(run.started).max(1) as f64;
                let so_far = run.done_bytes + bytes;
                run.rate = Some(so_far as f64 / elapsed);
            }
            RunEvent::Wrote { path, bytes } => {
                run.done_files += 1;
                run.done_bytes += bytes;
                if run.done_files >= run.files {
                    run.phase = Phase::Index;
                    run.now = None;
                }
                if path.contains(" (") && slot.copies.len() < 1000 {
                    slot.copies.push(path.clone());
                }
            }
            RunEvent::Failed { server_path, why } => {
                run.skipped += 1;
                run.done_files += 1;
                log = Some(LogItem::Skipped { path: server_path.clone(), why: why.clone() });
            }
            RunEvent::Committed { last, .. } => {
                run.saved = Some(now);
                if *last {
                    run.phase = Phase::Verifying;
                }
            }
            _ => {}
        }
        let urgent = run.phase != phase_was || !matches!(event, RunEvent::Progress { .. });
        let due = slot.told.is_none_or(|t| t.elapsed() >= TELL_EVERY);
        if urgent || due {
            slot.told = Some(Instant::now());
            let run = run.clone();
            self.tell(Event::Progress { key: key.to_string(), run });
        }
        if let Some(item) = log {
            self.tell(Event::Log { key: key.to_string(), item });
        }
    }

    fn done(&mut self, key: &str, done: JobDone) {
        let now = self.setup.home.now();
        match done {
            JobDone::Read(read) => {
                let Some(slot) = self.slot(key) else { return };
                match read {
                    Ok(facts) => {
                        let first = slot.view.read == Read::Reading;
                        let (name, tracks) = (facts.name().unwrap_or("").to_string(), facts.tracks);
                        let revision = facts.root.map_or(0, |r| r.1);
                        slot.view.read = Read::Read(facts);
                        if first {
                            let item = LogItem::PutIn { card: name, revision, tracks };
                            self.tell(Event::Log { key: key.to_string(), item });
                        }
                        self.resolve(key);
                        let asking = self.slot(key).is_some_and(|s| {
                            let set_up = s.view.facts().is_some_and(Facts::set_up);
                            set_up && s.view.server.link().is_some()
                        });
                        if let Some(slot) = self.slot(key) {
                            slot.view.next = if asking { Next::Asking } else { Next::Unasked };
                        }
                    }
                    Err(why) => {
                        slot.view.read = Read::Failed(why);
                        slot.view.busy = None;
                    }
                }
                self.told(key);
                return;
            }
            JobDone::Looked(next) => {
                if let Some(slot) = self.slot(key) {
                    slot.view.busy = None;
                    if let Next::Plan(plan) = &next {
                        let server = slot.view.server.link().map(|l| l.name.clone());
                        let (server, removed) = (server.unwrap_or_default(), plan.removals.len());
                        let (new, unchanged) = (plan.new, plan.unchanged);
                        let item = LogItem::Asked { server, new, removed, unchanged };
                        let plays = plan.tally.as_ref().map(|t| (t.new, plan.plays));
                        let key = key.to_string();
                        let _ = self.out.send(Event::Log { key: key.clone(), item });
                        if let Some((new, send)) = plays.filter(|(new, _)| *new > 0) {
                            let item = LogItem::Plays { new, send };
                            let _ = self.out.send(Event::Log { key, item });
                        }
                    }
                }
                if let Some(slot) = self.slot(key) {
                    let first = slot.view.next == Next::Asking;
                    slot.view.next = next;
                    if first {
                        let _ = self.out.send(Event::Note(Note::Read { key: key.to_string() }));
                    }
                }
            }
            JobDone::Fit(seq, next) => {
                if let Some(slot) = self.slot(key)
                    && slot.view.fit.as_ref().is_some_and(|(s, _)| *s == seq)
                {
                    slot.view.fit = Some((seq, next));
                }
            }
            JobDone::SetUp(result) | JobDone::Applied(result) => {
                if let Some(slot) = self.slot(key) {
                    slot.view.busy = None;
                    slot.look_again = true;
                    if let Err(why) = result {
                        self.tell(Event::Note(Note::Failed { why }));
                    }
                }
            }
            JobDone::Plays(result) => {
                let link = self.slot(key).and_then(|s| s.view.server.link().cloned());
                let server = link.map(|l| l.name).unwrap_or_default();
                if let Some(slot) = self.slot(key) {
                    slot.view.busy = None;
                    slot.look_again = true;
                }
                match result {
                    Ok(tally) => {
                        let n = tally.accepted + tally.duplicates;
                        self.tell(Event::Note(Note::PlaysSent { n, server }));
                    }
                    Err(why) => self.tell(Event::Note(Note::Failed { why })),
                }
            }
            JobDone::Ejected(result) => {
                let card = self.slot(key).map(|s| s.view.name().unwrap_or_else(|| s.view.drive()));
                if let Some(slot) = self.slot(key) {
                    slot.view.busy = None;
                }
                let card = card.unwrap_or_default();
                match result {
                    Ok(()) => self.tell(Event::Note(Note::Ejected { card })),
                    Err(why) => self.tell(Event::Note(Note::EjectFailed { card, why })),
                }
            }
            JobDone::Synced(finished) => {
                self.running = None;
                self.synced(key, *finished, now);
            }
        }
        // A command that waited for this job goes now, then a sync that
        // waited for it or for another's sync.
        let deferred = match self.slot(key) {
            Some(slot) if slot.view.busy.is_none() => std::mem::take(&mut slot.deferred),
            _ => Vec::new(),
        };
        let mut deferred = deferred.into_iter();
        if let Some(cmd) = deferred.next() {
            if let Some(slot) = self.slot(key) {
                slot.deferred.extend(deferred);
            }
            self.cmd(cmd);
        }
        let again = self.slot(key).is_some_and(|s| s.look_again && s.view.busy.is_none());
        self.next_in_queue();
        if again && self.running.as_ref().is_none_or(|r| r.key != key) {
            if let Some(slot) = self.slot(key) {
                slot.look_again = false;
            }
            self.look(key);
        } else {
            self.told(key);
        }
    }

    /// A sync's end, as the card shows it (clauses 52–54).
    fn synced(&mut self, key: &str, finished: Finished, now: u64) {
        let Some(slot) = self.slot(key) else { return };
        let run = slot.view.run.take().unwrap_or_default();
        let (at, of) = (run.done_files, run.files);
        let took = now.saturating_sub(run.started);
        let name = slot.view.name().unwrap_or_else(|| slot.view.drive());
        let mut note = None;
        let mut retry = None;
        let Some(synced) = finished.synced else {
            let why = finished.why.unwrap_or_default();
            slot.view.ended = Some(Ended::Failed(why));
            self.told(key);
            return;
        };
        let peers_refused: Vec<(u32, String)> = synced
            .peers
            .iter()
            .filter(|p| p.status == PeerStatus::Refused)
            .map(|p| (p.id, p.why.clone().unwrap_or_default()))
            .collect();
        slot.view.ended = match &synced.end {
            sync::End::Dry => None,
            sync::End::Ran(outcome) => match &outcome.parked {
                None => {
                    let mut skipped: Vec<(String, String)> = outcome.report.failed.clone();
                    let refused = outcome.report.refused.iter();
                    let why = |s: &crate::device::card::names::Skipped| format!("{:?}", s.why);
                    skipped.extend(refused.map(|s| (s.server_path.clone(), why(s))));
                    let plays = synced.plays.as_ref().map_or(0, |t| t.accepted + t.duplicates);
                    match &outcome.verified {
                        Ok(()) => {
                            let tracks = outcome.records;
                            note = Some(Note::Synced { card: name.clone(), took, tracks });
                            match peers_refused.first() {
                                Some((peer, why)) => {
                                    Some(Ended::Refused { peer: *peer, why: why.clone() })
                                }
                                None => Some(Ended::Done { tracks, took, skipped, plays }),
                            }
                        }
                        Err(why) => Some(Ended::Failed(why.clone())),
                    }
                }
                Some(_) if finished.stopped => Some(Ended::Stopped { at, of }),
                Some(why) => {
                    let pause = match synced.resume_at {
                        Some(until) => {
                            let waiting = synced.peers.iter().find(|p| p.resume_at.is_some());
                            let peer = waiting.map(|p| p.id.to_string()).unwrap_or_default();
                            let days = waiting.map_or(0, |p| p.days);
                            Pause::Budget { until, peer, days }
                        }
                        None if why.contains("sign") => Pause::SignIn,
                        None => {
                            slot.tries += 1;
                            let wait = (RETRY_FIRST << (slot.tries - 1).min(5)).min(RETRY_MOST);
                            Pause::Server { until: now + wait, tries: slot.tries }
                        }
                    };
                    if let Pause::Budget { until, .. } | Pause::Server { until, .. } = &pause {
                        retry = Some(*until);
                    }
                    Some(Ended::Paused { pause, at, of, bytes: (run.done_bytes, run.bytes) })
                }
            },
            sync::End::Halted(halt) => match halt {
                sync::Halt::Stopped => Some(Ended::Stopped { at: 0, of }),
                sync::Halt::Fit(fit) => Some(Ended::Fit(*fit)),
                sync::Halt::Server(why) if signed_out(why) => {
                    let bytes = (run.done_bytes, run.bytes);
                    Some(Ended::Paused { pause: Pause::SignIn, at, of, bytes })
                }
                sync::Halt::Server(_) => {
                    slot.tries += 1;
                    let wait = (RETRY_FIRST << (slot.tries - 1).min(5)).min(RETRY_MOST);
                    retry = Some(now + wait);
                    let pause = Pause::Server { until: now + wait, tries: slot.tries };
                    Some(Ended::Paused { pause, at, of, bytes: (run.done_bytes, run.bytes) })
                }
                sync::Halt::Card(Stop::Unanswered(guards)) => {
                    let (by, libs) = (&finished.by_library, &finished.removal_libs);
                    slot.view.gate = Some(gates(guards, by, libs, slot.view.facts()));
                    None
                }
                sync::Halt::Card(Stop::Guard(Guard::DiskCheck(_))) => {
                    slot.view.gate = Some(vec![Gate::DiskCheck]);
                    None
                }
                sync::Halt::Card(Stop::Pulled) if slot.view.out => Some(Ended::Out { at, of }),
                sync::Halt::Card(Stop::Pulled) => Some(Ended::Out { at, of }),
                sync::Halt::Card(Stop::DifferentCard) => Some(Ended::OtherCard),
                sync::Halt::Card(Stop::Card(e)) if full(e) => Some(Ended::Full { at, of }),
                sync::Halt::Card(Stop::Card(e)) if read_only(e) => Some(Ended::Locked { at, of }),
                other => Some(Ended::Failed(other.to_string())),
            },
        };
        if slot.view.out && !matches!(slot.view.ended, Some(Ended::Out { .. })) {
            slot.view.ended = Some(Ended::Out { at, of });
        }
        if matches!(slot.view.ended, Some(Ended::Done { .. })) {
            slot.tries = 0;
        }
        if let Some(until) = retry {
            // The answers it ran with, again: a pause goes on as it was.
            slot.retry = Some((until, finished_allow(&slot.view)));
        }
        // What came of it is the next look's to say (a run that ended reads
        // as in step, or what is left).
        let gone = slot.view.out;
        slot.look_again = !gone;
        // A peer only its parent's proxy reaches is asked about once
        // (clause 58): the sync went on without it.
        let ask_proxy: Vec<Gate> = synced
            .peers
            .iter()
            .filter(|p| p.status == PeerStatus::AskProxy)
            .map(|p| Gate::Proxy { peer: p.id })
            .collect();
        if !ask_proxy.is_empty() && slot.view.gate.is_none() {
            slot.view.gate = Some(ask_proxy);
        }
        // A file the listener changed stays theirs; the server's copy went
        // beside it (clause 51).
        let copies = std::mem::take(&mut slot.copies);
        let server = slot.view.server.link().map(|l| l.name.clone()).unwrap_or_default();
        let mut kept = Vec::new();
        if let sync::End::Ran(outcome) = &synced.end {
            for file in &outcome.report.edited {
                let (stem, ext) = file.rsplit_once('.').unwrap_or((file.as_str(), ""));
                let beside = |c: &&String| c.starts_with(&format!("{stem} (")) && c.ends_with(ext);
                let copy = copies.iter().find(beside);
                let copy = copy.cloned().unwrap_or_default();
                kept.push(LogItem::KeptYours { file: file.clone(), server: server.clone(), copy });
            }
        }
        if let Some(note) = note {
            self.tell(Event::Note(note));
        }
        for item in kept {
            self.tell(Event::Log { key: key.to_string(), item });
        }
        for (path, why) in synced.summary.as_ref().map(|s| s.skipped.clone()).unwrap_or_default() {
            let item = LogItem::Skipped { path, why: why.code().to_string() };
            self.tell(Event::Log { key: key.to_string(), item });
        }
        for line in &synced.notes {
            self.tell(Event::Log { key: key.to_string(), item: LogItem::Said(line.clone()) });
        }
    }
}

/// The answers a paused sync is retried with: its take-over, if any.
fn finished_allow(view: &CardView) -> Allow {
    Allow { take_over: view.take_over, ..Allow::default() }
}

/// A sync's question from its plan's guards.
fn gates(
    guards: &[Guard],
    by_library: &[(String, usize)],
    removal_libs: &[String],
    facts: Option<&Facts>,
) -> Vec<Gate> {
    guards
        .iter()
        .filter_map(|g| match g {
            Guard::Removals { paths, strong: false } => {
                let first = paths.iter().take(GATE_PATHS).cloned().collect();
                Some(Gate::Removals { n: paths.len(), first })
            }
            Guard::Removals { paths, strong: true } => {
                let total = facts.map_or(0, |f| f.tracks);
                let mut by: Vec<(String, usize, usize)> = Vec::new();
                for lib in removal_libs {
                    match by.iter_mut().find(|(l, ..)| l == lib) {
                        Some((_, n, _)) => *n += 1,
                        None => {
                            let of = by_library.iter().find(|(l, _)| l == lib);
                            let of = of.map_or(0, |(_, n)| *n);
                            by.push((lib.clone(), 1, of));
                        }
                    }
                }
                Some(Gate::Half { n: paths.len(), total, by_library: by })
            }
            Guard::Adopt(paths) => {
                let all = facts.map_or(paths.len(), |f| f.music.unlisted as usize);
                Some(Gate::Adopt { n: paths.len(), all: all.max(paths.len()) })
            }
            Guard::Server { rows, .. } => Some(Gate::Server { rows: *rows }),
            Guard::DiskCheck(_) => Some(Gate::DiskCheck),
            Guard::Newer(_) => None,
        })
        .collect()
}

/// A write the card refused for want of room.
fn full(e: &io::Error) -> bool {
    // ERROR_DISK_FULL on Windows, ENOSPC elsewhere.
    e.kind() == io::ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(112) | Some(28))
}

/// A write the card refused as read-only (its lock switch).
fn read_only(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::ReadOnlyFilesystem
        || e.kind() == io::ErrorKind::PermissionDenied
        || e.raw_os_error() == Some(19)
}

/// The server's answer refused the saved sign-in.
fn signed_out(why: &str) -> bool {
    why.contains("401") || why.contains("403") || why.to_lowercase().contains("sign")
}

/// Two volumes are the same card: one serial (a card swapped into the
/// reader has another).
fn same_volume(a: &Found, b: &Found) -> bool {
    a.volume.serial == b.volume.serial && a.volume.fs == b.volume.fs
}

/// The card's server for this computer (clause 3a): the saved server whose
/// key the card names, by its root's server 0, else its settings'; a new
/// card's, the session's, else the first saved. `take_over`: the settings'
/// first source is the one chosen.
pub(crate) fn server_of(facts: &Facts, servers: &[ServerLink], take_over: bool) -> Server {
    let settings = facts.state.as_ref().map(|s| &s.settings);
    let chosen = settings.and_then(|s| s.sources.iter().find(|src| !src.libraries.is_empty()));
    let key = match (take_over, chosen) {
        (true, Some(source)) => Some(source.url_key),
        _ => facts.server,
    };
    let Some(key) = key else {
        let new = servers.iter().find(|s| s.session).or_else(|| servers.first());
        return Server::New(new.cloned());
    };
    let confirmed: Vec<u64> = settings
        .and_then(|s| s.sources.iter().find(|src| src.url_key == key))
        .map(|s| s.confirmed.clone())
        .unwrap_or_default();
    let found = servers.iter().find(|s| {
        let keys = s.keys();
        keys.contains(&key) || keys.iter().any(|k| confirmed.contains(k))
    });
    match found {
        Some(link) => Server::Saved(link.clone()),
        None => Server::Unknown(facts.nickname.clone()),
    }
}

/// A card read as the device reads it, with nothing written.
pub(crate) fn read(card: &dyn CardWrite, cards: &Cards) -> Result<Facts, String> {
    let seen = volume::inspect(card).map_err(|e| e.to_string())?;
    let root = seen.root.as_ref().map(|r| {
        let m = &r.manifest;
        (m.header.card_id, m.header.generation, m.commit_id)
    });
    let state = State::of(&seen.state).filter(|s| root.is_some_and(|r| r.0 == s.card_id));
    let serial = card.serial().map_err(|e| e.to_string())?;
    let copied = match (&state, root) {
        (Some(state), Some((id, generation, commit))) => {
            let by_serial = state::copied(state, id, serial.as_ref()).is_some();
            by_serial || cards.diverged(id, generation, commit).is_some()
        }
        _ => false,
    };
    let records = seen.root.as_ref().and_then(|r| r.tags.as_ref().ok());
    let mut by_library: Vec<(String, usize)> = Vec::new();
    let mut tracks = 0;
    for entry in records.map(|t| t.records.as_slice()).unwrap_or_default() {
        let Some(ledger) = &entry.ledger else { continue };
        tracks += 1;
        let lib = ledger.server_path.split('/').next().unwrap_or_default();
        match by_library.iter_mut().find(|(l, _)| l == lib) {
            Some((_, n)) => *n += 1,
            None => by_library.push((lib.to_string(), 1)),
        }
    }
    let table = seen.root.as_ref().map(|r| r.manifest.server_table());
    let zero = table.as_ref().map(|t| t.root).filter(|s| s.url_key != 0 && tracks > 0);
    let server = zero.map(|s| s.url_key).or_else(|| {
        let sources = &state.as_ref()?.settings.sources;
        sources.iter().find(|s| !s.libraries.is_empty()).or(sources.first()).map(|s| s.url_key)
    });
    let nickname = state
        .as_ref()
        .and_then(|s| s.settings.sources.iter().find(|src| Some(src.url_key) == server))
        .map(|s| s.nickname.clone())
        .unwrap_or_default();
    let set = state.as_ref().and_then(|s| {
        let set = s.set.as_ref()?;
        Some((set.slot_of(s.card_id)? + 1, set.cards.len()))
    });
    Ok(Facts {
        root,
        committed: seen.root.as_ref().map(|r| u64::from(r.manifest.commit_time)),
        tracks,
        by_library,
        newer: seen.newer.clone(),
        check: seen.root.as_ref().is_some_and(|r| r.twice) || seen.state.twice || seen.resume.twice,
        pending: seen.pending.is_some(),
        music: seen.music,
        device: seen.device.clone(),
        server,
        nickname,
        copied,
        set,
        producer: seen.root.as_ref().map(|r| r.manifest.producer.clone()).unwrap_or_default(),
        state,
    })
}

/// A dry run of the card's sync: what the next sync would do (clause 46).
fn dry_run(
    card: &dyn CardWrite,
    home: &dyn Home,
    link: &ServerLink,
    ask: &sync::Ask,
    space: Option<Space>,
) -> Next {
    let now = home.now();
    let client = match link.client() {
        Ok(client) => Arc::new(client),
        Err(why) => return Next::Unreached { asked: now, why },
    };
    // The look reads no peer: their listings spend their day.
    let ask = sync::Ask { dry_run: true, peers: Some(Vec::new()), stop: None, ..ask.clone() };
    let fed = match home.federation(Vec::new()) {
        Ok(fed) => fed,
        Err(why) => return Next::Refused(why),
    };
    let (mut cards, mut envs) = (home.cards(), home.envs());
    let (plays, reached) = (home.plays(), link.reached());
    let envs = &mut *envs;
    let (quiet, cards) = (&mut |_| {}, &mut cards);
    let synced = sync::sync(card, space, client, reached, &ask, cards, plays, &fed, envs, quiet);
    planned(&synced, now)
}

/// A dry run's answer, as the Card view counts it.
fn planned(synced: &sync::Synced, now: u64) -> Next {
    match &synced.end {
        sync::End::Halted(sync::Halt::Server(why)) if signed_out(why) => Next::SignIn,
        sync::End::Halted(sync::Halt::Server(why)) => {
            Next::Unreached { asked: now, why: why.clone() }
        }
        sync::End::Halted(halt) if synced.summary.is_none() => Next::Refused(halt.to_string()),
        _ => {
            let Some(s) = &synced.summary else { return Next::Unasked };
            let tally = synced.plays.clone();
            let plays = tally.as_ref().map_or(0, |t| t.owed.max(t.due));
            Next::Plan(Box::new(Planned {
                asked: now,
                new: s.fetches,
                bytes: s.fetch_bytes,
                conversions: s.conversions,
                removals: s.removals.clone(),
                plays,
                tally,
                fit: s.fit,
                guards: s.guards.clone(),
                unchanged: s.listed == sync::Listed::NotModified,
                offered: s.offered.clone(),
                totals: s.totals.clone(),
                ask_proxy: s
                    .peers
                    .iter()
                    .filter(|p| p.status == PeerStatus::AskProxy)
                    .map(|p| p.id)
                    .collect(),
            }))
        }
    }
}

/// Writes the sheet's settings to the card's `state.json` (clauses 47, 48).
fn store(
    card: &dyn CardWrite,
    home: &dyn Home,
    want: &Want,
    link: Option<&ServerLink>,
) -> Result<(), String> {
    let url_key = link.and_then(ServerLink::url_key);
    let name = want.name.trim().to_string();
    let mut change = |state: &mut State| {
        if !name.is_empty() && state::label_fault(&name).is_none() {
            state.label = name.clone();
        }
        let s = &mut state.settings;
        s.conversion.convert = want.convert;
        s.conversion.codec = want.codec;
        s.conversion.bitrate = want.bitrate;
        s.removal = want.removal;
        s.covers = want.covers;
        s.auto_dj = want.auto_dj;
        s.for_someone_else = want.for_someone_else;
        if let Some(key) = url_key {
            let source = s.source(key);
            if let Some(link) = link {
                source.nickname = link.name.clone();
            }
            if let Some(libraries) = &want.libraries {
                source.libraries = libraries.clone();
            }
            // The chosen server first: it is the one the card is filled from.
            if let Some(at) = s.sources.iter().position(|src| src.url_key == key) {
                let chosen = s.sources.remove(at);
                s.sources.insert(0, chosen);
            }
            for (id, libraries) in &want.peers {
                s.peer_source(&[key], *id).libraries = libraries.clone();
            }
        }
    };
    let mut envs = home.envs();
    setup::store(card, &mut change, &mut *envs, &mut |_| {}).map_err(|stop| stop.to_string())?;
    // The peers' daily budget lives on this computer.
    if let (Some(mb), Some(key)) = (want.budget_mb, url_key) {
        let budget = Budget::system()?;
        budget.with(|book| {
            for (id, _) in &want.peers {
                book.set_limit(PeerKey { parent_key: key, peer_id: *id }, Some(mb));
            }
        })?;
    }
    Ok(())
}

/// What a sync runs on: the card's volume, the computer, the card's
/// server, and what was read off the card.
#[derive(Clone, Copy)]
struct Card<'a> {
    found: &'a Found,
    volumes: &'a dyn Volumes,
    home: &'a dyn Home,
    link: &'a ServerLink,
    facts: &'a Facts,
}

/// One sync, on its own thread: its progress to the worker, its end back.
fn run_sync(
    on: &Card<'_>,
    allow: &Allow,
    stop: Arc<AtomicBool>,
    accepted: Vec<PathBuf>,
    me: &Sender<In>,
    key: &str,
) -> Finished {
    let Card { found, volumes, home, link, facts } = *on;
    let failed = |why: String| Finished {
        synced: None,
        why: Some(why),
        by_library: Vec::new(),
        removal_libs: Vec::new(),
        stopped: false,
    };
    let Some(card) = volumes.open(found) else {
        return failed("the volume can't be opened".into());
    };
    let client = match link.client() {
        Ok(client) => Arc::new(client),
        Err(why) => return failed(why),
    };
    let fed = match home.federation(accepted) {
        Ok(fed) => fed,
        Err(why) => return failed(why),
    };
    let peers = match allow.leave_out.is_empty() {
        true => None,
        false => {
            let names = facts.state.as_ref().map(|s| s.settings.peers.clone()).unwrap_or_default();
            let kept = names.iter().filter(|p| !allow.leave_out.contains(&p.peer_id));
            Some(kept.map(|p| sync::PeerAsk { id: p.peer_id, libraries: None }).collect())
        }
    };
    let ask = sync::Ask {
        answers: Answers {
            new_servers: allow.take_over || allow.answers.new_servers,
            ..allow.answers
        },
        change: sync::Change { nickname: Some(link.name.clone()), ..sync::Change::default() },
        proxy_copies: allow.proxy_copies.clone(),
        peers,
        stop: Some(stop.clone()),
        ..sync::Ask::default()
    };
    let mut cards = home.cards();
    let mut envs = home.envs();
    let space = found.volume.space;
    let mut say = |event: RunEvent| {
        let _ = me.send(In::RunEvent(key.to_string(), event));
    };
    let (plays, reached, envs) = (home.plays(), link.reached(), &mut *envs);
    let synced =
        sync::sync(&*card, space, client, reached, &ask, &mut cards, plays, &fed, envs, &mut say);
    if let sync::End::Ran(outcome) = &synced.end {
        cards.synced(outcome.card_id, &link.name, home.now());
    }
    let kept = synced.summary.as_ref().and_then(|s| s.set.as_ref()).is_some_and(|s| s.written);
    if matches!(synced.end, sync::End::Ran(_)) || kept {
        let _ = home.save_cards(&cards);
    }
    // The stronger gate counts by library: each removal's library, from the
    // card's ledger.
    let mut removal_libs = Vec::new();
    if let sync::End::Halted(sync::Halt::Card(Stop::Unanswered(guards))) = &synced.end
        && guards.iter().any(|g| matches!(g, Guard::Removals { strong: true, .. }))
        && let Ok(seen) = volume::inspect(&*card)
        && let Some(Ok(tags)) = seen.root.as_ref().map(|r| r.tags.as_ref())
    {
        for g in guards {
            if let Guard::Removals { paths, strong: true } = g {
                for path in paths {
                    let row = tags.records.iter().find(|e| e.path == *path);
                    let ledger = row.and_then(|e| e.ledger.as_ref());
                    let lib = ledger.map(|l| l.server_path.split('/').next().unwrap_or_default());
                    let lib = lib.map(str::to_string);
                    removal_libs.push(lib.unwrap_or_default());
                }
            }
        }
    }
    Finished {
        synced: Some(synced),
        why: None,
        by_library: facts.by_library.clone(),
        removal_libs,
        stopped: stop.load(Ordering::SeqCst),
    }
}

/// Cards as the worker would tell them, for the page's and the GUI's
/// tests: the design set's examples (invented names, sizes and counts).
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::device::card::state::Settings;
    use crate::device::card::volume::{Bus, Clock, Disk, Fs, Serial, Table, Volume};

    /// The design's card: 63.8 GB, 41.2 used.
    pub(crate) fn space() -> Space {
        Space { size: 63_800_000_000, free: 22_600_000_000, cluster: 32 * 1024, clusters: 0 }
    }

    /// A volume at `drive` (`E:`), FAT32 on MBR unless `fs` says, in a USB
    /// reader.
    pub(crate) fn found(drive: &str, fs: Fs, table: Table) -> Found {
        let volume = Volume {
            path: format!("{drive}\\").into(),
            device: format!("vol-{drive}"),
            fs,
            table,
            label: String::new(),
            serial: Some(Serial::Fat(0x1A2B_3C4D)),
            space: Some(space()),
            read_only: false,
            utf8: Some(true),
            clock: Clock::Bias,
        };
        let disk = Disk {
            id: format!("disk-{drive}"),
            removable: true,
            bus: Bus::Usb,
            read_only: false,
            system: Vec::new(),
            others: Vec::new(),
        };
        Found::new(volume, disk)
    }

    /// Home, the session's server, at its URL key.
    pub(crate) fn home() -> ServerLink {
        ServerLink {
            id: "http://127.0.0.1:3000".into(),
            name: "Home".into(),
            base: Ok("http://127.0.0.1:3000".into()),
            token: Some("tok".into()),
            session: true,
            ..ServerLink::default()
        }
    }

    /// A card being read.
    pub(crate) fn reading(drive: &str) -> CardView {
        CardView::new(found(drive, Fs::Fat32, Table::Mbr))
    }

    /// A volume the player can't use, listed with why.
    pub(crate) fn refused(drive: &str, fs: Fs, table: Table) -> CardView {
        CardView::new(found(drive, fs, table))
    }

    /// A FAT32 card with nothing from mStream on it.
    pub(crate) fn new_card(drive: &str) -> CardView {
        let mut view = reading(drive);
        let mut facts = Facts::default();
        facts.music.present = true;
        view.read = Read::Read(Box::new(facts));
        view.server = Server::New(Some(home()));
        view
    }

    /// The design's ready card: Car music, Rock and Jazz from Home, 4,812
    /// tracks, last synced 3 days before `now`, and a plan of 12 new, 2 to
    /// remove and 37 plays.
    pub(crate) fn ready(drive: &str, now: u64) -> CardView {
        let mut view = reading(drive);
        let link = home();
        let key = link.url_key().unwrap_or(1);
        let mut state = State::new(0x5EED_C0DE_0A1B_2C3D, "Car music", None, now - 30 * 86_400);
        state.settings = Settings::defaults();
        let source = state.settings.source(key);
        source.nickname = "Home".into();
        source.libraries = vec!["Rock".into(), "Jazz".into()];
        let facts = Facts {
            state: Some(state),
            root: Some((0x5EED_C0DE_0A1B_2C3D, 42, 7)),
            committed: Some(now - 3 * 86_400),
            tracks: 4_812,
            by_library: vec![("Rock".into(), 2_904), ("Jazz".into(), 1_908)],
            device: crate::device::card::contract::device_txt::DeviceTxt::read(Some(
                b"contract=1
firmware=0.12.0
read.mpdj=2
codecs=mp3,flac,opus
history=1
",
            )),
            server: Some(key),
            nickname: "Home".into(),
            producer: "mstream-terminal 0.14.0".into(),
            ..Facts::default()
        };
        view.read = Read::Read(Box::new(facts));
        view.server = Server::Saved(link);
        let removals = vec![
            "Velvet Static/Harbour Songs/04 - Low Tide.flac".into(),
            "Velvet Static/Harbour Songs/05 - Undertow.flac".into(),
        ];
        let fit = Fit { needed: 2_000_000_000, room: 22_600_000_000, tracks: 4_822 };
        view.next = Next::Plan(Box::new(Planned {
            asked: now,
            new: 12,
            bytes: 1_900_000_000,
            removals,
            plays: 37,
            fit: Some(fit),
            offered: vec!["Rock".into(), "Jazz".into(), "Audiobooks".into(), "Live".into()],
            totals: vec![("Rock".into(), 2_904, 27_900_000_000), ("Jazz".into(), 1_908, 12_600_000_000)],
            ..Planned::default()
        }));
        view
    }

    /// The ready card syncing: 1,204 of 4,812 files, 6.1 of 21.3 GB.
    pub(crate) fn syncing(drive: &str, now: u64) -> CardView {
        let mut view = ready(drive, now);
        view.run = Some(Run {
            phase: Phase::Copying,
            files: 4_812,
            bytes: 21_300_000_000,
            done_files: 1_204,
            done_bytes: 6_100_000_000,
            now: Some("Orla Finch/Paper Boats/03 - Fold.flac".into()),
            rate: Some(12_400_000.0),
            saved: Some(now - 180),
            stopping: false,
            skipped: 0,
            started: now - 600,
        });
        view
    }

    /// A card's view with `change` made to it.
    pub(crate) fn with(mut view: CardView, change: impl FnOnce(&mut CardView)) -> CardView {
        change(&mut view);
        view
    }

    /// Its facts, changed.
    pub(crate) fn facts(view: &mut CardView) -> &mut Facts {
        match &mut view.read {
            Read::Read(facts) => facts,
            _ => panic!("a card read"),
        }
    }
}

#[cfg(test)]
mod tests;
