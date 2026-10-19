//! The tag reader (the firmware repo's docs/METADATA.md part 5, into the
//! record of 2.6.4 and its string run, 2.6.5): a file on the card read the
//! way mStream's default scanner reads it — lofty 0.25.1 in relaxed mode
//! behind mStream's ID3v2 repair pass, with its rust-parser's choice of tag
//! — and part 5's rules on top. So the record the transfer writes for a
//! file is the one the device's own scan (`lib/core/TagScan`) makes of the
//! same bytes (2.7; 2.17, item 3). A port of the firmware repo's
//! `tools/tagref`, the reference reader that made the parity corpus's
//! `expected.json`; the tests hold this one to that file
//! (test/fixtures/tags, copied from there).
//!
//! What a [`Reading`] fills: the twelve fields, the numbers and their
//! totals, BPM and key, ReplayGain (R128's for Opus), compilation, the
//! elected picture (its anchor, type, MIME and coding), container and
//! durationMs, `known`, and the NO_TAGS, UNREADABLE and TRUNCATED flags.
//! What it leaves to its caller: size, fatTime and qfp (the card, read
//! back), and the server's analysed BPM and key (BPM_ANALYSED, 2.7;
//! [`camelot`] reads mStream's `musical-key` too).
//!
//! **The picture's anchor.** lofty elects the picture (5.3) but doesn't say
//! where its bytes lie: [`anchors`], a port of the device's own walk, finds
//! the picture in the file that reads back to the elected one's image. A
//! picture with no such anchor (one the device doesn't read, as COVERART)
//! is written as absent: the five picture fields 0, while `known` is every
//! bit readRules 1 defines, as 2.6.4 has it for any readable file.
//!
//! [`restore`] is the other direction, for a file the transfer converted:
//! mStream's `/transcode` runs ffmpeg with `-vn`, which drops the embedded
//! picture, and its tag copy may not carry BPM or key into the new format,
//! so the cover and the source's own BPM and key tags are written back
//! before the converted file is read like any other.

use std::borrow::Cow;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use lofty::TextEncoding;
use lofty::config::{ParseOptions, ParsingMode, WriteOptions};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::{Frame, FrameId, Id3v2Tag, TextInformationFrame};
use lofty::io::FileLike;
use lofty::mpeg::MpegFile;
use lofty::ogg::{OggPictureStorage, OpusFile};
use lofty::picture::{APE_PICTURE_TYPES, MimeType, Picture, PictureType};
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagExt, TagType};

use super::contract::mptg::{
    CONTAINER_FLAC, CONTAINER_MP3, CONTAINER_OPUS, CONTAINER_UNKNOWN, HAS_RG_ALBUM, HAS_RG_TRACK,
    KNOWN_RULES1, NO_TAGS, RG_FROM_R128, Record, TRUNCATED, UNREADABLE,
};
use super::contract::numbers;
use super::contract::prim::fnv1a64;
use super::contract::text::{
    ALBUM, ALBUM_ARTIST, ALBUM_ARTIST_SORT, ALBUM_SORT, ARTIST, ARTIST_SORT, COMPOSER, GENRE,
    MB_ALBUM_ID, MB_RECORDING_ID, RUN_FIELDS, Run, TITLE, TITLE_SORT, build_field,
};

mod anchors;

/// The tags file header's parserVersion for the records this reader makes
/// (2.6.1). A change that alters a record bumps it, and the transfer then
/// reads every record it carries forward again from the card file. 2: the
/// picture's anchor (1 wrote every picture as absent).
pub(crate) const PARSER_VERSION: u16 = 2;

/// What the reader made of one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reading {
    /// The record as far as the file's tags fill it; size, fatTime and qfp
    /// are 0 for the caller to fill.
    pub(crate) rec: Record,
    /// The twelve fields, 2.3.6 applied: the run the record points to.
    pub(crate) run: Run,
    /// The embedded picture 5.3 elects, when the file has one.
    pub(crate) picture: Option<Elected>,
    /// lofty's reason, for an UNREADABLE file: for the log.
    pub(crate) error: Option<String>,
}

/// The embedded picture 5.3 elects (the first non-empty front cover, else
/// the first non-empty picture), as lofty read it: what its anchor must
/// read back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Elected {
    /// The ID3/FLAC picture type, 2.6.4's picType (3 = front cover).
    pub(crate) pic_type: u8,
    /// 2.6.4's picMime: 1 JPEG, 2 PNG, 3 other.
    pub(crate) mime: u8,
    /// The FNV-1a 64 of its image data (the corpus's `picFnv`).
    pub(crate) fnv: u64,
    /// Its image data's length in bytes.
    pub(crate) bytes: usize,
}

/// Reads a file by its path: its extension says how (`mp3`, `flac` or
/// `opus`, any case). Err only when the file itself can't be read; a file
/// lofty can't parse is an UNREADABLE [`Reading`].
pub(crate) fn read_path(path: &Path) -> io::Result<Reading> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let file = std::fs::File::open(path)?;
    read_from(io::BufReader::new(file), ext)
}

