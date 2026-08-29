//! Servers: the header dropdown, the add/edit form, the Manage Servers
//! room and the pairing QR — everything the GUI knows about the saved
//! server list.
//!
//! Two roads touch the network here, deliberately different. Adding or
//! editing an entry runs a ONE-SHOT client on its own thread: the live
//! session — and the api worker's client — are never touched until the
//! new server has actually answered, so a typo'd address is refused
//! without costing the music. SWITCHING is the opposite: it must repoint
//! the session, so it goes through the App's own funnel
//! ([`App::adopt_server`] → `begin()` → the api worker installs the new
//! client), which is also what keeps tunnel identities, token storage and
//! the Connected follow-ups working unchanged.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::{ApiError, Client};
use crate::config::{self, Config};
use crate::kit::{dim, input_display, modal_frame, modal_close, tall_button, tall_secondary};
use crate::kit::theme::{legacy_conhost, th};
use crate::tui::worker::{ApiCmd, Event};
use crate::tui::app::Effect;

use super::{Act, Gui, accent, bright_bold, put, sel};

// ── State ───────────────────────────────────────────────────────────────────

/// What a background probe has said about one saved server so far.
enum Probe {
    Pending,
    Version(String),
    Unreachable,
}

/// Which page of the add flow is showing — the TUI connect screen's own
/// stages, worn modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormStage {
    /// Pick how to reach the server: standard, or Quick Connect.
    Choosing,
    /// Address plus credentials (also the shape edits and sign-ins wear).
    Direct,
    /// Servers found on this network, and a pairing code to paste.
    QuickConnect,
}

/// The add/edit form. `session_login` marks the flavour that signs in to
/// the session's own target through the funnel (a switch that turned out
/// to need credentials) rather than validating on a one-shot client.
pub(crate) struct Form {
    pub stage: FormStage,
    pub server: String,
    pub username: String,
    pub password: String,
    pub self_signed: bool,
    pub public: bool,
    /// Direct: 0 server · 1 username · 2 password · 3 self-signed ·
    /// 4 public. Choosing: 0 standard · 1 Quick Connect.
    pub focus: usize,
    /// The pasted pairing code, and the Quick Connect cursor — an index
    /// into `found`, or `found.len()` for the paste row, which is last.
    pub code: String,
    pub row: usize,
    /// Servers found on this network, for the Quick Connect page.
    pub found: Vec<crate::discovery::DiscoveredServer>,
    pub searching: bool,
    /// The identity being edited, when this is an edit rather than an add.
    pub editing: Option<String>,
    pub submitting: bool,
    pub error: Option<String>,
    /// Adopt the server as the session once it answers. Adds switch;
    /// edits save and stay.
    pub switch: bool,
    pub session_login: bool,
    /// The URL whose plain-http warning has been acknowledged (the TUI
    /// connect form's own rule: warn once, let the answer be yes).
    pub insecure_ack: Option<String>,
}

impl Form {
    pub(crate) const FIELDS: usize = 5;

    fn add() -> Self {
        Form {
            stage: FormStage::Choosing,
            server: String::new(),
            username: String::new(),
            password: String::new(),
            self_signed: false,
            public: false,
            focus: 0,
            code: String::new(),
            row: 0,
            found: Vec::new(),
            searching: false,
            editing: None,
            submitting: false,
            error: None,
            switch: true,
            session_login: false,
            insecure_ack: None,
        }
    }

    /// The paste-a-code row sits after any discovered servers.
    fn paste_row(&self) -> usize {
        self.found.len()
    }

    /// Whether field `i` takes part in the focus cycle right now.
    fn focusable(&self, i: usize) -> bool {
        !(self.public && matches!(i, 1 | 2))
    }

    fn step_focus(&mut self, forward: bool) {
        let n = Self::FIELDS;
        let mut next = self.focus;
        for _ in 0..n {
            next = if forward { (next + 1) % n } else { (next + n - 1) % n };
            if self.focusable(next) {
                break;
            }
        }
        self.focus = next;
    }

    fn value_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            0 => Some(&mut self.server),
            1 => Some(&mut self.username),
            2 => Some(&mut self.password),
            _ => None,
        }
    }
}

/// The pairing modal: the code, plus both renderings — pixels where the
/// terminal draws them, half-blocks elsewhere.
pub(crate) struct Qr {
    pub code: String,
    pub lines: Vec<String>,
    pub art: Option<crate::tui::art::Art>,
    pub label: String,
}

/// What the background threads send home.
enum Reply {
    Version { key: String, version: Option<String> },
    Add(Box<Outcome>),
}

struct Outcome {
    /// Normalized URL that was reached.
    url: String,
    username: Option<String>,
    token: Option<String>,
    self_signed: bool,
    switch: bool,
    editing: Option<String>,
    result: Result<(), String>,
}

pub(crate) struct ServersUi {
    /// The header dropdown is open.
    pub drop_open: bool,
    /// The Manage Servers room replaces the Settings rows.
    pub room: bool,
    /// Row cursor in the room: an index into the saved list, or the add
    /// row at `len`.
    pub cursor: usize,
    pub form: Option<Form>,
    /// Remove confirmation, by index into the saved list.
    pub confirm: Option<usize>,
    pub qr: Option<Qr>,
    /// A switch in flight: the identity being adopted, for routing
    /// NeedsLogin/TunnelReady into the form.
    pub switching: Option<String>,
    /// A pairing-code dial in flight. Held OUTSIDE the session until the
    /// tunnel answers: the code is only seated (and the old server's
    /// state shed) on Connected/TunnelReady, so a bad code costs an error
    /// line, never the session that was playing.
    pub pending_code: Option<String>,
    versions: HashMap<String, Probe>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
}

impl ServersUi {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        ServersUi {
            drop_open: false,
            room: false,
            cursor: 0,
            form: None,
            confirm: None,
            qr: None,
            switching: None,
            pending_code: None,
            versions: HashMap::new(),
            tx,
            rx,
        }
    }

    /// Whether any servers surface owns the pointer and keyboard.
    pub(crate) fn modal_open(&self) -> bool {
        self.form.is_some() || self.confirm.is_some() || self.qr.is_some()
    }

    fn version_label(&self, key: &str) -> String {
        match self.versions.get(key) {
            Some(Probe::Pending) => "…".to_string(),
            Some(Probe::Version(v)) => format!("v{v}"),
            Some(Probe::Unreachable) | None => "—".to_string(),
        }
    }
}

// ── Opening the surfaces ────────────────────────────────────────────────────

/// Open the Manage Servers room and start version probes for every entry
/// that can be asked directly. A tunnel identity is not an address; the one
/// exception is the live session, whose loopback bridge can answer for it.
pub(crate) fn open_room(gui: &mut Gui) {
    gui.servers.room = true;
    gui.servers.cursor = 0;
    let entries: Vec<(String, bool, bool)> = gui
        .config
        .servers
        .iter()
        .map(|s| (s.url.clone(), s.self_signed, crate::quickconnect::is_tunnel_id(&s.url)))
        .collect();
    for (url, self_signed, tunnel) in entries {
        let target = if tunnel {
            let live = gui.app.connected
                && config::same_server(&gui.app.session.server_id, &url)
                && !gui.app.session.server.is_empty();
            if !live {
                continue;
            }
            gui.app.session.server.clone()
        } else {
            url.clone()
        };
        probe_version(gui, url, target, self_signed);
    }
}

fn probe_version(gui: &mut Gui, key: String, target: String, self_signed: bool) {
    if matches!(gui.servers.versions.get(&key), Some(Probe::Pending | Probe::Version(_))) {
        return;
    }
    gui.servers.versions.insert(key.clone(), Probe::Pending);
    let tx = gui.servers.tx.clone();
    std::thread::spawn(move || {
        let version = Client::new_with(&target, self_signed)
            .ok()
            .and_then(|c| c.server_info().ok())
            .and_then(|info| info.version);
        let _ = tx.send(Reply::Version { key, version });
    });
}

pub(crate) fn open_add(gui: &mut Gui) {
    gui.servers.drop_open = false;
    gui.servers.form = Some(Form::add());
}

fn open_edit(gui: &mut Gui, index: usize) {
    let Some(entry) = gui.config.servers.get(index) else { return };
    if crate::quickconnect::is_tunnel_id(&entry.url) {
        return; // a tunnel identity is not an editable address
    }
    gui.servers.form = Some(Form {
        stage: FormStage::Direct,
        server: entry.url.clone(),
        username: entry.username.clone().unwrap_or_default(),
        self_signed: entry.self_signed,
        editing: Some(entry.url.clone()),
        switch: false,
        ..Form::add()
    });
}

/// Turn to the Quick Connect page and start a browse of this network.
/// mDNS has no "that's everyone" signal, so the reply lands whenever the
/// worker's listening window closes.
fn open_quick_connect(gui: &mut Gui) {
    if let Some(form) = gui.servers.form.as_mut() {
        form.stage = FormStage::QuickConnect;
        form.error = None;
        form.searching = true;
        form.found.clear();
        form.row = 0;
    }
    gui.pend(vec![Effect::Discover]);
}

/// A discovered server is reachable directly — carry its address to the
/// standard page, where credentials and the checkboxes live.
fn pick_discovered(gui: &mut Gui, index: usize) {
    if let Some(form) = gui.servers.form.as_mut()
        && let Some(server) = form.found.get(index)
    {
        form.server = server.base_url.clone();
        form.stage = FormStage::Direct;
        form.focus = 0;
        form.error = None;
    }
}

