//! A card's set-up (the transfer plan's 2.13): what makes a FAT card in a
//! PC's reader one the transfer syncs, in three steps —
//! 1. `/music`, made when the card has none (2.8.1);
//! 2. the empty first commit, which mints the card's id (2.5.2): a tags
//!    file with no records whose folder table holds `/music` alone, and a
//!    final root naming it — the shape of the fixtures' golden
//!    `tags-00000001.bin` and `manifest-min.bin`, which the firmware repo's
//!    tools/synthtransfer.py shows the device takes;
//! 3. `state.json`, the card's identity and settings ([`state`]).
//!
//! Each step a card has already is left as it is: a card with a valid root
//! keeps its commit and its id, which are reported, and one whose state file
//! is its own keeps that too. A card that has all three is only looked at.
//! Every write is the engine's, in a session bound to the card (run.lock,
//! the identity triple); the listener's music and `/.player` are never
//! touched: a plan that would (a cut sync's leftovers to finish) stops the
//! set-up before it writes anything.
//!
//! **A copied card** (2.10) keeps the id of the card it was copied from.
//! Set-up says so — by the state file's volume serial ([`state::copied`]), and
//! for a byte-level clone, serial and all, by this computer's `cards.json`
//! ([`Cards::diverged`]) — and writes nothing more for it. A new identity is
//! the listener's answer ([`Ask::new_identity`]): a commit under a new card
//! id, then the state file rewritten for it, its settings kept.
//!
//! The listener's way in is the Card view and `device card <drive> setup`,
//! which wait for their UX contract; until then the hidden `device
//! card-setup` runs it ([`command`]).

#[cfg(test)]
mod tests;

use std::io::Write;

use serde_json::{Value, json};

use super::cards::Cards;
use super::engine::{
    Answers, Env, Event, Expect, Failed, Fetched, Fetcher, Guard, Knobs, NoHooks, Session, Stop,
    WantedCard, WantedFile,
};
use super::state::{self, Copied, State, hex};
use super::volume::{self, CardWrite, Found, Mounted, Refusal, Serial};

/// What the listener asks of a set-up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ask {
    /// Their name for the card: written with a state file this set-up
    /// makes, never over the card's own (a sync's settings change it).
    pub(crate) label: String,
    /// The answer to a copied card: an identity of its own.
    pub(crate) new_identity: bool,
}

/// What a set-up found, and what it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SetUp {
    /// The card's root now.
    pub(crate) card_id: u64,
    pub(crate) generation: u32,
    pub(crate) commit_id: u64,
    pub(crate) label: String,
    /// The volume serial, as the OS reads it now.
    pub(crate) serial: Option<Serial>,
    pub(crate) made_music: bool,
    /// A commit made: the first, or a new identity's.
    pub(crate) committed: bool,
    pub(crate) wrote_state: bool,
    /// The card id a new identity replaced.
    pub(crate) was: Option<u64>,
    /// The signs of a copied card, as the card was before this set-up.
    pub(crate) copied: Vec<Copied>,
}

/// A source with no bytes: a set-up's plan fetches nothing.
struct NoBytes;

impl Fetcher for NoBytes {
    fn fetch(&mut self, _: &WantedFile, _: &mut dyn Write) -> Result<Fetched, Failed> {
        Err(Failed { why: "a set-up fetches nothing".into(), stop: true })
    }

    fn read_range(&mut self, _: &WantedFile, _: u64, _: usize) -> Result<Vec<u8>, Failed> {
        Err(Failed { why: "a set-up fetches nothing".into(), stop: true })
    }
}

/// `step` in an open session; a stop lets the card go (run.lock deleted,
/// unless the card is no longer the bound one).
fn step<'c, T>(
    mut session: Session<'c>,
    step: impl FnOnce(&mut Session<'c>) -> Result<T, Stop>,
) -> Result<(Session<'c>, T), Stop> {
    match step(&mut session) {
        Ok(done) => Ok((session, done)),
        Err(stop) => {
            session.close();
            Err(stop)
        }
    }
}

