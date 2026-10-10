//! The write, start to finish, on a worker thread: the firmware found, the
//! board found and reached, what is on it read, a plan made — then, once
//! the page (or `--yes`) says go, the erase, the write, the restart. The
//! thread reports every step through a channel and takes its one decision
//! from another, so the page keeps drawing while the board is busy and
//! can leave at any point before the write begins (the board is restarted
//! on the way out, never left in its bootloader).
//!
//! Leaving is heard as early as the board allows: a Quit, or the page's
//! end of the channel dropped, stops the worker before it opens a port,
//! between the baud ladder's rungs, and as soon as the board is reached;
//! a board it holds is restarted and its port let go at once, with no
//! listen for a boot line, so the next worker finds the port free. Once
//! Go has arrived nothing is heard: an erase or a write is never cut.
//!
//! Beside the steps it reports the details the page's busy line never
//! says — which baud answered, what the descriptor held, how many chunks
//! a segment takes — as `Event::Log` lines, for the page's log.
//!
//! The whole run is today's page's. The desk (desk.rs), which the MP3
//! Player tab's redesign and `--yes` run on, reuses its pieces — the
//! plan, the baud ladder ([`open`]) and the write with its retry
//! ([`write`]) — with a callback where this worker has its channel.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use rust_i18n::t;

use super::DeviceError;
use super::engine::{BAUDS, DeviceInfo, Engine, Link, Report};
use super::firmware::{AppDesc, Firmware, Origin, Source};
use super::ports::{self, Candidate, Pick};

/// What the page tells the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Several boards: this port.
    Pick(String),
    /// No board, or several: look again.
    Rescan,
    /// Write, erasing the whole flash first or not.
    Go { erase: bool },
    /// Leave. A board held in its bootloader is restarted first; a board
    /// not reached yet is never opened.
    Quit,
}

/// What the worker tells the page, in the order things happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Phase(Phase),
    /// A download's bytes so far, and its total when the server said.
    Download { done: u64, total: Option<u64> },
    Firmware { version: String, origin: String, bytes: usize, kind: Origin },
    /// Nothing Core2-shaped; `others` are the serial ports that are there
    /// (the usual answer to "is the driver installed?").
    NoDevice { others: Vec<String> },
    Several(Vec<Candidate>),
    Board(DeviceInfo),
    /// The board read; the worker now waits for `Cmd::Go` or `Cmd::Quit`.
    Probed { on_board: Option<AppDesc>, plan: Plan },
    Progress(u8),
    Done { version: String, skipped: bool, boot: Option<String> },
    /// Quit answered once the board was being reached: it was restarted
    /// untouched, or never opened, and its port is free.
    Cancelled,
    Failed(DeviceError),
    /// A detail for the page's log, drawn nowhere else.
    Log(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Firmware,
    Scanning,
    Connecting,
    Reading,
    Erasing,
    Comparing,
    Writing,
    Verifying,
    Restarting,
}

impl Phase {
    /// The busy line's words.
    pub fn text(self) -> String {
        let key = match self {
            Phase::Firmware => "dev.phase_firmware",
            Phase::Scanning => "dev.phase_scanning",
            Phase::Connecting => "dev.phase_connecting",
            Phase::Reading => "dev.phase_reading",
            Phase::Erasing => "dev.phase_erasing",
            Phase::Comparing => "dev.phase_comparing",
            Phase::Writing => "dev.phase_writing",
            Phase::Verifying => "dev.phase_verifying",
            Phase::Restarting => "dev.phase_restarting",
        };
        t!(key).to_string()
    }
}

/// What the write will be, from what is on the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Nothing of ours on the board: a blank one, or other firmware.
    Install { found: Option<String> },
    /// Our firmware, another version.
    Update { from: String },
    /// Our firmware, this very version — allowed, and a no-op on the wire
    /// (the engine skips bytes the board already holds).
    Same,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub kind: Kind,
    /// Erase the whole flash first: asked for, or what the kind implies.
    pub erase: bool,
}

impl Plan {
    /// `install, erase first` — one line for `--yes` and the log.
    pub fn describe(&self) -> String {
        let kind = match &self.kind {
            Kind::Install { .. } => "install",
            Kind::Update { .. } => "update",
            Kind::Same => "same version again",
        };
        format!("{kind}, {}", if self.erase { "erase first" } else { "no erase" })
    }

