//! A card's play history, to its server (the transfer plan's part B15; the
//! firmware repo's docs/METADATA.md 2.20.6 and 2.5.6). The player logs
//! every play of a card file in `/.player/plays*.jnl` ([`mppl`]); with the
//! card in this computer's reader, the software reads the log, maps each new
//! play through the card's ledger to the server file it is a copy of, and
//! posts it to `POST /api/v1/stats/plays`, backdated by its own start and
//! idempotent by its id.
//!
//! **A look** ([`History::look`]: the engine's plays hook, step 5 of a run,
//! once the card is listed; or the hidden `device card-plays`, which only
//! reads the card):
//! - the card's part of the outbox taken, whatever the card holds now: the
//!   plays mapped before wait there for their server;
//! - the log's files read, each slot judged by the kit; a card that fails a
//!   read is looked at no further, so no play is ever passed over;
//! - the new plays: above the posted mark of their device and epoch in the
//!   card's `state.json` (2.12.7);
//! - each mapped through the tags file of the root that counts, by its path
//!   hash, held to its row's size and FAT time under the listing's skew
//!   ([`resolve`]): the row's server path on the row's server, or not sent
//!   and counted ([`Unsent`]: the listener's own file, a file that changed,
//!   a play from before the clock was ever set, a row whose server the card
//!   can't tell);
//! - the plays to send saved in the outbox ([`Outbox`], `card-plays.json`
//!   in the config folder, written whole) before the run writes anything:
//!   a checkpoint deletes the tags file they map through. A sync whose
//!   plays to send are in no outbox after its post, since a save failed,
//!   stops before it plans ([`History::unstored`]).
//!
//! **The post** ([`History::post`]): a path the server moved since the
//! ledger was written is found again by its audio hash in the server's
//! whole manifest ([`History::remap`]); then the plays whose row's server
//! is the one read now (2.11's test; a peer's go to its parent, with the
//! peer's id and a snapshot of the track) go in batches of 200, each answer
//! saved as it comes: accepted, duplicates and rejected alike are settled.
//! A play the server's schema refuses is never sent and settles as one not
//! sent: it would fail every batch it rode in. Plays of another server, and
//! those of a post that failed, wait in the outbox for a run that reads
//! their server.
//!
//! **The marks** ([`History::marks`]): each device and epoch's rises over
//! the plays settled or not sent, to the first that waits. A sync stores
//! them in the card's state file at its end, and raises its commits' ACKS
//! to them ([`History::acks`]) so the device may trim its log, only in a
//! commit the run makes anyway: `device card-plays` writes nothing to the
//! card, ACKS included. Only the card holds its marks: the outbox keeps the
//! ids the server settled, which are the board's own (a MAC, a random
//! epoch), so a card copied with its id never takes another's plays for
//! settled. A crash between a post and the outbox saying so only sends
//! those plays again, which the server answers as duplicates.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{self, Write};
use std::rc::Rc;

use serde_json::{Map, Value, json};

use super::contract::device_txt::DeviceTxt;
use super::contract::fat::skew;
use super::contract::mppl::{self, FILE_BYTES, Log, LogFile, NAMES, Slot};
use super::contract::mptg::{Entry, Ledger, Tags};
use super::contract::msmf::Ack;
use super::contract::prim::{hex, hex16};
use super::contract::server::{NO_INSTANCE, Server, Servers, Talk, Verdict, server_url_key};
use super::contract::text::{ALBUM, ARTIST, TITLE};
use super::engine::Hooks;
use super::names::match_key;
use super::state::{State, device_of, from_hex, hex as id_hex, marks_value};
use super::volume::{self, CardFs, Mounted, Root};
use crate::api::transfer::{
    CardPlay, MANIFEST_PAGE_MAX, Manifest, ManifestEntry, PLAYS_PER_CALL, PlayClient, PlayOutcome,
    PlaySource, PlayTrack, play_fault, play_version,
};
use crate::api::{ApiError, Client, wait};

/// Who reports a card's plays: the player, by its card (2.20.6's table).
pub(crate) const CLIENT: &str = "mstream-core2";

// ── Where a play goes ───────────────────────────────────────────────────────

/// The server a play is posted to: the one that filled its row, or for a
/// federation peer's row the peer's parent, with the peer's id there
/// (2.5.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct To {
    /// A server reached by its own URL, never a peer: its instance when the
    /// card knows it, its URL's key.
    pub(crate) server: Server,
    pub(crate) peer: Option<u32>,
}

impl To {
    /// Where a play of a row of server `index` goes; None for a server the
    /// card can't tell (2.5.5: an entry that breaks a writer rule, or none).
    pub(crate) fn of(servers: &Servers, index: usize) -> Option<To> {
        let srv = servers.get(index)?;
        if !srv.is_peer() {
            return Some(To { server: Server::direct(srv.instance, srv.url_key), peer: None });
        }
        // The parent as the card has it, else by its key alone.
        let parent = servers.parent(srv.parent_key).and_then(|i| servers.get(i));
        let server = match parent {
            Some(p) => Server::direct(p.instance, p.url_key),
            None => Server::direct(NO_INSTANCE, srv.parent_key),
        };
        Some(To { server, peer: Some(srv.peer_id) })
    }

    /// The server read now is this one, by 2.11's test: a play goes only
    /// there.
    pub(crate) fn reached(&self, talk: &Talk) -> bool {
        let talk = Talk { peer: None, ..talk.clone() };
        Servers { root: self.server, entries: Vec::new() }.same_root(&talk) == Verdict::Same
    }
}

// ── A play to send ──────────────────────────────────────────────────────────

/// What a peer's play says of its track, which its parent can't look up
/// (mStream's federated plays): from the row's record, and the row's file
/// hash as the player's own plays of a peer's track give it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) duration_ms: u32,
    /// Lowercase hex; "" for none.
    pub(crate) hash: String,
}

