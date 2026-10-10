//! Talking to the firmware that runs on the board, over the same USB serial
//! port, with no reset: the music plays on while the board is asked. The
//! port is opened with DTR and RTS held low (engine::Engine::listen), and
//! the firmware's host lines answer — the USB visualizer's `@` framing
//! (the firmware repo's docs/USB-VISUALIZER.md and docs/HOST-STATUS.md):
//!
//! - `@status` → one line, `@status fw=… elf=… card=… size=… free=… …`;
//!   firmware older than 0.9.0 answers `@err 7 status`, and then its
//!   console's `L` gives the version (sent on its own: right after a boot
//!   the console's Sync drops a key that comes with its Enter, while `@`
//!   lines are never dropped);
//! - `@count` → one count of the card's free clusters, `@count <pct>` as it
//!   goes and `@count done free=<bytes>`;
//! - `@identify <label>` → "This one" and the label on the board's screen
//!   for five seconds, `@identify ok`.
//!
//! The board talks all the time — `[stats]` every few seconds, the boot
//! log after a restart, the console's own replies — and a host line can
//! land between two of its lines, so a reply is picked out of the stream
//! by its first word and everything else passes by. Nothing here resets
//! anything: a board that says nothing is reported as silent, and only a
//! reset someone asks for (desk's Read) can learn more.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::DeviceError;
use super::board::{CardKind, CountWhy, Free, Heard, PlayState, Status, Tracks};

/// An open port to a running firmware: bytes both ways, a read that gives
/// up after a moment (the serial port's timeout) so the waits below can
/// look at the clock and at a stop.
pub(crate) trait Wire: Read + Write + Send {}
impl<T: Read + Write + Send + ?Sized> Wire for T {}

/// The longest line kept whole: a host line is at most 255 bytes, a log
/// line can be longer (the console's stack report), and what runs past
/// this is cut — memory stays bounded whatever a board sends.
const LINE_KEEP: usize = 1024;

/// How long each question waits for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Waits {
    /// `L`'s version line, once `@status` was refused.
    pub l: Duration,
    /// Past a boot line seen while waiting: the firmware is coming up and
    /// answers once its loop runs (its setup mounts the card, reads the
    /// library's index).
    pub boot: Duration,
    /// `@count`'s first line, and `@identify`'s answer.
    pub reply: Duration,
    /// The longest silence between two of a count's lines: one comes per
    /// tenth, or every five seconds while the count moves — a 1 TB card's
    /// tenth can take twelve.
    pub count_idle: Duration,
}

impl Waits {
    pub const REAL: Waits = Waits {
        l: Duration::from_millis(1500),
        boot: Duration::from_secs(8),
        reply: Duration::from_millis(1500),
        count_idle: Duration::from_secs(60),
    };
}

/// Bytes into lines: split at `\n` or `\r`, empty lines dropped, a line
/// past [`LINE_KEEP`] cut.
#[derive(Default)]
pub(crate) struct Lines {
    part: Vec<u8>,
    ready: VecDeque<String>,
}

impl Lines {
    pub fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                if !self.part.is_empty() {
                    self.ready.push_back(String::from_utf8_lossy(&self.part).into_owned());
                    self.part.clear();
                }
            } else if self.part.len() < LINE_KEEP {
                self.part.push(b);
            }
        }
    }

    pub fn next(&mut self) -> Option<String> {
        self.ready.pop_front()
    }
}

/// What the conversations report as they go, for the page's log: the host
/// lines both ways, word for word.
pub(crate) type Note<'a> = &'a mut dyn FnMut(String);

/// A conversation on an open port.
pub(crate) struct Console<'a> {
    wire: &'a mut dyn Wire,
    port: String,
    lines: Lines,
    stop: &'a AtomicBool,
    /// The firmware's first line, if it went by: the board restarted while
    /// it was asked.
    pub boot: Option<String>,
}

