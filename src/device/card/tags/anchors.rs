//! The picture's anchor (2.6.4): where in the file the elected picture's
//! bytes lie, which lofty doesn't say. The walk is the device's own (the
//! firmware repo's `lib/core/TagScan`, its picture rules ported): the tag
//! the picture came from, walked as it is stored — an ID3v2 tag's frames,
//! raw or unsynchronised by the tag or the frame, with mStream's repair
//! pass's sizes and cut for the tag at offset 0; a FLAC's comment block
//! and PICTURE blocks; an Opus stream's comment packet across its pages;
//! an APEv2 tag's binary cover items — and each picture there located by
//! its coding.
//!
//! Which of them lofty elected is told by the bytes: the anchor taken is
//! the first whose bytes, read back the way the device reads them, are
//! the elected picture's image (its FNV-1a 64, and its type too when two
//! pictures hold the same image). So the election stays lofty's, the
//! reference's (5.3), and an anchor is never another picture's. When none
//! reads back to it (a picture the device doesn't read: COVERART, an APE
//! tag at the head), there is no anchor, and the record says no picture.
//!
//! The walk reads where lofty read for the tags (an MP3's head and tail, a
//! FLAC's metadata blocks, an Opus stream's comment pages), never through
//! the audio, and takes those bytes from what the reader kept of lofty's
//! reads: the card isn't read again for them.

use std::io::{self, Read, Seek, SeekFrom};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use lofty::picture::{APE_PICTURE_TYPES, PictureType};
use lofty::tag::TagType;

use super::super::contract::mptg::{CODING_APE, CODING_OGG_BASE64, CODING_RAW, CODING_UNSYNC};
use super::super::contract::prim::fnv1a64;
use super::{Elected, MAX_REPAIR, boundary_ok, deunsync, id_byte, syncsafe};

/// lofty's `max_junk_bytes` (its default): how far from an MP3's start
/// an ID3v2 tag is looked for in junk when none is at its head.
const MAX_JUNK: u64 = 1024;

/// A picture's anchor (2.6.4): its first stored byte, how many there are,
/// and how they read (picCoding).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Anchor {
    pub(super) offset: u32,
    pub(super) length: u32,
    pub(super) coding: u8,
}

/// A picture the walk found: its anchor, its type, and the FNV-1a 64 of
/// the image its anchor reads back to.
struct Found {
    anchor: Anchor,
    pic_type: u8,
    fnv: u64,
}

impl Found {
    /// A picture whose `length` stored bytes start at `offset`; none when
    /// it has no bytes, or lies past what the record's fields hold.
    fn new(offset: u64, length: u64, coding: u8, pic_type: u8, fnv: u64) -> Option<Found> {
        let (offset, length) = (u32::try_from(offset).ok()?, u32::try_from(length).ok()?);
        let anchor = Anchor { offset, length, coding };
        (offset != 0 && length != 0).then_some(Found { anchor, pic_type, fnv })
    }
}

/// The anchor of `elected`, the picture lofty elected from the tag of
/// `kind` in a file of extension `ext` (lowercase); `held` is what the
/// reader already holds of the file (where each run starts, its bytes).
/// None when no picture the walk finds reads back to it; Err only when the
/// file can't be read.
pub(super) fn locate<R: Read + Seek>(
    r: &mut R,
    held: &[(u64, &[u8])],
    ext: &str,
    kind: TagType,
    elected: &Elected,
) -> io::Result<Option<Anchor>> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut f = File { r, len, held };
    let found = match (ext, kind) {
        ("mp3", TagType::Id3v2) => mp3_id3v2(&mut f)?,
        ("mp3", TagType::Ape) => ape(&mut f)?,
        ("flac", TagType::Id3v2) => flac_id3v2(&mut f)?,
        ("flac", TagType::VorbisComments) => flac(&mut f)?,
        ("opus", TagType::VorbisComments) => opus(&mut f)?,
        _ => Vec::new(),
    };
    let image = |p: &&Found| p.fnv == elected.fnv;
    let typed = found.iter().filter(image).find(|p| p.pic_type == elected.pic_type);
    Ok(typed.or_else(|| found.iter().find(image)).map(|p| p.anchor))
}

