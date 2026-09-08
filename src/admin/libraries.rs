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
//! ([`super::run_tui`]); this file is what the [`Screen`] trait asks of a
//! room.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::{Outcome, Screen, draw_bottom, draw_header, frame_ground, gate_message, host_of};
use crate::api::types::DirListing;
use crate::api::{ApiError, Client};
use crate::kit::theme::th;
use crate::kit::{self, Surface, accent, bold, dim};
use crate::setup::{
    Browse, PathDraft, common_prefix, derive_name, ellipsize_path_start, g, join_server_path,
    parent_server_path, picker, split_input,
};

/// Where a server-side browse starts: the server user's home, which the
/// admin file explorer spells `~` and resolves itself.
const SERVER_HOME: &str = "~";
/// The table's NAME column, the wizard's width — the vpath is the point.
const NAME_W: u16 = 16;
/// The ` [X]` column, outside the selection area.
const REMOVE_W: u16 = 4;
/// The SYMLINKS column: the header word, and a `[✓]`/`[ ]` cell under it.
const SYM_W: u16 = 8;

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
                    let listing = client.admin_file_explorer(&dir).map_err(|e| e.to_string());
                    Done::Completed { dir, listing }
                }
                Op::Add { directory, vpath } => {
                    let result = client.admin_add_directory(&directory, &vpath).map(|_| ());
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
    /// Last frame's selection — a change yanks the view to the selection.
    sel_anchor: Option<usize>,
    /// The kit's interaction surface: click/tip/bar registries, pointer,
    /// tooltip dwell, scrollbar capture and hold-repeat.
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
            sel_anchor: None,
            ui: Surface::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
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
    /// server lists and resolves: `~` is the server user's home, and the
    /// listing's own `path` is what accepted suggestions build on.
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
            let picked = match draft.suggestions().get(i) {
                Some(entry) => entry.clone(),
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
            Done::Picked(picker::Pick::Cancelled) => {}
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
        match &mut self.modal {
            Modal::PathEntry(draft) => {
                draft.scroll =
                    if up { draft.scroll.saturating_sub(1) } else { draft.scroll.saturating_add(1) };
            }
            Modal::None => {
                self.tscroll =
                    if up { self.tscroll.saturating_sub(1) } else { self.tscroll.saturating_add(1) };
            }
            _ => {}
        }
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
    let suggestions = draft.suggestions();
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
                    let n = draft.suggestions().len();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(0, |i| (i + 1) % n));
                    }
                }
                KeyCode::Up | KeyCode::BackTab => {
                    let n = draft.suggestions().len();
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

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, room: &mut Room) {
    room.ui.begin_frame();
    let Some(area) = frame_ground(frame, 58, 12) else { return };

    // A modal makes the room beneath INERT: the base draw sees no pointer,
    // and every rect it registered is dropped before the modal draws.
    let modal_open = !matches!(room.modal, Modal::None);
    let live_pointer = room.ui.pointer;
    if modal_open {
        room.ui.pointer = None;
    }

    draw_header(frame, area, &t!("admin.title"), &host_of(&room.client));

    // Full display: two-cell margins, the whole height above the two
    // bottom lines (a table room, not a form — the kit's 74-cell column
    // is deliberately not applied).
    let column = Rect {
        x: 2,
        y: 2,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(5),
    };
    draw_libraries(frame, room, column);

    draw_bottom(frame, area, room.note.as_ref(), room.busy.as_deref(), &footer_hint(room));

    if modal_open {
        room.ui.pointer = live_pointer;
        room.ui.clear_registries();
    }
    match room.modal.clone() {
        Modal::None => {}
        Modal::Browser(browse) => draw_browser(frame, room, area, &browse),
        Modal::PathEntry(draft) => draw_path_entry(frame, room, area, &draft),
        Modal::Name(draft) => draw_name(frame, room, area, &draft),
        Modal::Remove(i) => draw_remove(frame, room, area, i),
    }

    // The tooltip draws last — over everything, once the dwell matures.
    if let Some((target, text)) = room.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

fn footer_hint(room: &Room) -> String {
    match &room.modal {
        Modal::Browser(_) => t!("hint.browser"),
        Modal::PathEntry(_) => t!("hint.path"),
        Modal::Name(_) => t!("admin.hint_name"),
        Modal::Remove(_) => t!("admin.hint_remove"),
        Modal::None => match (room.libs.is_empty(), room.sel) {
            (true, _) => t!("admin.hint_empty"),
            (false, None) => t!("admin.hint_rows"),
            (false, Some(_)) => t!("admin.hint_selected"),
        },
    }
    .to_string()
}

/// The room's body: the add card, then the table.
fn draw_libraries(frame: &mut Frame, room: &mut Room, column: Rect) {
    let mut y = column.y;

    // The add card — the screen's one add affordance (typing a path is the
    // `t` shortcut, in the tips line) — sits ABOVE the table so it holds
    // one spot as libraries come and go. Green: the affirmative add.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = room.ui.pointer.is_some_and(|p| add_rect.contains(p));
    let add_color = if add_hover { th().bright } else { th().ok };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(add_color));
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("folders.add_card").to_string(),
            Style::default().fg(add_color).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        inner,
    );
    room.ui.click(add_rect, Act::Add);
    room.ui.tip(
        add_rect,
        if room.same_machine { t!("admin.tip_add_native") } else { t!("admin.tip_add_server") },
    );
    y += 4;

    // The table: NAME first — the vpath is the point. The [X] remove
    // control sits to the RIGHT of the selection area, not inside it.
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let header = Rect { x: column.x, y, width: sel_width, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{:<width$}", t!("folders.col_name"), width = NAME_W as usize),
                dim(),
            ),
            Span::styled(t!("folders.col_folder").to_string(), dim()),
        ])),
        header,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.col_symlinks").to_string(), dim()))
            .alignment(Alignment::Right),
        header,
    );
    y += 1;
    // The rule spans the FULL row — selection area and the [X] column.
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(column.width as usize), dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;

    // The table frame is ALWAYS on screen; an empty list says so where
    // the first row would be.
    if room.libs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("folders.nothing_yet").to_string(), dim())),
            Rect { x: column.x, y, width: sel_width, height: 1 },
        );
        return;
    }
    let avail = (column.y + column.height).saturating_sub(y) as usize;
    let sel_moved = room.sel != room.sel_anchor;
    room.sel_anchor = room.sel;
    let reveal = room.reveal.take().or(if sel_moved { room.sel } else { None });
    let (first, visible) = kit::table_view(room.libs.len(), reveal, room.tscroll, avail);
    room.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    let path_w = sel_width.saturating_sub(NAME_W + SYM_W + 2);
    for i in first..first + visible {
        let selected = room.sel == Some(i);
        let (name, root, follow) = {
            let lib = &room.libs[i];
            (lib.name.clone(), lib.root.clone(), lib.follow_symlinks)
        };
        let row_bg = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let rect = Rect { x: column.x, y, width: sel_width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(" ".repeat(sel_width as usize), row_bg)),
            rect,
        );
        // Names are plain text, not chips: the server has no rename, so
        // nothing here pretends to offer one.
        let name_style = if selected { row_bg.add_modifier(Modifier::BOLD) } else { Style::default() };
        frame.render_widget(
            Paragraph::new(Span::styled(name, name_style)),
            Rect { x: column.x, y, width: NAME_W.min(sel_width), height: 1 },
        );
        // Long paths keep their LEAF visible: a leading … and the tail. The
        // full path rides the tooltip whenever the cell clipped.
        let path_rect = Rect { x: column.x + NAME_W, y, width: path_w, height: 1 };
        let (shown, clipped) = ellipsize_path_start(&root, path_rect.width as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, row_bg)), path_rect);
        if clipped {
            let (tip, _) = ellipsize_path_start(&root, kit::TIP_WRAP * 3);
            room.ui.tip(path_rect, tip);
        }
        // The SYMLINKS cell: a checkbox glyph wearing its state, clickable
        // like every other control, `s` on the cursor row by key.
        let sym_rect = Rect { x: column.x + sel_width - SYM_W, y, width: SYM_W, height: 1 };
        let sym_hover = room.ui.pointer.is_some_and(|p| sym_rect.contains(p));
        let glyph = if follow { g("[✓]", "[x]") } else { "[ ]" };
        let sym_style = match (selected, sym_hover, follow) {
            (true, _, _) => row_bg,
            (false, true, _) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
            (false, false, true) => Style::default().fg(th().ok),
            (false, false, false) => dim(),
        };
        frame.render_widget(Paragraph::new(Span::styled(glyph, sym_style)), sym_rect);
        room.ui.click(sym_rect, Act::ToggleSymlinks(i));
        room.ui.tip(sym_rect, t!("admin.tip_symlinks"));
        // The [X]: DIM until the pointer arrives, then the destructive red.
        let x_rect = Rect { x: column.x + sel_width + 1, y, width: 3, height: 1 };
        let x_hover = room.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        room.ui.click(x_rect, Act::RemoveAt(i));
        room.ui.tip(x_rect, t!("admin.tip_remove"));
        y += 1;
    }

    // Overflow → the kit's scrollbar, in the margin cell right of the [X].
    let bar = Rect { x: column.x + column.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(
        frame,
        &mut room.ui,
        bar,
        room.libs.len(),
        visible,
        first,
        Act::TableScroll(-1),
        Act::TableScroll(1),
        Act::TableScrollTo,
    );
}

