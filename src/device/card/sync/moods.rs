//! A sync's mood scores (the player's docs/MOOD-FILTERS.md 5.2; METADATA.md
//! 2.6.4): at the final commit, the scores of the server's own rows from its
//! `POST /api/v1/db/mood-scores`, laid on the card as each record's tail
//! ([`tails`]), from which the player filters its Albums and Auto DJ by mood
//! with no network. The tail's bytes are the kit's ([`encode`]).
//!
//! **Who is asked, and when** ([`Rule::step`]), in 5.2's order: a card made
//! for someone else loses every tail and asks nothing, whatever its Auto DJ
//! setting says (the Auto DJ index's licence gate); `--moods false` or the
//! listener's stop asks nothing; a commit with a record scored, whose files
//! are the root's and that read nothing whole, asks nothing, so a 304 costs
//! no call and a change of scores alone waits for the next whole read (5.6);
//! else the server is asked, when its `/api/` says `moodScores`, for every
//! row of its own (origin 0) with a server path, whatever its hash. A peer's
//! rows are never asked for: they stay at 8 zero bytes.
//!
//! **The answer** ([`overlay`]): a row is its tail, read by its heads' names;
//! a null (no row, or a path the server won't say) is 8 zero bytes, which
//! clear one; a path the answer left out, or whose row isn't one, keeps its
//! own. No answer fails a run: a 403 or a 404, an error, the model, pin or
//! heads moving between the answer's chunks, each keeps every tail as it is,
//! with a note. Equal scores make no commit: the engine lays the tails before
//! it weighs the commit against the root.

use std::collections::{HashMap, HashSet};

use crate::api::Client;
use crate::api::transfer::MoodScores;
use crate::device::card::contract::moods::encode;
use crate::device::card::contract::mptg::{Ledger, RecordIn};
use crate::device::card::engine::{Commit, Moods};

/// What a run's mood step goes by: the card's settings, the run's ask and
/// the server's `/api/`, as the sync read them before its run.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Rule {
    /// The card is made for someone else.
    pub(super) someone_else: bool,
    /// No scores asked for: the hidden command's `--moods false`.
    pub(super) left_out: bool,
    /// The server's `/api/` says `moodScores`: it answers the route.
    pub(super) served: bool,
}

/// What a run's mood step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Every tail goes, and nothing is asked: a card for someone else.
    Clear,
    /// Each tail as the run holds it, and nothing asked.
    Keep,
    /// The same, for a server that serves none, which a note says.
    Unserved,
    /// The server's own rows asked for.
    Ask,
}

impl Rule {
    /// 5.2's fetch rule, in its order: `stopped` the listener's stop is set,
    /// `scored` a record of the commit has a tail, `same` its files are the
    /// root's and the run read nothing whole.
    pub(super) fn step(&self, stopped: bool, scored: bool, same: bool) -> Step {
        if self.someone_else {
            Step::Clear
        } else if self.left_out || stopped || (scored && same) {
            Step::Keep
        } else if !self.served {
            Step::Unserved
        } else {
            Step::Ask
        }
    }
}

/// What came of a run's mood step.
pub(super) struct Came {
    pub(super) moods: Moods,
    pub(super) note: Option<String>,
}

/// The notes a step leaves: a card for someone else's, an older server's,
/// a 403's or a 404's.
pub(super) const GIFT: &str = "no mood scores: the card is made for someone else";
pub(super) const UNSERVED: &str =
    "no new mood scores: the server doesn't serve them (an older mStream)";
pub(super) const REFUSED: &str = "no new mood scores: the server refused them (a 403 or a 404)";

/// A run's step at its final commit, as `step` says: the commit's rows of
/// its own server asked of `client`, that server's, in as many calls as the
/// route's caps need ([`Client::mood_scores`]).
pub(super) fn tails(client: &Client, commit: &Commit<'_>, step: Step) -> Came {
    let keep = |note: Option<String>| Came { moods: Moods::Keep, note };
    match step {
        Step::Clear => Came { moods: Moods::Clear, note: Some(GIFT.into()) },
        Step::Keep => keep(None),
        Step::Unserved => keep(Some(UNSERVED.into())),
        Step::Ask => {
            // Each server path once, in the records' order.
            let mut seen = HashSet::new();
            let asks = own_rows(commit).map(|(_, l)| l.server_path.as_str());
            let paths: Vec<String> = asks.filter(|p| seen.insert(*p)).map(str::to_string).collect();
            if paths.is_empty() {
                return keep(None);
            }
            match client.mood_scores(&paths) {
                Ok(Some(found)) => Came { moods: overlay(commit, &found), note: None },
                Ok(None) => keep(Some(REFUSED.into())),
                Err(e) => keep(Some(format!("no new mood scores: they didn't come ({e})"))),
            }
        }
    }
}

/// The commit's records of its own server (origin 0) that have a server
/// path, each with its ledger row: the rows a server's scores are for.
fn own_rows<'c>(commit: &'c Commit<'_>) -> impl Iterator<Item = (&'c RecordIn, &'c Ledger)> {
    let own = commit.origins.first().copied().flatten();
    commit.records.iter().filter_map(move |r| {
        let l = r.ledger.as_ref()?;
        (own == Some(l.server) && !l.server_path.is_empty()).then_some((r, l))
    })
}

/// An answer's tails, by card path: each of the server's own rows whose
/// server path the answer gave a key, its row's tail or 8 zero bytes for
/// its null. The rest keep theirs.
fn overlay(commit: &Commit<'_>, found: &MoodScores) -> Moods {
    let mut tails = HashMap::new();
    for (r, l) in own_rows(commit) {
        if let Some(row) = found.tracks.get(&l.server_path) {
            tails.insert(r.path.clone(), encode(row.as_ref()));
        }
    }
    Moods::Overlay(tails)
}
