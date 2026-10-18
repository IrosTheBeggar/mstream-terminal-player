//! The container (docs/METADATA.md 2.4): the one frame of MSMF, MPTG, MSPD
//! and MPDJ — a 40-byte common header, the type's own header, a directory
//! of 32-byte entries, then the sections, each 8-byte aligned.
//!
//! [`Container::open`] runs 2.4.3's checks of the frame and the directory,
//! in the player's order (`lib/core/CardContainer.cpp`), so the first one a
//! file fails is the reason both sides give: anything invalid is absent. A
//! format's own checks — its header's fields, its sections' structure and
//! CRCs — are its module's. An unknown optional section is skipped; an
//! unknown REQUIRED one makes the file absent (2.4.5). [`write`] lays a
//! file out as 2.4.2 says two writers must, so the same sections give the
//! same bytes.

use super::prim::{crc32, fourcc, u16_at, u32_at, u64_at};
use super::{WriteError, Why};

pub(crate) const COMMON_HEADER: u32 = 40;
pub(crate) const DIR_ENTRY: u32 = 32;
pub(crate) const MAX_SECTIONS: u32 = 64;
/// A directory entry's flag bit 0: a reader that doesn't know the type
/// MUST treat the file as absent.
pub(crate) const REQUIRED: u32 = 1;

pub(crate) const MSMF: u32 = fourcc(b"MSMF");
pub(crate) const MPTG: u32 = fourcc(b"MPTG");
pub(crate) const MSPD: u32 = fourcc(b"MSPD");
pub(crate) const MPDJ: u32 = fourcc(b"MPDJ");
pub(crate) const MPTH: u32 = fourcc(b"MPTH");
pub(crate) const STRS: u32 = fourcc(b"STRS");

/// The common header (2.4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) magic: u32,
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) header_bytes: u32,
    pub(crate) section_count: u32,
    pub(crate) file_bytes: u32,
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
    pub(crate) header_crc: u32,
}

impl Header {
    /// The common header's fields, from a file's first 40 bytes; unchecked.
    pub(crate) fn parse(b: &[u8]) -> Option<Header> {
        (b.len() >= COMMON_HEADER as usize).then(|| Header {
            magic: u32_at(b, 0),
            major: u16_at(b, 4),
            minor: u16_at(b, 6),
            header_bytes: u32_at(b, 8),
            section_count: u32_at(b, 12),
            file_bytes: u32_at(b, 16),
            generation: u32_at(b, 20),
            card_id: u64_at(b, 24),
            header_crc: u32_at(b, 32),
        })
    }
}

/// One directory entry (2.4.2), and its place in the directory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Section {
    pub(crate) kind: u32,
    pub(crate) flags: u32,
    pub(crate) offset: u32,
    pub(crate) bytes: u32,
    pub(crate) count: u32,
    pub(crate) stride: u32,
    pub(crate) crc: u32,
    pub(crate) index: u32,
}

/// A section a format defines: its type, its v1 stride (0 for a blob) and
/// whether every file has it.
pub(crate) struct SectionSpec {
    pub(crate) kind: u32,
    pub(crate) stride: u32,
    pub(crate) required: bool,
}

/// A format as its reader knows it: the magic, the major it reads, its v1
/// header's length, and its sections in its table's order.
pub(crate) struct FormatSpec {
    pub(crate) magic: u32,
    pub(crate) major: u16,
    pub(crate) header_bytes: u32,
    pub(crate) sections: &'static [SectionSpec],
}

/// A file whose frame and directory passed 2.4.3: its header, and where
/// each section its format knows lies (none for an absent optional one).
pub(crate) struct Container<'a> {
    bytes: &'a [u8],
    header: Header,
    known: Vec<Option<Section>>,
    type_header: &'a [u8],
}

