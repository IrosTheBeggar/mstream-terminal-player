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
//! the room is a **nav room** under TOOLS (the Auto DJ room's shape) plus
//! the `--torrent` command-line seam that stands in for the OS hand-off; a
//! second `--torrent` while one runs simply opens a second instance; the
//! webapp's fuller seed-check wordings are adopted; the hand-off is one
//! verb under the file's card; and the OS-defaults row waits for the
//! installers that will register the file types.
//!
//! Design: the "Add torrent · GUI" canvas (2026-11-10) — Add from
//! YouTube's shapes for a torrent. One block, centred, 56 cells at most.
//! The two ways in are two boxes with the record's "or" between them: a
//! tall frame that chooses a `.torrent`, and a link box with `Paste`
//! beside it where this build can read the clipboard. A loaded file is a
//! card (its name, how many files, how big) with its ways under it; a
//! link shows its own name. `Metadata` and `Destination` are rows that
//! each open one sheet; `Force fresh download` is a checkbox on the page;
//! the primary is tall under them, the block's width — a slab on the
//! title row where the room is too short for it. Under the form is what
//! this player sent, newest first. A refusal keeps the form and stands
//! under the primary in the server's own words, whole.
//!
//! The page shows words; the tips line names their keys. A letter acts
//! where its control is drawn ([`TorrentUi::drawn`]). A paste is taken
//! whole wherever the hand is ([`wants_paste`]): a magnet link fills the
//! link box, anything else is read as a path and loaded.
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
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::Client;
use crate::api::types::{
    SeedCheck, SeedMatch, TorrentAddRequest, TorrentAdded, TorrentDetect, TorrentDetectMeta,
    TorrentPreflight, TorrentSource,
};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, scroll_list, table_view, width};
use crate::tui::app::{Focus, Tab};
use crate::tui::worker::{ApiCmd, Event};

use super::keep::{self, crumb_sep};
use super::opener::{HandOff, open_target};
use super::torrent_meta::{self as meta, Confidence, TorrentMeta};
use super::ytdl_sheets::{bracket_button, field_card, open_sheet, open_sheet_anchored, sheet_text, sheet_title};
use super::{Act, Gui, Screen, accent, bright_bold, put, sel};

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

/// Where the hand can rest on the page, top to bottom. Which stops exist
/// depends on the state ([`TorrentUi::rows`]): with no source the two
/// boxes and the typed road; with one, its card or its link, its ways,
/// the two rows, the checkbox and the primary; then what was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// The file's box: `Choose a .torrent file…`, or the loaded file's card.
    File,
    /// `type a path ▸`, under the empty file box.
    TypePath,
    /// The link box.
    Magnet,
    /// The card's two ways.
    Detect,
    HandOff,
    Metadata,
    Destination,
    Force,
    Submit,
    /// A row of what was sent.
    Receipt(usize),
}

/// The Metadata sheet's stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetaStop {
    Artist,
    Album,
    Year,
    Detect,
    Done,
}

/// The Destination sheet's stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestStop {
    Library,
    Path,
    Rename,
    Done,
}

/// A sheet over the room, and where its hand is. Both edit the form as
/// they are used: `[ Done ]`, Enter and Esc all only close them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sheet {
    Metadata(MetaStop),
    Destination(DestStop),
}

/// The room's acts: its controls', its sheets' and its list's.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TorAct {
    /// A click that only moves the hand (the link box).
    Stop(Row),
    /// The native file dialog (falls back to the typed picker), the typed
    /// picker on purpose, the file let go, the link cleared, the
    /// clipboard read, the card's two ways.
    Pick,
    Type,
    Unload,
    ClearLink,
    PasteClip,
    HandOff,
    Detect,
    /// The two rows' sheets, each opened on a stop; the open one closed.
    OpenMetadata(MetaStop),
    OpenDestination(DestStop),
    SheetClose,
    /// The Destination sheet's library radios.
    Library(usize),
    ToggleRename,
    ToggleForce,
    Submit,
    /// The typed picker's controls.
    PickerClose,
    PickerSuggest(usize),
    PickerScrollBy(i32),
    PickerScrollTo(usize),
    /// Files already exist: the dot moved to a place (the last is the
    /// fresh download), the choice taken, or back.
    MatchPick(usize),
    MatchAdd,
    MatchClose,
    /// Torrent received: add here, hand it on, the don't-ask box.
    ChooseAdd,
    ChooseHandOff,
    ChooseAsk,
    ChooseClose,
    /// What was sent: a row's folder opened in Files, a row forgotten,
    /// the list scrolled.
    Receipt(usize),
    Forget(usize),
    ScrollBy(i32),
}

/// A room act as the hub carries it.
pub(super) fn tor(act: TorAct) -> Act {
    Act::Tor(act)
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
/// clause 51). Stop 0 adds, 1 hands off, 2 is the don't-ask box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chooser {
    pub incoming: Incoming,
    /// The hand: `None` until a walking key picks it up (Enter adds).
    pub row: Option<usize>,
    pub dont_ask: bool,
}

/// One entry of a listed folder: a sub-folder, or a `.torrent` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickEntry {
    pub name: String,
    /// The name lowercased once, for the match the picker runs every frame.
    lower: String,
    pub dir: bool,
}

impl PickEntry {
    fn new(name: String, dir: bool) -> Self {
        let lower = name.to_lowercase();
        Self { name, lower, dir }
    }
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
            .filter(|(_, e)| e.lower.starts_with(&partial))
            .map(|(i, _)| i)
            .collect()
    }
}

/// The partial-match sheet (clause 21): the places some of the files
/// already live, and the way to download fresh instead.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Matches {
    pub list: Vec<SeedMatch>,
    /// The dot: over the matches, then the download-fresh row at `len`.
    pub row: usize,
}

/// One thing this player sent the server, kept so the room can say what
/// became of it: the folder it lands in, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    /// The torrent's own name, as the server gave it back.
    pub name: String,
    pub vpath: String,
    /// The folder inside the library, `/`-joined; empty where the server
    /// said only which library (a torrent already on disk, now seeding).
    #[serde(default)]
    pub path: String,
    /// When, in milliseconds.
    pub at: i64,
}

impl Receipt {
    /// What its row is called: the folder's own name, else the torrent's.
    fn title(&self) -> String {
        self.path.rsplit('/').find(|s| !s.is_empty()).map_or_else(|| self.name.clone(), str::to_string)
    }

    /// The folder it sits in — the artist's, as a rule — else the library.
    fn parent(&self) -> String {
        let mut parts = self.path.split('/').filter(|s| !s.is_empty()).rev();
        parts.next();
        parts.next().map_or_else(|| self.vpath.clone(), str::to_string)
    }

    /// Where Files finds it.
    fn folder(&self) -> String {
        [self.vpath.as_str(), self.path.as_str()]
            .iter()
            .flat_map(|part| part.split('/'))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// How many receipts a server and account keep.
const RECEIPTS_KEPT: usize = 50;

/// The receipts as `torrents.json` holds them for one server and account.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ReceiptSlot {
    #[serde(default)]
    receipts: Vec<Receipt>,
}

/// How an auto-detect came out, for the line under the card's ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Ok,
    Gold,
}

/// What a typed path turned out to be, once a thread looked.
enum Loaded {
    Dir,
    File(Vec<u8>),
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
    /// The hand: None is stowed (↓ picks it up, Esc stows it; a nav room,
    /// it has no back for Esc again).
    pub cursor: Option<Row>,
    pub gate: Gate,
    /// Which session's server the gate answers for — a switch stales it.
    pub gate_for: String,
    /// Per-library path templates, once the server has said.
    pub templates: HashMap<String, String>,
    /// The chosen library, as an index into the App's list.
    pub vpath: usize,
    pub file: Option<LoadedFile>,
    /// What the loaded file holds: how many files, and their bytes.
    pub facts: Option<(u64, u64)>,
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
    pub sheet: Option<Sheet>,
    pub picker: Option<Picker>,
    pub chooser: Option<Chooser>,
    pub matches: Option<Matches>,
    /// Why the last add was refused, or a source could not be read: the
    /// server's own words, whole. It stands until the form changes.
    pub refusal: Option<String>,
    /// How the last auto-detect came out.
    pub detect: Option<(String, Tone)>,
    /// What this player sent this server, newest first.
    pub receipts: Vec<Receipt>,
    /// Whose receipts those are: the server's identity and the account.
    receipts_for: Option<(String, Option<String>)>,
    /// What the add on the wire will leave behind once it lands.
    sending: Option<Receipt>,
    /// The list's viewport.
    pub list: ListView,
    /// The letters live this frame, each with its control's act: a letter
    /// acts where its control is drawn.
    pub drawn: Vec<(char, TorAct)>,
    /// A check or an add is out: the form's controls stand where they
    /// are, neither clicking nor lighting, until the server answers.
    quiet: bool,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
}

impl TorrentUi {
    pub(crate) fn new() -> Self {
        let (tx, rx) = channel();
        TorrentUi {
            cursor: None,
            gate: Gate::NotAsked,
            gate_for: String::new(),
            templates: HashMap::new(),
            vpath: 0,
            file: None,
            facts: None,
            magnet: Input::default(),
            artist: Input::default(),
            album: Input::default(),
            year: Input::default(),
            path: Input::default(),
            path_edited: false,
            rename_root: true,
            force_fresh: false,
            busy: None,
            sheet: None,
            picker: None,
            chooser: None,
            matches: None,
            refusal: None,
            detect: None,
            receipts: Vec::new(),
            receipts_for: None,
            sending: None,
            list: ListView::default(),
            drawn: Vec::new(),
            quiet: false,
            tx,
            rx,
        }
    }

