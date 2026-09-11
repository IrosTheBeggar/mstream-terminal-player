//! The Torrents room: the webapp's Torrent Client page (beta), in the kit.
//!
//! Two phases. Until a client is chosen and connected, the page is the
//! setup: the client as a radio group (Transmission, qBittorrent, Deluge),
//! who may add torrents beneath it when the server has more than one user,
//! and the connect form — `^T` probes without saving, Enter saves after a
//! good probe. Connected, one state line carries the webapp's status card
//! and five tabs follow: Torrents (the daemon's list), Libraries (the
//! daemon-side path of each library and its destination template), Seeding
//! (hand the server a `.torrent` for content already on disk), Access (the
//! policy and the whitelist; only with more than one user) and Client (the
//! connection). Anything with fields is a modal; removals and the
//! disconnect are gold gates.
//!
//! Every server call runs on a worker thread (the wizard's Job/Done
//! pattern). The daemon's list is polled every five seconds while the
//! Torrents tab shows, everything else every thirty.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{
    Outcome, Screen, age_text, draw_bottom, draw_header, fmt_bytes, frame_ground, gate_message,
    host_of, iso_unix, printable, short_id, unix_now,
};
use crate::api::types::{
    AccessRow, AdminUser, PathTemplates, ProbeAnswer, RemoveAnswer, SeedOutcome, TemplateSaved,
    Torrent, TorrentClientConfig, TorrentList, TorrentParams, TorrentStatus, VpathAccess,
};
use crate::api::{ApiError, Client, TorrentCreds};
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;
use crate::setup::picker::{self, FilePick};

/// The daemon's list, while the Torrents tab shows.
const POLL_LIST: Duration = Duration::from_secs(5);
/// Everything else: the status probe, the libraries, the users.
const POLL_STATE: Duration = Duration::from_secs(30);
const MIN_W: u16 = 80;
const MIN_H: u16 = 24;
const TAB_GAP: u16 = 2;
/// Names on screen (users, libraries, daemon versions).
const NAME_MAX: usize = 64;
/// The server's cap on a template.
const TEMPLATE_MAX: usize = 500;
const PATH_MAX: usize = 400;
const PORT_MAX: u32 = 65_535;
/// How many completions the seed-path modal lists.
const SUGGEST_MAX: usize = 6;

// ── State ────────────────────────────────────────────────────────────────────

/// The four things `torrent.client` can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Disabled,
    Transmission,
    Qbittorrent,
    Deluge,
}

impl Kind {
    pub(crate) const ALL: [Kind; 4] = [Kind::Disabled, Kind::Transmission, Kind::Qbittorrent, Kind::Deluge];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Disabled => "disabled",
            Kind::Transmission => "transmission",
            Kind::Qbittorrent => "qbittorrent",
            Kind::Deluge => "deluge",
        }
    }

    pub(crate) fn parse(s: &str) -> Kind {
        match s {
            "transmission" => Kind::Transmission,
            "qbittorrent" => Kind::Qbittorrent,
            "deluge" => Kind::Deluge,
            _ => Kind::Disabled,
        }
    }

    fn index(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    /// The daemon's name as the webapp spells it.
    pub(crate) fn label(self) -> String {
        match self {
            Kind::Disabled => t!("tor.client_disabled"),
            Kind::Transmission => t!("tor.client_transmission"),
            Kind::Qbittorrent => t!("tor.client_qbittorrent"),
            Kind::Deluge => t!("tor.client_deluge"),
        }
        .to_string()
    }

    fn description(self) -> String {
        match self {
            Kind::Disabled => t!("tor.client_disabled_desc"),
            Kind::Transmission => t!("tor.client_transmission_desc"),
            Kind::Qbittorrent => t!("tor.client_qbittorrent_desc"),
            Kind::Deluge => t!("tor.client_deluge_desc"),
        }
        .to_string()
    }

    fn default_port(self) -> u16 {
        match self {
            Kind::Disabled => 0,
            Kind::Transmission => 9091,
            Kind::Qbittorrent => 8080,
            Kind::Deluge => 8112,
        }
    }

    /// Deluge's WebUI takes a password only.
    fn has_username(self) -> bool {
        matches!(self, Kind::Transmission | Kind::Qbittorrent)
    }

    /// Only Transmission mounts its RPC at a path the operator can move.
    fn has_rpc_path(self) -> bool {
        self == Kind::Transmission
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Torrents,
    Libraries,
    Seeding,
    Access,
    Client,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Torrents, Tab::Libraries, Tab::Seeding, Tab::Access, Tab::Client];
}

/// Which page the room is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Loading,
    /// The client radio group (and the policy beneath it).
    Choose,
    /// Credentials for the chosen client.
    Connect,
    /// The state line and the tabs.
    Tabs,
}

/// One connect-form field, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CField {
    Host,
    Port,
    Username,
    Password,
    RpcPath,
    Https,
}

/// The webapp's "Connect to <client>" card.
#[derive(Debug, Clone)]
pub(crate) struct Connect {
    pub kind: Kind,
    pub host: Input,
    pub port: Input,
    pub username: Input,
    pub password: Input,
    pub rpc_path: Input,
    pub https: bool,
    /// Index into [`Connect::fields`].
    pub focus: usize,
    /// The last probe of this draft: the daemon's version and RPC number,
    /// or its failure sentence.
    pub probe: Option<Result<(String, String), String>>,
    pub error: Option<String>,
}

impl Connect {
    fn new(kind: Kind, saved: &TorrentClientConfig) -> Self {
        let port = if saved.port > 0 { saved.port } else { kind.default_port() };
        let rpc = saved.rpc_path.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| "/transmission/rpc".to_string());
        Connect {
            kind,
            host: Input::new(saved.host.clone()),
            port: Input::new(port.to_string()),
            username: Input::new(saved.username.clone()),
            password: Input::default(),
            rpc_path: Input::new(rpc),
            https: saved.use_https,
            focus: 0,
            probe: None,
            error: None,
        }
    }

    pub(crate) fn fields(&self) -> Vec<CField> {
        let mut f = vec![CField::Host, CField::Port];
        if self.kind.has_username() {
            f.push(CField::Username);
        }
        f.push(CField::Password);
        if self.kind.has_rpc_path() {
            f.push(CField::RpcPath);
        }
        f.push(CField::Https);
        f
    }

    pub(crate) fn focused(&self) -> CField {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    fn input_mut(&mut self, field: CField) -> Option<&mut Input> {
        match field {
            CField::Host => Some(&mut self.host),
            CField::Port => Some(&mut self.port),
            CField::Username => Some(&mut self.username),
            CField::Password => Some(&mut self.password),
            CField::RpcPath => Some(&mut self.rpc_path),
            CField::Https => None,
        }
    }

    /// The credentials the draft describes, or what is missing.
    pub(crate) fn creds(&self) -> Result<TorrentCreds, String> {
        let host = self.host.value().trim().to_string();
        if host.is_empty() {
            return Err(t!("tor.err_host").to_string());
        }
        let port = match self.port.value().trim().parse::<u32>() {
            Ok(p) if (1..=PORT_MAX).contains(&p) => p as u16,
            _ => return Err(t!("tor.err_port").to_string()),
        };
        Ok(TorrentCreds {
            host,
            port,
            username: self.kind.has_username().then(|| self.username.value().trim().to_string()),
            password: self.password.value().to_string(),
            rpc_path: self.kind.has_rpc_path().then(|| {
                let p = self.rpc_path.value().trim();
                if p.is_empty() { "/transmission/rpc".to_string() } else { p.to_string() }
            }),
            use_https: self.https,
        })
    }
}

/// One `.torrent` handed in for seeding and what the server said.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SeedRow {
    pub file: String,
    /// `None` while the check is on its way.
    pub outcome: Option<SeedOutcome>,
    /// The request itself failed (the file unreadable, the server away).
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Modal {
    None,
    /// `m`: the daemon's path for one library, verified on save.
    Mapping { vpath: String, path: Input, error: Option<String> },
    /// `t`: one library's destination template.
    Template { vpath: String, template: Input, error: Option<String> },
    /// `a` on Seeding without the OS dialog: a local `.torrent` path.
    SeedPath { path: Input, matches: Vec<String>, error: Option<String> },
    /// `r`: the gate before a torrent leaves the daemon (files stay).
    Remove(String),
    /// `x`: the gate before the credentials are forgotten.
    Disconnect,
}

/// Everything a click or a key can ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Tab(Tab),
    Select(usize),
    TableScroll(i8),
    TableScrollTo(usize),
    PickClient(usize),
    PickPolicy(usize),
    ApplyClient,
    ConnectFocus(usize),
    ConnectHttps,
    ConnectTest,
    ConnectSubmit,
    ConnectBack,
    FilterFocus,
    Remove(String),
    RemoveConfirm,
    RemoveCancel,
    AutoDetect(Option<String>),
    Mapping(String),
    Template(String),
    TemplateSuggest,
    TemplateClear,
    ModalSubmit,
    ModalCancel,
    SeedAdd,
    SeedTick(usize),
    SeedClear,
    UserToggle(String),
    ClientTest,
    ClientSwitch,
    Disconnect,
    DisconnectConfirm,
    DisconnectCancel,
    Quit,
}

/// Work for the worker thread. Ops carry everything they need.
#[derive(Debug, Clone, PartialEq)]
enum Op {
    /// Params and users; then, for a configured client, the status probe,
    /// the libraries' access and templates, and the daemon's list.
    Load,
    /// The daemon's list alone — the five-second poll.
    List,
    SetClient(Kind),
    SetPolicy(String),
    UserAccess { user: String, allow: bool },
    Probe { kind: Kind, creds: TorrentCreds, connect: bool },
    Disconnect(Kind),
    Status,
    Remove(String),
    Detect(Option<String>),
    Map { vpath: String, path: String },
    Template { vpath: String, template: Option<String> },
    Seed { path: PathBuf, vpaths: Vec<String> },
    PickFile,
}

/// What one `Op::Load` brought back. The daemon-facing parts are optional:
/// a daemon that is down must not hide the settings.
struct Loaded {
    params: TorrentParams,
    users: BTreeMap<String, AdminUser>,
    status: Option<TorrentStatus>,
    access: Option<VpathAccess>,
    templates: Option<PathTemplates>,
    list: Option<TorrentList>,
    /// The first daemon-facing call that failed, in words.
    warning: Option<String>,
}

/// What the worker sends back for each [`Op`].
#[allow(clippy::large_enum_variant)]
enum Done {
    Loaded(Result<Box<Loaded>, ApiError>),
    Listed(Result<TorrentList, ApiError>),
    Status(Result<TorrentStatus, ApiError>),
    ClientSet { kind: Kind, result: Result<(), ApiError> },
    PolicySet { policy: String, result: Result<(), ApiError> },
    UserAccess { user: String, allow: bool, result: Result<(), ApiError> },
    Probed { kind: Kind, connect: bool, result: Result<ProbeAnswer, ApiError> },
    Disconnected(Result<(), ApiError>),
    Removed { hash: String, result: Result<RemoveAnswer, ApiError> },
    Detected(Result<VpathAccess, ApiError>),
    Mapped { vpath: String, result: Result<serde_json::Value, ApiError> },
    TemplateSaved { vpath: String, result: Result<TemplateSaved, ApiError> },
    Seeded { file: String, result: Result<SeedOutcome, String> },
    Picked(FilePick),
}

fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load => Done::Loaded(load(&client).map(Box::new)),
                Op::List => Done::Listed(client.admin_torrent_list()),
                Op::SetClient(kind) => Done::ClientSet { kind, result: client.admin_torrent_set_client(kind.as_str()).map(|_| ()) },
                Op::SetPolicy(policy) => {
                    let result = client.admin_torrent_set_policy(&policy).map(|_| ());
                    Done::PolicySet { policy, result }
                }
                Op::UserAccess { user, allow } => {
                    let result = client.admin_user_torrent_access(&user, allow).map(|_| ());
                    Done::UserAccess { user, allow, result }
                }
                Op::Probe { kind, creds, connect } => {
                    Done::Probed { kind, connect, result: client.admin_torrent_probe(kind.as_str(), &creds, connect) }
                }
                Op::Disconnect(kind) => Done::Disconnected(client.admin_torrent_disconnect(kind.as_str()).map(|_| ())),
                Op::Status => Done::Status(client.admin_torrent_status()),
                Op::Remove(hash) => {
                    let result = client.admin_torrent_remove(&hash);
                    Done::Removed { hash, result }
                }
                Op::Detect(vpath) => Done::Detected(client.admin_torrent_auto_detect(vpath.as_deref())),
                Op::Map { vpath, path } => {
                    let result = client.admin_torrent_manual_mapping(&vpath, &path);
                    Done::Mapped { vpath, result }
                }
                Op::Template { vpath, template } => {
                    let result = client.admin_torrent_set_template(&vpath, template.as_deref());
                    Done::TemplateSaved { vpath, result }
                }
                Op::Seed { path, vpaths } => {
                    let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                    let result = match std::fs::read(&path) {
                        Ok(bytes) => client.admin_torrent_seed_existing(&file, &bytes, &vpaths).map_err(|e| e.to_string()),
                        Err(e) => Err(t!("tor.err_seed_read", err = e.to_string()).to_string()),
                    };
                    Done::Seeded { file, result }
                }
                Op::PickFile => Done::Picked(picker::pick_file(&t!("tor.seed_path_title"))),
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

/// The whole load, on the worker: the settings first (those gate
/// everything), then what a configured client can tell.
fn load(client: &Client) -> Result<Loaded, ApiError> {
    let params = client.admin_torrent_params()?;
    let users = client.admin_users().unwrap_or_default();
    let kind = Kind::parse(&params.client);
    let mut loaded = Loaded { params, users, status: None, access: None, templates: None, list: None, warning: None };
    if kind != Kind::Disabled && config_of(&loaded.params, kind).configured {
        let warn = |e: ApiError, w: &mut Option<String>| {
            if w.is_none() {
                *w = Some(e.to_string());
            }
        };
        match client.admin_torrent_status() {
            Ok(s) => loaded.status = Some(s),
            Err(e) => warn(e, &mut loaded.warning),
        }
        match client.admin_torrent_vpath_access() {
            Ok(a) => loaded.access = Some(a),
            Err(e) => warn(e, &mut loaded.warning),
        }
        match client.admin_torrent_path_templates() {
            Ok(t) => loaded.templates = Some(t),
            Err(e) => warn(e, &mut loaded.warning),
        }
        match client.admin_torrent_list() {
            Ok(l) => loaded.list = Some(l),
            Err(e) => warn(e, &mut loaded.warning),
        }
    }
    Ok(loaded)
}

fn config_of(p: &TorrentParams, kind: Kind) -> &TorrentClientConfig {
    match kind {
        Kind::Qbittorrent => &p.qbittorrent,
        Kind::Deluge => &p.deluge,
        _ => &p.transmission,
    }
}

pub(crate) struct Room {
    client: Arc<Client>,
    to_worker: Sender<(Arc<Client>, Op)>,
    from_worker: Receiver<Done>,
    /// Booted with `--same-machine`: the OS file dialog picks a `.torrent`.
    same_machine: bool,
    /// `None` until the first load answers.
    pub params: Option<TorrentParams>,
    pub users: BTreeMap<String, AdminUser>,
    pub status: Option<TorrentStatus>,
    /// When the status last answered, for "reachable N ago".
    status_at: Option<Instant>,
    pub torrents: Vec<Torrent>,
    pub list_error: Option<String>,
    list_loaded: bool,
    pub access: BTreeMap<String, AccessRow>,
    pub templates: PathTemplates,
    /// Every library name the server knows, sorted.
    pub libraries: Vec<String>,
    pub seeds: Vec<SeedRow>,
    /// The SEARCH IN ticks, one per library.
    pub seed_ticks: Vec<(String, bool)>,
    /// `.torrent` files waiting their turn on the worker.
    seed_queue: VecDeque<(PathBuf, Vec<String>)>,
    /// The client page shown on purpose (`c` on the Client tab) while a
    /// client is configured.
    pub choosing: bool,
    /// The radio groups on the client page.
    pub client_pick: usize,
    pub group: u8,
    pub connect: Option<Connect>,
    pub tab: Tab,
    /// The KEYBOARD cursor over the active tab's rows — `None` until ↑/↓.
    pub sel: Option<usize>,
    /// The Torrents tab's name-or-hash filter.
    pub filter: Input,
    pub filter_focus: bool,
    pub modal: Modal,
    /// One line of status above the tips: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    tscroll: usize,
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    last_list: Option<Instant>,
    ui: Surface<Act>,
}

