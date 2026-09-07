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

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{
    Outcome, Screen, age_text, copy_to_clipboard, draw_bottom, draw_header, fmt_bytes, fmt_count,
    frame_ground, gate_message, host_of, iso_unix, printable, short_id, unix_now,
};
#[cfg(test)]
use super::iso_at;
use crate::api::types::{
    ActivityEntry, CatalogPeer, DiscoveryActivity, DiscoveryCatalog, DiscoveryStatus,
    FederationParams, FederationRequest,
};
use crate::api::{ApiError, Client, DiscoverySetting, PeerAction};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::g;

/// The page's own cadence: the mesh weaves in over a minute, and nobody
/// should have to mash a refresh key to watch it.
const POLL_EVERY: Duration = Duration::from_secs(10);
/// The tab pane: eight rows under the tabs row, whatever the tab.
const PANE_ROWS: u16 = 8;
const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
/// The server's limits on the announced identity (and no `|` — the
/// announcement's signing separator).
const NAME_MAX: usize = 64;
const DESCRIPTION_MAX: usize = 180;
const MESSAGE_MAX: usize = 500;
/// A held-snapshot count is "N of M" only while auto-fetch has a target.
const TAB_GAP: u16 = 2;

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
                    Done::Loaded(client.admin_discovery_status().and_then(|status| {
                        if !status.enabled {
                            return Ok(Box::new(Loaded {
                                status,
                                catalog: None,
                                federation: None,
                                requests: None,
                                activity: None,
                            }));
                        }
                        let catalog = client.admin_discovery_catalog(include_incompatible)?;
                        // Best-effort: the relationship column just stays
                        // blank when federation cannot answer.
                        let federation = client.admin_federation().ok();
                        let requests =
                            client.admin_federation_requests().ok().map(|r| r.requests);
                        let activity = activity_since
                            .and_then(|since| client.admin_discovery_activity(since).ok());
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
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    /// After a join, the next load that shows the network on opens the
    /// identity modal — 'mStream' beside a thousand 'mStream's is the
    /// first thing everyone renames.
    name_after_join: bool,
    ui: Surface<Act>,
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
            sel_anchor: None,
            last_load: None,
            name_after_join: false,
            ui: Surface::new(),
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
        let q = if self.filter_on { self.filter.value().trim().to_lowercase() } else { String::new() };
        self.catalog
            .peers
            .iter()
            .filter(|p| {
                q.is_empty()
                    || p.payload.name.to_lowercase().contains(&q)
                    || p.payload.description.to_lowercase().contains(&q)
                    || p.from.to_lowercase().contains(&q)
            })
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

    fn act(&mut self, act: Act) -> Option<Outcome> {
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
                    let ok = copy_to_clipboard(&ticket);
                    self.note = Some((
                        if ok { t!("p2p.copied") } else { t!("p2p.copy_failed") }.to_string(),
                        false,
                    ));
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
}

// ── The hub's view of the room ───────────────────────────────────────────────

impl Screen for Room {
    type Act = Act;

    fn ui(&mut self) -> &mut Surface<Act> {
        &mut self.ui
    }

    fn pump(&mut self) {
        loop {
            match self.from_worker.try_recv() {
                Ok(done) => self.apply(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.note = Some((t!("note.worker_gone").to_string(), true));
                    break;
                }
            }
        }
        self.dispatch_queued();
    }

    /// The page's ten-second poll, quiet: only while on the network, and
    /// never on top of a call already queued or running.
    fn tick(&mut self) {
        if self.enabled()
            && !self.in_flight
            && self.queued.is_none()
            && self.last_load.is_none_or(|t| t.elapsed() >= POLL_EVERY)
        {
            self.reload(false);
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        render(frame, self)
    }

    fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    fn act(&mut self, act: Act) -> Option<Outcome> {
        Room::act(self, act)
    }

    fn wheel(&mut self, up: bool, at: Position) {
        if !matches!(self.modal, Modal::None) {
            return;
        }
        // Over the Activity pane the wheel walks the log; anywhere else it
        // is the table's.
        if self.tab == Tab::Activity && (6..6 + PANE_ROWS).contains(&at.y) {
            self.ascroll = if up { self.ascroll.saturating_add(1) } else { self.ascroll.saturating_sub(1) };
        } else {
            self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
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

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.ui.begin_frame();
    let Some(area) = frame_ground(frame, MIN_W, MIN_H) else { return };

    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.ui.pointer;
    if modal_open {
        room.ui.pointer = None;
    }

    draw_header(frame, area, &t!("p2p.title"), &host_of(&room.client));
    let column = Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(5) };
    match room.status.clone() {
        None => {}
        Some(s) if !s.enabled => draw_off(frame, room, column, &s),
        Some(s) => draw_on(frame, room, column, &s),
    }
    draw_bottom(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Sheet { id, sel } => draw_sheet(frame, room, area, &id, sel),
        Modal::Federate(c) => draw_compose(frame, room, area, &c),
        Modal::Identity(d) => draw_identity(frame, room, area, &d),
        Modal::Setting(d) => draw_setting(frame, room, area, &d),
        Modal::Block(id) => draw_block(frame, room, area, &id),
        Modal::Leave => draw_leave(frame, room, area),
        Modal::Blocked(sel) => draw_blocked(frame, room, area, sel),
    }
    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

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

/// Off the network: the state line, the join card, the opt-in, the pitch.
fn draw_off(frame: &mut Frame, room: &mut Room, column: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let unavailable = !s.binary_found && !s.binary_fetchable;
    if unavailable {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.state_unavailable").to_string(), Style::default().fg(th().gold))),
            line(column.y),
        );
    } else {
        let mut state = t!("p2p.state_off").to_string();
        if !s.binary_found && s.binary_fetchable {
            state.push_str(" · ");
            state.push_str(&t!("p2p.off_will_download"));
        }
        frame.render_widget(Paragraph::new(Span::styled(state, dim())), line(column.y));
    }
    let mut y = column.y + 2;

    if !unavailable {
        // The one affirmative action, the add-folder card's shape: green,
        // hover brightens, Enter or a click joins at once.
        let card = Rect { x: column.x, y, width: column.width, height: 3 };
        let hover = room.ui.pointer.is_some_and(|p| card.contains(p));
        let color = if hover { th().bright } else { th().ok };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
        let inner = block.inner(card);
        frame.render_widget(block, card);
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.off_join").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
                .alignment(Alignment::Center),
            inner,
        );
        room.ui.click(card, Act::Join);
        room.ui.tip(card, if s.binary_found { t!("p2p.tip_join") } else { t!("p2p.tip_join_download") });
        y += 4;

        // The opt-in: the kit's checkbox card, on by default. Space and a
        // click flip it; the border hovers like every clickable.
        let opt = Rect { x: column.x, y, width: column.width, height: 4 };
        let hover = room.ui.pointer.is_some_and(|p| opt.contains(p));
        let border = if hover { th().bright } else { th().dim };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
        let inner = block.inner(opt);
        frame.render_widget(block, opt);
        let glyph = if room.accept_requests {
            Span::styled(format!("{} ", g("[✓]", "[x]")), Style::default().fg(th().ok))
        } else {
            Span::styled("[ ] ", dim())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![glyph, Span::styled(t!("p2p.off_requests").to_string(), bold())])),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(1), height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.off_requests_hint").to_string(), dim())),
            Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
        );
        room.ui.click(opt, Act::ToggleRequests);
        room.ui.tip(opt, t!("p2p.tip_requests"));
        y += 5;
    }

    // The pitch — the webapp's words, wrapped to the column.
    let lines = vec![
        Line::from(t!("p2p.off_pitch_1").to_string()),
        Line::from(""),
        Line::from(Span::styled(t!("p2p.off_shared").to_string(), dim().add_modifier(Modifier::BOLD))),
        Line::from(vec![
            Span::raw(t!("p2p.off_pitch_2a").to_string()),
            Span::styled(t!("p2p.off_pitch_2b").to_string(), bold()),
            Span::raw(t!("p2p.off_pitch_2c").to_string()),
        ]),
        Line::from(""),
        Line::from(t!("p2p.off_pitch_3").to_string()),
    ];
    let body = Rect { x: column.x, y, width: column.width, height: column.bottom().saturating_sub(y) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// On the network: the state row, the tabs and their pane, the table.
fn draw_on(frame: &mut Frame, room: &mut Room, column: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };

    // The state row: the dot in its color, then the facts.
    let recovering = (s.recovery.attempts > 0 || s.recovery.retry_pending) && s.neighbors == 0;
    let (word, color, detail) = if !s.binary_found {
        (t!("p2p.state_unavailable").to_string(), th().gold, String::new())
    } else if s.neighbors > 0 {
        let d = if s.neighbors == 1 { t!("p2p.state_neighbors_one") } else { t!("p2p.state_neighbors", n = s.neighbors) };
        (t!("p2p.state_connected").to_string(), th().ok, d.to_string())
    } else if recovering {
        (t!("p2p.state_reconnecting").to_string(), th().gold, t!("p2p.state_reconnecting_detail", n = s.recovery.attempts).to_string())
    } else if s.joined {
        (t!("p2p.state_waiting").to_string(), th().gold, String::new())
    } else {
        (t!("p2p.state_not_joined").to_string(), th().dim, String::new())
    };
    // The facts after the state are optional, in reverse order of loss: a
    // narrow window drops the announced name first (Config has it), then
    // the sidecar figure — the state itself is never clipped.
    let mut groups: Vec<Vec<Span>> = vec![vec![
        Span::styled(word, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw(detail),
    ]];
    if let Some(mb) = s.watchdog.last_rss_mb {
        let mut sidecar = vec![Span::raw(t!("p2p.state_sidecar", mb = format!("{mb:.0}")).to_string())];
        if s.watchdog.restarts > 0 {
            let r = if s.watchdog.restarts == 1 { t!("p2p.state_restart_one") } else { t!("p2p.state_restarts", n = s.watchdog.restarts) };
            sidecar.push(Span::styled(r.to_string(), Style::default().fg(th().gold)));
        }
        groups.push(sidecar);
    }
    groups.push(vec![
        Span::raw(t!("p2p.state_announcing").to_string()),
        Span::styled(printable(&s.server_name, NAME_MAX), bold()),
    ]);
    let width_of = |groups: &[Vec<Span>]| -> usize {
        groups.iter().flatten().map(|sp| sp.content.chars().count()).sum()
    };
    while groups.len() > 1 && width_of(&groups) > column.width as usize {
        groups.pop();
    }
    let state_w = width_of(&groups);
    frame.render_widget(Paragraph::new(Line::from(groups.concat())), line(column.y));
    let polls = t!("p2p.polls").to_string();
    if state_w + 2 + polls.chars().count() <= column.width as usize {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line(column.y));
    }
    // The indeterminate "something is happening" bar: the feature is on
    // but the mesh has no neighbor yet — the kit's all-dim bar, never a
    // fake percentage.
    if s.binary_found && (!s.running || !s.joined || s.neighbors == 0) {
        let text = if recovering { t!("p2p.recovering") } else { t!("p2p.searching") };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▱".repeat(10), dim()),
                Span::raw(" "),
                Span::styled(text.to_string(), accent()),
            ])),
            line(column.y + 1),
        );
    }

    // The tabs: the active one a filled slab, the rest text buttons.
    let tabs_y = column.y + 2;
    let mut x = column.x;
    for tab in Tab::ALL {
        let label = format!(" {} ", tab.label());
        let rect = Rect { x, y: tabs_y, width: label.chars().count() as u16, height: 1 };
        let hover = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if tab == room.tab {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        room.ui.click(rect, Act::Tab(tab));
        x = rect.right() + TAB_GAP;
    }
    if room.tab == Tab::Activity {
        let note = t!("p2p.activity_note").to_string();
        if x as usize + 2 + note.chars().count() <= column.right() as usize {
            frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
        }
    }

    let pane = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: PANE_ROWS };
    match room.tab {
        Tab::Stats => draw_stats(frame, room, pane, s),
        Tab::Activity => draw_activity(frame, room, pane),
        Tab::Invite => draw_invite(frame, room, pane, s),
        Tab::Config => draw_config(frame, room, pane, s),
    }

    let table = Rect { x: column.x, y: pane.bottom(), width: column.width, height: column.bottom().saturating_sub(pane.bottom()) };
    draw_servers(frame, room, table, s);
}

