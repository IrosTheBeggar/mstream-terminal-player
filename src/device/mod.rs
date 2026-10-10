//! The mStream MP3 player — an M5Stack Core2 running mstream-mp3-player —
//! over its USB cable. `mstream-player device list` names the boards that
//! look like one, each with what it runs and its card; `mstream-player
//! device flash` installs or updates the firmware; `mstream-player device
//! releases` lists the firmware's releases as GitHub has them. No mStream
//! session is involved: the board is a USB serial port, and the firmware is
//! a file — a release of IrosTheBeggar/mstream-mp3-player, in its QIO or
//! its DIO build, or a build of it.
//!
//! The pieces: [`ports`] finds the boards by the USB bridge chips M5Stack
//! ships, [`firmware`] finds the image (a path, a release, the pin), reads
//! what it is and orders versions, [`engine`] talks to the board (serialport
//! and espflash, or a fake for the tests), [`listen`] asks the running
//! firmware what it is and what its card holds — with no reset — and
//! [`board`] is one board as the page draws it. [`desk`] is the worker
//! that watches every board and does what is asked of each; [`flow`] holds
//! the write's pieces it reuses (the plan, the baud ladder, the write and
//! its retry); [`page`] draws it — one card about the board in view — on
//! the admin hub's terminal session, and the GUI player hosts the same
//! page, with no flags, as its MP3 Player tab (src/gui/device.rs,
//! docs/ux-contracts/mp3-player-screen.md). `--yes` prints the steps as
//! lines instead.

mod board;
mod desk;
mod engine;
mod firmware;
mod flow;
mod listen;
mod page;
mod ports;

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use clap::{Args, Subcommand};
use rust_i18n::t;

use self::board::{Board, Card, CardKind, CardUnknown, Free, Heard, Tracks, Verdict, Work, Written, gb};
use self::desk::{All, Cmd, Event, ImageState, LogKind, Refusal};
use self::engine::Engine;
use self::firmware::{Cached, Image, Images, Release, Supply, Target};

// What the GUI's MP3 Player tab holds of this module: the page, the one
// way to build it — with no flags — outside the tests, whose page rides
// channels they hold (`Page::quiet`, its `Ends`), and the line it draws
// while a page it left lets its ports go.
#[cfg(not(test))]
pub(crate) use self::page::hosted;
pub(crate) use self::page::{LET_GO, Page, draw_waiting};
#[cfg(test)]
pub(crate) use self::{desk::Cmd as WorkerCmd, page::Ends};

#[derive(Args)]
pub struct DeviceArgs {
    #[command(subcommand)]
    what: DeviceCmd,
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// List the boards on USB that look like a Core2, each with its firmware and card, asked over
    /// USB without a reset
    List(ListArgs),
    /// Install or update the player firmware on a connected Core2
    Flash(FlashArgs),
    /// List the firmware's releases on GitHub (one request): each one's date, its builds, and
    /// the ones already on this computer
    Releases(ReleasesArgs),
}

#[derive(Args, Clone)]
pub struct ReleasesArgs {
    /// Pre-releases too: hidden otherwise
    #[arg(long = "pre-releases", visible_alias = "pre")]
    pre: bool,
}

#[derive(Args, Clone)]
pub struct ListArgs {
    /// Only the ports, as they are listed: nothing is opened
    #[arg(long)]
    ports: bool,
}

#[derive(Args, Clone)]
pub struct FlashArgs {
    /// A firmware to write instead of the pinned release: a `*-full.bin` from
    /// a release, or a build directory holding `firmware.factory.bin`
    #[arg(long, value_name = "PATH")]
    firmware: Option<PathBuf>,

    /// A firmware release to download instead of the pinned one, by its tag
    /// (checked against that release's SHA256SUMS). The boards are still
    /// measured against the pinned release: this is what the next write puts
    /// on them
    #[arg(long, value_name = "TAG", conflicts_with = "firmware")]
    release: Option<String>,

    /// The flash mode of the image to write, for the pinned release or
    /// --release: qio (faster) or dio (runs on every Core2, for one that
    /// keeps restarting). Without it each board keeps the mode it runs. A
    /// --firmware file has its own, and a flag that says otherwise is refused
    #[arg(long, value_enum, value_name = "MODE")]
    flash_mode: Option<firmware::Mode>,

    /// The serial port the Core2 is on (default: the one Core2-shaped port)
    #[arg(long, value_name = "PORT")]
    port: Option<String>,

    /// Erase the whole flash first — a first install over other firmware.
    /// Without it the flash is erased only when the board does not run this
    /// firmware already
    #[arg(long)]
    erase: bool,

    /// Never erase: write the image over whatever is there (an update)
    #[arg(long, conflicts_with = "erase")]
    no_erase: bool,

    /// No questions and no screen: print each step and write
    #[arg(long, short = 'y')]
    yes: bool,

    /// With --yes: update every board that runs an older release of this
    /// firmware to the pinned release, one after another, each in the flash
    /// mode it runs (or --flash-mode's) — never an install, an erase or a
    /// step back; the rest are named and left as they are
    #[arg(long, requires = "yes", conflicts_with_all = ["port", "erase", "firmware", "release"])]
    all: bool,

    /// The mStream server this board will pair with — the launcher passes
    /// it beside every page it opens; nothing reads it yet
    #[arg(long, hide = true, value_name = "URL")]
    server: Option<String>,
}

impl FlashArgs {
    /// What `--erase` / `--no-erase` asked, or nothing: then what is on the
    /// board decides (flow::plan).
    fn erase_asked(&self) -> Option<bool> {
        if self.erase {
            Some(true)
        } else if self.no_erase {
            Some(false)
        } else {
            None
        }
    }

    /// What the flags choose for the next write, or nothing: then each
    /// board's own default, the pin in the mode it runs.
    fn preset(&self) -> Option<Image> {
        Image::from_flags(self.firmware.clone(), self.release.clone(), self.flash_mode)
    }

    /// `--flash-mode` beside `--firmware`: the file's header must say the
    /// same, or the run is refused before any board is touched. A file that
    /// cannot be read is left for the worker to say so, as it always has.
    fn mode_refused(&self) -> Option<DeviceError> {
        let (Some(path), Some(mode)) = (&self.firmware, self.flash_mode) else { return None };
        let firmware = firmware::read_local(path).ok()?;
        firmware::mode_conflict(&firmware.facts, mode)
    }
}

