//! The sync engine: every write the transfer makes to a card, in the order
//! the firmware repo's docs/METADATA.md 2.12 lays down, so a card pulled at
//! any moment is still one the player reads — the last commit, or the next.
//!
//! **In and out.** A run is given a card ([`CardWrite`], a volume the
//! volume layer accepted and the listener picked), a *wanted card*
//! ([`WantedCard`]: the files of a selection, each with its server's ledger
//! fields, what its bytes will be and their size) and the card's settings
//! ([`Knobs`]). It gives back a plan ([`Plan`]: what it will write, rename,
//! keep and delete, and the decisions it needs, [`Guard`]), then runs it,
//! saying what it does as it goes ([`Event`]) and what came of it
//! ([`Outcome`]). The bytes come through a [`Fetcher`]; what the server's
//! side of a sync adds — the plays of the card's log, a conversion's checks,
//! a converted file's cover and tags put back, the thumbnails, the Auto DJ
//! index — through [`Hooks`], each a no-op until its part fills it.
//!
//! **A run, step by step** (2.12.1; the transfer plan's 2.3):
//! - [`Session::open`]: 2.4.5's check (a newer major stops it before any
//!   write); the card bound — its identity triple and its locks
//!   ([`bind`]); steps 1-4 ([`settle`]): the root and the plan settled,
//!   the private files read, the plays hook, the cut-rename check (a stop
//!   for a disk check), the clean-up; then the card listed and every ledger
//!   row matched against it ([`survey`]).
//! - [`Session::plan`]: the server's side paired with the ledger, the plan
//!   table, the names, the guards ([`plan`]).
//! - [`Session::run`]: `pending.bin`, the deletes, each file staged,
//!   checked, renamed into `/music` and read back, checkpoints, thumbnails,
//!   the Auto DJ index, the final commit, the collection, and a last read of
//!   the card as the device reads it ([`run`]).
//!
//! The card is the reference for every name and stamp (2.8.5, 2.3.4): what
//! the engine records is what it reads back, never what it meant to write.

mod bind;
mod plan;
mod run;
mod settle;
mod survey;

#[cfg(test)]
mod fatsim;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::io::{self, Write};

pub(crate) use self::bind::Expect;
pub(crate) use self::plan::Plan;

use self::bind::{Bound, Identity};
use self::settle::Settled;
use self::survey::{Music, Survey};
use super::contract::device_txt::DeviceTxt;
use super::contract::mptg::{Ledger, RecordIn};
use super::contract::mpth::Pixels;
use super::contract::msmf::Ack;
use super::contract::server::{Server, Talk};
use super::names::{self, Bytes};
use super::volume::{CardFs, CardWrite, Private, Root};

/// The writer and its version, as every file it writes names it (2.5.1's
/// `producer`, run.lock's).
pub(crate) fn producer() -> String {
    format!("mstream-terminal {}", env!("CARGO_PKG_VERSION"))
}

// ── What a run is given ─────────────────────────────────────────────────────

/// A server the run reads: its own (server 0), another reached by its own
/// URL, or a federation peer (2.5.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Origin {
    /// What the software reads now, for 2.11's same-server test.
    pub(crate) talk: Talk,
    /// How the card names it: the root's header fields when it is server 0,
    /// else its SRVS entry.
    pub(crate) server: Server,
    /// Its sync manifest's `revision` (its ETag): the root's when it is
    /// server 0, written with the final commit only; another server's is the
    /// caller's to keep (2.12.7).
    pub(crate) revision: String,
    /// The manifest said `scanning: true`: no path missing from it is a
    /// deletion (2.10.2).
    pub(crate) scanning: bool,
    /// The audio hashes of the server's whole manifest, when the caller read
    /// it whole: most of a server's rows missing from it is a mass deletion
    /// (2.10.2, step 5).
    pub(crate) whole: Option<HashSet<[u8; 16]>>,
}

/// One file of the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WantedFile {
    /// Its server, an index into [`WantedCard::origins`].
    pub(crate) origin: usize,
    /// ORIG's fields as the server gives them (2.6.6): the server path, the
    /// ids, the hashes, `modified`, `file-size`, `created-at`, `hash-v`. The
    /// origin flags, the conversion and the server index are the engine's.
    pub(crate) ledger: Ledger,
    /// What goes on the card: the server's bytes, or a conversion.
    pub(crate) bytes: Bytes,
    /// A conversion's bitrate in kbit/s.
    pub(crate) kbps: u16,
    /// The bytes on the card: `file-size` for an original, an estimate for
    /// a conversion (the names' 4 GiB check, the plan's sizes).
    pub(crate) size: u64,
    /// mStream's analysis, recorded when the card file's own tags have
    /// neither a BPM nor a key (2.7: BPM_ANALYSED).
    pub(crate) bpm10: u16,
    pub(crate) camelot: u8,
}

