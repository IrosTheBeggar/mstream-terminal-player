//! Talking to the board. An [`Engine`] lists the boards and opens one two
//! ways: [`Engine::listen`] to the firmware running on it, with no reset
//! (listen.rs holds the conversation), and [`Engine::open`], whose
//! [`Link`] is that board held in its bootloader — probed, erased,
//! written, then restarted into whatever it now holds. The real engine is
//! serialport for the first and espflash as a library for the second: its
//! reset dance, its RAM stub (which is what makes the writes compressed
//! and the MD5 checks possible), its command protocol. The fake answers
//! the same calls — the running firmware's lines included — from a script
//! in `MSTREAM_DEVICE_FAKE`, so the page, the command line and the e2e leg
//! run with no board on the desk.

use std::io::Read;
use std::time::{Duration, Instant};

use espflash::connection::{Connection, Port, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::{FlashSize, Flasher};
use espflash::target::{Chip, ProgressCallbacks};
use serialport::{FlowControl, SerialPort};

use super::DeviceError;
use super::firmware::{AppDesc, Segment};
use super::listen::Wire;
use super::ports::{self, Candidate};

/// The baud the bootloader is reached at (the ROM's own), the one the
/// firmware's `pio run -t upload` writes at, and the steps between when a
/// bridge or a cable cannot hold the fast one.
pub(crate) const SYNC_BAUD: u32 = 115_200;
/// The running firmware's console and host lines: the boot log's baud.
pub(crate) const CONSOLE_BAUD: u32 = 115_200;
/// How long a read of a listening port waits before it says "nothing
/// yet": short, so a conversation looks at its clock and at a stop often.
const LISTEN_READ: Duration = Duration::from_millis(50);
pub(crate) const BAUDS: [u32; 3] = [921_600, 460_800, 115_200];
/// How long to listen for the firmware's first line after the restart.
pub(crate) const BOOT_LISTEN: Duration = Duration::from_secs(6);
/// The firmware's first serial line starts with its name (main.cpp).
const BOOT_LINE_START: &str = "mstream-mp3-player";

/// What the bootloader said about the board, once reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceInfo {
    pub port: String,
    pub bridge: String,
    pub chip: String,
    pub revision: Option<(u32, u32)>,
    pub flash_mb: Option<u32>,
    /// The baud the link was opened at (BAUDS, after the ladder).
    pub baud: u32,
}

impl DeviceInfo {
    /// `COM3 · CH9102 · esp32 rev 3.1 · 16 MB` — for a log line.
    pub fn describe(&self) -> String {
        let mut line = format!("{} · {} · {}", self.port, self.bridge, self.chip);
        if let Some((major, minor)) = self.revision {
            line.push_str(&format!(" rev {major}.{minor}"));
        }
        if let Some(mb) = self.flash_mb {
            line.push_str(&format!(" · {mb} MB"));
        }
        line
    }
}

/// What a write reports as it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Report {
    /// The written share of all the bytes, in whole percent.
    Percent(u8),
    /// A segment's write begins: where, and in how many chunks espflash
    /// will send it (the log's line; the percent counts bytes).
    Chunks { addr: u32, chunks: usize },
    /// Written; the board is computing the checksum.
    Verifying,
}

/// A board held in its bootloader.
pub(crate) trait Link {
    fn info(&self) -> &DeviceInfo;
    /// The app description of what is on the board, if any.
    fn app_desc(&mut self) -> Result<Option<AppDesc>, DeviceError>;
    /// The whole flash, erased.
    fn erase(&mut self) -> Result<(), DeviceError>;
    /// Every segment written and checked. `true` when nothing was written
    /// because the board already held exactly these bytes.
    fn write(&mut self, segments: &[Segment], report: &mut dyn FnMut(Report)) -> Result<bool, DeviceError>;
    /// Restart the board into its firmware, and listen a moment for the
    /// firmware's first line — after a write, whose new firmware says its
    /// name as it comes up.
    fn restart(self: Box<Self>) -> Result<Option<String>, DeviceError>;
    /// Restart the board into its firmware and let the port go at once,
    /// with no listen: the run ends without a write, so there is no new
    /// line to hear, and a port kept for the listen is one the next worker
    /// (the GUI's tab opened again, a retry) would find in use.
    fn let_go(self: Box<Self>);
}

/// Shared between the desk's worker and the jobs it runs beside it, one
/// thread per board it talks to.
pub(crate) trait Engine: Send + Sync {
    fn candidates(&self) -> Result<Vec<Candidate>, DeviceError>;
    /// The serial ports that are NOT a Core2's bridge, by name — what the
    /// page lists when it finds no board.
    fn others(&self) -> Vec<String> {
        Vec::new()
    }
    /// Open `candidate`'s port to the firmware running on the board, with
    /// DTR and RTS held low: the board is not reset, and its music plays
    /// on. The port is the caller's until it drops it.
    fn listen(&self, candidate: &Candidate) -> Result<Box<dyn Wire>, DeviceError>;
    /// Reach the board's bootloader on `candidate`, and raise the link to
    /// `baud` for the writes.
    fn open(&self, candidate: &Candidate, baud: u32) -> Result<Box<dyn Link>, DeviceError>;
    /// Restart the board on `candidate` into its firmware with no link to
    /// it: an `open` that failed to sync had already reset the board into
    /// its bootloader, and a run that stops there (a Quit heard between the
    /// baud ladder's rungs) must not leave it dark.
    fn let_go(&self, candidate: &Candidate);
}

