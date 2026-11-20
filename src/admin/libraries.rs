//! The Libraries room: the server's music folders, drawn full-screen from
//! the UI kit the way the setup wizard is.
//!
//! The room is the wizard's folders screen grown up: the add card above the
//! table, the table itself (NAME · FOLDER · SYMLINKS, the ` [X]` remove
//! control outside the selection area), key tips on the bottom edge. No
//! bottom bar and no gold rule — nothing here is a step of anything.
//!
//! Adding a folder has three ways in and one way out. Booted with
//! `--same-machine` the card (and `b`) open the OS folder dialog through
//! the quick setup's picker, on the worker, so the room stays live behind
//! it; otherwise `b` browses the SERVER's disk through the admin file
//! explorer, and `t` types a path with server-fed completion either way.
//! Every path lands in the Name modal: the vpath is set once and never
//! changed (the server has no rename), so it is asked for explicitly with
//! the permanence stated. Removing goes through a warning gate.
//!
//! Every server call runs on a worker thread — the loop only draws, reads
//! input, and folds results back between frames (the wizard's Job/Done
//! pattern, ops carrying their own data). The terminal session around the
//! room — ground lease, mouse, pointer, the event loop — is the hub's
//! ([`super::run_tui`]), and the room meets it in its cells view, as a
//! [`super::Screen`].
//!
//! This file is the room itself: its state, its worker, its keys and every
//! word it says. It names no drawing type, neither ratatui's nor the kit's.
//! What of the terminal it does name is the line editor's: the name and
//! path fields are tui_input's, edited by the room's own keymap through
//! that crate's crossterm handler, so a key reaches them as the crossterm
//! event it came in as. The room is drawn by a child module that reads it
//! as any view would: `cells` (cells.rs), in character cells, for a
//! terminal and for the GUI's Admin tab; a view in pixels would be its
//! sibling. A click in a view is one of the room's [`Act`]s and a key is
//! [`Room::key`], so no view can disagree about what the room does. What
//! a view keeps between frames it keeps in a type of its own, held here
//! unopened (`cells::State`).

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ratatui::crossterm::event::Event as TermEvent;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Outcome, ServerHome, gate_message};
use crate::api::types::DirListing;
use crate::api::{ApiError, Client};
use crate::input::{KeyCode, KeyEvent};
use crate::setup::{
    Browse, PathDraft, common_prefix, derive_name, join_server_path, parent_server_path, picker,
    split_input,
};

/// The room in character cells: the terminal's room and the GUI's Admin tab.
pub(crate) mod cells;

/// Where a server-side browse starts: the server user's home, which the
/// admin file explorer spells `~` and resolves itself.
const SERVER_HOME: &str = "~";

// ── State ────────────────────────────────────────────────────────────────────

/// One library folder as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lib {
    /// The vpath — the short name apps see, permanent once set.
    pub name: String,
    /// The folder as the SERVER sees it.
    pub root: String,
    pub follow_symlinks: bool,
}

/// The Name modal: a chosen folder waiting for its vpath.
#[derive(Debug, Clone, Default)]
pub(crate) struct NameDraft {
    pub directory: String,
    /// The line editor, prefilled with a slug from the folder's leaf. The
    /// vpath charset (a-z 0-9 dash) is enforced at the key gate.
    pub name: Input,
    /// Why the last attempt was refused — ours ("pick a name") or the
    /// server's words — shown in the modal, in error gold.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum Modal {
    None,
    /// The server-side directory browser (the no-flag browse, and the
    /// fallback when no native dialog can open).
    Browser(Browse),
    /// Typing an absolute server path by hand, with server-fed completion.
    PathEntry(PathDraft),
    /// A chosen folder waiting for its name.
    Name(NameDraft),
    /// The warning gate before removing the library at this row.
    Remove(usize),
}

/// Everything a click can mean. Rebuilt into a rect registry every draw;
/// the last-drawn rect wins, which is what puts modals above the room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Act {
    /// The add card, and `b`: the OS dialog with `--same-machine`, the
    /// server browser without.
    Add,
    TypePath,
    ToggleSymlinks(usize),
    RemoveAt(usize),
    RemoveConfirm,
    RemoveCancel,
    TableScroll(i8),
    TableScrollTo(usize),
    BrowseRow(usize),
    BrowseEnter,
    BrowseUp,
    BrowseAdd,
    BrowseCancel,
    PathSuggest(usize),
    PathCancel,
    PathScroll(i8),
    PathScrollTo(usize),
    NameCancel,
    NameAdd,
    Quit,
}

