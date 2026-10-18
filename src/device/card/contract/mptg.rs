//! Tags files (MPTG v1, docs/METADATA.md 2.6): the software's
//! `/.mstream/tags-<gen>.bin` (source 2), and the device's
//! `/.player/tags.bin` (source 1), one format for both producers.
//!
//! The folder table (FOLD) in pre-order, one record a file (RECS) in the
//! canonical order, the strings (STRS: the names, and each record's run of
//! tag fields), the path-hash index (HIDX) and, for the software, the
//! ledger (ORIG, with its server paths in OSTR): what each file on the card
//! is on its server. The same files and values give the same bytes from
//! both producers ([`write`]), and [`read`] checks every structure the
//! device's streaming builder relies on (2.4.3), in the player's order
//! (`lib/core/CardTags.cpp`'s walker), so a file in the wrong order, a
//! folder that is its own parent or a wrong firstRecord is absent here for
//! the same reason it is on the device.

use std::collections::{HashMap, HashSet};

use super::container::{self, Container, FileMeta, FormatSpec, Header, SectionOut, SectionSpec};
use super::container::{MPTG, REQUIRED, STRS, StrCursor, Strs};
use super::order::{MAX_REL_PATH, compare_folder_paths, compare_names, split_last, valid_name, valid_rel_path};
use super::prim::{MUSIC_HASH, bytes16_at, child_hash, fourcc, path_hash, u16_at, u32_at, u64_at};
use super::text::{ALBUM, ALBUM_ARTIST, ARTIST, RUN_FIELDS, Run, SEPARATOR, cut_utf8, field_max};
use super::{WriteError, Why};

pub(crate) const FOLD: u32 = fourcc(b"FOLD");
pub(crate) const RECS: u32 = fourcc(b"RECS");
pub(crate) const HIDX: u32 = fourcc(b"HIDX");
pub(crate) const ORIG: u32 = fourcc(b"ORIG");
pub(crate) const OSTR: u32 = fourcc(b"OSTR");

pub(crate) const MAJOR: u16 = 1;
pub(crate) const HEADER_BYTES: u32 = 72;
pub(crate) const FOLD_STRIDE: u32 = 16;
pub(crate) const RECS_STRIDE: u32 = 72;
pub(crate) const HIDX_STRIDE: u32 = 12;
pub(crate) const ORIG_STRIDE: u32 = 80;
/// Folder 0's parent.
pub(crate) const NO_PARENT: u32 = 0xFFFF_FFFF;

/// The header's `source` (2.6.1): who read the records.
pub(crate) const SOURCE_DEVICE: u8 = 1;
pub(crate) const SOURCE_SOFTWARE: u8 = 2;
pub(crate) const SOURCE_SYNC_AGENT: u8 = 3;
/// The readRules the records follow (part 5's reading rules).
pub(crate) const READ_RULES: u16 = 1;

/// FOLD flags (2.6.3): the producer created the folder (sources 2 and 3);
/// `/.mstream/thumbs` has the album folder's thumbnail (2.14.1).
pub(crate) const FOLDER_OWNED: u32 = 1;
pub(crate) const FOLDER_THUMB: u32 = 2;

/// RECS `flags` (2.6.4).
pub(crate) const COMPILATION_MASK: u16 = 3;
pub(crate) const HAS_RG_TRACK: u16 = 1 << 2;
pub(crate) const HAS_RG_ALBUM: u16 = 1 << 3;
pub(crate) const RG_FROM_R128: u16 = 1 << 4;
pub(crate) const NO_TAGS: u16 = 1 << 5;
pub(crate) const UNREADABLE: u16 = 1 << 6;
pub(crate) const TRUNCATED: u16 = 1 << 7;
pub(crate) const BPM_ANALYSED: u16 = 1 << 8;
pub(crate) const FROM_API: u16 = 1 << 9;
/// `known` for a readable audio file under readRules 1: bits 0-16.
pub(crate) const KNOWN_RULES1: u32 = 0x1FFFF;

/// RECS `container`.
pub(crate) const CONTAINER_UNKNOWN: u8 = 0;
pub(crate) const CONTAINER_MP3: u8 = 1;
pub(crate) const CONTAINER_FLAC: u8 = 2;
pub(crate) const CONTAINER_OPUS: u8 = 3;
pub(crate) const CONTAINER_NOT_AUDIO: u8 = 255;

/// ORIG `originFlags` (2.6.6).
pub(crate) const HASH_SAMPLED: u8 = 1;
pub(crate) const VERIFIED: u8 = 2;
pub(crate) const ADOPTED: u8 = 4;

/// How deep a folder stack the reader keeps: a path of at most 248 bytes
/// has at most 124 names.
const MAX_DEPTH: usize = (MAX_REL_PATH + 1) / 2;

