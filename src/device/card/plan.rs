//! A sync's wanted card (the transfer plan's 2.3, steps 5-6; 2.5; 2.9): the
//! user's own server's selection as the engine is given it — each file of
//! the selected libraries with its server's ledger fields, what its bytes
//! will be on the card and their size — and whether the plan the engine
//! makes of it fits the card. Pure code: the manifest, the metadata and the
//! card's settings come in, nothing is fetched or written.
//!
//! **As it is, or converted** ([`decide`]). A file goes on the card byte for
//! byte when the card's player plays it: a codec `device.txt` lists, under
//! an extension it lists (an Opus stream stored as `.ogg` or `.oga` is
//! copied and named `.opus`), at no more than its `max_rate` and
//! `max_channels`, and under FAT32's 4 GiB. Anything else is converted by
//! the server's `/transcode`, and so is everything under the card's
//! "convert everything" setting, but a file already in the codec at no more
//! than the bitrate (section 6's default: only what the player can't play,
//! to Opus 192k). A conversion keeps every channel its source has in Opus,
//! so a source of more channels than the player plays is converted to MP3,
//! whose encoder takes two at most (ESTIMATED: ffmpeg's own downmix). A
//! conversion's size is an estimate, duration × kbps ÷ 8 × 1.02, until its
//! bytes are there.
//!
//! **What a sync knows already.** A file whose ledger row is unchanged on
//! the server keeps the row's conversion when the settings it was planned
//! under haven't changed ([`Kept`]), so only new and changed files need the
//! full metadata (`metadata/batch`: sample rate, channels, `bpm-source`).
//! After a 304, the ledger itself is the wanted card ([`wanted_of_ledger`]).
//!
//! **The fit** ([`fit`]): what the run adds — each file rounded up to the
//! volume's cluster, its directory entries, the next tags file and Auto DJ
//! index beside the ones there, a thumbnail a new album, the stage file in
//! flight, the player's room for its library and its play log — against
//! the free space and what the run takes off, less a margin of 1% of the
//! card (at least 256 MB); and the 20,000-track cap, past which the player
//! shows no library at all.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::contract::device_txt::DeviceTxt;
use super::contract::mptg::{BPM_ANALYSED, CONTAINER_MP3, CONTAINER_OPUS, Ledger};
use super::contract::numbers;
use super::contract::order::split_last;
use super::contract::prim::hex16;
use super::engine::{Footprint, Origin, WantedCard, WantedFile};
use super::names::{self, Bytes};
use super::state::{Conversion, Convert};
use super::tags;
use super::volume::Space;
use crate::api::transfer::{ManifestEntry, Metadata};
use crate::api::types::TranscodeInfo;
use crate::api::urls::{TransferBitrate, TransferCodec};

/// The most tracks a card holds: 19,410 measured working on the player, and
/// past its memory's ceiling (about 23-27k, ESTIMATED) it shows no library.
pub(crate) const TRACK_CAP: usize = 20_000;
/// A conversion's size over its bitrate's: the container's own bytes.
const CONVERSION_OVERHEAD: f64 = 1.02;

// ── A file of the selection ─────────────────────────────────────────────────

/// What a decision about a server file reads of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Its extension, lowercase.
    pub(crate) ext: String,
    pub(crate) size: u64,
    pub(crate) sample_rate: Option<u32>,
    pub(crate) channels: Option<u16>,
    /// Bits per second.
    pub(crate) bitrate: Option<u64>,
    /// For an `.ogg` or `.oga`: whether its stream is Opus (its first page's
    /// packet says `OpusHead`); None when not looked at.
    pub(crate) ogg_opus: Option<bool>,
}

/// The extension of a server path, lowercase.
pub(crate) fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

/// An `.ogg` or `.oga`, whose codec only its bytes say.
pub(crate) fn is_ogg(path: &str) -> bool {
    matches!(extension(path).as_str(), "ogg" | "oga")
}

/// An Ogg stream's first bytes say Opus: the first page's first packet is
/// an `OpusHead`.
pub(crate) fn ogg_is_opus(head: &[u8]) -> bool {
    if head.len() < 27 || &head[..4] != b"OggS" {
        return false;
    }
    let body = 27 + head[26] as usize;
    head.get(body..body + 8).is_some_and(|p| p == b"OpusHead")
}

impl Facts {
    /// A manifest entry's, with its full metadata when it was read.
    pub(crate) fn of(entry: &ManifestEntry, meta: Option<&Metadata>, ogg: Option<bool>) -> Facts {
        let size = entry.file_size.or(meta.and_then(|m| m.file_size)).unwrap_or(0);
        Facts {
            ext: extension(&entry.filepath),
            size,
            sample_rate: meta.and_then(|m| m.sample_rate),
            channels: meta.and_then(|m| m.channels),
            bitrate: meta.and_then(|m| m.bitrate),
            ogg_opus: ogg,
        }
    }
}

/// Why a file of the selection can't go on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Skip {
    /// It needs a conversion, and the player plays neither codec one makes.
    NoCodec,
    /// It needs a conversion, and the server has no ffmpeg to make one.
    NoTranscode,
    /// It, or its conversion, is 4 GiB or more: past FAT32's largest file.
    TooBig,
}

