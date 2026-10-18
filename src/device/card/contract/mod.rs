//! The card contract: the files the transfer keeps on a Core2's SD card,
//! byte for byte as the player reads them — the firmware repo's
//! docs/METADATA.md part 2 (v1), and docs/AUTODJ-INDEX.md for the Auto DJ
//! index. Pure code: every function takes bytes and gives bytes, and
//! nothing here opens a file. The player's own implementation is its
//! `lib/core/Card*`, and its `tools/card_fixtures.py` is a second one; the
//! fixtures that one makes (test/fixtures/card, copied from the firmware
//! repo) are what this third one is tested against.
//!
//! So far its primitives, with the contract's sections:
//! - [`prim`]: 2.3's conventions — CRC-32, FNV-1a 64 and the path hash,
//!   the quick fingerprint, the identifiers, the MD5 of mStream's `hash`;
//! - [`fat`]: 2.3.4, the FAT timestamp and the uniform-skew rule;
//! - [`text`]: 2.3.6 and 2.6.5, the text rules and the string run;
//! - [`numbers`]: part 5.3's number rule (BPM, ReplayGain, R128);
//! - [`order`]: names and the canonical order (2.6.8), the album folder
//!   (2.14.1).

pub(crate) mod fat;
pub(crate) mod numbers;
pub(crate) mod order;
pub(crate) mod prim;
pub(crate) mod text;

#[cfg(test)]
mod tests;
