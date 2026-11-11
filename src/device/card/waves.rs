//! The waveform index's writer: `/.mstream/waves-<gen>.bin`, MPWF v1 (the
//! firmware repo's docs/WAVEFORM-INDEX.md, whose sections the numbers below
//! are; docs/METADATA.md 2.21, "M", ties it to the rest of the card), from
//! mStream's peaks of the music on the card. The file itself, the bars from
//! the peaks and 7.3's table are the kit's ([`mpwf`]); this is what a final
//! commit makes of them.
//!
//! **A row** (7.3) is a card file of the commit's tags file whose bars the
//! writer has: from peaks this run fetched, or from an old slot that still
//! holds. A path hash two card files share has none (4.3). A row takes the new
//! path's hash and lengthMs from the card file's own record, so a converted
//! file has its source's shape under its own length. Bars are kept by
//! recording, the canonical hash (`audio-hash`, else `hash`: M 2.6.6), which
//! is the waveform route's own key. A track with no bars has no row, and a
//! plain line on the device: in v1 that is a peer's file, which no route
//! answers for (unless a file the server has is the same recording, whose bars
//! it then has); a hand-copied or edited one is in no tags file's ledger at all.
//!
//! **The old file is the only memory of the peaks.** It has no content hash,
//! so a slot is joined through the tags ledger of the root the run began from:
//! its old path says which recording it was ([`Old`]). A moved file keeps its
//! bars under its new path hash. A checkpoint's ledger already shows a
//! recording replaced before it, while the file it names again still holds the
//! old bars: under such a root the slot's length must be the record's too.
//!
//! [`asks`] is what a run still asks the server for: each recording no old slot
//! holds, by its server paths. [`index`] is a run's whole step: 7.3's table,
//! its first line that holds. **A fetch that stops early has no line of its
//! own:** it decides only which rows there are, and the old file stays named
//! while its bytes are still the rows', and is replaced, never just dropped,
//! while any of its rows hold. This differs from the Auto DJ index's no
//! vectors, no index: only with no row left is there no file. The card id is
//! this writer's addition to the table, as it is the Auto DJ index's: every
//! file carries its card's (M 2.5.2), so a copied card given a new one gets a
//! file stamped with it, its bars asked for nowhere again.

use std::collections::HashMap;

use super::contract::WriteError;
use super::contract::device_txt::DeviceTxt;
use super::contract::mptg::{self, Ledger, RecordIn, Tags};
use super::contract::mpwf::{self, BAR_COUNT, Outcome, Slot, Table, TableIn};
use super::contract::prim::path_hash;

/// The bars a run's fetch brought ([`mpwf::bars_from`] of the peaks), by the
/// canonical hash of their recording.
pub(crate) type Shapes = HashMap<[u8; 16], [u8; BAR_COUNT]>;

// ── The old file (7.3) ──────────────────────────────────────────────────────

/// The file the current root names, as a writer reuses it: each slot whose CRC
/// passes, by what the ledger of the root the run began from says of its path.
pub(crate) struct Old<'a> {
    table: Table<'a>,
    /// By canonical hash, the rows of its slots: two copies of a recording have
    /// two.
    rows: HashMap<[u8; 16], Vec<u32>>,
    /// That root's FINAL is clear: a slot also needs its record's length.
    checkpoint: bool,
}

impl<'a> Old<'a> {
    /// `table` the file, valid (7.2); `tags` the tags file of the root the run
    /// began from, when it has one (none: no slot holds); `checkpoint` that
    /// root's FINAL clear.
    pub(crate) fn new(table: Table<'a>, tags: Option<&Tags>, checkpoint: bool) -> Old<'a> {
        let records = tags.map_or(&[][..], |t| t.records.as_slice());
        let hash = |e: &mptg::Entry| e.ledger.as_ref().and_then(Ledger::canonical_hash);
        Old::joined(table, records.iter().map(|e| (e.hash, hash(e))), checkpoint)
    }

    /// `ledger`: each record's path hash and canonical hash. A path hash two
    /// records share says nothing of its path.
    fn joined(
        table: Table<'a>,
        ledger: impl Iterator<Item = (u64, Option<[u8; 16]>)>,
        checkpoint: bool,
    ) -> Old<'a> {
        let mut by_path: HashMap<u64, Option<[u8; 16]>> = HashMap::new();
        for (path, hash) in ledger {
            by_path.entry(path).and_modify(|h| *h = None).or_insert(hash);
        }
        let mut rows: HashMap<[u8; 16], Vec<u32>> = HashMap::new();
        for r in 0..table.rows {
            let Ok(slot) = table.slot(r) else { continue };
            if let Some(Some(hash)) = by_path.get(&slot.path_hash) {
                rows.entry(*hash).or_default().push(r);
            }
        }
        Old { table, rows, checkpoint }
    }

    pub(crate) fn table(&self) -> &Table<'a> {
        &self.table
    }

    /// The bars of a slot that still holds for a record of this canonical hash
    /// and length.
    fn bars(&self, hash: &[u8; 16], length_ms: u32) -> Option<[u8; BAR_COUNT]> {
        let slots = self.rows.get(hash)?.iter().filter_map(|&r| self.table.slot(r).ok());
        slots.filter(|s| !self.checkpoint || s.length_ms == length_ms).map(|s| s.bars).next()
    }
}

// ── The rows ────────────────────────────────────────────────────────────────

/// A record as the rows read it.
struct Track<'r> {
    path_hash: u64,
    length_ms: u32,
    ledger: Option<&'r Ledger>,
}

fn tracks(records: &[RecordIn]) -> Vec<Track<'_>> {
    records
        .iter()
        .map(|r| Track {
            path_hash: path_hash(&r.path),
            length_ms: r.rec.duration_ms,
            ledger: r.ledger.as_ref(),
        })
        .collect()
}

