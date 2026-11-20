//! The P2P discovery room: the webapp's Discovery page, in the kit.
//!
//! Top to bottom, in the web page's own order: one state row (the dot line
//! — connected, joined-and-waiting, reconnecting, not joined, unavailable),
//! the four tabs (Stats · Activity · Invite · Config) in a pane of eight
//! rows, then "Servers you follow" as a table with the rest of the screen.
//! Tips on the bottom edge; no bottom bar, no gold rule — nothing here is
//! a step. Off the network, the room is the pitch, one affirmative card
//! (the add-folder card's shape) that joins at once, and the
//! federation-requests opt-in as a checkbox card, on by default.
//!
//! The catalog's card chips became the SNAPSHOT and FEDERATION columns;
//! the card's action row is the Enter sheet (a kit list picker), and every
//! action on it is also a direct key on the row. Every server call runs
//! on a worker thread (the wizard's Job/Done pattern); the room polls
//! itself every ten seconds while it is on the network, as the page does.
//!
//! This file is the room itself: its state, its worker, its keys and every
//! word it says. It names no drawing type of ratatui's or the kit's. A key
//! comes in as crate::input's KeyEvent; the fields the room edits (the
//! filter, the befriend box, the modals' fields) are tui_input's, and
//! take that key as crossterm's event, so those two stay here with the
//! keymap. Its views are child modules that read the room the same way;
//! today there is one, `cells` (cells.rs), in character cells, for a
//! terminal and for the GUI player's Admin tab. A click in a view is one
//! of the room's [`Act`]s and a key is [`Room::key`], so a view cannot
//! disagree with another about what the room does. What a view keeps
//! between frames it keeps in a type of its own, held here unopened
//! (`cells::State`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Claim, Outcome, gate_message, printable, short_id};
#[cfg(test)]
use super::{age_text, fmt_bytes, fmt_count, iso_at, iso_unix, unix_now};
use crate::api::types::{
    ActivityEntry, CatalogPeer, DiscoveryActivity, DiscoveryCatalog, DiscoveryStatus,
    FederationParams, FederationRequest,
};
use crate::api::{ApiError, Client, DiscoverySetting, PeerAction};
use crate::input::{KeyCode, KeyEvent};
use crate::kit;
use crate::kit::clipboard::Copied;

/// The room in character cells: the terminal's room and the GUI player's
/// Admin tab.
mod cells;

/// The page's own cadence: the mesh weaves in over a minute, and nobody
/// should have to mash a refresh key to watch it.
const POLL_EVERY: Duration = Duration::from_secs(10);
/// The server's limits on the announced identity (and no `|` — the
/// announcement's signing separator).
const NAME_MAX: usize = 64;
const DESCRIPTION_MAX: usize = 180;
const MESSAGE_MAX: usize = 500;

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Stats,
    Activity,
    Invite,
    Config,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Stats, Tab::Activity, Tab::Invite, Tab::Config];

    fn label(self) -> String {
        match self {
            Tab::Stats => t!("p2p.tab_stats"),
            Tab::Activity => t!("p2p.tab_activity"),
            Tab::Invite => t!("p2p.tab_invite"),
            Tab::Config => t!("p2p.tab_config"),
        }
        .to_string()
    }

    fn next(self, forward: bool) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        let n = Tab::ALL.len();
        Tab::ALL[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }
}

/// The catalog's relationship column, derived from the federation
/// requests: an existing pairing beats a pending inbound beats a live
/// outbound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    None,
    Federated,
    Theirs,
    Sent,
}

/// The federate compose modal: an optional message and the libraries
/// offered back if they accept (all of them, until unticked).
#[derive(Debug, Clone)]
pub(crate) struct Compose {
    pub id: String,
    pub name: String,
    pub message: Input,
    pub libraries: Vec<(String, bool)>,
    pub loaded: bool,
    /// `None` = the message field; `Some(i)` = library row i.
    pub focus: Option<usize>,
    pub error: Option<String>,
}

/// The identity modal: the two fields the network sees.
#[derive(Debug, Clone)]
pub(crate) struct IdentityDraft {
    pub name: Input,
    pub description: Input,
    pub on_description: bool,
    pub error: Option<String>,
}

/// One numeric setting being edited.
#[derive(Debug, Clone)]
pub(crate) struct SettingDraft {
    pub which: DiscoverySetting,
    pub value: Input,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Modal {
    None,
    /// Enter on a row: the server's facts and its actions as a list.
    Sheet { id: String, sel: usize },
    Federate(Compose),
    Identity(IdentityDraft),
    Setting(SettingDraft),
    /// The gate before blocking the server with this endpoint id.
    Block(String),
    /// The gate before leaving the network.
    Leave,
    /// The blocked list, with its cursor — Enter unblocks.
    Blocked(usize),
}

/// Everything a click can mean. Rebuilt into a rect registry every draw;
/// the last-drawn rect wins, which is what puts modals above the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Tab(Tab),
    Join,
    ToggleRequests,
    Leave,
    LeaveConfirm,
    LeaveCancel,
    /// A table row, by VISIBLE index.
    OpenSheet(usize),
    SheetPick(usize),
    SheetRun,
    SheetClose,
    Fetch(String),
    Pin(String, bool),
    RemoveSnapshot(String),
    Forget(String),
    Federate(String),
    Block(String),
    BlockConfirm,
    BlockCancel,
    ComposeFocusMessage,
    ComposeToggle(usize),
    ComposeSend,
    ComposeCancel,
    IdentityOpen,
    IdentityFocus(bool),
    IdentitySave,
    IdentityCancel,
    SettingOpen(DiscoverySetting),
    SettingSave,
    SettingCancel,
    TicketFocus,
    TicketJoin,
    CopyTicket,
    ToggleIncompatible,
    FilterOpen,
    BlockedOpen,
    BlockedPick(usize),
    BlockedRun,
    BlockedClose,
    TableScroll(i8),
    TableScrollTo(usize),
    Quit,
}

/// A server call queued from input handling and run right after the next
/// draw. Ops carry everything they need — the worker never sees the room.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// Status, and — once on the network — the catalog, federation's
    /// state and the requests; the activity ring too when asked.
    Load { include_incompatible: bool, activity_since: Option<u64> },
    Activity(u64),
    JoinNetwork { accept_requests: bool },
    LeaveNetwork,
    Identity { name: Option<String>, description: Option<String> },
    Befriend(String),
    Fetch { id: String, name: String },
    Pin { id: String, pinned: bool },
    Peer { action: PeerAction, id: String, name: String },
    Setting { which: DiscoverySetting, value: u64 },
    SendRequest { id: String, name: String, offer: Vec<String>, message: String },
    /// The library names, for the compose modal's offer list.
    Libraries,
}

/// One load's answer: the status is required, the rest is what the
/// server was willing to add.
struct Loaded {
    status: DiscoveryStatus,
    catalog: Option<DiscoveryCatalog>,
    federation: Option<FederationParams>,
    requests: Option<Vec<FederationRequest>>,
    activity: Option<DiscoveryActivity>,
}

