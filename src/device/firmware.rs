//! Where the firmware comes from, and what it is. An image is a source and
//! a flash mode: the release this build of the player pins (tag and both
//! checksums written here, bumped by hand when a player release adopts a
//! firmware release), another release by its tag (downloaded from the
//! firmware repo's GitHub Releases and checked against that release's own
//! `SHA256SUMS`), or a local file (`--firmware`, or the MP3 Player tab's
//! Advanced options: the merged `firmware.factory.bin` of a build, or a
//! release's `*-full.bin`), whose mode is its own. A GitHub release's files
//! can be replaced after the fact, so the pin's checksums, not the tag, are
//! what the player trusts; another release is as trusted as its
//! `SHA256SUMS`, kept beside the image so a release downloaded once can be
//! written offline; a local build is trusted only to be ours.
//!
//! Every image carries its own name and version: ESP-IDF's app description
//! (`esp_app_desc_t`) sits at a fixed offset of the app, and the firmware
//! fills it with its git version and the project name. The same 256 bytes,
//! read back from a board, say what is on it (see engine::Link::app_desc).
//! An image's flash mode is in its bootloader's header ([`Mode::of_header`]);
//! a board's is in its ELF id, against every release's pair ([`elf_mode`]).
//!
//! What the player asks the network for goes through [`Supply`]: the real
//! one ([`Net`]) is GitHub and the cache directory, and the tests hand the
//! worker a shelf of their own — no test reaches GitHub.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rust_i18n::t;
use sha2::{Digest, Sha256};

use super::DeviceError;

/// The firmware's repository, and the stem its release files share.
const REPO: &str = "IrosTheBeggar/mstream-mp3-player";
const ASSET_STEM: &str = "mstream-player-core2";
/// The release this player pins: what `device flash` writes when no flag
/// says otherwise, and what a board's firmware is measured against ("up to
/// date" is this, exactly, whatever image is chosen for a write). Bumped by
/// hand with player releases, once a firmware release has been tried on a
/// board.
pub(crate) const PINNED_TAG: Option<&str> = Some("v0.8.0");
/// The sha256 of that release's `*-full.bin` (the QIO build), from its
/// SHA256SUMS (and the same as GitHub's own digest of the asset, checked
/// when it was pinned). Release assets are mutable on GitHub; this is the
/// trust anchor, not the file.
pub(crate) const PINNED_FULL_SHA256: Option<&str> =
    Some("ba77f290f7159b2e40a7d0907a05b1400ed6529abe297437595259ed8d0671a3");
/// The sha256 of that release's `*-dio-full.bin`: the same firmware with
/// the flash in DIO, for a Core2 that keeps restarting on QIO. From v0.8.0's
/// SHA256SUMS and GitHub's digest of the asset (both `01de38ec…`, checked
/// 2026-10-10); bumped with the pin, or none for a pin with no DIO build.
pub(crate) const PINNED_DIO_SHA256: Option<&str> =
    Some("01de38ec86dbad5b37b278ec30a159c93fede0a7a834e12ab8a6202cf5a3af8e");
/// Whether a board running the pinned release answers `@status` — the
/// running firmware's report of its card (the firmware repo's
/// docs/HOST-STATUS.md: the release after v0.8.0). v0.8.0 does not: it
/// says `@err 7 status`, so its board shows its version but not its card,
/// and "update to see the card" would be a promise the update does not
/// keep. Flipped with the pin, when the pin moves to a release that
/// answers.
pub(crate) const PINNED_ANSWERS_STATUS: bool = false;

/// Where the merged image expects the app: ota_0 in the firmware's
/// partition table, and where the bootloader at 0x1000 sits inside it.
pub(crate) const APP_OFFSET: usize = 0x10000;
pub(crate) const BOOTLOADER_OFFSET: usize = 0x1000;
/// The first byte of every ESP image header.
const IMAGE_MAGIC: u8 = 0xE9;
/// A Core2 has 16 MB of flash; an image past this is not one of ours, and
/// a download past it is a lie or a mistake — stop reading either way.
const MAX_IMAGE: usize = 8 * 1024 * 1024;
const MAX_SUMS: usize = 64 * 1024;
/// GitHub's answer for a hundred releases with their assets is a few
/// hundred KB; past this it is not the list.
const MAX_LIST: usize = 4 * 1024 * 1024;
/// One page of the releases API holds every release the firmware has had
/// and will have for a long while; one request is the whole list.
const LIST_PAGE: usize = 100;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The longest a download goes on with no byte coming. The MP3 Player page
/// takes the gate's yes while the image still downloads, and is locked from
/// that yes until the write ends (the screen's contract, clause 9): a
/// connection that stalls must fail, failing the write that waits for it
/// with nothing touched, rather than hold the page for good.
const READ_STALL: Duration = Duration::from_secs(30);
/// Where the releases API answers, unless `MSTREAM_FIRMWARE_API` points
/// the player at a fake one (a smoke's).
const GITHUB_API: &str = "https://api.github.com";

// ── Modes ───────────────────────────────────────────────────────────────────

/// How an image drives the flash chip. Every release since v0.6.0 has two
/// builds of one version: QIO reads the flash on four data lines (the
/// lists, the dance and MP3 decoding are faster), DIO on two, as M5Stack
/// ships the Core2 — a little slower, and it starts on every unit. QIO is
/// the default; DIO is for a Core2 that keeps restarting on it. v0.5.0 and
/// before have one image, and it is DIO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, clap::ValueEnum)]
pub(crate) enum Mode {
    Qio,
    Dio,
}

impl Mode {
    /// `QIO` / `DIO`: the word the log, the command line and the release
    /// files use — never shown to a listener without its meaning beside it.
    pub fn word(self) -> &'static str {
        match self {
            Mode::Qio => "QIO",
            Mode::Dio => "DIO",
        }
    }

    /// The mode an image header says, read from its fourth byte: the flash
    /// size in the high nibble, the flash clock in the low one. The
    /// firmware's two builds differ in the clock — the QIO build runs the
    /// flash at 80 MHz (`F`), the DIO build at 40 MHz (`0`) — while the mode
    /// byte before it reads DIO in both (the bootloader starts in DIO) and
    /// proves nothing. Read on 2026-10-10: v0.5.0 `E9 03 02 40` (DIO, its
    /// one image), v0.7.0 and v0.8.0 `E9 03 02 4F`, v0.8.0's
    /// `-dio-full.bin` `E9 03 02 40`. Any other clock is no mode the
    /// player knows, and is said as such — never guessed.
    pub fn of_header(header: &[u8]) -> Option<Mode> {
        match header {
            [IMAGE_MAGIC, _, _, byte, ..] => match byte & 0x0F {
                0x0F => Some(Mode::Qio),
                0x00 => Some(Mode::Dio),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The flash clock an image header asks for, in MHz (ESP-IDF's encoding of
/// the low nibble of its fourth byte).
pub(crate) fn header_mhz(header: &[u8]) -> Option<u32> {
    match header.get(3).map(|b| b & 0x0F)? {
        0x0 => Some(40),
        0x1 => Some(26),
        0x2 => Some(20),
        0xF => Some(80),
        _ => None,
    }
}

// ── The images ──────────────────────────────────────────────────────────────

/// What to write: a source and, for a release, the build. A local file's
/// mode is its own, read from its header once it is read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Image {
    /// This player's release, checked by the checksums built into it.
    Pin(Mode),
    /// Another release, checked by its own SHA256SUMS.
    Release { tag: String, mode: Mode },
    /// A file, or a build folder's merged image.
    Local(PathBuf),
}

impl Image {
    /// A release by its tag — the pin when the tag is the pin's: its
    /// checksums are built in, so it is written as the pin (offline too).
    pub fn release(tag: &str, mode: Mode) -> Image {
        match PINNED_TAG {
            Some(pin) if pin == tag.trim() => Image::Pin(mode),
            _ => Image::Release { tag: tag.trim().to_string(), mode },
        }
    }

    /// What `device flash`'s flags ask for, or none without any: a path
    /// outranks a tag, and the mode applies to the pin or the tag (a file
    /// has its own, checked against the flag before anything starts). A tag
    /// with no mode is its release's own image: QIO where it has both, the
    /// one it has otherwise (v0.5.0's is DIO) — until a board's ELF says
    /// it runs DIO, when the worker moves that board's copy to DIO
    /// (desk::Setup::flags_mode).
    pub fn from_flags(firmware: Option<PathBuf>, release: Option<String>, mode: Option<Mode>) -> Option<Image> {
        match (firmware, release, mode) {
            (Some(path), _, _) => Some(Image::Local(path)),
            (None, Some(tag), mode) => {
                let own = match known_images(tag.trim()) {
                    Some(Images::DioOnly) => Mode::Dio,
                    _ => Mode::Qio,
                };
                Some(Image::release(&tag, mode.unwrap_or(own)))
            }
            (None, None, Some(mode)) => Some(Image::Pin(mode)),
            (None, None, None) => None,
        }
    }

    /// The build asked for: none for a local file.
    pub fn mode(&self) -> Option<Mode> {
        match self {
            Image::Pin(mode) | Image::Release { mode, .. } => Some(*mode),
            Image::Local(_) => None,
        }
    }

    /// The release's tag: the pin's, another's, or none for a file.
    pub fn tag(&self) -> Option<&str> {
        match self {
            Image::Pin(_) => PINNED_TAG,
            Image::Release { tag, .. } => Some(tag),
            Image::Local(_) => None,
        }
    }

    pub fn is_pin(&self) -> bool {
        matches!(self, Image::Pin(_))
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Image::Local(_))
    }

    /// The same source in another build: none for a file, whose build is
    /// the one it holds.
    pub fn in_mode(&self, mode: Mode) -> Option<Image> {
        match self {
            Image::Pin(_) => Some(Image::Pin(mode)),
            Image::Release { tag, .. } => Some(Image::Release { tag: tag.clone(), mode }),
            Image::Local(_) => None,
        }
    }

    /// Whether the release behind it has `mode`'s build, as far as the
    /// player knows before asking: none when it cannot say yet.
    pub fn has(&self, mode: Mode) -> Option<bool> {
        match self {
            Image::Pin(_) => Some(mode == Mode::Qio || PINNED_DIO_SHA256.is_some()),
            Image::Release { tag, .. } => known_images(tag).map(|images| images.has(mode)),
            Image::Local(_) => None,
        }
    }
}

/// What every board is measured against: the pin's version, and whether a
/// board running it answers `@status`. Never a chosen image's: a choice
/// drives the next write, not the verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub version: String,
    pub answers_status: bool,
}

/// The pin as a target, or none in a build of the player with no pin.
pub(crate) fn pin_target() -> Option<Target> {
    PINNED_TAG.map(|tag| Target { version: tag.to_string(), answers_status: PINNED_ANSWERS_STATUS })
}

/// A piece of the image and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    pub offset: u32,
    pub data: Vec<u8>,
}