/// The engine to use: the fake when `MSTREAM_DEVICE_FAKE` scripts one
/// (tests, the e2e battery), else the real thing.
pub(crate) fn from_env() -> Box<dyn Engine> {
    match std::env::var("MSTREAM_DEVICE_FAKE") {
        Ok(spec) if !spec.is_empty() => Box::new(fake::Fake::new(&spec)),
        _ => Box::new(Esp),
    }
}

// ── espflash ────────────────────────────────────────────────────────────────

pub(crate) struct Esp;

impl Engine for Esp {
    fn candidates(&self) -> Result<Vec<Candidate>, DeviceError> {
        ports::candidates()
    }

    fn others(&self) -> Vec<String> {
        ports::others()
    }

    /// The Core2 resets through its bridge's two modem lines, crossed into
    /// a transistor pair: the chip's EN goes low when RTS is asserted with
    /// DTR not, and GPIO0 when DTR is asserted with RTS not. Both low — or
    /// both high — and the firmware runs on. So the port is opened with both
    /// held low, and never pulsed.
    ///
    /// - Windows: serialport's open sets the DCB with DTR_CONTROL_DISABLE
    ///   and, with no flow control, RTS_CONTROL_DISABLE, in the one
    ///   SetCommState that opens the port — what pyserial does with `dtr`
    ///   and `rts` false before `open()`, which is how the real Core2 on
    ///   COM3 was asked by hand (2026-10-07, and the firmware's own
    ///   `@status` checks on 10-10): no reset, the music played on. This
    ///   code itself has not met that board yet.
    /// - Linux and macOS: the kernel raises DTR and RTS on open, together
    ///   in one request (cdc-acm and cp210x set both lines at once), which
    ///   the pair reads as "run". The danger is lowering them one at a
    ///   time in the wrong order: DTR first leaves RTS alone asserted — a
    ///   reset. So RTS goes first (GPIO0 is low for a moment, which matters
    ///   only at a reset), then DTR, and serialport is not asked to set DTR
    ///   on its own (`preserve_dtr_on_open`). Closing lowers both at once,
    ///   already low. Not yet tried on a real board on either: the order is
    ///   the whole of it.
    fn listen(&self, candidate: &Candidate) -> Result<Box<dyn Wire>, DeviceError> {
        let mut serial = serialport::new(&candidate.port, CONSOLE_BAUD)
            .flow_control(FlowControl::None)
            .timeout(LISTEN_READ)
            .preserve_dtr_on_open()
            .open_native()
            .map_err(|e| serial_error(&candidate.port, &e))?;
        let lines = |e: serialport::Error| DeviceError::Link(e.to_string());
        serial.write_request_to_send(false).map_err(lines)?;
        serial.write_data_terminal_ready(false).map_err(lines)?;
        Ok(Box::new(serial))
    }

    fn open(&self, candidate: &Candidate, baud: u32) -> Result<Box<dyn Link>, DeviceError> {
        let serial = serialport::new(&candidate.port, SYNC_BAUD)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_secs(3))
            .open_native()
            .map_err(|e| serial_error(&candidate.port, &e))?;
        let connection = Connection::new(
            serial,
            candidate.usb.clone(),
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            SYNC_BAUD,
        );
        // The stub (compressed writes, MD5), verify after each write, skip a
        // segment the flash already holds, and the chip pinned: a CH9102
        // with anything but an ESP32 behind it is not a Core2.
        let mut flasher =
            Flasher::connect(connection, true, true, true, Some(Chip::Esp32), Some(baud))
                .map_err(|e| flash_error(&candidate.port, e))?;
        let detected = flasher.device_info().map_err(|e| flash_error(&candidate.port, e))?;
        let flash_mb = flash_mb(detected.flash_size);
        let info = DeviceInfo {
            port: candidate.port.clone(),
            bridge: candidate.bridge.to_string(),
            chip: detected.chip.to_string(),
            revision: detected.revision,
            flash_mb: Some(flash_mb),
            baud,
        };
        if flash_mb != 16 {
            // Say what it is, then let the board go: a wrong board left in
            // its bootloader looks dead until it is unplugged.
            let _ = flasher.connection().reset();
            return Err(DeviceError::WrongFlash { found: format!("{flash_mb} MB") });
        }
        Ok(Box::new(EspLink { flasher, info }))
    }

    fn let_go(&self, candidate: &Candidate) {
        // The port opened again, the reset line pulsed — espflash's own
        // reset after a flash, which needs no sync — and the port closed.
        let Ok(serial) = serialport::new(&candidate.port, SYNC_BAUD)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_secs(3))
            .open_native()
        else {
            return;
        };
        let mut connection = Connection::new(
            serial,
            candidate.usb.clone(),
            ResetAfterOperation::HardReset,
            ResetBeforeOperation::DefaultReset,
            SYNC_BAUD,
        );
        let _ = connection.reset();
    }
}

/// espflash's size word as megabytes (the Core2 check compares whole MB).
fn flash_mb(size: FlashSize) -> u32 {
    match size {
        FlashSize::_256Kb | FlashSize::_512Kb => 0,
        FlashSize::_1Mb => 1,
        FlashSize::_2Mb => 2,
        FlashSize::_4Mb => 4,
        FlashSize::_8Mb => 8,
        FlashSize::_16Mb => 16,
        FlashSize::_32Mb => 32,
        FlashSize::_64Mb => 64,
        FlashSize::_128Mb => 128,
        FlashSize::_256Mb => 256,
        // The enum is non-exhaustive: a size espflash learns later is not
        // a Core2's either way.
        _ => 0,
    }
}

struct EspLink {
    flasher: Flasher,
    info: DeviceInfo,
}