impl Skip {
    /// The reason as one stable word, for the debug command and the tests.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Skip::NoCodec => "no-codec",
            Skip::NoTranscode => "no-transcode",
            Skip::TooBig => "too-big",
        }
    }
}

/// What a file of the selection is on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The server's bytes, as they are.
    Original { ogg_opus: bool },
    Convert(TransferCodec, TransferBitrate),
    Skip(Skip),
}

/// The codec the player decodes a server file by, from its extension, and
/// the extension it takes on the card; None for a file the player can't
/// play as it is.
fn native(f: &Facts) -> Option<(&'static str, &'static str)> {
    match f.ext.as_str() {
        "mp3" => Some(("mp3", "mp3")),
        "flac" => Some(("flac", "flac")),
        "opus" => Some(("opus", "opus")),
        "ogg" | "oga" if f.ogg_opus == Some(true) => Some(("opus", "opus")),
        _ => None,
    }
}

/// See the module's note: as it is, or converted to what, or not at all.
/// `transcode`: what the server converts, when it does (its `/api/`'s
/// transcode block): a conversion is made at the setting's bitrate when the
/// server takes it for that codec, else at the highest below it that it
/// does (MP3's 320k is 192k on a server that lists no such rate).
pub(crate) fn decide(
    f: &Facts,
    device: &DeviceTxt,
    c: &Conversion,
    transcode: Option<&TranscodeInfo>,
) -> Decision {
    let rate = f.sample_rate.is_none_or(|r| r <= device.max_rate);
    let channels = f.channels.is_none_or(|n| n as u32 <= device.max_channels);
    let codec = native(f).filter(|(codec, ext)| device.decodes(codec) && device.lists(ext));
    let plays = codec.is_some() && rate && channels && names::fits_fat32(f.size);
    let ogg_opus = matches!(f.ext.as_str(), "ogg" | "oga") && f.ogg_opus == Some(true);
    let keep = match c.convert {
        Convert::Unplayable => plays,
        Convert::Everything => {
            let same = codec.is_some_and(|(codec, _)| codec == c.codec.as_str());
            let within = f.bitrate.is_some_and(|b| b <= c.bitrate.kbps() as u64 * 1000);
            plays && same && within
        }
    };
    if keep {
        return Decision::Original { ogg_opus };
    }
    let card_plays = |k: TransferCodec| device.decodes(k.as_str()) && device.lists(k.extension());
    let many = !channels;
    let codec = if card_plays(c.codec) && !(many && c.codec == TransferCodec::Opus) {
        c.codec
    } else if card_plays(TransferCodec::Mp3) {
        TransferCodec::Mp3
    } else {
        return Decision::Skip(Skip::NoCodec);
    };
    match transcode {
        Some(server) => Decision::Convert(codec, server.best(codec, c.bitrate)),
        None => Decision::Skip(Skip::NoTranscode),
    }
}

/// A conversion's size before its bytes are there: its length × its
/// bitrate, and the container's few percent. With no length, the source's
/// bytes at its bitrate's share, else the source's size.
pub(crate) fn estimate(duration_ms: Option<u64>, f: &Facts, kbps: u32) -> u64 {
    let bytes_per_s = kbps as f64 * 1000.0 / 8.0 * CONVERSION_OVERHEAD;
    if let Some(ms) = duration_ms.filter(|&ms| ms > 0) {
        return (ms as f64 / 1000.0 * bytes_per_s).ceil() as u64;
    }
    match f.bitrate.filter(|&b| b > 0) {
        Some(b) => {
            let share = kbps as f64 * 1000.0 / b as f64;
            (f.size as f64 * share * CONVERSION_OVERHEAD).ceil() as u64
        }
        None => f.size,
    }
}

/// The container code a codec's conversion is (2.6.4).
pub(crate) fn container(codec: TransferCodec) -> u8 {
    match codec {
        TransferCodec::Opus => CONTAINER_OPUS,
        TransferCodec::Mp3 => CONTAINER_MP3,
    }
}

/// The codec a container code was converted to.
pub(crate) fn codec_of(container: u8) -> Option<TransferCodec> {
    match container {
        CONTAINER_OPUS => Some(TransferCodec::Opus),
        CONTAINER_MP3 => Some(TransferCodec::Mp3),
        _ => None,
    }
}

