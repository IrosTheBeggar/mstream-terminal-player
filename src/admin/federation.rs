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

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use base64::Engine;
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
    Outcome, Screen, age_text, copy_to_clipboard, draw_bottom, draw_header, fmt_bytes,
    frame_ground, gate_message, host_of, iso_at, iso_unix, printable, short_id, unix_now,
};
use crate::api::types::{
    FederationKey, FederationLimits, FederationParams, FederationPeer, FederationRequest,
    MintedKey, PeerTest,
};
use crate::api::{ApiError, Client, ExpiryChange};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::g;

/// The page's own cadence for its requests card.
const POLL_EVERY: Duration = Duration::from_secs(30);
const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
/// The server's cap on names (keys, peers).
const NAME_MAX: usize = 64;
const TAB_GAP: u16 = 2;
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
                Op::Load => Done::Loaded(client.admin_federation().map(|params| {
                    if !params.enabled {
                        return Box::new(Loaded {
                            params,
                            requests: None,
                            keys: None,
                            peers: None,
                            libraries: None,
                        });
                    }
                    // Best-effort lists: a list that fails keeps its last
                    // answer on screen rather than blanking the room.
                    let requests = client
                        .admin_federation_requests()
                        .ok()
                        .map(|r| (r.accept_requests, r.requests));
                    let keys = client.admin_federation_keys().ok();
                    let peers = client.admin_federation_peers().ok();
                    let libraries =
                        client.admin_directories().ok().map(|dirs| dirs.into_keys().collect());
                    Box::new(Loaded { params, requests, keys, peers, libraries })
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
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    ui: Surface<Act>,
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
            sel_anchor: None,
            last_load: None,
            ui: Surface::new(),
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

    fn act(&mut self, act: Act) -> Option<Outcome> {
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
                    let ok = copy_to_clipboard(&ticket);
                    self.note = Some((
                        if ok { t!("fed.copied") } else { t!("fed.copy_failed") }.to_string(),
                        false,
                    ));
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
                    let ok = copy_to_clipboard(ticket);
                    self.note = Some((
                        if ok { t!("fed.copied") } else { t!("fed.copy_failed") }.to_string(),
                        false,
                    ));
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

    /// The page's thirty-second poll, quiet: only while federation is on,
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

    fn wheel(&mut self, up: bool, _at: Position) {
        if matches!(self.modal, Modal::None) {
            self.tscroll = if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
        }
    }
}

/// The room, loading: what `mstream-player admin federation` opens.
pub(super) fn start(client: Client) -> Room {
    let mut room = Room::new(client);
    room.reload(true);
    room
}

// ── Row facts ────────────────────────────────────────────────────────────────

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

fn family_style(family: Family) -> Style {
    match family {
        Family::Theirs => Style::default().fg(th().gold).add_modifier(Modifier::BOLD),
        Family::Good => Style::default().fg(th().ok),
        Family::Dead | Family::Mute => dim(),
        Family::Wait => Style::default(),
    }
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

    draw_header(frame, area, &t!("fed.title"), &host_of(&room.client));
    let column = Rect {
        x: 2,
        y: 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(5),
    };
    match room.params.clone() {
        None => {}
        Some(p) if !p.enabled => draw_off(frame, room, column, &p),
        Some(p) => draw_on(frame, room, column, &p),
    }
    draw_bottom(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Form(f) => draw_form(frame, room, area, &f),
        Modal::Minted { name, ticket, libraries } => draw_minted(frame, room, area, &name, ticket.as_deref(), &libraries),
        Modal::PeerName { name, preview, .. } => draw_peer_name(frame, room, area, &name, &preview),
        Modal::Decline(id) => {
            let who = room.request(id).map(request_name).unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.decline_title", name = who).to_string(),
                vec![t!("fed.decline_1").to_string(), t!("fed.decline_2").to_string()],
                (t!("fed.gate_keep").to_string(), Act::DeclineCancel),
                (t!("fed.decline_confirm").to_string(), Act::DeclineConfirm),
            );
        }
        Modal::Revoke(id) => {
            let (name, libs) = room
                .key_row(id)
                .map(|k| (printable(&k.name, NAME_MAX), k.library_names.join(", ")))
                .unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.revoke_title", name = name).to_string(),
                vec![
                    t!("fed.revoke_1", name = name, libraries = libs).to_string(),
                    t!("fed.revoke_2").to_string(),
                ],
                (t!("fed.gate_keep").to_string(), Act::RevokeCancel),
                (t!("fed.revoke_confirm").to_string(), Act::RevokeConfirm),
            );
        }
        Modal::Forget(id) => {
            let name = room.peer(id).map(|p| printable(&p.name, NAME_MAX)).unwrap_or_default();
            draw_gate(
                frame,
                room,
                area,
                t!("fed.forget_title", name = name).to_string(),
                vec![t!("fed.forget_1").to_string(), t!("fed.forget_2").to_string()],
                (t!("fed.gate_keep").to_string(), Act::ForgetCancel),
                (t!("fed.forget_confirm").to_string(), Act::ForgetConfirm),
            );
        }
        Modal::TurnOff => draw_gate(
            frame,
            room,
            area,
            t!("fed.off_title").to_string(),
            vec![t!("fed.off_1").to_string(), t!("fed.off_2").to_string()],
            (t!("fed.off_stay").to_string(), Act::TurnOffCancel),
            (t!("fed.off_confirm").to_string(), Act::TurnOffConfirm),
        ),
    }
    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

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

/// Off: the state line, the turn-on card (or the gold sentence), the pitch.
fn draw_off(frame: &mut Frame, room: &mut Room, column: Rect, p: &FederationParams) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let mut y = column.y + 2;
    if p.available {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.state_off").to_string(), dim())), line(column.y));
        let card = Rect { x: column.x, y, width: column.width, height: 3 };
        let starting = room.starting;
        let hover = !starting && room.ui.pointer.is_some_and(|pt| card.contains(pt));
        let color = if starting { th().dim } else if hover { th().bright } else { th().ok };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
        let inner = block.inner(card);
        frame.render_widget(block, card);
        let label = if starting { t!("fed.turning_on") } else { t!("fed.turn_on") };
        frame.render_widget(
            Paragraph::new(Span::styled(label.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
            inner,
        );
        if !starting {
            room.ui.click(card, Act::TurnOn);
        }
        y += 4;
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("fed.state_unavailable").to_string(), Style::default().fg(th().gold))),
            line(column.y),
        );
    }
    let lines = vec![
        Line::from(vec![
            Span::raw(t!("fed.pitch_1a").to_string()),
            Span::styled(t!("fed.pitch_1b").to_string(), bold()),
            Span::raw(t!("fed.pitch_1c").to_string()),
        ]),
        Line::from(""),
        Line::from(Span::styled(t!("fed.pitch_credentials").to_string(), dim().add_modifier(Modifier::BOLD))),
        Line::from(t!("fed.pitch_2").to_string()),
    ];
    let body = Rect { x: column.x, y, width: column.width, height: column.bottom().saturating_sub(y) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

/// On: the state row, the tabs, the active tab's body.
fn draw_on(frame: &mut Frame, room: &mut Room, column: Rect, p: &FederationParams) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let connected = p.running && p.online;
    let mut spans = if !p.available {
        vec![Span::styled(t!("fed.state_unavailable").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD))]
    } else if connected {
        vec![
            Span::styled(t!("fed.state_on").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
            Span::raw(t!("fed.state_relay").to_string()),
        ]
    } else {
        vec![
            Span::styled(t!("fed.state_on").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
            Span::raw(t!("fed.state_connecting").to_string()),
        ]
    };
    if let Some(id) = &p.endpoint_id {
        spans.push(Span::raw(t!("fed.state_endpoint", id = short_id(id)).to_string()));
    }
    spans.push(Span::raw(
        if room.accept_requests { t!("fed.state_inbox_open") } else { t!("fed.state_inbox_closed") }.to_string(),
    ));
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    frame.render_widget(Paragraph::new(Line::from(spans)), line(column.y));
    let polls = t!("fed.polls").to_string();
    if state_w + 2 + polls.chars().count() <= column.width as usize {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line(column.y));
    }
    if p.available && !connected {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▱".repeat(10), dim()),
                Span::raw(" "),
                Span::styled(t!("fed.connecting_note").to_string(), accent()),
            ])),
            line(column.y + 1),
        );
    }

    // The tabs: the active one a filled slab, the rest text buttons; the
    // Requests tab carries the count waiting on an answer.
    let tabs_y = column.y + 2;
    let mut x = column.x;
    let pending = room.pending_inbound();
    for tab in Tab::ALL {
        let name = match tab {
            Tab::Requests if pending > 0 => t!("fed.tab_requests_n", n = pending).to_string(),
            Tab::Requests => t!("fed.tab_requests").to_string(),
            Tab::Tickets => t!("fed.tab_tickets").to_string(),
            Tab::Peers => t!("fed.tab_peers").to_string(),
        };
        let label = format!(" {name} ");
        let rect = Rect { x, y: tabs_y, width: label.chars().count() as u16, height: 1 };
        let hover = room.ui.pointer.is_some_and(|pt| rect.contains(pt));
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
    let note = match room.tab {
        Tab::Requests => t!("fed.note_requests"),
        Tab::Tickets => t!("fed.note_tickets"),
        Tab::Peers => t!("fed.note_peers"),
    }
    .to_string();
    if x as usize + 2 + note.chars().count() <= column.right() as usize {
        frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
    }

    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    match room.tab {
        Tab::Requests => draw_requests(frame, room, body),
        Tab::Tickets => draw_tickets(frame, room, body),
        Tab::Peers => draw_peers(frame, room, body),
    }
}

