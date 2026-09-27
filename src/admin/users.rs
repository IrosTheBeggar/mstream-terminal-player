//! The Users room: `mstream-player admin users` — who has an account on
//! the server, what each one may do, which libraries they see. The
//! webapp's Users page as one table with checkbox cells: a flag flips at
//! once (the whole set echoed, as the route demands), the library grant
//! and the password each have a modal, adding is a form, removing a gate.
//! A server with no users runs without logins; the first user ends that,
//! and this room signs itself in as that user so the panel stays open.

use std::collections::BTreeMap;
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
    Outcome, Screen, draw_bottom, draw_header, frame_ground, gate_message, host_of, login,
    printable,
};
use crate::api::types::{AdminUser, LoginResponse};
use crate::api::{ApiError, Client, NewUser, UserAccess};
use crate::kit::theme::th;
use crate::kit::{self, Surface, bold, dim};
use crate::setup::g;

/// A quiet reload: users rarely change, and the webapp never polls here.
const POLL: Duration = Duration::from_secs(30);
const MIN_W: u16 = 80;
const MIN_H: u16 = 22;
/// The USER column, the wizard's NAME width.
const USER_W: u16 = 16;
/// The ` [X]` column, outside the selection area.
const REMOVE_W: u16 = 4;
const NAME_MAX: usize = 64;

/// The five flags, in the table's order after LIBRARIES — and their digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flag {
    Admin,
    Mkdir,
    Upload,
    Audio,
    Modify,
}

impl Flag {
    fn from_digit(c: char) -> Option<Flag> {
        match c {
            '1' => Some(Flag::Admin),
            '2' => Some(Flag::Mkdir),
            '3' => Some(Flag::Upload),
            '4' => Some(Flag::Audio),
            '5' => Some(Flag::Modify),
            _ => None,
        }
    }

    fn get(self, u: &AdminUser) -> bool {
        match self {
            Flag::Admin => u.admin,
            Flag::Mkdir => u.allow_mkdir,
            Flag::Upload => u.allow_upload,
            Flag::Audio => u.allow_server_audio,
            Flag::Modify => u.allow_file_modify,
        }
    }

    fn set(self, a: &mut UserAccess, on: bool) {
        match self {
            Flag::Admin => a.admin = on,
            Flag::Mkdir => a.allow_mkdir = on,
            Flag::Upload => a.allow_upload = on,
            Flag::Audio => a.allow_server_audio = on,
            Flag::Modify => a.allow_file_modify = on,
        }
    }

    fn header(self) -> String {
        match self {
            Flag::Admin => t!("usr.col_admin"),
            Flag::Mkdir => t!("usr.col_folders"),
            Flag::Upload => t!("usr.col_upload"),
            Flag::Audio => t!("usr.col_audio"),
            Flag::Modify => t!("usr.col_modify"),
        }
        .to_string()
    }
}

/// The add form's fields, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AField {
    Username,
    Password,
    Lib(usize),
    Admin,
    Mkdir,
    Upload,
    Audio,
}

/// `a`: the webapp's Add User form. Every library ticked to start (its
/// select pre-selects all), admin ticked only on a server with no users
/// (its makeAdmin default), folders and uploads on, server audio off.
#[derive(Clone, Debug)]
pub(crate) struct AddForm {
    pub username: Input,
    pub password: Input,
    pub libraries: Vec<(String, bool)>,
    pub admin: bool,
    pub mkdir: bool,
    pub upload: bool,
    pub audio: bool,
    /// Index into [`AddForm::fields`].
    pub focus: usize,
    pub error: Option<String>,
}

impl AddForm {
    fn new(libraries: &[String], first: bool) -> Self {
        AddForm {
            username: Input::default(),
            password: Input::default(),
            libraries: libraries.iter().map(|l| (l.clone(), true)).collect(),
            admin: first,
            mkdir: true,
            upload: true,
            audio: false,
            focus: 0,
            error: None,
        }
    }

    fn fields(&self) -> Vec<AField> {
        let mut fields = vec![AField::Username, AField::Password];
        fields.extend((0..self.libraries.len()).map(AField::Lib));
        fields.extend([AField::Admin, AField::Mkdir, AField::Upload, AField::Audio]);
        fields
    }

    fn focused(&self) -> AField {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    fn checked(&self, field: AField) -> bool {
        match field {
            AField::Lib(i) => self.libraries.get(i).is_some_and(|(_, on)| *on),
            AField::Admin => self.admin,
            AField::Mkdir => self.mkdir,
            AField::Upload => self.upload,
            AField::Audio => self.audio,
            AField::Username | AField::Password => false,
        }
    }

    /// Flip a checkbox field; a text field is left alone (false).
    fn toggle(&mut self, field: AField) -> bool {
        match field {
            AField::Lib(i) => {
                if let Some(entry) = self.libraries.get_mut(i) {
                    entry.1 = !entry.1;
                }
            }
            AField::Admin => self.admin = !self.admin,
            AField::Mkdir => self.mkdir = !self.mkdir,
            AField::Upload => self.upload = !self.upload,
            AField::Audio => self.audio = !self.audio,
            AField::Username | AField::Password => return false,
        }
        true
    }

    fn input_mut(&mut self, field: AField) -> Option<&mut Input> {
        match field {
            AField::Username => Some(&mut self.username),
            AField::Password => Some(&mut self.password),
            _ => None,
        }
    }
}

/// `l`: the library grant as a checkbox list.
#[derive(Clone, Debug)]
pub(crate) struct LibsDraft {
    pub username: String,
    pub libraries: Vec<(String, bool)>,
    pub cursor: usize,
}

/// `p`: a new password, typed twice because the terminal masks it.
#[derive(Clone, Debug)]
pub(crate) struct PwDraft {
    pub username: String,
    pub password: Input,
    pub again: Input,
    /// 0 = PASSWORD, 1 = AGAIN.
    pub focus: usize,
    pub error: Option<String>,
}

impl PwDraft {
    /// Both filled and equal: what the button waits for.
    fn ready(&self) -> bool {
        !self.password.value().is_empty() && self.password.value() == self.again.value()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Modal {
    None,
    Add(Box<AddForm>),
    Libraries(LibsDraft),
    Password(PwDraft),
    Remove(String),
}

/// Everything a click can mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    Add,
    Select(usize),
    TableScroll(i8),
    TableScrollTo(usize),
    Flip(String, Flag),
    Libraries(String),
    Password(String),
    Remove(String),
    RemoveConfirm,
    RemoveCancel,
    FormFocus(usize),
    FormToggle(usize),
    FormSubmit,
    FormCancel,
    LibsRow(usize),
    LibsSubmit,
    LibsCancel,
    PwFocus(usize),
    PwSubmit,
    PwCancel,
    Quit,
}

/// A server call queued from input handling and run after the next draw.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Load,
    Add(NewUser),
    Remove(String),
    Password { username: String, password: String },
    Vpaths { username: String, vpaths: Vec<String> },
    Access { username: String, access: UserAccess },
    /// The first user on a public server: the room signs in as them.
    SignIn { username: String, password: String },
}

enum Done {
    Loaded(Result<(BTreeMap<String, AdminUser>, Vec<String>), ApiError>),
    Added { username: String, result: Result<(), ApiError> },
    Removed { username: String, result: Result<(), ApiError> },
    PasswordSet { username: String, result: Result<(), ApiError> },
    VpathsSet { username: String, result: Result<(), ApiError> },
    AccessSet { username: String, result: Result<(), ApiError> },
    SignedIn { username: String, result: Result<LoginResponse, ApiError> },
}

/// The worker thread: one op at a time, results back over the channel.
fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load => Done::Loaded(client.admin_users().map(|users| {
                    // The library list feeds the add form and the grant
                    // modal; best-effort, the users are the point.
                    let libraries = client
                        .admin_directories()
                        .map(|dirs| dirs.into_keys().collect())
                        .unwrap_or_default();
                    (users, libraries)
                })),
                Op::Add(user) => {
                    let result = client.admin_add_user(&user).map(|_| ());
                    Done::Added { username: user.username, result }
                }
                Op::Remove(username) => {
                    let result = client.admin_delete_user(&username).map(|_| ());
                    Done::Removed { username, result }
                }
                Op::Password { username, password } => {
                    let result = client.admin_set_user_password(&username, &password).map(|_| ());
                    Done::PasswordSet { username, result }
                }
                Op::Vpaths { username, vpaths } => {
                    let result = client.admin_set_user_vpaths(&username, &vpaths).map(|_| ());
                    Done::VpathsSet { username, result }
                }
                Op::Access { username, access } => {
                    let result = client.admin_set_user_access(&username, &access).map(|_| ());
                    Done::AccessSet { username, result }
                }
                Op::SignIn { username, password } => {
                    let result = client.login_shared(&username, &password);
                    Done::SignedIn { username, result }
                }
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
    pub users: BTreeMap<String, AdminUser>,
    pub libraries: Vec<String>,
    pub loaded: bool,
    /// Set once the room signed itself in after the first user.
    pub signed_in_as: Option<String>,
    /// Signed in, but the session could not be kept: the reason.
    pub session_unsaved: Option<String>,
    pub sel: Option<usize>,
    pub modal: Modal,
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    in_flight: bool,
    tscroll: usize,
    sel_anchor: Option<usize>,
    last_load: Option<Instant>,
    /// The add in flight: its name and password, kept until the server
    /// answers — on a server that had no users, they sign the room in.
    pending: Option<(String, String)>,
    ui: Surface<Act>,
}

