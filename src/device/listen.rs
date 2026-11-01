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
//!
//! The firmware serves its console from its main loop alone, and lists the
//! card's library before that loop first runs: for a while after each
//! start nothing answers, while its other tasks go on printing. A board
//! that answers nothing but shows itself ours that way ([`sign`]) is
//! starting up, not silent, and the page's listens ask it again until it
//! answers ([`ask_till_up`]).

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::DeviceError;
use super::board::{CardKind, CountWhy, Free, Heard, PlayState, Status, Tracks};
use super::firmware::Version;

/// An open port to a running firmware: bytes both ways, a read that gives
/// up after a moment (the serial port's timeout) so the waits below can
/// look at the clock and at a stop.
pub(crate) trait Wire: Read + Write + Send {}
impl<T: Read + Write + Send + ?Sized> Wire for T {}

/// The longest line kept whole: a host line is at most 255 bytes, a log
/// line can be longer (the console's stack report), and what runs past
/// this is cut — memory stays bounded whatever a board sends.
const LINE_KEEP: usize = 1024;
/// The lines a question keeps of what went by, newest last, for [`sign`]:
/// enough to hold two of the firmware's own among the Arduino core's.
const HEARD_KEEP: usize = 32;

/// How long a board that is ours and starting up is given to answer, from
/// the first sign of it. Its firmware answers once its main loop runs, and
/// lists the card's library before that: v0.8.0 and later about 20 s for
/// 20,000 tracks on a card another version used (1.5 s otherwise), v0.7.0
/// about 90 s at every start and minutes on its first start after v0.8.0
/// used the card. Past this, the board is called not answering, as before.
pub(crate) const STARTING_UP: Duration = Duration::from_secs(180);

/// The first release whose firmware answers the host lines, if only with
/// `@err 7`: one older that came up and said its name never will.
const HOST_LINES: (u64, u64, u64) = (0, 6, 0);

/// How long each question waits for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Waits {
    /// `L`'s version line, once `@status` was refused.
    pub l: Duration,
    /// Past a boot line seen while waiting: the firmware is coming up and
    /// answers once its loop runs (its setup mounts the card, reads the
    /// library's index). A board that talks and has shown nothing yet is
    /// listened to this long too ([`Sign::Talk`]).
    pub boot: Duration,
    /// `@count`'s first line, and `@identify`'s answer.
    pub reply: Duration,
    /// The longest silence between two of a count's lines: one comes per
    /// tenth, or every five seconds while the count moves — a 1 TB card's
    /// tenth can take twelve.
    pub count_idle: Duration,
    /// A board starting up is asked again this often, on the same port. A
    /// question it already holds is answered as soon as its loop runs, so
    /// asking again only covers a line lost on the way; every ask waits in
    /// the board's serial buffer (a few hundred bytes) while the loop is
    /// busy, so it is rare: at 30 s, [`STARTING_UP`] queues at most six.
    pub again: Duration,
    /// …and given this long to answer ([`STARTING_UP`]).
    pub up: Duration,
}

impl Waits {
    pub const REAL: Waits = Waits {
        l: Duration::from_millis(1500),
        boot: Duration::from_secs(8),
        reply: Duration::from_millis(1500),
        count_idle: Duration::from_secs(60),
        again: Duration::from_secs(30),
        up: STARTING_UP,
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
    /// The lines that went by unpicked, the last [`HEARD_KEEP`].
    heard: Vec<String>,
}

impl<'a> Console<'a> {
    pub fn new(wire: &'a mut dyn Wire, port: &str, stop: &'a AtomicBool) -> Console<'a> {
        Console { wire, port: port.to_string(), lines: Lines::default(), stop, boot: None, heard: Vec::new() }
    }

