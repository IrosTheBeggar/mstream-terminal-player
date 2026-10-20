//! The cards' command line (mp3-player-screen contract, clauses 64–67):
//! `device cards` lists the cards in this computer's readers, and `device
//! card <drive> status|setup|sync|plays|eject` does to one of them what the
//! Card view does — on the same transfer worker the GUI runs (clause 55),
//! so the plan, the gates and the run are the tab's own. Each gate is a
//! flag that names what it lets through, never merely "yes" (clause 65);
//! the lines are plain English whatever the locale, as `device list`'s
//! are: lines a script reads (clause 67). A card's sizes are the Card
//! view's words (`dev.card_size`, `dev.card_free`, `dev.card.fact_drive_kb`),
//! which the command line, setting no locale, prints in English.
//!
//! Exit codes: 0 done; 1 refused (a gate its flag didn't let through, no
//! `--yes`, nothing to sync from) or failed; 2 the volumes can't be listed,
//! or no card answers to `<drive>`; 3 paused or stopped — run it again
//! later, it goes on from the last save.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use rust_i18n::t;

use super::engine::Removal;
use super::plan::TRACK_CAP;
use super::state::{Convert, LabelFault};
use super::volume::{Fs, Refusal, Table};
use super::worker::{
    self, Allow, CardView, Cmd, Ended, Event, Fold, Gate, Link, LogItem, Next, Note, Pause, Phase, Planned,
    Read, Server, ServerLink, Setup, Want,
};
use crate::admin::fmt_count;
use crate::api::urls::{TransferBitrate, TransferCodec};
use crate::device::board::gb;

/// How long the cards are read and their servers asked, at most.
const SETTLE_WITHIN: Duration = Duration::from_secs(120);
/// A card is settled once nothing more was told of it for this long: a job
/// that ends and the one queued behind it are told together.
const QUIET: Duration = Duration::from_millis(400);
/// A set-up, a settings change, the plays or an eject, at most.
const JOB_WITHIN: Duration = Duration::from_secs(300);
/// The column of `status`'s values.
const KEY_W: usize = 12;

/// What the command line does to one card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum What {
    Status,
    Setup,
    Sync,
    Plays,
    Eject,
}

impl What {
    /// As the command line spells it.
    pub(crate) fn word(&self) -> &'static str {
        match self {
            What::Status => "status",
            What::Setup => "setup",
            What::Sync => "sync",
            What::Plays => "plays",
            What::Eject => "eject",
        }
    }
}

/// The flags of `device card` (clause 65): the gates' and the settings'.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    pub(crate) yes: bool,
    /// The removals gate's (and the stronger one's): the plan's exact count.
    pub(crate) allow_removals: Option<usize>,
    pub(crate) list_removals: bool,
    /// Another saved server, by its name or its address.
    pub(crate) server: Option<String>,
    pub(crate) take_over: bool,
    pub(crate) adopt: bool,
    pub(crate) keep_mine: bool,
    pub(crate) through_parent: bool,
    pub(crate) new_identity: bool,
    pub(crate) name: Option<String>,
    pub(crate) libraries: Option<Vec<String>>,
    pub(crate) convert: Option<Convert>,
    pub(crate) to: Option<(TransferCodec, Option<TransferBitrate>)>,
    pub(crate) removals: Option<Removal>,
    pub(crate) covers: Option<bool>,
    pub(crate) auto_dj: Option<bool>,
    pub(crate) for_someone_else: Option<bool>,
    pub(crate) budget_mb: Option<u64>,
}

impl Flags {
    /// A setting is changed by a flag.
    fn changes(&self) -> bool {
        self.name.is_some()
            || self.server.is_some()
            || self.libraries.is_some()
            || self.convert.is_some()
            || self.to.is_some()
            || self.removals.is_some()
            || self.covers.is_some()
            || self.auto_dj.is_some()
            || self.for_someone_else.is_some()
            || self.budget_mb.is_some()
    }

    /// `want` as the flags change it.
    fn apply(&self, want: &mut Want) {
        if let Some(name) = &self.name {
            want.name = name.trim().to_string();
        }
        if let Some(libraries) = &self.libraries {
            want.libraries = Some(libraries.clone());
        }
        if let Some(convert) = self.convert {
            want.convert = convert;
        }
        if let Some((codec, bitrate)) = self.to {
            want.codec = codec;
            if let Some(bitrate) = bitrate {
                want.bitrate = bitrate;
            }
        }
        if let Some(removal) = self.removals {
            want.removal = removal;
        }
        if let Some(covers) = self.covers {
            want.covers = covers;
        }
        if let Some(auto_dj) = self.auto_dj {
            want.auto_dj = auto_dj;
        }
        if let Some(gift) = self.for_someone_else {
            want.for_someone_else = gift;
        }
        if self.budget_mb.is_some() {
            want.budget_mb = self.budget_mb;
        }
    }
}

/// `--to`'s word: `opus`, `opus-192`, `mp3-320` (clause 48's Into list).
pub(crate) fn codec_of(word: &str) -> Option<(TransferCodec, Option<TransferBitrate>)> {
    let word = word.trim().to_ascii_lowercase();
    let (codec, kbps) = match word.split_once(['-', ':']) {
        Some((codec, kbps)) => (codec, Some(kbps.trim_end_matches('k'))),
        None => (word.as_str(), None),
    };
    let codec: TransferCodec = codec.parse().ok()?;
    let bitrate = match kbps {
        Some(kbps) => Some(TransferBitrate::from_kbps(kbps.parse().ok()?)?),
        None => None,
    };
    // Opus goes up to 192k (clause 48: the Into list).
    if codec == TransferCodec::Opus && bitrate.is_some_and(|b| b.kbps() > 192) {
        return None;
    }
    Some((codec, bitrate))
}

/// The terminal's side: the lines, the question, Ctrl+C.
pub(crate) struct Io<'a> {
    pub(crate) out: &'a mut dyn Write,
    pub(crate) err: &'a mut dyn Write,
    /// The listener's answer to "go on? [y/N]"; None off a terminal.
    pub(crate) ask: &'a mut dyn FnMut(&str) -> Option<bool>,
    /// How many times Ctrl+C was pressed (clause 57's rule): once stops a
    /// sync at its next save.
    pub(crate) ctrl_c: &'a AtomicU8,
}

