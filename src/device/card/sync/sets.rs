//! A sync of a card of a multi-card set (B7; [`span`]): the set's table as
//! the card and this computer know it, the selection's files cut to the
//! card's range — those it holds of other cards' ranges kept until their
//! cards hold them, then let go — a set made of a card's selection when the
//! listener asks to spread it, what a card that can't take its range is
//! offered and the answer taken, and, at the run's end, what the card now
//! says of itself in the table: its room, its load, the files it holds for
//! others, and whether it holds its whole range.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use super::{Ask, Halt, Halted, Key as FileKey, Listed};
use crate::device::card::cards::Cards;
use crate::device::card::contract::mptg::{self, Ledger, RecordIn};
use crate::device::card::engine::{Answers, Commit, Fetcher, Guard, Outcome, Plan, Session};
use crate::device::card::plan::{self, Wanted};
use crate::device::card::span::{
    self, Asked, Decision, Group, Holds, Key, Load, Offer, Part, Set, Slot, Stand, set_json,
};
use crate::device::card::state::{State, hex};
use crate::device::card::volume::{self, Root, Space};

/// A card's cluster when its volume doesn't say: a big card's.
const CLUSTER: u32 = 32 * 1024;

fn cluster_of(space: Option<Space>) -> u32 {
    space.map_or(CLUSTER, |s| s.cluster.max(512))
}

/// What the card's ledger says it holds, read while the session's root is
/// at hand (the run's plans take the session): each row's card path and
/// ledger, when a set needs them, and what the rows take of the card.
pub(super) struct Rows {
    rows: Vec<(String, Ledger)>,
    ours: Load,
}

impl Rows {
    pub(super) fn of(root: &Root, space: Option<Space>, with_rows: bool) -> Rows {
        let records = root.tags.as_ref().map(|t| t.records.as_slice()).unwrap_or_default();
        let row = |e: &mptg::Entry| Some((e.path.clone(), e.ledger.clone()?));
        let rows = if with_rows { records.iter().filter_map(row).collect() } else { Vec::new() };
        Rows { rows, ours: span::load_of(root, cluster_of(space)) }
    }
}

/// A sync's part in its card's set.
pub(super) struct SetRun {
    /// The table; None until a spread makes it.
    set: Option<Set>,
    /// The card's place in it.
    slot: usize,
    card_id: u64,
    label: String,
    /// The server the run reads, by its URL's key: a new set's.
    server: u64,
    /// A new set's id.
    new_id: u64,
    /// The set's asks before this run's first look: each look asks from it.
    asks0: Option<u64>,
    asked: Asked,
    /// The card's usable room, when its volume says how big it is.
    room: Option<u64>,
    cluster: u32,
    /// The files of its range, by origin and server path; and their groups.
    in_range: HashSet<FileKey>,
    groups: Vec<Group>,
    /// The card paths of the files it lets go: removals the set decided.
    dropped_paths: HashSet<String>,
    counts: Counts,
    offers: Vec<Offer>,
    /// The listener's answer was weighed against the offers.
    weighed: bool,
    decided: Option<Decision>,
    created: bool,
    written: bool,
}

/// How a run's files stood.
#[derive(Clone, Debug, Default)]
struct Counts {
    mine: usize,
    held: BTreeMap<usize, usize>,
    dropped: BTreeMap<usize, usize>,
    theirs: usize,
}

/// What a set's run says of it before it runs.
#[derive(Clone, Debug)]
pub(crate) struct SetSummary {
    /// The table as the run leaves it: a set made, an edge moved.
    pub(crate) set: Set,
    pub(crate) slot: usize,
    pub(crate) created: bool,
    pub(crate) decided: Option<Decision>,
    /// The selection's files of the card's range.
    pub(crate) mine: usize,
    /// Files of other cards' ranges, by card: held here and kept until that
    /// card holds them; held here and let go now; on no card here.
    pub(crate) held: BTreeMap<usize, usize>,
    pub(crate) dropped: BTreeMap<usize, usize>,
    pub(crate) theirs: usize,
    /// What the card is offered when it can't take its range.
    pub(crate) offers: Vec<Offer>,
    /// The listener's answer stored on the card with no run: the card can't
    /// take its range until the cards it holds files for have synced.
    pub(crate) written: bool,
}

/// What the run's end needs to say of the card in its set.
pub(super) struct SetEnd {
    set: Set,
    slot: usize,
    label: String,
    room: Option<u64>,
    asked: Asked,
    in_range: HashSet<FileKey>,
    pub(super) cluster: u32,
}

