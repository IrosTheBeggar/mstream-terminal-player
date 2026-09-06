//! The Add-torrent room: hand the server a torrent — a `.torrent` file or
//! a magnet link — to download into a library at a music-shaped path,
//! with the smart part done here: the torrent's own name parsed into
//! artist/album/year, the library's path template resolved, and the
//! files checked for already being on disk before anything downloads.
//! And because a torrent can turn out not to be for the server at all,
//! the flow can hand it onward to a real torrent client instead.
//!
//! Contract: docs/ux-contracts/add-torrent.md (clause numbers below cite
//! it). The record is the mobile app's smart panel; this surface's
//! answers to the contract's open questions live in its deviations log:
//! the room is a **Settings doorway** (Manage servers' shape) plus the
//! `--torrent` command-line seam that stands in for the OS hand-off; a
//! second `--torrent` while one runs simply opens a second instance; the
//! webapp's fuller seed-check wordings are adopted; the hand-off is one
//! text verb on the file chip; and the OS-defaults row waits for the
//! installers that will register the file types.
//!
//! Network: the servers room's shape — every call runs a one-shot
//! [`Client`] on its own thread, built from the live session, and the
//! replies fold back between draws through [`poll`]. The feature is this
//! GUI's alone, so nothing rides the shared App worker. Local disk work
//! (a listing, a file read, the hand-off) runs on threads too: a dead
//! network mount must never freeze the loop (the kit's rule).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::Client;
use crate::api::types::{
    SeedCheck, SeedMatch, TorrentAddRequest, TorrentAdded, TorrentDetect, TorrentDetectMeta,
    TorrentPreflight, TorrentSource,
};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{
    dim, input_display, modal_close, modal_frame, modal_frame_anchored, scroll_list, table_view,
    tall_button,
};
use crate::tui::worker::Event;

use super::torrent_meta::{self as meta, Confidence, TorrentMeta};
use super::{Act, Gui, accent, bright_bold, put, sel};

// ── State ───────────────────────────────────────────────────────────────────

/// A loaded `.torrent`: its bytes and the name it arrived under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadedFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A torrent the player was opened WITH (entry point 2): the `--torrent`
/// seam's parsed argument, waiting on the chooser's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incoming {
    File(LoadedFile),
    Magnet(String),
}

/// The keyboard rows, top to bottom. Which ones exist depends on the
/// state ([`TorrentUi::rows`]): a lone library is not a choice, the
/// magnet row yields to the file chip, and everything below the source
/// stays hidden until a source is real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Library,
    File,
    Magnet,
    Artist,
    Album,
    Year,
    Path,
    Rename,
    Force,
    Submit,
}

impl Row {
    /// Text rows take the typed keys while they hold the cursor.
    fn is_text(self) -> bool {
        matches!(self, Row::Magnet | Row::Artist | Row::Album | Row::Year | Row::Path)
    }
}

/// What the gate has said, for the session it was asked of.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Gate {
    NotAsked,
    Pending,
    Answered(TorrentPreflight),
    Failed(String),
}

impl Gate {
    fn ok(&self) -> bool {
        matches!(self, Gate::Answered(p) if p.ok())
    }
}

/// A background call in flight — one at a time, the record's own
/// `_submitting` / `_detecting` / `_passingOff` split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Busy {
    /// The native file dialog is up.
    Picking,
    Detecting,
    Checking,
    Adding,
    HandingOff,
}

/// The arrival chooser: "add it here, or hand it on?" (entry point 2,
/// clause 51). Row 0 adds, row 1 hands off, row 2 is the don't-ask box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chooser {
    pub incoming: Incoming,
    pub row: usize,
    pub dont_ask: bool,
}

/// One entry of a listed folder: a sub-folder, or a `.torrent` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickEntry {
    pub name: String,
    pub dir: bool,
}

/// The typed file picker (clause 2), the wizard's path modal worn for
/// files: a line editor over the local filesystem, suggestions for the
/// folder the text sits in, Tab to complete.
#[derive(Debug, Clone, Default)]
pub(crate) struct Picker {
    pub text: Input,
    /// The folder the cached entries belong to (with its separator).
    pub listed_for: String,
    pub entries: Vec<PickEntry>,
    /// Keyboard cursor within the current suggestions, if any.
    pub sel: Option<usize>,
    pub sel_anchor: Option<usize>,
    pub scroll: usize,
    /// Why the current folder could not be listed, or a load failed —
    /// silence would read as "nothing here".
    pub error: Option<String>,
}

impl Picker {
    /// Indices of the entries that match the current partial segment.
    fn suggestions(&self) -> Vec<usize> {
        let (_, partial) = crate::setup::split_input(self.text.value());
        let partial = partial.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name.to_lowercase().starts_with(&partial))
            .map(|(i, _)| i)
            .collect()
    }
}

/// The partial-match picker (clause 21): the places some of the files
/// already live, and the way to download fresh instead.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Matches {
    pub list: Vec<SeedMatch>,
    /// Cursor over the matches, then the download-fresh row at `len`.
    pub row: usize,
}

/// What a typed path turned out to be, once a thread looked.
enum Loaded {
    Dir,
    File(Vec<u8>),
}

/// How a hand-off went.
enum HandOff {
    /// The opener ran (or is still running — a client took the file).
    Launched,
    /// The opener said nothing will take it.
    Nothing,
    /// `MSTREAM_NO_OPEN`: the staged file's path, for the note.
    Headless(String),
}

/// What the threads send home.
enum Reply {
    /// The native file dialog's answer.
    Picked(crate::setup::picker::Pick),
    Preflight {
        server: String,
        gate: Result<TorrentPreflight, String>,
        templates: HashMap<String, String>,
    },
    Listing {
        dir: String,
        result: Result<Vec<PickEntry>, String>,
    },
    Loaded {
        path: String,
        result: Result<Loaded, String>,
    },
    Detected(Result<TorrentDetect, String>),
    Checked(Result<SeedCheck, String>),
    Added(Result<TorrentAdded, String>),
    HandedOff(Result<HandOff, String>),
}

pub(crate) struct TorrentUi {
    /// The room replaces the Settings rows, like Manage servers.
    pub room: bool,
    /// The keyboard cursor: None is stowed (↓ picks it up, Esc stows it,
    /// Esc again walks back to Settings).
    pub cursor: Option<Row>,
    pub gate: Gate,
    /// Which session's server the gate answers for — a switch stales it.
    pub gate_for: String,
    /// Per-library path templates, once the server has said.
    pub templates: HashMap<String, String>,
    /// The chosen library, as an index into the App's list.
    pub vpath: usize,
    pub file: Option<LoadedFile>,
    pub magnet: Input,
    pub artist: Input,
    pub album: Input,
    pub year: Input,
    pub path: Input,
    /// A hand-edited path stops recomputing (clause 13).
    pub path_edited: bool,
    pub rename_root: bool,
    pub force_fresh: bool,
    pub busy: Option<Busy>,
    pub picker: Option<Picker>,
    pub chooser: Option<Chooser>,
    pub matches: Option<Matches>,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
}

impl TorrentUi {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        TorrentUi {
            room: false,
            cursor: None,
            gate: Gate::NotAsked,
            gate_for: String::new(),
            templates: HashMap::new(),
            vpath: 0,
            file: None,
            magnet: Input::default(),
            artist: Input::default(),
            album: Input::default(),
            year: Input::default(),
            path: Input::default(),
            path_edited: false,
            rename_root: true,
            force_fresh: false,
            busy: None,
            picker: None,
            chooser: None,
            matches: None,
            tx,
            rx,
        }
    }

    /// Whether one of this room's overlays owns the pointer and keys.
    pub(crate) fn modal_open(&self) -> bool {
        self.picker.is_some() || self.chooser.is_some() || self.matches.is_some()
    }

    /// A source is real: a picked file, or a magnet with an infohash
    /// (clause 4). Everything below the source waits on this.
    fn has_source(&self) -> bool {
        self.file.is_some() || meta::is_valid_magnet(self.magnet.value())
    }

    /// The metadata as the fields hold it.
    fn meta(&self) -> TorrentMeta {
        TorrentMeta::new(self.artist.value(), self.album.value(), self.year.value(), Confidence::None)
    }

    /// The rows that exist right now, top to bottom.
    fn rows(&self, libraries: usize) -> Vec<Row> {
        let mut rows = Vec::new();
        if libraries != 1 {
            rows.push(Row::Library);
        }
        rows.push(Row::File);
        if self.file.is_none() {
            rows.push(Row::Magnet);
        }
        if self.has_source() {
            rows.extend([Row::Artist, Row::Album, Row::Year, Row::Path, Row::Rename]);
            if self.file.is_some() {
                rows.push(Row::Force);
            }
            rows.push(Row::Submit);
        }
        rows
    }

    fn input_mut(&mut self, row: Row) -> Option<&mut Input> {
        match row {
            Row::Magnet => Some(&mut self.magnet),
            Row::Artist => Some(&mut self.artist),
            Row::Album => Some(&mut self.album),
            Row::Year => Some(&mut self.year),
            Row::Path => Some(&mut self.path),
            _ => None,
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// The user's home, the meaning of a typed `~` (the wizard's rule).
fn home() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))
}

fn sep_for(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') { '\\' } else { '/' }
}

/// Where the picker starts: the platform's Downloads — a torrent almost
/// always arrives through the browser (clause 2) — or the home when
/// there is no such folder.
fn picker_start() -> String {
    let Some(home) = home() else { return String::new() };
    let home = home.trim_end_matches(['/', '\\']).to_string();
    let sep = sep_for(&home);
    let downloads = format!("{home}{sep}Downloads");
    if Path::new(&downloads).is_dir() {
        format!("{downloads}{sep}")
    } else {
        format!("{home}{sep}")
    }
}

/// A `~` at the front of a typed path, expanded.
fn expand_tilde(path: &str) -> String {
    if (path == "~" || path.starts_with("~/") || path.starts_with("~\\"))
        && let Some(home) = home()
    {
        return path.replacen('~', home.trim_end_matches(['/', '\\']), 1);
    }
    path.to_string()
}

/// A path as a terminal drops it: shells escape spaces with backslashes
/// and some quote the whole thing — both are undone so the text names
/// the file.
fn unescape_dropped(path: &str) -> String {
    let trimmed = path.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| trimmed.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(trimmed);
    unquoted.replace("\\ ", " ")
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Where a hand-off stages the file it hands over — and the way a bounced
/// hand-off is recognized: an arrival from inside this folder is our own
/// file coming back, which means this player is the system's default for
/// torrents and the loop must be named, not repeated.
fn handoff_dir() -> PathBuf {
    std::env::temp_dir().join("mstream-player-torrents")
}

/// List a LOCAL folder for the picker: sub-folders (symlinks resolved)
/// first, then `.torrent` files, each sorted case-insensitively. Runs on
/// a thread: a dead network mount can hang `read_dir`.
fn list_dir(dir: &str) -> Result<Vec<PickEntry>, String> {
    if dir.is_empty() {
        return Err("nothing to list".to_string());
    }
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = kind.is_dir()
            || (kind.is_symlink() && std::fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()));
        if is_dir {
            dirs.push(name);
        } else if name.to_lowercase().ends_with(".torrent") {
            files.push(name);
        }
    }
    dirs.sort_by_key(|a| a.to_lowercase());
    files.sort_by_key(|a| a.to_lowercase());
    Ok(dirs
        .into_iter()
        .map(|name| PickEntry { name, dir: true })
        .chain(files.into_iter().map(|name| PickEntry { name, dir: false }))
        .collect())
}