/// A play mapped to its server's file, waiting to be posted: what the
/// outbox keeps of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Owed {
    /// The play's id (2.20.4), and its key.
    pub(crate) id: String,
    pub(crate) device_id: [u8; 6],
    pub(crate) epoch: u64,
    pub(crate) seq: u32,
    pub(crate) to: To,
    /// `vpath/rel` on that server: the row's server path.
    pub(crate) file_path: String,
    /// The row's audio hash, to find a path the server moved since; zeros
    /// for none.
    pub(crate) audio_hash: [u8; 16],
    /// Unix ms, as dated (2.20.6, item 7).
    pub(crate) started_at: u64,
    pub(crate) played_ms: u32,
    /// 0: not said.
    pub(crate) duration_ms: u32,
    /// The slot's codes, unknown values read as 2.20.3 says: an outcome of
    /// 1-3, a source of 0-3.
    pub(crate) outcome: u8,
    pub(crate) source: u8,
    pub(crate) pause_count: u16,
    /// A peer's track.
    pub(crate) track: Option<Snapshot>,
}

impl Owed {
    fn of(p: &Slot, to: To, e: &Entry, ledger: &Ledger, started_at: u64) -> Owed {
        let artists: Vec<String> =
            e.run.values(ARTIST).map(|v| String::from_utf8_lossy(v).into()).collect();
        let hash = [ledger.file_hash, ledger.audio_hash].into_iter().find(|h| *h != [0; 16]);
        let track = to.peer.map(|_| Snapshot {
            title: e.run.text(TITLE).into(),
            artist: artists.join(", "),
            album: e.run.text(ALBUM).into(),
            duration_ms: if p.duration_ms != 0 { p.duration_ms } else { e.rec.duration_ms },
            hash: hash.map_or_else(String::new, |h| hex(&h)),
        });
        Owed {
            id: p.id(),
            device_id: p.device_id,
            epoch: p.epoch,
            seq: p.seq,
            to,
            file_path: ledger.server_path.clone(),
            audio_hash: ledger.audio_hash,
            started_at,
            played_ms: p.played_ms,
            duration_ms: p.duration_ms,
            outcome: if (1..=3).contains(&p.outcome) { p.outcome } else { 3 },
            source: if p.source <= 3 { p.source } else { 0 },
            pause_count: p.pause_count,
            track,
        }
    }

    /// The play as `stats/plays` takes it. A source the device didn't say
    /// is `device` where the server takes it, else `other`: 2.20.6's table,
    /// proposed before mStream had a `device` source, says `other`; the
    /// plan's mStream batch (B11) added `device` for these plays.
    fn play(&self, device: bool) -> CardPlay {
        let outcome = match self.outcome {
            1 => PlayOutcome::Completed,
            2 => PlayOutcome::Skipped,
            _ => PlayOutcome::Stopped,
        };
        let source = match self.source {
            1 => PlaySource::Manual,
            2 => PlaySource::Shuffle,
            3 => PlaySource::Autodj,
            _ if device => PlaySource::Device,
            _ => PlaySource::Other,
        };
        let text = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let track = self.track.as_ref().map(|t| PlayTrack {
            title: text(&t.title),
            artist: text(&t.artist),
            album: text(&t.album),
            duration_ms: (t.duration_ms != 0).then_some(t.duration_ms as u64),
            hash: text(&t.hash),
            art_file: None,
        });
        CardPlay {
            id: self.id.clone(),
            file_path: self.file_path.clone(),
            peer_id: self.to.peer.map(i64::from),
            started_at: self.started_at,
            ended_at: None,
            played_ms: self.played_ms as u64,
            duration_ms: (self.duration_ms != 0).then_some(self.duration_ms as u64),
            outcome,
            source: Some(source),
            session_id: None,
            pause_count: self.pause_count as u32,
            track,
        }
    }

    fn json(&self) -> Value {
        let mut server = Map::new();
        if self.to.server.instance != NO_INSTANCE {
            server.insert("instance".into(), hex(&self.to.server.instance).into());
        }
        server.insert("urlKey".into(), id_hex(self.to.server.url_key).into());
        let mut out = json!({
            "id": self.id, "deviceId": hex(&self.device_id), "epoch": id_hex(self.epoch),
            "seq": self.seq,
            "server": server, "filePath": self.file_path, "audioHash": hex(&self.audio_hash),
            "startedAt": self.started_at, "playedMs": self.played_ms, "durationMs": self.duration_ms,
            "outcome": mppl::outcome_name(self.outcome), "source": mppl::source_name(self.source),
            "pauseCount": self.pause_count,
        });
        if let Some(peer) = self.to.peer {
            out["peerId"] = peer.into();
        }
        if let Some(t) = &self.track {
            out["track"] = json!({"title": t.title, "artist": t.artist, "album": t.album,
                                  "durationMs": t.duration_ms, "hash": t.hash});
        }
        out
    }

