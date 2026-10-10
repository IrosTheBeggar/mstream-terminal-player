//! One board on the desk, as the worker knows it: who it is (its port, its
//! USB bridge and serial), what it said without a reset (the running
//! firmware's `@status`, or its console's `L`, or nothing), what its
//! bootloader said when it was read (only after a reset someone asked
//! for), and from those the two answers the MP3 Player tab gives — the
//! firmware's verdict against the target, and the SD card's state — with
//! what the worker is doing with it now and how its last write went.
//!
//! The worker sends the whole board each time any of it changes
//! (desk::Event::Board), so a page draws from its last copy and never
//! from half an update. The verdict is worked out here, once, for the page
//! and the command line alike: the tab's chip and `device list`'s words can
//! never disagree.

use std::time::Duration;

use super::DeviceError;
use super::engine::DeviceInfo;
use super::firmware::{AppDesc, Place, Target, place};
use super::flow::Phase;
use super::ports::Candidate;

/// The running firmware's answer to `@status` (the firmware repo's
/// docs/HOST-STATUS.md): one line, nothing in it read from the card at the
/// time of asking. A field the line leaves out, or one this player cannot
/// read, is unknown here — never a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Status {
    /// The version as the app description holds it (`L` and the boot line
    /// print the same).
    pub fw: String,
    /// The ELF's first eight hex digits, as About shows them.
    pub elf: Option<String>,
    pub card: CardKind,
    /// The card's size in bytes (its CSD's sectors), when one answered.
    pub size: Option<u64>,
    pub free: Free,
    pub tracks: Tracks,
    /// The indexed tracks' bytes: `?` in every firmware so far.
    pub music: Option<u64>,
    /// The battery, 0–100.
    pub bat: Option<u8>,
    pub state: PlayState,
    /// The paired headphones' name, decoded; none with nothing paired.
    pub bt: Option<String>,
}

/// What is in the card slot, in `card=`'s words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CardKind {
    /// No card answered.
    None,
    Fat32,
    /// FAT16 (FAT12 too: a card of a few MB) — the player reads it.
    Fat16,
    ExFat,
    Ntfs,
    /// A GPT partition table: the player reads MBR cards only.
    Gpt,
    /// A card answered and nothing on it was recognised (blank, Linux's, a
    /// FAT that did not mount).
    Other,
    /// A card answered and its first sector could not be read.
    Unreadable,
    /// A word this player does not know yet (a later firmware's), or none.
    Unknown(String),
}

impl CardKind {
    pub fn parse(word: &str) -> CardKind {
        match word {
            "none" => CardKind::None,
            "fat32" => CardKind::Fat32,
            "fat16" => CardKind::Fat16,
            "exfat" => CardKind::ExFat,
            "ntfs" => CardKind::Ntfs,
            "gpt" => CardKind::Gpt,
            "other" => CardKind::Other,
            "unreadable" => CardKind::Unreadable,
            other => CardKind::Unknown(other.to_string()),
        }
    }

    /// The player plays from it.
    pub fn readable(&self) -> bool {
        matches!(self, CardKind::Fat32 | CardKind::Fat16)
    }
}

/// The card's free space as the firmware knows it: a count (FSINFO's when
/// it is valid, or one `@count` made), not counted, or being counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Free {
    Bytes(u64),
    /// `free=?`: FSINFO's count is unset or broken — the page offers a
    /// count, never a guess.
    NotCounted,
    /// A count under way: the last percent heard, when this player asked
    /// for it (`free=counting` alone carries none).
    Counting { pct: Option<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tracks {
    Count(u64),
    /// The library index is being built.
    Building,
    /// `-`: no card.
    NoCard,
    Unknown,
}

/// What the player on the board is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlayState {
    Playing,
    Paused,
    Idle,
    /// A computer drives the dancer (the USB visualizer's session).
    Host,
    Unknown(String),
}