impl Room {
    pub(super) fn new(client: Client, same_machine: bool) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            same_machine,
            params: None,
            users: BTreeMap::new(),
            status: None,
            status_at: None,
            torrents: Vec::new(),
            list_error: None,
            list_loaded: false,
            access: BTreeMap::new(),
            templates: PathTemplates::default(),
            libraries: Vec::new(),
            seeds: Vec::new(),
            seed_ticks: Vec::new(),
            seed_queue: VecDeque::new(),
            choosing: false,
            client_pick: 0,
            group: 0,
            connect: None,
            tab: Tab::Torrents,
            sel: None,
            filter: Input::default(),
            filter_focus: false,
            modal: Modal::None,
            note: Some((t!("tor.beta_note").to_string(), false)),
            busy: None,
            queued: None,
            in_flight: false,
            tscroll: 0,
            sel_anchor: None,
            last_load: None,
            last_list: None,
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
            self.queue(Op::Load, t!("tor.busy_loading"));
        } else {
            self.queued = Some(Op::Load);
        }
    }

    // ── Facts ───────────────────────────────────────────────────────────────

    pub(crate) fn kind(&self) -> Kind {
        self.params.as_ref().map(|p| Kind::parse(&p.client)).unwrap_or(Kind::Disabled)
    }

    fn config(&self) -> Option<&TorrentClientConfig> {
        self.params.as_ref().map(|p| config_of(p, self.kind()))
    }

    pub(crate) fn phase(&self) -> Phase {
        let Some(p) = &self.params else { return Phase::Loading };
        let kind = Kind::parse(&p.client);
        if self.choosing || kind == Kind::Disabled {
            Phase::Choose
        } else if !config_of(p, kind).configured {
            Phase::Connect
        } else {
            Phase::Tabs
        }
    }

    /// The policy and the whitelist exist only with someone to whitelist.
    pub(crate) fn many_users(&self) -> bool {
        self.users.len() > 1
    }

    pub(crate) fn tabs(&self) -> Vec<Tab> {
        Tab::ALL.iter().copied().filter(|t| *t != Tab::Access || self.many_users()).collect()
    }

    fn next_tab(&self, forward: bool) -> Tab {
        let tabs = self.tabs();
        let i = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let n = tabs.len();
        tabs[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }

    fn policy_whitelist(&self) -> bool {
        self.params.as_ref().is_some_and(|p| p.enabled_for == "whitelist")
    }

    /// The torrents the filter lets through, as indices into the list.
    pub(crate) fn filtered(&self) -> Vec<usize> {
        let q = self.filter.value().trim().to_lowercase();
        self.torrents
            .iter()
            .enumerate()
            .filter(|(_, t)| q.is_empty() || t.name.to_lowercase().contains(&q) || t.info_hash.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect()
    }

    /// How many rows the active tab has.
    pub(crate) fn rows(&self) -> usize {
        match self.tab {
            Tab::Torrents => self.filtered().len(),
            Tab::Libraries => self.libraries.len(),
            Tab::Seeding => self.seeds.len(),
            Tab::Access => self.users.len(),
            Tab::Client => 0,
        }
    }

    fn selected_torrent(&self) -> Option<&Torrent> {
        if self.tab != Tab::Torrents {
            return None;
        }
        self.sel.and_then(|s| self.filtered().get(s).copied()).and_then(|i| self.torrents.get(i))
    }

    fn selected_library(&self) -> Option<&str> {
        (self.tab == Tab::Libraries).then(|| self.sel.and_then(|s| self.libraries.get(s).map(String::as_str))).flatten()
    }

    fn selected_user(&self) -> Option<&str> {
        (self.tab == Tab::Access).then(|| self.sel.and_then(|s| self.users.keys().nth(s).map(String::as_str))).flatten()
    }

    fn torrent(&self, hash: &str) -> Option<&Torrent> {
        self.torrents.iter().find(|t| t.info_hash == hash)
    }

    /// Libraries the daemon can reach, for the state line.
    fn reachable(&self) -> usize {
        self.libraries.iter().filter(|l| self.access.get(*l).is_some_and(|a| matches!(a.confidence.as_str(), "verified" | "inferred"))).count()
    }

    fn ticked_vpaths(&self) -> Vec<String> {
        self.seed_ticks.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect()
    }

    // ── Actions ─────────────────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Tab(tab) => {
                if self.tabs().contains(&tab) && tab != self.tab {
                    self.tab = tab;
                    self.sel = None;
                    self.sel_anchor = None;
                    self.tscroll = 0;
                    self.filter_focus = false;
                    self.note = None;
                    if tab == Tab::Torrents && self.last_list.is_none_or(|t| t.elapsed() >= POLL_LIST) && self.queued.is_none() {
                        self.queued = Some(Op::List);
                    }
                }
            }
            Act::Select(i) => {
                if i < self.rows() {
                    self.sel = Some(i);
                    self.note = None;
                }
            }
            Act::TableScroll(d) => {
                self.tscroll = if d < 0 { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            Act::TableScrollTo(i) => self.tscroll = i,
            Act::PickClient(i) => {
                if i < Kind::ALL.len() {
                    self.client_pick = i;
                    self.group = 0;
                }
            }
            Act::PickPolicy(i) => {
                let policy = if i == 0 { "all" } else { "whitelist" };
                if self.params.as_ref().is_some_and(|p| p.enabled_for != policy) {
                    self.queue(Op::SetPolicy(policy.to_string()), t!("tor.busy_saving"));
                }
            }
            Act::ApplyClient => self.apply_client(),
            Act::ConnectFocus(i) => {
                if let Some(c) = &mut self.connect {
                    c.focus = i.min(c.fields().len() - 1);
                }
            }
            Act::ConnectHttps => {
                if let Some(c) = &mut self.connect {
                    c.https = !c.https;
                    c.probe = None;
                }
            }
            Act::ConnectTest => self.probe(false),
            Act::ConnectSubmit => self.probe(true),
            Act::ConnectBack => {
                self.choosing = true;
                self.client_pick = self.kind().index();
                self.group = 0;
            }
            Act::FilterFocus => {
                if self.tab == Tab::Torrents {
                    self.filter_focus = true;
                    self.note = None;
                }
            }
            Act::Remove(hash) => {
                if self.torrent(&hash).is_some_and(|t| t.managed_by_mstream) {
                    self.modal = Modal::Remove(hash);
                }
            }
            Act::RemoveConfirm => {
                if let Modal::Remove(hash) = std::mem::replace(&mut self.modal, Modal::None) {
                    self.queue(Op::Remove(hash), t!("tor.busy_removing"));
                }
            }
            Act::RemoveCancel | Act::DisconnectCancel | Act::ModalCancel => self.modal = Modal::None,
            Act::AutoDetect(vpath) => {
                let busy = match &vpath {
                    Some(name) => t!("tor.busy_detect", name = printable(name, NAME_MAX)).to_string(),
                    None => t!("tor.busy_detect_all").to_string(),
                };
                self.queue(Op::Detect(vpath), busy);
            }
            Act::Mapping(vpath) => {
                let current = self.access.get(&vpath).and_then(|a| a.daemon_path.clone()).unwrap_or_default();
                self.modal = Modal::Mapping { vpath, path: Input::new(current), error: None };
            }
            Act::Template(vpath) => {
                let current = self.templates.vpaths.get(&vpath).and_then(|t| t.template.clone()).unwrap_or_default();
                self.modal = Modal::Template { vpath, template: Input::new(current), error: None };
            }
            Act::TemplateSuggest => {
                let suggested = self.templates.suggested_template.clone();
                if let Modal::Template { template, error, .. } = &mut self.modal {
                    *template = Input::new(suggested);
                    *error = None;
                }
            }
            Act::TemplateClear => {
                if let Modal::Template { template, error, .. } = &mut self.modal {
                    *template = Input::default();
                    *error = None;
                }
            }
            Act::ModalSubmit => self.submit_modal(),
            Act::SeedAdd => {
                if self.same_machine {
                    self.queue(Op::PickFile, t!("tor.busy_loading"));
                } else {
                    self.open_seed_path();
                }
            }
            Act::SeedTick(i) => {
                if let Some((_, on)) = self.seed_ticks.get_mut(i) {
                    *on = !*on;
                }
            }
            Act::SeedClear => {
                self.seeds.retain(|s| s.outcome.is_none() && s.error.is_none());
                self.sel = None;
                self.note = Some((t!("tor.done_cleared").to_string(), false));
            }
            Act::UserToggle(user) => {
                if let Some(u) = self.users.get(&user) {
                    let allow = !u.allow_torrent;
                    self.queue(Op::UserAccess { user, allow }, t!("tor.busy_saving"));
                }
            }
            Act::ClientTest => self.queue(Op::Status, t!("tor.busy_test")),
            Act::ClientSwitch => {
                self.choosing = true;
                self.client_pick = self.kind().index();
                self.group = 0;
                self.note = None;
            }
            Act::Disconnect => {
                if self.phase() == Phase::Tabs {
                    self.modal = Modal::Disconnect;
                }
            }
            Act::DisconnectConfirm => {
                self.modal = Modal::None;
                let kind = self.kind();
                self.queue(Op::Disconnect(kind), t!("tor.busy_saving"));
            }
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    /// Enter on the client page: the chosen client is posted; choosing the
    /// one already chosen just moves on to its credentials (or its tabs).
    fn apply_client(&mut self) {
        let kind = Kind::ALL[self.client_pick.min(Kind::ALL.len() - 1)];
        let current = self.kind();
        if kind == current {
            if kind != Kind::Disabled {
                self.choosing = false;
                self.ensure_connect();
            }
            return;
        }
        self.queue(Op::SetClient(kind), t!("tor.busy_client", client = kind.label()));
    }

    /// The connect form exists whenever the Connect page shows.
    fn ensure_connect(&mut self) {
        if self.phase() == Phase::Connect {
            let kind = self.kind();
            if self.connect.as_ref().is_none_or(|c| c.kind != kind) {
                let saved = self.config().cloned().unwrap_or_default();
                self.connect = Some(Connect::new(kind, &saved));
            }
        }
    }

    fn probe(&mut self, connect: bool) {
        let Some(c) = &mut self.connect else { return };
        match c.creds() {
            Err(e) => c.error = Some(e),
            Ok(creds) => {
                c.error = None;
                let kind = c.kind;
                self.queue(Op::Probe { kind, creds, connect }, if connect { t!("tor.busy_connecting") } else { t!("tor.busy_testing") });
            }
        }
    }

    fn open_seed_path(&mut self) {
        let start = self.seeds.last().and_then(|s| PathBuf::from(&s.file).parent().map(|p| p.to_string_lossy().to_string())).unwrap_or_default();
        let _ = start;
        self.modal = Modal::SeedPath { path: Input::default(), matches: Vec::new(), error: None };
    }

    /// Enter in a text modal.
    fn submit_modal(&mut self) {
        match &mut self.modal {
            Modal::Mapping { vpath, path, error } => {
                let p = path.value().trim().to_string();
                if p.is_empty() {
                    *error = Some(t!("tor.err_seed_path").to_string().replace(".torrent file", "path"));
                    return;
                }
                let vpath = vpath.clone();
                self.queue(Op::Map { vpath, path: p }, t!("tor.busy_mapping"));
            }
            Modal::Template { vpath, template, .. } => {
                let raw = template.value().trim().to_string();
                let vpath = vpath.clone();
                self.queue(Op::Template { vpath, template: (!raw.is_empty()).then_some(raw) }, t!("tor.busy_saving"));
            }
            Modal::SeedPath { path, error, .. } => {
                let raw = expand_home(path.value().trim());
                if raw.is_empty() {
                    *error = Some(t!("tor.err_seed_path").to_string());
                    return;
                }
                let file = PathBuf::from(&raw);
                if !file.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent")) {
                    *error = Some(t!("tor.err_seed_ext").to_string());
                    return;
                }
                self.modal = Modal::None;
                self.seed(file);
            }
            _ => {}
        }
    }

    /// One `.torrent` joins the queue; the worker takes them one at a time.
    fn seed(&mut self, path: PathBuf) {
        let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        self.seeds.push(SeedRow { file, outcome: None, error: None });
        self.seed_queue.push_back((path, self.ticked_vpaths()));
        self.tab = Tab::Seeding;
        self.sel = Some(self.seeds.len() - 1);
        self.note = None;
        self.dispatch_seed();
    }

    fn dispatch_seed(&mut self) {
        if self.queued.is_none()
            && !self.in_flight
            && let Some((path, vpaths)) = self.seed_queue.pop_front()
        {
            let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
            self.queue(Op::Seed { path, vpaths }, t!("tor.busy_seed", file = printable(&file, NAME_MAX)));
        }
    }

    // ── Server calls ────────────────────────────────────────────────────────

    fn dispatch_queued(&mut self) {
        if self.in_flight {
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
        match op {
            Op::Load => self.last_load = Some(Instant::now()),
            Op::List => self.last_list = Some(Instant::now()),
            _ => {}
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
                self.params = Some(loaded.params);
                self.users = loaded.users;
                if let Some(status) = loaded.status {
                    self.status = Some(status);
                    self.status_at = Some(Instant::now());
                }
                if let Some(access) = loaded.access {
                    self.access = access.vpaths;
                }
                if let Some(templates) = loaded.templates {
                    self.templates = templates;
                }
                if let Some(list) = loaded.list {
                    self.take_list(list);
                    self.last_list = Some(Instant::now());
                }
                if self.phase() != Phase::Tabs {
                    self.status = None;
                    self.torrents.clear();
                    self.list_error = None;
                    self.list_loaded = false;
                    self.sel = None;
                    self.filter_focus = false;
                }
                self.refresh_libraries();
                self.ensure_connect();
                if let Some(w) = loaded.warning {
                    self.note = Some((w, true));
                }
                if !self.tabs().contains(&self.tab) {
                    self.tab = Tab::Torrents;
                }
                let n = self.rows();
                self.sel = self.sel.filter(|_| n > 0).map(|s| s.min(n - 1));
            }
            Done::Loaded(Err(e)) => {
                self.note = Some((gate_message(&e, &t!("tor.load_failed")), true));
            }
            Done::Listed(Ok(list)) => {
                if self.phase() == Phase::Tabs {
                    self.take_list(list);
                    let n = self.rows();
                    self.sel = self.sel.filter(|_| n > 0).map(|s| s.min(n - 1));
                }
            }
            Done::Listed(Err(e)) => self.list_error = Some(e.to_string()),
            Done::Status(Ok(status)) => {
                let label = self.kind().label();
                self.note = Some(if status.connected {
                    (t!("tor.done_test_ok", client = label, version = version_words(&status)).to_string(), false)
                } else {
                    (t!("tor.done_test_failed", reason = printable(status.reason.as_deref().unwrap_or(""), 200)).to_string(), true)
                });
                self.status = Some(status);
                self.status_at = Some(Instant::now());
            }
            Done::Status(Err(e)) => self.fail(&t!("tor.done_test_failed", reason = ""), e),
            Done::ClientSet { kind, result: Ok(()) } => {
                self.choosing = false;
                self.connect = None;
                self.note = Some((
                    if kind == Kind::Disabled { t!("tor.done_client_off").to_string() } else { t!("tor.done_client", client = kind.label()).to_string() },
                    false,
                ));
                self.reload(true);
            }
            Done::ClientSet { result: Err(e), .. } => self.fail(&t!("tor.fail_client"), e),
            Done::PolicySet { policy, result: Ok(()) } => {
                if let Some(p) = &mut self.params {
                    p.enabled_for = policy.clone();
                }
                self.note = Some((
                    if policy == "whitelist" { t!("tor.done_policy_whitelist") } else { t!("tor.done_policy_all") }.to_string(),
                    false,
                ));
            }
            Done::PolicySet { result: Err(e), .. } => self.fail(&t!("tor.fail_policy"), e),
            Done::UserAccess { user, allow, result: Ok(()) } => {
                if let Some(u) = self.users.get_mut(&user) {
                    u.allow_torrent = allow;
                }
                let name = printable(&user, NAME_MAX);
                self.note = Some((
                    if allow { t!("tor.done_granted", user = name) } else { t!("tor.done_revoked", user = name) }.to_string(),
                    false,
                ));
            }
            Done::UserAccess { result: Err(e), .. } => self.fail(&t!("tor.fail_access"), e),
            Done::Probed { kind, connect, result: Ok(answer) } => {
                let version = printable(answer.version.as_deref().unwrap_or(""), NAME_MAX);
                let rpc = answer.rpc_version.as_ref().map(value_words).unwrap_or_default();
                if let Some(c) = &mut self.connect {
                    c.probe = Some(if answer.ok {
                        Ok((version.clone(), rpc))
                    } else {
                        Err(printable(answer.message.as_deref().or(answer.error.as_deref()).unwrap_or(""), 200))
                    });
                    if answer.ok && connect {
                        c.password = Input::default();
                    }
                }
                if answer.ok && connect {
                    self.note = Some((t!("tor.done_connected", client = kind.label(), version = version).to_string(), false));
                    self.choosing = false;
                    self.reload(true);
                }
            }
            Done::Probed { connect, result: Err(e), .. } => {
                let what = if connect { t!("tor.fail_connect") } else { t!("tor.probe_failed", err = "") }.to_string();
                match &mut self.connect {
                    Some(c) => c.probe = Some(Err(format!("{}{e}", what.trim_end_matches(": ")).replace("  ", " "))),
                    None => self.fail(&what, e),
                }
            }
            Done::Disconnected(Ok(())) => {
                self.connect = None;
                self.status = None;
                self.note = Some((t!("tor.done_disconnected").to_string(), false));
                self.reload(true);
            }
            Done::Disconnected(Err(e)) => self.fail(&t!("tor.fail_disconnect"), e),
            Done::Removed { hash, result: Ok(answer) } => {
                let name = self.torrent(&hash).map(|t| printable(&t.name, NAME_MAX)).unwrap_or_else(|| short_id(&hash));
                self.note = Some(if answer.daemon_remove_ok {
                    (t!("tor.done_removed", name = name).to_string(), false)
                } else {
                    (t!("tor.done_removed_daemon_failed", name = name, err = printable(answer.daemon_remove_error.as_deref().unwrap_or(""), 200)).to_string(), true)
                });
                self.queued = Some(Op::List);
            }
            Done::Removed { result: Err(ApiError::NotFound(_)), .. } => {
                self.note = Some((t!("tor.not_managed").to_string(), true));
                self.queued = Some(Op::List);
            }
            Done::Removed { result: Err(e), .. } => self.fail(&t!("tor.fail_remove"), e),
            Done::Detected(Ok(access)) => {
                self.access = access.vpaths;
                self.refresh_libraries();
                self.note = Some((t!("tor.done_detect").to_string(), false));
            }
            Done::Detected(Err(e)) => self.fail(&t!("tor.fail_detect"), e),
            Done::Mapped { vpath, result: Ok(v) } => {
                let path = v.get("daemonPath").and_then(|p| p.as_str()).unwrap_or("").to_string();
                let confidence = v.get("confidence").and_then(|c| c.as_str()).map(confidence_words).unwrap_or_default();
                self.modal = Modal::None;
                self.note = Some((
                    t!("tor.done_mapped", name = printable(&vpath, NAME_MAX), path = printable(&path, 200), confidence = confidence).to_string(),
                    false,
                ));
                self.reload(false);
            }
            Done::Mapped { result: Err(e), .. } => {
                let what = t!("tor.fail_mapping").to_string();
                match &mut self.modal {
                    Modal::Mapping { error, .. } => *error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
                self.reload(false);
            }
            Done::TemplateSaved { vpath, result: Ok(saved) } => {
                let cleared = saved.template.is_none();
                self.templates.vpaths.entry(vpath.clone()).or_default().template = saved.template;
                self.modal = Modal::None;
                let name = printable(&vpath, NAME_MAX);
                self.note = Some((
                    if cleared { t!("tor.done_template_cleared", name = name) } else { t!("tor.done_template", name = name) }.to_string(),
                    false,
                ));
            }
            Done::TemplateSaved { result: Err(e), .. } => {
                let what = t!("tor.fail_template").to_string();
                match &mut self.modal {
                    Modal::Template { error, .. } => *error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Seeded { file, result } => {
                if let Some(row) = self.seeds.iter_mut().find(|s| s.file == file && s.outcome.is_none() && s.error.is_none()) {
                    match result {
                        Ok(outcome) => row.outcome = Some(outcome),
                        Err(e) => row.error = Some(e),
                    }
                }
                if self.seed_queue.is_empty() {
                    self.queued = Some(Op::List);
                }
            }
            Done::Picked(FilePick::File(path)) => {
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent")) {
                    self.seed(path);
                } else {
                    self.note = Some((t!("tor.err_seed_ext").to_string(), true));
                }
            }
            Done::Picked(FilePick::Cancelled) => {}
            Done::Picked(FilePick::Unavailable(e)) => {
                self.note = Some((t!("tor.picker_unavailable", err = printable(&e, 120)).to_string(), true));
                self.open_seed_path();
            }
        }
        self.dispatch_seed();
    }

    fn take_list(&mut self, list: TorrentList) {
        self.torrents = list.torrents;
        self.list_error = list.error.filter(|e| !e.trim().is_empty());
        self.list_loaded = true;
    }

    /// The library names: every template row and every access row.
    fn refresh_libraries(&mut self) {
        let mut names: Vec<String> = self.templates.vpaths.keys().cloned().collect();
        for name in self.access.keys() {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.sort();
        let old: BTreeMap<String, bool> = self.seed_ticks.drain(..).collect();
        self.seed_ticks = names.iter().map(|n| (n.clone(), old.get(n).copied().unwrap_or(false))).collect();
        self.libraries = names;
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

    /// The polls, quiet: the list every five seconds while the Torrents
    /// tab shows, everything else every thirty — only on the tabs page, and
    /// never on top of a call already queued or running.
    fn tick(&mut self) {
        if self.phase() != Phase::Tabs || self.in_flight || self.queued.is_some() {
            return;
        }
        if self.last_load.is_none_or(|t| t.elapsed() >= POLL_STATE) {
            self.reload(false);
        } else if self.tab == Tab::Torrents && self.last_list.is_none_or(|t| t.elapsed() >= POLL_LIST) {
            self.queued = Some(Op::List);
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

/// The room, loading: what `mstream-player admin torrents` opens.
pub(super) fn start(client: Client, same_machine: bool) -> Room {
    let mut room = Room::new(client, same_machine);
    room.reload(true);
    room
}

// ── Words ────────────────────────────────────────────────────────────────────

/// A JSON scalar as text (Transmission's RPC number is a number).
fn value_words(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn version_words(s: &TorrentStatus) -> String {
    printable(s.version.as_deref().unwrap_or(""), NAME_MAX)
}

/// The webapp's status chip words.
pub(crate) fn status_word(status: &str) -> String {
    match status {
        "downloading" => t!("tor.st_downloading"),
        "seeding" => t!("tor.st_seeding"),
        "paused" => t!("tor.st_paused"),
        "queued" => t!("tor.st_queued"),
        "verifying" => t!("tor.st_verifying"),
        "error" => t!("tor.st_error"),
        _ => t!("tor.st_unknown"),
    }
    .to_string()
}

/// The chip colours in the kit's: downloading accent, seeding green, the
/// idle states dim, the worrying ones gold.
fn status_style(status: &str) -> Style {
    match status {
        "downloading" => Style::default().fg(th().accent),
        "seeding" => Style::default().fg(th().ok),
        "verifying" | "error" => Style::default().fg(th().gold),
        _ => dim(),
    }
}

/// The confidence ladder in the webapp's words.
pub(crate) fn confidence_words(c: &str) -> String {
    match c {
        "verified" => t!("tor.acc_verified"),
        "inferred" => t!("tor.acc_inferred"),
        "pending" => t!("tor.acc_pending"),
        _ => t!("tor.acc_unconfirmed"),
    }
    .to_string()
}

fn confidence_glyph(c: &str) -> &'static str {
    match c {
        "verified" => g("✓", "+"),
        "inferred" => "~",
        "pending" => g("⟳", "~"),
        _ => g("✗", "x"),
    }
}

fn confidence_style(c: &str) -> Style {
    match c {
        "verified" => Style::default().fg(th().ok),
        "inferred" => Style::default().fg(th().gold),
        "pending" => Style::default().fg(th().accent),
        _ => Style::default().fg(th().gold),
    }
}

/// The seed-existing outcome as the webapp's chip: glyph, word, style.
pub(crate) fn outcome_words(o: &str) -> (String, Style) {
    let (glyph, word, style) = match o {
        "seeded" => (g("✓", "+"), t!("tor.out_seeded"), Style::default().fg(th().ok)),
        "match_unmapped" => ("!", t!("tor.out_match_unmapped"), Style::default().fg(th().gold)),
        "pad_files_missing" => ("!", t!("tor.out_pad_files_missing"), Style::default().fg(th().gold)),
        "partial_match" => ("~", t!("tor.out_partial_match"), Style::default().fg(th().gold)),
        "already_in_daemon" => (g("⊝", "="), t!("tor.out_already_in_daemon"), dim()),
        "no_match" => (g("✗", "x"), t!("tor.out_no_match"), dim()),
        "invalid_torrent" => (g("✗", "x"), t!("tor.out_invalid_torrent"), Style::default().fg(th().gold)),
        "daemon_error" => (g("✗", "x"), t!("tor.out_daemon_error"), Style::default().fg(th().gold)),
        other => ("", std::borrow::Cow::Owned(other.to_string()), dim()),
    };
    (format!("{glyph} {word}").trim().to_string(), style)
}

/// The webapp's sentence for one seed outcome.
pub(crate) fn outcome_details(row: &SeedRow, client: &str) -> String {
    if let Some(e) = &row.error {
        return printable(e, 300);
    }
    let Some(o) = &row.outcome else { return t!("tor.seed_checking").to_string() };
    let vpath = printable(o.vpath.as_deref().unwrap_or(""), NAME_MAX);
    match o.outcome.as_str() {
        "seeded" => t!("tor.det_seeded", vpath = vpath, path = printable(o.added_at.as_deref().unwrap_or(""), 300)).to_string(),
        "match_unmapped" => {
            let root = printable(o.matched_root.as_deref().unwrap_or(""), 200);
            if o.mapping_confidence.is_some() {
                t!("tor.det_unmapped", vpath = vpath, root = root).to_string()
            } else {
                t!("tor.det_unmapped_unprobed", vpath = vpath, root = root).to_string()
            }
        }
        "pad_files_missing" => t!(
            "tor.det_pad",
            vpath = vpath,
            present = o.pad_files_present.unwrap_or(0),
            total = o.pad_files_total.unwrap_or(0),
            client = client
        )
        .to_string(),
        "partial_match" => {
            let mut missing: Vec<String> = o.missing.iter().take(3).map(|m| printable(m, 80)).collect();
            if o.missing.len() > 3 {
                missing.push(t!("tor.det_more", n = o.missing.len() - 3).to_string());
            }
            t!("tor.det_partial", matched = o.matched.unwrap_or(0), total = o.total.unwrap_or(0), vpath = vpath, missing = missing.join(", ")).to_string()
        }
        "no_match" => t!("tor.det_no_match", vpaths = o.checked_vpaths.iter().map(|v| printable(v, NAME_MAX)).collect::<Vec<_>>().join(", ")).to_string(),
        "already_in_daemon" => t!("tor.det_already").to_string(),
        _ => printable(o.error.as_deref().or(o.message.as_deref()).unwrap_or(""), 300),
    }
}

/// `~` and `~/…` become the home directory.
fn expand_home(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
    {
        return format!("{}{}", home.to_string_lossy(), &path[1..]);
    }
    path.to_string()
}

/// Tab in the seed-path modal: the longest common completion of what is
/// typed — folders and `.torrent` files — and the candidates to show.
fn complete_local(input: &mut Input) -> Vec<String> {
    let raw = expand_home(input.value());
    let (dir, prefix) = match raw.rfind('/') {
        Some(i) => (raw[..=i].to_string(), raw[i + 1..].to_string()),
        None => (String::new(), raw.clone()),
    };
    let list_dir = if dir.is_empty() { ".".to_string() } else { dir.clone() };
    let Ok(entries) = std::fs::read_dir(&list_dir) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(&prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                Some(format!("{name}/"))
            } else if name.to_lowercase().ends_with(".torrent") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    if names.is_empty() {
        return names;
    }
    let lcp = common_prefix(&names);
    if lcp.chars().count() > prefix.chars().count() {
        let value = format!("{dir}{lcp}");
        let cursor = value.chars().count();
        *input = Input::new(value).with_cursor(cursor);
    }
    names.truncate(SUGGEST_MAX);
    names
}

fn common_prefix(names: &[String]) -> String {
    let mut prefix: Vec<char> = names[0].chars().collect();
    for name in &names[1..] {
        let chars: Vec<char> = name.chars().collect();
        let common = prefix.iter().zip(chars.iter()).take_while(|(a, b)| a == b).count();
        prefix.truncate(common);
    }
    prefix.into_iter().collect()
}

/// The webapp's rate words: KB/s until a megabyte, then MB/s.
fn rate_words(bytes_per_sec: f64) -> String {
    if bytes_per_sec <= 0.0 {
        return "—".to_string();
    }
    if bytes_per_sec >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes_per_sec / 1024.0 / 1024.0)
    } else {
        format!("{:.0} KB/s", bytes_per_sec / 1024.0)
    }
}

/// The template resolved against the server's sample metadata, the way the
/// webapp previews it: variables case-insensitive, unknown ones empty,
/// separators inside a value become `-`, empty segments drop.
pub(crate) fn preview_template(template: &str, sample: &BTreeMap<String, String>) -> Option<String> {
    let raw = template.trim();
    if raw.is_empty() {
        return None;
    }
    let lookup = |name: &str| -> String {
        let key = match name.to_ascii_uppercase().as_str() {
            "ARTIST" => "artist",
            "ALBUM" => "album",
            "YEAR" => "year",
            "GENRE" => "genre",
            "ALBUMARTIST" => "albumartist",
            _ => return String::new(),
        };
        let value = sample.get(key).cloned().or_else(|| (key == "albumartist").then(|| sample.get("artist").cloned()).flatten()).unwrap_or_default();
        sanitize_segment(&value)
    };
    let mut out = String::new();
    let mut rest = raw;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push_str(&lookup(name));
                } else {
                    out.push_str(&rest[start..start + 2 + end + 2]);
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let path: Vec<String> = out.split(['/', '\\']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    Some(path.join("/"))
}

fn sanitize_segment(raw: &str) -> String {
    let mut v: String = raw
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '<' | '>' | '|' | '"') || c.is_control() { '-' } else { c })
        .collect();
    while v.contains("--") {
        v = v.replace("--", "-");
    }
    let v = v.split_whitespace().collect::<Vec<_>>().join(" ");
    let v = v.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    v.chars().take(200).collect()
}

/// "<verb> %{age} ago", except under a minute, which is "<verb> just now".
fn ago_words(key: &str, secs: i64) -> String {
    let age = age_text(secs);
    match (key, secs < 60) {
        ("reachable", true) => t!("tor.reachable_now"),
        ("reachable", false) => t!("tor.note_reachable", age = age),
        ("probed", true) => t!("tor.probed_now"),
        ("probed", false) => t!("tor.note_probed", age = age),
        (_, true) => t!("tor.added_now"),
        (_, false) => t!("tor.note_added_ago", age = age),
    }
    .to_string()
}

/// `lastProbedAt` as an age in seconds: an ISO or SQLite string, or a unix
/// number in seconds or milliseconds.
fn probed_secs(v: &serde_json::Value, now: i64) -> Option<i64> {
    let t = match v {
        serde_json::Value::String(s) => iso_unix(s)?,
        serde_json::Value::Number(n) => {
            let n = n.as_f64()?;
            if n > 1.0e12 { (n / 1000.0) as i64 } else { n as i64 }
        }
        _ => return None,
    };
    Some(now - t)
}

// ── Keys ─────────────────────────────────────────────────────────────────────

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match &mut room.modal {
        Modal::Mapping { path, error, .. } => {
            return match code {
                KeyCode::Esc => room.act(Act::ModalCancel),
                KeyCode::Enter => room.act(Act::ModalSubmit),
                KeyCode::Char(c) if c.is_control() => None,
                KeyCode::Char(_) if path.value().chars().count() >= PATH_MAX => None,
                _ => {
                    path.handle_event(&TermEvent::Key(key));
                    *error = None;
                    None
                }
            };
        }
        Modal::Template { template, error, .. } => {
            return match code {
                KeyCode::Esc => room.act(Act::ModalCancel),
                KeyCode::Enter => room.act(Act::ModalSubmit),
                KeyCode::Char('s') if ctrl => room.act(Act::TemplateSuggest),
                KeyCode::Char('x') if ctrl => room.act(Act::TemplateClear),
                KeyCode::Char(c) if c.is_control() => None,
                KeyCode::Char(_) if template.value().chars().count() >= TEMPLATE_MAX => None,
                _ => {
                    template.handle_event(&TermEvent::Key(key));
                    *error = None;
                    None
                }
            };
        }
        Modal::SeedPath { path, matches, error } => {
            return match code {
                KeyCode::Esc => room.act(Act::ModalCancel),
                KeyCode::Enter => room.act(Act::ModalSubmit),
                KeyCode::Tab => {
                    *matches = complete_local(path);
                    *error = None;
                    None
                }
                KeyCode::Char(c) if c.is_control() => None,
                KeyCode::Char(_) if path.value().chars().count() >= PATH_MAX => None,
                _ => {
                    path.handle_event(&TermEvent::Key(key));
                    matches.clear();
                    *error = None;
                    None
                }
            };
        }
        Modal::Remove(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::RemoveConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::RemoveCancel),
                _ => None,
            };
        }
        Modal::Disconnect => {
            return match code {
                KeyCode::Char('y') => room.act(Act::DisconnectConfirm),
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::DisconnectCancel),
                _ => None,
            };
        }
        Modal::None => {}
    }
    match room.phase() {
        Phase::Loading => match code {
            KeyCode::Esc | KeyCode::Char('q') => room.act(Act::Quit),
            _ => None,
        },
        Phase::Choose => choose_key(room, code),
        Phase::Connect => connect_key(room, key),
        Phase::Tabs => tabs_key(room, key),
    }
}

/// The client page: two radio groups, Enter on the card.
fn choose_key(room: &mut Room, code: KeyCode) -> Option<Outcome> {
    let groups = if room.many_users() { 2 } else { 1 };
    match code {
        KeyCode::Esc => {
            // Back to the tabs when the page was opened on purpose.
            if room.choosing && room.kind() != Kind::Disabled && room.config().is_some_and(|c| c.configured) {
                room.choosing = false;
                room.note = None;
                None
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Char('q') => room.act(Act::Quit),
        KeyCode::Tab | KeyCode::BackTab => {
            room.group = (room.group + 1) % groups;
            None
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Char(' ') => {
            let back = matches!(code, KeyCode::Left | KeyCode::Up);
            if room.group == 0 {
                let n = Kind::ALL.len();
                let i = if back { (room.client_pick + n - 1) % n } else { (room.client_pick + 1) % n };
                room.act(Act::PickClient(i))
            } else {
                let current = if room.policy_whitelist() { 1 } else { 0 };
                room.act(Act::PickPolicy(1 - current))
            }
        }
        KeyCode::Enter => room.act(Act::ApplyClient),
        _ => None,
    }
}

/// The connect form: Tab walks the fields, Space flips HTTPS, ^T probes.
fn connect_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let (n, focused) = match &room.connect {
        Some(c) => (c.fields().len(), c.focused()),
        None => {
            return match code {
                KeyCode::Esc => room.act(Act::ConnectBack),
                _ => None,
            };
        }
    };
    match code {
        KeyCode::Esc => room.act(Act::ConnectBack),
        KeyCode::Enter => room.act(Act::ConnectSubmit),
        KeyCode::Char('t') if ctrl => room.act(Act::ConnectTest),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(c) = &mut room.connect {
                c.focus = (c.focus + 1) % n;
            }
            None
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(c) = &mut room.connect {
                c.focus = (c.focus + n - 1) % n;
            }
            None
        }
        KeyCode::Char(' ') if focused == CField::Https => room.act(Act::ConnectHttps),
        _ => {
            if let KeyCode::Char(ch) = code {
                let allowed = match focused {
                    CField::Port => ch.is_ascii_digit(),
                    CField::Https => false,
                    _ => !ch.is_control(),
                };
                let max = if focused == CField::Port { 5 } else { 200 };
                let full = room
                    .connect
                    .as_mut()
                    .and_then(|c| c.input_mut(focused).map(|i| i.value().chars().count() >= max))
                    .unwrap_or(true);
                if !allowed || full {
                    return None;
                }
            }
            if let Some(c) = &mut room.connect {
                if let Some(input) = c.input_mut(focused) {
                    input.handle_event(&TermEvent::Key(key));
                }
                c.probe = None;
                c.error = None;
            }
            None
        }
    }
}

/// The tabs page: the cursor, the tabs, the filter, one letter per action.
fn tabs_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    if room.filter_focus {
        return match code {
            KeyCode::Esc | KeyCode::Enter => {
                room.filter_focus = false;
                None
            }
            KeyCode::Char(c) if c.is_control() => None,
            KeyCode::Char(_) if room.filter.value().chars().count() >= 80 => None,
            _ => {
                room.filter.handle_event(&TermEvent::Key(key));
                room.sel = None;
                room.tscroll = 0;
                None
            }
        };
    }
    let n = room.rows();
    match code {
        KeyCode::Esc => {
            if room.sel.is_some() {
                room.sel = None;
                room.note = None;
                None
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Char('q') => room.act(Act::Quit),
        KeyCode::Left => {
            let tab = room.next_tab(false);
            room.act(Act::Tab(tab))
        }
        KeyCode::Right => {
            let tab = room.next_tab(true);
            room.act(Act::Tab(tab))
        }
        KeyCode::Down if n > 0 => {
            let i = room.sel.map(|s| (s + 1).min(n - 1)).unwrap_or(0);
            room.act(Act::Select(i))
        }
        KeyCode::Up if n > 0 => {
            let i = room.sel.map(|s| s.saturating_sub(1)).unwrap_or(0);
            room.act(Act::Select(i))
        }
        KeyCode::Char('/') if room.tab == Tab::Torrents => room.act(Act::FilterFocus),
        KeyCode::Char(ch) => tab_letter(room, ch),
        _ => None,
    }
}

fn tab_letter(room: &mut Room, ch: char) -> Option<Outcome> {
    match room.tab {
        Tab::Torrents => match ch {
            'r' => {
                let hash = room.selected_torrent().map(|t| t.info_hash.clone());
                hash.and_then(|h| room.act(Act::Remove(h)))
            }
            _ => None,
        },
        Tab::Libraries => {
            let lib = room.selected_library().map(str::to_string);
            match ch {
                'd' => lib.and_then(|l| room.act(Act::AutoDetect(Some(l)))),
                'D' => room.act(Act::AutoDetect(None)),
                'm' => lib.and_then(|l| room.act(Act::Mapping(l))),
                't' => lib.and_then(|l| room.act(Act::Template(l))),
                _ => None,
            }
        }
        Tab::Seeding => match ch {
            'a' => room.act(Act::SeedAdd),
            'c' => room.act(Act::SeedClear),
            '1'..='9' => room.act(Act::SeedTick(ch as usize - '1' as usize)),
            _ => None,
        },
        Tab::Access => match ch {
            'p' => {
                let current = if room.policy_whitelist() { 1 } else { 0 };
                room.act(Act::PickPolicy(1 - current))
            }
            's' | ' ' => {
                let user = room.selected_user().map(str::to_string);
                user.and_then(|u| room.act(Act::UserToggle(u)))
            }
            _ => None,
        },
        Tab::Client => match ch {
            't' => room.act(Act::ClientTest),
            'c' => room.act(Act::ClientSwitch),
            'x' => room.act(Act::Disconnect),
            _ => None,
        },
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

    let title = t!("tor.title").to_string();
    draw_header(frame, area, &title, &host_of(&room.client));
    let chip_x = area.x + 2 + title.chars().count() as u16 + 1;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("tor.beta").to_string(), Style::default().fg(th().gold))),
        Rect { x: chip_x, y: area.y, width: area.width.saturating_sub(chip_x + 30), height: 1 },
    );
    let column = Rect {
        x: 2,
        y: 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(5),
    };
    match room.phase() {
        Phase::Loading => {}
        Phase::Choose => draw_choose(frame, room, column),
        Phase::Connect => {
            if let Some(c) = room.connect.clone() {
                draw_connect(frame, room, column, &c);
            }
        }
        Phase::Tabs => draw_tabs(frame, room, column),
    }
    draw_bottom(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Mapping { vpath, path, error } => {
            let client = room.kind().label();
            let body = vec![Line::from(Span::styled(t!("tor.mapping_hint").to_string(), dim()))];
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.mapping_title", name = printable(&vpath, NAME_MAX), client = client.clone()).to_string(),
                t!("tor.mapping_field", client = client.to_uppercase()).to_string(),
                &path,
                None,
                body,
                error.as_deref(),
                vec![(t!("tor.mapping_save").to_string(), true, Act::ModalSubmit)],
            );
        }
        Modal::Template { vpath, template, error } => {
            let vars = room.templates.supported_vars.iter().map(|v| format!("{{{{{v}}}}}")).collect::<Vec<_>>().join(" ");
            let preview = match preview_template(template.value(), &room.templates.sample_metadata) {
                None => (t!("tor.template_preview_none").to_string(), dim()),
                Some(p) if p.is_empty() => (t!("tor.template_preview_empty").to_string(), dim()),
                Some(p) => (p, bold()),
            };
            let body = vec![
                Line::from(vec![Span::styled(format!("{:<11}", t!("tor.template_vars")), dim()), Span::raw(vars)]),
                Line::from(Span::styled(t!("tor.template_vars_hint").to_string(), dim())),
                Line::from(""),
                Line::from(vec![Span::styled(format!("{:<11}", t!("tor.template_preview")), dim()), Span::styled(preview.0, preview.1)]),
                Line::from(Span::styled(t!("tor.template_preview_hint").to_string(), dim())),
            ];
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.template_title", name = printable(&vpath, NAME_MAX)).to_string(),
                t!("tor.template_field").to_string(),
                &template,
                None,
                body,
                error.as_deref(),
                vec![
                    (t!("tor.template_suggested").to_string(), false, Act::TemplateSuggest),
                    (t!("tor.template_clear").to_string(), false, Act::TemplateClear),
                    (t!("tor.template_save").to_string(), true, Act::ModalSubmit),
                ],
            );
        }
        Modal::SeedPath { path, matches, error } => {
            let mut body = vec![Line::from(Span::styled(t!("tor.seed_path_hint").to_string(), dim()))];
            for m in &matches {
                body.push(Line::from(Span::styled(format!("  {} {}", g("▸", "►"), clip(m, 64)), dim())));
            }
            draw_entry_modal(
                frame,
                room,
                area,
                t!("tor.seed_path_title").to_string(),
                t!("tor.seed_path_field").to_string(),
                &path,
                Some("~/Downloads/album.torrent"),
                body,
                error.as_deref(),
                vec![(t!("tor.seed_path_button").to_string(), true, Act::ModalSubmit)],
            );
        }
        Modal::Remove(hash) => {
            let name = room.torrent(&hash).map(|t| printable(&t.name, NAME_MAX)).unwrap_or_else(|| short_id(&hash));
            draw_gate(
                frame,
                room,
                area,
                t!("tor.remove_title", name = name, client = room.kind().label()).to_string(),
                vec![t!("tor.remove_1").to_string(), t!("tor.remove_2").to_string()],
                (t!("tor.remove_keep").to_string(), Act::RemoveCancel),
                (t!("tor.remove_confirm").to_string(), Act::RemoveConfirm),
            );
        }
        Modal::Disconnect => draw_gate(
            frame,
            room,
            area,
            t!("tor.disconnect_title", client = room.kind().label()).to_string(),
            vec![t!("tor.disconnect_1").to_string(), t!("tor.disconnect_2").to_string()],
            (t!("tor.disconnect_keep").to_string(), Act::DisconnectCancel),
            (t!("tor.disconnect_confirm").to_string(), Act::DisconnectConfirm),
        ),
    }
    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