/// Read a typed path: a folder to descend into, or a file's bytes.
fn load_path(path: &str) -> Result<Loaded, String> {
    let md = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if md.is_dir() {
        return Ok(Loaded::Dir);
    }
    std::fs::read(path).map(Loaded::File).map_err(|e| e.to_string())
}

/// Hand a file or a magnet link to the OS's opener and see whether
/// anything took it. `MSTREAM_NO_OPEN` is the test seam — headless
/// callers put the staged path in the note instead.
fn open_target(target: &str) -> Result<HandOff, String> {
    if std::env::var("MSTREAM_NO_OPEN").is_ok_and(|v| !v.is_empty() && v != "0") {
        return Ok(HandOff::Headless(target.to_string()));
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start", ""]).arg(target);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(target);
        c
    };
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // The openers answer fast when nothing can take the file (macOS's
    // `open` exits 1, xdg-open 3); one that is still running after a
    // moment has handed it to a client that is now starting up.
    for _ in 0..20 {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(if status.success() { HandOff::Launched } else { HandOff::Nothing });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(HandOff::Launched)
}

/// A one-shot client for the live session, the servers room's way.
fn session_client(gui: &Gui) -> Result<Client, String> {
    let session = &gui.app.session;
    if !gui.app.connected || session.server.is_empty() {
        return Err(t!("gui.no_server").to_string());
    }
    Client::new_with(&session.server, session.self_signed)
        .map(|c| c.with_token(session.token.clone()))
        .map_err(|e| e.to_string())
}

/// The chosen library's name, if the server has any.
fn library(gui: &Gui) -> Option<String> {
    gui.app.libraries.get(gui.torrent.vpath).cloned()
}

fn note(gui: &mut Gui, text: impl Into<String>, is_err: bool) {
    gui.note = Some((text.into(), is_err));
}

// ── Opening the room, and what arrives in it ────────────────────────────────

/// Open the room from Settings (entry point 1) and ask the gate.
pub(crate) fn open_room(gui: &mut Gui) {
    gui.active = super::SETTINGS_NAV;
    gui.cursor = None;
    gui.servers.room = false;
    gui.servers.drop_open = false;
    gui.torrent.room = true;
    gui.torrent.cursor = None;
    request_gate(gui);
}

/// Open the room WITH a torrent already in hand (entry point 2).
fn open_room_with(gui: &mut Gui, incoming: Incoming) {
    open_room(gui);
    match incoming {
        Incoming::File(file) => set_file(gui, file),
        Incoming::Magnet(link) => {
            gui.torrent.magnet = Input::new(link);
            magnet_changed(gui);
        }
    }
}

/// Ask `/torrent/preflight` (and the path templates, best-effort) for the
/// live session. Not connected: nothing to ask — the room says so, and
/// [`observe`] asks the moment a Connected lands.
fn request_gate(gui: &mut Gui) {
    let server = gui.app.session.server.clone();
    if gui.torrent.gate_for == server && !matches!(gui.torrent.gate, Gate::NotAsked | Gate::Failed(_)) {
        return; // answered, or on the wire, for this very server
    }
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(_) => {
            gui.torrent.gate = Gate::NotAsked;
            gui.torrent.gate_for.clear();
            return;
        }
    };
    gui.torrent.gate = Gate::Pending;
    gui.torrent.gate_for = server.clone();
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let gate = client.torrent_preflight().map_err(|e| e.to_string());
        // Templates are best-effort: an older server has no route, and the
        // legacy Artist/Album layout still applies (clause 12).
        let templates = client
            .torrent_path_templates()
            .map(|t| {
                t.vpaths
                    .into_iter()
                    .filter_map(|(k, v)| v.template.filter(|t| !t.is_empty()).map(|t| (k, t)))
                    .collect()
            })
            .unwrap_or_default();
        let _ = tx.send(Reply::Preflight { server, gate, templates });
    });
}

/// The event loop's hook: a fresh connection stales the gate and, with
/// the room open, asks again for the new server.
pub(crate) fn observe(gui: &mut Gui, event: &Event) {
    if let Event::Connected { .. } = event {
        gui.torrent.gate = Gate::NotAsked;
        gui.torrent.gate_for.clear();
        gui.torrent.templates.clear();
        gui.torrent.vpath = 0;
        if gui.torrent.room {
            request_gate(gui);
        }
    }
}

/// A torrent the player was opened with: `mstream-player gui --torrent
/// <file-or-magnet>` — the seam the installers' file associations will
/// launch. What happens next is the ask-me setting's call (entry point
/// 2): the chooser, or straight into the room. Every arrival is logged —
/// "I opened a torrent and nothing happened" is untriageable otherwise.
pub(crate) fn arrive(gui: &mut Gui, arg: &str) {
    let arg = unescape_dropped(arg);
    let bounced;
    let incoming = if arg.to_lowercase().starts_with("magnet:") {
        bounced = false;
        if !meta::is_valid_magnet(&arg) {
            tracing::info!("torrent arrival: an invalid magnet link, refused");
            note(gui, t!("gui.tor.magnet_invalid"), true);
            return;
        }
        Incoming::Magnet(arg.clone())
    } else {
        let path = expand_tilde(&arg);
        bounced = Path::new(&path).starts_with(handoff_dir());
        let name = basename(&path);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::info!("torrent arrival: {path} could not be read: {e}");
                note(gui, t!("gui.tor.read_failed", name = name, err = e), true);
                return;
            }
        };
        if !meta::is_torrent_file(&bytes) {
            tracing::info!("torrent arrival: {path} is not a torrent, refused");
            note(gui, t!("gui.tor.not_a_torrent", name = name), true);
            return;
        }
        Incoming::File(LoadedFile { name, bytes })
    };
    let what = match &incoming {
        Incoming::File(f) => format!("file {} ({} bytes)", f.name, f.bytes.len()),
        Incoming::Magnet(_) => "magnet".to_string(),
    };
    if bounced {
        // Our own hand-off came back: this player is the default app for
        // torrents. Say so and take the torrent — looping would be worse.
        tracing::info!("torrent arrival: {what} -> bounced hand-off (we are the default app)");
        open_room_with(gui, incoming);
        note(gui, t!("gui.tor.handoff_loop"), true);
        return;
    }
    let ask = gui.config.torrent.ask;
    tracing::info!("torrent arrival: {what} -> {}", if ask { "chooser" } else { "add" });
    if ask {
        gui.torrent.chooser = Some(Chooser { incoming, row: 0, dont_ask: false });
    } else {
        open_room_with(gui, incoming);
    }
}

// ── The source and the metadata ─────────────────────────────────────────────

/// A file is the source now: the magnet goes (clause 1), and the
/// torrent's own name pre-fills the metadata (clause 10).
fn set_file(gui: &mut Gui, file: LoadedFile) {
    let name = meta::extract_torrent_name(&file.bytes);
    gui.torrent.magnet = Input::default();
    gui.torrent.file = Some(file);
    if name.is_empty() {
        gui.torrent.path_edited = false;
        recompute_path(gui);
    } else {
        apply_meta(gui, meta::parse_music_name(&name), true);
    }
}

/// The magnet field changed: typing a magnet drops the file (clause 1),
/// and its `dn` pre-fills the metadata (clause 10).
fn magnet_changed(gui: &mut Gui) {
    let value = gui.torrent.magnet.value().trim().to_string();
    if !value.is_empty() && gui.torrent.file.is_some() {
        gui.torrent.file = None;
    }
    if let Some(dn) = meta::magnet_display_name(&value) {
        apply_meta(gui, meta::parse_music_name(&dn), true);
    }
}

fn apply_meta(gui: &mut Gui, m: TorrentMeta, reset_path_edited: bool) {
    gui.torrent.artist = Input::new(m.artist);
    gui.torrent.album = Input::new(m.album);
    gui.torrent.year = Input::new(m.year);
    if reset_path_edited {
        gui.torrent.path_edited = false;
    }
    recompute_path(gui);
}

/// The destination from the library's template (clause 12) — unless a
/// hand edit made the path sticky (clause 13).
fn recompute_path(gui: &mut Gui) {
    if gui.torrent.path_edited {
        return;
    }
    let template = library(gui).and_then(|v| gui.torrent.templates.get(&v).cloned());
    let path = meta::compute_path(template.as_deref(), &gui.torrent.meta());
    gui.torrent.path = Input::new(path);
}

/// A text row's value changed under the keys.
fn text_changed(gui: &mut Gui, row: Row) {
    match row {
        Row::Magnet => magnet_changed(gui),
        Row::Path => gui.torrent.path_edited = true,
        Row::Artist | Row::Album | Row::Year => recompute_path(gui),
        _ => {}
    }
}

/// Auto-detect (clause 11): the server reads the torrent and says how
/// sure it is.
fn auto_detect(gui: &mut Gui) {
    if gui.torrent.busy.is_some() {
        return;
    }
    let Some(file) = gui.torrent.file.clone() else { return };
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return note(gui, e, true),
    };
    let vpath = library(gui);
    gui.torrent.busy = Some(Busy::Detecting);
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = client
            .torrent_auto_detect(&file.bytes, &file.name, vpath.as_deref())
            .map_err(|e| e.to_string());
        let _ = tx.send(Reply::Detected(result));
    });
}

// ── Submit and the seed-existing check ──────────────────────────────────────

/// What a submit will do once the form has been read: check the disk
/// first (file sources, unless forced fresh), or add outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    Check(LoadedFile),
    Add(TorrentAddRequest),
}

/// Read the form into a plan, or the words for why not (clauses 1, 20).
fn plan_submit(gui: &Gui) -> Result<Plan, String> {
    let tor = &gui.torrent;
    let Some(vpath) = library(gui) else { return Err(t!("gui.tor.pick_library").to_string()) };
    let magnet = tor.magnet.value().trim().to_string();
    let has_file = tor.file.is_some();
    let has_magnet = !magnet.is_empty();
    if has_file == has_magnet {
        return Err(t!("gui.tor.one_source").to_string());
    }
    if has_magnet && !meta::is_valid_magnet(&magnet) {
        return Err(t!("gui.tor.magnet_invalid").to_string());
    }
    let (sub_path, directory_name) = meta::split_path(tor.path.value().trim());
    if directory_name.is_empty() {
        return Err(t!("gui.tor.path_empty").to_string());
    }
    if let Some(file) = tor.file.clone().filter(|_| !tor.force_fresh) {
        return Ok(Plan::Check(file));
    }
    Ok(Plan::Add(add_request(gui, vpath, sub_path, directory_name)))
}

fn add_request(gui: &Gui, vpath: String, sub_path: String, directory_name: String) -> TorrentAddRequest {
    let source = match &gui.torrent.file {
        Some(file) => TorrentSource::File { name: file.name.clone(), bytes: file.bytes.clone() },
        None => TorrentSource::Magnet(gui.torrent.magnet.value().trim().to_string()),
    };
    TorrentAddRequest {
        vpath,
        sub_path,
        directory_name,
        rename_root: gui.torrent.rename_root,
        source,
    }
}

fn submit(gui: &mut Gui) {
    if gui.torrent.busy.is_some() || !gui.torrent.gate.ok() {
        return;
    }
    match plan_submit(gui) {
        Err(words) => note(gui, words, true),
        Ok(Plan::Check(file)) => start_check(gui, file),
        Ok(Plan::Add(req)) => start_add(gui, req),
    }
}

