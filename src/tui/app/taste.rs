//! The GUI's Discover room: the history-seeded page (docs/ux-contracts/
//! discovery-page.md; mStream's docs/discovery-page-plan.md §3.1–§3.3).
//!
//! The room keeps **its own row model** here, the sonic room's precedent:
//! nothing in this file reads or writes the TUI's Discover tab (`App.discover`,
//! its drill and seed), the Now Playing panel's neighbours (`App.now_discover`)
//! or `Capture::Discover` — clause 67. The GUI draws from [`TasteState`] and
//! calls the `taste_*` methods; every queue row they build is stamped here,
//! the session's origin on a Library song and the peer's on a Peers row.
//!
//! The answer comes in parts asked in parallel (clause 30): **Main** (library
//! and network), **Peers**, and a filter change's **Library** alone. Each list
//! is a [`TasteSlot`] holding the seq of the ask it waits for, so the last ask
//! wins per list and a slow part never overwrites a newer one (clause 31).

use super::*;
use crate::api::types::{
    PluginLink, SeedSource, TasteFilter, TasteLead, TasteLibrary, TasteNetwork, TastePeers,
    TastePrefs, TastePrefsPatch, TasteRequest, TasteResponse,
};
use crate::tui::worker::{TasteFailure, TastePart};

/// An answer is reused on re-entry for this long after it arrived — the
/// server's own cache window (clause 3; plan §2.4). A failed one never is.
pub const TASTE_REUSE: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// The server takes at most this many pins (plan §3.1).
pub const TASTE_MAX_PINS: usize = 8;

/// The server keeps at most this many exclusions per user, trimming the
/// oldest, and refuses a longer list outright (plan §3.3; mStream's
/// taste-queries.js `EXCLUDE_CAP`).
pub const TASTE_EXCLUDE_CAP: usize = 200;

/// How many times "another pick" draws again on its own when the song it
/// drew turns out unanalyzed (clause 21). `random-songs` has no "analyzed
/// only" ask, and an unanalyzed pin's answer says `notAnalyzed`, which the
/// room would read as the whole server's scan not run (clause 81).
pub const TASTE_REDRAWS: u8 = 3;

/// The stale-days the `fresh` filter means when the prefs never said (plan
/// §2.5).
pub const TASTE_STALE_DAYS: u32 = 90;

/// The room's tabs (clause 40), and what `[gui] discover_tab` stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TasteTab {
    #[default]
    Library,
    Network,
    Peers,
}

impl TasteTab {
    /// The `GuiPrefs` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TasteTab::Library => "library",
            TasteTab::Network => "network",
            TasteTab::Peers => "peers",
        }
    }

    /// A remembered tab; anything else is no memory at all.
    pub fn parse(word: &str) -> Option<TasteTab> {
        match word.trim() {
            "library" => Some(TasteTab::Library),
            "network" => Some(TasteTab::Network),
            "peers" => Some(TasteTab::Peers),
            _ => None,
        }
    }
}

/// One source's rows and where its ask stands.
#[derive(Debug, Clone, PartialEq)]
pub struct TasteSlot<T> {
    /// The last rows that landed. Kept through a re-ask, so the room never
    /// goes blank while it asks again (clause 32), and through a failure
    /// (clause 83).
    pub data: Option<T>,
    /// The ask whose reply may fill this slot; 0 before the first.
    pub(crate) seq: u64,
    /// That ask is out.
    pub pending: bool,
    /// The source was asked in the latest round. A tab not asked shows `–`
    /// for a count (clause 40).
    pub asked: bool,
    /// The server answered `null` for a seed that was not its random pick:
    /// this caller lacks the source, and its tab hides until the next
    /// answer (clause 40).
    pub absent: bool,
    /// The server answered `null` because the seed is its random pick
    /// (clause 22): "asked once you've finished a song".
    pub not_asked_random: bool,
    /// Why the last ask failed — the panes' "couldn't ask for suggestions:
    /// {err}" with Retry (clause 83). The old rows stand beside it.
    pub failed: Option<String>,
}

impl<T> Default for TasteSlot<T> {
    fn default() -> Self {
        TasteSlot {
            data: None,
            seq: 0,
            pending: false,
            asked: false,
            absent: false,
            not_asked_random: false,
            failed: None,
        }
    }
}

impl<T> TasteSlot<T> {
    fn ask(&mut self, seq: u64) {
        self.seq = seq;
        self.pending = true;
        self.asked = true;
    }

    /// The first ask of a list with nothing yet to show: the scan bar, not
    /// a stale list (clause 32).
    pub fn first_wait(&self) -> bool {
        self.pending && self.data.is_none()
    }
}

