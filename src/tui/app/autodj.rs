//! Auto DJ: the lane, its turns, and the tab that shapes them
//! (docs/ux-contracts/auto-dj.md).
//!
//! The DJ is armed FOR a server, whichever one the session browses (clause
//! 19): every turn rides that server's own reach, its capabilities come from
//! a probe of its own, and its tunnel is kept up while the DJ stands. What
//! the DJ learns from the queue it is working — the cursor, the Camelot
//! anchor, the sonic history and pin, the warning budgets — is the **lane**,
//! reset whenever the user steers somewhere new and stamped with an epoch
//! so a reply from a dead lane is dropped (clauses 10 and 11).
//!
//! Most of App's DJ fields are serviced from here and nowhere else (audit
//! #57). The rule that matters lives in [`App::consume_dj`]: a reply in
//! hand means its request is no longer in flight, decided in one place.

use super::*;
use crate::tui::worker::{DjNote, dj_log};
use rust_i18n::t;

/// The one-shot opening seed (clause 5): the track that opens a session,
/// spent by the clear that starts it, so no later empty-queue start can
/// replay it.
#[derive(Debug, Clone, PartialEq)]
pub struct DjSeed {
    pub track: Track,
    /// The server the track lives on — what the DJ arms for.
    pub server: String,
    /// Whether the DJ chose it (the opener) or the user did.
    pub picked_by_dj: bool,
}

/// A session lane (clause 10): what the DJ has learned from the queue it is
/// working, forgotten whenever the user steers somewhere new.
#[derive(Debug, Default)]
pub struct DjLane {
    /// Bumped on every reset; a reply under another epoch is dropped.
    pub epoch: u64,
    /// The round-trip cursor (clause 24).
    pub ignore: Vec<u32>,
    /// The Camelot anchor, locked on the first keyed pick (clause 22).
    pub camelot_anchor: Option<String>,
    /// DJ-picked paths on the DJ's server, oldest first, at most
    /// [`dj::HISTORY_LEN`] — the rolling anchor (clause 23).
    pub history: Vec<String>,
    /// The locked anchor's pin (clause 23).
    pub locked_pin: Option<String>,
    /// A turn is out; the next trigger waits for it (clause 14).
    pub pending: bool,
    /// A turn that could not be asked for — the network, a tunnel down —
    /// retried when the way back opens (clauses 33 and 35).
    pub owed: bool,
    /// The opener is the turn that is out (clause 3).
    pub opener: bool,
    /// A seeded start still owes a second fetch when the first left the
    /// queue under three deep (clause 7).
    pub followers: bool,
    /// The once-per-lane budgets (clauses 30, 32, 36).
    pub warned_sonic: bool,
    pub warned_auth: bool,
    pub logged_not_ready: bool,
    pub logged_deferred: bool,
    /// Why this lane is picking without sonic similarity, once it had to
    /// let the pool go (clause 30) — what the room's row says in gold
    /// until the sonic settings change or the lane ends.
    pub sonic_drop: Option<DjNote>,
}

impl DjLane {
    /// A new lane: everything forgotten, the epoch moved on.
    pub fn reset(&mut self) {
        let epoch = self.epoch + 1;
        *self = DjLane { epoch, ..Default::default() };
    }
}

/// One edit from a room's control, in either shell (clause 51). A step on
/// a switch toggles it, on a radio cycles it, on a number moves it by a
/// useful increment; the GUI's bars and radios also set outright.
#[derive(Debug, Clone, PartialEq)]
pub enum DjEdit {
    Step(DjRow, i32),
    /// A bar's absolute value — songs, BPM, artists, a rating in halves,
    /// seconds — and hundredths of cosine for the strictness bar.
    Set(DjRow, u32),
    EmptyQueue(dj::EmptyQueueStart),
    /// The TUI row's ←→: off → whitelist → blacklist → off, one key. The
    /// switch and the mode are two fields all the same (clause 48; the
    /// record's `autoDJGenreMode` beside `autoDJGenreEnabled`): switching
    /// off keeps whitelist or blacklist for the next switch on.
    GenreCycle,
    /// A genre in or out of the chosen set.
    Genre(String),
    AddKeyword(String),
    RemoveKeyword(String),
    // The GUI room's sheets: each closes on one edit of everything it holds.
    /// Sonic similarity: the switch, the floor in hundredths of cosine,
    /// and the anchor. A lane that had dropped the pool gets another go.
    Sonic { on: bool, floor: u32, anchor: dj::SonicAnchor },
    /// Smart filters: nothing chosen is the filter off.
    Moods(crate::api::types::MoodFilter),
    /// Beat matching: the switch and ± BPM.
    Tempo { on: bool, tolerance: u32 },
    /// Length, to the second: a side left `None` has no limit; both `None`
    /// is Any, which is the window off.
    LengthWindow { min: Option<u32>, max: Option<u32>, unknown: bool },
    /// Skip words: the whole list; empty is the filter off.
    Keywords(Vec<String>),
    /// Genres, for the library the rules are for: the way the list cuts,
    /// the kind of genre it matches, and the list — empty is the filter off.
    GenreRule { mode: dj::GenreMode, source: GenreSource, genres: Vec<String> },
}

/// The empty-queue chooser (clause 2): "Start Auto DJ with what?" — Surprise
/// me, Let me choose, and the Remember-this box.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DjChooser {
    pub row: usize,
    pub remember: bool,
    /// Recently played's list is down, and the row in it the cursor is on.
    pub recent_open: bool,
    pub recent_row: usize,
    /// The list came down on its own once (the remembered way to start),
    /// so a list folded since stays folded.
    pub recent_seated: bool,
}

/// The chooser's rows: the two answers, then the remember box.
pub const DJ_CHOOSER_ROWS: usize = 3;

/// Why the DJ's server cannot do one of the room's settings — said in a
/// few words where the row's value would be (the record's rows never move:
/// a setting the server can't do keeps its place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DjGate {
    /// The server is known to predate the release that brought it.
    NeedsServer((u32, u32, u32)),
    DiscoveryOff,
    StillScanning,
    NoMoodData,
    NotOnPeers,
}

/// The TUI's Longest row on its 0:00–20:00 walk: the walk's end is "no
/// maximum" (the rail), one step under it 19:45; a longer bound — set in
/// the GUI's sheet — steps as it is.
fn step_longest(now: u32, by: i32, floor: u32) -> u32 {
    let top = dj::LENGTH_LEGACY_RAIL_SECONDS;
    if now >= dj::LENGTH_RAIL_SECONDS {
        return if by < 0 { (top as i32 + by).max(floor as i32) as u32 } else { now };
    }
    let next = (now as i32 + by).max(floor as i32) as u32;
    if (now < top && next >= top) || next >= dj::LENGTH_RAIL_SECONDS { dj::LENGTH_RAIL_SECONDS } else { next }
}

impl App {
    // ── Where the DJ stands ─────────────────────────────────────────────────

    /// Whether the DJ is armed on any server.
    pub fn dj_armed(&self) -> bool {
        self.dj_server.is_some()
    }

    /// The session's identity — what the toggle arms for — while a session
    /// is up.
    pub(crate) fn session_identity(&self) -> Option<String> {
        if !self.connected {
            return None;
        }
        let id = if self.session.server_id.is_empty() {
            self.session.server.clone()
        } else {
            self.session.server_id.clone()
        };
        (!id.is_empty()).then_some(id)
    }

    /// A server identity as the origin its rows carry: a peer names its
    /// parent and its id there, anything else names itself.
    pub(crate) fn origin_of(&self, identity: &str) -> Origin {
        if let Some(known) =
            self.servers.iter().find(|s| crate::config::same_server(&s.id, identity))
            && let Some((parent, id)) = &known.peer
        {
            return Origin { server: parent.clone(), peer: Some(*id) };
        }
        if self.session_identity().is_some_and(|s| crate::config::same_server(&s, identity)) {
            return self.origin();
        }
        Origin { server: identity.to_string(), peer: None }
    }

    /// The DJ's server as an origin.
    pub(crate) fn dj_origin(&self) -> Option<Origin> {
        self.dj_server.as_deref().map(|id| self.origin_of(id))
    }

