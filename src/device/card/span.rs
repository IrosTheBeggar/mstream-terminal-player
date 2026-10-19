//! Multi-card sets (the transfer plan's 2.12; part B7): one selection
//! spread over several cards, each a whole library of its own — its own card
//! id, commits, tags file, thumbnails and Auto DJ index — so the player needs
//! nothing new, and Auto DJ's neighbours are each card's own (AUTODJ-INDEX.md
//! 5.1: an index's rows are its card's files).
//!
//! **The split** ([`split`]): the selection's files grouped by their artist
//! folder, the first under their library, then by their album folder
//! ([`Key`]: each name as its match key, so a range reads the same on every
//! computer), and the cards filled in that order, A to Z, to about 90% of
//! their usable room and of the player's track cap, so each has room to grow.
//! An artist goes whole onto one card: only one that no card holds is split,
//! at its album folders, and an album never is.
//!
//! **What each card knows** ([`Set`]). Its `state.json` holds the set's id,
//! the card's index and the whole table — each card's id, label and range
//! (its first key: the ranges cover every key, so any card can say where new
//! music goes) — and this computer's `cards.json` mirrors it. Of two copies
//! the one of the newer ranges counts, and each card's own facts come from the
//! copy it wrote last ([`merge`]).
//!
//! **Re-syncs** ([`stands`]). A card's sync takes the selection's files of
//! its range, so new music lands on its range's card and the split stays where
//! it was. A card that holds files of another's range — the first card of a
//! set made from a full one, an edge moved, a server's move across an edge —
//! keeps them, and asks for them; it lets them go only at a sync after that
//! card has said it holds its whole range, under that ask or a later one
//! ([`Holds`]). That is the two-phase move: the files go to the card of their
//! range first, and leave the other at its next sync after that card
//! committed, so no music is ever only on a card that isn't in the reader. A
//! card that can't take its range is offered to move its edge (its last
//! artists to the next card) or to add a card after it ([`offers`]); the
//! answer is the listener's.
//!
//! The listener's way in is the Card view and the public command line, which
//! wait for their UX contract; until then the hidden `device card-sync`
//! makes a set and syncs its cards (`--spread`, `--move-edge`, `--add-card`),
//! and the hidden `device card-set` shows a card's set and joins a new card
//! to one ([`command`]).

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet};
use std::io::Write;

use serde_json::{Map, Value, json};

use super::cards::Cards;
use super::contract::prim::fnv1a64;
use super::engine::{Answers, Env, Event, Expect, Guard, Knobs, NoHooks, Session, Stop};
use super::names::match_key;
use super::plan::{self, TRACK_CAP};
use super::state::{Settings, State, from_hex, hex};
use super::volume::{self, CardWrite, Found, Mounted, Refusal, Space};

/// The table's own version: a table of another is a newer player's, whose
/// cards this one doesn't sync.
pub(crate) const SET_VERSION: u64 = 1;

/// A card of a set is filled to this share of its usable room, and of the
/// player's track cap: room to grow.
const FILL_PERCENT: u64 = 90;

/// A card's cluster when its volume doesn't say: a big card's.
const CLUSTER: u32 = 32 * 1024;

// ── Where a file goes ───────────────────────────────────────────────────────

/// A file's place in a set's order: its artist folder — the first folder
/// under its library — then its album folder, each as its match key (NFC,
/// upper case). A file at its library's top has no artist (""), one at its
/// artist's top no album; disc folders and deeper are their album's.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Key {
    pub(crate) artist: String,
    pub(crate) album: String,
}

impl Key {
    /// A server path's key (`<vpath>/<artist>/<album>/…/<file>`).
    pub(crate) fn of(server_path: &str) -> Key {
        let names: Vec<&str> = server_path.split('/').collect();
        let folders = names.get(1..names.len().saturating_sub(1)).unwrap_or_default();
        let fold = |i: usize| folders.get(i).map_or_else(String::new, |n| match_key(n));
        Key { artist: fold(0), album: fold(1) }
    }

    /// An artist's first key: all of it, whatever albums it gains.
    fn artist(artist: &str) -> Key {
        Key { artist: artist.to_string(), album: String::new() }
    }

    pub(crate) fn value(&self) -> Value {
        json!([self.artist, self.album])
    }

    fn read(v: &Value) -> Option<Key> {
        let [artist, album] = v.as_array()?.as_slice() else { return None };
        Some(Key { artist: artist.as_str()?.to_string(), album: album.as_str()?.to_string() })
    }
}