fn draw_browser(frame: &mut Frame, room: &mut Room, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame(frame, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("browse.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::BrowseCancel, t!("path_modal.tip_close"));
    frame.render_widget(
        Paragraph::new(Span::styled(browse.path.clone(), dim())),
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: 1 },
    );

    let list_top = inner.y + 3;
    let visible = inner.height.saturating_sub(5) as usize;
    let first = browse.sel.saturating_sub(visible.saturating_sub(1));
    for (row, i) in (first..browse.dirs.len().min(first + visible)).enumerate() {
        let selected = i == browse.sel;
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {}", g("▸", "►"), browse.dirs[i]), style)),
            rect,
        );
        room.ui.click(rect, Act::BrowseRow(i));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("browse.empty").to_string(), dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }

    let y = inner.bottom().saturating_sub(1);
    let row = |x| Rect { x, y, width: inner.width, height: 1 };
    let up = kit::button(frame, &mut room.ui, row(inner.x), &t!("browse.up"), false, Act::BrowseUp);
    let open = kit::button(
        frame,
        &mut room.ui,
        row(up.right() + 1),
        &t!("browse.open"),
        false,
        Act::BrowseEnter,
    );
    let add = kit::button(
        frame,
        &mut room.ui,
        row(open.right() + 1),
        &t!("browse.add"),
        true,
        Act::BrowseAdd,
    );
    kit::button(
        frame,
        &mut room.ui,
        row(add.right() + 1),
        &t!("browse.close"),
        false,
        Act::BrowseCancel,
    );
}