/// The tips line names only what works right now.
fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Mapping { .. } => t!("tor.hint_mapping"),
        Modal::Template { .. } => t!("tor.hint_template"),
        Modal::SeedPath { .. } => t!("tor.hint_seed_path"),
        Modal::Remove(_) => t!("tor.hint_remove"),
        Modal::Disconnect => t!("tor.hint_disconnect"),
        Modal::None => match room.phase() {
            Phase::Loading => t!("tor.hint_loading"),
            Phase::Choose => {
                let pick = Kind::ALL[room.client_pick.min(Kind::ALL.len() - 1)];
                if pick == Kind::Disabled && room.kind() == Kind::Disabled {
                    t!("tor.hint_off_nothing")
                } else if pick == Kind::Disabled {
                    t!("tor.hint_off_disable")
                } else if room.many_users() {
                    t!("tor.hint_off_groups")
                } else {
                    t!("tor.hint_off")
                }
            }
            Phase::Connect => t!("tor.hint_connect"),
            Phase::Tabs if room.filter_focus => t!("tor.hint_filter"),
            Phase::Tabs => match room.tab {
                Tab::Torrents => match room.selected_torrent() {
                    None => t!("tor.hint_torrents"),
                    Some(t) if t.managed_by_mstream => t!("tor.hint_torrents_row"),
                    Some(_) => t!("tor.hint_torrents_row_external"),
                },
                Tab::Libraries => {
                    if room.selected_library().is_some() { t!("tor.hint_libraries_row") } else { t!("tor.hint_libraries") }
                }
                Tab::Seeding => t!("tor.hint_seeding"),
                Tab::Access => if room.selected_user().is_some() { t!("tor.hint_access_row") } else { t!("tor.hint_access") },
                Tab::Client => t!("tor.hint_client"),
            },
        },
    }
    .to_string()
}

