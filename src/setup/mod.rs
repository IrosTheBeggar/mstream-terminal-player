//! First-run setup wizard: `mstream-player setup`.
//!
//! Five screens against a freshly installed mStream server — folders, first
//! login, opt-ins, Quick Connect — driven entirely through the server's admin
//! API on the fresh install's zero-account window (every request is an
//! implicit admin until the wizard creates the first user, at which point it
//! logs in and continues with the token).
//!
//! The look is the "airy minimal" direction from the design canvas: one
//! centered column, sparse rounded borders, frame-emphasis buttons that
//! brighten under the pointer, and a FIXED palette (see [`theme`]) so the
//! screens look the same in every terminal that can carry it.
//! Mouse-first — every control is clickable — and every action has
//! a key. All decisions live in [`Wizard`]; the loop below only draws,
//! reads input, and runs one queued server call per pass (queued so the
//! "working…" frame is on screen while the call blocks).

pub mod picker;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use clap::Args;
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::{ApiError, Client};
use crate::config;
use rust_i18n::t;

use crate::kit::{
    self, GroundGuard, POINTER_RESET, Surface, accent, bold, dim, set_pointer_shape, theme,
};
use crate::kit::theme::th;

/// How long to wait for input before redrawing anyway.
const POLL: Duration = Duration::from_millis(100);
/// How often the Done screen re-asks for scan progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(1500);
/// The one vpath name a single folder gets without being asked.
const SINGLE_NAME: &str = "media";
/// The widest the content column grows, in cells.
const COLUMN: u16 = 74;

