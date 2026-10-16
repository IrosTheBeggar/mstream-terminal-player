//! The GUI's Discover room: the history-seeded page (docs/ux-contracts/
//! discovery-page.md; mStream's docs/discovery-page-plan.md §3.1–§3.3).
//!
//! The room keeps **its own row model** here, the sonic room's precedent:
//! nothing in this file reads or writes the TUI's Discover tab (`App.discover`,
//! its drill and seed), the Now Playing panel's neighbours (`App.now_discover`)
//! or `Capture::Discover` — clause 67. The GUI draws from [`TasteState`] and
//! calls the `taste_*` methods; every queue row they build is stamped here,
//! the session's origin on a Library song and the peer's on a connected
//! server's row.
//!
//! The room is the PLAY-NOW page (the contract's 2026-10-11 log): only songs
//! you can play now — your library's and your connected servers'. The p2p
//! network's leads are not asked here; a later Explore room draws them.
//!
//! The room is STATELESS on the server (the contract's 2026-10-14 log):
//! every ask draws a new random batch of the songs you finished, and
//! nothing is saved there. What the room keeps is local — the window in
//! `[gui] discover_window` (the GUI's) and, for one visit, the pins, the
//! dropped seeds and the random picks passed over.
//!
//! The answer comes in two parts (clause 30): **Main** (the library), then
//! **Peers** (the connected servers), asked once Main lands and pinned to
//! Main's seeds — each call draws its own batch, so both tabs and the seeds
//! line share Main's. Each list is a [`TasteSlot`] holding the seq of the
//! ask it waits for, so the last ask wins per list and a slow part never
//! overwrites a newer one (clause 31).

use super::*;
use crate::api::types::{
    SeedSource, TasteLead, TasteLibrary, TasteLimit, TastePeerGroup, TastePeers, TasteRequest, TasteResponse,
};
use crate::tui::worker::{TasteFailure, TastePart};

/// The user picks at most this many songs (clause 14).
pub const TASTE_MAX_PINS: usize = 8;

/// The server takes at most this many pins — its `PIN_CAP`, the seed cap
/// (plan §3.1): the peers ask pins the Main answer's whole batch (clause
/// 30).
pub const TASTE_PIN_CAP: usize = 12;

/// The server refuses an `exclude` longer than this outright (plan §3.1;
/// mStream's taste-queries.js `EXCLUDE_CAP`).
pub const TASTE_EXCLUDE_CAP: usize = 200;

/// The window dropdown's choices, in days (0 = all time) — clause 15.
pub const TASTE_WINDOWS: [u32; 4] = [7, 30, 90, 0];

/// The window asked for when none was chosen: the server's own default
/// (plan §2.2), and `[gui] discover_window`'s.
pub const TASTE_WINDOW_DEFAULT: u32 = 30;

/// A remembered window, when it is one of the four (clause 15); anything
/// else is no memory at all.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn taste_window(days: u32) -> Option<u32> {
    TASTE_WINDOWS.contains(&days).then_some(days)
}

/// The album strip's cap, the webapp's `ALBUM_STRIP_MAX` (the 2026-10-11
/// strip log). The server sends at most eight library albums when no
/// `limit` is asked, so the library's strip never drops one.
pub const TASTE_STRIP_MAX: usize = 12;

/// The longest art file name a connected server's row may carry — the
/// webapp's `PIN_PATH_MAX`, read the same way (`peerArtOf`).
const PEER_ART_MAX: usize = 4096;

/// One album on the Connected servers tab's strip: the playable rows of one
/// server that share an album and an artist (the webapp's `peerAlbums`).
#[derive(Debug, Clone, PartialEq)]
pub struct TastePeerAlbum {
    /// The server's id on the session's server — the federation proxy's.
    pub peer: i64,
    /// Its name, for the tile's tooltip.
    pub server: String,
    pub album: String,
    pub artist: Option<String>,
    /// The first art file one of its rows names, through the federation
    /// art proxy; `None` draws the note tile.
    pub art: Option<String>,
    /// Its rows as (group, row), best match first — what `[+]` queues when
    /// the server will not list the album.
    pub rows: Vec<(usize, usize)>,
}