/// One page back: the add flow returns to the chooser; anything that
/// arrived on the Direct page directly (an edit, a sign-in) just closes.
fn form_back(gui: &mut Gui) {
    let close = match gui.servers.form.as_ref() {
        None => return,
        Some(f) => f.session_login || f.editing.is_some() || f.stage == FormStage::Choosing,
    };
    if close {
        gui.servers.form = None;
        gui.servers.pending_code = None;
    } else if let Some(form) = gui.servers.form.as_mut() {
        form.stage = FormStage::Choosing;
        form.error = None;
        form.focus = 0;
    }
}

fn open_qr(gui: &mut Gui, index: usize) {
    let Some(entry) = gui.config.servers.get(index) else { return };
    let url = entry.url.clone();
    let label = crate::quickconnect::display_server(&url);
    let code = config::load_credentials()
        .ok()
        .and_then(|credentials| config::pairing_for(&credentials, &url));
    let Some(code) = code else {
        gui.note = Some((t!("gui.srv.no_code").to_string(), true));
        return;
    };
    gui.servers.qr = Some(Qr {
        lines: crate::setup::qr_lines(&code).unwrap_or_default(),
        art: crate::setup::qr_art(&code),
        code,
        label,
    });
}

// ── Config writes ───────────────────────────────────────────────────────────

/// Every server-list write loads fresh, mutates, saves, and hands the
/// fresh copy back to the Gui — so a stale in-memory config can never
/// undo what another flow (a connect's SaveSession, an exit-time
/// remember) has written since boot.
fn update_config(gui: &mut Gui, mutate: impl FnOnce(&mut Config)) -> bool {
    if !gui.config_ok {
        return false;
    }
    let mut config = match config::load() {
        Ok(config) => config,
        Err(e) => {
            gui.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
            return false;
        }
    };
    mutate(&mut config);
    match config::save(&config) {
        Ok(()) => {
            gui.config = config;
            true
        }
        Err(e) => {
            gui.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
            false
        }
    }
}

fn make_default(gui: &mut Gui, index: usize) {
    let Some(url) = gui.config.servers.get(index).map(|s| s.url.clone()) else { return };
    if update_config(gui, |config| config::set_default_server(config, Some(&url))) {
        let shown = crate::quickconnect::display_server(&url);
        gui.note = Some((t!("gui.srv.made_default", server = shown).to_string(), false));
    }
}

fn remove_server(gui: &mut Gui, index: usize) {
    let Some(url) = gui.config.servers.get(index).map(|s| s.url.clone()) else { return };
    let mut credentials = match config::load_credentials() {
        Ok(credentials) => credentials,
        Err(_) => config::Credentials::default(),
    };
    let saved = update_config(gui, |config| {
        config::remove_server(config, &mut credentials, &url);
    });
    if saved {
        if config::save_credentials(&credentials).is_err() {
            gui.note = Some((t!("note.settings_save_failed", err = "credentials").to_string(), true));
        } else {
            let shown = crate::quickconnect::display_server(&url);
            gui.note = Some((t!("gui.srv.removed", server = shown).to_string(), false));
        }
        gui.servers.versions.remove(&url);
        if gui.servers.switching.as_deref().is_some_and(|s| config::same_server(s, &url)) {
            gui.servers.switching = None;
        }
    }
    let rows = gui.config.servers.len(); // the add row sits at len
    gui.servers.cursor = gui.servers.cursor.min(rows);
}

// ── Switching ───────────────────────────────────────────────────────────────

/// Adopt saved entry `index` as the session. The playing track keeps
/// playing (its URL is already absolute); the rest of the queue cannot
/// follow (queued tracks resolve against the session's server at play
/// time), so it is cleared and the note says so.
pub(crate) fn switch_to(gui: &mut Gui, index: usize) {
    gui.servers.drop_open = false;
    let Some(entry) = gui.config.servers.get(index).cloned() else { return };
    if gui.app.connected && config::same_server(&gui.app.session.server_id, &entry.url) {
        return; // already there
    }
    let credentials = config::load_credentials().unwrap_or_default();
    let token = config::token_for(&credentials, &entry.url);
    let (server, tunnel_code) = if crate::quickconnect::is_tunnel_id(&entry.url) {
        let Some(code) = config::pairing_for(&credentials, &entry.url) else {
            gui.note = Some((t!("gui.srv.no_code").to_string(), true));
            return;
        };
        (String::new(), Some(code))
    } else {
        (entry.url.clone(), None)
    };

    // The outgoing session's place is worth keeping before it is replaced.
    if gui.app.connected {
        crate::tui::remember(&gui.app);
        if let Ok(fresh) = config::load() {
            gui.config = fresh;
        }
    }

    let had_queue = !gui.app.queue.items.is_empty();
    let effects = gui.app.adopt_server(
        server,
        entry.url.clone(),
        entry.username.clone(),
        token,
        tunnel_code,
        entry.self_signed,
        entry.last_path.clone(),
    );
    gui.pend(effects);
    gui.servers.switching = Some(entry.url.clone());
    let shown = crate::quickconnect::display_server(&entry.url);
    gui.note = Some(if had_queue {
        (t!("gui.srv.queue_cleared", server = shown).to_string(), false)
    } else {
        (t!("gui.srv.reaching", server = shown).to_string(), false)
    });
}

// ── The form's submit ───────────────────────────────────────────────────────

pub(crate) fn submit_form(gui: &mut Gui) {
    // The pages before the Direct form settle without a snapshot.
    match gui.servers.form.as_ref().map(|f| (f.stage, f.submitting)) {
        None | Some((_, true)) => return,
        Some((FormStage::Choosing, _)) => {
            let focus = gui.servers.form.as_ref().is_some_and(|f| f.focus == 1);
            if focus {
                open_quick_connect(gui);
            } else if let Some(form) = gui.servers.form.as_mut() {
                form.stage = FormStage::Direct;
                form.focus = 0;
            }
            return;
        }
        Some((FormStage::QuickConnect, _)) => {
            let (row, on_paste, code) = {
                let Some(form) = gui.servers.form.as_ref() else { return };
                (form.row, form.row >= form.paste_row(), form.code.trim().to_string())
            };
            if !on_paste {
                pick_discovered(gui, row);
                return;
            }
            if code.is_empty() {
                if let Some(form) = gui.servers.form.as_mut() {
                    form.error = Some(t!("gui.srv.need_code").to_string());
                }
                return;
            }
            // The dial rides the funnel: only the api worker can host the
            // tunnel bridge, and it keeps the CURRENT session's bridge up
            // until the new tunnel actually answers (finding #20). The
            // code waits in `pending_code` — see the field's note.
            if let Some(form) = gui.servers.form.as_mut() {
                form.submitting = true;
                form.error = None;
            }
            gui.servers.pending_code = Some(code.clone());
            gui.pend(vec![Effect::Api(ApiCmd::QuickConnect { code, token: None })]);
            return;
        }
        Some((FormStage::Direct, false)) => {}
    }

    // Snapshot first: everything below writes back through `gui`, so the
    // form's borrow must not outlive this block.
    let snapshot = {
        let Some(form) = gui.servers.form.as_mut() else { return };
        if form.submitting {
            return;
        }
        form.error = None;
        let server = match crate::api::server_url::normalize(&form.server) {
            Ok(server) => server,
            Err(message) => {
                form.error = Some(message);
                return;
            }
        };
        form.server = server.clone();
        let username = form.username.trim().to_string();
        if !form.public && username.is_empty() {
            form.error = Some(t!("gui.srv.need_username").to_string());
            return;
        }
        if !form.public && !form.session_login && form.editing.is_none() && form.password.is_empty()
        {
            form.error = Some(t!("gui.srv.need_password").to_string());
            return;
        }
        (
            server,
            username,
            form.public,
            form.self_signed,
            form.password.clone(),
            form.editing.clone(),
            form.switch,
            form.session_login,
        )
    };
    let (server, username, public, self_signed, password, editing, switch, session_login) =
        snapshot;

    // The session-login flavour goes through the funnel: the api worker
    // installs the client, Connected repoints the session, SaveSession
    // persists — exactly the TUI connect screen's road.
    if session_login {
        if !public && password.is_empty() {
            if let Some(form) = gui.servers.form.as_mut() {
                form.error = Some(t!("gui.srv.need_password").to_string());
            }
            return;
        }
        gui.app.session.self_signed = self_signed;
        let effect = if public {
            Effect::Api(ApiCmd::Connect { server, token: None, self_signed })
        } else {
            Effect::Api(ApiCmd::Login { server, username, password, self_signed })
        };
        if let Some(form) = gui.servers.form.as_mut() {
            form.submitting = true;
            form.password.clear();
        }
        gui.pend(vec![effect]);
        return;
    }

    // An edit that types no password is an offline save: the entry's
    // spelling changes, nothing needs the network to be true.
    if let Some(old) = editing.clone()
        && !public
        && password.is_empty()
    {
        let username = (!username.is_empty()).then_some(username);
        let new_url = server.clone();
        let mut credentials = config::load_credentials().unwrap_or_default();
        let moved = !config::same_server(&old, &new_url);
        let saved = update_config(gui, |config| {
            if let Some(entry) =
                config.servers.iter_mut().find(|s| config::same_server(&s.url, &old))
            {
                entry.url = new_url.clone();
                if username.is_some() {
                    entry.username = username.clone();
                }
                entry.self_signed = self_signed;
            }
            if moved
                && config.default_server.as_deref().is_some_and(|d| config::same_server(d, &old))
            {
                config.default_server = Some(new_url.clone());
            }
        });
        if saved {
            if moved {
                // The token was issued to the same server under its old
                // spelling; it moves with the entry.
                if let Some(token) = config::token_for(&credentials, &old) {
                    config::store_token(&mut credentials, &old, None);
                    config::store_token(&mut credentials, &new_url, Some(token));
                    let _ = config::save_credentials(&credentials);
                }
                gui.servers.versions.remove(&old);
            }
            let shown = crate::quickconnect::display_server(&new_url);
            gui.note = Some((t!("gui.srv.saved", server = shown).to_string(), false));
            gui.servers.form = None;
            if gui.servers.room {
                probe_version(gui, new_url.clone(), new_url, self_signed);
            }
        }
        return;
    }

    // Plain http past the local network puts the password on the wire in
    // the clear. Say so once, and let the answer be yes.
    if !public && crate::api::server_url::crosses_the_internet_unencrypted(&server) {
        let warned = {
            let Some(form) = gui.servers.form.as_mut() else { return };
            if form.insecure_ack.as_deref() != Some(server.as_str()) {
                form.insecure_ack = Some(server.clone());
                form.error =
                    Some(t!("gui.srv.plain_http", server = server.clone()).to_string());
                true
            } else {
                false
            }
        };
        if warned {
            return;
        }
    }

    if let Some(form) = gui.servers.form.as_mut() {
        form.submitting = true;
        form.password.clear();
    }
    let tx = gui.servers.tx.clone();
    let outcome_base = Outcome {
        url: server.clone(),
        username: (!public).then(|| username.clone()),
        token: None,
        self_signed,
        switch,
        editing,
        result: Ok(()),
    };
    std::thread::spawn(move || {
        let mut outcome = outcome_base;
        let attempt = (|| -> Result<Option<String>, String> {
            let mut client =
                Client::new_with(&server, self_signed).map_err(|e| e.to_string())?;
            if public {
                match client.ping() {
                    Ok(_) => Ok(None),
                    Err(ApiError::Unauthorized) => Err(t!("gui.srv.public_auth").to_string()),
                    Err(e) => Err(e.to_string()),
                }
            } else {
                match client.login(&username, &password) {
                    Ok(resp) => Ok(Some(resp.token)),
                    Err(ApiError::Unauthorized) => Err(t!("gui.srv.bad_login").to_string()),
                    Err(e) => Err(e.to_string()),
                }
            }
        })();
        match attempt {
            Ok(token) => outcome.token = token,
            Err(message) => outcome.result = Err(message),
        }
        let _ = tx.send(Reply::Add(Box::new(outcome)));
    });
}