pub fn run(args: DeviceArgs) -> i32 {
    match args.what {
        DeviceCmd::List(args) => list(args.ports),
        DeviceCmd::Releases(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            releases_on(&*firmware::net(), args.pre, &mut out, &mut err)
        }
        DeviceCmd::Flash(args) => {
            if let Some(refused) = args.mode_refused() {
                eprintln!("mstream-player: {}", refused.text());
                return 2;
            }
            if args.yes {
                return lines(args);
            }
            crate::setup::boot_language();
            page::run(args)
        }
    }
}

/// `device list`: one line per board, exit 1 when there is none — a script
/// can ask "is a Core2 plugged in?" without parsing anything.
fn list(ports_only: bool) -> i32 {
    let engine = engine::from_env();
    let target = firmware::pin_target();
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    list_on(&*engine, ports_only, target.as_ref(), desk::Timing::REAL, &mut out, &mut err)
}

/// Each board's line is today's — port, bridge, serial, so a script that
/// reads those still works — and then what the board said when it was
/// asked over USB, all at once and with no reset (about a second): its
/// firmware against the pin, then its card. `--ports` opens nothing and
/// prints today's lines alone. Exit 0 with boards, 1 with none, 2 when the
/// ports cannot be listed.
fn list_on(
    engine: &dyn Engine,
    ports_only: bool,
    target: Option<&Target>,
    timing: desk::Timing,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let found = match engine.candidates() {
        Ok(found) => found,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: {}", e.text());
            return 2;
        }
    };
    if found.is_empty() {
        let _ = writeln!(out, "{}", t!("dev.no_device_title"));
        return 1;
    }
    if ports_only {
        for board in found {
            let _ = writeln!(out, "{}", board.describe());
        }
        return 0;
    }
    for mut board in desk::listen_all(engine, &found, timing.at_rest, timing.waits) {
        board.verdict = board.judge(target);
        let _ = writeln!(out, "{}", board_line(&board, target));
    }
    0
}

/// `COM3 · CH9102 · serial 5B1F007751 · v0.8.0, up to date · card 38.2 GB
/// free of 59.6 GB, 1,284 tracks`. In plain English whatever the locale,
/// as the `plan:` line is: these are lines a script reads.
fn board_line(board: &Board, target: Option<&Target>) -> String {
    let mut line = format!("{} · {}", board.candidate.describe(), verdict_words(board, target));
    if let Some(card) = card_words(board) {
        line.push_str(&format!(" · {card}"));
    }
    line
}

fn verdict_words(board: &Board, target: Option<&Target>) -> String {
    // A board on DIO says so beside its version, read from its ELF; QIO, the
    // default, goes unsaid.
    let dio = board.mode().is_some_and(|m| m.mode == firmware::Mode::Dio);
    let version = match board.version() {
        Some(version) if dio => format!("{version} · DIO"),
        Some(version) => version.to_string(),
        None => "?".to_string(),
    };
    let to = target.map_or("?", |t| t.version.as_str());
    match &board.verdict {
        Verdict::Asking => "not asked".to_string(),
        Verdict::UpToDate => format!("{version}, up to date"),
        Verdict::Update => format!("{version}, update to {to}"),
        Verdict::DevUpdate => format!("{version}, a development build — update to {to}"),
        Verdict::Newer => format!("{version}, up to date (ahead of this player's {to})"),
        Verdict::Unplaced if board.version().is_none() => "mStream firmware, too old to say its version".to_string(),
        Verdict::Unplaced => format!("{version}, not comparable with {to}"),
        Verdict::Other { name } => format!("other firmware ({name})"),
        Verdict::Blank => "nothing installed".to_string(),
        Verdict::Silent => "not answering — may not be an MP3 player".to_string(),
        Verdict::NotCore2 { found } => format!("not a Core2 ({found})"),
        Verdict::InUse => "in use by another program".to_string(),
        Verdict::Unreadable => match &board.heard {
            Heard::Failed(e) => e.text(),
            _ => "unreadable".to_string(),
        },
        Verdict::HalfWritten => "half written — write it again".to_string(),
    }
}

fn card_words(board: &Board) -> Option<String> {
    let tracks = |tracks: &Tracks| match tracks {
        Tracks::Count(0) => Some("no tracks".to_string()),
        Tracks::Count(1) => Some("1 track".to_string()),
        Tracks::Count(n) => Some(format!("{} tracks", crate::admin::fmt_count(*n))),
        Tracks::Building => Some("tracks being indexed".to_string()),
        Tracks::NoCard | Tracks::Unknown => None,
    };
    let size = |size: &Option<u64>| size.map(|s| format!("{} GB", gb(s)));
    Some(match board.card() {
        Card::Fat { size: card, free, tracks: count, .. } => {
            let mut words = match (&free, card) {
                (Free::Bytes(free), Some(card)) => format!("card {} GB free of {} GB", gb(*free), gb(card)),
                (Free::NotCounted, _) => format!("card {}, free space not counted", size(&card).unwrap_or_default()),
                (Free::Counting { .. }, _) => format!("card {}, counting free space", size(&card).unwrap_or_default()),
                (Free::Bytes(free), None) => format!("card {} GB free", gb(*free)),
            };
            if let Some(tracks) = tracks(&count) {
                words.push_str(&format!(", {tracks}"));
            }
            words
        }
        Card::Empty => "no card".to_string(),
        Card::Foreign { kind, size: card } => {
            let what = match &kind {
                CardKind::ExFat => "an exFAT card — the player reads FAT32 only".to_string(),
                CardKind::Ntfs => "an NTFS card — the player reads FAT32 only".to_string(),
                CardKind::Gpt => "a GPT card — the player reads MBR cards only".to_string(),
                CardKind::Unreadable => "a card that cannot be read".to_string(),
                CardKind::Unknown(word) if !word.is_empty() => format!("a {word} card"),
                _ => "a card the player cannot read".to_string(),
            };
            match size(&card) {
                Some(size) => format!("{what} ({size})"),
                None => what,
            }
        }
        Card::Unknown(CardUnknown::OldFirmware) => {
            format!("card not reported by {}", board.version().unwrap_or("this firmware"))
        }
        Card::Unknown(_) => return None,
    })
}