/// Where an image came from, as the one word the page's header puts
/// beside the version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// A release, downloaded now.
    Release,
    /// A release, from the copy kept earlier.
    Cached,
    /// A file, or a build directory.
    File,
}

/// What an image was checked against before it was offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// The checksum built into this player: the pin's two builds.
    Pinned,
    /// The release's own SHA256SUMS.
    Sums,
    /// Only its app description: a local build says it is ours, which is
    /// not the same as working.
    Description,
}

/// An image read and checked, without its bytes: what the page, the log
/// and the command line say about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageFacts {
    /// What was asked for.
    pub image: Image,
    /// Its app description's version: the truth, whatever the tag said.
    pub version: String,
    /// `release v0.8.0, downloaded earlier`, `file C:\…\firmware.factory.bin`.
    pub origin: String,
    pub kind: Origin,
    pub bytes: usize,
    pub layout: Layout,
    /// From its header; none for an app alone (which keeps the board's
    /// bootloader, and so its mode) or a clock the player does not know.
    pub mode: Option<Mode>,
    /// The four bytes of its first header: the bootloader's at 0x1000 in a
    /// merged image, the app's own at 0x0 in an app alone.
    pub header: Option<[u8; 4]>,
    /// The first eight hex digits of its ELF's SHA-256: what the board
    /// will say once it runs (About, `@status`, the boot line).
    pub elf: String,
    pub check: Check,
    /// The file it was read from: a local build's (a folder's merged
    /// image, named).
    pub file: Option<PathBuf>,
}

/// An image ready to write: its facts and its pieces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Firmware {
    pub facts: ImageFacts,
    pub segments: Vec<Segment>,
}

/// The log's line for an image's header: `0x1000: E9 03 02 40 — 40 MHz,
/// the DIO build`, read from the image itself, never from its name.
pub(crate) fn header_line(facts: &ImageFacts) -> Option<String> {
    let header = facts.header?;
    let at = match facts.layout {
        Layout::Merged => BOOTLOADER_OFFSET,
        Layout::App => 0,
    };
    let at = format!("{at:X}");
    let bytes = header.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
    Some(match (facts.mode, header_mhz(&header)) {
        (Some(mode), Some(mhz)) => {
            t!("dev.log_header", at = at, bytes = bytes, mhz = mhz, mode = mode.word()).to_string()
        }
        _ => t!("dev.log_header_unknown", at = at, bytes = bytes).to_string(),
    })
}

/// ESP-IDF's `esp_app_desc_t`: the fields the player reads. The firmware's
/// `tools/version.py` and `Version.cpp` fill `version` with the build's
/// git description (`v0.5.0`, `v0.5.0-3-gabc1234-dirty`) and
/// `project_name` with the firmware's name; an image of some other
/// firmware says something else, and a blank flash says nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppDesc {
    pub version: String,
    pub project: String,
    pub idf: String,
    /// The first eight hex digits of the ELF's SHA-256 — what About shows.
    pub elf8: String,
}

impl AppDesc {
    pub const MAGIC: u32 = 0xABCD_5432;
    /// Where the description sits in an app image: after the 24-byte image
    /// header and the first segment's 8-byte header.
    pub const OFFSET_IN_APP: usize = 0x20;
    pub const LEN: usize = 256;
    pub const OURS: &str = "mstream-mp3-player";

    /// The description in `bytes` (the 256 bytes at its offset), if the
    /// magic word is there.
    pub fn parse(bytes: &[u8]) -> Option<AppDesc> {
        if bytes.len() < Self::LEN {
            return None;
        }
        let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if magic != Self::MAGIC {
            return None;
        }
        let text = |from: usize, to: usize| -> String {
            let raw = &bytes[from..to];
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).trim().to_string()
        };
        Some(AppDesc {
            version: text(16, 48),
            project: text(48, 80),
            idf: text(112, 144),
            elf8: bytes[144..148].iter().map(|b| format!("{b:02x}")).collect(),
        })
    }

    /// The description in an app image (the bytes from its start).
    pub fn in_app(app: &[u8]) -> Option<AppDesc> {
        app.get(Self::OFFSET_IN_APP..Self::OFFSET_IN_APP + Self::LEN).and_then(Self::parse)
    }

    pub fn is_ours(&self) -> bool {
        self.project == Self::OURS
    }
}

/// What a file is, by its shape. A merged image has an image header at
/// 0x1000 (the bootloader) and one at 0x10000 (the app), and nothing but
/// erased bytes before the first; an app alone starts with its header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// The merged `firmware.factory.bin` / `*-full.bin`: written at 0x0.
    Merged,
    /// The app alone (`firmware.bin` / `*-app.bin`): written at ota_0.
    App,
}

/// The layout and the description of an image, or nothing for a file that
/// is neither (its description missing counts as neither: every image the
/// firmware builds carries one, and the page needs the version).
pub(crate) fn classify(bytes: &[u8]) -> Option<(Layout, AppDesc)> {
    let header_at = |at: usize| bytes.get(at).copied() == Some(IMAGE_MAGIC);
    if header_at(BOOTLOADER_OFFSET) && header_at(APP_OFFSET) && bytes[0] == 0xFF {
        let desc = AppDesc::in_app(&bytes[APP_OFFSET..])?;
        return Some((Layout::Merged, desc));
    }
    if header_at(0) {
        let desc = AppDesc::in_app(bytes)?;
        return Some((Layout::App, desc));
    }
    None
}

/// The MP3 Player tab takes a merged image only: an app alone keeps the
/// board's bootloader and so its flash mode, which the tab cannot show
/// honestly — it stays `device flash --firmware`'s.
pub(crate) fn merged_only(facts: &ImageFacts) -> Result<(), DeviceError> {
    match facts.layout {
        Layout::Merged => Ok(()),
        Layout::App => Err(DeviceError::Firmware(t!("dev.fw_app_alone").to_string())),
    }
}

/// `--flash-mode` beside `--firmware`: the file's own build decides, and a
/// flag that says otherwise is refused rather than ignored.
pub(crate) fn mode_conflict(facts: &ImageFacts, wanted: Mode) -> Option<DeviceError> {
    let path = facts.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default();
    let flag = wanted.word().to_ascii_lowercase();
    match (facts.layout, facts.mode) {
        (Layout::App, _) => Some(DeviceError::Firmware(t!("dev.fw_mode_app", path = path).to_string())),
        (Layout::Merged, Some(mode)) if mode != wanted => Some(DeviceError::Firmware(
            t!("dev.fw_mode_conflict", path = path, mode = mode.word(), flag = flag).to_string(),
        )),
        (Layout::Merged, None) => {
            Some(DeviceError::Firmware(t!("dev.fw_mode_unknown", path = path, flag = flag).to_string()))
        }
        _ => None,
    }
}

// ── Versions ────────────────────────────────────────────────────────────────

/// A firmware version, in the shapes the firmware's `tools/version.py`
/// writes into its app description: a release (`v0.8.0`, or a pre-release
/// `v0.9.0-rc.1`), a build past one (`git describe`'s `v0.8.0-5-g4e94418`,
/// `-dirty` when the tree had changes), or a build with no tag reachable
/// (`v0.9.0-dev+abc1234`: the NEXT release's dev build, so before it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    pub core: (u64, u64, u64),
    /// SemVer's pre-release identifiers (`rc.1`, `dev`); none for a final
    /// release.
    pub pre: Vec<String>,
    /// Commits past the tag (`git describe`'s count).
    pub ahead: u32,
    /// Not a release: past a tag, a `-dev+` build, or a dirty tree.
    pub dev: bool,
}

/// Where a board's version stands against the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// The very version: up to date.
    Same,
    /// Before it: an older release, or a dev build based below it.
    Older,
    /// It, or after it, and not the very version: a newer release, or a
    /// dev build on the target or later. Never offered as an update — the
    /// write would go back.
    Newer,
    /// A version the order cannot place (not one of the shapes above).
    Unknown,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let mut rest = text.trim().strip_prefix('v')?;
        let mut dev = false;
        if let Some(clean) = rest.strip_suffix("-dirty") {
            rest = clean;
            dev = true;
        }
        // Build metadata (`+abc1234`) says nothing about the order; its
        // presence says the build is not a release.
        if let Some((base, _build)) = rest.split_once('+') {
            rest = base;
            dev = true;
        }
        // `git describe`'s tail: `-<n>-g<hash>`.
        let mut ahead = 0;
        if let Some((base, tail)) = rest.rsplit_once("-g")
            && !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_hexdigit())
            && let Some((base, count)) = base.rsplit_once('-')
            && let Ok(n) = count.parse::<u32>()
        {
            rest = base;
            ahead = n;
            dev = true;
        }
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_string).collect::<Vec<_>>()),
            None => (rest, Vec::new()),
        };
        if pre.iter().any(String::is_empty) {
            return None;
        }
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Version { core: (major, minor, patch), pre, ahead, dev })
    }

    /// SemVer's order (a pre-release before its release, identifiers
    /// numeric before alphanumeric), then the commits past the tag.
    pub fn order(&self, other: &Version) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let pre = match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                let ident = |a: &String, b: &String| match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => a.cmp(b),
                };
                self.pre
                    .iter()
                    .zip(&other.pre)
                    .map(|(a, b)| ident(a, b))
                    .find(|o| o.is_ne())
                    .unwrap_or_else(|| self.pre.len().cmp(&other.pre.len()))
            }
        };
        self.core.cmp(&other.core).then(pre).then(self.ahead.cmp(&other.ahead))
    }
}

/// Where `version` (a board's) stands against `target`. Up to date is the
/// same text and nothing else: a dirty build of the tag, or one a commit
/// past it, is the board running something the player does not carry.
pub(crate) fn place(version: &str, target: &str) -> Place {
    if version.trim() == target.trim() {
        return Place::Same;
    }
    match (Version::parse(version), Version::parse(target)) {
        (Some(board), Some(target)) => {
            if board.order(&target).is_lt() {
                Place::Older
            } else {
                Place::Newer
            }
        }
        _ => Place::Unknown,
    }
}

// ── The boards' modes ───────────────────────────────────────────────────────

/// One release's builds by their ELF ids, as its notes name them: what
/// tells a board's flash mode from the ELF it reports (About, `@status`'s
/// `elf=`, `L`, the boot line, its bootloader's read). Graft 3 of the
/// Advanced options set (mStream docs/designs/mp3-tab, card 08): every
/// release up to the pin, bumped with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Builds {
    pub tag: &'static str,
    /// None for a release before QIO: its one image is DIO.
    pub qio: Option<&'static str>,
    pub dio: &'static str,
}