impl<'a> Container<'a> {
    /// 2.4.3's frame and directory, in the player's order: the length, the
    /// magic, the major, headerBytes, sectionCount, the directory's end
    /// within the file, fileBytes, headerCrc; then each entry — aligned, at
    /// or after the end of the one before and less than 8 bytes past it,
    /// inside the file, a type once — an unknown REQUIRED type, a known
    /// one's shape; then the last section's end and the sections the format
    /// requires. Sums and products are taken in 64 bits, so no crafted u32
    /// field wraps past a check.
    pub(crate) fn open(bytes: &'a [u8], spec: &FormatSpec) -> Result<Container<'a>, Why> {
        let size = bytes.len() as u64;
        let Some(header) = Header::parse(bytes) else { return Err(Why::Short) };
        if header.magic != spec.magic {
            return Err(Why::Magic);
        }
        if header.major != spec.major {
            return Err(Why::Major);
        }
        if header.header_bytes % 8 != 0 || header.header_bytes < spec.header_bytes {
            return Err(Why::HeaderBytes);
        }
        if header.section_count > MAX_SECTIONS {
            return Err(Why::SectionCount);
        }
        let dir_end = header.header_bytes as u64 + DIR_ENTRY as u64 * header.section_count as u64;
        if dir_end > size {
            return Err(Why::Short);
        }
        if header.file_bytes as u64 != size {
            return Err(Why::FileBytes);
        }
        let dir_end = dir_end as usize;
        // The header's CRC, over the header and the directory with its own
        // four bytes taken as zero.
        let mut h = crc32fast::Hasher::new();
        h.update(&bytes[..32]);
        h.update(&[0; 4]);
        h.update(&bytes[36..dir_end]);
        if h.finalize() != header.header_crc {
            return Err(Why::HeaderCrc);
        }

        let mut known: Vec<Option<Section>> = vec![None; spec.sections.len()];
        let mut types: Vec<u32> = Vec::with_capacity(header.section_count as usize);
        let mut end = dir_end as u64;
        for i in 0..header.section_count {
            let at = header.header_bytes as usize + (DIR_ENTRY * i) as usize;
            let e = &bytes[at..at + DIR_ENTRY as usize];
            let s = Section {
                kind: u32_at(e, 0),
                flags: u32_at(e, 4),
                offset: u32_at(e, 8),
                bytes: u32_at(e, 12),
                count: u32_at(e, 16),
                stride: u32_at(e, 20),
                crc: u32_at(e, 24),
                index: i,
            };
            let offset = s.offset as u64;
            if offset % 8 != 0 || offset < end || offset - end >= 8 {
                return Err(Why::Layout);
            }
            let s_end = offset + s.bytes as u64;
            if s_end > header.file_bytes as u64 || types.contains(&s.kind) {
                return Err(Why::Layout);
            }
            types.push(s.kind);
            end = s_end;
            let Some(k) = spec.sections.iter().position(|ks| ks.kind == s.kind) else {
                if s.flags & REQUIRED != 0 {
                    return Err(Why::UnknownRequired);
                }
                continue;
            };
            let ks = &spec.sections[k];
            if ks.stride == 0 {
                if s.count != 0 || s.stride != 0 {
                    return Err(Why::Shape);
                }
            } else if s.stride < ks.stride || s.count as u64 * s.stride as u64 != s.bytes as u64 {
                return Err(Why::Shape);
            }
            known[k] = Some(s);
        }
        if end != header.file_bytes as u64 {
            return Err(Why::Layout);
        }
        if spec.sections.iter().zip(&known).any(|(ks, s)| ks.required && s.is_none()) {
            return Err(Why::Missing);
        }
        let type_header = &bytes[COMMON_HEADER as usize..spec.header_bytes as usize];
        Ok(Container { bytes, header, known, type_header })
    }

    pub(crate) fn header(&self) -> &Header {
        &self.header
    }

    /// The type-specific header as far as this reader's format knows it
    /// (a later minor's longer header is read by its prefix: 2.4.5).
    pub(crate) fn type_header(&self) -> &'a [u8] {
        self.type_header
    }

    /// The format's section `k` (its index in the format's table), when
    /// the file has it.
    pub(crate) fn section(&self, k: usize) -> Option<Section> {
        self.known.get(k).copied().flatten()
    }

