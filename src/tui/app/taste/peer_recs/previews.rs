//! The Peer recs room's previews: one lead's 30-second clip at a time
//! (peer-recommendations plan §2 items 13–15 and §5.5; peer-recs contract
//! §1.8).
//!
//! A slot click, `l`, the sheet's Preview row or `n` presses a lead; `L`
//! plays the shown tab's leads from the top (Preview all). A press asks the
//! room's one preview plug-in about the lead (its raw row, sent as it came)
//! only when nothing answered for it this visit, one resolve in flight: a
//! press made meanwhile waits, a newer one taking its place, and goes 300 ms
//! after the flight ends. A 429 waits 5 s (60 s for iTunes) and is asked
//! once more, then is "No preview" for that press only; `preview: null` is
//! "No preview" for the rest of the visit. Every 200 is kept for the visit,
//! keyed by lead key and plug-in, at most 200 leads.
//!
//! The clip itself plays on the engine's aside sink (`AudioCmd::AsidePlay`,
//! its URL verbatim), which pauses main once the clip has opened and gives
//! it back when the session ends. Every main transport command ends the
//! clip first, as `Taken`, and nothing is restored for it. A clip that will
//! not play (`Failed`) is "No preview" for that press, and its answer is
//! forgotten while the visit still holds that URL, so the next press asks
//! again; a clip the engine only refused (`Refused`: no device) keeps it.
//!
//! The session holds its lead by key, with its own copy of the list a chain
//! walks, as the phone's `_liveKey` and `_list` do: a slot matches the live
//! clip by the key of the lead drawn there, so a poll or a tab that moves
//! the rows never moves the clip (peer-recs contract §1.8.2).
//!
//! The session is a machine over [`PreviewState`] that reads the room
//! through a `View` and answers with `Ask`s; the `App` methods at the
//! end of this file read the view from the room, run it, and turn its asks
//! into effects. An answer arrives with no clock, so a wait it starts is
//! stamped by the next call that has one (the room's tick runs every frame).
//!
//! Ports the phone's `peer_recs_previews.dart` and `preview_answers.dart`
//! and the webapp's `vp.js` `clip*`, including c718b2e8's forget rule.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::{rules, Reprobe};
use crate::api::peer_recs_door::PeerRecsCmd;
use crate::api::types::{PluginPreview, TasteLead};
use crate::api::PluginFailure;
use crate::clock::Instant;
use crate::player::{AsideEnd, AsideEnded, AsidePhase, AsideStatus, AsideThen};
use crate::tui::app::{App, Effect};
use crate::tui::worker::{ApiCmd, AudioCmd};

/// The visit keeps answers for at most this many leads; past it, the lead
/// first answered goes first (the webapp's `CLIP_RESULTS_MAX`).
pub const STORE_CAP: usize = 200;

/// A press queued behind the resolve in flight goes this long after it
/// ends (the webapp's `CLIP_QUEUE_MS`; peer-recs contract §1.8.4).
pub const QUEUE_DELAY: Duration = Duration::from_millis(300);

/// Preview all asks the next lead ahead once the live clip is this many
/// seconds in (peer-recs contract §1.8.6).
pub const PREFETCH_AT: f64 = 20.0;

/// How long the engine holds main paused while the next clip is found: a
/// press that switches from a live clip with no address known yet, and a
/// chain's step (peer-recs contract §1.8.8). Every resolve such a session
/// sends renews it, so it runs from the send, and it outlasts the client's
/// 20 s request timeout: a slow resolve that still answers finds main
/// still held. A 429's wait adds its own length (iTunes' 60 s and this
/// stay under the engine's 90 s most).
pub const HOLD_SPAN: Duration = Duration::from_secs(25);

/// A clip's length when neither the engine nor the catalogue knows it.
pub const CLIP_SECONDS: f64 = 30.0;

/// How many sent resolves and played clips are remembered, so a late
/// answer is still kept and a late failure still forgets its address.
const RECENT: usize = 8;

/// A lead's play slot, as the grid and the sheet's Preview row draw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Pressing previews it.
    Idle,
    /// The plug-in found no preview for it this visit: drawn dim, inert.
    Muted,
    /// Its clip is asked for, waits out a 429, or is opening.
    Busy,
    Playing,
    Paused,
}

/// What the band draws while a clip session lives (peer-recs contract
/// §1.8). It holds everything the band needs, so the band still draws when
/// the clip's lead has left the shown list.
#[derive(Debug, Clone, PartialEq)]
pub struct BandFacts {
    /// The live lead's place in the shown list; `None` once it has left it.
    pub index: Option<usize>,
    /// `Busy`, `Playing` or `Paused`.
    pub slot: Slot,
    pub title: String,
    pub artist: Option<String>,
    /// Seconds into the clip; 0 until it sounds.
    pub position: f64,
    /// The clip's length in seconds; 30 when nobody knows it.
    pub duration: f64,
    /// The catalogue's words, verbatim; `None` before the resolve answered.
    pub attribution: Option<String>,
    /// The store page the attribution opens, https only.
    pub link: Option<String>,
}

/// A note the room words at the bar's note, once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewNote {
    /// A single press found no clip.
    NoPreview,
}

/// What the session reads of the room on every call; never kept.
#[derive(Debug, Clone, Copy)]
struct View<'a> {
    /// The gate's preview plug-in (the first listed with `preview`).
    provider: Option<&'a str>,
    /// The engine opened an output.
    audio: bool,
    /// The shown tab's leads, in rank order.
    leads: &'a [TasteLead],
    /// The room's visit fence.
    visit: u64,
    /// The session can be asked anything.
    connected: bool,
}

impl<'a> View<'a> {
    /// The provider, when previews are offered at all.
    fn offered(&self) -> Option<&'a str> {
        self.provider.filter(|_| self.audio)
    }
}

/// What a call asks for. The `App` turns each into its effect.
#[derive(Debug, Clone, PartialEq)]
enum Ask {
    /// `AudioCmd::AsidePlay`: the clip's address, verbatim.
    Play { ticket: u64, url: String, then: AsideThen },
    /// `AudioCmd::AsidePause`.
    Pause { ticket: u64 },
    /// `AudioCmd::AsideResume`.
    Resume { ticket: u64 },
    /// `AudioCmd::AsideStop { ticket: None, restore }`: the session ends.
    Stop { restore: bool },
    /// `AudioCmd::AsideHold`: main stays paused while the next clip is found.
    Hold { span: Duration },
    /// `PeerRecsCmd::Preview`: the lead's raw row, as the taste route sent it.
    Resolve { ticket: u64, plugin: String, rec: Value },
    /// R7's re-ping, from the clip: a resolve 403 cleared the store.
    Reprobe,
    /// A resolve 404: the plug-in gate is read again.
    Regate,
}

/// Tickets name a resolve and the clip it plays. They are unique for the
/// process, so an engine event or an answer from an earlier session or
/// visit can never match a later one, whoever reset the state between.
static TICKETS: AtomicU64 = AtomicU64::new(0);

fn ticket() -> u64 {
    TICKETS.fetch_add(1, Ordering::Relaxed) + 1
}

/// When a wait ends. An answer arrives with no clock, so its wait is kept
/// as a length until a call that has one stamps it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Due {
    After(Duration),
    At(Instant),
}

impl Due {
    fn stamp(&mut self, now: Instant) {
        if let Due::After(span) = *self {
            *self = Due::At(now + span);
        }
    }

    fn passed(self, now: Instant) -> bool {
        matches!(self, Due::At(at) if now >= at)
    }
}

/// A lead as the session holds it: its key, its words and the row a
/// resolve sends.
#[derive(Debug, Clone)]
struct Lead {
    key: String,
    title: String,
    artist: Option<String>,
    raw: Value,
}

impl Lead {
    fn of(lead: &TasteLead) -> Lead {
        let trimmed = |s: &Option<String>| {
            let s = s.as_deref().map(|s| rules::peer_text(s).trim());
            s.filter(|s| !s.is_empty()).map(str::to_string)
        };
        let (title, artist) = match (trimmed(&lead.title), trimmed(&lead.artist)) {
            (Some(title), artist) => (title, artist),
            (None, artist) => (artist.unwrap_or_default(), None),
        };
        Lead { key: rules::lead_key(lead), title, artist, raw: lead.raw.clone() }
    }
}

fn leads_of(leads: &[TasteLead]) -> Vec<Lead> {
    leads.iter().map(Lead::of).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Its resolve is out or queued.
    Resolving,
    /// A 429's wait before the one retry.
    Waiting,
    /// The engine is opening the clip.
    Loading,
    Playing,
    Paused,
}

/// Why a press found no clip: none (kept for the visit) or failed (this
/// press only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Miss {
    None,
    Failed,
}

/// The one clip session.
#[derive(Debug, Clone)]
struct Session {
    /// The resolve's and the clip's name; a new one for every lead.
    ticket: u64,
    /// The plug-in the session asks.
    provider: String,
    lead: Lead,
    /// Its own copy of the list it walks (`n`, Preview all).
    list: Vec<Lead>,
    /// The lead's place in `list`.
    index: usize,
    /// Preview all's.
    chain: bool,
    status: Status,
    /// Resolves sent for this lead: a 429 is asked once more.
    attempt: u8,
    /// A 429's wait.
    wait: Option<Due>,
    /// The clip's answer, once the resolve found it.
    preview: Option<PluginPreview>,
    /// What its clip's run-out does.
    then: AsideThen,
    /// The engine may hold a clip session for it (its own clip, or one it
    /// took over), so a wait renews the hold and an end stops it.
    engaged: bool,
    position: f64,
    duration: f64,
    /// Preview all asked the next lead ahead during this clip.
    prefetched: bool,
}

impl Session {
    fn new(provider: String, list: Vec<Lead>, index: usize, chain: bool, engaged: bool) -> Session {
        Session {
            ticket: ticket(),
            provider,
            lead: list[index].clone(),
            list,
            index,
            chain,
            status: Status::Resolving,
            attempt: 0,
            wait: None,
            preview: None,
            then: AsideThen::Restore,
            engaged,
            position: 0.0,
            duration: 0.0,
            prefetched: false,
        }
    }

    fn slot(&self) -> Slot {
        match self.status {
            Status::Resolving | Status::Waiting | Status::Loading => Slot::Busy,
            Status::Playing => Slot::Playing,
            Status::Paused => Slot::Paused,
        }
    }
}

/// A resolve sent: what its answer is kept under.
#[derive(Debug, Clone)]
struct Flight {
    ticket: u64,
    key: String,
    plugin: String,
    /// Preview all's look ahead: stored, never played, never retried.
    prefetch: bool,
}

/// A clip sent to the engine: what its failure forgets.
#[derive(Debug, Clone)]
struct Played {
    ticket: u64,
    key: String,
    plugin: String,
    url: String,
}

/// The visit's answers (the phone's `PreviewAnswers`): lead key → plug-in
/// → the clip found, or none. Busy and failed are never stored.
#[derive(Debug, Clone, Default)]
struct Store {
    leads: BTreeMap<String, Kept>,
    stamp: u64,
}

#[derive(Debug, Clone)]
struct Kept {
    /// When the lead was first answered: the oldest goes first.
    first: u64,
    by: BTreeMap<String, Option<PluginPreview>>,
}

impl Store {
    fn of(&self, key: &str, plugin: &str) -> Option<&Option<PluginPreview>> {
        self.leads.get(key)?.by.get(plugin)
    }

