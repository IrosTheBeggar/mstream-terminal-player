//! The cards in this computer's readers: which mounted volumes the transfer
//! may write, and why each other one is refused. A card reaches this player
//! only in a PC's reader (the Core2's USB is a serial bridge), so this is
//! where the transfer meets the OS: [`find`] asks it about its volumes and
//! holds each to the drive-safety rule, [`inspect`] reads what an accepted
//! card holds, and [`CardFs`] is a card as the device lists it. Nothing here
//! writes to any volume; the run that writes (the engine) takes only a card
//! [`find`] accepted and the listener picked.
//!
//! **The rule** ([`judge`]) is one for every OS, once each OS has said what
//! it knows of a volume and its disk ([`linux`], [`windows`], [`macos`]):
//! - never one of the system's disks: one that holds the running system,
//!   its boot files or its swap ([`Disk::system`], each OS's own test);
//! - a card: removable media, or a disk on a USB, SD or MMC bus; never a
//!   fixed disk, and on a Mac never by "not internal" alone;
//! - a disk of its own: no other filesystem on it ([`Disk::others`]: a Pi's
//!   card in a reader holds its root filesystem beside the FAT one);
//! - what the player reads (the firmware repo's docs/METADATA.md 2.2):
//!   FAT32 or FAT16, on MBR or on no partition table; writable; and on
//!   Linux mounted so that names list as the card stores them (2.8.5).
//!
//! Every reason a volume fails is kept ([`Refusal`]), not only the first.
//! The words the listener reads are the UX contract's, still to come: until
//! then the reasons are codes, and the hidden `device card-probe` prints
//! them ([`probe`]).

pub(crate) mod linux;
pub(crate) mod macos;
pub(crate) mod windows;

use std::collections::HashSet;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde_json::{Value, json};

use super::contract::device_txt::DeviceTxt;
use super::contract::fat::fat_from_wall;
use super::contract::msmf::{self, Election, Manifest, Pick};
use super::contract::mptg::{self, Tags};
use super::contract::{MAX_FILE, Why, container, mpdj, mspd};
use super::names::{self, Listing};

/// A volume's filesystem, as the OS names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Fs {
    Fat32,
    Fat16,
    Fat12,
    ExFat,
    Ntfs,
    /// Any other, by the OS's own name for it (`ext4`, `apfs`, `RAW`).
    Other(String),
}

impl Fs {
    /// A FAT volume of `clusters` clusters, as the player's FatFs reads it:
    /// by that count alone, up to 4,085 FAT12 and up to 65,525 FAT16 (ff.c's
    /// MAX_FAT12 and MAX_FAT16, which it gives as DOS's and Windows's).
    /// Microsoft's spec and Linux's vfat put each boundary one lower, so at
    /// those two counts the PC reads another width: 4,085 is FAT12 here and
    /// refused either way; at 65,525 Linux mounts only a FAT32 volume, which
    /// FatFs then finds no filesystem in, so that count is no width the
    /// player reads (2.2).
    pub(crate) fn fat_of_clusters(clusters: u64) -> Fs {
        match clusters {
            0..=4085 => Fs::Fat12,
            4086..65525 => Fs::Fat16,
            65525 => Fs::Other("FAT, 65,525 clusters".into()),
            _ => Fs::Fat32,
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Fs::Fat32 => "FAT32",
            Fs::Fat16 => "FAT16",
            Fs::Fat12 => "FAT12",
            Fs::ExFat => "exFAT",
            Fs::Ntfs => "NTFS",
            Fs::Other(name) => name,
        }
    }

    /// The player's FatFs is built for FAT16 and FAT32 cards (2.2).
    fn player_reads(&self) -> bool {
        matches!(self, Fs::Fat32 | Fs::Fat16)
    }
}

/// How the disk is partitioned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Table {
    Mbr,
    Gpt,
    /// The filesystem fills the disk: no partition table at all.
    None,
    /// Another kind (Apple's, a BSD label), by the OS's name for it.
    Other(String),
    /// The OS didn't say (Linux without lsblk, or a query Windows refused):
    /// a sync may write, a format must not.
    Unknown,
}

impl Table {
    pub(crate) fn name(&self) -> &str {
        match self {
            Table::Mbr => "mbr",
            Table::Gpt => "gpt",
            Table::None => "none",
            Table::Other(name) => name,
            Table::Unknown => "unknown",
        }
    }
}

/// The bus a disk hangs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Bus {
    Usb,
    Sd,
    /// An MMC host whose card didn't say what it is.
    Mmc,
    /// Soldered flash on an MMC host (Linux's `device/type` MMC): a board's
    /// own storage, not a card.
    Emmc,
    Ata,
    Nvme,
    Scsi,
    /// No hardware: a mapped, RAID or loop device, a disk image.
    Virtual,
    Other(String),
    Unknown,
}

impl Bus {
    /// The buses a card reader sits on: a USB reader, a laptop's SD slot.
    fn card(&self) -> bool {
        matches!(self, Bus::Usb | Bus::Sd | Bus::Mmc)
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Bus::Usb => "usb",
            Bus::Sd => "sd",
            Bus::Mmc => "mmc",
            Bus::Emmc => "emmc",
            Bus::Ata => "ata",
            Bus::Nvme => "nvme",
            Bus::Scsi => "scsi",
            Bus::Virtual => "virtual",
            Bus::Other(name) => name,
            Bus::Unknown => "unknown",
        }
    }
}

/// The whole disk a volume is on, as one volume's verdict sees it: `others`
/// leaves that volume out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Disk {
    /// The OS's name for it: `PhysicalDrive2`, `sda`, `disk4`.
    pub(crate) id: String,
    /// Removable media (Linux's `removable`, Windows's RemovableMedia, a
    /// Mac's RemovableMedia or Ejectable).
    pub(crate) removable: bool,
    pub(crate) bus: Bus,
    /// The medium is write-protected (a card's lock switch, as the reader
    /// reports it).
    pub(crate) read_only: bool,
    /// Why it is one of the system's disks, each as the OS showed it (`/`,
    /// `swap`, `the Windows volume C:\`); empty: it isn't.
    pub(crate) system: Vec<String>,
    /// The other filesystems on it besides this volume, each by where or
    /// what it is (`ext4 at /media/rootfs`, `NTFS D:\`, `sda2 (unknown)`).
    pub(crate) others: Vec<String>,
}

/// The FAT volume serial, or the stand-in a Mac gives instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Serial {
    /// The boot sector's 32-bit serial (Windows; Linux's vfat UUID).
    Fat(u32),
    /// macOS's volume UUID, derived from the serial by the OS: as good for
    /// telling cards apart, never equal to the other two's.
    Uuid(String),
}

impl std::fmt::Display for Serial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Serial::Fat(n) => write!(f, "{:04X}-{:04X}", n >> 16, n & 0xFFFF),
            Serial::Uuid(text) => f.write_str(text),
        }
    }
}

impl Serial {
    /// `ABCD-1234`, as Linux's vfat UUID spells the serial; any other text
    /// is kept as a stand-in.
    pub(crate) fn parse(text: &str) -> Serial {
        let bytes = text.as_bytes();
        let fat = bytes.len() == 9
            && bytes[4] == b'-'
            && bytes.iter().enumerate().all(|(i, b)| i == 4 || b.is_ascii_hexdigit());
        match fat {
            true => Serial::Fat(u32::from_str_radix(&text.replace('-', ""), 16).unwrap_or(0)),
            false => Serial::Uuid(text.to_string()),
        }
    }
}