    fn read(v: &Value) -> Option<Owed> {
        let text = |key: &str| v.get(key).and_then(Value::as_str);
        let number = |key: &str| v.get(key).and_then(Value::as_u64);
        let small = |key: &str| number(key).and_then(|n| u32::try_from(n).ok());
        let server = v.get("server")?;
        let instance = server.get("instance").and_then(Value::as_str);
        let instance = instance.map_or(Some(NO_INSTANCE), hex16)?;
        let url_key = server.get("urlKey").and_then(Value::as_str).and_then(from_hex)?;
        let peer = match v.get("peerId") {
            None => None,
            Some(p) => Some(p.as_u64().and_then(|p| u32::try_from(p).ok())?),
        };
        let code = |key: &str, names: &[&str]| names.iter().position(|n| Some(*n) == text(key));
        let track = match v.get("track") {
            None => None,
            Some(t) => Some(Snapshot {
                title: t.get("title")?.as_str()?.into(),
                artist: t.get("artist")?.as_str()?.into(),
                album: t.get("album")?.as_str()?.into(),
                duration_ms: t.get("durationMs")?.as_u64().and_then(|d| u32::try_from(d).ok())?,
                hash: t.get("hash")?.as_str()?.into(),
            }),
        };
        let owed = Owed {
            id: text("id")?.into(),
            device_id: device_of(text("deviceId")?)?,
            epoch: from_hex(text("epoch")?)?,
            seq: small("seq")?,
            to: To { server: Server::direct(instance, url_key), peer },
            file_path: text("filePath")?.into(),
            audio_hash: hex16(text("audioHash")?)?,
            started_at: number("startedAt")?,
            played_ms: small("playedMs")?,
            duration_ms: small("durationMs")?,
            outcome: code("outcome", &["", "completed", "skipped", "stopped"]).filter(|&c| c > 0)?
                as u8,
            source: code("source", &["other", "manual", "shuffle", "autodj"])? as u8,
            pause_count: number("pauseCount").and_then(|n| u16::try_from(n).ok())?,
            track,
        };
        // An id that isn't its key's is no play this player made.
        let key = (owed.device_id, owed.epoch, owed.seq);
        (key_of_id(&owed.id) == Some(key) && owed.seq >= 1).then_some(owed)
    }
}

/// A play's key from its id (2.20.4), spelled as the player spells it.
pub(crate) fn key_of_id(id: &str) -> Option<mppl::Key> {
    let mut parts = id.strip_prefix("core2-")?.splitn(3, '-');
    let device_id = device_of(parts.next()?)?;
    let epoch = u64::from_str_radix(parts.next().filter(|e| e.len() == 16)?, 16).ok()?;
    let seq = parts.next().filter(|s| s.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()?;
    let slot = Slot { device_id, epoch, seq, ..Slot::default() };
    (slot.id() == id).then_some(slot.key())
}

// ── Resolving (2.20.6) ──────────────────────────────────────────────────────

/// Why a new play isn't sent; it is counted (2.20.6, item 8), and it settles
/// as it is: the mark passes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Unsent {
    /// No ledger row has its path: the listener's own file.
    Own,
    /// Its file isn't its row's (the size, or the time under the skew), or
    /// two rows share its path hash.
    Changed,
    /// Played before the clock was ever set, and no later play of its boot
    /// dates it: no start.
    Clock,
    /// Its row's server can't be told (an SRVS entry that breaks a rule).
    Server,
}

/// The posted mark of a device and epoch among `marks`; 0 for none.
fn mark_of(marks: &[Ack], device_id: [u8; 6], epoch: u64) -> u32 {
    marks.iter().filter(|a| a.key() == (device_id, epoch)).map(|a| a.seq).max().unwrap_or(0)
}

/// The new plays of `log` (above their marks), each mapped through the
/// ledger of `tags` and the card's servers, its stamps held to `skew`: a
/// play to send, or why not (2.20.6, items 4-9).
pub(crate) fn resolve(
    log: &Log,
    tags: &Tags,
    servers: &Servers,
    skew: i32,
    marks: &[Ack],
) -> Vec<(Slot, Result<Owed, Unsent>)> {
    // A row with no server path (an owned file with no server track, 2.6.6)
    // is none: its plays are the listener's, as the reference has them.
    let mut rows: HashMap<u64, Vec<(&Entry, &Ledger)>> = HashMap::new();
    for e in &tags.records {
        if let Some(ledger) = e.ledger.as_ref().filter(|l| !l.server_path.is_empty()) {
            rows.entry(e.hash).or_default().push((e, ledger));
        }
    }
    let dates = mppl::dates(&log.plays);
    let new = log.plays.iter().filter(|p| p.seq > mark_of(marks, p.device_id, p.epoch));
    let mut out = Vec::new();
    for p in new {
        let fate = match rows.get(&p.path_hash).map_or(&[][..], Vec::as_slice) {
            [] => Err(Unsent::Own),
            [(e, _)] if !mppl::same_file(p, e.rec.size, e.rec.fat_time, skew) => {
                Err(Unsent::Changed)
            }
            [(e, ledger)] => match dates.get(&p.key()).copied().flatten() {
                None => Err(Unsent::Clock),
                Some(start) => match To::of(servers, ledger.server as usize) {
                    Some(to) => Ok(Owed::of(p, to, e, ledger, start)),
                    None => Err(Unsent::Server),
                },
            },
            _ => Err(Unsent::Changed),
        };
        out.push((*p, fate));
    }
    out
}

// ── The outbox ──────────────────────────────────────────────────────────────

/// Where the outbox lives: `card-plays.json` in the config folder
/// ([`ConfigStore`]), or a test's.
pub(crate) trait Store {
    /// The file's text; None when there is none.
    fn load(&self) -> Result<Option<String>, String>;
    /// The file written whole, atomically.
    fn save(&self, text: &str) -> Result<(), String>;
}

/// `card-plays.json` beside the player's other files.
pub(crate) struct ConfigStore;

impl Store for ConfigStore {
    fn load(&self) -> Result<Option<String>, String> {
        crate::config::load_card_plays_file()
    }

    fn save(&self, text: &str) -> Result<(), String> {
        crate::config::save_card_plays_file(text)
    }
}

/// The file's schema.
pub(crate) const OUTBOX_VERSION: u64 = 1;