    /// The primary button's label for this plan.
    pub fn verb(&self) -> String {
        match self.kind {
            Kind::Install { .. } => t!("dev.install").to_string(),
            Kind::Update { .. } => t!("dev.update").to_string(),
            Kind::Same => t!("dev.again").to_string(),
        }
    }
}

/// The plan for `target` over `on_board`. The erase default is the one
/// rule that matters: never over our own firmware (the settings above the
/// image survive an update), always over anything else (its leftovers sit
/// where the settings and the filesystem go) — unless the flags said.
pub(crate) fn plan(on_board: Option<&AppDesc>, target: &str, asked: Option<bool>) -> Plan {
    let kind = match on_board {
        Some(desc) if desc.is_ours() && desc.version == target => Kind::Same,
        Some(desc) if desc.is_ours() => Kind::Update { from: desc.version.clone() },
        Some(desc) => Kind::Install { found: Some(desc.project.clone()) },
        None => Kind::Install { found: None },
    };
    let erase = asked.unwrap_or(matches!(kind, Kind::Install { .. }));
    Plan { kind, erase }
}

/// The "On the board" line.
pub(crate) fn on_board_text(on_board: Option<&AppDesc>) -> String {
    match on_board {
        Some(desc) if desc.is_ours() => t!("dev.on_board_ours", version = desc.version).to_string(),
        Some(desc) => t!("dev.on_board_other", name = desc.project).to_string(),
        None => t!("dev.on_board_unknown").to_string(),
    }
}

/// `1,118` — a count with its thousands marked, for the log.
pub(crate) fn grouped(n: usize) -> String {
    crate::admin::fmt_count(n as u64)
}

/// How the page gets its engine: a factory, so a retry gets a fresh one
/// and the tests get the fake they want.
pub(crate) type EngineFactory = Box<dyn Fn() -> Box<dyn Engine> + Send>;

/// Start the worker. The page keeps the two channel ends; the worker ends
/// when it has reported Done, Cancelled or Failed, or when the page drops
/// its end.
pub(crate) fn spawn(
    make: EngineFactory,
    source: Source,
    port: Option<String>,
    erase_asked: Option<bool>,
) -> (Sender<Cmd>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = channel();
    let (event_tx, event_rx) = channel();
    std::thread::Builder::new()
        .name("mstream-device".to_string())
        .spawn(move || run(&*make(), &source, port.as_deref(), erase_asked, &cmd_rx, &event_tx))
        .expect("spawn the device worker");
    (cmd_tx, event_rx)
}