/// The room, loading: what `mstream-player admin users` opens.
pub(super) fn start(client: Client) -> Room {
    let mut room = Room::new(client);
    room.queue(Op::Load, t!("usr.busy_loading"));
    room
}

impl Room {
    pub(crate) fn new(client: Client) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            users: BTreeMap::new(),
            libraries: Vec::new(),
            loaded: false,
            signed_in_as: None,
            session_unsaved: None,
            sel: None,
            modal: Modal::None,
            note: None,
            busy: None,
            queued: None,
            in_flight: false,
            tscroll: 0,
            sel_anchor: None,
            last_load: None,
            pending: None,
            ui: Surface::new(),
        }
    }

    fn names(&self) -> Vec<String> {
        self.users.keys().cloned().collect()
    }

    pub(crate) fn selected_name(&self) -> Option<String> {
        self.sel.and_then(|i| self.users.keys().nth(i).cloned())
    }

    fn admins(&self) -> usize {
        self.users.values().filter(|u| u.admin).count()
    }

    /// The one admin there is — the flag and the row the guards protect.
    fn last_admin(&self, name: &str) -> bool {
        self.admins() == 1 && self.users.get(name).is_some_and(|u| u.admin)
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    fn reload(&mut self, loud: bool) {
        if loud {
            self.queue(Op::Load, t!("usr.busy_loading"));
        } else {
            self.queued = Some(Op::Load);
        }
    }

    fn fail(&mut self, what: &str, e: ApiError) {
        self.note = Some((gate_message(&e, what), true));
    }

    // ── Input ───────────────────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Add => {
                let first = self.users.is_empty();
                self.modal = Modal::Add(Box::new(AddForm::new(&self.libraries, first)));
                self.note = None;
            }
            Act::Select(i) => {
                if i < self.users.len() {
                    self.sel = Some(i);
                }
            }
            Act::TableScroll(delta) => {
                self.tscroll = if delta < 0 { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            Act::TableScrollTo(pos) => self.tscroll = pos,
            Act::Flip(name, flag) => self.flip(&name, flag),
            Act::Libraries(name) => {
                if let Some(u) = self.users.get(&name) {
                    let libraries = self.libraries.iter().map(|l| (l.clone(), u.vpaths.contains(l))).collect();
                    self.modal = Modal::Libraries(LibsDraft { username: name, libraries, cursor: 0 });
                    self.note = None;
                }
            }
            Act::Password(name) => {
                if self.users.contains_key(&name) {
                    self.modal = Modal::Password(PwDraft {
                        username: name,
                        password: Input::default(),
                        again: Input::default(),
                        focus: 0,
                        error: None,
                    });
                    self.note = None;
                }
            }
            Act::Remove(name) => {
                if self.users.contains_key(&name) {
                    self.modal = Modal::Remove(name);
                }
            }
            Act::RemoveConfirm => {
                if let Modal::Remove(name) = &self.modal {
                    let name = name.clone();
                    self.modal = Modal::None;
                    self.queue(Op::Remove(name.clone()), t!("usr.busy_removing", user = printable(&name, NAME_MAX)));
                }
            }
            Act::RemoveCancel | Act::FormCancel | Act::LibsCancel | Act::PwCancel => self.modal = Modal::None,
            Act::FormFocus(i) => {
                if let Modal::Add(f) = &mut self.modal {
                    f.focus = i.min(f.fields().len() - 1);
                }
            }
            Act::FormToggle(i) => {
                if let Modal::Add(f) = &mut self.modal {
                    let fields = f.fields();
                    if let Some(field) = fields.get(i).copied() {
                        f.toggle(field);
                        f.focus = i;
                    }
                }
            }
            Act::FormSubmit => self.submit_form(),
            Act::LibsRow(i) => {
                if let Modal::Libraries(d) = &mut self.modal
                    && let Some(entry) = d.libraries.get_mut(i)
                {
                    entry.1 = !entry.1;
                    d.cursor = i;
                }
            }
            Act::LibsSubmit => {
                if let Modal::Libraries(d) = &self.modal {
                    let username = d.username.clone();
                    let vpaths = d.libraries.iter().filter(|(_, on)| *on).map(|(l, _)| l.clone()).collect();
                    self.queue(Op::Vpaths { username, vpaths }, t!("usr.busy_saving"));
                }
            }
            Act::PwFocus(i) => {
                if let Modal::Password(d) = &mut self.modal {
                    d.focus = i.min(1);
                }
            }
            Act::PwSubmit => self.submit_password(),
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    /// One flag on one user, at once — the other four echoed, as the route
    /// demands. The last admin keeps the admin flag: nobody could open this
    /// panel afterwards.
    fn flip(&mut self, name: &str, flag: Flag) {
        let Some(u) = self.users.get(name) else { return };
        if flag == Flag::Admin && self.last_admin(name) {
            self.note = Some((t!("usr.last_admin_keep", user = printable(name, NAME_MAX)).to_string(), true));
            return;
        }
        let mut access = UserAccess::of(u);
        flag.set(&mut access, !flag.get(u));
        self.queue(Op::Access { username: name.to_string(), access }, t!("usr.busy_saving"));
    }

    /// The add form's Enter: the refusals that need no server, then the call.
    fn submit_form(&mut self) {
        let taken: Vec<String> = self.names();
        let Modal::Add(f) = &mut self.modal else { return };
        let username = f.username.value().trim().to_string();
        if username.is_empty() {
            f.focus = 0;
            f.error = Some(t!("usr.err_name").to_string());
            return;
        }
        if taken.contains(&username) {
            f.focus = 0;
            f.error = Some(t!("usr.err_taken", user = printable(&username, NAME_MAX)).to_string());
            return;
        }
        if f.password.value().is_empty() {
            f.focus = 1;
            f.error = Some(t!("usr.err_password").to_string());
            return;
        }
        f.error = None;
        let password = f.password.value().to_string();
        let user = NewUser {
            username: username.clone(),
            password: password.clone(),
            vpaths: f.libraries.iter().filter(|(_, on)| *on).map(|(l, _)| l.clone()).collect(),
            admin: f.admin,
            allow_mkdir: f.mkdir,
            allow_upload: f.upload,
            allow_server_audio: f.audio,
        };
        self.pending = Some((username.clone(), password));
        self.queue(Op::Add(user), t!("usr.busy_adding", user = printable(&username, NAME_MAX)));
    }

    /// The password modal's Enter: empty and mismatched are refused here.
    fn submit_password(&mut self) {
        let Modal::Password(d) = &mut self.modal else { return };
        if d.password.value().is_empty() {
            d.focus = 0;
            d.error = Some(t!("usr.err_password").to_string());
            return;
        }
        if !d.ready() {
            d.focus = 1;
            d.error = Some(t!("usr.err_mismatch").to_string());
            return;
        }
        d.error = None;
        let username = d.username.clone();
        let password = d.password.value().to_string();
        self.queue(Op::Password { username, password }, t!("usr.busy_saving"));
    }

    // ── Server calls ────────────────────────────────────────────────────────

    fn dispatch_queued(&mut self) {
        if self.in_flight {
            return;
        }
        let Some(op) = self.queued.take() else { return };
        self.in_flight = true;
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
            Done::Loaded(Ok((users, libraries))) => {
                self.users = users;
                self.libraries = libraries;
                self.loaded = true;
                self.last_load = Some(Instant::now());
                self.sel = match self.sel {
                    Some(s) if !self.users.is_empty() => Some(s.min(self.users.len() - 1)),
                    _ => None,
                };
            }
            Done::Loaded(Err(e)) => {
                self.last_load = Some(Instant::now());
                self.fail(&t!("usr.load_failed"), e);
            }
            Done::Added { username, result: Ok(()) } => {
                self.modal = Modal::None;
                let shown = printable(&username, NAME_MAX);
                // The first user on a server that had none: logins are on
                // from this moment, and the room's next call would be
                // refused — so it signs in as the account it just made.
                let first = self.users.is_empty();
                match (first, self.pending.take()) {
                    (true, Some((name, password))) if name == username => {
                        self.queue(Op::SignIn { username, password }, t!("usr.busy_signing_in", user = shown));
                    }
                    _ => {
                        self.note = Some((t!("usr.done_added", user = shown).to_string(), false));
                        self.reload(false);
                    }
                }
            }
            Done::Added { username, result: Err(e) } => {
                self.pending = None;
                let what = t!("usr.fail_add", user = printable(&username, NAME_MAX)).to_string();
                match &mut self.modal {
                    Modal::Add(f) => f.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::SignedIn { username, result: Ok(resp) } => {
                let server = self.client.server();
                match Client::new(&server) {
                    Ok(fresh) => self.client = Arc::new(fresh.with_token(Some(resp.token.clone()))),
                    Err(e) => {
                        self.fail(&t!("usr.signin_failed", user = printable(&username, NAME_MAX), err = ""), e);
                        return;
                    }
                }
                let shown = printable(&username, NAME_MAX);
                match login::remember(&server, &username, &resp.token) {
                    Ok(()) => {
                        self.session_unsaved = None;
                        self.note = Some((t!("usr.done_signed_in", user = shown).to_string(), false));
                    }
                    Err(e) => {
                        self.note = Some((t!("usr.signed_in_unsaved", user = shown, err = e.clone()).to_string(), true));
                        self.session_unsaved = Some(e);
                    }
                }
                self.signed_in_as = Some(username);
                self.reload(false);
            }
            Done::SignedIn { username, result: Err(e) } => {
                self.note = Some((
                    t!("usr.signin_failed", user = printable(&username, NAME_MAX), err = e.to_string()).to_string(),
                    true,
                ));
                self.reload(false);
            }
            Done::Removed { username, result: Ok(()) } => {
                self.note = Some((t!("usr.done_removed", user = printable(&username, NAME_MAX)).to_string(), false));
                self.reload(false);
            }
            Done::Removed { username, result: Err(e) } => {
                self.fail(&t!("usr.fail_remove", user = printable(&username, NAME_MAX)), e);
            }
            Done::PasswordSet { username, result: Ok(()) } => {
                self.modal = Modal::None;
                self.note = Some((t!("usr.done_password", user = printable(&username, NAME_MAX)).to_string(), false));
            }
            Done::PasswordSet { result: Err(e), .. } => {
                let what = t!("usr.fail_password").to_string();
                match &mut self.modal {
                    Modal::Password(d) => d.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::VpathsSet { username, result: Ok(()) } => {
                self.modal = Modal::None;
                self.note = Some((t!("usr.done_libraries", user = printable(&username, NAME_MAX)).to_string(), false));
                self.reload(false);
            }
            Done::VpathsSet { result: Err(e), .. } => {
                let what = t!("usr.fail_libraries").to_string();
                self.fail(&what, e);
            }
            Done::AccessSet { username, result: Ok(()) } => {
                self.note = Some((t!("usr.done_access", user = printable(&username, NAME_MAX)).to_string(), false));
                self.reload(false);
            }
            Done::AccessSet { username, result: Err(e) } => {
                self.fail(&t!("usr.fail_access", user = printable(&username, NAME_MAX)), e);
                self.reload(false);
            }
        }
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

    /// The quiet reload — never under a modal (its draft would go stale),
    /// never on top of a call.
    fn tick(&mut self) {
        if self.loaded
            && matches!(self.modal, Modal::None)
            && !self.in_flight
            && self.queued.is_none()
            && self.last_load.is_none_or(|t| t.elapsed() >= POLL)
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

// ── Keys ─────────────────────────────────────────────────────────────────────

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match &mut room.modal {
        Modal::Add(f) => {
            let n = f.fields().len();
            let field = f.focused();
            return match code {
                KeyCode::Esc => room.act(Act::FormCancel),
                KeyCode::Enter => room.act(Act::FormSubmit),
                KeyCode::Tab | KeyCode::Down => {
                    f.focus = (f.focus + 1) % n;
                    None
                }
                KeyCode::BackTab | KeyCode::Up => {
                    f.focus = (f.focus + n - 1) % n;
                    None
                }
                KeyCode::Char(' ') if !matches!(field, AField::Username | AField::Password) => {
                    f.toggle(field);
                    None
                }
                _ => {
                    if let Some(input) = f.input_mut(field) {
                        input.handle_event(&TermEvent::Key(key));
                    }
                    None
                }
            };
        }
        Modal::Libraries(d) => {
            let n = d.libraries.len();
            return match code {
                KeyCode::Esc => room.act(Act::LibsCancel),
                KeyCode::Enter => room.act(Act::LibsSubmit),
                KeyCode::Down | KeyCode::Tab => {
                    if n > 0 {
                        d.cursor = (d.cursor + 1) % n;
                    }
                    None
                }
                KeyCode::Up | KeyCode::BackTab => {
                    if n > 0 {
                        d.cursor = (d.cursor + n - 1) % n;
                    }
                    None
                }
                KeyCode::Char(' ') => {
                    let i = d.cursor;
                    room.act(Act::LibsRow(i))
                }
                _ => None,
            };
        }
        Modal::Password(d) => {
            return match code {
                KeyCode::Esc => room.act(Act::PwCancel),
                KeyCode::Enter => room.act(Act::PwSubmit),
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                    d.focus = 1 - d.focus;
                    None
                }
                _ => {
                    let input = if d.focus == 0 { &mut d.password } else { &mut d.again };
                    input.handle_event(&TermEvent::Key(key));
                    None
                }
            };
        }
        Modal::Remove(_) => {
            return match code {
                KeyCode::Char('y') => room.act(Act::RemoveConfirm),
                // Enter is the SAFE choice on a warning gate.
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('n') => room.act(Act::RemoveCancel),
                _ => None,
            };
        }
        Modal::None => {}
    }

    let n = room.users.len();
    let name = room.selected_name();
    match code {
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
        KeyCode::Esc => {
            if room.sel.is_some() {
                room.sel = None;
                None
            } else {
                room.act(Act::Quit)
            }
        }
        KeyCode::Char('a') => room.act(Act::Add),
        KeyCode::Char('l') => name.and_then(|name| room.act(Act::Libraries(name))),
        KeyCode::Char('p') => name.and_then(|name| room.act(Act::Password(name))),
        KeyCode::Char('r') | KeyCode::Delete => name.and_then(|name| room.act(Act::Remove(name))),
        KeyCode::Char(c) if Flag::from_digit(c).is_some() => {
            let flag = Flag::from_digit(c)?;
            name.and_then(|name| room.act(Act::Flip(name, flag)))
        }
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

    draw_header(frame, area, &t!("usr.title"), &host_of(&room.client));
    let column = Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(5) };
    draw_body(frame, room, column);

    // The cursor row's own line, when nothing louder holds the note line.
    let row_note = match (&room.note, &room.busy, room.selected_name()) {
        (None, None, Some(name)) if !modal_open => room.users.get(&name).map(|u| {
            let user = printable(&name, NAME_MAX);
            let text = if u.vpaths.is_empty() {
                t!("usr.note_sees_none", user = user).to_string()
            } else {
                t!("usr.note_sees", user = user, libs = u.vpaths.join(" · ")).to_string()
            };
            (format!("{text}{}", t!("usr.note_flags")), false)
        }),
        _ => None,
    };
    let note = room.note.clone().or(row_note);
    draw_bottom(frame, area, note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Add(f) => draw_add(frame, room, area, &f),
        Modal::Libraries(d) => draw_libraries(frame, room, area, &d),
        Modal::Password(d) => draw_password(frame, room, area, &d),
        Modal::Remove(name) => draw_remove(frame, room, area, &name),
    }

    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Add(_) => t!("usr.hint_add"),
        Modal::Libraries(_) => t!("usr.hint_libraries"),
        Modal::Password(_) => t!("usr.hint_password"),
        Modal::Remove(_) => t!("usr.hint_remove"),
        Modal::None => match (room.users.is_empty(), room.sel) {
            (true, _) => t!("usr.hint_empty"),
            (false, None) => t!("usr.hint_rows"),
            (false, Some(_)) => t!("usr.hint_selected"),
        },
    }
    .to_string()
}

/// The state line, the add card, the table.
fn draw_body(frame: &mut Frame, room: &mut Room, column: Rect) {
    let line = |y: u16| Rect { x: column.x, y, width: column.width, height: 1 };
    let mut y = column.y;

    // The state line: public mode in gold, else the count — and, once the
    // room signed itself in, who it is signed in as.
    if room.loaded {
        let public = room.users.is_empty();
        let spans = if public {
            vec![
                Span::styled(t!("usr.state_public").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD)),
                Span::styled(t!("usr.state_public_detail").to_string(), Style::default().fg(th().gold)),
            ]
        } else if let Some(user) = &room.signed_in_as {
            let user = printable(user, NAME_MAX);
            let detail = match &room.session_unsaved {
                None => t!("usr.state_signed_in_detail", user = user).to_string(),
                Some(err) => t!("usr.state_signed_in_unsaved", user = user, err = err.clone()).to_string(),
            };
            vec![
                Span::styled(t!("usr.state_signed_in").to_string(), Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                Span::styled(detail, if room.session_unsaved.is_some() { Style::default().fg(th().gold) } else { Style::default() }),
            ]
        } else {
            let n = room.users.len();
            let admins = room.admins();
            let head = if n == 1 { t!("usr.state_users_one") } else { t!("usr.state_users", n = n) }.to_string();
            let tail = if admins == 1 { t!("usr.state_admins_one") } else { t!("usr.state_admins", n = admins) }.to_string();
            vec![Span::styled(head, bold()), Span::raw(tail)]
        };
        frame.render_widget(Paragraph::new(Line::from(spans)), line(y));
        if !public {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.polls").to_string(), dim())).alignment(Alignment::Right), line(y));
        }
    }
    y += 2;

    // The add card: the room's one add affordance, above the table.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = room.ui.pointer.is_some_and(|p| add_rect.contains(p));
    let add_color = if add_hover { th().bright } else { th().ok };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(add_color));
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    let label = if room.loaded && room.users.is_empty() { t!("usr.add_first_card") } else { t!("usr.add_card") };
    frame.render_widget(
        Paragraph::new(Span::styled(label.to_string(), Style::default().fg(add_color).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        inner,
    );
    room.ui.click(add_rect, Act::Add);
    y += 4;

    // The table. The [X] remove control sits to the RIGHT of the selection.
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let sel_end = column.x + sel_width;
    let x_col = sel_end + 1;
    let user_x = column.x;
    let admin_x = user_x + USER_W + 2;
    let admin_w = (Flag::Admin.header().chars().count() as u16).max(3);
    let libs_x = admin_x + admin_w + 2;
    // The four flag columns hang off the selection's right edge.
    let mut flag_cols: Vec<(Flag, u16, u16)> = Vec::new(); // (flag, x, w)
    let mut right = sel_end;
    for flag in [Flag::Modify, Flag::Audio, Flag::Upload, Flag::Mkdir] {
        let w = (flag.header().chars().count() as u16).max(3);
        right = right.saturating_sub(w);
        flag_cols.push((flag, right, w));
        right = right.saturating_sub(2);
    }
    flag_cols.reverse();
    let libs_w = flag_cols.first().map_or(0, |(_, x, _)| x.saturating_sub(2).saturating_sub(libs_x));
    let mut cols: Vec<(u16, u16, String, bool)> = vec![
        (user_x, USER_W, t!("usr.col_user").to_string(), false),
        (admin_x, admin_w, Flag::Admin.header(), false),
        (libs_x, libs_w, t!("usr.col_libraries").to_string(), false),
    ];
    cols.extend(flag_cols.iter().map(|(f, x, w)| (*x, *w, f.header(), false)));
    for (x, w, word, _) in &cols {
        frame.render_widget(Paragraph::new(Span::styled(clip(word, *w), dim())), Rect { x: *x, y, width: *w, height: 1 });
    }
    y += 1;
    frame.render_widget(Paragraph::new(Span::styled("─".repeat(column.width as usize), dim())), line(y));
    y += 1;

    if room.users.is_empty() {
        if room.loaded {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.empty_users").to_string(), dim())), line(y));
            // The webapp's two warnings, for the one state they apply to.
            let gold = Style::default().fg(th().gold);
            for (i, key) in ["usr.public_1", "usr.public_2"].iter().enumerate() {
                let r = y + 3 + i as u16;
                if r < column.bottom() {
                    frame.render_widget(Paragraph::new(Span::styled(t!(*key).to_string(), gold)), line(r));
                }
            }
            if y + 6 < column.bottom() {
                frame.render_widget(Paragraph::new(Span::styled(t!("usr.public_3").to_string(), dim())), line(y + 6));
            }
        }
        return;
    }

    let rows_rect = Rect { x: column.x, y, width: sel_width, height: column.bottom().saturating_sub(y) };
    if rows_rect.height == 0 {
        return;
    }
    let names = room.names();
    let users = room.users.clone();
    // A checkbox cell's glyph — a bar on the selected row, its own colour
    // otherwise — sitting one cell in from the column's left edge.
    let glyph_x = |x: u16, w: u16| x + (w.saturating_sub(3)) / 2;
    table_rows(frame, room, rows_rect, names.len(), |frame, room, i, rect, selected, hovered| {
        let name = &names[i];
        let Some(u) = users.get(name) else { return };
        let cell = |x: u16, w: u16| Rect { x, y: rect.y, width: w, height: 1 };
        let base = cell_style(selected, hovered, Style::default());
        frame.render_widget(
            Paragraph::new(Span::styled(clip(&printable(name, NAME_MAX), USER_W), if selected || hovered { base.add_modifier(Modifier::BOLD) } else { base })),
            cell(user_x, USER_W),
        );
        let libs = if u.vpaths.is_empty() { "—".to_string() } else { u.vpaths.join(" · ") };
        frame.render_widget(Paragraph::new(Span::styled(clip(&libs, libs_w), cell_style(selected, hovered, if u.vpaths.is_empty() { dim() } else { Style::default() }))), cell(libs_x, libs_w));
        let mut boxes: Vec<(Flag, u16, u16)> = vec![(Flag::Admin, admin_x, admin_w)];
        boxes.extend(flag_cols.iter().copied());
        for (flag, x, w) in boxes {
            let on = flag.get(u);
            let glyph = if on { g("[✓]", "[x]") } else { "[ ]" };
            let own = if on { Style::default().fg(th().ok) } else { dim() };
            let glyph_rect = cell(glyph_x(x, w), 3);
            frame.render_widget(Paragraph::new(Span::styled(glyph, cell_style(selected, hovered, own))), glyph_rect);
            room.ui.click(glyph_rect, Act::Flip(name.clone(), flag));
        }
        // The [X], outside the selection: dim, red under the pointer.
        let x_rect = Rect { x: x_col, y: rect.y, width: 3, height: 1 };
        let x_hover = room.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover { Style::default().fg(th().danger).add_modifier(Modifier::BOLD) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        room.ui.click(x_rect, Act::Remove(name.clone()));
    });
}

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

fn cell_style(selected: bool, hovered: bool, own: Style) -> Style {
    if selected {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if hovered {
        Style::default().fg(th().bright)
    } else {
        own
    }
}

fn clip(s: &str, w: u16) -> String {
    let w = w as usize;
    if s.chars().count() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}

/// A labelled three-row field: the label above, the value inside. Masked
/// for a password.
#[allow(clippy::too_many_arguments)]
fn field_box(frame: &mut Frame, room: &mut Room, at: Rect, label: &str, input: &Input, focused: bool, masked: bool, act: Act) {
    frame.render_widget(Paragraph::new(Span::styled(label.to_string(), dim())), Rect { x: at.x, y: at.y, width: at.width, height: 1 });
    let field = Rect { x: at.x, y: at.y + 1, width: at.width, height: 3 };
    let hover = room.ui.pointer.is_some_and(|p| field.contains(p));
    let border = if hover { th().bright } else if focused { th().accent } else { th().dim };
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(border));
    let inner = block.inner(field);
    frame.render_widget(block, field);
    let w = inner.width.saturating_sub(2);
    let value = if masked { "•".repeat(input.value().chars().count()) } else { input.value().to_string() };
    let shown = if focused { kit::input_display(&value, input.cursor(), w) } else { clip(&value, w) };
    frame.render_widget(Paragraph::new(Span::raw(shown)), Rect { x: inner.x + 1, y: inner.y, width: w, height: 1 });
    room.ui.click(field, act);
}

/// A checkbox row in a modal: the glyph wears its state, the keyboard
/// cursor is the accent bar on the glyph, hover brightens the label.
#[allow(clippy::too_many_arguments)]
fn check_row(frame: &mut Frame, room: &mut Room, at: Rect, on: bool, focused: bool, label: &str, desc: &str, act: Act) {
    let hover = room.ui.pointer.is_some_and(|p| at.contains(p));
    let glyph_style = if focused {
        Style::default().fg(th().on_accent).bg(th().accent)
    } else if on {
        Style::default().fg(th().ok)
    } else {
        dim()
    };
    let label_style = if hover { Style::default().fg(th().bright).add_modifier(Modifier::BOLD) } else { Style::default() };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if on { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
            Span::styled(format!(" {label}"), label_style),
            Span::styled(desc.to_string(), dim()),
        ])),
        at,
    );
    room.ui.click(at, act);
}

fn draw_add(frame: &mut Frame, room: &mut Room, area: Rect, f: &AddForm) {
    let inner = kit::modal_frame(frame, area, 84, 22, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    let fits = |y: u16| y < inner.bottom();
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.form_title").to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.ui, inner, Act::FormCancel);

    let fields = f.fields();
    let index_of = |field: AField| fields.iter().position(|x| *x == field).unwrap_or(0);
    let focused = f.focused();
    let half = w.saturating_sub(4) / 2;
    let mut y = inner.y + 2;
    if fits(y + 3) {
        field_box(frame, room, Rect { x, y, width: half, height: 4 }, &t!("usr.field_username"), &f.username, focused == AField::Username, false, Act::FormFocus(index_of(AField::Username)));
        field_box(frame, room, Rect { x: x + half + 4, y, width: half, height: 4 }, &t!("usr.field_password"), &f.password, focused == AField::Password, true, Act::FormFocus(index_of(AField::Password)));
    }
    y += 5;

    if fits(y + 1) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_libraries").to_string(), dim())), line(y));
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_libraries_hint").to_string(), dim())).alignment(Alignment::Right), line(y));
        y += 1;
        if f.libraries.is_empty() {
            frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_no_libraries").to_string(), dim())), line(y));
        } else {
            // One row of ticks, each its own click target, 3 cells apart.
            let mut lx = x;
            for (i, (name, on)) in f.libraries.iter().enumerate() {
                let label = printable(name, NAME_MAX);
                let cw = label.chars().count() as u16 + 4;
                if lx + cw > x + w {
                    break;
                }
                let rect = Rect { x: lx, y, width: cw, height: 1 };
                check_row(frame, room, rect, *on, focused == AField::Lib(i), &label, "", Act::FormToggle(index_of(AField::Lib(i))));
                lx += cw + 3;
            }
        }
    }
    y += 2;

    if fits(y) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_permissions").to_string(), dim())), line(y));
    }
    y += 1;
    let perms = [
        (AField::Admin, t!("usr.perm_admin").to_string(), t!("usr.perm_admin_desc").to_string()),
        (AField::Mkdir, t!("usr.perm_mkdir").to_string(), String::new()),
        (AField::Upload, t!("usr.perm_upload").to_string(), String::new()),
        (AField::Audio, t!("usr.perm_audio").to_string(), t!("usr.perm_audio_desc").to_string()),
    ];
    for (field, label, desc) in perms {
        if fits(y) {
            check_row(frame, room, line(y), f.checked(field), focused == field, &label, &desc, Act::FormToggle(index_of(field)));
        }
        y += 1;
    }
    y += 1;
    if fits(y) {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_permanent").to_string(), dim())).wrap(Wrap { trim: true }), line(y));
    }
    y += 1;
    if let Some(err) = &f.error
        && fits(y)
    {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))).wrap(Wrap { trim: true }), line(y));
    }
    let by = inner.bottom().saturating_sub(1);
    let label = t!("usr.form_add").to_string();
    let bw = label.chars().count() as u16 + 4;
    kit::button(frame, &mut room.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, true, Act::FormSubmit);
}