    /// A section's bytes.
    pub(crate) fn data(&self, s: &Section) -> &'a [u8] {
        &self.bytes[s.offset as usize..(s.offset + s.bytes) as usize]
    }

    /// The first `len` bytes of row `i` of an array section: the prefix a
    /// reader knows of a row that a later minor may have made longer.
    pub(crate) fn row(&self, s: &Section, i: u32, len: usize) -> &'a [u8] {
        let at = s.offset as usize + i as usize * s.stride as usize;
        &self.bytes[at..at + len]
    }

    pub(crate) fn crc_ok(&self, s: &Section) -> bool {
        crc32(self.data(s)) == s.crc
    }

    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

/// 2.4.5's writer guard: the software MUST NOT write to a card where any
/// contract file in `/.mstream` carries a known magic with a major it
/// doesn't know (MPDJ: it knows 1, the retired draft, and 2). Read from the
/// file's first 6 bytes alone, before any CRC, so a newer writer's
/// half-finished commit stops it too. Unknown magics and short files say
/// nothing.
pub(crate) fn unknown_major(head: &[u8]) -> bool {
    if head.len() < 6 {
        return false;
    }
    let major = u16_at(head, 4);
    match u32_at(head, 0) {
        MPDJ => major != 1 && major != 2,
        MSMF | MPTG | MSPD => major != 1,
        _ => false,
    }
}

/// What a writer gives the frame: the values a writer would otherwise
/// choose (the generation, the card's id) come from its caller (2.4.2).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FileMeta {
    pub(crate) magic: u32,
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) generation: u32,
    pub(crate) card_id: u64,
}

/// One section to write.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SectionOut<'s> {
    pub(crate) kind: u32,
    pub(crate) flags: u32,
    pub(crate) count: u32,
    pub(crate) stride: u32,
    pub(crate) data: &'s [u8],
}