/// How the OS turns a card's FAT stamps (local wall-clock time with no zone,
/// at 2 s) into the times it reports (docs/METADATA.md 2.3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Clock {
    /// Windows: with the time zone bias in force at the moment of the call,
    /// undone with that same bias (`FileTimeToLocalFileTime`).
    Bias,
    /// Linux: by a fixed offset, the mount's `time_offset=` or `tz=UTC`, else
    /// the kernel's `sys_tz`: seconds to add to UTC to get the stamp back.
    Offset(i64),
    /// macOS and others: the platform's local offset now; the skew rule and
    /// the fingerprint cover any error.
    Local,
}

/// Size, free space and the cluster, as the mounted volume reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Space {
    pub(crate) size: u64,
    pub(crate) free: u64,
    pub(crate) cluster: u32,
    pub(crate) clusters: u64,
}

/// A mounted volume, as the OS reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Volume {
    /// Where its files are: `E:\`, `/media/card`, `/Volumes/CARD`.
    pub(crate) path: PathBuf,
    /// The OS's name for the volume itself: `\\?\Volume{…}\`, `sdb1`,
    /// `disk4s1`.
    pub(crate) device: String,
    pub(crate) fs: Fs,
    pub(crate) table: Table,
    pub(crate) label: String,
    pub(crate) serial: Option<Serial>,
    pub(crate) space: Option<Space>,
    /// Mounted or flagged read-only.
    pub(crate) read_only: bool,
    /// Linux: the mount lists names in UTF-8 (`utf8`, `iocharset=utf8`);
    /// None where the OS always does.
    pub(crate) utf8: Option<bool>,
    pub(crate) clock: Clock,
}

/// Why a volume is not a card the transfer may write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// One of the system's disks ([`Disk::system`]).
    System(Vec<String>),
    /// A fixed disk: not removable media, and not on a USB, SD or MMC bus.
    Fixed,
    /// The disk holds other filesystems besides this one ([`Disk::others`]).
    Shared(Vec<String>),
    /// A filesystem the player doesn't read: exFAT, NTFS, FAT12, any other.
    Filesystem(Fs),
    /// Not MBR: the player's FatFs is built without GPT (2.2).
    Table(Table),
    ReadOnly,
    /// Linux: a FAT mount whose names don't list as the card stores them —
    /// vfat without `utf8` or `iocharset=utf8`, or msdos's short names
    /// (2.8.5: the software MUST refuse it, saying how to remount it).
    NotUtf8,
}

impl Refusal {
    /// The reason as one stable word, for the debug listing and the tests.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Refusal::System(_) => "system-disk",
            Refusal::Fixed => "fixed-disk",
            Refusal::Shared(_) => "shared-disk",
            Refusal::Filesystem(Fs::ExFat) => "exfat",
            Refusal::Filesystem(Fs::Ntfs) => "ntfs",
            Refusal::Filesystem(Fs::Fat12) => "fat12",
            Refusal::Filesystem(_) => "filesystem",
            Refusal::Table(Table::Gpt) => "gpt",
            Refusal::Table(_) => "partition-table",
            Refusal::ReadOnly => "read-only",
            Refusal::NotUtf8 => "not-utf8",
        }
    }
}

/// The drive-safety rule: every reason the transfer may not write `volume`
/// on `disk`; none, and it is a card it may write once the listener picks
/// it. A partition table the OS didn't name passes: a sync writes files, not
/// the table (a format, which would, refuses it).
pub(crate) fn judge(disk: &Disk, volume: &Volume) -> Vec<Refusal> {
    let mut no = Vec::new();
    if !disk.system.is_empty() {
        no.push(Refusal::System(disk.system.clone()));
    }
    if !disk.removable && !disk.bus.card() {
        no.push(Refusal::Fixed);
    }
    if !disk.others.is_empty() {
        no.push(Refusal::Shared(disk.others.clone()));
    }
    if !volume.fs.player_reads() {
        no.push(Refusal::Filesystem(volume.fs.clone()));
    }
    if !matches!(volume.table, Table::Mbr | Table::None | Table::Unknown) {
        no.push(Refusal::Table(volume.table.clone()));
    }
    if volume.read_only || disk.read_only {
        no.push(Refusal::ReadOnly);
    }
    if volume.utf8 == Some(false) {
        no.push(Refusal::NotUtf8);
    }
    no
}

/// A volume [`find`] looked at, with its disk and its verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) volume: Volume,
    pub(crate) disk: Disk,
    /// [`judge`]'s answer: empty, a card the transfer may write.
    pub(crate) refusals: Vec<Refusal>,
}

impl Found {
    pub(crate) fn new(volume: Volume, disk: Disk) -> Found {
        let refusals = judge(&disk, &volume);
        Found { volume, disk, refusals }
    }

    pub(crate) fn accepted(&self) -> bool {
        self.refusals.is_empty()
    }
}

/// The mounted volumes a card could be, each judged: on Linux and macOS
/// every FAT volume and every volume of a card-shaped disk, on Windows every
/// drive letter of a removable or fixed drive. The cards the transfer may
/// write, and the rest with their reasons. Asked of the OS alone: no file on
/// any volume is opened (Windows looks for a page file's name at each
/// root). An OS this module has no probe for lists none: 2.8.5 lets the
/// software write only where a real-card check has shown that names list
/// as stored.
pub(crate) fn find() -> io::Result<Vec<Found>> {
    #[cfg(windows)]
    return windows::find();
    #[cfg(target_os = "linux")]
    return linux::find();
    #[cfg(target_os = "macos")]
    return macos::find();
    #[allow(unreachable_code)]
    Ok(Vec::new())
}

/// One entry of a card's directory, as the device would list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    /// The name as the card stores it (2.8.5).
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) size: u64,
    /// The raw FAT stamp, read back (2.3.4); 0 unknown.
    pub(crate) fat_time: u32,
}

/// A card, read as the device reads it: paths are card paths (`/music/…`,
/// `/.mstream/manifest.bin`; `/` the volume's root), every name is listed as
/// the card stores it (2.8.5), every stamp as its raw FAT fields (2.3.4).
/// The read half; the engine's runs add the writes.
pub(crate) trait CardFs {
    /// The entries of a folder, in no particular order.
    fn list(&self, dir: &str) -> io::Result<Vec<Item>>;
    /// One entry, None when there is none.
    fn stat(&self, path: &str) -> io::Result<Option<Item>>;
    /// A whole file, None when there is none or when it is longer than
    /// `max` (the kit treats such a file as absent unread: [`MAX_FILE`]).
    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>>;
    /// `len` bytes from `offset`, fewer at the file's end (the
    /// fingerprint's head and tail: 2.3.5).
    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>>;
}

/// A card [`find`] accepted, at its mount point.
#[derive(Clone, Debug)]
pub(crate) struct Mounted {
    root: PathBuf,
    clock: Clock,
}

impl Mounted {
    /// The card of an accepted volume; None for a refused one, which the
    /// transfer never opens.
    pub(crate) fn of(found: &Found) -> Option<Mounted> {
        let (root, clock) = (found.volume.path.clone(), found.volume.clock);
        found.accepted().then_some(Mounted { root, clock })
    }