fn draw_libraries(frame: &mut Frame, room: &mut Room, area: Rect, d: &LibsDraft) {
    let rows = d.libraries.len().max(1) as u16;
    let inner = kit::modal_frame(frame, area, 60, rows + 9, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    let fits = |y: u16| y < inner.bottom();
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.libs_title", user = printable(&d.username, NAME_MAX)).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.ui, inner, Act::LibsCancel);
    let mut y = inner.y + 2;
    if d.libraries.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(t!("usr.form_no_libraries").to_string(), dim())), line(y));
        y += 1;
    }
    for (i, (name, on)) in d.libraries.iter().enumerate() {
        if !fits(y) {
            break;
        }
        let rect = line(y);
        let cursor = i == d.cursor;
        if cursor {
            frame.render_widget(Paragraph::new(Span::styled(" ".repeat(w as usize), Style::default().fg(th().on_accent).bg(th().accent))), rect);
        }
        let glyph_style = if cursor { Style::default().fg(th().on_accent).bg(th().accent) } else if *on { Style::default().fg(th().ok) } else { dim() };
        let name_style = if cursor { Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD) } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(if *on { g("[✓]", "[x]") } else { "[ ]" }, glyph_style),
                Span::styled(format!(" {}", printable(name, NAME_MAX)), name_style),
            ])),
            rect,
        );
        room.ui.click(rect, Act::LibsRow(i));
        y += 1;
    }
    y += 1;
    for key in ["usr.libs_note_1", "usr.libs_note_2"] {
        if fits(y) {
            frame.render_widget(Paragraph::new(Span::styled(t!(key).to_string(), dim())), line(y));
        }
        y += 1;
    }
    let by = inner.bottom().saturating_sub(1);
    let label = t!("usr.libs_save").to_string();
    let bw = label.chars().count() as u16 + 4;
    kit::button(frame, &mut room.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, true, Act::LibsSubmit);
}