/// A server call queued from input handling and run right after the next
/// draw, so its "working…" note is actually visible while it blocks. Ops
/// carry everything they need — the worker never sees the room.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Load,
    PickNative,
    OpenBrowser(String),
    BrowseTo(String),
    /// List a directory for the type-a-path modal's suggestions. Quiet:
    /// no busy note, and a failure just leaves the list empty.
    Complete(String),
    Add { directory: String, vpath: String },
    Remove(String),
    Symlinks { vpath: String, on: bool },
}

/// What the worker sends back for each [`Op`].
enum Done {
    Loaded(Result<Vec<Lib>, ApiError>),
    Picked(picker::Pick),
    Browsed(Result<DirListing, String>),
    Completed { dir: String, listing: Result<DirListing, String> },
    Added { vpath: String, result: Result<(), ApiError> },
    Removed { vpath: String, result: Result<(), ApiError> },
    Symlinks { vpath: String, on: bool, result: Result<(), ApiError> },
}

/// The worker thread: one op at a time, results back over the channel. The
/// picker runs here too, so the UI stays live while a dialog is open.
fn spawn_worker() -> (Sender<(Arc<Client>, Op)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Op)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        // A typed `~` is the server user's home: learned once, expanded
        // here before the listing and the add — the server resolves only
        // the bare form, and only in its file explorer.
        let mut home = ServerHome::new();
        while let Ok((client, op)) = job_rx.recv() {
            let done = match op {
                Op::Load => Done::Loaded(client.admin_directories().map(|dirs| {
                    dirs.into_iter()
                        .map(|(name, d)| Lib {
                            name,
                            root: d.root,
                            follow_symlinks: d.follow_symlinks,
                        })
                        .collect()
                })),
                Op::PickNative => Done::Picked(picker::pick_folder()),
                Op::OpenBrowser(path) | Op::BrowseTo(path) => {
                    Done::Browsed(client.admin_file_explorer(&path).map_err(|e| e.to_string()))
                }
                Op::Complete(dir) => {
                    let listing = home
                        .expand(&client, &dir)
                        .and_then(|dir| client.admin_file_explorer(&dir))
                        .map_err(|e| e.to_string());
                    Done::Completed { dir, listing }
                }
                Op::Add { directory, vpath } => {
                    let result = home
                        .expand(&client, &directory)
                        .and_then(|directory| client.admin_add_directory(&directory, &vpath))
                        .map(|_| ());
                    Done::Added { vpath, result }
                }
                Op::Remove(vpath) => {
                    let result = client.admin_remove_directory(&vpath).map(|_| ());
                    Done::Removed { vpath, result }
                }
                Op::Symlinks { vpath, on } => {
                    let result = client.admin_set_follow_symlinks(&vpath, on).map(|_| ());
                    Done::Symlinks { vpath, on, result }
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
    /// Booted with `--same-machine`: the OS folder dialog is the add
    /// affordance, and its paths are the server's paths.
    same_machine: bool,
    pub libs: Vec<Lib>,
    /// The KEYBOARD cursor — `None` until ↑/↓ is pressed. Mouse users
    /// never need it: every row action is directly clickable.
    pub sel: Option<usize>,
    /// A row to scroll into view on the next draw.
    reveal: Option<usize>,
    /// A freshly added library to reveal once the reload lists it.
    reveal_name: Option<String>,
    pub modal: Modal,
    /// One line of status above the tips: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    /// An op is running on the worker; further queues wait (completions
    /// supersede each other instead).
    in_flight: bool,
    pending_complete: Option<String>,
    /// The table's first visible row (wheel-scrollable).
    tscroll: usize,
    /// What the cells view keeps here between frames (cells.rs): its
    /// surface, and the cursor the table last followed.
    cells: cells::State,
}

impl Room {
    pub(super) fn new(client: Client, same_machine: bool) -> Self {
        let (to_worker, from_worker) = spawn_worker();
        Room {
            client: Arc::new(client),
            to_worker,
            from_worker,
            same_machine,
            libs: Vec::new(),
            sel: None,
            reveal: None,
            reveal_name: None,
            modal: Modal::None,
            note: None,
            busy: None,
            queued: None,
            in_flight: false,
            pending_complete: None,
            tscroll: 0,
            cells: cells::State::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    /// A key press: the room's keymap, the same under any view.
    pub(crate) fn key(&mut self, key: KeyEvent) -> Option<Outcome> {
        handle_key(self, key)
    }

    /// A click, drag or hold a view resolved to one of the room's actions.
    pub(crate) fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::Add => self.start_add(),
            Act::TypePath => {
                self.modal = Modal::PathEntry(PathDraft::default());
                self.refresh_completion();
            }
            Act::ToggleSymlinks(i) => {
                if let Some(lib) = self.libs.get(i) {
                    let vpath = lib.name.clone();
                    let on = !lib.follow_symlinks;
                    self.queue(Op::Symlinks { vpath, on }, t!("admin.busy_saving"));
                }
            }
            Act::RemoveAt(i) => {
                if i < self.libs.len() {
                    self.modal = Modal::Remove(i);
                }
            }
            Act::RemoveConfirm => {
                if let Modal::Remove(i) = self.modal
                    && let Some(lib) = self.libs.get(i)
                {
                    let name = lib.name.clone();
                    self.modal = Modal::None;
                    self.queue(Op::Remove(name.clone()), t!("admin.busy_removing", name = name));
                }
            }
            Act::RemoveCancel | Act::NameCancel | Act::PathCancel | Act::BrowseCancel => {
                self.modal = Modal::None;
            }
            Act::TableScroll(delta) => {
                self.tscroll = if delta < 0 {
                    self.tscroll.saturating_sub(1)
                } else {
                    self.tscroll.saturating_add(1)
                };
            }
            Act::TableScrollTo(pos) => self.tscroll = pos,
            Act::BrowseRow(i) => {
                if let Modal::Browser(b) = &mut self.modal {
                    b.sel = i.min(b.dirs.len().saturating_sub(1));
                }
            }
            Act::BrowseEnter => {
                if let Modal::Browser(b) = &self.modal
                    && let Some(dir) = b.dirs.get(b.sel)
                {
                    let to = join_server_path(&b.path, dir);
                    self.queue(Op::BrowseTo(to), t!("busy.listing"));
                }
            }
            Act::BrowseUp => {
                if let Modal::Browser(b) = &self.modal
                    && let Some(parent) = parent_server_path(&b.path)
                {
                    self.queue(Op::BrowseTo(parent), t!("busy.listing"));
                }
            }
            Act::BrowseAdd => {
                if let Modal::Browser(b) = &self.modal {
                    let path = b.path.clone();
                    self.open_name(path);
                }
            }
            Act::PathSuggest(i) => self.accept_suggestion(i),
            Act::PathScroll(delta) => {
                if let Modal::PathEntry(draft) = &mut self.modal {
                    draft.scroll = if delta < 0 {
                        draft.scroll.saturating_sub(1)
                    } else {
                        draft.scroll.saturating_add(1)
                    };
                }
            }
            Act::PathScrollTo(pos) => {
                if let Modal::PathEntry(draft) = &mut self.modal {
                    draft.scroll = pos;
                }
            }
            Act::NameAdd => self.submit_name(),
            Act::Quit => return Some(Outcome::Quit),
        }
        None
    }

    /// The add affordance: the OS dialog when this terminal shares the
    /// server's disk, the server's own browser otherwise.
    fn start_add(&mut self) {
        if self.same_machine {
            self.queue(Op::PickNative, t!("admin.busy_picker"));
        } else {
            self.queue(Op::OpenBrowser(SERVER_HOME.to_string()), t!("busy.listing"));
        }
    }

    /// A folder was chosen, whichever way: ask for its name. The draft is
    /// the folder's leaf as a slug — what the wizard would have called it.
    fn open_name(&mut self, directory: String) {
        let directory = directory.trim().to_string();
        if directory.is_empty() {
            return;
        }
        let name = derive_name(&directory);
        self.modal = Modal::Name(NameDraft { directory, name: Input::new(name), error: None });
        self.note = None;
    }

    /// Add the named folder — after the two refusals that need no server:
    /// an empty name, and a name another library already holds.
    fn submit_name(&mut self) {
        let libs = &self.libs;
        let Modal::Name(draft) = &mut self.modal else { return };
        let name = draft.name.value().trim().to_string();
        if name.is_empty() {
            draft.error = Some(t!("admin.name_empty").to_string());
            return;
        }
        if libs.iter().any(|l| l.name == name) {
            draft.error = Some(t!("admin.name_taken").to_string());
            return;
        }
        draft.error = None;
        let directory = draft.directory.clone();
        self.queue(
            Op::Add { directory, vpath: name.clone() },
            t!("admin.busy_adding", name = name),
        );
    }

    /// Queue a listing for the dir-part of the draft, if it changed. The
    /// server lists and resolves — the worker expands a leading `~` to the
    /// server user's home first — and the listing's own `path` is what
    /// accepted suggestions build on.
    fn refresh_completion(&mut self) {
        if let Modal::PathEntry(draft) = &mut self.modal {
            draft.sel = None;
            draft.sel_anchor = None;
            draft.scroll = 0;
            // An empty input suggests nothing — the completion starts once
            // there is something to complete.
            if draft.text.value().is_empty() {
                draft.listed_for.clear();
                draft.listed_path.clear();
                draft.entries.clear();
                draft.error = None;
                return;
            }
            // Collapse doubled separators — typing `/` right after a
            // completion that already ended with one is natural (a leading
            // pair survives for UNC paths).
            let raw = draft.text.value().to_string();
            let mut cleaned = String::with_capacity(raw.len());
            let mut prev_sep = false;
            for (i, ch) in raw.chars().enumerate() {
                let is_sep = ch == '/' || ch == '\\';
                if is_sep && prev_sep && i != 1 {
                    continue;
                }
                prev_sep = is_sep;
                cleaned.push(ch);
            }
            if cleaned != raw {
                let from_end = raw.chars().count() - draft.text.cursor();
                let cursor = cleaned.chars().count().saturating_sub(from_end);
                draft.text = Input::new(cleaned).with_cursor(cursor);
            }
            let (dir, _) = split_input(draft.text.value());
            if dir != draft.listed_for {
                draft.listed_for = dir.clone();
                draft.entries.clear();
                draft.error = None;
                // Quiet: no busy note for something that follows every
                // keystroke.
                self.queued = Some(Op::Complete(dir));
            }
        }
    }

    /// Take suggestion `i` as the next path segment and keep typing inside
    /// it — the accepted text becomes the server-resolved absolute path.
    fn accept_suggestion(&mut self, i: usize) {
        if let Modal::PathEntry(draft) = &mut self.modal {
            let picked = match draft.suggestion(i) {
                Some(entry) => entry.to_string(),
                None => return,
            };
            let base = if draft.listed_path.is_empty() {
                split_input(draft.text.value()).0
            } else {
                draft.listed_path.clone()
            };
            let joined = join_server_path(&base, &picked);
            let sep = if joined.contains('\\') { '\\' } else { '/' };
            draft.text = Input::new(format!("{joined}{sep}"));
            self.refresh_completion();
        }
    }

    // ── Server calls ────────────────────────────────────────────────────────

    /// Hand the queued op to the worker. Ops are single-flight: while one is
    /// in flight the UI shows its busy note and further queues wait
    /// (completion listings replace instead — typing outruns the network).
    fn dispatch_queued(&mut self) {
        if self.in_flight {
            match self.queued.take() {
                Some(Op::Complete(dir)) => self.pending_complete = Some(dir),
                other => self.queued = other,
            }
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
        if let Some(dir) = self.pending_complete.take() {
            self.queue(Op::Complete(dir), "");
        }
        match done {
            Done::Loaded(Ok(libs)) => {
                self.libs = libs;
                self.note = None;
                // The cursor stays a row that exists.
                self.sel = match self.sel {
                    Some(s) if !self.libs.is_empty() => Some(s.min(self.libs.len() - 1)),
                    _ => None,
                };
                // A fresh add scrolls its row into view but does not select
                // it (the kit's rule).
                if let Some(name) = self.reveal_name.take() {
                    self.reveal = self.libs.iter().position(|l| l.name == name);
                }
            }
            Done::Loaded(Err(e)) => {
                self.note = Some((gate_message(&e, &t!("admin.load_failed")), true))
            }
            Done::Picked(picker::Pick::Folder(path)) => {
                self.open_name(path.display().to_string());
            }
            Done::Picked(picker::Pick::Cancelled | picker::Pick::File(_)) => {}
            Done::Picked(picker::Pick::Unavailable(why)) => {
                self.note = Some((t!("note.no_picker", why = why).to_string(), false));
                self.queue(Op::OpenBrowser(SERVER_HOME.to_string()), t!("busy.listing"));
            }
            Done::Browsed(Ok(listing)) => {
                self.modal = Modal::Browser(Browse {
                    path: listing.path,
                    dirs: listing.directories.into_iter().map(|d| d.name).collect(),
                    sel: 0,
                });
            }
            Done::Browsed(Err(e)) => {
                self.note = Some((t!("note.browse_failed", err = e).to_string(), true));
            }
            Done::Completed { dir, listing } => {
                // The user may have typed on: only install the result if it
                // still answers the draft's current dir-part. Mid-typing
                // dirs are bogus half the time, so their failures stay
                // quiet — but a failure for the CURRENT dir is said in the
                // modal (silent emptiness reads as "no autocomplete").
                if let Modal::PathEntry(draft) = &mut self.modal
                    && draft.listed_for == dir
                {
                    match listing {
                        Ok(listing) => {
                            draft.error = None;
                            draft.listed_path = listing.path;
                            draft.entries =
                                listing.directories.into_iter().map(|d| d.name).collect();
                        }
                        Err(e) => {
                            draft.entries.clear();
                            let shown = if dir.is_empty() { "that folder" } else { &dir };
                            draft.error = Some(format!("could not list {shown}: {e}"));
                        }
                    }
                }
            }
            Done::Added { vpath, result: Ok(()) } => {
                // The server queued the folder's scan on its own; the
                // reload shows the row where the server lists it.
                self.modal = Modal::None;
                self.note = None;
                self.reveal_name = Some(vpath);
                self.queue(Op::Load, t!("admin.busy_loading"));
            }
            Done::Added { vpath, result: Err(e) } => {
                // A refusal keeps the modal open, the server's words in it.
                let what = t!("admin.add_failed", name = vpath).to_string();
                match &mut self.modal {
                    Modal::Name(draft) => draft.error = Some(format!("{what}: {e}")),
                    _ => self.fail(&what, e),
                }
            }
            Done::Removed { result: Ok(()), .. } => {
                self.note = None;
                self.queue(Op::Load, t!("admin.busy_loading"));
            }
            Done::Removed { vpath, result: Err(e) } => {
                self.fail(&t!("admin.remove_failed", name = vpath), e);
            }
            Done::Symlinks { vpath, on, result: Ok(()) } => {
                if let Some(lib) = self.libs.iter_mut().find(|l| l.name == vpath) {
                    lib.follow_symlinks = on;
                }
            }
            Done::Symlinks { vpath, result: Err(e), .. } => {
                self.fail(&t!("admin.symlinks_failed", name = vpath), e);
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
}

/// The room, loading: what `mstream-player admin` opens.
pub(super) fn start(client: Client, same_machine: bool) -> Room {
    let mut room = Room::new(client, same_machine);
    room.queue(Op::Load, t!("admin.busy_loading"));
    room
}

/// Tab (or Right from the end of the line): accept the picked suggestion,
/// the single match, or extend to the longest common prefix — and when
/// that gains nothing, start cycling.
fn complete_path(room: &mut Room) {
    let Modal::PathEntry(draft) = &mut room.modal else { return };
    let suggestions: Vec<&str> = draft.suggestions().collect();
    if let Some(i) = draft.sel {
        room.accept_suggestion(i);
    } else if suggestions.len() == 1 {
        room.accept_suggestion(0);
    } else if !suggestions.is_empty() {
        let (_, partial) = split_input(draft.text.value());
        let lcp = common_prefix(&suggestions);
        if lcp.chars().count() > partial.chars().count() {
            let keep = draft.text.value().chars().count() - partial.chars().count();
            let extended = draft.text.value().chars().take(keep).collect::<String>() + &lcp;
            draft.text = Input::new(extended);
            draft.sel = None;
        } else {
            draft.sel = Some(0);
        }
    }
}

fn handle_key(room: &mut Room, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    match &mut room.modal {
        Modal::PathEntry(draft) => {
            let at_end = draft.text.cursor() == draft.text.value().chars().count();
            match code {
                KeyCode::Esc => room.modal = Modal::None,
                KeyCode::Enter => {
                    let path = draft.text.value().trim().to_string();
                    room.modal = Modal::None;
                    room.open_name(path);
                }
                KeyCode::Down => {
                    let n = draft.suggestion_count();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(0, |i| (i + 1) % n));
                    }
                }
                KeyCode::Up | KeyCode::BackTab => {
                    let n = draft.suggestion_count();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(n - 1, |i| (i + n - 1) % n));
                    }
                }
                KeyCode::Tab => complete_path(room),
                KeyCode::Right if at_end => complete_path(room),
                _ => {
                    if draft
                        .text
                        .handle_event(&TermEvent::Key(key))
                        .is_some_and(|change| change.value)
                    {
                        room.refresh_completion();
                    }
                }
            }
            return None;
        }
        Modal::Browser(b) => {
            return match code {
                KeyCode::Esc => room.act(Act::BrowseCancel),
                KeyCode::Up => {
                    b.sel = b.sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    b.sel = (b.sel + 1).min(b.dirs.len().saturating_sub(1));
                    None
                }
                KeyCode::Enter | KeyCode::Right => room.act(Act::BrowseEnter),
                KeyCode::Left | KeyCode::Backspace => room.act(Act::BrowseUp),
                KeyCode::Char('a') => room.act(Act::BrowseAdd),
                _ => None,
            };
        }
        Modal::Name(draft) => {
            return match code {
                KeyCode::Esc => room.act(Act::NameCancel),
                KeyCode::Enter => room.act(Act::NameAdd),
                // The vpath charset (a-z 0-9 dash) is enforced at the gate:
                // legal characters fold to lowercase and reach the editor,
                // everything else typed is dropped. Non-character keys —
                // ←/→, Home/End, Backspace, Delete, the ctrl word ops —
                // pass straight through.
                KeyCode::Char(c) if !c.is_ascii_alphanumeric() && c != '-' => None,
                _ => {
                    let key = match code {
                        KeyCode::Char(c) => {
                            KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), key.modifiers)
                        }
                        _ => key,
                    };
                    draft.name.handle_event(&TermEvent::Key(key));
                    draft.error = None;
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

    match code {
        // ↑/↓ are the ONLY way a row gets highlighted: the first press
        // picks up the cursor (↓ from the top, ↑ from the bottom), Esc
        // puts it away again — and, with nothing selected, leaves.
        KeyCode::Up => {
            let n = room.libs.len();
            if n > 0 {
                room.sel = Some(room.sel.map_or(n - 1, |s| s.saturating_sub(1)));
            }
            None
        }
        KeyCode::Down => {
            let n = room.libs.len();
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
        KeyCode::Char('b') => room.act(Act::Add),
        KeyCode::Char('t') => room.act(Act::TypePath),
        KeyCode::Char('s') | KeyCode::Char(' ') => {
            room.sel.and_then(|s| room.act(Act::ToggleSymlinks(s)))
        }
        KeyCode::Char('r') | KeyCode::Delete => room.sel.and_then(|s| room.act(Act::RemoveAt(s))),
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
        Modal::Browser(_) => t!("hint.browser"),
        Modal::PathEntry(_) => t!("hint.path"),
        Modal::Name(_) => t!("admin.hint_name"),
        Modal::Remove(_) => t!("admin.hint_remove"),
        Modal::None => match (room.libs.is_empty(), room.sel, hosted) {
            (true, _, false) => t!("admin.hint_empty"),
            (true, _, true) => t!("admin.hint_empty_hosted"),
            (false, None, false) => t!("admin.hint_rows"),
            (false, None, true) => t!("admin.hint_rows_hosted"),
            (false, Some(_), _) => t!("admin.hint_selected"),
        },
    }
    .to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::DirEntry;
    use crate::input::KeyModifiers;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    pub(super) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    pub(super) fn room(same_machine: bool) -> Room {
        let client = Client::new("http://home.mstream.example:3000").expect("client");
        Room::new(client, same_machine)
    }

    pub(super) fn seeded() -> Room {
        let mut room = room(false);
        room.libs = vec![
            Lib { name: "music".into(), root: "/srv/music".into(), follow_symlinks: false },
            Lib {
                name: "vinyl".into(),
                root: "/Volumes/NAS/Vinyl Rips".into(),
                follow_symlinks: true,
            },
        ];
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

    /// The hub's order (performance audit #82): an answer is folded in
    /// before the draw, and the op a key queued meanwhile goes to the
    /// worker only in the pump after it — the frame between shows its
    /// busy note.
    #[test]
    fn absorb_folds_in_the_answer_and_leaves_the_next_call_to_the_pump() {
        let _en = english();
        let mut room = room(false);
        let (answer, answers) = std::sync::mpsc::channel();
        let (asks, asked) = std::sync::mpsc::channel();
        room.from_worker = answers;
        room.to_worker = asks;

        handle_key(&mut room, key(KeyCode::Char('b')));
        Room::pump(&mut room);
        assert!(room.awaiting(), "the browse is out with the worker");
        assert!(asked.try_recv().is_ok());

        let music = Lib { name: "music".into(), root: "/srv/music".into(), follow_symlinks: false };
        answer.send(Done::Loaded(Ok(vec![music.clone()]))).unwrap();
        room.queue(Op::Load, "reloading");
        Room::absorb(&mut room);
        assert_eq!(room.libs, vec![music], "the answer is in before the frame is drawn");
        assert!(!room.awaiting());
        assert_eq!(room.queued, Some(Op::Load), "absorbing never hands the worker the next call");
        assert!(asked.try_recv().is_err());

        Room::pump(&mut room);
        assert!(room.awaiting());
        assert!(matches!(asked.try_recv(), Ok((_, Op::Load))), "the pump does");
    }

    #[test]
    fn b_browses_the_server_unless_this_is_the_servers_machine() {
        let _en = english();
        let mut remote = room(false);
        handle_key(&mut remote, key(KeyCode::Char('b')));
        assert_eq!(remote.queued, Some(Op::OpenBrowser("~".into())));
        let mut local = room(true);
        handle_key(&mut local, key(KeyCode::Char('b')));
        assert_eq!(local.queued, Some(Op::PickNative));
        // No dialog on this machine: the room says so and falls through to
        // the server's browser, like the wizard.
        local.apply(Done::Picked(picker::Pick::Unavailable("no session bus".into())));
        assert!(local.note.as_ref().is_some_and(|(n, _)| n.contains("no session bus")));
        assert_eq!(local.queued, Some(Op::OpenBrowser("~".into())));
    }

    #[test]
    fn a_picked_folder_asks_for_its_name_and_the_gate_keeps_it_a_slug() {
        let _en = english();
        let mut room = room(true);
        room.apply(Done::Picked(picker::Pick::Folder("/Volumes/NAS/Field Recordings 2024".into())));
        let Modal::Name(draft) = &room.modal else { panic!("the Name modal") };
        assert_eq!(draft.directory, "/Volumes/NAS/Field Recordings 2024");
        assert_eq!(draft.name.value(), "field-recordings-2024");
        // Uppercase folds, spaces and dots are dropped, dashes pass.
        type_text(&mut room, "X y.-Z");
        let Modal::Name(draft) = &room.modal else { panic!("still the Name modal") };
        assert_eq!(draft.name.value(), "field-recordings-2024xy-z");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(
            room.queued,
            Some(Op::Add {
                directory: "/Volumes/NAS/Field Recordings 2024".into(),
                vpath: "field-recordings-2024xy-z".into(),
            })
        );
    }

    #[test]
    fn empty_and_taken_names_are_refused_before_the_server_sees_them() {
        let _en = english();
        let mut room = seeded();
        room.open_name("/mnt/music".into());
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Name(draft) = &room.modal else { panic!("the Name modal") };
        assert_eq!(draft.error.as_deref(), Some("already used by another library"));
        assert!(room.queued.is_none());
        for _ in 0..5 {
            handle_key(&mut room, key(KeyCode::Backspace));
        }
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Name(draft) = &room.modal else { panic!("the Name modal") };
        assert_eq!(draft.error.as_deref(), Some("pick a name"));
        assert!(room.queued.is_none());
    }

    #[test]
    fn adding_reloads_and_reveals_the_row_while_a_refusal_keeps_the_modal() {
        let _en = english();
        let mut room = seeded();
        room.open_name("/mnt/podcasts".into());
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.queued, Some(Op::Add { .. })));
        room.queued = None;
        room.apply(Done::Added {
            vpath: "podcasts".into(),
            result: Err(ApiError::Server { status: 500, message: "EACCES".into() }),
        });
        let Modal::Name(draft) = &room.modal else { panic!("a refusal keeps the modal") };
        assert!(draft.error.as_deref().is_some_and(|e| e.contains("EACCES")), "{:?}", draft.error);
        room.apply(Done::Added { vpath: "podcasts".into(), result: Ok(()) });
        assert!(matches!(room.modal, Modal::None));
        assert_eq!(room.queued, Some(Op::Load));
        let mut libs = room.libs.clone();
        libs.push(Lib { name: "podcasts".into(), root: "/mnt/podcasts".into(), follow_symlinks: false });
        room.apply(Done::Loaded(Ok(libs)));
        assert_eq!(room.reveal, Some(2), "the new row is revealed, not selected");
        assert_eq!(room.sel, None);
    }

    #[test]
    fn remove_is_gated_and_symlinks_toggle_from_the_cursor_row() {
        let _en = english();
        let mut room = seeded();
        // Nothing selected: r does nothing, Esc leaves.
        assert!(handle_key(&mut room, key(KeyCode::Char('r'))).is_none());
        assert!(matches!(room.modal, Modal::None));
        assert!(matches!(handle_key(&mut room, key(KeyCode::Esc)), Some(Outcome::Quit)));
        handle_key(&mut room, key(KeyCode::Down));
        assert_eq!(room.sel, Some(0));
        handle_key(&mut room, key(KeyCode::Char('r')));
        assert!(matches!(room.modal, Modal::Remove(0)));
        // Enter is the safe choice.
        handle_key(&mut room, key(KeyCode::Enter));
        assert!(matches!(room.modal, Modal::None));
        assert!(room.queued.is_none());
        handle_key(&mut room, key(KeyCode::Char('r')));
        handle_key(&mut room, key(KeyCode::Char('y')));
        assert_eq!(room.queued, Some(Op::Remove("music".into())));
        room.queued = None;
        room.apply(Done::Removed { vpath: "music".into(), result: Ok(()) });
        assert_eq!(room.queued, Some(Op::Load));
        room.queued = None;
        // s flips the cursor row's flag; the row wears it once the server
        // agrees, and only then.
        handle_key(&mut room, key(KeyCode::Char('s')));
        assert_eq!(room.queued, Some(Op::Symlinks { vpath: "music".into(), on: true }));
        assert!(!room.libs[0].follow_symlinks);
        room.apply(Done::Symlinks { vpath: "music".into(), on: true, result: Ok(()) });
        assert!(room.libs[0].follow_symlinks);
        // Esc stows the cursor; a second Esc leaves.
        assert!(handle_key(&mut room, key(KeyCode::Esc)).is_none());
        assert_eq!(room.sel, None);
    }

    #[test]
    fn the_path_modal_completes_against_the_server_and_ends_in_the_name_modal() {
        let _en = english();
        let mut room = room(false);
        handle_key(&mut room, key(KeyCode::Char('t')));
        assert!(matches!(room.modal, Modal::PathEntry(_)));
        assert!(room.queued.is_none(), "an empty input suggests nothing");
        type_text(&mut room, "/srv/m");
        assert_eq!(room.queued, Some(Op::Complete("/srv/".into())));
        room.queued = None;
        room.apply(Done::Completed {
            dir: "/srv/".into(),
            listing: Ok(DirListing {
                path: "/srv".into(),
                directories: vec![
                    DirEntry { name: "media".into() },
                    DirEntry { name: "music".into() },
                    DirEntry { name: "www".into() },
                ],
                files: Vec::new(),
            }),
        });
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.suggestions().collect::<Vec<_>>(), vec!["media", "music"]);
        assert_eq!(draft.suggestion_count(), 2);
        assert_eq!(draft.suggestion(1), Some("music"));
        // Tab cannot extend past the common prefix, so it starts cycling;
        // Enter takes the text as typed.
        handle_key(&mut room, key(KeyCode::Tab));
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.sel, Some(0));
        handle_key(&mut room, key(KeyCode::Tab));
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.text.value(), "/srv/media/");
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Name(draft) = &room.modal else { panic!("the Name modal") };
        assert_eq!(draft.directory, "/srv/media/");
        assert_eq!(draft.name.value(), "media");
    }

    #[test]
    fn a_typed_tilde_goes_out_as_typed_and_completes_to_the_servers_home() {
        let _en = english();
        let mut room = room(false);
        handle_key(&mut room, key(KeyCode::Char('t')));
        // The dir-part leaves as typed: the worker expands `~` once the
        // server has said where it is, and the listing comes back resolved.
        type_text(&mut room, "~/mu");
        assert_eq!(room.queued, Some(Op::Complete("~/".into())));
        room.queued = None;
        room.apply(Done::Completed {
            dir: "~/".into(),
            listing: Ok(DirListing {
                path: "/home/sherika".into(),
                directories: vec![DirEntry { name: "Music".into() }, DirEntry { name: "Videos".into() }],
                files: Vec::new(),
            }),
        });
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.suggestions().collect::<Vec<_>>(), vec!["Music"]);
        // Accepting builds on the resolved home: the field turns absolute.
        handle_key(&mut room, key(KeyCode::Tab));
        let Modal::PathEntry(draft) = &room.modal else { panic!("the path modal") };
        assert_eq!(draft.text.value(), "/home/sherika/Music/");
        assert_eq!(room.queued, Some(Op::Complete("/home/sherika/Music/".into())));
        room.queued = None;
        // Enter with the tilde still in the field keeps it for the add —
        // the worker expands that the same way.
        handle_key(&mut room, key(KeyCode::Esc));
        handle_key(&mut room, key(KeyCode::Char('t')));
        type_text(&mut room, "~/Music");
        room.queued = None;
        handle_key(&mut room, key(KeyCode::Enter));
        let Modal::Name(draft) = &room.modal else { panic!("the Name modal") };
        assert_eq!(draft.directory, "~/Music");
        assert_eq!(draft.name.value(), "music", "the leaf, as a slug");
        handle_key(&mut room, key(KeyCode::Enter));
        assert_eq!(room.queued, Some(Op::Add { directory: "~/Music".into(), vpath: "music".into() }));
    }

    #[test]
    fn load_errors_name_the_gate_that_bit() {
        let _en = english();
        let mut room = room(false);
        room.apply(Done::Loaded(Err(ApiError::Unauthorized)));
        assert!(room.note.as_ref().is_some_and(|(n, e)| *e && n.contains("login")));
        room.apply(Done::Loaded(Err(ApiError::Forbidden("ip not allowed".into()))));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("admin") && n.contains("ip not allowed")));
        room.apply(Done::Loaded(Err(ApiError::Server { status: 405, message: String::new() })));
        assert!(room.note.as_ref().is_some_and(|(n, _)| n.contains("locked")));
    }
}
