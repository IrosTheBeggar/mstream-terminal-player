//! The Federation room: the webapp's Federation tab, in the kit.
//!
//! The status card is one state row (on · connected to relay · the
//! endpoint id · the inbox state; gold while the relay connects). The
//! three cards — Requests, Tickets you minted, Peers — are three tabs,
//! each a full-height table with its own affordance on top: the inbox
//! checkbox card, the mint card, the paste box for a friend's ticket.
//! Single-key actions work on the cursor row; anything with fields is a
//! modal (mint, accept, limits, the peer's name); anything destructive is
//! a gold gate (decline, revoke, forget, turn off). Off, the room is the
//! webapp's pitch and one card that turns the endpoint on.
//!
//! Every server call runs on a worker thread (the wizard's Job/Done
//! pattern); the room polls itself every thirty seconds while federation
//! is on, as the page polls its requests.
//!
//! This file is the room itself: its state, its worker, its keys and
//! every word it says. It draws nothing and names none of ratatui's
//! drawing types or the kit's surface. What it still names of the
//! terminal is the keyboard: a key comes in as crate::input's KeyEvent,
//! and the forms' fields and the paste box are tui_input's, edited by
//! the room's own keymap through crossterm's event, whichever view is
//! up. The view in character cells is a child module, `cells`
//! (cells.rs), that reads the room: a click there is one of the room's
//! [`Act`]s and a key is [`Room::key`], so a view drawn beside it cannot
//! disagree about what the room does. What the view keeps between
//! frames it keeps in a type of its own, held here unopened
//! (`cells::State`).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine;
use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Claim, Outcome, age_text, gate_message, iso_at, iso_unix, printable, short_id, unix_now};
use crate::api::types::{
    FederationKey, FederationLimits, FederationParams, FederationPeer, FederationRequest,
    MintedKey, PeerTest,
};
use crate::api::{ApiError, Client, ExpiryChange};
use crate::input::{KeyCode, KeyEvent};
use crate::kit;
use crate::kit::clipboard::Copied;

/// The room in character cells: the terminal's room and the GUI's grid.
mod cells;
/// The same room drawn by egui, in the desktop window's panel.
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// The page's own cadence for its requests card.
const POLL_EVERY: Duration = Duration::from_secs(30);
/// The server's cap on names (keys, peers).
const NAME_MAX: usize = 64;
/// The inbox refuses new requests past this many waiting on an answer.
const INBOX_FULL: usize = 50;

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Requests,
    Tickets,
    Peers,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Requests, Tab::Tickets, Tab::Peers];

    fn next(self, forward: bool) -> Tab {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        let n = Tab::ALL.len();
        Tab::ALL[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }
}

/// What a decoded `mstrfed1:` ticket says about itself — the webapp's
/// client-side preview, so the operator sees who and what before the
/// server is asked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TicketPreview {
    pub name: Option<String>,
    pub libraries: Vec<String>,
    /// The date part of the expiry, if any.
    pub expires: Option<String>,
}

/// One form field, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Library(usize),
    StreamKbps,
    DailyMb,
    MaxStreams,
    Expires,
    TheirOffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormKind {
    /// A new ticket.
    Mint,
    /// Accepting a request IS minting, plus one decision about their offer.
    Accept,
    /// A key's limits and expiry.
    Limits,
}

/// The mint, accept and limits forms — one shape, three uses.
#[derive(Debug, Clone)]
pub(crate) struct Form {
    pub kind: FormKind,
    /// The request (Accept) or key (Limits) the form is about.
    pub id: i64,
    /// The counterpart's name (Accept) or the key's (Limits).
    pub about: String,
    /// Accept: their endpoint id and message, and what they offer.
    pub endpoint_id: String,
    pub message: String,
    pub their_offer: Vec<String>,
    /// Limits: the current expiry, in words.
    pub current_expiry: String,
    pub name: Input,
    pub libraries: Vec<(String, bool)>,
    pub stream_kbps: Input,
    pub daily_mb: Input,
    pub max_streams: Input,
    pub expires: Input,
    pub accept_their_offer: bool,
    /// Index into [`Form::fields`].
    pub focus: usize,
    pub error: Option<String>,
}

impl Form {
    fn new(kind: FormKind, libraries: &[String], limits: &FederationLimits) -> Self {
        Form {
            kind,
            id: 0,
            about: String::new(),
            endpoint_id: String::new(),
            message: String::new(),
            their_offer: Vec::new(),
            current_expiry: String::new(),
            name: Input::default(),
            libraries: libraries.iter().map(|l| (l.clone(), false)).collect(),
            stream_kbps: Input::new(limits.stream_kbps.to_string()),
            daily_mb: Input::new(limits.daily_mb.to_string()),
            max_streams: Input::new(limits.max_streams.to_string()),
            expires: Input::new(if kind == FormKind::Limits { String::new() } else { "0".to_string() }),
            accept_their_offer: true,
            focus: 0,
            error: None,
        }
    }

    /// The fields in Tab order: the name (mint), the libraries (mint,
    /// accept), the four limits, their offer (accept, when they made one).
    fn fields(&self) -> Vec<Field> {
        let mut fields = Vec::new();
        if self.kind == FormKind::Mint {
            fields.push(Field::Name);
        }
        if self.kind != FormKind::Limits {
            fields.extend((0..self.libraries.len()).map(Field::Library));
        }
        fields.extend([Field::StreamKbps, Field::DailyMb, Field::MaxStreams, Field::Expires]);
        if self.kind == FormKind::Accept && !self.their_offer.is_empty() {
            fields.push(Field::TheirOffer);
        }
        fields
    }