/// Six tiles in two rows of three: value BOLD, label and detail DIM.
fn draw_stats(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let held: Vec<&CatalogPeer> = room.catalog.peers.iter().filter(|p| p.fetched.is_some()).collect();
    let held_tracks: u64 = held.iter().map(|p| p.payload.row_count).sum();
    let hidden = if room.show_incompatible { 0 } else { room.catalog.hidden_incompatible };
    let known = room.catalog.peers.len() as u32 + hidden;
    let held_n = held.len().to_string();
    let held_value = if s.auto_fetch_count > 0 {
        t!("p2p.tile_held_of", held = held.len(), cap = s.auto_fetch_count).to_string()
    } else {
        held_n.clone()
    };
    let memory = match s.watchdog.last_rss_mb {
        Some(mb) => t!("p2p.value_mb", n = format!("{mb:.0}")).to_string(),
        None => "—".to_string(),
    };
    let memory_detail = if s.watchdog.max_rss_mb > 0 {
        t!("p2p.tile_memory_detail", max = s.watchdog.max_rss_mb, restarts = s.watchdog.restarts).to_string()
    } else {
        t!("p2p.tile_memory_off").to_string()
    };
    let tiles: [(String, String, String); 6] = [
        (s.neighbors.to_string(), t!("p2p.tile_neighbors").to_string(), t!("p2p.tile_neighbors_detail").to_string()),
        (
            known.to_string(),
            t!("p2p.tile_known").to_string(),
            t!("p2p.tile_known_detail", hidden = hidden, blocked = s.blocked_peers.len()).to_string(),
        ),
        (
            held_value,
            t!("p2p.tile_held").to_string(),
            t!("p2p.tile_held_detail", used = fmt_bytes(room.catalog.storage.used_bytes), cap = fmt_bytes(room.catalog.storage.cap_bytes)).to_string(),
        ),
        (fmt_count(held_tracks), t!("p2p.tile_tracks").to_string(), t!("p2p.tile_tracks_detail", n = held.len()).to_string()),
        (memory, t!("p2p.tile_memory").to_string(), memory_detail),
        (held_n, t!("p2p.tile_seeding").to_string(), t!("p2p.tile_seeding_detail").to_string()),
    ];
    let tile_w = pane.width / 3;
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let x = pane.x + (i as u16 % 3) * tile_w;
        let y = pane.y + (i as u16 / 3) * 4;
        let row = |dy: u16| Rect { x, y: y + dy, width: tile_w.saturating_sub(1), height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(value.clone(), bold())), row(0));
        frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), row(1));
        frame.render_widget(Paragraph::new(Span::styled(detail.clone(), dim())), row(2));
    }
}

/// The log, newest first, level as color: warn and error gold, debug dim.
fn draw_activity(frame: &mut Frame, room: &mut Room, pane: Rect) {
    if room.activity.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("p2p.activity_empty").to_string(), dim())),
            Rect { x: pane.x, y: pane.y, width: pane.width, height: 1 },
        );
        return;
    }
    let rows = pane.height as usize;
    let max_scroll = room.activity.len().saturating_sub(rows);
    room.ascroll = room.ascroll.min(max_scroll);
    let newest = room.activity.len() - room.ascroll;
    let first = newest.saturating_sub(rows);
    let now = unix_now();
    for (row, entry) in room.activity[first..newest].iter().rev().enumerate() {
        let style = match entry.level.as_str() {
            "warn" | "error" => Style::default().fg(th().gold),
            "debug" => dim(),
            _ => Style::default(),
        };
        let when = iso_unix(&entry.t).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{when:>9}  "), dim()),
                Span::styled(printable(&entry.message, 400), style),
            ])),
            Rect { x: pane.x, y: pane.y + row as u16, width: pane.width, height: 1 },
        );
    }
}