/// Why `--all` left a board as it was, in a word or two.
fn skip_words(board: &Board) -> &'static str {
    if board.left_out() {
        return "its own next write";
    }
    match board.verdict {
        Verdict::UpToDate => "up to date",
        Verdict::Newer => "up to date, ahead of this player's release",
        Verdict::Silent => "not answering",
        Verdict::InUse => "in use",
        Verdict::Other { .. } | Verdict::Blank => "not mStream firmware",
        Verdict::NotCore2 { .. } => "not a Core2",
        Verdict::HalfWritten => "half written",
        _ => "not comparable",
    }
}

/// `device flash --yes`: the desk on the image first (today's order: no
/// board is looked at for an image it cannot have), its reports printed as
/// lines, the one question answered by the flags. Several boards with no
/// `--port` is a refusal, not a guess — unless `--all`.
fn lines(args: FlashArgs) -> i32 {
    let engine: desk::Shared = Arc::from(engine::from_env());
    let setup = desk::Setup {
        supply: firmware::net(),
        preset: args.preset(),
        port: args.port.clone(),
        timing: desk::Timing::REAL,
        firmware_first: true,
    };
    let (cmds, events) = desk::spawn(engine, setup);
    let mode = Mode { all: args.all, named: args.port.is_some(), erase: args.erase_asked() };
    let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
    let code = lines_on(mode, &cmds, &events, &mut out, &mut err);
    // Letting go is quick: no board is in its bootloader once a line run
    // has ended, and a listen lets its port go at once.
    let _ = cmds.send(Cmd::Quit);
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while let Ok(event) = events.recv_timeout(until.saturating_duration_since(std::time::Instant::now())) {
        if event == Event::Released {
            break;
        }
    }
    code
}

/// What `--yes` was asked to do.
#[derive(Clone, Copy, Debug)]
struct Mode {
    all: bool,
    named: bool,
    erase: Option<bool>,
}