    /// Whether the DJ's server is a federated peer (no rating row, no
    /// `minRating` — clause 6).
    pub(crate) fn dj_is_peer(&self) -> bool {
        self.dj_origin().is_some_and(|origin| origin.peer.is_some())
    }

    /// What to call the DJ's server.
    pub fn dj_server_name(&self) -> String {
        let Some(id) = self.dj_server.as_deref() else { return String::new() };
        self.server_name_of(id)
    }

    /// How a server is reached right now: `Ok(None)` is the session's own
    /// client, `Ok(Some(_))` a reach of its own, `Err` a tunnel not up yet.
    fn reach_for(&self, identity: &str) -> Result<Option<Reach>, String> {
        let origin = self.origin_of(identity);
        if self.is_session_origin(&origin) {
            return Ok(None);
        }
        self.reach(&origin).map(Some)
    }

    /// The rules of a server's library (clause 51): its own entry's, with
    /// the session-wide fallbacks behind them.
    fn library_for(&self, identity: &str) -> dj::LibraryFilters {
        let known = self.servers.iter().find(|s| crate::config::same_server(&s.id, identity));
        let own = known.map(|s| s.dj.clone()).unwrap_or_default();
        let is_peer = self.origin_of(identity).peer.is_some();
        dj::LibraryFilters::resolve(&self.dj, &own, is_peer)
    }

    /// Whether a pool may be asked of the DJ's server (clauses 23 and 36):
    /// the switch, and the server's own discovery and readiness.
    pub fn dj_sonic_usable(&self) -> bool {
        self.dj.sonic && self.dj_info.as_ref().is_some_and(|info| info.sonic_usable())
    }

    /// Why the sonic row is disabled, in the room's words (clause 43) — or
    /// `None` while it can be honoured.
    pub fn dj_sonic_reason(&self) -> Option<String> {
        let info = self.dj_info.as_ref()?;
        if dj::known_older(info.version.as_deref(), dj::FLOOR_SONIC) {
            return Some(t!("dj.sonic_needs_newer").to_string());
        }
        if !info.discovery {
            return Some(t!("dj.sonic_unavailable").to_string());
        }
        if info.discovery_ready == Some(false) {
            return Some(t!("dj.sonic_not_ready").to_string());
        }
        None
    }

    /// Why sonic similarity cannot run on the DJ's server (the GUI room's
    /// row): it predates it, its scan has produced no vectors yet, or
    /// discovery is off there. `None` while it can — or nobody knows yet.
    pub fn dj_sonic_gate(&self) -> Option<DjGate> {
        let info = self.dj_info.as_ref()?;
        if dj::known_older(info.version.as_deref(), dj::FLOOR_SONIC) {
            return Some(DjGate::NeedsServer(dj::FLOOR_SONIC));
        }
        if info.discovery && info.discovery_ready == Some(false) {
            return Some(DjGate::StillScanning);
        }
        (!info.discovery).then_some(DjGate::DiscoveryOff)
    }

    /// Why Smart filters cannot narrow the picks there: a peer's key has
    /// no say over them, or the server has no mood scores.
    pub fn dj_mood_gate(&self) -> Option<DjGate> {
        if self.dj_target_is_peer() {
            return Some(DjGate::NotOnPeers);
        }
        let info = self.dj_info.as_ref()?;
        (!info.mood_filters).then_some(DjGate::NoMoodData)
    }

    /// Whether `moods` may ride a pick: the server SAID it has scores. One
    /// that has not answered yet is not sent the key.
    fn dj_mood_scores(&self) -> bool {
        !self.dj_target_is_peer() && self.dj_info.as_ref().is_some_and(|info| info.mood_filters)
    }

    /// A setting the DJ's server is KNOWN to predate (clause 50).
    pub fn dj_floor_gate(&self, floor: (u32, u32, u32)) -> Option<DjGate> {
        let version = self.dj_info.as_ref().and_then(|info| info.version.as_deref());
        dj::known_older(version, floor).then_some(DjGate::NeedsServer(floor))
    }

    /// Whether the library the room's rules are for is a federated peer's.
    pub fn dj_target_is_peer(&self) -> bool {
        self.dj_library_target().is_some_and(|id| self.origin_of(&id).peer.is_some())
    }

    /// The sonic settings changed: a lane that had dropped the pool gets
    /// another go with them (the record's `retrySonic`).
    fn dj_retry_sonic(&mut self) {
        if self.lane.sonic_drop.take().is_some() {
            self.lane.warned_sonic = false;
        }
        if let Some(identity) = self.dj_server.as_deref() {
            crate::tui::worker::dj_retry_sonic(identity, self.lane.epoch);
        }
    }

    // ── The toggle (entry point 1) ──────────────────────────────────────────

    /// The record's toggle (re-extracted 2026-11-10): on → off, wherever
    /// the DJ is armed — Stop always stops; off → armed for Play from's
    /// server, whichever one is browsed — the browsed one itself in a
    /// shell with no Play from. (Before, a DJ armed elsewhere was moved to
    /// the browsed server, so one button meant two things.)
    pub(super) fn toggle_autodj(&mut self) -> Vec<Effect> {
        if self.dj_armed() {
            return self.disarm_dj();
        }
        let Some(target) = self.dj_play_from() else {
            self.info(t!("dj.needs_server"));
            return Vec::new();
        };
        if self.queue.items.is_empty() { self.begin_empty_start(target) } else { self.arm_dj(target) }
    }

    /// Play from's server while the DJ is off — where Start arms it: the
    /// home (the server chosen in the room, or the one the DJ last ran on)
    /// while it is still a saved server, else the session's. A shell that
    /// shows no Play from has no home to start on: the session's it is.
    pub(crate) fn dj_play_from(&self) -> Option<String> {
        self.dj_home
            .as_ref()
            .filter(|_| self.dj_shows_play_from)
            .filter(|home| self.servers.iter().any(|s| crate::config::same_server(&s.id, home)))
            .cloned()
            .or_else(|| self.session_identity())
    }

    /// Play from names a server (the room's sheet): the home either way; a
    /// DJ running somewhere else starts afresh there, one already there
    /// carries on, and a DJ that is off stays off.
    pub(crate) fn dj_play_from_set(&mut self, identity: String) -> Vec<Effect> {
        if self.dj_armed() {
            return self.dj_move_to(identity);
        }
        if self.dj_play_from().is_some_and(|now| crate::config::same_server(&now, &identity)) {
            self.dj_home = Some(identity);
            return Vec::new();
        }
        self.dj_home = Some(identity);
        // Off, the rows describe the server Start would use: ask it.
        self.dj_info = None;
        self.dj_panel.rebuild(&self.dj, None, self.dj_is_peer());
        self.dj_room_opened()
    }

    /// Arm the DJ for `identity` (clause 1): a fresh lane when it is a
    /// different server, the probe of what that server offers, and the
    /// queue's end looked at.
    pub(crate) fn arm_dj(&mut self, identity: String) -> Vec<Effect> {
        let fresh = !self.dj_server.as_deref().is_some_and(|s| crate::config::same_server(s, &identity));
        if fresh {
            self.lane.reset();
            self.forget_dj_info_unless_for(&identity);
        }
        // Play from always holds the server the DJ runs on, as its home.
        self.dj_home = Some(identity.clone());
        self.dj_server = Some(identity);
        self.dj_chooser = None;
        // An opener still out was asked for another start: its target must
        // not re-arm the DJ elsewhere when it lands.
        self.dj_target = None;
        self.say_armed();
        let mut effects = self.probe_dj();
        effects.extend(self.dj_look_at_the_end());
        effects
    }

    /// The DJ is about to stand on `identity`: what the room knew of
    /// another server goes, and what it already asked of this one — the
    /// room open with the DJ off probes Play from's server — stays, so
    /// the rows do not blink to unknown and back on Start.
    fn forget_dj_info_unless_for(&mut self, identity: &str) {
        let same = self.dj_library_target().is_some_and(|target| crate::config::same_server(&target, identity));
        if !same {
            self.dj_info = None;
        }
    }

    fn say_armed(&mut self) {
        let name = self.dj_server_name();
        if self.servers.len() > 1 {
            self.info(t!("dj.on_from", server = name));
        } else {
            self.info(t!("dj.on"));
        }
    }

