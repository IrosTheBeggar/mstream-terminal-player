//! First-run setup wizard: `mstream-player setup`.
//!
//! Five screens against a freshly installed mStream server — folders, first
//! login, opt-ins, Quick Connect — driven entirely through the server's admin
//! API on the fresh install's zero-account window (every request is an
//! implicit admin until the wizard creates the first user, at which point it
//! logs in and continues with the token).
//!
//! The look is the "airy minimal" direction from the design canvas: one
//! centered column, sparse rounded borders, filled buttons that light up
//! under the pointer. Mouse-first — every control is clickable — and every action has
//! a key. All decisions live in [`Wizard`]; the loop below only draws,
//! reads input, and runs one queued server call per pass (queued so the
//! "working…" frame is on screen while the call blocks).

pub mod picker;

use std::time::{Duration, Instant};

use clap::Args;
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};

use crate::api::{ApiError, Client};
use crate::config;

/// How long to wait for input before redrawing anyway.
const POLL: Duration = Duration::from_millis(100);
/// How often the Done screen re-asks for scan progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(1500);
/// The one vpath name a single folder gets without being asked.
const SINGLE_NAME: &str = "media";
/// The widest the content column grows, in cells.
const COLUMN: u16 = 74;

#[derive(Args)]
pub struct SetupArgs {
    /// The server to configure — a fresh desktop install listens here
    #[arg(long, default_value = "http://localhost:3000")]
    server: String,

    /// Token for a server that already has accounts (testing)
    #[arg(long, hide = true)]
    token: Option<String>,
}

// ── Palette ──────────────────────────────────────────────────────────────────
//
// Named ANSI colors, never RGB: the wizard inherits the user's terminal
// theme like the player does, and these are the same families the player's
// own UI speaks (LightBlue/Cyan accents, DarkGray chrome).

const ACCENT: Color = Color::LightBlue;
const BRIGHT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const WARN: Color = Color::Yellow;
/// The 3b-modal gold, promoted to a fixture: the rule above the footer.
const GOLD: Color = Color::Yellow;
const OK: Color = Color::Green;

fn accent() -> Style {
    Style::default().fg(ACCENT)
}
fn dim() -> Style {
    Style::default().fg(DIM)
}
fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Folders,
    Login,
    Extras,
    Done,
}