/// The file as the walk reads it: from the runs held where they hold the
/// bytes, else from where they lie.
struct File<'a, R> {
    r: &'a mut R,
    len: u64,
    held: &'a [(u64, &'a [u8])],
}

impl<R: Read + Seek> File<'_, R> {
    /// The bytes of `[at, at + n)`, fewer where the file ends first.
    fn read(&mut self, at: u64, n: u64) -> io::Result<Vec<u8>> {
        let end = at.saturating_add(n).min(self.len);
        if at >= end {
            return Ok(Vec::new());
        }
        if let Some(bytes) = self.from_held(at, end) {
            return Ok(bytes);
        }
        self.r.seek(SeekFrom::Start(at))?;
        let mut out = Vec::new();
        Read::take(&mut *self.r, end - at).read_to_end(&mut out)?;
        Ok(out)
    }

    /// `[at, end)` from the runs held, a piece of each in turn (the run
    /// that holds the next byte and reaches furthest); None at a gap.
    fn from_held(&self, at: u64, end: u64) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut pos = at;
        while pos < end {
            let reach = |&&(start, run): &&(u64, &[u8])| start + run.len() as u64;
            let holds = |r: &&(u64, &[u8])| r.0 <= pos && pos < reach(r);
            let (start, run) = *self.held.iter().filter(holds).max_by_key(reach)?;
            let stop = (start + run.len() as u64).min(end);
            out.extend_from_slice(&run[(pos - start) as usize..(stop - start) as usize]);
            pos = stop;
        }
        Some(out)
    }
}

fn be32(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

fn le32(b: &[u8]) -> usize {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

// ── ID3v2 ───────────────────────────────────────────────────────────────────

/// An ID3v2 header as lofty's parse takes it ("ID3", a major version 2-4,
/// not v2.2's compression): its version, flags and body size.
fn id3_header(b: &[u8]) -> Option<(u8, u8, usize)> {
    let ok = b.len() >= 10 && &b[..3] == b"ID3" && (2..=4).contains(&b[3]);
    (ok && !(b[3] == 2 && b[5] & 0x40 != 0)).then(|| (b[3], b[5], syncsafe(&b[6..10])))
}

/// What a tag takes in the file: its header, its body, its footer when its
/// flags say it has one.
fn id3_total(ver: u8, flags: u8, size: usize) -> u64 {
    10 + size as u64 + if ver >= 3 && flags & 0x10 != 0 { 10 } else { 0 }
}

/// An MP3's ID3v2 pictures: every tag at its head, past any zero bytes
/// (lofty reads them all, the later ones' frames merged in), else the one
/// in the junk before its audio.
fn mp3_id3v2<R: Read + Seek>(f: &mut File<'_, R>) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let mut at = 0;
    // lofty skips zero bytes before the first tag, a byte at a time.
    while f.read(at, 1)? == [0] {
        at += 1;
    }
    let mut tags = 0;
    while f.read(at, 3)? == b"ID3"
        && let Some((ver, flags, size)) = id3_header(&f.read(at, 10)?)
    {
        // mStream's repair pass reads the tag at offset 0, and only that.
        let repairs = tags == 0 && at == 0 && size <= MAX_REPAIR;
        id3v2(&f.read(at, 10 + size as u64)?, at, repairs, &mut out);
        tags += 1;
        at += id3_total(ver, flags, size);
    }
    if tags == 0 {
        // lofty, relaxed: the first "ID3" within its junk window.
        let window = f.read(0, MAX_JUNK)?;
        if let Some(i) = window.windows(3).position(|w| w == b"ID3") {
            let at = i as u64;
            if let Some((_, _, size)) = id3_header(&f.read(at, 10)?) {
                id3v2(&f.read(at, 10 + size as u64)?, at, false, &mut out);
            }
        }
    }
    Ok(out)
}

/// A FLAC's front ID3v2 tag's pictures (the tag lofty takes when the file
/// has no comment block and no PICTURE block).
fn flac_id3v2<R: Read + Seek>(f: &mut File<'_, R>) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    if let Some((_, _, size)) = id3_header(&f.read(0, 10)?) {
        id3v2(&f.read(0, 10 + size as u64)?, 0, size <= MAX_REPAIR, &mut out);
    }
    Ok(out)
}

// The walk's byte inputs (TagScan's ByteIn): positions are the tag's, and
// one input reads through another.

trait In {
    /// The next byte; None at the end.
    fn get(&mut self) -> Option<u8>;
    /// The bytes left (an upper bound when unsynchronised).
    fn left(&self) -> usize;
    /// Where the next stored byte is.
    fn pos(&self) -> usize;
    /// Steps over `n` bytes; false when the end came first.
    fn skip(&mut self, n: usize) -> bool {
        (0..n).all(|_| self.get().is_some())
    }
    fn fill(&mut self, out: &mut [u8]) -> bool {
        out.iter_mut().all(|b| self.get().map(|v| *b = v).is_some())
    }
}

/// The stored bytes from `pos` to `end`.
struct Plain<'a> {
    t: &'a [u8],
    pos: usize,
    end: usize,
}