// ── Background replies ──────────────────────────────────────────────────────

/// Drain what the threads sent since the last pass.
pub(crate) fn poll(gui: &mut Gui) {
    loop {
        let reply = match gui.servers.rx.try_recv() {
            Ok(reply) => reply,
            Err(_) => return,
        };
        match reply {
            Reply::Version { key, version } => {
                let state = version.map_or(Probe::Unreachable, Probe::Version);
                gui.servers.versions.insert(key, state);
            }
            Reply::Add(outcome) => apply_outcome(gui, *outcome),
        }
    }
}

fn apply_outcome(gui: &mut Gui, outcome: Outcome) {
    if let Some(form) = gui.servers.form.as_mut() {
        form.submitting = false;
        if let Err(message) = &outcome.result {
            form.error = Some(message.clone());
            return;
        }
    } else if outcome.result.is_err() {
        return; // the form was closed while the thread was out
    }

    let Outcome { url, username, token, self_signed, switch, editing, .. } = outcome;
    let mut credentials = config::load_credentials().unwrap_or_default();
    let moved = editing.as_deref().is_some_and(|old| !config::same_server(old, &url));
    let saved = update_config(gui, |config| {
        match &editing {
            Some(old) => {
                if let Some(entry) =
                    config.servers.iter_mut().find(|s| config::same_server(&s.url, old))
                {
                    entry.url = url.clone();
                    entry.username = username.clone();
                    entry.self_signed = self_signed;
                }
                if moved
                    && config.default_server.as_deref().is_some_and(|d| config::same_server(d, old))
                {
                    config.default_server = Some(url.clone());
                }
            }
            None => {
                config::touch_server(config, &url, username.clone());
                if let Some(entry) =
                    config.servers.iter_mut().find(|s| config::same_server(&s.url, &url))
                {
                    entry.self_signed = self_signed;
                    if username.is_none() {
                        // Public mode: no accounts, so no name to sign in as.
                        entry.username = None;
                    }
                }
            }
        }
    });
    if !saved {
        return;
    }
    if let Some(old) = editing.as_deref().filter(|_| moved) {
        config::store_token(&mut credentials, old, None);
        gui.servers.versions.remove(old);
    }
    config::store_token(&mut credentials, &url, token);
    if config::save_credentials(&credentials).is_err() {
        gui.note = Some((t!("note.settings_save_failed", err = "credentials").to_string(), true));
    }

    gui.servers.form = None;
    let shown = crate::quickconnect::display_server(&url);
    if switch {
        if let Some(index) =
            gui.config.servers.iter().position(|s| config::same_server(&s.url, &url))
        {
            switch_to(gui, index);
        }
    } else {
        gui.note = Some((t!("gui.srv.saved", server = shown).to_string(), false));
    }
    if gui.servers.room {
        probe_version(gui, url.clone(), url, self_signed);
    }
}

// ── Session events ──────────────────────────────────────────────────────────

/// Watch the worker's session events go by, BEFORE the App applies them —
/// the GUI has no TUI connect screen, so the answers that would land there
/// open the form instead.
pub(crate) fn observe(gui: &mut Gui, event: &Event) {
    match event {
        Event::Connected { .. } => {
            gui.servers.switching = None;
            // A pairing-code dial just answered: the session about to be
            // seated by this event is the tunnel's, so the code goes in
            // now — nothing later knows it — and the old server's state
            // is shed the way a switch sheds it.
            if let Some(code) = gui.servers.pending_code.take() {
                gui.app.session.tunnel_code = Some(code);
                let had_queue = !gui.app.queue.items.is_empty();
                gui.app.shed_server_state();
                if had_queue && let Event::Connected { id, .. } = event {
                    let shown = crate::quickconnect::display_server(id);
                    gui.note =
                        Some((t!("gui.srv.queue_cleared", server = shown).to_string(), false));
                }
            }
            if gui.servers.form.as_ref().is_some_and(|f| f.session_login || f.switch) {
                gui.servers.form = None;
            }
        }
        Event::ServersDiscovered(found) => {
            if let Some(form) = gui.servers.form.as_mut()
                && form.stage == FormStage::QuickConnect
            {
                // The TUI's cursor rule: someone mid-paste keeps their
                // place; otherwise the cursor lands on the first server.
                let entered_a_code = !form.code.trim().is_empty();
                form.searching = false;
                form.found = found.clone();
                form.row =
                    if entered_a_code { form.paste_row() } else { form.row.min(form.paste_row()) };
            }
        }
        Event::NeedsLogin { server } if gui.servers.switching.is_some() => {
            let identity = gui.servers.switching.clone().unwrap_or_default();
            let entry = gui
                .config
                .servers
                .iter()
                .find(|s| config::same_server(&s.url, &identity));
            gui.servers.form = Some(Form {
                stage: FormStage::Direct,
                server: server.clone(),
                username: entry.and_then(|e| e.username.clone()).unwrap_or_default(),
                self_signed: entry.is_some_and(|e| e.self_signed),
                focus: 1,
                switch: true,
                session_login: true,
                ..Form::add()
            });
            gui.note = Some((t!("gui.srv.sign_in").to_string(), false));
        }
        Event::TunnelReady { local_url, .. }
            if gui.servers.switching.is_some() || gui.servers.pending_code.is_some() =>
        {
            // A fresh dial's code is seated now — the sign-in about to
            // happen ends in a Connected whose save needs it. The pending
            // marker stays armed until then, so that Connected still
            // sheds the old server's state.
            if let Some(code) = gui.servers.pending_code.clone() {
                gui.app.session.tunnel_code = Some(code);
            }
            let identity = gui.servers.switching.clone().unwrap_or_default();
            let entry = gui
                .config
                .servers
                .iter()
                .find(|s| config::same_server(&s.url, &identity));
            gui.servers.form = Some(Form {
                stage: FormStage::Direct,
                server: local_url.clone(),
                username: entry.and_then(|e| e.username.clone()).unwrap_or_default(),
                focus: 1,
                switch: true,
                session_login: true,
                ..Form::add()
            });
            gui.note = Some((t!("gui.srv.tunnel_signin").to_string(), false));
        }
        Event::Unauthorized if gui.app.connected => {
            // An established session went bad; offer the sign-in for the
            // server we were already on, the way the TUI does.
            gui.servers.form = Some(Form {
                stage: FormStage::Direct,
                server: gui.app.session.server.clone(),
                username: gui.app.session.username.clone().unwrap_or_default(),
                self_signed: gui.app.session.self_signed,
                focus: 1,
                switch: true,
                session_login: true,
                ..Form::add()
            });
        }
        Event::Error(message) => {
            // A dial that failed leaves the session exactly as it was —
            // the code never reached it. The error lands on the form.
            let dialling = gui.servers.pending_code.is_some();
            if let Some(form) = gui.servers.form.as_mut()
                && (form.session_login || dialling)
                && form.submitting
            {
                form.submitting = false;
                form.error = Some(message.clone());
            }
            if dialling {
                gui.servers.pending_code = None;
            }
        }
        _ => {}
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The servers side of [`Gui::act`]. Returns true when the act was one of
/// ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act {
        Act::SrvMenu => gui.servers.drop_open = !gui.servers.drop_open,
        Act::SrvCloseDrop => gui.servers.drop_open = false,
        Act::SrvDrop(i) => switch_to(gui, *i),
        Act::SrvAdd => open_add(gui),
        Act::SrvRow(i) => gui.servers.cursor = *i,
        Act::SrvSwitch(i) => switch_to(gui, *i),
        Act::SrvEdit(i) => open_edit(gui, *i),
        Act::SrvDefault(i) => make_default(gui, *i),
        Act::SrvQr(i) => open_qr(gui, *i),
        Act::SrvRemove(i) => gui.servers.confirm = Some(*i),
        Act::SrvConfirm(yes) => {
            if let Some(index) = gui.servers.confirm.take()
                && *yes
            {
                remove_server(gui, index);
            }
        }
        Act::FormFocus(i) => {
            if let Some(form) = gui.servers.form.as_mut() {
                // On the Quick Connect page the one focus is the row
                // cursor (the paste card registers its own row index);
                // on the others it is the field cycle.
                if form.stage == FormStage::QuickConnect {
                    form.row = (*i).min(form.paste_row());
                } else if form.focusable(*i) {
                    form.focus = *i;
                }
            }
        }
        Act::FormToggle(i) => {
            if let Some(form) = gui.servers.form.as_mut() {
                match i {
                    3 => form.self_signed = !form.self_signed,
                    _ => {
                        form.public = !form.public;
                        if form.public && matches!(form.focus, 1 | 2) {
                            form.focus = 0;
                        }
                    }
                }
                form.focus = (*i).min(Form::FIELDS - 1);
            }
        }
        Act::FormMethod(i) => {
            if let Some(form) = gui.servers.form.as_mut() {
                form.focus = (*i).min(1);
            }
            if *i == 1 {
                open_quick_connect(gui);
            } else if let Some(form) = gui.servers.form.as_mut() {
                form.stage = FormStage::Direct;
                form.focus = 0;
            }
        }
        Act::FormPick(i) => pick_discovered(gui, *i),
        Act::FormBack => form_back(gui),
        Act::FormSubmit => submit_form(gui),
        Act::FormCancel => {
            gui.servers.form = None;
            // A dial walked away from must not ambush a later connect.
            gui.servers.pending_code = None;
        }
        Act::QrClose => gui.servers.qr = None,
        Act::Guard => {}
        _ => return false,
    }
    true
}