impl Link for EspLink {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn app_desc(&mut self) -> Result<Option<AppDesc>, DeviceError> {
        // read_flash writes to a FILE (its API's one shape): a scratch file
        // under the temp dir, read back and removed. 256 bytes, one block.
        let scratch = std::env::temp_dir().join(format!("mstream-player-appdesc-{}.bin", std::process::id()));
        let at = super::firmware::APP_OFFSET as u32 + AppDesc::OFFSET_IN_APP as u32;
        let read = self.flasher.read_flash(at, AppDesc::LEN as u32, AppDesc::LEN as u32, 1, scratch.clone());
        let bytes = std::fs::read(&scratch);
        let _ = std::fs::remove_file(&scratch);
        read.map_err(|e| DeviceError::Link(e.to_string()))?;
        let bytes = bytes.map_err(|e| DeviceError::Link(e.to_string()))?;
        Ok(AppDesc::parse(&bytes))
    }

    fn erase(&mut self) -> Result<(), DeviceError> {
        self.flasher.erase_flash().map_err(|e| DeviceError::Link(e.to_string()))
    }

    fn write(&mut self, segments: &[Segment], report: &mut dyn FnMut(Report)) -> Result<bool, DeviceError> {
        let borrowed: Vec<espflash::image_format::Segment<'_>> = segments
            .iter()
            .map(|s| espflash::image_format::Segment { addr: s.offset, data: std::borrow::Cow::Borrowed(&s.data) })
            .collect();
        let mut progress = Percent::new(segments.iter().map(|s| s.data.len()).collect(), report);
        self.flasher
            .write_bins_to_flash(&borrowed, &mut progress)
            .map_err(|e| DeviceError::Link(e.to_string()))?;
        Ok(!progress.written)
    }

    fn restart(self: Box<Self>) -> Result<Option<String>, DeviceError> {
        let mut flasher = self.flasher;
        flasher.connection().reset().map_err(|e| DeviceError::Link(e.to_string()))?;
        // The link is done with the bootloader; the same port, back at the
        // boot console's baud, hears the firmware come up.
        let mut port: Port = flasher.into();
        let _ = port.set_baud_rate(SYNC_BAUD);
        let _ = port.set_timeout(Duration::from_millis(300));
        Ok(boot_line(&mut port, BOOT_LISTEN))
    }

    fn let_go(self: Box<Self>) {
        // The reset, then the flasher — and the port it owns — dropped.
        let mut flasher = self.flasher;
        let _ = flasher.connection().reset();
    }
}

/// espflash's progress as the share of all the bytes. Its callbacks count
/// in CHUNKS of a segment's (compressed) stream — `init` says how many,
/// `update` which one just went — and name no segment, so the segments'
/// byte sizes are kept here in write order and the count of `finish`
/// calls is the index. A segment the board already holds reports only
/// `finish(true)`, never `init` — which is how "nothing was written" is
/// known.
struct Percent<'a> {
    /// Each segment's bytes, in the order they are written.
    sizes: Vec<usize>,
    total: usize,
    /// Segments finished so far (written or skipped).
    index: usize,
    /// The current segment's chunk count, from `init`.
    chunks: usize,
    last: Option<u8>,
    written: bool,
    report: &'a mut dyn FnMut(Report),
}

impl<'a> Percent<'a> {
    fn new(sizes: Vec<usize>, report: &'a mut dyn FnMut(Report)) -> Percent<'a> {
        Percent { total: sizes.iter().sum(), sizes, index: 0, chunks: 0, last: None, written: false, report }
    }
}

impl ProgressCallbacks for Percent<'_> {
    fn init(&mut self, addr: u32, chunks: usize) {
        self.chunks = chunks.max(1);
        self.written = true;
        (self.report)(Report::Chunks { addr, chunks });
    }

    fn update(&mut self, current: usize) {
        let before: usize = self.sizes[..self.index.min(self.sizes.len())].iter().sum();
        let part = self.sizes.get(self.index).copied().unwrap_or(0);
        let done = before + part * current.min(self.chunks) / self.chunks.max(1);
        let pct = ((done * 100) / self.total.max(1)).min(100) as u8;
        if self.last != Some(pct) {
            self.last = Some(pct);
            (self.report)(Report::Percent(pct));
        }
    }

    fn verifying(&mut self) {
        (self.report)(Report::Verifying);
    }

    fn finish(&mut self, _skipped: bool) {
        self.index += 1;
        self.chunks = 0;
    }
}

/// The firmware's first line, if it shows up within `wait`: the ROM's own
/// boot chatter comes first, at the same baud; the line that starts with
/// the firmware's name is the one to keep.
fn boot_line(port: &mut dyn Read, wait: Duration) -> Option<String> {
    let deadline = Instant::now() + wait;
    let mut seen: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    while Instant::now() < deadline {
        match port.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => seen.extend_from_slice(&chunk[..n]),
            // A timeout is just "nothing yet"; anything else ends the wait.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        if let Some(line) = first_firmware_line(&seen) {
            return Some(line);
        }
        if seen.len() > 64 * 1024 {
            break;
        }
    }
    None
}

/// The first complete line in `bytes` that starts with the firmware's name.
pub(crate) fn first_firmware_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(BOOT_LINE_START))
        // Complete: the raw bytes end past it with a newline.
        .filter(|line| {
            let text = String::from_utf8_lossy(bytes);
            text.find(line.as_ref() as &str).is_some_and(|at| text[at + line.len()..].contains('\n'))
        })
        .map(str::to_string)
}

