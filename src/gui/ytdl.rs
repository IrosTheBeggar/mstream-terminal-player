//! Add from YouTube in the GUI: the nav's Add YouTube room — a link
//! pasted in, the video looked up, the song it makes shown as a card
//! with its cover, where it saves and as what on two rows, one primary
//! that adds it to a library — and, under the form, the From YouTube
//! list: the adds this player started, followed until they settle, then
//! the server's record of what came in. The Files door and its window
//! ([`super::ytdl_here`]), the sheets ([`super::ytdl_sheets`]: Edit tags,
//! Format) and Saves to with its Choose a folder ([`super::ytdl_where`])
//! share this module's tracker, its form ([`Draft`], read through a
//! [`Form`]), its rows ([`form_rows`], [`draw_row`]) and its paste seam.
//!
//! Design: the "Add from YouTube · GUI" canvas (2026-11-10; its boards
//! are the tests' — Main, Empty, After, Min, Wide and the states). One
//! block, centred, at most [`BLOCK_MAX`] cells wide; the link in a
//! bordered card with `Paste` beside it where the build can read the
//! clipboard; the found song a card (cover, title, artist, album and
//! year, YouTube's own line); `Saves to` and `Format` rows that each open
//! one sheet; one ACCENT primary Enter presses, tall and the block's
//! width under the song; the list in the room. The page's controls are
//! words with `▸`, never a key's letter: the letters (`e`, `d`, `f`, …)
//! still act where their control is drawn, and the tips line names them.
//! The rules are [`crate::ytdl`]'s and the jobs
//! [`crate::ytdl::jobs::Tracker`]'s; this module puts them on the screen.
//!
//! Network: the torrent room's shape — every call a one-shot [`Client`] on
//! a thread of its own, built from the live session, its answer folded
//! back between draws by [`poll`]. The tracker's calls go out the same way
//! through its [`Wire`], whose channel goes with the tracker when the
//! session moves to another server or account.
//!
//! Paste: while the room or the Files window is up the terminal is in
//! bracketed paste (`Host::paste`; the window's `Face::takes_paste`), so
//! a paste lands
//! whole in the link field — replacing the link and looking it up —
//! wherever the keys are. Windows' console delivers no paste event: a
//! paste is typed there, so the field keeps the keys after a find
//! ([`YtUi::whole_pastes`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use serde_json::Value;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::Client;
use crate::api::types::{Ping, Track, TrackMetadata};
use crate::tui::art::Art;
use crate::api::ytdl::{LOOKUP_BOUND, YtdlCallError, YtdlListing, YtdlMetadata};
use crate::config;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, width};
use crate::tui::app::{Focus, MessageKind, Tab};
use crate::tui::worker::{ApiCmd, Event};
use crate::ytdl::jobs::{
    AddedRow, Answer, Call, CallId, Clock, Job, JobEvent, JobState, Note, Origin, Redo, SlotStore, StartOutcome,
    StartRequest, Store, Tracker, Transport, WallClock, Way,
};
use crate::ytdl::{
    AddBlock, AddState, Choice, Failure, FolderCheck, Guess, GuessState, LayoutKind, Link, LinkKind, Listing,
    LookupState, Presets, Problem, Remembered, Surface, TagField, Tags, Target, Tier, Tone, Words, add_block,
    add_button_label, block_label, check_folders, choice_target, classify_lookup, failure_label, folder_target,
    format_name, format_options, guess, link_notes, lookup_state_of, name_check, parse_link, path_crumbs,
    remembered_choice, year_invalid,
};

use super::keep::{self, crumb_sep, fit_note};
use super::{Act, Gui, Screen, accent, bright_bold, put, sel, text_button};

/// A typed link is looked up once the typing rests this long.
const DEBOUNCE_MS: i64 = 400;
/// "Still looking — YouTube can be slow." after this long.
const SLOW_MS: i64 = 8_000;
/// A paste is one line of at most this many characters.
const PASTE_MAX: usize = 4096;
/// The block the room draws in: the room's width up to this many cells,
/// centred in what is left (the canvas's Wide board; the floor's room is
/// [`BLOCK_MIN`]).
pub(super) const BLOCK_MAX: u16 = 56;
/// The room at the 100 × 24 floor with the queue open: what the list's
/// columns are measured from.
const BLOCK_MIN: u16 = 47;
/// The song's cover: 8 × 4 cells, the bar card's size.
pub(super) const COVER_W: u16 = 8;
pub(super) const COVER_H: u16 = 4;
/// The list wants this many rows under the form: the form's blanks give
/// way for them (the canvas's Min board).
const LIST_WANTS: usize = 3;
/// A cover asked for is kept for this many videos.
const THUMBS_KEPT: usize = 12;
/// YouTube's image host answers a thumbnail in a blink; one that hangs is
/// given up, and one this big is no thumbnail.
const THUMB_BOUND: std::time::Duration = std::time::Duration::from_secs(10);
const THUMB_MAX: usize = 2 * 1024 * 1024;

// ── Acts ────────────────────────────────────────────────────────────────────

/// Everything a click on Add from YouTube's surfaces means.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum YtAct {
    /// The Files door's `! failed`: the room, where the list is.
    List,
    /// A click on the link card: the field takes the keys.
    Field,
    /// The card's `[X]`: the link cleared, the field holding the keys.
    Clear,
    /// `Paste`: the clipboard into the link field, looked up at once.
    PasteClip,
    /// A tag's card in the Edit tags sheet, clicked: it takes the keys.
    Tag(TagField),
    /// The form's controls, by what they do (and the letter that does it:
    /// `e`, `d`, `f`, `a`, `t`, `u`, `u`, `i`, `c`).
    Edit,
    Change,
    Format,
    Retry,
    Type,
    Use,
    Undo,
    Instead,
    Choose,
    /// The Edit tags sheet's `[ Done ]` (Enter, Esc, a click outside it).
    TagsDone,
    /// The primary.
    Add,
    /// The Format sheet: a format's radio, `[ Done ]`, and Esc or a click
    /// outside it (the format stays as it was).
    FormatPick(String),
    FormatDone,
    FormatClose,
    /// A song row's click and its hover verbs: play, add to the queue, its list.
    Play(usize),
    Queue(usize),
    More(usize),
    /// A row's way on (Try again, Dismiss, …), by job.
    Way(String, Way),
    /// The song list's choices: Add to queue, Show in folder.
    MenuPick(usize),
    MenuClose,
    ShowEarlier,
    ScrollBy(i32),
    /// The Files door and its window ([`super::ytdl_here`]).
    Here(super::ytdl_here::HereAct),
    /// Saves to and Choose a folder ([`super::ytdl_where`]).
    Place(super::ytdl_where::PlaceAct),
}

pub(super) fn yt(act: YtAct) -> Act {
    Act::Yt(act)
}

// ── What the server said, and where the jobs live ──────────────────────────

/// What the session's server said of Add from YouTube in its ping, caught
/// as the Connected lands (the App's `Capabilities` carries none of it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The identity the ping answered for (Connected's `id`).
    pub identity: String,
    /// `ytdl`: 1 is the new job list.
    pub generation: u32,
    /// `noUpload` false: the account may add files.
    pub uploads: bool,
    /// The server has audio conversion to offer.
    pub transcode: bool,
    pub no_mkdir: bool,
    /// `supportedAudioFiles`, read.
    pub formats: Option<Vec<String>>,
}

impl Facts {
    fn of(identity: &str, ping: &Ping) -> Facts {
        Facts {
            identity: identity.to_string(),
            generation: ping.ytdl,
            uploads: ping.no_upload == Some(false),
            transcode: ping.transcode.is_some(),
            no_mkdir: ping.no_mkdir == Some(true),
            formats: ping.supported_formats.clone(),
        }
    }
}

/// The room's and the tracker's clock: the wall's, or a test's hand on it.
#[derive(Clone, Default)]
pub(crate) struct YtClock(Option<Rc<Cell<i64>>>);

impl Clock for YtClock {
    fn now_ms(&self) -> i64 {
        match &self.0 {
            Some(hand) => hand.get(),
            None => WallClock.now_ms(),
        }
    }
}

/// Where the jobs are kept: `ytdl.json`'s slot for the server and account
/// — or, under test, memory, so no GUI test writes a jobs file.
pub(crate) enum YtStore {
    Slot(SlotStore),
    Memory(Rc<RefCell<Option<Value>>>),
}

impl Store for YtStore {
    fn load(&mut self) -> Result<Option<Value>, String> {
        match self {
            YtStore::Slot(slot) => slot.load(),
            YtStore::Memory(kept) => Ok(kept.borrow().clone()),
        }
    }

    fn save(&mut self, slot: &Value) -> Result<(), String> {
        match self {
            YtStore::Slot(store) => store.save(slot),
            YtStore::Memory(kept) => {
                *kept.borrow_mut() = Some(slot.clone());
                Ok(())
            }
        }
    }
}

/// The tracker's [`Transport`]: a call is queued here during the
/// tracker's turn, and [`poll`] sends each on a thread of its own with a
/// fresh client for the session once the turn is over (the torrent room's
/// way); the answers come back on the wire's own channel, which goes with
/// the tracker.
#[derive(Clone)]
pub(crate) struct Wire(Rc<RefCell<WireState>>);

struct WireState {
    out: Vec<(CallId, Call)>,
    tx: Sender<(CallId, Answer)>,
    rx: Receiver<(CallId, Answer)>,
}

impl Wire {
    fn new() -> Wire {
        let (tx, rx) = channel();
        Wire(Rc::new(RefCell::new(WireState { out: Vec::new(), tx, rx })))
    }

    /// The calls the tracker made since the last turn.
    pub(crate) fn take_out(&self) -> Vec<(CallId, Call)> {
        std::mem::take(&mut self.0.borrow_mut().out)
    }

    /// Where a call's answer goes.
    pub(crate) fn sender(&self) -> Sender<(CallId, Answer)> {
        self.0.borrow().tx.clone()
    }
}

impl Transport for Wire {
    fn send(&mut self, id: CallId, call: Call) {
        self.0.borrow_mut().out.push((id, call));
    }

    fn answers(&mut self) -> Vec<(CallId, Answer)> {
        self.0.borrow().rx.try_iter().collect()
    }
}

pub(crate) type Jobs = Tracker<Wire, YtClock, YtStore>;

/// The tracker for one server identity and account, and its wire.
pub(crate) struct Held {
    identity: String,
    username: Option<String>,
    pub jobs: Jobs,
    pub wire: Wire,
}

/// Work beside the tracker's calls, each on a thread of its own.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Work {
    /// GET /ytdl/metadata for the canonical link.
    Lookup { seq: u64, url: String },
    /// The new-folder walk for a target, and the target's own files.
    Folders { seq: u64, target: Target },
    /// The ping again: a flag it gave looks stale.
    Ping { identity: String },
    /// Saves to's own presets: the account's Discover
    /// destination (asked only where the server has Discover's plug-ins)
    /// and the libraries' templates.
    Presets { identity: String, discover: bool },
    /// One folder's names, for Choose a folder.
    Listing { seq: u64, directory: String },
    /// A video's thumbnail, from YouTube's image host, for the song's
    /// cover: by the video's id, never by an address the server gave.
    Thumb { id: String },
}

/// What a thread sends back.
pub(crate) enum Reply {
    Lookup { seq: u64, result: Result<YtdlMetadata, YtdlCallError> },
    Folders { seq: u64, check: FolderCheck },
    Pinged { identity: String, ping: Option<Ping> },
    Presets { identity: String, presets: Presets },
    Listing { seq: u64, result: Result<YtdlListing, YtdlCallError> },
    Thumb { id: String, art: Option<Art> },
}

// ── The form ────────────────────────────────────────────────────────────────

/// Who has the form's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keys {
    /// Nothing: the controls' letters act, and ↑ ↓ walk the list.
    None,
    Link,
    /// A tag's card in the Edit tags sheet.
    Tag(TagField),
}

/// The lookup, as the form shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Look {
    /// Not asked: no link, not a video, or the typing has not rested yet.
    Idle,
    /// Out since `since` (ms) for `url`, as request `seq`.
    Reading { seq: u64, url: String, since: i64 },
    Found { url: String, meta: Box<YtdlMetadata> },
    Failed { url: String, failure: Failure },
}

/// The new-folder check for a folder: its request and, once answered,
/// what it found.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FolderLook {
    pub(super) directory: String,
    pub(super) seq: u64,
    pub(super) check: Option<FolderCheck>,
}

/// The new-folder check for `target`, once the typing rests: the first
/// folder shown is checked at once, one the typing moves waits for the
/// typing to rest, as the lookup does. `folder` is the check out or
/// answered, `due` the folder waiting and when. The work to send, if due.
pub(super) fn watch_folder(
    folder: &mut Option<FolderLook>,
    due: &mut Option<(String, i64)>,
    target: &Target,
    now: i64,
    seqs: &mut u64,
) -> Option<Work> {
    let directory = target.directory();
    if directory.is_empty() || folder.as_ref().is_some_and(|f| f.directory == directory) {
        *due = None;
        return None;
    }
    match due {
        Some((dir, at)) if *dir == directory && now >= *at => {}
        Some((dir, _)) if *dir == directory => return None,
        _ => {
            let at = if folder.is_none() { now } else { now + DEBOUNCE_MS };
            *due = Some((directory.clone(), at));
            if at > now {
                return None;
            }
        }
    }
    *due = None;
    *seqs += 1;
    let seq = *seqs;
    *folder = Some(FolderLook { directory, seq, check: None });
    Some(Work::Folders { seq, target: target.clone() })
}

/// What the check said of `target`, while it is the target's.
pub(super) fn checked_for<'a>(folder: &'a Option<FolderLook>, target: &Target) -> Option<&'a FolderCheck> {
    let directory = target.directory();
    folder.as_ref().filter(|f| f.directory == directory).and_then(|f| f.check.as_ref())
}

const TAGS: [TagField; 4] = [TagField::Title, TagField::Artist, TagField::Album, TagField::Year];

fn tag_index(field: TagField) -> usize {
    TAGS.iter().position(|f| *f == field).unwrap_or(0)
}

/// The form Add from YouTube fills: the link, the lookup, the tags and
/// where the song goes. The room holds one; the Files window another.
pub(crate) struct Draft {
    pub link: Input,
    pub keys: Keys,
    /// The link as read.
    pub(super) parsed: Link,
    /// When a typed link is looked up (the debounce).
    due: Option<i64>,
    pub look: Look,
    /// An answer that came after `type the details`: offered with `use
    /// them`, never poured over what was typed.
    pub(super) late: Option<Box<YtdlMetadata>>,
    /// The details are typed by hand.
    pub(super) manual: bool,
    /// The Edit tags sheet is up over the form (`edit tags`, `type the
    /// details`, a trap's way on).
    pub(super) tags_open: bool,
    pub(super) guess: Guess,
    pub(super) gstate: GuessState,
    pub(super) fields: [Input; 4],
    pub(super) edited: [bool; 4],
    /// The format picked for this song; else the server's remembered one.
    pub(super) codec: Option<String>,
    pub(super) folder: Option<FolderLook>,
    /// The folder the tags last made, and when: its check waits for the
    /// typing to rest, as the lookup does.
    pub(super) folder_due: Option<(String, i64)>,
    /// "i save to … instead", Choose a folder: the folder that exists, for
    /// this song.
    pub(super) instead: Option<String>,
    /// Saves to's `use for this song`: the choice this song goes by, not
    /// remembered.
    pub(super) choice: Option<Choice>,
    /// The server refused the add while the form was up.
    pub(super) refusal: Option<Failure>,
    /// The add pressed and not answered yet: its job.
    pub starting: Option<String>,
}

impl Default for Draft {
    fn default() -> Draft {
        Draft {
            link: Input::default(),
            keys: Keys::Link,
            parsed: parse_link(""),
            due: None,
            look: Look::Idle,
            late: None,
            manual: false,
            tags_open: false,
            guess: Guess::none(),
            gstate: GuessState::default(),
            fields: Default::default(),
            edited: [false; 4],
            codec: None,
            folder: None,
            folder_due: None,
            instead: None,
            choice: None,
            refusal: None,
            starting: None,
        }
    }
}

impl Draft {
    /// The four tags as typed.
    pub(crate) fn tags(&self) -> Tags {
        let v = |i: usize| self.fields[i].value().to_string();
        Tags { title: v(0), artist: v(1), album: v(2), year: v(3) }
    }

    /// The video's own answer, once found.
    pub(super) fn meta(&self) -> Option<&YtdlMetadata> {
        match &self.look {
            Look::Found { meta, .. } => Some(meta),
            _ => None,
        }
    }

    pub(super) fn found(&self) -> bool {
        matches!(self.look, Look::Found { .. })
    }

    /// Where the lookup stands at `now` (the rules' reading).
    pub(super) fn lookup_state(&self, now: i64) -> LookupState {
        match &self.look {
            Look::Idle => LookupState::Idle,
            Look::Reading { since, .. } if now - since >= SLOW_MS => LookupState::Slow,
            Look::Reading { .. } => LookupState::Loading,
            Look::Found { .. } => LookupState::Found,
            Look::Failed { failure, .. } => lookup_state_of(failure),
        }
    }

    /// The link was replaced or edited: a new link starts the form over
    /// (who has the keys aside) and is looked up once the typing rests —
    /// a paste and Enter ask at once ([`ask`]).
    pub(super) fn link_changed(&mut self, now: i64) {
        let parsed = parse_link(self.link.value());
        let same = parsed.url.is_some() && parsed.url == self.parsed.url;
        self.parsed = parsed;
        if same {
            return;
        }
        let keys = self.keys;
        let link = std::mem::take(&mut self.link);
        let parsed = std::mem::replace(&mut self.parsed, parse_link(""));
        *self = Draft { link, keys, parsed, ..Draft::default() };
        if self.parsed.kind == LinkKind::Video {
            self.due = Some(now + DEBOUNCE_MS);
        }
    }

    /// Fill the tags from the guess: every field not typed in, or only the
    /// empty ones ("u use them").
    fn fill(&mut self, only_empty: bool) {
        let (title, artist) = self.guess.fields(&self.gstate);
        let values = [title, artist, self.guess.album.clone(), self.guess.year.clone()];
        for (i, value) in values.into_iter().enumerate() {
            let empty = self.fields[i].value().trim().is_empty();
            if (only_empty && !empty) || (!only_empty && self.edited[i]) {
                continue;
            }
            self.fields[i] = Input::new(value);
        }
    }

    /// A lookup's answer: the guess, the tags it gives.
    fn take_answer(&mut self, url: String, meta: YtdlMetadata) {
        self.guess = read_guess(&meta);
        self.gstate = GuessState::default();
        self.fill(false);
        self.look = Look::Found { url, meta: Box::new(meta) };
    }

    /// Ask what the link is, as request `seq` (from `seqs`): the work to
    /// send, or none when the link is no video.
    pub(crate) fn ask(&mut self, now: i64, seqs: &mut u64) -> Option<Work> {
        let url = self.parsed.url.clone().filter(|_| self.parsed.kind == LinkKind::Video)?;
        *seqs += 1;
        let seq = *seqs;
        self.due = None;
        self.look = Look::Reading { seq, url: url.clone(), since: now };
        Some(Work::Lookup { seq, url })
    }

    /// Request `seq`'s answer, if it is this form's (a link since replaced
    /// has none). `whole`: a paste arrives whole, so a find gives the
    /// field's keys back. True when it was this form's.
    pub(crate) fn answered(&mut self, seq: u64, result: Result<YtdlMetadata, YtdlCallError>, whole: bool) -> bool {
        let Look::Reading { seq: asked, url, .. } = &self.look else { return false };
        if *asked != seq {
            return false;
        }
        let url = url.clone();
        match result {
            // Typed by hand meanwhile: the answer is offered, not poured in.
            Ok(meta) if self.manual => {
                self.look = Look::Idle;
                self.late = Some(Box::new(meta));
            }
            Ok(meta) => {
                self.take_answer(url, meta);
                // Found: the field gives the keys back, so e, d, f and
                // Enter work — where a paste comes whole.
                if self.keys == Keys::Link && whole {
                    self.keys = Keys::None;
                }
            }
            Err(e) => {
                // Not sent at all — no session to send it on — is a server
                // the player can't reach, as the boards word it.
                let failure = match e {
                    YtdlCallError::Config(_) => classify_lookup(&crate::ytdl::CallFailure::NeverConnected),
                    e => classify_lookup(&crate::ytdl::jobs::call_failure(&e)),
                };
                if self.manual {
                    self.look = Look::Idle;
                } else if lookup_state_of(&failure) == LookupState::Failed {
                    // A video the server couldn't read: the details are
                    // the user's to type (`type the details`), and Add
                    // works without them — the file gets the video's own.
                    self.manual = true;
                    if self.keys == Keys::Link && whole {
                        self.keys = Keys::None;
                    }
                    self.look = Look::Failed { url, failure };
                } else {
                    if self.keys == Keys::Link && whole {
                        self.keys = Keys::None;
                    }
                    self.look = Look::Failed { url, failure };
                }
            }
        }
        true
    }

    /// The form's timers at `now`: the typed link's debounce (the lookup
    /// to send), and the lookup's own bound — past it with no answer the
    /// server took too long (a thread that never comes back can't hold
    /// the form). True when the form changed.
    pub(super) fn timers(&mut self, now: i64, seqs: &mut u64) -> (bool, Option<Work>) {
        let mut work = None;
        let mut changed = false;
        if self.due.is_some_and(|due| now >= due) {
            work = self.ask(now, seqs);
            changed = true;
        }
        let bound = LOOKUP_BOUND.as_millis() as i64;
        if let Look::Reading { url, since, .. } = &self.look
            && now - since >= bound + 1_000
        {
            let url = url.clone();
            self.look = if self.manual {
                Look::Idle
            } else {
                Look::Failed { url, failure: classify_lookup(&crate::ytdl::CallFailure::TimedOut) }
            };
            changed = true;
        }
        (changed, work)
    }

    /// The new-folder check for `target`, once the tags rest: the first
    /// folder a song shows is checked at once, one the typing moves waits
    /// for the typing to rest. The work to send, if it's due.
    pub(crate) fn folder_work(&mut self, target: &Target, now: i64, seqs: &mut u64) -> Option<Work> {
        watch_folder(&mut self.folder, &mut self.folder_due, target, now, seqs)
    }

    /// Request `seq`'s folder check, if it is this form's.
    pub(super) fn checked(&mut self, seq: u64, check: FolderCheck) {
        if let Some(folder) = self.folder.as_mut().filter(|f| f.seq == seq) {
            folder.check = Some(check);
        }
    }
}

fn read_guess(meta: &YtdlMetadata) -> Guess {
    guess(crate::ytdl::Lookup {
        title: meta.title.as_deref(),
        artist: meta.artist.as_deref(),
        album: meta.album.as_deref(),
        year: meta.year.as_deref(),
    })
}

// ── The list's own state ────────────────────────────────────────────────────

/// One item of the From YouTube list: a job of this player's, or a song
/// the Added list holds.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Job(String),
    Song(AddedRow),
}

/// The From YouTube list, under the room's form.
#[derive(Default)]
pub(crate) struct ListUi {
    /// The first item shown, and the keyboard's item (the kit's
    /// list-cursor law, in items).
    view: ListView,
    held: Option<usize>,
    /// A song's list ([⋯], a right click, `m`): its item, and the cursor
    /// once a key walks it (opened by the pointer it has none).
    menu: Option<(usize, Option<usize>)>,
    /// What the last frame listed, for the keys.
    items: Vec<Item>,
    /// The rows the last frame had for it.
    rows: usize,
    /// Where the last frame drew the `[⋯]` of the song whose list is up:
    /// the list hangs from it.
    menu_at: Option<Rect>,
}

/// The Format sheet ([`super::ytdl_sheets`]): the dot on the format in
/// the making, taken by `[ Done ]`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FormatSheet {
    pub(super) cursor: usize,
    /// The row it retries in the format taken; `None`: the song on screen.
    pub(super) job: Option<String>,
}

/// A video's cover, by its id: asked, here, or not to be had.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Thumb {
    Asked,
    Got(Art),
    None,
}

/// Add from YouTube's state on the Gui.
pub(crate) struct YtUi {
    /// The ping's facts for the session's server.
    facts: Option<Facts>,
    /// The jobs of the session's server and account.
    pub held: Option<Held>,
    /// The room's form.
    pub draft: Draft,
    /// Where new songs save, beyond the fixed presets: Discover's own
    /// destination and the libraries' templates, once read (Saves to asks
    /// for them).
    pub presets: Option<Presets>,
    list: ListUi,
    /// The tracker was told the room's list is showing.
    watching: bool,
    pub(super) formats: Option<FormatSheet>,
    /// The Files door, its window and the folder it re-lists.
    pub(super) here: super::ytdl_here::HereUi,
    /// Saves to or Choose a folder, while one is up.
    pub(super) place: Option<super::ytdl_where::Place>,
    /// The controls the last frame drew, by letter: a key acts only
    /// where its control stood.
    pub(super) drawn: Vec<(char, YtAct)>,
    /// A press is out (the window's `[ Adding… ]`): the controls show
    /// where they stand but neither click nor light.
    pub(super) quiet: bool,
    /// The clipboard's text, where this build can read it: the GUI's own
    /// window (`GuiFace::new`). `None` in a terminal — which is why
    /// `Paste` is drawn only there.
    pub clipboard: Option<fn() -> Option<String>>,
    /// The covers asked for, by video id, the newest last.
    thumbs: Vec<(String, Thumb)>,
    /// The ids asked for under test, where no thread runs.
    pub thumb_asks: Vec<String>,
    /// The cover's slots: the room's and the Files window's.
    slots: [Option<super::cover::Slot>; 2],
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    pub clock: YtClock,
    /// Requests and pings out.
    pub(super) seq: u64,
    out: usize,
    /// Tests: the work stays here for the test to answer — nothing reaches
    /// a network.
    hand: bool,
    pub work: Vec<Work>,
    /// A paste arrives whole (bracketed paste, or the window's clipboard):
    /// the link field gives the keys back once a song is found. Not on
    /// Windows' console, where a paste is typed — the field keeps them.
    /// (Add torrent's room goes by it too: where a paste is typed it opens
    /// with the hand on its link box.)
    pub whole_pastes: bool,
    /// The local zone, for the list's dates; read once.
    zone: Option<Option<crate::admin::tz::Zone>>,
    /// Show in folder: the folder Files lists next, and the file to rest
    /// the cursor on once it has.
    seat: Option<(String, String)>,
    /// The status line's "Adding …" and the job it is for: an end with no
    /// words of its own (not sure, not found) takes it down.
    adding_note: Option<(String, String)>,
}

impl YtUi {
    pub(crate) fn new() -> YtUi {
        let (tx, rx) = channel();
        YtUi {
            facts: None,
            held: None,
            draft: Draft::default(),
            presets: None,
            list: ListUi::default(),
            watching: false,
            formats: None,
            here: super::ytdl_here::HereUi::default(),
            place: None,
            drawn: Vec::new(),
            quiet: false,
            clipboard: None,
            thumbs: Vec::new(),
            thumb_asks: Vec::new(),
            slots: [None, None],
            tx,
            rx,
            clock: YtClock::default(),
            seq: 0,
            out: 0,
            hand: cfg!(test),
            work: Vec::new(),
            whole_pastes: !cfg!(windows),
            zone: None,
            seat: None,
            adding_note: None,
        }
    }

    pub(super) fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// A test's hand on the clock and the zone: stopped at `ms`, in UTC,
    /// so a row's age reads the same wherever the test runs.
    #[cfg(test)]
    pub(crate) fn stop_clock(&mut self, ms: i64) {
        self.clock = YtClock(Some(Rc::new(Cell::new(ms))));
        self.zone = Some(None);
    }

    pub(crate) fn jobs(&self) -> Option<&Jobs> {
        self.held.as_ref().map(|h| &h.jobs)
    }

    pub(super) fn jobs_mut(&mut self) -> Option<&mut Jobs> {
        self.held.as_mut().map(|h| &mut h.jobs)
    }

    /// A resize changes the cell-to-pixel mapping the covers encoded
    /// against (the card cover's rule, per slot).
    pub(crate) fn on_resize(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            slot.on_resize();
        }
    }


    /// Every form the threads' answers may be for, each knowing its own
    /// requests: the room's and the Files window's.
    fn drafts_mut(&mut self) -> impl Iterator<Item = &mut Draft> {
        [&mut self.draft, &mut self.here.draft].into_iter()
    }
}

// ── The two forms ───────────────────────────────────────────────────────────

/// Which form a reading or a key is for: the room's, or the Files
/// window's. Never both at once — the room is a nav room of its own, the
/// window over Files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Form {
    Room,
    Window,
}

impl Form {
    fn surface(self) -> Surface {
        match self {
            Form::Room => Surface::Room,
            Form::Window => Surface::Window,
        }
    }
}

pub(super) fn draft(gui: &Gui, form: Form) -> &Draft {
    match form {
        Form::Room => &gui.ytdl.draft,
        Form::Window => &gui.ytdl.here.draft,
    }
}

pub(super) fn draft_mut(gui: &mut Gui, form: Form) -> &mut Draft {
    match form {
        Form::Room => &mut gui.ytdl.draft,
        Form::Window => &mut gui.ytdl.here.draft,
    }
}

/// The form the controls on screen belong to: the Files window's while it
/// is up, else the room's.
pub(super) fn active_form(gui: &Gui) -> Form {
    if super::ytdl_here::window_up(gui) { Form::Window } else { Form::Room }
}

// ── The gate ────────────────────────────────────────────────────────────────

/// The server's identity as the session names it (a tunnel's, never its
/// bridge's address).
pub(super) fn identity(gui: &Gui) -> String {
    gui.app.origin().server
}

/// The ping's facts, when they are the session's server's.
pub(super) fn facts(gui: &Gui) -> Option<&Facts> {
    let id = identity(gui);
    gui.ytdl.facts.as_ref().filter(|f| !id.is_empty() && config::same_server(&f.identity, &id))
}

/// Add from YouTube shows where it can work (the phone's gates — hidden,
/// never greyed): an own server, not a federated peer; an account that may
/// upload; audio conversion there; a library to add to.
pub(crate) fn available(gui: &Gui) -> bool {
    gui.app.connected
        && gui.app.session.peer.is_none()
        && !gui.app.libraries.is_empty()
        && facts(gui).is_some_and(|f| f.uploads && f.transcode)
}

/// The room is the Library's active one — a nav room, so the Now Playing
/// screen over it counts as away (the Auto DJ room's rule).
pub(super) fn in_room(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == super::YOUTUBE_NAV
}

/// A surface of Add from YouTube's stands beneath its windows: the room,
/// or Files (whose door opens the window).
pub(super) fn surface_up(gui: &Gui) -> bool {
    in_room(gui) || super::ytdl_here::in_files(gui)
}

/// The Edit tags sheet is up, over the form it edits.
pub(super) fn tags_up(gui: &Gui) -> bool {
    match active_form(gui) {
        Form::Room => in_room(gui) && gui.ytdl.draft.tags_open,
        Form::Window => gui.ytdl.here.draft.tags_open,
    }
}

/// The Format sheet is up.
pub(super) fn formats_up(gui: &Gui) -> bool {
    gui.ytdl.formats.is_some() && surface_up(gui)
}

/// A song's list hangs from its row in the room's list.
fn menu_up(gui: &Gui) -> bool {
    in_room(gui) && gui.ytdl.list.menu.is_some()
}

/// A window, a sheet or a song's list over the room or Files: the Files
/// window, Edit tags, Format, Saves to, Choose a folder (what is beneath
/// is inert).
pub(crate) fn window_open(gui: &Gui) -> bool {
    super::ytdl_here::window_up(gui)
        || super::ytdl_where::is_open(gui)
        || tags_up(gui)
        || formats_up(gui)
        || menu_up(gui)
}

/// Anything over the room or Files that owns the pointer and the keys.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    window_open(gui)
}

/// The form's own field is what a paste is for: nothing stands over the
/// form but the Files window that holds it. (Over a sheet a paste is the
/// sheet's field's, typed.)
fn form_bare(gui: &Gui) -> bool {
    !(super::ytdl_where::is_open(gui)
        || tags_up(gui)
        || formats_up(gui)
        || menu_up(gui)
        || super::ytdl_here::sheet_up(gui))
}

/// The link field wants a paste whole: the room is up, or the Files
/// window, and nothing stands over its form.
pub(crate) fn wants_paste(gui: &Gui) -> bool {
    (in_room(gui) || super::ytdl_here::window_up(gui)) && !covered(gui) && form_bare(gui)
}

/// A surface not Add from YouTube's is up over the room or Files — the
/// sign-in form a lapsed token opens, Add a server, the header's list, the
/// sheet, a dialog: a paste is its own, typed into its field as before. So
/// is the quit's gate during a card sync, which holds every key
/// (mp3-player-screen contract, clause 57): no link is looked up under it.
fn covered(gui: &Gui) -> bool {
    super::actions::modal_open(gui)
        || super::dj::modal_open(gui)
        || super::sonic::modal_open(gui)
        || super::discover::modal_open(gui)
        || super::moods::is_open(gui)
        || super::playlists::modal_open(gui)
        || gui.servers.modal_open()
        || gui.servers.drop_open
        || gui.torrent.modal_open()
        || super::device::quit_gate_open(gui)
}

/// The wheel is the YouTube windows' own: Choose a folder's rows. (The
/// room's list takes it through the hub's room arm, [`wheel_room`].)
pub(crate) fn wheels(gui: &Gui) -> bool {
    super::ytdl_where::is_open(gui)
}

/// A call or a lookup is out: the loop waits briskly for it.
pub(crate) fn busy(gui: &Gui) -> bool {
    gui.ytdl.out > 0 || gui.ytdl.jobs().is_some_and(|j| j.in_flight())
}

/// A one-shot client for the live session, the torrent room's way: never
/// a peer's (nothing is added to one), the session's own way in.
pub(super) fn session_client(gui: &Gui) -> Result<Client, String> {
    let session = &gui.app.session;
    if !gui.app.connected || session.server.is_empty() {
        return Err(t!("gui.no_server").to_string());
    }
    if session.peer.is_some() {
        return Err(t!("gui.srv.read_only").to_string());
    }
    let reach = gui.app.reach(&gui.app.origin())?;
    Client::new_with(&reach.base, reach.self_signed)
        .map(|c| c.with_token(reach.token).with_local_token(reach.local_token).with_peer(reach.peer))
        .map_err(|e| e.to_string())
}

// ── Opening, and what arrives ───────────────────────────────────────────────

/// The nav's Add YouTube row: the room opens with the link field holding
/// the keys (the boards' deviation from a room that opens stowed — a paste
/// is the way in), unless its press is still out — the row is there to
/// click again mid-press. The hub calls this from `Act::Nav`, under the
/// gate.
pub(crate) fn open_room(gui: &mut Gui) {
    if gui.ytdl.draft.starting.is_none() {
        gui.ytdl.draft.keys = Keys::Link;
    }
    gui.ytdl.formats = None;
    ensure_jobs(gui);
    if let Some(jobs) = gui.ytdl.jobs_mut() {
        jobs.opened();
    }
}