/// Every language the mobile app ships, as (locale code, autonym) — the
/// wizard translates to exactly this set. English is index 0, the
/// fallback and the one human-written entry; the rest are machine
/// translations and say so when chosen.
pub(crate) const LANGS: [(&str, &str); 10] = [
    ("en", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("ja", "日本語"),
    ("pl", "Polski"),
    ("pt", "Português"),
    ("ru", "Русский"),
    ("zh", "中文"),
];

/// Detect and APPLY the boot language — `MSTREAM_SETUP_LANG`, then the
/// system locale, then English — returning its [`LANGS`] index.
pub(crate) fn boot_language() -> usize {
    let lang = detect_lang(std::env::var("MSTREAM_SETUP_LANG").ok(), sys_locale::get_locale());
    rust_i18n::set_locale(LANGS[lang].0);
    lang
}

/// The language to start in: `MSTREAM_SETUP_LANG` when set (tests, and an
/// explicit user override), else the system locale's base language when
/// the set covers it, else English.
pub(crate) fn detect_lang(env: Option<String>, system: Option<String>) -> usize {
    let pick = |code: &str| {
        let base = code.split(['-', '_', '.']).next().unwrap_or(code).to_ascii_lowercase();
        LANGS.iter().position(|(c, _)| *c == base)
    };
    env.and_then(|c| pick(&c)).or_else(|| system.and_then(|c| pick(&c))).unwrap_or(0)
}

#[derive(Args)]
pub struct SetupArgs {
    /// The server to configure — a fresh desktop install listens here
    #[arg(long, default_value = "http://localhost:3000")]
    server: String,

    /// Token for a server that already has accounts (testing)
    #[arg(long, hide = true)]
    token: Option<String>,
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
            Screen::Extras => 2,
            Screen::Login => 3,
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
    /// The language picker, from the folders screen's top-left selector.
    /// Carries its keyboard cursor.
    Language(usize),
}

/// The type-a-path modal's state: the text, plus a cached listing of the
/// directory the text currently sits in, from which suggestions derive.
#[derive(Debug, Clone, Default)]
pub(crate) struct PathDraft {
    /// The line editor (tui-input): value + cursor + the readline ops.
    pub text: Input,
    /// The dir-part the cached entries belong to (as derived from the text).
    pub listed_for: String,
    /// The server-resolved absolute form of that dir (what accepts build on).
    pub listed_path: String,
    pub entries: Vec<String>,
    /// Keyboard cursor within the CURRENT suggestion list, if any.
    pub sel: Option<usize>,
    /// First visible suggestion row — the list windows like the folders
    /// table, following the keyboard cursor past the fold.
    pub scroll: usize,
    /// Last frame's cursor — the view yanks only when it moves.
    pub sel_anchor: Option<usize>,
    /// Why the current dir-part could not be listed — silence would read
    /// as "no autocomplete here", so the failure is said out loud.
    pub error: Option<String>,
}

impl PartialEq for PathDraft {
    fn eq(&self, other: &Self) -> bool {
        self.text.value() == other.text.value()
            && self.text.cursor() == other.text.cursor()
            && self.listed_for == other.listed_for
            && self.listed_path == other.listed_path
            && self.entries == other.entries
            && self.sel == other.sel
            && self.scroll == other.scroll
            && self.sel_anchor == other.sel_anchor
            && self.error == other.error
    }
}
impl Eq for PathDraft {}

impl PathDraft {
    /// The entries that match the current partial segment, in order.
    pub fn suggestions(&self) -> Vec<String> {
        let (_, partial) = split_input(self.text.value());
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
    /// What the LOCAL filesystem says about this path — advisory only:
    /// warnings never block Continue (a remote server may still know a
    /// path this machine does not), the server has final say at commit.
    pub check: FolderCheck,
    /// The canonical local path (symlinks resolved), once validated —
    /// the basis for duplicate and nesting detection.
    pub canonical: Option<String>,
    /// The path of another chosen folder this one sits INSIDE (the
    /// server would scan these files twice). Recomputed as folders come
    /// and go.
    pub nested_in: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FolderCheck {
    /// Validation still on the worker (or not applicable).
    #[default]
    Pending,
    Ok,
    Missing,
    NotADir,
    Unreadable,
}

/// What the LOCAL filesystem says about a path, plus its canonical form.
/// Runs on the worker: a stat against a dead network mount can hang.
fn validate_path(path: &str) -> (FolderCheck, Option<String>) {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (FolderCheck::Missing, None),
        Err(_) => (FolderCheck::Unreadable, None),
        Ok(meta) if !meta.is_dir() => (FolderCheck::NotADir, None),
        Ok(_) => {
            let canonical = std::fs::canonicalize(path)
                .ok()
                .map(|c| c.to_string_lossy().into_owned());
            (FolderCheck::Ok, canonical)
        }
    }
}

/// Is `child` strictly inside `parent`? (Both canonical; the separator
/// boundary keeps /ab from matching /a.)
fn is_under(child: &str, parent: &str) -> bool {
    child != parent
        && child
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with(['/', '\\']))
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
    RenameFolder(usize),
    BrowseNative,
    TypePath,
    RemoveFolder,
    RemoveAt(usize),
    ContinueFolders,
    Focus(LoginField),
    CreateAdmin,
    BackToFolders,
    BackToExtras,
    SkipLogin,
    SkipConfirm,
    SkipCancel,
    Toggle(usize),
    ContinueExtras,
    TableScroll(i8),
    PathCancel,
    PathScroll(i8),
    TableScrollTo(usize),
    PathScrollTo(usize),
    BrowseRow(usize),
    BrowseEnter,
    BrowseUp,
    BrowseAdd,
    BrowseCancel,
    PathSuggest(usize),
    /// The store pages for the mobile apps — what the QR is scanned WITH.
    OpenAppStore,
    OpenPlayStore,
    /// The server's admin panel, in the browser.
    OpenAdmin,
    /// The language picker and its choice.
    OpenLanguage,
    SetLanguage(usize),
    Finish,
}

/// The OFFICIAL mobile apps' store pages — the server README's "Official
/// Mobile Apps" section (the open-source mstream_music repo), not the
/// third-party listings beneath it. The iOS URL drops the README's
/// storefront pin so Apple geo-resolves it.
const APP_STORE_URL: &str = "https://apps.apple.com/app/mstream-music/id1455309629";
const PLAY_STORE_URL: &str = "https://play.google.com/store/apps/details?id=mstream.music";

/// The Done page's button list, top to bottom: (label, tip).
fn done_buttons() -> [(String, String); DONE_ACTIONS] {
    [
        (t!("done.btn_android").to_string(), t!("done.tip_android").to_string()),
        (t!("done.btn_ios").to_string(), t!("done.tip_ios").to_string()),
        (t!("done.btn_admin").to_string(), t!("done.tip_admin").to_string()),
    ]
}
const DONE_ACTIONS: usize = 3;

fn done_action(i: usize) -> Act {
    match i {
        0 => Act::OpenPlayStore,
        1 => Act::OpenAppStore,
        _ => Act::OpenAdmin,
    }
}

/// The port a server URL listens on, for the Done page's opening line.
/// None for URLs that don't carry one (a Quick Connect tunnel id) and
/// don't imply one by scheme.
fn server_port(server: &str) -> Option<u16> {
    if let Some((_, tail)) = server.rsplit_once(':') {
        if let Ok(port) = tail.trim_end_matches('/').parse() {
            return Some(port);
        }
    }
    match server.split_once("://").map(|(scheme, _)| scheme) {
        Some("http") => Some(80),
        Some("https") => Some(443),
        _ => None,
    }
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
    Validate(String),
    CommitFolders,
    CreateAdmin,
    CommitExtras,
    LoadDone,
    PollProgress,
}

/// What the worker sends back for each [`Op`].
enum Done {
    /// Reachability, plus the folders the server ALREADY has (vpath name,
    /// server path) so a reopened wizard shows reality instead of
    /// re-deriving names that collide server-side. The list is
    /// best-effort: an auth wall (server already set up) yields an empty
    /// one and the wizard behaves as before.
    Ping(Result<Vec<(String, String)>, ApiError>),
    Picked(picker::Pick),
    Browsed(Result<crate::api::types::DirListing, String>),
    Completed { dir: String, listing: Result<crate::api::types::DirListing, String> },
    Validated { path: String, check: FolderCheck, canonical: Option<String> },
    /// Which folder indexes were committed this attempt, and the first
    /// failure if one stopped the batch.
    FoldersCommitted { committed: Vec<usize>, error: Option<(String, ApiError)> },
    /// The new admin's token — the main thread rebuilds its client around
    /// it so every later op runs authenticated.
    AdminCreated(Result<String, ApiError>),
    ExtrasCommitted { applied: Vec<(usize, bool)>, error: Option<(String, ApiError)> },
    Iroh(Result<crate::api::types::IrohStatus, ApiError>),
    Progress(Result<Vec<crate::api::types::ScanProgressRow>, ApiError>),
}

/// Everything an op needs to run away from the UI state. Snapshotted at
/// dispatch time — the worker never sees the Wizard.
enum Job {
    Plain(Op),
    Folders(Vec<(usize, String, String)>),
    Admin { username: String, password: String, vpaths: Vec<String> },
    Extras(Vec<(usize, &'static str, bool)>),
}

/// The worker thread: one op at a time, results back over the channel. The
/// picker runs here too, so the UI stays live while a dialog is open.
fn spawn_worker() -> (Sender<(Arc<Client>, Job)>, Receiver<Done>) {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(Arc<Client>, Job)>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Done>();
    std::thread::spawn(move || {
        while let Ok((client, job)) = job_rx.recv() {
            let done = match job {
                Job::Plain(Op::Ping) => Done::Ping(client.ping().map(|_| {
                    client
                        .admin_directories()
                        .map(|dirs| dirs.into_iter().map(|(name, d)| (name, d.root)).collect())
                        .unwrap_or_default()
                })),
                Job::Plain(Op::PickNative) => Done::Picked(picker::pick_folder()),
                Job::Plain(Op::OpenBrowser(path)) | Job::Plain(Op::BrowseTo(path)) => {
                    Done::Browsed(local_list(&path))
                }
                Job::Plain(Op::Complete(dir)) => {
                    let listing = local_list(&dir);
                    Done::Completed { dir, listing }
                }
                Job::Plain(Op::Validate(path)) => {
                    let (check, canonical) = validate_path(&path);
                    Done::Validated { path, check, canonical }
                }
                Job::Plain(Op::LoadDone) => Done::Iroh(client.admin_iroh()),
                Job::Plain(Op::PollProgress) => Done::Progress(client.scan_progress()),
                // These three carry snapshots instead.
                Job::Plain(Op::CommitFolders | Op::CreateAdmin | Op::CommitExtras) => continue,
                Job::Folders(batch) => {
                    let mut committed = Vec::new();
                    let mut error = None;
                    for (i, path, name) in batch {
                        match client.admin_add_directory(&path, &name) {
                            Ok(_) => committed.push(i),
                            Err(e) => {
                                error = Some((name, e));
                                break;
                            }
                        }
                    }
                    Done::FoldersCommitted { committed, error }
                }
                Job::Admin { username, password, vpaths } => Done::AdminCreated(
                    client
                        .admin_create_user(&username, &password, &vpaths, true)
                        .and_then(|_| client.login_shared(&username, &password))
                        .map(|resp| resp.token),
                ),
                Job::Extras(batch) => {
                    let mut applied = Vec::new();
                    let mut error = None;
                    for (i, label, on) in batch {
                        let result = match i {
                            0 => client.admin_update_mode(if on { "stage" } else { "notify" }).map(|_| ()),
                            1 => client.admin_auto_boot_audio(on).map(|_| ()),
                            2 => client.admin_discovery_enabled(on).map(|_| ()),
                            _ => Ok(()),
                        };
                        match result {
                            Ok(()) => applied.push((i, on)),
                            Err(e) => {
                                error = Some((label.to_string(), e));
                                break;
                            }
                        }
                    }
                    Done::ExtrasCommitted { applied, error }
                }
            };
            if done_tx.send(done).is_err() {
                return;
            }
        }
    });
    (job_tx, done_rx)
}

/// How the loop ended.
enum Outcome {
    Quit,
}

pub(crate) struct Wizard {
    client: Arc<Client>,
    pub screen: Screen,
    pub modal: Modal,

    pub folders: Vec<Folder>,
    /// The KEYBOARD cursor — `None` until ↑/↓ is pressed. Mouse users
    /// never need it: every row action is directly clickable, so no row
    /// is highlighted by default.
    pub sel: Option<usize>,
    /// A rename in progress: the row and the draft. Independent of the
    /// keyboard cursor — a mouse rename never selects.
    pub editing: Option<(usize, Input)>,
    /// A row to scroll into view on the next draw (a fresh add).
    reveal: Option<usize>,
    /// Paths waiting for their local validation to be sent to the worker.
    pending_validate: Vec<String>,

    pub username: Input,
    pub password: Input,
    pub confirm: Input,
    pub field: LoginField,
    /// Continue without an account — the public-mode path.
    pub public: bool,

    /// Extras toggles: automatic updates (on), server audio, discovery.
    pub extras: [bool; 3],
    /// The KEYBOARD cursor over the opt-in cards — `None` until ↑/↓.
    pub extras_sel: Option<usize>,
    /// Which extras already reached the server (a failed batch retries the
    /// rest, and toggling one off after a failure just drops it).
    /// The value last APPLIED per extra — re-continue sends changes only.
    extras_done: [Option<bool>; 3],

    /// Quick Connect ticket, rendered; None while loading or when off.
    pub qr: Option<Vec<String>>,
    /// The same ticket as a real picture, for terminals that draw pixels.
    /// Built alongside `qr` — drawing tries this first and falls back to
    /// the half-block lines, which occupy the identical cell band.
    qr_art: Option<crate::tui::art::Art>,
    /// The terminal's pixel-protocol answer, probed once at startup by the
    /// same machinery that draws the player's covers. Disabled everywhere
    /// a real terminal didn't answer — tests, pipes, expect harnesses.
    graphics: crate::tui::graphics::Graphics,
    /// The wordmark as a picture, built once when the terminal can draw
    /// pixels — see [`logo_art`]. `None` falls back to the figlet.
    logo_art: Option<crate::tui::art::Art>,
    /// A fork of `graphics` for the wordmark: the encode cache holds ONE
    /// picture, and the Done page draws the logo and the code each frame.
    logo_gfx: crate::tui::graphics::Graphics,
    /// The Done page's keyboard cursor over its button list — `None`
    /// until ↑/↓, the kit's selection convention.
    done_sel: Option<usize>,
    /// Index into [`LANGS`] — what the top-left selector shows.
    lang: usize,
    pub qr_note: String,
    pub progress: String,

    /// The Done screen doubles as the `mstream-player qr` page. Standalone
    /// it drops the wizard chrome: no step counter, no scan-progress
    /// polling, a "Quick Connect" heading, and Close instead of Finish.
    standalone: bool,

    /// One line of status under the card: (text, is_error).
    pub note: Option<(String, bool)>,
    busy: Option<String>,
    queued: Option<Op>,
    /// An op is running on the worker; further queues wait (completions
    /// supersede each other instead).
    in_flight: bool,
    pending_complete: Option<String>,
    last_poll: Instant,

    /// The folders table's first visible row (wheel-scrollable).
    tscroll: usize,
    /// Last frame's selection — a change yanks the view to the selection.
    sel_anchor: Option<usize>,
    /// The kit's interaction surface: click/tip/bar registries, pointer,
    /// tooltip dwell, scrollbar capture and hold-repeat.
    ui: Surface<Act>,
}

impl Wizard {
    fn new(client: Client) -> Self {
        Wizard {
            client: Arc::new(client),
            screen: Screen::Folders,
            modal: Modal::None,
            folders: Vec::new(),
            sel: None,
            reveal: None,
            pending_validate: Vec::new(),
            editing: None,
            username: Input::default(),
            password: Input::default(),
            confirm: Input::default(),
            field: LoginField::Username,
            public: false,
            extras: [true, false, false],
            extras_sel: None,
            extras_done: [None; 3],
            qr: None,
            qr_art: None,
            graphics: crate::tui::graphics::Graphics::disabled(),
            logo_art: None,
            logo_gfx: crate::tui::graphics::Graphics::disabled(),
            done_sel: None,
            lang: 0,
            qr_note: String::new(),
            progress: String::new(),
            standalone: false,
            note: None,
            busy: None,
            queued: None,
            in_flight: false,
            pending_complete: None,
            last_poll: Instant::now(),
            tscroll: 0,
            sel_anchor: None,
            ui: Surface::new(),
        }
    }

    fn queue(&mut self, op: Op, busy: impl Into<String>) {
        self.queued = Some(op);
        self.busy = Some(busy.into());
    }

    // ── Folder naming rules ─────────────────────────────────────────────────

    fn add_folder(&mut self, path: String) {
        if self.folders.iter().any(|f| f.path == path) {
            self.note = Some((t!("note.folder_dup").to_string(), false));
            return;
        }
        self.folders.push(Folder {
            name: String::new(),
            path: path.clone(),
            named_by_user: false,
            committed: false,
            check: FolderCheck::Pending,
            canonical: None,
            nested_in: None,
        });
        self.pending_validate.push(path);
        self.reveal = Some(self.folders.len() - 1);
        self.note = None;
        sync_names(&mut self.folders);
    }

    /// Rows for the folders the server ALREADY has, from the boot-time
    /// ping. They arrive committed (Continue skips them, [X]/rename point
    /// at the admin panel) and user-named (resyncs never touch a server
    /// name), and their names join the dedup set so a new folder can
    /// never re-derive a vpath the server would reject as a duplicate.
    fn seed_folders(&mut self, existing: Vec<(String, String)>) {
        for (name, path) in existing {
            if self.folders.iter().any(|f| f.path == path || f.name == name) {
                continue;
            }
            self.folders.push(Folder {
                name,
                path: path.clone(),
                named_by_user: true,
                committed: true,
                check: FolderCheck::Pending,
                canonical: None,
                nested_in: None,
            });
            self.pending_validate.push(path);
        }
        sync_names(&mut self.folders);
    }

    /// Recompute which folders sit INSIDE another chosen folder, from
    /// canonical paths — run whenever a validation lands or a row goes.
    fn recheck_nesting(&mut self) {
        let canon: Vec<Option<String>> = self.folders.iter().map(|f| f.canonical.clone()).collect();
        for i in 0..self.folders.len() {
            let mut nested = None;
            if let Some(ci) = &canon[i] {
                for (j, cj) in canon.iter().enumerate() {
                    if i != j {
                        if let Some(cj) = cj {
                            if is_under(ci, cj) {
                                nested = Some(self.folders[j].path.clone());
                                break;
                            }
                        }
                    }
                }
            }
            self.folders[i].nested_in = nested;
        }
    }

    fn remove_at(&mut self, i: usize) {
        if i >= self.folders.len() {
            return;
        }
        let removed = self.folders.remove(i);
        if removed.committed {
            self.note = Some((
                t!("note.committed_remove", name = removed.name).to_string(),
                false,
            ));
        }
        // The keyboard cursor and an in-progress rename follow the shift.
        self.sel = match self.sel {
            _ if self.folders.is_empty() => None,
            Some(s) if s > i => Some(s - 1),
            Some(s) => Some(s.min(self.folders.len() - 1)),
            None => None,
        };
        self.editing = match self.editing.take() {
            Some((row, _)) if row == i => None,
            Some((row, draft)) if row > i => Some((row - 1, draft)),
            other => other,
        };
        sync_names(&mut self.folders);
        self.recheck_nesting();
    }

    // ── Screen-level input ──────────────────────────────────────────────────

    fn act(&mut self, act: Act) -> Option<Outcome> {
        match act {
            Act::RenameFolder(i) => {
                // Clicking the chip that's already being edited must not
                // clobber the draft.
                if self.editing.as_ref().is_some_and(|(row, _)| *row == i) {
                    return None;
                }
                self.finish_rename();
                if let Some(folder) = self.folders.get(i) {
                    if folder.committed {
                        self.note = Some((
                            t!("note.committed_rename").to_string(),
                            false,
                        ));
                    } else {
                        self.editing = Some((i, Input::new(folder.name.clone())));
                    }
                }
            }
            Act::BrowseNative => self.queue(Op::PickNative, "opening the folder picker…"),
            Act::TypePath => {
                self.modal = Modal::PathEntry(PathDraft::default());
                self.refresh_completion();
            }
            Act::RemoveFolder => {
                if let Some(s) = self.sel {
                    self.remove_at(s);
                }
            }
            Act::RemoveAt(i) => self.remove_at(i),
            Act::ContinueFolders => {
                self.finish_rename();
                if self.folders.is_empty() {
                    self.note = Some((t!("note.need_folder").to_string(), true));
                } else {
                    self.queue(Op::CommitFolders, t!("busy.committing_folders"));
                }
            }
            Act::Focus(field) => self.field = field,
            Act::CreateAdmin => match self.login_problem() {
                Some(problem) => self.note = Some((problem.to_string(), true)),
                None => self.queue(Op::CreateAdmin, t!("busy.creating_login")),
            },
            Act::BackToFolders => {
                // Back to add or adjust folders. Already-committed rows
                // stay put on the server; Continue re-commits only the
                // NEW ones (the batch filters on `committed`), and the
                // server queues its scan per added directory as always.
                self.screen = Screen::Folders;
                self.note = None;
            }
            Act::SkipLogin => self.modal = Modal::SkipWarning,
            Act::SkipCancel => self.modal = Modal::None,
            Act::SkipConfirm => {
                self.modal = Modal::None;
                self.public = true;
                self.note = None;
                self.screen = Screen::Done;
                self.queue(Op::LoadDone, t!("busy.fetching_qc"));
            }
            Act::Toggle(i) => {
                if let Some(on) = self.extras.get_mut(i) {
                    *on = !*on;
                }
            }
            Act::BackToExtras => {
                self.screen = Screen::Extras;
                self.note = None;
            }
            Act::ContinueExtras => self.queue(Op::CommitExtras, t!("busy.saving_extras")),
            Act::TableScroll(delta) => {
                self.tscroll = if delta < 0 {
                    self.tscroll.saturating_sub(1)
                } else {
                    self.tscroll.saturating_add(1)
                };
            }
            Act::PathCancel => self.modal = Modal::None,
            Act::TableScrollTo(pos) => self.tscroll = pos,
            Act::PathScrollTo(pos) => {
                if let Modal::PathEntry(draft) = &mut self.modal {
                    draft.scroll = pos;
                }
            }
            Act::PathScroll(delta) => {
                if let Modal::PathEntry(draft) = &mut self.modal {
                    draft.scroll = if delta < 0 {
                        draft.scroll.saturating_sub(1)
                    } else {
                        draft.scroll.saturating_add(1)
                    };
                }
            }
            Act::BrowseRow(i) => {
                if let Modal::Browser(b) = &mut self.modal {
                    b.sel = i.min(b.dirs.len().saturating_sub(1));
                }
            }
            Act::BrowseEnter => {
                if let Modal::Browser(b) = &self.modal {
                    if let Some(dir) = b.dirs.get(b.sel) {
                        let to = join_server_path(&b.path, dir);
                        self.queue(Op::BrowseTo(to), t!("busy.listing"));
                    }
                }
            }
            Act::BrowseUp => {
                if let Modal::Browser(b) = &self.modal {
                    if let Some(parent) = parent_server_path(&b.path) {
                        self.queue(Op::BrowseTo(parent), t!("busy.listing"));
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
            Act::OpenAppStore => self.open_link(&t!("done.label_appstore"), APP_STORE_URL),
            Act::OpenPlayStore => self.open_link(&t!("done.label_play"), PLAY_STORE_URL),
            Act::OpenAdmin => {
                let url = format!("{}/admin", self.client.server().trim_end_matches('/'));
                let label = t!("done.label_admin").to_string();
                self.open_link(&label, &url);
            }
            Act::OpenLanguage => self.modal = Modal::Language(self.lang),
            Act::SetLanguage(i) => self.set_language(i),
            Act::Finish => return Some(Outcome::Quit),
        }
        None
    }

    /// Whether the Done page lays out as two columns. CAPABILITY decides
    /// — a terminal that draws pixels gets the columns from the very
    /// first frame (wordmark up, right column complete) and the code
    /// pops into its slot when the ticket arrives. Deciding on the data
    /// instead painted one stacked, unstyled frame and then reflowed the
    /// whole page a beat later. The fixed-size half-block fallback keeps
    /// the stacked layout — it cannot fit a half column. No
    /// ground-ownership gate: unlike the logo, the code's picture
    /// carries its own white quiet zone and is correct on any
    /// background.
    fn done_two_column(&self) -> bool {
        self.graphics.protocol().is_some()
    }

    /// Switch the wizard's language. Everything re-renders through t!()
    /// next frame; every language but English is machine-translated and
    /// says so — in its own words — the moment it is chosen.
    fn set_language(&mut self, i: usize) {
        let Some((code, _)) = LANGS.get(i) else { return };
        rust_i18n::set_locale(code);
        self.lang = i;
        self.modal = Modal::None;
        self.note =
            (*code != "en").then(|| (t!("lang.machine_note").to_string(), false));
    }

    /// Open a store page in the browser; headless (or under the test
    /// seam), the note carries the URL so it can be copied instead.
    fn open_link(&mut self, label: &str, url: &str) {
        self.note = Some(if open_url(url) {
            (t!("note.link_opened", label = label).to_string(), false)
        } else {
            (t!("note.link_headless", label = label, url = url).to_string(), false)
        });
    }

    /// Queue a listing for the dir-part of the draft, if it changed.
    fn refresh_completion(&mut self) {
        if let Modal::PathEntry(draft) = &mut self.modal {
            draft.sel = None;
            draft.sel_anchor = None;
            draft.scroll = 0;
            // An empty input suggests nothing: listing a default dir here
            // made bare Tab fill in entries of the home — the completion
            // starts once there is something to complete.
            if draft.text.value().is_empty() {
                draft.listed_for.clear();
                draft.listed_path.clear();
                draft.entries.clear();
                draft.error = None;
                return;
            }
            // A leading `~` expands to the local home the moment it is
            // typed — synchronously, cursor keeping its distance from
            // the end.
            let starts_tilde = {
                let v = draft.text.value();
                v == "~" || v.starts_with("~/") || v.starts_with("~\\")
            };
            if starts_tilde {
                if let Some(home) = local_home() {
                    let old = draft.text.value().to_string();
                    let home = home.trim_end_matches(['/', '\\']).to_string();
                    // A bare `~` gains its separator, so the preview lands
                    // INSIDE the home instead of listing its parent.
                    let replacement = if old == "~" {
                        let sep = if home.contains('\\') { '\\' } else { '/' };
                        format!("{home}{sep}")
                    } else {
                        home
                    };
                    let new_text = old.replacen('~', &replacement, 1);
                    let from_end = old.chars().count() - draft.text.cursor();
                    let cursor = new_text.chars().count().saturating_sub(from_end);
                    draft.text = Input::new(new_text).with_cursor(cursor);
                }
            }
            // Collapse doubled separators — typing `/` right after an
            // expansion or completion that already ended with one is
            // natural (a leading pair survives for UNC paths).
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
            // Bare text (no separator yet) completes against the home.
            let dir = if dir == "~" {
                match local_home() {
                    Some(home) => format!("{}/", home.trim_end_matches(['/', '\\'])),
                    None => return,
                }
            } else {
                dir
            };
            if dir != draft.listed_for {
                draft.listed_for = dir.clone();
                draft.entries.clear();
                draft.error = None;
                self.queued = Some(Op::Complete(dir));
                // Quiet: no busy note for something that follows every
                // keystroke.
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

    fn finish_rename(&mut self) {
        if let Some((row, draft)) = self.editing.take() {
            if let Some(folder) = self.folders.get_mut(row) {
                let clean = sanitize_name(draft.value());
                if !clean.is_empty() {
                    folder.name = clean;
                    folder.named_by_user = true;
                }
            }
            sync_names(&mut self.folders);
        }
    }

    fn login_problem(&self) -> Option<String> {
        if self.username.value().trim().is_empty() {
            return Some(t!("login_problem.username").to_string());
        }
        if self.password.value().is_empty() {
            return Some(t!("login_problem.password").to_string());
        }
        if self.password.value() != self.confirm.value() {
            return Some(t!("login_problem.mismatch").to_string());
        }
        None
    }

    // ── Server calls ────────────────────────────────────────────────────────
    //
    // Every op runs on the worker thread (below) — the loop stays live, keys
    // keep landing, and a slow server (or an open picker dialog) can never
    // freeze the UI. The first cut ran ops synchronously between draws and a
    // boot-busy server blocked the loop for eight seconds; the buffered
    // keystrokes then replayed into the wrong screens.

    /// Hand the queued op to the worker. Ops are single-flight: while one is
    /// in flight the UI shows its busy note and further queues are ignored
    /// (completion listings replace instead — typing outruns the network).
    fn dispatch_queued(&mut self, to_worker: &Sender<(Arc<Client>, Job)>) {
        for path in self.pending_validate.drain(..) {
            let _ = to_worker.send((self.client.clone(), Job::Plain(Op::Validate(path))));
        }
        if self.in_flight {
            // A newer completion wish supersedes a stale one; anything else
            // waits its turn behind the running op.
            match self.queued.take() {
                Some(Op::Complete(dir)) => self.pending_complete = Some(dir),
                other => self.queued = other,
            }
            return;
        }
        let Some(op) = self.queued.take() else { return };
        // Snapshot whatever the op needs — the worker never sees the Wizard.
        let job = match op {
            Op::CommitFolders => Job::Folders(
                self.folders
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !f.committed)
                    .map(|(i, f)| (i, f.path.clone(), f.name.clone()))
                    .collect(),
            ),
            Op::CreateAdmin => Job::Admin {
                username: self.username.value().to_string(),
                password: self.password.value().to_string(),
                vpaths: self.folders.iter().map(|f| f.name.clone()).collect(),
            },
            Op::CommitExtras => Job::Extras(
                [(0usize, "updates"), (1, "server audio"), (2, "discovery")]
                    .into_iter()
                    .filter(|(i, _)| self.extras_done[*i] != Some(self.extras[*i]))
                    .map(|(i, label)| (i, label, self.extras[i]))
                    .collect(),
            ),
            other => Job::Plain(other),
        };
        self.in_flight = true;
        if to_worker.send((self.client.clone(), job)).is_err() {
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
            Done::Ping(Ok(existing)) => {
                self.note = None;
                self.seed_folders(existing);
            }
            Done::Ping(Err(e)) => self.fail("could not reach the server", e),
            Done::Picked(picker::Pick::Folder(path)) => {
                self.add_folder(path.display().to_string());
            }
            Done::Picked(picker::Pick::Cancelled) => {}
            Done::Picked(picker::Pick::Unavailable(why)) => {
                self.note = Some((
                    t!("note.no_picker", why = why).to_string(),
                    false,
                ));
                let start = local_home().unwrap_or_else(|| "/".to_string());
                self.queue(Op::OpenBrowser(start), t!("busy.listing"));
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
                if let Modal::PathEntry(draft) = &mut self.modal {
                    if draft.listed_for == dir {
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
            }
            Done::Validated { path, check, canonical } => {
                let Some(i) = self.folders.iter().position(|f| f.path == path) else {
                    return; // the row went away while the stat ran
                };
                // Another spelling of a folder already chosen (trailing
                // slash, symlink, case): drop the newcomer, say why.
                if let Some(c) = &canonical {
                    if let Some(j) = self
                        .folders
                        .iter()
                        .position(|f| f.canonical.as_deref() == Some(c.as_str()))
                    {
                        if j != i {
                            let name = self.folders[j].name.clone();
                            self.remove_at(i);
                            self.note = Some((
                                t!("note.removed_same", path = path, name = name).to_string(),
                                false,
                            ));
                            return;
                        }
                    }
                }
                self.folders[i].check = check;
                self.folders[i].canonical = canonical;
                self.recheck_nesting();
                let name = &self.folders[i].name;
                match check {
                    FolderCheck::Missing => {
                        self.note = Some((
                            format!("{name}: not found on this machine — the server has final say at commit"),
                            true,
                        ));
                    }
                    FolderCheck::NotADir => {
                        self.note =
                            Some((format!("{name}: that is a file, not a folder"), true));
                    }
                    FolderCheck::Unreadable => {
                        self.note = Some((
                            format!("{name}: no permission to read it"),
                            true,
                        ));
                    }
                    FolderCheck::Ok | FolderCheck::Pending => {
                        if self.folders[i].nested_in.is_some() {
                            self.note = Some((
                                format!("{name} is inside another chosen folder — it would be scanned twice"),
                                true,
                            ));
                        }
                    }
                }
            }
            Done::FoldersCommitted { committed, error } => {
                for i in committed {
                    if let Some(folder) = self.folders.get_mut(i) {
                        folder.committed = true;
                    }
                }
                match error {
                    Some((name, e)) => self.fail(&t!("note.could_not_add", name = name), e),
                    None => {
                        self.note = None;
                        self.screen = Screen::Extras;
                    }
                }
            }
            Done::AdminCreated(Ok(token)) => {
                self.public = false;
                // Creating the first user closed the open-admin window; swap
                // in a token-carrying client so the rest of the wizard (and
                // any job the worker gets from now on) stays authorized.
                match Client::new(&self.client.server()) {
                    Ok(fresh) => self.client = Arc::new(fresh.with_token(Some(token))),
                    Err(e) => return self.fail(&t!("note.keep_session"), e),
                }
                self.remember_session();
                self.note = None;
                self.screen = Screen::Done;
                self.queue(Op::LoadDone, t!("busy.fetching_qc"));
            }
            Done::AdminCreated(Err(e)) => self.fail(&t!("note.create_login_failed"), e),
            Done::ExtrasCommitted { applied, error } => {
                for (i, on) in applied {
                    self.extras_done[i] = Some(on);
                }
                match error {
                    Some((label, e)) => {
                        let label = match label {
                            l if l == "updates" => t!("extras_label.updates").to_string(),
                            l if l == "server audio" => t!("extras_label.audio").to_string(),
                            _ => t!("extras_label.discovery").to_string(),
                        };
                        self.fail(&t!("note.extras_failed", label = label), e)
                    }
                    None => {
                        self.note = None;
                        self.screen = Screen::Login;
                    }
                }
            }
            Done::Iroh(result) => {
                match result {
                    Ok(status) => match status.qr.as_deref() {
                        Some(ticket) if status.enabled => match qr_lines(ticket) {
                            Some(lines) => {
                                self.qr = Some(lines);
                                self.qr_art = qr_art(ticket);
                                self.qr_note =
                                    t!("qc.scan_app").to_string();
                            }
                            None => {
                                self.qr_note =
                                    t!("qc.on_panel").to_string();
                            }
                        },
                        _ if !status.enabled => {
                            self.qr_note =
                                t!("qc.off_panel").to_string();
                        }
                        _ => {
                            self.qr_note =
                                t!("qc.starting").to_string();
                        }
                    },
                    Err(e) => self.fail(&t!("note.qc_state_failed"), e),
                }
                // Scan progress is wizard chrome — the standalone QR page
                // has no scan of its own to narrate.
                if !self.standalone {
                    self.queue(Op::PollProgress, "");
                }
            }
            Done::Progress(rows) => {
                self.last_poll = Instant::now();
                match rows {
                    Ok(rows) if rows.is_empty() => {
                        self.progress = t!("scan.complete").to_string();
                    }
                    Ok(rows) => {
                        let row = &rows[0];
                        let more =
                            if rows.len() > 1 { t!("scan.more_queued").to_string() } else { String::new() };
                        self.progress = match row.pct {
                            Some(pct) => t!(
                                "scan.progress_pct",
                                vpath = row.vpath,
                                pct = pct,
                                scanned = row.scanned,
                                more = more
                            )
                            .to_string(),
                            None => t!(
                                "scan.progress",
                                vpath = row.vpath,
                                scanned = row.scanned,
                                more = more
                            )
                            .to_string(),
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
                config::touch_server(&mut cfg, &server, Some(self.username.value().to_string()));
                if let Err(e) = config::save(&cfg) {
                    self.note = Some((t!("note.settings_save_failed", err = e).to_string(), false));
                }
            }
            Err(e) => self.note = Some((t!("note.settings_not_saved", err = e).to_string(), false)),
        }
        match config::load_credentials() {
            Ok(mut creds) => {
                config::store_token(&mut creds, &server, self.client.token().map(String::from));
                if let Err(e) = config::save_credentials(&creds) {
                    self.note = Some((t!("note.session_save_failed", err = e).to_string(), false));
                }
            }
            Err(e) => self.note = Some((t!("note.session_not_saved", err = e).to_string(), false)),
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

/// A path fitted into `width` cells with its TAIL kept — `…` plus the
/// last `width − 1` characters when it overflows. Returns whether it
/// clipped, so the caller can hang the full path on a tooltip.
pub(crate) fn ellipsize_path_start(path: &str, width: usize) -> (String, bool) {
    let count = path.chars().count();
    if count <= width || width == 0 {
        return (path.to_string(), false);
    }
    let tail: String = path.chars().skip(count - width.saturating_sub(1)).collect();
    (format!("…{tail}"), true)
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
/// The wizard user's home directory — the meaning of a typed `~`.
/// (Completion is LOCAL: the wizard is a same-machine first-run tool,
/// and its primary affordance — the native picker — already speaks the
/// local filesystem. The server validates every folder at commit.)
fn local_home() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))
}

/// List a LOCAL directory's subdirectories (symlinks resolved), sorted
/// case-insensitively — run on the worker: a dead network mount can
/// hang `read_dir`, and the UI never blocks.
fn local_list(dir: &str) -> Result<crate::api::types::DirListing, String> {
    use crate::api::types::{DirEntry, DirListing};
    if dir.is_empty() {
        return Err("nothing to list".to_string());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut names: Vec<String> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let kind = entry.file_type().ok()?;
            let is_dir =
                kind.is_dir() || (kind.is_symlink() && std::fs::metadata(entry.path()).ok()?.is_dir());
            is_dir.then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    let trimmed = dir.trim_end_matches(['/', '\\']);
    let path =
        if trimmed.is_empty() || trimmed.ends_with(':') { dir.to_string() } else { trimmed.to_string() };
    Ok(DirListing {
        path,
        directories: names.into_iter().map(|name| DirEntry { name }).collect(),
        files: Vec::new(),
    })
}

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

/// The pairing ticket as a real picture — black modules on their own white
/// quiet zone, PNG-encoded so [`tui::graphics`] can draw it through
/// whatever pixel protocol the terminal answered. Rendered generously
/// (8 px a module) so the protocol only ever scales it down.
fn qr_art(data: &str) -> Option<crate::tui::art::Art> {
    const SCALE: u32 = 8;
    const QUIET: u32 = 4; // modules of quiet zone each side, per the QR spec
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let modules = code.width() as u32;
    let side = (modules + QUIET * 2) * SCALE;
    let mut img = image::GrayImage::from_pixel(side, side, image::Luma([255u8]));
    for (i, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            let mx = (i as u32 % modules + QUIET) * SCALE;
            let my = (i as u32 / modules + QUIET) * SCALE;
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    img.put_pixel(mx + dx, my + dy, image::Luma([0u8]));
                }
            }
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    crate::tui::art::decode(&png)
}

/// The pairing ticket as unicode half-blocks, light-on-dark so it scans off
/// a dark terminal. No rendered quiet zone: an inverted code's quiet zone
/// must be DARK, and the page already surrounds the block with empty dark
/// cells wider than the spec's four modules — rendering it again only cost
/// four rows and columns this render can't spare.
fn qr_lines(data: &str) -> Option<Vec<String>> {
    use qrcode::render::unicode::Dense1x2;
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let rendered = code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(false)
        .build();
    Some(rendered.lines().map(str::to_string).collect())
}

/// Hand a URL to the OS's opener, detached. `MSTREAM_NO_OPEN` is the test
/// seam — harnesses assert the note instead of popping browsers. Returns
/// whether an opener was actually launched; headless callers put the URL
/// itself in the note so it can be copied over SSH.
fn open_url(url: &str) -> bool {
    if std::env::var("MSTREAM_NO_OPEN").is_ok_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    #[cfg(target_os = "macos")]
    let launched = std::process::Command::new("open").arg(url).spawn().is_ok();
    #[cfg(target_os = "windows")]
    let launched = std::process::Command::new("cmd")
        .args(["/c", "start", ""])
        .arg(url)
        .spawn()
        .is_ok();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let launched = std::process::Command::new("xdg-open").arg(url).spawn().is_ok();
    launched
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
    wizard.lang = boot_language();
    wizard.queue(Op::Ping, t!("busy.reaching"));
    run_tui(wizard)
}

#[derive(Args)]
pub struct QrArgs {
    /// The server to ask — defaults to the saved session's server
    #[arg(long)]
    server: Option<String>,

    /// Auth token override (default: the saved session's token)
    #[arg(long, hide = true)]
    token: Option<String>,
}

/// The Done screen alone, as its own command: the Quick Connect QR for
/// whenever someone wants to point a phone at it again.
pub fn run_qr(args: QrArgs) -> i32 {
    let client = match Client::resolve(args.server.as_deref(), args.token.as_deref()) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            return 1;
        }
    };
    let mut wizard = Wizard::new(client);
    wizard.lang = boot_language();
    wizard.standalone = true;
    wizard.screen = Screen::Done;
    wizard.queue(Op::LoadDone, t!("busy.fetching_qc"));
    run_tui(wizard)
}

/// The shared terminal session around both entries: ground lease, pixel
/// probe, mouse capture, pointer contract, event loop, teardown.
fn run_tui(mut wizard: Wizard) -> i32 {
    let (to_worker, from_worker) = spawn_worker();

    // Claim the window background BEFORE ratatui takes the terminal — the
    // OSC 11 query runs its own raw-mode transaction on the tty. Owning
    // the default background is what colors the margin pixels the terminal
    // reserves around the cell grid; cell fills alone would leave the
    // fixed ground inside a border of the user's own background.
    let claim = theme::acquire_ground();
    let ground_guard = GroundGuard;

    // Ask about pixels while nothing else is talking on stdio — the probe
    // runs its own bounded raw-mode transaction, and ratatui::init below
    // re-asserts terminal state behind it (the player's own ordering).
    wizard.graphics = crate::tui::graphics::Graphics::probe();
    if wizard.graphics.protocol().is_some() {
        wizard.logo_art = theme::th().ground_rgb.and_then(logo_art);
        wizard.logo_gfx = wizard.graphics.fork();
    }

    let mut terminal = ratatui::init();
    // A wizard whose buttons cannot be clicked is half a wizard; like the
    // player, a terminal that refuses mouse reports still works by keys.
    let mouse_on = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    if let Some(seq) = claim {
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(seq));
    }
    // Announce the arrow base — without this, terminals keep their text
    // beam until the pointer first crosses a clickable.
    set_pointer_shape(false, mouse_on);
    let outcome = event_loop(&mut terminal, &mut wizard, mouse_on, &to_worker, &from_worker);
    if mouse_on {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(POINTER_RESET));
    }
    ratatui::restore();
    // Hand the background back before anything adaptive (the player, the
    // shell) takes the screen.
    drop(ground_guard);

    match outcome {
        Ok(Outcome::Quit) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    wizard: &mut Wizard,
    mouse_on: bool,
    to_worker: &Sender<(Arc<Client>, Job)>,
    from_worker: &Receiver<Done>,
) -> std::io::Result<Outcome> {
    let mut hand = false;
    loop {
        terminal.draw(|frame| render(frame, wizard))?;

        // Fold in whatever the worker finished, then hand it the next op.
        loop {
            match from_worker.try_recv() {
                Ok(done) => wizard.apply(done),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    wizard.note =
                        Some((t!("note.worker_gone").to_string(), true));
                    break;
                }
            }
        }
        wizard.dispatch_queued(to_worker);

        // The hand cursor follows whether the pointer is over anything
        // clickable in the frame just drawn.
        let over = wizard.ui.hovering_clickable();
        if over != hand {
            hand = over;
            set_pointer_shape(hand, mouse_on);
        }

        // A held scrollbar arrow keeps stepping until the button lifts.
        if let Some(act) = wizard.ui.hold_action() {
            wizard.act(act);
        }

        // Tooltip dwell. Tips can't leak through modals — render drops
        // the base registries while one is up, so whatever is registered
        // belongs to the layer on top.
        wizard.ui.dwell_tick();

        if wizard.screen != Screen::Folders
            && !wizard.standalone
            && wizard.queued.is_none()
            && !wizard.in_flight
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
                    // Typing dismisses a tooltip (the dwell re-arms if the
                    // pointer just sits there, like native tooltips).
                    wizard.ui.dismiss_tooltip();
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        return Ok(Outcome::Quit);
                    }
                    if let Some(outcome) = handle_key(wizard, key) {
                        return Ok(outcome);
                    }
                }
                TermEvent::Mouse(mouse) => {
                    let at = Position { x: mouse.column, y: mouse.row };
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if !wizard.ui.begin_press(at) {
                                continue;
                            }
                            // Blur commits an in-progress rename: a click
                            // anywhere outside the active chip ends the
                            // edit (Enter's semantics), then the click
                            // proceeds as normal.
                            if let Some((row, _)) = &wizard.editing {
                                let row = *row;
                                let on_chip = wizard.ui.clicks.iter().any(|(rect, act)| {
                                    *act == Act::RenameFolder(row) && rect.contains(at)
                                });
                                if !on_chip {
                                    wizard.finish_rename();
                                }
                            }
                            if let Some(act) = wizard.ui.hit(at) {
                                if let Some(outcome) = wizard.act(act) {
                                    return Ok(outcome);
                                }
                            }
                            // A press on a scrollbar arms its interaction
                            // (endcaps hold-repeat, the track a thumb drag).
                            wizard.ui.arm_bars(at);
                        }
                        // A scrollbar interaction CAPTURES the mouse:
                        // while an arrow is held or the thumb dragged,
                        // sub-cell hand tremor must not retarget hover
                        // onto whatever sits beside the 1-cell bar —
                        // and terminals differ on whether mid-press
                        // motion arrives as Drag or plain Moved, so
                        // BOTH honor the capture.
                        MouseEventKind::Moved => {
                            wizard.ui.motion(at);
                        }
                        MouseEventKind::Drag(_) => {
                            wizard.ui.motion(at);
                            if let Some(act) = wizard.ui.drag_action(at) {
                                wizard.act(act);
                            }
                        }
                        MouseEventKind::Up(_) => {
                            wizard.ui.release();
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            wizard.ui.pointer = Some(at);
                            let up = mouse.kind == MouseEventKind::ScrollUp;
                            if let Modal::PathEntry(draft) = &mut wizard.modal {
                                draft.scroll = if up {
                                    draft.scroll.saturating_sub(1)
                                } else {
                                    draft.scroll.saturating_add(1)
                                };
                            } else if wizard.screen == Screen::Folders
                                && matches!(wizard.modal, Modal::None)
                            {
                                wizard.tscroll = if up {
                                    wizard.tscroll.saturating_sub(1)
                                } else {
                                    wizard.tscroll.saturating_add(1)
                                };
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

/// Tab (or Right from the end of the line): accept the picked suggestion,
/// the single match, or extend to the longest common prefix — and when
/// that gains nothing, start cycling.
fn complete_path(wizard: &mut Wizard) {
    let Modal::PathEntry(draft) = &mut wizard.modal else { return };
    let suggestions = draft.suggestions();
    if let Some(i) = draft.sel {
        wizard.accept_suggestion(i);
    } else if suggestions.len() == 1 {
        wizard.accept_suggestion(0);
    } else if !suggestions.is_empty() {
        let (_, partial) = split_input(draft.text.value());
        let lcp = common_prefix(&suggestions);
        if lcp.chars().count() > partial.chars().count() {
            let keep = draft.text.value().chars().count() - partial.chars().count();
            let extended =
                draft.text.value().chars().take(keep).collect::<String>() + &lcp;
            draft.text = Input::new(extended);
            // Same dir-part, narrower partial — no re-list.
            draft.sel = None;
        } else {
            draft.sel = Some(0);
        }
    }
}

fn handle_key(wizard: &mut Wizard, key: KeyEvent) -> Option<Outcome> {
    let code = key.code;
    // Text entry captures everything printable first.
    match &mut wizard.modal {
        Modal::PathEntry(draft) => {
            let at_end = draft.text.cursor() == draft.text.value().chars().count();
            match code {
                KeyCode::Esc => wizard.modal = Modal::None,
                KeyCode::Enter => {
                    let path = draft.text.value().trim().to_string();
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
                KeyCode::Up | KeyCode::BackTab => {
                    let n = draft.suggestions().len();
                    if n > 0 {
                        draft.sel = Some(draft.sel.map_or(n - 1, |i| (i + n - 1) % n));
                    }
                }
                KeyCode::Tab => complete_path(wizard),
                // Right completes only from the END of the line (fish
                // behavior) — anywhere else it is the editor's cursor key.
                KeyCode::Right if at_end => complete_path(wizard),
                _ => {
                    // The line editor owns the rest: chars, Backspace,
                    // Delete, ←/→, Home/End, the ctrl word ops.
                    if draft
                        .text
                        .handle_event(&TermEvent::Key(key))
                        .is_some_and(|change| change.value)
                    {
                        wizard.refresh_completion();
                    }
                }
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
        Modal::Language(sel) => {
            return match code {
                KeyCode::Esc => {
                    wizard.modal = Modal::None;
                    None
                }
                KeyCode::Up => {
                    *sel = sel.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    *sel = (*sel + 1).min(LANGS.len() - 1);
                    None
                }
                KeyCode::Enter => {
                    let choice = *sel;
                    wizard.act(Act::SetLanguage(choice))
                }
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

    if let Some((_, draft)) = &mut wizard.editing {
        match code {
            KeyCode::Esc => {
                wizard.editing = None;
            }
            KeyCode::Enter => wizard.finish_rename(),
            // The vpath charset (a-z 0-9 dash) is enforced at the gate:
            // legal characters fold to lowercase and reach the editor,
            // everything else typed is dropped. Non-character keys —
            // ←/→, Home/End, Backspace, Delete, the ctrl word ops —
            // pass straight through.
            KeyCode::Char(c) if !c.is_ascii_alphanumeric() && c != '-' => {}
            _ => {
                let key = match code {
                    KeyCode::Char(c) => {
                        KeyEvent::new(KeyCode::Char(c.to_ascii_lowercase()), key.modifiers)
                    }
                    _ => key,
                };
                draft.handle_event(&TermEvent::Key(key));
            }
        }
        return None;
    }

    match wizard.screen {
        Screen::Folders => match code {
            // ↑/↓ are the ONLY way a row gets highlighted: the first
            // press picks up the cursor (↓ from the top, ↑ from the
            // bottom), Esc puts it away again.
            KeyCode::Up => {
                let n = wizard.folders.len();
                if n > 0 {
                    wizard.sel = Some(wizard.sel.map_or(n - 1, |s| s.saturating_sub(1)));
                }
                None
            }
            KeyCode::Down => {
                let n = wizard.folders.len();
                if n > 0 {
                    wizard.sel = Some(wizard.sel.map_or(0, |s| (s + 1).min(n - 1)));
                }
                None
            }
            KeyCode::Esc => {
                wizard.sel = None;
                None
            }
            KeyCode::Enter => match wizard.sel {
                Some(s) => wizard.act(Act::RenameFolder(s)),
                None => None,
            },
            KeyCode::Char('b') => wizard.act(Act::BrowseNative),
            KeyCode::Char('t') => wizard.act(Act::TypePath),
            KeyCode::Char('l') => wizard.act(Act::OpenLanguage),
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
            KeyCode::Esc => wizard.act(Act::BackToExtras),
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                wizard.act(Act::SkipLogin)
            }
            // The focused field's line editor takes everything else —
            // chars, Backspace, Delete, ←/→, Home/End, the ctrl ops.
            _ => {
                let field = match wizard.field {
                    LoginField::Username => &mut wizard.username,
                    LoginField::Password => &mut wizard.password,
                    LoginField::Confirm => &mut wizard.confirm,
                };
                field.handle_event(&TermEvent::Key(key));
                None
            }
        },
        Screen::Extras => match code {
            KeyCode::Up => {
                let n = wizard.extras.len();
                wizard.extras_sel =
                    Some(wizard.extras_sel.map_or(n - 1, |s| s.saturating_sub(1)));
                None
            }
            KeyCode::Down => {
                let n = wizard.extras.len();
                wizard.extras_sel = Some(wizard.extras_sel.map_or(0, |s| (s + 1).min(n - 1)));
                None
            }
            KeyCode::Char(' ') | KeyCode::Enter => match wizard.extras_sel {
                Some(i) => wizard.act(Act::Toggle(i)),
                None => None,
            },
            KeyCode::Esc => wizard.act(Act::BackToFolders),
            KeyCode::Char('c') => wizard.act(Act::ContinueExtras),
            KeyCode::Char('q') => Some(Outcome::Quit),
            _ => None,
        },
        Screen::Done => match code {
            KeyCode::Down => {
                wizard.done_sel =
                    Some(wizard.done_sel.map_or(0, |i| (i + 1).min(DONE_ACTIONS - 1)));
                None
            }
            KeyCode::Up => {
                wizard.done_sel =
                    Some(wizard.done_sel.map_or(DONE_ACTIONS - 1, |i| i.saturating_sub(1)));
                None
            }
            KeyCode::Enter => match wizard.done_sel {
                Some(i) => wizard.act(done_action(i)),
                None => None,
            },
            KeyCode::Char('f') | KeyCode::Char('q') | KeyCode::Esc => wizard.act(Act::Finish),
            _ => None,
        },
    }
}

// ── Drawing ──────────────────────────────────────────────────────────────────

fn render(frame: &mut Frame, wizard: &mut Wizard) {
    wizard.ui.begin_frame();
    let area = frame.area();
    // The fixed scheme paints its own ground — but only when the terminal
    // granted OSC 11 ownership of the whole window, margins included
    // (all or nothing: cell fills alone would sit inside a border of the
    // user's background). Body text drawn with no explicit fg inherits
    // `text` from this fill; without the fill it stays the terminal's
    // default, which is the readable choice on an unknown ground.
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            Block::default().style(Style::default().bg(ground).fg(th().text)),
            area,
        );
    }
    if area.width < 58 || area.height < 20 {
        frame.render_widget(
            Paragraph::new(t!("resize").to_string()).style(dim()),
            area,
        );
        return;
    }

    // The centered column everything lives in — except the Done page,
    // which owns the whole width (its columns anchor LEFT) and, having no
    // rule or bottom bar, most of the height.
    let column = if wizard.screen == Screen::Done {
        Rect { x: 2, y: 2, width: area.width.saturating_sub(4), height: area.height.saturating_sub(6) }
    } else {
        let width = COLUMN.min(area.width.saturating_sub(4));
        Rect { x: (area.width - width) / 2, y: 2, width, height: area.height.saturating_sub(8) }
    };

    // A modal makes the screen beneath INERT: the base draw sees no
    // pointer (so nothing under the modal hovers), and every rect it
    // registered is dropped before the modal draws — only the modal's
    // own controls exist while it is up.
    let modal_open = !matches!(wizard.modal, Modal::None);
    let live_pointer = wizard.ui.pointer;
    if modal_open {
        wizard.ui.pointer = None;
    }

    match wizard.screen {
        Screen::Folders => draw_folders(frame, wizard, column),
        Screen::Login => draw_login(frame, wizard, column),
        Screen::Extras => draw_extras(frame, wizard, column),
        Screen::Done => draw_done(frame, wizard, column),
    }

    // The language selector, top-left on the folders screen — the mirror
    // of the step counter. Everything else re-renders through t!() the
    // moment it changes.
    if wizard.screen == Screen::Folders {
        let label = format!("▾ {}", LANGS[wizard.lang].1);
        let chip = Rect { x: 2, y: 0, width: label.chars().count() as u16, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| chip.contains(p));
        let style = if hovered { Style::default().fg(th().bright) } else { dim() };
        frame.render_widget(Paragraph::new(Span::styled(label, style)), chip);
        wizard.ui.click(chip, Act::OpenLanguage);
        wizard.ui.tip(chip, t!("lang.chip_tip"));
    }

    // Step counter, top-right — the chrome's only top element. The
    // standalone QR page is not a step of anything.
    if !wizard.standalone {
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{} / 4", wizard.screen.step()), dim()))
                .alignment(Alignment::Right),
            Rect { x: 2, y: 0, width: area.width.saturating_sub(4), height: 1 },
        );
    }

    // Status note (errors, busy) above the keyboard tips. The Done page
    // has no rule or bar, so its two lines sit at the bottom edge.
    let (note_y, tips_y) = if wizard.screen == Screen::Done {
        (area.height.saturating_sub(3), area.height.saturating_sub(2))
    } else {
        (area.height.saturating_sub(6), area.height.saturating_sub(5))
    };
    if let Some((text, is_err)) = wizard.note.clone() {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        let note_area = Rect { x: 2, y: note_y, width: area.width - 4, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(text, style)), note_area);
    }
    if let Some(busy) = wizard.busy.clone() {
        let busy_area = Rect { x: 2, y: note_y, width: area.width - 4, height: 1 };
        frame.render_widget(Paragraph::new(Span::styled(busy, accent())), busy_area);
    }

    // Keyboard tips, left, directly above the gold rule.
    let tips = Rect { x: 2, y: tips_y, width: area.width - 4, height: 1 };
    frame.render_widget(Paragraph::new(Span::styled(footer_hint(wizard), dim())), tips);

    // The Done page ends here: no horizontal rule, no bottom bar — its
    // vertical rule and in-column scan status carry the structure.
    if wizard.screen == Screen::Done {
        return;
    }

    // The one gold rule, with the 3-row bottom bar under it: scan widget on
    // the left (empty until a scan is actually running — folders commit on
    // Continue, so that is the NEXT screen at the earliest), the screen's
    // forward action on the right as the kit's tall primary block.
    let rule = Rect { x: 2, y: area.height.saturating_sub(4), width: area.width - 4, height: 1 };
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::default().fg(th().gold))),
        rule,
    );
    let bar = Rect { x: 2, y: area.height.saturating_sub(3), width: area.width - 4, height: 3 };
    if !wizard.progress.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(wizard.progress.clone(), Style::default().fg(th().ok))),
            Rect { x: bar.x, y: bar.y + 1, width: bar.width.saturating_sub(20), height: 1 },
        );
    }
    if wizard.screen == Screen::Folders {
        // Disabled until there is something to continue with; the tooltip
        // carries the why (the kit's one exception to disabled inertness).
        let enabled = !wizard.folders.is_empty();
        let label = if enabled {
            t!("folders.continue_enabled").to_string()
        } else {
            t!("folders.continue_disabled").to_string()
        };
        let x = bar.right().saturating_sub(label.chars().count() as u16 + 6);
        let rect = kit::tall_button(
            frame,
            &mut wizard.ui,
            Rect { x, y: bar.y, width: bar.width, height: 3 },
            &label,
            enabled,
            Act::ContinueFolders,
        );
        if !enabled {
            wizard.ui.tip(rect, t!("folders.tip_continue_disabled"));
        }
    }
    if wizard.screen == Screen::Extras {
        let label = t!("extras.continue").to_string();
        let x = bar.right().saturating_sub(label.chars().count() as u16 + 6);
        let rect = kit::tall_button(
            frame,
            &mut wizard.ui,
            Rect { x, y: bar.y, width: bar.width, height: 3 },
            &label,
            true,
            Act::ContinueExtras,
        );
        let back_label = t!("extras.back").to_string();
        let back_x = rect.x.saturating_sub(back_label.chars().count() as u16 + 6 + 2);
        let back = kit::tall_secondary(
            frame,
            &mut wizard.ui,
            Rect { x: back_x, y: bar.y, width: bar.width, height: 3 },
            &back_label,
            Act::BackToFolders,
        );
        wizard.ui.tip(back, t!("extras.tip_back"));
    }
    if wizard.screen == Screen::Login {
        let label = t!("login.create").to_string();
        let x = bar.right().saturating_sub(label.chars().count() as u16 + 6);
        let rect = kit::tall_button(
            frame,
            &mut wizard.ui,
            Rect { x, y: bar.y, width: bar.width, height: 3 },
            &label,
            true,
            Act::CreateAdmin,
        );
        let back_label = t!("login.back").to_string();
        let back_x = rect.x.saturating_sub(back_label.chars().count() as u16 + 6 + 2);
        let back = kit::tall_secondary(
            frame,
            &mut wizard.ui,
            Rect { x: back_x, y: bar.y, width: bar.width, height: 3 },
            &back_label,
            Act::BackToExtras,
        );
        wizard.ui.tip(back, t!("login.tip_back"));
    }

    if modal_open {
        wizard.ui.pointer = live_pointer;
        wizard.ui.clear_registries();
    }
    match wizard.modal.clone() {
        Modal::None => {}
        Modal::SkipWarning => draw_skip_warning(frame, wizard, area),
        Modal::Browser(browse) => draw_browser(frame, wizard, area, &browse),
        Modal::PathEntry(draft) => draw_path_entry(frame, wizard, area, &draft),
        Modal::Language(sel) => draw_language(frame, wizard, area, sel),
    }

    // The tooltip draws last — over everything, once the dwell matures.
    if let Some((target, text)) = wizard.ui.ripe_tooltip() {
        kit::draw_tooltip(frame, area, target, text);
    }
}

fn footer_hint(wizard: &Wizard) -> String {
    match (&wizard.modal, wizard.screen) {
        (Modal::Browser(_), _) => t!("hint.browser"),
        (Modal::PathEntry(_), _) => t!("hint.path"),
        (Modal::SkipWarning, _) => t!("hint.skip"),
        (Modal::Language(_), _) => t!("lang.hint"),
        (_, Screen::Folders) => match (wizard.folders.is_empty(), wizard.sel) {
            // No rows: only the ways to add one (and the language chip).
            (true, _) => t!("hint.folders_empty"),
            // Rows, cursor stowed: how to pick one up, and continue.
            (false, None) => t!("hint.folders_rows"),
            // A row under the cursor: the full set.
            (false, Some(_)) => t!("hint.folders_selected"),
        },
        (_, Screen::Login) => t!("hint.login"),
        (_, Screen::Extras) => match wizard.extras_sel {
            None => t!("hint.extras"),
            Some(_) => t!("hint.extras_selected"),
        },
        (_, Screen::Done) if wizard.standalone => t!("hint.done_qr"),
        (_, Screen::Done) => t!("hint.done_wizard"),
    }
    .to_string()
}

/// The language picker: one row a language, autonyms, the kit's modal
/// conventions.
fn draw_language(frame: &mut Frame, wizard: &mut Wizard, area: Rect, sel: usize) {
    let inner =
        kit::modal_frame(frame, area, 30, LANGS.len() as u16 + 4, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("lang.modal_title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::PathCancel, t!("path_modal.tip_close"));
    for (i, (code, autonym)) in LANGS.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        let current = wizard.lang == i;
        let style = if sel == i {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else if current {
            accent()
        } else {
            Style::default()
        };
        let marker = if current { "• " } else { "  " };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("{marker}{autonym}"), style)),
            rect,
        );
        wizard.ui.click(rect, Act::SetLanguage(i));
        let _ = code;
    }
}

fn card(frame: &mut Frame, at: Rect, focused: bool, hovered: bool) -> Rect {
    let style = if hovered {
        Style::default().fg(th().bright)
    } else if focused {
        accent()
    } else {
        dim()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(style);
    let inner = block.inner(at);
    frame.render_widget(block, at);
    inner
}

// Figlet "mStream" — kept byte-identical to the server's boot banner.
const LOGO: [&str; 5] = [
    r"           ____  _",
    r" _ __ ___ / ___|| |_ _ __ ___  __ _ _ __ ___",
    r"| '_ ` _ \\___ \| __| '__/ _ \/ _` | '_ ` _ \",
    r"| | | | | |___) | |_| | |  __/ (_| | | | | | |",
    r"|_| |_| |_|____/ \__|_|  \___|\__,_|_| |_| |_|",
];
/// The logo's widest line. Every line draws at one fixed x computed from
/// this, so ragged line widths can't skew per-line centering.
const LOGO_W: u16 = 46;

/// The real wordmark, for terminals that draw pixels: the webapp's vector
/// logo recolored to this kit's palette (no official dark variant exists —
/// the webapp's and the mobile app's marks are both navy, drawn for light
/// grounds) and rasterized with alpha at build time. Flattened here onto
/// the theme ground: the picture only ever draws on an OWNED ground, so
/// the flatten color is exactly what sits behind it — and sixel, which
/// has no alpha, gets correct pixels for free.
fn logo_art(ground: (u8, u8, u8)) -> Option<crate::tui::art::Art> {
    let png = include_bytes!("mstream-logo-dark.png");
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (gr, gg, gb) = ground;
    let mut flat = image::RgbImage::new(img.width(), img.height());
    for (x, y, px) in img.enumerate_pixels() {
        let [r, g, b, a] = px.0;
        let a = a as u32;
        let blend = |c: u8, g: u8| ((c as u32 * a + g as u32 * (255 - a)) / 255) as u8;
        flat.put_pixel(x, y, image::Rgb([blend(r, gr), blend(g, gg), blend(b, gb)]));
    }
    let mut png_out = Vec::new();
    image::DynamicImage::ImageRgb8(flat)
        .write_to(&mut std::io::Cursor::new(&mut png_out), image::ImageFormat::Png)
        .ok()?;
    crate::tui::art::decode(&png_out)
}

fn draw_folders(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    // The wordmark as pixels where the terminal can draw them and the
    // ground is ours (the picture is flattened onto that ground, so on an
    // unowned background the figlet is the honest rendering). Both paths
    // occupy the same band, so the screen never reflows on capability.
    let band = Rect { x: column.x, y, width: column.width, height: LOGO.len() as u16 };
    let drew = theme::ground_owned()
        && match &wizard.logo_art {
            Some(art) => wizard.logo_gfx.draw(frame, band, art),
            None => false,
        };
    if !drew {
        let logo_x = column.x + column.width.saturating_sub(LOGO_W) / 2;
        for (i, line) in LOGO.iter().enumerate() {
            frame.render_widget(
                Paragraph::new(Span::styled(*line, accent())),
                Rect { x: logo_x, y: y + i as u16, width: LOGO_W.min(column.width), height: 1 },
            );
        }
    }
    y += band.height + 2;

    // The picker card — the screen's one add affordance (typing a path is
    // the `t` shortcut, in the tips line) — sits ABOVE the table so it
    // holds one spot as folders come and go. Green: the affirmative add.
    let add_rect = Rect { x: column.x, y, width: column.width, height: 3 };
    let add_hover = wizard.ui.pointer.is_some_and(|p| add_rect.contains(p));
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
    wizard.ui.click(add_rect, Act::BrowseNative);
    y += 4;

    // The chosen folders as a table: NAME first — the vpath is the point.
    // The [X] remove control sits to the RIGHT of the selection area, not
    // inside it: selecting a row never highlights its remove button.
    const NAME_W: u16 = 16;
    const REMOVE_W: u16 = 4; // ' [X]'
    let sel_width = column.width.saturating_sub(REMOVE_W);
    let header = Rect { x: column.x, y, width: sel_width, height: 1 };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{:<width$}", t!("folders.col_name"), width = NAME_W as usize), dim()),
            Span::styled(t!("folders.col_folder").to_string(), dim()),
        ])),
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
    if wizard.folders.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("folders.nothing_yet").to_string(), dim())),
            Rect { x: column.x, y, width: sel_width, height: 1 },
        );
        return;
    }
    // Rows live in whatever height remains; overflow scrolls. The view
    // yanks to a fresh add or a moved keyboard cursor, else the wheel
    // offset stands.
    let avail = (column.y + column.height).saturating_sub(y) as usize;
    let sel_moved = wizard.sel != wizard.sel_anchor;
    wizard.sel_anchor = wizard.sel;
    let reveal = wizard.reveal.take().or(if sel_moved { wizard.sel } else { None });
    let (first, visible) = kit::table_view(wizard.folders.len(), reveal, wizard.tscroll, avail);
    wizard.tscroll = first;
    if visible == 0 {
        return;
    }
    let rows_y = y;
    for i in first..first + visible {
        let selected = wizard.sel == Some(i);
        let rect = Rect { x: column.x, y, width: sel_width, height: 1 };

        let folder = &wizard.folders[i];
        let editing = wizard.editing.as_ref().is_some_and(|(row, _)| *row == i);
        let name = match (&wizard.editing, editing) {
            (Some((_, draft)), true) => format!(
                "[{}]",
                kit::input_display(draft.value(), draft.cursor(), NAME_W.saturating_sub(2))
            ),
            _ => folder.name.clone(),
        };
        let row_bg = if selected && !editing {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else {
            Style::default()
        };
        let name_rect = Rect { x: column.x, y, width: NAME_W.min(sel_width), height: 1 };
        // The rename affordance lights up under the pointer — the same
        // brightening every other clickable gets (a selected row keeps
        // its bg; only the name's fg brightens).
        let name_hover = !editing && wizard.ui.pointer.is_some_and(|p| name_rect.contains(p));
        let name_style = if editing {
            Style::default().fg(th().bright)
        } else if name_hover && selected {
            Style::default().fg(th().bright).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if name_hover {
            Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else {
            accent()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(" ".repeat(sel_width as usize), row_bg)),
            rect,
        );
        frame.render_widget(Paragraph::new(Span::styled(name, name_style)), name_rect);
        wizard.ui.click(name_rect, Act::RenameFolder(i));
        if !editing {
            // The tip tells the truth per row: a committed name cannot
            // be renamed here (the server has no rename API — only the
            // admin panel's remove-and-re-add), and promising "click to
            // rename" on it reads as a broken click.
            if folder.committed {
                wizard.ui.tip(name_rect, t!("note.committed_rename"));
            } else {
                wizard.ui.tip(name_rect, t!("folders.tip_name"));
            }
        }
        let path_x = column.x + NAME_W;
        let path_rect =
            Rect { x: path_x, y, width: sel_width.saturating_sub(NAME_W), height: 1 };
        // The LOCAL filesystem's verdict, worn by the row: problems paint
        // the path gold, the tooltip says why. Advisory only — Continue
        // still works, the server has final say at commit.
        let problem: Option<String> = match folder.check {
            FolderCheck::Missing => Some(t!("folders.warn_missing").to_string()),
            FolderCheck::NotADir => Some(t!("folders.warn_file").to_string()),
            FolderCheck::Unreadable => Some(t!("folders.warn_unreadable").to_string()),
            FolderCheck::Ok | FolderCheck::Pending => folder
                .nested_in
                .is_some()
                .then(|| t!("folders.warn_nested").to_string()),
        };
        let path_style = match problem {
            Some(_) if !selected => Style::default().fg(th().gold),
            _ => row_bg,
        };
        // Long paths keep their LEAF visible: a leading … and the tail,
        // fitted to the cell (a path's informative end is its end — the
        // table rule's trailing … would show ten identical prefixes).
        // The full path rides the tooltip whenever the cell clipped;
        // a validation problem still outranks it.
        let (shown, clipped) = ellipsize_path_start(&folder.path, path_rect.width as usize);
        frame.render_widget(Paragraph::new(Span::styled(shown, path_style)), path_rect);
        match problem {
            Some(tip) => wizard.ui.tip(path_rect, tip),
            None if clipped => {
                // The tooltip carries at most THREE lines of path;
                // anything longer keeps its tail behind the same leading
                // …. The cap is THIS caller's policy, not the kit's —
                // wrap_tip itself has no line limit, so future tips may
                // run as long as they need.
                let (tip, _) =
                    ellipsize_path_start(&folder.path, crate::kit::TIP_WRAP * 3);
                wizard.ui.tip(path_rect, tip);
            }
            None => {}
        }
        let x_rect = Rect { x: column.x + sel_width + 1, y, width: 3, height: 1 };
        let x_hover = wizard.ui.pointer.is_some_and(|p| x_rect.contains(p));
        let x_style = if x_hover {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        frame.render_widget(Paragraph::new(Span::styled("[X]", x_style)), x_rect);
        wizard.ui.click(x_rect, Act::RemoveAt(i));
        wizard.ui.tip(x_rect, "Remove this folder");
        y += 1;
    }

    // Overflow → the kit's scrollbar, just right of the [X] column (the
    // column is centered, so the margin cell is always there). Endcaps
    // are click rects; the wheel scrolls too.
    let bar = Rect { x: column.x + column.width, y: rows_y, width: 1, height: visible as u16 };
    kit::scroll_list(
        frame,
        &mut wizard.ui,
        bar,
        wizard.folders.len(),
        visible,
        first,
        Act::TableScroll(-1),
        Act::TableScroll(1),
        Act::TableScrollTo,
    );
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
    let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
    let inner = card(frame, rect, focused, hovered);
    let _ = focused; // the caller renders the caret via input_display
    frame.render_widget(Paragraph::new(Span::raw(value)), inner);
    wizard.ui.click(rect, Act::Focus(field));
    y + 4
}

fn draw_login(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("login.title").to_string(), bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled(
            t!("login.subtitle").to_string(),
            dim(),
        )),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let width = column.width.min(44);
    let x = column.x + (column.width - width) / 2;
    let focus = wizard.field;
    let show = |input: &Input, secret: bool, focused: bool| -> String {
        let value =
            if secret { mask(input.value()) } else { input.value().to_string() };
        if focused {
            kit::input_display(&value, input.cursor(), width.saturating_sub(2))
        } else {
            value
        }
    };
    let (username, password, confirm) = (
        show(&wizard.username, false, focus == LoginField::Username),
        show(&wizard.password, true, focus == LoginField::Password),
        show(&wizard.confirm, true, focus == LoginField::Confirm),
    );
    y = field_row(frame, wizard, x, y, width, &t!("login.field_username"), username, LoginField::Username, focus == LoginField::Username);
    y = field_row(frame, wizard, x, y, width, &t!("login.field_password"), password, LoginField::Password, focus == LoginField::Password);
    y = field_row(frame, wizard, x, y, width, &t!("login.field_confirm"), confirm, LoginField::Confirm, focus == LoginField::Confirm);
    y += 1;

    let skip = kit::button(
        frame,
        &mut wizard.ui,
        Rect { x, y, width, height: 1 },
        &t!("login.skip"),
        false,
        Act::SkipLogin,
    );
    wizard.ui.tip(skip, t!("login.tip_skip"));
}

fn mask(secret: &str) -> String {
    "•".repeat(secret.chars().count())
}

fn draw_extras(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("extras.title").to_string(), bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 1;
    frame.render_widget(
        Paragraph::new(Span::styled(t!("extras.subtitle").to_string(), dim())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    let rows: [(String, String); 3] = [
        (t!("extras.updates_label").to_string(), t!("extras.updates_desc").to_string()),
        (t!("extras.audio_label").to_string(), t!("extras.audio_desc").to_string()),
        (t!("extras.discovery_label").to_string(), t!("extras.discovery_desc").to_string()),
    ];
    for (i, (label, desc)) in rows.into_iter().enumerate() {
        let selected = wizard.extras_sel == Some(i);
        let rect = Rect { x: column.x, y, width: column.width, height: 4 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        let inner = card(frame, rect, selected, hovered);
        wizard.ui.click(rect, Act::Toggle(i));
        let box_span = if wizard.extras[i] {
            Span::styled("[✓] ", Style::default().fg(th().ok))
        } else {
            Span::styled("[ ] ", dim())
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![box_span, Span::styled(label, bold())])),
            Rect { x: inner.x + 1, y: inner.y, width: inner.width, height: 1 },
        );
        frame.render_widget(
            Paragraph::new(Span::styled(desc, dim())),
            Rect { x: inner.x + 5, y: inner.y + 1, width: inner.width.saturating_sub(5), height: 1 },
        );
        y += 4;
    }
}

fn draw_done(frame: &mut Frame, wizard: &mut Wizard, column: Rect) {
    let mut y = column.y;
    let server = wizard.client.server();
    let opening = match server_port(&server) {
        Some(port) => {
            t!("done.running_port", port = port).to_string()
        }
        None => t!("done.running_at", server = server).to_string(),
    };

    // Two columns when the code draws as pixels (it scales to its box, so
    // it fits half the width at any window). The fixed-size half-block
    // fallback keeps the stacked layout below — it simply cannot fit a
    // half column.
    if wizard.done_two_column() {
        // Left-anchored: the wordmark over the code at a FIXED width (the
        // pictures scale to it — capped so the code never balloons on a
        // wide window), a full-height vertical gold rule, and the right
        // column takes the rest. No heading — the wordmark is the header.
        let height = column.height.saturating_sub(y - column.y);
        let left_w = 30u16.min(column.width / 2);
        let left = Rect { x: column.x, y, width: left_w, height };
        let rule_x = left.right() + 2;
        let right_x = rule_x + 3;
        let right =
            Rect { x: right_x, y, width: column.right().saturating_sub(right_x), height };
        for row in column.y..column.bottom() {
            frame.render_widget(
                Paragraph::new(Span::styled("│", Style::default().fg(th().gold))),
                Rect { x: rule_x, y: row, width: 1, height: 1 },
            );
        }

        // LEFT: the wordmark, then the code.
        let mut ly = left.y;
        if let Some(art) = &wizard.logo_art {
            let logo_band = Rect { x: left.x, y: ly, width: left.width, height: 4 };
            if wizard.logo_gfx.draw(frame, logo_band, art) {
                ly = logo_band.bottom() + 1;
            }
        }
        let band = Rect {
            x: left.x,
            y: ly,
            width: left.width,
            height: left.height.saturating_sub(ly - left.y).min(15),
        };
        if let Some(art) = &wizard.qr_art {
            wizard.graphics.draw(frame, band, art);
        }

        // RIGHT: the opening line, the scan's state, and the button list.
        let mut ry = right.y + 1;
        let scan_line = if wizard.qr.is_some() {
            t!("done.scan_left").to_string()
        } else {
            wizard.qr_note.clone()
        };
        frame.render_widget(
            Paragraph::new(format!("{opening} {scan_line}")).wrap(Wrap { trim: true }),
            Rect { x: right.x, y: ry, width: right.width, height: 4 },
        );
        ry += 5;
        if !wizard.progress.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    wizard.progress.clone(),
                    Style::default().fg(th().ok),
                ))
                .wrap(Wrap { trim: true }),
                Rect { x: right.x, y: ry, width: right.width, height: 1 },
            );
        }
        ry += 2;
        for (i, (label, tip)) in done_buttons().into_iter().enumerate() {
            let rect = Rect { x: right.x, y: ry, width: right.width.min(44), height: 3 };
            let selected = wizard.done_sel == Some(i);
            let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
            let inner = card(frame, rect, selected, hovered);
            frame.render_widget(
                Paragraph::new(Span::styled(label, bold())),
                Rect { x: inner.x + 1, y: inner.y, width: inner.width, height: 1 },
            );
            wizard.ui.click(rect, done_action(i));
            wizard.ui.tip(rect, tip);
            ry += 3;
        }
        return;
    }

    let heading =
        if wizard.standalone {
            t!("done.heading_qr").to_string()
        } else {
            t!("done.heading_wizard").to_string()
        };
    frame.render_widget(
        Paragraph::new(Span::styled(heading, bold())),
        Rect { x: column.x, y, width: column.width, height: 1 },
    );
    y += 2;

    if let Some(qr) = wizard.qr.clone() {
        let rows = qr.len() as u16;
        // The code gets whatever height the chrome below it does not
        // need: the message (2), a blank, and the button list (4).
        let avail = column.height.saturating_sub(y - column.y).saturating_sub(8);
        // The half-block code is fixed-size — drawn only when it fits
        // WHOLE (a cropped QR scans as nothing), and otherwise replaced
        // by the one honest line.
        if rows <= avail {
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
        } else {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    t!("done.too_short").to_string(),
                    dim(),
                ))
                .alignment(Alignment::Center),
                Rect { x: column.x, y, width: column.width, height: 1 },
            );
            y += 2;
        }
    }
    let scan_line = if wizard.qr.is_some() {
        t!("done.scan_above").to_string()
    } else {
        wizard.qr_note.clone()
    };
    frame.render_widget(
        Paragraph::new(format!("{opening} {scan_line}"))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        Rect { x: column.x, y, width: column.width, height: 2 },
    );
    y += 3;

    // With no bottom bar on this page, the scan status lives here.
    if !wizard.progress.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(wizard.progress.clone(), Style::default().fg(th().ok)))
                .alignment(Alignment::Center),
            Rect { x: column.x, y, width: column.width, height: 1 },
        );
        y += 2;
    }

    // The same button list, one row each, centered.
    let buttons = done_buttons();
    let widest = buttons.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) as u16;
    let bx = column.x + column.width.saturating_sub(widest + 4) / 2;
    for (i, (label, tip)) in buttons.into_iter().enumerate() {
        let rect = Rect { x: bx, y, width: widest + 4, height: 1 };
        let selected = wizard.done_sel == Some(i);
        kit::button(frame, &mut wizard.ui, rect, &label, selected, done_action(i));
        wizard.ui.tip(rect, tip);
        y += 1;
    }
}