/// Sets up a card a volume layer accepted and the listener named, or says
/// what it is: see the module's note. `envs` gives each session its clock
/// and ids; `cards` is this computer's cache, brought up to date (the
/// caller saves it).
pub(crate) fn set_up(
    card: &dyn CardWrite,
    ask: &Ask,
    envs: &mut dyn FnMut() -> Env,
    cards: &mut Cards,
    events: &mut dyn FnMut(Event),
) -> Result<SetUp, Stop> {
    let label = ask.label.trim();
    if let Some(fault) = state::label_fault(label) {
        return Err(Stop::Refused(fault.code().into()));
    }
    // What the card has, read only: a card with everything is left alone.
    let seen = volume::inspect(card).map_err(|_| Stop::Pulled)?;
    if !seen.newer.is_empty() {
        return Err(Stop::Guard(Guard::Newer(seen.newer)));
    }
    let serial = card.serial().map_err(|_| Stop::Pulled)?;
    let root = seen.root.as_ref().map(|r| {
        let m = &r.manifest;
        (m.header.card_id, m.header.generation, m.commit_id)
    });
    let (card_id, generation, commit_id) = root.unwrap_or_default();
    let old = State::of(&seen.state);
    let own = old.as_ref().filter(|s| root.is_some() && s.card_id == card_id);
    let mut copied: Vec<Copied> = Vec::new();
    copied.extend(own.and_then(|s| state::copied(s, card_id, serial.as_ref())));
    if root.is_some() {
        copied.extend(cards.diverged(card_id, generation, commit_id));
    }
    let commit = root.is_none() || ask.new_identity;
    let restate = commit || own.is_none();
    let mut done = SetUp {
        card_id,
        generation,
        commit_id,
        label: own.map_or_else(String::new, |s| s.label.clone()),
        serial: serial.clone(),
        made_music: false,
        committed: false,
        wrote_state: false,
        was: None,
        copied,
    };

    let mut env = envs();
    let now = (env.now)();
    if seen.music.present && !restate {
        cards.saw(card_id, &done.label, now, generation, commit_id);
        return Ok(done);
    }
    let bound = |card_id, commit_id| Expect {
        serial: serial.clone(),
        card_id: Some(card_id),
        commit_id: Some(commit_id),
    };
    let (knobs, answers) = (Knobs::default(), Answers::default());
    let expect = bound(card_id, commit_id);
    let mut session = Session::open(card, &expect, knobs.clone(), &answers, env, &mut NoHooks, events)?;
    if commit {
        // The empty plan: on a card with no root its run is the first commit,
        // on a copied card's (new_card_id) a commit of the same files under a
        // new id. Any delete, write or adoption is a cut sync's to finish.
        let new_id = Answers { new_card_id: root.is_some(), ..Answers::default() };
        let empty = WantedCard::default();
        let plan;
        (session, plan) = step(session, |s| s.plan(&empty, &new_id, &mut NoBytes))?;
        if !plan.guards.is_empty() || plan.touches_music() {
            session.close();
            let why = "a cut sync's plan is on the card: a sync finishes it";
            return Err(Stop::Refused(why.into()));
        }
        (session, done.made_music) = step(session, Session::music_folder)?;
        let outcome = session.run(plan, &mut NoBytes, &mut NoHooks, events)?;
        if let Err(why) = outcome.verified {
            // The commit is made, the state file not (the next set-up writes
            // it): what the card was given is said all the same.
            let (id, commit) = (hex(outcome.card_id), hex(outcome.commit_id));
            let made = format!("card {id} committed generation {} as {commit}", outcome.generation);
            return Err(Stop::Refused(format!("{made}, but the card doesn't read back: {why}")));
        }
        done.was = root.map(|_| card_id).filter(|&was| was != outcome.card_id);
        done.committed = outcome.commit_id != commit_id;
        (done.card_id, done.generation, done.commit_id) =
            (outcome.card_id, outcome.generation, outcome.commit_id);
        // The state file in a session of its own, bound to that commit.
        let expect = bound(done.card_id, done.commit_id);
        session = Session::open(card, &expect, knobs, &answers, envs(), &mut NoHooks, events)?;
    } else {
        (session, done.made_music) = step(session, Session::music_folder)?;
    }
    if restate {
        // The card's own settings are kept under its new identity, the
        // serials it was written on not (they were the other card's); a card
        // with none gets the listener's label, and so does one whose label
        // this software may not write (an address put there by hand or by
        // another tool: 2.10).
        let mut state = match old {
            Some(old) => {
                let (fat_serial, uuid) = (None, None);
                State { card_id: done.card_id, created: now, fat_serial, uuid, ..old }
            }
            None => State::new(done.card_id, label, serial.as_ref(), now),
        };
        if state.label.is_empty() || state::label_fault(&state.label).is_some() {
            state.label = label.to_string();
        }
        state.saw(serial.as_ref());
        let bytes = state.text(session.state().seq + 1);
        (session, _) = step(session, |s| s.write_state(&bytes))?;
        (done.label, done.wrote_state) = (state.label, true);
    }
    session.close();
    cards.saw(done.card_id, &done.label, now, done.generation, done.commit_id);
    Ok(done)
}

// ── The hidden command ──────────────────────────────────────────────────────