/// Checked on 2026-10-10 against each release's notes ("ELF … for this
/// build; ELF … for the `-dio-full.bin`") and against the images
/// themselves: v0.8.0's two, v0.7.0's and v0.5.0's `-full.bin` carry these
/// ids in their app descriptions.
pub(crate) const BUILDS: &[Builds] = &[
    Builds { tag: "v0.8.0", qio: Some("e127a6bf"), dio: "3523b80e" },
    Builds { tag: "v0.7.0", qio: Some("63ee7a2b"), dio: "aa45f60e" },
    Builds { tag: "v0.6.0", qio: Some("cd2ab3b1"), dio: "925d9680" },
    Builds { tag: "v0.5.0", qio: None, dio: "17352e55" },
    Builds { tag: "v0.5.0-beta.1", qio: None, dio: "6a58c1f8" },
];

/// What a board's ELF says about its flash mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ElfMode {
    pub mode: Mode,
    /// The release (or the build's version) the ELF is.
    pub version: String,
    /// The mode was a choice: the release had both builds. A board on
    /// v0.5.0 runs DIO because nothing else existed, and updates to QIO
    /// like any other; a board on v0.7.0's DIO build was put there, and its
    /// updates keep DIO.
    pub choice: bool,
}

/// The ELF ids of images the player has had in hand since it started —
/// a release newer than the table, a local build — so a board written
/// with one says its mode too. Never stored: a restart forgets them, and
/// the table covers every release up to the pin.
static LEARNED: Mutex<Vec<(String, ElfMode)>> = Mutex::new(Vec::new());
const LEARNED_MAX: usize = 64;

/// A board's flash mode from its ELF id (eight hex digits or more), or
/// none for an ELF the player has never seen.
pub(crate) fn elf_mode(elf: &str) -> Option<ElfMode> {
    let elf = elf.trim().to_ascii_lowercase();
    let elf = elf.get(..8)?;
    for builds in BUILDS {
        if builds.qio == Some(elf) {
            return Some(ElfMode { mode: Mode::Qio, version: builds.tag.to_string(), choice: true });
        }
        if builds.dio == elf {
            let choice = builds.qio.is_some();
            return Some(ElfMode { mode: Mode::Dio, version: builds.tag.to_string(), choice });
        }
    }
    let learned = LEARNED.lock().unwrap_or_else(|e| e.into_inner());
    learned.iter().find(|(id, _)| id == elf).map(|(_, mode)| mode.clone())
}

/// Remember an image's ELF and mode, read from the image: a merged image
/// with a known mode only.
fn learn(facts: &ImageFacts) {
    let (Layout::Merged, Some(mode)) = (facts.layout, facts.mode) else { return };
    let Some(elf) = facts.elf.get(..8).map(str::to_ascii_lowercase) else { return };
    if BUILDS.iter().any(|b| b.qio == Some(elf.as_str()) || b.dio == elf) {
        return;
    }
    // Before v0.6.0 there was no QIO build: DIO was no choice.
    let choice = Version::parse(&facts.version).is_none_or(|v| v.core >= (0, 6, 0));
    let mut learned = LEARNED.lock().unwrap_or_else(|e| e.into_inner());
    learned.retain(|(id, _)| *id != elf);
    learned.push((elf, ElfMode { mode, version: facts.version.clone(), choice }));
    if learned.len() > LEARNED_MAX {
        learned.remove(0);
    }
}

// ── The release list ────────────────────────────────────────────────────────

/// Which builds a release has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Images {
    /// QIO and DIO: every release since v0.6.0.
    Both,
    /// One image, and it is DIO: v0.5.0 and before.
    DioOnly,
    /// One image, and it is QIO: no release so far.
    QioOnly,
}

impl Images {
    pub fn has(self, mode: Mode) -> bool {
        match self {
            Images::Both => true,
            Images::DioOnly => mode == Mode::Dio,
            Images::QioOnly => mode == Mode::Qio,
        }
    }

    /// From the files a release lists: a `-dio-full.bin` means both. With
    /// none, its one image is DIO when it is from before QIO (the table,
    /// or a version before v0.6.0), else QIO.
    fn of(tag: &str, has_dio: bool) -> Images {
        if has_dio {
            return Images::Both;
        }
        match known_images(tag) {
            Some(Images::Both) | None => Images::QioOnly,
            Some(images) => images,
        }
    }
}

/// What the player knows of a release's builds without asking: the table,
/// or for a release before v0.6.0, DIO only.
fn known_images(tag: &str) -> Option<Images> {
    if let Some(builds) = BUILDS.iter().find(|b| b.tag == tag) {
        return Some(if builds.qio.is_some() { Images::Both } else { Images::DioOnly });
    }
    Version::parse(tag).filter(|v| v.core < (0, 6, 0)).map(|_| Images::DioOnly)
}

/// One release as GitHub lists it: only the ones with a merged image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Release {
    pub tag: String,
    /// Its title on GitHub (`mStream Player v0.7.0 (beta)`).
    pub title: String,
    /// GitHub's publishing date, `2026-10-07` (UTC).
    pub date: String,
    pub pre: bool,
    pub images: Images,
}

impl Release {
    pub fn is_pin(&self) -> bool {
        PINNED_TAG == Some(self.tag.as_str())
    }

    /// Newer than this player's release: offered as an update this player
    /// has not been tried with.
    pub fn newer_than_pin(&self) -> bool {
        PINNED_TAG.is_some_and(|pin| place(pin, &self.tag) == Place::Older)
    }
}

/// Why the list did not come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ListWhy {
    /// No answer: offline, or GitHub out of reach.
    Offline(String),
    /// GitHub's limit for lists asked without an account: 60 an hour from
    /// one address. When it resets, in Unix seconds, if it said.
    Limited { reset: Option<u64> },
    /// Another answer.
    Status(u16),
    /// An answer that was not the list.
    NotJson(String),
}

impl ListWhy {
    /// One line, in the page's language.
    pub fn text(&self) -> String {
        match self {
            ListWhy::Offline(_) => t!("dev.list_offline").to_string(),
            ListWhy::Limited { reset: Some(at) } => t!("dev.list_limited", time = clock_at(*at)).to_string(),
            ListWhy::Limited { reset: None } => t!("dev.list_limited_later").to_string(),
            ListWhy::Status(code) => t!("dev.list_status", code = code).to_string(),
            ListWhy::NotJson(_) => t!("dev.list_not_json").to_string(),
        }
    }

    /// What the log adds: the network's own words.
    pub fn detail(&self) -> Option<&str> {
        match self {
            ListWhy::Offline(detail) | ListWhy::NotJson(detail) => Some(detail),
            _ => None,
        }
    }
}