fn draw_path_entry(frame: &mut Frame, room: &mut Room, area: Rect, draft: &PathDraft) {
    let suggestions = draft.suggestions();
    let shown = suggestions.len().min(6) as u16;
    // Anchored as if always full: the title and input hold one spot and
    // the suggestion list grows DOWNWARD beneath them.
    let inner = kit::modal_frame_anchored(frame, area, 62, 7 + shown, 13, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("path_modal.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::PathCancel, t!("path_modal.tip_close"));
    frame.render_widget(
        Paragraph::new(Span::raw(kit::input_display(
            draft.text.value(),
            draft.text.cursor(),
            inner.width,
        ))),
        Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: 1 },
    );
    let sel_moved = draft.sel != draft.sel_anchor;
    let reveal = if sel_moved { draft.sel } else { None };
    let (first, visible) = kit::table_view(suggestions.len(), reveal, draft.scroll, 6);
    if let Modal::PathEntry(d) = &mut room.modal {
        d.scroll = first;
        d.sel_anchor = d.sel;
    }
    let overflow = suggestions.len() > visible;
    let row_width = if overflow { inner.width.saturating_sub(1) } else { inner.width };
    for (row, i) in (first..first + visible).enumerate() {
        let entry = &suggestions[i];
        let selected = draft.sel == Some(i);
        let rect = Rect { x: inner.x, y: inner.y + 4 + row as u16, width: row_width, height: 1 };
        let hovered = room.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} {entry}", g("▸", "►")), style)),
            rect,
        );
        room.ui.click(rect, Act::PathSuggest(i));
    }
    let bar = Rect {
        x: inner.x + inner.width.saturating_sub(1),
        y: inner.y + 4,
        width: 1,
        height: visible as u16,
    };
    kit::scroll_list(
        frame,
        &mut room.ui,
        bar,
        suggestions.len(),
        visible,
        first,
        Act::PathScroll(-1),
        Act::PathScroll(1),
        Act::PathScrollTo,
    );
    if let Some(err) = &draft.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            Rect { x: inner.x, y: inner.y + 4, width: inner.width, height: 1 },
        );
    }
}