/// A wanted file's bytes, unless it says otherwise: the server's own.
impl Default for Bytes {
    fn default() -> Bytes {
        Bytes::Original { ogg_opus: false }
    }
}

impl WantedFile {
    /// ORIG's `convertedTo`: 0 for the server's bytes.
    pub(crate) fn converted_to(&self) -> u8 {
        match self.bytes {
            Bytes::Converted(container) => container,
            _ => 0,
        }
    }

    /// The server's own bytes go on the card: the size and the MD5 are the
    /// server's.
    pub(crate) fn original(&self) -> bool {
        matches!(self.bytes, Bytes::Original { .. })
    }
}

/// What the card should hold of the servers this run reads. Where each
/// file goes is the engine's to say: a file already on the card stays
/// where the ledger has it (or where the listener moved it), and a new one
/// is named by 2.8.3's rules against the card as listed and the rows that
/// stay (the names layer), which only a session that has listed the card
/// can do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WantedCard {
    pub(crate) origins: Vec<Origin>,
    pub(crate) files: Vec<WantedFile>,
}

/// Q1: what happens to a file that left the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The card mirrors the selection: the file goes, after the list is
    /// shown and past the gates.
    #[default]
    Mirror,
    /// The card never loses a file: it stays, and stays the software's.
    Never,
}

/// A card's settings that shape a plan (the transfer plan's section 6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Knobs {
    pub(crate) removal: Removal,
    /// U4: a file the listener deleted is written again.
    pub(crate) rewrite_missing: bool,
    /// U4: a file the listener moved is followed where it went.
    pub(crate) follow_moves: bool,
    /// U5: a file the listener changed is replaced by the server's — the
    /// listener's explicit say-so (2.11). Off: kept, left out of the ledger.
    pub(crate) replace_edits: bool,
    /// U2: the vpath layout (`/music/<vpath>/…`, 2.8.6). None: as the card
    /// has it, or for a card with no ledger yet, when the selection holds
    /// more than one library.
    pub(crate) vpath: Option<bool>,
    /// Q1's gate: more removals than this ask first.
    pub(crate) gate: usize,
    /// Checkpoints (2.12.1, step 9): after this many files, or this many
    /// seconds.
    pub(crate) checkpoint_files: usize,
    pub(crate) checkpoint_secs: u64,
    /// The final check reads every owned file's fingerprint too.
    pub(crate) deep_check: bool,
}

impl Default for Knobs {
    fn default() -> Knobs {
        Knobs {
            removal: Removal::Mirror,
            rewrite_missing: true,
            follow_moves: true,
            replace_edits: false,
            vpath: None,
            gate: 50,
            checkpoint_files: 500,
            checkpoint_secs: 300,
            deep_check: false,
        }
    }
}

/// The listener's answers to the guards of a plan (2.8's table).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Answers {
    /// The OS's disk check has run since a cut rename was found.
    pub(crate) disk_checked: bool,
    /// More removals than the gate: go ahead.
    pub(crate) removals: bool,
    /// Most of the ledger, or of one server's files, removed: go ahead.
    pub(crate) mass_removals: bool,
    /// U6: take the listener's files that equal the selection's into the
    /// ledger. None: not asked yet.
    pub(crate) adopt: Option<bool>,
    /// U7: a server that may be one the card's files came from under
    /// another address is a server of its own.
    pub(crate) new_servers: bool,
}

/// A decision a plan needs before its run, or a refusal (the transfer
/// plan's 2.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Guard {
    /// 2.4.5: a contract file of a newer major, or a private file of a
    /// version this software doesn't know: it writes nothing (no answer).
    Newer(Vec<String>),
    /// 2.12.1, step 3: a rename may have been cut between its two directory
    /// writes, leaving two names on one cluster chain (the pairs). The run
    /// stops before it deletes or writes anything; the OS's disk check, then
    /// [`Answers::disk_checked`].
    DiskCheck(Vec<(String, String)>),
    /// U6: the listener's files that equal files of the selection, by path,
    /// size and fingerprint: [`Answers::adopt`].
    Adopt(Vec<String>),
    /// Q1: the files that left the selection, more than the gate allows
    /// ([`Answers::removals`]), or `strong`: more than half the ledger, or
    /// most of a server's files gone from its whole manifest
    /// ([`Answers::mass_removals`]).
    Removals { paths: Vec<String>, strong: bool },
    /// U7: a server none of the card's servers is known to be, while files
    /// on the card came from a server that can't be told apart from it:
    /// [`Answers::new_servers`], or the caller confirms it as that server
    /// (2.11's key rule) and plans again.
    Server { origin: usize, rows: usize },
}

