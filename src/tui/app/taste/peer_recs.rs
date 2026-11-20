//! The GUI's Peer recs room, its App half (docs/ux-contracts/
//! peer-recommendations.md; mStream's docs/peer-recommendations-plan.md
//! §3.5 R1–R8 and §5.6).
//!
//! The room draws the p2p network's leads: songs the peers on this
//! server's shelf have that match your taste, asked through the
//! generation-3 taste route (`sources: ["network"]`). Nothing here plays a
//! lead; a lead has no file. The room previews a lead through the clip
//! session in [`previews`], copies it, opens it elsewhere, or gets it
//! through an acquire plug-in.
//!
//! The room keeps **its own state**, never Discover's: it embeds a
//! [`TasteState`](super::TasteState) of its own for the visit's steering
//! (window, pins, drops, passed picks, the latch and the seq), and the
//! batch lives in that state's `head`, so the bar's seeds modal reads it as
//! it reads Discover's. Discover's own
//! `App.taste` is never written here.
//!
//! The room's asks all go through one worker door, `ApiCmd::PeerRecs`,
//! answered by `Event::PeerRecs`: the taste ask, the peers poll,
//! an Add, the plug-in gate, a preview resolve, the sheet's lookup, job
//! read and job start, and R7's re-ping. A failure stays inside the room's
//! own event, so it never becomes `Event::Error`.
//!
//! The pure rules (request bodies, the strip, the add outcomes, the lead
//! key, the copy text) are in [`rules`]; this file holds the visit, the
//! fences and the cadences, and calls them.

pub(crate) mod previews;
pub(crate) mod rules;


use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::*;
use crate::api::PluginFailure;
use crate::api::peer_recs_door::{PeerRecsCmd, PeerRecsEvent, PeersFailure, RecsFailure};
use crate::api::types::{
    CollectionDestination, CollectionDestinationView, DiscoveryPluginsResponse, JobLookup, JobOutcome,
    JobScope, JobStarted, P2pPeersAnswer, PeerAddAnswer, PluginJob, TasteLead, TasteResponse,
};
use crate::clock::Instant;

/// The peers poll's beat (peer-recommendations plan §3.5 R6).
pub const PEER_POLL: Duration = rules::PEER_POLL;

/// The peers poll's beat while an Add is out or a row reads adding (R6).
pub const PEER_POLL_FAST: Duration = rules::PEER_POLL_FAST;

/// A keyboard tab step asks this long after the last step (R8).
pub const TAB_DEBOUNCE: Duration = Duration::from_millis(250);

/// The lead sheet's job read, while its job is live (peer-recs contract
/// §1.9): Keep's modal beat.
pub const JOB_POLL: Duration = crate::tui::app::keep::KEEP_POLL_MODAL;

/// What the GUI shows of the room, set every frame (the `KeepWatch`
/// pattern). The TUI never sets it, so it never polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerRecsWatch {
    /// The room is not the active room: the visit is over.
    #[default]
    Away,
    /// The room is drawn. The lead sheet, the window list and the seeds
    /// modal over it still count as shown: they are not a cover.
    Shown,
    /// The room is the active room but not drawn (another screen, the mini
    /// player, a seed pick's Files room): the poll pauses, the visit stays.
    Covered,
}

/// The peer strip as the tabs draw it (peer-recs contract §1.5).
#[derive(Debug, Clone, PartialEq)]
pub struct StripView {
    /// All, then the shelf, then an admin's addable rows, in the server's
    /// order.
    pub items: Vec<rules::StripItem>,
    /// The selected tab: `None` is All.
    pub selected: Option<String>,
    /// The strip is drawn: never before the first poll answers, and never
    /// when All would stand alone.
    pub drawn: bool,
    /// The first poll is still out: the `░` skeleton.
    pub skeleton: bool,
}

/// One Get it row of the lead sheet (peer-recs contract §1.9).
#[derive(Debug, Clone, PartialEq)]
pub struct GetItRow {
    pub plugin: String,
    /// The plug-in's title, only when several plug-ins each get a row.
    pub title: Option<String>,
    /// Where the newest job of this plug-in for the lead stands; `None`
    /// before any.
    pub mark: Option<JobOutcome>,
}

/// Who asked R7's re-ping (peer-recommendations plan §3.5 R7): what an
/// "on" answer leads to depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reprobe {
    /// A taste 403: on is the scan-pending pane.
    Taste,
    /// A non-random 200 with no network section: on is Try again.
    NetworkNull,
    /// A peers 403: on keeps the room, the poll stopped.
    Peers,
    /// A preview resolve 403 (the previews' own): on changes nothing more.
    Clip,
}

/// When the next ask of a cadence is due. An answer lands with no clock
/// (an event carries none), so it schedules a wait, and the next tick
/// starts that wait's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    In(Duration),
    At(Instant),
}

/// Whether the ask `slot` holds has come at `now`, its clock started now
/// when it had none.
fn come(slot: &mut Option<Due>, now: Instant) -> bool {
    let at = match *slot {
        None => return false,
        Some(Due::In(wait)) => {
            let at = now + wait;
            *slot = Some(Due::At(at));
            at
        }
        Some(Due::At(at)) => at,
    };
    now >= at
}

/// The peers poll (R6; U/api.md §4.3).
#[derive(Debug, Clone, Default)]
struct PeersPoll {
    /// The last applied answer: the strip, and `online` for the sheet.
    view: Option<P2pPeersAnswer>,
    /// Polls sent: a local Add state stands until a poll sent after it
    /// answers. Never reset in a session, so a late answer from a visit
    /// before is older than every poll of this one.
    sent: u64,
    /// The newest poll applied; an older one answering late is dropped.
    answered: u64,
    /// The poll out: one at a time.
    asking: Option<u64>,
    next: Option<Due>,
    /// Covered while the poll ran: one at once on return.
    owed: bool,
    /// A 403, a 404, off, or the visit left.
    stopped: bool,
    /// A 404: an older server's, All alone.
    no_route: bool,
}

/// The plug-in gate (peer-recs contract §1.9 and §1.8).
#[derive(Debug, Clone, Default)]
struct RoomGate {
    /// The newest read sent; an older answer fills nothing.
    asked: u64,
    plugins: Option<DiscoveryPluginsResponse>,
    /// Where a Get it lands; `None` hides Get it (uploads off, no library).
    destination: Option<CollectionDestination>,
}

/// The lead sheet's own job state (peer-recs contract §1.9), keyed by its
/// lead's key, so a
/// reshuffle that moves the lead keeps it (D30).
#[derive(Debug, Clone, Default)]
struct LeadSheet {
    key: String,
    /// The one-row lookup asked: only its answer fills the sheet.
    lookup: Option<u64>,
    /// The lookup was sent, on open or when Get it's gate came to hold:
    /// once per sheet.
    looked_up: bool,
    /// The newest job per Get it plug-in.
    jobs: BTreeMap<String, PluginJob>,
    /// A start the server refused, per plug-in: the row reads failed, and
    /// Enter starts again.
    refused: BTreeMap<String, String>,
    /// The newest job read: only its answer lands.
    read: Option<u64>,
    /// That read is out: one at a time.
    read_out: bool,
    next: Option<Due>,
    /// Which live job the next read asks: they take turns.
    turn: usize,
    /// The newest start, and its plug-in: only its answer lands.
    start: Option<(u64, String)>,
    /// That start is out: its row waits, and Get it presses nothing.
    start_out: bool,
}

/// R7's re-ping out: who asked, in which visit, over which taste ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Probe {
    source: Reprobe,
    visit: u64,
    ask: u64,
}

/// Everything the room holds. One visit at a time (peer-recs contract §1.1.7):
/// the batch, the tab, every tab's kept answer, the local Add states, the
/// posted ids, the preview answers and the seeds-modal edits.
#[derive(Debug, Default)]
pub struct PeerRecsState {
    /// The visit's steering, as Discover's: window, pins, drops, passed
    /// picks, the latch and the seq. Its `head` is the batch: the first
    /// answer of the batch, its network list emptied. Its
    /// library and peers slots stay unused.
    pub taste: TasteState,
    pub preview: previews::PreviewState,
    pub watch: PeerRecsWatch,
    /// A seed pick armed from the room's seeds modal is out (D32).
    pub picking: bool,
    /// The session generation the state belongs to.
    generation: u64,
    /// The visit fence: bumped on open, leave and reset.
    visit: u64,
    /// A visit is open.
    open: bool,
    /// The selected tab's answer on screen.
    shown: Option<TasteResponse>,
    /// Each tab's answer for the batch: "" is All, else an endpoint id.
    answers: BTreeMap<String, TasteResponse>,
    /// The last taste ask: the last ask wins (R8).
    ask_seq: u64,
    /// That ask is a new batch (R1), not one pinned to the batch (R2).
    ask_new: bool,
    loading: bool,
    /// The last ask got no answer: the Try again pane.
    failed: bool,
    selected: Option<String>,
    /// R8's keyboard debounce: the step's ask is due then.
    select_due: Option<Instant>,
    poll: PeersPoll,
    local_adds: BTreeMap<String, rules::LocalAdd>,
    /// This visit's own Adds still on their way: a landing re-asks (R4).
    posted: BTreeSet<String>,
    /// Add POSTs out.
    posting: BTreeSet<String>,
    /// An R4 re-ask waits for holding to end.
    pending_reask: bool,
    gate: RoomGate,
    sheet: Option<LeadSheet>,
    probing: Option<Probe>,
    /// The gate's, the sheet's lookups', job reads' and starts' seqs.
    seq: u64,
}

impl PeerRecsState {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// A tab's key in the kept answers: "" is All.
fn tab_key(tab: Option<&str>) -> String {
    tab.unwrap_or_default().to_string()
}

impl App {
    // ── The gate and the facts the GUI draws ────────────────────────────────

    /// The room's nav gate (peer-recs contract §1.1, D31): the server half, the room's own
    /// rule (never a peer session) and R7's latch. No `connected` term, as
    /// Discover's has none: a short disconnect keeps the row.
    pub fn peer_recs_gate(&self) -> bool {
        self.capabilities.peer_recs()
            && self.session.peer.is_none()
            && self.peer_recs.taste.latch != TasteLatch::Off
    }

    /// A seed pick armed from the room is out (D32).
    pub fn peer_recs_picking(&self) -> bool {
        self.peer_recs.picking && self.capture == Some(Capture::TasteSeed)
    }

    /// The visit fence, for the previews.
    pub fn peer_recs_visit(&self) -> u64 {
        self.peer_recs.visit
    }

    /// A visit is open: the room's asks may go.
    pub fn peer_recs_visit_open(&self) -> bool {
        self.peer_recs.open
    }

    /// The gate's preview plug-in (peer-recommendations plan §5.5): the
    /// first listed that resolves previews, no fall-through.
    pub fn peer_recs_provider(&self) -> Option<&str> {
        self.peer_recs.gate.plugins.as_ref()?.preview_plugin().map(|p| p.name.as_str())
    }

    /// Where the lead whose key is `key` stands in the shown list.
    pub fn peer_recs_lead_index(&self, key: &str) -> Option<usize> {
        self.peer_recs_leads().iter().position(|lead| rules::lead_key(lead) == key)
    }

    /// The strip (peer-recs contract §1.5).
    pub fn peer_recs_strip(&self) -> StripView {
        let r = &self.peer_recs;
        let items = rules::strip_items(r.poll.view.as_ref(), &r.local_adds, r.poll.answered);
        let drawn = r.poll.view.is_some() && rules::strip_drawn(&items);
        let skeleton = r.open && r.poll.view.is_none() && !r.poll.no_route && !r.poll.stopped;
        StripView { items, selected: r.selected.clone(), drawn, skeleton }
    }

    /// The selected tab's leads, in rank order.
    pub fn peer_recs_leads(&self) -> &[TasteLead] {
        let network = self.peer_recs.shown.as_ref().and_then(|shown| shown.network.as_ref());
        network.map(|network| network.results.as_slice()).unwrap_or(&[])
    }

    /// What the list rect draws (peer-recs contract §1.10).
    pub fn peer_recs_pane(&self) -> rules::RecsPane {
        let r = &self.peer_recs;
        let scan_latch = match r.taste.latch {
            TasteLatch::ScanPending | TasteLatch::Probing => true,
            TasteLatch::Clear | TasteLatch::Off => false,
        };
        let searchable = r.poll.view.as_ref().map(|v| v.shelf.searchable);
        let asking = r.loading || r.select_due.is_some();
        rules::pane_of(r.shown.as_ref(), asking, r.failed, scan_latch, searchable)
    }

    /// The header's `↻ asking…`: a taste ask out or due, or a re-ping that
    /// decides what the list draws.
    pub fn peer_recs_asking(&self) -> bool {
        let r = &self.peer_recs;
        r.loading
            || r.select_due.is_some()
            || r.probing.is_some_and(|p| matches!(p.source, Reprobe::Taste | Reprobe::NetworkNull))
    }

    /// The window the header's list shows, as Discover's (D15).
    pub fn peer_recs_window_shown(&self) -> Option<u32> {
        self.peer_recs.taste.shown_window()
    }

    /// The latest poll says this peer is online: the sheet's `• online`.
    pub fn peer_recs_online(&self, endpoint_id: &str) -> bool {
        self.peer_recs.poll.view.as_ref().and_then(|v| v.row(endpoint_id)).is_some_and(|row| row.online)
    }

    /// Any plug-ins at all: without them Enter copies instead of opening
    /// the sheet (peer-recs contract §1.9).
    pub fn peer_recs_plugins_on(&self) -> bool {
        self.capabilities.discovery_plugins
    }

    /// Open elsewhere…'s gate: a links plug-in in the room's listing.
    pub fn peer_recs_links_on(&self) -> bool {
        self.connected
            && self.capabilities.discovery_plugins
            && self.session.peer.is_none()
            && self.peer_recs.gate.plugins.as_ref().is_some_and(|p| p.links_plugin().is_some())
    }

    /// The copy text of the lead at `index` (peer-recommendations plan §5.7).
    pub fn peer_recs_copy_text(&self, index: usize) -> Option<String> {
        self.peer_recs_leads().get(index).and_then(rules::copy_text)
    }