    fn focused(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    fn input_mut(&mut self, field: Field) -> Option<&mut Input> {
        match field {
            Field::Name => Some(&mut self.name),
            Field::StreamKbps => Some(&mut self.stream_kbps),
            Field::DailyMb => Some(&mut self.daily_mb),
            Field::MaxStreams => Some(&mut self.max_streams),
            Field::Expires => Some(&mut self.expires),
            Field::Library(_) | Field::TheirOffer => None,
        }
    }

    fn toggle(&mut self, field: Field) {
        match field {
            Field::Library(i) => {
                if let Some((_, on)) = self.libraries.get_mut(i) {
                    *on = !*on;
                }
            }
            Field::TheirOffer => self.accept_their_offer = !self.accept_their_offer,
            _ => {}
        }
    }

    fn vpaths(&self) -> Vec<String> {
        self.libraries.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect()
    }

    /// The three caps; an empty field reads as 0 = unlimited.
    fn limits(&self) -> Option<FederationLimits> {
        let num = |i: &Input| -> Option<u64> {
            let v = i.value().trim();
            if v.is_empty() { Some(0) } else { v.parse().ok() }
        };
        Some(FederationLimits {
            stream_kbps: num(&self.stream_kbps)?,
            daily_mb: num(&self.daily_mb)?,
            max_streams: num(&self.max_streams)?,
        })
    }

    /// The expiry field: None when blank, else the number of days.
    fn expires_days(&self) -> Result<Option<u64>, ()> {
        let v = self.expires.value().trim();
        if v.is_empty() { Ok(None) } else { v.parse().map(Some).map_err(|_| ()) }
    }

    /// The submit button is live once the form could be sent: a name and a
    /// library for a mint, a library for an accept, always for limits.
    fn can_submit(&self) -> bool {
        match self.kind {
            FormKind::Mint => !self.name.value().trim().is_empty() && !self.vpaths().is_empty(),
            FormKind::Accept => !self.vpaths().is_empty(),
            FormKind::Limits => true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Modal {
    None,
    Form(Box<Form>),
    /// A fresh ticket to hand over (None: the endpoint was down, so the
    /// server minted a key but could build no ticket).
    Minted { name: String, ticket: Option<String>, libraries: Vec<String> },
    /// The pasted ticket, previewed, waiting for the peer's display name.
    PeerName { ticket: String, name: Input, preview: TicketPreview },
    /// Gates, by row id.
    Decline(i64),
    Revoke(i64),
    Forget(i64),
    TurnOff,
}

/// Everything a click can mean. Rebuilt into a rect registry every draw;
/// the last-drawn rect wins, which is what puts modals above the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Tab(Tab),
    TurnOn,
    TurnOff,
    TurnOffConfirm,
    TurnOffCancel,
    ToggleInbox,
    /// A table row, by index in the active tab.
    Select(usize),
    Accept(i64),
    Decline(i64),
    DeclineConfirm,
    DeclineCancel,
    CancelRequest(i64),
    Dismiss(i64),
    Mint,
    CopyTicket(i64),
    Limits(i64),
    ResetBinding(i64),
    Revoke(i64),
    RevokeConfirm,
    RevokeCancel,
    TicketFocus,
    TicketAdd,
    Test(i64),
    ToggleDiscovery(i64),
    Forget(i64),
    ForgetConfirm,
    ForgetCancel,
    FormFocus(usize),
    FormToggle(usize),
    FormSubmit,
    FormCancel,
    MintedCopy,
    MintedDone,
    PeerNameAdd,
    PeerNameCancel,
    TableScroll(i8),
    TableScrollTo(usize),
    Quit,
}

/// A server call queued from input handling and run right after the next
/// draw. Ops carry everything they need — the worker never sees the room.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    /// The endpoint's state and — once on — requests, keys, peers, and the
    /// library names the forms offer.
    Load,
    SetEnabled(bool),
    SetInbox(bool),
    Accept {
        id: i64,
        vpaths: Vec<String>,
        limits: FederationLimits,
        expires_at: Option<String>,
        accept_their_offer: bool,
    },
    Reject(i64),
    Cancel(i64),
    Dismiss(i64),
    Mint { name: String, vpaths: Vec<String>, limits: FederationLimits, expires_at: Option<String> },
    Limits { id: i64, limits: FederationLimits, expiry: ExpiryChange },
    ResetBinding(i64),
    Revoke(i64),
    AddPeer { ticket: String, name: Option<String> },
    Test(i64),
    PeerDiscovery { id: i64, on: bool },
    RemovePeer(i64),
}

/// The simple answers, named for the note they leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Did {
    Inbox(bool),
    Accepted,
    Declined,
    Cancelled,
    Dismissed,
    LimitsSaved,
    BindingReset,
    Revoked,
    PeerAdded,
    PeerDiscovery(i64, bool),
    PeerRemoved,
}

/// One load's answer: the endpoint's state is required, the lists are
/// what the server was willing to add.
struct Loaded {
    params: FederationParams,
    requests: Option<(bool, Vec<FederationRequest>)>,
    keys: Option<Vec<FederationKey>>,
    peers: Option<Vec<FederationPeer>>,
    libraries: Option<Vec<String>>,
}

/// What the worker sends back for each [`Op`].
enum Done {
    Loaded(Result<Box<Loaded>, ApiError>),
    Enabled { on: bool, result: Result<serde_json::Value, ApiError> },
    Minted { libraries: Vec<String>, result: Result<MintedKey, ApiError> },
    Tested { id: i64, result: Result<PeerTest, ApiError> },
    Did { what: Did, result: Result<(), ApiError> },
}

fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let did = |what: Did, result: Result<serde_json::Value, ApiError>| Done::Did {
                what,
                result: result.map(|_| ()),
            };
            let done = match op {
                Op::Load => Done::Loaded(crate::api::wait(async {
                    let params = client.admin_federation_async().await?;
                    if !params.enabled {
                        return Ok(Box::new(Loaded {
                            params,
                            requests: None,
                            keys: None,
                            peers: None,
                            libraries: None,
                        }));
                    }
                    // The lists need nothing of each other: they go out
                    // together, one round trip, not four (performance
                    // audit #83).
                    let (requests, keys, peers, libraries) = tokio::join!(
                        client.admin_federation_requests_async(),
                        client.admin_federation_keys_async(),
                        client.admin_federation_peers_async(),
                        client.admin_directories_async(),
                    );
                    // Best-effort lists: a list that fails keeps its last
                    // answer on screen rather than blanking the room.
                    let requests = requests.ok().map(|r| (r.accept_requests, r.requests));
                    let (keys, peers) = (keys.ok(), peers.ok());
                    let libraries = libraries.ok().map(|dirs| dirs.into_keys().collect());
                    Ok(Box::new(Loaded { params, requests, keys, peers, libraries }))
                })),
                Op::SetEnabled(on) => {
                    Done::Enabled { on, result: client.admin_federation_enabled(on) }
                }
                Op::SetInbox(on) => {
                    did(Did::Inbox(on), client.admin_federation_accept_requests(on))
                }
                Op::Accept { id, vpaths, limits, expires_at, accept_their_offer } => did(
                    Did::Accepted,
                    client.admin_federation_request_accept(
                        id,
                        &vpaths,
                        &limits,
                        expires_at.as_deref(),
                        accept_their_offer,
                    ),
                ),
                Op::Reject(id) => did(Did::Declined, client.admin_federation_request_reject(id)),
                Op::Cancel(id) => did(Did::Cancelled, client.admin_federation_request_cancel(id)),
                Op::Dismiss(id) => did(Did::Dismissed, client.admin_federation_request_dismiss(id)),
                Op::Mint { name, vpaths, limits, expires_at } => Done::Minted {
                    libraries: vpaths.clone(),
                    result: client.admin_federation_mint(&name, &vpaths, &limits, expires_at.as_deref()),
                },
                Op::Limits { id, limits, expiry } => {
                    did(Did::LimitsSaved, client.admin_federation_key_limits(id, &limits, expiry))
                }
                Op::ResetBinding(id) => {
                    did(Did::BindingReset, client.admin_federation_key_reset_binding(id))
                }
                Op::Revoke(id) => did(Did::Revoked, client.admin_federation_key_revoke(id)),
                Op::AddPeer { ticket, name } => {
                    did(Did::PeerAdded, client.admin_federation_peer_add(&ticket, name.as_deref()))
                }
                Op::Test(id) => Done::Tested { id, result: client.admin_federation_peer_test(id) },
                Op::PeerDiscovery { id, on } => {
                    did(Did::PeerDiscovery(id, on), client.admin_federation_peer_discovery(id, on))
                }
                Op::RemovePeer(id) => did(Did::PeerRemoved, client.admin_federation_peer_remove(id)),
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
    pub params: Option<FederationParams>,
    pub accept_requests: bool,
    pub requests: Vec<FederationRequest>,
    pub keys: Vec<FederationKey>,
    pub peers: Vec<FederationPeer>,
    /// The library names the mint and accept forms offer.
    pub libraries: Vec<String>,
    /// What each peer said it shares, the last time it was tested here —
    /// the server keeps no such list.
    shares: HashMap<i64, Vec<String>>,
    pub tab: Tab,
    /// The KEYBOARD cursor over the active tab's rows — `None` until ↑/↓.
    pub sel: Option<usize>,
    /// The Peers tab's paste box.
    ticket: Input,
    ticket_focus: bool,
    pub modal: Modal,
    /// One line of status above the tips: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    /// The endpoint start is on its way: the card waits, dim and inert,
    /// until the next load says the endpoint is on (or the start failed).
    starting: bool,
    tscroll: usize,
    last_load: Option<Instant>,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, and the cursor the table last followed.
    cells: cells::State,
}

impl Room {
    pub(super) fn new(client: Client) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            params: None,
            accept_requests: false,
            requests: Vec::new(),
            keys: Vec::new(),
            peers: Vec::new(),
            libraries: Vec::new(),
            shares: HashMap::new(),
            tab: Tab::Requests,
            sel: None,
            ticket: Input::default(),
            ticket_focus: false,
            modal: Modal::None,
            note: None,
            busy: None,
            queued: None,
            starting: false,
            in_flight: false,
            tscroll: 0,
            last_load: None,
            cells: cells::State::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    /// A reload, with the busy note when asked for and quiet for the poll.
    fn reload(&mut self, loud: bool) {
        if loud {
            self.queue(Op::Load, t!("fed.busy_loading"));
        } else {
            self.queued = Some(Op::Load);
        }
    }