impl Io<'_> {
    fn say(&mut self, line: &str) {
        let _ = writeln!(self.out, "{line}");
    }

    fn warn(&mut self, line: &str) {
        let _ = writeln!(self.err, "mstream-player: {line}");
    }
}

// ── The worker, driven ──────────────────────────────────────────────────────

/// The transfer worker for one command: its link, its reports folded.
struct Session {
    link: Link,
    events: Receiver<Event>,
    fold: Fold,
    /// When the last report came.
    told: Instant,
}

impl Session {
    fn start(setup: Setup, servers: Vec<ServerLink>) -> Session {
        let (link, events) = worker::spawn(setup);
        link.send(Cmd::Servers(servers));
        Session { link, events, fold: Fold::default(), told: Instant::now() }
    }

    /// Folds what was told within `wait`.
    fn pump(&mut self, wait: Duration) {
        let Ok(first) = self.events.recv_timeout(wait) else { return };
        let mut next = Some(first);
        while let Some(event) = next {
            // The readers are looked at every few seconds, said or not: what
            // a card is told is what the quiet waits for.
            if matches!(event, Event::Card(_) | Event::Progress { .. } | Event::Gone { .. }) {
                self.told = Instant::now();
            }
            self.fold.apply(event);
            next = self.events.try_recv().ok();
        }
    }

    /// Folds until the readers were looked at and the cards (or the one at
    /// `key`) are read and their servers asked, quiet a moment. Err: the
    /// volumes can't be listed.
    fn settle(&mut self, key: Option<&str>) -> Result<(), String> {
        let t0 = Instant::now();
        loop {
            self.pump(Duration::from_millis(50));
            if let Some(why) = &self.fold.failed {
                return Err(why.clone());
            }
            let cards = self.fold.cards.iter().filter(|c| key.is_none_or(|k| c.key == k));
            let settled = self.fold.watched && cards.into_iter().all(settled);
            if (settled && self.told.elapsed() >= QUIET) || t0.elapsed() >= SETTLE_WITHIN {
                return Ok(());
            }
        }
    }

    fn card(&self, key: &str) -> Option<&CardView> {
        self.fold.cards.iter().find(|c| c.key == key)
    }

    /// Lets the worker end: nothing runs by now.
    fn end(self) {
        self.link.send(Cmd::Quit);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(2) {
            match self.events.recv_timeout(Duration::from_millis(50)) {
                Ok(Event::Idle) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                _ => {}
            }
        }
    }
}

/// Read, and its server asked or not to be asked: nothing pending.
fn settled(card: &CardView) -> bool {
    card.read != Read::Reading && card.busy.is_none() && card.next != Next::Asking
}

// ── device cards ────────────────────────────────────────────────────────────

/// `device cards` (clause 64): every card in a reader, one line each, and
/// every removable volume the player can't use, with why. It reads only,
/// asking each card's server once. Exit 0 with a card, 1 with none, 2 when
/// the volumes can't be listed.
pub(crate) fn cards(setup: Setup, servers: Vec<ServerLink>, io: &mut Io) -> i32 {
    let mut session = Session::start(setup, servers);
    if let Err(why) = session.settle(None) {
        io.warn(&format!("the volumes can't be listed: {why}"));
        return 2;
    }
    let lines: Vec<String> = session.fold.cards.iter().filter(|c| !c.out).map(card_line).collect();
    session.end();
    if lines.is_empty() {
        io.say("No card in a reader.");
        return 1;
    }
    for line in lines {
        io.say(&line);
    }
    0
}

/// `E: · Car music · 63.8 GB FAT32 · Home · 4,812 tracks · last sync 3
/// days ago · next sync 12 new, 2 to remove, 37 plays to send`.
fn card_line(card: &CardView) -> String {
    let mut parts = vec![card.drive()];
    if let Some(name) = card.name() {
        parts.push(name);
    }
    parts.push(volume_words(card));
    parts.extend(state_words(card));
    parts.join(" · ")
}

/// `63.8 GB FAT32`, `128.0 GB exFAT on GPT`. The size is the Card view's
/// words (`dev.card_size`), English as the command line sets no locale.
fn volume_words(card: &CardView) -> String {
    let v = &card.found.volume;
    let mut words = v.fs.name().to_string();
    if v.table == Table::Gpt {
        words.push_str(" on GPT");
    }
    match v.space {
        Some(space) => format!("{} {words}", t!("dev.card_size", size = gb(space.size))),
        None => words,
    }
}

/// The rest of a card's line: its server and what a sync would do, or why
/// it can't be synced.
fn state_words(card: &CardView) -> Vec<String> {
    let facts = match &card.read {
        Read::Refused => return vec![format!("can't be used: {}", refusals(card))],
        Read::Failed(why) => return vec![format!("can't be read: {why}")],
        Read::Reading => return vec!["not read yet".to_string()],
        Read::Read(facts) => facts,
    };
    if !facts.newer.is_empty() {
        return vec!["written by a newer mStream Player: this one can't sync it".to_string()];
    }
    if facts.copied {
        return vec!["a copy of another card: setup --new-identity gives it its own".to_string()];
    }
    if !facts.set_up() {
        let mut words = vec!["new card, not set up: setup sets it up".to_string()];
        if facts.music.unlisted > 0 {
            let n = fmt_count(facts.music.unlisted.into());
            words.push(format!("{n} music files mStream didn't put there"));
        }
        return words;
    }
    let mut words = vec![server_words(card), tracks(facts.tracks)];
    words.push(format!("last sync {}", last_words(card)));
    words.push(format!("next sync {}", next_words(card)));
    if facts.check {
        words.push("check the disk first".to_string());
    }
    words
}

/// Why the player can't use a volume, in words.
fn refusals(card: &CardView) -> String {
    let words: Vec<String> = card.found.refusals.iter().map(refusal_words).collect();
    if words.is_empty() { "refused".to_string() } else { words.join("; ") }
}

fn refusal_words(refusal: &Refusal) -> String {
    match refusal {
        Refusal::System(_) => "one of this computer's system disks".to_string(),
        Refusal::Fixed => "a fixed disk, not a card".to_string(),
        Refusal::Shared(fs) => format!("the card holds other filesystems too ({})", fs.join(", ")),
        Refusal::Filesystem(Fs::ExFat) => {
            "exFAT: the player reads FAT32 only (device format formats it in the player)".to_string()
        }
        Refusal::Filesystem(fs) => format!("{}: the player reads FAT32 only", fs.name()),
        Refusal::Table(table) => {
            format!("{} partition table: the player reads MBR cards only", table.name().to_uppercase())
        }
        Refusal::ReadOnly => "write-protected".to_string(),
        Refusal::NotUtf8 => "mounted without UTF-8 names: remount it with iocharset=utf8".to_string(),
    }
}

