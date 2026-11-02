//! The waveform index, `waves-<gen>.bin` (MPWF v1): read, checked and written
//! (docs/WAVEFORM-INDEX.md, docs/METADATA.md 2.21), with the rules that pin
//! it: the bars (5), a slot's CRC (4.3), the lookup (6), when to write a new
//! file (7.3). Which card files have a row, and whether an old slot still
//! holds for one, are the waves writer's, on these.
//!
//! For each card file the writer has the server's peaks for: its length and
//! its shape, 240 bars of 0-255, in a 256-byte slot seeded with its row's CRC.
//! One section, WVSL, at byte 256, so the file is exactly 256 + 256 × rows
//! bytes and no slot crosses a 512-byte sector. The slots are sorted by path
//! hash with no table beside them: the device finds a row by an interpolation
//! search of the slots themselves, each read alone and checked alone
//! (`lib/core/CardWaves`). No run looks a row up: [`Table::find`] is that
//! search, here so the vectors hold the two alike.
//!
//! The file keeps no memory of what was asked and no content hash: a track
//! with no data has no slot, and everything in it can be derived again. The
//! bars are linear: a stretch is the reader's, who knows its columns.

use super::container::{self, Container, FileMeta, FormatSpec, Header, Section, SectionOut, SectionSpec};
use super::container::{MPWF, REQUIRED};
use super::prim::{crc32, crc32_after, fourcc, u32_at, u64_at};
use super::{WriteError, Why};

pub(crate) const WVSL: u32 = fourcc(b"WVSL");

pub(crate) const MAJOR: u16 = 1;
/// Minor 0's header (4.1), zeros past its first 48 bytes: its one directory
/// entry ends at byte 256, where the slots start.
pub(crate) const HEADER_BYTES: u32 = 224;
pub(crate) const FIRST_SLOT: u32 = 256;
pub(crate) const SLOT_BYTES: u32 = 256;
pub(crate) const BAR_COUNT: usize = 240;
/// A slot's bars, after its hash and its length; its CRC, over the row and
/// the bytes before it.
const BARS_AT: usize = 12;
const CRC_AT: usize = 252;
/// 64,000,256 bytes, under the transfer's 64 MiB file cap ([`super::MAX_FILE`]).
pub(crate) const MAX_ROWS: u32 = 250_000;
/// What [`open_head`] reads of a file: past its directory's 256 bytes, so one
/// whose directory runs on, as no valid file's does, reads its own reason.
pub(crate) const HEAD_BYTES: usize = 4096;
/// A lookup's first probes interpolate; the later ones bisect.
const INTERPOLATED: u32 = 4;
/// A guard: a lookup takes 22 reads at the most (6).
const MAX_PROBES: u32 = 24;

const SEC_WVSL: usize = 0;

const SPEC: FormatSpec = FormatSpec {
    magic: MPWF,
    major: MAJOR,
    header_bytes: HEADER_BYTES,
    sections: &[SectionSpec { kind: WVSL, stride: SLOT_BYTES, required: true }],
};

/// A file's exact length (10).
pub(crate) fn file_bytes_for(rows: u32) -> u32 {
    FIRST_SLOT + SLOT_BYTES * rows
}

/// A slot's CRC (4.3): CRC-32 over the row as a little-endian u32, then the
/// slot's bytes before the CRC, so a slot read for the wrong row fails.
pub(crate) fn slot_crc(slot: &[u8], row: u32) -> u32 {
    crc32_after(crc32(&row.to_le_bytes()), &slot[..CRC_AT])
}

/// Bar i's peaks of M (5): `[start, end)`, the floors integer divisions.
pub(crate) fn bar_range(i: usize, m: usize) -> (usize, usize) {
    let at = |i: usize| (i as u64 * m as u64 / BAR_COUNT as u64) as usize;
    let start = at(i);
    (start, m.min((start + 1).max(at(i + 1))))
}