/// What the worker sends back for each [`Op`].
enum Done {
    Loaded(Result<Box<Loaded>, ApiError>),
    Activity(Result<DiscoveryActivity, ApiError>),
    Joined(Result<serde_json::Value, ApiError>),
    Left(Result<(), ApiError>),
    Identity(Result<(), ApiError>),
    Befriended(Result<(), ApiError>),
    Fetched { name: String, result: Result<(), ApiError> },
    Pinned { id: String, pinned: bool, result: Result<(), ApiError> },
    Peer { action: PeerAction, name: String, result: Result<(), ApiError> },
    Setting { which: DiscoverySetting, value: u64, result: Result<(), ApiError> },
    RequestSent { name: String, result: Result<(), ApiError> },
    Libraries(Result<Vec<String>, ApiError>),
}

fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load { include_incompatible, activity_since } => {
                    Done::Loaded(crate::api::wait(async {
                        let status = client.admin_discovery_status_async().await?;
                        if !status.enabled {
                            return Ok(Box::new(Loaded {
                                status,
                                catalog: None,
                                federation: None,
                                requests: None,
                                activity: None,
                            }));
                        }
                        // Past the switch no read needs another's answer:
                        // they go out together, one round trip, not four
                        // (performance audit #83).
                        let activity = async {
                            match activity_since {
                                Some(since) => client.admin_discovery_activity_async(since).await.ok(),
                                None => None,
                            }
                        };
                        let (catalog, federation, requests, activity) = tokio::join!(
                            client.admin_discovery_catalog_async(include_incompatible),
                            client.admin_federation_async(),
                            client.admin_federation_requests_async(),
                            activity,
                        );
                        let catalog = catalog?;
                        // Best-effort: the relationship column just stays
                        // blank when federation cannot answer.
                        let federation = federation.ok();
                        let requests = requests.ok().map(|r| r.requests);
                        Ok(Box::new(Loaded { status, catalog: Some(catalog), federation, requests, activity }))
                    }))
                }
                Op::Activity(since) => Done::Activity(client.admin_discovery_activity(since)),
                Op::JoinNetwork { accept_requests } => {
                    Done::Joined(client.admin_discovery_join_network(accept_requests))
                }
                Op::LeaveNetwork => Done::Left(client.admin_discovery_enabled(false).map(|_| ())),
                Op::Identity { name, description } => Done::Identity((|| {
                    if let Some(name) = name {
                        client.admin_discovery_set_name(&name)?;
                    }
                    if let Some(description) = description {
                        client.admin_discovery_set_description(&description)?;
                    }
                    Ok(())
                })()),
                Op::Befriend(peer) => {
                    Done::Befriended(client.admin_discovery_befriend(&peer).map(|_| ()))
                }
                Op::Fetch { id, name } => {
                    let result = client.admin_discovery_fetch(&id).map(|_| ());
                    Done::Fetched { name, result }
                }
                Op::Pin { id, pinned } => {
                    let result = client.admin_discovery_pin(&id, pinned).map(|_| ());
                    Done::Pinned { id, pinned, result }
                }
                Op::Peer { action, id, name } => {
                    let result = client.admin_discovery_peer(action, &id).map(|_| ());
                    Done::Peer { action, name, result }
                }
                Op::Setting { which, value } => {
                    let result = client.admin_discovery_setting(which, value).map(|_| ());
                    Done::Setting { which, value, result }
                }
                Op::SendRequest { id, name, offer, message } => {
                    let result = client
                        .admin_federation_request_send(&id, &offer, Some(&message))
                        .map(|_| ());
                    Done::RequestSent { name, result }
                }
                Op::Libraries => Done::Libraries(
                    client.admin_directories().map(|dirs| dirs.into_keys().collect()),
                ),
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

pub(crate) struct Room {
    client: Arc<Client>,
    to_worker: Sender<(Arc<Client>, Op)>,
    from_worker: Receiver<Done>,
    /// `None` until the first load answers.
    pub status: Option<DiscoveryStatus>,
    pub catalog: DiscoveryCatalog,
    pub federation: FederationParams,
    pub requests: Vec<FederationRequest>,
    /// The discovery log ring, oldest first, as delta-polled by `seq`.
    pub activity: Vec<ActivityEntry>,
    activity_seq: u64,
    /// The Activity pane's wheel offset from the newest line.
    ascroll: usize,
    pub tab: Tab,
    /// The KEYBOARD cursor over the VISIBLE rows — `None` until ↑/↓.
    pub sel: Option<usize>,
    /// The `/` filter: a line editor, live while `filter_on`, taking the
    /// keys while `filter_focus`.
    filter: Input,
    filter_on: bool,
    filter_focus: bool,
    show_incompatible: bool,
    /// The off-page opt-in: open the federation request inbox on join.
    pub accept_requests: bool,
    /// The Invite pane's befriend box.
    ticket: Input,
    ticket_focus: bool,
    pub modal: Modal,
    /// One line of status above the tips: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    tscroll: usize,
    last_load: Option<Instant>,
    /// After a join, the next load that shows the network on opens the
    /// identity modal — 'mStream' beside a thousand 'mStream's is the
    /// first thing everyone renames.
    name_after_join: bool,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, where it last drew the tabs' pane, and the cursor the table
    /// last followed.
    cells: cells::State,
}

impl Room {
    pub(super) fn new(client: Client) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            status: None,
            catalog: DiscoveryCatalog::default(),
            federation: FederationParams::default(),
            requests: Vec::new(),
            activity: Vec::new(),
            activity_seq: 0,
            ascroll: 0,
            tab: Tab::Stats,
            sel: None,
            filter: Input::default(),
            filter_on: false,
            filter_focus: false,
            show_incompatible: false,
            accept_requests: true,
            ticket: Input::default(),
            ticket_focus: false,
            modal: Modal::None,
            note: None,
            busy: None,
            queued: None,
            in_flight: false,
            tscroll: 0,
            last_load: None,
            name_after_join: false,
            cells: cells::State::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    /// A reload, with the busy note when asked for and quiet for the poll.
    fn reload(&mut self, loud: bool) {
        let op = Op::Load {
            include_incompatible: self.show_incompatible,
            activity_since: (self.tab == Tab::Activity).then_some(self.activity_seq),
        };
        if loud {
            self.queue(op, t!("p2p.busy_loading"));
        } else {
            self.queued = Some(op);
        }
    }

