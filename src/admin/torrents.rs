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
//! Torrents tab shows something moving — a download, a check, a queue, or
//! a list changed since the last look — and with everything else every
//! thirty once it all rests.
//!
//! This file is the room itself: its state, its worker, its keys and
//! every word it says. It draws nothing, and names no drawing type of
//! ratatui's or the kit's. The cells view (cells.rs), a child module,
//! draws it in character cells, for a terminal and for the GUI player's
//! Admin tab: a click there is one of the room's [`Act`]s and a key is
//! [`Room::key`], and what the view keeps between frames it keeps in a
//! type of its own, held here unopened (`cells::State`). The terminal
//! the room still names is its text fields' editor: each field is a
//! tui_input [`Input`], and a key edits it as crossterm's key event, so
//! the room's own keymap keeps editing its fields whichever view draws
//! them (keys reach the room, never a view).

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Claim, Outcome, age_text, gate_message, iso_unix, printable, short_id, unix_now};
use crate::api::types::{
    AccessRow, AdminUser, PathTemplates, ProbeAnswer, RemoveAnswer, SeedOutcome, TemplateSaved,
    Torrent, TorrentClientConfig, TorrentList, TorrentParams, TorrentStatus, VpathAccess,
};
use crate::api::{ApiError, Client, TorrentCreds};
use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::kit::pace::BRISK_FOR;
use crate::setup::picker::{self, Pick};

/// The room in character cells: the terminal's room and the GUI's grid.
mod cells;
/// The same room drawn by egui, in the desktop window's panel.
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// The daemon's list, while the Torrents tab shows it moving.
const POLL_LIST: Duration = Duration::from_secs(5);
/// Everything else: the status probe, the libraries, the users.
const POLL_STATE: Duration = Duration::from_secs(30);
/// Names on screen (users, libraries, daemon versions).
const NAME_MAX: usize = 64;
/// The server's cap on a template.
const TEMPLATE_MAX: usize = 500;
const PATH_MAX: usize = 400;
const PORT_MAX: u32 = 65_535;
/// How many completions the seed-path modal lists.
const SUGGEST_MAX: usize = 6;
/// How long a Tab in the seed-path modal waits for its folder's listing
/// before the frame goes on without it: a healthy disk lists a folder of
/// thousands well inside a frame at 30 Hz, and a dead mount costs no more.
const SEED_LIST_WAIT: Duration = Duration::from_millis(30);

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
    /// `asked` is what Tab was pressed on, while its folder is being listed.
    SeedPath { path: Input, matches: Vec<String>, error: Option<String>, asked: Option<String> },
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
    Picked(Pick),
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
                Op::PickFile => Done::Picked(picker::pick_torrent(&t!("tor.seed_path_title"), None)),
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

