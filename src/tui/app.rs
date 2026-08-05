//! TUI state and the rules that change it.
//!
//! Deliberately I/O-free: actions and worker events go in, state changes and
//! [`Effect`]s come out, and the run loop is the only thing that touches
//! channels. That keeps the interesting behaviour — navigation, queue
//! advancement, repeat/shuffle — testable without a terminal or a server.

use std::sync::Arc;

// Key handling lives in `super::keymap` now; the app only meets key events
// in its tests, which drive `map_key` below.
#[cfg(test)]
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::api::types::{Album, DirListing, Genre, Track};
use crate::api::urls;
use crate::dj;
use crate::player::PlayerStatus;

use super::worker::{
    ApiCmd, AudioCmd, AutoDjMode, DiscoverData, DiscoverNode, DjRequest, Event, LibraryData,
    LibraryNode,
};

const SEEK_STEP: f64 = 5.0;
/// The shifted seek keys. Five seconds is the wrong unit for a long mix or a
/// set recording, where the interesting distance is minutes.
const SEEK_STEP_FAR: f64 = 60.0;
const VOLUME_STEP: f32 = 0.05;
/// Rows a page key moves. Ctrl+u/d move half of this, as they do in vim.
const PAGE_STEP: isize = 10;

/// A side effect for the run loop to dispatch to a worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Audio(AudioCmd),
    Api(ApiCmd),
    /// Persist the session after a successful login.
    SaveSession,
    /// Look for servers advertising themselves on the local network.
    Discover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Files,
    Library,
    Playlists,
    Search,
    Discover,
}

impl Tab {
    pub const ALL: [Tab; 5] =
        [Tab::Files, Tab::Library, Tab::Playlists, Tab::Search, Tab::Discover];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Library => "Library",
            Tab::Playlists => "Playlists",
            Tab::Search => "Search",
            Tab::Discover => "Discover",
        }
    }

    /// Whether this server can serve the tab at all. A tab that can only ever
    /// say "not available here" is worse than no tab.
    pub fn available(self, capabilities: crate::api::types::Capabilities) -> bool {
        match self {
            Tab::Discover => capabilities.discovery,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Browser,
    Queue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }

    /// Anything unrecognised falls back to off rather than refusing to start.
    pub fn from_label(label: &str) -> Self {
        match label {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Editing,
    /// A modal overlay is up and owns the keyboard. Its own bindings apply,
    /// so a letter that means "previous track" outside it can mean something
    /// else inside without the two fighting.
    Panel,
    /// The full-screen now-playing view. Unlike [`InputMode::Panel`] this is a
    /// *view*, not a modal: it claims the arrows for its own tabs and falls
    /// through to the normal bindings for everything else, so play/pause,
    /// skip, seek and volume keep working — and keep obeying `[keys]`.
    Now,
}

/// A tab in the full-screen now-playing view.
///
/// Deliberately not [`Tab`]: those are places to browse, these are things to
/// watch while something plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NowTab {
    Queue,
    Lyrics,
    Discover,
    AutoDj,
    Visualizer,
}

pub const NOW_TABS: [NowTab; 5] =
    [NowTab::Queue, NowTab::Lyrics, NowTab::Discover, NowTab::AutoDj, NowTab::Visualizer];

impl NowTab {
    pub fn title(self) -> &'static str {
        match self {
            NowTab::Queue => "Queue",
            NowTab::Lyrics => "Lyrics",
            NowTab::Discover => "Discover",
            NowTab::AutoDj => "Auto-DJ",
            NowTab::Visualizer => "Visualizer",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Quit,
    ToggleHelp,
    CycleFocus,
    SelectTab(usize),
    Up,
    Down,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    First,
    Last,
    Activate,
    Back,
    AddToQueue,
    PlayPause,
    NextTrack,
    PrevTrack,
    SeekForward,
    SeekBackward,
    /// Coarse seek — the same idea shifted, for tracks where five seconds is
    /// not a useful unit.
    SeekForwardFar,
    SeekBackwardFar,
    /// Put the cursor back on the track that's playing.
    JumpToPlaying,
    ToggleNowPlaying,
    NowTabNext,
    NowTabPrev,
    VolumeUp,
    VolumeDown,
    RemoveFromQueue,
    ClearQueue,
    ToggleRepeat,
    ToggleShuffle,
    ToggleAutoDj,
    OpenDjPanel,
    StartJourney,
    StartSearch,
    StartFilter,
    CycleViz,
    ToggleScatter,
    Input(char),
    Backspace,
    Submit,
    Cancel,
}

/// One row in a browser pane. Unifying directories, playlists and tracks means
/// Enter and "add to queue" behave the same way on every tab.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Parent,
    Dir { label: String, path: String },
    /// A step deeper into the tag-based library (an artist, album, genre…).
    Node { label: String, node: LibraryNode },
    Playlist { name: String },
    /// A row in the search-class menu: what it matched on, and how many.
    Search { label: String, detail: String, node: SearchNode },
    /// A step deeper into Discover. `detail` is the dim right-hand column —
    /// how close, how much the guess rests on, what it sounds like.
    Discover { label: String, detail: String, node: DiscoverNode },
    Track { label: String, track: Box<Track> },
}

impl Entry {
    /// What this row is called. Also what a filter matches on: the thing you
    /// can read on screen is the thing you would think to type.
    pub fn label(&self) -> &str {
        match self {
            Entry::Parent => "..",
            Entry::Dir { label, .. } => label,
            Entry::Node { label, .. } => label,
            Entry::Playlist { name } => name,
            Entry::Search { label, .. } => label,
            Entry::Discover { label, .. } => label,
            Entry::Track { label, .. } => label,
        }
    }

    /// `needle` is already lowercase. `..` always survives: it is the way out
    /// of the directory, not a result in it, and filtering yourself into a
    /// listing with no way back is a trap.
    fn matches(&self, needle: &str) -> bool {
        matches!(self, Entry::Parent) || self.label().to_lowercase().contains(needle)
    }
}

/// Which of the five things a search matched on. The server answers all of
/// them at once and they mean different things -- a title hit and a lyrics hit
/// are not the same discovery -- so they get to stay separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchClass {
    Artists,
    Albums,
    Titles,
    Files,
    Lyrics,
}

pub const SEARCH_CLASSES: [SearchClass; 5] = [
    SearchClass::Artists,
    SearchClass::Albums,
    SearchClass::Titles,
    SearchClass::Files,
    SearchClass::Lyrics,
];

impl SearchClass {
    pub fn title(self) -> &'static str {
        match self {
            SearchClass::Artists => "Artists",
            SearchClass::Albums => "Albums",
            SearchClass::Titles => "Titles",
            SearchClass::Files => "Filenames",
            SearchClass::Lyrics => "Lyrics",
        }
    }
}

/// A position in the search results, the same shape the Library tab uses.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchNode {
    /// The list of classes, with how many each matched.
    Root,
    Class(SearchClass),
    /// An artist or album hit, opened. Carries a [`LibraryNode`] because that
    /// is exactly what it is -- the same place, reached from somewhere else.
    Library(LibraryNode),
}

/// A column to the left of the one you are in: a listing you came through,
/// kept so the browser can show where you are as columns rather than as a
/// path in a title bar.
///
/// Display-only, and captured from what was already on screen — walking back
/// up costs no request, and neither does drawing the context.
#[derive(Debug)]
pub struct Trail {
    pub entries: Vec<Entry>,
    /// Which row of this listing was taken to get to the next column.
    pub chosen: usize,
}

#[derive(Debug, Default)]
pub struct Pane {
    pub entries: Vec<Entry>,
    pub state: ListState,
    /// A request for this pane's contents is out. An empty list means
    /// "nothing here" or "not here yet" and the two look identical on
    /// screen, so the difference has to be recorded rather than guessed.
    pub loading: bool,
    /// The columns to the left of this one, innermost last.
    pub trail: Vec<Trail>,
    /// Text narrowing what is shown. Empty when everything is.
    pub filter: String,
    /// The list before the filter, kept so clearing it is instant and nothing
    /// has to be asked for again. `None` when nothing is hidden, so a pane
    /// with no filter — nearly always — holds one copy of its rows.
    unfiltered: Option<Vec<Entry>>,
}

impl Pane {
    pub fn set(&mut self, entries: Vec<Entry>) {
        // A filter describes the list it was typed against. This is a
        // different list, so it goes.
        self.filter.clear();
        self.unfiltered = None;
        self.entries = entries;
        self.rest_cursor();
        // Every reply lands here, so this is the one place that has to
        // remember to stop the spinner.
        self.loading = false;
    }

    /// Put the cursor on the first row worth being on: not "..", so entering
    /// a folder and pressing Enter doesn't just walk back out of it.
    fn rest_cursor(&mut self) {
        let selected = match self.entries.first() {
            None => None,
            Some(Entry::Parent) if self.entries.len() > 1 => Some(1),
            Some(_) => Some(0),
        };
        self.state.select(selected);
    }

    /// Narrow to the rows whose name contains `filter`, ignoring case. An
    /// empty filter puts everything back.
    pub fn apply_filter(&mut self, filter: String) {
        let all = self
            .unfiltered
            .take()
            .unwrap_or_else(|| std::mem::take(&mut self.entries));
        let needle = filter.trim().to_lowercase();
        self.filter = filter;
        if needle.is_empty() {
            self.entries = all;
        } else {
            self.entries = all.iter().filter(|entry| entry.matches(&needle)).cloned().collect();
            self.unfiltered = Some(all);
        }
        self.rest_cursor();
    }

    pub fn clear_filter(&mut self) {
        if !self.filter.is_empty() {
            self.apply_filter(String::new());
        }
    }