/// The table frame every tab shares: the header words at their columns,
/// the full-width rule beneath. Returns the first row's y.
fn table_head(frame: &mut Frame, at: Rect, cols: &[(u16, u16, &str, bool)]) -> u16 {
    for (x, w, word, right) in cols {
        let rect = Rect { x: *x, y: at.y, width: *w, height: 1 };
        let p = Paragraph::new(Span::styled(word.to_string(), dim()));
        frame.render_widget(if *right { p.alignment(Alignment::Right) } else { p }, rect);
    }
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(at.width as usize), dim())),
        Rect { x: at.x, y: at.y + 1, width: at.width, height: 1 },
    );
    at.y + 2
}

/// A table's rows: the viewport, the selection paint, the scrollbar; the
/// caller paints each row's cells through `draw_row`.
fn table_rows(
    frame: &mut Frame,
    room: &mut Room,
    at: Rect,
    len: usize,
    mut draw_row: impl FnMut(&mut Frame, &mut Room, usize, Rect, bool, bool),
) {
    let avail = at.height as usize;
    let sel_moved = room.sel != room.sel_anchor;
    room.sel_anchor = room.sel;
    let reveal = if sel_moved { room.sel } else { None };
    let (first, visible) = kit::table_view(len, reveal, room.tscroll, avail);
    room.tscroll = first;
    for (row, i) in (first..first + visible).enumerate() {
        let rect = Rect { x: at.x, y: at.y + row as u16, width: at.width, height: 1 };
        let selected = room.sel == Some(i);
        let hovered = !selected && room.ui.pointer.is_some_and(|p| rect.contains(p));
        if selected {
            frame.render_widget(
                Paragraph::new(Span::styled(" ".repeat(at.width as usize), Style::default().fg(th().on_accent).bg(th().accent))),
                rect,
            );
        }
        draw_row(frame, room, i, rect, selected, hovered);
        room.ui.click(rect, Act::Select(i));
    }
    let bar = Rect { x: at.x + at.width, y: at.y, width: 1, height: visible as u16 };
    kit::scroll_list(frame, &mut room.ui, bar, len, visible, first, Act::TableScroll(-1), Act::TableScroll(1), Act::TableScrollTo);
}

/// A cell's style: selection paint wins, then hover, then the cell's own.
fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(text: &str, width: u16) -> String {
    let w = width as usize;
    if text.chars().count() <= w {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(w.saturating_sub(1)).collect::<String>())
    }
}