    /// Whether one of this room's overlays owns the pointer and keys.
    pub(crate) fn modal_open(&self) -> bool {
        self.picker.is_some() || self.chooser.is_some() || self.matches.is_some() || self.sheet.is_some()
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

    /// The stops that exist right now, top to bottom. `detect`: the server
    /// takes torrents, so it can be asked what one holds.
    fn rows(&self, detect: bool) -> Vec<Row> {
        let mut rows = Vec::new();
        if self.file.is_some() {
            rows.push(Row::File);
            if detect {
                rows.push(Row::Detect);
            }
            rows.extend([Row::HandOff, Row::Metadata, Row::Destination, Row::Force, Row::Submit]);
        } else if self.has_source() {
            rows.extend([Row::Magnet, Row::Metadata, Row::Destination, Row::Submit]);
        } else {
            rows.extend([Row::File, Row::TypePath, Row::Magnet]);
        }
        rows.extend((0..self.receipts.len()).map(Row::Receipt));
        rows
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn sep_for(path: &str) -> char {
    if path.contains('\\') && !path.contains('/') { '\\' } else { '/' }
}

/// Where the picker starts: the platform's Downloads — a torrent almost
/// always arrives through the browser (clause 2) — or the home when
/// there is no such folder.
fn picker_start() -> String {
    let Some(home) = crate::setup::local_home() else { return String::new() };
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
        && let Some(home) = crate::setup::local_home()
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
    // The user's own directory, not the shared temp dir: there another
    // local user can pre-create the folder, plant a symlink where the file
    // is written, or swap the staged file before the other app reads it.
    crate::config::config_dir()
        .map(|dir| dir.join("torrents"))
        .unwrap_or_else(|_| std::env::temp_dir().join("mstream-player-torrents"))
}

/// The most a `.torrent` can reasonably be: a typed path or `--torrent`
/// that names something else (a video, `/dev/zero`) is refused before it
/// is read, not after it filled memory.
const TORRENT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Stage a hand-off file under `dir`: created fresh (never through a file
/// or link already there), a numbered name when the name is taken.
fn stage_file(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut path = dir.join(name);
    let mut n = 1;
    loop {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, bytes)?;
                return Ok(path.to_string_lossy().into_owned());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => {
                path = dir.join(format!("{n}-{name}"));
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
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
        .map(|name| PickEntry::new(name, true))
        .chain(files.into_iter().map(|name| PickEntry::new(name, false)))
        .collect())
}

/// Read a typed path: a folder to descend into, or a file's bytes.
fn load_path(path: &str) -> Result<Loaded, String> {
    let md = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if md.is_dir() {
        return Ok(Loaded::Dir);
    }
    // A regular file of a torrent's size: not a device, a pipe, or a video
    // read whole before its shape is even looked at.
    if !md.is_file() || md.len() > TORRENT_MAX_BYTES {
        return Err(t!("gui.tor.not_a_torrent", name = path).to_string());
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, TORRENT_MAX_BYTES + 1), &mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Loaded::File(bytes))
}

/// A one-shot client for the live session, the servers room's way.
fn session_client(gui: &Gui) -> Result<Client, String> {
    let session = &gui.app.session;
    if !gui.app.connected || session.server.is_empty() {
        return Err(t!("gui.no_server").to_string());
    }
    // A peer is read-only (multi-server clause 26): nothing is added to it,
    // and nothing goes to its parent in its name.
    if session.peer.is_some() {
        return Err(t!("gui.srv.read_only").to_string());
    }
    // The session's own way in — a tunnel bridge's loopback token, the
    // entry's trust — not a bare client the bridge would drop.
    let reach = gui.app.reach(&gui.app.origin())?;
    Client::new_with(&reach.base, reach.self_signed)
        .map(|c| c.with_token(reach.token).with_local_token(reach.local_token).with_peer(reach.peer))
        .map_err(|e| e.to_string())
}

/// The chosen library's name, if the server has any.
fn library(gui: &Gui) -> Option<String> {
    gui.app.libraries.get(gui.torrent.vpath).cloned()
}

fn note(gui: &mut Gui, text: impl Into<String>, is_err: bool) {
    gui.note = Some((text.into(), is_err));
}

/// A refusal — the add's, a source's that could not be read: the words
/// stand on the page, whole, under the primary (under the two boxes while
/// there is no source), until the form changes.
fn refuse(gui: &mut Gui, words: impl Into<String>) {
    gui.torrent.refusal = Some(words.into());
}

// ── Opening the room, and what arrives in it ────────────────────────────────

/// The nav's Add torrent row (entry point 1): the room opens with its
/// hand stowed and asks the gate. The hub calls this from `Act::Nav`.
pub(crate) fn open_room(gui: &mut Gui) {
    // Where a paste arrives as keys (a Windows console), the link box
    // holds them from the start, so a pasted link lands in it; elsewhere
    // the hand is stowed and a paste is taken whole.
    gui.torrent.cursor = (!gui.ytdl.whole_pastes && !gui.torrent.has_source()).then_some(Row::Magnet);
    gui.torrent.list.stow();
    request_gate(gui);
    load_receipts(gui);
}

/// Whether the room is the Library's active one — a nav room, so the
/// Now Playing screen over it counts as away (the Auto DJ room's rule).
fn in_room(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == super::TORRENT_NAV
}

/// Open the room WITH a torrent already in hand (entry point 2): the nav
/// room, by the nav's own way in, from whatever was up.
fn open_room_with(gui: &mut Gui, incoming: Incoming) {
    gui.act(Act::Nav(super::TORRENT_NAV));
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
        // A check or an add still out was asked of the server just left:
        // its answer must not add to this one.
        gui.torrent.busy = None;
        gui.torrent.matches = None;
        gui.torrent.sheet = None;
        gui.torrent.refusal = None;
        gui.torrent.sending = None;
        // What was sent is the server's and the account's: another's now.
        gui.torrent.receipts.clear();
        gui.torrent.receipts_for = None;
        if gui.active == super::TORRENT_NAV {
            request_gate(gui);
            load_receipts(gui);
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
        gui.torrent.chooser = Some(Chooser { incoming, row: None, dont_ask: false });
    } else {
        open_room_with(gui, incoming);
    }
}

// ── The source and the metadata ─────────────────────────────────────────────

/// A file is the source now: the magnet goes (clause 1), and the
/// torrent's own name pre-fills the metadata (clause 10).
fn set_file(gui: &mut Gui, file: LoadedFile) {
    let name = meta::extract_torrent_name(&file.bytes);
    gui.torrent.facts = meta::torrent_facts(&file.bytes);
    gui.torrent.magnet = Input::default();
    gui.torrent.file = Some(file);
    gui.torrent.refusal = None;
    gui.torrent.detect = None;
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
    gui.torrent.refusal = None;
    gui.torrent.detect = None;
    if !value.is_empty() && gui.torrent.file.is_some() {
        gui.torrent.file = None;
        gui.torrent.facts = None;
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

/// Auto-detect (clause 11): the server reads the torrent and says how
/// sure it is.
fn auto_detect(gui: &mut Gui) {
    if gui.torrent.busy.is_some() {
        return;
    }
    let Some(file) = gui.torrent.file.clone() else { return };
    gui.torrent.detect = None;
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return gui.torrent.detect = Some((e, Tone::Gold)),
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
    gui.torrent.refusal = None;
    match plan_submit(gui) {
        Err(words) => refuse(gui, words),
        Ok(Plan::Check(file)) => start_check(gui, file),
        Ok(Plan::Add(req)) => start_add(gui, req),
    }
}

/// The seed-existing check first (clause 20): if the files are already on
/// disk, seed them instead of downloading again.
fn start_check(gui: &mut Gui, file: LoadedFile) {
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return refuse(gui, e),
    };
    gui.torrent.busy = Some(Busy::Checking);
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        let result = client.torrent_seed_existing(&file.bytes, &file.name).map_err(|e| e.to_string());
        let _ = tx.send(Reply::Checked(result));
    });
}

fn start_add(gui: &mut Gui, req: TorrentAddRequest) {
    let client = match session_client(gui) {
        Ok(client) => client,
        Err(e) => return refuse(gui, e),
    };
    gui.torrent.busy = Some(Busy::Adding);
    gui.torrent.sending = Some(receipt_of(&req));
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
            // The server says which library, not which folder.
            let name = check
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .or_else(|| gui.torrent.file.as_ref().map(|f| f.name.clone()))
                .unwrap_or_default();
            let vpath = check.vpath.clone().or_else(|| library(gui)).unwrap_or_default();
            keep_receipt(gui, Receipt { name, vpath, path: String::new(), at: 0 });
            reset_source(gui);
        }
        "already_in_daemon" => {
            note(gui, t!("gui.tor.already_in_client"), false);
            reset_source(gui);
        }
        "invalid_torrent" => {
            let words = check.error.unwrap_or_else(|| t!("gui.tor.invalid_file").to_string());
            refuse(gui, words);
        }
        "match_unmapped" => {
            let vpath = check.vpath.unwrap_or_default();
            refuse(gui, t!("gui.tor.match_unmapped", vpath = vpath));
        }
        "pad_files_missing" => {
            let vpath = check.vpath.unwrap_or_default();
            let client = check.client_type.unwrap_or_else(|| "this client".to_string());
            refuse(gui, t!("gui.tor.pad_files_missing", vpath = vpath, client = client));
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
    let Some(vpath) = library(gui) else { return refuse(gui, t!("gui.tor.pick_library")) };
    let (sub_path, directory_name) = meta::split_path(gui.torrent.path.value().trim());
    if directory_name.is_empty() {
        return refuse(gui, t!("gui.tor.path_empty"));
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
    tor.facts = None;
    tor.refusal = None;
    tor.detect = None;
    tor.sheet = None;
    tor.sending = None;
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
    // A torrent the client already had left nothing new behind.
    if let Some(receipt) = gui.torrent.sending.take().filter(|_| !added.is_duplicate) {
        keep_receipt(gui, Receipt { name: added.name.clone().filter(|n| !n.is_empty()).unwrap_or(receipt.name.clone()), ..receipt });
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
                stage_file(&handoff_dir(), &file.name, &file.bytes).map_err(|e| e.to_string())
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
    let start = {
        let dir = picker_start();
        let trimmed = dir.trim_end_matches(['/', '\\']);
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    };
    let tx = gui.torrent.tx.clone();
    std::thread::spawn(move || {
        use crate::setup::picker::{self, TORRENT_TITLE};
        let _ = tx.send(Reply::Picked(picker::pick_torrent(TORRENT_TITLE, start.as_deref())));
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
    // The wizard's tidy: `~` expands, doubled separators collapse, the
    // cursor keeps its distance from the end.
    if let Some(tidied) = crate::setup::tidy_path_input(&picker.text) {
        picker.text = tidied;
    }
    let cleaned = picker.text.value().to_string();
    let (dir, _) = crate::setup::split_input(&cleaned);
    let dir = if dir == "~" {
        match crate::setup::local_home() {
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

/// Drain what the threads sent since the last pass; true when anything
/// came — a frame to draw.
pub(crate) fn poll(gui: &mut Gui) -> bool {
    let mut folded = false;
    while let Ok(reply) = gui.torrent.rx.try_recv() {
        apply_reply(gui, reply);
        folded = true;
    }
    folded
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
                    // A folder is the typed picker's to walk: opened on it
                    // where a pasted path named one.
                    let sep = sep_for(&path);
                    let dir = path.trim_end_matches(['/', '\\']).to_string();
                    let picker = gui.torrent.picker.get_or_insert_with(Picker::default);
                    picker.text = Input::new(format!("{dir}{sep}"));
                    picker_refresh(gui);
                }
                Ok(Loaded::File(bytes)) if meta::is_torrent_file(&bytes) => {
                    gui.torrent.picker = None;
                    set_file(gui, LoadedFile { name, bytes });
                    // The hand down: Enter adds, the letters work the card.
                    gui.torrent.cursor = None;
                }
                Ok(Loaded::File(_)) => {
                    let words = t!("gui.tor.not_a_torrent", name = name).to_string();
                    match gui.torrent.picker.as_mut() {
                        Some(picker) => picker.error = Some(words),
                        None => refuse(gui, words),
                    }
                }
                Err(e) => {
                    let words = t!("gui.tor.read_failed", name = name, err = e).to_string();
                    match gui.torrent.picker.as_mut() {
                        Some(picker) => picker.error = Some(words),
                        None => refuse(gui, words),
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
                    gui.torrent.detect = Some(if sure {
                        (t!("gui.tor.detected").to_string(), Tone::Ok)
                    } else {
                        (t!("gui.tor.detect_guess").to_string(), Tone::Gold)
                    });
                }
                Ok(detect) => {
                    let words = detect
                        .message
                        .filter(|m| !m.is_empty())
                        .unwrap_or_else(|| t!("gui.tor.detect_none").to_string());
                    gui.torrent.detect = Some((words, Tone::Gold));
                }
                Err(e) => gui.torrent.detect = Some((e, Tone::Gold)),
            }
        }
        // An answer nothing waits for any more — the server changed under
        // the request — is dropped rather than added to the new server.
        Reply::Checked(result) => {
            let stale = gui.torrent.busy.is_none();
            gui.torrent.busy = None;
            if !stale {
                match result {
                    Ok(check) => apply_check(gui, check),
                    Err(e) => refuse(gui, e),
                }
            }
        }
        Reply::Added(result) => {
            let stale = gui.torrent.busy.is_none();
            gui.torrent.busy = None;
            if !stale {
                match result {
                    Ok(added) => apply_added(gui, added),
                    Err(e) => {
                        gui.torrent.sending = None;
                        refuse(gui, t!("gui.tor.add_failed", err = e));
                    }
                }
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

// ── What was sent ───────────────────────────────────────────────────────────

/// Whose receipts the room shows: the session's own server and account (a
/// peer is read-only, so nothing is ever sent to one).
fn receipts_owner(gui: &Gui) -> Option<(String, Option<String>)> {
    let id = gui.app.origin().server;
    (gui.app.connected && gui.app.session.peer.is_none() && !id.is_empty()).then(|| (id, gui.app.session.username.clone()))
}

/// Read what this account sent this server, once per session of theirs.
fn load_receipts(gui: &mut Gui) {
    let owner = receipts_owner(gui);
    if owner.is_none() || gui.torrent.receipts_for == owner {
        return;
    }
    gui.torrent.receipts_for = owner.clone();
    gui.torrent.receipts.clear();
    // Under test the list is memory's alone: no test reads or writes the file.
    if cfg!(test) {
        return;
    }
    if let Some((id, user)) = owner
        && let Ok(Some(slot)) = crate::config::load_torrent_slot::<ReceiptSlot>(&id, user.as_deref())
    {
        gui.torrent.receipts = slot.receipts;
        gui.torrent.receipts.truncate(RECEIPTS_KEPT);
    }
}

fn save_receipts(gui: &mut Gui) {
    if cfg!(test) {
        return;
    }
    let Some((id, user)) = gui.torrent.receipts_for.clone() else { return };
    let slot = ReceiptSlot { receipts: gui.torrent.receipts.clone() };
    if let Err(e) = crate::config::save_torrent_slot(&id, user.as_deref(), Some(&slot)) {
        tracing::warn!("what Add torrent sent was not saved: {e}");
    }
}

/// What an add will leave in the list once the server takes it: the
/// folder it was sent to.
fn receipt_of(req: &TorrentAddRequest) -> Receipt {
    let path = [req.sub_path.as_str(), req.directory_name.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/");
    Receipt { name: req.directory_name.clone(), vpath: req.vpath.clone(), path, at: 0 }
}

/// A new receipt, first in the list, stamped now.
fn keep_receipt(gui: &mut Gui, receipt: Receipt) {
    load_receipts(gui);
    let at = gui.ytdl.now();
    gui.torrent.receipts.insert(0, Receipt { at, ..receipt });
    gui.torrent.receipts.truncate(RECEIPTS_KEPT);
    gui.torrent.list.scroll = 0;
    save_receipts(gui);
}

/// A row's folder, opened in Files: a jump, as Add from YouTube's Show in
/// folder is — the columns walked to before are no way back from there.
fn open_receipt(gui: &mut Gui, i: usize) {
    let Some(folder) = gui.torrent.receipts.get(i).map(Receipt::folder).filter(|f| !f.is_empty()) else { return };
    gui.act(Act::Nav(super::FILES_NAV));
    gui.app.tab = Tab::Files;
    gui.app.path = folder.clone();
    gui.app.files.trail.clear();
    gui.app.files.set(Vec::new());
    gui.pend(vec![crate::tui::app::Effect::Api(ApiCmd::Browse(folder))]);
}

// ── Paste ───────────────────────────────────────────────────────────────────

/// The room takes a paste whole: it is up, and nothing stands over it (a
/// sheet's box takes its own, typed).
pub(crate) fn wants_paste(gui: &Gui) -> bool {
    in_room(gui) && !gui.modal_open() && !gui.servers.drop_open
}

/// A paste, whole, wherever the hand is. A magnet link fills the link box
/// and lets a loaded file go; anything else is read as a path — a file
/// dropped on a terminal arrives as one — and loaded.
pub(crate) fn paste(gui: &mut Gui, text: &str) {
    if !wants_paste(gui) || matches!(gui.torrent.busy, Some(Busy::Checking | Busy::Adding)) {
        return;
    }
    let line = super::ytdl::clean_paste(text);
    if line.is_empty() {
        return;
    }
    if line.to_lowercase().starts_with("magnet:") {
        let real = meta::is_valid_magnet(&line);
        gui.torrent.magnet = Input::new(line);
        magnet_changed(gui);
        // A link that is one leaves the hand down, Enter adding; one that
        // is not is the link box's to mend.
        gui.torrent.cursor = (!real).then_some(Row::Magnet);
    } else {
        gui.torrent.refusal = None;
        picker_load(gui, expand_tilde(&unescape_dropped(&line)));
    }
}

// ── Drawing the room ────────────────────────────────────────────────────────

fn gold() -> Style {
    Style::default().fg(th().gold)
}

fn ok() -> Style {
    Style::default().fg(th().ok)
}

fn danger_bold() -> Style {
    Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
}

fn clip(text: &str, room: usize) -> String {
    super::bar::clip(text, room).into_owned()
}

/// A size as a card says it: GB past a gigabyte, whole megabytes past
/// ten, a decimal below that, kilobytes under one.
fn size_words(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).ceil())
    }
}

/// What a loaded file's card says under its name: `12 files · 356 MB`.
fn facts_words(facts: (u64, u64)) -> String {
    format!("{} · {}", keep::plural("gui.tor.files", facts.0), size_words(facts.1))
}

/// Whether the hand is on `row` — and the room's to show: the queue panel
/// holding the keys takes it from the room.
fn hand_on(gui: &Gui, row: Row) -> bool {
    gui.torrent.cursor == Some(row) && !queue_has_keys(gui)
}

fn queue_has_keys(gui: &Gui) -> bool {
    gui.queue_open && gui.app.focus == Focus::Queue
}

/// A control's words: ACCENT BOLD under the hand, BRIGHT BOLD under the
/// pointer, `rest` otherwise.
fn word_style(hand: bool, hover: bool, rest: Style) -> Style {
    match (hand, hover) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => rest,
    }
}

/// A tall control as wide as it is given: a Rounded frame in `ring`, its
/// label centred in `words`.
fn wide_frame(frame: &mut Frame, at: Rect, label: &str, ring: Color, words: Style) {
    crate::kit::cursor_ring(frame, at, Style::default().fg(ring));
    let room = at.width.saturating_sub(2);
    let label = clip(label, usize::from(room));
    let x = at.x + 1 + (room.saturating_sub(width(&label) as u16) + 1) / 2;
    put(frame, x, at.y + 1, &label, words);
}

/// The gate's standing, for the title row's right end: the client and
/// `ready`, once the server has said it takes torrents.
fn standing(gui: &Gui) -> Option<String> {
    match &gui.torrent.gate {
        Gate::Answered(p) if p.ok() => {
            let client = p.display_name.clone().or_else(|| p.client_type.clone()).unwrap_or_default();
            Some(t!("gui.tor.ready", client = client).to_string())
        }
        _ => None,
    }
}

/// The gate's words under the title: that it is being asked, why the
/// server can't take one — its own reason, whole — or that there is no
/// server to ask; on four rows at most.
fn banner(gui: &Gui, w: usize) -> Vec<(String, Style)> {
    let (words, style) = match &gui.torrent.gate {
        Gate::Pending => (t!("gui.tor.checking").to_string(), accent()),
        Gate::NotAsked if !gui.app.connected => (t!("gui.no_server").to_string(), dim()),
        Gate::Answered(p) if !p.ok() => (p.reason.clone().unwrap_or_else(|| t!("gui.tor.unavailable").to_string()), gold()),
        Gate::Failed(e) => (t!("gui.tor.preflight_failed", err = e).to_string(), gold()),
        _ => return Vec::new(),
    };
    crate::kit::wrap_whole(&words, w.max(1)).into_iter().take(4).map(|line| (line, style)).collect()
}

/// The primary's label, whether it can be pressed, and whether a press
/// is out — `None` while there is no source to add.
fn primary_of(gui: &Gui) -> Option<(String, bool, bool)> {
    let tor = &gui.torrent;
    if !tor.has_source() {
        return None;
    }
    if matches!(tor.busy, Some(Busy::Adding | Busy::Checking)) {
        return Some((t!("gui.tor.submitting").to_string(), false, true));
    }
    let ready = tor.gate.ok()
        && tor.busy.is_none()
        && library(gui).is_some()
        && !meta::split_path(tor.path.value().trim()).1.is_empty();
    let label = t!("gui.tor.submit").to_string();
    Some((if ready { format!("{label} {}", super::forward_glyph()) } else { label }, ready, false))
}

/// Why the primary is off, for its tip.
fn primary_why(gui: &Gui) -> String {
    match &gui.torrent.gate {
        Gate::Answered(p) if !p.ok() => p.reason.clone().unwrap_or_else(|| t!("gui.tor.unavailable").to_string()),
        gate if !gate.ok() => t!("gui.tor.unavailable").to_string(),
        _ if library(gui).is_none() => t!("gui.tor.pick_library").to_string(),
        _ if gui.torrent.busy.is_some() => t!("gui.tor.submitting").to_string(),
        _ => t!("gui.tor.path_empty").to_string(),
    }
}

/// The card's ways, laid from the left with ` · ` between them: as many a
/// row as fit `w`.
fn way_rows(gui: &Gui, w: usize) -> Vec<Vec<Row>> {
    let stops = gui.torrent.rows(gui.torrent.gate.ok());
    let mut rows: Vec<Vec<Row>> = Vec::new();
    let mut used = 0;
    for way in [Row::Detect, Row::HandOff].into_iter().filter(|way| stops.contains(way)) {
        let cells = width(&way_label(way));
        match rows.last_mut() {
            Some(row) if used + 3 + cells <= w => {
                row.push(way);
                used += 3 + cells;
            }
            _ => {
                rows.push(vec![way]);
                used = cells;
            }
        }
    }
    rows
}

fn way_label(way: Row) -> String {
    match way {
        Row::Detect => t!("gui.tor.auto_detect").to_string(),
        _ => t!("gui.tor.open_with").to_string(),
    }
}

/// How the last auto-detect came out, as the lines under the ways: GOLD
/// with its `!` unless the server was sure.
fn detect_lines(gui: &Gui, w: usize) -> Vec<(String, Style)> {
    match &gui.torrent.detect {
        Some((words, Tone::Ok)) => crate::kit::wrap_whole(words, w.max(1)).into_iter().map(|l| (l, ok())).collect(),
        Some((words, Tone::Gold)) => super::ytdl::gold_rows(words, w).into_iter().map(|l| (l, gold())).collect(),
        None => Vec::new(),
    }
}

/// The destination as crumbs: the library, then the path's folders.
fn dest_crumbs(gui: &Gui) -> Vec<String> {
    let mut crumbs: Vec<String> = library(gui).into_iter().collect();
    crumbs.extend(path_folders(gui.torrent.path.value()));
    crumbs
}

fn path_folders(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split(['/', '\\']).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Crumbs that fit `room` cells: all of them where they do; else the
/// first — the library, which is never the one to go — an ellipsis, and
/// as many from the right as fit; the last cut where it alone is too wide.
fn fit_dest(crumbs: &[String], room: usize) -> Vec<String> {
    let sep = width(crumb_sep());
    let wide = |items: &[String]| items.iter().map(|c| width(c)).sum::<usize>() + sep * items.len().saturating_sub(1);
    if wide(crumbs) <= room {
        return crumbs.to_vec();
    }
    let ell = keep::ell("…");
    let (Some(first), Some(last)) = (crumbs.first(), crumbs.last()) else { return Vec::new() };
    if crumbs.len() == 1 {
        return vec![clip(first, room.max(1))];
    }
    for from in 2..crumbs.len() {
        let mut items = vec![first.clone(), ell.clone()];
        items.extend(crumbs[from..].iter().cloned());
        if wide(&items) <= room {
            return items;
        }
    }
    let lead = width(first) + sep + width(&ell) + sep;
    if room > lead + 1 {
        return vec![first.clone(), ell, clip(last, room - lead)];
    }
    vec![clip(last, room.max(1))]
}

/// Crumbs from `x`, TEXT (BOLD where `bold`) between DIM `›`; an ellipsis
/// among them DIM. Returns the cells drawn.
fn draw_crumbs(frame: &mut Frame, x: u16, y: u16, crumbs: &[String], style: Style) -> u16 {
    let ell = keep::ell("…");
    let mut at = x;
    for (i, crumb) in crumbs.iter().enumerate() {
        if i > 0 {
            put(frame, at, y, crumb_sep(), dim());
            at += width(crumb_sep()) as u16;
        }
        put(frame, at, y, crumb, if *crumb == ell { dim() } else { style });
        at += width(crumb) as u16;
    }
    at - x
}

fn crumbs_width(crumbs: &[String]) -> u16 {
    (crumbs.iter().map(|c| width(c)).sum::<usize>() + width(crumb_sep()) * crumbs.len().saturating_sub(1)) as u16
}

/// One piece of the page, top to bottom under the title.
#[derive(Debug, Clone)]
enum Piece {
    /// A blank that gives way when the room is short — the lower its
    /// rank, the sooner.
    Blank(u8),
    Line(String, Style),
    FileBox,
    TypePath,
    Or,
    LinkBand,
    Card,
    Ways(Vec<Row>),
    Meta,
    Dest,
    Force,
    Primary,
}

impl Piece {
    fn rows(&self, gui: &Gui) -> u16 {
        match self {
            Piece::FileBox | Piece::LinkBand | Piece::Primary => 3,
            Piece::Card => 3 + u16::from(gui.torrent.facts.is_some()),
            _ => 1,
        }
    }
}

/// The page's pieces for the state it is in, the primary among them (the
/// caller takes it out where the room is too short for it).
fn pieces(gui: &Gui, w: usize) -> Vec<Piece> {
    let tor = &gui.torrent;
    let mut pieces = Vec::new();
    let banner = banner(gui, w);
    if banner.is_empty() {
        pieces.push(Piece::Blank(0));
    }
    pieces.extend(banner.into_iter().map(|(line, style)| Piece::Line(line, style)));
    let refusal = || tor.refusal.iter().flat_map(|words| crate::kit::wrap_whole(words, w.max(1))).map(|l| Piece::Line(l, gold()));
    if tor.file.is_some() {
        pieces.push(Piece::Card);
        pieces.extend(way_rows(gui, w).into_iter().map(Piece::Ways));
        pieces.extend(detect_lines(gui, w).into_iter().map(|(line, style)| Piece::Line(line, style)));
    } else if tor.has_source() {
        pieces.push(Piece::LinkBand);
        if let Some(name) = meta::magnet_display_name(tor.magnet.value()) {
            pieces.push(Piece::Line(clip(&name, w), dim()));
        }
    } else {
        pieces.extend([Piece::FileBox, Piece::TypePath, Piece::Or, Piece::LinkBand]);
        if !tor.magnet.value().trim().is_empty() {
            pieces.push(Piece::Line(clip(&t!("gui.tor.magnet_invalid"), w), gold()));
        }
        // No primary to stand under: a source that could not be read says
        // so under the two boxes.
        pieces.extend(refusal());
        return pieces;
    }
    pieces.extend([Piece::Blank(2), Piece::Meta, Piece::Dest]);
    if tor.file.is_some() {
        pieces.push(Piece::Force);
    }
    pieces.extend([Piece::Blank(1), Piece::Primary]);
    if tor.busy == Some(Busy::Checking) {
        pieces.push(Piece::Line(clip(&t!("gui.tor.checking_files"), w), accent()));
    }
    pieces.extend(refusal());
    pieces
}

/// The room (the canvas's Main, Empty, After, Min, Wide and the states):
/// the title and the gate's standing; the two boxes, or the source in
/// hand with what it is; the two rows and the checkbox; the primary, tall
/// under them — a slab on the title row where the room is too short for
/// it; a refusal under it; then what was sent, down to the room's last
/// row. Short of rows the blanks give way — the one under the title, then
/// the form's from the bottom up, then the one over the list — until the
/// list has its three.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    gui.torrent.drawn.clear();
    load_receipts(gui);
    let block = super::ytdl::room_block(content);
    let (x, w) = (block.x, usize::from(block.width));
    let bottom = content.bottom();
    gui.torrent.quiet = matches!(gui.torrent.busy, Some(Busy::Checking | Busy::Adding));

    let mut pieces = pieces(gui, w);
    let primary = primary_of(gui);
    let receipts = gui.torrent.receipts.len();
    let h = usize::from(content.height);
    let used = |pieces: &[Piece]| 1 + pieces.iter().map(|p| usize::from(p.rows(gui))).sum::<usize>();
    // The list's rows: its blank, its header and `rows` of it.
    let list = |rows: usize, blank: bool| if receipts == 0 { 0 } else { usize::from(blank) + 1 + rows };
    let tall = primary.is_some() && used(&pieces) + list(1, true) <= h;
    let mut list_blank = true;
    if !tall {
        // The slab holds the primary: its three rows go, and the blank
        // over them where nothing stands under it.
        if let Some(at) = pieces.iter().position(|p| matches!(p, Piece::Primary)) {
            pieces.remove(at);
            if at == pieces.len() {
                pieces.remove(at - 1);
            }
        }
        let wants = receipts.min(LIST_WANTS);
        for rank in 0..3 {
            if used(&pieces) + list(wants, list_blank) <= h {
                break;
            }
            pieces.retain(|p| !matches!(p, Piece::Blank(r) if *r == rank));
        }
        if used(&pieces) + list(wants, list_blank) > h {
            list_blank = false;
        }
    }

    // The title row: the slab where the primary is one, else the gate's
    // standing; the title gives way to either, two cells clear of it.
    let mut y = content.y;
    let mut title_end = block.right();
    match primary.clone().filter(|_| !tall) {
        Some((label, enabled, busy)) => title_end = draw_primary_slab(frame, gui, block, y, &label, enabled, busy).saturating_sub(2),
        None => {
            if let Some(words) = standing(gui) {
                let words = clip(&words, w.saturating_sub(width(&t!("gui.tor.title")) + 2));
                let at = block.right().saturating_sub(width(&words) as u16);
                put(frame, at, y, &words, dim());
                title_end = at.saturating_sub(2);
            }
        }
    }
    let title = clip(&t!("gui.tor.title"), usize::from(title_end.saturating_sub(x)));
    put(frame, x, y, &title, Style::default().add_modifier(Modifier::BOLD));
    y += 1;

    for piece in pieces {
        let rows = piece.rows(gui);
        if y + rows > bottom {
            break;
        }
        let at = Rect { x, y, width: block.width, height: rows };
        match piece {
            Piece::Blank(_) => {}
            Piece::Line(text, style) => put(frame, x, y, &text, style),
            Piece::FileBox => draw_file_box(frame, gui, at),
            Piece::TypePath => draw_type_path(frame, gui, block, y),
            Piece::Or => draw_or(frame, block, y),
            Piece::LinkBand => draw_link_band(frame, gui, at),
            Piece::Card => draw_card(frame, gui, at),
            Piece::Ways(ways) => draw_ways(frame, gui, block, y, &ways),
            Piece::Meta => draw_meta_row(frame, gui, block, y),
            Piece::Dest => draw_dest_row(frame, gui, block, y),
            Piece::Force => draw_force(frame, gui, block, y),
            Piece::Primary => {
                if let Some((label, enabled, busy)) = primary.clone() {
                    draw_primary(frame, gui, at, &label, enabled, busy);
                }
            }
        }
        y += rows;
    }
    gui.torrent.quiet = false;

    // What was sent, in what is left.
    y += u16::from(list_blank);
    if receipts > 0 && y + 1 < bottom {
        draw_list(frame, gui, Rect { x, y, width: block.width, height: bottom - y });
    }
}

/// The rows of the list the room keeps room for.
const LIST_WANTS: usize = 3;

/// The file's box while there is none: a tall frame the block's width
/// that opens the file dialog — DIM with its words TEXT, BRIGHT under the
/// pointer, ACCENT under the hand.
fn draw_file_box(frame: &mut Frame, gui: &mut Gui, at: Rect) {
    let picking = gui.torrent.busy == Some(Busy::Picking);
    let live = !gui.torrent.quiet && !picking;
    let (ring, words) = match (picking, hand_on(gui, Row::File), live && gui.ui.hovers(at)) {
        (true, _, _) => (th().dim, accent()),
        (_, true, _) => (th().accent, accent().add_modifier(Modifier::BOLD)),
        (_, _, true) => (th().bright, bright_bold()),
        _ => (th().dim, if live { Style::default() } else { dim() }),
    };
    let label = if picking { t!("gui.tor.picking") } else { t!("gui.tor.choose_file") };
    wide_frame(frame, at, &label, ring, words);
    if live {
        gui.ui.click(at, tor(TorAct::Pick));
        gui.torrent.drawn.push(('b', TorAct::Pick));
    }
}

/// `type a path ▸` at the block's right end, under the file's box: the
/// typed road — a terminal is where paths are typed, and SSH has no
/// dialog at all.
fn draw_type_path(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16) {
    let label = format!("{} {}", t!("gui.tor.type_path"), super::forward_glyph());
    let at = Rect { x: block.right().saturating_sub(width(&label) as u16), y, width: width(&label) as u16, height: 1 };
    let live = !gui.torrent.quiet;
    let style = if live { word_style(hand_on(gui, Row::TypePath), gui.ui.hovers(at), dim()) } else { dim() };
    put(frame, at.x, y, &label, style);
    if live {
        gui.ui.click(at, tor(TorAct::Type));
        gui.torrent.drawn.push(('t', TorAct::Type));
    }
}

/// The record's "or" between the two boxes, a DIM rule either side of it.
fn draw_or(frame: &mut Frame, block: Rect, y: u16) {
    let word = t!("gui.tor.or").to_string();
    let rule = if legacy_conhost() { "-" } else { "─" };
    let room = usize::from(block.width).saturating_sub(width(&word) + 2);
    let left = room / 2;
    put(frame, block.x, y, &format!("{} {word} {}", rule.repeat(left), rule.repeat(room - left)), dim());
}

/// The link's band, three rows of the block's width: the link box and —
/// where this build can read the clipboard — `Paste` beside it, a DIM
/// tall frame one cell off (BRIGHT under the pointer).
fn draw_link_band(frame: &mut Frame, gui: &mut Gui, band: Rect) {
    let label = t!("gui.ytdl.room.paste_button").to_string();
    let button = width(&label) as u16 + 4;
    let paste = gui.ytdl.clipboard.is_some() && band.width >= button + 1 + 16;
    let card = Rect { width: if paste { band.width - button - 1 } else { band.width }, height: 3, ..band };
    draw_link_card(frame, gui, card);
    if paste {
        let at = Rect { x: card.right() + 1, y: band.y, width: button, height: 3 };
        let tone = |hovered: bool| if hovered { (th().bright, true) } else { (th().dim, false) };
        let act = (!gui.torrent.quiet).then(|| tor(TorAct::PasteClip));
        crate::kit::tall_frame(frame, &mut gui.ui, at, &label, 1, tone, act);
    }
}

/// The link box: ACCENT with the caret while it has the hand, its DIM
/// placeholder while empty, `[X]` to clear it.
fn draw_link_card(frame: &mut Frame, gui: &mut Gui, card: Rect) {
    let live = !gui.torrent.quiet;
    let keys = live && hand_on(gui, Row::Magnet);
    let tone = if keys {
        th().accent
    } else if live && gui.ui.hovers(card) {
        th().bright
    } else {
        th().dim
    };
    crate::kit::cursor_ring(frame, card, Style::default().fg(tone));
    if live {
        gui.ui.click(card, tor(TorAct::Stop(Row::Magnet)));
    }
    let (fx, fy) = (card.x + 2, card.y + 1);
    let value = gui.torrent.magnet.value().to_string();
    let cursor = gui.torrent.magnet.cursor();
    let x_at = Rect { x: card.right().saturating_sub(5), y: fy, width: 3, height: 1 };
    let room = x_at.x.saturating_sub(fx + 1);
    let placeholder = clip(&t!("gui.tor.paste_link"), usize::from(room));
    if keys {
        super::text_field(frame, &mut gui.ui, fx, fy, &value, cursor, room, Style::default());
        if value.is_empty() {
            put(frame, fx + 1, fy, &placeholder, dim());
        }
    } else if value.is_empty() {
        put(frame, fx, fy, &placeholder, dim());
    } else {
        put(frame, fx, fy, &clip(&value, usize::from(room)), Style::default());
    }
    if !value.is_empty() {
        put(frame, x_at.x, fy, "[X]", if live && gui.ui.hovers(x_at) { danger_bold() } else { dim() });
        if live {
            gui.ui.click(x_at, tor(TorAct::ClearLink));
            gui.torrent.drawn.push(('x', TorAct::ClearLink));
        }
    }
}

/// The loaded file's card: its name BOLD and `[X]` to let it go, and
/// under the name how many files it holds and how big they are. A click
/// on it chooses another.
fn draw_card(frame: &mut Frame, gui: &mut Gui, at: Rect) {
    let live = !gui.torrent.quiet;
    let tone = if hand_on(gui, Row::File) {
        th().accent
    } else if live && gui.ui.hovers(at) {
        th().bright
    } else {
        th().dim
    };
    crate::kit::cursor_ring(frame, at, Style::default().fg(tone));
    let (fx, fy) = (at.x + 2, at.y + 1);
    let x_at = Rect { x: at.right().saturating_sub(5), y: fy, width: 3, height: 1 };
    let room = usize::from(x_at.x.saturating_sub(fx + 1));
    // The name alone: the loaded file carries the torrent's bytes, and a
    // clone per frame copied them to draw a card.
    let name = gui.torrent.file.as_ref().map(|f| f.name.clone()).unwrap_or_default();
    let bold = Style::default().add_modifier(Modifier::BOLD);
    put(frame, fx, fy, &clip(&name, room), bold);
    if let Some(facts) = gui.torrent.facts {
        put(frame, fx, fy + 1, &clip(&facts_words(facts), usize::from(at.width.saturating_sub(4))), dim());
    }
    put(frame, x_at.x, fy, "[X]", if live && gui.ui.hovers(x_at) { danger_bold() } else { dim() });
    if live {
        gui.ui.click(at, tor(TorAct::Pick));
        gui.torrent.drawn.push(('b', TorAct::Pick));
        gui.ui.click(x_at, tor(TorAct::Unload));
        gui.ui.tip_keyed(x_at, t!("gui.tor.unload_tip").to_string());
        gui.torrent.drawn.push(('x', TorAct::Unload));
    }
}

/// A row of the card's ways: TEXT words, DIM ` · ` between them — a way
/// that is out says so in ACCENT and does nothing.
fn draw_ways(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, ways: &[Row]) {
    let mut at = block.x;
    for (i, way) in ways.iter().enumerate() {
        if i > 0 {
            put(frame, at, y, " · ", dim());
            at += 3;
        }
        let (out, act, letter) = match way {
            Row::Detect => (gui.torrent.busy == Some(Busy::Detecting), TorAct::Detect, 'd'),
            _ => (gui.torrent.busy == Some(Busy::HandingOff), TorAct::HandOff, 'o'),
        };
        let label = if out && *way == Row::Detect { t!("gui.tor.detecting").to_string() } else { way_label(*way) };
        let rect = Rect { x: at, y, width: width(&label) as u16, height: 1 };
        let live = !gui.torrent.quiet && gui.torrent.busy.is_none();
        let style = match (out, live) {
            (true, _) => accent(),
            (false, true) => word_style(hand_on(gui, *way), gui.ui.hovers(rect), Style::default()),
            (false, false) => dim(),
        };
        put(frame, at, y, &label, style);
        if live {
            gui.ui.click(rect, tor(act.clone()));
            gui.torrent.drawn.push((letter, act));
        }
        at += rect.width;
    }
}

/// A row that opens a sheet: its label at the left, `▸` at the right —
/// the whole row the control. Returns where its value may end (two cells
/// before the `▸`) and the cells the value may take.
fn sheet_row(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, row: Row, label: &str, (letter, act): (char, TorAct)) -> (u16, usize) {
    let rect = Rect { x: block.x, y, width: block.width, height: 1 };
    let live = !gui.torrent.quiet;
    let hand = hand_on(gui, row);
    let hover = live && gui.ui.hovers(rect);
    put(frame, block.x, y, label, word_style(live && hand, hover, Style::default()));
    let glyph = super::forward_glyph();
    let at = block.right().saturating_sub(width(glyph) as u16);
    put(frame, at, y, glyph, if (live && hand) || hover { Style::default().fg(th().bright) } else { dim() });
    if live {
        gui.ui.click(rect, tor(act.clone()));
        gui.torrent.drawn.push((letter, act));
    }
    let end = at.saturating_sub(1);
    (end, usize::from(end.saturating_sub(block.x + width(label) as u16 + 2)))
}

/// `Metadata`: the artist, the album and the year the torrent's name
/// gave — or, where it gave neither artist nor album, a GOLD pointer at
/// the sheet that takes them.
fn draw_meta_row(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16) {
    let label = t!("gui.tor.metadata").to_string();
    let (end, room) = sheet_row(frame, gui, block, y, Row::Metadata, &label, ('e', TorAct::OpenMetadata(MetaStop::Artist)));
    let m = gui.torrent.meta();
    if m.artist.is_empty() && m.album.is_empty() {
        let key = t!("gui.ytdl.key.type");
        let words = clip(&format!("! {}", super::ytdl::page_words(&key).1), room);
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, gold());
        return;
    }
    let parts: Vec<&str> = [m.artist.as_str(), m.album.as_str(), m.year.as_str()].into_iter().filter(|p| !p.is_empty()).collect();
    let whole = parts.join(" · ");
    if width(&whole) > room {
        let words = clip(&whole, room);
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, Style::default());
        return;
    }
    let mut at = end.saturating_sub(width(&whole) as u16);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            put(frame, at, y, " · ", dim());
            at += 3;
        }
        put(frame, at, y, part, Style::default());
        at += width(part) as u16;
    }
}

/// `Destination`: where the files land, the library first — which never
/// gives way — then the folders; the library alone, DIM, while there is
/// no folder yet.
fn draw_dest_row(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16) {
    let label = t!("gui.tor.destination").to_string();
    let (end, room) = sheet_row(frame, gui, block, y, Row::Destination, &label, ('l', TorAct::OpenDestination(DestStop::Path)));
    let crumbs = dest_crumbs(gui);
    if library(gui).is_none() {
        let words = clip(&t!("gui.tor.no_libraries"), room);
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, dim());
    } else if crumbs.len() == 1 {
        let words = clip(&crumbs[0], room);
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, dim());
    } else {
        let shown = fit_dest(&crumbs, room);
        draw_crumbs(frame, end.saturating_sub(crumbs_width(&shown)), y, &shown, Style::default());
    }
}

/// `[ ] Force fresh download`, a file's alone: what it skips is its tip.
fn draw_force(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16) {
    let (check_on, check_off) = super::check_glyphs();
    let on = gui.torrent.force_fresh;
    let label = clip(&t!("gui.tor.force_fresh"), usize::from(block.width.saturating_sub(4)));
    let rect = Rect { x: block.x, y, width: 4 + width(&label) as u16, height: 1 };
    let live = !gui.torrent.quiet;
    let hover = live && gui.ui.hovers(rect);
    put(frame, block.x, y, if on { check_on } else { check_off }, if on { ok() } else { dim() });
    put(frame, block.x + 4, y, &label, word_style(live && hand_on(gui, Row::Force), hover, Style::default()));
    if live {
        gui.ui.click(rect, tor(TorAct::ToggleForce));
        gui.ui.tip(rect, t!("gui.tor.force_fresh_desc").to_string());
    }
}

/// The primary, tall and the block's width: ACCENT, its label BOLD and
/// centred — BRIGHT under the pointer or the hand; off, DIM with no `▸`
/// and its tip saying why; out, its frame DIM and `Adding…` ACCENT.
fn draw_primary(frame: &mut Frame, gui: &mut Gui, at: Rect, label: &str, enabled: bool, busy: bool) {
    let lit = enabled && (gui.ui.hovers(at) || hand_on(gui, Row::Submit));
    let (ring, words) = match (busy, enabled, lit) {
        (true, _, _) => (th().dim, accent()),
        (_, false, _) => (th().dim, dim()),
        (_, true, true) => (th().bright, bright_bold()),
        (_, true, false) => (th().accent, accent().add_modifier(Modifier::BOLD)),
    };
    wide_frame(frame, at, label, ring, words);
    if enabled {
        gui.ui.click(at, tor(TorAct::Submit));
    } else if !busy {
        gui.ui.tip(at, primary_why(gui));
    }
}

/// The primary where the room is too short for the tall one: a one-row
/// slab at the title row's right end — ACCENT ground, its label BOLD;
/// off, DIM words and no ground; out, ACCENT words. Returns where it
/// starts.
fn draw_primary_slab(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, label: &str, enabled: bool, busy: bool) -> u16 {
    let words = format!(" {label} ");
    let at = Rect { x: block.right().saturating_sub(width(&words) as u16), y, width: width(&words) as u16, height: 1 };
    let style = match (busy, enabled) {
        (true, _) => accent(),
        (_, false) => dim(),
        _ if gui.ui.hovers(at) || hand_on(gui, Row::Submit) => {
            Style::default().bg(th().bright).fg(th().on_accent).add_modifier(Modifier::BOLD)
        }
        _ => sel().add_modifier(Modifier::BOLD),
    };
    put(frame, at.x, y, &words, style);
    if enabled {
        gui.ui.click(at, tor(TorAct::Submit));
    } else if !busy {
        gui.ui.tip(at, primary_why(gui));
    }
    at.x
}

/// What was sent, under the form: its header, DIM capitals; then a row
/// each, newest first — ✓, the folder, the folder it sits in, when — as
/// many as the room has rows for, the rest under the wheel and the keys.
/// A row's click or Enter opens its folder in Files.
fn draw_list(frame: &mut Frame, gui: &mut Gui, rect: Rect) {
    let (x, end) = (rect.x, rect.right());
    let w = usize::from(rect.width);
    put(frame, x, rect.y, &clip(&t!("gui.tor.list_title").to_uppercase(), w), dim());
    let len = gui.torrent.receipts.len();
    let held = match gui.torrent.cursor {
        Some(Row::Receipt(i)) if !queue_has_keys(gui) => Some(i.min(len.saturating_sub(1))),
        _ => None,
    };
    let (first, visible) = gui.torrent.list.window(len, held, usize::from(rect.height.saturating_sub(1)));
    // The folder and the when share 22 cells at the floor and one more
    // for every four the block grows; the title has the rest (Add from
    // YouTube's columns).
    let zone = 22 + w.saturating_sub(47) / 4;
    let title_w = w.saturating_sub(3 + zone).max(8);
    let now = gui.ytdl.now();
    for (i, y) in (first..first + visible).zip(rect.y + 1..) {
        let Some(receipt) = gui.torrent.receipts.get(i).cloned() else { break };
        let row = Rect { x, y, width: rect.width, height: 1 };
        let on = held == Some(i);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let base = |style: Style| if on { sel() } else { style };
        put(frame, x, y, keep::tick(), base(ok()));
        let title_style = match (on, gui.ui.hovers(row)) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, x + 2, y, &clip(&receipt.title(), title_w), title_style);
        let when = super::ytdl::ago(gui, receipt.at, now);
        put(frame, end.saturating_sub(width(&when) as u16), y, &when, base(dim()));
        let px = x + 2 + title_w as u16 + 1;
        let room = usize::from(end.saturating_sub(width(&when) as u16 + 1).saturating_sub(px));
        if room >= 3 {
            put(frame, px, y, &clip(&receipt.parent(), room), base(dim()));
        }
        gui.ui.click(row, tor(TorAct::Receipt(i)));
    }
}