/// The servers side of the key handler: Some(quit) when a servers surface
/// owned the key, None to fall through. Runs before everything except
/// ctrl+c, so an open modal really owns the keyboard.
pub(crate) fn handle_key(gui: &mut Gui, key: ratatui::crossterm::event::KeyEvent) -> Option<bool> {
    use ratatui::crossterm::event::KeyCode;

    if gui.servers.qr.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
            gui.servers.qr = None;
        }
        return Some(false);
    }

    if gui.servers.confirm.is_some() {
        match key.code {
            KeyCode::Char('y') => {
                gui.act(Act::SrvConfirm(true));
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                gui.act(Act::SrvConfirm(false));
            }
            _ => {}
        }
        return Some(false);
    }

    if gui.servers.form.is_some() {
        let (stage, submitting, session_login, focus) = {
            let Some(form) = gui.servers.form.as_ref() else { return Some(false) };
            (form.stage, form.submitting, form.session_login, form.focus)
        };
        if submitting {
            if key.code == KeyCode::Esc && !session_login {
                gui.servers.form = None;
                gui.servers.pending_code = None;
            }
            return Some(false);
        }
        match stage {
            FormStage::Choosing => match key.code {
                KeyCode::Esc => form_back(gui),
                KeyCode::Enter => submit_form(gui),
                KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.focus = 1 - form.focus.min(1);
                    }
                }
                _ => {}
            },
            FormStage::QuickConnect => match key.code {
                KeyCode::Esc => form_back(gui),
                KeyCode::Enter => submit_form(gui),
                KeyCode::Up => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.row = form.row.saturating_sub(1);
                    }
                }
                KeyCode::Down | KeyCode::Tab => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.row = (form.row + 1).min(form.paste_row());
                    }
                }
                KeyCode::Backspace => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.code.pop();
                    }
                }
                KeyCode::Char(c) => {
                    // Typing anywhere means "I have a code", the TUI's rule.
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.row = form.paste_row();
                        form.code.push(c);
                    }
                }
                _ => {}
            },
            FormStage::Direct => match key.code {
                KeyCode::Esc => form_back(gui),
                KeyCode::Enter => submit_form(gui),
                KeyCode::Tab | KeyCode::Down => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.step_focus(true);
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    if let Some(form) = gui.servers.form.as_mut() {
                        form.step_focus(false);
                    }
                }
                KeyCode::Char(' ') if focus >= 3 => {
                    gui.act(Act::FormToggle(focus));
                }
                KeyCode::Backspace => {
                    if let Some(form) = gui.servers.form.as_mut()
                        && let Some(value) = form.value_mut()
                    {
                        value.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(form) = gui.servers.form.as_mut()
                        && let Some(value) = form.value_mut()
                    {
                        value.push(c);
                    }
                }
                _ => {}
            },
        }
        return Some(false);
    }

    if gui.servers.drop_open {
        if key.code == KeyCode::Esc {
            gui.servers.drop_open = false;
            return Some(false);
        }
        gui.servers.drop_open = false; // any other key falls through, closed
        return None;
    }

    if gui.servers.room && gui.active == super::SETTINGS_NAV {
        let rows = gui.config.servers.len(); // + the add row at `rows`
        match key.code {
            KeyCode::Esc => gui.servers.room = false,
            KeyCode::Down => gui.servers.cursor = (gui.servers.cursor + 1).min(rows),
            KeyCode::Up => gui.servers.cursor = gui.servers.cursor.saturating_sub(1),
            KeyCode::Enter => {
                let cursor = gui.servers.cursor;
                if cursor >= rows {
                    open_add(gui);
                } else {
                    switch_to(gui, cursor);
                }
            }
            KeyCode::Char('e') => {
                let cursor = gui.servers.cursor;
                if cursor < rows {
                    open_edit(gui, cursor);
                }
            }
            KeyCode::Char('d') => {
                let cursor = gui.servers.cursor;
                if cursor < rows {
                    make_default(gui, cursor);
                }
            }
            KeyCode::Char('p') => {
                let cursor = gui.servers.cursor;
                if cursor < rows
                    && gui
                        .config
                        .servers
                        .get(cursor)
                        .is_some_and(|s| crate::quickconnect::is_tunnel_id(&s.url))
                {
                    open_qr(gui, cursor);
                }
            }
            KeyCode::Char('x') => {
                let cursor = gui.servers.cursor;
                if cursor < rows {
                    gui.servers.confirm = Some(cursor);
                }
            }
            _ => return None,
        }
        return Some(false);
    }

    None
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The header's right corner: the server label (a dropdown trigger once a
/// second server is saved) and the [+] that adds one.
pub(crate) fn draw_header(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let plus = Rect { x: area.width.saturating_sub(5), y: 0, width: 3, height: 1 };
    let plus_hover = gui.ui.pointer.is_some_and(|p| plus.contains(p));
    put(frame, plus.x, 0, "[+]", if plus_hover { bright_bold() } else { dim() });
    gui.ui.click(plus, Act::SrvAdd);
    gui.ui.tip(plus, t!("gui.srv.add").to_string());

    if !gui.app.connected {
        return;
    }
    let server = crate::quickconnect::display_server(&gui.app.session.server_id);
    let many = gui.config.servers.len() > 1;
    let chevron = if legacy_conhost() { " v" } else { " ▾" };
    let label = if many { format!("{server}{chevron}") } else { server };
    let width = label.chars().count() as u16;
    let x = plus.x.saturating_sub(width + 1);
    let rect = Rect { x, y: 0, width, height: 1 };
    let hover = many && gui.ui.pointer.is_some_and(|p| rect.contains(p));
    put(frame, x, 0, &label, if hover { bright_bold() } else { dim() });
    if many {
        // No dwell tooltip here: it matured right where the dropdown
        // opens and sat on top of the first rows.
        gui.ui.click(rect, Act::SrvMenu);
    }
}