/// Requests: the inbox checkbox card, then the table.
fn draw_requests(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let card = Rect { x: body.x, y: body.y, width: body.width, height: 4 };
    let hover = room.ui.pointer.is_some_and(|p| card.contains(p));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if hover { th().bright } else { th().dim }));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let glyph = if room.accept_requests {
        Span::styled(format!("{} ", g("[✓]", "[x]")), Style::default().fg(th().ok))
    } else {
        Span::styled("[ ] ", dim())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![glyph, Span::styled(t!("fed.inbox_label").to_string(), bold())])),
        Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(1), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.inbox_hint").to_string(), dim())),
        Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
    );
    room.ui.click(card, Act::ToggleInbox);

    let table = Rect { x: body.x, y: body.y + 5, width: body.width, height: body.height.saturating_sub(5) };
    if table.height < 3 {
        return;
    }
    let (server_w, status_w, age_w) = (18u16, 20u16, 6u16);
    let fixed = server_w + 2 + status_w + 2 + age_w + 2;
    let msg_w = table.width.saturating_sub(fixed);
    let msg_x = table.x + server_w + 2;
    let status_x = msg_x + msg_w + 2;
    let age_x = status_x + status_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, server_w, &t!("fed.col_server"), false),
            (msg_x, msg_w, &t!("fed.col_offer"), false),
            (status_x, status_w, &t!("fed.col_status"), false),
            (age_x, age_w, &t!("fed.col_age"), true),
        ],
    );
    if room.requests.is_empty() {
        let text = if room.accept_requests { t!("fed.empty_requests") } else { t!("fed.empty_requests_closed") };
        frame.render_widget(Paragraph::new(Span::styled(text.to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let requests = room.requests.clone();
    table_rows(frame, room, rows_rect, requests.len(), |frame, room, i, rect, selected, hovered| {
        let r = &requests[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&request_name(r), server_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, server_w),
        );
        let offer = request_offer(r);
        frame.render_widget(Paragraph::new(Span::styled(clip(&offer, msg_w), base)), cell(msg_x, msg_w));
        if offer.chars().count() > msg_w as usize {
            room.ui.tip(cell(msg_x, msg_w), offer.clone());
        }
        let (words, family) = request_status(r);
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&words, status_w), cell_style(selected, hovered, family_style(family)))),
            cell(status_x, status_w),
        );
        frame.render_widget(
            Paragraph::new(Span::styled(age_of(Some(&r.created_at), now), cell_style(selected, hovered, dim()))).alignment(Alignment::Right),
            cell(age_x, age_w),
        );
    });
    // The cursor row's whole story rides the note line, when it is free.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(r) = room.selected_request()
    {
        let mut text = format!("{} · {} · {}", request_name(r), short_id(&r.peer_endpoint_id), request_offer(r));
        if let Some(reason) = r.reject_reason.as_deref().filter(|s| !s.trim().is_empty()) {
            text.push_str(&format!(" · \u{201c}{}\u{201d}", printable(reason, 200)));
        }
        if r.direction == "in" && r.state == "rejected" {
            text.push_str(&t!("fed.ignored_week"));
        }
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    } else if room.note.is_none() && room.busy.is_none() && room.accept_requests && room.pending_inbound() >= INBOX_FULL {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("fed.inbox_full").to_string(), Style::default().fg(th().gold))),
            line(body.bottom() + 1),
        );
    }
}

/// Tickets: the mint card, then the table.
fn draw_tickets(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let card = Rect { x: body.x, y: body.y, width: body.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| card.contains(p));
    let color = if hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.mint_card").to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
            .alignment(Alignment::Center),
        inner,
    );
    room.ui.click(card, Act::Mint);

    let table = Rect { x: body.x, y: body.y + 4, width: body.width, height: body.height.saturating_sub(4) };
    if table.height < 3 {
        return;
    }
    let (name_w, libs_w, today_w, last_w, claimed_w) = (16u16, 16u16, 6u16, 9u16, 9u16);
    let fixed = name_w + 2 + libs_w + 2 + today_w + 2 + last_w + 2 + claimed_w + 2;
    let limits_w = table.width.saturating_sub(fixed).max(8);
    let libs_x = table.x + name_w + 2;
    let limits_x = libs_x + libs_w + 2;
    let today_x = limits_x + limits_w + 2;
    let last_x = today_x + today_w + 2;
    let claimed_x = last_x + last_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, name_w, &t!("fed.col_name"), false),
            (libs_x, libs_w, &t!("fed.col_libraries"), false),
            (limits_x, limits_w, &t!("fed.col_limits"), false),
            (today_x, today_w, &t!("fed.col_today"), true),
            (last_x, last_w, &t!("fed.col_last_used"), false),
            (claimed_x, claimed_w, &t!("fed.col_claimed"), false),
        ],
    );
    if room.keys.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.empty_tickets").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let keys = room.keys.clone();
    table_rows(frame, room, rows_rect, keys.len(), |frame, room, i, rect, selected, hovered| {
        let k = &keys[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let faint = cell_style(selected, hovered, dim());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(&k.name, NAME_MAX), name_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, name_w),
        );
        let libs = k.library_names.join(", ");
        frame.render_widget(Paragraph::new(Span::styled(clip(&libs, libs_w), base)), cell(libs_x, libs_w));
        if libs.chars().count() > libs_w as usize {
            room.ui.tip(cell(libs_x, libs_w), libs.clone());
        }
        let (limits, limits_style) = if k.expired {
            (t!("fed.exp_expired").to_string(), cell_style(selected, hovered, Style::default().fg(th().gold)))
        } else {
            (limits_words(k), base)
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&limits, limits_w), limits_style)), cell(limits_x, limits_w));
        // The webapp's fmtBytes drops a trailing .0 ("218 MB", not "218.0 MB").
        let today = if k.usage_today_bytes > 0 { fmt_bytes(k.usage_today_bytes).replace(".0 ", " ") } else { "—".to_string() };
        frame.render_widget(
            Paragraph::new(Span::styled(today, if k.usage_today_bytes > 0 { base } else { faint })).alignment(Alignment::Right),
            cell(today_x, today_w),
        );
        let last = match k.last_used.as_deref() {
            Some(ts) => (age_of(Some(ts), now), base),
            None => (t!("fed.never").to_string(), faint),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&last.0, last_w), last.1)), cell(last_x, last_w));
        let claimed = if k.bound_endpoint_id.is_some() {
            (t!("fed.claimed").to_string(), cell_style(selected, hovered, Style::default().fg(th().ok)))
        } else {
            (t!("fed.not_claimed").to_string(), faint)
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&claimed.0, claimed_w), claimed.1)), cell(claimed_x, claimed_w));
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(k) = room.selected_key()
    {
        let claimed = match &k.bound_endpoint_id {
            Some(id) => t!("fed.claimed_by", id = short_id(id), age = age_of(k.bound_at.as_deref(), now)).to_string(),
            None => t!("fed.not_claimed_yet").to_string(),
        };
        let text = format!(
            "{} · {} · {} · {}",
            printable(&k.name, NAME_MAX),
            k.library_names.join(", "),
            expiry_words(k),
            claimed
        );
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    }
}