impl PlayState {
    pub fn parse(word: &str) -> PlayState {
        match word {
            "playing" => PlayState::Playing,
            "paused" => PlayState::Paused,
            "idle" => PlayState::Idle,
            "host" => PlayState::Host,
            other => PlayState::Unknown(other.to_string()),
        }
    }
}

/// What the board said with no reset: the music played on while it was
/// asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// Not asked yet, or the first asking is under way.
    Nothing,
    /// `@status` answered: our firmware, new enough to report its card.
    Status(Status),
    /// `@err 7 status` — only our firmware answers that way — then the
    /// console's `L` for the version (or the boot line, when the board was
    /// just coming up). Too old to report the card. The version is none
    /// when `L` said nothing in time.
    Old { version: Option<String>, elf: Option<String> },
    /// Neither `@status` nor anything else in time: other firmware, none
    /// running, or a hung board. Only a reset can say more.
    Silent,
    /// Another program holds the port: a serial monitor, the IDE, another
    /// mStream Player. That board's state, not the page's failure.
    InUse { detail: String },
    /// The port would not open for another reason (permission, gone).
    Failed(DeviceError),
}

/// What the board's bootloader said, after a reset somebody asked for
/// (Read the board, or a write's own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Probe {
    pub info: DeviceInfo,
    /// The app description in its flash: ours or someone else's, or none.
    pub on_board: Option<AppDesc>,
}

/// The firmware's answer, as one word the page and the command line draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Not heard yet: being asked (the scan widget, "asking the player…").
    Asking,
    /// Our firmware at exactly the target's version. Nothing to do.
    UpToDate,
    /// Our firmware, a release older than the target: Update ▸.
    Update,
    /// Our firmware, a development build based below the target: Update ▸,
    /// under its own name.
    DevUpdate,
    /// Our firmware, a release newer than the target or a development
    /// build on it or past it: the player is the one behind, and installing
    /// the target would go back. No primary.
    Newer,
    /// Our firmware, at a version the order cannot place (or none it would
    /// say): no primary, as Newer.
    Unplaced,
    /// Someone else's firmware, by its app description's project name (a
    /// read): Install ▸, erasing first.
    Other { name: String },
    /// Nothing installed (a read found no app description): Install ▸,
    /// erasing first.
    Blank,
    /// Said nothing to the listen: Read the board ▸, which resets it.
    Silent,
    /// The read found something that is not a Core2 — not an ESP32, or not
    /// 16 MB of flash. Nothing is offered.
    NotCore2 { found: String },
    /// Another program holds the port.
    InUse,
    /// The port could not be opened, or the board could not be read.
    Unreadable,
    /// A write stopped part way: the board cannot start until it is
    /// written again. Try again ▸.
    HalfWritten,
}

/// The card's primary, as the verdict decides it.
// The MP3 Player tab's page draws from this; until it moves onto the desk,
// only the tests and the command line do.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Primary {
    Update,
    Install,
    Read,
    TryAgain,
}

/// What the worker is doing with the board now. One thing at a time per
/// board; a write and a read (the two that reset it) one at a time on the
/// whole desk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Work {
    Idle,
    /// Asking it over USB with no reset (`@status`, then `L`).
    Listening,
    /// Its bootloader being read (a reset, then the board restarted as it
    /// was).
    Reading,
    /// `@count` running on it (its percent is in [`Count`]).
    Counting,
    /// `@identify` sent: "This one" on its screen for five seconds.
    Identifying,
    /// A write asked for, waiting its turn: the firmware still coming, or
    /// another board in its bootloader, or a listen letting the port go.
    Queued,
    /// The write, from reaching the bootloader to the restart's boot line:
    /// the phase, and the percent while it writes.
    Writing { phase: Phase, pct: Option<u8> },
}