/// The seed-existing check first (clause 20): if the files are already on
/// disk, seed them instead of downloading again.
fn start_check(gui: &mut Gui, file: LoadedFile) {
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return note(gui, e, true),
    };
    gui.torrent.busy = Some(Busy::Checking);
    note(gui, t!("gui.tor.checking_files"), false);
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = client.torrent_seed_existing(&file.bytes, &file.name).map_err(|e| e.to_string());
        let _ = tx.send(Reply::Checked(result));
    });
}

fn start_add(gui: &mut Gui, req: TorrentAddRequest) {
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return note(gui, e, true),
    };
    gui.torrent.busy = Some(Busy::Adding);
    note(gui, t!("gui.tor.submitting"), false);
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = client.torrent_add(&req).map_err(|e| e.to_string());
        let _ = tx.send(Reply::Added(result));
    });
}

/// The seed check's outcomes (clauses 20–23): done in words, refused in
/// the server's words, a match picker, or a fall-through to the add.
/// The webapp's two extra outcomes are worded too — a deliberate answer
/// to the contract's first open question.
fn apply_check(gui: &mut Gui, check: SeedCheck) {
    match check.outcome.as_str() {
        "seeded" => {
            note(gui, t!("gui.tor.seeded"), false);
            reset_source(gui);
        }
        "already_in_daemon" => {
            note(gui, t!("gui.tor.already_in_client"), false);
            reset_source(gui);
        }
        "invalid_torrent" => {
            let words = check.error.unwrap_or_else(|| t!("gui.tor.invalid_file").to_string());
            note(gui, words, true);
        }
        "match_unmapped" => {
            let vpath = check.vpath.unwrap_or_default();
            note(gui, t!("gui.tor.match_unmapped", vpath = vpath), true);
        }
        "pad_files_missing" => {
            let vpath = check.vpath.unwrap_or_default();
            let client = check.client_type.unwrap_or_else(|| "this client".to_string());
            note(gui, t!("gui.tor.pad_files_missing", vpath = vpath, client = client), true);
        }
        "partial_match" if !check.matches.is_empty() => {
            gui.note = None;
            gui.torrent.matches = Some(Matches { list: check.matches, row: 0 });
        }
        "daemon_error" => {
            // The CHECK failed, not the add: say it was skipped and go on.
            note(gui, t!("gui.tor.seed_check_failed"), false);
            add_at_typed_path(gui);
        }
        _ => add_at_typed_path(gui),
    }
}

/// A fresh add at the typed destination — the fall-through, and the
/// match picker's last row.
fn add_at_typed_path(gui: &mut Gui) {
    let Some(vpath) = library(gui) else { return note(gui, t!("gui.tor.pick_library"), true) };
    let (sub_path, directory_name) = meta::split_path(gui.torrent.path.value().trim());
    if directory_name.is_empty() {
        return note(gui, t!("gui.tor.path_empty"), true);
    }
    let req = add_request(gui, vpath, sub_path, directory_name);
    start_add(gui, req);
}

/// Seed what is there at a match's location and fetch only what is
/// missing (clause 21).
fn use_match(gui: &mut Gui, index: usize) {
    let Some(m) = gui.torrent.matches.as_ref().and_then(|ms| ms.list.get(index)).cloned() else {
        return;
    };
    let (sub_path, directory_name) = meta::split_path(&m.relative_path);
    if directory_name.is_empty() {
        return note(gui, t!("gui.tor.match_no_folder"), true);
    }
    gui.torrent.matches = None;
    let req = add_request(gui, m.vpath, sub_path, directory_name);
    start_add(gui, req);
}

/// After a success the source goes (the record pops its screen); the
/// library and the toggles stay for the next one.
fn reset_source(gui: &mut Gui) {
    let tor = &mut gui.torrent;
    tor.file = None;
    tor.magnet = Input::default();
    tor.artist = Input::default();
    tor.album = Input::default();
    tor.year = Input::default();
    tor.path = Input::default();
    tor.path_edited = false;
    tor.force_fresh = false;
    tor.cursor = None;
}

/// The add's answer (clauses 15, 22): the name it took, a duplicate's own
/// wording, and a rename that failed as a warning, not a failure.
fn apply_added(gui: &mut Gui, added: TorrentAdded) {
    let name = added
        .name
        .clone()
        .unwrap_or_else(|| meta::split_path(gui.torrent.path.value()).1);
    let words = if added.is_duplicate {
        t!("gui.tor.duplicate", name = name).to_string()
    } else {
        t!("gui.tor.added", name = name).to_string()
    };
    match added.rename_warning.filter(|w| !w.is_empty()) {
        Some(warning) => note(gui, format!("{words} — {warning}"), true),
        None => note(gui, words, false),
    }
    reset_source(gui);
}

// ── The hand-off ────────────────────────────────────────────────────────────

/// Open with… (clauses 60–61): stage the file and hand it to whatever the
/// system opens torrents with; a magnet link goes as itself. The form
/// stays standing — nothing was submitted.
fn hand_off(gui: &mut Gui, incoming: Option<Incoming>) {
    if gui.torrent.busy.is_some() {
        return;
    }
    let incoming = match incoming {
        Some(incoming) => incoming,
        None => match &gui.torrent.file {
            Some(file) => Incoming::File(file.clone()),
            None => {
                let magnet = gui.torrent.magnet.value().trim().to_string();
                if !meta::is_valid_magnet(&magnet) {
                    return;
                }
                Incoming::Magnet(magnet)
            }
        },
    };
    gui.torrent.busy = Some(Busy::HandingOff);
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let target = match incoming {
            Incoming::Magnet(link) => Ok(link),
            Incoming::File(file) => {
                let dir = handoff_dir();
                std::fs::create_dir_all(&dir)
                    .and_then(|()| {
                        let path = dir.join(&file.name);
                        std::fs::write(&path, &file.bytes).map(|()| path.to_string_lossy().into_owned())
                    })
                    .map_err(|e| e.to_string())
            }
        };
        let result = target.and_then(|t| open_target(&t));
        let _ = tx.send(Reply::HandedOff(result));
    });
}

// ── The picker ──────────────────────────────────────────────────────────────

/// Choose a file the platform's way (clause 2): the native dialog, typed
/// to `.torrent` and started in Downloads, on a thread so the loop stays
/// live. Where no dialog can open — SSH, no session bus, a refused
/// osascript — the reply says so and the typed picker takes over.
fn native_pick(gui: &mut Gui) {
    if gui.torrent.busy.is_some() {
        return;
    }
    gui.torrent.busy = Some(Busy::Picking);
    note(gui, t!("gui.tor.picking"), false);
    let start = {
        let dir = picker_start();
        let trimmed = dir.trim_end_matches(['/', '\\']);
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    };
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(Reply::Picked(crate::setup::picker::pick_torrent(start.as_deref())));
    });
}

/// The typed picker — the fallback, and its own road (`t`): a terminal is
/// where paths get pasted and dropped.
fn open_picker(gui: &mut Gui) {
    gui.torrent.picker = Some(Picker { text: Input::new(picker_start()), ..Picker::default() });
    picker_refresh(gui);
}

/// List the folder the text sits in, when it changed (the wizard's
/// completion, for files): a `~` expands as typed, doubled separators
/// collapse, and the listing runs on a thread.
fn picker_refresh(gui: &mut Gui) {
    let Some(picker) = gui.torrent.picker.as_mut() else { return };
    picker.sel = None;
    picker.sel_anchor = None;
    picker.scroll = 0;
    let raw = picker.text.value().to_string();
    if raw.is_empty() {
        picker.listed_for.clear();
        picker.entries.clear();
        picker.error = None;
        return;
    }
    let expanded = expand_tilde(&raw);
    let expanded = if raw == "~" { format!("{expanded}{}", sep_for(&expanded)) } else { expanded };
    let mut cleaned = String::with_capacity(expanded.len());
    let mut prev_sep = false;
    for (i, ch) in expanded.chars().enumerate() {
        let is_sep = ch == '/' || ch == '\\';
        if is_sep && prev_sep && i != 1 {
            continue;
        }
        prev_sep = is_sep;
        cleaned.push(ch);
    }
    if cleaned != raw {
        let from_end = raw.chars().count().saturating_sub(picker.text.cursor());
        let cursor = cleaned.chars().count().saturating_sub(from_end);
        picker.text = Input::new(cleaned.clone()).with_cursor(cursor);
    }
    let (dir, _) = crate::setup::split_input(&cleaned);
    let dir = if dir == "~" {
        match home() {
            Some(home) => format!("{}/", home.trim_end_matches(['/', '\\'])),
            None => return,
        }
    } else {
        dir
    };
    if dir == picker.listed_for {
        return;
    }
    picker.listed_for = dir.clone();
    picker.entries.clear();
    picker.error = None;
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = list_dir(&dir);
        let _ = tx.send(Reply::Listing { dir, result });
    });
}

/// Take suggestion `i`: a folder becomes the new place to type in, a
/// `.torrent` file is loaded.
fn picker_accept(gui: &mut Gui, i: usize) {
    let Some(picker) = gui.torrent.picker.as_mut() else { return };
    let Some(entry) = picker.suggestions().get(i).and_then(|&e| picker.entries.get(e)).cloned() else {
        return;
    };
    let base = picker.listed_for.clone();
    let sep = sep_for(&base);
    if entry.dir {
        picker.text = Input::new(format!("{base}{}{sep}", entry.name));
        picker_refresh(gui);
    } else {
        picker_load(gui, format!("{base}{}", entry.name));
    }
}

/// Enter: the picked suggestion, else whatever the text names — a folder
/// to descend into, a file to load, or a failure said under the input.
fn picker_enter(gui: &mut Gui) {
    let Some(picker) = gui.torrent.picker.as_ref() else { return };
    if let Some(i) = picker.sel {
        return picker_accept(gui, i);
    }
    let value = unescape_dropped(picker.text.value());
    if value.is_empty() {
        return;
    }
    picker_load(gui, expand_tilde(&value));
}

/// Tab (or → at the end of the line): the picked suggestion, the single
/// match, or the longest common prefix — and when that gains nothing,
/// start cycling.
fn picker_complete(gui: &mut Gui) {
    let Some(picker) = gui.torrent.picker.as_mut() else { return };
    let suggestions = picker.suggestions();
    if let Some(i) = picker.sel {
        return picker_accept(gui, i);
    }
    if suggestions.len() == 1 {
        return picker_accept(gui, 0);
    }
    if suggestions.is_empty() {
        return;
    }
    let names: Vec<String> = suggestions.iter().map(|&i| picker.entries[i].name.clone()).collect();
    let (_, partial) = crate::setup::split_input(picker.text.value());
    let lcp = crate::setup::common_prefix(&names);
    if lcp.chars().count() > partial.chars().count() {
        let keep = picker.text.value().chars().count() - partial.chars().count();
        let extended = picker.text.value().chars().take(keep).collect::<String>() + &lcp;
        picker.text = Input::new(extended);
        picker.sel = None;
    } else {
        picker.sel = Some(0);
    }
}

fn picker_load(gui: &mut Gui, path: String) {
    if let Some(picker) = gui.torrent.picker.as_mut() {
        picker.error = None;
    }
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = load_path(&path);
        let _ = tx.send(Reply::Loaded { path, result });
    });
}