    /// How many rows are on screen, and how many there would be with no
    /// filter. `..` counts as neither: it is the way out, not a result.
    pub fn counts(&self) -> (usize, usize) {
        let real = |list: &[Entry]| list.iter().filter(|e| !matches!(e, Entry::Parent)).count();
        let shown = real(&self.entries);
        (shown, self.unfiltered.as_ref().map_or(shown, |all| real(all)))
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.state.selected().and_then(|i| self.entries.get(i))
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.state.select(None);
            return;
        }
        let last = self.entries.len() as isize - 1;
        let current = self.state.selected().unwrap_or(0) as isize;
        self.state.select(Some((current + delta).clamp(0, last) as usize));
    }

    pub fn select_first(&mut self) {
        if !self.entries.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.state.select(Some(self.entries.len() - 1));
        }
    }

    /// Every playable row, plus where `selected` sits among them — used to
    /// enqueue a whole directory starting at the highlighted track.
    pub fn tracks_with_offset(&self) -> (Vec<Track>, usize) {
        let mut tracks = Vec::new();
        let mut offset = 0;
        for (i, entry) in self.entries.iter().enumerate() {
            if let Entry::Track { track, .. } = entry {
                if Some(i) == self.state.selected() {
                    offset = tracks.len();
                }
                tracks.push((**track).clone());
            }
        }
        (tracks, offset)
    }
}

#[derive(Debug, Default)]
pub struct Queue {
    pub items: Vec<Track>,
    pub current: Option<usize>,
    pub state: ListState,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Tracks started since playback last began from nothing. Linear play
    /// can see the end of the queue coming positionally; shuffle has no
    /// position, so its end is this count reaching the queue's length.
    played: usize,
}

impl Default for Repeat {
    fn default() -> Self {
        Repeat::Off
    }
}

impl Queue {
    pub fn replace(&mut self, tracks: Vec<Track>) {
        self.items = tracks;
        self.current = None;
        self.state.select(if self.items.is_empty() { None } else { Some(0) });
    }

    pub fn push(&mut self, track: Track) {
        self.items.push(track);
        if self.state.selected().is_none() {
            self.state.select(Some(0));
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = None;
        self.state.select(None);
    }

    /// Remove `index`, keeping `current` pointing at the same track.
    /// Returns true when the removed entry was the one playing.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.items.remove(index);

        let was_current = match self.current {
            Some(cur) if cur == index => true,
            Some(cur) if cur > index => {
                self.current = Some(cur - 1);
                false
            }
            _ => false,
        };
        if was_current {
            self.current = None;
        }

        if self.items.is_empty() {
            self.state.select(None);
        } else {
            let sel = self.state.selected().unwrap_or(0).min(self.items.len() - 1);
            self.state.select(Some(sel));
        }
        was_current
    }

    /// Note that the track at `index` is starting, and move the cursor to
    /// it. Also the pass clock: starting from nothing deals a fresh pass,
    /// and every start — chosen, skipped to, or advanced to — spends one
    /// of its plays.
    pub fn start(&mut self, index: usize) {
        if self.current.is_none() {
            self.played = 0;
        }
        self.played += 1;
        self.current = Some(index);
        self.state.select(Some(index));
    }

    /// Next track to play. `manual` marks a user-pressed skip, which is never
    /// trapped by repeat-one — the same rule the engine uses.
    pub fn next_index(&self, manual: bool) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let Some(current) = self.current else {
            return Some(0);
        };
        if !manual && self.repeat == Repeat::One {
            return Some(current);
        }
        if self.shuffle {
            // No position to run out of, so the end is counted instead:
            // the pass is over once as many tracks have started as the
            // queue holds. Picks repeat, so this bounds the pass rather
            // than promising that everything in it was played.
            if self.repeat == Repeat::Off && self.played >= self.items.len() {
                return None;
            }
            if self.items.len() <= 1 {
                return Some(0);
            }
            let offset = fastrand::usize(1..self.items.len());
            return Some((current + offset) % self.items.len());
        }
        let next = current + 1;
        if next < self.items.len() {
            Some(next)
        } else if self.repeat == Repeat::All {
            Some(0)
        } else {
            None
        }
    }

    pub fn prev_index(&self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        match self.current {
            Some(0) if self.repeat == Repeat::All => Some(self.items.len() - 1),
            Some(0) | None => Some(0),
            Some(cur) => Some(cur - 1),
        }
    }

    // The cursor, not playback: these move what Enter and `d` act on.

    pub fn select_first(&mut self) {
        if !self.items.is_empty() {
            self.state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.items.is_empty() {
            self.state.select(Some(self.items.len() - 1));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub text: String,
    pub kind: MessageKind,
}

// Auto-DJ and the journey live in `autodj` (audit #57) — named for what it
// does, since `dj` is already the crate's Camelot/BPM helpers. The way in —
// the connect screen, and the [`Session`] it produces — lives in `session`
// (audit #56), re-exported so every caller keeps saying `app::ConnectForm`.
mod autodj;
mod session;
pub use session::{CONNECT_METHODS, ConnectForm, ConnectStage, Session};

/// How many recent tracks to keep for anchoring and cooldown. The sonic
/// centroid takes at most 8 (the server's cap), the artist cooldown at most
/// 20; keeping the longer list serves both.
const RECENT_MEMORY: usize = 20;

/// How many picks the panel's sample takes. Each is its own round trip, so
/// enough to show the character of the settings and no more.
const DJ_SAMPLE_COUNT: usize = 3;

/// One adjustable line in the Auto-DJ panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DjRow {
    Mode,
    Tightness,
    Anchor,
    Tempo,
    Key,
    Rating,
    Cooldown,
    Genres,
}

impl DjRow {
    pub fn label(self) -> &'static str {
        match self {
            DjRow::Mode => "Mode",
            DjRow::Tightness => "Sonic pool",
            DjRow::Anchor => "Anchor",
            DjRow::Tempo => "Tempo window",
            DjRow::Key => "Key matching",
            DjRow::Rating => "Rating floor",
            DjRow::Cooldown => "Artist cooldown",
            DjRow::Genres => "Genres",
        }
    }
}

/// The Auto-DJ panel's own state. Rows shown depend on what the server can
/// do, so the panel is built fresh from capabilities each time it opens.
#[derive(Debug, Default)]
pub struct DjPanel {
    pub rows: Vec<DjRow>,
    pub row: usize,
    /// Genre chooser, when open over the panel.
    pub genres: Option<GenrePicker>,
    /// Sample picks from the current settings, and what the pool looked like.
    pub sample: Vec<Track>,
    pub sample_pending: bool,
    pub pool: Option<crate::api::types::SonicReport>,
}

impl DjPanel {
    fn new(capabilities: crate::api::types::Capabilities) -> Self {
        let mut rows = vec![DjRow::Mode];
        // No index, no pool — and no row promising one.
        if capabilities.discovery {
            rows.push(DjRow::Tightness);
            rows.push(DjRow::Anchor);
        }
        rows.extend([
            DjRow::Tempo,
            DjRow::Key,
            DjRow::Rating,
            DjRow::Cooldown,
            DjRow::Genres,
        ]);
        DjPanel { rows, ..Default::default() }
    }

    pub fn selected(&self) -> DjRow {
        self.rows.get(self.row).copied().unwrap_or(DjRow::Mode)
    }
}

/// A walk from one track to another through the embedding space, waiting to
/// be looked at and queued.
#[derive(Debug)]
pub struct Journey {
    pub from: Track,
    pub to: Track,
    /// Total stops asked for, both ends included.
    pub length: u32,
    pub stops: Vec<crate::api::types::JourneyStop>,
    pub pending: bool,
    /// First visible row, so a long arc can be scrolled.
    pub offset: usize,
}

/// Choosing which genres the filter applies to.
#[derive(Debug, Default)]
pub struct GenrePicker {
    /// Every genre the server knows, alphabetical as it sent them.
    pub all: Vec<String>,
    pub row: usize,
    pub loading: bool,
}

pub struct App {
    /// Which server this is, whose token, as one value — not five parallel
    /// fields that every connect path had to remember together (audit #56).
    pub session: Session,
    pub connected: bool,
    pub connecting: bool,
    pub connect: ConnectForm,

    pub tab: Tab,
    pub focus: Focus,

    pub path: String,
    /// Whether the next listing is the one that gets to say where we are.
    ///
    /// Every other listing has to agree with `path`, which works because
    /// `path` moves when the request goes out rather than when the reply
    /// lands. The browse that opens the browser is the exception both ways:
    /// it asks for [`crate::api::BEST_START`], which only the server can
    /// resolve, or for a remembered path the server may spell back
    /// differently — and there is nothing on screen yet for a wrong answer
    /// to overwrite.
    opening: bool,
    pub files: Pane,
    pub library: Pane,
    /// Breadcrumb through the tag hierarchy; the last element is the view on
    /// screen. Always non-empty once the Library tab has been opened.
    pub library_stack: Vec<LibraryNode>,
    /// The whole search reply, kept rather than flattened. Every class comes
    /// back in one response, so moving between them costs nothing.
    pub search_hits: Option<Box<crate::api::types::SearchResults>>,
    pub search_stack: Vec<SearchNode>,
    /// Whether the queue is showing as the last column. It is the end of the
    /// same chain -- artist, album, track, queued -- so it reads better as one
    /// more column than as a separate pane that is always there.
    pub queue_column: bool,
    pub playlists: Pane,
    pub playlist_open: Option<String>,
    pub discover: Pane,
    /// Breadcrumb through the Discover tab, mirroring `library_stack`.
    pub discover_stack: Vec<DiscoverNode>,
    /// The track every Discover view hangs off. Captured when a view is
    /// opened so the list doesn't quietly re-anchor when the song changes.
    pub discover_seed: Option<Track>,
    /// The last artist list, kept so drilling into one of them costs nothing.
    pub discover_artists: Vec<crate::api::types::SimilarArtist>,
    pub search: Pane,
    pub query: String,
    pub editing_query: bool,
    /// The filter prompt is open and taking keys. Distinct from the filter
    /// itself, which stays applied after you stop typing — the whole point is
    /// to narrow a list and then move around what's left.
    pub filtering: bool,
    pub search_summary: Option<String>,