/// mStream's `created-at` (SQLite's `YYYY-MM-DD HH:MM:SS`, UTC; an ISO
/// `T` and a trailing `Z` or fraction taken too) as Unix seconds; None for
/// anything else, or a time ORIG can't hold.
pub(crate) fn unix_seconds(text: &str) -> Option<u32> {
    let t = text.trim();
    let digits = |s: &&str| s.bytes().all(|c| c.is_ascii_digit());
    let num = |a: usize, b: usize| t.get(a..b).filter(digits)?.parse::<i64>().ok();
    let sep = t.as_bytes();
    if sep.len() < 19 || sep[4] != b'-' || sep[7] != b'-' || !matches!(sep[10], b' ' | b'T') {
        return None;
    }
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u32::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A manifest entry's ORIG fields (2.6.6): an id of 2^32 or more, or below
/// 0, is 0; a hash that isn't 32 hex digits is none.
pub(crate) fn ledger_of(entry: &ManifestEntry) -> Ledger {
    let id = |n: Option<i64>| n.and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
    let digest = |h: &Option<String>| h.as_deref().and_then(hex16).unwrap_or([0; 16]);
    Ledger {
        server_path: entry.filepath.clone(),
        mstream_id: id(Some(entry.id)),
        album_id: id(entry.album_id),
        artist_id: id(entry.artist_id),
        audio_hash: digest(&entry.audio_hash),
        file_hash: digest(&entry.hash),
        server_modified: entry.modified.and_then(|m| u64::try_from(m).ok()).unwrap_or(0),
        server_size: entry.file_size.unwrap_or(0),
        created_at: entry.created_at.as_deref().and_then(unix_seconds).unwrap_or(0),
        hash_v: entry.hash_v.and_then(|v| u16::try_from(v).ok()).unwrap_or(0),
        ..Ledger::default()
    }
}

/// A BPM as mStream gives it (a number) as the tag text a file would carry:
/// whole, or with its fraction.
fn bpm_text(bpm: f64) -> String {
    if bpm.fract() == 0.0 { format!("{bpm:.0}") } else { format!("{bpm}") }
}

/// What the sync knows of a file's source beside its ledger fields, for
/// its conversion's checks and retag, its cover and its Auto DJ row.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Extra {
    /// The source's length, which a conversion must have within a second.
    pub(crate) duration_ms: Option<u64>,
    /// mStream's cover for it: a cache file name.
    pub(crate) album_art: Option<String>,
    /// The source's own BPM and key, as its tags carry them (`bpm-source`
    /// `tag`): what a conversion's retag puts back. mStream's analysis is
    /// never written into a file.
    pub(crate) bpm_tag: Option<String>,
    pub(crate) key_tag: Option<String>,
    /// The full metadata was read: `bpm-source` is known.
    pub(crate) full: bool,
    /// The server said what it has of this file this run (its manifest
    /// entry, or its full metadata): `album_art` is what it has now. After
    /// a 304 only the ledger speaks, which names no art.
    pub(crate) listed: bool,
}

impl Extra {
    /// From the manifest's lite metadata, and the full one when read.
    pub(crate) fn of(lite: &Metadata, full: Option<&Metadata>) -> Extra {
        let mut extra = Extra {
            duration_ms: lite.duration.filter(|d| *d > 0.0).map(|d| (d * 1000.0).round() as u64),
            album_art: lite.album_art.clone().filter(|a| !a.is_empty()),
            listed: true,
            ..Extra::default()
        };
        if let Some(m) = full {
            extra.fill(m);
        }
        extra
    }

    /// The full metadata, read later.
    pub(crate) fn fill(&mut self, m: &Metadata) {
        let duration = m.duration.filter(|d| *d > 0.0).map(|d| (d * 1000.0).round() as u64);
        self.duration_ms = self.duration_ms.or(duration);
        let art = m.album_art.clone().filter(|a| !a.is_empty());
        self.album_art = self.album_art.take().or(art);
        if m.bpm_source.as_deref() == Some("tag") {
            self.bpm_tag = m.bpm.filter(|b| *b > 0.0).map(bpm_text);
            self.key_tag = m.musical_key.clone().filter(|k| !k.trim().is_empty());
        }
        (self.full, self.listed) = (true, true);
    }
}

/// mStream's analysis as 2.6.4 records it, for a file whose own tags have
/// neither (BPM_ANALYSED): bpm10 and camelot.
fn analysed(m: &Metadata) -> (u16, u8) {
    let bpm10 = m.bpm.and_then(|b| numbers::bpm10(&bpm_text(b))).unwrap_or(0);
    (bpm10, m.musical_key.as_deref().map_or(0, tags::camelot))
}

/// A ledger row and its record, as the card's root holds them: what a sync
/// reuses for an entry the server hasn't changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Kept {
    pub(crate) ledger: Ledger,
    /// The card file's bytes and length.
    pub(crate) size: u64,
    pub(crate) duration_ms: u32,
    pub(crate) bpm10: u16,
    pub(crate) camelot: u8,
    /// bpm10 and camelot are mStream's analysis (BPM_ANALYSED).
    pub(crate) analysed: bool,
}

impl Kept {
    pub(crate) fn of(entry: &super::contract::mptg::Entry) -> Option<Kept> {
        let ledger = entry.ledger.clone()?;
        Some(Kept {
            ledger,
            size: entry.rec.size as u64,
            duration_ms: entry.rec.duration_ms,
            bpm10: entry.rec.bpm10,
            camelot: entry.rec.camelot,
            analysed: entry.rec.flags & BPM_ANALYSED != 0,
        })
    }

    /// The entry is the row's file still (2.10.2): its audio hash, size,
    /// `modified`, and file hash when both have one.
    pub(crate) fn unchanged(&self, entry: &ManifestEntry) -> bool {
        let l = ledger_of(entry);
        let none = [0; 16];
        let (theirs, ours) = (l.file_hash, self.ledger.file_hash);
        let file = theirs == ours || theirs == none || ours == none;
        l.audio_hash == self.ledger.audio_hash
            && l.server_size == self.ledger.server_size
            && l.server_modified == self.ledger.server_modified
            && file
    }