/// The state line for every page: the webapp's status card in one row.
fn state_spans(room: &Room) -> (Vec<Span<'static>>, Option<String>) {
    let kind = room.kind();
    if kind == Kind::Disabled {
        return (vec![Span::styled(t!("tor.state_off").to_string(), dim()), Span::raw(t!("tor.state_off_detail").to_string())], None);
    }
    let label = kind.label();
    if !room.config().is_some_and(|c| c.configured) {
        return (
            vec![
                Span::styled(t!("tor.state_picked", client = kind.as_str()).to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_picked_detail").to_string()),
            ],
            None,
        );
    }
    let polls = Some(if room.tab == Tab::Torrents { t!("tor.polls") } else { t!("tor.polls_slow") }.to_string());
    match &room.status {
        None => (
            vec![
                Span::styled(t!("tor.state_asking", client = kind.as_str()).to_string(), dim().add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_asking_detail").to_string()),
            ],
            polls,
        ),
        Some(s) if s.connected => {
            let host = room.config().map(|c| format!("{}:{}", c.host, c.port)).unwrap_or_default();
            (
                vec![
                    Span::styled(t!("tor.state_connected").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                    Span::raw(
                        t!(
                            "tor.state_connected_detail",
                            client = label,
                            version = version_words(s),
                            host = printable(&host, 80),
                            reachable = room.reachable(),
                            total = room.libraries.len()
                        )
                        .to_string(),
                    ),
                ],
                polls,
            )
        }
        Some(s) => (
            vec![
                Span::styled(t!("tor.state_down").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::raw(t!("tor.state_down_detail", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string()),
            ],
            polls,
        ),
    }
}

fn draw_state(frame: &mut Frame, room: &Room, column: Rect) {
    let (spans, polls) = state_spans(room);
    let state_w: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let line = Rect { x: column.x, y: column.y, width: column.width, height: 1 };
    frame.render_widget(Paragraph::new(Line::from(spans)), line);
    if let Some(polls) = polls
        && state_w + 2 + polls.chars().count() <= column.width as usize
    {
        frame.render_widget(Paragraph::new(Span::styled(polls, dim())).alignment(Alignment::Right), line);
    }
}

/// Radio rows: `(•) name — description`, one per row; returns the rows used.
fn radio_rows(frame: &mut Frame, room: &mut Room, at: Rect, options: &[(String, String)], chosen: usize, focused: bool, act: impl Fn(usize) -> Act) -> u16 {
    for (i, (name, desc)) in options.iter().enumerate() {
        if i as u16 >= at.height {
            break;
        }
        let rect = Rect { x: at.x, y: at.y + i as u16, width: at.width, height: 1 };
        let on = i == chosen;
        let hover = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let glyph_style = if on && focused {
            Style::default().fg(th().accent)
        } else if on {
            Style::default()
        } else {
            dim()
        };
        let name_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else if on { bold() } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if on { g("(•)", "(*)") } else { "( )" }, glyph_style),
                Span::raw(" "),
                Span::styled(name.clone(), name_style),
                Span::styled(format!(" — {desc}"), dim()),
            ])),
            rect,
        );
        room.ui.click(rect, act(i));
    }
    options.len() as u16
}

/// A three-row affirmative card with a centred label.
fn card(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, color: ratatui::style::Color, act: Act) {
    let hover = room.ui.pointer.is_some_and(|p| at.contains(p));
    let color = if hover { th().bright } else { color };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(color));
    let inner = block.inner(at);
    frame.render_widget(block, at);
    frame.render_widget(
        Paragraph::new(Span::styled(label.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        inner,
    );
    room.ui.click(at, act);
}

/// The client page: the pitch, the client radio group, the card, the policy.
fn draw_choose(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    draw_state(frame, room, column);
    frame.render_widget(Paragraph::new(t!("tor.pitch_1").to_string()), line(column.y + 2));
    frame.render_widget(Paragraph::new(t!("tor.pitch_2").to_string()), line(column.y + 3));
    frame.render_widget(
        Paragraph::new(Span::styled(t!("tor.mobile_note", host = host_of(&room.client)).to_string(), dim())),
        line(column.y + 4),
    );
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.client_label").to_string(), dim())), line(column.y + 6));
    let options: Vec<(String, String)> = Kind::ALL.iter().map(|k| (k.label(), k.description())).collect();
    let rows = radio_rows(
        frame,
        room,
        Rect { x: column.x + 2, y: column.y + 7, width: column.width.saturating_sub(2), height: 4 },
        &options,
        room.client_pick,
        room.group == 0,
        Act::PickClient,
    );
    let mut y = column.y + 7 + rows + 1;
    let pick = Kind::ALL[room.client_pick.min(Kind::ALL.len() - 1)];
    if pick != Kind::Disabled {
        card(frame, room, Rect { x: column.x, y, width: column.width, height: 3 }, &t!("tor.connect_card", client = pick.label()), th().ok, Act::ApplyClient);
        y += 4;
    } else if room.kind() != Kind::Disabled {
        card(frame, room, Rect { x: column.x, y, width: column.width, height: 3 }, &t!("tor.off_card"), th().gold, Act::ApplyClient);
        y += 4;
    }
    if room.many_users() && y + 3 <= column.bottom() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.policy_label").to_string(), dim())), line(y));
        let policy = vec![
            (t!("tor.policy_all").to_string(), t!("tor.policy_all_desc").to_string()),
            (t!("tor.policy_whitelist").to_string(), t!("tor.policy_whitelist_desc").to_string()),
        ];
        let chosen = if room.policy_whitelist() { 1 } else { 0 };
        radio_rows(
            frame,
            room,
            Rect { x: column.x + 2, y: y + 1, width: column.width.saturating_sub(2), height: 2 },
            &policy,
            chosen,
            room.group == 1,
            Act::PickPolicy,
        );
    }
}

