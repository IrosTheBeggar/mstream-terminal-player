//! The Users room: `mstream-player admin users` — who has an account on
//! the server, what each one may do, which libraries they see. The
//! webapp's Users page as one table with checkbox cells: a flag flips at
//! once (the whole set echoed, as the route demands), the library grant
//! and the password each have a modal, adding is a form, removing a gate.
//! A server with no users runs without logins; the first user ends that,
//! and this room signs itself in as that user so the panel stays open.
//!
//! This file is the room itself: its state, its worker, its keys and every
//! word it says. It names no drawing type, of ratatui's or the kit's. Its
//! view is a child module, `cells` (cells.rs), which draws the room in
//! character cells, for a terminal and for the GUI player's Admin tab; a
//! view in pixels would be its sibling and read the room the same way. A
//! click in a view is one of the room's [`Act`]s and a key is
//! [`Room::key`], so no view can disagree about what the room does. What
//! a view keeps between frames it keeps in a type of its own, held here
//! unopened (`cells::State`). The add form's fields and the password
//! modal's are tui_input's, edited through its crossterm handler, so the
//! keymap still names crossterm's event: a key reaches the room the same
//! way whichever view draws it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Outcome, gate_message, login, printable};
use crate::api::types::{AdminUser, LoginResponse};
use crate::api::{ApiError, Client, NewUser, UserAccess};
use crate::input::{KeyCode, KeyEvent};

/// The room in character cells: the terminal's room and the GUI's Admin
/// tab.
pub(crate) mod cells;

/// The room in pixels: its egui face, for the GUI player's Admin tab in a
/// window of its own.
#[cfg(feature = "window")]
pub(crate) mod egui_view;

/// A quiet reload: users rarely change, and the webapp never polls here.
const POLL: Duration = Duration::from_secs(30);
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
                // Both reads go out together: one round trip, not two
                // (performance audit #83).
                Op::Load => Done::Loaded(crate::api::wait(async {
                    let (users, libraries) =
                        tokio::join!(client.admin_users_async(), client.admin_directories_async());
                    // The library list feeds the add form and the grant
                    // modal; best-effort, the users are the point.
                    let libraries = libraries.map(|dirs| dirs.into_keys().collect()).unwrap_or_default();
                    Ok((users?, libraries))
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
    last_load: Option<Instant>,
    /// The add in flight: its name and password, kept until the server
    /// answers — on a server that had no users, they sign the room in.
    pending: Option<(String, String)>,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, and the cursor the table last followed.
    cells: cells::State,
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
            last_load: None,
            pending: None,
            cells: cells::State::new(),
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

    /// A key press: the room's keymap, the same under any view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
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

    /// The quiet reload — never under a modal (its draft would go stale),
    /// never on top of a call.
    pub(crate) fn tick(&mut self) {
        if self.loaded
            && matches!(self.modal, Modal::None)
            && !self.in_flight
            && self.queued.is_none()
            && self.last_load.is_none_or(|t| t.elapsed() >= POLL)
        {
            self.reload(false);
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

// ── The tips ─────────────────────────────────────────────────────────────────

/// The room's keyboard tips. Hosted, the base tips end on "Esc back"
/// rather than "q quit": in the GUI player Esc and `q` hand the keyboard
/// back to the hallway, and quitting is the hallway's own `q`.
fn footer_hint_as(room: &Room, hosted: bool) -> String {
    match &room.modal {
        Modal::Add(_) => t!("usr.hint_add"),
        Modal::Libraries(_) => t!("usr.hint_libraries"),
        Modal::Password(_) => t!("usr.hint_password"),
        Modal::Remove(_) => t!("usr.hint_remove"),
        Modal::None => match (room.users.is_empty(), room.sel, hosted) {
            (true, _, false) => t!("usr.hint_empty"),
            (true, _, true) => t!("usr.hint_empty_hosted"),
            (false, None, false) => t!("usr.hint_rows"),
            (false, None, true) => t!("usr.hint_rows_hosted"),
            (false, Some(_), _) => t!("usr.hint_selected"),
        },
    }
    .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::KeyModifiers;

    // The fixtures are the view's tests' too (cells.rs).

    pub(super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    pub(super) fn room() -> Room {
        Room::new(Client::new("http://home.mstream.example:3000").expect("client"))
    }

    pub(super) fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(super) fn press(room: &mut Room, code: KeyCode) -> Option<Outcome> {
        handle_key(room, key(code))
    }

    pub(super) fn type_text(room: &mut Room, text: &str) {
        for c in text.chars() {
            press(room, KeyCode::Char(c));
        }
    }

    pub(super) fn user(admin: bool, libs: &[&str], mkdir: bool, upload: bool, audio: bool, modify: bool) -> AdminUser {
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

    pub(super) fn libs() -> Vec<String> {
        ["music", "audiobooks", "field-recordings"].iter().map(|s| s.to_string()).collect()
    }

    /// The canvas's four users: anna the only admin.
    pub(super) fn loaded() -> Room {
        let mut r = room();
        r.queued = None;
        let mut users = BTreeMap::new();
        users.insert("anna".to_string(), user(true, &["music", "audiobooks", "field-recordings"], true, true, true, true));
        users.insert("ben".to_string(), user(false, &["music", "field-recordings"], true, false, false, true));
        users.insert("guest".to_string(), user(false, &["music"], false, false, false, false));
        r.apply(Done::Loaded(Ok((users, libs()))));
        r
    }

    pub(super) fn note(room: &Room) -> String {
        room.note.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
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

    #[test]
    fn a_load_asks_for_the_users_and_the_libraries_together() {
        // One round trip, not two (performance audit #83); the library
        // list stays best-effort.
        let server = super::super::waves::serve(&[2], |path| match path {
            "/api/v1/admin/users" => (200, r#"{"anna":{"admin":true,"vpaths":["music"]}}"#.into()),
            _ => (500, "{}".into()),
        });
        let (to_worker, from_worker) = spawn_worker();
        to_worker.send((Arc::new(Client::new(&server.url).expect("client")), Op::Load)).unwrap();
        let Done::Loaded(Ok((users, libraries))) =
            from_worker.recv_timeout(std::time::Duration::from_secs(30)).expect("an answer")
        else {
            panic!("a load");
        };
        assert_eq!(server.peak(), 2);
        assert_eq!((users.len(), libraries.len()), (1, 0));
    }
}