fn draw_password(frame: &mut Frame, room: &mut Room, area: Rect, d: &PwDraft) {
    let inner = kit::modal_frame(frame, area, 60, 15, th().accent);
    let x = inner.x + 1;
    let w = inner.width.saturating_sub(2);
    let line = |y: u16| Rect { x, y, width: w, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(t!("usr.pw_title", user = printable(&d.username, NAME_MAX)).to_string(), Style::default().fg(th().accent).add_modifier(Modifier::BOLD))),
        line(inner.y),
    );
    kit::modal_close_plain(frame, &mut room.ui, inner, Act::PwCancel);
    let fw = w.min(44);
    if inner.y + 5 < inner.bottom() {
        field_box(frame, room, Rect { x, y: inner.y + 2, width: fw, height: 4 }, &t!("usr.field_password"), &d.password, d.focus == 0, true, Act::PwFocus(0));
    }
    if inner.y + 10 < inner.bottom() {
        field_box(frame, room, Rect { x, y: inner.y + 7, width: fw, height: 4 }, &t!("usr.field_again"), &d.again, d.focus == 1, true, Act::PwFocus(1));
    }
    let by = inner.bottom().saturating_sub(1);
    if let Some(err) = &d.error {
        frame.render_widget(Paragraph::new(Span::styled(format!("{} {err}", g("!", "!")), Style::default().fg(th().gold))), line(by));
    }
    let label = t!("usr.pw_set").to_string();
    let bw = label.chars().count() as u16 + 4;
    // Dim until the two agree — the button waits, like the kit's disabled.
    kit::button(frame, &mut room.ui, Rect { x: inner.right().saturating_sub(bw + 1), y: by, width: bw, height: 1 }, &label, d.ready(), Act::PwSubmit);
}