    /// A card path's place on this computer: names only, each one that means
    /// itself here ([`plain_name`]). The paths come from the card's own files
    /// (a ledger, a plan), so none of them, however made, leaves the volume
    /// or opens another file on it.
    fn host(&self, card: &str) -> io::Result<PathBuf> {
        let refused = || io::Error::new(io::ErrorKind::InvalidInput, "not a card path");
        let mut path = self.root.clone();
        for name in card.split('/').filter(|n| !n.is_empty()) {
            if !plain_name(name) {
                return Err(refused());
            }
            path.push(name);
        }
        // The same said of the whole: the path is still under the root.
        if path.starts_with(&self.root) { Ok(path) } else { Err(refused()) }
    }

    fn item(&self, name: String, meta: &std::fs::Metadata) -> Item {
        let fat_time = meta.modified().ok().map_or(0, |t| fat_of(self.clock, unix_seconds(t)));
        Item { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, fat_time }
    }
}

impl CardFs for Mounted {
    fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
        let mut items = Vec::new();
        for entry in std::fs::read_dir(self.host(dir)?)? {
            let entry = entry?;
            let name = stored_name(&entry.file_name());
            items.push(self.item(name, &entry.metadata()?));
        }
        Ok(items)
    }

    fn stat(&self, path: &str) -> io::Result<Option<Item>> {
        let host = self.host(path)?;
        let meta = match std::fs::metadata(&host) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let name = host.file_name().map_or_else(String::new, stored_name);
        Ok(Some(self.item(name, &meta)))
    }

    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        let file = match std::fs::File::open(self.host(path)?) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if file.metadata()?.len() > max as u64 {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        file.take(max as u64 + 1).read_to_end(&mut bytes)?;
        Ok((bytes.len() <= max).then_some(bytes))
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(self.host(path)?)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(len);
        file.take(len as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// A card path's component that [`Mounted::host`] may join: not `.` or `..`,
/// and no `\`, `:` or NUL. No FAT name holds those (2.8.3), and Windows reads
/// them as a separator, a drive, a share or a device (`C:`, `\\host`,
/// `\\.\PhysicalDrive0`), which a push puts in place of the volume's root.
/// On Windows also no name it opens as another: one ending in a dot or a
/// space opens the name without them, and a DOS device's name, alone or with
/// an extension (`CON`, `aux.mp3`; 2.8.3 step 6), opens the device on
/// Windows 10. Elsewhere those are names like any other.
fn plain_name(name: &str) -> bool {
    let plain = name != "." && name != ".." && !name.contains(['\\', ':', '\0']);
    plain && !(cfg!(windows) && win32_alias(name))
}

/// A name Windows opens as another: see [`plain_name`].
fn win32_alias(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |port: &str| {
        let mut digit = stem.strip_prefix(port).unwrap_or_default().chars();
        matches!((digit.next(), digit.next()), (Some('0'..='9' | '¹' | '²' | '³'), None))
    };
    name.ends_with(['.', ' '])
        || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || numbered("COM")
        || numbered("LPT")
}

/// A name as the OS listed it, brought back to the card's spelling (2.8.5)
/// by the path rules' one place for it ([`names::stored_name`], this OS's
/// [`Listing`]): Windows and a utf8 Linux mount list the stored long name
/// unchanged; macOS lists names decomposed, so they go back to NFC.
fn stored_name(listed: &std::ffi::OsStr) -> String {
    names::stored_name(&listed.to_string_lossy(), Listing::here()).into_owned()
}

/// Seconds since 1970 of a time the OS reported (negative before).
fn unix_seconds(t: std::time::SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs_f64().ceil() as i64),
    }
}

/// 1980-01-01 00:00 in seconds since 1970: FAT's epoch.
const FAT_EPOCH: i64 = 315_532_800;

/// The raw FAT stamp of a file the OS dates `utc` (Unix seconds), undone as
/// `clock` says this OS made it (2.3.4).
fn fat_of(clock: Clock, utc: i64) -> u32 {
    let wall = match clock {
        Clock::Offset(offset) => utc + offset,
        Clock::Bias => windows::local_wall(utc),
        Clock::Local => utc + macos::local_offset(),
    };
    fat_from_wall(wall - FAT_EPOCH)
}

/// What an accepted card holds, read with the contract kit and changed
/// nothing: a look that only reads takes the same winners a run would and
/// finishes none of their renames (2.12.7).
#[derive(Clone, Debug)]
pub(crate) struct Inspection {
    /// The root that counts (2.5.4); None, the card has no transfer data.
    pub(crate) root: Option<Root>,
    /// The contract and private files whose major or version this software
    /// doesn't know (2.4.5, 2.12.7): it must write nothing to this card.
    pub(crate) newer: Vec<String>,
    /// A cut run's plan (2.12.5): `pending.bin`, or a valid `pending.tmp`
    /// when it wins.
    pub(crate) pending: Option<mspd::Plan>,
    /// `state.json` (the card's settings) and `resume.json`, each by its read
    /// rule (2.12.7).
    pub(crate) state: Private,
    pub(crate) resume: Private,
    /// `/.player/device.txt`, or its no-file values (2.15).
    pub(crate) device: DeviceTxt,
    pub(crate) music: Music,
}

/// The root that counts, and the companions it names (2.5.3, 2.5.4).
#[derive(Clone, Debug)]
pub(crate) struct Root {
    /// `manifest.bin` or `manifest.tmp`.
    pub(crate) file: &'static str,
    /// Both valid and one commit seen twice: a rename may have been cut, and
    /// a run stops for a disk check (2.12.1, step 3).
    pub(crate) twice: bool,
    pub(crate) manifest: Manifest,
    /// The tags file it names, or why it is absent: a companion that fails
    /// is absent, and the root stays valid (2.5.3).
    pub(crate) tags: Result<Tags, Why>,
    /// Its Auto DJ index's verdict, when it names one.
    pub(crate) autodj: Option<Result<(), Why>>,
}

/// A private file of 2.12.7 as its read rule settles it, read only.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Private {
    /// The file that counts: `X.json`, or `X.new` when it wins; None when
    /// neither is valid. Or the file of a `version` this software doesn't
    /// know, which stops it: then nothing of either is read.
    pub(crate) file: Option<String>,
    /// The winner, a JSON object of its own `format`.
    pub(crate) value: Option<Value>,
    pub(crate) seq: u64,
    /// That file's version.
    pub(crate) version: u64,
    /// Both files there and byte-equal: one write seen twice; a run stops
    /// for a disk check.
    pub(crate) twice: bool,
}

/// `/music` in numbers: what the walk would list, and how much of it the
/// committed ledger doesn't (2.11: on a card with no ledger, all of it is
/// the listener's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Music {
    pub(crate) present: bool,
    pub(crate) folders: u32,
    pub(crate) files: u32,
    /// The files whose extension the device lists as audio (2.15).
    pub(crate) audio: u32,
    pub(crate) audio_bytes: u64,
    /// The audio files no ledger row lists.
    pub(crate) unlisted: u32,
    pub(crate) unlisted_bytes: u64,
}

/// The schema majors of the private files this software reads (2.12.7).
const STATE_VERSION: u64 = 1;
const RESUME_VERSION: u64 = 1;

/// The deepest folder the music summary walks into: the device lists 8
/// levels, so this only bounds a card made strangely.
const WALK_DEPTH: usize = 32;