/// The card's server: `Home`, or the one it names with no connection here.
fn server_words(card: &CardView) -> String {
    match &card.server {
        Server::Saved(link) | Server::New(Some(link)) => link.name.clone(),
        Server::Unknown(nick) => format!("filled from {nick}, which this computer has no connection to"),
        Server::New(None) => "no server saved on this computer".to_string(),
    }
}

fn tracks(n: usize) -> String {
    match n {
        0 => "no tracks".to_string(),
        1 => "1 track".to_string(),
        n => format!("{} tracks", fmt_count(n as u64)),
    }
}

/// `3 days ago`, `never`, `just now (verified)`.
fn last_words(card: &CardView) -> String {
    if let Some(Ended::Done { .. }) = &card.ended {
        return "just now, verified".to_string();
    }
    let Some(facts) = card.facts() else { return "never".to_string() };
    match facts.committed {
        Some(_) if facts.tracks == 0 => "never".to_string(),
        Some(at) => format!("{} ({})", date(at), ago(at, now())),
        None => "never".to_string(),
    }
}

/// What the next sync would do, from the server's last answer.
fn next_words(card: &CardView) -> String {
    match &card.next {
        Next::Plan(plan) => plan_words(plan, &server_name(card)),
        Next::Unasked => match card.server {
            Server::Unknown(_) | Server::New(None) => "not asked: no server to ask".to_string(),
            _ => "not asked".to_string(),
        },
        Next::Asking => "not answered yet".to_string(),
        Next::Unreached { why, .. } => format!("{} didn't answer ({why})", server_name(card)),
        Next::SignIn => format!("{} asks this computer to sign in again", server_name(card)),
        Next::Refused(why) => why.clone(),
    }
}

/// `12 new, 2 to remove, 37 plays to send`, or `nothing new on Home`.
fn plan_words(plan: &Planned, server: &str) -> String {
    let mut parts = Vec::new();
    if plan.new > 0 {
        parts.push(format!("{} new ({} GB)", fmt_count(plan.new as u64), gb(plan.bytes)));
    }
    if !plan.removals.is_empty() {
        parts.push(format!("{} to remove", fmt_count(plan.removals.len() as u64)));
    }
    if plan.plays > 0 {
        parts.push(format!("{} plays to send", fmt_count(plan.plays as u64)));
    }
    if parts.is_empty() {
        return format!("nothing new on {server}");
    }
    parts.join(", ")
}

fn server_name(card: &CardView) -> String {
    match &card.server {
        Server::Saved(link) | Server::New(Some(link)) => link.name.clone(),
        Server::Unknown(nick) => nick.clone(),
        Server::New(None) => "its server".to_string(),
    }
}

// ── device card <drive> … ───────────────────────────────────────────────────

/// `device card <drive> status|setup|sync|plays|eject` (clause 65).
pub(crate) fn card(
    setup: Setup,
    servers: Vec<ServerLink>,
    drive: &str,
    what: &What,
    flags: &Flags,
    io: &mut Io,
) -> i32 {
    let mut session = Session::start(setup, servers.clone());
    if let Err(why) = session.settle(None) {
        io.warn(&format!("the volumes can't be listed: {why}"));
        return 2;
    }
    let key = match pick(&session.fold.cards, drive) {
        Ok(key) => key,
        Err(why) => {
            io.warn(&why);
            return 2;
        }
    };
    let code = match what {
        What::Status => status(&session, &key, io),
        What::Setup => set_up(&mut session, &key, flags, &servers, io),
        What::Sync => sync(&mut session, &key, flags, &servers, io),
        What::Plays => plays(&mut session, &key, flags, io),
        What::Eject => eject(&mut session, &key, io),
    };
    session.end();
    code
}

/// The card `<drive>` names: a drive letter (`E`, `E:`, `E:\`), a mount
/// path, or a card's name when one card has it.
fn pick(cards: &[CardView], drive: &str) -> Result<String, String> {
    let bare = |s: &str| s.trim().trim_end_matches(['\\', '/']).trim_end_matches(':').to_lowercase();
    let asked = bare(drive);
    let listed: Vec<&CardView> = cards.iter().filter(|c| !c.out).collect();
    let by_place = listed.iter().find(|c| bare(&c.key) == asked || bare(&c.drive()) == asked);
    if let Some(card) = by_place {
        return Ok(card.key.clone());
    }
    let wanted = drive.trim().to_lowercase();
    let named: Vec<&&CardView> =
        listed.iter().filter(|c| c.name().is_some_and(|n| n.to_lowercase() == wanted)).collect();
    match named.as_slice() {
        [one] => Ok(one.key.clone()),
        [] if listed.is_empty() => Err(format!("{drive}: no card in a reader")),
        [] => {
            let drives: Vec<String> = listed.iter().map(|c| c.drive()).collect();
            Err(format!("{drive} isn't a card in a reader here (these are: {})", drives.join(", ")))
        }
        _ => {
            let drives: Vec<String> = named.iter().map(|c| c.drive()).collect();
            let (n, drives) = (named.len(), drives.join(", "));
            Err(format!("{n} cards are called {drive}: name one by its drive ({drives})"))
        }
    }
}

/// `status`: the Card view's facts as `key  value` lines; it writes nothing.
fn status(session: &Session, key: &str, io: &mut Io) -> i32 {
    let Some(card) = session.card(key) else { return 2 };
    for (k, v) in facts_rows(card) {
        io.say(&format!("{k:<KEY_W$}{v}"));
    }
    0
}