/// Endpoint, the ticket whole, and the befriend box — the kit's inline
/// text input, focused by click or `j`.
fn draw_invite(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: pane.x, y, width: pane.width, height: 1 };
    let label_w = 11u16;
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.invite_endpoint").to_string(), dim())), line(pane.y));
    frame.render_widget(
        Paragraph::new(Span::raw(match &s.endpoint_id {
            Some(id) => printable(id, 64),
            None => t!("p2p.invite_no_endpoint").to_string(),
        })),
        Rect { x: pane.x + label_w, y: pane.y, width: pane.width.saturating_sub(label_w), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("p2p.invite_ticket").to_string(), dim()),
            Span::styled(t!("p2p.invite_ticket_hint").to_string(), dim()),
        ])),
        line(pane.y + 1),
    );
    if let Some(ticket) = &s.ticket {
        let copy = t!("p2p.invite_copy").to_string();
        let w = copy.chars().count() as u16 + 4;
        let rect = kit::button(
            frame,
            &mut room.ui,
            Rect { x: pane.right().saturating_sub(w), y: pane.y + 1, width: w, height: 1 },
            &copy,
            false,
            Act::CopyTicket,
        );
        room.ui.tip(rect, t!("p2p.tip_copy"));
        frame.render_widget(
            Paragraph::new(printable(ticket, 4096)).wrap(Wrap { trim: false }),
            Rect { x: pane.x, y: pane.y + 2, width: pane.width, height: 2 },
        );
    } else {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.invite_no_endpoint").to_string(), dim())), line(pane.y + 2));
    }
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("p2p.invite_befriend").to_string(), dim()),
            Span::styled(t!("p2p.invite_befriend_hint").to_string(), dim()),
        ])),
        line(pane.y + 4),
    );
    let field = Rect { x: pane.x, y: pane.y + 5, width: pane.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if room.ticket_focus { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if room.ticket_focus {
        kit::input_display(room.ticket.value(), room.ticket.cursor(), inner.width.saturating_sub(2))
    } else {
        room.ticket.value().to_string()
    };
    frame.render_widget(
        Paragraph::new(Span::raw(shown)),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 },
    );
    room.ui.click(field, Act::TicketFocus);
}

/// The identity line, then the settings rows: key, label, value, hint.
fn draw_config(frame: &mut Frame, room: &mut Room, pane: Rect, s: &DiscoveryStatus) {
    let line = |y: u16| Rect { x: pane.x, y, width: pane.width, height: 1 };
    let mut spans = vec![
        Span::styled(t!("p2p.config_announcing").to_string(), dim()),
        Span::raw("  "),
        Span::styled(printable(&s.server_name, NAME_MAX), bold()),
    ];
    if !s.server_description.trim().is_empty() {
        spans.push(Span::raw(format!(" — {}", printable(&s.server_description, DESCRIPTION_MAX))));
    }
    let edit = t!("p2p.config_edit").to_string();
    let edit_w = edit.chars().count() as u16 + 4;
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect { x: pane.x, y: pane.y, width: pane.width.saturating_sub(edit_w + 1), height: 1 },
    );
    kit::button(
        frame,
        &mut room.ui,
        Rect { x: pane.right().saturating_sub(edit_w), y: pane.y, width: edit_w, height: 1 },
        &edit,
        false,
        Act::IdentityOpen,
    );

    let days = |n: u32| if n > 0 { t!("p2p.value_days", n = n).to_string() } else { t!("p2p.value_never").to_string() };
    let rows: [(char, String, String, String, Option<DiscoverySetting>); 6] = [
        ('1', t!("p2p.config_storage").to_string(), t!("p2p.value_mb", n = s.max_peer_db_storage_mb).to_string(), t!("p2p.config_storage_hint").to_string(), Some(DiscoverySetting::MaxStorageMb)),
        (
            '2',
            t!("p2p.config_retention").to_string(),
            if s.peer_retention_days > 0 { t!("p2p.value_days_silence", n = s.peer_retention_days).to_string() } else { t!("p2p.value_never").to_string() },
            t!("p2p.config_retention_hint").to_string(),
            Some(DiscoverySetting::PeerRetentionDays),
        ),
        (
            '3',
            t!("p2p.config_autofetch").to_string(),
            if !room.catalog.auto_fetch {
                t!("p2p.value_off").to_string()
            } else if s.auto_fetch_count == 0 {
                t!("p2p.value_paused").to_string()
            } else {
                t!("p2p.value_servers", n = s.auto_fetch_count).to_string()
            },
            t!("p2p.config_autofetch_hint").to_string(),
            Some(DiscoverySetting::AutoFetchCount),
        ),
        ('4', t!("p2p.config_rotation").to_string(), days(s.rotation_days), t!("p2p.config_rotation_hint").to_string(), Some(DiscoverySetting::RotationDays)),
        (
            '5',
            t!("p2p.config_memory").to_string(),
            if s.watchdog.max_rss_mb > 0 { t!("p2p.value_mb", n = s.watchdog.max_rss_mb).to_string() } else { t!("p2p.tile_memory_off").to_string() },
            t!("p2p.config_memory_hint").to_string(),
            Some(DiscoverySetting::SidecarMaxRssMb),
        ),
        (' ', t!("p2p.config_seeds").to_string(), if s.community_seeds { t!("p2p.value_on") } else { t!("p2p.value_off") }.to_string(), t!("p2p.config_seeds_hint").to_string(), None),
    ];
    let label_x = pane.x + 3;
    let value_x = pane.x + 27;
    let hint_x = pane.x + 53;
    for (i, (key, label, value, hint, setting)) in rows.iter().enumerate() {
        let y = pane.y + 2 + i as u16;
        let rect = line(y);
        let hover = setting.is_some() && room.ui.pointer.is_some_and(|p| rect.contains(p));
        let key_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else { bold() };
        frame.render_widget(Paragraph::new(Span::styled(key.to_string(), key_style)), Rect { x: pane.x, y, width: 1, height: 1 });
        frame.render_widget(
            Paragraph::new(Span::styled(label.clone(), if hover { Style::default().fg(th().bright) } else { dim() })),
            Rect { x: label_x, y, width: value_x.saturating_sub(label_x + 1), height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(value.clone(), bold())),
            Rect { x: value_x, y, width: hint_x.saturating_sub(value_x + 1), height: 1 },
        );
        if pane.right() > hint_x + 4 {
            frame.render_widget(
                Paragraph::new(Span::styled(hint.clone(), dim())),
                Rect { x: hint_x, y, width: pane.right().saturating_sub(hint_x), height: 1 },
            );
        }
        if let Some(which) = setting {
            room.ui.click(rect, Act::SettingOpen(*which));
        }
    }
}