impl In for Plain<'_> {
    fn get(&mut self) -> Option<u8> {
        let b = *self.t[..self.end].get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn left(&self) -> usize {
        self.end - self.pos
    }
    fn pos(&self) -> usize {
        self.pos
    }
    fn skip(&mut self, n: usize) -> bool {
        let fits = n <= self.end - self.pos;
        self.pos = if fits { self.pos + n } else { self.end };
        fits
    }
}

/// ID3 unsynchronisation undone: after an FF, a 00 is dropped (lofty's
/// UnsynchronizedStream). The 00 after the last FF read is taken only by
/// the next read.
struct Unsync<'a> {
    raw: &'a mut dyn In,
    ff: bool,
}

impl In for Unsync<'_> {
    fn get(&mut self) -> Option<u8> {
        let mut b = self.raw.get()?;
        if self.ff && b == 0 {
            let Some(next) = self.raw.get() else {
                self.ff = false;
                return None;
            };
            b = next;
        }
        self.ff = b == 0xFF;
        Some(b)
    }
    fn left(&self) -> usize {
        self.raw.left()
    }
    fn pos(&self) -> usize {
        self.raw.pos()
    }
}

/// At most `n` bytes of another input: a frame.
struct Limit<'a> {
    inner: &'a mut dyn In,
    n: usize,
}

impl Limit<'_> {
    fn finish(&mut self) -> bool {
        self.skip(self.n)
    }
}

impl In for Limit<'_> {
    fn get(&mut self) -> Option<u8> {
        if self.n == 0 {
            return None;
        }
        let b = self.inner.get();
        self.n = if b.is_some() { self.n - 1 } else { 0 };
        b
    }
    fn left(&self) -> usize {
        self.inner.left().min(self.n)
    }
    fn pos(&self) -> usize {
        self.inner.pos()
    }
    fn skip(&mut self, n: usize) -> bool {
        if n > self.n {
            self.inner.skip(self.n);
            self.n = 0;
            return false;
        }
        self.n -= n;
        self.inner.skip(n)
    }
}

