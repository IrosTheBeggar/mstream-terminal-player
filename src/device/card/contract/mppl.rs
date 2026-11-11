//! `/.player/plays.jnl` (MPPL v1): the device's play log, which the
//! software only reads (docs/METADATA.md 2.20). The device appends one
//! 80-byte slot at each play's end, each slot with its own CRC, to
//! `plays.jnl`, rotating it to `plays.1.jnl` under a cap; a reader reads
//! both, and `plays.2.jnl` to `plays.9.jnl` (a later firmware's longer
//! rotation), in any order: a play's place is its seq, not its file.
//!
//! A play is (deviceId, epoch, seq): the board's MAC, a random epoch NVS
//! keeps (a flasher's erase makes a new one, so the seq starting again at 1
//! never repeats an id), a sequence number. [`read_log`] is 2.20.6's reader:
//! every slot judged, each play once, what it skipped counted; [`dates`]
//! the start of a play made before the clock was set; [`same_file`] whether
//! the file played is still the ledger's; [`next_marks`] how far the posted
//! mark of each (deviceId, epoch) rises once the server has settled plays.
//! The firmware repo's tools/playlog.py is the reference these follow rule
//! for rule, and its fixtures (test/fixtures/card/plays) the goldens.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::fat::time_matches;
use super::msmf::Ack;
use super::prim::{crc32, hex, u16_at, u32_at, u64_at};

/// A slot's length; a file is slots from offset 0, with no header.
pub(crate) const SLOT: usize = 80;
/// `MPPL`, the u32 0x4C50504D.
pub(crate) const MAGIC: [u8; 4] = *b"MPPL";
/// The slot's major; a reader skips another's and counts it.
pub(crate) const MAJOR: u8 = 1;
/// A slot's kinds: a play, and a drop note (plays a rotation removed before
/// any software read them). Any other is a later minor's, skipped.
pub(crate) const KIND_PLAY: u8 = 1;
pub(crate) const KIND_DROP: u8 = 2;
/// A file's cap: 6,553 slots, 524,240 bytes. Bytes past it are no device's:
/// a reader reads only the cap and counts the rest as damaged slots.
pub(crate) const FILE_SLOTS: usize = 6553;
pub(crate) const FILE_BYTES: usize = FILE_SLOTS * SLOT;
/// The files a reader reads, in its order. The twin names a cut rename
/// leaves (`plays.xl1` to `plays.xl9`) are the device's; nobody reads them.
pub(crate) const NAMES: [&str; 10] = [
    "plays.jnl",
    "plays.1.jnl",
    "plays.2.jnl",
    "plays.3.jnl",
    "plays.4.jnl",
    "plays.5.jnl",
    "plays.6.jnl",
    "plays.7.jnl",
    "plays.8.jnl",
    "plays.9.jnl",
];

/// One slot's fields (2.20.3), the reserved bytes aside. A drop note has
/// the clock, epoch, deviceId, startedAt, bootSeq and uptimeMs of the play
/// written after it, its `dropped` in `played_ms`, and the rest 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Slot {
    pub(crate) kind: u8,
    /// 1 completed, 2 skipped, 3 stopped; any other reads as stopped.
    pub(crate) outcome: u8,
    /// 0 not said, 1 manual, 2 shuffle, 3 autodj; any other reads as 0.
    pub(crate) source: u8,
    pub(crate) seq: u32,
    pub(crate) pause_count: u16,
    /// 0 unset, 1 the RTC's, 2 a host's: informational, startedAt decides.
    pub(crate) clock: u8,
    pub(crate) epoch: u64,
    /// The board's MAC, its 6 bytes in their printed order.
    pub(crate) device_id: [u8; 6],
    /// Unix ms when the play began; 0 unknown (the clock unset).
    pub(crate) started_at: u64,
    pub(crate) boot_seq: u32,
    /// Since that boot, when the play began, mod 2^32.
    pub(crate) uptime_ms: u32,
    /// A play's audio played, pauses excluded; a drop note's `dropped`.
    pub(crate) played_ms: u32,
    /// The track's length as the device knew it; 0 unknown.
    pub(crate) duration_ms: u32,
    /// The file played, as the device's library has it: its path hash
    /// (2.3.3), size and raw FAT stamp.
    pub(crate) path_hash: u64,
    pub(crate) size: u32,
    pub(crate) fat_time: u32,
    /// The generation of the root that counted when the play began; 0 none.
    pub(crate) generation: u32,
}