/// The rows `status` prints.
fn facts_rows(card: &CardView) -> Vec<(&'static str, String)> {
    let v = &card.found.volume;
    let mut rows = vec![("drive", card.drive())];
    let mut volume = volume_words(card);
    // The free space and the clusters in the Card view's words (its Space
    // row's and its Drive fact's), English as the command line sets no
    // locale (clause 67).
    if let Some(space) = v.space {
        let free = t!("dev.card_free", free = gb(space.free));
        let kb = t!("dev.card.fact_drive_kb", kb = space.cluster / 1024);
        volume.push_str(&format!(" · {free} · {kb}"));
    }
    rows.push(("volume", volume));
    let facts = match &card.read {
        Read::Read(facts) => facts,
        _ => {
            rows.push(("state", state_words(card).join(" · ")));
            return rows;
        }
    };
    if let Some(name) = facts.name() {
        rows.push(("name", name.to_string()));
    }
    if let Some((id, revision, _)) = facts.root {
        let mut words = format!("id {id:016x} · revision {revision}");
        if !facts.producer.is_empty() {
            words.push_str(&format!(" · {}", facts.producer));
        }
        rows.push(("card", words));
    }
    if !facts.set_up() {
        rows.push(("state", state_words(card).join(" · ")));
        return rows;
    }
    rows.push(("server", server_words(card)));
    let libraries = facts.libraries();
    let holds = match libraries.is_empty() {
        true => "every library its server lists".to_string(),
        false => libraries.join(", "),
    };
    rows.push(("holds", holds));
    rows.push(("tracks", fmt_count(facts.tracks as u64)));
    rows.push(("last sync", last_words(card)));
    rows.push(("next sync", next_words(card)));
    if let Next::Plan(plan) = &card.next {
        if let Some(fit) = plan.fit.as_ref() {
            let size = v.space.map_or(0, |s| s.size);
            let used = size.saturating_sub(fit.room).saturating_add(fit.needed);
            let spare = fit.room.saturating_sub(fit.needed);
            let words = if fit.fits() {
                format!("{} of {} GB after this sync · {} GB spare", gb(used), gb(size), gb(spare))
            } else {
                format!("doesn't fit by {} GB", gb(fit.short()))
            };
            rows.push(("fit", words));
        }
        if let Some(first) = plan.removals.first() {
            let more = plan.removals.len() - 1;
            let words = match more {
                0 => first.clone(),
                more => format!("{first} and {} more", fmt_count(more as u64)),
            };
            rows.push(("removes", words));
        }
    }
    if let Some(state) = &facts.state {
        let s = &state.settings;
        let c = s.conversion;
        let codec = codec_words(c.codec, c.bitrate);
        let convert = match c.convert {
            Convert::Unplayable => format!("only what the player can't play, to {codec}"),
            Convert::Everything => format!("everything, to {codec}"),
        };
        rows.push(("converts", convert));
        let removals = match s.removal {
            Removal::Mirror => format!("follow {}", server_name(card)),
            Removal::Never => "never".to_string(),
        };
        rows.push(("removals", removals));
        rows.push(("covers", on_off(s.covers)));
        rows.push(("autodj", on_off(s.auto_dj)));
        rows.push(("for", if s.for_someone_else { "someone else" } else { "me" }.to_string()));
    }
    let d = &facts.device;
    let player = if d.present {
        format!("Core2 {}: {} (its device.txt)", d.firmware, d.codecs)
    } else {
        "no device.txt yet: a player's first defaults, MP3 and FLAC".to_string()
    };
    rows.push(("player", player));
    if let Some((i, n)) = facts.set {
        rows.push(("set", format!("card {i} of {n}")));
    }
    rows
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.to_string()
}

/// `Opus 192k`.
fn codec_words(codec: TransferCodec, bitrate: TransferBitrate) -> String {
    let name = match codec {
        TransferCodec::Opus => "Opus",
        TransferCodec::Mp3 => "MP3",
    };
    format!("{name} {}", bitrate.as_str())
}

/// The question, or `--yes`: true to go on. Off a terminal without `--yes`
/// the plan was all.
fn go_on(flags: &Flags, io: &mut Io, what: &str) -> bool {
    if flags.yes {
        return true;
    }
    match (io.ask)("go on? [y/N] ") {
        Some(yes) => {
            if !yes {
                io.say("Nothing written.");
            }
            yes
        }
        None => {
            io.say(&format!("Nothing written: add --yes to {what}."));
            false
        }
    }
}

/// The saved server `--server` names, by its name or its address.
fn server_named<'a>(servers: &'a [ServerLink], name: &str) -> Result<&'a ServerLink, String> {
    let name = name.trim();
    let address = name.trim_end_matches('/');
    let same = |s: &&ServerLink| {
        s.name.eq_ignore_ascii_case(name) || s.id.trim_end_matches('/').eq_ignore_ascii_case(address)
    };
    servers.iter().find(same).ok_or_else(|| {
        let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
        if names.is_empty() {
            format!("--server {name}: no server is saved on this computer")
        } else {
            format!("--server {name} isn't a server saved here ({})", names.join(", "))
        }
    })
}

/// Waits for a job on the card to end: true once it has (its card told
/// with no job, or a note said how it went).
fn job_done(session: &mut Session, key: &str, began: &mut bool) -> bool {
    session.pump(Duration::from_millis(50));
    let busy = session.card(key).is_some_and(|c| c.busy.is_some());
    *began |= busy;
    *began && !busy
}

/// The notes told since `from`.
fn notes_since(session: &Session, from: usize) -> Vec<Note> {
    session.fold.notes.iter().skip(from).cloned().collect()
}