// ── The table ───────────────────────────────────────────────────────────────

/// A card's word that it held every file of its range: under the ranges of
/// `rev`, after the asks up to `asks`, at its commit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Holds {
    pub(crate) rev: u32,
    pub(crate) asks: u64,
    pub(crate) generation: u32,
    pub(crate) commit_id: u64,
}

/// A card of a set.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Slot {
    /// Its card id; None while the set waits for its card.
    pub(crate) card_id: Option<u64>,
    pub(crate) label: String,
    /// The first key of its range; the first card's is the least key.
    pub(crate) from: Key,
    /// Its usable room, and the bytes and tracks the transfer's files take
    /// of it ([`plan::track_cost`]): as its last sync measured them, or as
    /// the split planned them for a card not set up yet.
    pub(crate) room: u64,
    pub(crate) bytes: u64,
    pub(crate) tracks: u64,
    /// Counted up by each of the card's own writes of its facts: of two
    /// copies of the table, the one of the higher seq has its newer facts.
    pub(crate) seq: u64,
    pub(crate) holds: Option<Holds>,
    /// The files of other cards' ranges it holds: how many, their key (a
    /// fingerprint of them), and the ask that covers them.
    pub(crate) others: u64,
    pub(crate) others_key: u64,
    pub(crate) asked: u64,
    /// The members this software doesn't know, kept.
    raw: Map<String, Value>,
}

/// A set's table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Set {
    pub(crate) id: u64,
    /// The ranges' revision: each move of an edge, each card added, counts
    /// it up, and a card's word that it holds its range ([`Holds`]) counts
    /// only under the revision it was given at.
    pub(crate) rev: u32,
    /// The asks so far: a card that holds files of others' ranges asks for
    /// them by the next number.
    pub(crate) asks: u64,
    /// The server every card of it is filled from (its URL's key), and the
    /// libraries, by vpath: one selection.
    pub(crate) server: u64,
    pub(crate) libraries: Vec<String>,
    /// In their ranges' order, each card's first key above the one before.
    pub(crate) cards: Vec<Slot>,
    raw: Map<String, Value>,
}

/// The members a table's JSON and a card's hold, written in this order.
const SET_MEMBERS: [&str; 8] =
    ["version", "id", "rev", "asks", "server", "libraries", "index", "cards"];
const SLOT_MEMBERS: [&str; 12] = [
    "cardId", "label", "from", "room", "bytes", "tracks", "seq", "holds", "others", "othersKey",
    "asked", "index",
];