/// The catalog as a table: the section line (filter, hidden, blocked),
/// the header and rule, then the rows — Enter (or a click) opens a sheet.
fn draw_servers(frame: &mut Frame, room: &mut Room, table: Rect, s: &DiscoveryStatus) {
    if table.height < 4 {
        return;
    }
    let mut y = table.y;
    let line = |y: u16| Rect { x: table.x, y, width: table.width, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.section_servers").to_string(), dim().add_modifier(Modifier::BOLD))),
        line(y),
    );
    // The right side of the section line: the filter (live text while it
    // is on), the hidden-incompatible toggle and the blocked count.
    let mut parts: Vec<Span> = Vec::new();
    if room.filter_on {
        let shown = if room.filter_focus {
            kit::input_display(room.filter.value(), room.filter.cursor(), 30)
        } else {
            room.filter.value().to_string()
        };
        parts.push(Span::styled(format!("/ {shown}"), if room.filter_focus { accent() } else { Style::default() }));
    } else {
        parts.push(Span::styled(t!("p2p.filter_key").to_string(), dim()));
    }
    let hidden = room.catalog.hidden_incompatible;
    if hidden > 0 || room.show_incompatible {
        parts.push(Span::styled(" · ", dim()));
        if room.show_incompatible {
            parts.push(Span::styled(t!("p2p.hidden_showing").to_string(), dim()));
        } else if hidden == 1 {
            parts.push(Span::styled(t!("p2p.hidden_one").to_string(), dim()));
        } else {
            parts.push(Span::styled(t!("p2p.hidden_many", n = hidden).to_string(), dim()));
        }
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(if room.show_incompatible { t!("p2p.hidden_hide") } else { t!("p2p.hidden_show") }.to_string(), dim()));
    }
    if !s.blocked_peers.is_empty() {
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(t!("p2p.blocked_n", n = s.blocked_peers.len()).to_string(), dim()));
        parts.push(Span::styled(" · ", dim()));
        parts.push(Span::styled(t!("p2p.blocked_manage").to_string(), dim()));
    }
    let right_w: usize = parts.iter().map(|p| p.content.chars().count()).sum();
    let right = Rect {
        x: table.right().saturating_sub(right_w as u16),
        y,
        width: (right_w as u16).min(table.width),
        height: 1,
    };
    frame.render_widget(Paragraph::new(Line::from(parts)), right);
    room.ui.click(right, Act::FilterOpen);
    y += 1;

    // Columns: the fixed ones from the right, SERVER takes the rest —
    // and the narrowest windows give up seeders, then tracks.
    let (tracks_w, seeders_w, online_w, snap_w, fed_w) = (7u16, 7u16, 13u16, 19u16, 14u16);
    let mut show_seeders = true;
    let mut show_tracks = true;
    let fixed = |seeders: bool, tracks: bool| {
        online_w + 2 + snap_w + 2 + fed_w + if seeders { seeders_w + 2 } else { 0 } + if tracks { tracks_w + 2 } else { 0 }
    };
    if table.width.saturating_sub(fixed(true, true)) < 16 {
        show_seeders = false;
    }
    if table.width.saturating_sub(fixed(false, true)) < 16 {
        show_tracks = false;
    }
    let server_w = table.width.saturating_sub(fixed(show_seeders, show_tracks));
    let mut x = table.x + server_w;
    let tracks_x = x;
    if show_tracks {
        x += tracks_w + 2;
    }
    let seeders_x = x;
    if show_seeders {
        x += seeders_w + 2;
    }
    let online_x = x;
    let snap_x = online_x + online_w + 2;
    let fed_x = snap_x + snap_w + 2;

    let head = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_server").to_string(), dim())), head(table.x, server_w));
    if show_tracks {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_tracks").to_string(), dim())).alignment(Alignment::Right), head(tracks_x, tracks_w));
    }
    if show_seeders {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_seeders").to_string(), dim())).alignment(Alignment::Right), head(seeders_x, seeders_w));
    }
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_online").to_string(), dim())), head(online_x, online_w));
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_snapshot").to_string(), dim())), head(snap_x, snap_w));
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.col_federation").to_string(), dim())), head(fed_x, fed_w));
    y += 1;
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(table.width as usize), dim())), line(y));
    y += 1;

    let rows: Vec<CatalogPeer> = room.rows().into_iter().cloned().collect();
    if rows.is_empty() {
        let text = if room.filter_on && !room.catalog.peers.is_empty() { t!("p2p.empty_filter") } else { t!("p2p.empty_servers") };
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), line(y));
        return;
    }
    let avail = table.bottom().saturating_sub(y) as usize;
    let sel_moved = room.sel != room.sel_anchor;
    room.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(rows.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    let now = unix_now();
    for (i, peer) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = room.sel == Some(i);
        let rect = line(y);
        let hovered = !selected && room.ui.pointer.is_some_and(|p| rect.contains(p));
        let row_bg = if selected { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default() };
        frame.render_widget(Paragraph::new(Span::styled(" ".repeat(table.width as usize), row_bg)), rect);
        let base = if selected { row_bg } else if hovered { Style::default().fg(th().bright) } else { Style::default() };
        let faint = if selected { row_bg } else if hovered { Style::default().fg(th().bright) } else { dim() };
        let cell = |x: u16, w: u16| Rect { x, y, width: w, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(display_name(peer), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(table.x, server_w.saturating_sub(2)),
        );
        if show_tracks {
            frame.render_widget(Paragraph::new(Span::styled(fmt_count(peer.payload.row_count), base)).alignment(Alignment::Right), cell(tracks_x, tracks_w));
        }
        if show_seeders {
            frame.render_widget(Paragraph::new(Span::styled(peer.seeders.to_string(), base)).alignment(Alignment::Right), cell(seeders_x, seeders_w));
        }
        let online = if peer.online {
            (t!("p2p.online").to_string(), base)
        } else {
            let age = iso_unix(&peer.updated_at).map(|t| age_text(now - t));
            (
                match age {
                    Some(age) => t!("p2p.offline_age", age = age).to_string(),
                    None => t!("p2p.offline").to_string(),
                },
                faint,
            )
        };
        frame.render_widget(Paragraph::new(Span::styled(online.0, online.1)), cell(online_x, online_w));
        let snap = match &peer.fetched {
            None => ("—".to_string(), faint),
            Some(h) => (
                match (h.stale, h.pinned) {
                    (false, false) => t!("p2p.snap_downloaded"),
                    (false, true) => t!("p2p.snap_pinned"),
                    (true, false) => t!("p2p.snap_update"),
                    (true, true) => t!("p2p.snap_update_pinned"),
                }
                .to_string(),
                base,
            ),
        };
        frame.render_widget(Paragraph::new(Span::styled(snap.0, snap.1)), cell(snap_x, snap_w));
        let fed = match room.relation(&peer.from) {
            Relation::None => ("—".to_string(), faint),
            Relation::Federated => (t!("p2p.fed_federated").to_string(), if selected || hovered { base } else { Style::default().fg(th().ok) }),
            Relation::Theirs => (t!("p2p.fed_theirs").to_string(), if selected || hovered { base } else { Style::default().fg(th().gold) }),
            Relation::Sent => (t!("p2p.fed_sent").to_string(), faint),
        };
        frame.render_widget(Paragraph::new(Span::styled(fed.0, fed.1)), cell(fed_x, fed_w));
        room.ui.click(rect, Act::OpenSheet(i));
        if !peer.payload.description.trim().is_empty() {
            room.ui.tip(rect, printable(&peer.payload.description, DESCRIPTION_MAX));
        }
        y += 1;
    }
    // The selected row's description rides the note line, when nothing
    // else is on it.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(peer) = room.selected_peer()
        && !peer.payload.description.trim().is_empty()
    {
        let text = format!("{} · {}", display_name(peer), printable(&peer.payload.description, DESCRIPTION_MAX));
        frame.render_widget(
            Paragraph::new(Span::raw(text)),
            Rect { x: table.x, y: table.bottom() + 1, width: table.width, height: 1 },
        );
    }
    let bar = Rect { x: table.x + table.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.ui, bar, rows.len(), visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// Enter on a row: the server's facts, then its actions as a list picker.
fn draw_sheet(frame: &mut Frame, room: &mut Room, area: Rect, id: &str, sel: usize) {
    let Some(peer) = room.peer(id).cloned() else {
        room.modal = Modal::None;
        return;
    };
    let menu = room.sheet_menu(&peer);
    let height = 10 + menu.len() as u16;
    let inner = kit::modal_frame(frame, area, 68, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(display_name(&peer), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::SheetClose, t!("path_modal.tip_close"));
    frame.render_widget(
        Paragraph::new(Span::raw(printable(&peer.payload.description, DESCRIPTION_MAX))),
        line(inner.y + 1),
    );
    let now = unix_now();
    let label_w = 12u16;
    let fact = |frame: &mut Frame, y: u16, label: String, value: String, style: Style| {
        frame.render_widget(Paragraph::new(Span::styled(label, dim())), line(y));
        frame.render_widget(
            Paragraph::new(Span::styled(value, style)),
            Rect { x: inner.x + 1 + label_w, y, width: inner.width.saturating_sub(2 + label_w), height: 1 },
        );
    };
    let seen = iso_unix(&peer.updated_at).map(|t| t!("p2p.sheet_seen", age = age_text(now - t)).to_string()).unwrap_or_default();
    fact(frame, inner.y + 3, t!("p2p.sheet_endpoint").to_string(), format!("{}{seen}", short_id(&peer.from)), Style::default());
    fact(
        frame,
        inner.y + 4,
        t!("p2p.sheet_online").to_string(),
        format!(
            "{} · {}",
            if peer.online { t!("p2p.online") } else { t!("p2p.offline") },
            t!("p2p.sheet_tracks", tracks = fmt_count(peer.payload.row_count), seeders = peer.seeders)
        ),
        Style::default(),
    );
    let snapshot = match &peer.fetched {
        None => t!("p2p.sheet_snapshot_none").to_string(),
        Some(h) => {
            let age = iso_unix(&h.fetched_at).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string());
            let mut s = t!("p2p.sheet_snapshot_held", age = age, size = fmt_bytes(h.size_bytes)).to_string();
            if h.stale {
                s.push_str(&t!("p2p.sheet_snapshot_stale"));
            }
            if h.pinned {
                s.push_str(&t!("p2p.sheet_snapshot_pinned"));
            }
            s
        }
    };
    fact(frame, inner.y + 5, t!("p2p.sheet_snapshot").to_string(), snapshot, Style::default());
    let (fed, fed_style) = match room.relation(&peer.from) {
        Relation::None => ("—".to_string(), dim()),
        Relation::Federated => (t!("p2p.sheet_fed_federated").to_string(), Style::default().fg(th().ok)),
        Relation::Theirs => (t!("p2p.sheet_fed_theirs").to_string(), Style::default().fg(th().gold)),
        Relation::Sent => (t!("p2p.sheet_fed_sent").to_string(), Style::default()),
    };
    fact(frame, inner.y + 6, t!("p2p.sheet_federation").to_string(), fed, fed_style);

    let list_y = inner.y + 8;
    for (i, (key, label, _)) in menu.iter().enumerate() {
        let rect = line(list_y + i as u16);
        let selected = i == sel;
        let hovered = !selected && room.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {key}  "), style.add_modifier(Modifier::BOLD)),
                Span::styled(label.clone(), style),
            ]))
            .style(style),
            rect,
        );
        // A click on the cursor row runs it; elsewhere it moves the cursor.
        room.ui.click(rect, if selected { Act::SheetRun } else { Act::SheetPick(i) });
    }
}