/// Why a session stops.
#[derive(Debug)]
pub(crate) enum Stop {
    /// Another run holds this card: in this process, or another program on
    /// this computer (run.lock's OS lock).
    Busy,
    /// The card in the reader isn't the one named or bound: its serial, its
    /// card id or its commit differs. Nothing more is written to it.
    DifferentCard,
    /// The card stopped answering (pulled). Nothing more is written.
    Pulled,
    /// A guard that holds before anything is written: a newer format, or a
    /// cut rename.
    Guard(Guard),
    /// The plan's decisions still to answer.
    Unanswered(Vec<Guard>),
    /// A plan made in another session.
    StalePlan,
    /// What the plan or the run won't do: a layout the device can't keep, a
    /// path the run doesn't own, a file a writer refuses. The card is left
    /// as it is for the next run to recover.
    Refused(String),
    /// A write the card refused (full, read-only, damaged), the card still
    /// the bound one: what the run did so far stays for the next to recover.
    Card(io::Error),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Busy => f.write_str("another run holds this card"),
            Stop::DifferentCard => f.write_str("this is a different card"),
            Stop::Pulled => f.write_str("the card stopped answering"),
            Stop::Guard(g) => write!(f, "a guard holds: {g:?}"),
            Stop::Unanswered(g) => write!(f, "{} decisions to answer", g.len()),
            Stop::StalePlan => f.write_str("a plan of another session"),
            Stop::Refused(why) => write!(f, "refused: {why}"),
            Stop::Card(e) => write!(f, "the card refused a write: {e}"),
        }
    }
}

// ── What a run says ─────────────────────────────────────────────────────────

/// What a run does, as it does it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// A run.lock with no live lock behind it was taken over (2.12.7): what
    /// it held, informational.
    TookOver(String),
    /// A file left the card.
    Deleted(String),
    /// Bytes of a file arriving, so far.
    Progress { path: String, bytes: u64 },
    /// A file is in place, read back: its path as stored, its size.
    Wrote { path: String, bytes: u64 },
    /// A file moved on the server, moved on the card.
    Renamed { from: String, to: String },
    /// A file failed and was skipped: its server path and why.
    Failed { server_path: String, why: String },
    /// A commit: its generation, and whether it ended the run.
    Committed { generation: u32, last: bool },
}

/// What a plan or a run reports beside its work (2.8.4, 2.10.2, U4-U5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// U5: files the listener changed, kept as they are and left out of the
    /// ledger.
    pub(crate) edited: Vec<String>,
    /// U4: ledger files no longer on the card, left out of the ledger (the
    /// listener deleted them, and the run doesn't write them again).
    pub(crate) gone: Vec<String>,
    /// Ledger files whose server can't be told (an SRVS entry that breaks
    /// 2.5.5's rules): the listener's from now on.
    pub(crate) unknown: Vec<String>,
    /// Files that left the selection and stay ([`Removal::Never`]).
    pub(crate) kept: Vec<String>,
    /// Files the names refused (2.8.3, 2.8.4).
    pub(crate) refused: Vec<names::Skipped>,
    /// Folders past about 2,000 entries (2.8.4).
    pub(crate) big_folders: Vec<(String, usize)>,
    /// Files that failed in the run: the server path and why.
    pub(crate) failed: Vec<(String, String)>,
    /// Deletes not made: the file changed between the listing and its
    /// delete.
    pub(crate) spared: Vec<String>,
}

/// What a run came to.
#[derive(Debug)]
pub(crate) struct Outcome {
    /// The root it left: its generation, card id and commit id.
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) commit_id: u64,
    /// Files the final ledger lists.
    pub(crate) records: usize,
    pub(crate) written: usize,
    pub(crate) renamed: usize,
    pub(crate) deleted: usize,
    /// The source stopped the run (a quota, lost sign-in): why. Its plan
    /// stays on the card for the next run to take up (2.12.8).
    pub(crate) parked: Option<String>,
    pub(crate) report: Report,
    /// The card read back as the device reads it (2.3, step 16).
    pub(crate) verified: Result<(), String>,
}