/// Peers: the paste box with its preview, then the table.
fn draw_peers(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(t!("fed.paste_label").to_string(), dim()),
            Span::styled(t!("fed.paste_hint").to_string(), dim()),
        ])),
        line(body.y),
    );
    let field = Rect { x: body.x, y: body.y + 1, width: body.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if room.ticket_focus { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if room.ticket_focus {
        kit::input_display(room.ticket.value(), room.ticket.cursor(), inner.width.saturating_sub(2))
    } else {
        clip(room.ticket.value(), inner.width.saturating_sub(2))
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    room.ui.click(field, Act::TicketFocus);
    // The preview: who and what, before the server is asked.
    let typed = room.ticket.value().trim();
    if !typed.is_empty() {
        let (text, style) = match decode_ticket(typed) {
            Some(p) => (preview_words(&p), Style::default().fg(th().ok)),
            None => (t!("fed.ticket_invalid").to_string(), Style::default().fg(th().gold)),
        };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), line(body.y + 4));
    }

    let table = Rect { x: body.x, y: body.y + 6, width: body.width, height: body.height.saturating_sub(6) };
    if table.height < 3 {
        return;
    }
    let (server_w, seen_w, disc_w) = (22u16, 12u16, 10u16);
    let fixed = server_w + 2 + seen_w + 2 + disc_w + 2;
    let status_w = table.width.saturating_sub(fixed).max(10);
    let status_x = table.x + server_w + 2;
    let seen_x = status_x + status_w + 2;
    let disc_x = seen_x + seen_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x + 2, server_w - 2, &t!("fed.col_server"), false),
            (status_x, status_w, &t!("fed.col_status"), false),
            (seen_x, seen_w, &t!("fed.col_last_seen"), false),
            (disc_x, disc_w, &t!("fed.col_discovery"), false),
        ],
    );
    if room.peers.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("fed.empty_peers").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let now = unix_now();
    let peers = room.peers.clone();
    table_rows(frame, room, rows_rect, peers.len(), |frame, room, i, rect, selected, hovered| {
        let p = &peers[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let (status, own) = match p.last_status.as_deref() {
            None => (t!("fed.never_tested").to_string(), dim()),
            Some("ok") => (t!("fed.status_ok").to_string(), Style::default().fg(th().ok)),
            Some(other) => (printable(other, 200), Style::default().fg(th().gold)),
        };
        frame.render_widget(Paragraph::new(Span::styled("•", cell_style(selected, hovered, own))), cell(rect.x, 1));
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(&p.name, NAME_MAX), server_w - 2), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x + 2, server_w - 2),
        );
        frame.render_widget(Paragraph::new(Span::styled(clip(&status, status_w), cell_style(selected, hovered, own))), cell(status_x, status_w));
        if status.chars().count() > status_w as usize {
            room.ui.tip(cell(status_x, status_w), status.clone());
        }
        let seen = match p.last_seen.as_deref() {
            Some(ts) => (age_of(Some(ts), now), base),
            None => (t!("fed.never").to_string(), cell_style(selected, hovered, dim())),
        };
        frame.render_widget(Paragraph::new(Span::styled(seen.0, seen.1)), cell(seen_x, seen_w));
        // The discovery opt-out: a checkbox wearing its state, clickable.
        let disc = cell(disc_x, 3);
        let disc_hover = room.ui.pointer.is_some_and(|pt| disc.contains(pt));
        let glyph = if p.use_discovery { g("[✓]", "[x]") } else { "[ ]" };
        let disc_style = match (selected, disc_hover, p.use_discovery) {
            (true, _, _) => base,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, disc_style)), disc);
        room.ui.click(disc, Act::ToggleDiscovery(p.id));
        room.ui.tip(disc, t!("fed.tip_discovery"));
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(p) = room.selected_peer()
    {
        let text = match room.shares.get(&p.id) {
            Some(libs) if !libs.is_empty() => t!("fed.peer_shares", name = printable(&p.name, NAME_MAX), libraries = libs.join(", ")).to_string(),
            Some(_) => t!("fed.peer_shares", name = printable(&p.name, NAME_MAX), libraries = t!("fed.shares_nothing")).to_string(),
            None => t!("fed.peer_untested_note", name = printable(&p.name, NAME_MAX)).to_string(),
        };
        frame.render_widget(Paragraph::new(Span::raw(text)), line(body.bottom() + 1));
    }
}

// ── Modals ───────────────────────────────────────────────────────────────────

/// A labelled 3-row input inside a modal.
fn modal_field(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let card = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = card.inner(field);
    frame.render_widget(card, field);
    let shown = if focused {
        kit::input_display(input.value(), input.cursor(), inner.width.saturating_sub(2))
    } else {
        clip(input.value(), inner.width.saturating_sub(2))
    };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: 1 });
    room.ui.click(field, act);
}

/// Checkbox rows in three columns; returns the rows used.
fn checkbox_grid(frame: &mut Frame, room: &mut Room, at: Rect, items: &[(String, bool)], focused: Option<usize>, field_index: impl Fn(usize) -> usize) -> u16 {
    let col_w = at.width / 3;
    for (i, (name, on)) in items.iter().enumerate() {
        let rect = Rect { x: at.x + (i as u16 % 3) * col_w, y: at.y + i as u16 / 3, width: col_w.saturating_sub(1), height: 1 };
        let is_focus = focused == Some(i);
        let hovered = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let box_style = if is_focus {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if *on {
            Style::default().fg(th().ok)
        } else {
            dim()
        };
        let glyph = if *on { format!("{} ", g("[✓]", "[x]")) } else { "[ ] ".to_string() };
        let name_style = if hovered { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if is_focus { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(glyph, box_style), Span::styled(clip(name, col_w.saturating_sub(5)), name_style)])),
            rect,
        );
        room.ui.click(rect, Act::FormToggle(field_index(i)));
    }
    items.len().div_ceil(3) as u16
}