/// One card's part of the outbox.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Owing {
    /// The firmware that keeps the card's log, as its `device.txt` says:
    /// the `client` block's version.
    pub(crate) firmware: String,
    /// The plays the server settled above the card's marks, and those its
    /// schema refuses (never sent): until a sync stores marks past them on
    /// the card, or the card's log holds them no more.
    pub(crate) done: BTreeSet<String>,
    /// The plays waiting to be posted.
    pub(crate) plays: Vec<Owed>,
    /// Entries of `plays` this player can't read, and the members of the
    /// card's object it doesn't know: a newer player's, kept as they are.
    kept: Vec<Value>,
    keep: Map<String, Value>,
}

const OWING_KNOWN: [&str; 3] = ["firmware", "done", "plays"];

impl Owing {
    fn read(v: &Value) -> Owing {
        let Some(object) = v.as_object() else { return Owing::default() };
        let mut owing = Owing {
            firmware: object.get("firmware").and_then(Value::as_str).unwrap_or_default().into(),
            ..Owing::default()
        };
        let done = object.get("done").and_then(Value::as_array).into_iter().flatten();
        let ids = done.filter_map(Value::as_str).filter(|id| key_of_id(id).is_some());
        owing.done = ids.map(Into::into).collect();
        for item in object.get("plays").and_then(Value::as_array).into_iter().flatten() {
            match Owed::read(item) {
                Some(owed) if !owing.plays.iter().any(|o| o.id == owed.id) => owing.plays.push(owed),
                Some(_) => {}
                None => owing.kept.push(item.clone()),
            }
        }
        let unknown = object.iter().filter(|(k, _)| !OWING_KNOWN.contains(&k.as_str()));
        owing.keep = unknown.map(|(k, v)| (k.clone(), v.clone())).collect();
        owing
    }

    fn json(&self) -> Value {
        let mut object = self.keep.clone();
        object.insert("firmware".into(), self.firmware.as_str().into());
        object.insert("done".into(), self.done.iter().cloned().collect::<Vec<_>>().into());
        let plays = self.plays.iter().map(Owed::json).chain(self.kept.iter().cloned());
        object.insert("plays".into(), plays.collect::<Vec<_>>().into());
        Value::Object(object)
    }
}

/// `card-plays.json`: the plays read from cards that wait for their
/// server, and those the server settled that no card's marks pass yet, by
/// card id. A newer player's file is read as none and never written over,
/// and so is one that doesn't read as this player's (not JSON, no version,
/// cards that aren't an object): the plays it holds can't be mapped again
/// once a sync has replaced their ledger. Every member this player doesn't
/// know is kept, a card key that isn't a card id among them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Outbox {
    pub(crate) cards: BTreeMap<u64, Owing>,
    others: Map<String, Value>,
    keep: Map<String, Value>,
    newer: bool,
    damaged: bool,
}

impl Outbox {
    pub(crate) fn parse(text: &str) -> Outbox {
        let damaged = || Outbox { damaged: true, ..Outbox::default() };
        if text.trim().is_empty() {
            return Outbox::default();
        }
        let Ok(Value::Object(top)) = serde_json::from_str::<Value>(text) else { return damaged() };
        match top.get("version").and_then(Value::as_u64) {
            Some(OUTBOX_VERSION) => {}
            Some(v) if v > OUTBOX_VERSION => return Outbox { newer: true, ..Outbox::default() },
            _ => return damaged(),
        }
        let listed = match top.get("cards") {
            None => None,
            Some(Value::Object(listed)) => Some(listed),
            Some(_) => return damaged(),
        };
        let (mut cards, mut others) = (BTreeMap::new(), Map::new());
        for (key, card) in listed.into_iter().flatten() {
            if let Some(id) = from_hex(key) {
                cards.insert(id, Owing::read(card));
            } else {
                others.insert(key.clone(), card.clone());
            }
        }
        let keep = top.iter().filter(|(k, _)| !matches!(k.as_str(), "version" | "cards"));
        let keep = keep.map(|(k, v)| (k.clone(), v.clone())).collect();
        Outbox { cards, others, keep, newer: false, damaged: false }
    }

    pub(crate) fn text(&self) -> String {
        let mut top = self.keep.clone();
        top.insert("version".into(), OUTBOX_VERSION.into());
        let mut cards = self.others.clone();
        cards.extend(self.cards.iter().map(|(id, owing)| (id_hex(*id), owing.json())));
        top.insert("cards".into(), Value::Object(cards));
        serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_default() + "\n"
    }

    pub(crate) fn load(store: &dyn Store) -> Result<Outbox, String> {
        Ok(store.load()?.as_deref().map(Outbox::parse).unwrap_or_default())
    }
}

/// One card's part written into the outbox as it is now on disk: another
/// process's cards stay as it left them.
fn save_card(store: &dyn Store, card_id: u64, owing: &Owing) -> Result<(), String> {
    let mut outbox = Outbox::load(store)?;
    if outbox.newer {
        return Err("card-plays.json is a newer player's".into());
    }
    if outbox.damaged {
        return Err("card-plays.json doesn't read as this player's, and isn't written over".into());
    }
    // A card with nothing waiting or settled past its marks has no part.
    let idle = owing.plays.is_empty() && owing.done.is_empty() && owing.kept.is_empty();
    if idle && owing.keep.is_empty() {
        outbox.cards.remove(&card_id);
    } else {
        outbox.cards.insert(card_id, owing.clone());
    }
    store.save(&outbox.text())
}

// ── A look, a post, the marks ───────────────────────────────────────────────