fn draw_skip_warning(frame: &mut Frame, wizard: &mut Wizard, area: Rect) {
    let inner = kit::modal_frame(frame, area, 62, 13, th().gold);
    let lines = vec![
        Line::from(Span::styled(t!("skip_modal.title").to_string(), Style::default().fg(th().gold).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(t!("skip_modal.body").to_string()),
        Line::from(""),
        Line::from(Span::styled(t!("skip_modal.plus_access").to_string(), Style::default().fg(th().ok))),
        Line::from(Span::styled(t!("skip_modal.plus_tv").to_string(), Style::default().fg(th().ok))),
        Line::from(Span::styled(t!("skip_modal.minus_control").to_string(), Style::default().fg(th().gold))),
        Line::from(Span::styled(t!("skip_modal.minus_qc").to_string(), Style::default().fg(th().gold))),
        Line::from(""),
        Line::from(Span::styled(t!("skip_modal.later").to_string(), dim())),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    let y = inner.bottom().saturating_sub(1);
    let back = kit::button(
        frame,
        &mut wizard.ui,
        Rect { x: inner.x, y, width: inner.width, height: 1 },
        &t!("skip_modal.back"),
        true,
        Act::SkipCancel,
    );
    kit::button(
        frame,
        &mut wizard.ui,
        Rect { x: back.right() + 2, y, width: inner.width, height: 1 },
        &t!("skip_modal.confirm"),
        false,
        Act::SkipConfirm,
    );
}

fn draw_browser(frame: &mut Frame, wizard: &mut Wizard, area: Rect, browse: &Browse) {
    let inner = kit::modal_frame(frame, area, 66, 18, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("browse.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::BrowseCancel, t!("path_modal.tip_close"));
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
            Paragraph::new(Span::styled(format!("▸ {}", browse.dirs[i]), style)),
            rect,
        );
        wizard.ui.click(rect, Act::BrowseRow(i));
    }
    if browse.dirs.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(t!("browse.empty").to_string(), dim())),
            Rect { x: inner.x, y: list_top, width: inner.width, height: 1 },
        );
    }

    let y = inner.bottom().saturating_sub(1);
    let up = kit::button(frame, &mut wizard.ui, Rect { x: inner.x, y, width: inner.width, height: 1 }, &t!("browse.up"), false, Act::BrowseUp);
    let open = kit::button(frame, &mut wizard.ui, Rect { x: up.right() + 1, y, width: inner.width, height: 1 }, &t!("browse.open"), false, Act::BrowseEnter);
    let add = kit::button(
        frame,
        &mut wizard.ui,
        Rect { x: open.right() + 1, y, width: inner.width, height: 1 },
        &t!("browse.add"),
        true,
        Act::BrowseAdd,
    );
    kit::button(frame, &mut wizard.ui, Rect { x: add.right() + 1, y, width: inner.width, height: 1 }, &t!("browse.close"), false, Act::BrowseCancel);
}