/// A labelled field: the label row, then a three-row box; the value may be
/// masked (a password) and may show a placeholder while empty.
#[allow(clippy::too_many_arguments)]
fn field_box(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, masked: bool, placeholder: Option<&str>, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let w = inner.width.saturating_sub(2);
    let value = if masked { "•".repeat(input.value().chars().count()) } else { input.value().to_string() };
    let (shown, style) = if value.is_empty() && !focused {
        (placeholder.map(|p| clip(p, w)).unwrap_or_default(), dim())
    } else if focused {
        (kit::input_display(&value, input.cursor(), w), Style::default())
    } else {
        (clip(&value, w), Style::default())
    };
    frame.render_widget(Paragraph::new(Span::styled(shown, style)), Rect { x: inner.x + 1, y: inner.y, width: w, height: 1 });
    room.ui.click(field, act);
}

/// The connect page: the form, the probe's answer, the two buttons.
fn draw_connect(frame: &mut Frame, room: &mut Room, column: Rect, c: &Connect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    draw_state(frame, room, column);
    let fields = c.fields();
    let index_of = |f: CField| fields.iter().position(|x| *x == f).unwrap_or(0);
    let focused = c.focused();
    let x = column.x;
    let wide = column.width.saturating_sub(20).min(56);
    let half = column.width.saturating_sub(4).min(44).min(column.width.saturating_sub(4) / 2);
    let mut y = column.y + 2;
    field_box(frame, room, Rect { x, y, width: wide, height: 4 }, &t!("tor.field_host"), &c.host, focused == CField::Host, false, Some("127.0.0.1"), Act::ConnectFocus(index_of(CField::Host)));
    field_box(frame, room, Rect { x: x + wide + 4, y, width: 16.min(column.width.saturating_sub(wide + 4)), height: 4 }, &t!("tor.field_port"), &c.port, focused == CField::Port, false, None, Act::ConnectFocus(index_of(CField::Port)));
    y += 5;
    if c.kind.has_username() {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("tor.field_username"), &c.username, focused == CField::Username, false, None, Act::ConnectFocus(index_of(CField::Username)));
        field_box(frame, room, Rect { x: x + half + 4, y, width: half, height: 4 }, &t!("tor.field_password"), &c.password, focused == CField::Password, true, None, Act::ConnectFocus(index_of(CField::Password)));
    } else {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("tor.field_password"), &c.password, focused == CField::Password, true, None, Act::ConnectFocus(index_of(CField::Password)));
    }
    y += 5;
    let https_at;
    if c.kind.has_rpc_path() {
        field_box(frame, room, Rect { x, y, width: wide, height: 4 }, &t!("tor.field_rpc"), &c.rpc_path, focused == CField::RpcPath, false, None, Act::ConnectFocus(index_of(CField::RpcPath)));
        https_at = Rect { x: x + wide + 4, y: y + 2, width: column.width.saturating_sub(wide + 4), height: 1 };
        y += 5;
    } else {
        https_at = Rect { x, y, width: column.width, height: 1 };
        y += 2;
    }
    let glyph_style = if focused == CField::Https {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if c.https {
        Style::default().fg(th().ok)
    } else {
        dim()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if c.https { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
            Span::raw(format!(" {}", t!("tor.https"))),
        ])),
        https_at,
    );
    room.ui.click(https_at, Act::ConnectHttps);

    // The probe's answer, the form's own complaint, or qBittorrent's caveat.
    let verdict: Option<(String, Style)> = if let Some(e) = &c.error {
        Some((format!("{} {e}", g("−", "-")), Style::default().fg(th().gold)))
    } else {
        match &c.probe {
            Some(Ok((version, rpc))) => Some((
                format!(
                    "{} {}",
                    g("✓", "+"),
                    t!(
                        "tor.probe_ok",
                        client = c.kind.label(),
                        version = version,
                        rpc = if rpc.is_empty() { String::new() } else { t!("tor.probe_rpc", n = rpc).to_string() }
                    )
                ),
                Style::default().fg(th().ok),
            )),
            Some(Err(e)) => Some((format!("{} {}", g("−", "-"), t!("tor.probe_failed", err = e)), Style::default().fg(th().gold))),
            None if c.kind == Kind::Qbittorrent => Some((t!("tor.csrf_note").to_string(), Style::default().fg(th().gold))),
            None => None,
        }
    };
    if let Some((text, style)) = verdict
        && y < column.bottom()
    {
        frame.render_widget(Paragraph::new(Span::styled(text, style)).wrap(Wrap { trim: true }), Rect { x, y, width: column.width, height: 2.min(column.bottom() - y) });
    }
    let by = column.bottom().saturating_sub(1).max(y + 2);
    if by < column.bottom() + 1 {
        let connect = t!("tor.connect_button").to_string();
        let connect_w = connect.chars().count() as u16 + 4;
        let connect_rect = kit::button(frame, &mut room.ui, Rect { x: column.right().saturating_sub(connect_w), y: by, width: connect_w, height: 1 }, &connect, true, Act::ConnectSubmit);
        let test = t!("tor.test_button").to_string();
        let test_w = test.chars().count() as u16 + 4;
        kit::button(frame, &mut room.ui, Rect { x: connect_rect.x.saturating_sub(test_w + 2), y: by, width: test_w, height: 1 }, &test, false, Act::ConnectTest);
    }
    let _ = line;
}

/// Connected: the state line, the tabs, the active tab's body.
fn draw_tabs(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    draw_state(frame, room, column);
    let tabs_y = column.y + 2;
    let mut x = column.x;
    for tab in room.tabs() {
        let name = match tab {
            Tab::Torrents if !room.torrents.is_empty() => t!("tor.tab_torrents_n", n = room.torrents.len()).to_string(),
            Tab::Torrents => t!("tor.tab_torrents").to_string(),
            Tab::Libraries => t!("tor.tab_libraries").to_string(),
            Tab::Seeding => t!("tor.tab_seeding").to_string(),
            Tab::Access => t!("tor.tab_access").to_string(),
            Tab::Client => t!("tor.tab_client").to_string(),
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
        Tab::Torrents => t!("tor.note_torrents"),
        Tab::Libraries => t!("tor.note_libraries"),
        Tab::Seeding => t!("tor.note_seeding"),
        Tab::Access => t!("tor.note_access"),
        Tab::Client => t!("tor.note_client"),
    }
    .to_string();
    if x as usize + 2 + note.chars().count() <= column.right() as usize {
        frame.render_widget(Paragraph::new(Span::styled(note, dim())).alignment(Alignment::Right), line(tabs_y));
    }
    let body = Rect { x: column.x, y: tabs_y + 2, width: column.width, height: column.bottom().saturating_sub(tabs_y + 2) };
    match room.tab {
        Tab::Torrents => draw_torrents(frame, room, body),
        Tab::Libraries => draw_libraries(frame, room, body),
        Tab::Seeding => draw_seeding(frame, room, body),
        Tab::Access => draw_access(frame, room, body),
        Tab::Client => draw_client(frame, room, body),
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

/// Torrents: the filter line, then the daemon's list.
fn draw_torrents(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    // The filter: an inline input, dim words until it has something.
    let filter_rect = Rect { x: body.x, y: body.y, width: body.width.saturating_sub(20), height: 1 };
    let hover = room.ui.pointer.is_some_and(|p| filter_rect.contains(p));
    let slash_style = if room.filter_focus { Style::default().fg(th().accent).add_modifier(Modifier::BOLD) } else if hover { Style::default().fg(th().bright) } else { dim() };
    let text = if room.filter_focus {
        Span::raw(kit::input_display(room.filter.value(), room.filter.cursor(), filter_rect.width.saturating_sub(2)))
    } else if room.filter.value().is_empty() {
        Span::styled(t!("tor.filter_hint").to_string(), dim())
    } else {
        Span::raw(clip(room.filter.value(), filter_rect.width.saturating_sub(2)))
    };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled("/ ", slash_style), text])), filter_rect);
    room.ui.click(filter_rect, Act::FilterFocus);
    let filtered = room.filtered();
    let count = if !room.filter.value().trim().is_empty() {
        t!("tor.count_match", n = filtered.len(), total = room.torrents.len()).to_string()
    } else if room.torrents.len() == 1 {
        t!("tor.count_one").to_string()
    } else {
        t!("tor.count", n = room.torrents.len()).to_string()
    };
    if room.list_loaded && room.list_error.is_none() {
        frame.render_widget(Paragraph::new(Span::styled(count, dim())).alignment(Alignment::Right), line(body.y));
    }

    let table = Rect { x: body.x, y: body.y + 2, width: body.width, height: body.height.saturating_sub(2) };
    if table.height < 3 {
        return;
    }
    let (status_w, prog_w, down_w, size_w, by_w) = (11u16, 15u16, 10u16, 8u16, 13u16);
    let fixed = status_w + 2 + prog_w + 2 + down_w + 2 + size_w + 2 + by_w + 2;
    let name_w = table.width.saturating_sub(fixed).max(12);
    let status_x = table.x + name_w + 2;
    let prog_x = status_x + status_w + 2;
    let down_x = prog_x + prog_w + 2;
    let size_x = down_x + down_w + 2;
    let by_x = size_x + size_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, name_w, &t!("tor.col_name"), false),
            (status_x, status_w, &t!("tor.col_status"), false),
            (prog_x, prog_w, &t!("tor.col_progress"), false),
            (down_x, down_w, &t!("tor.col_down"), true),
            (size_x, size_w, &t!("tor.col_size"), true),
            (by_x, by_w, &t!("tor.col_by"), false),
        ],
    );
    if let Some(err) = &room.list_error {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("tor.list_error", err = printable(err, 200)).to_string(), Style::default().fg(th().gold))).wrap(Wrap { trim: true }),
            Rect { x: table.x, y: rows_y, width: table.width, height: 2.min(table.bottom().saturating_sub(rows_y)) },
        );
        return;
    }
    if !room.list_loaded {
        return;
    }
    if room.torrents.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_list").to_string(), dim())), line(rows_y));
        return;
    }
    if filtered.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_filter").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let torrents = room.torrents.clone();
    table_rows(frame, room, rows_rect, filtered.len(), |frame, room, i, rect, selected, hovered| {
        let t = &torrents[filtered[i]];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        let name = printable(&t.name, 300);
        frame.render_widget(Paragraph::new(Span::styled(clip(&name, name_w), base)), cell(rect.x, name_w));
        if name.chars().count() > name_w as usize {
            room.ui.tip(cell(rect.x, name_w), format!("{name} · {}", t.info_hash));
        }
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&status_word(&t.status), status_w), cell_style(selected, hovered, status_style(&t.status)))),
            cell(status_x, status_w),
        );
        let pct = (t.percent * 100.0).round().clamp(0.0, 100.0) as u16;
        let filled = (pct / 10) as usize;
        let bar_style = if t.status == "seeding" { Style::default().fg(th().ok) } else { Style::default().fg(th().accent) };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("▰".repeat(filled), cell_style(selected, hovered, bar_style)),
                Span::styled("▱".repeat(10 - filled), cell_style(selected, hovered, dim())),
                Span::styled(format!(" {pct:>3}%"), cell_style(selected, hovered, dim())),
            ])),
            cell(prog_x, prog_w),
        );
        let down = rate_words(t.rate_download);
        frame.render_widget(
            Paragraph::new(Span::styled(down.clone(), cell_style(selected, hovered, if down == "—" { dim() } else { Style::default() }))).alignment(Alignment::Right),
            cell(down_x, down_w),
        );
        frame.render_widget(Paragraph::new(Span::styled(fmt_bytes(t.size_bytes), base)).alignment(Alignment::Right), cell(size_x, size_w));
        let (by, own) = match (t.managed_by_mstream, &t.managed_by) {
            (true, Some(u)) => (printable(u, NAME_MAX), Style::default()),
            (true, None) => ("mStream".to_string(), Style::default()),
            (false, _) => (t!("tor.by_external").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&by, by_w), cell_style(selected, hovered, own))), cell(by_x, by_w));
    });
    // The cursor row's hash, error and origin ride the note line.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(t) = room.selected_torrent()
    {
        let mut spans = vec![Span::styled(short_id(&t.info_hash), dim())];
        if !t.error_message.trim().is_empty() {
            spans.push(Span::styled(" · ", dim()));
            spans.push(Span::styled(printable(&t.error_message, 200), Style::default().fg(th().gold)));
        }
        if let Some(u) = t.managed_by.as_deref().filter(|_| t.managed_by_mstream) {
            spans.push(Span::styled(format!(" · {}", t!("tor.note_added_by", user = printable(u, NAME_MAX))), dim()));
        }
        if t.added_at > 0.0 {
            spans.push(Span::styled(format!(" · {}", ago_words("added", unix_now() - t.added_at as i64)), dim()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(body.bottom() + 1));
    }
}

/// Libraries: the daemon's view of each library and its template.
fn draw_libraries(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let client = room.kind().label();
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.intro_1", client = client.clone()).to_string(), dim())), line(body.y));
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.intro_2").to_string(), dim())), line(body.y + 1));
    let table = Rect { x: body.x, y: body.y + 3, width: body.width, height: body.height.saturating_sub(3) };
    if table.height < 3 {
        return;
    }
    let (lib_w, access_w, tmpl_w) = (12u16, 14u16, 34u16);
    let fixed = lib_w + 2 + access_w + 2 + tmpl_w + 2;
    let path_w = table.width.saturating_sub(fixed).max(12);
    let access_x = table.x + lib_w + 2;
    let path_x = access_x + access_w + 2;
    let tmpl_x = path_x + path_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, lib_w, &t!("tor.col_library"), false),
            (access_x, access_w, &t!("tor.col_access"), false),
            (path_x, path_w, &t!("tor.col_path", client = client.to_uppercase()), false),
            (tmpl_x, tmpl_w, &t!("tor.col_template"), false),
        ],
    );
    if room.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_libraries").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let libraries = room.libraries.clone();
    let access = room.access.clone();
    let templates = room.templates.vpaths.clone();
    table_rows(frame, room, rows_rect, libraries.len(), |frame, room, i, rect, selected, hovered| {
        let name = &libraries[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), lib_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(rect.x, lib_w),
        );
        let row = access.get(name);
        let confidence = row.map(|a| a.confidence.as_str()).unwrap_or("unconfirmed");
        frame.render_widget(
            Paragraph::new(Span::styled(
                clip(&format!("{} {}", confidence_glyph(confidence), confidence_words(confidence)), access_w),
                cell_style(selected, hovered, confidence_style(confidence)),
            )),
            cell(access_x, access_w),
        );
        let (path, own) = match row.and_then(|a| a.daemon_path.clone()).filter(|p| !p.is_empty()) {
            Some(p) if confidence != "pending" => (p, Style::default()),
            _ if confidence == "pending" => (t!("tor.path_probing").to_string(), dim()),
            _ => (t!("tor.path_none").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&path, 300), path_w), cell_style(selected, hovered, own))), cell(path_x, path_w));
        if path.chars().count() > path_w as usize {
            room.ui.tip(cell(path_x, path_w), path.clone());
        }
        let (tmpl, own) = match templates.get(name).and_then(|t| t.template.clone()).filter(|t| !t.trim().is_empty()) {
            Some(t) => (t, Style::default()),
            None => (t!("tor.tmpl_none").to_string(), dim()),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&tmpl, 300), tmpl_w), cell_style(selected, hovered, own))), cell(tmpl_x, tmpl_w));
        if tmpl.chars().count() > tmpl_w as usize {
            room.ui.tip(cell(tmpl_x, tmpl_w), tmpl.clone());
        }
    });
    // The cursor row's probe story rides the note line.
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(name) = room.selected_library()
    {
        let now = unix_now();
        let mut spans = vec![Span::styled(printable(name, NAME_MAX), bold())];
        match room.access.get(name) {
            Some(a) => {
                let mut parts: Vec<(String, Style)> = Vec::new();
                if let Some(e) = a.last_error.as_deref().filter(|e| !e.trim().is_empty()) {
                    parts.push((printable(e, 200), Style::default().fg(th().gold)));
                } else if let Some(m) = a.method.as_deref().filter(|_| a.confidence == "verified" || a.confidence == "inferred") {
                    parts.push((t!("tor.note_via", method = printable(m, 60)).to_string(), dim()));
                }
                if a.source.as_deref() == Some("manual") {
                    parts.push((t!("tor.note_manual").to_string(), dim()));
                }
                if let Some(secs) = a.last_probed_at.as_ref().and_then(|v| probed_secs(v, now)) {
                    parts.push((ago_words("probed", secs), dim()));
                }
                for (text, style) in parts {
                    spans.push(Span::styled(" — ", dim()));
                    spans.push(Span::styled(text, style));
                    if let Some(last) = spans.last_mut()
                        && last.content.is_empty()
                    {
                        spans.pop();
                    }
                }
            }
            None => {
                spans.push(Span::styled(" — ", dim()));
                spans.push(Span::styled(t!("tor.path_none").to_string(), dim()));
            }
        }
        // Later separators read as " · ".
        let mut seen = 0;
        for s in spans.iter_mut() {
            if s.content == " — " {
                seen += 1;
                if seen > 1 {
                    s.content = " · ".into();
                }
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), line(body.bottom() + 1));
    }
}