    fn enabled(&self) -> bool {
        self.params.as_ref().is_some_and(|p| p.enabled)
    }

    fn limit_defaults(&self) -> FederationLimits {
        self.params.as_ref().map(|p| p.limit_defaults.clone()).unwrap_or_default()
    }

    /// How many rows the active tab has.
    pub(crate) fn rows(&self) -> usize {
        match self.tab {
            Tab::Requests => self.requests.len(),
            Tab::Tickets => self.keys.len(),
            Tab::Peers => self.peers.len(),
        }
    }

    /// Inbound requests waiting on a human here — the tab's count.
    fn pending_inbound(&self) -> usize {
        self.requests.iter().filter(|r| r.direction == "in" && r.state == "received").count()
    }

    fn selected_request(&self) -> Option<&FederationRequest> {
        (self.tab == Tab::Requests).then(|| self.sel.and_then(|s| self.requests.get(s))).flatten()
    }

    fn selected_key(&self) -> Option<&FederationKey> {
        (self.tab == Tab::Tickets).then(|| self.sel.and_then(|s| self.keys.get(s))).flatten()
    }

    fn selected_peer(&self) -> Option<&FederationPeer> {
        (self.tab == Tab::Peers).then(|| self.sel.and_then(|s| self.peers.get(s))).flatten()
    }

    fn request(&self, id: i64) -> Option<&FederationRequest> {
        self.requests.iter().find(|r| r.id == id)
    }

    fn key_row(&self, id: i64) -> Option<&FederationKey> {
        self.keys.iter().find(|k| k.id == id)
    }