/// One tag's pictures. `t` is the tag as stored (its header and body, cut
/// where the file ends) at file offset `at`; `repairs` when mStream's
/// repair pass reads it: a v2.4 size that isn't syncsafe read where it
/// lands on a frame (rule 6), and the walk's end at a frame past the
/// tag's (rule 4).
fn id3v2(t: &[u8], at: u64, mut repairs: bool, out: &mut Vec<Found>) {
    let (ver, flags, size) = (t[3], t[5], syncsafe(&t[6..10]));
    if size == 0 {
        return;
    }
    let mut end = t.len().min(10 + size);
    let mut pos = 10;
    // v2.2/2.3 tag-level unsynchronisation is read through: a frame's size
    // counts its bytes resynchronised.
    let whole = flags & 0x80 != 0 && ver < 4;
    if ver >= 3 && flags & 0x40 != 0 {
        // The extended header as lofty reads it: its size, two bytes for a
        // flag count and v2.4's flags, then the CRC and restrictions they
        // name (in v2.3 that leaves a padding size whose first byte is 0:
        // no frame is read). mStream's pass leaves such a tag alone.
        repairs = false;
        if ver == 3 && whole {
            return;
        }
        let Some(e) = t.get(pos..pos + 6) else { return };
        let ext = if ver == 3 { be32(e) } else { syncsafe(e) };
        if ext < 6 || ext >= size {
            return;
        }
        pos += 6 + if e[5] & 0x20 != 0 { 6 } else { 0 } + if e[5] & 0x10 != 0 { 2 } else { 0 };
        end = t.len().min(pos + (size - ext));
    }
    let mut raw = Plain { t, pos: pos.min(end), end };
    let mut un;
    let input: &mut dyn In = if whole {
        un = Unsync { raw: &mut raw, ff: false };
        &mut un
    } else {
        &mut raw
    };
    let hl = if ver == 2 { 6 } else { 10 };
    let mut fh = [0u8; 10];
    loop {
        let frame_at = input.pos();
        if !input.fill(&mut fh[..hl]) || fh[0] == 0 {
            break;
        }
        let (id, id_ok, mut size, fflags) = if ver == 2 {
            let size = (fh[3] as usize) << 16 | (fh[4] as usize) << 8 | fh[5] as usize;
            (&fh[..3], fh[..3].iter().all(|b| id_byte(*b)), size, 0)
        } else {
            // lofty trims trailing NULs from an id; a v2.3 tag may carry a
            // v2.2 id padded with one.
            let mut n = fh[..4].iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
            if ver == 3 && fh[3] == 0 {
                n = n.min(3);
            }
            let ok = n >= 3 && fh[..n].iter().all(|b| id_byte(*b));
            (&fh[..n], ok, be32(&fh[4..8]), u16::from_be_bytes([fh[8], fh[9]]))
        };
        if ver == 4 {
            let ss = syncsafe(&fh[4..8]);
            let plain = size;
            size = ss;
            if repairs && plain != ss {
                let low = fh[4..8].iter().all(|b| b & 0x80 == 0);
                let body = frame_at + 10;
                let ok_ss = low && boundary_ok(&t[..end], body + ss);
                let ok_be = boundary_ok(&t[..end], body.saturating_add(plain));
                if !ok_ss && ok_be {
                    size = plain;
                }
            }
        }
        // v2.2's ids, and a v2.3 tag's three-letter ones, as lofty upgrades
        // them: PIC is APIC, WXX is WXXX.
        let three = id.len() == 3 && ver <= 3;
        let apic = id == b"APIC" || three && id == b"PIC";
        let url = id.first() == Some(&b'W') && id != b"WXXX" && !(three && id == b"WXX");
        if !id_ok {
            // lofty, relaxed: a frame whose header it can't read is skipped
            // by its declared size.
            if !input.skip(size) {
                break;
            }
            continue;
        }
        if size == 0 {
            continue;
        }
        let truncated = !whole && size > input.left();
        let bit = |b: u16| fflags & b != 0;
        let (mut compressed, mut encrypted, mut grouping, mut unsync, mut dli) = Default::default();
        if ver == 3 {
            (compressed, encrypted, grouping) = (bit(0x80), bit(0x40), bit(0x20));
        } else if ver == 4 {
            (grouping, compressed, encrypted) = (bit(0x40), bit(0x08), bit(0x04));
            (unsync, dli) = (bit(0x02) || flags & 0x80 != 0, bit(0x01));
        }
        let lead = encrypted as usize + grouping as usize + if dli || compressed { 4 } else { 0 };
        let mut frame = Limit { inner: &mut *input, n: size };
        if lead > size {
            // lofty fails a tag with an undersized frame.
            frame.finish();
            continue;
        }
        if compressed || encrypted {
            // A compressed or encrypted picture is never elected (5.3).
            frame.finish();
        } else if apic {
            frame.skip(lead);
            let head = match unsync {
                true => apic_head(&mut Unsync { raw: &mut frame, ff: false }, ver),
                false => apic_head(&mut frame, ver),
            };
            if let Some((pic_type, data)) = head {
                // Its stored length: in a tag read through, known only by
                // reading through the picture.
                let stored = if whole {
                    frame.finish();
                    frame.pos()
                } else {
                    frame.pos() + frame.left()
                };
                let bytes = &t[data..stored];
                let (coding, fnv) = match whole || unsync {
                    true => (CODING_UNSYNC, fnv1a64(&deunsync(bytes))),
                    false => (CODING_RAW, fnv1a64(bytes)),
                };
                out.extend(Found::new(at + data as u64, bytes.len() as u64, coding, pic_type, fnv));
            }
            frame.finish();
        } else if url && !unsync {
            // lofty reads a URL to its first NUL and the next header from
            // there; mStream's pass drops a NUL in front of the URL first.
            frame.skip(lead);
            let content = frame.left();
            let mut first = true;
            let mut nul = false;
            while let Some(b) = frame.get() {
                if first && b == 0 && repairs && content > 1 {
                    first = false;
                    continue;
                }
                first = false;
                if b == 0 {
                    nul = true;
                    break;
                }
            }
            if !nul {
                frame.finish();
            }
        } else {
            frame.finish();
        }
        if truncated && repairs {
            // The pass cuts the frame at the tag's end, and nothing follows.
            break;
        }
    }
}