/// `device card-setup <volume>`, undocumented: sets up the card at
/// `volume` (where it is mounted, `E:` or `/media/card`, or the OS's name
/// for it, `sdb1`), which must be one the volume layer accepts — a refused
/// volume, or one that isn't a card at all, is never opened. Prints what
/// the set-up found and did as JSON, the engine's events with it (on
/// stderr, before the reason, when it stopped); `cards.json` is brought up
/// to date.
/// For the real-card checks: the listener's `device card <drive> setup`
/// waits for its UX contract. Exit 0; 1 when the set-up stopped; 2 when the
/// volume isn't one it may write, or the volumes can't be listed.
pub(crate) fn command(
    volume: &str,
    label: Option<&str>,
    new_identity: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let found = match volume::find() {
        Ok(found) => found,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: the volumes can't be listed: {e}");
            return 2;
        }
    };
    let mut cards = Cards::load();
    let code = on(&found, volume, label, new_identity, &mut cards, &mut Env::system, out, err);
    if code == 0
        && let Err(e) = cards.save()
    {
        let _ = writeln!(err, "mstream-player: {e}");
    }
    code
}

/// [`command`] on the volumes found.
#[allow(clippy::too_many_arguments)]
fn on(
    found: &[Found],
    named: &str,
    label: Option<&str>,
    new_identity: bool,
    cards: &mut Cards,
    envs: &mut dyn FnMut() -> Env,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(found) = pick(found, named) else {
        let _ = writeln!(err, "mstream-player: {named} isn't a volume this computer lists");
        return 2;
    };
    let Some(card) = Mounted::of(found) else {
        let codes: Vec<&str> = found.refusals.iter().map(Refusal::code).collect();
        let _ = writeln!(err, "mstream-player: {named} is refused: {}", codes.join(", "));
        return 2;
    };
    // The volume's own label, when none is given.
    let label = label.map_or_else(|| found.volume.label.trim().to_string(), str::to_string);
    let ask = Ask { label, new_identity };
    // What the engine says beside the three steps (a stale run.lock taken
    // over, another card's plan deleted) is said too: in the JSON, or before
    // the stop.
    let mut said = Vec::new();
    match set_up(&card, &ask, envs, cards, &mut |e| said.push(event_json(&e))) {
        Ok(done) => {
            let mut json = done_json(&done);
            json["events"] = Value::Array(said);
            let _ = writeln!(out, "{}", serde_json::to_string_pretty(&json).unwrap_or_default());
            0
        }
        Err(stop) => {
            for event in &said {
                let _ = writeln!(err, "mstream-player: {event}");
            }
            let _ = writeln!(err, "mstream-player: the set-up stopped: {stop}");
            1
        }
    }
}

/// The volume a command line names: by where it is mounted (`E:`, `E:\`,
/// `/media/card/`; any case on Windows), or by the OS's name for it.
pub(super) fn pick<'f>(found: &'f [Found], named: &str) -> Option<&'f Found> {
    let plain = |path: &str| {
        let path = path.trim_end_matches(['/', '\\']);
        if cfg!(windows) { path.to_ascii_lowercase() } else { path.to_string() }
    };
    let want = plain(named);
    let by_path = |f: &&Found| !want.is_empty() && plain(&f.volume.path.to_string_lossy()) == want;
    found.iter().find(by_path).or_else(|| found.iter().find(|f| f.volume.device == named))
}

fn done_json(done: &SetUp) -> Value {
    let copied: Vec<Value> = done
        .copied
        .iter()
        .map(|c| match c {
            Copied::Serial { was, now } => {
                json!({ "sign": "serial", "was": was.to_string(), "now": now.to_string() })
            }
            Copied::Diverged { generation, seen, now } => json!({
                "sign": "commit",
                "generation": generation,
                "seen": hex(*seen),
                "now": hex(*now),
            }),
        })
        .collect();
    json!({
        "cardId": hex(done.card_id),
        "generation": done.generation,
        "commitId": hex(done.commit_id),
        "label": done.label,
        "serial": done.serial.as_ref().map(Serial::to_string),
        "madeMusic": done.made_music,
        "committed": done.committed,
        "wroteState": done.wrote_state,
        "was": done.was.map(hex),
        "copied": copied,
    })
}

/// An engine event, as the command says it. A set-up's plan fetches,
/// writes and deletes nothing in `/music`, so the rest are only named.
fn event_json(event: &Event) -> Value {
    match event {
        Event::TookOver(note) => json!({ "event": "tookOver", "note": note }),
        Event::ForeignPlan(card_id) => json!({ "event": "foreignPlan", "cardId": hex(*card_id) }),
        Event::Committed { generation, last } => {
            json!({ "event": "committed", "generation": generation, "last": last })
        }
        other => json!({ "event": format!("{other:?}") }),
    }
}