/// What a look and a post came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// What the reader skipped, and the drop notes' sum: plays lost before
    /// any software read them.
    pub(crate) read: mppl::Counts,
    /// The plays above the posted marks.
    pub(crate) new: usize,
    /// Of them, those to send (mapped now, or already waiting), and those
    /// not sent, by why.
    pub(crate) owed: usize,
    pub(crate) own: usize,
    pub(crate) changed: usize,
    pub(crate) clock: usize,
    pub(crate) server: usize,
    /// Paths the server moved, found again by their audio hash.
    pub(crate) remapped: usize,
    /// The server's answers.
    pub(crate) accepted: usize,
    pub(crate) duplicates: usize,
    pub(crate) rejected: usize,
    /// Plays the server's schema refuses (a path too long, say): never
    /// sent, they settle as the plays not sent do.
    pub(crate) refused: usize,
    /// For the server read now and not posted: a dry run's, or after a post
    /// that failed.
    pub(crate) due: usize,
    /// In the outbox after it all, for any server.
    pub(crate) waiting: usize,
    /// A read of the card or a post failed.
    pub(crate) failed: bool,
    pub(crate) notes: Vec<String>,
}

impl Tally {
    pub(crate) fn json(&self) -> Value {
        let c = &self.read;
        json!({
            "read": {"slots": c.slots, "damaged": c.damaged, "tail": c.tail, "newer": c.newer,
                     "unknownKinds": c.unknown_kinds, "unusable": c.unusable, "twins": c.twins,
                     "conflicts": c.conflicts},
            "dropped": c.dropped,
            "new": self.new,
            "owed": self.owed,
            "unsent": {"own": self.own, "changed": self.changed, "clock": self.clock,
                       "server": self.server},
            "remapped": self.remapped,
            "posted": {"accepted": self.accepted, "duplicates": self.duplicates,
                       "rejected": self.rejected},
            "refused": self.refused,
            "due": self.due,
            "waiting": self.waiting,
            "failed": self.failed,
            "notes": self.notes,
        })
    }
}

/// One card's plays through a run: looked at once, then remapped and
/// posted, its marks given to the run's commits and state file.
pub(crate) struct History {
    store: Rc<dyn Store>,
    /// Read and resolve only: nothing saved, nothing posted.
    dry: bool,
    /// The card's state file's marks, and the card id that file names: the
    /// marks are taken only for that card's root.
    posted: Vec<Ack>,
    posted_for: u64,
    card_id: u64,
    /// The log as the look read it; None until a look read one whole.
    log: Option<Log>,
    /// The card's part of the outbox, taken by a look of a card with a root
    /// (`loaded`); `unread` when the outbox couldn't be read, so this part
    /// is never saved over the one on disk.
    owing: Owing,
    loaded: bool,
    unread: bool,
    /// The plays the outbox on disk holds, as this run knows (those it
    /// read, then those of its last save), and why the last save failed.
    stored: HashSet<String>,
    unsaved: Option<String>,
    /// The new plays not sent, by id: they settle as they are.
    unsent: HashSet<String>,
    pub(crate) tally: Tally,
}

impl History {
    /// `posted`: the marks of the card's state file and the card id it
    /// names, read by the caller (2.12.7) since a session's look comes
    /// before it says what the file holds.
    pub(crate) fn new(store: Rc<dyn Store>, dry: bool, posted: (u64, Vec<Ack>)) -> History {
        History {
            store,
            dry,
            posted: posted.1,
            posted_for: posted.0,
            card_id: 0,
            log: None,
            owing: Owing::default(),
            loaded: false,
            unread: false,
            stored: HashSet::new(),
            unsaved: None,
            unsent: HashSet::new(),
            tally: Tally::default(),
        }
    }

    fn note(&mut self, failed: bool, note: String) {
        self.tally.failed |= failed;
        self.tally.notes.push(note);
    }

    fn save(&mut self) {
        if self.dry || !self.loaded {
            return;
        }
        self.tally.waiting = self.owing.plays.len();
        let saved = match self.unread {
            true => Err("it can't be read, so it isn't written over".to_string()),
            false => save_card(&*self.store, self.card_id, &self.owing),
        };
        match saved {
            Ok(()) => {
                self.stored = self.owing.plays.iter().map(|o| o.id.clone()).collect();
                self.unsaved = None;
            }
            Err(e) => {
                self.note(true, format!("the plays' outbox wasn't saved: {e}"));
                self.unsaved = Some(e);
            }
        }
    }

    /// Why plays to send are held by nothing but this run, when some are:
    /// mapped and not in the outbox on disk, since its save failed. A run
    /// that went on would replace the rows they map through, and the next
    /// look would take them for the listener's files (2.20.6, item 5: the
    /// store comes before any checkpoint). None in a dry run, which writes
    /// nothing to the card.
    pub(crate) fn unstored(&self) -> Option<String> {
        let held = self.owing.plays.iter().filter(|o| !self.stored.contains(&o.id)).count();
        if self.dry || held == 0 {
            return None;
        }
        let why = self.unsaved.as_deref().unwrap_or("not saved");
        Some(format!("{held} of the card's plays to send aren't in the plays' outbox ({why})"))
    }