/// Seeding: the pitch, the add card, the SEARCH IN ticks, the results.
fn draw_seeding(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_intro_1").to_string(), dim())), line(body.y));
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_intro_2").to_string(), dim())), line(body.y + 1));
    card(frame, room, Rect { x: body.x, y: body.y + 3, width: body.width, height: 3 }, &t!("tor.seed_card"), th().ok, Act::SeedAdd);
    let ticks_y = body.y + 7;
    let mut x = body.x;
    let label = format!("{}  ", t!("tor.seed_search"));
    frame.render_widget(Paragraph::new(Span::styled(label.clone(), dim())), Rect { x, y: ticks_y, width: label.chars().count() as u16, height: 1 });
    x += label.chars().count() as u16;
    let ticks = room.seed_ticks.clone();
    for (i, (name, on)) in ticks.iter().enumerate() {
        let text = format!("{} {}", if *on { g("[✓]", "[x]") } else { "[ ]" }, printable(name, NAME_MAX));
        let w = text.chars().count() as u16;
        if x + w > body.right() {
            break;
        }
        let rect = Rect { x, y: ticks_y, width: w, height: 1 };
        let hover = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if hover { Style::default().fg(th().bright) } else if *on { Style::default().fg(th().ok) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
        room.ui.click(rect, Act::SeedTick(i));
        x += w + 3;
    }
    if ticks.iter().all(|(_, on)| !*on) {
        let hint = t!("tor.seed_all").to_string();
        if x + 2 + hint.chars().count() as u16 <= body.right() {
            frame.render_widget(Paragraph::new(Span::styled(hint, dim())).alignment(Alignment::Right), line(ticks_y));
        }
    }
    let table = Rect { x: body.x, y: body.y + 9, width: body.width, height: body.height.saturating_sub(9) };
    if table.height < 3 {
        return;
    }
    let (file_w, outcome_w) = (26u16, 16u16);
    let outcome_x = table.x + file_w + 2;
    let details_x = outcome_x + outcome_w + 2;
    let details_w = table.right().saturating_sub(details_x);
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, file_w, &t!("tor.col_file"), false),
            (outcome_x, outcome_w, &t!("tor.col_outcome"), false),
            (details_x, details_w, &t!("tor.col_details"), false),
        ],
    );
    if room.seeds.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.seed_empty").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let seeds = room.seeds.clone();
    let client = room.kind().label();
    table_rows(frame, room, rows_rect, seeds.len(), |frame, room, i, rect, selected, hovered| {
        let s = &seeds[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(&s.file, 200), file_w), base)), cell(rect.x, file_w));
        let (word, style) = match (&s.outcome, &s.error) {
            (Some(o), _) => outcome_words(&o.outcome),
            (None, Some(_)) => outcome_words("daemon_error"),
            (None, None) => (format!("{} {}", g("⟳", "~"), t!("tor.seed_checking")), Style::default().fg(th().accent)),
        };
        frame.render_widget(Paragraph::new(Span::styled(clip(&word, outcome_w), cell_style(selected, hovered, style))), cell(outcome_x, outcome_w));
        let details = outcome_details(s, &client);
        let own = if s.outcome.as_ref().is_some_and(|o| matches!(o.outcome.as_str(), "no_match" | "already_in_daemon")) { dim() } else { Style::default() };
        frame.render_widget(Paragraph::new(Span::styled(clip(&details, details_w), cell_style(selected, hovered, own))), cell(details_x, details_w));
        if details.chars().count() > details_w as usize {
            room.ui.tip(cell(details_x, details_w), details.clone());
        }
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(s) = room.sel.and_then(|i| room.seeds.get(i))
    {
        let details = outcome_details(s, &client);
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(printable(&s.file, NAME_MAX), bold()), Span::raw(format!(" — {}", clip(&details, body.width.saturating_sub(s.file.chars().count() as u16 + 3))))])),
            line(body.bottom() + 1),
        );
    }
}

/// Access: the policy, then the users.
fn draw_access(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(t!("tor.policy_label").to_string(), dim())), line(body.y));
    let policy = vec![
        (t!("tor.policy_all").to_string(), t!("tor.policy_all_desc").to_string()),
        (t!("tor.policy_whitelist").to_string(), t!("tor.policy_whitelist_desc_here").to_string()),
    ];
    let whitelist = room.policy_whitelist();
    radio_rows(frame, room, Rect { x: body.x + 2, y: body.y + 1, width: body.width.saturating_sub(2), height: 2 }, &policy, if whitelist { 1 } else { 0 }, true, Act::PickPolicy);
    let table = Rect { x: body.x, y: body.y + 4, width: body.width, height: body.height.saturating_sub(4) };
    if table.height < 3 {
        return;
    }
    let (user_w, admin_w, tor_w) = (22u16, 8u16, 10u16);
    let admin_x = table.x + user_w + 2;
    let tor_x = admin_x + admin_w + 2;
    let rows_y = table_head(
        frame,
        table,
        &[
            (table.x, user_w, &t!("tor.col_user"), false),
            (admin_x, admin_w, &t!("tor.col_admin"), false),
            (tor_x, tor_w, &t!("tor.col_torrents"), false),
        ],
    );
    if room.users.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.empty_users").to_string(), dim())), line(rows_y));
        return;
    }
    let rows_rect = Rect { x: table.x, y: rows_y, width: table.width, height: table.bottom().saturating_sub(rows_y) };
    let users: Vec<(String, AdminUser)> = room.users.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    table_rows(frame, room, rows_rect, users.len(), |frame, room, i, rect, selected, hovered| {
        let (name, u) = &users[i];
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        // With every user allowed, the ticks are information, not a lever: dim.
        let own = if whitelist { Style::default() } else { dim() };
        let base = cell_style(selected, hovered, own);
        frame.render_widget(Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), user_w), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })), cell(rect.x, user_w));
        let admin = if u.admin { t!("tor.admin_yes").to_string() } else { "—".to_string() };
        frame.render_widget(Paragraph::new(Span::styled(admin, cell_style(selected, hovered, dim()))), cell(admin_x, admin_w));
        let glyph = if u.allow_torrent { g("[✓]", "[x]") } else { "[ ]" };
        let glyph_style = if u.allow_torrent && whitelist { Style::default().fg(th().ok) } else { dim() };
        let glyph_rect = cell(tor_x, 3);
        frame.render_widget(Paragraph::new(Span::styled(glyph, cell_style(selected, hovered, glyph_style))), glyph_rect);
        room.ui.click(glyph_rect, Act::UserToggle(name.clone()));
    });
    if room.note.is_none()
        && room.busy.is_none()
        && let Some(name) = room.selected_user()
    {
        let allowed = room.users.get(name).is_some_and(|u| u.allow_torrent);
        let user = printable(name, NAME_MAX);
        let text = if allowed { t!("tor.note_user_may", user = user) } else { t!("tor.note_user_may_not", user = user) }.to_string();
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::raw(text), Span::styled(t!("tor.note_user_tail").to_string(), dim())])),
            line(body.bottom() + 1),
        );
    }
}

/// Client: the connection card — status, the saved fields, the buttons.
fn draw_client(frame: &mut Frame, room: &mut Room, body: Rect) {
    let line = |y: u16| Rect { x: body.x, y, width: body.width, height: 1 };
    let kind = room.kind();
    let label = kind.label();
    let status = match &room.status {
        Some(s) if s.connected => Span::styled(t!("tor.status_connected", client = label.clone(), version = version_words(s)).to_string(), Style::default().fg(th().ok)),
        Some(s) => Span::styled(t!("tor.status_down", reason = printable(s.reason.as_deref().unwrap_or(""), 120)).to_string(), Style::default().fg(th().gold)),
        None => Span::styled(t!("tor.status_unknown").to_string(), dim()),
    };
    frame.render_widget(Paragraph::new(Line::from(vec![Span::styled(label.to_uppercase(), bold()), Span::raw("   "), status])), line(body.y));
    let cfg = room.config().cloned().unwrap_or_default();
    let mut rows: Vec<(String, String, bool)> = vec![
        (t!("tor.field_host").to_string(), cfg.host.clone(), true),
        (t!("tor.field_port").to_string(), cfg.port.to_string(), true),
    ];
    if kind.has_username() {
        let u = cfg.username.trim().to_string();
        rows.push((t!("tor.field_username").to_string(), if u.is_empty() { "(none)".to_string() } else { u.clone() }, !u.is_empty()));
    }
    if kind.has_rpc_path() {
        rows.push((t!("tor.field_rpc").to_string(), cfg.rpc_path.clone().unwrap_or_default(), true));
    }
    rows.push(("HTTPS".to_string(), if cfg.use_https { "yes".to_string() } else { "no".to_string() }, true));
    let mut y = body.y + 2;
    for (k, v, plain) in rows {
        frame.render_widget(Paragraph::new(Span::styled(k, dim())), Rect { x: body.x, y, width: 11, height: 1 });
        frame.render_widget(Paragraph::new(Span::styled(printable(&v, 200), if plain { Style::default() } else { dim() })), Rect { x: body.x + 12, y, width: body.width.saturating_sub(12), height: 1 });
        y += 1;
    }
    y += 1;
    let others: Vec<String> = room
        .params
        .as_ref()
        .map(|p| {
            Kind::ALL
                .iter()
                .filter(|k| **k != Kind::Disabled && **k != kind && config_of(p, **k).configured)
                .map(|k| format!("{} {}:{}", k.label(), config_of(p, *k).host, config_of(p, *k).port))
                .collect()
        })
        .unwrap_or_default();
    if !others.is_empty() && y < body.bottom() {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}  ", t!("tor.also_saved")), dim()),
                Span::raw(t!("tor.also_saved_line", clients = others.join(", ")).to_string()),
            ]))
            .wrap(Wrap { trim: true }),
            Rect { x: body.x, y, width: body.width, height: 2.min(body.bottom() - y) },
        );
        y += 2;
    }
    if y + 1 < body.bottom() {
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.mobile_note", host = host_of(&room.client)).to_string(), dim())), line(y));
        frame.render_widget(Paragraph::new(Span::styled(t!("tor.disconnect_note").to_string(), dim())), line(y + 1));
    }
    let by = body.bottom().saturating_sub(1);
    if by > y + 1 {
        let switch = t!("tor.btn_switch").to_string();
        let switch_w = switch.chars().count() as u16 + 4;
        let switch_rect = kit::button(frame, &mut room.ui, Rect { x: body.right().saturating_sub(switch_w), y: by, width: switch_w, height: 1 }, &switch, true, Act::ClientSwitch);
        let disc = t!("tor.btn_disconnect").to_string();
        let disc_w = disc.chars().count() as u16 + 4;
        let disc_rect = kit::button(frame, &mut room.ui, Rect { x: switch_rect.x.saturating_sub(disc_w + 2), y: by, width: disc_w, height: 1 }, &disc, false, Act::Disconnect);
        let test = t!("tor.btn_test").to_string();
        let test_w = test.chars().count() as u16 + 4;
        kit::button(frame, &mut room.ui, Rect { x: disc_rect.x.saturating_sub(test_w + 2), y: by, width: test_w, height: 1 }, &test, true, Act::ClientTest);
    }
    if room.note.is_none() && room.busy.is_none() {
        let text = match (&room.status, room.status_at) {
            (Some(s), Some(at)) if s.connected => Some((ago_words("reachable", at.elapsed().as_secs() as i64), dim())),
            (Some(s), _) => Some((t!("tor.note_unreachable", reason = printable(s.reason.as_deref().unwrap_or(""), 200)).to_string(), Style::default().fg(th().gold))),
            _ => None,
        };
        if let Some((text, style)) = text {
            frame.render_widget(Paragraph::new(Span::styled(text, style)), line(body.bottom() + 1));
        }
    }
}

/// One text field in a modal, helper lines beneath, a row of buttons.
#[allow(clippy::too_many_arguments)]
fn draw_entry_modal(
    frame: &mut Frame,
    room: &mut Room,
    area: Rect,
    title: String,
    label: String,
    input: &Input,
    placeholder: Option<&str>,
    body: Vec<Line<'static>>,
    error: Option<&str>,
    buttons: Vec<(String, bool, Act)>,
) {
    let height = 10 + body.len() as u16 + if error.is_some() { 1 } else { 0 };
    let inner = kit::modal_frame(frame, area, 74, height, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(clip(&title, w.saturating_sub(4)), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))), line(inner.y));
    kit::modal_close(frame, &mut room.ui, inner, Act::ModalCancel, t!("path_modal.tip_close"));
    field_box(frame, room, Rect { x, y: inner.y + 2, width: w, height: 4 }, &label, input, true, false, placeholder, Act::ModalSubmit);
    let mut y = inner.y + 7;
    for l in body {
        frame.render_widget(Paragraph::new(l), line(y));
        y += 1;
    }
    if let Some(e) = error {
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {}", g("−", "-"), clip(e, w.saturating_sub(2))), Style::default().fg(th().gold))), line(y));
    }
    let by = inner.bottom().saturating_sub(1);
    let mut right = inner.right().saturating_sub(1);
    for (label, primary, act) in buttons.into_iter().rev() {
        let bw = label.chars().count() as u16 + 4;
        let rect = kit::button(frame, &mut room.ui, Rect { x: right.saturating_sub(bw), y: by, width: bw, height: 1 }, &label, primary, act);
        right = rect.x.saturating_sub(2);
    }
}