impl Screen {
    /// The 1-of-4 step this screen is, for the footer.
    fn step(self) -> u8 {
        match self {
            Screen::Folders => 1,
            Screen::Login => 2,
            Screen::Extras => 3,
            Screen::Done => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Modal {
    None,
    /// The public-mode tradeoff, shown when Skip is chosen on Login.
    SkipWarning,
    /// The server-side directory browser (native picker unavailable, or
    /// chosen on purpose — it is the only browse that works over SSH).
    Browser(Browse),
    /// Typing an absolute path by hand, with server-fed tab completion.
    PathEntry(PathDraft),
}

/// The type-a-path modal's state: the text, plus a cached listing of the
/// directory the text currently sits in, from which suggestions derive.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PathDraft {
    pub text: String,
    /// The dir-part the cached entries belong to (as derived from the text).
    pub listed_for: String,
    /// The server-resolved absolute form of that dir (what accepts build on).
    pub listed_path: String,
    pub entries: Vec<String>,
    /// Keyboard cursor within the CURRENT suggestion list, if any.
    pub sel: Option<usize>,
}

impl PathDraft {
    /// The entries that match the current partial segment, in order.
    pub fn suggestions(&self) -> Vec<String> {
        let (_, partial) = split_input(&self.text);
        self.entries
            .iter()
            .filter(|e| starts_with_fold(e, &partial))
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Browse {
    /// The server-native absolute path currently listed.
    pub path: String,
    pub dirs: Vec<String>,
    pub sel: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Folder {
    pub path: String,
    pub name: String,
    /// A name the user typed survives resyncs; derived ones follow the
    /// one-folder-is-media rule as folders come and go.
    pub named_by_user: bool,
    /// Already PUT to the server (a failed batch retries only the rest).
    pub committed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginField {
    Username,
    Password,
    Confirm,
}

/// Everything a click can mean. Rebuilt into a rect registry every draw;
/// the last-drawn rect wins, which is what puts modals above screens.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Act {
    SelectFolder(usize),
    RenameFolder(usize),
    BrowseNative,
    TypePath,
    RemoveFolder,
    RemoveAt(usize),
    ContinueFolders,
    Focus(LoginField),
    CreateAdmin,
    SkipLogin,
    SkipConfirm,
    SkipCancel,
    Toggle(usize),
    ContinueExtras,
    BrowseRow(usize),
    BrowseEnter,
    BrowseUp,
    BrowseAdd,
    BrowseCancel,
    PathSuggest(usize),
    OpenPlayer,
    Finish,
}

/// A server call queued from input handling and run right after the next
/// draw, so its "working…" note is actually visible while it blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Ping,
    PickNative,
    OpenBrowser(String),
    BrowseTo(String),
    /// List a directory for the type-a-path modal's suggestions. Quiet:
    /// no busy note, and a failure just leaves the list empty.
    Complete(String),
    CommitFolders,
    CreateAdmin,
    CommitExtras,
    LoadDone,
    PollProgress,
}

/// How the loop ended.
enum Outcome {
    Quit,
    OpenPlayer,
}

pub(crate) struct Wizard {
    client: Client,
    pub screen: Screen,
    pub modal: Modal,

    pub folders: Vec<Folder>,
    pub sel: usize,
    /// A rename in progress on `folders[sel]`, holding the draft.
    pub editing: Option<String>,

    pub username: String,
    pub password: String,
    pub confirm: String,
    pub field: LoginField,
    /// Continue without an account — the public-mode path.
    pub public: bool,

    /// Extras toggles: automatic updates (on), server audio, discovery.
    pub extras: [bool; 3],
    pub extras_sel: usize,
    /// Which extras already reached the server (a failed batch retries the
    /// rest, and toggling one off after a failure just drops it).
    extras_done: [bool; 3],

    /// Quick Connect ticket, rendered; None while loading or when off.
    pub qr: Option<Vec<String>>,
    pub qr_note: String,
    pub progress: String,

    /// One line of status under the card: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<&'static str>,
    queued: Option<Op>,
    last_poll: Instant,

    clicks: Vec<(Rect, Act)>,
    /// Where the mouse last was, for hover styling. None until it moves.
    pointer: Option<Position>,
}

impl Wizard {
    fn new(client: Client) -> Self {
        Wizard {
            client,
            screen: Screen::Folders,
            modal: Modal::None,
            folders: Vec::new(),
            sel: 0,
            editing: None,
            username: String::new(),
            password: String::new(),
            confirm: String::new(),
            field: LoginField::Username,
            public: false,
            extras: [true, false, false],
            extras_sel: 0,
            extras_done: [false; 3],
            qr: None,
            qr_note: String::new(),
            progress: String::new(),
            note: None,
            busy: None,
            queued: None,
            last_poll: Instant::now(),
            clicks: Vec::new(),
            pointer: None,
        }
    }

    fn queue(&mut self, op: Op, busy: &'static str) {
        self.queued = Some(op);
        self.busy = Some(busy);
    }

    // ── Folder naming rules ─────────────────────────────────────────────────

    fn add_folder(&mut self, path: String) {
        if self.folders.iter().any(|f| f.path == path) {
            self.note = Some(("that folder is already in the list".to_string(), false));
            return;
        }
        self.folders.push(Folder {
            name: String::new(),
            path,
            named_by_user: false,
            committed: false,
        });
        self.sel = self.folders.len() - 1;
        self.note = None;
        sync_names(&mut self.folders);
    }

    fn remove_selected(&mut self) {
        if self.folders.is_empty() {
            return;
        }
        let removed = self.folders.remove(self.sel);
        if removed.committed {
            self.note = Some((
                format!("{} was already added to the server — remove it in the admin panel", removed.name),
                false,
            ));
        }
        self.sel = self.sel.min(self.folders.len().saturating_sub(1));
        sync_names(&mut self.folders);
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::SelectFolder(i) => {
                self.finish_rename();
                self.sel = i.min(self.folders.len().saturating_sub(1));
            }
            Act::RenameFolder(i) => {
                self.sel = i.min(self.folders.len().saturating_sub(1));
                if let Some(folder) = self.folders.get(self.sel) {
                    if folder.committed {
                        self.note = Some((
                            "already on the server — rename it in the admin panel".to_string(),
                            false,
                        ));
                    } else {
                        self.editing = Some(folder.name.clone());
                    }
                }
            }
            Act::BrowseNative => self.queue(Op::PickNative, "opening the folder picker…"),
            Act::TypePath => {
                self.modal = Modal::PathEntry(PathDraft::default());
                self.refresh_completion();
            }
            Act::RemoveFolder => self.remove_selected(),
            Act::RemoveAt(i) => {
                if i < self.folders.len() {
                    self.sel = i;
                    self.remove_selected();
                }
            }
            Act::ContinueFolders => {
                self.finish_rename();
                if self.folders.is_empty() {
                    self.note = Some(("add at least one folder first".to_string(), true));
                } else {
                    self.queue(Op::CommitFolders, "adding folders to the server…");
                }
            }
            Act::Focus(field) => self.field = field,
            Act::CreateAdmin => match self.login_problem() {
                Some(problem) => self.note = Some((problem.to_string(), true)),
                None => self.queue(Op::CreateAdmin, "creating your login…"),
            },
            Act::SkipLogin => self.modal = Modal::SkipWarning,
            Act::SkipCancel => self.modal = Modal::None,
            Act::SkipConfirm => {
                self.modal = Modal::None;
                self.public = true;
                self.note = None;
                self.screen = Screen::Extras;
            }
            Act::Toggle(i) => {
                if let Some(on) = self.extras.get_mut(i) {
                    *on = !*on;
                    self.extras_sel = i;
                }
            }
            Act::ContinueExtras => self.queue(Op::CommitExtras, "saving your choices…"),
            Act::BrowseRow(i) => {
                if let Modal::Browser(b) = &mut self.modal {
                    b.sel = i.min(b.dirs.len().saturating_sub(1));
                }
            }
            Act::BrowseEnter => {
                if let Modal::Browser(b) = &self.modal {
                    if let Some(dir) = b.dirs.get(b.sel) {
                        let to = join_server_path(&b.path, dir);
                        self.queue(Op::BrowseTo(to), "listing…");
                    }
                }
            }
            Act::BrowseUp => {
                if let Modal::Browser(b) = &self.modal {
                    if let Some(parent) = parent_server_path(&b.path) {
                        self.queue(Op::BrowseTo(parent), "listing…");
                    }
                }
            }
            Act::BrowseAdd => {
                if let Modal::Browser(b) = &self.modal {
                    let path = b.path.clone();
                    self.modal = Modal::None;
                    self.add_folder(path);
                }
            }
            Act::BrowseCancel => self.modal = Modal::None,
            Act::PathSuggest(i) => self.accept_suggestion(i),
            Act::OpenPlayer => return Some(Outcome::OpenPlayer),
            Act::Finish => return Some(Outcome::Quit),
        }
        None
    }

    /// Queue a listing for the dir-part of the draft, if it changed.
    fn refresh_completion(&mut self) {
        if let Modal::PathEntry(draft) = &mut self.modal {
            draft.sel = None;
            let (dir, _) = split_input(&draft.text);
            if dir != draft.listed_for {
                draft.listed_for = dir.clone();
                draft.entries.clear();
                self.queued = Some(Op::Complete(dir));
            }
        }
    }

    /// Take suggestion `i` as the next path segment and keep typing inside
    /// it — the accepted text becomes the server-resolved absolute path, so
    /// a first completion turns "Mus" into a real /home/... path.
    fn accept_suggestion(&mut self, i: usize) {
        if let Modal::PathEntry(draft) = &mut self.modal {
            let picked = match draft.suggestions().get(i) {
                Some(entry) => entry.clone(),
                None => return,
            };
            let base = if draft.listed_path.is_empty() {
                split_input(&draft.text).0
            } else {
                draft.listed_path.clone()
            };
            let joined = join_server_path(&base, &picked);
            let sep = if joined.contains('\\') { '\\' } else { '/' };
            draft.text = format!("{joined}{sep}");
            self.refresh_completion();
        }
    }

    fn finish_rename(&mut self) {
        if let Some(draft) = self.editing.take() {
            if let Some(folder) = self.folders.get_mut(self.sel) {
                let clean = sanitize_name(&draft);
                if !clean.is_empty() {
                    folder.name = clean;
                    folder.named_by_user = true;
                }
            }
            sync_names(&mut self.folders);
        }
    }

    fn login_problem(&self) -> Option<&'static str> {
        if self.username.trim().is_empty() {
            return Some("pick a username");
        }
        if self.password.is_empty() {
            return Some("pick a password");
        }
        if self.password != self.confirm {
            return Some("the passwords do not match");
        }
        None
    }

    // ── Server calls (run between draws) ────────────────────────────────────

    fn run_queued(&mut self) {
        let Some(op) = self.queued.take() else { return };
        self.busy = None;
        match op {
            Op::Ping => match self.client.ping() {
                Ok(_) => self.note = None,
                Err(e) => self.fail("could not reach the server", e),
            },
            Op::PickNative => match picker::pick_folder() {
                picker::Pick::Folder(path) => self.add_folder(path.display().to_string()),
                picker::Pick::Cancelled => {}
                picker::Pick::Unavailable(why) => {
                    self.note = Some((
                        format!("no native picker here ({why}) — browsing on the server instead"),
                        false,
                    ));
                    self.queue(Op::OpenBrowser("~".to_string()), "listing…");
                }
            },
            Op::OpenBrowser(path) | Op::BrowseTo(path) => {
                match self.client.admin_file_explorer(&path) {
                    Ok(listing) => {
                        self.modal = Modal::Browser(Browse {
                            path: listing.path,
                            dirs: listing.directories.into_iter().map(|d| d.name).collect(),
                            sel: 0,
                        });
                    }
                    Err(e) => self.fail("could not browse there", e),
                }
            }
            Op::Complete(dir) => {
                // The user may have typed on: only install the listing if it
                // still answers the draft's current dir-part.
                match self.client.admin_file_explorer(&dir) {
                    Ok(listing) => {
                        if let Modal::PathEntry(draft) = &mut self.modal {
                            if draft.listed_for == dir {
                                draft.listed_path = listing.path;
                                draft.entries =
                                    listing.directories.into_iter().map(|d| d.name).collect();
                            }
                        }
                    }
                    // Mid-typing the dir-part is bogus half the time; an
                    // empty list is the honest render for that.
                    Err(_) => {}
                }
            }
            Op::CommitFolders => {
                for i in 0..self.folders.len() {
                    if self.folders[i].committed {
                        continue;
                    }
                    let (path, name) = (self.folders[i].path.clone(), self.folders[i].name.clone());
                    match self.client.admin_add_directory(&path, &name) {
                        Ok(_) => self.folders[i].committed = true,
                        Err(e) => {
                            self.fail(&format!("could not add {name}"), e);
                            return;
                        }
                    }
                }
                self.note = None;
                self.screen = Screen::Login;
            }
            Op::CreateAdmin => {
                let vpaths: Vec<String> = self.folders.iter().map(|f| f.name.clone()).collect();
                if let Err(e) =
                    self.client.admin_create_user(&self.username, &self.password, &vpaths, true)
                {
                    return self.fail("could not create the login", e);
                }
                // Sign in as the account that now guards the server, and
                // remember it — finishing the wizard should leave the player
                // itself ready to use.
                if let Err(e) = self.client.login(&self.username.clone(), &self.password.clone()) {
                    return self.fail("created, but could not sign in", e);
                }
                self.remember_session();
                self.note = None;
                self.screen = Screen::Extras;
            }
            Op::CommitExtras => {
                let steps: [(usize, &str, fn(&Client, bool) -> Result<(), ApiError>); 3] = [
                    (0, "updates", |c, on| {
                        c.admin_update_mode(if on { "stage" } else { "notify" }).map(|_| ())
                    }),
                    (1, "server audio", |c, on| {
                        if on { c.admin_auto_boot_audio(true).map(|_| ()) } else { Ok(()) }
                    }),
                    (2, "discovery", |c, on| {
                        if on { c.admin_discovery_enabled(true).map(|_| ()) } else { Ok(()) }
                    }),
                ];
                for (i, label, apply) in steps {
                    if self.extras_done[i] {
                        continue;
                    }
                    match apply(&self.client, self.extras[i]) {
                        Ok(()) => self.extras_done[i] = true,
                        Err(e) => {
                            return self.fail(&format!("could not set up {label}"), e);
                        }
                    }
                }
                self.note = None;
                self.screen = Screen::Done;
                self.queue(Op::LoadDone, "fetching your Quick Connect code…");
            }
            Op::LoadDone => {
                match self.client.admin_iroh() {
                    Ok(status) => match status.qr.as_deref() {
                        Some(ticket) if status.enabled => match qr_lines(ticket) {
                            Some(lines) => {
                                self.qr = Some(lines);
                                self.qr_note =
                                    "Scan with the mStream app to connect from anywhere — no port forwarding.".to_string();
                            }
                            None => self.qr_note = "Quick Connect is on — the code is in the admin panel.".to_string(),
                        },
                        _ if !status.enabled => {
                            self.qr_note =
                                "Quick Connect is off — turn it on in the admin panel to connect from anywhere.".to_string();
                        }
                        _ => {
                            self.qr_note =
                                "Quick Connect is still starting — the code will be in the admin panel.".to_string();
                        }
                    },
                    Err(e) => self.fail("could not read Quick Connect state", e),
                }
                self.queue(Op::PollProgress, "");
            }
            Op::PollProgress => {
                self.last_poll = Instant::now();
                match self.client.scan_progress() {
                    Ok(rows) if rows.is_empty() => {
                        self.progress = "Library scan complete.".to_string();
                    }
                    Ok(rows) => {
                        let row = &rows[0];
                        self.progress = match row.pct {
                            Some(pct) => format!(
                                "Scanning {} — {}% ({} tracks so far{})",
                                row.vpath,
                                pct,
                                row.scanned,
                                if rows.len() > 1 { ", more queued" } else { "" }
                            ),
                            None => format!(
                                "Scanning {} — {} tracks so far{}",
                                row.vpath,
                                row.scanned,
                                if rows.len() > 1 { ", more queued" } else { "" }
                            ),
                        };
                    }
                    // Progress is garnish; a hiccup should never mark the
                    // Done screen with an error.
                    Err(_) => {}
                }
            }
        }
    }

    fn fail(&mut self, what: &str, e: ApiError) {
        self.note = Some((format!("{what}: {e}"), true));
    }

    fn remember_session(&mut self) {
        let server = self.client.server();
        // House rule (tui::remember): a config that fails to LOAD is never
        // overwritten with defaults — this run just doesn't get saved.
        match config::load() {
            Ok(mut cfg) => {
                config::touch_server(&mut cfg, &server, Some(self.username.clone()));
                if let Err(e) = config::save(&cfg) {
                    self.note = Some((format!("could not save settings: {e}"), false));
                }
            }
            Err(e) => self.note = Some((format!("settings not saved — {e}"), false)),
        }
        match config::load_credentials() {
            Ok(mut creds) => {
                config::store_token(&mut creds, &server, self.client.token().map(String::from));
                if let Err(e) = config::save_credentials(&creds) {
                    self.note = Some((format!("could not save the session: {e}"), false));
                }
            }
            Err(e) => self.note = Some((format!("session not saved — {e}"), false)),
        }
    }
}

// ── Pure helpers (unit-tested below) ────────────────────────────────────────

/// A vpath the server will accept: letters, digits and dashes only.
pub(crate) fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true; // no leading dash
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// The default name for a folder: its basename, sanitized.
pub(crate) fn derive_name(path: &str) -> String {
    let base = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("");
    let clean = sanitize_name(base);
    if clean.is_empty() { "folder".to_string() } else { clean }
}

/// Re-derive every non-user-named folder's name for the current shape of the
/// list: one folder is simply `media`; several get basename-derived names,
/// deduplicated with numeric suffixes. Names the user typed are never
/// touched (they only join dedup as taken).
pub(crate) fn sync_names(folders: &mut [Folder]) {
    let single = folders.len() == 1;
    let mut taken: Vec<String> = folders
        .iter()
        .filter(|f| f.named_by_user || f.committed)
        .map(|f| f.name.clone())
        .collect();
    for folder in folders.iter_mut() {
        if folder.named_by_user || folder.committed {
            continue;
        }
        let base = if single { SINGLE_NAME.to_string() } else { derive_name(&folder.path) };
        let mut name = base.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        taken.push(name.clone());
        folder.name = name;
    }
}

/// Join a listing's server-native path with a child entry. The separator is
/// whichever family the path already uses — the server may be Windows.
pub(crate) fn join_server_path(path: &str, child: &str) -> String {
    let sep = if path.contains('\\') { '\\' } else { '/' };
    if path.ends_with(sep) {
        format!("{path}{child}")
    } else {
        format!("{path}{sep}{child}")
    }
}

/// The parent of a server-native path, or None at a root.
pub(crate) fn parent_server_path(path: &str) -> Option<String> {
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let trimmed = path.trim_end_matches(sep);
    let cut = trimmed.rfind(sep)?;
    let parent = &trimmed[..cut];
    if parent.is_empty() {
        // "/music" → "/" ; "C:" has no parent worth visiting.
        if sep == '/' { Some("/".to_string()) } else { None }
    } else {
        Some(parent.to_string())
    }
}

/// Split the type-a-path draft into (dir-part to list, partial segment).
/// The dir-part keeps its trailing separator; an empty or separator-less
/// draft completes against the server user's home ("~", which the admin
/// file-explorer resolves).
pub(crate) fn split_input(text: &str) -> (String, String) {
    let cut = text.rfind(['/', '\\']);
    match cut {
        Some(i) => (text[..=i].to_string(), text[i + 1..].to_string()),
        None => ("~".to_string(), text.to_string()),
    }
}

/// Case-folded prefix test (mac and Windows server filesystems are
/// case-insensitive; on Linux this is merely forgiving).
pub(crate) fn starts_with_fold(name: &str, prefix: &str) -> bool {
    let mut name_chars = name.chars();
    prefix.chars().all(|p| {
        name_chars
            .next()
            .is_some_and(|n| n.to_lowercase().eq(p.to_lowercase()))
    })
}

/// The longest common prefix of the suggestions, case-insensitively, in the
/// first entry's own casing.
pub(crate) fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else { return String::new() };
    let mut len = first.chars().count();
    for item in &items[1..] {
        let matched = first
            .chars()
            .zip(item.chars())
            .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            .count();
        len = len.min(matched);
    }
    first.chars().take(len).collect()
}

/// The pairing ticket as unicode half-blocks, light-on-dark so it scans off
/// a dark terminal.
fn qr_lines(data: &str) -> Option<Vec<String>> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let rendered = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    Some(rendered.lines().map(str::to_string).collect())
}