/// The whole load, on the worker: the settings first (those gate
/// everything), then what a configured client can tell. Each wave's calls
/// go out together — the settings beside the users, then the four
/// daemon-side reads — so a load is two round trips, not six, and a daemon
/// that hangs holds it for one ceiling, not two (performance audit #83).
fn load(client: &Client) -> Result<Loaded, ApiError> {
    crate::api::wait(async {
        let (params, users) = tokio::join!(client.admin_torrent_params_async(), client.admin_users_async());
        let params = params?;
        let users = users.unwrap_or_default();
        let kind = Kind::parse(&params.client);
        let mut loaded = Loaded { params, users, status: None, access: None, templates: None, list: None, warning: None };
        if kind != Kind::Disabled && config_of(&loaded.params, kind).configured {
            let (status, access, templates, list) = tokio::join!(
                client.admin_torrent_status_async(),
                client.admin_torrent_vpath_access_async(),
                client.admin_torrent_path_templates_async(),
                client.admin_torrent_list_async(),
            );
            // Folded in the order they were once asked, so the warning
            // still names the first of them that failed.
            let warn = |e: ApiError, w: &mut Option<String>| {
                if w.is_none() {
                    *w = Some(e.to_string());
                }
            };
            match status {
                Ok(s) => loaded.status = Some(s),
                Err(e) => warn(e, &mut loaded.warning),
            }
            match access {
                Ok(a) => loaded.access = Some(a),
                Err(e) => warn(e, &mut loaded.warning),
            }
            match templates {
                Ok(t) => loaded.templates = Some(t),
                Err(e) => warn(e, &mut loaded.warning),
            }
            match list {
                Ok(l) => loaded.list = Some(l),
                Err(e) => warn(e, &mut loaded.warning),
            }
        }
        Ok(loaded)
    })
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
    /// Each torrent's name and hash, lowercased once as the list lands,
    /// for the filter that runs over them every frame (performance audit
    /// #110) — the GUI's torrent picker keeps its names the same way.
    hay: Vec<String>,
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
    /// The seed-path modal's folders being listed, each on a thread of
    /// its own ([`Room::seed_tab`]), and where their listings arrive.
    seed_listing: HashSet<String>,
    listed_tx: Sender<(String, Vec<String>)>,
    listed_rx: Receiver<(String, Vec<String>)>,
    /// [`SEED_LIST_WAIT`], held here so a test can take a listing's slow road.
    seed_wait: Duration,
    /// When the last Tab asked — a listing still out is waited for
    /// briskly from then (see [`Room::listing_awaited`]).
    seed_asked_at: Option<Instant>,
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
    /// The worker thread is gone: its channel is not read again, and its
    /// note stands.
    worker_gone: bool,
    tscroll: usize,
    /// The cursor as the cells view's table last followed it: the table
    /// scrolls to the cursor only on the frame it moved. A tab switch
    /// forgets it ([`Act::Tab`]), so it is the room's to hold, beside the
    /// cursor, rather than the view's.
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    last_list: Option<Instant>,
    /// The last list differed from the one before it — a torrent came or
    /// went, or one's state or progress moved: worth another look soon.
    list_moved: bool,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface.
    cells: cells::State,
}

impl Room {
    pub(super) fn new(client: Client, same_machine: bool) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        let (listed_tx, listed_rx) = std::sync::mpsc::channel();
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
            hay: Vec::new(),
            list_error: None,
            list_loaded: false,
            access: BTreeMap::new(),
            templates: PathTemplates::default(),
            libraries: Vec::new(),
            seeds: Vec::new(),
            seed_ticks: Vec::new(),
            seed_queue: VecDeque::new(),
            seed_listing: HashSet::new(),
            listed_tx,
            listed_rx,
            seed_wait: SEED_LIST_WAIT,
            seed_asked_at: None,
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
            worker_gone: false,
            tscroll: 0,
            sel_anchor: None,
            last_load: None,
            last_list: None,
            list_moved: false,
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
            .zip(&self.hay)
            .enumerate()
            .filter(|(_, (_, hay))| q.is_empty() || hay.contains(&q))
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

    /// Whether the list is worth a look every five seconds: something in
    /// it is moving — downloading, being checked, waiting its turn — or
    /// the last look found it changed. At rest (seeding, paused, stopped)
    /// the thirty-second load keeps it fresh: each look is the daemon
    /// walking its whole list and ~420 bytes a torrent on the wire, and
    /// nothing on screen can have moved (performance audit #100).
    fn list_live(&self) -> bool {
        self.list_moved
            || self.torrents.iter().any(|t| {
                matches!(t.status.as_str(), "downloading" | "verifying" | "queued") || t.rate_download > 0.0
            })
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

    /// A key press: the room's keymap, whichever view draws the room.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
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
        self.modal = Modal::SeedPath { path: Input::default(), matches: Vec::new(), error: None, asked: None };
    }

