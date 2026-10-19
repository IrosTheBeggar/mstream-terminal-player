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
//!
//! [`card`] is the board's SD card, written with the card in a PC's reader,
//! never over the cable: so far its contract kit, the files the firmware
//! reads from a card byte for byte; the names, tag records and Auto DJ
//! index the transfer puts there by it; the cards in this computer's
//! readers, found and held to the drive-safety rule; the engine that
//! writes a sync onto one of them; and the set-up that makes a card one it
//! syncs, with the card's identity and settings file.

mod board;
mod card;
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

use self::board::{Board, Card, CardKind, CardUnknown, Format, FormatWhy, Free, Heard, Tracks, Verdict, Work};
use self::board::{Written, gb};
use self::desk::{All, Cmd, Event, ImageState, LogKind, Refusal};
use self::engine::Engine;
use self::firmware::{Cached, Image, Images, Release, Supply, Target};

// What the GUI's MP3 Player tab holds of this module: the page, the one
// way to build it — with no flags — outside the tests, whose page rides
// channels they hold (`Page::quiet`, its `Ends`), and the line it draws
// while a page it left lets its ports go.
#[cfg(not(test))]
pub(crate) use self::page::hosted;
pub(crate) use self::page::{LET_GO, Page, Transfer, Wants, draw_waiting};
// The SD card transfer's worker, which the GUI starts and owns (the MP3
// Player contract's clause 55).
pub(crate) use self::card::worker;
#[cfg(test)]
pub(crate) use self::{desk::Cmd as WorkerCmd, page::Ends};
#[cfg(test)]
pub(crate) use self::firmware::{Image as WorkerImage, Mode as WorkerMode};

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
    /// List the SD cards in this computer's card readers, one line each: the drive, the name,
    /// the size and file system, then the server, tracks, last sync and next sync, or why the
    /// player can't use it. Reads only, and asks each card's server once
    Cards,
    /// One card in a reader: `status` prints its facts; `setup`, `sync` and `plays` print their
    /// plan and stop without --yes; `eject` lets it go. Each gate is a flag naming what it lets
    /// through
    Card(CardArgs),
    /// Ask the Core2 on USB to format its SD card as FAT32, which erases all of it: the player
    /// asks on its own screen (Format..., then its red button held 2 s) and formats the card
    /// itself. Prints the card, the player's answer, the progress and the result
    Format(FormatArgs),
    // The hidden card commands below stay, as JSON, for the real-card checks
    // and scripts that read every field: `device cards` and `device card`
    // are the listener's, in plain lines, on the GUI's transfer worker
    // (docs/ux-contracts/mp3-player-screen.md, clauses 64-67; the decision
    // of 2026-10-19 in its deviations log).
    //
    // The cards in this computer's readers as JSON, each volume with its
    // verdict and an accepted card with what it holds; read only.
    #[command(hide = true)]
    CardProbe,
    // Sets up the card at a volume the drive-safety rule accepts (`/music`,
    // the empty first commit, its state file) and prints what it found and
    // did as JSON; `--dry-run` says what it would do and writes nothing.
    #[command(hide = true)]
    CardSetup(CardSetupArgs),
    // Syncs a set-up card at a volume the drive-safety rule accepts from the
    // server of the saved session, and prints the plan and what came of it
    // as JSON; `--dry-run` plans and writes nothing. Its spread flags make a
    // multi-card set, which the Card view doesn't yet.
    #[command(hide = true)]
    CardSync(CardSyncArgs),
    // Prints the multi-card set of a set-up card at a volume the drive-safety
    // rule accepts, as the card and this computer know it, and the sets this
    // computer knows that wait for a card; `--join SET --index N` makes the
    // card that card of the set, `--dry-run` writing nothing. For the
    // real-card checks: the listener's Card view waits for its UX contract.
    #[command(hide = true)]
    CardSet(CardSetArgs),
    // Reads the play log of a set-up card at a volume the drive-safety rule
    // accepts, maps each new play through the card's ledger and posts it to
    // the server of the saved session, and prints what came of it as JSON.
    // It only reads the card: the plays it settles are acknowledged there
    // (ACKS, the state file's marks) by the next sync's commit. `--dry-run`
    // maps them and saves and posts nothing.
    #[command(hide = true)]
    CardPlays(CardPlaysArgs),
    // Reads the clock of the board on a port with a bare `@time`, over the
    // running firmware's host lines and with no reset, and sets it to this
    // computer's (`@time <unix-ms>`) when it is unset or more than 2 s off;
    // `--dry-run` reads it and says whether it would. Prints what it found
    // and did as JSON. For the board checks (the firmware's HOST-STATUS,
    // step 10): the MP3 Player tab setting the clock when it sees the board
    // waits for its UX contract.
    #[command(hide = true)]
    Time(TimeArgs),
}

/// What `device card` does to the card.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum CardWhat {
    Status,
    Setup,
    Sync,
    Plays,
    Eject,
}

// The flags of `device card` (clause 65): the gates' name what they let
// through; the settings' write state.json only. Each is checked against
// the action it belongs to.
#[derive(Args, Clone)]
pub struct CardArgs {
    /// The card: a drive letter (E:), a mount path, or a card's name when one card has it
    drive: String,
    /// What to do: status, setup, sync, plays or eject
    #[arg(value_enum)]
    what: CardWhat,
    /// setup, sync, plays: no question; print each step and do it
    #[arg(long, short = 'y')]
    yes: bool,
    /// sync: let it remove N tracks, N the plan's exact count (more than 50 removals, or half
    /// the card, ask first)
    #[arg(long, value_name = "N")]
    allow_removals: Option<usize>,
    /// sync: print every path the sync would remove
    #[arg(long)]
    list_removals: bool,
    /// setup, sync: the saved server the card is filled from, by its name or its address
    #[arg(long, value_name = "NAME")]
    server: Option<String>,
    /// setup, sync: with --server, make it the server of a card another server filled
    #[arg(long, requires = "server")]
    take_over: bool,
    /// setup, sync: the music on the card that mStream didn't put there and the server also
    /// has is the sync's from now on, removals and all
    #[arg(long, conflicts_with = "keep_mine")]
    adopt: bool,
    /// setup, sync: the music on the card that mStream didn't put there stays yours
    #[arg(long)]
    keep_mine: bool,
    /// sync: copy a peer's files through the parent server that alone reaches it
    #[arg(long)]
    through_parent: bool,
    /// setup: a copied card takes an identity of its own
    #[arg(long)]
    new_identity: bool,
    /// setup: the card's name
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// setup: the server's libraries it holds, comma-separated
    #[arg(long, value_delimiter = ',', value_name = "LIST")]
    libraries: Option<Vec<String>>,
    /// setup: convert only what the player can't play, or everything
    #[arg(long, value_parser = ["needed", "all"])]
    convert: Option<String>,
    /// setup: what it converts to: opus-64 to opus-192, mp3-128 to mp3-320
    #[arg(long, value_name = "CODEC")]
    to: Option<String>,
    /// setup: follow the server's removals, or never remove
    #[arg(long, value_parser = ["follow", "never"])]
    removals: Option<String>,
    /// setup: small covers for the player
    #[arg(long, value_parser = ["on", "off"])]
    covers: Option<String>,
    /// setup: write its Auto DJ index
    #[arg(long = "autodj", value_parser = ["on", "off"])]
    auto_dj: Option<String>,
    /// setup: a card for you, or for someone else (no Auto DJ index, plays not sent)
    #[arg(long = "for", value_parser = ["me", "someone-else"])]
    for_whom: Option<String>,
    /// setup: what this computer takes from each peer a day, in MB
    #[arg(long, value_name = "MB")]
    budget: Option<u64>,
}