fn number(o: &Map<String, Value>, key: &str) -> u64 {
    o.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn unknown(o: &Map<String, Value>, known: &[&str]) -> Map<String, Value> {
    let unknown = o.iter().filter(|(k, _)| !known.contains(&k.as_str()));
    unknown.map(|(k, v)| (k.clone(), v.clone())).collect()
}

impl Holds {
    fn read(v: &Value) -> Option<Holds> {
        let o = v.as_object()?;
        Some(Holds {
            rev: u32::try_from(number(o, "rev")).ok()?,
            asks: number(o, "asks"),
            generation: u32::try_from(number(o, "generation")).ok()?,
            commit_id: o.get("commitId").and_then(Value::as_str).and_then(from_hex)?,
        })
    }

    fn value(&self) -> Value {
        json!({
            "rev": self.rev, "asks": self.asks, "generation": self.generation,
            "commitId": hex(self.commit_id),
        })
    }
}

impl Slot {
    fn read(v: &Value) -> Option<Slot> {
        let o = v.as_object()?;
        let card_id = match o.get("cardId") {
            None | Some(Value::Null) => None,
            Some(id) => Some(id.as_str().and_then(from_hex).filter(|&id| id != 0)?),
        };
        let holds = match o.get("holds") {
            None | Some(Value::Null) => None,
            Some(h) => Some(Holds::read(h)?),
        };
        Some(Slot {
            card_id,
            label: o.get("label").and_then(Value::as_str).unwrap_or_default().to_string(),
            from: Key::read(o.get("from")?)?,
            room: number(o, "room"),
            bytes: number(o, "bytes"),
            tracks: number(o, "tracks"),
            seq: number(o, "seq"),
            holds,
            others: number(o, "others"),
            others_key: o.get("othersKey").and_then(Value::as_str).and_then(from_hex).unwrap_or(0),
            asked: number(o, "asked"),
            raw: unknown(o, &SLOT_MEMBERS),
        })
    }

    fn value(&self) -> Value {
        let mut o = self.raw.clone();
        let mut set = |name: &str, value: Value| {
            o.insert(name.to_string(), value);
        };
        set("cardId", self.card_id.map_or(Value::Null, |id| hex(id).into()));
        set("label", self.label.as_str().into());
        set("from", self.from.value());
        set("room", self.room.into());
        set("bytes", self.bytes.into());
        set("tracks", self.tracks.into());
        set("seq", self.seq.into());
        set("holds", self.holds.map_or(Value::Null, |h| h.value()));
        set("others", self.others.into());
        set("othersKey", hex(self.others_key).into());
        set("asked", self.asked.into());
        Value::Object(o)
    }

    /// A card a split planned, waiting for its card: its range's first key,
    /// its room, and its load.
    pub(crate) fn planned(from: Key, room: u64, load: Load) -> Slot {
        Slot { from, room, bytes: load.bytes, tracks: load.tracks, ..Slot::default() }
    }

    /// What its range takes of it now.
    pub(crate) fn load(&self) -> Load {
        Load { bytes: self.bytes, tracks: self.tracks }
    }
}

impl Set {
    /// A new set of `cards`, made from the server at `server`'s `libraries`.
    pub(crate) fn new(id: u64, server: u64, libraries: &[String], cards: Vec<Slot>) -> Set {
        let mut libraries = libraries.to_vec();
        libraries.sort();
        libraries.dedup();
        Set { id, rev: 1, asks: 0, server, libraries, cards, raw: Map::new() }
    }

    /// A table as a card's state file or `cards.json` holds it; None for one
    /// this software can't read: of another version, or whose ranges don't
    /// cover every key once each.
    pub(crate) fn read(v: &Value) -> Option<Set> {
        let o = v.as_object()?;
        if o.get("version").and_then(Value::as_u64) != Some(SET_VERSION) {
            return None;
        }
        let libraries = o.get("libraries")?.as_array()?;
        let libraries: Option<Vec<String>> =
            libraries.iter().map(|l| l.as_str().map(str::to_string)).collect();
        let cards: Option<Vec<Slot>> = o.get("cards")?.as_array()?.iter().map(Slot::read).collect();
        let set = Set {
            id: o.get("id").and_then(Value::as_str).and_then(from_hex).filter(|&id| id != 0)?,
            rev: u32::try_from(number(o, "rev")).ok().filter(|&r| r > 0)?,
            asks: number(o, "asks"),
            server: o.get("server").and_then(Value::as_str).and_then(from_hex)?,
            libraries: libraries?,
            cards: cards?,
            raw: unknown(o, &SET_MEMBERS),
        };
        set.valid().then_some(set)
    }

    /// Its ranges cover every key once each, and no card is in it twice.
    fn valid(&self) -> bool {
        let Some(first) = self.cards.first() else { return false };
        let rising = self.cards.windows(2).all(|w| w[0].from < w[1].from);
        let ids: Vec<u64> = self.cards.iter().filter_map(|s| s.card_id).collect();
        let distinct = ids.iter().collect::<HashSet<_>>().len() == ids.len();
        first.from == Key::default() && rising && distinct
    }

    /// Its JSON; `index`, the reading card's own place in it, from 1.
    pub(crate) fn value(&self, index: Option<usize>) -> Value {
        let mut o = self.raw.clone();
        o.insert("version".into(), SET_VERSION.into());
        o.insert("id".into(), hex(self.id).into());
        o.insert("rev".into(), self.rev.into());
        o.insert("asks".into(), self.asks.into());
        o.insert("server".into(), hex(self.server).into());
        o.insert("libraries".into(), self.libraries.clone().into());
        if let Some(index) = index {
            o.insert("index".into(), index.into());
        }
        o.insert("cards".into(), Value::Array(self.cards.iter().map(Slot::value).collect()));
        Value::Object(o)
    }

    /// The card whose range holds `key`: the last whose first key is at
    /// most it.
    pub(crate) fn owner(&self, key: &Key) -> usize {
        self.cards.iter().rposition(|s| s.from <= *key).unwrap_or(0)
    }

    /// Where card `i`'s range ends: the next card's first key; None for the
    /// last card's, which runs to the end.
    pub(crate) fn until(&self, i: usize) -> Option<&Key> {
        self.cards.get(i + 1).map(|s| &s.from)
    }

    /// The card of this id.
    pub(crate) fn slot_of(&self, card_id: u64) -> Option<usize> {
        self.cards.iter().position(|s| s.card_id == Some(card_id))
    }

    /// Card `i` holds its whole range as the table stands now, after every
    /// ask up to `asked`.
    pub(crate) fn holds(&self, i: usize, asked: u64) -> bool {
        let s = &self.cards[i];
        let now = |h: &Holds| h.rev == self.rev && h.asks >= asked;
        s.card_id.is_some() && s.holds.as_ref().is_some_and(now)
    }

    /// Card `i`'s range as text: what a plan of it depends on beside the
    /// card's settings.
    pub(crate) fn range_text(&self, i: usize) -> String {
        let text = |k: &Key| format!("{}\u{1f}{}", k.artist, k.album);
        let until = self.until(i).map_or_else(String::new, text);
        format!("set {} rev {}: {} .. {until}", hex(self.id), self.rev, text(&self.cards[i].from))
    }

    /// The listener's answer to an offer ([`offers`]): the ranges change (a
    /// new revision: no card's word that it holds its range counts any
    /// more), and a card added takes `room`, as planned.
    pub(crate) fn apply(&mut self, offer: &Offer, room: u64) {
        match offer {
            Offer::MoveEdge { to, from, load, .. } => {
                let next = &mut self.cards[*to];
                next.from = from.clone();
                if next.card_id.is_none() {
                    let (bytes, tracks) = (next.bytes + load.bytes, next.tracks + load.tracks);
                    (next.bytes, next.tracks) = (bytes, tracks);
                }
            }
            Offer::AddCard { at, from, load } => {
                self.cards.insert(*at, Slot::planned(from.clone(), room, *load));
            }
        }
        self.rev += 1;
    }
}

/// Two copies of one set's table — a card's own and this computer's mirror,
/// or the mirror and a card's — as one: the copy of the newer ranges, `own`
/// on a tie; each card's facts from the copy that has them newest (its
/// seq); a card that joined under the same ranges, named in either; and the
/// asks, the most either counted. Another set's copy changes nothing.
pub(crate) fn merge(own: &Set, other: &Set) -> Set {
    if own.id != other.id {
        return own.clone();
    }
    let (mut base, alt) = match other.rev > own.rev {
        true => (other.clone(), own),
        false => (own.clone(), other),
    };
    base.asks = base.asks.max(alt.asks);
    let same_ranges = base.rev == alt.rev
        && base.cards.len() == alt.cards.len()
        && base.cards.iter().zip(&alt.cards).all(|(a, b)| a.from == b.from);
    let named: HashSet<u64> = base.cards.iter().filter_map(|s| s.card_id).collect();
    for (i, slot) in base.cards.iter_mut().enumerate() {
        if slot.card_id.is_none()
            && same_ranges
            && alt.cards[i].card_id.is_some_and(|id| !named.contains(&id))
        {
            *slot = alt.cards[i].clone();
            continue;
        }
        let Some(id) = slot.card_id else { continue };
        if let Some(theirs) = alt.cards.iter().find(|s| s.card_id == Some(id))
            && theirs.seq > slot.seq
        {
            *slot = Slot { from: slot.from.clone(), ..theirs.clone() };
        }
    }
    base
}

// ── The split ───────────────────────────────────────────────────────────────

/// What files take of a card: bytes ([`plan::track_cost`]), and tracks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Load {
    pub(crate) bytes: u64,
    pub(crate) tracks: u64,
}