    fn none(&self, key: &str, plugin: &str) -> bool {
        matches!(self.of(key, plugin), Some(None))
    }

    fn put(&mut self, key: &str, plugin: &str, answer: Option<PluginPreview>) {
        if let Some(kept) = self.leads.get_mut(key) {
            // A lead answered again keeps its place (webapp clipRemember).
            kept.by.insert(plugin.to_string(), answer);
            return;
        }
        while self.leads.len() >= STORE_CAP {
            let oldest = self.leads.iter().min_by_key(|(_, kept)| kept.first);
            let oldest = oldest.map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            self.leads.remove(&oldest);
        }
        self.stamp += 1;
        let by = BTreeMap::from([(plugin.to_string(), answer)]);
        self.leads.insert(key.to_string(), Kept { first: self.stamp, by });
    }

    /// A clip at `url` would not play: its answer goes, but only while the
    /// store still holds that address (one stored since stays).
    fn forget(&mut self, key: &str, plugin: &str, url: &str) -> bool {
        let Some(kept) = self.leads.get_mut(key) else {
            return false;
        };
        if !matches!(kept.by.get(plugin), Some(Some(found)) if found.url == url) {
            return false;
        }
        kept.by.remove(plugin);
        if kept.by.is_empty() {
            self.leads.remove(key);
        }
        true
    }

    fn clear(&mut self) {
        self.leads.clear();
    }
}

/// The visit's previews: the answers store, the clip session, the resolve
/// in flight and the press queued behind it.
#[derive(Debug, Clone, Default)]
pub struct PreviewState {
    /// The visit these belong to; another one forgets them.
    visit: u64,
    store: Store,
    /// The provider the gate last named, so a change clears the store.
    provider: Option<String>,
    session: Option<Session>,
    /// The resolve in flight; another waits for it.
    flight: Option<u64>,
    /// Recent resolves, so a late answer is still kept.
    asked: VecDeque<Flight>,
    /// The press waiting for the flight, by ticket; a newer one replaces it.
    queued: Option<u64>,
    /// When the queued press may go: `QUEUE_DELAY` after the flight.
    queue_due: Option<Due>,
    /// Recent clips, so a late failure still forgets its address.
    played: VecDeque<Played>,
    note: Option<PreviewNote>,
    /// A reset forgot a live session; its stop goes out on the next tick.
    owed_stop: bool,
}

impl PreviewState {
    /// A clip session runs: a clip is asked for, waited on, opening,
    /// playing or paused. The room holds its re-asks while it does (R4).
    pub fn live(&self) -> bool {
        self.session.is_some()
    }

    /// An unrelated error cleared the room's waits (`clear_peer_recs_waits`):
    /// the resolve in flight no longer holds the next one back. Its answer
    /// still lands when its ticket is the session's.
    pub(crate) fn clear_waits(&mut self) {
        if self.flight.take().is_some() {
            self.arm_queue();
        }
    }

    // ── the room's reads ────────────────────────────────────────────────

    fn slot(&self, view: View<'_>, index: usize) -> Slot {
        let (Some(provider), Some(lead)) = (view.offered(), view.leads.get(index)) else {
            return Slot::Idle;
        };
        if self.visit != view.visit {
            return Slot::Idle;
        }
        let key = rules::lead_key(lead);
        if let Some(session) = self.session.as_ref().filter(|s| s.lead.key == key) {
            return session.slot();
        }
        if self.store.none(&key, provider) { Slot::Muted } else { Slot::Idle }
    }

    fn band(&self, view: View<'_>) -> Option<BandFacts> {
        // Without the offer there is no band (peer-recs contract §1.8.1).
        if self.visit != view.visit || view.offered().is_none() {
            return None;
        }
        let session = self.session.as_ref()?;
        let found = session.preview.as_ref();
        let duration = if session.duration > 0.0 {
            session.duration
        } else {
            found.map(|p| f64::from(p.seconds)).filter(|s| *s > 0.0).unwrap_or(CLIP_SECONDS)
        };
        let https = |l: &String| l.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://"));
        Some(BandFacts {
            index: view.leads.iter().position(|l| rules::lead_key(l) == session.lead.key),
            slot: session.slot(),
            title: session.lead.title.clone(),
            artist: session.lead.artist.clone(),
            position: session.position,
            duration,
            attribution: found.and_then(|p| p.attribution.clone()),
            link: found.and_then(|p| p.link.clone()).filter(https),
        })
    }

    // ── presses ─────────────────────────────────────────────────────────

    fn press(&mut self, view: View<'_>, index: usize, now: Instant) -> Vec<Ask> {
        self.fence(view.visit);
        self.stamp(now);
        let (Some(provider), Some(lead)) = (view.offered(), view.leads.get(index)) else {
            return Vec::new();
        };
        let key = rules::lead_key(lead);
        if self.session.as_ref().is_some_and(|s| s.lead.key == key) {
            return self.toggle();
        }
        if self.store.none(&key, provider) {
            // The press is noted, and nothing else moves (peer-recs
            // contract §1.8.3).
            self.note = Some(PreviewNote::NoPreview);
            return Vec::new();
        }
        let engaged = self.halt();
        self.begin(view, provider.to_string(), leads_of(view.leads), index, false, engaged)
    }

    fn preview_all(&mut self, view: View<'_>, now: Instant) -> Vec<Ask> {
        self.fence(view.visit);
        self.stamp(now);
        if self.session.as_ref().is_some_and(|s| s.chain) {
            return self.end(true);
        }
        let Some(provider) = view.offered() else {
            return Vec::new();
        };
        let list = leads_of(view.leads);
        let store = &self.store;
        let skip = |i: usize| store.none(&list[i].key, provider);
        let Some(first) = rules::next_index(list.len(), None, skip) else {
            return Vec::new();
        };
        let engaged = self.halt();
        self.begin(view, provider.to_string(), list, first, true, engaged)
    }