    fn peer(&self, id: i64) -> Option<&FederationPeer> {
        self.peers.iter().find(|p| p.id == id)
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    /// A key press: the room's keymap, the same under any view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Tab(tab) => self.switch_tab(tab),
            Act::TurnOn => {
                if let Some(p) = &self.params
                    && !p.enabled
                    && p.available
                    && !self.starting
                {
                    self.starting = true;
                    self.queue(Op::SetEnabled(true), t!("fed.busy_turning_on"));
                }
            }
            Act::TurnOff => {
                if self.enabled() {
                    self.modal = Modal::TurnOff;
                }
            }
            Act::TurnOffConfirm => {
                self.modal = Modal::None;
                self.queue(Op::SetEnabled(false), t!("fed.busy_turning_off"));
            }
            Act::TurnOffCancel
            | Act::DeclineCancel
            | Act::RevokeCancel
            | Act::ForgetCancel
            | Act::FormCancel
            | Act::MintedDone
            | Act::PeerNameCancel => self.modal = Modal::None,
            Act::ToggleInbox => {
                if self.enabled() {
                    let on = !self.accept_requests;
                    self.queue(Op::SetInbox(on), t!("fed.busy_saving"));
                }
            }
            Act::Select(i) => {
                if i < self.rows() {
                    self.sel = Some(i);
                }
            }
            Act::Accept(id) => {
                if let Some(r) = self.request(id)
                    && r.direction == "in"
                    && r.state == "received"
                {
                    let mut form = Form::new(FormKind::Accept, &self.libraries, &self.limit_defaults());
                    form.id = id;
                    form.about = request_name(r);
                    form.endpoint_id = r.peer_endpoint_id.clone();
                    form.message = printable(&r.message, 500);
                    form.their_offer = r.offered_libraries.iter().map(|l| printable(l, NAME_MAX)).collect();
                    self.modal = Modal::Form(Box::new(form));
                }
            }
            Act::Decline(id) => {
                if self.request(id).is_some_and(|r| r.direction == "in" && r.state == "received") {
                    self.modal = Modal::Decline(id);
                }
            }
            Act::DeclineConfirm => {
                if let Modal::Decline(id) = self.modal {
                    self.modal = Modal::None;
                    self.queue(Op::Reject(id), t!("fed.busy_saving"));
                }
            }
            Act::CancelRequest(id) => {
                if self.request(id).is_some_and(request_cancellable) {
                    self.queue(Op::Cancel(id), t!("fed.busy_saving"));
                }
            }
            Act::Dismiss(id) => {
                if self.request(id).is_some_and(request_finished) {
                    self.queue(Op::Dismiss(id), t!("fed.busy_saving"));
                }
            }
            Act::Mint => {
                if self.enabled() {
                    self.modal = Modal::Form(Box::new(Form::new(FormKind::Mint, &self.libraries, &self.limit_defaults())));
                }
            }
            Act::CopyTicket(id) => {
                if let Some(ticket) = self.key_row(id).and_then(|k| k.ticket.clone()) {
                    self.note = Some((copied_note(&ticket), false));
                }
            }
            Act::Limits(id) => {
                if let Some(k) = self.key_row(id) {
                    let current = FederationLimits {
                        stream_kbps: k.stream_kbps,
                        daily_mb: k.daily_mb,
                        max_streams: k.max_streams,
                    };
                    let mut form = Form::new(FormKind::Limits, &[], &current);
                    form.id = id;
                    form.about = printable(&k.name, NAME_MAX);
                    form.current_expiry = expiry_words(k);
                    self.modal = Modal::Form(Box::new(form));
                }
            }
            Act::ResetBinding(id) => {
                if self.key_row(id).is_some_and(|k| k.bound_endpoint_id.is_some()) {
                    self.queue(Op::ResetBinding(id), t!("fed.busy_saving"));
                }
            }
            Act::Revoke(id) => {
                if self.key_row(id).is_some() {
                    self.modal = Modal::Revoke(id);
                }
            }
            Act::RevokeConfirm => {
                if let Modal::Revoke(id) = self.modal {
                    self.modal = Modal::None;
                    self.queue(Op::Revoke(id), t!("fed.busy_saving"));
                }
            }
            Act::TicketFocus => {
                if self.enabled() {
                    self.tab = Tab::Peers;
                    self.sel = None;
                    self.ticket_focus = true;
                }
            }
            Act::TicketAdd => {
                let ticket = self.ticket.value().trim().to_string();
                if ticket.is_empty() {
                    return None;
                }
                match decode_ticket(&ticket) {
                    Some(preview) => {
                        let name = Input::new(preview.name.clone().unwrap_or_default());
                        self.ticket_focus = false;
                        self.modal = Modal::PeerName { ticket, name, preview };
                    }
                    None => self.note = Some((t!("fed.ticket_invalid").to_string(), true)),
                }
            }
            Act::PeerNameAdd => {
                if let Modal::PeerName { ticket, name, .. } = &self.modal {
                    let name = name.value().trim().to_string();
                    let op = Op::AddPeer { ticket: ticket.clone(), name: (!name.is_empty()).then_some(name) };
                    self.queue(op, t!("fed.busy_adding_peer"));
                }
            }
            Act::Test(id) => {
                if let Some(p) = self.peer(id) {
                    let name = printable(&p.name, NAME_MAX);
                    self.queue(Op::Test(id), t!("fed.busy_testing", name = name));
                }
            }
            Act::ToggleDiscovery(id) => {
                if let Some(p) = self.peer(id) {
                    let on = !p.use_discovery;
                    self.queue(Op::PeerDiscovery { id, on }, t!("fed.busy_saving"));
                }
            }
            Act::Forget(id) => {
                if self.peer(id).is_some() {
                    self.modal = Modal::Forget(id);
                }
            }
            Act::ForgetConfirm => {
                if let Modal::Forget(id) = self.modal {
                    self.modal = Modal::None;
                    self.queue(Op::RemovePeer(id), t!("fed.busy_saving"));
                }
            }
            Act::FormFocus(i) => {
                if let Modal::Form(f) = &mut self.modal {
                    f.focus = i.min(f.fields().len().saturating_sub(1));
                }
            }
            Act::FormToggle(i) => {
                if let Modal::Form(f) = &mut self.modal
                    && let Some(field) = f.fields().get(i).copied()
                {
                    f.toggle(field);
                    f.focus = i;
                }
            }
            Act::FormSubmit => self.submit_form(),
            Act::MintedCopy => {
                if let Modal::Minted { ticket: Some(ticket), .. } = &self.modal {
                    self.note = Some((copied_note(ticket), false));
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
        if !self.enabled() || tab == self.tab {
            return;
        }
        self.tab = tab;
        self.sel = None;
        self.tscroll = 0;
        self.ticket_focus = false;
    }

    /// Send the form — after the refusals that need no server: numbers
    /// that are not numbers, a mint with no name, no library ticked.
    fn submit_form(&mut self) {
        let now = unix_now();
        let Modal::Form(f) = &mut self.modal else { return };
        if !f.can_submit() {
            f.error = Some(t!("fed.form_incomplete").to_string());
            return;
        }
        let (Some(limits), Ok(days)) = (f.limits(), f.expires_days()) else {
            f.error = Some(t!("fed.form_numbers").to_string());
            return;
        };
        f.error = None;
        let cutoff = |days: u64| iso_at(now + days as i64 * 86_400);
        let op = match f.kind {
            FormKind::Mint => Op::Mint {
                name: f.name.value().trim().to_string(),
                vpaths: f.vpaths(),
                limits,
                expires_at: days.filter(|d| *d > 0).map(cutoff),
            },
            FormKind::Accept => Op::Accept {
                id: f.id,
                vpaths: f.vpaths(),
                limits,
                expires_at: days.filter(|d| *d > 0).map(cutoff),
                accept_their_offer: f.accept_their_offer,
            },
            FormKind::Limits => Op::Limits {
                id: f.id,
                limits,
                expiry: match days {
                    None => ExpiryChange::Keep,
                    Some(0) => ExpiryChange::Never,
                    Some(d) => ExpiryChange::At(cutoff(d)),
                },
            },
        };
        let busy = match f.kind {
            FormKind::Mint => t!("fed.busy_minting"),
            FormKind::Accept => t!("fed.busy_accepting"),
            FormKind::Limits => t!("fed.busy_saving"),
        };
        self.queue(op, busy);
    }

    // ── Server calls ────────────────────────────────────────────────────────

    fn dispatch_queued(&mut self) {
        if self.in_flight {
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
        if op == Op::Load {
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
                let enabled = loaded.params.enabled;
                self.starting = false;
                self.params = Some(loaded.params);
                if let Some((accept, requests)) = loaded.requests {
                    self.accept_requests = accept;
                    self.requests = requests;
                }
                if let Some(keys) = loaded.keys {
                    self.keys = keys;
                }
                if let Some(peers) = loaded.peers {
                    self.peers = peers;
                }
                if let Some(libraries) = loaded.libraries {
                    self.libraries = libraries;
                }
                if !enabled {
                    self.requests.clear();
                    self.keys.clear();
                    self.peers.clear();
                    self.sel = None;
                    self.ticket_focus = false;
                }
                // The cursor stays a row that exists.
                let n = self.rows();
                self.sel = self.sel.filter(|_| n > 0).map(|s| s.min(n - 1));
            }
            Done::Loaded(Err(e)) => {
                self.note = Some((gate_message(&e, &t!("fed.load_failed")), true));
            }
            Done::Enabled { on, result: Ok(answer) } => {
                if on && answer.get("available").and_then(|v| v.as_bool()) == Some(false) {
                    self.note = Some((t!("fed.unavailable_note").to_string(), true));
                } else {
                    self.note = None;
                }
                self.reload(true);
            }
            Done::Enabled { on, result: Err(e) } => {
                self.starting = false;
                self.fail(&if on { t!("fed.fail_turn_on") } else { t!("fed.fail_turn_off") }, e);
                self.reload(false);
            }
            Done::Minted { libraries, result: Ok(minted) } => {
                let name = printable(&minted.name, NAME_MAX);
                if minted.ticket.is_none() {
                    self.note = Some((t!("fed.minted_no_ticket").to_string(), true));
                } else {
                    self.note = None;
                }
                self.modal = Modal::Minted { name, ticket: minted.ticket, libraries };
                self.reload(false);
            }
            Done::Minted { result: Err(e), .. } => {
                let what = t!("fed.fail_mint").to_string();
                match &mut self.modal {
                    Modal::Form(f) => f.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Tested { id, result: Ok(test) } => {
                let name = self.peer(id).map(|p| printable(&p.name, NAME_MAX)).unwrap_or_default();
                if test.ok {
                    let libs: Vec<String> = test
                        .health
                        .map(|h| h.libraries.iter().map(|l| printable(l, NAME_MAX)).collect())
                        .unwrap_or_default();
                    let shares = if libs.is_empty() { t!("fed.shares_nothing").to_string() } else { libs.join(", ") };
                    self.note = Some((t!("fed.test_ok", name = name, libraries = shares).to_string(), false));
                    self.shares.insert(id, libs);
                } else {
                    let why = printable(test.error.as_deref().unwrap_or(""), 200);
                    self.note = Some((t!("fed.test_failed", name = name, err = why).to_string(), true));
                }
                self.reload(false);
            }
            Done::Tested { result: Err(e), .. } => {
                self.fail(&t!("fed.fail_test"), e);
                self.reload(false);
            }
            Done::Did { what, result: Ok(()) } => {
                let note = match what {
                    Did::Inbox(true) => t!("fed.done_inbox_on"),
                    Did::Inbox(false) => t!("fed.done_inbox_off"),
                    Did::Accepted => t!("fed.done_accepted"),
                    Did::Declined => t!("fed.done_declined"),
                    Did::Cancelled => t!("fed.done_cancelled"),
                    Did::Dismissed => t!("fed.done_dismissed"),
                    Did::LimitsSaved => t!("fed.done_limits"),
                    Did::BindingReset => t!("fed.done_binding_reset"),
                    Did::Revoked => t!("fed.done_revoked"),
                    Did::PeerAdded => t!("fed.done_peer_added"),
                    Did::PeerDiscovery(..) => t!("fed.done_discovery"),
                    Did::PeerRemoved => t!("fed.done_forgotten"),
                };
                if matches!(what, Did::Accepted | Did::LimitsSaved | Did::PeerAdded) {
                    self.modal = Modal::None;
                }
                if let Did::PeerDiscovery(id, on) = what
                    && let Some(p) = self.peers.iter_mut().find(|p| p.id == id)
                {
                    p.use_discovery = on;
                }
                if what == Did::PeerAdded {
                    self.ticket = Input::default();
                }
                self.note = Some((note.to_string(), false));
                self.reload(false);
            }
            Done::Did { what, result: Err(e) } => {
                let text = match what {
                    Did::Inbox(_) => t!("fed.fail_inbox"),
                    Did::Accepted => t!("fed.fail_accept"),
                    Did::Declined => t!("fed.fail_decline"),
                    Did::Cancelled => t!("fed.fail_cancel"),
                    Did::Dismissed => t!("fed.fail_dismiss"),
                    Did::LimitsSaved => t!("fed.fail_limits"),
                    Did::BindingReset => t!("fed.fail_binding_reset"),
                    Did::Revoked => t!("fed.fail_revoke"),
                    Did::PeerAdded => t!("fed.fail_add_peer"),
                    Did::PeerDiscovery(..) => t!("fed.fail_discovery"),
                    Did::PeerRemoved => t!("fed.fail_forget"),
                }
                .to_string();
                // A refusal keeps the form open with the server's words in it.
                match &mut self.modal {
                    Modal::Form(f) if matches!(what, Did::Accepted | Did::LimitsSaved) => {
                        f.error = Some(format!("{text}: {e}"));
                    }
                    Modal::PeerName { .. } if what == Did::PeerAdded => {
                        self.modal = Modal::None;
                        self.fail(&text, e);
                    }
                    _ => self.fail(&text, e),
                }
            }
        }
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

    /// The page's thirty-second poll, quiet: only while federation is on,
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

    // ── What a host asks ────────────────────────────────────────────────────

    /// The keyboard tips a host's footer shows while the room is hosted,
    /// in the words its own tips row would use.
    pub(crate) fn hint(&self) -> String {
        footer_hint(self)
    }

    /// Whether one of the room's modals is up.
    pub(crate) fn modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    /// Every key while a modal is up, and while the Peers tab's paste box
    /// has the caret: a ticket is letters, and none of them is the host's.
    pub(crate) fn claim(&self) -> Claim {
        if self.modal_open() || self.ticket_focus { Claim::All } else { Claim::Open }
    }
}

/// The room, loading: what `mstream-player admin federation` opens.
pub(super) fn start(client: Client) -> Room {
    let mut room = Room::new(client);
    room.reload(true);
    room
}

// ── Row facts ────────────────────────────────────────────────────────────────

/// Copy a ticket and say how it left: on the clipboard, handed to the
/// terminal (which may refuse it without a word), or nowhere.
fn copied_note(ticket: &str) -> String {
    match kit::clipboard::copy(ticket) {
        Copied::Clipboard => t!("fed.copied_clipboard"),
        Copied::Terminal => t!("fed.copied"),
        Copied::Failed => t!("fed.copy_failed"),
    }
    .to_string()
}

/// The request's counterpart, for the screen: their self-asserted name,
/// gated, or the short endpoint id.
fn request_name(r: &FederationRequest) -> String {
    let name = printable(r.peer_name.as_deref().unwrap_or(""), NAME_MAX);
    if name.is_empty() { short_id(&r.peer_endpoint_id) } else { name }
}

/// An outbound request that has not been answered yet.
fn request_cancellable(r: &FederationRequest) -> bool {
    r.direction == "out" && matches!(r.state.as_str(), "pending-delivery" | "delivered")
}

/// A record the exchange is done with, one way or another.
fn request_finished(r: &FederationRequest) -> bool {
    matches!(r.state.as_str(), "completed" | "rejected" | "refused" | "cancelled" | "expired")
}

fn request_needs_answer(r: &FederationRequest) -> bool {
    r.direction == "in" && r.state == "received"
}

/// The webapp's chip families, as kit colors: what needs YOU is gold, a
/// pairing that happened is green, a dead exchange is dim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Wait,
    Theirs,
    Good,
    Dead,
    Mute,
}

/// The status words for a request — the webapp's own table, plus the
/// retry ladder straight off the engine's state.
fn request_status(r: &FederationRequest) -> (String, Family) {
    let (words, family) = match (r.direction.as_str(), r.state.as_str()) {
        ("out", "pending-delivery") => (t!("fed.st_sending"), Family::Wait),
        ("out", "delivered") => (t!("fed.st_waiting_them"), Family::Wait),
        ("out", "granting") => (t!("fed.st_sharing_back"), Family::Wait),
        ("in", "received") => (t!("fed.st_needs_answer"), Family::Theirs),
        ("in", "accepted") => (t!("fed.st_sending_ticket"), Family::Wait),
        ("in", "granting") => (t!("fed.st_waiting_share"), Family::Wait),
        ("out", "rejected") => (t!("fed.st_declined"), Family::Dead),
        ("in", "rejected") => (t!("fed.st_you_declined"), Family::Dead),
        ("out", "refused") => (t!("fed.st_inbox_closed"), Family::Dead),
        (_, "completed") => (t!("fed.st_federated"), Family::Good),
        (_, "cancelled") => (t!("fed.st_withdrawn"), Family::Mute),
        (_, "expired") => (t!("fed.st_expired"), Family::Mute),
        (_, other) => (std::borrow::Cow::Owned(other.to_string()), Family::Mute),
    };
    let mut words = words.to_string();
    let sending = (r.direction == "out" && matches!(r.state.as_str(), "pending-delivery" | "granting"))
        || (r.direction == "in" && r.state == "accepted");
    if sending && r.fail_count > 0 && r.next_attempt_at.is_some() {
        words.push_str(&t!("fed.st_retry", n = r.fail_count + 1));
    }
    (words, family)
}

/// The message-and-offer cell: their words quoted, then the offer.
fn request_offer(r: &FederationRequest) -> String {
    let libs: Vec<String> = r.offered_libraries.iter().map(|l| printable(l, NAME_MAX)).collect();
    let offer = match (r.direction.as_str(), libs.is_empty()) {
        ("in", false) => t!("fed.offers", libraries = libs.join(", ")),
        ("in", true) => t!("fed.offers_nothing"),
        (_, false) => t!("fed.you_offered", libraries = libs.join(", ")),
        (_, true) => t!("fed.you_offered_nothing"),
    }
    .to_string();
    let message = printable(&r.message, 500);
    if message.is_empty() { offer } else { format!("\u{201c}{message}\u{201d} · {offer}") }
}

/// The webapp's fmtLimits: each cap that is set, or "unlimited".
fn limits_words(k: &FederationKey) -> String {
    let mut parts = Vec::new();
    if k.stream_kbps > 0 {
        parts.push(if k.stream_kbps >= 1000 {
            t!("fed.lim_mbps", n = trim_float(k.stream_kbps as f64 / 1000.0)).to_string()
        } else {
            t!("fed.lim_kbps", n = k.stream_kbps).to_string()
        });
    }
    if k.daily_mb > 0 {
        parts.push(if k.daily_mb >= 1024 {
            t!("fed.lim_gb_day", n = trim_float(k.daily_mb as f64 / 1024.0)).to_string()
        } else {
            t!("fed.lim_mb_day", n = k.daily_mb).to_string()
        });
    }
    if k.max_streams > 0 {
        parts.push(
            if k.max_streams == 1 { t!("fed.lim_stream_one") } else { t!("fed.lim_streams", n = k.max_streams) }
                .to_string(),
        );
    }
    if parts.is_empty() { t!("fed.lim_unlimited").to_string() } else { parts.join(" · ") }
}

/// `7.8` stays, `8.0` becomes `8`.
fn trim_float(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// The key's expiry in words: never, expired, or the days left.
fn expiry_words(k: &FederationKey) -> String {
    if k.expired {
        return t!("fed.exp_expired").to_string();
    }
    match k.expires_at.as_deref().and_then(iso_unix) {
        None => t!("fed.exp_never").to_string(),
        Some(at) => {
            let days = (at - unix_now() + 86_399) / 86_400;
            if days <= 1 { t!("fed.exp_today").to_string() } else { t!("fed.exp_in_days", n = days).to_string() }
        }
    }
}

/// A SQLite timestamp as an age ("3h"), or the dash.
fn age_of(ts: Option<&str>, now: i64) -> String {
    ts.and_then(iso_unix).map(|t| age_text(now - t)).unwrap_or_else(|| "—".to_string())
}

/// The webapp's client-side preview: `mstrfed<V>:<base64url(JSON)>` with
/// `t` and `k` required, `n`/`l`/`e` optional. None when it is not a ticket.
pub(crate) fn decode_ticket(s: &str) -> Option<TicketPreview> {
    let rest = s.trim().strip_prefix("mstrfed")?;
    let colon = rest.find(':')?;
    let (version, body) = rest.split_at(colon);
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let body = body[1..].trim().trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(body)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(body))
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if !v.get("t").is_some_and(|t| t.is_string()) || !v.get("k").is_some_and(|k| k.is_string()) {
        return None;
    }
    Some(TicketPreview {
        name: v.get("n").and_then(|n| n.as_str()).map(|n| printable(n, NAME_MAX)).filter(|n| !n.is_empty()),
        libraries: v
            .get("l")
            .and_then(|l| l.as_array())
            .map(|l| l.iter().filter_map(|x| x.as_str()).map(|x| printable(x, NAME_MAX)).collect())
            .unwrap_or_default(),
        expires: v.get("e").and_then(|e| e.as_str()).map(|e| e.chars().take(10).collect()),
    })
}

fn preview_words(p: &TicketPreview) -> String {
    let mut s = p.name.clone().unwrap_or_else(|| t!("fed.unnamed").to_string());
    if !p.libraries.is_empty() {
        s.push_str(&t!("fed.preview_shares", libraries = p.libraries.join(", ")));
    }
    if let Some(e) = &p.expires {
        s.push_str(&t!("fed.preview_until", date = e));
    }
    s
}

// ── Keys ─────────────────────────────────────────────────────────────────────

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match &mut room.modal {
        Modal::Form(f) => {
            let n = f.fields().len();
            return match code {
                KeyCode::Esc => room.act(Act::FormCancel),
                KeyCode::Enter => room.act(Act::FormSubmit),
                KeyCode::Tab | KeyCode::Down => {
                    f.focus = (f.focus + 1) % n.max(1);
                    None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    f.focus = (f.focus + n.max(1) - 1) % n.max(1);
                    None
                }
                KeyCode::Char(' ') if f.input_mut(f.focused()).is_none() => {
                    let i = f.focus;
                    room.act(Act::FormToggle(i))
                }
                _ => {
                    let field = f.focused();
                    let numeric = !matches!(field, Field::Name);
                    if let KeyCode::Char(ch) = code {
                        // The name takes text (no `|`, the server's rule);
                        // the numbers take digits — and nothing else.
                        let allowed = if numeric { ch.is_ascii_digit() } else { ch != '|' && !ch.is_control() };
                        let max = if numeric { 9 } else { NAME_MAX };
                        let full = f.input_mut(field).is_some_and(|i| i.value().chars().count() >= max);
                        if !allowed || full {
                            return None;
                        }
                    }
                    if let Some(input) = f.input_mut(field) {
                        input.handle_event(&TermEvent::Key(key));
                        f.error = None;
                    }
                    None
                }
            };
        }
        Modal::Minted { .. } => {
            return match code {
                KeyCode::Esc | KeyCode::Enter => room.act(Act::MintedDone),
                KeyCode::Char('y') => room.act(Act::MintedCopy),
                _ => None,
            };
        }
        Modal::PeerName { name, .. } => {
            return match code {
                KeyCode::Esc => room.act(Act::PeerNameCancel),
                KeyCode::Enter => room.act(Act::PeerNameAdd),
                KeyCode::Char('|') => None,
                KeyCode::Char(ch) if ch.is_control() => None,
                KeyCode::Char(_) if name.value().chars().count() >= NAME_MAX => None,
                _ => {
                    name.handle_event(&TermEvent::Key(key));
                    None
                }
            };
        }
        Modal::Decline(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::DeclineConfirm),
                // Enter is the SAFE choice on a warning gate.
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::DeclineCancel),
                _ => None,
            };
        }
        Modal::Revoke(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::RevokeConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::RevokeCancel),
                _ => None,
            };
        }
        Modal::Forget(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::ForgetConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::ForgetCancel),
                _ => None,
            };
        }
        Modal::TurnOff => {
            return match code {
                KeyCode::Char('y') => room.act(Act::TurnOffConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::TurnOffCancel),
                _ => None,
            };
        }
        Modal::None => {}
    }

    // Off: the page has two keys.
    if !room.enabled() {
        return match code {
            KeyCode::Enter => room.act(Act::TurnOn),
            KeyCode::Esc | KeyCode::Char('q') => room.act(Act::Quit),
            _ => None,
        };
    }

    // The paste box with focus takes the keys; Esc gives them back.
    if room.ticket_focus {
        return match code {
            KeyCode::Esc => {
                room.ticket_focus = false;
                None
            }
            KeyCode::Enter => room.act(Act::TicketAdd),
            _ => {
                room.ticket.handle_event(&TermEvent::Key(key));
                None
            }
        };
    }

    let n = room.rows();
    let req = room.selected_request().map(|r| r.id);
    let key_id = room.selected_key().map(|k| k.id);
    let peer = room.selected_peer().map(|p| p.id);
    match code {
        // ↑/↓ are the ONLY way a row gets highlighted (a click on a row
        // does the same); Esc puts the cursor away — and, with nothing
        // selected, leaves.
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
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Enter => match room.tab {
            Tab::Requests => req.and_then(|id| room.act(Act::Accept(id))),
            Tab::Tickets => None,
            Tab::Peers => peer.and_then(|id| room.act(Act::Test(id))),
        },
        KeyCode::Char('a') => req.and_then(|id| room.act(Act::Accept(id))),
        KeyCode::Char('c') => req.and_then(|id| room.act(Act::CancelRequest(id))),
        KeyCode::Char('d') => req.and_then(|id| room.act(Act::Dismiss(id))),
        KeyCode::Char('n') => match room.tab {
            Tab::Requests => req.and_then(|id| room.act(Act::Decline(id))),
            Tab::Tickets => room.act(Act::Mint),
            Tab::Peers => None,
        },
        KeyCode::Char('s') => match room.tab {
            Tab::Requests => room.act(Act::ToggleInbox),
            Tab::Peers => peer.and_then(|id| room.act(Act::ToggleDiscovery(id))),
            Tab::Tickets => None,
        },
        KeyCode::Char('y') => key_id.and_then(|id| room.act(Act::CopyTicket(id))),
        KeyCode::Char('l') => key_id.and_then(|id| room.act(Act::Limits(id))),
        KeyCode::Char('b') => key_id.and_then(|id| room.act(Act::ResetBinding(id))),
        KeyCode::Char('r') | KeyCode::Delete => match room.tab {
            Tab::Tickets => key_id.and_then(|id| room.act(Act::Revoke(id))),
            Tab::Peers => peer.and_then(|id| room.act(Act::Forget(id))),
            Tab::Requests => None,
        },
        KeyCode::Char('t') => peer.and_then(|id| room.act(Act::Test(id))),
        KeyCode::Char('j') => room.act(Act::TicketFocus),
        KeyCode::Char('x') => room.act(Act::TurnOff),
        KeyCode::Char('q') => room.act(Act::Quit),
        _ => None,
    }
}