/// `setup` (clause 50): a new card set up with the flags' settings, or a
/// set-up card's settings changed (`state.json` only), or a copied card
/// given an identity of its own.
fn set_up(session: &mut Session, key: &str, flags: &Flags, servers: &[ServerLink], io: &mut Io) -> i32 {
    let Some(card) = session.card(key).cloned() else { return 2 };
    let facts = match &card.read {
        Read::Read(facts) => facts.clone(),
        _ => {
            io.warn(&format!("{}: {}", card.drive(), state_words(&card).join(" · ")));
            return 1;
        }
    };
    if facts.copied {
        if !flags.new_identity {
            io.say(&format!("{} is a copy of another card: neither would sync rightly.", card.drive()));
            io.say("--new-identity gives it an identity of its own; its music stays as it is.");
            return 1;
        }
        io.say(&format!("Give {} an identity of its own; its music stays as it is.", card.drive()));
        if !go_on(flags, io, "give it one") {
            return 1;
        }
        session.link.send(Cmd::NewIdentity { key: key.to_string() });
        return wait_job(session, key, io, &format!("{} has an identity of its own.", card.drive()));
    }
    if !facts.newer.is_empty() {
        io.warn(&format!("{}: written by a newer mStream Player: this one can't change it", card.drive()));
        return 1;
    }
    let link = match &flags.server {
        Some(name) => match server_named(servers, name) {
            Ok(link) => Some(link.clone()),
            Err(why) => {
                io.warn(&why);
                return 2;
            }
        },
        None => card.server.link().cloned(),
    };
    let mut want = Want::of(&facts, link.as_ref());
    if !facts.set_up() && want.name.is_empty() {
        want.name = card.found.volume.label.trim().to_string();
    }
    flags.apply(&mut want);
    if !facts.set_up() && want.name.is_empty() {
        io.warn(&format!("{}: a new card needs a name: --name", card.drive()));
        return 1;
    }
    if let Some(fault) = super::state::label_fault(&want.name) {
        let why = match fault {
            LabelFault::Address => "a name, never an address",
            LabelFault::Control => "no control characters",
            LabelFault::Long => "too long",
        };
        io.warn(&format!("--name {}: {why}", want.name));
        return 1;
    }
    let server = link.as_ref().map_or_else(|| "no server yet".to_string(), |l| l.name.clone());
    if facts.set_up() {
        if !flags.changes() {
            let drive = card.drive();
            io.say(&format!("{drive} is set up already: the settings' flags change it (see --help)."));
            return 0;
        }
        let current = card.server.link().map(|l| l.id.clone());
        if want.server.is_some() && want.server != current && facts.tracks > 0 && !flags.take_over {
            server_gate(&card, link.as_ref(), io);
            return 1;
        }
        let drive = card.drive();
        io.say(&format!("Change {drive}'s settings (state.json only; nothing else on it is touched):"));
        settings_lines(&want, &server, io);
        if !go_on(flags, io, "change them") {
            return 1;
        }
        session.link.send(Cmd::Apply { key: key.to_string(), want: Box::new(want) });
        return wait_job(session, key, io, &format!("{}'s settings are changed.", card.drive()));
    }
    if facts.music.unlisted > 0 && !flags.adopt && !flags.keep_mine {
        let n = fmt_count(facts.music.unlisted.into());
        let drive = card.drive();
        io.say(&format!("{drive} holds {n} music files mStream didn't put there. Say what becomes of them:"));
        io.say(&format!("  --adopt      the first sync adopts the ones {server} also has (sync --adopt)"));
        io.say("  --keep-mine  they stay yours: never changed or removed");
        return 1;
    }
    let (drive, name) = (card.drive(), want.name.clone());
    io.say(&format!("Set up {drive} as \"{name}\": /music, its id and name; nothing else on it is touched."));
    settings_lines(&want, &server, io);
    if facts.music.unlisted > 0 {
        let words = if flags.adopt {
            format!("the first sync adopts the files {server} also has, with sync --adopt")
        } else {
            "the music already on it stays yours".to_string()
        };
        io.say(&format!("  {words}"));
    }
    if !go_on(flags, io, "set it up") {
        return 1;
    }
    session.link.send(Cmd::SetUp { key: key.to_string(), want: Box::new(want) });
    wait_job(session, key, io, &format!("{drive} is set up: device card {drive} sync fills it."))
}

/// The settings a set-up or a change writes.
fn settings_lines(want: &Want, server: &str, io: &mut Io) {
    let libraries = match &want.libraries {
        Some(l) if !l.is_empty() => l.join(", "),
        _ => "every library it lists".to_string(),
    };
    io.say(&format!("  from        {server}: {libraries}"));
    let codec = codec_words(want.codec, want.bitrate);
    let convert = match want.convert {
        Convert::Unplayable => format!("only what the player can't play, to {codec}"),
        Convert::Everything => format!("everything, to {codec}"),
    };
    io.say(&format!("  converts    {convert}"));
    let removals = match want.removal {
        Removal::Mirror => format!("follow {server}"),
        Removal::Never => "never".to_string(),
    };
    io.say(&format!("  removals    {removals}"));
    io.say(&format!("  covers      {}", on_off(want.covers)));
    io.say(&format!("  autodj      {}", on_off(want.auto_dj)));
    io.say(&format!("  for         {}", if want.for_someone_else { "someone else" } else { "me" }));
    if let Some(mb) = want.budget_mb {
        io.say(&format!("  budget      {} MB a day per peer", fmt_count(mb)));
    }
}

/// A job's end: `done` said, or the note that it failed.
fn wait_job(session: &mut Session, key: &str, io: &mut Io, done: &str) -> i32 {
    let from = session.fold.notes.len();
    let (t0, mut began) = (Instant::now(), false);
    while !job_done(session, key, &mut began) {
        if t0.elapsed() >= JOB_WITHIN {
            io.warn("no answer from the card in time");
            return 1;
        }
    }
    for note in notes_since(session, from) {
        if let Note::Failed { why } = note {
            io.warn(&why);
            return 1;
        }
    }
    io.say(done);
    0
}

/// The another-server gate's words (clause 58), and its flag.
fn server_gate(card: &CardView, new: Option<&ServerLink>, io: &mut Io) {
    let old = server_name(card);
    let new = new.map_or_else(|| "it".to_string(), |l| l.name.clone());
    let rows = card.facts().map_or(0, |f| f.tracks);
    let (drive, rows) = (card.drive(), fmt_count(rows as u64));
    io.say(&format!("{drive} was filled from {old}. From now on {new}'s libraries would decide what it holds:"));
    io.say(&format!("  the tracks only {old} has leave the card at the next sync ({rows} on it now)"));
    io.say(&format!("  {old} is no longer its server: syncing it from {old} asks again"));
    io.say(&format!("Nothing written: --server {new} --take-over makes {new} its server."));
}

// ── sync ────────────────────────────────────────────────────────────────────