    /// What the row's file is on the card.
    fn bytes(&self) -> Bytes {
        match self.ledger.converted_to {
            0 => Bytes::Original { ogg_opus: is_ogg(&self.ledger.server_path) },
            container => Bytes::Converted(container),
        }
    }
}

/// The wanted card, and what the sync keeps beside it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Wanted {
    pub(crate) card: WantedCard,
    /// Each file's, by its index in `card.files`.
    pub(crate) extras: Vec<Extra>,
    /// The files of the selection that can't go on the card: server path,
    /// why.
    pub(crate) skipped: Vec<(String, Skip)>,
}

/// The wanted card of a manifest's entries (one origin, 0): `metadata` is
/// the full metadata of each entry read (an entry it lacks needs none: a
/// row of `kept` stands for it, or its decision is made on what the
/// manifest says), `ogg` what each `.ogg`'s bytes said, `kept` the card's
/// rows by server path, reused for an entry they still describe when
/// `same_plan` (the settings it was planned under are these).
#[allow(clippy::too_many_arguments)]
pub(crate) fn wanted_of(
    origin: Origin,
    entries: &[ManifestEntry],
    metadata: &HashMap<String, Option<Metadata>>,
    ogg: &HashMap<String, bool>,
    kept: &HashMap<String, Kept>,
    same_plan: bool,
    device: &DeviceTxt,
    conversion: &Conversion,
    transcode: Option<&TranscodeInfo>,
) -> Wanted {
    let card = WantedCard { origins: vec![origin], files: Vec::new() };
    let mut out = Wanted { card, ..Wanted::default() };
    for entry in entries {
        let path = &entry.filepath;
        let full = metadata.get(path).and_then(Option::as_ref);
        let ledger = ledger_of(entry);
        let (bpm10, camelot) = analysed(full.unwrap_or(&entry.metadata));
        let extra = Extra::of(&entry.metadata, full);
        let reuse = kept.get(path).filter(|k| same_plan && k.unchanged(entry));
        let (bytes, kbps, size) = match reuse {
            Some(k) if k.ledger.converted_to == 0 => (k.bytes(), 0, ledger.server_size),
            Some(k) => (k.bytes(), k.ledger.convert_kbps, k.size),
            None => {
                let facts = Facts::of(entry, full, ogg.get(path).copied());
                match decide(&facts, device, conversion, transcode) {
                    Decision::Original { ogg_opus } => {
                        (Bytes::Original { ogg_opus }, 0, ledger.server_size)
                    }
                    Decision::Convert(codec, bitrate) => {
                        let size = estimate(extra.duration_ms, &facts, bitrate.kbps());
                        if !names::fits_fat32(size) {
                            out.skipped.push((path.clone(), Skip::TooBig));
                            continue;
                        }
                        (Bytes::Converted(container(codec)), bitrate.kbps() as u16, size)
                    }
                    Decision::Skip(why) => {
                        out.skipped.push((path.clone(), why));
                        continue;
                    }
                }
            }
        };
        let file = WantedFile { origin: 0, ledger, bytes, kbps, size, bpm10, camelot };
        out.card.files.push(file);
        out.extras.push(extra);
    }
    out
}

/// The wanted card after a 304: the server's files are, probably, the ones
/// the card's ledger lists, so each row stands for its entry — its ledger
/// fields, its conversion, its length, and mStream's analysis it recorded.
pub(crate) fn wanted_of_ledger(origin: Origin, rows: &[Kept]) -> Wanted {
    let card = WantedCard { origins: vec![origin], files: Vec::new() };
    let mut out = Wanted { card, ..Wanted::default() };
    for k in rows.iter().filter(|k| !k.ledger.server_path.is_empty()) {
        let original = k.ledger.converted_to == 0;
        let (bpm10, camelot) = if k.analysed { (k.bpm10, k.camelot) } else { (0, 0) };
        let ledger =
            Ledger { origin_flags: 0, converted_to: 0, convert_kbps: 0, server: 0, ..k.ledger.clone() };
        out.card.files.push(WantedFile {
            origin: 0,
            ledger,
            bytes: k.bytes(),
            kbps: if original { 0 } else { k.ledger.convert_kbps },
            size: if original { k.ledger.server_size } else { k.size },
            bpm10,
            camelot,
        });
        let duration_ms = (k.duration_ms > 0).then_some(k.duration_ms as u64);
        out.extras.push(Extra { duration_ms, ..Extra::default() });
    }
    out
}

/// A row of the card stands for the entry: unchanged, under the same plan.
fn reuses(kept: &HashMap<String, Kept>, same_plan: bool, e: &ManifestEntry) -> bool {
    same_plan && kept.get(&e.filepath).is_some_and(|k| k.unchanged(e))
}

/// The `.ogg` and `.oga` entries whose codec must be read from their bytes
/// before a decision: those no reused row stands for.
pub(crate) fn oggs_to_sniff<'e>(
    entries: &'e [ManifestEntry],
    kept: &HashMap<String, Kept>,
    same_plan: bool,
) -> Vec<&'e ManifestEntry> {
    let reused = |e: &ManifestEntry| reuses(kept, same_plan, e);
    entries.iter().filter(|e| is_ogg(&e.filepath) && !reused(e)).collect()
}