impl Load {
    fn add(self, other: Load) -> Load {
        Load { bytes: self.bytes + other.bytes, tracks: self.tracks + other.tracks }
    }

    fn within(self, room: Load) -> bool {
        self.bytes <= room.bytes && self.tracks <= room.tracks
    }
}

/// What the split fills a card of `room` usable bytes to: about 90% of
/// them, and of the track cap.
pub(crate) fn target(room: u64) -> Load {
    let tracks = TRACK_CAP as u64 * FILL_PERCENT / 100;
    Load { bytes: room / 100 * FILL_PERCENT, tracks }
}

/// An album folder of the selection (or an artist's loose files), and its
/// load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub(crate) key: Key,
    pub(crate) load: Load,
}

/// The groups of `files` (each by its server path, and its bytes on the
/// card), on a card of `cluster`-byte clusters, in key order.
pub(crate) fn groups<'a>(
    files: impl IntoIterator<Item = (&'a str, u64)>,
    cluster: u32,
) -> Vec<Group> {
    let mut by: BTreeMap<Key, Load> = BTreeMap::new();
    for (path, size) in files {
        let album = Load { bytes: plan::album_cost(cluster), tracks: 0 };
        let load = by.entry(Key::of(path)).or_insert(album);
        *load = load.add(Load { bytes: plan::track_cost(size, cluster), tracks: 1 });
    }
    by.into_iter().map(|(key, load)| Group { key, load }).collect()
}