/// A play's identity, and the log's order: the device id's bytes, the
/// epoch unsigned, the seq.
pub(crate) type Key = ([u8; 6], u64, u32);

impl Slot {
    /// The slot's 80 bytes: the fields as given (a test may write an
    /// unknown enum value), the reserved bytes 0, the CRC over bytes 0-75
    /// at 76.
    pub(crate) fn encode(&self) -> [u8; SLOT] {
        let mut b = [0u8; SLOT];
        b[..4].copy_from_slice(&MAGIC);
        b[4] = MAJOR;
        (b[5], b[6], b[7]) = (self.kind, self.outcome, self.source);
        b[8..12].copy_from_slice(&self.seq.to_le_bytes());
        b[12..14].copy_from_slice(&self.pause_count.to_le_bytes());
        b[14] = self.clock;
        b[16..24].copy_from_slice(&self.epoch.to_le_bytes());
        b[24..30].copy_from_slice(&self.device_id);
        b[32..40].copy_from_slice(&self.started_at.to_le_bytes());
        b[40..44].copy_from_slice(&self.boot_seq.to_le_bytes());
        b[44..48].copy_from_slice(&self.uptime_ms.to_le_bytes());
        b[48..52].copy_from_slice(&self.played_ms.to_le_bytes());
        b[52..56].copy_from_slice(&self.duration_ms.to_le_bytes());
        b[56..64].copy_from_slice(&self.path_hash.to_le_bytes());
        b[64..68].copy_from_slice(&self.size.to_le_bytes());
        b[68..72].copy_from_slice(&self.fat_time.to_le_bytes());
        b[72..76].copy_from_slice(&self.generation.to_le_bytes());
        let crc = crc32(&b[..SLOT - 4]);
        b[SLOT - 4..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    fn fields(b: &[u8]) -> Slot {
        let mut device_id = [0u8; 6];
        device_id.copy_from_slice(&b[24..30]);
        Slot {
            kind: b[5],
            outcome: b[6],
            source: b[7],
            seq: u32_at(b, 8),
            pause_count: u16_at(b, 12),
            clock: b[14],
            epoch: u64_at(b, 16),
            device_id,
            started_at: u64_at(b, 32),
            boot_seq: u32_at(b, 40),
            uptime_ms: u32_at(b, 44),
            played_ms: u32_at(b, 48),
            duration_ms: u32_at(b, 52),
            path_hash: u64_at(b, 56),
            size: u32_at(b, 64),
            fat_time: u32_at(b, 68),
            generation: u32_at(b, 72),
        }
    }

    pub(crate) fn key(&self) -> Key {
        (self.device_id, self.epoch, self.seq)
    }

    /// The play's id, mStream's idempotency key (2.20.4): `core2-`, the
    /// device id as 12 lowercase hex digits, `-`, the epoch as 16, `-`, the
    /// seq in decimal. At most 46 characters.
    pub(crate) fn id(&self) -> String {
        format!("core2-{}-{:016x}-{}", hex(&self.device_id), self.epoch, self.seq)
    }

    /// A valid play slot the software may use: an epoch, a seq, a device id
    /// (2.20.6, item 2).
    pub(crate) fn usable(&self) -> bool {
        self.epoch != 0 && self.seq >= 1 && self.device_id != [0; 6]
    }
}

/// 2.20.6's verdict on one slot, in the reference's order: a tail shorter
/// than a slot; the magic; the major (a newer firmware's, CRC unchecked);
/// the CRC; then the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// A play or a drop note.
    Ok(Slot),
    /// A valid slot of a kind this reader doesn't know: a later minor's.
    Kind(u8),
    /// The magic, another major: a newer firmware's.
    Newer(u8),
    /// Another magic, or the CRC fails: damaged.
    Damaged,
    /// Fewer than 80 bytes: a cut append.
    Short,
}

pub(crate) fn decode(b: &[u8]) -> Verdict {
    if b.len() < SLOT {
        return Verdict::Short;
    }
    if b[..4] != MAGIC {
        return Verdict::Damaged;
    }
    if b[4] != MAJOR {
        return Verdict::Newer(b[4]);
    }
    if crc32(&b[..SLOT - 4]) != u32_at(b, SLOT - 4) {
        return Verdict::Damaged;
    }
    match b[5] {
        KIND_PLAY | KIND_DROP => Verdict::Ok(Slot::fields(&b[..SLOT])),
        kind => Verdict::Kind(kind),
    }
}

/// What a reader skipped, and the drop notes' sum (2.20.6, items 1-3, 8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    /// Every slot read, whatever came of it (a tail aside).
    pub(crate) slots: u64,
    pub(crate) damaged: u64,
    /// Cut appends: a file's trailing part shorter than a slot.
    pub(crate) tail: u64,
    /// A newer firmware's slots: the software needs an update.
    pub(crate) newer: u64,
    pub(crate) unknown_kinds: u64,
    /// Valid play slots with no epoch, seq or device id.
    pub(crate) unusable: u64,
    /// Byte-equal second copies (a cut rotation's), each taken once.
    pub(crate) twins: u64,
    /// Keys whose slots differ: neither is used.
    pub(crate) conflicts: u64,
    /// The drop notes' `dropped`: plays lost before any software read them.
    pub(crate) dropped: u64,
}

