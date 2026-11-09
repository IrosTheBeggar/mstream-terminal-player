//! Add from YouTube in the GUI: the Settings doorway's room — a link
//! pasted in, the video looked up, its tags guessed and the folder it
//! lands in shown, one primary that adds it to a library — and the From
//! YouTube list the room's title-row word opens, where the adds this
//! player started are followed until they settle, beside the server's
//! record of what came in. The Files door and its window
//! ([`super::ytdl_here`]), and Where new songs land with its Choose a
//! folder ([`super::ytdl_where`]), share this module's tracker, its form
//! ([`Draft`], read through a [`Form`]) and its paste seam.
//!
//! Design: the "Add from YouTube — webapp & desktop" canvas's terminal
//! page (TermMain, TermRoomStates, TermList; frame ids below cite them) —
//! the calmer grammar: no labels but the tags', one ACCENT primary Enter
//! presses, every other control a DIM key word that also clicks and acts
//! only where it is drawn, GOLD `! ` lines, count-free. The rules are
//! [`crate::ytdl`]'s and the jobs [`crate::ytdl::jobs::Tracker`]'s; this
//! module puts them on the screen.
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
use crate::api::ytdl::{LOOKUP_BOUND, YtdlCallError, YtdlListing, YtdlMetadata};
use crate::config;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, tall_button, width};
use crate::tui::app::{Focus, MessageKind, Tab};
use crate::tui::worker::{ApiCmd, Event};
use crate::ytdl::jobs::{
    AddedRow, Answer, Call, CallId, Clock, Job, JobEvent, JobState, Note, Origin, Redo, SlotStore, StartOutcome,
    StartRequest, Store, Tracker, Transport, WallClock, Way, Word,
};
use crate::ytdl::{
    AddBlock, AddState, Choice, Failure, FolderCheck, Guess, GuessState, LayoutKind, Link, LinkKind, Listing,
    LookupState, Presets, Problem, Remembered, Surface, TagField, Tags, Target, Tier, Tone, Words, add_block,
    add_button_label, block_label, check_folders, choice_target, classify_lookup, failure_label, field_help,
    folder_target, format_help, format_name, format_options, guess, link_notes, lookup_state_of, name_check,
    parse_link, path_crumbs, record_format, remembered_choice, year_invalid,
};

use super::keep::{self, crumb_sep, fit_note};
use super::{Act, Gui, SettingsRoom, accent, bright_bold, put, sel, text_button};

/// A typed link is looked up once the typing rests this long.
const DEBOUNCE_MS: i64 = 400;
/// "Still looking — YouTube can be slow." after this long (B2).
const SLOW_MS: i64 = 8_000;
/// A paste is one line of at most this many characters.
const PASTE_MAX: usize = 4096;
/// The cells between key words on a row, and before a right-hand one.
const KEY_GAP: u16 = 3;
/// The From YouTube list's title column (TermList: 20 cells).
const TITLE_W: usize = 20;

// ── Acts ────────────────────────────────────────────────────────────────────

/// Everything a click on Add from YouTube's surfaces means.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum YtAct {
    /// The room's ◂ — back to the Settings rows (the second Esc's twin).
    Back,
    /// The title-row word (`l`): the From YouTube list.
    List,
    /// A click on the link card: the field takes the keys.
    Field,
    /// The card's `[X]`: the link cleared, the field holding the keys.
    Clear,
    /// A tag's row, clicked: it takes the keys.
    Tag(TagField),
    /// The room's key words, by what they do.
    Edit,
    Change,
    Format,
    Retry,
    Type,
    Use,
    Undo,
    Instead,
    Choose,
    /// The primary.
    Add,
    /// The format list: a format, or a click anywhere else.
    FormatPick(String),
    FormatClose,
    /// The From YouTube list.
    ListClose,
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
    ScrollTo(usize),
    /// The Files door and its window ([`super::ytdl_here`]).
    Here(super::ytdl_here::HereAct),
    /// Where new songs land and Choose a folder ([`super::ytdl_where`]).
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
    /// Where new songs land's own presets: the account's Discover
    /// destination (asked only where the server has Discover's plug-ins)
    /// and the libraries' templates.
    Presets { identity: String, discover: bool },
    /// One folder's names, for Choose a folder.
    Listing { seq: u64, directory: String },
}

/// What a thread sends back.
pub(crate) enum Reply {
    Lookup { seq: u64, result: Result<YtdlMetadata, YtdlCallError> },
    Folders { seq: u64, check: FolderCheck },
    Pinged { identity: String, ping: Option<Ping> },
    Presets { identity: String, presets: Presets },
    Listing { seq: u64, result: Result<YtdlListing, YtdlCallError> },
}

// ── The form ────────────────────────────────────────────────────────────────

/// Who has the form's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Keys {
    /// Nothing: the key words act.
    None,
    Link,
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
    /// An answer that came after "t type the details": offered with "u use
    /// them", never poured over what was typed (B3).
    pub(super) late: Option<Box<YtdlMetadata>>,
    /// The details are typed by hand.
    manual: bool,
    /// The four tags show as fields: "e edit tags", typed by hand, a
    /// lookup that couldn't read the video.
    pub(super) tags_open: bool,
    guess: Guess,
    gstate: GuessState,
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
    /// Where new songs land's "u use for this song": the choice this song
    /// goes by, not remembered.
    pub(super) choice: Option<Choice>,
    /// The server refused the add while the form was up (D3).
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
                    // A video the server couldn't read: the tags open,
                    // empty, the title holding the keys (B4).
                    self.manual = true;
                    self.tags_open = true;
                    self.keys = Keys::Tag(TagField::Title);
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

// ── The list window's own state ─────────────────────────────────────────────

/// One item of the From YouTube list: a job of this player's, or a song
/// the Added list holds.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Job(String),
    Song(AddedRow),
}

/// The From YouTube list while it is up.
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
    /// Where the last frame drew each row's ways: a format list hangs from
    /// Change format and retry's first cell.
    ways: Vec<(String, Way, Rect)>,
}

/// The format list hanging from its control: the room's `MP3 ▾` (rising,
/// over its bottom band), the Files window's, or a row's Change format and
/// retry.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FormatMenu {
    anchor: Rect,
    cursor: usize,
    /// The row it retries; `None`: the song on screen.
    job: Option<String>,
}

/// Add from YouTube's state on the Gui.
pub(crate) struct YtUi {
    /// The ping's facts for the session's server.
    facts: Option<Facts>,
    /// The jobs of the session's server and account.
    pub held: Option<Held>,
    /// The room's form.
    pub draft: Draft,
    /// Where new songs land, beyond the fixed presets: Discover's own
    /// destination and the libraries' templates, once read (Where new songs
    /// land asks for them).
    pub presets: Option<Presets>,
    list: Option<ListUi>,
    pub(super) formats: Option<FormatMenu>,
    /// The Files door, its window and the folder it re-lists.
    pub(super) here: super::ytdl_here::HereUi,
    /// Where new songs land or Choose a folder, while one is up.
    pub(super) place: Option<super::ytdl_where::Place>,
    /// Where the last frame drew `MP3 ▾`: the format list hangs from it.
    format_at: Option<Rect>,
    /// The key words the last frame drew, by letter: a key acts only
    /// where its word stood.
    pub(super) drawn: Vec<(char, YtAct)>,
    /// A press is out (the window's `[ Adding… ]`): the key words show
    /// where they stand but neither click nor light (C4).
    pub(super) quiet: bool,
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
            list: None,
            formats: None,
            here: super::ytdl_here::HereUi::default(),
            place: None,
            format_at: None,
            drawn: Vec::new(),
            quiet: false,
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

    pub(crate) fn jobs(&self) -> Option<&Jobs> {
        self.held.as_ref().map(|h| &h.jobs)
    }

    pub(super) fn jobs_mut(&mut self) -> Option<&mut Jobs> {
        self.held.as_mut().map(|h| &mut h.jobs)
    }

    /// Every form the threads' answers may be for, each knowing its own
    /// requests: the room's and the Files window's.
    fn drafts_mut(&mut self) -> impl Iterator<Item = &mut Draft> {
        [&mut self.draft, &mut self.here.draft].into_iter()
    }
}

// ── The two forms ───────────────────────────────────────────────────────────

/// Which form a reading or a key is for: the room's, or the Files
/// window's. Never both at once — the room stands in Settings, the window
/// over Files.
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

/// The room is what stands in Settings.
pub(super) fn in_room(gui: &Gui) -> bool {
    gui.in_settings_room(SettingsRoom::YouTube)
}

/// A surface of Add from YouTube's stands beneath its windows: the room,
/// or Files (whose door opens the window and whose `! failed` the list).
pub(super) fn surface_up(gui: &Gui) -> bool {
    in_room(gui) || super::ytdl_here::in_files(gui)
}

pub(crate) fn list_open(gui: &Gui) -> bool {
    list_held(gui) && !super::ytdl_where::is_open(gui)
}

/// The From YouTube list is up — or stands behind Choose a folder, which
/// took its place.
pub(super) fn list_held(gui: &Gui) -> bool {
    gui.ytdl.list.is_some() && surface_up(gui)
}

/// A window over the room or Files: the From YouTube list, the Files
/// window, Where new songs land, Choose a folder (what is beneath is
/// inert).
pub(crate) fn window_open(gui: &Gui) -> bool {
    list_open(gui) || super::ytdl_here::window_up(gui) || super::ytdl_where::is_open(gui)
}

/// Anything over the room or Files that owns the pointer and the keys: the
/// windows, the format list.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    window_open(gui) || (surface_up(gui) && gui.ytdl.formats.is_some())
}

/// The link field wants a paste: the room is up, or the Files window, and
/// nothing of another's stands over it.
pub(crate) fn wants_paste(gui: &Gui) -> bool {
    (in_room(gui) || super::ytdl_here::window_up(gui)) && !covered(gui)
}

/// A surface not Add from YouTube's is up over the room or Files — the
/// sign-in form a lapsed token opens, Add a server, the header's list, the
/// sheet, a dialog: a paste is its own, typed into its field as before.
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
}

/// The wheel is the YouTube windows' own: the list's rows, Choose a
/// folder's.
pub(crate) fn wheels(gui: &Gui) -> bool {
    list_open(gui) || super::ytdl_where::is_open(gui)
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

/// Open the room from Settings: the link field holds the keys (the boards'
/// deviation from a room that opens stowed — a paste is the way in).
pub(crate) fn open_room(gui: &mut Gui) {
    gui.active = super::SETTINGS_NAV;
    gui.cursor = None;
    gui.servers.drop_open = false;
    gui.settings_room = Some(SettingsRoom::YouTube);
    gui.ytdl.draft.keys = Keys::Link;
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
        // The window was over another server's folder; Where new songs
        // land and Choose a folder read another server's libraries, and
        // the format list its formats.
        super::ytdl_here::close(gui);
        gui.ytdl.place = None;
        gui.ytdl.formats = None;
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
        gui.ytdl.work.push(work);
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
    // session moved to a peer) goes back to the Settings rows: hidden,
    // never greyed. A connection that dropped closes nothing — the room
    // says it can't reach the server.
    if in_room(gui) && gui.app.connected && !available(gui) {
        leave_room(gui);
        changed = true;
    }
    ensure_jobs(gui);
    changed |= form_turn(gui);
    // The Files window's form and its folder, then Where new songs land's
    // and Choose a folder's: each closes once what it stood over went.
    changed |= super::ytdl_here::turn(gui);
    changed |= super::ytdl_where::turn(gui);
    let room = in_room(gui);
    // Left by any road, the overlays go with what they stood over.
    if !surface_up(gui) {
        if gui.ytdl.list.is_some() {
            close_list(gui);
        }
        gui.ytdl.formats = None;
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
            // Where new songs land checks its own place too.
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
    }
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
    if in_room(gui) && (gui.ytdl.draft.found() || gui.ytdl.draft.tags_open) {
        let target = room_target(gui);
        let ui = &mut gui.ytdl;
        if let Some(work) = ui.draft.folder_work(&target, now, &mut ui.seq) {
            send(gui, work);
            changed = true;
        }
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
                        // holding the keys for the next link (E1).
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

/// The choice the room's song goes by: Where new songs land's "u use for
/// this song", else the server's remembered one.
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
    /// that exists ("i save to … instead").
    pub(super) no_mkdir: Option<(String, Option<String>)>,
    /// The file line: a clash (GOLD, with `f change format`) or letters
    /// dropped (DIM).
    pub(super) file_line: Option<(Tone, String)>,
    /// Above the primary: a refusal (GOLD) or why it is off (DIM).
    pub(super) above: Option<(Tone, String)>,
    pub(super) refused_format: bool,
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
    /// A lookup that failed with a way to ask again (B5, D2), or none (D1).
    Failed { text: String, retry: bool, missing: bool },
    /// Found: the title, its facts, the clean-up and the source (T2, C2, C3).
    Found { title: String, facts: String, cleaned: Option<String>, source: Option<(Tone, String)> },
    /// The tags as fields (C1, B3, B4): a failure's words over them.
    Tags { failed: Option<String> },
}

fn room_view(gui: &Gui) -> RoomView {
    form_view(gui, Form::Room)
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
    let stage = match &draft.look {
        _ if draft.tags_open => {
            let failed = match &draft.look {
                Look::Failed { failure, .. } => Some(failure_label(failure, &words).text()),
                _ => None,
            };
            Stage::Tags { failed }
        }
        Look::Reading { .. } => Stage::Reading { slow: lookup == LookupState::Slow },
        Look::Failed { failure, .. } => Stage::Failed {
            text: failure_label(failure, &words).text(),
            retry: matches!(lookup, LookupState::TimedOut | LookupState::Unreachable),
            missing: lookup == LookupState::YtdlpMissing,
        },
        Look::Found { .. } => {
            let tags = draft.tags();
            let mut facts = Vec::new();
            if !tags.artist.trim().is_empty() {
                facts.push(tags.artist.trim().to_string());
            }
            facts.push(if tags.album.trim().is_empty() {
                t!("gui.ytdl.room.no_album").to_string()
            } else {
                tags.album.trim().to_string()
            });
            if !tags.year.trim().is_empty() {
                facts.push(tags.year.trim().to_string());
            }
            let view = draft.guess.view(&draft.gstate, draft.edited[1]);
            let cleaned = view.cleaned.then(|| draft.guess.cleaned_label().text());
            let source = draft.guess.source(&draft.gstate).line().map(|(tone, label)| (tone, label.text()));
            Stage::Found { title: tags.title.trim().to_string(), facts: facts.join(" · "), cleaned, source }
        }
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
    let tags = draft.tags();
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
    let shows_primary = match &stage {
        Stage::Found { .. } | Stage::Tags { .. } => true,
        Stage::Failed { missing, .. } => *missing,
        _ => false,
    };
    let primary = shows_primary.then(|| {
        let enabled = block == AddBlock::None && !starting;
        // The window adds into the folder on screen: Add here — past it
        // (a subfolder, a folder chosen) it adds to the library (C1).
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
            format!("{words}  ")
        };
        (label, enabled)
    });
    let refusal = draft.refusal.as_ref().filter(|f| !(f.problem == Problem::MkdirOff && mkdir_blocked));
    let refused_format = refusal.is_some_and(|f| matches!(f.problem, Problem::Exists | Problem::BadFormat));
    let above = if let Some(f) = refusal {
        Some((Tone::Refusal, failure_label(f, &words).text()))
    } else if shows_primary && !matches!(stage, Stage::Failed { .. }) {
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
        file_line,
        above,
        refused_format,
        primary,
        anyway: nc.add_anyway,
        block,
        request,
    }
}

/// The title-row word and the Settings description's: what the jobs have
/// to say, in its token. On the title row the room's own press, while it
/// is out, is the primary's to say (D5: `Adding…`, one ACCENT element).
fn news_word(gui: &Gui, history: bool) -> Option<(String, Style)> {
    let jobs = gui.ytdl.jobs()?;
    let mut summary = jobs.summary();
    let pressed = gui.ytdl.draft.starting.as_deref().and_then(|id| jobs.job(id));
    if history && pressed.is_some_and(|j| j.state.open() && !j.hidden) {
        summary.adding = summary.adding.saturating_sub(1);
    }
    let word = summary.word()?;
    Some(match word {
        Word::Adding => (t!("gui.ytdl.word.adding").to_string(), accent()),
        Word::Failed => (format!("! {}", t!("gui.ytdl.word.failed")), Style::default().fg(th().gold)),
        Word::Added => (format!("{} {}", keep::tick(), t!("gui.ytdl.word.added")), Style::default().fg(th().ok)),
        Word::History if history => (t!("gui.ytdl.word.history").to_string(), dim()),
        Word::History => return None,
    })
}

/// Settings › Add from YouTube's description while there is news: one word
/// (TermList S2–S4); none, the description stands.
pub(crate) fn settings_word(gui: &Gui) -> Option<(String, Style)> {
    news_word(gui, false)
}

// ── Drawing the room ────────────────────────────────────────────────────────

fn gold() -> Style {
    Style::default().fg(th().gold)
}

fn reading_bar() -> &'static str {
    if legacy_conhost() { "··········" } else { "▱▱▱▱▱▱▱▱▱▱" }
}

pub(super) fn chevron() -> &'static str {
    if legacy_conhost() { "v" } else { "▾" }
}

/// A GOLD line's rows: `! ` and the words wrapped to `room`, each row after
/// the first two cells in.
pub(super) fn gold_rows(text: &str, room: usize) -> Vec<String> {
    let lines = crate::kit::wrap_whole(text, room.saturating_sub(2).max(1));
    lines.into_iter().enumerate().map(|(i, l)| if i == 0 { format!("! {l}") } else { format!("  {l}") }).collect()
}

/// Crumbs that fit `room` cells, giving up whole folders from the left —
/// `… › Discovery` — and clipping the last one only when it alone is too
/// wide (T3).
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

/// A key word: DIM, BRIGHT BOLD under the pointer, its click registered and
/// its letter live for this frame. Returns its width in cells.
pub(super) fn key_word(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, key: &str, act: YtAct) -> u16 {
    let label = t!(key).to_string();
    key_label(frame, gui, x, y, &label, act)
}

/// [`key_word`] for words already read (one with a param in it).
pub(super) fn key_label(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, label: &str, act: YtAct) -> u16 {
    // The window's press is out: the word stands, DIM, and does nothing.
    if gui.ytdl.quiet {
        put(frame, x, y, label, dim());
        return width(label) as u16;
    }
    let letter = label.chars().next().unwrap_or(' ');
    gui.ytdl.drawn.push((letter, act.clone()));
    text_button(frame, gui, x, y, label, false, yt(act))
}

/// A key word ending one cell in from the right edge (`right_at`).
pub(super) fn key_word_right(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16, key: &str, act: YtAct) -> u16 {
    let x = super::right_at(content, &t!(key));
    key_word(frame, gui, x, y, key, act);
    x
}