/// The event loop's hook: a Connected's ping says what the server offers;
/// one for another server or account lets the old tracker go, its calls'
/// answers with it, and the form's lookup and folder check with them.
pub(crate) fn observe(gui: &mut Gui, event: &Event) {
    // A folder's listing on its way in: the Files door's re-list landing.
    super::ytdl_here::observe(gui, event);
    let Event::Connected { id, username, ping, .. } = event else { return };
    // A token's reconnect names no account: the session's stays (the App
    // keeps it), and so does the tracker made for it.
    let account = username.clone().or_else(|| gui.app.session.username.clone());
    let ui = &mut gui.ytdl;
    let moved = ui.held.as_ref().is_some_and(|h| !config::same_server(&h.identity, id) || h.username != account);
    if moved {
        ui.held = None;
    }
    let other = ui.facts.as_ref().is_none_or(|f| !config::same_server(&f.identity, id));
    ui.facts = Some(Facts::of(id, ping));
    if other || moved {
        ui.presets = None;
        let draft = &mut ui.draft;
        draft.folder = None;
        draft.starting = None;
        draft.refusal = None;
        draft.instead = None;
        if !matches!(draft.look, Look::Idle) && draft.parsed.kind == LinkKind::Video {
            draft.look = Look::Idle;
            draft.due = Some(0);
        }
        // The window was over another server's folder; Saves to and
        // Choose a folder read another server's libraries, and the Format
        // sheet its formats; the list's cursor was on another's rows.
        super::ytdl_here::close(gui);
        gui.ytdl.place = None;
        gui.ytdl.formats = None;
        gui.ytdl.list = ListUi::default();
    }
}

/// The tracker for the session's server and account, made when there is a
/// session to make it for (and the old one let go when the session is
/// another's).
fn ensure_jobs(gui: &mut Gui) {
    let wanted = (gui.app.connected && gui.app.session.peer.is_none())
        .then(|| (identity(gui), gui.app.session.username.clone()))
        .filter(|(id, _)| !id.is_empty());
    let keep = match (&gui.ytdl.held, &wanted) {
        (Some(h), Some((id, user))) => config::same_server(&h.identity, id) && h.username == *user,
        _ => false,
    };
    if keep {
        return;
    }
    if gui.ytdl.held.is_some() && wanted.is_some() {
        gui.ytdl.held = None;
    }
    let Some((id, user)) = wanted else { return };
    let Some(f) = facts(gui).cloned() else { return };
    let wire = Wire::new();
    let store = if cfg!(test) {
        YtStore::Memory(Rc::new(RefCell::new(None)))
    } else {
        YtStore::Slot(SlotStore::new(id.clone(), user.clone()))
    };
    let mut jobs = Tracker::new(f.generation, wire.clone(), gui.ytdl.clock.clone(), store);
    jobs.set_no_mkdir(f.no_mkdir);
    gui.ytdl.held = Some(Held { identity: id, username: user, jobs, wire });
}

/// Hand `work` to a thread of its own (or, under test, to the test).
pub(super) fn send(gui: &mut Gui, work: Work) {
    if gui.ytdl.hand {
        match work {
            // A cover is asked beside the form's own work: the tests'
            // count of that stays what it was.
            Work::Thumb { id } => gui.ytdl.thumb_asks.push(id),
            work => gui.ytdl.work.push(work),
        }
        return;
    }
    let tx = gui.ytdl.tx.clone();
    let client = session_client(gui);
    gui.ytdl.out += 1;
    std::thread::spawn(move || {
        let reply = match work {
            Work::Lookup { seq, url } => {
                let result = client.map_err(YtdlCallError::Config).and_then(|c| c.ytdl_metadata(&url));
                Reply::Lookup { seq, result }
            }
            Work::Folders { seq, target } => {
                let check = match client {
                    Ok(c) => check_folders(
                        &target,
                        &mut |dir| c.ytdl_listing(dir).ok().map(|l| Listing { directories: l.directories, files: l.files }),
                        &|_| None,
                    ),
                    Err(_) => check_folders(&target, &mut |_| None, &|_| None),
                };
                Reply::Folders { seq, check }
            }
            Work::Ping { identity } => {
                let ping = client.ok().and_then(|c| c.ping().ok());
                Reply::Pinged { identity, ping }
            }
            // The destination only where Discover's plug-ins are (it
            // answers 404 elsewhere), and never written; the torrent
            // templates stand in for a view that names none (the phone's
            // fallback).
            Work::Presets { identity, discover } => {
                let (view, torrent) = match client {
                    Ok(c) => {
                        let view = discover.then(|| crate::api::wait(c.collection_destination_async()).ok()).flatten();
                        let named = view.as_ref().is_some_and(|v| v.libraries.iter().any(|l| l.template.is_some()));
                        let torrent = (!named).then(|| c.torrent_path_templates().ok()).flatten();
                        (view, torrent)
                    }
                    Err(_) => (None, None),
                };
                Reply::Presets { identity, presets: Presets::of(view.as_ref(), torrent.as_ref()) }
            }
            Work::Listing { seq, directory } => {
                let result = client.map_err(YtdlCallError::Config).and_then(|c| c.ytdl_listing(&directory));
                Reply::Listing { seq, result }
            }
            Work::Thumb { id } => Reply::Thumb { art: fetch_thumb(&id), id },
        };
        let _ = tx.send(reply);
    });
}

/// The tracker's calls since its last turn, each on a thread of its own
/// with a fresh client for the session; under test they wait in the wire.
fn send_calls(gui: &mut Gui) {
    if gui.ytdl.hand {
        return;
    }
    let Some(held) = gui.ytdl.held.as_ref() else { return };
    let calls = held.wire.take_out();
    if calls.is_empty() {
        return;
    }
    let tx = held.wire.sender();
    for (id, call) in calls {
        match session_client(gui) {
            Ok(client) => {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send((id, call.run(&client)));
                });
            }
            Err(e) => {
                let _ = tx.send((id, call.failed(YtdlCallError::Config(e))));
            }
        }
    }
}

/// Once a frame (`frame`'s pumps): the threads' answers folded in, a
/// rested link looked up, a lookup past its bound given up, the folder
/// checked, the tracker's turn and its news. True when anything shown
/// changed.
pub(crate) fn poll(gui: &mut Gui) -> bool {
    let mut changed = false;
    while let Ok(reply) = gui.ytdl.rx.try_recv() {
        gui.ytdl.out = gui.ytdl.out.saturating_sub(1);
        apply_reply(gui, reply);
        changed = true;
    }
    // A room whose gate closed (a re-ping said uploads are off, the
    // session moved to a peer) yields to Files as its row goes, the
    // Discover room's way: hidden, never greyed — what hung over it let go
    // first, as Files is a surface the sweep below keeps them on. A
    // connection that dropped closes nothing — the room says it can't
    // reach the server.
    if in_room(gui) && gui.app.connected && !available(gui) {
        close_overlays(gui);
        gui.act(Act::Nav(super::FILES_NAV));
        changed = true;
    }
    ensure_jobs(gui);
    changed |= form_turn(gui);
    // The Files window's form and its folder, then Saves to's
    // and Choose a folder's: each closes once what it stood over went.
    changed |= super::ytdl_here::turn(gui);
    changed |= super::ytdl_where::turn(gui);
    let room = in_room(gui);
    // Left by any road, the overlays go with what they stood over; the
    // room left, its list's hand is down and its song's list gone.
    if !surface_up(gui) {
        gui.ytdl.formats = None;
    }
    if !room {
        gui.ytdl.list.menu = None;
        gui.ytdl.list.view.stow();
    }
    // The list shows while the room does: outcomes are seen as they land.
    let watch = room && gui.ytdl.jobs().is_some();
    if watch != gui.ytdl.watching {
        gui.ytdl.watching = watch;
        if let Some(jobs) = gui.ytdl.jobs_mut() {
            jobs.watch(watch);
        }
    }
    if let Some(jobs) = gui.ytdl.jobs_mut() {
        jobs.set_showing(Origin::Room, room);
        changed |= jobs.tick();
        let events = jobs.take_events();
        if !events.is_empty() {
            changed = true;
        }
        for event in events {
            take_event(gui, event);
        }
    }
    send_calls(gui);
    changed |= seat(gui);
    changed
}

pub(super) fn apply_reply(gui: &mut Gui, reply: Reply) {
    match reply {
        Reply::Lookup { seq, result } => {
            let whole = gui.ytdl.whole_pastes;
            if let Some(draft) = gui.ytdl.drafts_mut().find(|d| matches!(d.look, Look::Reading { seq: s, .. } if s == seq)) {
                draft.answered(seq, result, whole);
            }
        }
        Reply::Folders { seq, check } => {
            // Saves to checks its own place too.
            super::ytdl_where::checked(gui, seq, &check);
            for draft in gui.ytdl.drafts_mut() {
                draft.checked(seq, check.clone());
            }
        }
        Reply::Pinged { identity, ping } => {
            if let Some(ping) = ping
                && gui.ytdl.facts.as_ref().is_some_and(|f| config::same_server(&f.identity, &identity))
            {
                let facts = Facts::of(&identity, &ping);
                if let Some(jobs) = gui.ytdl.jobs_mut() {
                    jobs.set_no_mkdir(facts.no_mkdir);
                }
                gui.ytdl.facts = Some(facts);
            }
        }
        Reply::Presets { identity: id, presets } => {
            if config::same_server(&id, &identity(gui)) {
                gui.ytdl.presets = Some(presets);
            }
        }
        Reply::Listing { seq, result } => super::ytdl_where::listed(gui, seq, result),
        Reply::Thumb { id, art } => {
            if let Some((_, thumb)) = gui.ytdl.thumbs.iter_mut().find(|(i, _)| *i == id) {
                *thumb = art.map_or(Thumb::None, Thumb::Got);
            }
        }
    }
}

/// The cover for the video a form holds, asked once a song shows on it:
/// YouTube's own thumbnail for the id, from YouTube's image host.
pub(super) fn want_thumb(gui: &mut Gui, form: Form) {
    let draft = draft(gui, form);
    if !(draft.found() || draft.manual) {
        return;
    }
    let Some(id) = draft.parsed.video_id.clone().filter(|id| thumb_url(id).is_some()) else { return };
    if gui.ytdl.thumbs.iter().any(|(i, _)| *i == id) {
        return;
    }
    if gui.ytdl.thumbs.len() >= THUMBS_KEPT {
        gui.ytdl.thumbs.remove(0);
    }
    gui.ytdl.thumbs.push((id.clone(), Thumb::Asked));
    send(gui, Work::Thumb { id });
}

/// Where a video's thumbnail is: YouTube's image host, by the video's
/// id — an id's own letters only, so the address is never anyone else's.
fn thumb_url(id: &str) -> Option<String> {
    let plain = (6..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    plain.then(|| format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"))
}

/// Fetch a video's thumbnail and make the cover of it: the picture's
/// middle square (a 16:9 frame; a YouTube Music track's is its album
/// cover between two bars). `None` covers every way there is none.
fn fetch_thumb(id: &str) -> Option<Art> {
    let url = thumb_url(id)?;
    crate::runtime::install_tls_provider();
    let bytes = crate::runtime::block_on(async {
        let client = reqwest::Client::builder().connect_timeout(THUMB_BOUND).timeout(THUMB_BOUND).build().ok()?;
        let answer = client.get(&url).send().await.ok()?;
        if !answer.status().is_success() {
            return None;
        }
        let bytes = answer.bytes().await.ok()?;
        (bytes.len() <= THUMB_MAX).then(|| bytes.to_vec())
    })
    .ok()
    .flatten()?;
    square_cover(&bytes)
}

/// A picture's middle square as a cover.
pub(super) fn square_cover(bytes: &[u8]) -> Option<Art> {
    let picture = image::load_from_memory(bytes).ok()?;
    let side = picture.width().min(picture.height());
    if side == 0 {
        return None;
    }
    let (x, y) = ((picture.width() - side) / 2, (picture.height() - side) / 2);
    let square = picture.crop_imm(x, y, side, side);
    let mut png = std::io::Cursor::new(Vec::new());
    square.write_to(&mut png, image::ImageFormat::Png).ok()?;
    crate::tui::art::decode(&png.into_inner())
}

/// The room's form at its turn: its timers, and the folder its song lands
/// in checked once the tags rest.
fn form_turn(gui: &mut Gui) -> bool {
    let now = gui.ytdl.now();
    let ui = &mut gui.ytdl;
    let (mut changed, work) = ui.draft.timers(now, &mut ui.seq);
    if let Some(work) = work {
        send(gui, work);
    }
    if in_room(gui) && (gui.ytdl.draft.found() || gui.ytdl.draft.manual) {
        let target = room_target(gui);
        let ui = &mut gui.ytdl;
        if let Some(work) = ui.draft.folder_work(&target, now, &mut ui.seq) {
            send(gui, work);
            changed = true;
        }
        want_thumb(gui, Form::Room);
    }
    changed
}

/// Ask the server what the link on screen is (the room's, or the Files
/// window's).
pub(super) fn ask(gui: &mut Gui) {
    let now = gui.ytdl.now();
    let form = active_form(gui);
    let mut seq = gui.ytdl.seq;
    let work = draft_mut(gui, form).ask(now, &mut seq);
    gui.ytdl.seq = seq;
    if let Some(work) = work {
        send(gui, work);
    }
}

/// A fresh request number (the lookups', the folder checks' and the
/// listings' one count, so no answer is taken for another's).
pub(super) fn next_seq(gui: &mut Gui) -> u64 {
    gui.ytdl.seq += 1;
    gui.ytdl.seq
}

/// The tracker's news, for the surfaces: the form's start answered, the
/// note line's words, the ping or the format to set straight.
fn take_event(gui: &mut Gui, event: JobEvent) {
    match event {
        JobEvent::Started { job, outcome, .. } => {
            // Whose press it was: the room's, or the Files window's.
            let mine = [Form::Room, Form::Window]
                .into_iter()
                .find(|form| draft(gui, *form).starting.as_deref() == Some(job.as_str()));
            match outcome {
                StartOutcome::Took | StartOutcome::Unsure => {
                    let title = gui.ytdl.jobs().and_then(|j| j.job(&job)).map(Job::shown_title).unwrap_or_default();
                    match mine {
                        // The server has it: the form starts over, the field
                        // holding the keys for the next link.
                        Some(Form::Room) => gui.ytdl.draft = Draft::default(),
                        // The window closes; the door's word carries the job.
                        Some(Form::Window) => super::ytdl_here::close(gui),
                        None => {}
                    }
                    let text = keep::ell(&t!("gui.ytdl.note.adding", title = title));
                    let note = fit_note(&text, None, keep::note_room(gui));
                    gui.ytdl.adding_note = Some((job.clone(), note.clone()));
                    keep::set_note(gui, (note, MessageKind::Busy));
                }
                StartOutcome::Refused(failure) if mine.is_some() => {
                    let draft = draft_mut(gui, mine.unwrap_or(Form::Room));
                    draft.starting = None;
                    draft.refusal = Some(failure);
                }
                StartOutcome::Refused(_) | StartOutcome::Failed(_) => {
                    if let Some(form) = mine {
                        draft_mut(gui, form).starting = None;
                    }
                }
            }
        }
        JobEvent::Settled { note, job, state } => {
            // A song that landed in the folder Files shows, or below it:
            // the folder lists again (F4).
            super::ytdl_here::settled(gui, &job, state);
            let room = keep::note_room(gui);
            // Its "Adding …" is over however it ended: one with no words
            // of its own (not sure, not found — the list says) takes the
            // note down if it still stands.
            if let Some((_, adding)) = gui.ytdl.adding_note.take_if(|(id, _)| *id == job) {
                let standing = gui.app.message.as_ref().is_some_and(|m| m.text == adding && m.kind == MessageKind::Busy);
                if note.is_none() && standing {
                    gui.app.message = None;
                }
            }
            match note {
                Some(Note::Added { title, folder }) => {
                    let head = t!("gui.ytdl.note.added", title = title).to_string();
                    let place = path_crumbs(&folder).into_iter().skip(1).collect::<Vec<_>>().join(crumb_sep());
                    let wide =
                        (!place.is_empty()).then(|| t!("gui.ytdl.note.added_in", title = title, place = place).to_string());
                    keep::set_note(gui, (fit_note(&head, wide.as_deref(), room), MessageKind::Success));
                }
                Some(Note::CouldntAdd { title }) => {
                    let head = t!("gui.ytdl.note.couldnt", title = title).to_string();
                    keep::set_note(gui, (fit_note(&head, None, room), MessageKind::Error));
                }
                None => {}
            }
        }
        JobEvent::Reping => reping(gui),
        JobEvent::FormatOff => {
            // The server turned the format off: the remembered one goes
            // back to the default, the rest of the server's choices kept.
            let id = identity(gui);
            super::servers::update_config(gui, |c| {
                let mut prefs = config::ytdl_prefs(c, &id);
                prefs.format = None;
                config::set_ytdl_prefs(c, &id, &prefs);
            });
            gui.ytdl.draft.codec = None;
            gui.ytdl.here.draft.codec = None;
        }
    }
}

/// Ping the server again: a flag it gave looks stale.
fn reping(gui: &mut Gui) {
    let identity = identity(gui);
    if identity.is_empty() {
        return;
    }
    send(gui, Work::Ping { identity });
}

// ── The room's reading of the form ──────────────────────────────────────────

/// What the server's account remembers for Add from YouTube, as the rules
/// read it.
pub(super) fn remembered(gui: &Gui) -> (Remembered, Option<String>) {
    let prefs = config::ytdl_prefs(&gui.config, &identity(gui));
    let kind = prefs.layout_kind.as_deref().and_then(LayoutKind::parse);
    (Remembered { library: prefs.library, layout: prefs.layout, kind }, prefs.format)
}

/// The choice the room's song goes by: Saves to's `use for this song`,
/// else the server's remembered one.
pub(super) fn room_choice(gui: &Gui) -> Choice {
    if let Some(choice) = &gui.ytdl.draft.choice {
        return choice.clone();
    }
    let (saved, _) = remembered(gui);
    remembered_choice(gui.ytdl.presets.as_ref(), Some(&saved), &gui.app.libraries)
}

/// Where the room's song lands: the choice it goes by for its tags, or
/// the folder "i save to … instead" or Choose a folder chose.
fn room_target(gui: &Gui) -> Target {
    let draft = &gui.ytdl.draft;
    let song = draft.tags().song();
    if let Some(dir) = &draft.instead {
        return folder_target(dir, None, &song);
    }
    choice_target(&room_choice(gui), &song)
}

/// Where a form's song lands.
pub(super) fn form_target(gui: &Gui, form: Form) -> Target {
    match form {
        Form::Room => room_target(gui),
        Form::Window => super::ytdl_here::target(gui),
    }
}

/// The format a form's song goes as: the one picked for it, else the
/// server's remembered one.
pub(super) fn form_codec(gui: &Gui, form: Form) -> String {
    let formats = facts(gui).and_then(|f| f.formats.as_deref());
    match &draft(gui, form).codec {
        Some(codec) => codec.clone(),
        None => crate::ytdl::effective_format(remembered(gui).1.as_deref(), formats).to_string(),
    }
}

pub(super) fn tier(gui: &Gui) -> Tier {
    gui.ytdl.jobs().map_or(Tier::Older, |j| j.tier())
}

pub(super) fn server_name(gui: &Gui) -> String {
    gui.app.server_name_of(&identity(gui))
}

/// What the folder check said of a form's target, while it is the
/// target's.
fn folder_check<'a>(gui: &'a Gui, form: Form, target: &Target) -> Option<&'a FolderCheck> {
    checked_for(&draft(gui, form).folder, target)
}

/// The folder an add makes first, and the first new folder's name: one on
/// the path is new, or it couldn't be checked.
fn mkdir_for(target: &Target, check: Option<&FolderCheck>) -> Option<(String, Option<String>)> {
    match check {
        Some(c) if c.known && c.new_segments.is_empty() => None,
        Some(c) if c.known => {
            Some((target.directory(), c.new_segments.first().and_then(|&i| target.segments.get(i)).cloned()))
        }
        _ => Some((target.directory(), None)),
    }
}

/// Everything a form's frame says and what its primary would press, read
/// at once from the form, the server and the jobs.
pub(super) struct RoomView {
    pub(super) stage: Stage,
    pub(super) notes: Vec<(Tone, String)>,
    pub(super) target: Target,
    pub(super) new_folder: bool,
    /// The GOLD line for a folder the account can't make, and the folder
    /// that exists (`save to … instead`).
    pub(super) no_mkdir: Option<(String, Option<String>)>,
    /// The file line: a clash (GOLD) or letters dropped (DIM).
    pub(super) file_line: Option<(Tone, String)>,
    /// Above the primary: a refusal (GOLD) or why it is off (DIM).
    pub(super) above: Option<(Tone, String)>,
    /// The primary's label as the surface draws it (the room's tall one
    /// with its `▸`, the window's bracketed words) and whether it is on.
    pub(super) primary: Option<(String, bool)>,
    /// The primary reads Add anyway (a predicted clash).
    pub(super) anyway: bool,
    pub(super) block: AddBlock,
    pub(super) request: Option<StartRequest>,
}

/// What the form shows below the link.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Stage {
    /// Nothing yet: no link, or not a video, or the typing not rested.
    Empty,
    Reading { slow: bool },
    /// A lookup that failed with a way to ask again, or none (the server
    /// has no yt-dlp).
    Failed { text: String, retry: bool, missing: bool },
    /// The song as a card: found, or its details typed by hand.
    Song(Song),
    /// Typed by hand and nothing typed yet — a video the server couldn't
    /// read (its words stand over `type the details`).
    Blank { failed: Option<String> },
}

/// The song's card: what the four lines beside the cover say, and the
/// lines under it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Song {
    /// The title, and whether it is the tag's own — a blank tag leaves
    /// the file the video's title, which then shows DIM.
    pub(super) title: (String, bool),
    pub(super) artist: String,
    /// `Discovery · 2001`, `No album · 2009`.
    pub(super) album: String,
    /// `YouTube · Daft Punk · 5:20`: the channel and the length, as the
    /// lookup gave them. None for a song typed by hand.
    pub(super) from: Option<String>,
    /// "Cleaned from “…”", with `undo`.
    pub(super) cleaned: Option<crate::ytdl::Label>,
    pub(super) source: Option<(Tone, String)>,
    /// The lookup's failure, for a song typed over one.
    pub(super) failed: Option<String>,
}

fn room_view(gui: &Gui) -> RoomView {
    form_view(gui, Form::Room)
}

/// A length in seconds as the bar writes one: `5:20`, `1:02:03`.
fn length(secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// The card for what a form holds: its tags, and — found — the video's
/// own line and where the tags came from.
fn song_of(draft: &Draft, failed: Option<String>) -> Song {
    let tags = draft.tags();
    let meta = draft.meta();
    let own = draft.guess.raw_title.trim();
    let title = match tags.title.trim() {
        "" if !own.is_empty() => (own.to_string(), false),
        "" => (draft.parsed.shown.clone(), false),
        title => (title.to_string(), true),
    };
    let album = if tags.album.trim().is_empty() { t!("gui.ytdl.room.no_album").to_string() } else { tags.album.trim().to_string() };
    let album = if tags.year.trim().is_empty() { album } else { format!("{album} · {}", tags.year.trim()) };
    let from = meta.map(|m| {
        let channel = m.channel.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        let parts = [Some("YouTube".to_string()), channel, m.duration_sec.filter(|d| *d > 0.0).map(length)];
        parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
    });
    let view = draft.guess.view(&draft.gstate, draft.edited[1]);
    Song {
        title,
        artist: tags.artist.trim().to_string(),
        album,
        from,
        cleaned: (meta.is_some() && view.cleaned).then(|| draft.guess.cleaned_label()),
        source: meta.and_then(|_| draft.guess.source(&draft.gstate).line()).map(|(tone, label)| (tone, label.text())),
        failed,
    }
}

pub(super) fn form_view(gui: &Gui, form: Form) -> RoomView {
    let now = gui.ytdl.now();
    let draft = draft(gui, form);
    let tier = tier(gui);
    let server = server_name(gui);
    let codec = form_codec(gui, form);
    let words = Words { surface: form.surface(), server: &server, codec: &codec, tier };
    let lookup = draft.lookup_state(now);
    let live = draft.meta().is_some_and(YtdlMetadata::is_stream);
    let notes: Vec<(Tone, String)> =
        link_notes(&draft.parsed, live, tier).into_iter().map(|(tone, label)| (tone, label.text())).collect();
    let tags = draft.tags();
    let typed = [&tags.title, &tags.artist, &tags.album, &tags.year].iter().any(|t| !t.trim().is_empty());
    let failed = match &draft.look {
        Look::Failed { failure, .. } => Some(failure_label(failure, &words).text()),
        _ => None,
    };
    let stage = match &draft.look {
        Look::Found { .. } => Stage::Song(song_of(draft, None)),
        _ if draft.manual && typed => Stage::Song(song_of(draft, failed)),
        Look::Reading { .. } => Stage::Reading { slow: lookup == LookupState::Slow },
        _ if draft.manual => Stage::Blank { failed },
        Look::Failed { .. } => Stage::Failed {
            text: failed.unwrap_or_default(),
            retry: matches!(lookup, LookupState::TimedOut | LookupState::Unreachable),
            missing: lookup == LookupState::YtdlpMissing,
        },
        Look::Idle => Stage::Empty,
    };

    let target = form_target(gui, form);
    let check = folder_check(gui, form, &target);
    let new_folder = check.is_some_and(|c| c.known && !c.new_segments.is_empty());
    let mkdir = mkdir_for(&target, check);
    let facts = facts(gui);
    let no_mkdir_flag = facts.is_some_and(|f| f.no_mkdir) || gui.ytdl.jobs().is_some_and(|j| j.mkdir_refused());
    let mkdir_blocked = no_mkdir_flag && mkdir.is_some() && target.is_valid();
    // Its line once the check has answered: a folder still being looked at
    // is off (the block) but not yet said to be missing.
    let no_mkdir = (mkdir_blocked && check.is_some()).then(|| {
        let line = match mkdir.as_ref().and_then(|(_, folder)| folder.clone()) {
            Some(folder) => t!("gui.ytdl.no_mkdir.new", folder = folder).to_string(),
            None => t!("gui.ytdl.no_mkdir.unchecked").to_string(),
        };
        let deepest = check.and_then(|c| c.deepest.clone()).filter(|d| *d != target.directory());
        (line, deepest)
    });
    let raw_title = draft.guess.raw_title.clone();
    let nc = name_check(&raw_title, draft.meta().and_then(|m| m.file_stem.as_deref()), &codec, check.and_then(|c| c.files.as_deref()), tier);
    let file_line = nc.line.as_ref().map(|(tone, label)| (*tone, label.text()));
    let already = gui.ytdl.jobs().is_some_and(|j| {
        draft.parsed.url.as_deref().is_some_and(|url| j.is_adding(url, &target.directory()))
    });
    let state = AddState {
        convert_off: facts.is_some_and(|f| !f.transcode),
        video: draft.parsed.kind == LinkKind::Video,
        lookup,
        manual: draft.manual,
        uploads_off: facts.is_some_and(|f| !f.uploads),
        starting: draft.starting.is_some(),
        already_adding: already,
        target_valid: target.is_valid(),
        year_invalid: year_invalid(&tags.year),
        mkdir_blocked,
        clash_older: nc.blocks,
    };
    let block = add_block(&state);
    let starting = draft.starting.is_some();
    // The primary stands wherever a song is on its way: off (and DIM)
    // while the video is read, on once there is something to add.
    let shows_primary = match &stage {
        Stage::Song(_) | Stage::Blank { .. } | Stage::Reading { .. } => true,
        Stage::Failed { missing, .. } => *missing,
        Stage::Empty => false,
    };
    let primary = shows_primary.then(|| {
        let enabled = block == AddBlock::None && !starting;
        // The window adds into the folder on screen: Add here — past it
        // (a subfolder, a folder chosen) it adds to the library.
        let here = form == Form::Window && super::ytdl_here::is_here(gui, &target);
        let surface = if here { Surface::Window } else { Surface::Room };
        let words = add_button_label(surface, nc.add_anyway, starting).text();
        let label = if form == Form::Window {
            format!("[ {} ]", keep::ell(&words))
        } else if starting {
            keep::ell(&words)
        } else if enabled {
            format!("{words} {}", super::forward_glyph())
        } else {
            words
        };
        (label, enabled)
    });
    let refusal = draft.refusal.as_ref().filter(|f| !(f.problem == Problem::MkdirOff && mkdir_blocked));
    let above = if let Some(f) = refusal {
        Some((Tone::Refusal, failure_label(f, &words).text()))
    } else if shows_primary && !matches!(stage, Stage::Failed { .. } | Stage::Reading { .. }) {
        match block {
            AddBlock::AlreadyAdding | AddBlock::YearInvalid | AddBlock::InvalidLayout => {
                block_label(block, &server).map(|l| (Tone::Note, l.text()))
            }
            AddBlock::ConvertOff | AddBlock::UploadsOff => block_label(block, &server).map(|l| (Tone::Refusal, l.text())),
            _ => None,
        }
    } else {
        None
    };
    let request = (primary.as_ref().is_some_and(|(_, on)| *on)).then(|| {
        let (mkdir, mkdir_folder) = match mkdir.clone() {
            Some((dir, folder)) => (Some(dir), folder),
            None => (None, None),
        };
        StartRequest {
            url: draft.parsed.url.clone().unwrap_or_default(),
            directory: target.directory(),
            codec: codec.clone(),
            tags: tags.clone(),
            title: if tags.title.trim().is_empty() { raw_title.clone() } else { tags.title.trim().to_string() },
            file_name: nc.name.clone(),
            mkdir,
            mkdir_folder,
            origin: match form {
                Form::Room => Origin::Room,
                Form::Window => super::ytdl_here::origin(gui),
            },
        }
    });
    RoomView {
        stage,
        notes,
        target,
        new_folder,
        no_mkdir,
        // The server's 409 says what the warning guessed: its GOLD lines
        // over the primary stand alone.
        file_line: file_line.filter(|_| !refusal.is_some_and(|f| f.problem == Problem::Exists)),
        above,
        primary,
        anyway: nc.add_anyway,
        block,
        request,
    }
}

// ── Drawing the form ────────────────────────────────────────────────────────

fn gold() -> Style {
    Style::default().fg(th().gold)
}

/// `(•)` and `( )`, the legacy console's `(*)`.
pub(super) fn dots() -> (&'static str, &'static str) {
    if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") }
}

/// A GOLD line's rows: `! ` and the words wrapped to `room`, each row after
/// the first two cells in.
pub(super) fn gold_rows(text: &str, room: usize) -> Vec<String> {
    let lines = crate::kit::wrap_whole(text, room.saturating_sub(2).max(1));
    lines.into_iter().enumerate().map(|(i, l)| if i == 0 { format!("! {l}") } else { format!("  {l}") }).collect()
}

/// Crumbs that fit `room` cells, giving up whole folders from the left —
/// `… › Discovery` — and clipping the last one only when it alone is too
/// wide.
pub(super) fn fit_crumbs(crumbs: &[String], tail: usize, room: usize) -> (bool, Vec<String>) {
    let sep = width(crumb_sep());
    let ell = keep::ell("…");
    let wide = |from: usize| -> usize {
        let names: usize = crumbs[from..].iter().map(|c| width(c)).sum();
        let lead = if from > 0 { width(&ell) + sep } else { 0 };
        lead + names + sep * crumbs.len().saturating_sub(from + 1) + tail
    };
    for from in 0..crumbs.len() {
        if wide(from) <= room {
            return (from > 0, crumbs[from..].to_vec());
        }
    }
    let last = crumbs.last().cloned().unwrap_or_default();
    let budget = room.saturating_sub(width(&ell) + sep + tail);
    (crumbs.len() > 1, vec![super::bar::clip(&last, budget.max(1)).into_owned()])
}

/// The place a song goes, from `x`: the crumbs TEXT between DIM `›`, the
/// library first, `new` DIM after them when a folder will be made, all
/// within `room` cells. Returns the cells drawn.
pub(super) fn draw_place(frame: &mut Frame, x: u16, y: u16, crumbs: &[String], new: bool, room: usize) -> u16 {
    let new_word = t!("gui.ytdl.room.new").to_string();
    let tail = if new { 1 + width(&new_word) } else { 0 };
    let (clipped, shown) = fit_crumbs(crumbs, tail, room);
    let mut at = x;
    if clipped {
        let ell = keep::ell("…");
        put(frame, at, y, &ell, dim());
        at += width(&ell) as u16;
        put(frame, at, y, crumb_sep(), dim());
        at += width(crumb_sep()) as u16;
    }
    for (i, crumb) in shown.iter().enumerate() {
        if i > 0 {
            put(frame, at, y, crumb_sep(), dim());
            at += width(crumb_sep()) as u16;
        }
        put(frame, at, y, crumb, Style::default());
        at += width(crumb) as u16;
    }
    if new {
        put(frame, at + 1, y, &new_word, dim());
        at += tail as u16;
    }
    at - x
}

/// The cells [`draw_place`] takes for these crumbs in `room`.
pub(super) fn place_width(crumbs: &[String], new: bool, room: usize) -> u16 {
    let tail = if new { 1 + width(&t!("gui.ytdl.room.new")) } else { 0 };
    let (clipped, shown) = fit_crumbs(crumbs, tail, room);
    let sep = width(crumb_sep());
    let lead = if clipped { width(&keep::ell("…")) + sep } else { 0 };
    let names: usize = shown.iter().map(|c| width(c)).sum();
    (lead + names + sep * shown.len().saturating_sub(1) + tail) as u16
}

/// The words a control shows and the letter that works it, out of its
/// key word: "e edit tags" is `e` and "edit tags" (every locale's key
/// words open with their letter, a space after it).
pub(super) fn page_words(key_word: &str) -> (char, &str) {
    let mut chars = key_word.chars();
    let letter = chars.next().unwrap_or(' ');
    (letter.to_ascii_lowercase(), chars.as_str().trim_start())
}

/// The cells [`link`] takes for a key word.
pub(super) fn link_width(key_word: &str, forward: bool) -> u16 {
    width(page_words(key_word).1) as u16 + if forward { 1 + width(super::forward_glyph()) as u16 } else { 0 }
}

/// A control of the page: its words — and `▸` when it opens something —
/// DIM, BRIGHT BOLD under the pointer, its click registered and its
/// letter live for this frame. `key_word` is the locale's key word for it
/// (the tips line's): the page shows its words without the key. Returns
/// its width in cells.
pub(super) fn link(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, key_word: &str, forward: bool, act: YtAct) -> u16 {
    let (letter, words) = page_words(key_word);
    let label = if forward { format!("{words} {}", super::forward_glyph()) } else { words.to_string() };
    // A press is out: the control stands, DIM, and does nothing.
    if gui.ytdl.quiet {
        put(frame, x, y, &label, dim());
        return width(&label) as u16;
    }
    gui.ytdl.drawn.push((letter, act.clone()));
    text_button(frame, gui, x, y, &label, false, yt(act))
}

/// One row of a form's body, as drawn.
#[derive(Debug, Clone)]
pub(super) enum Row {
    /// A blank that gives way when the frame is short.
    Blank,
    Plain(String, Style),
    Gold(String),
    /// A line of the song's card, beside its cover: the title and `edit
    /// tags ▸`, the artist, the album and year, YouTube's own line.
    Card(u8),
    /// A line beside the cover's empty slot while the video is read: the
    /// words, "Still looking…" (two rows of it at most), `type the
    /// details ▸`.
    Reading(u8),
    /// A line with a control at its right end (the clean-up's `undo`, a
    /// late answer's `use them`): the words, or a label whose `title`
    /// gives way inside its quotes.
    WithLink(Result<String, crate::ytdl::Label>, String, YtAct),
    /// Controls from the left, ` · ` between them: each its key word and
    /// whether it opens something.
    Links(Vec<(String, bool, YtAct)>),
    /// `Saves to` (the window's `Saves here`) and the place.
    Place,
    /// `Format` and the format.
    Format,
}

/// Controls laid from the left, as many a row as fit `w` with ` · `
/// between them.
fn link_rows(links: Vec<(String, bool, YtAct)>, w: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut line: Vec<(String, bool, YtAct)> = Vec::new();
    let mut used = 0;
    for (key, forward, act) in links {
        let cells = usize::from(link_width(&key, forward));
        if !line.is_empty() && used + 3 + cells > w {
            rows.push(Row::Links(std::mem::take(&mut line)));
            used = 0;
        }
        used += if line.is_empty() { cells } else { 3 + cells };
        line.push((key, forward, act));
    }
    if !line.is_empty() {
        rows.push(Row::Links(line));
    }
    rows
}

/// A form's body under its link card, top to bottom — the room's and the
/// Files window's alike: the link's own lines, a failure's or a late
/// answer's (or the blank they take); what the link came to; then where
/// the song saves and as what, and what the folder has to say.
pub(super) fn form_rows(gui: &Gui, form: Form, view: &RoomView, w: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let draft = draft(gui, form);
    let gold = |text: &str, rows: &mut Vec<Row>| rows.extend(gold_rows(text, w).into_iter().map(Row::Gold));
    let plain = |text: &str, rows: &mut Vec<Row>| {
        rows.extend(crate::kit::wrap_whole(text, w.max(1)).into_iter().map(|l| Row::Plain(l, dim())))
    };
    for (tone, text) in &view.notes {
        match tone {
            Tone::Refusal | Tone::Warn => gold(text, &mut rows),
            Tone::Note | Tone::Ok => plain(text, &mut rows),
        }
    }
    let failed = match &view.stage {
        Stage::Failed { text, .. } => Some(text),
        Stage::Song(song) => song.failed.as_ref(),
        Stage::Blank { failed } => failed.as_ref(),
        _ => None,
    };
    if let Some(text) = failed {
        gold(text, &mut rows);
    }
    if draft.late.is_some() {
        // Beside `use them`, wrapped rather than clipped.
        let key = t!("gui.ytdl.key.use").to_string();
        let room = w.saturating_sub(usize::from(link_width(&key, false)) + 3).max(1);
        for (i, line) in crate::kit::wrap_whole(&t!("gui.ytdl.room.late"), room).into_iter().enumerate() {
            rows.push(if i == 0 { Row::WithLink(Ok(line), key.clone(), YtAct::Use) } else { Row::Plain(line, dim()) });
        }
    }
    let headed = !rows.is_empty();
    if !headed {
        rows.push(Row::Blank);
    }
    match &view.stage {
        // The limits, in the room, while the link has nothing to say.
        Stage::Empty => {
            if !headed && form == Form::Room {
                plain(&t!("gui.ytdl.room.limits"), &mut rows);
            }
        }
        Stage::Reading { .. } => rows.extend((0..COVER_H as u8).map(Row::Reading)),
        Stage::Failed { retry, .. } => {
            if *retry {
                rows.push(Row::Blank);
                rows.push(Row::Links(vec![(t!("gui.ytdl.key.retry").to_string(), false, YtAct::Retry)]));
            }
        }
        Stage::Song(song) => {
            rows.extend((0..COVER_H as u8).map(Row::Card));
            if let Some(cleaned) = &song.cleaned {
                rows.push(Row::WithLink(Err(cleaned.clone()), t!("gui.ytdl.key.undo").to_string(), YtAct::Undo));
            }
            match &song.source {
                Some((Tone::Warn | Tone::Refusal, text)) => gold(text, &mut rows),
                Some((_, text)) => plain(text, &mut rows),
                None => {}
            }
        }
        Stage::Blank { .. } => {
            if headed {
                rows.push(Row::Blank);
            }
            rows.push(Row::Links(vec![(t!("gui.ytdl.key.type").to_string(), true, YtAct::Type)]));
        }
    }
    rows.push(Row::Blank);
    rows.push(Row::Place);
    rows.push(Row::Format);
    match &view.no_mkdir {
        // The folder can't be made: its GOLD line under the rows, and the
        // ways on under that.
        Some((line, deepest)) => {
            gold(line, &mut rows);
            let mut links = Vec::new();
            if let Some(dir) = deepest.as_ref().filter(|_| form == Form::Room) {
                let place = path_crumbs(dir).join(crumb_sep());
                links.push((t!("gui.ytdl.key.instead", place = place).to_string(), false, YtAct::Instead));
            }
            links.push((t!("gui.ytdl.key.choose").to_string(), true, YtAct::Choose));
            rows.extend(link_rows(links, w));
        }
        None => match &view.file_line {
            Some((Tone::Warn | Tone::Refusal, text)) => gold(text, &mut rows),
            Some((_, text)) => plain(text, &mut rows),
            None => {}
        },
    }
    rows
}

/// The rows over the primary: a refusal's GOLD lines, or the DIM reason it
/// is off, right-aligned: (text, style, right-aligned).
pub(super) fn above_rows(view: &RoomView, w: usize) -> Vec<(String, Style, bool)> {
    match &view.above {
        Some((Tone::Refusal | Tone::Warn, text)) => gold_rows(text, w).into_iter().map(|l| (l, gold(), false)).collect(),
        // Wrapped, not clipped: in the 47-cell room most languages' reason
        // takes two lines.
        Some((_, text)) => crate::kit::wrap_whole(text, w.max(1)).into_iter().map(|l| (l, dim(), true)).collect(),
        None => Vec::new(),
    }
}

/// The formats the server takes, as the sheet offers them.
pub(super) fn formats_offered(gui: &Gui) -> Vec<&'static str> {
    format_options(facts(gui).and_then(|f| f.formats.as_deref()))
}