fn total(groups: &[Group]) -> Load {
    groups.iter().fold(Load::default(), |l, g| l.add(g.load))
}

/// How many of `groups`, from the first, a card filled to `room` takes:
/// whole artists while they fit; an artist no card holds, its albums from
/// wherever it begins, as many as fit; one a card holds goes whole to the
/// next card. One group at least: an album bigger than a card goes alone,
/// never split.
fn fill(groups: &[Group], room: Load) -> usize {
    let (mut used, mut at) = (Load::default(), 0);
    while at < groups.len() {
        let artist = &groups[at].key.artist;
        let end = at + groups[at..].iter().take_while(|g| g.key.artist == *artist).count();
        let whole = total(&groups[at..end]);
        if used.add(whole).within(room) {
            (used, at) = (used.add(whole), end);
            continue;
        }
        if !whole.within(room) {
            for g in &groups[at..end] {
                if at > 0 && !used.add(g.load).within(room) {
                    break;
                }
                (used, at) = (used.add(g.load), at + 1);
            }
        }
        break;
    }
    at.max(1).min(groups.len())
}

/// The first key of a card that begins at `groups[at]`: its artist's, when
/// the artist begins there, so the artist's albums to come go there too;
/// else the album's.
fn edge(groups: &[Group], at: usize) -> Key {
    let key = &groups[at].key;
    match at.checked_sub(1) {
        Some(before) if groups[before].key.artist == key.artist => key.clone(),
        _ => Key::artist(&key.artist),
    }
}

/// A card of a split: its first key, and what it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Part {
    pub(crate) from: Key,
    pub(crate) load: Load,
}

/// The selection's `groups` over as many cards as it needs, A to Z (see the
/// module's note): the first card of `first` usable bytes, each other of
/// `others`. The first card's range begins at the least key.
pub(crate) fn split(groups: &[Group], first: u64, others: u64) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    let mut at = 0;
    loop {
        let room = target(if parts.is_empty() { first } else { others });
        let n = fill(&groups[at..], room);
        let from = if parts.is_empty() { Key::default() } else { edge(groups, at) };
        parts.push(Part { from, load: total(&groups[at..at + n]) });
        at += n;
        if at >= groups.len() {
            return parts;
        }
    }
}

/// What a card of a set that can't take its range is offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Offer {
    /// Its last artists (or albums), from `from` on, go to the next card,
    /// `to`; `fits`: that card's room, as last seen, takes them too.
    MoveEdge { to: usize, from: Key, load: Load, fits: bool },
    /// A new card after it, at `at`, takes them.
    AddCard { at: usize, from: Key, load: Load },
}

/// The listener's answer to a card that doesn't fit, or a card's asking to
/// be spread over a new set (the sync's `Ask`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Make a set of the card's selection, this card its first.
    Spread,
    MoveEdge,
    AddCard,
}

impl Decision {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Decision::Spread => "spread",
            Decision::MoveEdge => "move-edge",
            Decision::AddCard => "add-card",
        }
    }

    /// The offer it takes, of those made.
    pub(crate) fn pick(self, offers: &[Offer]) -> Option<&Offer> {
        offers.iter().find(|o| match self {
            Decision::MoveEdge => matches!(o, Offer::MoveEdge { .. }),
            Decision::AddCard => matches!(o, Offer::AddCard { .. }),
            Decision::Spread => false,
        })
    }
}