/// What a 403 has been found to mean (clause 82).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TasteLatch {
    #[default]
    Clear,
    /// A 403 landed; the ping is being asked why.
    Probing,
    /// The gate still stands: the index is not there yet — clause 81's
    /// sentence and Check again.
    ScanPending,
    /// The gate is gone: switched off, no retry; the nav row goes with the
    /// next ping (clause 1).
    Off,
}

/// A lead's "find it elsewhere" links, kept with the answer (clause 63).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TasteLinks {
    Asking,
    Ready(Vec<PluginLink>),
    Failed(String),
}

/// A song the user picked to start from (clause 14). `random` marks the
/// fallback card's "another pick" (clause 21), which the next such pick
/// replaces in place.
#[derive(Debug, Clone, PartialEq)]
pub struct TastePin {
    pub track: Track,
    pub random: bool,
}

/// Why a pin was not taken. The GUI words it (`gui.discover.pins_full`,
/// `gui.discover.pick_other_server`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TastePinRefusal {
    /// Eight stand already.
    Full,
    /// Already a pin or a seed — nothing to add.
    AlreadyThere,
    /// Not this server's song: a pin seeds on ONE server's history. Carries
    /// the server to name.
    OtherServer(String),
}

/// A row the room's verbs act on: a Library song by its index in
/// `library.tracks` (albums are containers, clause 61), or a Peers row by
/// its group and its index in that group (clause 70).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasteRow {
    Song(usize),
    Peer { group: usize, row: usize },
}

/// What a row verb does (clauses 60 and 62).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasteVerb {
    /// A click or Enter: the row plays and the tab's songs queue after it.
    PlayFrom,
    /// `[▸]` / `P`.
    Now,
    /// `[»]` / `N`.
    Next,
    /// `[+]` / `a`.
    Queue,
}

/// Everything the room holds. Reset with the session (a server switch drops
/// the answer, the pins and the prefs read — clause 31), except the tab,
/// which is the room's one memory of its own (clause 40).
#[derive(Debug, Clone, Default)]
pub struct TasteState {
    /// The last seq handed out; each ask takes the next.
    seq: u64,
    /// The session generation the state belongs to.
    pub(crate) generation: u64,
    pub tab: TasteTab,
    /// The window chosen in this session; `None` lets the saved one apply
    /// (clause 16).
    pub window: Option<u32>,
    /// The filter chosen in this session; `None` as for `window`.
    pub filter: Option<TasteFilter>,
    /// The saved prefs, read once a session for `staleDays` (clause 16).
    pub prefs: Option<TastePrefs>,
    prefs_asked: bool,
    pub pins: Vec<TastePin>,
    /// Seed hashes dropped this session: sent as `exclude` on every ask.
    pub dropped: Vec<String>,
    /// A restore's `PUT …/prefs` is out; the ask follows its answer, with
    /// `remember` as said (clause 13).
    ask_after_prefs: Option<bool>,
    /// The last Main answer's facts — seeds, header, excluded — with its
    /// lists moved into the slots below.
    pub head: Option<TasteResponse>,
    pub library: TasteSlot<TasteLibrary>,
    pub network: TasteSlot<TasteNetwork>,
    pub peers: TasteSlot<TastePeers>,
    pub latch: TasteLatch,
    /// The fallback card's memory (clause 20): set by a pinless answer that
    /// fell back to the server's random seed, cleared by one with history.
    pub no_history: bool,
    /// Whether the Main ask in flight carried pins.
    main_pinned: bool,
    /// The seq of the last Main ask. The head's facts (seeds, header, the
    /// card's memory, the reuse clock) are Main's alone and land by THIS,
    /// not by the library slot's seq: a filter change's Library ask moves
    /// that slot while Main is out, and must not cost Main its head.
    main_seq: u64,
    /// That Main ask is out.
    main_pending: bool,
    /// "Another pick"'s own re-draws since the user last pressed it.
    redraws: u8,
    /// When the last complete Main answer landed (the 5-minute reuse).
    pub arrived: Option<crate::clock::Instant>,
    /// Links per lead key (`TasteLead::key`).
    pub links: HashMap<String, TasteLinks>,
    /// The `links` plug-ins' names, once read this session.
    link_plugins: Option<Vec<String>>,
    /// "Another pick" is out.
    pub random_pending: bool,
    /// The last capture pick the room refused, for the GUI to word once
    /// and take (clause 14).
    pub refused: Option<TastePinRefusal>,
}

impl TasteState {
    /// The window the header's dropdown shows: the one ASKED for (clause
    /// 15) — `widenedFrom` when the server widened, else its `window`, else
    /// this session's choice. While a Main ask carrying this session's
    /// choice is out, that choice: the old answer's window would make a
    /// re-pick of it a no-op against the new ask, and `]` pressed twice
    /// step from the same place twice.
    pub fn shown_window(&self) -> Option<u32> {
        if self.main_pending && self.window.is_some() {
            return self.window;
        }
        self.head
            .as_ref()
            .and_then(|head| head.widened_from.or(head.window))
            .or(self.window)
            .or_else(|| self.prefs.as_ref().and_then(|p| p.window))
    }