/// An APIC (v2.2 PIC) frame's head as lofty parses it: the text encoding,
/// the MIME (v2.2's three-letter format), the picture type, and the
/// description to its terminator (a NUL pair in UTF-16). The type, and
/// where the image data starts; None when the frame ends first.
fn apic_head(r: &mut dyn In, ver: u8) -> Option<(u8, usize)> {
    let encoding = r.get()?;
    if ver == 2 {
        r.skip(3).then_some(())?;
    } else {
        while r.get()? != 0 {}
    }
    let pic_type = r.get()?;
    if matches!(encoding, 1 | 2) {
        while (r.get()?, r.get()?) != (0, 0) {}
    } else {
        while r.get()? != 0 {}
    }
    Some((pic_type, r.pos()))
}

// ── FLAC and Opus ───────────────────────────────────────────────────────────

/// A FLAC's comment pictures, then its PICTURE blocks' (lofty's order),
/// the last comment block's alone, as lofty keeps it.
fn flac<R: Read + Seek>(f: &mut File<'_, R>) -> io::Result<Vec<Found>> {
    let mut at = 0;
    if let Some((ver, flags, size)) = id3_header(&f.read(0, 10)?) {
        at = id3_total(ver, flags, size);
    }
    let (mut comments, mut blocks) = (Vec::new(), Vec::new());
    let marker = f.read(at, 5)?;
    // fLaC, then STREAMINFO.
    if marker.len() < 5 || &marker[..4] != b"fLaC" || marker[4] & 0x7F != 0 {
        return Ok(blocks);
    }
    let mut block = at + 4;
    let mut first = true;
    loop {
        let h = f.read(block, 4)?;
        if h.len() < 4 {
            break;
        }
        let (last, kind) = (h[0] & 0x80 != 0, h[0] & 0x7F);
        let (body, len) = (block + 4, be32(&[0, h[1], h[2], h[3]]) as u64);
        if body + len > f.len {
            break;
        }
        if !first && len > 0 && matches!(kind, 4 | 6) {
            let c = f.read(body, len)?;
            if kind == 4 {
                // A later comment block replaces an earlier one (lofty,
                // relaxed).
                comments.clear();
                comment_pictures(&c, 0, |i| Some(body + i as u64), &mut comments);
            } else if let Some((pic_type, head, n)) = flac_picture(&c) {
                let fnv = fnv1a64(&c[head..head + n]);
                blocks.extend(Found::new(body + head as u64, n as u64, CODING_RAW, pic_type, fnv));
            }
        }
        first = false;
        block = body + len;
        if last {
            break;
        }
    }
    comments.extend(blocks);
    Ok(comments)
}