// ── Background replies ──────────────────────────────────────────────────────

/// Drain what the threads sent since the last pass.
pub(crate) fn poll(gui: &mut Gui) {
    while let Ok(reply) = gui.torrent.rx.try_recv() {
        apply_reply(gui, reply);
    }
}

fn apply_reply(gui: &mut Gui, reply: Reply) {
    match reply {
        Reply::Picked(pick) => {
            use crate::setup::picker::Pick;
            gui.torrent.busy = None;
            match pick {
                Pick::File(path) => {
                    gui.note = None;
                    picker_load(gui, path.to_string_lossy().into_owned());
                }
                Pick::Cancelled => gui.note = None,
                // A folder from a file dialog cannot happen; said rather
                // than assumed.
                Pick::Folder(_) => gui.note = None,
                Pick::Unavailable(why) => {
                    note(gui, t!("gui.tor.no_picker", why = why), false);
                    open_picker(gui);
                }
            }
        }
        Reply::Preflight { server, gate, templates } => {
            // A switch since the ask: the answer describes another server.
            if server != gui.torrent.gate_for || server != gui.app.session.server {
                return;
            }
            gui.torrent.gate = match gate {
                Ok(preflight) => Gate::Answered(preflight),
                Err(e) => Gate::Failed(e),
            };
            gui.torrent.templates = templates;
            recompute_path(gui);
        }
        Reply::Listing { dir, result } => {
            if let Some(picker) = gui.torrent.picker.as_mut().filter(|p| p.listed_for == dir) {
                match result {
                    Ok(entries) => picker.entries = entries,
                    Err(e) => picker.error = Some(t!("gui.tor.picker_list_failed", dir = dir, err = e).to_string()),
                }
            }
        }
        Reply::Loaded { path, result } => {
            let name = basename(&path);
            match result {
                Ok(Loaded::Dir) => {
                    if let Some(picker) = gui.torrent.picker.as_mut() {
                        let sep = sep_for(&path);
                        let dir = path.trim_end_matches(['/', '\\']).to_string();
                        picker.text = Input::new(format!("{dir}{sep}"));
                        picker_refresh(gui);
                    }
                }
                Ok(Loaded::File(bytes)) if meta::is_torrent_file(&bytes) => {
                    gui.torrent.picker = None;
                    set_file(gui, LoadedFile { name, bytes });
                    gui.torrent.cursor = Some(Row::File);
                }
                Ok(Loaded::File(_)) => {
                    let words = t!("gui.tor.not_a_torrent", name = name).to_string();
                    match gui.torrent.picker.as_mut() {
                        Some(picker) => picker.error = Some(words),
                        None => note(gui, words, true),
                    }
                }
                Err(e) => {
                    let words = t!("gui.tor.read_failed", name = name, err = e).to_string();
                    match gui.torrent.picker.as_mut() {
                        Some(picker) => picker.error = Some(words),
                        None => note(gui, words, true),
                    }
                }
            }
        }
        Reply::Detected(result) => {
            gui.torrent.busy = None;
            match result {
                Ok(detect) if detect.ok => {
                    if let Some(md) = &detect.metadata {
                        let m = TorrentMeta::new(
                            &TorrentDetectMeta::text(&md.artist),
                            &TorrentDetectMeta::text(&md.album),
                            &TorrentDetectMeta::text(&md.year),
                            Confidence::None,
                        );
                        apply_meta(gui, m, true);
                    }
                    let sure = detect.confidence.as_deref() == Some("high");
                    let words = if sure { t!("gui.tor.detected") } else { t!("gui.tor.detect_guess") };
                    note(gui, words, false);
                }
                Ok(detect) => {
                    let words = detect
                        .message
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| t!("gui.tor.detect_none").to_string());
                    note(gui, words, true);
                }
                Err(e) => note(gui, e, true),
            }
        }
        Reply::Checked(result) => {
            gui.torrent.busy = None;
            match result {
                Ok(check) => apply_check(gui, check),
                Err(e) => note(gui, e, true),
            }
        }
        Reply::Added(result) => {
            gui.torrent.busy = None;
            match result {
                Ok(added) => apply_added(gui, added),
                Err(e) => note(gui, t!("gui.tor.add_failed", err = e), true),
            }
        }
        Reply::HandedOff(result) => {
            gui.torrent.busy = None;
            match result {
                Ok(HandOff::Launched) => note(gui, t!("gui.tor.handed_off"), false),
                Ok(HandOff::Nothing) => note(gui, t!("gui.tor.open_with_none"), true),
                Ok(HandOff::Headless(path)) => note(gui, t!("gui.tor.handoff_staged", path = path), false),
                Err(e) => note(gui, format!("{}: {e}", t!("gui.tor.open_with_failed")), true),
            }
        }
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The label column: the widest label plus a gap, so translated labels
/// line up whatever their length.
fn label_width() -> u16 {
    let labels = [
        t!("gui.tor.library"),
        t!("gui.tor.source"),
        t!("gui.tor.magnet"),
        t!("gui.tor.artist"),
        t!("gui.tor.album"),
        t!("gui.tor.year"),
        t!("gui.tor.path"),
    ];
    let widest = labels.iter().map(|l| l.chars().count()).max().unwrap_or(8) as u16;
    (widest + 2).clamp(8, 18)
}

/// A row's label: dim at rest, the accent while the keyboard cursor is on
/// the row, bright under the pointer (the Settings rows' grammar).
fn label_style(focused: bool, hover: bool) -> Style {
    match (focused, hover) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => dim(),
    }
}

/// One text row: label, the value (with the caret while focused, a dim
/// placeholder while empty), and an optional trailing word at the right
/// edge — the magnet's invalid mark (clause 4).
#[allow(clippy::too_many_arguments)]
fn draw_text_row(
    frame: &mut Frame,
    gui: &mut Gui,
    content: Rect,
    y: u16,
    row: Row,
    label: &str,
    placeholder: Option<&str>,
    trailing: Option<(String, Style)>,
) {
    let rect = Rect { x: content.x, y, width: content.width, height: 1 };
    let focused = gui.torrent.cursor == Some(row);
    let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
    put(frame, content.x, y, label, label_style(focused, hover));
    let lw = label_width();
    let trailing_w = trailing.as_ref().map_or(0, |(t, _)| t.chars().count() as u16 + 2);
    let vx = content.x + lw;
    let avail = content.width.saturating_sub(lw + trailing_w + 1);
    let (value, cursor) = {
        let input = match row {
            Row::Magnet => &gui.torrent.magnet,
            Row::Artist => &gui.torrent.artist,
            Row::Album => &gui.torrent.album,
            Row::Year => &gui.torrent.year,
            _ => &gui.torrent.path,
        };
        (input.value().to_string(), input.cursor())
    };
    if focused {
        put(frame, vx, y, &input_display(&value, cursor, avail), Style::default());
    } else if value.is_empty() {
        if let Some(placeholder) = placeholder {
            put(frame, vx, y, &super::bar::clip(placeholder, avail as usize), dim());
        }
    } else {
        let style = if hover { Style::default().fg(th().bright) } else { Style::default() };
        put(frame, vx, y, &super::bar::clip(&value, avail as usize), style);
    }
    if let Some((text, style)) = trailing {
        put(frame, content.right().saturating_sub(text.chars().count() as u16), y, &text, style);
    }
    gui.ui.click(rect, Act::TorRow(row));
}

/// A 1-row text button: dim at rest, bright under the pointer; the
/// accent when it is the row's one way forward. Returns its width.
fn text_button(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, label: &str, lead: bool, act: Act) -> u16 {
    let rect = Rect { x, y, width: label.chars().count() as u16, height: 1 };
    let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
    let style = match (hover, lead) {
        (true, _) => bright_bold(),
        (false, true) => accent(),
        (false, false) => dim(),
    };
    put(frame, x, y, label, style);
    gui.ui.click(rect, act);
    rect.width
}