/// The room (TermMain T2–T4, TermRoomStates): the title row and its word,
/// the link card, what the link came to, where the song goes, and the
/// format and the primary on the bottom band.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    gui.ytdl.drawn.clear();
    let view = room_view(gui);
    let x = content.x;
    let y = content.y;
    let over = list_open(gui);

    // Title row: the way back, the name, and the jobs' one word.
    let back = Rect { x, y, width: 1, height: 1 };
    let hover = gui.ui.hovers(back);
    put(frame, x, y, super::back_glyph(), if hover { bright_bold() } else { dim() });
    gui.ui.click(back, yt(YtAct::Back));
    // The press is out: ◂ is the one way left; everything else stands where
    // it is, neither clicking nor lighting (D5).
    gui.ytdl.quiet = gui.ytdl.draft.starting.is_some();
    let quiet = gui.ytdl.quiet;
    put(frame, x + 2, y, &t!("gui.ytdl.room.title"), Style::default().add_modifier(Modifier::BOLD));
    if let Some((word, style)) = news_word(gui, true) {
        let at = Rect { x: super::right_at(content, &word), y, width: width(&word) as u16, height: 1 };
        let style = if !quiet && gui.ui.hovers(at) { bright_bold() } else { style };
        put(frame, at.x, y, &word, style);
        if !quiet {
            gui.ui.click(at, yt(YtAct::List));
            gui.ytdl.drawn.push(('l', YtAct::List));
        }
    }

    // The bottom band: the primary, and the format beside it.
    let primary_at = view.primary.as_ref().map(|(label, _)| {
        let w = crate::kit::tall_width(label);
        Rect { x: content.right().saturating_sub(w), y: content.bottom().saturating_sub(3), width: w, height: 3 }
    });
    let floor = primary_at.map_or(content.bottom(), |p| p.y);
    let above_rows = above_rows(&view, content);
    let limit = floor.saturating_sub(above_rows.len() as u16 + u16::from(view.refused_format));

    // The link card: the field, its [X] while it holds anything — a row
    // higher when the body can't fit under it otherwise (its blank is the
    // last of the room's to give way).
    let keys = room_keys(gui);
    let rows = body_rows(gui, &view, content, keys);
    let (rows, tight) = give_way(rows, limit.saturating_sub(y + 5) as usize);
    let card = Rect { x, y: y + if tight { 1 } else { 2 }, width: content.width, height: 3 };
    let link = gui.ytdl.draft.link.clone();
    draw_link_card(frame, gui, card, &link, keys == Keys::Link && !over, (yt(YtAct::Field), yt(YtAct::Clear)));
    for (y, row) in (card.bottom()..limit).zip(rows) {
        draw_row(frame, gui, Form::Room, content, y, row);
    }

    // Over the primary: a refusal or why it's off.
    let mut ay = floor.saturating_sub(above_rows.len() as u16 + u16::from(view.refused_format));
    for (text, style, right) in above_rows {
        let at = if right { super::right_at(content, &text) } else { x };
        put(frame, at, ay, &text, style);
        ay += 1;
    }
    if view.refused_format && keys == Keys::None {
        key_word(frame, gui, x, ay, "gui.ytdl.key.format", YtAct::Format);
    }

    if let (Some((label, enabled)), Some(at)) = (view.primary.clone(), primary_at) {
        let missing = matches!(view.stage, Stage::Failed { .. });
        if !missing {
            draw_format_control(frame, gui, Form::Room, x, at.y + 1);
        }
        tall_button(frame, &mut gui.ui, at, &label, enabled, yt(YtAct::Add));
    }
    gui.ytdl.quiet = false;
}

/// Who has the room's keys this frame: the queue panel holding them, or a
/// window over the room, takes them from the form.
fn room_keys(gui: &Gui) -> Keys {
    form_keys(gui, Form::Room)
}

/// Who has a form's keys this frame. The Files window is modal: the queue
/// panel can't hold them while it is up.
pub(super) fn form_keys(gui: &Gui, form: Form) -> Keys {
    if form == Form::Room && gui.queue_open && gui.app.focus == Focus::Queue {
        return Keys::None;
    }
    draft(gui, form).keys
}

/// The 3-row link card (A1, T2; the Files window's too): ACCENT while the
/// field has the keys, the caret then the DIM placeholder when empty, `[X]`
/// to clear it; a click on the card is `field`, on `[X]` `clear`. A press
/// out, it stands DIM and does nothing.
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

/// `MP3 ▾` (T2, F3): the format as the control, its chevron DIM; BRIGHT
/// BOLD under the pointer; the format list rises from it. Returns its
/// width in cells.
pub(super) fn draw_format_control(frame: &mut Frame, gui: &mut Gui, form: Form, x: u16, y: u16) -> u16 {
    let name = format_name(&form_codec(gui, form));
    let rect = Rect { x, y, width: width(&name) as u16 + 2, height: 1 };
    gui.ytdl.format_at = Some(rect);
    if gui.ui.hovers(rect) && !gui.ytdl.quiet {
        put(frame, x, y, &format!("{name} {}", chevron()), bright_bold());
    } else {
        put(frame, x, y, &name, Style::default());
        put(frame, x + width(&name) as u16 + 1, y, chevron(), dim());
    }
    if !gui.ytdl.quiet {
        gui.ui.click(rect, yt(YtAct::Format));
        gui.ytdl.drawn.push(('f', YtAct::Format));
    }
    rect.width
}

/// One row of the room's body, as drawn.
#[derive(Debug, Clone)]
pub(super) enum Row {
    /// A blank that gives way when the room is short.
    Blank,
    Plain(String, Style),
    Gold(String),
    /// A line of the reading's words: the first beside the bar (and `t type
    /// the details` once it is slow), the rest under them.
    Reading { slow: bool, words: String, first: bool },
    /// A line of "Details found for this video": the first with `u use
    /// them` at the right.
    Late { words: String, first: bool },
    Title(String),
    /// The facts, `e edit tags` at the right.
    Facts(String),
    /// A line with a key word at the right (the clean-up's `u undo`).
    WithKey(String, Style, &'static str, YtAct),
    Tag(TagField),
    Help(Option<String>),
    /// "Files go to" and the place: its crumbs and whether a folder is new
    /// (`None`: nowhere to land), `d change` at the right.
    Place(Option<(Vec<String>, bool)>),
    /// Key words from the left, one at the right.
    Keys(Vec<(String, YtAct)>, Option<(&'static str, YtAct)>),
}

/// The body under the card, top to bottom, for the stage the form is in.
fn body_rows(gui: &Gui, view: &RoomView, content: Rect, keys: Keys) -> Vec<Row> {
    let room = usize::from(content.width.saturating_sub(1));
    let free = keys == Keys::None;
    let (mut rows, done) = stage_rows(gui, Form::Room, view, room, free);
    if done {
        return rows;
    }
    rows.push(Row::Blank);
    match &view.no_mkdir {
        // The folder can't be made: the GOLD line takes the place's row, its
        // ways on under it (C5).
        Some((line, deepest)) => {
            rows.extend(gold_rows(line, room).into_iter().map(Row::Gold));
            if free {
                let mut words = Vec::new();
                if let Some(dir) = deepest {
                    let place = path_crumbs(dir).join(crumb_sep());
                    words.push((t!("gui.ytdl.key.instead", place = place).to_string(), YtAct::Instead));
                }
                words.push((t!("gui.ytdl.key.choose").to_string(), YtAct::Choose));
                rows.extend(key_rows(words, ("gui.ytdl.key.change", YtAct::Change), room));
            }
        }
        None => {
            let place = view.target.is_valid().then(|| (view.target.crumbs(), view.new_folder));
            rows.push(Row::Place(place));
            if let Some((tone, text)) = &view.file_line {
                match tone {
                    Tone::Warn | Tone::Refusal => {
                        rows.extend(gold_rows(text, room).into_iter().map(Row::Gold));
                        if free && formats_offered(gui) > 1 {
                            rows.push(Row::Keys(vec![(t!("gui.ytdl.key.format").to_string(), YtAct::Format)], None));
                        }
                    }
                    _ => rows.extend(crate::kit::wrap_whole(text, room).into_iter().map(|l| Row::Plain(l, dim()))),
                }
            }
        }
    }
    rows
}

/// What the link came to, under the link, for the stage a form is in —
/// the room's and the Files window's alike: the link's own lines (or the
/// blank they take), then the reading, the failure, the song found or its
/// tags. True when the stage ends the body there (nothing found yet).
pub(super) fn stage_rows(gui: &Gui, form: Form, view: &RoomView, room: usize, free: bool) -> (Vec<Row>, bool) {
    let mut rows = Vec::new();
    // The link's own lines take the row under the card; none, it's blank.
    let draft = draft(gui, form);
    if draft.late.is_some() {
        // Beside `u use them`, wrapped rather than clipped.
        let key = if free { width(&t!("gui.ytdl.key.use")) + 2 } else { 1 };
        let lines = crate::kit::wrap_whole(&t!("gui.ytdl.room.late"), room.saturating_sub(key).max(1));
        rows.extend(lines.into_iter().enumerate().map(|(i, words)| Row::Late { words, first: i == 0 }));
    } else if view.notes.is_empty() {
        rows.push(Row::Blank);
    }
    for (tone, text) in &view.notes {
        match tone {
            Tone::Refusal | Tone::Warn => rows.extend(gold_rows(text, room).into_iter().map(Row::Gold)),
            Tone::Note | Tone::Ok => {
                rows.extend(crate::kit::wrap_whole(text, room).into_iter().map(|l| Row::Plain(l, dim())))
            }
        }
    }
    match &view.stage {
        Stage::Empty => return (rows, true),
        Stage::Reading { slow } => {
            // The words from one cell past the bar, ending two before `t type
            // the details`: a sentence wider than that goes on under them.
            let words = keep::ell(&t!(if *slow { "gui.ytdl.room.slow" } else { "gui.ytdl.room.reading" }));
            let key = if *slow && free { width(&t!("gui.ytdl.key.type")) + 2 } else { 0 };
            let at = width(reading_bar()) + 1;
            let lines = crate::kit::wrap_whole(&words, room.saturating_sub(at + key).max(1));
            rows.extend(lines.into_iter().enumerate().map(|(i, words)| Row::Reading { slow: *slow, words, first: i == 0 }));
            return (rows, true);
        }
        Stage::Failed { text, retry, .. } => {
            rows.extend(gold_rows(text, room).into_iter().map(Row::Gold));
            if *retry && free {
                rows.push(Row::Keys(vec![(t!("gui.ytdl.key.retry").to_string(), YtAct::Retry)], None));
            }
            return (rows, true);
        }
        Stage::Found { title, facts, cleaned, source } => {
            rows.push(Row::Title(title.clone()));
            rows.push(Row::Facts(facts.clone()));
            if let Some(cleaned) = cleaned {
                rows.push(Row::WithKey(cleaned.clone(), dim(), "gui.ytdl.key.undo", YtAct::Undo));
            }
            if let Some((tone, text)) = source {
                match tone {
                    Tone::Warn | Tone::Refusal => rows.extend(gold_rows(text, room).into_iter().map(Row::Gold)),
                    _ => rows.push(Row::Plain(text.clone(), dim())),
                }
            }
        }
        Stage::Tags { failed } => {
            if let Some(text) = failed {
                rows.extend(gold_rows(text, room).into_iter().map(Row::Gold));
                rows.push(Row::Blank);
            }
            rows.extend(TAGS.iter().map(|f| Row::Tag(*f)));
            rows.push(Row::Help(tag_help(gui, form)));
        }
    }
    (rows, false)
}

/// Key words laid from the left, `right` ending the first row: as many a
/// row as fit with [`KEY_GAP`] between them, the rest on the rows under it.
fn key_rows(words: Vec<(String, YtAct)>, right: (&'static str, YtAct), room: usize) -> Vec<Row> {
    let gap = usize::from(KEY_GAP);
    let mut budget = room.saturating_sub(width(&t!(right.0)) + 2);
    let mut rows = Vec::new();
    let mut line: Vec<(String, YtAct)> = Vec::new();
    let mut used = 0;
    for (label, act) in words {
        let w = width(&label);
        if !line.is_empty() && used + gap + w > budget {
            let right = if rows.is_empty() { Some(right.clone()) } else { None };
            rows.push(Row::Keys(std::mem::take(&mut line), right));
            budget = room;
            used = 0;
        }
        used += if line.is_empty() { w } else { gap + w };
        line.push((label, act));
    }
    let right = if rows.is_empty() { Some(right) } else { None };
    rows.push(Row::Keys(line, right));
    rows
}

/// The rows over the primary: a refusal's GOLD lines, or the DIM reason it
/// is off, right-aligned (D3, D4): (text, style, right-aligned).
fn above_rows(view: &RoomView, content: Rect) -> Vec<(String, Style, bool)> {
    let room = usize::from(content.width.saturating_sub(1));
    match &view.above {
        Some((Tone::Refusal | Tone::Warn, text)) => gold_rows(text, room).into_iter().map(|l| (l, gold(), false)).collect(),
        // Wrapped, not clipped: in the 47-cell room most languages' reason
        // takes two lines.
        Some((_, text)) => crate::kit::wrap_whole(text, room).into_iter().map(|l| (l, dim(), true)).collect(),
        None => Vec::new(),
    }
}

/// The formats the server takes, as the menu offers them.
fn formats_offered(gui: &Gui) -> usize {
    format_options(facts(gui).and_then(|f| f.formats.as_deref())).len()
}

/// The rows a short room gives up, in order: its blanks from the bottom
/// up, then the blank under the title row (true: the card rises into it),
/// then a tag's help line — never where the song goes.
fn give_way(mut rows: Vec<Row>, room: usize) -> (Vec<Row>, bool) {
    let mut i = rows.len();
    while rows.len() > room && i > 0 {
        i -= 1;
        if matches!(rows[i], Row::Blank) {
            rows.remove(i);
        }
    }
    let tight = rows.len() > room;
    if rows.len() > room + 1 {
        rows.retain(|row| !matches!(row, Row::Help(_)));
    }
    (rows, tight)
}

/// The focused tag's one help line; none while no tag has the keys.
fn tag_help(gui: &Gui, form: Form) -> Option<String> {
    let draft = draft(gui, form);
    let Keys::Tag(field) = form_keys(gui, form) else { return None };
    let i = tag_index(field);
    let album = draft.meta().and_then(|m| m.album.as_deref());
    Some(field_help(field, &draft.guess, &draft.gstate, draft.edited[i], draft.fields[i].value(), album).text())
}

/// The tags' label column: the widest label and a gap (C1: eight cells).
fn tag_label_width() -> u16 {
    let widest = TAGS.iter().map(|f| width(&t!(tag_key(*f)))).max().unwrap_or(6) as u16;
    (widest + 2).clamp(8, 18)
}

fn tag_key(field: TagField) -> &'static str {
    match field {
        TagField::Title => "gui.ytdl.tag.title",
        TagField::Artist => "gui.ytdl.tag.artist",
        TagField::Album => "gui.ytdl.tag.album",
        TagField::Year => "gui.ytdl.tag.year",
    }
}

/// The key words are drawn: nothing in a form has its keys, and nothing
/// stands over it.
pub(super) fn form_free(gui: &Gui, form: Form) -> bool {
    match form {
        Form::Room => room_keys(gui) == Keys::None && !list_open(gui),
        Form::Window => form_keys(gui, form) == Keys::None && super::ytdl_here::keys_free(gui),
    }
}

pub(super) fn draw_row(frame: &mut Frame, gui: &mut Gui, form: Form, content: Rect, y: u16, row: Row) {
    let x = content.x;
    let room = usize::from(content.width.saturating_sub(1));
    let free = form_free(gui, form);
    match row {
        Row::Blank => {}
        Row::Plain(text, style) => put(frame, x, y, &super::bar::clip(&text, room), style),
        Row::Gold(text) => put(frame, x, y, &super::bar::clip(&text, room), gold()),
        Row::Reading { slow, words, first } => {
            let at = x + width(reading_bar()) as u16 + 1;
            let end = if first && slow && free {
                key_word_right(frame, gui, content, y, "gui.ytdl.key.type", YtAct::Type).saturating_sub(2)
            } else {
                content.right().saturating_sub(1)
            };
            if first {
                put(frame, x, y, reading_bar(), dim());
            }
            put(frame, at, y, &super::bar::clip(&words, end.saturating_sub(at) as usize), accent());
        }
        Row::Late { words, first } => {
            let end = if first && free {
                key_word_right(frame, gui, content, y, "gui.ytdl.key.use", YtAct::Use)
            } else {
                content.right()
            };
            put(frame, x, y, &super::bar::clip(&words, end.saturating_sub(x + 2) as usize), dim());
        }
        Row::Title(text) => {
            put(frame, x, y, &super::bar::clip(&text, room), Style::default().add_modifier(Modifier::BOLD));
        }
        Row::Facts(text) => {
            let end = if free {
                key_word_right(frame, gui, content, y, "gui.ytdl.key.edit", YtAct::Edit).saturating_sub(2)
            } else {
                content.right().saturating_sub(1)
            };
            put(frame, x, y, &super::bar::clip(&text, end.saturating_sub(x) as usize), dim());
        }
        Row::WithKey(text, style, key, act) => {
            let end = if free { key_word_right(frame, gui, content, y, key, act).saturating_sub(2) } else { content.right() };
            put(frame, x, y, &super::bar::clip(&text, end.saturating_sub(x) as usize), style);
        }
        Row::Tag(field) => draw_tag(frame, gui, form, content, y, field),
        Row::Help(text) => {
            if let Some(text) = text {
                put(frame, x, y, &super::bar::clip(&text, room), dim());
            }
        }
        Row::Place(place) => {
            let lead = t!("gui.ytdl.room.files_go_to").to_string();
            put(frame, x, y, &lead, dim());
            let px = x + width(&lead) as u16 + 1;
            let end = if free {
                key_word_right(frame, gui, content, y, "gui.ytdl.key.change", YtAct::Change).saturating_sub(2)
            } else {
                content.right().saturating_sub(1)
            };
            match place {
                Some((crumbs, new)) => {
                    draw_place(frame, px, y, &crumbs, new, end.saturating_sub(px) as usize);
                }
                None => put(frame, px, y, super::discover::dash(), dim()),
            }
        }
        Row::Keys(words, right) => {
            let end = match right {
                Some((key, act)) if free => key_word_right(frame, gui, content, y, key, act).saturating_sub(2),
                _ => content.right().saturating_sub(1),
            };
            let mut at = x;
            for (label, act) in words {
                let w = width(&label) as u16;
                if !free || at + w > end {
                    break;
                }
                key_label(frame, gui, at, y, &label, act);
                at += w + KEY_GAP;
            }
        }
    }
}

/// A tag's row (C1, B3): its label in the label column — ACCENT BOLD while
/// it has the keys — and its value, the caret in it then.
fn draw_tag(frame: &mut Frame, gui: &mut Gui, form: Form, content: Rect, y: u16, field: TagField) {
    let lw = tag_label_width();
    let rect = Rect { x: content.x, y, width: content.width, height: 1 };
    let focused = form_keys(gui, form) == Keys::Tag(field) && !list_open(gui);
    let hover = gui.ui.hovers(rect);
    let style = match (focused, hover) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => dim(),
    };
    put(frame, content.x, y, &t!(tag_key(field)), style);
    let i = tag_index(field);
    let value = draft(gui, form).fields[i].value().to_string();
    let cursor = draft(gui, form).fields[i].cursor();
    let vx = content.x + lw;
    let room = content.width.saturating_sub(lw + 1);
    if focused {
        super::text_field(frame, &mut gui.ui, vx, y, &value, cursor, room, Style::default());
    } else {
        let style = if hover { Style::default().fg(th().bright) } else { Style::default() };
        put(frame, vx, y, &super::bar::clip(&value, room as usize), style);
    }
    gui.ui.click(rect, yt(YtAct::Tag(field)));
}

// ── The overlays: the format list, the From YouTube list ────────────────────

/// Draw what stands over the room or Files: Where new songs land or Choose
/// a folder (in the place of the window it was opened from), the From
/// YouTube list, the Files window — then the format list (after them, so
/// its rects win the pointer).
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if !surface_up(gui) {
        return;
    }
    if super::ytdl_where::is_open(gui) {
        super::ytdl_where::draw(frame, gui, area);
    } else if gui.ytdl.list.is_some() {
        draw_list(frame, gui, area);
    } else if super::ytdl_here::window_up(gui) {
        super::ytdl_here::draw_window(frame, gui, area);
    }
    if gui.ytdl.formats.is_some() {
        draw_formats(frame, gui, area);
    }
}

/// The format list (the kit's Dropdown, risen): the formats the server
/// takes, each with its one line, the current one wearing `•`, the cursor
/// the slab; a click anywhere else closes it.
fn draw_formats(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(menu) = gui.ytdl.formats.clone() else { return };
    let options = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
    let current = match &menu.job {
        Some(id) => gui.ytdl.jobs().and_then(|j| j.job(id)).map(|j| j.codec.clone()).unwrap_or_default(),
        None => form_codec(gui, active_form(gui)),
    };
    gui.ui.click(area, yt(YtAct::FormatClose));
    let names: Vec<String> = options.iter().map(|f| format_name(f)).collect();
    let helps: Vec<String> = options.iter().map(|f| format_help(f).map(|l| l.text()).unwrap_or_default()).collect();
    let name_w = names.iter().map(|n| width(n)).max().unwrap_or(3);
    let help_w = helps.iter().map(|h| width(h)).max().unwrap_or(0);
    let w = (5 + name_w + if help_w > 0 { 2 + help_w } else { 0 } + 1) as u16;
    let w = w.min(area.width.saturating_sub(2));
    let h = options.len() as u16 + 2;
    let x = menu.anchor.x.saturating_sub(1).min(area.right().saturating_sub(w + 1));
    // Down from a row's word where it fits above the bar, else up from the
    // control — the room's sits on its bottom band.
    let below = menu.anchor.bottom() + h <= area.bottom().saturating_sub(super::bar::BAR_ROWS);
    let y = if menu.job.is_some() && below { menu.anchor.bottom() } else { menu.anchor.y.saturating_sub(h) };
    let rect = Rect { x, y, width: w, height: h };
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    for (i, codec) in options.iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let on = i == menu.cursor;
        let hover = !on && gui.ui.hovers(row);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let mark = if *codec == current.as_str() { super::bullet_glyph() } else { " " };
        put(frame, row.x + 1, row.y, mark, if on { sel() } else { accent() });
        let style = match (on, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, row.x + 3, row.y, &names[i], style);
        if !helps[i].is_empty() {
            let hx = row.x + 3 + name_w as u16 + 2;
            let room = row.right().saturating_sub(hx + 1) as usize;
            put(frame, hx, row.y, &super::bar::clip(&helps[i], room), if on { sel() } else { dim() });
        }
        gui.ui.click(row, yt(YtAct::FormatPick(codec.to_string())));
    }
}