/// An Opus stream's comment pictures: its second packet, OpusTags, read
/// across its pages (RFC 3533), each page's part of it kept with where it
/// lies in the file.
fn opus<R: Read + Seek>(f: &mut File<'_, R>) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let h = f.read(0, 27)?;
    if h.len() < 27 || &h[..4] != b"OggS" || h[4] != 0 || h[26] == 0 {
        return Ok(out);
    }
    let serial = le32(&h[14..18]);
    let lacing = f.read(27, h[26] as u64)?;
    let mut page = 27 + h[26] as u64;
    if f.read(page, 8)? != b"OpusHead" {
        return Ok(out);
    }
    page += lacing.iter().map(|&l| l as u64).sum::<u64>();
    // The packet, and each page's part of it: (where it starts in the
    // packet, where in the file).
    let mut packet = Vec::new();
    let mut parts: Vec<(usize, u64)> = Vec::new();
    loop {
        let h = f.read(page, 27)?;
        if h.len() < 27 || &h[..4] != b"OggS" || h[4] != 0 || le32(&h[14..18]) != serial {
            break;
        }
        // The packet starts a page; its later pages continue it.
        if (h[5] & 1 != 0) == parts.is_empty() {
            break;
        }
        let segs = h[26] as u64;
        let lacing = f.read(page + 27, segs)?;
        if lacing.len() as u64 != segs {
            break;
        }
        let ends = lacing.iter().position(|&l| l < 255);
        let sum = |l: &[u8]| l.iter().map(|&l| l as u64).sum::<u64>();
        let part = sum(&lacing[..ends.map_or(lacing.len(), |e| e + 1)]);
        let at = page + 27 + segs;
        let bytes = f.read(at, part)?;
        let whole = bytes.len() as u64 == part;
        parts.push((packet.len(), at));
        packet.extend(bytes);
        if ends.is_some() || !whole {
            break;
        }
        page = at + sum(&lacing);
    }
    if !packet.starts_with(b"OpusTags") {
        return Ok(out);
    }
    // A byte at a page's end is followed by the next page's first: the
    // page header between them isn't the packet's.
    let file_at = |i: usize| {
        let k = parts.partition_point(|&(start, _)| start <= i).checked_sub(1)?;
        let (start, at) = parts[k];
        Some(at + (i - start) as u64)
    };
    comment_pictures(&packet, 8, file_at, &mut out);
    Ok(out)
}

/// The METADATA_BLOCK_PICTURE values of a comment packet `p` (its vendor
/// string at `from`): each the base64 of a FLAC PICTURE, anchored at its
/// first character and as long as its characters (2.6.4, picCoding 2).
/// `file_at` is where in the file a byte of `p` lies.
fn comment_pictures(
    p: &[u8],
    from: usize,
    file_at: impl Fn(usize) -> Option<u64>,
    out: &mut Vec<Found>,
) {
    let word = |i: usize| p.get(i..i + 4).map(le32);
    let Some(vendor) = word(from) else { return };
    let mut i = from + 4;
    if vendor > p.len() - i {
        return;
    }
    i += vendor;
    let Some(count) = word(i) else { return };
    i += 4;
    for _ in 0..count {
        let Some(len) = word(i) else { return };
        i += 4;
        if len > p.len() - i {
            return;
        }
        let comment = &p[i..i + len];
        let at = i;
        i += len;
        let Some(eq) = comment.iter().position(|&b| b == b'=') else { continue };
        if !comment[..eq].eq_ignore_ascii_case(b"METADATA_BLOCK_PICTURE") {
            continue;
        }
        // lofty decodes the whole value as strict base64.
        let value = &comment[eq + 1..];
        let Ok(block) = BASE64.decode(value) else { continue };
        let Some((pic_type, head, n)) = flac_picture(&block) else { continue };
        let Some(first) = file_at(at + eq + 1) else { continue };
        let fnv = fnv1a64(&block[head..head + n]);
        out.extend(Found::new(first, value.len() as u64, CODING_OGG_BASE64, pic_type, fnv));
    }
}

