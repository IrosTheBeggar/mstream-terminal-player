//! The mStream MP3 player — an M5Stack Core2 running mstream-mp3-player —
//! over its USB cable. `mstream-player device list` names the boards that
//! look like one; `mstream-player device flash` installs or updates the
//! firmware on one. No mStream session is involved: the board is a USB
//! serial port, and the firmware is a file — a release of
//! IrosTheBeggar/mstream-mp3-player, or a build of it.
//!
//! The pieces: [`ports`] finds the boards by the USB bridge chips M5Stack
//! ships, [`firmware`] finds the image (a path, a release, the pin) and
//! reads what it is, [`engine`] talks to the board (espflash as a library,
//! or a fake for the tests), [`flow`] runs the whole write in a worker
//! thread and reports each step, and [`page`] draws those reports on the
//! admin hub's terminal session — with a step line, and a log of every
//! report behind `l`. `--yes` prints the steps as lines instead. The GUI
//! player hosts the same page, with no flags, as its MP3 Player tab
//! (src/gui/device.rs, docs/ux-contracts/mp3-player-screen.md).

mod engine;
mod firmware;
mod flow;
mod page;
mod ports;

use std::path::PathBuf;

use clap::{Args, Subcommand};
use rust_i18n::t;

use self::flow::{Cmd, Event};

// What the GUI's MP3 Player tab holds of this module: the page, and the
// one way to build it — with no flags — outside the tests, whose page
// rides channels they hold (`Page::quiet`, its `Ends`).
#[cfg(not(test))]
pub(crate) use self::page::hosted;
pub(crate) use self::page::Page;
#[cfg(test)]
pub(crate) use self::{flow::Cmd as WorkerCmd, page::Ends};

#[derive(Args)]
pub struct DeviceArgs {
    #[command(subcommand)]
    what: DeviceCmd,
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// List the boards on USB that look like a Core2 (by their serial bridge chip)
    List,
    /// Install or update the player firmware on a connected Core2
    Flash(FlashArgs),
}

#[derive(Args, Clone)]
pub struct FlashArgs {
    /// A firmware to write instead of the pinned release: a `*-full.bin` from
    /// a release, or a build directory holding `firmware.factory.bin`
    #[arg(long, value_name = "PATH")]
    firmware: Option<PathBuf>,

    /// A firmware release to download instead of the pinned one, by its tag
    /// (checked against that release's SHA256SUMS)
    #[arg(long, value_name = "TAG", conflicts_with = "firmware")]
    release: Option<String>,

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

    fn source(&self) -> firmware::Source {
        firmware::Source::from_args(self.firmware.clone(), self.release.clone())
    }
}

pub fn run(args: DeviceArgs) -> i32 {
    match args.what {
        DeviceCmd::List => list(),
        DeviceCmd::Flash(args) if args.yes => lines(args),
        DeviceCmd::Flash(args) => {
            crate::setup::boot_language();
            page::run(args)
        }
    }
}

/// `device list`: one line per board, exit 1 when there is none — a script
/// can ask "is a Core2 plugged in?" without parsing anything.
fn list() -> i32 {
    let engine = engine::from_env();
    match engine.candidates() {
        Ok(found) if found.is_empty() => {
            println!("{}", t!("dev.no_device_title"));
            1
        }
        Ok(found) => {
            for board in found {
                println!("{}", board.describe());
            }
            0
        }
        Err(e) => {
            eprintln!("mstream-player: {}", e.text());
            2
        }
    }
}

/// `device flash --yes`: the same worker as the page, its reports printed
/// as lines, the one question answered by the flags. Several boards with
/// no `--port` is a refusal, not a guess.
fn lines(args: FlashArgs) -> i32 {
    let (cmds, events) =
        flow::spawn(Box::new(engine::from_env), args.source(), args.port.clone(), args.erase_asked());
    let mut last_pct: Option<u8> = None;
    loop {
        let Ok(event) = events.recv() else {
            eprintln!("mstream-player: {}", t!("note.worker_gone"));
            return 1;
        };
        match event {
            Event::Phase(phase) => println!("{}", phase.text()),
            Event::Download { done, total } => {
                // Quarters, not every chunk: a line per packet is noise.
                if let Some(total) = total.filter(|t| *t > 0) {
                    let pct = ((done * 100) / total).min(100) as u8;
                    if pct.is_multiple_of(25) && last_pct != Some(pct) {
                        last_pct = Some(pct);
                        println!("  {pct}%");
                    }
                }
            }
            Event::Firmware { version, origin, bytes, .. } => {
                last_pct = None;
                println!("firmware: {version} ({origin}, {} KB)", bytes / 1024);
            }
            Event::NoDevice { others } => {
                println!("{}", t!("dev.no_device_title"));
                println!("{}", t!("dev.no_device_body"));
                if !others.is_empty() {
                    println!("{} {}", t!("dev.ports_seen"), others.join(", "));
                }
                return 1;
            }
            Event::Several(found) => {
                println!("{}", t!("dev.several_title"));
                for board in found {
                    println!("  {}", board.describe());
                }
                println!("{}", t!("dev.several_pass_port"));
                return 1;
            }
            Event::Board(info) => println!("board: {}", info.describe()),
            Event::Probed { on_board, plan } => {
                println!("{}: {}", t!("dev.on_board"), flow::on_board_text(on_board.as_ref()));
                println!("plan: {}", plan.describe());
                if cmds.send(Cmd::Go { erase: plan.erase }).is_err() {
                    return 1;
                }
            }
            Event::Progress(pct) => {
                if pct.is_multiple_of(10) && last_pct != Some(pct) {
                    last_pct = Some(pct);
                    println!("  {pct}%");
                }
            }
            Event::Done { version, skipped, boot } => {
                if skipped {
                    println!("{}", t!("dev.done_skipped"));
                }
                println!("{}", t!("dev.done_body", version = version));
                if let Some(line) = boot {
                    println!("{}", t!("dev.done_booted", line = line));
                }
                return 0;
            }
            Event::Cancelled => {
                println!("{}", t!("dev.cancelled"));
                return 1;
            }
            Event::Failed(e) => {
                eprintln!("mstream-player: {}", e.text());
                if let Some(hint) = e.hint() {
                    eprintln!("  {hint}");
                }
                return 1;
            }
            // The page's log lines: the line mode prints the steps, not
            // the details (a --verbose can, later).
            Event::Log(_) => {}
        }
    }
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
            server: None,
        };
        assert_eq!(base.erase_asked(), None, "neither flag: the board decides");
        assert_eq!(FlashArgs { erase: true, ..base.clone() }.erase_asked(), Some(true));
        assert_eq!(FlashArgs { no_erase: true, ..base.clone() }.erase_asked(), Some(false));
    }

    #[test]
    fn every_error_has_a_sentence_and_the_actionable_ones_a_hint() {
        rust_i18n::set_locale("en");
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
}