const SEC_FOLD: usize = 0;
const SEC_RECS: usize = 1;
const SEC_STRS: usize = 2;
const SEC_HIDX: usize = 3;
const SEC_ORIG: usize = 4;
const SEC_OSTR: usize = 5;

const SPEC: FormatSpec = FormatSpec {
    magic: MPTG,
    major: MAJOR,
    header_bytes: HEADER_BYTES,
    sections: &[
        SectionSpec { kind: FOLD, stride: FOLD_STRIDE, required: true },
        SectionSpec { kind: RECS, stride: RECS_STRIDE, required: true },
        SectionSpec { kind: STRS, stride: 0, required: true },
        SectionSpec { kind: HIDX, stride: HIDX_STRIDE, required: false },
        SectionSpec { kind: ORIG, stride: ORIG_STRIDE, required: false },
        SectionSpec { kind: OSTR, stride: 0, required: false },
    ],
};

/// Which sections a reader uses: the software every one; the device's
/// builder FOLD, RECS and STRS (it ignores the ledger, and builds its own
/// index of paths).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Uses {
    pub(crate) hidx: bool,
    pub(crate) ledger: bool,
}

impl Uses {
    pub(crate) const ALL: Uses = Uses { hidx: true, ledger: true };
    pub(crate) const DEVICE: Uses = Uses { hidx: false, ledger: false };
}

/// One file's record (2.6.4), its layout fields (folder, name, run
/// offsets) aside: what the producer read of the file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Record {
    pub(crate) size: u32,
    pub(crate) fat_time: u32,
    pub(crate) duration_ms: u32,
    pub(crate) qfp: u64,
    pub(crate) known: u32,
    pub(crate) flags: u16,
    pub(crate) year: u16,
    pub(crate) track: u16,
    pub(crate) track_total: u16,
    pub(crate) disc: u16,
    pub(crate) disc_total: u16,
    pub(crate) bpm10: u16,
    pub(crate) rg_track_gain: i16,
    pub(crate) rg_album_gain: i16,
    pub(crate) rg_track_peak: u16,
    pub(crate) rg_album_peak: u16,
    pub(crate) container: u8,
    pub(crate) camelot: u8,
    pub(crate) pic_offset: u32,
    pub(crate) pic_length: u32,
    pub(crate) pic_type: u8,
    pub(crate) pic_mime: u8,
    pub(crate) pic_coding: u8,
}

impl Record {
    fn encode(&self, folder: u32, name: u32, strings: u32) -> [u8; RECS_STRIDE as usize] {
        let mut b = [0u8; RECS_STRIDE as usize];
        let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
        put(0, &folder.to_le_bytes());
        put(4, &name.to_le_bytes());
        put(8, &strings.to_le_bytes());
        put(12, &self.size.to_le_bytes());
        put(16, &self.fat_time.to_le_bytes());
        put(20, &self.duration_ms.to_le_bytes());
        put(24, &self.qfp.to_le_bytes());
        put(32, &self.known.to_le_bytes());
        put(36, &self.flags.to_le_bytes());
        put(38, &self.year.to_le_bytes());
        put(40, &self.track.to_le_bytes());
        put(42, &self.track_total.to_le_bytes());
        put(44, &self.disc.to_le_bytes());
        put(46, &self.disc_total.to_le_bytes());
        put(48, &self.bpm10.to_le_bytes());
        put(50, &self.rg_track_gain.to_le_bytes());
        put(52, &self.rg_album_gain.to_le_bytes());
        put(54, &self.rg_track_peak.to_le_bytes());
        put(56, &self.rg_album_peak.to_le_bytes());
        put(58, &[self.container, self.camelot]);
        put(60, &self.pic_offset.to_le_bytes());
        put(64, &self.pic_length.to_le_bytes());
        put(68, &[self.pic_type, self.pic_mime, self.pic_coding, 0]);
        b
    }

    /// A record from its row's first 72 bytes, and the row's folder index,
    /// name offset and run offset.
    fn decode(b: &[u8]) -> (Record, u32, u32, u32) {
        let r = Record {
            size: u32_at(b, 12),
            fat_time: u32_at(b, 16),
            duration_ms: u32_at(b, 20),
            qfp: u64_at(b, 24),
            known: u32_at(b, 32),
            flags: u16_at(b, 36),
            year: u16_at(b, 38),
            track: u16_at(b, 40),
            track_total: u16_at(b, 42),
            disc: u16_at(b, 44),
            disc_total: u16_at(b, 46),
            bpm10: u16_at(b, 48),
            rg_track_gain: u16_at(b, 50) as i16,
            rg_album_gain: u16_at(b, 52) as i16,
            rg_track_peak: u16_at(b, 54),
            rg_album_peak: u16_at(b, 56),
            container: b[58],
            camelot: b[59],
            pic_offset: u32_at(b, 60),
            pic_length: u32_at(b, 64),
            pic_type: b[68],
            pic_mime: b[69],
            pic_coding: b[70],
        };
        (r, u32_at(b, 0), u32_at(b, 4), u32_at(b, 8))
    }