fn draw_path_entry(frame: &mut Frame, wizard: &mut Wizard, area: Rect, draft: &PathDraft) {
    let suggestions = draft.suggestions();
    let shown = suggestions.len().min(6) as u16;
    // Anchored as if always full: the title and input hold one spot and
    // the suggestion list grows DOWNWARD beneath them.
    let inner = kit::modal_frame_anchored(frame, area, 62, 7 + shown, 13, th().accent);
    frame.render_widget(
        Paragraph::new(Span::styled(t!("path_modal.title").to_string(), bold())),
        Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 },
    );
    kit::modal_close(frame, &mut wizard.ui, inner, Act::PathCancel, t!("path_modal.tip_close"));
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
    if let Modal::PathEntry(d) = &mut wizard.modal {
        d.scroll = first;
        d.sel_anchor = d.sel;
    }
    let overflow = suggestions.len() > visible;
    let row_width = if overflow { inner.width.saturating_sub(1) } else { inner.width };
    for (row, i) in (first..first + visible).enumerate() {
        let entry = &suggestions[i];
        let selected = draft.sel == Some(i);
        let rect =
            Rect { x: inner.x, y: inner.y + 4 + row as u16, width: row_width, height: 1 };
        let hovered = wizard.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = if selected {
            Style::default().fg(th().on_accent).bg(th().accent)
        } else if hovered {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!("▸ {entry}"), style)),
            rect,
        );
        wizard.ui.click(rect, Act::PathSuggest(i));
    }
    let bar = Rect {
        x: inner.x + inner.width.saturating_sub(1),
        y: inner.y + 4,
        width: 1,
        height: visible as u16,
    };
    kit::scroll_list(
        frame,
        &mut wizard.ui,
        bar,
        suggestions.len(),
        visible,
        first,
        Act::PathScroll(-1),
        Act::PathScroll(1),
        Act::PathScrollTo,
    );
    // A listing failure for the current dir-part shows where suggestions
    // would be — kit error style, the server's words.
    if let Some(err) = &draft.error {
        frame.render_widget(
            Paragraph::new(Span::styled(err.clone(), Style::default().fg(th().gold))),
            Rect { x: inner.x, y: inner.y + 4, width: inner.width, height: 1 },
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(path: &str) -> Folder {
        Folder {
            path: path.to_string(),
            name: String::new(),
            named_by_user: false,
            committed: false,
            check: FolderCheck::Pending,
            canonical: None,
            nested_in: None,
        }
    }

    #[test]
    fn no_row_is_selected_until_the_arrows_say_so() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.add_folder("/tmp/a".to_string());
        wizard.add_folder("/tmp/b".to_string());
        // Adding reveals (scroll target) but never selects.
        assert_eq!(wizard.sel, None);
        assert_eq!(wizard.reveal, Some(1));
        // A mouse rename never selects either.
        wizard.act(Act::RenameFolder(0));
        assert!(wizard.editing.as_ref().is_some_and(|(row, _)| *row == 0));
        assert_eq!(wizard.sel, None);
        wizard.finish_rename();
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // The keyboard picks the cursor up from either end…
        handle_key(&mut wizard, key(KeyCode::Down));
        assert_eq!(wizard.sel, Some(0));
        handle_key(&mut wizard, key(KeyCode::Down));
        assert_eq!(wizard.sel, Some(1));
        // …and Esc stows it again.
        handle_key(&mut wizard, key(KeyCode::Esc));
        assert_eq!(wizard.sel, None);
        handle_key(&mut wizard, key(KeyCode::Up));
        assert_eq!(wizard.sel, Some(1), "up starts from the bottom");
        // r removes the row under the cursor; without one it's a no-op.
        handle_key(&mut wizard, key(KeyCode::Esc));
        handle_key(&mut wizard, key(KeyCode::Char('r')));
        assert_eq!(wizard.folders.len(), 2);
        handle_key(&mut wizard, key(KeyCode::Down));
        handle_key(&mut wizard, key(KeyCode::Char('r')));
        assert_eq!(wizard.folders.len(), 1);
    }

    #[test]
    fn removing_shifts_the_cursor_and_a_rename_in_progress() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        for p in ["/tmp/a", "/tmp/b", "/tmp/c"] {
            wizard.add_folder(p.to_string());
        }
        wizard.sel = Some(2);
        wizard.editing = Some((2, "draft".into()));
        wizard.remove_at(0);
        assert_eq!(wizard.sel, Some(1), "cursor follows its row left");
        assert_eq!(wizard.editing.as_ref().map(|(r, _)| *r), Some(1));
        // Removing the edited row cancels the edit.
        wizard.remove_at(1);
        assert!(wizard.editing.is_none());
    }

    #[test]
    fn the_logo_is_the_server_banner_verbatim() {
        assert!(LOGO.iter().all(|l| l.chars().count() <= LOGO_W as usize));
        assert_eq!(LOGO[0].trim(), "____  _");
        // The joins that line breaks once mangled: the S's back, the r's
        // stem, and the two-cell gap before the e.
        assert!(LOGO[2].contains(r"_ \\___ \|"), "m's trailing and S's leading backslash are ADJACENT");
        assert!(LOGO[3].contains(r"| |_| | |  __/"));
        assert!(LOGO[4].contains(r"\__|_|  \___|"));
    }

    #[test]
    fn the_scan_widget_renders_every_progress_shape() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        let row = |vpath: &str, pct: Option<u32>, scanned: u64| crate::api::types::ScanProgressRow {
            vpath: vpath.to_string(),
            pct,
            scanned,
        };
        // A percentage estimate, one library.
        wizard.apply(Done::Progress(Ok(vec![row("media", Some(44), 1204)])));
        assert_eq!(wizard.progress, "Scanning media — 44% (1204 tracks so far)");
        // A second library behind it earns the queued suffix.
        wizard.apply(Done::Progress(Ok(vec![
            row("media", Some(76), 2261),
            row("audiobooks", None, 4),
        ])));
        assert_eq!(wizard.progress, "Scanning media — 76% (2261 tracks so far, more queued)");
        // No estimate: the pct-less template.
        wizard.apply(Done::Progress(Ok(vec![row("audiobooks", None, 141)])));
        assert_eq!(wizard.progress, "Scanning audiobooks — 141 tracks so far");
        // Empty means done.
        wizard.apply(Done::Progress(Ok(vec![])));
        assert_eq!(wizard.progress, "Library scan complete.");
        // A poll hiccup never marks the page — the last state stands.
        wizard.apply(Done::Progress(Err(crate::api::ApiError::Config("net".into()))));
        assert_eq!(wizard.progress, "Library scan complete.");
    }

    #[test]
    fn long_paths_keep_their_leaf_and_carry_the_full_path_in_the_tip() {
        // Fits: untouched, no clip.
        assert_eq!(ellipsize_path_start("/music", 10), ("/music".to_string(), false));
        // Overflows: leading …, the tail survives, width is exact.
        let (shown, clipped) = ellipsize_path_start("/home/anna/Music/Favorites", 12);
        assert!(clipped);
        assert_eq!(shown.chars().count(), 12);
        assert!(shown.starts_with('…'));
        assert!(shown.ends_with("c/Favorites"), "{shown}");
        // Degenerate width never panics.
        assert_eq!(ellipsize_path_start("/x", 0).0, "/x");
        // The tooltip pairing: a spaceless path hard-wraps at the tip
        // width instead of clipping, and the THREE-line cap holds however
        // long the path gets. The cap is the path tip's policy alone —
        // wrap_tip itself is uncapped, so a longer text keeps all its
        // lines (future tips may need them).
        let long = format!("/very{}/leaf", "/deep".repeat(60));
        let (tip, clipped) = ellipsize_path_start(&long, crate::kit::TIP_WRAP * 3);
        assert!(clipped);
        let lines = crate::kit::wrap_tip(&tip);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(lines.iter().all(|l| l.chars().count() <= crate::kit::TIP_WRAP));
        assert!(lines[2].ends_with("/leaf"), "the leaf survives: {lines:?}");
        assert_eq!(crate::kit::wrap_tip(&long).len(), 8, "the kit itself never caps");
        // And ordinary sentence tips still word-wrap as before.
        let prose = crate::kit::wrap_tip("Add or adjust folders — nothing is lost");
        assert_eq!(prose.len(), 1);
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

    /// Serialises the one test that flips the process-global locale
    /// against the tests that assert English strings — today's config
    /// env-race lesson, applied before it flakes.
    static LOCALE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn every_locale_mirrors_the_english_keys_and_placeholders() {
        let files: [(&str, &str); 10] = [
            ("en", include_str!("../../locales/en.yml")),
            ("de", include_str!("../../locales/de.yml")),
            ("es", include_str!("../../locales/es.yml")),
            ("fr", include_str!("../../locales/fr.yml")),
            ("it", include_str!("../../locales/it.yml")),
            ("ja", include_str!("../../locales/ja.yml")),
            ("pl", include_str!("../../locales/pl.yml")),
            ("pt", include_str!("../../locales/pt.yml")),
            ("ru", include_str!("../../locales/ru.yml")),
            ("zh", include_str!("../../locales/zh.yml")),
        ];
        fn leaves(prefix: &str, v: &serde_yaml::Value, out: &mut Vec<(String, String)>) {
            match v {
                serde_yaml::Value::Mapping(m) => {
                    for (k, v) in m {
                        let k = k.as_str().expect("string keys");
                        let key =
                            if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
                        leaves(&key, v, out);
                    }
                }
                other => out.push((
                    prefix.to_string(),
                    other.as_str().expect("string leaves").to_string(),
                )),
            }
        }
        let mut sets = Vec::new();
        for (code, body) in files {
            let parsed: serde_yaml::Value = serde_yaml::from_str(body).expect(code);
            let mut out = Vec::new();
            leaves("", &parsed, &mut out);
            for (key, text) in &out {
                assert!(!text.trim().is_empty(), "{code}: {key} is empty");
            }
            sets.push((code, out));
        }
        let (_, en) = &sets[0];
        let en_keys: std::collections::BTreeMap<&str, &str> =
            en.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        // The languages the app offers are exactly the files on disk.
        assert_eq!(LANGS.len(), sets.len());
        for (code, entries) in &sets[1..] {
            let keys: std::collections::BTreeSet<&str> =
                entries.iter().map(|(k, _)| k.as_str()).collect();
            let want: std::collections::BTreeSet<&str> = en_keys.keys().copied().collect();
            assert_eq!(keys, want, "{code}: key set differs from en");
            // Placeholders must survive translation, name for name.
            for (key, text) in entries.iter() {
                let holes = |s: &str| {
                    let mut found: Vec<String> = Vec::new();
                    let mut rest = s;
                    while let Some(i) = rest.find("%{") {
                        let tail = &rest[i + 2..];
                        let end = tail.find('}').expect("closed placeholder");
                        found.push(tail[..end].to_string());
                        rest = &tail[end..];
                    }
                    found.sort();
                    found
                };
                assert_eq!(
                    holes(text),
                    holes(en_keys[key.as_str()]),
                    "{code}: {key} placeholder mismatch"
                );
            }
        }
    }

    #[test]
    fn the_boot_language_prefers_env_then_system_then_english() {
        assert_eq!(detect_lang(None, None), 0);
        assert_eq!(detect_lang(None, Some("de-DE".into())), 1);
        assert_eq!(detect_lang(None, Some("pt_BR.UTF-8".into())), 7);
        assert_eq!(detect_lang(None, Some("zh-Hans-CN".into())), 9);
        assert_eq!(detect_lang(None, Some("ko-KR".into())), 0, "unsupported falls to English");
        assert_eq!(detect_lang(Some("ja".into()), Some("de-DE".into())), 5, "env outranks system");
        assert_eq!(detect_lang(Some("nope".into()), Some("ru".into())), 8, "bad env falls through");
    }

    #[test]
    fn the_done_layout_is_decided_by_capability_not_data() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        assert!(!wizard.done_two_column(), "no pixels, no columns");
        // Pixels mean columns from the FIRST frame, before any Quick
        // Connect data arrives — deciding on the data painted one
        // stacked, unstyled frame and then reflowed the page.
        wizard.graphics =
            crate::tui::graphics::Graphics::forced(ratatui_image::picker::ProtocolType::Kitty);
        assert!(wizard.qr.is_none() && wizard.qr_art.is_none());
        assert!(wizard.done_two_column());
    }

    #[test]
    fn the_language_selector_lists_switches_and_confesses() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // l opens the picker with the current language under the cursor.
        handle_key(&mut wizard, key(KeyCode::Char('l')));
        assert!(matches!(wizard.modal, Modal::Language(0)));
        // Arrows move, Enter chooses; German is machine-translated and
        // says so in German.
        handle_key(&mut wizard, key(KeyCode::Down));
        handle_key(&mut wizard, key(KeyCode::Enter));
        assert!(matches!(wizard.modal, Modal::None));
        assert_eq!(wizard.lang, 1);
        let note = wizard.note.clone().expect("the machine-translation note").0;
        assert!(note.contains("maschinell"), "{note}");
        // Back to English: no confession needed.
        wizard.set_language(0);
        assert!(wizard.note.is_none());
        assert_eq!(rust_i18n::locale().to_string(), "en");
        // Spot checks without touching the global locale: every language
        // renders the add-card in its own words.
        assert_eq!(t!("folders.add_card", locale = "de"), "Musikordner hinzufügen");
        assert_eq!(t!("folders.add_card", locale = "ja"), "音楽フォルダーを追加");
        assert_ne!(t!("lang.machine_note", locale = "ru"), t!("lang.machine_note", locale = "en"));
    }

    #[test]
    fn the_logo_picture_flattens_onto_the_ground() {
        let art = logo_art((0x12, 0x13, 0x1c)).expect("a picture");
        let img = image::load_from_memory(art.source()).expect("its source decodes");
        assert_eq!((img.width(), img.height()), (1224, 306));
        let rgb = img.to_rgb8();
        // Transparent corners took the ground color EXACTLY — nothing
        // else may show behind a flattened picture.
        assert_eq!(rgb.get_pixel(0, 0).0, [0x12, 0x13, 0x1c]);
        assert_eq!(rgb.get_pixel(1223, 305).0, [0x12, 0x13, 0x1c]);
        // And the recolored wordmark is actually drawn on it.
        assert!(rgb.pixels().any(|p| p.0[2] > 0xa0), "the wordmark is present");
    }

    #[test]
    fn the_qr_picture_is_a_square_png_with_a_quiet_zone() {
        let art = qr_art("iroh-ticket-abc123").expect("a picture");
        let img = image::load_from_memory(art.source()).expect("its source decodes");
        assert_eq!(img.width(), img.height(), "QR pictures are square");
        // 8 px a module and 4 quiet modules a side — the side length is
        // (modules + 8) * 8 whatever version the ticket needed.
        assert_eq!(img.width() % 8, 0);
        assert!(img.width() >= (21 + 8) * 8, "at least a version-1 code plus quiet zone");
        // Corners sit inside the quiet zone: white, or the scan fails.
        assert_eq!(img.to_luma8().get_pixel(0, 0).0[0], 255);
    }

    #[test]
    fn the_standalone_qr_page_drops_the_wizard_chrome() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.standalone = true;
        wizard.screen = Screen::Done;
        let status = crate::api::types::IrohStatus {
            enabled: true,
            qr: Some("ticket".to_string()),
            ..Default::default()
        };
        wizard.apply(Done::Iroh(Ok(status.clone())));
        assert!(wizard.qr.is_some());
        assert!(wizard.qr_art.is_some(), "the picture rides along with the half-blocks");
        assert_eq!(wizard.queued, None, "no scan-progress polling outside the wizard");
        assert_eq!(footer_hint(&wizard), "↑ ↓ select · Enter open · Esc close");
        // The button list answers to arrows + Enter; under the test seam
        // the note carries the URL instead of popping a browser.
        unsafe { std::env::set_var("MSTREAM_NO_OPEN", "1") };
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        assert_eq!(wizard.done_sel, None, "no button is selected until the arrows say so");
        handle_key(&mut wizard, key(KeyCode::Enter));
        assert!(wizard.note.is_none(), "Enter without a selection does nothing");
        handle_key(&mut wizard, key(KeyCode::Down));
        assert_eq!(wizard.done_sel, Some(0));
        handle_key(&mut wizard, key(KeyCode::Enter));
        let note = wizard.note.clone().expect("a note").0;
        assert!(note.contains("play.google.com"), "Android first: {note}");
        handle_key(&mut wizard, key(KeyCode::Down));
        handle_key(&mut wizard, key(KeyCode::Enter));
        let note = wizard.note.clone().expect("a note").0;
        assert!(note.contains("apps.apple.com"), "iOS second: {note}");
        handle_key(&mut wizard, key(KeyCode::Down));
        handle_key(&mut wizard, key(KeyCode::Enter));
        let note = wizard.note.clone().expect("a note").0;
        assert!(note.contains("/admin"), "admin third: {note}");
        // Up from nothing starts at the bottom; the ends clamp.
        wizard.done_sel = None;
        handle_key(&mut wizard, key(KeyCode::Up));
        assert_eq!(wizard.done_sel, Some(DONE_ACTIONS - 1));
        handle_key(&mut wizard, key(KeyCode::Down));
        assert_eq!(wizard.done_sel, Some(DONE_ACTIONS - 1), "the bottom clamps");
        // Esc closes the page (and the wizard's Done screen alike).
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(handle_key(&mut wizard, key), Some(Outcome::Quit)));
        // The wizard flavor still polls after the same answer.
        let mut wizard = Wizard::new(Client::new("http://127.0.0.1:9").expect("client"));
        wizard.screen = Screen::Done;
        wizard.apply(Done::Iroh(Ok(status)));
        assert_eq!(wizard.queued, Some(Op::PollProgress));
    }

    #[test]
    fn a_reopened_wizard_seeds_server_folders_and_dedups_against_them() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.apply(Done::Ping(Ok(vec![("media".to_string(), "/srv/music".to_string())])));
        assert_eq!(wizard.folders.len(), 1);
        assert!(wizard.folders[0].committed, "Continue must skip server rows");
        assert!(wizard.folders[0].named_by_user, "resyncs never rename server rows");
        assert_eq!(wizard.folders[0].name, "media");
        assert_eq!(wizard.pending_validate, vec!["/srv/music".to_string()]);
        assert_eq!(wizard.sel, None, "seeding never grabs the cursor");
        // The reopen bug this guards: a fresh wizard's first new folder used
        // to derive the single-folder name `media` and the server 500'd on
        // the duplicate vpath. With a seeded row the list isn't single, and
        // seeded names join the dedup set.
        wizard.add_folder("/srv/media".to_string());
        assert_eq!(wizard.folders[1].name, "media-2");
        // Re-seeding (same vpath, or same path) never duplicates rows.
        wizard.seed_folders(vec![
            ("media".to_string(), "/srv/music".to_string()),
            ("extra".to_string(), "/srv/extra".to_string()),
        ]);
        assert_eq!(wizard.folders.len(), 3);
        // Continue sends ONLY the new folders to the server.
        wizard.queued = Some(Op::CommitFolders);
        let (tx, rx) = std::sync::mpsc::channel();
        wizard.dispatch_queued(&tx);
        let batch = rx
            .try_iter()
            .find_map(|(_, job)| match job {
                Job::Folders(batch) => Some(batch),
                _ => None,
            })
            .expect("a folders batch");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].2, "media-2");
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
            text: "Mus".into(),
            listed_for: "~".to_string(),
            listed_path: "/home/anna".to_string(),
            entries: vec!["Music".to_string(), "Movies".to_string()],
            ..PathDraft::default()
        });
        wizard.accept_suggestion(0);
        let Modal::PathEntry(draft) = &wizard.modal else { panic!("modal closed") };
        assert_eq!(draft.text.value(), "/home/anna/Music/");
        assert!(
            matches!(&wizard.queued, Some(Op::Complete(dir)) if dir == "/home/anna/Music/"),
            "stepping into the dir must queue its listing"
        );
    }

    #[test]
    fn is_under_respects_separator_boundaries() {
        assert!(is_under("/a/b", "/a"));
        assert!(is_under("/a/b/c", "/a"));
        assert!(!is_under("/ab", "/a"), "prefix without a boundary is not nesting");
        assert!(!is_under("/a", "/a"), "equal is duplicate, not nested");
        assert!(!is_under("/a", "/a/b"), "a parent is not under its child");
        assert!(is_under(r"C:\\music\\rock", r"C:\\music"));
    }

    #[test]
    fn validation_marks_problems_and_removes_other_spellings_of_the_same_folder() {
        let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.add_folder("/media/music".to_string());
        wizard.add_folder("/media/music/rock".to_string());
        wizard.add_folder("/media/music-link".to_string());
        assert_eq!(wizard.pending_validate.len(), 3, "every add queues a validation");

        // The parent and child land: the child gets the nested mark.
        wizard.apply(Done::Validated {
            path: "/media/music".to_string(),
            check: FolderCheck::Ok,
            canonical: Some("/media/music".to_string()),
        });
        wizard.apply(Done::Validated {
            path: "/media/music/rock".to_string(),
            check: FolderCheck::Ok,
            canonical: Some("/media/music/rock".to_string()),
        });
        assert_eq!(wizard.folders[1].nested_in.as_deref(), Some("/media/music"));
        assert!(wizard.note.as_ref().is_some_and(|(n, _)| n.contains("scanned twice")));

        // A symlink spelling of the parent lands: removed, with the why.
        wizard.apply(Done::Validated {
            path: "/media/music-link".to_string(),
            check: FolderCheck::Ok,
            canonical: Some("/media/music".to_string()),
        });
        assert_eq!(wizard.folders.len(), 2, "the duplicate spelling is dropped");
        assert!(wizard.note.as_ref().is_some_and(|(n, _)| n.contains("the same folder as")));

        // Removing the parent clears the child's nested mark.
        wizard.remove_at(0);
        assert!(wizard.folders[0].nested_in.is_none());

        // A missing path is marked and said, but never blocks the list.
        wizard.add_folder("/definitely/not/real".to_string());
        wizard.apply(Done::Validated {
            path: "/definitely/not/real".to_string(),
            check: FolderCheck::Missing,
            canonical: None,
        });
        let bad = wizard.folders.iter().find(|f| f.path == "/definitely/not/real").unwrap();
        assert_eq!(bad.check, FolderCheck::Missing);
        assert!(wizard.note.as_ref().is_some_and(|(n, err)| *err && n.contains("not found")));
    }

    #[test]
    fn validate_path_tells_dirs_from_files_from_nothing() {
        let base = std::env::temp_dir().join(format!("wiz-vp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("real")).unwrap();
        std::fs::write(base.join("song.mp3"), b"x").unwrap();
        let (check, canonical) = validate_path(base.join("real").to_str().unwrap());
        assert_eq!(check, FolderCheck::Ok);
        assert!(canonical.is_some());
        let (check, _) = validate_path(base.join("song.mp3").to_str().unwrap());
        assert_eq!(check, FolderCheck::NotADir);
        let (check, _) = validate_path(base.join("ghost").to_str().unwrap());
        assert_eq!(check, FolderCheck::Missing);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn local_listing_returns_sorted_dirs_and_says_why_it_cannot() {
        let base = std::env::temp_dir().join(format!("wiz-ll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("beta")).unwrap();
        std::fs::create_dir_all(base.join("Alpha")).unwrap();
        std::fs::write(base.join("a-file.txt"), b"x").unwrap();
        let listing = local_list(base.to_str().unwrap()).expect("listing");
        let names: Vec<_> = listing.directories.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "beta"], "dirs only, case-insensitive order");
        assert!(local_list("").is_err());
        assert!(local_list("/definitely/not/a/real/dir").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_typed_tilde_expands_to_the_local_home_immediately() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.modal = Modal::PathEntry(PathDraft { text: "~/Mu".into(), ..PathDraft::default() });
        wizard.refresh_completion();
        let home = local_home().expect("test env has a home");
        let home = home.trim_end_matches(['/', '\\']).to_string();
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert_eq!(draft.text.value(), format!("{home}/Mu"));
        assert_eq!(
            draft.text.cursor(),
            draft.text.value().chars().count(),
            "cursor keeps its distance from the end"
        );
        assert_eq!(draft.listed_for, format!("{home}/"));
        assert!(
            matches!(&wizard.queued, Some(Op::Complete(d)) if *d == format!("{home}/")),
            "the expanded dir is what gets listed"
        );

        // A BARE ~ gains its separator (the platform's own): the preview
        // lands INSIDE the home, not in its parent directory.
        let sep = if home.contains('\\') { '\\' } else { '/' };
        wizard.modal = Modal::PathEntry(PathDraft { text: "~".into(), ..PathDraft::default() });
        wizard.refresh_completion();
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert_eq!(draft.text.value(), format!("{home}{sep}"));
        assert_eq!(draft.listed_for, format!("{home}{sep}"));
        assert_eq!(draft.text.cursor(), draft.text.value().chars().count());
    }

    #[test]
    fn doubled_separators_collapse_as_typed() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        // Typing `/` right after the tilde expansion's own separator.
        wizard.modal = Modal::PathEntry(PathDraft { text: "/home/anna//".into(), ..PathDraft::default() });
        wizard.refresh_completion();
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert_eq!(draft.text.value(), "/home/anna/");
        assert_eq!(draft.text.cursor(), draft.text.value().chars().count());
        // A UNC lead survives; inner doubles still collapse.
        wizard.modal =
            Modal::PathEntry(PathDraft { text: r"\\server\music\\rock".into(), ..PathDraft::default() });
        wizard.refresh_completion();
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert_eq!(draft.text.value(), r"\\server\music\rock");
    }

    #[test]
    fn an_empty_input_suggests_nothing() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.modal = Modal::PathEntry(PathDraft {
            listed_for: "~".to_string(),
            entries: vec!["Music".to_string()],
            ..PathDraft::default()
        });
        wizard.refresh_completion();
        assert!(wizard.queued.is_none(), "no listing for an empty input");
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert!(draft.entries.is_empty() && draft.suggestions().is_empty());
    }

    #[test]
    fn a_failed_listing_for_the_current_dir_is_said_and_a_late_one_stays_quiet() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.modal = Modal::PathEntry(PathDraft {
            text: "/music/x".into(),
            listed_for: "/music/".to_string(),
            ..PathDraft::default()
        });
        // A failure for a dir the user already typed past: quiet.
        wizard.apply(Done::Completed {
            dir: "/mus/".to_string(),
            listing: Err("no route to host".to_string()),
        });
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        assert!(draft.error.is_none());
        // A failure for the CURRENT dir-part: said out loud.
        wizard.apply(Done::Completed {
            dir: "/music/".to_string(),
            listing: Err("no route to host".to_string()),
        });
        let Modal::PathEntry(draft) = &wizard.modal else { panic!() };
        let said = draft.error.as_deref().expect("the failure must be visible");
        assert!(said.contains("/music/") && said.contains("no route to host"));
    }

    #[test]
    fn extras_cursor_is_keyboard_only_and_creating_the_admin_locks_back() {
        // AdminCreated(Ok) below runs remember_session, which LOADS AND
        // SAVES the config file — without holding the config scratch this
        // test wrote "127.0.0.1:9" into whichever directory the env var
        // pointed at: another test's scratch when one was live (the
        // observed parallel flake), and the developer's REAL config when
        // none was. Every test that crosses config takes the same lock.
        let scratch = crate::config::testing::Scratch::new("setup-admin-created");
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.screen = Screen::Extras;
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // Click-toggle never selects; Space without a cursor is a no-op.
        wizard.act(Act::Toggle(1));
        assert!(wizard.extras[1] && wizard.extras_sel.is_none());
        handle_key(&mut wizard, key(KeyCode::Char(' ')));
        assert!(wizard.extras[1]);
        // Arrows pick the cursor up; Space toggles under it.
        handle_key(&mut wizard, key(KeyCode::Down));
        assert_eq!(wizard.extras_sel, Some(0));
        handle_key(&mut wizard, key(KeyCode::Char(' ')));
        assert!(!wizard.extras[0], "the default-on updates card toggled off");
        // Esc walks back to the folders screen (extras is step 2 now).
        handle_key(&mut wizard, key(KeyCode::Esc));
        assert_eq!(wizard.screen, Screen::Folders);
        // Skip completes setup as public mode…
        wizard.screen = Screen::Login;
        wizard.act(Act::SkipLogin);
        wizard.act(Act::SkipConfirm);
        assert!(wizard.public);
        assert_eq!(wizard.screen, Screen::Done, "skip finishes the flow");
        // …and creating the admin instead would have ended public mode.
        wizard.apply(Done::AdminCreated(Ok("tok".to_string())));
        assert!(!wizard.public, "an account ends public mode");
        assert_eq!(wizard.screen, Screen::Done);
        // The session write landed INSIDE the scratch — the tripwire for
        // the guard above ever being removed.
        let saved = std::fs::read_to_string(scratch.dir.join("config.toml"))
            .expect("remember_session wrote into the scratch");
        assert!(saved.contains("127.0.0.1:9"));
    }

    #[test]
    fn going_back_and_continuing_recommits_only_the_new_folders() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.add_folder("/media/a".to_string());
        wizard.folders[0].committed = true;
        wizard.screen = Screen::Login;
        wizard.act(Act::BackToFolders);
        assert_eq!(wizard.screen, Screen::Folders);
        wizard.add_folder("/media/b".to_string());
        wizard.queued = Some(Op::CommitFolders);
        let (tx, rx) = std::sync::mpsc::channel();
        wizard.dispatch_queued(&tx);
        let batches: Vec<_> = rx
            .try_iter()
            .filter_map(|(_, job)| match job {
                Job::Folders(batch) => Some(batch),
                _ => None,
            })
            .collect();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.len(), 1, "only the NEW folder goes to the server");
        assert_eq!(batch[0].1, "/media/b");
    }

    #[test]
    fn renames_run_on_the_line_editor_with_the_vpath_charset_gate() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.add_folder("/tmp/music".to_string());
        wizard.act(Act::RenameFolder(0));
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        // Mid-line cursor editing; uppercase folds at the gate.
        handle_key(&mut wizard, key(KeyCode::Left));
        handle_key(&mut wizard, key(KeyCode::Left));
        handle_key(&mut wizard, key(KeyCode::Char('Z')));
        // Illegal characters are dropped whole.
        handle_key(&mut wizard, key(KeyCode::Char(' ')));
        handle_key(&mut wizard, key(KeyCode::Char('!')));
        let (row, draft) = wizard.editing.as_ref().unwrap();
        assert_eq!(*row, 0);
        assert_eq!(draft.value(), "medzia");
        assert_eq!(draft.cursor(), 4, "the cursor sits after the insert");
        handle_key(&mut wizard, key(KeyCode::Enter));
        assert_eq!(wizard.folders[0].name, "medzia");
    }

    #[test]
    fn re_clicking_the_edited_chip_keeps_the_draft_and_blur_commits_it() {
        let client = Client::new("http://127.0.0.1:9").expect("client");
        let mut wizard = Wizard::new(client);
        wizard.add_folder("/tmp/music".to_string());
        wizard.act(Act::RenameFolder(0));
        assert_eq!(wizard.editing.as_ref().map(|(_, d)| d.value()), Some("media"));
        // Mid-edit draft; clicking the same chip again must not clobber it.
        wizard.editing = Some((0, "vinyl".into()));
        wizard.act(Act::RenameFolder(0));
        assert_eq!(wizard.editing.as_ref().map(|(_, d)| d.value()), Some("vinyl"));
        // The blur path: finish_rename commits, Enter's semantics.
        wizard.finish_rename();
        assert!(wizard.editing.is_none());
        assert_eq!(wizard.folders[0].name, "vinyl");
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
        assert_eq!(wizard.screen, Screen::Done, "login is the last step — skip completes");
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