// ── Entry ────────────────────────────────────────────────────────────────────

pub fn run(args: SetupArgs) -> i32 {
    let server = match crate::api::server_url::normalize(&args.server) {
        Ok(server) => server,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return 1;
        }
    };
    let client = match Client::new(&server) {
        Ok(client) => client.with_token(args.token),
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return 1;
        }
    };

    let mut wizard = Wizard::new(client);
    wizard.queue(Op::Ping, "reaching the server…");

    let mut terminal = ratatui::init();
    // A wizard whose buttons cannot be clicked is half a wizard; like the
    // player, a terminal that refuses mouse reports still works by keys.
    let mouse_on = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    let outcome = event_loop(&mut terminal, &mut wizard, mouse_on);
    if mouse_on {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        set_pointer_shape(false, true);
    }
    ratatui::restore();

    match outcome {
        Ok(Outcome::OpenPlayer) => {
            let token = wizard.client.token().map(String::from);
            crate::tui::run(Some(wizard.client.server()), token)
        }
        Ok(Outcome::Quit) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

/// Ask the terminal for a hand cursor over clickables (OSC 22, the xterm
/// pointerShape control). iTerm2, kitty, WezTerm and friends honor it;
/// everything else ignores the sequence, which costs nothing. Emitted only
/// on state CHANGES so the stream is not littered with it.
fn set_pointer_shape(hand: bool, mouse_on: bool) {
    if !mouse_on {
        return;
    }
    let sequence = if hand { "\x1b]22;pointer\x1b\\" } else { "\x1b]22;default\x1b\\" };
    let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(sequence));
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    wizard: &mut Wizard,
    mouse_on: bool,
) -> std::io::Result<Outcome> {
    let mut hand = false;
    loop {
        terminal.draw(|frame| render(frame, wizard))?;

        // The hand cursor follows whether the pointer is over anything
        // clickable in the frame just drawn.
        let over = wizard
            .pointer
            .is_some_and(|p| wizard.clicks.iter().any(|(rect, _)| rect.contains(p)));
        if over != hand {
            hand = over;
            set_pointer_shape(hand, mouse_on);
        }

        // Server calls run here, after their "working…" frame is visible.
        wizard.run_queued();

        if wizard.screen == Screen::Done
            && wizard.queued.is_none()
            && wizard.last_poll.elapsed() >= PROGRESS_EVERY
        {
            wizard.queued = Some(Op::PollProgress);
            continue;
        }

        if !event::poll(POLL)? {
            continue;
        }
        // Drain everything queued before the next draw: mouse capture arms
        // any-motion tracking, so a sweep of the pointer is one event per
        // cell crossed — pointer updates are cheap, but each must not cost
        // a frame (the player's collapse-moves lesson).
        let mut inputs = vec![event::read()?];
        while event::poll(Duration::ZERO)? {
            inputs.push(event::read()?);
        }
        for input in inputs {
            match input {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        return Ok(Outcome::Quit);
                    }
                    if let Some(outcome) = handle_key(wizard, key.code) {
                        return Ok(outcome);
                    }
                }
                TermEvent::Mouse(mouse) => {
                    let at = Position { x: mouse.column, y: mouse.row };
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            wizard.pointer = Some(at);
                            let hit = wizard
                                .clicks
                                .iter()
                                .rev()
                                .find(|(rect, _)| rect.contains(at))
                                .map(|(_, act)| act.clone());
                            if let Some(act) = hit {
                                if let Some(outcome) = wizard.act(act) {
                                    return Ok(outcome);
                                }
                            }
                        }
                        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                            wizard.pointer = Some(at);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn handle_key(wizard: &mut Wizard, code: KeyCode) -> Option<Outcome> {
    // Text entry captures everything printable first.
    match &mut wizard.modal {
        Modal::PathEntry(draft) => {
            match code {
                KeyCode::Esc => wizard.modal = Modal::None,
                KeyCode::Backspace => {
                    draft.text.pop();
                    wizard.refresh_completion();
                }
                KeyCode::Enter => {
                    let path = draft.text.trim().to_string();
                    wizard.modal = Modal::None;
                    if !path.is_empty() {
                        wizard.add_folder(path);
                    }
                }
                KeyCode::Down => {
                    let n = draft.suggestions().len();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(0, |i| (i + 1) % n));
                    }
                }
                KeyCode::Up => {
                    let n = draft.suggestions().len();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(n - 1, |i| (i + n - 1) % n));
                    }
                }
                KeyCode::Tab | KeyCode::Right => {
                    let suggestions = draft.suggestions();
                    if let Some(i) = draft.sel {
                        wizard.accept_suggestion(i);
                    } else if suggestions.len() == 1 {
                        wizard.accept_suggestion(0);
                    } else if !suggestions.is_empty() {
                        // Extend to the longest common prefix; if that gains
                        // nothing, start cycling.
                        let (_, partial) = split_input(&draft.text);
                        let lcp = common_prefix(&suggestions);
                        if lcp.chars().count() > partial.chars().count() {
                            let keep = draft.text.chars().count() - partial.chars().count();
                            draft.text = draft.text.chars().take(keep).collect::<String>() + &lcp;
                            // Same dir-part, narrower partial — no re-list.
                            draft.sel = None;
                        } else {
                            draft.sel = Some(0);
                        }
                    }
                }
                KeyCode::Char(c) => {
                    draft.text.push(c);
                    wizard.refresh_completion();
                }
                _ => {}
            }
            return None;
        }
        Modal::SkipWarning => {
            return match code {
                KeyCode::Esc | KeyCode::Char('b') => wizard.act(Act::SkipCancel),
                KeyCode::Enter => wizard.act(Act::SkipConfirm),
                _ => None,
            };
        }
        Modal::Browser(b) => {
            return match code {
                KeyCode::Esc => wizard.act(Act::BrowseCancel),
                KeyCode::Up => {
                    b.sel = b.sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    b.sel = (b.sel + 1).min(b.dirs.len().saturating_sub(1));
                    None
                }
                KeyCode::Enter | KeyCode::Right => wizard.act(Act::BrowseEnter),
                KeyCode::Left | KeyCode::Backspace => wizard.act(Act::BrowseUp),
                KeyCode::Char('a') => wizard.act(Act::BrowseAdd),
                _ => None,
            };
        }
        Modal::None => {}
    }

    if let Some(draft) = &mut wizard.editing {
        match code {
            KeyCode::Esc => {
                wizard.editing = None;
            }
            KeyCode::Backspace => {
                draft.pop();
            }
            KeyCode::Enter => wizard.finish_rename(),
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == '-' => {
                draft.push(c.to_ascii_lowercase());
            }
            _ => {}
        }
        return None;
    }

    match wizard.screen {
        Screen::Folders => match code {
            KeyCode::Up => wizard.act(Act::SelectFolder(wizard.sel.saturating_sub(1))),
            KeyCode::Down => wizard.act(Act::SelectFolder(wizard.sel + 1)),
            KeyCode::Enter => wizard.act(Act::RenameFolder(wizard.sel)),
            KeyCode::Char('b') => wizard.act(Act::BrowseNative),
            KeyCode::Char('t') => wizard.act(Act::TypePath),
            KeyCode::Char('r') | KeyCode::Delete => wizard.act(Act::RemoveFolder),
            KeyCode::Char('c') => wizard.act(Act::ContinueFolders),
            KeyCode::Char('q') => Some(Outcome::Quit),
            _ => None,
        },
        Screen::Login => match code {
            KeyCode::Tab | KeyCode::Down => {
                wizard.field = match wizard.field {
                    LoginField::Username => LoginField::Password,
                    LoginField::Password => LoginField::Confirm,
                    LoginField::Confirm => LoginField::Username,
                };
                None
            }
            KeyCode::BackTab | KeyCode::Up => {
                wizard.field = match wizard.field {
                    LoginField::Username => LoginField::Confirm,
                    LoginField::Password => LoginField::Username,
                    LoginField::Confirm => LoginField::Password,
                };
                None
            }
            KeyCode::Enter => wizard.act(Act::CreateAdmin),
            KeyCode::Backspace => {
                match wizard.field {
                    LoginField::Username => wizard.username.pop(),
                    LoginField::Password => wizard.password.pop(),
                    LoginField::Confirm => wizard.confirm.pop(),
                };
                None
            }
            KeyCode::Char(c) => {
                match wizard.field {
                    LoginField::Username => wizard.username.push(c),
                    LoginField::Password => wizard.password.push(c),
                    LoginField::Confirm => wizard.confirm.push(c),
                }
                None
            }
            KeyCode::Esc => wizard.act(Act::SkipLogin),
            _ => None,
        },
        Screen::Extras => match code {
            KeyCode::Up => {
                wizard.extras_sel = wizard.extras_sel.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                wizard.extras_sel = (wizard.extras_sel + 1).min(wizard.extras.len() - 1);
                None
            }
            KeyCode::Char(' ') | KeyCode::Enter => wizard.act(Act::Toggle(wizard.extras_sel)),
            KeyCode::Char('c') => wizard.act(Act::ContinueExtras),
            KeyCode::Char('q') => Some(Outcome::Quit),
            _ => None,
        },
        Screen::Done => match code {
            KeyCode::Enter | KeyCode::Char('o') => wizard.act(Act::OpenPlayer),
            KeyCode::Char('f') | KeyCode::Char('q') => wizard.act(Act::Finish),
            _ => None,
        },
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, wizard: &mut Wizard) {
    wizard.clicks.clear();
    let area = frame.area();
    if area.width < 58 || area.height < 20 {
        frame.render_widget(
            Paragraph::new("please make the terminal a little larger").style(dim()),
            area,
        );
        return;
    }

    // The centered column everything lives in.
    let width = COLUMN.min(area.width.saturating_sub(4));
    let column = Rect {
        x: (area.width - width) / 2,
        y: 2,
        width,
        height: area.height.saturating_sub(4),
    };

    match wizard.screen {
        Screen::Folders => draw_folders(frame, wizard, column),
        Screen::Login => draw_login(frame, wizard, column),
        Screen::Extras => draw_extras(frame, wizard, column),
        Screen::Done => draw_done(frame, wizard, column),
    }

    // Status note.
    if let Some((text, is_err)) = wizard.note.clone() {
        let style = if is_err { Style::default().fg(WARN) } else { dim() };
        let note_area =
            Rect { x: column.x, y: area.height.saturating_sub(3), width: column.width, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), note_area);
    }
    if let Some(busy) = wizard.busy {
        let busy_area =
            Rect { x: column.x, y: area.height.saturating_sub(3), width: column.width, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(busy, accent())), busy_area);
    }

    // Footer, under its gold rule.
    let rule = Rect { x: 2, y: area.height.saturating_sub(2), width: area.width - 4, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::default().fg(GOLD))),
        rule,
    );
    let footer = Rect { x: 2, y: area.height.saturating_sub(1), width: area.width - 4, height: 1 };
    let step = format!("Step {} of 4", wizard.screen.step());
    frame.render_widget(Paragraph::new(Span::styled(step, dim())), footer);
    frame.render_widget(
        Paragraph::new(Span::styled(footer_hint(wizard), dim())).alignment(Alignment::Right),
        footer,
    );

    match wizard.modal.clone() {
        Modal::None => {}
        Modal::SkipWarning => draw_skip_warning(frame, wizard, area),
        Modal::Browser(browse) => draw_browser(frame, wizard, area, &browse),
        Modal::PathEntry(draft) => draw_path_entry(frame, wizard, area, &draft),
    }
}

