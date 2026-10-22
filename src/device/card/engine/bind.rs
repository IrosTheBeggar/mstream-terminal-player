//! A run bound to one card, not to a drive path (the transfer plan's 2.3,
//! step 2; docs/METADATA.md 2.12.7).
//!
//! **The identity triple**: the volume serial, the card id and the commit
//! the run last wrote (or found). It is checked again before every batch of
//! deletes, every rename into `/music` and every commit, and after any
//! error: a card swapped in the reader stops the run, and nothing more is
//! written to it. Before every other write, the cheap half: the run.lock
//! handle still reaches the card.
//!
//! **The locks**: one inside this process (the GUI's transfer worker and a
//! command-line run share none), and the OS's advisory lock on the open
//! `/.mstream/run.lock` (`LockFileEx`, `flock`), so two programs on this
//! computer never write one card at once. The file itself is only a note
//! (which run, when; no host or user name) and is never trusted: a run
//! that finds one with no live lock behind it — a crash, a pulled card,
//! another computer — takes it over and says so. FAT has no lock across
//! computers; settling the root and the plan bring the card back from
//! whatever that run left.

use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::super::contract::msmf::{self, Manifest, Pick};
use super::super::volume::{CardFs, CardWrite, LockFile, Serial};
use super::Stop;

/// The cards a run of this process holds, by [`CardWrite::key`].
static HELD: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(super) const RUN_LOCK: &str = "/.mstream/run.lock";

/// How long a serial read stays good for the check before a rename into
/// `/music`. On a Mac it is a diskutil process; the run.lock handle and
/// the root, read every time, see a swapped card first anyway. Before a
/// delete batch, a commit and after an error it is always read again.
const SERIAL_FOR: Duration = Duration::from_secs(1);

/// The identity triple.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) serial: Option<Serial>,
    /// The root's card id; 0 before the card's first commit.
    pub(crate) card_id: u64,
    /// The root's commit id: the one the run found, then each it wrote.
    pub(crate) commit_id: u64,
}

/// What the caller says the card is, from when the listener picked it (or
/// a run cut short): each part given must match, or the session stops with
/// [`Stop::DifferentCard`] before it writes anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Expect {
    pub(crate) serial: Option<Serial>,
    pub(crate) card_id: Option<u64>,
    pub(crate) commit_id: Option<u64>,
}

impl Expect {
    pub(super) fn matches(&self, found: &Identity) -> Result<(), Stop> {
        let serial = self.serial.as_ref().is_none_or(|s| found.serial.as_ref() == Some(s));
        let card = self.card_id.is_none_or(|c| c == found.card_id);
        let commit = self.commit_id.is_none_or(|c| c == found.commit_id);
        if serial && card && commit { Ok(()) } else { Err(Stop::DifferentCard) }
    }
}

/// The card id and commit id of the root that counts (2.5.4): (0, 0) for a
/// card with none. An error reading either file is the card's (pulled).
pub(super) fn root_ids(card: &dyn CardFs) -> io::Result<(u64, u64)> {
    let valid = |name: &str| -> io::Result<Option<Manifest>> {
        let bytes = card.read(&format!("/.mstream/{name}"), 64 * 1024 * 1024)?;
        Ok(bytes.and_then(|b| Manifest::read(&b, msmf::Uses::ALL).ok()))
    };
    let (bin, tmp) = (valid("manifest.bin")?, valid("manifest.tmp")?);
    let candidate = |m: &Option<Manifest>| m.as_ref().map(Manifest::candidate);
    let election = msmf::elect(candidate(&bin), candidate(&tmp));
    let root = match election.pick {
        Pick::Bin => bin,
        Pick::Tmp => tmp,
        Pick::None => None,
    };
    Ok(root.map_or((0, 0), |m| (m.header.card_id, m.commit_id)))
}

/// The card as it is now, read only.
pub(super) fn look(card: &dyn CardWrite) -> Result<Identity, Stop> {
    let serial = card.serial().map_err(|_| Stop::Pulled)?;
    let (card_id, commit_id) = root_ids(card).map_err(|_| Stop::Pulled)?;
    Ok(Identity { serial, card_id, commit_id })
}

/// run.lock's note (2.12.7, vectors.json's privateFiles.run): which run,
/// since when, by what; never a host or user name.
pub(super) fn note(run_id: u64, started: u64, producer: &str) -> String {
    let producer = serde_json::to_string(producer).unwrap_or_default();
    format!(
        "{{\"format\": \"mstream-card-run\", \"runId\": \"0x{run_id:016X}\", \"started\": {started}, \
         \"producer\": {producer}}}\n"
    )
}