/// The line run's whole story: the image, the boards heard, the one
/// decision, then the write (or Update all's writes) as lines. Returns the
/// exit code.
fn lines_on(
    mode: Mode,
    cmds: &Sender<Cmd>,
    events: &Receiver<Event>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let mut boards: Vec<Board> = Vec::new();
    let mut others: Vec<String> = Vec::new();
    let mut watched = false;
    let mut decided = false;
    let mut target: Option<Target> = None;
    // The board being written now, and with --all, the list it is part of.
    let mut current: Option<String> = None;
    let mut skipped: Vec<String> = Vec::new();
    let mut last_pct: Option<u8> = None;
    // A board that keeps restarting after its write: the run says so, and
    // ends with 1 — written and checked is not running.
    let mut looped = false;
    let indent = if mode.all { "  " } else { "" };
    loop {
        let Ok(event) = events.recv_timeout(Duration::from_secs(600)) else {
            let _ = writeln!(err, "mstream-player: {}", t!("note.worker_gone"));
            return 1;
        };
        match event {
            Event::Log { kind: LogKind::Phase, port: None, text } => {
                let _ = writeln!(out, "{text}");
            }
            // The steps of the board being written; the listens' and the
            // other boards' lines are the page's log, not these.
            Event::Log { kind: LogKind::Phase, port: Some(port), text } if current.as_ref() == Some(&port) => {
                let _ = writeln!(out, "{indent}{text}");
            }
            Event::Log { .. } => {}
            // Every image the run gets — the flags', the pin's, the pin's
            // DIO build for a board that runs DIO — is said as it comes.
            Event::Image { state: ImageState::Getting { done, total: Some(total) }, .. } if total > 0 => {
                // Quarters, not every chunk: a line per packet is noise.
                let pct = ((done * 100) / total).min(100) as u8;
                if pct.is_multiple_of(25) && last_pct != Some(pct) {
                    last_pct = Some(pct);
                    let _ = writeln!(out, "  {pct}%");
                }
            }
            Event::Image { state: ImageState::Ready(facts), .. } => {
                last_pct = None;
                let _ = writeln!(out, "{}", flow::firmware_line(&facts));
            }
            Event::Image { .. } | Event::Download { .. } | Event::Firmware { .. } => {}
            Event::FirmwareFailed(e) | Event::Failed(e) => {
                let _ = writeln!(err, "mstream-player: {}", e.text());
                return 1;
            }
            Event::Vetted { .. } | Event::Releases(_) => {}
            Event::Target(found) => target = Some(found),
            Event::Watch { ports, others: seen } => {
                watched = true;
                others = seen;
                boards.retain(|b| ports.iter().any(|p| p == b.port()));
                boards.sort_by_key(|b| ports.iter().position(|p| p == b.port()));
            }
            Event::Gone { port } => boards.retain(|b| b.port() != port),
            Event::Board(board) => {
                let port = board.port().to_string();
                match boards.iter_mut().find(|b| b.port() == port) {
                    Some(known) => *known = board.clone(),
                    None => boards.push(board.clone()),
                }
                // The board being written: its percent, and how it ended.
                // (A board just queued carries no result: the write clears
                // the last one as it starts.)
                if current.as_deref() == Some(port.as_str()) {
                    if let Work::Writing { pct: Some(pct), .. } = board.work
                        && pct.is_multiple_of(10)
                        && last_pct != Some(pct)
                    {
                        last_pct = Some(pct);
                        let _ = writeln!(out, "{indent}  {pct}%");
                    }
                    match &board.written {
                        Some(Written::Done { version, took, skipped: same, boot, image, looping, .. }) => {
                            last_pct = None;
                            current = None;
                            let restarting = looping.map(|l| loop_line(l.restarts, l.secs, image));
                            looped |= restarting.is_some();
                            if mode.all {
                                let _ = writeln!(out, "  done: {version} written and checked in {} s", took.as_secs());
                                if let Some(line) = &restarting {
                                    let _ = writeln!(out, "  {line}");
                                }
                            } else {
                                if *same {
                                    let _ = writeln!(out, "{}", t!("dev.done_skipped"));
                                }
                                let _ = writeln!(out, "{}", t!("dev.done_body", version = version));
                                if let Some(line) = boot {
                                    let _ = writeln!(out, "{}", t!("dev.done_booted", line = line));
                                }
                                if let Some(line) = restarting {
                                    let _ = writeln!(err, "mstream-player: {line}");
                                    return 1;
                                }
                                return 0;
                            }
                        }
                        Some(Written::Failed { error, .. }) => {
                            let _ = writeln!(err, "mstream-player: {}", error.text());
                            if let Some(hint) = error.hint() {
                                let _ = writeln!(err, "  {hint}");
                            }
                            if !mode.all {
                                return 1;
                            }
                            current = None;
                        }
                        None => {}
                    }
                }
            }
            Event::Plan { info, on_board, plan, image, .. } => {
                let _ = writeln!(out, "{indent}board: {}", info.describe());
                let _ = writeln!(out, "{indent}{}: {}", t!("dev.on_board"), flow::on_board_text(on_board.as_ref()));
                let _ = writeln!(out, "{indent}plan: {} · {}", plan.describe(), flow::image_words(&image));
            }
            Event::All(All::Running { ports, at }) => {
                let port = ports[at].clone();
                let board = boards.iter().find(|b| b.port() == port);
                let from = board.and_then(Board::version).unwrap_or("?").to_string();
                let to = target.as_ref().map_or("?", |t| t.version.as_str());
                // A board that runs DIO is updated in DIO, and says so.
                let dio = board.is_some_and(|b| b.update_image() == Image::Pin(firmware::Mode::Dio));
                let _ = writeln!(out, "{port} · {from} → {to}{}", if dio { " · DIO" } else { "" });
                current = Some(port);
            }
            Event::All(All::Done { ports, passed, .. }) => {
                // Passed over at its turn — unplugged, or no longer behind:
                // named with the rest.
                skipped.extend(passed.iter().map(|port| match boards.iter().find(|b| b.port() == port) {
                    Some(board) => format!("{port} ({})", skip_words(board)),
                    None => format!("{port} (unplugged)"),
                }));
                let _ = writeln!(out, "{}", updated_line(ports.len() - passed.len(), ports.len(), &skipped));
                return i32::from(looped);
            }
            Event::All(All::Stopped { ports, at, error }) => {
                if error.is_none() {
                    let port = &ports[at];
                    let _ = writeln!(err, "mstream-player: {}", t!("dev.log_all_stopped", port = port));
                }
                let _ = writeln!(out, "{}", updated_line(at, ports.len(), &skipped));
                return 1;
            }
            Event::Refused { why: Refusal::NothingToUpdate, .. } => {
                let _ = writeln!(out, "{}", updated_line(0, 0, &skipped));
                return 0;
            }
            Event::Refused { port, why, .. } => {
                let _ = writeln!(err, "mstream-player: {} — {why:?}", port.unwrap_or_default());
                return 1;
            }
            Event::Identified { .. } | Event::Released => {}
        }

        let settled = watched && boards.iter().all(|b| b.verdict != Verdict::Asking && b.work == Work::Idle);
        if decided || !settled {
            continue;
        }
        decided = true;
        if boards.is_empty() {
            let _ = writeln!(out, "{}", t!("dev.no_device_title"));
            let _ = writeln!(out, "{}", t!("dev.no_device_body"));
            if !others.is_empty() {
                let _ = writeln!(out, "{} {}", t!("dev.ports_seen"), others.join(", "));
            }
            return 1;
        }
        if mode.all {
            let wanted: Vec<String> =
                boards.iter().filter(|b| b.in_update_all()).map(|b| b.port().to_string()).collect();
            skipped = boards
                .iter()
                .filter(|b| !b.in_update_all())
                .map(|b| format!("{} ({})", b.port(), skip_words(b)))
                .collect();
            if wanted.is_empty() {
                let _ = writeln!(out, "{}", updated_line(0, 0, &skipped));
                return 0;
            }
            if cmds.send(Cmd::UpdateAll { ports: wanted }).is_err() {
                return 1;
            }
            continue;
        }
        if boards.len() > 1 && !mode.named {
            let _ = writeln!(out, "{}", t!("dev.several_title"));
            for board in &boards {
                let _ = writeln!(out, "  {}", board_line(board, target.as_ref()));
            }
            let _ = writeln!(out, "{}", t!("dev.several_pass_port_all"));
            return 1;
        }
        let board = &boards[0];
        // A port another program holds, or one that would not open: said as
        // the write would have said it, and nothing tried.
        let refused = match &board.heard {
            Heard::InUse { detail } => {
                Some(DeviceError::Busy { port: board.port().to_string(), detail: detail.clone() })
            }
            Heard::Failed(e) => Some(e.clone()),
            _ => None,
        };
        if let Some(e) = refused {
            let _ = writeln!(err, "mstream-player: {}", e.text());
            if let Some(hint) = e.hint() {
                let _ = writeln!(err, "  {hint}");
            }
            return 1;
        }
        current = Some(board.port().to_string());
        if cmds.send(Cmd::Write { port: board.port().to_string(), erase: mode.erase }).is_err() {
            return 1;
        }
    }
}

/// `device releases`: one line per release GitHub lists with a merged
/// image, newest first — its tag, its date, its builds, and what this
/// computer has of it — in plain English, as the other lines a script
/// reads. Pre-releases only with `--pre-releases`. Exit 0 with the list;
/// 1 when GitHub did not give it, after the releases this computer can
/// write without it.
fn releases_on(supply: &dyn Supply, pre: bool, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match supply.releases() {
        Ok(list) => {
            let cached = supply.cached();
            for release in list.iter().filter(|r| pre || !r.pre) {
                let _ = writeln!(out, "{}", release_line(release, &cached));
            }
            let hidden = list.iter().filter(|r| r.pre && !pre).count();
            match hidden {
                0 => {}
                1 => {
                    let _ = writeln!(out, "(1 pre-release hidden: --pre-releases lists it)");
                }
                n => {
                    let _ = writeln!(out, "({n} pre-releases hidden: --pre-releases lists them)");
                }
            }
            0
        }
        Err(why) => {
            let mut line = why.text();
            if let Some(detail) = why.detail() {
                line.push_str(&format!(" ({detail})"));
            }
            let _ = writeln!(err, "mstream-player: {line}");
            let cached = supply.cached();
            if !cached.is_empty() {
                let _ = writeln!(out, "on this computer, written without the network:");
                for kept in cached {
                    let modes: Vec<&str> = kept.modes.iter().map(|m| m.word()).collect();
                    let _ = writeln!(out, "  {} · {}", kept.tag, modes.join(" and "));
                }
            }
            1
        }
    }
}