/// The entries that need their full metadata for a decision.
pub(crate) fn needing_metadata(
    entries: &[ManifestEntry],
    kept: &HashMap<String, Kept>,
    same_plan: bool,
) -> Vec<String> {
    let reused = |e: &ManifestEntry| reuses(kept, same_plan, e);
    entries.iter().filter(|e| !reused(e)).map(|e| e.filepath.clone()).collect()
}

// ── The fit ─────────────────────────────────────────────────────────────────

/// A tags file's bytes a track, and an Auto DJ index's (ESTIMATED, from
/// docs/METADATA.md's sizes).
const TAGS_PER_TRACK: u64 = 345;
const MPDJ_PER_TRACK: u64 = 530;
/// The player's own room on the card: its library a track, a megabyte, and
/// its play log's cap (2.20.2).
const PLAYER_PER_TRACK: u64 = 330;
const PLAYER_FIXED: u64 = (1 << 20) + 1_048_480;
/// The margin the card keeps: 1% of it, at least this.
const MARGIN_MIN: u64 = 256 << 20;

/// Whether a run fits the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fit {
    /// The bytes the run needs, its margin included.
    pub(crate) needed: u64,
    /// The free space, and what the run takes off the card.
    pub(crate) room: u64,
    /// The card's tracks after the run.
    pub(crate) tracks: usize,
}

impl Fit {
    pub(crate) fn fits(&self) -> bool {
        self.needed <= self.room
    }

    /// By how much it doesn't.
    pub(crate) fn short(&self) -> u64 {
        self.needed.saturating_sub(self.room)
    }

    pub(crate) fn over_cap(&self) -> bool {
        self.tracks > TRACK_CAP
    }
}

/// See the module's note. A run that fetches nothing needs nothing new: its
/// commit takes the place of the last one.
pub(crate) fn fit(space: &Space, fp: &Footprint, auto_dj: bool) -> Fit {
    let cluster = (space.cluster as u64).max(512);
    let up = |b: u64| b.div_ceil(cluster) * cluster;
    let room = space.free + fp.freed.iter().map(|&b| up(b)).sum::<u64>();
    if fp.writes.is_empty() {
        return Fit { needed: 0, room, tracks: fp.records };
    }
    let music: u64 = fp.writes.iter().map(|(_, size)| up(*size)).sum();
    // Directory entries: each new name in its folder, every folder's growth
    // a cluster at a time, and a cluster of its own for each new folder.
    let mut entries: BTreeMap<&str, u64> = BTreeMap::new();
    let named = fp.writes.iter().map(|(p, _)| p.as_str()).chain(fp.folders.iter().map(String::as_str));
    for path in named {
        let (dir, name) = split_last(path);
        *entries.entry(dir).or_default() += names::dir_entries(name) as u64 * 32;
    }
    let dirs = entries.values().map(|&b| up(b)).sum::<u64>() + fp.folders.len() as u64 * cluster;
    let tracks = fp.records as u64;
    let mut mstream = TAGS_PER_TRACK * tracks;
    if auto_dj {
        mstream += MPDJ_PER_TRACK * tracks;
    }
    let albums: BTreeSet<&str> = fp.writes.iter().map(|(p, _)| split_last(p).0).collect();
    mstream += albums.len() as u64 * cluster;
    mstream += fp.writes.iter().map(|(_, size)| up(*size)).max().unwrap_or(0);
    let added = fp.records.saturating_sub(fp.records_before) as u64;
    let player = PLAYER_PER_TRACK * added + if fp.records_before == 0 { PLAYER_FIXED } else { 0 };
    let margin = (space.size / 100).max(MARGIN_MIN);
    Fit { needed: music + dirs + mstream + player + margin, room, tracks: fp.records }
}

/// What a track takes of a card beside its bytes: the next tags file's
/// record and Auto DJ row beside the ones there, the player's room for it,
/// and its name's directory entries (about a long name's). A multi-card
/// set's split and its offers (B7) count a card's load by it.
const TRACK_EXTRA: u64 = 2 * TAGS_PER_TRACK + 2 * MPDJ_PER_TRACK + PLAYER_PER_TRACK + 96;

/// A track of `size` bytes on a card of `cluster`-byte clusters, its
/// overheads with it.
pub(crate) fn track_cost(size: u64, cluster: u32) -> u64 {
    let cluster = (cluster as u64).max(512);
    size.div_ceil(cluster) * cluster + TRACK_EXTRA
}

/// An album folder's own: its folder's cluster and its thumbnail's.
pub(crate) fn album_cost(cluster: u32) -> u64 {
    2 * (cluster as u64).max(512)
}