    /// Copy list: the tab's leads in rank order.
    pub fn peer_recs_copy_list(&self) -> Option<String> {
        rules::copy_list(self.peer_recs_leads())
    }

    /// Get it's gate (peer-recs contract §1.9): jobs allowed, a plug-in other than the copy
    /// plug-in, and a destination. The band's `[ get it ]` reads it with no sheet open.
    pub fn peer_recs_get_it_on(&self) -> bool {
        let gate = &self.peer_recs.gate;
        self.connected
            && self.capabilities.discovery_plugins
            && self.session.peer.is_none()
            && gate.destination.is_some()
            && gate.plugins.as_ref().is_some_and(|p| p.jobs.allowed && !p.get_it_plugins().is_empty())
    }

    /// The sheet's Get it rows, one per acquire plug-in (DS §8.6), each
    /// with its job's word. None without the gate or a sheet.
    pub fn peer_recs_get_it_rows(&self) -> Vec<GetItRow> {
        let Some(sheet) = self.peer_recs.sheet.as_ref() else { return Vec::new() };
        if !self.peer_recs_get_it_on() {
            return Vec::new();
        }
        let Some(plugins) = self.peer_recs.gate.plugins.as_ref() else { return Vec::new() };
        let listed = plugins.get_it_plugins();
        let several = listed.len() > 1;
        listed
            .into_iter()
            .map(|p| {
                let waiting =
                    sheet.start_out && sheet.start.as_ref().is_some_and(|(_, name)| *name == p.name);
                let mark = if waiting {
                    Some(JobOutcome::Queued)
                } else if let Some(words) = sheet.refused.get(&p.name) {
                    Some(JobOutcome::Failed { error: Some(words.clone()), many: None })
                } else {
                    sheet.jobs.get(&p.name).map(PluginJob::outcome)
                };
                let titled = p.title.clone().filter(|t| !t.trim().is_empty());
                let title = several.then(|| titled.unwrap_or(p.name.clone()));
                GetItRow { plugin: p.name.clone(), title, mark }
            })
            .collect()
    }

    // ── The visit ───────────────────────────────────────────────────────────

    /// A fresh visit (peer-recs contract §1.1.7): every kept thing of the
    /// last one goes, and R1, the first poll and the plug-in gate are asked
    /// together. `window` is `[gui] discover_window`, shared with Discover
    /// (D15). No auto-pin of the playing song (D16, TD's rule).
    pub(crate) fn peer_recs_open(&mut self, window: Option<u32>) -> Vec<Effect> {
        self.peer_recs_sync();
        let mut effects = self.peer_recs_preview_reset();
        let r = &mut self.peer_recs;
        r.visit += 1;
        r.open = true;
        r.watch = PeerRecsWatch::Shown;
        r.taste.new_visit();
        r.taste.head = None;
        if let Some(window) = window.and_then(taste_window) {
            r.taste.window = Some(window);
        }
        // A scan-pending verdict was the last visit's; a probe still out
        // keeps its say (Discover's rule, clause 82).
        if r.taste.latch == TasteLatch::ScanPending {
            r.taste.latch = TasteLatch::Clear;
        }
        r.shown = None;
        r.answers.clear();
        r.loading = false;
        r.taste.main_pending = false;
        r.failed = false;
        r.selected = None;
        r.select_due = None;
        r.local_adds.clear();
        r.posted.clear();
        r.posting.clear();
        r.pending_reask = false;
        r.sheet = None;
        r.poll = PeersPoll { sent: r.poll.sent, answered: r.poll.sent, ..PeersPoll::default() };
        r.ask_seq = r.taste.next_seq();
        if !self.peer_recs_gate() {
            return effects;
        }
        effects.extend(self.peer_recs_ask_new());
        effects.extend(self.peer_recs_poll_now());
        effects.extend(self.peer_recs_regate());
        effects
    }

    /// The visit ends (peer-recs contract §1.1.7): the poll stops, a live
    /// clip ends with main restored, and every answer still out is
    /// dropped by the visit fence.
    fn peer_recs_leave(&mut self) -> Vec<Effect> {
        let effects = self.peer_recs_preview_reset();
        let r = &mut self.peer_recs;
        if !r.open {
            return effects;
        }
        r.open = false;
        r.visit += 1;
        r.ask_seq = r.taste.next_seq();
        r.loading = false;
        r.taste.main_pending = false;
        r.select_due = None;
        r.poll.stopped = true;
        r.poll.asking = None;
        r.poll.next = None;
        r.sheet = None;
        r.pending_reask = false;
        effects
    }

    /// The GUI's watch (the `KeepWatch` pattern), set every frame: a clip
    /// ends with main restored on leaving Shown; Covered pauses the poll
    /// (one at once on return); Away ends the visit.
    pub(crate) fn peer_recs_watch(&mut self, watch: PeerRecsWatch) -> Vec<Effect> {
        let was = self.peer_recs.watch;
        if was == watch {
            return Vec::new();
        }
        self.peer_recs.watch = watch;
        match watch {
            PeerRecsWatch::Away => self.peer_recs_leave(),
            PeerRecsWatch::Covered => {
                self.peer_recs.poll.owed = true;
                if was == PeerRecsWatch::Shown && self.peer_recs_clip_live() {
                    return self.peer_recs_clip_stop(true);
                }
                Vec::new()
            }
            PeerRecsWatch::Shown => {
                let poll = &mut self.peer_recs.poll;
                if std::mem::take(&mut poll.owed) && !poll.stopped {
                    poll.next = Some(Due::In(Duration::ZERO));
                }
                Vec::new()
            }
        }
    }

    /// The session moved to another server (`shed_server_state`): a
    /// fresh, closed visit. A live clip's stop is owed to the next tick,
    /// with main restored (D10).
    pub(crate) fn reset_peer_recs(&mut self) {
        // The previews forget everything and owe the live clip's stop to
        // their next tick; their tickets run on, so an engine end still
        // out never matches a clip of the new server's.
        self.peer_recs_preview_shed();
        let r = &mut self.peer_recs;
        let mut taste = std::mem::take(&mut r.taste);
        taste.reset();
        let fresh = PeerRecsState {
            taste,
            preview: std::mem::take(&mut r.preview),
            watch: r.watch,
            generation: self.session_gen,
            visit: r.visit + 1,
            ..PeerRecsState::default()
        };
        *r = fresh;
    }

    /// The state belongs to the session it was filled under.
    fn peer_recs_sync(&mut self) {
        if self.peer_recs.generation != self.session_gen {
            self.reset_peer_recs();
        }
    }

    /// A fresh ping is the word on the gate now: a room switched off by
    /// R7 shows its row again if the flags are back; a re-ping nothing
    /// will answer is let go.
    pub(crate) fn peer_recs_reconnected(&mut self) {
        let r = &mut self.peer_recs;
        if r.taste.latch == TasteLatch::Off {
            r.taste.latch = TasteLatch::Clear;
        }
        if r.probing.take().is_some() && r.taste.latch == TasteLatch::Probing {
            r.taste.latch = TasteLatch::Clear;
        }
    }

    /// An error cleared every wait (`clear_pending`, which any
    /// `Event::Error` runs, a playlist rename's included; a door ask with
    /// no session answers one): only the asking flags go — the taste
    /// ask's, the poll in flight (its cadence resumes), the sheet's job
    /// read and start, and the previews' resolve flight — so the next ask
    /// can go. No fence moves, no clip ends and no answer is dropped: a
    /// late answer whose seq, poll number or ticket is still the current
    /// one lands.
    pub(crate) fn clear_peer_recs_waits(&mut self) {
        let r = &mut self.peer_recs;
        r.loading = false;
        r.taste.main_pending = false;
        r.poll.asking = None;
        if r.open && !r.poll.stopped && r.poll.next.is_none() {
            r.poll.next = Some(Due::In(PEER_POLL));
        }
        if let Some(sheet) = r.sheet.as_mut() {
            sheet.read_out = false;
            sheet.start_out = false;
        }
        r.preview.clear_waits();
    }

    // ── Asking (R1–R3, R8) ──────────────────────────────────────────────────

    /// Whether a taste ask may go: a visit open on a connected, gated
    /// session, and no 403 verdict standing or out.
    fn peer_recs_may_ask(&self) -> bool {
        self.peer_recs.open
            && self.connected
            && self.peer_recs_gate()
            && self.peer_recs.taste.latch == TasteLatch::Clear
    }

    fn peer_recs_send(&self, cmd: PeerRecsCmd) -> Effect {
        Effect::Api(ApiCmd::PeerRecs { generation: self.session_gen, cmd })
    }

    /// R1: a new batch for the selected tab. The batch and every tab's
    /// kept answer go; the old rows stand until the new land.
    fn peer_recs_ask_new(&mut self) -> Vec<Effect> {
        if !self.peer_recs_may_ask() {
            return Vec::new();
        }
        let r = &mut self.peer_recs;
        let pins: Vec<String> = r.taste.pins.iter().map(|pin| pin.track.filepath.clone()).collect();
        let window = r.taste.window.unwrap_or(TASTE_WINDOW_DEFAULT);
        let body = rules::r1_body(window, &pins, &r.taste.exclusions(), r.selected.as_deref());
        r.taste.head = None;
        r.answers.clear();
        let seq = r.taste.next_seq();
        r.ask_seq = seq;
        r.ask_new = true;
        r.taste.main_seq = seq;
        r.taste.main_pending = true;
        r.taste.main_pinned = !pins.is_empty();
        r.loading = true;
        r.failed = false;
        vec![self.peer_recs_send(PeerRecsCmd::Taste { seq, body: Box::new(body) })]
    }

    /// R2: the selected tab on the batch on screen; R1 when there is no
    /// pinnable batch.
    fn peer_recs_ask_tab(&mut self) -> Vec<Effect> {
        if !self.peer_recs_may_ask() {
            return Vec::new();
        }
        let r = &mut self.peer_recs;
        let window = r.taste.window.unwrap_or(TASTE_WINDOW_DEFAULT);
        let exclude = r.taste.exclusions();
        let tab = r.selected.as_deref();
        let body = r.taste.head.as_ref().and_then(|head| rules::r2_body(head, window, &exclude, tab));
        let Some(body) = body else { return self.peer_recs_ask_new() };
        let seq = r.taste.next_seq();
        r.ask_seq = seq;
        r.ask_new = false;
        r.taste.main_pending = false;
        r.loading = true;
        r.failed = false;
        vec![self.peer_recs_send(PeerRecsCmd::Taste { seq, body: Box::new(body) })]
    }

    /// The selected tab's kept answer for this batch, drawn with no call
    /// (an ask still out for another tab is dropped), else R2 (R2, R5).
    fn peer_recs_show_or_ask(&mut self) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        let key = tab_key(r.selected.as_deref());
        let kept = r.taste.head.as_ref().and_then(|_| r.answers.get(&key).cloned());
        match kept {
            Some(answer) => {
                r.ask_seq = r.taste.next_seq();
                r.loading = false;
                r.taste.main_pending = false;
                self.peer_recs_show(Some(answer));
                Vec::new()
            }
            None => self.peer_recs_ask_tab(),
        }
    }

    /// An answer drawn: the list changed under a Preview all chain.
    fn peer_recs_show(&mut self, answer: Option<TasteResponse>) {
        let r = &mut self.peer_recs;
        r.failed = answer.is_none();
        r.shown = answer;
        self.peer_recs_list_changed();
    }

    /// R3, New seeds (`R`, `↻ new seeds`, the nav row while up): a new
    /// batch keeping the tab. On a random batch its pick is passed over
    /// first, so it goes into `exclude`. Nothing while an ask is out.
    pub(crate) fn peer_recs_new_seeds(&mut self) -> Vec<Effect> {
        self.peer_recs_sync();
        let r = &mut self.peer_recs;
        if !r.open || r.loading {
            return Vec::new();
        }
        if let Some(head) = r.taste.head.as_ref().filter(|head| rules::is_random(head)) {
            let picks: Vec<String> = head
                .seeds
                .iter()
                .filter(|seed| !seed.pinned && !seed.hash.is_empty())
                .map(|seed| seed.hash.clone())
                .collect();
            for hash in picks {
                r.taste.skipped.retain(|h| *h != hash);
                r.taste.skipped.push(hash);
            }
        }
        r.select_due = None;
        self.peer_recs_ask_new()
    }

    /// A tab clicked or chosen on the tabs stop (R2): at once. `None` is
    /// All. Only All and the shelf's tabs are selectable.
    pub(crate) fn peer_recs_select(&mut self, id: Option<String>) -> Vec<Effect> {
        self.peer_recs_sync();
        if !self.peer_recs.open {
            return Vec::new();
        }
        let id = id.map(|id| id.to_ascii_lowercase());
        if let Some(id) = id.as_deref() {
            let items = self.peer_recs_strip().items;
            if !items.iter().any(|item| item.id.as_deref() == Some(id) && item.selectable()) {
                return Vec::new();
            }
        }
        let r = &mut self.peer_recs;
        r.selected = id;
        r.select_due = None;
        self.peer_recs_show_or_ask()
    }

    /// Stowed ← →: the selected tab moves at once, skipping the add tabs,
    /// and stops at the ends. A kept answer draws at once; else the ask
    /// goes 250 ms after the last step (R8).
    pub(crate) fn peer_recs_step(&mut self, dir: i32, now: Instant) -> Vec<Effect> {
        self.peer_recs_sync();
        if !self.peer_recs.open {
            return Vec::new();
        }
        let items = self.peer_recs_strip().items;
        let Some(next) = rules::step_tab(&items, self.peer_recs.selected.as_deref(), dir) else {
            return Vec::new();
        };
        let r = &mut self.peer_recs;
        r.selected = next;
        let kept = r.taste.head.is_some() && r.answers.contains_key(&tab_key(r.selected.as_deref()));
        if kept {
            r.select_due = None;
            return self.peer_recs_show_or_ask();
        }
        // The ask still out was for another tab: it is dropped now.
        r.ask_seq = r.taste.next_seq();
        r.loading = false;
        r.select_due = Some(now + TAB_DEBOUNCE);
        Vec::new()
    }