/// The federate compose: message, the offer as checkboxes, Send.
fn draw_compose(frame: &mut Frame, room: &mut Room, area: Rect, c: &Compose) {
    let lib_rows = c.libraries.len().div_ceil(3).max(1) as u16;
    let inner = kit::modal_frame(frame, area, 68, 14 + lib_rows, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.compose_title", name = c.name).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::ComposeCancel, t!("path_modal.tip_close"));
    for (i, key) in ["p2p.compose_1", "p2p.compose_2", "p2p.compose_3"].iter().enumerate() {
        frame.render_widget(Paragraph::new(Span::styled(t!(*key).to_string(), dim())), line(inner.y + 1 + i as u16));
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.compose_message", n = c.message.value().chars().count()).to_string(), dim())),
        line(inner.y + 5),
    );
    let field = Rect { x: inner.x + 1, y: inner.y + 6, width: inner.width.saturating_sub(2), height: 3 };
    let focused = c.focus.is_none();
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let field_inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        kit::input_display(c.message.value(), c.message.cursor(), field_inner.width.saturating_sub(2))
    } else {
        c.message.value().to_string()
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: field_inner.x + 1, y: field_inner.y, width: field_inner.width.saturating_sub(2), height: 1 });
    room.ui.click(field, Act::ComposeFocusMessage);

    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_libraries").to_string(), dim())), line(inner.y + 10));
    let libs_y = inner.y + 11;
    if !c.loaded {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_loading").to_string(), dim())), line(libs_y));
    } else if c.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.compose_none").to_string(), dim())), line(libs_y));
    }
    let col_w = inner.width.saturating_sub(2) / 3;
    for (i, (name, on)) in c.libraries.iter().enumerate() {
        let rect = Rect { x: inner.x + 1 + (i as u16 % 3) * col_w, y: libs_y + i as u16 / 3, width: col_w.saturating_sub(1), height: 1 };
        let focused = c.focus == Some(i);
        let hovered = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph = if *on {
            Span::styled(format!("{} ", g("[✓]", "[x]")), if focused { Style::default().fg(th().on_accent).bg(th().accent) } else { Style::default().fg(th().ok) })
        } else {
            Span::styled("[ ] ", if focused { Style::default().fg(th().on_accent).bg(th().accent) } else { dim() })
        };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if focused { bold() } else { Style::default() };
        frame.render_widget(Paragraph::new(Line::from(vec![glyph, Span::styled(name.clone(), name_style)])), rect);
        room.ui.click(rect, Act::ComposeToggle(i));
    }
    if let Some(err) = &c.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            line(inner.bottom().saturating_sub(2)),
        );
    }
    let label = t!("p2p.compose_send").to_string();
    let x = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, c.loaded, Act::ComposeSend);
}

/// A labelled 3-row input inside a modal; returns the rect it took.
fn modal_field<A: Clone>(
    frame: &mut Frame,
    ui: &mut Surface<A>,
    at: Rect,
    label: &str,
    input: &Input,
    focused: bool,
    act: A,
) -> Rect {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        kit::input_display(input.value(), input.cursor(), inner.width.saturating_sub(2))
    } else {
        let w = inner.width.saturating_sub(2) as usize;
        let v = input.value();
        if v.chars().count() > w { format!("{}…", v.chars().take(w.saturating_sub(1)).collect::<String>()) } else { v.to_string() }
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    ui.click(field, act);
    Rect { x: at.x, y: at.y, width: at.width, height: 4 }
}

/// The identity modal: the two fields the network sees.
fn draw_identity(frame: &mut Frame, room: &mut Room, area: Rect, d: &IdentityDraft) {
    let inner = kit::modal_frame(frame, area, 68, 17, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.identity_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::IdentityCancel, t!("path_modal.tip_close"));
    let at = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 4 };
    modal_field(
        frame,
        &mut room.ui,
        at(inner.y + 2),
        &t!("p2p.identity_name", n = d.name.value().chars().count()),
        &d.name,
        !d.on_description,
        Act::IdentityFocus(false),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.identity_name_hint").to_string(), dim())), line(inner.y + 6));
    modal_field(
        frame,
        &mut room.ui,
        at(inner.y + 8),
        &t!("p2p.identity_description", n = d.description.value().chars().count()),
        &d.description,
        d.on_description,
        Act::IdentityFocus(true),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("p2p.identity_description_hint").to_string(), dim())), line(inner.y + 12));
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))), line(inner.y + 14));
    }
    let label = t!("p2p.identity_save").to_string();
    let x = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, true, Act::IdentitySave);
}

