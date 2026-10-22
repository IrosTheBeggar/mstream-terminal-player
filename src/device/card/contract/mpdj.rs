//! The Auto DJ index, `autodj-<gen>.bin` (MPDJ v2): read and checked
//! (docs/AUTODJ-INDEX.md 3-4 and 8, docs/METADATA.md 2.13), with the
//! primitives its rules pin (5.2's level, 5.4's strided ranks, 6's
//! signature). Building one — the rows, the exact cosines, the selection,
//! the slots, §7.3's reuse — is the Auto DJ writer's, on these.
//!
//! For each recording on the card that has an embedding (a row): a slot of
//! up to k similar rows with their levels, in a fixed 256- or 512-byte slot
//! the device reads in one sector, each seeded with its row's CRC; each
//! row's BPM, key and artist key (DJRF); a map from every card file's path
//! hash to its row (DJPH); and five strings naming the model and its
//! licence (CC BY-NC-SA: no real index ever goes into a fixture).

use super::container::{Container, FormatSpec, Header, Section, SectionSpec, StrCursor};
use super::container::{DIR_ENTRY, MPDJ, STRS};
use super::prim::{bytes16_at, crc32, crc32_after, fourcc, u16_at, u32_at, u64_at};
use super::Why;

pub(crate) const DJSL: u32 = fourcc(b"DJSL");
pub(crate) const DJRF: u32 = fourcc(b"DJRF");
pub(crate) const DJPH: u32 = fourcc(b"DJPH");

pub(crate) const MAJOR: u16 = 2;
/// The header's length in minor 0 (headerBytes is this or more, padded so
/// DJSL starts on a slot: [`header_bytes_for`]).
pub(crate) const MIN_HEADER_BYTES: u32 = 104;
pub(crate) const SMALL_SLOT: u32 = 256;
pub(crate) const LARGE_SLOT: u32 = 512;
pub(crate) const FILTER_STRIDE: u32 = 8;
pub(crate) const PATH_STRIDE: u32 = 10;
pub(crate) const MAX_ROWS: u32 = 65_535;
pub(crate) const MIN_K: u32 = 2;
pub(crate) const MAX_SMALL_K: u32 = 84;
pub(crate) const MAX_K: u32 = 169;
/// An unused slot entry's index.
pub(crate) const UNUSED: u16 = 0xFFFF;
/// selKind: the exact top k, or strided by rank (the writer's default).
pub(crate) const SEL_TOP: u8 = 0;
pub(crate) const SEL_STRIDED: u8 = 1;
/// DJRF flags bit 0: bpm10 and camelot are mStream's analysis.
pub(crate) const FILTER_BPM_ANALYSED: u8 = 1;

const SEC_DJSL: usize = 0;
const SEC_DJRF: usize = 1;
const SEC_DJPH: usize = 2;
const SEC_STRS: usize = 3;

const SPEC: FormatSpec = FormatSpec {
    magic: MPDJ,
    major: MAJOR,
    header_bytes: MIN_HEADER_BYTES,
    sections: &[
        // slotBytes: a slot's own size is checked against the header.
        SectionSpec { kind: DJSL, stride: SMALL_SLOT, required: true },
        SectionSpec { kind: DJRF, stride: FILTER_STRIDE, required: true },
        SectionSpec { kind: DJPH, stride: PATH_STRIDE, required: true },
        SectionSpec { kind: STRS, stride: 0, required: true },
    ],
};

/// The slot for a k: 256 bytes for k 2-84, 512 for 85-169; 0 for any other
/// k, which no file has.
pub(crate) fn slot_bytes_for(k: u32) -> u32 {
    match k {
        MIN_K..=MAX_SMALL_K => SMALL_SLOT,
        85..=MAX_K => LARGE_SLOT,
        _ => 0,
    }
}

/// 3.3's alignment rule: the smallest multiple of 8, at least the header's
/// size, that ends the directory on a slot boundary, so DJSL — always the
/// first section — starts on one. A writer MUST write it; readers check
/// only the alignment.
pub(crate) fn header_bytes_for(section_count: u32, slot_bytes: u32, minimum: u32) -> u32 {
    if slot_bytes == 0 || slot_bytes % 8 != 0 {
        return 0;
    }
    let mut hb = (minimum + 7) & !7;
    while (hb + DIR_ENTRY * section_count) % slot_bytes != 0 {
        hb += 8;
    }
    hb
}

/// A slot's CRC (4.3): CRC-32 over the row as a little-endian u32, then the
/// slot's bytes before the CRC, so a slot read for the wrong row fails.
pub(crate) fn slot_crc(slot: &[u8], row: u32) -> u32 {
    crc32_after(crc32(&row.to_le_bytes()), &slot[..slot.len() - 4])
}