/// A label whose `param` gives way so the words fit `room` cells: the
/// clean-up's title is cut inside its quotes, never the closing one.
fn fit_label(label: &crate::ytdl::Label, param: &'static str, room: usize) -> String {
    let whole = label.text();
    if width(&whole) <= room {
        return whole;
    }
    let value = label.param(param).unwrap_or_default();
    let fixed = width(&whole).saturating_sub(width(value));
    let cut = super::bar::clip(value, room.saturating_sub(fixed).max(1)).into_owned();
    let cut_label = label.params.iter().fold(crate::ytdl::Label::new(label.key), |l, (name, v)| {
        l.with(name, if *name == param { cut.clone() } else { v.clone() })
    });
    super::bar::clip(&cut_label.text(), room).into_owned()
}

/// Who has a form's keys this frame. The Files window is modal: the queue
/// panel can't hold them while it is up.
pub(super) fn form_keys(gui: &Gui, form: Form) -> Keys {
    if form == Form::Room && gui.queue_open && gui.app.focus == Focus::Queue {
        return Keys::None;
    }
    draft(gui, form).keys
}

/// Who has the room's keys this frame: the queue panel holding them takes
/// them from the form.
fn room_keys(gui: &Gui) -> Keys {
    form_keys(gui, Form::Room)
}

/// The 3-row link card: ACCENT while the field has the keys, the caret
/// then the DIM placeholder when empty, `[X]` to clear it; a click on the
/// card is `field`, on `[X]` `clear`. A press out, it stands DIM and does
/// nothing.
pub(crate) fn draw_link_card(frame: &mut Frame, gui: &mut Gui, card: Rect, link: &Input, keys: bool, (field, clear): (Act, Act)) {
    let quiet = gui.ytdl.quiet;
    let hover = !quiet && gui.ui.hovers(card);
    let tone = if keys {
        th().accent
    } else if hover {
        th().bright
    } else {
        th().dim
    };
    crate::kit::frame_at(frame, card, tone);
    if !quiet {
        gui.ui.click(card, field);
    }
    let fx = card.x + 2;
    let fy = card.y + 1;
    let value = link.value();
    let cursor = link.cursor();
    let x_at = Rect { x: card.right().saturating_sub(5), y: fy, width: 3, height: 1 };
    let room = x_at.x.saturating_sub(fx + 1);
    let placeholder = t!("gui.ytdl.room.paste").to_string();
    if keys {
        super::text_field(frame, &mut gui.ui, fx, fy, value, cursor, room, Style::default());
        if value.is_empty() {
            put(frame, fx + 1, fy, &super::bar::clip(&placeholder, room as usize), dim());
        }
    } else if value.is_empty() {
        put(frame, fx, fy, &super::bar::clip(&placeholder, room as usize), dim());
    } else {
        put(frame, fx, fy, &super::bar::clip(value, room as usize), Style::default());
    }
    if !value.is_empty() {
        let style = if !quiet && gui.ui.hovers(x_at) {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, x_at.x, fy, "[X]", style);
        if !quiet {
            gui.ui.click(x_at, clear);
        }
    }
}

/// The link's band, three rows of the block's width: the link card and —
/// where this build can read the clipboard — `Paste` beside it, a DIM
/// tall frame one cell off (BRIGHT under the pointer). A terminal can't
/// read the clipboard, so there the card takes the whole band and a paste
/// is the terminal's own.
pub(super) fn draw_link_band(frame: &mut Frame, gui: &mut Gui, form: Form, band: Rect, keys: bool) {
    let label = t!("gui.ytdl.room.paste_button").to_string();
    let button = width(&label) as u16 + 4;
    let paste = gui.ytdl.clipboard.is_some() && band.width >= button + 1 + 16;
    let card = Rect { width: if paste { band.width - button - 1 } else { band.width }, height: 3, ..band };
    let link = draft(gui, form).link.clone();
    draw_link_card(frame, gui, card, &link, keys, (yt(YtAct::Field), yt(YtAct::Clear)));
    if paste {
        let at = Rect { x: card.right() + 1, y: band.y, width: button, height: 3 };
        let tone = |hovered: bool| if hovered { (th().bright, true) } else { (th().dim, false) };
        let act = (!gui.ytdl.quiet).then(|| yt(YtAct::PasteClip));
        crate::kit::tall_frame(frame, &mut gui.ui, at, &label, 1, tone, act);
    }
}

/// A form's row, drawn across `block`'s width at `y`.
pub(super) fn draw_row(frame: &mut Frame, gui: &mut Gui, form: Form, view: &RoomView, block: Rect, y: u16, row: Row) {
    let x = block.x;
    let w = usize::from(block.width);
    let end = block.right();
    match row {
        Row::Blank => {}
        Row::Plain(text, style) => put(frame, x, y, &super::bar::clip(&text, w), style),
        Row::Gold(text) => put(frame, x, y, &super::bar::clip(&text, w), gold()),
        Row::Card(line) => draw_card_row(frame, gui, view, block, y, line),
        Row::Reading(line) => draw_reading_row(frame, gui, view, block, y, line),
        Row::WithLink(words, key, act) => {
            let at = end.saturating_sub(link_width(&key, false));
            link(frame, gui, at, y, &key, false, act);
            let room = usize::from(at.saturating_sub(x + 3));
            let words = match words {
                Ok(words) => super::bar::clip(&words, room).into_owned(),
                Err(label) => fit_label(&label, "title", room),
            };
            put(frame, x, y, &words, dim());
        }
        Row::Links(links) => {
            let mut at = x;
            for (i, (key, forward, act)) in links.into_iter().enumerate() {
                if i > 0 {
                    put(frame, at, y, " · ", dim());
                    at += 3;
                }
                at += link(frame, gui, at, y, &key, forward, act);
            }
        }
        Row::Place => draw_place_row(frame, gui, form, view, block, y),
        Row::Format => draw_format_row(frame, gui, form, block, y),
    }
}

/// A line of the song's card, from one cell past the cover.
fn draw_card_row(frame: &mut Frame, gui: &mut Gui, view: &RoomView, block: Rect, y: u16, line: u8) {
    let Stage::Song(song) = &view.stage else { return };
    let x = block.x + COVER_W + 1;
    let room = usize::from(block.right().saturating_sub(x));
    match line {
        0 => {
            let key = t!("gui.ytdl.key.edit").to_string();
            let at = block.right().saturating_sub(link_width(&key, true));
            link(frame, gui, at, y, &key, true, YtAct::Edit);
            let (title, own) = &song.title;
            let style = if *own { Style::default().add_modifier(Modifier::BOLD) } else { dim() };
            put(frame, x, y, &super::bar::clip(title, usize::from(at.saturating_sub(x + 2))), style);
        }
        1 => put(frame, x, y, &super::bar::clip(&song.artist, room), Style::default()),
        2 => put(frame, x, y, &super::bar::clip(&song.album, room), dim()),
        _ => {
            if let Some(from) = &song.from {
                put(frame, x, y, &super::bar::clip(from, room), dim());
            }
        }
    }
}

/// A line beside the cover's slot while the video is read.
fn draw_reading_row(frame: &mut Frame, gui: &mut Gui, view: &RoomView, block: Rect, y: u16, line: u8) {
    let Stage::Reading { slow } = view.stage else { return };
    let x = block.x + COVER_W + 1;
    let room = usize::from(block.right().saturating_sub(x));
    match line {
        0 => put(frame, x, y, &super::bar::clip(&keep::ell(&t!("gui.ytdl.room.reading")), room), accent()),
        1 | 2 if slow => {
            let lines = crate::kit::wrap_whole(&t!("gui.ytdl.room.slow"), room.max(1));
            if let Some(words) = lines.get(usize::from(line) - 1) {
                put(frame, x, y, &super::bar::clip(words, room), dim());
            }
        }
        3 if slow => {
            link(frame, gui, x, y, &t!("gui.ytdl.key.type"), true, YtAct::Type);
        }
        _ => {}
    }
}

/// What the place row says before there is a song: the library and the
/// layout new songs go by (`Music · Artist / Album`).
fn choice_words(gui: &Gui) -> String {
    let choice = room_choice(gui);
    let layout = match choice.kind {
        LayoutKind::Custom => choice.layout.clone(),
        kind => kind.label().text(),
    };
    if layout.is_empty() { choice.library } else { format!("{} · {layout}", choice.library) }
}

/// A row of the form that opens a sheet: its label TEXT at the left, its
/// value ending two cells before the `▸` at the right — the whole row the
/// control, BRIGHT under the pointer. Returns where the value may end and
/// the cells it may take.
fn sheet_row(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, label: &str, key_word: &str, act: YtAct) -> (u16, usize) {
    let row = Rect { x: block.x, y, width: block.width, height: 1 };
    let quiet = gui.ytdl.quiet;
    let hover = !quiet && gui.ui.hovers(row);
    put(frame, block.x, y, label, if hover { bright_bold() } else { Style::default() });
    let glyph = super::forward_glyph();
    let at = block.right().saturating_sub(width(glyph) as u16);
    put(frame, at, y, glyph, if hover { Style::default().fg(th().bright) } else { dim() });
    if !quiet {
        gui.ui.click(row, yt(act.clone()));
        gui.ytdl.drawn.push((page_words(key_word).0, act));
    }
    let end = at.saturating_sub(1);
    (end, usize::from(end.saturating_sub(block.x + width(label) as u16 + 2)))
}

/// `Saves to` — the Files window's `Saves here` while it adds into the
/// folder on screen — and the place: the song's own crumbs once there is
/// one, else what new songs go by.
fn draw_place_row(frame: &mut Frame, gui: &mut Gui, form: Form, view: &RoomView, block: Rect, y: u16) {
    let here = form == Form::Window && super::ytdl_here::is_here(gui, &view.target);
    let label = t!(if here { "gui.ytdl.here.saves_here" } else { "gui.ytdl.room.saves_to" }).to_string();
    let (key, act) = match form {
        Form::Room => (t!("gui.ytdl.key.change"), YtAct::Change),
        Form::Window => (t!("gui.ytdl.key.subfolders"), YtAct::Here(super::ytdl_here::HereAct::Subfolders)),
    };
    let (end, room) = sheet_row(frame, gui, block, y, &label, &key, act);
    let draft = draft(gui, form);
    let song = draft.found() || draft.manual || form == Form::Window;
    if !song {
        let words = super::bar::clip(&choice_words(gui), room).into_owned();
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, dim());
    } else if view.target.is_valid() {
        let crumbs = view.target.crumbs();
        let at = end.saturating_sub(place_width(&crumbs, view.new_folder, room));
        draw_place(frame, at, y, &crumbs, view.new_folder, room);
    } else {
        let dash = super::discover::dash();
        put(frame, end.saturating_sub(width(dash) as u16), y, dash, dim());
    }
}

/// `Format` and the format the song goes as.
fn draw_format_row(frame: &mut Frame, gui: &mut Gui, form: Form, block: Rect, y: u16) {
    let label = t!("gui.ytdl.room.format").to_string();
    let name = format_name(&form_codec(gui, form));
    let (end, room) = sheet_row(frame, gui, block, y, &label, &t!("gui.ytdl.key.format"), YtAct::Format);
    let name = super::bar::clip(&name, room).into_owned();
    put(frame, end.saturating_sub(width(&name) as u16), y, &name, dim());
}

/// The song's cover in its slot, 8 × 4: the video's thumbnail once it is
/// here — pixels where the terminal draws them, the mosaic under an
/// overlay — else the kit's empty slot (while the video is read, or where
/// there is no picture to be had). `own`: the window the cover stands in,
/// whose own footprint is no overlay over it.
pub(super) fn draw_cover(frame: &mut Frame, gui: &mut Gui, form: Form, rect: Rect, own: Option<Rect>) {
    let id = draft(gui, form).parsed.video_id.clone().filter(|_| !matches!(draft(gui, form).look, Look::Reading { .. }));
    let Gui { app, ytdl, ui, hot, .. } = &mut *gui;
    let YtUi { thumbs, slots, .. } = ytdl;
    let art = id.and_then(|id| thumbs.iter().find(|(i, _)| *i == id)).and_then(|(_, thumb)| match thumb {
        Thumb::Got(art) => Some(art),
        _ => None,
    });
    let Some(art) = art else {
        super::bar::cover_slot(frame, rect.x, rect.y, rect.width, rect.height);
        return;
    };
    let slot = slots[usize::from(form == Form::Window)].get_or_insert_with(|| super::cover::Slot::new(app.graphics.fork()));
    let under = match own {
        Some(own) => ui.covered_last_frame_by_another(rect, own),
        None => ui.covered_last_frame(rect),
    };
    if under {
        slot.draw_mosaic(frame, rect, art);
    } else {
        let pace = super::cover::Pace::frame();
        slot.draw_paced(frame, rect, art, &pace);
        // A slot the budget turned away wants the very next frame.
        if pace.starved() {
            *hot = true;
        }
    }
}

// ── Drawing the room ────────────────────────────────────────────────────────

/// The block the room draws in: the room's width up to [`BLOCK_MAX`],
/// centred in what is left.
pub(super) fn room_block(content: Rect) -> Rect {
    let w = content.width.min(BLOCK_MAX);
    Rect { x: content.x + (content.width - w) / 2, width: w, ..content }
}

/// The room's primary, tall and the block's width: a Rounded frame, its
/// label centred, both ACCENT (the label BOLD) — BRIGHT under the
/// pointer; off, DIM, no click and no `▸`.
fn draw_primary(frame: &mut Frame, gui: &mut Gui, at: Rect, label: &str, enabled: bool) {
    let live = enabled && !gui.ytdl.quiet;
    let hovered = live && gui.ui.hovers(at);
    let (color, bold) = match (enabled, hovered) {
        (false, _) => (th().dim, false),
        (true, true) => (th().bright, true),
        (true, false) => (th().accent, true),
    };
    crate::kit::cursor_ring(frame, at, Style::default().fg(color));
    let room = at.width.saturating_sub(2);
    let label = super::bar::clip(label, usize::from(room));
    let x = at.x + 1 + (room.saturating_sub(width(&label) as u16) + 1) / 2;
    let style = Style::default().fg(color);
    put(frame, x, at.y + 1, &label, if bold { style.add_modifier(Modifier::BOLD) } else { style });
    if live {
        gui.ui.click(at, yt(YtAct::Add));
    }
}

/// The primary where the frame is too short for the tall one: a one-row
/// slab at the title row's right end — ACCENT ground, its label BOLD;
/// off, DIM words and no ground. Returns where it starts.
fn draw_primary_slab(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, label: &str, enabled: bool) -> u16 {
    let words = format!(" {label} ");
    let at = Rect { x: block.right().saturating_sub(width(&words) as u16), y, width: width(&words) as u16, height: 1 };
    let live = enabled && !gui.ytdl.quiet;
    let style = if !enabled {
        dim()
    } else if live && gui.ui.hovers(at) {
        Style::default().bg(th().bright).fg(th().on_accent).add_modifier(Modifier::BOLD)
    } else {
        sel().add_modifier(Modifier::BOLD)
    };
    put(frame, at.x, y, &words, style);
    if live {
        gui.ui.click(at, yt(YtAct::Add));
    }
    at.x
}

/// The room (the canvas's Main, Empty, After, Min, Wide and the states):
/// the title; the link's band; what the link came to; the two rows; the
/// primary, tall under them — a slab on the title row where the frame is
/// too short for it; then the From YouTube list, down to the room's last
/// row. Short of rows the blanks give way — the one under the title, then
/// the form's from the bottom up, then the one over the list — until the
/// list has its three.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    gui.ytdl.drawn.clear();
    let view = room_view(gui);
    let block = room_block(content);
    let (x, w) = (block.x, usize::from(block.width));
    let bottom = content.bottom();

    // The press out, every control stands where it is, neither clicking
    // nor lighting; the nav is the way on.
    gui.ytdl.quiet = gui.ytdl.draft.starting.is_some();
    let keys = room_keys(gui);
    let mut rows = form_rows(gui, Form::Room, &view, w);
    let above = above_rows(&view, w);
    let list = list_plan(gui, block);

    // What fits: the tall primary where the whole form does, with the
    // list's header and a row under it; else the slab, and the blanks
    // give way for the list's rows.
    let h = usize::from(content.height);
    let wants = list.wants.min(LIST_WANTS);
    let used = |title_blank: bool, rows: usize, tall: bool, list_blank: bool| {
        1 + usize::from(title_blank) + 3 + rows + above.len() + if tall { 4 } else { 0 } + usize::from(list_blank) + 1
    };
    let tall = view.primary.is_some() && used(true, rows.len(), true, true) + 1 <= h;
    let (mut title_blank, mut list_blank) = (true, true);
    if !tall {
        if used(title_blank, rows.len(), false, list_blank) + wants > h {
            title_blank = false;
        }
        while used(title_blank, rows.len(), false, list_blank) + wants > h {
            let Some(at) = rows.iter().rposition(|row| matches!(row, Row::Blank)) else { break };
            rows.remove(at);
        }
        if used(title_blank, rows.len(), false, list_blank) + wants > h {
            list_blank = false;
        }
    }

    // Title row — and the slab, which the title gives way to: two cells
    // clear of it, cut where a language's words are longer than that.
    let mut y = content.y;
    let mut title_end = block.right();
    if let Some((label, enabled)) = view.primary.clone().filter(|_| !tall) {
        title_end = draw_primary_slab(frame, gui, block, y, &label, enabled).saturating_sub(2);
    }
    let title = super::bar::clip(&t!("gui.ytdl.room.title"), usize::from(title_end.saturating_sub(x))).into_owned();
    put(frame, x, y, &title, Style::default().add_modifier(Modifier::BOLD));
    y += 1 + u16::from(title_blank);

    // The link's band.
    draw_link_band(frame, gui, Form::Room, Rect { x, y, width: block.width, height: 3 }, keys == Keys::Link);
    y += 3;

    // The form: a card's four rows hold its cover.
    let mut cover = None;
    for row in rows {
        if y >= bottom {
            break;
        }
        if matches!(row, Row::Card(0) | Row::Reading(0)) && y + COVER_H <= bottom {
            cover = Some(Rect { x, y, width: COVER_W, height: COVER_H });
        }
        draw_row(frame, gui, Form::Room, &view, block, y, row);
        y += 1;
    }
    if let Some(rect) = cover {
        draw_cover(frame, gui, Form::Room, rect, None);
    }

    // Over the primary: a refusal, or why it's off.
    for (text, style, right) in above {
        if y >= bottom {
            break;
        }
        let text = super::bar::clip(&text, w);
        let at = if right { block.right().saturating_sub(width(&text) as u16) } else { x };
        put(frame, at, y, &text, style);
        y += 1;
    }
    if let (Some((label, enabled)), true) = (view.primary.clone(), tall) {
        y += 1;
        draw_primary(frame, gui, Rect { x, y, width: block.width, height: 3 }, &label, enabled);
        y += 3;
    }
    gui.ytdl.quiet = false;

    // The list, in what is left.
    y += u16::from(list_blank);
    if y + 1 < bottom {
        draw_list(frame, gui, Rect { x, y, width: block.width, height: bottom - y }, list);
    } else {
        gui.ytdl.list.rows = 0;
    }
}

// ── The overlays ────────────────────────────────────────────────────────────

/// Draw what stands over the room or Files: the Files window (and the
/// sheet its `Saves here` opens), or Saves to or Choose a folder in its
/// place; then the Edit tags sheet, the Format sheet and a song's list —
/// each after what it stands over, so its rects win the pointer.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if !surface_up(gui) {
        return;
    }
    if super::ytdl_where::is_open(gui) {
        super::ytdl_where::draw(frame, gui, area);
    } else if super::ytdl_here::window_up(gui) {
        super::ytdl_here::draw_window(frame, gui, area);
    }
    if tags_up(gui) {
        super::ytdl_sheets::draw_tags(frame, gui, area);
    }
    if formats_up(gui) {
        super::ytdl_sheets::draw_formats(frame, gui, area);
    }
    if menu_up(gui) {
        draw_song_menu(frame, gui, area);
    }
}

/// Open the Format sheet: for the song on screen, or for a row to add
/// again in the format taken. The dot starts on the format in force.
fn open_formats(gui: &mut Gui, job: Option<String>) {
    let options = formats_offered(gui);
    let current = match &job {
        Some(id) => gui.ytdl.jobs().and_then(|j| j.job(id)).map(|j| j.codec.clone()).unwrap_or_default(),
        None => form_codec(gui, active_form(gui)),
    };
    let cursor = options.iter().position(|o| *o == current.as_str()).unwrap_or(0);
    gui.ytdl.formats = Some(FormatSheet { cursor, job });
}

/// The Format sheet hung over a form goes with the form (a row's stays
/// with the room).
pub(super) fn drop_form_formats(gui: &mut Gui) {
    if gui.ytdl.formats.as_ref().is_some_and(|m| m.job.is_none()) {
        gui.ytdl.formats = None;
    }
}

/// The format taken: the song on screen (the room's or the Files
/// window's) goes as it and the server remembers it (only the format — the
/// rest of its choices stay as they were); a row's Change format and retry
/// adds again in it.
fn take_format(gui: &mut Gui) {
    let Some(sheet) = gui.ytdl.formats.take() else { return };
    let Some(codec) = formats_offered(gui).get(sheet.cursor).map(|c| c.to_string()) else { return };
    match sheet.job {
        Some(id) => {
            let next = gui.ytdl.jobs_mut().and_then(|jobs| jobs.retry(&id, Redo { codec: Some(codec), ..Redo::default() }));
            follow_retry(gui, &id, next);
        }
        None => {
            let form = active_form(gui);
            if form_codec(gui, form) == codec {
                return;
            }
            let id = identity(gui);
            let chosen = codec.clone();
            super::servers::update_config(gui, |c| {
                let mut prefs = config::ytdl_prefs(c, &id);
                prefs.format = Some(chosen);
                config::set_ytdl_prefs(c, &id, &prefs);
            });
            let draft = draft_mut(gui, form);
            draft.codec = Some(codec);
            // A refused format or a taken name: another format is the way on.
            if draft.refusal.as_ref().is_some_and(|f| matches!(f.problem, Problem::Exists | Problem::BadFormat)) {
                draft.refusal = None;
            }
        }
    }
}

// ── The list ────────────────────────────────────────────────────────────────

/// The From YouTube list's items, as it lists them: the open adds, those
/// that need a look, then the Added list.
fn list_items(jobs: &Jobs) -> Vec<Item> {
    let mut open: Vec<&Job> = jobs.adding();
    open.sort_by_key(|j| j.state != JobState::Adding);
    let mut items: Vec<Item> = open.into_iter().map(|j| Item::Job(j.id.clone())).collect();
    items.extend(jobs.needs_look().into_iter().map(|j| Item::Job(j.id.clone())));
    items.extend(jobs.added().into_iter().map(Item::Song));
    items
}

/// A job row's lines under its title: the words (wrapped) and its ways on.
fn job_detail(gui: &Gui, job: &Job, room: usize) -> (Vec<String>, Vec<(String, Way)>) {
    let formats = formats_offered(gui);
    let ways: Vec<(String, Way)> = job
        .ways(Some(&formats))
        .into_iter()
        // A row not sure of its add offers the check alone; x dismisses
        // it all the same.
        .filter(|w| !(matches!(job.state, JobState::NotSureStarted | JobState::NotSureFinished) && *w == Way::Dismiss))
        .map(|w| (way_words(w), w))
        .collect();
    let text = match job.state {
        JobState::NotFound => Some(t!("gui.ytdl.list.not_found").to_string()),
        JobState::Failed | JobState::StartFailed => job.failure().map(|f| {
            let server = server_name(gui);
            let words = Words { surface: Surface::List, server: &server, codec: &job.codec, tier: tier(gui) };
            failure_label(&f, &words).text()
        }),
        _ => None,
    };
    let lines = text.map(|t| crate::kit::wrap_whole(&t, room.max(1))).unwrap_or_default();
    (lines, ways)
}

fn way_words(way: Way) -> String {
    t!(match way {
        Way::Retry => "gui.ytdl.list.try_again",
        Way::ChangeFormat => "gui.ytdl.list.change_format",
        Way::ChooseFolder => "gui.ytdl.list.choose_folder",
        Way::TopOfLibrary => "gui.ytdl.list.top",
        Way::CheckAgain => "gui.ytdl.list.check_again",
        Way::AddAgain => "gui.ytdl.list.add_again",
        Way::Dismiss => "gui.ytdl.list.dismiss",
    })
    .to_string()
}

/// The ways' cells on a row, ` · ` between them.
fn ways_width(ways: &[(String, Way)]) -> usize {
    let sep = width(" · ");
    ways.iter().map(|(w, _)| width(w)).sum::<usize>() + sep * ways.len().saturating_sub(1)
}

/// The ways share the detail's last line where both fit three cells
/// apart, else take a row of their own.
fn ways_share(lines: &[String], ways: &[(String, Way)], w: usize) -> bool {
    lines.last().is_some_and(|l| 2 + width(l) + 3 + ways_width(ways) <= w)
}

/// The ways as rows of the list: as many a row as fit `w` cells, where a
/// language's words are longer than one row holds.
fn way_rows(ways: &[(String, Way)], w: usize) -> Vec<&[(String, Way)]> {
    let mut rows = Vec::new();
    let mut from = 0;
    for to in 1..=ways.len() {
        if to - from > 1 && ways_width(&ways[from..to]) > w {
            rows.push(&ways[from..to - 1]);
            from = to - 1;
        }
    }
    if from < ways.len() {
        rows.push(&ways[from..]);
    }
    rows
}

/// An item's rows in a list `w` cells wide.
fn item_rows(gui: &Gui, item: &Item, w: usize) -> usize {
    match item {
        Item::Song(_) => 1,
        Item::Job(id) => {
            let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)) else { return 0 };
            if job.state.open() {
                return 1;
            }
            let (lines, ways) = job_detail(gui, job, w.saturating_sub(2));
            1 + lines.len() + way_rows(&ways, w).len() - usize::from(ways_share(&lines, &ways, w))
        }
    }
}

fn mark(job: &Job) -> (&'static str, Style) {
    let legacy = legacy_conhost();
    match job.state {
        JobState::Adding => (if legacy { "v" } else { "↓" }, accent()),
        JobState::Starting | JobState::StartUnsure => (if legacy { "~" } else { "◷" }, dim()),
        JobState::NotSureStarted | JobState::NotSureFinished => ("?", dim()),
        JobState::Added => (keep::tick(), Style::default().fg(th().ok)),
        _ => ("!", gold()),
    }
}

/// How long ago, for a row: just now, minutes, hours, else the day.
pub(super) fn ago(gui: &mut Gui, then: i64, now: i64) -> String {
    let secs = (now - then).max(0) / 1000;
    if secs < 60 {
        return t!("gui.ytdl.list.just_now").to_string();
    }
    if secs < 3600 {
        return t!("gui.ytdl.list.min_ago", count = secs / 60).to_string();
    }
    if secs < 86_400 {
        return t!("gui.ytdl.list.hours_ago", count = secs / 3600).to_string();
    }
    let zone = gui.ytdl.zone.get_or_insert_with(crate::admin::tz::local);
    let offset = zone.as_ref().map_or(0, |z| z.offset_at(then / 1000));
    let day = |t: i64| (t / 1000 + i64::from(offset)).div_euclid(86_400);
    let (y, m, d) = crate::admin::tz::civil_from_days(day(then));
    let (this_year, _, _) = crate::admin::tz::civil_from_days(day(now));
    let month = t!("sta.months_short").split(" · ").nth(m as usize - 1).unwrap_or_default().to_string();
    if y == this_year {
        t!("sta.day_label", month = month, day = d).to_string()
    } else {
        t!("sta.day_label_year", month = month, day = d, year = y).to_string()
    }
}

fn verb_glyphs() -> [&'static str; 3] {
    if legacy_conhost() { ["[>]", "[+]", "[.]"] } else { ["[▸]", "[+]", "[⋯]"] }
}

/// What the list has to show this frame, and how its columns stand in
/// the block.
struct ListPlan {
    items: Vec<Item>,
    heights: Vec<usize>,
    /// Earlier songs can be asked for: `Show earlier` is the last row.
    more: bool,
    /// The first time, with nothing added and nothing adding: its words.
    words: Vec<String>,
    /// The rows it would take, all of it.
    wants: usize,
    /// The title's column: the block's width less the mark and the cells
    /// kept for the place and the when.
    title_w: usize,
}

fn list_plan(gui: &Gui, block: Rect) -> ListPlan {
    let w = usize::from(block.width);
    // The place and the when share 22 cells at the floor and one more for
    // every four the block grows; the title has the rest.
    let zone = 22 + w.saturating_sub(usize::from(BLOCK_MIN)) / 4;
    let title_w = w.saturating_sub(3 + zone).max(8);
    let (items, more, loading) = match gui.ytdl.jobs() {
        Some(jobs) => (list_items(jobs), jobs.can_show_earlier(), jobs.added_loading()),
        None => (Vec::new(), false, true),
    };
    let heights: Vec<usize> = items.iter().map(|i| item_rows(gui, i, w)).collect();
    let words: Vec<String> = if items.is_empty() && !more && !loading {
        [t!("gui.ytdl.room.empty_title"), t!("gui.ytdl.room.empty_web")]
            .iter()
            .flat_map(|line| crate::kit::wrap_whole(line, w.max(1)))
            .collect()
    } else {
        Vec::new()
    };
    let wants = heights.iter().sum::<usize>() + usize::from(more) + words.len();
    ListPlan { items, heights, more, words, wants, title_w }
}