/// What card `i` of `set` is offered when it can't take its range: its
/// range's `groups` filled to its `room` say where its edge goes. None when
/// its range fills no more than that: the card is full of something else
/// (files it holds for other cards, the listener's own).
pub(crate) fn offers(set: &Set, i: usize, groups: &[Group], room: u64) -> Vec<Offer> {
    let n = fill(groups, target(room));
    if n >= groups.len() {
        return Vec::new();
    }
    let (from, load) = (edge(groups, n), total(&groups[n..]));
    let mut offers = Vec::new();
    if let Some(next) = set.cards.get(i + 1) {
        let fits = next.load().add(load).within(target(next.room));
        offers.push(Offer::MoveEdge { to: i + 1, from: from.clone(), load, fits });
    }
    offers.push(Offer::AddCard { at: i + 1, from, load });
    offers
}

// ── A card's sync ───────────────────────────────────────────────────────────

/// Where a file of the selection stands for a card's sync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stand {
    /// Of its range: on this card.
    Mine,
    /// Of card `n`'s range, and on this card: it stays until that card
    /// holds its range.
    Held(usize),
    /// Of card `n`'s range, on this card, and that card holds it: it goes.
    Dropped(usize),
    /// Of card `n`'s range, and not on this card.
    Theirs(usize),
}

/// What a card's sync asked of its set: the asks it found, and its own ask
/// for the files it holds of other ranges, with their key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Asked {
    pub(crate) seen: u64,
    pub(crate) asked: u64,
    /// The key of the files it keeps of other ranges, and how many.
    pub(crate) others_key: u64,
    pub(crate) others: u64,
}

fn fingerprint(ids: &mut [&str]) -> u64 {
    if ids.is_empty() {
        return 0;
    }
    ids.sort_unstable();
    fnv1a64(ids.join("\n").as_bytes()).max(1)
}

/// Where each of a sync's files stands for card `i` of `set`: each by its
/// key, and its id (origin and server path) when the card holds it. A file
/// of another's range that the card holds stays until that card holds its
/// range after the ask that covers it; the files held changed since the
/// card last asked — or never asked — are a new ask (the set's asks counted
/// up), which lets none of them go this time.
pub(crate) fn stands(
    set: &mut Set,
    i: usize,
    files: &[(Key, Option<String>)],
) -> (Asked, Vec<Stand>) {
    let owners: Vec<usize> = files.iter().map(|(k, _)| set.owner(k)).collect();
    let held = files.iter().zip(&owners).filter(|(_, o)| **o != i);
    let mut ids: Vec<&str> = held.filter_map(|((_, id), _)| id.as_deref()).collect();
    let key = fingerprint(&mut ids);
    let seen = set.asks;
    let me = &set.cards[i];
    let fresh = key != 0 && (key != me.others_key || me.asked == 0);
    let asked = if fresh {
        set.asks += 1;
        set.asks
    } else {
        me.asked
    };
    let stands: Vec<Stand> = files
        .iter()
        .zip(&owners)
        .map(|((_, id), &o)| match (o == i, id.is_some()) {
            (true, _) => Stand::Mine,
            (false, false) => Stand::Theirs(o),
            (false, true) if !fresh && set.holds(o, asked) => Stand::Dropped(o),
            (false, true) => Stand::Held(o),
        })
        .collect();
    let mut kept: Vec<&str> = files
        .iter()
        .zip(&stands)
        .filter(|(_, s)| matches!(s, Stand::Held(_)))
        .filter_map(|((_, id), _)| id.as_deref())
        .collect();
    let others = kept.len() as u64;
    let asked = Asked { seen, asked, others_key: fingerprint(&mut kept), others };
    (asked, stands)
}

impl Set {
    /// Card `i`'s own facts after a sync that asked `asked`: its label, its
    /// room, what it holds of others; its seq counted up.
    pub(crate) fn note(&mut self, i: usize, label: &str, room: Option<u64>, asked: &Asked) {
        let s = &mut self.cards[i];
        s.label = label.to_string();
        s.room = room.unwrap_or(s.room);
        (s.others, s.others_key, s.asked) = (asked.others, asked.others_key, asked.asked);
        s.seq += 1;
    }
}

// ── A card joins a set ──────────────────────────────────────────────────────

/// What a join found and did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Joined {
    pub(crate) set: Set,
    /// The card's place in it, from 0.
    pub(crate) slot: usize,
    pub(crate) card_id: u64,
    /// It was that card already: nothing written.
    pub(crate) already: bool,
    pub(crate) wrote: bool,
}

fn refused(why: impl Into<String>) -> Stop {
    Stop::Refused(why.into())
}