/// Reads a file's bytes, `ext` its extension.
pub(crate) fn read_bytes(bytes: &[u8], ext: &str) -> Reading {
    // A slice's reader never fails.
    let failed = |e: io::Error| unreadable(container_of(ext), e.to_string());
    read_from(Cursor::new(bytes), ext).unwrap_or_else(failed)
}

/// Reads a file from where it lies: only the parts lofty seeks to are read
/// (an MP3's head and tail, a FLAC's metadata blocks, an Opus stream's first
/// and last pages, and the tag at the head whole for the repair pass), never
/// the whole file. Err when the reader itself fails, which says nothing
/// about the file's tags.
pub(crate) fn read_from<R: Read + Seek>(mut r: R, ext: &str) -> io::Result<Reading> {
    let ext = ext.to_ascii_lowercase();
    let container = container_of(&ext);
    let len = r.seek(SeekFrom::End(0))?;
    let (stored, head) = head_tag(&mut r, len)?;
    let mut src =
        Spliced { head, inner: r, inner_at: None, pos: 0, len, failed: None, seen: Vec::new() };
    let probed = probe(&mut src, &ext);
    if let Some(e) = src.failed.take() {
        return Err(e);
    }
    let p = match probed {
        Ok(p) => p,
        Err(e) => return Ok(unreadable(container, e)),
    };
    let mut reading = reading(&p, &ext, container);
    // The elected picture's anchor, from the bytes as they lie (2.6.4): the
    // tag at the head as stored, and what lofty read past it, are at hand.
    let kind = chosen(&p.file).map(Tag::tag_type);
    let Spliced { inner, seen, .. } = &mut src;
    let at_hand = seen.iter().map(|(at, b)| (*at, b.as_slice()));
    let held: Vec<(u64, &[u8])> = std::iter::once((0, stored.as_slice())).chain(at_hand).collect();
    if let (Some(pic), Some(kind)) = (reading.picture, kind)
        && let Some(a) = anchors::locate(inner, &held, &ext, kind, &pic)?
    {
        let rec = &mut reading.rec;
        (rec.pic_offset, rec.pic_length, rec.pic_coding) = (a.offset, a.length, a.coding);
        (rec.pic_type, rec.pic_mime) = (pic.pic_type, pic.mime);
    }
    Ok(reading)
}

fn container_of(ext: &str) -> u8 {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => CONTAINER_MP3,
        "flac" => CONTAINER_FLAC,
        "opus" => CONTAINER_OPUS,
        _ => CONTAINER_UNKNOWN,
    }
}

/// 2.6.4: UNREADABLE, every tag field absent, `known` 0 and no length.
fn unreadable(container: u8, error: String) -> Reading {
    let rec = Record { container, flags: UNREADABLE, ..Record::default() };
    Reading { rec, run: Run::default(), picture: None, error: Some(error) }
}

// ── mStream's ID3v2 repair pass (5.2) ───────────────────────────────────────
//
// A rewrite of the tag at the file's start, the same length, that lofty then
// reads in its place:
//   1. v2.4 unsynchronisation (the frame's flag, or the tag's for every
//      frame) undone frame by frame; v2.2/2.3 tag-level unsynchronisation
//      undone for the walk and the flag cleared;
//   2. a UTF-16 text frame of odd length: the stray byte becomes U+FFFD;
//   3. a UTF-8 text frame that isn't: lossy, U+FFFD per invalid subpart;
//   4. a frame running past the tag's end: cut there, nothing after it;
//   5. a URL frame (W***, not WXXX) with a NUL in front: the NUL dropped;
//   6. a v2.4 frame size that isn't syncsafe: read as a plain integer when
//      that, and not the syncsafe reading, lands on a frame boundary.
// When the grown rewrite doesn't fit the tag, the pass shrinks instead (the
// stray byte dropped, '?' per bad byte); a tag with an extended header, or a
// v2.4 unsynchronised frame shorter than its flagged lead bytes, isn't
// rewritten.

/// The largest tag the pass rewrites; a larger one is read as it stands.
const MAX_REPAIR: usize = 64 * 1024 * 1024;

fn syncsafe(b: &[u8]) -> usize {
    ((b[0] & 0x7f) as usize) << 21
        | ((b[1] & 0x7f) as usize) << 14
        | ((b[2] & 0x7f) as usize) << 7
        | (b[3] & 0x7f) as usize
}

fn deunsync(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        out.push(src[i]);
        if src[i] == 0xff && src.get(i + 1) == Some(&0) {
            i += 1;
        }
        i += 1;
    }
    out
}

fn text_frame_id(id: &[u8]) -> bool {
    id.first() == Some(&b'T')
        || matches!(id, b"COMM" | b"USLT" | b"IPLS" | b"COM" | b"ULT" | b"IPL")
}