    /// The library filter the bottom row shows (clause 65): this session's
    /// choice, else the answer's, else the saved one, else `fresh`.
    pub fn shown_filter(&self) -> TasteFilter {
        self.filter
            .or_else(|| self.library.data.as_ref().map(|l| l.filter))
            .or_else(|| self.prefs.as_ref().and_then(|p| p.filter))
            .filter(|f| *f != TasteFilter::Other)
            .unwrap_or_default()
    }

    /// `fresh`'s n — "Not in n days" (clause 65).
    pub fn stale_days(&self) -> u32 {
        self.prefs.as_ref().and_then(|p| p.stale_days).unwrap_or(TASTE_STALE_DAYS)
    }

    /// Scan pending, by the answer or by a 403's probe (clauses 81–82).
    pub fn scan_pending(&self) -> bool {
        self.latch == TasteLatch::ScanPending || self.head.as_ref().is_some_and(|h| h.not_analyzed)
    }

    /// Any part still out — the state line's `• asking…` (clause 41) — or
    /// "another pick", whose ask follows its song.
    pub fn asking(&self) -> bool {
        self.library.pending
            || self.network.pending
            || self.peers.pending
            || self.random_pending
            || self.latch == TasteLatch::Probing
    }

    /// The seed whose hash a row names — the row's "sounds like" (clause 56).
    pub fn seed(&self, hash: &str) -> Option<&crate::api::types::TasteSeed> {
        self.head.as_ref()?.seeds.iter().find(|seed| seed.hash == hash)
    }

    /// The Peers tab's playable rows in order, each with its peer's id.
    pub fn peer_rows(&self) -> Vec<(i64, &TasteLead)> {
        self.peers
            .data
            .iter()
            .flat_map(|p| &p.peers)
            .flat_map(|group| group.results.iter().map(move |row| (group.id, row)))
            .filter(|(_, row)| row.filepath.is_some())
            .collect()
    }

    /// The network lead at `index`.
    pub fn lead(&self, index: usize) -> Option<&TasteLead> {
        self.network.data.as_ref()?.results.get(index)
    }

