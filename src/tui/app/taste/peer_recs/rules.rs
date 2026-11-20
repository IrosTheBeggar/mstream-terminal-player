//! The Peer recs room's pure rules (peer-recommendations plan §3.5 R1–R8,
//! §5.3 and §5.5–5.7): the taste bodies, how an answer is read and which
//! pane it draws, the peer strip and its Adds, the lead key, the preview
//! helpers and the copy text.
//!
//! Nothing here holds state, asks, waits or draws: the room's App half
//! ([`super`]) keeps the visit and calls these, and the GUI draws what they
//! say. They port the phone's `peer_recs_rules.dart` and the webapp's
//! `PEERRECS` (`peer-recs.js`) and page builders (`discovery-page.js`
//! `buildRequests`, `batchRequest`, `networkState`, `stateOf`), so every
//! client reads the server the same way.
//!
//! Every state is keyed on a peer's endpoint id, never on its label: a twin
//! announced later can take the same label (peer-recs contract §1.5).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::api::types::{
    name_key, NetworkReason, NetworkTasteRequest, P2pPeer, P2pPeersAnswer, PeerAddAnswer,
    PeerAddKind, SeedSource, TasteLead, TasteResponse, ITUNES_PLUGIN, SEEDS_CAP,
};

use super::super::{TASTE_EXCLUDE_CAP, TASTE_PIN_CAP};

/// R6: the strip asks the peers route this often (peer-recommendations
/// plan §3.5 R6).
pub const PEER_POLL: Duration = Duration::from_secs(15);

/// R6: and this often while a tab it draws is being added, or one of this
/// visit's own Add POSTs is still out.
pub const PEER_POLL_FAST: Duration = Duration::from_secs(3);

/// A preview resolve's 429 waits this long before its one retry
/// (peer-recommendations plan §5.5): Deezer's limiter and any provider's
/// but iTunes'.
pub const RETRY_WAIT: Duration = Duration::from_secs(5);

/// iTunes' limiter allows 18 calls a minute, so its 429 waits a minute.
pub const RETRY_WAIT_ITUNES: Duration = Duration::from_secs(60);

// ── The taste bodies (R1, R2) ───────────────────────────────────────────────

/// R1, a new batch (peer-recommendations plan §3.5 R1): the network alone,
/// a full batch of 12 seeds and 32 leads, a walk on a random seed too, the
/// user's own `pins` and the visit's `exclude`, and the tab as
/// `networkPeers` (none for All, and only a well-formed id). Never `moods`
/// (a mood filter leaves the network out) and never `ignoreVPaths` (the
/// terminal's Discover sends none either).
///
/// Empty and repeated entries are dropped; at most the server's
/// [`TASTE_PIN_CAP`] pins go, and the newest [`TASTE_EXCLUDE_CAP`]
/// exclusions.
pub fn r1_body(
    window: u32,
    pins: &[String],
    exclude: &[String],
    tab: Option<&str>,
) -> NetworkTasteRequest {
    let pins = distinct(pins.iter());
    let pins = pins[..pins.len().min(TASTE_PIN_CAP)].to_vec();
    NetworkTasteRequest::new(window, pins, exclusions(exclude), SEEDS_CAP, tab)
}

/// The batch's seeds as pins (peer-recommendations plan §3.5 R2): every
/// seed's path in order, non-empty, de-duplicated, at most
/// [`TASTE_PIN_CAP`]. The random pick is included, unlike Discover's peers
/// half: the room walks the network on it, and a pinned walk over an
/// unchanged shelf gives the same rows.
pub fn batch_pins(batch: &TasteResponse) -> Vec<String> {
    let pins = distinct(batch.seeds.iter().map(|seed| &seed.filepath));
    pins[..pins.len().min(TASTE_PIN_CAP)].to_vec()
}

/// R2, a re-ask on the batch on screen (a tab, Try again, R4, R5): R1's
/// body pinned to every seed of the batch ([`batch_pins`]), with
/// `limit.seeds` their count so the server draws no seed of its own and the
/// seeds modal stays true. The user's own pins are replaced by the batch's.
/// `None` when the batch has no seed with a path: the caller asks R1, a new
/// batch, instead.
pub fn r2_body(
    batch: &TasteResponse,
    window: u32,
    exclude: &[String],
    tab: Option<&str>,
) -> Option<NetworkTasteRequest> {
    let pins = batch_pins(batch);
    if pins.is_empty() {
        return None;
    }
    let seeds = pins.len() as u32;
    Some(NetworkTasteRequest::new(window, pins, exclusions(exclude), seeds, tab))
}

/// The answer's seed is the server's random pick (R3's "another pick"
/// passes it over): the terminal's Discover's own reading.
pub fn is_random(answer: &TasteResponse) -> bool {
    answer.seed_source == SeedSource::Random || answer.fallback
}

/// No pick left (Discover's clause 21; the phone's `peerNoPickLeft`): an
/// unpinned ask (R1, not an R2 re-ask on the batch) that carried `passed`
/// random picks got no seed back on a random or not-analyzed answer, so
/// the picks passed over excluded every song the server could draw. The
/// room clears them and asks once more, the old rows standing.
pub fn no_pick_left(answer: &TasteResponse, pinned: bool, passed: usize) -> bool {
    !pinned
        && passed > 0
        && answer.seeds.is_empty()
        && (is_random(answer) || answer.not_analyzed)
}

/// The entries in order, each once, empty ones dropped.
fn distinct<'a>(list: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in list {
        if !entry.is_empty() && !out.contains(entry) {
            out.push(entry.clone());
        }
    }
    out
}

/// `exclude` as the route takes it: each hash once, the newest
/// [`TASTE_EXCLUDE_CAP`] (a longer list is refused outright).
fn exclusions(exclude: &[String]) -> Vec<String> {
    let all = distinct(exclude.iter());
    all[all.len().saturating_sub(TASTE_EXCLUDE_CAP)..].to_vec()
}

// ── Reading an answer (R5, R7) and the pane ─────────────────────────────────

/// What a taste 200 means for the visit (peer-recommendations plan §3.5;
/// the webapp's `peerRecsAnswer`, the phone's `_answer`). A 403 is not an
/// answer: the room reads it as R7 itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerRead {
    /// R7: a history (not random) answer with no network section can only
    /// mean p2p went off, since the room always sends `networkOnRandomSeed`
    /// and never `moods`. One re-ping decides (build plan D18).
    Off,
    /// R5: the selected peer left the shelf. Select All; the answer is kept
    /// for no tab.
    NotOnShelf,
    /// Draw it and keep it for its tab (the first of a batch is the batch).
    Keep,
    /// Draw it, but keep it for no tab, so a tab chosen again asks again:
    /// the walk failed (`reason: error`), or a random answer came back with
    /// no network section (its pane is Try again).
    ShowNotKeep,
}

/// A taste 200, read in the contract's order (R7, then R5, then the rest).
/// `tab_selected` is whether a peer tab (not All) was asked.
pub fn read_answer(answer: &TasteResponse, tab_selected: bool) -> AnswerRead {
    let reason = answer.network.as_ref().and_then(|network| network.reason);
    if answer.network.is_none() && !is_random(answer) {
        return AnswerRead::Off;
    }
    if tab_selected && reason == Some(NetworkReason::NotOnShelf) {
        return AnswerRead::NotOnShelf;
    }
    if answer.network.is_none() || reason == Some(NetworkReason::Error) {
        return AnswerRead::ShowNotKeep;
    }
    AnswerRead::Keep
}

/// The list rect's state (peer-recs contract §1.10). Off is not a pane: the
/// room yields to Files instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecsPane {
    /// No answer yet and an ask is out: the `░` rows.
    Loading,
    /// Leads: the verbs line and the grid.
    Ready,
    /// The shelf has nothing to walk: `No peers yet`.
    NoPeers,
    /// The walk found nothing new: `Nothing new` and `↻ new seeds`.
    NothingNew,
    /// No answer, or the walk failed: `Try again`.
    TryAgain,
    /// The server's scan has not run: the scan-pending words and
    /// `Check again`.
    ScanPending,
}