/// One numeric setting: its title, the webapp's helper text, one field.
fn draw_setting(frame: &mut Frame, room: &mut Room, area: Rect, d: &SettingDraft) {
    let (title, label, help) = match d.which {
        DiscoverySetting::MaxStorageMb => ("p2p.setting_storage_title", "p2p.setting_storage_label", "p2p.setting_storage_help"),
        DiscoverySetting::PeerRetentionDays => ("p2p.setting_retention_title", "p2p.setting_retention_label", "p2p.setting_retention_help"),
        DiscoverySetting::AutoFetchCount => ("p2p.setting_autofetch_title", "p2p.setting_autofetch_label", "p2p.setting_autofetch_help"),
        DiscoverySetting::RotationDays => ("p2p.setting_rotation_title", "p2p.setting_rotation_label", "p2p.setting_rotation_help"),
        DiscoverySetting::SidecarMaxRssMb => ("p2p.setting_memory_title", "p2p.setting_memory_label", "p2p.setting_memory_help"),
    };
    let inner = kit::modal_frame(frame, area, 68, 15, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!(title).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::SettingCancel, t!("path_modal.tip_close"));
    frame.render_widget(
        Paragraph::new(Span::styled(t!(help).to_string(), dim())).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y + 2, width: inner.width.saturating_sub(2), height: 4 },
    );
    modal_field(
        frame,
        &mut room.ui,
        Rect { x: inner.x + 1, y: inner.y + 7, width: 24.min(inner.width.saturating_sub(2)), height: 4 },
        &t!(label),
        &d.value,
        true,
        Act::SettingSave,
    );
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))), line(inner.y + 12));
    }
    let save = t!("p2p.setting_save").to_string();
    let x = inner.right().saturating_sub(save.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &save, true, Act::SettingSave);
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 6 + body.len() as u16, th().gold);
    let gold = Style::default().fg(th().gold);
    let mut lines = vec![Line::from(Span::styled(title, gold.add_modifier(Modifier::BOLD))), Line::from("")];
    lines.extend(body.into_iter().map(|b| Line::from(Span::styled(b, gold))));
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) },
    );
    let y = inner.bottom().saturating_sub(1);
    let go_w = go.0.chars().count() as u16 + 4;
    let safe_w = safe.0.chars().count() as u16 + 4;
    let safe_x = inner.right().saturating_sub(go_w + 2 + safe_w);
    let safe_rect = kit::button(frame, &mut room.ui, Rect { x: safe_x, y, width: inner.width, height: 1 }, &safe.0, true, safe.1);
    kit::button(frame, &mut room.ui, Rect { x: safe_rect.right() + 2, y, width: inner.width, height: 1 }, &go.0, false, go.1);
}

fn draw_block(frame: &mut Frame, room: &mut Room, area: Rect, id: &str) {
    let name = room.peer(id).map(display_name).unwrap_or_else(|| short_id(id));
    draw_gate(
        frame,
        room,
        area,
        t!("p2p.block_title", name = name).to_string(),
        vec![t!("p2p.block_1").to_string(), t!("p2p.block_2").to_string()],
        (t!("p2p.block_keep").to_string(), Act::BlockCancel),
        (t!("p2p.block_confirm").to_string(), Act::BlockConfirm),
    );
}

fn draw_leave(frame: &mut Frame, room: &mut Room, area: Rect) {
    draw_gate(
        frame,
        room,
        area,
        t!("p2p.leave_title").to_string(),
        vec![t!("p2p.leave_1").to_string(), t!("p2p.leave_2").to_string()],
        (t!("p2p.leave_stay").to_string(), Act::LeaveCancel),
        (t!("p2p.leave_confirm").to_string(), Act::LeaveConfirm),
    );
}