fn open_formats(gui: &mut Gui, anchor: Rect, job: Option<String>) {
    let options = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
    let current = match &job {
        Some(id) => gui.ytdl.jobs().and_then(|j| j.job(id)).map(|j| j.codec.clone()).unwrap_or_default(),
        None => form_codec(gui, active_form(gui)),
    };
    let cursor = options.iter().position(|o| *o == current.as_str()).unwrap_or(0);
    gui.ytdl.formats = Some(FormatMenu { anchor, cursor, job });
}

/// The format list hung from a form's `MP3 ▾` goes with the form (a row's
/// stays with the list).
pub(super) fn drop_form_formats(gui: &mut Gui) {
    if gui.ytdl.formats.as_ref().is_some_and(|m| m.job.is_none()) {
        gui.ytdl.formats = None;
    }
}

/// The format chosen: the song on screen (the room's or the Files
/// window's) goes as it and the server remembers it (only the format — the
/// rest of its choices stay as they were); a row's Change format and retry
/// adds again in it.
fn pick_format(gui: &mut Gui, codec: String) {
    let Some(menu) = gui.ytdl.formats.take() else { return };
    match menu.job {
        Some(id) => {
            let next = gui.ytdl.jobs_mut().and_then(|jobs| jobs.retry(&id, Redo { codec: Some(codec), ..Redo::default() }));
            follow_retry(gui, &id, next);
        }
        None => {
            let id = identity(gui);
            let chosen = codec.clone();
            super::servers::update_config(gui, |c| {
                let mut prefs = config::ytdl_prefs(c, &id);
                prefs.format = Some(chosen);
                config::set_ytdl_prefs(c, &id, &prefs);
            });
            let form = active_form(gui);
            let draft = draft_mut(gui, form);
            draft.codec = Some(codec);
            // A refused format or a taken name: another format is the way on.
            if draft.refusal.as_ref().is_some_and(|f| matches!(f.problem, Problem::Exists | Problem::BadFormat)) {
                draft.refusal = None;
            }
        }
    }
}

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
    let formats: Vec<&str> = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
    let ways: Vec<(String, Way)> = job
        .ways(Some(&formats))
        .into_iter()
        // A row not sure of its add offers the check alone (L2); x
        // dismisses it all the same.
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
    let lines = text.map(|t| crate::kit::wrap_whole(&t, room)).unwrap_or_default();
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

/// How a way's words and the detail share the row: the ways on the
/// detail's last line where both fit three cells apart, else a row of
/// their own.
fn ways_width(ways: &[(String, Way)]) -> usize {
    let sep = width(" · ");
    ways.iter().map(|(w, _)| width(w)).sum::<usize>() + sep * ways.len().saturating_sub(1)
}

/// An item's rows in the list.
fn item_rows(gui: &Gui, item: &Item, room: usize) -> usize {
    match item {
        Item::Song(_) => 1,
        Item::Job(id) => {
            let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)) else { return 0 };
            if job.state.open() {
                return 1;
            }
            let (lines, ways) = job_detail(gui, job, room - 3);
            let shared = lines.last().is_some_and(|l| 3 + width(l) + 3 + ways_width(&ways) <= room) || ways.is_empty();
            1 + lines.len() + usize::from(!shared)
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
fn ago(gui: &mut Gui, then: i64, now: i64) -> String {
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

/// The From YouTube list (TermList L1–L4, F1): Copies' shape and place, 76
/// wide; its title on the top border and its way out on the bottom; the
/// open adds, those that need a look with their ways on, then the songs
/// that came in.
fn draw_list(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(jobs) = gui.ytdl.jobs() else { return };
    let items = list_items(jobs);
    let more = jobs.can_show_earlier();
    let going = !jobs.adding().is_empty();
    let room = super::content_rect(area.width, area.height, gui.queue_open, gui.footer());
    let w = keep::KEEP_W;
    let text = usize::from(w.saturating_sub(4));
    let heights: Vec<usize> = items.iter().map(|i| item_rows(gui, i, text)).collect();
    let tail = if more { 2 } else { 0 };
    let total: usize = heights.iter().sum::<usize>() + tail;
    let (rect, band) = keep::window_rect(area, room, w, 2 + total.max(1) as u16);
    let fits = usize::from(rect.height.saturating_sub(2));
    let list = gui.ytdl.list.get_or_insert_with(ListUi::default);
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
    list.ways.clear();
    if let Some(held) = list.held {
        list.held = Some(held.min(items.len().saturating_sub(1)));
    }
    // The first item: the held one shown, and as many after it as fit.
    let mut first = list.view.scroll.min(items.len().saturating_sub(1));
    if let Some(held) = list.held.filter(|_| list.view.held) {
        first = first.min(held);
        while first < held && heights[first..=held].iter().sum::<usize>() > fits.saturating_sub(tail) {
            first += 1;
        }
    }
    list.view.scroll = first;
    let held = list.held.filter(|_| list.view.held);
    let menu = list.menu;

    let (inner, _own) = keep::open_window(frame, gui, area, rect, band);
    let footer = if going { t!("gui.ytdl.list.esc_going") } else { t!("gui.discover.keep.esc_close") };
    keep::draw_borders(frame, gui, rect, &t!("gui.ytdl.list.title"), &footer, yt(YtAct::ListClose));
    let x = inner.x + 1;
    let end = inner.right().saturating_sub(1);
    let bottom = inner.bottom().saturating_sub(tail as u16);
    let mut y = inner.y;
    let now = gui.ytdl.now();
    let mut menu_at = None;
    for (i, item) in items.iter().enumerate().skip(first) {
        let h = heights[i] as u16;
        if y + h > bottom {
            break;
        }
        let rows = Rect { x: inner.x, y, width: inner.width, height: h };
        let on = held == Some(i);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rows);
        }
        match item {
            Item::Job(id) => draw_job(frame, gui, id, (x, y, end), text, on, now),
            Item::Song(song) => {
                let at = draw_song(frame, gui, song, i, (x, y, end), on, now);
                if menu.is_some_and(|(m, _)| m == i) {
                    menu_at = Some(at);
                }
            }
        }
        y += h;
    }
    if more {
        let words = t!("gui.ytdl.list.show_earlier").to_string();
        let sx = end.saturating_sub(width(&words) as u16);
        text_button(frame, gui, sx, inner.bottom().saturating_sub(1), &words, false, yt(YtAct::ShowEarlier));
    }
    // More items than fit: the kit's bar on the right border, in items.
    let visible = (first..items.len()).scan(0usize, |used, i| {
        *used += heights[i];
        (*used <= fits.saturating_sub(tail)).then_some(())
    });
    let visible = visible.count();
    if visible < items.len() {
        let bar = Rect { x: rect.right().saturating_sub(1), y: inner.y, width: 1, height: inner.height };
        crate::kit::scroll_items(
            frame,
            &mut gui.ui,
            bar,
            items.len(),
            visible,
            first,
            yt(YtAct::ScrollBy(-1)),
            yt(YtAct::ScrollBy(1)),
            |first| Act::Yt(YtAct::ScrollTo(first)),
        );
    }
    if let (Some((_, cursor)), Some(anchor)) = (menu, menu_at) {
        draw_song_menu(frame, gui, area, anchor, cursor);
    }
}

/// A job's rows: its mark, title and place, and where it stands at the
/// right; under a failure the words and its ways on.
fn draw_job(frame: &mut Frame, gui: &mut Gui, id: &str, (x, y, end): (u16, u16, u16), text: usize, on: bool, now: i64) {
    let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)).cloned() else { return };
    let waiting = gui.ytdl.jobs().is_some_and(|j| j.waiting());
    let base = |style: Style| if on { sel() } else { style };
    let (glyph, mark_style) = mark(&job);
    put(frame, x, y, glyph, base(mark_style));
    put(frame, x + 2, y, &super::bar::clip(&job.shown_title(), TITLE_W), base(Style::default()));
    // Where it stands, at the right.
    let mut right: Vec<(String, Style)> = Vec::new();
    match job.state {
        _ if job.state.open() && waiting => right.push((t!("gui.ytdl.list.waiting").to_string(), dim())),
        JobState::Adding => {
            right.push((t!("gui.ytdl.list.adding").to_string(), accent()));
            let since = job.sent_at.unwrap_or(job.created_at);
            let minutes = (now - since).max(0) / 60_000;
            if minutes > 0 {
                right.push((format!(" · {}", t!("gui.ytdl.list.minutes", count = minutes)), dim()));
            }
            right.push((format!(" · {}", format_name(&job.codec)), dim()));
        }
        JobState::Starting | JobState::StartUnsure => right.push((keep::ell(&t!("gui.ytdl.list.starting")), dim())),
        JobState::NotSureStarted => right.push((t!("gui.ytdl.list.not_sure_started").to_string(), dim())),
        JobState::NotSureFinished => right.push((t!("gui.ytdl.list.not_sure_finished").to_string(), dim())),
        JobState::Added => {}
        _ => right.push((t!("gui.ytdl.list.failed").to_string(), gold())),
    }
    let right_w: usize = right.iter().map(|(t, _)| width(t)).sum();
    let mut rx = end.saturating_sub(right_w as u16);
    for (words, style) in &right {
        put(frame, rx, y, words, base(*style));
        rx += width(words) as u16;
    }
    let px = x + 2 + TITLE_W as u16 + 2;
    let room = end.saturating_sub(right_w as u16 + 3).saturating_sub(px) as usize;
    let crumbs = path_crumbs(&job.folder());
    let (clipped, crumbs) = fit_crumbs(&crumbs, 0, room);
    let mut place = crumbs.join(crumb_sep());
    if clipped {
        place = format!("{}{}{place}", keep::ell("…"), crumb_sep());
    }
    put(frame, px, y, &place, base(dim()));
    if job.state.open() {
        return;
    }
    // The words, then the ways on — sharing the last line where both fit.
    let (lines, ways) = job_detail(gui, &job, text - 3);
    let mut ly = y + 1;
    for line in &lines {
        put(frame, x + 2, ly, &super::bar::clip(line, text - 3), base(dim()));
        ly += 1;
    }
    let shared = lines.last().is_some_and(|l| 3 + width(l) + 3 + ways_width(&ways) <= text);
    let wy = if shared { ly - 1 } else { ly };
    let mut wx = end.saturating_sub(ways_width(&ways) as u16);
    for (i, (words, way)) in ways.iter().enumerate() {
        if i > 0 {
            put(frame, wx, wy, " · ", base(dim()));
            wx += width(" · ") as u16;
        }
        let rect = Rect { x: wx, y: wy, width: width(words) as u16, height: 1 };
        let style = if gui.ui.hovers(rect) { bright_bold() } else { base(accent()) };
        put(frame, wx, wy, words, style);
        gui.ui.click(rect, yt(YtAct::Way(job.id.clone(), *way)));
        if let Some(list) = gui.ytdl.list.as_mut() {
            list.ways.push((job.id.clone(), *way, rect));
        }
        wx += rect.width;
    }
}

/// A song's row: ✓, the title, the place, its format and when — or, under
/// the pointer, its verbs (L3). Returns where its list hangs from.
fn draw_song(
    frame: &mut Frame,
    gui: &mut Gui,
    song: &AddedRow,
    i: usize,
    (x, y, end): (u16, u16, u16),
    on: bool,
    now: i64,
) -> Rect {
    let row = Rect { x: x - 1, y, width: end + 1 - x, height: 1 };
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
    put(frame, x + 2, y, &super::bar::clip(&song.title, TITLE_W), title_style);
    let verbs = verb_glyphs();
    let verbs_w = (verbs.iter().map(|v| width(v)).sum::<usize>() + verbs.len() - 1) as u16;
    let mut menu_anchor = Rect { x: end.saturating_sub(verbs_w), y, width: verbs_w, height: 1 };
    let right_w = if hover || (on && playable) {
        // The hover verbs: play, add to the queue, the song's list.
        let mut vx = end.saturating_sub(verbs_w);
        for (k, glyph) in verbs.iter().enumerate() {
            let rect = Rect { x: vx, y, width: width(glyph) as u16, height: 1 };
            let style = if gui.ui.hovers(rect) { bright_bold() } else { base(dim()) };
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
        verbs_w as usize
    } else {
        let words = if song.present {
            let format = song
                .file_path
                .as_deref()
                .and_then(record_format)
                .or_else(|| song.ext.as_deref().map(format_name))
                .unwrap_or_default();
            let when = song.at.map(|at| ago(gui, at, now)).unwrap_or_default();
            [format, when].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" · ")
        } else {
            t!("gui.ytdl.list.file_missing").to_string()
        };
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, base(dim()));
        width(&words)
    };
    let px = x + 2 + TITLE_W as u16 + 2;
    let room = end.saturating_sub(right_w as u16 + 3).saturating_sub(px) as usize;
    let (clipped, crumbs) = fit_crumbs(&path_crumbs(&song.folder), 0, room);
    let mut place = crumbs.join(crumb_sep());
    if clipped {
        place = format!("{}{}{place}", keep::ell("…"), crumb_sep());
    }
    put(frame, px, y, &place, base(dim()));
    menu_anchor
}

/// A song's list (L4): Add to queue `a`, Show in folder — hung under the
/// row from its [⋯], right edges level.
fn draw_song_menu(frame: &mut Frame, gui: &mut Gui, area: Rect, anchor: Rect, cursor: Option<usize>) {
    let rows = [(t!("gui.ytdl.list.add_to_queue").to_string(), "a"), (t!("gui.ytdl.list.show_in_folder").to_string(), "")];
    let inner_w =
        rows.iter().map(|(label, key)| width(label) + if key.is_empty() { 0 } else { 2 + width(key) }).max().unwrap_or(10) + 2;
    let w = inner_w as u16 + 2;
    let h = rows.len() as u16 + 2;
    gui.ui.click(area, yt(YtAct::MenuClose));
    let x = anchor.right().saturating_sub(w);
    let rect = Rect { x, y: anchor.bottom(), width: w, height: h };
    gui.ui.overlay(rect);
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

/// Open the From YouTube list (the room's word, the Files door's `!
/// failed`): what it shows is read again, and seen.
pub(super) fn open_list(gui: &mut Gui) {
    if gui.ytdl.jobs().is_none() {
        return;
    }
    gui.ytdl.list = Some(ListUi::default());
    gui.ytdl.formats = None;
    if let Some(jobs) = gui.ytdl.jobs_mut() {
        jobs.opened();
        jobs.watch(true);
    }
}

fn close_list(gui: &mut Gui) {
    gui.ytdl.list = None;
    if let Some(jobs) = gui.ytdl.jobs_mut() {
        jobs.watch(false);
    }
}

fn song_at(gui: &Gui, i: usize) -> Option<AddedRow> {
    match gui.ytdl.list.as_ref()?.items.get(i)? {
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
    close_list(gui);
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

fn way(gui: &mut Gui, id: String, way: Way, anchor: Option<Rect>) {
    match way {
        Way::Retry => {
            let next = gui.ytdl.jobs_mut().and_then(|jobs| jobs.retry(&id, Redo::default()));
            follow_retry(gui, &id, next);
        }
        Way::ChangeFormat => {
            let anchor = anchor.unwrap_or_default();
            open_formats(gui, anchor, Some(id));
        }
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
        // Choose a folder takes the list's place; the row adds again into
        // the folder used, and the list comes back.
        Way::ChooseFolder => super::ytdl_where::open_choose_for_job(gui, &id),
    }
}

/// A row added again is a new job, listed with the open ones: the list's
/// cursor goes with it (the next frame seats it where the row now is).
pub(super) fn follow_retry(gui: &mut Gui, old: &str, next: Option<String>) {
    let (Some(next), Some(list)) = (next, gui.ytdl.list.as_mut()) else { return };
    for item in &mut list.items {
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

/// Where a job row's word for `way` was drawn, for a list hung from it.
fn way_anchor(gui: &Gui, id: &str, way: Way) -> Option<Rect> {
    let list = gui.ytdl.list.as_ref()?;
    list.ways.iter().find(|(j, w, _)| j == id && *w == way).map(|(_, _, rect)| *rect)
}

pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if super::ytdl_where::is_open(gui) {
        super::ytdl_where::wheel(gui, delta);
        return;
    }
    gui.act(yt(YtAct::ScrollBy(delta)));
}

// ── Acts ────────────────────────────────────────────────────────────────────

fn leave_room(gui: &mut Gui) {
    gui.settings_room = None;
    gui.ytdl.formats = None;
    close_list(gui);
    if gui.ytdl.draft.keys != Keys::None {
        gui.ytdl.draft.keys = Keys::Link;
    }
}

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
}

/// Handle Add from YouTube's acts. Returns true when the act was its own.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Yt(act) = act else { return false };
    let now = gui.ytdl.now();
    // The form's own controls act on the form on screen: the room's, or
    // the Files window's over Files.
    let form = active_form(gui);
    // Its press out, they wait for the server with it — the way back is
    // the one left (D5, C4).
    let waiting = draft(gui, form).starting.is_some() && (form == Form::Window || in_room(gui));
    let controls = matches!(
        act,
        YtAct::List
            | YtAct::Edit
            | YtAct::Type
            | YtAct::Use
            | YtAct::Undo
            | YtAct::Retry
            | YtAct::Instead
            | YtAct::Change
            | YtAct::Choose
            | YtAct::Format
    );
    if waiting && controls {
        return true;
    }
    match act.clone() {
        YtAct::Back => leave_room(gui),
        YtAct::List => open_list(gui),
        YtAct::Field => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() {
                draft.keys = Keys::Link;
            }
            super::ytdl_here::keys_taken(gui);
        }
        YtAct::Clear => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() {
                draft.link = Input::default();
                draft.keys = Keys::Link;
                draft.link_changed(now);
            }
            super::ytdl_here::keys_taken(gui);
        }
        YtAct::Tag(field) => {
            let draft = draft_mut(gui, form);
            if draft.starting.is_none() {
                draft.keys = Keys::Tag(field);
            }
            super::ytdl_here::keys_taken(gui);
        }
        YtAct::Edit => {
            let draft = draft_mut(gui, form);
            draft.tags_open = true;
            draft.keys = Keys::Tag(TagField::Title);
            super::ytdl_here::keys_taken(gui);
        }
        YtAct::Type => {
            let draft = draft_mut(gui, form);
            draft.manual = true;
            draft.tags_open = true;
            draft.keys = Keys::Tag(TagField::Title);
            super::ytdl_here::keys_taken(gui);
        }
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
        // Where new songs land over the room; Choose a folder in the place
        // of what it was opened from.
        YtAct::Change => super::ytdl_where::open_where(gui),
        YtAct::Choose => super::ytdl_where::open_choose(gui, form),
        YtAct::Format => {
            if gui.ytdl.formats.is_some() {
                gui.ytdl.formats = None;
            } else {
                // From where `MP3 ▾` stood; before any frame drew it, the
                // room's bottom band.
                let anchor = gui.ytdl.format_at.unwrap_or_else(|| {
                    let content = gui.room_rect();
                    Rect { x: content.x, y: content.bottom().saturating_sub(2), width: 5, height: 1 }
                });
                open_formats(gui, anchor, None);
            }
        }
        YtAct::FormatPick(codec) => pick_format(gui, codec),
        YtAct::FormatClose => gui.ytdl.formats = None,
        YtAct::Add => press(gui),
        YtAct::ListClose => close_list(gui),
        YtAct::Play(i) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.menu = None;
            }
            play_song(gui, i, true);
        }
        YtAct::Queue(i) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.menu = None;
            }
            play_song(gui, i, false);
        }
        YtAct::More(i) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.menu = Some((i, None));
            }
        }
        YtAct::Way(id, w) => {
            let anchor = way_anchor(gui, &id, w);
            way(gui, id, w, anchor);
        }
        YtAct::MenuPick(k) => {
            let Some((i, _)) = gui.ytdl.list.as_mut().and_then(|l| l.menu.take()) else { return true };
            if k == 0 {
                play_song(gui, i, false);
            } else {
                show_in_folder(gui, i);
            }
        }
        YtAct::MenuClose => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.menu = None;
            }
        }
        YtAct::ShowEarlier => {
            if let Some(jobs) = gui.ytdl.jobs_mut() {
                jobs.show_earlier();
            }
        }
        YtAct::ScrollBy(delta) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                let last = list.items.len().saturating_sub(1);
                list.view.scroll = (list.view.scroll as i64 + i64::from(delta)).clamp(0, last as i64) as usize;
                list.view.held = false;
            }
        }
        YtAct::ScrollTo(first) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.view.scroll = first;
                list.view.held = false;
            }
        }
        YtAct::Here(act) => super::ytdl_here::act(gui, act),
        YtAct::Place(act) => super::ytdl_where::act(gui, act),
    }
    true
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The letters the room's key words use: their capitals are the room's
/// too (Shift or Caps Lock on `d`, `t`, `a` would open Auto DJ, Stats, or
/// toggle the DJ).
const ROOM_LETTERS: &[char] = &['a', 'c', 'd', 'e', 'f', 'i', 'l', 't', 'u'];