/// The room (entry point 1's screen): the gate's banner, then the form
/// revealed in steps — source, metadata, destination, options, submit.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    // Title row: the way back, the name, the gate's standing at the right.
    let back = Rect { x: content.x, y: content.y, width: 1, height: 1 };
    let bhover = gui.ui.pointer.is_some_and(|p| back.contains(p));
    put(frame, back.x, back.y, if legacy_conhost() { "<" } else { "◂" }, if bhover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::TorBack);
    gui.ui.tip(back, t!("gui.tor.back_tip").to_string());
    put(frame, content.x + 2, content.y, &t!("gui.tor.title"), Style::default().add_modifier(Modifier::BOLD));
    let standing = match &gui.torrent.gate {
        Gate::Pending => Some((t!("gui.tor.checking").to_string(), accent())),
        Gate::Answered(p) if p.ok() => {
            let client = p.display_name.clone().or_else(|| p.client_type.clone()).unwrap_or_default();
            Some((t!("gui.tor.ready", client = client).to_string(), dim()))
        }
        Gate::NotAsked if !gui.app.connected => Some((t!("gui.no_server").to_string(), dim())),
        _ => None,
    };
    if let Some((text, style)) = standing {
        let shown = super::bar::clip(&text, content.width.saturating_sub(16) as usize);
        put(frame, content.right().saturating_sub(shown.chars().count() as u16), content.y, &shown, style);
    }

    // The banner (the record's reason box): the server's own words for
    // why it can't take one, or a hint while nothing is loaded yet.
    let banner = match &gui.torrent.gate {
        Gate::Answered(p) if !p.ok() => Some((
            p.reason.clone().unwrap_or_else(|| t!("gui.tor.unavailable").to_string()),
            Style::default().fg(th().gold),
        )),
        Gate::Failed(e) => Some((t!("gui.tor.preflight_failed", err = e).to_string(), Style::default().fg(th().gold))),
        _ if !gui.torrent.has_source() => Some((t!("gui.tor.hint").to_string(), dim())),
        _ => None,
    };
    if let Some((text, style)) = banner {
        put(frame, content.x, content.y + 1, &super::bar::clip(&text, content.width as usize), style);
    }

    let roomy = content.height >= 18;
    let gap = u16::from(roomy);
    let lw = label_width();
    let libraries = gui.app.libraries.clone();
    let rows = gui.torrent.rows(libraries.len());
    let mut y = content.y + 2 + gap;
    let forward = if legacy_conhost() { ">" } else { "▸" };

    for row in rows {
        // The metadata block breathes when the room has the height.
        if row == Row::Artist {
            y += gap;
        }
        match row {
            Row::Library => {
                let rect = Rect { x: content.x, y, width: content.width, height: 1 };
                let focused = gui.torrent.cursor == Some(Row::Library);
                let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
                put(frame, content.x, y, &t!("gui.tor.library"), label_style(focused, hover));
                if libraries.is_empty() {
                    put(frame, content.x + lw, y, &t!("gui.tor.no_libraries"), dim());
                    gui.ui.click(rect, Act::TorRow(Row::Library));
                } else {
                    // ◂ name ▸: the arrows step, the name cycles.
                    let name = libraries.get(gui.torrent.vpath).cloned().unwrap_or_default();
                    let (l, r) = if legacy_conhost() { ("<", ">") } else { ("◂", "▸") };
                    let lrect = Rect { x: content.x + lw, y, width: 1, height: 1 };
                    let lhover = gui.ui.pointer.is_some_and(|p| lrect.contains(p));
                    put(frame, lrect.x, y, l, if lhover { bright_bold() } else { dim() });
                    let nx = lrect.x + 2;
                    let shown = super::bar::clip(&name, content.width.saturating_sub(lw + 6) as usize);
                    let nrect = Rect { x: nx, y, width: shown.chars().count() as u16, height: 1 };
                    let nhover = gui.ui.pointer.is_some_and(|p| nrect.contains(p));
                    put(frame, nx, y, &shown, if nhover { bright_bold() } else { Style::default().add_modifier(Modifier::BOLD) });
                    let rrect = Rect { x: nrect.right() + 1, y, width: 1, height: 1 };
                    let rhover = gui.ui.pointer.is_some_and(|p| rrect.contains(p));
                    put(frame, rrect.x, y, r, if rhover { bright_bold() } else { dim() });
                    gui.ui.click(rect, Act::TorRow(Row::Library));
                    gui.ui.click(lrect, Act::TorLib(-1));
                    gui.ui.click(nrect, Act::TorLib(1));
                    gui.ui.click(rrect, Act::TorLib(1));
                }
            }
            Row::File => {
                let rect = Rect { x: content.x, y, width: content.width, height: 1 };
                let focused = gui.torrent.cursor == Some(Row::File);
                let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
                put(frame, content.x, y, &t!("gui.tor.source"), label_style(focused, hover));
                gui.ui.click(rect, Act::TorRow(Row::File));
                match gui.torrent.file.clone() {
                    None => {
                        // The one way in for a file: a text button, the
                        // accent so the empty form has a lead.
                        let picking = gui.torrent.busy == Some(Busy::Picking);
                        let mut x = content.x + lw;
                        if picking {
                            let label = t!("gui.tor.picking").to_string();
                            put(frame, x, y, &label, accent());
                            x += label.chars().count() as u16;
                        } else {
                            let label = format!("{} {forward}", t!("gui.tor.choose_file"));
                            let w = text_button(frame, gui, x, y, &label, true, Act::TorPick);
                            gui.ui.tip(Rect { x, y, width: w, height: 1 }, format!("{} — b", t!("gui.tor.choose_file")));
                            x += w;
                        }
                        // The typed road beside the dialog: paths get
                        // pasted and dropped in a terminal, and SSH has
                        // no dialog at all.
                        put(frame, x, y, " · ", dim());
                        x += 3;
                        let typed = t!("gui.tor.type_path").to_string();
                        let w = text_button(frame, gui, x, y, &typed, false, Act::TorType);
                        gui.ui.tip(Rect { x, y, width: w, height: 1 }, format!("{typed} — t"));
                    }
                    Some(file) => {
                        // The chip (clause 5): the name, its [X], and on
                        // the line beneath the hand-off and auto-detect.
                        let name_w = content.width.saturating_sub(lw + 5) as usize;
                        let shown = super::bar::clip(&file.name, name_w);
                        let nrect = Rect { x: content.x + lw, y, width: shown.chars().count() as u16, height: 1 };
                        let nhover = gui.ui.pointer.is_some_and(|p| nrect.contains(p));
                        put(frame, nrect.x, y, &shown, if nhover { bright_bold() } else { Style::default().add_modifier(Modifier::BOLD) });
                        gui.ui.click(nrect, Act::TorPick);
                        let xrect = Rect { x: nrect.right() + 1, y, width: 3, height: 1 };
                        let xhover = gui.ui.pointer.is_some_and(|p| xrect.contains(p));
                        let xstyle = if xhover {
                            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
                        } else {
                            dim()
                        };
                        put(frame, xrect.x, y, "[X]", xstyle);
                        gui.ui.click(xrect, Act::TorUnload);
                        gui.ui.tip(xrect, t!("gui.tor.unload_tip").to_string());

                        y += 1;
                        let mut x = content.x + lw;
                        let busy = gui.torrent.busy;
                        let open = t!("gui.tor.open_with").to_string();
                        if busy == Some(Busy::HandingOff) {
                            put(frame, x, y, &open, accent());
                            x += open.chars().count() as u16;
                        } else {
                            let w = text_button(frame, gui, x, y, &open, false, Act::TorHandOff);
                            gui.ui.tip(Rect { x, y, width: w, height: 1 }, format!("{open} — o"));
                            x += w;
                        }
                        put(frame, x, y, " · ", dim());
                        x += 3;
                        if busy == Some(Busy::Detecting) {
                            put(frame, x, y, &t!("gui.tor.detecting"), accent());
                        } else {
                            let detect = t!("gui.tor.auto_detect").to_string();
                            let w = text_button(frame, gui, x, y, &detect, false, Act::TorDetect);
                            gui.ui.tip(Rect { x, y, width: w, height: 1 }, format!("{detect} — d"));
                        }
                    }
                }
            }
            Row::Magnet => {
                let value = gui.torrent.magnet.value().trim().to_string();
                let invalid = !value.is_empty() && !meta::is_valid_magnet(&value);
                let trailing = invalid.then(|| (t!("gui.tor.magnet_invalid").to_string(), Style::default().fg(th().gold)));
                draw_text_row(frame, gui, content, y, Row::Magnet, &t!("gui.tor.magnet"), Some("magnet:?xt=urn:btih:…"), trailing);
            }
            Row::Artist => draw_text_row(frame, gui, content, y, Row::Artist, &t!("gui.tor.artist"), None, None),
            Row::Album => draw_text_row(frame, gui, content, y, Row::Album, &t!("gui.tor.album"), None, None),
            Row::Year => draw_text_row(frame, gui, content, y, Row::Year, &t!("gui.tor.year"), None, None),
            Row::Path => {
                draw_text_row(frame, gui, content, y, Row::Path, &t!("gui.tor.path"), Some("Artist/Album"), None);
                gui.ui.tip(Rect { x: content.x, y, width: lw, height: 1 }, t!("gui.tor.path_tip").to_string());
                // The preview line (clause 14): the real landing spot.
                y += 1;
                let path = gui.torrent.path.value().trim().trim_end_matches('/').to_string();
                let preview = match library(gui) {
                    Some(vpath) => format!("/{vpath}/{path}/{}", t!("gui.tor.preview_contents")),
                    None => t!("gui.tor.preview_no_library", path = path).to_string(),
                };
                put(frame, content.x + lw, y, &super::clip_lead(&preview, content.width.saturating_sub(lw) as usize), dim());
            }
            Row::Rename | Row::Force => {
                // Both toggles share one line: the second sits after the
                // first, each its own target, each carrying its
                // description as its tip (clause 15).
                let (check_on, check_off) = if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") };
                let on = if row == Row::Rename { gui.torrent.rename_root } else { gui.torrent.force_fresh };
                let (label, desc, act) = if row == Row::Rename {
                    (t!("gui.tor.rename_root"), t!("gui.tor.rename_root_desc"), Act::TorToggleRename)
                } else {
                    (t!("gui.tor.force_fresh"), t!("gui.tor.force_fresh_desc"), Act::TorToggleForce)
                };
                let text = format!("{} {label}", if on { check_on } else { check_off });
                let x = if row == Row::Rename {
                    content.x
                } else {
                    let first = format!("{} {}", check_on, t!("gui.tor.rename_root"));
                    content.x + first.chars().count() as u16 + 3
                };
                let rect = Rect { x, y, width: text.chars().count() as u16, height: 1 };
                let focused = gui.torrent.cursor == Some(row);
                let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
                let glyph_style = if on { Style::default().fg(th().ok) } else { dim() };
                let label_style = match (focused, hover) {
                    (true, _) => accent().add_modifier(Modifier::BOLD),
                    (false, true) => bright_bold(),
                    (false, false) => Style::default(),
                };
                if rect.right() <= content.right() {
                    put(frame, x, y, if on { check_on } else { check_off }, glyph_style);
                    put(frame, x + 4, y, &label, label_style);
                    gui.ui.click(rect, act);
                    gui.ui.tip(rect, desc.to_string());
                }
                // Rename and Force share the row; only the last advances.
                if row == Row::Rename && gui.torrent.file.is_some() {
                    continue;
                }
            }
            Row::Submit => {
                // The kit's one primary, bottom-right — disabled still
                // tips WHY, the one exception to disabled inertness.
                let ready = gui.torrent.gate.ok() && gui.torrent.has_source() && gui.torrent.busy.is_none() && library(gui).is_some();
                let label = match gui.torrent.busy {
                    Some(Busy::Adding | Busy::Checking) => t!("gui.tor.submitting").to_string(),
                    _ if ready => format!("{} {forward}", t!("gui.tor.submit")),
                    _ => format!("{}  ", t!("gui.tor.submit")),
                };
                let width = label.chars().count() as u16 + 6;
                let at = Rect {
                    x: content.right().saturating_sub(width),
                    y: content.bottom().saturating_sub(3).max(y),
                    width,
                    height: 3,
                };
                let rect = tall_button(frame, &mut gui.ui, at, &label, ready, Act::TorSubmit);
                if !ready {
                    let why = match &gui.torrent.gate {
                        Gate::Answered(p) if !p.ok() => p.reason.clone().unwrap_or_else(|| t!("gui.tor.unavailable").to_string()),
                        Gate::Failed(_) | Gate::NotAsked | Gate::Pending if !gui.torrent.gate.ok() => t!("gui.tor.unavailable").to_string(),
                        _ if library(gui).is_none() => t!("gui.tor.pick_library").to_string(),
                        _ if gui.torrent.busy.is_some() => t!("gui.tor.submitting").to_string(),
                        _ => t!("gui.tor.hint").to_string(),
                    };
                    gui.ui.tip(rect, why);
                }
                if gui.torrent.cursor == Some(Row::Submit) {
                    frame.render_widget(
                        ratatui::widgets::Block::default()
                            .borders(ratatui::widgets::Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(if ready { accent() } else { dim().add_modifier(Modifier::BOLD) }),
                        rect,
                    );
                }
            }
        }
        y += 1;
        if y >= content.bottom() {
            break;
        }
    }
}

// ── Modals ──────────────────────────────────────────────────────────────────

/// Greedy word wrap for the modal sentences.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let need = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
        if need > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A modal's row: the cursor's selection bg, hover bright, an optional
/// dim detail at the right edge.
#[allow(clippy::too_many_arguments)]
fn modal_row(frame: &mut Frame, gui: &mut Gui, inner: Rect, y: u16, label: &str, detail: Option<&str>, selected: bool, lead: bool, act: Act) {
    let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
    let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
    if selected {
        frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
    }
    let style = match (selected, hover, lead) {
        (true, _, _) => sel().add_modifier(Modifier::BOLD),
        (false, true, _) => bright_bold(),
        (false, false, true) => accent().add_modifier(Modifier::BOLD),
        (false, false, false) => Style::default(),
    };
    let detail_w = detail.map_or(0, |d| d.chars().count() + 2);
    put(frame, inner.x + 1, y, &super::bar::clip(label, (inner.width as usize).saturating_sub(2 + detail_w)), style);
    if let Some(detail) = detail {
        let dstyle = if selected { sel() } else { dim() };
        put(frame, inner.right().saturating_sub(detail.chars().count() as u16 + 1), y, detail, dstyle);
    }
    gui.ui.click(rect, act);
}