    // An unknown enum value as 2.4.5's table reads it.

    /// The container, 4-254 (reserved) read as 0, unknown.
    pub(crate) fn container_kind(&self) -> u8 {
        if (4..=254).contains(&self.container) { CONTAINER_UNKNOWN } else { self.container }
    }

    /// The Camelot key, above 24 read as 0, none.
    pub(crate) fn camelot_key(&self) -> u8 {
        if self.camelot > 24 { 0 } else { self.camelot }
    }

    /// The picture's MIME, above 3 read as 3, other.
    pub(crate) fn pic_mime_kind(&self) -> u8 {
        self.pic_mime.min(3)
    }

    /// An embedded picture is anchored: a known coding and an offset.
    pub(crate) fn has_picture(&self) -> bool {
        self.pic_coding <= 3 && self.pic_offset != 0
    }

    /// The compilation bits: 0 not said, 1 yes, 2 said no (3 reads as 0).
    pub(crate) fn compilation(&self) -> u8 {
        match (self.flags & COMPILATION_MASK) as u8 {
            3 => 0,
            c => c,
        }
    }
}

/// One ledger row (2.6.6): what the file is on its server. The device never
/// reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Ledger {
    /// mStream's `filepath` (`vpath/relative/path.ext`) as the server
    /// returned it; "" for an owned file with no server track.
    pub(crate) server_path: String,
    /// mStream's track `id` (ids of 2^32 and more are stored as 0).
    pub(crate) mstream_id: u32,
    pub(crate) album_id: u32,
    pub(crate) artist_id: u32,
    pub(crate) audio_hash: [u8; 16],
    pub(crate) file_hash: [u8; 16],
    /// mStream's `modified`, epoch ms.
    pub(crate) server_modified: u64,
    pub(crate) server_size: u64,
    /// mStream's `created-at` as Unix seconds.
    pub(crate) created_at: u32,
    pub(crate) hash_v: u16,
    pub(crate) origin_flags: u8,
    /// 0: the card file is the server's bytes; else the container code of a
    /// conversion.
    pub(crate) converted_to: u8,
    pub(crate) convert_kbps: u16,
    /// The server the file came from (2.5.5): 0 the root's own, i ≥ 1 SRVS
    /// entry i − 1.
    pub(crate) server: u16,
}

impl Ledger {
    fn encode(&self, server_path: u32) -> [u8; ORIG_STRIDE as usize] {
        let mut b = [0u8; ORIG_STRIDE as usize];
        let mut put = |at: usize, v: &[u8]| b[at..at + v.len()].copy_from_slice(v);
        put(0, &server_path.to_le_bytes());
        put(4, &self.mstream_id.to_le_bytes());
        put(8, &self.album_id.to_le_bytes());
        put(12, &self.artist_id.to_le_bytes());
        put(16, &self.audio_hash);
        put(32, &self.file_hash);
        put(48, &self.server_modified.to_le_bytes());
        put(56, &self.server_size.to_le_bytes());
        put(64, &self.created_at.to_le_bytes());
        put(68, &self.hash_v.to_le_bytes());
        put(70, &[self.origin_flags, self.converted_to]);
        put(72, &self.convert_kbps.to_le_bytes());
        put(74, &self.server.to_le_bytes());
        b
    }

    /// A row from its first 80 bytes (server path aside), and the path's
    /// OSTR offset.
    fn decode(b: &[u8]) -> (Ledger, u32) {
        let l = Ledger {
            server_path: String::new(),
            mstream_id: u32_at(b, 4),
            album_id: u32_at(b, 8),
            artist_id: u32_at(b, 12),
            audio_hash: bytes16_at(b, 16),
            file_hash: bytes16_at(b, 32),
            server_modified: u64_at(b, 48),
            server_size: u64_at(b, 56),
            created_at: u32_at(b, 64),
            hash_v: u16_at(b, 68),
            origin_flags: b[70],
            converted_to: b[71],
            convert_kbps: u16_at(b, 72),
            server: u16_at(b, 74),
        };
        (l, u32_at(b, 0))
    }

    /// mStream's canonical key: `audio-hash`, else `hash`
    /// (`COALESCE(audio_hash, file_hash)`); None when it has neither.
    pub(crate) fn canonical_hash(&self) -> Option<[u8; 16]> {
        [self.audio_hash, self.file_hash].into_iter().find(|h| *h != [0; 16])
    }
}