/// The mint, accept and limits forms, one drawing.
fn draw_form(frame: &mut Frame, room: &mut Room, area: Rect, f: &Form) {
    let fields = f.fields();
    let focused = f.focused();
    let index_of = |field: Field| fields.iter().position(|x| *x == field).unwrap_or(0);
    let lib_rows = if f.kind == FormKind::Limits { 0 } else { f.libraries.len().div_ceil(3).max(1) as u16 };
    // Outer heights: the rows each layout below takes, plus the frame.
    let height = match f.kind {
        FormKind::Mint => 19 + lib_rows,
        FormKind::Accept => 15 + lib_rows + u16::from(!f.their_offer.is_empty()),
        FormKind::Limits => 18,
    };
    let inner = kit::modal_frame(frame, area, 68, height, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    let title = match f.kind {
        FormKind::Mint => t!("fed.mint_title"),
        FormKind::Accept => t!("fed.accept_title", name = f.about),
        FormKind::Limits => t!("fed.limits_title", name = f.about),
    };
    frame.render_widget(
        Paragraph::new(Span::styled(title.to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::FormCancel, t!("path_modal.tip_close"));
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let mut y = inner.y + 1;
    match f.kind {
        FormKind::Accept => {
            let mut spans = vec![Span::styled(format!("{} · ", short_id(&f.endpoint_id)), dim())];
            if f.message.is_empty() {
                spans.push(Span::styled(t!("fed.no_message").to_string(), dim()));
            } else {
                spans.push(Span::raw(format!("\u{201c}{}\u{201d}", f.message)));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), line(y));
            y += 2;
        }
        FormKind::Limits => {
            frame.render_widget(Paragraph::new(Span::styled(t!("fed.limits_note").to_string(), dim())), line(y));
            y += 2;
        }
        FormKind::Mint => {
            y += 1;
            modal_field(frame, room, Rect { x, y, width: 32.min(w), height: 4 }, &t!("fed.mint_name"), &f.name, focused == Field::Name, Act::FormFocus(index_of(Field::Name)));
            y += 5;
        }
    }
    if f.kind != FormKind::Limits {
        frame.render_widget(
            Paragraph::new(Span::styled(
                if f.kind == FormKind::Mint { t!("fed.mint_libraries") } else { t!("fed.accept_libraries") }.to_string(),
                dim(),
            )),
            line(y),
        );
        y += 1;
        if f.libraries.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled(t!("fed.no_libraries").to_string(), dim())), line(y));
            y += 1;
        } else {
            let focus_lib = if let Field::Library(i) = focused { Some(i) } else { None };
            let lib_offset = index_of(Field::Library(0));
            y += checkbox_grid(frame, room, Rect { x, y, width: w, height: lib_rows }, &f.libraries, focus_lib, |i| lib_offset + i);
        }
        y += 1;
    }
    frame.render_widget(Paragraph::new(Span::styled(t!("fed.limits_label").to_string(), dim())), line(y));
    y += 1;
    let fields_row: Vec<(Field, String, &Input)> = {
        let mut v = vec![
            (Field::StreamKbps, t!("fed.field_kbps").to_string(), &f.stream_kbps),
            (Field::DailyMb, t!("fed.field_daily").to_string(), &f.daily_mb),
            (Field::MaxStreams, t!("fed.field_streams").to_string(), &f.max_streams),
        ];
        if f.kind != FormKind::Limits {
            v.push((Field::Expires, t!("fed.field_expires").to_string(), &f.expires));
        }
        v
    };
    for (i, (field, label, input)) in fields_row.iter().enumerate() {
        let fx = x + i as u16 * 16;
        modal_field(frame, room, Rect { x: fx, y, width: 14.min(w.saturating_sub(i as u16 * 16)), height: 4 }, label, input, focused == *field, Act::FormFocus(index_of(*field)));
    }
    y += 5;
    match f.kind {
        FormKind::Limits => {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(t!("fed.expiry_current").to_string(), dim()),
                    Span::raw(f.current_expiry.clone()),
                ])),
                line(y),
            );
            y += 1;
            modal_field(frame, room, Rect { x, y, width: 14.min(w), height: 4 }, &t!("fed.field_new_expiry"), &f.expires, focused == Field::Expires, Act::FormFocus(index_of(Field::Expires)));
            frame.render_widget(
                Paragraph::new(Span::styled(t!("fed.expiry_hint").to_string(), dim())),
                Rect { x: x + 16, y: y + 2, width: w.saturating_sub(16), height: 1 },
            );
        }
        FormKind::Accept if !f.their_offer.is_empty() => {
            let rect = line(y);
            let is_focus = focused == Field::TheirOffer;
            let glyph = if f.accept_their_offer { format!("{} ", g("[✓]", "[x]")) } else { "[ ] ".to_string() };
            let box_style = if is_focus { Style::default().fg(th().on_accent).bg(th().accent) } else if f.accept_their_offer { Style::default().fg(th().ok) } else { dim() };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(glyph, box_style),
                    Span::styled(t!("fed.accept_their_offer", libraries = f.their_offer.join(", ")).to_string(), if is_focus { bold() } else { Style::default() }),
                ])),
                rect,
            );
            room.ui.click(rect, Act::FormToggle(index_of(Field::TheirOffer)));
        }
        _ => {}
    }
    if let Some(err) = &f.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            line(inner.bottom().saturating_sub(2)),
        );
    }
    let label = match f.kind {
        FormKind::Mint => t!("fed.mint_submit"),
        FormKind::Accept => t!("fed.accept_submit"),
        FormKind::Limits => t!("fed.limits_submit"),
    }
    .to_string();
    let bx = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x: bx, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, f.can_submit(), Act::FormSubmit);
}

/// The ticket to hand over: wrapped whole, y to copy, the credential
/// warning in the webapp's words.
fn draw_minted(frame: &mut Frame, room: &mut Room, area: Rect, name: &str, ticket: Option<&str>, libraries: &[String]) {
    let inner = kit::modal_frame(frame, area, 68, 17, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.minted_title", name = name).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::MintedDone, t!("path_modal.tip_close"));
    let body = Rect { x: inner.x + 1, y: inner.y + 1, width: inner.width.saturating_sub(2), height: 2 };
    frame.render_widget(
        Paragraph::new(t!("fed.minted_text", libraries = libraries.join(", ")).to_string()).wrap(Wrap { trim: false }),
        body,
    );
    match ticket {
        Some(ticket) => {
            frame.render_widget(
                Paragraph::new(printable(ticket, 4096)).wrap(Wrap { trim: false }),
                Rect { x: inner.x + 1, y: inner.y + 4, width: inner.width.saturating_sub(2), height: 6 },
            );
        }
        None => {
            frame.render_widget(
                Paragraph::new(Span::styled(t!("fed.minted_no_ticket").to_string(), Style::default().fg(th().gold))).wrap(Wrap { trim: false }),
                Rect { x: inner.x + 1, y: inner.y + 4, width: inner.width.saturating_sub(2), height: 3 },
            );
        }
    }
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.minted_note").to_string(), dim())).wrap(Wrap { trim: false }),
        Rect { x: inner.x + 1, y: inner.y + 11, width: inner.width.saturating_sub(2), height: 2 },
    );
    let y = inner.bottom().saturating_sub(1);
    let done = t!("fed.minted_done").to_string();
    let done_w = done.chars().count() as u16 + 4;
    let done_rect = kit::button(frame, &mut room.ui, Rect { x: inner.right().saturating_sub(done_w), y, width: done_w, height: 1 }, &done, true, Act::MintedDone);
    if ticket.is_some() {
        let copy = t!("fed.minted_copy").to_string();
        let copy_w = copy.chars().count() as u16 + 4;
        kit::button(frame, &mut room.ui, Rect { x: done_rect.x.saturating_sub(copy_w + 2), y, width: copy_w, height: 1 }, &copy, false, Act::MintedCopy);
    }
}