    fn toggle(&mut self) -> Vec<Ask> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        // The status moves only once the engine says it did (webapp clipPause).
        match session.status {
            Status::Playing => vec![Ask::Pause { ticket: session.ticket }],
            Status::Paused => vec![Ask::Resume { ticket: session.ticket }],
            Status::Resolving | Status::Waiting | Status::Loading => self.end(true),
        }
    }

    fn next(&mut self, view: View<'_>, now: Instant) -> Vec<Ask> {
        self.fence(view.visit);
        self.stamp(now);
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let store = &self.store;
        let skip = |i: usize| store.none(&session.list[i].key, &session.provider);
        match rules::next_index(session.list.len(), Some(session.index), skip) {
            // `n` past the last lead ends the clip, as the webapp's does
            // (peer-recs contract §1.8.10).
            None => self.end(true),
            Some(n) => {
                let Some(old) = self.session.take() else {
                    return Vec::new();
                };
                self.queued = None;
                self.begin(view, old.provider, old.list, n, old.chain, old.engaged)
            }
        }
    }

    /// A room-side end: the session goes, and the engine is told.
    fn end(&mut self, restore: bool) -> Vec<Ask> {
        self.queued = None;
        match self.session.take() {
            Some(_) => vec![Ask::Stop { restore }],
            None => Vec::new(),
        }
    }

    /// The session before a new one goes quietly (its queue and its wait
    /// with it): whether the engine may still hold a clip session for it.
    fn halt(&mut self) -> bool {
        self.queued = None;
        self.session.take().is_some_and(|s| s.engaged)
    }

    /// A new session at `list[index]`: it plays the visit's answer, asks for
    /// one, or (in a chain) passes a lead known to have none.
    fn begin(
        &mut self,
        view: View<'_>,
        provider: String,
        list: Vec<Lead>,
        mut index: usize,
        chain: bool,
        engaged: bool,
    ) -> Vec<Ask> {
        loop {
            match self.store.of(&list[index].key, &provider).cloned() {
                Some(Some(found)) => {
                    let mut session = Session::new(provider, list, index, chain, engaged);
                    let play = self.play(&mut session, found);
                    self.session = Some(session);
                    return vec![play];
                }
                Some(None) if chain => {
                    let store = &self.store;
                    let skip = |i: usize| store.none(&list[i].key, &provider);
                    match rules::next_index(list.len(), Some(index), skip) {
                        Some(n) => index = n,
                        None => return self.stopped_by_engine(engaged),
                    }
                }
                Some(None) => {
                    self.note = Some(PreviewNote::NoPreview);
                    return self.stopped_by_engine(engaged);
                }
                None => {
                    self.session = Some(Session::new(provider, list, index, chain, engaged));
                    // The old clip stops at once and main stays paused while
                    // this one is found (peer-recs contract §1.8.8). A resolve
                    // sent now carries its own hold; one queued behind the
                    // flight is held here, and again when it goes.
                    let asks = self.ask(view);
                    let held = asks.iter().any(|ask| matches!(ask, Ask::Hold { .. }));
                    if engaged && !held {
                        return [vec![Ask::Hold { span: HOLD_SPAN }], asks].concat();
                    }
                    return asks;
                }
            }
        }
    }

    /// The stop an internal end owes the engine: only when it may hold a
    /// clip session for the room.
    fn stopped_by_engine(&mut self, engaged: bool) -> Vec<Ask> {
        if engaged { vec![Ask::Stop { restore: true }] } else { Vec::new() }
    }

    /// The session's clip plays `found`.
    fn play(&mut self, session: &mut Session, found: PluginPreview) -> Ask {
        session.status = Status::Loading;
        session.then = if session.chain { AsideThen::Hold } else { AsideThen::Restore };
        session.engaged = true;
        session.position = 0.0;
        session.duration = 0.0;
        session.wait = None;
        let url = found.url.clone();
        remember(
            &mut self.played,
            Played {
                ticket: session.ticket,
                key: session.lead.key.clone(),
                plugin: session.provider.clone(),
                url: url.clone(),
            },
        );
        session.preview = Some(found);
        Ask::Play { ticket: session.ticket, url, then: session.then }
    }

    /// No clip for the session's press (webapp clipSettle): a chain passes
    /// a lead with none and ends on a failed one, so it never runs on into
    /// a spent limiter; a single press ends and says so.
    fn settle(&mut self, view: View<'_>, miss: Miss) -> Vec<Ask> {
        self.queued = None;
        let Some(session) = self.session.take() else {
            return Vec::new();
        };
        if session.chain && miss == Miss::None {
            return self.advance(view, session);
        }
        if !session.chain {
            self.note = Some(PreviewNote::NoPreview);
        }
        self.stopped_by_engine(session.engaged)
    }

    /// A chain's next lead after `session`'s, past those with none; past
    /// the last, the end.
    fn advance(&mut self, view: View<'_>, session: Session) -> Vec<Ask> {
        let store = &self.store;
        let skip = |i: usize| store.none(&session.list[i].key, &session.provider);
        match rules::next_index(session.list.len(), Some(session.index), skip) {
            Some(n) if session.chain => {
                self.begin(view, session.provider, session.list, n, true, session.engaged)
            }
            _ => self.stopped_by_engine(session.engaged),
        }
    }

    // ── resolves ────────────────────────────────────────────────────────

    /// The session's resolve: sent now, or queued behind the one in flight.
    fn ask(&mut self, view: View<'_>) -> Vec<Ask> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        if self.flight.is_some() || self.queue_due.is_some() {
            self.queued = Some(session.ticket);
            return Vec::new();
        }
        self.send(view)
    }

    /// Sends the session's resolve, unless the visit answered it since.
    fn send(&mut self, view: View<'_>) -> Vec<Ask> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        match self.store.of(&session.lead.key, &session.provider).cloned() {
            Some(Some(found)) => {
                let Some(mut session) = self.session.take() else {
                    return Vec::new();
                };
                let play = self.play(&mut session, found);
                self.session = Some(session);
                return vec![play];
            }
            Some(None) => return self.settle(view, Miss::None),
            None => {}
        }
        if !view.connected {
            return self.settle(view, Miss::Failed);
        }
        let ticket = session.ticket;
        let plugin = session.provider.clone();
        let rec = session.lead.raw.clone();
        // Main's hold runs from the send, whenever it goes (behind a flight,
        // after a 429's wait), so it never lapses while the resolve is out
        // (peer-recs contract §1.8.8).
        let mut asks = Vec::new();
        if session.engaged {
            asks.push(Ask::Hold { span: HOLD_SPAN });
        }
        self.flight = Some(ticket);
        let key = session.lead.key.clone();
        let flight = Flight { ticket, key, plugin: plugin.clone(), prefetch: false };
        remember(&mut self.asked, flight);
        asks.push(Ask::Resolve { ticket, plugin, rec });
        asks
    }

    fn answered(
        &mut self,
        view: View<'_>,
        ticket: u64,
        answer: Result<Option<Box<PluginPreview>>, PluginFailure>,
    ) -> Vec<Ask> {
        self.fence(view.visit);
        let Some(at) = self.asked.iter().position(|f| f.ticket == ticket) else {
            return Vec::new();
        };
        let Some(flight) = self.asked.remove(at) else {
            return Vec::new();
        };
        if self.flight == Some(ticket) {
            self.flight = None;
            self.arm_queue();
        }
        // A 200 is kept for the visit even when the press is no longer
        // current (peer-recs contract §1.8.4).
        if let Ok(found) = &answer {
            self.store.put(&flight.key, &flight.plugin, found.as_deref().cloned());
        }
        if flight.prefetch {
            return Vec::new();
        }
        let Some(session) =
            self.session.as_mut().filter(|s| s.ticket == ticket && s.status == Status::Resolving)
        else {
            return Vec::new();
        };
        match answer {
            Ok(Some(found)) => {
                let Some(mut session) = self.session.take() else {
                    return Vec::new();
                };
                let play = self.play(&mut session, *found);
                self.session = Some(session);
                vec![play]
            }
            Ok(None) => self.settle(view, Miss::None),
            Err(PluginFailure::Refused { status: 429, .. }) if session.attempt == 0 => {
                let wait = rules::retry_wait(&session.provider);
                session.status = Status::Waiting;
                session.attempt = 1;
                session.wait = Some(Due::After(wait));
                // Main stays paused through the wait: a 60 s iTunes wait
                // outlives the engine's own hold (peer-recs contract §1.8.8).
                match session.engaged {
                    true => vec![Ask::Hold { span: wait + HOLD_SPAN }],
                    false => Vec::new(),
                }
            }
            Err(PluginFailure::Refused { status: 403, .. }) => {
                // The plug-ins are not there for this account any more:
                // the visit's answers go, and R7 asks whether the room is.
                self.store.clear();
                self.queued = None;
                let engaged = self.session.take().is_some_and(|s| s.engaged);
                let mut asks = self.stopped_by_engine(engaged);
                asks.push(Ask::Reprobe);
                asks
            }
            Err(PluginFailure::Refused { status: 404, .. }) => {
                // The plug-in went: this press fails, and the gate is read
                // again (peer-recs contract §1.8.4).
                let mut asks = self.settle(view, Miss::Failed);
                asks.push(Ask::Regate);
                asks
            }
            Err(_) => self.settle(view, Miss::Failed),
        }
    }

    /// The flight ended: a press waiting for it goes `QUEUE_DELAY` later.
    fn arm_queue(&mut self) {
        if self.queued.is_some() && self.queue_due.is_none() {
            self.queue_due = Some(Due::After(QUEUE_DELAY));
        }
    }

    // ── the engine ──────────────────────────────────────────────────────

    fn status(&mut self, view: View<'_>, status: Option<AsideStatus>) -> Vec<Ask> {
        self.fence(view.visit);
        let Some(status) = status else {
            return Vec::new();
        };
        let Some(session) = self.session.as_mut().filter(|s| s.ticket == status.ticket) else {
            return Vec::new();
        };
        let sounding = matches!(session.status, Status::Loading | Status::Playing | Status::Paused);
        match status.phase {
            AsidePhase::Playing if sounding => session.status = Status::Playing,
            AsidePhase::Paused if sounding => session.status = Status::Paused,
            AsidePhase::Loading | AsidePhase::Held | AsidePhase::Playing | AsidePhase::Paused => {}
        }
        if matches!(session.status, Status::Playing | Status::Paused) {
            session.position = status.position.max(0.0);
            session.duration = status.duration.max(0.0);
        }
        if !(session.chain && session.status == Status::Playing && session.position >= PREFETCH_AT)
            || session.prefetched
        {
            return Vec::new();
        }
        // Preview all's one look ahead (webapp clipPrefetch): never while a
        // resolve is out or queued, never for a lead already answered.
        session.prefetched = true;
        let store = &self.store;
        let skip = |i: usize| store.none(&session.list[i].key, &session.provider);
        let Some(next) = rules::next_index(session.list.len(), Some(session.index), skip) else {
            return Vec::new();
        };
        let lead = &session.list[next];
        let idle = self.flight.is_none() && self.queued.is_none() && self.queue_due.is_none();
        if !view.connected || !idle || self.store.of(&lead.key, &session.provider).is_some() {
            return Vec::new();
        }
        let ticket = ticket();
        let plugin = session.provider.clone();
        let rec = lead.raw.clone();
        let key = lead.key.clone();
        let flight = Flight { ticket, key, plugin: plugin.clone(), prefetch: true };
        self.flight = Some(ticket);
        remember(&mut self.asked, flight);
        vec![Ask::Resolve { ticket, plugin, rec }]
    }

    fn ended(&mut self, view: View<'_>, ended: AsideEnded) -> Vec<Ask> {
        self.fence(view.visit);
        if let AsideEnd::Failed(_) = ended.end
            && let Some(dead) = self.played.iter().find(|p| p.ticket == ended.ticket)
        {
            // A dead address is forgotten wherever it was kept, whoever was
            // pressed since (c718b2e8): the next press asks again.
            let Played { key, plugin, url, .. } = dead.clone();
            self.store.forget(&key, &plugin, &url);
        }
        if !self.session.as_ref().is_some_and(|s| s.ticket == ended.ticket) {
            return Vec::new();
        }
        match ended.end {
            AsideEnd::RanOut => {
                self.queued = None;
                let Some(session) = self.session.take() else {
                    return Vec::new();
                };
                if session.chain {
                    self.advance(view, session)
                } else if session.then == AsideThen::Hold {
                    // Preview all was let go during this clip: main comes
                    // back now rather than after the hold.
                    vec![Ask::Stop { restore: true }]
                } else {
                    Vec::new()
                }
            }
            // Bookkeeping: the room's own stop, or a newer clip.
            AsideEnd::Stopped | AsideEnd::Superseded => Vec::new(),
            // Main always wins: the session and the chain end, and nothing
            // is restored.
            AsideEnd::Taken => {
                self.queued = None;
                self.session = None;
                Vec::new()
            }
            AsideEnd::Failed(_) | AsideEnd::Refused(_) => self.settle(view, Miss::Failed),
        }
    }

    // ── time and lifetime ───────────────────────────────────────────────

    fn tick(&mut self, view: View<'_>, now: Instant) -> Vec<Ask> {
        self.fence(view.visit);
        let mut asks = Vec::new();
        if std::mem::take(&mut self.owed_stop) {
            asks.push(Ask::Stop { restore: true });
        }
        self.stamp(now);
        if let Some(session) = self.session.as_mut()
            && session.status == Status::Waiting
            && session.wait.is_some_and(|w| w.passed(now))
        {
            session.status = Status::Resolving;
            session.wait = None;
            asks.extend(self.ask(view));
        }
        if self.flight.is_none() && self.queue_due.is_some_and(|due| due.passed(now)) {
            self.queue_due = None;
            let queued = self.queued.take();
            let current = self
                .session
                .as_ref()
                .is_some_and(|s| Some(s.ticket) == queued && s.status == Status::Resolving);
            if current {
                asks.extend(self.send(view));
            }
        }
        asks
    }

    fn stamp(&mut self, now: Instant) {
        if let Some(due) = self.queue_due.as_mut() {
            due.stamp(now);
        }
        if let Some(wait) = self.session.as_mut().and_then(|s| s.wait.as_mut()) {
            wait.stamp(now);
        }
    }

    /// Another visit: what was kept is not its own. A live session's stop
    /// is owed.
    fn fence(&mut self, visit: u64) {
        if self.visit == visit {
            return;
        }
        let owed = self.session.is_some() || self.owed_stop;
        let provider = self.provider.take();
        *self = PreviewState { visit, provider, owed_stop: owed, ..PreviewState::default() };
    }

    fn main_command(&mut self) {
        // The engine ends a clip it holds as `Taken`; a press that only
        // resolves or waits is cancelled here, so nothing plays later.
        self.queued = None;
        self.session = None;
    }

    /// The audio thread is gone (`Event::AudioFailed`): it answers no
    /// ticket again, so the session ends here, with no stop sent or owed
    /// (nothing would hear it), and the room's re-asks are not held for it.
    fn audio_lost(&mut self) {
        self.queued = None;
        self.queue_due = None;
        self.session = None;
        self.owed_stop = false;
    }

    fn list_changed(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.chain = false;
        }
    }

    fn provider_changed(&mut self, provider: Option<String>) {
        if self.provider == provider {
            return;
        }
        self.store.clear();
        if provider.is_none() && self.session.is_some() {
            self.queued = None;
            self.session = None;
            self.owed_stop = true;
        }
        self.provider = provider;
    }

    fn reset(&mut self) -> Vec<Ask> {
        let live = self.session.is_some() || self.owed_stop;
        self.shed();
        self.owed_stop = false;
        if live { vec![Ask::Stop { restore: true }] } else { Vec::new() }
    }

    fn shed(&mut self) {
        let owed = self.session.is_some() || self.owed_stop;
        let (visit, provider) = (self.visit, self.provider.take());
        *self = PreviewState { visit, provider, owed_stop: owed, ..PreviewState::default() };
    }
}

/// Keeps the last few, oldest out.
fn remember<T>(ring: &mut VecDeque<T>, item: T) {
    if ring.len() >= RECENT {
        ring.pop_front();
    }
    ring.push_back(item);
}