    /// The Try again pane: the tab on the batch, else a new batch.
    pub(crate) fn peer_recs_retry(&mut self) -> Vec<Effect> {
        self.peer_recs_sync();
        self.peer_recs.select_due = None;
        self.peer_recs_ask_tab()
    }

    /// The scan-pending pane's Check again (D17; Discover's clause 81, its
    /// `taste_ask_again`): the 403 verdict goes and a new batch is asked,
    /// keeping the tab and the visit's steering; a 403 again re-pings
    /// again. Nothing while a re-ping is out.
    pub(crate) fn peer_recs_check_again(&mut self) -> Vec<Effect> {
        self.peer_recs_sync();
        let r = &mut self.peer_recs;
        if !r.open || r.taste.latch == TasteLatch::Probing || r.taste.latch == TasteLatch::Off {
            return Vec::new();
        }
        r.taste.latch = TasteLatch::Clear;
        r.select_due = None;
        self.peer_recs_ask_new()
    }

    /// The header's window list (D15): a new batch with the window,
    /// keeping the tab. The GUI keeps it in `[gui] discover_window`; a
    /// failed ask keeps it (TD's rule).
    pub(crate) fn peer_recs_set_window(&mut self, window: u32) -> Vec<Effect> {
        self.peer_recs_sync();
        if taste_window(window).is_none() {
            return Vec::new();
        }
        let t = &self.peer_recs.taste;
        if t.shown_window() == Some(window) && t.window.unwrap_or(TASTE_WINDOW_DEFAULT) == window {
            return Vec::new();
        }
        self.peer_recs.taste.window = Some(window);
        self.peer_recs.select_due = None;
        self.peer_recs_ask_new()
    }

    /// The seeds modal closed (Discover's `taste_seeds_closed`, on the
    /// room's own steering): a seed edit asks a new batch, keeping the tab.
    pub(crate) fn peer_recs_seeds_closed(
        &mut self,
        dropped: &[String],
        restored: &[String],
        pins_changed: bool,
    ) -> Vec<Effect> {
        self.peer_recs_sync();
        let t = &mut self.peer_recs.taste;
        let mut changed = pins_changed;
        for hash in dropped {
            if !t.dropped.contains(hash) && !restored.contains(hash) {
                t.dropped.push(hash.clone());
                changed = true;
            }
        }
        let before = t.dropped.len();
        t.dropped.retain(|hash| !restored.contains(hash));
        changed |= t.dropped.len() != before;
        if !changed {
            return Vec::new();
        }
        self.peer_recs.select_due = None;
        self.peer_recs_ask_new()
    }

    /// "Pin the playing song" in the room's seeds modal (clause 14's rule,
    /// on the room's pins).
    pub fn peer_recs_can_pin_playing(&self) -> bool {
        self.can_pin_playing_in(&self.peer_recs.taste)
    }

    pub(crate) fn peer_recs_pin_playing(&mut self) -> bool {
        if !self.peer_recs_can_pin_playing() {
            return false;
        }
        let playing = self.queue.current.and_then(|i| self.queue.items.get(i));
        let Some(track) = playing.map(|q| q.track.clone()) else { return false };
        self.peer_recs.taste.pins.push(TastePin { track });
        true
    }

    pub(crate) fn peer_recs_unpin(&mut self, filepath: &str) -> bool {
        let pins = &mut self.peer_recs.taste.pins;
        let before = pins.len();
        pins.retain(|pin| pin.track.filepath != filepath);
        pins.len() != before
    }

    /// "Pin a song…" in the room's seeds modal: the room's own pick (D32).
    /// The pick that lands asks R3 ([`App::peer_recs_seed_picked`]).
    pub(crate) fn peer_recs_choose_song(&mut self) -> Vec<Effect> {
        self.peer_recs.taste.refused = None;
        self.peer_recs.picking = true;
        self.arm_capture(Capture::TasteSeed)
    }

    /// The room's seed pick landed (D32): a pin, then a new batch keeping
    /// the tab; a refusal stays in the room for the GUI to word.
    pub(super) fn peer_recs_seed_picked(&mut self, track: Track) -> Vec<Effect> {
        self.peer_recs.picking = false;
        if let Some(refusal) = self.pin_refusal(&self.peer_recs.taste, &track) {
            self.peer_recs.taste.refused = Some(refusal);
            return Vec::new();
        }
        self.peer_recs.taste.pins.push(TastePin { track });
        self.peer_recs.select_due = None;
        self.peer_recs_ask_new()
    }

    // ── Adds (peer-recommendations plan §3.3) ───────────────────────────────

    /// `+ {label}` or `{label} try again`: adding at once and the poll at
    /// 3 s, then the POST. Only an add tab whose press Adds, and not one
    /// already posting.
    pub(crate) fn peer_recs_add(&mut self, endpoint_id: &str) -> Vec<Effect> {
        self.peer_recs_sync();
        if !self.peer_recs.open || !self.connected || self.peer_recs.posting.contains(endpoint_id) {
            return Vec::new();
        }
        let items = self.peer_recs_strip().items;
        let adds = items.iter().any(|item| {
            item.id.as_deref() == Some(endpoint_id)
                && matches!(item.kind, rules::StripKind::Addable | rules::StripKind::TryAgain)
        });
        if !adds {
            return Vec::new();
        }
        let r = &mut self.peer_recs;
        let stamp = r.poll.sent;
        let adding = rules::LocalAdd { kind: rules::LocalAddKind::Adding, stamp };
        r.local_adds.insert(endpoint_id.to_string(), adding);
        r.posted.insert(endpoint_id.to_string());
        r.posting.insert(endpoint_id.to_string());
        if !r.poll.stopped && r.poll.asking.is_none() {
            r.poll.next = Some(Due::In(PEER_POLL_FAST));
        }
        vec![self.peer_recs_send(PeerRecsCmd::Add { endpoint_id: endpoint_id.to_string() })]
    }

    // ── R7 and the plug-in gate ─────────────────────────────────────────────