// ── Where the bytes come from, and what other parts add ─────────────────────

/// What a fetch said of the bytes it wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fetched {
    /// The answer's Content-Type: a conversion's is checked (B1's hook).
    pub(crate) content_type: Option<String>,
}

/// A fetch that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failed {
    pub(crate) why: String,
    /// The whole run must stop, not only this file: a quota, lost
    /// sign-in, a server gone. The run parks at a checkpoint.
    pub(crate) stop: bool,
}

/// The source of a file's bytes: the server over HTTP, the local disk, a
/// peer, a local ffmpeg (the transfer plan's 2.4, 2.5).
pub(crate) trait Fetcher {
    /// The file's bytes, written into `into` from their first byte.
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed>;
    /// `len` bytes from `start` of the source's file (fewer at its end):
    /// the server's side of a fingerprint, for U6's adoption.
    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed>;
}

/// An album folder as the thumbnail step sees it (2.14).
pub(crate) struct Album<'a> {
    /// Relative to `/music` (2.14.1's album folder).
    pub(crate) folder: &'a str,
    /// Its files: each card path and ledger row.
    pub(crate) files: Vec<(&'a str, &'a Ledger)>,
    /// Its thumbnail is on the card and reads as its own (2.12.1, step 10).
    pub(crate) on_card: bool,
}

/// What becomes of an album's thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cover {
    /// The one on the card, if it is there and reads as the album's.
    Keep,
    /// A new thumbnail from these pixels (`source_bytes`: the image's size).
    New { pixels: Pixels, source_bytes: u32 },
    /// No thumbnail: the device's own cover order (2.14.3).
    Gone,
}

/// What the final commit's Auto DJ step is given (2.13, AUTODJ-INDEX.md 7).
pub(crate) struct Commit<'a> {
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    /// Unix seconds now: a new file's builtTime.
    pub(crate) now: u32,
    /// The commit's records, each with its ledger row.
    pub(crate) records: &'a [RecordIn],
    pub(crate) device: &'a DeviceTxt,
    /// The index the current root names, when it is valid.
    pub(crate) old: Option<&'a [u8]>,
}

/// The final commit's Auto DJ index (AUTODJ-INDEX.md 7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AutoDj {
    /// The new root names none.
    None,
    /// The new root names the old file again.
    Same,
    /// A new file, `autodj-<generation>.bin` of this commit.
    New(Vec<u8>),
}

/// What other parts add to a run, each a no-op here ([`NoHooks`]): B15's
/// plays, B1's conversion checks, retags, covers and Auto DJ index.
pub(crate) trait Hooks {
    /// Step 2: the new plays of the card's log, mapped through the tags file
    /// of the root that counts before a checkpoint deletes it (2.12.1,
    /// step 5; 2.20.6).
    fn plays(&mut self, _card: &dyn CardFs, _root: Option<&Root>) {}
    /// Step 8: a conversion's bytes are the ones asked for (the type, the
    /// length within 1 s of the manifest's). Err: the file is skipped.
    fn conversion(
        &mut self,
        _file: &WantedFile,
        _fetched: &Fetched,
        _card: &dyn CardFs,
        _stage: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Step 9, before a file's record is read: what a conversion dropped
    /// (the cover, the source's own BPM and key) written back into the
    /// staged file. Err: the file is skipped.
    fn before_record(
        &mut self,
        _file: &WantedFile,
        _card: &dyn CardWrite,
        _stage: &str,
    ) -> Result<(), String> {
        Ok(())
    }
    /// Step 10: an album folder's thumbnail.
    fn cover(&mut self, _album: &Album<'_>) -> Cover {
        Cover::Keep
    }
    /// Step 11: the final commit's Auto DJ index.
    fn autodj(&mut self, _commit: &Commit<'_>) -> AutoDj {
        AutoDj::None
    }
    /// The plays a commit acknowledges (2.5.6), from the root's before:
    /// each raised to the software's mark, never lowered.
    fn acks(&mut self, carried: &[Ack]) -> Vec<Ack> {
        carried.to_vec()
    }
}

/// No part added yet.
pub(crate) struct NoHooks;

impl Hooks for NoHooks {}

/// The run's clock and ids: the system's, or fixed ones in the tests.
pub(crate) struct Env {
    /// Unix seconds now: commitTime, run.lock's start.
    pub(crate) now: Box<dyn FnMut() -> u64 + Send>,
    /// A random u64, never 0: commitIds, the runId, a new cardId.
    pub(crate) id: Box<dyn FnMut() -> u64 + Send>,
    /// The writer and its version ([`producer`]).
    pub(crate) producer: String,
}

impl Env {
    pub(crate) fn system() -> Env {
        let since = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH);
        let now = move || since(std::time::SystemTime::now()).map_or(0, |d| d.as_secs());
        Env { now: Box::new(now), id: Box::new(|| fastrand::u64(..)), producer: producer() }
    }