// ── The sheets ──────────────────────────────────────────────────────────────

/// How many places Files already exist shows before the fresh download.
const MATCHES_SHOWN: usize = 5;
/// The typed picker's suggestions on screen at once.
const PICKER_ROWS: usize = 6;

/// What stands over the room: the Metadata or Destination sheet, Files
/// already exist, the typed picker — and Torrent received, which can be
/// up over any room. Each is drawn (and registered) after what it stands
/// over, so its rects win the pointer.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    match gui.torrent.sheet {
        Some(Sheet::Metadata(stop)) => draw_metadata(frame, gui, area, stop),
        Some(Sheet::Destination(stop)) => draw_destination(frame, gui, area, stop),
        None => {}
    }
    if gui.torrent.matches.is_some() {
        draw_matches(frame, gui, area);
    }
    if gui.torrent.picker.is_some() {
        draw_picker(frame, gui, area);
    }
    if gui.torrent.chooser.is_some() {
        draw_chooser(frame, gui, area);
    }
}

/// A sheet's `[ Done ]`-like button, BRIGHT where the hand is on it.
fn sheet_button(frame: &mut Frame, gui: &mut Gui, rect: Rect, y: u16, words: &str, hand: bool, act: TorAct) {
    let at = bracket_button(frame, gui, rect.right().saturating_sub(3), y, words, true, tor(act));
    if hand {
        put(frame, at, y, &format!("[ {words} ]"), bright_bold());
    }
}

/// The Metadata sheet (the canvas's Metadata board): ARTIST, ALBUM and
/// YEAR over three boxes side by side; how the last auto-detect came out;
/// `Auto-detect metadata`, where the server can be asked; `[ Done ]`.
/// The boxes edit the form as they are typed, and the destination with
/// it.
fn draw_metadata(frame: &mut Frame, gui: &mut Gui, area: Rect, stop: MetaStop) {
    let room = super::ytdl_sheets::sheet_room(gui, area);
    let lines = if gui.torrent.busy == Some(Busy::Detecting) {
        vec![(t!("gui.tor.detecting").to_string(), accent())]
    } else {
        detect_lines(gui, room)
    };
    let said = lines.len().clamp(1, 3) as u16;
    let rows = 4 + 1 + said + 1 + 1;
    let rect = open_sheet(frame, gui, area, rows, tor(TorAct::SheetClose));
    keep::border_title(frame, rect, &t!("gui.tor.metadata"));
    let (x, total) = sheet_text(rect);
    let year_w = 9.min(total);
    let rest = total.saturating_sub(year_w + 4);
    let album_w = rest * 3 / 7;
    let artist_w = rest - album_w;
    let cards = [
        (MetaStop::Artist, t!("gui.tor.artist"), Rect { x, y: rect.y + 2, width: artist_w, height: 3 }),
        (MetaStop::Album, t!("gui.tor.album"), Rect { x: x + artist_w + 2, y: rect.y + 2, width: album_w, height: 3 }),
        (MetaStop::Year, t!("gui.tor.year"), Rect { x: x + artist_w + album_w + 4, y: rect.y + 2, width: year_w, height: 3 }),
    ];
    for (field, label, card) in cards {
        put(frame, card.x, card.y - 1, &clip(&label, usize::from(card.width)), dim());
        let input = match field {
            MetaStop::Artist => &gui.torrent.artist,
            MetaStop::Album => &gui.torrent.album,
            _ => &gui.torrent.year,
        };
        let (value, cursor) = (input.value().to_string(), input.cursor());
        field_card(frame, gui, card, &value, cursor, stop == field, tor(TorAct::OpenMetadata(field)));
    }
    for (y, (line, style)) in (rect.y + 6..).zip(lines.into_iter().take(3)) {
        put(frame, x, y, &clip(&line, usize::from(total)), style);
    }
    let foot = rect.y + rows;
    if gui.torrent.file.is_some() && gui.torrent.gate.ok() && gui.torrent.busy.is_none() {
        let label = way_label(Row::Detect);
        let at = Rect { x, y: foot, width: width(&label) as u16, height: 1 };
        put(frame, x, foot, &label, word_style(stop == MetaStop::Detect, gui.ui.hovers(at), Style::default()));
        gui.ui.click(at, tor(TorAct::Detect));
    }
    sheet_button(frame, gui, rect, foot, &t!("gui.ytdl.done"), stop == MetaStop::Done, TorAct::SheetClose);
}

/// The libraries as radios, laid from the left three cells apart: as many
/// a row as fit `total`.
fn library_rows(libraries: &[String], total: usize) -> Vec<Vec<usize>> {
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut used = 0;
    for (i, name) in libraries.iter().enumerate() {
        let cells = 4 + width(name);
        match rows.last_mut() {
            Some(row) if used + 3 + cells <= total => {
                row.push(i);
                used += 3 + cells;
            }
            _ => {
                rows.push(vec![i]);
                used = cells;
            }
        }
    }
    rows
}