/// A port that would not open, as the page tells it: in use elsewhere,
/// not permitted, or gone — the three things a person can act on.
fn serial_error(port: &str, e: &serialport::Error) -> DeviceError {
    let detail = e.description.clone();
    match e.kind() {
        serialport::ErrorKind::NoDevice => DeviceError::Gone { port: port.to_string() },
        serialport::ErrorKind::Io(kind) => match kind {
            // Windows answers an open port with "access denied"; Linux says
            // busy for that and reserves "permission" for the group.
            std::io::ErrorKind::PermissionDenied if cfg!(windows) => {
                DeviceError::Busy { port: port.to_string(), detail }
            }
            std::io::ErrorKind::PermissionDenied => DeviceError::Permission { port: port.to_string(), detail },
            std::io::ErrorKind::ResourceBusy => DeviceError::Busy { port: port.to_string(), detail },
            std::io::ErrorKind::NotFound => DeviceError::Gone { port: port.to_string() },
            _ => DeviceError::NoSync { port: port.to_string(), detail },
        },
        _ => DeviceError::NoSync { port: port.to_string(), detail },
    }
}

/// A connect that failed, as the page tells it: the wrong chip is its own
/// story; everything else is the board not answering, with espflash's
/// words as the detail (with its `serialport` feature on, an I/O error
/// arrives as a connection error too — the open above already told the
/// port's own stories apart).
fn flash_error(port: &str, e: espflash::Error) -> DeviceError {
    match e {
        espflash::Error::ChipMismatch(_, found) => DeviceError::WrongChip { found },
        other => DeviceError::NoSync { port: port.to_string(), detail: other.to_string() },
    }
}

// ── The fake ────────────────────────────────────────────────────────────────

pub(crate) mod fake {
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::{DeviceInfo, Engine, Link, Report};
    use crate::device::DeviceError;
    use crate::device::firmware::{AppDesc, Segment, Version};
    use crate::device::listen::Wire;
    use crate::device::ports::Candidate;