fn id_byte(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

/// A frame may end at `at`: the tag's end, padding, or another frame's id.
fn boundary_ok(data: &[u8], at: usize) -> bool {
    let id_at = |at: usize| at + 4 <= data.len() && data[at..at + 4].iter().all(|b| id_byte(*b));
    at == data.len() || (at < data.len() && (data[at] == 0 || id_at(at)))
}

/// The pass over `tag`, the file's first `10 + size` bytes (fewer when the
/// file is shorter): the rewritten tag, or None when nothing needs it.
fn repair_pass(tag: &[u8], grow: bool) -> Option<Vec<u8>> {
    if tag.len() < 10 || &tag[..3] != b"ID3" || !(2..=4).contains(&tag[3]) {
        return None;
    }
    let major = tag[3];
    let flags = tag[5];
    if flags & 0x40 != 0 {
        return None;
    }
    let size = syncsafe(&tag[6..10]);
    if size == 0 || size > MAX_REPAIR {
        return None;
    }
    let raw = &tag[10..tag.len().min(10 + size)];
    let tag_unsync = flags & 0x80 != 0;
    let whole;
    let data: &[u8] = if tag_unsync && major < 4 {
        whole = deunsync(raw);
        &whole
    } else {
        raw
    };
    let (hl, idl) = if major == 2 { (6, 3) } else { (10, 4) };
    let mut out: Option<Vec<u8>> = None;
    let mut pos = 0;
    while pos + hl <= data.len() {
        if data[pos] == 0 {
            break;
        }
        let id = &data[pos..pos + idl];
        let mut plain = false;
        let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let declared = match major {
            2 => be32(&[0, data[pos + 3], data[pos + 4], data[pos + 5]]),
            3 => be32(&data[pos + 4..pos + 8]),
            _ => {
                let b = &data[pos + 4..pos + 8];
                let ss = syncsafe(b);
                let be = be32(b);
                if be == ss {
                    ss
                } else {
                    let ok_ss = b.iter().all(|x| x & 0x80 == 0) && boundary_ok(data, pos + hl + ss);
                    let ok_be = boundary_ok(data, pos + hl + be);
                    if !ok_ss && ok_be {
                        plain = true;
                        be
                    } else {
                        ss
                    }
                }
            }
        };
        let fflags = if major == 2 { 0 } else { u16::from_be_bytes([data[pos + 8], data[pos + 9]]) };
        let body_at = pos + hl;
        let truncated = declared > data.len() - body_at;
        let core = if major == 3 && id[3] == 0 { &id[..3] } else { id };
        if !core.iter().all(|b| id_byte(*b)) {
            if truncated {
                break;
            }
            if let Some(o) = out.as_mut() {
                o.extend_from_slice(&data[pos..body_at + declared]);
            }
            pos = body_at + declared;
            continue;
        }
        let raw_body = &data[body_at..(body_at + declared).min(data.len())];
        // (compressed, encrypted, grouping, unsynchronised, data length indicator)
        let (compressed, encrypted, grouping, frame_unsync, dli) = match major {
            3 => (fflags & 0x80 != 0, fflags & 0x40 != 0, fflags & 0x20 != 0, false, false),
            4 => (
                fflags & 0x08 != 0,
                fflags & 0x04 != 0,
                fflags & 0x40 != 0,
                fflags & 0x02 != 0,
                fflags & 0x01 != 0,
            ),
            _ => (false, false, false, false, false),
        };
        let mut new_flags = fflags;
        let mut changed = truncated || plain;
        let mut body: Vec<u8>;
        if major == 4 && (frame_unsync || tag_unsync) {
            let length_lead = if dli || compressed { 4 } else { 0 };
            let lead = encrypted as usize + grouping as usize + length_lead;
            if raw_body.len() < lead {
                return None;
            }
            body = raw_body[..lead].to_vec();
            body.extend(deunsync(&raw_body[lead..]));
            new_flags &= !0x0002;
            changed = true;
        } else {
            body = raw_body.to_vec();
        }
        if !compressed && !encrypted && id[0] == b'W' && id != b"WXXX" && id != b"WXX" {
            let lead = grouping as usize + if major == 4 && dli { 4 } else { 0 };
            if body.len() > lead + 1 && body[lead] == 0 {
                body.remove(lead);
                changed = true;
            }
        }
        if !compressed && !encrypted && text_frame_id(id) {
            let lead = grouping as usize + if major == 4 && dli { 4 } else { 0 };
            let lang = if matches!(id, b"COMM" | b"USLT" | b"COM" | b"ULT") { 3 } else { 0 };
            let text = lead + 1 + lang;
            if body.len() > text {
                match body[lead] {
                    e @ (1 | 2) if (body.len() - text) % 2 == 1 => {
                        body.pop();
                        if grow {
                            let be = e == 2 || body[text..].starts_with(&[0xfe, 0xff]);
                            body.extend(if be { [0xff, 0xfd] } else { [0xfd, 0xff] });
                        }
                        changed = true;
                    }
                    3 if std::str::from_utf8(&body[text..]).is_err() => {
                        let fixed: Vec<u8> = if grow {
                            String::from_utf8_lossy(&body[text..]).into_owned().into_bytes()
                        } else {
                            let mut v = Vec::new();
                            for c in body[text..].utf8_chunks() {
                                v.extend_from_slice(c.valid().as_bytes());
                                v.extend(std::iter::repeat_n(b'?', c.invalid().len()));
                            }
                            v
                        };
                        body.truncate(text);
                        body.extend(fixed);
                        changed = true;
                    }
                    _ => {}
                }
            }
        }
        if changed && out.is_none() {
            let mut o = Vec::with_capacity(10 + size);
            o.extend_from_slice(&tag[..10]);
            o[5] = flags & !0x80;
            o.extend_from_slice(&data[..pos]);
            out = Some(o);
        }
        if let Some(o) = out.as_mut() {
            o.extend_from_slice(id);
            let n = body.len();
            match major {
                2 => o.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]),
                3 => o.extend_from_slice(&(n as u32).to_be_bytes()),
                _ => o.extend_from_slice(&[
                    ((n >> 21) & 0x7f) as u8,
                    ((n >> 14) & 0x7f) as u8,
                    ((n >> 7) & 0x7f) as u8,
                    (n & 0x7f) as u8,
                ]),
            }
            if major != 2 {
                o.extend_from_slice(&new_flags.to_be_bytes());
            }
            o.extend_from_slice(&body);
        }
        if truncated {
            break;
        }
        pos = body_at + declared;
    }
    out
}