fn footer_hint(wizard: &Wizard) -> &'static str {
    match (&wizard.modal, wizard.screen) {
        (Modal::Browser(_), _) => "↑ ↓ move · Enter open · a add this folder · Esc close",
        (Modal::PathEntry(_), _) => "Tab complete · ↑ ↓ pick · Enter add folder · Esc close",
        (Modal::SkipWarning, _) => "Enter go public · Esc back",
        (_, Screen::Folders) => "b browse · t type a path · Enter rename · c continue",
        (_, Screen::Login) => "Tab next field · Enter create · Esc skip",
        (_, Screen::Extras) => "Space toggle · c continue",
        (_, Screen::Done) => "Enter open the player · f finish",
    }
}

/// A one-line clickable button: draws itself, registers its click, and
/// lights up when the pointer is over it. Returns the rect it drew into.
fn button(
    frame: &mut Frame,
    wizard: &mut Wizard,
    at: Rect,
    label: &str,
    primary: bool,
    act: Act,
) -> Rect {
    let text = format!("  {label}  ");
    let width = (text.chars().count() as u16).min(at.width);
    let rect = Rect { x: at.x, y: at.y, width, height: 1 };
    let hovered = wizard.pointer.is_some_and(|p| rect.contains(p));
    let style = match (primary, hovered) {
        (true, true) => Style::default().fg(Color::Black).bg(BRIGHT).add_modifier(Modifier::BOLD),
        (true, false) => Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD),
        (false, true) => Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
        (false, false) => dim(),
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), rect);
    wizard.clicks.push((rect, act));
    rect
}