/// The chooser, the picker and the match picker — drawn (and registered)
/// after the base screen so their rects win the pointer; a whole-screen
/// guard makes the room beneath inert.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if let Some(chooser) = gui.torrent.chooser.clone() {
        gui.ui.click(area, Act::TorChooseClose);
        let inner = modal_frame(frame, area, 60, 10, th().accent);
        put(frame, inner.x + 1, inner.y, &t!("gui.tor.received_title"), accent().add_modifier(Modifier::BOLD));
        modal_close(frame, &mut gui.ui, inner, Act::TorChooseClose, t!("gui.srv.close_tip").to_string());
        let what = match &chooser.incoming {
            Incoming::File(f) => f.name.clone(),
            Incoming::Magnet(link) => meta::magnet_display_name(link).unwrap_or_else(|| t!("gui.tor.magnet_link").to_string()),
        };
        put(frame, inner.x + 1, inner.y + 1, &super::bar::clip(&what, inner.width as usize - 2), dim());
        for (i, line) in wrap(&t!("gui.tor.received_body"), inner.width as usize - 2).into_iter().take(2).enumerate() {
            put(frame, inner.x + 1, inner.y + 2 + i as u16, &line, Style::default());
        }
        let (check_on, check_off) = if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") };
        let ask = format!("{} {}", if chooser.dont_ask { check_on } else { check_off }, t!("gui.tor.received_dont_ask"));
        let rows: [(String, Act); 3] = [
            (t!("gui.tor.received_add").to_string(), Act::TorChooseAdd),
            (t!("gui.tor.open_with").to_string(), Act::TorChooseHandOff),
            (ask, Act::TorChooseAsk),
        ];
        for (i, (label, act)) in rows.into_iter().enumerate() {
            modal_row(frame, gui, inner, inner.y + 5 + i as u16, &label, None, chooser.row == i, i == 0, act);
        }
    }

    if let Some(picker) = gui.torrent.picker.clone() {
        gui.ui.click(area, Act::TorPickerClose);
        let suggestions = picker.suggestions();
        let shown = suggestions.len().min(6) as u16;
        // Anchored as if always full: the title and input hold one spot
        // and the suggestion list grows DOWNWARD beneath them.
        let inner = modal_frame_anchored(frame, area, 66, 7 + shown, 13, th().accent);
        put(frame, inner.x, inner.y, &t!("gui.tor.picker_title"), accent().add_modifier(Modifier::BOLD));
        modal_close(frame, &mut gui.ui, inner, Act::TorPickerClose, t!("gui.srv.close_tip").to_string());
        put(
            frame,
            inner.x,
            inner.y + 2,
            &input_display(picker.text.value(), picker.text.cursor(), inner.width),
            Style::default(),
        );
        let sel_moved = picker.sel != picker.sel_anchor;
        let reveal = if sel_moved { picker.sel } else { None };
        let (first, visible) = table_view(suggestions.len(), reveal, picker.scroll, 6);
        if let Some(p) = gui.torrent.picker.as_mut() {
            p.scroll = first;
            p.sel_anchor = p.sel;
        }
        let overflow = suggestions.len() > visible;
        let row_width = if overflow { inner.width.saturating_sub(1) } else { inner.width };
        let forward = if legacy_conhost() { ">" } else { "▸" };
        for (row, i) in (first..first + visible).enumerate() {
            let entry = &picker.entries[suggestions[i]];
            let selected = picker.sel == Some(i);
            let rect = Rect { x: inner.x, y: inner.y + 4 + row as u16, width: row_width, height: 1 };
            let hovered = gui.ui.pointer.is_some_and(|p| rect.contains(p));
            let style = match (selected, hovered, entry.dir) {
                (true, _, _) => sel(),
                (false, true, _) => Style::default().fg(th().bright),
                (false, false, true) => dim(),
                (false, false, false) => Style::default(),
            };
            let text = if entry.dir {
                format!("{forward} {}{}", entry.name, sep_for(&picker.listed_for))
            } else {
                format!("  {}", entry.name)
            };
            put(frame, rect.x, rect.y, &super::bar::clip(&text, row_width as usize), style);
            gui.ui.click(rect, Act::TorPickerSuggest(i));
        }
        scroll_list(
            frame,
            &mut gui.ui,
            Rect { x: inner.x + inner.width.saturating_sub(1), y: inner.y + 4, width: 1, height: visible as u16 },
            suggestions.len(),
            visible,
            first,
            Act::TorPickerScrollBy(-1),
            Act::TorPickerScrollBy(1),
            Act::TorPickerScrollTo,
        );
        if let Some(err) = &picker.error {
            put(frame, inner.x, inner.y + 4, &super::bar::clip(err, inner.width as usize), Style::default().fg(th().gold));
        } else if suggestions.is_empty() && !picker.listed_for.is_empty() {
            put(frame, inner.x, inner.y + 4, &t!("gui.tor.picker_empty"), dim());
        }
    }

    if let Some(matches) = gui.torrent.matches.clone() {
        gui.ui.click(area, Act::TorMatchClose);
        let n = matches.list.len().min(8) as u16;
        let inner = modal_frame(frame, area, 72, 7 + n, th().accent);
        put(frame, inner.x + 1, inner.y, &t!("gui.tor.partial_title"), accent().add_modifier(Modifier::BOLD));
        modal_close(frame, &mut gui.ui, inner, Act::TorMatchClose, t!("gui.srv.close_tip").to_string());
        for (i, line) in wrap(&t!("gui.tor.partial_body"), inner.width as usize - 2).into_iter().take(2).enumerate() {
            put(frame, inner.x + 1, inner.y + 1 + i as u16, &line, dim());
        }
        let top = inner.y + 4;
        for (i, m) in matches.list.iter().take(n as usize).enumerate() {
            let label = format!("{}/{}", m.vpath, m.relative_path);
            let count = t!(
                "gui.tor.partial_count",
                matched = m.matched.map_or("?".to_string(), |v| v.to_string()),
                total = m.total.map_or("?".to_string(), |v| v.to_string())
            )
            .to_string();
            let detail = match m.missing {
                Some(missing) => format!("{count}{}", t!("gui.tor.partial_missing", missing = missing)),
                None => count,
            };
            modal_row(frame, gui, inner, top + i as u16, &label, Some(&detail), matches.row == i, false, Act::TorMatch(i));
        }
        let fresh_row = matches.list.len();
        modal_row(
            frame,
            gui,
            inner,
            top + n + 1,
            &t!("gui.tor.download_fresh"),
            None,
            matches.row == fresh_row,
            true,
            Act::TorMatchFresh,
        );
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The ask-me switch (clause 51), persisted the servers room's way and
/// mirrored in memory whatever the disk said.
pub(crate) fn set_ask(gui: &mut Gui, ask: bool) {
    super::servers::update_config(gui, |config| config.torrent.ask = ask);
    gui.config.torrent.ask = ask;
}

fn step_library(gui: &mut Gui, delta: i32) {
    let n = gui.app.libraries.len();
    if n == 0 {
        return;
    }
    gui.torrent.vpath = (gui.torrent.vpath as i32 + delta).rem_euclid(n as i32) as usize;
    recompute_path(gui);
}

/// Move the keyboard cursor along the rows that exist right now.
fn move_cursor(gui: &mut Gui, delta: i32) {
    let rows = gui.torrent.rows(gui.app.libraries.len());
    if rows.is_empty() {
        gui.torrent.cursor = None;
        return;
    }
    let at = gui.torrent.cursor.and_then(|c| rows.iter().position(|r| *r == c));
    let next = match at {
        Some(i) => (i as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize,
        None if delta < 0 => rows.len() - 1,
        None => 0,
    };
    gui.torrent.cursor = Some(rows[next]);
}

/// Handle the room's actions. Returns true when the act was this room's.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act.clone() {
        Act::TorBack => {
            gui.torrent.room = false;
            gui.torrent.cursor = None;
        }
        Act::TorRow(row) => gui.torrent.cursor = Some(row),
        Act::TorLib(delta) => step_library(gui, delta),
        Act::TorPick => native_pick(gui),
        Act::TorType => open_picker(gui),
        Act::TorUnload => {
            gui.torrent.file = None;
            gui.torrent.force_fresh = false;
            if gui.torrent.cursor == Some(Row::Force) {
                gui.torrent.cursor = Some(Row::Rename);
            }
        }
        Act::TorHandOff => hand_off(gui, None),
        Act::TorDetect => auto_detect(gui),
        Act::TorToggleRename => gui.torrent.rename_root = !gui.torrent.rename_root,
        Act::TorToggleForce => gui.torrent.force_fresh = !gui.torrent.force_fresh,
        Act::TorSubmit => submit(gui),
        Act::TorPickerClose => gui.torrent.picker = None,
        Act::TorPickerSuggest(i) => picker_accept(gui, i),
        Act::TorPickerScrollBy(delta) => {
            if let Some(picker) = gui.torrent.picker.as_mut() {
                picker.scroll = if delta < 0 { picker.scroll.saturating_sub(1) } else { picker.scroll + 1 };
            }
        }
        Act::TorPickerScrollTo(first) => {
            if let Some(picker) = gui.torrent.picker.as_mut() {
                picker.scroll = first;
            }
        }
        Act::TorMatch(i) => use_match(gui, i),
        Act::TorMatchFresh => {
            gui.torrent.matches = None;
            add_at_typed_path(gui);
        }
        Act::TorMatchClose => gui.torrent.matches = None,
        Act::TorChooseAdd => {
            if let Some(chooser) = gui.torrent.chooser.take() {
                if chooser.dont_ask {
                    set_ask(gui, false);
                }
                open_room_with(gui, chooser.incoming);
            }
        }
        Act::TorChooseHandOff => {
            if let Some(chooser) = gui.torrent.chooser.take() {
                hand_off(gui, Some(chooser.incoming));
            }
        }
        Act::TorChooseAsk => {
            if let Some(chooser) = gui.torrent.chooser.as_mut() {
                chooser.dont_ask = !chooser.dont_ask;
                chooser.row = 2;
            }
        }
        Act::TorChooseClose => gui.torrent.chooser = None,
        _ => return false,
    }
    true
}