/// The Name modal: the chosen folder, the kit's text input holding the
/// draft vpath, the one fact that matters in gold, and Add.
fn draw_name(frame: &mut Frame, room: &mut Room, area: Rect, draft: &NameDraft) {
    let inner = kit::modal_frame(frame, area, 68, 16, th().accent);
    let line = |y: u16| Rect { x: inner.x + 1, y, width: inner.width.saturating_sub(2), height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("admin.name_title").to_string(),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        )),
        line(inner.y),
    );
    kit::modal_close(frame, &mut room.ui, inner, Act::NameCancel, t!("path_modal.tip_close"));

    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_folder").to_string(), dim())),
        line(inner.y + 2),
    );
    let (shown, _) = ellipsize_path_start(&draft.directory, inner.width.saturating_sub(2) as usize);
    frame.render_widget(Paragraph::new(Span::raw(shown)), line(inner.y + 3));

    // The text input: label above, a 3-row Rounded card, focused border
    // with the caret after the value.
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_label").to_string(), dim())),
        line(inner.y + 5),
    );
    let field = Rect { x: inner.x + 1, y: inner.y + 6, width: 32.min(inner.width), height: 3 };
    let card = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(accent());
    let field_inner = card.inner(field);
    frame.render_widget(card, field);
    frame.render_widget(
        Paragraph::new(Span::raw(kit::input_display(
            draft.name.value(),
            draft.name.cursor(),
            field_inner.width.saturating_sub(1),
        ))),
        Rect { x: field_inner.x + 1, y: field_inner.y, width: field_inner.width.saturating_sub(1), height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_charset").to_string(), dim())),
        Rect {
            x: field.right() + 2,
            y: inner.y + 7,
            width: inner.width.saturating_sub(field.width + 3),
            height: 1,
        },
    );

    let gold = Style::default().fg(th().gold);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_permanent_1").to_string(), gold)),
        line(inner.y + 9),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(t!("admin.name_permanent_2").to_string(), gold)),
        line(inner.y + 10),
    );
    if let Some(err) = &draft.error {
        frame.render_widget(Paragraph::new(Span::styled(err.clone(), gold)), line(inner.y + 12));
    }

    let label = t!("admin.name_add").to_string();
    let x = inner.right().saturating_sub(label.chars().count() as u16 + 4);
    kit::button(
        frame,
        &mut room.ui,
        Rect { x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 },
        &label,
        true,
        Act::NameAdd,
    );
}

