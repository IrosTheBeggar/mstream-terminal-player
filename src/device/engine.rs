//! Talking to the board. An [`Engine`] lists the boards and opens one; the
//! [`Link`] it hands back is that board held in its bootloader — probed,
//! erased, written, then restarted into whatever it now holds. The real
//! engine is espflash as a library: its reset dance, its RAM stub (which
//! is what makes the writes compressed and the MD5 checks possible), its
//! command protocol. The fake answers the same calls from a one-word
//! script in `MSTREAM_DEVICE_FAKE`, so the page and the e2e leg run with
//! no board on the desk.

use std::io::Read;
use std::time::{Duration, Instant};

use espflash::connection::{Connection, Port, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::{FlashSize, Flasher};
use espflash::target::{Chip, ProgressCallbacks};
use serialport::{FlowControl, SerialPort};

use super::DeviceError;
use super::firmware::{AppDesc, Segment};
use super::ports::{self, Candidate};

/// The baud the bootloader is reached at (the ROM's own), the one the
/// firmware's `pio run -t upload` writes at, and the steps between when a
/// bridge or a cable cannot hold the fast one.
pub(crate) const SYNC_BAUD: u32 = 115_200;
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

pub(crate) trait Engine {
    fn candidates(&self) -> Result<Vec<Candidate>, DeviceError>;
    /// The serial ports that are NOT a Core2's bridge, by name — what the
    /// page lists when it finds no board.
    fn others(&self) -> Vec<String> {
        Vec::new()
    }
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

// ── espflash ──────────────────────────────────────────────────────────────

pub(crate) struct Esp;

impl Engine for Esp {
    fn candidates(&self) -> Result<Vec<Candidate>, DeviceError> {
        ports::candidates()
    }

    fn others(&self) -> Vec<String> {
        ports::others()
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

// ── The fake ──────────────────────────────────────────────────────────────

pub(crate) mod fake {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{DeviceInfo, Engine, Link, Report};
    use crate::device::DeviceError;
    use crate::device::firmware::{AppDesc, Segment};
    use crate::device::ports::Candidate;

    /// `MSTREAM_DEVICE_FAKE` words: `fresh` (a blank board), `ours:<v>` (a
    /// board running our firmware at that version), `other` (someone
    /// else's firmware), `nodevice`, `two` (two boards), `busy` (the port
    /// is held), `nosync` (nothing answers), `failwrite` (the write dies
    /// halfway). Anything else reads as `fresh`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum Spec {
        Fresh,
        Ours(String),
        Other,
        NoDevice,
        Two,
        Busy,
        NoSync,
        FailWrite,
    }

    /// Cloned, it shares its trace: a test keeps one copy and hands the
    /// worker's engine factory the other.
    #[derive(Clone)]
    pub(crate) struct Fake {
        spec: Spec,
        /// How long the fake's write takes end to end — the real one takes
        /// half a minute; a test wants the bar to move, not to wait.
        pub pace: Duration,
        /// How long each `open` takes to answer — espflash's connect, a
        /// second or more on a real board, during which the worker hears
        /// nothing. None by default.
        reach: Duration,
        /// What was done to the board, in order (`open 921600`, `let go`,
        /// `restart`, `let go FAKE0`), for the tests to read.
        trace: Arc<Mutex<Vec<String>>>,
    }

    impl Fake {
        pub(crate) fn new(spec: &str) -> Fake {
            let spec = match spec.trim() {
                "ours" => Spec::Ours("v0.4.0".to_string()),
                s if s.starts_with("ours:") => Spec::Ours(s["ours:".len()..].to_string()),
                "other" => Spec::Other,
                "nodevice" => Spec::NoDevice,
                "two" => Spec::Two,
                "busy" => Spec::Busy,
                "nosync" => Spec::NoSync,
                "failwrite" => Spec::FailWrite,
                _ => Spec::Fresh,
            };
            Fake {
                spec,
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

        /// What was done to the board so far, shared with the fake.
        #[cfg(test)]
        pub(crate) fn trace(&self) -> Arc<Mutex<Vec<String>>> {
            self.trace.clone()
        }

        fn note(trace: &Mutex<Vec<String>>, what: String) {
            trace.lock().unwrap_or_else(|e| e.into_inner()).push(what);
        }

        fn board(port: &str) -> Candidate {
            Candidate {
                port: port.to_string(),
                bridge: "CH9102",
                usb: serialport::UsbPortInfo {
                    vid: 0x1A86,
                    pid: 0x55D4,
                    serial_number: Some(format!("FAKE{}", &port[port.len() - 1..])),
                    manufacturer: None,
                    product: Some("fake Core2".to_string()),
                },
            }
        }
    }

    impl Engine for Fake {
        fn candidates(&self) -> Result<Vec<Candidate>, DeviceError> {
            Ok(match self.spec {
                Spec::NoDevice => vec![],
                Spec::Two => vec![Fake::board("FAKE0"), Fake::board("FAKE1")],
                _ => vec![Fake::board("FAKE0")],
            })
        }

        /// A desk with no Core2 still has a port on it — the page's
        /// "ports seen" line has something to say in the e2e leg.
        fn others(&self) -> Vec<String> {
            match self.spec {
                Spec::NoDevice => vec!["FAKECOM1".to_string()],
                _ => Vec::new(),
            }
        }

        fn open(&self, candidate: &Candidate, baud: u32) -> Result<Box<dyn Link>, DeviceError> {
            if self.spec == Spec::Busy {
                let detail = "held by the fake".into();
                return Err(DeviceError::Busy { port: candidate.port.clone(), detail });
            }
            // The port opened: the board is reset into its bootloader,
            // whether or not it then answers.
            Fake::note(&self.trace, format!("open {baud}"));
            std::thread::sleep(self.reach);
            if self.spec == Spec::NoSync {
                let detail = "no answer".into();
                return Err(DeviceError::NoSync { port: candidate.port.clone(), detail });
            }
            Ok(Box::new(FakeLink {
                spec: self.spec.clone(),
                pace: self.pace,
                info: DeviceInfo {
                    port: candidate.port.clone(),
                    bridge: candidate.bridge.to_string(),
                    chip: "esp32".to_string(),
                    revision: Some((3, 1)),
                    flash_mb: Some(16),
                    baud,
                },
                written: None,
                trace: self.trace.clone(),
            }))
        }

        fn let_go(&self, candidate: &Candidate) {
            Fake::note(&self.trace, format!("let go {}", candidate.port));
        }
    }

    struct FakeLink {
        spec: Spec,
        pace: Duration,
        info: DeviceInfo,
        /// The version of the image written, for the boot line.
        written: Option<String>,
        trace: Arc<Mutex<Vec<String>>>,
    }

    impl Link for FakeLink {
        fn info(&self) -> &DeviceInfo {
            &self.info
        }

        fn app_desc(&mut self) -> Result<Option<AppDesc>, DeviceError> {
            Ok(match &self.spec {
                Spec::Ours(version) => Some(AppDesc {
                    version: version.clone(),
                    project: AppDesc::OURS.to_string(),
                    idf: "v5.5.5".to_string(),
                    elf8: "fa4e0000".to_string(),
                }),
                Spec::Other => Some(AppDesc {
                    version: "3.3.12".to_string(),
                    project: "arduino-lib-builder".to_string(),
                    idf: "v5.5.5".to_string(),
                    elf8: "00000000".to_string(),
                }),
                _ => None,
            })
        }

        fn erase(&mut self) -> Result<(), DeviceError> {
            std::thread::sleep(self.pace / 4);
            Ok(())
        }

        fn write(&mut self, segments: &[Segment], report: &mut dyn FnMut(Report)) -> Result<bool, DeviceError> {
            let steps = 10u8;
            for step in 0..=steps {
                std::thread::sleep(self.pace / u32::from(steps));
                let pct = step * 10;
                if self.spec == Spec::FailWrite && pct == 50 {
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
            Fake::note(&self.trace, "restart".to_string());
            Ok(self
                .written
                .map(|v| format!("mstream-mp3-player {v} (commit fake000, 2026-10-01), ELF fa4e0000")))
        }

        fn let_go(self: Box<Self>) {
            Fake::note(&self.trace, "let go".to_string());
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