/// The tag lofty reads in place of `tag`, the same length: the grown
/// rewrite, else the shrunk one when only that fits; None when the tag
/// stands as it is.
fn repaired(tag: &[u8]) -> Option<Vec<u8>> {
    let mut out = repair_pass(tag, true)?;
    if out.len() > tag.len() {
        out = repair_pass(tag, false).filter(|o| o.len() <= tag.len())?;
    }
    out.resize(tag.len(), 0);
    Some(out)
}

/// The file's first ID3v2 tag as stored (empty when there is none, or
/// larger than [`MAX_REPAIR`]), and repaired (empty when the pass leaves
/// it as it is).
fn head_tag<R: Read + Seek>(r: &mut R, len: u64) -> io::Result<(Vec<u8>, Vec<u8>)> {
    if len < 10 {
        return Ok(Default::default());
    }
    // "ID3" first: lofty reads no more of a file's start than that when no
    // tag is there.
    let mut header = [0u8; 10];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut header[..3])?;
    if &header[..3] != b"ID3" {
        return Ok(Default::default());
    }
    r.read_exact(&mut header[3..])?;
    let size = syncsafe(&header[6..10]);
    if size == 0 || size > MAX_REPAIR {
        return Ok(Default::default());
    }
    let mut tag = vec![0u8; len.min(10 + size as u64) as usize];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut tag)?;
    let fixed = repaired(&tag).unwrap_or_default();
    Ok((tag, fixed))
}

/// The file as lofty is given it: the repaired head from memory, the rest
/// from where it lies. The file's own failures are kept apart from lofty's
/// verdict on its bytes: a card that can't be read isn't an UNREADABLE file.
struct Spliced<R> {
    head: Vec<u8>,
    inner: R,
    /// Where the inner reader is, when known: a seek only when lofty jumps.
    inner_at: Option<u64>,
    pos: u64,
    len: u64,
    failed: Option<io::Error>,
    /// What lofty read from where it lies, in runs (where each starts, its
    /// bytes): the picture's anchor is looked for in them, not on the card
    /// again.
    seen: Vec<(u64, Vec<u8>)>,
}

impl<R> Spliced<R> {
    fn fail(&mut self, e: io::Error) -> io::Error {
        let told = io::Error::new(e.kind(), e.to_string());
        self.inner_at = None;
        self.failed.get_or_insert(e);
        told
    }

    /// Keeps bytes read at `at`: a read that goes on from the last one
    /// extends its run.
    fn keep(&mut self, at: u64, bytes: &[u8]) {
        match self.seen.last_mut() {
            Some((start, run)) if *start + run.len() as u64 == at => run.extend_from_slice(bytes),
            _ => self.seen.push((at, bytes.to_vec())),
        }
    }
}

impl<R: Read + Seek> Read for Spliced<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        if self.pos < self.head.len() as u64 {
            let at = self.pos as usize;
            let n = buf.len().min(self.head.len() - at);
            buf[..n].copy_from_slice(&self.head[at..at + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        if self.inner_at != Some(self.pos) {
            if let Err(e) = self.inner.seek(SeekFrom::Start(self.pos)) {
                return Err(self.fail(e));
            }
            self.inner_at = Some(self.pos);
        }
        match self.inner.read(buf) {
            Ok(n) => {
                self.keep(self.pos, &buf[..n]);
                self.pos += n as u64;
                self.inner_at = Some(self.pos);
                Ok(n)
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(e),
            Err(e) => Err(self.fail(e)),
        }
    }
}

impl<R> Seek for Spliced<R> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let at = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let before = || io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start");
        let at = at.ok_or_else(before)?;
        self.pos = at;
        Ok(at)
    }
}