/// The warning gate before a remove: consequences before verbs, the safe
/// choice as the primary, no [X] — a gate forces an explicit choice.
fn draw_remove(frame: &mut Frame, room: &mut Room, area: Rect, i: usize) {
    let name = room.libs.get(i).map(|l| l.name.clone()).unwrap_or_default();
    let inner = kit::modal_frame(frame, area, 68, 11, th().gold);
    let gold = Style::default().fg(th().gold);
    let lines = vec![
        Line::from(Span::styled(
            t!("admin.remove_title", name = name).to_string(),
            gold.add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            t!("admin.remove_files_stay").to_string(),
            Style::default().fg(th().ok),
        )),
        Line::from(Span::styled(t!("admin.remove_users", name = name).to_string(), gold)),
        Line::from(Span::styled(t!("admin.remove_tracks").to_string(), gold)),
    ];
    // The body clips ABOVE the button row.
    let body = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height.saturating_sub(2),
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let y = inner.bottom().saturating_sub(1);
    let keep = t!("admin.remove_keep").to_string();
    let remove = t!("admin.remove_confirm").to_string();
    let remove_w = remove.chars().count() as u16 + 4;
    let keep_w = keep.chars().count() as u16 + 4;
    let keep_x = inner.right().saturating_sub(remove_w + 2 + keep_w);
    let keep_rect = kit::button(
        frame,
        &mut room.ui,
        Rect { x: keep_x, y, width: inner.width, height: 1 },
        &keep,
        true,
        Act::RemoveCancel,
    );
    kit::button(
        frame,
        &mut room.ui,
        Rect { x: keep_rect.right() + 2, y, width: inner.width, height: 1 },
        &remove,
        false,
        Act::RemoveConfirm,
    );
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::DirEntry;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    /// English strings under assertion: hold the wizard tests' locale lock
    /// (one of them flips the process-global locale) and pin English.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn room(same_machine: bool) -> Room {
        let client = Client::new("http://home.mstream.example:3000").expect("client");
        Room::new(client, same_machine)
    }

    fn seeded() -> Room {
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
        assert_eq!(draft.suggestions(), vec!["media".to_string(), "music".to_string()]);
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

    #[test]
    fn the_room_draws_the_kit_table_and_names_only_what_works() {
        let _en = english();
        let mut room = seeded();
        let frame = draw(&mut room);
        assert!(frame.contains("Libraries"), "{frame}");
        assert!(frame.contains("home.mstream.example · admin"), "{frame}");
        assert!(frame.contains("Add a music folder"), "{frame}");
        assert!(frame.contains("NAME") && frame.contains("FOLDER") && frame.contains("SYMLINKS"), "{frame}");
        assert!(frame.contains("music           /srv/music"), "{frame}");
        assert!(frame.contains("[✓]") && frame.contains("[ ]") && frame.contains("[X]"), "{frame}");
        assert!(frame.contains("↑ ↓ select · b browse · t type a path · q quit"), "{frame}");
        assert!(!frame.contains("Scan now") && !frame.contains("─────────────────────────────────────────────────────────────────────────────────────────────────"), "no bottom bar, no gold rule");
        handle_key(&mut room, key(KeyCode::Down));
        let frame = draw(&mut room);
        assert!(frame.contains("s symlinks · r remove · Esc deselect"), "{frame}");
        room.libs.clear();
        let frame = draw(&mut room);
        assert!(frame.contains("(nothing added yet)"), "{frame}");
        assert!(frame.contains("b browse · t type a path · q quit"), "{frame}");
    }

    #[test]
    fn the_name_and_remove_modals_draw_their_one_fact() {
        let _en = english();
        let mut room = seeded();
        room.open_name("/Volumes/NAS/Field Recordings 2024".into());
        let frame = draw(&mut room);
        assert!(frame.contains("Name this library"), "{frame}");
        assert!(frame.contains("field-recordings-2024"), "{frame}");
        assert!(frame.contains("can't be changed later"), "{frame}");
        assert!(frame.contains("Add folder ▸"), "{frame}");
        assert!(frame.contains("type to edit · Enter add · Esc cancel"), "{frame}");
        room.modal = Modal::Remove(1);
        let frame = draw(&mut room);
        assert!(frame.contains("Remove vinyl from mStream?"), "{frame}");
        assert!(frame.contains("stay on disk"), "{frame}");
        assert!(frame.contains("◂ Keep it") && frame.contains("Remove  "), "{frame}");
        assert!(frame.contains("y remove · Esc keep"), "{frame}");
    }
}