    /// Tab in the seed-path modal: its folder is listed on a thread of its
    /// own, and the completion lands with the listing — a dead network
    /// mount can hang `read_dir` for as long as it likes, and this is the
    /// thread that draws and reads keys (performance audit #85). Not the
    /// room's worker either: a hung listing there would hold the list poll
    /// and the seed queue behind it. A folder already being listed is not
    /// asked again, so Tabs at a dead mount leave one thread waiting, not
    /// one each; its answer serves the latest Tab. A healthy folder
    /// answers within [`SEED_LIST_WAIT`], and completes in this frame as
    /// Tab always did.
    fn seed_tab(&mut self) {
        let Modal::SeedPath { path, error, asked, .. } = &mut self.modal else { return };
        *error = None;
        *asked = Some(path.value().to_string());
        self.seed_asked_at = Some(Instant::now());
        let (_, _, dir) = seed_dir(path.value());
        if self.seed_listing.insert(dir.clone()) {
            let tx = self.listed_tx.clone();
            std::thread::spawn(move || {
                let names = list_local(&dir);
                let _ = tx.send((dir, names));
            });
        }
        let until = Instant::now() + self.seed_wait;
        while matches!(&self.modal, Modal::SeedPath { asked: Some(_), .. }) {
            match self.listed_rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok((dir, names)) => self.take_listing(dir, names),
                Err(_) => break,
            }
        }
    }

    /// Whether a listing a Tab left out is still worth a brisk wait: its
    /// completion repaints, and one slower than [`SEED_LIST_WAIT`] waited
    /// out a whole poll to show (performance audit #85 follow-up). For
    /// [`BRISK_FOR`] after the Tab and no longer — a dead mount's listing
    /// may never come, and counted for good it would hold the loop's
    /// clock through every worker call after it.
    fn listing_awaited(&self) -> bool {
        !self.seed_listing.is_empty() && self.seed_asked_at.is_some_and(|at| at.elapsed() < BRISK_FOR)
    }

    /// A folder's listing, back: it completes the input its Tab was
    /// pressed on — unless that input has moved on since, or the modal
    /// has closed.
    fn take_listing(&mut self, dir: String, names: Vec<String>) {
        self.seed_listing.remove(&dir);
        let Modal::SeedPath { path, matches, asked, .. } = &mut self.modal else { return };
        if asked.as_deref() != Some(path.value()) || seed_dir(path.value()).2 != dir {
            return;
        }
        *asked = None;
        *matches = complete_from(path, &names);
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
                    self.hay.clear();
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
            Done::Picked(Pick::File(path)) => {
                if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent")) {
                    self.seed(path);
                } else {
                    self.note = Some((t!("tor.err_seed_ext").to_string(), true));
                }
            }
            // A folder from the file dialog cannot happen; said rather
            // than assumed.
            Done::Picked(Pick::Cancelled | Pick::Folder(_)) => {}
            Done::Picked(Pick::Unavailable(e)) => {
                self.note = Some((t!("tor.picker_unavailable", err = printable(&e, 120)).to_string(), true));
                self.open_seed_path();
            }
        }
        self.dispatch_seed();
    }

    fn take_list(&mut self, list: TorrentList) {
        // What a row shows that can move: which torrents, their state and
        // their progress (a download's rate moves only while it downloads).
        let same = |a: &Torrent, b: &Torrent| a.info_hash == b.info_hash && a.status == b.status && a.percent == b.percent;
        self.list_moved = self.torrents.len() != list.torrents.len()
            || !self.torrents.iter().zip(&list.torrents).all(|(a, b)| same(a, b));
        // A line apart: nothing typed into the filter spans the two.
        self.hay = list.torrents.iter().map(|t| format!("{}\n{}", t.name, t.info_hash).to_lowercase()).collect();
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

    /// Fold in whatever the worker finished and whatever folder listings
    /// came back, and say whether anything came: a frame to draw,
    /// whichever view draws it.
    pub(crate) fn absorb(&mut self) -> bool {
        let mut folded = false;
        while !self.worker_gone {
            match self.from_worker.try_recv() {
                Ok(done) => self.apply(done),
                Err(TryRecvError::Empty) => break,
                // Nothing is out with a worker that is gone.
                Err(TryRecvError::Disconnected) => {
                    self.worker_gone = true;
                    self.in_flight = false;
                }
            }
            folded = true;
        }
        // Its note stands for good — one a tab switch cleared is back the
        // next pass — but is set only when it is not up: returning from
        // here with it every pass redrew the room every pass and skipped
        // the listings below, which have threads of their own (performance
        // audit #85 follow-up).
        if self.worker_gone {
            let gone = t!("note.worker_gone");
            if !self.note.as_ref().is_some_and(|(note, _)| *note == gone) {
                self.note = Some((gone.to_string(), true));
                folded = true;
            }
        }
        // The seed path's folder listings, read off the draw thread
        // (performance audit #85): a completion that lands repaints.
        while let Ok((dir, names)) = self.listed_rx.try_recv() {
            self.take_listing(dir, names);
            folded = true;
        }
        folded
    }

    /// Hand the worker the next queued op.
    pub(crate) fn pump(&mut self) {
        self.dispatch_queued();
    }

    /// A call out with the worker, or a seed-path listing still out.
    pub(crate) fn awaiting(&self) -> bool {
        self.in_flight || self.listing_awaited()
    }

    /// The polls, quiet: the list every five seconds while the Torrents
    /// tab shows it moving, everything else — the list with it — every
    /// thirty; only on the tabs page, and never on top of a call already
    /// queued or running.
    pub(crate) fn tick(&mut self) {
        if self.phase() != Phase::Tabs || self.in_flight || self.queued.is_some() {
            return;
        }
        if self.last_load.is_none_or(|t| t.elapsed() >= POLL_STATE) {
            self.reload(false);
        } else if self.tab == Tab::Torrents
            && self.list_live()
            && self.last_list.is_none_or(|t| t.elapsed() >= POLL_LIST)
        {
            self.queued = Some(Op::List);
        }
    }

    /// The tips a host's footer shows while the room is hosted: the words
    /// of the room's own tips row ([`footer_hint`]).
    pub(crate) fn hint(&self) -> String {
        footer_hint(self)
    }

    /// Whether one of the room's modals is up.
    pub(crate) fn modal_open(&self) -> bool {
        !matches!(self.modal, Modal::None)
    }

    /// Every key while a modal is up, on the connect form (its fields take
    /// any letter), and while the filter has the caret. On the client page
    /// Tab walks the two radio groups, so the room keeps Tab as well.
    pub(crate) fn claim(&self) -> Claim {
        if self.modal_open() {
            return Claim::All;
        }
        match self.phase() {
            Phase::Connect => Claim::All,
            Phase::Tabs if self.filter_focus => Claim::All,
            Phase::Choose => Claim::OwnTab,
            Phase::Loading | Phase::Tabs => Claim::Open,
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

/// What is typed in the seed-path modal, split: the folder part (home
/// expanded), the name begun in it, and the folder to list.
fn seed_dir(typed: &str) -> (String, String, String) {
    let raw = expand_home(typed);
    let (dir, prefix) = match raw.rfind('/') {
        Some(i) => (raw[..=i].to_string(), raw[i + 1..].to_string()),
        None => (String::new(), raw.clone()),
    };
    let list_dir = if dir.is_empty() { ".".to_string() } else { dir.clone() };
    (dir, prefix, list_dir)
}

/// A folder's candidates for the seed-path modal — its folders (with their
/// `/`) and its `.torrent` files. On a thread of its own ([`Room::seed_tab`]):
/// a folder that cannot be read has none.
fn list_local(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                Some(format!("{name}/"))
            } else if name.to_lowercase().ends_with(".torrent") {
                Some(name)
            } else {
                None
            }
        })
        .collect()
}