    /// R7's re-ping: one at a time (a second trigger while one is out
    /// sends nothing, and the ping out answers for it). A taste 403's
    /// holds the asks until a ping answers; a null network with a ping
    /// already out is Try again at once (the phone's `_answer`).
    pub(crate) fn peer_recs_reprobe(&mut self, source: Reprobe) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        if r.taste.latch == TasteLatch::Off || !self.connected {
            return Vec::new();
        }
        if source == Reprobe::Taste {
            r.taste.latch = TasteLatch::Probing;
        }
        if r.probing.is_some() {
            if source == Reprobe::NetworkNull {
                self.peer_recs_show(None);
            }
            return Vec::new();
        }
        r.probing = Some(Probe { source, visit: r.visit, ask: r.ask_seq });
        vec![self.peer_recs_send(PeerRecsCmd::Probe)]
    }

    /// Read the plug-in gate again: the listing, then the destination
    /// behind Get it. Asked whatever the ping's plug-in flag says: the
    /// listing is the word on the preview provider, and a server without
    /// one answers a failure that stays in the room's event.
    pub(crate) fn peer_recs_regate(&mut self) -> Vec<Effect> {
        if !self.connected || self.session.peer.is_some() {
            return Vec::new();
        }
        let seq = self.peer_recs.next_seq();
        self.peer_recs.gate.asked = seq;
        vec![self.peer_recs_send(PeerRecsCmd::Gate { seq })]
    }

    // ── The lead sheet (peer-recs contract §1.9) ────────────────────────────

    /// A lead's sheet opened: its one-row lookup, when Get it is offered.
    /// The state is keyed by the lead's key (D30): a sheet re-opened on the
    /// same lead keeps it.
    pub(crate) fn peer_recs_sheet_open(&mut self, index: usize) -> Vec<Effect> {
        self.peer_recs_sync();
        let Some(lead) = self.peer_recs_leads().get(index) else { return Vec::new() };
        let key = rules::lead_key(lead);
        if self.peer_recs.sheet.as_ref().is_some_and(|sheet| sheet.key == key) {
            return Vec::new();
        }
        self.peer_recs.sheet = Some(LeadSheet { key, ..LeadSheet::default() });
        self.peer_recs_sheet_lookup()
    }

    /// The open sheet's one-row lookup (peer-recs contract §1.9), once per
    /// sheet and only while Get it is offered: on open, else when Get it's
    /// gate comes to hold under a sheet opened before it did.
    fn peer_recs_sheet_lookup(&mut self) -> Vec<Effect> {
        let unasked = self.peer_recs.sheet.as_ref().filter(|s| !s.looked_up);
        let Some(key) = unasked.map(|s| s.key.clone()) else { return Vec::new() };
        if !self.peer_recs_get_it_on() {
            return Vec::new();
        }
        let Some(index) = self.peer_recs_lead_index(&key) else { return Vec::new() };
        let rec = self.peer_recs_leads()[index].raw.clone();
        let seq = self.peer_recs.next_seq();
        if let Some(sheet) = self.peer_recs.sheet.as_mut() {
            sheet.lookup = Some(seq);
            sheet.looked_up = true;
        }
        vec![self.peer_recs_send(PeerRecsCmd::Lookup { seq, rec: Box::new(rec) })]
    }

    /// The sheet closed: its job runs on; a held R4 re-ask runs now.
    pub(crate) fn peer_recs_sheet_close(&mut self) -> Vec<Effect> {
        if self.peer_recs.sheet.take().is_none() {
            return Vec::new();
        }
        self.peer_recs_release()
    }

    /// Get it on row `row` (peer-recs contract §1.9): the job starts at once. Inert while
    /// its job is queued or running, and after it kept the song.
    pub(crate) fn peer_recs_get_it(&mut self, row: usize) -> Vec<Effect> {
        let Some(found) = self.peer_recs_get_it_rows().into_iter().nth(row) else { return Vec::new() };
        let again = matches!(
            found.mark,
            None | Some(
                JobOutcome::Failed { .. }
                    | JobOutcome::Stopped { .. }
                    | JobOutcome::Cancelled { .. }
                    | JobOutcome::Refused { .. }
            )
        );
        // One start out at a time, whichever row it is for.
        if !again || self.peer_recs.sheet.as_ref().is_none_or(|s| s.start_out) {
            return Vec::new();
        }
        let Some(key) = self.peer_recs.sheet.as_ref().map(|s| s.key.clone()) else { return Vec::new() };
        let Some(index) = self.peer_recs_lead_index(&key) else { return Vec::new() };
        let rec = self.peer_recs_leads()[index].raw.clone();
        let destination = self.peer_recs.gate.destination.clone();
        let seq = self.peer_recs.next_seq();
        let Some(sheet) = self.peer_recs.sheet.as_mut() else { return Vec::new() };
        sheet.refused.remove(&found.plugin);
        sheet.start = Some((seq, found.plugin.clone()));
        sheet.start_out = true;
        let cmd = PeerRecsCmd::Start { seq, plugin: found.plugin, rec: Box::new(rec), destination };
        vec![self.peer_recs_send(cmd)]
    }

    /// Open elsewhere… (peer-recs contract §1.9): Keep's links list, fed the
    /// raw lead.
    pub(crate) fn peer_recs_open_elsewhere(&mut self, index: usize) -> Vec<Effect> {
        if !self.peer_recs_links_on() {
            return Vec::new();
        }
        let links = self.peer_recs.gate.plugins.as_ref().and_then(|p| p.links_plugin());
        let Some(plugin) = links.map(|p| p.name.clone()) else { return Vec::new() };
        let Some(lead) = self.peer_recs_leads().get(index) else { return Vec::new() };
        let rec = lead.raw.clone();
        self.links_open_rec(&plugin, rec)
    }

    // ── Holding (R4) ────────────────────────────────────────────────────────

    /// A clip is live or the sheet is open: an R4 re-ask waits.
    fn peer_recs_holding(&self) -> bool {
        self.peer_recs.sheet.is_some() || self.peer_recs.preview.live()
    }

    /// R4's re-ask: the tab on the batch, once nothing holds the list.
    fn peer_recs_reask(&mut self) -> Vec<Effect> {
        if self.peer_recs_holding() {
            self.peer_recs.pending_reask = true;
            return Vec::new();
        }
        self.peer_recs.pending_reask = false;
        self.peer_recs.answers.clear();
        self.peer_recs_ask_tab()
    }

    /// Holding may have ended: a held re-ask runs at once
    /// (peer-recommendations plan §3.5 R4, the phone's rule).
    fn peer_recs_release(&mut self) -> Vec<Effect> {
        if self.peer_recs.open && self.peer_recs.pending_reask && !self.peer_recs_holding() {
            return self.peer_recs_reask();
        }
        Vec::new()
    }

    // ── The cadences ────────────────────────────────────────────────────────

    /// Once a tick: an owed stop, the keyboard debounce, a held re-ask
    /// whose hold ended, the poll, the sheet's job read, then the
    /// previews' own tick.
    pub(crate) fn peer_recs_tick(&mut self, now: Instant) -> Vec<Effect> {
        self.peer_recs_sync();
        let mut effects = Vec::new();
        // A pick let go some other way (a cancel) ends the room's.
        if self.peer_recs.picking && self.capture != Some(Capture::TasteSeed) {
            self.peer_recs.picking = false;
        }
        if self.peer_recs.open {
            if self.peer_recs.select_due.is_some_and(|due| now >= due) {
                self.peer_recs.select_due = None;
                effects.extend(self.peer_recs_show_or_ask());
            }
            effects.extend(self.peer_recs_release());
            effects.extend(self.peer_recs_poll_tick(now));
            effects.extend(self.peer_recs_job_tick(now));
        }
        effects.extend(self.peer_recs_preview_tick(now));
        effects
    }

    /// One poll now (R6).
    fn peer_recs_poll_now(&mut self) -> Vec<Effect> {
        if !self.connected {
            return Vec::new();
        }
        let poll = &mut self.peer_recs.poll;
        poll.next = None;
        poll.sent += 1;
        poll.asking = Some(poll.sent);
        let sent = poll.sent;
        vec![self.peer_recs_send(PeerRecsCmd::Peers { poll: sent })]
    }

    fn peer_recs_poll_tick(&mut self, now: Instant) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        let shown = r.watch == PeerRecsWatch::Shown;
        if r.poll.stopped || r.poll.asking.is_some() || !self.connected || !shown {
            return Vec::new();
        }
        if !come(&mut r.poll.next, now) {
            return Vec::new();
        }
        self.peer_recs_poll_now()
    }

    fn peer_recs_job_tick(&mut self, now: Instant) -> Vec<Effect> {
        if !self.connected {
            return Vec::new();
        }
        let Some(sheet) = self.peer_recs.sheet.as_mut() else { return Vec::new() };
        if sheet.read_out {
            return Vec::new();
        }
        let live: Vec<i64> = sheet.jobs.values().filter(|job| job.live()).map(|job| job.id).collect();
        if live.is_empty() {
            sheet.next = None;
            return Vec::new();
        }
        if sheet.next.is_none() {
            sheet.next = Some(Due::In(JOB_POLL));
        }
        if !come(&mut sheet.next, now) {
            return Vec::new();
        }
        sheet.next = None;
        let id = live[sheet.turn % live.len()];
        sheet.turn = sheet.turn.wrapping_add(1);
        let seq = self.peer_recs.next_seq();
        if let Some(sheet) = self.peer_recs.sheet.as_mut() {
            sheet.read = Some(seq);
            sheet.read_out = true;
        }
        vec![self.peer_recs_send(PeerRecsCmd::Job { seq, id })]
    }

    // ── Answers ─────────────────────────────────────────────────────────────

    /// A door answer, checked against the session generation, then
    /// against its own seq, poll number or ticket.
    pub(crate) fn consume_peer_recs(&mut self, generation: u64, event: PeerRecsEvent) -> Vec<Effect> {
        if generation != self.session_gen {
            return Vec::new();
        }
        self.peer_recs_sync();
        match event {
            PeerRecsEvent::Taste { seq, answer } => self.peer_recs_taste_answered(seq, answer),
            PeerRecsEvent::Peers { poll, answer } => self.peer_recs_peers_answered(poll, answer),
            PeerRecsEvent::Added { endpoint_id, answer } => self.peer_recs_added(endpoint_id, answer),
            PeerRecsEvent::Gate { seq, plugins, destination } => {
                self.peer_recs_gate_answered(seq, plugins, destination)
            }
            PeerRecsEvent::Preview { ticket, answer } => {
                self.peer_recs_preview_answered(ticket, answer)
            }
            PeerRecsEvent::Lookup { seq, answer } => self.peer_recs_lookup_answered(seq, answer),
            PeerRecsEvent::Job { seq, answer } => self.peer_recs_job_answered(seq, answer),
            PeerRecsEvent::Started { seq, answer } => self.peer_recs_started(seq, answer),
            PeerRecsEvent::Probed { on } => self.peer_recs_probed(on),
        }
    }

    /// A taste answer (U/behaviour.md §3.8, the phone's `_answer`).
    fn peer_recs_taste_answered(
        &mut self,
        seq: u64,
        answer: Result<Box<TasteResponse>, RecsFailure>,
    ) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        if !r.open || seq != r.ask_seq {
            return Vec::new();
        }
        r.loading = false;
        if r.taste.main_seq == seq {
            r.taste.main_pending = false;
        }
        let answer = match answer {
            Ok(answer) => *answer,
            Err(RecsFailure::Forbidden) => {
                r.failed = true;
                return self.peer_recs_reprobe(Reprobe::Taste);
            }
            Err(RecsFailure::Failed) => {
                self.peer_recs_show(None);
                return Vec::new();
            }
        };
        // No pick left (Discover's clause 21): every song the random draw
        // could take was passed over. The picks start over and the batch is
        // asked once more, the old rows standing.
        if r.ask_new && rules::no_pick_left(&answer, false, r.taste.skipped.len()) {
            r.taste.skipped.clear();
            r.taste.shown_random.clear();
            return self.peer_recs_ask_new();
        }
        match rules::read_answer(&answer, r.selected.is_some()) {
            rules::AnswerRead::Off => self.peer_recs_reprobe(Reprobe::NetworkNull),
            rules::AnswerRead::NotOnShelf => {
                r.selected = None;
                self.peer_recs_show_or_ask()
            }
            read @ (rules::AnswerRead::Keep | rules::AnswerRead::ShowNotKeep) => {
                if r.taste.head.is_none() {
                    let mut head = answer.clone();
                    if let Some(network) = head.network.as_mut() {
                        network.results.clear();
                    }
                    head.library = None;
                    head.peers = None;
                    r.taste.head = Some(head);
                }
                let key = tab_key(r.selected.as_deref());
                if read == rules::AnswerRead::Keep {
                    r.answers.insert(key, answer.clone());
                } else {
                    r.answers.remove(&key);
                }
                self.peer_recs_show(Some(answer));
                Vec::new()
            }
        }
    }

    /// A peers answer (R6; the phone's `_poll` and `_takePeers`).
    fn peer_recs_peers_answered(
        &mut self,
        poll: u64,
        answer: Result<Box<P2pPeersAnswer>, PeersFailure>,
    ) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        if r.poll.asking == Some(poll) {
            r.poll.asking = None;
        }
        if !r.open || r.poll.stopped || poll <= r.poll.answered {
            return Vec::new();
        }
        let view = match answer {
            Ok(view) => *view,
            Err(PeersFailure::Forbidden) => {
                r.poll.stopped = true;
                r.poll.next = None;
                return self.peer_recs_reprobe(Reprobe::Peers);
            }
            Err(PeersFailure::NoRoute) => {
                r.poll.stopped = true;
                r.poll.next = None;
                r.poll.no_route = true;
                r.poll.view = None;
                return Vec::new();
            }
            Err(PeersFailure::Failed) => {
                // The last strip stands, and the cadence with it.
                let items = rules::strip_items(r.poll.view.as_ref(), &r.local_adds, r.poll.answered);
                r.poll.next = Some(Due::In(rules::poll_delay(&items, !r.posting.is_empty())));
                return Vec::new();
            }
        };
        let showing_no_peers = self.peer_recs_pane() == rules::RecsPane::NoPeers;
        let r = &mut self.peer_recs;
        r.poll.answered = poll;
        let prev = r.poll.view.take();
        let (landed, settled) = rules::landed(prev.as_ref(), &view, &r.posted);
        for id in &settled {
            r.posted.remove(id);
        }
        let reask = landed
            || rules::grew_under_no_peers(prev.as_ref(), &view, showing_no_peers)
            || rules::emptied(prev.as_ref(), &view);
        let fallback = r.selected.as_deref().is_some_and(|id| rules::selected_gone(&view, id));
        // A local Add state stands until a poll sent after it answers, and
        // only on a row still addable.
        r.local_adds = rules::standing_adds(&view, &r.local_adds, poll);
        r.poll.view = Some(view);
        if reask {
            r.answers.clear();
        }
        let items = rules::strip_items(r.poll.view.as_ref(), &r.local_adds, r.poll.answered);
        r.poll.next = Some(Due::In(rules::poll_delay(&items, !r.posting.is_empty())));
        if fallback {
            // R5 does not wait for holding.
            r.selected = None;
            r.select_due = None;
            return self.peer_recs_show_or_ask();
        }
        if reask || r.pending_reask {
            return self.peer_recs_reask();
        }
        Vec::new()
    }

    /// An Add's answer (peer-recommendations plan §5.6's table).
    fn peer_recs_added(&mut self, endpoint_id: String, answer: PeerAddAnswer) -> Vec<Effect> {
        let r = &mut self.peer_recs;
        // An Add of an earlier visit's: this visit never posted it.
        if !r.open || !r.posting.remove(&endpoint_id) {
            return Vec::new();
        }
        let outcome = rules::add_outcome(&answer);
        match outcome.local_kind() {
            Some(kind) => {
                r.local_adds.insert(endpoint_id.clone(), rules::LocalAdd { kind, stamp: r.poll.sent });
            }
            None => {
                r.local_adds.remove(&endpoint_id);
            }
        }
        if !outcome.keeps_posted() {
            r.posted.remove(&endpoint_id);
        }
        if !r.poll.stopped && r.poll.asking.is_none() {
            let items = rules::strip_items(r.poll.view.as_ref(), &r.local_adds, r.poll.answered);
            let cadence = rules::poll_delay(&items, !r.posting.is_empty());
            let wait = if outcome.polls_now() { Duration::ZERO } else { cadence };
            r.poll.next = Some(Due::In(wait));
        }
        Vec::new()
    }

    /// The plug-in gate's answer: the newest read wins. A changed preview
    /// provider clears the previews' answers.
    fn peer_recs_gate_answered(
        &mut self,
        seq: u64,
        plugins: Result<Box<DiscoveryPluginsResponse>, PluginFailure>,
        destination: Option<Result<Box<CollectionDestinationView>, PluginFailure>>,
    ) -> Vec<Effect> {
        if seq != self.peer_recs.gate.asked {
            return Vec::new();
        }
        let before = self.peer_recs_provider().map(str::to_string);
        let gate = &mut self.peer_recs.gate;
        if let Ok(plugins) = plugins {
            gate.plugins = Some(*plugins);
        }
        gate.destination = match destination {
            Some(Ok(view)) => view.destination,
            Some(Err(_)) | None => None,
        };
        let after = self.peer_recs_provider().map(str::to_string);
        if after != before {
            self.peer_recs_provider_changed(after);
        }
        // A sheet opened before Get it was offered asks its lookup now, so
        // its job word is read before Enter can start a second job.
        self.peer_recs_sheet_lookup()
    }

    /// The sheet's lookup: the newest song job per Get it plug-in.
    fn peer_recs_lookup_answered(
        &mut self,
        seq: u64,
        answer: Result<Box<JobLookup>, PluginFailure>,
    ) -> Vec<Effect> {
        let names: Vec<String> = self
            .peer_recs
            .gate
            .plugins
            .as_ref()
            .map(|p| p.get_it_plugins().into_iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        let sheet = self.peer_recs.sheet.as_mut().filter(|s| s.lookup == Some(seq));
        let Some(sheet) = sheet else { return Vec::new() };
        sheet.lookup = None;
        let Ok(lookup) = answer else { return Vec::new() };
        for name in names {
            let newest = lookup
                .jobs
                .iter()
                .filter(|job| job.plugin == name && job.scope() == Some(JobScope::Song))
                .max_by_key(|job| (job.created_at.unwrap_or_default(), job.id));
            // A job the sheet already holds that is as new (a start that
            // answered while the lookup was out, or a read) is newer word
            // than the lookup's: job ids only grow (Keep's `RowJobs::merge`).
            let Some(job) = newest else { continue };
            if sheet.jobs.get(&name).is_none_or(|held| held.id < job.id) {
                sheet.jobs.insert(name, job.clone());
            }
        }
        Vec::new()
    }

    /// A job read: the row's word moves; a finished job stops the reads.
    fn peer_recs_job_answered(
        &mut self,
        seq: u64,
        answer: Result<Option<Box<PluginJob>>, PluginFailure>,
    ) -> Vec<Effect> {
        let sheet = self.peer_recs.sheet.as_mut().filter(|s| s.read == Some(seq));
        let Some(sheet) = sheet else { return Vec::new() };
        sheet.read = None;
        sheet.read_out = false;
        if let Ok(Some(job)) = answer
            && sheet.jobs.contains_key(&job.plugin)
        {
            sheet.jobs.insert(job.plugin.clone(), *job);
        }
        sheet.next = Some(Due::In(JOB_POLL));
        Vec::new()
    }

    /// A start's answer: the job joins the reads; a changed destination
    /// reads the gate again and the row goes back to idle (plan §3.6).
    fn peer_recs_started(
        &mut self,
        seq: u64,
        answer: Result<Box<JobStarted>, PluginFailure>,
    ) -> Vec<Effect> {
        let Some(sheet) = self.peer_recs.sheet.as_mut() else { return Vec::new() };
        let Some((_, plugin)) = sheet.start.take_if(|(asked, _)| *asked == seq) else { return Vec::new() };
        sheet.start_out = false;
        match answer {
            Ok(started) => {
                sheet.jobs.insert(plugin, started.job);
                sheet.next = Some(Due::In(JOB_POLL));
                Vec::new()
            }
            Err(PluginFailure::DestinationChanged(place)) => {
                if let Some(place) = place {
                    self.peer_recs.gate.destination = place;
                }
                self.peer_recs_regate()
            }
            Err(failure) => {
                sheet.refused.insert(plugin, failure.words().to_string());
                Vec::new()
            }
        }
    }

    /// R7's re-ping answered. Off (or no answer) is off whoever asked; on
    /// depends on who asked (peer-recommendations plan §3.5 R7).
    fn peer_recs_probed(&mut self, on: bool) -> Vec<Effect> {
        // No probe out: a reconnect let it go, and its fresh ping is the
        // word on the gate now, so this older answer says nothing.
        let Some(probe) = self.peer_recs.probing.take() else { return Vec::new() };
        if !on {
            let r = &mut self.peer_recs;
            r.taste.latch = TasteLatch::Off;
            r.ask_seq = r.taste.next_seq();
            r.loading = false;
            r.taste.main_pending = false;
            r.select_due = None;
            r.poll.stopped = true;
            r.poll.next = None;
            // The GUI's yield to Files is Q's `settle` (D31).
            return self.peer_recs_clip_stop(true);
        }
        let r = &mut self.peer_recs;
        // A taste 403 holds the asks whichever source's ping answers it.
        if r.taste.latch == TasteLatch::Probing {
            r.taste.latch = TasteLatch::ScanPending;
        }
        match probe.source {
            Reprobe::Taste | Reprobe::Peers | Reprobe::Clip => {}
            Reprobe::NetworkNull => {
                // Try again, unless a newer ask took over meanwhile.
                if probe.visit == r.visit && probe.ask == r.ask_seq {
                    self.peer_recs_show(None);
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::fixtures::{self, ATTIC, BASEMENT, KITCHEN, LIGHTHOUSE, ROOFTOP, STATIC};
    use crate::api::types::{Capabilities, PeerAddKind, PeerAddState};
    use serde_json::{Value, json};

    /// A session on a server with the room's four flags and plug-ins.
    fn room_app() -> App {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.capabilities = Capabilities {
            discovery: true,
            discovery_p2p: true,
            discovery_taste: 3,
            discovery_p2p_peers: 1,
            discovery_plugins: true,
            uploads: true,
            ..Capabilities::default()
        };
        app
    }

    fn cmds(effects: &[Effect]) -> Vec<PeerRecsCmd> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::PeerRecs { cmd, .. }) => Some(cmd.clone()),
                _ => None,
            })
            .collect()
    }

    /// The taste asks: their seq and their body as JSON.
    fn tastes(effects: &[Effect]) -> Vec<(u64, Value)> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Taste { seq, body } => Some((seq, serde_json::to_value(&*body).unwrap())),
                _ => None,
            })
            .collect()
    }

    fn one_taste(effects: &[Effect]) -> (u64, Value) {
        let asks = tastes(effects);
        assert_eq!(asks.len(), 1, "one taste ask in {effects:?}");
        asks.into_iter().next().unwrap()
    }

    fn polls(effects: &[Effect]) -> Vec<u64> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Peers { poll } => Some(poll),
                _ => None,
            })
            .collect()
    }

    fn gates(effects: &[Effect]) -> Vec<u64> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Gate { seq } => Some(seq),
                _ => None,
            })
            .collect()
    }

    fn adds(effects: &[Effect]) -> Vec<String> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Add { endpoint_id } => Some(endpoint_id),
                _ => None,
            })
            .collect()
    }

    fn probes(effects: &[Effect]) -> usize {
        cmds(effects).iter().filter(|c| **c == PeerRecsCmd::Probe).count()
    }

    fn land(app: &mut App, event: PeerRecsEvent) -> Vec<Effect> {
        let generation = app.session_gen;
        app.apply_event(Event::PeerRecs { generation, event: Box::new(event) })
    }

    fn answer(app: &mut App, seq: u64, name: &str) -> Vec<Effect> {
        land(app, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(fixtures::taste(name))) })
    }

    fn answer_with(app: &mut App, seq: u64, response: TasteResponse) -> Vec<Effect> {
        land(app, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(response)) })
    }

    fn peers(app: &mut App, poll: u64, name: &str) -> Vec<Effect> {
        land(app, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(fixtures::peers(name))) })
    }

    fn peers_with(app: &mut App, poll: u64, view: P2pPeersAnswer) -> Vec<Effect> {
        land(app, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(view)) })
    }

    /// The stage's listing: links, Deezer previews, a Get it plug-in.
    fn listing(jobs: bool) -> DiscoveryPluginsResponse {
        serde_json::from_value(json!({
            "plugins": [
                {"name": "links", "title": "Links", "capabilities": ["links"]},
                {"name": "deezer", "title": "Deezer", "capabilities": ["preview"]},
                {"name": "noop-acquire", "title": "Noop", "capabilities": ["acquire"]}
            ],
            "jobs": {"allowed": jobs}
        }))
        .unwrap()
    }

    fn destination() -> CollectionDestinationView {
        let place = json!({"vpath": "music", "base": "", "layout": "{{ARTIST}}"});
        serde_json::from_value(json!({"destination": place})).unwrap()
    }

    fn gate(app: &mut App, seq: u64, plugins: DiscoveryPluginsResponse, place: bool) -> Vec<Effect> {
        let destination = place.then(|| Ok(Box::new(destination())));
        land(app, PeerRecsEvent::Gate { seq, plugins: Ok(Box::new(plugins)), destination })
    }

    /// A visit open, its R1 answered with the stage's All batch, the
    /// strip's first poll answered as an admin's, and the gate read.
    fn ready_with(peers_name: &str) -> App {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        let (seq, _) = one_taste(&effects);
        answer(&mut app, seq, "taste_all");
        peers(&mut app, polls(&effects)[0], peers_name);
        gate(&mut app, gates(&effects)[0], listing(true), true);
        app
    }

    fn ready() -> App {
        ready_with("peers_admin")
    }

    /// A clip playing for the shown lead at `index`: pressed, resolved
    /// through the stage's Deezer answer, and handed to the engine.
    fn playing(app: &mut App, index: usize) {
        app.audio_available = true;
        let pressed = app.peer_recs_press(index, Instant::now());
        let ticket = cmds(&pressed)
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Preview { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("the press resolves its lead");
        let deezer: crate::api::types::PreviewResolveResponse =
            serde_json::from_value(fixtures::preview("preview_deezer")).unwrap();
        let answer = Ok(deezer.into_preview().map(Box::new));
        let effects = land(app, PeerRecsEvent::Preview { ticket, answer });
        assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::AsidePlay { .. }))), "{effects:?}");
        assert!(app.peer_recs_clip_live());
    }

    /// The room's stop of a live clip with main restored.
    fn restores(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::AsideStop { ticket: None, restore: true })))
    }

    fn titles(app: &App) -> Vec<String> {
        app.peer_recs_leads().iter().map(|l| l.title.clone().unwrap_or_default()).collect()
    }

    /// R1 at open: All, no pins, nothing dropped, window 30.
    fn r1_open() -> Value {
        json!({"window":30,"sources":["network"],"newArtistsOnly":false,
               "limit":{"seeds":12,"network":32},"networkOnRandomSeed":true})
    }

    fn batch_pins() -> Value {
        json!(["testlib/Vosto/Night Drive/01 - Highway.mp3", "testlib/Vosto/Night Drive/02 - Neon.mp3"])
    }

    // ── Asks and fences ─────────────────────────────────────────────────

    #[test]
    fn open_sends_r1_one_poll_and_one_gate_together() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        let (_, body) = one_taste(&effects);
        assert_eq!(body, r1_open());
        assert_eq!(polls(&effects), vec![1]);
        assert_eq!(gates(&effects).len(), 1);
        assert_eq!(cmds(&effects).len(), 3);
        assert!(app.peer_recs_asking());
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::Loading);
    }

    #[test]
    fn a_tab_asks_r2_pinned_to_the_batch_and_all_again_is_cached() {
        let mut app = ready();
        assert_eq!(app.peer_recs_leads().len(), 12);
        let effects = app.peer_recs_select(Some(KITCHEN.to_string()));
        let (seq, body) = one_taste(&effects);
        assert_eq!(
            body,
            json!({"window":30,"pin":batch_pins(),"sources":["network"],"newArtistsOnly":false,
                   "limit":{"seeds":2,"network":32},"networkOnRandomSeed":true,"networkPeers":[KITCHEN]})
        );
        answer(&mut app, seq, "taste_kitchen");
        assert_eq!(app.peer_recs_leads().len(), 5);
        // All again: its kept answer, no call.
        assert!(app.peer_recs_select(None).is_empty());
        assert_eq!(app.peer_recs_leads().len(), 12);
        // And Kitchen Radio again.
        assert!(app.peer_recs_select(Some(KITCHEN.to_string())).is_empty());
        assert_eq!(titles(&app)[0], "Signal Fires");
    }

    #[test]
    fn a_kept_tab_answer_draws_with_no_call_and_drops_an_older_ask() {
        let mut app = ready();
        let (kitchen, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, kitchen, "taste_kitchen");
        let (basement, _) = one_taste(&app.peer_recs_select(Some(BASEMENT.to_string())));
        // Back to the kept Kitchen tab while Basement's ask is out.
        assert!(app.peer_recs_select(Some(KITCHEN.to_string())).is_empty());
        assert!(!app.peer_recs_asking());
        answer(&mut app, basement, "taste_basement");
        assert_eq!(app.peer_recs_strip().selected.as_deref(), Some(KITCHEN));
        assert_eq!(titles(&app)[0], "Signal Fires");
    }

    #[test]
    fn keyboard_steps_ask_once_250_ms_after_the_last_and_clicks_at_once() {
        let mut app = ready();
        let t0 = Instant::now();
        // → → within 250 ms: Basement, then Kitchen Radio.
        assert!(app.peer_recs_step(1, t0).is_empty());
        assert!(app.peer_recs_step(1, t0 + Duration::from_millis(100)).is_empty());
        assert_eq!(app.peer_recs_strip().selected.as_deref(), Some(KITCHEN));
        assert!(app.peer_recs_asking());
        assert!(tastes(&app.tick_at(t0 + Duration::from_millis(300))).is_empty());
        let (_, body) = one_taste(&app.tick_at(t0 + Duration::from_millis(350)));
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
        assert!(tastes(&app.tick_at(t0 + Duration::from_secs(1))).is_empty());
        // A click asks at once.
        let (_, body) = one_taste(&app.peer_recs_select(Some(BASEMENT.to_string())));
        assert_eq!(body["networkPeers"], json!([BASEMENT]));
        // ← back to All draws its kept answer at once.
        assert!(app.peer_recs_step(-1, t0 + Duration::from_secs(2)).is_empty());
        assert_eq!(app.peer_recs_strip().selected, None);
        assert_eq!(app.peer_recs_leads().len(), 12);
        assert!(!app.peer_recs_asking());
        // And at the strip's start nothing moves.
        assert!(app.peer_recs_step(-1, t0 + Duration::from_secs(3)).is_empty());
        assert!(tastes(&app.tick_at(t0 + Duration::from_secs(4))).is_empty());
    }

    #[test]
    fn the_last_ask_wins_and_another_generation_or_visit_is_dropped() {
        let mut app = ready();
        let (old, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        let (new, _) = one_taste(&app.peer_recs_select(Some(BASEMENT.to_string())));
        answer(&mut app, old, "taste_kitchen");
        assert_eq!(app.peer_recs_leads().len(), 12, "an older ask's answer fills nothing");
        // Another generation's answer for the newest seq.
        let generation = app.session_gen + 1;
        let event = PeerRecsEvent::Taste { seq: new, answer: Ok(Box::new(fixtures::taste("taste_basement"))) };
        app.apply_event(Event::PeerRecs { generation, event: Box::new(event) });
        assert_eq!(app.peer_recs_leads().len(), 12);
        answer(&mut app, new, "taste_basement");
        assert_eq!(app.peer_recs_leads().len(), 5);
        // A visit left drops its answers.
        let (late, _) = one_taste(&app.peer_recs_new_seeds());
        app.peer_recs_watch(PeerRecsWatch::Away);
        let effects = app.peer_recs_open(None);
        answer(&mut app, late, "taste_kitchen");
        assert!(app.peer_recs_leads().is_empty());
        let (seq, _) = one_taste(&effects);
        answer(&mut app, seq, "taste_all");
        assert_eq!(app.peer_recs_leads().len(), 12);
    }

    #[test]
    fn new_seeds_on_a_random_batch_excludes_the_pick_and_keeps_the_tab() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        peers(&mut app, polls(&effects)[0], "peers_admin");
        answer(&mut app, one_taste(&effects).0, "taste_random");
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        let (_, body) = one_taste(&app.peer_recs_new_seeds());
        assert_eq!(
            body,
            json!({"window":30,"exclude":["cd7a9eee4221ed34b9e416a86005a343"],"sources":["network"],
                   "newArtistsOnly":false,"limit":{"seeds":12,"network":32},"networkOnRandomSeed":true,
                   "networkPeers":[KITCHEN]})
        );
        assert!(app.peer_recs.taste.head.is_none(), "a new batch lets the old one go");
    }

    #[test]
    fn new_seeds_while_an_ask_is_out_asks_nothing() {
        let mut app = room_app();
        app.peer_recs_open(None);
        assert!(tastes(&app.peer_recs_new_seeds()).is_empty());
    }

    #[test]
    fn window_and_seed_edits_ask_a_new_batch_keeping_the_tab() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        let (seq, body) = one_taste(&app.peer_recs_set_window(90));
        assert_eq!(body["window"], json!(90));
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
        assert!(body.get("pin").is_none());
        assert_eq!(app.peer_recs_window_shown(), Some(90));
        // The window already asked is no new ask.
        assert!(app.peer_recs_set_window(90).is_empty());
        answer(&mut app, seq, "taste_kitchen");
        // A dropped seed asks with it excluded.
        let hash = "dbd0ae2c76e2117f94b7e34970912a7e".to_string();
        let (_, body) = one_taste(&app.peer_recs_seeds_closed(&[hash.clone()], &[], false));
        assert_eq!(body["exclude"], json!([hash]));
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
        // A modal closed with nothing changed asks nothing.
        assert!(app.peer_recs_seeds_closed(&[], &[], false).is_empty());
    }

    #[test]
    fn no_pick_left_clears_passed_picks_and_asks_once_more() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        answer(&mut app, one_taste(&effects).0, "taste_random");
        let (seq, body) = one_taste(&app.peer_recs_new_seeds());
        assert_eq!(body["exclude"], json!(["cd7a9eee4221ed34b9e416a86005a343"]));
        let mut empty = fixtures::taste("taste_random");
        empty.seeds.clear();
        let effects = answer_with(&mut app, seq, empty.clone());
        let (again, body) = one_taste(&effects);
        assert!(body.get("exclude").is_none(), "the passed picks start over");
        assert!(app.peer_recs.taste.skipped.is_empty());
        // Once: with no pick passed over, an empty answer is drawn.
        assert!(tastes(&answer_with(&mut app, again, empty)).is_empty());
    }

    #[test]
    fn not_on_shelf_falls_back_to_all_once_and_is_never_kept() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(ATTIC.to_string())));
        // All's answer is kept: drawn with no call.
        assert!(answer(&mut app, seq, "taste_not_on_shelf").is_empty());
        assert_eq!(app.peer_recs_strip().selected, None);
        assert_eq!(app.peer_recs_leads().len(), 12);
        // Not kept for Attic: choosing it again asks again.
        assert_eq!(tastes(&app.peer_recs_select(Some(ATTIC.to_string()))).len(), 1);
        // With no kept All, the fallback asks All on the batch.
        let mut app = ready();
        app.peer_recs.answers.clear();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(ATTIC.to_string())));
        let (_, body) = one_taste(&answer(&mut app, seq, "taste_not_on_shelf"));
        assert_eq!(body["pin"], batch_pins());
        assert!(body.get("networkPeers").is_none());
    }

    #[test]
    fn an_error_answer_is_shown_but_never_kept() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_error");
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::TryAgain);
        assert!(app.peer_recs_select(None).is_empty());
        assert_eq!(tastes(&app.peer_recs_select(Some(KITCHEN.to_string()))).len(), 1);
    }

    #[test]
    fn the_room_never_sends_moods_or_ignore_v_paths() {
        let mut app = ready();
        app.peer_recs.taste.moods = crate::api::types::MoodFilter::default();
        let mut bodies: Vec<Value> = Vec::new();
        let (seq, body) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        bodies.push(body);
        answer(&mut app, seq, "taste_kitchen");
        let (seq, body) = one_taste(&app.peer_recs_new_seeds());
        bodies.push(body);
        answer(&mut app, seq, "taste_all");
        bodies.extend(tastes(&app.peer_recs_set_window(7)).into_iter().map(|t| t.1));
        assert_eq!(bodies.len(), 3);
        for body in bodies {
            assert!(body.get("moods").is_none() && body.get("ignoreVPaths").is_none(), "{body}");
            assert_eq!(body["sources"], json!(["network"]));
        }
    }

    // ── The poll ────────────────────────────────────────────────────────

    #[test]
    fn polls_every_15_s_and_3_s_while_adding_one_at_a_time() {
        let mut app = ready();
        let t0 = Instant::now();
        assert!(polls(&app.tick_at(t0)).is_empty(), "the wait starts now");
        assert!(polls(&app.tick_at(t0 + Duration::from_secs(14))).is_empty());
        assert_eq!(polls(&app.tick_at(t0 + Duration::from_secs(15))), vec![2]);
        // One at a time: nothing more while it is out.
        assert!(polls(&app.tick_at(t0 + Duration::from_secs(40))).is_empty());
        peers(&mut app, 2, "peers_admin");
        // An Add: the next poll in 3 s.
        assert_eq!(adds(&app.peer_recs_add(LIGHTHOUSE)), vec![LIGHTHOUSE.to_string()]);
        let t1 = t0 + Duration::from_secs(41);
        app.tick_at(t1);
        assert_eq!(polls(&app.tick_at(t1 + Duration::from_secs(3))), vec![3]);
        // The add still out, and a row reading adding: 3 s again.
        let mut view = fixtures::peers("peers_admin");
        view.peers.iter_mut().find(|p| p.endpoint_id == LIGHTHOUSE).unwrap().add =
            Some(PeerAddState { state: PeerAddKind::Adding, code: None, at: None });
        peers_with(&mut app, 3, view.clone());
        let t2 = t1 + Duration::from_secs(4);
        app.tick_at(t2);
        assert_eq!(polls(&app.tick_at(t2 + Duration::from_secs(3))), vec![4]);
        // An older poll's late answer is dropped.
        peers(&mut app, 4, "peers_admin");
        assert_eq!(tab_kind(&app, LIGHTHOUSE), Some(rules::StripKind::Addable));
        peers_with(&mut app, 3, view);
        assert_eq!(tab_kind(&app, LIGHTHOUSE), Some(rules::StripKind::Addable));
        assert_eq!(app.peer_recs.poll.answered, 4);
    }

    #[test]
    fn covered_polls_nothing_and_one_on_return() {
        let mut app = ready();
        let t0 = Instant::now();
        app.tick_at(t0);
        app.peer_recs_watch(PeerRecsWatch::Covered);
        assert!(polls(&app.tick_at(t0 + Duration::from_secs(60))).is_empty());
        app.peer_recs_watch(PeerRecsWatch::Shown);
        assert_eq!(polls(&app.tick_at(t0 + Duration::from_secs(61))), vec![2]);
        assert!(app.peer_recs.open, "a cover keeps the visit");
        assert_eq!(app.peer_recs_leads().len(), 12);
    }

    #[test]
    fn leaving_stops_the_poll_and_drops_late_answers() {
        let mut app = ready();
        let t0 = Instant::now();
        app.tick_at(t0);
        let effects = app.tick_at(t0 + Duration::from_secs(15));
        assert_eq!(polls(&effects), vec![2]);
        app.peer_recs_watch(PeerRecsWatch::Away);
        peers(&mut app, 2, "peers_admin_empty_shelf");
        assert!(polls(&app.tick_at(t0 + Duration::from_secs(120))).is_empty());
        assert!(!app.peer_recs.open);
        // The next visit's first poll is newer than every earlier one.
        let effects = app.peer_recs_open(None);
        assert_eq!(polls(&effects), vec![3]);
        peers(&mut app, 2, "peers_admin_empty_shelf");
        assert!(app.peer_recs.poll.view.is_none());
    }

    #[test]
    fn a_404_peers_route_is_all_alone_and_stops() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        assert!(app.peer_recs_strip().skeleton);
        land(&mut app, PeerRecsEvent::Peers { poll: polls(&effects)[0], answer: Err(PeersFailure::NoRoute) });
        let strip = app.peer_recs_strip();
        assert!(!strip.drawn && !strip.skeleton);
        assert_eq!(strip.items.len(), 1);
        assert!(polls(&app.tick_at(Instant::now() + Duration::from_secs(60))).is_empty());
        assert_eq!(probes(&app.tick_at(Instant::now() + Duration::from_secs(61))), 0);
    }

    #[test]
    fn the_selected_peer_gone_falls_back_to_all_without_waiting_for_holding() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        app.peer_recs_sheet_open(0);
        assert!(app.peer_recs_holding());
        let mut view = fixtures::peers("peers_admin");
        view.peers.retain(|p| p.endpoint_id != KITCHEN);
        let effects = peers_with(&mut app, 2, view.clone());
        assert!(tastes(&effects).is_empty(), "All's kept answer, at once");
        assert_eq!(app.peer_recs_strip().selected, None);
        assert_eq!(app.peer_recs_leads().len(), 12);
        // An offline shelf row keeps its tab.
        let (seq, _) = one_taste(&app.peer_recs_select(Some(ATTIC.to_string())));
        answer(&mut app, seq, "taste_attic");
        peers_with(&mut app, 3, view);
        assert_eq!(app.peer_recs_strip().selected.as_deref(), Some(ATTIC));
        // With a clip live, the same.
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        playing(&mut app, 0);
        let mut view = fixtures::peers("peers_admin");
        view.peers.retain(|p| p.endpoint_id != KITCHEN);
        assert!(tastes(&peers_with(&mut app, 2, view)).is_empty());
        assert_eq!(app.peer_recs_strip().selected, None);
        assert_eq!(app.peer_recs_leads().len(), 12);
        assert!(app.peer_recs_clip_live(), "a tab change lets the clip play out");
    }

    #[test]
    fn a_failed_poll_keeps_the_strip_and_the_cadence() {
        let mut app = ready();
        let t0 = Instant::now();
        app.tick_at(t0);
        assert_eq!(polls(&app.tick_at(t0 + Duration::from_secs(15))), vec![2]);
        let before = app.peer_recs_strip();
        land(&mut app, PeerRecsEvent::Peers { poll: 2, answer: Err(PeersFailure::Failed) });
        assert_eq!(app.peer_recs_strip(), before);
        let t1 = t0 + Duration::from_secs(16);
        app.tick_at(t1);
        assert_eq!(polls(&app.tick_at(t1 + Duration::from_secs(15))), vec![3]);
        // A dropped socket's error clears the wait; the cadence goes on.
        app.apply_event(Event::Error("socket dropped".into()));
        let t2 = t1 + Duration::from_secs(16);
        app.tick_at(t2);
        assert_eq!(polls(&app.tick_at(t2 + Duration::from_secs(15))), vec![4]);
        assert_eq!(app.peer_recs_strip(), before);
    }

    #[test]
    fn nothing_is_asked_while_disconnected_and_the_gate_holds() {
        let mut app = ready();
        app.connected = false;
        assert!(app.peer_recs_gate(), "a short disconnect keeps the row");
        let t0 = Instant::now();
        assert!(cmds(&app.tick_at(t0 + Duration::from_secs(60))).is_empty());
        assert!(cmds(&app.peer_recs_select(Some(KITCHEN.to_string()))).is_empty());
        assert!(cmds(&app.peer_recs_new_seeds()).is_empty());
        assert!(cmds(&app.peer_recs_add(LIGHTHOUSE)).is_empty());
        let mut fresh = room_app();
        fresh.connected = false;
        assert!(cmds(&fresh.peer_recs_open(None)).is_empty());
        assert!(fresh.peer_recs_gate());
    }

    // ── Adds ────────────────────────────────────────────────────────────

    #[test]
    fn a_member_never_posts() {
        let mut app = ready_with("peers_member");
        assert!(app.peer_recs_strip().items.iter().all(|i| i.acts() != rules::StripAct::Add));
        for id in [LIGHTHOUSE, STATIC, ROOFTOP, KITCHEN] {
            assert!(adds(&app.peer_recs_add(id)).is_empty());
        }
    }

    #[test]
    fn no_room_and_adding_never_post() {
        let mut app = ready_with("peers_admin_add_states");
        assert!(adds(&app.peer_recs_add(LIGHTHOUSE)).is_empty(), "no room");
        assert!(adds(&app.peer_recs_add(STATIC)).is_empty(), "adding");
        assert_eq!(adds(&app.peer_recs_add(ROOFTOP)), vec![ROOFTOP.to_string()], "try again posts again");
        assert!(adds(&app.peer_recs_add(ROOFTOP)).is_empty(), "a second press while posting");
    }

    fn add_answer(status: u16, state: Option<(PeerAddKind, Option<&str>)>) -> PeerAddAnswer {
        PeerAddAnswer {
            status,
            state: state.map(|(state, code)| PeerAddState { state, code: code.map(str::to_string), at: None }),
        }
    }

    fn tab_kind(app: &App, id: &str) -> Option<rules::StripKind> {
        app.peer_recs_strip().items.into_iter().find(|i| i.id.as_deref() == Some(id)).map(|i| i.kind)
    }

    #[test]
    fn every_add_outcome_moves_the_strip_and_the_poll() {
        use rules::StripKind::{Addable, Adding, NoRoom, TryAgain};
        let failed = |code| Some((PeerAddKind::Failed, Some(code)));
        // The answer, the tab it leaves, whether it polls at once, and
        // whether the id stays posted.
        let cases: [(PeerAddAnswer, Option<rules::StripKind>, bool, bool); 8] = [
            (add_answer(202, None), Some(Adding), false, true),
            (add_answer(200, Some((PeerAddKind::Done, None))), Some(Adding), true, true),
            (add_answer(200, Some((PeerAddKind::Adding, None))), Some(Adding), false, true),
            (add_answer(200, failed("no-room")), Some(NoRoom), false, false),
            (add_answer(200, failed("offline")), Some(TryAgain), false, false),
            (add_answer(404, None), Some(Addable), true, false),
            (add_answer(405, None), Some(Addable), true, false),
            (add_answer(0, None), Some(TryAgain), false, false),
        ];
        for (answer, kind, now, posted) in cases {
            let mut app = ready();
            let t0 = Instant::now();
            app.peer_recs_add(LIGHTHOUSE);
            assert_eq!(tab_kind(&app, LIGHTHOUSE), Some(rules::StripKind::Adding));
            let status = answer.status;
            land(&mut app, PeerRecsEvent::Added { endpoint_id: LIGHTHOUSE.to_string(), answer });
            assert_eq!(tab_kind(&app, LIGHTHOUSE), kind, "status {status}");
            assert_eq!(app.peer_recs.posted.contains(LIGHTHOUSE), posted, "status {status}");
            assert_eq!(polls(&app.tick_at(t0)).len(), usize::from(now), "status {status}: poll at once");
        }
    }

    #[test]
    fn an_own_add_that_lands_re_asks_pinned_once() {
        let mut app = ready();
        app.peer_recs_add(LIGHTHOUSE);
        let answer = add_answer(202, None);
        land(&mut app, PeerRecsEvent::Added { endpoint_id: LIGHTHOUSE.to_string(), answer });
        let mut view = fixtures::peers("peers_admin");
        let row = view.peers.iter_mut().find(|p| p.endpoint_id == LIGHTHOUSE).unwrap();
        row.on_shelf = true;
        let (_, body) = one_taste(&peers_with(&mut app, 2, view.clone()));
        assert_eq!(body["pin"], batch_pins());
        assert!(body.get("networkPeers").is_none());
        assert!(!app.peer_recs.posted.contains(LIGHTHOUSE));
        // Once: the next poll that says the same asks nothing.
        assert!(tastes(&peers_with(&mut app, 3, view)).is_empty());
        // Another admin's Add only moves the strip.
        let mut other = fixtures::peers("peers_admin");
        other.peers.iter_mut().find(|p| p.endpoint_id == STATIC).unwrap().on_shelf = true;
        assert!(tastes(&peers_with(&mut app, 4, other)).is_empty());
    }

    #[test]
    fn while_holding_the_re_ask_waits_then_runs_at_once() {
        let mut app = ready();
        app.peer_recs_add(LIGHTHOUSE);
        app.peer_recs_sheet_open(0);
        let mut view = fixtures::peers("peers_admin");
        view.peers.iter_mut().find(|p| p.endpoint_id == LIGHTHOUSE).unwrap().on_shelf = true;
        assert!(tastes(&peers_with(&mut app, 2, view)).is_empty());
        assert!(app.peer_recs.pending_reask);
        // Every tab's answer went at once.
        assert!(app.peer_recs.answers.is_empty());
        let (_, body) = one_taste(&app.peer_recs_sheet_close());
        assert_eq!(body["pin"], batch_pins());
        assert!(!app.peer_recs.pending_reask);
        // A live clip holds it too; its end lets it run on the next tick.
        let mut app = ready();
        playing(&mut app, 0);
        app.peer_recs_add(LIGHTHOUSE);
        let mut view = fixtures::peers("peers_admin");
        view.peers.iter_mut().find(|p| p.endpoint_id == LIGHTHOUSE).unwrap().on_shelf = true;
        assert!(tastes(&peers_with(&mut app, 2, view)).is_empty());
        assert!(app.peer_recs.pending_reask);
        app.peer_recs_clip_stop(true);
        assert_eq!(tastes(&app.tick_at(Instant::now())).len(), 1);
    }

    #[test]
    fn covering_or_leaving_the_room_ends_a_live_clip_with_main_restored() {
        let mut app = ready();
        playing(&mut app, 0);
        assert!(restores(&app.peer_recs_watch(PeerRecsWatch::Covered)));
        assert!(!app.peer_recs_clip_live());
        assert!(app.peer_recs.open, "a cover keeps the visit");
        app.peer_recs_watch(PeerRecsWatch::Shown);
        playing(&mut app, 1);
        assert!(restores(&app.peer_recs_watch(PeerRecsWatch::Away)));
        assert!(!app.peer_recs.open);
        // The sheet over the room is no cover: the clip plays on.
        let mut app = ready();
        playing(&mut app, 0);
        app.peer_recs_sheet_open(3);
        assert!(app.peer_recs_watch(PeerRecsWatch::Shown).is_empty());
        assert!(app.peer_recs_clip_live());
    }

    #[test]
    fn the_shelf_emptying_drops_kept_answers_and_re_asks() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        let mut view = fixtures::peers("peers_admin");
        view.shelf.searchable = 0;
        let (seq, body) = one_taste(&peers_with(&mut app, 2, view));
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
        assert!(app.peer_recs.answers.is_empty());
        answer(&mut app, seq, "taste_empty_shelf");
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::NoPeers);
        // The shelf growing under No peers yet asks again.
        assert_eq!(tastes(&peers(&mut app, 3, "peers_admin")).len(), 1);
    }

    // ── R7, the gate and the session ────────────────────────────────────

    fn probed(app: &mut App, on: bool) -> Vec<Effect> {
        land(app, PeerRecsEvent::Probed { on })
    }

    #[test]
    fn taste_403_reprobes_then_scan_pending_or_off() {
        for on in [true, false] {
            let mut app = room_app();
            let (seq, _) = one_taste(&app.peer_recs_open(None));
            let effects = land(&mut app, PeerRecsEvent::Taste { seq, answer: Err(RecsFailure::Forbidden) });
            assert_eq!(probes(&effects), 1);
            assert_eq!(app.peer_recs_pane(), rules::RecsPane::ScanPending);
            assert!(tastes(&app.peer_recs_new_seeds()).is_empty(), "asks wait for the re-ping");
            probed(&mut app, on);
            if on {
                assert_eq!(app.peer_recs.taste.latch, TasteLatch::ScanPending);
                assert_eq!(app.peer_recs_pane(), rules::RecsPane::ScanPending);
                assert!(app.peer_recs_gate());
            } else {
                assert!(!app.peer_recs_gate());
            }
        }
    }

    #[test]
    fn network_null_reprobes_then_try_again_or_off() {
        for on in [true, false] {
            let mut app = room_app();
            let (seq, _) = one_taste(&app.peer_recs_open(None));
            assert_eq!(probes(&answer(&mut app, seq, "taste_network_null")), 1);
            probed(&mut app, on);
            if on {
                assert!(app.peer_recs_gate());
                assert_eq!(app.peer_recs_pane(), rules::RecsPane::TryAgain);
            } else {
                assert!(!app.peer_recs_gate());
            }
        }
        // A newer ask taken over during the re-ping keeps its answer.
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_network_null");
        let (seq, _) = one_taste(&app.peer_recs_select(Some(BASEMENT.to_string())));
        answer(&mut app, seq, "taste_basement");
        probed(&mut app, true);
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::Ready);
        // A random answer with no network is Try again, no re-ping.
        let mut app = room_app();
        let (seq, _) = one_taste(&app.peer_recs_open(None));
        let mut random = fixtures::taste("taste_random");
        random.network = None;
        assert_eq!(probes(&answer_with(&mut app, seq, random)), 0);
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::TryAgain);
    }

    #[test]
    fn peers_403_reprobes_and_stops_the_poll() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        let poll = polls(&effects)[0];
        let effects = land(&mut app, PeerRecsEvent::Peers { poll, answer: Err(PeersFailure::Forbidden) });
        assert_eq!(probes(&effects), 1);
        probed(&mut app, true);
        assert!(app.peer_recs_gate());
        assert!(polls(&app.tick_at(Instant::now() + Duration::from_secs(120))).is_empty());
    }

    #[test]
    fn a_reprobe_with_no_answer_is_off() {
        // The door's shaper reads a failed ping as `Probed { on: false }`.
        let mut app = ready();
        app.peer_recs_reprobe(Reprobe::Peers);
        probed(&mut app, false);
        assert!(!app.peer_recs_gate());
        assert!(polls(&app.tick_at(Instant::now() + Duration::from_secs(120))).is_empty());
    }

    #[test]
    fn one_reprobe_at_a_time() {
        let mut app = ready();
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Peers)), 1);
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Taste)), 0);
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Clip)), 0);
        probed(&mut app, true);
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Clip)), 1);
    }

    #[test]
    fn off_hides_the_gate_until_the_next_connected() {
        let mut app = ready();
        app.peer_recs_reprobe(Reprobe::Peers);
        probed(&mut app, false);
        assert!(!app.peer_recs_gate());
        app.peer_recs_reconnected();
        assert!(app.peer_recs_gate());
    }

    /// A ping asked before a same-server reconnect, answered off after it:
    /// the reconnect's fresh ping is the word on the gate, and the late
    /// answer has no probe left to answer.
    #[test]
    fn a_late_off_after_a_reconnect_is_dropped() {
        let mut app = ready();
        let t0 = Instant::now();
        app.tick_at(t0);
        let poll = polls(&app.tick_at(t0 + Duration::from_secs(60)))[0];
        let forbidden = PeerRecsEvent::Peers { poll, answer: Err(PeersFailure::Forbidden) };
        assert_eq!(probes(&land(&mut app, forbidden)), 1);
        app.peer_recs_reconnected();
        let effects = probed(&mut app, false);
        assert_ne!(app.peer_recs.taste.latch, TasteLatch::Off);
        assert!(app.peer_recs_gate(), "the row stays");
        assert!(!restores(&effects), "nothing ends: {effects:?}");
    }

    /// A null network answer while another source's re-ping is out: Try
    /// again now, never the last tab's leads under the new tab.
    #[test]
    fn a_null_network_with_a_reprobe_out_is_try_again_now() {
        let mut app = ready();
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Clip)), 1);
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        assert_eq!(probes(&answer(&mut app, seq, "taste_network_null")), 0, "one re-ping at a time");
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::TryAgain);
        assert!(app.peer_recs_leads().is_empty(), "All's leads are not Kitchen's");
        probed(&mut app, true);
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::TryAgain);
    }

    /// A taste 403 while the strip's re-ping is out: the asks wait and the
    /// pane reads Scan pending; that ping's on answer settles it.
    #[test]
    fn a_taste_403_with_a_reprobe_out_is_scan_pending() {
        for on in [true, false] {
            let mut app = room_app();
            let effects = app.peer_recs_open(None);
            let poll = polls(&effects)[0];
            let (seq, _) = one_taste(&effects);
            let peers = PeerRecsEvent::Peers { poll, answer: Err(PeersFailure::Forbidden) };
            assert_eq!(probes(&land(&mut app, peers)), 1);
            let taste = PeerRecsEvent::Taste { seq, answer: Err(RecsFailure::Forbidden) };
            assert_eq!(probes(&land(&mut app, taste)), 0, "one re-ping at a time");
            assert_eq!(app.peer_recs_pane(), rules::RecsPane::ScanPending);
            assert!(tastes(&app.peer_recs_new_seeds()).is_empty(), "asks wait for the re-ping");
            probed(&mut app, on);
            if on {
                assert_eq!(app.peer_recs.taste.latch, TasteLatch::ScanPending);
                assert_eq!(app.peer_recs_pane(), rules::RecsPane::ScanPending);
            } else {
                assert!(!app.peer_recs_gate());
            }
        }
    }

    #[test]
    fn the_gate_needs_the_flags_an_own_session_and_no_off_latch() {
        let app = room_app();
        assert!(app.peer_recs_gate());
        type Off = fn(&mut Capabilities);
        let offs: [Off; 4] = [
            |c| c.discovery = false,
            |c| c.discovery_p2p = false,
            |c| c.discovery_taste = 2,
            |c| c.discovery_p2p_peers = 0,
        ];
        for off in offs {
            let mut app = room_app();
            off(&mut app.capabilities);
            assert!(!app.peer_recs_gate());
            assert!(cmds(&app.peer_recs_open(None)).is_empty());
        }
        let mut app = room_app();
        app.session.peer = Some(("Kitchen".into(), 7));
        assert!(!app.peer_recs_gate());
        let mut app = room_app();
        app.peer_recs.taste.latch = TasteLatch::Off;
        assert!(!app.peer_recs_gate());
    }

    #[test]
    fn a_server_switch_resets_the_visit_and_owes_the_clip_a_stop() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        app.peer_recs.taste.window = Some(90);
        playing(&mut app, 0);
        app.session_gen += 1;
        app.reset_peer_recs();
        assert!(!app.peer_recs_clip_live());
        assert!(restores(&app.tick_at(Instant::now())), "the clip's stop, owed to the next tick");
        assert!(!app.peer_recs.open);
        assert!(app.peer_recs_leads().is_empty());
        assert_eq!(app.peer_recs_strip().selected, None);
        assert_eq!(app.peer_recs.taste.window, Some(90), "the window is a memory, not the server's");
        // The old session's answers fill nothing.
        let event = PeerRecsEvent::Taste { seq, answer: Ok(Box::new(fixtures::taste("taste_kitchen"))) };
        app.apply_event(Event::PeerRecs { generation: app.session_gen - 1, event: Box::new(event) });
        assert!(app.peer_recs_leads().is_empty());
        assert!(cmds(&app.tick_at(Instant::now() + Duration::from_secs(60))).is_empty());
        assert!(!restores(&app.tick_at(Instant::now() + Duration::from_secs(61))), "owed once");
    }

    #[test]
    fn check_again_reprobes_on_a_403_latch_else_asks_keeping_the_tab() {
        let mut app = ready();
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        land(&mut app, PeerRecsEvent::Taste { seq, answer: Err(RecsFailure::Forbidden) });
        assert!(app.peer_recs_check_again().is_empty(), "nothing while the re-ping is out");
        probed(&mut app, true);
        // The 403 verdict goes and a new batch is asked, keeping the tab;
        // a 403 again re-pings again.
        let (seq, body) = one_taste(&app.peer_recs_check_again());
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
        assert!(body.get("pin").is_none());
        let effects = land(&mut app, PeerRecsEvent::Taste { seq, answer: Err(RecsFailure::Forbidden) });
        assert_eq!(probes(&effects), 1);
        probed(&mut app, true);
        // A scan pending answer (no 403): Check again asks, keeping the tab.
        let (seq, _) = one_taste(&app.peer_recs_check_again());
        answer(&mut app, seq, "taste_scan_pending");
        assert_eq!(app.peer_recs_pane(), rules::RecsPane::ScanPending);
        let (_, body) = one_taste(&app.peer_recs_check_again());
        assert_eq!(body["networkPeers"], json!([KITCHEN]));
    }

    #[test]
    fn a_clip_reprobe_that_says_off_closes_the_gate() {
        let mut app = ready();
        assert_eq!(probes(&app.peer_recs_reprobe(Reprobe::Clip)), 1);
        probed(&mut app, false);
        assert!(!app.peer_recs_gate());
        assert_eq!(app.peer_recs.taste.latch, TasteLatch::Off);
    }

    #[test]
    fn an_unrelated_error_clears_only_the_asking_flags() {
        let mut app = ready();
        playing(&mut app, 1);
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        let visit = app.peer_recs_visit();
        let t0 = Instant::now();
        app.tick_at(t0);
        assert_eq!(polls(&app.tick_at(t0 + Duration::from_secs(15))), vec![2]);
        app.apply_event(Event::Error("rename failed".into()));
        assert!(!app.peer_recs_asking(), "the next ask can go");
        assert_eq!(app.peer_recs_visit(), visit);
        assert!(app.peer_recs_clip_live(), "no clip ends");
        assert_eq!(app.peer_recs_leads().len(), 12, "the answer on screen stands");
        // The late answers still land.
        answer(&mut app, seq, "taste_kitchen");
        assert_eq!(app.peer_recs_leads().len(), 5);
        peers(&mut app, 2, "peers_admin_add_states");
        assert_eq!(tab_kind(&app, LIGHTHOUSE), Some(rules::StripKind::NoRoom));
        // A sheet's start out across the error: its late answer lands too.
        app.peer_recs_sheet_open(0);
        let start = cmds(&app.peer_recs_get_it(0))
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Start { seq, .. } => Some(seq),
                _ => None,
            })
            .unwrap();
        app.apply_event(Event::Error("rename failed".into()));
        let started = JobStarted { job: job(9, "running"), created: true };
        land(&mut app, PeerRecsEvent::Started { seq: start, answer: Ok(Box::new(started)) });
        let mark = app.peer_recs_get_it_rows()[0].mark.clone();
        assert!(matches!(mark, Some(JobOutcome::Running { .. })), "{mark:?}");
    }

    // ── The sheet ───────────────────────────────────────────────────────

    fn lookups(effects: &[Effect]) -> Vec<(u64, Value)> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Lookup { seq, rec } => Some((seq, *rec)),
                _ => None,
            })
            .collect()
    }

    fn job_reads(effects: &[Effect]) -> Vec<(u64, i64)> {
        cmds(effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Job { seq, id } => Some((seq, id)),
                _ => None,
            })
            .collect()
    }

    fn job(id: i64, state: &str) -> PluginJob {
        serde_json::from_value(json!({"id": id, "plugin": "noop-acquire", "state": state})).unwrap()
    }

    #[test]
    fn the_get_it_gate_needs_jobs_a_plugin_and_a_destination() {
        let mut app = ready();
        app.peer_recs_sheet_open(0);
        assert_eq!(app.peer_recs_get_it_rows().len(), 1);
        let seq = gates(&app.peer_recs_regate())[0];
        gate(&mut app, seq, listing(false), true);
        assert!(app.peer_recs_get_it_rows().is_empty(), "jobs not allowed");
        let seq = gates(&app.peer_recs_regate())[0];
        gate(&mut app, seq, listing(true), false);
        assert!(app.peer_recs_get_it_rows().is_empty(), "no destination");
        let mut only_copy = listing(true);
        only_copy.plugins.retain(|p| p.name != "noop-acquire");
        let copy = json!({"name": "federation-copy", "capabilities": ["acquire"]});
        only_copy.plugins.push(serde_json::from_value(copy).unwrap());
        let seq = gates(&app.peer_recs_regate())[0];
        gate(&mut app, seq, only_copy, true);
        assert!(app.peer_recs_get_it_rows().is_empty(), "the copy plug-in is not Get it");
        // Several plug-ins: a row each, titled.
        let mut two = listing(true);
        let other = json!({"name": "other", "title": "Other", "capabilities": ["acquire"]});
        two.plugins.push(serde_json::from_value(other).unwrap());
        let seq = gates(&app.peer_recs_regate())[0];
        gate(&mut app, seq, two, true);
        let rows = app.peer_recs_get_it_rows();
        assert_eq!(rows.iter().map(|r| r.plugin.as_str()).collect::<Vec<_>>(), ["noop-acquire", "other"]);
        assert_eq!(rows[1].title.as_deref(), Some("Other"));
    }

    #[test]
    fn the_sheet_looks_up_its_row_once_and_polls_a_live_job_every_1_5_s() {
        let mut app = ready();
        let effects = app.peer_recs_sheet_open(0);
        let asked = lookups(&effects);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].1, app.peer_recs_leads()[0].raw);
        assert!(app.peer_recs_sheet_open(0).is_empty(), "once");
        let lookup: JobLookup = serde_json::from_value(json!({"jobs": [
            {"id": 4, "plugin": "noop-acquire", "state": "failed", "createdAt": 1},
            {"id": 7, "plugin": "noop-acquire", "state": "running", "createdAt": 2}
        ]}))
        .unwrap();
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::new(lookup)) });
        assert!(app.peer_recs_get_it_rows()[0].mark.as_ref().is_some_and(JobOutcome::live));
        let t0 = Instant::now();
        assert!(job_reads(&app.tick_at(t0)).is_empty());
        let reads = job_reads(&app.tick_at(t0 + Duration::from_millis(1500)));
        assert_eq!(reads.iter().map(|r| r.1).collect::<Vec<_>>(), [7]);
        land(&mut app, PeerRecsEvent::Job { seq: reads[0].0, answer: Ok(Some(Box::new(job(7, "done")))) });
        assert!(!app.peer_recs_get_it_rows()[0].mark.as_ref().unwrap().live());
        let t1 = t0 + Duration::from_secs(2);
        app.tick_at(t1);
        assert!(job_reads(&app.tick_at(t1 + Duration::from_secs(5))).is_empty(), "a finished job is not read");
    }

    #[test]
    fn a_get_it_press_starts_one_job_and_a_changed_destination_reads_the_gate_again() {
        let mut app = ready();
        let asked = lookups(&app.peer_recs_sheet_open(2));
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::default()) });
        let effects = app.peer_recs_get_it(0);
        let starts: Vec<(u64, String, Value, Option<CollectionDestination>)> = cmds(&effects)
            .into_iter()
            .filter_map(|c| match c {
                PeerRecsCmd::Start { seq, plugin, rec, destination } => Some((seq, plugin, *rec, destination)),
                _ => None,
            })
            .collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(starts[0].1, "noop-acquire");
        assert_eq!(starts[0].2, app.peer_recs_leads()[2].raw);
        assert_eq!(starts[0].3.as_ref().map(|d| d.vpath.as_str()), Some("music"));
        assert!(app.peer_recs_get_it(0).is_empty(), "one job at a time");
        let changed = PluginFailure::DestinationChanged(Some(None));
        let effects = land(&mut app, PeerRecsEvent::Started { seq: starts[0].0, answer: Err(changed) });
        assert_eq!(gates(&effects).len(), 1);
        assert!(app.peer_recs_get_it_rows().is_empty(), "nowhere to land: Get it hides");
        // A start that lands joins the reads; a kept song is inert.
        let mut app = ready();
        app.peer_recs_sheet_open(2);
        let start = cmds(&app.peer_recs_get_it(0))
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Start { seq, .. } => Some(seq),
                _ => None,
            })
            .unwrap();
        let mut kept = job(9, "done");
        kept.result = Some(json!({"filepath": "music/a.mp3"}));
        let started = JobStarted { job: kept, created: true };
        land(&mut app, PeerRecsEvent::Started { seq: start, answer: Ok(Box::new(started)) });
        assert!(app.peer_recs_get_it_rows()[0].mark.is_some());
        // Failed starts again.
        app.peer_recs.sheet.as_mut().unwrap().jobs.insert("noop-acquire".into(), job(9, "failed"));
        assert_eq!(cmds(&app.peer_recs_get_it(0)).len(), 1);
    }

    /// A Get it start sent while the sheet's lookup is out lands first: the
    /// lookup, read before that job existed, never puts its older job over
    /// it, and the new job's reads go on.
    #[test]
    fn a_lookup_never_overwrites_a_newer_job_its_start_returned() {
        let mut app = ready();
        let asked = lookups(&app.peer_recs_sheet_open(2));
        let start = cmds(&app.peer_recs_get_it(0))
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Start { seq, .. } => Some(seq),
                _ => None,
            })
            .expect("Get it starts while the lookup is out");
        let started = JobStarted { job: job(12, "running"), created: true };
        land(&mut app, PeerRecsEvent::Started { seq: start, answer: Ok(Box::new(started)) });
        let lookup: JobLookup = serde_json::from_value(json!({"jobs": [
            {"id": 4, "plugin": "noop-acquire", "state": "failed", "createdAt": 1}
        ]}))
        .unwrap();
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::new(lookup)) });
        let mark = app.peer_recs_get_it_rows()[0].mark.clone();
        assert!(mark.as_ref().is_some_and(JobOutcome::live), "{mark:?}");
        let t0 = Instant::now();
        app.tick_at(t0);
        let reads = job_reads(&app.tick_at(t0 + Duration::from_millis(1500)));
        assert_eq!(reads.iter().map(|r| r.1).collect::<Vec<_>>(), [12]);
    }

    #[test]
    fn a_sheet_answer_for_a_closed_sheet_is_dropped() {
        let mut app = ready();
        let asked = lookups(&app.peer_recs_sheet_open(0));
        app.peer_recs_sheet_close();
        let lookup = JobLookup { jobs: vec![job(7, "running")], ..JobLookup::default() };
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::new(lookup.clone())) });
        app.peer_recs_sheet_open(1);
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::new(lookup)) });
        assert_eq!(app.peer_recs_get_it_rows()[0].mark, None);
    }

    #[test]
    fn open_elsewhere_feeds_the_raw_lead_to_keeps_links() {
        let mut app = ready();
        let effects = app.peer_recs_open_elsewhere(3);
        let asked: Vec<(String, Value)> = effects
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::PluginLinks { plugin, rec, .. }) => Some((plugin.clone(), (**rec).clone())),
                _ => None,
            })
            .collect();
        assert_eq!(asked, vec![("links".to_string(), app.peer_recs_leads()[3].raw.clone())]);
        let asking = crate::tui::app::keep::LinksStatus::Asking;
        assert!(app.keep.links.as_ref().is_some_and(|l| l.status == asking));
        // Try again asks the same plug-in and row again.
        let again = app.links_retry();
        let links = |e: &Effect| matches!(e, Effect::Api(ApiCmd::PluginLinks { plugin, .. }) if plugin == "links");
        assert!(again.iter().any(links));
    }

    #[test]
    fn the_sheet_state_follows_its_lead_key_across_a_reshuffle() {
        let mut app = ready();
        let lead = app.peer_recs_leads()[3].clone();
        let key = rules::lead_key(&lead);
        let asked = lookups(&app.peer_recs_sheet_open(3));
        let lookup = JobLookup { jobs: vec![job(7, "running")], ..JobLookup::default() };
        land(&mut app, PeerRecsEvent::Lookup { seq: asked[0].0, answer: Ok(Box::new(lookup)) });
        // A new answer moves the lead to the top.
        let mut moved = fixtures::taste("taste_all");
        let results = &mut moved.network.as_mut().unwrap().results;
        let at = results.iter().position(|l| rules::lead_key(l) == key).unwrap();
        let row = results.remove(at);
        results.insert(0, row);
        let (seq, _) = one_taste(&app.peer_recs_retry());
        answer_with(&mut app, seq, moved);
        assert_eq!(app.peer_recs_lead_index(&key), Some(0));
        assert!(app.peer_recs_sheet_open(0).is_empty(), "the same lead: the sheet stands");
        assert!(app.peer_recs_get_it_rows()[0].mark.as_ref().is_some_and(JobOutcome::live));
    }

    // ── The bar, the pick and Discover ──────────────────────────────────

    fn track(path: &str) -> Track {
        Track { filepath: path.to_string(), metadata: Default::default() }
    }

    #[test]
    fn a_seeds_modal_pick_lands_in_the_room_that_armed_it() {
        let mut app = ready();
        app.peer_recs_choose_song();
        assert!(app.peer_recs_picking());
        let effects = app.answer_capture(Capture::TasteSeed, track("music/chosen.mp3"));
        let (_, body) = one_taste(&effects);
        assert_eq!(body["pin"], json!(["music/chosen.mp3"]));
        assert!(!app.peer_recs_picking());
        assert!(app.taste.pins.is_empty(), "Discover's pins are untouched");
        // Discover's own pick still lands in Discover.
        app.taste_choose_song();
        let effects = app.answer_capture(Capture::TasteSeed, track("music/mine.mp3"));
        assert!(tastes(&effects).is_empty());
        assert_eq!(app.taste.pins.len(), 1);
        assert_eq!(app.peer_recs.taste.pins.len(), 1);
    }

    #[test]
    fn a_refused_room_pick_stays_in_the_room() {
        let mut app = ready();
        let seed = "testlib/Vosto/Night Drive/01 - Highway.mp3";
        app.peer_recs_choose_song();
        let effects = app.answer_capture(Capture::TasteSeed, track(seed));
        assert!(effects.is_empty());
        assert_eq!(app.peer_recs.taste.refused, Some(TastePinRefusal::AlreadyThere));
        assert_eq!(app.taste.refused, None);
    }

    #[test]
    fn picking_holds_only_while_the_capture_does() {
        let mut app = ready();
        app.peer_recs_choose_song();
        assert!(app.peer_recs_picking());
        // A cancel lets the capture go.
        app.capture = None;
        assert!(!app.peer_recs_picking());
        app.tick_at(Instant::now());
        assert!(!app.peer_recs.picking);
        // A landed pick.
        app.peer_recs_choose_song();
        app.answer_capture(Capture::TasteSeed, track("music/a.mp3"));
        assert!(!app.peer_recs.picking);
        // Discover's own pick.
        app.peer_recs_choose_song();
        app.taste_choose_song();
        assert!(!app.peer_recs.picking);
        assert_eq!(app.capture, Some(Capture::TasteSeed));
    }

    #[test]
    fn the_head_is_the_batch_and_a_tab_answer_never_replaces_it() {
        let mut app = room_app();
        let effects = app.peer_recs_open(None);
        peers(&mut app, polls(&effects)[0], "peers_admin");
        assert!(app.peer_recs.taste.main_pending);
        answer(&mut app, one_taste(&effects).0, "taste_all");
        assert!(!app.peer_recs.taste.main_pending);
        let head = app.peer_recs.taste.head.clone().unwrap();
        assert!(head.network.as_ref().unwrap().results.is_empty());
        assert_eq!(head.seeds.len(), 2);
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        assert!(!app.peer_recs.taste.main_pending, "a tab's ask is not the batch's");
        answer(&mut app, seq, "taste_kitchen");
        assert_eq!(app.peer_recs.taste.head, Some(head));
        // A new batch's ask is.
        app.peer_recs_new_seeds();
        assert!(app.peer_recs.taste.main_pending);
        assert!(app.peer_recs.taste.head.is_none());
    }

    #[test]
    fn discover_is_untouched_by_the_room() {
        let mut app = room_app();
        app.taste.window = Some(7);
        let before = format!("{:?}", app.taste);
        let effects = app.peer_recs_open(Some(90));
        answer(&mut app, one_taste(&effects).0, "taste_all");
        peers(&mut app, polls(&effects)[0], "peers_admin");
        gate(&mut app, gates(&effects)[0], listing(true), true);
        let (seq, _) = one_taste(&app.peer_recs_select(Some(KITCHEN.to_string())));
        answer(&mut app, seq, "taste_kitchen");
        app.peer_recs_new_seeds();
        app.peer_recs_set_window(30);
        app.peer_recs_seeds_closed(&["dbd0ae2c76e2117f94b7e34970912a7e".to_string()], &[], false);
        app.peer_recs_add(LIGHTHOUSE);
        app.peer_recs_sheet_open(0);
        app.peer_recs_sheet_close();
        app.peer_recs_choose_song();
        app.answer_capture(Capture::TasteSeed, track("music/x.mp3"));
        app.peer_recs_reprobe(Reprobe::Taste);
        probed(&mut app, true);
        app.peer_recs_watch(PeerRecsWatch::Away);
        assert_eq!(format!("{:?}", app.taste), before);
    }

    #[test]
    fn the_seeds_modal_pins_the_room_and_reads_the_rooms_rules() {
        let mut app = ready();
        assert!(!app.peer_recs_can_pin_playing(), "nothing plays");
        assert!(!app.peer_recs_unpin("music/none.mp3"));
        app.peer_recs.taste.pins.push(TastePin { track: track("music/p.mp3") });
        assert!(app.peer_recs_unpin("music/p.mp3"));
    }
}