/// The room's wheel: the picker's suggestions are the one list here.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if gui.torrent.picker.is_some() {
        gui.act(Act::TorPickerScrollBy(delta));
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The room's keys, and its modals' (which outrank everything — the
/// chooser can be up over any room). Returns `Some(quit)` when the key
/// was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if let Some(chooser) = gui.torrent.chooser.as_mut() {
        match key.code {
            KeyCode::Up => chooser.row = chooser.row.saturating_sub(1),
            KeyCode::Down => chooser.row = (chooser.row + 1).min(2),
            KeyCode::Char(' ') => return Some(gui.act(Act::TorChooseAsk)),
            KeyCode::Enter => {
                let act = match chooser.row {
                    0 => Act::TorChooseAdd,
                    1 => Act::TorChooseHandOff,
                    _ => Act::TorChooseAsk,
                };
                return Some(gui.act(act));
            }
            KeyCode::Esc => gui.torrent.chooser = None,
            _ => {}
        }
        return Some(false);
    }
    if let Some(picker) = gui.torrent.picker.as_mut() {
        let at_end = picker.text.cursor() == picker.text.value().chars().count();
        match key.code {
            KeyCode::Esc => gui.torrent.picker = None,
            KeyCode::Enter => picker_enter(gui),
            KeyCode::Down => {
                let n = picker.suggestions().len();
                if n > 0 {
                    picker.sel = Some(picker.sel.map_or(0, |i| (i + 1) % n));
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                let n = picker.suggestions().len();
                if n > 0 {
                    picker.sel = Some(picker.sel.map_or(n - 1, |i| (i + n - 1) % n));
                }
            }
            KeyCode::Tab => picker_complete(gui),
            // Right completes only from the END of the line (fish
            // behavior) — anywhere else it is the editor's cursor key.
            KeyCode::Right if at_end => picker_complete(gui),
            _ => {
                let changed = picker
                    .text
                    .handle_event(&TermEvent::Key(key))
                    .is_some_and(|change| change.value);
                if changed {
                    picker_refresh(gui);
                }
            }
        }
        return Some(false);
    }
    if let Some(matches) = gui.torrent.matches.as_mut() {
        let last = matches.list.len();
        match key.code {
            KeyCode::Up => matches.row = matches.row.saturating_sub(1),
            KeyCode::Down => matches.row = (matches.row + 1).min(last),
            KeyCode::Enter => {
                let act = if matches.row < last { Act::TorMatch(matches.row) } else { Act::TorMatchFresh };
                return Some(gui.act(act));
            }
            KeyCode::Esc => gui.torrent.matches = None,
            _ => {}
        }
        return Some(false);
    }
    if !(gui.torrent.room && gui.active == super::SETTINGS_NAV) {
        return None;
    }

    match gui.torrent.cursor {
        Some(row) if row.is_text() => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab | KeyCode::Enter => move_cursor(gui, 1),
            KeyCode::Esc => gui.torrent.cursor = None,
            _ => {
                // The line editor owns the rest: chars, Backspace, Delete,
                // ←/→, Home/End, the ctrl word ops.
                let changed = gui
                    .torrent
                    .input_mut(row)
                    .and_then(|input| input.handle_event(&TermEvent::Key(key)))
                    .is_some_and(|change| change.value);
                if changed {
                    text_changed(gui, row);
                }
            }
        },
        Some(Row::File) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.torrent.cursor = None,
            KeyCode::Enter | KeyCode::Char('b') => return Some(gui.act(Act::TorPick)),
            KeyCode::Char('t') => return Some(gui.act(Act::TorType)),
            KeyCode::Char('x') if gui.torrent.file.is_some() => return Some(gui.act(Act::TorUnload)),
            KeyCode::Char('o') if gui.torrent.file.is_some() => return Some(gui.act(Act::TorHandOff)),
            KeyCode::Char('d') if gui.torrent.file.is_some() => return Some(gui.act(Act::TorDetect)),
            _ => return None,
        },
        Some(Row::Library) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.torrent.cursor = None,
            KeyCode::Left => return Some(gui.act(Act::TorLib(-1))),
            KeyCode::Right | KeyCode::Enter => return Some(gui.act(Act::TorLib(1))),
            _ => return None,
        },
        Some(row @ (Row::Rename | Row::Force)) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.torrent.cursor = None,
            KeyCode::Char(' ') | KeyCode::Enter => {
                let act = if row == Row::Rename { Act::TorToggleRename } else { Act::TorToggleForce };
                return Some(gui.act(act));
            }
            _ => return None,
        },
        Some(Row::Submit) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.torrent.cursor = None,
            KeyCode::Char(' ') | KeyCode::Enter => return Some(gui.act(Act::TorSubmit)),
            _ => return None,
        },
        Some(_) | None => match key.code {
            KeyCode::Down => move_cursor(gui, 1),
            KeyCode::Up => move_cursor(gui, -1),
            KeyCode::Char('b') => return Some(gui.act(Act::TorPick)),
            KeyCode::Char('t') => return Some(gui.act(Act::TorType)),
            KeyCode::Esc => return Some(gui.act(Act::TorBack)),
            _ => return None,
        },
    }
    Some(false)
}