impl App {
    /// What the clip session reads of the room now.
    fn peer_recs_preview_view(&self) -> View<'_> {
        View {
            provider: self.peer_recs_provider(),
            audio: self.audio_available,
            leads: self.peer_recs_leads(),
            visit: self.peer_recs_visit(),
            connected: self.connected,
        }
    }

    /// Runs the clip session against the room, then sends what it asked.
    /// The state is taken out for the call, so the view can borrow the rest
    /// of the room while the session changes.
    fn peer_recs_previews(
        &mut self,
        run: impl FnOnce(&mut PreviewState, View<'_>) -> Vec<Ask>,
    ) -> Vec<Effect> {
        let mut previews = std::mem::take(&mut self.peer_recs.preview);
        let asks = run(&mut previews, self.peer_recs_preview_view());
        self.peer_recs.preview = previews;
        self.peer_recs_preview_effects(asks)
    }

    fn peer_recs_preview_effects(&mut self, asks: Vec<Ask>) -> Vec<Effect> {
        let generation = self.session_gen();
        let mut effects = Vec::new();
        for ask in asks {
            let audio = match ask {
                Ask::Play { ticket, url, then } => AudioCmd::AsidePlay { ticket, url, then },
                Ask::Pause { ticket } => AudioCmd::AsidePause { ticket },
                Ask::Resume { ticket } => AudioCmd::AsideResume { ticket },
                Ask::Stop { restore } => AudioCmd::AsideStop { ticket: None, restore },
                Ask::Hold { span } => AudioCmd::AsideHold { span },
                Ask::Resolve { ticket, plugin, rec } => {
                    let cmd = PeerRecsCmd::Preview { ticket, plugin, rec: Box::new(rec) };
                    effects.push(Effect::Api(ApiCmd::PeerRecs { generation, cmd }));
                    continue;
                }
                Ask::Reprobe => {
                    effects.extend(self.peer_recs_reprobe(Reprobe::Clip));
                    continue;
                }
                Ask::Regate => {
                    effects.extend(self.peer_recs_regate());
                    continue;
                }
            };
            effects.push(Effect::Audio(audio));
        }
        effects
    }

    /// The room offers previews: an audio device and a preview plug-in.
    pub fn peer_recs_preview_offered(&self) -> bool {
        self.peer_recs_preview_view().offered().is_some()
    }

    /// The slot of the shown list's lead at `index`.
    pub fn peer_recs_slot(&self, index: usize) -> Slot {
        self.peer_recs.preview.slot(self.peer_recs_preview_view(), index)
    }

    /// The band, while a clip session lives.
    pub fn peer_recs_band(&self) -> Option<BandFacts> {
        self.peer_recs.preview.band(self.peer_recs_preview_view())
    }

    /// A clip session lives (any status but idle).
    pub fn peer_recs_clip_live(&self) -> bool {
        self.peer_recs.preview.live()
    }

    /// `l`, a slot click, the sheet's Preview: the shown lead at `index` is
    /// pressed. The live lead toggles; another lead switches to it.
    pub(crate) fn peer_recs_press(&mut self, index: usize, now: Instant) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.press(view, index, now))
    }

    /// `L`: Preview all starts from the top, or stops when it runs.
    pub(crate) fn peer_recs_preview_all(&mut self, now: Instant) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.preview_all(view, now))
    }

    /// Space, the band's toggle: pause, resume, or drop a clip still on its
    /// way.
    pub(crate) fn peer_recs_clip_toggle(&mut self) -> Vec<Effect> {
        self.peer_recs_previews(|p, _| p.toggle())
    }

    /// `n`, `▸▸`: the next lead of the clip's own list; past the last, the
    /// end.
    pub(crate) fn peer_recs_clip_next(&mut self, now: Instant) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.next(view, now))
    }

    /// `x`, `[X]`, Esc, the room not drawn, `w` and `e`: the session ends,
    /// and main comes back when `restore` and the engine may bring it back.
    pub(crate) fn peer_recs_clip_stop(&mut self, restore: bool) -> Vec<Effect> {
        self.peer_recs_previews(|p, _| p.end(restore))
    }

    /// The note a press left, once.
    pub(crate) fn peer_recs_take_note(&mut self) -> Option<PreviewNote> {
        self.peer_recs.preview.note.take()
    }

    /// A resolve's answer, through the room's door.
    pub(crate) fn peer_recs_preview_answered(
        &mut self,
        ticket: u64,
        answer: Result<Option<Box<PluginPreview>>, PluginFailure>,
    ) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.answered(view, ticket, answer))
    }

    /// The engine's clip status (`Event::Aside`).
    pub(crate) fn consume_peer_recs_aside(&mut self, status: Option<AsideStatus>) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.status(view, status))
    }

    /// One clip's end (`Event::AsideEnded`).
    pub(crate) fn consume_peer_recs_aside_ended(&mut self, ended: AsideEnded) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.ended(view, ended))
    }

    /// The previews' turn of the room's tick: a 429's wait, the queued
    /// press, the stop a reset owed.
    pub(crate) fn peer_recs_preview_tick(&mut self, now: Instant) -> Vec<Effect> {
        self.peer_recs_previews(|p, view| p.tick(view, now))
    }

    /// A main transport command went out: a press that only resolves or
    /// waits is cancelled (the engine ends a clip it holds as `Taken`).
    pub(crate) fn peer_recs_main_command(&mut self) {
        self.peer_recs.preview.main_command();
    }

    /// The audio thread is gone: the clip session ends, nothing sent to it.
    pub(crate) fn peer_recs_audio_lost(&mut self) {
        self.peer_recs.preview.audio_lost();
    }

    /// A new answer is drawn: Preview all plays its clip out and stops.
    pub(crate) fn peer_recs_list_changed(&mut self) {
        self.peer_recs.preview.list_changed();
    }

    /// The gate read a provider (or none): another plug-in's answers are
    /// not this one's, so the store is cleared. With none, a live clip ends
    /// and its stop is owed to the next tick.
    pub(crate) fn peer_recs_provider_changed(&mut self, provider: Option<String>) {
        self.peer_recs.preview.provider_changed(provider);
    }

    /// The visit ends: a live clip stops (main restored) and everything is
    /// forgotten.
    pub(crate) fn peer_recs_preview_reset(&mut self) -> Vec<Effect> {
        self.peer_recs_previews(|p, _| p.reset())
    }

    /// The session moved (`reset_peer_recs`, which returns no effects):
    /// everything is forgotten, and a live clip's stop is owed to the next
    /// [`App::peer_recs_preview_tick`].
    pub(crate) fn peer_recs_preview_shed(&mut self) {
        self.peer_recs.preview.shed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::fixtures;

    /// The stage's All answer: 2 Signal Fires, 3 Harbor Lights, 5 Quiet
    /// Engine, 7 Arcade Rain, 11 Field Notes.
    fn leads(name: &str) -> Vec<TasteLead> {
        fixtures::taste(name).network.map(|n| n.results).unwrap_or_default()
    }

    fn clip(url: &str) -> PluginPreview {
        PluginPreview {
            provider: "deezer".into(),
            url: url.into(),
            seconds: 30,
            title: None,
            artist: None,
            album: None,
            attribution: Some("Preview from Deezer".into()),
            link: Some("https://www.deezer.com/track/3".into()),
            match_score: None,
        }
    }

    fn refused(status: u16) -> PluginFailure {
        PluginFailure::Refused { status, words: String::new() }
    }

    /// One visit's previews over a room the test controls.
    struct Rig {
        p: PreviewState,
        leads: Vec<TasteLead>,
        provider: Option<String>,
        audio: bool,
        visit: u64,
        connected: bool,
        now: Instant,
    }

    macro_rules! view {
        ($rig:expr) => {
            View {
                provider: $rig.provider.as_deref(),
                audio: $rig.audio,
                leads: &$rig.leads,
                visit: $rig.visit,
                connected: $rig.connected,
            }
        };
    }

    impl Rig {
        fn new() -> Rig {
            let mut p = PreviewState::default();
            p.provider_changed(Some("deezer".into()));
            Rig {
                p,
                leads: leads("taste_all"),
                provider: Some("deezer".into()),
                audio: true,
                visit: 1,
                connected: true,
                now: Instant::now(),
            }
        }
        fn press(&mut self, index: usize) -> Vec<Ask> {
            self.p.press(view!(self), index, self.now)
        }
        fn all(&mut self) -> Vec<Ask> {
            self.p.preview_all(view!(self), self.now)
        }
        fn next(&mut self) -> Vec<Ask> {
            self.p.next(view!(self), self.now)
        }
        fn answer(
            &mut self,
            ticket: u64,
            answer: Result<Option<PluginPreview>, PluginFailure>,
        ) -> Vec<Ask> {
            self.p.answered(view!(self), ticket, answer.map(|a| a.map(Box::new)))
        }
        fn found(&mut self, ticket: u64, url: &str) -> Vec<Ask> {
            self.answer(ticket, Ok(Some(clip(url))))
        }
        fn status(&mut self, ticket: u64, phase: AsidePhase, position: f64) -> Vec<Ask> {
            let status = AsideStatus { ticket, phase, position, duration: 0.0 };
            self.p.status(view!(self), Some(status))
        }
        fn ended(&mut self, ticket: u64, end: AsideEnd) -> Vec<Ask> {
            self.p.ended(view!(self), AsideEnded { ticket, end })
        }
        fn tick(&mut self, by: Duration) -> Vec<Ask> {
            self.now += by;
            self.p.tick(view!(self), self.now)
        }
        fn slot(&self, index: usize) -> Slot {
            self.p.slot(view!(self), index)
        }
        fn band(&self) -> Option<BandFacts> {
            self.p.band(view!(self))
        }
        fn ticket(&self) -> u64 {
            self.p.session.as_ref().map(|s| s.ticket).expect("a live session")
        }
        /// A press on `index` answered with `url` and sounding.
        fn playing(&mut self, index: usize, url: &str) -> u64 {
            let asks = self.press(index);
            let ticket = match asks.as_slice() {
                [Ask::Resolve { ticket, .. }] => *ticket,
                [Ask::Play { ticket, .. }] => *ticket,
                other => panic!("a press asks or plays, not {other:?}"),
            };
            if matches!(asks.as_slice(), [Ask::Resolve { .. }]) {
                self.found(ticket, url);
            }
            self.status(ticket, AsidePhase::Playing, 1.0);
            assert_eq!(self.slot(index), Slot::Playing);
            ticket
        }
    }

    fn resolve(asks: &[Ask]) -> (u64, &str, &Value) {
        match asks {
            [.., Ask::Resolve { ticket, plugin, rec }] => (*ticket, plugin.as_str(), rec),
            other => panic!("expected a resolve last, got {other:?}"),
        }
    }

    fn play(asks: &[Ask]) -> (u64, &str, AsideThen) {
        match asks {
            [.., Ask::Play { ticket, url, then }] => (*ticket, url.as_str(), *then),
            other => panic!("expected a play last, got {other:?}"),
        }
    }

    const STOP: Ask = Ask::Stop { restore: true };

    // ── single presses ──────────────────────────────────────────────────

    #[test]
    fn a_press_resolves_the_raw_lead_and_plays_the_url_verbatim() {
        let mut rig = Rig::new();
        let asks = rig.press(2);
        let (ticket, plugin, rec) = resolve(&asks);
        assert_eq!(asks.len(), 1, "one resolve and nothing else: {asks:?}");
        assert_eq!(plugin, "deezer");
        assert_eq!(rec, &rig.leads[2].raw, "the lead goes as the taste route sent it");
        assert_eq!(rig.slot(2), Slot::Busy);
        // The address goes to the engine exactly as it came.
        let url = "http://127.0.0.1:3996/preview/3.mp3?hdnea=exp=1~acl=/*~hmac=ab%2Fcd";
        let asks = rig.found(ticket, url);
        assert_eq!(asks, vec![Ask::Play { ticket, url: url.into(), then: AsideThen::Restore }]);
        assert_eq!(rig.slot(2), Slot::Busy, "opening reads busy");
        rig.status(ticket, AsidePhase::Playing, 0.5);
        assert_eq!(rig.slot(2), Slot::Playing);
        assert_eq!(rig.slot(3), Slot::Idle);
    }

    #[test]
    fn a_stored_answer_plays_with_no_resolve() {
        let mut rig = Rig::new();
        let first = rig.playing(2, "http://cdn/3.mp3");
        let ran_out = rig.ended(first, AsideEnd::RanOut);
        assert_eq!(ran_out, Vec::new(), "a single run-out restores by itself");
        assert!(!rig.p.live());
        let asks = rig.press(2);
        let (ticket, url, then) = play(&asks);
        assert_eq!(asks.len(), 1, "no resolve: {asks:?}");
        assert_ne!(ticket, first, "every clip has its own ticket");
        assert_eq!((url, then), ("http://cdn/3.mp3", AsideThen::Restore));
    }

    #[test]
    fn null_is_none_for_the_visit_and_mutes_the_slot() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(7));
        assert_eq!(rig.answer(ticket, Ok(None)), Vec::new());
        assert!(!rig.p.live());
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.slot(7), Slot::Muted);
        // It stays muted on another tab that lists it.
        rig.leads = leads("taste_basement");
        assert_eq!(rig.slot(3), Slot::Muted, "Arcade Rain on the Basement tab");
        assert_eq!(rig.press(3), Vec::new(), "never asked again this visit");
    }

    #[test]
    fn a_press_on_a_muted_lead_notes_no_preview_and_asks_nothing() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(7));
        rig.answer(ticket, Ok(None));
        rig.p.note = None;
        let live = rig.playing(2, "http://cdn/3.mp3");
        assert_eq!(rig.press(7), Vec::new());
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.ticket(), live, "the live clip goes on");
        assert_eq!(rig.slot(2), Slot::Playing);
    }

    #[test]
    fn a_press_on_the_live_lead_toggles_or_stops() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.press(2));
        // Still on its way: a press drops it.
        assert_eq!(rig.press(2), vec![STOP]);
        assert!(!rig.p.live());
        // Its answer is kept, and plays on the next press.
        assert_eq!(rig.found(first, "http://cdn/3.mp3"), Vec::new());
        let (ticket, ..) = play(&rig.press(2));
        // Opening: a press drops it too.
        assert_eq!(rig.press(2), vec![STOP]);
        let (ticket2, ..) = play(&rig.press(2));
        assert_ne!(ticket, ticket2);
        rig.status(ticket2, AsidePhase::Playing, 3.0);
        assert_eq!(rig.press(2), vec![Ask::Pause { ticket: ticket2 }]);
        assert_eq!(rig.slot(2), Slot::Playing, "the status moves when the engine says so");
        rig.status(ticket2, AsidePhase::Paused, 3.0);
        assert_eq!(rig.slot(2), Slot::Paused);
        assert_eq!(rig.press(2), vec![Ask::Resume { ticket: ticket2 }]);
        rig.status(ticket2, AsidePhase::Playing, 3.1);
        assert_eq!(rig.slot(2), Slot::Playing);
        // The band's toggle and Space do the same.
        assert_eq!(rig.p.toggle(), vec![Ask::Pause { ticket: ticket2 }]);
    }

    #[test]
    fn a_switch_holds_main_and_plays_the_new_clip() {
        let mut rig = Rig::new();
        let first = rig.playing(2, "http://cdn/3.mp3");
        // Nothing known for Harbor Lights: the old clip stops at once and main
        // stays paused while it is found.
        let asks = rig.press(3);
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN }, "{asks:?}");
        let (ticket, ..) = resolve(&asks);
        assert_eq!(rig.slot(2), Slot::Idle);
        assert_eq!(rig.slot(3), Slot::Busy);
        assert_eq!(rig.ended(first, AsideEnd::Superseded), Vec::new());
        assert_eq!(play(&rig.found(ticket, "http://cdn/4.mp3")).1, "http://cdn/4.mp3");
        rig.status(ticket, AsidePhase::Playing, 1.0);
        // Back to a lead the visit knows: it plays at once, no hold.
        let asks = rig.press(2);
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert_eq!(play(&asks).1, "http://cdn/3.mp3");
    }

    #[test]
    fn next_at_the_lists_end_ends_the_clip() {
        let mut rig = Rig::new();
        rig.playing(2, "http://cdn/3.mp3");
        let asks = rig.next();
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN });
        let (harbor, _, rec) = resolve(&asks);
        assert_eq!(rec, &rig.leads[3].raw, "n goes to Harbor Lights");
        assert_eq!(play(&rig.found(harbor, "http://cdn/4.mp3")).1, "http://cdn/4.mp3");
        let last = rig.leads.len() - 1;
        rig.p.end(true);
        rig.playing(last, "http://cdn/12.mp3");
        assert_eq!(rig.next(), vec![STOP]);
        assert!(!rig.p.live());
    }

    // ── pacing ──────────────────────────────────────────────────────────

    #[test]
    fn a_429_waits_5_s_or_60_s_and_retries_once_then_fails_quietly() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(5));
        assert_eq!(rig.answer(ticket, Err(refused(429))), Vec::new());
        assert_eq!(rig.slot(5), Slot::Busy, "waiting reads busy");
        assert_eq!(rig.tick(Duration::ZERO), Vec::new());
        assert_eq!(rig.tick(Duration::from_millis(4_900)), Vec::new());
        let asks = rig.tick(Duration::from_millis(200));
        assert_eq!(resolve(&asks).0, ticket, "the one retry");
        // A second 429 is "No preview" for this press only.
        assert_eq!(rig.answer(ticket, Err(refused(429))), Vec::new());
        assert!(!rig.p.live());
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.slot(5), Slot::Idle, "never stored: the next press asks again");
        assert_eq!(rig.tick(Duration::from_secs(10)), Vec::new(), "no third ask");
        let (again, ..) = resolve(&rig.press(5));
        rig.answer(again, Err(refused(500)));
        // iTunes' limiter waits a minute.
        rig.provider = Some("itunes".into());
        let (ticket, ..) = resolve(&rig.press(5));
        rig.answer(ticket, Err(refused(429)));
        rig.tick(Duration::ZERO);
        assert_eq!(rig.tick(Duration::from_secs(59)), Vec::new());
        assert_eq!(resolve(&rig.tick(Duration::from_millis(1_100))).1, "itunes");
    }

    #[test]
    fn one_resolve_in_flight_and_a_newer_press_replaces_the_queued_one() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.press(2));
        assert_eq!(rig.press(3), Vec::new(), "queued behind the flight");
        assert_eq!(rig.press(4), Vec::new(), "replaces the queued press");
        assert_eq!(rig.slot(4), Slot::Busy);
        assert_eq!(rig.slot(3), Slot::Idle);
        // The flight's answer is kept for its lead, and plays nothing.
        assert_eq!(rig.found(first, "http://cdn/3.mp3"), Vec::new());
        assert_eq!(rig.tick(Duration::ZERO), Vec::new());
        assert_eq!(rig.tick(Duration::from_millis(299)), Vec::new());
        let asks = rig.tick(Duration::from_millis(1));
        assert_eq!(resolve(&asks).2, &rig.leads[4].raw, "Glass Season, 300 ms after the flight");
        assert_eq!(asks.len(), 1);
        // Harbor Lights was never asked; Signal Fires' answer stands.
        rig.p.end(true);
        assert_eq!(play(&rig.press(2)).1, "http://cdn/3.mp3");
    }

    // ── Preview all ─────────────────────────────────────────────────────

    #[test]
    fn preview_all_skips_none_ends_on_failed_and_restores_at_the_end() {
        let mut rig = Rig::new();
        let (salt, ..) = resolve(&rig.press(0));
        rig.answer(salt, Ok(None));
        rig.p.note = None;
        // From the top, past Salt Road (none this visit).
        let asks = rig.all();
        assert_eq!(asks.len(), 1, "nothing to hold yet: {asks:?}");
        let (ticket, _, rec) = resolve(&asks);
        assert_eq!(rec, &rig.leads[1].raw);
        assert_eq!(play(&rig.found(ticket, "http://cdn/2.mp3")).2, AsideThen::Hold);
        rig.status(ticket, AsidePhase::Playing, 1.0);
        // Run out: the next lead, main held while it is found.
        let asks = rig.ended(ticket, AsideEnd::RanOut);
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN });
        let (signal, _, rec) = resolve(&asks);
        assert_eq!(rec, &rig.leads[2].raw);
        // None: skipped.
        let asks = rig.answer(signal, Ok(None));
        assert_eq!(resolve(&asks).2, &rig.leads[3].raw);
        assert_eq!(rig.p.note, None, "a chain says nothing");
        // Failed: the chain ends there, quietly, and main comes back.
        let (harbor, ..) = resolve(&asks);
        assert_eq!(rig.answer(harbor, Err(refused(500))), vec![STOP]);
        assert!(!rig.p.live());
        assert_eq!(rig.p.note, None);
        // At the list's end, the session ends and main is restored.
        rig.leads = leads("taste_3f9a");
        let (a, ..) = resolve(&rig.all());
        rig.found(a, "http://cdn/6.mp3");
        let (b, ..) = resolve(&rig.ended(a, AsideEnd::RanOut));
        rig.found(b, "http://cdn/9.mp3");
        assert_eq!(rig.ended(b, AsideEnd::RanOut), vec![STOP]);
        assert!(!rig.p.live());
    }

    #[test]
    fn preview_all_prefetches_one_ahead_after_20_s_and_never_retries() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.all());
        rig.found(first, "http://cdn/1.mp3");
        assert_eq!(rig.status(first, AsidePhase::Playing, 19.9), Vec::new());
        let asks = rig.status(first, AsidePhase::Playing, 20.0);
        let (ahead, _, rec) = resolve(&asks);
        assert_ne!(ahead, first);
        assert_eq!(rec, &rig.leads[1].raw, "the next lead");
        assert!(!asks.iter().any(|a| matches!(a, Ask::Play { .. })));
        assert_eq!(rig.status(first, AsidePhase::Playing, 21.0), Vec::new(), "once per clip");
        // A 429 stores nothing and is never retried.
        assert_eq!(rig.answer(ahead, Err(refused(429))), Vec::new());
        assert_eq!(rig.tick(Duration::from_secs(30)), Vec::new());
        // The next lead is asked for when the chain gets there.
        let (second, ..) = resolve(&rig.ended(first, AsideEnd::RanOut));
        rig.found(second, "http://cdn/2.mp3");
        let (ahead, ..) = resolve(&rig.status(second, AsidePhase::Playing, 22.0));
        // A found look ahead is kept, and plays with no resolve.
        assert_eq!(rig.found(ahead, "http://cdn/3.mp3"), Vec::new());
        let asks = rig.ended(second, AsideEnd::RanOut);
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert_eq!(play(&asks).1, "http://cdn/3.mp3");
        // Never for a lead already answered.
        let third = rig.ticket();
        let harbor = rules::lead_key(&rig.leads[3]);
        rig.p.store.put(&harbor, "deezer", Some(clip("http://cdn/4.mp3")));
        assert_eq!(rig.status(third, AsidePhase::Playing, 25.0), Vec::new());
    }

    #[test]
    fn preview_all_renews_the_hold_at_every_wait() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.all());
        rig.found(first, "http://cdn/1.mp3");
        rig.status(first, AsidePhase::Playing, 1.0);
        let asks = rig.ended(first, AsideEnd::RanOut);
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN }, "the step's resolve");
        let (second, ..) = resolve(&asks);
        assert_eq!(
            rig.answer(second, Err(refused(429))),
            vec![Ask::Hold { span: Duration::from_secs(5) + HOLD_SPAN }],
            "the 429's wait"
        );
        rig.tick(Duration::ZERO);
        let asks = rig.tick(Duration::from_secs(5));
        assert_eq!(resolve(&asks).0, second);
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN }, "the retry renews it: {asks:?}");
        assert_eq!(asks.len(), 2, "{asks:?}");
        assert_eq!(play(&rig.found(second, "http://cdn/2.mp3")).2, AsideThen::Hold);
    }

    #[test]
    fn a_chain_step_queued_behind_the_look_ahead_renews_the_hold_when_it_goes() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.all());
        rig.found(first, "http://cdn/1.mp3");
        let (ahead, ..) = resolve(&rig.status(first, AsidePhase::Playing, 21.0));
        // The clip runs out while the look ahead is still out: the step waits for it.
        let asks = rig.ended(first, AsideEnd::RanOut);
        assert_eq!(asks, vec![Ask::Hold { span: HOLD_SPAN }], "the old clip stops, main held");
        assert_eq!(rig.answer(ahead, Err(refused(500))), Vec::new(), "a look ahead says nothing");
        rig.tick(Duration::ZERO);
        let asks = rig.tick(QUEUE_DELAY);
        assert_eq!(resolve(&asks).2, &rig.leads[1].raw);
        assert_eq!(asks.len(), 2, "{asks:?}");
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN }, "the hold runs from the send");
    }

    #[test]
    fn the_hold_outlasts_the_slowest_resolve_that_still_answers() {
        // The client gives a request 20 s (api's REQUEST_TIMEOUT), and the engine holds at most
        // 90 s (aside's ASIDE_HOLD_MOST), which iTunes' 60 s wait plus the hold stays under.
        assert!(HOLD_SPAN >= Duration::from_secs(20) + Duration::from_secs(5), "{HOLD_SPAN:?}");
        assert!(HOLD_SPAN + Duration::from_secs(60) <= Duration::from_secs(90), "{HOLD_SPAN:?}");
    }

    #[test]
    fn pressing_preview_all_again_stops_it() {
        let mut rig = Rig::new();
        let single = rig.playing(5, "http://cdn/6.mp3");
        // A single clip live is replaced by the chain from the top.
        let asks = rig.all();
        assert_eq!(asks[0], Ask::Hold { span: HOLD_SPAN });
        assert_eq!(resolve(&asks).2, &rig.leads[0].raw);
        assert_ne!(rig.ticket(), single);
        assert_eq!(rig.all(), vec![STOP]);
        assert!(!rig.p.live());
    }

    #[test]
    fn a_list_change_drops_the_chain_and_lets_the_clip_play_out() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.all());
        rig.found(first, "http://cdn/1.mp3");
        rig.status(first, AsidePhase::Playing, 21.0);
        rig.p.list_changed();
        assert!(rig.p.live(), "the clip plays on");
        assert_eq!(rig.slot(0), Slot::Playing);
        // Its hold is not wanted any more: main comes back at once.
        assert_eq!(rig.ended(first, AsideEnd::RanOut), vec![STOP]);
        assert!(!rig.p.live());
    }

    // ── engine ends ─────────────────────────────────────────────────────

    #[test]
    fn a_dead_clip_forgets_only_its_own_url_and_reads_idle() {
        let mut rig = Rig::new();
        let dead = rig.playing(2, "http://cdn/expired.mp3");
        assert_eq!(rig.ended(dead, AsideEnd::Failed("404".into())), vec![STOP]);
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.slot(2), Slot::Idle, "idle, not muted");
        // The next press asks again, and a fresh address is kept.
        let (ticket, ..) = resolve(&rig.press(2));
        rig.found(ticket, "http://cdn/fresh.mp3");
        rig.p.end(true);
        // A late word about the dead address leaves the fresh one alone.
        assert_eq!(rig.ended(dead, AsideEnd::Failed("404".into())), Vec::new());
        assert_eq!(play(&rig.press(2)).1, "http://cdn/fresh.mp3");
        // A clip that played out stays kept.
        let ticket = rig.ticket();
        rig.ended(ticket, AsideEnd::RanOut);
        assert_eq!(play(&rig.press(2)).1, "http://cdn/fresh.mp3");
    }

    #[test]
    fn a_refused_clip_keeps_its_answer() {
        let mut rig = Rig::new();
        let ticket = rig.playing(2, "http://cdn/3.mp3");
        assert_eq!(rig.ended(ticket, AsideEnd::Refused("no output device".into())), vec![STOP]);
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.slot(2), Slot::Idle);
        let asks = rig.press(2);
        assert_eq!(asks.len(), 1, "{asks:?}");
        assert_eq!(play(&asks).1, "http://cdn/3.mp3", "kept: the clip was not at fault");
    }

    #[test]
    fn a_taken_clip_ends_the_session_and_the_chain_without_restore() {
        let mut rig = Rig::new();
        let (first, ..) = resolve(&rig.all());
        rig.found(first, "http://cdn/1.mp3");
        rig.status(first, AsidePhase::Playing, 4.0);
        assert_eq!(rig.ended(first, AsideEnd::Taken), Vec::new(), "nothing is restored");
        assert!(!rig.p.live());
        assert_eq!(rig.ended(first, AsideEnd::RanOut), Vec::new(), "the chain is over");
        assert_eq!(rig.tick(Duration::from_secs(10)), Vec::new());
        let single = rig.playing(3, "http://cdn/4.mp3");
        assert_eq!(rig.ended(single, AsideEnd::Taken), Vec::new());
        assert_eq!(rig.p.note, None);
        assert_eq!(rig.slot(3), Slot::Idle);
    }

    #[test]
    fn a_main_command_cancels_a_pending_press() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(2));
        rig.p.main_command();
        assert!(!rig.p.live());
        assert_eq!(rig.found(ticket, "http://cdn/3.mp3"), Vec::new(), "nothing plays later");
        assert_eq!(rig.slot(2), Slot::Idle);
        // A 429's wait is cancelled too: no retry.
        let (ticket, ..) = resolve(&rig.press(5));
        rig.answer(ticket, Err(refused(429)));
        rig.p.main_command();
        rig.tick(Duration::ZERO);
        assert_eq!(rig.tick(Duration::from_secs(6)), Vec::new());
        // The answer that came is still the visit's.
        assert_eq!(play(&rig.press(2)).1, "http://cdn/3.mp3");
    }

    // ── the store, the gate and the fences ──────────────────────────────

    #[test]
    fn a_resolve_403_clears_the_store_and_reprobes() {
        let mut rig = Rig::new();
        let (arcade, ..) = resolve(&rig.press(7));
        rig.answer(arcade, Ok(None));
        let (ticket, ..) = resolve(&rig.press(2));
        assert_eq!(rig.answer(ticket, Err(refused(403))), vec![Ask::Reprobe]);
        assert!(!rig.p.live());
        assert_eq!(rig.slot(7), Slot::Idle, "the store was cleared");
        // With a clip held, the engine is told to let main go.
        rig.playing(2, "http://cdn/3.mp3");
        let (ticket, ..) = resolve(&rig.press(3));
        assert_eq!(rig.answer(ticket, Err(refused(403))), vec![STOP, Ask::Reprobe]);
    }

    #[test]
    fn a_resolve_404_fails_and_regates() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(2));
        assert_eq!(rig.answer(ticket, Err(refused(404))), vec![Ask::Regate]);
        assert_eq!(rig.p.note.take(), Some(PreviewNote::NoPreview));
        assert_eq!(rig.slot(2), Slot::Idle, "a failure is never stored");
        assert!(matches!(rig.press(2).as_slice(), [Ask::Resolve { .. }]));
    }

    #[test]
    fn the_store_holds_at_most_200_oldest_first_out() {
        let mut store = Store::default();
        for n in 0..STORE_CAP {
            store.put(&format!("lead {n}"), "deezer", None);
        }
        // Answered again, the first keeps its place.
        store.put("lead 0", "deezer", Some(clip("http://cdn/0.mp3")));
        store.put("lead 200", "deezer", None);
        assert_eq!(store.leads.len(), STORE_CAP);
        assert!(store.of("lead 0", "deezer").is_none(), "the oldest went first");
        assert!(store.none("lead 1", "deezer"));
        assert!(store.none("lead 200", "deezer"));
        // Another plug-in's answer for a kept lead takes no new place.
        store.put("lead 1", "itunes", None);
        assert_eq!(store.leads.len(), STORE_CAP);
        assert!(store.none("lead 1", "itunes") && store.none("lead 2", "deezer"));
    }

    #[test]
    fn a_provider_change_clears_the_store() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(7));
        rig.answer(ticket, Ok(None));
        rig.p.provider_changed(Some("deezer".into()));
        assert_eq!(rig.slot(7), Slot::Muted, "the same provider keeps the answers");
        rig.p.provider_changed(Some("itunes".into()));
        rig.provider = Some("itunes".into());
        assert_eq!(rig.slot(7), Slot::Idle);
        rig.p.provider_changed(Some("deezer".into()));
        rig.provider = Some("deezer".into());
        assert_eq!(rig.slot(7), Slot::Idle, "cleared, not set aside");
        // No provider at all: a live clip ends, its stop owed to the tick.
        rig.playing(2, "http://cdn/3.mp3");
        rig.p.provider_changed(None);
        rig.provider = None;
        assert!(!rig.p.live());
        assert_eq!(rig.tick(Duration::ZERO), vec![STOP]);
        assert_eq!(rig.tick(Duration::ZERO), Vec::new());
    }

    #[test]
    fn no_audio_or_no_provider_offers_no_preview() {
        let mut rig = Rig::new();
        rig.audio = false;
        assert_eq!(rig.press(2), Vec::new());
        assert_eq!(rig.all(), Vec::new());
        assert_eq!(rig.slot(2), Slot::Idle);
        assert_eq!(rig.p.note, None);
        rig.audio = true;
        rig.provider = None;
        assert_eq!(rig.press(2), Vec::new());
        assert_eq!(rig.all(), Vec::new());
        assert!(rig.band().is_none());
    }

    #[test]
    fn a_stale_ticket_or_visit_does_nothing() {
        let mut rig = Rig::new();
        let (ticket, ..) = resolve(&rig.press(2));
        assert_eq!(rig.found(ticket + 1_000, "http://cdn/x.mp3"), Vec::new());
        assert_eq!(rig.status(ticket + 1_000, AsidePhase::Playing, 1.0), Vec::new());
        assert_eq!(rig.ended(ticket + 1_000, AsideEnd::RanOut), Vec::new());
        assert_eq!(rig.slot(2), Slot::Busy, "the press stands");
        // Another visit: the old press's answer is not its own.
        rig.visit = 2;
        assert_eq!(rig.found(ticket, "http://cdn/3.mp3"), Vec::new());
        assert!(!rig.p.live());
        assert_eq!(rig.tick(Duration::ZERO), vec![STOP], "the old clip's stop is owed");
        assert!(matches!(rig.press(2).as_slice(), [Ask::Resolve { .. }]), "nothing was kept");
    }

    #[test]
    fn the_band_follows_the_engine_status_and_falls_back_to_30_s() {
        let mut rig = Rig::new();
        assert!(rig.band().is_none());
        let (ticket, ..) = resolve(&rig.press(2));
        let band = rig.band().expect("a band while resolving");
        assert_eq!(band.slot, Slot::Busy);
        assert_eq!(band.index, Some(2));
        assert_eq!(band.title, "Signal Fires");
        assert_eq!(band.artist.as_deref(), Some("Paper Lantern"));
        assert_eq!((band.attribution, band.link), (None, None), "no attribution before the answer");
        assert_eq!((band.position, band.duration), (0.0, 30.0));
        let mut found = clip("http://cdn/3.mp3");
        found.link = Some("http://www.deezer.com/track/3".into());
        rig.answer(ticket, Ok(Some(found)));
        let band = rig.band().unwrap();
        assert_eq!(band.slot, Slot::Busy, "opening");
        assert_eq!(band.attribution.as_deref(), Some("Preview from Deezer"));
        assert_eq!(band.link, None, "an http link is never opened");
        rig.status(ticket, AsidePhase::Playing, 12.0);
        let band = rig.band().unwrap();
        assert_eq!((band.slot, band.position, band.duration), (Slot::Playing, 12.0, 30.0));
        let status =
            AsideStatus { ticket, phase: AsidePhase::Paused, position: 12.5, duration: 29.5 };
        rig.p.status(view!(rig), Some(status));
        let band = rig.band().unwrap();
        assert_eq!((band.slot, band.position, band.duration), (Slot::Paused, 12.5, 29.5));
        rig.p.end(true);
        assert!(rig.band().is_none());
    }

    #[test]
    fn no_artwork_is_ever_asked() {
        let mut rig = Rig::new();
        let answer = fixtures::preview("preview_itunes");
        let found = PluginPreview::from_value(&answer["result"]["preview"], Some("itunes"))
            .expect("the iTunes fixture has a clip");
        rig.provider = Some("itunes".into());
        let mut asks = rig.press(2);
        let (ticket, ..) = resolve(&asks);
        asks.extend(rig.answer(ticket, Ok(Some(found))));
        asks.extend(rig.status(ticket, AsidePhase::Playing, 21.0));
        asks.extend(rig.ended(ticket, AsideEnd::RanOut));
        asks.extend(rig.all());
        let dump = format!("{asks:?}");
        for art in ["mzstatic", "artwork", "album-art", "/art/"] {
            assert!(!dump.contains(art), "{art} in {dump}");
        }
        assert!(asks.iter().all(|a| match a {
            Ask::Play { url, .. } => url.ends_with(".m4a"),
            Ask::Resolve { rec, .. } => rig.leads.iter().any(|l| &l.raw == rec),
            _ => true,
        }));
    }

    #[test]
    fn the_live_slot_follows_its_lead_key_when_the_list_moves() {
        let mut rig = Rig::new();
        rig.playing(2, "http://cdn/3.mp3");
        // The Kitchen tab: Signal Fires is its first row.
        rig.leads = leads("taste_kitchen");
        assert_eq!(rig.slot(0), Slot::Playing);
        assert_eq!(rig.slot(2), Slot::Idle, "Long Exposure is not the clip");
        assert_eq!(rig.band().unwrap().index, Some(0));
        // The Basement tab has no Signal Fires: the band stands, unplaced.
        rig.leads = leads("taste_basement");
        let band = rig.band().expect("the band outlives the row");
        assert_eq!(band.index, None);
        assert_eq!(band.title, "Signal Fires");
        assert!((0..rig.leads.len()).all(|i| rig.slot(i) == Slot::Idle));
        // `n` walks the clip's own list, not the tab's.
        let asks = rig.next();
        assert_eq!(resolve(&asks).2["title"], "Harbor Lights");
    }

    // ── through the App ─────────────────────────────────────────────────

    mod room {
        use super::*;
        use crate::api::peer_recs_door::PeerRecsEvent;
        use crate::api::types::{Capabilities, DiscoveryPluginsResponse};
        use crate::tui::worker::{ApiCmd, AudioCmd, Event};
        use serde_json::json;

        fn cmds(effects: &[Effect]) -> Vec<PeerRecsCmd> {
            effects
                .iter()
                .filter_map(|e| match e {
                    Effect::Api(ApiCmd::PeerRecs { cmd, .. }) => Some(cmd.clone()),
                    _ => None,
                })
                .collect()
        }

        fn land(app: &mut App, event: PeerRecsEvent) -> Vec<Effect> {
            let generation = app.session_gen();
            app.apply_event(Event::PeerRecs { generation, event: Box::new(event) })
        }

        fn listing(preview: bool) -> DiscoveryPluginsResponse {
            let deezer = if preview { "preview" } else { "links" };
            serde_json::from_value(json!({
                "plugins": [
                    {"name": "links", "title": "Links", "capabilities": ["links"]},
                    {"name": "deezer", "title": "Deezer", "capabilities": [deezer]}
                ],
                "jobs": {"allowed": false}
            }))
            .unwrap()
        }

        /// A visit open on the stage's All batch, the gate read with
        /// `listing`.
        fn room(plugins: DiscoveryPluginsResponse) -> App {
            let app = room_on(plugins, fixtures::taste("taste_all"));
            assert_eq!(app.peer_recs_leads().len(), 12, "the stage's All batch is shown");
            app
        }

        /// A visit open on `taste`'s batch, landed through the App's door.
        fn room_on(
            plugins: DiscoveryPluginsResponse,
            taste: crate::api::types::TasteResponse,
        ) -> App {
            let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
            app.connected = true;
            app.capabilities = Capabilities {
                discovery: true,
                discovery_p2p: true,
                discovery_taste: 3,
                discovery_p2p_peers: 1,
                discovery_plugins: true,
                ..Capabilities::default()
            };
            let effects = app.peer_recs_open(None);
            for cmd in cmds(&effects) {
                match cmd {
                    PeerRecsCmd::Taste { seq, .. } => {
                        let answer = Ok(Box::new(taste.clone()));
                        land(&mut app, PeerRecsEvent::Taste { seq, answer });
                    }
                    PeerRecsCmd::Gate { seq } => {
                        let plugins = Ok(Box::new(plugins.clone()));
                        land(&mut app, PeerRecsEvent::Gate { seq, plugins, destination: None });
                    }
                    _ => {}
                }
            }
            app
        }

        /// The resolve a press sent: its ticket, plug-in and row.
        fn resolve_cmd(effects: &[Effect]) -> (u64, String, Value) {
            match cmds(effects).as_slice() {
                [PeerRecsCmd::Preview { ticket, plugin, rec }] => {
                    (*ticket, plugin.clone(), (**rec).clone())
                }
                other => panic!("expected one resolve, got {other:?} in {effects:?}"),
            }
        }

        fn found(app: &mut App, ticket: u64, url: &str) -> Vec<Effect> {
            let answer = Ok(Some(Box::new(clip(url))));
            land(app, PeerRecsEvent::Preview { ticket, answer })
        }

        #[test]
        fn a_press_rides_the_rooms_door_and_plays_on_the_aside_sink() {
            let mut app = room(listing(true));
            assert!(app.peer_recs_preview_offered());
            let effects = app.peer_recs_press(2, Instant::now());
            let (ticket, plugin, rec) = resolve_cmd(&effects);
            assert_eq!(plugin, "deezer");
            assert_eq!(rec, app.peer_recs_leads()[2].raw);
            assert_eq!(app.peer_recs_slot(2), Slot::Busy);
            let url = "http://127.0.0.1:3996/preview/3.mp3";
            let effects = found(&mut app, ticket, url);
            let play = AudioCmd::AsidePlay { ticket, url: url.into(), then: AsideThen::Restore };
            assert_eq!(effects, vec![Effect::Audio(play)]);
            let status =
                AsideStatus { ticket, phase: AsidePhase::Playing, position: 4.0, duration: 30.0 };
            app.apply_event(Event::Aside(Some(status)));
            assert_eq!(app.peer_recs_slot(2), Slot::Playing);
            let band = app.peer_recs_band().expect("the band");
            assert_eq!((band.index, band.position), (Some(2), 4.0));
            let pause = Effect::Audio(AudioCmd::AsidePause { ticket });
            assert_eq!(app.peer_recs_clip_toggle(), vec![pause]);
            app.apply_event(Event::AsideEnded(AsideEnded { ticket, end: AsideEnd::RanOut }));
            assert!(!app.peer_recs_clip_live());
            assert!(app.peer_recs_band().is_none());
        }

        #[test]
        fn a_main_command_noted_on_its_way_out_cancels_the_press() {
            let mut app = room(listing(true));
            let effects = app.peer_recs_press(2, Instant::now());
            let (ticket, ..) = resolve_cmd(&effects);
            // The room's own effects pass the same funnel and end nothing.
            app.note_pending(&effects);
            app.note_pending(&[Effect::Audio(AudioCmd::AsideHold { span: HOLD_SPAN })]);
            assert!(app.peer_recs_clip_live());
            app.note_pending(&[Effect::Audio(AudioCmd::Resume)]);
            assert!(!app.peer_recs_clip_live());
            let late = found(&mut app, ticket, "http://cdn/3.mp3");
            assert_eq!(late, Vec::new(), "nothing plays later");
        }

        #[test]
        fn a_resolve_403_asks_the_rooms_re_ping_and_a_404_reads_the_gate() {
            let mut app = room(listing(true));
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            let answer = Err(refused(403));
            let effects = land(&mut app, PeerRecsEvent::Preview { ticket, answer });
            assert!(cmds(&effects).contains(&PeerRecsCmd::Probe), "{effects:?}");
            let mut app = room(listing(true));
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            let answer = Err(refused(404));
            let effects = land(&mut app, PeerRecsEvent::Preview { ticket, answer });
            let regate = cmds(&effects).iter().any(|c| matches!(c, PeerRecsCmd::Gate { .. }));
            assert!(regate, "{effects:?}");
            assert_eq!(app.peer_recs_take_note(), Some(PreviewNote::NoPreview));
            assert_eq!(app.peer_recs_take_note(), None, "once");
        }

        #[test]
        fn the_room_offers_previews_only_with_audio_and_a_preview_plugin() {
            let mut app = room(listing(true));
            app.audio_available = false;
            assert!(!app.peer_recs_preview_offered());
            assert!(app.peer_recs_press(2, Instant::now()).is_empty());
            let mut app = room(listing(false));
            assert!(!app.peer_recs_preview_offered());
            assert!(app.peer_recs_preview_all(Instant::now()).is_empty());
            assert_eq!(app.peer_recs_slot(2), Slot::Idle);
        }

        #[test]
        fn the_rooms_tick_sends_the_retry_and_the_queued_press() {
            let mut app = room(listing(true));
            let t0 = Instant::now();
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(5, t0));
            let answer = Err(refused(429));
            land(&mut app, PeerRecsEvent::Preview { ticket, answer });
            assert!(cmds(&app.tick_at(t0)).is_empty());
            let effects = app.tick_at(t0 + Duration::from_secs(5));
            assert_eq!(resolve_cmd(&effects).0, ticket, "the retry");
            // A press behind it goes 300 ms after it answers.
            let effects = app.peer_recs_press(3, t0 + Duration::from_secs(5));
            assert!(cmds(&effects).is_empty());
            found(&mut app, ticket, "http://cdn/6.mp3");
            let at = t0 + Duration::from_secs(6);
            assert!(cmds(&app.tick_at(at)).is_empty());
            let effects = app.tick_at(at + QUEUE_DELAY);
            assert_eq!(resolve_cmd(&effects).2, app.peer_recs_leads()[3].raw);
        }

        /// Main plays the first of `paths`, its status naming it: the source its end names.
        fn main_playing(app: &mut App, paths: &[&str]) -> String {
            use crate::api::types::{Track, TrackMetadata};
            use crate::player::PlayerStatus;
            let track = |path: &&str| Track {
                filepath: path.to_string(),
                metadata: TrackMetadata::default(),
            };
            app.replace_queue(paths.iter().map(track).collect());
            let effects = app.handle_action(crate::tui::app::Action::PlayPause);
            let url = effects.iter().find_map(|effect| match effect {
                Effect::Audio(AudioCmd::Play { url, .. }) => Some(url.clone()),
                _ => None,
            });
            let url = url.unwrap_or_else(|| panic!("main plays: {effects:?}"));
            let playing = PlayerStatus {
                source: url.clone(),
                playing: true,
                position: 170.0,
                duration: 174.0,
                ..PlayerStatus::default()
            };
            app.apply_event(Event::Status(playing));
            url
        }

        fn clip_sent(effects: &[Effect]) -> bool {
            effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::AsidePlay { .. })))
        }

        #[test]
        fn a_tracks_own_end_leaves_a_press_that_resolves_or_waits() {
            // Main runs out and the queue moves on by itself: not the listener's command, so a
            // press still finding its clip goes on (peer-recs build plan D19).
            let mut app = room(listing(true));
            let ended = main_playing(&mut app, &["a.mp3", "b.mp3"]);
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            let advanced = app.apply_event(Event::TrackEnded { source: ended });
            let next = advanced.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. })));
            assert!(next, "the queue moved on: {advanced:?}");
            // The GUI notes a batch once more on its way out (`Gui::pend`).
            app.note_pending(&advanced);
            assert!(app.peer_recs_clip_live());
            let played = found(&mut app, ticket, "http://cdn/3.mp3");
            assert!(clip_sent(&played), "the answer still plays: {played:?}");

            // A 429's wait, through the Stop at the queue's end.
            let mut app = room(listing(true));
            let ended = main_playing(&mut app, &["a.mp3"]);
            let t0 = Instant::now();
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(5, t0));
            land(&mut app, PeerRecsEvent::Preview { ticket, answer: Err(refused(429)) });
            app.tick_at(t0);
            let advanced = app.apply_event(Event::TrackEnded { source: ended });
            let stop = advanced.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Stop)));
            assert!(stop, "the queue's end: {advanced:?}");
            app.note_pending(&advanced);
            let effects = app.tick_at(t0 + Duration::from_secs(5));
            assert_eq!(resolve_cmd(&effects).0, ticket, "the retry still goes");

            // The listener's own command after it still cancels.
            let mut app = room(listing(true));
            let ended = main_playing(&mut app, &["a.mp3", "b.mp3"]);
            let advanced = app.apply_event(Event::TrackEnded { source: ended });
            app.note_pending(&advanced);
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            app.handle_action(crate::tui::app::Action::PlayPause);
            assert!(!app.peer_recs_clip_live());
            assert!(!clip_sent(&found(&mut app, ticket, "http://cdn/3.mp3")), "nothing plays later");
        }

        fn main_play(effects: &[Effect]) -> bool {
            effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. })))
        }

        #[test]
        fn a_surprise_me_opener_is_the_listeners_start_and_cancels_a_waiting_press() {
            // The opener is the listener's "Surprise me" landing late, as a Listing's play-all
            // does: a main command, not the App's advance (peer-recs build plan D19).
            use crate::api::types::{Track, TrackMetadata};
            let mut app = room(listing(true));
            let t0 = Instant::now();
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(5, t0));
            land(&mut app, PeerRecsEvent::Preview { ticket, answer: Err(refused(429)) });
            app.tick_at(t0);
            let asked = app.dj_surprise();
            let ask = asked.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDj(_))));
            assert!(ask && app.lane.opener, "the opener is out: {asked:?}");
            app.note_pending(&asked);
            assert!(app.peer_recs_clip_live(), "the ask alone plays nothing");
            let song = Track { filepath: "dj.mp3".into(), metadata: TrackMetadata::default() };
            let pick = Event::AutoDjPick {
                epoch: app.lane.epoch,
                songs: vec![song],
                ignore_list: Vec::new(),
                sonic: true,
                note: None,
                failure: None,
            };
            let started = app.apply_event(pick);
            assert!(main_play(&started), "the opener plays: {started:?}");
            app.note_pending(&started);
            assert!(!app.peer_recs_clip_live(), "the press is cancelled");
            let effects = app.tick_at(t0 + Duration::from_secs(5));
            assert!(cmds(&effects).is_empty(), "no retry goes out: {effects:?}");
        }

        #[test]
        fn a_failed_rows_own_skip_or_retry_leaves_a_press_finding_its_clip() {
            // A row that will not open is walked past by the App, no listener behind it.
            let mut app = room(listing(true));
            let failing = main_playing(&mut app, &["a.mp3", "b.mp3"]);
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            let error = "decode error".to_string();
            let skipped = app.apply_event(Event::PlaybackFailed { source: failing, error });
            assert!(main_play(&skipped), "the next row plays: {skipped:?}");
            app.note_pending(&skipped);
            assert!(app.peer_recs_clip_live());
            assert!(clip_sent(&found(&mut app, ticket, "http://cdn/3.mp3")), "the answer plays");

            // A transient failure: the probe's answer plays the row again, through a 429's wait.
            let mut app = room(listing(true));
            let failing = main_playing(&mut app, &["a.mp3"]);
            let t0 = Instant::now();
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(5, t0));
            land(&mut app, PeerRecsEvent::Preview { ticket, answer: Err(refused(429)) });
            app.tick_at(t0);
            let error = "connection reset".to_string();
            let probed = app.apply_event(Event::PlaybackFailed { source: failing, error });
            let server = probed.iter().find_map(|e| match e {
                Effect::Api(ApiCmd::Probe { server, .. }) => Some(server.clone()),
                _ => None,
            });
            let server = server.unwrap_or_else(|| panic!("a probe: {probed:?}"));
            app.note_pending(&probed);
            let retried = app.apply_event(Event::Reachable { server, reachable: true });
            assert!(main_play(&retried), "the row is played again: {retried:?}");
            app.note_pending(&retried);
            let effects = app.tick_at(t0 + Duration::from_secs(5));
            assert_eq!(resolve_cmd(&effects).0, ticket, "the retry still goes");
        }

        #[test]
        fn the_listeners_seek_or_next_while_a_press_resolves_cancels_it() {
            use crate::tui::app::Action;
            for action in ["seek", "next"] {
                let mut app = room(listing(true));
                main_playing(&mut app, &["a.mp3", "b.mp3"]);
                let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
                let effects = match action {
                    "seek" => app.seek_to(app.status.position + 5.0),
                    _ => app.handle_action(Action::NextTrack),
                };
                assert!(!effects.is_empty(), "{action}: the listener's command went out");
                app.note_pending(&effects);
                assert!(!app.peer_recs_clip_live(), "{action}: {effects:?}");
                let late = found(&mut app, ticket, "http://cdn/3.mp3");
                assert!(!clip_sent(&late), "{action}: nothing plays later");
            }
        }

        #[test]
        fn a_peers_padded_or_outsized_words_land_trimmed_and_bounded() {
            // The server caps a lead's words after trimming them and forwards them untrimmed: a
            // hostile peer's padding reaches the room. It is trimmed, then cut, once, as the
            // answer lands, so the grid, the band, the sheet and every key read short words.
            use crate::api::types::TasteLead;
            let pad = " ".repeat(1_000_000);
            let huge = "x".repeat(1_000_000);
            let mut taste = fixtures::taste("taste_all");
            let results = &mut taste.network.as_mut().expect("a network").results;
            let endpoint = results[0].peer.endpoint_id.clone().expect("a peer");
            let peer = json!({ "endpointId": endpoint, "name": format!("{pad}Kitchen"), "label": huge });
            let padded = |title: &str| format!("{pad}{title}{pad}");
            results[0] = TasteLead::from_raw(json!({
                "title": padded("Signal Fires"), "artist": huge, "album": padded("Night"), "peer": peer,
            }));
            results[1] = TasteLead::from_raw(json!({
                "title": padded("Copper Dawn"), "artist": huge, "album": padded("Night"), "peer": peer,
            }));
            results[2] = TasteLead::from_raw(json!({
                "title": "Glass", "exportId": padded("ex-7"), "peer": peer,
            }));
            let app = room_on(listing(true), taste);
            let leads = app.peer_recs_leads();
            let by = |title: &str| {
                let hit = leads.iter().find(|l| l.title.as_deref().is_some_and(|t| t.trim() == title));
                hit.unwrap_or_else(|| panic!("{title} is shown"))
            };
            let (signal, copper, glass) = (by("Signal Fires"), by("Copper Dawn"), by("Glass"));
            assert_eq!(signal.title.as_deref(), Some("Signal Fires"), "trimmed as it lands");
            assert_eq!(signal.album.as_deref(), Some("Night"));
            assert_eq!(signal.peer.name.as_deref(), Some("Kitchen"));
            let most = rules::PEER_TEXT_MOST;
            assert!(signal.artist.as_ref().is_some_and(|a| a.len() <= most), "the artist is cut");
            assert!(signal.peer.label.as_ref().is_some_and(|l| l.len() <= most), "the label is cut");
            assert_eq!(Lead::of(signal).title, "Signal Fires", "the band reads the words");
            assert_ne!(rules::lead_key(signal), rules::lead_key(copper), "two leads, two keys");
            assert_eq!(glass.export_id.as_deref(), Some("ex-7"));
            assert_eq!(rules::lead_key(glass), format!("{endpoint}\0ex-7"));
            assert_eq!(signal.raw["title"], json!(padded("Signal Fires")), "it goes back as sent");
        }

        #[test]
        fn a_lost_audio_thread_ends_the_clip_session_with_nothing_sent_to_it() {
            let aside = |effects: &[Effect]| {
                effects.iter().any(|e| {
                    matches!(
                        e,
                        Effect::Audio(
                            AudioCmd::AsidePlay { .. }
                                | AudioCmd::AsideStop { .. }
                                | AudioCmd::AsideHold { .. }
                        )
                    )
                })
            };
            let mut app = room(listing(true));
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            found(&mut app, ticket, "http://cdn/3.mp3");
            let status =
                AsideStatus { ticket, phase: AsidePhase::Playing, position: 4.0, duration: 30.0 };
            app.apply_event(Event::Aside(Some(status)));
            assert!(app.peer_recs_band().is_some());
            let effects = app.apply_event(Event::AudioFailed("the audio engine crashed".into()));
            assert!(!app.peer_recs_clip_live(), "the room's re-asks are not held for it");
            assert!(app.peer_recs_band().is_none());
            assert!(!aside(&effects), "{effects:?}");
            let ticked = app.tick_at(Instant::now());
            assert!(!aside(&ticked), "no stop is owed to a thread that is gone: {ticked:?}");
            // A press still finding its clip when it fell goes the same way.
            let mut app = room(listing(true));
            let (ticket, ..) = resolve_cmd(&app.peer_recs_press(2, Instant::now()));
            app.apply_event(Event::AudioFailed("the audio engine crashed".into()));
            assert!(!app.peer_recs_clip_live());
            assert!(!aside(&found(&mut app, ticket, "http://cdn/3.mp3")), "nothing plays later");
        }
    }
}