/// The 240 bars of the server's M peaks (5): each the root mean square of its
/// range in f64, rounded half up by `floor(x + 0.5)`, not by a language's
/// `round()`, which may go to even: `sqrt(25 / 4)` reads 3. M 0 gives zeros.
pub(crate) fn bars_from(peaks: &[u8]) -> [u8; BAR_COUNT] {
    let mut bars = [0; BAR_COUNT];
    if peaks.is_empty() {
        return bars;
    }
    for (i, bar) in bars.iter_mut().enumerate() {
        let (start, end) = bar_range(i, peaks.len());
        let n: u64 = peaks[start..end].iter().map(|&b| b as u64 * b as u64).sum();
        // n and the width are exact; a tie is exact too, and anything else is
        // far enough from one for any f64 to agree.
        *bar = ((n as f64 / (end - start) as f64).sqrt() + 0.5).floor() as u8;
    }
    bars
}

/// A slot's fields (4.3): one read from a file, or a row to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    /// The card file's path hash (2.3.3). Hash 0 and 2^64 − 1 are valid.
    pub(crate) path_hash: u64,
    /// Its MPTG record's durationMs; 0 unknown.
    pub(crate) length_ms: u32,
    pub(crate) bars: [u8; BAR_COUNT],
}

impl Slot {
    /// Its bytes at `row`, the CRC computed for that place: one row added
    /// moves all those after it.
    pub(crate) fn encode(&self, row: u32) -> [u8; SLOT_BYTES as usize] {
        let mut out = [0; SLOT_BYTES as usize];
        out[..8].copy_from_slice(&self.path_hash.to_le_bytes());
        out[8..BARS_AT].copy_from_slice(&self.length_ms.to_le_bytes());
        out[BARS_AT..CRC_AT].copy_from_slice(&self.bars);
        let crc = slot_crc(&out, row);
        out[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// A slot's fields from its 256 bytes, its CRC unchecked.
    pub(crate) fn decode(raw: &[u8]) -> Slot {
        let mut bars = [0; BAR_COUNT];
        bars.copy_from_slice(&raw[BARS_AT..CRC_AT]);
        Slot { path_hash: u64_at(raw, 0), length_ms: u32_at(raw, 8), bars }
    }
}

/// The next row a lookup reads (6): rounded to the nearest by the top 32 bits
/// of (h − loH) / (hiH − loH) over lo..hi, while `probes` (those already made)
/// is below 4 and that span is 2^32 or more; else the middle, rounded down.
/// lo ≤ hi and loH ≤ h ≤ hiH; the answer is in lo..hi. No 128-bit integer:
/// hi − lo is below 2^18 and the top 32 bits below 2^32, so the sum stays
/// below 2^51.
pub(crate) fn pick(h: u64, lo: u32, hi: u32, lo_h: u64, hi_h: u64, probes: u32) -> u32 {
    let span = (hi - lo) as u64;
    let den = (hi_h - lo_h) >> 32;
    if probes < INTERPOLATED && den != 0 {
        return lo + ((span * ((h - lo_h) >> 32) + den / 2) / den) as u32;
    }
    lo + (span / 2) as u32
}

/// What a lookup reads (6, 8.3). `Miss`: no row holds the hash, a track with
/// no data. `Damaged`: a slot's CRC failed, the card's fault: this lookup
/// misses, the file stays. `Broken`: a hash breaks the sort rule under a good
/// CRC, a broken writer's: the file is absent from then on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lookup {
    Hit,
    Miss,
    Damaged,
    Broken,
}

impl Lookup {
    /// The player's word for it: [`Why::Ok`] but for a slot that failed.
    pub(crate) fn why(self) -> Why {
        match self {
            Lookup::Hit | Lookup::Miss => Why::Ok,
            Lookup::Damaged => Why::WvSlotCrc,
            Lookup::Broken => Why::WvSlotOrder,
        }
    }
}

/// A lookup's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) lookup: Lookup,
    /// The row it stopped on: a hit, or the slot that failed.
    pub(crate) row: Option<u32>,
    /// What a hit's slot holds.
    pub(crate) slot: Option<Slot>,
    /// The rows it read, in order.
    pub(crate) probes: Vec<u32>,
}

/// An index whose frame and header passed 8.1 and 8.2: the file is valid as
/// far as 7.2 asks, once its row in the root matches
/// ([`msmf::companion_matches`](super::msmf::companion_matches)). Its slots
/// are checked as they are read.
pub(crate) struct Table<'a> {
    c: Container<'a>,
    wvsl: Section,
    pub(crate) rows: u32,
}