/// A folder of a tags file read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Folder {
    /// Relative to `/music`; `""` for folder 0, `/music` itself.
    pub(crate) path: String,
    pub(crate) parent: u32,
    pub(crate) flags: u32,
    pub(crate) first_record: u32,
    pub(crate) hash: u64,
}

/// A record of a tags file read back: the file's path relative to `/music`
/// and its hash, its folder's index, the record, its run, and its ledger
/// row when the file has one and the reader read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) hash: u64,
    pub(crate) folder: u32,
    pub(crate) rec: Record,
    pub(crate) run: Run,
    pub(crate) ledger: Option<Ledger>,
}

/// A tags file that passed 2.4.3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tags {
    pub(crate) header: Header,
    pub(crate) source: u8,
    pub(crate) parser_version: u16,
    pub(crate) read_rules: u16,
    pub(crate) album_values: u32,
    pub(crate) artist_values: u32,
    /// The writer and its version; informational, any bad UTF-8 replaced.
    pub(crate) producer: String,
    /// In pre-order, folder 0 first.
    pub(crate) folders: Vec<Folder>,
    /// In the canonical order.
    pub(crate) records: Vec<Entry>,
    /// The file has a valid ledger and it was read (2.11: a source 2 or 3
    /// file without one is a card with no ledger).
    pub(crate) has_ledger: bool,
}

impl Tags {
    /// What [`write`] makes this file from again: the same header values,
    /// the records, and the folders whose flags say more than that they are
    /// a record's ancestor (OWNED, THUMB). The minor is this writer's, 0: a
    /// newer minor's additions are dropped, and its next run derives them
    /// again (2.4.5).
    pub(crate) fn to_input(&self) -> (Meta, Vec<RecordIn>, Vec<FolderIn>) {
        let meta = Meta {
            source: self.source,
            parser_version: self.parser_version,
            read_rules: self.read_rules,
            generation: self.header.generation,
            card_id: self.header.card_id,
            minor: 0,
            producer: self.producer.clone(),
        };
        let records = self
            .records
            .iter()
            .map(|e| RecordIn {
                path: e.path.clone(),
                rec: e.rec,
                run: e.run.clone(),
                ledger: e.ledger.clone(),
            })
            .collect();
        let folders = self
            .folders
            .iter()
            .filter(|f| f.flags != 0)
            .map(|f| FolderIn { path: f.path.clone(), flags: f.flags })
            .collect();
        (meta, records, folders)
    }
}

/// The text of a name or a path: the software's own check on top of the
/// player's. The software writes only UTF-8, and a name it can't spell
/// can't be matched on a PC's listing, so such a file is absent to it.
fn utf8(b: &[u8], why: Why) -> Result<&str, Why> {
    std::str::from_utf8(b).map_err(|_| why)
}

/// A folder on the reader's stack: its index, path hash, path, and the
/// offset of its own name in that path.
struct Level {
    index: u32,
    hash: u64,
    path: String,
    name_at: usize,
}

/// The next record, decoded and its name read, before it is placed.
struct Look<'a> {
    folder: u32,
    rec: Record,
    name: &'a [u8],
    strings: u32,
}