impl<'a> Console<'a> {
    pub fn new(wire: &'a mut dyn Wire, port: &str, stop: &'a AtomicBool) -> Console<'a> {
        Console { wire, port: port.to_string(), lines: Lines::default(), stop, boot: None }
    }

    /// One whole line or one key, in a single write: `@` lines with their
    /// `\n`, `L` alone.
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.wire
            .write_all(bytes)
            .and_then(|()| self.wire.flush())
            .map_err(|e| self.lost(&e))
    }

    fn lost(&self, e: &std::io::Error) -> DeviceError {
        match e.kind() {
            ErrorKind::NotFound | ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof => {
                DeviceError::Gone { port: self.port.clone() }
            }
            _ => DeviceError::Link(e.to_string()),
        }
    }

    /// The next line before `deadline`: None at the deadline, or once the
    /// conversation is asked to stop.
    fn line(&mut self, deadline: Instant) -> Result<Option<String>, DeviceError> {
        let mut chunk = [0u8; 256];
        loop {
            if let Some(line) = self.lines.next() {
                let line = line.trim().to_string();
                if self.boot.is_none() && parse_boot_line(&line).is_some() {
                    self.boot = Some(line.clone());
                }
                return Ok(Some(line));
            }
            if Instant::now() >= deadline || self.stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match self.wire.read(&mut chunk) {
                Ok(0) => {}
                Ok(n) => self.lines.push(&chunk[..n]),
                // The port's timeout: nothing yet.
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                }
                Err(e) => return Err(self.lost(&e)),
            }
        }
    }

    /// The first line `pick` takes, before `deadline`; every other line
    /// goes by. A boot line going by moves the deadline out once, to the
    /// boot wait: the firmware answers when its loop starts.
    fn reply<T>(
        &mut self,
        mut deadline: Instant,
        boot_wait: Duration,
        note: Note,
        mut pick: impl FnMut(&str) -> Option<T>,
    ) -> Result<Option<T>, DeviceError> {
        let mut extended = false;
        while let Some(line) = self.line(deadline)? {
            if line.starts_with('@') {
                note(line.clone());
            }
            if let Some(found) = pick(&line) {
                return Ok(Some(found));
            }
            if !extended && self.boot.is_some() {
                extended = true;
                deadline = deadline.max(Instant::now() + boot_wait);
            }
        }
        Ok(None)
    }
}

/// What a listen learned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Asked {
    pub heard: Heard,
    /// From `L`'s partition line, when `L` was asked.
    pub flash_mb: Option<u32>,
    /// The firmware's first line, when the board restarted meanwhile.
    pub boot: Option<String>,
}

/// Ask the running firmware what it is: `@status`, waiting `wait` for the
/// answer; on `@err … status` (our firmware, older than the query), `L`
/// for the version. Silence is an answer too.
pub(crate) fn ask(
    wire: &mut dyn Wire,
    port: &str,
    wait: Duration,
    waits: Waits,
    stop: &AtomicBool,
    note: Note,
) -> Result<Asked, DeviceError> {
    let mut console = Console::new(wire, port, stop);
    note("@status".to_string());
    console.send(b"@status\n")?;
    let answer = console.reply(Instant::now() + wait, waits.boot, &mut *note, |line| {
        if let Some(status) = parse_status(line) {
            return Some(Ok(status));
        }
        match parse_err(line) {
            Some(err) if err.verb == "status" => Some(Err(err.code)),
            _ => None,
        }
    })?;
    match answer {
        Some(Ok(status)) => Ok(Asked { heard: Heard::Status(status), flash_mb: None, boot: console.boot }),
        Some(Err(_code)) => {
            let facts = facts_on(&mut console, waits, &mut *note)?;
            let (version, elf) = match (facts.version, console.boot.as_deref().and_then(parse_boot_line)) {
                (Some(version), _) => (Some(version), facts.elf),
                (None, Some((version, elf))) => (Some(version), elf),
                (None, None) => (None, None),
            };
            Ok(Asked { heard: Heard::Old { version, elf }, flash_mb: facts.flash_mb, boot: console.boot })
        }
        None if console.stop.load(Ordering::Relaxed) => {
            Ok(Asked { heard: Heard::Nothing, flash_mb: None, boot: console.boot })
        }
        // It came up while asked, said its name, and never answered an `@`
        // line: our firmware from before the host lines (v0.5.0 and older).
        None => match console.boot.as_deref().and_then(parse_boot_line) {
            Some((version, elf)) => {
                Ok(Asked { heard: Heard::Old { version: Some(version), elf }, flash_mb: None, boot: console.boot })
            }
            None => Ok(Asked { heard: Heard::Silent, flash_mb: None, boot: console.boot }),
        },
    }
}