/// What the gates' flags let through, or why the sync stops before it
/// starts (the gate's own words, then its flag).
pub(crate) fn pass_gates(card: &CardView, plan: &Planned, flags: &Flags) -> Result<Allow, Vec<String>> {
    let mut allow = Allow::default();
    if flags.through_parent {
        allow.proxy_copies.extend(plan.ask_proxy.iter().copied());
    }
    let name = card.name().unwrap_or_else(|| card.drive());
    let server = server_name(card);
    // A removals gate passed checks the count itself; the run removes that
    // many or asks again (the engine's `removals_seen`).
    let mut counted = false;
    while let Some(gate) = worker::gate_ahead(card, plan, &allow) {
        match gate {
            Gate::Half { n, total, .. } => {
                if flags.allow_removals != Some(n) {
                    return Err(removals_refused(n, Some(total), &name, &server, flags));
                }
                allow.answers.removals = true;
                allow.answers.mass_removals = true;
                allow.answers.removals_seen = Some(n);
                counted = true;
            }
            Gate::Removals { n, .. } => {
                if flags.allow_removals != Some(n) {
                    return Err(removals_refused(n, None, &name, &server, flags));
                }
                allow.answers.removals = true;
                allow.answers.removals_seen = Some(n);
                counted = true;
            }
            Gate::Adopt { n, all } => {
                if flags.adopt {
                    allow.answers.adopt = Some(true);
                } else if flags.keep_mine {
                    allow.answers.adopt = Some(false);
                } else {
                    let (n, all, drive) = (fmt_count(n as u64), fmt_count(all as u64), card.drive());
                    return Err(vec![
                        format!("{drive} has {all} music files mStream didn't put there; {n} match {server}'s."),
                        "Adopted files stay where they are and are synced from then on, removals and all."
                            .to_string(),
                        format!("Nothing written: --adopt adopts the {n}, --keep-mine keeps them all yours."),
                    ]);
                }
            }
            _ => break,
        }
    }
    if !flags.through_parent && !plan.ask_proxy.is_empty() {
        let peers: Vec<String> = plan.ask_proxy.iter().map(|id| peer_name(card, *id)).collect();
        let peers = peers.join(", ");
        return Err(vec![
            format!("{peers}: reached only through {server}, whose mStream can't yet tell them these bytes"),
            "are a copy, so a refusal of copies there can't stop it. Ask their owner first.".to_string(),
            "Nothing written: --through-parent copies through the parent.".to_string(),
        ]);
    }
    if !counted && flags.allow_removals.is_some_and(|n| n != plan.removals.len()) {
        let n = fmt_count(plan.removals.len() as u64);
        let asked = flags.allow_removals.unwrap_or(0);
        return Err(vec![format!("Nothing written: this sync removes {n}, not {asked}.")]);
    }
    Ok(allow)
}

/// A removals gate's words: the plan's count, and the flag that names it.
fn removals_refused(
    n: usize,
    total: Option<usize>,
    card: &str,
    server: &str,
    flags: &Flags,
) -> Vec<String> {
    let count = fmt_count(n as u64);
    let mut lines = match total {
        Some(total) => {
            let total = fmt_count(total as u64);
            vec![
                format!("This sync removes {count} of {card}'s {total} tracks: half the card or more."),
                "When a server's disk is unplugged or a library renamed, its tracks look removed:".to_string(),
                format!("check {server} first."),
            ]
        }
        None => vec![format!("{server} no longer has {count} of {card}'s tracks: this sync removes them.")],
    };
    let wrong = flags.allow_removals.filter(|&asked| asked != n);
    if let Some(asked) = wrong {
        lines.push(format!("--allow-removals {asked} isn't this plan's count ({count})."));
    }
    lines.push(format!("Nothing written: --allow-removals {n} lets them go (--list-removals lists them)."));
    lines
}

fn peer_name(card: &CardView, id: u32) -> String {
    let state = card.facts().and_then(|f| f.state.as_ref());
    let peer = state.and_then(|s| s.settings.peers.iter().find(|p| p.peer_id == id));
    peer.map_or_else(|| format!("peer {id}"), |p| p.nickname.clone())
}

/// `sync` (clauses 51–58): the plan printed, the gates passed by their
/// flags, then the run, its steps as lines.
fn sync(session: &mut Session, key: &str, flags: &Flags, servers: &[ServerLink], io: &mut Io) -> i32 {
    let Some(mut card) = session.card(key).cloned() else { return 2 };
    let Some(facts) = card.facts().cloned() else {
        io.warn(&format!("{}: {}", card.drive(), state_words(&card).join(" · ")));
        return 1;
    };
    if !facts.set_up() || facts.copied || !facts.newer.is_empty() || facts.check {
        let mut why = state_words(&card).join(" · ");
        if facts.check {
            why = "a rename may have been cut: check the disk first, then sync again".to_string();
        }
        io.warn(&format!("{}: {why}", card.drive()));
        return 1;
    }
    // Another server (`--server S --take-over`, clause 58's gate): its plan
    // is asked with nothing written — a dry run of the take-over — and the
    // settings are written only once the listener goes on (clause 65).
    let mut take_over = None;
    if let Some(name) = &flags.server {
        let link = match server_named(servers, name) {
            Ok(link) => link.clone(),
            Err(why) => {
                io.warn(&why);
                return 2;
            }
        };
        if card.server.link().map(|l| &l.id) != Some(&link.id) {
            if facts.tracks > 0 && !flags.take_over {
                server_gate(&card, Some(&link), io);
                return 1;
            }
            let mut want = Want::of(&facts, Some(&link));
            want.server = Some(link.id.clone());
            card.next = asked_of(session, key, &want);
            card.server = Server::Saved(link);
            take_over = Some(want);
        }
    }
    let plan = match &card.next {
        Next::Plan(plan) => plan.as_ref().clone(),
        _ => {
            io.warn(&format!("{}: can't sync now: {}", card.drive(), next_words(&card)));
            return 1;
        }
    };
    let name = card.name().unwrap_or_else(|| card.drive());
    let server = server_name(&card);
    io.say(&format!("Sync {} ({name}) from {server}: {}.", card.drive(), plan_words(&plan, &server)));
    if plan.conversions > 0 {
        let c = facts.state.as_ref().map(|s| s.settings.conversion);
        let codec = c.map(|c| codec_words(c.codec, c.bitrate)).unwrap_or_default();
        io.say(&format!("  converts {} to {codec}", fmt_count(plan.conversions as u64)));
    }
    if let Some(fit) = plan.fit.as_ref().filter(|f| !f.fits() || f.over_cap()) {
        let why = if fit.over_cap() {
            let (n, cap) = (fmt_count(fit.tracks as u64), fmt_count(TRACK_CAP as u64));
            format!("{n} tracks: the player takes {cap} on a card")
        } else {
            format!("doesn't fit by {} GB", gb(fit.short()))
        };
        let drive = card.drive();
        io.warn(&format!("{drive}: {why}: choose less, or convert more (setup --libraries, --convert)"));
        return 1;
    }
    if flags.list_removals {
        for path in &plan.removals {
            io.say(&format!("  remove {path}"));
        }
    }
    let mut allow = match pass_gates(&card, &plan, flags) {
        Ok(allow) => allow,
        Err(lines) => {
            for line in lines {
                io.say(&line);
            }
            return 1;
        }
    };
    if worker::relay_ahead(&card, &plan) {
        let size = gb(plan.bytes);
        io.say(&format!("{server} is reached through mStream's Quick Connect relay, not directly:"));
        io.say(&format!("about 1-2 MB/s, so {size} GB takes a while."));
        io.say("You can stop at any save and go on later.");
    }
    if !go_on(flags, io, "sync") {
        return 1;
    }
    // The take-over's yes: its settings written (clause 48's Apply), and
    // the sync takes the card over.
    if let Some(want) = take_over {
        let from = session.fold.notes.len();
        session.link.send(Cmd::Apply { key: key.to_string(), want: Box::new(want) });
        let (t0, mut began) = (Instant::now(), false);
        while !job_done(session, key, &mut began) {
            if t0.elapsed() >= JOB_WITHIN {
                io.warn("no answer from the card in time");
                return 1;
            }
        }
        for note in notes_since(session, from) {
            if let Note::Failed { why } = note {
                io.warn(&why);
                return 1;
            }
        }
        allow.take_over = true;
    }
    run(session, key, allow, flags, io)
}