// ── Reading (mStream's read_tagged) ─────────────────────────────────────────
//
// lofty's file reader by the extension, and what the generic Tag doesn't
// carry: TPE2's values, the TXXX descriptions, the ID3v2 pictures with their
// frames' flags, every APE cover item.

struct Probed {
    file: TaggedFile,
    parts: Parts,
}

#[derive(Default)]
struct Parts {
    tpe2: Option<Vec<String>>,
    custom: Vec<(String, String)>,
    id3_pictures: Vec<Picture>,
    ape_pictures: Vec<Picture>,
}

fn id3_parts(tag: Option<&Id3v2Tag>, p: &mut Parts) {
    let Some(tag) = tag else { return };
    for frame in tag {
        match frame {
            Frame::Text(f) if frame.id().as_str() == "TPE2" => {
                let values = f.value.split('\0').filter(|v| !v.is_empty()).map(str::to_string);
                p.tpe2 = Some(values.collect());
            }
            Frame::UserText(f) => {
                p.custom.push((f.description.to_ascii_uppercase(), f.content.to_string()));
            }
            // 5.3: a compressed or encrypted frame's picture is never
            // elected (an encrypted one isn't a picture to lofty anyway).
            Frame::Picture(f) if !frame.flags().compression && frame.flags().encryption.is_none() => {
                p.id3_pictures.push(f.picture.clone().into_owned());
            }
            _ => {}
        }
    }
}

fn ape_parts(tag: Option<&lofty::ape::ApeTag>, p: &mut Parts) {
    let Some(tag) = tag else { return };
    for item in tag {
        let cover_key = APE_PICTURE_TYPES.iter().find(|k| k.eq_ignore_ascii_case(item.key()));
        let Some(canonical) = cover_key else { continue };
        if let ItemValue::Binary(data) = item.value()
            && let Ok(pic) = Picture::from_ape_bytes(canonical, data)
        {
            p.ape_pictures.push(pic);
        }
    }
}

fn probe<R: Read + Seek>(src: &mut R, ext: &str) -> Result<Probed, String> {
    let opts = ParseOptions::new().parsing_mode(ParsingMode::Relaxed);
    let mut parts = Parts::default();
    let file: TaggedFile = match ext {
        "mp3" => {
            let f = MpegFile::read_from(src, opts).map_err(|e| e.to_string())?;
            id3_parts(f.id3v2(), &mut parts);
            ape_parts(f.ape(), &mut parts);
            f.into()
        }
        "flac" => {
            let f = FlacFile::read_from(src, opts).map_err(|e| e.to_string())?;
            id3_parts(f.id3v2(), &mut parts);
            f.into()
        }
        "opus" => OpusFile::read_from(src, opts).map_err(|e| e.to_string())?.into(),
        _ => return Err("not an audio extension".into()),
    };
    Ok(Probed { file, parts })
}

// ── 5.3's rules ─────────────────────────────────────────────────────────────

/// mStream's `CAMELOT_TO_KEYS` (its src/api/random.js): each key's aliases,
/// 1A, 1B, 2A … 12B.
const CAMELOT: [&[&str]; 24] = [
    &["1A", "Ab minor", "Abmin", "G# minor", "G#min", "Abm", "G#m"],
    &["1B", "B major", "Bmaj", "B"],
    &["2A", "Eb minor", "Ebmin", "D# minor", "D#min", "Ebm", "D#m"],
    &["2B", "F# major", "F#maj", "Gb major", "Gbmaj", "F#", "Gb"],
    &["3A", "Bb minor", "Bbmin", "A# minor", "A#min", "Bbm", "A#m"],
    &["3B", "Db major", "Dbmaj", "C# major", "C#maj", "Db", "C#"],
    &["4A", "F minor", "Fmin", "Fm"],
    &["4B", "Ab major", "Abmaj", "G# major", "G#maj", "Ab", "G#"],
    &["5A", "C minor", "Cmin", "Cm"],
    &["5B", "Eb major", "Ebmaj", "D# major", "D#maj", "Eb", "D#"],
    &["6A", "G minor", "Gmin", "Gm"],
    &["6B", "Bb major", "Bbmaj", "A# major", "A#maj", "Bb", "A#"],
    &["7A", "D minor", "Dmin", "Dm"],
    &["7B", "F major", "Fmaj", "F"],
    &["8A", "A minor", "Amin", "Am"],
    &["8B", "C major", "Cmaj", "C"],
    &["9A", "E minor", "Emin", "Em"],
    &["9B", "G major", "Gmaj", "G"],
    &["10A", "B minor", "Bmin", "Bm"],
    &["10B", "D major", "Dmaj", "D"],
    &["11A", "F# minor", "F#min", "Gb minor", "Gbmin", "F#m", "Gbm"],
    &["11B", "A major", "Amaj", "A"],
    &["12A", "C# minor", "C#min", "Db minor", "Dbmin", "C#m", "Dbm"],
    &["12B", "E major", "Emaj", "E"],
];