/// The pane for the answer on screen (the phone's `peerRecsStateOf`, the
/// webapp's `stateOf` and `networkState`), first match wins:
/// - `scan_latch` (a taste 403 whose re-ping said the flags are still on) →
///   scan pending, as the terminal's Discover reads it (build plan D17);
/// - no answer (or the newest ask `failed`: a transport failure or a 5xx,
///   so the answer it stood on is no longer drawn) → loading while an ask
///   is out, else Try again;
/// - not analyzed, or `reason: scan-pending` → scan pending;
/// - no network section (a random answer with p2p off) → Try again;
/// - leads → ready: rows win over any reason;
/// - `reason: error` → Try again;
/// - `empty-shelf`, `no-compatible`, or a shelf the last poll found with
///   nothing `searchable` → No peers yet;
/// - anything else (`nothing-new`, no reason, a word a newer server added)
///   → Nothing new.
pub fn pane_of(
    answer: Option<&TasteResponse>,
    asking: bool,
    failed: bool,
    scan_latch: bool,
    searchable: Option<u32>,
) -> RecsPane {
    if scan_latch {
        return RecsPane::ScanPending;
    }
    let Some(answer) = answer.filter(|_| !failed) else {
        return if asking { RecsPane::Loading } else { RecsPane::TryAgain };
    };
    let network = answer.network.as_ref();
    let reason = network.and_then(|network| network.reason);
    if answer.not_analyzed || reason == Some(NetworkReason::ScanPending) {
        return RecsPane::ScanPending;
    }
    let Some(network) = network else { return RecsPane::TryAgain };
    if !network.results.is_empty() {
        return RecsPane::Ready;
    }
    match reason {
        Some(NetworkReason::Error) => RecsPane::TryAgain,
        Some(NetworkReason::EmptyShelf | NetworkReason::NoCompatible) => RecsPane::NoPeers,
        _ if searchable == Some(0) => RecsPane::NoPeers,
        _ => RecsPane::NothingNew,
    }
}

// ── The peer strip (R6) ─────────────────────────────────────────────────────

/// One tab's state (peer-recs contract §1.5; peer-recommendations plan
/// §5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripKind {
    /// The client's own first tab.
    All,
    /// A peer on the shelf: selectable, `•` while online.
    Shelf { online: bool },
    /// A peer an admin may Add: `+ {label}`.
    Addable,
    /// Its Add is on its way (the server's `adding` or `done`, or this
    /// visit's press): inert.
    Adding,
    /// Its Add failed for want of room: inert, never POSTed again.
    NoRoom,
    /// Its Add failed for any other reason: Enter POSTs it again.
    TryAgain,
}

/// What Enter or a click does on a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripAct {
    /// Select it: All or a shelf tab (R2).
    Select,
    /// POST its Add: a `+` or a try again tab.
    Add,
    /// Nothing: adding or no room.
    Nothing,
}

/// One tab of the strip. `id` is the peer's endpoint id, `None` on All;
/// `label` is the server's, verbatim, and empty on All, whose word is the
/// GUI's (`gui.peer_recs.all`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripItem {
    pub id: Option<String>,
    pub label: String,
    pub kind: StripKind,
}

impl StripItem {
    /// It can be the selected tab: All and the shelf tabs. A `+`, adding,
    /// no room or try again tab never is (peer-recommendations plan §5.6).
    pub fn selectable(&self) -> bool {
        matches!(self.kind, StripKind::All | StripKind::Shelf { .. })
    }

    /// Enter and a click do nothing on it, and the tabs stop skips it.
    pub fn inert(&self) -> bool {
        matches!(self.kind, StripKind::Adding | StripKind::NoRoom)
    }

    /// What Enter or a click does on it.
    pub fn acts(&self) -> StripAct {
        match self.kind {
            StripKind::All | StripKind::Shelf { .. } => StripAct::Select,
            StripKind::Addable | StripKind::TryAgain => StripAct::Add,
            StripKind::Adding | StripKind::NoRoom => StripAct::Nothing,
        }
    }
}

/// A local Add state: what the strip draws for a peer before the server
/// says so (the webapp's `LOCAL_STATES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAddKind {
    /// From the press until a later poll answers.
    Adding,
    /// The POST got no usable answer: try again.
    Failed,
    /// The POST answered failed `no-room`.
    NoRoom,
}

/// A local Add state and its `stamp`: how many polls the visit had sent
/// when it was set, at the press and again when the POST answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalAdd {
    pub kind: LocalAddKind,
    pub stamp: u64,
}

/// A local state stands until a poll sent after it was stamped answers
/// (the webapp's `localStands`): `answered_poll` is the number of the poll
/// whose answer the strip is drawn from, counted as the polls were sent.
pub fn local_stands(entry: &LocalAdd, answered_poll: u64) -> bool {
    answered_poll <= entry.stamp
}

/// The strip as it draws (the webapp's `stripItems`, the phone's
/// `peerStripItems`): All, then the shelf rows in the server's order
/// (online first, then label), then, only when the answer says `canAdd`,
/// the other rows in the server's order. An addable row shows its standing
/// local state ([`local_stands`]), else the server's Add state. A shelf
/// row's Add state is ignored: the server stamps it there too.
///
/// Before the first answer (`view` `None`) it is All alone.
pub fn strip_items(
    view: Option<&P2pPeersAnswer>,
    local: &BTreeMap<String, LocalAdd>,
    answered_poll: u64,
) -> Vec<StripItem> {
    let mut items = vec![StripItem { id: None, label: String::new(), kind: StripKind::All }];
    let Some(view) = view else { return items };
    let tab = |row: &P2pPeer, kind| StripItem {
        id: Some(row.endpoint_id.clone()),
        label: row.label.clone(),
        kind,
    };
    for row in view.peers.iter().filter(|row| row.on_shelf) {
        items.push(tab(row, StripKind::Shelf { online: row.online }));
    }
    if view.can_add {
        for row in view.peers.iter().filter(|row| !row.on_shelf) {
            let mine = local.get(&row.endpoint_id).filter(|e| local_stands(e, answered_poll));
            let kind = match mine {
                Some(entry) => match entry.kind {
                    LocalAddKind::Adding => StripKind::Adding,
                    LocalAddKind::Failed => StripKind::TryAgain,
                    LocalAddKind::NoRoom => StripKind::NoRoom,
                },
                None => server_add(row),
            };
            items.push(tab(row, kind));
        }
    }
    items
}

/// The local states to keep after a poll's answer (the phone's
/// `peerStripItems` `local`): those that still stand, for rows still
/// addable. A row on the shelf, gone, or under `canAdd` false drops its
/// own.
pub fn standing_adds(
    view: &P2pPeersAnswer,
    local: &BTreeMap<String, LocalAdd>,
    answered_poll: u64,
) -> BTreeMap<String, LocalAdd> {
    local
        .iter()
        .filter(|(id, entry)| {
            view.can_add
                && view.row(id).is_some_and(|row| !row.on_shelf)
                && local_stands(entry, answered_poll)
        })
        .map(|(id, entry)| (id.clone(), *entry))
        .collect()
}

/// An addable row's state from the server's (the webapp's
/// `serverAddState`): `done` still reads adding, since it lands as a shelf
/// row on the next poll.
fn server_add(row: &P2pPeer) -> StripKind {
    match &row.add {
        None => StripKind::Addable,
        Some(add) => match add.state {
            PeerAddKind::Adding | PeerAddKind::Done => StripKind::Adding,
            PeerAddKind::Failed if add.no_room() => StripKind::NoRoom,
            PeerAddKind::Failed => StripKind::TryAgain,
        },
    }
}

/// The strip is drawn: All alone (a member with an empty shelf, or the
/// peers route's 404) carries nothing, so its row stays blank (peer-recs
/// contract §1.5).
pub fn strip_drawn(items: &[StripItem]) -> bool {
    items.len() > 1
}

/// R6's wait before the next poll (the webapp's `pollDelay`): fast while
/// one of this visit's Add POSTs is out (`posting`) or a tab reads adding;
/// try again and no room do not hurry it.
pub fn poll_delay(items: &[StripItem], posting: bool) -> Duration {
    if posting || items.iter().any(|item| item.kind == StripKind::Adding) {
        PEER_POLL_FAST
    } else {
        PEER_POLL
    }
}

/// Stowed `←` `→` (build plan D22; R8): the tab `dir` steps from the
/// selected one through All and the shelf tabs, skipping the add tabs,
/// which are never selected. `Some(None)` is All; `None` is no step (the
/// strip's end, or `dir` 0). A selection no longer in the strip steps from
/// All.
pub fn step_tab(items: &[StripItem], selected: Option<&str>, dir: i32) -> Option<Option<String>> {
    let tabs: Vec<&StripItem> = items.iter().filter(|item| item.selectable()).collect();
    let at = tabs.iter().position(|item| item.id.as_deref() == selected).unwrap_or(0);
    let to = match dir.signum() {
        1 => at + 1,
        -1 => at.checked_sub(1)?,
        _ => return None,
    };
    tabs.get(to).map(|item| item.id.clone())
}