/// A connected server's row's art file, when the row carries one — the
/// webapp's `peerArtOf`: the taste route's rows carry none today, and a
/// responder that adds one is picked up without a client change.
fn peer_art(lead: &TasteLead) -> Option<String> {
    ["art", "albumArt", "album-art", "album_art_file"].iter().find_map(|key| {
        lead.raw
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= PEER_ART_MAX)
            .map(str::to_string)
    })
}

/// The room's two tabs (clause 40), and what `[gui] discover_tab` stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TasteTab {
    #[default]
    Library,
    /// Your connected (federated) servers' songs.
    Servers,
}

impl TasteTab {
    /// The `GuiPrefs` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            TasteTab::Library => "library",
            TasteTab::Servers => "servers",
        }
    }

    /// A remembered tab; anything else is no memory at all. `peers` is the
    /// tab's spelling before the play-now page; `network`, a tab that left
    /// the room, reads as no memory and opens on Library.
    pub fn parse(word: &str) -> Option<TasteTab> {
        match word.trim() {
            "library" => Some(TasteTab::Library),
            "servers" | "peers" => Some(TasteTab::Servers),
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
    /// The source was asked in the latest round.
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

/// A song the user picked to start from (clause 14), for this visit.
#[derive(Debug, Clone, PartialEq)]
pub struct TastePin {
    pub track: Track,
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
/// `library.tracks` (albums are containers, clause 61), or a connected
/// server's row by its group and its index in that group (clause 53).
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
/// the answer and the visit's pins and drops — clause 31), except the tab
/// and the window, the room's memories of its own (clauses 15, 40), which
/// the GUI seeds from `[gui]`.
#[derive(Debug, Clone, Default)]
pub struct TasteState {
    /// The last seq handed out; each ask takes the next.
    seq: u64,
    /// The session generation the state belongs to.
    pub(crate) generation: u64,
    pub tab: TasteTab,
    /// The window asked for (clause 15); `None` asks the default, 30 days.
    pub window: Option<u32>,
    /// The songs picked to start from, for this visit (clause 14).
    pub pins: Vec<TastePin>,
    /// Seed hashes dropped in this visit (clause 13): sent as `exclude` on
    /// every ask until the room is entered afresh. Never stored anywhere.
    pub dropped: Vec<String>,
    /// The random picks passed over with "another pick" in this visit
    /// (clause 21): sent in `exclude` beside the drops, never drawn as
    /// dropped seeds. Emptied when an answer finds no pick left, and asked
    /// once more, so the card starts over (clause 21).
    pub skipped: Vec<String>,
    /// Every random pick the fallback card showed in this visit (clause
    /// 21), whatever ask drew it — `G`, a window change, Retry as well as
    /// "another pick": each call draws a fresh random song, so the list
    /// accumulates, and "another pick" passes them all over (even when a
    /// pin now stands in the card's place), so no earlier pick comes back.
    pub shown_random: Vec<String>,
    /// The last Main answer's facts — seeds, header, excluded — with its
    /// lists moved into the slots below.
    pub head: Option<TasteResponse>,
    pub library: TasteSlot<TasteLibrary>,
    pub peers: TasteSlot<TastePeers>,
    pub latch: TasteLatch,
    /// The fallback card's memory (clause 20): set by a pinless answer that
    /// fell back to the server's random seed, cleared by one with history.
    pub no_history: bool,
    /// Whether the Main ask in flight carried pins.
    main_pinned: bool,
    /// The seq of the last Main ask: the head's facts (seeds, header, the
    /// card's memory) are Main's alone and land by it.
    main_seq: u64,
    /// That Main ask is out.
    main_pending: bool,
    /// The Main ask whose answer the peers ask waits for (clause 30); 0
    /// when none waits. Only that Main, landed, asks the peers.
    peers_after: u64,
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
    }

    /// The asked window where the shown answer widened it to all time
    /// (`widenedFrom`) — none while a Main ask carrying a new choice is
    /// out, whose window the dropdown already shows (the GUI header's
    /// "Last 30 days → All time", discovery-page clause 15).
    pub fn widened_shown(&self) -> Option<u32> {
        if self.main_pending && self.window.is_some() {
            return None;
        }
        self.head.as_ref()?.widened_from
    }

    /// What every ask sends as `exclude` (clauses 13, 21): this visit's
    /// drops and passed-over random picks, newest kept under the cap.
    pub fn exclusions(&self) -> Vec<String> {
        let mut all: Vec<String> = Vec::new();
        for hash in self.skipped.iter().chain(&self.dropped) {
            if !all.contains(hash) {
                all.push(hash.clone());
            }
        }
        newest(&all, TASTE_EXCLUDE_CAP)
    }

    /// The answer's excluded seeds that the user dropped — the seeds
    /// modal's DROPPED section (clause 13); a passed-over random pick is
    /// not one.
    pub fn dropped_seeds(&self) -> Vec<&crate::api::types::TasteExcluded> {
        let Some(head) = &self.head else { return Vec::new() };
        head.excluded.iter().filter(|e| self.dropped.contains(&e.hash)).collect()
    }

    /// Scan pending, by the answer or by a 403's probe (clauses 81–82).
    pub fn scan_pending(&self) -> bool {
        self.latch == TasteLatch::ScanPending || self.head.as_ref().is_some_and(|h| h.not_analyzed)
    }

    /// Any part still out, or a 403's probe — what the GUI header's
    /// refresh reads as `↻ asking…` (discovery-page clause 17).
    pub fn asking(&self) -> bool {
        self.library.pending || self.peers.pending || self.latch == TasteLatch::Probing
    }

    /// The seed whose hash a row names — the row's "sounds like" (clause 56).
    pub fn seed(&self, hash: &str) -> Option<&crate::api::types::TasteSeed> {
        self.head.as_ref()?.seeds.iter().find(|seed| seed.hash == hash)
    }

    /// The Connected servers tab's rows in the server's order, as (group,
    /// row) in the answer: only what plays now (clause 53) — a server that
    /// did not answer or speaks another sonic model shows no rows, and a row
    /// without a file is not one.
    pub fn peer_lines(&self) -> Vec<(usize, usize)> {
        let shown = |group: &TastePeerGroup| group.reachable && !group.mismatched;
        let mut out = Vec::new();
        for (g, group) in self.peers.data.iter().flat_map(|p| &p.peers).enumerate() {
            if !shown(group) {
                continue;
            }
            for (r, row) in group.results.iter().enumerate() {
                if row.filepath.is_some() {
                    out.push((g, r));
                }
            }
        }
        out
    }

    /// A connected server's row and its server's id.
    pub fn peer_row(&self, group: usize, row: usize) -> Option<(i64, &TasteLead)> {
        let group = self.peers.data.as_ref()?.peers.get(group)?;
        Some((group.id, group.results.get(row)?))
    }

    /// The name of the server a connected server's row plays from (ON).
    pub fn peer_name(&self, group: usize) -> Option<&str> {
        Some(self.peers.data.as_ref()?.peers.get(group)?.name.as_str())
    }

    /// The Library tab's album strip (the 2026-10-11 strip log): indices
    /// into `library.albums`, best match first — the server's order where
    /// matches tie — capped at [`TASTE_STRIP_MAX`].
    pub fn library_strip(&self) -> Vec<usize> {
        let Some(library) = self.library.data.as_ref() else { return Vec::new() };
        let mut order: Vec<usize> = (0..library.albums.len()).collect();
        // A stable sort: equal matches keep the server's order.
        order.sort_by(|a, b| library.albums[*b].similarity.total_cmp(&library.albums[*a].similarity));
        order.truncate(TASTE_STRIP_MAX);
        order
    }

    /// The Connected servers tab's album strip, the webapp's `peerAlbums`:
    /// the rows that play now ([`TasteState::peer_lines`]), best match
    /// first across the servers, grouped by server + album + artist (case
    /// and surrounding blanks folded) — a row with no album tag joins no
    /// album. An album ranks by its best row, carries the first art one of
    /// its rows names, and is capped at [`TASTE_STRIP_MAX`].
    pub fn peer_albums(&self) -> Vec<TastePeerAlbum> {
        let fold = |s: Option<&str>| s.unwrap_or("").trim().to_lowercase();
        let mut rows: Vec<(usize, usize, i64, &TasteLead)> = self
            .peer_lines()
            .into_iter()
            .filter_map(|(g, r)| self.peer_row(g, r).map(|(peer, lead)| (g, r, peer, lead)))
            .collect();
        // A stable sort: equal matches keep the server's order.
        rows.sort_by(|a, b| b.3.similarity.total_cmp(&a.3.similarity));
        let mut out: Vec<TastePeerAlbum> = Vec::new();
        for (group, row, peer, lead) in rows {
            let Some(album) = lead.album.as_deref().map(str::trim).filter(|a| !a.is_empty()) else { continue };
            let artist = lead.artist.as_deref().map(str::trim).filter(|a| !a.is_empty());
            let key = (peer, fold(Some(album)), fold(artist));
            let found = out.iter().position(|a| (a.peer, fold(Some(&a.album)), fold(a.artist.as_deref())) == key);
            let at = match found {
                Some(at) => at,
                None => {
                    out.push(TastePeerAlbum {
                        peer,
                        server: self.peer_name(group).unwrap_or_default().to_string(),
                        album: album.to_string(),
                        artist: artist.map(str::to_string),
                        art: None,
                        rows: Vec::new(),
                    });
                    out.len() - 1
                }
            };
            let card = &mut out[at];
            if card.art.is_none() {
                card.art = peer_art(lead);
            }
            card.rows.push((group, row));
        }
        out.truncate(TASTE_STRIP_MAX);
        out
    }

    fn reset(&mut self) {
        let (tab, window) = (self.tab, self.window);
        *self = TasteState { tab, window, ..TasteState::default() };
    }

    /// A fresh visit (clause 3): the pins, the drops and the passed-over
    /// picks were the last visit's and go. The answer stays drawn until
    /// the new one lands (clause 32).
    fn new_visit(&mut self) {
        self.pins.clear();
        self.dropped.clear();
        self.skipped.clear();
        self.shown_random.clear();
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

impl App {
    // ── Asking ──────────────────────────────────────────────────────────────

    /// The room is entered afresh (clause 3): the last visit's pins, drops
    /// and passed-over picks go, and the room asks — every visit asks, no
    /// earlier answer is reused; the old rows stand until the new land.
    /// A 403's probe still out keeps its verdict: the visit starts afresh
    /// but asks nothing until the probe answers (clause 82).
    pub(crate) fn taste_open(&mut self) -> Vec<Effect> {
        self.taste_sync();
        self.taste.new_visit();
        if matches!(self.taste.latch, TasteLatch::Off | TasteLatch::Probing) {
            return Vec::new();
        }
        self.taste_ask()
    }

    /// Ask again now, the visit kept: `G` or the nav row while the room is
    /// up (clause 3), Check again (clause 81), Retry (clause 83). The same
    /// window, pins and drops — the server's new random batch is what makes
    /// the answer new.
    pub(crate) fn taste_ask_again(&mut self) -> Vec<Effect> {
        self.taste_sync();
        self.taste_ask()
    }

    /// The Main ask (clause 30); the peers half waits for its answer
    /// ([`App::taste_ask_peers`]) while the server federates discovery,
    /// marked out now so its tab reads "asking…" and an older peers reply
    /// fills nothing.
    pub(crate) fn taste_ask(&mut self) -> Vec<Effect> {
        self.taste_sync();
        if !self.connected || !self.capabilities.taste() || self.session.peer.is_some() {
            return Vec::new();
        }
        self.taste.latch = TasteLatch::Clear;
        let pins: Vec<String> = self.taste.pins.iter().map(|pin| pin.track.filepath.clone()).collect();
        let main = self.taste_request(TastePart::Main, pins, None);
        if self.capabilities.federation_discovery {
            let seq = self.taste.next_seq();
            self.taste.peers.ask(seq);
            self.taste.peers_after = self.taste.main_seq;
        } else {
            self.taste.peers = TasteSlot::default();
            self.taste.peers_after = 0;
        }
        vec![main]
    }

    /// One part's ask: the body the contract names for it (clause 30), a new
    /// seq on the slot it fills, the session generation on the command.
    fn taste_request(&mut self, part: TastePart, pin: Vec<String>, limit: Option<TasteLimit>) -> Effect {
        self.taste_sync();
        let seq = self.taste.next_seq();
        // The play-now page asks for nothing it cannot play: never the p2p
        // network (the contract's 2026-10-11 log).
        let sources: Vec<String> = match part {
            TastePart::Main => vec!["library".into()],
            TastePart::Peers => vec!["peers".into()],
        };
        let body = TasteRequest {
            window: Some(self.taste.window.unwrap_or(TASTE_WINDOW_DEFAULT)),
            pin,
            exclude: self.taste.exclusions(),
            sources,
            // Not ported in v1 (Out of scope): said outright, so a server
            // default that changes cannot change the room.
            new_artists_only: Some(false),
            ignore_v_paths: Vec::new(),
            limit,
        };
        match part {
            TastePart::Main => {
                self.taste.library.ask(seq);
                self.taste.main_pinned = !self.taste.pins.is_empty();
                self.taste.main_seq = seq;
                self.taste.main_pending = true;
            }
            TastePart::Peers => self.taste.peers.ask(seq),
        }
        Effect::Api(ApiCmd::Taste { seq, generation: self.session_gen, part, body: Box::new(body) })
    }

    /// The peers half, once the Main answer it waited for has landed
    /// (clause 30): every call draws its own batch, so it is pinned to that
    /// answer's seeds alone — the user's pins that resolved are among them,
    /// and one that did not would only push a seed out at the cap — capped
    /// at the server's [`TASTE_PIN_CAP`], with `limit.seeds` their count so
    /// the server adds none, and the Connected servers tab is built from
    /// the batch the seeds modal shows (the webapp's `peersRequest`). A
    /// random seed asks no connected server (clause 22), and neither does
    /// an answer with no seed at all.
    fn taste_ask_peers(&mut self, head: &TasteResponse, random: bool) -> Vec<Effect> {
        if !self.capabilities.federation_discovery {
            self.taste.peers = TasteSlot::default();
            return Vec::new();
        }
        let mut pins: Vec<String> = Vec::new();
        for path in head.seeds.iter().map(|seed| &seed.filepath) {
            if pins.len() >= TASTE_PIN_CAP {
                break;
            }
            if !path.is_empty() && !pins.contains(path) {
                pins.push(path.clone());
            }
        }
        if random || pins.is_empty() {
            let slot = &mut self.taste.peers;
            slot.pending = false;
            slot.failed = None;
            slot.absent = false;
            slot.not_asked_random = random;
            slot.data = None;
            return Vec::new();
        }
        let limit = TasteLimit { seeds: Some(pins.len() as u32) };
        vec![self.taste_request(TastePart::Peers, pins, Some(limit))]
    }

    /// The window dropdown (clause 15): both parts. The GUI keeps the
    /// choice in `[gui] discover_window`.
    pub(crate) fn set_taste_window(&mut self, window: u32) -> Vec<Effect> {
        self.taste_sync();
        // Picking the window already shown — and already asked for —
        // changes nothing (clause 15). One shown but not the one asked
        // (an ask for another still out) is asked, so the room, the ask
        // and `[gui] discover_window` agree.
        let asked = self.taste.window.unwrap_or(TASTE_WINDOW_DEFAULT);
        if self.taste.shown_window() == Some(window) && asked == window {
            return Vec::new();
        }
        self.taste.window = Some(window);
        self.taste_ask()
    }

    // ── Seeds and pins (clauses 13, 14, 21) ─────────────────────────────────

    /// The seeds modal closed. `dropped` and `restored` are hashes the user
    /// unchecked and re-checked ("use again"); `pins_changed` says a pin
    /// came or went. Drops and restores are this visit's alone — sent as
    /// `exclude`, never stored. A modal closed with nothing changed asks
    /// nothing.
    pub(crate) fn taste_seeds_closed(
        &mut self,
        dropped: &[String],
        restored: &[String],
        pins_changed: bool,
    ) -> Vec<Effect> {
        self.taste_sync();
        let mut changed = pins_changed;
        for hash in dropped {
            if !self.taste.dropped.contains(hash) && !restored.contains(hash) {
                self.taste.dropped.push(hash.clone());
                changed = true;
            }
        }
        let before = self.taste.dropped.len();
        self.taste.dropped.retain(|hash| !restored.contains(hash));
        changed |= self.taste.dropped.len() != before;
        if !changed {
            return Vec::new();
        }
        self.taste_ask()
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
        self.taste.pins.push(TastePin { track });
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
        self.taste.pins.push(TastePin { track });
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
        Ok(self.taste_ask())
    }

    /// "Another pick" (`r`, clause 21): the plain ask again, with every
    /// random pick the card showed in this visit passed over — the server
    /// draws a new random song on every call, and the exclusions keep it
    /// from drawing one already shown — and the card's own pick let go:
    /// pins alone make a page, so the server draws a random song only when
    /// none stands. An answer with no pick left starts the list over
    /// ([`App::taste_answered`]).
    pub(crate) fn taste_another_pick(&mut self) -> Vec<Effect> {
        self.taste_sync();
        if !self.connected {
            return Vec::new();
        }
        for hash in std::mem::take(&mut self.taste.shown_random) {
            if !self.taste.skipped.contains(&hash) {
                self.taste.skipped.push(hash);
            }
        }
        self.taste.pins.clear();
        self.taste_ask()
    }

    fn taste_holds(&self, filepath: &str) -> bool {
        self.taste.pins.iter().any(|pin| pin.track.filepath == filepath)
            || self.taste.head.as_ref().is_some_and(|h| h.seeds.iter().any(|s| s.filepath == filepath))
    }

    // ── Rows ────────────────────────────────────────────────────────────────

    /// A row verb (clauses 60 and 62). Library songs carry the session's
    /// origin; a connected server's row the parent's with that server's id
    /// (the Listen verb's way), so its bytes
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

    /// The verbs line's play and shuffle (clause 64, browser-top-bar clauses
    /// 10 and 12): the tab's rows replace the queue; `shuffle` reorders them
    /// once.
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

    /// The verbs line's queue all (clause 64, browser-top-bar clause 11):
    /// append, say how many, and start an empty idle queue.
    pub(crate) fn taste_queue_all(&mut self) -> Vec<Effect> {
        let items = self.taste_tab_items();
        self.taste_append(items)
    }

    /// The verbs line's save as playlist (clause 64, the webapp's "Save as
    /// playlist"): the house name prompt — the sonic room's, empty as the
    /// webapp's modal opens — over the Library tab's songs in the server's
    /// order; albums are containers. The Connected servers tab saves
    /// nothing: its songs are not this server's. False when nothing opened.
    pub(crate) fn taste_save_playlist(&mut self) -> bool {
        if self.taste.tab != TasteTab::Library {
            return false;
        }
        let files: Vec<String> =
            self.taste.library.data.iter().flat_map(|l| &l.tracks).map(|t| t.filepath.clone()).collect();
        if files.is_empty() {
            return false;
        }
        self.playlist_save_files = Some(files);
        self.sonic_playlist_name = Some(String::new());
        true
    }

    /// Append rows the way queue all does: say how many, and start an
    /// empty idle queue.
    fn taste_append(&mut self, items: Vec<Queued>) -> Vec<Effect> {
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

    /// An album row's `[+]` or `a` (clause 61, the webapp's album play
    /// control): its songs are asked by name, as the drill asks them, and
    /// appended when they land.
    pub(crate) fn taste_queue_album(&mut self, index: usize) -> Vec<Effect> {
        let Some(LibraryNode::Album { name, .. }) = self.taste_album_node(index) else { return Vec::new() };
        vec![Effect::Api(ApiCmd::TasteAlbum {
            name,
            artist: None,
            generation: self.session_gen,
            peer: None,
            reach: None,
            play: false,
        })]
    }

    /// A connected server's album on the strip (the 2026-10-11 strip log),
    /// by its index in [`TasteState::peer_albums`]: `[+]` / `a` / `+`
    /// queues it (`play` false), a click or Enter plays it (`play` true) —
    /// the album has no drill of its own to open into here. Its songs are
    /// asked of THAT server through the parent's federation proxy, by name
    /// and artist (a peer's row names its track artist, `album-songs`' own
    /// key); a server that will not list it leaves the rows that matched,
    /// the webapp's `discoveryQueuePeerAlbum`.
    pub(crate) fn taste_peer_album(&mut self, index: usize, play: bool) -> Vec<Effect> {
        let Some(album) = self.taste.peer_albums().into_iter().nth(index) else { return Vec::new() };
        let origin = Origin { server: self.origin().server, peer: Some(album.peer) };
        match self.reach(&origin) {
            Ok(reach) => vec![Effect::Api(ApiCmd::TasteAlbum {
                name: album.album,
                artist: album.artist,
                generation: self.session_gen,
                peer: Some(album.peer),
                reach: Some(reach),
                play,
            })],
            // Nowhere to ask: the rows that matched, at once.
            Err(_) => {
                let items = self.taste_peer_queued(&album.rows);
                self.taste_album_landed(items, play)
            }
        }
    }

    /// A strip album's cover from a connected server, through the parent's
    /// federation art proxy (the queue panel's way for a peer's row): `None`
    /// when the album names no art, the cache already holds or awaits it,
    /// or the server cannot be reached.
    pub(crate) fn taste_peer_album_art(&mut self, album: &TastePeerAlbum) -> Option<Effect> {
        let file = album.art.as_deref()?;
        if self.art.contains_key(file) {
            return None;
        }
        let origin = Origin { server: self.origin().server, peer: Some(album.peer) };
        let reach = self.reach(&origin).ok()?;
        self.fetch_art_from(file, Some(reach))
    }

    /// An album's songs, landed: played (the queue replaced, the first
    /// started) or appended the way queue all appends.
    fn taste_album_landed(&mut self, items: Vec<Queued>, play: bool) -> Vec<Effect> {
        if items.is_empty() {
            self.error(t!("gui.discover.album_failed").to_string());
            return Vec::new();
        }
        if play {
            self.queue.replace(items);
            return self.play_index(0);
        }
        self.taste_append(items)
    }

    /// Connected servers' rows as queue rows, each stamped with the session
    /// server and its own peer.
    fn taste_peer_queued(&self, rows: &[(usize, usize)]) -> Vec<Queued> {
        let server = self.origin().server;
        rows.iter()
            .filter_map(|&(group, row)| {
                let (peer, lead) = self.taste.peer_row(group, row)?;
                let origin = Origin { server: server.clone(), peer: Some(peer) };
                lead.to_track().map(|track| Queued { dj: None, origin, track })
            })
            .collect()
    }

    /// The Library tab's songs as tracks, in the server's order.
    fn taste_songs(&self) -> Vec<Track> {
        self.taste.library.data.iter().flat_map(|l| &l.tracks).map(|t| t.to_track()).collect()
    }

    /// The Connected servers tab's rows as queue rows, each stamped with the
    /// session server and its own peer.
    fn taste_peer_items(&self) -> Vec<Queued> {
        self.taste_peer_queued(&self.taste.peer_lines())
    }

    /// Where row `row` of group `group` sits among the tab's rows.
    fn taste_peer_position(&self, group: usize, row: usize) -> Option<usize> {
        self.taste.peer_lines().iter().position(|at| *at == (group, row))
    }

    /// The tab in front's rows (clause 64).
    fn taste_tab_items(&self) -> Vec<Queued> {
        match self.taste.tab {
            TasteTab::Library => self.queued_all(self.taste_songs()),
            TasteTab::Servers => self.taste_peer_items(),
        }
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
            Event::TasteAlbum { generation, tracks, name, artist, peer, play } => {
                if generation != self.session_gen {
                    return Vec::new();
                }
                let items = match (peer, tracks) {
                    (None, Some(tracks)) => self.queued_all(tracks),
                    (None, None) => Vec::new(),
                    // A connected server's album: its songs with that
                    // server's origin; none listed, the rows that matched
                    // (the webapp's `discoveryQueuePeerAlbum`).
                    (Some(id), Some(tracks)) if !tracks.is_empty() => {
                        let origin = Origin { server: self.origin().server, peer: Some(id) };
                        tracks.into_iter().map(|track| Queued { dj: None, origin: origin.clone(), track }).collect()
                    }
                    (Some(id), _) => {
                        let fold = |s: Option<&str>| s.unwrap_or("").trim().to_lowercase();
                        let key = (id, fold(Some(&name)), fold(artist.as_deref()));
                        let rows = self
                            .taste
                            .peer_albums()
                            .into_iter()
                            .find(|a| (a.peer, fold(Some(&a.album)), fold(a.artist.as_deref())) == key)
                            .map(|a| a.rows)
                            .unwrap_or_default();
                        self.taste_peer_queued(&rows)
                    }
                };
                self.taste_album_landed(items, play)
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
        let library = part == TastePart::Main && t.library.seq == seq;
        let peers = part == TastePart::Peers && t.peers.seq == seq;
        // And whether it is the latest Main, whose head it carries.
        let main = part == TastePart::Main && t.main_seq == seq;
        if !library && !peers && !main {
            return Vec::new();
        }
        if main {
            t.main_pending = false;
        }
        // The peers half waits on this answer (clause 30): only the latest
        // Main it waited for asks it — a stale Main never does.
        let peers_wait = main && t.peers_after == seq;
        if peers_wait {
            t.peers_after = 0;
        }
        let mut response = match answer {
            Ok(response) => response,
            Err(TasteFailure::Failed(err)) => {
                if library {
                    t.library.pending = false;
                    t.library.failed = Some(err.clone());
                }
                // A Main that failed fails the half that waited on it: its
                // Try again asks both (clause 83).
                if peers || peers_wait {
                    t.peers.pending = false;
                    t.peers.failed = Some(err);
                }
                return Vec::new();
            }
            Err(TasteFailure::Forbidden) => {
                let peers = peers || peers_wait;
                for (wanted, slot_pending) in [(library, &mut t.library.pending), (peers, &mut t.peers.pending)] {
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
        // No pick left (clause 21): the passed-over picks excluded every
        // song the server could draw — no seed came back. The list starts
        // over and the room asks once more, the old rows standing, instead
        // of stranding on a scan-pending card; the list is empty then, so
        // this asks once.
        // Pinned or not, as the webapp's `noPickLeft`: a pin the server
        // could not resolve falls back to the random draw too.
        if main
            && response.seeds.is_empty()
            && (random || response.not_analyzed)
            && !t.skipped.is_empty()
        {
            t.skipped.clear();
            t.shown_random.clear();
            return self.taste_ask();
        }
        if library {
            let lists = response.library.take();
            t.library.pending = false;
            t.library.failed = None;
            // A random seed still searches the library (clause 20): the
            // rows stand under the card.
            t.library.absent = lists.is_none() && !random;
            t.library.data = lists;
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
            // The card's random pick, added to the visit's shown picks for
            // "another pick" to pass over (clause 21).
            if response.seed_source == SeedSource::Random {
                for seed in response.seeds.iter().filter(|seed| !seed.pinned) {
                    if !seed.hash.is_empty() && !t.shown_random.contains(&seed.hash) {
                        t.shown_random.push(seed.hash.clone());
                    }
                }
            }
            response.library = None;
            response.network = None;
            response.peers = None;
            t.head = Some(*response);
        }
        if peers_wait && let Some(head) = self.taste.head.take() {
            let effects = self.taste_ask_peers(&head, random);
            self.taste.head = Some(head);
            return effects;
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
    /// nothing will answer must not leave "asking…" standing (clauses 17
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
        if t.peers.pending {
            t.peers.pending = false;
            t.peers.failed = Some(reason());
        }
        t.main_pending = false;
    }
}

/// The last `cap` of `list`, which is oldest first.
fn newest(list: &[String], cap: usize) -> Vec<String> {
    list[list.len().saturating_sub(cap)..].to_vec()
}