impl<'a> Table<'a> {
    /// Opens an index (8.1, 8.2, in their order): the frame, WVSL present and
    /// its stride at least 256; then slotBytes 256 and barCount 240; WVSL's
    /// stride 256; 1-250,000 rows; WVSL at byte 256, so the one section and
    /// the file 256 + 256 × rows bytes.
    pub(crate) fn open(bytes: &'a [u8]) -> Result<Table<'a>, Why> {
        Table::of(Container::open(bytes, &SPEC)?)
    }

    fn of(c: Container<'a>) -> Result<Table<'a>, Why> {
        let th = c.type_header();
        if u32_at(th, 0) != SLOT_BYTES || u32_at(th, 4) != BAR_COUNT as u32 {
            return Err(Why::Enum);
        }
        let wvsl = c.section(SEC_WVSL).ok_or(Why::Missing)?;
        if wvsl.stride != SLOT_BYTES {
            return Err(Why::Shape);
        }
        if !(1..=MAX_ROWS).contains(&wvsl.count) {
            return Err(Why::Enum);
        }
        if wvsl.offset != FIRST_SLOT {
            return Err(Why::WvLayout);
        }
        Ok(Table { c, wvsl, rows: wvsl.count })
    }

    pub(crate) fn header(&self) -> &Header {
        self.c.header()
    }

    /// The whole file, as a tool verifies it (8.4): every slot in row order,
    /// its CRC, then its hash above the one before; last WVSL's CRC. A reader
    /// that does so reads any bad slot as the file's absence. The device never
    /// runs it: it checks each slot it reads.
    pub(crate) fn check(&self) -> Result<(), Why> {
        let mut prev: Option<u64> = None;
        for r in 0..self.rows {
            let h = self.slot(r)?.path_hash;
            if prev.is_some_and(|p| h <= p) {
                return Err(Why::WvSlotOrder);
            }
            prev = Some(h);
        }
        if !self.c.crc_ok(&self.wvsl) {
            return Err(Why::SectionCrc);
        }
        Ok(())
    }

    /// WVSL's whole. The section's CRC depends only on `rows` when every slot
    /// passes (4.2), and so does headerCrc: whether the contents changed is
    /// these bytes' to say.
    pub(crate) fn wvsl(&self) -> &'a [u8] {
        self.c.data(&self.wvsl)
    }

    /// Row r's slot as stored.
    pub(crate) fn slot_raw(&self, r: u32) -> Option<&'a [u8]> {
        (r < self.rows).then(|| self.c.row(&self.wvsl, r, SLOT_BYTES as usize))
    }

    /// Row r's slot, its CRC checked (W1): a slot that fails, or a row past
    /// the last, reads [`Why::WvSlotCrc`], and costs only that slot.
    pub(crate) fn slot(&self, r: u32) -> Result<Slot, Why> {
        let raw = self.slot_raw(r).ok_or(Why::WvSlotCrc)?;
        if slot_crc(raw, r) != u32_at(raw, CRC_AT) {
            return Err(Why::WvSlotCrc);
        }
        Ok(Slot::decode(raw))
    }

    /// The slot of a card file's path hash, by 6's search. lo and hi are
    /// signed: hi reaches −1 when row 0 is above the hash. The first bounds
    /// aren't slots, so the sort rule looks at loH only once lo has moved, and
    /// at hiH only once hi has: a slot may hold hash 0 or 2^64 − 1.
    pub(crate) fn find(&self, h: u64) -> Found {
        let last = self.rows as i64 - 1;
        let (mut lo, mut hi, mut lo_h, mut hi_h) = (0i64, last, 0u64, u64::MAX);
        let mut probes: Vec<u32> = Vec::new();
        let done = |lookup, row, slot, probes| Found { lookup, row, slot, probes };
        while lo <= hi && (probes.len() as u32) < MAX_PROBES {
            let r = pick(h, lo as u32, hi as u32, lo_h, hi_h, probes.len() as u32);
            probes.push(r);
            let Ok(slot) = self.slot(r) else { return done(Lookup::Damaged, Some(r), None, probes) };
            let v = slot.path_hash;
            if (lo > 0 && v <= lo_h) || (hi < last && v >= hi_h) {
                return done(Lookup::Broken, Some(r), None, probes);
            }
            if v == h {
                return done(Lookup::Hit, Some(r), Some(slot), probes);
            }
            if v < h {
                (lo, lo_h) = (r as i64 + 1, v);
            } else {
                (hi, hi_h) = (r as i64 - 1, v);
            }
        }
        done(Lookup::Miss, None, None, probes)
    }
}