/// The pasted ticket's preview and the peer's display name, then Add.
fn draw_peer_name(frame: &mut Frame, room: &mut Room, area: Rect, name: &Input, preview: &TicketPreview) {
    let inner = kit::modal_frame(frame, area, 68, 11, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("fed.peer_name_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::PeerNameCancel, t!("path_modal.tip_close"));
    frame.render_widget(Paragraph::new(Span::styled(preview_words(preview), Style::default().fg(th().ok))), line(inner.y + 2));
    modal_field(frame, room, Rect { x: inner.x + 1, y: inner.y + 4, width: 40.min(inner.width.saturating_sub(2)), height: 4 }, &t!("fed.peer_name_label"), name, true, Act::PeerNameAdd);
    let label = t!("fed.peer_name_add").to_string();
    let x = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(frame, &mut room.ui, Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 }, &label, true, Act::PeerNameAdd);
}

/// A warning gate: gold, consequences before verbs, the safe choice as
/// the primary, no [X].
fn draw_gate(frame: &mut Frame, room: &mut Room, area: Rect, title: String, body: Vec<String>, safe: (String, Act), go: (String, Act)) {
    let inner = kit::modal_frame(frame, area, 68, 7 + body.len() as u16, th().gold);
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::PeerHealth;
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

    fn new_room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    fn hex(seed: &str) -> String {
        seed.repeat(8).chars().take(64).collect()
    }

    /// SQLite's UTC form, as the federation rows carry it.
    fn sqlite_at(t: i64) -> String {
        iso_at(t).replace('T', " ")[..19].to_string()
    }

    fn params_on() -> FederationParams {
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

    fn params_off() -> FederationParams {
        FederationParams { enabled: false, running: false, endpoint_id: None, online: false, ..params_on() }
    }

    fn request(id: i64, name: &str, seed: &str, direction: &str, state: &str, ago: i64) -> FederationRequest {
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

    fn requests() -> Vec<FederationRequest> {
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

    fn key(id: i64, name: &str, libs: &[&str], limits: (u64, u64, u64), bound: bool) -> FederationKey {
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

    fn keys() -> Vec<FederationKey> {
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

    fn peer(id: i64, name: &str, status: Option<&str>, seen_ago: Option<i64>, discovery: bool) -> FederationPeer {
        FederationPeer {
            id,
            name: name.into(),
            last_status: status.map(str::to_string),
            last_seen: seen_ago.map(|ago| sqlite_at(unix_now() - ago)),
            use_discovery: discovery,
            added_at: sqlite_at(unix_now() - 30 * 86_400),
        }
    }

    fn peers() -> Vec<FederationPeer> {
        vec![
            peer(1, "Basement Archive", Some("ok"), Some(10), true),
            peer(2, "studio-nas", Some("ok"), Some(180), false),
            peer(3, "jazz-corner", Some("dial timed out after 20 s"), Some(3 * 86_400), true),
            peer(4, "Ada's laptop", None, None, true),
        ]
    }

    fn loaded(params: FederationParams) -> Done {
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

    fn on() -> Room {
        let mut room = new_room();
        room.apply(loaded(params_on()));
        room
    }

    fn key_press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key_press(code))
    }

    fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
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

    fn row(frame: &str, name: &str) -> String {
        frame.lines().find(|l| l.contains(name)).map(str::to_string).unwrap_or_default()
    }

    fn ticket_for(payload: serde_json::Value) -> String {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("mstrfed1:{body}")
    }

    #[test]
    fn off_the_page_turns_the_endpoint_on_with_enter() {
        let _en = english();
        let mut room = new_room();
        room.apply(loaded(params_off()));
        let frame = draw(&mut room);
        assert!(frame.contains("• off — federation is not running"), "{frame}");
        assert!(frame.contains("Turn federation on"), "{frame}");
        assert!(frame.contains("TICKETS ARE CREDENTIALS"), "{frame}");
        assert!(frame.contains("Enter turn on · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetEnabled(true)));
        room.queued = None;
        // While the start is on its way the card is dim, says so, and takes no second press.
        let frame = draw(&mut room);
        assert!(frame.contains("Starting federation…") && !frame.contains("Turn federation on"), "{frame}");
        assert!(frame.contains("starting the federation endpoint…"), "{frame}");
        assert!(!frame.contains("Enter turn on"), "{frame}");
        press(&mut room, KeyCode::Enter);
        room.act(Act::TurnOn);
        assert!(room.queued.is_none(), "one start at a time");
        // The platform without an Iroh binary says so, in gold.
        room.apply(Done::Enabled { on: true, result: Ok(serde_json::json!({ "enabled": true, "available": false })) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("no prebuilt binary")));
        assert_eq!(room.queued, Some(Op::Load));
        let mut unavailable = params_off();
        unavailable.available = false;
        room.apply(loaded(unavailable));
        let frame = draw(&mut room);
        assert!(frame.contains("• unavailable — Iroh has no prebuilt binary"), "{frame}");
        assert!(!frame.contains("Turn federation on") && !frame.contains("Starting federation…"), "{frame}");
        // A failed start hands the card back.
        room.apply(loaded(params_off()));
        press(&mut room, KeyCode::Enter);
        assert!(room.starting);
        room.queued = None;
        room.apply(Done::Enabled { on: true, result: Err(ApiError::Server { status: 500, message: "relay down".into() }) });
        assert!(!room.starting);
        assert!(draw(&mut room).contains("Turn federation on"));
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn the_state_row_and_the_tabs() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("• on — connected to relay · endpoint 51403e7b9c51… · inbox open"), "{frame}");
        assert!(frame.contains("polls every 30 s"), "{frame}");
        assert!(frame.contains(" Requests · 1 ") && frame.contains(" Tickets ") && frame.contains(" Peers "), "{frame}");
        assert!(frame.contains("no access changes hands until a request is accepted"), "{frame}");
        let mut connecting = params_on();
        connecting.online = false;
        room.apply(loaded(connecting));
        let frame = draw(&mut room);
        assert!(frame.contains("• on — connecting to the relay…"), "{frame}");
        assert!(frame.contains("▱▱▱▱▱▱▱▱▱▱ the endpoint is coming up"), "{frame}");
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Tickets);
        press(&mut room, KeyCode::Left);
        press(&mut room, KeyCode::Left);
        assert_eq!(room.tab, Tab::Peers, "the tabs wrap");
        assert!(draw(&mut room).contains("servers you can read"));
    }

    #[test]
    fn requests_show_the_webapps_states_and_act_by_row() {
        let _en = english();
        let mut room = on();
        let frame = draw(&mut room);
        assert!(frame.contains("[✓] Accept requests from the discovery network"), "{frame}");
        for word in ["SERVER", "MESSAGE / OFFER", "STATUS", "AGE"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let friend = row(&frame, "friend-node");
        assert!(friend.contains("\u{201c}Jazz for field recordings?\u{201d} · offers: ambient") && friend.contains("needs your answer") && friend.contains("2h"), "{friend}");
        let studio = row(&frame, "studio-nas");
        assert!(studio.contains("you offered: music, vinyl") && studio.contains("waiting on them") && studio.contains("24h"), "{studio}");
        assert!(row(&frame, "Basement Archive").contains("sending… · retry #2"), "{frame}");
        assert!(row(&frame, "jazz-corner").contains("federated"), "{frame}");
        assert!(row(&frame, "Ada's laptop").contains("declined"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · s inbox · x turn off · Esc back"), "{frame}");
        // The cursor row: only the keys that apply, and its story on the note line.
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("↑↓ rows · a accept · n decline · Esc deselect"), "{frame}");
        assert!(frame.contains("friend-node · c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d} · offers: ambient"), "{frame}");
        press(&mut room, KeyCode::Char('a'));
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!((f.kind, f.id, f.about.as_str()), (FormKind::Accept, 1, "friend-node"));
        assert_eq!(f.their_offer, vec!["ambient".to_string()]);
        assert!(f.libraries.iter().all(|(_, on)| !on), "nothing pre-checked: they initiated");
        press(&mut room, KeyCode::Esc);
        press(&mut room, KeyCode::Char('n'));
        assert!(matches!(room.modal, Modal::Decline(1)));
        let frame = draw(&mut room);
        assert!(frame.contains("Decline friend-node's request?") && frame.contains("ignored for seven days") && frame.contains("y decline · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('n'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Reject(1)));
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Down); // studio-nas, out · delivered
        assert!(draw(&mut room).contains("↑↓ rows · c withdraw · Esc deselect"));
        press(&mut room, KeyCode::Char('a'));
        assert!(matches!(room.modal, Modal::None), "only an inbound, received request can be accepted");
        press(&mut room, KeyCode::Char('c'));
        assert_eq!(room.queued, Some(Op::Cancel(2)));
        room.queued = None;
        room.busy = None;
        for _ in 0..3 {
            press(&mut room, KeyCode::Down);
        }
        let frame = draw(&mut room);
        assert!(frame.contains("↑↓ rows · d dismiss · Esc deselect"), "{frame}");
        assert!(frame.contains("Ada's laptop · 05d6e8f2ac05… · you offered: music · \u{201c}not now, sorry\u{201d}"), "{frame}");
        press(&mut room, KeyCode::Char('d'));
        assert_eq!(room.queued, Some(Op::Dismiss(5)));
        room.queued = None;
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::SetInbox(false)));
        room.queued = None;
        room.apply(Done::Did { what: Did::Inbox(false), result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("inbox closed")));
    }

    #[test]
    fn the_accept_form_walks_its_fields_and_sends_what_was_ticked() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('a'));
        // Nothing ticked: refused before the server sees it.
        press(&mut room, KeyCode::Enter);
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!(f.error.as_deref(), Some("name it and tick at least one library"));
        assert!(room.queued.is_none());
        let frame = draw(&mut room);
        assert!(frame.contains("Accept — share libraries with friend-node"), "{frame}");
        assert!(frame.contains("c4a8f0e19dc4… · \u{201c}Jazz for field recordings?\u{201d}"), "{frame}");
        assert!(frame.contains("LIBRARIES THEY CAN READ") && frame.contains("[ ] music") && frame.contains("STREAM KBPS"), "{frame}");
        assert!(frame.contains("[✓] Add their libraries too (ambient) when they share back"), "{frame}");
        assert!(frame.contains("Tab next field · Space toggle · Enter accept · Esc cancel"), "{frame}");
        press(&mut room, KeyCode::Char(' ')); // music
        assert!(draw(&mut room).contains("[✓] music"));
        for _ in 0..8 {
            press(&mut room, KeyCode::Tab); // past the five libraries and three caps to Expires
        }
        let Modal::Form(f) = &room.modal else { panic!("the accept form") };
        assert_eq!(f.focused(), Field::Expires);
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "7x");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Char(' ')); // their offer: off
        press(&mut room, KeyCode::Enter);
        match room.queued.take() {
            Some(Op::Accept { id, vpaths, limits, expires_at, accept_their_offer }) => {
                assert_eq!(id, 1);
                assert_eq!(vpaths, vec!["music".to_string()]);
                assert_eq!(limits, FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 });
                let cutoff = expires_at.as_deref().and_then(iso_unix).expect("an ISO cutoff");
                assert!((cutoff - unix_now() - 7 * 86_400).abs() < 5, "seven days out, the x dropped");
                assert!(!accept_their_offer);
            }
            other => panic!("{other:?}"),
        }
        room.apply(Done::Did { what: Did::Accepted, result: Err(ApiError::Server { status: 409, message: "cannot accept a request in state 'accepted'".into() }) });
        let Modal::Form(f) = &room.modal else { panic!("a refusal keeps the form") };
        assert!(f.error.as_deref().is_some_and(|e| e.contains("cannot accept")));
        room.apply(Done::Did { what: Did::Accepted, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("accepted")));
        assert_eq!(room.queued, Some(Op::Load));
    }

    #[test]
    fn minting_shows_the_ticket_to_hand_over() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Char('n'));
        let Modal::Form(f) = &room.modal else { panic!("the mint form") };
        assert_eq!((f.kind, f.focused()), (FormKind::Mint, Field::Name));
        type_text(&mut room, "Bob's NAS|");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Char(' '));
        let frame = draw(&mut room);
        assert!(frame.contains("New federation ticket") && frame.contains("WHO IS THIS FOR?") && frame.contains("Bob's NAS"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("EXPIRES (DAYS)") && frame.contains("Mint ticket ▸"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(
            room.queued,
            Some(Op::Mint {
                name: "Bob's NAS".into(),
                vpaths: vec!["music".into()],
                limits: FederationLimits { stream_kbps: 8000, daily_mb: 2048, max_streams: 3 },
                expires_at: None,
            }),
            "the pipe never reaches the name; 0 days means never"
        );
        room.queued = None;
        room.apply(Done::Minted {
            libraries: vec!["music".into()],
            result: Ok(MintedKey { id: 9, name: "Bob's NAS".into(), ticket: Some("mstrfed1:eyJ0IjoibmV3In0".into()) }),
        });
        assert!(matches!(room.modal, Modal::Minted { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Ticket for Bob's NAS") && frame.contains("mstrfed1:eyJ0IjoibmV3In0"), "{frame}");
        assert!(frame.contains("can read music until it is claimed or revoked"), "{frame}");
        assert!(frame.contains("y copy") && frame.contains("Done") && frame.contains("y copy · Esc done"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(matches!(room.modal, Modal::None));
        room.apply(Done::Minted { libraries: vec!["music".into()], result: Ok(MintedKey { id: 10, name: "x".into(), ticket: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("no ticket")));
    }

    #[test]
    fn tickets_show_limits_and_claims_and_act_by_row() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Right);
        let frame = draw(&mut room);
        for word in ["Mint a ticket for a friend", "NAME", "LIBRARIES", "LIMITS", "TODAY", "LAST USED", "CLAIMED"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let ada = row(&frame, "Ada's laptop");
        assert!(ada.contains("music, vinyl") && ada.contains("8 Mbps · 2 GB/day · 3 streams") && ada.contains("218 MB") && ada.contains("1h") && ada.contains("✓ claimed"), "{ada}");
        let studio = row(&frame, "studio-nas");
        assert!(studio.contains("music, vinyl, j…") && studio.contains("unlimited") && studio.contains("not yet"), "{studio}");
        assert!(row(&frame, "jazz-corner").contains("2 Mbps · 500 MB/day · 1 stream"), "{frame}");
        assert!(row(&frame, "Bob's NAS").contains("expired"), "{frame}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("Ada's laptop · music, vinyl · never expires · claimed by 8f31c0e2a78f… 2d ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · y copy ticket · l limits · b reset binding · r revoke · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('l'));
        let Modal::Form(f) = &room.modal else { panic!("the limits form") };
        assert_eq!((f.kind, f.id, f.about.as_str(), f.current_expiry.as_str()), (FormKind::Limits, 1, "Ada's laptop", "never expires"));
        assert_eq!(f.stream_kbps.value(), "8000");
        let frame = draw(&mut room);
        assert!(frame.contains("Limits for Ada's laptop") && frame.contains("EXPIRY · currently never expires") && frame.contains("NEW EXPIRY"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { id: 1, expiry: ExpiryChange::Keep, .. })), "blank keeps the expiry");
        room.queued = None;
        for _ in 0..3 {
            press(&mut room, KeyCode::Tab);
        }
        type_text(&mut room, "0");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { expiry: ExpiryChange::Never, .. })), "0 means never");
        room.queued = None;
        press(&mut room, KeyCode::Backspace);
        type_text(&mut room, "30");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.queued, Some(Op::Limits { expiry: ExpiryChange::At(_), .. })), "30 means thirty days from now");
        room.queued = None;
        room.apply(Done::Did { what: Did::LimitsSaved, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("limits saved")));
        room.note = None;
        room.queued = None;
        press(&mut room, KeyCode::Char('b'));
        assert_eq!(room.queued, Some(Op::ResetBinding(1)));
        room.queued = None;
        room.busy = None;
        press(&mut room, KeyCode::Down); // studio-nas: not claimed
        press(&mut room, KeyCode::Char('b'));
        assert!(room.queued.is_none(), "nothing to reset on an unclaimed ticket");
        assert!(draw(&mut room).contains("studio-nas · music, vinyl, jazz · expires in 24 days · not yet claimed"));
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Revoke(2)));
        let frame = draw(&mut room);
        // The gate wraps its sentence to the modal's width.
        assert!(frame.contains("Revoke studio-nas's ticket?") && frame.contains("studio-nas can no longer") && frame.contains("read music, vinyl, jazz.") && frame.contains("y revoke · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Revoke(2)));
    }

    #[test]
    fn peers_preview_the_pasted_ticket_and_act_by_row() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Char('j'));
        assert_eq!(room.tab, Tab::Peers);
        assert!(room.ticket_focus);
        type_text(&mut room, "garbage");
        let frame = draw(&mut room);
        assert!(frame.contains("that doesn't look like a federation ticket"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("doesn't look like")));
        for _ in 0.."garbage".len() {
            press(&mut room, KeyCode::Backspace);
        }
        let ticket = ticket_for(serde_json::json!({
            "t": "nodeaa4qk3r7z2nlxwm5c6bdhpj2ev5gktr3q7hifobyq2j5ru4kiggizaaaq",
            "k": "f34b91c0d2e7a89b1c2d3e4f5a6b7c8d",
            "n": "mStream Den",
            "l": ["music", "vinyl"],
            "e": "2026-12-01T00:00:00.000Z",
        }));
        type_text(&mut room, &ticket);
        let frame = draw(&mut room);
        assert!(frame.contains("mStream Den — shares music, vinyl · valid until 2026-12-01"), "{frame}");
        assert!(frame.contains("type or paste the ticket · Enter add · Esc leave the field"), "{frame}");
        press(&mut room, KeyCode::Enter);
        let Modal::PeerName { name, .. } = &room.modal else { panic!("the name modal") };
        assert_eq!(name.value(), "mStream Den");
        let frame = draw(&mut room);
        assert!(frame.contains("Name this peer") && frame.contains("Add peer ▸"), "{frame}");
        type_text(&mut room, " 2");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::AddPeer { ticket: ticket.clone(), name: Some("mStream Den 2".into()) }));
        room.queued = None;
        room.apply(Done::Did { what: Did::PeerAdded, result: Ok(()) });
        assert!(matches!(room.modal, Modal::None) && room.ticket.value().is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("peer added")));

        room.note = None;
        let frame = draw(&mut room);
        let basement = row(&frame, "Basement Archive");
        assert!(basement.starts_with("  • Basement Archive") && basement.contains(" ok ") && basement.contains("[✓]"), "{basement}");
        assert!(row(&frame, "studio-nas").contains("[ ]"), "{frame}");
        let jazz = row(&frame, "jazz-corner");
        assert!(jazz.contains("dial timed out after 20 s") && jazz.contains("3d"), "{jazz}");
        let ada = row(&frame, "Ada's laptop");
        assert!(ada.contains("never tested") && ada.contains("never"), "{ada}");
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("Basement Archive · t tests the connection"), "{frame}");
        assert!(frame.contains("↑↓ rows · t test · s discovery · r forget · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('t'));
        assert_eq!(room.queued, Some(Op::Test(1)));
        room.queued = None;
        room.apply(Done::Tested {
            id: 1,
            result: Ok(PeerTest { ok: true, error: None, health: Some(PeerHealth { libraries: vec!["music".into(), "vinyl".into()] }) }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n == "Basement Archive shares: music, vinyl"));
        room.note = None;
        assert!(draw(&mut room).contains("Basement Archive shares: music, vinyl"), "the answer stays on the note line");
        room.apply(Done::Tested { id: 1, result: Ok(PeerTest { ok: false, error: Some("dial timed out".into()), health: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("unreachable: dial timed out")));
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::PeerDiscovery { id: 1, on: false }));
        room.queued = None;
        room.apply(Done::Did { what: Did::PeerDiscovery(1, false), result: Ok(()) });
        assert!(!room.peers[0].use_discovery);
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Forget(1)));
        assert!(draw(&mut room).contains("Forget Basement Archive?"));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::RemovePeer(1)));
    }

    #[test]
    fn turning_off_is_gated_and_the_poll_is_quiet() {
        let _en = english();
        let mut room = on();
        press(&mut room, KeyCode::Char('x'));
        assert!(matches!(room.modal, Modal::TurnOff));
        let frame = draw(&mut room);
        assert!(frame.contains("Turn federation off?") && frame.contains("◂ Stay on") && frame.contains("y turn off · Esc stay on"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert!(room.queued.is_none());
        press(&mut room, KeyCode::Char('x'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::SetEnabled(false)));
        room.queued = None;
        room.apply(loaded(params_off()));
        assert!(room.requests.is_empty() && room.keys.is_empty() && room.peers.is_empty());

        let mut room = new_room();
        room.tick();
        assert!(room.queued.is_none(), "nothing before the first load answers");
        room.apply(loaded(params_off()));
        room.tick();
        assert!(room.queued.is_none(), "off, no poll");
        room.apply(loaded(params_on()));
        room.tick();
        assert_eq!(room.queued, Some(Op::Load));
        assert!(room.busy.is_none(), "the poll shows no busy line");
        room.last_load = Some(Instant::now());
        room.queued = None;
        room.tick();
        assert!(room.queued.is_none(), "not again within the cadence");
        room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("locked")));
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
}