/// The From YouTube list, under the form: its header, DIM capitals; the
/// open adds, those that need a look with their ways on, then the songs
/// that came in — as many as the room has rows for, the rest under the
/// wheel and the keys — and `Show earlier` after the last of them.
fn draw_list(frame: &mut Frame, gui: &mut Gui, rect: Rect, plan: ListPlan) {
    let (x, end) = (rect.x, rect.right());
    let w = usize::from(rect.width);
    put(frame, x, rect.y, &super::bar::clip(&t!("gui.ytdl.list.title").to_uppercase(), w), dim());
    let top = rect.y + 1;
    let fits = usize::from(rect.height.saturating_sub(1));
    let ListPlan { items, heights, more, words, title_w, .. } = plan;
    for (y, line) in (top..rect.bottom()).zip(&words) {
        put(frame, x, y, &super::bar::clip(line, w), dim());
    }
    let tail = usize::from(more);
    let list = &mut gui.ytdl.list;
    // The cursor and a song's list stay on their row as the rows move — a
    // row added again lists with the open ones, a new add above them all.
    let now_at = |at: usize| list.items.get(at).and_then(|was| items.iter().position(|item| same_item(item, was)));
    let held_at = list.held.and_then(now_at);
    let menu_at = list.menu.filter(|(at, _)| *at < list.items.len()).map(|(at, cursor)| (now_at(at), cursor));
    if let Some(at) = held_at {
        list.held = Some(at);
    }
    if let Some((at, cursor)) = menu_at {
        list.menu = at.map(|at| (at, cursor));
    }
    list.items = items.clone();
    list.rows = fits;
    list.menu_at = None;
    if items.is_empty() {
        list.held = None;
        list.view.stow();
        list.menu = None;
    }
    if let Some(held) = list.held {
        list.held = Some(held.min(items.len().saturating_sub(1)));
    }
    // The first item: no further down than leaves the last rows showing,
    // the held one shown, and as many after it as fit.
    let mut last_first = items.len();
    let mut rows = tail;
    while last_first > 0 && rows + heights[last_first - 1] <= fits {
        last_first -= 1;
        rows += heights[last_first];
    }
    let mut first = list.view.scroll.min(last_first.min(items.len().saturating_sub(1)));
    if let Some(held) = list.held.filter(|_| list.view.held && std::mem::take(&mut list.view.reveal)) {
        first = first.min(held);
        while first < held && heights[first..=held].iter().sum::<usize>() > fits {
            first += 1;
        }
    }
    list.view.scroll = first;
    let held = list.held.filter(|_| list.view.held);
    let menu = list.menu.map(|(at, _)| at);

    let now = gui.ytdl.now();
    let mut y = top;
    let mut shown = first;
    for (i, item) in items.iter().enumerate().skip(first) {
        let h = heights[i] as u16;
        if y + h > rect.bottom() {
            break;
        }
        let on = held == Some(i);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), Rect { x, y, width: rect.width, height: h });
        }
        match item {
            Item::Job(id) => draw_job(frame, gui, id, (x, y, end), on, now),
            Item::Song(song) => {
                let anchor = draw_song(frame, gui, song, i, (x, y, end), title_w, (on, menu == Some(i)), now);
                if menu == Some(i) {
                    gui.ytdl.list.menu_at = Some(anchor);
                }
            }
        }
        y += h;
        shown = i + 1;
    }
    // `Show earlier`, once the last song shows and a row is left for it.
    if more && shown == items.len() && y < rect.bottom() {
        let words = t!("gui.ytdl.list.show_earlier").to_string();
        let sx = end.saturating_sub(width(&words) as u16);
        text_button(frame, gui, sx, y, &words, false, yt(YtAct::ShowEarlier));
    }
    // A song's list whose row scrolled out of sight has nothing to hang from.
    if menu.is_some() && gui.ytdl.list.menu_at.is_none() {
        gui.ytdl.list.menu = None;
    }
}

/// The last folder of a place, for a row: the album's, as a rule.
fn last_crumb(folder: &str) -> String {
    path_crumbs(folder).pop().unwrap_or_default()
}

/// A job's rows: its mark and title, and where it stands at the right;
/// under a failure the words and its ways on.
fn draw_job(frame: &mut Frame, gui: &mut Gui, id: &str, (x, y, end): (u16, u16, u16), on: bool, now: i64) {
    let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)).cloned() else { return };
    let waiting = gui.ytdl.jobs().is_some_and(|j| j.waiting());
    let w = usize::from(end.saturating_sub(x));
    let base = |style: Style| if on { sel() } else { style };
    let (glyph, mark_style) = mark(&job);
    put(frame, x, y, glyph, base(mark_style));
    // Where it stands, at the right.
    let (right, style) = match job.state {
        _ if job.state.open() && waiting => (t!("gui.ytdl.list.waiting").to_string(), dim()),
        JobState::Adding => {
            let mut words = vec![t!("gui.ytdl.list.adding").to_string()];
            let since = job.sent_at.unwrap_or(job.created_at);
            let minutes = (now - since).max(0) / 60_000;
            if minutes > 0 {
                words.push(t!("gui.ytdl.list.minutes", count = minutes).to_string());
            }
            words.push(format_name(&job.codec));
            (words.join(" · "), accent())
        }
        JobState::Starting | JobState::StartUnsure => (keep::ell(&t!("gui.ytdl.list.starting")), dim()),
        JobState::NotSureStarted => (t!("gui.ytdl.list.not_sure_started").to_string(), dim()),
        JobState::NotSureFinished => (t!("gui.ytdl.list.not_sure_finished").to_string(), dim()),
        JobState::Added => (String::new(), dim()),
        _ => (t!("gui.ytdl.list.failed").to_string(), gold()),
    };
    let right = super::bar::clip(&right, w.saturating_sub(4)).into_owned();
    let right_w = width(&right) as u16;
    put(frame, end.saturating_sub(right_w), y, &right, base(style));
    // The title has the row up to two cells before them.
    let room = usize::from(end.saturating_sub(right_w + 2).saturating_sub(x + 2));
    put(frame, x + 2, y, &super::bar::clip(&job.shown_title(), room), base(Style::default()));
    if job.state.open() {
        return;
    }
    // The words, then the ways on — sharing the last line where both fit.
    let (lines, ways) = job_detail(gui, &job, w.saturating_sub(2));
    let mut ly = y + 1;
    for line in &lines {
        put(frame, x + 2, ly, &super::bar::clip(line, w.saturating_sub(2)), base(dim()));
        ly += 1;
    }
    let mut wy = if ways_share(&lines, &ways, w) { ly - 1 } else { ly };
    for row in way_rows(&ways, w) {
        let mut wx = end.saturating_sub(ways_width(row).min(w) as u16);
        for (i, (words, way)) in row.iter().enumerate() {
            if i > 0 {
                put(frame, wx, wy, " · ", base(dim()));
                wx += width(" · ") as u16;
            }
            let words = super::bar::clip(words, w);
            let rect = Rect { x: wx, y: wy, width: width(&words) as u16, height: 1 };
            let style = if gui.ui.hovers(rect) { bright_bold() } else { base(Style::default()) };
            put(frame, wx, wy, &words, style);
            gui.ui.click(rect, yt(YtAct::Way(job.id.clone(), *way)));
            wx += rect.width;
        }
        wy += 1;
    }
}

/// A song's row: ✓, the title, its folder and when it came in — or, under
/// the pointer, the keys' hand or its own list, its verbs. Returns where
/// its list hangs from.
#[allow(clippy::too_many_arguments)]
fn draw_song(
    frame: &mut Frame,
    gui: &mut Gui,
    song: &AddedRow,
    i: usize,
    (x, y, end): (u16, u16, u16),
    title_w: usize,
    (on, menu): (bool, bool),
    now: i64,
) -> Rect {
    let row = Rect { x, y, width: end.saturating_sub(x), height: 1 };
    let playable = song.present && song.file_path.is_some();
    let hover = gui.ui.hovers(row) && playable;
    let base = |style: Style| if on { sel() } else { style };
    let ok = if song.present { Style::default().fg(th().ok) } else { dim() };
    put(frame, x, y, keep::tick(), base(ok));
    let title_style = match (on, hover, song.present) {
        (true, _, _) => sel().add_modifier(Modifier::BOLD),
        (false, true, _) => bright_bold(),
        (false, false, true) => Style::default(),
        (false, false, false) => dim(),
    };
    if playable {
        gui.ui.click(row, yt(YtAct::Play(i)));
        gui.ui.context(row, yt(YtAct::More(i)));
    }
    put(frame, x + 2, y, &super::bar::clip(&song.title, title_w), title_style);
    let verbs = verb_glyphs();
    let verbs_w = (verbs.iter().map(|v| width(v)).sum::<usize>() + verbs.len() - 1) as u16;
    let mut menu_anchor = Rect { x: end.saturating_sub(width(verbs[2]) as u16), y, width: width(verbs[2]) as u16, height: 1 };
    let right_w = if playable && (hover || on || menu) {
        // The verbs: play, add to the queue, the song's list.
        let mut vx = end.saturating_sub(verbs_w);
        for (k, glyph) in verbs.iter().enumerate() {
            let rect = Rect { x: vx, y, width: width(glyph) as u16, height: 1 };
            let style = if gui.ui.hovers(rect) { bright_bold() } else { base(Style::default()) };
            put(frame, vx, y, glyph, style);
            let act = match k {
                0 => YtAct::Play(i),
                1 => YtAct::Queue(i),
                _ => YtAct::More(i),
            };
            gui.ui.click(rect, yt(act));
            if k == 2 {
                menu_anchor = rect;
            }
            vx += rect.width + 1;
        }
        verbs_w + 1
    } else {
        let words = if song.present {
            song.at.map(|at| ago(gui, at, now)).unwrap_or_default()
        } else {
            t!("gui.ytdl.list.file_missing").to_string()
        };
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, base(dim()));
        width(&words) as u16
    };
    let px = x + 2 + title_w as u16 + 1;
    let room = usize::from(end.saturating_sub(right_w + 1).saturating_sub(px));
    if room >= 3 {
        put(frame, px, y, &super::bar::clip(&last_crumb(&song.folder), room), base(dim()));
    }
    menu_anchor
}

/// A song's list: Add to queue `a`, Show in folder — hung under its row
/// from the `[⋯]`, right edges level; over the row where the room has no
/// rows left under it.
fn draw_song_menu(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let (Some(anchor), Some((_, cursor))) = (gui.ytdl.list.menu_at, gui.ytdl.list.menu) else { return };
    let rows = [(t!("gui.ytdl.list.add_to_queue").to_string(), "a"), (t!("gui.ytdl.list.show_in_folder").to_string(), "")];
    let inner_w =
        rows.iter().map(|(label, key)| width(label) + if key.is_empty() { 0 } else { 2 + width(key) }).max().unwrap_or(10) + 2;
    let w = inner_w as u16 + 2;
    let h = rows.len() as u16 + 2;
    gui.ui.clear_registries();
    gui.ui.click(area, yt(YtAct::MenuClose));
    let x = anchor.right().saturating_sub(w);
    let y = if anchor.bottom() + h <= gui.room_rect().bottom() { anchor.bottom() } else { anchor.y.saturating_sub(h) };
    let rect = Rect { x, y, width: w, height: h };
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    for (i, (label, key)) in rows.iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let on = cursor == Some(i);
        let hover = !on && gui.ui.hovers(row);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let style = match (on, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, row.x + 1, row.y, label, style);
        if !key.is_empty() {
            put(frame, row.right().saturating_sub(1 + width(key) as u16), row.y, key, if on { sel() } else { dim() });
        }
        gui.ui.click(row, yt(YtAct::MenuPick(i)));
    }
}

// ── The list's acts ─────────────────────────────────────────────────────────

/// Whatever hangs over the room — Choose a folder, a sheet, Saves to — let
/// go, as the room yields to Files: Files counts as their ground, so the
/// frame's sweep would keep them (`discover::reopen` lets its own go the
/// same way).
fn close_overlays(gui: &mut Gui) {
    gui.ytdl.formats = None;
    gui.ytdl.place = None;
    gui.ytdl.draft.tags_open = false;
    gui.ytdl.list.menu = None;
}

fn song_at(gui: &Gui, i: usize) -> Option<AddedRow> {
    match gui.ytdl.list.items.get(i)? {
        Item::Song(song) => Some(song.clone()),
        Item::Job(_) => None,
    }
}

fn song_track(song: &AddedRow) -> Option<Track> {
    let filepath = song.file_path.clone().filter(|_| song.present)?;
    Some(Track {
        filepath,
        metadata: TrackMetadata { title: Some(song.title.clone()), ..TrackMetadata::default() },
    })
}

fn play_song(gui: &mut Gui, i: usize, play: bool) {
    let Some(track) = song_at(gui, i).as_ref().and_then(song_track) else { return };
    let origin = gui.app.origin();
    let effects = if play { gui.app.queue_track_next(&origin, track, true) } else { gui.app.queue_track_end(&origin, track) };
    gui.pend(effects);
}

/// Show in folder: Files on the song's folder, the cursor on the song once
/// it is listed.
fn show_in_folder(gui: &mut Gui, i: usize) {
    let Some(path) = song_at(gui, i).and_then(|s| s.file_path) else { return };
    let Some((folder, name)) = path.trim_matches('/').rsplit_once('/') else { return };
    let (folder, name) = (folder.to_string(), name.to_string());
    gui.act(Act::Nav(super::FILES_NAV));
    gui.app.tab = Tab::Files;
    gui.app.path = folder.clone();
    // A jump, not a step in: the columns walked to before are no way back
    // from here — Back asks for the parent instead.
    gui.app.files.trail.clear();
    gui.app.files.set(Vec::new());
    gui.pend(vec![crate::tui::app::Effect::Api(ApiCmd::Browse(folder.clone()))]);
    gui.ytdl.seat = Some((folder, name));
}

/// Show in folder's seat: once Files lists the folder, the cursor rests on
/// the song (the list-cursor law's exception: it is lit, held).
fn seat(gui: &mut Gui) -> bool {
    let Some((folder, name)) = gui.ytdl.seat.clone() else { return false };
    if gui.app.path.trim_matches('/') != folder {
        gui.ytdl.seat = None;
        return false;
    }
    if gui.app.files.loading {
        return false;
    }
    gui.ytdl.seat = None;
    let Some(i) = gui.app.files.entries.iter().position(|e| e.label() == name) else { return false };
    gui.app.focus = Focus::Browser;
    gui.app.files.state.select(Some(i));
    gui.files_view.pick_up();
    true
}

fn way(gui: &mut Gui, id: String, way: Way) {
    match way {
        Way::Retry => {
            let next = gui.ytdl.jobs_mut().and_then(|jobs| jobs.retry(&id, Redo::default()));
            follow_retry(gui, &id, next);
        }
        Way::ChangeFormat => open_formats(gui, Some(id)),
        Way::TopOfLibrary => {
            let library = gui.ytdl.jobs().and_then(|j| j.job(&id)).and_then(|j| path_crumbs(&j.directory).first().cloned());
            if let (Some(library), Some(jobs)) = (library, gui.ytdl.jobs_mut()) {
                let next = jobs.retry(&id, Redo { directory: Some(format!("/{library}/")), ..Redo::default() });
                follow_retry(gui, &id, next);
            }
        }
        Way::CheckAgain => {
            if let Some(jobs) = gui.ytdl.jobs_mut() {
                jobs.check_again(&id);
            }
        }
        Way::AddAgain => {
            if let Some(jobs) = gui.ytdl.jobs_mut() {
                jobs.add_again(&id);
            }
        }
        Way::Dismiss => {
            if let Some(jobs) = gui.ytdl.jobs_mut() {
                jobs.dismiss(&id);
            }
        }
        // Choose a folder stands over the room; the row adds again into
        // the folder used.
        Way::ChooseFolder => super::ytdl_where::open_choose_for_job(gui, &id),
    }
}

/// A row added again is a new job, listed with the open ones: the list's
/// cursor goes with it (the next frame seats it where the row now is).
pub(super) fn follow_retry(gui: &mut Gui, old: &str, next: Option<String>) {
    let Some(next) = next else { return };
    for item in &mut gui.ytdl.list.items {
        if matches!(item, Item::Job(id) if id == old) {
            *item = Item::Job(next.clone());
        }
    }
}

/// The same row of the list, from one frame to the next: a job by its id,
/// a song by its file and title.
fn same_item(a: &Item, b: &Item) -> bool {
    match (a, b) {
        (Item::Job(a), Item::Job(b)) => a == b,
        (Item::Song(a), Item::Song(b)) => a.file_path == b.file_path && a.title == b.title && a.folder == b.folder,
        _ => false,
    }
}

/// The wheel over a YouTube window: Choose a folder's rows.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if super::ytdl_where::is_open(gui) {
        super::ytdl_where::wheel(gui, delta);
    }
}

/// The wheel over the room: its list's rows.
pub(crate) fn wheel_room(gui: &mut Gui, delta: i32) {
    gui.act(yt(YtAct::ScrollBy(delta)));
}

// ── Acts ────────────────────────────────────────────────────────────────────

/// The primary: Add to library, Add here (Add anyway, Enter again after a
/// refusal) — the start handed to the tracker, never twice for the same
/// link and folder.
pub(super) fn press(gui: &mut Gui) {
    let form = active_form(gui);
    let view = form_view(gui, form);
    let Some(request) = view.request.filter(|_| view.block == AddBlock::None) else { return };
    let Some(jobs) = gui.ytdl.jobs_mut() else { return };
    // The tracker doesn't guard a double start: the rules' "Already adding
    // to this folder" does, here too.
    if jobs.is_adding(&request.url, &request.directory) {
        return;
    }
    let id = jobs.start(request);
    let draft = draft_mut(gui, form);
    draft.refusal = None;
    draft.starting = Some(id);
    draft.keys = Keys::None;
    draft.tags_open = false;
}

/// The Edit tags sheet over a form, a tag holding the keys.
pub(super) fn open_tags(gui: &mut Gui, form: Form, field: TagField) {
    gui.ytdl.formats = None;
    gui.ytdl.list.menu = None;
    let draft = draft_mut(gui, form);
    draft.tags_open = true;
    draft.keys = Keys::Tag(field);
    super::ytdl_here::keys_taken(gui);
}

/// The sheet closes: what was typed stays, the keys go back to the form.
pub(super) fn close_tags(gui: &mut Gui, form: Form) {
    let draft = draft_mut(gui, form);
    draft.tags_open = false;
    if matches!(draft.keys, Keys::Tag(_)) {
        draft.keys = Keys::None;
    }
}

/// Handle Add from YouTube's acts. Returns true when the act was its own.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Yt(act) = act else { return false };
    let now = gui.ytdl.now();
    // The form's own controls act on the form on screen: the room's, or
    // the Files window's over Files.
    let form = active_form(gui);
    // Its press out, they wait for the server with it — the nav is the way
    // on.
    let waiting = draft(gui, form).starting.is_some() && (form == Form::Window || in_room(gui));
    let controls = matches!(
        act,
        YtAct::Edit
            | YtAct::Type
            | YtAct::Use
            | YtAct::Undo
            | YtAct::Retry
            | YtAct::Instead
            | YtAct::Change
            | YtAct::Choose
            | YtAct::Format
            | YtAct::PasteClip
    );
    if waiting && controls {
        return true;
    }
    match act.clone() {
        // The Files door's `! failed`: the room, where the row is.
        YtAct::List => {
            if available(gui) {
                gui.act(Act::Nav(super::YOUTUBE_NAV));
            }
        }
        YtAct::Field => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() {
                draft.keys = Keys::Link;
            }
            gui.ytdl.list.view.stow();
            super::ytdl_here::keys_taken(gui);
        }
        YtAct::Clear => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() {
                draft.link = Input::default();
                draft.keys = Keys::Link;
                draft.link_changed(now);
            }
            gui.ytdl.list.view.stow();
            super::ytdl_here::keys_taken(gui);
        }
        // The clipboard, where this build reads it: a paste like any other.
        YtAct::PasteClip => {
            if let Some(text) = gui.ytdl.clipboard.and_then(|read| read()) {
                paste(gui, &text);
            }
        }
        YtAct::Tag(field) => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() && draft.tags_open {
                draft.keys = Keys::Tag(field);
            }
        }
        YtAct::Edit => open_tags(gui, form, TagField::Title),
        YtAct::Type => {
            draft_mut(gui, form).manual = true;
            open_tags(gui, form, TagField::Title);
        }
        YtAct::TagsDone => close_tags(gui, form),
        YtAct::Use => {
            let draft = draft_mut(gui, form);
            if let Some(meta) = draft.late.take() {
                let url = draft.parsed.url.clone().unwrap_or_default();
                draft.guess = read_guess(&meta);
                draft.gstate = GuessState::default();
                draft.fill(true);
                draft.look = Look::Found { url, meta };
            }
        }
        YtAct::Undo => {
            let draft = draft_mut(gui, form);
            draft.gstate = GuessState { restored: (0..draft.guess.fragments.len()).collect(), ..GuessState::default() };
            draft.fill(false);
        }
        YtAct::Retry => ask(gui),
        YtAct::Instead => {
            let target = form_target(gui, form);
            let deepest = folder_check(gui, form, &target).and_then(|c| c.deepest.clone());
            if let Some(dir) = deepest {
                // A folder chosen, not typed: checked at once.
                let draft = draft_mut(gui, form);
                draft.instead = Some(dir);
                draft.folder = None;
            }
        }
        // Saves to over the room; Choose a folder in the place of what it
        // was opened from.
        YtAct::Change => super::ytdl_where::open_where(gui),
        YtAct::Choose => super::ytdl_where::open_choose(gui, form),
        YtAct::Format => {
            gui.ytdl.list.menu = None;
            open_formats(gui, None);
        }
        YtAct::FormatPick(codec) => {
            let at = formats_offered(gui).iter().position(|c| *c == codec.as_str());
            if let (Some(at), Some(sheet)) = (at, gui.ytdl.formats.as_mut()) {
                sheet.cursor = at;
            }
        }
        YtAct::FormatDone => take_format(gui),
        YtAct::FormatClose => gui.ytdl.formats = None,
        YtAct::Add => press(gui),
        YtAct::Play(i) => {
            gui.ytdl.list.menu = None;
            play_song(gui, i, true);
        }
        YtAct::Queue(i) => {
            gui.ytdl.list.menu = None;
            play_song(gui, i, false);
        }
        YtAct::More(i) => gui.ytdl.list.menu = Some((i, None)),
        YtAct::Way(id, w) => way(gui, id, w),
        YtAct::MenuPick(k) => {
            let Some((i, _)) = gui.ytdl.list.menu.take() else { return true };
            if k == 0 {
                play_song(gui, i, false);
            } else {
                show_in_folder(gui, i);
            }
        }
        YtAct::MenuClose => gui.ytdl.list.menu = None,
        YtAct::ShowEarlier => {
            if let Some(jobs) = gui.ytdl.jobs_mut() {
                jobs.show_earlier();
            }
        }
        // The draw clamps it to what the list holds.
        YtAct::ScrollBy(delta) => {
            let list = &mut gui.ytdl.list;
            list.view.scroll = list.view.scroll.saturating_add_signed(delta as isize);
        }
        YtAct::Here(act) => super::ytdl_here::act(gui, act),
        YtAct::Place(act) => super::ytdl_where::act(gui, act),
    }
    true
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The letters the room's controls use: their capitals are the room's
/// too (Shift or Caps Lock on `d`, `t`, `a` would open Auto DJ, Stats, or
/// toggle the DJ).
const ROOM_LETTERS: &[char] = &['a', 'c', 'd', 'e', 'f', 'i', 'm', 't', 'u', 'x'];

/// The room's keys, and its windows' (which outrank everything over the
/// room or Files): Saves to or Choose a folder, the Edit tags sheet, the
/// Format sheet, a song's list, the Files window. `Some(quit)` when the
/// key was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !surface_up(gui) {
        return None;
    }
    if super::ytdl_where::is_open(gui) {
        return Some(super::ytdl_where::handle_key(gui, key));
    }
    if formats_up(gui) {
        super::ytdl_sheets::formats_key(gui, key);
        return Some(false);
    }
    if tags_up(gui) {
        super::ytdl_sheets::tags_key(gui, key);
        return Some(false);
    }
    if menu_up(gui) {
        return Some(menu_key(gui, key));
    }
    if super::ytdl_here::window_up(gui) {
        return Some(super::ytdl_here::handle_key(gui, key));
    }
    if !in_room(gui) {
        return None;
    }
    // The queue panel holds the keys: the room yields them.
    if gui.queue_open && gui.app.focus == Focus::Queue {
        return None;
    }
    let ctrl = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let now = gui.ytdl.now();
    // The press is out: the form's keys wait for the server — a link
    // pasted as keys, where the terminal sends one so, among them: its
    // letters and digits must never reach the shell's globals. The list
    // under the form keeps its own.
    let pressed = gui.ytdl.draft.starting.is_some();
    match gui.ytdl.draft.keys {
        Keys::Link if !pressed => match key.code {
            KeyCode::Esc => gui.ytdl.draft.keys = Keys::None,
            KeyCode::Enter => link_enter(gui, Form::Room),
            KeyCode::Tab | KeyCode::BackTab => return None,
            // ↓ leaves the field for the list under the form.
            KeyCode::Down if !gui.ytdl.list.items.is_empty() => {
                gui.ytdl.draft.keys = Keys::None;
                gui.ytdl.list.held = Some(0);
                gui.ytdl.list.view.pick_up();
            }
            KeyCode::Up | KeyCode::Down => {}
            _ => link_typed(gui, Form::Room, key, now),
        },
        // A nav room has no way back for Esc to take: with the list's
        // hand down it is the shell's (an armed pick's, else nothing).
        _ => {
            if let Some(quit) = list_key(gui, key) {
                return Some(quit);
            }
            match key.code {
                KeyCode::Enter if !pressed => {
                    if room_view(gui).primary.is_some_and(|(_, on)| on) {
                        press(gui);
                    } else if gui.ytdl.draft.parsed.kind == LinkKind::Video && matches!(gui.ytdl.draft.look, Look::Idle) {
                        ask(gui);
                    }
                }
                KeyCode::Enter | KeyCode::Up | KeyCode::Down => {}
                KeyCode::Char(c) if !ctrl => {
                    let lower = c.to_ascii_lowercase();
                    let drawn = gui.ytdl.drawn.iter().find(|(letter, _)| *letter == lower).map(|(_, act)| act.clone());
                    match drawn {
                        Some(act) if !pressed => return Some(gui.act(yt(act))),
                        // The press out, every typed key waits with it.
                        _ if pressed => {}
                        // A capital of the room's own letters is the room's,
                        // drawn or not: it never reaches the shell's globals.
                        None if c.is_ascii_uppercase() && ROOM_LETTERS.contains(&lower) => {}
                        _ => return None,
                    }
                }
                _ => return None,
            }
        }
    }
    Some(false)
}

/// Enter in a form's link field: the primary once a song is found, else
/// the typed link looked up (the keys go back meanwhile).
pub(super) fn link_enter(gui: &mut Gui, form: Form) {
    let draft = draft(gui, form);
    if draft.found() {
        press(gui);
    } else if draft.parsed.kind == LinkKind::Video {
        draft_mut(gui, form).keys = Keys::None;
        ask(gui);
    }
}

/// A key the link field's line editor takes; a changed link starts the
/// form over.
pub(super) fn link_typed(gui: &mut Gui, form: Form, key: KeyEvent, now: i64) {
    let draft = draft_mut(gui, form);
    let changed = draft.link.handle_event(&TermEvent::Key(key)).is_some_and(|c| c.value);
    if changed {
        draft.link_changed(now);
    }
}

/// A key while a tag has the keys: Tab and ↓ to the next tag, Shift-Tab
/// and ↑ to the one before, the rest its line editor's.
pub(super) fn tag_typed(gui: &mut Gui, form: Form, field: TagField, key: KeyEvent) {
    let i = tag_index(field);
    let draft = draft_mut(gui, form);
    match key.code {
        KeyCode::Up | KeyCode::BackTab => draft.keys = Keys::Tag(TAGS[i.saturating_sub(1)]),
        KeyCode::Down | KeyCode::Tab => draft.keys = Keys::Tag(TAGS[(i + 1).min(TAGS.len() - 1)]),
        _ => {
            let changed = draft.fields[i].handle_event(&TermEvent::Key(key)).is_some_and(|c| c.value);
            if changed {
                draft.edited[i] = true;
                draft.instead = None;
            }
        }
    }
}

/// A song's list's keys: ↑ ↓ walk it, Enter picks, `a` adds to the
/// queue, Esc closes it.
fn menu_key(gui: &mut Gui, key: KeyEvent) -> bool {
    let Some((item, cursor)) = gui.ytdl.list.menu else { return false };
    match (key.code, cursor) {
        // The first walking key shows the cursor, on the first choice.
        (KeyCode::Up | KeyCode::Down, None) => gui.ytdl.list.menu = Some((item, Some(0))),
        (KeyCode::Up, Some(at)) => gui.ytdl.list.menu = Some((item, Some(at.saturating_sub(1)))),
        (KeyCode::Down, Some(at)) => gui.ytdl.list.menu = Some((item, Some((at + 1).min(1)))),
        (KeyCode::Enter, Some(at)) => return gui.act(yt(YtAct::MenuPick(at))),
        (KeyCode::Char('a' | 'A'), _) => return gui.act(yt(YtAct::MenuPick(0))),
        (KeyCode::Esc, _) => gui.ytdl.list.menu = None,
        _ => {}
    }
    false
}

/// The list's keys, the form's free: ↑ ↓ pick its cursor up, then walk
/// it — ↑ from the first row hands the keys back to the link; Enter is
/// the held row's first way (a song's: play), `x` dismisses, `a` and `m`
/// a song's queue and list, Esc puts the hand down. `None`: the key is
/// not the list's.
fn list_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    let pressed = gui.ytdl.draft.starting.is_some();
    let list = &mut gui.ytdl.list;
    let len = list.items.len();
    if !list.view.held {
        // The first walking key shows the cursor; the next ones move it.
        if matches!(key.code, KeyCode::Down | KeyCode::Up) && len > 0 {
            list.held = Some(list.held.unwrap_or(0).min(len - 1));
            list.view.pick_up();
            return Some(false);
        }
        return None;
    }
    let held = list.held.unwrap_or(0).min(len.saturating_sub(1));
    match key.code {
        KeyCode::Esc => {
            list.view.stow();
            return Some(false);
        }
        KeyCode::Down => {
            list.held = Some((held + 1).min(len.saturating_sub(1)));
            list.view.pick_up();
            return Some(false);
        }
        KeyCode::Up if held == 0 => {
            list.view.stow();
            if !pressed {
                gui.ytdl.draft.keys = Keys::Link;
            }
            return Some(false);
        }
        KeyCode::Up => {
            list.held = Some(held - 1);
            list.view.pick_up();
            return Some(false);
        }
        _ => {}
    }
    let item = list.items.get(held).cloned();
    match (key.code, item) {
        (KeyCode::Enter, Some(Item::Song(_))) => Some(gui.act(yt(YtAct::Play(held)))),
        (KeyCode::Char('a' | 'A'), Some(Item::Song(_))) => Some(gui.act(yt(YtAct::Queue(held)))),
        (KeyCode::Char('m' | 'M'), Some(Item::Song(_))) => {
            gui.ytdl.list.menu = Some((held, Some(0)));
            Some(false)
        }
        (KeyCode::Enter, Some(Item::Job(id))) => {
            let first = gui.ytdl.jobs().and_then(|j| j.job(&id)).and_then(|job| {
                let formats = formats_offered(gui);
                job.ways(Some(&formats)).into_iter().next()
            });
            Some(match first {
                Some(w) => gui.act(yt(YtAct::Way(id, w))),
                None => false,
            })
        }
        (KeyCode::Char('x' | 'X'), Some(Item::Job(id))) => {
            let settled = gui.ytdl.jobs().and_then(|j| j.job(&id)).is_some_and(|j| !j.state.open());
            Some(if settled { gui.act(yt(YtAct::Way(id, Way::Dismiss))) } else { false })
        }
        // A held row's Enter is never the primary's.
        (KeyCode::Enter, _) => Some(false),
        _ => None,
    }
}

// ── Paste ───────────────────────────────────────────────────────────────────

/// A paste as the link field takes it: the first non-blank line, controls
/// and the invisible format characters copied text carries left out, at
/// most [`PASTE_MAX`] characters.
pub(super) fn clean_paste(text: &str) -> String {
    let line = text.split(['\n', '\r', '\u{2028}', '\u{2029}']).map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control() && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}'))
        .take(PASTE_MAX)
        .collect()
}

/// A paste, whole: it replaces the link on screen (the room's, the Files
/// window's) and looks it up at once, whoever had the keys; once found the
/// field gives them back. Nothing wants one — a sheet, Saves to or Choose
/// a folder stands over the form — dropped.
pub(crate) fn paste(gui: &mut Gui, text: &str) {
    let form = active_form(gui);
    if !wants_paste(gui) || draft(gui, form).starting.is_some() {
        return;
    }
    let now = gui.ytdl.now();
    super::ytdl_here::keys_taken(gui);
    gui.ytdl.list.view.stow();
    let whole = gui.ytdl.whole_pastes;
    let draft = draft_mut(gui, form);
    draft.link = Input::new(clean_paste(text));
    draft.keys = Keys::Link;
    // The same link pasted again asks again.
    draft.parsed = parse_link("");
    draft.link_changed(now);
    if draft.parsed.kind == LinkKind::Video {
        if whole {
            draft.keys = Keys::None;
        }
        ask(gui);
    }
}

// ── Tips ────────────────────────────────────────────────────────────────────