/// [`Table::open`]'s checks from a file's first [`HEAD_BYTES`] and its length,
/// as the device opens one: none of them reads a slot, so a file of any size
/// costs one small read to be valid or absent (7.2). Its header, for its row in
/// the root.
pub(crate) fn open_head(head: &[u8], size: u64) -> Result<Header, Why> {
    Ok(*Table::of(Container::open_head(head, size, &SPEC)?)?.header())
}

/// What a file's writer is given: every value a writer would otherwise choose
/// (2.4.2) comes from its caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TableIn {
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) minor: u16,
    /// In any order ([`in_row_order`]).
    pub(crate) rows: Vec<Slot>,
}

/// The rows a file holds of those given (4.3): by path hash, and a hash two or
/// more rows hold left out, all of them, so a lookup never finds another
/// file's bars. A hash a row shares with a card file that has no row is the
/// caller's to leave out, before this: it goes whichever of them have peaks.
pub(crate) fn in_row_order(rows: &[Slot]) -> Vec<&Slot> {
    let mut order: Vec<&Slot> = rows.iter().collect();
    order.sort_by_key(|s| s.path_hash);
    let alone = order.chunk_by(|a, b| a.path_hash == b.path_hash).filter(|same| same.len() == 1);
    alone.map(|same| same[0]).collect()
}

/// WVSL's whole: rows in row order, each CRC seeded with its place.
pub(crate) fn wvsl_bytes(rows: &[&Slot]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * SLOT_BYTES as usize);
    for (r, slot) in rows.iter().enumerate() {
        out.extend_from_slice(&slot.encode(r as u32));
    }
    out
}

/// Writes an index. Refused: no row left to write (a writer with no row writes
/// no file: 7.3), more than 250,000.
pub(crate) fn write(t: &TableIn) -> Result<Vec<u8>, WriteError> {
    let rows = in_row_order(&t.rows);
    if rows.is_empty() {
        return Err(WriteError("MPWF: no row to write"));
    }
    if rows.len() > MAX_ROWS as usize {
        return Err(WriteError("MPWF: more than 250,000 rows"));
    }
    let data = wvsl_bytes(&rows);
    let mut th = [0; 8];
    th[..4].copy_from_slice(&SLOT_BYTES.to_le_bytes());
    th[4..].copy_from_slice(&(BAR_COUNT as u32).to_le_bytes());
    let count = rows.len() as u32;
    let section = SectionOut { kind: WVSL, flags: REQUIRED, count, stride: SLOT_BYTES, data: &data };
    let (minor, generation, card_id) = (t.minor, t.generation, t.card_id);
    let meta = FileMeta { magic: MPWF, major: MAJOR, minor, generation, card_id };
    container::write(&meta, &th, HEADER_BYTES, &[section])
}

/// What a final commit does about the file (7.3's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// No MPWF: the new root names none.
    None,
    /// The new root names the old file again, its generation older than the
    /// root's. No commit for this alone.
    Same,
    /// A new file, its header values from this commit.
    New,
}

/// 7.3's table, the first line that holds. `writes`: `read.mpwf` lists 1
/// ([`DeviceTxt::writes_mpwf`](super::device_txt::DeviceTxt::writes_mpwf)).
/// `old`: the file the current root names, when it is valid (7.2). `rows`: the
/// rows the writer would write now, in any order. A fetch that stopped early
/// isn't an input: it decides only what `rows` holds. The bytes are compared,
/// not a CRC (4.2).
pub(crate) fn outcome(writes: bool, old: Option<&Table<'_>>, rows: &[Slot]) -> Outcome {
    let rows = in_row_order(rows);
    if !writes || rows.is_empty() {
        return Outcome::None;
    }
    match old.is_some_and(|t| t.wvsl() == wvsl_bytes(&rows)) {
        true => Outcome::Same,
        false => Outcome::New,
    }
}

/// 7.4: a writer writes MPWF only when `/.player/device.txt`'s `read.mpwf`
/// lists 1 (the device_txt module reads it).
pub(crate) fn writes_v1(read_mpwf: u32) -> bool {
    read_mpwf & (1 << (MAJOR - 1)) != 0
}