/// `09:12` on this computer's clock (UTC on a machine without a zone).
pub(crate) fn clock_at(epoch: u64) -> String {
    let zone = crate::admin::tz::local();
    let t = epoch as i64;
    let local = t + zone.as_ref().map_or(0, |z| z.offset_at(t) as i64);
    let s = local.rem_euclid(86_400);
    format!("{:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// A release already on this computer that can be written with no
/// network: an image beside the SHA256SUMS it was checked against (or the
/// pin's, by its checksums).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cached {
    pub tag: String,
    pub modes: Vec<Mode>,
    /// When the first of them was downloaded.
    pub since: Option<SystemTime>,
}

/// The list as GitHub's releases API answers it: drafts and releases
/// without a `-full.bin` left out, newest first by the version order (a
/// tag the order cannot place after the rest, newest date first).
pub(crate) fn parse_releases(text: &str) -> Result<Vec<Release>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = value.as_array().ok_or_else(|| "not a list".to_string())?;
    let mut releases: Vec<Release> = list
        .iter()
        .filter(|entry| entry["draft"].as_bool() != Some(true))
        .filter_map(|entry| {
            let tag = entry["tag_name"].as_str()?.trim().to_string();
            let names: Vec<&str> = entry["assets"]
                .as_array()
                .map(|assets| assets.iter().filter_map(|a| a["name"].as_str()).collect())
                .unwrap_or_default();
            if !names.contains(&full_asset_name(&tag).as_str()) {
                return None;
            }
            let has_dio = names.contains(&dio_asset_name(&tag).as_str());
            Some(Release {
                title: entry["name"].as_str().unwrap_or(&tag).to_string(),
                date: entry["published_at"].as_str().map(|d| d.chars().take(10).collect()).unwrap_or_default(),
                pre: entry["prerelease"].as_bool() == Some(true),
                images: Images::of(&tag, has_dio),
                tag,
            })
        })
        .collect();
    releases.sort_by(|a, b| match (Version::parse(&a.tag), Version::parse(&b.tag)) {
        (Some(x), Some(y)) => y.order(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.date.cmp(&a.date),
    });
    Ok(releases)
}

// ── Where the images come from ──────────────────────────────────────────────

/// What the worker asks for images and the release list: [`Net`] for
/// real, a shelf of the test's own under test.
pub(crate) trait Supply: Send + Sync {
    /// The image, read or downloaded and checked. `progress` hears a
    /// download's bytes so far and, when the server said, its total.
    fn resolve(&self, image: &Image, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Firmware, DeviceError>;
    /// Every release with a merged image, newest first: one request.
    fn releases(&self) -> Result<Vec<Release>, ListWhy>;
    /// The releases this computer can write offline.
    fn cached(&self) -> Vec<Cached>;
}

/// GitHub and the cache directory: the releases' files under
/// `<cache>/firmware/<tag>/`, each beside its release's `SHA256SUMS`.
/// `MSTREAM_FIRMWARE_BASE` points the downloads at a mirror (a test's local
/// server): the files right under it by their release names, `{tag}` in it
/// replaced by the tag. `MSTREAM_FIRMWARE_API` does the same for the
/// releases API (a smoke's fake GitHub).
pub(crate) struct Net {
    dir: Option<PathBuf>,
    assets: Option<String>,
    api: String,
}

/// The real supply, as the worker shares it.
pub(crate) fn net() -> Arc<dyn Supply> {
    Arc::new(Net::from_env())
}

impl Net {
    pub fn from_env() -> Net {
        let var = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Net {
            dir: crate::config::firmware_dir(),
            assets: var("MSTREAM_FIRMWARE_BASE"),
            api: var("MSTREAM_FIRMWARE_API").unwrap_or_else(|| GITHUB_API.to_string()),
        }
    }

    /// Where a release's file is.
    fn asset_url(&self, tag: &str, name: &str) -> String {
        match &self.assets {
            Some(base) => format!("{}/{name}", base.replace("{tag}", tag).trim_end_matches('/')),
            None => format!("https://github.com/{REPO}/releases/download/{tag}/{name}"),
        }
    }

    fn releases_url(&self) -> String {
        format!("{}/repos/{REPO}/releases?per_page={LIST_PAGE}", self.api.trim_end_matches('/'))
    }

    fn kept(&self, tag: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|dir| dir.join(tag))
    }

    /// The pin in `mode`: its checksum is built in.
    fn pin(&self, mode: Mode, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Firmware, DeviceError> {
        let Some(tag) = PINNED_TAG else {
            return Err(DeviceError::Firmware(t!("dev.fw_no_pin").to_string()));
        };
        let (name, sha) = match mode {
            Mode::Qio => (full_asset_name(tag), PINNED_FULL_SHA256),
            Mode::Dio => (dio_asset_name(tag), PINNED_DIO_SHA256),
        };
        let Some(sha) = sha else {
            let key = if mode == Mode::Dio { "dev.fw_no_dio" } else { "dev.fw_no_pin" };
            return Err(DeviceError::Firmware(t!(key, tag = tag).to_string()));
        };
        let image = Image::Pin(mode);
        self.checked(&image, tag, &name, sha, Check::Pinned, None, progress)
    }

    /// Another release in `mode`, checked by its own SHA256SUMS: the copy
    /// kept earlier beside the sums it passed, with no network at all, or
    /// the sums fetched now and the image after them.
    fn release(
        &self,
        tag: &str,
        mode: Mode,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Firmware, DeviceError> {
        if let Some(images) = known_images(tag)
            && !images.has(mode)
        {
            return Err(no_build(tag, mode));
        }
        let image = Image::Release { tag: tag.to_string(), mode };
        if let Some(dir) = self.kept(tag)
            && let Ok(text) = std::fs::read(dir.join("SHA256SUMS"))
        {
            let sums = parse_sums(&String::from_utf8_lossy(&text));
            if let Ok(name) = asset_for(tag, mode, &sums)
                && let Some((sha, _)) = sums.iter().find(|(_, file)| *file == name)
                && let Ok(bytes) = std::fs::read(dir.join(&name))
                && sha256_hex(&bytes) == *sha
            {
                let origin = t!("dev.origin_cached", tag = tag).to_string();
                let firmware = build(image, bytes, &name, origin, Origin::Cached, Check::Sums, None)?;
                return in_mode(firmware, mode, &name);
            }
        }
        let text = fetch(&self.asset_url(tag, "SHA256SUMS"), MAX_SUMS, &mut |_, _| {}, "SHA256SUMS")?;
        let sums = parse_sums(&String::from_utf8_lossy(&text));
        let name = asset_for(tag, mode, &sums)?;
        let sha = sums
            .iter()
            .find(|(_, file)| *file == name)
            .map(|(sha, _)| sha.clone())
            .ok_or_else(|| DeviceError::Firmware(t!("dev.fw_sums_missing", name = name).to_string()))?;
        self.checked(&image, tag, &name, &sha, Check::Sums, Some(&text), progress)
    }

    /// A release's file against `sha`: the copy kept earlier when it still
    /// matches, else a download, checked, then kept — with the sums it
    /// passed, when it has any, so the next time needs no network.
    #[allow(clippy::too_many_arguments)]
    fn checked(
        &self,
        image: &Image,
        tag: &str,
        name: &str,
        sha: &str,
        check: Check,
        sums: Option<&[u8]>,
        progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<Firmware, DeviceError> {
        let sha = sha.to_ascii_lowercase();
        let mode = image.mode().unwrap_or(Mode::Qio);
        let dir = self.kept(tag);
        if let Some(bytes) = dir.as_ref().and_then(|d| std::fs::read(d.join(name)).ok())
            && sha256_hex(&bytes) == sha
        {
            let origin = t!("dev.origin_cached", tag = tag).to_string();
            let firmware = build(image.clone(), bytes, name, origin, Origin::Cached, check, None)?;
            if let (Some(dir), Some(sums)) = (&dir, sums) {
                keep(dir, "SHA256SUMS", sums);
            }
            return in_mode(firmware, mode, name);
        }
        let bytes = fetch(&self.asset_url(tag, name), MAX_IMAGE, progress, name)?;
        if sha256_hex(&bytes) != sha {
            return Err(DeviceError::Firmware(t!("dev.fw_checksum", name = name).to_string()));
        }
        let origin = t!("dev.origin_release", tag = tag).to_string();
        let firmware = in_mode(build(image.clone(), bytes, name, origin, Origin::Release, check, None)?, mode, name)?;
        if let Some(dir) = &dir {
            // Kept for next time; a cache that cannot be written only costs
            // the next download, so nothing here fails on it.
            let data: Vec<u8> = firmware.segments.iter().flat_map(|s| s.data.iter().copied()).collect();
            keep(dir, name, &data);
            if let Some(sums) = sums {
                keep(dir, "SHA256SUMS", sums);
            }
        }
        Ok(firmware)
    }
}

impl Supply for Net {
    fn resolve(&self, image: &Image, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Firmware, DeviceError> {
        match image {
            Image::Local(path) => read_local(path),
            Image::Pin(mode) => self.pin(*mode, progress),
            Image::Release { tag, mode } => self.release(tag, *mode, progress),
        }
    }

    fn releases(&self) -> Result<Vec<Release>, ListWhy> {
        let answer = get_api(&self.releases_url()).map_err(ListWhy::Offline)?;
        match answer.status {
            200 => parse_releases(&String::from_utf8_lossy(&answer.body)).map_err(ListWhy::NotJson),
            403 | 429 if answer.remaining == Some(0) || answer.status == 429 => {
                Err(ListWhy::Limited { reset: answer.reset })
            }
            code => Err(ListWhy::Status(code)),
        }
    }

    fn cached(&self) -> Vec<Cached> {
        let Some(root) = &self.dir else { return Vec::new() };
        let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
        let mut found: Vec<Cached> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|entry| {
                let tag = entry.file_name().to_string_lossy().to_string();
                let dir = entry.path();
                let sums = std::fs::read(dir.join("SHA256SUMS"))
                    .map(|text| parse_sums(&String::from_utf8_lossy(&text)))
                    .unwrap_or_default();
                let mut modes = Vec::new();
                let mut since: Option<SystemTime> = None;
                for mode in [Mode::Qio, Mode::Dio] {
                    let wanted = if PINNED_TAG == Some(tag.as_str()) {
                        let sha = if mode == Mode::Qio { PINNED_FULL_SHA256 } else { PINNED_DIO_SHA256 };
                        let name = if mode == Mode::Qio { full_asset_name(&tag) } else { dio_asset_name(&tag) };
                        sha.map(|sha| (name, sha.to_string()))
                    } else {
                        asset_for(&tag, mode, &sums).ok().and_then(|name| {
                            sums.iter().find(|(_, file)| *file == name).map(|(sha, _)| (name, sha.clone()))
                        })
                    };
                    let Some((name, sha)) = wanted else { continue };
                    let path = dir.join(&name);
                    let Ok(bytes) = std::fs::read(&path) else { continue };
                    if sha256_hex(&bytes) != sha {
                        continue;
                    }
                    modes.push(mode);
                    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                    since = match (since, modified) {
                        (Some(a), Some(b)) => Some(a.min(b)),
                        (a, b) => a.or(b),
                    };
                }
                (!modes.is_empty()).then_some(Cached { tag, modes, since })
            })
            .collect();
        found.sort_by(|a, b| match (Version::parse(&a.tag), Version::parse(&b.tag)) {
            (Some(x), Some(y)) => y.order(&x),
            _ => b.tag.cmp(&a.tag),
        });
        found
    }
}

/// A release asked for a build it does not have.
fn no_build(tag: &str, mode: Mode) -> DeviceError {
    let key = match mode {
        Mode::Qio => "dev.fw_no_qio",
        Mode::Dio => "dev.fw_no_dio",
    };
    DeviceError::Firmware(t!(key, tag = tag).to_string())
}

/// The release's file for `mode`, by what its SHA256SUMS lists: the
/// `-dio-full.bin` for DIO where it has one; the `-full.bin` otherwise —
/// QIO since v0.6.0, and before that its one image, which is DIO.
fn asset_for(tag: &str, mode: Mode, sums: &[(String, String)]) -> Result<String, DeviceError> {
    let dio = dio_asset_name(tag);
    let images = Images::of(tag, sums.iter().any(|(_, file)| *file == dio));
    match (images, mode) {
        (Images::Both, Mode::Dio) => Ok(dio),
        (images, mode) if images.has(mode) => Ok(full_asset_name(tag)),
        _ => Err(no_build(tag, mode)),
    }
}

/// The image's header agrees with the build asked for, or the image is
/// refused: a release file that says the other mode is not what was
/// chosen, whatever its name.
fn in_mode(firmware: Firmware, mode: Mode, name: &str) -> Result<Firmware, DeviceError> {
    match firmware.facts.mode {
        Some(found) if found != mode => Err(DeviceError::Firmware(
            t!("dev.fw_wrong_build", name = name, found = found.word(), wanted = mode.word()).to_string(),
        )),
        _ => Ok(firmware),
    }
}

/// The file a local path means: itself, or a build folder's merged image —
/// `firmware.factory.bin` in it, or in the one build environment under it
/// (a PlatformIO `.pio/build/` holds a folder per environment).
pub(crate) fn local_file(path: &Path) -> Result<PathBuf, DeviceError> {
    if !path.is_dir() {
        return Ok(path.to_path_buf());
    }
    const MERGED: &str = "firmware.factory.bin";
    if path.join(MERGED).is_file() {
        return Ok(path.join(MERGED));
    }
    let shown = path.display().to_string();
    let mut builds: Vec<PathBuf> = std::fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| e.path().join(MERGED)).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    builds.sort();
    match builds.len() {
        1 => Ok(builds.remove(0)),
        0 => Err(DeviceError::Firmware(t!("dev.fw_folder_empty", path = shown).to_string())),
        _ => {
            let names: Vec<String> = builds
                .iter()
                .filter_map(|p| p.parent()?.file_name().map(|n| n.to_string_lossy().to_string()))
                .collect();
            let list = names.join(", ");
            Err(DeviceError::Firmware(t!("dev.fw_folder_several", path = shown, list = list).to_string()))
        }
    }
}

/// A local file, or a build folder's merged image, read and vetted.
pub(crate) fn read_local(path: &Path) -> Result<Firmware, DeviceError> {
    let file = local_file(path)?;
    let shown = file.display().to_string();
    let read_failed =
        |e: std::io::Error| DeviceError::Firmware(t!("dev.fw_read", path = shown, err = e).to_string());
    let size = std::fs::metadata(&file).map_err(read_failed)?.len();
    if size > MAX_IMAGE as u64 {
        return Err(DeviceError::Firmware(t!("dev.fw_too_big", what = shown).to_string()));
    }
    let bytes = std::fs::read(&file).map_err(read_failed)?;
    let origin = t!("dev.origin_file", path = shown).to_string();
    build(Image::Local(path.to_path_buf()), bytes, &shown, origin, Origin::File, Check::Description, Some(file))
}

/// `bytes` as one of our images, its facts read from it, or the reason it
/// is not one.
fn build(
    image: Image,
    bytes: Vec<u8>,
    what: &str,
    origin: String,
    kind: Origin,
    check: Check,
    file: Option<PathBuf>,
) -> Result<Firmware, DeviceError> {
    let (layout, desc) = vetted(&bytes, what)?;
    let at = match layout {
        Layout::Merged => BOOTLOADER_OFFSET,
        Layout::App => 0,
    };
    let header = bytes.get(at..at + 4).map(|h| [h[0], h[1], h[2], h[3]]);
    let mode = match layout {
        Layout::Merged => header.as_ref().and_then(|h| Mode::of_header(h)),
        Layout::App => None,
    };
    let facts = ImageFacts {
        image,
        version: desc.version,
        origin,
        kind,
        bytes: bytes.len(),
        layout,
        mode,
        header,
        elf: desc.elf8,
        check,
        file,
    };
    learn(&facts);
    Ok(Firmware { facts, segments: vec![segment(layout, bytes)] })
}

/// The image's one segment: the merged image at 0x0, an app at ota_0.
fn segment(layout: Layout, data: Vec<u8>) -> Segment {
    let offset = match layout {
        Layout::Merged => 0,
        Layout::App => APP_OFFSET as u32,
    };
    Segment { offset, data }
}

/// `bytes` as one of our images, or the reason it is not.
fn vetted(bytes: &[u8], what: &str) -> Result<(Layout, AppDesc), DeviceError> {
    let Some((layout, desc)) = classify(bytes) else {
        return Err(DeviceError::Firmware(t!("dev.fw_not_image", path = what).to_string()));
    };
    if !desc.is_ours() {
        return Err(DeviceError::Firmware(
            t!("dev.fw_not_ours", what = what, name = desc.project).to_string(),
        ));
    }
    Ok((layout, desc))
}

/// A file kept beside the others of its release, whole or not at all.
fn keep(dir: &Path, name: &str, bytes: &[u8]) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(name);
    let part = dir.join(format!("{name}.part"));
    if std::fs::write(&part, bytes).is_ok() {
        let _ = std::fs::rename(&part, &path);
    }
}

// ── Pieces ──────────────────────────────────────────────────────────────────

/// `mstream-player-core2-v0.7.0-full.bin`: the release's merged image. The
/// firmware's packager replaces what a file name cannot carry (a `+` in a
/// dev build's version) with `_`; a release tag has none, but the rule is
/// the same here so a `--release` of such a build still resolves.
pub(crate) fn full_asset_name(tag: &str) -> String {
    format!("{ASSET_STEM}-{}-full.bin", file_version(tag))
}

/// `mstream-player-core2-v0.8.0-dio-full.bin`: the same firmware in DIO,
/// from v0.6.0 on.
pub(crate) fn dio_asset_name(tag: &str) -> String {
    format!("{ASSET_STEM}-{}-dio-full.bin", file_version(tag))
}

fn file_version(tag: &str) -> String {
    tag.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect()
}

/// `(sha256, file name)` per line of a `sha256sum` listing — two spaces,
/// or a space and an asterisk for a binary-mode entry.
pub(crate) fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (sha, rest) = line.split_once(' ')?;
            let name = rest.trim_start().trim_start_matches('*').trim();
            (sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (sha.to_ascii_lowercase(), name.to_string()))
        })
        .collect()
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// One GET, the body capped at `cap` bytes, `progress` told as it lands.
/// The shared runtime drives reqwest; this is called from the worker's
/// threads, never from inside the runtime.
fn fetch(
    url: &str,
    cap: usize,
    progress: &mut dyn FnMut(u64, Option<u64>),
    what: &str,
) -> Result<Vec<u8>, DeviceError> {
    fetch_within(url, cap, progress, what, READ_STALL)
}