    fn reset(&mut self) {
        let tab = self.tab;
        *self = TasteState { tab, ..TasteState::default() };
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

impl App {
    // ── Asking ──────────────────────────────────────────────────────────────

    /// The room is entered (clause 3): an answer that landed within five
    /// minutes is shown as it stands; anything older, failed or never
    /// asked asks again.
    pub(crate) fn taste_open(&mut self) -> Vec<Effect> {
        self.taste_open_at(crate::clock::Instant::now())
    }

    /// [`App::taste_open`] against a given clock, for the reuse window's
    /// tests.
    pub(crate) fn taste_open_at(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        self.taste_sync();
        let t = &self.taste;
        let failed = t.library.failed.is_some() || t.peers.failed.is_some();
        let fresh = t.arrived.is_some_and(|at| now.saturating_duration_since(at) < TASTE_REUSE);
        if (fresh && !failed) || t.asking() || t.latch == TasteLatch::Off {
            return Vec::new();
        }
        self.taste_ask(false)
    }

    /// Ask again now, whatever the reuse window says: `G` or the nav row
    /// while the room is up (clause 3), Check again (clause 81), Retry
    /// (clause 83). Both parts, as a fresh visit asks.
    pub(crate) fn taste_ask_again(&mut self) -> Vec<Effect> {
        self.taste_sync();
        self.taste_ask(false)
    }

    /// Both parts (clause 30): Main always, Peers while the server federates
    /// discovery; and the prefs, once a session. `remember` rides Main alone.
    pub(crate) fn taste_ask(&mut self, remember: bool) -> Vec<Effect> {
        self.taste_sync();
        if !self.connected || !self.capabilities.taste() || self.session.peer.is_some() {
            return Vec::new();
        }
        self.taste.latch = TasteLatch::Clear;
        let mut effects = vec![self.taste_request(TastePart::Main, remember)];
        if self.capabilities.federation_discovery {
            effects.push(self.taste_request(TastePart::Peers, false));
        } else {
            self.taste.peers = TasteSlot::default();
        }
        if !self.taste.prefs_asked {
            self.taste.prefs_asked = true;
            effects.push(Effect::Api(ApiCmd::TastePrefs { generation: self.session_gen, patch: None }));
        }
        effects
    }

    /// One part's ask: the body the contract names for it (clause 30), a new
    /// seq on each slot it fills, the session generation on the command.
    pub(crate) fn taste_request(&mut self, part: TastePart, remember: bool) -> Effect {
        self.taste_sync();
        let seq = self.taste.next_seq();
        let p2p = self.capabilities.discovery_p2p;
        let sources: Vec<String> = match part {
            TastePart::Main if p2p => vec!["library".into(), "network".into()],
            TastePart::Main | TastePart::Library => vec!["library".into()],
            TastePart::Peers => vec!["peers".into()],
        };
        let lists_library = part != TastePart::Peers;
        let body = TasteRequest {
            window: self.taste.window,
            pin: self.taste.pins.iter().map(|pin| pin.track.filepath.clone()).collect(),
            exclude: newest(&self.taste.dropped, TASTE_EXCLUDE_CAP),
            // Only the library-bearing ask writes the prefs; the peers ask
            // would write the same thing twice (clause 30).
            remember: (remember && lists_library).then_some(true),
            filter: if lists_library { self.taste.filter } else { None },
            // Read, never written here (Out of scope).
            stale_days: None,
            sources,
            // Not ported in v1 (Out of scope): said outright, so a server
            // default that changes cannot change the room.
            new_artists_only: Some(false),
            ignore_v_paths: Vec::new(),
        };
        match part {
            TastePart::Main => {
                self.taste.library.ask(seq);
                if p2p {
                    self.taste.network.ask(seq);
                } else {
                    self.taste.network = TasteSlot::default();
                }
                self.taste.main_pinned = !self.taste.pins.is_empty();
                self.taste.main_seq = seq;
                self.taste.main_pending = true;
            }
            TastePart::Library => self.taste.library.ask(seq),
            TastePart::Peers => self.taste.peers.ask(seq),
        }
        Effect::Api(ApiCmd::Taste { seq, generation: self.session_gen, part, body: Box::new(body) })
    }

    /// The window dropdown (clause 15): both parts, remembered.
    pub(crate) fn set_taste_window(&mut self, window: u32) -> Vec<Effect> {
        self.taste_sync();
        // Picking the window already shown changes nothing (clause 15).
        if self.taste.shown_window() == Some(window) {
            return Vec::new();
        }
        self.taste.window = Some(window);
        self.taste_ask(true)
    }

    /// The library filter (clause 65): the library alone, remembered; the
    /// Network and Peers rows stand.
    pub(crate) fn set_taste_filter(&mut self, filter: TasteFilter) -> Vec<Effect> {
        self.taste_sync();
        if self.taste.shown_filter() == filter {
            return Vec::new();
        }
        self.taste.filter = Some(filter);
        if !self.connected || !self.capabilities.taste() {
            return Vec::new();
        }
        vec![self.taste_request(TastePart::Library, true)]
    }

    // ── Seeds and pins (clauses 13, 14, 21) ─────────────────────────────────

    /// The seeds modal closed. `dropped` and `restored` are hashes the user
    /// unchecked and re-checked; `pins_changed` says a pin came or went. A
    /// modal closed with nothing changed asks nothing. A restored seed is a
    /// `PUT …/prefs` with the saved exclusions minus it, and the ask waits
    /// for that answer; new drops go out on the ask as `exclude` with
    /// `remember` (plan §2.3).
    pub(crate) fn taste_seeds_closed(
        &mut self,
        dropped: &[String],
        restored: &[String],
        pins_changed: bool,
    ) -> Vec<Effect> {
        self.taste_sync();
        let mut changed = pins_changed;
        let mut newly_dropped = false;
        for hash in dropped {
            if !self.taste.dropped.contains(hash) && !restored.contains(hash) {
                self.taste.dropped.push(hash.clone());
                changed = true;
                newly_dropped = true;
            }
        }
        // What the server keeps: its prefs, the answer's resolvable
        // exclusions, and this session's drops (sent with `remember`).
        let mut saved: Vec<String> = self.taste.prefs.as_ref().map(|p| p.exclude.clone()).unwrap_or_default();
        if let Some(head) = &self.taste.head {
            saved.extend(head.excluded.iter().map(|e| e.hash.clone()));
        }
        saved.extend(self.taste.dropped.iter().cloned());
        let restoring: Vec<&String> = restored.iter().filter(|hash| saved.contains(hash)).collect();
        self.taste.dropped.retain(|hash| !restored.contains(hash));
        if !changed && restoring.is_empty() {
            return Vec::new();
        }
        if !restoring.is_empty() {
            let mut exclude: Vec<String> = Vec::new();
            for hash in saved {
                if !restored.contains(&hash) && !exclude.contains(&hash) {
                    exclude.push(hash);
                }
            }
            self.taste.ask_after_prefs = Some(newly_dropped);
            // The server's list is oldest first and its cap a hard limit on
            // a PUT: 200 saved plus this session's drops would be a 400.
            let exclude = newest(&exclude, TASTE_EXCLUDE_CAP);
            let patch = TastePrefsPatch { exclude: Some(exclude), ..TastePrefsPatch::default() };
            return vec![Effect::Api(ApiCmd::TastePrefs { generation: self.session_gen, patch: Some(patch) })];
        }
        self.taste_ask(newly_dropped)
    }

    /// Whether "Pin the playing song" is offered (clause 14): the playing
    /// row is the session server's own, not a peer's, not a pin or a seed
    /// already, and fewer than eight pins stand — otherwise hidden.
    pub fn taste_can_pin_playing(&self) -> bool {
        let Some(item) = self.queue.current.and_then(|i| self.queue.items.get(i)) else { return false };
        self.session.peer.is_none()
            && item.origin.peer.is_none()
            && self.is_session_origin(&item.origin)
            && self.taste.pins.len() < TASTE_MAX_PINS
            && !self.taste_holds(&item.filepath)
    }

    /// Pin the playing song; the ask waits for the modal to close.
    pub(crate) fn taste_pin_playing(&mut self) -> bool {
        if !self.taste_can_pin_playing() {
            return false;
        }
        let Some(track) = self.queue.current.and_then(|i| self.queue.items.get(i)).map(|q| q.track.clone())
        else {
            return false;
        };
        self.taste.pins.push(TastePin { track, random: false });
        true
    }

    /// "Pin a song…" / "choose a song…" (clauses 14 and 21): arm the
    /// room's own capture, in the sonic room's grammar; the pick lands in
    /// [`App::taste_picked`].
    pub(crate) fn taste_choose_song(&mut self) -> Vec<Effect> {
        self.taste.refused = None;
        self.arm_capture(Capture::TasteSeed)
    }

    /// Add a pin without asking — the modal's, which asks on close.
    pub(crate) fn taste_add_pin(&mut self, track: Track) -> Result<(), TastePinRefusal> {
        self.taste_sync();
        if self.session.peer.is_some() {
            return Err(TastePinRefusal::OtherServer(self.server_display()));
        }
        if self.taste_holds(&track.filepath) {
            return Err(TastePinRefusal::AlreadyThere);
        }
        if self.taste.pins.len() >= TASTE_MAX_PINS {
            return Err(TastePinRefusal::Full);
        }
        self.taste.pins.push(TastePin { track, random: false });
        Ok(())
    }

    /// Unpin (`x`, clause 13); the ask waits for the modal to close.
    pub(crate) fn taste_unpin(&mut self, filepath: &str) -> bool {
        let before = self.taste.pins.len();
        self.taste.pins.retain(|pin| pin.track.filepath != filepath);
        self.taste.pins.len() != before
    }

    /// A capture pick landed (clause 14: "a pin asks … when the pick
    /// lands"). A refusal leaves the pins as they were.
    pub(crate) fn taste_picked(&mut self, track: Track) -> Result<Vec<Effect>, TastePinRefusal> {
        self.taste_add_pin(track)?;
        Ok(self.taste_ask(false))
    }

    /// "Another pick" (`r`, clause 21): one random song of this server,
    /// pinned in place of the previous such pick when it lands.
    pub(crate) fn taste_another_pick(&mut self) -> Vec<Effect> {
        self.taste_sync();
        if !self.connected || self.taste.random_pending {
            return Vec::new();
        }
        self.taste.redraws = 0;
        self.taste_draw()
    }

    fn taste_draw(&mut self) -> Vec<Effect> {
        self.taste.random_pending = true;
        vec![Effect::Api(ApiCmd::TasteRandom { generation: self.session_gen })]
    }

    fn taste_holds(&self, filepath: &str) -> bool {
        self.taste.pins.iter().any(|pin| pin.track.filepath == filepath)
            || self.taste.head.as_ref().is_some_and(|h| h.seeds.iter().any(|s| s.filepath == filepath))
    }

    // ── Rows ────────────────────────────────────────────────────────────────

    /// A row verb (clauses 60 and 62). Library songs carry the session's
    /// origin; a Peers row the parent's with the peer's id, so its bytes
    /// come through `/api/v1/federation/peers/{id}/stream/…` (or the peer's
    /// own tunnel) and its play is reported with `peerId` and the row's
    /// snapshot (multi-server clause 27, play-reporting clause 7).
    pub(crate) fn activate_taste_row(&mut self, row: TasteRow, verb: TasteVerb) -> Vec<Effect> {
        match row {
            TasteRow::Song(index) => {
                let Some(track) = self.taste_songs().get(index).cloned() else { return Vec::new() };
                let origin = self.origin();
                match verb {
                    TasteVerb::PlayFrom => {
                        let songs = self.taste_songs();
                        self.replace_queue(songs);
                        self.play_index(index)
                    }
                    TasteVerb::Now => self.queue_track_next(&origin, track, true),
                    TasteVerb::Next => self.queue_track_next(&origin, track, false),
                    TasteVerb::Queue => self.queue_track_end(&origin, track),
                }
            }
            TasteRow::Peer { group, row } => {
                let rows = self.taste_peer_items();
                let Some(at) = self.taste_peer_position(group, row) else { return Vec::new() };
                let Some(item) = rows.get(at).cloned() else { return Vec::new() };
                match verb {
                    TasteVerb::PlayFrom => {
                        self.queue.replace(rows);
                        self.play_index(at)
                    }
                    TasteVerb::Now => self.queue_track_next(&item.origin, item.track, true),
                    TasteVerb::Next => self.queue_track_next(&item.origin, item.track, false),
                    TasteVerb::Queue => self.queue_track_end(&item.origin, item.track),
                }
            }
        }
    }

    /// The bottom row's play and shuffle (clause 64, browser-top-bar clauses
    /// 10 and 12): the tab's playable rows replace the queue; `shuffle`
    /// reorders them once. Network has nothing to play.
    pub(crate) fn taste_play_all(&mut self, shuffle: bool) -> Vec<Effect> {
        let mut items = self.taste_tab_items();
        if items.is_empty() {
            return Vec::new();
        }
        if shuffle {
            fastrand::shuffle(&mut items);
        }
        self.queue.replace(items);
        self.play_index(0)
    }

    /// The bottom row's queue all (clause 64, browser-top-bar clause 11):
    /// append, say how many, and start an empty idle queue.
    pub(crate) fn taste_queue_all(&mut self) -> Vec<Effect> {
        let items = self.taste_tab_items();
        if items.is_empty() {
            return Vec::new();
        }
        let count = items.len();
        let was_empty = self.queue.items.is_empty();
        for item in items {
            self.queue.push(item);
        }
        self.info(format!("queued {count}"));
        if was_empty && self.status.is_idle() {
            return self.play_index(0);
        }
        Vec::new()
    }

    /// An album row opens in the Albums room's drill (clause 61): the node
    /// its click asks for — by NAME alone, as the Albums room asks. The row's
    /// credit is the ALBUM artist (or the first of several), but
    /// `album-songs`' `artist` key filters by TRACK artist: sent, a "Various
    /// Artists" compilation would open empty and a two-credit album open
    /// with one credit's tracks (mStream's db.js album-songs).
    pub fn taste_album_node(&self, index: usize) -> Option<LibraryNode> {
        let album = self.taste.library.data.as_ref()?.albums.get(index)?;
        let name = album.name.clone().filter(|n| !n.is_empty())?;
        Some(LibraryNode::Album { name, artist: None })
    }

    /// The Library tab's songs as tracks, in the server's order.
    fn taste_songs(&self) -> Vec<Track> {
        self.taste.library.data.iter().flat_map(|l| &l.tracks).map(|t| t.to_track()).collect()
    }

    /// The Peers tab's playable rows as queue rows, each stamped with the
    /// session server and its own peer.
    fn taste_peer_items(&self) -> Vec<Queued> {
        let server = self.origin().server;
        self.taste
            .peer_rows()
            .into_iter()
            .filter_map(|(peer, row)| {
                let origin = Origin { server: server.clone(), peer: Some(peer) };
                row.to_track().map(|track| Queued { dj: None, origin, track })
            })
            .collect()
    }

    /// Where row `row` of group `group` sits among the playable rows.
    fn taste_peer_position(&self, group: usize, row: usize) -> Option<usize> {
        let groups = &self.taste.peers.data.as_ref()?.peers;
        let target = groups.get(group)?.results.get(row)?;
        target.filepath.as_ref()?;
        let before: usize = groups[..group]
            .iter()
            .map(|g| g.results.iter().filter(|r| r.filepath.is_some()).count())
            .sum();
        let within = groups[group].results[..row].iter().filter(|r| r.filepath.is_some()).count();
        Some(before + within)
    }

    /// The tab in front's playable rows (clause 64).
    fn taste_tab_items(&self) -> Vec<Queued> {
        match self.taste.tab {
            TasteTab::Library => self.queued_all(self.taste_songs()),
            TasteTab::Peers => self.taste_peer_items(),
            TasteTab::Network => Vec::new(),
        }
    }

    // ── Leads (clause 63) ───────────────────────────────────────────────────

    /// The menu opened on lead `index`: its links are asked for the first
    /// time it opens, and kept with the answer. Nothing is asked while the
    /// server lists no plug-ins, or once the list held no `links` one.
    pub(crate) fn taste_links_for(&mut self, index: usize) -> Vec<Effect> {
        self.taste_sync();
        if !self.capabilities.discovery_plugins {
            return Vec::new();
        }
        if self.taste.link_plugins.as_ref().is_some_and(Vec::is_empty) {
            return Vec::new();
        }
        let Some(lead) = self.taste.lead(index) else { return Vec::new() };
        let key = lead.key();
        if matches!(self.taste.links.get(&key), Some(TasteLinks::Asking | TasteLinks::Ready(_))) {
            return Vec::new();
        }
        let recommendation = Box::new(lead.raw.clone());
        self.taste.links.insert(key.clone(), TasteLinks::Asking);
        vec![Effect::Api(ApiCmd::TasteLinks {
            generation: self.session_gen,
            key,
            recommendation,
            plugins: self.taste.link_plugins.clone(),
        })]
    }

    /// THIS LEAD's copy text, "Artist – Title" (clause 63).
    pub fn taste_copy_text(&self, index: usize) -> Option<String> {
        self.taste.lead(index).map(TasteLead::artist_title).filter(|text| !text.is_empty())
    }

    /// Whether the menu has a FIND ELSEWHERE section to draw at all.
    pub fn taste_offers_links(&self) -> bool {
        self.capabilities.discovery_plugins && !self.taste.link_plugins.as_ref().is_some_and(Vec::is_empty)
    }

    // ── Replies ─────────────────────────────────────────────────────────────

    /// Drop everything the room holds when the session moved to another
    /// server (clause 31), keeping the tab.
    pub(crate) fn reset_taste(&mut self) {
        self.taste.reset();
        self.taste.generation = self.session_gen;
    }

    /// The state belongs to the session it was filled under; a moved
    /// generation means a server switch the reset did not see.
    fn taste_sync(&mut self) {
        if self.taste.generation != self.session_gen {
            self.reset_taste();
        }
    }

    pub(super) fn consume_taste(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Taste { seq, generation, part, answer } => {
                if generation != self.session_gen {
                    return Vec::new();
                }
                self.taste_sync();
                self.taste_answered(seq, part, answer)
            }
            Event::TastePrefs { generation, wrote, answer } => {
                if generation != self.session_gen {
                    return Vec::new();
                }
                self.taste_sync();
                match answer {
                    Ok(prefs) => self.taste.prefs = Some(prefs),
                    // A failed restore is said, not pretended: the seed stays
                    // dropped on the server, and the ask that follows (for
                    // any new drops) shows it under Dropped still.
                    Err(err) if wrote => {
                        self.error(t!("gui.discover.restore_failed", err = err).to_string());
                    }
                    // A failed read is asked again on the next visit.
                    Err(_) => self.taste.prefs_asked = false,
                }
                match (wrote, self.taste.ask_after_prefs.take()) {
                    (true, Some(remember)) => self.taste_ask(remember),
                    _ => Vec::new(),
                }
            }
            Event::TasteRandom { generation, track } => {
                if generation != self.session_gen {
                    return Vec::new();
                }
                self.taste_sync();
                self.taste.random_pending = false;
                let Some(track) = track else {
                    self.error("couldn't fetch a song from the server");
                    return Vec::new();
                };
                // In place of the previous such pick (clause 21).
                self.taste.pins.retain(|pin| !pin.random);
                if self.taste.pins.len() >= TASTE_MAX_PINS || self.taste_holds(&track.filepath) {
                    return Vec::new();
                }
                self.taste.pins.push(TastePin { track: *track, random: true });
                self.taste_ask(false)
            }
            Event::TasteLinks { generation, key, plugins, answer } => {
                if generation != self.session_gen {
                    return Vec::new();
                }
                self.taste_sync();
                if let Some(plugins) = plugins {
                    self.taste.link_plugins = Some(plugins);
                }
                let state = match answer {
                    Ok(links) => TasteLinks::Ready(links),
                    Err(err) => TasteLinks::Failed(err),
                };
                // A failure is not kept as the answer: the next open asks
                // again.
                self.taste.links.insert(key, state);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn taste_answered(
        &mut self,
        seq: u64,
        part: TastePart,
        answer: Result<Box<TasteResponse>, TasteFailure>,
    ) -> Vec<Effect> {
        let t = &mut self.taste;
        // Which slots this reply may still fill: last ask wins per list.
        let library = part != TastePart::Peers && t.library.seq == seq;
        let network = part == TastePart::Main && t.network.seq == seq;
        let peers = part == TastePart::Peers && t.peers.seq == seq;
        // And whether it is the latest Main, whose head it carries whatever
        // a later Library ask did to the library slot.
        let main = part == TastePart::Main && t.main_seq == seq;
        if !library && !network && !peers && !main {
            return Vec::new();
        }
        if main {
            t.main_pending = false;
        }
        // "Another pick" drew an unanalyzed song (clause 21): its answer
        // would say `notAnalyzed` and turn every pane to the scan sentence
        // under a server that has analyzed plenty. Draw again in its place,
        // a few times, before believing it.
        if let Ok(response) = &answer
            && main
            && response.not_analyzed
            && t.redraws < TASTE_REDRAWS
            && t.pins.iter().any(|pin| {
                pin.random && response.seeds.iter().any(|s| s.filepath == pin.track.filepath && !s.analyzed)
            })
        {
            t.redraws += 1;
            t.pins.retain(|pin| !pin.random);
            for (wanted, slot_pending) in [(library, &mut t.library.pending), (network, &mut t.network.pending)] {
                if wanted {
                    *slot_pending = false;
                }
            }
            return self.taste_draw();
        }
        let t = &mut self.taste;
        let mut response = match answer {
            Ok(response) => response,
            Err(TasteFailure::Failed(err)) => {
                if library {
                    t.library.pending = false;
                    t.library.failed = Some(err.clone());
                }
                if network {
                    t.network.pending = false;
                    t.network.failed = Some(err.clone());
                }
                if peers {
                    t.peers.pending = false;
                    t.peers.failed = Some(err);
                }
                return Vec::new();
            }
            Err(TasteFailure::Forbidden) => {
                for (wanted, slot_pending) in [
                    (library, &mut t.library.pending),
                    (network, &mut t.network.pending),
                    (peers, &mut t.peers.pending),
                ] {
                    if wanted {
                        *slot_pending = false;
                    }
                }
                // One probe, however many parts were refused (clause 82).
                if matches!(t.latch, TasteLatch::Probing | TasteLatch::Off) {
                    return Vec::new();
                }
                t.latch = TasteLatch::Probing;
                return vec![Effect::Api(ApiCmd::DiscoveryProbe)];
            }
        };
        let random = response.seed_source == SeedSource::Random || response.fallback;
        if library {
            let lists = response.library.take();
            t.library.pending = false;
            t.library.failed = None;
            // A random seed still searches the library (clause 20): the
            // rows stand under the card.
            t.library.absent = lists.is_none() && !random;
            t.library.data = lists;
        }
        if network {
            let lists = response.network.take();
            t.network.pending = false;
            t.network.failed = None;
            t.network.absent = lists.is_none() && !random;
            t.network.not_asked_random = lists.is_none() && random;
            t.network.data = lists;
        }
        if peers {
            let lists = response.peers.take();
            t.peers.pending = false;
            t.peers.failed = None;
            t.peers.absent = lists.is_none() && !random;
            t.peers.not_asked_random = lists.is_none() && random;
            t.peers.data = lists;
        }
        if main {
            // The fallback card's memory (clause 20): learned from a
            // pinless answer, kept until one carries history seeds.
            let history = !random && response.seeds.iter().any(|seed| !seed.pinned);
            if response.fallback && !t.main_pinned {
                t.no_history = true;
            } else if history {
                t.no_history = false;
            }
            response.library = None;
            response.network = None;
            response.peers = None;
            t.head = Some(*response);
            t.arrived = Some(crate::clock::Instant::now());
            // A new answer, new leads: the links were kept with the old one.
            t.links.clear();
        }
        Vec::new()
    }

    /// The probe's verdict for the room (clause 82): the gate still on is
    /// the scan not run yet; the gate gone is switched off. A probe the
    /// room did not ask for changes nothing here.
    pub(super) fn consume_taste_probe(&mut self, taste: bool) -> Vec<Effect> {
        if self.taste.latch != TasteLatch::Probing {
            return Vec::new();
        }
        self.taste.latch = if taste { TasteLatch::ScanPending } else { TasteLatch::Off };
        Vec::new()
    }

    /// A fresh ping is the word on the gate now (clause 1): a room switched
    /// off by an earlier 403 asks again on the next visit if the flag is
    /// back.
    pub(super) fn taste_reconnected(&mut self) {
        if self.taste.latch == TasteLatch::Off {
            self.taste.latch = TasteLatch::Clear;
        }
    }

    /// An error cleared every wait (`clear_pending`): a probe or a part
    /// nothing will answer must not leave "asking…" standing (clauses 41
    /// and 82) — nor keep [`App::taste_open`] from asking after a re-login
    /// to the same server, which resets nothing. Each part still out ends
    /// as a failure, so its pane offers Retry (clause 83); a reply that
    /// lands after all still fills it, by its seq.
    pub(super) fn clear_taste_waits(&mut self) {
        let t = &mut self.taste;
        if t.latch == TasteLatch::Probing {
            t.latch = TasteLatch::Clear;
        }
        let reason = || t!("gui.discover.state_failed").to_string();
        if t.library.pending {
            t.library.pending = false;
            t.library.failed = Some(reason());
        }
        if t.network.pending {
            t.network.pending = false;
            t.network.failed = Some(reason());
        }
        if t.peers.pending {
            t.peers.pending = false;
            t.peers.failed = Some(reason());
        }
        t.main_pending = false;
        t.random_pending = false;
    }
}

/// The last `cap` of `list`, which is oldest first.
fn newest(list: &[String], cap: usize) -> Vec<String> {
    list[list.len().saturating_sub(cap)..].to_vec()
}
