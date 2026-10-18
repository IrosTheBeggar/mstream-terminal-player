//! The card contract: the files the transfer keeps on a Core2's SD card,
//! byte for byte as the player reads them — the firmware repo's
//! docs/METADATA.md part 2 (v1), and docs/AUTODJ-INDEX.md for the Auto DJ
//! index. Pure code: every function takes bytes and gives bytes, and
//! nothing here opens a file. The player's own implementation is its
//! `lib/core/Card*`, and its `tools/card_fixtures.py` is a second one; the
//! fixtures that one makes (test/fixtures/card, copied from the firmware
//! repo) are what this third one is tested against.
//!
//! The pieces, with the contract's sections:
//! - [`prim`]: 2.3's conventions — CRC-32, FNV-1a 64 and the path hash,
//!   the quick fingerprint, the identifiers, the MD5 of mStream's `hash`;
//! - [`fat`]: 2.3.4, the FAT timestamp and the uniform-skew rule;
//! - [`text`]: 2.3.6 and 2.6.5, the text rules and the string run;
//! - [`numbers`]: part 5.3's number rule (BPM, ReplayGain, R128);
//! - [`order`]: names and the canonical order (2.6.8), the album folder
//!   (2.14.1);
//! - [`container`]: 2.4, the frame of every file but a thumbnail, its
//!   strict validation (2.4.3) and its writer;
//! - [`server`]: 2.5.1, 2.5.5 and 2.11 — serverUrlKey, the card's servers
//!   and the same-server test;
//! - one module a file: [`msmf`] (`manifest.bin`, the commit point, and
//!   which root counts), [`mptg`] (`tags-<gen>.bin`, the records and the
//!   ledger), [`mspd`] (`pending.bin`, a run's plan), [`mpdj`]
//!   (`autodj-<gen>.bin` read and checked; its builder is the Auto DJ
//!   writer's), [`mpth`] (the album thumbnails, from pixels or an image) and
//!   [`device_txt`] (`/.player/device.txt`, what the firmware reads).
//!
//! **Absent, not an error.** A reader treats a file that fails any of
//! 2.4.3's checks as absent: it says why with a [`Why`], the player's own
//! word for each check (`lib/core/CardContainer.cpp`'s `whyName`), and the
//! hardening files check that both sides refuse each broken file for the
//! same first reason. Readers take the whole file as one slice, never index
//! past what they have checked, and allocate in proportion to the slice;
//! [`MAX_FILE`] is how much of a file the I/O around them reads at most.

pub(crate) mod container;
pub(crate) mod device_txt;
pub(crate) mod fat;
pub(crate) mod mpdj;
pub(crate) mod mptg;
pub(crate) mod mpth;
pub(crate) mod msmf;
pub(crate) mod mspd;
pub(crate) mod numbers;
pub(crate) mod order;
pub(crate) mod prim;
pub(crate) mod server;
pub(crate) mod text;

#[cfg(test)]
mod tests;

/// The most of one contract file a reader is handed: past it the file is
/// treated as absent unread. The largest a writer makes is an Auto DJ index
/// of 65,535 rows at k = 169 (about 34 MB: AUTODJ-INDEX.md 10.2); a tags
/// file at the PSRAM ceiling's 27k tracks is about 9 MB (2.19).
pub(crate) const MAX_FILE: usize = 64 * 1024 * 1024;

/// Why a reader treats a file as absent (2.4.3, AUTODJ-INDEX.md 8), or
/// [`Why::Ok`]. The names are the player's (`whyName`), which the shared
/// hardening index lists for every file it breaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Why {
    Ok,
    // The player's word for a read that failed, kept with its list.
    #[allow(dead_code)]
    Io,
    Short,
    Magic,
    Major,
    HeaderBytes,
    SectionCount,
    FileBytes,
    HeaderCrc,
    Layout,
    UnknownRequired,
    Missing,
    Shape,
    SectionCrc,
    Counts,
    Enum,
    String,
    StringOrder,
    Name,
    PathLength,
    FoldRoot,
    FoldParent,
    FoldOrder,
    FirstRecord,
    RecsFolder,
    RecsOrder,
    Hidx,
    Orig,
    Comp,
    Libr,
    DjPaths,
    DjLayout,
    DjSelection,
    DjSlotCrc,
    DjSlotUnused,
    DjSlotIndex,
    DjSlotOrder,
    PendOrder,
    PendPath,
    PendOp,
}

impl Why {
    /// The player's word for it, as the hardening index spells it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Why::Ok => "ok",
            Why::Io => "io",
            Why::Short => "short",
            Why::Magic => "magic",
            Why::Major => "major",
            Why::HeaderBytes => "headerBytes",
            Why::SectionCount => "sectionCount",
            Why::FileBytes => "fileBytes",
            Why::HeaderCrc => "headerCrc",
            Why::Layout => "layout",
            Why::UnknownRequired => "unknownRequired",
            Why::Missing => "missing",
            Why::Shape => "shape",
            Why::SectionCrc => "sectionCrc",
            Why::Counts => "counts",
            Why::Enum => "enum",
            Why::String => "string",
            Why::StringOrder => "stringOrder",
            Why::Name => "name",
            Why::PathLength => "pathLength",
            Why::FoldRoot => "foldRoot",
            Why::FoldParent => "foldParent",
            Why::FoldOrder => "foldOrder",
            Why::FirstRecord => "firstRecord",
            Why::RecsFolder => "recsFolder",
            Why::RecsOrder => "recsOrder",
            Why::Hidx => "hidx",
            Why::Orig => "orig",
            Why::Comp => "comp",
            Why::Libr => "libr",
            Why::DjPaths => "djPaths",
            Why::DjLayout => "djLayout",
            Why::DjSelection => "djSelection",
            Why::DjSlotCrc => "djSlotCrc",
            Why::DjSlotUnused => "djSlotUnused",
            Why::DjSlotIndex => "djSlotIndex",
            Why::DjSlotOrder => "djSlotOrder",
            Why::PendOrder => "pendOrder",
            Why::PendPath => "pendPath",
            Why::PendOp => "pendOp",
        }
    }
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// What a writer refuses to write, and why: input that would make a file
/// its own reader treats as absent, or that breaks a writer rule. Writers
/// never write such a file; the message is for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WriteError(pub(crate) &'static str);

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for WriteError {}