/// What the files a card's ledger lists take of it.
pub(crate) fn load_of(root: &volume::Root, cluster: u32) -> Load {
    let records = root.tags.as_ref().map(|t| t.records.as_slice()).unwrap_or_default();
    let bytes = records.iter().map(|e| plan::track_cost(e.rec.size as u64, cluster)).sum();
    Load { bytes, tracks: records.len() as u64 }
}

/// A set-up card made card `index` (from 1) of the set of `set_id`, a set
/// this computer knows (`cards`, its mirror): the set's table goes into the
/// card's state file with this card's id in that place, and the set's
/// selection into its settings; `cards` is brought up to date (the caller
/// saves it). The card's own files stay: those of other cards' ranges leave
/// it as any card's do ([`stands`]). A card of another set, a place another
/// card has, a card not set up, are refused; the card already in that place
/// is left as it is. `dry_run`: nothing is written.
#[allow(clippy::too_many_arguments)]
pub(crate) fn join(
    card: &dyn CardWrite,
    space: Option<Space>,
    set_id: u64,
    index: usize,
    dry_run: bool,
    cards: &mut Cards,
    envs: &mut dyn FnMut() -> Env,
    events: &mut dyn FnMut(Event),
) -> Result<Joined, Stop> {
    let seen = volume::inspect(card).map_err(|_| Stop::Pulled)?;
    if !seen.newer.is_empty() {
        return Err(Stop::Guard(Guard::Newer(seen.newer)));
    }
    let not_set_up = "the card isn't set up for the transfer: run `device card-setup` on it first";
    let Some(root) = seen.root.as_ref() else { return Err(refused(not_set_up)) };
    let (card_id, generation, commit_id) =
        (root.manifest.header.card_id, root.manifest.header.generation, root.manifest.commit_id);
    let Some(mut state) = State::of(&seen.state).filter(|s| s.card_id == card_id) else {
        return Err(refused(not_set_up));
    };
    if state.set_unread {
        return Err(refused("the card is in a set of a newer player's"));
    }
    let Some(known) = cards.sets.get(&set_id) else {
        return Err(refused(format!("this computer knows no set {}", hex(set_id))));
    };
    let mut set = match &state.set {
        Some(own) if own.id != set_id => {
            return Err(refused(format!("the card is in another set, {}", hex(own.id))));
        }
        Some(own) => merge(own, known),
        None => known.clone(),
    };
    let Some(at) = index.checked_sub(1).filter(|&i| i < set.cards.len()) else {
        return Err(refused(format!("the set has no card {index}")));
    };
    match (set.slot_of(card_id), set.cards[at].card_id) {
        (Some(i), _) if i == at && state.set.is_some() => {
            return Ok(Joined { set, slot: at, card_id, already: true, wrote: false });
        }
        (Some(i), _) if i != at => {
            return Err(refused(format!("the card is the set's card {}", i + 1)));
        }
        (None, Some(_)) => return Err(refused(format!("the set's card {index} is another card"))),
        _ => {}
    }
    let cluster = space.map_or(CLUSTER, |s| s.cluster);
    let load = load_of(root, cluster);
    let room = space.map(|s| plan::usable_room(&s, load.bytes));
    let slot = &mut set.cards[at];
    slot.card_id = Some(card_id);
    (slot.bytes, slot.tracks) = (load.bytes, load.tracks);
    set.note(at, &state.label.clone(), room, &Asked::default());
    // The set's selection is this card's from now on.
    if !state.settings.stored {
        state.settings = Settings::defaults();
    }
    state.settings.source(set.server).libraries = set.libraries.clone();
    state.set = Some(set.clone());
    let mut joined = Joined { set, slot: at, card_id, already: false, wrote: false };
    if dry_run {
        return Ok(joined);
    }
    let serial = card.serial().map_err(|_| Stop::Pulled)?;
    let expect = Expect { serial, card_id: Some(card_id), commit_id: Some(commit_id) };
    let mut env = envs();
    let now = (env.now)();
    let (knobs, answers) = (Knobs::default(), Answers::default());
    let mut session = Session::open(card, &expect, knobs, &answers, env, &mut NoHooks, events)?;
    let bytes = state.text(volume::next_seq(session.state().seq));
    if let Err(stop) = session.write_state(&bytes) {
        session.close();
        return Err(stop);
    }
    session.close();
    joined.wrote = true;
    cards.saw(card_id, &state.label, now, generation, commit_id);
    cards.mirror(&joined.set);
    Ok(joined)
}

// ── The hidden command ──────────────────────────────────────────────────────