/// The tips line for what is up: Saves to or Choose a folder, a sheet, a
/// song's list, the Files window, the room as its keys stand.
pub(crate) fn tips(gui: &Gui) -> String {
    if super::ytdl_where::is_open(gui) {
        return super::ytdl_where::tips(gui);
    }
    if formats_up(gui) {
        return t!("gui.tips.ytdl_formats").to_string();
    }
    if tags_up(gui) {
        return t!("gui.tips.ytdl_tags").to_string();
    }
    if menu_up(gui) {
        return t!("gui.tips.ytdl_menu").to_string();
    }
    if super::ytdl_here::window_up(gui) {
        return super::ytdl_here::tips(gui);
    }
    // The list's hand is up: the held row's keys.
    let list = &gui.ytdl.list;
    if let Some(item) = list.held.filter(|_| list.view.held).and_then(|i| list.items.get(i)) {
        let mut parts = vec![t!("gui.tips.ytdl_rows").to_string()];
        match item {
            Item::Song(_) => {
                parts.push(t!("gui.tips.ytdl_play").to_string());
                parts.push(t!("gui.tips.ytdl_queue").to_string());
                parts.push(t!("gui.tips.ytdl_more").to_string());
            }
            Item::Job(id) => {
                if let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)) {
                    let formats = formats_offered(gui);
                    if let Some(w) = job.ways(Some(&formats)).into_iter().next() {
                        parts.push(t!("gui.tips.ytdl_enter", verb = way_words(w)).to_string());
                    }
                    if !job.state.open() {
                        parts.push(t!("gui.tips.ytdl_dismiss").to_string());
                    }
                }
            }
        }
        parts.push(t!("gui.tips.ytdl_stow").to_string());
        return parts.join(" · ");
    }
    // The press is out: none of the form's keys does anything, so the line
    // names the list's alone.
    let rows = (!list.items.is_empty()).then(|| t!("gui.tips.ytdl_rows").to_string());
    if gui.ytdl.draft.starting.is_some() {
        return rows.unwrap_or_default();
    }
    match room_keys(gui) {
        Keys::Link => t!("gui.tips.ytdl_field").to_string(),
        Keys::Tag(_) => t!("gui.tips.ytdl_tags").to_string(),
        Keys::None => {
            // The controls the frame drew, top to bottom, by their key words.
            let mut keys: Vec<String> = Vec::new();
            for (letter, act) in &gui.ytdl.drawn {
                let words = match act {
                    YtAct::Edit => t!("gui.ytdl.key.edit"),
                    YtAct::Change => t!("gui.ytdl.key.change"),
                    YtAct::Format => t!("gui.tips.ytdl_format"),
                    YtAct::Retry => t!("gui.ytdl.key.retry"),
                    YtAct::Type => t!("gui.ytdl.key.type"),
                    YtAct::Use => t!("gui.ytdl.key.use"),
                    YtAct::Undo => t!("gui.ytdl.key.undo"),
                    YtAct::Choose => t!("gui.ytdl.key.choose"),
                    _ => continue,
                };
                if words.starts_with(*letter) && !keys.iter().any(|k| k.as_str() == words) {
                    keys.push(words.to_string());
                }
            }
            let enter = room_view(gui).primary.is_some_and(|(_, on)| on).then(|| t!("gui.tips.ytdl_enter_add").to_string());
            let line = |keys: &[String]| {
                std::iter::once(t!("gui.tips.ytdl_paste").to_string())
                    .chain(keys.iter().cloned())
                    .chain(enter.iter().cloned())
                    .chain(rows.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            // At most the footer's 99 cells: the last key words give way,
            // the paste and Enter stay.
            while !keys.is_empty() && width(&line(&keys)) > 99 {
                keys.pop();
            }
            line(&keys)
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::types::TranscodeInfo;
    use crate::api::ytdl::{YtdlEntry, YtdlStarted};
    use crate::config::Config;
    use crate::tui::app::{App, KnownServer};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use ratatui::style::Color;

    pub(crate) const HOST: &str = "http://host:3000";
    pub(crate) const LINK: &str = "https://youtu.be/FGBhQbmPwH8";
    pub(crate) const WATCH: &str = "https://www.youtube.com/watch?v=FGBhQbmPwH8";
    /// The test clock's start: 2026-11-03, midday UTC.
    pub(crate) const START: i64 = 1_793_707_200_000;

    pub(crate) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    pub(crate) fn ping(generation: u32) -> Ping {
        Ping {
            vpaths: vec!["Music".into(), "Mixes".into()],
            transcode: Some(TranscodeInfo::default()),
            no_upload: Some(false),
            supported_formats: Some(["mp3", "m4a", "opus", "flac"].map(String::from).to_vec()),
            ytdl: generation,
            ..Ping::default()
        }
    }

    pub(crate) fn connected(ping: Ping) -> Event {
        Event::Connected { server: HOST.into(), id: HOST.into(), username: None, token: None, ping: Box::new(ping) }
    }

    /// A Gui on "Home server" with the ping's facts as a Connected brings
    /// them, the clock in the test's hand, Settings up — the room not yet.
    pub(crate) fn seated(generation: u32) -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.queue_open = false;
        gui.app.connected = true;
        gui.app.libraries = vec!["Music".into(), "Mixes".into()];
        gui.app.session.server = HOST.into();
        gui.app.capabilities.uploads = true;
        gui.app.servers = vec![KnownServer {
            id: HOST.into(),
            name: "Home server".into(),
            token: None,
            username: None,
            self_signed: false,
            peer: None,
            pairing: None,
            dj: Default::default(),
        }];
        gui.ytdl.clock = YtClock(Some(Rc::new(Cell::new(START))));
        // The boards are drawn with whole pastes, as every platform but a
        // Windows console delivers them: pinned here, so they read the
        // same on Windows CI. The typed-paste path has its own test.
        gui.ytdl.whole_pastes = true;
        observe(&mut gui, &connected(ping(generation)));
        gui.active = super::super::SETTINGS_NAV;
        gui
    }

    /// The room open on the new server (generation 1), by its nav row.
    pub(crate) fn room() -> Gui {
        let mut gui = seated(1);
        gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        gui
    }

    pub(crate) fn advance(gui: &Gui, ms: i64) {
        let hand = gui.ytdl.clock.0.as_ref().expect("the test's clock");
        hand.set(hand.get() + ms);
    }

    pub(crate) fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub(crate) fn shift(gui: &mut Gui, c: char) {
        super::super::handle_key(gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT));
    }

    pub(crate) fn type_text(gui: &mut Gui, text: &str) {
        for c in text.chars() {
            key(gui, KeyCode::Char(c));
        }
    }

    pub(crate) fn buffer(gui: &mut Gui, w: u16, h: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|frame| super::super::render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    pub(crate) fn lines(buffer: &Buffer) -> Vec<String> {
        let area = *buffer.area();
        (0..area.height).map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    pub(crate) fn draw(gui: &mut Gui) -> Vec<String> {
        lines(&buffer(gui, 100, 30))
    }



    pub(crate) fn at(rows: &[String], text: &str) -> Position {
        for (y, row) in rows.iter().enumerate() {
            if let Some(i) = row.find(text) {
                return Position { x: row[..i].chars().count() as u16, y: y as u16 };
            }
        }
        panic!("{text:?} is not on screen:\n{}", rows.join("\n"));
    }



    /// The lookup that's out, answered.
    pub(crate) fn answer_lookup(gui: &mut Gui, result: Result<YtdlMetadata, YtdlCallError>) {
        let seq = gui
            .ytdl
            .work
            .iter()
            .rev()
            .find_map(|w| match w {
                Work::Lookup { seq, .. } => Some(*seq),
                _ => None,
            })
            .expect("a lookup out");
        gui.ytdl.work.retain(|w| !matches!(w, Work::Lookup { .. }));
        apply_reply(gui, Reply::Lookup { seq, result });
    }

    /// The folder check that's out, answered.
    pub(crate) fn answer_folders(gui: &mut Gui, check: FolderCheck) {
        poll(gui);
        let seq = gui
            .ytdl
            .work
            .iter()
            .rev()
            .find_map(|w| match w {
                Work::Folders { seq, .. } => Some(*seq),
                _ => None,
            })
            .expect("a folder check out");
        gui.ytdl.work.retain(|w| !matches!(w, Work::Folders { .. }));
        apply_reply(gui, Reply::Folders { seq, check });
    }

    pub(crate) fn one_more_time() -> YtdlMetadata {
        YtdlMetadata {
            title: Some("One More Time".into()),
            artist: Some("Daft Punk".into()),
            album: Some("Discovery".into()),
            year: Some("2001".into()),
            file_stem: Some("One_More_Time".into()),
            channel: Some("Daft Punk".into()),
            duration_sec: Some(320.0),
            ..YtdlMetadata::default()
        }
    }

    /// Daft Punk › Discovery is new under Music, which has Daft Punk.
    pub(crate) fn discovery_new() -> FolderCheck {
        FolderCheck { known: true, new_segments: vec![1], deepest: Some("/Music/Daft Punk/".into()), files: None }
    }

    /// Music has no Daft Punk yet.
    pub(crate) fn artist_new() -> FolderCheck {
        FolderCheck { known: true, new_segments: vec![0], deepest: Some("/Music/".into()), files: None }
    }

    /// A drawn row read by its cells: what stands from cell `x`, what ends
    /// at cell `end` (a glyph is a cell here — the boards' are all narrow).
    pub(crate) trait Cells {
        fn starts_with_at(&self, x: usize, text: &str) -> bool;
        fn ends_with_at(&self, end: usize, text: &str) -> bool;
    }

    impl Cells for String {
        fn starts_with_at(&self, x: usize, text: &str) -> bool {
            self.chars().skip(x).collect::<String>().starts_with(text)
        }
        fn ends_with_at(&self, end: usize, text: &str) -> bool {
            self.chars().take(end).collect::<String>().ends_with(text)
        }
    }

    /// The link pasted, its answer in, the folder checked: the boards' song.
    fn found(gui: &mut Gui, meta: YtdlMetadata, check: FolderCheck) {
        paste(gui, LINK);
        answer_lookup(gui, Ok(meta));
        answer_folders(gui, check);
    }


    /// The folder is there, holding `files`.
    pub(crate) fn there(directory: &str, files: &[&str]) -> FolderCheck {
        FolderCheck {
            known: true,
            new_segments: Vec::new(),
            deepest: Some(directory.into()),
            files: Some(files.iter().map(|f| f.to_string()).collect()),
        }
    }

    // ── Board checks ───────────────────────────────────────────────────────

    /// The cells from `(x, y)` against a board's rows — each row the board's
    /// to its last non-blank cell — and every token the board names on its
    /// non-blank cells: colour, background, weight.
    pub(crate) fn assert_frame(buffer: &Buffer, (x0, y0): (u16, u16), width: u16, rows: &str, runs: &[&str]) {
        let want: Vec<String> =
            rows.lines().map(str::trim_start).filter_map(|l| l.strip_prefix('|')).map(str::to_string).collect();
        let got: Vec<String> = (0..want.len() as u16)
            .map(|r| (0..width).map(|c| buffer[(x0 + c, y0 + r)].symbol()).collect::<String>().trim_end().to_string())
            .collect();
        assert_eq!(got, want, "the frame:\n|{}", got.join("\n|"));
        let mut wrong = Vec::new();
        for run in runs {
            let parts: Vec<&str> = run.split(':').collect();
            let row: u16 = parts[0].parse().unwrap();
            let (first, last) = parts[1].split_once('-').unwrap();
            let (first, last): (u16, u16) = (first.parse().unwrap(), last.parse().unwrap());
            let token = parts[2];
            let bold_too = parts.get(3) == Some(&"B");
            for c in first..=last {
                let cell = &buffer[(x0 + c, y0 + row)];
                if cell.symbol().trim().is_empty() {
                    continue;
                }
                let colour = match token {
                    "Text" => cell.fg == Color::Reset || cell.fg == th().text,
                    "Dim" => cell.fg == th().dim,
                    "Accent" => cell.fg == th().accent,
                    "Bright" => cell.fg == th().bright,
                    "Gold" => cell.fg == th().gold,
                    "Ok" => cell.fg == th().ok,
                    "Danger" => cell.fg == th().danger,
                    "OnAccent" => cell.fg == th().on_accent && cell.bg == th().accent,
                    other => panic!("token {other}"),
                };
                let weight = cell.modifier.contains(Modifier::BOLD) == bold_too;
                if !(colour && weight) {
                    wrong.push(format!("{run} at {c}: {:?} {:?} {:?}", cell.symbol(), cell.fg, cell.modifier));
                }
            }
        }
        assert!(wrong.is_empty(), "tokens off the board:\n{}", wrong.join("\n"));
    }

    /// A frame's cells written down as a board, with its tokens: how the
    /// boards the canvas did not draw (the Files window empty, the sheet
    /// its `Saves here` opens, Saves to as it opens) were put on record,
    /// once looked at — for a dump test to print.
    pub(crate) fn board_of(buffer: &Buffer, (x0, y0): (u16, u16), (w, h): (u16, u16), name: &str) -> String {
        let token = |cell: &ratatui::buffer::Cell| {
            let colour = match cell.fg {
                fg if fg == th().on_accent && cell.bg == th().accent => "OnAccent",
                fg if fg == Color::Reset || fg == th().text => "Text",
                fg if fg == th().dim => "Dim",
                fg if fg == th().accent => "Accent",
                fg if fg == th().bright => "Bright",
                fg if fg == th().gold => "Gold",
                fg if fg == th().ok => "Ok",
                fg if fg == th().danger => "Danger",
                _ => "Other",
            };
            if cell.modifier.contains(Modifier::BOLD) { format!("{colour}:B") } else { colour.to_string() }
        };
        let mut out = format!("        pub(super) const {name}: &str = r#\"\n");
        let mut runs: Vec<String> = Vec::new();
        for r in 0..h {
            let row: String = (0..w).map(|c| buffer[(x0 + c, y0 + r)].symbol()).collect();
            out += &format!("            |{}\n", row.trim_end());
            let mut run: Option<(u16, u16, String)> = None;
            for c in 0..w {
                let cell = &buffer[(x0 + c, y0 + r)];
                if cell.symbol().trim().is_empty() {
                    continue;
                }
                let t = token(cell);
                match &mut run {
                    Some((_, last, have)) if *have == t => *last = c,
                    _ => {
                        runs.extend(run.take().map(|(a, b, t)| format!("\"{r}:{a}-{b}:{t}\"")));
                        run = Some((c, c, t));
                    }
                }
            }
            runs.extend(run.map(|(a, b, t)| format!("\"{r}:{a}-{b}:{t}\"")));
        }
        out += &format!("        \"#;\n        pub(super) const {name}_RUNS: &[&str] = &[\n");
        for chunk in runs.chunks(6) {
            out += &format!("            {},\n", chunk.join(", "));
        }
        out + "        ];\n"
    }

    // ── Seating jobs ───────────────────────────────────────────────────────

    /// A job as `ytdl.json` keeps it.
    pub(crate) fn job(id: &str, state: &str, title: &str, directory: &str) -> Value {
        serde_json::json!({
            "id": id, "url": WATCH, "directory": directory, "codec": "mp3", "title": title,
            "fileName": "", "createdAt": START - 10_000, "sentAt": START - 10_000, "state": state, "seen": true,
        })
    }

    /// A song added two days ago, seen: the list has history, nothing new.
    pub(crate) fn history() -> Value {
        let mut j = job("h1", "added", "Digital Love", "/Music/Daft Punk/Discovery/");
        j["filePath"] = "Music/Daft Punk/Discovery/Digital_Love.mp3".into();
        j["endedAt"] = (START - 2 * 86_400_000).into();
        j
    }

    /// The tracker seated on these jobs, as a restart loads them.
    pub(crate) fn seed(gui: &mut Gui, jobs: Vec<Value>) {
        let wire = Wire::new();
        let slot = serde_json::json!({ "jobs": jobs, "retired": [] });
        let store = YtStore::Memory(Rc::new(RefCell::new(Some(slot))));
        let generation = gui.ytdl.facts.as_ref().map_or(1, |f| f.generation);
        let jobs = Tracker::new(generation, wire.clone(), gui.ytdl.clock.clone(), store);
        gui.ytdl.held = Some(Held { identity: HOST.into(), username: None, jobs, wire });
    }

    /// Answer the tracker's calls with `reply` until it asks nothing more
    /// it answers (the clock moving a poll beat between turns); a call it
    /// leaves unanswered stays out.
    pub(crate) fn serve(gui: &mut Gui, reply: &mut dyn FnMut(&Call) -> Option<Answer>) {
        for pass in 0..40 {
            if pass > 0 {
                advance(gui, 3_000);
            }
            poll(gui);
            let held = gui.ytdl.held.as_ref().expect("a tracker");
            let calls = held.wire.take_out();
            let tx = held.wire.sender();
            let mut left = Vec::new();
            let mut answered = false;
            for (id, call) in calls {
                match reply(&call) {
                    Some(answer) => {
                        tx.send((id, answer)).unwrap();
                        answered = true;
                    }
                    None => left.push((id, call)),
                }
            }
            held.wire.0.borrow_mut().out.extend(left);
            if !answered {
                poll(gui);
                return;
            }
        }
    }

    /// A new server (generation 1) taking the add: the folder made, the
    /// reads before the POST answered, the POST's id — then, once
    /// `listed`, its entry in the job list as `status`.
    pub(crate) fn new_server(listed: Option<(&'static str, Option<&'static str>)>) -> impl FnMut(&Call) -> Option<Answer> {
        let mut posted = false;
        move |call| match call {
            Call::Mkdir { .. } => Some(Answer::Mkdir(Ok(()))),
            Call::Records { .. } => Some(Answer::Records(Ok(Some(crate::api::ytdl::YtdlRecordPage {
                limit: 1,
                ..Default::default()
            })))),
            Call::Jobs if !posted => Some(Answer::Jobs(Ok(Vec::new()))),
            Call::Jobs => listed.map(|(status, error)| {
                Answer::Jobs(Ok(vec![YtdlEntry {
                    id: Some("srv1".into()),
                    url: WATCH.into(),
                    directory: "/Music/Daft Punk/Discovery/".into(),
                    output_codec: "mp3".into(),
                    status: status.into(),
                    start_time: Some(START),
                    end_time: Some(START + 60_000),
                    file_name: Some("One_More_Time.mp3".into()),
                    file_path: (status == "complete").then(|| "Music/Daft Punk/Discovery/One_More_Time.mp3".into()),
                    error: error.map(str::to_string),
                    ..Default::default()
                }]))
            }),
            Call::Add(_) => {
                posted = true;
                Some(Answer::Add(Ok(YtdlStarted {
                    message: Some("Download started".into()),
                    id: Some("srv1".into()),
                    file_name: Some("One_More_Time.mp3".into()),
                })))
            }
            Call::Added { .. } => Some(Answer::Added(Ok(Some(crate::api::ytdl::YtdlAddedRead {
                end: true,
                ..Default::default()
            })))),
            _ => None,
        }
    }

    pub(crate) fn calls(gui: &Gui) -> Vec<Call> {
        gui.ytdl.held.as_ref().map(|h| h.wire.0.borrow().out.iter().map(|(_, c)| c.clone()).collect()).unwrap_or_default()
    }

    // ── The canvas's room ──────────────────────────────────────────────────

    /// What the test's clipboard holds: the boards' link.
    fn clip() -> Option<String> {
        Some(LINK.to_string())
    }

    pub(crate) fn record(id: i64, title: &str, path: &str, at: i64, present: bool) -> crate::api::ytdl::YtdlRecord {
        let (vpath, rel) = path.split_once('/').unwrap();
        crate::api::ytdl::YtdlRecord {
            id,
            plugin: "ytdl".into(),
            vpath: vpath.into(),
            filepath: path.into(),
            relative_path: rel.into(),
            title: Some(title.into()),
            downloaded_at: Some(at),
            present,
            ..Default::default()
        }
    }

    /// The record's first page: these rows, and more before them.
    fn records(rows: Vec<crate::api::ytdl::YtdlRecord>) -> impl FnMut(&Call) -> Option<Answer> {
        move |call| match call {
            Call::Added { .. } => Some(Answer::Added(Ok(Some(crate::api::ytdl::YtdlAddedRead {
                rows: rows.clone(),
                cursor: Some(1),
                end: false,
                newest_id: rows.first().map(|r| r.id),
            })))),
            _ => None,
        }
    }

    /// The record read to its end: these rows, and none before them.
    fn all_added(rows: Vec<crate::api::ytdl::YtdlRecord>) -> impl FnMut(&Call) -> Option<Answer> {
        move |call| match call {
            Call::Added { .. } => Some(Answer::Added(Ok(Some(crate::api::ytdl::YtdlAddedRead {
                rows: rows.clone(),
                end: true,
                newest_id: rows.first().map(|r| r.id),
                ..Default::default()
            })))),
            _ => None,
        }
    }

    /// The record read, and nothing in it: the first time.
    fn nothing_added() -> impl FnMut(&Call) -> Option<Answer> {
        all_added(Vec::new())
    }

    const DAY: i64 = 86_400_000;

    /// The canvas's Added list: one an hour ago, the rest in October.
    pub(crate) fn canvas_songs() -> Vec<crate::api::ytdl::YtdlRecord> {
        let discovery = |name: &str| format!("Music/Daft Punk/Discovery/{name}.mp3");
        let homework = |name: &str| format!("Music/Daft Punk/Homework/{name}.mp3");
        vec![
            record(19, "Digital Love", &discovery("Digital_Love"), START - 3_600_000, true),
            record(18, "Harder, Better, Faster, Stronger", &discovery("Harder_Better_Faster_Stronger"), START - 3 * DAY, true),
            record(17, "Aerodynamic", "Mixes/Aerodynamic.opus", START - 6 * DAY, true),
            record(16, "Around the World", &homework("Around_the_World"), START - 6 * DAY - 1, true),
            record(15, "Da Funk", &homework("Da_Funk"), START - 7 * DAY, true),
            record(14, "Veridis Quo", &discovery("Veridis_Quo"), START - 7 * DAY - 1, true),
            record(13, "Something About Us", &discovery("Something_About_Us"), START - 9 * DAY, true),
            record(12, "Face to Face", &discovery("Face_to_Face"), START - 9 * DAY - 1, true),
            record(11, "Voyager", &discovery("Voyager"), START - 9 * DAY - 2, true),
        ]
    }

    /// The room as the canvas drew it: the queue open (47 cells), `Paste`
    /// beside the link, the tracker seated on `jobs` and — given `songs` —
    /// the record's first page read.
    pub(crate) fn canvas(jobs: Vec<Value>, songs: Option<Vec<crate::api::ytdl::YtdlRecord>>) -> Gui {
        let mut gui = room();
        gui.queue_open = true;
        gui.ytdl.clipboard = Some(clip);
        gui.ytdl.zone = Some(None);
        seed(&mut gui, jobs);
        if let Some(jobs) = gui.ytdl.jobs_mut() {
            jobs.opened();
        }
        if let Some(songs) = songs {
            serve(&mut gui, &mut records(songs));
        }
        gui
    }

    /// The canvas's room with its song found.
    fn main_room() -> Gui {
        let mut gui = canvas(vec![], Some(canvas_songs()));
        found(&mut gui, one_more_time(), discovery_new());
        gui
    }

    /// The room's cells at this size against a board, from the block's
    /// first cell.
    fn assert_board(gui: &mut Gui, (w, h): (u16, u16), x: u16, width: u16, rows: &str, runs: &[&str]) -> Vec<String> {
        let b = buffer(gui, w, h);
        assert_frame(&b, (x, 2), width, rows, runs);
        lines(&b)
    }

    /// A job that failed in the server's own words.
    fn failed_job(id: &str, title: &str) -> Value {
        let mut failed = job(id, "failed", title, "/Music/Daft Punk/Homework/");
        failed["failure"] = serde_json::json!({
            "problem": "other", "phase": "download",
            "detail": "Sign in to confirm you’re not a bot.", "text": "Sign in to confirm you’re not a bot.",
        });
        failed
    }

    /// One adding for a minute and a half, one failed in the server's words.
    fn after_jobs() -> Vec<Value> {
        let mut adding = job("j1", "adding", "One More Time", "/Music/Daft Punk/Discovery/");
        adding["serverId"] = "srv1".into();
        adding["sentAt"] = (START - 90_000).into();
        vec![adding, failed_job("j3", "Robot Rock")]
    }

    // ── The nav ────────────────────────────────────────────────────────────

    /// The nav's TOOLS group ends with Add torrent and Add YouTube, after
    /// Discover — here, with no sonic path and no Discover, packed under
    /// Auto DJ on 16 and 17; the row brightens under the pointer and opens the
    /// room with the link field holding the keys. Settings keeps no doorway:
    /// TORRENTS holds the torrent switch alone.
    #[test]
    fn the_nav_row_opens_the_room_and_settings_keep_no_doorway() {
        let _en = english();
        let mut gui = seated(1);
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        let nav = |rows: &[String], y: usize| rows[y].chars().take(15).collect::<String>();
        assert_eq!((nav(&rows, 15).trim(), nav(&rows, 16).trim(), nav(&rows, 17).trim()), ("Auto DJ", "Add torrent", "Add YouTube"), "{text}");
        assert!(!text.contains("ADD MUSIC") && !text.contains("Add from YouTube"), "no doorway in Settings:\n{text}");
        assert!(rows[12].contains("TORRENTS") && rows[13].contains("[✓] Ask what to do with torrents"), "{text}");
        assert!(rows[15].contains("DISPLAY") && rows[16].contains("Keyboard hints"), "{text}");
        gui.ui.motion(Position { x: 5, y: 17 });
        let b = buffer(&mut gui, 100, 30);
        assert!(b[(5, 17)].modifier.contains(Modifier::BOLD) && b[(5, 17)].fg == th().bright, "{:?}", b[(5, 17)]);
        let row = gui.ui.hit(Position { x: 3, y: 17 }).expect("the row is a click target");
        assert_eq!(row, Act::Nav(super::super::YOUTUBE_NAV));

        gui.act(row);
        assert!(in_room(&gui) && gui.active == super::super::YOUTUBE_NAV);
        assert_eq!(gui.ytdl.draft.keys, Keys::Link, "the field holds the keys");
        assert_eq!(gui.cursor, None);
        let rows = draw(&mut gui);
        assert!(rows[17].starts_with(" ▸ Add YouTube"), "the row lit:\n{}", rows[17]);
        // The queue folded, the block is 56 cells in the room's 80.
        assert!(rows[2].chars().skip(29).collect::<String>().starts_with("Add from YouTube"), "centred:\n{}", rows[2]);
        assert_eq!(gui.ui.hit(Position { x: 29, y: 2 }), None, "no way back: the title is only a title");
    }

    /// Hidden, never greyed: on a peer, with uploads off, with no audio
    /// conversion, with no library, Add YouTube's row is absent — Add
    /// torrent closing TOOLS, nothing under it — and there is no way in; a room
    /// whose gate closes under it yields to Files, the Discover room's way,
    /// its sheets and windows going with it.
    #[test]
    fn the_nav_row_is_absent_under_each_gate() {
        let _en = english();
        type Close = fn(&mut Gui);
        let gates: [(&str, Close); 5] = [
            ("a peer", |g| g.app.session.peer = Some((HOST.into(), 4))),
            ("uploads off", |g| observe(g, &connected(Ping { no_upload: Some(true), ..ping(1) }))),
            ("an older server's silence on uploads", |g| observe(g, &connected(Ping { no_upload: None, ..ping(1) }))),
            ("no audio conversion", |g| observe(g, &connected(Ping { transcode: None, ..ping(1) }))),
            ("no library", |g| g.app.libraries.clear()),
        ];
        for (why, close) in gates {
            let mut gui = seated(1);
            close(&mut gui);
            let rows = draw(&mut gui);
            let text = rows.join("\n");
            let nav = |y: usize| rows[y].chars().take(15).collect::<String>();
            assert!(!text.contains("Add YouTube"), "{why}: absent\n{text}");
            assert_eq!((nav(16).trim(), nav(17).trim()), ("Add torrent", ""), "{why}: Add torrent closes TOOLS\n{text}");
            gui.act(Act::Nav(super::super::YOUTUBE_NAV));
            assert!(!in_room(&gui) && gui.active == super::super::SETTINGS_NAV, "{why}: no way in");

            let mut gui = room();
            draw(&mut gui);
            close(&mut gui);
            poll(&mut gui);
            assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::FILES_NAV), "{why}: the room yields to Files");

            // What hung over the room goes with it, none of it left over
            // Files: a row's Format sheet and the song's own, Edit tags,
            // Saves to, and a failed row's Choose a folder.
            for over in ["a row's Format sheet", "the song's Format sheet", "Edit tags", "Saves to", "Choose a folder"] {
                let mut gui = room();
                seed(&mut gui, vec![failed_job("j3", "Robot Rock")]);
                found(&mut gui, one_more_time(), discovery_new());
                draw(&mut gui);
                match over {
                    "a row's Format sheet" => {
                        gui.act(yt(YtAct::Way("j3".into(), Way::ChangeFormat)));
                    }
                    "the song's Format sheet" => {
                        gui.act(yt(YtAct::Format));
                    }
                    "Edit tags" => {
                        gui.act(yt(YtAct::Edit));
                    }
                    "Saves to" => {
                        gui.act(yt(YtAct::Change));
                    }
                    _ => {
                        gui.act(yt(YtAct::Way("j3".into(), Way::ChooseFolder)));
                    }
                }
                assert!(modal_open(&gui), "{why}: {over} up");
                close(&mut gui);
                poll(&mut gui);
                assert_eq!(gui.active, super::super::FILES_NAV, "{why}: {over}");
                assert!(gui.ytdl.formats.is_none() && gui.ytdl.place.is_none() && !modal_open(&gui), "{why}: {over} let go with the room");
                let text = draw(&mut gui).join("\n");
                for title in [" Format ", " Edit tags ", " Saves to ", "Choose a folder"] {
                    assert!(!text.contains(title), "{why}: {over}: nothing over Files\n{text}");
                }
            }
        }
        // A dropped connection closes nothing: the room says it can't reach
        // the server.
        let mut gui = room();
        gui.app.connected = false;
        poll(&mut gui);
        assert!(in_room(&gui));
    }

    // ── The room, board by board ───────────────────────────────────────────

    /// Main: the link in its card with `Paste` beside it, the song's card
    /// (its cover's slot, the title and `edit tags ▸`, the artist, the
    /// album and year, YouTube's own line), where the tags came from, the
    /// two rows, the tall primary the block's width, and the list under it
    /// — every control a click, the tips naming their keys.
    #[test]
    fn a_found_song_is_the_main_board() {
        let _en = english();
        let mut gui = main_room();
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "found: the field gave the keys back");
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::MAIN, boards::MAIN_RUNS);
        for (text, act) in [
            ("[X]", YtAct::Clear),
            ("Paste", YtAct::PasteClip),
            ("edit tags ▸", YtAct::Edit),
            ("Saves to", YtAct::Change),
            ("Discovery new", YtAct::Change),
            ("Format", YtAct::Format),
            ("MP3 ▸", YtAct::Format),
            ("Add to library ▸", YtAct::Add),
            ("Digital Love", YtAct::Play(0)),
            ("Aerodynamic", YtAct::Play(2)),
        ] {
            assert_eq!(gui.ui.hit(at(&rows, text)), Some(yt(act)), "{text}");
        }
        assert_eq!(gui.ui.hit(Position { x: 17, y: 2 }), None, "no way back: the title is only a title");
        assert_eq!(gui.ui.hit(Position { x: 30, y: 5 }), Some(yt(YtAct::Field)));
        assert_eq!(gui.ui.hit(Position { x: 30, y: 18 }), Some(yt(YtAct::Add)), "the whole frame presses");
        // The tips line names the controls' keys, top to bottom — no Esc: a
        // nav room has nothing to go back to.
        assert_eq!(tips(&gui), "a paste replaces the link · e edit tags · d change · f format · Enter add · ↑ ↓ rows");
        gui.ytdl.draft.keys = Keys::Link;
        assert_eq!(tips(&gui), "type or paste a link · Enter look it up · Esc give the keys back");
    }

    /// Min and Wide: at the 100 × 24 floor the primary is a slab on the
    /// title row, the blank under the title and the one over the two rows
    /// give way, and the list keeps three rows; in a wide window the block
    /// stops growing at 56 cells and sits in the middle of the room.
    #[test]
    fn the_floor_and_a_wide_window_are_the_boards() {
        let _en = english();
        let mut gui = main_room();
        let rows = assert_board(&mut gui, (100, 24), 17, 47, boards::MIN, boards::MIN_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "Add to library ▸")), Some(yt(YtAct::Add)), "the slab presses");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "and Enter with it");

        let mut gui = main_room();
        let rows = assert_board(&mut gui, (132, 34), 28, 56, boards::WIDE, boards::WIDE_RUNS);
        assert!(rows[2].starts_with_at(28, "Add from YouTube"), "the block starts at 28:\n{}", rows[2]);
        assert_eq!(room_block(Rect { x: 17, y: 2, width: 79, height: 27 }), Rect { x: 28, y: 2, width: 56, height: 27 });
        assert_eq!(room_block(Rect { x: 17, y: 2, width: 47, height: 23 }), Rect { x: 17, y: 2, width: 47, height: 23 });
    }

    /// Empty: the field has the keys (ACCENT, the caret, its placeholder),
    /// the limits under it, the rows saying what new songs go by, and the
    /// list down to the room's last row with `Show earlier` after it.
    /// First: nothing ever added — the list's two lines say what it is for.
    #[test]
    fn the_empty_room_is_the_board_and_the_first_time_says_so() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::EMPTY, boards::EMPTY_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "Show earlier")), Some(yt(YtAct::ShowEarlier)));
        assert_eq!(gui.ui.hit(at(&rows, "Voyager")), Some(yt(YtAct::Play(8))));
        assert_eq!(gui.ui.hit(at(&rows, "Music · Artist / Album")), Some(yt(YtAct::Change)), "the rows work before a link");
        assert_eq!(tips(&gui), "type or paste a link · Enter look it up · Esc give the keys back");

        let mut gui = canvas(vec![], None);
        serve(&mut gui, &mut nothing_added());
        assert_board(&mut gui, (100, 30), 17, 47, boards::FIRST, boards::FIRST_RUNS);
        // Until the record is read the list says nothing either way.
        let mut gui = canvas(vec![], None);
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("FROM YOUTUBE") && !text.contains("Paste a link to add a song"), "{text}");
    }

    /// After: the form started over for the next link, and the list
    /// follows the adds — one adding (ACCENT), one that failed in the
    /// server's words with its ways on, then the songs; the one under the
    /// pointer shows its verbs.
    #[test]
    fn after_an_add_the_list_follows_it_as_the_board() {
        let _en = english();
        let mut gui = canvas(after_jobs(), Some(canvas_songs()));
        gui.ui.motion(Position { x: 22, y: 19 });
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::AFTER, boards::AFTER_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "Try again")), Some(yt(YtAct::Way("j3".into(), Way::Retry))));
        assert_eq!(gui.ui.hit(at(&rows, "Dismiss")), Some(yt(YtAct::Way("j3".into(), Way::Dismiss))));
        assert_eq!(gui.ui.hit(Position { x: 54, y: 19 }), Some(yt(YtAct::Play(2))), "[▸]");
        assert_eq!(gui.ui.hit(Position { x: 58, y: 19 }), Some(yt(YtAct::Queue(2))), "[+]");
        assert_eq!(gui.ui.hit(Position { x: 62, y: 19 }), Some(yt(YtAct::More(2))), "[⋯]");
        assert_eq!(gui.ui.hit_context(at(&rows, "Digital Love")), Some(yt(YtAct::More(2))), "a right click is its list");
        assert_eq!(gui.ui.hit(at(&rows, "One More Time")), None, "an open add is no control");
        // The list is longer than the room: `Show earlier` comes after its
        // last song, under the wheel.
        assert!(!rows.join("\n").contains("Show earlier"));
        gui.wheel(Position { x: 30, y: 20 }, 40);
        let rows = draw(&mut gui);
        assert!(rows[23].contains("Voyager") && rows[24].ends_with_at(64, "Show earlier"), "{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(at(&rows, "Show earlier")), Some(yt(YtAct::ShowEarlier)));
        // The room is up: what landed is seen as it lands.
        assert!(gui.ytdl.jobs().unwrap().job("j3").unwrap().seen);
    }

    /// Reading: the cover's slot empty, the words ACCENT beside it — then,
    /// eight seconds on, "Still looking" and `type the details ▸` — the
    /// rows as they were and the primary there but off. Refused: a link
    /// that isn't YouTube's, in GOLD under the card, the field keeping the
    /// keys, nothing asked of the server.
    #[test]
    fn the_reading_and_a_refused_link_are_the_boards() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        paste(&mut gui, LINK);
        let rows = draw(&mut gui);
        assert!(rows[8].contains("Reading the video…") && !rows.join("\n").contains("Still looking"), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("type the details"), "not offered yet");
        advance(&gui, 8_000);
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::READING, boards::READING_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "type the details ▸")), Some(yt(YtAct::Type)));
        assert_eq!(gui.ui.hit(at(&rows, "Add to library")), None, "off: nothing to add yet");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none());

        // (The board's list is six songs, and no more before them.)
        let mut gui = canvas(vec![], None);
        serve(&mut gui, &mut all_added(canvas_songs()[..6].to_vec()));
        paste(&mut gui, "https://soundcloud.com/daft-punk/one-more-time");
        assert_board(&mut gui, (100, 30), 17, 47, boards::REFUSED, boards::REFUSED_RUNS);
        assert!(gui.ytdl.work.is_empty(), "nothing asked of the server");
        assert_eq!(gui.ytdl.draft.keys, Keys::Link, "a refused link keeps the keys");
        // A playlist reads the same way; [X] clears it, the keys staying.
        paste(&mut gui, "https://youtube.com/playlist?list=PLSdoVPM5WnndSQEXRz704yQkKwx76GvPV");
        let rows = draw(&mut gui);
        assert!(rows[7].contains("! That's a playlist."), "{}", rows.join("\n"));
        gui.act(yt(YtAct::Clear));
        let rows = draw(&mut gui);
        assert!(rows[5].contains("▏Paste a YouTube link") && rows[8].starts_with_at(17, "One video per link"), "{}", rows.join("\n"));
    }

    /// FailedLookup: a video the server couldn't read — its words in GOLD
    /// under the card, `type the details ▸`, the place the library alone,
    /// and Add on (the file gets the video's own). No answer in time, or
    /// no server: `try again` and no primary. No yt-dlp: the primary off.
    #[test]
    fn a_failed_lookup_is_the_board() {
        let _en = english();
        let bare_500 = YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None };
        let mut gui = canvas(vec![], Some(canvas_songs()));
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(bare_500));
        answer_folders(&mut gui, there("/Music/", &[]));
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::FAILED, boards::FAILED_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "type the details ▸")), Some(yt(YtAct::Type)));
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "the keys are the form's: t, d, f and Enter work");
        assert!(!gui.ytdl.draft.tags_open, "the sheet waits to be asked for");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "Add works without the details");

        let mut gui = canvas(vec![], Some(canvas_songs()));
        paste(&mut gui, LINK);
        let late = YtdlCallError::Unanswered { why: crate::api::ytdl::NoAnswer::TimedOut, detail: String::new() };
        answer_lookup(&mut gui, Err(late));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[7].starts_with_at(17, "! The server took too long — it may still be"), "{text}");
        let again = at(&rows, "try again");
        assert_eq!((again.x, gui.ui.hit(again)), (17, Some(yt(YtAct::Retry))), "{text}");
        assert!(!text.contains("Add to library"), "no primary:\n{text}");
        assert_eq!(tips(&gui), "a paste replaces the link · a try again · d change · f format · ↑ ↓ rows");
        key(&mut gui, KeyCode::Char('a'));
        assert!(matches!(gui.ytdl.draft.look, Look::Reading { .. }), "a: asked again");

        let mut gui = canvas(vec![], Some(canvas_songs()));
        paste(&mut gui, LINK);
        let missing = YtdlCallError::Answered {
            status: 500,
            words: Some("yt-dlp is not installed".into()),
            reason: None,
            file_name: None,
        };
        answer_lookup(&mut gui, Err(missing));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(text.contains("! This server can't download from YouTube yet."), "{text}");
        assert_eq!(gui.ui.hit(at(&rows, "Add to library")), None, "the primary is there, and off:\n{text}");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none());

        // No server at all, or no way to it: the same words, and `try again`.
        for why in [crate::api::ytdl::NoAnswer::Refused, crate::api::ytdl::NoAnswer::Offline] {
            let mut gui = canvas(vec![], Some(canvas_songs()));
            paste(&mut gui, LINK);
            answer_lookup(&mut gui, Err(YtdlCallError::Unanswered { why, detail: String::new() }));
            let text = draw(&mut gui).join("\n");
            assert!(text.contains("! Can't reach Home server.") && text.contains("try again"), "{why:?}:\n{text}");
        }
    }

    /// Cleaned: a title with upload noise in it — "Cleaned from “…”", its
    /// title cut inside the quotes, with `undo`, which puts it all back —
    /// and an artist that may be the channel's, in GOLD. Clash: the folder
    /// already has the file's name — GOLD under the rows, Add anyway.
    #[test]
    fn the_clean_up_and_a_clash_are_the_boards() {
        let _en = english();
        let cleaned = YtdlMetadata {
            title: Some("One More Time (Official Video)".into()),
            artist: Some("Daft Punk".into()),
            year: Some("2009".into()),
            channel: Some("DaftPunkVEVO".into()),
            duration_sec: Some(320.0),
            ..YtdlMetadata::default()
        };
        let mut gui = canvas(vec![], Some(canvas_songs()));
        found(&mut gui, cleaned, artist_new());
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::CLEANED, boards::CLEANED_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "undo")), Some(yt(YtAct::Undo)));
        key(&mut gui, KeyCode::Char('u'));
        assert_eq!(gui.ytdl.draft.tags().title, "One More Time (Official Video)", "u put it all back");

        let mut gui = canvas(vec![], Some(canvas_songs()));
        found(&mut gui, one_more_time(), there("/Music/Daft Punk/Discovery/", &["One_More_Time.mp3", "Aerodynamic.mp3"]));
        let rows = assert_board(&mut gui, (100, 30), 17, 47, boards::CLASH, boards::CLASH_RUNS);
        assert_eq!(gui.ui.hit(at(&rows, "Add anyway ▸")), Some(yt(YtAct::Add)));
        assert_eq!(tips(&gui), "a paste replaces the link · e edit tags · d change · f format · Enter add · ↑ ↓ rows");
        key(&mut gui, KeyCode::Char('f'));
        assert!(gui.ytdl.formats.is_some(), "f opens Format: another format is a way on");
    }

    /// In a terminal the player can't read the clipboard: no `Paste`, the
    /// link's card the block's whole width — a paste is the terminal's
    /// own. Where it can (the GUI's window), the button pastes: the
    /// clipboard's link in the field and looked up at once.
    #[test]
    fn paste_stands_only_where_the_clipboard_can_be_read() {
        let _en = english();
        let mut gui = main_room();
        gui.ytdl.clipboard = None;
        let rows = draw(&mut gui);
        assert!(!rows.join("\n").contains("Paste"), "{}", rows.join("\n"));
        assert_eq!(rows[4].chars().skip(17).take(47).collect::<String>(), format!("╭{}╮", "─".repeat(45)));
        assert_eq!(gui.ui.hit(Position { x: 59, y: 5 }), Some(yt(YtAct::Clear)), "[X] at the card's end");

        let mut gui = canvas(vec![], Some(canvas_songs()));
        let rows = draw(&mut gui);
        let frame = at(&rows, "│ Paste │");
        assert_eq!((frame.x, frame.y), (55, 5), "{}", rows.join("\n"));
        let button = Position { x: 57, y: 5 };
        gui.act(gui.ui.hit(button).expect("a click"));
        assert_eq!(gui.ytdl.draft.link.value(), LINK);
        assert!(matches!(&gui.ytdl.work[..], [Work::Lookup { url, .. }] if url == WATCH), "looked up at once");
        assert_eq!(gui.ytdl.draft.keys, Keys::None);
        // Under the pointer the frame and its word brighten.
        gui.ui.motion(button);
        let b = buffer(&mut gui, 100, 30);
        assert!(b[(55, 4)].fg == th().bright && b[(57, 5)].fg == th().bright && b[(57, 5)].modifier.contains(Modifier::BOLD));
        // An empty clipboard pastes nothing.
        let mut gui = canvas(vec![], None);
        gui.ytdl.clipboard = Some(|| None);
        gui.act(yt(YtAct::PasteClip));
        assert!(gui.ytdl.draft.link.value().is_empty() && gui.ytdl.work.is_empty());
    }

    // ── The cover ──────────────────────────────────────────────────────────

    /// The song's cover is YouTube's own thumbnail for the video's id,
    /// asked of YouTube's image host once the song shows and never by an
    /// address the server gave; until it is here — and where there is none
    /// — the slot stands empty.
    #[test]
    fn the_cover_is_the_videos_own_thumbnail() {
        let _en = english();
        assert_eq!(thumb_url("FGBhQbmPwH8").as_deref(), Some("https://i.ytimg.com/vi/FGBhQbmPwH8/mqdefault.jpg"));
        for bad in ["", "abc", "../../etc", "FGBhQbmPwH8?x=1", "FGBhQbmPwH8/..", "a b c d e f"] {
            assert_eq!(thumb_url(bad), None, "{bad:?} names no thumbnail");
        }
        let mut gui = canvas(vec![], None);
        paste(&mut gui, LINK);
        poll(&mut gui);
        assert!(gui.ytdl.thumb_asks.is_empty(), "nothing asked while the video is read");
        let mut gui = main_room();
        assert_eq!(gui.ytdl.thumb_asks, ["FGBhQbmPwH8"], "asked by the video's id, once the song shows");
        poll(&mut gui);
        draw(&mut gui);
        assert_eq!(gui.ytdl.thumb_asks.len(), 1, "once");
        let rows = draw(&mut gui);
        assert!(rows[8].starts_with_at(17, "╭──────╮ One More Time"), "the empty slot meanwhile:\n{}", rows.join("\n"));

        // A 16:9 frame: the cover is its middle square.
        let mut frame = image::RgbImage::from_pixel(320, 180, image::Rgb([200, 30, 30]));
        for x in 0..70 {
            for y in 0..180 {
                frame.put_pixel(x, y, image::Rgb([0, 0, 0]));
                frame.put_pixel(319 - x, y, image::Rgb([0, 0, 0]));
            }
        }
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(frame).write_to(&mut jpeg, image::ImageFormat::Jpeg).unwrap();
        let art = square_cover(&jpeg.into_inner()).expect("a cover");
        assert_eq!(art.width(), art.height(), "square");
        let corner = &art.rgb()[..3];
        assert!(corner[0] > 150 && corner[1] < 90, "the bars at the sides are cut away: {corner:?}");
        assert!(square_cover(b"not a picture").is_none());

        apply_reply(&mut gui, Reply::Thumb { id: "FGBhQbmPwH8".into(), art: Some(art) });
        let b = buffer(&mut gui, 100, 30);
        let cell = &b[(18, 9)];
        assert!(cell.symbol() != " " && cell.symbol() != "│", "the picture in the slot: {cell:?}");
        assert_eq!(b[(26, 8)].symbol(), "O", "the words where they were");
        // A sheet over it: the next frame draws it as cells that sit under text.
        key(&mut gui, KeyCode::Char('e'));
        draw(&mut gui);
        let b = buffer(&mut gui, 100, 30);
        assert_eq!(b[(17, 7)].symbol(), "╭", "the sheet over the cover's column");
        // No picture to be had: the slot stays, and it is not asked again.
        let mut gui = main_room();
        poll(&mut gui);
        apply_reply(&mut gui, Reply::Thumb { id: "FGBhQbmPwH8".into(), art: None });
        poll(&mut gui);
        assert_eq!(gui.ytdl.thumb_asks.len(), 1);
        assert!(draw(&mut gui)[8].starts_with_at(17, "╭──────╮"));
    }

    // ── Edit tags ──────────────────────────────────────────────────────────

    /// Tags: `edit tags ▸` (or `e`) opens the sheet — the four tags in
    /// bordered cards, the title's ACCENT with the caret, where its value
    /// came from and what a blank one does, `[ Done ]`. Tab and ↓ walk the
    /// tags, typing edits the form's own tags as it goes, Enter, Esc,
    /// `[ Done ]` and a click outside all only close it.
    #[test]
    fn edit_tags_is_the_sheet_on_the_board() {
        let _en = english();
        let mut gui = main_room();
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        assert!(tags_up(&gui) && modal_open(&gui));
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Title));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 7), 66, boards::TAGS, boards::TAGS_RUNS);
        let rows = lines(&b);
        assert_eq!(tips(&gui), "type · Tab ↑ ↓ tags · Enter done · Esc close");
        assert_eq!(gui.ui.hit(at(&rows, "[ Done ]")), Some(yt(YtAct::TagsDone)));
        assert_eq!(gui.ui.hit(at(&rows, "Discovery  ")), Some(yt(YtAct::Tag(TagField::Album))), "a card takes the keys");
        assert_eq!(gui.ui.hit(Position { x: 5, y: 5 }), Some(yt(YtAct::TagsDone)), "a click outside closes it");
        assert_eq!(gui.ui.hit(Position { x: 30, y: 18 }), Some(Act::Guard), "inside, nothing beneath is reached");
        assert!(!wants_paste(&gui), "a paste is the tag's now, typed");

        // Tab and ↓ walk on, Shift-Tab and ↑ back; a typed letter is the
        // tag's, never the room's key.
        key(&mut gui, KeyCode::Tab);
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Artist));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Year));
        key(&mut gui, KeyCode::Backspace);
        key(&mut gui, KeyCode::Backspace);
        let rows = draw(&mut gui);
        let why = at(&rows, "! A year is 4 digits, like 2001");
        assert_eq!((why.x, why.y), (19, 17), "said in the sheet, in GOLD:\n{}", rows.join("\n"));
        type_text(&mut gui, "01");
        key(&mut gui, KeyCode::BackTab);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Title));
        type_text(&mut gui, "d");
        assert_eq!(gui.ytdl.draft.tags().title, "One More Timed", "typed into the title");
        assert!(gui.ytdl.place.is_none(), "d was no `d change`");
        let rows = draw(&mut gui);
        assert!(rows[17].starts_with_at(19, "Edited"), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Esc);
        assert!(draw(&mut gui)[8].contains("One More Timed"), "the room's card has it");
        key(&mut gui, KeyCode::Char('e'));
        key(&mut gui, KeyCode::Backspace);
        key(&mut gui, KeyCode::Esc);

        // Every way out closes it and leaves the tags as typed.
        for way in ["Enter", "Esc", "Done", "outside"] {
            gui.act(yt(YtAct::Edit));
            type_text(&mut gui, "!");
            match way {
                "Enter" => key(&mut gui, KeyCode::Enter),
                "Esc" => key(&mut gui, KeyCode::Esc),
                _ => {
                    gui.act(yt(YtAct::TagsDone));
                }
            }
            assert!(!tags_up(&gui) && gui.ytdl.draft.keys == Keys::None, "{way}: closed, the keys the form's");
            assert!(gui.ytdl.draft.tags().title.ends_with('!'), "{way}: what was typed stays");
            assert!(gui.ytdl.draft.starting.is_none(), "{way}: Enter in the sheet adds nothing");
        }
        // Back in the room Enter is the primary's again.
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some());
    }

    /// Typed by hand: `type the details ▸` opens the sheet on an empty
    /// form; what is typed makes the card, and an answer that comes
    /// after is offered with `use them`, which fills only what was left
    /// empty.
    #[test]
    fn details_typed_by_hand_make_the_card_and_a_late_answer_is_offered() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        paste(&mut gui, LINK);
        advance(&gui, 8_000);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('t'));
        assert!(tags_up(&gui) && gui.ytdl.draft.manual);
        let rows = draw(&mut gui);
        assert!(rows[17].starts_with_at(19, "Blank: the file gets the video's own title."), "{}", rows.join("\n"));
        type_text(&mut gui, "One More Time");
        key(&mut gui, KeyCode::Tab);
        type_text(&mut gui, "Daft Punk");
        key(&mut gui, KeyCode::Enter);
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[8].starts_with_at(26, "One More Time") && rows[9].starts_with_at(26, "Daft Punk"), "the card:\n{text}");
        assert!(rows[10].starts_with_at(26, "No album") && rows[11].chars().skip(26).take(38).all(|c| c == ' '), "no line of YouTube's:\n{text}");
        assert!(text.contains("edit tags ▸") && text.contains("Add to library ▸"), "{text}");

        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, there("/Music/Daft Punk/", &[]));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[7].starts_with_at(17, "Details found for this video"), "{text}");
        let used = at(&rows, "use them");
        assert_eq!((used.x + 8, gui.ui.hit(used)), (64, Some(yt(YtAct::Use))), "at the row's end:\n{text}");
        key(&mut gui, KeyCode::Char('u'));
        let tags = gui.ytdl.draft.tags();
        assert_eq!(
            (tags.title.as_str(), tags.artist.as_str(), tags.album.as_str(), tags.year.as_str()),
            ("One More Time", "Daft Punk", "Discovery", "2001"),
            "only the empty tags filled"
        );
        assert!(draw(&mut gui)[11].contains("YouTube · Daft Punk · 5:20"), "and the video's own line with them");
    }

    // ── Format ─────────────────────────────────────────────────────────────

    /// Format: the row (or `f`) opens the sheet — the formats the server
    /// takes, the dot on the one in force, what each is good for, who
    /// remembers it, `[ Done ]`. The dot is the choice in the making: ↑ ↓
    /// or a click moves it, Enter or `[ Done ]` takes it, Esc leaves the
    /// format as it was.
    #[test]
    fn the_format_sheet_is_the_board() {
        let _en = english();
        let mut gui = seated(1);
        gui.app.servers[0].name = "music.home.arpa".into();
        let five = Some(["mp3", "m4a", "opus", "flac", "wav"].map(String::from).to_vec());
        observe(&mut gui, &connected(Ping { supported_formats: five, ..ping(1) }));
        gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        gui.queue_open = true;
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('f'));
        assert!(formats_up(&gui) && modal_open(&gui));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 10), 66, boards::FORMAT, boards::FORMAT_RUNS);
        let rows = lines(&b);
        assert_eq!(tips(&gui), "↑ ↓ pick · Enter choose · Esc close");
        assert_eq!(gui.ui.hit(at(&rows, "Opus")), Some(yt(YtAct::FormatPick("opus".into()))));
        assert_eq!(gui.ui.hit(at(&rows, "Biggest, no cover art")), Some(yt(YtAct::FormatPick("wav".into()))), "the whole row");
        assert_eq!(gui.ui.hit(at(&rows, "[ Done ]")), Some(yt(YtAct::FormatDone)));
        assert_eq!(gui.ui.hit(Position { x: 90, y: 5 }), Some(yt(YtAct::FormatClose)), "a click elsewhere closes it");

        // The dot moves; nothing is taken until Done.
        key(&mut gui, KeyCode::Down);
        let rows = draw(&mut gui);
        assert!(rows[12].contains("( ) MP3") && rows[13].contains("(•) M4A"), "{}", rows.join("\n"));
        assert!(rows[10].contains(" Format · MP3 "), "the title says what is in force:\n{}", rows.join("\n"));
        assert_eq!(gui.ytdl.draft.codec, None);
        key(&mut gui, KeyCode::Esc);
        assert!(!formats_up(&gui) && gui.ytdl.draft.codec.is_none(), "Esc: as it was");
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('f'));
        gui.act(yt(YtAct::FormatPick("opus".into())));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.ytdl.draft.codec.as_deref(), Some("opus"));
        assert!(!formats_up(&gui));
        let rows = draw(&mut gui);
        assert!(rows[15].ends_with_at(64, "Opus ▸"), "the row follows:\n{}", rows.join("\n"));
        // `f` again closes it, like Esc.
        key(&mut gui, KeyCode::Char('f'));
        assert!(formats_up(&gui));
        key(&mut gui, KeyCode::Char('f'));
        assert!(!formats_up(&gui));
    }

    // ── The press ──────────────────────────────────────────────────────────

    /// The press is out — `Adding…`, DIM, no click, no hover, the form's
    /// controls standing and doing nothing, its keys waiting; the server
    /// refused it — its words in GOLD over the primary, Enter trying again;
    /// already adding this link to this folder — said DIM, the primary off.
    #[test]
    fn the_press_waits_and_the_servers_answers_stand_over_the_primary() {
        let _en = english();
        let mut gui = main_room();
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "Enter pressed the primary");
        poll(&mut gui);
        assert!(
            calls(&gui).iter().any(|c| matches!(c, Call::Mkdir { directory } if directory == "/Music/Daft Punk/Discovery/")),
            "the new folder made first: {:?}",
            calls(&gui)
        );
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let adding = at(&rows, "Adding…");
        assert_eq!((adding.y, b[(adding.x, adding.y)].fg, b[(17, 17)].fg), (18, th().dim, th().dim), "the frame and its word DIM");
        assert_eq!(gui.ui.hit(adding), None, "no click");
        // Every control of the form stands, neither clicking nor lighting
        // under the pointer; the list under it keeps working.
        for text in ["edit tags ▸", "Saves to", "Format", "[X]", "Paste", "youtu.be"] {
            let spot = at(&rows, text);
            gui.ui.pointer = Some(spot);
            let b = buffer(&mut gui, 100, 30);
            assert_eq!(gui.ui.hit(spot), None, "{text}: no click");
            let cell = &b[(spot.x, spot.y)];
            assert!(!cell.modifier.contains(Modifier::BOLD) && cell.fg != th().bright, "{text}: no hover: {cell:?}");
        }
        gui.ui.pointer = None;
        let rows = draw(&mut gui);
        assert!(rows[22].starts_with_at(17, "◷ One More Time") && rows[22].ends_with_at(64, "starting…"), "its row, already:\n{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(at(&rows, "Digital Love")), Some(yt(YtAct::Play(1))), "the list is not the form's");
        assert_eq!(tips(&gui), "↑ ↓ rows", "the form's keys wait: the line names none of them");
        for act in [YtAct::Edit, YtAct::Change, YtAct::Format, YtAct::PasteClip] {
            gui.act(yt(act));
        }
        assert!(!gui.ytdl.draft.tags_open && gui.ytdl.place.is_none() && gui.ytdl.formats.is_none(), "their acts wait too");
        assert_eq!(gui.ytdl.draft.link.value(), LINK, "and Paste");
        key(&mut gui, KeyCode::Char('e'));
        assert!(!gui.ytdl.draft.tags_open, "keys wait");

        let mut ffmpeg = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Err(YtdlCallError::Answered {
                status: 500,
                words: Some("FFmpeg is not available yet".into()),
                reason: None,
                file_name: None,
            }))),
            Call::Added { .. } => None,
            other => new_server(None)(other),
        };
        serve(&mut gui, &mut ffmpeg);
        assert!(gui.ytdl.draft.starting.is_none());
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let refusal = at(&rows, "! The server can't convert audio right now. Try");
        assert_eq!((refusal.x, refusal.y, b[(refusal.x, refusal.y)].fg), (17, 16, th().gold), "{}", rows.join("\n"));
        assert!(rows[17].starts_with_at(17, "  again later, or ask its admin."), "{}", rows.join("\n"));
        assert!(rows[20].starts_with_at(17, "│               Add to library ▸"), "a blank, then the primary, on:\n{}", rows.join("\n"));
        assert!(gui.ytdl.work.iter().any(|w| matches!(w, Work::Ping { .. })), "a stale flag: pinged again");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "Enter tries again");

        let mut open = job("o1", "adding", "One More Time", "/Music/Daft Punk/Discovery/");
        open["hidden"] = true.into();
        open["serverId"] = "srv0".into();
        let mut gui = canvas(vec![open], Some(canvas_songs()));
        found(&mut gui, one_more_time(), there("/Music/Daft Punk/Discovery/", &[]));
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let why = at(&rows, "Already adding to this folder");
        assert_eq!((why.x + 29, why.y, b[(why.x, why.y)].fg), (64, 16, th().dim), "DIM, at the block's right end:\n{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(at(&rows, "Add to library")), None);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none(), "never twice for one link and folder");
    }

    /// The server took it: the form starts over with the field holding the
    /// keys for the next link, the job heads the list and the note says
    /// so; it landed, or it failed — the row and the note follow.
    #[test]
    fn after_the_press_the_form_starts_over_and_the_list_has_the_job() {
        let _en = english();
        for (id, row, note, note_runs, listed) in [
            ("adding", "↓ One More Time", boards::N1, boards::N1_RUNS, None),
            ("added", "✓ One More Time", boards::N2, boards::N2_RUNS, Some(("complete", None))),
            ("failed", "! One More Time", boards::N5, boards::N5_RUNS, Some(("error", Some("Sign in to confirm you’re not a bot.")))),
        ] {
            let mut gui = canvas(vec![], None);
            found(&mut gui, one_more_time(), discovery_new());
            key(&mut gui, KeyCode::Enter);
            serve(&mut gui, &mut new_server(listed));
            assert_eq!(gui.ytdl.draft.keys, Keys::Link, "{id}: the field has the keys again");
            assert!(gui.ytdl.draft.link.value().is_empty(), "{id}: for the next link");
            let b = buffer(&mut gui, 100, 30);
            let rows = lines(&b);
            let text = rows.join("\n");
            assert!(rows[5].starts_with_at(17, "│ ▏Paste a YouTube link"), "{id}:\n{text}");
            assert!(rows[15].starts_with_at(17, row), "{id}: its row heads the list:\n{text}");
            assert_frame(&b, (24, 29), 30, note, note_runs);
        }
    }

    /// An add that ends with no words of its own (not sure, not found —
    /// the list says) takes its "Adding …" off the note line; another's
    /// note since stays.
    #[test]
    fn an_adding_note_goes_when_its_job_ends_wordless() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(None));
        let adding = |gui: &Gui| gui.app.message.as_ref().is_some_and(|m| m.text.starts_with("Adding"));
        assert!(adding(&gui), "{:?}", gui.app.message);
        let id = gui.ytdl.jobs().unwrap().adding()[0].id.clone();
        take_event(&mut gui, JobEvent::Settled { job: id.clone(), state: JobState::NotSureFinished, note: None });
        assert!(gui.app.message.is_none(), "the note is down");

        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(None));
        let id = gui.ytdl.jobs().unwrap().adding()[0].id.clone();
        keep::set_note(&mut gui, ("Copied".into(), MessageKind::Success));
        take_event(&mut gui, JobEvent::Settled { job: id, state: JobState::NotFound, note: None });
        assert_eq!(gui.app.message.as_ref().map(|m| m.text.as_str()), Some("Copied"), "another's note stays");
    }

    /// Where the note has more than 30 cells the place joins it.
    #[test]
    fn the_place_joins_the_note_only_where_it_is_wider() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        buffer(&mut gui, 120, 30);
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(Some(("complete", None))));
        let b = buffer(&mut gui, 120, 30);
        assert_frame(&b, (24, 29), 50, boards::N4, boards::N4_RUNS);
    }

    /// The notes on the bar's row: Adding … ACCENT, Added … OK, Couldn't
    /// add … GOLD, clipped at 30 cells.
    #[test]
    fn the_notes_ride_the_bar_in_their_tokens() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(None));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (24, 29), 30, boards::N1, boards::N1_RUNS);
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(Some(("complete", None))));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (24, 29), 30, boards::N2, boards::N2_RUNS);
        let mut gui = room();
        let around = YtdlMetadata { title: Some("Around the World".into()), ..one_more_time() };
        found(&mut gui, around, discovery_new());
        key(&mut gui, KeyCode::Enter);
        serve(&mut gui, &mut new_server(Some(("error", Some("unavailable")))));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (24, 29), 30, boards::N3, boards::N3_RUNS);
    }

    // ── The list ───────────────────────────────────────────────────────────

    fn held_item(gui: &Gui) -> Option<Item> {
        let list = &gui.ytdl.list;
        list.held.filter(|_| list.view.held).and_then(|i| list.items.get(i).cloned())
    }

    /// The list's cursor is on a row, not a place: a row added again lists
    /// with the open ones, and the cursor goes with it — a second Enter
    /// never lands on the row that moved up under it.
    #[test]
    fn the_lists_cursor_follows_its_row() {
        let _en = english();
        let failed = |id: &str, title: &str| {
            let mut j = job(id, "failed", title, "/Music/Daft Punk/Homework/");
            j["failure"] = serde_json::json!({ "problem": "other", "phase": "download", "detail": "No.", "text": "No." });
            j
        };
        let mut gui = canvas(vec![failed("n1", "Da Funk"), failed("n2", "Revolution 909"), failed("n3", "Fresh")], None);
        gui.ytdl.draft.keys = Keys::None;
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        draw(&mut gui);
        let held = |gui: &Gui| match held_item(gui) {
            Some(Item::Job(id)) => id,
            other => panic!("a job held: {other:?}"),
        };
        let title = |gui: &Gui, id: &str| gui.ytdl.jobs().unwrap().job(id).unwrap().shown_title();
        let picked = held(&gui);
        let song = title(&gui, &picked);
        key(&mut gui, KeyCode::Enter);
        draw(&mut gui);
        let now = held(&gui);
        assert_ne!(now, picked, "added again: a new job");
        assert_eq!(title(&gui, &now), song, "the cursor on that song's row");
        let open = |gui: &Gui| gui.ytdl.jobs().unwrap().adding().len();
        assert_eq!(open(&gui), 1);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(open(&gui), 1, "Enter on the open row adds no other");
    }

    /// Every kind that needs a look, each with its ways on: the words
    /// under the title, the ways at the row's right end — sharing the
    /// words' last line where both fit, else on a row of their own.
    /// Change format and retry opens Format for the row; Choose a folder
    /// its window.
    #[test]
    fn every_kind_that_needs_a_look_has_its_ways_on() {
        let _en = english();
        let unsure = job("k1", "notSureStarted", "Digital Love", "/Music/Daft Punk/Discovery/");
        let mut landed = job("k2", "failed", "One More Time", "/Music/Daft Punk/Discovery/");
        landed["failure"] = serde_json::json!({ "problem": "exists", "phase": "download", "detail": "One_More_Time.mp3" });
        let gone = job("k3", "notFound", "Harder, Better, Faster, Stronger", "/Music/Daft Punk/Discovery/");
        let mut gui = canvas(vec![unsure, landed, gone], None);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        let row = |words: &str| at(&rows, words).y as usize;
        assert!(rows[row("? Digital Love")].ends_with_at(64, "not sure it started"), "{text}");
        assert!(rows[row("? Digital Love") + 1].ends_with_at(64, "Check again"), "its one way, on a row of its own:\n{text}");
        assert!(rows[row("! One More Time")].ends_with_at(64, "failed"), "{text}");
        assert!(rows[row("! One More Time") + 1].starts_with_at(19, "“One_More_Time.mp3” landed here meanwhile ·"), "{text}");
        assert!(rows[row("! Harder, Better, Faster, Stronger")].ends_with_at(64, "failed"), "a job's title has the row up to its word:\n{text}");
        assert!(rows[row("! Harder, Better, Faster, Stronger") + 1].starts_with_at(19, "not found on the server"), "{text}");
        assert!(rows[row("! Harder, Better, Faster, Stronger") + 1].ends_with_at(64, "Add again · Dismiss"), "sharing the words' line:\n{text}");
        assert_eq!(gui.ui.hit(at(&rows, "Check again")), Some(yt(YtAct::Way("k1".into(), Way::CheckAgain))));
        assert_eq!(gui.ui.hit(at(&rows, "Add again")), Some(yt(YtAct::Way("k3".into(), Way::AddAgain))));
        let ways = at(&rows, "Change format and retry");
        assert_eq!(b[(ways.x, ways.y)].fg, Color::Reset, "a way is TEXT: the room's one ACCENT is its primary");
        // Change format and retry: Format, for the row — its own title,
        // no one remembering; Enter adds again in the format taken.
        gui.act(gui.ui.hit(ways).unwrap());
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(formats_up(&gui) && text.contains(" Format · MP3 ") && !text.contains("Remembered for"), "{text}");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.formats.is_none());
        assert!(gui.ytdl.jobs().unwrap().jobs().iter().any(|j| j.codec == "m4a"), "added again in the format taken");
        assert_eq!(gui.ytdl.draft.codec, None, "the form's own format untouched");
    }

    /// A song under the pointer shows its verbs; `[⋯]` (or a right click,
    /// or `m`) hangs its list from it: Add to queue `a`, Show in folder —
    /// the room inert beneath, a click anywhere else closing it.
    #[test]
    fn a_song_under_the_pointer_shows_its_verbs_and_its_list() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        gui.ui.motion(Position { x: 22, y: 16 });
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        assert!(rows[16].starts_with_at(17, "✓ Harder, Better, Faste… Discovery  [▸] [+] [⋯]"), "{}", rows.join("\n"));
        assert!(b[(19, 16)].fg == th().bright && b[(19, 16)].modifier.contains(Modifier::BOLD), "its title lit");
        assert!(rows[15].ends_with_at(64, "1 h ago"), "the others say when:\n{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(Position { x: 54, y: 16 }), Some(yt(YtAct::Play(1))), "[▸]");
        assert_eq!(gui.ui.hit(Position { x: 58, y: 16 }), Some(yt(YtAct::Queue(1))), "[+]");
        assert_eq!(gui.ui.hit(Position { x: 62, y: 16 }), Some(yt(YtAct::More(1))), "[⋯]");

        gui.act(yt(YtAct::More(1)));
        assert!(modal_open(&gui));
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        assert!(rows[17].ends_with_at(64, "╭─────────────────╮") && rows[20].ends_with_at(64, "╰─────────────────╯"), "hung under the row, right edges level:\n{text}");
        assert!(rows[18].ends_with_at(64, "│ Add to queue  a │") && rows[19].ends_with_at(64, "│ Show in folder  │"), "{text}");
        assert!(rows[16].ends_with_at(64, "[▸] [+] [⋯]"), "the row keeps its verbs while its list is up:\n{text}");
        assert_eq!(gui.ui.hit(at(&rows, "Add to queue")), Some(yt(YtAct::MenuPick(0))));
        assert_eq!(gui.ui.hit(Position { x: 30, y: 5 }), Some(yt(YtAct::MenuClose)), "a click elsewhere closes it");
        assert_eq!(tips(&gui), "↑ ↓ walk · Enter picks · Esc close the list");
        gui.act(yt(YtAct::MenuPick(0)));
        assert_eq!(gui.app.queue.items.len(), 1, "queued");
        assert_eq!(gui.app.queue.items[0].track.filepath, "Music/Daft Punk/Discovery/Harder_Better_Faster_Stronger.mp3");
        assert!(!modal_open(&gui));
        // On the room's last row the list hangs over it, not off the frame.
        gui.act(yt(YtAct::More(8)));
        let rows = draw(&mut gui);
        assert!(rows[19].ends_with_at(64, "╭─────────────────╮") && rows[22].ends_with_at(64, "╰─────────────────╯"), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Esc);
        assert!(!modal_open(&gui) && in_room(&gui));
    }

    /// The list's keys, the form's free: ↓ shows the cursor, then walks it
    /// (from the link's field too); Enter is the held row's first way (a
    /// song's: play), `x` dismisses a settled row, `a` queues a song and
    /// `m` hangs its list; ↑ from the first row hands the keys back to the
    /// link; Esc puts the hand down — and Enter is the primary's again.
    #[test]
    fn the_lists_keys_walk_act_and_stow() {
        let _en = english();
        let mut gui = canvas(after_jobs(), Some(canvas_songs()));
        draw(&mut gui);
        assert_eq!(gui.ytdl.draft.keys, Keys::Link);
        key(&mut gui, KeyCode::Down);
        assert_eq!((gui.ytdl.draft.keys, gui.ytdl.list.held, gui.ytdl.list.view.held), (Keys::None, Some(0), true), "↓ leaves the field for the list");
        let b = buffer(&mut gui, 100, 30);
        assert!(b[(17, 15)].bg == th().accent && b[(63, 15)].bg == th().accent, "the held row is the slab, the block's width");
        assert_eq!(tips(&gui), "↑ ↓ rows · Esc deselect", "an open add has no way on");
        key(&mut gui, KeyCode::Down);
        assert_eq!(tips(&gui), "↑ ↓ rows · Enter Try again · x dismiss · Esc deselect");
        key(&mut gui, KeyCode::Char('x'));
        assert!(gui.ytdl.jobs().unwrap().job("j3").is_none(), "x dismissed the failure");
        draw(&mut gui);
        assert!(matches!(held_item(&gui), Some(Item::Song(_))), "the cursor rests on the row that took its place");
        assert_eq!(tips(&gui), "↑ ↓ rows · Enter play · a add to queue · m more · Esc deselect");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.queue.items.len(), 1, "Enter plays the held song");
        assert!(gui.ytdl.draft.starting.is_none(), "never the primary's while a row is held");
        key(&mut gui, KeyCode::Char('a'));
        assert_eq!(gui.app.queue.items.len(), 2, "a adds it to the queue");
        key(&mut gui, KeyCode::Char('m'));
        assert!(gui.ytdl.list.menu.is_some_and(|(_, cursor)| cursor == Some(0)), "m: its list, the cursor in it");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.ytdl.list.menu.is_none() && gui.ytdl.list.view.held, "Esc closes the song's list first");
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Up);
        assert_eq!((gui.ytdl.draft.keys, gui.ytdl.list.view.held), (Keys::Link, false), "↑ from the first row: the link's again");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.ytdl.list.view.held && in_room(&gui), "Esc puts the hand down");
        // A click in the link's card puts it down too.
        key(&mut gui, KeyCode::Down);
        gui.act(yt(YtAct::Field));
        assert!(!gui.ytdl.list.view.held && gui.ytdl.draft.keys == Keys::Link);
    }

    /// More rows than the room has: the wheel walks the list by a row,
    /// stopping where the last one shows with `Show earlier` under it; the
    /// keys bring the held row into view.
    #[test]
    fn the_wheel_and_the_keys_scroll_the_list() {
        let _en = english();
        let mut gui = main_room();
        let rows = draw(&mut gui);
        assert!(rows[22].contains("Digital Love") && rows[24].contains("Aerodynamic") && !rows.join("\n").contains("Show earlier"));
        gui.wheel(Position { x: 30, y: 23 }, 2);
        let rows = draw(&mut gui);
        assert!(rows[22].contains("Aerodynamic") && rows[24].contains("Da Funk"), "{}", rows.join("\n"));
        gui.wheel(Position { x: 30, y: 10 }, 40);
        let rows = draw(&mut gui);
        assert!(rows[22].contains("Face to Face") && rows[23].contains("Voyager") && rows[24].ends_with_at(64, "Show earlier"), "{}", rows.join("\n"));
        assert_eq!(gui.ytdl.list.view.scroll, 7, "clamped where the last rows show");
        gui.wheel(Position { x: 30, y: 23 }, -40);
        assert!(draw(&mut gui)[22].contains("Digital Love"));
        // The queue's own wheel is still the queue's.
        gui.wheel(Position { x: 80, y: 10 }, 3);
        assert_eq!(gui.ytdl.list.view.scroll, 0);
        // ↓ past the last row shown scrolls it into view.
        for _ in 0..5 {
            key(&mut gui, KeyCode::Down);
        }
        let rows = draw(&mut gui);
        assert_eq!(gui.ytdl.list.held, Some(4));
        assert!(rows[24].contains("Da Funk"), "the held row is the last shown:\n{}", rows.join("\n"));
        let b = buffer(&mut gui, 100, 30);
        assert_eq!(b[(17, 24)].bg, th().accent);
    }

    /// Show in folder: Files on the song's folder, and once it lists, the
    /// cursor held on the song.
    #[test]
    fn show_in_folder_rests_the_cursor_on_the_song() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        // Files was walked into somewhere else before: Music › Abba.
        let abba = crate::tui::app::Entry::Dir { label: "Abba".into(), path: "Music/Abba".into() };
        gui.app.files.trail.push(crate::tui::app::Trail { entries: vec![abba], chosen: 0 });
        draw(&mut gui);
        gui.act(yt(YtAct::More(0)));
        gui.act(yt(YtAct::MenuPick(1)));
        assert_eq!(gui.active, super::super::FILES_NAV);
        assert_eq!(gui.app.path, "Music/Daft Punk/Discovery");
        assert!(gui.pending.iter().any(|e| matches!(e, crate::tui::app::Effect::Api(ApiCmd::Browse(p)) if p == "Music/Daft Punk/Discovery")));
        poll(&mut gui);
        assert!(gui.ytdl.list.menu.is_none() && !modal_open(&gui));
        let track = |name: &str| crate::tui::app::Entry::Track {
            label: name.into(),
            track: Box::new(Track { filepath: format!("Music/Daft Punk/Discovery/{name}"), metadata: TrackMetadata::default() }),
        };
        gui.app.files.set(vec![crate::tui::app::Entry::Parent, track("Aerodynamic.mp3"), track("Digital_Love.mp3")]);
        gui.app.files.loading = false;
        poll(&mut gui);
        assert_eq!(gui.app.files.state.selected(), Some(2));
        assert!(gui.files_view.held, "lit: the list-cursor law's exception");
        // A jump, not a step in: Back asks for the parent, never the old
        // walk's rows.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('h'));
        assert_eq!(gui.app.path, "Music/Daft Punk");
        assert!(
            gui.pending.iter().any(|e| matches!(e, crate::tui::app::Effect::Api(ApiCmd::Browse(p)) if p == "Music/Daft Punk")),
            "{:?}",
            gui.pending
        );
        assert!(!gui.app.files.entries.iter().any(|e| e.label() == "Abba"));
    }

    // ── Keys ───────────────────────────────────────────────────────────────

    /// Esc gives the field's keys back; a second Esc leaves nothing — a
    /// nav room has no back, the Auto DJ room's rule — and goes to the
    /// shell, which stows nothing here. The room's own capitals never reach
    /// the shell (D opens Auto DJ, T Stats, A toggles the DJ); the shell's
    /// keys do, a digit opening its room — but not while a press is out:
    /// then every typed key waits, a link pasted as keys too, and the nav's
    /// rows are the way on.
    #[test]
    fn esc_gives_the_keys_back_and_the_rooms_capitals_stay_home() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        gui.ytdl.draft.keys = Keys::Link;
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "the keys given back");
        assert!(in_room(&gui));
        draw(&mut gui);
        let armed = gui.app.dj_armed();
        for c in ['D', 'T', 'A', 'E', 'U', 'M', 'X'] {
            shift(&mut gui, c);
            assert!(in_room(&gui), "{c} stayed in the room");
            assert_eq!(gui.screen, Screen::Library, "{c}");
            assert_eq!(gui.app.dj_armed(), armed, "{c}");
            gui.ytdl.draft.keys = Keys::None;
            gui.ytdl.draft.tags_open = false;
            gui.ytdl.place = None;
            draw(&mut gui);
        }
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Esc);
        assert!(in_room(&gui), "the second Esc leaves nothing: a nav room has no back");
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::YOUTUBE_NAV));
        assert_eq!(gui.settings_room, None, "and walks to no Settings room");
        key(&mut gui, KeyCode::Char('1'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::FILES_NAV), "1 is the nav's");

        // With the press out every typed key waits: the form's letters,
        // either case, and a link pasted as keys (a terminal with no
        // bracketed paste) — no `p`, `s`, `/`, `=`, digit or `q` of it
        // reaches the shell's globals. Esc still leaves nothing; the nav's
        // rows are the way on.
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some());
        draw(&mut gui);
        for c in ['e', 'd', 'f'] {
            key(&mut gui, KeyCode::Char(c));
        }
        for c in ['D', 'T', 'A'] {
            shift(&mut gui, c);
        }
        assert!(!gui.ytdl.draft.tags_open && gui.ytdl.place.is_none() && gui.ytdl.formats.is_none(), "the form's keys wait");
        assert_eq!((gui.screen, gui.app.dj_armed()), (Screen::Library, armed), "the capitals too");
        let transport = |gui: &Gui| (gui.app.queue.shuffle, gui.app.queue.repeat, gui.app.volume);
        let was = transport(&gui);
        for c in "https://www.youtube.com/watch?v=q7Zr-3sN9pE".chars() {
            let quit = super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            assert!(!quit, "{c}: no quit");
            assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::YOUTUBE_NAV), "{c}: no room");
        }
        assert!(!gui.app.editing_query, "no Search");
        assert_eq!(transport(&gui), was, "no Shuffle, Repeat or volume");
        assert!(gui.ytdl.draft.starting.is_some() && gui.ytdl.draft.keys == Keys::None, "the press still out, the form quiet");
        key(&mut gui, KeyCode::Esc);
        assert!(in_room(&gui), "Esc: nothing to leave");
        gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "the row again mid-press: the field stays quiet");
        gui.act(Act::Nav(super::super::FILES_NAV));
        assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::FILES_NAV), "a nav row is the way on");
    }

    /// Caps Lock sends the capital with no Shift: the room keeps it just
    /// the same — a capital of a control it drew is that control's (D: the
    /// Saves to row) — and so do the sheets and windows over it, whose
    /// keys are all theirs. D, T and A never open Auto DJ or Stats or
    /// toggle the DJ while any of them is up.
    #[test]
    fn caps_lock_capitals_stay_in_the_room_and_its_sheets() {
        let _en = english();
        let mut gui = main_room();
        draw(&mut gui);
        let armed = gui.app.dj_armed();
        let stays = |gui: &mut Gui, c: char, modifiers: KeyModifiers, up: &dyn Fn(&Gui) -> bool, what: &str| {
            super::super::handle_key(gui, KeyEvent::new(KeyCode::Char(c), modifiers));
            assert!(up(gui), "{what}: {c} ({modifiers:?}) kept it up");
            assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::YOUTUBE_NAV), "{what}: {c}");
            assert_eq!(gui.app.dj_armed(), armed, "{what}: {c}");
        };
        let room_up = |gui: &Gui| in_room(gui);
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            stays(&mut gui, 'D', modifiers, &room_up, "the room");
            assert!(super::super::ytdl_where::is_open(&gui), "D is the Saves to row");
            key(&mut gui, KeyCode::Esc);
            draw(&mut gui);
            for c in ['T', 'A'] {
                stays(&mut gui, c, modifiers, &room_up, "the room");
                draw(&mut gui);
            }
        }
        type Up = fn(&Gui) -> bool;
        let sheets: [(char, Up, &str); 3] =
            [('d', super::super::ytdl_where::is_open, "Saves to"), ('f', formats_up, "Format"), ('e', tags_up, "Edit tags")];
        for (opens, up, what) in sheets {
            key(&mut gui, KeyCode::Char(opens));
            draw(&mut gui);
            assert!(up(&gui), "{what} up");
            // Format's own `f` closes it: the rest are every sheet's to keep.
            for (c, modifiers) in ['D', 'T', 'A'].into_iter().flat_map(|c| [(c, KeyModifiers::NONE), (c, KeyModifiers::SHIFT)]) {
                stays(&mut gui, c, modifiers, &up, what);
                draw(&mut gui);
            }
            key(&mut gui, KeyCode::Esc);
            draw(&mut gui);
            assert!(in_room(&gui) && !modal_open(&gui), "{what} closed back to the room");
        }
        // Edit tags took the capitals as text: they are the title's.
        assert!(gui.ytdl.draft.tags().title.ends_with("DDTTAA"), "{}", gui.ytdl.draft.tags().title);
    }

    /// A key acts only where its control was drawn: `u` with nothing to
    /// undo does nothing, `a` with nothing to try again neither, and a
    /// letter typed into a tag is the tag's.
    #[test]
    fn a_key_acts_only_where_its_control_stands() {
        let _en = english();
        let mut gui = main_room();
        draw(&mut gui);
        let before = gui.ytdl.draft.tags();
        key(&mut gui, KeyCode::Char('u'));
        assert_eq!(gui.ytdl.draft.tags(), before);
        key(&mut gui, KeyCode::Char('a'));
        key(&mut gui, KeyCode::Char('t'));
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.ytdl.draft.found() && !modal_open(&gui), "no control, no act");
        for (c, up) in [('e', tags_up as fn(&Gui) -> bool), ('d', super::super::ytdl_where::is_open), ('f', formats_up)] {
            key(&mut gui, KeyCode::Char(c));
            assert!(up(&gui), "{c}: its control stands, so it acts");
            key(&mut gui, KeyCode::Esc);
            draw(&mut gui);
        }
    }

    /// The arrows are the room's: with no list under the form ↑ ↓ ← →
    /// walk no cursor and leave nothing, and Space stays the transport
    /// key.
    #[test]
    fn the_arrows_stay_the_rooms() {
        let _en = english();
        let mut gui = room();
        assert_eq!(gui.ytdl.draft.keys, Keys::Link);
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Esc, KeyCode::Down, KeyCode::Right, KeyCode::Left] {
            key(&mut gui, code);
            assert!(in_room(&gui) && gui.cursor.is_none(), "{code:?}: the room stays, no cursor walked");
            assert!(!gui.ytdl.list.view.held, "{code:?}: no list, no hand");
        }
        key(&mut gui, KeyCode::Char(' '));
        assert!(in_room(&gui) && gui.cursor.is_none(), "Space opened no row");
        // With the press out the arrows are the list's alone.
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some());
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Right);
        assert!(in_room(&gui) && gui.cursor.is_none());
    }

    /// The mini player draws no room: the letters of the last full frame's
    /// controls act on nothing there.
    #[test]
    fn the_mini_player_leaves_the_rooms_letters_dead() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        assert!(gui.ytdl.drawn.iter().any(|(c, _)| *c == 'd'), "the Saves to row drawn");
        buffer(&mut gui, 60, 12);
        key(&mut gui, KeyCode::Char('d'));
        assert!(!super::super::ytdl_where::is_open(&gui), "nothing opened behind the mini player");
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('d'));
        assert!(super::super::ytdl_where::is_open(&gui), "full size again, d is the row's");
    }

    // ── The lookup ─────────────────────────────────────────────────────────

    /// A typed link is looked up once the typing rests 400 ms, the field
    /// keeping the keys while it reads; found, it gives them back.
    #[test]
    fn a_typed_link_is_looked_up_once_the_typing_rests() {
        let _en = english();
        let mut gui = room();
        type_text(&mut gui, LINK);
        poll(&mut gui);
        assert!(gui.ytdl.work.is_empty());
        advance(&gui, 399);
        poll(&mut gui);
        assert!(gui.ytdl.work.is_empty(), "still typing");
        advance(&gui, 1);
        poll(&mut gui);
        assert!(matches!(&gui.ytdl.work[..], [Work::Lookup { url, .. }] if url == WATCH), "youtu.be asked as watch?v=");
        assert_eq!(gui.ytdl.draft.keys, Keys::Link);
        answer_lookup(&mut gui, Ok(one_more_time()));
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "found: the keys go back");
        // Enter looks up at once, giving them back meanwhile.
        let mut gui = room();
        type_text(&mut gui, LINK);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.ytdl.work.len(), 1);
        assert_eq!(gui.ytdl.draft.keys, Keys::None);
    }

    /// Request ids: an answer for a link since replaced is no one's; the
    /// lookup's own bound gives up on a thread that never answers.
    #[test]
    fn a_replaced_links_answer_is_dropped_and_the_lookup_has_a_bound() {
        let _en = english();
        let mut gui = room();
        paste(&mut gui, LINK);
        let first = match &gui.ytdl.work[0] {
            Work::Lookup { seq, .. } => *seq,
            other => panic!("{other:?}"),
        };
        paste(&mut gui, "https://youtu.be/HG7I6hd6Jfo");
        apply_reply(&mut gui, Reply::Lookup { seq: first, result: Ok(one_more_time()) });
        assert!(matches!(gui.ytdl.draft.look, Look::Reading { .. }), "the old answer fills nothing");
        advance(&gui, LOOKUP_BOUND.as_millis() as i64 + 1_000);
        poll(&mut gui);
        assert!(
            matches!(&gui.ytdl.draft.look, Look::Failed { failure, .. } if failure.problem == Problem::TimedOut),
            "{:?}",
            gui.ytdl.draft.look
        );
    }

    // ── Paste ──────────────────────────────────────────────────────────────

    /// A paste replaces the link and looks it up at once, whoever had the
    /// keys — through the loop's input half as the terminal sends it; on
    /// Windows' console, where a paste is typed, the field keeps the keys
    /// after a find.
    #[test]
    fn a_paste_lands_in_the_link_field_wherever_the_keys_are() {
        let _en = english();
        let mut gui = canvas(after_jobs(), Some(canvas_songs()));
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        assert!(gui.ytdl.list.view.held, "the list's hand is up");
        let mut ctx = super::super::tests::quiet_ctx(&gui);
        super::super::input(&mut gui, &mut ctx, TermEvent::Paste("  watch this:\nhttps://youtu.be/HG7I6hd6Jfo ".into()));
        assert_eq!(gui.ytdl.draft.link.value(), "watch this:", "the first line, trimmed");
        assert!(!gui.ytdl.list.view.held, "and the hand is down");
        super::super::input(&mut gui, &mut ctx, TermEvent::Paste("https://youtu.be/HG7I6hd6Jfo\u{200B}".into()));
        assert_eq!(gui.ytdl.draft.link.value(), "https://youtu.be/HG7I6hd6Jfo");
        assert!(matches!(gui.ytdl.draft.look, Look::Reading { .. }), "asked at once");
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "a whole paste gives the keys back");
        assert!(gui.ytdl.draft.tags().title.is_empty(), "a new link starts the form over");

        let mut gui = room();
        gui.ytdl.whole_pastes = false;
        type_text(&mut gui, LINK);
        advance(&gui, DEBOUNCE_MS);
        poll(&mut gui);
        answer_lookup(&mut gui, Ok(one_more_time()));
        assert_eq!(gui.ytdl.draft.keys, Keys::Link, "a typed paste may still be typing: the field keeps the keys");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some() || gui.ytdl.draft.folder.is_none(), "Enter is the primary once found");

        // Nothing wants a paste outside the room: dropped.
        let mut gui = seated(1);
        paste(&mut gui, LINK);
        assert!(gui.ytdl.draft.link.value().is_empty());
    }

    /// Another's surface over the room — the sign-in form a lapsed token
    /// opens, Add a server, the header's list — owns a paste: the link
    /// field wants none (it is typed into that surface's field, the old
    /// way), and one that comes anyway leaves the form be. So does one
    /// under the room's own sheets: over Edit tags and Saves to a paste is
    /// their field's, typed.
    #[test]
    fn a_paste_is_the_surface_over_the_rooms() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        let mut ctx = super::super::tests::quiet_ctx(&gui);
        super::super::servers::observe(&mut gui, &Event::Unauthorized);
        assert!(gui.servers.modal_open() && in_room(&gui), "the sign-in form over the room");
        assert!(!wants_paste(&gui), "bracketed paste goes off");
        super::super::input(&mut gui, &mut ctx, TermEvent::Paste("hunter2".into()));
        assert_eq!(gui.ytdl.draft.link.value(), LINK, "the link stands");
        assert!(gui.ytdl.draft.found(), "and the song found with it");
        gui.servers.form = None;
        assert!(wants_paste(&gui), "the form gone, the field wants one again");

        super::super::servers::open_add(&mut gui);
        assert!(!wants_paste(&gui), "Add a server");
        gui.servers.form = None;
        gui.servers.drop_open = true;
        assert!(!wants_paste(&gui), "the header's server list");
        // The quit's gate during a card sync holds every key: a paste under
        // it looks nothing up (mp3-player-screen contract, clause 57).
        gui.servers.drop_open = false;
        gui.device.quit_gate = true;
        assert!(!wants_paste(&gui), "the quit's gate during a card sync");
        let link = gui.ytdl.draft.link.value().to_string();
        super::super::input(&mut gui, &mut ctx, TermEvent::Paste("https://www.youtube.com/watch?v=zzz".into()));
        assert_eq!(gui.ytdl.draft.link.value(), link, "the link stands under the gate");
        gui.device.quit_gate = false;
        assert!(wants_paste(&gui), "the gate gone, the field wants one again");

        for (opens, what) in [(YtAct::Edit, "Edit tags"), (YtAct::Change, "Saves to"), (YtAct::Format, "Format")] {
            gui.act(yt(opens));
            assert!(modal_open(&gui) && !wants_paste(&gui), "{what}: no whole paste for the link");
            paste(&mut gui, "https://youtu.be/HG7I6hd6Jfo");
            assert_eq!(gui.ytdl.draft.link.value(), LINK, "{what}: one that comes anyway is dropped");
            key(&mut gui, KeyCode::Esc);
            assert!(wants_paste(&gui), "{what} closed: the field wants one again");
        }
    }

    /// Bracketed paste follows the room: on while it is up, off once it is
    /// left by any road, told to the host only when it changes.
    #[test]
    fn bracketed_paste_is_on_only_while_the_room_is_up() {
        #[derive(Default)]
        struct Pastes(Vec<bool>);
        impl super::super::Host for Pastes {
            fn pointer(&mut self, _hand: bool) {}
            fn paste(&mut self, on: bool) {
                self.0.push(on);
            }
        }
        let _en = english();
        let mut gui = seated(1);
        let mut ctx = super::super::tests::quiet_ctx(&gui);
        let mut terminal = Terminal::new(super::super::tests::IoTest(TestBackend::new(100, 30))).unwrap();
        let mut host = Pastes::default();
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        assert!(host.0.is_empty(), "off, and never turned on");
        gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        assert_eq!(host.0, vec![true]);
        gui.act(Act::Nav(super::super::FILES_NAV));
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        assert_eq!(host.0, vec![true, false], "leaving turned it off");
    }

    /// The window's face takes the clipboard whole while the room is up,
    /// and hands the room its clipboard for `Paste`.
    #[cfg(feature = "window")]
    #[test]
    fn the_window_takes_a_paste_whole_while_the_room_is_up() {
        use crate::gui::window::face::Face;
        let _en = english();
        let gui = seated(1);
        assert!(gui.ytdl.clipboard.is_none(), "a terminal reads no clipboard");
        let channels = super::super::tests::quiet_ctx(&gui).channels;
        let mut face = super::super::GuiFace::new(gui, channels);
        assert!(face.gui.ytdl.clipboard.is_some() && face.gui.ytdl.whole_pastes, "the window does");
        assert!(!face.takes_paste());
        face.gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        assert!(face.takes_paste());
        let rows = draw(&mut face.gui);
        assert!(rows[5].contains("│ Paste │"), "so `Paste` stands beside the link:\n{}", rows.join("\n"));
    }

    // ── The tracker ────────────────────────────────────────────────────────

    /// One tracker per server identity and account: kept across a
    /// reconnect to the same one, let go — its calls' answers with it — when
    /// the session moves to another; its calls wait in its wire until the
    /// turn is over, and keep the loop brisk while out.
    #[test]
    fn one_tracker_per_server_and_account() {
        let _en = english();
        let mut gui = room();
        assert!(gui.ytdl.held.as_ref().is_some_and(|h| h.identity == HOST));
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        poll(&mut gui);
        assert!(!calls(&gui).is_empty() && busy(&gui), "the add's first calls are out");
        observe(&mut gui, &connected(ping(1)));
        assert!(gui.ytdl.held.is_some(), "the same server again: kept");
        let other = Event::Connected {
            server: "http://other:3000".into(),
            id: "http://other:3000".into(),
            username: None,
            token: None,
            ping: Box::new(ping(0)),
        };
        observe(&mut gui, &other);
        assert!(gui.ytdl.held.is_none(), "another server: let go");
        assert!(gui.ytdl.draft.starting.is_none());
        gui.app.session.server = "http://other:3000".into();
        poll(&mut gui);
        let held = gui.ytdl.held.as_ref().expect("a tracker for the new server");
        assert_eq!(held.identity, "http://other:3000");
        assert_eq!(held.jobs.generation(), 0);
        // Another account on the first server is another tracker too.
        let mut gui = room();
        gui.app.session.username = Some("ana".into());
        poll(&mut gui);
        assert_eq!(gui.ytdl.held.as_ref().unwrap().username.as_deref(), Some("ana"));
        // A token's reconnect names no account: the session's stands, and
        // so do its tracker, the Files window and what the room holds.
        found(&mut gui, one_more_time(), discovery_new());
        gui.act(Act::Nav(super::super::FILES_NAV));
        gui.app.tab = Tab::Files;
        gui.app.path = "Music/Daft Punk".into();
        super::super::ytdl_here::open(&mut gui);
        assert!(super::super::ytdl_here::window_up(&gui));
        observe(&mut gui, &connected(ping(1)));
        assert_eq!(gui.ytdl.held.as_ref().map(|h| h.username.as_deref()), Some(Some("ana")), "kept");
        assert!(super::super::ytdl_here::window_up(&gui), "the window stays");
        assert!(gui.ytdl.draft.found(), "the room's song stays");
        let bob = Event::Connected {
            server: HOST.into(),
            id: HOST.into(),
            username: Some("bob".into()),
            token: None,
            ping: Box::new(ping(1)),
        };
        observe(&mut gui, &bob);
        assert!(gui.ytdl.held.is_none(), "a sign-in as another: let go");
        assert!(!super::super::ytdl_here::window_up(&gui));
    }

    // ── Past the boards ────────────────────────────────────────────────────

    /// Every key this module and its sheets draw is in the English file.
    #[test]
    fn the_rooms_words_are_in_the_locale() {
        let _en = english();
        let parsed: serde_yaml::Value = serde_yaml::from_str(include_str!("../../locales/en.yml")).unwrap();
        for source in [include_str!("ytdl.rs"), include_str!("ytdl_sheets.rs")] {
            let keys: std::collections::BTreeSet<&str> = source
                .match_indices("\"gui.")
                .map(|(at, _)| &source[at + 1..at + 1 + source[at + 1..].find('"').unwrap()])
                .filter(|k| !k.ends_with('.') && !k.contains('{'))
                .collect();
            for key in keys.into_iter().chain(crate::ytdl::KEYS.iter().copied()) {
                let mut node = &parsed;
                for part in key.split('.') {
                    node = &node[part];
                }
                assert!(node.as_str().is_some_and(|s| !s.is_empty()), "{key} is not in en.yml");
            }
        }
    }

    /// At the 100 × 24 floor with key hints on and the queue open — the
    /// tightest room there is — a failed lookup's three GOLD rows still
    /// leave the two rows and the primary's slab on screen: the blanks
    /// give way, the list takes what is left. And a folder the account
    /// can't make: its GOLD line under the rows, the ways on under it,
    /// wrapping rather than dropping one.
    #[test]
    fn the_tightest_room_keeps_where_the_song_goes() {
        let _en = english();
        let mut gui = canvas(vec![], Some(canvas_songs()));
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None }));
        answer_folders(&mut gui, there("/Music/", &[]));
        let rows = lines(&buffer(&mut gui, 100, 24));
        let text = rows.join("\n");
        assert!(rows[2].ends_with_at(64, " Add to library ▸ "), "the slab on the title row:\n{text}");
        assert_eq!(at(&rows, "╭──").y, 3, "the card rose into the title row's blank:\n{text}");
        let (place, format) = (at(&rows, "Saves to"), at(&rows, "Format"));
        assert_eq!((format.y, format.x), (place.y + 1, 17), "{text}");
        assert!(at(&rows, "FROM YOUTUBE").y > format.y && rows.iter().filter(|r| r.contains('✓')).count() == 3, "three rows of the list:\n{text}");

        let mut gui = seated(1);
        observe(&mut gui, &connected(Ping { no_mkdir: Some(true), ..ping(1) }));
        gui.act(Act::Nav(super::super::YOUTUBE_NAV));
        gui.queue_open = true;
        found(&mut gui, one_more_time(), discovery_new());
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        let gold = at(&rows, "! “Discovery” doesn't exist yet and your");
        assert_eq!((gold.x, gold.y, b[(gold.x, gold.y)].fg), (17, 16, th().gold), "under the two rows:\n{text}");
        let instead = at(&rows, "save to Music › Daft Punk instead");
        assert_eq!((instead.x, instead.y), (17, 18), "{text}");
        assert_eq!(at(&rows, "choose a folder ▸"), Position { x: 17, y: 19 }, "wrapped, not dropped:\n{text}");
        assert_eq!(gui.ui.hit(at(&rows, "choose a folder ▸")), Some(yt(YtAct::Choose)));
        assert_eq!(gui.ui.hit(at(&rows, "Add to library")), None, "the primary is off");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none());
        key(&mut gui, KeyCode::Char('i'));
        assert_eq!(gui.ytdl.draft.instead.as_deref(), Some("/Music/Daft Punk/"));
        answer_folders(&mut gui, there("/Music/Daft Punk/", &[]));
        let rows = draw(&mut gui);
        assert!(rows[14].ends_with_at(64, "Music › Daft Punk ▸"), "the folder that exists:\n{}", rows.join("\n"));
        assert!(rows[18].contains("Add to library ▸"), "and Add works again:\n{}", rows.join("\n"));
    }

    /// The form's other lines: letters a file name drops (DIM under the
    /// rows), a layout that lands nowhere (the place a dash, why DIM over
    /// the primary, which is off), the 409's taken name (GOLD over the
    /// primary) — another format the way on.
    #[test]
    fn the_forms_other_lines_read_in_their_tokens() {
        let _en = english();
        let mut gui = canvas(vec![], None);
        let kanji = YtdlMetadata { title: Some("東京 Night".into()), file_stem: None, ..one_more_time() };
        found(&mut gui, kanji, there("/Music/Daft Punk/Discovery/", &[]));
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let line = at(&rows, "about ");
        assert_eq!((line.x, line.y, b[(line.x, line.y)].fg), (17, 16, th().dim), "under the rows:\n{}", rows.join("\n"));
        assert!(rows[17].contains("dropped from file names"), "wrapped whole:\n{}", rows.join("\n"));

        let mut gui = canvas(vec![], None);
        gui.config.servers.push(config::ServerEntry {
            url: HOST.into(),
            ytdl_layout: Some("{{TITLE}}".into()),
            ytdl_layout_kind: Some("custom".into()),
            ..Default::default()
        });
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Ok(one_more_time()));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[14].ends_with_at(64, "— ▸"), "{text}");
        assert!(rows[16].ends_with_at(64, "Fix the layout to see where files land"), "{text}");
        assert_eq!(gui.ui.hit(at(&rows, "Add to library")), None, "off:\n{text}");
        assert!(gui.ytdl.work.iter().all(|w| !matches!(w, Work::Folders { .. })), "nowhere to check");

        let mut gui = canvas(vec![], None);
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        let mut taken = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Err(YtdlCallError::Answered {
                status: 409,
                words: Some("One_More_Time.mp3 is already in this folder".into()),
                reason: Some("exists".into()),
                file_name: Some("One_More_Time.mp3".into()),
            }))),
            other => new_server(None)(other),
        };
        serve(&mut gui, &mut taken);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let refusal = at(&rows, "! “One_More_Time.mp3” is already in this");
        assert_eq!((refusal.x, refusal.y, b[(refusal.x, refusal.y)].fg), (17, 16, th().gold), "{}", rows.join("\n"));
        // Another format is the way on: taken, the refusal goes.
        key(&mut gui, KeyCode::Char('f'));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.refusal.is_none());
        assert_eq!(gui.ytdl.draft.codec.as_deref(), Some("m4a"));
    }

    /// The clash warned of ahead of time, Add anyway pressed, and the
    /// server's 409: its GOLD lines over the primary stand alone — the
    /// warning they confirm goes.
    #[test]
    fn a_409_stands_for_the_clash_it_confirms() {
        let _en = english();
        let mut gui = canvas(vec![], None);
        found(&mut gui, one_more_time(), there("/Music/Daft Punk/Discovery/", &["One_More_Time.mp3"]));
        let rows = draw(&mut gui);
        assert!(rows.iter().any(|r| r.contains("looks to")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Enter);
        let mut taken = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Err(YtdlCallError::Answered {
                status: 409,
                words: Some("One_More_Time.mp3 is already in this folder".into()),
                reason: Some("exists".into()),
                file_name: Some("One_More_Time.mp3".into()),
            }))),
            other => new_server(None)(other),
        };
        serve(&mut gui, &mut taken);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("! “One_More_Time.mp3” is already in this") && all.contains("so nothing was added."), "{all}");
        assert!(!all.contains("looks to"), "{all}");
    }

    // ── Every language ─────────────────────────────────────────────────────

    pub(crate) const LOCALES: [&str; 10] = ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

    /// `words` whole on one of `rows` between cells `x0` and `x1` — a wide
    /// glyph's second cell reads as a blank, so compared without blanks.
    pub(crate) fn whole(rows: &[String], words: &str, (x0, x1): (usize, usize)) -> bool {
        let packed = words.replace(' ', "");
        rows.iter().any(|row| row.chars().skip(x0).take(x1 - x0).collect::<String>().replace(' ', "").contains(&packed))
    }

    /// The key words open with their key and a blank in every language —
    /// the letter never translates, the page shows the words after it, and
    /// a key acts only where its control stood — and every letter the room
    /// draws is one whose capital it keeps.
    #[test]
    fn every_locales_key_words_open_with_their_letter() {
        let words = [
            ("gui.ytdl.key.edit", 'e'),
            ("gui.ytdl.key.change", 'd'),
            ("gui.ytdl.key.format", 'f'),
            ("gui.ytdl.key.retry", 'a'),
            ("gui.ytdl.key.type", 't'),
            ("gui.ytdl.key.use", 'u'),
            ("gui.ytdl.key.undo", 'u'),
            ("gui.ytdl.key.instead", 'i'),
            ("gui.ytdl.key.choose", 'c'),
            ("gui.ytdl.key.subfolders", 's'),
            ("gui.ytdl.key.use_song", 'u'),
            ("gui.ytdl.key.add_year", 'y'),
            ("gui.tips.ytdl_format", 'f'),
            ("gui.tips.ytdl_dismiss", 'x'),
            ("gui.tips.ytdl_queue", 'a'),
            ("gui.tips.ytdl_more", 'm'),
        ];
        for code in LOCALES {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            for (key, letter) in words {
                let word = t(key);
                assert!(word.starts_with(&format!("{letter} ")), "{code}: {key} is {word}");
                let (shown_letter, shown) = page_words(&word);
                assert!(shown_letter == letter && !shown.is_empty() && !shown.starts_with(' '), "{code}: {key} shows {shown:?}");
            }
        }
        for (key, letter) in &words[..9] {
            assert!(ROOM_LETTERS.contains(letter), "{key}: the room keeps {} too", letter.to_ascii_uppercase());
        }
        assert!(ROOM_LETTERS.contains(&'m') && ROOM_LETTERS.contains(&'x'), "the list's m and x");
    }

    /// Every language in the cells the boards give it: each whole tips
    /// line in the footer's 99, and what a composed one always keeps once
    /// its key words gave way; `Paste` leaving the link's card its room at
    /// 47; a row's label, a value and its `▸` in the 47; the primary's
    /// words in the tall frame and the slab beside the title; the card's
    /// `edit tags ▸` leaving the title its room; a list row's right-hand
    /// words and its ways; the sheet's labels over their cards and its
    /// radios in their columns; the note's head with the boards' song.
    #[test]
    fn every_locales_words_fit_their_cells() {
        for code in LOCALES {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            let cells = |text: &str| width(text);
            let widest = |keys: &[&str]| keys.iter().map(|k| t(k)).max_by_key(|w| cells(w)).unwrap_or_default();
            for key in [
                "gui.tips.ytdl_field",
                "gui.tips.ytdl_tags",
                "gui.tips.ytdl_formats",
                "gui.tips.ytdl_menu",
                "gui.tips.ytdl_here_field",
                "gui.tips.ytdl_where_layout",
                "gui.tips.ytdl_choose",
            ] {
                assert!(cells(&t(key)) <= 99, "{code}: {key} is {} cells", cells(&t(key)));
            }
            let room_keys = ["gui.ytdl.key.edit", "gui.ytdl.key.change", "gui.ytdl.key.retry", "gui.ytdl.key.type", "gui.ytdl.key.use", "gui.ytdl.key.undo", "gui.ytdl.key.choose"];
            let ways = ["gui.ytdl.list.try_again", "gui.ytdl.list.change_format", "gui.ytdl.list.choose_folder", "gui.ytdl.list.top", "gui.ytdl.list.check_again", "gui.ytdl.list.add_again", "gui.ytdl.list.dismiss"];
            let enter = rust_i18n::t!("gui.tips.ytdl_enter", locale = code, verb = widest(&ways)).to_string();
            for line in [
                vec![t("gui.tips.ytdl_paste"), widest(&room_keys), t("gui.tips.ytdl_enter_add"), t("gui.tips.ytdl_rows")],
                vec![t("gui.tips.ytdl_enter_here"), widest(&[&room_keys[..], &["gui.ytdl.key.subfolders"][..]].concat()), t("gui.tips.ytdl_paste"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_here_dot"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_rows"), enter.clone(), t("gui.tips.ytdl_dismiss"), t("gui.tips.ytdl_stow")],
                vec![t("gui.tips.ytdl_rows"), t("gui.tips.ytdl_play"), t("gui.tips.ytdl_queue"), t("gui.tips.ytdl_more"), t("gui.tips.ytdl_stow")],
                vec![t("gui.tips.ytdl_where_library"), t("gui.tips.ytdl_where_rows"), t("gui.tips.ytdl_where_save"), t("gui.tips.ytdl_close")],
            ] {
                let line = line.join(" · ");
                assert!(cells(&line) <= 99, "{code}: {line} is {} cells", cells(&line));
            }

            // The link's band at 47: `Paste` in its frame, a cell, and a
            // card with its sixteen cells and more.
            let paste = cells(&t("gui.ytdl.room.paste_button")) + 4;
            assert!(paste + 1 + 16 <= 47, "{code}: Paste is {paste} cells");
            // The rows at 47: the label, two cells, a value of ten at
            // least, and ` ▸`.
            for key in ["gui.ytdl.room.saves_to", "gui.ytdl.here.saves_here", "gui.ytdl.room.format"] {
                assert!(cells(&t(key)) + 2 + 10 + 2 <= 47, "{code}: {key} is {} cells", cells(&t(key)));
            }
            // The primary's words: in the tall frame's 45, and as the slab
            // two cells clear of a title that keeps eight cells at 47.
            for key in ["gui.ytdl.add.to_library", "gui.ytdl.add.anyway", "gui.ytdl.add.adding"] {
                let label = format!("{} ▸", t(key));
                assert!(cells(&label) <= 45, "{code}: {label}");
                assert!(8 + 2 + cells(&label) + 2 <= 47, "{code}: {label} beside the title, as the slab");
            }
            assert!(cells(&format!("[ {} ]", widest(&["gui.ytdl.add.here", "gui.ytdl.add.to_library", "gui.ytdl.add.anyway", "gui.ytdl.add.adding"]))) <= 72, "{code}: the window's primary");
            // The card at 47: `edit tags ▸` leaves the title ten cells
            // (French's "modifier les étiquettes ▸" is the longest).
            let edit = usize::from(link_width(&t("gui.ytdl.key.edit"), true));
            assert!(9 + 10 + 2 + edit <= 47, "{code}: edit tags is {edit} cells");
            // The clean-up's line keeps ten cells of the title beside `undo`.
            let undo = usize::from(link_width(&t("gui.ytdl.key.undo"), false));
            let cleaned = cells(&rust_i18n::t!("gui.ytdl.source.cleaned", locale = code, title = "").to_string());
            assert!(cleaned + 10 + 3 + undo <= 47, "{code}: the clean-up's line");

            // A list row at 47: a job's words at its right leave its title
            // six cells (Russian's "not sure it finished" is the longest); a
            // song's when sits in the place's twenty-two.
            let stands = [
                format!("{} · {} · Opus", t("gui.ytdl.list.adding"), rust_i18n::t!("gui.ytdl.list.minutes", locale = code, count = 59)),
                t("gui.ytdl.list.starting"),
                t("gui.ytdl.list.not_sure_started"),
                t("gui.ytdl.list.not_sure_finished"),
                t("gui.ytdl.list.waiting"),
                t("gui.ytdl.list.failed"),
            ];
            for words in &stands {
                assert!(2 + 6 + 2 + cells(words) <= 47, "{code}: {words} crowds the title");
            }
            let when = [
                t("gui.ytdl.list.file_missing"),
                t("gui.ytdl.list.just_now"),
                rust_i18n::t!("gui.ytdl.list.min_ago", locale = code, count = 59).to_string(),
                rust_i18n::t!("gui.ytdl.list.hours_ago", locale = code, count = 23).to_string(),
            ];
            for words in &when {
                assert!(cells(words) <= 22, "{code}: {words} is wider than the place's cells");
            }
            // A row's ways, each whole in the block; a set of them on two
            // rows at most (German's "Direkt in die Bibliothek · Ordner
            // wählen · Ausblenden" takes the second).
            for set in [&[ways[3], ways[2], ways[6]][..], &[ways[0], ways[2], ways[6]], &[ways[1], ways[6]]] {
                let set: Vec<(String, Way)> = set.iter().map(|key| (t(key), Way::Dismiss)).collect();
                assert!(set.iter().all(|(words, _)| cells(words) <= 47), "{code}: {set:?}");
                assert!(way_rows(&set, 47).len() <= 2, "{code}: {set:?}");
                assert!(way_rows(&set, 47).iter().all(|row| ways_width(row) <= 47), "{code}: {set:?}");
            }
            assert!(cells(&t("gui.ytdl.list.show_earlier")) <= 47, "{code}: Show earlier");
            assert!(crate::kit::wrap_whole(&t("gui.ytdl.room.limits"), 47).len() <= 3, "{code}: the limits at 47");

            // The sheets, 62 cells of text: Edit tags' labels over their
            // cards (the year's nine), its title on the border; Format's
            // names and helps on a row, its foot beside `[ Done ]`; the
            // window's radios in their half.
            for (key, room) in [("gui.ytdl.tag.title", 62), ("gui.ytdl.tag.artist", 28), ("gui.ytdl.tag.album", 21), ("gui.ytdl.tag.year", 9)] {
                assert!(cells(&t(key)) <= room, "{code}: {key}");
            }
            let done = cells(&format!("[ {} ]", t("gui.ytdl.done")));
            assert!(cells(&t("gui.ytdl.format.best")) <= 62, "{code}: what Format chooses");
            for format in ["mp3", "m4a", "opus", "flac", "wav"] {
                let help = rust_i18n::t!(format!("gui.ytdl.format.{format}"), locale = code).to_string();
                assert!(crate::kit::wrap_whole(&help, 50).len() <= 2, "{code}: {format}'s help is {} cells", cells(&help));
            }
            let remembered = rust_i18n::t!("gui.ytdl.format.remembered", locale = code, server = "music.home.arpa").to_string();
            assert!(cells(&remembered) + 2 + done <= 62 - 1, "{code}: {remembered} beside Done");
            for key in ["gui.ytdl.sort.here", "gui.ytdl.sort.album", "gui.ytdl.sort.artist_album", "gui.ytdl.sort.custom"] {
                assert!(4 + cells(&t(key)) <= 30, "{code}: {key} in its column");
            }
            let save = cells(&format!("[ {} ]", t("gui.ytdl.where.save")));
            let use_song = usize::from(link_width(&t("gui.ytdl.key.use_song"), false));
            assert!(use_song + 3 + save <= 62 - 1, "{code}: use for this song beside Save");

            // The note's head whole in its 30 with the boards' song.
            for key in ["gui.ytdl.note.adding", "gui.ytdl.note.added"] {
                let note = rust_i18n::t!(key, locale = code, title = "One More Time").to_string();
                assert!(cells(&note) <= 30, "{code}: {note} is {} cells", cells(&note));
            }

            // Sentences that wrap rather than clip: still looking beside the
            // slot (two rows of the card's 38), the late answer beside `use
            // them` and why Add is off at 47.
            let lines = |key: &str, room: usize| crate::kit::wrap_whole(&t(key), room).len();
            assert!(lines("gui.ytdl.room.slow", 38) <= 2, "{code}: still looking beside the slot");
            let used = usize::from(link_width(&t("gui.ytdl.key.use"), false));
            assert!(lines("gui.ytdl.room.late", 47 - 3 - used) <= 2, "{code}: the late answer at 47");
            assert!(lines("gui.ytdl.where.fix_layout", 47) <= 2, "{code}: why Add is off at 47");
            assert!(lines("gui.ytdl.room.empty_title", 47) + lines("gui.ytdl.room.empty_web", 47) <= 5, "{code}: the first time's words");
        }
    }

    /// Every language drawn: the nav row's label whole in its 12 cells, a
    /// click target (100 × 30); at 47 cells with the queue open the room
    /// keeps its controls whole and its tips line in the footer's 99; the
    /// list's ways, the sheets' words and their tips likewise.
    #[test]
    fn every_locale_draws_the_room_and_its_sheets_whole() {
        let Some(code) = crate::setup::tests::in_own_process(&LOCALES) else { return };
        let mut gui = seated(1);
        let rows = draw(&mut gui);
        let label = t!("gui.nav.add_youtube").to_string();
        assert!(whole(&rows[17..18], &label, (3, 15)), "{code}: {label} whole in x 3–14:\n{}", rows[17]);
        assert_eq!(gui.ui.hit(Position { x: 3, y: 17 }), Some(Act::Nav(super::super::YOUTUBE_NAV)), "{code}");

        let shown = |key: &str| page_words(&t!(key)).1.to_string();
        let mut gui = main_room();
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        for words in [
            t!("gui.ytdl.room.paste_button").to_string(),
            shown("gui.ytdl.key.edit"),
            t!("gui.ytdl.room.saves_to").to_string(),
            t!("gui.ytdl.room.format").to_string(),
            t!("gui.ytdl.add.to_library").to_string(),
            t!("gui.ytdl.list.title").to_uppercase(),
        ] {
            assert!(whole(&rows, &words, (17, 64)), "{code}: {words} whole at 47 cells:\n{text}");
        }
        let line = tips(&gui);
        assert!(width(&line) <= 99 && line.contains(&*t!("gui.tips.ytdl_enter_add")), "{code}: Enter stays: {line}");
        assert!(line.starts_with(&*t!("gui.tips.ytdl_paste")), "{code}: the paste stays: {line}");
        // The floor: the slab whole beside the title.
        let rows = lines(&buffer(&mut gui, 100, 24));
        assert!(whole(&rows[2..3], &t!("gui.ytdl.add.to_library"), (17, 64)), "{code}: the slab:\n{}", rows[2]);
        let title: String = t!("gui.ytdl.room.title").chars().take(4).collect();
        assert!(whole(&rows[2..3], &title, (17, 64)), "{code}: and the title's start, two cells clear of it:\n{}", rows[2]);

        // The sheets: their titles, their button, their tips.
        key(&mut gui, KeyCode::Char('e'));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        for words in [t!("gui.ytdl.tags.title"), t!("gui.ytdl.tag.artist"), t!("gui.ytdl.tag.year"), t!("gui.ytdl.done")] {
            assert!(whole(&rows, &words, (17, 83)), "{code}: {words} in Edit tags:\n{text}");
        }
        assert!(width(&tips(&gui)) <= 99, "{code}");
        key(&mut gui, KeyCode::Esc);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('f'));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        let remembered = t!("gui.ytdl.format.remembered", server = "Home server").to_string();
        for words in [t!("gui.ytdl.format.best").to_string(), remembered, t!("gui.ytdl.format.mp3").to_string()] {
            assert!(whole(&rows, &words, (17, 83)), "{code}: {words} in Format:\n{text}");
        }
        // A help longer than its column wraps in it, nothing cut.
        let packed: String = rows.iter().map(|r| r.chars().skip(31).take(50).collect::<String>()).collect::<String>().replace(' ', "");
        assert!(packed.contains(&t!("gui.ytdl.format.opus").replace(' ', "")), "{code}: Opus's help whole:\n{text}");
        key(&mut gui, KeyCode::Esc);

        // The list: a failure's ways, its tips with the hand up.
        let mut landed = job("k2", "failed", "One More Time", "/Music/Daft Punk/Discovery/");
        landed["failure"] = serde_json::json!({ "problem": "exists", "phase": "download", "detail": "One_More_Time.mp3" });
        let mut gui = canvas(vec![landed], None);
        let rows = draw(&mut gui);
        let ways = format!("{} · {}", t!("gui.ytdl.list.change_format"), t!("gui.ytdl.list.dismiss"));
        assert!(whole(&rows, &ways, (17, 64)), "{code}: {ways}:\n{}", rows.join("\n"));
        assert!(whole(&rows, &t!("gui.ytdl.list.failed"), (17, 64)), "{code}");
        key(&mut gui, KeyCode::Down);
        let line = tips(&gui);
        assert!(width(&line) <= 99 && line.ends_with(&*t!("gui.tips.ytdl_stow")), "{code}: {line}");

        // A sentence shown whole however it wraps — one with no blanks (ja,
        // zh) included — beside the controls on its rows.
        // (Blanks of every kind aside: French sets its "500 Mo" with a
        // no-break space, which a wrap may break at all the same.)
        let pack = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let shows = |rows: &[String], text: &str, (x0, x1): (usize, usize), beside: &[&str]| {
            let mut packed = pack(&rows.iter().map(|r| r.chars().skip(x0).take(x1 - x0).collect::<String>()).collect::<String>());
            for words in beside {
                packed = packed.replacen(&pack(words), "", 1);
            }
            packed.contains(&pack(text))
        };
        // Still looking, beside the slot at 47 cells.
        let mut gui = canvas(vec![], None);
        paste(&mut gui, LINK);
        advance(&gui, 8_000);
        poll(&mut gui);
        let rows = draw(&mut gui);
        let (slow, typed) = (t!("gui.ytdl.room.slow").to_string(), shown("gui.ytdl.key.type"));
        assert!(whole(&rows, &typed, (26, 64)), "{code}: {typed}:\n{}", rows.join("\n"));
        assert!(shows(&rows[9..11], &slow, (26, 64), &[]), "{code}: {slow}:\n{}", rows.join("\n"));
        // The late answer beside `use them`.
        key(&mut gui, KeyCode::Char('t'));
        type_text(&mut gui, "One More Time");
        key(&mut gui, KeyCode::Esc);
        answer_lookup(&mut gui, Ok(one_more_time()));
        let rows = draw(&mut gui);
        let (late, used) = (t!("gui.ytdl.room.late").to_string(), shown("gui.ytdl.key.use"));
        assert!(whole(&rows, &used, (17, 64)), "{code}: {used} at 47 cells:\n{}", rows.join("\n"));
        assert!(shows(&rows, &late, (17, 64), &[&used]), "{code}: {late} at 47 cells:\n{}", rows.join("\n"));
        // A lookup the server couldn't read, and the limits before any link.
        let mut gui = canvas(vec![], None);
        let rows = draw(&mut gui);
        assert!(shows(&rows, &t!("gui.ytdl.room.limits"), (17, 64), &[]), "{code}: the limits:\n{}", rows.join("\n"));
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None }));
        let rows = draw(&mut gui);
        let failed = t!("gui.ytdl.lookup.failed").to_string();
        assert!(shows(&rows, &format!("!{failed}"), (17, 64), &[]), "{code}: {failed}:\n{}", rows.join("\n"));
        // A row that didn't start, inside the room's 47.
        let mut start = job("s1", "startFailed", "One More Time", "/Music/Daft Punk/Discovery/");
        start["failure"] = serde_json::json!({ "problem": "ffmpegNotReady", "phase": "start" });
        let mut gui = canvas(vec![start], None);
        let rows = draw(&mut gui);
        let ffmpeg = t!("gui.ytdl.err.ffmpeg").to_string();
        assert!(shows(&rows, &ffmpeg, (19, 64), &[]), "{code}: {ffmpeg} inside the list:\n{}", rows.join("\n"));
        // The first time's words.
        let mut gui = canvas(vec![], None);
        serve(&mut gui, &mut nothing_added());
        let rows = draw(&mut gui);
        assert!(shows(&rows, &t!("gui.ytdl.room.empty_title"), (17, 64), &[]), "{code}:\n{}", rows.join("\n"));
    }

    /// A sentence with no blanks to break at (Japanese, Chinese) wraps
    /// within its room — a GOLD line's rows, a row's words — never one line
    /// past it.
    #[test]
    fn a_sentence_without_blanks_wraps_within_its_room() {
        let text = "サーバーはこの動画の情報を読み込めませんでした。".repeat(3);
        for room in [20, 46, 79] {
            let rows = gold_rows(&text, room);
            assert!(rows.len() > 1 && rows.iter().all(|r| width(r) <= room), "{room}: {rows:?}");
            let back: String = rows.iter().map(|r| r.trim_start_matches('!').trim_start()).collect();
            assert_eq!(back, text, "{room}: nothing lost");
        }
    }

    /// The canvas's frames ("Add from YouTube · GUI", 2026-11-10), row by
    /// row from their first cell (trailing blanks dropped), and their
    /// tokens as `row:first-last:TOKEN[:B]` — read off the boards' own
    /// cells, the cover's picture drawn as the empty slot the tests see.
    /// N1–N5 are the bar's notes.
    #[rustfmt::skip]
    mod boards {
        pub(super) const MAIN: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ One More Time              edit tags ▸
            |│      │ Daft Punk
            |│      │ Discovery · 2001
            |╰──────╯ YouTube · Daft Punk · 5:20
            |Tags from YouTube Music
            |
            |Saves to    Music › Daft Punk › Discovery new ▸
            |Format                                    MP3 ▸
            |
            |╭─────────────────────────────────────────────╮
            |│               Add to library ▸              │
            |╰─────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
        "#;
        pub(super) const MAIN_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:32-46:Dim", "4:0-46:Dim", "6:0-7:Dim",
            "6:9-21:Text:B", "6:36-46:Dim", "7:0-7:Dim", "7:9-17:Text", "8:0-24:Dim", "9:0-34:Dim", "10:0-22:Dim",
            "12:0-16:Text", "12:18-18:Dim", "12:20-28:Text", "12:30-30:Dim", "12:32-40:Text", "12:42-46:Dim",
            "13:0-5:Text", "13:42-46:Dim", "15:0-46:Accent", "16:0-0:Accent", "16:16-31:Accent:B", "16:46-46:Accent",
            "17:0-46:Accent", "19:0-11:Dim", "20:0-0:Ok", "20:2-13:Text", "20:25-46:Dim", "21:0-0:Ok",
            "21:2-23:Text", "21:25-46:Dim", "22:0-0:Ok", "22:2-12:Text", "22:25-46:Dim",
        ];
        pub(super) const EMPTY: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ ▏Paste a YouTube link             │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |One video per link · the server stops at 500 MB
            |or after an hour · no live streams or playlists
            |
            |Saves to               Music · Artist / Album ▸
            |Format                                    MP3 ▸
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
            |✓ Around the World       Homework        Oct 28
            |✓ Da Funk                Homework        Oct 27
            |✓ Veridis Quo            Discovery       Oct 27
            |✓ Something About Us     Discovery       Oct 25
            |✓ Face to Face           Discovery       Oct 25
            |✓ Voyager                Discovery       Oct 25
            |                                   Show earlier
        "#;
        pub(super) const EMPTY_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-36:Accent", "2:38-46:Dim", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:36-36:Accent", "3:38-46:Dim", "4:0-36:Accent", "4:38-46:Dim", "6:0-46:Dim", "7:0-46:Dim",
            "9:0-7:Text", "9:23-46:Dim", "10:0-5:Text", "10:42-46:Dim", "12:0-11:Dim", "13:0-0:Ok", "13:2-13:Text",
            "13:25-46:Dim", "14:0-0:Ok", "14:2-23:Text", "14:25-46:Dim", "15:0-0:Ok", "15:2-12:Text", "15:25-46:Dim",
            "16:0-0:Ok", "16:2-17:Text", "16:25-46:Dim", "17:0-0:Ok", "17:2-8:Text", "17:25-46:Dim", "18:0-0:Ok",
            "18:2-12:Text", "18:25-46:Dim", "19:0-0:Ok", "19:2-19:Text", "19:25-46:Dim", "20:0-0:Ok", "20:2-13:Text",
            "20:25-46:Dim", "21:0-0:Ok", "21:2-8:Text", "21:25-46:Dim", "22:35-46:Dim",
        ];
        pub(super) const AFTER: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ ▏Paste a YouTube link             │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |One video per link · the server stops at 500 MB
            |or after an hour · no live streams or playlists
            |
            |Saves to               Music · Artist / Album ▸
            |Format                                    MP3 ▸
            |
            |FROM YOUTUBE
            |↓ One More Time            adding · 1 min · MP3
            |! Robot Rock                             failed
            |  the server said: “Sign in to confirm you’re
            |  not a bot.”               Try again · Dismiss
            |✓ Digital Love           Discovery  [▸] [+] [⋯]
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
            |✓ Around the World       Homework        Oct 28
            |✓ Da Funk                Homework        Oct 27
            |✓ Veridis Quo            Discovery       Oct 27
        "#;
        pub(super) const AFTER_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-36:Accent", "2:38-46:Dim", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:36-36:Accent", "3:38-46:Dim", "4:0-36:Accent", "4:38-46:Dim", "6:0-46:Dim", "7:0-46:Dim",
            "9:0-7:Text", "9:23-46:Dim", "10:0-5:Text", "10:42-46:Dim", "12:0-11:Dim", "13:0-0:Accent",
            "13:2-14:Text", "13:27-46:Accent", "14:0-0:Gold", "14:2-11:Text", "14:41-46:Gold", "15:2-44:Dim",
            "16:2-12:Dim", "16:28-36:Text", "16:38-38:Dim", "16:40-46:Text", "17:0-0:Ok", "17:2-13:Bright:B",
            "17:25-33:Dim", "17:36-46:Text", "18:0-0:Ok", "18:2-23:Text", "18:25-46:Dim", "19:0-0:Ok",
            "19:2-12:Text", "19:25-46:Dim", "20:0-0:Ok", "20:2-17:Text", "20:25-46:Dim", "21:0-0:Ok", "21:2-8:Text",
            "21:25-46:Dim", "22:0-0:Ok", "22:2-12:Text", "22:25-46:Dim",
        ];
        pub(super) const MIN: &str = r#"
            |Add from YouTube              Add to library ▸
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ One More Time              edit tags ▸
            |│      │ Daft Punk
            |│      │ Discovery · 2001
            |╰──────╯ YouTube · Daft Punk · 5:20
            |Tags from YouTube Music
            |Saves to    Music › Daft Punk › Discovery new ▸
            |Format                                    MP3 ▸
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
        "#;
        pub(super) const MIN_RUNS: &[&str] = &[
            "0:0-15:Text:B", "0:30-45:OnAccent:B", "1:0-46:Dim", "2:0-0:Dim", "2:2-29:Text", "2:32-46:Dim",
            "3:0-46:Dim", "5:0-7:Dim", "5:9-21:Text:B", "5:36-46:Dim", "6:0-7:Dim", "6:9-17:Text", "7:0-24:Dim",
            "8:0-34:Dim", "9:0-22:Dim", "10:0-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim",
            "10:32-40:Text", "10:42-46:Dim", "11:0-5:Text", "11:42-46:Dim", "13:0-11:Dim", "14:0-0:Ok",
            "14:2-13:Text", "14:25-46:Dim", "15:0-0:Ok", "15:2-23:Text", "15:25-46:Dim", "16:0-0:Ok", "16:2-12:Text",
            "16:25-46:Dim",
        ];
        pub(super) const WIDE: &str = r#"
            |Add from YouTube
            |
            |╭────────────────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8           [X] │ │ Paste │
            |╰────────────────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ One More Time                       edit tags ▸
            |│      │ Daft Punk
            |│      │ Discovery · 2001
            |╰──────╯ YouTube · Daft Punk · 5:20
            |Tags from YouTube Music
            |
            |Saves to             Music › Daft Punk › Discovery new ▸
            |Format                                             MP3 ▸
            |
            |╭──────────────────────────────────────────────────────╮
            |│                   Add to library ▸                   │
            |╰──────────────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love                  Discovery        1 h ago
            |✓ Harder, Better, Faster, Stro… Discovery         Oct 31
            |✓ Aerodynamic                   Mixes             Oct 28
            |✓ Around the World              Homework          Oct 28
            |✓ Da Funk                       Homework          Oct 27
            |✓ Veridis Quo                   Discovery         Oct 27
            |✓ Something About Us            Discovery         Oct 25
        "#;
        pub(super) const WIDE_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-55:Dim", "3:0-0:Dim", "3:2-29:Text", "3:41-55:Dim", "4:0-55:Dim", "6:0-7:Dim",
            "6:9-21:Text:B", "6:45-55:Dim", "7:0-7:Dim", "7:9-17:Text", "8:0-24:Dim", "9:0-34:Dim", "10:0-22:Dim",
            "12:0-25:Text", "12:27-27:Dim", "12:29-37:Text", "12:39-39:Dim", "12:41-49:Text", "12:51-55:Dim",
            "13:0-5:Text", "13:51-55:Dim", "15:0-55:Accent", "16:0-0:Accent", "16:20-35:Accent:B", "16:55-55:Accent",
            "17:0-55:Accent", "19:0-11:Dim", "20:0-0:Ok", "20:2-13:Text", "20:32-55:Dim", "21:0-0:Ok",
            "21:2-30:Text", "21:32-55:Dim", "22:0-0:Ok", "22:2-12:Text", "22:32-55:Dim", "23:0-0:Ok", "23:2-17:Text",
            "23:32-55:Dim", "24:0-0:Ok", "24:2-8:Text", "24:32-55:Dim", "25:0-0:Ok", "25:2-12:Text", "25:32-55:Dim",
            "26:0-0:Ok", "26:2-19:Text", "26:32-55:Dim",
        ];
        pub(super) const FIRST: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ ▏Paste a YouTube link             │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |One video per link · the server stops at 500 MB
            |or after an hour · no live streams or playlists
            |
            |Saves to               Music · Artist / Album ▸
            |Format                                    MP3 ▸
            |
            |FROM YOUTUBE
            |Paste a link to add a song to your library.
            |Songs added from the web app show up here too.
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const FIRST_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-36:Accent", "2:38-46:Dim", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:36-36:Accent", "3:38-46:Dim", "4:0-36:Accent", "4:38-46:Dim", "6:0-46:Dim", "7:0-46:Dim",
            "9:0-7:Text", "9:23-46:Dim", "10:0-5:Text", "10:42-46:Dim", "12:0-11:Dim", "13:0-42:Dim", "14:0-45:Dim",
        ];
        pub(super) const READING: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ Reading the video…
            |│      │ Still looking — YouTube can be slow.
            |│      │
            |╰──────╯ type the details ▸
            |
            |Saves to               Music · Artist / Album ▸
            |Format                                    MP3 ▸
            |
            |╭─────────────────────────────────────────────╮
            |│                Add to library               │
            |╰─────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
            |✓ Around the World       Homework        Oct 28
        "#;
        pub(super) const READING_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:32-46:Dim", "4:0-46:Dim", "6:0-7:Dim",
            "6:9-26:Accent", "7:0-44:Dim", "8:0-7:Dim", "9:0-26:Dim", "11:0-7:Text", "11:23-46:Dim", "12:0-5:Text",
            "12:42-46:Dim", "14:0-46:Dim", "15:0-46:Dim", "16:0-46:Dim", "18:0-11:Dim", "19:0-0:Ok", "19:2-13:Text",
            "19:25-46:Dim", "20:0-0:Ok", "20:2-23:Text", "20:25-46:Dim", "21:0-0:Ok", "21:2-12:Text", "21:25-46:Dim",
            "22:0-0:Ok", "22:2-17:Text", "22:25-46:Dim",
        ];
        pub(super) const REFUSED: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ …com/daft-punk/one-more-time▏ [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |! That's not a YouTube link. Paste one from
            |  youtube.com or youtu.be.
            |
            |Saves to               Music · Artist / Album ▸
            |Format                                    MP3 ▸
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
            |✓ Around the World       Homework        Oct 28
            |✓ Da Funk                Homework        Oct 27
            |✓ Veridis Quo            Discovery       Oct 27
            |
            |
            |
            |
            |
        "#;
        pub(super) const REFUSED_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-36:Accent", "2:38-46:Dim", "3:0-0:Accent", "3:2-30:Text", "3:32-34:Dim",
            "3:36-36:Accent", "3:38-46:Dim", "4:0-36:Accent", "4:38-46:Dim", "5:0-42:Gold", "6:2-25:Gold",
            "8:0-7:Text", "8:23-46:Dim", "9:0-5:Text", "9:42-46:Dim", "11:0-11:Dim", "12:0-0:Ok", "12:2-13:Text",
            "12:25-46:Dim", "13:0-0:Ok", "13:2-23:Text", "13:25-46:Dim", "14:0-0:Ok", "14:2-12:Text", "14:25-46:Dim",
            "15:0-0:Ok", "15:2-17:Text", "15:25-46:Dim", "16:0-0:Ok", "16:2-8:Text", "16:25-46:Dim", "17:0-0:Ok",
            "17:2-12:Text", "17:25-46:Dim",
        ];
        pub(super) const CLEANED: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ One More Time              edit tags ▸
            |│      │ Daft Punk
            |│      │ No album · 2009
            |╰──────╯ YouTube · DaftPunkVEVO · 5:20
            |Cleaned from “One More Time (Official …”   undo
            |! Artist may be the channel name — check it
            |
            |Saves to                Music › Daft Punk new ▸
            |Format                                    MP3 ▸
            |
            |╭─────────────────────────────────────────────╮
            |│               Add to library ▸              │
            |╰─────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
        "#;
        pub(super) const CLEANED_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:32-46:Dim", "4:0-46:Dim", "6:0-7:Dim",
            "6:9-21:Text:B", "6:36-46:Dim", "7:0-7:Dim", "7:9-17:Text", "8:0-23:Dim", "9:0-37:Dim", "10:0-46:Dim",
            "11:0-42:Gold", "13:0-28:Text", "13:30-30:Dim", "13:32-40:Text", "13:42-46:Dim", "14:0-5:Text",
            "14:42-46:Dim", "16:0-46:Accent", "17:0-0:Accent", "17:16-31:Accent:B", "17:46-46:Accent",
            "18:0-46:Accent", "20:0-11:Dim", "21:0-0:Ok", "21:2-13:Text", "21:25-46:Dim", "22:0-0:Ok",
            "22:2-23:Text", "22:25-46:Dim",
        ];
        pub(super) const CLASH: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |
            |╭──────╮ One More Time              edit tags ▸
            |│      │ Daft Punk
            |│      │ Discovery · 2001
            |╰──────╯ YouTube · Daft Punk · 5:20
            |Tags from YouTube Music
            |
            |Saves to        Music › Daft Punk › Discovery ▸
            |Format                                    MP3 ▸
            |! Something like “One_More_Time.mp3” looks to
            |  be in this folder already.
            |
            |╭─────────────────────────────────────────────╮
            |│                 Add anyway ▸                │
            |╰─────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
        "#;
        pub(super) const CLASH_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:32-46:Dim", "4:0-46:Dim", "6:0-7:Dim",
            "6:9-21:Text:B", "6:36-46:Dim", "7:0-7:Dim", "7:9-17:Text", "8:0-24:Dim", "9:0-34:Dim", "10:0-22:Dim",
            "12:0-20:Text", "12:22-22:Dim", "12:24-32:Text", "12:34-34:Dim", "12:36-44:Text", "12:46-46:Dim",
            "13:0-5:Text", "13:42-46:Dim", "14:0-44:Gold", "15:2-27:Gold", "17:0-46:Accent", "18:0-0:Accent",
            "18:18-29:Accent:B", "18:46-46:Accent", "19:0-46:Accent", "21:0-11:Dim", "22:0-0:Ok", "22:2-13:Text",
            "22:25-46:Dim",
        ];
        pub(super) const FAILED: &str = r#"
            |Add from YouTube
            |
            |╭───────────────────────────────────╮ ╭───────╮
            |│ https://youtu.be/FGBhQbmPwH8  [X] │ │ Paste │
            |╰───────────────────────────────────╯ ╰───────╯
            |! Couldn't read this video's details. It may be
            |  private, age-restricted or blocked for the
            |  server.
            |
            |type the details ▸
            |
            |Saves to                                Music ▸
            |Format                                    MP3 ▸
            |
            |╭─────────────────────────────────────────────╮
            |│               Add to library ▸              │
            |╰─────────────────────────────────────────────╯
            |
            |FROM YOUTUBE
            |✓ Digital Love           Discovery      1 h ago
            |✓ Harder, Better, Faste… Discovery       Oct 31
            |✓ Aerodynamic            Mixes           Oct 28
            |✓ Around the World       Homework        Oct 28
        "#;
        pub(super) const FAILED_RUNS: &[&str] = &[
            "0:0-15:Text:B", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:32-46:Dim", "4:0-46:Dim", "5:0-46:Gold",
            "6:2-43:Gold", "7:2-8:Gold", "9:0-17:Dim", "11:0-44:Text", "11:46-46:Dim", "12:0-5:Text", "12:42-46:Dim",
            "14:0-46:Accent", "15:0-0:Accent", "15:16-31:Accent:B", "15:46-46:Accent", "16:0-46:Accent",
            "18:0-11:Dim", "19:0-0:Ok", "19:2-13:Text", "19:25-46:Dim", "20:0-0:Ok", "20:2-23:Text", "20:25-46:Dim",
            "21:0-0:Ok", "21:2-12:Text", "21:25-46:Dim", "22:0-0:Ok", "22:2-17:Text", "22:25-46:Dim",
        ];
        pub(super) const TAGS: &str = r#"
            |╭─ Edit tags ────────────────────────────────────────────────────╮
            |│ TITLE                                                          │
            |│ ╭────────────────────────────────────────────────────────────╮ │
            |│ │ One More Time▏                                             │ │
            |│ ╰────────────────────────────────────────────────────────────╯ │
            |│ ARTIST                        ALBUM                  YEAR      │
            |│ ╭──────────────────────────╮  ╭───────────────────╮  ╭───────╮ │
            |│ │ Daft Punk                │  │ Discovery         │  │ 2001  │ │
            |│ ╰──────────────────────────╯  ╰───────────────────╯  ╰───────╯ │
            |│                                                                │
            |│ From YouTube Music                                             │
            |│ Blank: the file gets the video's own title.                    │
            |│                                                                │
            |│                                                      [ Done ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const TAGS_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-11:Text:B", "0:13-65:Accent", "1:0-0:Accent", "1:2-6:Dim", "1:65-65:Accent",
            "2:0-65:Accent", "3:0-2:Accent", "3:4-17:Text", "3:63-65:Accent", "4:0-65:Accent", "5:0-0:Accent",
            "5:2-58:Dim", "5:65-65:Accent", "6:0-0:Accent", "6:2-63:Dim", "6:65-65:Accent", "7:0-0:Accent",
            "7:2-2:Dim", "7:4-12:Text", "7:29-32:Dim", "7:34-42:Text", "7:52-55:Dim", "7:57-60:Text", "7:63-63:Dim",
            "7:65-65:Accent", "8:0-0:Accent", "8:2-63:Dim", "8:65-65:Accent", "9:0-65:Accent", "10:0-0:Accent",
            "10:2-19:Dim", "10:65-65:Accent", "11:0-0:Accent", "11:2-44:Dim", "11:65-65:Accent", "12:0-65:Accent",
            "13:0-0:Accent", "13:55-62:Accent:B", "13:65-65:Accent", "14:0-65:Accent",
        ];
        pub(super) const FORMAT: &str = r#"
            |╭─ Format · MP3 ─────────────────────────────────────────────────╮
            |│ Best audio the video has, saved as                             │
            |│ (•) MP3     Plays everywhere                                   │
            |│ ( ) M4A     Good for Apple devices                             │
            |│ ( ) Opus    Usually YouTube's own audio, no re-encode          │
            |│ ( ) FLAC    Bigger, not better than the video's sound          │
            |│ ( ) WAV     Biggest, no cover art                              │
            |│                                                                │
            |│ Remembered for music.home.arpa                       [ Done ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const FORMAT_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-8:Text:B", "0:10-14:Dim", "0:16-65:Accent", "1:0-0:Accent", "1:2-35:Dim",
            "1:65-65:Accent", "2:0-4:Accent", "2:6-8:Text:B", "2:14-29:Dim", "2:65-65:Accent", "3:0-0:Accent",
            "3:2-4:Dim", "3:6-8:Text", "3:14-35:Dim", "3:65-65:Accent", "4:0-0:Accent", "4:2-4:Dim", "4:6-9:Text",
            "4:14-54:Dim", "4:65-65:Accent", "5:0-0:Accent", "5:2-4:Dim", "5:6-9:Text", "5:14-54:Dim",
            "5:65-65:Accent", "6:0-0:Accent", "6:2-4:Dim", "6:6-8:Text", "6:14-34:Dim", "6:65-65:Accent",
            "7:0-65:Accent", "8:0-0:Accent", "8:2-31:Dim", "8:55-62:Accent:B", "8:65-65:Accent", "9:0-65:Accent",
        ];
        pub(super) const N1: &str = r#"
            |Adding “One More Time”…
        "#;
        pub(super) const N1_RUNS: &[&str] = &[
            "0:0-22:Accent",
        ];
        pub(super) const N2: &str = r#"
            |Added “One More Time”
        "#;
        pub(super) const N2_RUNS: &[&str] = &[
            "0:0-20:Ok",
        ];
        pub(super) const N3: &str = r#"
            |Couldn't add “Around the Worl…
        "#;
        pub(super) const N3_RUNS: &[&str] = &[
            "0:0-29:Gold",
        ];
        pub(super) const N4: &str = r#"
            |Added “One More Time” · Daft Punk › Discovery
        "#;
        pub(super) const N4_RUNS: &[&str] = &[
            "0:0-44:Ok",
        ];
        pub(super) const N5: &str = r#"
            |Couldn't add “One More Time”
        "#;
        pub(super) const N5_RUNS: &[&str] = &[
            "0:0-27:Gold",
        ];
    }
}