/// What the console's `L` says that the page wants: the running app's
/// version and ELF, and the flash chip's size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub version: Option<String>,
    pub elf: Option<String>,
    pub flash_mb: Option<u32>,
}

/// `L` alone, and its lines read up to the running app's (the partition
/// table's header, with the chip's size, comes before it).
pub(crate) fn facts(
    wire: &mut dyn Wire,
    port: &str,
    waits: Waits,
    stop: &AtomicBool,
    note: Note,
) -> Result<Facts, DeviceError> {
    let mut console = Console::new(wire, port, stop);
    facts_on(&mut console, waits, note)
}

fn facts_on(console: &mut Console, waits: Waits, note: Note) -> Result<Facts, DeviceError> {
    note("L".to_string());
    console.send(b"L")?;
    let mut flash_mb = None;
    let found = console.reply(Instant::now() + waits.l, waits.boot, &mut *note, |line| {
        if let Some(mb) = parse_flash_chip(line) {
            flash_mb = Some(mb);
        }
        parse_running_app(line)
    })?;
    let (version, elf) = found.map_or((None, None), |(v, e)| (Some(v), e));
    Ok(Facts { version, elf, flash_mb })
}

/// Count the card's free space: `progress` hears each percent. The free
/// bytes at the end, or why it did not run or stopped — `part_way` true
/// for a count that had begun.
pub(crate) fn count(
    wire: &mut dyn Wire,
    port: &str,
    waits: Waits,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(u8),
    note: Note,
) -> Result<u64, (CountWhy, bool)> {
    enum Line {
        Pct(u8),
        Done(u64),
        Refused(CountWhy),
    }
    let pick = |line: &str| {
        if let Some(rest) = line.strip_prefix("@count ") {
            if let Some(free) = rest.strip_prefix("done ").and_then(|f| field(f, "free")) {
                return free.parse::<u64>().ok().map(Line::Done);
            }
            return rest.trim().parse::<u8>().ok().map(Line::Pct);
        }
        match parse_err(line) {
            Some(err) if err.verb == "count" && err.code == 7 => Some(Line::Refused(CountWhy::Old)),
            Some(err) if err.verb == "count" => {
                Some(Line::Refused(err.why.as_deref().map_or(CountWhy::Other(String::new()), CountWhy::from_word)))
            }
            _ => None,
        }
    };
    let mut console = Console::new(wire, port, stop);
    note("@count".to_string());
    console.send(b"@count\n").map_err(|e| (CountWhy::Port(e), false))?;
    let mut begun = false;
    loop {
        let wait = if begun { waits.count_idle } else { waits.reply };
        match console.reply(Instant::now() + wait, waits.boot, &mut *note, pick) {
            Ok(Some(Line::Pct(pct))) => {
                begun = true;
                progress(pct.min(100));
            }
            Ok(Some(Line::Done(free))) => return Ok(free),
            Ok(Some(Line::Refused(why))) => return Err((why, begun)),
            Ok(None) if console.stop.load(Ordering::Relaxed) => return Err((CountWhy::Abandoned, begun)),
            Ok(None) => return Err((CountWhy::NoAnswer, begun)),
            Err(e) => return Err((CountWhy::Port(e), begun)),
        }
    }
}

/// Why the board did not show itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IdentifyWhy {
    /// The start-up screen: the UI is not up yet.
    Ui,
    /// Another screen has the display (the touch calibration, a tool).
    Screen,
    /// A computer drives the dancer: the Dance tab shows it already.
    Viz,
    /// The firmware has no `@identify`.
    Old,
    /// The label was refused (none, or past 16 bytes).
    Label,
    NoAnswer,
    Port(DeviceError),
    Other(String),
}