    /// Reads the card's log and maps its new plays through the root's tags
    /// file, `skew` its listing's (the engine's plays hook). The card's part
    /// of the outbox is taken first, whenever the root names the card: the
    /// plays mapped before go to their server even when the card has no
    /// log, tags file or ledger now to map new ones through. A card with no
    /// root, ledger or log has nothing to map; one that fails a read is left
    /// for the next look.
    pub(crate) fn look(&mut self, card: &dyn CardFs, root: Option<&Root>, skew: i32) {
        if let Some(root) = root {
            self.load(card, root.manifest.header.card_id);
        }
        let files = match read_files(card) {
            Ok(files) if files.is_empty() => return,
            Ok(files) => files,
            Err(e) => {
                self.note(true, format!("the play log can't be read: {e}"));
                return;
            }
        };
        let tags = root.map(|r| r.tags.as_ref());
        let (Some(root), Some(Ok(tags))) = (root, tags) else {
            self.note(false, "the card has no tags file to map its plays through: they wait".into());
            return;
        };
        if !tags.has_ledger {
            self.note(false, "the card has no ledger yet: its plays wait for a sync".into());
            return;
        }
        let named: Vec<LogFile<'_>> =
            files.iter().map(|(name, bytes, size)| LogFile { name, bytes, size: *size }).collect();
        let log = mppl::read_log(&named);
        let mut owing = std::mem::take(&mut self.owing);
        // A settled id the log holds no more goes: its play is never read
        // again, and a seq the card doesn't hold holds no mark back.
        let held: HashSet<String> = log.plays.iter().map(Slot::id).collect();
        owing.done.retain(|id| held.contains(id));

        self.tally.read = log.counts;
        let servers = root.manifest.server_table();
        for (slot, fate) in resolve(&log, tags, &servers, skew, &self.posted) {
            self.tally.new += 1;
            let owed = match fate {
                Ok(owed) => owed,
                Err(why) => {
                    *match why {
                        Unsent::Own => &mut self.tally.own,
                        Unsent::Changed => &mut self.tally.changed,
                        Unsent::Clock => &mut self.tally.clock,
                        Unsent::Server => &mut self.tally.server,
                    } += 1;
                    self.unsent.insert(slot.id());
                    continue;
                }
            };
            self.tally.owed += 1;
            // A play mapped already keeps its first mapping, through the
            // ledger it was played against.
            if !owing.done.contains(&owed.id) && !owing.plays.iter().any(|o| o.id == owed.id) {
                owing.plays.push(owed);
            }
        }
        (self.log, self.owing) = (Some(log), owing);
        self.save();
    }

    /// The card's part of the outbox, less what the card's marks pass, and
    /// the firmware its `device.txt` names now. An outbox that can't be
    /// read gives an empty part, which is never saved: it would take the
    /// place of the part on disk.
    fn load(&mut self, card: &dyn CardFs, card_id: u64) {
        let mut owing = match Outbox::load(&*self.store) {
            Ok(mut outbox) => outbox.cards.remove(&card_id).unwrap_or_default(),
            Err(e) => {
                self.note(true, format!("the plays' outbox can't be read: {e}"));
                self.unread = true;
                Owing::default()
            }
        };
        // A state file of another identity says nothing of this card's log.
        if self.posted_for != card_id {
            self.posted.clear();
        }
        // Settled as the card's marks say (by a run of this computer's, or
        // another's): forgotten.
        let posted = &self.posted;
        let above = |d: [u8; 6], e: u64, s: u32| s > mark_of(posted, d, e);
        owing.plays.retain(|o| above(o.device_id, o.epoch, o.seq));
        owing.done.retain(|id| key_of_id(id).is_some_and(|(d, e, s)| above(d, e, s)));
        let device = card.read("/.player/device.txt", 64 * 1024).ok().flatten();
        let device = DeviceTxt::read(device.as_deref());
        if !device.firmware.is_empty() {
            owing.firmware = device.firmware;
        }
        self.stored = owing.plays.iter().map(|o| o.id.clone()).collect();
        self.tally.waiting = owing.plays.len();
        (self.card_id, self.owing, self.loaded) = (card_id, owing, true);
    }

    /// The plays for the server `talk` reads now whose paths that server
    /// moved since: found again by their audio hash in its whole manifest
    /// (`entries`), when one entry has it. `libraries`: the vpaths it
    /// listed, None for every one.
    pub(crate) fn remap(
        &mut self,
        talk: &Talk,
        entries: &[ManifestEntry],
        libraries: Option<&[String]>,
    ) {
        let listed: HashSet<&str> = entries.iter().map(|e| e.filepath.as_str()).collect();
        let mut by_audio: HashMap<[u8; 16], Vec<&str>> = HashMap::new();
        for e in entries {
            if let Some(hash) = e.audio_hash.as_deref().and_then(hex16) {
                by_audio.entry(hash).or_default().push(&e.filepath);
            }
        }
        let vpath = |p: &str| p.split('/').next().unwrap_or_default().to_string();
        let mut moved = 0;
        let ours = |o: &&mut Owed| o.to.peer.is_none() && o.to.reached(talk);
        for o in self.owing.plays.iter_mut().filter(ours) {
            let looked = libraries.is_none_or(|l| l.contains(&vpath(&o.file_path)));
            if !looked || listed.contains(o.file_path.as_str()) || o.audio_hash == [0; 16] {
                continue;
            }
            if let Some([one]) = by_audio.get(&o.audio_hash).map(Vec::as_slice) {
                o.file_path = one.to_string();
                moved += 1;
            }
        }
        if moved > 0 {
            self.tally.remapped += moved;
            self.save();
        }
    }

    /// The plays waiting for the server `talk` reads.
    pub(crate) fn due(&self, talk: &Talk) -> usize {
        self.owing.plays.iter().filter(|o| o.to.reached(talk)).count()
    }