/// A key as 2.6.4's camelot: trimmed, its first 12 characters matched
/// against the aliases ignoring ASCII case; 1-12 the minor keys 1A-12A,
/// 13-24 the major 1B-12B, 0 no match. The same rule reads a file's TKEY or
/// INITIALKEY and mStream's analysed `musical-key` (2.7).
pub(crate) fn camelot(s: &str) -> u8 {
    let k: String = s.trim().chars().take(12).collect();
    for (row, aliases) in CAMELOT.iter().enumerate() {
        if aliases.iter().any(|a| a.eq_ignore_ascii_case(&k)) {
            let n = (row / 2 + 1) as u8;
            return if row % 2 == 1 { 12 + n } else { n };
        }
    }
    0
}

/// TCMP or COMPILATION: `1` or `true` (any case) is 1, `0` or `false` is 2
/// (said no), anything else 0.
fn compilation(s: &str) -> u8 {
    if s == "1" || s.eq_ignore_ascii_case("true") {
        1
    } else if s == "0" || s.eq_ignore_ascii_case("false") {
        2
    } else {
        0
    }
}

/// mStream's lofty22_year: the Year item, else RecordingDate; after leading
/// whitespace, four ASCII digits.
fn year_of(tag: &Tag) -> Option<u16> {
    let s = tag.get_string(ItemKey::Year).or_else(|| tag.get_string(ItemKey::RecordingDate))?;
    let lead = s.chars().skip_while(|c| c.is_whitespace());
    let digits = lead.take_while(char::is_ascii_digit).take(4);
    let digits: Vec<u16> = digits.filter_map(|c| c.to_digit(10)).map(|d| d as u16).collect();
    (digits.len() == 4).then(|| digits.iter().fold(0, |y, d| y * 10 + d))
}

/// mStream's parse_num_of: "N/M" split once on `/`, each part trimmed and
/// parsed as an integer.
fn num_of(s: &str) -> (Option<u32>, Option<u32>) {
    let (n, t) = match s.split_once('/') {
        Some((n, t)) => (n, Some(t)),
        None => (s, None),
    };
    (n.trim().parse::<u32>().ok(), t.and_then(|t| t.trim().parse::<u32>().ok()))
}

fn texts(tag: &Tag, key: ItemKey) -> Vec<String> {
    let text = |i: &lofty::tag::TagItem| match i.value() {
        ItemValue::Text(s) => Some(s.to_string()),
        _ => None,
    };
    tag.get_items(key).filter_map(text).collect()
}

/// An ID3v1 string, trimmed of whitespace; none when that leaves nothing.
fn id3v1_text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// 5.1's blank: every value empty or whitespace only.
fn blank(stored: &[u8]) -> bool {
    String::from_utf8_lossy(stored).split('\u{1f}').all(|v| v.trim().is_empty())
}

/// 0 is none; above 65,535 is 65,535.
fn clamp(v: Option<u32>) -> u16 {
    v.map_or(0, |n| n.min(65535) as u16)
}

// ── One file's reading ──────────────────────────────────────────────────────

/// The tag mStream reads (5.1): lofty's primary tag, else its first.
fn chosen(file: &TaggedFile) -> Option<&Tag> {
    file.primary_tag().or_else(|| file.first_tag())
}

