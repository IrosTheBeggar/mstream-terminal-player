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
    Anchor(dj::SonicAnchor),
    EmptyQueue(dj::EmptyQueueStart),
    /// Whitelist or blacklist; Off is the switch (a step on the Genres row
    /// cycles all three, the TUI's habit).
    GenreMode(dj::GenreMode),
    /// A genre in or out of the chosen set.
    Genre(String),
    AddKeyword(String),
    RemoveKeyword(String),
}

/// The empty-queue chooser (clause 2): "Start Auto DJ with what?" — Surprise
/// me, Let me choose, and the Remember-this box.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DjChooser {
    pub row: usize,
    pub remember: bool,
}

/// The chooser's rows: the two answers, then the remember box.
pub const DJ_CHOOSER_ROWS: usize = 3;

/// The banner the "Let me choose" road wears (clause 4).
pub const DJ_PICK_BANNER: &str = "Pick the opening song — choose a track anywhere in the library";

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
    pub fn dj_sonic_reason(&self) -> Option<&'static str> {
        let info = self.dj_info.as_ref()?;
        if dj::known_older(info.version.as_deref(), dj::FLOOR_SONIC) {
            return Some("Needs server 6.15.2 or newer");
        }
        if !info.discovery {
            return Some("This server doesn't have discovery data — picks stay random.");
        }
        if info.discovery_ready == Some(false) {
            return Some(
                "Discovery is on but the scan hasn't produced data yet — picks stay random until it does.",
            );
        }
        None
    }

    // ── The toggle (entry point 1) ──────────────────────────────────────────

    /// The record's toggle: off → arm for the session's server; on here →
    /// off; on elsewhere → move here.
    pub(super) fn toggle_autodj(&mut self) -> Vec<Effect> {
        let Some(here) = self.session_identity() else {
            self.info("Auto DJ needs a server — connect to one first.");
            return Vec::new();
        };
        match self.dj_server.clone() {
            Some(current) if crate::config::same_server(&current, &here) => self.disarm_dj(),
            _ if self.queue.items.is_empty() => self.begin_empty_start(here),
            _ => self.arm_dj(here),
        }
    }

    /// Arm the DJ for `identity` (clause 1): a fresh lane when it is a
    /// different server, the probe of what that server offers, and the
    /// queue's end looked at.
    pub(crate) fn arm_dj(&mut self, identity: String) -> Vec<Effect> {
        let fresh = !self.dj_server.as_deref().is_some_and(|s| crate::config::same_server(s, &identity));
        if fresh {
            self.lane.reset();
            self.dj_info = None;
        }
        self.dj_server = Some(identity);
        self.dj_chooser = None;
        self.say_armed();
        let mut effects = self.probe_dj();
        effects.extend(self.dj_look_at_the_end());
        effects
    }

    fn say_armed(&mut self) {
        let name = self.dj_server_name();
        if self.servers.len() > 1 {
            self.info(format!("Auto DJ on — picking from {name}"));
        } else {
            self.info("Auto DJ on");
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
        self.info("Auto DJ off");
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
        self.dj_target = Some(identity);
        match self.dj.empty_queue {
            dj::EmptyQueueStart::Ask => {
                self.dj_chooser = Some(DjChooser::default());
                Vec::new()
            }
            dj::EmptyQueueStart::Random => self.dj_surprise(),
            dj::EmptyQueueStart::Pick => self.dj_pick_from_library(),
        }
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
                self.error(format!("Couldn't fetch a song from the server — {why}"));
                self.dj_target = None;
                return Vec::new();
            }
        };
        let mut ask = self.compose_ask(&identity);
        ask.opener = true;
        ask.ignore_list.clear();
        self.lane.opener = true;
        self.lane.pending = true;
        self.info("Auto DJ: picking an opening song…");
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
        self.info(DJ_PICK_BANNER);
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
            self.dj_info = None;
        }
        self.dj_server = Some(seed.server.clone());
        // The clear that starts it consumes the seed: a new lane, and nothing
        // left for a later empty-queue start to replay.
        self.lane.reset();
        let origin = self.origin_of(&seed.server);
        let mark = seed.picked_by_dj.then_some(DjMark { sonic: false });
        self.queue.replace(vec![Queued { origin, dj: mark, track: seed.track }]);
        self.say_armed();
        let mut effects = self.probe_dj();
        effects.extend(self.play_index(0));
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
            dj::EmptyQueueStart::Ask => Vec::new(),
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
    fn request_turn(&mut self) -> Vec<Effect> {
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
        if self.dj_server.is_some() && self.dj_info.is_none() {
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
                if let Some(note) = note
                    && !self.lane.warned_sonic
                {
                    self.lane.warned_sonic = true;
                    self.info(note);
                    explained = true;
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
                    self.info(note);
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
                    picker.all = genres.into_iter().map(|g| g.name).collect();
                    picker.row = picker.row.min(picker.all.len().saturating_sub(1));
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
                    self.error(crate::tui::worker::DJ_NOTE_AUTH);
                }
            }
            DjFailure::Network(why) => {
                if opener {
                    self.error("Couldn't fetch a song from the server.");
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
                    self.error("No songs match your Auto DJ filters — try loosening them");
                } else {
                    self.info("Auto DJ: nothing matches the filters — loosen them to keep going");
                }
            }
            DjFailure::Server(message) => {
                if opener {
                    self.error("Couldn't fetch a song from the server.");
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
        let already: std::collections::HashSet<String> =
            self.queue.items.iter().map(|t| t.filepath.clone()).collect();
        let fresh: Vec<Track> = songs.into_iter().filter(|t| !already.contains(&t.filepath)).collect();
        if fresh.is_empty() {
            self.info("Auto DJ: nothing new to add");
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
            self.info(match names.len() {
                1 => format!("Auto DJ: {}", names[0]),
                n => format!("Auto DJ: {} (and {} more)", names[0], n - 1),
            });
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
                self.info("keywords are edited in the GUI's Auto DJ room for now");
                Vec::new()
            }
            _ => self.adjust_dj_row(1),
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
                }
                DjRow::Strictness => {
                    let raw = self.dj.sonic_min_similarity + f64::from(delta) * dj::SONIC_STEP;
                    self.dj.sonic_min_similarity = dj::clamp_similarity(raw);
                }
                DjRow::Anchor => {
                    self.dj.sonic_anchor = self.dj.sonic_anchor.next();
                    self.lane.locked_pin = None;
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
                    self.dj.max_seconds =
                        step(self.dj.max_seconds, by, self.dj.min_seconds, dj::LENGTH_RAIL_SECONDS);
                }
                DjRow::UnknownLength => self.dj.allow_unknown_length = !self.dj.allow_unknown_length,
                DjRow::Genres => {
                    let mode = self.dj_library().genre_mode.next();
                    effects = self.set_dj_genre_mode(mode);
                }
                DjRow::Keywords => self.dj.keyword_filter = !self.dj.keyword_filter,
            },
            DjEdit::Set(row, value) => match row {
                DjRow::SongsPerFetch => self.dj.songs_per_fetch = value.clamp(1, dj::SONGS_PER_FETCH_MAX),
                DjRow::Strictness => {
                    self.dj.sonic_min_similarity = dj::clamp_similarity(f64::from(value) / 100.0);
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
            DjEdit::Anchor(anchor) => {
                if self.dj.sonic_anchor != anchor {
                    self.dj.sonic_anchor = anchor;
                    self.lane.locked_pin = None;
                }
            }
            DjEdit::EmptyQueue(choice) => self.dj.empty_queue = choice,
            DjEdit::GenreMode(mode) => effects = self.set_dj_genre_mode(mode),
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
                    self.info(format!("At most {} keywords.", dj::KEYWORDS_MAX));
                    return Vec::new();
                }
                self.dj.keywords.push(word);
                // A word added with the filter off is a dead end; switch it
                // on rather than silently ignoring the word.
                self.dj.keyword_filter = true;
            }
            DjEdit::RemoveKeyword(word) => self.dj.keywords.retain(|k| !k.eq_ignore_ascii_case(&word)),
        }
        self.dj_panel.rebuild(&self.dj, self.dj_info.as_ref(), self.dj_is_peer());
        effects
    }

    // ── The library's rules (clause 51) ────────────────────────────────────

    /// The library a room edits: the DJ's server while armed, else the
    /// session's — the one Start would use.
    pub(crate) fn dj_library_target(&self) -> Option<String> {
        self.dj_server.clone().or_else(|| self.session_identity())
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

    fn set_dj_genre_mode(&mut self, mode: dj::GenreMode) -> Vec<Effect> {
        match self.dj_library_entry() {
            Some(i) => {
                self.servers[i].dj.genre_mode = Some(mode.label().to_string());
                self.save_dj_library(i)
            }
            None => {
                self.dj.genre_mode = mode;
                Vec::new()
            }
        }
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
                self.info(format!("At most {} genres.", dj::GENRES_MAX));
                return Vec::new();
            }
            genres.push(name);
        }
        let mode = if library.genre_mode == dj::GenreMode::Off && !genres.is_empty() {
            dj::GenreMode::Whitelist
        } else {
            library.genre_mode
        };
        match self.dj_library_entry() {
            Some(i) => {
                self.servers[i].dj.genres = Some(genres);
                self.servers[i].dj.genre_mode = Some(mode.label().to_string());
                self.save_dj_library(i)
            }
            None => {
                self.dj.genres = genres;
                self.dj.genre_mode = mode;
                Vec::new()
            }
        }
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
        let Some(identity) = self.session_identity() else { return Vec::new() };
        match self.reach_for(&identity) {
            Ok(reach) => vec![Effect::Api(ApiCmd::DjProbe { identity, reach })],
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn open_genre_picker(&mut self) -> Vec<Effect> {
        self.dj_panel.genres = Some(GenrePicker { loading: true, ..Default::default() });
        vec![Effect::Api(ApiCmd::Genres)]
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
        let Some(id) = self.dj_server.as_deref() else { return Vec::new() };
        self.servers
            .iter()
            .find(|s| crate::config::same_server(&s.id, id))
            .map(|s| s.dj.sources_off.clone())
            .unwrap_or_default()
    }

    /// Switch one of the DJ server's libraries on or off (clause 42); the
    /// change is the server entry's, so the shells persist it.
    pub(crate) fn toggle_dj_source(&mut self, library: &str) -> Vec<Effect> {
        let Some(id) = self.dj_server.clone() else { return Vec::new() };
        let all = self.dj_info.as_ref().map(|i| i.libraries.len()).unwrap_or(0);
        let Some(known) = self.servers.iter_mut().find(|s| crate::config::same_server(&s.id, &id)) else {
            return Vec::new();
        };
        if let Some(at) = known.dj.sources_off.iter().position(|s| s == library) {
            known.dj.sources_off.remove(at);
        } else {
            if all > 0 && known.dj.sources_off.len() + 1 >= all {
                self.info("At least one source is required.");
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
        let Some(identity) = self.dj_server.clone().or_else(|| self.session_identity()) else {
            self.info("Auto DJ needs a server — connect to one first.");
            return Vec::new();
        };
        let Ok(reach) = self.reach_for(&identity) else {
            self.info("that server is not reachable yet");
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

/// The DJ's second voice (clause 63): the shell's log on the native build,
/// where a filter that quietly does nothing would otherwise be
/// indistinguishable from one that works. The browser build has no log.
fn dj_log(line: String) {
    #[cfg(not(target_arch = "wasm32"))]
    tracing::info!("{line}");
    #[cfg(target_arch = "wasm32")]
    let _ = line;
}

/// Whether two origins name the same server and place.
fn same_origin(a: &Origin, b: &Origin) -> bool {
    crate::config::same_server(&a.server, &b.server) && a.peer == b.peer
}