fn card(frame: &mut Frame, at: Rect, focused: bool) -> Rect {
    let style = if focused { accent() } else { dim() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style);
    let inner = block.inner(at);
    frame.render_widget(block, at);
    inner
}

const LOGO: [&str; 5] = [
    r"            ____  _                            ",
    r"  _ __ ___ / ___|| |_ _ __ ___  __ _ _ __ ___  ",
    r" | '_ ` _ \ ___ \| __| '__/ _ \/ _` | '_ ` _ \ ",
    r" | | | | | |___) | |_| |  __/ (_| | | | | | |  ",
    r" |_| |_| |_|____/ \__|_|\___|\__,_|_| |_| |_|  ",
];

fn draw_folders(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    for line in LOGO {
        frame.render_widget(
            Paragraph::new(Span::styled(line, accent())).alignment(Alignment::Center),
            Rect { x: column.x, y, width: column.width, height: 1 },
        );
        y += 1;
    }
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled("Where does your music live?", bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    for i in 0..wizard.folders.len() {
        let selected = i == wizard.sel;
        let rect = Rect { x: column.x, y, width: column.width, height: 3 };
        let inner = card(frame, rect, selected);
        wizard.clicks.push((rect, Act::SelectFolder(i)));

        // [name]  /the/path                                            ✕
        let folder = &wizard.folders[i];
        let name = match (&wizard.editing, selected) {
            (Some(draft), true) => format!("[{draft}▏]"),
            _ => format!("[{}]", folder.name),
        };
        let name_width = (name.chars().count() as u16).min(inner.width);
        let name_rect = Rect { x: inner.x + 1, y: inner.y, width: name_width, height: 1 };
        let name_style = if wizard.editing.is_some() && selected {
            Style::default().fg(BRIGHT)
        } else {
            accent()
        };
        frame.render_widget(Paragraph::new(Span::styled(name, name_style)), name_rect);
        wizard.clicks.push((name_rect, Act::RenameFolder(i)));

        let path_x = name_rect.right() + 2;
        frame.render_widget(
            Paragraph::new(Span::raw(folder.path.clone())),
            Rect {
                x: path_x,
                y: inner.y,
                width: inner.right().saturating_sub(path_x + 4),
                height: 1,
            },
        );

        let x_rect = Rect { x: inner.right().saturating_sub(2), y: inner.y, width: 1, height: 1 };
        let x_hover = wizard.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled("✕", x_style)), x_rect);
        wizard.clicks.push((x_rect, Act::RemoveAt(i)));
        y += 3;
    }

    // The add-card, button-like: its border lights up under the pointer.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = wizard.pointer.is_some_and(|p| add_rect.contains(p));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if add_hover { accent() } else { dim() });
    let inner = block.inner(add_rect);
    frame.render_widget(block, add_rect);
    frame.render_widget(
        Paragraph::new(Span::styled("Click to open file picker", accent()))
            .alignment(Alignment::Center),
        inner,
    );
    wizard.clicks.push((add_rect, Act::BrowseNative));
    y += 4;

    button(
        frame,
        wizard,
        Rect { x: column.x, y, width: column.width, height: 1 },
        "type a path",
        false,
        Act::TypePath,
    );

    let label = "Continue ▸";
    let x = column.right().saturating_sub(label.len() as u16 + 4);
    button(frame, wizard, Rect { x, y, width: column.width, height: 1 }, label, true, Act::ContinueFolders);
}