    fn id(&mut self) -> u64 {
        loop {
            let id = (self.id)();
            if id != 0 {
                return id;
            }
        }
    }

    fn now(&mut self) -> u64 {
        (self.now)()
    }
}

// ── The session ─────────────────────────────────────────────────────────────

/// One card, bound, settled and listed: plans are made against it, and one
/// of them runs. Dropping it lets the card go without deleting run.lock
/// (the next run takes it over); [`Session::close`] deletes it.
pub(crate) struct Session<'c> {
    card: &'c dyn CardWrite,
    bound: Bound<'c>,
    env: Env,
    knobs: Knobs,
    /// Ties a plan to this session's listing.
    token: u64,
    run_id: u64,
    settled: Settled,
    device: DeviceTxt,
    music: Music,
    survey: Survey,
}

impl<'c> Session<'c> {
    /// Opens a card for a run: 2.4.5's check, the card bound to the run
    /// ([`Expect`]: what the caller says the card is), steps 1-4 of 2.12.1,
    /// and the card listed and matched. A stop leaves the card as steps
    /// 1-4 left it.
    pub(crate) fn open(
        card: &'c dyn CardWrite,
        expect: &Expect,
        knobs: Knobs,
        answers: &Answers,
        mut env: Env,
        hooks: &mut dyn Hooks,
        events: &mut dyn FnMut(Event),
    ) -> Result<Session<'c>, Stop> {
        let newer = settle::newer(card).map_err(|_| Stop::Pulled)?;
        if !newer.is_empty() {
            return Err(Stop::Guard(Guard::Newer(newer)));
        }
        let found = bind::look(card)?;
        expect.matches(&found)?;
        let (token, run_id) = (env.id(), env.id());
        let note = bind::note(run_id, env.now(), &env.producer);
        let (mut bound, stale) = Bound::take(card, found, &note)?;
        if let Some(stale) = stale {
            events(Event::TookOver(stale));
        }
        let mut look = || -> Result<_, Stop> {
            let settled = settle::settle(card, &mut bound, answers, hooks)?;
            let music = Music::list(card).map_err(|e| bound.failed(e))?;
            let survey = survey::survey(card, &music, &settled, &knobs).map_err(|e| bound.failed(e))?;
            Ok((settled, music, survey))
        };
        let (settled, music, survey) = match look() {
            Ok(looked) => looked,
            // A card still the bound one is let go; a pulled or different
            // one is never written again, run.lock included.
            Err(stop @ (Stop::Pulled | Stop::DifferentCard)) => return Err(stop),
            Err(stop) => {
                bound.release();
                return Err(stop);
            }
        };
        let device = settle::device_txt(card);
        Ok(Session { card, bound, env, knobs, token, run_id, settled, device, music, survey })
    }

    /// The card's settings and state (`state.json`, 2.12.7), as step 2
    /// settled it.
    pub(crate) fn state(&self) -> &Private {
        &self.settled.state
    }

    /// The card's `/.player/device.txt`, or its no-file values (2.15).
    pub(crate) fn device(&self) -> &DeviceTxt {
        &self.device
    }

    /// The identity the run is bound to.
    pub(crate) fn identity(&self) -> (Option<super::volume::Serial>, u64, u64) {
        let Identity { serial, card_id, commit_id } = self.bound.id.clone();
        (serial, card_id, commit_id)
    }

    /// A plan for this card: the selection paired with the ledger, the
    /// table's actions, the names, and the decisions it needs. Plan again
    /// with new answers as often as needed.
    pub(crate) fn plan(
        &mut self,
        wanted: &WantedCard,
        answers: &Answers,
        fetcher: &mut dyn Fetcher,
    ) -> Result<Plan, Stop> {
        plan::plan(self, wanted, answers, fetcher)
    }

    /// Lets the card go without a run: run.lock closed and deleted.
    pub(crate) fn close(self) {
        self.bound.release();
    }
}