/// Tab in the seed-path modal, once its folder is listed: the longest
/// common completion of what is typed and the candidates to show.
fn complete_from(input: &mut Input, listed: &[String]) -> Vec<String> {
    let (dir, prefix, _) = seed_dir(input.value());
    let mut names: Vec<String> = listed
        .iter()
        .filter(|name| name.starts_with(&prefix) && !(name.starts_with('.') && !prefix.starts_with('.')))
        .cloned()
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
        Modal::SeedPath { path, matches, error, asked } => {
            return match code {
                KeyCode::Esc => room.act(Act::ModalCancel),
                KeyCode::Enter => room.act(Act::ModalSubmit),
                KeyCode::Tab => {
                    room.seed_tab();
                    None
                }
                KeyCode::Char(c) if c.is_control() => None,
                KeyCode::Char(_) if path.value().chars().count() >= PATH_MAX => None,
                _ => {
                    path.handle_event(&TermEvent::Key(key));
                    matches.clear();
                    *error = None;
                    // A listing still out was for the input as it was:
                    // its completion is dropped when it lands.
                    *asked = None;
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

// ── The tips ─────────────────────────────────────────────────────────────────

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::TemplateRow;

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
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"), false)
    }

    pub(super) fn params(client: &str, configured: bool) -> TorrentParams {
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

    pub(super) fn users(n: usize) -> BTreeMap<String, AdminUser> {
        let all = [("iros", true, true), ("mira", false, true), ("guest", false, false), ("dj-tom", false, false)];
        all.iter().take(n).map(|(u, admin, allow)| (u.to_string(), AdminUser { admin: *admin, allow_torrent: *allow, ..Default::default() })).collect()
    }

    pub(super) fn status_ok() -> TorrentStatus {
        TorrentStatus {
            connected: true,
            configured: true,
            client_type: Some("transmission".into()),
            version: Some("4.0.6".into()),
            rpc_version: Some(serde_json::json!(17)),
            reason: None,
        }
    }

    pub(super) fn access() -> VpathAccess {
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

    pub(super) fn templates() -> PathTemplates {
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

    pub(super) fn torrent(seed: &str, name: &str, status: &str, pct: f64, rate: f64, size: u64, by: Option<&str>) -> Torrent {
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

    pub(super) fn list() -> TorrentList {
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

    pub(super) fn loaded(p: TorrentParams, users: BTreeMap<String, AdminUser>, daemon: bool) -> Done {
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
    pub(super) fn connected() -> Room {
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", true), users(4), true));
        room.note = None;
        room
    }

    pub(super) fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(code, KeyModifiers::NONE))
    }

    pub(super) fn ctrl(room: &mut Room, c: char) -> Option<Outcome> {
        handle_key(room, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    pub(super) fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
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
        assert_eq!(cells::outcome_words("seeded").0, "✓ seeding");
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

    #[test]
    fn a_load_asks_in_two_waves() {
        // The settings beside the users, then the four daemon-side reads
        // together: two round trips, not six (performance audit #83).
        let server = super::super::waves::serve(&[2, 4], |path| match path {
            "/api/v1/admin/torrent" => (
                200,
                r#"{"client":"transmission","transmission":{"host":"nas","port":9091,"configured":true}}"#.into(),
            ),
            "/api/v1/admin/torrent/status" => (500, r#"{"error":"status down"}"#.into()),
            "/api/v1/admin/torrent/path-templates" => (500, r#"{"error":"templates down"}"#.into()),
            _ => (200, "{}".into()),
        });
        let loaded = load(&Client::new(&server.url).expect("client")).expect("a load");
        let paths = server.paths();
        assert_eq!((server.peak(), paths.len()), (4, 6), "{paths:?}");
        let mut first: Vec<&str> = paths[..2].iter().map(String::as_str).collect();
        first.sort();
        assert_eq!(first, ["/api/v1/admin/torrent", "/api/v1/admin/users"]);
        // Folded in the order they were once asked: the warning names the
        // status probe, the first of them to fail.
        assert!(loaded.warning.as_deref().is_some_and(|w| w.contains("status down")), "{:?}", loaded.warning);
        assert!(loaded.status.is_none() && loaded.templates.is_none());
        assert!(loaded.access.is_some() && loaded.list.is_some());
        // No client configured: the settings and the users, nothing else.
        let server = super::super::waves::serve(&[2], |_| (200, "{}".into()));
        let loaded = load(&Client::new(&server.url).expect("client")).expect("a load");
        assert_eq!((server.peak(), server.paths().len()), (2, 2));
        assert!(loaded.list.is_none() && loaded.warning.is_none());
    }

    /// Pump the room until the seed-path modal's Tab has had its listing.
    pub(super) fn listed(room: &mut Room) {
        let until = Instant::now() + Duration::from_secs(10);
        while matches!(&room.modal, Modal::SeedPath { asked: Some(_), .. }) && Instant::now() < until {
            room.absorb();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_listing_still_out_is_waited_for_and_lands_after_the_worker_is_gone() {
        // Its completion repaints, so a listing slower than the Tab's own
        // wait keeps the loop brisk as a worker's call does — for
        // BRISK_FOR after its Tab, not for as long as a dead mount holds
        // it (performance audit #85 follow-up).
        let _en = english();
        let mut room = connected();
        assert!(!room.awaiting());
        room.open_seed_path();
        type_text(&mut room, "/nowhere/b");
        room.seed_wait = Duration::ZERO;
        // The folder is being listed already: the Tab asks nothing new, and
        // its listing stays out until the test sends it.
        room.seed_listing.insert("/nowhere/".into());
        press(&mut room, KeyCode::Tab);
        assert!(room.awaiting(), "a listing is out: waited for briskly");
        room.seed_asked_at = room.seed_asked_at.and_then(|at| at.checked_sub(BRISK_FOR));
        assert!(!room.awaiting(), "past BRISK_FOR a listing that never comes holds nothing");
        press(&mut room, KeyCode::Tab);
        assert!(room.awaiting(), "the next Tab waits again");

        // The worker dies with a call out: said once, the room stops
        // redrawing for it, and the listing still lands.
        let (_, dead) = std::sync::mpsc::channel();
        room.from_worker = dead;
        room.in_flight = true;
        assert!(room.absorb(), "the worker's end is news");
        assert!(room.note.as_ref().is_some_and(|(note, err)| *err && note.contains("worker thread is gone")));
        assert!(!room.absorb(), "once: nothing to redraw for on the passes after");
        room.note = None; // a tab switch's
        assert!(room.absorb() && room.note.as_ref().is_some_and(|(_, err)| *err), "the note stands");
        assert!(!room.absorb());
        room.listed_tx.send(("/nowhere/".into(), vec!["albums/".into(), "boc.torrent".into()])).unwrap();
        assert!(room.absorb(), "a listing after the worker's end still lands");
        let Modal::SeedPath { path, asked, .. } = &room.modal else { panic!("the path modal") };
        assert!(path.value() == "/nowhere/boc.torrent" && asked.is_none(), "{}", path.value());
        assert!(!room.awaiting(), "nothing is out: not the listing, not the dead worker's call");
    }

    #[test]
    fn the_claim_follows_the_phase() {
        let _en = english();
        let mut room = new_room();
        assert_eq!(room.phase(), Phase::Loading);
        assert_eq!(room.claim(), Claim::Open);
        room.queued = None;
        room.apply(loaded(params("disabled", false), users(4), false));
        assert_eq!(room.phase(), Phase::Choose);
        assert_eq!(room.claim(), Claim::OwnTab, "Tab walks the radio groups");
        let mut room = new_room();
        room.queued = None;
        room.apply(loaded(params("transmission", false), users(1), false));
        assert_eq!(room.phase(), Phase::Connect);
        assert_eq!(room.claim(), Claim::All, "the form's fields take every letter");
        let mut room = connected();
        assert_eq!(room.phase(), Phase::Tabs);
        assert_eq!(room.claim(), Claim::Open);
        press(&mut room, KeyCode::Char('/'));
        assert!(room.filter_focus);
        assert_eq!(room.claim(), Claim::All, "the filter has the caret");
        press(&mut room, KeyCode::Enter);
        assert_eq!(room.claim(), Claim::Open);
        room.modal = Modal::Disconnect;
        assert!(room.modal_open());
        assert_eq!(room.claim(), Claim::All);
        assert_eq!(room.hint(), t!("tor.hint_disconnect"));
    }
}