/// The final commit's ledger, as a set's run's end reads it: each file by
/// origin and server path, and what the files take of the card.
pub(super) struct Final {
    keys: HashSet<FileKey>,
    load: Load,
}

impl Final {
    pub(super) fn of(commit: &Commit<'_>, cluster: u32) -> Final {
        let origin: HashMap<u16, usize> =
            commit.origins.iter().enumerate().filter_map(|(o, s)| Some(((*s)?, o))).collect();
        let keys = commit
            .records
            .iter()
            .filter_map(|r| r.ledger.as_ref())
            .filter_map(|l| Some((*origin.get(&l.server)?, l.server_path.clone())))
            .collect();
        let cost = |r: &RecordIn| plan::track_cost(r.rec.size as u64, cluster);
        let bytes = commit.records.iter().map(cost).sum();
        Final { keys, load: Load { bytes, tracks: commit.records.len() as u64 } }
    }
}

fn sorted(libraries: &[String]) -> Vec<String> {
    let mut libraries = libraries.to_vec();
    libraries.sort();
    libraries.dedup();
    libraries
}

impl SetRun {
    /// The card's set, as it and this computer (`cards`) know it, or a new
    /// one when the listener asks to spread the card's selection (`id`);
    /// None for a card of no set. Refused: a set of a newer player's, a
    /// table that doesn't name the card, another server's set, another
    /// selection, a federation peer (a set's cards read the server's own
    /// libraries), a decision for a card of no set.
    pub(super) fn open(
        state: &State,
        cards: &Cards,
        card_id: u64,
        url_key: u64,
        ask: &Ask,
        id: u64,
    ) -> Result<Option<SetRun>, String> {
        if state.set_unread {
            return Err("the card is in a set of a newer player's: it isn't synced".into());
        }
        let peers = ask.peers.as_ref().is_some_and(|p| !p.is_empty())
            || ask.peers_only
            || state.settings.peers.iter().any(|p| !p.libraries.is_empty());
        let no_peers = "a set's cards read the server's own libraries only: no federation peers";
        let run = |set: Option<Set>, slot: usize| SetRun {
            set,
            slot,
            card_id,
            label: state.label.clone(),
            server: url_key,
            new_id: id.max(1),
            asks0: None,
            asked: Asked::default(),
            room: None,
            cluster: CLUSTER,
            in_range: HashSet::new(),
            groups: Vec::new(),
            dropped_paths: HashSet::new(),
            counts: Counts::default(),
            offers: Vec::new(),
            weighed: false,
            decided: None,
            created: false,
            written: false,
        };
        let set = match (&state.set, ask.decide) {
            (None, None) => return Ok(None),
            (None, Some(Decision::Spread)) if peers => return Err(no_peers.into()),
            (None, Some(Decision::Spread)) => return Ok(Some(run(None, 0))),
            (None, Some(d)) => return Err(format!("{} is for a card of a set", d.code())),
            (Some(_), Some(Decision::Spread)) => return Err("the card is in a set already".into()),
            (Some(own), _) => cards.set_of(own),
        };
        let Some(slot) = set.slot_of(card_id) else {
            return Err(format!("the table of set {} doesn't name this card", hex(set.id)));
        };
        if set.server != url_key && !state.settings.keys(url_key).contains(&set.server) {
            return Err(format!("set {} is filled from another server", hex(set.id)));
        }
        if peers {
            return Err(no_peers.into());
        }
        if let Some(asked) = &ask.libraries
            && sorted(asked) != set.libraries
        {
            let theirs = set.libraries.join(", ");
            return Err(format!("a card of a set syncs the set's libraries ({theirs}), no others"));
        }
        Ok(Some(run(Some(set), slot)))
    }

    /// The set's libraries: a card of it syncs those.
    pub(super) fn libraries(&self) -> Option<Vec<String>> {
        self.set.as_ref().map(|s| s.libraries.clone())
    }

    /// A plan of the card depends on its range too: another range, and a
    /// 304 can't stand for a listing.
    pub(super) fn plan_key(&self, key: u64) -> u64 {
        match &self.set {
            Some(set) => {
                let text = format!("{key:016x}\n{}", set.range_text(self.slot));
                crate::device::card::contract::prim::fnv1a64(text.as_bytes())
            }
            None => key,
        }
    }

