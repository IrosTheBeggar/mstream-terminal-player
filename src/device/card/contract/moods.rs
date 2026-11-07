//! The mood tail (MPTG minor 1, docs/METADATA.md 2.6.4; the player's
//! docs/MOOD-FILTERS.md 5.3): a track's mStream mood scores in bytes 72-79
//! of its RECS record, in rows of 80 bytes or more, from which the player
//! makes the track's mood word (METADATA 3.4.3) and filters its Albums and
//! Auto DJ by mood with no network. [`encode`] makes the tail a writer
//! stores from the server's row (`POST /api/v1/db/mood-scores`), and
//! [`read_tail`] is the tail a reader takes. The player's own are
//! `lib/core/MoodFilter.cpp`'s `tailOf` and `tailValid`; its
//! `tools/moodfilter.py`, which makes the fixtures, is a second.
//!
//! | Byte | Field |
//! |---|---|
//! | 72 | flags: bit 0 PRESENT; bit 1 sad beat happy; bit 2 aggressive beat relaxed; bits 3-7 are 0 |
//! | 73 | valence, the larger of happy and sad, 0-100; 255 if either is NULL |
//! | 74 | energy, the larger of relaxed and aggressive, the same |
//! | 75 | instrumental, 0-100; 255 NULL |
//! | 76 | acoustic, the same |
//! | 77 | party, the same |
//! | 78 | danceable, the same |
//! | 79 | reserved, 0 |
//!
//! 8 zero bytes are no server row. The tail is one-way: bits 3-7 and byte
//! 79 never get a meaning in major 1, and a later field goes from byte 80.

// The sync's overlay is `encode`'s caller, and lands after this; until then
// it has only the tests.
#![cfg_attr(not(test), allow(dead_code))]

/// mStream's mood heads in its `MOOD_COLUMNS` order (mStream
/// `src/db/mood-sync.js`): the slots of [`Scores`], by the indexes below.
pub(crate) const HEADS: [&str; 8] =
    ["danceable", "happy", "sad", "relaxed", "aggressive", "party", "acoustic", "instrumental"];
pub(crate) const DANCEABLE: usize = 0;
pub(crate) const HAPPY: usize = 1;
pub(crate) const SAD: usize = 2;
pub(crate) const RELAXED: usize = 3;
pub(crate) const AGGRESSIVE: usize = 4;
pub(crate) const PARTY: usize = 5;
pub(crate) const ACOUSTIC: usize = 6;
pub(crate) const INSTRUMENTAL: usize = 7;

/// A server row: each head's stored double, in [`HEADS`] order, `None` for
/// a NULL head (never 0).
pub(crate) type Scores = [Option<f64>; 8];

/// Where the tail starts in a record, and its length: rows of
/// `TAIL_AT + TAIL_BYTES` bytes or more carry one.
pub(crate) const TAIL_AT: usize = 72;
pub(crate) const TAIL_BYTES: usize = 8;

/// A record's bytes 72-79.
pub(crate) type Tail = [u8; TAIL_BYTES];

/// No server row (a track mStream hasn't analysed, an owned file with no
/// server track, a peer's track), and a record's tail when it has none.
pub(crate) const NO_ROW: Tail = [0; TAIL_BYTES];

/// A value byte's NULL.
pub(crate) const NULL: u8 = 255;

/// The flags byte's bits.
pub(crate) const PRESENT: u8 = 1 << 0;
pub(crate) const SAD_WINS: u8 = 1 << 1;
pub(crate) const AGGRESSIVE_WINS: u8 = 1 << 2;
/// Bits 3-7: 0, and a reader refuses a tail with one set.
const FLAGS_RESERVED: u8 = 0xF8;

// The tail's bytes, from its start.
const FLAGS: usize = 0;
const VALENCE: usize = 1;
const ENERGY: usize = 2;
const INSTRUMENTAL_AT: usize = 3;
const ACOUSTIC_AT: usize = 4;
const PARTY_AT: usize = 5;
const DANCEABLE_AT: usize = 6;
const RESERVED: usize = 7;

/// A score's value byte: `min(100, max(0, floor(score × 100 + 1e-9)))` in
/// f64, so 0.29 is 29 (its product is 28.999999999999996: a plain floor is
/// wrong at 0.29, 0.57 and 0.58), and a score outside 0-1 clamps. The slack
/// is exact on the server's grid of 4 places, so a correctly rounded parse
/// of its JSON can't move a byte. [`NULL`] for a NULL head.
pub(crate) fn value(score: Option<f64>) -> u8 {
    match score {
        Some(s) if !s.is_nan() => (s * 100.0 + 1e-9).floor().clamp(0.0, 100.0) as u8,
        _ => NULL,
    }
}

/// The tail a writer stores for a track: [`NO_ROW`] for `None`, no server
/// row. A scale's byte is the larger of its two heads, [`NULL`] when either
/// is NULL; its order bit comes from the raw doubles, set when sad beats
/// happy, aggressive beats relaxed (a tie is happy's, relaxed's), and is 0
/// when either head is NULL. A row of NULLs is `01 FF FF FF FF FF FF 00`:
/// analysed, unlike no row, so the player still counts it in an album.
pub(crate) fn encode(scores: Option<&Scores>) -> Tail {
    let Some(s) = scores else { return NO_ROW };
    let mut flags = PRESENT;
    let mut scale = |first: Option<f64>, second: Option<f64>, wins: u8| match (first, second) {
        (Some(a), Some(b)) if !a.is_nan() && !b.is_nan() => {
            if b > a {
                flags |= wins;
            }
            value(Some(a.max(b)))
        }
        _ => NULL,
    };
    let valence = scale(s[HAPPY], s[SAD], SAD_WINS);
    let energy = scale(s[RELAXED], s[AGGRESSIVE], AGGRESSIVE_WINS);
    let voice = value(s[INSTRUMENTAL]);
    [flags, valence, energy, voice, value(s[ACOUSTIC]), value(s[PARTY]), value(s[DANCEABLE]), 0]
}

/// The tail a reader takes from a record's bytes 72-79: `Some` only when
/// PRESENT is set, bits 3-7 are 0, byte 79 is 0 and each of bytes 73-78 is
/// 0-100 or [`NULL`]. Otherwise that record has no scores, and the record
/// and the file are fine: no check of 2.4.3 looks at the tail. A tail taken
/// is kept as it came, an order bit beside a NULL included (the player's
/// word ignores it; this writer never sets one).
pub(crate) fn read_tail(t: &[u8]) -> Option<Tail> {
    let t: Tail = t.try_into().ok()?;
    let flags = t[FLAGS] & PRESENT != 0 && t[FLAGS] & FLAGS_RESERVED == 0 && t[RESERVED] == 0;
    let values = [VALENCE, ENERGY, INSTRUMENTAL_AT, ACOUSTIC_AT, PARTY_AT, DANCEABLE_AT];
    (flags && values.iter().all(|&i| t[i] <= 100 || t[i] == NULL)).then_some(t)
}
