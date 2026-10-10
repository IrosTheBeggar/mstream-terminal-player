//! The write's pieces, for the desk (desk.rs) to run on its jobs' threads:
//! the plan (what a write will be over what is on the board), the baud
//! ladder that reaches the bootloader ([`open`]), and the write with its
//! retry one step down ([`write`]). Each reports through a callback — the
//! phases, the percent, and the details the page's busy line never says
//! (which baud answered, what the descriptor held, how many chunks a
//! segment takes) as [`Event::Log`] lines for the log.
//!
//! The whole run this module once drove — the firmware, the board found
//! and held in its bootloader from the probe to the question, the write —
//! was the page's before the MP3 Player tab's redesign (mStream
//! docs/designs/mp3-tab, alternate A), which listens first and reaches the
//! bootloader only after the gate's yes: the desk does that now.

use rust_i18n::t;

use super::DeviceError;
use super::engine::{BAUDS, Engine, Link, Report};
use super::firmware::{AppDesc, Firmware};
use super::ports::Candidate;

/// What the pieces report as they go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Phase(Phase),
    Progress(u8),
    /// A detail for the log, drawn nowhere else.
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
/// `tell` hears the log lines: the desk's job hands them to its page.
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

    /// The pieces' reports, gathered.
    fn gather() -> (std::sync::Arc<std::sync::Mutex<Vec<Event>>>, impl Fn(Event)) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = seen.clone();
        (seen, move |event: Event| into.lock().unwrap().push(event))
    }

    fn logs(seen: &[Event]) -> Vec<String> {
        seen.iter()
            .filter_map(|e| match e {
                Event::Log(line) => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_ladder_reaches_the_bootloader_at_the_fastest_baud_that_answers() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("ours:v0.4.0");
        let board = fake.candidates().unwrap().remove(0);
        let (seen, tell) = gather();
        let link = open(&fake, &board, None, None, &tell).ok().expect("reached");
        assert_eq!(link.info().baud, 921_600);
        assert_eq!(logs(&seen.lock().unwrap()), ["921600 baud: the bootloader answered"]);
    }

    #[test]
    fn a_board_that_never_answers_tries_every_rung_then_fails() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("nosync");
        let board = fake.candidates().unwrap().remove(0);
        let (seen, tell) = gather();
        let failed = open(&fake, &board, None, None, &tell);
        assert!(matches!(failed, Err(Stop::Failed(DeviceError::NoSync { .. }))));
        let rungs = logs(&seen.lock().unwrap()).iter().filter(|l| l.contains("baud: no answer")).count();
        assert_eq!(rungs, 3);
    }

    #[test]
    fn a_quit_between_the_ladders_rungs_lets_the_reset_board_go() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("nosync");
        let trace = fake.trace();
        let board = fake.candidates().unwrap().remove(0);
        let (seen, tell) = gather();
        let quit = || true;
        let stopped = open(&fake, &board, None, Some(&quit), &tell);
        assert!(matches!(stopped, Err(Stop::Quit)));
        let rungs = logs(&seen.lock().unwrap()).iter().filter(|l| l.contains("baud: no answer")).count();
        assert_eq!(rungs, 1, "no second rung");
        assert_eq!(*trace.lock().unwrap(), ["open 921600", "let go FAKE0"], "the board the rung reset is restarted");
    }

    #[test]
    fn a_write_that_dies_on_the_wire_is_tried_again_a_rung_down_each_time() {
        let _en = crate::setup::tests::in_locale("en");
        let fake = Fake::new("failwrite").with_pace(Duration::from_millis(30));
        let board = fake.candidates().unwrap().remove(0);
        let (seen, tell) = gather();
        let link = open(&fake, &board, None, None, &tell).ok().expect("reached");
        let desc = desc_bytes("v0.5.0", AppDesc::OURS);
        let firmware = Firmware {
            version: "v0.5.0".into(),
            origin: "a test".into(),
            kind: crate::device::firmware::Origin::File,
            segments: vec![crate::device::firmware::Segment { offset: 0, data: merged_bytes(&desc) }],
        };
        let failed = write(&fake, &board, link, &firmware, &tell);
        assert!(matches!(failed, Err(DeviceError::Link(_))));
        let log = logs(&seen.lock().unwrap());
        assert!(log.iter().any(|l| l.starts_with("the link failed at 921600 baud — trying 460800")), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("the link failed at 460800 baud — trying 115200")), "{log:?}");
        assert!(log.iter().any(|l| l.starts_with("to write: 0x0 · ")), "{log:?}");
    }

    #[test]
    fn a_write_reports_its_phases_and_percent_and_restarts_into_what_it_wrote() {
        let fake = Fake::new("ours:v0.4.0").with_pace(Duration::from_millis(30));
        let board = fake.candidates().unwrap().remove(0);
        let (seen, tell) = gather();
        let link = open(&fake, &board, None, None, &tell).ok().expect("reached");
        let desc = desc_bytes("v0.5.0", AppDesc::OURS);
        let firmware = Firmware {
            version: "v0.5.0".into(),
            origin: "a test".into(),
            kind: crate::device::firmware::Origin::File,
            segments: vec![crate::device::firmware::Segment { offset: 0, data: merged_bytes(&desc) }],
        };
        let (link, skipped) = write(&fake, &board, link, &firmware, &tell).expect("written");
        assert!(!skipped);
        let phases: Vec<Phase> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Phase(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(phases, [Phase::Comparing, Phase::Writing, Phase::Verifying]);
        assert!(seen.lock().unwrap().contains(&Event::Progress(100)));
        let boot = link.restart().unwrap().expect("the fake boots what it wrote");
        assert!(boot.starts_with("mstream-mp3-player v0.5.0"), "{boot}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.5.0"));
    }
}