fn field_row(
    frame: &mut Frame,
    wizard: &mut Wizard,
    x: u16,
    y: u16,
    width: u16,
    label: &str,
    value: String,
    field: LoginField,
    focused: bool,
) -> u16 {
    frame.render_widget(
        Paragraph::new(Span::styled(label, dim())),
        Rect { x, y, width, height: 1 },
    );
    let rect = Rect { x, y: y + 1, width, height: 3 };
    let inner = card(frame, rect, focused);
    let shown = if focused { format!("{value}▏") } else { value };
    frame.render_widget(Paragraph::new(Span::raw(shown)), inner);
    wizard.clicks.push((rect, Act::Focus(field)));
    y + 4
}

fn draw_login(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled("Create your login", bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled(
            "This account manages the server — keep the password somewhere safe.",
            dim(),
        )),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let width = column.width.min(44);
    let x = column.x + (column.width - width) / 2;
    let (username, password, confirm) =
        (wizard.username.clone(), mask(&wizard.password), mask(&wizard.confirm));
    let focus = wizard.field;
    y = field_row(frame, wizard, x, y, width, "USERNAME", username, LoginField::Username, focus == LoginField::Username);
    y = field_row(frame, wizard, x, y, width, "PASSWORD", password, LoginField::Password, focus == LoginField::Password);
    y = field_row(frame, wizard, x, y, width, "CONFIRM PASSWORD", confirm, LoginField::Confirm, focus == LoginField::Confirm);
    y += 1;

    let rect = button(frame, wizard, Rect { x, y, width, height: 1 }, "Create Admin ▸", true, Act::CreateAdmin);
    button(
        frame,
        wizard,
        Rect { x: rect.right() + 2, y, width, height: 1 },
        "Skip for now",
        false,
        Act::SkipLogin,
    );
}