/// The blocked list: endpoint ids, Enter unblocks the cursor row.
fn draw_blocked(frame: &mut Frame, room: &mut Room, area: Rect, sel: usize) {
    let blocked = room.status.as_ref().map(|s| s.blocked_peers.clone()).unwrap_or_default();
    let inner = kit::modal_frame(frame, area, 74, 6 + blocked.len().max(1) as u16, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.blocked_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::BlockedClose, t!("path_modal.tip_close"));
    if blocked.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("p2p.blocked_empty").to_string(), dim())), line(inner.y + 2));
    }
    for (i, id) in blocked.iter().enumerate() {
        let rect = line(inner.y + 2 + i as u16);
        let selected = i == sel;
        let hovered = !selected && room.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(Span::styled(format!(" {}", printable(id, 64)), style)).style(style), rect);
        room.ui.click(rect, if selected { Act::BlockedRun } else { Act::BlockedPick(i) });
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("p2p.blocked_unblock").to_string(), dim())).alignment(Alignment::Right),
        line(inner.bottom().saturating_sub(1)),
    );
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    fn hex(seed: &str) -> String {
        seed.repeat(8).chars().take(64).collect()
    }

    fn status_on() -> DiscoveryStatus {
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

    fn status_off() -> DiscoveryStatus {
        DiscoveryStatus { enabled: false, binary_found: false, binary_fetchable: true, ..status_on() }
    }

    fn peer(
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

    fn catalog() -> DiscoveryCatalog {
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

    fn request(id: &str, direction: &str, state: &str) -> FederationRequest {
        FederationRequest {
            id: 1,
            peer_endpoint_id: hex(id),
            direction: direction.into(),
            state: state.into(),
            ..Default::default()
        }
    }

    fn loaded(status: DiscoveryStatus) -> Done {
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

    fn on() -> Room {
        let mut room = room();
        room.apply(loaded(status_on()));
        room
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            handle_key(room, key(KeyCode::Char(c)));
        }
    }

    /// Render one frame and flatten the buffer to text for assertions.
    fn draw(room: &mut Room) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, room)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn off_the_network_the_page_joins_at_once_with_the_inbox_on_by_default() {
        let _en = english();
        let mut room = room();
        room.apply(loaded(status_off()));
        let frame = draw(&mut room);
        assert!(frame.contains("• not on the discovery network"), "{frame}");
        assert!(frame.contains("Join the discovery network"), "{frame}");
        assert!(frame.contains("[✓] Also let other servers send me federation requests"), "{frame}");
        assert!(frame.contains("Never any audio files."), "{frame}");
        assert!(frame.contains("• not on the discovery network · The p2p-sidecar will be downloaded when you join."), "{frame}");
        assert!(frame.contains("Enter join the network · Space federation requests · Esc back"), "{frame}");
        // Enter joins with the inbox — no gate in between.
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::JoinNetwork { accept_requests: true }));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char(' ')));
        assert!(draw(&mut room).contains("[ ] Also let"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::JoinNetwork { accept_requests: false }));
        room.queued = None;
        // The answer: a note, a reload, and the identity modal once the
        // network shows as on — the webapp's straight-into-naming move.
        room.apply(Done::Joined(Ok(serde_json::json!({ "acceptRequests": true }))));
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.contains("give the mesh a minute") && n.contains("inbox")));
        assert!(matches!(room.queued, Some(Op::Load { .. })));
        room.queued = None;
        room.apply(loaded(status_on()));
        assert!(matches!(room.modal, Modal::Identity(_)));
        // A partial failure says exactly which half failed.
        let mut room = room_after_join();
        room.apply(Done::Joined(Ok(serde_json::json!({ "federationError": "iroh has no build here" }))));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("iroh has no build here")));
    }

    fn room_after_join() -> Room {
        let mut room = room();
        room.apply(loaded(status_off()));
        handle_key(&mut room, key(KeyCode::Enter));
        room.queued = None;
        room
    }

    #[test]
    fn the_state_row_says_the_mesh_state_in_the_webapps_words() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("• connected — 3 mesh neighbors · sidecar 19 MB · announcing as mStream Den"), "{frame}");
        assert!(frame.contains("polls every 10 s"), "{frame}");
        assert!(!frame.contains("searching for peers"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        s.recovery = DiscoveryRecovery { attempts: 2, retry_pending: true };
        s.watchdog.restarts = 1;
        room.apply(loaded(s));
        let frame = draw(&mut room);
        assert!(frame.contains("• reconnecting — sidecar died, replaying (attempt 2) · sidecar 19 MB (1 watchdog restart)"), "{frame}");
        assert!(!frame.lines().nth(2).unwrap_or_default().contains("announcing"), "the name drops first when the row is too wide\n{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ crash recovery replays the stack"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        room.apply(loaded(s));
        let frame = draw(&mut room);
        assert!(frame.contains("• joined, waiting for neighbors"), "{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ searching for peers — this room updates itself every 10 s"), "{frame}");

        let mut s = status_on();
        s.neighbors = 0;
        s.joined = false;
        room.apply(loaded(s));
        assert!(draw(&mut room).contains("• not joined yet"));

        let mut s = status_on();
        s.binary_found = false;
        room.apply(loaded(s));
        assert!(draw(&mut room).contains("• unavailable — the p2p-sidecar binary was not found"));
    }

    #[test]
    fn the_table_derives_its_columns_from_the_catalog_and_the_requests() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        for word in ["SERVERS YOU FOLLOW", "SERVER", "TRACKS", "SEEDERS", "ONLINE", "SNAPSHOT", "FEDERATION"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("/ filter · 1 hidden — incompatible model · h show · 1 blocked · u unblock"), "{frame}");
        let row = |name: &str| frame.lines().find(|l| l.starts_with(&format!("  {name}"))).map(str::to_string).unwrap_or_default();
        let basement = row("Basement Archive");
        assert!(basement.contains("12,204") && basement.contains("online") && basement.contains("downloaded") && basement.contains("federated"), "{basement}");
        let vinyl = row("Vinyl Rips '94");
        assert!(vinyl.contains("downloaded · pinned") && vinyl.contains(" —"), "{vinyl}");
        let friend = row("friend-node");
        assert!(friend.contains("update available") && friend.contains("they asked you"), "{friend}");
        let jazz = row("jazz-corner");
        assert!(jazz.contains("offline · 3d") && jazz.contains("—"), "{jazz}");
        let plain = row("mStream ");
        assert!(plain.contains("offline · 12d") && plain.contains("request sent"), "{plain}");
        assert!(frame.contains("↑↓ select · ←→ tab · / filter · h hidden · e name · x leave · Esc back"), "{frame}");
        // The cursor row's description rides the note line.
        handle_key(&mut room, key(KeyCode::Down));
        let frame = draw(&mut room);
        assert!(frame.contains("Basement Archive · Northern soul, dub and a wall of 7-inch singles."), "{frame}");
        assert!(frame.contains("↑↓ rows · Enter details · d download"), "{frame}");
        // Relations, by the webapp's priority.
        assert_eq!(room.relation(&hex("8f31c0e2a7")), Relation::Federated);
        assert_eq!(room.relation(&hex("c4a8f0e19d")), Relation::Theirs);
        assert_eq!(room.relation(&hex("05d6e8f2ac")), Relation::Sent);
        assert_eq!(room.relation(&hex("7e11aa93b0")), Relation::None);
    }

    #[test]
    fn the_stat_tiles_add_up_and_the_tabs_switch_with_the_arrows() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains(" Stats "), "{frame}");
        for word in ["3 of 6", "snapshots held", "23.0 MB of 500.0 MB", "24,569", "peer tracks searchable", "across 3 held libraries", "6", "servers known", "1 hidden · 1 blocked", "19 MB", "ceiling 256 MB · 0 restarts", "seeding"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Activity);
        assert_eq!(room.queued, Some(Op::Activity(0)));
        room.queued = None;
        let now = unix_now();
        let entry = |seq: u64, ago: i64, level: &str, message: &str| ActivityEntry {
            seq,
            t: iso_at(now - ago),
            level: level.into(),
            message: message.into(),
        };
        room.apply(Done::Activity(Ok(DiscoveryActivity {
            entries: vec![
                entry(1, 3 * 3600, "info", "sidecar started (pid 48211)"),
                entry(2, 61 * 60, "warn", "rotation found nothing fetchable under the cap"),
                entry(3, 3 * 60, "info", "Basement Archive announced a new snapshot (seq 40 -> 41)"),
            ],
            last_seq: 3,
        })));
        let frame = draw(&mut room);
        let newest = frame.lines().position(|l| l.contains("announced a new snapshot")).unwrap();
        let oldest = frame.lines().position(|l| l.contains("sidecar started")).unwrap();
        assert!(newest < oldest, "newest first\n{frame}");
        assert!(frame.contains("       3m  Basement Archive announced"), "{frame}");
        assert!(frame.contains("newest first · full history in the server logs"), "{frame}");
        // A restarted server resets its seq: the history starts over.
        room.apply(Done::Activity(Ok(DiscoveryActivity { entries: vec![entry(1, 10, "info", "fresh boot")], last_seq: 1 })));
        assert_eq!(room.activity.len(), 1);
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Invite);
        let frame = draw(&mut room);
        assert!(frame.contains("ENDPOINT") && frame.contains(&hex("51403e7b9c")), "{frame}");
        assert!(frame.contains("YOUR TICKET") && frame.contains("nodeaa4qk3r7z2nlxwm5c6bdhpj2ev5gktr3q7hifobyq2j5ru4kiggizaaaqbxk3ewgk6wxjm"), "{frame}");
        assert!(frame.contains("BEFRIEND A SERVER") && frame.contains("y copy"), "{frame}");
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Config);
        let frame = draw(&mut room);
        assert!(frame.contains("ANNOUNCING AS  mStream Den — Vinyl, jazz and field recordings"), "{frame}");
        assert!(frame.contains("1  Snapshot storage cap    500 MB"), "{frame}");
        assert!(frame.contains("2  Forget offline servers  after 30 days of silence"), "{frame}");
        assert!(frame.contains("3  Auto-fetch snapshots    up to 6 servers"), "{frame}");
        assert!(frame.contains("4  Rotate downloads        after 7 days"), "{frame}");
        assert!(frame.contains("5  Sidecar memory ceiling  256 MB"), "{frame}");
        assert!(frame.contains("   Community seeds         on"), "{frame}");
        assert!(frame.contains("1–5 edit a setting"), "{frame}");
        handle_key(&mut room, key(KeyCode::Right));
        assert_eq!(room.tab, Tab::Stats, "the tabs wrap");
        handle_key(&mut room, key(KeyCode::Left));
        assert_eq!(room.tab, Tab::Config);
    }

    #[test]
    fn the_sheet_lists_only_what_applies_and_its_letters_act() {
        let _en = english();
        let mut room = on();
        // Basement: held, federated, online → update, pin, remove, block.
        handle_key(&mut room, key(KeyCode::Down));
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Sheet { id, sel: 0 } = &room.modal else { panic!("the sheet, cursor on top") };
        let keys: Vec<char> = room.sheet_menu(room.peer(id).unwrap()).iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, vec!['d', 'p', 'r', 'b']);
        let frame = draw(&mut room);
        assert!(frame.contains("Update the snapshot") && frame.contains("Pin it") && !frame.contains("Ask to federate"), "{frame}");
        assert!(frame.contains("downloaded 2d ago · 8.0 MB"), "{frame}");
        assert!(frame.contains("12,204 tracks · 3 seeders"), "{frame}");
        assert!(frame.contains("↑↓ choose · Enter act · Esc close"), "{frame}");
        // Enter on the cursor row runs it.
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None));
        assert!(matches!(room.queued, Some(Op::Fetch { .. })));
        room.queued = None;
        // jazz-corner: offline, nothing held, no relation → download,
        // federate, forget, block.
        for _ in 0..3 {
            handle_key(&mut room, key(KeyCode::Down));
        }
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Sheet { id, .. } = &room.modal else { panic!("the sheet") };
        let keys: Vec<char> = room.sheet_menu(room.peer(id).unwrap()).iter().map(|(k, _, _)| *k).collect();
        assert_eq!(keys, vec!['d', 'f', 'g', 'b']);
        // The letters work inside the sheet, and block is gated.
        handle_key(&mut room, key(KeyCode::Char('b')));
        assert!(matches!(room.modal, Modal::Block(_)));
        let frame = draw(&mut room);
        assert!(frame.contains("Block jazz-corner?") && frame.contains("◂ Keep") && frame.contains("y block · Esc keep"), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        handle_key(&mut room, key(KeyCode::Char('b')));
        handle_key(&mut room, key(KeyCode::Char('y')));
        assert_eq!(
            room.queued,
            Some(Op::Peer { action: PeerAction::Block, id: hex("7e11aa93b0"), name: "jazz-corner".into() })
        );
    }

    #[test]
    fn row_keys_act_on_the_cursor_row_and_the_compose_offers_every_library() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Down)); // Basement: federated
        handle_key(&mut room, key(KeyCode::Char('f')));
        assert!(matches!(room.modal, Modal::None), "no compose for a server already federated with");
        handle_key(&mut room, key(KeyCode::Down)); // Vinyl: held, pinned
        handle_key(&mut room, key(KeyCode::Char('p')));
        assert_eq!(room.queued, Some(Op::Pin { id: hex("2b7d9e4c11"), pinned: false }));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('r')));
        assert!(matches!(room.queued, Some(Op::Peer { action: PeerAction::RemoveSnapshot, .. })));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('g')));
        assert!(room.queued.is_none(), "an online, held server cannot be forgotten");
        handle_key(&mut room, key(KeyCode::Down));
        handle_key(&mut room, key(KeyCode::Down)); // jazz-corner
        handle_key(&mut room, key(KeyCode::Char('g')));
        assert!(matches!(room.queued, Some(Op::Peer { action: PeerAction::Forget, .. })));
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('f')));
        assert!(matches!(room.modal, Modal::Federate(_)));
        assert_eq!(room.queued, Some(Op::Libraries));
        room.queued = None;
        room.apply(Done::Libraries(Ok(vec!["music".into(), "vinyl".into()])));
        type_text(&mut room, "Swap jazz for field recordings?");
        let frame = draw(&mut room);
        assert!(frame.contains("Ask jazz-corner to federate"), "{frame}");
        assert!(frame.contains("MESSAGE · optional · 31 / 500"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("[✓] vinyl"), "{frame}");
        handle_key(&mut room, key(KeyCode::Tab));
        handle_key(&mut room, key(KeyCode::Tab));
        handle_key(&mut room, key(KeyCode::Char(' ')));
        assert!(draw(&mut room).contains("[ ] vinyl"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(
            room.queued,
            Some(Op::SendRequest {
                id: hex("7e11aa93b0"),
                name: "jazz-corner".into(),
                offer: vec!["music".into()],
                message: "Swap jazz for field recordings?".into(),
            })
        );
        room.queued = None;
        room.apply(Done::RequestSent {
            name: "jazz-corner".into(),
            result: Err(ApiError::Server { status: 409, message: "already asked".into() }),
        });
        let Modal::Federate(c) = &room.modal else { panic!("a refusal keeps the compose") };
        assert!(c.error.as_deref().is_some_and(|e| e.contains("already asked")));
        room.apply(Done::RequestSent { name: "jazz-corner".into(), result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("jazz-corner will see it")));
    }

    #[test]
    fn settings_open_by_digit_and_refuse_what_the_server_would() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('3')));
        let Modal::Setting(d) = &room.modal else { panic!("the setting modal") };
        assert_eq!(d.which, DiscoverySetting::AutoFetchCount);
        assert_eq!(d.value.value(), "6");
        handle_key(&mut room, key(KeyCode::Backspace));
        type_text(&mut room, "9x9");
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Setting(d) = &room.modal else { panic!("still the setting modal") };
        assert_eq!(d.value.value(), "99", "only digits reach the field");
        assert_eq!(d.error.as_deref(), Some("enter a whole number from 0 to 50"));
        assert!(room.queued.is_none());
        handle_key(&mut room, key(KeyCode::Backspace));
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::Setting { which: DiscoverySetting::AutoFetchCount, value: 9 }));
        room.queued = None;
        room.apply(Done::Setting { which: DiscoverySetting::AutoFetchCount, value: 9, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.status.as_ref().unwrap().auto_fetch_count, 9);
        let frame = draw(&mut room);
        assert!(frame.contains("setting saved"), "{frame}");
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
    fn the_befriend_box_and_the_filter_take_the_keys_only_while_focused() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('j')));
        assert_eq!(room.tab, Tab::Invite);
        assert!(room.ticket_focus);
        type_text(&mut room, "nodeab7fj3pq2xw");
        assert!(room.sel.is_none(), "typing never moved the table cursor");
        let frame = draw(&mut room);
        assert!(frame.contains("nodeab7fj3pq2xw▏"), "{frame}");
        assert!(frame.contains("type or paste the ticket · Enter join · Esc leave the field"), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::Befriend("nodeab7fj3pq2xw".into())));
        room.queued = None;
        room.apply(Done::Befriended(Ok(())));
        assert!(!room.ticket_focus && room.ticket.value().is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("within a minute")));

        handle_key(&mut room, key(KeyCode::Char('/')));
        assert!(room.filter_focus);
        type_text(&mut room, "jazz");
        assert_eq!(room.rows().len(), 1);
        assert!(draw(&mut room).contains("/ jazz▏"));
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(!room.filter_focus && room.filter_on);
        handle_key(&mut room, key(KeyCode::Down));
        assert_eq!(room.selected_peer().map(display_name).as_deref(), Some("jazz-corner"));
        handle_key(&mut room, key(KeyCode::Esc)); // stows the cursor
        handle_key(&mut room, key(KeyCode::Esc)); // clears the filter
        assert!(!room.filter_on);
        assert_eq!(room.rows().len(), 5);
        assert!(matches!(handle_key(&mut room, key(KeyCode::Esc)), Some(Outcome::Quit)));
    }

    #[test]
    fn leaving_is_gated_and_load_errors_name_the_gate_that_bit() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('x')));
        assert!(matches!(room.modal, Modal::Leave));
        let frame = draw(&mut room);
        assert!(frame.contains("Leave the discovery network?") && frame.contains("◂ Stay") && frame.contains("y leave · Esc stay"), "{frame}");
        handle_key(&mut room, key(KeyCode::Esc));
        assert!(room.queued.is_none());
        handle_key(&mut room, key(KeyCode::Char('x')));
        handle_key(&mut room, key(KeyCode::Char('y')));
        assert_eq!(room.queued, Some(Op::LeaveNetwork));
        room.queued = None;
        room.apply(Done::Left(Ok(())));
        assert!(matches!(room.queued, Some(Op::Load { .. })));
        room.apply(loaded(status_off()));
        assert!(!room.enabled() && room.sel.is_none());

        let mut room = room_after_join();
        room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("locked")));
        room.apply(Done::Loaded(Err(ApiError::Unauthorized)));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("login")));
    }

    #[test]
    fn the_blocked_list_unblocks_and_the_hidden_toggle_reloads() {
        let _en = english();
        let mut room = on();
        handle_key(&mut room, key(KeyCode::Char('u')));
        assert!(matches!(room.modal, Modal::Blocked(0)));
        let frame = draw(&mut room);
        assert!(frame.contains("Blocked servers") && frame.contains(&hex("a1b2c3d4e5")), "{frame}");
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(
            room.queued,
            Some(Op::Peer { action: PeerAction::Unblock, id: hex("a1b2c3d4e5"), name: "a1b2c3d4e5a1…".into() })
        );
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Char('h')));
        assert_eq!(room.queued, Some(Op::Load { include_incompatible: true, activity_since: None }));
        assert!(draw(&mut room).contains("showing incompatible servers · h hide"));
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
}