// ── Adds (R4) ───────────────────────────────────────────────────────────────

/// What an Add's answer means for the strip (peer-recommendations plan
/// §5.6; the webapp's `addOutcome`). Only the status and the stored state
/// are read; the server's words are never drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// 202, or 200 `adding`.
    Adding,
    /// 200 `done`: on the shelf already; the poll it asks for shows it.
    Landed,
    /// 200 `failed` `no-room`.
    NoRoom,
    /// 200 `failed` with any other code, or anything unlisted (400, 401,
    /// a 5xx, no answer, a 200 with no state).
    Failed,
    /// 404 or 409: the peer left the catalog, or is blocked or
    /// incompatible; its row goes.
    Gone,
    /// 403 or 405: not an admin here any more, or p2p off; `canAdd` comes
    /// back false.
    NotAllowed,
}

impl AddOutcome {
    /// The local state the tab draws until a later poll answers; `None`
    /// drops it.
    pub fn local_kind(self) -> Option<LocalAddKind> {
        match self {
            AddOutcome::Adding | AddOutcome::Landed => Some(LocalAddKind::Adding),
            AddOutcome::NoRoom => Some(LocalAddKind::NoRoom),
            AddOutcome::Failed => Some(LocalAddKind::Failed),
            AddOutcome::Gone | AddOutcome::NotAllowed => None,
        }
    }

    /// Poll at once rather than on the cadence.
    pub fn polls_now(self) -> bool {
        matches!(self, AddOutcome::Landed | AddOutcome::Gone | AddOutcome::NotAllowed)
    }

    /// The id stays among this visit's posted ones (R4): only an Add still
    /// on its way can land.
    pub fn keeps_posted(self) -> bool {
        matches!(self, AddOutcome::Adding | AddOutcome::Landed)
    }
}

/// An Add's answer read (status 0 is no answer at all).
pub fn add_outcome(answer: &PeerAddAnswer) -> AddOutcome {
    match (answer.status, &answer.state) {
        (202, _) => AddOutcome::Adding,
        (200, Some(state)) => match state.state {
            PeerAddKind::Done => AddOutcome::Landed,
            PeerAddKind::Adding => AddOutcome::Adding,
            PeerAddKind::Failed if state.no_room() => AddOutcome::NoRoom,
            PeerAddKind::Failed => AddOutcome::Failed,
        },
        (404 | 409, _) => AddOutcome::Gone,
        (403 | 405, _) => AddOutcome::NotAllowed,
        _ => AddOutcome::Failed,
    }
}

/// A row holds its peer: on the shelf, or its Add done.
fn held(row: Option<&P2pPeer>) -> bool {
    row.is_some_and(|row| row.on_shelf || add_is(row, PeerAddKind::Done))
}

fn add_is(row: &P2pPeer, kind: PeerAddKind) -> bool {
    row.add.as_ref().is_some_and(|add| add.state == kind)
}

/// R4 between the answer the strip was drawn from (`prev`, `None` before
/// the first) and the new one (the webapp's `landed`, the phone's
/// `peerLanded`): whether an id this visit POSTed (`posted`) is now held,
/// on the shelf or its Add done, where it was not before; and the posted
/// ids to stop tracking, those that landed and those whose Add has just
/// failed. An Add by another client, or a rotation, only moves the strip.
pub fn landed(
    prev: Option<&P2pPeersAnswer>,
    next: &P2pPeersAnswer,
    posted: &BTreeSet<String>,
) -> (bool, Vec<String>) {
    let failed = |row: Option<&P2pPeer>| row.is_some_and(|row| add_is(row, PeerAddKind::Failed));
    let mut landed = false;
    let mut done = Vec::new();
    for id in posted {
        let was = prev.and_then(|prev| prev.row(id));
        let now = next.row(id);
        if held(now) && !held(was) {
            landed = true;
            done.push(id.clone());
        } else if failed(now) && !failed(was) {
            done.push(id.clone());
        }
    }
    (landed, done)
}

/// R4's other half: the room shows No peers yet and the shelf's
/// `searchable` count went from 0 (or no answer) to more.
pub fn grew_under_no_peers(
    prev: Option<&P2pPeersAnswer>,
    next: &P2pPeersAnswer,
    showing_no_peers: bool,
) -> bool {
    let before = prev.map_or(0, |prev| prev.shelf.searchable);
    showing_no_peers && before == 0 && next.shelf.searchable > 0
}

/// The shelf emptied under the room (peer-recommendations plan §3.5 R4):
/// `searchable` went from more to 0 between two answers, so every kept
/// answer was walked over peers no longer there. Nothing before the first.
pub fn emptied(prev: Option<&P2pPeersAnswer>, next: &P2pPeersAnswer) -> bool {
    prev.is_some_and(|prev| prev.shelf.searchable > 0) && next.shelf.searchable == 0
}

/// R5 from a poll: the selected peer is no longer a shelf row. An offline
/// shelf row still is one, and keeps its tab.
pub fn selected_gone(next: &P2pPeersAnswer, selected: &str) -> bool {
    !next.peers.iter().any(|row| row.on_shelf && row.endpoint_id == selected)
}

// ── Leads, previews and copy ────────────────────────────────────────────────

/// One lead across answers and tabs (peer-recommendations plan §5.3): the
/// live clip, the visit's preview answers, the sheet it is open in. The
/// peer's endpoint id and the snapshot's export id; without one, the
/// folded artist, title and album ([`name_key`], the server's fold).
/// Each part is read through [`peer_text`]: the room keys every drawn lead
/// on every frame, so a peer's outsized string costs no more than a real one.
pub fn lead_key(lead: &TasteLead) -> String {
    let id = lead.peer.endpoint_id.as_deref().map(peer_text).unwrap_or_default();
    match lead.export_id.as_deref().map(peer_text).filter(|export| !export.is_empty()) {
        Some(export) => format!("{id}\0{export}"),
        None => format!(
            "{id}\0{}|{}|{}",
            name_key(lead.artist.as_deref().map(peer_text)),
            name_key(lead.title.as_deref().map(peer_text)),
            name_key(lead.album.as_deref().map(peer_text)),
        ),
    }
}

/// How much of a peer's string the room reads, in bytes: the server's own
/// cap of 512 characters at 4 bytes each, so every string it lets through
/// whole is read whole.
pub const PEER_TEXT_MOST: usize = 2048;