    /// Posts the plays waiting for the server `client` reaches (`talk`), in
    /// batches of [`PLAYS_PER_CALL`], each answer saved as it comes. `stats`:
    /// the server takes plays (its Stats API); `device`: it takes the
    /// `device` source. A post that fails leaves the rest waiting.
    pub(crate) fn post(&mut self, client: &Client, talk: &Talk, stats: bool, device: bool) {
        let mut due: Vec<String> =
            self.owing.plays.iter().filter(|o| o.to.reached(talk)).map(|o| o.id.clone()).collect();
        self.tally.due = due.len();
        if due.is_empty() || self.dry || !self.loaded {
            return;
        }
        if !stats {
            self.note(false, "the server takes no plays (it has no Stats API): they wait".into());
            return;
        }
        // A play the server's schema refuses would fail each batch it rode
        // in, at every run, and hold its device's mark for good: it isn't
        // sent, and settles as a play not sent does.
        let mut refused = HashSet::new();
        for o in self.owing.plays.iter().filter(|o| o.to.reached(talk)) {
            if let Some(fault) = play_fault(&o.play(false)) {
                let note = format!("a play the server would refuse isn't sent: {fault}");
                self.tally.notes.push(note);
                refused.insert(o.id.clone());
            }
        }
        if !refused.is_empty() {
            self.tally.refused += refused.len();
            due.retain(|id| !refused.contains(id));
            self.owing.plays.retain(|o| !refused.contains(&o.id));
            self.owing.done.extend(refused);
            self.tally.due = due.len();
            self.save();
        }
        // A version longer than the server takes is left out, not the plays.
        let block = PlayClient {
            name: CLIENT.into(),
            version: play_version(&self.owing.firmware),
            instance_id: Some(format!("{:016x}", self.card_id)),
        };
        let mut device = device;
        for ids in due.chunks(PLAYS_PER_CALL) {
            let plays = |device: bool| -> Vec<CardPlay> {
                let owed = |id: &String| self.owing.plays.iter().find(|o| &o.id == id);
                ids.iter().filter_map(owed).map(|o| o.play(device)).collect()
            };
            let mut answer = client.report_card_plays(&block, &plays(device));
            // A server older than the `device` source refuses the batch
            // whole: the rest go as `other`.
            if device && matches!(answer, Err(ApiError::Server { status: 400, .. })) {
                device = false;
                answer = client.report_card_plays(&block, &plays(false));
            }
            let answer = match answer {
                Ok(answer) => answer,
                Err(e) => {
                    self.note(true, format!("the plays weren't posted: {e}"));
                    break;
                }
            };
            let asked: HashSet<&String> = ids.iter().collect();
            let mut settled: HashSet<String> = HashSet::new();
            for id in answer.accepted.iter().filter(|id| asked.contains(id)) {
                self.tally.accepted += settled.insert(id.clone()) as usize;
            }
            for id in answer.duplicates.iter().filter(|id| asked.contains(id)) {
                self.tally.duplicates += settled.insert(id.clone()) as usize;
            }
            for r in answer.rejected.iter().filter(|r| asked.contains(&r.id)) {
                self.tally.rejected += settled.insert(r.id.clone()) as usize;
            }
            self.owing.plays.retain(|o| !settled.contains(&o.id));
            self.owing.done.extend(settled);
            self.tally.due = self.due(talk);
            self.save();
        }
    }

    /// The posted marks now: the card's, each device and epoch's risen over
    /// the plays settled or not sent (2.20.6, item 10).
    pub(crate) fn marks(&self) -> Vec<Ack> {
        let Some(log) = &self.log else { return self.posted.clone() };
        let waiting: HashSet<&str> = self.owing.plays.iter().map(|o| o.id.as_str()).collect();
        let done = |p: &Slot| {
            let id = p.id();
            let settled = self.unsent.contains(&id) || self.owing.done.contains(&id);
            settled && !waiting.contains(id.as_str())
        };
        mppl::next_marks(&log.plays, &self.posted, &done)
    }

    /// The ACKS of a commit (2.5.6): each entry of the root before raised to
    /// its mark, never lowered; an entry for each device and epoch the log
    /// holds plays of; never above the highest seq read of the card's log.
    /// A run that read no log carries them as they are.
    pub(crate) fn acks(&self, carried: &[Ack]) -> Vec<Ack> {
        let Some(log) = &self.log else { return carried.to_vec() };
        let mut out = carried.to_vec();
        for mark in self.marks() {
            let read = log.last_seq(mark.device_id, mark.epoch);
            let seq = mark.seq.min(read);
            match out.iter_mut().find(|a| a.key() == mark.key()) {
                Some(a) => a.seq = a.seq.max(seq),
                None if seq > 0 => out.push(Ack { seq, ..mark }),
                None => {}
            }
        }
        out
    }
}

impl Hooks for History {
    fn plays(&mut self, card: &dyn CardFs, root: Option<&Root>, skew: i32) {
        self.look(card, root, skew);
    }
}

/// The log's files on the card ([`NAMES`], in any case): each one's name as
/// a reader names it, its bytes up to the cap, and its whole length. A file
/// that reads short has changed or the card was pulled: an error, so no
/// play is passed over.
fn read_files(card: &dyn CardFs) -> io::Result<Vec<(&'static str, Vec<u8>, u64)>> {
    let items = match card.list("/.player") {
        Ok(items) => items,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for item in items.iter().filter(|i| !i.dir) {
        let Some(name) = NAMES.iter().find(|n| n.eq_ignore_ascii_case(&item.name)) else {
            continue;
        };
        let want = item.size.min(FILE_BYTES as u64) as usize;
        let bytes = card.read_at(&format!("/.player/{}", item.name), 0, want)?;
        if bytes.len() != want {
            let short = "a play log shorter than listed";
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, short));
        }
        out.push((*name, bytes, item.size));
    }
    Ok(out)
}