/// Reads a tags file, checking it as the player's walker does, in its
/// order (2.4.3): the frame; the header's source and counts, and folder 0
/// being there; the sections the reader uses; then a walk of the folders in
/// pre-order with the records of each — every parent on the stack, siblings
/// and records strictly increasing by name bytes, firstRecord right, every
/// name valid, every path within 255 bytes, every string inside its
/// section in 2.4.4's order; then the sections' CRCs, and HIDX against the
/// records' own (pathHash, record) pairs.
pub(crate) fn read(bytes: &[u8], uses: Uses) -> Result<Tags, Why> {
    let c = Container::open(bytes, &SPEC)?;
    let th = c.type_header();
    let source = th[0];
    let parser_version = u16_at(th, 2);
    let read_rules = u16_at(th, 4);
    let record_count = u32_at(th, 8);
    let folder_count = u32_at(th, 12);
    let album_values = u32_at(th, 16);
    let artist_values = u32_at(th, 20);
    let producer_at = u32_at(th, 24);
    if !(SOURCE_DEVICE..=SOURCE_SYNC_AGENT).contains(&source) {
        return Err(Why::Enum);
    }
    let fold = c.section(SEC_FOLD).ok_or(Why::Missing)?;
    let recs = c.section(SEC_RECS).ok_or(Why::Missing)?;
    let strs = c.section(SEC_STRS).ok_or(Why::Missing)?;
    if fold.count != folder_count || recs.count != record_count {
        return Err(Why::Counts);
    }
    if folder_count == 0 {
        return Err(Why::FoldRoot);
    }
    let hidx = c.section(SEC_HIDX).filter(|_| uses.hidx);
    if uses.hidx {
        if record_count > 0 && hidx.is_none() {
            return Err(Why::Missing);
        }
        if hidx.is_some_and(|h| h.count != record_count) {
            return Err(Why::Hidx);
        }
    }
    let orig = c.section(SEC_ORIG).filter(|_| uses.ledger);
    let ostr = c.section(SEC_OSTR);
    if let Some(o) = orig {
        if ostr.is_none() {
            return Err(Why::Missing);
        }
        if o.count != record_count {
            return Err(Why::Orig);
        }
    }
    let sdata = c.data(&strs);
    if sdata.is_empty() {
        return Err(Why::String);
    }

    // STRS is read as two runs at once (2.4.4): the producer and the folder
    // names, then from the first record's name on, the records' names and
    // runs.
    let mut first: Option<(Record, u32, u32, u32)> = None;
    let split = if record_count > 0 {
        let row = Record::decode(c.row(&recs, 0, RECS_STRIDE as usize));
        if row.2 == 0 {
            return Err(Why::Name);
        }
        if row.2 as usize >= sdata.len() {
            return Err(Why::String);
        }
        first = Some(row);
        row.2 as usize
    } else {
        sdata.len()
    };
    let mut names = StrCursor::new(sdata, 0, split);
    let mut runs = StrCursor::new(sdata, split, sdata.len());
    if names.byte() != Some(0) {
        return Err(Why::String);
    }
    let mut producer = String::new();
    if producer_at != 0 {
        if producer_at as usize >= sdata.len() {
            return Err(Why::String);
        }
        if producer_at as usize >= split {
            return Err(Why::StringOrder);
        }
        producer = String::from_utf8_lossy(names.string_at(producer_at as usize)?).into_owned();
    }
    let ledger = orig.zip(ostr);
    let odata = ostr.map_or(&[][..], |s| c.data(&s));
    let mut ostrs = StrCursor::new(odata, 0, odata.len());
    if ledger.is_some() && ostrs.byte() != Some(0) {
        return Err(Why::String);
    }
    let mut look: Option<Look> = None;
    if let Some((rec, folder, _, strings)) = first {
        let name = runs.next_string()?;
        if name.len() > MAX_REL_PATH {
            return Err(Why::PathLength);
        }
        if !valid_name(name) {
            return Err(Why::Name);
        }
        if folder >= folder_count {
            return Err(Why::RecsFolder);
        }
        look = Some(Look { folder, rec, name, strings });
    }

    let mut folders: Vec<Folder> = Vec::with_capacity(folder_count as usize);
    let mut records: Vec<Entry> = Vec::with_capacity(record_count as usize);
    let mut stack: Vec<Level> = Vec::new();
    let (mut next_folder, mut cur_folder, mut next_record) = (0u32, None::<u32>, 1u32);
    loop {
        let in_folder = matches!((cur_folder, &look), (Some(cf), Some(l)) if l.folder == cf);
        if in_folder {
            // ── A record of the folder entered last ──
            let Some(l) = look.take() else { break };
            let top = stack.last().ok_or(Why::FoldParent)?;
            let at = if top.path.is_empty() { 0 } else { top.path.len() + 1 };
            if at + l.name.len() > MAX_REL_PATH {
                return Err(Why::PathLength);
            }
            let name = utf8(l.name, Why::Name)?;
            let path = if top.path.is_empty() { name.to_owned() } else { format!("{}/{name}", top.path) };
            let hash = child_hash(top.hash, name);
            let index = records.len() as u32;
            let run = match l.strings {
                0 => Run::default(),
                at => read_run(&mut runs, sdata, at as usize)?,
            };
            let mut entry_ledger = None;
            if let Some((o, _)) = ledger {
                let (mut row, path_at) = Ledger::decode(c.row(&o, index, ORIG_STRIDE as usize));
                if path_at != 0 {
                    if path_at as usize >= odata.len() {
                        return Err(Why::String);
                    }
                    if (path_at as usize) < ostrs.position() {
                        return Err(Why::StringOrder);
                    }
                    row.server_path = utf8(ostrs.string_at(path_at as usize)?, Why::String)?.to_owned();
                }
                entry_ledger = Some(row);
            }
            records.push(Entry { path, hash, folder: l.folder, rec: l.rec, run, ledger: entry_ledger });

            // The next record, in order after this one: its folder, then its
            // name.
            if next_record < record_count {
                let row = c.row(&recs, next_record, RECS_STRIDE as usize);
                let (rec, folder, name_at, strings) = Record::decode(row);
                if folder >= folder_count {
                    return Err(Why::RecsFolder);
                }
                if folder < l.folder {
                    return Err(Why::RecsOrder);
                }
                if name_at == 0 {
                    return Err(Why::Name);
                }
                if name_at as usize >= sdata.len() {
                    return Err(Why::String);
                }
                if (name_at as usize) < runs.position() {
                    return Err(Why::StringOrder);
                }
                let name = runs.string_at(name_at as usize)?;
                if name.len() > MAX_REL_PATH {
                    return Err(Why::PathLength);
                }
                if !valid_name(name) {
                    return Err(Why::Name);
                }
                if folder == l.folder && compare_names(name, l.name).is_le() {
                    return Err(Why::RecsOrder);
                }
                next_record += 1;
                look = Some(Look { folder, rec, name, strings });
            }
            continue;
        }
        if next_folder < folder_count {
            if look.as_ref().is_some_and(|l| l.folder < next_folder) {
                return Err(Why::RecsOrder);
            }
            // ── The next folder in pre-order ──
            let i = next_folder;
            let row = c.row(&fold, i, FOLD_STRIDE as usize);
            let (parent, name_at) = (u32_at(row, 0), u32_at(row, 4));
            let (flags, first_record) = (u32_at(row, 8), u32_at(row, 12));
            if i == 0 {
                if parent != NO_PARENT || name_at != 0 {
                    return Err(Why::FoldRoot);
                }
                stack.push(Level { index: 0, hash: MUSIC_HASH, path: String::new(), name_at: 0 });
            } else {
                if parent >= i {
                    return Err(Why::FoldParent);
                }
                // Pop to the parent, which must be on the stack (the folder
                // before, or one of its ancestors); the level popped just
                // above it is the previous sibling.
                let mut prev: Option<Level> = None;
                while stack.last().is_some_and(|top| top.index != parent) {
                    prev = stack.pop();
                }
                let Some(up) = stack.last() else { return Err(Why::FoldParent) };
                if name_at == 0 {
                    return Err(Why::Name);
                }
                if name_at as usize >= sdata.len() {
                    return Err(Why::String);
                }
                if name_at as usize >= split || (name_at as usize) < names.position() {
                    return Err(Why::StringOrder);
                }
                let name = names.string_at(name_at as usize)?;
                if name.len() > MAX_REL_PATH {
                    return Err(Why::PathLength);
                }
                if !valid_name(name) {
                    return Err(Why::Name);
                }
                let at = if up.path.is_empty() { 0 } else { up.path.len() + 1 };
                if at + name.len() > MAX_REL_PATH || stack.len() > MAX_DEPTH {
                    return Err(Why::PathLength);
                }
                if let Some(p) = &prev
                    && compare_names(name, p.path[p.name_at..].as_bytes()).is_le()
                {
                    return Err(Why::FoldOrder);
                }
                let name = utf8(name, Why::Name)?;
                let path = if up.path.is_empty() { name.to_owned() } else { format!("{}/{name}", up.path) };
                let hash = child_hash(up.hash, name);
                stack.push(Level { index: i, hash, path, name_at: at });
            }
            if first_record != records.len() as u32 {
                return Err(Why::FirstRecord);
            }
            let top = stack.last().ok_or(Why::FoldParent)?;
            folders.push(Folder { path: top.path.clone(), parent, flags, first_record, hash: top.hash });
            cur_folder = Some(i);
            next_folder += 1;
            continue;
        }
        if look.is_some() {
            return Err(Why::RecsOrder);
        }
        break;
    }

    // The sections' CRCs, then HIDX against the records.
    if !c.crc_ok(&fold) || !c.crc_ok(&recs) || !c.crc_ok(&strs) {
        return Err(Why::SectionCrc);
    }
    if let Some(h) = hidx {
        let mut prev: Option<(u64, u32)> = None;
        let mut entries: Vec<(u64, u32)> = Vec::with_capacity(h.count as usize);
        for i in 0..h.count {
            let e = c.row(&h, i, HIDX_STRIDE as usize);
            let pair = (u64_at(e, 0), u32_at(e, 8));
            if pair.1 >= record_count || prev.is_some_and(|p| pair <= p) {
                return Err(Why::Hidx);
            }
            entries.push(pair);
            prev = Some(pair);
        }
        if !c.crc_ok(&h) {
            return Err(Why::SectionCrc);
        }
        let mut own: Vec<(u64, u32)> = records.iter().enumerate().map(|(i, e)| (e.hash, i as u32)).collect();
        own.sort_unstable();
        if own != entries {
            return Err(Why::Hidx);
        }
    }
    if let Some((o, s)) = ledger
        && (!c.crc_ok(&o) || !c.crc_ok(&s))
    {
        return Err(Why::SectionCrc);
    }
    Ok(Tags {
        header: *c.header(),
        source,
        parser_version,
        read_rules,
        album_values,
        artist_values,
        producer,
        folders,
        records,
        has_ledger: ledger.is_some(),
    })
}