    /// The manifest is read whole: a set to make, or a card that doesn't
    /// hold its range under the set's last ask (its word is made only of a
    /// whole listing: a 304 stands for the files the card has).
    pub(super) fn whole(&self) -> bool {
        self.set.as_ref().is_none_or(|s| !s.holds(self.slot, s.asks))
    }

    /// `wanted`, the run's whole selection, cut to the card's range (see the
    /// module's note); a set made of it first when the listener asked to
    /// spread it. `held`: the card's ledger, which says what the card holds.
    pub(super) fn take(
        &mut self,
        wanted: &mut Wanted,
        libraries: &[String],
        held: &Rows,
        space: Option<Space>,
    ) -> Result<(), String> {
        self.cluster = cluster_of(space);
        self.room = space.map(|s| plan::usable_room(&s, held.ours.bytes));
        if self.set.is_none() {
            let Some(room) = self.room else {
                return Err("the card's size isn't known: no set is made".into());
            };
            let sizes = wanted.card.files.iter().map(|f| (f.ledger.server_path.as_str(), f.size));
            let parts = span::split(&span::groups(sizes, self.cluster), room, room);
            self.set = Some(self.made(parts, room, libraries));
            self.created = true;
        }
        let set = self.set.as_mut().expect("a set");
        let asks0 = *self.asks0.get_or_insert(set.asks);
        set.asks = asks0;

        // What the card holds: the ledger's row of each file, paired as the
        // engine pairs them — by server path, else by its audio hash (else its
        // file hash) among the rows left: a move on the server.
        let rows: Vec<(&str, &Ledger)> = held.rows.iter().map(|(p, l)| (p.as_str(), l)).collect();
        let files = &wanted.card.files;
        let mut row_of: Vec<Option<usize>> = vec![None; files.len()];
        let mut used = vec![false; rows.len()];
        let by_path: HashMap<&str, usize> =
            rows.iter().enumerate().map(|(r, (_, l))| (l.server_path.as_str(), r)).collect();
        for (f, file) in files.iter().enumerate() {
            if let Some(&r) = by_path.get(file.ledger.server_path.as_str())
                && !used[r]
            {
                (row_of[f], used[r]) = (Some(r), true);
            }
        }
        let hashes = |l: &Ledger| [l.audio_hash, l.file_hash].into_iter().filter(|h| *h != [0; 16]);
        let mut by_hash: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
        for (r, (_, l)) in rows.iter().enumerate().filter(|(r, _)| !used[*r]) {
            for h in hashes(l) {
                by_hash.entry(h).or_default().push(r);
            }
        }
        for f in 0..files.len() {
            if row_of[f].is_some() {
                continue;
            }
            let free = hashes(&files[f].ledger).flat_map(|h| by_hash.get(&h).into_iter().flatten());
            let free: Vec<usize> = free.copied().filter(|&r| !used[r]).collect();
            if let Some(&r) = free.first() {
                (row_of[f], used[r]) = (Some(r), true);
            }
        }
        let id = |i: usize| format!("{}\u{1f}{}", files[i].origin, files[i].ledger.server_path);
        let keyed: Vec<(Key, Option<String>)> = (0..files.len())
            .map(|i| (Key::of(&files[i].ledger.server_path), row_of[i].map(|_| id(i))))
            .collect();
        let (asked, stands) = span::stands(set, self.slot, &keyed);
        self.asked = asked;

        let mut counts = Counts::default();
        let (mut in_range, mut dropped_paths) = (HashSet::new(), HashSet::new());
        let mut mine: Vec<(&str, u64)> = Vec::new();
        for (i, stand) in stands.iter().enumerate() {
            match *stand {
                Stand::Mine => {
                    counts.mine += 1;
                    in_range.insert((files[i].origin, files[i].ledger.server_path.clone()));
                    mine.push((files[i].ledger.server_path.as_str(), files[i].size));
                }
                Stand::Held(n) => *counts.held.entry(n).or_default() += 1,
                Stand::Dropped(n) => {
                    *counts.dropped.entry(n).or_default() += 1;
                    dropped_paths.extend(row_of[i].map(|r| rows[r].0.to_string()));
                }
                Stand::Theirs(_) => counts.theirs += 1,
            }
        }
        self.groups = span::groups(mine, self.cluster);
        (self.in_range, self.dropped_paths, self.counts) = (in_range, dropped_paths, counts);

        // The card takes its range and what it keeps for others; the skipped
        // files of its range are its own to report.
        let keep: Vec<bool> =
            stands.iter().map(|s| matches!(s, Stand::Mine | Stand::Held(_))).collect();
        let mut k = keep.iter();
        wanted.card.files.retain(|_| *k.next().unwrap_or(&false));
        let mut k = keep.iter();
        wanted.extras.retain(|_| *k.next().unwrap_or(&false));
        let (set, slot) = (&*set, self.slot);
        wanted.skipped.retain(|(path, _)| set.owner(&Key::of(path)) == slot);
        Ok(())
    }