/// The worker's whole story. Every `send` may fail once the page is gone;
/// the next `recv` then fails too, and the thread ends.
pub(crate) fn run(
    engine: &dyn Engine,
    source: &Source,
    port: Option<&str>,
    erase_asked: Option<bool>,
    cmds: &Receiver<Cmd>,
    events: &Sender<Event>,
) {
    let tell = |event: Event| events.send(event).is_ok();

    // 1. The firmware — before the board, so a missing file or a dead
    //    download never resets a board for nothing.
    if !tell(Event::Phase(Phase::Firmware)) {
        return;
    }
    let firmware = match source.resolve(&mut |done, total| {
        let _ = events.send(Event::Download { done, total });
    }) {
        Ok(firmware) => firmware,
        Err(e) => {
            tell(Event::Failed(e));
            return;
        }
    };
    tell(Event::Firmware {
        version: firmware.version.clone(),
        origin: firmware.origin.clone(),
        bytes: firmware.bytes(),
        kind: firmware.kind,
    });

    // 2. The board.
    let Some(candidate) = find_board(engine, port, cmds, events) else {
        return;
    };

    // 3. Reach it, read it, plan. The step is told BEFORE the last look for
    //    a Quit, and the port opened only after it: a page that sent Quit
    //    and has not heard "reaching" since knows the port will never be
    //    opened, and one that has heard it waits for Cancelled — the GUI's
    //    tab leans on this when it is left (mp3-player-screen contract,
    //    clause 8).
    if !tell(Event::Phase(Phase::Connecting)) {
        return;
    }
    if quit_asked(cmds) {
        tell(Event::Cancelled);
        return;
    }
    let send = |event: Event| {
        let _ = events.send(event);
    };
    let quit = || quit_asked(cmds);
    let mut link = match open(engine, &candidate, None, Some(&quit), &send) {
        Ok(link) => link,
        Err(Stop::Quit) => {
            tell(Event::Cancelled);
            return;
        }
        Err(Stop::Failed(e)) => {
            tell(Event::Failed(e));
            return;
        }
    };
    // A Quit that came while espflash was reaching the board: let it go
    // now rather than after the read.
    if quit_asked(cmds) {
        link.let_go();
        tell(Event::Cancelled);
        return;
    }
    tell(Event::Board(link.info().clone()));
    tell(Event::Phase(Phase::Reading));
    let on_board = match link.app_desc() {
        Ok(desc) => desc,
        Err(e) => {
            link.let_go();
            tell(Event::Failed(e));
            return;
        }
    };
    tell(Event::Log(descriptor_line(on_board.as_ref())));
    let plan = plan(on_board.as_ref(), &firmware.version, erase_asked);
    if !tell(Event::Probed { on_board, plan }) {
        link.let_go();
        return;
    }

    // 4. The one decision. A watch's Rescan or a Pick the page sent before
    //    it heard the board was reached is no answer, and is passed over.
    let erase = loop {
        match cmds.recv() {
            Ok(Cmd::Go { erase }) => break erase,
            Ok(Cmd::Rescan | Cmd::Pick(_)) => continue,
            Ok(Cmd::Quit) | Err(_) => {
                link.let_go();
                tell(Event::Cancelled);
                return;
            }
        }
    };

    // 5. Erase, write, restart.
    if erase {
        tell(Event::Phase(Phase::Erasing));
        if let Err(e) = link.erase() {
            tell(Event::Failed(e));
            return;
        }
    }
    let (link, skipped) = match write(engine, &candidate, link, &firmware, &send) {
        Ok(done) => done,
        Err(e) => {
            tell(Event::Failed(e));
            return;
        }
    };
    tell(Event::Log(t!(if skipped { "dev.log_same" } else { "dev.log_checked" }).to_string()));
    tell(Event::Phase(Phase::Restarting));
    tell(Event::Log(t!("dev.log_restart", secs = super::engine::BOOT_LISTEN.as_secs()).to_string()));
    match link.restart() {
        Ok(boot) => {
            tell(Event::Done { version: firmware.version, skipped, boot });
        }
        Err(e) => {
            tell(Event::Failed(e));
        }
    }
}

/// The descriptor as the log says it: the address it was read from, then
/// what it held — a firmware's name and version (ours or not), or nothing
/// readable.
pub(crate) fn descriptor_line(on_board: Option<&AppDesc>) -> String {
    let at = format!("{:X}", super::firmware::APP_OFFSET + AppDesc::OFFSET_IN_APP);
    match on_board {
        Some(desc) => {
            let mut what = format!("{} {}", desc.project, desc.version);
            if !desc.idf.is_empty() {
                what.push_str(&format!(" · idf {}", desc.idf));
            }
            if !desc.elf8.is_empty() {
                what.push_str(&format!(" · elf {}", desc.elf8));
            }
            t!("dev.log_desc", at = at, desc = what).to_string()
        }
        None => t!("dev.log_desc_none", at = at).to_string(),
    }
}

/// The board to write, asking the page when there is none or several.
/// The page asks again every couple of seconds while it waits, so a
/// board plugged in (or the wrong one unplugged) is picked up by itself.
fn find_board(
    engine: &dyn Engine,
    port: Option<&str>,
    cmds: &Receiver<Cmd>,
    events: &Sender<Event>,
) -> Option<Candidate> {
    loop {
        if events.send(Event::Phase(Phase::Scanning)).is_err() {
            return None;
        }
        let found = match engine.candidates() {
            Ok(found) => found,
            Err(e) => {
                let _ = events.send(Event::Failed(e));
                return None;
            }
        };
        match ports::pick(&found, port) {
            Pick::One(candidate) => {
                // A port named by hand may not be a Core2's bridge at all;
                // the log says which it was.
                let line = if candidate.bridge == "?" {
                    t!("dev.log_named_port", port = candidate.port).to_string()
                } else {
                    t!("dev.log_one_board", board = candidate.describe()).to_string()
                };
                let _ = events.send(Event::Log(line));
                return Some(candidate);
            }
            Pick::None => {
                let _ = events.send(Event::NoDevice { others: engine.others() });
                match cmds.recv() {
                    Ok(Cmd::Rescan) => continue,
                    _ => return None,
                }
            }
            Pick::Several => {
                let _ = events.send(Event::Several(found.clone()));
                match cmds.recv() {
                    Ok(Cmd::Pick(name)) => match found.into_iter().find(|c| c.port == name) {
                        Some(candidate) => return Some(candidate),
                        None => continue,
                    },
                    Ok(Cmd::Rescan) => continue,
                    _ => return None,
                }
            }
        }
    }
}