/// A record's run at `at` (2.6.5): its field count n, then n strings; the
/// fields after the twelve readRules 1 knows are a later minor's, read past.
/// A field longer than its limit is cut (readers MAY), at a code point.
fn read_run(runs: &mut StrCursor, sdata: &[u8], at: usize) -> Result<Run, Why> {
    if at >= sdata.len() {
        return Err(Why::String);
    }
    if at < runs.position() {
        return Err(Why::StringOrder);
    }
    let n = runs.byte_at(at).ok_or(Why::String)?;
    let mut run = Run::default();
    for f in 0..n as usize {
        let value = runs.next_string()?;
        if f < RUN_FIELDS {
            run.fields[f] = cut_utf8(value, field_max(f)).to_vec();
        }
    }
    Ok(run)
}

/// What a tags file's writer is given besides the records: every value a
/// writer would otherwise choose comes from its caller (2.4.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub(crate) source: u8,
    pub(crate) parser_version: u16,
    pub(crate) read_rules: u16,
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) minor: u16,
    pub(crate) producer: String,
}

/// A folder the table must hold besides every record's ancestors: an OWNED
/// one (kept even empty, with its ancestors, so the software can remove it
/// later), or a THUMB album folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FolderIn {
    pub(crate) path: String,
    pub(crate) flags: u32,
}