/// Why a count did not run, or stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CountWhy {
    /// Music plays (or waits for the headphones): pause it first.
    Playing,
    /// The library update holds the card.
    Library,
    /// No FAT card mounted, or the count cannot run on it.
    Card,
    /// A count is under way already.
    Counting,
    /// The firmware has no `@count`.
    Old,
    /// Nothing came back in time.
    NoAnswer,
    /// The page let the board go first; the count goes on on the board.
    Abandoned,
    /// The port: in use, gone.
    Port(DeviceError),
    Other(String),
}

impl CountWhy {
    pub fn from_word(word: &str) -> CountWhy {
        match word {
            "playing" => CountWhy::Playing,
            "library" => CountWhy::Library,
            "card" => CountWhy::Card,
            "counting" => CountWhy::Counting,
            other => CountWhy::Other(other.to_string()),
        }
    }
}

/// The free-space count this player asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Count {
    Idle,
    Running { pct: u8 },
    Done { free: u64 },
    /// Refused at once, or stopped `part_way` (nothing changed either way).
    Refused { why: CountWhy, part_way: bool },
}

/// How the last write went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Written {
    /// Written and checked, the board restarted (its first line, when it
    /// said one in time). `install` over other firmware or a blank board.
    Done { version: String, took: Duration, skipped: bool, install: bool, boot: Option<String> },
    /// It failed. `half`: the erase or the write had begun, so the board
    /// cannot start until it is written again (its ROM bootloader always
    /// answers); else nothing was touched. `pct` where the write stopped.
    Failed { error: DeviceError, half: bool, pct: Option<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Board {
    pub candidate: Candidate,
    pub heard: Heard,
    /// The bootloader's word: a probe, or why the read failed (the wrong
    /// chip or flash, among others).
    pub probe: Option<Result<Probe, DeviceError>>,
    /// The flash chip's size, from `L` ("16 MB chip") or the probe.
    pub flash_mb: Option<u32>,
    pub verdict: Verdict,
    pub work: Work,
    pub count: Count,
    pub written: Option<Written>,
}

// The MP3 Player tab's page draws from this; until it moves onto the desk,
// only the tests and the command line do.
#[cfg_attr(not(test), allow(dead_code))]
impl Board {
    pub fn new(candidate: Candidate) -> Board {
        Board {
            candidate,
            heard: Heard::Nothing,
            probe: None,
            flash_mb: None,
            verdict: Verdict::Asking,
            work: Work::Idle,
            count: Count::Idle,
            written: None,
        }
    }

    pub fn port(&self) -> &str {
        &self.candidate.port
    }

    /// The bridge's USB serial, fixed per board: what tells two of them
    /// apart when they move between sockets.
    pub fn serial(&self) -> Option<&str> {
        self.candidate.usb.serial_number.as_deref().filter(|s| !s.is_empty())
    }

    pub fn status(&self) -> Option<&Status> {
        match &self.heard {
            Heard::Status(status) => Some(status),
            _ => None,
        }
    }

    /// The version on the board: what the firmware said, else what its
    /// bootloader read (when that was ours).
    pub fn version(&self) -> Option<&str> {
        match &self.heard {
            Heard::Status(status) => Some(&status.fw),
            Heard::Old { version: Some(version), .. } => Some(version),
            _ => self.probed_ours().map(|desc| desc.version.as_str()),
        }
    }

    pub fn elf(&self) -> Option<&str> {
        match &self.heard {
            Heard::Status(status) => status.elf.as_deref(),
            Heard::Old { elf, .. } => elf.as_deref(),
            _ => self.probed_ours().map(|desc| desc.elf8.as_str()),
        }
    }

    fn probed_ours(&self) -> Option<&AppDesc> {
        match &self.probe {
            Some(Ok(Probe { on_board: Some(desc), .. })) if desc.is_ours() => Some(desc),
            _ => None,
        }
    }

    /// It answers `@status`: the card can be shown, counted, and the board
    /// asked to show itself.
    pub fn answers_status(&self) -> bool {
        self.status().is_some()
    }

    /// It runs our firmware, by its own word or its bootloader's: "your MP3
    /// player"; anything else is "this Core2", or an unknown board.
    pub fn ours(&self) -> bool {
        matches!(self.heard, Heard::Status(_) | Heard::Old { .. }) || self.probed_ours().is_some()
    }

    /// The verdict from what is known, against `target` (none until the
    /// target is: a `--firmware` file not read yet).
    pub fn judge(&self, target: Option<&Target>) -> Verdict {
        if let Some(Written::Failed { half: true, .. }) = self.written {
            return Verdict::HalfWritten;
        }
        let by_version = |version: &str| match target.map(|t| place(version, &t.version)) {
            Some(Place::Same) => Verdict::UpToDate,
            Some(Place::Older) if super::firmware::Version::parse(version).is_some_and(|v| !v.dev) => {
                Verdict::Update
            }
            Some(Place::Older) => Verdict::DevUpdate,
            Some(Place::Newer) => Verdict::Newer,
            Some(Place::Unknown) | None => Verdict::Unplaced,
        };
        match &self.heard {
            Heard::Status(status) => by_version(&status.fw),
            Heard::Old { version: Some(version), .. } => by_version(version),
            // Too old for @status, and no version in time: older than any
            // target that answers it; against one that does not, unknown.
            Heard::Old { version: None, .. } => match target {
                Some(t) if t.answers_status => Verdict::Update,
                _ => Verdict::Unplaced,
            },
            Heard::InUse { .. } => Verdict::InUse,
            Heard::Failed(_) => Verdict::Unreadable,
            Heard::Silent | Heard::Nothing => match &self.probe {
                Some(Ok(Probe { on_board: Some(desc), .. })) if desc.is_ours() => by_version(&desc.version),
                Some(Ok(Probe { on_board: Some(desc), .. })) => Verdict::Other { name: desc.project.clone() },
                Some(Ok(Probe { on_board: None, .. })) => Verdict::Blank,
                Some(Err(DeviceError::WrongChip { found } | DeviceError::WrongFlash { found })) => {
                    Verdict::NotCore2 { found: found.clone() }
                }
                Some(Err(_)) => Verdict::Unreadable,
                None if self.heard == Heard::Silent => Verdict::Silent,
                None => Verdict::Asking,
            },
        }
    }

    /// The card's one action, from the verdict — or Try again after a write
    /// that failed, whatever the board said before it.
    pub fn primary(&self) -> Option<Primary> {
        if let Some(Written::Failed { .. }) = self.written {
            return Some(Primary::TryAgain);
        }
        match self.verdict {
            Verdict::Update | Verdict::DevUpdate => Some(Primary::Update),
            Verdict::Other { .. } | Verdict::Blank => Some(Primary::Install),
            Verdict::Silent => Some(Primary::Read),
            _ => None,
        }
    }

    /// What Update all writes: our firmware behind the target, nothing
    /// under way on it, and no failure to look at first.
    pub fn needs_update(&self) -> bool {
        matches!(self.verdict, Verdict::Update | Verdict::DevUpdate)
            && self.work == Work::Idle
            && !matches!(self.written, Some(Written::Failed { .. }))
    }

    /// The SD card row: only what the running firmware just reported,
    /// never a number from before a reset.
    pub fn card(&self) -> Card {
        if matches!(self.work, Work::Writing { .. } | Work::Queued | Work::Reading) {
            return Card::Unknown(CardUnknown::Writing);
        }
        if self.verdict == Verdict::HalfWritten {
            return Card::Unknown(CardUnknown::HalfWritten);
        }
        match &self.heard {
            Heard::Status(status) => {
                let free = match self.count {
                    Count::Running { pct } => Free::Counting { pct: Some(pct) },
                    _ => status.free.clone(),
                };
                match &status.card {
                    CardKind::None => Card::Empty,
                    kind if kind.readable() => Card::Fat {
                        fat16: *kind == CardKind::Fat16,
                        size: status.size,
                        free,
                        tracks: status.tracks.clone(),
                    },
                    kind => Card::Foreign { kind: kind.clone(), size: status.size },
                }
            }
            Heard::Old { .. } => Card::Unknown(CardUnknown::OldFirmware),
            Heard::InUse { .. } => Card::Unknown(CardUnknown::InUse),
            Heard::Failed(_) => Card::Unknown(CardUnknown::Unreadable),
            // Being asked (after a write, the new firmware coming up).
            Heard::Nothing if self.work == Work::Listening => Card::Unknown(CardUnknown::Asking),
            Heard::Silent | Heard::Nothing => match self.verdict {
                Verdict::Asking => Card::Unknown(CardUnknown::Asking),
                _ if self.probed_ours().is_some() => Card::Unknown(CardUnknown::NotRunning),
                _ => Card::Unknown(CardUnknown::NotOurs),
            },
        }
    }
}

/// What the SD card row says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Card {
    /// Nothing to show, and why.
    Unknown(CardUnknown),
    /// No card in the slot.
    Empty,
    /// A card the player reads: its size, its free space (or that it is
    /// not counted, or being counted), its tracks.
    Fat { fat16: bool, size: Option<u64>, free: Free, tracks: Tracks },
    /// A card the player cannot read — exFAT, NTFS, GPT, other, unreadable
    /// — with its size where the firmware reported one.
    Foreign { kind: CardKind, size: Option<u64> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardUnknown {
    /// The first asking is under way.
    Asking,
    /// A write (or a read) under way: nothing can read the card now.
    Writing,
    /// A write stopped part way: unknown until the firmware runs again.
    HalfWritten,
    /// Our firmware, too old to report it (the target may show it).
    OldFirmware,
    /// Only mStream firmware can read the card, and this is not it — or
    /// nothing said what it is.
    NotOurs,
    /// Our firmware is on the board (its bootloader said) but did not
    /// answer: unknown until it runs.
    NotRunning,
    /// Unknown until the port is free.
    InUse,
    Unreadable,
}

// The MP3 Player tab's page draws from this; until it moves onto the desk,
// only the tests and the command line do.
#[cfg_attr(not(test), allow(dead_code))]
impl Card {
    /// Used bytes: the size less the free space, when both are known.
    pub fn used(&self) -> Option<u64> {
        match self {
            Card::Fat { size: Some(size), free: Free::Bytes(free), .. } => Some(size.saturating_sub(*free)),
            _ => None,
        }
    }

    /// 98 % used, or under 1 GB free: the bar and the figure turn gold.
    pub fn nearly_full(&self) -> bool {
        match self {
            Card::Fat { size: Some(size), free: Free::Bytes(free), .. } => {
                *free < 1_000_000_000 || size.saturating_sub(*free) * 100 >= size * 98
            }
            _ => false,
        }
    }
}

/// `59.6` — bytes in decimal gigabytes with one decimal, as the Core2's
/// About shows a card's size, so the page and the board say the same.
pub(crate) fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn candidate(port: &str, serial: &str) -> Candidate {
        Candidate {
            port: port.into(),
            bridge: "CH9102",
            usb: serialport::UsbPortInfo {
                vid: 0x1A86,
                pid: 0x55D4,
                serial_number: Some(serial.into()),
                manufacturer: None,
                product: None,
            },
        }
    }

    pub(crate) fn status(fw: &str) -> Status {
        Status {
            fw: fw.into(),
            elf: Some("63ee7a2b".into()),
            card: CardKind::Fat32,
            size: Some(59_617_918_976),
            free: Free::Bytes(38_214_565_888),
            tracks: Tracks::Count(1284),
            music: None,
            bat: Some(87),
            state: PlayState::Paused,
            bt: None,
        }
    }

    fn pin() -> Target {
        Target { version: "v0.8.0".into(), answers_status: false }
    }

    fn heard(heard: Heard) -> Board {
        let mut board = Board::new(candidate("COM3", "5B1F007751"));
        board.heard = heard;
        board.verdict = board.judge(Some(&pin()));
        board
    }

    fn probed(on_board: Option<AppDesc>) -> Board {
        let mut board = heard(Heard::Silent);
        let info = DeviceInfo {
            port: "COM3".into(),
            bridge: "CH9102".into(),
            chip: "esp32".into(),
            revision: Some((3, 1)),
            flash_mb: Some(16),
            baud: 921_600,
        };
        board.probe = Some(Ok(Probe { info, on_board }));
        board.verdict = board.judge(Some(&pin()));
        board
    }

    fn desc(version: &str, project: &str) -> AppDesc {
        AppDesc { version: version.into(), project: project.into(), idf: "v5.5.5".into(), elf8: "11c35a4a".into() }
    }

    #[test]
    fn the_verdict_follows_the_version_against_the_pin_and_names_every_other_case() {
        assert_eq!(heard(Heard::Status(status("v0.8.0"))).verdict, Verdict::UpToDate);
        assert_eq!(heard(Heard::Status(status("v0.8.0-5-g4e94418"))).verdict, Verdict::Newer, "the real board, past the pin");
        assert_eq!(heard(Heard::Old { version: Some("v0.7.0".into()), elf: None }).verdict, Verdict::Update);
        assert_eq!(heard(Heard::Old { version: Some("v0.6.0-37-g221d99d".into()), elf: None }).verdict, Verdict::DevUpdate);
        assert_eq!(heard(Heard::Old { version: Some("v0.8.0".into()), elf: None }).verdict, Verdict::UpToDate, "v0.8.0 is the pin, @status or not");
        assert_eq!(heard(Heard::Status(status("garbage"))).verdict, Verdict::Unplaced);
        assert_eq!(heard(Heard::Old { version: None, elf: None }).verdict, Verdict::Unplaced, "a pin without @status cannot say older");
        let mut board = heard(Heard::Old { version: None, elf: None });
        let answering = Target { version: "v0.9.0".into(), answers_status: true };
        assert_eq!(board.judge(Some(&answering)), Verdict::Update, "too old for @status is older than a pin that has it");
        assert_eq!(heard(Heard::Silent).verdict, Verdict::Silent);
        assert_eq!(heard(Heard::Nothing).verdict, Verdict::Asking);
        assert_eq!(heard(Heard::InUse { detail: "x".into() }).verdict, Verdict::InUse);
        let gone = DeviceError::Gone { port: "COM3".into() };
        assert_eq!(heard(Heard::Failed(gone)).verdict, Verdict::Unreadable);
        assert_eq!(board.judge(None), Verdict::Unplaced, "no target yet: nothing to measure against");
        board.written = Some(Written::Failed { error: DeviceError::Link("x".into()), half: true, pct: Some(38) });
        assert_eq!(board.judge(Some(&pin())), Verdict::HalfWritten, "a failed write outranks what the board said before it");
    }

    #[test]
    fn a_read_names_other_firmware_a_blank_board_and_a_board_that_is_not_a_core2() {
        assert_eq!(probed(Some(desc("3.3.12", "arduino-lib-builder"))).verdict, Verdict::Other { name: "arduino-lib-builder".into() });
        assert_eq!(probed(None).verdict, Verdict::Blank);
        let ours = probed(Some(desc("v0.7.0", AppDesc::OURS)));
        assert_eq!(ours.verdict, Verdict::Update, "ours, not running: its version from the read");
        assert_eq!(ours.version(), Some("v0.7.0"));
        assert_eq!(ours.elf(), Some("11c35a4a"), "the ELF About shows, from the read");
        assert!(ours.ours());
        assert_eq!(ours.card(), Card::Unknown(CardUnknown::NotRunning));
        let mut chip = heard(Heard::Silent);
        chip.probe = Some(Err(DeviceError::WrongFlash { found: "4 MB".into() }));
        assert_eq!(chip.judge(Some(&pin())), Verdict::NotCore2 { found: "4 MB".into() });
        assert!(!chip.ours());
    }

    #[test]
    fn the_primary_follows_the_verdict_and_a_failed_write_asks_to_try_again() {
        let primary = |board: Board| board.primary();
        assert_eq!(primary(heard(Heard::Status(status("v0.8.0")))), None, "current: nothing to do");
        assert_eq!(primary(heard(Heard::Old { version: Some("v0.7.0".into()), elf: None })), Some(Primary::Update));
        assert_eq!(primary(heard(Heard::Status(status("v0.9.0")))), None, "newer: never offered a step back");
        assert_eq!(primary(probed(None)), Some(Primary::Install));
        assert_eq!(primary(heard(Heard::Silent)), Some(Primary::Read));
        assert_eq!(primary(heard(Heard::InUse { detail: String::new() })), None);
        let mut failed = heard(Heard::Status(status("v0.8.0")));
        failed.written = Some(Written::Failed { error: DeviceError::Gone { port: "COM3".into() }, half: false, pct: None });
        assert_eq!(failed.primary(), Some(Primary::TryAgain));
        assert!(!failed.needs_update());
        let mut busy = heard(Heard::Old { version: Some("v0.7.0".into()), elf: None });
        assert!(busy.needs_update());
        busy.work = Work::Listening;
        assert!(!busy.needs_update(), "a board being asked is not taken into Update all");
    }

    #[test]
    fn the_card_shows_only_what_the_running_firmware_reported() {
        let board = heard(Heard::Status(status("v0.8.0")));
        let card = board.card();
        assert!(matches!(card, Card::Fat { fat16: false, size: Some(59_617_918_976), free: Free::Bytes(_), tracks: Tracks::Count(1284) }));
        assert_eq!(card.used(), Some(21_403_353_088));
        assert_eq!(gb(21_403_353_088), "21.4");
        assert_eq!(gb(59_617_918_976), "59.6");
        assert!(!card.nearly_full());
        let mut full = status("v0.8.0");
        full.free = Free::Bytes(700_000_000);
        assert!(heard(Heard::Status(full)).card().nearly_full(), "under 1 GB free");
        let mut empty = status("v0.8.0");
        empty.card = CardKind::None;
        assert_eq!(heard(Heard::Status(empty)).card(), Card::Empty);
        let mut exfat = status("v0.8.0");
        exfat.card = CardKind::ExFat;
        assert_eq!(heard(Heard::Status(exfat)).card(), Card::Foreign { kind: CardKind::ExFat, size: Some(59_617_918_976) });
        let mut counting = heard(Heard::Status(status("v0.8.0")));
        counting.count = Count::Running { pct: 37 };
        assert!(matches!(counting.card(), Card::Fat { free: Free::Counting { pct: Some(37) }, .. }));
        counting.work = Work::Writing { phase: Phase::Writing, pct: Some(62) };
        assert_eq!(counting.card(), Card::Unknown(CardUnknown::Writing), "no stale numbers during a write");
        assert_eq!(heard(Heard::Old { version: Some("v0.7.0".into()), elf: None }).card(), Card::Unknown(CardUnknown::OldFirmware));
        assert_eq!(heard(Heard::Silent).card(), Card::Unknown(CardUnknown::NotOurs));
        assert_eq!(heard(Heard::Nothing).card(), Card::Unknown(CardUnknown::Asking));
        assert_eq!(heard(Heard::InUse { detail: String::new() }).card(), Card::Unknown(CardUnknown::InUse));
    }

    #[test]
    fn a_board_is_known_by_its_port_and_serial() {
        let board = Board::new(candidate("COM5", "5B1F00A2C4"));
        assert_eq!(board.port(), "COM5");
        assert_eq!(board.serial(), Some("5B1F00A2C4"));
        let mut bare = Board::new(Candidate::bare("COM9"));
        assert_eq!(bare.serial(), None);
        bare.candidate.usb.serial_number = Some(String::new());
        assert_eq!(bare.serial(), None, "an empty serial is none");
    }
}