    fn enabled(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.enabled)
    }

    /// The catalog rows the table shows: the filter's matches, in the
    /// server's order (seeders, then online, then size).
    pub(crate) fn rows(&self) -> Vec<&CatalogPeer> {
        self.row_indices().into_iter().map(|i| &self.catalog.peers[i]).collect()
    }

    /// [`Room::rows`] as places in the catalog — what the table draws
    /// from, beside the room it registers its clicks on.
    fn row_indices(&self) -> Vec<usize> {
        let q = if self.filter_on { self.filter.value().trim().to_lowercase() } else { String::new() };
        self.catalog
            .peers
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                q.is_empty()
                    || p.payload.name.to_lowercase().contains(&q)
                    || p.payload.description.to_lowercase().contains(&q)
                    || p.from.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn peer(&self, id: &str) -> Option<&CatalogPeer> {
        self.catalog.peers.iter().find(|p| p.from == id)
    }

    fn selected_peer(&self) -> Option<&CatalogPeer> {
        self.sel.and_then(|s| self.rows().get(s).copied())
    }

    fn relation(&self, id: &str) -> Relation {
        let rows: Vec<&FederationRequest> =
            self.requests.iter().filter(|r| r.peer_endpoint_id == id).collect();
        if rows.iter().any(|r| r.state == "completed") {
            Relation::Federated
        } else if rows.iter().any(|r| {
            r.direction == "in" && matches!(r.state.as_str(), "received" | "accepted" | "granting")
        }) {
            Relation::Theirs
        } else if rows.iter().any(|r| {
            r.direction == "out"
                && matches!(r.state.as_str(), "pending-delivery" | "delivered" | "granting")
        }) {
            Relation::Sent
        } else {
            Relation::None
        }
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    /// A key press: the room's keymap, the same under any view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// Whether one of the room's modals is up.
    pub(crate) fn modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    /// Every key while a modal is up, and while the filter or the befriend
    /// box has the caret: a typed `L` is a letter of the ticket, not the
    /// host's.
    pub(crate) fn claim(&self) -> Claim {
        if self.modal_open() || self.filter_focus || self.ticket_focus { Claim::All } else { Claim::Open }
    }

    /// The keyboard tips, in the words the room's own tips row uses: what
    /// a host's footer shows while the room is hosted.
    pub(crate) fn hint(&self) -> String {
        footer_hint(self)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Tab(tab) => self.switch_tab(tab),
            Act::Join => {
                if let Some(s) = &self.status
                    && !s.enabled
                    && (s.binary_found || s.binary_fetchable)
                {
                    let accept_requests = self.accept_requests;
                    self.name_after_join = true;
                    self.queue(Op::JoinNetwork { accept_requests }, t!("p2p.busy_joining"));
                }
            }
            Act::ToggleRequests => self.accept_requests = !self.accept_requests,
            Act::Leave => {
                if self.enabled() {
                    self.modal = Modal::Leave;
                }
            }
            Act::LeaveConfirm => {
                self.modal = Modal::None;
                self.queue(Op::LeaveNetwork, t!("p2p.busy_leaving"));
            }
            Act::LeaveCancel
            | Act::SheetClose
            | Act::BlockCancel
            | Act::ComposeCancel
            | Act::IdentityCancel
            | Act::SettingCancel
            | Act::BlockedClose => self.modal = Modal::None,
            Act::OpenSheet(i) => {
                if let Some(peer) = self.rows().get(i) {
                    self.modal = Modal::Sheet { id: peer.from.clone(), sel: 0 };
                }
            }
            Act::SheetPick(i) => {
                if let Modal::Sheet { sel, .. } = &mut self.modal {
                    *sel = i;
                }
            }
            Act::SheetRun => {
                if let Modal::Sheet { id, sel } = &self.modal
                    && let Some(peer) = self.peer(id)
                    && let Some((_, _, act)) = self.sheet_menu(peer).get(*sel)
                {
                    let act = act.clone();
                    self.modal = Modal::None;
                    return self.act(act);
                }
            }
            Act::Fetch(id) => {
                if let Some(peer) = self.peer(&id) {
                    let name = display_name(peer);
                    self.modal = Modal::None;
                    self.queue(Op::Fetch { id, name: name.clone() }, t!("p2p.busy_fetching", name = name));
                }
            }
            Act::Pin(id, pinned) => {
                if self.peer(&id).is_some_and(|p| p.fetched.is_some()) {
                    self.modal = Modal::None;
                    self.queue(Op::Pin { id, pinned }, t!("p2p.busy_saving"));
                }
            }
            Act::RemoveSnapshot(id) => self.peer_op(PeerAction::RemoveSnapshot, id),
            Act::Forget(id) => self.peer_op(PeerAction::Forget, id),
            Act::Federate(id) => {
                if let Some(peer) = self.peer(&id)
                    && self.federation.available
                    && self.relation(&id) == Relation::None
                {
                    self.modal = Modal::Federate(Compose {
                        id: id.clone(),
                        name: display_name(peer),
                        message: Input::default(),
                        libraries: Vec::new(),
                        loaded: false,
                        focus: None,
                        error: None,
                    });
                    self.queue(Op::Libraries, t!("p2p.busy_libraries"));
                }
            }
            Act::Block(id) => {
                if self.peer(&id).is_some() {
                    self.modal = Modal::Block(id);
                }
            }
            Act::BlockConfirm => {
                if let Modal::Block(id) = &self.modal {
                    let id = id.clone();
                    self.modal = Modal::None;
                    self.peer_op(PeerAction::Block, id);
                }
            }
            Act::ComposeFocusMessage => {
                if let Modal::Federate(c) = &mut self.modal {
                    c.focus = None;
                }
            }
            Act::ComposeToggle(i) => {
                if let Modal::Federate(c) = &mut self.modal
                    && let Some((_, on)) = c.libraries.get_mut(i)
                {
                    *on = !*on;
                    c.focus = Some(i);
                }
            }
            Act::ComposeSend => {
                if let Modal::Federate(c) = &self.modal
                    && c.loaded
                {
                    let (id, name) = (c.id.clone(), c.name.clone());
                    let offer: Vec<String> =
                        c.libraries.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect();
                    let message = c.message.value().trim().to_string();
                    self.queue(Op::SendRequest { id, name, offer, message }, t!("p2p.busy_sending"));
                }
            }
            Act::IdentityOpen => {
                if let Some(s) = &self.status
                    && s.enabled
                {
                    self.modal = Modal::Identity(IdentityDraft {
                        name: Input::new(s.server_name.clone()),
                        description: Input::new(s.server_description.clone()),
                        on_description: false,
                        error: None,
                    });
                }
            }
            Act::IdentityFocus(description) => {
                if let Modal::Identity(d) = &mut self.modal {
                    d.on_description = description;
                }
            }
            Act::IdentitySave => self.submit_identity(),
            Act::SettingOpen(which) => {
                if let Some(s) = &self.status
                    && s.enabled
                {
                    let current = match which {
                        DiscoverySetting::MaxStorageMb => s.max_peer_db_storage_mb,
                        DiscoverySetting::PeerRetentionDays => s.peer_retention_days as u64,
                        DiscoverySetting::AutoFetchCount => s.auto_fetch_count as u64,
                        DiscoverySetting::RotationDays => s.rotation_days as u64,
                        DiscoverySetting::SidecarMaxRssMb => s.watchdog.max_rss_mb,
                    };
                    self.modal = Modal::Setting(SettingDraft {
                        which,
                        value: Input::new(current.to_string()),
                        error: None,
                    });
                }
            }
            Act::SettingSave => self.submit_setting(),
            Act::TicketFocus => {
                if self.enabled() {
                    self.tab = Tab::Invite;
                    self.ticket_focus = true;
                    self.filter_focus = false;
                }
            }
            Act::TicketJoin => {
                let peer = self.ticket.value().trim().to_string();
                if !peer.is_empty() {
                    self.queue(Op::Befriend(peer), t!("p2p.busy_befriending"));
                }
            }
            Act::CopyTicket => {
                if let Some(ticket) = self.status.as_ref().and_then(|s| s.ticket.clone()) {
                    let said = match kit::clipboard::copy(&ticket) {
                        Copied::Clipboard => t!("p2p.copied_clipboard"),
                        Copied::Terminal => t!("p2p.copied"),
                        Copied::Failed => t!("p2p.copy_failed"),
                    };
                    self.note = Some((said.to_string(), false));
                }
            }
            Act::ToggleIncompatible => {
                // A server-side filter: the toggle re-fetches rather than
                // un-hiding stale client state.
                self.show_incompatible = !self.show_incompatible;
                self.reload(true);
            }
            Act::FilterOpen => {
                if self.enabled() {
                    self.filter_on = true;
                    self.filter_focus = true;
                    self.ticket_focus = false;
                    self.sel = None;
                }
            }
            Act::BlockedOpen => {
                if self.status.as_ref().is_some_and(|s| !s.blocked_peers.is_empty()) {
                    self.modal = Modal::Blocked(0);
                }
            }
            Act::BlockedPick(i) => {
                if let Modal::Blocked(sel) = &mut self.modal {
                    *sel = i;
                }
            }
            Act::BlockedRun => {
                if let Modal::Blocked(sel) = &self.modal
                    && let Some(id) = self.status.as_ref().and_then(|s| s.blocked_peers.get(*sel))
                {
                    let id = id.clone();
                    self.modal = Modal::None;
                    let name = short_id(&id);
                    self.queue(
                        Op::Peer { action: PeerAction::Unblock, id, name },
                        t!("p2p.busy_saving"),
                    );
                }
            }
            Act::TableScroll(delta) => {
                self.tscroll = if delta < 0 {
                    self.tscroll.saturating_sub(1)
                } else {
                    self.tscroll.saturating_add(1)
                };
            }
            Act::TableScrollTo(pos) => self.tscroll = pos,
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    fn switch_tab(&mut self, tab: Tab) {
        if !self.enabled() {
            return;
        }
        self.tab = tab;
        self.ticket_focus = false;
        if tab == Tab::Activity {
            // The ring past what the room holds — everything, the first time.
            self.queued = Some(Op::Activity(self.activity_seq));
        }
    }

    /// The one-argument peer actions, named for the note they leave.
    fn peer_op(&mut self, action: PeerAction, id: String) {
        let Some(peer) = self.peer(&id) else { return };
        // The server refuses these itself, but the tips never offer them.
        match action {
            PeerAction::RemoveSnapshot if peer.fetched.is_none() => return,
            PeerAction::Forget if peer.online || peer.fetched.is_some() => return,
            _ => {}
        }
        let name = display_name(peer);
        self.modal = Modal::None;
        self.queue(Op::Peer { action, id, name }, t!("p2p.busy_saving"));
    }

    /// Save the identity — only the fields that changed, after the one
    /// refusal that needs no server: a blank name.
    fn submit_identity(&mut self) {
        let current = self.status.clone().unwrap_or_default();
        let Modal::Identity(draft) = &mut self.modal else { return };
        let name = draft.name.value().trim().to_string();
        let description = draft.description.value().trim().to_string();
        if name.is_empty() {
            draft.error = Some(t!("p2p.identity_empty").to_string());
            return;
        }
        draft.error = None;
        let name = (name != current.server_name).then_some(name);
        let description = (description != current.server_description).then_some(description);
        if name.is_none() && description.is_none() {
            self.modal = Modal::None;
            return;
        }
        self.queue(Op::Identity { name, description }, t!("p2p.busy_saving"));
    }

    /// Save a setting — after the bounds check the server would make.
    fn submit_setting(&mut self) {
        let Modal::Setting(draft) = &mut self.modal else { return };
        let (min, max) = draft.which.bounds();
        match draft.value.value().trim().parse::<u64>() {
            Ok(v) if (min..=max).contains(&v) => {
                draft.error = None;
                let which = draft.which;
                self.queue(Op::Setting { which, value: v }, t!("p2p.busy_saving"));
            }
            _ => {
                draft.error = Some(t!("p2p.setting_invalid", min = min, max = max).to_string());
            }
        }
    }

    /// The sheet's rows for this peer: key, label, action — only what
    /// applies (hidden, never greyed).
    fn sheet_menu(&self, peer: &CatalogPeer) -> Vec<(char, String, Act)> {
        let id = peer.from.clone();
        let held = peer.fetched.as_ref();
        let mut menu = vec![(
            'd',
            if held.is_some() { t!("p2p.menu_update") } else { t!("p2p.menu_download") }.to_string(),
            Act::Fetch(id.clone()),
        )];
        if let Some(h) = held {
            menu.push((
                'p',
                if h.pinned { t!("p2p.menu_unpin") } else { t!("p2p.menu_pin") }.to_string(),
                Act::Pin(id.clone(), !h.pinned),
            ));
            menu.push(('r', t!("p2p.menu_remove").to_string(), Act::RemoveSnapshot(id.clone())));
        }
        if self.federation.available && self.relation(&id) == Relation::None {
            menu.push(('f', t!("p2p.menu_federate").to_string(), Act::Federate(id.clone())));
        }
        if !peer.online && held.is_none() {
            menu.push(('g', t!("p2p.menu_forget").to_string(), Act::Forget(id.clone())));
        }
        menu.push(('b', t!("p2p.menu_block").to_string(), Act::Block(id)));
        menu
    }

    // ── Server calls ────────────────────────────────────────────────────────

    /// Hand the queued op to the worker. Ops are single-flight: while one is
    /// in flight the UI shows its busy note and further queues wait.
    fn dispatch_queued(&mut self) {
        if self.in_flight {
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
        if matches!(op, Op::Load { .. }) {
            self.last_load = Some(Instant::now());
        }
        if self.to_worker.send((self.client.clone(), op)).is_err() {
            self.in_flight = false;
            self.note = Some((t!("note.worker_gone").to_string(), true));
        }
    }

    /// Fold one worker result back into the state.
    fn apply(&mut self, done: Done) {
        self.in_flight = false;
        self.busy = None;
        match done {
            Done::Loaded(Ok(loaded)) => {
                let loaded = *loaded;
                let keep = self.selected_peer().map(|p| p.from.clone());
                let enabled = loaded.status.enabled;
                self.status = Some(loaded.status);
                if let Some(catalog) = loaded.catalog {
                    self.catalog = catalog;
                } else {
                    self.catalog = DiscoveryCatalog::default();
                }
                if let Some(federation) = loaded.federation {
                    self.federation = federation;
                }
                if let Some(requests) = loaded.requests {
                    self.requests = requests;
                }
                if let Some(activity) = loaded.activity {
                    self.take_activity(activity);
                }
                // The cursor follows its server through a re-sort, and
                // stays a row that exists.
                self.sel = keep
                    .and_then(|id| self.rows().iter().position(|p| p.from == id))
                    .or(match (self.sel, self.rows().len()) {
                        (Some(s), n) if n > 0 => Some(s.min(n - 1)),
                        _ => None,
                    });
                if !enabled {
                    self.sel = None;
                    self.filter_on = false;
                    self.filter_focus = false;
                    self.ticket_focus = false;
                }
                if self.name_after_join && enabled {
                    self.name_after_join = false;
                    self.act(Act::IdentityOpen);
                }
            }
            Done::Loaded(Err(e)) => {
                self.note = Some((gate_message(&e, &t!("p2p.load_failed")), true));
            }
            Done::Activity(Ok(activity)) => self.take_activity(activity),
            Done::Activity(Err(_)) => {} // quiet — the poll or next tab click retries
            Done::Joined(Ok(answer)) => {
                let mut note = t!("p2p.joined_network").to_string();
                if answer.get("acceptRequests").and_then(|v| v.as_bool()) == Some(true) {
                    note.push_str(&t!("p2p.joined_inbox"));
                }
                // Partial failure: discovery came up, the inbox half did
                // not — say exactly that, never "it all failed".
                let mut is_err = false;
                if let Some(err) = answer.get("federationError").and_then(|v| v.as_str()) {
                    note = t!("p2p.inbox_failed", err = err).to_string();
                    is_err = true;
                }
                self.note = Some((note, is_err));
                self.reload(true);
            }
            Done::Joined(Err(e)) => {
                self.name_after_join = false;
                self.fail(&t!("p2p.fail_join"), e);
                self.reload(false);
            }
            Done::Left(Ok(())) => {
                self.note = None;
                self.reload(true);
            }
            Done::Left(Err(e)) => {
                self.fail(&t!("p2p.fail_leave"), e);
                self.reload(false);
            }
            Done::Identity(Ok(())) => {
                self.modal = Modal::None;
                self.note = Some((t!("p2p.done_identity").to_string(), false));
                self.reload(false);
            }
            Done::Identity(Err(e)) => {
                let what = t!("p2p.fail_identity").to_string();
                match &mut self.modal {
                    Modal::Identity(d) => d.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Befriended(Ok(())) => {
                self.ticket = Input::default();
                self.ticket_focus = false;
                self.note = Some((t!("p2p.joined_peer").to_string(), false));
                self.reload(false);
            }
            Done::Befriended(Err(e)) => self.fail(&t!("p2p.fail_befriend"), e),
            Done::Fetched { name, result: Ok(()) } => {
                self.note = Some((t!("p2p.done_fetched", name = name).to_string(), false));
                self.reload(false);
            }
            Done::Fetched { result: Err(e), .. } => {
                self.fail(&t!("p2p.fail_fetch"), e);
                self.reload(false);
            }
            Done::Pinned { id, pinned, result: Ok(()) } => {
                if let Some(h) = self.catalog.peers.iter_mut().find(|p| p.from == id).and_then(|p| p.fetched.as_mut()) {
                    h.pinned = pinned;
                }
            }
            Done::Pinned { pinned, result: Err(e), .. } => {
                self.fail(&if pinned { t!("p2p.fail_pin") } else { t!("p2p.fail_unpin") }, e);
            }
            Done::Peer { action, name, result: Ok(()) } => {
                let note = match action {
                    PeerAction::RemoveSnapshot => t!("p2p.done_removed", name = name),
                    PeerAction::Forget => t!("p2p.done_forgotten", name = name),
                    PeerAction::Block => t!("p2p.done_blocked", name = name),
                    PeerAction::Unblock => t!("p2p.done_unblocked"),
                };
                self.note = Some((note.to_string(), false));
                self.reload(false);
            }
            Done::Peer { action, result: Err(e), .. } => {
                let what = match action {
                    PeerAction::RemoveSnapshot => t!("p2p.fail_remove"),
                    PeerAction::Forget => t!("p2p.fail_forget"),
                    PeerAction::Block => t!("p2p.fail_block"),
                    PeerAction::Unblock => t!("p2p.fail_unblock"),
                };
                self.fail(&what, e);
            }
            Done::Setting { which, value, result: Ok(()) } => {
                if let Some(s) = &mut self.status {
                    match which {
                        DiscoverySetting::MaxStorageMb => s.max_peer_db_storage_mb = value,
                        DiscoverySetting::PeerRetentionDays => s.peer_retention_days = value as u32,
                        DiscoverySetting::AutoFetchCount => s.auto_fetch_count = value as u32,
                        DiscoverySetting::RotationDays => s.rotation_days = value as u32,
                        DiscoverySetting::SidecarMaxRssMb => s.watchdog.max_rss_mb = value,
                    }
                }
                if matches!(self.modal, Modal::Setting(_)) {
                    self.modal = Modal::None;
                }
                self.note = Some((t!("p2p.done_setting").to_string(), false));
            }
            Done::Setting { result: Err(e), .. } => {
                let what = t!("p2p.fail_setting").to_string();
                match &mut self.modal {
                    Modal::Setting(d) => d.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::RequestSent { name, result: Ok(()) } => {
                self.modal = Modal::None;
                self.note = Some((t!("p2p.request_sent", name = name).to_string(), false));
                self.reload(false);
            }
            Done::RequestSent { result: Err(e), .. } => {
                let what = t!("p2p.fail_request").to_string();
                match &mut self.modal {
                    Modal::Federate(c) => c.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Libraries(Ok(names)) => {
                if let Modal::Federate(c) = &mut self.modal {
                    c.libraries = names.into_iter().map(|n| (n, true)).collect();
                    c.loaded = true;
                }
            }
            Done::Libraries(Err(e)) => {
                let what = t!("p2p.fail_libraries").to_string();
                match &mut self.modal {
                    Modal::Federate(c) => {
                        c.loaded = true;
                        c.error = Some(format!("{what}: {e}"));
                    }
                    _ => self.fail(&what, e),
                }
            }
        }
    }

    /// Append a delta from the ring; a `last_seq` below the cursor means
    /// the server restarted, so the history starts over.
    fn take_activity(&mut self, activity: DiscoveryActivity) {
        if activity.last_seq < self.activity_seq {
            self.activity.clear();
        }
        self.activity.extend(activity.entries);
        if self.activity.len() > 500 {
            let drop = self.activity.len() - 500;
            self.activity.drain(..drop);
        }
        self.activity_seq = activity.last_seq;
    }

    fn fail(&mut self, what: &str, e: ApiError) {
        self.note = Some((format!("{what}: {e}"), true));
    }

    /// Fold in whatever the worker finished, and say whether anything
    /// came: a frame to draw, whichever view draws it.
    pub(crate) fn absorb(&mut self) -> bool {
        let mut folded = false;
        loop {
            match self.from_worker.try_recv() {
                Ok(done) => self.apply(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.note = Some((t!("note.worker_gone").to_string(), true));
                    return true;
                }
            }
            folded = true;
        }
        folded
    }

    /// Hand the worker the next queued op.
    pub(crate) fn pump(&mut self) {
        self.dispatch_queued();
    }

    /// Whether a call is out with the worker.
    pub(crate) fn awaiting(&self) -> bool {
        self.in_flight
    }

    /// The page's ten-second poll, quiet: only while on the network, and
    /// never on top of a call already queued or running.
    pub(crate) fn tick(&mut self) {
        if self.enabled()
            && !self.in_flight
            && self.queued.is_none()
            && self.last_load.is_none_or(|t| t.elapsed() >= POLL_EVERY)
        {
            self.reload(false);
        }
    }
}

/// The room, loading: what `mstream-player admin discovery` opens.
pub(super) fn start(client: Client) -> Room {
    let mut room = Room::new(client);
    room.reload(true);
    room
}

// ── Keys ─────────────────────────────────────────────────────────────────────

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match &mut room.modal {
        Modal::Sheet { id, sel } => {
            let (id, cur) = (id.clone(), *sel);
            let menu = room.peer(&id).map(|p| room.sheet_menu(p)).unwrap_or_default();
            let n = menu.len();
            let move_to = |room: &mut Room, to: usize| {
                if let Modal::Sheet { sel, .. } = &mut room.modal {
                    *sel = to;
                }
            };
            return match code {
                KeyCode::Esc => room.act(Act::SheetClose),
                KeyCode::Up => {
                    move_to(room, cur.saturating_sub(1));
                    None
                }
                KeyCode::Down => {
                    move_to(room, (cur + 1).min(n.saturating_sub(1)));
                    None
                }
                KeyCode::Enter => room.act(Act::SheetRun),
                // The row's own letters work inside the sheet too.
                KeyCode::Char(c) => match menu.into_iter().find(|(k, _, _)| *k == c) {
                    Some((_, _, act)) => {
                        room.modal = Modal::None;
                        room.act(act)
                    }
                    None => None,
                },
                _ => None,
            };
        }
        Modal::Federate(c) => {
            let n = c.libraries.len();
            return match code {
                KeyCode::Esc => room.act(Act::ComposeCancel),
                KeyCode::Enter => room.act(Act::ComposeSend),
                KeyCode::Tab | KeyCode::Down => {
                    c.focus = match c.focus {
                        None if n > 0 => Some(0),
                        Some(i) if i + 1 < n => Some(i + 1),
                        _ => None,
                    };
                    None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    c.focus = match c.focus {
                        None if n > 0 => Some(n - 1),
                        Some(0) | None => None,
                        Some(i) => Some(i - 1),
                    };
                    None
                }
                KeyCode::Char(' ') if c.focus.is_some() => {
                    let i = c.focus.unwrap_or(0);
                    room.act(Act::ComposeToggle(i))
                }
                _ if c.focus.is_none() => {
                    // The message editor takes the rest, capped at the
                    // server's 500 characters.
                    if let KeyCode::Char(_) = code
                        && c.message.value().chars().count() >= MESSAGE_MAX
                    {
                        return None;
                    }
                    c.message.handle_event(&TermEvent::Key(key));
                    c.error = None;
                    None
                }
                _ => None,
            };
        }
        Modal::Identity(d) => {
            return match code {
                KeyCode::Esc => room.act(Act::IdentityCancel),
                KeyCode::Enter => room.act(Act::IdentitySave),
                KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => {
                    d.on_description = !d.on_description;
                    None
                }
                // The server's rules at the gate: no `|`, no control
                // characters, the field's length cap.
                KeyCode::Char('|') => None,
                KeyCode::Char(ch) if ch.is_control() => None,
                _ => {
                    let (field, max) = if d.on_description {
                        (&mut d.description, DESCRIPTION_MAX)
                    } else {
                        (&mut d.name, NAME_MAX)
                    };
                    if let KeyCode::Char(_) = code
                        && field.value().chars().count() >= max
                    {
                        return None;
                    }
                    field.handle_event(&TermEvent::Key(key));
                    d.error = None;
                    None
                }
            };
        }
        Modal::Setting(d) => {
            return match code {
                KeyCode::Esc => room.act(Act::SettingCancel),
                KeyCode::Enter => room.act(Act::SettingSave),
                KeyCode::Char(ch) if !ch.is_ascii_digit() => None,
                KeyCode::Char(_) if d.value.value().len() >= 6 => None,
                _ => {
                    d.value.handle_event(&TermEvent::Key(key));
                    d.error = None;
                    None
                }
            };
        }
        Modal::Block(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::BlockConfirm),
                // Enter is the SAFE choice on a warning gate.
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::BlockCancel),
                _ => None,
            };
        }
        Modal::Leave => {
            return match code {
                KeyCode::Char('y') => room.act(Act::LeaveConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::LeaveCancel),
                _ => None,
            };
        }
        Modal::Blocked(sel) => {
            let n = room.status.as_ref().map(|s| s.blocked_peers.len()).unwrap_or(0);
            return match code {
                KeyCode::Esc => room.act(Act::BlockedClose),
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    *sel = (*sel + 1).min(n.saturating_sub(1));
                    None
                }
                KeyCode::Enter => room.act(Act::BlockedRun),
                _ => None,
            };
        }
        Modal::None => {}
    }

    // Off the network: the join page has three keys.
    if !room.enabled() {
        return match code {
            KeyCode::Enter => room.act(Act::Join),
            KeyCode::Char(' ') => room.act(Act::ToggleRequests),
            KeyCode::Esc | KeyCode::Char('q') => room.act(Act::Quit),
            _ => None,
        };
    }

    // An inline field with focus takes the keys; Esc gives them back.
    if room.ticket_focus {
        return match code {
            KeyCode::Esc => {
                room.ticket_focus = false;
                None
            }
            KeyCode::Enter => room.act(Act::TicketJoin),
            _ => {
                room.ticket.handle_event(&TermEvent::Key(key));
                None
            }
        };
    }
    if room.filter_focus {
        return match code {
            KeyCode::Esc => {
                room.filter = Input::default();
                room.filter_on = false;
                room.filter_focus = false;
                None
            }
            KeyCode::Enter => {
                room.filter_focus = false;
                None
            }
            _ => {
                room.filter.handle_event(&TermEvent::Key(key));
                room.sel = None;
                None
            }
        };
    }

    let n = room.rows().len();
    let sel_id = room.selected_peer().map(|p| p.from.clone());
    match code {
        // ↑/↓ are the ONLY way a row gets highlighted: the first press
        // picks up the cursor (↓ from the top, ↑ from the bottom), Esc
        // puts it away again — and, with nothing selected, leaves.
        KeyCode::Up => {
            if n > 0 {
                room.sel = Some(room.sel.map_or(n - 1, |s| s.saturating_sub(1)));
            }
            None
        }
        KeyCode::Down => {
            if n > 0 {
                room.sel = Some(room.sel.map_or(0, |s| (s + 1).min(n - 1)));
            }
            None
        }
        KeyCode::Left => {
            room.switch_tab(room.tab.next(false));
            None
        }
        KeyCode::Right => {
            room.switch_tab(room.tab.next(true));
            None
        }
        KeyCode::Esc => {
            if room.sel.is_some() {
                room.sel = None;
                None
            } else if room.filter_on {
                room.filter = Input::default();
                room.filter_on = false;
                None
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Enter => room.sel.and_then(|s| room.act(Act::OpenSheet(s))),
        KeyCode::Char('d') => sel_id.and_then(|id| room.act(Act::Fetch(id))),
        KeyCode::Char('p') => {
            let pinned = room.selected_peer().and_then(|p| p.fetched.as_ref()).map(|h| h.pinned);
            match (sel_id, pinned) {
                (Some(id), Some(pinned)) => room.act(Act::Pin(id, !pinned)),
                _ => None,
            }
        }
        KeyCode::Char('r') | KeyCode::Delete => {
            sel_id.and_then(|id| room.act(Act::RemoveSnapshot(id)))
        }
        KeyCode::Char('f') => sel_id.and_then(|id| room.act(Act::Federate(id))),
        KeyCode::Char('g') => sel_id.and_then(|id| room.act(Act::Forget(id))),
        KeyCode::Char('b') => sel_id.and_then(|id| room.act(Act::Block(id))),
        KeyCode::Char('e') => room.act(Act::IdentityOpen),
        KeyCode::Char('x') => room.act(Act::Leave),
        KeyCode::Char('/') => room.act(Act::FilterOpen),
        KeyCode::Char('h') => room.act(Act::ToggleIncompatible),
        KeyCode::Char('u') => room.act(Act::BlockedOpen),
        KeyCode::Char('y') => room.act(Act::CopyTicket),
        KeyCode::Char('j') => room.act(Act::TicketFocus),
        KeyCode::Char('1') => room.act(Act::SettingOpen(DiscoverySetting::MaxStorageMb)),
        KeyCode::Char('2') => room.act(Act::SettingOpen(DiscoverySetting::PeerRetentionDays)),
        KeyCode::Char('3') => room.act(Act::SettingOpen(DiscoverySetting::AutoFetchCount)),
        KeyCode::Char('4') => room.act(Act::SettingOpen(DiscoverySetting::RotationDays)),
        KeyCode::Char('5') => room.act(Act::SettingOpen(DiscoverySetting::SidecarMaxRssMb)),
        KeyCode::Char('q') => room.act(Act::Quit),
        _ => None,
    }
}

// ── The tips ─────────────────────────────────────────────────────────────────

fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Sheet { .. } => t!("p2p.hint_sheet"),
        Modal::Federate(_) => t!("p2p.hint_compose"),
        Modal::Identity(_) => t!("p2p.hint_identity"),
        Modal::Setting(_) => t!("p2p.hint_setting"),
        Modal::Block(_) => t!("p2p.hint_block"),
        Modal::Leave => t!("p2p.hint_leave"),
        Modal::Blocked(_) => t!("p2p.hint_blocked"),
        Modal::None if room.status.is_none() => t!("p2p.hint_loading"),
        Modal::None if !room.enabled() => t!("p2p.hint_off"),
        Modal::None if room.ticket_focus => t!("p2p.hint_ticket"),
        Modal::None if room.filter_focus => t!("p2p.hint_filter"),
        Modal::None => match (room.sel, room.tab, room.rows().is_empty()) {
            (Some(_), _, _) => t!("p2p.hint_selected"),
            (None, Tab::Config, _) => t!("p2p.hint_config"),
            (None, _, true) => t!("p2p.hint_empty"),
            (None, _, false) => t!("p2p.hint_rows"),
        },
    }
    .to_string()
}

// ── Text helpers ─────────────────────────────────────────────────────────────

/// A catalog peer's name for the screen — remote text, so gated: nothing
/// that steers a terminal, one line, bounded. Unnamed peers show the
/// short endpoint id.
fn display_name(peer: &CatalogPeer) -> String {
    let name = printable(&peer.payload.name, NAME_MAX);
    if name.is_empty() { short_id(&peer.from) } else { name }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{CatalogStorage, DiscoveryRecovery, DiscoveryWatchdog, HeldSnapshot, PeerPayload};
    use crate::input::KeyModifiers;

    // The fixtures are the cells view's tests' too (cells.rs).

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    pub(super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    pub(super) fn room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    pub(super) fn hex(seed: &str) -> String {
        seed.repeat(8).chars().take(64).collect()
    }

    pub(super) fn status_on() -> DiscoveryStatus {
        DiscoveryStatus {
            enabled: true,
            binary_found: true,
            binary_fetchable: true,
            running: true,
            endpoint_id: Some(hex("51403e7b9c")),
            ticket: Some("nodeaa4qk3r7z2nlxwm5c6bdhpj2ev5gktr3q7hifobyq2j5ru4kiggizaaaqbxk3ewgk6wxjm".into()),
            joined: true,
            neighbors: 3,
            neighbor_ids: Vec::new(),
            watchdog: DiscoveryWatchdog { last_rss_mb: Some(19.4), restarts: 0, max_rss_mb: 256 },
            recovery: DiscoveryRecovery::default(),
            known_peers: 6,
            community_seeds: true,
            server_name: "mStream Den".into(),
            server_description: "Vinyl, jazz and field recordings".into(),
            max_peer_db_storage_mb: 500,
            auto_fetch_count: 6,
            rotation_days: 7,
            peer_retention_days: 30,
            blocked_peers: vec![hex("a1b2c3d4e5")],
        }
    }

    pub(super) fn status_off() -> DiscoveryStatus {
        DiscoveryStatus { enabled: false, binary_found: false, binary_fetchable: true, ..status_on() }
    }

    pub(super) fn peer(
        id: &str,
        name: &str,
        description: &str,
        tracks: u64,
        ago: i64,
        seeders: u32,
        held: Option<(bool, bool)>,
    ) -> CatalogPeer {
        let now = unix_now();
        CatalogPeer {
            from: hex(id),
            payload: PeerPayload {
                name: name.into(),
                description: description.into(),
                row_count: tracks,
                snapshot_seq: 41,
                model_id: "effnet-discogs".into(),
            },
            updated_at: iso_at(now - ago),
            online: ago < 90,
            seeders,
            fetched: held.map(|(stale, pinned)| HeldSnapshot {
                snapshot_seq: 40,
                stale,
                size_bytes: 8 * 1_048_576,
                fetched_at: iso_at(now - 2 * 86_400),
                first_fetched_at: iso_at(now - 20 * 86_400),
                pinned,
            }),
            compatible: Some(true),
        }
    }

    pub(super) fn catalog() -> DiscoveryCatalog {
        DiscoveryCatalog {
            peers: vec![
                peer("8f31c0e2a7", "Basement Archive", "Northern soul, dub and a wall of 7-inch singles.", 12_204, 12, 3, Some((false, false))),
                peer("2b7d9e4c11", "Vinyl Rips '94", "", 9_871, 30, 2, Some((false, true))),
                peer("c4a8f0e19d", "friend-node", "Ambient and modern classical, updated weekly.", 2_494, 45, 1, Some((true, false))),
                peer("7e11aa93b0", "jazz-corner", "Hard bop through spiritual jazz.", 6_010, 3 * 86_400, 1, None),
                peer("05d6e8f2ac", "mStream", "", 412, 12 * 86_400, 0, None),
            ],
            hidden_incompatible: 1,
            local_model_id: Some("effnet-discogs".into()),
            auto_fetch: true,
            storage: CatalogStorage { used_bytes: 23 * 1_048_576, cap_bytes: 500 * 1_048_576 },
        }
    }

    pub(super) fn request(id: &str, direction: &str, state: &str) -> FederationRequest {
        FederationRequest {
            id: 1,
            peer_endpoint_id: hex(id),
            direction: direction.into(),
            state: state.into(),
            ..Default::default()
        }
    }

    pub(super) fn loaded(status: DiscoveryStatus) -> Done {
        let on = status.enabled;
        Done::Loaded(Ok(Box::new(Loaded {
            status,
            catalog: on.then(catalog),
            federation: on.then(|| FederationParams {
                enabled: true,
                available: true,
                accept_requests: true,
                ..Default::default()
            }),
            requests: on.then(|| {
                vec![
                    request("8f31c0e2a7", "out", "completed"),
                    request("c4a8f0e19d", "in", "received"),
                    request("05d6e8f2ac", "out", "delivered"),
                ]
            }),
            activity: None,
        })))
    }

    pub(super) fn on() -> Room {
        let mut room = room();
        room.apply(loaded(status_on()));
        room
    }

    pub(super) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(super) fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            handle_key(room, key(KeyCode::Char(c)));
        }
    }

    pub(super) fn room_after_join() -> Room {
        let mut room = room();
        room.apply(loaded(status_off()));
        handle_key(&mut room, key(KeyCode::Enter));
        room.queued = None;
        room
    }

    #[test]
    fn the_identity_modal_saves_only_what_changed_and_never_a_blank_name() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('e')));
        assert!(matches!(room.modal, Modal::Identity(_)));
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "nothing changed, nothing sent");
        handle_key(&mut room, key(KeyCode::Char('e')));
        type_text(&mut room, " two|");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(
            room.queued,
            Some(Op::Identity { name: Some("mStream Den two".into()), description: None }),
            "the pipe never reaches the field"
        );
        room.queued = None;
        room.apply(Done::Identity(Ok(())));
        assert!(matches!(room.modal, Modal::None));
        assert!(matches!(room.queued, Some(Op::Load { .. })), "a saved identity reloads");
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('e')));
        for _ in 0.."mStream Den".len() {
            handle_key(&mut room, key(KeyCode::Backspace));
        }
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Identity(d) = &room.modal else { panic!("the identity modal") };
        assert_eq!(d.error.as_deref(), Some("the server name must not be blank"));
        handle_key(&mut room, key(KeyCode::Tab));
        type_text(&mut room, " and more");
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Identity(d) = &room.modal else { panic!("the identity modal") };
        assert!(d.error.is_some(), "a blank name is refused whatever the description");
        assert!(room.queued.is_none());
    }

    #[test]
    fn the_poll_is_quiet_and_only_while_on_the_network() {
        let _en = english();
        let mut room = room();
        room.tick();
        assert!(room.queued.is_none(), "nothing before the first load answers");
        room.apply(loaded(status_off()));
        room.tick();
        assert!(room.queued.is_none(), "off the network, no poll");
        room.apply(loaded(status_on()));
        room.tick();
        assert_eq!(room.queued, Some(Op::Load { include_incompatible: false, activity_since: None }));
        assert!(room.busy.is_none(), "the poll shows no busy line");
        room.last_load = Some(Instant::now());
        room.queued = None;
        room.tick();
        assert!(room.queued.is_none(), "not again within the cadence");
    }

    #[test]
    fn time_and_number_helpers() {
        let _en = english();
        assert_eq!(iso_unix("2026-09-06T09:41:52Z"), Some(1_788_687_712));
        assert_eq!(iso_unix("2026-09-06T09:41:52.123Z"), Some(1_788_687_712));
        assert_eq!(iso_unix("yesterday"), None);
        for t in [0, 951_782_400, 1_788_687_712, 4_102_444_800] {
            assert_eq!(iso_unix(&iso_at(t)), Some(t), "{t}");
        }
        assert_eq!(age_text(30), "now");
        assert_eq!(age_text(120), "2m");
        assert_eq!(age_text(3 * 3600), "3h");
        assert_eq!(age_text(47 * 3600), "47h");
        assert_eq!(age_text(3 * 86_400), "3d");
        assert_eq!(fmt_count(12_204), "12,204");
        assert_eq!(fmt_count(412), "412");
        assert_eq!(fmt_count(1_000_000), "1,000,000");
        assert_eq!(fmt_bytes(23 * 1_048_576), "23.0 MB");
        assert_eq!(fmt_bytes(3 * 1_073_741_824), "3.0 GB");
        assert_eq!(fmt_bytes(1_536), "2 KB");
        assert_eq!(short_id(&hex("8f31c0e2a7")), "8f31c0e2a78f…");
        assert_eq!(printable("bad\u{1b}[31mname\u{202E}", 10), "bad[31mnam");
    }

    #[test]
    fn a_load_asks_past_the_switch_in_one_wave() {
        // The status first — while it is off nothing else is asked — then
        // the catalog, federation, the requests and the activity together:
        // two round trips, not five (performance audit #83).
        let answer = |catalog: u16| {
            move |path: &str| match path.split('?').next().unwrap_or_default() {
                "/api/v1/admin/discovery/p2p/status" => (200, r#"{"enabled":true}"#.to_string()),
                "/api/v1/admin/discovery/p2p/catalog" => (catalog, "{}".to_string()),
                "/api/v1/admin/federation/requests" => (500, "{}".to_string()),
                _ => (200, "{}".to_string()),
            }
        };
        let run = |url: &str| {
            let (to_worker, from_worker) = spawn_worker();
            let op = Op::Load { include_incompatible: false, activity_since: Some(0) };
            to_worker.send((Arc::new(Client::new(url).expect("client")), op)).unwrap();
            let Done::Loaded(result) = from_worker.recv_timeout(Duration::from_secs(30)).expect("an answer") else {
                panic!("a load");
            };
            result
        };
        let server = super::super::waves::serve(&[1, 4], answer(200));
        let loaded = run(&server.url).expect("a load");
        let paths = server.paths();
        assert_eq!((server.peak(), paths.len()), (4, 5), "{paths:?}");
        assert_eq!(paths[0], "/api/v1/admin/discovery/p2p/status");
        // The requests are best-effort; the catalog is not.
        assert!(loaded.catalog.is_some() && loaded.activity.is_some() && loaded.requests.is_none());
        let server = super::super::waves::serve(&[1, 4], answer(500));
        assert!(run(&server.url).is_err());
    }

    // ── Hosted in the GUI player's Admin tab ─────────────────────────────

    #[test]
    fn it_claims_every_key_while_filtering_or_befriending() {
        let _en = english();
        let mut room = on();
        assert_eq!(room.claim(), Claim::Open);
        handle_key(&mut room, key(KeyCode::Char('/')));
        assert_eq!(room.claim(), Claim::All, "the filter has the caret");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.claim(), Claim::Open);
        handle_key(&mut room, key(KeyCode::Char('j')));
        assert!(room.ticket_focus);
        assert_eq!(room.claim(), Claim::All, "the befriend box has the caret");
        handle_key(&mut room, key(KeyCode::Char('L')));
        assert_eq!(room.ticket.value(), "L", "a capital L is a letter of the ticket");
        handle_key(&mut room, key(KeyCode::Esc));
        assert_eq!(room.claim(), Claim::Open);
        room.modal = Modal::Leave;
        assert!(room.modal_open());
        assert_eq!(room.claim(), Claim::All);
        assert_eq!(room.hint(), t!("p2p.hint_leave"));
    }

    /// `y` hands the invite's ticket to the kit's clipboard and the note
    /// says which way it went: on the clipboard, handed to the terminal
    /// (which may refuse it), or nowhere.
    #[test]
    fn y_on_the_invite_copies_the_ticket_and_says_how() {
        use crate::kit::clipboard::{Copied, catch, caught};
        let _en = english();
        let mut room = on();
        room.switch_tab(Tab::Invite);
        let ticket = room.status.as_ref().and_then(|s| s.ticket.clone()).expect("a ticket");
        for (answer, said) in [
            (Copied::Clipboard, t!("p2p.copied_clipboard")),
            (Copied::Terminal, t!("p2p.copied")),
            (Copied::Failed, t!("p2p.copy_failed")),
        ] {
            catch(answer);
            room.note = None;
            handle_key(&mut room, key(KeyCode::Char('y')));
            assert_eq!(room.note, Some((said.to_string(), false)), "{answer:?}");
        }
        assert_eq!(caught(), [ticket.clone(), ticket.clone(), ticket]);
    }
}