/// One file to record: its path relative to `/music`, as stored, its
/// record, its run (2.3.6 applied: [`Run::build`]) and its ledger row (none
/// for the device's own file; a zeroed row for the software's, if none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RecordIn {
    pub(crate) path: String,
    pub(crate) rec: Record,
    pub(crate) run: Run,
    pub(crate) ledger: Option<Ledger>,
}

/// Writes a tags file in 2.6.8's order, whatever order the records and
/// folders come in: FOLD in pre-order from `/music`, siblings by their
/// names' bytes; RECS by folder, then name; HIDX by (hash, record); STRS
/// the producer, the folder names, then each record's name and run; ORIG
/// and OSTR a row a record, for source 2 or 3 with records.
///
/// Refused: a source that isn't 1-3; a record path that isn't a relative
/// path of names within 255 bytes with `/music/`, or one given twice, or a
/// path both a file and a folder; a folder that is neither OWNED nor a
/// record's ancestor; a run that breaks 2.3.6; a ledger row in a source 1
/// file; a producer or a server path with a NUL in it.
pub(crate) fn write(meta: &Meta, records: &[RecordIn], folders: &[FolderIn]) -> Result<Vec<u8>, WriteError> {
    if !(SOURCE_DEVICE..=SOURCE_SYNC_AGENT).contains(&meta.source) {
        return Err(WriteError("source"));
    }
    if records.len() > 0x7FFF_FFFF {
        return Err(WriteError("too many records"));
    }
    fn ancestors(path: &str, set: &mut HashMap<String, u32>) {
        let mut a = split_last(path).0;
        while !a.is_empty() {
            set.entry(a.to_owned()).or_insert(0);
            a = split_last(a).0;
        }
    }
    let mut fset: HashMap<String, u32> = HashMap::new();
    fset.insert(String::new(), 0);
    let mut paths: HashSet<&str> = HashSet::with_capacity(records.len());
    for r in records {
        if !valid_rel_path(r.path.as_bytes()) {
            return Err(WriteError("a record's path"));
        }
        if !paths.insert(&r.path) {
            return Err(WriteError("a record's path twice"));
        }
        ancestors(&r.path, &mut fset);
    }
    for f in folders {
        if !f.path.is_empty() && !valid_rel_path(f.path.as_bytes()) {
            return Err(WriteError("a folder's path"));
        }
        if !fset.contains_key(&f.path) && f.flags & FOLDER_OWNED == 0 {
            return Err(WriteError("a folder neither OWNED nor a record's ancestor"));
        }
    }
    for f in folders {
        *fset.entry(f.path.clone()).or_insert(0) |= f.flags;
        ancestors(&f.path, &mut fset);
    }
    if paths.iter().any(|p| fset.contains_key(*p)) {
        return Err(WriteError("a path both a file and a folder"));
    }
    if records.iter().any(|r| !r.run.conforms()) {
        return Err(WriteError("a run that breaks 2.3.6"));
    }
    if meta.source == SOURCE_DEVICE && records.iter().any(|r| r.ledger.is_some()) {
        return Err(WriteError("a ledger row in a source 1 file"));
    }
    let nul = |s: &str| s.contains('\0');
    if nul(&meta.producer) || records.iter().any(|r| r.ledger.as_ref().is_some_and(|l| nul(&l.server_path))) {
        return Err(WriteError("a string with a NUL"));
    }

    let mut fpaths: Vec<&str> = fset.keys().map(String::as_str).collect();
    fpaths.sort_by(|a, b| compare_folder_paths(a, b));
    let findex: HashMap<&str, u32> = fpaths.iter().enumerate().map(|(i, p)| (*p, i as u32)).collect();
    let mut order: Vec<(u32, &str, &RecordIn)> =
        records.iter().map(|r| (findex[split_last(&r.path).0], split_last(&r.path).1, r)).collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| compare_names(a.1.as_bytes(), b.1.as_bytes())));

    let mut strs = Strs::new();
    let producer = strs.add(meta.producer.as_bytes());
    let mut first = vec![0u32; fpaths.len() + 1];
    for (f, _, _) in &order {
        first[*f as usize + 1] += 1;
    }
    for i in 1..first.len() {
        first[i] += first[i - 1];
    }
    let mut fold = Vec::with_capacity(fpaths.len() * FOLD_STRIDE as usize);
    for (i, p) in fpaths.iter().enumerate() {
        let (parent, name) = if i == 0 {
            (NO_PARENT, 0)
        } else {
            let (up, name) = split_last(p);
            (findex[up], strs.add(name.as_bytes()))
        };
        for v in [parent, name, fset[*p], first[i]] {
            fold.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut rows = Vec::with_capacity(order.len() * RECS_STRIDE as usize);
    let mut hidx: Vec<(u64, u32)> = Vec::with_capacity(order.len());
    let mut albums: HashSet<&[u8]> = HashSet::new();
    let mut artists: HashSet<&[u8]> = HashSet::new();
    for (k, (folder, name, r)) in order.iter().enumerate() {
        let name = strs.add(name.as_bytes());
        let run = r.run.encode();
        let strings = if run.is_empty() { 0 } else { strs.raw(&run) };
        rows.extend_from_slice(&r.rec.encode(*folder, name, strings));
        hidx.push((path_hash(&r.path), k as u32));
        if !r.run.get(ALBUM).is_empty() {
            albums.insert(r.run.get(ALBUM));
        }
        for f in [ARTIST, ALBUM_ARTIST] {
            if !r.run.get(f).is_empty() {
                artists.extend(r.run.get(f).split(|&c| c == SEPARATOR));
            }
        }
    }
    if strs.len() > u32::MAX as usize {
        return Err(WriteError("the strings would pass 4 GiB"));
    }
    hidx.sort_unstable();
    let mut hidx_bytes = Vec::with_capacity(hidx.len() * HIDX_STRIDE as usize);
    for (h, k) in &hidx {
        hidx_bytes.extend_from_slice(&h.to_le_bytes());
        hidx_bytes.extend_from_slice(&k.to_le_bytes());
    }
    let ledger = meta.source != SOURCE_DEVICE && !order.is_empty();
    let mut ostr = Strs::new();
    let mut orig = Vec::new();
    if ledger {
        let none = Ledger::default();
        for (_, _, r) in &order {
            let l = r.ledger.as_ref().unwrap_or(&none);
            orig.extend_from_slice(&l.encode(ostr.add(l.server_path.as_bytes())));
        }
    }

    let mut th = Vec::with_capacity((HEADER_BYTES - 40) as usize);
    th.extend_from_slice(&[meta.source, 0]);
    th.extend_from_slice(&meta.parser_version.to_le_bytes());
    th.extend_from_slice(&meta.read_rules.to_le_bytes());
    th.extend_from_slice(&[0, 0]);
    let (records, folders) = (order.len() as u32, fpaths.len() as u32);
    for v in [records, folders, albums.len() as u32, artists.len() as u32, producer, 0] {
        th.extend_from_slice(&v.to_le_bytes());
    }
    let n = records;
    let mut sections = vec![
        SectionOut { kind: FOLD, flags: REQUIRED, count: folders, stride: FOLD_STRIDE, data: &fold },
        SectionOut { kind: RECS, flags: REQUIRED, count: n, stride: RECS_STRIDE, data: &rows },
        SectionOut { kind: STRS, flags: REQUIRED, count: 0, stride: 0, data: strs.bytes() },
    ];
    if n > 0 {
        sections.push(SectionOut { kind: HIDX, flags: 0, count: n, stride: HIDX_STRIDE, data: &hidx_bytes });
    }
    if ledger {
        sections.push(SectionOut { kind: ORIG, flags: 0, count: n, stride: ORIG_STRIDE, data: &orig });
        sections.push(SectionOut { kind: OSTR, flags: 0, count: 0, stride: 0, data: ostr.bytes() });
    }
    let (minor, generation, card_id) = (meta.minor, meta.generation, meta.card_id);
    let file = FileMeta { magic: MPTG, major: MAJOR, minor, generation, card_id };
    container::write(&file, &th, HEADER_BYTES, &sections)
}