fn mask(secret: &str) -> String {
    "•".repeat(secret.chars().count())
}

fn draw_extras(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled("Extras", bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled("All optional, all changeable later.", dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let rows: [(&str, &str); 3] = [
        ("Automatic updates", "download new versions in the background, apply when you restart"),
        ("Server-side audio", "play music out of this machine's own speakers"),
        ("Discovery network", "find and share libraries with other mStream servers, peer to peer"),
    ];
    for (i, (label, desc)) in rows.iter().enumerate() {
        let selected = i == wizard.extras_sel;
        let rect = Rect { x: column.x, y, width: column.width, height: 4 };
        let inner = card(frame, rect, selected);
        wizard.clicks.push((rect, Act::Toggle(i)));
        let box_span = if wizard.extras[i] {
            Span::styled("[x] ", Style::default().fg(OK))
        } else {
            Span::styled("[ ] ", dim())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![box_span, Span::styled(*label, bold())])),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(*desc, dim())),
            Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
        );
        y += 4;
    }
    y += 1;

    let label = "Continue ▸";
    let x = column.right().saturating_sub(label.len() as u16 + 4);
    button(frame, wizard, Rect { x, y, width: column.width, height: 1 }, label, true, Act::ContinueExtras);
}

fn draw_done(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled("You are set. Take it with you.", bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    if let Some(qr) = wizard.qr.clone() {
        let qr_width = qr.first().map(|l| l.chars().count()).unwrap_or(0) as u16;
        let x = column.x + (column.width.saturating_sub(qr_width)) / 2;
        for line in &qr {
            frame.render_widget(
                Paragraph::new(Span::raw(line.clone())),
                Rect { x, y, width: qr_width.min(column.width), height: 1 },
            );
            y += 1;
        }
        y += 1;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(wizard.qr_note.clone(), Style::default()))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        Rect { x: column.x, y, width: column.width, height: 2 },
    );
    y += 2;
    frame.render_widget(
        Paragraph::new(Span::styled(wizard.progress.clone(), Style::default().fg(OK)))
            .alignment(Alignment::Center),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let open = button(
        frame,
        wizard,
        Rect { x: column.x, y, width: column.width, height: 1 },
        "Open the Player ▸",
        true,
        Act::OpenPlayer,
    );
    button(
        frame,
        wizard,
        Rect { x: open.right() + 2, y, width: column.width, height: 1 },
        "Finish",
        false,
        Act::Finish,
    );
}

fn modal_frame(frame: &mut Frame, area: Rect, width: u16, height: u16, title_color: Color) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(title_color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

fn draw_skip_warning(frame: &mut Frame, wizard: &mut Wizard, area: Rect) {
    let inner = modal_frame(frame, area, 62, 13, WARN);
    let lines = vec![
        Line::from(Span::styled("Run in Public Mode?", Style::default().fg(WARN).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("No login means the server is open to everyone who can reach it."),
        Line::from(""),
        Line::from(Span::styled("+ Instant access for everyone on your home network", Style::default().fg(OK))),
        Line::from(Span::styled("+ Nothing to type on TVs and shared devices", Style::default().fg(OK))),
        Line::from(Span::styled("− Anyone who reaches the server has full control", Style::default().fg(WARN))),
        Line::from(Span::styled("− Your Quick Connect code becomes a key to everything", Style::default().fg(WARN))),
        Line::from(""),
        Line::from(Span::styled("You can add a login later from the admin panel.", dim())),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    let y = inner.bottom().saturating_sub(1);
    let back = button(
        frame,
        wizard,
        Rect { x: inner.x, y, width: inner.width, height: 1 },
        "◂ Back — create a login",
        true,
        Act::SkipCancel,
    );
    button(
        frame,
        wizard,
        Rect { x: back.right() + 2, y, width: inner.width, height: 1 },
        "Go public anyway",
        false,
        Act::SkipConfirm,
    );
}

fn draw_browser(frame: &mut Frame, wizard: &mut Wizard, area: Rect, browse: &Browse) {
    let inner = modal_frame(frame, area, 66, 18, ACCENT);
    frame.render_widget(
        Paragraph::new(Span::styled("Browse the server's folders", bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
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
            Style::default().fg(Color::Black).bg(ACCENT)
        } else {
            Style::default()
        };
        let rect = Rect { x: inner.x, y: list_top + row as u16, width: inner.width, height: 1 };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("▸ {}", browse.dirs[i]), style)),
            rect,
        );
        wizard.clicks.push((rect, Act::BrowseRow(i)));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("(no folders in here)", dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }

    let y = inner.bottom().saturating_sub(1);
    let up = button(frame, wizard, Rect { x: inner.x, y, width: inner.width, height: 1 }, "◂ up", false, Act::BrowseUp);
    let open = button(frame, wizard, Rect { x: up.right() + 1, y, width: inner.width, height: 1 }, "open", false, Act::BrowseEnter);
    let add = button(
        frame,
        wizard,
        Rect { x: open.right() + 1, y, width: inner.width, height: 1 },
        "Add this folder ▸",
        true,
        Act::BrowseAdd,
    );
    button(frame, wizard, Rect { x: add.right() + 1, y, width: inner.width, height: 1 }, "close", false, Act::BrowseCancel);
}

fn draw_path_entry(frame: &mut Frame, wizard: &mut Wizard, area: Rect, draft: &PathDraft) {
    let suggestions = draft.suggestions();
    let shown = suggestions.len().min(6) as u16;
    let inner = modal_frame(frame, area, 62, 7 + shown, ACCENT);
    frame.render_widget(
        Paragraph::new(Span::styled("Type the full path of a music folder", bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    frame.render_widget(
        Paragraph::new(Span::raw(format!("{}▏", draft.text))),
        Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: 1 },
    );
    for (i, entry) in suggestions.iter().take(shown as usize).enumerate() {
        let selected = draft.sel == Some(i);
        let rect =
            Rect { x: inner.x, y: inner.y + 4 + i as u16, width: inner.width, height: 1 };
        let hovered = wizard.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(Color::Black).bg(ACCENT)
        } else if hovered {
            Style::default().fg(BRIGHT)
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("▸ {entry}"), style)),
            rect,
        );
        wizard.clicks.push((rect, Act::PathSuggest(i)));
    }
    if suggestions.len() > shown as usize {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("… and {} more", suggestions.len() - shown as usize),
                dim(),
            )),
            Rect { x: inner.x, y: inner.y + 4 + shown, width: inner.width, height: 1 },
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str) -> Folder {
        Folder { path: path.to_string(), name: String::new(), named_by_user: false, committed: false }
    }

    #[test]
    fn a_single_folder_is_simply_media() {
        let mut folders = vec![folder("/Users/anna/Music")];
        sync_names(&mut folders);
        assert_eq!(folders[0].name, "media");
    }

    #[test]
    fn several_folders_get_basename_names_and_the_first_loses_media() {
        let mut folders = vec![folder("/Users/anna/Music")];
        sync_names(&mut folders);
        folders.push(folder("/Volumes/NAS/Audiobooks"));
        sync_names(&mut folders);
        assert_eq!(folders[0].name, "music");
        assert_eq!(folders[1].name, "audiobooks");
    }

    #[test]
    fn removing_back_to_one_returns_to_media() {
        let mut folders = vec![folder("/a/Music"), folder("/b/Audiobooks")];
        sync_names(&mut folders);
        folders.remove(1);
        sync_names(&mut folders);
        assert_eq!(folders[0].name, "media");
    }

    #[test]
    fn user_typed_names_survive_resync_and_join_dedup() {
        let mut folders = vec![folder("/a/Music"), folder("/b/Music")];
        sync_names(&mut folders);
        assert_eq!(folders[1].name, "music-2", "same basename dedups");
        folders[0].name = "vinyl".to_string();
        folders[0].named_by_user = true;
        folders.push(folder("/c/Vinyl"));
        sync_names(&mut folders);
        assert_eq!(folders[0].name, "vinyl", "the typed name is never touched");
        assert_eq!(folders[2].name, "vinyl-2", "and it counts as taken");
    }

    #[test]
    fn names_are_sanitized_to_the_server_charset() {
        assert_eq!(sanitize_name("My Music! (2024)"), "my-music-2024");
        assert_eq!(sanitize_name("---"), "");
        assert_eq!(derive_name("/srv/Ünicode Beats/"), "nicode-beats");
        assert_eq!(derive_name("C:\\Users\\anna\\My Music"), "my-music");
        assert_eq!(derive_name("///"), "folder");
    }

    #[test]
    fn server_paths_join_and_climb_in_their_own_separator_family() {
        assert_eq!(join_server_path("/home/anna", "Music"), "/home/anna/Music");
        assert_eq!(join_server_path("C:\\Users", "anna"), "C:\\Users\\anna");
        assert_eq!(parent_server_path("/home/anna/Music").as_deref(), Some("/home/anna"));
        assert_eq!(parent_server_path("/home").as_deref(), Some("/"));
        assert_eq!(parent_server_path("/"), None);
        assert_eq!(parent_server_path("C:\\Users\\anna").as_deref(), Some("C:\\Users"));
        assert_eq!(parent_server_path("C:\\Users").as_deref(), Some("C:"));
    }

    #[test]
    fn path_input_splits_into_listable_dir_and_partial() {
        assert_eq!(split_input(""), ("~".to_string(), String::new()));
        assert_eq!(split_input("Mus"), ("~".to_string(), "Mus".to_string()));
        assert_eq!(split_input("/"), ("/".to_string(), String::new()));
        assert_eq!(split_input("/Users/an"), ("/Users/".to_string(), "an".to_string()));
        assert_eq!(split_input("C:\\Us"), ("C:\\".to_string(), "Us".to_string()));
    }

    #[test]
    fn suggestions_filter_case_insensitively_and_share_a_prefix() {
        assert!(starts_with_fold("Music", "mus"));
        assert!(!starts_with_fold("Music", "musik"));
        let items = vec!["Music".to_string(), "Musicals".to_string(), "music-old".to_string()];
        assert_eq!(common_prefix(&items), "Music");
        assert_eq!(common_prefix(&[]), "");
    }

    #[test]
    fn accepting_a_suggestion_extends_into_the_resolved_dir() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.modal = Modal::PathEntry(PathDraft {
            text: "Mus".to_string(),
            listed_for: "~".to_string(),
            listed_path: "/home/anna".to_string(),
            entries: vec!["Music".to_string(), "Movies".to_string()],
            sel: None,
        });
        wizard.accept_suggestion(0);
        let Modal::PathEntry(draft) = &wizard.modal else { panic!("modal closed") };
        assert_eq!(draft.text, "/home/anna/Music/");
        assert!(
            matches!(&wizard.queued, Some(Op::Complete(dir)) if dir == "/home/anna/Music/"),
            "stepping into the dir must queue its listing"
        );
    }

    #[test]
    fn the_skip_modal_is_a_real_gate() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.screen = Screen::Login;
        wizard.act(Act::SkipLogin);
        assert_eq!(wizard.modal, Modal::SkipWarning);
        assert_eq!(wizard.screen, Screen::Login, "showing the warning must not advance");
        wizard.act(Act::SkipCancel);
        assert_eq!(wizard.modal, Modal::None);
        assert_eq!(wizard.screen, Screen::Login);
        wizard.act(Act::SkipLogin);
        wizard.act(Act::SkipConfirm);
        assert_eq!(wizard.screen, Screen::Extras);
        assert!(wizard.public);
    }

    #[test]
    fn create_admin_validates_before_touching_the_network() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.screen = Screen::Login;
        wizard.act(Act::CreateAdmin);
        assert!(wizard.queued.is_none(), "empty form must not queue a server call");
        wizard.username = "anna".into();
        wizard.password = "hunter2".into();
        wizard.confirm = "hunter3".into();
        wizard.act(Act::CreateAdmin);
        assert!(wizard.queued.is_none(), "mismatched passwords must not queue");
        wizard.confirm = "hunter2".into();
        wizard.act(Act::CreateAdmin);
        assert!(matches!(wizard.queued, Some(Op::CreateAdmin)));
    }

}