    /// With a queue the DJ follows what is there (clause 1): on the last row
    /// and idle, a turn whose first song plays at once; on the last row and
    /// playing, a turn as the next rows; anywhere else nothing until the
    /// queue's end.
    fn dj_look_at_the_end(&mut self) -> Vec<Effect> {
        if self.queue.items.is_empty() {
            return Vec::new();
        }
        let last = self.queue.items.len() - 1;
        match self.queue.current {
            Some(index) if index == last => self.request_turn(),
            None if self.queue.items.len() == 1 => self.request_turn(),
            _ => Vec::new(),
        }
    }

    /// Off: the lane ends with it.
    pub(crate) fn disarm_dj(&mut self) -> Vec<Effect> {
        self.dj_server = None;
        self.dj_info = None;
        self.dj_chooser = None;
        self.dj_target = None;
        self.lane.reset();
        if self.capture == Some(Capture::DjSeed) {
            self.capture = None;
        }
        self.info(t!("dj.off"));
        Vec::new()
    }

    /// A removed server takes its DJ with it (contract clause 35 of the
    /// multi-server contract, and this one's clause 10).
    pub(crate) fn dj_server_removed(&mut self, server: &str) -> Vec<Effect> {
        let Some(origin) = self.dj_origin() else { return Vec::new() };
        let gone = crate::config::same_server(&origin.server, server)
            || self.dj_server.as_deref().is_some_and(|id| crate::config::same_server(id, server));
        if gone {
            return self.disarm_dj();
        }
        Vec::new()
    }

    // ── The empty queue (clauses 2–5, 7 and 16) ─────────────────────────────

    /// Switching on with nothing queued: the opening question, or the
    /// answer the user asked to remember.
    fn begin_empty_start(&mut self, identity: String) -> Vec<Effect> {
        self.dj_target = Some(identity.clone());
        match self.dj.empty_queue {
            // Recently played names a song, so it asks every time too —
            // with its list already down once the songs are in hand.
            dj::EmptyQueueStart::Ask | dj::EmptyQueueStart::Recent => {
                let recent_open = self.dj.empty_queue == dj::EmptyQueueStart::Recent;
                self.dj_chooser = Some(DjChooser { recent_open, ..Default::default() });
                self.dj_want_recent(&identity)
            }
            dj::EmptyQueueStart::Random => self.dj_surprise(),
            dj::EmptyQueueStart::Pick => self.dj_pick_from_library(),
        }
    }

    /// Ask a server for the songs last played on it (Recently played): the
    /// list in hand, if it is that server's, stands until the new one
    /// lands. A peer is not asked — a key or a guest has no history there —
    /// and nor is a server that cannot be reached right now.
    pub(crate) fn dj_want_recent(&mut self, identity: &str) -> Vec<Effect> {
        let kept = self
            .dj_recent
            .take()
            .filter(|(whose, _)| crate::config::same_server(whose, identity))
            .and_then(|(_, list)| list);
        if self.origin_of(identity).peer.is_some() {
            self.dj_recent = Some((identity.to_string(), Some(Vec::new())));
            return Vec::new();
        }
        match self.reach_for(identity) {
            Ok(reach) => {
                self.dj_recent = Some((identity.to_string(), kept));
                vec![Effect::Api(ApiCmd::DjRecent { identity: identity.to_string(), reach })]
            }
            Err(_) => {
                self.dj_recent = Some((identity.to_string(), Some(kept.unwrap_or_default())));
                Vec::new()
            }
        }
    }

    /// The songs Recently played offers for the server an opener is for —
    /// none while the list is out, or when there is nothing to offer.
    pub fn dj_recent_songs(&self) -> &[Track] {
        let Some(target) = self.opener_target() else { return &[] };
        match &self.dj_recent {
            Some((whose, Some(list))) if crate::config::same_server(whose, &target) => list,
            _ => &[],
        }
    }

    /// One of Recently played's songs opens the session (the chooser's
    /// list, or the armed empty queue's): it is the seed, on the server it
    /// was played on. What is remembered is the way to start, not the song.
    pub(crate) fn answer_dj_start_recent(&mut self, index: usize, remember: bool) -> Vec<Effect> {
        let Some(track) = self.dj_recent_songs().get(index).cloned() else { return Vec::new() };
        let Some(server) = self.opener_target() else { return Vec::new() };
        if remember {
            self.dj.empty_queue = dj::EmptyQueueStart::Recent;
        }
        self.dj_chooser = None;
        self.start_from_seed(DjSeed { track, server, picked_by_dj: false })
    }

    /// The server an opener is for: the one the toggle was pressed on, or
    /// the DJ's own when it is already armed (the empty state's buttons,
    /// clause 16), else the session's.
    fn opener_target(&self) -> Option<String> {
        self.dj_target.clone().or_else(|| self.dj_server.clone()).or_else(|| self.session_identity())
    }

    /// "Surprise me" (clause 3): the filtered opener, one song, from the
    /// target server.
    pub(super) fn dj_surprise(&mut self) -> Vec<Effect> {
        self.dj_chooser = None;
        let Some(identity) = self.opener_target() else { return Vec::new() };
        self.dj_target = Some(identity.clone());
        let reach = match self.reach_for(&identity) {
            Ok(reach) => reach,
            Err(why) => {
                self.error(t!("dj.fetch_failed_why", why = why));
                self.dj_target = None;
                return Vec::new();
            }
        };
        let mut ask = self.compose_ask(&identity);
        ask.opener = true;
        ask.ignore_list.clear();
        self.lane.opener = true;
        self.lane.pending = true;
        self.info(t!("dj.picking_opener"));
        vec![Effect::Api(ApiCmd::AutoDj(Box::new(DjRequest {
            identity,
            reach,
            epoch: self.lane.epoch,
            ask,
        })))]
    }

    /// "Let me choose" (clause 4): the library, under a banner; the DJ
    /// switches on when a row lands, not before.
    pub(super) fn dj_pick_from_library(&mut self) -> Vec<Effect> {
        self.dj_chooser = None;
        let effects = self.arm_capture(Capture::DjSeed);
        self.info(t!("dj.pick_banner"));
        effects
    }

    /// The row the "Let me choose" road landed on: the seed, and the DJ arms
    /// for the server it was chosen from (paths are per-library).
    pub(super) fn dj_seed_chosen(&mut self, track: Track) -> Vec<Effect> {
        let Some(server) = self.session_identity() else { return Vec::new() };
        self.start_from_seed(DjSeed { track, server, picked_by_dj: false })
    }

    /// A seed opens exactly one session (clauses 5 and 7): the DJ arms for
    /// the seed's server, the queue becomes the seed, it plays, and the
    /// followers are asked for — one fetch, a second only if the first
    /// left the queue under three deep.
    fn start_from_seed(&mut self, seed: DjSeed) -> Vec<Effect> {
        self.dj_target = None;
        let fresh = !self.dj_server.as_deref().is_some_and(|s| crate::config::same_server(s, &seed.server));
        if fresh {
            self.lane.reset();
            self.forget_dj_info_unless_for(&seed.server);
        }
        self.dj_server = Some(seed.server.clone());
        self.dj_home = Some(seed.server.clone());
        // The clear that starts it consumes the seed: a new lane, and nothing
        // left for a later empty-queue start to replay.
        self.lane.reset();
        let origin = self.origin_of(&seed.server);
        let mark = seed.picked_by_dj.then_some(DjMark { sonic: false });
        let row = Queued { origin, dj: mark, track: seed.track };
        // The seed opens an empty queue; a queue built while the opener
        // was out (or while the pick's banner stood) stays, and the seed
        // joins its end and plays from there — never wiped from under the
        // user (clause 5's "the queue becomes the seed" is the empty case).
        let index = if self.queue.items.is_empty() {
            self.queue.replace(vec![row]);
            0
        } else {
            self.queue.push(row);
            self.queue.items.len() - 1
        };
        self.say_armed();
        let mut effects = self.probe_dj();
        effects.extend(self.play_index(index));
        self.lane.followers = true;
        effects.extend(self.request_turn());
        effects
    }