    pub queue: Queue,
    /// What the connected server offers. Default (nothing) until a ping says
    /// otherwise, so an optional feature is never assumed present.
    pub capabilities: crate::api::types::Capabilities,
    /// The server's libraries, as the ping named them. Empty until it answers,
    /// which is the same as "no reason to stop anywhere in particular".
    libraries: Vec<String>,
    pub autodj: AutoDjMode,
    /// How Auto-DJ chooses, beyond the mode.
    pub dj: dj::Settings,
    /// The settings panel, when it is open.
    pub dj_panel: Option<DjPanel>,
    /// The journey being looked at, when one is.
    pub journey: Option<Journey>,
    /// A journey start chosen but not yet paired with a destination.
    pub journey_from: Option<Track>,
    /// Tracks played recently, newest first. Feeds both the sonic anchor and
    /// the artist cooldown, so the session anchors on where it has been
    /// rather than only on the song currently sounding.
    autodj_recent: Vec<Track>,
    /// Round-trip cursor the random-songs picker uses to avoid repeats.
    autodj_ignore: Vec<u32>,
    /// A request is in flight; don't pile on another.
    autodj_pending: bool,
    pub status: PlayerStatus,
    /// The source asked for but not yet heard back about.
    ///
    /// Playback state arrives a tick behind the decision to play, and opening
    /// a remote track takes as long as it takes. In that gap the last status
    /// still describes the track that just finished, so the transport read
    /// "<the next track> · stopped" at 2:54 of a track nobody was on. Holding
    /// the source we are waiting for lets a status about anything else be
    /// recognised as describing where we no longer are.
    starting: Option<String>,
    /// Tracks that failed to start since the last one that played. Bounds the
    /// skipping so a queue of nothing but broken files stops rather than
    /// looping.
    failures: usize,
    pub volume: f32,
    pub now_playing: Option<Track>,
    pub audio_available: bool,
    /// A copy of the audio coming out of the engine, for the visualiser to
    /// draw. Read-only from here and `None` off the real audio thread, so a
    /// test or a replay run without one draws the same as silence.
    pub tap: Option<Arc<crate::engine::tap::AudioTap>>,
    /// The last place the pointer was seen, when the terminal reports it.
    pub pointer: Option<ratatui::layout::Position>,
    /// Which visualiser is showing, and everything it remembers between
    /// frames — bars fall from where they were, and a spectrogram is nothing
    /// but where it has been.
    pub viz: crate::tui::viz::Visualizer,
    /// The full-screen now-playing view. A view rather than an overlay: every
    /// normal key still means what it meant, so `space`, `n` and the seek keys
    /// keep working while you are looking at it.
    pub fullscreen: bool,
    /// Which frame the spinner is on. Advanced by the event loop against a
    /// wall clock rather than counted per draw, so it turns at one speed
    /// whether the app is idle or flooded — and stays still under test.
    pub spinner: usize,
    pub now_tab: NowTab,
    /// Cursor for whichever now-playing tab is not the queue. The Queue tab
    /// keeps using the queue's own selection, so `d` there removes the row you
    /// are looking at rather than one the other screen had highlighted.
    pub now_scroll: usize,

    pub message: Option<Message>,
    pub show_help: bool,
    pub should_quit: bool,
    /// The bindings in force. Held here rather than looked up globally so the
    /// help screen and the key handler can never disagree about them.
    pub keymap: Keymap,
}

impl App {
    /// Build the app from a saved session, returning the effects needed to get
    /// started (connect immediately, or show the connect form).
    pub fn new(server: Option<String>, token: Option<String>, username: Option<String>) -> Self {
        let mut app = App {
            session: Session {
                server: server.clone().unwrap_or_default(),
                server_id: server.clone().unwrap_or_default(),
                tunnel_code: None,
                token,
                username,
            },
            connected: false,
            connecting: false,
            connect: ConnectForm::default(),
            tab: Tab::Files,
            focus: Focus::Browser,
            path: String::new(),
            opening: true,
            files: Pane::default(),
            library: Pane::default(),
            library_stack: Vec::new(),
            search_hits: None,
            search_stack: Vec::new(),
            queue_column: false,
            playlists: Pane::default(),
            playlist_open: None,
            discover: Pane::default(),
            discover_stack: Vec::new(),
            discover_seed: None,
            discover_artists: Vec::new(),
            search: Pane::default(),
            query: String::new(),
            editing_query: false,
            filtering: false,
            search_summary: None,
            queue: Queue::default(),
            capabilities: Default::default(),
            libraries: Vec::new(),
            autodj: AutoDjMode::Off,
            dj: dj::Settings::default(),
            dj_panel: None,
            journey: None,
            journey_from: None,
            autodj_recent: Vec::new(),
            autodj_ignore: Vec::new(),
            autodj_pending: false,
            status: PlayerStatus::default(),
            starting: None,
            failures: 0,
            volume: 1.0,
            now_playing: None,
            audio_available: true,
            tap: None,
            pointer: None,
            viz: Default::default(),
            fullscreen: false,
            spinner: 0,
            now_tab: NowTab::Queue,
            now_scroll: 0,
            message: None,
            show_help: false,
            should_quit: false,
            keymap: Keymap::default(),
        };
        // A tunnel identity is not an address: it can't be typed, edited or
        // connected to directly, so it stays out of both the endpoint and the
        // form until dialling turns it into a loopback URL.
        if crate::quickconnect::is_tunnel_id(&app.session.server_id) {
            app.session.server.clear();
        } else {
            // Prefill only a server we actually know — a saved session, or one
            // passed on the command line. Guessing localhost just means the
            // first thing a new user does is delete it.
            app.connect.server = server.unwrap_or_default();
        }
        app
    }