/// A card held by a run.
pub(super) struct Bound<'c> {
    card: &'c dyn CardWrite,
    key: String,
    lock: Option<Box<dyn LockFile + 'c>>,
    pub(super) id: Identity,
    serial_read: Instant,
    /// A check failed: the card in the reader isn't the bound one, and is
    /// never written again — run.lock included.
    lost: bool,
}

impl<'c> Bound<'c> {
    /// Takes the card: the lock inside this process, `/.mstream` made when
    /// it isn't there, then run.lock opened and its OS lock taken, and the
    /// note written into it. What a stale run.lock held, when one was there:
    /// this run takes it over.
    pub(super) fn take(
        card: &'c dyn CardWrite,
        id: Identity,
        note: &str,
    ) -> Result<(Bound<'c>, Option<String>), Stop> {
        let key = card.key();
        {
            let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
            if held.contains(&key) {
                return Err(Stop::Busy);
            }
            held.push(key.clone());
        }
        let mut bound = Bound { card, key, lock: None, id, serial_read: Instant::now(), lost: false };
        let made = match card.stat("/.mstream") {
            Ok(Some(item)) if item.dir => Ok(()),
            Ok(_) => card.mkdir("/.mstream"),
            Err(e) => Err(e),
        };
        made.map_err(|e| bound.failed(e))?;
        let mut lock = card.lock_file(RUN_LOCK).map_err(|e| bound.failed(e))?;
        if !lock.try_lock().map_err(|e| bound.failed(e))? {
            return Err(Stop::Busy);
        }
        let old = lock.read_all().map_err(|e| bound.failed(e))?;
        lock.replace(note.as_bytes()).map_err(|e| bound.failed(e))?;
        bound.lock = Some(lock);
        let stale = (!old.is_empty()).then(|| String::from_utf8_lossy(&old).into_owned());
        Ok((bound, stale))
    }

    /// The cheap check before a write: the run.lock handle still reaches
    /// the card.
    pub(super) fn alive(&mut self) -> Result<&'c dyn CardWrite, Stop> {
        if self.lost {
            return Err(Stop::DifferentCard);
        }
        match self.lock.as_mut().is_some_and(|l| l.alive()) {
            true => Ok(self.card),
            false => {
                self.lost = true;
                Err(Stop::Pulled)
            }
        }
    }

    /// The whole check: the handle; the root's card id and commit, the ones
    /// this run found or last wrote; and the serial (`serial`, or when the
    /// last read is a second old).
    pub(super) fn check(&mut self, serial: bool) -> Result<&'c dyn CardWrite, Stop> {
        self.alive()?;
        let ids = match root_ids(self.card) {
            Ok(ids) => ids,
            Err(_) => return Err(self.lose(Stop::Pulled)),
        };
        if ids != (self.id.card_id, self.id.commit_id) {
            return Err(self.lose(Stop::DifferentCard));
        }
        if serial || self.serial_read.elapsed() >= SERIAL_FOR {
            let now = match self.card.serial() {
                Ok(now) => now,
                Err(_) => return Err(self.lose(Stop::Pulled)),
            };
            self.serial_read = Instant::now();
            if now != self.id.serial {
                return Err(self.lose(Stop::DifferentCard));
            }
        }
        Ok(self.card)
    }

    fn lose(&mut self, stop: Stop) -> Stop {
        self.lost = true;
        stop
    }

    /// An error from the card: checked again, it is the card's own (full,
    /// read-only) while the card is still the bound one, and a pulled or
    /// different card's otherwise.
    pub(super) fn failed(&mut self, e: io::Error) -> Stop {
        if self.lock.is_none() {
            return Stop::Card(e);
        }
        match self.check(true) {
            Ok(_) => Stop::Card(e),
            Err(stop) => stop,
        }
    }

    /// A commit this run made: the root's ids from now on.
    pub(super) fn committed(&mut self, card_id: u64, commit_id: u64) {
        self.id.card_id = card_id;
        self.id.commit_id = commit_id;
    }

    /// Lets the card go: run.lock deleted while its handle still holds the
    /// lock (a delete of an open file is pending on FAT until the close),
    /// then closed. A card that is no longer the bound one is left alone.
    pub(super) fn release(mut self) {
        if self.alive().is_ok() {
            let _ = self.card.delete(RUN_LOCK);
        }
        self.lock = None;
    }
}

impl Drop for Bound<'_> {
    fn drop(&mut self) {
        // A run that stops without release leaves run.lock as a stale note
        // (its OS lock goes with the handle): the next run takes it over.
        self.lock = None;
        let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|k| k != &self.key);
    }
}