/// A peer's string as far as [`PEER_TEXT_MOST`] runs, cut on a character
/// boundary. The server bounds a lead's words only after trimming and
/// forwards them untrimmed, so a padded one may arrive any length; the
/// per-frame paths (keys, the grid's words) read it through this. A lead
/// the room keeps is already short ([`bound_answer`]); this is the guard
/// for one that is not.
pub fn peer_text(text: &str) -> &str {
    if text.len() <= PEER_TEXT_MOST {
        return text;
    }
    let mut end = PEER_TEXT_MOST;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A taste answer's leads as the room keeps them, once, as the answer
/// lands: each peer word the room reads (title, artist, album, album
/// artist, export id, the peer's id, name and label) trimmed, as the
/// server trims before its caps, then cut by [`peer_text`]; blank is
/// absent. The grid, the band, the sheet and every key then read short
/// words on every frame. `raw` is untouched: a lead goes back to the
/// server as the taste route sent it.
pub fn bound_answer(answer: &mut TasteResponse) {
    fn bound(word: &mut Option<String>) {
        let Some(text) = word.as_deref() else { return };
        let kept = peer_text(text.trim_start()).trim_end();
        if kept.is_empty() {
            *word = None;
        } else if kept.len() != text.len() {
            *word = Some(kept.to_string());
        }
    }
    let Some(network) = answer.network.as_mut() else { return };
    for lead in &mut network.results {
        for word in [&mut lead.title, &mut lead.artist, &mut lead.album, &mut lead.album_artist] {
            bound(word);
        }
        bound(&mut lead.export_id);
        let peer = &mut lead.peer;
        for word in [&mut peer.endpoint_id, &mut peer.name, &mut peer.label] {
            bound(word);
        }
    }
}

/// How long a resolve's 429 waits before its one retry, by the preview
/// plug-in's name (peer-recommendations plan §5.5).
pub fn retry_wait(provider: &str) -> Duration {
    if provider == ITUNES_PLUGIN {
        RETRY_WAIT_ITUNES
    } else {
        RETRY_WAIT
    }
}

/// Preview all's next lead (the webapp's `nextIndex`): the first index
/// after `from` (`None` is from the top) whose lead is not `known_none`
/// (no preview for the visit); `None` at the list's end.
pub fn next_index(
    len: usize,
    from: Option<usize>,
    known_none: impl Fn(usize) -> bool,
) -> Option<usize> {
    let start = from.map_or(0, |at| at + 1);
    (start..len).find(|&at| !known_none(at))
}

/// Copy (peer-recommendations plan §5.7): `Artist - Title`, both trimmed;
/// the title alone when the artist is blank, the artist alone when the
/// title is; `None` when both are. Each part is a peer's words on their way
/// to the clipboard and likely a shell, so it is put on one line first
/// ([`one_line`]).
pub fn copy_text(lead: &TasteLead) -> Option<String> {
    let artist = one_line(lead.artist.as_deref().unwrap_or_default());
    let title = one_line(lead.title.as_deref().unwrap_or_default());
    match (artist.is_empty(), title.is_empty()) {
        (true, true) => None,
        (true, false) => Some(title),
        (false, true) => Some(artist),
        (false, false) => Some(format!("{artist} - {title}")),
    }
}

/// `text` on one line: every run of whitespace (line breaks and tabs
/// among it) is one space, other control characters and the bidi steering
/// marks (the set `discovery.rs`'s `steers` names) are dropped, and the
/// ends are trimmed.
fn one_line(text: &str) -> String {
    let steers = |c: char| {
        c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
    };
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() {
            gap = true;
        } else if !steers(c) {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out
}

/// Copy list: the tab's leads in rank order, one line each, blank ones
/// dropped, joined by `\n` with no trailing newline; `None` when nothing
/// is left to copy.
pub fn copy_list(leads: &[TasteLead]) -> Option<String> {
    let lines: Vec<String> = leads.iter().filter_map(copy_text).collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::PeerAddState;
    use serde_json::{json, Value};

    /// Endpoint ids as the wire carries them: 64 lower-case hex.
    fn id(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    fn basement() -> String {
        id('b')
    }
    fn kitchen() -> String {
        id('c')
    }
    fn f3a() -> String {
        id('d')
    }
    fn attic() -> String {
        id('e')
    }
    fn lighthouse() -> String {
        id('1')
    }
    fn garden() -> String {
        id('2')
    }
    fn rooftop() -> String {
        id('3')
    }

    fn answer(v: Value) -> TasteResponse {
        serde_json::from_value(v).unwrap()
    }

    /// A history answer whose walk found no rows, for `reason`.
    fn empty_walk(reason: Value) -> TasteResponse {
        answer(json!({ "seedSource": "history", "network": { "results": [], "reason": reason } }))
    }

    fn body(req: &NetworkTasteRequest) -> Value {
        serde_json::to_value(req).unwrap()
    }

    /// One peers-route row; `add` is its Add state, or null.
    fn row(id: &str, label: &str, on_shelf: bool, online: bool, add: Value) -> Value {
        json!({
            "endpointId": id, "name": label, "label": label, "monogram": "", "hue": 0,
            "online": online, "onShelf": on_shelf, "pinned": on_shelf, "rowCount": 10, "add": add,
        })
    }

    fn adding() -> Value {
        json!({ "state": "adding", "code": null })
    }

    fn failed(code: &str) -> Value {
        json!({ "state": "failed", "code": code })
    }

    fn peers(can_add: bool, searchable: u32, rows: Vec<Value>) -> P2pPeersAnswer {
        serde_json::from_value(json!({
            "canAdd": can_add,
            "shelf": { "held": searchable, "searchable": searchable, "running": true },
            "peers": rows,
        }))
        .unwrap()
    }

    /// The stage's strip as an admin reads it (build plan DS §2): four
    /// shelf peers, then the catalog in each Add state.
    fn stage_admin() -> P2pPeersAnswer {
        peers(
            true,
            3,
            vec![
                row(&basement(), "Basement Hi-Fi", true, true, Value::Null),
                row(&kitchen(), "Kitchen Radio", true, true, Value::Null),
                row(&f3a(), "mStream · 3f9a", true, false, Value::Null),
                row(&attic(), "Attic Tapes", true, false, Value::Null),
                row(&lighthouse(), "Lighthouse Tapes", false, true, Value::Null),
                row(&garden(), "Static Garden", false, true, adding()),
                row(&rooftop(), "Rooftop FM", false, true, failed("offline")),
            ],
        )
    }

    fn seeds(paths: &[&str]) -> Value {
        Value::Array(
            paths
                .iter()
                .enumerate()
                .map(|(at, path)| json!({ "hash": format!("h{at}"), "filepath": path }))
                .collect(),
        )
    }

    fn history_batch(paths: &[&str]) -> TasteResponse {
        answer(json!({
            "seedSource": "history", "seeds": seeds(paths), "network": { "results": [] },
        }))
    }

    fn lead(v: Value) -> TasteLead {
        TasteLead::from_raw(v)
    }

    fn kinds(items: &[StripItem]) -> Vec<StripKind> {
        items.iter().map(|item| item.kind).collect()
    }

    fn none_stand() -> BTreeMap<String, LocalAdd> {
        BTreeMap::new()
    }

    fn add_state(state: PeerAddKind, code: Option<&str>) -> PeerAddState {
        PeerAddState { state, code: code.map(str::to_string), at: None }
    }

    fn add_answer(status: u16, state: Option<(PeerAddKind, Option<&str>)>) -> PeerAddAnswer {
        PeerAddAnswer { status, state: state.map(|(state, code)| add_state(state, code)) }
    }

    // ── The bodies ──────────────────────────────────────────────────────────

    #[test]
    fn r1_at_open_is_the_golden_body() {
        assert_eq!(
            body(&r1_body(30, &[], &[], None)),
            json!({
                "window": 30, "sources": ["network"], "newArtistsOnly": false,
                "limit": { "seeds": 12, "network": 32 }, "networkOnRandomSeed": true,
            })
        );
    }

    #[test]
    fn r1_carries_pins_excludes_and_the_tab() {
        let pins = ["music/pin.mp3".to_string(), String::new(), "music/pin.mp3".to_string()];
        let exclude = ["h1".to_string(), "h2".to_string(), "h1".to_string()];
        // The tab arrives upper-cased: the route takes lower-case hex only.
        let tab = kitchen().to_uppercase();
        assert_eq!(
            body(&r1_body(90, &pins, &exclude, Some(&tab))),
            json!({
                "window": 90, "pin": ["music/pin.mp3"], "exclude": ["h1", "h2"],
                "sources": ["network"], "newArtistsOnly": false,
                "limit": { "seeds": 12, "network": 32 }, "networkOnRandomSeed": true,
                "networkPeers": [kitchen()],
            })
        );
        // A tab that is not an endpoint id is never sent: `[]` is a 400.
        let bad = body(&r1_body(30, &[], &[], Some("kitchen")));
        assert!(bad.get("networkPeers").is_none(), "{bad}");
        // The newest exclusions go when there are more than the cap.
        let many: Vec<String> = (0..TASTE_EXCLUDE_CAP + 5).map(|at| format!("h{at}")).collect();
        let sent = body(&r1_body(30, &[], &many, None));
        let sent = sent["exclude"].as_array().unwrap();
        assert_eq!(sent.len(), TASTE_EXCLUDE_CAP);
        assert_eq!(sent[0], json!("h5"));
    }

    #[test]
    fn r2_pins_every_batch_seed_including_the_random_pick() {
        let batch = history_batch(&["a.mp3", "b.mp3"]);
        assert_eq!(
            body(&r2_body(&batch, 30, &[], Some(&kitchen())).unwrap()),
            json!({
                "window": 30, "pin": ["a.mp3", "b.mp3"], "sources": ["network"],
                "newArtistsOnly": false, "limit": { "seeds": 2, "network": 32 },
                "networkOnRandomSeed": true, "networkPeers": [kitchen()],
            })
        );
        // The room walks the network on a random pick, so it is pinned too.
        let random = answer(json!({
            "seedSource": "random", "fallback": true,
            "seeds": [{ "hash": "pick", "filepath": "music/pick.mp3" }],
            "network": { "results": [] },
        }));
        let sent = body(&r2_body(&random, 7, &["h1".to_string()], None).unwrap());
        assert_eq!(sent["pin"], json!(["music/pick.mp3"]));
        assert_eq!(sent["limit"], json!({ "seeds": 1, "network": 32 }));
        assert_eq!(sent["exclude"], json!(["h1"]));
        assert_eq!(sent["window"], json!(7));
    }

    #[test]
    fn r2_for_all_has_no_network_peers() {
        let batch = history_batch(&["a.mp3"]);
        let sent = body(&r2_body(&batch, 30, &[], None).unwrap());
        assert!(sent.get("networkPeers").is_none(), "{sent}");
        assert_eq!(sent["pin"], json!(["a.mp3"]));
    }

    #[test]
    fn r2_without_a_pinnable_batch_is_none() {
        assert_eq!(r2_body(&history_batch(&[]), 30, &[], Some(&kitchen())), None);
        // Seeds with no path pin nothing either: the caller asks R1.
        assert_eq!(r2_body(&history_batch(&["", ""]), 30, &[], None), None);
    }

    #[test]
    fn batch_pins_are_deduped_and_capped_at_12() {
        let mut paths: Vec<String> = (0..14).map(|at| format!("music/{at}.mp3")).collect();
        paths.insert(1, "music/0.mp3".into());
        paths.insert(3, String::new());
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let pins = batch_pins(&history_batch(&refs));
        let want: Vec<String> = (0..12).map(|at| format!("music/{at}.mp3")).collect();
        assert_eq!(pins, want);
        assert_eq!(TASTE_PIN_CAP, 12);
    }

    // ── Reading answers and the panes ───────────────────────────────────────

    #[test]
    fn read_answer_orders_off_not_on_shelf_keep_and_error() {
        // R7: a history answer with no network section, tab or not.
        let off = answer(json!({
            "seedSource": "history", "seeds": seeds(&["a.mp3"]), "network": null,
        }));
        assert_eq!(read_answer(&off, false), AnswerRead::Off);
        assert_eq!(read_answer(&off, true), AnswerRead::Off);
        // R5 only with a peer tab asked.
        let gone = empty_walk(json!("not-on-shelf"));
        assert_eq!(read_answer(&gone, true), AnswerRead::NotOnShelf);
        assert_eq!(read_answer(&gone, false), AnswerRead::Keep);
        // A failed walk is shown, never kept.
        let error = empty_walk(json!("error"));
        assert_eq!(read_answer(&error, true), AnswerRead::ShowNotKeep);
        assert_eq!(read_answer(&error, false), AnswerRead::ShowNotKeep);
        // Rows, and every other empty reason, are kept.
        let rows = answer(json!({
            "seedSource": "history",
            "network": { "results": [{ "title": "Salt Road", "artist": "Hollow Coast" }] },
        }));
        assert_eq!(read_answer(&rows, true), AnswerRead::Keep);
        for reason in ["nothing-new", "empty-shelf", "no-compatible", "scan-pending", "someday"] {
            assert_eq!(read_answer(&empty_walk(json!(reason)), true), AnswerRead::Keep, "{reason}");
        }
    }

    #[test]
    fn network_null_on_a_random_answer_is_try_again_not_off() {
        let random = answer(json!({
            "seedSource": "random", "seeds": seeds(&["pick.mp3"]), "network": null,
        }));
        assert_eq!(read_answer(&random, false), AnswerRead::ShowNotKeep);
        assert_eq!(pane_of(Some(&random), false, false, false, Some(3)), RecsPane::TryAgain);
        // The server's fallback flag reads as random too.
        let fallback = answer(json!({ "fallback": true, "network": null }));
        assert!(is_random(&fallback));
        assert_eq!(read_answer(&fallback, true), AnswerRead::ShowNotKeep);
    }

    #[test]
    fn pane_of_follows_the_phone_order_and_rows_win() {
        let rows = answer(json!({
            "seedSource": "history",
            "network": { "results": [{ "title": "Salt Road" }], "reason": "nothing-new" },
        }));
        // A 403 whose re-ping said on wins over everything.
        assert_eq!(pane_of(Some(&rows), false, false, true, Some(3)), RecsPane::ScanPending);
        assert_eq!(pane_of(None, true, false, true, None), RecsPane::ScanPending);
        // No answer: loading while an ask is out, else Try again.
        assert_eq!(pane_of(None, true, false, false, None), RecsPane::Loading);
        assert_eq!(pane_of(None, false, false, false, None), RecsPane::TryAgain);
        assert_eq!(pane_of(None, false, true, false, None), RecsPane::TryAgain);
        // A failed ask is no answer, even with rows that stood before it.
        assert_eq!(pane_of(Some(&rows), false, true, false, Some(3)), RecsPane::TryAgain);
        assert_eq!(pane_of(Some(&rows), true, true, false, Some(3)), RecsPane::Loading);
        // A re-ask keeps the old rows standing.
        assert_eq!(pane_of(Some(&rows), true, false, false, Some(3)), RecsPane::Ready);

        // From here on: an answer on screen, nothing out, no latch.
        let pane =
            |answer: &TasteResponse, searchable| pane_of(Some(answer), false, false, false, searchable);
        // Scan pending, by the flag or the reason.
        let unscanned = answer(json!({ "notAnalyzed": true, "network": { "results": [] } }));
        assert_eq!(pane(&unscanned, Some(3)), RecsPane::ScanPending);
        assert_eq!(pane(&empty_walk(json!("scan-pending")), Some(0)), RecsPane::ScanPending);
        // Rows win over any reason, a zero shelf included.
        assert_eq!(pane(&rows, Some(0)), RecsPane::Ready);
        // The empty states.
        assert_eq!(pane(&empty_walk(json!("error")), Some(3)), RecsPane::TryAgain);
        assert_eq!(pane(&empty_walk(json!("empty-shelf")), Some(3)), RecsPane::NoPeers);
        assert_eq!(pane(&empty_walk(json!("no-compatible")), None), RecsPane::NoPeers);
        assert_eq!(pane(&empty_walk(json!("nothing-new")), Some(0)), RecsPane::NoPeers);
        assert_eq!(pane(&empty_walk(Value::Null), Some(0)), RecsPane::NoPeers);
        assert_eq!(pane(&empty_walk(json!("nothing-new")), Some(3)), RecsPane::NothingNew);
        assert_eq!(pane(&empty_walk(Value::Null), None), RecsPane::NothingNew);
        assert_eq!(pane(&empty_walk(json!("someday")), Some(3)), RecsPane::NothingNew);
        assert_eq!(pane(&empty_walk(json!("not-on-shelf")), Some(3)), RecsPane::NothingNew);
    }

    // ── The strip and Adds ──────────────────────────────────────────────────

    #[test]
    fn strip_items_are_all_then_shelf_then_addable_for_admins_only() {
        let admin = stage_admin();
        let items = strip_items(Some(&admin), &none_stand(), 1);
        assert_eq!(
            kinds(&items),
            [
                StripKind::All,
                StripKind::Shelf { online: true },
                StripKind::Shelf { online: true },
                StripKind::Shelf { online: false },
                StripKind::Shelf { online: false },
                StripKind::Addable,
                StripKind::Adding,
                StripKind::TryAgain,
            ]
        );
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "",
                "Basement Hi-Fi",
                "Kitchen Radio",
                "mStream · 3f9a",
                "Attic Tapes",
                "Lighthouse Tapes",
                "Static Garden",
                "Rooftop FM",
            ]
        );
        assert_eq!(items[0].id, None);
        assert_eq!(items[2].id.as_deref(), Some(kitchen().as_str()));
        // The server sends a member no catalog rows; even if one did,
        // `canAdd` false draws the shelf alone.
        let mut member = admin.clone();
        member.can_add = false;
        let items = strip_items(Some(&member), &none_stand(), 1);
        assert_eq!(items.len(), 5);
        assert!(items.iter().all(StripItem::selectable));
        // A done Add not yet on the shelf reads adding; no room is its own.
        let stamped = peers(
            true,
            3,
            vec![
                row(&lighthouse(), "Lighthouse Tapes", false, true, json!({ "state": "done" })),
                row(&garden(), "Static Garden", false, true, failed("no-room")),
                row(&rooftop(), "Rooftop FM", false, true, json!({ "state": "failed" })),
            ],
        );
        assert_eq!(
            kinds(&strip_items(Some(&stamped), &none_stand(), 1)),
            [StripKind::All, StripKind::Adding, StripKind::NoRoom, StripKind::TryAgain]
        );
    }

    #[test]
    fn a_tab_tells_whether_it_selects_adds_or_does_nothing() {
        let tab = |kind| StripItem { id: None, label: String::new(), kind };
        for (kind, selectable, inert, acts) in [
            (StripKind::All, true, false, StripAct::Select),
            (StripKind::Shelf { online: true }, true, false, StripAct::Select),
            (StripKind::Shelf { online: false }, true, false, StripAct::Select),
            (StripKind::Addable, false, false, StripAct::Add),
            (StripKind::TryAgain, false, false, StripAct::Add),
            (StripKind::Adding, false, true, StripAct::Nothing),
            (StripKind::NoRoom, false, true, StripAct::Nothing),
        ] {
            let item = tab(kind);
            assert_eq!(item.selectable(), selectable, "{kind:?}");
            assert_eq!(item.inert(), inert, "{kind:?}");
            assert_eq!(item.acts(), acts, "{kind:?}");
        }
    }

    #[test]
    fn shelf_rows_ignore_their_add_stamp() {
        let answer = peers(
            true,
            2,
            vec![
                row(&kitchen(), "Kitchen Radio", true, true, failed("no-room")),
                row(&lighthouse(), "Lighthouse Tapes", true, false, adding()),
            ],
        );
        let mut local = BTreeMap::new();
        local.insert(lighthouse(), LocalAdd { kind: LocalAddKind::Adding, stamp: 5 });
        assert_eq!(
            kinds(&strip_items(Some(&answer), &local, 5)),
            [StripKind::All, StripKind::Shelf { online: true }, StripKind::Shelf { online: false }]
        );
    }

    #[test]
    fn a_local_add_state_stands_until_a_poll_sent_after_it_answers() {
        let entry = LocalAdd { kind: LocalAddKind::Adding, stamp: 3 };
        assert!(local_stands(&entry, 2));
        assert!(local_stands(&entry, 3));
        assert!(!local_stands(&entry, 4));
        let admin = stage_admin();
        let mut local = BTreeMap::new();
        local.insert(lighthouse(), LocalAdd { kind: LocalAddKind::Adding, stamp: 3 });
        local.insert(rooftop(), LocalAdd { kind: LocalAddKind::NoRoom, stamp: 3 });
        local.insert(garden(), LocalAdd { kind: LocalAddKind::Failed, stamp: 3 });
        let tail = |poll| kinds(&strip_items(Some(&admin), &local, poll))[5..].to_vec();
        // The poll that was out at the press (3) still draws the press.
        assert_eq!(tail(3), [StripKind::Adding, StripKind::TryAgain, StripKind::NoRoom]);
        // The next one sent (4) answers with the server's word.
        assert_eq!(tail(4), [StripKind::Addable, StripKind::Adding, StripKind::TryAgain]);
    }

    #[test]
    fn local_states_drop_for_rows_no_longer_addable() {
        let mut local = BTreeMap::new();
        for id in [lighthouse(), garden(), rooftop(), id('9')] {
            local.insert(id, LocalAdd { kind: LocalAddKind::Adding, stamp: 4 });
        }
        local.insert(id('8'), LocalAdd { kind: LocalAddKind::Failed, stamp: 2 });
        let next = peers(
            true,
            3,
            vec![
                // Landed: on the shelf now.
                row(&lighthouse(), "Lighthouse Tapes", true, true, Value::Null),
                row(&garden(), "Static Garden", false, true, adding()),
                row(&rooftop(), "Rooftop FM", false, true, Value::Null),
                // Still addable, but stamped before this poll was sent.
                row(&id('8'), "Eighth", false, true, Value::Null),
            ],
        );
        // `9` left the catalog.
        let kept = standing_adds(&next, &local, 4);
        assert_eq!(kept.keys().cloned().collect::<Vec<_>>(), [garden(), rooftop()]);
        // No Adds at all once `canAdd` is false.
        let mut member = next.clone();
        member.can_add = false;
        assert!(standing_adds(&member, &local, 4).is_empty());
    }

    #[test]
    fn all_alone_is_not_drawn() {
        let none = strip_items(None, &none_stand(), 0);
        assert_eq!(kinds(&none), [StripKind::All]);
        assert!(!strip_drawn(&none));
        let lonely = peers(false, 0, Vec::new());
        assert!(!strip_drawn(&strip_items(Some(&lonely), &none_stand(), 1)));
        // An admin with only the catalog to Add from has a strip.
        let catalog =
            peers(true, 0, vec![row(&lighthouse(), "Lighthouse Tapes", false, true, Value::Null)]);
        assert!(strip_drawn(&strip_items(Some(&catalog), &none_stand(), 1)));
        assert!(strip_drawn(&strip_items(Some(&stage_admin()), &none_stand(), 1)));
    }

    #[test]
    fn poll_delay_is_3_s_while_adding_or_posting_else_15_s() {
        let admin = stage_admin();
        let adding = strip_items(Some(&admin), &none_stand(), 1);
        assert_eq!(poll_delay(&adding, false), Duration::from_secs(3));
        // Failed and no room do not hurry it.
        let settled = peers(
            true,
            3,
            vec![
                row(&kitchen(), "Kitchen Radio", true, true, Value::Null),
                row(&garden(), "Static Garden", false, true, failed("no-room")),
                row(&rooftop(), "Rooftop FM", false, true, failed("offline")),
            ],
        );
        let items = strip_items(Some(&settled), &none_stand(), 1);
        assert_eq!(poll_delay(&items, false), Duration::from_secs(15));
        // An own POST still out hurries it.
        assert_eq!(poll_delay(&items, true), Duration::from_secs(3));
        // So does a press, before the server has said anything.
        let mut local = BTreeMap::new();
        local.insert(rooftop(), LocalAdd { kind: LocalAddKind::Adding, stamp: 1 });
        let pressed = strip_items(Some(&settled), &local, 1);
        assert_eq!(poll_delay(&pressed, false), Duration::from_secs(3));
        assert_eq!(PEER_POLL, Duration::from_secs(15));
        assert_eq!(PEER_POLL_FAST, Duration::from_secs(3));
    }

    #[test]
    fn every_add_answer_has_its_outcome() {
        use PeerAddKind::{Adding, Done, Failed};
        let cases = [
            (add_answer(202, None), AddOutcome::Adding),
            (add_answer(202, Some((Adding, None))), AddOutcome::Adding),
            (add_answer(200, Some((Done, None))), AddOutcome::Landed),
            (add_answer(200, Some((Adding, None))), AddOutcome::Adding),
            (add_answer(200, Some((Failed, Some("no-room")))), AddOutcome::NoRoom),
            (add_answer(200, Some((Failed, Some("offline")))), AddOutcome::Failed),
            (add_answer(200, Some((Failed, None))), AddOutcome::Failed),
            (add_answer(200, None), AddOutcome::Failed),
            (add_answer(404, None), AddOutcome::Gone),
            (add_answer(409, None), AddOutcome::Gone),
            (add_answer(403, None), AddOutcome::NotAllowed),
            (add_answer(405, None), AddOutcome::NotAllowed),
            (add_answer(400, None), AddOutcome::Failed),
            (add_answer(401, None), AddOutcome::Failed),
            (add_answer(500, None), AddOutcome::Failed),
            (add_answer(0, None), AddOutcome::Failed),
        ];
        for (answer, want) in cases {
            assert_eq!(add_outcome(&answer), want, "{answer:?}");
        }
        // What each draws until a later poll, and whether it polls at once.
        for (outcome, local, now) in [
            (AddOutcome::Adding, Some(LocalAddKind::Adding), false),
            (AddOutcome::Landed, Some(LocalAddKind::Adding), true),
            (AddOutcome::NoRoom, Some(LocalAddKind::NoRoom), false),
            (AddOutcome::Failed, Some(LocalAddKind::Failed), false),
            (AddOutcome::Gone, None, true),
            (AddOutcome::NotAllowed, None, true),
        ] {
            assert_eq!(outcome.local_kind(), local, "{outcome:?}");
            assert_eq!(outcome.polls_now(), now, "{outcome:?}");
        }
    }

    #[test]
    fn only_adding_and_landed_keep_the_id_posted() {
        for (outcome, keeps) in [
            (AddOutcome::Adding, true),
            (AddOutcome::Landed, true),
            (AddOutcome::NoRoom, false),
            (AddOutcome::Failed, false),
            (AddOutcome::Gone, false),
            (AddOutcome::NotAllowed, false),
        ] {
            assert_eq!(outcome.keeps_posted(), keeps, "{outcome:?}");
        }
    }

    #[test]
    fn landed_needs_an_own_post_now_on_the_shelf_or_done() {
        let prev = stage_admin();
        let posted: BTreeSet<String> = [lighthouse(), garden()].into_iter().collect();
        // Nothing moved: nothing landed.
        assert_eq!(landed(Some(&prev), &prev, &posted), (false, Vec::new()));
        // Lighthouse lands on the shelf, Attic leaves it.
        let next = peers(
            true,
            3,
            vec![
                row(&basement(), "Basement Hi-Fi", true, true, Value::Null),
                row(&kitchen(), "Kitchen Radio", true, true, Value::Null),
                row(&f3a(), "mStream · 3f9a", true, false, Value::Null),
                row(&lighthouse(), "Lighthouse Tapes", true, true, Value::Null),
                row(&attic(), "Attic Tapes", false, false, Value::Null),
                row(&garden(), "Static Garden", false, true, adding()),
                row(&rooftop(), "Rooftop FM", false, true, failed("offline")),
            ],
        );
        assert_eq!(landed(Some(&prev), &next, &posted), (true, vec![lighthouse()]));
        // Another client's Add, or a rotation, lands nothing of ours.
        let ours: BTreeSet<String> = [garden()].into_iter().collect();
        assert_eq!(landed(Some(&prev), &next, &ours), (false, Vec::new()));
        let with_garden = |state: PeerAddState| {
            let mut answer = prev.clone();
            answer.peers.iter_mut().find(|row| row.endpoint_id == garden()).unwrap().add =
                Some(state);
            answer
        };
        // A done Add counts before its shelf row shows.
        let done = with_garden(add_state(PeerAddKind::Done, None));
        assert_eq!(landed(Some(&prev), &done, &ours), (true, vec![garden()]));
        // Held already: no second landing.
        assert_eq!(landed(Some(&done), &done, &ours), (false, Vec::new()));
        // An own Add that has just failed stops being tracked, landing nothing.
        let broke = with_garden(add_state(PeerAddKind::Failed, Some("offline")));
        assert_eq!(landed(Some(&prev), &broke, &ours), (false, vec![garden()]));
        // Rooftop had failed already: nothing new to drop.
        let rooftop_posted: BTreeSet<String> = [rooftop()].into_iter().collect();
        assert_eq!(landed(Some(&prev), &prev, &rooftop_posted), (false, Vec::new()));
    }

    #[test]
    fn the_shelf_growing_under_no_peers_and_emptying_both_re_ask() {
        let kitchen_row = || vec![row(&kitchen(), "Kitchen Radio", true, true, Value::Null)];
        let empty = peers(true, 0, Vec::new());
        let two = peers(true, 2, kitchen_row());
        let one = peers(true, 1, kitchen_row());
        assert!(grew_under_no_peers(Some(&empty), &two, true));
        assert!(grew_under_no_peers(None, &two, true));
        assert!(!grew_under_no_peers(Some(&empty), &two, false));
        assert!(!grew_under_no_peers(Some(&one), &two, true));
        assert!(!grew_under_no_peers(Some(&empty), &empty, true));
        assert!(emptied(Some(&two), &empty));
        assert!(!emptied(Some(&empty), &empty));
        assert!(!emptied(None, &empty));
        assert!(!emptied(Some(&two), &one));
    }

    #[test]
    fn the_selected_peer_gone_falls_back_but_offline_does_not() {
        let k = kitchen();
        let one = |row: Value| peers(true, 2, vec![row]);
        let online = one(row(&k, "Kitchen Radio", true, true, Value::Null));
        let offline = one(row(&k, "Kitchen Radio", true, false, Value::Null));
        let rotated = one(row(&k, "Kitchen Radio", false, true, Value::Null));
        let absent = one(row(&basement(), "Basement Hi-Fi", true, true, Value::Null));
        assert!(!selected_gone(&online, &k));
        assert!(!selected_gone(&offline, &k));
        assert!(selected_gone(&rotated, &k));
        assert!(selected_gone(&absent, &k));
    }

    #[test]
    fn stepping_tabs_skips_add_tabs_and_stops_at_the_ends() {
        let items = strip_items(Some(&stage_admin()), &none_stand(), 1);
        let (b, k, d, e) = (basement(), kitchen(), f3a(), attic());
        assert_eq!(step_tab(&items, None, 1), Some(Some(b.clone())));
        assert_eq!(step_tab(&items, Some(&b), 1), Some(Some(k.clone())));
        assert_eq!(step_tab(&items, Some(&d), 1), Some(Some(e.clone())));
        // Attic is the last shelf tab: the add tabs after it are skipped.
        assert_eq!(step_tab(&items, Some(&e), 1), None);
        assert_eq!(step_tab(&items, Some(&b), -1), Some(None));
        assert_eq!(step_tab(&items, None, -1), None);
        assert_eq!(step_tab(&items, Some(&k), 0), None);
        // A selection no longer in the strip steps from All.
        assert_eq!(step_tab(&items, Some(&id('9')), 1), Some(Some(b)));
        // All alone steps nowhere.
        assert_eq!(step_tab(&strip_items(None, &none_stand(), 0), None, 1), None);
    }

    // ── Leads, previews and copy ────────────────────────────────────────────

    #[test]
    fn lead_key_uses_export_id_else_the_name_keys() {
        let k = kitchen();
        let peer = json!({ "endpointId": k, "name": "Kitchen Radio", "label": "Kitchen Radio" });
        let with = lead(json!({
            "title": "Signal Fires", "artist": "Paper Lantern", "exportId": "e-17", "peer": peer,
        }));
        assert_eq!(lead_key(&with), format!("{k}\0e-17"));
        // A numeric export id keys as its decimal.
        let numeric = lead(json!({ "title": "Signal Fires", "exportId": 42, "peer": peer }));
        assert_eq!(lead_key(&numeric), format!("{k}\0{}", 42));
        // Without one: the server's fold of artist, title and album.
        let folded = lead(json!({
            "title": "Don\u{2019}t  Stop", "artist": " Paper \u{2013} Lantern ",
            "album": "NIGHT Ferry", "peer": peer,
        }));
        assert_eq!(lead_key(&folded), format!("{k}\0paper - lantern|don't stop|night ferry"));
        let empty_export = lead(json!({ "title": "Signal Fires", "exportId": "", "peer": peer }));
        assert_eq!(lead_key(&empty_export), format!("{k}\0|signal fires|"));
        // The same song from another peer is another lead.
        let other = lead(json!({
            "title": "Signal Fires", "exportId": "e-17", "peer": { "endpointId": basement() },
        }));
        assert_ne!(lead_key(&other), lead_key(&with));
        // No peer at all still keys.
        assert_eq!(lead_key(&lead(json!({ "exportId": "x" }))), "\0x");
    }

    #[test]
    fn retry_wait_is_60_s_for_itunes_and_5_s_otherwise() {
        assert_eq!(retry_wait("itunes"), Duration::from_secs(60));
        assert_eq!(retry_wait(ITUNES_PLUGIN), Duration::from_secs(60));
        assert_eq!(retry_wait("deezer"), Duration::from_secs(5));
        assert_eq!(retry_wait(""), Duration::from_secs(5));
    }

    #[test]
    fn next_index_skips_known_none() {
        let none = [false, true, true, false, true];
        let known = |at: usize| none[at];
        assert_eq!(next_index(5, None, known), Some(0));
        assert_eq!(next_index(5, Some(0), known), Some(3));
        assert_eq!(next_index(5, Some(3), known), None);
        assert_eq!(next_index(5, Some(9), known), None);
        assert_eq!(next_index(0, None, |_| false), None);
        assert_eq!(next_index(3, None, |_| true), None);
    }

    #[test]
    fn copy_text_trims_and_falls_back() {
        let both = lead(json!({ "title": " Long Exposure ", "artist": " Marlowe Fields " }));
        assert_eq!(copy_text(&both).as_deref(), Some("Marlowe Fields - Long Exposure"));
        let title_only = lead(json!({ "title": "Long Exposure", "artist": "  " }));
        assert_eq!(copy_text(&title_only).as_deref(), Some("Long Exposure"));
        let artist_only = lead(json!({ "artist": "Marlowe Fields" }));
        assert_eq!(copy_text(&artist_only).as_deref(), Some("Marlowe Fields"));
        assert_eq!(copy_text(&lead(json!({ "title": " ", "artist": "" }))), None);
        assert_eq!(copy_text(&lead(json!({}))), None);
    }

    #[test]
    fn copy_keeps_each_lead_on_one_line_with_no_controls() {
        // A peer's words reach the clipboard, and from there a shell: line breaks and tabs are
        // spaces, every other control character and bidi steering mark goes.
        let leads = [
            lead(json!({ "artist": "Paper\tLantern", "title": "Harbor Lights\nrm -rf ~/Music\n" })),
            lead(json!({ "artist": "Hollow Coast", "title": "Salt\r\nRoad\u{1b}[201~ echo hi" })),
            lead(json!({
                "artist": "\u{202E}Quiet\u{2066} Engine", "title": "Tide\u{85}water\u{2028}",
            })),
            lead(json!({ "artist": "\u{1b}\u{7}", "title": "\n\r\t\u{9b}" })),
            lead(json!({ "artist": "Marlowe  Fields", "title": " Long Exposure " })),
        ];
        let list = copy_list(&leads).expect("four leads have words");
        let lines: Vec<&str> = list.split('\n').collect();
        assert_eq!(
            lines,
            [
                "Paper Lantern - Harbor Lights rm -rf ~/Music",
                "Hollow Coast - Salt Road[201~ echo hi",
                "Quiet Engine - Tide water",
                "Marlowe Fields - Long Exposure",
            ],
            "one line per lead with words"
        );
        let steering = |c: char| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
        assert!(!list.chars().any(|c| c != '\n' && (c.is_control() || steering(c))), "{list:?}");
        assert_eq!(copy_text(&leads[3]), None, "nothing but controls is nothing to copy");
    }

    #[test]
    fn a_peers_outsized_strings_key_in_bounded_work() {
        // The room keys every drawn lead on every frame: a peer's megabyte export id or padded
        // title is read only as far as any real one runs (512 characters, at most 2048 bytes).
        let k = kitchen();
        let huge = "x".repeat(1_000_000);
        let padded = format!("Signal Fires{}", " ".repeat(1_000_000));
        let long_export = lead(json!({ "exportId": huge, "peer": { "endpointId": k } }));
        assert!(lead_key(&long_export).len() <= 64 + 1 + 2048, "the export id is bounded");
        let long_names = lead(json!({
            "title": padded, "artist": huge, "album": huge, "peer": { "endpointId": k },
        }));
        let key = lead_key(&long_names);
        assert!(key.len() <= 64 + 3 + 3 * 2048, "the names are bounded: {}", key.len());
        assert!(key.contains("|signal fires|"), "a padded title still folds to its words");
        // Text at the server's own caps keys whole, in any script.
        let export = "ж".repeat(256);
        let at_cap = lead(json!({ "exportId": export, "peer": { "endpointId": k } }));
        assert_eq!(lead_key(&at_cap), format!("{k}\0{export}"));
        let title = "🎵".repeat(512);
        let at_cap = lead(json!({ "title": title, "peer": { "endpointId": k } }));
        assert_eq!(lead_key(&at_cap), format!("{k}\0|{title}|"));
    }

    #[test]
    fn copy_list_is_rank_order_joined_by_newlines_without_a_trailing_one() {
        let leads = [
            lead(json!({ "title": "Salt Road", "artist": "Hollow Coast" })),
            lead(json!({ "title": " ", "artist": "" })),
            lead(json!({ "title": "Signal Fires", "artist": "Paper Lantern" })),
            lead(json!({ "title": "Copper Dawn" })),
        ];
        assert_eq!(
            copy_list(&leads).as_deref(),
            Some("Hollow Coast - Salt Road\nPaper Lantern - Signal Fires\nCopper Dawn")
        );
        assert_eq!(copy_list(&[]), None);
        assert_eq!(copy_list(&leads[1..2]), None);
    }

    /// The rules over the stage's own answers (A's live-captured fixtures),
    /// as the boards draw them (build plan DS §2).
    #[test]
    fn the_stage_answers_read_as_the_boards_draw() {
        use crate::api::types::fixtures::{self, ATTIC, BASEMENT, F3A, KITCHEN};
        let strip = |name| strip_items(Some(&fixtures::peers(name)), &none_stand(), 1);
        let admin = strip("peers_admin");
        let ids: Vec<Option<&str>> = admin.iter().map(|item| item.id.as_deref()).collect();
        assert_eq!(ids[..5], [None, Some(BASEMENT), Some(KITCHEN), Some(ATTIC), Some(F3A)]);
        assert_eq!(&kinds(&admin)[5..], [StripKind::Addable; 3]);
        assert_eq!(poll_delay(&admin, false), PEER_POLL);
        let member = strip("peers_member");
        assert_eq!(member.len(), 5);
        assert!(member.iter().all(StripItem::selectable));
        let states = strip("peers_admin_add_states");
        assert_eq!(
            &kinds(&states)[5..],
            [StripKind::NoRoom, StripKind::Adding, StripKind::TryAgain]
        );
        assert_eq!(poll_delay(&states, false), PEER_POLL_FAST);
        // The shelf emptying under the room re-asks, and draws No peers yet.
        let empty = fixtures::peers("peers_admin_empty_shelf");
        assert!(emptied(Some(&fixtures::peers("peers_admin")), &empty));
        assert_eq!(kinds(&strip("peers_admin_empty_shelf"))[1..], [StripKind::Addable; 5]);
        let none = fixtures::taste("taste_empty_shelf");
        assert_eq!(read_answer(&none, false), AnswerRead::Keep);
        let searchable = Some(empty.shelf.searchable);
        assert_eq!(pane_of(Some(&none), false, false, false, searchable), RecsPane::NoPeers);

        // The batch, its tabs and its states.
        let pane = |answer: &TasteResponse| pane_of(Some(answer), false, false, false, Some(4));
        let all = fixtures::taste("taste_all");
        assert_eq!(read_answer(&all, false), AnswerRead::Keep);
        assert_eq!(pane(&all), RecsPane::Ready);
        let r2 = body(&r2_body(&all, 30, &[], Some(KITCHEN)).unwrap());
        assert_eq!(r2["pin"], json!(batch_pins(&all)));
        assert_eq!(r2["limit"], json!({ "seeds": 2, "network": 32 }));
        assert_eq!(r2["networkPeers"], json!([KITCHEN]));
        let leads = &all.network.as_ref().unwrap().results;
        let keys: BTreeSet<String> = leads.iter().map(lead_key).collect();
        assert_eq!(keys.len(), 12, "every lead keys apart");
        let list = copy_list(leads).unwrap();
        assert_eq!(list.lines().count(), 12);
        assert_eq!(list.lines().next(), Some("Hollow Coast - Salt Road"));
        assert!(!list.ends_with('\n'));
        let tabs = [("taste_kitchen", KITCHEN), ("taste_basement", BASEMENT), ("taste_3f9a", F3A)];
        for (name, tab) in tabs {
            let answer = fixtures::taste(name);
            assert_eq!(read_answer(&answer, true), AnswerRead::Keep, "{name}");
            assert_eq!(pane(&answer), RecsPane::Ready, "{name}");
            let peer = &answer.network.as_ref().unwrap().results[0].peer.endpoint_id;
            assert_eq!(peer.as_deref(), Some(tab), "{name}");
        }
        assert_eq!(pane(&fixtures::taste("taste_attic")), RecsPane::NothingNew);
        let gone = fixtures::taste("taste_not_on_shelf");
        assert_eq!(read_answer(&gone, true), AnswerRead::NotOnShelf);
        let random = fixtures::taste("taste_random");
        assert!(is_random(&random));
        assert_eq!(read_answer(&random, false), AnswerRead::Keep);
        assert_eq!(pane(&random), RecsPane::Ready);
        assert_eq!(pane(&fixtures::taste("taste_scan_pending")), RecsPane::ScanPending);
        assert_eq!(read_answer(&fixtures::taste("taste_network_null"), false), AnswerRead::Off);
        let error = fixtures::taste("taste_error");
        assert_eq!(read_answer(&error, false), AnswerRead::ShowNotKeep);
        assert_eq!(pane(&error), RecsPane::TryAgain);
    }

    #[test]
    fn no_pick_left_needs_an_unpinned_ask_with_passed_picks() {
        let random = answer(json!({
            "seedSource": "random", "seeds": [], "network": { "results": [] },
        }));
        assert!(no_pick_left(&random, false, 2));
        // A pinned re-ask on the batch, or no passed picks, is never one.
        assert!(!no_pick_left(&random, true, 2));
        assert!(!no_pick_left(&random, false, 0));
        // A seed came back: a pick was left.
        let picked = answer(json!({ "seedSource": "random", "seeds": seeds(&["pick.mp3"]) }));
        assert!(!no_pick_left(&picked, false, 2));
        // The server's not-analyzed answer, or its fallback flag, reads the same.
        assert!(no_pick_left(&answer(json!({ "notAnalyzed": true })), false, 1));
        assert!(no_pick_left(&answer(json!({ "fallback": true })), false, 1));
        // A history answer with no seed is something else.
        assert!(!no_pick_left(&answer(json!({ "seedSource": "history" })), false, 1));
    }
}