/// The level of a cosine (5.2): `round(cos × 255)` in f64, half away from
/// zero, kept to 0-255 (a negative cosine is 0).
pub(crate) fn level(cos: f64) -> u8 {
    let r = (cos * 255.0).round();
    if !(r > 0.0) {
        0
    } else if r > 255.0 {
        255
    } else {
        r as u8
    }
}

/// The rule's cosine of two rows (5.2): the sum in index order of their f32
/// components' products, each product and partial sum in f64 (a product of
/// two f32 is exact there, so a fused multiply-add can't change it).
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).fold(0.0f64, |s, (&x, &y)| s + x as f64 * y as f64)
}

/// The level of two rows by the rule.
pub(crate) fn level_of(a: &[f32], b: &[f32]) -> u8 {
    level(cosine(a, b))
}

/// The rank strided entry j takes when a row has m > k candidates (5.4):
/// j itself in the head (j < H), else `H + (j − H)(m − 1 − H) / (k − 1 −
/// H)`; j wherever the list isn't strided (m ≤ k, or a head the list can't
/// hold).
pub(crate) fn strided_rank(j: u32, k: u32, head: u32, m: u32) -> u32 {
    if j < head || m <= k || head + 2 > k {
        return j;
    }
    head + (j - head) * (m - 1 - head) / (k - 1 - head)
}

/// The least level a slider of `percent` passes, `ceil(255 p / 100)` in
/// integers (11.4; informative, the device's).
pub(crate) fn floor_level(percent: u32) -> u32 {
    (255 * percent + 99) / 100
}

/// The selection signature (6): the first 16 bytes of the SHA-256 of its
/// text. Equal signatures mean equal lists; it is also the root's COMP key.
pub(crate) fn signature(text: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(text.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

/// The signature text's `vec` digest: the SHA-256 of every row's
/// components as little-endian f32, in row order.
pub(crate) fn vec_digest<'r>(rows: impl IntoIterator<Item = &'r [f32]>) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for row in rows {
        for v in row {
            h.update(v.to_le_bytes());
        }
    }
    h.finalize().into()
}

/// How one slot reads (8.4, 8.5): its entries, or `Damaged` (its CRC fails:
/// the card's fault, that slot alone unusable) or `Broken` (a good CRC over
/// a bad list: a broken writer, the file absent from then on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotRead {
    Damaged,
    Broken,
}

/// One entry of a slot's list: a row and its level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) row: u16,
    pub(crate) level: u8,
}

/// 8.4's checks of one slot, in order, each over the whole slot: S1 its CRC
/// seeded with `row`; S2 no used entry after an unused one; S3 every used
/// index a row, and not this one; S4 the levels never rise, and a tie has
/// its indexes increasing.
pub(crate) fn check_slot(
    slot: &[u8],
    k: u32,
    row: u32,
    row_count: u32,
) -> Result<Vec<Entry>, (SlotRead, Why)> {
    let k = k as usize;
    if slot.len() < 4 || k == 0 || 3 * k > slot.len() - 4 {
        return Err((SlotRead::Broken, Why::Shape));
    }
    if slot_crc(slot, row) != u32_at(slot, slot.len() - 4) {
        return Err((SlotRead::Damaged, Why::DjSlotCrc));
    }
    let index = |j: usize| u16_at(slot, 2 * j);
    let c = (0..k).find(|&j| index(j) == UNUSED).unwrap_or(k);
    if (c..k).any(|j| index(j) != UNUSED) {
        return Err((SlotRead::Broken, Why::DjSlotUnused));
    }
    if (0..c).any(|j| index(j) as u32 >= row_count || index(j) as u32 == row) {
        return Err((SlotRead::Broken, Why::DjSlotIndex));
    }
    let levels = &slot[2 * k..3 * k];
    for j in 1..c {
        if levels[j] > levels[j - 1] || (levels[j] == levels[j - 1] && index(j) <= index(j - 1)) {
            return Err((SlotRead::Broken, Why::DjSlotOrder));
        }
    }
    Ok((0..c).map(|j| Entry { row: index(j), level: levels[j] }).collect())
}

/// Which sections a reader checks (8.6): a whole-file check every one; the
/// device's join the filters and the paths, its slots as it reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Uses {
    pub(crate) filters: bool,
    pub(crate) paths: bool,
    pub(crate) slots: bool,
}

impl Uses {
    pub(crate) const ALL: Uses = Uses { filters: true, paths: true, slots: true };
    pub(crate) const DEVICE: Uses = Uses { filters: true, paths: true, slots: false };
}