/// The gold gate: a title, its lines, the safe choice first.
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
    use crate::api::types::TemplateRow;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn new_room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"), false)
    }

    fn params(client: &str, configured: bool) -> TorrentParams {
        TorrentParams {
            client: client.into(),
            enabled_for: "all".into(),
            transmission: TorrentClientConfig {
                host: if configured { "127.0.0.1".into() } else { String::new() },
                port: 9091,
                username: String::new(),
                rpc_path: Some("/transmission/rpc".into()),
                use_https: false,
                configured,
            },
            qbittorrent: TorrentClientConfig { port: 8080, ..Default::default() },
            deluge: TorrentClientConfig { port: 8112, ..Default::default() },
        }
    }

    fn users(n: usize) -> BTreeMap<String, AdminUser> {
        let all = [("iros", true, true), ("mira", false, true), ("guest", false, false), ("dj-tom", false, false)];
        all.iter().take(n).map(|(u, admin, allow)| (u.to_string(), AdminUser { admin: *admin, vpaths: Vec::new(), allow_torrent: *allow })).collect()
    }

    fn status_ok() -> TorrentStatus {
        TorrentStatus {
            connected: true,
            configured: true,
            client_type: Some("transmission".into()),
            version: Some("4.0.6".into()),
            rpc_version: Some(serde_json::json!(17)),
            reason: None,
        }
    }

    fn access() -> VpathAccess {
        let row = |path: Option<&str>, confidence: &str, source: &str, method: Option<&str>, error: Option<&str>| AccessRow {
            daemon_path: path.map(str::to_string),
            mstream_writable: Some(true),
            confidence: confidence.into(),
            source: Some(source.into()),
            method: method.map(str::to_string),
            last_probed_at: Some(serde_json::json!((unix_now() - 180) * 1000)),
            last_error: error.map(str::to_string),
        };
        let mut vpaths = BTreeMap::new();
        vpaths.insert("music".to_string(), row(Some("/downloads/music"), "verified", "auto", Some("transmission:free-space"), None));
        vpaths.insert("podcasts".to_string(), row(Some("/downloads/podcasts"), "verified", "manual", Some("transmission:free-space"), None));
        vpaths.insert("vinyl".to_string(), row(None, "unconfirmed", "auto", Some("transmission:free-space"), Some("daemon free-space returned -1 (path not visible to daemon)")));
        vpaths.insert("audiobooks".to_string(), row(None, "pending", "auto", None, None));
        VpathAccess { client_type: Some("transmission".into()), vpaths, error: None }
    }

    fn templates() -> PathTemplates {
        let mut vpaths = BTreeMap::new();
        vpaths.insert("music".to_string(), TemplateRow { template: Some("{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into()) });
        vpaths.insert("podcasts".to_string(), TemplateRow { template: None });
        vpaths.insert("vinyl".to_string(), TemplateRow { template: Some("{{ALBUMARTIST}}/{{ALBUM}}".into()) });
        vpaths.insert("audiobooks".to_string(), TemplateRow { template: None });
        let sample = [("artist", "Pink Floyd"), ("album", "The Dark Side of the Moon"), ("year", "1973"), ("genre", "Progressive Rock"), ("albumartist", "Pink Floyd")];
        PathTemplates {
            vpaths,
            supported_vars: ["ARTIST", "ALBUM", "YEAR", "GENRE", "ALBUMARTIST"].iter().map(|s| s.to_string()).collect(),
            suggested_template: "{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into(),
            sample_metadata: sample.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn torrent(seed: &str, name: &str, status: &str, pct: f64, rate: f64, size: u64, by: Option<&str>) -> Torrent {
        Torrent {
            info_hash: seed.repeat(10).chars().take(40).collect(),
            name: name.into(),
            status: status.into(),
            percent: pct,
            rate_download: rate,
            rate_upload: 0.0,
            eta: -1.0,
            size_bytes: size,
            error_message: if status == "error" { "Tracker gave HTTP response code 403 (Forbidden)".into() } else { String::new() },
            managed_by_mstream: by.is_some(),
            managed_by: by.map(str::to_string),
            added_at: (unix_now() - 2 * 86_400) as f64,
        }
    }

    fn list() -> TorrentList {
        TorrentList {
            torrents: vec![
                torrent("5f4a", "Boards of Canada - Music Has the Right to Children (1998) [FLAC]", "downloading", 0.63, 2.4 * 1024.0 * 1024.0, 641_728_512, Some("iros")),
                torrent("a1b2", "Aphex Twin - Selected Ambient Works 85-92", "seeding", 1.0, 0.0, 417_333_248, Some("mira")),
                torrent("c3d4", "linux-6.9.iso", "seeding", 1.0, 0.0, 2_791_728_742, None),
                torrent("e5f6", "Radiohead - In Rainbows [MP3 V0]", "error", 0.44, 0.0, 138_412_032, Some("iros")),
            ],
            error: None,
            client_type: Some("transmission".into()),
        }
    }

    fn loaded(p: TorrentParams, users: BTreeMap<String, AdminUser>, daemon: bool) -> Done {
        Done::Loaded(Ok(Box::new(Loaded {
            params: p,
            users,
            status: daemon.then(status_ok),
            access: daemon.then(access),
            templates: daemon.then(templates),
            list: daemon.then(list),
            warning: None,
        })))
    }

    /// A connected room on the Torrents tab, four users.
    fn connected() -> Room {
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", true), users(4), true));
        room.note = None;
        room
    }

    fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(room: &mut Room, c: char) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
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

    fn row(frame: &str, start: &str) -> String {
        frame.lines().find(|l| l.trim_start().starts_with(start)).map(str::to_string).unwrap_or_default()
    }

    #[test]
    fn the_client_page_offers_the_daemons_and_hides_the_policy_for_one_user() {
        let _en = english();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(1), false));
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("Torrents beta"), "{frame}");
        assert!(frame.contains("• off — no torrent client chosen"), "{frame}");
        assert!(frame.contains("Hand off magnet links and .torrent files to a torrent client on this host or on the LAN."), "{frame}");
        assert!(frame.contains("Users add torrents from a phone at home.mstream.example/torrent"), "{frame}");
        assert!(frame.contains("(•) Disabled — nobody can add torrents"), "{frame}");
        assert!(frame.contains("( ) Transmission — RPC on port 9091, usually at /transmission/rpc"), "{frame}");
        assert!(frame.contains("( ) qBittorrent — WebUI on port 8080 · its CSRF protection must be off"), "{frame}");
        assert!(!frame.contains("WHO CAN ADD TORRENTS"), "one user has nobody to whitelist\n{frame}");
        assert!(!frame.contains("Connect to"), "nothing to connect to yet\n{frame}");
        assert!(frame.contains("←→ or Space choose a client · Esc back"), "{frame}");
        assert!(frame.contains("beta — the torrent feature is new"), "{frame}");
        press(&mut room, KeyCode::Right);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Transmission") && frame.contains("Connect to Transmission ▸"), "{frame}");
        assert!(frame.contains("←→ or Space choose · Enter connect · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetClient(Kind::Transmission)));
        room.queued = None;
        room.apply(Done::ClientSet { kind: Kind::Transmission, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "torrent client: Transmission"));
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.phase(), Phase::Connect);
        let frame = draw(&mut room);
        assert!(frame.contains("• transmission — no credentials yet · Test saves nothing; Connect saves after a good probe"), "{frame}");
        for word in ["HOST", "PORT", "USERNAME", "PASSWORD", "RPC PATH", "[ ] use HTTPS", "9091", "/transmission/rpc", "^T test", "Connect ▸"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("Tab next field · Space HTTPS · ^T test · Enter connect · Esc back to the client choice"), "{frame}");
        // Esc returns to the choice; the picked client stays applied.
        press(&mut room, KeyCode::Esc);
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("• transmission — no credentials yet") && frame.contains("(•) Transmission"), "{frame}");
        // Picking Disabled again offers to turn the feature off.
        press(&mut room, KeyCode::Left);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Disabled") && frame.contains("Turn torrents off ▸"), "{frame}");
        assert!(frame.contains("←→ or Space choose · Enter turn off · Esc back"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::SetClient(Kind::Disabled)));
        room.queued = None;
        room.apply(Done::ClientSet { kind: Kind::Disabled, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "torrents are off"));

        // Four users: the policy group appears, Tab reaches it, a change posts at once.
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(4), false));
        let frame = draw(&mut room);
        assert!(frame.contains("WHO CAN ADD TORRENTS") && frame.contains("(•) all users — every signed-in user can add torrents"), "{frame}");
        assert!(frame.contains("( ) whitelist — only the users you tick on the Access tab"), "{frame}");
        assert!(frame.contains("←→ or Space choose a client · Esc back"), "{frame}");
        press(&mut room, KeyCode::Tab);
        press(&mut room, KeyCode::Right);
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));
        room.queued = None;
        room.apply(Done::PolicySet { policy: "whitelist".into(), result: Ok(()) });
        assert!(room.policy_whitelist());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "policy: only the whitelist may add torrents"));
        assert!(draw(&mut room).contains("(•) whitelist"));
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn the_connect_form_probes_without_saving_then_connects() {
        let _en = english();
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.connect.as_ref().map(|c| c.focused()), Some(CField::Host));
        press(&mut room, KeyCode::Enter);
        let frame = draw(&mut room);
        assert!(frame.contains("− a host is needed"), "{frame}");
        assert!(room.queued.is_none());
        type_text(&mut room, "127.0.0.1");
        press(&mut room, KeyCode::Tab); // port: digits only
        type_text(&mut room, "x");
        assert_eq!(room.connect.as_ref().unwrap().port.value(), "9091");
        press(&mut room, KeyCode::Tab);
        type_text(&mut room, "admin");
        press(&mut room, KeyCode::Tab);
        type_text(&mut room, "s3cret");
        let frame = draw(&mut room);
        assert!(frame.contains("••••••") && !frame.contains("s3cret"), "the password is masked\n{frame}");
        press(&mut room, KeyCode::Tab); // rpc path
        press(&mut room, KeyCode::Tab); // https
        assert_eq!(room.connect.as_ref().unwrap().focused(), CField::Https);
        press(&mut room, KeyCode::Char(' '));
        assert!(room.connect.as_ref().unwrap().https);
        ctrl(&mut room, 't');
        let creds = TorrentCreds {
            host: "127.0.0.1".into(),
            port: 9091,
            username: Some("admin".into()),
            password: "s3cret".into(),
            rpc_path: Some("/transmission/rpc".into()),
            use_https: true,
        };
        assert_eq!(room.queued, Some(Op::Probe { kind: Kind::Transmission, creds: creds.clone(), connect: false }));
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: false,
            result: Ok(ProbeAnswer { ok: true, version: Some("4.0.6".into()), rpc_version: Some(serde_json::json!(17)), ..Default::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("✓ reachable — Transmission 4.0.6 · RPC 17 · nothing saved yet"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().password.value(), "s3cret", "a test keeps the draft");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Probe { kind: Kind::Transmission, creds: creds.clone(), connect: true }));
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: true,
            result: Ok(ProbeAnswer { ok: false, error: Some("connection_failed".into()), message: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() }),
        });
        let frame = draw(&mut room);
        assert!(frame.contains("− connection failed: connect ECONNREFUSED 127.0.0.1:9091"), "{frame}");
        assert_eq!(room.phase(), Phase::Connect, "nothing was saved");
        press(&mut room, KeyCode::Enter);
        room.queued = None;
        room.apply(Done::Probed {
            kind: Kind::Transmission,
            connect: true,
            result: Ok(ProbeAnswer { ok: true, version: Some("4.0.6".into()), ..Default::default() }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "connected to Transmission 4.0.6"));
        assert_eq!(room.queued, Some(Op::Load));
        assert_eq!(room.connect.as_ref().unwrap().password.value(), "", "the password leaves the draft once accepted");

        // qBittorrent: no RPC path, the CSRF caveat; Deluge: no username.
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("qbittorrent", false), users(1), false));
        let frame = draw(&mut room);
        assert!(frame.contains("USERNAME") && !frame.contains("RPC PATH"), "{frame}");
        assert!(frame.contains("mStream sends no Referer — turn off qBittorrent's CSRF protection"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().port.value(), "8080");
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("deluge", false), users(1), false));
        let frame = draw(&mut room);
        assert!(!frame.contains("USERNAME") && !frame.contains("RPC PATH") && frame.contains("PASSWORD"), "{frame}");
        assert_eq!(room.connect.as_ref().unwrap().fields(), vec![CField::Host, CField::Port, CField::Password, CField::Https]);
    }

    #[test]
    fn connected_the_torrents_tab_lists_filters_and_gates_the_remove() {
        let _en = english();
        let mut room = connected();
        assert_eq!(room.phase(), Phase::Tabs);
        let frame = draw(&mut room);
        assert!(frame.contains("• connected — Transmission 4.0.6 · 127.0.0.1:9091 · 2 of 4 libraries reachable"), "{frame}");
        assert!(frame.contains("polls every 5 s"), "{frame}");
        for word in [" Torrents · 4 ", " Libraries ", " Seeding ", " Access ", " Client ", "the daemon's list", "/ filter by name or info-hash", "4 torrents"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        for word in ["NAME", "STATUS", "PROGRESS", "DOWN", "SIZE", "ADDED BY"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        let boc = row(&frame, "Boards of Canada");
        assert!(boc.contains("downloading") && boc.contains("▰▰▰▰▰▰▱▱▱▱  63%") && boc.contains("2.4 MB/s") && boc.contains("612.0 MB") && boc.contains("iros"), "{boc}");
        let linux = row(&frame, "linux-6.9.iso");
        assert!(linux.contains("seeding") && linux.contains("▰▰▰▰▰▰▰▰▰▰ 100%") && linux.contains("external"), "{linux}");
        assert!(row(&frame, "Radiohead").contains("error"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · / filter · Esc back"), "{frame}");
        // The cursor row's story on the note line.
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        let frame = draw(&mut room);
        assert!(frame.contains("e5f6e5f6e5f6… · Tracker gave HTTP response code 403 (Forbidden) · added by iros · 2d ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · / filter · r remove (files stay) · Esc deselect"), "{frame}");
        // The filter takes keys only while focused, and narrows the rows.
        press(&mut room, KeyCode::Char('/'));
        assert!(room.filter_focus);
        type_text(&mut room, "linux");
        let frame = draw(&mut room);
        assert!(frame.contains("1 of 4 match") && frame.contains("type to filter · Enter or Esc done"), "{frame}");
        assert!(!frame.contains("Boards of Canada"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(!room.filter_focus);
        press(&mut room, KeyCode::Down);
        assert!(draw(&mut room).contains("↑↓ rows · ←→ tab · / filter · Esc deselect"), "external rows have no r");
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::None));
        press(&mut room, KeyCode::Char('/'));
        for _ in 0..5 {
            press(&mut room, KeyCode::Backspace);
        }
        press(&mut room, KeyCode::Esc);
        assert!(draw(&mut room).contains("4 torrents"));
        // r on an mStream row: the gate, then the daemon's answer.
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Char('r'));
        assert!(matches!(room.modal, Modal::Remove(_)));
        let frame = draw(&mut room);
        assert!(frame.contains("from Transmission?") && frame.contains("The files on disk are KEPT"), "{frame}");
        assert!(frame.contains("y remove · Esc keep"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none(), "Enter is the safe choice");
        press(&mut room, KeyCode::Char('r'));
        press(&mut room, KeyCode::Char('y'));
        let hash: String = "5f4a".repeat(10);
        assert_eq!(room.queued, Some(Op::Remove(hash.clone())));
        room.queued = None;
        room.apply(Done::Removed { hash: hash.clone(), result: Ok(RemoveAnswer { ok: true, daemon_remove_ok: true, daemon_remove_error: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| !*e && n.starts_with("removed Boards of Canada") && n.ends_with("the files on disk stay")));
        assert_eq!(room.queued, Some(Op::List));
        room.apply(Done::Removed { hash: hash.clone(), result: Err(ApiError::NotFound("x".into())) });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.starts_with("mStream did not add this torrent")));
        room.apply(Done::Removed {
            hash,
            result: Ok(RemoveAnswer { ok: true, daemon_remove_ok: false, daemon_remove_error: Some("daemon offline".into()) }),
        });
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("the daemon-side remove failed: daemon offline")));
        // The polls: the list every five seconds here, everything every thirty.
        room.queued = None;
        room.last_load = Some(Instant::now());
        room.last_list = Some(Instant::now() - Duration::from_secs(6));
        room.tick();
        assert_eq!(room.queued, Some(Op::List));
        room.queued = None;
        room.last_load = Some(Instant::now() - Duration::from_secs(31));
        room.tick();
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        // A daemon that went away: the list's error, in gold, and the state line.
        room.apply(Done::Listed(Ok(TorrentList { torrents: Vec::new(), error: Some("connect ECONNREFUSED".into()), client_type: None })));
        room.apply(Done::Status(Ok(TorrentStatus { connected: false, configured: true, reason: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() })));
        let frame = draw(&mut room);
        assert!(frame.contains("• disconnected — connect ECONNREFUSED 127.0.0.1:9091 · credentials kept"), "{frame}");
        assert!(frame.contains("couldn't fetch torrents: connect ECONNREFUSED"), "{frame}");
    }

    #[test]
    fn the_libraries_tab_shows_the_ladder_and_edits_mapping_and_template() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Libraries);
        let frame = draw(&mut room);
        assert!(frame.contains("The paths are as Transmission sees them, not as mStream does"), "{frame}");
        assert!(frame.contains("PATH AS SEEN BY TRANSMISSION") && frame.contains("TEMPLATE"), "{frame}");
        let music = row(&frame, "music");
        assert!(music.contains("✓ verified") && music.contains("/downloads/music") && music.contains("{{ARTIST}}/{{ALBUM}} ({{YEAR}})"), "{music}");
        let vinyl = row(&frame, "vinyl");
        assert!(vinyl.contains("✗ unconfirmed") && vinyl.contains("no mapping yet"), "{vinyl}");
        let books = row(&frame, "audiobooks");
        assert!(books.contains("⟳ probing…") && books.contains("asking the daemon…") && books.contains("(none — typed by hand)"), "{books}");
        assert!(frame.contains("↑↓ select · ←→ tab · D auto-detect all · Esc back"), "{frame}");
        // audiobooks · music · podcasts · vinyl.
        for _ in 0..4 {
            press(&mut room, KeyCode::Down);
        }
        assert_eq!(room.selected_library(), Some("vinyl"));
        let frame = draw(&mut room);
        assert!(frame.contains("vinyl — daemon free-space returned -1 (path not visible to daemon) · probed 3m ago"), "{frame}");
        assert!(frame.contains("↑↓ rows · ←→ tab · d auto-detect · m map by hand · t template · Esc deselect"), "{frame}");
        press(&mut room, KeyCode::Char('d'));
        assert_eq!(room.queued, Some(Op::Detect(Some("vinyl".into()))));
        assert_eq!(room.busy.as_deref(), Some("asking the daemon about vinyl…"));
        room.queued = None;
        room.apply(Done::Detected(Ok(access())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "auto-detect done"));
        press(&mut room, KeyCode::Char('D'));
        assert_eq!(room.queued, Some(Op::Detect(None)));
        room.queued = None;
        room.busy = None;
        // m: the daemon's path, verified on save; a 422 keeps the modal with the reason.
        press(&mut room, KeyCode::Char('m'));
        assert!(matches!(room.modal, Modal::Mapping { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Map vinyl for Transmission") && frame.contains("PATH AS SEEN BY TRANSMISSION") && frame.contains("Verify and save ▸"), "{frame}");
        type_text(&mut room, "/data/vinyl");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Map { vpath: "vinyl".into(), path: "/data/vinyl".into() }));
        room.queued = None;
        room.apply(Done::Mapped { vpath: "vinyl".into(), result: Err(ApiError::Server { status: 422, message: "daemon free-space returned -1".into() }) });
        assert!(matches!(&room.modal, Modal::Mapping { error: Some(e), .. } if e.contains("daemon free-space returned -1")));
        assert!(draw(&mut room).contains("− the daemon could not verify the path: server error 422"));
        room.queued = None;
        room.apply(Done::Mapped {
            vpath: "vinyl".into(),
            result: Ok(serde_json::json!({ "ok": true, "daemonPath": "/data/vinyl", "confidence": "verified" })),
        });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "vinyl mapped → /data/vinyl (verified)"));
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        // t: the template, with the live preview, the suggestion and the clear.
        press(&mut room, KeyCode::Up);
        press(&mut room, KeyCode::Up);
        assert_eq!(room.selected_library(), Some("music"));
        room.note = None;
        press(&mut room, KeyCode::Char('t'));
        let frame = draw(&mut room);
        assert!(frame.contains("Path template — music"), "{frame}");
        assert!(frame.contains("{{ARTIST}} {{ALBUM}} {{YEAR}} {{GENRE}} {{ALBUMARTIST}}"), "{frame}");
        assert!(frame.contains("Pink Floyd/The Dark Side of the Moon (1973)"), "{frame}");
        assert!(frame.contains("^S suggested template") && frame.contains("^X clear") && frame.contains("Save ▸"), "{frame}");
        ctrl(&mut room, 'x');
        assert!(draw(&mut room).contains("(no template — the user types the path)"));
        type_text(&mut room, "{{GENRE}}/{{ARTIST}}");
        assert!(draw(&mut room).contains("Progressive Rock/Pink Floyd"));
        ctrl(&mut room, 's');
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: Some("{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into()) }));
        room.queued = None;
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Err(ApiError::Server { status: 400, message: "unknown variable".into() }) });
        assert!(matches!(&room.modal, Modal::Template { error: Some(e), .. } if e.contains("unknown variable")));
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Ok(TemplateSaved { ok: true, template: Some("{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into()), sample_path: None }) });
        assert!(matches!(room.modal, Modal::None));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "music: template saved"));
        press(&mut room, KeyCode::Char('t'));
        ctrl(&mut room, 'x');
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.queued, Some(Op::Template { vpath: "music".into(), template: None }));
        room.queued = None;
        room.apply(Done::TemplateSaved { vpath: "music".into(), result: Ok(TemplateSaved { ok: true, template: None, sample_path: None }) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "music: template cleared"));
        room.note = None;
        assert!(row(&draw(&mut room), "music").contains("(none — typed by hand)"));
    }

    #[test]
    fn seeding_queues_files_one_at_a_time_and_reads_the_outcomes() {
        let _en = english();
        let dir = std::env::temp_dir().join(format!("mstream-player-seed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("boc.torrent");
        let second = dir.join("kob.torrent");
        std::fs::write(&first, b"d8:announce0:e").unwrap();
        std::fs::write(&second, b"d8:announce0:e").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let mut room = connected();
        press(&mut room, KeyCode::Right);
        press(&mut room, KeyCode::Right);
        assert_eq!(room.tab, Tab::Seeding);
        let frame = draw(&mut room);
        assert!(frame.contains("Add a .torrent file ▸") && frame.contains("SEARCH IN") && frame.contains("[ ] music"), "{frame}");
        assert!(frame.contains("none ticked = every library") && frame.contains("(no files checked yet)"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · a add a .torrent · 1–9 tick a library · c clear results · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('2')); // audiobooks · music · …
        assert!(draw(&mut room).contains("[✓] music"));
        press(&mut room, KeyCode::Char('a'));
        assert!(matches!(room.modal, Modal::SeedPath { .. }));
        let frame = draw(&mut room);
        assert!(frame.contains("Add a .torrent file") && frame.contains(".TORRENT FILE ON THIS MACHINE") && frame.contains("Check and seed ▸"), "{frame}");
        assert!(frame.contains("type · Tab complete · Enter check and seed · Esc cancel"), "{frame}");
        // Tab completes what is typed: folders and .torrent files only.
        type_text(&mut room, &format!("{}/", dir.display()));
        press(&mut room, KeyCode::Tab);
        let Modal::SeedPath { path, matches, .. } = &room.modal else { panic!("the path modal") };
        assert_eq!(matches, &vec!["boc.torrent".to_string(), "kob.torrent".to_string()]);
        assert!(path.value().ends_with("/"), "two candidates share no prefix beyond the folder: {}", path.value());
        type_text(&mut room, "b");
        press(&mut room, KeyCode::Tab);
        let Modal::SeedPath { path, .. } = &room.modal else { panic!("the path modal") };
        assert!(path.value().ends_with("/boc.torrent"), "{}", path.value());
        assert!(draw(&mut room).contains("▸ boc.torrent"));
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.seeds.len(), 1);
        assert_eq!(room.queued, Some(Op::Seed { path: first.clone(), vpaths: vec!["music".into()] }));
        assert!(draw(&mut room).contains("⟳ checking…"));
        // A second file waits its turn.
        press(&mut room, KeyCode::Char('a'));
        type_text(&mut room, &second.display().to_string());
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.seeds.len(), 2);
        assert_eq!(room.seed_queue.len(), 1);
        room.queued = None;
        room.apply(Done::Seeded {
            file: "boc.torrent".into(),
            result: Ok(SeedOutcome { ok: true, outcome: "seeded".into(), vpath: Some("music".into()), added_at: Some("/downloads/music/Boards of Canada".into()), ..Default::default() }),
        });
        assert_eq!(room.queued, Some(Op::Seed { path: second.clone(), vpaths: vec!["music".into()] }), "the queue advances after the list refresh is asked for");
        room.queued = None;
        room.apply(Done::Seeded {
            file: "kob.torrent".into(),
            result: Ok(SeedOutcome {
                ok: true,
                outcome: "partial_match".into(),
                vpath: Some("music".into()),
                matched: Some(9),
                total: Some(12),
                missing: vec!["10 - Flamenco Sketches (alt).flac".into(), "cover.jpg".into(), "log.txt".into(), "x.cue".into()],
                ..Default::default()
            }),
        });
        let frame = draw(&mut room);
        let boc = row(&frame, "boc.torrent");
        assert!(boc.contains("✓ seeding") && boc.contains("music → /downloads/music/Boards of Canada"), "{boc}");
        let kob = row(&frame, "kob.torrent");
        assert!(kob.contains("~ partial") && kob.contains("9/12 matched in music · missing: 10 - Flamenco Sk"), "{kob}");
        assert_eq!(outcome_details(&room.seeds[1], "Transmission"), "9/12 matched in music · missing: 10 - Flamenco Sketches (alt).flac, cover.jpg, log.txt, +1 more");
        // Other outcomes, in the webapp's words.
        let words = |o: &str, extra: SeedOutcome| {
            let row = SeedRow { file: "f".into(), outcome: Some(SeedOutcome { outcome: o.into(), ..extra }), error: None };
            (outcome_words(o).0, outcome_details(&row, "Transmission"))
        };
        assert_eq!(words("match_unmapped", SeedOutcome { vpath: Some("vinyl".into()), matched_root: Some("/Volumes/Vinyl".into()), mapping_confidence: Some("unconfirmed".into()), ..Default::default() }).1,
            "all files in vinyl at /Volumes/Vinyl — the mapping for vinyl is not confirmed: map it on the Libraries tab, then retry");
        assert_eq!(words("pad_files_missing", SeedOutcome { vpath: Some("music".into()), pad_files_present: Some(0), pad_files_total: Some(3), ..Default::default() }).0, "! needs padding");
        assert_eq!(words("no_match", SeedOutcome { checked_vpaths: vec!["music".into(), "vinyl".into()], ..Default::default() }), ("✗ not found".into(), "not in music, vinyl".into()));
        assert_eq!(words("already_in_daemon", SeedOutcome::default()).0, "⊝ already there");
        assert_eq!(words("invalid_torrent", SeedOutcome { error: Some("not a bencoded metainfo".into()), ..Default::default() }), ("✗ invalid".into(), "not a bencoded metainfo".into()));
        // A wrong extension is refused before anything is read; c clears the finished rows.
        press(&mut room, KeyCode::Char('a'));
        type_text(&mut room, &dir.join("notes.txt").display().to_string());
        press(&mut room, KeyCode::Enter);
        assert!(matches!(&room.modal, Modal::SeedPath { error: Some(e), .. } if e == "that is not a .torrent file"));
        press(&mut room, KeyCode::Esc);
        press(&mut room, KeyCode::Char('c'));
        assert!(room.seeds.is_empty());
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "results cleared"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn access_flips_the_policy_and_the_users_and_leaves_with_one_user() {
        let _en = english();
        let mut room = connected();
        for _ in 0..3 {
            press(&mut room, KeyCode::Right);
        }
        assert_eq!(room.tab, Tab::Access);
        let frame = draw(&mut room);
        assert!(frame.contains("WHO CAN ADD TORRENTS") && frame.contains("(•) all users") && frame.contains("( ) whitelist — only the users ticked below"), "{frame}");
        for word in ["USER", "ADMIN", "TORRENTS"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(row(&frame, "iros").contains("admin") && row(&frame, "iros").contains("[✓]"), "{frame}");
        assert!(row(&frame, "guest").contains("[ ]"), "{frame}");
        assert!(frame.contains("↑↓ select · ←→ tab · p policy (all ⇄ whitelist) · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('p'));
        assert_eq!(room.queued, Some(Op::SetPolicy("whitelist".into())));
        room.queued = None;
        room.apply(Done::PolicySet { policy: "whitelist".into(), result: Ok(()) });
        room.note = None;
        assert!(draw(&mut room).contains("(•) whitelist"));
        // dj-tom · guest · iros · mira
        press(&mut room, KeyCode::Down);
        press(&mut room, KeyCode::Down);
        assert_eq!(room.selected_user(), Some("guest"));
        let frame = draw(&mut room);
        assert!(frame.contains("guest may not add torrents · s flips it, at once"), "{frame}");
        press(&mut room, KeyCode::Char('s'));
        assert_eq!(room.queued, Some(Op::UserAccess { user: "guest".into(), allow: true }));
        room.queued = None;
        room.apply(Done::UserAccess { user: "guest".into(), allow: true, result: Ok(()) });
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "guest may add torrents"));
        assert!(row(&draw(&mut room), "guest").contains("[✓]"));
        // Down to one user: the tab is gone and the room moves off it.
        room.queued = None;
        room.apply(loaded(params("transmission", true), users(1), true));
        assert_eq!(room.tabs(), vec![Tab::Torrents, Tab::Libraries, Tab::Seeding, Tab::Client]);
        assert_eq!(room.tab, Tab::Torrents);
        assert!(!draw(&mut room).contains(" Access "));
    }

    #[test]
    fn the_client_tab_tests_switches_and_disconnects() {
        let _en = english();
        let mut room = connected();
        press(&mut room, KeyCode::Left);
        assert_eq!(room.tab, Tab::Client);
        let frame = draw(&mut room);
        assert!(frame.contains("TRANSMISSION   ● connected — Transmission 4.0.6"), "{frame}");
        for word in ["HOST", "127.0.0.1", "PORT", "9091", "(none)", "RPC PATH", "/transmission/rpc", "HTTPS", "no"] {
            assert!(frame.contains(word), "{word}\n{frame}");
        }
        assert!(frame.contains("Disconnecting forgets the credentials here; the daemon keeps every torrent it has."), "{frame}");
        assert!(frame.contains("Test") && frame.contains("Disconnect") && frame.contains("Switch client ▸"), "{frame}");
        assert!(frame.contains("polls every 30 s") && frame.contains("reachable just now"), "{frame}");
        assert!(frame.contains("←→ tab · t test · c switch client · x disconnect · Esc back"), "{frame}");
        press(&mut room, KeyCode::Char('t'));
        assert_eq!(room.queued, Some(Op::Status));
        room.queued = None;
        room.apply(Done::Status(Ok(TorrentStatus { connected: false, configured: true, reason: Some("connect ECONNREFUSED 127.0.0.1:9091".into()), ..Default::default() })));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n == "not reachable: connect ECONNREFUSED 127.0.0.1:9091"));
        let frame = draw(&mut room);
        assert!(frame.contains("• disconnected — connect ECONNREFUSED 127.0.0.1:9091 · credentials kept"), "{frame}");
        assert!(frame.contains("● disconnected — connect ECONNREFUSED 127.0.0.1:9091"), "{frame}");
        // x: the gate, Enter keeps, y goes.
        press(&mut room, KeyCode::Char('x'));
        assert!(matches!(room.modal, Modal::Disconnect));
        let frame = draw(&mut room);
        assert!(frame.contains("Disconnect from Transmission?") && frame.contains("y disconnect · Esc stay"), "{frame}");
        press(&mut room, KeyCode::Enter);
        assert!(matches!(room.modal, Modal::None) && room.queued.is_none());
        press(&mut room, KeyCode::Char('x'));
        press(&mut room, KeyCode::Char('y'));
        assert_eq!(room.queued, Some(Op::Disconnect(Kind::Transmission)));
        room.queued = None;
        room.apply(Done::Disconnected(Ok(())));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n == "disconnected — the credentials are gone"));
        assert_eq!(room.queued, Some(Op::Load));
        // c: back to the client choice, with the other saved client shown; Esc returns.
        room.queued = None;
        let mut p = params("transmission", true);
        p.qbittorrent = TorrentClientConfig { host: "192.168.1.20".into(), port: 8080, configured: true, ..Default::default() };
        room.apply(loaded(p, users(4), true));
        room.tab = Tab::Client;
        room.note = None;
        let frame = draw(&mut room);
        assert!(frame.contains("ALSO SAVED  qBittorrent 192.168.1.20:8080 — c switches without retyping the password"), "{frame}");
        press(&mut room, KeyCode::Char('c'));
        assert_eq!(room.phase(), Phase::Choose);
        let frame = draw(&mut room);
        assert!(frame.contains("(•) Transmission") && frame.contains("Connect to Transmission ▸"), "{frame}");
        press(&mut room, KeyCode::Esc);
        assert_eq!(room.phase(), Phase::Tabs);
        assert!(matches!(press(&mut room, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn words_for_rates_outcomes_and_templates() {
        let _en = english();
        assert_eq!(rate_words(0.0), "—");
        assert_eq!(rate_words(2.4 * 1024.0 * 1024.0), "2.4 MB/s");
        assert_eq!(rate_words(480.0 * 1024.0), "480 KB/s");
        let sample = templates().sample_metadata;
        assert_eq!(preview_template("{{ARTIST}}/{{ALBUM}} ({{YEAR}})", &sample).as_deref(), Some("Pink Floyd/The Dark Side of the Moon (1973)"));
        assert_eq!(preview_template("{{artist}}/{{NOPE}}/{{GENRE}}", &sample).as_deref(), Some("Pink Floyd/Progressive Rock"));
        assert_eq!(preview_template("   ", &sample), None);
        assert_eq!(preview_template("{{NOPE}}", &sample).as_deref(), Some(""));
        assert_eq!(sanitize_segment("AC/DC: Back in Black?"), "AC-DC- Back in Black-");
        assert_eq!(outcome_words("seeded").0, "✓ seeding");
        assert_eq!(confidence_words("pending"), "probing…");
        assert_eq!(status_word("verifying"), "verifying");
        assert_eq!(status_word("weird"), "unknown");
        assert_eq!(common_prefix(&["boc.torrent".into(), "bob.torrent".into()]), "bo");
        assert_eq!(probed_secs(&serde_json::json!((unix_now() - 90) * 1000), unix_now()), Some(90));
        assert_eq!(probed_secs(&serde_json::json!(unix_now() - 3 * 86_400), unix_now()), Some(3 * 86_400));
        assert_eq!(probed_secs(&serde_json::json!(null), unix_now()), None);
        assert_eq!(ago_words("probed", 30), "probed just now");
        assert_eq!(ago_words("probed", 180), "probed 3m ago");
        assert_eq!(ago_words("added", 2 * 86_400), "2d ago");
        assert_eq!(Kind::parse("qbittorrent").label(), "qBittorrent");
        assert_eq!(Kind::parse("nonsense"), Kind::Disabled);
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            assert_eq!(expand_home("~/x.torrent"), format!("{home}/x.torrent"));
        }
        assert_eq!(expand_home("/abs"), "/abs");
    }
}