/// The room a card has for the transfer's files, as a set counts it: its
/// size less the margin, and less what the rest of the card takes beside
/// `ours`, the load of the files its ledger lists now ([`track_cost`]) — the
/// player's own room at least (its library, its play log, there once it has
/// booted with music), the listener's files, the file system's own.
pub(crate) fn usable_room(space: &Space, ours: u64) -> u64 {
    let margin = (space.size / 100).max(MARGIN_MIN);
    let theirs = space.size.saturating_sub(space.free).saturating_sub(ours);
    space.size.saturating_sub(margin + theirs.max(PLAYER_FIXED))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::card::contract::server::{NO_INSTANCE, Server, Talk};

    fn device(codecs: &str) -> DeviceTxt {
        let mut d = DeviceTxt::defaults();
        (d.codecs, d.extensions, d.present) = (codecs.into(), codecs.into(), true);
        d
    }

    fn facts(ext: &str, rate: u32, channels: u16, kbps: u64) -> Facts {
        Facts {
            ext: ext.into(),
            size: 30 << 20,
            sample_rate: Some(rate),
            channels: Some(channels),
            bitrate: Some(kbps * 1000),
            ogg_opus: None,
        }
    }

    #[test]
    fn a_file_goes_as_it_is_only_when_the_player_plays_it() {
        let card = device("mp3,flac,opus");
        let unplayable = Conversion::default();
        // A server that converts, older than the card transfer's batch of
        // mStream changes: no list of rates.
        let old = TranscodeInfo::default();
        let opus192 = Decision::Convert(TransferCodec::Opus, TransferBitrate::K192);
        let original = Decision::Original { ogg_opus: false };
        let cases = [
            (facts("flac", 44_100, 2, 900), original),
            (facts("mp3", 48_000, 1, 320), original),
            (facts("opus", 48_000, 2, 96), original),
            (facts("flac", 96_000, 2, 3000), opus192),
            (facts("wav", 44_100, 2, 1411), opus192),
            (facts("m4a", 44_100, 2, 256), opus192),
            (facts("ogg", 44_100, 2, 160), opus192),
            // Opus would keep the six channels: MP3's encoder takes two.
            (facts("flac", 48_000, 6, 4000), Decision::Convert(TransferCodec::Mp3, TransferBitrate::K192)),
        ];
        for (f, want) in cases {
            assert_eq!(decide(&f, &card, &unplayable, Some(&old)), want, "{f:?}");
        }
        // An Ogg whose bytes are Opus: copied, and named .opus.
        let ogg_opus = Facts { ogg_opus: Some(true), ..facts("ogg", 48_000, 2, 128) };
        assert_eq!(decide(&ogg_opus, &card, &unplayable, Some(&old)), Decision::Original { ogg_opus: true });
        // 4 GiB or more is past FAT32.
        let huge = Facts { size: 1 << 32, ..facts("flac", 44_100, 2, 900) };
        assert_eq!(decide(&huge, &card, &unplayable, Some(&old)), opus192);
        // A player that reads no Opus gets MP3; no server ffmpeg, no
        // conversion; a player of neither, none at all.
        let v1 = device("mp3,flac");
        let mp3 = Decision::Convert(TransferCodec::Mp3, TransferBitrate::K192);
        assert_eq!(decide(&facts("opus", 48_000, 2, 96), &v1, &unplayable, Some(&old)), mp3);
        let wav = facts("wav", 44_100, 2, 1411);
        assert_eq!(decide(&wav, &card, &unplayable, None), Decision::Skip(Skip::NoTranscode));
        assert_eq!(decide(&wav, &device("flac"), &unplayable, Some(&old)), Decision::Skip(Skip::NoCodec));
        // Facts the metadata didn't give stand in the file's favour.
        let bare = Facts { ext: "flac".into(), size: 1 << 20, ..Facts::default() };
        assert_eq!(decide(&bare, &card, &unplayable, Some(&old)), original);

        // Everything, to Opus 128k: but a file already Opus at no more.
        let everything = Conversion { convert: Convert::Everything, bitrate: TransferBitrate::K128, ..unplayable };
        let opus128 = Decision::Convert(TransferCodec::Opus, TransferBitrate::K128);
        assert_eq!(decide(&facts("flac", 44_100, 2, 900), &card, &everything, Some(&old)), opus128);
        assert_eq!(decide(&facts("mp3", 44_100, 2, 128), &card, &everything, Some(&old)), opus128);
        assert_eq!(decide(&facts("opus", 48_000, 2, 96), &card, &everything, Some(&old)), original);
        assert_eq!(decide(&facts("opus", 48_000, 2, 192), &card, &everything, Some(&old)), opus128);
        let unknown = Facts { bitrate: None, ..facts("opus", 48_000, 2, 0) };
        assert_eq!(decide(&unknown, &card, &everything, Some(&old)), opus128, "no bitrate: not known to be within");

        assert!(ogg_is_opus(&crate::device::card::source::length::tests::opus_bytes(1000, 2, true)));
        assert!(!ogg_is_opus(b"OggS\0\x02 not opus at all, not at all, no"));
    }

    #[test]
    fn mp3_goes_past_192k_only_on_a_server_that_lists_the_rate() {
        use TransferBitrate::{K192, K320};
        use TransferCodec::{Mp3, Opus};
        let card = device("mp3,flac,opus");
        let rates = serde_json::json!(["64k", "96k", "128k", "192k", "256k", "320k"]);
        let codecs = serde_json::json!({"mp3": {"bitrates": rates}, "opus": {"bitrates": rates}});
        let new: TranscodeInfo = serde_json::from_value(serde_json::json!({ "codecs": codecs })).unwrap();
        let old = TranscodeInfo::default();
        let mp3 = Conversion { convert: Convert::Everything, codec: Mp3, bitrate: K320 };
        let flac = facts("flac", 44_100, 2, 900);
        assert_eq!(decide(&flac, &card, &mp3, Some(&new)), Decision::Convert(Mp3, K320));
        assert_eq!(decide(&flac, &card, &mp3, Some(&old)), Decision::Convert(Mp3, K192), "today's ceiling");
        // A file in the codec at no more than the setting stays as it is,
        // whatever the server makes.
        let original = Decision::Original { ogg_opus: false };
        assert_eq!(decide(&facts("mp3", 44_100, 2, 256), &card, &mp3, Some(&old)), original);
        // Opus stops at 192k on any server; six channels go to MP3, at its rate.
        let opus = Conversion { codec: Opus, ..mp3 };
        assert_eq!(decide(&flac, &card, &opus, Some(&new)), Decision::Convert(Opus, K192));
        let six = facts("flac", 48_000, 6, 4000);
        assert_eq!(decide(&six, &card, &opus, Some(&new)), Decision::Convert(Mp3, K320));
        // The size is the rate's.
        assert_eq!(estimate(Some(181_500), &flac, 320), 7_405_200);
    }

    #[test]
    fn a_conversions_size_is_its_length_at_its_bitrate() {
        let f = facts("flac", 44_100, 2, 900);
        // 181.5 s at 192 kbit/s: 4,356,000 bytes, and 2%.
        assert_eq!(estimate(Some(181_500), &f, 192), 4_443_120);
        // No length: the source's bytes at the bitrates' ratio.
        assert_eq!(estimate(None, &f, 90), 3_208_643);
        let bare = Facts { bitrate: None, ..f.clone() };
        assert_eq!(estimate(None, &bare, 192), 30 << 20);
    }

    #[test]
    fn a_manifest_entry_is_its_ledger_row() {
        let entry: ManifestEntry = serde_json::from_value(serde_json::json!({
            "filepath": "Rock/Artist A/Album B/01.flac", "id": 4_294_967_296i64, "file-size": 4096,
            "modified": 1_790_000_000_123i64, "hash": "00112233445566778899aabbccddeeff",
            "audio-hash": "not a hash", "hash-v": 2, "album-id": 7, "artist-id": -1,
            "created-at": "2026-01-02 03:04:05",
        }))
        .unwrap();
        let l = ledger_of(&entry);
        assert_eq!((l.mstream_id, l.album_id, l.artist_id), (0, 7, 0), "ids past u32 are 0");
        assert_eq!(l.file_hash[0..2], [0x00, 0x11]);
        assert_eq!(l.audio_hash, [0; 16]);
        assert_eq!((l.server_size, l.server_modified, l.hash_v), (4096, 1_790_000_000_123, 2));
        assert_eq!(l.created_at, 1_767_323_045);
        assert_eq!(unix_seconds("1970-01-01 00:00:00"), Some(0));
        assert_eq!(unix_seconds("2000-02-29T12:00:00.000Z"), Some(951_825_600));
        for bad in ["", "2026-13-01 00:00:00", "2026/01/02 03:04:05", "yesterday", "1969-12-31 23:59:59"] {
            assert_eq!(unix_seconds(bad), None, "{bad}");
        }
    }

    fn origin() -> Origin {
        let server = Server::direct(NO_INSTANCE, 7);
        Origin { talk: Talk { url_key: 7, ..Talk::default() }, server, ..Origin::default() }
    }

    fn entry(path: &str, id: i64, size: u64) -> ManifestEntry {
        serde_json::from_value(serde_json::json!({
            "filepath": path, "id": id, "file-size": size, "modified": 1_790_000_000_000i64 + id,
            "hash": format!("{:032x}", id), "audio-hash": format!("{:032x}", id + 1000), "hash-v": 2,
            "metadata": {"duration": 120.5, "album-art": "ab.jpg", "bpm": 128, "musical-key": "A minor"},
        }))
        .unwrap()
    }

    #[test]
    fn the_wanted_card_reuses_what_the_ledger_knows_and_reads_the_rest() {
        let card = device("mp3,flac,opus");
        let entries = vec![
            entry("Rock/A/01.flac", 1, 3000),
            entry("Rock/A/02.wav", 2, 9000),
            entry("Rock/A/03.ogg", 3, 500),
            entry("Rock/A/04.wav", 4, 9000),
        ];
        // 04 was converted to MP3 128k under these settings; unchanged since.
        let mut kept = HashMap::new();
        let mut row = ledger_of(&entries[3]);
        (row.converted_to, row.convert_kbps) = (CONTAINER_MP3, 128);
        let size = 1_234;
        kept.insert(row.server_path.clone(), Kept { ledger: row, size, ..Kept::default() });
        assert_eq!(needing_metadata(&entries, &kept, true), ["Rock/A/01.flac", "Rock/A/02.wav", "Rock/A/03.ogg"]);
        assert_eq!(needing_metadata(&entries, &kept, false).len(), 4, "another plan reads all");
        assert_eq!(oggs_to_sniff(&entries, &kept, true).len(), 1);

        let full = |source: &str| -> Option<Metadata> {
            let m = serde_json::json!({"sample-rate": 44100, "channels": 2, "bpm": 128, "musical-key": "A minor",
                                      "bpm-source": source, "duration": 120.5});
            Some(serde_json::from_value(m).unwrap())
        };
        let metadata: HashMap<String, Option<Metadata>> = HashMap::from([
            ("Rock/A/01.flac".to_string(), full("essentia")),
            ("Rock/A/02.wav".to_string(), full("tag")),
            ("Rock/A/03.ogg".to_string(), None),
        ]);
        let ogg = HashMap::from([("Rock/A/03.ogg".to_string(), true)]);
        let w = wanted_of(origin(), &entries, &metadata, &ogg, &kept, true, &card, &Conversion::default(), Some(&TranscodeInfo::default()));
        let files = &w.card.files;
        let kinds: Vec<(Bytes, u16, u64)> = files.iter().map(|f| (f.bytes, f.kbps, f.size)).collect();
        assert_eq!(
            kinds,
            [
                (Bytes::Original { ogg_opus: false }, 0, 3000),
                (Bytes::Converted(CONTAINER_OPUS), 192, 2_949_840),
                (Bytes::Original { ogg_opus: true }, 0, 500),
                (Bytes::Converted(CONTAINER_MP3), 128, 1_234),
            ]
        );
        assert_eq!((files[0].bpm10, files[0].camelot), (1280, 8), "mStream's analysis, for an untagged file");
        assert_eq!(files[0].ledger.mstream_id, 1);
        let tagged = &w.extras[1];
        assert_eq!((tagged.bpm_tag.as_deref(), tagged.key_tag.as_deref()), (Some("128"), Some("A minor")));
        assert!(w.extras[0].full && w.extras[0].bpm_tag.is_none(), "analysed values are never retagged");
        assert_eq!((w.extras[3].full, w.extras[3].duration_ms), (false, Some(120_500)));
        assert_eq!(w.extras[0].album_art.as_deref(), Some("ab.jpg"));
        assert!(w.extras.iter().all(|e| e.listed));

        // With no server ffmpeg the WAV can't go; the reused conversion stays.
        let w = wanted_of(origin(), &entries, &metadata, &ogg, &kept, true, &card, &Conversion::default(), None);
        assert_eq!(w.skipped, [("Rock/A/02.wav".to_string(), Skip::NoTranscode)]);
        assert_eq!(w.card.files.len(), 3);

        // After a 304, the ledger is the selection.
        let rows: Vec<Kept> = kept.values().cloned().collect();
        let w = wanted_of_ledger(origin(), &rows);
        assert_eq!(w.card.files[0].bytes, Bytes::Converted(CONTAINER_MP3));
        assert_eq!((w.card.files[0].kbps, w.card.files[0].size), (128, 1_234));
        assert_eq!(w.card.files[0].ledger.converted_to, 0, "the engine's to set");
        assert!(!w.extras[0].listed, "a 304 names no art");
    }

    #[test]
    fn a_run_fits_by_its_files_its_overheads_and_the_margin() {
        let space = Space { size: 64 << 30, free: 10 << 30, cluster: 32 * 1024, clusters: 0 };
        let mut fp = Footprint {
            writes: vec![("A/B/01.flac".into(), 30_000_001), ("A/B/02.flac".into(), 1)],
            folders: vec!["A".into(), "A/B".into()],
            freed: vec![1],
            records_before: 0,
            records: 2,
        };
        let f = fit(&space, &fp, true);
        let c = 32 * 1024u64;
        let music = 30_015_488 + c;
        // The growth of `/music` (A), A (B) and A/B (the two files), a
        // cluster each, and a cluster of each new folder's own.
        let dirs = 3 * c + 2 * c;
        // The next tags file and index, a thumbnail, the stage file.
        let mstream = 2 * 345 + 2 * 530 + c + 30_015_488;
        let player = 2 * 330 + (1 << 20) + 1_048_480;
        let margin = (64u64 << 30) / 100;
        assert_eq!(f.needed, music + dirs + mstream + player + margin);
        assert_eq!(f.room, (10 << 30) + c);
        assert!(f.fits() && !f.over_cap());

        // A full card: short by the difference.
        let tight = Space { free: f.needed - c - 10, ..space };
        let f2 = fit(&tight, &fp, true);
        assert!(!f2.fits());
        assert_eq!(f2.short(), 10);
        // A run that fetches nothing always fits; the cap counts tracks.
        fp.writes.clear();
        assert!(fit(&tight, &fp, true).fits());
        fp.records = TRACK_CAP + 1;
        assert!(fit(&space, &fp, false).over_cap());
        // A small card's margin is 256 MB.
        let small = Space { size: 2 << 30, free: 2 << 30, cluster: 0, clusters: 0 };
        let one = Footprint { writes: vec![("x.mp3".into(), 1)], records: 1, ..Footprint::default() };
        assert!(fit(&small, &one, false).needed > 256 << 20);
    }
}