/// A file, laid out as 2.4.2 says: the common header, `type_header` and
/// zeros up to `header_bytes` (a multiple of 8; MPDJ pads further so its
/// first section starts on a slot: AUTODJ-INDEX.md 3.3), the directory in
/// the order given — the format's table order — then each section at the
/// end of the one before rounded up to 8, the gaps zero; every CRC, and
/// headerCrc last.
pub(crate) fn write(
    meta: &FileMeta,
    type_header: &[u8],
    header_bytes: u32,
    sections: &[SectionOut],
) -> Result<Vec<u8>, WriteError> {
    // The longest header any format has is MPDJ's 104 bytes, padded to a
    // 512-byte slot: past a few KB it is a caller's bug.
    let short = (header_bytes as usize) < COMMON_HEADER as usize + type_header.len();
    if short || header_bytes % 8 != 0 || header_bytes > 4096 {
        return Err(WriteError("the header's length"));
    }
    if sections.len() > MAX_SECTIONS as usize {
        return Err(WriteError("more than 64 sections"));
    }
    for (i, s) in sections.iter().enumerate() {
        let shaped = match s.stride {
            0 => s.count == 0,
            stride => s.count as u64 * stride as u64 == s.data.len() as u64,
        };
        if !shaped {
            return Err(WriteError("a section's count and stride don't make its length"));
        }
        if sections[..i].iter().any(|t| t.kind == s.kind) {
            return Err(WriteError("a section type twice"));
        }
    }
    let n = sections.len() as u64;
    let mut at = header_bytes as u64 + DIR_ENTRY as u64 * n;
    let mut offsets = Vec::with_capacity(sections.len());
    for s in sections {
        at = (at + 7) & !7;
        offsets.push(at);
        at += s.data.len() as u64;
    }
    if at > u32::MAX as u64 {
        return Err(WriteError("the file would pass 4 GiB"));
    }
    let mut out = vec![0u8; at as usize];
    out[..4].copy_from_slice(&meta.magic.to_le_bytes());
    out[4..6].copy_from_slice(&meta.major.to_le_bytes());
    out[6..8].copy_from_slice(&meta.minor.to_le_bytes());
    out[8..12].copy_from_slice(&header_bytes.to_le_bytes());
    out[12..16].copy_from_slice(&(n as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(at as u32).to_le_bytes());
    out[20..24].copy_from_slice(&meta.generation.to_le_bytes());
    out[24..32].copy_from_slice(&meta.card_id.to_le_bytes());
    out[40..40 + type_header.len()].copy_from_slice(type_header);
    for (i, (s, &o)) in sections.iter().zip(&offsets).enumerate() {
        let o = o as usize;
        out[o..o + s.data.len()].copy_from_slice(s.data);
        let e = header_bytes as usize + DIR_ENTRY as usize * i;
        let fields = [s.kind, s.flags, o as u32, s.data.len() as u32, s.count, s.stride, crc32(s.data), 0];
        for (k, v) in fields.iter().enumerate() {
            out[e + 4 * k..e + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    let dir_end = header_bytes as usize + DIR_ENTRY as usize * sections.len();
    let crc = crc32(&out[..dir_end]);
    out[32..36].copy_from_slice(&crc.to_le_bytes());
    Ok(out)
}

/// A string section being written (2.4.4): byte 0 is NUL, so offset 0 is
/// the empty string; each non-empty string gets its own copy and a NUL, in
/// the order it is added — the order its format's readers check.
pub(crate) struct Strs(Vec<u8>);

impl Strs {
    pub(crate) fn new() -> Strs {
        Strs(vec![0])
    }

    /// A string's offset, 0 for the empty string (never written).
    pub(crate) fn add(&mut self, s: &[u8]) -> u32 {
        if s.is_empty() {
            return 0;
        }
        let at = self.0.len() as u32;
        self.0.extend_from_slice(s);
        self.0.push(0);
        at
    }

    /// Raw bytes (a record's run, whose NULs are its own): their offset.
    pub(crate) fn raw(&mut self, b: &[u8]) -> u32 {
        let at = self.0.len() as u32;
        self.0.extend_from_slice(b);
        at
    }

    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A string section being read front to back (2.4.3): every string a
/// reader uses starts at or after the end of the one before it in 2.4.4's
/// order, and has its NUL inside the part being read (`[.., end)`). The
/// player reads STRS as two such runs at once.
pub(crate) struct StrCursor<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> StrCursor<'a> {
    /// The part `[start, end)` of a section's bytes `data`.
    pub(crate) fn new(data: &'a [u8], start: usize, end: usize) -> StrCursor<'a> {
        let end = end.min(data.len());
        StrCursor { data, pos: start.min(end), end }
    }

    pub(crate) fn position(&self) -> usize {
        self.pos
    }

    /// The byte at the cursor (byte 0 of a section: its NUL), moving past
    /// it.
    pub(crate) fn byte(&mut self) -> Option<u8> {
        let b = *self.data[..self.end].get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    /// The byte at `off`, at or after the cursor and before the part's end,
    /// moving past it: a run's field count.
    pub(crate) fn byte_at(&mut self, off: usize) -> Option<u8> {
        if off < self.pos || off >= self.end {
            return None;
        }
        self.pos = off + 1;
        Some(self.data[off])
    }

    /// The string at `off`, which the caller has checked is inside the
    /// section and not before [`position`](Self::position): its bytes, the
    /// cursor moved past its NUL. [`Why::String`] when it has no NUL before
    /// the part's end.
    pub(crate) fn string_at(&mut self, off: usize) -> Result<&'a [u8], Why> {
        if off < self.pos || off >= self.end {
            return Err(Why::String);
        }
        let Some(n) = self.data[off..self.end].iter().position(|&c| c == 0) else { return Err(Why::String) };
        self.pos = off + n + 1;
        Ok(&self.data[off..off + n])
    }

    /// The next string at the cursor.
    pub(crate) fn next_string(&mut self) -> Result<&'a [u8], Why> {
        self.string_at(self.pos)
    }

    /// The common read of 2.4.4's strings: 0 is the empty string; any other
    /// offset inside the section ([`Why::String`] otherwise), at or after
    /// the cursor ([`Why::StringOrder`]), with its NUL inside.
    pub(crate) fn ordered(&mut self, off: u32) -> Result<&'a [u8], Why> {
        let off = off as usize;
        if off == 0 {
            return Ok(&[]);
        }
        if off >= self.data.len() {
            return Err(Why::String);
        }
        if off < self.pos {
            return Err(Why::StringOrder);
        }
        self.string_at(off)
    }
}