/// One file of the log as a reader has it: its first bytes, up to the cap,
/// and its whole length.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LogFile<'a> {
    pub(crate) name: &'a str,
    pub(crate) bytes: &'a [u8],
    pub(crate) size: u64,
}

impl<'a> LogFile<'a> {
    /// A file read whole.
    pub(crate) fn whole(name: &'a str, bytes: &'a [u8]) -> LogFile<'a> {
        LogFile { name, bytes, size: bytes.len() as u64 }
    }
}

/// The log as the software takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Log {
    /// The plays, each once, in [`Key`] order.
    pub(crate) plays: Vec<Slot>,
    pub(crate) counts: Counts,
}

impl Log {
    /// The highest seq of a play of `device_id` and `epoch` the log holds; 0
    /// for none.
    pub(crate) fn last_seq(&self, device_id: [u8; 6], epoch: u64) -> u32 {
        let mine = self.plays.iter().filter(|p| (p.device_id, p.epoch) == (device_id, epoch));
        mine.map(|p| p.seq).max().unwrap_or(0)
    }
}

/// 2.20.6's reader over the log's files ([`NAMES`]; others are left out),
/// in the reader's order, though the order changes nothing: every slot
/// judged, reading on past a damaged one; a file's bytes past the cap
/// counted as damaged slots, unread; each play once, by its key, two
/// byte-equal slots one play and two different ones none.
pub(crate) fn read_log(files: &[LogFile<'_>]) -> Log {
    let mut files: Vec<(usize, &LogFile<'_>)> = files
        .iter()
        .filter_map(|f| NAMES.iter().position(|n| *n == f.name).map(|at| (at, f)))
        .collect();
    files.sort_by_key(|(at, _)| *at);
    let mut counts = Counts::default();
    let mut seen: BTreeMap<Key, (Slot, [u8; SLOT])> = BTreeMap::new();
    let mut conflicted: HashSet<Key> = HashSet::new();
    let mut notes: HashSet<[u8; SLOT]> = HashSet::new();
    for (_, file) in files {
        let end = file.bytes.len().min(FILE_BYTES);
        for raw in file.bytes[..end].chunks(SLOT) {
            let verdict = decode(raw);
            if verdict == Verdict::Short {
                counts.tail += 1;
                continue;
            }
            counts.slots += 1;
            let slot = match verdict {
                Verdict::Ok(slot) => slot,
                Verdict::Newer(_) => {
                    counts.newer += 1;
                    continue;
                }
                Verdict::Kind(_) => {
                    counts.unknown_kinds += 1;
                    continue;
                }
                Verdict::Damaged | Verdict::Short => {
                    counts.damaged += 1;
                    continue;
                }
            };
            let mut bytes = [0u8; SLOT];
            bytes.copy_from_slice(raw);
            if slot.kind == KIND_DROP {
                if notes.insert(bytes) {
                    counts.dropped += slot.played_ms as u64;
                } else {
                    counts.twins += 1;
                }
                continue;
            }
            if !slot.usable() {
                counts.unusable += 1;
                continue;
            }
            let key = slot.key();
            if conflicted.contains(&key) {
                continue;
            }
            match seen.get(&key) {
                Some((_, first)) if *first == bytes => counts.twins += 1,
                // One key, two different slots: neither is used, so a play
                // is never credited wrongly.
                Some(_) => {
                    seen.remove(&key);
                    conflicted.insert(key);
                    counts.conflicts += 1;
                }
                None => {
                    seen.insert(key, (slot, bytes));
                }
            }
        }
        // Past the cap: no device's bytes, counted as the slots (or parts
        // of one) they would be.
        if file.size > FILE_BYTES as u64 {
            let slots = (file.size - FILE_BYTES as u64).div_ceil(SLOT as u64);
            counts.slots += slots;
            counts.damaged += slots;
        }
    }
    Log { plays: seen.into_values().map(|(slot, _)| slot).collect(), counts }
}

/// mStream's `outcome` (2.20.3): an unknown value reads as stopped.
pub(crate) fn outcome_name(v: u8) -> &'static str {
    match v {
        1 => "completed",
        2 => "skipped",
        _ => "stopped",
    }
}

/// The source as the device knew it (2.20.3): an unknown value reads as 0,
/// not said, which is mStream's `other`.
pub(crate) fn source_name(v: u8) -> &'static str {
    match v {
        1 => "manual",
        2 => "shuffle",
        3 => "autodj",
        _ => "other",
    }
}