/// The dropdown under the header label: every saved server, then the add
/// row. Drawn (and registered) after everything else so its rows win the
/// pointer — with a whole-screen guard underneath so a stray click only
/// closes it.
pub(crate) fn draw_dropdown(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if !gui.servers.drop_open {
        return;
    }
    gui.ui.click(area, Act::SrvCloseDrop);

    let entries: Vec<(String, bool, bool)> = gui
        .config
        .servers
        .iter()
        .map(|s| {
            let current =
                gui.app.connected && config::same_server(&gui.app.session.server_id, &s.url);
            let default = gui
                .config
                .default_server
                .as_deref()
                .is_some_and(|d| config::same_server(d, &s.url));
            (crate::quickconnect::display_server(&s.url), current, default)
        })
        .collect();
    let add_label = format!("+ {}", t!("gui.srv.add"));
    let widest = entries
        .iter()
        .map(|(label, _, _)| label.chars().count() + 4)
        .chain([add_label.chars().count() + 2])
        .max()
        .unwrap_or(20);
    let width = (widest as u16 + 4).clamp(24, 44).min(area.width.saturating_sub(2));
    let height = entries.len() as u16 + 3;
    let x = area.width.saturating_sub(width + 1);
    let rect = Rect { x, y: 1, width, height: height.min(area.height.saturating_sub(8)) };

    frame.render_widget(ratatui::widgets::Clear, rect);
    if let Some(ground) = th().ground.filter(|_| crate::kit::theme::ground_owned()) {
        frame.render_widget(
            ratatui::widgets::Block::default().style(Style::default().bg(ground).fg(th().text)),
            rect,
        );
    }
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(dim());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let marker = if legacy_conhost() { ">" } else { "▸" };
    let star = if legacy_conhost() { "*" } else { "★" };
    for (i, (label, current, default)) in entries.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| row.contains(p));
        let style = match (current, hover) {
            (true, _) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let lead = if *current { marker } else { " " };
        let shown = super::bar::clip(label, inner.width as usize - 4);
        put(frame, inner.x, y, &format!("{lead} {shown}"), style);
        if *default {
            put(frame, inner.right().saturating_sub(1), y, star, Style::default().fg(th().gold));
        }
        gui.ui.click(row, Act::SrvDrop(i));
    }
    let add_y = inner.y + entries.len() as u16;
    if add_y < inner.bottom() {
        let row = Rect { x: inner.x, y: add_y, width: inner.width, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| row.contains(p));
        put(frame, inner.x + 2, add_y, &add_label, if hover { bright_bold() } else { dim() });
        gui.ui.click(row, Act::SrvAdd);
    }
}

/// The Manage Servers room, in the Settings content column.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    put(frame, content.x, content.y, &t!("gui.srv.title"), dim());

    let servers = gui.config.servers.clone();
    let default = gui.config.default_server.clone();
    let star = if legacy_conhost() { "*" } else { "★" };
    let marker = if legacy_conhost() { ">" } else { "▸" };

    let name_w = content.width.saturating_sub(30) as usize;
    for (i, entry) in servers.iter().enumerate() {
        let y = content.y + 2 + i as u16;
        if y + 3 >= content.bottom() {
            break;
        }
        let row = Rect { x: content.x, y, width: content.width, height: 1 };
        let selected = gui.servers.cursor == i;
        let hover = gui.ui.pointer.is_some_and(|p| row.contains(p));
        if selected {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let current =
            gui.app.connected && config::same_server(&gui.app.session.server_id, &entry.url);
        let style = match (selected, current, hover) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
            (false, false, true) => bright_bold(),
            (false, false, false) => Style::default(),
        };
        if current {
            let mstyle = if selected { sel() } else { Style::default().fg(th().accent) };
            put(frame, content.x, y, marker, mstyle.add_modifier(Modifier::BOLD));
        }
        let name = crate::quickconnect::display_server(&entry.url);
        put(frame, content.x + 2, y, &super::bar::clip(&name, name_w), style);

        let meta = if selected { sel() } else { dim() };
        let user = entry.username.as_deref().unwrap_or("");
        put(frame, content.right().saturating_sub(26), y, &super::bar::clip(user, 12), meta);
        let version = gui.servers.version_label(&entry.url);
        put(frame, content.right().saturating_sub(12), y, &super::bar::clip(&version, 9), meta);
        if default.as_deref().is_some_and(|d| config::same_server(d, &entry.url)) {
            let dstyle = if selected { sel() } else { Style::default().fg(th().gold) };
            put(frame, content.right().saturating_sub(2), y, star, dstyle);
        }
        gui.ui.click(row, Act::SrvRow(i));
    }

    // The add row closes the list, cursor-reachable like any other.
    let add_y = content.y + 2 + servers.len() as u16;
    let add_label = format!("+ {}", t!("gui.srv.add"));
    if add_y + 3 < content.bottom() {
        let row = Rect { x: content.x, y: add_y, width: content.width, height: 1 };
        let selected = gui.servers.cursor == servers.len();
        let hover = gui.ui.pointer.is_some_and(|p| row.contains(p));
        if selected {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let style = if selected {
            sel().add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else {
            dim()
        };
        put(frame, content.x + 2, add_y, &add_label, style);
        gui.ui.click(row, Act::SrvAdd);
    }

    // The cursored row's actions, on their own line under the list.
    let Some(entry) = servers.get(gui.servers.cursor) else { return };
    let index = gui.servers.cursor;
    let actions_y = (add_y + 2).min(content.bottom().saturating_sub(1));
    let tunnel = crate::quickconnect::is_tunnel_id(&entry.url);
    let current = gui.app.connected && config::same_server(&gui.app.session.server_id, &entry.url);
    let is_default = default.as_deref().is_some_and(|d| config::same_server(d, &entry.url));

    let mut x = content.x + 2;
    let word = |frame: &mut Frame,
                    gui: &mut Gui,
                    x: &mut u16,
                    label: String,
                    style: Style,
                    hover_style: Style,
                    act: Option<Act>| {
        let width = label.chars().count() as u16;
        let rect = Rect { x: *x, y: actions_y, width, height: 1 };
        let hovered = act.is_some() && gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(frame, *x, actions_y, &label, if hovered { hover_style } else { style });
        if let Some(act) = act {
            gui.ui.click(rect, act);
        }
        *x += width + 3;
    };

    if current {
        word(
            frame,
            gui,
            &mut x,
            t!("gui.srv.connected").to_string(),
            Style::default().fg(th().accent),
            Style::default().fg(th().accent),
            None,
        );
    } else {
        word(
            frame,
            gui,
            &mut x,
            t!("gui.srv.act_switch").to_string(),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
            bright_bold(),
            Some(Act::SrvSwitch(index)),
        );
    }
    if !tunnel {
        word(
            frame,
            gui,
            &mut x,
            t!("gui.srv.act_edit").to_string(),
            dim(),
            bright_bold(),
            Some(Act::SrvEdit(index)),
        );
    }
    if !is_default {
        word(
            frame,
            gui,
            &mut x,
            t!("gui.srv.act_default").to_string(),
            dim(),
            bright_bold(),
            Some(Act::SrvDefault(index)),
        );
    }
    if tunnel {
        word(
            frame,
            gui,
            &mut x,
            t!("gui.srv.act_qr").to_string(),
            dim(),
            bright_bold(),
            Some(Act::SrvQr(index)),
        );
    }
    word(
        frame,
        gui,
        &mut x,
        t!("gui.srv.act_remove").to_string(),
        Style::default().fg(th().danger),
        Style::default().fg(th().danger).add_modifier(Modifier::BOLD),
        Some(Act::SrvRemove(index)),
    );
}

/// The add/edit form, the remove confirmation and the pairing QR — one of
/// them at a time, over everything.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if gui.servers.form.is_some() {
        draw_form(frame, gui, area);
    } else if let Some(index) = gui.servers.confirm {
        draw_confirm(frame, gui, area, index);
    } else if gui.servers.qr.is_some() {
        draw_qr(frame, gui, area);
    }
}

/// What one frame of the form needs to know — cloned out so drawing can
/// register clicks on `gui` freely.
struct FormView {
    server: String,
    username: String,
    password: String,
    self_signed: bool,
    public: bool,
    focus: usize,
    editing: bool,
    submitting: bool,
    error: Option<String>,
    session_login: bool,
}

fn draw_form(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    match gui.servers.form.as_ref().map(|f| f.stage) {
        Some(FormStage::Choosing) => draw_choose(frame, gui, area),
        Some(FormStage::QuickConnect) => draw_quick_connect(frame, gui, area),
        Some(FormStage::Direct) => draw_direct(frame, gui, area),
        None => {}
    }
}

/// The chooser: the two ways in, the TUI connect screen's own menu worn
/// modal. The focused way is the primary button; ↑↓ or Tab flips.
fn draw_choose(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::Guard);
    let inner = modal_frame(frame, area, 60, 12, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.srv.form_add"), bright_bold());
    modal_close(frame, &mut gui.ui, inner, Act::FormCancel, t!("gui.srv.close_tip").to_string());

    let focus = gui.servers.form.as_ref().map_or(0, |f| f.focus.min(1));
    for (i, (label, desc)) in [
        (t!("gui.srv.method_direct").to_string(), t!("gui.srv.method_direct_desc").to_string()),
        (t!("gui.srv.method_qc").to_string(), t!("gui.srv.method_qc_desc").to_string()),
    ]
    .into_iter()
    .enumerate()
    {
        let at = Rect { x: inner.x + 1, y: inner.y + 2 + 4 * i as u16, width: 26, height: 3 };
        let rect = if focus == i {
            tall_button(frame, &mut gui.ui, at, &label, true, Act::FormMethod(i))
        } else {
            tall_secondary(frame, &mut gui.ui, at, &label, Act::FormMethod(i))
        };
        put(
            frame,
            rect.right() + 2,
            at.y + 1,
            &super::bar::clip(&desc, inner.right().saturating_sub(rect.right() + 3) as usize),
            dim(),
        );
    }
}