/// `v0.7.0          2026-10-07  QIO and DIO  · on this computer: QIO`.
fn release_line(release: &Release, cached: &[Cached]) -> String {
    let images = match release.images {
        Images::Both => "QIO and DIO",
        Images::DioOnly => "DIO only",
        Images::QioOnly => "QIO only",
    };
    let mut line = format!("{:<15} {:<10}  {images}", release.tag, release.date);
    if release.pre {
        line.push_str(" · pre-release");
    }
    if release.is_pin() {
        line.push_str(" · this player's release");
    } else if release.newer_than_pin() {
        line.push_str(" · newer than this player");
    }
    if let Some(kept) = cached.iter().find(|c| c.tag == release.tag) {
        let modes: Vec<&str> = kept.modes.iter().map(|m| m.word()).collect();
        line.push_str(&format!(" · on this computer: {}", modes.join(" and ")));
    }
    line
}

/// `the board keeps restarting: 3 restarts in 6 s, and never its first
/// line — …`: a write that went on and left the board in a restart loop,
/// with the cure where there is one. Plain English, as the plan line.
fn loop_line(restarts: usize, secs: u64, image: &firmware::ImageFacts) -> String {
    let mut line = format!("the board keeps restarting: {restarts} restarts in {secs} s, and never its first line");
    if desk::dio_cure(image).is_some() {
        line.push_str(" — its flash may not run QIO; --flash-mode dio writes the DIO build");
    }
    line
}

/// `updated 2 of 2 · skipped: COM9 (not answering)`.
fn updated_line(k: usize, n: usize, skipped: &[String]) -> String {
    let mut line = t!("dev.all_updated", k = k, n = n).to_string();
    if !skipped.is_empty() {
        line.push_str(&format!(" · {}", t!("dev.all_skipped", list = skipped.join(", "))));
    }
    line
}

/// What went wrong, in the shapes the page answers differently: who to
/// blame (a port in use, a missing permission, a silent board, the wrong
/// board) decides the sentence AND the hint under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeviceError {
    /// The OS would not even list its ports.
    List(String),
    /// The port exists but is open elsewhere (a serial monitor, the IDE).
    Busy { port: String, detail: String },
    /// Linux: the account may not open the device node.
    Permission { port: String, detail: String },
    /// The port vanished mid-way: the cable, most likely.
    Gone { port: String },
    /// The board never answered its bootloader's sync.
    NoSync { port: String, detail: String },
    /// Something answered, and it is not an ESP32.
    WrongChip { found: String },
    /// An ESP32 with the wrong flash: not a Core2.
    WrongFlash { found: String },
    /// A command on the open link failed (reading, erasing, writing).
    Link(String),
    /// The firmware could not be found, downloaded or trusted.
    Firmware(String),
}

impl DeviceError {
    /// The sentence the page shows in gold (and `--yes` prints).
    pub(crate) fn text(&self) -> String {
        match self {
            DeviceError::List(err) => t!("dev.err_list", err = err).to_string(),
            DeviceError::Busy { port, detail } => {
                t!("dev.err_busy", port = port, err = detail).to_string()
            }
            DeviceError::Permission { port, detail } => {
                t!("dev.err_permission", port = port, err = detail).to_string()
            }
            DeviceError::Gone { port } => t!("dev.err_gone", port = port).to_string(),
            DeviceError::NoSync { port, detail } => {
                t!("dev.err_nosync", port = port, err = detail).to_string()
            }
            DeviceError::WrongChip { found } => t!("dev.err_chip", found = found).to_string(),
            DeviceError::WrongFlash { found } => t!("dev.err_flash", found = found).to_string(),
            DeviceError::Link(err) => t!("dev.err_link", err = err).to_string(),
            DeviceError::Firmware(err) => err.clone(),
        }
    }