impl CardArgs {
    /// What to do and how, or the flag that doesn't belong with it.
    fn flags(&self) -> Result<(card::lines::What, card::lines::Flags), String> {
        use card::lines::What;
        let what = match self.what {
            CardWhat::Status => What::Status,
            CardWhat::Setup => What::Setup,
            CardWhat::Sync => What::Sync,
            CardWhat::Plays => What::Plays,
            CardWhat::Eject => What::Eject,
        };
        let on = |v: &Option<String>| v.as_deref().map(|w| w == "on");
        let to = match &self.to {
            Some(word) => {
                let refused = || format!("--to {word} isn't one it takes");
                Some(card::lines::codec_of(word).ok_or_else(refused)?)
            }
            None => None,
        };
        let flags = card::lines::Flags {
            yes: self.yes,
            allow_removals: self.allow_removals,
            list_removals: self.list_removals,
            server: self.server.clone(),
            take_over: self.take_over,
            adopt: self.adopt,
            keep_mine: self.keep_mine,
            through_parent: self.through_parent,
            new_identity: self.new_identity,
            name: self.name.clone(),
            libraries: self.libraries.clone(),
            convert: self.convert.as_deref().map(|w| {
                if w == "all" { card::state::Convert::Everything } else { card::state::Convert::Unplayable }
            }),
            to,
            removals: self.removals.as_deref().map(|w| {
                if w == "never" { card::engine::Removal::Never } else { card::engine::Removal::Mirror }
            }),
            covers: on(&self.covers),
            auto_dj: on(&self.auto_dj),
            for_someone_else: self.for_whom.as_deref().map(|w| w == "someone-else"),
            budget_mb: self.budget,
        };
        // Each flag with the action it belongs to.
        let setup_only = [
            ("--new-identity", self.new_identity),
            ("--name", self.name.is_some()),
            ("--libraries", self.libraries.is_some()),
            ("--convert", self.convert.is_some()),
            ("--to", self.to.is_some()),
            ("--removals", self.removals.is_some()),
            ("--covers", self.covers.is_some()),
            ("--autodj", self.auto_dj.is_some()),
            ("--for", self.for_whom.is_some()),
            ("--budget", self.budget.is_some()),
        ];
        let sync_only = [
            ("--allow-removals", self.allow_removals.is_some()),
            ("--list-removals", self.list_removals),
            ("--through-parent", self.through_parent),
        ];
        let both = [
            ("--server", self.server.is_some()),
            ("--take-over", self.take_over),
            ("--adopt", self.adopt),
            ("--keep-mine", self.keep_mine),
        ];
        let (setup, sync) = (what == What::Setup, what == What::Sync);
        let wrong = |list: &[(&'static str, bool)], fits: bool| {
            list.iter().find(|(_, on)| *on && !fits).map(|(flag, _)| *flag)
        };
        let refused = wrong(&setup_only, setup)
            .or_else(|| wrong(&sync_only, sync))
            .or_else(|| wrong(&both, setup || sync))
            .or_else(|| wrong(&[("--yes", self.yes)], setup || sync || what == What::Plays));
        match refused {
            Some(flag) => Err(format!("{flag} doesn't go with {}", what.word())),
            None => Ok((what, flags)),
        }
    }
}

#[derive(Args, Clone)]
pub struct TimeArgs {
    // The board's serial port (`COM3`, `/dev/ttyACM0`): one of the ports
    // `device list` names.
    port: String,
    // Read the clock and print, and set nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Clone)]
pub struct CardSetupArgs {
    // Where the card is mounted (`E:`, `/media/card`), or the OS's name for
    // the volume.
    volume: String,
    // The card's label, for a card with no state file yet; the volume's own
    // without it.
    #[arg(long)]
    label: Option<String>,
    // A copied card's answer: an identity of its own.
    #[arg(long)]
    new_identity: bool,
    // Say what the set-up would do, and write nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Clone)]
pub struct CardSetArgs {
    // Where the card is mounted, or the OS's name for the volume.
    volume: String,
    // The set to join, by its id (`0x…`), as `cards.json` knows it.
    #[arg(long, value_name = "SET", requires = "index")]
    join: Option<String>,
    // The card's place in the set, from 1.
    #[arg(long, value_name = "N", requires = "join")]
    index: Option<usize>,
    // Say what the join would do, and write nothing.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Clone)]
pub struct CardPlaysArgs {
    // Where the card is mounted, or the OS's name for the volume.
    volume: String,
    // Map the plays and print, and save and post nothing.
    #[arg(long)]
    dry_run: bool,
}

// The settings flags change the card's settings, stored after a run; the
// answers are this run's alone.
#[derive(Args, Clone)]
pub struct CardSyncArgs {
    // Where the card is mounted, or the OS's name for the volume.
    volume: String,
    // Plan and print, and write nothing to the card.
    #[arg(long)]
    dry_run: bool,
    // A library to sync, by its name on the server; again for several. The
    // card's own selection without, else every library the account sees.
    #[arg(long = "library", value_name = "VPATH")]
    libraries: Vec<String>,
    // unplayable | everything
    #[arg(long)]
    convert: Option<String>,
    // opus | mp3
    #[arg(long)]
    codec: Option<String>,
    // 64 | 96 | 128 | 192 | 256 | 320: past 192 MP3 only, on a server that
    // lists the rate (else the highest below it that the server takes)
    #[arg(long)]
    kbps: Option<u32>,
    // auto | plain | vpath
    #[arg(long)]
    layout: Option<String>,
    // mirror | never
    #[arg(long)]
    removal: Option<String>,
    #[arg(long, value_name = "BOOL")]
    covers: Option<bool>,
    #[arg(long, value_name = "BOOL")]
    auto_dj: Option<bool>,
    #[arg(long, value_name = "BOOL")]
    for_someone_else: Option<bool>,
    // The server's name on this card ("Home"); never an address.
    #[arg(long)]
    nickname: Option<String>,
    #[arg(long)]
    allow_removals: bool,
    #[arg(long)]
    allow_mass_removals: bool,
    #[arg(long, value_name = "BOOL")]
    adopt: Option<bool>,
    #[arg(long)]
    new_server: bool,
    #[arg(long)]
    same_server: bool,
    #[arg(long)]
    disk_checked: bool,
    // Read the server's manifest whole, whatever the schedule says.
    #[arg(long)]
    whole: bool,
    // A federation peer of the server to read too, by its id in the
    // server's peer list; again for several. The card's own peers without.
    #[arg(long = "peer", value_name = "ID")]
    peers: Vec<u32>,
    // One library of a peer, as ID:VPATH; again for several. Its peer is
    // read for these alone.
    #[arg(long = "peer-library", value_name = "ID:VPATH")]
    peer_libraries: Vec<String>,
    // Read the peers alone: the server's own files on the card stay as the
    // card has them.
    #[arg(long)]
    peers_only: bool,
    // A peer whose files may come through an older server's proxy, which
    // can't tell the peer they are copies; again for several. Stored on the
    // card.
    #[arg(long = "proxy-copies", value_name = "ID")]
    proxy_copies: Vec<u32>,
    // The daily budget for the peers read, in MiB, kept on this computer;
    // 0 goes back to the peer's quota, or 1,500.
    #[arg(long, value_name = "MIB")]
    budget_mb: Option<u64>,
    // Spread the card's selection over a new multi-card set, by artist, this
    // card its first; the others join it with `device card-set`.
    #[arg(long, conflicts_with_all = ["move_edge", "add_card"])]
    spread: bool,
    // A card of a set that can't take its range: its last artists go to the
    // next card of the set.
    #[arg(long, conflicts_with = "add_card")]
    move_edge: bool,
    // A card of a set that can't take its range: its last artists go to a
    // new card after it.
    #[arg(long)]
    add_card: bool,
}

impl CardSyncArgs {
    /// The sync these flags ask for, or the flag a word of which isn't one.
    fn ask(&self) -> Result<card::sync::Ask, String> {
        use card::engine::{Answers, Removal};
        use card::state::{Convert, Layout};
        type Word<T> = Result<Option<T>, String>;
        fn word<T>(flag: &str, value: &Option<String>, of: impl Fn(&str) -> Option<T>) -> Word<T> {
            let refused = |v: &str| format!("--{flag} {v} isn't one it takes");
            match value {
                None => Ok(None),
                Some(v) => of(v).map(Some).ok_or_else(|| refused(v)),
            }
        }
        let removal = |w: &str| match w {
            "mirror" => Some(Removal::Mirror),
            "never" => Some(Removal::Never),
            _ => None,
        };
        let bitrate = match self.kbps {
            None => None,
            Some(k) => {
                let bitrate = crate::api::urls::TransferBitrate::from_kbps(k);
                Some(bitrate.ok_or_else(|| format!("--kbps {k} isn't one it takes"))?)
            }
        };
        let change = card::sync::Change {
            layout: word("layout", &self.layout, Layout::of_word)?,
            convert: word("convert", &self.convert, Convert::of_word)?,
            codec: word("codec", &self.codec, |c| c.parse().ok())?,
            bitrate,
            covers: self.covers,
            auto_dj: self.auto_dj,
            for_someone_else: self.for_someone_else,
            removal: word("removal", &self.removal, removal)?,
            nickname: self.nickname.clone(),
        };
        let mut peers: Vec<card::sync::PeerAsk> = Vec::new();
        for &id in &self.peers {
            if !peers.iter().any(|p| p.id == id) {
                peers.push(card::sync::PeerAsk { id, libraries: None });
            }
        }
        for one in &self.peer_libraries {
            let refused = || format!("--peer-library {one} isn't ID:VPATH");
            let (id, vpath) = one.split_once(':').ok_or_else(refused)?;
            let id: u32 = id.trim().parse().ok().filter(|&id| id > 0).ok_or_else(refused)?;
            if vpath.is_empty() {
                return Err(refused());
            }
            let at = match peers.iter().position(|p| p.id == id) {
                Some(at) => at,
                None => {
                    peers.push(card::sync::PeerAsk { id, libraries: None });
                    peers.len() - 1
                }
            };
            peers[at].libraries.get_or_insert_with(Vec::new).push(vpath.to_string());
        }
        let answers = Answers {
            disk_checked: self.disk_checked,
            removals: self.allow_removals,
            mass_removals: self.allow_mass_removals,
            removals_seen: None,
            adopt: self.adopt,
            new_servers: self.new_server,
            new_card_id: false,
        };
        Ok(card::sync::Ask {
            libraries: (!self.libraries.is_empty()).then(|| self.libraries.clone()),
            change,
            answers,
            same_server: self.same_server,
            whole: self.whole,
            dry_run: self.dry_run,
            peers: (!peers.is_empty()).then_some(peers),
            peers_only: self.peers_only,
            proxy_copies: self.proxy_copies.clone(),
            budget_mb: self.budget_mb,
            decide: match (self.spread, self.move_edge, self.add_card) {
                (true, _, _) => Some(card::span::Decision::Spread),
                (_, true, _) => Some(card::span::Decision::MoveEdge),
                (_, _, true) => Some(card::span::Decision::AddCard),
                _ => None,
            },
            stop: None,
        })
    }
}

#[derive(Args, Clone)]
pub struct FormatArgs {
    /// The serial port the Core2 is on (default: the one Core2-shaped port)
    #[arg(long, value_name = "PORT")]
    port: Option<String>,
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
        DeviceCmd::Cards => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let (mut ask, ctrl_c) = (|_: &str| None, std::sync::atomic::AtomicU8::new(0));
            let mut io = card::lines::Io { out: &mut out, err: &mut err, ask: &mut ask, ctrl_c: &ctrl_c };
            card::lines::cards(card::worker::Setup::system(), saved_servers(), &mut io)
        }
        DeviceCmd::Card(args) => {
            let (what, flags) = match args.flags() {
                Ok(asked) => asked,
                Err(why) => {
                    eprintln!("mstream-player: {why}");
                    return 2;
                }
            };
            if what == card::lines::What::Sync {
                interrupt::catch();
            }
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let mut ask = ask_on_terminal;
            let ctrl_c = &interrupt::PRESSED;
            let mut io = card::lines::Io { out: &mut out, err: &mut err, ask: &mut ask, ctrl_c };
            let setup = card::worker::Setup::system();
            card::lines::card(setup, saved_servers(), &args.drive, &what, &flags, &mut io)
        }
        DeviceCmd::Format(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let engine = engine::from_env();
            format_on(&*engine, args.port.as_deref(), desk::Timing::REAL, &mut out, &mut err)
        }
        DeviceCmd::CardProbe => card::volume::probe(&mut std::io::stdout(), &mut std::io::stderr()),
        DeviceCmd::CardSetup(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let label = args.label.as_deref();
            let (new_identity, dry_run) = (args.new_identity, args.dry_run);
            card::setup::command(&args.volume, label, new_identity, dry_run, &mut out, &mut err)
        }
        DeviceCmd::CardSet(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let join = match (&args.join, args.index) {
                (Some(id), Some(index)) => match card::state::from_hex(id).filter(|&id| id != 0) {
                    Some(id) => Some((id, index)),
                    None => {
                        eprintln!("mstream-player: --join {id} isn't a set's id (0x…)");
                        return 2;
                    }
                },
                _ => None,
            };
            card::span::command(&args.volume, join, args.dry_run, &mut out, &mut err)
        }
        DeviceCmd::CardPlays(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            card::history::command(&args.volume, args.dry_run, &mut out, &mut err)
        }
        DeviceCmd::Time(args) => {
            let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
            let engine = engine::from_env();
            let now = || unix_ms(std::time::SystemTime::now());
            time_on(&*engine, &args.port, args.dry_run, &now, listen::Waits::REAL, &mut out, &mut err)
        }
        DeviceCmd::CardSync(args) => match args.ask() {
            Ok(ask) => {
                let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
                card::sync::command(&args.volume, &ask, &mut out, &mut err)
            }
            Err(why) => {
                eprintln!("mstream-player: {why}");
                2
            }
        },
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

/// The servers saved on this computer as the command line reaches them
/// (clause 3a): each at its address. A server reached with Quick Connect
/// only the player can dial; it is listed, with why it can't be reached
/// from here, so a card filled from it still names it.
fn saved_servers() -> Vec<card::worker::ServerLink> {
    let Ok(config) = crate::config::load() else { return Vec::new() };
    let credentials = crate::config::load_credentials().ok();
    let session = crate::config::preferred_server(&config).map(|e| e.url.clone());
    let saved = config.servers.iter().filter(|e| e.peer.is_none());
    saved
        .map(|e| {
            let base = if crate::quickconnect::is_tunnel_id(&e.url) {
                let why = "reached with Quick Connect, which only the player can dial: sync it from the GUI";
                Err(why.to_string())
            } else {
                Ok(e.url.clone())
            };
            card::worker::ServerLink {
                id: e.url.clone(),
                name: crate::config::display_name(e),
                base,
                token: credentials.as_ref().and_then(|c| crate::config::token_for(c, &e.url)),
                self_signed: e.self_signed,
                local_token: None,
                relay: false,
                session: session.as_deref() == Some(e.url.as_str()),
            }
        })
        .collect()
}

/// "go on? [y/N]" on a terminal: the listener's answer; None off one, where
/// only `--yes` goes on.
fn ask_on_terminal(question: &str) -> Option<bool> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    print!("{question}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok()?;
    Some(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Ctrl+C in `device card <drive> sync` (clause 65, clause 57's rule): the
/// first is counted, and the sync stops at its next save; a second ends
/// the process at once, and the next sync goes on from the last save.
mod interrupt {
    use std::sync::atomic::{AtomicU8, Ordering};

    pub(super) static PRESSED: AtomicU8 = AtomicU8::new(0);

    #[cfg(windows)]
    pub(super) fn catch() {
        use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
        unsafe extern "system" fn handler(kind: u32) -> windows_sys::core::BOOL {
            if kind != CTRL_C_EVENT && kind != CTRL_BREAK_EVENT {
                return 0;
            }
            if PRESSED.fetch_add(1, Ordering::SeqCst) >= 1 {
                std::process::exit(130);
            }
            1
        }
        // SAFETY: the handler is a plain function that touches an atomic
        // and, the second time, ends the process.
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(super) fn catch() {
        extern "C" fn handler(_: libc::c_int) {
            if PRESSED.fetch_add(1, Ordering::SeqCst) >= 1 {
                // SAFETY: _exit is async-signal-safe.
                unsafe { libc::_exit(130) }
            }
        }
        // SAFETY: the handler touches an atomic and, the second time, calls
        // _exit: both async-signal-safe.
        unsafe {
            libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
        }
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    pub(super) fn catch() {}
}

/// A time as Unix ms, as `@time` takes it; 0 before 1970.
fn unix_ms(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `device time <port> [--dry-run]`, undocumented: the clock of the board
/// on `port`, one of the boards the engine lists, read and set as
/// [`listen::clock`] does, through the port opened as every listen opens it
/// (DTR and RTS held low: no reset, the music plays on). `now`: this
/// computer's clock in Unix ms. Prints what it found and did as JSON, the
/// host lines both ways among it. Exit 0 when the clock is in step or was
/// set (a dry run: read); 1 when the board has no clock verb (`@err 7
/// time`: firmware older than 0.12.0), refused the time, or didn't answer,
/// or this computer's clock is out of the board's range; 2 for the port,
/// not the board: one that isn't a board's this computer lists, ports that
/// can't be listed, or one that couldn't be opened (another program holds
/// it) or failed partway.
fn time_on(
    engine: &dyn Engine,
    port: &str,
    dry_run: bool,
    now: &dyn Fn() -> u64,
    waits: listen::Waits,
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
    let Some(candidate) = found.iter().find(|c| c.port.eq_ignore_ascii_case(port)) else {
        let _ = writeln!(err, "mstream-player: {port} isn't a board's port this computer lists");
        return 2;
    };
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut lines = Vec::new();
    let clocked = match engine.listen(candidate) {
        Ok(mut wire) => {
            let mut note = |line: String| lines.push(line);
            listen::clock(&mut *wire, &candidate.port, now, dry_run, waits, &stop, &mut note)
        }
        Err(e) => Err(listen::TimeWhy::Port(e)),
    };
    let mut printed = serde_json::json!({ "port": candidate.port, "dryRun": dry_run, "lines": lines });
    let code = match &clocked {
        Ok(c) => {
            let board = match c.before {
                listen::BoardTime::At(ms) => serde_json::json!(ms),
                listen::BoardTime::Unset => serde_json::json!("unset"),
            };
            printed["board"] = board;
            printed["offBy"] = serde_json::json!(c.off_ms);
            printed["due"] = c.due.into();
            printed["sent"] = serde_json::json!(c.sent);
            printed["after"] = serde_json::json!(c.after_ms);
            0
        }
        Err(why) => {
            printed["error"] = why.code().into();
            let words = match why {
                listen::TimeWhy::Old => "no clock verb: firmware older than 0.12.0".to_string(),
                listen::TimeWhy::Range => "the board refused the time as out of range".to_string(),
                listen::TimeWhy::Syntax => "the board couldn't read the time".to_string(),
                listen::TimeWhy::Rtc => "the board's clock chip didn't take the time".to_string(),
                listen::TimeWhy::OurClock => "this computer's clock is outside 2026-2099".to_string(),
                listen::TimeWhy::NoAnswer => "the board didn't answer".to_string(),
                listen::TimeWhy::Port(e) => e.text(),
                listen::TimeWhy::Other(line) => line.clone(),
            };
            let _ = writeln!(err, "mstream-player: {}: {words}", candidate.port);
            if matches!(why, listen::TimeWhy::Port(_)) { 2 } else { 1 }
        }
    };
    let _ = writeln!(out, "{}", serde_json::to_string_pretty(&printed).unwrap_or_default());
    code
}

/// `device format [--port P]` (contract clause 66): the board's player is
/// asked to format its card, over USB with no reset as `device list` asks
/// it, and what comes of it is said in plain lines — the card as the board
/// reports it (what is lost), the player's answer, the progress and the
/// result. There is no `--yes`: the player confirms on its own screen,
/// twice, and nothing on this side writes the card. Exit 0 formatted; 1
/// declined, timed out, refused (older firmware too) or failed, and with
/// several boards and no `--port`; 2 no board, ports that can't be listed,
/// or a `--port` this computer doesn't list.
fn format_on(
    engine: &dyn Engine,
    port: Option<&str>,
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
    let found: Vec<ports::Candidate> = match port {
        Some(port) => match found.into_iter().find(|c| c.port.eq_ignore_ascii_case(port)) {
            Some(candidate) => vec![candidate],
            None => {
                let _ = writeln!(err, "mstream-player: {port} isn't a board's port this computer lists");
                return 2;
            }
        },
        None => found,
    };
    if found.is_empty() {
        let _ = writeln!(out, "{}", t!("dev.no_device_title"));
        return 2;
    }
    let target = firmware::pin_target();
    let mut boards = desk::listen_all(engine, &found, timing.at_rest, timing.waits);
    for board in &mut boards {
        board.verdict = board.judge(target.as_ref());
    }
    if boards.len() > 1 {
        let _ = writeln!(out, "{}", t!("dev.several_title"));
        for board in &boards {
            let _ = writeln!(out, "  {}", board_line(board, target.as_ref()));
        }
        let _ = writeln!(out, "Pass --port with one of them.");
        return 1;
    }
    let (board, candidate) = (&boards[0], &found[0]);
    let name = board.port().to_string();
    if board.status().is_none() {
        let words = verdict_words(board, target.as_ref());
        let only = "Only a player that answers over USB formats its card.";
        let _ = writeln!(err, "mstream-player: {name}: {words}. {only}");
        return 1;
    }
    let card = card_words(board).unwrap_or_else(|| "a card".to_string());
    let _ = writeln!(out, "{name}: {card}. Formatting erases all of it, even what the player can't read.");
    let stop = std::sync::atomic::AtomicBool::new(false);
    let result = match engine.listen(candidate) {
        Ok(mut wire) => {
            let mut last = None;
            let mut told = |state: Format| match state {
                Format::Waiting => {
                    let how = "Format... then its red button held 2 s";
                    let _ = writeln!(out, "Asked: confirm on {name}'s screen, {how}. Nothing is erased until you do.");
                }
                Format::Restarting => {
                    let _ = writeln!(out, "Confirmed: {name} restarts and formats the card.");
                }
                Format::Running { pct } if last != Some(pct) => {
                    last = Some(pct);
                    let _ = writeln!(out, "  {pct}%");
                }
                _ => {}
            };
            listen::format(&mut *wire, &name, timing.waits, &stop, &mut told, &mut |_| {})
        }
        Err(e) => Err(FormatWhy::Port(e)),
    };
    match result {
        Ok(done) => {
            let took = desk::took_text(done.took);
            let (size, free, kb) = (gb(done.size), gb(done.free), done.cluster / 1024);
            let card = format!("{size} GB, {free} GB free, {kb} KB clusters");
            let _ = writeln!(out, "Formatted {name}'s card as FAT32 in {took}: {card}.");
            let _ = writeln!(out, "Next: put it in this computer's card reader to fill it (device cards).");
            0
        }
        Err(why @ (FormatWhy::Declined | FormatWhy::TimedOut)) => {
            let _ = writeln!(out, "{}", format_line(&why, &name));
            1
        }
        Err(why) => {
            let _ = writeln!(err, "mstream-player: {}", format_line(&why, &name));
            1
        }
    }
}

/// A format's end in `device format`'s plain words: what the tab's busy line
/// says (clause 59), as a script reads it.
fn format_line(why: &FormatWhy, port: &str) -> String {
    let word = |why: &str, words: &[(&str, &str)]| {
        let known = words.iter().find(|(w, _)| *w == why).map(|(_, said)| said.to_string());
        known.unwrap_or_else(|| format!("it answered {why}"))
    };
    match why {
        FormatWhy::Declined => format!("Not formatted: it was cancelled on {port}'s screen."),
        FormatWhy::TimedOut => "Not formatted: nobody confirmed on the player within a minute.".to_string(),
        FormatWhy::Old => format!("{port} can't format a card: its firmware is older than 0.12.0."),
        FormatWhy::NotAsked(why) => {
            let words = [
                ("ui", "the player is still starting up; try again in a few seconds"),
                ("screen", "another screen has its display; try again once it is closed"),
                ("viz", "a computer drives its dancer; end that first"),
                ("busy", "it is restarting; try again once it is back"),
                ("card", "no card answers in its slot"),
            ];
            format!("Not asked: {port}: {}.", word(why, &words))
        }
        FormatWhy::Refused(why) => {
            let words = [
                ("card", "another card is in its slot since it started, or none answers"),
                ("size", "FAT32 can't take the card (under 42 MB, or 2 TB and more)"),
                ("busy", "it was restarting already"),
            ];
            format!("Not formatted: {port}: {}. Nothing was erased.", word(why, &words))
        }
        FormatWhy::Failed(why) => {
            let words = [
                ("card", "after its restart another card answered, or none; nothing was written"),
                ("size", "FAT32 can't take its size; nothing was written"),
                ("write", "the card refused a write"),
                ("mkfs", "its file system couldn't be made"),
                ("check", "it didn't read back what was written"),
                ("files", "it is FAT32 now, but /music or device.txt couldn't be written: format it again"),
            ];
            format!("Couldn't format the card in {port}: {}.", word(why, &words))
        }
        FormatWhy::NoAnswer => format!("{port} didn't answer @format: nothing was asked."),
        FormatWhy::Lost | FormatWhy::Port(_) | FormatWhy::Abandoned => {
            format!("Lost touch with {port} during the format: it goes on by itself.")
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
        // `device list` asks once: a board listing its library is said so,
        // not waited for.
        Verdict::Starting if board.version().is_some() => {
            format!("{version}, starting up — it answers once its library is listed")
        }
        Verdict::Starting => {
            "mStream firmware, starting up — it answers once its library is listed".to_string()
        }
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
        Verdict::Starting => "starting up",
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
        flags_mode: args.flash_mode,
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
        // No gate: the write is whatever the board's next write is.
        let write = Cmd::Write { port: board.port().to_string(), erase: mode.erase, image: None };
        if cmds.send(write).is_err() {
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
    use super::firmware::tests::{Scratch, Shelf, desc_bytes, merged_in, past_pin, pin, pin_elf};
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

    /// `device cards` and `device card <drive> <what>` are the listener's,
    /// in the help (clauses 64-67), and each flag goes with its action alone.
    #[test]
    fn the_cards_commands_are_in_the_help_and_each_flag_goes_with_its_action() {
        use clap::CommandFactory;
        use crate::api::urls::{TransferBitrate, TransferCodec};
        use card::lines::What;
        let help = Line::command().render_help().to_string();
        assert!(help.contains("  cards ") && help.contains("  card "), "{help}");
        let parse = |args: &[&str]| Line::try_parse_from(["mstream-player"].iter().chain(args)).map(|l| l.what);
        assert!(matches!(parse(&["cards"]), Ok(DeviceCmd::Cards)));
        let flags = |args: &[&str]| match parse(args) {
            Ok(DeviceCmd::Card(args)) => args.flags(),
            Ok(_) => Err("another command".to_string()),
            Err(e) => Err(e.to_string()),
        };
        let sync = ["card", "E:", "sync", "--yes", "--allow-removals", "214", "--through-parent"];
        let (what, f) = flags(&sync).unwrap();
        assert_eq!((what, f.yes, f.allow_removals, f.through_parent), (What::Sync, true, Some(214), true));
        let take = ["card", "E:", "sync", "--server", "Work", "--take-over", "--keep-mine"];
        let take = flags(&take).unwrap().1;
        assert_eq!((take.server.as_deref(), take.take_over, take.keep_mine), (Some("Work"), true, true));
        let setup = [
            "card", "Car music", "setup", "--name", "Gym", "--libraries", "Rock,Jazz", "--convert",
            "all", "--to", "mp3-320", "--removals", "never", "--covers", "off", "--autodj", "on",
            "--for", "someone-else", "--budget", "500",
        ];
        let (what, f) = flags(&setup).unwrap();
        assert_eq!(what, What::Setup);
        let libraries = Some(vec!["Rock".to_string(), "Jazz".to_string()]);
        assert_eq!((f.name.as_deref(), f.libraries.clone()), (Some("Gym"), libraries));
        assert_eq!(f.convert, Some(card::state::Convert::Everything));
        assert_eq!(f.to, Some((TransferCodec::Mp3, Some(TransferBitrate::K320))));
        assert_eq!(f.removals, Some(card::engine::Removal::Never));
        let settings = (f.covers, f.auto_dj, f.for_someone_else, f.budget_mb);
        assert_eq!(settings, (Some(false), Some(true), Some(true), Some(500)));
        assert_eq!(flags(&["card", "E:", "status", "--yes"]).unwrap_err(), "--yes doesn't go with status");
        assert_eq!(flags(&["card", "E:", "eject", "--yes"]).unwrap_err(), "--yes doesn't go with eject");
        assert_eq!(flags(&["card", "E:", "sync", "--name", "x"]).unwrap_err(), "--name doesn't go with sync");
        let removals = flags(&["card", "E:", "setup", "--allow-removals", "3"]).unwrap_err();
        assert_eq!(removals, "--allow-removals doesn't go with setup");
        assert_eq!(flags(&["card", "E:", "plays", "--adopt"]).unwrap_err(), "--adopt doesn't go with plays");
        assert!(flags(&["card", "E:", "sync", "--take-over"]).is_err(), "--take-over names its server");
        assert!(flags(&["card", "E:", "sync", "--adopt", "--keep-mine"]).is_err(), "one answer");
        let opus = flags(&["card", "E:", "setup", "--to", "opus-320"]).unwrap_err();
        assert!(opus.contains("--to opus-320 isn't"), "{opus}");
        assert!(flags(&["card", "E:", "setup", "--convert", "some"]).is_err());
        assert!(flags(&["card", "E:", "format"]).is_err(), "formatting is device format's (B6)");
        assert!(flags(&["card", "E:"]).is_err(), "an action, always");
    }

    /// The card probe answers to its name and stays out of the help, as the
    /// other JSON card commands do: the listener's are `device cards` and
    /// `device card`.
    #[test]
    fn the_card_probe_parses_and_stays_out_of_the_help() {
        use clap::CommandFactory;
        let parsed = Line::try_parse_from(["mstream-player", "card-probe"]).map(|l| l.what);
        assert!(matches!(parsed, Ok(DeviceCmd::CardProbe)));
        let help = Line::command().render_help().to_string();
        assert!(help.contains("releases") && !help.contains("card-probe"), "{help}");
        // So does the set-up's, which needs a volume.
        let parse = |args: &[&str]| Line::try_parse_from(["mstream-player"].iter().chain(args)).map(|l| l.what);
        let setup = parse(&["card-setup", "E:", "--label", "Car", "--new-identity"]);
        let Ok(DeviceCmd::CardSetup(args)) = setup else { panic!("card-setup") };
        assert_eq!((args.volume.as_str(), args.label.as_deref(), args.new_identity), ("E:", Some("Car"), true));
        assert!(!args.dry_run, "a set-up writes unless asked not to");
        assert!(matches!(parse(&["card-setup", "E:"]), Ok(DeviceCmd::CardSetup(CardSetupArgs { label: None, .. }))));
        let dry = parse(&["card-setup", "E:", "--dry-run"]);
        let Ok(DeviceCmd::CardSetup(dry)) = dry else { panic!("card-setup --dry-run") };
        assert!(dry.dry_run && dry.label.is_none());
        assert!(parse(&["card-setup"]).is_err(), "a volume, always");
        assert!(!help.contains("card-setup"), "{help}");
        // And the sync's: its flags are the card's settings and the run's
        // answers.
        let sync = parse(&[
            "card-sync", "E:", "--dry-run", "--library", "Rock", "--library", "Jazz", "--convert", "everything",
            "--codec", "mp3", "--kbps", "128", "--removal", "never", "--auto-dj", "false", "--allow-removals",
        ]);
        let Ok(DeviceCmd::CardSync(args)) = sync else { panic!("card-sync") };
        let ask = args.ask().expect("words it takes");
        assert!(ask.dry_run && ask.answers.removals && !ask.answers.mass_removals);
        assert_eq!(ask.libraries.as_deref(), Some(["Rock".to_string(), "Jazz".to_string()].as_slice()));
        let c = &ask.change;
        assert_eq!(c.convert, Some(card::state::Convert::Everything));
        use crate::api::urls::{TransferBitrate, TransferCodec};
        assert_eq!((c.codec, c.bitrate), (Some(TransferCodec::Mp3), Some(TransferBitrate::K128)));
        assert_eq!((c.removal, c.auto_dj), (Some(card::engine::Removal::Never), Some(false)));
        let Ok(DeviceCmd::CardSync(high)) = parse(&["card-sync", "E:", "--kbps", "320"]) else { panic!() };
        let wanted = high.ask().expect("320k: what the server makes of it is the plan's").change.bitrate;
        assert_eq!(wanted, Some(TransferBitrate::K320));
        let Ok(DeviceCmd::CardSync(bad)) = parse(&["card-sync", "E:", "--kbps", "160"]) else { panic!() };
        assert!(bad.ask().unwrap_err().contains("160"), "no rate the server takes");
        let Ok(DeviceCmd::CardSync(bad)) = parse(&["card-sync", "E:", "--layout", "spiral"]) else { panic!() };
        assert!(bad.ask().is_err());
        // A federation peer's: which peers and libraries, the run's say on an
        // older server's proxy, the day's budget.
        let peers = parse(&[
            "card-sync", "E:", "--peer", "3", "--peer-library", "4:Folk", "--peer-library", "4:Jazz Hands",
            "--peer", "3", "--peers-only", "--proxy-copies", "4", "--budget-mb", "700",
        ]);
        let Ok(DeviceCmd::CardSync(args)) = peers else { panic!("card-sync with peers") };
        let ask = args.ask().expect("peers it takes");
        let three = card::sync::PeerAsk { id: 3, libraries: None };
        let four = card::sync::PeerAsk { id: 4, libraries: Some(vec!["Folk".into(), "Jazz Hands".into()]) };
        assert_eq!(ask.peers, Some(vec![three, four]));
        assert!(ask.peers_only);
        assert_eq!((ask.proxy_copies.as_slice(), ask.budget_mb), ([4u32].as_slice(), Some(700)));
        let no_peers = parse(&["card-sync", "E:"]).map(|l| match l {
            DeviceCmd::CardSync(args) => args.ask().map(|a| a.peers),
            _ => Err("card-sync".into()),
        });
        assert!(matches!(no_peers, Ok(Ok(None))), "the card's own peers");
        for bad in ["Folk", "0:Folk", "x:Folk", "4:"] {
            let Ok(DeviceCmd::CardSync(args)) = parse(&["card-sync", "E:", "--peer-library", bad]) else {
                panic!("{bad}")
            };
            assert!(args.ask().unwrap_err().contains(bad), "{bad}");
        }
        assert!(parse(&["card-sync"]).is_err(), "a volume, always");
        assert!(!help.contains("card-sync"), "{help}");
        // And the plays': a volume, and a dry run.
        let Ok(DeviceCmd::CardPlays(args)) = parse(&["card-plays", "E:", "--dry-run"]) else { panic!() };
        assert_eq!((args.volume.as_str(), args.dry_run), ("E:", true));
        let Ok(DeviceCmd::CardPlays(args)) = parse(&["card-plays", "/media/card"]) else { panic!() };
        assert!(!args.dry_run);
        assert!(parse(&["card-plays"]).is_err(), "a volume, always");
        assert!(!help.contains("card-plays"), "{help}");
        // A multi-card set's: the sync's one answer, of three; the set's
        // join, its id and the card's place both or neither.
        use card::span::Decision;
        let decide = |args: &[&str]| match parse(args) {
            Ok(DeviceCmd::CardSync(args)) => Ok(args.ask().expect("words it takes").decide),
            Ok(_) => Err("card-sync".to_string()),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(decide(&["card-sync", "E:"]), Ok(None));
        assert_eq!(decide(&["card-sync", "E:", "--spread"]), Ok(Some(Decision::Spread)));
        assert_eq!(decide(&["card-sync", "E:", "--move-edge"]), Ok(Some(Decision::MoveEdge)));
        assert_eq!(decide(&["card-sync", "E:", "--add-card"]), Ok(Some(Decision::AddCard)));
        assert!(decide(&["card-sync", "E:", "--spread", "--add-card"]).is_err(), "one answer");
        assert!(decide(&["card-sync", "E:", "--move-edge", "--add-card"]).is_err(), "one answer");
        let joining = parse(&["card-set", "F:", "--join", "0x5E7", "--index", "2"]);
        let Ok(DeviceCmd::CardSet(args)) = joining else { panic!("card-set") };
        let flags = (args.join.as_deref(), args.index, args.dry_run);
        assert_eq!(flags, (Some("0x5E7"), Some(2), false));
        let Ok(DeviceCmd::CardSet(args)) = parse(&["card-set", "F:", "--dry-run"]) else {
            panic!("card-set")
        };
        assert_eq!((args.join, args.index, args.dry_run), (None, None, true));
        assert!(parse(&["card-set", "F:", "--join", "0x5E7"]).is_err(), "a place with a set");
        assert!(parse(&["card-set", "F:", "--index", "2"]).is_err(), "a set with a place");
        assert!(parse(&["card-set"]).is_err(), "a volume, always");
        assert!(!help.contains("card-set"), "{help}");
    }

    /// `device list` on the fake, measured against this build's pin as
    /// `list` measures it.
    fn listed(fake: &Fake, ports_only: bool) -> (i32, Vec<String>) {
        let target = firmware::pin_target();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = list_on(fake, ports_only, target.as_ref(), QUICK, &mut out, &mut err);
        assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
        (code, String::from_utf8(out).unwrap().lines().map(str::to_string).collect())
    }

    #[test]
    fn device_list_adds_each_boards_firmware_and_card_asked_without_a_reset() {
        let _en = crate::setup::tests::in_locale("en");
        let (pin, after) = (pin(), past_pin());
        let fake = Fake::new(&format!("status:{pin},old:v0.7.0,chip,busy,status:{after}/card=exfat/free=?/tracks=-"));
        let (code, lines) = listed(&fake, false);
        assert_eq!(code, 0);
        let ahead = format!("{after}, up to date (ahead of this player's {pin})");
        assert_eq!(
            lines,
            [
                format!("FAKE0 · CH9102 · serial FAKE0 · {pin}, up to date · card 38.2 GB free of 63.9 GB, 1,284 tracks"),
                format!("FAKE1 · CH9102 · serial FAKE1 · v0.7.0, update to {pin} · card not reported by v0.7.0"),
                "FAKE2 · CP210x · serial CHIP2 · not answering — may not be an MP3 player".into(),
                "FAKE3 · CH9102 · serial FAKE3 · in use by another program".into(),
                format!("FAKE4 · CH9102 · serial FAKE4 · {ahead} · an exFAT card — the player reads FAT32 only (63.9 GB)"),
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

    /// The hidden `device time`: the clock of a board the engine lists, read
    /// with a bare `@time` and set when it is unset or more than 2 s off,
    /// on the port as every listen opens it (never a reset). Older
    /// firmware's `@err 7 time` is no clock verb; a port no board is on is
    /// refused; it stays out of the help.
    #[test]
    fn device_time_reads_a_boards_clock_and_sets_it_only_when_unset_or_off() {
        let now = || unix_ms(std::time::SystemTime::now());
        let timed = |fake: &Fake, port: &str, dry_run: bool| {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = time_on(fake, port, dry_run, &now, listen::tests::QUICK, &mut out, &mut err);
            let printed: serde_json::Value = serde_json::from_slice(&out).unwrap_or_default();
            (code, printed, String::from_utf8(err).unwrap())
        };
        let boards = [
            "status:v0.12.0/clock=unset",
            "status:v0.12.0/clock=-90000",
            "status:v0.12.0",
            "status:v0.11.0",
            "status:v0.12.0/clock=unset/rtc=dead",
        ];
        let fake = Fake::new(&boards.join(","));
        // A dry run reads it and says, and sets nothing.
        let (code, dry, err) = timed(&fake, "FAKE0", true);
        let said = (dry["board"].as_str(), dry["due"].as_bool());
        assert_eq!((code, said), (0, (Some("unset"), Some(true))), "{err}");
        assert!(dry["sent"].is_null(), "{dry}");
        // Unset: set, read back in step, and in step from then on.
        let (code, set, err) = timed(&fake, "fake0", false);
        assert_eq!(code, 0, "{err}");
        assert!(set["sent"].is_u64() && set["after"].as_i64().is_some_and(|ms| ms.abs() < 2000), "{set}");
        let words: Vec<&str> = set["lines"].as_array().unwrap().iter().filter_map(|l| l.as_str()).collect();
        assert_eq!((words[0], words[1], words[3]), ("@time", "@time unset", "@time ok"), "{words:?}");
        let (_, again, _) = timed(&fake, "FAKE0", false);
        assert_eq!((again["due"].as_bool(), again["sent"].is_null()), (Some(false), true), "{again}");
        // 90 s behind: set. In step: only asked.
        let (_, behind, _) = timed(&fake, "FAKE1", false);
        assert!(behind["offBy"].as_i64().is_some_and(|ms| ms < -2000) && behind["sent"].is_u64(), "{behind}");
        let (_, in_step, _) = timed(&fake, "FAKE2", false);
        let asked = in_step["lines"].as_array().map(Vec::len);
        assert_eq!((in_step["due"].as_bool(), asked), (Some(false), Some(2)), "{in_step}");
        // Older firmware has no clock verb; a chip that takes nothing.
        let (code, old, err) = timed(&fake, "FAKE3", false);
        assert_eq!((code, old["error"].as_str()), (1, Some("old")));
        assert!(err.contains("older than 0.12.0"), "{err}");
        let (code, dead, _) = timed(&fake, "FAKE4", false);
        assert_eq!((code, dead["error"].as_str()), (1, Some("rtc")));
        // A port no board is on; a board's port another program holds: the
        // port's failure, not the board's.
        let (code, _, err) = timed(&fake, "COM9", false);
        assert!(code == 2 && err.contains("isn't a board's port"), "{err}");
        let (code, held, err) = timed(&Fake::new("busy"), "FAKE0", false);
        assert_eq!((code, held["error"].as_str()), (2, Some("port")), "{err}");
        // Listened to, never reset; set twice.
        let trace = fake.trace().lock().unwrap().clone();
        assert!(trace.iter().all(|t| t.starts_with("listen ") || t.starts_with("time ")), "{trace:?}");
        assert_eq!(trace.iter().filter(|t| t.starts_with("time ")).count(), 2, "{trace:?}");
        // The boards' status lines say so: `cid=` and `clock=` from 0.12.0.
        let found = fake.candidates().unwrap();
        let heard = desk::listen_all(&fake, &found, Duration::from_millis(800), listen::tests::QUICK);
        let clocks: Vec<Option<board::Clock>> = heard
            .iter()
            .map(|b| match &b.heard {
                Heard::Status(s) => Some(s.clock),
                _ => None,
            })
            .collect();
        use board::Clock::{Ok as Set, Unknown, Unset};
        assert_eq!(clocks, [Some(Set), Some(Set), Some(Set), Some(Unknown), Some(Unset)]);
        let Heard::Status(newer) = &heard[2].heard else { panic!("a status") };
        assert_eq!(newer.cid, board::CardRoot::None, "a card no transfer wrote");

        let parsed = Line::try_parse_from(["mstream-player", "time", "COM3", "--dry-run"]).map(|l| l.what);
        let Ok(DeviceCmd::Time(args)) = parsed else { panic!("device time") };
        assert_eq!((args.port.as_str(), args.dry_run), ("COM3", true));
        assert!(Line::try_parse_from(["mstream-player", "time"]).is_err(), "a port, always");
        use clap::CommandFactory;
        let help = Line::command().render_help().to_string();
        assert!(help.contains("releases"), "{help}");
        assert!(!help.lines().any(|l| l.trim_start().starts_with("time")), "{help}");
    }

    /// `device format` (clause 66): the board asked over USB with no reset,
    /// its card said first — what is lost — then the player's answer, the
    /// progress and the card it made. Exit 0 formatted; 1 declined, timed
    /// out, refused or failed, or several boards and no `--port`; 2 no
    /// board.
    #[test]
    fn device_format_asks_the_player_and_says_its_answer_the_progress_and_the_card_it_made() {
        let _en = crate::setup::tests::in_locale("en");
        let run = |fake: &Fake, port: Option<&str>| {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = format_on(fake, port, QUICK, &mut out, &mut err);
            (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
        };
        let fake = Fake::new("status:v0.12.0/card=exfat/free=?/tracks=-").with_pace(Duration::from_millis(300));
        let (code, out, err) = run(&fake, None);
        assert_eq!(code, 0, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        let lost = "FAKE0: an exFAT card — the player reads FAT32 only (63.9 GB). Formatting erases all of it, \
                    even what the player can't read.";
        assert_eq!(lines[0], lost);
        let asked = "Asked: confirm on FAKE0's screen, Format... then its red button held 2 s. Nothing is erased \
                     until you do.";
        assert_eq!(lines[1], asked);
        assert_eq!(lines[2], "Confirmed: FAKE0 restarts and formats the card.");
        assert!(lines.contains(&"  50%") && lines.contains(&"  100%"), "{out}");
        let done = lines.iter().find(|l| l.starts_with("Formatted FAKE0's card as FAT32 in ")).expect(&out);
        assert!(done.ends_with(": 63.9 GB, 63.8 GB free, 32 KB clusters."), "{done}");
        assert!(err.is_empty(), "{err}");
        let trace = fake.trace().lock().unwrap().clone();
        assert!(trace.iter().all(|t| t.starts_with("listen ") || t == "format FAKE0"), "never a reset: {trace:?}");
        // What the board says of its card since: an empty FAT32 card.
        let (_, lines) = listed(&fake, false);
        assert!(lines[0].ends_with("· card 63.8 GB free of 63.9 GB, no tracks"), "{lines:?}");

        // The player's no, and its minute: said, and exit 1.
        let declined = run(&Fake::new("status:v0.12.0/format=no"), None);
        assert_eq!(declined.0, 1);
        assert!(declined.1.ends_with("Not formatted: it was cancelled on FAKE0's screen.\n"), "{declined:?}");
        let late = run(&Fake::new("status:v0.12.0/format=late"), None);
        assert!(late.1.ends_with("Not formatted: nobody confirmed on the player within a minute.\n"), "{late:?}");
        // Refused at once, at the second answer, and after the restart.
        let refused = |spec: &str| {
            let (code, _, err) = run(&Fake::new(spec).with_pace(Duration::from_millis(200)), None);
            assert_eq!(code, 1, "{spec}: {err}");
            err.trim_end().trim_start_matches("mstream-player: ").to_string()
        };
        assert_eq!(refused("status:v0.12.0/card=none"), "Not asked: FAKE0: no card answers in its slot.");
        let viz = "Not asked: FAKE0: a computer drives its dancer; end that first.";
        assert_eq!(refused("status:v0.12.0/state=host"), viz);
        let size = "Not formatted: FAKE0: FAT32 can't take the card (under 42 MB, or 2 TB and more). Nothing was erased.";
        assert_eq!(refused("status:v0.12.0/format=second-size"), size);
        let write = "Couldn't format the card in FAKE0: the card refused a write.";
        assert_eq!(refused("status:v0.12.0/card=gpt/format=fail-write"), write);
        // Older firmware: the pin has no format; one that can't say its card.
        let pin = "FAKE0 can't format a card: its firmware is older than 0.12.0.";
        assert_eq!(refused("status:v0.11.0/card=exfat"), pin);
        let old = refused("old:v0.8.0");
        assert!(old.starts_with("FAKE0: v0.8.0, update to") && old.ends_with("formats its card."), "{old}");

        // Several boards and no --port: named, as flash --yes names them.
        let two = Fake::new("status:v0.12.0/format=no,status:v0.12.0/card=ntfs");
        let (code, out, _) = run(&two, None);
        assert_eq!(code, 1);
        assert!(out.starts_with("Several boards look like a Core2") && out.contains("Pass --port"), "{out}");
        assert!(!two.trace().lock().unwrap().iter().any(|t| t.starts_with("format")), "neither asked");
        let (code, out, _) = run(&two, Some("fake1"));
        assert_eq!(code, 0, "{out}");
        assert!(out.starts_with("FAKE1: an NTFS card"), "{out}");
        // No board; a port no board is on.
        let (code, out, _) = run(&Fake::new("nodevice"), None);
        assert_eq!((code, out.trim()), (2, "No Core2 found"));
        let (code, _, err) = run(&two, Some("COM9"));
        assert!(code == 2 && err.contains("isn't a board's port"), "{err}");

        // In the help, with --port and no --yes.
        let parsed = Line::try_parse_from(["mstream-player", "format", "--port", "COM3"]).map(|l| l.what);
        let Ok(DeviceCmd::Format(args)) = parsed else { panic!("device format") };
        assert_eq!(args.port.as_deref(), Some("COM3"));
        assert!(Line::try_parse_from(["mstream-player", "format", "--yes"]).is_err(), "the player confirms");
        use clap::CommandFactory;
        let help = Line::command().render_help().to_string();
        assert!(help.lines().any(|l| l.trim_start().starts_with("format ")), "{help}");
    }

    #[test]
    fn device_list_words_a_count_under_way_an_empty_card_and_a_dev_build() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        let spec = format!("status:{pin}/free=counting/tracks=building,status:{pin}/card=none/size=-/tracks=-,old:v0.6.0-37-g221d99d");
        let (_, lines) = listed(&Fake::new(&spec), false);
        let counting = format!("{pin}, up to date · card 63.9 GB, counting free space, tracks being indexed");
        assert_eq!(lines[0], format!("FAKE0 · CH9102 · serial FAKE0 · {counting}"));
        assert_eq!(lines[1], format!("FAKE1 · CH9102 · serial FAKE1 · {pin}, up to date · no card"));
        let dev = format!("v0.6.0-37-g221d99d, a development build — update to {pin} · card not reported by v0.6.0-37-g221d99d");
        assert_eq!(lines[2], format!("FAKE2 · CH9102 · serial FAKE2 · {dev}"));
    }

    /// `device flash --yes` on the fake, the pin's builds on the shelf: the
    /// exit code, stdout, stderr, and the fake for its trace.
    fn flashed(spec: &str, port: Option<&str>, mode: Mode) -> (i32, String, String, Fake) {
        flashed_with(spec, (None, None), port, mode)
    }

    /// …with `--release` and `--flash-mode`, handed to the worker as
    /// [`lines`] hands them.
    fn flashed_with(
        spec: &str,
        (release, flash_mode): (Option<&str>, Option<Build>),
        port: Option<&str>,
        mode: Mode,
    ) -> (i32, String, String, Fake) {
        let fake = Fake::new(spec).with_pace(Duration::from_millis(60));
        let setup = desk::Setup {
            supply: Arc::new(Shelf::new()),
            preset: Image::from_flags(None, release.map(str::to_string), flash_mode),
            flags_mode: flash_mode,
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
        let pin = pin();
        let (code, out, err, _) = flashed("fresh", None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "getting the firmware…");
        assert!(lines[1].starts_with(&format!("firmware: {pin} in QIO (release {pin}")), "{out}");
        assert_eq!(lines[2], "looking for a Core2…");
        assert!(out.contains("reaching the board's bootloader…\n"), "{out}");
        assert!(out.contains("board: FAKE0 · CH9102 · esp32 rev 3.1 · 16 MB\n"), "{out}");
        assert!(out.contains(&format!("plan: install, erase first · {pin} in QIO\n")), "the pin, in its default build:\n{out}");
        assert!(out.contains("  50%\n") && out.contains("  100%\n"), "{out}");
        let elf = pin_elf(Build::Qio);
        let done = format!(
            "{pin} is on the board. It is restarting — unplug it when its screen comes up.\n\
             The board reports: mstream-mp3-player {pin} (commit fake000, 2026-10-01), ELF {elf}"
        );
        assert!(out.trim_end().ends_with(&done), "Done, then the board's own first line:\n{out}");
        assert!(err.is_empty(), "{err}");

        let (code, out, err, fake) = flashed("busy", None, ONE);
        assert_eq!(code, 1, "{out}");
        assert!(err.contains("FAKE0 is in use by another program") && err.contains("serial monitor"), "{err}");
        assert!(!fake.trace().lock().unwrap().iter().any(|t| t.starts_with("open")), "nothing tried");
    }

    #[test]
    fn flash_yes_with_several_boards_refuses_and_names_each_boards_firmware() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        let spec = format!("old:v0.7.0,status:{pin}");
        let (code, out, _, fake) = flashed(&spec, None, ONE);
        assert_eq!(code, 1);
        let tail: Vec<&str> = out.lines().skip_while(|l| !l.starts_with("Several")).collect();
        let (behind, current) = (
            format!("  FAKE0 · CH9102 · serial FAKE0 · v0.7.0, update to {pin} · card not reported by v0.7.0"),
            format!("  FAKE1 · CH9102 · serial FAKE1 · {pin}, up to date · card 38.2 GB free of 63.9 GB, 1,284 tracks"),
        );
        assert_eq!(
            tail,
            [
                "Several boards look like a Core2 — which one?",
                &behind,
                &current,
                "pass --port with one of them, or --all to update every one that needs it",
            ]
        );
        assert!(!fake.trace().lock().unwrap().iter().any(|t| t.starts_with("open")), "no board reset");

        let named = Mode { named: true, ..ONE };
        let (code, out, _, fake) = flashed(&spec, Some("FAKE0"), named);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plan: update, no erase"), "{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some(pin));
    }

    #[test]
    fn flash_yes_all_updates_each_board_that_needs_it_and_names_the_rest() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        let all = Mode { all: true, ..ONE };
        let spec = format!("old:v0.7.0,status:{pin},old:v0.6.0,silent");
        let (code, out, err, fake) = flashed(&spec, None, all);
        assert_eq!(code, 0, "{out}{err}");
        let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle:?} in:\n{out}"));
        assert!(at(&format!("FAKE0 · v0.7.0 → {pin}\n")) < at(&format!("FAKE2 · v0.6.0 → {pin}\n")), "one after another");
        assert!(out.contains("  reaching the board's bootloader…\n"), "the steps under each port:\n{out}");
        assert_eq!(out.matches(&format!("  done: {pin} written and checked in ")).count(), 2, "{out}");
        let summary = "updated 2 of 2 · skipped: FAKE1 (up to date), FAKE3 (not answering)";
        assert!(out.trim_end().ends_with(summary), "{out}");
        assert_eq!(fake.version_on("FAKE3").as_deref(), Some("v0.7.0"), "not answering: left as it was");

        let (code, out, err, fake) = flashed("old:v0.7.0/fail=write,old:v0.6.0", None, all);
        assert_eq!(code, 1, "{out}");
        assert!(err.contains("the board stopped answering"), "{err}");
        assert!(out.trim_end().ends_with("updated 0 of 2"), "{out}");
        assert_eq!(fake.version_on("FAKE1").as_deref(), Some("v0.6.0"), "never started");

        let (code, out, _, _) = flashed(&format!("status:{pin}"), None, all);
        assert_eq!(code, 0, "none needed one");
        assert!(out.trim_end().ends_with("updated 0 of 0 · skipped: FAKE0 (up to date)"), "{out}");
    }

    #[test]
    fn flash_yes_on_a_board_that_runs_dio_writes_the_pins_dio_build() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        let (code, out, err, fake) = flashed("old:v0.7.0/mode=dio", None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains(&format!("firmware: {pin} in DIO (release {pin}")), "its own build fetched:\n{out}");
        assert!(out.contains(&format!("plan: update, no erase · {pin} in DIO\n")), "{out}");
        let elf = format!("ELF {}", pin_elf(Build::Dio));
        assert!(out.trim_end().ends_with(&elf), "the board reports the DIO build's ELF:\n{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some(pin));
        let all = Mode { all: true, ..ONE };
        let (code, out, _, _) = flashed("old:v0.7.0,old:v0.6.0/mode=dio", None, all);
        assert_eq!(code, 0, "{out}");
        let (qio, dio) = (format!("FAKE0 · v0.7.0 → {pin}\n"), format!("FAKE1 · v0.6.0 → {pin} · DIO\n"));
        assert!(out.contains(&qio) && out.contains(&dio), "{out}");
    }

    #[test]
    fn flash_yes_on_a_board_that_keeps_restarting_says_so_and_names_the_dio_build() {
        let _en = crate::setup::tests::in_locale("en");
        let (code, out, err, fake) = flashed("old:v0.7.0/loop=qio", None, ONE);
        assert_eq!(code, 1, "written and checked is not running:\n{out}{err}");
        assert!(out.contains(&format!("{} is on the board.", pin())), "{out}");
        let line = "mstream-player: the board keeps restarting: 3 restarts in 6 s, and never its first line \
                    — its flash may not run QIO; --flash-mode dio writes the DIO build\n";
        assert_eq!(err, line);
        assert_eq!(fake.trace().lock().unwrap().iter().filter(|t| t.starts_with("open")).count(), 1, "nothing more");
    }

    #[test]
    fn flash_yes_with_another_release_says_what_it_writes_and_still_measures_against_the_pin() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        let release = (Some("v0.7.0"), None);
        let (code, out, err, fake) = flashed_with("old:v0.6.0", release, None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        let plan = format!("plan: update, no erase · v0.7.0 in QIO, release v0.7.0 — not this player's release ({pin})\n");
        assert!(out.contains(&plan), "{out}");
        assert!(out.contains("v0.7.0 is on the board."), "{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.7.0"));
        // Over the pin itself — a board running it, answering as it does —
        // the release is a step back, and says so.
        let (code, out, _, _) = flashed_with(&format!("ours:{pin}"), release, None, ONE);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plan: go back, no erase · v0.7.0 in QIO, release v0.7.0"), "{out}");
    }

    #[test]
    fn flash_yes_with_a_release_and_no_mode_keeps_a_board_on_dio_and_a_named_mode_is_obeyed() {
        let _en = crate::setup::tests::in_locale("en");
        let pin = pin();
        // `--release` alone: the board's own build, as its ELF says.
        let dio = "old:v0.6.0/mode=dio";
        let (code, out, err, fake) = flashed_with(dio, (Some("v0.7.0"), None), None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        let plan = "plan: update, no erase · v0.7.0 in DIO, release v0.7.0";
        assert!(out.contains(plan), "never QIO put back:\n{out}");
        assert_eq!(fake.version_on("FAKE0").as_deref(), Some("v0.7.0"));
        // The pin's tag alone: the pin in the board's own build.
        let on_v070 = "old:v0.7.0/mode=dio";
        let (code, out, err, _) = flashed_with(on_v070, (Some(pin), None), None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains(&format!("plan: update, no erase · {pin} in DIO\n")), "{out}");
        // `--flash-mode qio` named: QIO, as asked, and the plan says so.
        let named = (Some("v0.7.0"), Some(Build::Qio));
        let (code, out, err, _) = flashed_with(dio, named, None, ONE);
        assert_eq!(code, 0, "{out}{err}");
        assert!(out.contains("plan: update, no erase · v0.7.0 in QIO, release v0.7.0"), "{out}");
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
        let pin = pin();
        let (_, lines) = listed(&Fake::new("old:v0.7.0/mode=dio,old:v0.7.0"), false);
        assert_eq!(lines[0], format!("FAKE0 · CH9102 · serial FAKE0 · v0.7.0 · DIO, update to {pin} · card not reported by v0.7.0"));
        assert_eq!(lines[1], format!("FAKE1 · CH9102 · serial FAKE1 · v0.7.0, update to {pin} · card not reported by v0.7.0"));
    }

    #[test]
    fn device_releases_lists_newest_first_with_what_this_computer_has_and_hides_the_pre_release() {
        // GitHub's list of 2026-10-10 with this player's pin on top of it
        // and a release after the pin above that — on dates of the test's
        // own — so the marks follow the pin, whichever release it is.
        let (pin, after) = (pin(), past_pin());
        let release = |tag: &str, date: &str| Release {
            tag: tag.into(),
            title: format!("mStream Player {tag} (beta)"),
            date: date.into(),
            pre: false,
            images: Images::Both,
        };
        let mut list = firmware::tests::listed();
        list.retain(|r| r.tag != pin);
        list.splice(0..0, [release(&after, "2099-01-02"), release(pin, "2099-01-01")]);
        let mut shelf = Shelf::new();
        *shelf.list.lock().unwrap() = Ok(list);
        shelf.cached = vec![
            Cached { tag: pin.into(), modes: vec![Build::Qio], since: None },
            Cached { tag: "v0.5.0".into(), modes: vec![Build::Dio], since: None },
        ];
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(releases_on(&shelf, false, &mut out, &mut err), 0);
        assert!(err.is_empty());
        let out = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        let (newer, ours) = (
            format!("{after:<15} 2099-01-02  QIO and DIO · newer than this player"),
            format!("{pin:<15} 2099-01-01  QIO and DIO · this player's release · on this computer: QIO"),
        );
        assert_eq!(
            lines,
            [
                &newer,
                &ours,
                "v0.8.0          2026-10-10  QIO and DIO",
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
        let mut shelf = Shelf::new();
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
        let (code, out, _, _) = flashed("nodevice", None, ONE);
        assert_eq!(code, 1);
        assert!(out.contains("No Core2 found\n") && out.contains("Serial ports seen: FAKECOM1"), "{out}");
    }
}