/// Whether the page has asked the worker to stop — a Quit, or its end of
/// the channel dropped — among the commands waiting, without waiting for
/// one. Asked only before the write, where any other command is stale (a
/// watch's Rescan, a Pick the worker no longer needs) and is dropped.
fn quit_asked(cmds: &Receiver<Cmd>) -> bool {
    loop {
        match cmds.try_recv() {
            Ok(Cmd::Quit) | Err(TryRecvError::Disconnected) => return true,
            Ok(_) => continue,
            Err(TryRecvError::Empty) => return false,
        }
    }
}

/// Why reaching the board stopped short of a link.
pub(crate) enum Stop {
    /// The page asked to leave between the ladder's rungs; the board was
    /// let go.
    Quit,
    Failed(DeviceError),
}

/// Reach the bootloader at the fastest baud the link holds: a failure to
/// sync at one speed (a bridge or a cable that cannot keep it) tries the
/// next one down; a port that is busy, forbidden or gone is final. `below`
/// starts the ladder under a speed that already failed mid-write. Every
/// rung goes to the log — the one place the ladder is ever visible. With
/// `quit`, before the write, a Quit is heard after each rung that failed:
/// that rung reset the board into its bootloader, so it is let go before
/// the worker stops. The write's own retry passes none — nothing cuts it.
/// `tell` hears the log lines: this worker's channel, or the desk's.
pub(crate) fn open(
    engine: &dyn Engine,
    candidate: &Candidate,
    below: Option<u32>,
    quit: Option<&dyn Fn() -> bool>,
    tell: &dyn Fn(Event),
) -> Result<Box<dyn Link>, Stop> {
    let mut last = None;
    for baud in BAUDS.iter().copied().filter(|b| below.is_none_or(|limit| *b < limit)) {
        match engine.open(candidate, baud) {
            Ok(link) => {
                tell(Event::Log(t!("dev.log_baud_ok", baud = baud).to_string()));
                return Ok(link);
            }
            Err(e @ DeviceError::NoSync { .. }) => {
                let detail = match &e {
                    DeviceError::NoSync { detail, .. } => detail.clone(),
                    _ => String::new(),
                };
                let line = t!("dev.log_baud_no", baud = baud, err = detail).to_string();
                tell(Event::Log(line));
                last = Some(e);
                if quit.is_some_and(|asked| asked()) {
                    engine.let_go(candidate);
                    return Err(Stop::Quit);
                }
            }
            Err(e) => return Err(Stop::Failed(e)),
        }
    }
    Err(Stop::Failed(last.unwrap_or_else(|| DeviceError::NoSync {
        port: candidate.port.clone(),
        detail: t!("dev.err_no_slower").to_string(),
    })))
}