/// The Destination sheet (the canvas's Destination board): the library,
/// where the server has more than one; the PATH's box, the real landing
/// spot under it (clause 14) and how a hand edit behaves; `Rename the
/// torrent's root folder` with what it does; `[ Done ]`. Its title wears
/// the place. It edits the form as it is used.
fn draw_destination(frame: &mut Frame, gui: &mut Gui, area: Rect, stop: DestStop) {
    let libraries = gui.app.libraries.clone();
    let radios = if libraries.len() > 1 { library_rows(&libraries, super::ytdl_sheets::sheet_room(gui, area)) } else { Vec::new() };
    let top = if radios.is_empty() { 0 } else { radios.len() as u16 + 2 };
    let rows = top + 11;
    let rect = open_sheet(frame, gui, area, rows, tor(TorAct::SheetClose));
    let crumbs = dest_crumbs(gui);
    sheet_title(frame, rect, &t!("gui.tor.destination"), |frame, x, room| {
        let shown = fit_dest(&crumbs, room);
        draw_crumbs(frame, x, rect.y, &shown, Style::default())
    });
    let (x, total) = sheet_text(rect);
    let mut y = rect.y + 1;
    if !radios.is_empty() {
        put(frame, x, y, &clip(&t!("gui.tor.library"), usize::from(total)), dim());
        let (dot_on, dot_off) = super::ytdl::dots();
        for row in &radios {
            y += 1;
            let mut at = x;
            for &i in row {
                let name = &libraries[i];
                let on = i == gui.torrent.vpath;
                let radio = Rect { x: at, y, width: 4 + width(name) as u16, height: 1 };
                put(frame, at, y, if on { dot_on } else { dot_off }, if on { ok() } else { dim() });
                let rest = if on { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
                put(frame, at + 4, y, name, word_style(on && stop == DestStop::Library, gui.ui.hovers(radio), rest));
                gui.ui.click(radio, tor(TorAct::Library(i)));
                at += radio.width + 3;
            }
        }
        y += 2;
    }
    put(frame, x, y, &clip(&t!("gui.tor.path"), usize::from(total)), dim());
    let card = Rect { x, y: y + 1, width: total, height: 3 };
    let (value, cursor) = (gui.torrent.path.value().to_string(), gui.torrent.path.cursor());
    field_card(frame, gui, card, &value, cursor, stop == DestStop::Path, tor(TorAct::OpenDestination(DestStop::Path)));
    y += 4;
    // The preview (clause 14): the real landing spot.
    let path = value.trim().trim_end_matches('/').to_string();
    let (open, close) = super::angle_glyphs();
    let preview = match library(gui) {
        Some(vpath) => format!("/{vpath}/{path}/{open}{}{close}", t!("gui.tor.preview_contents")),
        None => format!("{open}{}{close}/{path}", t!("gui.tor.preview_no_library")),
    };
    put(frame, x, y, &super::clip_lead(&preview, usize::from(total)), dim());
    put(frame, x, y + 1, &clip(&t!("gui.tor.path_tip"), usize::from(total)), dim());
    y += 3;
    let (check_on, check_off) = super::check_glyphs();
    let on = gui.torrent.rename_root;
    let label = clip(&t!("gui.tor.rename_root"), usize::from(total.saturating_sub(4)));
    let check = Rect { x, y, width: 4 + width(&label) as u16, height: 2 };
    put(frame, x, y, if on { check_on } else { check_off }, if on { ok() } else { dim() });
    put(frame, x + 4, y, &label, word_style(stop == DestStop::Rename, gui.ui.hovers(check), Style::default()));
    put(frame, x + 4, y + 1, &clip(&t!("gui.tor.rename_root_desc"), usize::from(total.saturating_sub(4))), dim());
    gui.ui.click(check, tor(TorAct::ToggleRename));
    sheet_button(frame, gui, rect, rect.y + rows, &t!("gui.ytdl.done"), stop == DestStop::Done, TorAct::SheetClose);
}

/// Files already exist (clause 21; the canvas's Matches board): what the
/// sheet is for; each place some of the files already live, with how many
/// are there and how many would download; `Download fresh anyway`, at the
/// destination typed; `[ Add torrent ]`. The dot is the choice in the
/// making — ACCENT, its place BOLD: a click moves it, the button or Enter
/// takes it.
fn draw_matches(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(matches) = gui.torrent.matches.clone() else { return };
    let body = crate::kit::wrap_whole(&t!("gui.tor.partial_body"), super::ytdl_sheets::sheet_room(gui, area).max(1));
    let shown = matches.list.len().min(MATCHES_SHOWN);
    let rows = body.len() as u16 + 2 * shown as u16 + 5;
    let rect = open_sheet(frame, gui, area, rows, tor(TorAct::MatchClose));
    keep::border_title(frame, rect, &t!("gui.tor.partial_title"));
    let (x, total) = sheet_text(rect);
    for (y, line) in (rect.y + 1..).zip(&body) {
        put(frame, x, y, line, dim());
    }
    let room = usize::from(total.saturating_sub(4));
    let (dot_on, dot_off) = super::ytdl::dots();
    let dot = matches.row.min(shown);
    let mut y = rect.y + 2 + body.len() as u16;
    for i in 0..=shown {
        let place = Rect { x, y, width: total, height: 2 };
        let on = i == dot;
        put(frame, x, y, if on { dot_on } else { dot_off }, if on { accent() } else { dim() });
        let rest = if on { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
        let style = if gui.ui.hovers(place) { bright_bold() } else { rest };
        match matches.list.get(i).filter(|_| i < shown) {
            Some(m) => {
                let crumbs: Vec<String> = std::iter::once(m.vpath.clone()).chain(path_folders(&m.relative_path)).collect();
                draw_crumbs(frame, x + 4, y, &fit_dest(&crumbs, room), style);
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
                put(frame, x + 4, y + 1, &clip(&detail, room), dim());
            }
            None => {
                put(frame, x + 4, y, &clip(&t!("gui.tor.download_fresh"), room), style);
                draw_crumbs(frame, x + 4, y + 1, &fit_dest(&dest_crumbs(gui), room), dim());
            }
        }
        gui.ui.click(place, tor(TorAct::MatchPick(i)));
        y += 2;
    }
    sheet_button(frame, gui, rect, rect.y + rows, &t!("gui.tor.submit"), false, TorAct::MatchAdd);
}

/// Torrent received (entry point 2; the canvas's Received board): what
/// arrived, BOLD; the question; `Add to mStream ▸`, the primary, and
/// `Open in another app` beside it — one over the other where a
/// language's words are wider than half the sheet; then the don't-ask
/// box. Enter adds until the hand is picked up.
fn draw_chooser(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // Read off the chooser first: what arrived can be a whole .torrent's
    // bytes, and a clone per frame copied them to draw a name.
    let Some((what, hand, dont_ask)) = gui.torrent.chooser.as_ref().map(|c| {
        let what = match &c.incoming {
            Incoming::File(f) => f.name.clone(),
            Incoming::Magnet(link) => meta::magnet_display_name(link).unwrap_or_else(|| t!("gui.tor.magnet_link").to_string()),
        };
        (what, c.row, c.dont_ask)
    }) else {
        return;
    };
    let room = super::ytdl_sheets::sheet_room(gui, area);
    let body: Vec<String> = crate::kit::wrap_whole(&t!("gui.tor.received_body"), room.max(1)).into_iter().take(3).collect();
    let add = format!("{} {}", t!("gui.tor.received_add"), super::forward_glyph());
    let other = t!("gui.tor.open_with").to_string();
    let left = room / 2;
    let right = room.saturating_sub(left + 1);
    let beside = width(&add) + 4 <= left && width(&other) + 4 <= right;
    let rows = 1 + body.len() as u16 + 1 + if beside { 3 } else { 6 } + 2;
    let rect = open_sheet(frame, gui, area, rows, tor(TorAct::ChooseClose));
    keep::border_title(frame, rect, &t!("gui.tor.received_title"));
    let (x, total) = sheet_text(rect);
    put(frame, x, rect.y + 1, &clip(&what, usize::from(total)), Style::default().add_modifier(Modifier::BOLD));
    for (y, line) in (rect.y + 2..).zip(&body) {
        put(frame, x, y, line, dim());
    }
    let top = rect.y + 3 + body.len() as u16;
    let (first, second) = if beside {
        (
            Rect { x, y: top, width: left as u16, height: 3 },
            Rect { x: x + left as u16 + 1, y: top, width: right as u16, height: 3 },
        )
    } else {
        (Rect { x, y: top, width: total, height: 3 }, Rect { x, y: top + 3, width: total, height: 3 })
    };
    let lit = gui.ui.hovers(first) || hand == Some(0);
    let (ring, words) = if lit { (th().bright, bright_bold()) } else { (th().accent, accent().add_modifier(Modifier::BOLD)) };
    wide_frame(frame, first, &add, ring, words);
    gui.ui.click(first, tor(TorAct::ChooseAdd));
    let (ring, words) = match (hand == Some(1), gui.ui.hovers(second)) {
        (true, _) => (th().accent, accent().add_modifier(Modifier::BOLD)),
        (false, true) => (th().bright, bright_bold()),
        (false, false) => (th().dim, Style::default()),
    };
    wide_frame(frame, second, &other, ring, words);
    gui.ui.click(second, tor(TorAct::ChooseHandOff));
    let y = rect.y + rows;
    let (check_on, check_off) = super::check_glyphs();
    let label = clip(&t!("gui.tor.received_dont_ask"), usize::from(total.saturating_sub(4)));
    let check = Rect { x, y, width: 4 + width(&label) as u16, height: 1 };
    put(frame, x, y, if dont_ask { check_on } else { check_off }, if dont_ask { ok() } else { dim() });
    put(frame, x + 4, y, &label, word_style(hand == Some(2), gui.ui.hovers(check), Style::default()));
    gui.ui.click(check, tor(TorAct::ChooseAsk));
}

/// The typed picker (clause 2; the canvas's Picker board): the path's
/// box, and under it the folder it sits in — its sub-folders DIM behind a
/// `▸`, its `.torrent` files TEXT — narrowed as the path is typed. The
/// sheet stands where it would with its list full and grows downward, so
/// the box holds one spot.
fn draw_picker(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(suggestions) = gui.torrent.picker.as_ref().map(Picker::suggestions) else { return };
    let listed = suggestions.len().min(PICKER_ROWS).max(1) as u16;
    let rect = open_sheet_anchored(frame, gui, area, 3 + listed + 1, 3 + PICKER_ROWS as u16 + 1, tor(TorAct::PickerClose));
    keep::border_title(frame, rect, &t!("gui.tor.picker_title"));
    let (x, total) = sheet_text(rect);
    let card = Rect { x, y: rect.y + 1, width: total, height: 3 };
    let (value, cursor) = gui.torrent.picker.as_ref().map(|p| (p.text.value().to_string(), p.text.cursor())).unwrap_or_default();
    field_card(frame, gui, card, &value, cursor, true, Act::Guard);
    let top = rect.y + 4;
    // The picker draws from its listing in place: that is every entry of
    // the folder, and a clone per frame copied them all to show six.
    let Gui { torrent, ui, .. } = &mut *gui;
    let Some(picker) = torrent.picker.as_mut() else { return };
    let sel_moved = picker.sel != picker.sel_anchor;
    let reveal = if sel_moved { picker.sel } else { None };
    let (first, visible) = table_view(suggestions.len(), reveal, picker.scroll, PICKER_ROWS);
    picker.scroll = first;
    picker.sel_anchor = picker.sel;
    let overflow = suggestions.len() > visible;
    let row_width = if overflow { total.saturating_sub(1) } else { total };
    let forward = super::forward_glyph();
    for (row, i) in (first..first + visible).enumerate() {
        let entry = &picker.entries[suggestions[i]];
        let selected = picker.sel == Some(i);
        let rect = Rect { x, y: top + row as u16, width: row_width, height: 1 };
        let style = match (selected, ui.hovers(rect), entry.dir) {
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
        if selected {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        put(frame, rect.x, rect.y, &clip(&text, usize::from(row_width)), style);
        ui.click(rect, tor(TorAct::PickerSuggest(i)));
    }
    scroll_list(
        frame,
        ui,
        Rect { x: x + total.saturating_sub(1), y: top, width: 1, height: visible as u16 },
        suggestions.len(),
        visible,
        first,
        tor(TorAct::PickerScrollBy(-1)),
        tor(TorAct::PickerScrollBy(1)),
        |first| tor(TorAct::PickerScrollTo(first)),
    );
    if let Some(err) = &picker.error {
        put(frame, x, top, &clip(err, usize::from(total)), gold());
    } else if suggestions.is_empty() && !picker.listed_for.is_empty() {
        put(frame, x, top, &clip(&t!("gui.tor.picker_empty"), usize::from(total)), dim());
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The ask-me switch (clause 51), persisted the servers room's way and
/// mirrored in memory whatever the disk said.
pub(crate) fn set_ask(gui: &mut Gui, ask: bool) {
    super::servers::update_config(gui, |config| config.torrent.ask = ask);
    gui.config.torrent.ask = ask;
}

/// The library chosen: the destination goes by its layout from here on.
fn set_library(gui: &mut Gui, vpath: usize) {
    if vpath < gui.app.libraries.len() {
        gui.torrent.vpath = vpath;
        gui.torrent.refusal = None;
        recompute_path(gui);
    }
}

/// Move the hand along the stops that exist right now.
fn move_cursor(gui: &mut Gui, delta: i32) {
    let rows = gui.torrent.rows(gui.torrent.gate.ok());
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
    if matches!(rows[next], Row::Receipt(_)) {
        gui.torrent.list.pick_up();
    }
}

/// What Enter does on a stop: its control's own act.
fn stop_act(row: Row) -> Option<TorAct> {
    Some(match row {
        Row::File => TorAct::Pick,
        Row::TypePath => TorAct::Type,
        Row::Detect => TorAct::Detect,
        Row::HandOff => TorAct::HandOff,
        Row::Metadata => TorAct::OpenMetadata(MetaStop::Artist),
        Row::Destination => TorAct::OpenDestination(DestStop::Path),
        Row::Force => TorAct::ToggleForce,
        Row::Submit => TorAct::Submit,
        Row::Receipt(i) => TorAct::Receipt(i),
        Row::Magnet => return None,
    })
}

/// Handle the room's acts. Returns true when the act was this room's.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Tor(act) = act else { return false };
    match act.clone() {
        TorAct::Stop(row) => gui.torrent.cursor = Some(row),
        TorAct::Pick => native_pick(gui),
        TorAct::Type => open_picker(gui),
        TorAct::Unload => {
            gui.torrent.file = None;
            gui.torrent.facts = None;
            gui.torrent.force_fresh = false;
            gui.torrent.refusal = None;
            gui.torrent.detect = None;
            gui.torrent.cursor = None;
        }
        TorAct::ClearLink => {
            gui.torrent.magnet = Input::default();
            magnet_changed(gui);
            gui.torrent.cursor = Some(Row::Magnet);
        }
        TorAct::PasteClip => {
            if let Some(text) = gui.ytdl.clipboard.and_then(|read| read()) {
                paste(gui, &text);
            }
        }
        TorAct::HandOff => hand_off(gui, None),
        TorAct::Detect => auto_detect(gui),
        TorAct::OpenMetadata(stop) => gui.torrent.sheet = Some(Sheet::Metadata(stop)),
        TorAct::OpenDestination(stop) => gui.torrent.sheet = Some(Sheet::Destination(stop)),
        TorAct::SheetClose => gui.torrent.sheet = None,
        TorAct::Library(vpath) => {
            set_library(gui, vpath);
            if gui.torrent.sheet.is_some() {
                gui.torrent.sheet = Some(Sheet::Destination(DestStop::Library));
            }
        }
        TorAct::ToggleRename => gui.torrent.rename_root = !gui.torrent.rename_root,
        TorAct::ToggleForce => gui.torrent.force_fresh = !gui.torrent.force_fresh,
        TorAct::Submit => submit(gui),
        TorAct::PickerClose => gui.torrent.picker = None,
        TorAct::PickerSuggest(i) => picker_accept(gui, i),
        TorAct::PickerScrollBy(delta) => {
            if let Some(picker) = gui.torrent.picker.as_mut() {
                picker.scroll = if delta < 0 { picker.scroll.saturating_sub(1) } else { picker.scroll + 1 };
            }
        }
        TorAct::PickerScrollTo(first) => {
            if let Some(picker) = gui.torrent.picker.as_mut() {
                picker.scroll = first;
            }
        }
        TorAct::MatchPick(row) => {
            if let Some(matches) = gui.torrent.matches.as_mut() {
                matches.row = row;
            }
        }
        TorAct::MatchAdd => {
            let Some(matches) = gui.torrent.matches.as_ref() else { return true };
            let shown = matches.list.len().min(MATCHES_SHOWN);
            if matches.row < shown {
                use_match(gui, matches.row);
            } else {
                gui.torrent.matches = None;
                add_at_typed_path(gui);
            }
        }
        TorAct::MatchClose => gui.torrent.matches = None,
        TorAct::ChooseAdd => {
            if let Some(chooser) = gui.torrent.chooser.take() {
                if chooser.dont_ask {
                    set_ask(gui, false);
                }
                open_room_with(gui, chooser.incoming);
            }
        }
        TorAct::ChooseHandOff => {
            if let Some(chooser) = gui.torrent.chooser.take() {
                hand_off(gui, Some(chooser.incoming));
            }
        }
        TorAct::ChooseAsk => {
            if let Some(chooser) = gui.torrent.chooser.as_mut() {
                chooser.dont_ask = !chooser.dont_ask;
                chooser.row = Some(2);
            }
        }
        TorAct::ChooseClose => gui.torrent.chooser = None,
        TorAct::Receipt(i) => open_receipt(gui, i),
        TorAct::Forget(i) => {
            if i < gui.torrent.receipts.len() {
                gui.torrent.receipts.remove(i);
                save_receipts(gui);
                let left = gui.torrent.receipts.len();
                gui.torrent.cursor = match left {
                    0 => None,
                    _ => Some(Row::Receipt(i.min(left - 1))),
                };
            }
        }
        TorAct::ScrollBy(delta) => gui.torrent.list.step(delta),
    }
    true
}

/// The room's wheel: the typed picker's suggestions while it is up (the
/// hub hands it over before a modal swallows it), else what was sent.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if gui.torrent.picker.is_some() {
        gui.act(tor(TorAct::PickerScrollBy(delta)));
    } else if !gui.torrent.modal_open() {
        gui.act(tor(TorAct::ScrollBy(delta)));
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The letters the room's controls use: their capitals are the room's
/// too (Shift or Caps Lock on `d`, `t` would open Auto DJ or Stats).
const ROOM_LETTERS: &[char] = &['b', 'd', 'e', 'l', 'o', 't', 'x'];

/// A text box's key: the line editor's. True when the value changed.
fn typed(input: &mut Input, key: KeyEvent) -> bool {
    input.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value)
}

/// The Metadata sheet's keys: Tab and ↓, Shift-Tab and ↑ walk its stops;
/// Enter closes it from a box (the form is edited as it is typed), and
/// does its control's act on the two under them; Esc closes it.
fn metadata_key(gui: &mut Gui, stop: MetaStop, key: KeyEvent) {
    let detect = gui.torrent.file.is_some() && gui.torrent.gate.ok();
    let mut stops = vec![MetaStop::Artist, MetaStop::Album, MetaStop::Year];
    if detect {
        stops.push(MetaStop::Detect);
    }
    stops.push(MetaStop::Done);
    let at = stops.iter().position(|s| *s == stop).unwrap_or(0);
    match key.code {
        KeyCode::Esc => gui.torrent.sheet = None,
        KeyCode::Up | KeyCode::BackTab => gui.torrent.sheet = Some(Sheet::Metadata(stops[at.saturating_sub(1)])),
        KeyCode::Down | KeyCode::Tab => gui.torrent.sheet = Some(Sheet::Metadata(stops[(at + 1).min(stops.len() - 1)])),
        KeyCode::Enter if stop == MetaStop::Detect => auto_detect(gui),
        KeyCode::Enter => gui.torrent.sheet = None,
        _ => {
            let input = match stop {
                MetaStop::Artist => &mut gui.torrent.artist,
                MetaStop::Album => &mut gui.torrent.album,
                MetaStop::Year => &mut gui.torrent.year,
                _ => return,
            };
            if typed(input, key) {
                gui.torrent.refusal = None;
                recompute_path(gui);
            }
        }
    }
}

/// The Destination sheet's keys: Tab and ↓, Shift-Tab and ↑ walk its
/// stops; ← → step the library; the PATH's box takes what is typed — a
/// hand edit sticks (clause 13); Space ticks the box; Enter closes it
/// from anywhere but the checkbox; Esc closes it.
fn destination_key(gui: &mut Gui, stop: DestStop, key: KeyEvent) {
    let mut stops = Vec::new();
    if gui.app.libraries.len() > 1 {
        stops.push(DestStop::Library);
    }
    stops.extend([DestStop::Path, DestStop::Rename, DestStop::Done]);
    let at = stops.iter().position(|s| *s == stop).unwrap_or(0);
    let libraries = gui.app.libraries.len();
    match (key.code, stop) {
        (KeyCode::Esc, _) => gui.torrent.sheet = None,
        (KeyCode::Up | KeyCode::BackTab, _) => gui.torrent.sheet = Some(Sheet::Destination(stops[at.saturating_sub(1)])),
        (KeyCode::Down | KeyCode::Tab, _) => gui.torrent.sheet = Some(Sheet::Destination(stops[(at + 1).min(stops.len() - 1)])),
        (KeyCode::Left, DestStop::Library) if libraries > 0 => set_library(gui, (gui.torrent.vpath + libraries - 1) % libraries),
        (KeyCode::Right, DestStop::Library) if libraries > 0 => set_library(gui, (gui.torrent.vpath + 1) % libraries),
        (KeyCode::Char(' ') | KeyCode::Enter, DestStop::Rename) => gui.torrent.rename_root = !gui.torrent.rename_root,
        (KeyCode::Enter, _) => gui.torrent.sheet = None,
        (_, DestStop::Path) => {
            if typed(&mut gui.torrent.path, key) {
                gui.torrent.path_edited = true;
                gui.torrent.refusal = None;
            }
        }
        _ => {}
    }
}

/// The room's keys, and its sheets' (which outrank everything — Torrent
/// received can be up over any room). Returns `Some(quit)` when the key
/// was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if let Some(chooser) = gui.torrent.chooser.as_mut() {
        match (key.code, chooser.row) {
            // The first walking key shows the hand, on the primary.
            (KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab, None) => chooser.row = Some(0),
            (KeyCode::Up | KeyCode::BackTab | KeyCode::Left, Some(row)) => chooser.row = Some(row.saturating_sub(1)),
            (KeyCode::Down | KeyCode::Tab | KeyCode::Right, Some(row)) => chooser.row = Some((row + 1).min(2)),
            (KeyCode::Char(' '), _) => return Some(gui.act(tor(TorAct::ChooseAsk))),
            (KeyCode::Enter, row) => {
                let act = match row {
                    Some(1) => TorAct::ChooseHandOff,
                    Some(2) => TorAct::ChooseAsk,
                    _ => TorAct::ChooseAdd,
                };
                return Some(gui.act(tor(act)));
            }
            (KeyCode::Esc, _) => gui.torrent.chooser = None,
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
                if typed(&mut picker.text, key) {
                    picker_refresh(gui);
                }
            }
        }
        return Some(false);
    }
    if let Some(matches) = gui.torrent.matches.as_mut() {
        let last = matches.list.len().min(MATCHES_SHOWN);
        match key.code {
            KeyCode::Up => matches.row = matches.row.min(last).saturating_sub(1),
            KeyCode::Down => matches.row = (matches.row + 1).min(last),
            KeyCode::Enter => return Some(gui.act(tor(TorAct::MatchAdd))),
            KeyCode::Esc => gui.torrent.matches = None,
            _ => {}
        }
        return Some(false);
    }
    match gui.torrent.sheet {
        Some(Sheet::Metadata(stop)) => {
            metadata_key(gui, stop, key);
            return Some(false);
        }
        Some(Sheet::Destination(stop)) => {
            destination_key(gui, stop, key);
            return Some(false);
        }
        None => {}
    }
    // The queue panel holding the keys, the room yields them.
    if !in_room(gui) || queue_has_keys(gui) {
        return None;
    }
    // A check or an add is out: the form's keys wait for the server — a
    // link pasted as keys, where a terminal sends one so, among them.
    let out = matches!(gui.torrent.busy, Some(Busy::Checking | Busy::Adding));
    let ctrl = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let held = gui.torrent.cursor;
    match (held, key.code) {
        (Some(_), KeyCode::Esc) => {
            gui.torrent.cursor = None;
            gui.torrent.list.stow();
        }
        (Some(_), KeyCode::Up | KeyCode::BackTab) => move_cursor(gui, -1),
        (Some(_), KeyCode::Down | KeyCode::Tab) => move_cursor(gui, 1),
        (None, KeyCode::Up) => move_cursor(gui, -1),
        (None, KeyCode::Down) => move_cursor(gui, 1),
        // The link box: Enter moves on — or, where what was typed is no
        // link at all, reads it as the path it must be and loads it; the
        // rest is its line editor's.
        (Some(Row::Magnet), KeyCode::Enter) => {
            let typed = gui.torrent.magnet.value().trim().to_string();
            if typed.is_empty() || typed.to_lowercase().starts_with("magnet:") {
                move_cursor(gui, 1);
            } else if !out {
                gui.torrent.refusal = None;
                picker_load(gui, expand_tilde(&unescape_dropped(&typed)));
            }
        }
        (Some(Row::Magnet), _) if !out => {
            if typed(&mut gui.torrent.magnet, key) {
                magnet_changed(gui);
            }
        }
        (Some(Row::Magnet), _) => {}
        (Some(Row::Receipt(i)), KeyCode::Char('x' | 'X')) => return Some(gui.act(tor(TorAct::Forget(i)))),
        (Some(row), KeyCode::Enter) => {
            return Some(match stop_act(row).filter(|act| !out || matches!(act, TorAct::Receipt(_))) {
                Some(act) => gui.act(tor(act)),
                None => false,
            });
        }
        (Some(Row::Force | Row::Submit), KeyCode::Char(' ')) if !out => {
            let act = if held == Some(Row::Force) { TorAct::ToggleForce } else { TorAct::Submit };
            return Some(gui.act(tor(act)));
        }
        // The hand down, Enter is the primary's wherever it can be pressed.
        (None, KeyCode::Enter) if primary_of(gui).is_some_and(|(_, on, _)| on) => return Some(gui.act(tor(TorAct::Submit))),
        (_, KeyCode::Char(c)) if !ctrl => {
            let lower = c.to_ascii_lowercase();
            let drawn = gui.torrent.drawn.iter().find(|(letter, _)| *letter == lower).map(|(_, act)| act.clone());
            match drawn {
                Some(act) if !out => return Some(gui.act(tor(act))),
                // A press out, every typed key waits with it.
                _ if out => {}
                // A capital of the room's own letters is the room's, drawn
                // or not: it never reaches the shell's globals.
                None if c.is_ascii_uppercase() && ROOM_LETTERS.contains(&lower) => {}
                _ => return None,
            }
        }
        // Esc has nothing to leave: a nav room has no back.
        _ => return None,
    }
    Some(false)
}

/// The tips line for this room's states.
pub(crate) fn tips(gui: &Gui) -> String {
    let rows = t!("gui.tips.ytdl_rows").to_string();
    let enter = |verb: String| t!("gui.tips.ytdl_enter", verb = verb).to_string();
    if gui.torrent.chooser.is_some() {
        return t!("gui.tips.tor_chooser").to_string();
    }
    if gui.torrent.picker.is_some() {
        return t!("gui.tips.tor_picker").to_string();
    }
    if gui.torrent.matches.is_some() {
        return t!("gui.tips.tor_matches").to_string();
    }
    match gui.torrent.sheet {
        Some(Sheet::Metadata(MetaStop::Detect)) => {
            return [enter(way_label(Row::Detect)), rows, t!("gui.tips.ytdl_close").to_string()].join(" · ");
        }
        Some(Sheet::Metadata(MetaStop::Done) | Sheet::Destination(DestStop::Done)) => {
            return [enter(t!("gui.ytdl.done").to_string()), rows, t!("gui.tips.ytdl_close").to_string()].join(" · ");
        }
        Some(Sheet::Metadata(_)) => return t!("gui.tips.ytdl_tags").to_string(),
        Some(Sheet::Destination(DestStop::Library)) => {
            return [t!("gui.tips.ytdl_where_library").to_string(), rows, t!("gui.tips.ytdl_close").to_string()].join(" · ");
        }
        Some(Sheet::Destination(DestStop::Rename)) => {
            return [t!("gui.tips.tor_toggle").to_string(), t!("gui.tips.ytdl_close").to_string()].join(" · ");
        }
        Some(Sheet::Destination(_)) => return t!("gui.tips.ytdl_tags").to_string(),
        None => {}
    }
    let stow = t!("gui.tips.ytdl_stow").to_string();
    match gui.torrent.cursor {
        None if gui.torrent.has_source() => t!("gui.tips.tor_ready").to_string(),
        None => t!("gui.tips.tor_room").to_string(),
        Some(Row::Magnet) => t!("gui.tips.tor_text").to_string(),
        Some(Row::File) => t!("gui.tips.tor_file").to_string(),
        Some(Row::Force) => t!("gui.tips.tor_toggle").to_string(),
        Some(Row::Submit) => t!("gui.tips.tor_submit").to_string(),
        Some(Row::Receipt(_)) => {
            [enter(t!("gui.ytdl.list.show_in_folder").to_string()), t!("gui.tips.ytdl_dismiss").to_string(), rows, stow].join(" · ")
        }
        Some(Row::TypePath) => [enter(t!("gui.tor.type_path").to_string()), rows, stow].join(" · "),
        Some(Row::Detect) => [enter(way_label(Row::Detect)), rows, stow].join(" · "),
        Some(Row::HandOff) => [enter(way_label(Row::HandOff)), rows, stow].join(" · "),
        Some(Row::Metadata) => [enter(t!("gui.tor.metadata").to_string()), rows, stow].join(" · "),
        Some(Row::Destination) => [enter(t!("gui.tor.destination").to_string()), rows, stow].join(" · "),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::ytdl::tests::{START, assert_frame, board_of, buffer, lines};
    use super::*;
    use crate::config::Config;
    use crate::tui::app::App;
    use ratatui::layout::Position;

    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    /// The canvas's torrent, and its link.
    const NAME: &str = "Northlight - Field Notes (2019) [FLAC]";
    const MAGNET: &str =
        "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Northlight+-+Field+Notes+(2019)+%5BFLAC%5D";

    /// English words under assertion: the locale lock, and English pinned —
    /// the locale is process-wide, and the tests that switch it hold the
    /// lock while they draw in another language.
    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    /// A torrent of `files` files, `bytes` all told, bencoded by hand —
    /// with no name of its own where `name` is empty.
    fn album(name: &str, files: u64, bytes: u64) -> Vec<u8> {
        let mut out = b"d8:announce17:http://t.example/4:infod5:filesl".to_vec();
        let each = bytes / files;
        for i in 0..files {
            let length = if i + 1 == files { bytes - each * (files - 1) } else { each };
            out.extend(format!("d6:lengthi{length}e4:pathl6:{:02}.mp3ee", i + 1).into_bytes());
        }
        out.extend(b"e");
        if !name.is_empty() {
            out.extend(format!("4:name{}:{name}", name.len()).into_bytes());
        }
        out.extend(b"12:piece lengthi16384e6:pieces20:");
        out.extend([0u8; 20]);
        out.extend(b"ee");
        out
    }

    fn gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.queue_open = false;
        // Pastes arrive whole, as every platform but a Windows console
        // delivers them: pinned, so the tests read the same on Windows CI.
        // The typed-paste road has its own test.
        gui.ytdl.whole_pastes = true;
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
        gui.ytdl.stop_clock(START);
        gui.act(Act::Nav(super::super::TORRENT_NAV));
        gui
    }

    fn no_clip() -> Option<String> {
        None
    }

    /// What the canvas's lists hold, newest first.
    fn sent(rows: usize) -> Vec<Receipt> {
        [
            ("Perihelion", "Music", "Vela/Perihelion", HOUR),
            ("Cassini", "Music", "Vela/Cassini", 3 * DAY),
            ("Walden", "Audiobooks", "Thoreau/Walden", 6 * DAY),
            ("Nocturnes", "Music", "Halden Row/Nocturnes", 7 * DAY),
            ("Low Tide Songs", "Music", "Marrow & Pine/Low Tide Songs", 7 * DAY + 1),
            ("The Long Field", "Music", "Northlight/The Long Field", 9 * DAY),
            ("Leaves of Grass", "Audiobooks", "Whitman/Leaves of Grass", 13 * DAY),
        ]
        .iter()
        .take(rows)
        .map(|(name, vpath, path, ago)| Receipt { name: name.to_string(), vpath: vpath.to_string(), path: path.to_string(), at: START - ago })
        .collect()
    }

    /// The room as the canvas drew it: the queue open (47 cells), `Paste`
    /// beside the link box, two libraries, three torrents sent before.
    fn canvas() -> Gui {
        let mut gui = ready_gui(&["Music", "Audiobooks"]);
        gui.queue_open = true;
        gui.ytdl.clipboard = Some(no_clip);
        gui.torrent.receipts = sent(3);
        gui
    }

    /// The canvas's torrent, loaded: twelve files, 356 MB.
    fn field_notes(gui: &mut Gui) {
        set_file(gui, LoadedFile { name: format!("{NAME}.torrent"), bytes: album(NAME, 12, 356 * 1024 * 1024) });
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        lines(&buffer(gui, 100, 30))
    }

    /// The room's 47 cells at 100 × 30, the queue open, against a board.
    fn assert_room(gui: &mut Gui, rows: &str, runs: &[&str]) {
        assert_frame(&buffer(gui, 100, 30), (17, 2), 47, rows, runs);
    }

    /// A sheet `h` rows tall over the 100 × 30 frame, against a board.
    fn assert_sheet(gui: &mut Gui, h: u16, rows: &str, runs: &[&str]) {
        assert_frame(&buffer(gui, 100, 30), (17, (30 - h) / 2), 66, rows, runs);
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

    /// Wait for the room's threads to answer (a listing, a file read).
    fn settle(gui: &mut Gui, done: impl Fn(&Gui) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done(gui) && std::time::Instant::now() < deadline {
            poll(gui);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(done(gui), "the room's thread never answered");
    }

    /// The add the form would send, taken as sent: the server's answer is
    /// the test's to give.
    fn sending(gui: &mut Gui) {
        gui.torrent.force_fresh = true;
        let Ok(Plan::Add(req)) = plan_submit(gui) else { panic!("the form plans no add") };
        gui.torrent.sending = Some(receipt_of(&req));
        gui.torrent.busy = Some(Busy::Adding);
    }

    fn added(name: &str) -> Reply {
        Reply::Added(Ok(TorrentAdded { ok: true, name: Some(name.into()), ..Default::default() }))
    }

    // ── The canvas's boards ────────────────────────────────────────────────

    #[test]
    fn a_loaded_torrent_is_a_card_with_its_rows_and_add_under_them() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        assert_room(&mut gui, boards::MAIN, boards::MAIN_RUNS);
        assert_eq!(gui.torrent.facts, Some((12, 356 * 1024 * 1024)), "read from the file itself");
        assert_eq!(tips(&gui), "Enter add the torrent · e metadata · l destination · x let go · ↑ ↓ rows");
    }

    #[test]
    fn the_room_opens_on_the_two_boxes_and_what_was_sent() {
        let _en = english();
        let mut gui = canvas();
        assert_room(&mut gui, boards::EMPTY, boards::EMPTY_RUNS);
        assert_eq!(gui.torrent.cursor, None, "the hand stowed");
        assert_eq!(tips(&gui), "↓ pick a row · b choose a file · t type a path");
    }

    #[test]
    fn after_an_add_the_folder_is_first_in_the_list_and_the_form_starts_over() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        sending(&mut gui);
        apply_reply(&mut gui, added(NAME));
        assert_room(&mut gui, boards::AFTER, boards::AFTER_RUNS);
        assert_eq!(gui.note, Some((format!("Added “{NAME}”"), false)));
        assert_eq!(
            gui.torrent.receipts[0],
            Receipt { name: NAME.into(), vpath: "Music".into(), path: "Northlight/Field Notes".into(), at: START }
        );
        assert!(gui.torrent.file.is_none() && !gui.torrent.force_fresh, "the source gone, the checkbox as it started");
    }

    #[test]
    fn at_the_floor_add_is_the_title_rows_slab_and_the_list_keeps_four_rows() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.receipts = sent(4);
        field_notes(&mut gui);
        assert_frame(&buffer(&mut gui, 100, 24), (17, 2), 47, boards::MIN, boards::MIN_RUNS);
    }

    #[test]
    fn a_wide_window_centres_the_block_at_fifty_six_cells() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.receipts = sent(7);
        field_notes(&mut gui);
        assert_frame(&buffer(&mut gui, 132, 34), (28, 2), 56, boards::WIDE, boards::WIDE_RUNS);
    }

    #[test]
    fn a_pasted_magnet_link_fills_the_link_box_and_shows_its_name() {
        let _en = english();
        let mut gui = canvas();
        assert!(wants_paste(&gui));
        paste(&mut gui, &format!("  {MAGNET}\n"));
        assert_room(&mut gui, boards::MAGNET, boards::MAGNET_RUNS);
        assert_eq!(gui.torrent.cursor, None, "a real link leaves the hand down: Enter adds");
        assert_eq!(
            gui.torrent.rows(true),
            vec![Row::Magnet, Row::Metadata, Row::Destination, Row::Submit, Row::Receipt(0), Row::Receipt(1), Row::Receipt(2)],
            "a link has no files to check, and no ways"
        );
    }

    #[test]
    fn a_server_that_cannot_take_torrents_says_why_and_still_hands_one_on() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.gate = Gate::Answered(TorrentPreflight::default());
        field_notes(&mut gui);
        assert_room(&mut gui, boards::REFUSED, boards::REFUSED_RUNS);
        let rows = draw(&mut gui);
        let at = Position { x: 17 + 20, y: 2 + 13 };
        assert!(rows[15].contains("Add torrent"), "{}", rows[15]);
        assert_eq!(gui.ui.hit(at), None, "the primary is off: no click");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.torrent.busy.is_none(), "and Enter adds nothing");

        // The server's own reason, where it gives one — whole, on as many
        // rows as it needs, the card under them.
        let reason = "Torrents are turned off: no client is connected, ask the admin to set one up on the Torrent page.";
        gui.torrent.gate = Gate::Answered(TorrentPreflight { reason: Some(reason.into()), ..Default::default() });
        let rows = draw(&mut gui);
        let said: String = rows[3..6].iter().map(|r| r.chars().skip(17).take(47).collect::<String>().trim().to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(said, reason);
        assert_eq!(rows[6].chars().nth(17), Some('╭'), "the card under its three rows:\n{}", rows[6]);
    }

    #[test]
    fn a_link_that_is_no_magnet_is_marked_under_the_link_box() {
        let _en = english();
        let mut gui = canvas();
        gui.act(tor(TorAct::Stop(Row::Magnet)));
        type_text(&mut gui, "magnet:?xt=urn:btih:abc");
        assert_room(&mut gui, boards::BADLINK, boards::BADLINK_RUNS);
        assert!(!gui.torrent.has_source(), "a half-typed link is not a source");
        assert_eq!(tips(&gui), "type · ↑ ↓ rows · Tab next · Enter next · Esc done");

        // A bad link pasted puts the hand on the box that mends it.
        gui.torrent.cursor = None;
        paste(&mut gui, "magnet:?dn=nothing");
        assert_eq!(gui.torrent.cursor, Some(Row::Magnet));
    }

    #[test]
    fn a_name_that_says_nothing_points_at_the_details_and_add_waits() {
        let _en = english();
        let mut gui = canvas();
        // A torrent that names nothing: neither artist nor album to go by.
        set_file(&mut gui, LoadedFile { name: "download (3).torrent".into(), bytes: album("", 9, 88 * 1024 * 1024) });
        assert_room(&mut gui, boards::NODETAILS, boards::NODETAILS_RUNS);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.torrent.busy.is_none() && gui.torrent.refusal.is_none(), "Enter is not the primary's while it is off");

        // The details typed, the destination follows and Add is on.
        key(&mut gui, KeyCode::Char('e'));
        assert_eq!(gui.torrent.sheet, Some(Sheet::Metadata(MetaStop::Artist)));
        type_text(&mut gui, "Vela");
        key(&mut gui, KeyCode::Tab);
        type_text(&mut gui, "Cassini");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.sheet, None);
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Music › Vela › Cassini") && text.contains("Add torrent ▸"), "{text}");
    }

    #[test]
    fn a_best_effort_guess_says_so_under_the_cards_ways() {
        let _en = english();
        let mut gui = canvas();
        set_file(
            &mut gui,
            LoadedFile { name: "nl-fieldnotes-2019-web-flac.torrent".into(), bytes: album("nl-fieldnotes-2019-web-flac", 12, 356 * 1024 * 1024) },
        );
        gui.torrent.busy = Some(Busy::Detecting);
        let rows = draw(&mut gui);
        assert!(rows[8].chars().skip(17).collect::<String>().starts_with("Detecting… · Open in another app"), "{}", rows[8]);
        apply_reply(
            &mut gui,
            Reply::Detected(Ok(TorrentDetect {
                ok: true,
                confidence: Some("low".into()),
                metadata: Some(TorrentDetectMeta {
                    artist: Some(serde_json::json!("Northlight")),
                    album: Some(serde_json::json!("Field Notes")),
                    year: Some(serde_json::json!(2019)),
                }),
                message: None,
            })),
        );
        assert_room(&mut gui, boards::GUESS, boards::GUESS_RUNS);

        // A sure answer says so in the OK colour; nothing found, the
        // server's words in GOLD; a new file starts clean.
        gui.torrent.busy = Some(Busy::Detecting);
        apply_reply(&mut gui, Reply::Detected(Ok(TorrentDetect { ok: true, confidence: Some("high".into()), ..Default::default() })));
        assert_eq!(gui.torrent.detect, Some(("Metadata detected".into(), Tone::Ok)));
        gui.torrent.busy = Some(Busy::Detecting);
        apply_reply(&mut gui, Reply::Detected(Ok(TorrentDetect::default())));
        assert_eq!(gui.torrent.detect, Some(("Not enough metadata — fill the fields in manually".into(), Tone::Gold)));
        let rows = draw(&mut gui);
        assert!(rows[9].contains("! Not enough metadata — fill the fields in") && rows[10].contains("  manually"), "wrapped:\n{}\n{}", rows[9], rows[10]);
        field_notes(&mut gui);
        assert_eq!(gui.torrent.detect, None);
    }

    #[test]
    fn while_an_add_is_out_the_form_stands_and_nothing_on_it_acts() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        gui.torrent.busy = Some(Busy::Checking);
        assert_room(&mut gui, boards::ADDING, boards::ADDING_RUNS);
        // No click on the form, and its letters wait with it.
        for (x, y) in [(20, 5), (20, 8), (20, 10), (20, 11), (20, 12), (30, 15)] {
            let hit = gui.ui.hit(Position { x, y });
            assert_eq!(hit, None, "a click at {x},{y}");
        }
        for c in ['x', 'o', 'd', 'e', 'l', 'b'] {
            key(&mut gui, KeyCode::Char(c));
        }
        key(&mut gui, KeyCode::Enter);
        assert!(gui.torrent.file.is_some() && gui.torrent.sheet.is_none() && gui.torrent.busy == Some(Busy::Checking));
        assert!(!gui.torrent.modal_open());
        // A whole paste waits too; the list keeps its click.
        paste(&mut gui, MAGNET);
        assert!(gui.torrent.file.is_some() && gui.torrent.magnet.value().is_empty());
        assert_eq!(gui.ui.hit(Position { x: 20, y: 20 }), Some(tor(TorAct::Receipt(0))));
    }

    #[test]
    fn a_refusal_keeps_the_form_and_stands_under_add_in_the_servers_words() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        gui.torrent.busy = Some(Busy::Checking);
        apply_reply(
            &mut gui,
            Reply::Checked(Ok(SeedCheck { ok: false, outcome: "match_unmapped".into(), vpath: Some("Music".into()), ..Default::default() })),
        );
        assert_room(&mut gui, boards::FAILED, boards::FAILED_RUNS);
        assert_eq!(gui.note, None, "never the one-row note");
        assert!(gui.torrent.file.is_some(), "the form stands");

        // The longest of them, whole, on as many rows as it needs.
        gui.torrent.busy = Some(Busy::Checking);
        apply_reply(
            &mut gui,
            Reply::Checked(Ok(SeedCheck {
                ok: false,
                outcome: "pad_files_missing".into(),
                vpath: Some("Music".into()),
                client_type: Some("Transmission".into()),
                ..Default::default()
            })),
        );
        let said: String =
            draw(&mut gui)[17..].iter().map(|row| row.chars().skip(17).take(47).collect::<String>().trim().to_string()).collect::<Vec<_>>().join(" ");
        let whole = t!("gui.tor.pad_files_missing", vpath = "Music", client = "Transmission").to_string();
        assert!(said.starts_with(&whole), "every word of it:\n{said}");

        // It goes when the form changes, or the next press goes out.
        key(&mut gui, KeyCode::Char('l'));
        type_text(&mut gui, "x");
        assert_eq!(gui.torrent.refusal, None, "an edit");
        key(&mut gui, KeyCode::Esc);
        refuse(&mut gui, "no");
        gui.act(tor(TorAct::Unload));
        assert_eq!(gui.torrent.refusal, None, "the file let go");

        // The add's own failure, and a source that could not be read.
        field_notes(&mut gui);
        sending(&mut gui);
        apply_reply(&mut gui, Reply::Added(Err("412 Precondition Failed".into())));
        assert_eq!(gui.torrent.refusal.as_deref(), Some("couldn't add the torrent: 412 Precondition Failed"));
        assert_eq!(gui.torrent.sending, None);
        assert_eq!(gui.torrent.receipts.len(), 3, "nothing was sent");
        gui.act(tor(TorAct::Unload));
        apply_reply(&mut gui, Reply::Loaded { path: "/tmp/song.mp3".into(), result: Ok(Loaded::File(b"ID3".to_vec())) });
        let rows = draw(&mut gui);
        assert!(rows[12].chars().skip(17).collect::<String>().starts_with("“song.mp3” is not a .torrent file"), "under the two boxes:\n{}", rows[12]);
    }

    #[test]
    fn metadata_is_a_sheet_of_three_boxes_edited_as_they_are_typed() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        gui.torrent.detect = Some((t!("gui.tor.detected").to_string(), Tone::Ok));
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        assert_sheet(&mut gui, 10, boards::METADATA, boards::METADATA_RUNS);
        assert_eq!(tips(&gui), "type · Tab ↑ ↓ tags · Enter done · Esc close");
        assert!(!wants_paste(&gui), "a paste is the box's now, typed");

        // Typed, the destination follows; the stops walk; Esc closes.
        type_text(&mut gui, " Trio");
        assert_eq!((gui.torrent.artist.value(), gui.torrent.path.value()), ("Northlight Trio", "Northlight Trio/Field Notes"));
        for stop in [MetaStop::Album, MetaStop::Year, MetaStop::Detect, MetaStop::Done, MetaStop::Done] {
            key(&mut gui, KeyCode::Tab);
            assert_eq!(gui.torrent.sheet, Some(Sheet::Metadata(stop)));
        }
        assert_eq!(tips(&gui), "Enter Done · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::BackTab);
        assert_eq!(tips(&gui), "Enter Auto-detect metadata · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.torrent.sheet, None);

        // A link has no file for the server to read: no Auto-detect.
        gui.act(tor(TorAct::Unload));
        paste(&mut gui, MAGNET);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("─ Metadata ─") && !text.contains("Auto-detect metadata"), "{text}");
        for stop in [MetaStop::Album, MetaStop::Year, MetaStop::Done] {
            key(&mut gui, KeyCode::Down);
            assert_eq!(gui.torrent.sheet, Some(Sheet::Metadata(stop)));
        }
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.sheet, None, "Enter on Done closes");
    }

    #[test]
    fn destination_is_a_sheet_the_library_first_and_a_hand_edit_sticks() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('l'));
        assert_sheet(&mut gui, 16, boards::DESTINATION, boards::DESTINATION_RUNS);

        // ↑ to the library, → steps it: the path goes by its layout.
        gui.torrent.templates.insert("Audiobooks".into(), "{{ARTIST}} - {{ALBUM}} ({{YEAR}})".into());
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.sheet, Some(Sheet::Destination(DestStop::Library)));
        assert_eq!(tips(&gui), "← → library · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::Right);
        assert_eq!((gui.torrent.vpath, gui.torrent.path.value()), (1, "Northlight - Field Notes (2019)"));
        let rows = draw(&mut gui);
        assert!(rows[7].contains("─ Destination · Audiobooks › Northlight - Field Notes (2019) ─"), "the title wears the place:\n{}", rows[7]);
        key(&mut gui, KeyCode::Left);
        assert_eq!((gui.torrent.vpath, gui.torrent.path.value()), (0, "Northlight/Field Notes"));

        // A hand edit sticks until a new source (clause 13).
        key(&mut gui, KeyCode::Down);
        type_text(&mut gui, " (live)");
        assert!(gui.torrent.path_edited);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.torrent.path.value(), "Northlight/Field Notes (live)", "the library changed, the path stayed");
        key(&mut gui, KeyCode::Left);

        // The checkbox, by Space or a click; Enter on Done closes.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.sheet, Some(Sheet::Destination(DestStop::Rename)));
        assert_eq!(tips(&gui), "Space toggle · ↑ ↓ rows · Esc close");
        key(&mut gui, KeyCode::Char(' '));
        assert!(!gui.torrent.rename_root);
        let rows = draw(&mut gui);
        let at = super::super::ytdl::tests::at(&rows, "[ ] Rename the torrent's root folder");
        let hit = gui.ui.hit(Position { x: at.x + 6, y: at.y }).expect("the checkbox is a click target");
        gui.act(hit);
        assert!(gui.torrent.rename_root);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.sheet, None);
        field_notes(&mut gui);
        assert_eq!(gui.torrent.path.value(), "Northlight/Field Notes", "a new source starts the path over");

        // One library is no choice: the sheet opens without its radios.
        let mut gui = ready_gui(&["Music"]);
        field_notes(&mut gui);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('l'));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[8].contains("─ Destination · Music › Northlight › Field Notes ─"), "{text}");
        assert!(rows[9].contains("│ PATH") && !text.contains("(•)"), "the path's box first, no radios:\n{text}");
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.sheet, Some(Sheet::Destination(DestStop::Path)), "the path is its first stop");
    }

    /// Where some of the canvas's torrent already lives.
    fn two_matches() -> Vec<SeedMatch> {
        vec![
            SeedMatch { vpath: "Music".into(), relative_path: "_incoming/Field Notes [FLAC]".into(), matched: Some(10), total: Some(12), missing: Some(2) },
            SeedMatch { vpath: "Audiobooks".into(), relative_path: "misc/Field Notes".into(), matched: Some(3), total: Some(12), missing: Some(9) },
        ]
    }

    #[test]
    fn files_already_exist_takes_a_choice_and_a_button() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        gui.torrent.busy = Some(Busy::Checking);
        apply_reply(&mut gui, Reply::Checked(Ok(SeedCheck { ok: true, outcome: "partial_match".into(), matches: two_matches(), ..Default::default() })));
        assert_sheet(&mut gui, 13, boards::MATCHES, boards::MATCHES_RUNS);
        assert_eq!(tips(&gui), "↑ ↓ pick · Enter use this location · Esc back");

        // A click on a place moves the dot; nothing is added by it.
        let rows = draw(&mut gui);
        let at = super::super::ytdl::tests::at(&rows, "Download fresh anyway");
        let hit = gui.ui.hit(Position { x: at.x, y: at.y }).expect("a place is a click target");
        assert_eq!(hit, tor(TorAct::MatchPick(2)));
        gui.act(hit);
        assert_eq!(gui.torrent.matches.as_ref().map(|m| m.row), Some(2));
        assert!(gui.torrent.busy.is_none(), "a click only chooses");
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.torrent.matches.as_ref().map(|m| m.row), Some(0));
        // Esc backs out with nothing added.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.torrent.matches.is_none() && gui.torrent.busy.is_none() && gui.torrent.file.is_some());
    }

    #[test]
    fn torrent_received_asks_with_two_buttons_and_enter_adds() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.chooser = Some(Chooser { incoming: Incoming::Magnet(MAGNET.into()), row: None, dont_ask: false });
        assert_sheet(&mut gui, 11, boards::RECEIVED, boards::RECEIVED_RUNS);
        assert_eq!(tips(&gui), "↑ ↓ pick · Enter choose · Space don't ask again · Esc close");

        // The first walking key shows the hand, on the primary.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.chooser.as_ref().map(|c| c.row), Some(Some(0)));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.chooser.as_ref().map(|c| c.row), Some(Some(2)));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.chooser.as_ref().map(|c| c.dont_ask), Some(true), "Enter on the box ticks it");
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.torrent.chooser.as_ref().map(|c| c.dont_ask), Some(false));
    }

    /// Where a language's words are wider than half the sheet, one button
    /// stands over the other.
    #[test]
    fn the_received_buttons_stack_where_their_words_are_wide() {
        let Some(code) = crate::setup::tests::in_own_process(&["ru"]) else { return };
        let mut gui = canvas();
        gui.torrent.chooser = Some(Chooser { incoming: Incoming::Magnet(MAGNET.into()), row: None, dont_ask: false });
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        let add = t!("gui.tor.received_add").to_string();
        let other = t!("gui.tor.open_with").to_string();
        let a = rows.iter().position(|r| r.contains(&add)).expect("add");
        let b = rows.iter().position(|r| r.contains(&other)).expect("other");
        assert_eq!(b, a + 3, "{code}: stacked\n{text}");
    }

    #[test]
    fn the_typed_picker_is_a_sheet_that_grows_downward_from_its_box() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.picker = Some(Picker {
            text: Input::new("~/Downloads/".into()),
            listed_for: "~/Downloads/".into(),
            entries: vec![
                PickEntry::new("Music".into(), true),
                PickEntry::new("old".into(), true),
                PickEntry::new(format!("{NAME}.torrent"), false),
                PickEntry::new("ubuntu-26.10-desktop-amd64.iso.torrent".into(), false),
            ],
            ..Picker::default()
        });
        // It stands where it would with its six rows: one row up from its
        // own middle, the box holding its spot as the list narrows.
        assert_frame(&buffer(&mut gui, 100, 30), (17, 9), 66, boards::PICKER, boards::PICKER_RUNS);
        gui.torrent.picker.as_mut().unwrap().text = Input::new("~/Downloads/N".into());
        let rows = draw(&mut gui);
        assert!(rows[9].contains("─ Choose a .torrent file ─") && rows[11].contains("~/Downloads/N"), "the box where it was:\n{}", rows.join("\n"));
        assert!(rows[13].contains(&format!("  {NAME}.torrent")), "one row of it:\n{}", rows.join("\n"));
        assert_eq!(rows[15].chars().nth(17), Some('╰'), "and the sheet ends under it:\n{}", rows.join("\n"));

        // The wheel is the sheet's while it is up: the hub hands it over.
        let picker = gui.torrent.picker.as_mut().unwrap();
        picker.text = Input::new("~/Downloads/".into());
        picker.entries = (0..20).map(|i| PickEntry::new(format!("album-{i:02}.torrent"), false)).collect();
        draw(&mut gui);
        gui.wheel(Position { x: 40, y: 14 }, 1);
        gui.wheel(Position { x: 40, y: 14 }, 1);
        let rows = draw(&mut gui);
        assert!(rows[13].contains("album-02.torrent"), "scrolled two:\n{}", rows[13]);
    }

    // ── The page ───────────────────────────────────────────────────────────

    #[test]
    fn the_nav_row_opens_the_room_and_esc_has_nothing_to_leave() {
        let _en = english();
        let mut gui = gui();
        // Settings keeps TORRENTS to the ask-me switch alone: the room is
        // the nav's, the row under TOOLS after Discover — here, with no
        // sonic path and no Discover, packed under Auto DJ on r16.
        gui.active = super::super::SETTINGS_NAV;
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[12].contains("TORRENTS"), "the group:\n{text}");
        assert!(rows[13].contains("[✓] Ask what to do with torrents"), "ask is on by default:\n{text}");
        assert!(!text.contains("Add a torrent"), "no doorway in Settings:\n{text}");
        let nav: String = rows[16].chars().take(15).collect();
        assert_eq!(nav.trim(), "Add torrent", "the nav row:\n{text}");
        let row = gui.ui.hit(Position { x: 3, y: 16 }).expect("the row is a click target");
        assert_eq!(row, Act::Nav(super::super::TORRENT_NAV));

        gui.act(row);
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::TORRENT_NAV), "the row opens the room");
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        // The queue folded, the room is 80 cells: the block centred in it.
        assert!(rows[2].chars().skip(29).collect::<String>().starts_with("Add torrent"), "no way back:\n{}", rows[2]);
        assert!(!rows[2].contains('◂'), "{}", rows[2]);
        assert!(rows[16].starts_with(" ▸ Add torrent"), "the row lit:\n{}", rows[16]);
        assert!(text.contains("Choose a .torrent file…") && text.contains("Paste a magnet link"), "the two ways in:\n{text}");
        assert!(!text.contains("Metadata") && !text.contains("FROM TORRENTS"), "nothing below the source, nothing sent:\n{text}");
        assert!(!text.contains("│ Paste │"), "a terminal reads no clipboard: no Paste\n{text}");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::File));
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.torrent.cursor, None, "Esc stows the hand");
        key(&mut gui, KeyCode::Esc);
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::TORRENT_NAV), "and the room stays: a nav room has no back");

        // The ask row flips the persisted choice.
        gui.act(Act::Nav(super::super::SETTINGS_NAV));
        gui.act(Act::Row(super::super::ROW_ASK));
        assert!(!gui.config.torrent.ask);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("[ ] Ask what to do with torrents"), "got:\n{text}");
    }

    #[test]
    fn the_gate_stands_on_the_title_row_and_speaks_under_it() {
        let _en = english();
        let mut gui = ready_gui(&["Music"]);
        gui.queue_open = true;
        let rows = draw(&mut gui);
        assert!(rows[2].chars().skip(17).take(47).collect::<String>().ends_with("Transmission · ready"), "{}", rows[2]);
        // Being asked: the words whole, under the title — beside it they
        // were cut at 32 cells.
        gui.torrent.gate = Gate::Pending;
        let rows = draw(&mut gui);
        assert_eq!(rows[2].chars().skip(17).take(47).collect::<String>().trim(), "Add torrent");
        assert!(rows[3].contains("asking whether this server takes torrents…"), "{}", rows[3]);
        gui.torrent.gate = Gate::Failed("timed out".into());
        let rows = draw(&mut gui);
        assert!(rows[3].contains("couldn't ask the server about torrents: timed"), "{}", rows[3]);
        assert!(rows[4].contains("out"), "{}", rows[4]);
        // No server at all: what to do about it, whole, on its two rows.
        gui.torrent.gate = Gate::NotAsked;
        gui.app.connected = false;
        let rows = draw(&mut gui);
        let said: String = rows[3..5].iter().map(|r| r.chars().skip(17).take(47).collect::<String>().trim().to_string()).collect::<Vec<_>>().join(" ");
        assert_eq!(said, t!("gui.no_server").to_string());
    }

    #[test]
    fn the_form_stays_hidden_until_a_source_is_real() {
        let _en = english();
        let mut gui = ready_gui(&["music"]);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Transmission · ready"), "the gate's standing:\n{text}");

        // ↓ lands on the file's box, then the typed road, then the link's.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::File));
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::TypePath));
        assert_eq!(tips(&gui), "Enter type a path · ↑ ↓ rows · Esc deselect");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::Magnet));
        type_text(&mut gui, "magnet:?xt=urn:btih:abc");
        assert!(!gui.torrent.has_source(), "a half-typed link is not a source");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Not a valid magnet link"), "marked in place (clause 4):\n{text}");
        assert!(!text.contains("Metadata"), "still hidden:\n{text}");

        // Finish it: the form reveals, pre-filled from the dn.
        for _ in 0.."abc".len() {
            key(&mut gui, KeyCode::Backspace);
        }
        type_text(&mut gui, "0123456789abcdef0123456789abcdef01234567&dn=Vela+-+Cassini+(2020)");
        assert!(gui.torrent.has_source());
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert_eq!(gui.torrent.album.value(), "Cassini");
        assert_eq!(gui.torrent.year.value(), "2020");
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini", "the legacy layout with no template");
        let text = draw(&mut gui).join("\n");
        for needed in ["Vela - Cassini (2020)", "Vela · Cassini · 2020", "music › Vela › Cassini", "Add torrent ▸"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        for gone in ["Choose a .torrent file", "Force fresh download", "Not a valid magnet link", "Open in another app"] {
            assert!(!text.contains(gone), "{gone:?} stands:\n{text}");
        }
        // Enter in the box moves on to the rows.
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.cursor, Some(Row::Metadata));

        // A template on the library re-shapes the path.
        gui.torrent.templates.insert("music".into(), "{{ARTIST}}/{{ALBUM}} ({{YEAR}})".into());
        recompute_path(&mut gui);
        assert_eq!(gui.torrent.path.value(), "Vela/Cassini (2020)");

        // [X] clears the link and the two boxes are back, the hand on it.
        let rows = draw(&mut gui);
        let at = super::super::ytdl::tests::at(&rows, "[X]");
        gui.act(gui.ui.hit(at).expect("[X] is a click target"));
        assert!(gui.torrent.magnet.value().is_empty() && gui.torrent.cursor == Some(Row::Magnet));
        assert!(draw(&mut gui).join("\n").contains("Choose a .torrent file…"));
    }

    #[test]
    fn a_loaded_file_takes_the_boxes_place_and_its_x_lets_it_go() {
        let _en = english();
        let mut gui = ready_gui(&["music"]);
        gui.torrent.magnet = Input::new("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".into());
        set_file(&mut gui, LoadedFile { name: "vela.torrent".into(), bytes: album("Vela - Cassini (2020)", 3, 3 * 1024 * 1024) });
        assert!(gui.torrent.magnet.value().is_empty(), "exactly one source (clause 1)");
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert_eq!(gui.torrent.year.value(), "2020");
        let text = draw(&mut gui).join("\n");
        for needed in ["vela.torrent", "[X]", "3 files · 3.0 MB", "Auto-detect metadata · Open in another app", "[ ] Force fresh download"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        assert!(!text.contains("Paste a magnet link") && !text.contains("Choose a .torrent file"), "the card takes the boxes' place (clause 5):\n{text}");

        // A file whose list can't be read still loads: the name alone.
        set_file(&mut gui, LoadedFile { name: "odd.torrent".into(), bytes: b"d4:infod4:name3:odde".to_vec() });
        assert_eq!(gui.torrent.facts, None);
        let rows = draw(&mut gui);
        assert!(rows[5].contains("odd.torrent") && rows[6].contains('╰'), "a three-row card:\n{}", rows.join("\n"));

        // [X] lets go of the file; the two boxes return.
        let at = super::super::ytdl::tests::at(&rows, "[X]");
        assert_eq!(gui.ui.hit(at), Some(tor(TorAct::Unload)));
        gui.act(tor(TorAct::Unload));
        assert!(gui.torrent.file.is_none() && gui.torrent.facts.is_none());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Paste a magnet link") && text.contains("Choose a .torrent file…"), "got:\n{text}");
    }

    #[test]
    fn a_letter_acts_where_its_control_is_drawn() {
        let _en = english();
        let mut gui = canvas();
        // Nothing loaded: only the two ways in have letters.
        draw(&mut gui);
        let letters = |gui: &Gui| gui.torrent.drawn.iter().map(|(c, _)| *c).collect::<String>();
        assert_eq!(letters(&gui), "bt");
        for c in ['x', 'o', 'd', 'e', 'l'] {
            assert_eq!(handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)), None, "{c} is the shell's");
        }
        // Their capitals are the room's all the same, drawn or not.
        for c in ['D', 'E', 'L', 'X', 'O'] {
            let taken = handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT));
            assert_eq!(taken, Some(false), "{c} would reach the shell");
        }
        key(&mut gui, KeyCode::Char('t'));
        assert!(gui.torrent.picker.is_some(), "t types a path");
        gui.torrent.picker = None;

        field_notes(&mut gui);
        draw(&mut gui);
        assert_eq!(letters(&gui), "bxdoel");
        key(&mut gui, KeyCode::Char('e'));
        assert_eq!(gui.torrent.sheet, Some(Sheet::Metadata(MetaStop::Artist)));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Char('l'));
        assert_eq!(gui.torrent.sheet, Some(Sheet::Destination(DestStop::Path)));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Char('x'));
        assert!(gui.torrent.file.is_none(), "x lets go");

        // The queue panel holding the keys, the room yields every one.
        field_notes(&mut gui);
        gui.app.focus = Focus::Queue;
        draw(&mut gui);
        for code in [KeyCode::Char('x'), KeyCode::Down, KeyCode::Enter] {
            assert_eq!(handle_key(&mut gui, KeyEvent::new(code, KeyModifiers::NONE)), None, "{code:?}");
        }
        assert!(gui.torrent.file.is_some() && gui.torrent.cursor.is_none());
    }

    #[test]
    fn the_hand_walks_the_stops_that_exist_and_enter_does_each_ones_act() {
        let _en = english();
        let mut gui = canvas();
        assert_eq!(
            gui.torrent.rows(true),
            vec![Row::File, Row::TypePath, Row::Magnet, Row::Receipt(0), Row::Receipt(1), Row::Receipt(2)]
        );
        field_notes(&mut gui);
        let stops = vec![
            Row::File,
            Row::Detect,
            Row::HandOff,
            Row::Metadata,
            Row::Destination,
            Row::Force,
            Row::Submit,
            Row::Receipt(0),
            Row::Receipt(1),
            Row::Receipt(2),
        ];
        assert_eq!(gui.torrent.rows(true), stops);
        assert!(!gui.torrent.rows(false).contains(&Row::Detect), "a server that takes none is not asked what one holds");
        for stop in &stops {
            key(&mut gui, KeyCode::Down);
            assert_eq!(gui.torrent.cursor, Some(*stop));
        }
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.torrent.cursor, Some(Row::Receipt(2)), "the last stop holds");
        assert_eq!(tips(&gui), "Enter Show in folder · x dismiss · ↑ ↓ rows · Esc deselect");
        for _ in 0..4 {
            key(&mut gui, KeyCode::Up);
        }
        assert_eq!(gui.torrent.cursor, Some(Row::Force));
        assert_eq!(tips(&gui), "Space toggle · ↑ ↓ rows");
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.torrent.force_fresh);
        key(&mut gui, KeyCode::Enter);
        assert!(!gui.torrent.force_fresh, "Enter ticks it too");
        key(&mut gui, KeyCode::Up);
        assert_eq!(tips(&gui), "Enter Destination · ↑ ↓ rows · Esc deselect");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.sheet, Some(Sheet::Destination(DestStop::Path)));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.sheet, Some(Sheet::Metadata(MetaStop::Artist)));
        key(&mut gui, KeyCode::Esc);

        // The hand shows: the row's label ACCENT BOLD, the card's ring.
        let buf = buffer(&mut gui, 100, 30);
        let cell = &buf[(17, 10)];
        assert_eq!((cell.symbol(), cell.fg), ("M", th().accent), "Metadata under the hand");
        assert!(cell.modifier.contains(Modifier::BOLD));
        gui.torrent.cursor = Some(Row::File);
        let buf = buffer(&mut gui, 100, 30);
        assert_eq!(buf[(17, 4)].fg, th().accent, "the card's frame");
        gui.torrent.cursor = Some(Row::Submit);
        let buf = buffer(&mut gui, 100, 30);
        assert_eq!(buf[(17, 14)].fg, th().bright, "the primary under the hand is lit");
        assert_eq!(tips(&gui), "Enter add the torrent · ↑ ↓ rows");
    }

    #[test]
    fn the_destination_keeps_its_library_when_the_row_is_short() {
        let crumbs: Vec<String> = ["Music", "Various Artists", "Compilations", "Field Notes"].iter().map(|s| s.to_string()).collect();
        let fit = |room: usize| fit_dest(&crumbs, room).join(" › ");
        assert_eq!(fit(60), "Music › Various Artists › Compilations › Field Notes");
        assert_eq!(fit(40), "Music › … › Compilations › Field Notes");
        assert_eq!(fit(30), "Music › … › Field Notes");
        assert_eq!(fit(20), "Music › … › Field N…");
        assert_eq!(fit(9), "Field No…", "too short for the library and anything of the folder");
        assert_eq!(fit_dest(&crumbs[..1], 3).join(""), "Mu…");

        // On the page: the library first where today's preview lost it.
        let _en = english();
        let mut gui = canvas();
        set_file(
            &mut gui,
            LoadedFile { name: "va.torrent".into(), bytes: album("Various Artists - Field Recordings of the North Atlantic (2019)", 2, 1024) },
        );
        let rows = draw(&mut gui);
        let row: String = rows[11].chars().skip(17).take(47).collect();
        assert_eq!(row, "Destination  Music › … › Field Recordings of… ▸", "{row}");
    }

    #[test]
    fn a_paste_is_taken_whole_wherever_the_hand_is() {
        let _en = english();
        let dir = scratch("paste");
        let file = dir.join("Vela - Cassini (2020).torrent");
        std::fs::write(&file, album("Vela - Cassini (2020)", 2, 2048)).unwrap();

        // A path — a file dropped on a terminal arrives as one — loads it.
        let mut gui = canvas();
        gui.torrent.cursor = Some(Row::Receipt(1));
        paste(&mut gui, &format!("{}\n", file.to_string_lossy().replace(' ', "\\ ")));
        settle(&mut gui, |gui| gui.torrent.file.is_some());
        assert_eq!(gui.torrent.file.as_ref().map(|f| f.name.as_str()), Some("Vela - Cassini (2020).torrent"));
        assert_eq!(gui.torrent.artist.value(), "Vela");
        assert_eq!(gui.torrent.cursor, None, "the hand down: Enter adds");

        // A link pasted over it lets the file go.
        paste(&mut gui, MAGNET);
        assert!(gui.torrent.file.is_none() && gui.torrent.facts.is_none());
        assert_eq!(gui.torrent.magnet.value(), MAGNET);
        assert_eq!(gui.torrent.album.value(), "Field Notes");

        // What is neither says so under the boxes; a folder opens the
        // typed picker on itself.
        gui.act(tor(TorAct::ClearLink));
        let not_one = dir.join("song.mp3");
        std::fs::write(&not_one, b"ID3").unwrap();
        paste(&mut gui, &not_one.to_string_lossy());
        settle(&mut gui, |gui| gui.torrent.refusal.is_some());
        assert_eq!(gui.torrent.refusal.as_deref(), Some("“song.mp3” is not a .torrent file"));
        paste(&mut gui, &dir.to_string_lossy());
        settle(&mut gui, |gui| gui.torrent.picker.is_some());
        let typed = gui.torrent.picker.as_ref().map(|p| p.text.value().to_string()).unwrap_or_default();
        // The folder and a separator after it: the platform's own, `\` on
        // Windows.
        let folder = format!("paste-{}{}", std::process::id(), std::path::MAIN_SEPARATOR);
        assert!(typed.ends_with(&folder), "{typed}");
        assert_eq!(gui.torrent.refusal, None, "the last paste's words go with the next");
        assert!(!wants_paste(&gui), "the picker's box takes its own, typed");
        gui.torrent.picker = None;

        // Nothing wants one elsewhere: another room, a sheet, a modal.
        assert!(wants_paste(&gui));
        gui.act(Act::Nav(super::super::SETTINGS_NAV));
        assert!(!wants_paste(&gui), "another room");
        gui.act(Act::Nav(super::super::TORRENT_NAV));
        field_notes(&mut gui);
        gui.act(tor(TorAct::OpenDestination(DestStop::Path)));
        assert!(!wants_paste(&gui), "a sheet");
        paste(&mut gui, MAGNET);
        assert!(gui.torrent.file.is_some(), "and one that arrives is dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_opens_the_picker_with_the_separator_its_path_uses() {
        // Read off the reply itself, so both spellings are tried on every
        // platform: a Windows path keeps its backslash, and a separator
        // already at the end is not doubled.
        let mut gui = ready_gui(&["Music"]);
        for (path, typed) in [
            (r"C:\Music\Drops", r"C:\Music\Drops\"),
            (r"C:\Music\Drops\", r"C:\Music\Drops\"),
            ("/home/vela/Drops", "/home/vela/Drops/"),
            ("/home/vela/Drops/", "/home/vela/Drops/"),
        ] {
            apply_reply(&mut gui, Reply::Loaded { path: path.into(), result: Ok(Loaded::Dir) });
            assert_eq!(gui.torrent.picker.as_ref().map(|p| p.text.value()), Some(typed), "{path}");
        }
    }

    #[test]
    fn where_a_paste_arrives_as_keys_the_link_box_holds_them_from_the_start() {
        let _en = english();
        let dir = scratch("typed");
        let file = dir.join("vela.torrent");
        std::fs::write(&file, album("Vela - Cassini (2020)", 2, 2048)).unwrap();

        // A Windows console: no whole paste, so the room opens with the
        // hand on the link box and a pasted link's letters land in it.
        let mut gui = ready_gui(&["Music"]);
        gui.ytdl.whole_pastes = false;
        gui.act(Act::Nav(super::super::SETTINGS_NAV));
        gui.act(Act::Nav(super::super::TORRENT_NAV));
        assert_eq!(gui.torrent.cursor, Some(Row::Magnet));
        type_text(&mut gui, MAGNET);
        assert_eq!(gui.torrent.magnet.value(), MAGNET);
        assert!(gui.torrent.picker.is_none() && gui.torrent.sheet.is_none(), "none of its letters was a key");
        assert_eq!(gui.torrent.album.value(), "Field Notes");

        // A path typed (or pasted as keys) into the box is no link: Enter
        // reads it as the path it is and loads the file.
        gui.act(tor(TorAct::ClearLink));
        type_text(&mut gui, &file.to_string_lossy());
        assert!(draw(&mut gui).join("\n").contains("Not a valid magnet link"));
        key(&mut gui, KeyCode::Enter);
        settle(&mut gui, |gui| gui.torrent.file.is_some());
        assert!(gui.torrent.magnet.value().is_empty() && gui.torrent.cursor.is_none());
        assert_eq!(gui.torrent.artist.value(), "Vela");
        // With a source in hand the room opens as everywhere: hand stowed.
        gui.act(Act::Nav(super::super::SETTINGS_NAV));
        gui.act(Act::Nav(super::super::TORRENT_NAV));
        assert_eq!(gui.torrent.cursor, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paste_stands_only_where_the_clipboard_can_be_read() {
        let _en = english();
        let mut gui = canvas();
        gui.ytdl.clipboard = None;
        let rows = draw(&mut gui);
        assert!(!rows.join("\n").contains("│ Paste │"), "a terminal has none");
        let row: String = rows[10].chars().skip(17).take(47).collect();
        assert_eq!(row, format!("│ Paste a magnet link{}│", " ".repeat(25)), "the box takes the band");

        fn magnet() -> Option<String> {
            Some(MAGNET.to_string())
        }
        gui.ytdl.clipboard = Some(magnet);
        let rows = draw(&mut gui);
        let at = super::super::ytdl::tests::at(&rows, "│ Paste │");
        let hit = gui.ui.hit(Position { x: at.x + 3, y: at.y }).expect("Paste is a click target");
        assert_eq!(hit, tor(TorAct::PasteClip));
        gui.act(hit);
        assert_eq!(gui.torrent.magnet.value(), MAGNET, "the clipboard read on the click");
        // Beside a link too; an empty clipboard changes nothing.
        gui.ytdl.clipboard = Some(no_clip);
        assert!(draw(&mut gui).join("\n").contains("│ Paste │"));
        gui.act(tor(TorAct::PasteClip));
        assert_eq!(gui.torrent.magnet.value(), MAGNET);
    }

    // ── What was sent ──────────────────────────────────────────────────────

    #[test]
    fn what_was_sent_is_kept_newest_first_and_a_rows_enter_opens_its_folder() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);

        // Seeding what is already there: the library is all the server says.
        gui.torrent.busy = Some(Busy::Checking);
        apply_reply(
            &mut gui,
            Reply::Checked(Ok(SeedCheck { ok: true, outcome: "seeded".into(), name: Some(NAME.into()), vpath: Some("Audiobooks".into()), ..Default::default() })),
        );
        assert_eq!(gui.note, Some(("Already on disk — seeding it now".into(), false)));
        assert_eq!(gui.torrent.receipts[0], Receipt { name: NAME.into(), vpath: "Audiobooks".into(), path: String::new(), at: START });
        let rows = draw(&mut gui);
        let row: String = rows[14].chars().skip(17).take(47).collect();
        assert_eq!(row, "✓ Northlight - Field No… Audiobooks    just now", "its own name, its library");

        // One the client already had leaves nothing new; neither does one
        // already in the client.
        field_notes(&mut gui);
        sending(&mut gui);
        apply_reply(&mut gui, Reply::Added(Ok(TorrentAdded { ok: true, name: Some(NAME.into()), is_duplicate: true, ..Default::default() })));
        assert_eq!(gui.note, Some((format!("“{NAME}” is already in the client"), false)));
        field_notes(&mut gui);
        gui.torrent.busy = Some(Busy::Checking);
        apply_reply(&mut gui, Reply::Checked(Ok(SeedCheck { ok: true, outcome: "already_in_daemon".into(), ..Default::default() })));
        assert_eq!(gui.torrent.receipts.len(), 4);

        // A place files already were: its own folder is the receipt's.
        field_notes(&mut gui);
        gui.torrent.matches = Some(Matches { list: two_matches(), row: 0 });
        let req = add_request(&gui, "Music".into(), "_incoming".into(), "Field Notes [FLAC]".into());
        assert_eq!(receipt_of(&req).folder(), "Music/_incoming/Field Notes [FLAC]");

        // Enter on a row opens its folder in Files; x takes the row off.
        gui.torrent.matches = None;
        gui.torrent.cursor = Some(Row::Receipt(1));
        key(&mut gui, KeyCode::Enter);
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::FILES_NAV));
        assert_eq!(gui.app.path, "Music/Vela/Perihelion");
        gui.act(Act::Nav(super::super::TORRENT_NAV));
        gui.torrent.cursor = Some(Row::Receipt(3));
        key(&mut gui, KeyCode::Char('x'));
        assert_eq!(gui.torrent.receipts.len(), 3);
        assert_eq!(gui.torrent.cursor, Some(Row::Receipt(2)), "the hand on the row that took its place");
        // The seeded one opens its library: that is all it knows.
        gui.act(tor(TorAct::Receipt(0)));
        assert_eq!(gui.app.path, "Audiobooks");
    }

    #[test]
    fn the_list_scrolls_under_the_wheel_and_belongs_to_the_server() {
        let _en = english();
        let mut gui = canvas();
        gui.torrent.receipts =
            (0..30).map(|i| Receipt { name: format!("Album {i:02}"), vpath: "Music".into(), path: format!("Artist/Album {i:02}"), at: START - i * DAY }).collect();
        field_notes(&mut gui);
        let first = |gui: &mut Gui| draw(gui)[19].chars().skip(17).take(10).collect::<String>();
        assert_eq!(first(&mut gui), "✓ Album 00");
        gui.wheel(Position { x: 30, y: 22 }, 3);
        assert_eq!(first(&mut gui), "✓ Album 03");
        // The keys' hand brings its row into view.
        gui.torrent.cursor = Some(Row::Receipt(29));
        gui.torrent.list.pick_up();
        let rows = draw(&mut gui);
        assert!(rows[24].contains("✓ Album 29"), "{}", rows[24]);

        // The list keeps fifty; a new one drops the oldest.
        gui.torrent.receipts =
            (0..RECEIPTS_KEPT).map(|i| Receipt { name: format!("A{i}"), vpath: "Music".into(), path: format!("A{i}"), at: 0 }).collect();
        sending(&mut gui);
        apply_reply(&mut gui, added(NAME));
        assert_eq!(gui.torrent.receipts.len(), RECEIPTS_KEPT);
        assert_eq!(gui.torrent.receipts.last().map(|r| r.name.as_str()), Some("A48"));

        // Another server's session: its own list, not this one's.
        gui.app.session.server = "http://other:3000".into();
        observe(
            &mut gui,
            &Event::Connected {
                server: "http://other:3000".into(),
                id: "http://other:3000".into(),
                username: None,
                token: None,
                ping: Box::default(),
            },
        );
        assert!(gui.torrent.receipts.is_empty() && gui.torrent.gate == Gate::Pending, "asked afresh, its own list");
    }

    // ── Carried over ───────────────────────────────────────────────────────

    #[test]
    fn submit_reads_the_form_and_refuses_in_words() {
        let _en = english();
        let mut gui = ready_gui(&["music"]);
        assert_eq!(plan_submit(&gui), Err("Add a magnet link or a .torrent file (one)".to_string()));
        gui.torrent.magnet = Input::new("magnet:?xt=urn:btih:nope".into());
        assert_eq!(plan_submit(&gui), Err("Not a valid magnet link".to_string()));
        gui.torrent.magnet = Input::new(MAGNET.into());
        magnet_changed(&mut gui);
        gui.torrent.path = Input::new("  ".into());
        assert_eq!(plan_submit(&gui), Err("Destination path is empty".to_string()));
        gui.torrent.path = Input::new("Northlight/Live/Field Notes".into());
        let Ok(Plan::Add(req)) = plan_submit(&gui) else { panic!("a magnet adds outright") };
        assert_eq!((req.vpath.as_str(), req.sub_path.as_str(), req.directory_name.as_str()), ("music", "Northlight/Live", "Field Notes"));
        assert!(req.rename_root, "on by default (clause 15)");
        assert_eq!(req.source, TorrentSource::Magnet(MAGNET.into()));

        // A file checks the disk first, unless forced fresh.
        let file = LoadedFile { name: "a.torrent".into(), bytes: album("Vela - Cassini (2020)", 1, 10) };
        set_file(&mut gui, file.clone());
        assert_eq!(plan_submit(&gui), Ok(Plan::Check(file)));
        gui.torrent.force_fresh = true;
        assert!(matches!(plan_submit(&gui), Ok(Plan::Add(_))));

        // No library at all.
        gui.app.libraries.clear();
        assert_eq!(plan_submit(&gui), Err("Pick a library".to_string()));
    }

    #[test]
    fn the_seed_check_outcomes_are_worded_and_the_partial_match_asks() {
        let _en = english();
        let mut gui = ready_gui(&["music"]);
        let file = LoadedFile { name: "a.torrent".into(), bytes: album("Vela - Cassini (2020)", 1, 10) };
        let check = |outcome: &str| SeedCheck { ok: true, outcome: outcome.into(), ..Default::default() };

        set_file(&mut gui, file.clone());
        apply_check(&mut gui, check("already_in_daemon"));
        assert_eq!(gui.note, Some(("Already in the torrent client".into(), false)));
        assert!(gui.torrent.file.is_none(), "done: the source goes");

        set_file(&mut gui, file.clone());
        apply_check(&mut gui, SeedCheck { error: Some("bad pieces".into()), ..check("invalid_torrent") });
        assert_eq!(gui.torrent.refusal.as_deref(), Some("bad pieces"), "the server's words, on the page");
        assert!(gui.torrent.file.is_some(), "refused: the form stands");
        apply_check(&mut gui, check("invalid_torrent"));
        assert_eq!(gui.torrent.refusal.as_deref(), Some("Invalid torrent file"));

        // A partial match asks; a match with no folder is said, not sent.
        apply_check(&mut gui, SeedCheck { matches: vec![SeedMatch { vpath: "music".into(), relative_path: String::new(), ..Default::default() }], ..check("partial_match") });
        assert_eq!(gui.torrent.matches.as_ref().map(|m| (m.list.len(), m.row)), Some((1, 0)));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.note, Some(("That match has no folder name — use 'Download fresh' instead".into(), true)));
        assert!(gui.torrent.matches.is_some() && gui.torrent.busy.is_none());
        key(&mut gui, KeyCode::Esc);
        assert!(gui.torrent.matches.is_none());
    }

    #[test]
    fn the_arrival_chooser_asks_and_the_dont_ask_box_persists() {
        let _en = english();
        let mut gui = gui();
        arrive(&mut gui, MAGNET);
        assert!(matches!(gui.torrent.chooser, Some(Chooser { incoming: Incoming::Magnet(_), row: None, dont_ask: false })));
        assert_ne!(gui.active, super::super::TORRENT_NAV, "nothing opens until the answer");
        let text = draw(&mut gui).join("\n");
        for needed in ["Torrent received", NAME, "Add to mStream ▸", "Open in another app", "[ ] Always add to mStream, don't ask again"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        // Space ticks the box, wherever the hand is; Enter — the hand
        // walked back to the primary — adds, and the tick flips the setting.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.torrent.chooser.as_ref().map(|c| (c.row, c.dont_ask)), Some((Some(2), true)));
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.torrent.chooser.is_none());
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::TORRENT_NAV), "Add opens the nav room");
        assert_eq!(gui.torrent.magnet.value(), MAGNET);
        assert_eq!(gui.torrent.artist.value(), "Northlight");
        assert!(!gui.config.torrent.ask, "the box flipped the setting (clause 51)");

        // Asking is off now: the next one goes straight in.
        arrive(&mut gui, MAGNET);
        assert!(gui.torrent.chooser.is_none(), "ask is off now: straight in");

        // Asked again, Enter with the hand down adds; Esc drops it.
        gui.config.torrent.ask = true;
        gui.torrent.magnet = Input::default();
        arrive(&mut gui, MAGNET);
        key(&mut gui, KeyCode::Esc);
        assert!(gui.torrent.chooser.is_none() && gui.torrent.magnet.value().is_empty(), "Esc takes nothing");
        arrive(&mut gui, MAGNET);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.torrent.magnet.value(), MAGNET, "Enter is the primary's until the hand is picked up");
    }

    #[test]
    fn a_torrent_arriving_with_ask_off_goes_straight_to_the_room() {
        let _en = english();
        let dir = scratch("arrive");
        let path = dir.join("vela.torrent");
        std::fs::write(&path, album("Vela - Cassini (2020)", 2, 2048)).unwrap();
        let mut gui = gui();
        gui.config.torrent.ask = false;
        // From the Now Playing screen: the nav room, the Library brought
        // back for it.
        gui.act(Act::Screen(Screen::NowPlaying));
        arrive(&mut gui, &path.to_string_lossy());
        assert!(gui.torrent.chooser.is_none());
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::TORRENT_NAV), "opened with mStream: the nav room");
        assert!(!gui.app.fullscreen);
        assert_eq!(gui.torrent.file.as_ref().map(|f| f.name.as_str()), Some("vela.torrent"));
        assert_eq!(gui.torrent.album.value(), "Cassini");
        assert_eq!(gui.torrent.facts, Some((2, 2048)));

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
        let _en = english();
        // handoff_dir() follows MSTREAM_PLAYER_CONFIG_DIR, which the config
        // tests repoint process-wide under their scratch lock. Unheld, the
        // folder could move between this write and arrive()'s own look (our
        // file taken for a stranger's: a chooser), or the file could land
        // in another test's scratch and vanish with it (nothing read, no
        // room) — and with no scratch live it stayed behind in the test run's
        // own folder. Holding a scratch pins the folder for the whole test,
        // and its drop cleans up.
        let scratch = crate::config::testing::Scratch::new("torrent-bounce");
        let dir = handoff_dir();
        assert!(dir.starts_with(&scratch.dir), "the hand-off folder is in the scratch: {dir:?}");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bounce.torrent");
        std::fs::write(&path, album("Vela - Cassini (2020)", 1, 10)).unwrap();
        let mut gui = gui();
        assert!(gui.config.torrent.ask, "asking is on");
        arrive(&mut gui, &path.to_string_lossy());
        assert!(gui.torrent.chooser.is_none(), "no chooser for our own file coming back");
        assert_eq!(gui.active, super::super::TORRENT_NAV, "the torrent is still taken");
        let (words, is_err) = gui.note.clone().unwrap();
        assert!(words.contains("default app"), "{words}");
        assert!(is_err);
    }

    #[test]
    fn the_picker_lists_folders_and_torrents_and_completes() {
        let _en = english();
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
        gui.torrent.picker = Some(Picker { text: Input::new(format!("{base}ve")), listed_for: base.clone(), entries, ..Picker::default() });
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
        let _en = english();
        use crate::setup::picker::Pick;
        let mut gui = ready_gui(&["music"]);
        // The dialog is up: the file's box says so and does nothing.
        gui.torrent.busy = Some(Busy::Picking);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("opening the file picker…") && !text.contains("Choose a .torrent file…"), "the box says so:\n{text}");
        // No dialog could open: the room says so and the typed picker
        // takes over, starting where the dialog would have.
        apply_reply(&mut gui, Reply::Picked(Pick::Unavailable("no session bus".into())));
        assert!(gui.torrent.busy.is_none());
        assert!(gui.torrent.picker.is_some(), "the fallback opened");
        let (words, is_err) = gui.note.clone().unwrap();
        assert!(words.contains("no session bus"), "{words}");
        assert!(!is_err, "a fallback is news, not a failure");
        assert_eq!(tips(&gui), "Tab complete · ↑ ↓ pick · Enter open or choose · Esc close");
        key(&mut gui, KeyCode::Esc);

        // A declined dialog costs nothing.
        gui.torrent.busy = Some(Busy::Picking);
        note(&mut gui, "opening…", false);
        apply_reply(&mut gui, Reply::Picked(Pick::Cancelled));
        assert!(gui.torrent.busy.is_none() && gui.note.is_none() && gui.torrent.picker.is_none());

        // `t` is the typed road on purpose, dialog or not; so is a click
        // on its words under the file's box.
        key(&mut gui, KeyCode::Char('t'));
        assert!(gui.torrent.picker.is_some());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Choose a .torrent file"), "got:\n{text}");
        key(&mut gui, KeyCode::Esc);
        let rows = draw(&mut gui);
        let at = super::super::ytdl::tests::at(&rows, "type a path ▸");
        assert_eq!(gui.ui.hit(at), Some(tor(TorAct::Type)), "the typed road is named under the box");
    }

    #[test]
    fn dropped_paths_and_tildes_read_as_paths() {
        assert_eq!(unescape_dropped("  /tmp/My\\ Album.torrent "), "/tmp/My Album.torrent");
        assert_eq!(unescape_dropped("'/tmp/My Album.torrent'"), "/tmp/My Album.torrent");
        assert_eq!(unescape_dropped("\"/tmp/a.torrent\""), "/tmp/a.torrent");
        if let Some(home) = crate::setup::local_home() {
            let home = home.trim_end_matches(['/', '\\']);
            assert_eq!(expand_tilde("~/a.torrent"), format!("{home}/a.torrent"));
        }
        assert_eq!(expand_tilde("/abs/~/a.torrent"), "/abs/~/a.torrent");
        assert_eq!(basename("/tmp/dir/a.torrent"), "a.torrent");
    }

    #[test]
    fn a_cards_size_reads_as_a_person_says_it() {
        let _en = english();
        assert_eq!(size_words(900), "1 KB");
        assert_eq!(size_words(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
        assert_eq!(size_words(356 * 1024 * 1024), "356 MB");
        assert_eq!(size_words(38 * 1024 * 1024 * 1024 + 512 * 1024 * 1024), "38.5 GB");
        assert_eq!(facts_words((1, 2048)), "1 file · 2 KB");
        assert_eq!(facts_words((12, 356 * 1024 * 1024)), "12 files · 356 MB");
    }

    // ── Every language ─────────────────────────────────────────────────────

    const LOCALES: [&str; 10] = ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

    /// Text without its blanks: a wide glyph's second cell reads as one,
    /// so words are looked for packed.
    fn packed(text: &str) -> String {
        text.replace(' ', "")
    }

    /// The room's cells and the two beside them, row by row: nothing may
    /// stand between the block and the queue's rule.
    fn assert_inside(gui: &mut Gui, (w, h): (u16, u16), what: &str, lang: &str) {
        let buf = buffer(gui, w, h);
        let rule = w - 34;
        for y in 2..h - 6 {
            for x in [rule - 2, rule - 1] {
                assert_eq!(buf[(x, y)].symbol(), " ", "{lang}, {what}: row {y} runs past the block\n{}", lines(&buf).join("\n"));
            }
            assert_ne!(buf[(rule, y)].symbol(), " ", "{lang}, {what}: row {y} wrote over the queue's rule\n{}", lines(&buf).join("\n"));
        }
    }

    #[test]
    fn the_room_stays_inside_its_block_in_every_language() {
        let Some(lang) = crate::setup::tests::in_own_process(&LOCALES) else { return };
        {
            for size in [(100, 30), (100, 24)] {
                let mut gui = canvas();
                assert_inside(&mut gui, size, "the two boxes", lang);
                gui.act(tor(TorAct::Stop(Row::Magnet)));
                type_text(&mut gui, "magnet:?xt=urn:btih:abc");
                assert_inside(&mut gui, size, "a bad link", lang);
                gui.act(tor(TorAct::ClearLink));
                gui.torrent.cursor = None;
                field_notes(&mut gui);
                assert_inside(&mut gui, size, "a file", lang);
                gui.torrent.detect = Some((t!("gui.tor.detect_none").to_string(), Tone::Gold));
                gui.torrent.busy = Some(Busy::Checking);
                assert_inside(&mut gui, size, "a check out", lang);
                gui.torrent.busy = None;
                refuse(&mut gui, t!("gui.tor.pad_files_missing", vpath = "Music", client = "Transmission"));
                assert_inside(&mut gui, size, "the longest refusal", lang);
                gui.torrent.gate = Gate::Answered(TorrentPreflight::default());
                assert_inside(&mut gui, size, "the gate's no", lang);
                paste(&mut gui, MAGNET);
                assert_inside(&mut gui, size, "a link", lang);
            }
        }
    }

    #[test]
    fn the_rows_keep_their_words_and_the_ways_wrap_in_every_language() {
        let Some(lang) = crate::setup::tests::in_own_process(&LOCALES) else { return };
        {
            let mut gui = canvas();
            field_notes(&mut gui);
            let rows = draw(&mut gui);
            let text = rows.join("\n");
            let has = |words: &str| packed(&text).contains(&packed(words));
            // The two rows' labels whole, each with something of its value
            // and its chevron; the ways whole on as many rows as they need.
            for key in ["gui.tor.metadata", "gui.tor.destination", "gui.tor.auto_detect", "gui.tor.open_with", "gui.tor.force_fresh"] {
                let words = t!(key).to_string();
                assert!(has(&words), "{lang}: {words:?} is cut\n{text}");
            }
            let label = packed(&t!("gui.tor.destination"));
            let dest = rows.iter().find(|r| packed(&r.chars().skip(17).take(47).collect::<String>()).starts_with(&label)).unwrap();
            assert!(dest.contains("Music") && dest.contains("Field Notes") && dest.contains('▸'), "{lang}: the library and the folder\n{dest}");
            let submit = format!("{} ▸", t!("gui.tor.submit"));
            assert!(has(&submit), "{lang}: {submit:?}\n{text}");
            let title = t!("gui.tor.list_title").to_uppercase();
            assert!(has(&title), "{lang}: {title:?}\n{text}");
            // The floor: the slab on the title row, the title whole beside it.
            let rows = lines(&buffer(&mut gui, 100, 24));
            let top = packed(&rows[2].chars().skip(17).take(47).collect::<String>());
            assert!(top.starts_with(&packed(&t!("gui.tor.title"))) && top.ends_with(&packed(&submit)), "{lang}: {top:?}");
        }
    }

    #[test]
    fn the_sheets_fit_their_words_in_every_language() {
        let Some(lang) = crate::setup::tests::in_own_process(&LOCALES) else { return };
        {
            let mut gui = canvas();
            field_notes(&mut gui);
            let whole = |gui: &mut Gui, what: &str, keys: &[&str]| {
                let buf = buffer(gui, 100, 30);
                let text = lines(&buf).join("\n");
                for key in keys {
                    let words = t!(*key).to_string();
                    assert!(packed(&text).contains(&packed(&words)), "{lang}, {what}: {words:?} is cut\n{text}");
                }
                // The sheet's frame whole, top to bottom: nothing of it
                // runs through its border.
                let ends = |y: u16| (buf[(17, y)].symbol().to_string(), buf[(82, y)].symbol().to_string());
                let top = (0..30).find(|y| ends(*y) == ("╭".to_string(), "╮".to_string())).unwrap_or_else(|| panic!("{lang}, {what}: no sheet\n{text}"));
                let bottom = (top..30).find(|y| ends(*y) == ("╰".to_string(), "╯".to_string())).unwrap_or_else(|| panic!("{lang}, {what}: no end\n{text}"));
                for y in top + 1..bottom {
                    assert_eq!(ends(y), ("│".to_string(), "│".to_string()), "{lang}, {what}: row {y} runs through the border\n{text}");
                }
            };
            gui.act(tor(TorAct::OpenMetadata(MetaStop::Artist)));
            whole(&mut gui, "Metadata", &["gui.tor.metadata", "gui.tor.artist", "gui.tor.album", "gui.tor.year", "gui.tor.auto_detect", "gui.ytdl.done"]);
            gui.act(tor(TorAct::OpenDestination(DestStop::Path)));
            whole(
                &mut gui,
                "Destination",
                &["gui.tor.destination", "gui.tor.library", "gui.tor.path", "gui.tor.rename_root", "gui.tor.rename_root_desc", "gui.ytdl.done"],
            );
            gui.act(tor(TorAct::SheetClose));
            gui.torrent.matches = Some(Matches { list: two_matches(), row: 0 });
            whole(&mut gui, "Files already exist", &["gui.tor.partial_title", "gui.tor.download_fresh", "gui.tor.submit"]);
            gui.torrent.matches = None;
            gui.torrent.chooser = Some(Chooser { incoming: Incoming::Magnet(MAGNET.into()), row: None, dont_ask: false });
            whole(&mut gui, "Torrent received", &["gui.tor.received_title", "gui.tor.received_add", "gui.tor.open_with", "gui.tor.received_dont_ask"]);
        }
    }

    /// The boards as the build draws them, for a look: `cargo test
    /// dump_torrent_boards -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_torrent_boards() {
        let _en = english();
        let mut gui = canvas();
        field_notes(&mut gui);
        let buf = buffer(&mut gui, 100, 30);
        println!("{}", board_of(&buf, (17, 2), (47, 23), "MAIN"));
        println!("{}", lines(&buf).join("\n"));
    }

    mod boards {
        pub(super) const MAIN: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]… [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                Add torrent ▸                │
            |╰─────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
        "#;
        pub(super) const MAIN_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-40:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "8:0-23:Text", "8:25-25:Dim", "8:27-37:Text",
            "8:39-39:Dim", "8:41-44:Text", "8:46-46:Dim", "9:0-17:Text", "9:19-19:Dim", "9:21-30:Text",
            "9:32-32:Dim", "9:34-44:Text", "9:46-46:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-46:Accent",
            "13:0-0:Accent", "13:17-29:Accent:B", "13:46-46:Accent", "14:0-46:Accent", "16:0-12:Dim", "17:0-0:Ok",
            "17:2-11:Text", "17:25-46:Dim", "18:0-0:Ok", "18:2-8:Text", "18:25-46:Dim", "19:0-0:Ok", "19:2-7:Text",
            "19:25-46:Dim",
        ];
        pub(super) const EMPTY: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│           Choose a .torrent file…           │
            |╰─────────────────────────────────────────────╯
            |                                  type a path ▸
            |───────────────────── or ──────────────────────
            |╭───────────────────────────────────╮ ╭───────╮
            |│ Paste a magnet link               │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const EMPTY_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:12-34:Text", "3:46-46:Dim", "4:0-46:Dim",
            "5:34-46:Dim", "6:0-46:Dim", "7:0-46:Dim", "8:0-46:Dim", "9:0-46:Dim", "11:0-12:Dim", "12:0-0:Ok",
            "12:2-11:Text", "12:25-46:Dim", "13:0-0:Ok", "13:2-8:Text", "13:25-46:Dim", "14:0-0:Ok", "14:2-7:Text",
            "14:25-46:Dim",
        ];
        pub(super) const AFTER: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│           Choose a .torrent file…           │
            |╰─────────────────────────────────────────────╯
            |                                  type a path ▸
            |───────────────────── or ──────────────────────
            |╭───────────────────────────────────╮ ╭───────╮
            |│ Paste a magnet link               │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |FROM TORRENTS
            |✓ Field Notes            Northlight    just now
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const AFTER_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:12-34:Text", "3:46-46:Dim", "4:0-46:Dim",
            "5:34-46:Dim", "6:0-46:Dim", "7:0-46:Dim", "8:0-46:Dim", "9:0-46:Dim", "11:0-12:Dim", "12:0-0:Ok",
            "12:2-12:Text", "12:25-46:Dim", "13:0-0:Ok", "13:2-11:Text", "13:25-46:Dim", "14:0-0:Ok", "14:2-8:Text",
            "14:25-46:Dim", "15:0-0:Ok", "15:2-7:Text", "15:25-46:Dim",
        ];
        pub(super) const MIN: &str = r#"
            |Add torrent                      Add torrent ▸
            |
            |╭─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]… [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |✓ Nocturnes              Halden Row      Oct 27
        "#;
        pub(super) const MIN_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:33-45:OnAccent:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-40:Text:B", "3:42-46:Dim",
            "4:0-46:Dim", "5:0-46:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "8:0-23:Text", "8:25-25:Dim",
            "8:27-37:Text", "8:39-39:Dim", "8:41-44:Text", "8:46-46:Dim", "9:0-17:Text", "9:19-19:Dim",
            "9:21-30:Text", "9:32-32:Dim", "9:34-44:Text", "9:46-46:Dim", "10:0-2:Dim", "10:4-23:Text",
            "12:0-12:Dim", "13:0-0:Ok", "13:2-11:Text", "13:25-46:Dim", "14:0-0:Ok", "14:2-8:Text", "14:25-46:Dim",
            "15:0-0:Ok", "15:2-7:Text", "15:25-46:Dim", "16:0-0:Ok", "16:2-10:Text", "16:25-46:Dim",
        ];
        pub(super) const WIDE: &str = r#"
            |Add torrent                         Transmission · ready
            |
            |╭──────────────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC].torrent   [X] │
            |│ 12 files · 356 MB                                    │
            |╰──────────────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata               Northlight · Field Notes · 2019 ▸
            |Destination           Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭──────────────────────────────────────────────────────╮
            |│                     Add torrent ▸                    │
            |╰──────────────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion                    Vela             1 h ago
            |✓ Cassini                       Vela              Oct 31
            |✓ Walden                        Thoreau           Oct 28
            |✓ Nocturnes                     Halden Row        Oct 27
            |✓ Low Tide Songs                Marrow & Pine     Oct 27
            |✓ The Long Field                Northlight        Oct 25
            |✓ Leaves of Grass               Whitman           Oct 21
            |
            |
            |
        "#;
        pub(super) const WIDE_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:36-55:Dim", "2:0-55:Dim", "3:0-0:Dim", "3:2-47:Text:B", "3:51-55:Dim", "4:0-55:Dim",
            "5:0-55:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "8:0-32:Text", "8:34-34:Dim", "8:36-46:Text",
            "8:48-48:Dim", "8:50-53:Text", "8:55-55:Dim", "9:0-26:Text", "9:28-28:Dim", "9:30-39:Text",
            "9:41-41:Dim", "9:43-53:Text", "9:55-55:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-55:Accent",
            "13:0-0:Accent", "13:22-34:Accent:B", "13:55-55:Accent", "14:0-55:Accent", "16:0-12:Dim", "17:0-0:Ok",
            "17:2-11:Text", "17:32-55:Dim", "18:0-0:Ok", "18:2-8:Text", "18:32-55:Dim", "19:0-0:Ok", "19:2-7:Text",
            "19:32-55:Dim", "20:0-0:Ok", "20:2-10:Text", "20:32-55:Dim", "21:0-0:Ok", "21:2-15:Text", "21:32-55:Dim",
            "22:0-0:Ok", "22:2-15:Text", "22:32-55:Dim", "23:0-0:Ok", "23:2-16:Text", "23:32-55:Dim",
        ];
        pub(super) const MAGNET: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ magnet:?xt=urn:btih:c12fe1c0… [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |Northlight - Field Notes (2019) [FLAC]
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |
            |╭─────────────────────────────────────────────╮
            |│                Add torrent ▸                │
            |╰─────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
            |
            |
        "#;
        pub(super) const MAGNET_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-30:Text", "3:32-46:Dim", "4:0-46:Dim",
            "5:0-37:Dim", "7:0-23:Text", "7:25-25:Dim", "7:27-37:Text", "7:39-39:Dim", "7:41-44:Text", "7:46-46:Dim",
            "8:0-17:Text", "8:19-19:Dim", "8:21-30:Text", "8:32-32:Dim", "8:34-44:Text", "8:46-46:Dim",
            "10:0-46:Accent", "11:0-0:Accent", "11:17-29:Accent:B", "11:46-46:Accent", "12:0-46:Accent",
            "14:0-12:Dim", "15:0-0:Ok", "15:2-11:Text", "15:25-46:Dim", "16:0-0:Ok", "16:2-8:Text", "16:25-46:Dim",
            "17:0-0:Ok", "17:2-7:Text", "17:25-46:Dim",
        ];
        pub(super) const REFUSED: &str = r#"
            |Add torrent
            |Torrents are unavailable on this server.
            |╭─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]… [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Open in another app
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                 Add torrent                 │
            |╰─────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
        "#;
        pub(super) const REFUSED_RUNS: &[&str] = &[
            "0:0-10:Text:B", "1:0-39:Gold", "2:0-46:Dim", "3:0-0:Dim", "3:2-40:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-18:Text", "8:0-23:Text", "8:25-25:Dim", "8:27-37:Text", "8:39-39:Dim", "8:41-44:Text",
            "8:46-46:Dim", "9:0-17:Text", "9:19-19:Dim", "9:21-30:Text", "9:32-32:Dim", "9:34-44:Text",
            "9:46-46:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-46:Dim", "13:0-46:Dim", "14:0-46:Dim", "16:0-12:Dim",
            "17:0-0:Ok", "17:2-11:Text", "17:25-46:Dim", "18:0-0:Ok", "18:2-8:Text", "18:25-46:Dim", "19:0-0:Ok",
            "19:2-7:Text", "19:25-46:Dim",
        ];
        pub(super) const BADLINK: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│           Choose a .torrent file…           │
            |╰─────────────────────────────────────────────╯
            |                                  type a path ▸
            |───────────────────── or ──────────────────────
            |╭───────────────────────────────────╮ ╭───────╮
            |│ magnet:?xt=urn:btih:abc▏      [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |Not a valid magnet link
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const BADLINK_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:12-34:Text", "3:46-46:Dim", "4:0-46:Dim",
            "5:34-46:Dim", "6:0-46:Dim", "7:0-36:Accent", "7:38-46:Dim", "8:0-0:Accent", "8:2-25:Text",
            "8:32-34:Dim", "8:36-36:Accent", "8:38-46:Dim", "9:0-36:Accent", "9:38-46:Dim", "10:0-22:Gold",
            "12:0-12:Dim", "13:0-0:Ok", "13:2-11:Text", "13:25-46:Dim", "14:0-0:Ok", "14:2-8:Text", "14:25-46:Dim",
            "15:0-0:Ok", "15:2-7:Text", "15:25-46:Dim",
        ];
        pub(super) const NODETAILS: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│ download (3).torrent                    [X] │
            |│ 9 files · 88 MB                             │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata                   ! type the details ▸
            |Destination                             Music ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                 Add torrent                 │
            |╰─────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
            |
        "#;
        pub(super) const NODETAILS_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-21:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "8:0-7:Text", "8:27-44:Gold", "8:46-46:Dim",
            "9:0-10:Text", "9:40-46:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-46:Dim", "13:0-46:Dim", "14:0-46:Dim",
            "16:0-12:Dim", "17:0-0:Ok", "17:2-11:Text", "17:25-46:Dim", "18:0-0:Ok", "18:2-8:Text", "18:25-46:Dim",
            "19:0-0:Ok", "19:2-7:Text", "19:25-46:Dim",
        ];
        pub(super) const GUESS: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│ nl-fieldnotes-2019-web-flac.torrent     [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |! Best-effort guess — please verify the fields
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                Add torrent ▸                │
            |╰─────────────────────────────────────────────╯
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
        "#;
        pub(super) const GUESS_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-36:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "7:0-45:Gold", "9:0-23:Text", "9:25-25:Dim",
            "9:27-37:Text", "9:39-39:Dim", "9:41-44:Text", "9:46-46:Dim", "10:0-17:Text", "10:19-19:Dim",
            "10:21-30:Text", "10:32-32:Dim", "10:34-44:Text", "10:46-46:Dim", "11:0-2:Dim", "11:4-23:Text",
            "13:0-46:Accent", "14:0-0:Accent", "14:17-29:Accent:B", "14:46-46:Accent", "15:0-46:Accent",
            "17:0-12:Dim", "18:0-0:Ok", "18:2-11:Text", "18:25-46:Dim", "19:0-0:Ok", "19:2-8:Text", "19:25-46:Dim",
            "20:0-0:Ok", "20:2-7:Text", "20:25-46:Dim",
        ];
        pub(super) const ADDING: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]… [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                   Adding…                   │
            |╰─────────────────────────────────────────────╯
            |checking for files already on the server…
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
            |✓ Walden                 Thoreau         Oct 28
            |
            |
        "#;
        pub(super) const ADDING_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-40:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-41:Dim", "8:0-23:Text", "8:25-25:Dim", "8:27-37:Text", "8:39-39:Dim", "8:41-44:Text",
            "8:46-46:Dim", "9:0-17:Text", "9:19-19:Dim", "9:21-30:Text", "9:32-32:Dim", "9:34-44:Text",
            "9:46-46:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-46:Dim", "13:0-0:Dim", "13:20-26:Accent",
            "13:46-46:Dim", "14:0-46:Dim", "15:0-40:Accent", "17:0-12:Dim", "18:0-0:Ok", "18:2-11:Text",
            "18:25-46:Dim", "19:0-0:Ok", "19:2-8:Text", "19:25-46:Dim", "20:0-0:Ok", "20:2-7:Text", "20:25-46:Dim",
        ];
        pub(super) const FAILED: &str = r#"
            |Add torrent                Transmission · ready
            |
            |╭─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]… [X] │
            |│ 12 files · 356 MB                           │
            |╰─────────────────────────────────────────────╯
            |Auto-detect metadata · Open in another app
            |
            |Metadata      Northlight · Field Notes · 2019 ▸
            |Destination  Music › Northlight › Field Notes ▸
            |[ ] Force fresh download
            |
            |╭─────────────────────────────────────────────╮
            |│                Add torrent ▸                │
            |╰─────────────────────────────────────────────╯
            |All files are already in your “Music” library,
            |but the torrent client's path mapping for it
            |isn't set up. Ask your admin to run auto-detect
            |on the Torrent admin page, then retry.
            |
            |FROM TORRENTS
            |✓ Perihelion             Vela           1 h ago
            |✓ Cassini                Vela            Oct 31
        "#;
        pub(super) const FAILED_RUNS: &[&str] = &[
            "0:0-10:Text:B", "0:27-46:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-40:Text:B", "3:42-46:Dim", "4:0-46:Dim",
            "5:0-46:Dim", "6:0-19:Text", "6:21-21:Dim", "6:23-41:Text", "8:0-23:Text", "8:25-25:Dim", "8:27-37:Text",
            "8:39-39:Dim", "8:41-44:Text", "8:46-46:Dim", "9:0-17:Text", "9:19-19:Dim", "9:21-30:Text",
            "9:32-32:Dim", "9:34-44:Text", "9:46-46:Dim", "10:0-2:Dim", "10:4-23:Text", "12:0-46:Accent",
            "13:0-0:Accent", "13:17-29:Accent:B", "13:46-46:Accent", "14:0-46:Accent", "15:0-45:Gold",
            "16:0-43:Gold", "17:0-46:Gold", "18:0-37:Gold", "20:0-12:Dim", "21:0-0:Ok", "21:2-11:Text",
            "21:25-46:Dim", "22:0-0:Ok", "22:2-8:Text", "22:25-46:Dim",
        ];
        pub(super) const METADATA: &str = r#"
            |╭─ Metadata ─────────────────────────────────────────────────────╮
            |│ ARTIST                        ALBUM                  YEAR      │
            |│ ╭──────────────────────────╮  ╭───────────────────╮  ╭───────╮ │
            |│ │ Northlight▏              │  │ Field Notes       │  │ 2019  │ │
            |│ ╰──────────────────────────╯  ╰───────────────────╯  ╰───────╯ │
            |│                                                                │
            |│ Metadata detected                                              │
            |│                                                                │
            |│ Auto-detect metadata                                 [ Done ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const METADATA_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-10:Text:B", "0:12-65:Accent", "1:0-0:Accent", "1:2-58:Dim", "1:65-65:Accent",
            "2:0-29:Accent", "2:32-63:Dim", "2:65-65:Accent", "3:0-2:Accent", "3:4-14:Text", "3:29-29:Accent",
            "3:32-32:Dim", "3:34-44:Text", "3:52-55:Dim", "3:57-60:Text", "3:63-63:Dim", "3:65-65:Accent",
            "4:0-29:Accent", "4:32-63:Dim", "4:65-65:Accent", "5:0-65:Accent", "6:0-0:Accent", "6:2-18:Ok",
            "6:65-65:Accent", "7:0-65:Accent", "8:0-0:Accent", "8:2-21:Text", "8:55-62:Accent:B", "8:65-65:Accent",
            "9:0-65:Accent",
        ];
        pub(super) const DESTINATION: &str = r#"
            |╭─ Destination · Music › Northlight › Field Notes ───────────────╮
            |│ LIBRARY                                                        │
            |│ (•) Music   ( ) Audiobooks                                     │
            |│                                                                │
            |│ PATH                                                           │
            |│ ╭────────────────────────────────────────────────────────────╮ │
            |│ │ Northlight/Field Notes▏                                    │ │
            |│ ╰────────────────────────────────────────────────────────────╯ │
            |│ /Music/Northlight/Field Notes/‹torrent contents›               │
            |│ Path in the library — a hand edit sticks until a new source    │
            |│                                                                │
            |│ [✓] Rename the torrent's root folder                           │
            |│     Match the destination folder name                          │
            |│                                                                │
            |│                                                      [ Done ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const DESTINATION_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-13:Text:B", "0:15-15:Dim", "0:17-21:Text", "0:23-23:Dim", "0:25-34:Text",
            "0:36-36:Dim", "0:38-48:Text", "0:50-65:Accent", "1:0-0:Accent", "1:2-8:Dim", "1:65-65:Accent",
            "2:0-0:Accent", "2:2-4:Ok", "2:6-10:Text:B", "2:14-16:Dim", "2:18-27:Text", "2:65-65:Accent",
            "3:0-65:Accent", "4:0-0:Accent", "4:2-5:Dim", "4:65-65:Accent", "5:0-65:Accent", "6:0-2:Accent",
            "6:4-26:Text", "6:63-65:Accent", "7:0-65:Accent", "8:0-0:Accent", "8:2-49:Dim", "8:65-65:Accent",
            "9:0-0:Accent", "9:2-60:Dim", "9:65-65:Accent", "10:0-65:Accent", "11:0-0:Accent", "11:2-4:Ok",
            "11:6-37:Text", "11:65-65:Accent", "12:0-0:Accent", "12:6-38:Dim", "12:65-65:Accent", "13:0-65:Accent",
            "14:0-0:Accent", "14:55-62:Accent:B", "14:65-65:Accent", "15:0-65:Accent",
        ];
        pub(super) const MATCHES: &str = r#"
            |╭─ Some files already exist ─────────────────────────────────────╮
            |│ Point the torrent at an existing copy to seed it and download  │
            |│ only what is missing.                                          │
            |│                                                                │
            |│ (•) Music › _incoming › Field Notes [FLAC]                     │
            |│     10/12 files here · 2 to download                           │
            |│ ( ) Audiobooks › misc › Field Notes                            │
            |│     3/12 files here · 9 to download                            │
            |│ ( ) Download fresh anyway                                      │
            |│     Music › Northlight › Field Notes                           │
            |│                                                                │
            |│                                               [ Add torrent ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const MATCHES_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-26:Text:B", "0:28-65:Accent", "1:0-0:Accent", "1:2-62:Dim", "1:65-65:Accent",
            "2:0-0:Accent", "2:2-22:Dim", "2:65-65:Accent", "3:0-65:Accent", "4:0-4:Accent", "4:6-10:Text:B",
            "4:12-12:Dim", "4:14-22:Text:B", "4:24-24:Dim", "4:26-43:Text:B", "4:65-65:Accent", "5:0-0:Accent",
            "5:6-37:Dim", "5:65-65:Accent", "6:0-0:Accent", "6:2-4:Dim", "6:6-15:Text", "6:17-17:Dim",
            "6:19-22:Text", "6:24-24:Dim", "6:26-36:Text", "6:65-65:Accent", "7:0-0:Accent", "7:6-36:Dim",
            "7:65-65:Accent", "8:0-0:Accent", "8:2-4:Dim", "8:6-26:Text", "8:65-65:Accent", "9:0-0:Accent",
            "9:6-37:Dim", "9:65-65:Accent", "10:0-65:Accent", "11:0-0:Accent", "11:48-62:Accent:B",
            "11:65-65:Accent", "12:0-65:Accent",
        ];
        pub(super) const RECEIVED: &str = r#"
            |╭─ Torrent received ─────────────────────────────────────────────╮
            |│ Northlight - Field Notes (2019) [FLAC]                         │
            |│ Add it to a library on your mStream server, or hand it to      │
            |│ another app.                                                   │
            |│                                                                │
            |│ ╭─────────────────────────────╮ ╭────────────────────────────╮ │
            |│ │       Add to mStream ▸      │ │     Open in another app    │ │
            |│ ╰─────────────────────────────╯ ╰────────────────────────────╯ │
            |│                                                                │
            |│ [ ] Always add to mStream, don't ask again                     │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const RECEIVED_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-18:Text:B", "0:20-65:Accent", "1:0-0:Accent", "1:2-39:Text:B", "1:65-65:Accent",
            "2:0-0:Accent", "2:2-58:Dim", "2:65-65:Accent", "3:0-0:Accent", "3:2-13:Dim", "3:65-65:Accent",
            "4:0-65:Accent", "5:0-32:Accent", "5:34-63:Dim", "5:65-65:Accent", "6:0-2:Accent", "6:10-25:Accent:B",
            "6:32-32:Accent", "6:34-34:Dim", "6:40-58:Text", "6:63-63:Dim", "6:65-65:Accent", "7:0-32:Accent",
            "7:34-63:Dim", "7:65-65:Accent", "8:0-65:Accent", "9:0-0:Accent", "9:2-4:Dim", "9:6-43:Text",
            "9:65-65:Accent", "10:0-65:Accent",
        ];
        pub(super) const PICKER: &str = r#"
            |╭─ Choose a .torrent file ───────────────────────────────────────╮
            |│ ╭────────────────────────────────────────────────────────────╮ │
            |│ │ ~/Downloads/▏                                              │ │
            |│ ╰────────────────────────────────────────────────────────────╯ │
            |│ ▸ Music/                                                       │
            |│ ▸ old/                                                         │
            |│   Northlight - Field Notes (2019) [FLAC].torrent               │
            |│   ubuntu-26.10-desktop-amd64.iso.torrent                       │
            |│                                                                │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const PICKER_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-24:Text:B", "0:26-65:Accent", "1:0-65:Accent", "2:0-2:Accent", "2:4-16:Text",
            "2:63-65:Accent", "3:0-65:Accent", "4:0-0:Accent", "4:2-9:Dim", "4:65-65:Accent", "5:0-0:Accent",
            "5:2-7:Dim", "5:65-65:Accent", "6:0-0:Accent", "6:4-49:Text", "6:65-65:Accent", "7:0-0:Accent",
            "7:4-41:Text", "7:65-65:Accent", "8:0-65:Accent", "9:0-65:Accent",
        ];
    }
}