/// What a sync of the card with `want`'s settings would do, written nowhere
/// (the worker's fit: a dry run, as the sheet's fit line asks it).
fn asked_of(session: &mut Session, key: &str, want: &Want) -> Next {
    // A number no other request of this command has.
    let seq = 1;
    session.link.send(Cmd::Fit { key: key.to_string(), want: Box::new(want.clone()), seq });
    let t0 = Instant::now();
    while t0.elapsed() < SETTLE_WITHIN {
        session.pump(Duration::from_millis(50));
        let answer = session.card(key).and_then(|c| c.fit.clone());
        if let Some((_, next)) = answer.filter(|(n, next)| *n == seq && *next != Next::Asking) {
            return next;
        }
    }
    Next::Unreached { asked: now(), why: "no answer in time".into() }
}

/// The run, its steps as lines until it ends.
fn run(session: &mut Session, key: &str, mut allow: Allow, flags: &Flags, io: &mut Io) -> i32 {
    session.link.send(Cmd::Sync { key: key.to_string(), allow: Box::new(allow.clone()) });
    let (mut logs, mut phase, mut tenth, mut stopping) = (session.fold.logs.len(), None, 0u64, false);
    let mut started = false;
    loop {
        session.pump(Duration::from_millis(100));
        if io.ctrl_c.load(Ordering::SeqCst) > 0 && !stopping {
            stopping = true;
            session.link.send(Cmd::Stop { key: key.to_string() });
            let card = session.card(key);
            let name = card.map(|c| c.name().unwrap_or_else(|| c.drive())).unwrap_or_default();
            io.warn(&format!(
                "stopping {name} at the next save… (Ctrl+C again quits at once; the next sync goes on)"
            ));
        }
        for (k, item) in session.fold.logs.iter().skip(logs) {
            if k == key && let Some(line) = log_line(item) {
                let _ = writeln!(io.out, "  {line}");
            }
        }
        logs = session.fold.logs.len();
        let Some(card) = session.card(key).cloned() else {
            io.warn("the card is gone");
            return 3;
        };
        if let Some(run) = &card.run {
            started = true;
            if phase != Some(run.phase) {
                phase = Some(run.phase);
                io.say(&phase_words(run.phase, run.files, run.bytes, &server_name(&card)));
            }
            if run.phase == Phase::Copying && run.bytes > 0 {
                let now = run.done_bytes.min(run.bytes) * 10 / run.bytes;
                if now > tenth && now < 10 {
                    tenth = now;
                    io.say(&format!(
                        "  {} of {} files · {} of {} GB",
                        fmt_count(run.done_files as u64),
                        fmt_count(run.files as u64),
                        gb(run.done_bytes),
                        gb(run.bytes)
                    ));
                }
            }
            continue;
        }
        if !started && card.ended.is_none() && card.gate.is_none() {
            continue;
        }
        if let Some(asks) = &card.gate {
            // A gate the run met: the flags answer it, or the sync stops.
            let proxy: Vec<u32> = asks.iter().filter_map(|g| match g {
                Gate::Proxy { peer } => Some(*peer),
                _ => None,
            }).collect();
            let new = proxy.iter().any(|p| !allow.proxy_copies.contains(p));
            if flags.through_parent && new {
                allow.proxy_copies.extend(proxy);
                session.link.send(Cmd::Sync { key: key.to_string(), allow: Box::new(allow.clone()) });
                // The worker puts the gate away as it starts again.
                if let Some(card) = session.fold.cards.iter_mut().find(|c| c.key == key) {
                    card.gate = None;
                    card.ended = None;
                }
                started = false;
                continue;
            }
            for gate in asks {
                for line in asked_lines(&card, gate) {
                    io.say(&line);
                }
            }
            return 1;
        }
        return ended(&card, io);
    }
}

/// A phase's line as the run reaches it.
fn phase_words(phase: Phase, files: usize, bytes: u64, server: &str) -> String {
    match phase {
        Phase::Waiting => "waiting for the sync under way to finish (one sync at a time)".to_string(),
        Phase::Listing => format!("listing {server}'s libraries…"),
        Phase::Planning => "working out what changes…".to_string(),
        Phase::Copying => format!("copying {} files, {} GB…", fmt_count(files as u64), gb(bytes)),
        Phase::Index => "writing the card's index, covers and Auto DJ…".to_string(),
        Phase::Verifying => "verifying the card…".to_string(),
    }
}

/// A log item as a line, or nothing.
fn log_line(item: &LogItem) -> Option<String> {
    Some(match item {
        LogItem::Skipped { path, why } => format!("skipped {path}: {why}"),
        LogItem::KeptYours { file, server, copy } => format!(
            "kept {file}: changed on the card since the last sync; {server}'s copy is beside it, {copy}"
        ),
        LogItem::Said(words) => words.clone(),
        _ => return None,
    })
}