fn draw_remove(frame: &mut Frame, room: &mut Room, area: Rect, name: &str) {
    let last_admin = room.last_admin(name);
    let inner = kit::modal_frame(frame, area, 70, 11, th().gold);
    let gold = Style::default().fg(th().gold);
    let user = printable(name, NAME_MAX);
    let mut lines = vec![
        Line::from(Span::styled(t!("usr.remove_title", user = user.clone()).to_string(), gold.add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled(t!("usr.remove_1").to_string(), gold)),
        Line::from(Span::styled(t!("usr.remove_2").to_string(), Style::default().fg(th().ok))),
    ];
    if last_admin {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(t!("usr.remove_last_admin", user = user).to_string(), gold.add_modifier(Modifier::BOLD))));
    }
    let body = Rect { x: inner.x + 1, y: inner.y, width: inner.width.saturating_sub(2), height: inner.height.saturating_sub(2) };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let y = inner.bottom().saturating_sub(1);
    let keep = t!("usr.remove_keep").to_string();
    let remove = t!("usr.remove_confirm").to_string();
    let remove_w = remove.chars().count() as u16 + 4;
    let keep_w = keep.chars().count() as u16 + 4;
    let keep_x = inner.right().saturating_sub(remove_w + 2 + keep_w);
    let keep_rect = kit::button(frame, &mut room.ui, Rect { x: keep_x, y, width: inner.width, height: 1 }, &keep, true, Act::RemoveCancel);
    kit::button(frame, &mut room.ui, Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 }, &remove, false, Act::RemoveConfirm);
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key(code))
    }

    fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
    }

    fn user(admin: bool, libs: &[&str], mkdir: bool, upload: bool, audio: bool, modify: bool) -> AdminUser {
        AdminUser {
            admin,
            vpaths: libs.iter().map(|l| l.to_string()).collect(),
            allow_mkdir: mkdir,
            allow_upload: upload,
            allow_file_modify: modify,
            allow_server_audio: audio,
            allow_torrent: false,
        }
    }

    fn libs() -> Vec<String> {
        ["music", "audiobooks", "field-recordings"].iter().map(|s| s.to_string()).collect()
    }

    /// The canvas's four users: anna the only admin.
    fn loaded() -> Room {
        let mut r = room();
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("anna".to_string(), user(true, &["music", "audiobooks", "field-recordings"], true, true, true, true));
        users.insert("ben".to_string(), user(false, &["music", "field-recordings"], true, false, false, true));
        users.insert("guest".to_string(), user(false, &["music"], false, false, false, false));
        r.apply(Done::Loaded(Ok((users, libs()))));
        r
    }

    fn note(room: &Room) -> String {
        room.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
    }

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

    fn row(frame: &str, needle: &str) -> String {
        frame.lines().find(|l| l.contains(needle)).map(|l| l.to_string()).unwrap_or_else(|| panic!("no row with {needle:?}:\n{frame}"))
    }

    #[test]
    fn boots_loading_then_draws_the_table_with_its_flags_and_the_cursor_note() {
        let _en = english();
        let mut r = start(Client::new("http://home.mstream.example:3000").expect("client"));
        assert_eq!(r.queued, Some(Op::Load));
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("anna".to_string(), user(true, &["music", "audiobooks", "field-recordings"], true, true, true, true));
        users.insert("ben".to_string(), user(false, &["music", "field-recordings"], true, false, false, true));
        users.insert("guest".to_string(), user(false, &["music"], false, false, false, false));
        r.apply(Done::Loaded(Ok((users, libs()))));
        assert!(r.loaded);
        let frame = draw(&mut r);
        assert!(frame.contains("Users"), "{frame}");
        assert!(frame.contains("• 3 users — 1 admin · logins on"), "{frame}");
        assert!(frame.contains("reloads every 30 s"), "{frame}");
        assert!(frame.contains("Add a user ▸"), "{frame}");
        for word in ["USER", "ADMIN", "LIBRARIES", "FOLDERS", "UPLOAD", "AUDIO", "MODIFY"] {
            assert!(frame.contains(word), "{word} missing:\n{frame}");
        }
        let anna = row(&frame, "anna");
        assert_eq!(anna.matches("[✓]").count(), 5, "every flag on: {anna}");
        assert!(anna.contains("music · audiobooks · field-recordi…") && anna.contains("[X]"), "the column clips, the note line does not: {anna}");
        let guest = row(&frame, "guest");
        assert_eq!(guest.matches("[ ]").count(), 5, "every flag off: {guest}");
        assert!(frame.contains("↑ ↓ select · a add a user · q quit"), "{frame}");
        // The cursor picks up rows and the note line says what they see.
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down);
        assert_eq!(r.selected_name().as_deref(), Some("ben"));
        let frame = draw(&mut r);
        assert!(frame.contains("ben sees music · field-recordings · 1–5 flip a flag, saved at once"), "{frame}");
        assert!(frame.contains("r remove · Esc deselect"), "{frame}");
        assert!(press(&mut r, KeyCode::Esc).is_none() && r.sel.is_none(), "Esc stows the cursor");
        assert!(matches!(press(&mut r, KeyCode::Esc), Some(Outcome::Quit)));
    }

    #[test]
    fn public_mode_says_so_and_the_first_user_signs_the_room_in() {
        let _en = english();
        let scratch = crate::config::testing::Scratch::new("admin-users-first-signin");
        let _ = &scratch;
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Ok((BTreeMap::new(), libs()))));
        let frame = draw(&mut r);
        assert!(frame.contains("• public — no users, so no logins"), "{frame}");
        assert!(frame.contains("Add the first user ▸") && frame.contains("(no users yet — a adds the first)"), "{frame}");
        assert!(frame.contains("The first user turns logins on") && frame.contains("Make it an admin"), "{frame}");
        assert!(frame.contains("a add the first user · q quit"), "{frame}");

        press(&mut r, KeyCode::Char('a'));
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert!(f.admin, "admin is pre-ticked only on a server with no users");
        assert!(f.mkdir && f.upload && !f.audio, "the webapp's defaults");
        assert!(f.libraries.iter().all(|(_, on)| *on), "every library ticked");
        type_text(&mut r, "mira");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "hunter2");
        let frame = draw(&mut r);
        assert!(frame.contains("Add a user") && frame.contains("mira") && frame.contains("•••••••"), "{frame}");
        assert!(!frame.contains("hunter2"), "masked:\n{frame}");
        assert!(frame.contains("[✓] admin") && frame.contains("[✓] create folders") && frame.contains("[ ] server audio"), "{frame}");
        press(&mut r, KeyCode::Enter);
        assert_eq!(
            r.queued,
            Some(Op::Add(NewUser {
                username: "mira".into(),
                password: "hunter2".into(),
                vpaths: libs(),
                admin: true,
                allow_mkdir: true,
                allow_upload: true,
                allow_server_audio: false,
            }))
        );
        r.queued = None;
        // The server said yes: logins are on now, so the room signs in as mira.
        r.apply(Done::Added { username: "mira".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(r.queued, Some(Op::SignIn { username: "mira".into(), password: "hunter2".into() }));
        r.queued = None;
        r.apply(Done::SignedIn { username: "mira".into(), result: Ok(LoginResponse { token: "tok".into(), vpaths: vec![] }) });
        assert_eq!(r.signed_in_as.as_deref(), Some("mira"));
        assert!(note(&r).contains("signed in as mira") && note(&r).contains("saved"), "{}", note(&r));
        assert_eq!(r.queued, Some(Op::Load), "and reloads with the session");
        let creds = crate::config::load_credentials().unwrap();
        assert_eq!(crate::config::token_for(&creds, "http://home.mstream.example:3000"), Some("tok".to_string()));
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("mira".to_string(), user(true, &["music"], true, true, false, true));
        r.apply(Done::Loaded(Ok((users, libs()))));
        r.note = None;
        let frame = draw(&mut r);
        assert!(frame.contains("• logins on — signed in as mira · the session is saved"), "{frame}");
    }

    #[test]
    fn a_second_user_just_reloads_and_the_form_refuses_what_the_server_would_500() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Char('a'));
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert!(!f.admin, "admin stays off once users exist");
        press(&mut r, KeyCode::Enter);
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert_eq!(f.error.as_deref(), Some("a username is needed"));
        type_text(&mut r, "anna");
        press(&mut r, KeyCode::Enter);
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert_eq!(f.error.as_deref(), Some("anna already exists"));
        for _ in 0..4 {
            press(&mut r, KeyCode::Backspace);
        }
        type_text(&mut r, "zed");
        press(&mut r, KeyCode::Enter);
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert_eq!(f.error.as_deref(), Some("a password is needed"));
        assert_eq!(f.focused(), AField::Password, "focus moves to the empty field");
        type_text(&mut r, "pw");
        // Tab walks into the ticks: untick audiobooks, tick server audio.
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Char(' '));
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Tab);
        press(&mut r, KeyCode::Tab);
        let Modal::Add(f) = &r.modal else { panic!("the add form") };
        assert_eq!(f.focused(), AField::Audio);
        press(&mut r, KeyCode::Char(' '));
        press(&mut r, KeyCode::Enter);
        assert_eq!(
            r.queued,
            Some(Op::Add(NewUser {
                username: "zed".into(),
                password: "pw".into(),
                vpaths: vec!["music".into(), "field-recordings".into()],
                admin: false,
                allow_mkdir: true,
                allow_upload: true,
                allow_server_audio: true,
            }))
        );
        r.queued = None;
        r.apply(Done::Added { username: "zed".into(), result: Ok(()) });
        assert_eq!(r.queued, Some(Op::Load), "a server with users just reloads");
        assert_eq!(note(&r), "zed added");
        // A refusal keeps the form with the server's words.
        press(&mut r, KeyCode::Char('a'));
        type_text(&mut r, "zed2");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "pw");
        press(&mut r, KeyCode::Enter);
        r.queued = None;
        r.apply(Done::Added { username: "zed2".into(), result: Err(ApiError::Server { status: 500, message: "Server Error".into() }) });
        let Modal::Add(f) = &r.modal else { panic!("the form stays") };
        assert!(f.error.as_deref().is_some_and(|e| e.starts_with("could not add zed2")), "{:?}", f.error);
    }

    #[test]
    fn a_flag_flips_at_once_echoing_all_five_and_the_last_admin_keeps_the_flag() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down); // anna, the only admin
        press(&mut r, KeyCode::Char('1'));
        assert!(r.queued.is_none(), "nothing sent");
        assert_eq!(note(&r), "anna is the only admin — the flag stays until another admin exists");
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('3')); // upload: off → on
        assert_eq!(
            r.queued,
            Some(Op::Access {
                username: "ben".into(),
                access: UserAccess { admin: false, allow_mkdir: true, allow_upload: true, allow_file_modify: true, allow_server_audio: false },
            })
        );
        r.queued = None;
        r.apply(Done::AccessSet { username: "ben".into(), result: Ok(()) });
        assert_eq!(note(&r), "ben: access saved");
        assert_eq!(r.queued, Some(Op::Load));
        r.queued = None;
        // A click on ben's admin box does the same through the registry.
        r.act(Act::Flip("ben".into(), Flag::Admin));
        assert!(matches!(r.queued, Some(Op::Access { ref access, .. }) if access.admin && !access.allow_upload));
    }

    #[test]
    fn the_libraries_modal_ticks_and_saves_the_whole_list() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('l'));
        let frame = draw(&mut r);
        assert!(frame.contains("Libraries for ben"), "{frame}");
        assert!(frame.contains("[✓] music") && frame.contains("[ ] audiobooks") && frame.contains("[✓] field-recordings"), "{frame}");
        assert!(frame.contains("Admins see only these too"), "{frame}");
        assert!(frame.contains("Save ▸") && frame.contains("Space tick · Enter save"), "{frame}");
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Char(' '));
        press(&mut r, KeyCode::Enter);
        assert_eq!(r.queued, Some(Op::Vpaths { username: "ben".into(), vpaths: libs() }), "the whole list, in library order");
        r.queued = None;
        r.apply(Done::VpathsSet { username: "ben".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(note(&r), "ben: libraries saved");
        assert_eq!(r.queued, Some(Op::Load));
    }

    #[test]
    fn the_password_modal_refuses_a_mismatch_then_sends_masked_all_the_way() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down);
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('p'));
        press(&mut r, KeyCode::Enter);
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!(d.error.as_deref(), Some("a password is needed"));
        type_text(&mut r, "hunter2");
        press(&mut r, KeyCode::Tab);
        type_text(&mut r, "hunter3");
        press(&mut r, KeyCode::Enter);
        let Modal::Password(d) = &r.modal else { panic!("the password modal") };
        assert_eq!(d.error.as_deref(), Some("the two do not match"));
        assert!(r.queued.is_none());
        let frame = draw(&mut r);
        assert!(frame.contains("New password for ben") && frame.contains("the two do not match"), "{frame}");
        assert!(frame.contains("•••••••") && !frame.contains("hunter"), "masked:\n{frame}");
        press(&mut r, KeyCode::Backspace);
        type_text(&mut r, "2");
        press(&mut r, KeyCode::Enter);
        assert_eq!(r.queued, Some(Op::Password { username: "ben".into(), password: "hunter2".into() }));
        r.queued = None;
        r.apply(Done::PasswordSet { username: "ben".into(), result: Ok(()) });
        assert!(matches!(r.modal, Modal::None));
        assert_eq!(note(&r), "ben: password set");
    }

    #[test]
    fn the_remove_gate_names_the_cascade_and_the_last_admin_and_y_confirms() {
        let _en = english();
        let mut r = loaded();
        press(&mut r, KeyCode::Down); // anna
        press(&mut r, KeyCode::Char('r'));
        let frame = draw(&mut r);
        assert!(frame.contains("Remove anna?"), "{frame}");
        assert!(frame.contains("playlists, play history and library grants") && frame.contains("The music files stay"), "{frame}");
        assert!(frame.contains("anna is the only admin"), "{frame}");
        assert!(frame.contains("◂ Keep") && frame.contains("y remove · Esc keep"), "{frame}");
        assert!(press(&mut r, KeyCode::Enter).is_none() && matches!(r.modal, Modal::None), "Enter keeps");
        press(&mut r, KeyCode::Down); // ben
        press(&mut r, KeyCode::Char('r'));
        let frame = draw(&mut r);
        assert!(frame.contains("Remove ben?") && !frame.contains("only admin"), "{frame}");
        press(&mut r, KeyCode::Char('y'));
        assert_eq!(r.queued, Some(Op::Remove("ben".into())));
        assert!(matches!(r.modal, Modal::None));
        r.queued = None;
        r.apply(Done::Removed { username: "ben".into(), result: Ok(()) });
        assert_eq!(note(&r), "ben removed");
        assert_eq!(r.queued, Some(Op::Load));
    }

    #[test]
    fn load_errors_name_the_gate_that_bit_and_the_poll_is_quiet() {
        let _en = english();
        let mut r = room();
        r.queued = None;
        r.apply(Done::Loaded(Err(ApiError::Unauthorized)));
        assert!(note(&r).contains("login"), "{}", note(&r));
        r.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(note(&r).contains("locked"), "{}", note(&r));
        let mut r = loaded();
        r.tick();
        assert!(r.queued.is_none(), "just loaded: no reload yet");
        r.last_load = Some(Instant::now() - POLL);
        r.tick();
        assert_eq!(r.queued, Some(Op::Load));
        assert!(r.busy.is_none(), "quiet");
        r.queued = None;
        press(&mut r, KeyCode::Char('a'));
        r.last_load = Some(Instant::now() - POLL);
        r.tick();
        assert!(r.queued.is_none(), "never under a modal");
    }
}