// ── The tips ─────────────────────────────────────────────────────────────────

/// The tips line names only what works right now.
fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Form(f) => match f.kind {
            FormKind::Mint => t!("fed.hint_mint"),
            FormKind::Accept => t!("fed.hint_accept"),
            FormKind::Limits => t!("fed.hint_limits"),
        },
        Modal::Minted { .. } => t!("fed.hint_minted"),
        Modal::PeerName { .. } => t!("fed.hint_peer_name"),
        Modal::Decline(_) => t!("fed.hint_decline"),
        Modal::Revoke(_) => t!("fed.hint_revoke"),
        Modal::Forget(_) => t!("fed.hint_forget"),
        Modal::TurnOff => t!("fed.hint_turn_off"),
        Modal::None if room.params.is_none() => t!("fed.hint_loading"),
        Modal::None if !room.enabled() => if room.starting { t!("fed.hint_loading") } else { t!("fed.hint_off") },
        Modal::None if room.ticket_focus => t!("fed.hint_ticket"),
        Modal::None => {
            let mut s = String::new();
            match room.tab {
                Tab::Requests => match room.selected_request() {
                    None => s.push_str(&t!("fed.hint_requests")),
                    Some(r) => {
                        s.push_str(&t!("fed.hint_rows"));
                        if request_needs_answer(r) {
                            s.push_str(&t!("fed.hint_answer"));
                        }
                        if request_cancellable(r) {
                            s.push_str(&t!("fed.hint_cancel"));
                        }
                        if request_finished(r) {
                            s.push_str(&t!("fed.hint_dismiss"));
                        }
                        s.push_str(&t!("fed.hint_deselect"));
                    }
                },
                Tab::Tickets => match room.selected_key() {
                    None => s.push_str(&t!("fed.hint_tickets")),
                    Some(k) => {
                        s.push_str(&t!("fed.hint_rows"));
                        if k.ticket.is_some() {
                            s.push_str(&t!("fed.hint_copy"));
                        }
                        s.push_str(&t!("fed.hint_limits_key"));
                        if k.bound_endpoint_id.is_some() {
                            s.push_str(&t!("fed.hint_reset"));
                        }
                        s.push_str(&t!("fed.hint_revoke_key"));
                        s.push_str(&t!("fed.hint_deselect"));
                    }
                },
                Tab::Peers => match room.selected_peer() {
                    None => s.push_str(&t!("fed.hint_peers")),
                    Some(_) => s.push_str(&t!("fed.hint_peer_selected")),
                },
            }
            return s;
        }
    }
    .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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

    pub(super) fn new_room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    pub(super) fn hex(seed: &str) -> String {
        seed.repeat(8).chars().take(64).collect()
    }

    /// SQLite's UTC form, as the federation rows carry it.
    pub(super) fn sqlite_at(t: i64) -> String {
        iso_at(t).replace('T', " ")[..19].to_string()
    }

    pub(super) fn params_on() -> FederationParams {
        FederationParams {
            enabled: true,
            available: true,
            running: true,
            endpoint_id: Some(hex("51403e7b9c")),
            online: true,
            relay_url: Some("https://use1-1.relay.iroh.network./".into()),
            limit_defaults: FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 },
            accept_requests: true,
        }
    }

    pub(super) fn params_off() -> FederationParams {
        FederationParams { enabled: false, running: false, endpoint_id: None, online: false, ..params_on() }
    }

    pub(super) fn request(id: i64, name: &str, seed: &str, direction: &str, state: &str, ago: i64) -> FederationRequest {
        FederationRequest {
            id,
            peer_endpoint_id: hex(seed),
            peer_name: Some(name.into()),
            direction: direction.into(),
            state: state.into(),
            created_at: sqlite_at(unix_now() - ago),
            ..Default::default()
        }
    }

    pub(super) fn requests() -> Vec<FederationRequest> {
        let mut friend = request(1, "friend-node", "c4a8f0e19d", "in", "received", 2 * 3600);
        friend.message = "Jazz for field recordings?".into();
        friend.offered_libraries = vec!["ambient".into()];
        let mut studio = request(2, "studio-nas", "2b7d9e4c11", "out", "delivered", 86_400);
        studio.offered_libraries = vec!["music".into(), "vinyl".into()];
        let mut basement = request(3, "Basement Archive", "8f31c0e2a7", "out", "pending-delivery", 600);
        basement.offered_libraries = vec!["dub".into(), "soul".into()];
        basement.fail_count = 1;
        basement.next_attempt_at = Some(sqlite_at(unix_now() + 300));
        let mut jazz = request(4, "jazz-corner", "7e11aa93b0", "out", "completed", 3 * 86_400);
        jazz.offered_libraries = vec!["jazz".into()];
        jazz.created_peer_id = Some(3);
        let mut ada = request(5, "Ada's laptop", "05d6e8f2ac", "out", "rejected", 5 * 86_400);
        ada.offered_libraries = vec!["music".into()];
        ada.reject_reason = Some("not now, sorry".into());
        vec![friend, studio, basement, jazz, ada]
    }

    pub(super) fn key(id: i64, name: &str, libs: &[&str], limits: (u64, u64, u64), bound: bool) -> FederationKey {
        FederationKey {
            id,
            name: name.into(),
            library_names: libs.iter().map(|l| l.to_string()).collect(),
            stream_kbps: limits.0,
            daily_mb: limits.1,
            max_streams: limits.2,
            bound_endpoint_id: bound.then(|| hex("8f31c0e2a7")),
            bound_at: bound.then(|| sqlite_at(unix_now() - 2 * 86_400)),
            created_at: sqlite_at(unix_now() - 20 * 86_400),
            ticket: Some(format!("mstrfed1:ticket-for-{id}")),
            ..Default::default()
        }
    }

    pub(super) fn keys() -> Vec<FederationKey> {
        let mut ada = key(1, "Ada's laptop", &["music", "vinyl"], (8000, 2048, 3), true);
        ada.usage_today_bytes = 218 * 1_048_576;
        ada.last_used = Some(sqlite_at(unix_now() - 3600));
        let mut studio = key(2, "studio-nas", &["music", "vinyl", "jazz"], (0, 0, 0), false);
        studio.expires_at = Some(sqlite_at(unix_now() + 24 * 86_400));
        studio.last_used = Some(sqlite_at(unix_now() - 30 * 3600));
        let mut jazz = key(3, "jazz-corner", &["jazz"], (2000, 500, 1), true);
        jazz.usage_today_bytes = 4_300_000;
        let mut bob = key(4, "Bob's NAS", &["audiobooks"], (0, 0, 0), false);
        bob.expired = true;
        bob.expires_at = Some(sqlite_at(unix_now() - 86_400));
        vec![ada, studio, jazz, bob]
    }

    pub(super) fn peer(id: i64, name: &str, status: Option<&str>, seen_ago: Option<i64>, discovery: bool) -> FederationPeer {
        FederationPeer {
            id,
            name: name.into(),
            last_status: status.map(str::to_string),
            last_seen: seen_ago.map(|ago| sqlite_at(unix_now() - ago)),
            use_discovery: discovery,
            added_at: sqlite_at(unix_now() - 30 * 86_400),
        }
    }

    pub(super) fn peers() -> Vec<FederationPeer> {
        vec![
            peer(1, "Basement Archive", Some("ok"), Some(10), true),
            peer(2, "studio-nas", Some("ok"), Some(180), false),
            peer(3, "jazz-corner", Some("dial timed out after 20 s"), Some(3 * 86_400), true),
            peer(4, "Ada's laptop", None, None, true),
        ]
    }

    pub(super) fn loaded(params: FederationParams) -> Done {
        let on = params.enabled;
        Done::Loaded(Ok(Box::new(Loaded {
            params,
            requests: on.then(|| (true, requests())),
            keys: on.then(keys),
            peers: on.then(peers),
            libraries: on.then(|| {
                ["music", "vinyl", "field-recordings", "audiobooks", "podcasts"].iter().map(|s| s.to_string()).collect()
            }),
        })))
    }

    pub(super) fn on() -> Room {
        let mut room = new_room();
        room.apply(loaded(params_on()));
        room
    }

    pub(super) fn key_press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(super) fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key_press(code))
    }

    pub(super) fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
    }

    pub(super) fn ticket_for(payload: serde_json::Value) -> String {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("mstrfed1:{body}")
    }

    #[test]
    fn words_for_limits_expiry_states_and_tickets() {
        let _en = english();
        let none: &[&str] = &[];
        let k = |limits: (u64, u64, u64)| key(1, "k", none, limits, false);
        assert_eq!(limits_words(&k((8000, 2048, 3))), "8 Mbps · 2 GB/day · 3 streams");
        assert_eq!(limits_words(&k((7800, 500, 1))), "7.8 Mbps · 500 MB/day · 1 stream");
        assert_eq!(limits_words(&k((320, 0, 0))), "320 kbps");
        assert_eq!(limits_words(&k((0, 0, 0))), "unlimited");
        let mut e = k((0, 0, 0));
        assert_eq!(expiry_words(&e), "never expires");
        e.expires_at = Some(sqlite_at(unix_now() + 24 * 86_400 + 60));
        assert_eq!(expiry_words(&e), "expires in 25 days");
        e.expires_at = Some(sqlite_at(unix_now() + 3600));
        assert_eq!(expiry_words(&e), "expires today");
        e.expired = true;
        assert_eq!(expiry_words(&e), "expired");
        let r = |direction: &str, state: &str| request(1, "x", "ab", direction, state, 0);
        assert_eq!(request_status(&r("in", "received")), ("needs your answer".to_string(), Family::Theirs));
        assert_eq!(request_status(&r("out", "refused")), ("their inbox is closed".to_string(), Family::Dead));
        assert_eq!(request_status(&r("in", "completed")), ("federated".to_string(), Family::Good));
        assert_eq!(request_status(&r("out", "cancelled")), ("withdrawn".to_string(), Family::Mute));
        let mut retry = r("in", "accepted");
        retry.fail_count = 2;
        retry.next_attempt_at = Some(sqlite_at(unix_now() + 60));
        assert_eq!(request_status(&retry).0, "sending your ticket… · retry #3");
        assert_eq!(decode_ticket("nope"), None);
        assert_eq!(decode_ticket("mstrfed1:not-base64!"), None);
        assert_eq!(decode_ticket(&ticket_for(serde_json::json!({ "t": "node", "k": "key" }))), Some(TicketPreview::default()));
        assert_eq!(
            decode_ticket(&format!("  {}  ", ticket_for(serde_json::json!({ "t": "node", "k": "key", "n": "Den\u{1b}[31m", "l": ["a", 1], "e": "2026-12-01T00:00:00Z" })))),
            Some(TicketPreview { name: Some("Den[31m".into()), libraries: vec!["a".into()], expires: Some("2026-12-01".into()) })
        );
        assert!(decode_ticket(&ticket_for(serde_json::json!({ "k": "key" }))).is_none(), "t is required");
        assert!(iso_unix("2026-09-06 09:41:52").is_some(), "SQLite's form parses too");
    }

    #[test]
    fn a_load_asks_for_the_lists_in_one_wave() {
        // Federation's switch first, then the requests, keys, peers and
        // libraries together: two round trips, not five (performance
        // audit #83). A list that fails is left out, not the load.
        let server = super::super::waves::serve(&[1, 4], |path| match path {
            "/api/v1/admin/federation" => (200, r#"{"enabled":true}"#.into()),
            "/api/v1/admin/federation/keys" => (500, "{}".into()),
            "/api/v1/admin/federation/peers" => (200, "[]".into()),
            _ => (200, "{}".into()),
        });
        let (to_worker, from_worker) = spawn_worker();
        to_worker.send((Arc::new(Client::new(&server.url).expect("client")), Op::Load)).unwrap();
        let Done::Loaded(Ok(loaded)) = from_worker.recv_timeout(std::time::Duration::from_secs(30)).expect("an answer")
        else {
            panic!("a load");
        };
        let paths = server.paths();
        assert_eq!((server.peak(), paths.len()), (4, 5), "{paths:?}");
        assert_eq!(paths[0], "/api/v1/admin/federation");
        assert!(loaded.keys.is_none());
        assert!(loaded.requests.is_some() && loaded.peers.is_some() && loaded.libraries.is_some());
    }

    // ── Hosted in the GUI player's Admin tab ─────────────────────────────

    #[test]
    fn it_claims_every_key_while_the_paste_box_has_the_caret() {
        let _en = english();
        let mut room = on();
        assert_eq!(room.claim(), Claim::Open);
        press(&mut room, KeyCode::Char('j'));
        assert!(room.ticket_focus);
        assert_eq!(room.claim(), Claim::All);
        press(&mut room, KeyCode::Char('L'));
        assert_eq!(room.ticket.value(), "L", "a capital L is a letter of the ticket");
        press(&mut room, KeyCode::Esc);
        assert_eq!(room.claim(), Claim::Open);
        room.modal = Modal::TurnOff;
        assert!(room.modal_open());
        assert_eq!(room.claim(), Claim::All);
        assert_eq!(room.hint(), t!("fed.hint_turn_off"));
    }

    /// `y` on a ticket row and on a freshly minted ticket both go through
    /// the kit's clipboard, and the note says which way the ticket went.
    #[test]
    fn y_on_a_key_and_on_the_minted_ticket_copy_and_say_how() {
        use crate::kit::clipboard::{Copied, catch, caught};
        let _en = english();
        let answers = [
            (Copied::Clipboard, t!("fed.copied_clipboard")),
            (Copied::Terminal, t!("fed.copied")),
            (Copied::Failed, t!("fed.copy_failed")),
        ];
        let mut room = on();
        room.switch_tab(Tab::Tickets);
        press(&mut room, KeyCode::Down);
        let row = room.selected_key().and_then(|k| k.ticket.clone()).expect("a ticket row");
        for (answer, said) in &answers {
            catch(*answer);
            room.note = None;
            press(&mut room, KeyCode::Char('y'));
            assert_eq!(room.note, Some((said.to_string(), false)), "the row, {answer:?}");
        }
        assert_eq!(caught(), [row.clone(), row.clone(), row]);

        let minted = "mstrfed1:eyJ0IjoibmV3In0".to_string();
        room.modal = Modal::Minted { name: "Bob's NAS".into(), ticket: Some(minted.clone()), libraries: vec!["music".into()] };
        for (answer, said) in &answers {
            catch(*answer);
            room.note = None;
            press(&mut room, KeyCode::Char('y'));
            assert_eq!(room.note, Some((said.to_string(), false)), "the minted ticket, {answer:?}");
        }
        assert_eq!(caught(), [minted.clone(), minted.clone(), minted]);
    }
}