/// `device card-set <volume> [--join <SET> --index <N>] [--dry-run]`,
/// undocumented: the set of the card at `volume`, which must be one the
/// volume layer accepts — a refused volume, or one that isn't a card at all,
/// is never opened — as the card and this computer know it, with the sets
/// this computer knows that wait for a card; or, with `join` (the set's id,
/// the card's index from 1), the card made that card of the set ([`join`]).
/// Prints JSON. For the real-card checks: the listener's Card view and its
/// command line wait for their UX contract. Exit 0; 1 when the join was
/// refused; 2 when the volume isn't one it may write, or the volumes can't
/// be listed.
pub(crate) fn command(
    volume: &str,
    join: Option<(u64, usize)>,
    dry_run: bool,
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
    let code = on(&found, volume, join, dry_run, &mut cards, &mut Env::system, out, err);
    if code == 0
        && join.is_some()
        && !dry_run
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
    join_as: Option<(u64, usize)>,
    dry_run: bool,
    cards: &mut Cards,
    envs: &mut dyn FnMut() -> Env,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(found) = super::setup::pick(found, named) else {
        let _ = writeln!(err, "mstream-player: {named} isn't a volume this computer lists");
        return 2;
    };
    let Some(card) = Mounted::of(found) else {
        let codes: Vec<&str> = found.refusals.iter().map(Refusal::code).collect();
        let _ = writeln!(err, "mstream-player: {named} is refused: {}", codes.join(", "));
        return 2;
    };
    let space = found.volume.space;
    let json = match join_as {
        Some((set_id, index)) => {
            let mut said = Vec::new();
            let mut say = |e| said.push(e);
            match join(&card, space, set_id, index, dry_run, cards, envs, &mut say) {
                Ok(j) => json!({
                    "dryRun": dry_run, "cardId": hex(j.card_id), "already": j.already,
                    "wrote": j.wrote, "set": set_json(&j.set, Some(j.slot)),
                }),
                Err(stop) => {
                    for e in &said {
                        let _ = writeln!(err, "mstream-player: {e:?}");
                    }
                    let _ = writeln!(err, "mstream-player: the join stopped: {stop}");
                    return 1;
                }
            }
        }
        None => match look(&card, cards) {
            Ok(json) => json,
            Err(e) => {
                let _ = writeln!(err, "mstream-player: the card can't be read: {e}");
                return 1;
            }
        },
    };
    let _ = writeln!(out, "{}", serde_json::to_string_pretty(&json).unwrap_or_default());
    0
}

/// The card's set as it and this computer know it, read only; and the sets
/// this computer knows that wait for a card.
fn look(card: &dyn CardWrite, cards: &Cards) -> std::io::Result<Value> {
    let seen = volume::inspect(card)?;
    let card_id = seen.root.as_ref().map(|r| r.manifest.header.card_id);
    let state = State::of(&seen.state).filter(|s| Some(s.card_id) == card_id);
    let set = state.as_ref().and_then(|s| s.set.as_ref());
    let merged = set.map(|own| cards.set_of(own));
    let me = merged.as_ref().zip(card_id).and_then(|(s, id)| s.slot_of(id));
    let waiting: Vec<Value> = cards
        .sets
        .values()
        .filter(|s| s.cards.iter().any(|c| c.card_id.is_none()))
        .map(|s| set_json(s, None))
        .collect();
    Ok(json!({
        "cardId": card_id.map(hex),
        "setUnread": state.as_ref().is_some_and(|s| s.set_unread),
        "set": merged.as_ref().map(|s| set_json(s, me)),
        "waiting": waiting,
    }))
}

/// A table as the hidden commands print it: each card from 1, its range's
/// first key and its end.
pub(crate) fn set_json(set: &Set, me: Option<usize>) -> Value {
    let cards: Vec<Value> = set
        .cards
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "index": i + 1, "cardId": s.card_id.map(hex), "label": s.label,
                "from": s.from.value(), "until": set.until(i).map(Key::value),
                "room": s.room, "bytes": s.bytes, "tracks": s.tracks,
                "holdsRange": set.holds(i, 0), "holds": s.holds.map(|h| h.value()),
                "others": s.others, "asked": s.asked,
            })
        })
        .collect();
    json!({
        "id": hex(set.id), "rev": set.rev, "asks": set.asks, "libraries": set.libraries,
        "index": me.map(|i| i + 1), "count": set.cards.len(), "cards": cards,
    })
}