    /// The chooser's keys (clause 2): ↑↓ walk its three rows, Space toggles
    /// the remember box, Enter answers, Esc leaves the DJ off.
    pub(super) fn handle_dj_chooser_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        let Some(chooser) = self.dj_chooser.as_mut() else { return Vec::new() };
        match action {
            Action::Up => chooser.row = chooser.row.saturating_sub(1),
            Action::Down => chooser.row = (chooser.row + 1).min(DJ_CHOOSER_ROWS - 1),
            Action::First => chooser.row = 0,
            Action::Last => chooser.row = DJ_CHOOSER_ROWS - 1,
            Action::PlayPause => chooser.remember = !chooser.remember,
            Action::Cancel => {
                self.dj_chooser = None;
                self.dj_target = None;
            }
            Action::Activate | Action::Submit => {
                let (row, remember) = (chooser.row, chooser.remember);
                return match row {
                    0 => self.answer_dj_start(dj::EmptyQueueStart::Random, remember),
                    1 => self.answer_dj_start(dj::EmptyQueueStart::Pick, remember),
                    _ => {
                        chooser.remember = !chooser.remember;
                        Vec::new()
                    }
                };
            }
            _ => {}
        }
        Vec::new()
    }

    /// One of the chooser's two answers, remembered when asked (clause 2).
    pub(crate) fn answer_dj_start(&mut self, choice: dj::EmptyQueueStart, remember: bool) -> Vec<Effect> {
        if remember {
            self.dj.empty_queue = choice;
        }
        self.dj_chooser = None;
        match choice {
            dj::EmptyQueueStart::Random => self.dj_surprise(),
            dj::EmptyQueueStart::Pick => self.dj_pick_from_library(),
            // Recently played answers with a song: `answer_dj_start_recent`.
            dj::EmptyQueueStart::Ask | dj::EmptyQueueStart::Recent => Vec::new(),
        }
    }

    // ── Turns (clauses 13–15, 19–27) ────────────────────────────────────────

    /// The top-up rule (clause 13): exactly when a real playback session
    /// stands on the last row — not idle, not walking failed tracks. The
    /// callers are the two index changes: a track started, a track adopted.
    pub(super) fn maybe_autodj(&mut self) -> Vec<Effect> {
        if !self.dj_armed() || self.lane.pending {
            return Vec::new();
        }
        let Some(current) = self.queue.current else { return Vec::new() };
        if current + 1 < self.queue.items.len() {
            return Vec::new();
        }
        if self.status.is_idle() || self.failures > 0 || self.retries > 0 {
            return Vec::new();
        }
        self.request_turn()
    }

    /// One turn on the DJ's server — or an owed one, when its tunnel is
    /// not up (clause 35).
    pub(super) fn request_turn(&mut self) -> Vec<Effect> {
        let Some(identity) = self.dj_server.clone() else { return Vec::new() };
        if self.lane.pending {
            return Vec::new();
        }
        let reach = match self.reach_for(&identity) {
            Ok(reach) => reach,
            Err(why) => {
                self.lane.owed = true;
                if !self.lane.logged_deferred {
                    self.lane.logged_deferred = true;
                    dj_log(format!("[dj] {why} — the pick is owed until it is back"));
                }
                return Vec::new();
            }
        };
        let ask = self.compose_ask(&identity);
        self.lane.pending = true;
        vec![Effect::Api(ApiCmd::AutoDj(Box::new(DjRequest {
            identity,
            reach,
            epoch: self.lane.epoch,
            ask,
        })))]
    }

    /// Everything a turn is composed from (clauses 20–27): the library's
    /// rules, the playing track's tags, the lane's anchors, the cursor.
    fn compose_ask(&mut self, identity: &str) -> dj::Ask {
        let playing = self.now_playing.clone();
        let playing_key = playing.as_ref().and_then(|t| t.metadata.musical_key.clone());
        // The anchor seeds from the playing track's key when there is none
        // yet; otherwise the first keyed pick locks it (clause 22).
        if self.dj.harmonic
            && self.lane.camelot_anchor.is_none()
            && let Some(code) = playing_key.as_deref().and_then(dj::to_camelot)
        {
            self.lane.camelot_anchor = Some(code.code());
        }
        let sonic_seeds = self.sonic_seeds(identity);
        dj::Ask {
            settings: self.dj.clone(),
            library: self.library_for(identity),
            playing_bpm: playing.as_ref().and_then(|t| t.metadata.bpm),
            playing_key,
            camelot_anchor: self.lane.camelot_anchor.clone(),
            sonic_seeds,
            ignore_list: self.lane.ignore.clone(),
            recent_artists: self.recent_artists(),
            opener: false,
            // What the probe learned of the server: a key it is known to
            // predate stays out of the body (clauses 25, 27, 50).
            server_version: self.dj_info.as_ref().and_then(|info| info.version.clone()),
            mood_scores: self.dj_mood_scores(),
        }
    }

    /// The pool's seeds by the anchor rule (clause 23), all on the DJ's
    /// server: rolling — the lane's history, else the playing track when
    /// it lives there; locked — one pin, set on the lane's first turn.
    fn sonic_seeds(&mut self, identity: &str) -> Vec<String> {
        if !self.dj_sonic_usable() {
            let not_ready = self
                .dj_info
                .as_ref()
                .is_some_and(|info| info.discovery && info.discovery_ready == Some(false));
            if self.dj.sonic && not_ready && !self.lane.logged_not_ready {
                self.lane.logged_not_ready = true;
                dj_log(format!(
                    "[dj] sonic off: {} has discovery on but nothing analysed yet — picks stay random until the scan gets there",
                    self.server_name_of(identity)
                ));
            }
            return Vec::new();
        }
        let origin = self.origin_of(identity);
        let playing_here = self
            .queue
            .current
            .and_then(|i| self.queue.items.get(i))
            .filter(|row| same_origin(&row.origin, &origin))
            .map(|row| row.filepath.clone());
        match self.dj.sonic_anchor {
            dj::SonicAnchor::Rolling => {
                if !self.lane.history.is_empty() {
                    self.lane.history.clone()
                } else {
                    playing_here.into_iter().collect()
                }
            }
            dj::SonicAnchor::Locked => {
                if self.lane.locked_pin.is_none() {
                    self.lane.locked_pin = playing_here;
                }
                self.lane.locked_pin.iter().cloned().collect()
            }
        }
    }

    /// What the DJ's server offers (clause 19): asked when the DJ arms, and
    /// again when its tunnel comes back. The session's own server answers
    /// from what the session already knows while the probe is out.
    pub(super) fn probe_dj(&mut self) -> Vec<Effect> {
        let Some(identity) = self.dj_server.clone() else { return Vec::new() };
        if self.dj_info.is_none() && self.session_identity().is_some_and(|s| crate::config::same_server(&s, &identity)) {
            self.dj_info = Some(DjServerInfo {
                version: None,
                discovery: self.capabilities.discovery,
                discovery_ready: None,
                libraries: self.libraries.clone(),
                mood_filters: self.capabilities.mood_filters,
            });
            self.dj_panel.rebuild(&self.dj, self.dj_info.as_ref(), self.dj_is_peer());
        }
        match self.reach_for(&identity) {
            Ok(reach) => vec![Effect::Api(ApiCmd::DjProbe { identity, reach })],
            Err(_) => Vec::new(),
        }
    }

    /// A tunnel came up: the DJ armed on it probes if it has not yet, and
    /// an owed turn is asked for (clause 35).
    pub(super) fn dj_tunnel_up(&mut self, id: &str) -> Vec<Effect> {
        let Some(origin) = self.dj_origin() else { return Vec::new() };
        let mine = crate::config::same_server(&origin.server, id)
            || origin.peer.is_some_and(|peer| crate::config::peer_identity(&origin.server, peer) == id);
        if !mine {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if self.dj_info.is_none() {
            effects.extend(self.probe_dj());
        }
        if self.lane.owed {
            self.lane.owed = false;
            self.lane.logged_deferred = false;
            effects.extend(self.request_turn());
        }
        effects
    }

    /// The session reconnected to what the DJ is armed on: its capabilities
    /// are the session's until the probe says more.
    pub(super) fn dj_session_connected(&mut self) -> Vec<Effect> {
        if self.dj_server.is_none() {
            // Off, what the room knew was the previous server's: the room
            // probes the new one when it opens (clause 50).
            self.dj_info = None;
            self.dj_panel.rebuild(&self.dj, None, self.dj_is_peer());
            return Vec::new();
        }
        if self.dj_info.is_none() {
            return self.probe_dj();
        }
        Vec::new()
    }

    /// The DJ's server as a tunnel target while armed (clause 19): a Quick
    /// Connect server's own tunnel, or a peer's — the record's
    /// tunnel-follows-the-DJ.
    pub(crate) fn dj_tunnel_target(&self, wanted: &mut std::collections::BTreeSet<String>) {
        let Some(origin) = self.dj_origin() else { return };
        match origin.peer {
            Some(id) => self.want_for_peer(wanted, &origin.server, id),
            None => {
                if crate::quickconnect::is_tunnel_id(&origin.server) {
                    wanted.insert(origin.server);
                }
            }
        }
    }

    // ── The answers ─────────────────────────────────────────────────────────

    /// The DJ's replies, through one door.
    ///
    /// Whatever else a reply says, the request it answers is no longer in
    /// flight — settled here, first, for all of them. A reply from another
    /// lane is dropped whole (clause 11): landing it would smuggle a dead
    /// session's tracks into the fresh queue and overwrite its cursor.
    pub(super) fn consume_dj(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::AutoDjPick { epoch, songs, ignore_list, sonic, note, failure } => {
                if epoch != self.lane.epoch {
                    return Vec::new();
                }
                self.lane.pending = false;
                let opener = std::mem::take(&mut self.lane.opener);
                if !opener && !self.dj_armed() {
                    return Vec::new(); // switched off while the request was out
                }
                self.lane.ignore = ignore_list;
                let mut explained = false;
                if let Some(note) = note {
                    // What the room's row says until the sonic settings
                    // change or the lane ends.
                    if matches!(note, DjNote::SonicRange | DjNote::SonicUnscanned) {
                        self.lane.sonic_drop = Some(note.clone());
                    }
                    if !self.lane.warned_sonic {
                        self.lane.warned_sonic = true;
                        self.info(dj_note_words(&note));
                        explained = true;
                    }
                }
                if let Some(failure) = failure {
                    return self.dj_turn_failed(failure, opener);
                }
                // A working pick clears the budgets (clause 34).
                self.lane.owed = false;
                self.lane.warned_auth = false;
                if opener {
                    let Some(track) = songs.into_iter().next() else { return Vec::new() };
                    let Some(server) = self.dj_target.take() else { return Vec::new() };
                    return self.start_from_seed(DjSeed { track, server, picked_by_dj: true });
                }
                self.queue_turn(songs, sonic, explained)
            }
            Event::AutoDjSample { tracks, pool, note } => {
                self.dj_panel.sample_pending = false;
                self.dj_panel.sample = tracks;
                // Keep the last pool size when this pick didn't report one:
                // it still describes the settings on screen.
                self.dj_panel.pool = pool.or(self.dj_panel.pool.take());
                if let Some(note) = note {
                    self.info(dj_note_words(&note));
                }
                Vec::new()
            }
            // The Sonic Path tab owns this one; it lands here only because
            // the discovery replies come through one door.
            Event::Journey { stops, note, length, issue } => {
                self.consume_journey(stops, note, length, issue)
            }
            Event::Genres(genres) => {
                if let Some(picker) = self.dj_panel.genres.as_mut() {
                    picker.loading = false;
                    picker.failed = None;
                    picker.all = genres.into_iter().map(|g| g.name).collect();
                    picker.row = picker.row.min(picker.all.len().saturating_sub(1));
                }
                Vec::new()
            }
            // The Genres sheet's list: the kind it asked for, and what the
            // server has of each. An answer under a kind no longer asked
            // for is dropped.
            Event::DjGenres { source, genres, sources } => {
                if let Some(picker) = self.dj_panel.genres.as_mut()
                    && picker.shown.is_none_or(|asked| asked == source)
                {
                    picker.loading = false;
                    picker.failed = None;
                    picker.all = genres.into_iter().map(|g| g.name).collect();
                    picker.sources = sources;
                    picker.shown = Some(source);
                    picker.row = picker.row.min(picker.all.len().saturating_sub(1));
                }
                Vec::new()
            }
            Event::DjRecent { identity, tracks } => {
                if let Some((whose, list)) = self.dj_recent.as_mut()
                    && crate::config::same_server(whose, &identity)
                {
                    *list = Some(tracks);
                }
                Vec::new()
            }
            // The list could not be fetched: the picker says so rather than
            // loading forever (clause 48's "Could not load genres").
            Event::GenresFailed(err) => {
                if let Some(picker) = self.dj_panel.genres.as_mut() {
                    picker.loading = false;
                    picker.failed = Some(err);
                }
                Vec::new()
            }
            Event::DjProbed { identity, info } => {
                // The DJ's server while armed; with it off, the session's —
                // the room's target either way (clause 50).
                if self.dj_library_target().is_some_and(|id| crate::config::same_server(&id, &identity))
                    && let Some(info) = info
                {
                    self.dj_info = Some(info);
                    self.dj_panel.rebuild(&self.dj, self.dj_info.as_ref(), self.dj_is_peer());
                }
                Vec::new()
            }
            // The routing arm in `apply_event` sends exactly the arms above.
            _ => Vec::new(),
        }
    }

    /// A turn that came back empty-handed (clauses 30–34): the auth note
    /// once per lane, a network failure silent and owed, an opener's failure
    /// in the record's two sentences.
    fn dj_turn_failed(&mut self, failure: DjFailure, opener: bool) -> Vec<Effect> {
        match failure {
            DjFailure::Auth => {
                if !self.lane.warned_auth {
                    self.lane.warned_auth = true;
                    self.error(t!("dj.auth_expired"));
                }
            }
            DjFailure::Network(why) => {
                if opener {
                    self.error(t!("dj.fetch_failed"));
                } else {
                    self.lane.owed = true;
                    if !self.lane.logged_deferred {
                        self.lane.logged_deferred = true;
                        dj_log(format!("[dj] random-songs failed: {why} — the pick is owed"));
                    }
                }
            }
            DjFailure::NoMatch => {
                if opener {
                    self.error(t!("dj.no_match"));
                } else {
                    self.info(t!("dj.no_match_lane"));
                }
            }
            DjFailure::Server(message) => {
                if opener {
                    self.error(t!("dj.fetch_failed"));
                }
                dj_log(format!("[dj] random-songs failed: {message}"));
            }
        }
        if opener {
            self.dj_target = None;
        }
        Vec::new()
    }

    /// The batch lands as the next rows in the server's order (clause 14),
    /// each wearing the badge (clause 60); the Camelot anchor locks on the
    /// first keyed one (clause 22); every one joins the history (clause
    /// 15). The first plays at once only when the queue had run dry.
    fn queue_turn(&mut self, songs: Vec<Track>, sonic: bool, explained: bool) -> Vec<Effect> {
        let Some(origin) = self.dj_origin() else { return Vec::new() };
        // Queued already — or queued once and let go at a restore, which
        // for this rule is the same (multi-server clause 40).
        let already: std::collections::HashSet<&str> = self
            .queue
            .items
            .iter()
            .map(|t| t.filepath.as_str())
            .chain(self.queue.retired.iter().map(String::as_str))
            .collect();
        let fresh: Vec<Track> = songs.into_iter().filter(|t| !already.contains(t.filepath.as_str())).collect();
        if fresh.is_empty() {
            self.info(t!("dj.nothing_new"));
            return Vec::new();
        }
        let last = self.queue.items.len().saturating_sub(1);
        let start_it = self.status.is_idle()
            && (self.queue.current.is_none() || self.queue.current == Some(last) || self.queue.items.is_empty());
        let first_new = self.queue.items.len();
        let mut names = Vec::new();
        for track in fresh {
            if self.dj.harmonic
                && self.lane.camelot_anchor.is_none()
                && let Some(code) = track.metadata.musical_key.as_deref().and_then(dj::to_camelot)
            {
                self.lane.camelot_anchor = Some(code.code());
            }
            self.lane.history.push(track.filepath.clone());
            if self.lane.history.len() > dj::HISTORY_LEN {
                let extra = self.lane.history.len() - dj::HISTORY_LEN;
                self.lane.history.drain(..extra);
            }
            names.push(track.display_name());
            self.queue.push(Queued { origin: origin.clone(), dj: Some(DjMark { sonic }), track });
        }
        // The degrade note, when one was just said, outranks the track name.
        if !explained {
            let said = match names.len() {
                1 => t!("dj.picked", song = names[0]),
                n => t!("dj.picked_more", song = names[0], count = n - 1),
            };
            self.info(said);
        }
        let mut effects = Vec::new();
        if start_it {
            effects.extend(self.play_index(first_new));
        }
        // A seeded start owes a second fetch while the queue is short.
        if std::mem::take(&mut self.lane.followers) && self.queue.items.len() < 3 {
            effects.extend(self.request_turn());
        }
        effects
    }

    // ── What the tab knows ──────────────────────────────────────────────────

    /// Recently-played artist names, newest first and deduped, for the
    /// cooldown. Tracks with no artist tag contribute nothing rather than an
    /// empty name the server would match against everything.
    pub(super) fn recent_artists(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.autodj_recent
            .iter()
            .filter_map(|t| t.metadata.artist.as_deref())
            .filter(|a| !a.trim().is_empty())
            .filter(|a| seen.insert(a.to_ascii_lowercase()))
            .map(str::to_string)
            .collect()
    }

    /// Note a track as played, for the cooldown.
    pub(super) fn remember_played(&mut self, track: &Track) {
        self.autodj_recent.retain(|t| t.filepath != track.filepath);
        self.autodj_recent.insert(0, track.clone());
        self.autodj_recent.truncate(RECENT_MEMORY);
    }

    // ── The Auto-DJ tab ─────────────────────────────────────────────────────

    /// Whether the Auto-DJ tab is the one in front of you — and so whether
    /// it gets the arrows and Enter. Its rows are values, so ←→ adjust them
    /// there and Tab / Shift+Tab do the tab switching instead.
    pub(super) fn on_dj_tab(&self) -> bool {
        self.fullscreen && self.now_tab() == NowTab::AutoDj
    }

    /// Enter on the highlighted row: the switches toggle, the pickers open,
    /// Preview asks, and the numbers nudge as the right arrow does.
    pub(super) fn activate_dj_row(&mut self) -> Vec<Effect> {
        match self.dj_panel.selected() {
            DjRow::Genres => self.open_genre_picker(),
            DjRow::Sources => self.open_sources_picker(),
            DjRow::Sample => self.sample_dj(),
            DjRow::Keywords => {
                self.dj_keyword = Some(String::new());
                Vec::new()
            }
            _ => self.adjust_dj_row(1),
        }
    }

    /// The tab's keyword entry (clause 49, the TUI's half): Enter on the row
    /// opens it, typed characters build a word, Enter adds the word and
    /// clears for the next, Enter on nothing or Esc leaves, Backspace past
    /// the start leaves too.
    pub(super) fn handle_dj_keyword_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        match action {
            Action::Input(c) => {
                if let Some(text) = self.dj_keyword.as_mut() {
                    text.push(c);
                }
            }
            Action::Backspace => {
                if self.dj_keyword.as_mut().is_none_or(|text| text.pop().is_none()) {
                    self.dj_keyword = None;
                }
            }
            Action::Cancel => self.dj_keyword = None,
            Action::Submit => {
                let word = self.dj_keyword.take().unwrap_or_default();
                if word.trim().is_empty() {
                    return Vec::new();
                }
                self.dj_keyword = Some(String::new());
                return self.dj_edit(DjEdit::AddKeyword(word));
            }
            _ => {}
        }
        Vec::new()
    }

    /// x on the Keywords row: the last word goes.
    pub(super) fn dj_remove_last_keyword(&mut self) -> Vec<Effect> {
        match self.dj.keywords.last().cloned() {
            Some(word) => self.dj_edit(DjEdit::RemoveKeyword(word)),
            None => Vec::new(),
        }
    }

    /// Move the highlighted setting by one step (the TUI's ←→ and Enter):
    /// the switches toggle either way, the radios cycle, the numbers move
    /// in useful increments so a bar crosses its range in a handful of
    /// presses.
    pub(super) fn adjust_dj_row(&mut self, delta: i32) -> Vec<Effect> {
        match self.dj_panel.selected() {
            DjRow::Armed => self.toggle_autodj(),
            // Not values: Enter opens or asks; ←→ have nothing to move.
            DjRow::Sources | DjRow::Sample => Vec::new(),
            // One row for a switch and a mode: the arrows walk the three states.
            DjRow::Genres => self.dj_edit(DjEdit::GenreCycle),
            row => self.dj_edit(DjEdit::Step(row, delta)),
        }
    }

    /// One edit from a room's control, in either shell (clause 51): applied
    /// to the running lane at once, the rows refit, and a per-library rule
    /// written where it belongs.
    pub(crate) fn dj_edit(&mut self, edit: DjEdit) -> Vec<Effect> {
        let step = |value: u32, by: i32, lo: u32, hi: u32| -> u32 {
            (value as i32 + by).clamp(lo as i32, hi as i32) as u32
        };
        let mut effects = Vec::new();
        match edit {
            DjEdit::Step(row, delta) => match row {
                DjRow::Armed | DjRow::Sources | DjRow::Sample => {}
                DjRow::SongsPerFetch => {
                    self.dj.songs_per_fetch = step(self.dj.songs_per_fetch, delta, 1, dj::SONGS_PER_FETCH_MAX);
                }
                DjRow::Sonic => {
                    self.dj.sonic = !self.dj.sonic;
                    // Switching off clears the lane's anchors (clause 43).
                    if !self.dj.sonic {
                        self.lane.history.clear();
                        self.lane.locked_pin = None;
                    }
                    self.dj_retry_sonic();
                }
                DjRow::Strictness => {
                    let raw = self.dj.sonic_min_similarity + f64::from(delta) * dj::SONIC_STEP;
                    self.dj.sonic_min_similarity = dj::clamp_similarity(raw);
                    self.dj_retry_sonic();
                }
                DjRow::Anchor => {
                    self.dj.sonic_anchor = self.dj.sonic_anchor.next();
                    self.lane.locked_pin = None;
                    self.dj_retry_sonic();
                }
                DjRow::EmptyQueue => self.dj.empty_queue = self.dj.empty_queue.next(),
                DjRow::Bpm => self.dj.bpm = !self.dj.bpm,
                DjRow::Tolerance => {
                    self.dj.bpm_tolerance =
                        step(self.dj.bpm_tolerance, delta, dj::BPM_TOLERANCE_MIN, dj::BPM_TOLERANCE_MAX);
                }
                DjRow::Harmonic => {
                    self.dj.harmonic = !self.dj.harmonic;
                    if !self.dj.harmonic {
                        self.lane.camelot_anchor = None;
                    }
                }
                DjRow::Cooldown => {
                    self.dj.artist_cooldown = step(self.dj.artist_cooldown, delta, 0, dj::ARTIST_COOLDOWN_MAX);
                }
                DjRow::Rating => {
                    let now = self.dj_library().min_rating;
                    effects = self.set_dj_rating(step(now, delta, 0, dj::RATING_MAX));
                }
                DjRow::Length => self.dj.length = !self.dj.length,
                DjRow::Shortest => {
                    let by = delta * dj::LENGTH_STEP_SECONDS as i32;
                    self.dj.min_seconds = step(self.dj.min_seconds, by, 0, self.dj.max_seconds);
                }
                DjRow::Longest => {
                    let by = delta * dj::LENGTH_STEP_SECONDS as i32;
                    self.dj.max_seconds = step_longest(self.dj.max_seconds, by, self.dj.min_seconds);
                }
                DjRow::UnknownLength => self.dj.allow_unknown_length = !self.dj.allow_unknown_length,
                DjRow::Genres => {
                    let on = !self.dj_library().genre_filter;
                    effects = self.set_dj_genre_filter(on);
                }
                DjRow::Keywords => self.dj.keyword_filter = !self.dj.keyword_filter,
            },
            DjEdit::Set(row, value) => match row {
                DjRow::SongsPerFetch => self.dj.songs_per_fetch = value.clamp(1, dj::SONGS_PER_FETCH_MAX),
                DjRow::Strictness => {
                    self.dj.sonic_min_similarity = dj::clamp_similarity(f64::from(value) / 100.0);
                    self.dj_retry_sonic();
                }
                DjRow::Tolerance => {
                    self.dj.bpm_tolerance = value.clamp(dj::BPM_TOLERANCE_MIN, dj::BPM_TOLERANCE_MAX);
                }
                DjRow::Cooldown => self.dj.artist_cooldown = value.min(dj::ARTIST_COOLDOWN_MAX),
                DjRow::Rating => effects = self.set_dj_rating(value.min(dj::RATING_MAX)),
                DjRow::Shortest => self.dj.min_seconds = value.min(self.dj.max_seconds),
                DjRow::Longest => {
                    self.dj.max_seconds = value.clamp(self.dj.min_seconds, dj::LENGTH_RAIL_SECONDS);
                }
                _ => {}
            },
            DjEdit::EmptyQueue(choice) => self.dj.empty_queue = choice,
            DjEdit::GenreCycle => {
                let library = self.dj_library();
                effects = match (library.genre_filter, library.genre_mode) {
                    (false, _) => self.set_dj_genre_rule(true, dj::GenreMode::Whitelist),
                    (true, dj::GenreMode::Whitelist) => self.set_dj_genre_rule(true, dj::GenreMode::Blacklist),
                    (true, dj::GenreMode::Blacklist) => self.set_dj_genre_rule(false, dj::GenreMode::Blacklist),
                };
            }
            DjEdit::Genre(name) => effects = self.toggle_dj_genre(name),
            DjEdit::AddKeyword(word) => {
                let word = word.trim().to_string();
                if word.is_empty() {
                    return Vec::new();
                }
                if self.dj.keywords.iter().any(|k| k.eq_ignore_ascii_case(&word)) {
                    return Vec::new();
                }
                if self.dj.keywords.len() >= dj::KEYWORDS_MAX {
                    self.info(t!("dj.max_keywords", count = dj::KEYWORDS_MAX));
                    return Vec::new();
                }
                self.dj.keywords.push(word);
                // A word added with the filter off is a dead end; switch it
                // on rather than silently ignoring the word.
                self.dj.keyword_filter = true;
            }
            DjEdit::RemoveKeyword(word) => self.dj.keywords.retain(|k| !k.eq_ignore_ascii_case(&word)),
            DjEdit::Sonic { on, floor, anchor } => {
                let floor = dj::clamp_similarity(f64::from(floor) / 100.0);
                let changed = (self.dj.sonic, self.dj.sonic_min_similarity, self.dj.sonic_anchor) != (on, floor, anchor);
                if self.dj.sonic_anchor != anchor {
                    self.lane.locked_pin = None;
                }
                // Switching off clears the lane's anchors (clause 43).
                if self.dj.sonic && !on {
                    self.lane.history.clear();
                    self.lane.locked_pin = None;
                }
                self.dj.sonic = on;
                self.dj.sonic_min_similarity = floor;
                self.dj.sonic_anchor = anchor;
                if changed {
                    self.dj_retry_sonic();
                }
            }
            DjEdit::Moods(filter) => self.dj.moods = filter,
            DjEdit::Tempo { on, tolerance } => {
                self.dj.bpm = on;
                self.dj.bpm_tolerance = tolerance.clamp(dj::BPM_TOLERANCE_MIN, dj::BPM_TOLERANCE_MAX);
            }
            DjEdit::LengthWindow { min, max, unknown } => {
                let rail = dj::LENGTH_RAIL_SECONDS;
                let mut lo = min.unwrap_or(0).min(rail);
                // No maximum is the rail; a typed zero is none too.
                let mut hi = max.filter(|m| *m > 0).unwrap_or(rail).min(rail);
                // A window with no possible match: the bounds change places.
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                self.dj.min_seconds = lo;
                self.dj.max_seconds = hi;
                self.dj.length = lo > 0 || hi < rail;
                self.dj.allow_unknown_length = unknown;
            }
            DjEdit::Keywords(words) => {
                let mut seen = std::collections::HashSet::new();
                self.dj.keywords = words
                    .iter()
                    .map(|w| w.trim().to_string())
                    .filter(|w| !w.is_empty() && seen.insert(w.to_lowercase()))
                    .take(dj::KEYWORDS_MAX)
                    .collect();
                self.dj.keyword_filter = !self.dj.keywords.is_empty();
            }
            DjEdit::GenreRule { mode, source, genres } => effects = self.set_dj_genres(mode, source, genres),
        }
        self.dj_panel.rebuild(&self.dj, self.dj_info.as_ref(), self.dj_is_peer());
        effects
    }

    // ── The library's rules (clause 51) ────────────────────────────────────

    /// The library a room edits: the DJ's server while armed, else the
    /// session's — the one Start would use.
    pub(crate) fn dj_library_target(&self) -> Option<String> {
        self.dj_server.clone().or_else(|| self.dj_play_from())
    }

    /// That library's rules as the room shows them and the request sends
    /// them: the server entry's own, with the session-wide fallbacks behind.
    pub fn dj_library(&self) -> dj::LibraryFilters {
        match self.dj_library_target() {
            Some(identity) => self.library_for(&identity),
            None => dj::LibraryFilters::resolve(&self.dj, &Default::default(), false),
        }
    }

    /// The saved entry a per-library rule is written to, when there is one.
    fn dj_library_entry(&self) -> Option<usize> {
        let target = self.dj_library_target()?;
        self.servers.iter().position(|s| crate::config::same_server(&s.id, &target))
    }

    fn save_dj_library(&self, index: usize) -> Vec<Effect> {
        let known = &self.servers[index];
        vec![Effect::SaveDjLibrary { server: known.id.clone(), overrides: known.dj.clone() }]
    }

    fn set_dj_rating(&mut self, rating: u32) -> Vec<Effect> {
        match self.dj_library_entry() {
            Some(i) => {
                self.servers[i].dj.min_rating = Some(rating);
                self.save_dj_library(i)
            }
            None => {
                self.dj.min_rating = rating;
                Vec::new()
            }
        }
    }

    /// The genre rule's two fields (clause 48), written where the library's
    /// rules live. Set apart on purpose: the switch never touches the mode,
    /// so whitelist or blacklist survives a switch off, and the mode never
    /// touches the switch.
    fn set_dj_genre_rule(&mut self, filter: bool, mode: dj::GenreMode) -> Vec<Effect> {
        match self.dj_library_entry() {
            Some(i) => {
                self.servers[i].dj.genre_filter = Some(filter);
                self.servers[i].dj.genre_mode = Some(mode.label().to_string());
                self.save_dj_library(i)
            }
            None => {
                self.dj.genre_filter = filter;
                self.dj.genre_mode = mode;
                Vec::new()
            }
        }
    }

    fn set_dj_genre_filter(&mut self, on: bool) -> Vec<Effect> {
        let mode = self.dj_library().genre_mode;
        self.set_dj_genre_rule(on, mode)
    }

    /// A genre in or out of the chosen set (clause 48), at most
    /// [`dj::GENRES_MAX`]; choosing with the filter off switches it on
    /// rather than silently ignoring the choice.
    fn toggle_dj_genre(&mut self, name: String) -> Vec<Effect> {
        let library = self.dj_library();
        let mut genres = library.genres;
        if let Some(at) = genres.iter().position(|g| *g == name) {
            genres.remove(at);
        } else {
            if genres.len() >= dj::GENRES_MAX {
                self.info(t!("dj.max_genres", count = dj::GENRES_MAX));
                return Vec::new();
            }
            genres.push(name);
        }
        // Choosing with the filter off switches it on; the mode is its own.
        let filter = library.genre_filter || !genres.is_empty();
        match self.dj_library_entry() {
            Some(i) => {
                self.servers[i].dj.genres = Some(genres);
                self.servers[i].dj.genre_filter = Some(filter);
                self.servers[i].dj.genre_mode = Some(library.genre_mode.label().to_string());
                self.save_dj_library(i)
            }
            None => {
                self.dj.genres = genres;
                self.dj.genre_filter = filter;
                Vec::new()
            }
        }
    }

    /// The Genres sheet's Done: the rule whole — the way it cuts, the kind
    /// of genre it matches, and the list, at most [`dj::GENRES_MAX`]. An
    /// empty list is the filter off; the mode and the kind are kept.
    fn set_dj_genres(&mut self, mode: dj::GenreMode, source: GenreSource, genres: Vec<String>) -> Vec<Effect> {
        let mut seen = std::collections::HashSet::new();
        let genres: Vec<String> = genres
            .into_iter()
            .filter(|g| !g.is_empty() && seen.insert(g.clone()))
            .take(dj::GENRES_MAX)
            .collect();
        let filter = !genres.is_empty();
        match self.dj_library_entry() {
            Some(i) => {
                let own = &mut self.servers[i].dj;
                own.genres = Some(genres);
                own.genre_filter = Some(filter);
                own.genre_mode = Some(mode.label().to_string());
                own.genre_source = Some(source.as_str().to_string());
                self.save_dj_library(i)
            }
            None => {
                self.dj.genres = genres;
                self.dj.genre_filter = filter;
                self.dj.genre_mode = mode;
                self.dj.genre_source = source;
                Vec::new()
            }
        }
    }

    /// The Genres sheet asks for its list: the genres of one kind in the
    /// library the rule is for. What the server said it has of each kind
    /// stays in hand while the answer is out.
    pub(crate) fn dj_genres_ask(&mut self, source: GenreSource) -> Vec<Effect> {
        let sources = self.dj_panel.genres.as_ref().and_then(|picker| picker.sources);
        self.dj_panel.genres =
            Some(GenrePicker { loading: true, sources, shown: Some(source), ..Default::default() });
        let reach = self.dj_library_target().and_then(|target| self.reach_for(&target).ok()).flatten();
        vec![Effect::Api(ApiCmd::DjGenres { reach, source })]
    }

    /// The room's server picker (clause 41): re-selecting the DJ's server is
    /// a no-op (a restart would drop the lane for nothing); another moves the
    /// DJ there and starts a new lane.
    pub(crate) fn dj_move_to(&mut self, identity: String) -> Vec<Effect> {
        if self.dj_server.as_deref().is_some_and(|s| crate::config::same_server(s, &identity)) {
            return Vec::new();
        }
        self.arm_dj(identity)
    }

    /// Either picker closed, whichever shell drew it.
    pub(crate) fn close_dj_picker(&mut self) {
        self.dj_panel.genres = None;
        self.dj_panel.sources = None;
    }

    /// The room opened with the DJ off: what the session's server offers
    /// decides which rows show (clause 50), so it is probed like the DJ's
    /// own would be.
    pub(crate) fn dj_room_opened(&mut self) -> Vec<Effect> {
        if self.dj_server.is_some() || self.dj_info.is_some() {
            return Vec::new();
        }
        // Play from's server: the one Start would use, the session's or not.
        let Some(identity) = self.dj_play_from() else { return Vec::new() };
        match self.reach_for(&identity) {
            Ok(reach) => vec![Effect::Api(ApiCmd::DjProbe { identity, reach })],
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn open_genre_picker(&mut self) -> Vec<Effect> {
        self.dj_panel.genres = Some(GenrePicker { loading: true, ..Default::default() });
        // The genres of the library the rule is FOR — the DJ's server while
        // armed — not whichever server is browsed (clauses 48 and 51).
        let reach = self.dj_library_target().and_then(|target| self.reach_for(&target).ok()).flatten();
        vec![Effect::Api(ApiCmd::Genres { reach })]
    }

    /// The sources picker (clause 42): the DJ server's libraries, each a
    /// switch; the last one on cannot be switched off.
    fn open_sources_picker(&mut self) -> Vec<Effect> {
        let all = self.dj_info.as_ref().map(|i| i.libraries.clone()).unwrap_or_default();
        self.dj_panel.sources = Some(GenrePicker { all, ..Default::default() });
        Vec::new()
    }

    /// The libraries switched off for the DJ's server.
    pub fn dj_sources_off(&self) -> Vec<String> {
        // The library the rules are for: the DJ's server while armed, else
        // the session's — the other per-library rules' target.
        let Some(id) = self.dj_library_target() else { return Vec::new() };
        let id = id.as_str();
        self.servers
            .iter()
            .find(|s| crate::config::same_server(&s.id, id))
            .map(|s| s.dj.sources_off.clone())
            .unwrap_or_default()
    }

    /// Switch one of the DJ server's libraries on or off (clause 42); the
    /// change is the server entry's, so the shells persist it.
    pub(crate) fn toggle_dj_source(&mut self, library: &str) -> Vec<Effect> {
        let Some(id) = self.dj_library_target() else { return Vec::new() };
        let all = self.dj_info.as_ref().map(|i| i.libraries.len()).unwrap_or(0);
        let Some(known) = self.servers.iter_mut().find(|s| crate::config::same_server(&s.id, &id)) else {
            return Vec::new();
        };
        if let Some(at) = known.dj.sources_off.iter().position(|s| s == library) {
            known.dj.sources_off.remove(at);
        } else {
            if all > 0 && known.dj.sources_off.len() + 1 >= all {
                self.info(t!("dj.one_source"));
                return Vec::new();
            }
            known.dj.sources_off.push(library.to_string());
        }
        let overrides = known.dj.clone();
        vec![Effect::SaveDjLibrary { server: id, overrides }]
    }

    /// The keys inside either picker: Space toggles, Enter and Esc close —
    /// a chooser you leave with the same key you pick with is a chooser you
    /// keep leaving by accident.
    pub(super) fn handle_genre_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        let sources = self.dj_panel.sources.is_some();
        let Some(picker) = self.dj_panel.sources.as_mut().or(self.dj_panel.genres.as_mut()) else {
            return Vec::new();
        };
        match action {
            Action::Cancel | Action::Activate | Action::Submit => {
                self.dj_panel.genres = None;
                self.dj_panel.sources = None;
            }
            Action::Up => picker.row = picker.row.saturating_sub(1),
            Action::Down => {
                picker.row = (picker.row + 1).min(picker.all.len().saturating_sub(1));
            }
            Action::First => picker.row = 0,
            Action::Last => picker.row = picker.all.len().saturating_sub(1),
            Action::PlayPause => {
                let Some(name) = picker.all.get(picker.row).cloned() else { return Vec::new() };
                if sources {
                    return self.toggle_dj_source(&name);
                }
                return self.dj_edit(DjEdit::Genre(name));
            }
            _ => {}
        }
        Vec::new()
    }

    /// Preview (clause 53): three picks with the current settings, none of
    /// them queued.
    pub(crate) fn sample_dj(&mut self) -> Vec<Effect> {
        if self.dj_panel.sample_pending {
            return Vec::new();
        }
        let Some(identity) = self.dj_library_target() else {
            self.info(t!("dj.needs_server"));
            return Vec::new();
        };
        let Ok(reach) = self.reach_for(&identity) else {
            self.info(t!("dj.not_reachable"));
            return Vec::new();
        };
        self.dj_panel.sample_pending = true;
        self.dj_panel.sample.clear();
        let ask = self.compose_ask(&identity);
        vec![Effect::Api(ApiCmd::AutoDjSample {
            request: Box::new(DjRequest { identity, reach, epoch: self.lane.epoch, ask }),
            count: DJ_SAMPLE_COUNT,
        })]
    }
}

/// A pick's or a preview's note in the user's words (clauses 30, 53).
fn dj_note_words(note: &DjNote) -> String {
    match note {
        DjNote::SonicRange => t!("dj.sonic_range").to_string(),
        DjNote::SonicUnscanned => t!("dj.sonic_unscanned").to_string(),
        DjNote::PreviewFailed(failure) => {
            t!("dj.preview_failed", why = dj_failure_words(failure)).to_string()
        }
    }
}

/// Why a turn came back empty-handed, in the user's words.
fn dj_failure_words(failure: &DjFailure) -> String {
    match failure {
        DjFailure::Auth => t!("dj.fail_auth").to_string(),
        DjFailure::Network(_) => t!("dj.fail_network").to_string(),
        DjFailure::NoMatch => t!("dj.fail_no_match").to_string(),
        DjFailure::Server(message) => message.clone(),
    }
}

/// Whether two origins name the same server and place.
fn same_origin(a: &Origin, b: &Origin) -> bool {
    crate::config::same_server(&a.server, &b.server) && a.peer == b.peer
}