/// [`fetch`], giving up once `stall` passes with no byte.
fn fetch_within(
    url: &str,
    cap: usize,
    progress: &mut dyn FnMut(u64, Option<u64>),
    what: &str,
    stall: Duration,
) -> Result<Vec<u8>, DeviceError> {
    let failed = |err: String| DeviceError::Firmware(t!("dev.fw_download", what = what, err = err).to_string());
    let result = crate::runtime::block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(stall)
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let total = response.content_length();
        let mut bytes: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > cap {
                return Err(t!("dev.fw_too_big", what = what).to_string());
            }
            progress(bytes.len() as u64, total);
        }
        Ok::<Vec<u8>, String>(bytes)
    });
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(err)) | Err(err) => Err(failed(err)),
    }
}

/// The releases API's answer: its status, the rate limit's two headers,
/// the body.
struct Answer {
    status: u16,
    remaining: Option<u64>,
    reset: Option<u64>,
    body: Vec<u8>,
}

/// One GET of GitHub's API, as GitHub asks to be asked: a User-Agent, its
/// JSON media type and version. Any status is an answer; only no answer
/// at all is an error.
fn get_api(url: &str) -> Result<Answer, String> {
    let result = crate::runtime::block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_STALL)
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(url)
            .header("User-Agent", concat!("mstream-player/", env!("CARGO_PKG_VERSION")))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let number = |name: &str| -> Option<u64> {
            response.headers().get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok())
        };
        let (remaining, reset) = (number("x-ratelimit-remaining"), number("x-ratelimit-reset"));
        let status = response.status().as_u16();
        let mut body: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_LIST {
                return Err("the answer is too long to be the release list".to_string());
            }
        }
        Ok::<Answer, String>(Answer { status, remaining, reset, body })
    });
    match result {
        Ok(answer) => answer,
        Err(err) => Err(err),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The first 0x120 bytes of a real core2 `firmware.bin` (the image
    /// header, the first segment's header, the app description) — a build
    /// of 2026-09-30, version `v0.5.0-dev+ac93229-dirty`.
    pub(crate) const APP_HEAD: &[u8] = include_bytes!("../../test/golden/core2/app-head.bin");

    /// A description as the firmware writes one, for `version` and
    /// `project` — the rest as a real build fills it.
    pub(crate) fn desc_bytes(version: &str, project: &str) -> Vec<u8> {
        desc_with_elf(version, project, &[0xA1; 4])
    }

    /// …with the ELF id the board will report once it runs it.
    pub(crate) fn desc_with_elf(version: &str, project: &str, elf: &[u8; 4]) -> Vec<u8> {
        let mut bytes = vec![0u8; AppDesc::LEN];
        bytes[..4].copy_from_slice(&AppDesc::MAGIC.to_le_bytes());
        bytes[16..16 + version.len()].copy_from_slice(version.as_bytes());
        bytes[48..48 + project.len()].copy_from_slice(project.as_bytes());
        bytes[112..118].copy_from_slice(b"v5.5.5");
        bytes[144..176].copy_from_slice(&[0xA1; 32]);
        bytes[144..148].copy_from_slice(elf);
        bytes
    }

    /// `e127a6bf` → its four bytes.
    pub(crate) fn elf_bytes(hex: &str) -> [u8; 4] {
        let mut out = [0u8; 4];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
        }
        out
    }

    /// A merged image as pioarduino makes one: erased bytes, a header at
    /// 0x1000, a header at 0x10000 followed by `desc`. Its clock byte is
    /// erased (`FF`): the QIO build's nibble.
    pub(crate) fn merged_bytes(desc: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xFF; APP_OFFSET + AppDesc::OFFSET_IN_APP + AppDesc::LEN + 64];
        bytes[BOOTLOADER_OFFSET] = IMAGE_MAGIC;
        bytes[APP_OFFSET] = IMAGE_MAGIC;
        let at = APP_OFFSET + AppDesc::OFFSET_IN_APP;
        bytes[at..at + desc.len()].copy_from_slice(desc);
        bytes
    }

    /// …with the headers a real build of `mode` writes.
    pub(crate) fn merged_in(desc: &[u8], mode: Mode) -> Vec<u8> {
        let mut bytes = merged_bytes(desc);
        let clock = match mode {
            Mode::Qio => 0x4F,
            Mode::Dio => 0x40,
        };
        bytes[BOOTLOADER_OFFSET..BOOTLOADER_OFFSET + 4].copy_from_slice(&[0xE9, 0x03, 0x02, clock]);
        bytes[APP_OFFSET..APP_OFFSET + 4].copy_from_slice(&[0xE9, 0x06, 0x02, clock]);
        bytes
    }

    /// An image in hand, as the worker holds one, from its bytes.
    pub(crate) fn in_hand(bytes: Vec<u8>) -> Firmware {
        let image = Image::Local(PathBuf::from("test.bin"));
        let origin = "a test".to_string();
        build(image, bytes, "test.bin", origin, Origin::File, Check::Description, None).expect("an image of ours")
    }

    /// The pin's two builds in miniature, with their real ELF ids.
    pub(crate) fn pin_image(version: &str, mode: Mode) -> Vec<u8> {
        let elf = match mode {
            Mode::Qio => "e127a6bf",
            Mode::Dio => "3523b80e",
        };
        merged_in(&desc_with_elf(version, AppDesc::OURS, &elf_bytes(elf)), mode)
    }

    /// The facts of the pin's build in miniature, as a write's Done carries
    /// them.
    pub(crate) fn pin_facts(version: &str, mode: Mode) -> ImageFacts {
        let origin = format!("release {version}, downloaded earlier");
        let bytes = pin_image(version, mode);
        build(Image::Pin(mode), bytes, version, origin, Origin::Cached, Check::Pinned, None).expect("ours").facts
    }

    /// A shelf of images and a release list, standing in for GitHub: what
    /// the worker's tests hand it. Local paths are read for real (the
    /// tests' temp files).
    pub(crate) struct Shelf {
        pub images: Mutex<HashMap<Image, Vec<u8>>>,
        pub list: Mutex<Result<Vec<Release>, ListWhy>>,
        pub cached: Vec<Cached>,
        /// How many times the list was asked for.
        pub asked: AtomicUsize,
        /// How long each image takes to come, a little at a time.
        pub pace: Duration,
    }

    impl Shelf {
        /// The pin's two builds of `version`, and the three releases before
        /// it.
        pub(crate) fn new(version: &str) -> Shelf {
            let mut images = HashMap::new();
            images.insert(Image::Pin(Mode::Qio), pin_image(version, Mode::Qio));
            images.insert(Image::Pin(Mode::Dio), pin_image(version, Mode::Dio));
            for (tag, qio, dio) in [("v0.7.0", "63ee7a2b", "aa45f60e"), ("v0.6.0", "cd2ab3b1", "925d9680")] {
                let image = |elf: &str, mode| merged_in(&desc_with_elf(tag, AppDesc::OURS, &elf_bytes(elf)), mode);
                images.insert(Image::release(tag, Mode::Qio), image(qio, Mode::Qio));
                images.insert(Image::release(tag, Mode::Dio), image(dio, Mode::Dio));
            }
            let v050 = merged_in(&desc_with_elf("v0.5.0", AppDesc::OURS, &elf_bytes("17352e55")), Mode::Dio);
            images.insert(Image::release("v0.5.0", Mode::Dio), v050);
            Shelf {
                images: Mutex::new(images),
                list: Mutex::new(Ok(listed())),
                cached: Vec::new(),
                asked: AtomicUsize::new(0),
                pace: Duration::ZERO,
            }
        }
    }

    /// GitHub's list as of 2026-10-10, parsed.
    pub(crate) fn listed() -> Vec<Release> {
        parse_releases(GITHUB_LIST).expect("the sample parses")
    }

    impl Supply for Shelf {
        fn resolve(&self, image: &Image, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Firmware, DeviceError> {
            if let Image::Local(path) = image {
                return read_local(path);
            }
            let bytes = self.images.lock().unwrap().get(image).cloned();
            let Some(bytes) = bytes else {
                let what = format!("{image:?}");
                return Err(DeviceError::Firmware(format!("could not download {what}: not on the test's shelf")));
            };
            // A paced shelf downloads, a quarter at a time; an unpaced one
            // is the copy kept earlier, which says nothing as it is read.
            let total = bytes.len() as u64;
            for step in (1..=4u64).filter(|_| !self.pace.is_zero()) {
                std::thread::sleep(self.pace / 4);
                progress(total * step / 4, Some(total));
            }
            let tag = image.tag().unwrap_or_default().to_string();
            let check = if image.is_pin() { Check::Pinned } else { Check::Sums };
            let origin = format!("release {tag}, from the test's shelf");
            let mode = image.mode().unwrap_or(Mode::Qio);
            in_mode(build(image.clone(), bytes, &tag, origin, Origin::Cached, check, None)?, mode, &tag)
        }

        fn releases(&self) -> Result<Vec<Release>, ListWhy> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(self.pace);
            self.list.lock().unwrap().clone()
        }

        fn cached(&self) -> Vec<Cached> {
            self.cached.clone()
        }
    }

    /// GitHub's releases API on 2026-10-10, cut to the fields the player
    /// reads, plus a draft and a release with no merged image.
    pub(crate) const GITHUB_LIST: &str = r#"[
      {"tag_name":"v0.9.0","name":"draft","draft":true,"prerelease":false,"published_at":null,
       "assets":[{"name":"mstream-player-core2-v0.9.0-full.bin"}]},
      {"tag_name":"v0.8.0","name":"mStream Player v0.8.0 (beta)","draft":false,"prerelease":false,
       "published_at":"2026-10-10T01:41:08Z",
       "assets":[{"name":"mstream-player-core2-v0.8.0-full.bin"},{"name":"mstream-player-core2-v0.8.0-dio-full.bin"},
                 {"name":"SHA256SUMS"}]},
      {"tag_name":"v0.5.0-beta.1","name":"mStream Player v0.5.0-beta.1 (pre-release)","draft":false,
       "prerelease":true,"published_at":"2026-10-01T13:31:32Z",
       "assets":[{"name":"mstream-player-core2-v0.5.0-beta.1-full.bin"}]},
      {"tag_name":"v0.6.0","name":"mStream Player v0.6.0 (beta)","draft":false,"prerelease":false,
       "published_at":"2026-10-02T20:53:19Z",
       "assets":[{"name":"mstream-player-core2-v0.6.0-full.bin"},{"name":"mstream-player-core2-v0.6.0-dio-full.bin"}]},
      {"tag_name":"v0.7.0","name":"mStream Player v0.7.0 (beta)","draft":false,"prerelease":false,
       "published_at":"2026-10-07T17:26:39Z",
       "assets":[{"name":"mstream-player-core2-v0.7.0-full.bin"},{"name":"mstream-player-core2-v0.7.0-dio-full.bin"}]},
      {"tag_name":"v0.5.0","name":"mStream Player v0.5.0 (beta)","draft":false,"prerelease":false,
       "published_at":"2026-10-01T22:54:23Z",
       "assets":[{"name":"mstream-player-core2-v0.5.0-full.bin"}]},
      {"tag_name":"v0.4.0","name":"no merged image","draft":false,"prerelease":false,
       "published_at":"2026-09-20T10:00:00Z","assets":[{"name":"firmware.bin"}]}
    ]"#;

    /// A server on loopback answering each request by its path: status,
    /// extra headers, body. Every answer closes its connection. It serves
    /// `n` requests, then stops; its address is the base URL.
    pub(crate) fn serve(n: usize, routes: Vec<(&'static str, u16, Vec<(&'static str, String)>, Vec<u8>)>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for _ in 0..n {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let found = routes.iter().find(|(p, ..)| path.starts_with(p));
                let (status, headers, body) = match found {
                    Some((_, status, headers, body)) => (*status, headers.clone(), body.clone()),
                    None => (404, Vec::new(), b"not found".to_vec()),
                };
                let mut answer = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (name, value) in headers {
                    answer.push_str(&format!("{name}: {value}\r\n"));
                }
                answer.push_str("\r\n");
                let _ = stream.write_all(answer.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    /// A temp directory of the test's own, gone when dropped.
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(test: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("mstream-player-fw-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn net_at(dir: &Path, assets: &str, api: &str) -> Net {
        Net { dir: Some(dir.to_path_buf()), assets: Some(format!("{assets}/{{tag}}")), api: api.to_string() }
    }

    #[test]
    fn the_real_builds_description_reads_back() {
        let desc = AppDesc::in_app(APP_HEAD).expect("a description at 0x20");
        assert_eq!(desc.version, "v0.5.0-dev+ac93229-dirty");
        assert_eq!(desc.project, "mstream-mp3-player");
        assert_eq!(desc.idf, "v5.5.5");
        assert_eq!(desc.elf8, "a12e6cfd");
        assert!(desc.is_ours());
    }

    #[test]
    fn a_description_without_the_magic_word_is_nothing() {
        let mut bytes = desc_bytes("v0.5.0", "mstream-mp3-player");
        assert!(AppDesc::parse(&bytes).is_some());
        bytes[0] ^= 1;
        assert!(AppDesc::parse(&bytes).is_none());
        assert!(AppDesc::parse(&bytes[..100]).is_none(), "too short to be one");
        let other = AppDesc::parse(&desc_bytes("3.3.12", "arduino-lib-builder")).unwrap();
        assert!(!other.is_ours(), "the Arduino core's own description: other firmware");
    }

    #[test]
    fn a_file_is_a_merged_image_an_app_or_neither() {
        let desc = desc_bytes("v0.5.0", "mstream-mp3-player");
        let merged = merged_bytes(&desc);
        assert!(matches!(classify(&merged), Some((Layout::Merged, d)) if d.version == "v0.5.0"));
        let mut app = vec![0u8; AppDesc::OFFSET_IN_APP];
        app[0] = IMAGE_MAGIC;
        app.extend_from_slice(&desc);
        assert!(matches!(classify(&app), Some((Layout::App, d)) if d.version == "v0.5.0"));
        assert!(classify(APP_HEAD).is_some_and(|(layout, _)| layout == Layout::App));
        assert!(classify(b"not an image at all").is_none());
        let mut headers_only = vec![0xFF; APP_OFFSET + 0x200];
        headers_only[BOOTLOADER_OFFSET] = IMAGE_MAGIC;
        headers_only[APP_OFFSET] = IMAGE_MAGIC;
        assert!(classify(&headers_only).is_none(), "a merged image without a description is not ours");
    }

    #[test]
    fn an_images_flash_mode_is_the_clock_in_its_header_never_its_name() {
        // The real headers, read from the releases on 2026-10-10.
        assert_eq!(Mode::of_header(&[0xE9, 0x03, 0x02, 0x4F]), Some(Mode::Qio), "v0.7.0, v0.8.0: 80 MHz");
        assert_eq!(Mode::of_header(&[0xE9, 0x03, 0x02, 0x40]), Some(Mode::Dio), "v0.5.0, v0.8.0's -dio-full.bin");
        assert_eq!(Mode::of_header(&[0xE9, 0x03, 0x02, 0x41]), None, "26 MHz is no build of ours");
        assert_eq!(Mode::of_header(&[0x00, 0x03, 0x02, 0x40]), None, "not an image header");
        assert_eq!(Mode::of_header(&[0xE9, 0x03]), None);
        assert_eq!((header_mhz(&[0xE9, 3, 2, 0x4F]), header_mhz(&[0xE9, 3, 2, 0x40])), (Some(80), Some(40)));

        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("modes");
        for (mode, line) in [
            (Mode::Dio, "0x1000: E9 03 02 40 — 40 MHz, the DIO build"),
            (Mode::Qio, "0x1000: E9 03 02 4F — 80 MHz, the QIO build"),
        ] {
            // Named for the other build: the name says nothing, the header
            // does.
            let other = if mode == Mode::Qio { Mode::Dio } else { Mode::Qio };
            let file = scratch.0.join(format!("{}-full.bin", other.word()));
            std::fs::write(&file, merged_in(&desc_bytes("v0.8.0", AppDesc::OURS), mode)).unwrap();
            let read = read_local(&file).unwrap();
            assert_eq!(read.facts.mode, Some(mode));
            assert_eq!(header_line(&read.facts).as_deref(), Some(line));
            assert_eq!(read.facts.check, Check::Description);
            assert_eq!(read.facts.file.as_deref(), Some(file.as_path()));
        }
    }

    #[test]
    fn a_local_path_is_a_file_or_a_build_folder_or_says_why_not() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("local");
        let dir = &scratch.0;
        let merged = merged_bytes(&desc_bytes("v0.5.0-3-gabc1234", "mstream-mp3-player"));
        std::fs::write(dir.join("firmware.factory.bin"), &merged).unwrap();
        let from_dir = read_local(dir).unwrap();
        assert_eq!(from_dir.facts.version, "v0.5.0-3-gabc1234");
        assert_eq!(from_dir.segments.len(), 1);
        assert_eq!(from_dir.segments[0].offset, 0, "the merged image goes at 0x0");
        assert_eq!(from_dir.facts.bytes, merged.len());
        assert!(from_dir.facts.origin.contains("firmware.factory.bin"));
        assert_eq!(from_dir.facts.image, Image::Local(dir.clone()), "named as it was asked for");

        // PlatformIO's .pio/build/ holds one folder per environment.
        let pio = dir.join("pio");
        std::fs::create_dir_all(pio.join("m5stack-core2")).unwrap();
        assert!(read_local(&pio).unwrap_err().text().contains("No firmware.factory.bin"), "nothing built yet");
        std::fs::write(pio.join("m5stack-core2").join("firmware.factory.bin"), &merged).unwrap();
        assert_eq!(local_file(&pio).unwrap(), pio.join("m5stack-core2").join("firmware.factory.bin"));
        std::fs::create_dir_all(pio.join("core2-dio")).unwrap();
        std::fs::write(pio.join("core2-dio").join("firmware.factory.bin"), &merged).unwrap();
        let several = read_local(&pio).unwrap_err().text();
        assert!(several.contains("core2-dio, m5stack-core2"), "{several}");

        let other = dir.join("other.bin");
        std::fs::write(&other, merged_bytes(&desc_bytes("1.0", "someone-elses"))).unwrap();
        let err = read_local(&other).unwrap_err();
        assert!(err.text().contains("someone-elses"), "{}", err.text());

        let junk = dir.join("junk.bin");
        std::fs::write(&junk, b"hello").unwrap();
        let err = read_local(&junk).unwrap_err();
        assert!(err.text().contains("not a Core2 firmware image"), "{}", err.text());

        let missing = read_local(&dir.join("nope.bin")).unwrap_err();
        assert!(missing.text().contains("nope.bin"), "{}", missing.text());

        // An app alone: the command line writes it; the tab does not.
        let mut app = vec![0u8; AppDesc::OFFSET_IN_APP];
        app[..4].copy_from_slice(&[0xE9, 0x06, 0x02, 0x4F]);
        app.extend_from_slice(&desc_bytes("v0.8.0", AppDesc::OURS));
        std::fs::write(dir.join("firmware.bin"), &app).unwrap();
        let alone = read_local(&dir.join("firmware.bin")).unwrap();
        assert_eq!((alone.facts.layout, alone.facts.mode), (Layout::App, None), "its mode is the board's bootloader's");
        assert_eq!(alone.segments[0].offset, APP_OFFSET as u32);
        assert!(merged_only(&alone.facts).unwrap_err().text().contains("device flash --firmware"));
        assert!(merged_only(&from_dir.facts).is_ok());
    }

    #[test]
    fn a_flash_mode_flag_beside_a_file_must_agree_with_its_header() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("conflict");
        let file = scratch.0.join("dio.bin");
        std::fs::write(&file, merged_in(&desc_bytes("v0.8.0", AppDesc::OURS), Mode::Dio)).unwrap();
        let facts = read_local(&file).unwrap().facts;
        assert_eq!(mode_conflict(&facts, Mode::Dio), None);
        let refused = mode_conflict(&facts, Mode::Qio).unwrap().text();
        assert!(refused.contains("is the DIO build") && refused.contains("--flash-mode qio"), "{refused}");
        let mut unknown = facts.clone();
        unknown.mode = None;
        assert!(mode_conflict(&unknown, Mode::Dio).is_some(), "an unknown clock is no agreement");
    }

    #[test]
    fn the_pin_names_a_release_and_the_checksums_of_both_its_builds() {
        let _en = crate::setup::tests::in_locale("en");
        let hex = |sha: &str| sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        match (PINNED_TAG, PINNED_FULL_SHA256) {
            (Some(tag), Some(sha)) => {
                assert!(tag.starts_with('v') && tag[1..].split('.').count() == 3, "a release tag: {tag}");
                assert_eq!(full_asset_name(tag), format!("mstream-player-core2-{tag}-full.bin"));
                assert_eq!(dio_asset_name(tag), format!("mstream-player-core2-{tag}-dio-full.bin"));
                assert!(hex(sha), "{sha}");
                assert!(PINNED_DIO_SHA256.is_none_or(hex));
                assert_ne!(PINNED_DIO_SHA256, Some(sha), "two builds, two checksums");
            }
            (None, None) => {
                let err = Net::from_env().resolve(&Image::Pin(Mode::Qio), &mut |_, _| {}).unwrap_err();
                assert!(err.text().contains("--firmware"), "{}", err.text());
            }
            other => panic!("a tag without its checksum, or the other way round: {other:?}"),
        }
        // v0.8.0's -dio-full.bin, by its SHA256SUMS and GitHub's digest.
        assert_eq!(PINNED_DIO_SHA256, Some("01de38ec86dbad5b37b278ec30a159c93fede0a7a834e12ab8a6202cf5a3af8e"));
        let path = PathBuf::from("x.bin");
        assert_eq!(Image::from_flags(Some(path.clone()), Some("v9".into()), None), Some(Image::Local(path)), "a path outranks a tag");
        let v070 = Image::Release { tag: "v0.7.0".into(), mode: Mode::Qio };
        assert_eq!(Image::from_flags(None, Some("v0.7.0".into()), None), Some(v070));
        assert_eq!(Image::from_flags(None, Some("v0.8.0".into()), Some(Mode::Dio)), Some(Image::Pin(Mode::Dio)), "the pin's tag is the pin");
        assert_eq!(Image::from_flags(None, None, Some(Mode::Dio)), Some(Image::Pin(Mode::Dio)));
        let v050 = Image::Release { tag: "v0.5.0".into(), mode: Mode::Dio };
        assert_eq!(Image::from_flags(None, Some("v0.5.0".into()), None), Some(v050), "its one image, which is DIO");
        assert_eq!(Image::from_flags(None, None, None), None, "no flag: each board's own default");
    }

    #[test]
    fn the_pin_says_whether_its_release_answers_the_status_query() {
        // v0.8.0 answers `@err 7 status`: the page must not promise the
        // card after an update to it. The release after it answers.
        assert_eq!(PINNED_TAG, Some("v0.8.0"));
        assert!(!PINNED_ANSWERS_STATUS, "v0.8.0 has no @status");
        assert_eq!(pin_target(), Some(Target { version: "v0.8.0".into(), answers_status: PINNED_ANSWERS_STATUS }));
    }

    #[test]
    fn the_elf_table_tells_a_boards_mode_and_whether_it_was_a_choice() {
        let mode = |elf: &str| elf_mode(elf).map(|m| (m.mode, m.version, m.choice));
        assert_eq!(mode("e127a6bf"), Some((Mode::Qio, "v0.8.0".into(), true)));
        assert_eq!(mode("3523b80e"), Some((Mode::Dio, "v0.8.0".into(), true)));
        assert_eq!(mode("AA45F60E"), Some((Mode::Dio, "v0.7.0".into(), true)), "case does not matter");
        assert_eq!(mode("925d9680a1b2c3"), Some((Mode::Dio, "v0.6.0".into(), true)), "a longer id: its first eight");
        assert_eq!(mode("17352e55"), Some((Mode::Dio, "v0.5.0".into(), false)), "v0.5.0 had nothing but DIO");
        assert_eq!(mode("6a58c1f8").map(|m| m.2), Some(false));
        assert_eq!(mode("be894f89"), None, "the real dev build: never seen, never guessed");
        assert_eq!(mode("123"), None);
        // Every release up to the pin is in the table, the pin first.
        assert_eq!(BUILDS[0].tag, PINNED_TAG.unwrap());
        assert!(BUILDS.iter().all(|b| b.qio.is_none_or(|q| q.len() == 8) && b.dio.len() == 8));

        // An image in hand teaches its ELF: a local build in DIO.
        let scratch = Scratch::new("learn");
        let file = scratch.0.join("dio.bin");
        let desc = desc_with_elf("v0.8.0-5-g4e94418", AppDesc::OURS, &elf_bytes("be894f8a"));
        std::fs::write(&file, merged_in(&desc, Mode::Dio)).unwrap();
        read_local(&file).unwrap();
        assert_eq!(mode("be894f8a"), Some((Mode::Dio, "v0.8.0-5-g4e94418".into(), true)));
    }

    #[test]
    fn the_release_list_is_githubs_newest_first_with_what_each_release_has() {
        let releases = listed();
        let tags: Vec<&str> = releases.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["v0.8.0", "v0.7.0", "v0.6.0", "v0.5.0", "v0.5.0-beta.1"], "no draft, no release without a merged image");
        assert_eq!(releases[0].images, Images::Both);
        assert!(releases[0].is_pin() && !releases[0].newer_than_pin());
        assert_eq!(releases[1].date, "2026-10-07");
        assert_eq!(releases[1].title, "mStream Player v0.7.0 (beta)");
        assert_eq!(releases[3].images, Images::DioOnly, "v0.5.0: one image, from before QIO");
        assert_eq!(releases[4].images, Images::DioOnly);
        assert!(releases[4].pre && !releases[3].pre);
        let newer = Release { tag: "v0.9.0".into(), title: String::new(), date: String::new(), pre: false, images: Images::Both };
        assert!(newer.newer_than_pin());
        assert!(parse_releases("{\"message\":\"Not Found\"}").is_err());
        assert!(parse_releases("<html>").is_err());
    }

    #[test]
    fn a_release_is_checked_by_its_own_sums_and_written_offline_once_kept() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("release");
        let image = merged_in(&desc_with_elf("v0.7.0", AppDesc::OURS, &elf_bytes("aa45f60e")), Mode::Dio);
        let name = dio_asset_name("v0.7.0");
        let sums = format!("{}  {}\n{}  {}\n", sha256_hex(b"q"), full_asset_name("v0.7.0"), sha256_hex(&image), name);
        let base = serve(2, vec![
            ("/v0.7.0/SHA256SUMS", 200, Vec::new(), sums.clone().into_bytes()),
            ("/v0.7.0/mstream-player-core2-v0.7.0-dio-full.bin", 200, Vec::new(), image.clone()),
        ]);
        let net = net_at(&scratch.0, &base, "http://127.0.0.1:1");
        let wanted = Image::release("v0.7.0", Mode::Dio);
        let mut heard = 0;
        let got = net.resolve(&wanted, &mut |done, _| heard = done).unwrap();
        assert_eq!((got.facts.kind, got.facts.check, got.facts.mode), (Origin::Release, Check::Sums, Some(Mode::Dio)));
        assert_eq!(got.facts.elf, "aa45f60e");
        assert_eq!(heard, image.len() as u64);
        let kept = scratch.0.join("v0.7.0");
        assert_eq!(std::fs::read_to_string(kept.join("SHA256SUMS")).unwrap(), sums, "the sums kept beside it");
        // The server is gone: the copy kept and its sums are enough.
        let again = net.resolve(&wanted, &mut |_, _| panic!("nothing downloaded")).unwrap();
        assert_eq!((again.facts.kind, again.facts.origin.as_str()), (Origin::Cached, "release v0.7.0, downloaded earlier"));
        let cached = net.cached();
        assert_eq!(cached.len(), 1);
        assert_eq!((cached[0].tag.as_str(), cached[0].modes.clone()), ("v0.7.0", vec![Mode::Dio]));
        assert!(cached[0].since.is_some());
        // A kept image that no longer matches is never written.
        std::fs::write(kept.join(&name), b"tampered").unwrap();
        let offline = net.resolve(&wanted, &mut |_, _| {}).unwrap_err().text();
        assert!(offline.contains("could not download SHA256SUMS"), "{offline}");
        assert!(net.cached().is_empty());
    }

    #[test]
    fn a_download_that_does_not_match_its_sums_or_its_build_is_refused() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("refused");
        let qio = merged_in(&desc_bytes("v0.6.0", AppDesc::OURS), Mode::Qio);
        // The DIO file's name over the QIO build: the header decides.
        let sums = format!("{}  {}\n", sha256_hex(&qio), dio_asset_name("v0.6.0"));
        let base = serve(4, vec![
            ("/v0.6.0/SHA256SUMS", 200, Vec::new(), sums.into_bytes()),
            ("/v0.6.0/mstream-player-core2-v0.6.0-dio-full.bin", 200, Vec::new(), qio),
        ]);
        let net = net_at(&scratch.0, &base, "http://127.0.0.1:1");
        let wrong = net.resolve(&Image::release("v0.6.0", Mode::Dio), &mut |_, _| {}).unwrap_err().text();
        assert!(wrong.contains("is the QIO build, not DIO"), "{wrong}");
        assert!(!scratch.0.join("v0.6.0").join(dio_asset_name("v0.6.0")).exists(), "never kept");
        let unlisted = net.resolve(&Image::release("v0.6.0", Mode::Qio), &mut |_, _| {}).unwrap_err().text();
        assert!(unlisted.contains("does not list mstream-player-core2-v0.6.0-full.bin"), "{unlisted}");
        // v0.5.0 had one image, and it was DIO: refused before any request.
        let none = Net { dir: None, assets: Some("http://127.0.0.1:1".into()), api: String::new() };
        let no_qio = none.resolve(&Image::release("v0.5.0", Mode::Qio), &mut |_, _| {}).unwrap_err().text();
        assert!(no_qio.contains("v0.5.0 has one image, and it is DIO"), "{no_qio}");
        assert_eq!(Image::release("v0.5.0", Mode::Qio).has(Mode::Qio), Some(false));
        assert_eq!(Image::release("v0.9.0", Mode::Qio).has(Mode::Dio), None, "not known before asking");
    }

    #[test]
    fn the_list_says_offline_the_rate_limit_and_an_answer_that_is_not_the_list() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("list");
        let ok = serve(1, vec![("/repos/IrosTheBeggar/mstream-mp3-player/releases", 200, Vec::new(), GITHUB_LIST.into())]);
        let net = net_at(&scratch.0, "http://127.0.0.1:1", &ok);
        assert_eq!(net.releases().unwrap().len(), 5);

        let headers = vec![("x-ratelimit-remaining", "0".to_string()), ("x-ratelimit-reset", "1760086320".to_string())];
        let limited = serve(1, vec![("/repos", 403, headers, b"{\"message\":\"API rate limit exceeded\"}".to_vec())]);
        let why = net_at(&scratch.0, "", &limited).releases().unwrap_err();
        assert_eq!(why, ListWhy::Limited { reset: Some(1_760_086_320) });
        assert!(why.text().starts_with("GitHub: too many lists from this address; try after "), "{}", why.text());

        let broken = serve(1, vec![("/repos", 500, Vec::new(), b"oops".to_vec())]);
        assert_eq!(net_at(&scratch.0, "", &broken).releases(), Err(ListWhy::Status(500)));
        let html = serve(1, vec![("/repos", 200, Vec::new(), b"<html>captive portal</html>".to_vec())]);
        assert!(matches!(net_at(&scratch.0, "", &html).releases(), Err(ListWhy::NotJson(_))));
        // Nothing listens on port 1.
        let offline = net_at(&scratch.0, "", "http://127.0.0.1:1").releases().unwrap_err();
        assert!(matches!(offline, ListWhy::Offline(_)));
        assert_eq!(offline.text(), "GitHub did not answer: offline?");
    }

    #[test]
    fn versions_order_releases_pre_releases_and_describe_builds() {
        let v = |s: &str| Version::parse(s).unwrap_or_else(|| panic!("{s} parses"));
        assert_eq!(v("v0.8.0"), Version { core: (0, 8, 0), pre: vec![], ahead: 0, dev: false });
        assert_eq!(v("v0.8.0-5-g4e94418").ahead, 5);
        assert!(v("v0.8.0-5-g4e94418").dev);
        assert_eq!(v("v0.5.0-3-gabc1234-dirty"), Version { core: (0, 5, 0), pre: vec![], ahead: 3, dev: true });
        assert_eq!(v("v0.9.0-dev+abc1234-dirty"), Version { core: (0, 9, 0), pre: vec!["dev".into()], ahead: 0, dev: true });
        assert_eq!(v("v0.9.0-rc.1").pre, ["rc", "1"]);
        assert!(!v("v0.9.0-rc.1").dev, "a pre-release tag is a release");
        assert_eq!(v("v0.9.0-rc.1-2-gdeadbee").pre, ["rc", "1"], "describe past a pre-release tag");
        for junk in ["", "0.8", "v0.8", "v0.8.0.1", "3.3.12x", "va.b.c", "v0.8.0-"] {
            assert_eq!(Version::parse(junk), None, "{junk:?}");
        }
        assert!(Version::parse("3.3.12").is_none(), "another project's bare version is not ours to order");
    }

    #[test]
    fn a_board_is_up_to_date_only_on_the_very_version_and_never_offered_a_step_back() {
        let pin = "v0.8.0";
        assert_eq!(place("v0.8.0", pin), Place::Same);
        assert_eq!(place("v0.7.0", pin), Place::Older);
        assert_eq!(place("v0.6.0-37-g221d99d", pin), Place::Older, "a dev build based below the pin");
        assert_eq!(place("v0.9.0-rc.1", "v0.9.0"), Place::Older, "a pre-release before its release");
        assert_eq!(place("v0.9.0-dev+abc1234", "v0.9.0"), Place::Older, "the next release's dev build comes before it");
        assert_eq!(place("v0.9.0-dev+abc1234", pin), Place::Newer);
        // The real Core2 of 2026-10-10: a build five commits past the pin.
        assert_eq!(place("v0.8.0-5-g4e94418", pin), Place::Newer, "a dev build on the pin is ahead of it");
        assert_eq!(place("v0.8.0-dirty", pin), Place::Newer, "the tag with local changes is not the release");
        assert_eq!(place("v0.9.0", pin), Place::Newer);
        assert_eq!(place("v0.10.0", "v0.9.0"), Place::Newer, "numbers, not text");
        assert_eq!(place("v0.9.0-rc.2", "v0.9.0-rc.10"), Place::Older, "numeric identifiers as numbers");
        assert_eq!(place("v0.9.0-alpha", "v0.9.0-1"), Place::Newer, "alphanumeric after numeric");
        assert_eq!(place("nightly", pin), Place::Unknown);
    }

    #[test]
    fn a_download_that_stalls_fails_instead_of_holding_the_write_that_waits_for_it() {
        // A server on loopback that sends the head and a first piece, then
        // holds the socket open in silence: a connection that stalled.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut head = [0u8; 2048];
                let _ = stream.read(&mut head);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n0123456789");
                std::thread::sleep(Duration::from_secs(10));
            }
        });
        let url = format!("http://{addr}/image.bin");
        let t0 = std::time::Instant::now();
        let mut seen = 0;
        let stall = Duration::from_millis(300);
        let got = fetch_within(&url, MAX_IMAGE, &mut |done, _| seen = done, "image.bin", stall);
        assert!(matches!(got, Err(DeviceError::Firmware(_))), "{got:?}");
        assert_eq!(seen, 10, "the piece that came was heard");
        assert!(t0.elapsed() < Duration::from_secs(5), "given up after the stall, not the server's silence");
        assert!(READ_STALL >= Duration::from_secs(10), "a slow link that still moves is no stall");
    }

    #[test]
    fn the_release_names_and_the_sums_listing() {
        assert_eq!(full_asset_name("v0.5.0"), "mstream-player-core2-v0.5.0-full.bin");
        assert_eq!(
            full_asset_name("v0.5.0-dev+abc1234"),
            "mstream-player-core2-v0.5.0-dev_abc1234-full.bin",
            "the packager's `+` → `_`"
        );
        assert_eq!(dio_asset_name("v0.7.0"), "mstream-player-core2-v0.7.0-dio-full.bin");
        let text = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789ABCDEF  mstream-player-core2-v0.5.0-full.bin
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *SHA256SUMS-binary-mode
not a line
0123  too-short.bin
";
        let sums = parse_sums(text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].1, "mstream-player-core2-v0.5.0-full.bin");
        assert!(sums[0].0.ends_with("abcdef"), "lowercased");
        assert_eq!(sums[1].1, "SHA256SUMS-binary-mode");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        // Which file a build is, by what the release lists.
        let listed = |names: &[&str]| -> Vec<(String, String)> {
            names.iter().map(|n| (sha256_hex(n.as_bytes()), n.to_string())).collect()
        };
        let both = listed(&["mstream-player-core2-v0.7.0-full.bin", "mstream-player-core2-v0.7.0-dio-full.bin"]);
        assert_eq!(asset_for("v0.7.0", Mode::Dio, &both).unwrap(), dio_asset_name("v0.7.0"));
        assert_eq!(asset_for("v0.7.0", Mode::Qio, &both).unwrap(), full_asset_name("v0.7.0"));
        let one = listed(&["mstream-player-core2-v0.5.0-full.bin"]);
        assert_eq!(asset_for("v0.5.0", Mode::Dio, &one).unwrap(), full_asset_name("v0.5.0"), "its one image is the DIO one");
        assert!(asset_for("v0.5.0", Mode::Qio, &one).is_err());
    }

    #[test]
    fn the_env_points_the_downloads_and_the_list_elsewhere() {
        // The variables are process-global; this is the one test that
        // sets them: set, read, cleared.
        unsafe {
            std::env::set_var("MSTREAM_FIRMWARE_BASE", "http://127.0.0.1:1/fw/");
            std::env::set_var("MSTREAM_FIRMWARE_API", "http://127.0.0.1:2/");
        }
        let net = Net::from_env();
        unsafe {
            std::env::remove_var("MSTREAM_FIRMWARE_BASE");
            std::env::remove_var("MSTREAM_FIRMWARE_API");
        }
        assert_eq!(net.asset_url("v0.5.0", "SHA256SUMS"), "http://127.0.0.1:1/fw/SHA256SUMS");
        assert_eq!(net.releases_url(), "http://127.0.0.1:2/repos/IrosTheBeggar/mstream-mp3-player/releases?per_page=100");
        let tagged = Net { dir: None, assets: Some("http://127.0.0.1:1/{tag}/".into()), api: String::new() };
        assert_eq!(tagged.asset_url("v0.7.0", "SHA256SUMS"), "http://127.0.0.1:1/v0.7.0/SHA256SUMS", "{{tag}}: one folder a release");
        let github = Net::from_env();
        assert_eq!(
            github.asset_url("v0.5.0", "x.bin"),
            "https://github.com/IrosTheBeggar/mstream-mp3-player/releases/download/v0.5.0/x.bin"
        );
        assert_eq!(github.api, "https://api.github.com");
    }
}