/// Show `label` on the board's screen ("This one"), five seconds and a
/// buzz.
pub(crate) fn identify(
    wire: &mut dyn Wire,
    port: &str,
    label: &str,
    waits: Waits,
    stop: &AtomicBool,
    note: Note,
) -> Result<(), IdentifyWhy> {
    let mut console = Console::new(wire, port, stop);
    let line = format!("@identify {label}");
    note(line.clone());
    console.send(format!("{line}\n").as_bytes()).map_err(IdentifyWhy::Port)?;
    let answer = console
        .reply(Instant::now() + waits.reply, waits.boot, &mut *note, |line| {
            if line.trim() == "@identify ok" {
                return Some(Ok(()));
            }
            match parse_err(line) {
                Some(err) if err.verb == "identify" => Some(Err(match (err.code, err.why.as_deref()) {
                    (7, _) => IdentifyWhy::Old,
                    (1 | 5, _) => IdentifyWhy::Label,
                    (_, Some("ui")) => IdentifyWhy::Ui,
                    (_, Some("screen")) => IdentifyWhy::Screen,
                    (_, Some("viz")) => IdentifyWhy::Viz,
                    (_, why) => IdentifyWhy::Other(why.unwrap_or_default().to_string()),
                })),
                _ => None,
            }
        })
        .map_err(IdentifyWhy::Port)?;
    answer.unwrap_or(Err(IdentifyWhy::NoAnswer))
}

/// The label `@identify` puts on the board's screen: the computer's name
/// for the port, short — `COM5`, `ttyACM0`, `cu.usbmodem14101` — within
/// the firmware's 16 printable bytes (a longer one it refuses).
pub(crate) fn label_for(port: &str) -> String {
    let name = port.rsplit(['/', '\\']).next().unwrap_or(port);
    let name: String = name.chars().filter(|c| c.is_ascii_graphic()).collect();
    if name.len() <= 16 {
        return name;
    }
    let short = name.strip_prefix("cu.").or_else(|| name.strip_prefix("tty.")).unwrap_or(&name);
    // The end tells sockets apart (`usbserial-14130`): keep the end.
    short[short.len().saturating_sub(16)..].to_string()
}

// ── The lines ───────────────────────────────────────────────────────────────

/// `@status`'s fields, in any order; a key this player does not know is
/// passed over (a later firmware adds fields before `bt=`).
pub(crate) fn parse_status(line: &str) -> Option<Status> {
    let rest = line.trim().strip_prefix("@status")?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let mut status = Status {
        fw: String::new(),
        elf: None,
        card: CardKind::Unknown(String::new()),
        size: None,
        free: Free::NotCounted,
        tracks: Tracks::Unknown,
        music: None,
        bat: None,
        state: PlayState::Unknown(String::new()),
        bt: None,
    };
    let mut fw = None;
    for pair in rest.split_whitespace() {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let known = |v: &str| (v != "-" && v != "?").then(|| v.to_string());
        match key {
            "fw" => fw = known(value),
            "elf" => status.elf = known(value),
            "card" => status.card = CardKind::parse(value),
            "size" => status.size = value.parse().ok(),
            "free" => {
                status.free = match value {
                    "counting" => Free::Counting { pct: None },
                    v => v.parse().map_or(Free::NotCounted, Free::Bytes),
                }
            }
            "tracks" => {
                status.tracks = match value {
                    "building" => Tracks::Building,
                    "-" => Tracks::NoCard,
                    v => v.parse().map_or(Tracks::Unknown, Tracks::Count),
                }
            }
            "music" => status.music = value.parse().ok(),
            "bat" => status.bat = value.parse::<u8>().ok().filter(|b| *b <= 100),
            "state" => status.state = PlayState::parse(value),
            "bt" => status.bt = known(value).map(|v| percent_decode(&v)),
            _ => {}
        }
    }
    // A status with no version is no status: the verdict rests on it.
    status.fw = fw?;
    Some(status)
}

/// `@err <code> <verb> [<why>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ErrLine {
    pub code: u8,
    pub verb: String,
    pub why: Option<String>,
}