fn reading(p: &Probed, ext: &str, container: u8) -> Reading {
    let file = &p.file;
    let millis = file.properties().duration().as_millis();
    let mut rec = Record {
        container,
        // 2.6.4: every bit readRules 1 defines, whatever the file carries.
        known: KNOWN_RULES1,
        duration_ms: u32::try_from(millis).unwrap_or(u32::MAX),
        ..Record::default()
    };
    let tag = chosen(file);
    let kind = tag.map(|t| t.tag_type());
    let mut run = Run::default();
    let mut cut = false;
    let (mut year, mut track, mut track_total) = (None, None, None);
    let (mut disc, mut disc_total) = (None, None);
    let (mut rg_track, mut rg_album, mut from_r128) = (None, None, false);
    let mut comp = 0u8;
    let mut pictures: Vec<&Picture> = Vec::new();
    if let Some(t) = tag {
        let v1 = kind == Some(TagType::Id3v1);
        // ID3v1's strings are trimmed, chosen or filling (5.1).
        let tx = |key| {
            let v = texts(t, key);
            if v1 { v.iter().filter_map(|s| id3v1_text(s)).collect() } else { v }
        };
        let trimmed = |key| texts(t, key).iter().map(|s| s.trim().to_string()).collect::<Vec<_>>();
        let mut raw: [Vec<String>; RUN_FIELDS] = Default::default();
        raw[TITLE] = tx(ItemKey::TrackTitle);
        raw[ARTIST] = tx(ItemKey::TrackArtist);
        raw[ALBUM] = tx(ItemKey::AlbumTitle);
        // 5.3: ID3v2's album artist is TPE2's values, else the first
        // non-blank TXXX ALBUMARTIST or ALBUM ARTIST.
        raw[ALBUM_ARTIST] = if kind == Some(TagType::Id3v2) {
            match &p.parts.tpe2 {
                Some(v) if !v.is_empty() => v.clone(),
                _ => {
                    let named = |(k, v): &&(String, String)| {
                        (k == "ALBUMARTIST" || k == "ALBUM ARTIST") && !v.trim().is_empty()
                    };
                    let found = p.parts.custom.iter().find(named);
                    found.map(|(_, v)| vec![v.clone()]).unwrap_or_default()
                }
            }
        } else {
            texts(t, ItemKey::AlbumArtist)
        };
        raw[GENRE] = tx(ItemKey::Genre);
        raw[COMPOSER] = texts(t, ItemKey::Composer);
        raw[TITLE_SORT] = texts(t, ItemKey::TrackTitleSortOrder);
        raw[ARTIST_SORT] = texts(t, ItemKey::TrackArtistSortOrder);
        raw[ALBUM_SORT] = texts(t, ItemKey::AlbumTitleSortOrder);
        raw[ALBUM_ARTIST_SORT] = texts(t, ItemKey::AlbumArtistSortOrder);
        raw[MB_ALBUM_ID] = trimmed(ItemKey::MusicBrainzReleaseId);
        raw[MB_RECORDING_ID] = trimmed(ItemKey::MusicBrainzRecordingId);
        (run, cut) = Run::build(&raw);

        year = year_of(t);
        track = t.track();
        track_total = t.track_total();
        disc = t.disk();
        disc_total = t.disk_total();
        // lofty's readers first, then the raw string's own split.
        if (track.is_none() || track_total.is_none())
            && let Some(s) = t.get_string(ItemKey::TrackNumber)
        {
            let (n, total) = num_of(s);
            track = track.or(n);
            track_total = track_total.or(total);
        }
        if (disc.is_none() || disc_total.is_none())
            && let Some(s) = t.get_string(ItemKey::DiscNumber)
        {
            let (n, total) = num_of(s);
            disc = disc.or(n);
            disc_total = disc_total.or(total);
        }
        let bpm = t.get_string(ItemKey::Bpm).or_else(|| t.get_string(ItemKey::IntegerBpm));
        rec.bpm10 = bpm.and_then(numbers::bpm10).unwrap_or(0);
        rec.camelot = t.get_string(ItemKey::InitialKey).map_or(0, camelot);
        comp = t.get_string(ItemKey::FlagCompilation).map_or(0, compilation);
        if ext == "opus" {
            // Opus (RFC 7845, 5.2.1): the gains are R128's.
            rg_track = t.get_string(ItemKey::R128TrackGain).and_then(numbers::r128);
            rg_album = t.get_string(ItemKey::R128AlbumGain).and_then(numbers::r128);
            from_r128 = rg_track.is_some() || rg_album.is_some();
        } else {
            rg_track = t.get_string(ItemKey::ReplayGainTrackGain).and_then(numbers::gain);
            rg_album = t.get_string(ItemKey::ReplayGainAlbumGain).and_then(numbers::gain);
        }
        let peak = |key| t.get_string(key).and_then(numbers::peak).unwrap_or(0);
        rec.rg_track_peak = peak(ItemKey::ReplayGainTrackPeak);
        rec.rg_album_peak = peak(ItemKey::ReplayGainAlbumPeak);
        pictures = match kind {
            Some(TagType::Id3v2) => p.parts.id3_pictures.iter().collect(),
            Some(TagType::Ape) => p.parts.ape_pictures.iter().collect(),
            Some(TagType::Id3v1) => Vec::new(),
            _ => t.pictures().iter().collect(),
        };
        // 5.1: ID3v1 fills a blank title, artist, album or genre and an
        // absent year or track number of another chosen tag.
        if !v1 && let Some(one) = file.tag(TagType::Id3v1) {
            let fills = [
                (TITLE, ItemKey::TrackTitle),
                (ARTIST, ItemKey::TrackArtist),
                (ALBUM, ItemKey::AlbumTitle),
                (GENRE, ItemKey::Genre),
            ];
            for (field, key) in fills {
                if blank(run.get(field))
                    && let Some(v) = one.get_string(key).and_then(id3v1_text)
                {
                    run.fields[field] = build_field(&[v], false).0;
                }
            }
            year = year.or_else(|| year_of(one));
            track = track.or_else(|| one.track());
        }
    }
    // 5.3: every non-empty picture is seen; the first front cover, else the
    // first, is elected.
    pictures.retain(|p| !p.data().is_empty());
    let front = pictures.iter().find(|p| p.pic_type() == PictureType::CoverFront);
    let elected = front.or(pictures.first());
    let picture = elected.map(|pic| Elected {
        pic_type: pic.pic_type().as_u8(),
        mime: match pic.mime_type() {
            Some(MimeType::Jpeg) => 1,
            Some(MimeType::Png) => 2,
            _ => 3,
        },
        fnv: fnv1a64(pic.data()),
        bytes: pic.data().len(),
    });

    rec.year = year.unwrap_or(0);
    rec.track = clamp(track);
    rec.track_total = clamp(track_total);
    rec.disc = clamp(disc);
    rec.disc_total = clamp(disc_total);
    rec.rg_track_gain = rg_track.unwrap_or(0);
    rec.rg_album_gain = rg_album.unwrap_or(0);
    rec.flags = comp as u16;
    if rg_track.is_some() {
        rec.flags |= HAS_RG_TRACK;
    }
    if rg_album.is_some() {
        rec.flags |= HAS_RG_ALBUM;
    }
    if from_r128 {
        rec.flags |= RG_FROM_R128;
    }
    if cut {
        rec.flags |= TRUNCATED;
    }
    let numbered = [rec.year, rec.track, rec.track_total, rec.disc, rec.disc_total, rec.bpm10];
    let any = !run.is_empty()
        || numbered.iter().any(|n| *n != 0)
        || rec.camelot != 0
        || comp != 0
        || rg_track.is_some()
        || rg_album.is_some()
        || rec.rg_track_peak != 0
        || rec.rg_album_peak != 0
        || picture.is_some();
    if !any {
        rec.flags |= NO_TAGS;
    }
    Reading { rec, run, picture, error: None }
}