    /// What the lines that went by say of the board ([`sign`]).
    fn sign(&self) -> Sign {
        sign(self.boot.as_deref(), &self.heard)
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
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
            if self.heard.len() == HEARD_KEEP {
                self.heard.remove(0);
            }
            self.heard.push(line);
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
/// for the version. Silence is an answer too, and what went by meanwhile
/// says whose ([`sign`]): ours starting up, ours from before the host
/// lines, or silent.
pub(crate) fn ask(
    wire: &mut dyn Wire,
    port: &str,
    wait: Duration,
    waits: Waits,
    stop: &AtomicBool,
    note: Note,
) -> Result<Asked, DeviceError> {
    ask_on(&mut Console::new(wire, port, stop), wait, waits, true, note)
}

/// [`ask`] on a conversation under way. `linger`: a board that talks and
/// has shown nothing yet is given the boot wait before it is called silent
/// — our firmware busy listing its library prints a log line every few
/// seconds, and the first wait is only a second or so.
fn ask_on(
    console: &mut Console,
    wait: Duration,
    waits: Waits,
    linger: bool,
    note: Note,
) -> Result<Asked, DeviceError> {
    note("@status".to_string());
    console.send(b"@status\n")?;
    let pick = |line: &str| {
        if let Some(status) = parse_status(line) {
            return Some(Ok(status));
        }
        match parse_err(line) {
            Some(err) if err.verb == "status" => Some(Err(err.code)),
            _ => None,
        }
    };
    let mut answer = console.reply(Instant::now() + wait, waits.boot, &mut *note, pick)?;
    if answer.is_none() && linger && !console.stopped() && console.sign() == Sign::Talk {
        answer = console.reply(Instant::now() + waits.boot, waits.boot, &mut *note, pick)?;
    }
    let boot = console.boot.clone();
    match answer {
        Some(Ok(status)) => Ok(Asked { heard: Heard::Status(status), flash_mb: None, boot }),
        Some(Err(_code)) => {
            let facts = facts_on(console, waits, &mut *note)?;
            let (version, elf) = match (facts.version, console.boot.as_deref().and_then(parse_boot_line)) {
                (Some(version), _) => (Some(version), facts.elf),
                (None, Some((version, elf))) => (Some(version), elf),
                (None, None) => (None, None),
            };
            let boot = console.boot.clone();
            Ok(Asked { heard: Heard::Old { version, elf }, flash_mb: facts.flash_mb, boot })
        }
        None if console.stopped() => Ok(Asked { heard: Heard::Nothing, flash_mb: None, boot }),
        None => {
            let heard = match console.sign() {
                Sign::Starting { version, elf } => Heard::Starting { version, elf },
                // It came up while asked, said its name, and never answered
                // an `@` line: our firmware from before the host lines.
                Sign::Mute { version, elf } => Heard::Old { version: Some(version), elf },
                Sign::Talk | Sign::Nothing => Heard::Silent,
            };
            Ok(Asked { heard, flash_mb: None, boot })
        }
    }
}

/// [`ask`], and again every `waits.again` on the same port while the board
/// is ours and starting up, until it answers — `waits.up` at most from the
/// first sign of it, after which it is silent, as one that never showed
/// itself. `since`: a sign heard already, before this listen (the write's
/// boot line): from the first question on, a board that says nothing is
/// still starting. `told` hears the board starting up, and again whenever
/// what is known of it grows (its version, from a boot line).
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask_till_up(
    wire: &mut dyn Wire,
    port: &str,
    wait: Duration,
    waits: Waits,
    since: Option<Heard>,
    stop: &AtomicBool,
    note: Note,
    told: &mut dyn FnMut(&Heard),
) -> Result<Asked, DeviceError> {
    let mut until = since.as_ref().map(|_| Instant::now() + waits.up);
    let mut starting = since;
    let mut said: Option<Heard> = None;
    let mut wait = wait;
    // The bytes of a line under way when one question ended: the next
    // reads it whole.
    let mut lines = Lines::default();
    loop {
        let mut console = Console::new(&mut *wire, port, stop);
        console.lines = std::mem::take(&mut lines);
        let asked = ask_on(&mut console, wait, waits, until.is_none(), &mut *note);
        lines = std::mem::take(&mut console.lines);
        let asked = asked?;
        let now = match (&asked.heard, &starting) {
            (Heard::Starting { version, elf }, kept) => {
                let (was, had) = match kept {
                    Some(Heard::Starting { version, elf }) => (version.clone(), elf.clone()),
                    _ => (None, None),
                };
                Heard::Starting { version: version.clone().or(was), elf: elf.clone().or(had) }
            }
            // Nothing went by this time: still listing, as far as anyone
            // can tell.
            (Heard::Silent, Some(kept)) => kept.clone(),
            _ => return Ok(asked),
        };
        let deadline = *until.get_or_insert_with(|| Instant::now() + waits.up);
        if Instant::now() >= deadline {
            return Ok(Asked { heard: Heard::Silent, ..asked });
        }
        if said.as_ref() != Some(&now) {
            told(&now);
            said = Some(now.clone());
        }
        starting = Some(now);
        wait = waits.again;
    }
}

// ── A board that does not answer ────────────────────────────────────────────

/// What the lines a board printed say of it when nothing answered — the one
/// rule that tells our firmware, busy, from a board that is not ours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sign {
    /// Not a line: other firmware, none running, or a hung board.
    Nothing,
    /// Lines, none of them ours by this rule: a board that talks is given
    /// the boot wait before it is called silent.
    Talk,
    /// Our firmware with its console not served yet: its boot line (its
    /// version and ELF), or its own log lines.
    Starting { version: Option<String>, elf: Option<String> },
    /// Our firmware from before the host lines (its boot line names v0.5.0
    /// or older): it never answers.
    Mute { version: String, elf: Option<String> },
}

/// `boot`, the firmware's first line if it went by, and the `lines` that
/// went by unanswered. Conservative, since a board called ours is asked
/// again for minutes and offered no read meanwhile: its boot line, or two
/// lines or more shaped like its own log — a lowercase tag in brackets, a
/// space, words (`[bt] reconnect: paging the remembered headphones …`,
/// `[stats] bat=87`) — never the Arduino core's `[ 11267][W][…]` or
/// ESP-IDF's `I (123) tag:`, which any firmware prints. One such line
/// alone is not enough.
pub(crate) fn sign(boot: Option<&str>, lines: &[String]) -> Sign {
    if let Some((version, elf)) = boot.and_then(parse_boot_line) {
        if Version::parse(&version).is_some_and(|v| v.core < HOST_LINES) {
            return Sign::Mute { version, elf };
        }
        return Sign::Starting { version: Some(version), elf };
    }
    let ours = |line: &&String| {
        let Some((tag, text)) = line.trim().strip_prefix('[').and_then(|rest| rest.split_once("] ")) else {
            return false;
        };
        (2..=10).contains(&tag.len()) && tag.bytes().all(|b| b.is_ascii_lowercase()) && !text.trim().is_empty()
    };
    match lines.iter().filter(ours).count() {
        2.. => Sign::Starting { version: None, elf: None },
        _ if lines.iter().any(|line| !line.trim().is_empty()) => Sign::Talk,
        _ => Sign::Nothing,
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
        again: Duration::from_millis(200),
        up: Duration::from_millis(2500),
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

    /// Lines in the shapes the real Core2 printed while v0.7.0 listed 19,410
    /// tracks (2026-10-10): its own Bluetooth task's, and the Arduino core's.
    const BT: &str = "[bt] reconnect: paging the remembered headphones …";
    const ARDUINO: &str = "[ 11267][W][BluetoothA2DPSource.cpp:551] av_hdl_stack_evt(): av_hdl_stack_evt type 2";

    fn lines(of: &[&str]) -> Vec<String> {
        of.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn only_a_boot_line_or_two_lines_of_our_own_log_say_ours_starting_up() {
        let starting = Sign::Starting { version: None, elf: None };
        assert_eq!(sign(None, &[]), Sign::Nothing);
        assert_eq!(sign(None, &lines(&[BT, ARDUINO, "[stats] bat=87 state=paused"])), starting);
        assert_eq!(sign(None, &lines(&[BT, ARDUINO, BT])), starting, "the same tag twice is two lines");
        assert_eq!(sign(None, &lines(&[ARDUINO, BT, ARDUINO])), Sign::Talk, "one line of ours is not enough");
        // Other firmwares' logs, and lines only shaped like ours at a glance.
        let others = [ARDUINO, "I (1234) wifi: connected", "[INFO] booting", "[Wifi] up", "[a] one letter"];
        let almost = ["[gaplesspadding] too long a tag", "[bt]", "[bt]no space", "[bt]  ", "[b t] spaced", "[bt2] digit"];
        assert_eq!(sign(None, &lines(&[others.as_slice(), almost.as_slice()].concat())), Sign::Talk);
        let boot = "mstream-mp3-player v0.8.0-5-g4e94418 (commit 4e94418, 2026-10-10), ELF be894f89";
        let named = Sign::Starting { version: Some("v0.8.0-5-g4e94418".into()), elf: Some("be894f89".into()) };
        assert_eq!(sign(Some(boot), &[]), named, "its boot line alone, with its version");
        let old = "mstream-mp3-player v0.5.0 (commit 1, 2026-09-30), ELF 17352e55";
        let mute = Sign::Mute { version: "v0.5.0".into(), elf: Some("17352e55".into()) };
        assert_eq!(sign(Some(old), &lines(&[BT, BT])), mute, "from before the host lines: it never answers");
    }

    #[test]
    fn a_board_that_prints_its_own_log_and_never_answers_is_ours_starting_up() {
        let busy = format!("{BT}\r\n{ARDUINO}\r\n[bt] reconnect: page timeout\r\n");
        let asked = asked(&mut Script::new(&[("@status\n", busy.as_str())]));
        assert_eq!(asked.heard, Heard::Starting { version: None, elf: None });
    }

    #[test]
    fn a_board_that_talks_is_listened_to_for_the_boot_wait_and_one_that_prints_nothing_is_not() {
        let stop = AtomicBool::new(false);
        let wait = Duration::from_millis(200);
        let t0 = Instant::now();
        let quiet = ask(&mut Script::new(&[]), "COM3", wait, QUICK, &stop, &mut |_| {}).unwrap();
        assert_eq!(quiet.heard, Heard::Silent);
        assert!(t0.elapsed() < wait + QUICK.boot, "nothing printed: today's one wait, {:?}", t0.elapsed());
        let chatty = format!("{ARDUINO}\r\nI (1234) wifi: connected\r\n");
        let t0 = Instant::now();
        let talking = ask(&mut Script::new(&[("@status\n", chatty.as_str())]), "COM3", wait, QUICK, &stop, &mut |_| {});
        assert_eq!(talking.unwrap().heard, Heard::Silent, "talk that is not ours is silence, after all");
        assert!(t0.elapsed() >= wait + QUICK.boot, "but heard out: {:?}", t0.elapsed());
    }

    /// `ask_till_up` on `wire` with nothing known before it, the starts it
    /// told.
    fn till_up(wire: &mut dyn Wire, waits: Waits, since: Option<Heard>) -> (Asked, Vec<Heard>) {
        let stop = AtomicBool::new(false);
        let mut told = Vec::new();
        let wait = Duration::from_millis(300);
        let asked = ask_till_up(wire, "COM3", wait, waits, since, &stop, &mut |_| {}, &mut |h| told.push(h.clone()));
        (asked.unwrap(), told)
    }

    #[test]
    fn a_board_starting_up_is_asked_again_on_the_same_port_until_it_answers() {
        let busy = format!("{BT}\r\n{BT}\r\n");
        let script = Script::new(&[("@status\n", busy.as_str())]);
        let sent = script.sent.clone();
        let late = b"[stats] bat=87\r\n@status fw=v0.9.0 card=none\r\n".to_vec();
        let mut wire = Delayed { inner: script, late, after: Instant::now() + Duration::from_millis(900) };
        let (asked, told) = till_up(&mut wire, QUICK, None);
        assert!(matches!(asked.heard, Heard::Status(ref s) if s.fw == "v0.9.0"), "{asked:?}");
        assert_eq!(told, [Heard::Starting { version: None, elf: None }], "told once that it is starting up");
        let asks = sent.lock().unwrap().iter().filter(|s| *s == "@status\n").count();
        assert!(asks >= 3, "asked every `again` meanwhile: {asks}");
    }

    #[test]
    fn a_board_that_never_answers_is_silent_once_its_time_is_up() {
        let waits = Waits { up: Duration::from_millis(700), ..QUICK };
        let busy = format!("{BT}\r\n{BT}\r\n");
        let t0 = Instant::now();
        let (asked, told) = till_up(&mut Script::new(&[("@status\n", busy.as_str())]), waits, None);
        assert_eq!(asked.heard, Heard::Silent, "not answering after all");
        assert_eq!(told.len(), 1);
        assert!(t0.elapsed() >= Duration::from_millis(1000), "300 ms, then 700 more: {:?}", t0.elapsed());
        // One that prints nothing is never starting up: asked once.
        let (asked, told) = till_up(&mut Script::new(&[]), waits, None);
        assert_eq!((asked.heard, told.len()), (Heard::Silent, 0));
    }

    #[test]
    fn after_a_boot_line_a_board_that_says_nothing_yet_is_still_starting_up() {
        // The write heard the boot line; the board then lists its library
        // with its radio off: not a line, until it answers.
        let since = Heard::Starting { version: Some("v0.8.0".into()), elf: Some("3523b80e".into()) };
        let late = b"@err 7 status\r\n[flash] running app: version \"v0.8.0\" (app description), ELF 3523b80e\r\n".to_vec();
        let mut wire = Delayed { inner: Script::new(&[]), late, after: Instant::now() + Duration::from_millis(600) };
        let (asked, told) = till_up(&mut wire, QUICK, Some(since.clone()));
        assert_eq!(asked.heard, Heard::Old { version: Some("v0.8.0".into()), elf: Some("3523b80e".into()) });
        assert_eq!(told, [since], "the card says it is starting up meanwhile");
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