/// A row's filter record (4.4): camelot above 24 reads as 0; flag bits other
/// than BPM_ANALYSED are ignored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) bpm10: u16,
    pub(crate) camelot: u8,
    pub(crate) flags: u8,
    pub(crate) artist_key: u32,
}

/// An index whose frame and header passed 8.1 and 8.2.
pub(crate) struct Table<'a> {
    c: Container<'a>,
    pub(crate) row_count: u32,
    pub(crate) path_count: u32,
    pub(crate) k: u32,
    pub(crate) slot_bytes: u32,
    // The selection as the header has it: for diagnostics, which nothing
    // prints yet.
    #[allow(dead_code)]
    pub(crate) sel_kind: u8,
    #[allow(dead_code)]
    pub(crate) sel_head: u8,
    #[allow(dead_code)]
    pub(crate) sel_floor: u8,
    pub(crate) built_time: u32,
    pub(crate) tags_generation: u32,
    /// The selection signature (6), as stored.
    pub(crate) sig: [u8; 16],
    /// The STRS offsets of modelId, modelVersion, metric, license and
    /// attribution.
    string_at: [u32; 5],
}

impl<'a> Table<'a> {
    /// Opens an index (8.1, 8.2, in their order): the frame; then the
    /// header — rowCount 1-65,535, k 2-169, slotBytes as k says, indexBytes
    /// 2 and scoreKind 1, the selection's fields; the sections' counts,
    /// DJSL's stride; DJSL the first directory entry on a slot boundary and
    /// the four in order.
    pub(crate) fn open(bytes: &'a [u8]) -> Result<Table<'a>, Why> {
        let c = Container::open(bytes, &SPEC)?;
        let th = c.type_header();
        let row_count = u32_at(th, 0);
        let path_count = u32_at(th, 4);
        let k = u16_at(th, 8) as u32;
        let slot_bytes = u16_at(th, 10) as u32;
        let (index_bytes, score_kind, sel_kind, sel_head) = (th[12], th[13], th[14], th[15]);
        let built_time = u32_at(th, 16);
        let tags_generation = u32_at(th, 20);
        let sig = bytes16_at(th, 24);
        let string_at = [u32_at(th, 40), u32_at(th, 44), u32_at(th, 48), u32_at(th, 52), u32_at(th, 56)];
        let sel_floor = th[60];
        if !(1..=MAX_ROWS).contains(&row_count) || !(MIN_K..=MAX_K).contains(&k) {
            return Err(Why::Enum);
        }
        if slot_bytes != slot_bytes_for(k) {
            return Err(Why::Enum);
        }
        if index_bytes != 2 || score_kind != 1 {
            return Err(Why::Enum);
        }
        let selection = match sel_kind {
            SEL_TOP => sel_head as u32 == k && sel_floor == 0,
            SEL_STRIDED => sel_head >= 1 && sel_head as u32 + 2 <= k && sel_floor >= 1,
            // A later kind: readers use only selHead and selFloor (9.3).
            _ => sel_head as u32 <= k,
        };
        if !selection {
            return Err(Why::DjSelection);
        }
        let section = |i| c.section(i).ok_or(Why::Missing);
        let (slots, filters) = (section(SEC_DJSL)?, section(SEC_DJRF)?);
        let (paths, strs) = (section(SEC_DJPH)?, section(SEC_STRS)?);
        if slots.count != row_count || filters.count != row_count || paths.count != path_count {
            return Err(Why::Counts);
        }
        if slots.stride != slot_bytes {
            return Err(Why::Shape);
        }
        let order = [slots.index, filters.index, paths.index, strs.index];
        if slots.index != 0 || slots.offset % slot_bytes != 0 || order.windows(2).any(|w| w[1] <= w[0]) {
            return Err(Why::DjLayout);
        }
        Ok(Table {
            c,
            row_count,
            path_count,
            k,
            slot_bytes,
            sel_kind,
            sel_head,
            sel_floor,
            built_time,
            tags_generation,
            sig,
            string_at,
        })
    }

    fn section(&self, i: usize) -> Section {
        // open() found all four.
        self.c.section(i).unwrap_or_default()
    }

    pub(crate) fn header(&self) -> &Header {
        self.c.header()
    }

    /// 8.3 and 8.4 in 8.6's order: STRS always (its first byte, each of the
    /// five strings inside it, in order, with its NUL, then its CRC); DJRF's
    /// CRC; DJPH's rows in range and hashes strictly increasing, then its
    /// CRC; every slot in row order, then DJSL's CRC — as `uses` says. A
    /// whole-file check reads any bad slot as the file's absence.
    pub(crate) fn check(&self, uses: Uses) -> Result<(), Why> {
        let strs = self.section(SEC_STRS);
        let sdata = self.c.data(&strs);
        let mut ss = StrCursor::new(sdata, 0, sdata.len());
        if ss.byte() != Some(0) {
            return Err(Why::String);
        }
        for off in self.string_at {
            ss.ordered(off)?;
        }
        if !self.c.crc_ok(&strs) {
            return Err(Why::SectionCrc);
        }
        if uses.filters && !self.c.crc_ok(&self.section(SEC_DJRF)) {
            return Err(Why::SectionCrc);
        }
        if uses.paths {
            let mut prev: Option<u64> = None;
            for (h, r) in self.paths() {
                if r as u32 >= self.row_count || prev.is_some_and(|p| h <= p) {
                    return Err(Why::DjPaths);
                }
                prev = Some(h);
            }
            if !self.c.crc_ok(&self.section(SEC_DJPH)) {
                return Err(Why::SectionCrc);
            }
        }
        if uses.slots {
            for r in 0..self.row_count {
                self.slot(r).map_err(|(_, why)| why)?;
            }
            if !self.c.crc_ok(&self.section(SEC_DJSL)) {
                return Err(Why::SectionCrc);
            }
        }
        Ok(())
    }

    /// Row r's filter record.
    pub(crate) fn filter(&self, r: u32) -> Option<Filter> {
        let s = self.section(SEC_DJRF);
        (r < s.count).then(|| {
            let e = self.c.row(&s, r, FILTER_STRIDE as usize);
            Filter {
                bpm10: u16_at(e, 0),
                camelot: if e[2] <= 24 { e[2] } else { 0 },
                flags: e[3] & FILTER_BPM_ANALYSED,
                artist_key: u32_at(e, 4),
            }
        })
    }

    /// DJPH's entries: (path hash, row).
    pub(crate) fn paths(&self) -> Vec<(u64, u16)> {
        let s = self.section(SEC_DJPH);
        (0..s.count)
            .map(|i| {
                let e = self.c.row(&s, i, PATH_STRIDE as usize);
                (u64_at(e, 0), u16_at(e, 8))
            })
            .collect()
    }

    /// Row r's slot as stored.
    pub(crate) fn slot_raw(&self, r: u32) -> Option<&'a [u8]> {
        let s = self.section(SEC_DJSL);
        (r < self.row_count).then(|| self.c.row(&s, r, self.slot_bytes as usize))
    }