/// Reads an accepted card's state: what a run's steps 1 and 2 would settle
/// on, read only (2.5.4, 2.12.1, 2.12.5, 2.12.7), `/.player/device.txt`
/// (2.15), and `/music` in numbers. A file that can't be read is absent, as
/// for the device; an error listing the root is the card's (pulled, say).
pub(crate) fn inspect(card: &dyn CardFs) -> io::Result<Inspection> {
    let names: Vec<String> = match card.list("/.mstream") {
        Ok(items) => items.into_iter().filter(|i| !i.dir).map(|i| i.name).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let read = |name: &str| card.read(&format!("/.mstream/{name}"), MAX_FILE).ok().flatten();
    let head = |name: &str| card.read_at(&format!("/.mstream/{name}"), 0, 6);
    let mut newer: Vec<String> = names
        .iter()
        .filter(|n| contract_named(n) && head(n).is_ok_and(|h| container::unknown_major(&h)))
        .cloned()
        .collect();
    newer.sort();
    let root = settle_root(&read);
    let pending = settle_pending(&read);
    if pending.as_ref().is_some_and(|p| p.stops_software()) {
        newer.push("pending.bin".into());
    }
    let private = |x: &str, known: u64| {
        let (json, new) = (read(&format!("{x}.json")), read(&format!("{x}.new")));
        settle_private(json.as_deref(), new.as_deref(), x, known)
    };
    let (state, resume) = (private("state", STATE_VERSION), private("resume", RESUME_VERSION));
    for (private, known) in [(&state, STATE_VERSION), (&resume, RESUME_VERSION)] {
        if let Some(file) = private.file.as_ref().filter(|_| private.version != known) {
            newer.push(file.clone());
        }
    }
    let device = DeviceTxt::read(card.read("/.player/device.txt", 64 * 1024).ok().flatten().as_deref());
    let listed: HashSet<&str> = match root.as_ref().map(|r| &r.tags) {
        Some(Ok(tags)) => {
            tags.records.iter().filter(|e| e.ledger.is_some()).map(|e| e.path.as_str()).collect()
        }
        _ => HashSet::new(),
    };
    let music = summarize_music(card, &device, &listed)?;
    Ok(Inspection { root, newer, pending, state, resume, device, music })
}

/// The names 2.4.5's check reads: `manifest.*` and `pending.*` (`.bin`,
/// `.tmp`), `tags-*.bin`, `autodj-*.bin`.
fn contract_named(name: &str) -> bool {
    matches!(name, "manifest.bin" | "manifest.tmp" | "pending.bin" | "pending.tmp")
        || (name.starts_with("tags-") || name.starts_with("autodj-")) && name.ends_with(".bin")
}

/// 2.5.4: both roots validated, the election, then the companions the
/// winner names, each valid only when it validates and matches its entry.
fn settle_root(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<Root> {
    let valid = |name| read(name).and_then(|b| Manifest::read(&b, msmf::Uses::ALL).ok());
    let (bin, tmp) = (valid("manifest.bin"), valid("manifest.tmp"));
    let Election { pick, same_commit } = msmf::elect(
        bin.as_ref().map(Manifest::candidate),
        tmp.as_ref().map(Manifest::candidate),
    );
    let (file, manifest) = match pick {
        Pick::Bin => ("manifest.bin", bin?),
        Pick::Tmp => ("manifest.tmp", tmp?),
        Pick::None => return None,
    };
    let tags = match read(&manifest.tags.name()) {
        None => Err(Why::Missing),
        Some(bytes) => match mptg::read(&bytes, mptg::Uses::ALL) {
            Ok(tags) => match msmf::companion_matches(&manifest.tags, &tags.header, tags.source, None) {
                true => Ok(tags),
                false => Err(Why::Missing),
            },
            Err(why) => Err(why),
        },
    };
    let autodj = manifest.autodj.as_ref().map(|entry| {
        let bytes = read(&entry.name()).ok_or(Why::Missing)?;
        let table = mpdj::Table::open(&bytes)?;
        table.check(mpdj::Uses::ALL)?;
        let ok = msmf::companion_matches(entry, table.header(), 0, Some(&table.sig));
        if ok { Ok(()) } else { Err(Why::Missing) }
    });
    Some(Root { file, twice: same_commit, manifest, tags, autodj })
}

/// 2.12.5: `pending.bin`, or a valid `pending.tmp` that wins by 2.5.4's rule.
fn settle_pending(read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<mspd::Plan> {
    let valid = |name| read(name).and_then(|b| mspd::Plan::read(&b).ok());
    let (bin, tmp) = (valid("pending.bin"), valid("pending.tmp"));
    let candidate = |plan: &Option<mspd::Plan>| plan.as_ref().map(mspd::Plan::candidate);
    match msmf::elect(candidate(&bin), candidate(&tmp)).pick {
        Pick::Bin => bin,
        Pick::Tmp => tmp,
        Pick::None => None,
    }
}

/// 2.12.7's read, for `X` = `state` or `resume`: the valid file, or of two
/// different valid ones the higher `seq` (equal: `X.json`); two byte-equal
/// files are one write seen twice. A file of a `version` other than `known`
/// stops the software, whichever of the two it is: it reads none of it, and
/// that file is the one named, with its version. Nothing is finished or
/// deleted here.
fn settle_private(json: Option<&[u8]>, new: Option<&[u8]>, x: &str, known: u64) -> Private {
    let format = format!("mstream-card-{x}");
    let valid = |bytes: Option<&[u8]>| bytes.and_then(|b| private_value(b, &format, known));
    let twice = matches!((json, new), (Some(a), Some(b)) if a == b);
    for (file, form) in [(format!("{x}.json"), valid(json)), (format!("{x}.new"), valid(new))] {
        if let Some((_, _, version)) = form.filter(|f| f.2 != known) {
            return Private { file: Some(file), version, twice, ..Private::default() };
        }
    }
    let pick = match (valid(json), valid(new)) {
        (Some(j), Some(n)) if n.1 > j.1 => Some((format!("{x}.new"), n)),
        (Some(j), _) => Some((format!("{x}.json"), j)),
        (None, Some(n)) => Some((format!("{x}.new"), n)),
        (None, None) => None,
    };
    match pick {
        Some((file, (value, seq, version))) => {
            Private { file: Some(file), value: Some(value), seq, version, twice }
        }
        None => Private { twice, ..Private::default() },
    }
}

/// A private file is valid when it is UTF-8 with no byte-order mark that
/// parses whole as one object with its own `format`, no member named twice
/// in any object, an integer `version` from 1 and an integer `seq` from 1 to
/// 2^53 − 1 (2.12.7): the object, its seq and its version. A version other
/// than `known` is a newer software's, whose members, `seq` among them,
/// this one doesn't read: seq 0 then.
fn private_value(bytes: &[u8], format: &str, known: u64) -> Option<(Value, u64, u64)> {
    let text = std::str::from_utf8(bytes).ok().filter(|t| !t.starts_with('\u{FEFF}'))?;
    let value = serde_json::from_str::<Unique>(text).ok()?.0;
    let object = value.as_object()?;
    if object.get("format")?.as_str()? != format {
        return None;
    }
    let version = object.get("version")?.as_u64().filter(|v| *v >= 1)?;
    if version != known {
        return Some((value, 0, version));
    }
    let seq = object.get("seq")?.as_u64().filter(|s| (1..1u64 << 53).contains(s))?;
    Some((value, seq, version))
}

/// JSON as serde_json reads it into a [`Value`], but for an object with a
/// member named twice, which is an error here: serde_json keeps the last
/// one, and 2.12.7 makes such a file invalid.
struct Unique(Value);

impl<'de> serde::Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Unique, D::Error> {
        d.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueVisitor {
    type Value = Unique;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("JSON with each object's members named once")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Unique, E> {
        Ok(Unique(Value::Bool(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Unique, E> {
        Ok(Unique(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Unique, E> {
        Ok(Unique(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Unique, E> {
        Ok(Unique(v.into()))
    }

    fn visit_str<E>(self, v: &str) -> Result<Unique, E> {
        Ok(Unique(v.into()))
    }

    fn visit_unit<E>(self) -> Result<Unique, E> {
        Ok(Unique(Value::Null))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut items: A) -> Result<Unique, A::Error> {
        let mut out = Vec::new();
        while let Some(Unique(v)) = items.next_element()? {
            out.push(v);
        }
        Ok(Unique(Value::Array(out)))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut members: A) -> Result<Unique, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(name) = members.next_key::<String>()? {
            if out.contains_key(&name) {
                return Err(serde::de::Error::custom("a member named twice"));
            }
            let Unique(v) = members.next_value()?;
            out.insert(name, v);
        }
        Ok(Unique(Value::Object(out)))
    }
}

/// Walks `/music` as the device's walk would list it, dot names left out
/// (2.2), counting what is there and what no ledger row lists. The folder is
/// found by name in any case: FAT matches names case-insensitively, and a
/// utf8 Linux mount doesn't.
fn summarize_music(card: &dyn CardFs, device: &DeviceTxt, listed: &HashSet<&str>) -> io::Result<Music> {
    let mut music = Music::default();
    let top = card.list("/")?.into_iter().find(|i| i.dir && i.name.eq_ignore_ascii_case("music"));
    let Some(top) = top else { return Ok(music) };
    music.present = true;
    // (card path, path relative to /music, depth)
    let mut stack = vec![(format!("/{}", top.name), String::new(), 0usize)];
    while let Some((dir, rel, depth)) = stack.pop() {
        let Ok(items) = card.list(&dir) else { continue };
        for item in items.into_iter().filter(|i| !i.name.starts_with('.')) {
            let rel = if rel.is_empty() { item.name.clone() } else { format!("{rel}/{}", item.name) };
            if item.dir {
                music.folders += 1;
                if depth + 1 < WALK_DEPTH {
                    stack.push((format!("{dir}/{}", item.name), rel, depth + 1));
                }
                continue;
            }
            music.files += 1;
            let audio = item.name.rsplit_once('.').is_some_and(|(_, ext)| device.lists(ext));
            if audio {
                music.audio += 1;
                music.audio_bytes += item.size;
                if !listed.contains(rel.as_str()) {
                    music.unlisted += 1;
                    music.unlisted_bytes += item.size;
                }
            }
        }
    }
    Ok(music)
}

/// `device card-probe`, undocumented: every volume [`find`] looked at, as
/// JSON on stdout — its facts, its disk, its verdict with every reason, and
/// for an accepted card what [`inspect`] reads on it. For the real-card
/// checks; the listener's `device cards` waits for its UX contract. Read
/// only. Exit 0, or 2 when the OS can't list its volumes.
pub(crate) fn probe(out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let found = match find() {
        Ok(found) => found,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: the volumes can't be listed: {e}");
            return 2;
        }
    };
    let cards: Vec<Value> = found
        .iter()
        .map(|f| {
            let mut card = found_json(f);
            if let Some(mounted) = Mounted::of(f) {
                card["inspect"] = match inspect(&mounted) {
                    Ok(seen) => inspection_json(&seen),
                    Err(e) => json!({ "error": e.to_string() }),
                };
            }
            card
        })
        .collect();
    let _ = writeln!(out, "{}", serde_json::to_string_pretty(&Value::Array(cards)).unwrap_or_default());
    0
}

fn found_json(f: &Found) -> Value {
    let (v, d) = (&f.volume, &f.disk);
    let refusals: Vec<Value> = f
        .refusals
        .iter()
        .map(|r| match r {
            Refusal::System(why) | Refusal::Shared(why) => json!({ "code": r.code(), "what": why }),
            Refusal::Filesystem(fs) => json!({ "code": r.code(), "what": fs.name() }),
            Refusal::Table(table) => json!({ "code": r.code(), "what": table.name() }),
            _ => json!({ "code": r.code() }),
        })
        .collect();
    let clock = match v.clock {
        Clock::Bias => json!("bias"),
        Clock::Offset(seconds) => json!({ "offset": seconds }),
        Clock::Local => json!("local"),
    };
    json!({
        "path": v.path.display().to_string(),
        "device": v.device,
        "fs": v.fs.name(),
        "table": v.table.name(),
        "label": v.label,
        "serial": v.serial.as_ref().map(Serial::to_string),
        "size": v.space.map(|s| s.size),
        "free": v.space.map(|s| s.free),
        "cluster": v.space.map(|s| s.cluster),
        "readOnly": v.read_only,
        "utf8": v.utf8,
        "clock": clock,
        "disk": {
            "id": d.id,
            "removable": d.removable,
            "bus": d.bus.name(),
            "readOnly": d.read_only,
            "system": d.system,
            "others": d.others,
        },
        "accepted": f.accepted(),
        "refusals": refusals,
    })
}

fn inspection_json(seen: &Inspection) -> Value {
    let root = seen.root.as_ref().map(|r| {
        let m = &r.manifest;
        let tags = match &r.tags {
            Ok(t) => json!({
                "name": m.tags.name(),
                "records": t.records.len(),
                "ledger": t.has_ledger,
                "rows": t.records.iter().filter(|e| e.ledger.is_some()).count(),
            }),
            Err(why) => json!({ "name": m.tags.name(), "absent": why.name() }),
        };
        let autodj = r.autodj.as_ref().map(|v| match v {
            Ok(()) => json!("valid"),
            Err(why) => json!({ "absent": why.name() }),
        });
        json!({
            "file": r.file,
            "twice": r.twice,
            "generation": m.header.generation,
            "cardId": format!("0x{:016X}", m.header.card_id),
            "commitId": format!("0x{:016X}", m.commit_id),
            "final": m.is_final(),
            "producer": m.producer,
            "roots": m.roots,
            "servers": m.servers.len() + 1,
            "tags": tags,
            "autodj": autodj,
        })
    });
    let private = |p: &Private| {
        let pick = |key: &str| p.value.as_ref().and_then(|v| v.get(key)).cloned();
        json!({
            "file": p.file,
            "seq": p.seq,
            "version": p.version,
            "twice": p.twice,
            "label": pick("label"),
            "cardId": pick("cardId"),
        })
    };
    let d = &seen.device;
    let m = &seen.music;
    json!({
        "root": root,
        "newer": seen.newer,
        "pending": seen.pending.as_ref().map(|p| json!({ "ops": p.ops.len(), "base": p.base_generation })),
        "state": private(&seen.state),
        "resume": private(&seen.resume),
        "deviceTxt": {
            "present": d.present,
            "firmware": d.firmware,
            "codecs": d.codecs,
            "extensions": d.extensions,
            "mpdj": d.writes_mpdj(),
        },
        "music": {
            "present": m.present,
            "folders": m.folders,
            "files": m.files,
            "audio": m.audio,
            "audioBytes": m.audio_bytes,
            "unlisted": m.unlisted,
            "unlistedBytes": m.unlisted_bytes,
        },
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/golden");
        std::fs::read(dir.join(name)).unwrap()
    }

    /// A card in memory: files by card path with their stamps, folders
    /// implied.
    #[derive(Default)]
    struct Card {
        files: BTreeMap<String, (Vec<u8>, u32)>,
    }

    impl Card {
        fn put(&mut self, path: &str, bytes: &[u8]) -> &mut Self {
            self.files.insert(path.to_string(), (bytes.to_vec(), 0x5D47_73D5));
            self
        }

        fn golden(&mut self, name: &str, as_name: &str) -> &mut Self {
            self.put(&format!("/.mstream/{as_name}"), &fixture(name))
        }
    }

    impl CardFs for Card {
        fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
            let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
            let mut items: BTreeMap<String, Item> = BTreeMap::new();
            for (path, (bytes, fat_time)) in &self.files {
                let Some(rest) = path.strip_prefix(&prefix) else { continue };
                let (name, dir) = match rest.split_once('/') {
                    Some((name, _)) => (name, true),
                    None => (rest, false),
                };
                let size = if dir { 0 } else { bytes.len() as u64 };
                let item = Item { name: name.to_string(), dir, size, fat_time: *fat_time };
                items.insert(name.to_string(), item);
            }
            if items.is_empty() && dir != "/" {
                return Err(io::ErrorKind::NotFound.into());
            }
            Ok(items.into_values().collect())
        }

        fn stat(&self, path: &str) -> io::Result<Option<Item>> {
            let (dir, name) = path.rsplit_once('/').unwrap();
            let items = self.list(if dir.is_empty() { "/" } else { dir }).unwrap_or_default();
            Ok(items.into_iter().find(|i| i.name == name))
        }

        fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).map(|f| f.0.clone()).filter(|b| b.len() <= max))
        }

        fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let bytes = &self.files.get(path).ok_or(io::ErrorKind::NotFound)?.0;
            let start = (offset as usize).min(bytes.len());
            Ok(bytes[start..(start + len).min(bytes.len())].to_vec())
        }
    }

    fn card_disk() -> Disk {
        let (system, others) = (Vec::new(), Vec::new());
        Disk { id: "sdz".into(), removable: true, bus: Bus::Usb, read_only: false, system, others }
    }

    fn card_volume(path: &Path) -> Volume {
        Volume {
            path: path.to_path_buf(),
            device: "sdz1".into(),
            fs: Fs::Fat32,
            table: Table::Mbr,
            label: "CARD".into(),
            serial: Some(Serial::Fat(0x1234_ABCD)),
            space: None,
            read_only: false,
            utf8: Some(true),
            clock: Clock::Offset(0),
        }
    }

    fn codes(disk: &Disk, volume: &Volume) -> Vec<&'static str> {
        judge(disk, volume).iter().map(Refusal::code).collect()
    }

    #[test]
    fn the_rule_takes_a_card_and_names_every_reason_it_refuses_one() {
        let (disk, volume) = (card_disk(), card_volume(Path::new("/media/card")));
        assert!(judge(&disk, &volume).is_empty());
        // A slot that says removable=0 passes by its bus; an MMC host's
        // soldered eMMC doesn't, nor any other fixed disk.
        assert!(judge(&Disk { removable: false, bus: Bus::Sd, ..card_disk() }, &volume).is_empty());
        assert!(judge(&Disk { removable: false, bus: Bus::Mmc, ..card_disk() }, &volume).is_empty());
        assert_eq!(codes(&Disk { removable: false, bus: Bus::Emmc, ..card_disk() }, &volume), ["fixed-disk"]);
        assert_eq!(codes(&Disk { removable: false, bus: Bus::Nvme, ..card_disk() }, &volume), ["fixed-disk"]);
        // Removable media on any bus is a card's shape.
        assert!(judge(&Disk { bus: Bus::Unknown, ..card_disk() }, &volume).is_empty());

        let system = Disk { system: vec!["/".into()], ..card_disk() };
        assert_eq!(judge(&system, &volume), [Refusal::System(vec!["/".into()])]);
        let shared = Disk { others: vec!["ext4 on sdz2".into()], ..card_disk() };
        assert_eq!(codes(&shared, &volume), ["shared-disk"]);
        let filesystems =
            [(Fs::ExFat, "exfat"), (Fs::Ntfs, "ntfs"), (Fs::Fat12, "fat12"), (Fs::Other("ext4".into()), "filesystem")];
        for (fs, code) in filesystems {
            assert_eq!(codes(&disk, &Volume { fs, ..volume.clone() }), [code]);
        }
        assert!(judge(&disk, &Volume { fs: Fs::Fat16, ..volume.clone() }).is_empty());
        assert_eq!(codes(&disk, &Volume { table: Table::Gpt, ..volume.clone() }), ["gpt"]);
        let apm = Volume { table: Table::Other("apm".into()), ..volume.clone() };
        assert_eq!(codes(&disk, &apm), ["partition-table"]);
        // No table, or one the OS wouldn't name: a sync may write.
        assert!(judge(&disk, &Volume { table: Table::None, ..volume.clone() }).is_empty());
        assert!(judge(&disk, &Volume { table: Table::Unknown, ..volume.clone() }).is_empty());
        assert_eq!(codes(&disk, &Volume { read_only: true, ..volume.clone() }), ["read-only"]);
        assert_eq!(codes(&Disk { read_only: true, ..card_disk() }, &volume), ["read-only"]);
        assert_eq!(codes(&disk, &Volume { utf8: Some(false), ..volume.clone() }), ["not-utf8"]);
        assert!(judge(&disk, &Volume { utf8: None, ..volume.clone() }).is_empty());

        // All of them at once, in the rule's order.
        let system = vec!["swap".into()];
        let worst = Disk { removable: false, bus: Bus::Nvme, read_only: true, system, ..shared };
        let all = Volume { fs: Fs::ExFat, table: Table::Gpt, utf8: Some(false), ..volume };
        assert_eq!(
            codes(&worst, &all),
            ["system-disk", "fixed-disk", "shared-disk", "exfat", "gpt", "read-only", "not-utf8"]
        );
    }

    #[test]
    fn fat_widths_serials_and_clocks() {
        // FatFs's boundaries, each one past the spec's: the player's reading
        // is the one that counts.
        assert_eq!(Fs::fat_of_clusters(4085), Fs::Fat12);
        assert_eq!(Fs::fat_of_clusters(4086), Fs::Fat16);
        assert_eq!(Fs::fat_of_clusters(65_524), Fs::Fat16);
        let volume = card_volume(Path::new("/c"));
        let width = |clusters| Volume { fs: Fs::fat_of_clusters(clusters), ..volume.clone() };
        assert_eq!(codes(&card_disk(), &width(65_525)), ["filesystem"], "no width the player reads");
        assert_eq!(Fs::fat_of_clusters(65_526), Fs::Fat32);
        assert_eq!(Serial::parse("1A2B-3C4D"), Serial::Fat(0x1A2B_3C4D));
        assert_eq!(Serial::Fat(0x0001_00AB).to_string(), "0001-00AB");
        assert_eq!(Serial::parse("0f1e2d3c-aaaa"), Serial::Uuid("0f1e2d3c-aaaa".into()));
        assert_eq!(Serial::parse("1A2B-3C4G"), Serial::Uuid("1A2B-3C4G".into()));

        // 2.3.4's example: 2026-10-07 14:30:42 is 0x5D4773D5 on the card's
        // wall clock. A mount at UTC reports it as that instant; one an hour
        // east as an hour before.
        let at = 1_791_383_442;
        assert_eq!(fat_of(Clock::Offset(0), at), 0x5D47_73D5);
        assert_eq!(fat_of(Clock::Offset(3600), at - 3600), 0x5D47_73D5);
        assert_eq!(fat_of(Clock::Offset(0), at + 3600), 0x5D47_7BD5);
        assert_eq!(fat_of(Clock::Offset(0), FAT_EPOCH - 2), 0, "before 1980: no stamp");
        // Windows' bias and a Mac's offset are this machine's own: whole
        // quarter hours, within a day.
        for clock in [Clock::Bias, Clock::Local] {
            let wall = match clock {
                Clock::Bias => windows::local_wall(at),
                _ => at + macos::local_offset(),
            };
            assert_eq!((wall - at) % 900, 0, "{clock:?}");
            assert!((wall - at).abs() <= 86_400, "{clock:?}");
        }
        // A listed name as stored: NFC again on a Mac, as listed elsewhere.
        let nfd = std::ffi::OsStr::new("Cafe\u{301}");
        assert_eq!(stored_name(nfd), if cfg!(target_os = "macos") { "Caf\u{E9}" } else { "Cafe\u{301}" });
        assert_eq!(stored_name(std::ffi::OsStr::new("Caf\u{E9}")), "Caf\u{E9}");
    }

    fn music(card: &mut Card) {
        card.put("/music/Artist/Album/01 - Title.mp3", &[1; 1000]);
        card.put("/music/Artist/x.mp3", &[2; 1185]);
        card.put("/music/Mine/song.MP3", &[3; 500]);
        card.put("/music/Mine/notes.txt", b"not audio");
        card.put("/music/.hidden/x.mp3", &[4; 10]);
        card.put("/.player/device.txt", &fixture("device.txt"));
    }

    #[test]
    fn a_card_with_no_transfer_data_has_all_its_music_unlisted() {
        let mut card = Card::default();
        card.put("/Music/Artist/a.mp3", &[0; 100]);
        card.put("/Music/Artist/b.flac", &[0; 50]);
        card.put("/Music/Artist/c.opus", &[0; 25]);
        let seen = inspect(&card).unwrap();
        assert!(seen.root.is_none() && seen.pending.is_none() && seen.newer.is_empty());
        assert!(!seen.device.present, "no device.txt: the no-file values");
        assert_eq!(seen.state, Private::default());
        // The no-file extensions are mp3 and flac: Opus isn't the walk's
        // audio until the player says it reads it.
        let m = seen.music;
        assert_eq!((m.present, m.folders, m.files, m.audio, m.audio_bytes), (true, 1, 3, 2, 150));
        assert_eq!((m.unlisted, m.unlisted_bytes), (2, 150));
    }

    #[test]
    fn a_filled_card_reads_as_its_root_names_it() {
        let mut card = Card::default();
        card.golden("manifest.bin", "manifest.bin");
        card.golden("tags-0000002a.bin", "tags-0000002a.bin");
        card.golden("autodj-00000029.bin", "autodj-00000029.bin");
        music(&mut card);
        let seen = inspect(&card).unwrap();
        let root = seen.root.as_ref().unwrap();
        assert_eq!((root.file, root.twice), ("manifest.bin", false));
        assert_eq!(root.manifest.header.generation, 42);
        assert_eq!(root.manifest.header.card_id, 0x5EED_C0DE_0A1B_2C3D);
        assert_eq!(root.manifest.roots, ["Lib A", "Lib B"]);
        let tags = root.tags.as_ref().unwrap();
        assert_eq!(tags.records.len(), 15);
        assert!(tags.has_ledger);
        assert_eq!(root.autodj, Some(Ok(())));
        assert!(seen.device.present && seen.device.writes_mpdj());
        // Two of the three audio files are the ledger's; the listener's song
        // is the third (an extension in any case), and the dot folder isn't
        // walked.
        let m = seen.music;
        assert_eq!((m.files, m.audio, m.unlisted, m.unlisted_bytes), (4, 3, 1, 500));

        // A tags file that isn't the one the root names is absent; the root
        // stays.
        card.golden("tags-00000007.bin", "tags-0000002a.bin");
        let seen = inspect(&card).unwrap();
        assert_eq!(seen.root.as_ref().unwrap().tags.as_ref().err(), Some(&Why::Missing));
        assert_eq!(seen.music.unlisted, 3, "with no ledger, all the audio is the listener's");

        // So is a missing Auto DJ index.
        card.files.remove("/.mstream/autodj-00000029.bin");
        assert_eq!(inspect(&card).unwrap().root.unwrap().autodj, Some(Err(Why::Missing)));
    }

    #[test]
    fn the_newer_root_wins_and_one_commit_seen_twice_is_said() {
        let mut card = Card::default();
        card.golden("manifest-min.bin", "manifest.bin");
        card.golden("tags-00000001.bin", "tags-00000001.bin");
        card.golden("manifest.bin", "manifest.tmp");
        let root = inspect(&card).unwrap().root.unwrap();
        assert_eq!((root.file, root.manifest.header.generation, root.twice), ("manifest.tmp", 42, false));
        assert_eq!(root.tags.err(), Some(Why::Missing), "tags-0000002a.bin isn't on this card");

        card.golden("manifest-min.bin", "manifest.tmp");
        let root = inspect(&card).unwrap().root.unwrap();
        assert_eq!((root.file, root.twice), ("manifest.bin", true));
        assert_eq!(root.tags.unwrap().records.len(), 0, "the empty first commit");

        // A broken root is absent.
        let mut bytes = card.files["/.mstream/manifest.tmp"].0.clone();
        bytes[60] ^= 1;
        card.put("/.mstream/manifest.tmp", &bytes);
        let root = inspect(&card).unwrap().root.unwrap();
        assert_eq!((root.file, root.twice), ("manifest.bin", false));
    }

    #[test]
    fn a_newer_major_or_private_version_stops_the_writer_and_a_plan_is_seen() {
        let mut card = Card::default();
        card.golden("manifest-min.bin", "manifest.bin");
        card.golden("tags-00000001.bin", "tags-00000001.bin");
        card.golden("pending.bin", "pending.bin");
        let mut newer = fixture("tags-00000007.bin");
        newer[4] = 2;
        card.put("/.mstream/tags-00000099.bin", &newer);
        card.put("/.mstream/notes-tags.bin", &newer);
        let seen = inspect(&card).unwrap();
        assert_eq!(seen.newer, ["tags-00000099.bin"], "a private name isn't 2.4.5's to read");
        assert!(seen.pending.is_some(), "a cut run's plan");
        assert!(seen.root.is_some(), "the root still reads: only the writer stops");

        card.put("/.mstream/state.json", br#"{"format":"mstream-card-state","version":2,"seq":3}"#);
        assert_eq!(inspect(&card).unwrap().newer, ["tags-00000099.bin", "state.json"]);
    }

    #[test]
    fn private_files_settle_by_their_read_rule() {
        let state = |seq: u64, label: &str| {
            let card_id = "0x5EEDC0DE0A1B2C3D";
            let text = format!(
                r#"{{"format":"mstream-card-state","version":1,"seq":{seq},"label":"{label}","cardId":"{card_id}"}}"#
            );
            text.into_bytes()
        };
        let pick = |json: Option<&[u8]>, new: Option<&[u8]>| {
            let p = settle_private(json, new, "state", STATE_VERSION);
            let label = p.value.as_ref().and_then(|v| v["label"].as_str().map(String::from));
            (p.file, p.seq, label, p.twice)
        };
        let (a, b) = (state(4, "Car"), state(5, "Car, renamed"));
        let (json, new) = (Some("state.json".to_string()), Some("state.new".to_string()));
        let (car, renamed) = (Some("Car".to_string()), Some("Car, renamed".to_string()));
        assert_eq!(pick(Some(&a), None), (json.clone(), 4, car.clone(), false));
        assert_eq!(pick(None, Some(&b)), (new.clone(), 5, renamed.clone(), false));
        assert_eq!(pick(Some(&a), Some(&b)), (new, 5, renamed.clone(), false));
        assert_eq!(pick(Some(&b), Some(&a)), (json.clone(), 5, renamed, false));
        assert_eq!(pick(Some(&a), Some(&a)), (json.clone(), 4, car.clone(), true));
        let torn = &b[..b.len() - 3];
        assert_eq!(pick(Some(&a), Some(torn)), (json, 4, car, false));
        // Invalid: a byte-order mark, another format, a seq of 0, a version
        // below 1, no object.
        let bom = [b"\xEF\xBB\xBF".as_slice(), &a].concat();
        let resume = br#"{"format":"mstream-card-resume","version":1,"seq":1}"#;
        let zero = br#"{"format":"mstream-card-state","version":1,"seq":0}"#;
        let v0 = br#"{"format":"mstream-card-state","version":0,"seq":1}"#;
        for bad in [bom.as_slice(), resume, zero, v0, b"[1]", b"{"] {
            assert_eq!(pick(Some(bad), None), (None, 0, None, false), "{}", String::from_utf8_lossy(bad));
        }
        // A member named twice, at any depth, is invalid too: serde_json
        // alone would keep the last.
        let nested = br#"{"format":"mstream-card-state","version":1,"seq":2,"x":{"a":1,"a":2}}"#;
        assert_eq!(pick(Some(nested), None), (None, 0, None, false));
        // A newer version stops the software from either file, even the one
        // a lower seq would leave aside.
        let v2 = br#"{"format":"mstream-card-state","version":2,"seq":1}"#;
        let p = settle_private(Some(&b), Some(v2), "state", STATE_VERSION);
        assert_eq!((p.file.as_deref(), p.version, p.value), (Some("state.new"), 2, None));
    }

    /// vectors.json's privateFiles.settle (2.12.7), as a look that only reads
    /// takes it: which file counts, its seq, one write seen twice, a version
    /// that stops the software. The deletes, the rename and the next seq are
    /// a run's (the engine's, not built yet).
    #[test]
    fn private_files_settle_as_the_vectors_say() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/vectors.json");
        let v: Value = serde_json::from_slice(&std::fs::read(path).expect("vectors.json")).expect("JSON");
        let cases = v["privateFiles"]["settle"].as_array().expect("settle");
        assert_eq!(cases.len(), 20);
        for c in cases {
            let (stem, note) = (c["stem"].as_str().expect("a stem"), c["note"].as_str().expect("a note"));
            let file = |key: &str| c[key].as_str().map(str::as_bytes);
            let p = settle_private(file("json"), file("new"), stem, STATE_VERSION);
            if c["stops"].as_bool().expect("stops") {
                assert!(p.file.is_some() && p.version != STATE_VERSION && p.value.is_none(), "{note}");
                continue;
            }
            assert_eq!(p.file.as_deref().unwrap_or("none"), c["pick"].as_str().expect("a pick"), "{note}");
            assert_eq!(p.seq, c["seq"].as_u64().unwrap_or(0), "{note}");
            assert_eq!(p.twice, c["diskCheck"].as_bool().expect("diskCheck"), "{note}");
        }
    }

    #[test]
    fn a_card_path_is_names_under_the_volume_and_nothing_else() {
        let root = std::env::temp_dir().join("mstream-card-root");
        let card = Mounted { root: root.clone(), clock: Clock::Offset(0) };
        let path = card.host("/music/A B/01 - Title.mp3").unwrap();
        assert_eq!(path, root.join("music").join("A B").join("01 - Title.mp3"));
        assert_eq!(card.host("/").unwrap(), root);
        assert_eq!(card.host("/music/...x").unwrap(), root.join("music").join("...x"));
        // A name a card can't hold: Windows would push a drive, a share or a
        // device in place of the root, or climb out by a backslash.
        let escapes = [
            "/music/../escape",
            "/./music",
            "/C:/Windows/x",
            "/C:",
            r"/\\host/share/x",
            r"/\\?\C:\x",
            r"/\\.\PhysicalDrive0",
            r"/music/a\..\..\b",
            "/music/a.mp3:stream",
            "/music/a\0b",
        ];
        for path in escapes {
            assert!(card.host(path).is_err(), "{path:?}");
        }
        // What Windows opens as another name or as a device; elsewhere a
        // name like any other.
        let devices = ["CON", "aux.mp3", "Com1.flac", "lpt¹", "nul .txt"];
        for name in devices.into_iter().chain(["x.", "x ", "dir./x.mp3"]) {
            let path = format!("/music/{name}");
            assert_eq!(card.host(&path).is_err(), cfg!(windows), "{path:?}");
        }
        for path in ["/music/CONCERT.mp3", "/music/COM10.mp3", "/music/Auxiliary/x.mp3", "/music/.x"] {
            assert!(card.host(path).is_ok(), "{path:?}");
        }
    }

    #[test]
    fn a_mounted_card_lists_reads_and_reads_back_its_stamps() {
        let dir = std::env::temp_dir().join(format!("mstream-card-volume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("music/Artist")).unwrap();
        let file = dir.join("music/Artist/01 - Title.mp3");
        std::fs::write(&file, (0..=255u8).cycle().take(10_000).collect::<Vec<_>>()).unwrap();
        let at = UNIX_EPOCH + std::time::Duration::from_secs(1_791_383_442);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(at).unwrap();

        let refused = Found::new(Volume { read_only: true, ..card_volume(&dir) }, card_disk());
        assert!(Mounted::of(&refused).is_none(), "a refused volume is never opened");
        let found = Found::new(card_volume(&dir), card_disk());
        let card = Mounted::of(&found).unwrap();

        let names: Vec<String> = card.list("/").unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["music"]);
        let item = card.stat("/music/Artist/01 - Title.mp3").unwrap().unwrap();
        assert_eq!((item.name.as_str(), item.dir, item.size), ("01 - Title.mp3", false, 10_000));
        assert_eq!(item.fat_time, 0x5D47_73D5, "a UTC clock gives the stamp back unchanged");
        assert_eq!(card.list("/music/Artist").unwrap(), [item]);
        assert!(card.stat("/music/none.mp3").unwrap().is_none());
        assert!(card.read("/music/none.mp3", 10).unwrap().is_none());
        assert!(card.read("/music/Artist/01 - Title.mp3", 9_999).unwrap().is_none(), "past max: absent");
        assert_eq!(card.read("/music/Artist/01 - Title.mp3", 10_000).unwrap().unwrap().len(), 10_000);
        assert_eq!(card.read_at("/music/Artist/01 - Title.mp3", 9_998, 4096).unwrap(), [14, 15]);
        assert!(card.read("/music/../escape", 10).is_err(), "no card path leaves the volume");
        let seen = inspect(&card).unwrap();
        assert!(seen.root.is_none());
        assert_eq!((seen.music.audio, seen.music.unlisted), (1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