    /// What to try, under the sentence — only where there is something.
    pub(crate) fn hint(&self) -> Option<String> {
        let key = match self {
            DeviceError::Busy { .. } => "dev.hint_busy",
            DeviceError::Permission { .. } => "dev.hint_permission",
            DeviceError::Gone { .. } | DeviceError::NoSync { .. } => "dev.hint_cable",
            DeviceError::WrongChip { .. } | DeviceError::WrongFlash { .. } => "dev.hint_board",
            DeviceError::Link(_) => "dev.hint_link",
            DeviceError::List(_) | DeviceError::Firmware(_) => return None,
        };
        Some(t!(key).to_string())
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::desk::tests::QUICK;
    use super::engine::fake::Fake;
    use super::firmware::tests::{Scratch, Shelf, desc_bytes, merged_in};
    use super::firmware::{AppDesc, Mode as Build};
    use super::*;

    #[test]
    fn the_erase_flags_say_yes_no_or_nothing() {
        let base = FlashArgs {
            firmware: None,
            release: None,
            port: None,
            erase: false,
            no_erase: false,
            yes: false,
            all: false,
            server: None,
            flash_mode: None,
        };
        assert_eq!(base.erase_asked(), None, "neither flag: the board decides");
        assert_eq!(FlashArgs { erase: true, ..base.clone() }.erase_asked(), Some(true));
        assert_eq!(FlashArgs { no_erase: true, ..base.clone() }.erase_asked(), Some(false));
    }

    #[test]
    fn every_error_has_a_sentence_and_the_actionable_ones_a_hint() {
        let _en = crate::setup::tests::in_locale("en");
        let busy = DeviceError::Busy { port: "COM3".into(), detail: "Access is denied".into() };
        assert!(busy.text().contains("COM3"), "{}", busy.text());
        assert!(busy.hint().is_some_and(|h| h.contains("monitor")), "a busy port names the usual holder");
        let nosync = DeviceError::NoSync { port: "COM3".into(), detail: "timeout".into() };
        assert!(nosync.hint().is_some_and(|h| h.contains("cable")));
        let fw = DeviceError::Firmware("no such file".into());
        assert_eq!(fw.text(), "no such file", "a firmware error is already a sentence");
        assert!(fw.hint().is_none());
        let chip = DeviceError::WrongChip { found: "esp32s3".into() };
        assert!(chip.text().contains("esp32s3"));
    }

    #[derive(Parser)]
    struct Line {
        #[command(subcommand)]
        what: DeviceCmd,
    }

    #[test]
    fn all_needs_yes_and_never_comes_with_a_port_an_erase_or_a_file() {
        let parse = |args: &[&str]| Line::try_parse_from(["mstream-player"].iter().chain(args)).map(|l| l.what);
        assert!(matches!(parse(&["flash", "--yes", "--all"]), Ok(DeviceCmd::Flash(FlashArgs { all: true, .. }))));
        assert!(parse(&["flash", "--all"]).is_err(), "--all is the line mode's");
        assert!(parse(&["flash", "--yes", "--all", "--port", "COM3"]).is_err());
        assert!(parse(&["flash", "--yes", "--all", "--erase"]).is_err());
        assert!(parse(&["flash", "--yes", "--all", "--firmware", "x.bin"]).is_err());
        // Update all writes the pin, each board in its own mode: another
        // release is one board's next write, from its own tab or --port.
        assert!(parse(&["flash", "--yes", "--all", "--release", "v0.7.0"]).is_err());
        assert!(parse(&["flash", "--yes", "--all", "--flash-mode", "dio"]).is_ok(), "one mode for every board");
        let mode = |args: &[&str]| match parse(args) {
            Ok(DeviceCmd::Flash(flags)) => flags.flash_mode,
            _ => None,
        };
        assert_eq!(mode(&["flash", "--flash-mode", "dio"]), Some(Build::Dio));
        assert_eq!(mode(&["flash", "--release", "v0.7.0", "--flash-mode", "QIO"]), None, "the values are lower case");
        assert_eq!(mode(&["flash", "--release", "v0.7.0", "--flash-mode", "qio"]), Some(Build::Qio));
        assert!(parse(&["flash", "--flash-mode", "fast"]).is_err(), "no write speed, no other mode");
        assert!(matches!(parse(&["releases"]), Ok(DeviceCmd::Releases(ReleasesArgs { pre: false }))));
        assert!(matches!(parse(&["releases", "--pre-releases"]), Ok(DeviceCmd::Releases(ReleasesArgs { pre: true }))));
        assert!(matches!(parse(&["releases", "--pre"]), Ok(DeviceCmd::Releases(ReleasesArgs { pre: true }))));
        assert!(matches!(parse(&["list"]), Ok(DeviceCmd::List(ListArgs { ports: false }))));
        assert!(matches!(parse(&["list", "--ports"]), Ok(DeviceCmd::List(ListArgs { ports: true }))));
    }

    fn listed(fake: &Fake, ports_only: bool) -> (i32, Vec<String>) {
        let target = Target { version: "v0.8.0".into(), answers_status: false };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = list_on(fake, ports_only, Some(&target), QUICK, &mut out, &mut err);
        assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
        (code, String::from_utf8(out).unwrap().lines().map(str::to_string).collect())
    }

    #[test]
    fn device_list_adds_each_boards_firmware_and_card_asked_without_a_reset() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("status:v0.8.0,old:v0.7.0,chip,busy,status:v0.9.0/card=exfat/free=?/tracks=-");
        let (code, lines) = listed(&fake, false);
        assert_eq!(code, 0);
        assert_eq!(
            lines,
            [
                "FAKE0 · CH9102 · serial FAKE0 · v0.8.0, up to date · card 38.2 GB free of 63.9 GB, 1,284 tracks",
                "FAKE1 · CH9102 · serial FAKE1 · v0.7.0, update to v0.8.0 · card not reported by v0.7.0",
                "FAKE2 · CP210x · serial CHIP2 · not answering — may not be an MP3 player",
                "FAKE3 · CH9102 · serial FAKE3 · in use by another program",
                "FAKE4 · CH9102 · serial FAKE4 · v0.9.0, up to date (ahead of this player's v0.8.0) · an exFAT card — the player reads FAT32 only (63.9 GB)",
            ]
        );
        let trace = fake.trace().lock().unwrap().clone();
        assert!(trace.iter().all(|t| t.starts_with("listen")), "asked, never reset: {trace:?}");

        // --ports: today's lines, and nothing opened.
        let (code, lines) = listed(&fake, true);
        assert_eq!(code, 0);
        assert_eq!(lines[0], "FAKE0 · CH9102 · serial FAKE0");
        assert_eq!(lines.len(), 5);
        assert_eq!(fake.trace().lock().unwrap().len(), trace.len(), "no port opened for --ports");

        let (code, lines) = listed(&Fake::new("nodevice"), false);
        assert_eq!((code, lines), (1, vec!["No Core2 found".to_string()]));
    }

    #[test]
    fn device_list_words_a_count_under_way_an_empty_card_and_a_dev_build() {
        let _en = crate::setup::tests::in_locale("en");
        let spec = "status:v0.8.0/free=counting/tracks=building,status:v0.8.0/card=none/size=-/tracks=-,old:v0.6.0-37-g221d99d";
        let (_, lines) = listed(&Fake::new(spec), false);
        let counting = "v0.8.0, up to date · card 63.9 GB, counting free space, tracks being indexed";
        assert_eq!(lines[0], format!("FAKE0 · CH9102 · serial FAKE0 · {counting}"));
        assert_eq!(lines[1], "FAKE1 · CH9102 · serial FAKE1 · v0.8.0, up to date · no card");
        let dev = "v0.6.0-37-g221d99d, a development build — update to v0.8.0 · card not reported by v0.6.0-37-g221d99d";
        assert_eq!(lines[2], format!("FAKE2 · CH9102 · serial FAKE2 · {dev}"));
    }

    /// `device flash --yes` on the fake, the image `version` its firmware:
    /// the exit code, stdout, stderr, and the fake for its trace.
    fn flashed(spec: &str, version: &str, port: Option<&str>, mode: Mode) -> (i32, String, String, Fake) {
        flashed_with(spec, version, None, port, mode)
    }