/// The room's keys, and its windows' (which outrank everything over the
/// room or Files): Where new songs land or Choose a folder, the format
/// list, the From YouTube list, the Files window. `Some(quit)` when the
/// key was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !surface_up(gui) {
        return None;
    }
    if super::ytdl_where::is_open(gui) {
        return Some(super::ytdl_where::handle_key(gui, key));
    }
    if gui.ytdl.formats.is_some() {
        formats_key(gui, key);
        return Some(false);
    }
    if gui.ytdl.list.is_some() {
        return Some(list_key(gui, key));
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
    // The press is out: every key but Esc waits for the server (D5). The
    // arrows are the room's throughout: the Settings rows stand hidden
    // behind it, and a cursor walked there would have ← → and Space act on
    // a row no one sees.
    if gui.ytdl.draft.starting.is_some() {
        return match key.code {
            KeyCode::Esc => Some(gui.act(yt(YtAct::Back))),
            KeyCode::Char(_) if !ctrl => Some(false),
            KeyCode::Up | KeyCode::Down => Some(false),
            _ => None,
        };
    }
    match gui.ytdl.draft.keys {
        Keys::Link => match key.code {
            KeyCode::Esc => gui.ytdl.draft.keys = Keys::None,
            KeyCode::Enter => link_enter(gui, Form::Room),
            KeyCode::Tab | KeyCode::BackTab => return None,
            KeyCode::Up | KeyCode::Down => {}
            _ => link_typed(gui, Form::Room, key, now),
        },
        Keys::Tag(field) => match key.code {
            KeyCode::Esc => gui.ytdl.draft.keys = Keys::None,
            KeyCode::Enter => press(gui),
            _ => tag_typed(gui, Form::Room, field, key),
        },
        Keys::None => match key.code {
            KeyCode::Esc => return Some(gui.act(yt(YtAct::Back))),
            KeyCode::Enter => {
                if room_view(gui).primary.is_some_and(|(_, on)| on) {
                    press(gui);
                } else if gui.ytdl.draft.parsed.kind == LinkKind::Video && matches!(gui.ytdl.draft.look, Look::Idle) {
                    ask(gui);
                }
            }
            // ↓ picks a tag where the tags show (B3).
            KeyCode::Down if gui.ytdl.draft.tags_open => gui.ytdl.draft.keys = Keys::Tag(TagField::Title),
            KeyCode::Up | KeyCode::Down => {}
            KeyCode::Char(c) if !ctrl => {
                let lower = c.to_ascii_lowercase();
                let drawn = gui.ytdl.drawn.iter().find(|(letter, _)| *letter == lower).map(|(_, act)| act.clone());
                match drawn {
                    Some(act) => return Some(gui.act(yt(act))),
                    // A capital of the room's own letters is the room's,
                    // drawn or not: it never reaches the shell's globals.
                    None if c.is_ascii_uppercase() && ROOM_LETTERS.contains(&lower) => {}
                    None => return None,
                }
            }
            _ => return None,
        },
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

/// A key while a tag has the keys: ↑ ↓ (Tab, Shift-Tab) between the tags,
/// the rest its line editor's.
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

/// The format list's keys: ↑ ↓ the cursor, Enter picks, Esc or `f` closes.
fn formats_key(gui: &mut Gui, key: KeyEvent) {
    let options = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
    let Some(menu) = gui.ytdl.formats.as_mut() else { return };
    match key.code {
        KeyCode::Up => menu.cursor = menu.cursor.saturating_sub(1),
        KeyCode::Down => menu.cursor = (menu.cursor + 1).min(options.len().saturating_sub(1)),
        KeyCode::Enter => {
            if let Some(codec) = options.get(menu.cursor) {
                pick_format(gui, codec.to_string());
            }
        }
        KeyCode::Esc | KeyCode::Char('f' | 'F') => gui.ytdl.formats = None,
        _ => {}
    }
}

/// The list's keys (L1–L4): ↑ ↓ the held row, Enter its first way (a
/// song's: play), `x` dismisses, `a` and `m` a song's queue and list, Esc
/// closes — the song's list first.
fn list_key(gui: &mut Gui, key: KeyEvent) -> bool {
    let Some(list) = gui.ytdl.list.as_mut() else { return false };
    if let Some((item, cursor)) = list.menu {
        match (key.code, cursor) {
            // The first walking key shows the cursor, on the first choice.
            (KeyCode::Up | KeyCode::Down, None) => list.menu = Some((item, Some(0))),
            (KeyCode::Up, Some(at)) => list.menu = Some((item, Some(at.saturating_sub(1)))),
            (KeyCode::Down, Some(at)) => list.menu = Some((item, Some((at + 1).min(1)))),
            (KeyCode::Enter, Some(at)) => return gui.act(yt(YtAct::MenuPick(at))),
            (KeyCode::Char('a'), _) => return gui.act(yt(YtAct::MenuPick(0))),
            (KeyCode::Esc, _) => list.menu = None,
            _ => {}
        }
        return false;
    }
    let len = list.items.len();
    match key.code {
        KeyCode::Esc => {
            close_list(gui);
            return false;
        }
        KeyCode::Down | KeyCode::Up if len > 0 => {
            // The first walking key shows the cursor; the next ones move it.
            if !list.view.held {
                list.view.held = true;
                list.held = Some(list.held.unwrap_or(0).min(len - 1));
            } else {
                let at = list.held.unwrap_or(0);
                list.held = Some(if key.code == KeyCode::Down { (at + 1).min(len - 1) } else { at.saturating_sub(1) });
            }
        }
        _ => {}
    }
    let Some(held) = list.held.filter(|_| list.view.held) else { return false };
    let item = list.items.get(held).cloned();
    match (key.code, item) {
        (KeyCode::Enter, Some(Item::Song(_))) => return gui.act(yt(YtAct::Play(held))),
        (KeyCode::Char('a'), Some(Item::Song(_))) => return gui.act(yt(YtAct::Queue(held))),
        (KeyCode::Char('m'), Some(Item::Song(_))) => {
            if let Some(list) = gui.ytdl.list.as_mut() {
                list.menu = Some((held, Some(0)));
            }
        }
        (KeyCode::Enter, Some(Item::Job(id))) => {
            let first = gui.ytdl.jobs().and_then(|j| j.job(&id)).and_then(|job| {
                let formats = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
                job.ways(Some(&formats)).into_iter().next()
            });
            if let Some(w) = first {
                return gui.act(yt(YtAct::Way(id, w)));
            }
        }
        (KeyCode::Char('x'), Some(Item::Job(id))) => {
            let settled = gui.ytdl.jobs().and_then(|j| j.job(&id)).is_some_and(|j| !j.state.open());
            if settled {
                return gui.act(yt(YtAct::Way(id, Way::Dismiss)));
            }
        }
        _ => {}
    }
    false
}

// ── Paste ───────────────────────────────────────────────────────────────────

/// A paste as the link field takes it: the first non-blank line, controls
/// and the invisible format characters copied text carries left out, at
/// most [`PASTE_MAX`] characters.
fn clean_paste(text: &str) -> String {
    let line = text.split(['\n', '\r', '\u{2028}', '\u{2029}']).map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    line.chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control() && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}'))
        .take(PASTE_MAX)
        .collect()
}

/// A paste, whole: it replaces the link on screen (the room's, the Files
/// window's) and looks it up at once, whoever had the keys; once found the
/// field gives them back. Nothing wants one — or the From YouTube list,
/// Where new songs land or Choose a folder stands over the form — dropped.
pub(crate) fn paste(gui: &mut Gui, text: &str) {
    let form = active_form(gui);
    if !wants_paste(gui) || draft(gui, form).starting.is_some() || list_held(gui) || super::ytdl_where::is_open(gui) {
        return;
    }
    let now = gui.ytdl.now();
    gui.ytdl.formats = None;
    super::ytdl_here::keys_taken(gui);
    let whole = gui.ytdl.whole_pastes;
    let draft = draft_mut(gui, form);
    draft.link = Input::new(clean_paste(text));
    draft.keys = Keys::Link;
    // The same link pasted again asks again (B1: "asks again").
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

/// The tips line for what is up: Where new songs land or Choose a folder,
/// the format list, the list window, the Files window, the room as its
/// keys stand.
pub(crate) fn tips(gui: &Gui) -> String {
    if super::ytdl_where::is_open(gui) {
        return super::ytdl_where::tips(gui);
    }
    if gui.ytdl.formats.is_some() {
        return t!("gui.tips.ytdl_formats").to_string();
    }
    if gui.ytdl.list.is_none() && super::ytdl_here::window_up(gui) {
        return super::ytdl_here::tips(gui);
    }
    if let Some(list) = &gui.ytdl.list {
        if list.menu.is_some() {
            return t!("gui.tips.ytdl_menu").to_string();
        }
        let mut parts = vec![t!("gui.tips.ytdl_rows").to_string()];
        match list.held.filter(|_| list.view.held).and_then(|i| list.items.get(i)) {
            Some(Item::Song(_)) => {
                parts.push(t!("gui.tips.ytdl_play").to_string());
                parts.push(t!("gui.tips.ytdl_queue").to_string());
                parts.push(t!("gui.tips.ytdl_more").to_string());
            }
            Some(Item::Job(id)) => {
                if let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)) {
                    let formats = format_options(facts(gui).and_then(|f| f.formats.as_deref()));
                    if let Some(w) = job.ways(Some(&formats)).into_iter().next() {
                        parts.push(t!("gui.tips.ytdl_enter", verb = way_words(w)).to_string());
                    }
                    if !job.state.open() {
                        parts.push(t!("gui.tips.ytdl_dismiss").to_string());
                    }
                }
            }
            None => {}
        }
        parts.push(t!("gui.tips.ytdl_close").to_string());
        return parts.join(" · ");
    }
    // The press is out: Esc is the one key that does anything (D5).
    if gui.ytdl.draft.starting.is_some() {
        return t!("gui.tips.ytdl_esc_back").to_string();
    }
    match room_keys(gui) {
        Keys::Link => t!("gui.tips.ytdl_field").to_string(),
        Keys::Tag(_) => t!("gui.tips.ytdl_tags").to_string(),
        Keys::None => {
            // The key words the frame drew, top to bottom, the list's last
            // (T2's Keys line).
            let mut drawn: Vec<&(char, YtAct)> = gui.ytdl.drawn.iter().collect();
            drawn.sort_by_key(|(_, act)| *act == YtAct::List);
            let mut keys: Vec<String> = Vec::new();
            for (letter, act) in drawn {
                let words = match act {
                    YtAct::Edit => t!("gui.ytdl.key.edit"),
                    YtAct::Change => t!("gui.ytdl.key.change"),
                    YtAct::Format => t!("gui.tips.ytdl_format"),
                    YtAct::Retry => t!("gui.ytdl.key.retry"),
                    YtAct::Type => t!("gui.ytdl.key.type"),
                    YtAct::Use => t!("gui.ytdl.key.use"),
                    YtAct::Undo => t!("gui.ytdl.key.undo"),
                    YtAct::Choose => t!("gui.ytdl.key.choose"),
                    YtAct::List => t!("gui.tips.ytdl_list_key"),
                    _ => continue,
                };
                if words.starts_with(*letter) && !keys.iter().any(|k| k.as_str() == words) {
                    keys.push(words.to_string());
                }
            }
            let mut tail = Vec::new();
            if room_view(gui).primary.is_some_and(|(_, on)| on) {
                tail.push(t!("gui.tips.ytdl_enter_add").to_string());
            }
            tail.push(t!("gui.tips.ytdl_esc_back").to_string());
            let line = |keys: &[String]| {
                std::iter::once(t!("gui.tips.ytdl_paste").to_string())
                    .chain(keys.iter().cloned())
                    .chain(tail.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" · ")
            };
            // At most the footer's 99 cells: the last key words give way,
            // the paste, Enter and the way back stay.
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
    use crate::api::ytdl::{YtdlAdd, YtdlEntry, YtdlStarted};
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

    pub(crate) fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        observe(&mut gui, &connected(ping(generation)));
        gui.active = super::super::SETTINGS_NAV;
        gui
    }

    /// The room open on the new server (generation 1).
    pub(crate) fn room() -> Gui {
        let mut gui = seated(1);
        open_room(&mut gui);
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
            ..YtdlMetadata::default()
        }
    }

    /// Daft Punk › Discovery is new under Music, which has Daft Punk.
    pub(crate) fn discovery_new() -> FolderCheck {
        FolderCheck { known: true, new_segments: vec![1], deepest: Some("/Music/Daft Punk/".into()), files: None }
    }

    /// The link pasted, its answer in, the folder checked: T2's song.
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

    /// The room's 80 × 23 at 100 × 30 (TermRoomStates), from x 17, row 2.
    fn assert_room(gui: &mut Gui, rows: &str, runs: &[&str]) {
        let b = buffer(gui, 100, 30);
        assert_frame(&b, (17, 2), 80, rows, runs);
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

    // ── Settings ───────────────────────────────────────────────────────────

    /// T1: TORRENTS is ADD MUSIC, holding Add a torrent, Add from YouTube
    /// and the torrent switch — 17 rows — and the doorway brightens under
    /// the pointer.
    #[test]
    fn settings_hold_add_from_youtube_under_add_music() {
        let _en = english();
        let mut gui = seated(1);
        gui.app.crossfade = 0.0;
        gui.app.gapless = true;
        gui.app.blend_skips = false;
        gui.app.pause_fade = false;
        gui.app.resume_queue = true;
        gui.ui.motion(Position { x: 30, y: 14 });
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::T1, boards::T1_RUNS);
        assert_eq!(gui.ui.hit(Position { x: 30, y: 14 }), Some(Act::Row(super::super::ROW_YOUTUBE)));

        // ↓ walks onto it; Enter opens the room, the field holding the keys.
        gui.cursor = Some(super::super::ROW_TORRENT);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.cursor, Some(super::super::ROW_YOUTUBE));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.in_settings_room(SettingsRoom::YouTube));
        assert_eq!(gui.ytdl.draft.keys, Keys::Link);
        assert_eq!(gui.cursor, None, "back in Settings the cursor is stowed");
    }

    /// Hidden, never greyed: on a peer, with uploads off, with no audio
    /// conversion, with no library, the row is absent and the ones under it
    /// move up a line — the cursor walks past where it was.
    #[test]
    fn the_doorway_is_absent_under_each_gate() {
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
            assert!(!text.contains("Add from YouTube"), "{why}: absent\n{text}");
            assert!(rows[14].contains("[✓] Ask what to do with torrents"), "{why}: the switch moved up\n{text}");
            assert!(rows[16].contains("DISPLAY") && rows[17].contains("Keyboard hints"), "{why}\n{text}");
            gui.cursor = Some(super::super::ROW_TORRENT);
            key(&mut gui, KeyCode::Down);
            assert_eq!(gui.cursor, Some(super::super::ROW_ASK), "{why}: ↓ skips the absent row");
            gui.act(Act::Row(super::super::ROW_YOUTUBE));
            assert!(!gui.in_settings_room(SettingsRoom::YouTube), "{why}: no way in");
        }
        // A cursor resting on it when it goes moves to the row in its place.
        let mut gui = seated(1);
        gui.cursor = Some(super::super::ROW_YOUTUBE);
        gui.app.libraries.clear();
        draw(&mut gui);
        assert_eq!(gui.cursor, Some(super::super::ROW_ASK));
    }

    /// At 100 × 24 with key hints on the column has 16 rows: the blank above
    /// DISPLAY gives way and Keyboard hints keeps its line above the bar.
    #[test]
    fn the_blank_above_display_gives_way_at_the_floor() {
        let _en = english();
        let mut gui = seated(1);
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = lines(&buffer(&mut gui, 100, 24));
        let text = rows.join("\n");
        assert!(rows[15].contains("[✓] Ask what to do with torrents"), "{text}");
        assert!(rows[16].contains("DISPLAY"), "no blank above it:\n{text}");
        assert!(rows[17].contains("Keyboard hints"), "{text}");
        // Without the YouTube row the sixteen lines fit, blank and all.
        gui.app.libraries.clear();
        let rows = lines(&buffer(&mut gui, 100, 24));
        assert!(rows[15].trim_end().ends_with('│') && rows[16].contains("DISPLAY"), "{}", rows.join("\n"));
    }

    /// S1–S4: the description, or one word while there is news — adding
    /// ACCENT, ! failed GOLD, ✓ added OK for an hour, then the description
    /// again.
    #[test]
    fn the_doorway_says_one_word_while_there_is_news() {
        let _en = english();
        let cases: [(&str, &str, &[&str], Vec<Value>); 4] = [
            ("S1", boards::S1, boards::S1_RUNS, vec![history()]),
            ("S2", boards::S2, boards::S2_RUNS, vec![job("a1", "adding", "One More Time", "/Music/Daft Punk/Discovery/")]),
            ("S3", boards::S3, boards::S3_RUNS, {
                let mut j = job("f1", "failed", "Around the World", "/Music/Daft Punk/Homework/");
                j["seen"] = false.into();
                j["endedAt"] = START.into();
                vec![j]
            }),
            ("S4", boards::S4, boards::S4_RUNS, {
                let mut j = history();
                j["seen"] = false.into();
                j["endedAt"] = (START - 10 * 60_000).into();
                vec![j]
            }),
        ];
        for (id, rows, runs, jobs) in cases {
            let mut gui = seated(1);
            seed(&mut gui, jobs);
            let b = buffer(&mut gui, 100, 30);
            eprintln!("{id}");
            assert_frame(&b, (17, 12), 80, rows, runs);
        }
        // The hour over, ✓ added gives the description back.
        let mut gui = seated(1);
        let mut j = history();
        j["seen"] = false.into();
        j["endedAt"] = (START - 10 * 60_000).into();
        seed(&mut gui, vec![j]);
        advance(&gui, 60 * 60_000);
        let rows = draw(&mut gui);
        assert!(rows[14].contains("a YouTube link, into a library"), "{}", rows[14]);
    }

    // ── The room ───────────────────────────────────────────────────────────

    /// T2: found, at 80 cells — the card gave the keys back, the song and
    /// its facts, where it goes, the format and the tall primary.
    #[test]
    fn a_found_song_at_eighty_cells_is_the_board() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "found: the field gave the keys back");
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::T2, boards::T2_RUNS);
        // Every control the board draws clicks.
        let rows = lines(&b);
        for (text, act) in [
            ("added songs", YtAct::List),
            ("e edit tags", YtAct::Edit),
            ("d change", YtAct::Change),
            ("[X]", YtAct::Clear),
            ("MP3 ▾", YtAct::Format),
            ("Add to library", YtAct::Add),
        ] {
            assert_eq!(gui.ui.hit(at(&rows, text)), Some(yt(act)), "{text}");
        }
        assert_eq!(gui.ui.hit(Position { x: 17, y: 2 }), Some(yt(YtAct::Back)));
        assert_eq!(gui.ui.hit(Position { x: 40, y: 5 }), Some(yt(YtAct::Field)));
        // The tips line names the board's keys, in its order.
        assert_eq!(tips(&gui), "a paste replaces the link · e edit tags · d change · f format · l list · Enter add · Esc back");
        gui.ytdl.draft.keys = Keys::Link;
        assert_eq!(tips(&gui), "type or paste a link · Enter look it up · Esc give the keys back");
    }

    /// T3 and T4: with the queue open the room is 47 cells — the place gives
    /// up whole folders from the left, e edit tags and d change stay; at the
    /// 100 × 24 floor the primary rises and every blank row holds.
    #[test]
    fn the_room_gives_way_at_forty_seven_cells_and_the_floor() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        gui.queue_open = true;
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 47, boards::T3, boards::T3_RUNS);
        let b = buffer(&mut gui, 100, 24);
        assert_frame(&b, (17, 2), 47, boards::T4, boards::T4_RUNS);
        assert_eq!(gui.ui.hit(Position { x: 50, y: 17 }), Some(yt(YtAct::Add)), "the primary at x 42–63");
    }

    /// A1–A3: empty, the field has the keys (caret, placeholder); a link
    /// that isn't YouTube's or names a playlist is refused in GOLD under it,
    /// the field keeping the keys.
    #[test]
    fn the_link_field_and_its_refusals_are_the_boards() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        assert_room(&mut gui, boards::A1, boards::A1_RUNS);
        paste(&mut gui, "https://soundcloud.com/daft-punk/one-more-time");
        assert_room(&mut gui, boards::A2, boards::A2_RUNS);
        assert!(gui.ytdl.work.is_empty(), "nothing asked of the server");
        assert_eq!(gui.ytdl.draft.keys, Keys::Link, "a refused link keeps the keys");
        paste(&mut gui, "https://youtube.com/playlist?list=PLSdoVPM5WnndSQEXRz704yQkKwx76GvPV");
        assert_room(&mut gui, boards::A3, boards::A3_RUNS);
        // [X] clears it, the keys staying.
        gui.act(yt(YtAct::Clear));
        assert_room(&mut gui, boards::A1, boards::A1_RUNS);
    }

    /// A4, A5: a video in a playlist (DIM: just this video) and a live link
    /// (GOLD) — the line under the card, the song under it.
    #[test]
    fn a_videos_own_lines_take_the_row_under_the_card() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, "https://www.youtube.com/watch?v=FGBhQbmPwH8&list=RDFGBhQbmPwH8");
        assert!(matches!(&gui.ytdl.work[..], [Work::Lookup { url, .. }] if url == WATCH), "the canonical link asked");
        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, discovery_new());
        assert_room(&mut gui, boards::A4, boards::A4_RUNS);
        paste(&mut gui, "https://www.youtube.com/live/FGBhQbmPwH8");
        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, discovery_new());
        assert_room(&mut gui, boards::A5, boards::A5_RUNS);
    }

    /// B1, B2: reading, then still looking after 8 s with `t type the
    /// details`; B3: typed by hand, the late answer offered with `u use
    /// them`, which fills only what was left empty.
    #[test]
    fn the_lookup_reads_then_waits_then_offers_a_late_answer() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        assert_room(&mut gui, boards::B1, boards::B1_RUNS);
        advance(&gui, 8_000);
        assert_room(&mut gui, boards::B2, boards::B2_RUNS);
        key(&mut gui, KeyCode::Char('t'));
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Title));
        type_text(&mut gui, "One More Time");
        key(&mut gui, KeyCode::Down);
        type_text(&mut gui, "Daft Punk");
        key(&mut gui, KeyCode::Esc);
        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, there("/Music/Daft Punk/", &[]));
        assert_room(&mut gui, boards::B3, boards::B3_RUNS);
        key(&mut gui, KeyCode::Char('u'));
        let tags = gui.ytdl.draft.tags();
        assert_eq!(
            (tags.title.as_str(), tags.artist.as_str(), tags.album.as_str(), tags.year.as_str()),
            ("One More Time", "Daft Punk", "Discovery", "2001"),
            "only the empty tags filled"
        );
    }

    /// B4: a video the server couldn't read — the GOLD words, the tags open
    /// empty with the title holding the keys and its help line; B5 and D2:
    /// no answer in time, or no server — `a try again`, no primary; D1: no
    /// yt-dlp — the primary off.
    #[test]
    fn a_failed_lookup_reads_as_the_boards() {
        let _en = english();
        let bare_500 = YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None };
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(bare_500));
        answer_folders(&mut gui, there("/Music/", &[]));
        assert_room(&mut gui, boards::B4, boards::B4_RUNS);

        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        let late = YtdlCallError::Unanswered { why: crate::api::ytdl::NoAnswer::TimedOut, detail: String::new() };
        answer_lookup(&mut gui, Err(late));
        assert_room(&mut gui, boards::B5, boards::B5_RUNS);
        key(&mut gui, KeyCode::Char('a'));
        assert!(matches!(gui.ytdl.draft.look, Look::Reading { .. }), "a: asked again");

        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        let missing = YtdlCallError::Answered {
            status: 500,
            words: Some("yt-dlp is not installed".into()),
            reason: None,
            file_name: None,
        };
        answer_lookup(&mut gui, Err(missing));
        assert_room(&mut gui, boards::D1, boards::D1_RUNS);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none(), "the primary is off");

        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        let refused = YtdlCallError::Unanswered { why: crate::api::ytdl::NoAnswer::Refused, detail: String::new() };
        answer_lookup(&mut gui, Err(refused));
        assert_room(&mut gui, boards::D2, boards::D2_RUNS);
        // Offline reads the same: the boards have no offline words.
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        paste(&mut gui, LINK);
        let offline = YtdlCallError::Unanswered { why: crate::api::ytdl::NoAnswer::Offline, detail: String::new() };
        answer_lookup(&mut gui, Err(offline));
        assert_room(&mut gui, boards::D2, boards::D2_RUNS);
    }

    /// C1: e edit tags — four fields under a label column, the title's
    /// label ACCENT with the caret, one help line; the key words stand down
    /// while a field has the keys.
    #[test]
    fn edit_tags_opens_the_four_fields() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        assert_room(&mut gui, boards::C1, boards::C1_RUNS);
        // ↓ walks the tags; typing marks a tag edited; Esc gives the keys
        // back and the key words come back with them.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Artist));
        key(&mut gui, KeyCode::Esc);
        let rows = draw(&mut gui);
        assert!(rows[14].contains("d change"), "{}", rows.join("\n"));
    }

    /// C2: a cleaned title — "Cleaned from “…”" with `u undo`, which puts the
    /// clean-up back; C3: an artist that may be the channel's, in GOLD.
    #[test]
    fn the_clean_up_and_the_channel_check_are_the_boards() {
        let _en = english();
        let cleaned = YtdlMetadata {
            title: Some("Daft Punk - One More Time (Official Video)".into()),
            artist: Some("Daft Punk".into()),
            year: Some("2009".into()),
            ..YtdlMetadata::default()
        };
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, cleaned, there("/Music/Daft Punk/", &[]));
        assert_room(&mut gui, boards::C2, boards::C2_RUNS);
        key(&mut gui, KeyCode::Char('u'));
        assert_eq!(gui.ytdl.draft.tags().title, "Daft Punk - One More Time (Official Video)", "u put it all back");

        let channel = YtdlMetadata {
            title: Some("One More Time".into()),
            artist: Some("Daft Punk".into()),
            year: Some("2009".into()),
            ..YtdlMetadata::default()
        };
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, channel, there("/Music/Daft Punk/", &[]));
        assert_room(&mut gui, boards::C3, boards::C3_RUNS);
    }

    /// C4: the folder already has the file's name — GOLD, `f change format`,
    /// Add anyway; C5: a folder the account can't make — GOLD in the place's
    /// row, `i save to … instead`, `c choose a folder`, the primary off.
    #[test]
    fn a_clash_and_a_folder_that_cant_be_made_are_the_boards() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), there("/Music/Daft Punk/Discovery/", &["One_More_Time.mp3", "Aerodynamic.mp3"]));
        assert_room(&mut gui, boards::C4, boards::C4_RUNS);
        key(&mut gui, KeyCode::Char('f'));
        assert!(gui.ytdl.formats.is_some(), "f opens the format list");

        let mut gui = seated(1);
        observe(&mut gui, &connected(Ping { no_mkdir: Some(true), ..ping(1) }));
        open_room(&mut gui);
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        assert_room(&mut gui, boards::C5, boards::C5_RUNS);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none(), "the primary is off");
        key(&mut gui, KeyCode::Char('i'));
        assert_eq!(gui.ytdl.draft.instead.as_deref(), Some("/Music/Daft Punk/"));
        answer_folders(&mut gui, there("/Music/Daft Punk/", &[]));
        let rows = draw(&mut gui);
        assert!(rows[12].contains("Files go to Music › Daft Punk "), "the folder that exists:\n{}", rows.join("\n"));
        assert!(rows[23].contains("Add to library ▸"), "and Add works again");
    }

    /// D5: the press is out — Adding…, no click, no hover, every key but
    /// Esc waiting; D3: the server refused it — its words in GOLD over the
    /// primary, Enter trying again; D4: already adding this link to this
    /// folder.
    #[test]
    fn the_press_and_the_servers_answers_are_the_boards() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "Enter pressed the primary");
        poll(&mut gui);
        assert!(
            calls(&gui).iter().any(|c| matches!(c, Call::Mkdir { directory } if directory == "/Music/Daft Punk/Discovery/")),
            "the new folder made first: {:?}",
            calls(&gui)
        );
        assert_room(&mut gui, boards::D5, boards::D5_RUNS);
        let rows = draw(&mut gui);
        assert_eq!(gui.ui.hit(at(&rows, "Adding…")), None, "no click");
        // ◂ back only: every other control stands, neither clicking nor
        // lighting under the pointer, and the tips name Esc alone.
        for text in ["e edit tags", "d change", "MP3 ▾", "[X]", "added songs", "youtu.be"] {
            let spot = at(&rows, text);
            gui.ui.pointer = Some(spot);
            let b = buffer(&mut gui, 100, 30);
            assert_eq!(gui.ui.hit(spot), None, "{text}: no click");
            let cell = &b[(spot.x, spot.y)];
            assert!(!cell.modifier.contains(Modifier::BOLD) && cell.fg != th().bright, "{text}: no hover: {cell:?}");
        }
        gui.ui.pointer = None;
        draw(&mut gui);
        assert_eq!(gui.ui.hit(Position { x: 17, y: 2 }), Some(yt(YtAct::Back)));
        assert_eq!(tips(&gui), "Esc back");
        for act in [YtAct::Edit, YtAct::Change, YtAct::Format, YtAct::List] {
            gui.act(yt(act));
        }
        assert!(
            !gui.ytdl.draft.tags_open && gui.ytdl.place.is_none() && gui.ytdl.formats.is_none() && gui.ytdl.list.is_none(),
            "their acts wait too"
        );
        key(&mut gui, KeyCode::Char('e'));
        assert!(!gui.ytdl.draft.tags_open, "keys wait");

        let mut ffmpeg = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Err(YtdlCallError::Answered {
                status: 500,
                words: Some("FFmpeg is not available yet".into()),
                reason: None,
                file_name: None,
            }))),
            other => new_server(None)(other),
        };
        serve(&mut gui, &mut ffmpeg);
        assert!(gui.ytdl.draft.starting.is_none());
        assert_room(&mut gui, boards::D3, boards::D3_RUNS);
        assert!(gui.ytdl.work.iter().any(|w| matches!(w, Work::Ping { .. })), "a stale flag: pinged again");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some(), "Enter tries again");

        let mut gui = room();
        let mut open = job("o1", "adding", "One More Time", "/Music/Daft Punk/Discovery/");
        open["hidden"] = true.into();
        open["serverId"] = "srv0".into();
        seed(&mut gui, vec![history(), open]);
        found(&mut gui, one_more_time(), there("/Music/Daft Punk/Discovery/", &[]));
        assert_room(&mut gui, boards::D4, boards::D4_RUNS);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_none(), "never twice for one link and folder");
    }

    /// E1–E3: the server took it — the form starts over with the field
    /// holding the keys, the title row's word and the note say so; it
    /// landed, or it failed.
    #[test]
    fn after_the_press_the_form_resets_and_the_word_follows_the_job() {
        let _en = english();
        for (id, rows, runs, note, note_runs, listed) in [
            ("E1", boards::E1, boards::E1_RUNS, boards::E1_NOTE, boards::E1_NOTE_RUNS, None),
            ("E2", boards::E2, boards::E2_RUNS, boards::E2_NOTE, boards::E2_NOTE_RUNS, Some(("complete", None))),
            (
                "E3",
                boards::E3,
                boards::E3_RUNS,
                boards::E3_NOTE,
                boards::E3_NOTE_RUNS,
                Some(("error", Some("Sign in to confirm you’re not a bot."))),
            ),
        ] {
            eprintln!("{id}");
            let mut gui = room();
            seed(&mut gui, vec![history()]);
            found(&mut gui, one_more_time(), discovery_new());
            key(&mut gui, KeyCode::Enter);
            serve(&mut gui, &mut new_server(listed));
            assert_eq!(gui.ytdl.draft.keys, Keys::Link, "{id}: the field has the keys again");
            let b = buffer(&mut gui, 100, 30);
            assert_frame(&b, (17, 2), 80, rows, runs);
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

    /// N4: where the note has more than 30 cells the place joins it.
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

    /// N1–N3 on the bar's row: Adding … ACCENT, Added … OK, Couldn't add …
    /// GOLD, clipped at 30 cells.
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

    // ── The From YouTube list ──────────────────────────────────────────────

    fn record(id: i64, title: &str, path: &str, at: i64, present: bool) -> crate::api::ytdl::YtdlRecord {
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

    /// The boards' Added list: one an hour ago, two in October, one gone.
    fn added_list(newest: (&str, &str, i64)) -> Vec<crate::api::ytdl::YtdlRecord> {
        vec![
            record(9, newest.0, newest.1, newest.2, true),
            record(8, "Harder, Better, Faster, Stronger", "Music/Daft Punk/Discovery/Harder_Better_Faster_Stronger.mp3", START - 3 * 86_400_000, true),
            record(7, "Aerodynamic", "Mixes/Aerodynamic.opus", START - 6 * 86_400_000, true),
            record(6, "Around the World", "Mixes/Around_the_World.mp3", START - 7 * 86_400_000, false),
        ]
    }

    /// L1's jobs: one adding for two minutes, one starting, one failed in
    /// the server's words.
    fn l1_jobs() -> Vec<Value> {
        let mut adding = job("j1", "adding", "One More Time", "/Music/Daft Punk/Discovery/");
        adding["serverId"] = "srv1".into();
        adding["sentAt"] = (START - 120_000).into();
        let starting = job("j2", "startUnsure", "Aerodynamic", "/Music/Daft Punk/Discovery/");
        let mut failed = job("j3", "failed", "Around the World", "/Music/Daft Punk/Homework/");
        failed["failure"] = serde_json::json!({
            "problem": "other", "phase": "download",
            "detail": "Sign in to confirm you’re not a bot.", "text": "Sign in to confirm you’re not a bot.",
        });
        vec![adding, starting, failed]
    }

    fn list_gui(jobs: Vec<Value>, songs: Option<Vec<crate::api::ytdl::YtdlRecord>>) -> Gui {
        let mut gui = room();
        gui.ytdl.zone = Some(None);
        seed(&mut gui, jobs);
        gui.act(yt(YtAct::List));
        if let Some(songs) = songs {
            serve(&mut gui, &mut records(songs));
        }
        gui
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
        let mut gui = list_gui(vec![failed("n1", "Da Funk"), failed("n2", "Revolution 909"), failed("n3", "Fresh")], None);
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        draw(&mut gui);
        let held = |gui: &Gui| {
            let list = gui.ytdl.list.as_ref().unwrap();
            match list.held.and_then(|i| list.items.get(i)) {
                Some(Item::Job(id)) => id.clone(),
                _ => panic!("a job held"),
            }
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

    /// L1: adding, then needs a look, then added — 76 wide at Copies' place,
    /// the title on the border, "Esc close · it keeps going" while one runs.
    #[test]
    fn the_list_while_one_is_adding_is_the_board() {
        let _en = english();
        let mut gui = list_gui(
            l1_jobs(),
            Some(added_list(("Digital Love", "Music/Daft Punk/Discovery/Digital_Love.mp3", START - 3_600_000))),
        );
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (19, 6), 76, boards::L1, boards::L1_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Try again")), Some(yt(YtAct::Way("j3".into(), Way::Retry))));
        assert_eq!(gui.ui.hit(at(&rows, "Show earlier")), Some(yt(YtAct::ShowEarlier)));
        assert_eq!(gui.ui.hit(at(&rows, "Esc close")), Some(yt(YtAct::ListClose)));
        assert_eq!(gui.ui.hit(at(&rows, "Digital Love")), Some(yt(YtAct::Play(3))), "a ✓ song plays");
        assert_eq!(gui.ui.hit(at(&rows, "file missing")), Some(Act::Guard), "a missing one doesn't");
        // The room beneath is inert, and the right click is the song's list.
        assert_eq!(gui.ui.hit(Position { x: 17, y: 2 }), Some(Act::Guard));
        assert_eq!(gui.ui.hit_context(at(&rows, "Digital Love")), Some(yt(YtAct::More(3))));
    }

    /// L2: every kind that needs a look, each with its ways on — Check
    /// again alone on its own row, the words and the ways sharing a row
    /// where both fit.
    #[test]
    fn every_kind_that_needs_a_look_is_the_board() {
        let _en = english();
        let unsure = job("k1", "notSureStarted", "Digital Love", "/Music/Daft Punk/Discovery/");
        let mut landed = job("k2", "failed", "One More Time", "/Music/Daft Punk/Discovery/");
        landed["failure"] = serde_json::json!({ "problem": "exists", "phase": "download", "detail": "One_More_Time.mp3" });
        let gone = job("k3", "notFound", "Harder, Better, Faster, Stronger", "/Music/Daft Punk/Discovery/");
        let mut gui = list_gui(vec![unsure, landed, gone], None);
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (19, 6), 76, boards::L2, boards::L2_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Check again")), Some(yt(YtAct::Way("k1".into(), Way::CheckAgain))));
        assert_eq!(gui.ui.hit(at(&rows, "Add again")), Some(yt(YtAct::Way("k3".into(), Way::AddAgain))));
        // Change format and retry hangs the format list from its first cell.
        let change = at(&rows, "Change format and retry");
        gui.act(gui.ui.hit(change).unwrap());
        let rows = draw(&mut gui);
        let menu = gui.ytdl.formats.clone().expect("the format list");
        assert_eq!((menu.anchor.x, menu.anchor.y), (change.x, change.y));
        assert!(rows[change.y as usize + 1].contains('╭'), "it hangs under the word:\n{}", rows.join("\n"));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.formats.is_none());
        assert!(
            calls(&gui).iter().any(|c| matches!(c, Call::Add(YtdlAdd { codec, .. }) if codec == "m4a"))
                || gui.ytdl.jobs().unwrap().jobs().iter().any(|j| j.codec == "m4a"),
            "retried in the format picked"
        );
    }

    /// L3, L4: history only — the pointer on a song shows its verbs; [⋯]
    /// (or a right click) hangs its list: Add to queue `a`, Show in folder.
    #[test]
    fn a_song_under_the_pointer_shows_its_verbs_and_its_list() {
        let _en = english();
        let mut gui = list_gui(
            vec![],
            Some(added_list(("One More Time", "Music/Daft Punk/Discovery/One_More_Time.mp3", START - 3 * 60_000))),
        );
        // Digital Love, an hour old, sits second: the record's own row.
        let mut songs = added_list(("One More Time", "Music/Daft Punk/Discovery/One_More_Time.mp3", START - 3 * 60_000));
        songs.insert(1, record(10, "Digital Love", "Music/Daft Punk/Discovery/Digital_Love.mp3", START - 3_600_000, true));
        gui.act(yt(YtAct::ListClose));
        gui.act(yt(YtAct::List));
        serve(&mut gui, &mut records(songs));
        gui.ui.motion(Position { x: 26, y: 8 });
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (19, 6), 76, boards::L3, boards::L3_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(Position { x: 82, y: 8 }), Some(yt(YtAct::Play(1))), "[▸]");
        assert_eq!(gui.ui.hit(Position { x: 86, y: 8 }), Some(yt(YtAct::Queue(1))), "[+]");
        assert_eq!(gui.ui.hit(Position { x: 90, y: 8 }), Some(yt(YtAct::More(1))), "[⋯]");
        let _ = rows;
        gui.ui.motion(Position { x: 91, y: 8 });
        gui.act(yt(YtAct::More(1)));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (19, 6), 76, boards::L4, boards::L4_RUNS);
        // The list's choices: Add to queue queues the song.
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Add to queue")), Some(yt(YtAct::MenuPick(0))));
        gui.act(yt(YtAct::MenuPick(0)));
        assert_eq!(gui.app.queue.items.len(), 1, "queued");
        assert_eq!(gui.app.queue.items[0].track.filepath, "Music/Daft Punk/Discovery/Digital_Love.mp3");
    }

    /// F1: over the 47-cell room the list centres on the frame; the room's
    /// word and its field stand beneath it, inert.
    #[test]
    fn the_list_over_a_narrow_room_is_the_board() {
        let _en = english();
        let mut gui = list_gui(
            l1_jobs(),
            Some(added_list(("Digital Love", "Music/Daft Punk/Discovery/Digital_Love.mp3", START - 3_600_000))),
        );
        gui.queue_open = true;
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (12, 6), 76, boards::F1, boards::F1_RUNS);
        assert_frame(&b, (17, 2), 47, boards::F1_ROOM, boards::F1_ROOM_RUNS);
    }

    /// The list's keys: ↓ shows the cursor, then walks it; Enter is the held
    /// row's first way (a song's: play), `x` dismisses a settled row, `m`
    /// the song's list, Esc closes the song's list, then the list.
    #[test]
    fn the_lists_keys_walk_act_and_close() {
        let _en = english();
        let mut gui = list_gui(
            l1_jobs(),
            Some(added_list(("Digital Love", "Music/Daft Punk/Discovery/Digital_Love.mp3", START - 3_600_000))),
        );
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.ytdl.list.as_ref().unwrap().held, Some(0), "picked up where it rests");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        let tips = tips(&gui);
        assert_eq!(tips, "↑ ↓ rows · Enter Try again · x dismiss · Esc close");
        key(&mut gui, KeyCode::Char('x'));
        assert!(gui.ytdl.jobs().unwrap().job("j3").is_none(), "x dismissed the failure");
        draw(&mut gui);
        assert!(matches!(gui.ytdl.list.as_ref().unwrap().items.get(2), Some(Item::Song(_))));
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.queue.items.len(), 1, "Enter plays the held song");
        key(&mut gui, KeyCode::Char('m'));
        assert!(gui.ytdl.list.as_ref().unwrap().menu.is_some());
        key(&mut gui, KeyCode::Esc);
        assert!(gui.ytdl.list.as_ref().unwrap().menu.is_none(), "Esc closes the song's list first");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.ytdl.list.is_none(), "then the list");
        assert!(gui.in_settings_room(SettingsRoom::YouTube), "the room stays");
        assert!(!gui.ytdl.jobs().unwrap().summary().word().is_some_and(|w| w == Word::Failed));
    }

    // ── Keys ───────────────────────────────────────────────────────────────

    /// Esc gives the field's keys back; a second Esc goes back to Settings.
    /// The room's own capitals never reach the shell (D opens Auto DJ, T
    /// Stats, A toggles the DJ).
    #[test]
    fn esc_gives_the_keys_back_and_the_rooms_capitals_stay_home() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        gui.ytdl.draft.keys = Keys::Link;
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.ytdl.draft.keys, Keys::None, "the keys given back");
        assert!(gui.in_settings_room(SettingsRoom::YouTube));
        draw(&mut gui);
        let armed = gui.app.dj_armed();
        for c in ['D', 'T', 'A', 'E', 'U'] {
            shift(&mut gui, c);
            assert!(gui.in_settings_room(SettingsRoom::YouTube), "{c} stayed in the room");
            assert_eq!(gui.screen, super::super::Screen::Library, "{c}");
            assert_eq!(gui.app.dj_armed(), armed, "{c}");
            gui.ytdl.draft.keys = Keys::None;
            gui.ytdl.draft.tags_open = false;
            draw(&mut gui);
        }
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.in_settings_room(SettingsRoom::YouTube), "the second Esc goes back");
        assert_eq!(gui.active, super::super::SETTINGS_NAV);
    }

    /// Caps Lock sends the capital with no Shift: the room keeps it just
    /// the same — a capital of a word it drew is that word's (D: d change)
    /// — and so do the windows over it, the From YouTube list and Where
    /// new songs land, whose keys are all theirs. D, T and A never open
    /// Auto DJ or Stats or toggle the DJ while any of them is up.
    #[test]
    fn caps_lock_capitals_stay_in_the_room_and_its_windows() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        found(&mut gui, one_more_time(), discovery_new());
        gui.ytdl.draft.keys = Keys::None;
        draw(&mut gui);
        let armed = gui.app.dj_armed();
        let stays = |gui: &mut Gui, c: char, modifiers: KeyModifiers, up: &dyn Fn(&Gui) -> bool, what: &str| {
            super::super::handle_key(gui, KeyEvent::new(KeyCode::Char(c), modifiers));
            assert!(up(gui), "{what}: {c} ({modifiers:?}) kept it up");
            assert_eq!((gui.screen, gui.active), (super::super::Screen::Library, super::super::SETTINGS_NAV), "{what}: {c}");
            assert_eq!(gui.app.dj_armed(), armed, "{what}: {c}");
        };
        let room_up = |gui: &Gui| gui.in_settings_room(SettingsRoom::YouTube);
        for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            stays(&mut gui, 'D', modifiers, &room_up, "the room");
            assert!(super::super::ytdl_where::is_open(&gui), "D is d change");
            key(&mut gui, KeyCode::Esc);
            draw(&mut gui);
            for c in ['T', 'A'] {
                stays(&mut gui, c, modifiers, &room_up, "the room");
                draw(&mut gui);
            }
        }
        key(&mut gui, KeyCode::Char('l'));
        draw(&mut gui);
        for (c, modifiers) in ['D', 'T', 'A'].into_iter().flat_map(|c| [(c, KeyModifiers::NONE), (c, KeyModifiers::SHIFT)]) {
            stays(&mut gui, c, modifiers, &list_open, "the list");
            draw(&mut gui);
        }
        key(&mut gui, KeyCode::Esc);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('d'));
        draw(&mut gui);
        for (c, modifiers) in ['D', 'T', 'A'].into_iter().flat_map(|c| [(c, KeyModifiers::NONE), (c, KeyModifiers::SHIFT)]) {
            stays(&mut gui, c, modifiers, &super::super::ytdl_where::is_open, "Where new songs land");
            draw(&mut gui);
        }
        key(&mut gui, KeyCode::Esc);
        assert!(gui.in_settings_room(SettingsRoom::YouTube) && !list_open(&gui), "each window closed back to the room");
    }

    /// A key acts only where its word was drawn: `u` with nothing to undo
    /// does nothing, a letter typed into a tag is the tag's, `l` with no word
    /// on the title row opens nothing.
    #[test]
    fn a_key_acts_only_where_its_word_stands() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        let before = gui.ytdl.draft.tags();
        key(&mut gui, KeyCode::Char('u'));
        assert_eq!(gui.ytdl.draft.tags(), before);
        key(&mut gui, KeyCode::Char('l'));
        assert!(gui.ytdl.list.is_none(), "no word, no list");
        key(&mut gui, KeyCode::Char('e'));
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.ytdl.draft.tags().title, "One More Timed", "typed into the title");
        // With history the word is drawn, and `l` opens the list.
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        gui.ytdl.draft.keys = Keys::None;
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('l'));
        assert!(gui.ytdl.list.is_some());
        assert!(gui.ytdl.jobs().unwrap().summary().word() == Some(Word::History));
    }

    /// The arrows are the room's: the Settings rows stand hidden behind it,
    /// so ↑ ↓ walk no cursor there and ← → Space find no row to act on —
    /// Space stays the transport key; the tags still take ↓ (B3).
    #[test]
    fn the_arrows_never_reach_the_hidden_settings_rows() {
        let _en = english();
        let mut gui = room();
        let blend = gui.app.crossfade;
        assert_eq!(gui.ytdl.draft.keys, Keys::Link);
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::Esc, KeyCode::Down, KeyCode::Right, KeyCode::Left] {
            key(&mut gui, code);
            assert_eq!(gui.cursor, None, "{code:?}: no Settings cursor");
        }
        assert_eq!(gui.app.crossfade, blend, "the blend unseen and untouched");
        key(&mut gui, KeyCode::Char(' '));
        assert!(in_room(&gui) && gui.cursor.is_none(), "Space opened no row");
        // With the press out the arrows wait like the rest.
        found(&mut gui, one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.starting.is_some());
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Right);
        assert_eq!((gui.cursor, gui.app.crossfade), (None, blend));
        // The tags take ↓ where they show.
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.ytdl.draft.keys, Keys::Tag(TagField::Title));
    }

    /// The mini player draws no room: the letters of the last full frame's
    /// key words act on nothing there.
    #[test]
    fn the_mini_player_leaves_the_rooms_letters_dead() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        assert!(gui.ytdl.drawn.iter().any(|(c, _)| *c == 'd'), "d change drawn");
        buffer(&mut gui, 60, 12);
        key(&mut gui, KeyCode::Char('d'));
        assert!(!super::super::ytdl_where::is_open(&gui), "nothing opened behind the mini player");
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('d'));
        assert!(super::super::ytdl_where::is_open(&gui), "full size again, d is d change");
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
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        gui.ytdl.draft.keys = Keys::Tag(TagField::Album);
        let mut ctx = super::super::tests::quiet_ctx(&gui);
        super::super::input(&mut gui, &mut ctx, TermEvent::Paste("  watch this:\nhttps://youtu.be/HG7I6hd6Jfo ".into()));
        assert_eq!(gui.ytdl.draft.link.value(), "watch this:", "the first line, trimmed");
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
    /// way), and one that comes anyway leaves the form be; so does one
    /// under the From YouTube list.
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

        let mut gui = list_gui(vec![history()], None);
        assert!(list_open(&gui));
        paste(&mut gui, LINK);
        assert!(gui.ytdl.draft.link.value().is_empty(), "under the list: dropped");
        assert!(!gui.ytdl.work.iter().any(|w| matches!(w, Work::Lookup { .. })), "nothing asked");
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
        open_room(&mut gui);
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        assert_eq!(host.0, vec![true]);
        gui.act(Act::Nav(super::super::FILES_NAV));
        super::super::frame(&mut terminal, &mut gui, &mut ctx, &mut host).unwrap();
        assert_eq!(host.0, vec![true, false], "leaving turned it off");
    }

    /// The window's face takes the clipboard whole while the room is up.
    #[cfg(feature = "window")]
    #[test]
    fn the_window_takes_a_paste_whole_while_the_room_is_up() {
        use crate::gui::window::face::Face;
        let _en = english();
        let gui = seated(1);
        let ctx = super::super::tests::quiet_ctx(&gui);
        let mut face = super::super::GuiFace { gui, ctx };
        assert!(!face.takes_paste());
        open_room(&mut face.gui);
        assert!(face.takes_paste());
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

    // ── The format list ────────────────────────────────────────────────────

    /// MP3 ▾ (or f) raises the format list over the bottom band, the current
    /// one marked; a pick changes the song's format and the control.
    #[test]
    fn the_format_list_rises_from_its_control() {
        let _en = english();
        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('f'));
        let rows = draw(&mut gui);
        let text = rows.join("\n");
        assert!(rows[22].contains('╰') && rows[17].contains('╭'), "its bottom just above the control:\n{text}");
        assert!(rows[18].contains("• MP3") && rows[18].contains("Plays everywhere"), "{text}");
        assert!(rows[21].contains("FLAC"), "the server's four:\n{text}");
        assert_eq!(gui.ui.hit(at(&rows, "Opus")), Some(yt(YtAct::FormatPick("opus".into()))));
        assert_eq!(gui.ui.hit(Position { x: 90, y: 5 }), Some(yt(YtAct::FormatClose)), "a click elsewhere closes it");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.ytdl.draft.codec.as_deref(), Some("m4a"));
        let rows = draw(&mut gui);
        assert!(rows[23].contains("M4A ▾"), "{}", rows.join("\n"));
    }

    /// Every key this module draws is in the English file.
    #[test]
    fn the_rooms_words_are_in_the_locale() {
        let _en = english();
        let parsed: serde_yaml::Value = serde_yaml::from_str(include_str!("../../locales/en.yml")).unwrap();
        let source = include_str!("ytdl.rs");
        let keys: std::collections::BTreeSet<&str> = source
            .match_indices("\"gui.")
            .map(|(at, _)| &source[at + 1..at + 1 + source[at + 1..].find('"').unwrap()])
            .filter(|k| !k.ends_with('.'))
            .collect();
        for key in keys.into_iter().chain(crate::ytdl::KEYS.iter().copied()) {
            let mut node = &parsed;
            for part in key.split('.') {
                node = &node[part];
            }
            assert!(node.as_str().is_some_and(|s| !s.is_empty()), "{key} is not in en.yml");
        }
    }
    /// Past the boards: at the 100 × 24 floor with key hints on and the
    /// queue open a failed lookup's three GOLD rows and the four tags still
    /// leave where the song goes above the primary — the blanks give way,
    /// then the one under the title row; at 47 cells a row of key words
    /// wraps rather than dropping one.
    #[test]
    fn the_tightest_room_keeps_where_the_song_goes() {
        let _en = english();
        let mut gui = room();
        seed(&mut gui, vec![history()]);
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        gui.queue_open = true;
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None }));
        answer_folders(&mut gui, there("/Music/", &[]));
        let rows = lines(&buffer(&mut gui, 100, 24));
        let text = rows.join("\n");
        let files = at(&rows, "Files go to Music");
        let primary = at(&rows, "Add to library");
        assert!(files.y + 1 < primary.y, "above the primary's frame:\n{text}");
        assert_eq!(at(&rows, "╭──").y, 3, "the card rose into the title row's blank:\n{text}");
        assert!(text.contains("Blank: the file gets the video's own title."), "the help line held:\n{text}");

        let mut gui = seated(1);
        observe(&mut gui, &connected(Ping { no_mkdir: Some(true), ..ping(1) }));
        open_room(&mut gui);
        found(&mut gui, one_more_time(), discovery_new());
        gui.queue_open = true;
        let rows = draw(&mut gui);
        let instead = at(&rows, "i save to Music › Daft Punk instead");
        assert_eq!(at(&rows, "d change").y, instead.y, "d change ends the first row");
        assert_eq!(at(&rows, "c choose a folder"), Position { x: 17, y: instead.y + 1 }, "wrapped, not dropped");
        assert_eq!(gui.ui.hit(at(&rows, "c choose a folder")), Some(yt(YtAct::Choose)));
    }

    /// Past the boards, the form's other lines: letters a file name drops
    /// (DIM under the place), a year that isn't one and a layout that lands
    /// nowhere (DIM over the primary, which is off), the 409's taken name
    /// (GOLD over the primary, with `f change format`).
    #[test]
    fn the_forms_other_lines_read_in_their_tokens() {
        let _en = english();
        let mut gui = room();
        let kanji = YtdlMetadata { title: Some("東京 Night".into()), file_stem: None, ..one_more_time() };
        found(&mut gui, kanji, there("/Music/Daft Punk/Discovery/", &[]));
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let line = at(&rows, "about ");
        assert!(rows[line.y as usize].contains(".mp3 · letters outside A–Z are dropped from file names"), "{}", rows.join("\n"));
        assert_eq!((line.x, line.y), (17, 13), "under the place:\n{}", rows.join("\n"));
        assert_eq!(b[(line.x, line.y)].fg, th().dim);

        let mut gui = room();
        found(&mut gui, one_more_time(), discovery_new());
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('e'));
        for _ in 0..3 {
            key(&mut gui, KeyCode::Down);
        }
        key(&mut gui, KeyCode::Backspace);
        key(&mut gui, KeyCode::Backspace);
        key(&mut gui, KeyCode::Esc);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let why = at(&rows, "A year is 4 digits, like 2001");
        assert_eq!((why.x as usize + "A year is 4 digits, like 2001".len(), why.y), (96, 21), "ending one cell in, over the primary");
        assert_eq!(b[(why.x, why.y)].fg, th().dim);
        assert!(rows[23].contains("Add to library  "), "off, its ▸ padded away:\n{}", rows.join("\n"));

        let mut gui = room();
        gui.config.servers.push(config::ServerEntry {
            url: HOST.into(),
            ytdl_layout: Some("{{TITLE}}".into()),
            ytdl_layout_kind: Some("custom".into()),
            ..Default::default()
        });
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Ok(one_more_time()));
        let rows = draw(&mut gui);
        assert!(rows[12].contains("Files go to —"), "{}", rows.join("\n"));
        assert!(rows[21].contains("Fix the layout to see where files land"), "{}", rows.join("\n"));
        assert!(gui.ytdl.work.iter().all(|w| !matches!(w, Work::Folders { .. })), "nowhere to check");

        let mut gui = room();
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
        let refusal = at(&rows, "! “One_More_Time.mp3” is already in this folder, so nothing was added.");
        assert_eq!((refusal.x, refusal.y), (17, 20));
        assert_eq!(b[(refusal.x, refusal.y)].fg, th().gold);
        assert_eq!(at(&rows, "f change format"), Position { x: 17, y: 21 }, "under it, over the primary's row");
        assert_eq!(gui.ui.hit(Position { x: 18, y: 21 }), Some(yt(YtAct::Format)));
        // Another format is the way on: picked, the refusal goes.
        key(&mut gui, KeyCode::Char('f'));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.draft.refusal.is_none());
    }

    /// Show in folder: Files on the song's folder, and once it lists, the
    /// cursor held on the song.
    #[test]
    fn show_in_folder_rests_the_cursor_on_the_song() {
        let _en = english();
        let mut gui = list_gui(
            vec![],
            Some(added_list(("One More Time", "Music/Daft Punk/Discovery/One_More_Time.mp3", START - 3 * 60_000))),
        );
        // Files was walked into somewhere else before: Music › Abba.
        let abba = crate::tui::app::Entry::Dir { label: "Abba".into(), path: "Music/Abba".into() };
        gui.app.files.trail.push(crate::tui::app::Trail { entries: vec![abba], chosen: 0 });
        draw(&mut gui);
        gui.act(yt(YtAct::More(0)));
        gui.act(yt(YtAct::MenuPick(1)));
        assert_eq!(gui.active, super::super::FILES_NAV);
        assert_eq!(gui.app.path, "Music/Daft Punk/Discovery");
        assert!(gui.pending.iter().any(|e| matches!(e, crate::tui::app::Effect::Api(ApiCmd::Browse(p)) if p == "Music/Daft Punk/Discovery")));
        assert!(gui.ytdl.list.is_none());
        let track = |name: &str| crate::tui::app::Entry::Track {
            label: name.into(),
            track: Box::new(Track { filepath: format!("Music/Daft Punk/Discovery/{name}"), metadata: TrackMetadata::default() }),
        };
        gui.app.files.set(vec![crate::tui::app::Entry::Parent, track("Aerodynamic.mp3"), track("One_More_Time.mp3")]);
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

    // ── Every language ─────────────────────────────────────────────────────

    pub(crate) const LOCALES: [&str; 10] = ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

    /// `words` whole on one of `rows` between cells `x0` and `x1` — a wide
    /// glyph's second cell reads as a blank, so compared without blanks.
    pub(crate) fn whole(rows: &[String], words: &str, (x0, x1): (usize, usize)) -> bool {
        let packed = words.replace(' ', "");
        rows.iter().any(|row| row.chars().skip(x0).take(x1 - x0).collect::<String>().replace(' ', "").contains(&packed))
    }

    /// The key words open with their key in every language — the letter
    /// never translates, and a key acts only where its word stood — and
    /// every letter the room draws is one whose capital it keeps.
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
            ("gui.tips.ytdl_list_key", 'l'),
            ("gui.tips.ytdl_format", 'f'),
            ("gui.tips.ytdl_dismiss", 'x'),
            ("gui.tips.ytdl_queue", 'a'),
            ("gui.tips.ytdl_more", 'm'),
        ];
        for code in LOCALES {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            for (key, letter) in words {
                assert!(t(key).starts_with(&format!("{letter} ")), "{code}: {key} is {}", t(key));
            }
            // The list's footer opens with the kit's "Esc close", the part
            // that is a button (keep::draw_borders).
            assert!(t("gui.ytdl.list.esc_going").starts_with(&t("gui.discover.keep.esc_close")), "{code}");
        }
        for (key, letter) in &words[..9] {
            assert!(ROOM_LETTERS.contains(letter), "{key}: the room keeps {} too", letter.to_ascii_uppercase());
        }
        assert!(ROOM_LETTERS.contains(&'l'), "the list's l");
    }

    /// Every language in the cells the boards give it (Keep a copy's §9
    /// habit): each whole tips line in the footer's 99, and what a composed
    /// one always keeps once its key words gave way; the tall primary
    /// beside `OGG Vorbis ▾` at 47 cells (T3); the window's action row and
    /// its four dots in the text's 72 (F3, C1); the doorway's label short
    /// of the description column, its news words in what 47 cells leave
    /// (S1–S4); the title row's word beside the title at 47 (T3); a list
    /// row's right-hand words with a crumb's room left (L1), its
    /// ways on a row of their own (L2); the note's head with the boards'
    /// song (N2); the tags' labels inside their column's clamp (C1).
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
                "gui.tips.ytdl_here_layout",
                "gui.tips.ytdl_where_layout",
                "gui.tips.ytdl_choose",
            ] {
                assert!(cells(&t(key)) <= 99, "{code}: {key} is {} cells", cells(&t(key)));
            }
            let room_keys = ["gui.ytdl.key.edit", "gui.ytdl.key.change", "gui.ytdl.key.retry", "gui.ytdl.key.type", "gui.ytdl.key.use", "gui.ytdl.key.undo", "gui.ytdl.key.choose"];
            let ways = ["gui.ytdl.list.try_again", "gui.ytdl.list.change_format", "gui.ytdl.list.choose_folder", "gui.ytdl.list.top", "gui.ytdl.list.check_again", "gui.ytdl.list.add_again", "gui.ytdl.list.dismiss"];
            let enter = rust_i18n::t!("gui.tips.ytdl_enter", locale = code, verb = widest(&ways)).to_string();
            for line in [
                vec![t("gui.tips.ytdl_paste"), widest(&room_keys), t("gui.tips.ytdl_enter_add"), t("gui.tips.ytdl_esc_back")],
                vec![t("gui.tips.ytdl_enter_here"), widest(&[&room_keys[..], &["gui.ytdl.key.subfolders"][..]].concat()), t("gui.tips.ytdl_paste"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_here_dot"), t("gui.tips.ytdl_enter_library"), t("gui.tips.ytdl_close")],
                vec![t("gui.ytdl.key.format"), t("gui.tips.ytdl_enter_anyway"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_rows"), enter.clone(), t("gui.tips.ytdl_dismiss"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_rows"), t("gui.tips.ytdl_play"), t("gui.tips.ytdl_queue"), t("gui.tips.ytdl_more"), t("gui.tips.ytdl_close")],
                vec![t("gui.tips.ytdl_where_library"), t("gui.tips.ytdl_where_rows"), t("gui.tips.ytdl_where_save"), t("gui.tips.ytdl_close")],
            ] {
                let line = line.join(" · ");
                assert!(cells(&line) <= 99, "{code}: {line} is {} cells", cells(&line));
            }

            let format = cells("OGG Vorbis ▾");
            for key in ["gui.ytdl.add.to_library", "gui.ytdl.add.anyway", "gui.ytdl.add.adding"] {
                let label = format!("{} ▸", t(key));
                assert!(usize::from(crate::kit::tall_width(&label)) + 1 + format <= 47, "{code}: {label} beside {format} cells");
            }
            let action = format + 3 + cells(&t("gui.ytdl.key.format")) + 3 + cells(&format!("[ {} ]", t("gui.ytdl.add.anyway")));
            assert!(action <= 72, "{code}: the action row is {action} cells");
            let sorts = ["gui.ytdl.sort.here", "gui.ytdl.sort.album", "gui.ytdl.sort.artist_album", "gui.ytdl.sort.custom"];
            let dots: usize = sorts.iter().map(|key| 4 + cells(&t(key))).sum();
            assert!(dots + 3 * 3 <= 72, "{code}: the dots are {} cells", dots + 9);

            let label = format!("{} ▸", t("gui.set.yt_add"));
            assert!(cells(&label) + 2 <= 27, "{code}: {label} pushes the description column");
            let news = [t("gui.ytdl.word.adding"), format!("! {}", t("gui.ytdl.word.failed")), format!("✓ {}", t("gui.ytdl.word.added"))];
            for word in &news {
                assert!(27 + cells(word) <= 47, "{code}: {word} at 47 cells");
            }
            let title = cells(&t("gui.ytdl.room.title"));
            for word in news.iter().chain([&t("gui.ytdl.word.history")]) {
                assert!(2 + title + 2 + cells(word) < 47, "{code}: {word} beside the title, one cell in, at 47 cells");
            }
            let right = [
                format!("{} · {} · Opus", t("gui.ytdl.list.adding"), rust_i18n::t!("gui.ytdl.list.minutes", locale = code, count = 59)),
                t("gui.ytdl.list.starting"),
                t("gui.ytdl.list.not_sure_started"),
                t("gui.ytdl.list.not_sure_finished"),
                t("gui.ytdl.list.waiting"),
                t("gui.ytdl.list.failed"),
                t("gui.ytdl.list.file_missing"),
                format!("Opus · {}", rust_i18n::t!("gui.ytdl.list.min_ago", locale = code, count = 59)),
                format!("Opus · {}", rust_i18n::t!("gui.ytdl.list.hours_ago", locale = code, count = 23)),
            ];
            for words in &right {
                // 72 less the mark, the title's 20 and two gaps, three to the
                // crumb, and eight for `… › Disc`.
                assert!(cells(words) <= 72 - 24 - 3 - 8, "{code}: {words} crowds the place");
            }
            for set in [&[ways[3], ways[2], ways[6]][..], &[ways[0], ways[2], ways[6]], &[ways[1], ways[6]]] {
                let row = set.iter().map(|key| t(key)).collect::<Vec<_>>().join(" · ");
                assert!(cells(&row) <= 72 - 3, "{code}: {row}");
            }
            assert!(cells(&t("gui.ytdl.list.esc_going")) <= 50, "{code}: the list's footer");

            // N1 and N2: the note's head whole in its 30 with the boards' song.
            for key in ["gui.ytdl.note.adding", "gui.ytdl.note.added"] {
                let note = rust_i18n::t!(key, locale = code, title = "One More Time").to_string();
                assert!(cells(&note) <= 30, "{code}: {note} is {} cells", cells(&note));
            }
            let tags = TAGS.iter().map(|f| cells(&t(tag_key(*f)))).max().unwrap_or(0);
            assert!(tags + 2 <= 18, "{code}: the tags' labels");

            // Sentences that wrap rather than clip, kept to two lines: still
            // looking beside `t type the details` (the room's 80, the
            // window's 72), the late answer beside `u use them` and why Add
            // is off at 47.
            let lines = |key: &str, room: usize| crate::kit::wrap_whole(&t(key), room).len();
            for room in [80, 73] {
                let words = room - 1 - 13 - cells(&t("gui.ytdl.key.type"));
                assert!(lines("gui.ytdl.room.slow", words) <= 2, "{code}: still looking at {room}");
            }
            assert!(lines("gui.ytdl.room.late", 46 - 2 - cells(&t("gui.ytdl.key.use"))) <= 2, "{code}: the late answer at 47");
            assert!(lines("gui.ytdl.where.fix_layout", 46) <= 2, "{code}: why Add is off at 47");
        }
    }

    /// Every language drawn: the doorway's label whole with its description
    /// at the column (100 × 30); at 47 cells with the queue open the room
    /// keeps T3's key words and title-row word whole and its tips line in
    /// the footer's 99; the list's ways and its tips likewise.
    #[test]
    fn every_locale_draws_the_room_and_the_list_whole() {
        let Some(code) = crate::setup::tests::in_own_process(&LOCALES) else { return };
        let mut gui = seated(1);
        let rows = draw(&mut gui);
        let label = format!("{} {}", t!("gui.set.yt_add"), super::super::forward_glyph());
        let y = rows.iter().position(|r| r.replace(' ', "").contains(&label.replace(' ', ""))).unwrap_or_else(|| panic!("{code}: no doorway"));
        let desc = t!("gui.set.yt_add_desc").to_string();
        let shown: String = rows[y].chars().skip(17 + 27).collect();
        assert!(shown.starts_with(desc.chars().next().unwrap()), "{code}: the description at the column: {shown:?}");

        let mut gui = room();
        seed(&mut gui, vec![history()]);
        gui.queue_open = true;
        found(&mut gui, one_more_time(), discovery_new());
        gui.ytdl.draft.keys = Keys::None;
        let rows = draw(&mut gui);
        for words in [t!("gui.ytdl.key.edit"), t!("gui.ytdl.key.change"), t!("gui.ytdl.word.history"), t!("gui.ytdl.add.to_library")] {
            assert!(whole(&rows, &words, (17, 64)), "{code}: {words} whole at 47 cells:\n{}", rows.join("\n"));
        }
        let line = tips(&gui);
        assert!(width(&line) <= 99 && line.ends_with(&*t!("gui.tips.ytdl_esc_back")), "{code}: {line}");
        assert!(line.contains(&*t!("gui.tips.ytdl_enter_add")), "{code}: Enter stays: {line}");

        let mut landed = job("k2", "failed", "One More Time", "/Music/Daft Punk/Discovery/");
        landed["failure"] = serde_json::json!({ "problem": "exists", "phase": "download", "detail": "One_More_Time.mp3" });
        let mut gui = list_gui(vec![landed], None);
        let rows = draw(&mut gui);
        let ways = format!("{} · {}", t!("gui.ytdl.list.change_format"), t!("gui.ytdl.list.dismiss"));
        assert!(whole(&rows, &ways, (19, 95)), "{code}: {ways}:\n{}", rows.join("\n"));
        assert!(whole(&rows, &t!("gui.ytdl.list.failed"), (19, 95)), "{code}");
        key(&mut gui, KeyCode::Down);
        let line = tips(&gui);
        assert!(width(&line) <= 99 && line.ends_with(&*t!("gui.tips.ytdl_close")), "{code}: {line}");

        // A sentence shown whole however it wraps — one with no blanks (ja,
        // zh) included — beside the key words on its rows.
        let shows = |rows: &[String], text: &str, (x0, x1): (usize, usize), beside: &[&str]| {
            let mut packed: String =
                rows.iter().map(|r| r.chars().skip(x0).take(x1 - x0).collect::<String>()).collect::<String>().replace(' ', "");
            for words in beside {
                packed = packed.replacen(&words.replace(' ', ""), "", 1);
            }
            packed.contains(&text.replace(' ', ""))
        };
        // B2, the room's 80 cells and the window's 72: still looking.
        let mut gui = room();
        paste(&mut gui, LINK);
        advance(&gui, 8_000);
        poll(&mut gui);
        let rows = draw(&mut gui);
        let (slow, typed) = (t!("gui.ytdl.room.slow").to_string(), t!("gui.ytdl.key.type").to_string());
        assert!(whole(&rows, &typed, (17, 97)), "{code}: {typed}:\n{}", rows.join("\n"));
        assert!(shows(&rows, &slow, (17, 97), &[&typed]), "{code}: {slow}:\n{}", rows.join("\n"));
        let mut gui = super::super::ytdl_here::tests::files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        paste(&mut gui, LINK);
        advance(&gui, 8_000);
        poll(&mut gui);
        let rows = draw(&mut gui);
        assert!(whole(&rows, &typed, (20, 94)), "{code}: {typed} in the window:\n{}", rows.join("\n"));
        assert!(shows(&rows, &slow, (20, 94), &[&typed]), "{code}: {slow} in the window:\n{}", rows.join("\n"));
        // B3 at 47 cells: the late answer beside `u use them`.
        let mut gui = room();
        gui.queue_open = true;
        paste(&mut gui, LINK);
        advance(&gui, 8_000);
        poll(&mut gui);
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('t'));
        type_text(&mut gui, "One More Time");
        key(&mut gui, KeyCode::Esc);
        answer_lookup(&mut gui, Ok(one_more_time()));
        let rows = draw(&mut gui);
        let (late, used) = (t!("gui.ytdl.room.late").to_string(), t!("gui.ytdl.key.use").to_string());
        assert!(whole(&rows, &used, (17, 64)), "{code}: {used} at 47 cells:\n{}", rows.join("\n"));
        assert!(shows(&rows, &late, (17, 64), &[&used]), "{code}: {late} at 47 cells:\n{}", rows.join("\n"));
        // B4: a lookup the server couldn't read.
        let mut gui = room();
        paste(&mut gui, LINK);
        answer_lookup(&mut gui, Err(YtdlCallError::Answered { status: 500, words: None, reason: None, file_name: None }));
        let rows = draw(&mut gui);
        let failed = t!("gui.ytdl.lookup.failed").to_string();
        assert!(shows(&rows, &format!("! {failed}"), (17, 97), &[]), "{code}: {failed}:\n{}", rows.join("\n"));
        // A row that didn't start, inside the list's 72.
        let mut start = job("s1", "startFailed", "One More Time", "/Music/Daft Punk/Discovery/");
        start["failure"] = serde_json::json!({ "problem": "ffmpegNotReady", "phase": "start" });
        let mut gui = list_gui(vec![start], None);
        let rows = draw(&mut gui);
        let ffmpeg = t!("gui.ytdl.err.ffmpeg").to_string();
        assert!(shows(&rows, &ffmpeg, (20, 94), &[]), "{code}: {ffmpeg} inside the list:\n{}", rows.join("\n"));
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

    /// The canvas's frames, row by row from their first cell (trailing
    /// blanks dropped), and their tokens as `row:first-last:TOKEN[:B]` —
    /// read off the boards' cells (TermMain, TermRoomStates, TermList).
    #[rustfmt::skip]
    mod boards {
        pub(super) const T1: &str = r#"
            |PLAYBACK
            |Blend length   -   off  +  tracks blend as one ends · 0 cuts over
            |[✓] Gapless                sample-tight boundaries when no blend is set
            |[ ] Blend skips            manual skips blend for a second
            |[ ] Pause fade             pause and resume ride a short ramp
            |[✓] Resume queue           the queue and your place come back on launch
            |
            |SERVERS
            |Manage servers ▸           add, switch, edit · pair a phone
            |
            |ADD MUSIC
            |Add a torrent ▸            a .torrent file or a magnet link, into a library
            |Add from YouTube ▸         a YouTube link, into a library
            |[✓] Ask what to do with torrents  when one is opened with mStream, offer to han…
            |
            |DISPLAY
            |[ ] Keyboard hints         show the keys in the footer and on the tooltips
        "#;
        pub(super) const T1_RUNS: &[&str] = &[
            "0:0-7:Dim", "1:0-24:Text", "1:27-64:Dim", "2:0-10:Text", "2:27-70:Dim", "3:0-14:Text", "3:27-57:Dim",
            "4:0-13:Text", "4:27-60:Dim", "5:0-15:Text", "5:27-70:Dim", "7:0-6:Dim", "8:0-15:Text", "8:27-58:Dim",
            "10:0-8:Dim", "11:0-14:Text", "11:27-74:Dim", "12:0-17:Bright:B", "12:27-56:Bright", "13:0-31:Text",
            "13:34-79:Dim", "15:0-6:Dim", "16:0-17:Text", "16:27-73:Dim",
        ];
        pub(super) const T2: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const T2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text", "10:42-44:Dim",
            "10:71-78:Dim", "20:58-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B",
            "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const T3: &str = r#"
            |◂ Add from YouTube                 added songs
            |
            |╭─────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8            [X] │
            |╰─────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001       e edit tags
            |Tags from YouTube Music
            |
            |Files go to … › Discovery new         d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                         ╭────────────────────╮
            |MP3 ▾                    │  Add to library ▸  │
            |                         ╰────────────────────╯
        "#;
        pub(super) const T3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:35-45:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:42-44:Dim",
            "3:46-46:Dim", "4:0-46:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:35-45:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-12:Dim", "10:14-14:Dim", "10:16-24:Text", "10:26-28:Dim", "10:38-45:Dim", "20:25-46:Accent",
            "21:0-2:Text", "21:4-4:Dim", "21:25-25:Accent", "21:26-45:Accent:B", "21:46-46:Accent", "22:25-46:Accent",
        ];
        pub(super) const T4: &str = r#"
            |◂ Add from YouTube                 added songs
            |
            |╭─────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8            [X] │
            |╰─────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001       e edit tags
            |Tags from YouTube Music
            |
            |Files go to … › Discovery new         d change
            |
            |
            |
            |                         ╭────────────────────╮
            |MP3 ▾                    │  Add to library ▸  │
            |                         ╰────────────────────╯
        "#;
        pub(super) const T4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:35-45:Dim", "2:0-46:Dim", "3:0-0:Dim", "3:2-29:Text", "3:42-44:Dim",
            "3:46-46:Dim", "4:0-46:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:35-45:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-12:Dim", "10:14-14:Dim", "10:16-24:Text", "10:26-28:Dim", "10:38-45:Dim", "14:25-46:Accent",
            "15:0-2:Text", "15:4-4:Dim", "15:25-25:Accent", "15:26-45:Accent:B", "15:46-46:Accent", "16:25-46:Accent",
        ];

        pub(super) const A1: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ ▏Paste a YouTube link                                                        │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const A1_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Accent", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:79-79:Accent", "4:0-79:Accent",
        ];
        pub(super) const A2: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://soundcloud.com/daft-punk/one-more-time▏                          [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |! That's not a YouTube link. Paste one from youtube.com or youtu.be.
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const A2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Accent", "3:0-0:Accent", "3:2-47:Text", "3:48-48:Text",
            "3:75-77:Dim", "3:79-79:Accent", "4:0-79:Accent", "5:0-67:Gold",
        ];
        pub(super) const A3: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtube.com/playlist?list=PLSdoVPM5WnndSQEXRz704yQkKwx76GvPV▏    [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |! That's a playlist. Open one video in it and copy that link.
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const A3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Accent", "3:0-0:Accent", "3:2-69:Text", "3:70-70:Text",
            "3:75-77:Dim", "3:79-79:Accent", "4:0-79:Accent", "5:0-60:Gold",
        ];
        pub(super) const A4: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://www.youtube.com/watch?v=FGBhQbmPwH8&list=RDFGBhQbmPwH8           [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |Just this video, not the playlist.
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const A4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-63:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "5:0-33:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim",
            "10:0-10:Dim", "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text",
            "10:42-44:Dim", "10:71-78:Dim", "20:58-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent",
            "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const A5: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://www.youtube.com/live/FGBhQbmPwH8                                 [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |! Live streams can't be saved. If this one has ended, it may still work.
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const A5_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-41:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "5:0-71:Gold", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim",
            "10:0-10:Dim", "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text",
            "10:42-44:Dim", "10:71-78:Dim", "20:58-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent",
            "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const B1: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |▱▱▱▱▱▱▱▱▱▱ Reading the video…
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const B1_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-9:Dim", "6:11-28:Accent",
        ];
        pub(super) const B2: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |▱▱▱▱▱▱▱▱▱▱ Still looking — YouTube can be slow.              t type the details
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const B2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-9:Dim", "6:11-46:Accent", "6:61-78:Dim",
        ];
        pub(super) const B3: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |Details found for this video                                         u use them
            |TITLE   One More Time
            |ARTIST  Daft Punk
            |ALBUM
            |YEAR
            |
            |
            |Files go to Music › Daft Punk                                          d change
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const B3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "5:0-27:Dim", "5:69-78:Dim", "6:0-4:Dim", "6:8-20:Text", "7:0-5:Dim",
            "7:8-16:Text", "8:0-4:Dim", "9:0-3:Dim", "12:0-10:Dim", "12:12-16:Text", "12:18-18:Dim", "12:20-28:Text",
            "12:71-78:Dim", "20:58-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B",
            "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const B4: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |! Couldn't read this video's details. It may be private, age-restricted or
            |  blocked for the server.
            |
            |TITLE   ▏
            |ARTIST
            |ALBUM
            |YEAR
            |Blank: the file gets the video's own title.
            |
            |Files go to Music
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const B4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-73:Gold", "7:0-24:Gold", "9:0-4:Accent:B", "9:8-8:Text", "10:0-5:Dim",
            "11:0-4:Dim", "12:0-3:Dim", "13:0-42:Dim", "15:0-10:Dim", "15:12-16:Text", "20:58-79:Accent", "21:0-2:Text",
            "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const B5: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |! The server took too long — it may still be setting up its downloader.
            |a try again
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const B5_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-70:Gold", "7:0-10:Dim",
        ];
        pub(super) const C1: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |TITLE   One More Time▏
            |ARTIST  Daft Punk
            |ALBUM   Discovery
            |YEAR    2001
            |From YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const C1_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-4:Accent:B", "6:8-20:Text", "6:21-21:Text", "7:0-5:Dim", "7:8-16:Text",
            "8:0-4:Dim", "8:8-16:Text", "9:0-3:Dim", "9:8-11:Text", "10:0-17:Dim", "12:0-10:Dim", "12:12-16:Text",
            "12:18-18:Dim", "12:20-28:Text", "12:30-30:Dim", "12:32-40:Text", "12:42-44:Dim", "20:58-79:Accent",
            "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const C2: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · No album · 2009                                         e edit tags
            |Cleaned from “Daft Punk - One More Time (Official Video)”                u undo
            |
            |Files go to Music › Daft Punk                                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const C2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-26:Dim", "7:68-78:Dim", "8:0-56:Dim", "8:73-78:Dim",
            "10:0-10:Dim", "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:71-78:Dim", "20:58-79:Accent",
            "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const C3: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · No album · 2009                                         e edit tags
            |! Artist may be the channel name — check it
            |
            |Files go to Music › Daft Punk                                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const C3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-26:Dim", "7:68-78:Dim", "8:0-42:Gold", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:71-78:Dim", "20:58-79:Accent", "21:0-2:Text",
            "21:4-4:Dim", "21:58-58:Accent", "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const C4: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery                              d change
            |! Something like “One_More_Time.mp3” looks to be in this folder already.
            |f change format
            |
            |
            |
            |
            |
            |
            |
            |                                                              ╭────────────────╮
            |MP3 ▾                                                         │  Add anyway ▸  │
            |                                                              ╰────────────────╯
        "#;
        pub(super) const C4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text", "10:71-78:Dim",
            "11:0-71:Gold", "12:0-14:Dim", "20:62-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:62-62:Accent",
            "21:63-78:Accent:B", "21:79-79:Accent", "22:62-79:Accent",
        ];
        pub(super) const C5: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |! “Discovery” doesn't exist yet and your account can't make folders.
            |i save to Music › Daft Punk instead   c choose a folder                d change
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library    │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const C5_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-67:Gold",
            "11:0-34:Dim", "11:38-54:Dim", "11:71-78:Dim", "20:58-79:Dim", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Dim",
            "21:59-78:Dim", "21:79-79:Dim", "22:58-79:Dim",
        ];
        pub(super) const D1: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |! This server can't download from YouTube yet. Ask its admin to set up yt-dlp.
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                          ╭────────────────────╮
            |                                                          │  Add to library    │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const D1_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-77:Gold", "20:58-79:Dim", "21:58-58:Dim", "21:59-78:Dim", "21:79-79:Dim",
            "22:58-79:Dim",
        ];
        pub(super) const D2: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |! Can't reach Home server. Check that it's running, then try again.
            |a try again
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const D2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-66:Gold", "7:0-10:Dim",
        ];
        pub(super) const D3: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |! The server can't convert audio right now. Try again later, or ask its admin.
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library ▸  │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const D3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text", "10:42-44:Dim",
            "10:71-78:Dim", "19:0-77:Gold", "20:58-79:Accent", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Accent",
            "21:59-78:Accent:B", "21:79-79:Accent", "22:58-79:Accent",
        ];
        pub(super) const D4: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery                              d change
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                  Already adding to this folder
            |                                                          ╭────────────────────╮
            |MP3 ▾                                                     │  Add to library    │
            |                                                          ╰────────────────────╯
        "#;
        pub(super) const D4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text", "10:71-78:Dim",
            "19:50-78:Dim", "20:58-79:Dim", "21:0-2:Text", "21:4-4:Dim", "21:58-58:Dim", "21:59-78:Dim", "21:79-79:Dim",
            "22:58-79:Dim",
        ];
        pub(super) const D5: &str = r#"
            |◂ Add from YouTube                                                  added songs
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ https://youtu.be/FGBhQbmPwH8                                             [X] │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |One More Time
            |Daft Punk · Discovery · 2001                                        e edit tags
            |Tags from YouTube Music
            |
            |Files go to Music › Daft Punk › Discovery new                          d change
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |                                                                   ╭───────────╮
            |MP3 ▾                                                              │  Adding…  │
            |                                                                   ╰───────────╯
        "#;
        pub(super) const D5_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:68-78:Dim", "2:0-79:Dim", "3:0-0:Dim", "3:2-29:Text", "3:75-77:Dim",
            "3:79-79:Dim", "4:0-79:Dim", "6:0-12:Text:B", "7:0-27:Dim", "7:68-78:Dim", "8:0-22:Dim", "10:0-10:Dim",
            "10:12-16:Text", "10:18-18:Dim", "10:20-28:Text", "10:30-30:Dim", "10:32-40:Text", "10:42-44:Dim",
            "10:71-78:Dim", "20:67-79:Dim", "21:0-2:Text", "21:4-4:Dim", "21:67-67:Dim", "21:68-78:Dim", "21:79-79:Dim",
            "22:67-79:Dim",
        ];
        pub(super) const E1: &str = r#"
            |◂ Add from YouTube                                                       adding
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ ▏Paste a YouTube link                                                        │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const E1_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:73-78:Accent", "2:0-79:Accent", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:79-79:Accent", "4:0-79:Accent",
        ];
        pub(super) const E1_NOTE: &str = r#"
            |Adding “One More Time”…
        "#;
        pub(super) const E1_NOTE_RUNS: &[&str] = &[
            "0:0-22:Accent",
        ];
        pub(super) const E2: &str = r#"
            |◂ Add from YouTube                                                      ✓ added
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ ▏Paste a YouTube link                                                        │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const E2_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:72-78:Ok", "2:0-79:Accent", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:79-79:Accent", "4:0-79:Accent",
        ];
        pub(super) const E2_NOTE: &str = r#"
            |Added “One More Time”
        "#;
        pub(super) const E2_NOTE_RUNS: &[&str] = &[
            "0:0-20:Ok",
        ];
        pub(super) const E3: &str = r#"
            |◂ Add from YouTube                                                     ! failed
            |
            |╭──────────────────────────────────────────────────────────────────────────────╮
            |│ ▏Paste a YouTube link                                                        │
            |╰──────────────────────────────────────────────────────────────────────────────╯
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
            |
        "#;
        pub(super) const E3_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:71-78:Gold", "2:0-79:Accent", "3:0-0:Accent", "3:2-2:Text", "3:3-22:Dim",
            "3:79-79:Accent", "4:0-79:Accent",
        ];
        pub(super) const E3_NOTE: &str = r#"
            |Couldn't add “One More Time”
        "#;
        pub(super) const E3_NOTE_RUNS: &[&str] = &[
            "0:0-27:Gold",
        ];

        pub(super) const L1: &str = r#"
            |╭─ From YouTube ───────────────────────────────────────────────────────────╮
            |│ ↓ One More Time         … › Daft Punk › Discovery   adding · 2 min · MP3 │
            |│ ◷ Aerodynamic           Music › Daft Punk › Discovery          starting… │
            |│ ! Around the World      Music › Daft Punk › Homework              failed │
            |│   the server said: “Sign in to confirm you’re not a bot.”                │
            |│                                                      Try again · Dismiss │
            |│ ✓ Digital Love          Music › Daft Punk › Discovery      MP3 · 1 h ago │
            |│ ✓ Harder, Better, Fas…  Music › Daft Punk › Discovery       MP3 · Oct 31 │
            |│ ✓ Aerodynamic           Mixes                              Opus · Oct 28 │
            |│ ✓ Around the World      Mixes                               file missing │
            |│                                                                          │
            |│                                                             Show earlier │
            |╰───────────────────────────────────────────── Esc close · it keeps going ─╯
        "#;
        pub(super) const L1_RUNS: &[&str] = &[
            "0:0-2:Accent", "0:3-14:Text:B", "0:15-75:Accent", "1:0-0:Accent", "1:2-2:Accent", "1:4-16:Text",
            "1:26-50:Dim", "1:54-59:Accent", "1:60-73:Dim", "1:75-75:Accent", "2:0-0:Accent", "2:2-2:Dim", "2:4-14:Text",
            "2:26-54:Dim", "2:65-73:Dim", "2:75-75:Accent", "3:0-0:Accent", "3:2-2:Gold", "3:4-19:Text", "3:26-53:Dim",
            "3:68-73:Gold", "3:75-75:Accent", "4:0-0:Accent", "4:4-58:Dim", "4:75-75:Accent", "5:0-0:Accent",
            "5:55-63:Accent", "5:64-66:Dim", "5:67-73:Accent", "5:75-75:Accent", "6:0-0:Accent", "6:2-2:Ok",
            "6:4-15:Text", "6:26-54:Dim", "6:61-73:Dim", "6:75-75:Accent", "7:0-0:Accent", "7:2-2:Ok", "7:4-23:Text",
            "7:26-54:Dim", "7:62-73:Dim", "7:75-75:Accent", "8:0-0:Accent", "8:2-2:Ok", "8:4-14:Text", "8:26-30:Dim",
            "8:61-73:Dim", "8:75-75:Accent", "9:0-0:Accent", "9:2-2:Dim", "9:4-19:Dim", "9:26-30:Dim", "9:62-73:Dim",
            "9:75-75:Accent", "10:0-0:Accent", "10:75-75:Accent", "11:0-0:Accent", "11:62-73:Dim", "11:75-75:Accent",
            "12:0-46:Accent", "12:47-72:Dim", "12:73-75:Accent",
        ];
        pub(super) const L2: &str = r#"
            |╭─ From YouTube ───────────────────────────────────────────────────────────╮
            |│ ? Digital Love          … › Daft Punk › Discovery    not sure it started │
            |│                                                              Check again │
            |│ ! One More Time         Music › Daft Punk › Discovery             failed │
            |│   “One_More_Time.mp3” landed here meanwhile · nothing was replaced       │
            |│                                        Change format and retry · Dismiss │
            |│ ! Harder, Better, Fas…  Music › Daft Punk › Discovery             failed │
            |│   not found on the server                            Add again · Dismiss │
            |╰────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const L2_RUNS: &[&str] = &[
            "0:0-2:Accent", "0:3-14:Text:B", "0:15-75:Accent", "1:0-0:Accent", "1:2-2:Dim", "1:4-15:Text", "1:26-50:Dim",
            "1:55-73:Dim", "1:75-75:Accent", "2:0-0:Accent", "2:63-73:Accent", "2:75-75:Accent", "3:0-0:Accent",
            "3:2-2:Gold", "3:4-16:Text", "3:26-54:Dim", "3:68-73:Gold", "3:75-75:Accent", "4:0-0:Accent", "4:4-67:Dim",
            "4:75-75:Accent", "5:0-0:Accent", "5:41-63:Accent", "5:64-66:Dim", "5:67-73:Accent", "5:75-75:Accent",
            "6:0-0:Accent", "6:2-2:Gold", "6:4-23:Text", "6:26-54:Dim", "6:68-73:Gold", "6:75-75:Accent", "7:0-0:Accent",
            "7:4-26:Dim", "7:55-63:Accent", "7:64-66:Dim", "7:67-73:Accent", "7:75-75:Accent", "8:0-63:Accent",
            "8:64-72:Dim", "8:73-75:Accent",
        ];
        pub(super) const L3: &str = r#"
            |╭─ From YouTube ───────────────────────────────────────────────────────────╮
            |│ ✓ One More Time         Music › Daft Punk › Discovery    MP3 · 3 min ago │
            |│ ✓ Digital Love          Music › Daft Punk › Discovery        [▸] [+] [⋯] │
            |│ ✓ Harder, Better, Fas…  Music › Daft Punk › Discovery       MP3 · Oct 31 │
            |│ ✓ Aerodynamic           Mixes                              Opus · Oct 28 │
            |│ ✓ Around the World      Mixes                               file missing │
            |│                                                                          │
            |│                                                             Show earlier │
            |╰────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const L3_RUNS: &[&str] = &[
            "0:0-2:Accent", "0:3-14:Text:B", "0:15-75:Accent", "1:0-0:Accent", "1:2-2:Ok", "1:4-16:Text", "1:26-54:Dim",
            "1:59-73:Dim", "1:75-75:Accent", "2:0-0:Accent", "2:2-2:Ok", "2:4-15:Bright:B", "2:26-54:Dim", "2:63-65:Dim",
            "2:67-69:Dim", "2:71-73:Dim", "2:75-75:Accent", "3:0-0:Accent", "3:2-2:Ok", "3:4-23:Text", "3:26-54:Dim",
            "3:62-73:Dim", "3:75-75:Accent", "4:0-0:Accent", "4:2-2:Ok", "4:4-14:Text", "4:26-30:Dim", "4:61-73:Dim",
            "4:75-75:Accent", "5:0-0:Accent", "5:2-2:Dim", "5:4-19:Dim", "5:26-30:Dim", "5:62-73:Dim", "5:75-75:Accent",
            "6:0-0:Accent", "6:75-75:Accent", "7:0-0:Accent", "7:62-73:Dim", "7:75-75:Accent", "8:0-63:Accent",
            "8:64-72:Dim", "8:73-75:Accent",
        ];
        pub(super) const L4: &str = r#"
            |╭─ From YouTube ───────────────────────────────────────────────────────────╮
            |│ ✓ One More Time         Music › Daft Punk › Discovery    MP3 · 3 min ago │
            |│ ✓ Digital Love          Music › Daft Punk › Discovery        [▸] [+] [⋯] │
            |│ ✓ Harder, Better, Fas…  Music › Daft Punk › Discovery╭─────────────────╮ │
            |│ ✓ Aerodynamic           Mixes                        │ Add to queue  a │ │
            |│ ✓ Around the World      Mixes                        │ Show in folder  │ │
            |│                                                      ╰─────────────────╯ │
            |│                                                             Show earlier │
            |╰────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const L4_RUNS: &[&str] = &[
            "0:0-2:Accent", "0:3-14:Text:B", "0:15-75:Accent", "1:0-0:Accent", "1:2-2:Ok", "1:4-16:Text", "1:26-54:Dim",
            "1:59-73:Dim", "1:75-75:Accent", "2:0-0:Accent", "2:2-2:Ok", "2:4-15:Bright:B", "2:26-54:Dim", "2:63-65:Dim",
            "2:67-69:Dim", "2:71-73:Bright:B", "2:75-75:Accent", "3:0-0:Accent", "3:2-2:Ok", "3:4-23:Text", "3:26-54:Dim",
            "3:55-73:Accent", "3:75-75:Accent", "4:0-0:Accent", "4:2-2:Ok", "4:4-14:Text", "4:26-30:Dim",
            "4:55-55:Accent", "4:57-68:Text", "4:71-71:Dim", "4:73-73:Accent", "4:75-75:Accent", "5:0-0:Accent",
            "5:2-2:Dim", "5:4-19:Dim", "5:26-30:Dim", "5:55-55:Accent", "5:57-70:Text", "5:73-73:Accent",
            "5:75-75:Accent", "6:0-0:Accent", "6:55-73:Accent", "6:75-75:Accent", "7:0-0:Accent", "7:62-73:Dim",
            "7:75-75:Accent", "8:0-63:Accent", "8:64-72:Dim", "8:73-75:Accent",
        ];
        pub(super) const F1: &str = r#"
            |╭─ From YouTube ───────────────────────────────────────────────────────────╮
            |│ ↓ One More Time         … › Daft Punk › Discovery   adding · 2 min · MP3 │
            |│ ◷ Aerodynamic           Music › Daft Punk › Discovery          starting… │
            |│ ! Around the World      Music › Daft Punk › Homework              failed │
            |│   the server said: “Sign in to confirm you’re not a bot.”                │
            |│                                                      Try again · Dismiss │
            |│ ✓ Digital Love          Music › Daft Punk › Discovery      MP3 · 1 h ago │
            |│ ✓ Harder, Better, Fas…  Music › Daft Punk › Discovery       MP3 · Oct 31 │
            |│ ✓ Aerodynamic           Mixes                              Opus · Oct 28 │
            |│ ✓ Around the World      Mixes                               file missing │
            |│                                                                          │
            |│                                                             Show earlier │
            |╰───────────────────────────────────────────── Esc close · it keeps going ─╯
        "#;
        pub(super) const F1_RUNS: &[&str] = &[
            "0:0-2:Accent", "0:3-14:Text:B", "0:15-75:Accent", "1:0-0:Accent", "1:2-2:Accent", "1:4-16:Text",
            "1:26-50:Dim", "1:54-59:Accent", "1:60-73:Dim", "1:75-75:Accent", "2:0-0:Accent", "2:2-2:Dim", "2:4-14:Text",
            "2:26-54:Dim", "2:65-73:Dim", "2:75-75:Accent", "3:0-0:Accent", "3:2-2:Gold", "3:4-19:Text", "3:26-53:Dim",
            "3:68-73:Gold", "3:75-75:Accent", "4:0-0:Accent", "4:4-58:Dim", "4:75-75:Accent", "5:0-0:Accent",
            "5:55-63:Accent", "5:64-66:Dim", "5:67-73:Accent", "5:75-75:Accent", "6:0-0:Accent", "6:2-2:Ok",
            "6:4-15:Text", "6:26-54:Dim", "6:61-73:Dim", "6:75-75:Accent", "7:0-0:Accent", "7:2-2:Ok", "7:4-23:Text",
            "7:26-54:Dim", "7:62-73:Dim", "7:75-75:Accent", "8:0-0:Accent", "8:2-2:Ok", "8:4-14:Text", "8:26-30:Dim",
            "8:61-73:Dim", "8:75-75:Accent", "9:0-0:Accent", "9:2-2:Dim", "9:4-19:Dim", "9:26-30:Dim", "9:62-73:Dim",
            "9:75-75:Accent", "10:0-0:Accent", "10:75-75:Accent", "11:0-0:Accent", "11:62-73:Dim", "11:75-75:Accent",
            "12:0-46:Accent", "12:47-72:Dim", "12:73-75:Accent",
        ];
        pub(super) const F1_ROOM: &str = r#"
            |◂ Add from YouTube                      adding
            |
            |╭─────────────────────────────────────────────╮
            |│ Paste a YouTube link                        │
        "#;
        pub(super) const F1_ROOM_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-17:Text:B", "0:40-45:Accent", "2:0-46:Dim", "3:0-0:Dim", "3:2-21:Dim", "3:46-46:Dim",
        ];
        pub(super) const S1: &str = r#"
            |ADD MUSIC
            |Add a torrent ▸            a .torrent file or a magnet link, into a library
            |Add from YouTube ▸         a YouTube link, into a library
            |[✓] Ask what to do with torrents  when one is opened with mStream, offer to han…
        "#;
        pub(super) const S1_RUNS: &[&str] = &[
            "0:0-8:Dim", "1:0-14:Text", "1:27-74:Dim", "2:0-17:Text", "2:27-56:Dim", "3:0-31:Text", "3:34-79:Dim",
        ];
        pub(super) const S2: &str = r#"
            |ADD MUSIC
            |Add a torrent ▸            a .torrent file or a magnet link, into a library
            |Add from YouTube ▸         adding
            |[✓] Ask what to do with torrents  when one is opened with mStream, offer to han…
        "#;
        pub(super) const S2_RUNS: &[&str] = &[
            "0:0-8:Dim", "1:0-14:Text", "1:27-74:Dim", "2:0-17:Text", "2:27-32:Accent", "3:0-31:Text", "3:34-79:Dim",
        ];
        pub(super) const S3: &str = r#"
            |ADD MUSIC
            |Add a torrent ▸            a .torrent file or a magnet link, into a library
            |Add from YouTube ▸         ! failed
            |[✓] Ask what to do with torrents  when one is opened with mStream, offer to han…
        "#;
        pub(super) const S3_RUNS: &[&str] = &[
            "0:0-8:Dim", "1:0-14:Text", "1:27-74:Dim", "2:0-17:Text", "2:27-34:Gold", "3:0-31:Text", "3:34-79:Dim",
        ];
        pub(super) const S4: &str = r#"
            |ADD MUSIC
            |Add a torrent ▸            a .torrent file or a magnet link, into a library
            |Add from YouTube ▸         ✓ added
            |[✓] Ask what to do with torrents  when one is opened with mStream, offer to han…
        "#;
        pub(super) const S4_RUNS: &[&str] = &[
            "0:0-8:Dim", "1:0-14:Text", "1:27-74:Dim", "2:0-17:Text", "2:27-33:Ok", "3:0-31:Text", "3:34-79:Dim",
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

    }
}