/// The skew (2.3.4) of a tags file's rows against the card as it lists now,
/// read only: a run's survey finds the same over its own listing.
pub(crate) fn listed_skew(card: &dyn CardFs, tags: &Tags) -> io::Result<i32> {
    let mut folders: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in tags.records.iter().filter(|e| e.ledger.is_some()) {
        let folder = e.path.rsplit_once('/').map_or("", |(dir, _)| dir);
        folders.entry(folder).or_default().push(e);
    }
    let mut pairs = Vec::new();
    for (folder, rows) in folders {
        let dir = match folder {
            "" => "/music".to_string(),
            _ => format!("/music/{folder}"),
        };
        let items = match card.list(&dir) {
            Ok(items) => items,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for e in rows {
            let name = e.path.rsplit('/').next().unwrap_or_default();
            let files = || items.iter().filter(|i| !i.dir);
            let item = files().find(|i| i.name == name).or_else(|| {
                let mut same = files().filter(|i| match_key(&i.name) == match_key(name));
                match (same.next(), same.next()) {
                    (Some(one), None) => Some(one),
                    _ => None,
                }
            });
            if let Some(item) = item.filter(|i| i.size == e.rec.size as u64) {
                pairs.push((e.rec.fat_time, item.fat_time));
            }
        }
    }
    Ok(skew(pairs))
}

// ── Without a sync: the hidden command ──────────────────────────────────────

/// A card's plays read, mapped and posted without a sync, the card only
/// read: it writes nothing to it, so no ACKS (they ride on a sync's commit)
/// and no marks in its state file (the outbox keeps what the server
/// settled until a sync stores the marks). With `dry_run`, nothing is saved
/// or posted either. What it came to, and the marks after it.
pub(crate) fn plays_of(
    card: &dyn CardFs,
    client: &Client,
    dry_run: bool,
    store: Rc<dyn Store>,
) -> (Tally, Vec<Ack>) {
    let mut history = History::new(store, dry_run, (0, Vec::new()));
    let seen = match volume::inspect(card) {
        Ok(seen) => seen,
        Err(e) => {
            history.note(true, format!("the card can't be read: {e}"));
            return (history.tally, Vec::new());
        }
    };
    let Some(root) = seen.root.as_ref() else {
        history.note(false, "the card has no commit: its plays wait for a sync".into());
        return (history.tally, Vec::new());
    };
    let state = State::of(&seen.state).filter(|s| s.card_id == root.manifest.header.card_id);
    if let Some(state) = &state {
        (history.posted, history.posted_for) = (state.plays.clone(), state.card_id);
    }
    let skew = match root.tags.as_ref().map(|tags| listed_skew(card, tags)) {
        Ok(Err(e)) => {
            history.note(true, format!("the card can't be listed: {e}"));
            return (history.tally.clone(), history.marks());
        }
        Ok(Ok(skew)) => skew,
        Err(_) => 0,
    };
    history.look(card, Some(root), skew);
    let url_key = server_url_key(&client.server());
    let confirmed = state.as_ref().map_or_else(Vec::new, |s| s.settings.keys(url_key));
    let talk = Talk { url_key, instance: NO_INSTANCE, confirmed, peer: None };
    if dry_run || history.due(&talk) == 0 {
        history.tally.due = history.due(&talk);
        return (history.tally.clone(), history.marks());
    }
    let info = match wait(client.layered_info_async()) {
        Ok(info) => info,
        Err(e) => {
            history.note(true, format!("the server: {e}"));
            return (history.tally.clone(), history.marks());
        }
    };
    // The paths it moved since, found again before any play goes: a play
    // of a path it doesn't know is rejected, and settles lost.
    if info.features.sync {
        match client.manifest(&[], MANIFEST_PAGE_MAX, None, &mut |_| {}) {
            Ok(Manifest::Listed(listing)) => history.remap(&talk, &listing.entries, None),
            Ok(Manifest::NotModified) => {}
            Err(e) => {
                history.note(true, format!("the server's listing: {e}"));
                return (history.tally.clone(), history.marks());
            }
        }
    }
    history.post(client, &talk, info.features.stats.is_some(), info.instance_id.is_some());
    (history.tally.clone(), history.marks())
}

/// `device card-plays <volume> [--dry-run]`, undocumented: the plays of the
/// card at `volume` (where it is mounted, or the OS's name for it), which
/// must be one the volume layer accepts, read, mapped through its ledger
/// and posted to the server of the terminal's saved session, without a
/// sync. It only reads the card: the plays it settles are acknowledged on
/// the card (ACKS, the state file's marks) by the next sync's commit, never
/// by it. With `dry_run` it maps them and says what it would post, and
/// saves and posts nothing. Prints what it found and did as JSON. Exit 0;
/// 1 when the card or the server failed; 2 when the volume isn't one it may
/// read, the volumes can't be listed, or there is no saved session.
pub(crate) fn command(
    volume: &str,
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
    let mut saved = || Client::resolve(None, None).map_err(|e| e.to_string());
    on(&found, volume, dry_run, &mut saved, Rc::new(ConfigStore), out, err)
}

/// [`command`] on the volumes found; `saved` the saved session's client,
/// asked for only once the volume is one it may read.
pub(crate) fn on(
    found: &[volume::Found],
    named: &str,
    dry_run: bool,
    saved: &mut dyn FnMut() -> Result<Client, String>,
    store: Rc<dyn Store>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(found) = super::setup::pick(found, named) else {
        let _ = writeln!(err, "mstream-player: {named} isn't a volume this computer lists");
        return 2;
    };
    let Some(card) = Mounted::of(found) else {
        let codes: Vec<&str> = found.refusals.iter().map(volume::Refusal::code).collect();
        let _ = writeln!(err, "mstream-player: {named} is refused: {}", codes.join(", "));
        return 2;
    };
    let client = match saved() {
        Ok(client) => client,
        Err(e) => {
            let _ = writeln!(err, "mstream-player: no saved session: {e}");
            return 2;
        }
    };
    let (tally, marks) = plays_of(&card, &client, dry_run, store);
    let mut printed = tally.json();
    printed["dryRun"] = dry_run.into();
    printed["marks"] = marks_value(&marks);
    let acks = "written only by a sync's commit: this command writes nothing to the card";
    printed["acks"] = acks.into();
    let _ = writeln!(out, "{}", serde_json::to_string_pretty(&printed).unwrap_or_default());
    if tally.failed { 1 } else { 0 }
}

#[cfg(test)]
pub(crate) mod tests;