    /// A new set of `parts`: this card the first, the others waiting for
    /// their cards, each of `room` as planned.
    fn made(&self, parts: Vec<Part>, room: u64, libraries: &[String]) -> Set {
        let cards = parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let mut slot = Slot::planned(p.from, room, p.load);
                if i == 0 {
                    (slot.card_id, slot.label) = (Some(self.card_id), self.label.clone());
                }
                slot
            })
            .collect();
        Set::new(self.new_id, self.server, libraries, cards)
    }

    /// The card can't take its range: what it is offered, and the listener's
    /// answer taken (true: the ranges changed, and the run plans again). An
    /// answer with no offer to take is said in the notes.
    pub(super) fn decide(&mut self, decision: Option<Decision>, notes: &mut Vec<String>) -> bool {
        let Some(set) = self.set.as_mut() else { return false };
        let room = self.room.unwrap_or(set.cards[self.slot].room);
        self.offers = span::offers(set, self.slot, &self.groups, room);
        let Some(d) = decision.filter(|d| *d != Decision::Spread && !self.weighed) else {
            return false;
        };
        self.weighed = true;
        match d.pick(&self.offers).cloned() {
            Some(offer) => {
                set.apply(&offer, room);
                self.decided = Some(d);
                true
            }
            None => {
                let why = match self.offers.is_empty() {
                    true => "its range fits it: the files it holds for others leave once they sync",
                    false => "there is no next card",
                };
                notes.push(format!("no {} for this card: {why}", d.code()));
                false
            }
        }
    }

    /// An answer for a card that takes its range is said in the notes.
    pub(super) fn unused(&self, decision: Option<Decision>, notes: &mut Vec<String>) {
        if let Some(d) = decision.filter(|d| *d != Decision::Spread && !self.weighed) {
            notes.push(format!("no {} for this card: it takes its range", d.code()));
        }
    }

    /// The listener's answer, or a set made, kept: true once one is.
    pub(super) fn decided(&self) -> bool {
        self.decided.is_some() || self.created
    }

    /// The listener's answer stored on the card with no run (a run that
    /// stops): the card's state file as it was (`was`), with the table and
    /// the card's own facts in it. Said in the notes when it can't be.
    pub(super) fn keep(&mut self, session: &mut Session<'_>, was: &State, notes: &mut Vec<String>) {
        let Some(set) = self.set.as_mut() else { return };
        set.note(self.slot, &self.label, self.room, &self.asked);
        let mut state = was.clone();
        state.set = Some(set.clone());
        let bytes = state.text(volume::next_seq(session.state().seq));
        match session.write_state(&bytes) {
            Ok(()) => {
                self.written = true;
                let why = "the card takes its range once the cards it holds files for have synced";
                notes.push(format!("the set's table is stored on the card: {why}"));
            }
            Err(stop) => notes.push(format!("the set's table wasn't stored: {stop}")),
        }
    }

    pub(super) fn summary(&self) -> Option<SetSummary> {
        let set = self.set.clone()?;
        Some(SetSummary {
            set,
            slot: self.slot,
            created: self.created,
            decided: self.decided,
            mine: self.counts.mine,
            held: self.counts.held.clone(),
            dropped: self.counts.dropped.clone(),
            theirs: self.counts.theirs,
            offers: self.offers.clone(),
            written: self.written,
        })
    }

    /// What the run's end needs of it.
    pub(super) fn end(&self) -> Option<SetEnd> {
        Some(SetEnd {
            set: self.set.clone()?,
            slot: self.slot,
            label: self.label.clone(),
            room: self.room,
            asked: self.asked,
            in_range: self.in_range.clone(),
            cluster: self.cluster,
        })
    }
}