    /// What a fake board's flash holds.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Flash {
        Blank,
        Ours(String),
        /// The Arduino core's own description: someone else's firmware.
        Other,
    }

    /// How a fake board's running firmware answers the host lines.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Talk {
        /// `@status`, `@count`, `@identify` (0.9.0 and later).
        Status,
        /// `@err 7` to every host line, `L` to the console (v0.6.0–v0.8.0).
        Old,
        /// Nothing: other firmware, none, or ours not running.
        Silent,
    }

    /// Whether our firmware at `version` answers `@status`: 0.9.0's work,
    /// so the next release's dev builds (`v0.9.0-dev+…`) too.
    fn talk_of(version: &str) -> Talk {
        match Version::parse(version) {
            Some(v) if v.core >= (0, 9, 0) => Talk::Status,
            _ => Talk::Old,
        }
    }

    #[derive(Clone, Debug)]
    struct FakeBoard {
        port: String,
        bridge: &'static str,
        vid: u16,
        pid: u16,
        serial: String,
        flash: Flash,
        talk: Talk,
        /// The port is held by another program, always.
        busy: bool,
        /// …or for its first moments.
        held_for: Duration,
        /// The bootloader never answers.
        nosync: bool,
        /// The write dies halfway.
        failwrite: bool,
        /// A board that only shares the USB chip: 4 MB of flash.
        chip: bool,
        /// Plugged in this long after the fake began, and unplugged this
        /// long after (never, when none).
        arrives: Duration,
        leaves: Option<Duration>,
        /// `@status` fields said instead of the defaults.
        fields: Vec<(String, String)>,
        /// `@identify` refused with this word (ui, screen, viz).
        identify: Option<String>,
        /// The free bytes a count found, said by `@status` from then on.
        counted: Option<u64>,
    }

    impl FakeBoard {
        fn new(index: usize, word: &str) -> FakeBoard {
            let port = format!("FAKE{index}");
            let mut parts = word.split('/');
            let head = parts.next().unwrap_or_default().trim();
            let (kind, version) = match head.split_once(':') {
                Some((kind, version)) => (kind, Some(version.to_string())),
                None => (head, None),
            };
            let mut board = FakeBoard {
                serial: port.clone(),
                port,
                bridge: "CH9102",
                vid: 0x1A86,
                pid: 0x55D4,
                flash: Flash::Blank,
                talk: Talk::Silent,
                busy: false,
                held_for: Duration::ZERO,
                nosync: false,
                failwrite: false,
                chip: false,
                arrives: Duration::ZERO,
                leaves: None,
                fields: Vec::new(),
                identify: None,
                counted: None,
            };
            let ours = |default: &str| version.clone().unwrap_or_else(|| default.to_string());
            match kind {
                "ours" => {
                    let v = ours("v0.4.0");
                    board.talk = talk_of(&v);
                    board.flash = Flash::Ours(v);
                }
                "status" => {
                    board.flash = Flash::Ours(ours("v0.9.0"));
                    board.talk = Talk::Status;
                }
                "old" => {
                    board.flash = Flash::Ours(ours("v0.8.0"));
                    board.talk = Talk::Old;
                }
                "silent" => board.flash = Flash::Ours(ours("v0.7.0")),
                "other" => board.flash = Flash::Other,
                "chip" => {
                    board.chip = true;
                    (board.bridge, board.vid, board.pid) = ("CP210x", 0x10C4, 0xEA60);
                    board.serial = format!("CHIP{index}");
                }
                "busy" => board.busy = true,
                "nosync" => board.nosync = true,
                "failwrite" => board.failwrite = true,
                _ => {}
            }
            let secs = |v: &str| Duration::from_secs_f64(v.parse::<f64>().unwrap_or(0.0).max(0.0));
            for option in parts {
                let Some((key, value)) = option.split_once('=') else { continue };
                match key {
                    "in" => board.arrives = secs(value),
                    "out" => board.leaves = Some(secs(value)),
                    "held" => board.held_for = secs(value),
                    "identify" => board.identify = Some(value.to_string()),
                    "talk" => {
                        board.talk = match value {
                            "status" => Talk::Status,
                            "old" => Talk::Old,
                            _ => Talk::Silent,
                        }
                    }
                    "serial" => board.serial = value.to_string(),
                    "fail" => board.failwrite = value == "write",
                    "flash" => {
                        board.flash = match value {
                            "blank" => Flash::Blank,
                            "other" => Flash::Other,
                            version => Flash::Ours(version.to_string()),
                        }
                    }
                    _ => board.fields.push((key.to_string(), value.to_string())),
                }
            }
            board
        }

        fn present(&self, age: Duration) -> bool {
            age >= self.arrives && self.leaves.is_none_or(|out| age < out)
        }

        fn held(&self, age: Duration) -> bool {
            self.busy || age < self.arrives + self.held_for
        }

        fn candidate(&self) -> Candidate {
            Candidate {
                port: self.port.clone(),
                bridge: self.bridge,
                usb: serialport::UsbPortInfo {
                    vid: self.vid,
                    pid: self.pid,
                    serial_number: Some(self.serial.clone()),
                    manufacturer: None,
                    product: Some("fake Core2".to_string()),
                },
            }
        }

        fn version(&self) -> Option<&str> {
            match &self.flash {
                Flash::Ours(version) => Some(version),
                _ => None,
            }
        }

        /// A field of `@status`: what the board was told to say, else the
        /// default, a 64 GB card with music on it, paused.
        fn field(&self, key: &str) -> String {
            if let Some((_, value)) = self.fields.iter().find(|(k, _)| k == key) {
                return value.clone();
            }
            match key {
                "fw" => self.version().unwrap_or("?").to_string(),
                "elf" => "63ee7a2b".to_string(),
                "card" => "fat32".to_string(),
                "size" => "63864569856".to_string(),
                "free" => self.counted.map_or_else(|| "38214565888".to_string(), |n| n.to_string()),
                "tracks" => "1284".to_string(),
                "music" => "?".to_string(),
                "bat" => "87".to_string(),
                "state" => "paused".to_string(),
                "bt" => "-".to_string(),
                _ => String::new(),
            }
        }

        fn status_line(&self) -> String {
            let mut line = "@status".to_string();
            for key in ["fw", "elf", "card", "size", "free", "tracks", "music", "bat", "state", "bt"] {
                line.push_str(&format!(" {key}={}", self.field(key)));
            }
            line
        }
    }

    /// `MSTREAM_DEVICE_FAKE`: one board's word, or several separated by
    /// commas — on ports FAKE0, FAKE1, … in that order — or `nodevice` (no
    /// board), or `two` (two blank ones). A word is a kind, a version after
    /// a colon where it takes one, and options after slashes:
    ///
    /// - `fresh`: nothing installed (and anything unknown reads as this);
    /// - `ours:<v>`: our firmware at v (v0.4.0 when none), answering as that
    ///   version would — `@status` from 0.9.0, else `@err 7` and `L`;
    /// - `status:<v>` (v0.9.0) and `old:<v>` (v0.8.0): the same, the answer
    ///   chosen whatever the version;
    /// - `silent:<v>`: our firmware at v (v0.7.0) on the board, not
    ///   running: says nothing, its bootloader reads it;
    /// - `other`: someone else's firmware, saying nothing;
    /// - `chip`: a board that only shares the USB chip (a CP210x), saying
    ///   nothing, whose bootloader shows 4 MB of flash;
    /// - `busy`: its port held by another program; `nosync`: its bootloader
    ///   never answers; `failwrite`: a blank board whose write dies halfway.
    ///
    /// Options: `/in=<s>` plugged in that long after the fake begins,
    /// `/out=<s>` unplugged then, `/held=<s>` its port held that long,
    /// `/identify=<why>` `@identify` refused (ui, screen, viz),
    /// `/talk=status|old|silent`, `/serial=<s>`, `/fail=write` (its write
    /// dies halfway), `/flash=other|blank|<version>` (what its bootloader
    /// reads, whatever it says), and any of `@status`'s fields
    /// (`/card=exfat`, `/free=?`, `/state=playing`, `/bt=…`, …).
    /// `status:v0.9.0/free=?,old:v0.7.0,chip/in=3` is three boards, the
    /// last plugged in three seconds on.
    ///
    /// Cloned, it shares its boards and its trace: a test keeps one copy
    /// and hands the worker the other.
    #[derive(Clone)]
    pub(crate) struct Fake {
        boards: Arc<Mutex<Vec<FakeBoard>>>,
        born: Instant,
        /// How long the fake's write takes end to end — the real one takes
        /// half a minute; a test wants the bar to move, not to wait. A
        /// count takes as long.
        pub pace: Duration,
        /// How long each `open` takes to answer — espflash's connect, a
        /// second or more on a real board, during which the worker hears
        /// nothing. None by default.
        reach: Duration,
        /// What was done to the boards, in order (`open 921600`, `let go`,
        /// `restart`, `let go FAKE0`, `listen FAKE1`, `identify FAKE1
        /// FAKE1`), for the tests to read.
        trace: Arc<Mutex<Vec<String>>>,
    }

    /// The boards' list, whoever panicked while holding it.
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl Fake {
        pub(crate) fn new(spec: &str) -> Fake {
            let boards = match spec.trim() {
                "nodevice" => Vec::new(),
                "two" => vec![FakeBoard::new(0, "fresh"), FakeBoard::new(1, "fresh")],
                words => words.split(',').enumerate().map(|(i, word)| FakeBoard::new(i, word.trim())).collect(),
            };
            Fake {
                boards: Arc::new(Mutex::new(boards)),
                born: Instant::now(),
                pace: Duration::from_millis(1200),
                reach: Duration::ZERO,
                trace: Arc::default(),
            }
        }

        /// The tests' fake: a write that takes a blink, not a second.
        #[cfg(test)]
        pub(crate) fn with_pace(mut self, pace: Duration) -> Fake {
            self.pace = pace;
            self
        }

        /// A board that takes `reach` to answer each `open`, as a real one
        /// does while espflash syncs with it.
        #[cfg(test)]
        pub(crate) fn with_reach(mut self, reach: Duration) -> Fake {
            self.reach = reach;
            self
        }

        /// What was done to the boards so far, shared with the fake.
        #[cfg(test)]
        pub(crate) fn trace(&self) -> Arc<Mutex<Vec<String>>> {
            self.trace.clone()
        }

        /// The version on a fake board's flash now, for the tests to see
        /// what a write left there.
        #[cfg(test)]
        pub(crate) fn version_on(&self, port: &str) -> Option<String> {
            lock(&self.boards).iter().find(|b| b.port == port).and_then(|b| b.version().map(str::to_string))
        }

        fn note(trace: &Mutex<Vec<String>>, what: String) {
            lock(trace).push(what);
        }

        fn age(&self) -> Duration {
            self.born.elapsed()
        }

        /// The board on `port`, as it is now: one the script names, or a
        /// blank one for a port named by hand.
        fn board(&self, port: &str) -> (Option<usize>, FakeBoard) {
            let boards = lock(&self.boards);
            match boards.iter().position(|b| b.port.eq_ignore_ascii_case(port)) {
                Some(i) => (Some(i), boards[i].clone()),
                None => {
                    let mut stray = FakeBoard::new(0, "fresh");
                    stray.port = port.to_string();
                    (None, stray)
                }
            }
        }
    }

    impl Engine for Fake {
        fn candidates(&self) -> Result<Vec<Candidate>, DeviceError> {
            let age = self.age();
            Ok(lock(&self.boards).iter().filter(|b| b.present(age)).map(FakeBoard::candidate).collect())
        }

        /// A desk with no Core2 still has a port on it — the page's
        /// "ports seen" line has something to say in the e2e leg.
        fn others(&self) -> Vec<String> {
            let age = self.age();
            if lock(&self.boards).iter().any(|b| b.present(age)) {
                Vec::new()
            } else {
                vec!["FAKECOM1".to_string()]
            }
        }

        fn listen(&self, candidate: &Candidate) -> Result<Box<dyn Wire>, DeviceError> {
            let (index, board) = self.board(&candidate.port);
            let age = self.age();
            if index.is_some() && !board.present(age) {
                return Err(DeviceError::Gone { port: candidate.port.clone() });
            }
            if board.held(age) {
                let detail = "held by the fake".into();
                return Err(DeviceError::Busy { port: candidate.port.clone(), detail });
            }
            Fake::note(&self.trace, format!("listen {}", candidate.port));
            Ok(Box::new(FakeWire {
                fake: self.clone(),
                index,
                line: Vec::new(),
                out: VecDeque::new(),
            }))
        }

        fn open(&self, candidate: &Candidate, baud: u32) -> Result<Box<dyn Link>, DeviceError> {
            let (index, board) = self.board(&candidate.port);
            if board.held(self.age()) {
                let detail = "held by the fake".into();
                return Err(DeviceError::Busy { port: candidate.port.clone(), detail });
            }
            // The port opened: the board is reset into its bootloader,
            // whether or not it then answers.
            Fake::note(&self.trace, format!("open {baud}"));
            std::thread::sleep(self.reach);
            if board.nosync {
                let detail = "no answer".into();
                return Err(DeviceError::NoSync { port: candidate.port.clone(), detail });
            }
            if board.chip {
                // The real engine says so once the flash is read, and
                // resets the board on its way out.
                return Err(DeviceError::WrongFlash { found: "4 MB".into() });
            }
            Ok(Box::new(FakeLink {
                fake: self.clone(),
                index,
                info: DeviceInfo {
                    port: candidate.port.clone(),
                    bridge: candidate.bridge.to_string(),
                    chip: "esp32".to_string(),
                    revision: Some((3, 1)),
                    flash_mb: Some(16),
                    baud,
                },
                written: None,
            }))
        }

        fn let_go(&self, candidate: &Candidate) {
            Fake::note(&self.trace, format!("let go {}", candidate.port));
        }
    }

    struct FakeLink {
        fake: Fake,
        index: Option<usize>,
        info: DeviceInfo,
        /// The version of the image written, for the boot line.
        written: Option<String>,
    }

    impl FakeLink {
        fn board(&self) -> FakeBoard {
            self.fake.board(&self.info.port).1
        }

        fn change(&self, change: impl FnOnce(&mut FakeBoard)) {
            if let Some(i) = self.index {
                change(&mut lock(&self.fake.boards)[i]);
            }
        }
    }

    impl Link for FakeLink {
        fn info(&self) -> &DeviceInfo {
            &self.info
        }

        fn app_desc(&mut self) -> Result<Option<AppDesc>, DeviceError> {
            Ok(match self.board().flash {
                Flash::Ours(version) => Some(AppDesc {
                    version,
                    project: AppDesc::OURS.to_string(),
                    idf: "v5.5.5".to_string(),
                    elf8: "fa4e0000".to_string(),
                }),
                Flash::Other => Some(AppDesc {
                    version: "3.3.12".to_string(),
                    project: "arduino-lib-builder".to_string(),
                    idf: "v5.5.5".to_string(),
                    elf8: "00000000".to_string(),
                }),
                Flash::Blank => None,
            })
        }

        fn erase(&mut self) -> Result<(), DeviceError> {
            std::thread::sleep(self.fake.pace / 4);
            self.change(|b| {
                b.flash = Flash::Blank;
                b.talk = Talk::Silent;
            });
            Ok(())
        }

        fn write(&mut self, segments: &[Segment], report: &mut dyn FnMut(Report)) -> Result<bool, DeviceError> {
            let steps = 10u8;
            let failing = self.board().failwrite;
            for step in 0..=steps {
                std::thread::sleep(self.fake.pace / u32::from(steps));
                let pct = step * 10;
                let unplugged = self.index.is_some() && !self.board().present(self.fake.age());
                if unplugged {
                    self.change(|b| {
                        b.flash = Flash::Blank;
                        b.talk = Talk::Silent;
                    });
                    return Err(DeviceError::Gone { port: self.info.port.clone() });
                }
                if failing && pct == 50 {
                    // Half an image boots nothing.
                    self.change(|b| {
                        b.flash = Flash::Blank;
                        b.talk = Talk::Silent;
                    });
                    return Err(DeviceError::Link("the fake's write died halfway".into()));
                }
                report(Report::Percent(pct));
            }
            report(Report::Verifying);
            // What was written names itself the way the real board will.
            self.written = segments.iter().find_map(|s| {
                let app = match s.offset {
                    0 => s.data.get(crate::device::firmware::APP_OFFSET..)?,
                    _ => &s.data[..],
                };
                AppDesc::in_app(app).map(|d| d.version)
            });
            Ok(false)
        }

        fn restart(self: Box<Self>) -> Result<Option<String>, DeviceError> {
            Fake::note(&self.fake.trace, "restart".to_string());
            if let Some(version) = &self.written {
                self.change(|b| {
                    b.flash = Flash::Ours(version.clone());
                    b.talk = talk_of(version);
                    b.fields.retain(|(k, _)| k != "fw");
                });
            }
            Ok(self
                .written
                .map(|v| format!("mstream-mp3-player {v} (commit fake000, 2026-10-01), ELF fa4e0000")))
        }

        fn let_go(self: Box<Self>) {
            Fake::note(&self.fake.trace, "let go".to_string());
        }
    }

    /// A fake board's running firmware over its port: what the host writes
    /// is read as host lines and console keys, and the answers come back
    /// in small pieces, among `[stats]` lines, as a real port delivers
    /// them.
    struct FakeWire {
        fake: Fake,
        index: Option<usize>,
        /// A host line under way, up to its newline.
        line: Vec<u8>,
        /// Bytes to read, each piece from its moment on.
        out: VecDeque<(Instant, Vec<u8>)>,
    }

    impl FakeWire {
        fn board(&self) -> Option<FakeBoard> {
            self.index.map(|i| lock(&self.fake.boards)[i].clone())
        }

        fn say(&mut self, at: Instant, text: &str) {
            self.out.push_back((at, format!("{text}\r\n").into_bytes()));
        }

        /// One reply, after a `[stats]` line, as the firmware interleaves
        /// them.
        fn answer(&mut self, text: &str) {
            let now = Instant::now();
            self.say(now, "[stats] bat=87 state=paused heap=81234");
            self.say(now, text);
        }

        fn host_line(&mut self, line: &str) {
            let Some(board) = self.board() else { return };
            let line = line.trim_start_matches('@');
            let (verb, arg) = line.split_once(' ').unwrap_or((line, ""));
            match board.talk {
                Talk::Silent => {}
                Talk::Old => self.answer(&format!("@err 7 {verb}")),
                Talk::Status => match verb {
                    "status" => self.answer(&board.status_line()),
                    "count" => self.count(&board),
                    "identify" => {
                        let label = arg.trim();
                        if label.is_empty() {
                            self.answer("@err 1 identify");
                        } else if label.len() > 16 {
                            self.answer("@err 5 identify");
                        } else if let Some(why) = &board.identify {
                            self.answer(&format!("@err 4 identify {why}"));
                        } else {
                            Fake::note(&self.fake.trace, format!("identify {} {label}", board.port));
                            self.answer("@identify ok");
                        }
                    }
                    other => self.answer(&format!("@err 7 {other}")),
                },
            }
        }

        /// `@count`: refused while it plays or with no FAT card, else a line
        /// a tenth across the fake's pace, then the free bytes — 60 % of
        /// the card — which `@status` says from then on.
        fn count(&mut self, board: &FakeBoard) {
            if board.field("state") == "playing" {
                return self.answer("@err 4 count playing");
            }
            if !matches!(board.field("card").as_str(), "fat32" | "fat16") {
                return self.answer("@err 4 count card");
            }
            Fake::note(&self.fake.trace, format!("count {}", board.port));
            let size: u64 = board.field("size").parse().unwrap_or(0);
            let free = size / 10 * 6;
            let t0 = Instant::now();
            for step in 0..=10u32 {
                self.say(t0 + self.fake.pace * step / 10, &format!("@count {}", step * 10));
            }
            self.say(t0 + self.fake.pace, &format!("@count done free={free}"));
            if let Some(i) = self.index {
                lock(&self.fake.boards)[i].counted = Some(free);
            }
        }

        /// The console's `L`: the partition table's head, the running app.
        fn console_l(&mut self) {
            let Some(board) = self.board() else { return };
            let (Talk::Status | Talk::Old, Some(version)) = (board.talk, board.version()) else { return };
            let now = Instant::now();
            self.say(now, "[flash] partition table as flashed (16 MB chip):");
            self.say(now, "[flash]   label     type subtype   offset    end       size");
            self.say(now, "[stats] bat=87 state=paused heap=81234");
            self.say(now, &format!("[flash] running app: version \"{version}\" (app description), ELF 11c35a4a"));
            self.say(now, "[console] L: the loop task's stack: 5120 B never used during it");
        }
    }

    impl Write for FakeWire {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for &b in buf {
                if self.line.is_empty() {
                    match b {
                        b'@' => self.line.push(b),
                        b'L' => self.console_l(),
                        // Other keys and lone line ends: nothing the fake does.
                        _ => {}
                    }
                } else if b == b'\n' || b == b'\r' {
                    let line = String::from_utf8_lossy(&std::mem::take(&mut self.line)).into_owned();
                    self.host_line(&line);
                } else {
                    self.line.push(b);
                }
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakeWire {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(board) = self.board()
                && !board.present(self.fake.age())
            {
                return Err(io::Error::new(ErrorKind::BrokenPipe, "the fake board was unplugged"));
            }
            match self.out.front_mut() {
                Some((at, bytes)) if *at <= Instant::now() => {
                    // A piece at a time, as a port hands them over.
                    let n = bytes.len().min(buf.len()).min(16);
                    buf[..n].copy_from_slice(&bytes[..n]);
                    bytes.drain(..n);
                    if bytes.is_empty() {
                        self.out.pop_front();
                    }
                    Ok(n)
                }
                _ => {
                    std::thread::sleep(Duration::from_millis(5));
                    Err(io::Error::new(ErrorKind::TimedOut, "nothing yet"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boot_line_is_the_first_complete_line_with_the_firmwares_name() {
        let rom = b"rst:0x1 (POWERON_RESET),boot:0x17 (SPI_FAST_FLASH_BOOT)\r\nload:0x3fff0030\r\n";
        assert_eq!(first_firmware_line(rom), None, "the ROM's chatter is not it");
        let mut bytes = rom.to_vec();
        bytes.extend_from_slice(b"\nmstream-mp3-player v0.5.0 (commit abc1234, 2026-10-01), ELF 1a2b3c4d");
        assert_eq!(first_firmware_line(&bytes), None, "not complete yet");
        bytes.extend_from_slice(b"\r\nCopyright (C) 2026 IrosTheBeggar.\r\n");
        assert_eq!(
            first_firmware_line(&bytes).as_deref(),
            Some("mstream-mp3-player v0.5.0 (commit abc1234, 2026-10-01), ELF 1a2b3c4d")
        );
    }

    #[test]
    fn progress_is_the_share_of_all_the_bytes_whatever_the_chunks_and_knows_a_skipped_write() {
        // Two segments of 50 and 150 bytes; espflash counts the first in 4
        // chunks and the second in 3 (compressed streams: the counts have
        // nothing to do with the bytes).
        let mut seen = Vec::new();
        {
            let mut report = |r: Report| seen.push(r);
            let mut p = Percent::new(vec![50, 150], &mut report);
            p.init(0, 4);
            p.update(2);
            p.update(2);
            p.verifying();
            p.finish(false);
            p.init(0x10000, 3);
            p.update(1);
            p.update(3);
            p.finish(false);
            assert!(p.written);
        }
        assert_eq!(
            seen,
            [
                Report::Chunks { addr: 0, chunks: 4 },
                Report::Percent(12),
                Report::Verifying,
                Report::Chunks { addr: 0x10000, chunks: 3 },
                Report::Percent(50),
                Report::Percent(100),
            ],
            "one report per changed percent, across segments, in bytes; each segment's chunks once"
        );
        // The first segment skipped (no init), the second written whole.
        let mut seen = Vec::new();
        {
            let mut report = |r: Report| seen.push(r);
            let mut p = Percent::new(vec![50, 150], &mut report);
            p.finish(true);
            assert!(!p.written, "a skipped segment never inits: nothing was written yet");
            p.init(0x10000, 2);
            p.update(2);
            p.finish(false);
            assert!(p.written);
        }
        assert_eq!(
            seen,
            [Report::Chunks { addr: 0x10000, chunks: 2 }, Report::Percent(100)],
            "the skipped segment counts as done"
        );
    }

    #[test]
    fn the_fake_runs_a_whole_write_from_its_script() {
        let fake = fake::Fake::new("ours:v0.4.0").with_pace(Duration::from_millis(20));
        let found = fake.candidates().unwrap();
        assert_eq!(found.len(), 1);
        let mut link = fake.open(&found[0], 921_600).unwrap();
        assert_eq!(link.info().flash_mb, Some(16));
        let on_board = link.app_desc().unwrap().expect("a board running our firmware");
        assert_eq!(on_board.version, "v0.4.0");
        let desc = crate::device::firmware::tests::desc_bytes("v0.5.0", AppDesc::OURS);
        let image = crate::device::firmware::tests::merged_bytes(&desc);
        let mut reports = Vec::new();
        let skipped = link.write(&[Segment { offset: 0, data: image }], &mut |r| reports.push(r)).unwrap();
        assert!(!skipped);
        assert_eq!(reports.first(), Some(&Report::Percent(0)));
        assert_eq!(reports.last(), Some(&Report::Verifying));
        let boot = link.restart().unwrap().expect("the fake boots what it wrote");
        assert!(boot.starts_with("mstream-mp3-player v0.5.0"), "{boot}");

        assert!(fake::Fake::new("nodevice").candidates().unwrap().is_empty());
        assert_eq!(fake::Fake::new("two").candidates().unwrap().len(), 2);
        let busy = fake::Fake::new("busy");
        assert!(matches!(busy.open(&found[0], 115_200).err(), Some(DeviceError::Busy { .. })));
        let dying = fake::Fake::new("failwrite").with_pace(Duration::from_millis(20));
        let mut dying = dying.open(&found[0], 115_200).unwrap();
        assert!(dying.write(&[Segment { offset: 0, data: vec![0xFF; 16] }], &mut |_| {}).is_err());
    }
}
