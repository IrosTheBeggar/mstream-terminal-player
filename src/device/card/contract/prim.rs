//! The conventions every contract file shares (docs/METADATA.md 2.3):
//! little-endian integers, CRC-32/ISO-HDLC, FNV-1a 64 and the path hash
//! over the literal `/music`, the quick fingerprint (qfp), the two kinds of
//! identifier, and the MD5 a download is checked against.

use md5::{Digest, Md5};

// ── Bytes (2.3.1) ───────────────────────────────────────────────────────────

/// A fourcc as the u32 its 4 ASCII bytes read as: `MPTG` is the bytes
/// 4D 50 54 47, the u32 0x4754504D.
pub(crate) const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*s)
}

// The readers' fixed-width loads. Every caller has checked the bytes are
// there (a section's length, a record's stride), so these index directly;
// the hardening tests and the mutation fuzz hold them to it.
pub(crate) fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

pub(crate) fn u64_at(b: &[u8], at: usize) -> u64 {
    u32_at(b, at) as u64 | (u32_at(b, at + 4) as u64) << 32
}

pub(crate) fn bytes16_at(b: &[u8], at: usize) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&b[at..at + 16]);
    out
}

/// A byte string identifier — an MD5 digest (`hash`, `audio-hash`), a
/// SHA-256 prefix, a UUID — from its hex text: its bytes in the text's
/// order, never an integer (2.3.1). 32 hex digits in either case; a UUID's
/// hyphens are dropped first (RFC 4122 order, not a Windows GUID's). None
/// for anything else.
pub(crate) fn hex16(text: &str) -> Option<[u8; 16]> {
    let digits: Vec<u8> = text.bytes().filter(|&c| c != b'-').collect();
    if digits.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in digits.chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi << 4 | lo) as u8;
    }
    Some(out)
}

/// The text of a byte string identifier: lowercase hex, as mStream writes
/// its digests.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── CRC-32 (2.3.2) ──────────────────────────────────────────────────────────

/// CRC-32/ISO-HDLC, as zlib and PNG compute it: the CRC of `123456789` is
/// 0xCBF43926. Not the Ogg page CRC, which is another.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// The CRC of what came before (whose CRC is `crc`) followed by `data`,
/// chained as zlib's `crc32(data, crc)`: an Auto DJ slot's CRC is seeded
/// with its row this way (AUTODJ-INDEX.md 4.3).
pub(crate) fn crc32_after(crc: u32, data: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new_with_initial(crc);
    h.update(data);
    h.finalize()
}

// ── FNV-1a 64 and the path hash (2.3.3) ─────────────────────────────────────

pub(crate) const FNV_BASIS: u64 = 0xCBF2_9CE4_8422_2325;
pub(crate) const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

/// FNV-1a 64: the path hash, qfp, artistKey and serverUrlKey.
pub(crate) fn fnv1a64(data: &[u8]) -> u64 {
    fnv1a64_from(FNV_BASIS, data)
}

/// FNV-1a 64 going on from the state `h`.
pub(crate) fn fnv1a64_from(mut h: u64, data: &[u8]) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// The hash of the literal `/music`, the root of every path hash, whatever
/// case the card stores that folder in (a card's `Music` still hashes as
/// `/music`).
pub(crate) const MUSIC_HASH: u64 = 0x75DC_8A6A_3868_7865;

/// The path hash of a path relative to `/music` (`""` is `/music` itself):
/// FNV-1a 64 of `/music`, then `/` and the path, every name spelled as the
/// card's directory stores it (2.8.5), no trailing slash. NFC and NFD
/// spellings of one name hash apart, which is why names are recorded as
/// stored.
pub(crate) fn path_hash(rel: &str) -> u64 {
    if rel.is_empty() {
        return MUSIC_HASH;
    }
    fnv1a64_from(fnv1a64_from(MUSIC_HASH, b"/"), rel.as_bytes())
}

/// The hash of `name` below a folder whose hash is `parent`.
pub(crate) fn child_hash(parent: u64, name: &str) -> u64 {
    fnv1a64_from(fnv1a64_from(parent, b"/"), name.as_bytes())
}

// ── The quick fingerprint (2.3.5) ───────────────────────────────────────────

/// The length of the head and of the tail the fingerprint reads.
pub(crate) const QFP_PART: u64 = 4096;

/// The two reads a fingerprint takes of a file of `size` bytes: the head
/// `[0, head)`, and the tail `[tail_offset, tail_offset + tail)` — empty
/// when the file is no longer than the head. They never overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QfpRanges {
    pub(crate) head: u64,
    pub(crate) tail_offset: u64,
    pub(crate) tail: u64,
}

pub(crate) fn qfp_ranges(size: u64) -> QfpRanges {
    let head = size.min(QFP_PART);
    let tail_offset = if size > QFP_PART { (size - QFP_PART).max(QFP_PART) } else { size };
    QfpRanges { head, tail_offset, tail: size - tail_offset }
}

/// qfp: FNV-1a 64 of the size as a little-endian u64, then the head, then
/// the tail ([`qfp_ranges`]: the caller reads them). Wrong lengths are the
/// caller's bug, and give a value that matches nothing.
pub(crate) fn qfp(size: u64, head: &[u8], tail: &[u8]) -> u64 {
    debug_assert_eq!((head.len() as u64, tail.len() as u64), {
        let r = qfp_ranges(size);
        (r.head, r.tail)
    });
    fnv1a64_from(fnv1a64_from(fnv1a64(&size.to_le_bytes()), head), tail)
}

/// qfp of a whole file held in memory.
pub(crate) fn qfp_of(data: &[u8]) -> u64 {
    let r = qfp_ranges(data.len() as u64);
    let tail = &data[r.tail_offset as usize..];
    qfp(data.len() as u64, &data[..r.head as usize], tail)
}

// ── MD5 (2.6.6) ─────────────────────────────────────────────────────────────

/// ORIG's HASH_SAMPLED: mStream's hashes are sampled digests — not the MD5
/// of the whole file — exactly when its `hash-v` is 2 or more and the file
/// is 25 MiB (26,214,400 bytes) or more (its rust-parser's
/// `file_size >= sample_threshold`). Only an unsampled `hash` can make a
/// download VERIFIED.
pub(crate) fn hash_sampled(hash_v: u16, file_size: u64) -> bool {
    hash_v >= 2 && file_size >= 26_214_400
}

/// The MD5 of a download, fed as it arrives, to check against mStream's
/// `hash` (2.12.1, step 8.2).
#[derive(Clone, Default)]
pub(crate) struct Md5Sum(Md5);

impl Md5Sum {
    pub(crate) fn new() -> Self {
        Md5Sum(Md5::new())
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub(crate) fn finish(self) -> [u8; 16] {
        self.0.finalize().into()
    }
}

/// The MD5 of bytes held whole.
pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    let mut sum = Md5Sum::new();
    sum.update(data);
    sum.finish()
}