pub(crate) fn parse_err(line: &str) -> Option<ErrLine> {
    let mut words = line.trim().strip_prefix("@err ")?.split_whitespace();
    let code = words.next()?.parse().ok()?;
    let verb = words.next()?.to_string();
    let why = words.next().map(str::to_string);
    Some(ErrLine { code, verb, why })
}

/// `[flash] running app: version "v0.8.0" (app description), ELF 11c35a4a`
/// → the version and the ELF.
pub(crate) fn parse_running_app(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.trim().strip_prefix("[flash] running app: version \"")?;
    let (version, tail) = rest.split_once('"')?;
    let elf = tail.split_once("ELF ").map(|(_, e)| e.trim().to_string()).filter(|e| !e.is_empty());
    (!version.is_empty()).then(|| (version.to_string(), elf))
}

/// `[flash] partition table as flashed (16 MB chip):` → 16.
pub(crate) fn parse_flash_chip(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix("[flash] partition table as flashed (")?;
    rest.split_once(" MB chip")?.0.trim().parse().ok()
}

/// `mstream-mp3-player v0.6.0 (commit abc1234, 2026-10-02), ELF 1a2b3c4d`
/// → the version and the ELF.
pub(crate) fn parse_boot_line(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.trim().strip_prefix(super::firmware::AppDesc::OURS)?.trim_start();
    let version = rest.split_whitespace().next()?.to_string();
    let elf = rest.rsplit_once("ELF ").map(|(_, e)| e.trim().to_string()).filter(|e| !e.is_empty());
    version.starts_with('v').then_some((version, elf))
}

/// The value of `key=` among space-separated fields.
fn field<'a>(fields: &'a str, key: &str) -> Option<&'a str> {
    fields.split_whitespace().find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// `Paul%27s%20headphones` → `Paul's headphones`: RFC 3986's encoding of