/// Each play's start in Unix ms, or None (2.20.6, item 7). A startedAt
/// other than 0 is the start. A play with none takes it from the first play
/// after it, by seq, of the same device, epoch and boot that has one: that
/// play's startedAt less the uptime between them (mod 2^32). With none, the
/// start is unknown.
pub(crate) fn dates(plays: &[Slot]) -> HashMap<Key, Option<u64>> {
    let mut boots: BTreeMap<([u8; 6], u64, u32), Vec<&Slot>> = BTreeMap::new();
    for p in plays {
        boots.entry((p.device_id, p.epoch, p.boot_seq)).or_default().push(p);
    }
    let mut out = HashMap::new();
    for group in boots.values_mut() {
        group.sort_by_key(|p| p.seq);
        let mut later: Option<&Slot> = None;
        for p in group.iter().rev() {
            if p.started_at != 0 {
                out.insert(p.key(), Some(p.started_at));
                later = Some(p);
                continue;
            }
            let gap = |l: &Slot| l.uptime_ms.wrapping_sub(p.uptime_ms) as u64;
            out.insert(p.key(), later.and_then(|l| l.started_at.checked_sub(gap(l))));
        }
    }
    out
}

/// The file the device played is the ledger row's (2.20.6, item 6): the
/// same size, and valid stamps that are equal or differ by the tags file's
/// skew (2.3.4). A 0 or an invalid stamp never matches: a slot has no
/// fingerprint to decide by.
pub(crate) fn same_file(play: &Slot, size: u32, fat_time: u32, skew: i32) -> bool {
    play.size == size && time_matches(fat_time, play.fat_time, skew)
}

/// The posted marks once `done` says which plays are settled (2.20.6, item
/// 10): for each device and epoch, from its mark in `posted` (0 for none),
/// up to the highest seq below which every play the log holds is done —
/// settled by the server (accepted, a duplicate, rejected), or not sent. A
/// play still waiting holds it back; a seq the log doesn't hold (lost, or
/// reserved and never used) doesn't. The marks in `posted` of devices and
/// epochs the log holds nothing of stay as they are; none is lowered.
pub(crate) fn next_marks(plays: &[Slot], posted: &[Ack], done: &dyn Fn(&Slot) -> bool) -> Vec<Ack> {
    let mut marks: BTreeMap<([u8; 6], u64), u32> = BTreeMap::new();
    for a in posted {
        let m = marks.entry(a.key()).or_insert(0);
        *m = (*m).max(a.seq);
    }
    let mut groups: BTreeMap<([u8; 6], u64), Vec<&Slot>> = BTreeMap::new();
    for p in plays {
        groups.entry((p.device_id, p.epoch)).or_default().push(p);
    }
    for (key, mut group) in groups {
        group.sort_by_key(|p| p.seq);
        let from = marks.get(&key).copied().unwrap_or(0);
        let mut m = from;
        for p in group.into_iter().filter(|p| p.seq > from) {
            if !done(p) {
                break;
            }
            m = p.seq;
        }
        if m > 0 {
            marks.insert(key, m);
        }
    }
    marks.into_iter().map(|((device_id, epoch), seq)| Ack { device_id, epoch, seq }).collect()
}
