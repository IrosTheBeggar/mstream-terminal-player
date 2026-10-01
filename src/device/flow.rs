//! The write, start to finish, on a worker thread: the firmware found, the
//! board found and reached, what is on it read, a plan made — then, once
//! the page (or `--yes`) says go, the erase, the write, the restart. The
//! thread reports every step through a channel and takes its one decision
//! from another, so the page keeps drawing while the board is busy and
//! can leave at any point before the write begins (the board is restarted
//! on the way out, never left in its bootloader).

use std::sync::mpsc::{Receiver, Sender, channel};

use rust_i18n::t;

use super::DeviceError;
use super::engine::{BAUDS, DeviceInfo, Engine, Link, Report};
use super::firmware::{AppDesc, Firmware, Source};
use super::ports::{self, Candidate, Pick};

/// What the page tells the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Several boards: this port.
    Pick(String),
    /// No board: look again.
    Rescan,
    /// Write, erasing the whole flash first or not.
    Go { erase: bool },
    /// Leave. A board held in its bootloader is restarted first.
    Quit,
}

/// What the worker tells the page, in the order things happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Phase(Phase),
    /// A download's bytes so far, and its total when the server said.
    Download { done: u64, total: Option<u64> },
    Firmware { version: String, origin: String, bytes: usize },
    NoDevice,
    Several(Vec<Candidate>),
    Board(DeviceInfo),
    /// The board read; the worker now waits for `Cmd::Go` or `Cmd::Quit`.
    Probed { on_board: Option<AppDesc>, plan: Plan },
    Progress(u8),
    Done { version: String, skipped: bool, boot: Option<String> },
    /// Quit answered while the board was held: it was restarted untouched.
    Cancelled,
    Failed(DeviceError),
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
    /// `install, erase first` — one line for `--yes`.
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
    });

    // 2. The board.
    let Some(candidate) = find_board(engine, port, cmds, events) else {
        return;
    };

    // 3. Reach it, read it, plan.
    tell(Event::Phase(Phase::Connecting));
    let mut link = match open(engine, &candidate, None) {
        Ok(link) => link,
        Err(e) => {
            tell(Event::Failed(e));
            return;
        }
    };
    tell(Event::Board(link.info().clone()));
    tell(Event::Phase(Phase::Reading));
    let on_board = match link.app_desc() {
        Ok(desc) => desc,
        Err(e) => {
            let _ = link.restart();
            tell(Event::Failed(e));
            return;
        }
    };
    let plan = plan(on_board.as_ref(), &firmware.version, erase_asked);
    if !tell(Event::Probed { on_board, plan }) {
        let _ = link.restart();
        return;
    }

    // 4. The one decision.
    let erase = match cmds.recv() {
        Ok(Cmd::Go { erase }) => erase,
        _ => {
            let _ = link.restart();
            tell(Event::Cancelled);
            return;
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
    let (link, skipped) = match write(engine, &candidate, link, &firmware, events) {
        Ok(done) => done,
        Err(e) => {
            tell(Event::Failed(e));
            return;
        }
    };
    tell(Event::Phase(Phase::Restarting));
    match link.restart() {
        Ok(boot) => {
            tell(Event::Done { version: firmware.version, skipped, boot });
        }
        Err(e) => {
            tell(Event::Failed(e));
        }
    }
}

/// The board to write, asking the page when there is none or several.
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
            Pick::One(candidate) => return Some(candidate),
            Pick::None => {
                let _ = events.send(Event::NoDevice);
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

/// Reach the bootloader at the fastest baud the link holds: a failure to
/// sync at one speed (a bridge or a cable that cannot keep it) tries the
/// next one down; a port that is busy, forbidden or gone is final. `below`
/// starts the ladder under a speed that already failed mid-write.
fn open(engine: &dyn Engine, candidate: &Candidate, below: Option<u32>) -> Result<Box<dyn Link>, DeviceError> {
    let mut last = None;
    for baud in BAUDS.iter().copied().filter(|b| below.is_none_or(|limit| *b < limit)) {
        match engine.open(candidate, baud) {
            Ok(link) => return Ok(link),
            Err(e @ DeviceError::NoSync { .. }) => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| DeviceError::NoSync {
        port: candidate.port.clone(),
        detail: t!("dev.err_no_slower").to_string(),
    }))
}

/// The write, retried one baud step down when it fails on the wire — a
/// fresh link each time, since the failed one may be anywhere. Returns
/// the link that wrote (for the restart) and whether nothing needed
/// writing.
fn write(
    engine: &dyn Engine,
    candidate: &Candidate,
    mut link: Box<dyn Link>,
    firmware: &Firmware,
    events: &Sender<Event>,
) -> Result<(Box<dyn Link>, bool), DeviceError> {
    loop {
        let _ = events.send(Event::Phase(Phase::Comparing));
        let mut phase = Phase::Comparing;
        let result = link.write(&firmware.segments, &mut |report| {
            let next = match report {
                Report::Percent(_) => Phase::Writing,
                Report::Verifying => Phase::Verifying,
            };
            if next != phase {
                phase = next;
                let _ = events.send(Event::Phase(next));
            }
            if let Report::Percent(pct) = report {
                let _ = events.send(Event::Progress(pct));
            }
        });
        match result {
            Ok(skipped) => return Ok((link, skipped)),
            Err(e @ DeviceError::Link(_)) if link.info().baud > BAUDS[BAUDS.len() - 1] => {
                let failed_at = link.info().baud;
                drop(link);
                let _ = events.send(Event::Phase(Phase::Connecting));
                link = open(engine, candidate, Some(failed_at)).map_err(|_| e)?;
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
        rust_i18n::set_locale("en");
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

    /// A worker on the fake, driven to the end: every report in order.
    fn drive(spec: &str, source: Source, port: Option<&str>, answer: Cmd) -> Vec<Event> {
        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let fake = Fake::new(spec).with_pace(Duration::from_millis(30));
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

    #[test]
    fn a_write_runs_firmware_board_probe_go_write_restart() {
        rust_i18n::set_locale("en");
        let image = image_file("write", "v0.5.0");
        let seen = drive("ours:v0.4.0", Source::Local(image.clone()), None, Cmd::Go { erase: false });
        let _ = std::fs::remove_file(&image);
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
        assert!(seen.iter().any(|e| matches!(e, Event::Firmware { version, .. } if version == "v0.5.0")));
        let update = Kind::Update { from: "v0.4.0".into() };
        assert!(seen.iter().any(|e| matches!(e, Event::Probed { plan, .. } if plan.kind == update)));
        assert!(seen.iter().any(|e| matches!(e, Event::Progress(100))));
        let Some(Event::Done { version, skipped: false, boot: Some(boot) }) = seen.last() else {
            panic!("the last report is Done, with the board's line: {:?}", seen.last());
        };
        assert_eq!(version, "v0.5.0");
        assert!(boot.contains("v0.5.0"), "{boot}");
    }

    #[test]
    fn quit_at_the_question_restarts_the_board_and_says_cancelled() {
        let image = image_file("quit", "v0.5.0");
        let seen = drive("fresh", Source::Local(image.clone()), None, Cmd::Quit);
        let _ = std::fs::remove_file(&image);
        assert!(seen.iter().any(|e| matches!(e, Event::Probed { on_board: None, plan } if plan.erase)));
        assert_eq!(seen.last(), Some(&Event::Cancelled));
        assert!(!seen.iter().any(|e| matches!(e, Event::Progress(_))), "nothing was written");
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

        let (cmd_tx, cmd_rx) = channel();
        let (event_tx, event_rx) = channel();
        let src = Source::Local(image.clone());
        std::thread::spawn(move || run(&Fake::new("nodevice"), &src, None, None, &cmd_rx, &event_tx));
        assert!(event_rx.iter().any(|e| e == Event::NoDevice));
        cmd_tx.send(Cmd::Rescan).unwrap();
        assert!(event_rx.iter().any(|e| e == Event::NoDevice), "looked again, still nothing");
        drop(cmd_tx);
        assert!(event_rx.iter().next().is_none(), "the worker ends when the page is gone");
        let _ = std::fs::remove_file(&image);
    }

    #[test]
    fn a_firmware_that_cannot_be_read_fails_before_any_board_is_touched() {
        rust_i18n::set_locale("en");
        let seen = drive("ours", Source::Local("/nowhere/at/all.bin".into()), None, Cmd::Quit);
        assert!(matches!(seen.last(), Some(Event::Failed(DeviceError::Firmware(_)))));
        assert!(!seen.iter().any(|e| matches!(e, Event::Phase(Phase::Scanning))));
    }

    #[test]
    fn a_held_port_and_a_dying_write_are_reported_as_what_they_are() {
        let image = image_file("held", "v0.5.1");
        let busy = drive("busy", Source::Local(image.clone()), None, Cmd::Quit);
        assert!(matches!(busy.last(), Some(Event::Failed(DeviceError::Busy { .. }))));
        let dying = drive("failwrite", Source::Local(image.clone()), None, Cmd::Go { erase: false });
        assert!(matches!(dying.last(), Some(Event::Failed(DeviceError::Link(_)))));
        let _ = std::fs::remove_file(&image);
    }
}