/// The write, retried one baud step down when it fails on the wire — a
/// fresh link each time, since the failed one may be anywhere. Returns
/// the link that wrote (for the restart) and whether nothing needed
/// writing. `tell` hears the phases, the percent and the log lines.
pub(crate) fn write(
    engine: &dyn Engine,
    candidate: &Candidate,
    mut link: Box<dyn Link>,
    firmware: &Firmware,
    tell: &dyn Fn(Event),
) -> Result<(Box<dyn Link>, bool), DeviceError> {
    let list: Vec<String> = firmware
        .segments
        .iter()
        .map(|s| format!("0x{:X} · {} B", s.offset, grouped(s.data.len())))
        .collect();
    tell(Event::Log(t!("dev.log_segments", list = list.join("; ")).to_string()));
    loop {
        tell(Event::Phase(Phase::Comparing));
        let mut phase = Phase::Comparing;
        let result = link.write(&firmware.segments, &mut |report| {
            let next = match report {
                Report::Percent(_) | Report::Chunks { .. } => Phase::Writing,
                Report::Verifying => Phase::Verifying,
            };
            if next != phase {
                phase = next;
                tell(Event::Phase(next));
            }
            match report {
                Report::Percent(pct) => tell(Event::Progress(pct)),
                Report::Chunks { addr, chunks } => {
                    let line = t!("dev.log_chunks", at = format!("{addr:X}"), n = grouped(chunks));
                    tell(Event::Log(line.to_string()));
                }
                Report::Verifying => {}
            }
        });
        match result {
            Ok(skipped) => return Ok((link, skipped)),
            Err(e @ DeviceError::Link(_)) if link.info().baud > BAUDS[BAUDS.len() - 1] => {
                let failed_at = link.info().baud;
                let next = BAUDS.iter().copied().find(|b| *b < failed_at).unwrap_or(BAUDS[BAUDS.len() - 1]);
                let line = t!("dev.log_retry", baud = failed_at, next = next).to_string();
                tell(Event::Log(line));
                drop(link);
                tell(Event::Phase(Phase::Connecting));
                link = open(engine, candidate, Some(failed_at), None, tell).map_err(|_| e)?;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::device::engine::fake::Fake;
    use crate::device::firmware::tests::{desc_bytes, merged_bytes};

    fn ours(version: &str) -> AppDesc {
        AppDesc { version: version.into(), project: AppDesc::OURS.into(), idf: "v5.5.5".into(), elf8: "00".into() }
    }

    #[test]
    fn the_plan_erases_over_strangers_and_never_over_our_own_unless_told() {
        let _en = crate::setup::tests::in_locale("en");
        let blank = plan(None, "v0.5.0", None);
        assert_eq!(blank, Plan { kind: Kind::Install { found: None }, erase: true });
        let other = AppDesc {
            version: "3.3.12".into(),
            project: "arduino-lib-builder".into(),
            idf: "".into(),
            elf8: "".into(),
        };
        let over_other = plan(Some(&other), "v0.5.0", None);
        assert_eq!(over_other.kind, Kind::Install { found: Some("arduino-lib-builder".into()) });
        assert!(over_other.erase);
        let older = plan(Some(&ours("v0.4.0")), "v0.5.0", None);
        assert_eq!(older, Plan { kind: Kind::Update { from: "v0.4.0".into() }, erase: false });
        let same = plan(Some(&ours("v0.5.0")), "v0.5.0", None);
        assert_eq!(same.kind, Kind::Same);
        assert!(!same.erase);
        assert!(plan(Some(&ours("v0.4.0")), "v0.5.0", Some(true)).erase, "--erase wins");
        assert!(!plan(None, "v0.5.0", Some(false)).erase, "--no-erase wins");
        assert_eq!(older.describe(), "update, no erase");
        assert_eq!(blank.describe(), "install, erase first");
        assert_eq!(older.verb(), "Update ▸");
        assert_eq!(on_board_text(Some(&ours("v0.4.0"))), "mstream-mp3-player v0.4.0");
        assert!(on_board_text(None).contains("nothing"));
    }

    #[test]
    fn the_descriptor_line_names_the_address_and_what_it_held() {
        let _en = crate::setup::tests::in_locale("en");
        let line = descriptor_line(Some(&ours("v0.4.0")));
        assert_eq!(line, "0x10020: mstream-mp3-player v0.4.0 · idf v5.5.5 · elf 00");
        assert_eq!(descriptor_line(None), "0x10020: nothing readable");
        assert_eq!(grouped(2_289_360), "2,289,360");
    }

    /// A worker on the fake, driven to the end: every report in order.
    fn drive(spec: &str, source: Source, port: Option<&str>, answer: Cmd) -> Vec<Event> {
        drive_fake(Fake::new(spec).with_pace(Duration::from_millis(30)), source, port, answer)
    }

    /// `drive`, on a fake the test built (to read its trace after).
    fn drive_fake(fake: Fake, source: Source, port: Option<&str>, answer: Cmd) -> Vec<Event> {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let port = port.map(str::to_string);
        std::thread::spawn(move || run(&fake, &source, port.as_deref(), None, &cmd_rx, &event_tx));
        let mut seen = Vec::new();
        for event in event_rx.iter() {
            let answer_now = matches!(event, Event::Probed { .. });
            let several = matches!(event, Event::Several(_));
            seen.push(event);
            if answer_now {
                cmd_tx.send(answer.clone()).unwrap();
            }
            if several {
                cmd_tx.send(Cmd::Pick("FAKE1".into())).unwrap();
            }
        }
        seen
    }

    /// A merged image in miniature, named per TEST: the tests run in
    /// parallel and each removes its own file at the end.
    fn image_file(test: &str, version: &str) -> std::path::PathBuf {
        let name = format!("mstream-player-flow-{}-{test}-{version}.bin", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, merged_bytes(&desc_bytes(version, AppDesc::OURS))).unwrap();
        path
    }

    fn logs(seen: &[Event]) -> Vec<&str> {
        seen.iter()
            .filter_map(|e| match e {
                Event::Log(line) => Some(line.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_write_runs_firmware_board_probe_go_write_restart() {
        let _en = crate::setup::tests::in_locale("en");
        let image = image_file("write", "v0.5.0");
        let fake = Fake::new("ours:v0.4.0").with_pace(Duration::from_millis(30));
        let trace = fake.trace();
        let seen = drive_fake(fake, Source::Local(image.clone()), None, Cmd::Go { erase: false });
        let _ = std::fs::remove_file(&image);
        assert_eq!(*trace.lock().unwrap(), ["open 921600", "restart"], "only a write's restart listens for the boot line");
        let phases: Vec<Phase> = seen
            .iter()
            .filter_map(|e| match e {
                Event::Phase(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(
            phases,
            [
                Phase::Firmware,
                Phase::Scanning,
                Phase::Connecting,
                Phase::Reading,
                Phase::Comparing,
                Phase::Writing,
                Phase::Verifying,
                Phase::Restarting,
            ]
        );
        assert!(seen.iter().any(|e| matches!(e, Event::Firmware { version, kind: Origin::File, .. } if version == "v0.5.0")));
        let update = Kind::Update { from: "v0.4.0".into() };
        assert!(seen.iter().any(|e| matches!(e, Event::Probed { plan, .. } if plan.kind == update)));
        assert!(seen.iter().any(|e| matches!(e, Event::Progress(100))));
        let Some(Event::Done { version, skipped: false, boot: Some(boot) }) = seen.last() else {
            panic!("the last report is Done, with the board's line: {:?}", seen.last());
        };
        assert_eq!(version, "v0.5.0");
        assert!(boot.contains("v0.5.0"), "{boot}");
        // The log's details, in order: the board, the baud, the
        // descriptor, the segments, the check, the restart.
        let log = logs(&seen);
        assert!(log[0].starts_with("1 port with a Core2's bridge: FAKE0"), "{log:?}");
        assert_eq!(log[1], "921600 baud: the bootloader answered");
        assert!(log[2].starts_with("0x10020: mstream-mp3-player v0.4.0"), "{log:?}");
        assert!(log[3].starts_with("to write: 0x0 · "), "{log:?}");
        assert_eq!(log[4], "written and checked — the checksum matches");
        assert!(log[5].starts_with("reset — listening"), "{log:?}");
    }

    #[test]
    fn quit_at_the_question_lets_the_board_go_without_a_listen_and_says_cancelled() {
        let image = image_file("quit", "v0.5.0");
        let fake = Fake::new("fresh").with_pace(Duration::from_millis(30));
        let trace = fake.trace();
        let seen = drive_fake(fake, Source::Local(image.clone()), None, Cmd::Quit);
        let _ = std::fs::remove_file(&image);
        assert!(seen.iter().any(|e| matches!(e, Event::Probed { on_board: None, plan } if plan.erase)));
        assert_eq!(seen.last(), Some(&Event::Cancelled));
        assert!(!seen.iter().any(|e| matches!(e, Event::Progress(_))), "nothing was written");
        assert_eq!(*trace.lock().unwrap(), ["open 921600", "let go"], "reset and the port free at once, no boot line awaited");
    }

    /// A worker on `fake` with its two far ends, run on a thread of its own;
    /// `before` is said to it before it starts.
    fn spawn_on(fake: Fake, image: &std::path::Path, before: Option<Cmd>) -> (Sender<Cmd>, Receiver<Event>) {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        if let Some(cmd) = before {
            cmd_tx.send(cmd).unwrap();
        }
        let source = Source::Local(image.to_path_buf());
        std::thread::spawn(move || run(&fake, &source, None, None, &cmd_rx, &event_tx));
        (cmd_tx, event_rx)
    }

    #[test]
    fn a_quit_before_the_board_is_reached_never_opens_its_port() {
        let image = image_file("early", "v0.5.0");
        // Quit already waiting when the board is found: told "reaching",
        // then Cancelled, and the port never touched.
        let fake = Fake::new("fresh");
        let trace = fake.trace();
        let (_cmds, events) = spawn_on(fake, &image, Some(Cmd::Quit));
        let seen: Vec<Event> = events.iter().collect();
        assert!(seen.ends_with(&[Event::Phase(Phase::Connecting), Event::Cancelled]), "{seen:?}");
        assert!(trace.lock().unwrap().is_empty(), "no port opened: {:?}", trace.lock().unwrap());

        // The page's end dropped instead: the same, with nobody to tell.
        let fake = Fake::new("fresh");
        let trace = fake.trace();
        let (cmds, events) = spawn_on(fake, &image, None);
        drop(cmds);
        let _: Vec<Event> = events.iter().collect();
        assert!(trace.lock().unwrap().is_empty(), "no port opened: {:?}", trace.lock().unwrap());
        let _ = std::fs::remove_file(&image);
    }

    /// Wait until the fake has begun its `open` — the port opened, espflash
    /// reaching the board — so a Quit sent now lands mid-connect.
    fn until_opened(trace: &std::sync::Mutex<Vec<String>>) {
        let t0 = std::time::Instant::now();
        while !trace.lock().unwrap().iter().any(|t| t.starts_with("open")) {
            assert!(t0.elapsed() < Duration::from_secs(5), "the fake was never opened");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_quit_while_the_board_is_being_reached_lets_it_go_as_soon_as_it_answers() {
        let _en = crate::setup::tests::in_locale("en");
        let image = image_file("reaching", "v0.5.0");
        let reach = Duration::from_millis(400);
        let fake = Fake::new("fresh").with_reach(reach);
        let trace = fake.trace();
        let (cmds, events) = spawn_on(fake, &image, None);
        let mut seen = Vec::new();
        let mut asked = None;
        for event in events.iter() {
            if event == Event::Phase(Phase::Connecting) {
                until_opened(&trace);
                cmds.send(Cmd::Quit).unwrap();
                asked = Some(std::time::Instant::now());
            }
            seen.push(event);
        }
        let took = asked.expect("the board was reached").elapsed();
        assert_eq!(seen.last(), Some(&Event::Cancelled), "{seen:?}");
        assert!(!seen.iter().any(|e| matches!(e, Event::Board(_) | Event::Probed { .. })), "nothing read: {seen:?}");
        assert_eq!(*trace.lock().unwrap(), ["open 921600", "let go"]);
        assert!(took < reach + Duration::from_secs(2), "let go once the board answered, no listen: {took:?}");
        let _ = std::fs::remove_file(&image);
    }

    #[test]
    fn a_quit_between_the_ladders_rungs_lets_the_reset_board_go() {
        let _en = crate::setup::tests::in_locale("en");
        let image = image_file("rungs", "v0.5.0");
        let fake = Fake::new("nosync").with_reach(Duration::from_millis(300));
        let trace = fake.trace();
        let (cmds, events) = spawn_on(fake, &image, None);
        let mut seen = Vec::new();
        for event in events.iter() {
            if event == Event::Phase(Phase::Connecting) {
                until_opened(&trace);
                cmds.send(Cmd::Quit).unwrap();
            }
            seen.push(event);
        }
        assert_eq!(seen.last(), Some(&Event::Cancelled), "{seen:?}");
        let rungs = logs(&seen).iter().filter(|l| l.contains("baud: no answer")).count();
        assert_eq!(rungs, 1, "no second rung: {:?}", logs(&seen));
        assert_eq!(*trace.lock().unwrap(), ["open 921600", "let go FAKE0"], "the board the rung reset is restarted");
        let _ = std::fs::remove_file(&image);
    }

    #[test]
    fn a_stale_rescan_at_the_question_is_no_answer() {
        let image = image_file("stale", "v0.5.0");
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let source = Source::Local(image.clone());
        let fake = Fake::new("ours:v0.4.0").with_pace(Duration::from_millis(30));
        std::thread::spawn(move || run(&fake, &source, None, None, &cmd_rx, &event_tx));
        let mut last = None;
        for event in event_rx.iter() {
            if matches!(event, Event::Probed { .. }) {
                // The watch's request, sent before the page heard the board.
                cmd_tx.send(Cmd::Rescan).unwrap();
                cmd_tx.send(Cmd::Go { erase: false }).unwrap();
            }
            last = Some(event);
        }
        let _ = std::fs::remove_file(&image);
        assert!(matches!(last, Some(Event::Done { .. })), "the Go after it is the answer: {last:?}");
    }

    #[test]
    fn several_boards_wait_for_a_pick_and_no_board_waits_for_a_rescan() {
        let image = image_file("several", "v0.5.0");
        let seen = drive("two", Source::Local(image.clone()), None, Cmd::Go { erase: true });
        assert!(seen.iter().any(|e| matches!(e, Event::Several(list) if list.len() == 2)));
        assert!(seen.iter().any(|e| matches!(e, Event::Board(info) if info.port == "FAKE1")), "the picked one");
        assert!(matches!(seen.last(), Some(Event::Done { .. })));

        let named = drive("two", Source::Local(image.clone()), Some("fake0"), Cmd::Go { erase: false });
        assert!(!named.iter().any(|e| matches!(e, Event::Several(_))), "--port skips the question");
        assert!(
            named.iter().any(|e| matches!(e, Event::Board(info) if info.port == "FAKE0")),
            "the listed board, found by its name whatever the case: {named:?}"
        );
        let _en = crate::setup::tests::in_locale("en");
        let bare = drive("nodevice", Source::Local(image.clone()), Some("COM9"), Cmd::Quit);
        assert!(
            logs(&bare).contains(&"the port named by hand: COM9"),
            "an unlisted --port is opened as named, and the log says so, not that it is a Core2's: {:?}",
            logs(&bare)
        );

        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let src = Source::Local(image.clone());
        std::thread::spawn(move || run(&Fake::new("nodevice"), &src, None, None, &cmd_rx, &event_tx));
        let first = event_rx.iter().find(|e| matches!(e, Event::NoDevice { .. })).unwrap();
        assert_eq!(first, Event::NoDevice { others: vec!["FAKECOM1".into()] }, "the other ports come along");
        cmd_tx.send(Cmd::Rescan).unwrap();
        assert!(event_rx.iter().any(|e| matches!(e, Event::NoDevice { .. })), "looked again, still nothing");
        drop(cmd_tx);
        assert!(event_rx.iter().next().is_none(), "the worker ends when the page is gone");
        let _ = std::fs::remove_file(&image);
    }

    #[test]
    fn a_firmware_that_cannot_be_read_fails_before_any_board_is_touched() {
        let _en = crate::setup::tests::in_locale("en");
        let seen = drive("ours", Source::Local("/nowhere/at/all.bin".into()), None, Cmd::Quit);
        assert!(matches!(seen.last(), Some(Event::Failed(DeviceError::Firmware(_)))));
        assert!(!seen.iter().any(|e| matches!(e, Event::Phase(Phase::Scanning))));
    }

    #[test]
    fn a_held_port_and_a_dying_write_are_reported_as_what_they_are() {
        let _en = crate::setup::tests::in_locale("en");
        let image = image_file("held", "v0.5.1");
        let busy = drive("busy", Source::Local(image.clone()), None, Cmd::Quit);
        assert!(matches!(busy.last(), Some(Event::Failed(DeviceError::Busy { .. }))));
        let dying = drive("failwrite", Source::Local(image.clone()), None, Cmd::Go { erase: false });
        assert!(matches!(dying.last(), Some(Event::Failed(DeviceError::Link(_)))));
        // The ladder's lower rungs were tried before giving up, and the log says so.
        let log = logs(&dying);
        assert!(log.iter().any(|l| l.starts_with("the link failed at 921600 baud — trying 460800")), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("the link failed at 460800 baud — trying 115200")), "{log:?}");
        let _ = std::fs::remove_file(&image);

        // A board that never answers: three rungs, three log lines, then the failure.
        let silent_image = image_file("silent", "v0.5.1");
        let silent = drive("nosync", Source::Local(silent_image.clone()), None, Cmd::Quit);
        let _ = std::fs::remove_file(&silent_image);
        assert!(matches!(silent.last(), Some(Event::Failed(DeviceError::NoSync { .. }))));
        let rungs = logs(&silent).iter().filter(|l| l.contains("baud: no answer")).count();
        assert_eq!(rungs, 3, "{:?}", logs(&silent));
    }
}