    /// …with the flags' image (`--release`, `--firmware`, `--flash-mode`).
    fn flashed_with(
        spec: &str,
        version: &str,
        preset: Option<Image>,
        port: Option<&str>,
        mode: Mode,
    ) -> (i32, String, String, Fake) {
        let fake = Fake::new(spec).with_pace(Duration::from_millis(60));
        let setup = desk::Setup {
            supply: Arc::new(Shelf::new(version)),
            preset,
            port: port.map(str::to_string),
            timing: QUICK,
            firmware_first: true,
        };
        let (cmds, events) = desk::spawn(Arc::new(fake.clone()), setup);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = lines_on(mode, &cmds, &events, &mut out, &mut err);
        let _ = cmds.send(Cmd::Quit);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap(), fake)
    }

    const ONE: Mode = Mode { all: false, named: false, erase: None };

    #[test]
    fn flash_yes_on_one_board_prints_todays_steps() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, err, _) = flashed("fresh", "v0.8.0", None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "getting the firmware…");
        assert!(lines[1].starts_with("firmware: v0.8.0 in QIO (release v0.8.0"), "{out}");
        assert_eq!(lines[2], "looking for a Core2…");
        assert!(out.contains("reaching the board's bootloader…\n"), "{out}");
        assert!(out.contains("board: FAKE0 · CH9102 · esp32 rev 3.1 · 16 MB\n"), "{out}");
        assert!(out.contains("plan: install, erase first · v0.8.0 in QIO\n"), "the pin, in its default build:\n{out}");
        assert!(out.contains("  50%\n") && out.contains("  100%\n"), "{out}");
        let done = "v0.8.0 is on the board. It is restarting — unplug it when its screen comes up.\n\
                    The board reports: mstream-mp3-player v0.8.0 (commit fake000, 2026-10-01), ELF e127a6bf";
        assert!(out.trim_end().ends_with(done), "Done, then the board's own first line:\n{out}");
        assert!(err.is_empty(), "{err}");

        let (code, out, err, fake) = flashed("busy", "v0.8.0", None, ONE);
        assert_eq!(code, 1, "{out}");
        assert!(err.contains("FAKE0 is in use by another program") && err.contains("serial monitor"), "{err}");
        assert!(!fake.trace().lock().unwrap().iter().any(|t| t.starts_with("open")), "nothing tried");
    }

    #[test]
    fn flash_yes_with_several_boards_refuses_and_names_each_boards_firmware() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, _, fake) = flashed("old:v0.7.0,status:v0.8.0", "v0.8.0", None, ONE);
        assert_eq!(code, 1);
        let tail: Vec<&str> = out.lines().skip_while(|l| !l.starts_with("Several")).collect();
        assert_eq!(
            tail,
            [
                "Several boards look like a Core2 — which one?",
                "  FAKE0 · CH9102 · serial FAKE0 · v0.7.0, update to v0.8.0 · card not reported by v0.7.0",
                "  FAKE1 · CH9102 · serial FAKE1 · v0.8.0, up to date · card 38.2 GB free of 63.9 GB, 1,284 tracks",
                "pass --port with one of them, or --all to update every one that needs it",
            ]
        );
        assert!(!fake.trace().lock().unwrap().iter().any(|t| t.starts_with("open")), "no board reset");

        let named = Mode { named: true, ..ONE };
        let (code, out, _, fake) = flashed("old:v0.7.0,status:v0.8.0", "v0.8.0", Some("FAKE0"), named);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plan: update, no erase"), "{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.8.0"));
    }

    #[test]
    fn flash_yes_all_updates_each_board_that_needs_it_and_names_the_rest() {
        let _en = crate::setup::tests::in_locale("en");
        let all = Mode { all: true, ..ONE };
        let spec = "old:v0.7.0,status:v0.8.0,old:v0.6.0,silent";
        let (code, out, err, fake) = flashed(spec, "v0.8.0", None, all);
        assert_eq!(code, 0, "{out}{err}");
        let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle:?} in:\n{out}"));
        assert!(at("FAKE0 · v0.7.0 → v0.8.0\n") < at("FAKE2 · v0.6.0 → v0.8.0\n"), "one after another");
        assert!(out.contains("  reaching the board's bootloader…\n"), "the steps under each port:\n{out}");
        assert_eq!(out.matches("  done: v0.8.0 written and checked in ").count(), 2, "{out}");
        let summary = "updated 2 of 2 · skipped: FAKE1 (up to date), FAKE3 (not answering)";
        assert!(out.trim_end().ends_with(summary), "{out}");
        assert_eq!(fake.version_on("FAKE3").as_deref(), Some("v0.7.0"), "not answering: left as it was");

        let (code, out, err, fake) = flashed("old:v0.7.0/fail=write,old:v0.6.0", "v0.8.0", None, all);
        assert_eq!(code, 1, "{out}");
        assert!(err.contains("the board stopped answering"), "{err}");
        assert!(out.trim_end().ends_with("updated 0 of 2"), "{out}");
        assert_eq!(fake.version_on("FAKE1").as_deref(), Some("v0.6.0"), "never started");

        let (code, out, _, _) = flashed("status:v0.8.0", "v0.8.0", None, all);
        assert_eq!(code, 0, "none needed one");
        assert!(out.trim_end().ends_with("updated 0 of 0 · skipped: FAKE0 (up to date)"), "{out}");
    }

    #[test]
    fn flash_yes_on_a_board_that_runs_dio_writes_the_pins_dio_build() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, err, fake) = flashed("old:v0.7.0/mode=dio", "v0.8.0", None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains("firmware: v0.8.0 in DIO (release v0.8.0"), "its own build fetched:\n{out}");
        assert!(out.contains("plan: update, no erase · v0.8.0 in DIO\n"), "{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.8.0"));
        let all = Mode { all: true, ..ONE };
        let (code, out, _, _) = flashed("old:v0.7.0,old:v0.6.0/mode=dio", "v0.8.0", None, all);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("FAKE0 · v0.7.0 → v0.8.0\n") && out.contains("FAKE1 · v0.6.0 → v0.8.0 · DIO\n"), "{out}");
    }

    #[test]
    fn flash_yes_on_a_board_that_keeps_restarting_says_so_and_names_the_dio_build() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, err, fake) = flashed("old:v0.7.0/loop=qio", "v0.8.0", None, ONE);
        assert_eq!(code, 1, "written and checked is not running:\n{out}{err}");
        assert!(out.contains("v0.8.0 is on the board."), "{out}");
        let line = "mstream-player: the board keeps restarting: 3 restarts in 6 s, and never its first line \
                    — its flash may not run QIO; --flash-mode dio writes the DIO build\n";
        assert_eq!(err, line);
        assert_eq!(fake.trace().lock().unwrap().iter().filter(|t| t.starts_with("open")).count(), 1, "nothing more");
    }

    #[test]
    fn flash_yes_with_another_release_says_what_it_writes_and_still_measures_against_the_pin() {
        let _en = crate::setup::tests::in_locale("en");
        let release = Some(Image::release("v0.7.0", Build::Qio));
        let (code, out, err, fake) = flashed_with("old:v0.6.0", "v0.8.0", release.clone(), None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        let plan = "plan: update, no erase · v0.7.0 in QIO, release v0.7.0 — not this player's release (v0.8.0)\n";
        assert!(out.contains(plan), "{out}");
        assert!(out.contains("v0.7.0 is on the board."), "{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.7.0"));
        // Over the pin itself, the release is a step back, and says so.
        let (code, out, _, _) = flashed_with("old:v0.8.0", "v0.8.0", release, None, ONE);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plan: go back, no erase · v0.7.0 in QIO, release v0.7.0"), "{out}");
    }

    #[test]
    fn a_flash_mode_that_disagrees_with_the_files_header_is_refused_before_anything_starts() {
        let _en = crate::setup::tests::in_locale("en");
        let scratch = Scratch::new("mod-mode");
        let dio = scratch.0.join("dio-full.bin");
        std::fs::write(&dio, merged_in(&desc_bytes("v0.8.0", AppDesc::OURS), Build::Dio)).unwrap();
        let args = |firmware: &std::path::Path, mode: Option<Build>| FlashArgs {
            firmware: Some(firmware.to_path_buf()),
            release: None,
            flash_mode: mode,
            port: None,
            erase: false,
            no_erase: false,
            yes: true,
            all: false,
            server: None,
        };
        let refused = args(&dio, Some(Build::Qio)).mode_refused().expect("refused").text();
        assert!(refused.contains("is the DIO build") && refused.contains("--flash-mode qio"), "{refused}");
        assert_eq!(args(&dio, Some(Build::Dio)).mode_refused(), None, "the flag agrees with the header");
        assert_eq!(args(&dio, None).mode_refused(), None, "no flag: the file's own mode");
        let mut app = vec![0u8; AppDesc::OFFSET_IN_APP];
        app[0] = 0xE9;
        app.extend_from_slice(&desc_bytes("v0.8.0", AppDesc::OURS));
        let alone = scratch.0.join("firmware.bin");
        std::fs::write(&alone, &app).unwrap();
        assert!(args(&alone, Some(Build::Dio)).mode_refused().is_some(), "an app alone keeps the board's mode");
        let missing = scratch.0.join("nope.bin");
        assert_eq!(args(&missing, Some(Build::Dio)).mode_refused(), None, "unreadable: the worker says so, as always");
    }

    #[test]
    fn device_list_says_a_board_runs_dio_and_says_nothing_of_qio() {
        let _en = crate::setup::tests::in_locale("en");
        let (_, lines) = listed(&Fake::new("old:v0.7.0/mode=dio,old:v0.7.0"), false);
        assert_eq!(lines[0], "FAKE0 · CH9102 · serial FAKE0 · v0.7.0 · DIO, update to v0.8.0 · card not reported by v0.7.0");
        assert_eq!(lines[1], "FAKE1 · CH9102 · serial FAKE1 · v0.7.0, update to v0.8.0 · card not reported by v0.7.0");
    }

    #[test]
    fn device_releases_lists_newest_first_with_what_this_computer_has_and_hides_the_pre_release() {
        let mut shelf = Shelf::new("v0.8.0");
        shelf.cached = vec![
            Cached { tag: "v0.8.0".into(), modes: vec![Build::Qio], since: None },
            Cached { tag: "v0.5.0".into(), modes: vec![Build::Dio], since: None },
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(releases_on(&shelf, false, &mut out, &mut err), 0);
        assert!(err.is_empty());
        let out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "v0.8.0          2026-10-10  QIO and DIO · this player's release · on this computer: QIO",
                "v0.7.0          2026-10-07  QIO and DIO",
                "v0.6.0          2026-10-02  QIO and DIO",
                "v0.5.0          2026-10-01  DIO only · on this computer: DIO",
                "(1 pre-release hidden: --pre-releases lists it)",
            ]
        );
        let mut out = Vec::new();
        assert_eq!(releases_on(&shelf, true, &mut out, &mut err), 0);
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().last(), Some("v0.5.0-beta.1   2026-10-01  DIO only · pre-release"));
        assert_eq!(shelf.asked.load(std::sync::atomic::Ordering::Relaxed), 2, "one request a list");
    }

    #[test]
    fn device_releases_without_github_says_why_and_names_what_can_be_written_without_it() {
        let _en = crate::setup::tests::in_locale("en");
        let mut shelf = Shelf::new("v0.8.0");
        *shelf.list.lock().unwrap() = Err(firmware::ListWhy::Limited { reset: None });
        shelf.cached = vec![Cached { tag: "v0.7.0".into(), modes: vec![Build::Qio, Build::Dio], since: None }];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(releases_on(&shelf, false, &mut out, &mut err), 1);
        let err = String::from_utf8(err).unwrap();
        assert_eq!(err, "mstream-player: GitHub: too many lists from this address; try again later\n");
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out, "on this computer, written without the network:\n  v0.7.0 · QIO and DIO\n");
    }

    #[test]
    fn flash_yes_with_no_board_says_so_with_the_ports_it_saw() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, _, _) = flashed("nodevice", "v0.8.0", None, ONE);
        assert_eq!(code, 1);
        assert!(out.contains("No Core2 found\n") && out.contains("Serial ports seen: FAKECOM1"), "{out}");
    }
}