/// the name's UTF-8, read back byte for byte. A broken escape stays as it
/// was written.
pub(crate) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (hex(i + 1), hex(i + 2))
        {
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io;
    use std::sync::Mutex;

    use super::*;

    /// The real Core2's answer of 2026-10-10 (a 1 TB card).
    const REAL: &str = "@status fw=v0.8.0-5-g4e94418 elf=be894f89 card=fat32 size=1023871549440 free=1022132355072 tracks=113 music=? bat=100 state=paused bt=SPYDRONE";

    /// A wire that answers from a script: what the board sends back after
    /// each thing the host writes, in chunks as a port delivers them.
    pub(crate) struct Script {
        /// (what the host's write must contain, the bytes that come back)
        pub replies: VecDeque<(&'static str, Vec<u8>)>,
        pub out: VecDeque<u8>,
        pub sent: std::sync::Arc<Mutex<Vec<String>>>,
        /// Bytes per read: small, so lines arrive in pieces.
        pub chunk: usize,
    }

    impl Script {
        pub(crate) fn new(replies: &[(&'static str, &str)]) -> Script {
            Script {
                replies: replies.iter().map(|(k, v)| (*k, v.as_bytes().to_vec())).collect(),
                out: VecDeque::new(),
                sent: Default::default(),
                chunk: 7,
            }
        }
    }

    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let text = String::from_utf8_lossy(buf).to_string();
            self.sent.lock().unwrap().push(text.clone());
            if let Some((want, _)) = self.replies.front()
                && text.contains(want)
            {
                let (_, bytes) = self.replies.pop_front().unwrap();
                self.out.extend(bytes);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.out.is_empty() {
                std::thread::sleep(Duration::from_millis(2));
                return Err(io::Error::new(ErrorKind::TimedOut, "nothing yet"));
            }
            let n = self.chunk.min(buf.len()).min(self.out.len());
            for slot in buf.iter_mut().take(n) {
                *slot = self.out.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    /// Waits that a scripted board, answering at once, never runs into
    /// even on a busy machine — and that a silent one costs little.
    pub(crate) const QUICK: Waits = Waits {
        l: Duration::from_millis(800),
        boot: Duration::from_millis(800),
        reply: Duration::from_millis(800),
        count_idle: Duration::from_millis(1500),
    };

    fn asked(script: &mut Script) -> Asked {
        let stop = AtomicBool::new(false);
        ask(script, "COM3", Duration::from_millis(800), QUICK, &stop, &mut |_| {}).unwrap()
    }

    #[test]
    fn the_real_boards_status_line_reads_back_field_for_field() {
        let status = parse_status(REAL).expect("a status");
        assert_eq!(status.fw, "v0.8.0-5-g4e94418");
        assert_eq!(status.elf.as_deref(), Some("be894f89"));
        assert_eq!(status.card, CardKind::Fat32);
        assert_eq!(status.size, Some(1_023_871_549_440));
        assert_eq!(status.free, Free::Bytes(1_022_132_355_072));
        assert_eq!(status.tracks, Tracks::Count(113));
        assert_eq!(status.music, None, "music=? is unknown");
        assert_eq!(status.bat, Some(100));
        assert_eq!(status.state, PlayState::Paused);
        assert_eq!(status.bt.as_deref(), Some("SPYDRONE"));
    }

    #[test]
    fn a_status_line_passes_over_what_it_does_not_know_and_never_guesses() {
        let line = "@status fw=v0.9.0 elf=63ee7a2b card=exfat size=63864569856 free=? tracks=building music=? bat=- state=host shiny=new bt=Paul%27s%20headphones";
        let status = parse_status(line).unwrap();
        assert_eq!(status.card, CardKind::ExFat);
        assert_eq!(status.free, Free::NotCounted, "free=? is not counted, never 0");
        assert_eq!(status.tracks, Tracks::Building);
        assert_eq!(status.bat, None);
        assert_eq!(status.state, PlayState::Host);
        assert_eq!(status.bt.as_deref(), Some("Paul's headphones"));
        let none = parse_status("@status fw=v0.9.0 card=none size=- free=? tracks=- bt=-").unwrap();
        assert_eq!((none.card, none.size, none.tracks, none.bt), (CardKind::None, None, Tracks::NoCard, None));
        assert_eq!(parse_status("@status fw=v0.9.0 free=counting card=fat16").unwrap().free, Free::Counting { pct: None });
        assert_eq!(parse_status("@status fw=v0.9.0 card=zfs").unwrap().card, CardKind::Unknown("zfs".into()));
        assert!(parse_status("@status card=fat32").is_none(), "no version, no status");
        assert!(parse_status("@statusfw=v1").is_none());
        assert!(parse_status("[stats] fw=v0.9.0").is_none());
        assert_eq!(parse_status("@status fw=v0.9.0 bat=250").unwrap().bat, None, "a battery past 100 is no reading");
    }

    #[test]
    fn errors_running_apps_flash_chips_and_boot_lines_parse() {
        assert_eq!(parse_err("@err 7 status"), Some(ErrLine { code: 7, verb: "status".into(), why: None }));
        assert_eq!(parse_err("@err 4 count playing").unwrap().why.as_deref(), Some("playing"));
        assert_eq!(parse_err("@err x status"), None);
        let real = "[flash] running app: version \"v0.8.0\" (app description), ELF 11c35a4a";
        assert_eq!(parse_running_app(real), Some(("v0.8.0".into(), Some("11c35a4a".into()))));
        assert_eq!(parse_running_app("[flash] running ota_0 at 0x10000"), None);
        assert_eq!(parse_flash_chip("[flash] partition table as flashed (16 MB chip):"), Some(16));
        let boot = "mstream-mp3-player v0.6.0 (commit abc1234, 2026-10-02), ELF 1a2b3c4d";
        assert_eq!(parse_boot_line(boot), Some(("v0.6.0".into(), Some("1a2b3c4d".into()))));
        assert_eq!(parse_boot_line("mstream-mp3-player starting"), None);
        assert_eq!(percent_decode("caf%C3%A9%20%E2%80%A6"), "café …");
        assert_eq!(percent_decode("100%"), "100%", "a broken escape stays");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn lines_come_whole_out_of_any_chunking_and_an_endless_one_is_cut() {
        let mut lines = Lines::default();
        lines.push(b"[stats] bat=87\r\n@sta");
        assert_eq!(lines.next().as_deref(), Some("[stats] bat=87"));
        assert_eq!(lines.next(), None, "half a line waits");
        lines.push(b"tus fw=v0.9.0\n\n\r");
        assert_eq!(lines.next().as_deref(), Some("@status fw=v0.9.0"));
        assert_eq!(lines.next(), None, "empty lines are none");
        lines.push(&[b'x'; 5000]);
        lines.push(b"\n");
        assert_eq!(lines.next().map(|l| l.len()), Some(LINE_KEEP));
    }

    #[test]
    fn a_board_that_answers_status_is_read_out_of_its_log_chatter() {
        let reply = format!("[stats] bat=100 state=paused heap=81234\r\n[ui] scroll\r\n{REAL}\r\n[stats] bat=100\r\n");
        let mut script = Script::new(&[("@status\n", reply.as_str())]);
        let sent = script.sent.clone();
        let asked = asked(&mut script);
        assert!(matches!(&asked.heard, Heard::Status(s) if s.fw == "v0.8.0-5-g4e94418"), "{asked:?}");
        assert_eq!(*sent.lock().unwrap(), ["@status\n"], "one line, whole, with its newline; no L");
    }

    #[test]
    fn old_firmware_refuses_status_and_l_alone_gives_its_version_and_flash() {
        let l_reply = "\
[flash] partition table as flashed (16 MB chip):\r\n\
[stats] bat=64\r\n\
[flash] * running ota_0 (OTA state valid); boots ota_0; next update ota_1\r\n\
[flash] running app: version \"v0.7.0\" (app description), ELF 11c35a4a\r\n\
[console] L: the loop task's stack: 5000 B never used during it\r\n";
        let mut script = Script::new(&[("@status\n", "[stats] x\r\n@err 7 status\r\n"), ("L", l_reply)]);
        let sent = script.sent.clone();
        let asked = asked(&mut script);
        assert_eq!(asked.heard, Heard::Old { version: Some("v0.7.0".into()), elf: Some("11c35a4a".into()) });
        assert_eq!(asked.flash_mb, Some(16));
        assert_eq!(*sent.lock().unwrap(), ["@status\n", "L"], "L on its own: no Enter in the same write");
    }

    #[test]
    fn silence_is_reported_as_silence_and_l_that_never_comes_leaves_the_version_unknown() {
        assert_eq!(asked(&mut Script::new(&[])).heard, Heard::Silent);
        let mut refusing = Script::new(&[("@status\n", "@err 7 status\n")]);
        assert_eq!(asked(&mut refusing).heard, Heard::Old { version: None, elf: None });
    }

    #[test]
    fn a_board_coming_up_while_asked_gets_the_boot_wait_and_its_boot_line_counts() {
        // The boot line, then the answer after the first wait has run out:
        // the boot line moved the deadline.
        let boot = "rst:0x1 (POWERON_RESET)\r\nmstream-mp3-player v0.9.0 (commit abc1234, 2026-10-10), ELF 63ee7a2b\r\n";
        let stop = AtomicBool::new(false);
        let late = b"@status fw=v0.9.0 card=none\n".to_vec();
        let after = Instant::now() + Duration::from_millis(700);
        let mut wire = Delayed { inner: Script::new(&[("@status\n", boot)]), late, after };
        let asked = ask(&mut wire, "COM3", Duration::from_millis(300), QUICK, &stop, &mut |_| {}).unwrap();
        assert!(matches!(asked.heard, Heard::Status(ref s) if s.fw == "v0.9.0"), "{asked:?}");
        assert!(asked.boot.unwrap().contains("v0.9.0"));
        // A board from before the host lines: its name, and nothing to `@`.
        let mut old = Script::new(&[("@status\n", "mstream-mp3-player v0.5.0 (commit 1, 2026-09-30), ELF 0a0b0c0d\r\n")]);
        let asked = ask(&mut old, "COM3", Duration::from_millis(300), QUICK, &stop, &mut |_| {}).unwrap();
        assert_eq!(asked.heard, Heard::Old { version: Some("v0.5.0".into()), elf: Some("0a0b0c0d".into()) });
    }

    /// A script, and one more line that arrives only after a moment.
    struct Delayed {
        inner: Script,
        late: Vec<u8>,
        after: Instant,
    }

    impl Write for Delayed {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for Delayed {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if Instant::now() >= self.after && !self.late.is_empty() {
                self.inner.out.extend(self.late.drain(..));
            }
            self.inner.read(buf)
        }
    }

    #[test]
    fn a_stop_ends_a_wait_at_once() {
        let stop = AtomicBool::new(true);
        let t0 = Instant::now();
        let asked = ask(&mut Script::new(&[]), "COM3", Duration::from_secs(5), QUICK, &stop, &mut |_| {}).unwrap();
        assert_eq!(asked.heard, Heard::Nothing, "stopped: nothing learned, nothing claimed");
        assert!(t0.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_count_reports_its_tenths_and_its_free_bytes_or_why_it_did_not_run() {
        let stop = AtomicBool::new(false);
        let run = "@count 0\r\n[stats] x\r\n@count 10\r\n@count 50\r\n@count 100\r\n@count done free=38214565888\r\n";
        let mut seen = Vec::new();
        let free = count(&mut Script::new(&[("@count\n", run)]), "COM3", QUICK, &stop, &mut |p| seen.push(p), &mut |_| {});
        assert_eq!(free, Ok(38_214_565_888));
        assert_eq!(seen, [0, 10, 50, 100]);
        let playing = count(&mut Script::new(&[("@count\n", "@err 4 count playing\n")]), "COM3", QUICK, &stop, &mut |_| {}, &mut |_| {});
        assert_eq!(playing, Err((CountWhy::Playing, false)));
        let stopped = count(&mut Script::new(&[("@count\n", "@count 0\n@count 30\n@err 4 count playing\n")]), "COM3", QUICK, &stop, &mut |_| {}, &mut |_| {});
        assert_eq!(stopped, Err((CountWhy::Playing, true)), "stopped part way by a play");
        let old = count(&mut Script::new(&[("@count\n", "@err 7 count\n")]), "COM3", QUICK, &stop, &mut |_| {}, &mut |_| {});
        assert_eq!(old, Err((CountWhy::Old, false)));
        let silent = count(&mut Script::new(&[]), "COM3", QUICK, &stop, &mut |_| {}, &mut |_| {});
        assert_eq!(silent, Err((CountWhy::NoAnswer, false)));
    }

    #[test]
    fn identify_says_ok_or_why_not_and_its_label_fits_the_firmwares_sixteen_bytes() {
        let stop = AtomicBool::new(false);
        let mut ok = Script::new(&[("@identify COM5\n", "@identify ok\n")]);
        let sent = ok.sent.clone();
        assert_eq!(identify(&mut ok, "COM5", "COM5", QUICK, &stop, &mut |_| {}), Ok(()));
        assert_eq!(*sent.lock().unwrap(), ["@identify COM5\n"]);
        let ui = identify(&mut Script::new(&[("@identify", "@err 4 identify ui\n")]), "COM5", "COM5", QUICK, &stop, &mut |_| {});
        assert_eq!(ui, Err(IdentifyWhy::Ui));
        let old = identify(&mut Script::new(&[("@identify", "@err 7 identify\n")]), "COM5", "COM5", QUICK, &stop, &mut |_| {});
        assert_eq!(old, Err(IdentifyWhy::Old));
        let quiet = identify(&mut Script::new(&[]), "COM5", "COM5", QUICK, &stop, &mut |_| {});
        assert_eq!(quiet, Err(IdentifyWhy::NoAnswer));
        assert_eq!(label_for("COM5"), "COM5");
        assert_eq!(label_for("/dev/ttyACM0"), "ttyACM0");
        assert_eq!(label_for("/dev/cu.usbmodem14101"), "cu.usbmodem14101");
        assert_eq!(label_for("/dev/cu.usbserial-14130"), "usbserial-14130");
        assert!(label_for("/dev/cu.wchusbserial-5B1F007751").len() <= 16);
    }
}