/// A FLAC PICTURE (a block's content, or a METADATA_BLOCK_PICTURE value
/// decoded) as lofty's from_flac_bytes reads it, a description that
/// doesn't fit before the rest left unskipped: its type, where its image
/// data starts, and how long that is; None when lofty drops it.
fn flac_picture(c: &[u8]) -> Option<(u8, usize, usize)> {
    if c.len() < 32 {
        return None;
    }
    let word = |i: usize| c.get(i..i + 4).map(be32);
    let (pic_type, mime) = (word(0)?, word(4)?);
    let mut rest = c.len() - 8;
    if mime > rest {
        return None;
    }
    std::str::from_utf8(&c[8..8 + mime]).ok()?;
    rest -= mime;
    let described = word(8 + mime)?;
    rest = rest.checked_sub(4)?;
    let skipped = if described > 0 && described < rest { described } else { 0 };
    rest = (rest - skipped).checked_sub(20)?;
    let n = word(8 + mime + 4 + skipped + 16)?;
    (n <= rest).then_some((pic_type as u8, 32 + mime + skipped, n))
}

// ── APEv2 ───────────────────────────────────────────────────────────────────

/// An MP3's APEv2 tag at its end (before an ID3v1 tag): its binary cover
/// items, each a file name to its NUL and then the image (lofty's
/// from_ape_bytes), under any case of its key.
fn ape<R: Read + Seek>(f: &mut File<'_, R>) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let len = f.len;
    let v1 = len >= 128 && f.read(len - 128, 3)? == b"TAG";
    let used = if v1 { 128 } else { 0 };
    if len < used + 32 {
        return Ok(out);
    }
    // The footer's magic, version, size and item count.
    let footer_at = len - used - 32;
    let footer = f.read(footer_at, 20)?;
    if &footer[..8] != b"APETAGEX" {
        return Ok(out);
    }
    let (size, count) = (le32(&footer[12..16]) as u64, le32(&footer[16..20]));
    if size < 32 || size > footer_at + 32 {
        return Ok(out);
    }
    let items_at = footer_at + 32 - size;
    let items = f.read(items_at, size - 32)?;
    let mut i = 0;
    for _ in 0..count {
        let Some(h) = items.get(i..i + 8) else { break };
        let (value_len, flags) = (le32(&h[..4]), le32(&h[4..]));
        let Some(nul) = items[i + 8..].iter().position(|&b| b == 0) else { break };
        let key = &items[i + 8..i + 8 + nul];
        let value_at = i + 8 + nul + 1;
        let Some(value) = items.get(value_at..value_at.saturating_add(value_len)) else { break };
        let cover = APE_PICTURE_TYPES.iter().find(|k| k.as_bytes().eq_ignore_ascii_case(key));
        if let Some(cover) = cover
            && (flags >> 1) & 3 == 1
            && let Some(name) = value.iter().position(|&b| b == 0)
        {
            let image = &value[name + 1..];
            let pic_type = PictureType::from_ape_key(cover).as_u8();
            let first = items_at + (value_at + name + 1) as u64;
            out.extend(Found::new(first, image.len() as u64, CODING_APE, pic_type, fnv1a64(image)));
        }
        i = value_at + value_len;
    }
    Ok(out)
}