/// Quick Connect: servers found on this network — mDNS rows, reachable
/// directly, so a click carries them to the standard page — and the
/// pairing code paste for everywhere else.
fn draw_quick_connect(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::Guard);
    let inner = modal_frame(frame, area, 60, 20, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.srv.form_qc"), bright_bold());
    modal_close(frame, &mut gui.ui, inner, Act::FormCancel, t!("gui.srv.close_tip").to_string());

    let (found, row, searching, code, submitting, error) = {
        let Some(form) = gui.servers.form.as_ref() else { return };
        (
            form.found.clone(),
            form.row,
            form.searching,
            form.code.clone(),
            form.submitting,
            form.error.clone(),
        )
    };

    put(frame, inner.x + 1, inner.y + 2, &t!("gui.srv.on_network"), dim());
    let list_y = inner.y + 3;
    if searching {
        put(frame, inner.x + 2, list_y, &t!("gui.srv.searching"), accent());
    } else if found.is_empty() {
        put(frame, inner.x + 2, list_y, &t!("gui.srv.none_found"), dim());
    }
    for (i, server) in found.iter().take(4).enumerate() {
        let y = list_y + i as u16;
        let rect = Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
        let selected = row == i;
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        if selected {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (selected, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let label = format!("{} — {}", server.name, server.base_url);
        put(frame, rect.x + 1, y, &super::bar::clip(&label, rect.width as usize - 12), style);
        if let Some(version) = &server.version {
            let shown = super::bar::clip(&format!("v{version}"), 9);
            let vstyle = if selected { sel() } else { dim() };
            put(frame, rect.right().saturating_sub(1 + shown.chars().count() as u16), y, &shown, vstyle);
        }
        gui.ui.click(rect, Act::FormPick(i));
    }

    // The paste card: focused whenever the cursor sits on the paste row,
    // which is where typing anywhere puts it.
    let card_y = list_y + 5;
    let on_paste = row >= found.len();
    let rect = Rect { x: inner.x + 1, y: card_y, width: inner.width.saturating_sub(2), height: 3 };
    let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
    let border = if on_paste {
        Style::default().fg(th().accent)
    } else if hover {
        Style::default().fg(th().bright)
    } else {
        dim()
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(border)
        .title(ratatui::text::Span::styled(t!("gui.srv.form_code").to_string(), dim()));
    let body = block.inner(rect);
    frame.render_widget(block, rect);
    if on_paste {
        let cursor = code.chars().count();
        put(
            frame,
            body.x + 1,
            body.y,
            &input_display(&code, cursor, body.width.saturating_sub(2)),
            Style::default(),
        );
    } else {
        put(frame, body.x + 1, body.y, &super::bar::clip(&code, body.width as usize - 2), dim());
    }
    gui.ui.click(rect, Act::FormFocus(found.len()));

    let status_y = card_y + 4;
    if submitting {
        put(
            frame,
            inner.x + 2,
            status_y,
            &super::bar::clip(&t!("gui.srv.dialling"), inner.width as usize - 4),
            accent(),
        );
    } else if let Some(error) = &error {
        put(
            frame,
            inner.x + 2,
            status_y,
            &super::bar::clip(error, inner.width as usize - 4),
            Style::default().fg(th().gold),
        );
    }

    let buttons_y = inner.bottom().saturating_sub(3);
    let at = Rect { x: inner.x + 1, y: buttons_y, width: inner.width - 1, height: 3 };
    let connect =
        tall_button(frame, &mut gui.ui, at, &t!("gui.srv.connect"), !submitting, Act::FormSubmit);
    let beside = Rect {
        x: connect.right() + 2,
        y: buttons_y,
        width: inner.right().saturating_sub(connect.right() + 2),
        height: 3,
    };
    tall_secondary(frame, &mut gui.ui, beside, &t!("gui.srv.back"), Act::FormBack);
}

fn draw_direct(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::Guard);
    let inner = modal_frame(frame, area, 60, 20, th().accent);
    let view = {
        let Some(form) = gui.servers.form.as_ref() else { return };
        FormView {
            server: form.server.clone(),
            username: form.username.clone(),
            password: form.password.clone(),
            self_signed: form.self_signed,
            public: form.public,
            focus: form.focus,
            editing: form.editing.is_some(),
            submitting: form.submitting,
            error: form.error.clone(),
            session_login: form.session_login,
        }
    };

    let title = if view.session_login {
        t!("gui.srv.form_signin").to_string()
    } else if view.editing {
        t!("gui.srv.form_edit").to_string()
    } else {
        t!("gui.srv.form_add").to_string()
    };
    put(frame, inner.x + 1, inner.y, &title, bright_bold());
    modal_close(frame, &mut gui.ui, inner, Act::FormCancel, t!("gui.srv.close_tip").to_string());

    let (check_on, check_off) = if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") };
    let field_w = inner.width.saturating_sub(2);

    let card = |frame: &mut Frame,
                    gui: &mut Gui,
                    y: u16,
                    label: String,
                    value: &str,
                    focused: bool,
                    enabled: bool,
                    mask: bool,
                    act: Act| {
        let rect = Rect { x: inner.x + 1, y, width: field_w, height: 3 };
        let hover = enabled && gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let border = if focused && enabled {
            Style::default().fg(th().accent)
        } else if hover {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(border)
            .title(ratatui::text::Span::styled(label, dim()));
        let body = block.inner(rect);
        frame.render_widget(block, rect);
        let shown: String =
            if mask { value.chars().map(|_| '•').collect() } else { value.to_string() };
        let text_style = if enabled { Style::default() } else { dim() };
        if focused && enabled {
            let cursor = shown.chars().count();
            put(
                frame,
                body.x + 1,
                body.y,
                &input_display(&shown, cursor, body.width.saturating_sub(2)),
                text_style,
            );
        } else {
            put(
                frame,
                body.x + 1,
                body.y,
                &super::bar::clip(&shown, body.width as usize - 2),
                text_style,
            );
        }
        if enabled {
            gui.ui.click(rect, act);
        }
    };

    card(
        frame,
        gui,
        inner.y + 1,
        t!("gui.srv.form_server").to_string(),
        &view.server,
        view.focus == 0,
        !view.session_login,
        false,
        Act::FormFocus(0),
    );
    card(
        frame,
        gui,
        inner.y + 4,
        t!("gui.srv.form_username").to_string(),
        &view.username,
        view.focus == 1,
        !view.public,
        false,
        Act::FormFocus(1),
    );
    card(
        frame,
        gui,
        inner.y + 7,
        t!("gui.srv.form_password").to_string(),
        &view.password,
        view.focus == 2,
        !view.public,
        true,
        Act::FormFocus(2),
    );

    for (slot, (on, label)) in [
        (view.self_signed, t!("gui.srv.self_signed").to_string()),
        (view.public, t!("gui.srv.public").to_string()),
    ]
    .into_iter()
    .enumerate()
    {
        let y = inner.y + 10 + slot as u16;
        let text = format!("{} {}", if on { check_on } else { check_off }, label);
        let rect = Rect { x: inner.x + 2, y, width: text.chars().count() as u16, height: 1 };
        let focused = view.focus == slot + 3;
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if focused {
            sel().add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else if on {
            Style::default().fg(th().ok)
        } else {
            dim()
        };
        put(frame, rect.x, y, &text, style);
        gui.ui.click(rect, Act::FormToggle(slot + 3));
    }

    let status_y = inner.y + 13;
    if view.submitting {
        put(
            frame,
            inner.x + 2,
            status_y,
            &super::bar::clip(
                &t!("gui.srv.reaching", server = view.server.clone()),
                field_w as usize - 2,
            ),
            accent(),
        );
    } else if let Some(error) = &view.error {
        put(
            frame,
            inner.x + 2,
            status_y,
            &super::bar::clip(error, field_w as usize - 2),
            Style::default().fg(th().gold),
        );
    }

    let submit_label = if view.session_login || !view.editing {
        t!("gui.srv.connect").to_string()
    } else {
        t!("gui.srv.save").to_string()
    };
    let buttons_y = inner.y + 15;
    let at = Rect { x: inner.x + 1, y: buttons_y, width: inner.width - 1, height: 3 };
    let submit_rect =
        tall_button(frame, &mut gui.ui, at, &submit_label, !view.submitting, Act::FormSubmit);
    let beside = Rect {
        x: submit_rect.right() + 2,
        y: buttons_y,
        width: inner.right().saturating_sub(submit_rect.right() + 2),
        height: 3,
    };
    // An add came through the chooser and can go back to it; an edit or a
    // sign-in arrived here directly, so its neutral exit is cancel.
    let (back_label, back_act) = if view.session_login || view.editing {
        (t!("gui.srv.cancel").to_string(), Act::FormCancel)
    } else {
        (t!("gui.srv.back").to_string(), Act::FormBack)
    };
    tall_secondary(frame, &mut gui.ui, beside, &back_label, back_act);
}

fn draw_confirm(frame: &mut Frame, gui: &mut Gui, area: Rect, index: usize) {
    gui.ui.click(area, Act::Guard);
    let Some(entry) = gui.config.servers.get(index) else {
        gui.servers.confirm = None;
        return;
    };
    let url = entry.url.clone();
    let tunnel = crate::quickconnect::is_tunnel_id(&url);
    let inner = modal_frame(frame, area, 56, if tunnel { 11 } else { 9 }, th().danger);
    put(frame, inner.x + 1, inner.y, &t!("gui.srv.remove_title"), bright_bold());
    modal_close(frame, &mut gui.ui, inner, Act::SrvConfirm(false), t!("gui.srv.close_tip").to_string());

    let shown = crate::quickconnect::display_server(&url);
    put(
        frame,
        inner.x + 2,
        inner.y + 2,
        &super::bar::clip(
            &t!("gui.srv.remove_body", server = shown),
            inner.width as usize - 4,
        ),
        Style::default(),
    );
    if tunnel {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(t!("gui.srv.remove_tunnel_note").to_string())
                .style(Style::default().fg(th().gold))
                .wrap(ratatui::widgets::Wrap { trim: true }),
            Rect { x: inner.x + 2, y: inner.y + 3, width: inner.width - 4, height: 2 },
        );
    }

    let buttons_y = inner.bottom().saturating_sub(3);
    let at = Rect { x: inner.x + 1, y: buttons_y, width: inner.width - 1, height: 3 };
    let yes = tall_button(frame, &mut gui.ui, at, &t!("gui.srv.remove_yes"), true, Act::SrvConfirm(true));
    let beside = Rect {
        x: yes.right() + 2,
        y: buttons_y,
        width: inner.right().saturating_sub(yes.right() + 2),
        height: 3,
    };
    tall_secondary(frame, &mut gui.ui, beside, &t!("gui.srv.remove_keep"), Act::SrvConfirm(false));
}

fn draw_qr(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::QrClose);
    let (label, code, line_count) = {
        let Some(qr) = gui.servers.qr.as_ref() else { return };
        (qr.label.clone(), qr.code.clone(), qr.lines.len() as u16)
    };
    let height = area.height.saturating_sub(2).min(line_count.max(16) + 6);
    let width = (line_count.max(24) + 8).clamp(40, area.width.saturating_sub(4));
    let inner = modal_frame(frame, area, width, height, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.srv.qr_title"), bright_bold());
    modal_close(frame, &mut gui.ui, inner, Act::QrClose, t!("gui.srv.close_tip").to_string());
    put(
        frame,
        inner.x + 1,
        inner.y + 1,
        &super::bar::clip(&label, inner.width as usize - 2),
        dim(),
    );

    let band = Rect {
        x: inner.x + 1,
        y: inner.y + 3,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(6),
    };
    // Pixels where the terminal draws them (the probe already answered);
    // half-blocks where it doesn't — and when even those don't fit, the
    // code itself is on screen below to copy by hand.
    let drew = {
        let art = gui.servers.qr.as_ref().and_then(|qr| qr.art.as_ref());
        match art {
            Some(art) => gui.app.graphics.draw(frame, band, art),
            None => false,
        }
    };
    if !drew {
        let Some(qr) = gui.servers.qr.as_ref() else { return };
        if line_count <= band.height && line_count > 0 {
            let x = band.x + (band.width.saturating_sub(line_count)) / 2;
            for (i, line) in qr.lines.iter().enumerate() {
                put(frame, x, band.y + i as u16, line, Style::default());
            }
        } else {
            put(
                frame,
                band.x + 1,
                band.y + 1,
                &super::bar::clip(&t!("gui.srv.qr_enlarge"), band.width as usize - 2),
                dim(),
            );
        }
    }

    put(
        frame,
        inner.x + 1,
        inner.bottom().saturating_sub(2),
        &super::bar::clip(&t!("gui.srv.qr_note"), inner.width as usize - 2),
        dim(),
    );
    put(
        frame,
        inner.x + 1,
        inner.bottom().saturating_sub(1),
        &super::bar::clip(&code, inner.width as usize - 2),
        dim(),
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerEntry, testing::Scratch};
    use crate::tui::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(url: &str, username: Option<&str>) -> ServerEntry {
        ServerEntry {
            url: url.to_string(),
            username: username.map(str::to_string),
            ..ServerEntry::default()
        }
    }

    /// A Gui holding two saved servers, connected to the first.
    fn two_server_gui() -> Gui {
        let mut config = Config::default();
        config.servers = vec![
            entry("http://attic.local:3000", Some("paul")),
            entry("http://office.local:3000", None),
        ];
        let mut gui = Gui::new(
            config,
            false,
            App::new(Some("http://attic.local:3000".into()), None, None),
        );
        gui.app.connected = true;
        gui
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn the_header_grows_a_dropdown_once_a_second_server_is_saved() {
        // The chevron obeys the same glyph gate as everything else, so the
        // assertion reads it from the same switch.
        let chevron = if legacy_conhost() { " v" } else { " ▾" };
        let mut gui = two_server_gui();
        gui.queue_open = false;
        let rows = draw(&mut gui);
        assert!(rows[0].contains("[+]"), "the add button is always there");
        assert!(
            rows[0].contains(&format!("attic.local:3000{chevron}")),
            "two servers make the label a menu: {}",
            rows[0]
        );

        gui.act(Act::SrvMenu);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("office.local"), "the other server is a row");
        assert!(all.contains(&format!("+ {}", t!("gui.srv.add"))), "so is adding one");

        // One server only: no chevron, no menu.
        let mut config = Config::default();
        config.servers = vec![entry("http://attic.local:3000", None)];
        let mut lone =
            Gui::new(config, false, App::new(Some("http://attic.local:3000".into()), None, None));
        lone.app.connected = true;
        let rows = draw(&mut lone);
        assert!(!rows[0].contains(&format!("attic.local:3000{chevron}")));
    }

    #[test]
    fn picking_a_dropdown_row_switches_through_the_apps_funnel() {
        let scratch = Scratch::new("gui-switch");
        let _ = &scratch; // credentials load finds an empty dir, not the user's
        let mut gui = two_server_gui();
        // On disk too: the switch saves the outgoing session and reloads.
        config::save(&gui.config).unwrap();
        gui.app.queue.items.push(crate::api::types::Track {
            filepath: "music/a.mp3".into(),
            metadata: Default::default(),
        });
        gui.app.now_playing = Some(crate::api::types::Track {
            filepath: "music/a.mp3".into(),
            metadata: Default::default(),
        });

        switch_to(&mut gui, 1);
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::Connect { server, .. }) if server == "http://office.local:3000"
            )),
            "the switch rode out as the App's own connect: {:?}",
            gui.pending
        );
        assert!(gui.app.queue.items.is_empty(), "the old server's queue cannot come along");
        assert!(gui.app.now_playing.is_some(), "what was streaming keeps playing");
        assert_eq!(gui.app.session.server_id, "http://office.local:3000");
        assert_eq!(gui.servers.switching.as_deref(), Some("http://office.local:3000"));

        // Connected clears the in-flight marker.
        observe(
            &mut gui,
            &Event::Connected {
                server: "http://office.local:3000".into(),
                id: "http://office.local:3000".into(),
                username: None,
                token: None,
                ping: Box::default(),
            },
        );
        assert!(gui.servers.switching.is_none());
    }

    #[test]
    fn switching_to_the_current_server_is_a_no_op() {
        let mut gui = two_server_gui();
        switch_to(&mut gui, 0);
        assert!(gui.pending.is_empty());
        assert!(gui.servers.switching.is_none());
    }

    #[test]
    fn the_add_flow_starts_at_the_chooser() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        assert_eq!(gui.servers.form.as_ref().unwrap().stage, FormStage::Choosing);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.srv.method_direct").to_string()), "standard is offered");
        assert!(all.contains(&t!("gui.srv.method_qc").to_string()), "and Quick Connect");

        // Enter on the focused method opens its page; Esc from the
        // chooser closes the whole flow.
        submit_form(&mut gui);
        assert_eq!(gui.servers.form.as_ref().unwrap().stage, FormStage::Direct);
        form_back(&mut gui);
        assert_eq!(gui.servers.form.as_ref().unwrap().stage, FormStage::Choosing);
        form_back(&mut gui);
        assert!(gui.servers.form.is_none());
    }

    #[test]
    fn the_quick_connect_page_discovers_and_takes_a_code() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        gui.act(Act::FormMethod(1));
        let form = gui.servers.form.as_ref().unwrap();
        assert_eq!(form.stage, FormStage::QuickConnect);
        assert!(form.searching, "the browse starts with the page");
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Discover)),
            "the mDNS browse rode out: {:?}",
            gui.pending
        );

        // The browse answers: rows land on the page, cursor on the first.
        observe(
            &mut gui,
            &Event::ServersDiscovered(vec![crate::discovery::DiscoveredServer {
                name: "attic".into(),
                base_url: "http://192.168.1.71:3999".into(),
                version: Some("5.13.2".into()),
                quick_connect: true,
            }]),
        );
        let form = gui.servers.form.as_ref().unwrap();
        assert!(!form.searching);
        assert_eq!(form.row, 0, "the cursor lands on the first server");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("attic"), "discovered servers are rows");
        assert!(all.contains("v5.13.2"), "with their advertised version");

        // Picking a row is not a dial — it carries the address to the
        // standard page, where credentials live.
        gui.act(Act::FormPick(0));
        let form = gui.servers.form.as_ref().unwrap();
        assert_eq!(form.stage, FormStage::Direct);
        assert_eq!(form.server, "http://192.168.1.71:3999");
    }

    #[test]
    fn a_pasted_code_dials_through_the_funnel_and_seats_on_answer() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        gui.act(Act::FormMethod(1));
        // Typing anywhere means "I have a code" — the TUI's rule.
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        for c in "mstr1:abc".chars() {
            handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(gui.servers.form.as_ref().unwrap().code, "mstr1:abc", "keys landed in the code");
        submit_form(&mut gui);
        assert_eq!(gui.servers.pending_code.as_deref(), Some("mstr1:abc"));
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(ApiCmd::QuickConnect { code, .. }) if code == "mstr1:abc"
            )),
            "the dial rode the funnel: {:?}",
            gui.pending
        );
        assert!(gui.app.session.tunnel_code.is_none(), "nothing seated until it answers");

        // The tunnel answers: the code is seated for the save, the old
        // server's queue is shed, the form closes.
        gui.app.queue.items.push(crate::api::types::Track {
            filepath: "music/a.mp3".into(),
            metadata: Default::default(),
        });
        observe(
            &mut gui,
            &Event::Connected {
                server: "http://127.0.0.1:51234".into(),
                id: "mstream+iroh://endpointabc".into(),
                username: None,
                token: None,
                ping: Box::default(),
            },
        );
        assert_eq!(gui.app.session.tunnel_code.as_deref(), Some("mstr1:abc"));
        assert!(gui.app.queue.items.is_empty(), "the old server's queue is shed");
        assert!(gui.servers.pending_code.is_none());
        assert!(gui.servers.form.is_none());
    }

    #[test]
    fn a_failed_dial_costs_an_error_line_never_the_session() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        gui.act(Act::FormMethod(1));
        submit_form(&mut gui);
        let form = gui.servers.form.as_ref().unwrap();
        assert!(form.error.is_some(), "an empty code is refused locally");

        gui.servers.form.as_mut().unwrap().code = "mstr1:bad".into();
        submit_form(&mut gui);
        observe(&mut gui, &Event::Error("could not reach the tunnel".into()));
        let form = gui.servers.form.as_ref().unwrap();
        assert!(!form.submitting);
        assert_eq!(form.error.as_deref(), Some("could not reach the tunnel"));
        assert!(gui.servers.pending_code.is_none());
        assert!(gui.app.session.tunnel_code.is_none(), "the session never felt it");
        assert_eq!(gui.app.session.server_id, "http://attic.local:3000", "still home");
    }

    #[test]
    fn the_form_validates_before_any_network() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        gui.act(Act::FormMethod(0));
        submit_form(&mut gui);
        let form = gui.servers.form.as_ref().unwrap();
        assert!(form.error.is_some(), "an empty address is refused locally");

        let form = gui.servers.form.as_mut().unwrap();
        form.server = "http://host:3000".into();
        form.username = "alice".into();
        submit_form(&mut gui);
        let form = gui.servers.form.as_ref().unwrap();
        assert!(
            form.error.as_deref().unwrap().contains(&t!("gui.srv.need_password").to_string()),
            "a username without a password is caught here"
        );
    }

    #[test]
    fn public_mode_stands_the_credential_fields_down() {
        let mut gui = two_server_gui();
        open_add(&mut gui);
        gui.act(Act::FormMethod(0));
        gui.act(Act::FormToggle(4));
        let form = gui.servers.form.as_ref().unwrap();
        assert!(form.public);
        assert!(!form.focusable(1) && !form.focusable(2), "no fields to type a login into");

        // The focus cycle walks server → self-signed → public and back.
        let form = gui.servers.form.as_mut().unwrap();
        form.focus = 0;
        form.step_focus(true);
        assert_eq!(form.focus, 3, "username and password are skipped");

        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.srv.public").to_string()));
    }

    #[test]
    fn an_edit_without_a_password_saves_offline_and_moves_the_token() {
        let scratch = Scratch::new("gui-edit");
        let _ = &scratch;
        let mut config = Config::default();
        config.servers = vec![entry("http://attic.local:3000", Some("paul"))];
        config::set_default_server(&mut config, Some("http://attic.local:3000"));
        config::save(&config).unwrap();
        let mut credentials = config::Credentials::default();
        config::store_token(&mut credentials, "http://attic.local:3000", Some("jwt".into()));
        config::save_credentials(&credentials).unwrap();

        let mut gui = Gui::new(config, true, App::new(None, None, None));
        open_edit(&mut gui, 0);
        let form = gui.servers.form.as_mut().unwrap();
        assert_eq!(form.username, "paul", "the entry prefills");
        form.server = "http://attic.lan:3000".into();
        form.self_signed = true;
        submit_form(&mut gui);

        assert!(gui.servers.form.is_none(), "an offline save closes the form");
        let reloaded = config::load().unwrap();
        assert_eq!(reloaded.servers[0].url, "http://attic.lan:3000");
        assert!(reloaded.servers[0].self_signed);
        assert_eq!(
            reloaded.default_server.as_deref(),
            Some("http://attic.lan:3000"),
            "the default follows the rename"
        );
        let credentials = config::load_credentials().unwrap();
        assert_eq!(config::token_for(&credentials, "http://attic.lan:3000"), Some("jwt".into()));
        assert_eq!(config::token_for(&credentials, "http://attic.local:3000"), None);
    }

    #[test]
    fn the_room_lists_servers_with_their_marks_and_removes_with_secrets() {
        let scratch = Scratch::new("gui-room");
        let _ = &scratch;
        let mut config = Config::default();
        config.servers = vec![
            entry("http://attic.local:3000", Some("paul")),
            entry("mstream+iroh://endpointabcdef123456", None),
        ];
        config::set_default_server(&mut config, Some("http://attic.local:3000"));
        config::save(&config).unwrap();
        let mut credentials = config::Credentials::default();
        config::store_pairing(
            &mut credentials,
            "mstream+iroh://endpointabcdef123456",
            Some("mstr1:thecode".into()),
        );
        config::save_credentials(&credentials).unwrap();

        let mut gui = Gui::new(config, true, App::new(None, None, None));
        gui.active = super::super::SETTINGS_NAV;
        gui.queue_open = false;
        open_room(&mut gui);
        let star = if legacy_conhost() { "*" } else { "★" };
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("attic.local"), "direct servers by address");
        assert!(all.contains("quick connect"), "tunnels by their friendly name");
        assert!(all.contains(star), "the default wears its star");
        assert!(all.contains("paul"), "who signs in is a column");

        // Remove the tunnel: confirm, and the pairing code goes with it.
        gui.servers.cursor = 1;
        gui.act(Act::SrvRemove(1));
        assert_eq!(gui.servers.confirm, Some(1));
        let all = draw(&mut gui).join("\n");
        // Wrapped across the modal's width, so ask for a stable fragment.
        assert!(all.contains("pairing code"), "removal says what a tunnel loses");
        gui.act(Act::SrvConfirm(true));
        assert_eq!(gui.config.servers.len(), 1);
        let credentials = config::load_credentials().unwrap();
        assert_eq!(
            config::pairing_for(&credentials, "mstream+iroh://endpointabcdef123456"),
            None,
            "removing a server is the one flow that drops its code"
        );
    }

    #[test]
    fn the_qr_modal_carries_the_pairing_code_both_ways() {
        let scratch = Scratch::new("gui-qr");
        let _ = &scratch;
        let mut config = Config::default();
        config.servers = vec![entry("mstream+iroh://endpointabcdef123456", None)];
        config::save(&config).unwrap();
        let mut credentials = config::Credentials::default();
        config::store_pairing(
            &mut credentials,
            "mstream+iroh://endpointabcdef123456",
            Some("mstr1:thecode".into()),
        );
        config::save_credentials(&credentials).unwrap();

        let mut gui = Gui::new(config, true, App::new(None, None, None));
        open_qr(&mut gui, 0);
        let qr = gui.servers.qr.as_ref().expect("the modal opened");
        assert_eq!(qr.code, "mstr1:thecode");
        assert!(!qr.lines.is_empty(), "the half-block rendering rides along");
        assert!(qr.art.is_some(), "and the pixel one");

        gui.active = super::super::SETTINGS_NAV;
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("mstr1:thecode"), "the code itself is on screen to copy");

        // No stored code: the modal refuses with words, not a blank square.
        let mut credentials = config::load_credentials().unwrap();
        config::store_pairing(&mut credentials, "mstream+iroh://endpointabcdef123456", None);
        config::save_credentials(&credentials).unwrap();
        gui.servers.qr = None;
        open_qr(&mut gui, 0);
        assert!(gui.servers.qr.is_none());
        assert!(gui.note.as_ref().is_some_and(|(_, is_err)| *is_err));
    }

    #[test]
    fn a_needs_login_during_a_switch_opens_the_sign_in_form() {
        let mut gui = two_server_gui();
        gui.app.connected = false;
        gui.servers.switching = Some("http://attic.local:3000".into());
        observe(&mut gui, &Event::NeedsLogin { server: "http://attic.local:3000".into() });
        let form = gui.servers.form.as_ref().expect("the form opened");
        assert!(form.session_login, "this sign-in rides the funnel, not a one-shot client");
        assert_eq!(form.username, "paul", "the saved entry prefills");
        assert_eq!(form.focus, 1, "straight to the username");
    }

    #[test]
    fn version_replies_land_in_the_room_labels() {
        let mut gui = two_server_gui();
        gui.servers.versions.insert("http://attic.local:3000".into(), Probe::Pending);
        gui.servers
            .tx
            .clone()
            .send(Reply::Version {
                key: "http://attic.local:3000".into(),
                version: Some("5.13.2".into()),
            })
            .unwrap();
        poll(&mut gui);
        assert_eq!(gui.servers.version_label("http://attic.local:3000"), "v5.13.2");
        assert_eq!(gui.servers.version_label("http://office.local:3000"), "—");
    }

    #[test]
    fn the_settings_row_opens_the_room_and_esc_walks_back() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut gui = two_server_gui();
        gui.active = super::super::SETTINGS_NAV;
        gui.act(Act::Row(super::super::ROW_MANAGE));
        assert!(gui.servers.room);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.srv.title").to_string()));

        let handled =
            handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(handled, Some(false));
        assert!(!gui.servers.room, "Esc closes the room");
    }
}