    /// Row r's slot as the device reads it (8.4).
    pub(crate) fn slot(&self, r: u32) -> Result<Vec<Entry>, (SlotRead, Why)> {
        let Some(raw) = self.slot_raw(r) else { return Err((SlotRead::Broken, Why::DjSlotIndex)) };
        check_slot(raw, self.k, r, self.row_count)
    }

    /// The five strings — modelId, modelVersion, metric, license,
    /// attribution — any bad UTF-8 replaced; None for an offset outside
    /// STRS or with no NUL after it (a file [`check`](Self::check) refuses).
    pub(crate) fn strings(&self) -> [Option<String>; 5] {
        let sdata = self.c.data(&self.section(SEC_STRS));
        self.string_at.map(|off| {
            let off = off as usize;
            if off == 0 {
                return Some(String::new());
            }
            let tail = sdata.get(off..)?;
            let n = tail.iter().position(|&c| c == 0)?;
            Some(String::from_utf8_lossy(&tail[..n]).into_owned())
        })
    }

    /// Every section in directory order: (type, flags, count, stride,
    /// bytes), so a writer can lay the same file out again.
    pub(crate) fn sections(&self) -> Vec<(u32, u32, u32, u32, &'a [u8])> {
        let b = self.c.bytes();
        let h = self.c.header();
        (0..h.section_count)
            .map(|i| {
                let e = &b[(h.header_bytes + DIR_ENTRY * i) as usize..];
                let (off, len) = (u32_at(e, 8) as usize, u32_at(e, 12) as usize);
                (u32_at(e, 0), u32_at(e, 4), u32_at(e, 16), u32_at(e, 20), &b[off..off + len])
            })
            .collect()
    }
}

/// 7.4: a writer writes MPDJ v2 only when `/.player/device.txt`'s
/// `read.mpdj` lists 2 (the device_txt module reads it).
pub(crate) fn writes_v2(read_mpdj: u32) -> bool {
    read_mpdj & (1 << (MAJOR - 1)) != 0
}