// ── Putting back what a conversion dropped ──────────────────────────────────

/// What [`restore`] writes into a converted file. Each is written only when
/// given, in place of what the file holds for it.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Restore<'a> {
    /// The album's cover, a baseline JPEG (the one kind of embedded image
    /// the device decodes, 2.14.3): the file's front cover.
    pub(crate) cover: Option<&'a [u8]>,
    /// The source file's own BPM tag, its text as it was (TBPM; Vorbis BPM).
    pub(crate) bpm: Option<&'a str>,
    /// The source file's own key tag (TKEY; Vorbis INITIALKEY, KEY dropped).
    pub(crate) key: Option<&'a str>,
}

/// Writes `what` into a converted file, an MP3's ID3v2 tag (made if it has
/// none) or an Opus stream's comments, through lofty's writer; the file's
/// other tags and its audio are left as they are. Never for an original:
/// the transfer copies those byte for byte. A cover that isn't a JPEG, or a
/// file of any other format, is refused before anything is written.
pub(crate) fn restore<F: FileLike>(file: &mut F, ext: &str, what: &Restore<'_>) -> Result<(), String> {
    let cover = match what.cover {
        Some(c) if !c.starts_with(&[0xFF, 0xD8, 0xFF]) => {
            return Err("the cover isn't a JPEG".into());
        }
        Some(c) => {
            let pic = Picture::unchecked(c.to_vec()).pic_type(PictureType::CoverFront);
            Some(pic.mime_type(MimeType::Jpeg).build())
        }
        None => None,
    };
    let opts = ParseOptions::new().parsing_mode(ParsingMode::Relaxed);
    let parsed = |e: lofty::error::FileParseError| e.to_string();
    let saved = |e: lofty::error::FileEncodingError| e.to_string();
    file.rewind().map_err(|e| e.to_string())?;
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => {
            let mpeg = MpegFile::read_from(file, opts).map_err(parsed)?;
            let mut tag = mpeg.id3v2().cloned().unwrap_or_default();
            let mut text = |id: &'static str, value: &str| {
                let id = FrameId::Valid(Cow::Borrowed(id));
                tag.remove(&id).for_each(drop);
                let frame = TextInformationFrame::new(id, TextEncoding::UTF8, value.to_string());
                tag.insert(Frame::Text(frame));
            };
            if let Some(bpm) = what.bpm {
                text("TBPM", bpm);
            }
            if let Some(key) = what.key {
                text("TKEY", key);
            }
            if let Some(pic) = cover {
                tag.remove_picture_type(PictureType::CoverFront);
                tag.insert_picture(pic);
            }
            file.rewind().map_err(|e| e.to_string())?;
            tag.save_to(file, WriteOptions::default()).map_err(saved)
        }
        "opus" => {
            let opus = OpusFile::read_from(file, opts).map_err(parsed)?;
            let mut tag = opus.vorbis_comments().clone();
            if let Some(bpm) = what.bpm {
                tag.insert("BPM".into(), bpm.to_string());
            }
            if let Some(key) = what.key {
                tag.remove("KEY").for_each(drop);
                tag.insert("INITIALKEY".into(), key.to_string());
            }
            if let Some(pic) = cover {
                tag.remove_picture_type(PictureType::CoverFront);
                tag.insert_picture(pic, None).map_err(|e| e.to_string())?;
            }
            file.rewind().map_err(|e| e.to_string())?;
            tag.save_to(file, WriteOptions::default()).map_err(saved)
        }
        other => Err(format!("no tags are put back into a .{other} file")),
    }
}

/// [`restore`] on a file by its path, its extension saying which format.
// For a conversion on the local disk (B1, B9).
#[allow(dead_code)]
pub(crate) fn restore_path(path: &Path, what: &Restore<'_>) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let opened = std::fs::OpenOptions::new().read(true).write(true).open(path);
    let mut file = opened.map_err(|e| e.to_string())?;
    restore(&mut file, ext, what)
}

#[cfg(test)]
mod tests;