/// The tips line for this room's states.
pub(crate) fn tips(gui: &Gui) -> String {
    if gui.torrent.chooser.is_some() {
        return t!("gui.tips.tor_chooser").to_string();
    }
    if gui.torrent.picker.is_some() {
        return t!("gui.tips.tor_picker").to_string();
    }
    if gui.torrent.matches.is_some() {
        return t!("gui.tips.tor_matches").to_string();
    }
    match gui.torrent.cursor {
        None => t!("gui.tips.tor_room"),
        Some(row) if row.is_text() => t!("gui.tips.tor_text"),
        Some(Row::File) => t!("gui.tips.tor_file"),
        Some(Row::Library) => t!("gui.tips.tor_library"),
        Some(Row::Rename | Row::Force) => t!("gui.tips.tor_toggle"),
        Some(_) => t!("gui.tips.tor_submit"),
    }
    .to_string()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::App;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Vela+-+Cassini+(2020)";

    /// A minimal single-file torrent, bencoded by hand.
    fn torrent(name: &str) -> Vec<u8> {
        let mut bytes = b"d8:announce18:http://t.example/4:infod6:lengthi1234e4:name".to_vec();
        bytes.extend(format!("{}:{name}", name.len()).into_bytes());
        bytes.extend(b"12:piece lengthi16384e6:pieces20:");
        bytes.extend([0u8; 20]);
        bytes.extend(b"ee");
        bytes
    }

    fn gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.queue_open = false;
        gui
    }

    /// A connected Gui in the room, the gate already answered — seated
    /// BEFORE the room opens, so nothing is asked of a network.
    fn ready_gui(libraries: &[&str]) -> Gui {
        let mut gui = gui();
        gui.app.connected = true;
        gui.app.libraries = libraries.iter().map(|l| l.to_string()).collect();
        gui.app.session.server = "http://host:3000".into();
        gui.torrent.gate = Gate::Answered(TorrentPreflight {
            active: true,
            user_allowed: true,
            display_name: Some("Transmission".into()),
            ..Default::default()
        });
        gui.torrent.gate_for = "http://host:3000".into();
        open_room(&mut gui);
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

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn type_text(gui: &mut Gui, text: &str) {
        for c in text.chars() {
            key(gui, KeyCode::Char(c));
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mstream-torrent-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_settings_doorway_opens_the_room_and_esc_walks_back() {
        let mut gui = gui();
        gui.active = super::super::SETTINGS_NAV;
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("TORRENTS"), "the settings grew a group:\n{text}");
        assert!(text.contains("Add a torrent"), "got:\n{text}");
        assert!(text.contains("[✓] Ask what to do with torrents"), "ask is on by default:\n{text}");

        gui.act(Act::Row(super::super::ROW_TORRENT));
        assert!(gui.torrent.room, "the doorway opens the room");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Add torrent"), "got:\n{text}");
        assert!(text.contains("Choose a .torrent file"), "the way in for a file:\n{text}");
        assert!(!text.contains("ARTIST"), "nothing below the source yet:\n{text}");
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.torrent.room, "Esc walks back to Settings");

        // The ask row flips the persisted choice.
        gui.act(Act::Row(super::super::ROW_ASK));
        assert!(!gui.config.torrent.ask);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("[ ] Ask what to do with torrents"), "got:\n{text}");
    }

    #[test]
    fn the_form_stays_hidden_until_a_source_is_real() {
        let mut gui = ready_gui(&["music"]);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Transmission · ready"), "the gate's standing:\n{text}");
        assert!(!text.contains("◂ music ▸"), "a lone library is not a choice:\n{text}");

        // ↓ lands on the file row (no library row), ↓ again on the magnet.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::File));
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::Magnet));
        type_text(&mut gui, "magnet:?xt=urn:btih:abc");
        assert!(!gui.torrent.has_source(), "a half-typed link is not a source");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Not a valid magnet link"), "marked in place (clause 4):\n{text}");
        assert!(!text.contains("ARTIST"), "still hidden:\n{text}");

        // Finish it: the form reveals, pre-filled from the dn.
        for _ in 0.."abc".len() {
            key(&mut gui, KeyCode::Backspace);
        }
        type_text(&mut gui, &MAGNET["magnet:?xt=urn:btih:".len()..]);
        assert!(gui.torrent.has_source());
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert_eq!(gui.torrent.album.value(), "Cassini");
        assert_eq!(gui.torrent.year.value(), "2020");
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini", "the legacy layout with no template");
        let text = draw(&mut gui).join("\n");
        for needed in ["ARTIST", "ALBUM", "YEAR", "PATH", "/music/Vela/Cassini/‹torrent contents›", "Rename the torrent's root folder", "Add torrent ▸"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        assert!(!text.contains("Force fresh download"), "a magnet has no files to check:\n{text}");
        assert!(!text.contains("Not a valid magnet link"), "the mark clears:\n{text}");

        // A template on the library re-shapes the path.
        gui.torrent.templates.insert("music".into(), "{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into());
        recompute_path(&mut gui);
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini (2020)");
    }

    #[test]
    fn a_loaded_file_names_itself_and_takes_the_magnets_place() {
        let mut gui = ready_gui(&["music"]);
        gui.torrent.magnet = Input::new("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into());
        set_file(&mut gui, LoadedFile { name: "vela.torrent".into(), bytes: torrent("Vela - Cassini (2020)") });
        assert!(gui.torrent.magnet.value().is_empty(), "exactly one source (clause 1)");
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert_eq!(gui.torrent.year.value(), "2020");
        let text = draw(&mut gui).join("\n");
        for needed in ["vela.torrent", "[X]", "Open in another app", "Auto-detect metadata", "Force fresh download"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        assert!(!text.contains("MAGNET"), "the chip takes the magnet's spot (clause 5):\n{text}");

        // [X] lets go of the file; the magnet row returns.
        gui.act(Act::TorUnload);
        assert!(gui.torrent.file.is_none());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("MAGNET"), "got:\n{text}");
    }

    #[test]
    fn hand_edits_to_the_path_stick_until_a_new_source() {
        let mut gui = ready_gui(&["music"]);
        set_file(&mut gui, LoadedFile { name: "vela.torrent".into(), bytes: torrent("Vela - Cassini (2020)") });
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini");
        gui.torrent.cursor = Some(Row::Path);
        type_text(&mut gui, "/Deluxe");
        assert!(gui.torrent.path_edited);
        gui.torrent.cursor = Some(Row::Artist);
        type_text(&mut gui, "s");
        assert_eq!(gui.torrent.artist.value(), "Velas");
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini/Deluxe", "sticky (clause 13)");
        set_file(&mut gui, LoadedFile { name: "nadir.torrent".into(), bytes: torrent("Nadir - Aphelion (2019)") });
        assert_eq!(gui.torrent.path.value(), "Nadir/Aphelion", "a fresh source resets the stickiness");
    }

    #[test]
    fn submit_reads_the_form_and_refuses_in_words() {
        let mut gui = ready_gui(&[]);
        gui.torrent.magnet = Input::new(MAGNET.into());
        assert_eq!(plan_submit(&gui).unwrap_err(), "Pick a library");

        let mut gui = ready_gui(&["music"]);
        assert_eq!(plan_submit(&gui).unwrap_err(), "Add a magnet link or a .torrent file (one)");
        gui.torrent.magnet = Input::new("magnet:?dn=only".into());
        assert_eq!(plan_submit(&gui).unwrap_err(), "Not a valid magnet link");
        gui.torrent.magnet = Input::new(MAGNET.into());
        magnet_changed(&mut gui);
        match plan_submit(&gui).unwrap() {
            Plan::Add(req) => {
                assert_eq!(req.vpath, "music");
                assert_eq!((req.sub_path.as_str(), req.directory_name.as_str()), ("Vela", "Cassini"));
                assert!(req.rename_root, "on by default (clause 15)");
                assert_eq!(req.source, TorrentSource::Magnet(MAGNET.into()));
            }
            other => panic!("a magnet adds outright: {other:?}"),
        }
        gui.torrent.path = Input::default();
        assert_eq!(plan_submit(&gui).unwrap_err(), "Destination path is empty");

        // A file checks the disk first — unless forced fresh (clause 20).
        set_file(&mut gui, LoadedFile { name: "vela.torrent".into(), bytes: torrent("Vela - Cassini (2020)") });
        assert!(matches!(plan_submit(&gui).unwrap(), Plan::Check(_)));
        gui.torrent.force_fresh = true;
        assert!(matches!(plan_submit(&gui).unwrap(), Plan::Add(TorrentAddRequest { source: TorrentSource::File { .. }, .. })));

        // A shut gate refuses silently: the banner already says why, and
        // the button is disabled with the reason on its tip.
        gui.torrent.gate = Gate::Answered(TorrentPreflight {
            active: false,
            user_allowed: true,
            reason: Some("No torrent client is selected".into()),
            ..Default::default()
        });
        submit(&mut gui);
        assert!(gui.torrent.busy.is_none());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("No torrent client is selected"), "the server's own words:\n{text}");
        assert!(!text.contains("Add torrent ▸"), "no arrow on a disabled primary:\n{text}");
    }

    #[test]
    fn the_seed_check_outcomes_are_worded_and_the_partial_match_asks() {
        let mut gui = ready_gui(&["music"]);
        let file = LoadedFile { name: "vela.torrent".into(), bytes: torrent("Vela - Cassini (2020)") };
        set_file(&mut gui, file.clone());
        apply_check(&mut gui, SeedCheck { outcome: "seeded".into(), ..Default::default() });
        assert_eq!(gui.note.as_ref().map(|n| n.0.as_str()), Some("Already on disk — seeding it now"));
        assert!(gui.torrent.file.is_none(), "done means done");

        set_file(&mut gui, file.clone());
        apply_check(&mut gui, SeedCheck { outcome: "match_unmapped".into(), vpath: Some("music".into()), ..Default::default() });
        let (words, is_err) = gui.note.clone().unwrap();
        assert!(words.contains("path mapping") && words.contains("music"), "{words}");
        assert!(is_err);
        apply_check(&mut gui, SeedCheck { outcome: "pad_files_missing".into(), vpath: Some("music".into()), client_type: Some("transmission".into()), ..Default::default() });
        let words = gui.note.clone().unwrap().0;
        assert!(words.contains("padding") && words.contains("transmission"), "{words}");
        apply_check(&mut gui, SeedCheck { outcome: "invalid_torrent".into(), error: Some("bad bencode".into()), ..Default::default() });
        assert_eq!(gui.note.as_ref().map(|n| n.0.as_str()), Some("bad bencode"));

        apply_check(
            &mut gui,
            SeedCheck {
                outcome: "partial_match".into(),
                matches: vec![SeedMatch {
                    vpath: "music".into(),
                    relative_path: "Vela/Cassini".into(),
                    matched: Some(8),
                    total: Some(10),
                    missing: Some(2),
                }],
                ..Default::default()
            },
        );
        assert!(gui.torrent.matches.is_some(), "the match picker (clause 21)");
        let text = draw(&mut gui).join("\n");
        for needed in ["Some files already exist", "music/Vela/Cassini", "8/10 files here · 2 to download", "Download fresh anyway"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.matches.as_ref().unwrap().row, 1, "the last row is download fresh");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.torrent.matches.is_none(), "Esc backs out with nothing sent");

        // A match without a folder name cannot be used (the record's words).
        gui.torrent.matches = Some(Matches {
            list: vec![SeedMatch { vpath: "music".into(), relative_path: String::new(), ..Default::default() }],
            row: 0,
        });
        use_match(&mut gui, 0);
        assert!(gui.note.as_ref().unwrap().0.contains("Download fresh"));
    }

    #[test]
    fn the_arrival_chooser_asks_and_the_dont_ask_box_persists() {
        let mut gui = gui();
        arrive(&mut gui, MAGNET);
        assert!(matches!(gui.torrent.chooser, Some(Chooser { incoming: Incoming::Magnet(_), .. })));
        assert!(!gui.torrent.room, "nothing opens until the answer");
        let text = draw(&mut gui).join("\n");
        for needed in ["Torrent received", "Add to mStream", "Open in another app", "don't ask again", "Vela - Cassini (2020)"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.torrent.chooser.as_ref().unwrap().dont_ask);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.torrent.chooser.is_none());
        assert!(gui.torrent.room && gui.active == super::super::SETTINGS_NAV, "Add opens the room");
        assert_eq!(gui.torrent.magnet.value(), MAGNET);
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert!(!gui.config.torrent.ask, "the box flipped the setting (clause 51)");

        // Esc drops a torrent nobody asked for.
        arrive(&mut gui, MAGNET);
        assert!(gui.torrent.chooser.is_none(), "ask is off now: straight in");
    }

    #[test]
    fn a_torrent_arriving_with_ask_off_goes_straight_to_the_room() {
        let dir = scratch("arrive");
        let path = dir.join("vela.torrent");
        std::fs::write(&path, torrent("Vela - Cassini (2020)")).unwrap();
        let mut gui = gui();
        gui.config.torrent.ask = false;
        arrive(&mut gui, &path.to_string_lossy());
        assert!(gui.torrent.chooser.is_none());
        assert!(gui.torrent.room);
        assert_eq!(gui.torrent.file.as_ref().map(|f| f.name.as_str()), Some("vela.torrent"));
        assert_eq!(gui.torrent.album.value(), "Cassini");

        // The structural gate names a mistaken file (clause 3).
        let junk = dir.join("song.mp3");
        std::fs::write(&junk, b"ID3\x03not a torrent").unwrap();
        arrive(&mut gui, &junk.to_string_lossy());
        assert_eq!(gui.note.as_ref().map(|n| n.0.as_str()), Some("“song.mp3” is not a .torrent file"));
        arrive(&mut gui, "magnet:?dn=nothing");
        assert_eq!(gui.note.as_ref().map(|n| n.0.as_str()), Some("Not a valid magnet link"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bounced_hand_off_is_named_not_looped() {
        let dir = handoff_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("bounce-{}.torrent", std::process::id()));
        std::fs::write(&path, torrent("Vela - Cassini (2020)")).unwrap();
        let mut gui = gui();
        assert!(gui.config.torrent.ask, "asking is on");
        arrive(&mut gui, &path.to_string_lossy());
        assert!(gui.torrent.chooser.is_none(), "no chooser for our own file coming back");
        assert!(gui.torrent.room, "the torrent is still taken");
        let (words, is_err) = gui.note.clone().unwrap();
        assert!(words.contains("default app"), "{words}");
        assert!(is_err);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_picker_lists_folders_and_torrents_and_completes() {
        let dir = scratch("picker");
        std::fs::create_dir_all(dir.join("Albums")).unwrap();
        std::fs::write(dir.join("vela.torrent"), b"x").unwrap();
        std::fs::write(dir.join("Velvet.TORRENT"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let entries = list_dir(&dir.to_string_lossy()).unwrap();
        let names: Vec<(&str, bool)> = entries.iter().map(|e| (e.name.as_str(), e.dir)).collect();
        assert_eq!(names, vec![("Albums", true), ("vela.torrent", false), ("Velvet.TORRENT", false)], "folders first, then torrents, never the txt");

        let mut gui = ready_gui(&["music"]);
        let base = format!("{}/", dir.to_string_lossy());
        gui.torrent.picker = Some(Picker {
            text: Input::new(format!("{base}ve")),
            listed_for: base.clone(),
            entries,
            ..Picker::default()
        });
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Choose a .torrent file"), "got:\n{text}");
        assert!(text.contains("vela.torrent") && text.contains("Velvet.TORRENT"), "got:\n{text}");
        assert!(!text.contains("Albums/"), "the partial narrows the suggestions:\n{text}");

        key(&mut gui, KeyCode::Tab);
        assert_eq!(gui.torrent.picker.as_ref().unwrap().text.value(), format!("{base}vel"), "Tab extends to the common prefix");
        key(&mut gui, KeyCode::Tab);
        assert_eq!(gui.torrent.picker.as_ref().unwrap().sel, Some(0), "then starts cycling");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.torrent.picker.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_native_dialog_falls_back_to_the_typed_picker_and_t_is_its_own_road() {
        use crate::setup::picker::Pick;
        let mut gui = ready_gui(&["music"]);
        // No dialog could open: the room says so and the typed picker
        // takes over, starting where the dialog would have.
        gui.torrent.busy = Some(Busy::Picking);
        apply_reply(&mut gui, Reply::Picked(Pick::Unavailable("no session bus".into())));
        assert!(gui.torrent.busy.is_none());
        assert!(gui.torrent.picker.is_some(), "the fallback opened");
        let (words, is_err) = gui.note.clone().unwrap();
        assert!(words.contains("no session bus"), "{words}");
        assert!(!is_err, "a fallback is news, not a failure");
        key(&mut gui, KeyCode::Esc);

        // A declined dialog costs nothing.
        gui.torrent.busy = Some(Busy::Picking);
        note(&mut gui, "opening…", false);
        apply_reply(&mut gui, Reply::Picked(Pick::Cancelled));
        assert!(gui.torrent.busy.is_none() && gui.note.is_none() && gui.torrent.picker.is_none());

        // `t` is the typed road on purpose, dialog or not.
        key(&mut gui, KeyCode::Char('t'));
        assert!(gui.torrent.picker.is_some());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Choose a .torrent file"), "got:\n{text}");
        key(&mut gui, KeyCode::Esc);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("type a path"), "the typed road is named beside the dialog:\n{text}");
    }

    #[test]
    fn dropped_paths_and_tildes_read_as_paths() {
        assert_eq!(unescape_dropped("/Users/me/My\\ Music/x.torrent "), "/Users/me/My Music/x.torrent");
        assert_eq!(unescape_dropped("'/tmp/a b.torrent'"), "/tmp/a b.torrent");
        assert_eq!(unescape_dropped("\"/tmp/q.torrent\""), "/tmp/q.torrent");
        if let Some(home) = home() {
            assert!(expand_tilde("~/Downloads/x.torrent").starts_with(home.trim_end_matches('/')));
        }
        assert_eq!(expand_tilde("/abs/x.torrent"), "/abs/x.torrent");
    }

    #[test]
    fn the_cursor_walks_the_rows_that_exist() {
        let mut gui = ready_gui(&["music", "podcasts"]);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("◂ music ▸"), "two libraries make a choice:\n{text}");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::Library), "two libraries make a choice");
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.torrent.vpath, 1);
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.torrent.vpath, 0, "and it wraps");
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.cursor, Some(Row::Library), "the top clamps");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.torrent.cursor, None, "Esc stows");
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.cursor, Some(Row::Magnet), "↑ picks up at the bottom");
        set_file(&mut gui, LoadedFile { name: "v.torrent".into(), bytes: torrent("Vela - Cassini (2020)") });
        gui.torrent.cursor = None;
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.cursor, Some(Row::Submit), "with a source the bottom is Submit");
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.cursor, Some(Row::Force), "a file source has the force row");
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.torrent.force_fresh, "Space toggles");
        assert!(!tips(&gui).is_empty());
    }
}