/// A plan of the run's files. The removals of files the card holds for a
/// card that holds its range now are the set's (a move the listener chose,
/// a set made): a gate whose files are all such isn't asked again.
pub(super) fn plan(
    session: &mut Session<'_>,
    wanted: &Wanted,
    answers: &Answers,
    fetcher: &mut dyn Fetcher,
    span: Option<&SetRun>,
) -> Result<Plan, Halted> {
    let made = session.plan(&wanted.card, answers, fetcher).map_err(|s| (None, Halt::Card(s)))?;
    let Some(span) = span.filter(|s| !s.dropped_paths.is_empty()) else { return Ok(made) };
    let set_s = |paths: &[String]| {
        !paths.is_empty() && paths.iter().all(|p| span.dropped_paths.contains(p))
    };
    let mut again = *answers;
    for guard in &made.guards {
        if let Guard::Removals { paths, strong } = guard
            && set_s(paths)
        {
            again.removals = true;
            again.mass_removals |= *strong;
        }
    }
    if again == *answers {
        return Ok(made);
    }
    session.plan(&wanted.card, &again, fetcher).map_err(|s| (None, Halt::Card(s)))
}

/// What a card of no set that doesn't fit would be as a set (the fit's
/// offer): its selection split over cards of its room.
pub(super) fn spread(wanted: &Wanted, held: &Rows, space: Option<Space>) -> Vec<Part> {
    let Some(space) = space else { return Vec::new() };
    let cluster = cluster_of(Some(space));
    let room = plan::usable_room(&space, held.ours.bytes);
    let sizes = wanted.card.files.iter().map(|f| (f.ledger.server_path.as_str(), f.size));
    span::split(&span::groups(sizes, cluster), room, room)
}

impl SetEnd {
    /// The table as the run leaves it: the card's own facts — its label,
    /// room and load, the files it keeps for others — and, after a whole
    /// listing of a run that ended, whether it holds its whole range: every
    /// file of it in the final ledger (but those the names refuse on any
    /// card), the card read back as the device reads it.
    pub(super) fn settle(
        &mut self,
        outcome: &Outcome,
        last: Option<&Final>,
        listed: Listed,
    ) -> Set {
        let set = &mut self.set;
        set.note(self.slot, &self.label, self.room, &self.asked);
        let rev = set.rev;
        let s = &mut set.cards[self.slot];
        if let Some(last) = last {
            (s.bytes, s.tracks) = (last.load.bytes, last.load.tracks);
        }
        if listed == Listed::Whole && outcome.parked.is_none() {
            let refused: HashSet<&str> =
                outcome.report.refused.iter().map(|r| r.server_path.as_str()).collect();
            let landed = |k: &FileKey| {
                refused.contains(k.1.as_str()) || last.is_some_and(|l| l.keys.contains(k))
            };
            let all = last.is_some() && self.in_range.iter().all(landed);
            let all = all && outcome.verified.is_ok();
            let (generation, commit_id) = (outcome.generation, outcome.commit_id);
            s.holds = all.then_some(Holds { rev, asks: self.asked.seen, generation, commit_id });
        }
        set.clone()
    }
}

// ── What the hidden command prints ──────────────────────────────────────────

fn offer_json(o: &Offer) -> Value {
    match o {
        Offer::MoveEdge { to, from, load, fits } => json!({
            "offer": "move-edge", "to": to + 1, "from": from.value(), "bytes": load.bytes,
            "tracks": load.tracks, "fits": fits,
        }),
        Offer::AddCard { at, from, load } => json!({
            "offer": "add-card", "at": at + 1, "from": from.value(), "bytes": load.bytes,
            "tracks": load.tracks,
        }),
    }
}

fn by_card(counts: &BTreeMap<usize, usize>) -> Value {
    let by = counts.iter().map(|(k, n)| ((k + 1).to_string(), Value::from(*n)));
    Value::Object(by.collect())
}

impl SetSummary {
    pub(super) fn json(&self) -> Value {
        json!({
            "set": set_json(&self.set, Some(self.slot)), "created": self.created,
            "decided": self.decided.map(Decision::code), "mine": self.mine,
            "held": by_card(&self.held), "dropped": by_card(&self.dropped), "theirs": self.theirs,
            "offers": self.offers.iter().map(offer_json).collect::<Vec<_>>(),
            "written": self.written,
        })
    }
}

pub(super) fn parts_json(parts: &[Part]) -> Value {
    let part = |p: &Part| {
        json!({ "from": p.from.value(), "bytes": p.load.bytes, "tracks": p.load.tracks })
    };
    Value::Array(parts.iter().map(part).collect())
}