    /// Apply the `[keys]` section. Anything wrong with it is reported on the
    /// first screen rather than thrown, because a mistyped binding should
    /// cost you that binding and nothing else.
    pub fn with_keys(
        mut self,
        overrides: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Self {
        if overrides.is_empty() {
            return self;
        }
        let (keymap, warnings) = Keymap::default().with_overrides(overrides);
        self.keymap = keymap;
        if let Some(first) = warnings.first() {
            self.error(match warnings.len() {
                1 => first.clone(),
                n => format!("{first} (and {} more)", n - 1),
            });
        }
        self
    }

    /// Supply the pairing code for a remembered tunnel server, which is what
    /// makes reconnecting to one possible at all.
    pub fn with_tunnel(mut self, code: Option<String>) -> Self {
        self.session.tunnel_code = code;
        self
    }

    /// Start from remembered preferences.
    pub fn with_prefs(mut self, prefs: &crate::config::PlayerPrefs) -> Self {
        self.volume = prefs.volume.clamp(0.0, 1.0);
        self.queue.repeat = Repeat::from_label(&prefs.repeat);
        self.queue.shuffle = prefs.shuffle;
        self.autodj = AutoDjMode::from_label(&prefs.autodj);
        self.dj = dj::Settings::from_prefs(&prefs.dj);
        self
    }

    /// The preferences worth remembering for next time.
    pub fn prefs(&self) -> crate::config::PlayerPrefs {
        crate::config::PlayerPrefs {
            // Rounded because this lands in a file people are meant to read;
            // 0.74999994 is noise from repeated nudges.
            volume: (self.volume * 100.0).round() / 100.0,
            repeat: self.queue.repeat.label().to_string(),
            shuffle: self.queue.shuffle,
            autodj: self.autodj.label().to_string(),
            dj: self.dj.to_prefs(),
        }
    }

    /// Effects to run at startup.
    pub fn start(&mut self) -> Vec<Effect> {
        let effects = self.begin();
        self.note_pending(&effects);
        effects
    }

    pub fn input_mode(&self) -> InputMode {
        if !self.connected || self.editing_query || self.filtering {
            InputMode::Editing
        } else if self.dj_panel.is_some() || self.journey.is_some() {
            // A modal drawn over the full-screen view still owns the keyboard.
            InputMode::Panel
        } else if self.fullscreen {
            InputMode::Now
        } else {
            InputMode::Normal
        }
    }

    /// The now-playing tabs this session can actually fill. Lyrics turns on
    /// the track rather than the server, so the strip changes shape as the
    /// queue moves — which is why nothing may assume its own tab still exists.
    pub fn now_tabs(&self) -> Vec<NowTab> {
        NOW_TABS.iter().copied().filter(|tab| self.now_tab_available(*tab)).collect()
    }

    fn now_tab_available(&self, tab: NowTab) -> bool {
        match tab {
            NowTab::Queue | NowTab::AutoDj | NowTab::Visualizer => true,
            NowTab::Lyrics => {
                self.now_playing.as_ref().is_some_and(|track| track.metadata.has_lyrics)
            }
            NowTab::Discover => self.capabilities.discovery,
        }
    }

    /// The tab on screen. Falls back rather than trusting the stored one: the
    /// track that was carrying the Lyrics tab can end while you are reading it.
    pub fn now_tab(&self) -> NowTab {
        if self.now_tab_available(self.now_tab) { self.now_tab } else { NowTab::Queue }
    }

    fn move_now_tab(&mut self, delta: isize) -> Vec<Effect> {
        let tabs = self.now_tabs();
        let current = self.now_tab();
        let at = tabs.iter().position(|t| *t == current).unwrap_or(0) as isize;
        // Wrap: five tabs and two keys, so walking off one end and coming back
        // round beats making the user turn around.
        let next = (at + delta).rem_euclid(tabs.len() as isize) as usize;
        self.now_tab = tabs[next];
        self.now_scroll = 0;
        Vec::new()
    }

    pub fn pane(&self) -> &Pane {
        match self.tab {
            Tab::Files => &self.files,
            Tab::Library => &self.library,
            Tab::Playlists => &self.playlists,
            Tab::Search => &self.search,
            Tab::Discover => &self.discover,
        }
    }

    fn pane_mut(&mut self) -> &mut Pane {
        let tab = self.tab;
        self.pane_for_mut(tab)
    }

    fn pane_for_mut(&mut self, tab: Tab) -> &mut Pane {
        match tab {
            Tab::Files => &mut self.files,
            Tab::Library => &mut self.library,
            Tab::Playlists => &mut self.playlists,
            Tab::Search => &mut self.search,
            Tab::Discover => &mut self.discover,
        }
    }

    /// Note which panes have a request in flight, reading it off the effects
    /// that were just produced rather than asking every call site to remember.
    /// Tagging by the command means a reply for a tab you have since left
    /// still clears the right spinner.
    fn note_pending(&mut self, effects: &[Effect]) {
        for effect in effects {
            // Playback is answered by the audio thread rather than a pane, so
            // it gets recorded here rather than lighting a spinner.
            if let Effect::Audio(cmd) = effect {
                match cmd {
                    AudioCmd::Play { url, duration_hint } => {
                        self.starting = Some(url.clone());
                        // Describe what was asked for, so the bar shows the
                        // new track's length from nothing rather than the old
                        // track's position against no length at all.
                        self.status = PlayerStatus {
                            source: url.clone(),
                            duration: duration_hint.unwrap_or(0.0),
                            volume: self.status.volume,
                            ..PlayerStatus::default()
                        };
                    }
                    AudioCmd::Stop => self.starting = None,
                    _ => {}
                }
                continue;
            }
            let tab = match effect {
                Effect::Api(ApiCmd::Browse(_)) => Tab::Files,
                Effect::Api(ApiCmd::Library(_)) => Tab::Library,
                Effect::Api(ApiCmd::Playlists | ApiCmd::LoadPlaylist(_)) => Tab::Playlists,
                Effect::Api(ApiCmd::Search(_)) => Tab::Search,
                Effect::Api(ApiCmd::Discover { .. }) => Tab::Discover,
                _ => continue,
            };
            self.pane_for_mut(tab).loading = true;
        }
    }

    /// Whether we have asked for a track and not yet heard it start.
    pub fn is_starting(&self) -> bool {
        self.starting.is_some()
    }

    /// Whether something the audio thread said is about the track we are on.
    ///
    /// The thread answers about the source it was holding when it spoke, and
    /// on a slow open that can be a track the user has long since moved past.
    /// `status.source` is the name to check against because it is set from
    /// the play we asked for rather than from the answer — so it says what we
    /// are on from the moment we ask, which is exactly the window that a late
    /// reply arrives in.
    fn is_current_source(&self, source: &str) -> bool {
        self.status.source == source
    }

    /// Whether what is on screen is drawn from the audio, and so wants
    /// redrawing far more often than a progress bar does.
    pub fn drawing_audio(&self) -> bool {
        self.fullscreen && self.now_tab() == NowTab::Visualizer
    }

    /// The path the file browser treats as the top.
    ///
    /// A server with one library has nothing above it worth showing: the list
    /// of libraries is that one row, so going up is a step everyone has to
    /// take back. The web UI stops at the library for exactly that reason and
    /// this follows it. Two or more libraries make the list a real choice, so
    /// the top stays where it was.
    fn browser_root(&self) -> &str {
        match self.libraries.as_slice() {
            [only] => only,
            _ => "",
        }
    }

    fn at_browser_root(&self) -> bool {
        self.path.trim_matches('/') == self.browser_root()
    }

    /// A request that failed answers nothing, so nothing calls `Pane::set` and
    /// nothing would otherwise stop the spinner. A stuck spinner is a worse
    /// lie than an empty list.
    fn clear_pending(&mut self) {
        let panes = [
            &mut self.files,
            &mut self.library,
            &mut self.playlists,
            &mut self.search,
            &mut self.discover,
        ];
        for pane in panes {
            pane.loading = false;
        }
    }

    /// Where the Search tab is: the class menu, one class, or something
    /// opened out of it.
    pub fn search_node(&self) -> &SearchNode {
        self.search_stack.last().unwrap_or(&SearchNode::Root)
    }

    /// The library view currently on screen.
    pub fn library_node(&self) -> &LibraryNode {
        self.library_stack.last().unwrap_or(&LibraryNode::Root)
    }

    pub fn discover_node(&self) -> &DiscoverNode {
        self.discover_stack.last().unwrap_or(&DiscoverNode::Root)
    }

    /// The tabs this server can actually serve, in order. The numbers on the
    /// header are positions in *this* list, so they stay 1..n with no gaps.
    pub fn tabs(&self) -> Vec<Tab> {
        Tab::ALL.into_iter().filter(|t| t.available(self.capabilities)).collect()
    }

    /// Where the current tab sits among the visible ones.
    pub fn tab_index(&self) -> usize {
        self.tabs().iter().position(|t| *t == self.tab).unwrap_or(0)
    }

    /// The current server as it should be shown. A tunnel session's endpoint
    /// is a loopback port that means nothing to anyone, so it is named by its
    /// identity instead.
    pub fn server_display(&self) -> String {
        if crate::quickconnect::is_tunnel_id(&self.session.server_id) {
            return crate::quickconnect::display_server(&self.session.server_id);
        }
        self.session.server.clone()
    }

    fn info(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), kind: MessageKind::Info });
    }

    fn error(&mut self, text: impl Into<String>) {
        self.message = Some(Message { text: text.into(), kind: MessageKind::Error });
    }

    // ── Actions ─────────────────────────────────────────────────────────────

    pub fn handle_action(&mut self, action: Action) -> Vec<Effect> {
        let effects = self.act(action);
        self.note_pending(&effects);
        effects
    }

    fn act(&mut self, action: Action) -> Vec<Effect> {
        // The connect screen swallows everything except quit.
        if !self.connected {
            return self.handle_connect_action(action);
        }
        // Panels are modal: they own the arrow keys and the letters they use,
        // so playback shortcuts can't fire while one is up.
        if self.dj_panel.is_some() {
            return self.handle_dj_action(action);
        }
        if self.journey.is_some() {
            return self.handle_journey_action(action);
        }
        if self.editing_query
            && let Some(effects) = self.handle_query_action(&action)
        {
            return effects;
        }
        // Only the keys that edit the filter are claimed. Up and Down fall
        // through, so the list can be narrowed and walked in one breath.
        if self.filtering
            && let Some(effects) = self.handle_filter_action(&action)
        {
            return effects;
        }

        match action {
            Action::Quit => {
                self.should_quit = true;
                vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)]
            }
            Action::ToggleHelp => {
                self.show_help = !self.show_help;
                Vec::new()
            }
            Action::Cancel => {
                self.show_help = false;
                Vec::new()
            }
            Action::CycleFocus if !self.fullscreen => {
                self.queue_column = !self.queue_column;
                self.focus = if self.queue_column { Focus::Queue } else { Focus::Browser };
                Vec::new()
            }
            Action::CycleFocus => {
                self.focus = match self.focus {
                    Focus::Browser => Focus::Queue,
                    Focus::Queue => Focus::Browser,
                };
                Vec::new()
            }
            Action::SelectTab(i) => self.select_tab(i),

            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::PageUp => self.move_selection(-PAGE_STEP),
            Action::PageDown => self.move_selection(PAGE_STEP),
            Action::HalfPageUp => self.move_selection(-PAGE_STEP / 2),
            Action::HalfPageDown => self.move_selection(PAGE_STEP / 2),
            Action::First => {
                // The same ownership rule as `move_selection`: the
                // full-screen view keeps the jump keys while it is up, and
                // on its scrolling tabs First means the top.
                if self.fullscreen {
                    match self.now_tab() {
                        NowTab::Queue => self.queue.select_first(),
                        _ => self.now_scroll = 0,
                    }
                } else {
                    match self.focus {
                        Focus::Browser => self.pane_mut().select_first(),
                        Focus::Queue => self.queue.select_first(),
                    }
                }
                Vec::new()
            }
            Action::Last => {
                if self.fullscreen {
                    // The scrolling tabs have no bottom to know about, so
                    // in the full-screen view Last only means something on
                    // the queue.
                    if self.now_tab() == NowTab::Queue {
                        self.queue.select_last();
                    }
                } else {
                    match self.focus {
                        Focus::Browser => self.pane_mut().select_last(),
                        Focus::Queue => self.queue.select_last(),
                    }
                }
                Vec::new()
            }

            Action::Activate => self.activate(),
            Action::Back => self.go_back(),
            Action::AddToQueue => self.add_selected_to_queue(),

            Action::PlayPause => self.play_pause(),
            Action::NextTrack => self.skip(true),
            Action::PrevTrack => self.skip_previous(),
            Action::SeekForward => self.seek_by(SEEK_STEP),
            Action::SeekBackward => self.seek_by(-SEEK_STEP),
            Action::SeekForwardFar => self.seek_by(SEEK_STEP_FAR),
            Action::SeekBackwardFar => self.seek_by(-SEEK_STEP_FAR),
            Action::JumpToPlaying => self.jump_to_playing(),
            Action::ToggleNowPlaying => {
                self.fullscreen = !self.fullscreen;
                Vec::new()
            }
            Action::NowTabNext => self.move_now_tab(1),
            Action::NowTabPrev => self.move_now_tab(-1),
            Action::VolumeUp => self.change_volume(VOLUME_STEP),
            Action::VolumeDown => self.change_volume(-VOLUME_STEP),

            Action::RemoveFromQueue => self.remove_from_queue(),
            Action::ClearQueue => {
                self.queue.clear();
                self.now_playing = None;
                vec![Effect::Audio(AudioCmd::Stop)]
            }
            Action::ToggleRepeat => {
                self.queue.repeat = self.queue.repeat.next();
                self.info(format!("repeat: {}", self.queue.repeat.label()));
                Vec::new()
            }
            Action::ToggleShuffle => {
                self.queue.shuffle = !self.queue.shuffle;
                self.info(format!(
                    "shuffle: {}",
                    if self.queue.shuffle { "on" } else { "off" }
                ));
                Vec::new()
            }
            Action::ToggleAutoDj => self.cycle_autodj(),
            Action::OpenDjPanel => self.open_dj_panel(),
            Action::StartJourney => self.start_journey(),

            Action::StartSearch => {
                self.tab = Tab::Search;
                self.focus = Focus::Browser;
                self.editing_query = true;
                Vec::new()
            }
            // No message: the panel names the mode under the picture, and one
            // set here would still be sitting in the browser's footer long
            // after you left the visualiser.
            Action::CycleViz => {
                self.viz.mode = self.viz.mode.next();
                // The last mode's history describes the last mode.
                self.viz.forget();
                Vec::new()
            }
            // Kept across a mode change and across leaving the view: it is a
            // preference about how you like to read a trace, not a property
            // of the trace.
            Action::ToggleScatter => {
                self.viz.scatter = !self.viz.scatter;
                Vec::new()
            }
            // Reopens on whatever is already typed, so a filter can be
            // widened or backed out of rather than only started again.
            Action::StartFilter => {
                self.focus = Focus::Browser;
                self.filtering = true;
                Vec::new()
            }

            // Text-entry actions outside an editing context.
            Action::Input(_) | Action::Backspace | Action::Submit => Vec::new(),
        }
    }

    /// Text entry for the search box. Returns `None` for keys the search box
    /// doesn't claim, so they fall through to the normal bindings.
    /// The filter prompt. Narrowing happens on every keystroke — there is
    /// nothing to submit, only somewhere to stop typing.
    fn handle_filter_action(&mut self, action: &Action) -> Option<Vec<Effect>> {
        let mut text = self.pane().filter.clone();
        match action {
            Action::Input(c) => text.push(*c),
            Action::Backspace => {
                // Backspacing past the start is how you leave without having
                // meant to type anything at all.
                if text.pop().is_none() {
                    self.filtering = false;
                    return Some(Vec::new());
                }
            }
            Action::Cancel => {
                self.filtering = false;
                self.pane_mut().clear_filter();
                return Some(Vec::new());
            }
            // Stop typing, keep what was typed: the narrowed list is the
            // point, and it is no use if it goes when you reach for it.
            Action::Submit => {
                self.filtering = false;
                return Some(Vec::new());
            }
            _ => return None,
        }
        self.pane_mut().apply_filter(text);
        Some(Vec::new())
    }

    fn handle_query_action(&mut self, action: &Action) -> Option<Vec<Effect>> {
        match action {
            Action::Input(c) => {
                self.query.push(*c);
                Some(Vec::new())
            }
            Action::Backspace => {
                self.query.pop();
                Some(Vec::new())
            }
            Action::Cancel => {
                self.editing_query = false;
                Some(Vec::new())
            }
            Action::Submit => {
                self.editing_query = false;
                let query = self.query.trim().to_string();
                if query.is_empty() {
                    return Some(Vec::new());
                }
                self.info(format!("searching for {query:?}…"));
                self.search_stack.clear();
                self.search.trail.clear();
                Some(vec![Effect::Api(ApiCmd::Search(query))])
            }
            _ => None,
        }
    }

    fn select_tab(&mut self, index: usize) -> Vec<Effect> {
        let Some(tab) = self.tabs().get(index).copied() else {
            return Vec::new();
        };
        let already_here = self.tab == tab;
        // Whatever the cursor is on *now* is what "more like this" means, and
        // switching tabs moves the cursor somewhere else entirely — so the
        // candidate has to be taken before the change, not after.
        let carried = self.now_playing.clone().or_else(|| self.selected_track());
        self.tab = tab;
        self.focus = Focus::Browser;

        // Load a tab's contents the first time it's opened.
        match tab {
            Tab::Library if self.library_stack.is_empty() => {
                self.library_stack.push(LibraryNode::Root);
                self.library.set(library_root_entries());
                Vec::new()
            }
            Tab::Discover => {
                self.set_discover_seed(carried);
                if self.discover_stack.is_empty() {
                    self.discover_stack.push(DiscoverNode::Root);
                }
                if *self.discover_node() == DiscoverNode::Root {
                    self.discover.set(self.discover_root_entries());
                }
                Vec::new()
            }
            Tab::Playlists if self.playlists.entries.is_empty() => {
                vec![Effect::Api(ApiCmd::Playlists)]
            }
            // Pressing the Search tab while already on it means "another
            // one" — there is nowhere else that keystroke could sensibly go,
            // and looking at results with no way back to the box was the
            // dead end. Arriving from elsewhere keeps the results you had.
            Tab::Search
                if already_here || (self.search.entries.is_empty() && self.query.is_empty()) =>
            {
                self.editing_query = true;
                self.query.clear();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Where the pointer last was, so the drawing can light what is under
    /// it. A terminal will not change the mouse cursor for us, so the
    /// affordance has to be on our side of the glass.
    pub fn note_pointer(&mut self, at: ratatui::layout::Position) {
        self.pointer = Some(at);
    }

    /// A click on the progress bar, in seconds from the start.
    pub fn seek_to(&mut self, position: f64) -> Vec<Effect> {
        if self.status.is_idle() || !position.is_finite() {
            return Vec::new();
        }
        vec![Effect::Audio(AudioCmd::Seek(position.max(0.0)))]
    }

    fn move_selection(&mut self, delta: isize) -> Vec<Effect> {
        // The full-screen view owns the arrows while it is up, and what they
        // move depends on which of its tabs is showing.
        if self.fullscreen {
            if self.now_tab() == NowTab::Queue {
                return self.move_queue_selection(delta);
            }
            self.now_scroll = self.now_scroll.saturating_add_signed(delta);
            return Vec::new();
        }
        match self.focus {
            Focus::Browser => self.pane_mut().move_by(delta),
            Focus::Queue => return self.move_queue_selection(delta),
        }
        Vec::new()
    }

    fn move_queue_selection(&mut self, delta: isize) -> Vec<Effect> {
        if !self.queue.items.is_empty() {
            let last = self.queue.items.len() as isize - 1;
            let current = self.queue.state.selected().unwrap_or(0) as isize;
            self.queue.state.select(Some((current + delta).clamp(0, last) as usize));
        }
        Vec::new()
    }

    fn activate(&mut self) -> Vec<Effect> {
        // In the full-screen view there is no browser on screen to open into,
        // so Enter means the one thing it can mean: play what is highlighted.
        if self.fullscreen {
            if self.now_tab() != NowTab::Queue {
                return Vec::new();
            }
            return match self.queue.state.selected() {
                Some(index) => self.play_index(index),
                None => Vec::new(),
            };
        }

        if self.focus == Focus::Queue {
            let Some(index) = self.queue.state.selected() else {
                return Vec::new();
            };
            return self.play_index(index);
        }

        let Some(entry) = self.pane().selected().cloned() else {
            return Vec::new();
        };
        match entry {
            Entry::Parent => self.go_back(),
            Entry::Dir { path, .. } => {
                self.push_trail();
                self.path = path.clone();
                vec![Effect::Api(ApiCmd::Browse(path))]
            }
            Entry::Node { node, label } if self.tab == Tab::Search => {
                self.push_trail();
                self.search_stack.push(SearchNode::Library(node.clone()));
                self.search.set(Vec::new());
                self.info(format!("loading {label}…"));
                vec![Effect::Api(ApiCmd::SearchDrill(node))]
            }
            Entry::Node { node, label } => {
                self.push_trail();
                self.library_stack.push(node.clone());
                self.library.set(Vec::new());
                self.info(format!("loading {label}…"));
                vec![Effect::Api(ApiCmd::Library(node))]
            }
            Entry::Search { node, label, .. } => {
                self.push_trail();
                self.search_stack.push(node.clone());
                match node {
                    // Every class is already in hand -- no request.
                    SearchNode::Class(_) | SearchNode::Root => {
                        self.search.set(self.search_class_entries());
                        Vec::new()
                    }
                    SearchNode::Library(node) => {
                        self.search.set(Vec::new());
                        self.info(format!("loading {label}…"));
                        vec![Effect::Api(ApiCmd::SearchDrill(node))]
                    }
                }
            }
            Entry::Discover { node, label, .. } => {
                self.push_trail();
                self.open_discover(node, &label)
            }
            Entry::Playlist { name } => {
                self.push_trail();
                self.info(format!("loading playlist {name}…"));
                // Open now rather than when the tracks land, so this is a
                // record of where the user went instead of a record of what
                // last answered — which is what lets a late reply be told
                // apart from a wanted one, and lets Back close a playlist
                // that has not answered yet.
                self.playlist_open = Some(name.clone());
                vec![Effect::Api(ApiCmd::LoadPlaylist(name))]
            }
            Entry::Track { .. } => {
                // Enqueue everything visible and start at the highlighted row —
                // what every other player does on Enter.
                let (tracks, offset) = self.pane().tracks_with_offset();
                if tracks.is_empty() {
                    return Vec::new();
                }
                self.queue.replace(tracks);
                self.play_index(offset)
            }
        }
    }

    /// Remember the listing on screen as a column, on the way into the next
    /// one. Called before the request goes out, so the context is there while
    /// the reply is still coming.
    fn push_trail(&mut self) {
        let pane = self.pane_mut();
        if pane.entries.is_empty() {
            return;
        }
        let chosen = pane.state.selected().unwrap_or(0);
        // The column behind keeps the whole listing, not the narrowed view of
        // it. A filter is a way of finding one row, and once it has been found
        // the rest of the folder is the context worth having — which also
        // means coming back out is a list with nothing hidden and no filter
        // left over to explain.
        let entries = pane.entries.clone();
        let (entries, chosen) = match &pane.unfiltered {
            Some(all) => {
                let row = all.iter().position(|entry| entry == &entries[chosen]);
                (all.clone(), row.unwrap_or(0))
            }
            None => (entries, chosen),
        };
        pane.trail.push(Trail { entries, chosen });
    }

    fn go_back(&mut self) -> Vec<Effect> {
        // Where we are going is already in hand: the trail holds the listing
        // captured on the way in. Restoring it is instant, puts the cursor
        // back on the row you came through, and costs no request at all.
        //
        // Asking the server again left the deeper listing on screen with one
        // fewer column beside it until the reply landed — which reads as the
        // middle column blinking out and then coming back.
        let Some(effects) = self.step_out() else {
            return Vec::new();
        };
        let Some(step) = self.pane_mut().trail.pop() else {
            return effects;
        };
        // A "loading…" note is about the view being left, and now that its
        // reply is one we drop rather than apply, nothing else would ever
        // clear it. An error stays: it is about something that already
        // happened, and is still worth reading here.
        if matches!(self.message, Some(Message { kind: MessageKind::Info, .. })) {
            self.message = None;
        }
        let pane = self.pane_mut();
        pane.filter.clear();
        pane.unfiltered = None;
        pane.entries = step.entries;
        pane.state.select(Some(step.chosen));
        pane.loading = false;
        Vec::new()
    }

    /// Move the position one level out, returning what the old way of getting
    /// there would have cost. `None` means there was nowhere to go, which is
    /// what stops [`App::go_back`] restoring a column it never left.
    fn step_out(&mut self) -> Option<Vec<Effect>> {
        Some(match self.tab {
            Tab::Files => {
                if self.at_browser_root() {
                    return None;
                }
                let parent = match self.path.rsplit_once('/') {
                    Some((head, _)) => head.to_string(),
                    None => String::new(),
                };
                self.path = parent.clone();
                vec![Effect::Api(ApiCmd::Browse(parent))]
            }
            Tab::Search => {
                if self.search_stack.len() <= 1 {
                    return None; // already at the class menu
                }
                self.search_stack.pop();
                match self.search_node().clone() {
                    SearchNode::Root => {
                        self.search.set(self.search_root_entries());
                        Vec::new()
                    }
                    SearchNode::Class(_) => {
                        self.search.set(self.search_class_entries());
                        Vec::new()
                    }
                    SearchNode::Library(node) => {
                        self.search.set(Vec::new());
                        vec![Effect::Api(ApiCmd::SearchDrill(node))]
                    }
                }
            }
            Tab::Library => {
                if self.library_stack.len() <= 1 {
                    return None; // already at the mode menu
                }
                self.library_stack.pop();
                match self.library_node().clone() {
                    LibraryNode::Root => {
                        self.library.set(library_root_entries());
                        Vec::new()
                    }
                    node => {
                        self.library.set(Vec::new());
                        vec![Effect::Api(ApiCmd::Library(node))]
                    }
                }
            }
            Tab::Playlists if self.playlist_open.is_some() => {
                self.playlist_open = None;
                vec![Effect::Api(ApiCmd::Playlists)]
            }
            Tab::Discover => {
                if self.discover_stack.len() <= 1 {
                    return None; // already at the mode menu
                }
                self.discover_stack.pop();
                match self.discover_node().clone() {
                    DiscoverNode::Root => {
                        // Back at the top: re-anchor on whatever is current,
                        // so the next look starts from where you are now.
                        let carried = self.now_playing.clone();
                        self.set_discover_seed(carried);
                        self.discover.set(self.discover_root_entries());
                        Vec::new()
                    }
                    // An artist's ways in came with the artist list, so
                    // stepping back to it needs nothing from the server.
                    DiscoverNode::Artists => {
                        self.discover.set(self.discover_artist_entries());
                        Vec::new()
                    }
                    node => self.request_discover(node),
                }
            }
            _ => Vec::new(),
        })
    }

    // ── Discover ────────────────────────────────────────────────────────────

    /// Re-anchor Discover, keeping the previous seed when there is no new
    /// candidate — stepping around inside the tab must not lose it.
    fn set_discover_seed(&mut self, candidate: Option<Track>) {
        if let Some(seed) = candidate {
            self.discover_seed = Some(seed);
        }
    }

    /// The mode menu. Static, so opening the tab costs no request.
    fn discover_root_entries(&self) -> Vec<Entry> {
        let artist = self
            .discover_seed
            .as_ref()
            .and_then(|t| t.metadata.artist.clone())
            .unwrap_or_default();
        vec![
            Entry::Discover {
                label: "Similar tracks".into(),
                detail: "in your library".into(),
                node: DiscoverNode::Tracks,
            },
            Entry::Discover {
                label: "Similar artists".into(),
                detail: if artist.is_empty() {
                    "needs an artist tag".into()
                } else {
                    format!("like {artist}")
                },
                node: DiscoverNode::Artists,
            },
        ]
    }

    /// Rows for the artist list already in hand.
    fn discover_artist_entries(&self) -> Vec<Entry> {
        let mut entries = vec![Entry::Parent];
        entries.extend(self.discover_artists.iter().map(|a| {
            let ways = match a.entry_points.len() {
                0 => "no way in".to_string(),
                1 => "1 way in".to_string(),
                n => format!("{n} ways in"),
            };
            let mut detail = format!("{:.2}  {ways}", a.similarity);
            let tags: Vec<&str> = a.genre_tags.iter().take(2).map(|t| tidy_tag(t)).collect();
            if !tags.is_empty() {
                detail.push_str(" · ");
                detail.push_str(&tags.join(", "));
            }
            Entry::Discover {
                label: a.artist.clone(),
                detail,
                node: DiscoverNode::Artist(a.artist.clone()),
            }
        }));
        entries
    }

    /// The chosen artist's doorways, as playable rows.
    fn discover_entry_point_entries(&self, artist: &str) -> Vec<Entry> {
        let mut entries = vec![Entry::Parent];
        if let Some(found) = self.discover_artists.iter().find(|a| a.artist == artist) {
            entries.extend(found.entry_points.iter().map(|track| Entry::Track {
                label: track.display_name(),
                track: Box::new(track.clone()),
            }));
        }
        entries
    }

    fn open_discover(&mut self, node: DiscoverNode, label: &str) -> Vec<Effect> {
        // An artist's ways in are already here; going in costs nothing.
        if let DiscoverNode::Artist(artist) = &node {
            let entries = self.discover_entry_point_entries(artist);
            self.discover_stack.push(node);
            self.discover.set(entries);
            self.message = None;
            return Vec::new();
        }
        self.discover_stack.push(node.clone());
        self.discover.set(Vec::new());
        self.info(format!("looking for {}…", label.to_lowercase()));
        self.request_discover(node)
    }

    fn request_discover(&mut self, node: DiscoverNode) -> Vec<Effect> {
        let Some(seed) = self.discover_seed.clone() else {
            self.error("play or highlight a track first — discovery needs somewhere to start");
            return Vec::new();
        };
        vec![Effect::Api(ApiCmd::Discover { node, seed: Box::new(seed) })]
    }

    /// Put the cursor back on the playing track.
    ///
    /// Every other terminal player has this (cmus `i`, ncmpcpp `o`,
    /// musikcube `x`) and for the same reason: browsing takes you a long way
    /// from the music — several tabs and two drill-downs, here — and there
    /// needs to be one key that means "back to where I was listening".
    fn jump_to_playing(&mut self) -> Vec<Effect> {
        let Some(index) = self.queue.current else {
            self.info("nothing is playing");
            return Vec::new();
        };
        // Put the queue on screen before handing it the cursor. Focus alone
        // is not that: with the column hidden it left every later key
        // driving a list nobody could see — and the full-screen view keeps
        // its queue on a tab, where focus means nothing at all.
        if self.fullscreen {
            self.now_tab = NowTab::Queue;
        } else {
            self.queue_column = true;
            self.focus = Focus::Queue;
        }
        self.queue.state.select(Some(index));
        if let Some(track) = self.queue.items.get(index) {
            self.info(format!("playing {}", track.display_name()));
        }
        Vec::new()
    }

    /// Where the file browser should open.
    ///
    /// Where you left off, if that was remembered. Otherwise let the server
    /// choose: with a single library the old answer was a list containing
    /// exactly one row, which everyone had to step through before reaching
    /// any music.
    fn opening_path(&self) -> String {
        if self.path.is_empty() {
            crate::api::BEST_START.to_string()
        } else {
            self.path.clone()
        }
    }

    /// The track under the cursor, wherever the cursor is. Directories and
    /// playlist rows are not tracks and give nothing.
    fn selected_track(&self) -> Option<Track> {
        match self.focus {
            Focus::Browser => match self.pane().selected() {
                Some(Entry::Track { track, .. }) => Some((**track).clone()),
                _ => None,
            },
            Focus::Queue => {
                self.queue.state.selected().and_then(|i| self.queue.items.get(i)).cloned()
            }
        }
    }

    fn add_selected_to_queue(&mut self) -> Vec<Effect> {
        if self.focus != Focus::Browser {
            return Vec::new();
        }
        let Some(Entry::Track { track, .. }) = self.pane().selected().cloned() else {
            return Vec::new();
        };
        let label = track.display_name();
        let was_empty = self.queue.items.is_empty();
        self.queue.push(*track);
        self.info(format!("queued {label}"));

        // Nothing playing and nothing queued before: start immediately.
        if was_empty && self.status.is_idle() {
            return self.play_index(0);
        }
        Vec::new()
    }

    fn remove_from_queue(&mut self) -> Vec<Effect> {
        // The Queue tab of the full-screen view is the queue, whatever the
        // hidden browser screen happens to have focused.
        let on_the_queue =
            if self.fullscreen { self.now_tab() == NowTab::Queue } else { self.focus == Focus::Queue };
        if !on_the_queue {
            return Vec::new();
        }
        let Some(index) = self.queue.state.selected() else {
            return Vec::new();
        };
        if self.queue.remove(index) {
            // The playing track was removed: stop rather than silently
            // continuing something that is no longer in the queue.
            self.now_playing = None;
            return vec![Effect::Audio(AudioCmd::Stop)];
        }
        Vec::new()
    }

    pub fn play_index(&mut self, index: usize) -> Vec<Effect> {
        let Some(track) = self.queue.items.get(index).cloned() else {
            return Vec::new();
        };
        let session = &self.session;
        let url = match urls::media_url(&session.server, &track.filepath, session.token.as_deref()) {
            Ok(url) => url,
            Err(e) => {
                self.error(e);
                return Vec::new();
            }
        };
        self.queue.start(index);
        let hint = track.metadata.duration;
        self.remember_played(&track);
        self.now_playing = Some(track);

        let mut effects = vec![Effect::Audio(AudioCmd::Play { url, duration_hint: hint })];
        effects.extend(self.maybe_autodj());
        effects
    }

    fn play_pause(&mut self) -> Vec<Effect> {
        if self.status.is_idle() {
            // Nothing loaded — start the queue if there is one.
            return match self.queue.next_index(true) {
                Some(index) => self.play_index(index),
                None => Vec::new(),
            };
        }
        if self.status.paused {
            vec![Effect::Audio(AudioCmd::Resume)]
        } else {
            vec![Effect::Audio(AudioCmd::Pause)]
        }
    }

    fn skip(&mut self, manual: bool) -> Vec<Effect> {
        match self.queue.next_index(manual) {
            Some(index) => self.play_index(index),
            None => {
                self.now_playing = None;
                self.queue.current = None;
                vec![Effect::Audio(AudioCmd::Stop)]
            }
        }
    }

    fn skip_previous(&mut self) -> Vec<Effect> {
        // Restart the track if we're more than a few seconds in, like every
        // other player.
        if self.status.position > 3.0 {
            return vec![Effect::Audio(AudioCmd::Seek(0.0))];
        }
        match self.queue.prev_index() {
            Some(index) => self.play_index(index),
            None => Vec::new(),
        }
    }

    fn seek_by(&mut self, delta: f64) -> Vec<Effect> {
        if self.status.is_idle() {
            return Vec::new();
        }
        let target = (self.status.position + delta).max(0.0);
        vec![Effect::Audio(AudioCmd::Seek(target))]
    }

    fn change_volume(&mut self, delta: f32) -> Vec<Effect> {
        self.volume = (self.volume + delta).clamp(0.0, 1.0);
        vec![Effect::Audio(AudioCmd::SetVolume(self.volume))]
    }

    // ── Worker events ───────────────────────────────────────────────────────

    pub fn apply_event(&mut self, event: Event) -> Vec<Effect> {
        if matches!(event, Event::Error(_) | Event::Unauthorized) {
            self.clear_pending();
        }
        let effects = self.consume(event);
        self.note_pending(&effects);
        effects
    }

    fn consume(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Status(status) => {
                // While a track is starting, a status about any other source
                // is the tail of the one we left — the empty poll after it ran
                // out, or the last of a track being skipped away from. Taking
                // it would hang that track's position and its "stopped" under
                // the name of the one coming up.
                if let Some(wanted) = &self.starting {
                    if status.source != *wanted {
                        return Vec::new();
                    }
                    self.starting = None;
                }
                // Something loaded and is playing, so whatever went wrong
                // before is behind us — the run of failures starts over.
                if !status.source.is_empty() {
                    self.failures = 0;
                }
                self.status = status;
                Vec::new()
            }
            Event::TrackEnded { source } => {
                // The end of a track we are no longer on. Advancing on it
                // would skip past the one the user chose instead.
                if !self.is_current_source(&source) {
                    return Vec::new();
                }
                self.skip(false)
            }
            Event::AudioFailed(e) => {
                self.audio_available = false;
                self.error(format!("audio unavailable: {e}"));
                Vec::new()
            }
            Event::PlaybackFailed { source, error } => {
                // Same rule, and this is where it earns its keep: an open can
                // take as long as the network does to give up, so a failure
                // for the track before this one lands while this one is
                // starting. Everything below would then be about the wrong
                // track — the name in the message, the failure count, and the
                // skip that moves past a track nothing has been tried on.
                if !self.is_current_source(&source) {
                    return Vec::new();
                }
                // The source we were waiting on is never going to arrive, and
                // a wait with nothing coming would discard every status after
                // it. Whatever we move to next sets its own.
                self.starting = None;
                // One bad file used to end the listening session: the message
                // appeared and the queue simply stopped. Say which track, and
                // carry on to the next.
                let what = self
                    .now_playing
                    .as_ref()
                    .map(Track::display_name)
                    .unwrap_or_else(|| "that track".to_string());
                self.failures += 1;
                // A queue where nothing plays must not be walked forever —
                // with repeat on, skipping would go round and round.
                if self.failures >= self.queue.items.len().max(1) {
                    self.failures = 0;
                    self.now_playing = None;
                    self.queue.current = None;
                    self.error(format!("{what} could not be played, and nor could the rest"));
                    return vec![Effect::Audio(AudioCmd::Stop)];
                }
                self.error(format!("skipping {what} — {error}"));
                // Manual, so repeat-one doesn't sit on the broken track.
                self.skip(true)
            }
            Event::Connected { server, id, username, token, ping } => {
                self.connected = true;
                self.connecting = false;
                self.connect.submitting = false;
                self.session.server = server;
                self.session.server_id = id;
                if token.is_some() {
                    self.session.token = token;
                }
                if username.is_some() {
                    self.session.username = username;
                }
                self.capabilities = crate::api::types::Capabilities::from(ping.as_ref());
                self.libraries = ping.vpaths.clone();
                let libraries = ping.vpaths.len();
                self.info(format!(
                    "connected to {} ({} librar{})",
                    self.server_display(),
                    libraries,
                    if libraries == 1 { "y" } else { "ies" }
                ));

                // A remembered mode can outlive the server that supported it —
                // preferences are global, capabilities are per-server. Say so
                // rather than leaving a mode selected that quietly does
                // something else.
                if !self.autodj.available(self.capabilities) {
                    self.autodj = self.autodj.next_available(self.capabilities);
                    self.info(format!(
                        "this server has no similarity index — auto-dj is on {}",
                        self.autodj.label()
                    ));
                }

                // Opening the browser again: whatever this browse comes back
                // with is where we are, since neither `~` nor a remembered
                // path is a promise about how the server will spell it.
                self.opening = true;
                let mut effects = vec![
                    Effect::Api(ApiCmd::Browse(self.opening_path())),
                    Effect::Audio(AudioCmd::SetVolume(self.volume)),
                ];
                // Worth persisting when we hold a token we logged in for — or
                // a pairing code, which is the only way back to this server
                // even when it needs no login at all.
                let signed_in = self.session.token.is_some() && self.session.username.is_some();
                if signed_in || self.session.tunnel_code.is_some() {
                    effects.push(Effect::SaveSession);
                }
                effects
            }
            Event::ServersDiscovered(found) => {
                // Results can land after the user has already made a choice.
                // Row 0 means "the paste row" while the list is empty and
                // "the first server" once it isn't, so without this the
                // cursor silently retargets and Enter connects somewhere the
                // user never picked.
                let entered_a_code = !self.connect.code.trim().is_empty();
                self.connect.searching = false;
                self.connect.found = found;
                self.connect.row = if entered_a_code {
                    // Someone mid-paste keeps their place; otherwise the
                    // cursor lands on the first server, which is what a user
                    // who simply waited expects.
                    self.connect.paste_row()
                } else {
                    self.connect.row.min(self.connect.paste_row())
                };
                Vec::new()
            }
            Event::TunnelReady { local_url, id } => {
                self.connecting = false;
                self.connect.submitting = false;
                // The form carries the loopback address, which is a real,
                // working endpoint for the sign-in about to happen; the
                // identity is what the session will be filed under.
                self.connect.server = local_url;
                self.session.server_id = id;
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1; // straight to the username
                self.info("tunnel open — sign in to continue");
                Vec::new()
            }
            Event::Listing(listing) => {
                let path = listing.path.trim_matches('/');
                // A reply for a folder we have since left. Taking it would put
                // that folder's rows on screen and drag `path` back in after
                // them, while the trail beside it still describes the way to
                // where the user actually is — and nothing afterwards repairs
                // that, because going back is answered from the trail rather
                // than by asking again.
                if !self.opening && path != self.path {
                    return Vec::new();
                }
                self.opening = false;
                self.path = path.to_string();
                let root = self.browser_root().to_string();
                self.files.set(entries_from_listing(&listing, &root));
                Vec::new()
            }
            Event::Library { node, data } => {
                // Drop a reply for a view the user has already navigated away
                // from, so a slow request can't overwrite the current screen.
                if self.library_node() != &node {
                    return Vec::new();
                }
                self.library.set(entries_from_library(data));
                self.message = None;
                Vec::new()
            }
            Event::Discover { node, data, note } => {
                // Drop a reply for a view the user has already left, the same
                // rule the Library tab follows.
                if *self.discover_node() != node {
                    return Vec::new();
                }
                match data {
                    DiscoverData::Tracks(tracks) => {
                        let mut entries = vec![Entry::Parent];
                        entries.extend(tracks.into_iter().map(|track| Entry::Track {
                            label: track.display_name(),
                            track: Box::new(track),
                        }));
                        self.discover.set(entries);
                    }
                    DiscoverData::Artists(artists) => {
                        self.discover_artists = artists;
                        self.discover.set(self.discover_artist_entries());
                    }
                }
                match note {
                    Some(note) => self.info(note),
                    None => self.message = None,
                }
                Vec::new()
            }
            // The four DJ replies go through one door, so the in-flight
            // bookkeeping happens in one place (audit #57).
            event @ (Event::AutoDjSample { .. }
            | Event::Journey { .. }
            | Event::Genres(_)
            | Event::AutoDjPick { .. }) => self.consume_dj(event),
            Event::Playlists(playlists) => {
                self.playlist_open = None;
                self.playlists.set(
                    playlists
                        .into_iter()
                        .map(|p| Entry::Playlist { name: p.name })
                        .collect(),
                );
                Vec::new()
            }
            Event::PlaylistTracks { name, tracks } => {
                // A playlist the user has closed, or moved off to another —
                // its tracks would otherwise open over the top of whatever
                // they went to instead, under that one's name.
                if self.playlist_open.as_deref() != Some(name.as_str()) {
                    return Vec::new();
                }
                let mut entries = vec![Entry::Parent];
                entries.extend(tracks.into_iter().map(|t| Entry::Track {
                    label: t.display_name(),
                    track: Box::new(t),
                }));
                self.playlists.set(entries);
                self.message = None;
                Vec::new()
            }
            Event::SearchResults(results) => {
                // Kept whole. Flattening the three track classes into one list
                // and counting the other two into a sentence was throwing the
                // artist and album hits away entirely -- the server found
                // them, we said how many, and no key reached them.
                let total: usize = search_counts(&results).iter().sum();
                self.search_summary = Some(match total {
                    1 => "1 match".to_string(),
                    n => format!("{n} matches"),
                });
                self.search_hits = Some(results);
                self.search_stack = vec![SearchNode::Root];
                self.search.trail.clear();
                self.search.set(self.search_root_entries());
                self.message = None;
                Vec::new()
            }
            Event::SearchDrill { node, data } => {
                // Drop a reply for a view already left, the same rule the
                // Library and Discover tabs follow.
                if self.search_node() != &SearchNode::Library(node) {
                    return Vec::new();
                }
                self.search.set(entries_from_library(data));
                self.message = None;
                Vec::new()
            }
            Event::NeedsLogin { server } => {
                // A reply from a connection attempt that has been overtaken —
                // we already reached somewhere else. Applying it would drag a
                // connected session back to a login form.
                if self.connected {
                    return Vec::new();
                }
                self.connecting = false;
                self.connect.submitting = false;
                self.connect.server = server;
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1; // straight to the username
                self.info("this server needs a sign-in");
                Vec::new()
            }
            Event::Unauthorized => {
                // An established session went bad. Offer the login form for
                // the server we were already using rather than dumping the
                // user back at "how do you want to connect?".
                self.connected = false;
                self.connecting = false;
                self.connect.submitting = false;
                if !self.session.server.is_empty() {
                    self.connect.server = self.session.server.clone();
                }
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1;
                self.session.token = None;
                self.error("session expired — sign in again");
                Vec::new()
            }
            Event::Error(e) => {
                self.connecting = false;
                self.connect.submitting = false;
                self.error(e);
                Vec::new()
            }
        }
    }
}

/// The Library tab's mode menu — static, so opening the tab costs no request.
fn library_root_entries() -> Vec<Entry> {
    [
        ("Artists", LibraryNode::Artists),
        ("Albums", LibraryNode::Albums),
        ("Genres", LibraryNode::Genres),
        ("Recently Added", LibraryNode::Recent),
    ]
    .into_iter()
    .map(|(label, node)| Entry::Node { label: label.to_string(), node })
    .collect()
}

fn album_label(album: &Album) -> String {
    let name = album.name.as_deref().unwrap_or("(untitled album)");
    let year = album.year.map(|y| format!(" ({y})")).unwrap_or_default();
    match album.artist.as_deref() {
        Some(artist) if !artist.is_empty() => format!("{artist} — {name}{year}"),
        _ => format!("{name}{year}"),
    }
}

fn genre_label(genre: &Genre) -> String {
    match genre.track_count {
        Some(count) => format!("{} ({count})", genre.name),
        None => genre.name.clone(),
    }
}

/// Rows for a loaded library view. Every one of these sits below the mode
/// menu, so they all get a ".." to climb back out.
/// How many hits each class holds, in menu order.
fn search_counts(results: &crate::api::types::SearchResults) -> [usize; 5] {
    [
        results.artists.len(),
        results.albums.len(),
        results.title.len(),
        results.files.len(),
        results.lyrics.len(),
    ]
}

impl App {
    /// The class menu: what matched, and how many. Classes that matched
    /// nothing are left out -- a row saying zero is a row you have to read to
    /// learn it was not worth reading.
    fn search_root_entries(&self) -> Vec<Entry> {
        let Some(hits) = &self.search_hits else {
            return Vec::new();
        };
        let counts = search_counts(hits);
        SEARCH_CLASSES
            .iter()
            .zip(counts)
            .filter(|(_, n)| *n > 0)
            .map(|(class, n)| Entry::Search {
                label: class.title().to_string(),
                detail: n.to_string(),
                node: SearchNode::Class(*class),
            })
            .collect()
    }

    /// The hits inside whichever class is open. Artists and albums become the
    /// same nodes the Library tab drills, because that is what they are.
    fn search_class_entries(&self) -> Vec<Entry> {
        let (Some(hits), SearchNode::Class(class)) = (&self.search_hits, self.search_node())
        else {
            return Vec::new();
        };
        let track_rows = |rows: &[crate::api::types::SearchTrack]| {
            rows.iter()
                .map(|hit| {
                    let track =
                        Track { filepath: hit.filepath.clone(), metadata: hit.metadata.clone() };
                    Entry::Track { label: track.display_name(), track: Box::new(track) }
                })
                .collect::<Vec<_>>()
        };

        let mut entries = vec![Entry::Parent];
        match class {
            SearchClass::Artists => entries.extend(hits.artists.iter().map(|group| Entry::Node {
                label: group.name.clone(),
                node: LibraryNode::Artist(group.name.clone()),
            })),
            SearchClass::Albums => entries.extend(hits.albums.iter().map(|group| Entry::Node {
                label: group.name.clone(),
                node: LibraryNode::Album { name: group.name.clone(), artist: None },
            })),
            SearchClass::Titles => entries.extend(track_rows(&hits.title)),
            SearchClass::Files => entries.extend(track_rows(&hits.files)),
            SearchClass::Lyrics => entries.extend(track_rows(&hits.lyrics)),
        }
        entries
    }
}

fn entries_from_library(data: LibraryData) -> Vec<Entry> {
    let mut entries = vec![Entry::Parent];
    match data {
        LibraryData::Artists(artists) => entries.extend(artists.into_iter().map(|name| {
            Entry::Node { label: name.clone(), node: LibraryNode::Artist(name) }
        })),
        LibraryData::Albums(albums) => entries.extend(albums.into_iter().map(|album| {
            let label = album_label(&album);
            let node = LibraryNode::Album {
                name: album.name.unwrap_or_default(),
                artist: album.artist,
            };
            Entry::Node { label, node }
        })),
        LibraryData::Genres(genres) => entries.extend(genres.into_iter().map(|genre| {
            let label = genre_label(&genre);
            Entry::Node { label, node: LibraryNode::Genre(genre.name) }
        })),
        LibraryData::Tracks(tracks) => entries.extend(tracks.into_iter().map(|track| {
            Entry::Track { label: track.display_name(), track: Box::new(track) }
        })),
    }
    entries
}

/// The model writes hierarchical tags — "Electronic---Dubstep". In a list of
/// artists similar to each other the prefix is the same on every row, so only
/// the leaf carries information; it is also the difference between two tags
/// fitting on a line and none of them fitting.
fn tidy_tag(tag: &str) -> &str {
    tag.rsplit("---").next().unwrap_or(tag).trim()
}

/// Join a directory prefix and an entry name into a library path.
fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// `root` is the path with nothing above it worth offering — empty for the
/// list of libraries, or the one library on a server that has only one.
fn entries_from_listing(listing: &DirListing, root: &str) -> Vec<Entry> {
    let prefix = listing.path.trim_matches('/');
    let mut entries = Vec::new();
    if !prefix.is_empty() && prefix != root {
        entries.push(Entry::Parent);
    }
    for dir in &listing.directories {
        entries.push(Entry::Dir {
            label: dir.name.clone(),
            path: qualify(prefix, &dir.name),
        });
    }
    for file in &listing.files {
        // A playlist file is a list of tracks, not a track. The server
        // indexes them all the same, and `Enter` queues everything on screen,
        // so leaving one here puts something undecodable in the queue.
        if !is_audio(file.kind.as_deref()) {
            continue;
        }
        // The server's own filepath when it sent one: it is the canonical
        // form, and it is what these tags were looked up under. Falling back
        // to the joined path keeps listings without metadata working exactly
        // as before.
        let tags = file.metadata.as_ref();
        let filepath = tags
            .map(|m| m.filepath.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| qualify(prefix, &file.name));
        // The label stays the filename — this is the view of what is on disk,
        // and that is what people are looking for here. The tags ride along
        // for the queue, the now-playing screen and Auto-DJ, which all read
        // them off the track rather than the row.
        entries.push(Entry::Track {
            label: file.name.clone(),
            track: Box::new(Track {
                filepath,
                metadata: tags.and_then(|m| m.metadata.clone()).unwrap_or_default(),
            }),
        });
    }
    entries
}

/// Whether the file explorer should offer this as something to play.
///
/// mStream indexes playlist files alongside audio — its ping even reports
/// `m3u: false` under `supportedAudioFiles`, and its own Auto-DJ picker
/// excludes them with the note that a client cannot stream one. The file
/// browser is the one place they still reach a queue.
///
/// Anything unrecognised is treated as audio: a format this player cannot
/// decode should fail loudly when played, not vanish from the listing.
fn is_audio(kind: Option<&str>) -> bool {
    !matches!(
        kind.map(str::to_ascii_lowercase).as_deref(),
        Some("m3u" | "m3u8" | "pls" | "cue" | "xspf" | "asx")
    )
}

// The keymap lives in `super::keymap` (audit #55); these imports keep
// the app's tests reading exactly as they did through `use super::*`.
use crate::tui::keymap::Keymap;
#[cfg(test)]
use crate::tui::keymap::{Key, ctrl, key};

/// Map a key press using the *default* bindings.
///
/// Test-only: the player reads `app.keymap`, which may have been rebound.
#[cfg(test)]
pub fn map_key(key: KeyEvent, mode: InputMode) -> Option<Action> {
    static DEFAULTS: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(Keymap::default).action(key, mode)
}


#[cfg(test)]
mod tests;