/// The tracks that may have a row: a path hash two or more of them share has
/// none, whichever of them have bars (4.3), so a lookup never finds another
/// file's.
fn alone<'t, 'r>(tracks: &'t [Track<'r>]) -> impl Iterator<Item = &'t Track<'r>> {
    let mut count: HashMap<u64, u32> = HashMap::new();
    for t in tracks {
        *count.entry(t.path_hash).or_default() += 1;
    }
    tracks.iter().filter(move |t| count[&t.path_hash] == 1)
}

/// The rows the writer would write now (7.3), in the records' order: `fetched`
/// is what this run's fetch brought, `old` the file the current root names.
pub(crate) fn rows(records: &[RecordIn], old: Option<&Old<'_>>, fetched: &Shapes) -> Vec<Slot> {
    rows_of(&tracks(records), old, fetched)
}

fn rows_of(tracks: &[Track<'_>], old: Option<&Old<'_>>, fetched: &Shapes) -> Vec<Slot> {
    let bars = |t: &Track<'_>| {
        let hash = t.ledger?.canonical_hash()?;
        fetched.get(&hash).copied().or_else(|| old?.bars(&hash, t.length_ms))
    };
    let row = |t: &Track<'_>| {
        Some(Slot { path_hash: t.path_hash, length_ms: t.length_ms, bars: bars(t)? })
    };
    alone(tracks).filter_map(row).collect()
}

// ── What a run asks ─────────────────────────────────────────────────────────

/// A recording a run asks the server's waveform route for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ask {
    /// Its canonical hash: what the answer's bars are kept by ([`Shapes`]).
    pub(crate) hash: [u8; 16],
    /// The route's `filepath`s: its card files' server paths, in the records'
    /// order, the rest asked only when one before has nothing for it. A
    /// converted file's is its source's: the route answers with the source's
    /// peaks, of the same length within a second.
    pub(crate) server_paths: Vec<String>,
    /// Its server, in the records' numbering (the ledger's `server`).
    pub(crate) server: u16,
}

/// What a run still asks for: each recording among the tracks that may have a
/// row, with a server path, on a server `client` says the run can ask, that no
/// old slot holds for. Once, however many card files it has, with the server
/// path of each.
pub(crate) fn asks(
    records: &[RecordIn],
    old: Option<&Old<'_>>,
    client: impl Fn(u16) -> bool,
) -> Vec<Ask> {
    asks_of(&tracks(records), old, client)
}

fn asks_of(tracks: &[Track<'_>], old: Option<&Old<'_>>, client: impl Fn(u16) -> bool) -> Vec<Ask> {
    let mut at: HashMap<[u8; 16], usize> = HashMap::new();
    let mut out: Vec<Ask> = Vec::new();
    for t in alone(tracks) {
        let asked = |l: &&Ledger| !l.server_path.is_empty() && client(l.server);
        let Some(l) = t.ledger.filter(asked) else { continue };
        let Some(hash) = l.canonical_hash() else { continue };
        if old.is_some_and(|o| o.bars(&hash, t.length_ms).is_some()) {
            continue;
        }
        let Some(&k) = at.get(&hash) else {
            at.insert(hash, out.len());
            out.push(Ask { hash, server_paths: vec![l.server_path.clone()], server: l.server });
            continue;
        };
        if !out[k].server_paths.contains(&l.server_path) {
            out[k].server_paths.push(l.server_path.clone());
        }
    }
    out
}

// ── The file on the card (7.3, 7.4) ─────────────────────────────────────────

/// What the writer is given.
pub(crate) struct IndexIn<'a> {
    /// The commit's (7.2): the file is named after it.
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    /// The commit's records, each with its ledger row.
    pub(crate) records: &'a [RecordIn],
    /// The file the current root names, when it is valid.
    pub(crate) old: Option<&'a Old<'a>>,
    pub(crate) fetched: &'a Shapes,
}

/// Why the new root names no file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoIndex {
    /// The card's device.txt doesn't list `read.mpwf=1`, or it has none (7.4).
    NotRead,
    /// No row to write: there is no file of 0 rows (4.2).
    NoRows,
}

/// What a run does about its file (7.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Index {
    /// The new root names none: the clean-up takes any old file.
    None(NoIndex),
    /// The new root names the file the current root names, again; its
    /// generation stays older than the root's. No commit for this alone.
    Same,
    /// A new file: written under `waves-<gen>.bin` before the root that names
    /// it (7.2).
    New(Vec<u8>),
}

/// A run's step for the file: `device` the card's device.txt as read. Refused:
/// more rows than a file holds, which no card reaches ([`mpwf::write`]).
pub(crate) fn index(device: &DeviceTxt, input: &IndexIn<'_>) -> Result<Index, WriteError> {
    let rows = rows(input.records, input.old, input.fetched);
    answer(device, input.generation, input.card_id, input.old, rows)
}

/// 7.3's table for these rows, the bytes compared and never a CRC.
fn answer(
    device: &DeviceTxt,
    generation: u32,
    card_id: u64,
    old: Option<&Old<'_>>,
    rows: Vec<Slot>,
) -> Result<Index, WriteError> {
    let writes = device.writes_mpwf();
    let old = old.map(Old::table).filter(|t| t.header().card_id == card_id);
    Ok(match mpwf::outcome(writes, old, &rows) {
        Outcome::None if writes => Index::None(NoIndex::NoRows),
        Outcome::None => Index::None(NoIndex::NotRead),
        Outcome::Same => Index::Same,
        Outcome::New => Index::New(mpwf::write(&TableIn { generation, card_id, minor: 0, rows })?),
    })
}

#[cfg(test)]
mod tests;