/// A gate the run stopped at, with no flag that answers it.
fn asked_lines(card: &CardView, gate: &Gate) -> Vec<String> {
    let flags = Flags::default();
    let (drive, server) = (card.drive(), server_name(card));
    match gate {
        Gate::Removals { n, .. } => removals_refused(*n, None, &drive, &server, &flags),
        Gate::Half { n, total, .. } => removals_refused(*n, Some(*total), &drive, &server, &flags),
        Gate::Adopt { n, .. } => {
            let n = fmt_count(*n as u64);
            vec![format!("{n} files match tracks on {server}: --adopt or --keep-mine.")]
        }
        Gate::Server { .. } => {
            vec![format!("{drive} names another server: --server {server} --take-over makes it its server.")]
        }
        Gate::Proxy { peer } => {
            let peer = peer_name(card, *peer);
            vec![format!("{peer} can be reached only through {server}: --through-parent copies through it.")]
        }
        Gate::DiskCheck => {
            vec!["A rename may have been cut: check the disk first, then sync again.".to_string()]
        }
    }
}

/// The run's end, said; its exit code.
fn ended(card: &CardView, io: &mut Io) -> i32 {
    let name = card.name().unwrap_or_else(|| card.drive());
    let drive = card.drive();
    match &card.ended {
        Some(Ended::Done { tracks: n, took, skipped, plays }) => {
            let (took, n) = (span(*took), tracks(*n));
            io.say(&format!("{name} synced in {took}: {n}, verified. Eject it before you take it out."));
            if *plays > 0 {
                io.say(&format!("  {} plays sent to {}", fmt_count(*plays as u64), server_name(card)));
            }
            if !skipped.is_empty() {
                io.say(&format!("  {} files skipped:", fmt_count(skipped.len() as u64)));
                for (path, why) in skipped {
                    io.say(&format!("    {path}: {why}"));
                }
            }
            0
        }
        Some(Ended::Stopped { at, of }) => {
            io.say(&format!("Stopped at a save: {at} of {of} on the card. The next sync goes on from it."));
            3
        }
        Some(Ended::Paused { pause, at, of, .. }) => {
            let why = match pause {
                Pause::Budget { peer, days, .. } => {
                    format!("{peer}'s day's budget is spent ({days} more days at this pace)")
                }
                Pause::Server { tries, .. } => format!("{} didn't answer (try {tries})", server_name(card)),
                Pause::SignIn => format!("{} signed this computer out: sign in again", server_name(card)),
            };
            io.say(&format!("Paused at {at} of {of}: {why}."));
            io.say("Run it again later: it goes on from the last save.");
            3
        }
        Some(Ended::Out { at, of }) => {
            io.say(&format!("Taken out at {at} of {of}: put it back in any reader and sync again."));
            io.say("Nothing is lost.");
            3
        }
        Some(Ended::Full { at, of }) => {
            io.warn(&format!("{drive}: card full: stopped at the last save, {at} of {of} on the card"));
            1
        }
        Some(Ended::Locked { .. }) => {
            io.warn(&format!("{drive}: write-protected: slide the lock on its side up and put it back"));
            1
        }
        Some(Ended::OtherCard) => {
            io.warn(&format!("{drive} holds another card now: nothing was written to it"));
            1
        }
        Some(Ended::Fit(fit)) => {
            io.warn(&format!("{drive}: doesn't fit by {} GB", gb(fit.short())));
            1
        }
        Some(Ended::Refused { peer, why }) => {
            io.warn(&format!("{} refuses copies: {why}", peer_name(card, *peer)));
            1
        }
        Some(Ended::Failed(why)) => {
            io.warn(&format!("{drive}: {why}"));
            1
        }
        None => 1,
    }
}

// ── plays and eject ─────────────────────────────────────────────────────────

/// `plays` (clause 61): the card's plays sent to its server, no sync.
fn plays(session: &mut Session, key: &str, flags: &Flags, io: &mut Io) -> i32 {
    let Some(card) = session.card(key).cloned() else { return 2 };
    if !card.facts().is_some_and(|f| f.set_up()) || card.server.link().is_none() {
        io.warn(&format!("{}: {}", card.drive(), state_words(&card).join(" · ")));
        return 1;
    }
    let server = server_name(&card);
    let n = match &card.next {
        Next::Plan(plan) => plan.plays,
        _ => {
            io.warn(&format!("{}: can't send its plays now: {}", card.drive(), next_words(&card)));
            return 1;
        }
    };
    if n == 0 {
        io.say(&format!("No plays to send to {server}."));
        return 0;
    }
    io.say(&format!("Send {} plays from {} to {server}.", fmt_count(n as u64), card.drive()));
    if !go_on(flags, io, "send them") {
        return 1;
    }
    let from = session.fold.notes.len();
    session.link.send(Cmd::Plays { key: key.to_string() });
    let (t0, mut began) = (Instant::now(), false);
    while !job_done(session, key, &mut began) {
        if t0.elapsed() >= JOB_WITHIN {
            io.warn("no answer in time");
            return 1;
        }
    }
    for note in notes_since(session, from) {
        match note {
            Note::PlaysSent { n, server } => {
                io.say(&format!("{} plays sent to {server}.", fmt_count(n as u64)));
                return 0;
            }
            Note::Failed { why } => {
                io.warn(&why);
                return 1;
            }
            _ => {}
        }
    }
    1
}

/// `eject` (clause 54): the OS asked to let the card go.
fn eject(session: &mut Session, key: &str, io: &mut Io) -> i32 {
    let Some(card) = session.card(key).cloned() else { return 2 };
    let from = session.fold.notes.len();
    session.link.send(Cmd::Eject { key: key.to_string() });
    let t0 = Instant::now();
    while t0.elapsed() < JOB_WITHIN {
        session.pump(Duration::from_millis(50));
        for note in notes_since(session, from) {
            match note {
                Note::Ejected { card: name } => {
                    io.say(&format!("{} ({name}) can be taken out.", card.drive()));
                    return 0;
                }
                Note::EjectFailed { why, .. } => {
                    io.warn(&format!("{}: {why}", card.drive()));
                    return 1;
                }
                _ => {}
            }
        }
    }
    io.warn("no answer in time");
    1
}

// ── Times ───────────────────────────────────────────────────────────────────

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `2026-10-16 09:12 UTC`.
fn date(at: u64) -> String {
    let (y, m, d) = crate::admin::tz::civil_from_days((at / 86_400) as i64);
    let secs = at % 86_400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", secs / 3_600, secs % 3_600 / 60)
}

/// `3 days ago`, `yesterday`, `5 min ago`, `just now`.
fn ago(then: u64, now: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => "just now".to_string(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "yesterday".to_string(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// `25 min`, `5 h 12 min`, `40 s`.
fn span(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3_600 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3_600, secs % 3_600 / 60),
    }
}

#[cfg(test)]
mod tests;
