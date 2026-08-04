//! TUI state and the rules that change it.
//!
//! Deliberately I/O-free: actions and worker events go in, state changes and
//! [`Effect`]s come out, and the run loop is the only thing that touches
//! channels. That keeps the interesting behaviour — navigation, queue
//! advancement, repeat/shuffle — testable without a terminal or a server.

use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::api::types::{Album, DirListing, Genre, Track};
use crate::api::urls;
use crate::discovery::DiscoveredServer;
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

/// Which step of the connect screen is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectStage {
    /// Pick how to reach the server.
    #[default]
    Choosing,
    /// Server address plus credentials.
    Direct,
    /// Paste a pairing code and reach the server over its Iroh tunnel.
    QuickConnect,
}

/// The two ways in, in menu order.
pub const CONNECT_METHODS: [(&str, &str); 2] = [
    ("Direct", "server address on your network"),
    ("Quick Connect", "pairing code — works from anywhere"),
];

/// The connect screen, shown when there is no usable session.
#[derive(Debug, Default)]
pub struct ConnectForm {
    pub stage: ConnectStage,
    pub choice: usize,
    pub server: String,
    pub username: String,
    pub password: String,
    pub code: String,
    pub field: usize,
    pub submitting: bool,
    /// Servers found on the network, for the Quick Connect screen.
    pub found: Vec<DiscoveredServer>,
    pub searching: bool,
    /// Row selected on the Quick Connect screen: an index into `found`, or
    /// `found.len()` for the paste-a-code row, which is always last.
    pub row: usize,
    /// The server URL whose plaintext warning has been acknowledged. Held as
    /// the URL rather than a flag so that editing the address asks again
    /// instead of carrying consent over to a different host.
    pub insecure_ack: Option<String>,
}

impl ConnectForm {
    /// The paste-a-code row sits after any discovered servers.
    pub fn paste_row(&self) -> usize {
        self.found.len()
    }

    pub fn on_paste_row(&self) -> bool {
        self.row >= self.paste_row()
    }
}

impl ConnectForm {
    pub const FIELDS: usize = 3;

    fn value_mut(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.server,
            1 => &mut self.username,
            _ => &mut self.password,
        }
    }

    fn next_field(&mut self) {
        self.field = (self.field + 1) % Self::FIELDS;
    }
}

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
    /// Where this session's requests and stream URLs go. For a Quick Connect
    /// session that is the loopback bridge, which lives and dies with the
    /// process — see [`App::server_id`] for the part worth remembering.
    pub server: String,
    /// What the current server is remembered as: the same URL for a direct
    /// connection, a `mstream+iroh://` identity for a tunnel.
    pub server_id: String,
    /// Pairing code for the tunnel this session is using, if any. Held so it
    /// can be saved alongside the session — without it, a remembered tunnel
    /// server cannot be reached again.
    pub tunnel_code: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
    pub connected: bool,
    pub connecting: bool,
    pub connect: ConnectForm,

    pub tab: Tab,
    pub focus: Focus,

    pub path: String,
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
            server: server.clone().unwrap_or_default(),
            server_id: server.clone().unwrap_or_default(),
            tunnel_code: None,
            token,
            username,
            connected: false,
            connecting: false,
            connect: ConnectForm::default(),
            tab: Tab::Files,
            focus: Focus::Browser,
            path: String::new(),
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
        if crate::quickconnect::is_tunnel_id(&app.server_id) {
            app.server.clear();
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
        self.tunnel_code = code;
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

    /// Recent track paths, newest first — the sonic anchor.
    fn anchors(&self) -> Vec<String> {
        self.autodj_recent.iter().map(|t| t.filepath.clone()).collect()
    }

    /// Recently-played artist names, newest first and deduped, for the
    /// cooldown. Tracks with no artist tag contribute nothing rather than an
    /// empty name the server would match against everything.
    fn recent_artists(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.autodj_recent
            .iter()
            .filter_map(|t| t.metadata.artist.as_deref())
            .filter(|a| !a.trim().is_empty())
            .filter(|a| seen.insert(a.to_ascii_lowercase()))
            .map(str::to_string)
            .collect()
    }

    /// Note a track as played, for anchoring and cooldown.
    fn remember_played(&mut self, track: &Track) {
        self.autodj_recent.retain(|t| t.filepath != track.filepath);
        self.autodj_recent.insert(0, track.clone());
        self.autodj_recent.truncate(RECENT_MEMORY);
    }

    /// Everything the worker needs to ask for a pick.
    fn dj_request(&self) -> Box<DjRequest> {
        Box::new(DjRequest {
            mode: self.autodj,
            settings: self.dj.clone(),
            seed: self.now_playing.clone().map(Box::new),
            ignore_list: self.autodj_ignore.clone(),
            anchors: self.anchors(),
            recent_artists: self.recent_artists(),
            sonic_available: self.capabilities.discovery,
        })
    }

    /// Effects to run at startup.
    pub fn start(&mut self) -> Vec<Effect> {
        let effects = self.begin();
        self.note_pending(&effects);
        effects
    }

    fn begin(&mut self) -> Vec<Effect> {
        // A tunnel server has no address to connect to until its code is
        // dialled, so reconnecting means opening the tunnel again first.
        if crate::quickconnect::is_tunnel_id(&self.server_id) {
            let Some(code) = self.tunnel_code.clone() else {
                // Remembered, but the code that reaches it is gone — deleted
                // credentials, or a config copied without them.
                self.server_id.clear();
                self.error(
                    "the pairing code for the last server is gone — paste it again to reconnect",
                );
                return Vec::new();
            };
            self.connecting = true;
            return vec![Effect::Api(ApiCmd::QuickConnect { code, token: self.token.clone() })];
        }
        if self.server.is_empty() {
            return Vec::new(); // connect form is showing
        }
        self.connecting = true;
        vec![Effect::Api(ApiCmd::Connect {
            server: self.server.clone(),
            token: self.token.clone(),
        })]
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
        if crate::quickconnect::is_tunnel_id(&self.server_id) {
            return crate::quickconnect::display_server(&self.server_id);
        }
        self.server.clone()
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
                match self.focus {
                    Focus::Browser => self.pane_mut().select_first(),
                    Focus::Queue => {
                        if !self.queue.items.is_empty() {
                            self.queue.state.select(Some(0));
                        }
                    }
                }
                Vec::new()
            }
            Action::Last => {
                match self.focus {
                    Focus::Browser => self.pane_mut().select_last(),
                    Focus::Queue => {
                        if !self.queue.items.is_empty() {
                            self.queue.state.select(Some(self.queue.items.len() - 1));
                        }
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
            // `A` cycles the mode, which is the whole interaction most of the
            // time; the panel behind `D` is for the rest of it.
            Action::ToggleAutoDj => {
                self.autodj = self.autodj.next_available(self.capabilities);
                self.info(format!("auto-dj: {}", self.autodj.label()));
                if self.autodj == AutoDjMode::Off {
                    // Any reply still in flight is no longer wanted.
                    self.autodj_pending = false;
                    return Vec::new();
                }
                self.maybe_autodj()
            }
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

    fn handle_connect_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }

        // A connection attempt takes seconds. Keys pressed in the meantime
        // must not edit the credentials or code being used, nor fire a second
        // attempt — a few stray characters appended to a pairing code turn it
        // into unreadable base64. Esc still abandons the attempt.
        if self.connect.submitting {
            return match action {
                Action::Cancel => {
                    self.connect.submitting = false;
                    self.connecting = false;
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                _ => Vec::new(),
            };
        }

        match self.connect.stage {
            ConnectStage::Choosing => match action {
                Action::Up => {
                    self.connect.choice = self.connect.choice.saturating_sub(1);
                    Vec::new()
                }
                Action::Down | Action::CycleFocus => {
                    self.connect.choice = (self.connect.choice + 1).min(CONNECT_METHODS.len() - 1);
                    Vec::new()
                }
                Action::Submit | Action::Activate => {
                    self.message = None;
                    if self.connect.choice == 0 {
                        self.connect.stage = ConnectStage::Direct;
                        return Vec::new();
                    }
                    self.connect.stage = ConnectStage::QuickConnect;
                    self.connect.searching = true;
                    self.connect.found.clear();
                    self.connect.row = 0;
                    vec![Effect::Discover]
                }
                _ => Vec::new(),
            },

            ConnectStage::Direct => match action {
                Action::Input(c) => {
                    self.connect.value_mut().push(c);
                    Vec::new()
                }
                Action::Backspace => {
                    self.connect.value_mut().pop();
                    Vec::new()
                }
                Action::CycleFocus | Action::Down => {
                    self.connect.next_field();
                    Vec::new()
                }
                Action::Up => {
                    self.connect.field =
                        (self.connect.field + ConnectForm::FIELDS - 1) % ConnectForm::FIELDS;
                    Vec::new()
                }
                Action::Cancel => {
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                Action::Submit => self.submit_connect(),
                _ => Vec::new(),
            },

            ConnectStage::QuickConnect => match action {
                Action::Up => {
                    self.connect.row = self.connect.row.saturating_sub(1);
                    Vec::new()
                }
                Action::Down => {
                    self.connect.row = (self.connect.row + 1).min(self.connect.paste_row());
                    Vec::new()
                }
                Action::Input(c) => {
                    // Typing anywhere means "I have a code", so jump to it.
                    self.connect.row = self.connect.paste_row();
                    self.connect.code.push(c);
                    Vec::new()
                }
                Action::Backspace => {
                    self.connect.code.pop();
                    Vec::new()
                }
                Action::Cancel => {
                    self.connect.stage = ConnectStage::Choosing;
                    self.message = None;
                    Vec::new()
                }
                Action::Submit => {
                    // A server found on this network is reachable directly —
                    // no tunnel needed, and no code to paste.
                    if let Some(server) = self.connect.found.get(self.connect.row).cloned() {
                        self.connecting = true;
                        self.connect.submitting = true;
                        self.connect.server = server.base_url.clone();
                        self.info(format!("connecting to {}…", server.name));
                        return vec![Effect::Api(ApiCmd::Connect {
                            server: server.base_url,
                            token: None,
                        })];
                    }
                    self.submit_quick_connect()
                }
                _ => Vec::new(),
            },
        }
    }

    fn submit_quick_connect(&mut self) -> Vec<Effect> {
        let code = self.connect.code.trim().to_string();
        if code.is_empty() {
            self.error("paste a pairing code first");
            return Vec::new();
        }
        self.connecting = true;
        self.connect.submitting = true;
        // Kept from here on: it is the only way back to this server, and
        // nothing is written until the connection actually succeeds.
        self.tunnel_code = Some(code.clone());
        self.info("dialling the tunnel — this can take a few seconds…");
        vec![Effect::Api(ApiCmd::QuickConnect { code, token: self.token.clone() })]
    }

    fn submit_connect(&mut self) -> Vec<Effect> {
        // Everything that can be settled without the network is settled here:
        // a round trip to learn that an address was mistyped is a slow way to
        // be told something we already know.
        let server = match crate::api::server_url::normalize(&self.connect.server) {
            Ok(server) => server,
            Err(message) => {
                self.error(message);
                return Vec::new();
            }
        };
        // Show what was filled in. "nas:3000" becoming "http://nas:3000" is
        // exactly what someone needs to see when it doesn't connect.
        self.connect.server = server.clone();

        let username = self.connect.username.trim().to_string();

        // No username means the server is expected to be in public mode, where
        // every request authenticates anyway.
        if username.is_empty() {
            self.connecting = true;
            self.connect.submitting = true;
            self.message = None;
            self.server = server.clone();
            return vec![Effect::Api(ApiCmd::Connect { server, token: None })];
        }

        if self.connect.password.is_empty() {
            self.error("enter a password, or clear the username for a public server");
            return Vec::new();
        }

        // Plain http past the local network puts the password on the wire in
        // the clear. Say so once, and let the answer be yes.
        if crate::api::server_url::crosses_the_internet_unencrypted(&server)
            && self.connect.insecure_ack.as_deref() != Some(server.as_str())
        {
            self.connect.insecure_ack = Some(server.clone());
            self.error(format!(
                "{server} is plain http — your password would cross the internet \
                 unencrypted. Enter again to send it anyway."
            ));
            return Vec::new();
        }

        self.connecting = true;
        self.connect.submitting = true;
        self.message = None;
        vec![Effect::Api(ApiCmd::Login {
            server,
            username,
            password: std::mem::take(&mut self.connect.password),
        })]
    }

    // ── Sonic Journey ───────────────────────────────────────────────────────

    /// `J` on a track. What's playing is the natural place to set off from,
    /// so one press is usually enough; with nothing playing it takes two —
    /// one to mark where to start, one to say where to end up.
    fn start_journey(&mut self) -> Vec<Effect> {
        if !self.capabilities.discovery_path {
            self.error("this server can't plot journeys — it has no discovery index");
            return Vec::new();
        }
        let Some(picked) = self.selected_track() else {
            self.error("highlight a track to travel to");
            return Vec::new();
        };

        let from = self.now_playing.clone().or_else(|| self.journey_from.take());
        let Some(from) = from else {
            self.journey_from = Some(picked.clone());
            self.info(format!(
                "journey starts at {} — now highlight where it should end and press J",
                picked.display_name()
            ));
            return Vec::new();
        };

        self.journey_from = None;
        self.open_journey(from, picked)
    }

    fn open_journey(&mut self, from: Track, to: Track) -> Vec<Effect> {
        let length = self
            .journey
            .as_ref()
            .map_or(crate::api::types::JOURNEY_DEFAULT_LENGTH, |j| j.length);
        self.journey = Some(Journey {
            from,
            to,
            length,
            stops: Vec::new(),
            pending: true,
            offset: 0,
        });
        self.message = None;
        self.fetch_journey()
    }

    fn fetch_journey(&mut self) -> Vec<Effect> {
        let Some(journey) = self.journey.as_mut() else { return Vec::new() };
        journey.pending = true;
        journey.offset = 0;
        vec![Effect::Api(ApiCmd::Journey {
            start: journey.from.filepath.clone(),
            end: journey.to.filepath.clone(),
            length: journey.length,
        })]
    }

    fn handle_journey_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        let Some(journey) = self.journey.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::Input('J') => {
                self.journey = None;
                Vec::new()
            }
            // Changing the length is a different arc, so it has to be asked
            // for again rather than trimmed locally.
            Action::SeekForward | Action::Back | Action::SeekBackward => {
                let delta: i32 = if action == Action::SeekForward { 2 } else { -2 };
                let length = (journey.length as i32 + delta).clamp(
                    crate::api::types::JOURNEY_MIN_LENGTH as i32,
                    crate::api::types::JOURNEY_MAX_LENGTH as i32,
                ) as u32;
                if length == journey.length {
                    return Vec::new();
                }
                journey.length = length;
                self.fetch_journey()
            }
            Action::Down => {
                journey.offset = (journey.offset + 1).min(journey.stops.len().saturating_sub(1));
                Vec::new()
            }
            Action::Up => {
                journey.offset = journey.offset.saturating_sub(1);
                Vec::new()
            }
            Action::First => {
                journey.offset = 0;
                Vec::new()
            }
            Action::Activate | Action::Submit => self.queue_journey(),
            _ => Vec::new(),
        }
    }

    /// Take the arc as the queue and start at its first stop.
    fn queue_journey(&mut self) -> Vec<Effect> {
        let Some(journey) = self.journey.take() else { return Vec::new() };
        if journey.stops.is_empty() {
            return Vec::new();
        }
        let count = journey.stops.len();
        let tracks: Vec<Track> = journey.stops.iter().map(|s| s.to_track()).collect();
        self.queue.replace(tracks);
        self.info(format!(
            "{count} stops from {} to {}",
            journey.from.display_name(),
            journey.to.display_name()
        ));
        self.play_index(0)
    }

    // ── Auto-DJ panel ───────────────────────────────────────────────────────

    /// Open the panel, built for what this server can do.
    fn open_dj_panel(&mut self) -> Vec<Effect> {
        self.dj_panel = Some(DjPanel::new(self.capabilities));
        self.message = None;
        Vec::new()
    }

    /// Keys while the panel is open. Left/right adjust the highlighted row;
    /// everything else is navigation or one of the panel's own commands.
    fn handle_dj_action(&mut self, action: Action) -> Vec<Effect> {
        if action == Action::Quit {
            self.should_quit = true;
            return vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)];
        }
        // The genre chooser sits over the panel and takes keys first.
        if self.dj_panel.as_ref().is_some_and(|p| p.genres.is_some()) {
            return self.handle_genre_action(action);
        }

        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::ToggleAutoDj => {
                self.dj_panel = None;
                Vec::new()
            }
            Action::Up => {
                panel.row = panel.row.saturating_sub(1);
                Vec::new()
            }
            Action::Down => {
                panel.row = (panel.row + 1).min(panel.rows.len().saturating_sub(1));
                Vec::new()
            }
            Action::First => {
                panel.row = 0;
                Vec::new()
            }
            Action::Last => {
                panel.row = panel.rows.len().saturating_sub(1);
                Vec::new()
            }
            // `h`/`l` and the arrows both land here — `[`/`]` too, which is
            // the same left/right shape.
            Action::Back | Action::SeekBackward => self.adjust_dj_row(-1),
            Action::SeekForward => self.adjust_dj_row(1),
            Action::Activate | Action::Submit => match panel.selected() {
                // Enter on the genre row opens the chooser; elsewhere it
                // nudges the setting, matching what right-arrow does.
                DjRow::Genres => self.open_genre_picker(),
                _ => self.adjust_dj_row(1),
            },
            // `p` samples what these settings produce.
            Action::Input('p') => self.sample_dj(),
            _ => Vec::new(),
        }
    }

    /// Move the highlighted setting by one step. Numbers move in useful
    /// increments rather than by one, so a slider crosses its range in a
    /// handful of presses.
    fn adjust_dj_row(&mut self, delta: i32) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_ref() else { return Vec::new() };
        let step = |value: u32, by: i32, max: u32| -> u32 {
            (value as i32 + by).clamp(0, max as i32) as u32
        };
        match panel.selected() {
            DjRow::Mode => {
                self.autodj = if delta > 0 {
                    self.autodj.next_available(self.capabilities)
                } else {
                    // Three modes, so stepping forward twice is stepping back.
                    self.autodj
                        .next_available(self.capabilities)
                        .next_available(self.capabilities)
                };
                if self.autodj == AutoDjMode::Off {
                    self.autodj_pending = false;
                    return Vec::new();
                }
                return self.maybe_autodj();
            }
            DjRow::Tightness => {
                self.dj.sonic_tightness = step(self.dj.sonic_tightness, delta * 5, 100);
            }
            DjRow::Anchor => self.dj.sonic_anchor = self.dj.sonic_anchor.next(),
            DjRow::Tempo => {
                self.dj.tempo_tolerance =
                    step(self.dj.tempo_tolerance, delta, dj::TEMPO_TOLERANCE_MAX);
            }
            DjRow::Key => {
                self.dj.key_matching = if delta > 0 {
                    self.dj.key_matching.next()
                } else {
                    self.dj.key_matching.next().next()
                };
            }
            DjRow::Rating => self.dj.min_rating = step(self.dj.min_rating, delta, dj::RATING_MAX),
            DjRow::Cooldown => {
                self.dj.artist_cooldown =
                    step(self.dj.artist_cooldown, delta, dj::ARTIST_COOLDOWN_MAX);
            }
            DjRow::Genres => self.dj.genre_mode = self.dj.genre_mode.next(),
        }
        Vec::new()
    }

    fn open_genre_picker(&mut self) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        panel.genres = Some(GenrePicker { loading: true, ..Default::default() });
        vec![Effect::Api(ApiCmd::Genres)]
    }

    fn handle_genre_action(&mut self, action: Action) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        let Some(picker) = panel.genres.as_mut() else { return Vec::new() };
        match action {
            Action::Cancel | Action::Activate | Action::Submit => {
                panel.genres = None;
            }
            Action::Up => picker.row = picker.row.saturating_sub(1),
            Action::Down => {
                picker.row = (picker.row + 1).min(picker.all.len().saturating_sub(1));
            }
            Action::First => picker.row = 0,
            Action::Last => picker.row = picker.all.len().saturating_sub(1),
            // Space toggles, which is why Enter closes rather than selects:
            // a chooser you leave with the same key you pick with is a
            // chooser you keep leaving by accident.
            Action::PlayPause => {
                if let Some(name) = picker.all.get(picker.row).cloned() {
                    if let Some(at) = self.dj.genres.iter().position(|g| *g == name) {
                        self.dj.genres.remove(at);
                    } else {
                        self.dj.genres.push(name);
                    }
                    // Choosing genres with the filter off is a dead end;
                    // switch it on rather than silently ignoring the choice.
                    if self.dj.genre_mode == dj::GenreMode::Off && !self.dj.genres.is_empty() {
                        self.dj.genre_mode = dj::GenreMode::Whitelist;
                    }
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn sample_dj(&mut self) -> Vec<Effect> {
        let Some(panel) = self.dj_panel.as_mut() else { return Vec::new() };
        if panel.sample_pending {
            return Vec::new();
        }
        panel.sample_pending = true;
        panel.sample.clear();
        vec![Effect::Api(ApiCmd::AutoDjSample {
            request: self.dj_request(),
            count: DJ_SAMPLE_COUNT,
        })]
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

    /// The wheel. Three rows a notch, which is what everything else does.
    ///
    /// `over_queue` is whether the pointer was over the queue column, which
    /// the arrows cannot express: they move whatever has focus, and a wheel
    /// should move whatever it is pointing at.
    pub fn wheel(&mut self, notches: isize, over_queue: bool) -> Vec<Effect> {
        const ROWS_PER_NOTCH: isize = 3;
        let delta = notches * ROWS_PER_NOTCH;
        if over_queue {
            return self.move_queue_selection(delta);
        }
        self.move_selection(delta)
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
        self.focus = Focus::Queue;
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
        let url = match urls::media_url(&self.server, &track.filepath, self.token.as_deref()) {
            Ok(url) => url,
            Err(e) => {
                self.error(e);
                return Vec::new();
            }
        };
        self.queue.current = Some(index);
        self.queue.state.select(Some(index));
        let hint = track.metadata.duration;
        self.remember_played(&track);
        self.now_playing = Some(track);

        let mut effects = vec![Effect::Audio(AudioCmd::Play { url, duration_hint: hint })];
        effects.extend(self.maybe_autodj());
        effects
    }

    /// Ask Auto-DJ for another track once the queue has nothing left after the
    /// one playing — early enough that it lands before the current track ends.
    fn maybe_autodj(&mut self) -> Vec<Effect> {
        if self.autodj == AutoDjMode::Off || self.autodj_pending {
            return Vec::new();
        }
        let needs_more = match self.queue.current {
            Some(index) => index + 1 >= self.queue.items.len(),
            // Nothing playing: only step in if there's nothing queued either,
            // so switching it on doesn't jump a queue the user just built.
            None => self.queue.items.is_empty(),
        };
        if !needs_more {
            return Vec::new();
        }

        self.autodj_pending = true;
        vec![Effect::Api(ApiCmd::AutoDj(self.dj_request()))]
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
            Event::TrackEnded => self.skip(false),
            Event::AudioFailed(e) => {
                self.audio_available = false;
                self.error(format!("audio unavailable: {e}"));
                Vec::new()
            }
            Event::PlaybackFailed(e) => {
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
                self.error(format!("skipping {what} — {e}"));
                // Manual, so repeat-one doesn't sit on the broken track.
                self.skip(true)
            }
            Event::Connected { server, id, username, token, ping } => {
                self.connected = true;
                self.connecting = false;
                self.connect.submitting = false;
                self.server = server;
                self.server_id = id;
                if token.is_some() {
                    self.token = token;
                }
                if username.is_some() {
                    self.username = username;
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

                let mut effects = vec![
                    Effect::Api(ApiCmd::Browse(self.opening_path())),
                    Effect::Audio(AudioCmd::SetVolume(self.volume)),
                ];
                // Worth persisting when we hold a token we logged in for — or
                // a pairing code, which is the only way back to this server
                // even when it needs no login at all.
                let signed_in = self.token.is_some() && self.username.is_some();
                if signed_in || self.tunnel_code.is_some() {
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
                self.server_id = id;
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1; // straight to the username
                self.info("tunnel open — sign in to continue");
                Vec::new()
            }
            Event::Listing(listing) => {
                self.path = listing.path.trim_matches('/').to_string();
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
            Event::AutoDjSample { tracks, pool, note } => {
                if let Some(panel) = self.dj_panel.as_mut() {
                    panel.sample_pending = false;
                    panel.sample = tracks;
                    // Keep the last pool size when this pick didn't report
                    // one: it still describes the settings on screen.
                    panel.pool = pool.or(panel.pool.take());
                }
                if let Some(note) = note {
                    self.info(note);
                }
                Vec::new()
            }
            Event::Journey { stops, note } => {
                // A journey the user closed while it was in flight stays
                // closed; the reply is no longer wanted.
                if let Some(journey) = self.journey.as_mut() {
                    journey.pending = false;
                    journey.stops = stops;
                    journey.offset = 0;
                }
                if let Some(note) = note {
                    self.info(note);
                }
                Vec::new()
            }
            Event::Genres(genres) => {
                if let Some(picker) =
                    self.dj_panel.as_mut().and_then(|panel| panel.genres.as_mut())
                {
                    picker.all = genres.into_iter().map(|g| g.name).collect();
                    picker.loading = false;
                    picker.row = picker.row.min(picker.all.len().saturating_sub(1));
                }
                Vec::new()
            }
            Event::AutoDjPick { candidates, ignore_list, note } => {
                self.autodj_pending = false;
                self.autodj_ignore = ignore_list;
                let explained = note.is_some();
                if let Some(note) = note {
                    self.info(note);
                }
                if self.autodj == AutoDjMode::Off {
                    return Vec::new(); // switched off while the request was out
                }

                let queued: std::collections::HashSet<String> =
                    self.queue.items.iter().map(|t| t.filepath.clone()).collect();
                let Some(pick) = candidates.into_iter().find(|t| !queued.contains(&t.filepath))
                else {
                    if !explained {
                        self.info("auto-dj: nothing new to add");
                    }
                    return Vec::new();
                };

                let label = pick.display_name();
                // If the queue already ran dry, this pick should start playing
                // rather than sit there.
                let start_it = self.queue.current.is_none() && self.status.is_idle();
                self.queue.push(pick);
                if !explained {
                    self.info(format!("auto-dj: {label}"));
                }
                if start_it {
                    return self.play_index(self.queue.items.len() - 1);
                }
                Vec::new()
            }
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
                self.playlist_open = Some(name);
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
                if !self.server.is_empty() {
                    self.connect.server = self.server.clone();
                }
                self.connect.stage = ConnectStage::Direct;
                self.connect.field = 1;
                self.token = None;
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

// ── Keymap ──────────────────────────────────────────────────────────────────
//
// One table per mode, and the help screen is rendered from it. Keeping a
// second hand-written list of "what the keys are" meant the help drifted
// from the truth: it still advertised four tabs after a fifth was added.

/// A key press, modifier included. Carrying the modifier is what stops
/// `Ctrl+D` from being mistaken for `d` — which it was, quietly deleting a
/// queue entry when a vim user reached for half-page-down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
}

const fn key(code: KeyCode) -> Key {
    Key { code, ctrl: false }
}
const fn ch(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: false }
}
const fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: true }
}

impl Key {
    /// How this key is written on the help screen.
    pub fn label(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::Backspace => "Bksp".to_string(),
            KeyCode::Up => "↑".to_string(),
            KeyCode::Down => "↓".to_string(),
            KeyCode::Left => "←".to_string(),
            KeyCode::Right => "→".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            other => format!("{other:?}"),
        };
        if self.ctrl { format!("^{base}") } else { base }
    }
}

impl Key {
    /// How this key is written in config.toml. Chosen to be typeable —
    /// `ctrl+d` rather than the `^d` the help screen shows.
    pub fn spec(self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "enter".to_string(),
            KeyCode::Esc => "esc".to_string(),
            KeyCode::Tab => "tab".to_string(),
            KeyCode::Backspace => "backspace".to_string(),
            KeyCode::Up => "up".to_string(),
            KeyCode::Down => "down".to_string(),
            KeyCode::Left => "left".to_string(),
            KeyCode::Right => "right".to_string(),
            KeyCode::Home => "home".to_string(),
            KeyCode::End => "end".to_string(),
            KeyCode::PageUp => "pageup".to_string(),
            KeyCode::PageDown => "pagedown".to_string(),
            other => format!("{other:?}").to_lowercase(),
        };
        if self.ctrl { format!("ctrl+{base}") } else { base }
    }

    /// Read a key back out of config.toml. Accepts the spelling [`Key::spec`]
    /// writes plus the obvious near-misses, since this is hand-edited.
    pub fn parse(spec: &str) -> Option<Key> {
        let spec = spec.trim();
        // `ctrl+d`, `Ctrl-D` and the help screen's own `^d` all mean one thing.
        let (ctrl, rest) = match spec.strip_prefix('^') {
            Some(rest) => (true, rest),
            None => {
                let lower = spec.to_ascii_lowercase();
                match lower.strip_prefix("ctrl+").or_else(|| lower.strip_prefix("ctrl-")) {
                    // The suffix is taken from the original so a bound
                    // capital survives the lowercasing done to find it.
                    Some(rest) => (true, &spec[spec.len() - rest.len()..]),
                    None => (false, spec),
                }
            }
        };
        if rest.is_empty() {
            return None;
        }
        let code = match rest.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" | "bksp" => KeyCode::Backspace,
            "up" | "↑" => KeyCode::Up,
            "down" | "↓" => KeyCode::Down,
            "left" | "←" => KeyCode::Left,
            "right" | "→" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            _ => {
                let mut chars = rest.chars();
                let only = chars.next()?;
                // More than one character and not a name above: not a key.
                if chars.next().is_some() {
                    return None;
                }
                // Case matters for a bare letter — `g` and `G` are different
                // bindings — but not with Ctrl, where the terminal sends the
                // lowercase form either way. Keeping the capital there would
                // make a binding that can never fire.
                if ctrl { KeyCode::Char(only.to_ascii_lowercase()) } else { KeyCode::Char(only) }
            }
        };
        Some(Key { code, ctrl })
    }
}

impl Action {
    /// The name this action answers to in `[keys]`. `None` for actions that
    /// are the app talking to itself rather than something to bind.
    pub fn name(&self) -> Option<&'static str> {
        Some(match self {
            Action::Up => "up",
            Action::Down => "down",
            Action::PageUp => "page-up",
            Action::PageDown => "page-down",
            Action::HalfPageUp => "half-page-up",
            Action::HalfPageDown => "half-page-down",
            Action::First => "first",
            Action::Last => "last",
            Action::Activate => "open",
            Action::Back => "back",
            Action::AddToQueue => "add-to-queue",
            Action::CycleFocus => "switch-pane",
            Action::SelectTab(0) => "tab-1",
            Action::SelectTab(1) => "tab-2",
            Action::SelectTab(2) => "tab-3",
            Action::SelectTab(3) => "tab-4",
            Action::SelectTab(4) => "tab-5",
            Action::StartSearch => "search",
            Action::StartFilter => "filter",
            Action::CycleViz => "visualiser-mode",
            Action::ToggleScatter => "visualiser-dots",
            Action::PlayPause => "play-pause",
            Action::NextTrack => "next-track",
            Action::PrevTrack => "previous-track",
            Action::JumpToPlaying => "jump-to-playing",
            Action::ToggleNowPlaying => "now-playing",
            Action::SeekForward => "seek-forward",
            Action::SeekBackward => "seek-back",
            Action::SeekForwardFar => "seek-forward-far",
            Action::SeekBackwardFar => "seek-back-far",
            Action::VolumeUp => "volume-up",
            Action::VolumeDown => "volume-down",
            Action::RemoveFromQueue => "remove-from-queue",
            Action::ClearQueue => "clear-queue",
            Action::ToggleRepeat => "repeat",
            Action::ToggleShuffle => "shuffle",
            Action::ToggleAutoDj => "auto-dj",
            Action::OpenDjPanel => "auto-dj-panel",
            Action::StartJourney => "sonic-journey",
            Action::ToggleHelp => "help",
            Action::Quit => "quit",
            // Text entry, and the keys the overlays claim for themselves.
            // Those tables aren't rebindable — the panel and the full-screen
            // view both draw their own hints, so the keys are on screen where
            // they apply rather than in the config file.
            Action::SelectTab(_)
            | Action::Input(_)
            | Action::Backspace
            | Action::Submit
            | Action::Cancel
            | Action::NowTabNext
            | Action::NowTabPrev => return None,
        })
    }

    pub fn from_name(name: &str) -> Option<Action> {
        // Derived from `name()` rather than a second list, so the two cannot
        // disagree about what anything is called.
        default_normal().into_iter().map(|b| b.action).find(|a| a.name() == Some(name))
    }
}

/// One action and every key that fires it.
#[derive(Debug, Clone)]
pub struct Binding {
    pub keys: Vec<Key>,
    pub action: Action,
    /// What it does, for the help screen. `None` keeps a binding working but
    /// off the list.
    pub help: Option<&'static str>,
}

/// The player's keys. Order is the order the help lists them, so related
/// things are grouped rather than sorted.
fn default_normal() -> Vec<Binding> {
    vec![
    Binding { keys: vec![ch('j'), key(KeyCode::Down)], action: Action::Down, help: Some("move down") },
    Binding { keys: vec![ch('k'), key(KeyCode::Up)], action: Action::Up, help: Some("move up") },
    Binding {
        keys: vec![key(KeyCode::PageDown)],
        action: Action::PageDown,
        help: Some("a screenful down"),
    },
    Binding { keys: vec![key(KeyCode::PageUp)], action: Action::PageUp, help: Some("a screenful up") },
    // Half a page, as in vim — and reachable without a Fn key, which is why
    // it earns its place beside PgUp/PgDn rather than replacing them.
    Binding {
        keys: vec![ctrl('d')],
        action: Action::HalfPageDown,
        help: Some("half that, down"),
    },
    Binding { keys: vec![ctrl('u')], action: Action::HalfPageUp, help: Some("half that, up") },
    Binding {
        keys: vec![ch('g'), key(KeyCode::Home)],
        action: Action::First,
        help: Some("first"),
    },
    Binding { keys: vec![ch('G'), key(KeyCode::End)], action: Action::Last, help: Some("last") },
    Binding {
        keys: vec![key(KeyCode::Enter), ch('l'), key(KeyCode::Right)],
        action: Action::Activate,
        help: Some("open, or play from here"),
    },
    Binding {
        keys: vec![ch('h'), key(KeyCode::Left)],
        action: Action::Back,
        help: Some("go back"),
    },
    Binding { keys: vec![ch('a')], action: Action::AddToQueue, help: Some("add to queue") },
    Binding {
        keys: vec![key(KeyCode::Tab)],
        action: Action::CycleFocus,
        help: Some("show / hide the queue"),
    },
    // One digit per visible tab; `select_tab` ignores a number past the end,
    // so a server without Discover simply has nothing on 5. Listed one per
    // row because "which tab is 3" is the thing a reader actually wants.
    Binding { keys: vec![ch('1')], action: Action::SelectTab(0), help: Some("Files") },
    Binding { keys: vec![ch('2')], action: Action::SelectTab(1), help: Some("Library") },
    Binding { keys: vec![ch('3')], action: Action::SelectTab(2), help: Some("Playlists") },
    Binding { keys: vec![ch('4')], action: Action::SelectTab(3), help: Some("Search") },
    Binding {
        keys: vec![ch('5')],
        action: Action::SelectTab(4),
        help: Some("Discover, if enabled"),
    },
    Binding { keys: vec![ch('/')], action: Action::StartSearch, help: Some("search") },
    // `/` already asks the server. This one narrows what is already on
    // screen, which is a different enough job to want its own key.
    Binding { keys: vec![ch('f')], action: Action::StartFilter, help: Some("filter this list") },
    Binding { keys: vec![ch(' ')], action: Action::PlayPause, help: Some("play or pause") },
    Binding { keys: vec![ch('n')], action: Action::NextTrack, help: Some("next track") },
    Binding { keys: vec![ch('p')], action: Action::PrevTrack, help: Some("previous track") },
    Binding {
        keys: vec![ch('i')],
        action: Action::JumpToPlaying,
        help: Some("jump to what's playing"),
    },
    // Zero reads as "the screen before the tabs": 1..5 are places to go, this
    // is where you already are.
    Binding {
        keys: vec![ch('0')],
        action: Action::ToggleNowPlaying,
        help: Some("full-screen now playing"),
    },
    Binding {
        keys: vec![ch(']')],
        action: Action::SeekForward,
        help: Some("seek 5s forward"),
    },
    Binding { keys: vec![ch('[')], action: Action::SeekBackward, help: Some("seek 5s back") },
    Binding {
        keys: vec![ch('}')],
        action: Action::SeekForwardFar,
        help: Some("seek a minute forward"),
    },
    Binding {
        keys: vec![ch('{')],
        action: Action::SeekBackwardFar,
        help: Some("seek a minute back"),
    },
    Binding { keys: vec![ch('+'), ch('=')], action: Action::VolumeUp, help: Some("louder") },
    Binding { keys: vec![ch('-')], action: Action::VolumeDown, help: Some("quieter") },
    Binding {
        keys: vec![ch('d')],
        action: Action::RemoveFromQueue,
        help: Some("remove from queue"),
    },
    Binding { keys: vec![ch('C')], action: Action::ClearQueue, help: Some("clear the queue") },
    Binding { keys: vec![ch('r')], action: Action::ToggleRepeat, help: Some("repeat") },
    Binding { keys: vec![ch('s')], action: Action::ToggleShuffle, help: Some("shuffle") },
    Binding { keys: vec![ch('A')], action: Action::ToggleAutoDj, help: Some("auto-dj on / off") },
    Binding { keys: vec![ch('D')], action: Action::OpenDjPanel, help: Some("auto-dj settings") },
    Binding { keys: vec![ch('J')], action: Action::StartJourney, help: Some("sonic journey") },
    Binding {
        keys: vec![ch('?'), key(KeyCode::Esc)],
        action: Action::ToggleHelp,
        help: Some("this help"),
    },
    Binding { keys: vec![ch('q'), ctrl('c')], action: Action::Quit, help: Some("quit") },
    ]
}

/// Keys while a modal overlay is up. It gets its own set rather than
/// borrowing the player's: sharing them meant `p` arrived as "previous
/// track" and the panel's own sample key did nothing.
///
/// Not configurable, deliberately: a panel draws its own hints along the
/// bottom, and those would start lying the moment its keys could move.
fn default_panel() -> Vec<Binding> {
    vec![
    Binding { keys: vec![ch('j'), key(KeyCode::Down)], action: Action::Down, help: None },
    Binding { keys: vec![ch('k'), key(KeyCode::Up)], action: Action::Up, help: None },
    Binding {
        keys: vec![ch('h'), ch('['), key(KeyCode::Left)],
        action: Action::Back,
        help: None,
    },
    Binding {
        keys: vec![ch('l'), ch(']'), key(KeyCode::Right)],
        action: Action::SeekForward,
        help: None,
    },
    Binding { keys: vec![ch('g'), key(KeyCode::Home)], action: Action::First, help: None },
    Binding { keys: vec![ch('G'), key(KeyCode::End)], action: Action::Last, help: None },
    Binding { keys: vec![key(KeyCode::Enter)], action: Action::Submit, help: None },
    Binding { keys: vec![ch(' ')], action: Action::PlayPause, help: None },
    Binding { keys: vec![key(KeyCode::Esc), ch('D')], action: Action::Cancel, help: None },
    Binding { keys: vec![ch('q'), ctrl('c')], action: Action::Quit, help: None },
    ]
}

/// What the full-screen view claims for itself. Everything absent from here
/// falls through to the normal bindings, which is the point: this list is only
/// the keys that mean something different once the browser is off screen.
fn default_now() -> Vec<Binding> {
    vec![
    Binding {
        keys: vec![key(KeyCode::Right), ch('l'), key(KeyCode::Tab)],
        action: Action::NowTabNext,
        help: None,
    },
    Binding { keys: vec![key(KeyCode::Left), ch('h')], action: Action::NowTabPrev, help: None },
    // The visualiser's own tab, on its own key: `←→` already belongs to the
    // panel's tabs and `↑↓` to whatever list is in them.
    Binding { keys: vec![ch('v')], action: Action::CycleViz, help: None },
    // A dot, for the mode that draws dots. `s` and `p` would have been
    // the mnemonic picks and both are taken by transport keys that this
    // view deliberately lets through.
    Binding { keys: vec![ch('.')], action: Action::ToggleScatter, help: None },
    // Esc leaves, because a screen filling the terminal should close the way
    // every other full-screen thing does. `0` still toggles.
    Binding { keys: vec![key(KeyCode::Esc)], action: Action::ToggleNowPlaying, help: None },
    ]
}

/// The bindings in force: the defaults, with whatever `[keys]` in config.toml
/// had to say about them.
#[derive(Debug, Clone)]
pub struct Keymap {
    normal: Vec<Binding>,
    panel: Vec<Binding>,
    /// Claimed by the full-screen view, which then falls through to `normal`.
    /// Only the keys whose meaning actually changes go here; everything else
    /// keeps working there, including whatever `[keys]` moved it to.
    now: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap { normal: default_normal(), panel: default_panel(), now: default_now() }
    }
}

impl Keymap {
    /// Layer a config's `[keys]` over the defaults.
    ///
    /// Naming an action **replaces** its keys rather than adding to them, so
    /// a binding can be moved or removed outright (`action = []`). Everything
    /// unmentioned keeps its default.
    ///
    /// A key the user asks for is taken from whatever held it before: binding
    /// `d` to something new should not also require unbinding it from the old
    /// thing. Problems are collected rather than raised — a typo in one line
    /// must not cost someone their music player.
    pub fn with_overrides(
        mut self,
        overrides: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        // Who has asked for each key. First claim wins: two lines wanting one
        // key is a contradiction, and cancelling both — which is what
        // stripping each in turn does — would leave the key doing nothing at
        // all, which is nobody's intent.
        let mut claimed: std::collections::HashMap<Key, Action> = std::collections::HashMap::new();

        for (name, specs) in overrides {
            let Some(action) = Action::from_name(name) else {
                warnings.push(format!("[keys] has no action called '{name}'"));
                continue;
            };
            let Some(slot) = self.normal.iter().position(|b| b.action == action) else {
                warnings.push(format!("[keys] '{name}' cannot be rebound"));
                continue;
            };
            let mut keys = Vec::new();
            for spec in specs {
                let Some(key) = Key::parse(spec) else {
                    warnings.push(format!("[keys] '{name}': '{spec}' is not a key"));
                    continue;
                };
                match claimed.get(&key) {
                    Some(first) if *first != action => {
                        warnings.push(format!(
                            "[keys] '{spec}' is bound to both {} and {name} — keeping {}",
                            first.name().unwrap_or("?"),
                            first.name().unwrap_or("?"),
                        ));
                        continue;
                    }
                    _ => {
                        claimed.insert(key, action.clone());
                        keys.push(key);
                    }
                }
            }
            // A line whose every key was unreadable is a mistake, not a
            // request to unbind — that is what an explicit `[]` is for. Leave
            // the default alone; the warning already says what went wrong.
            if keys.is_empty() && !specs.is_empty() {
                warnings.push(format!("[keys] '{name}' left as it was"));
                continue;
            }
            self.normal[slot].keys = keys;
        }

        // Take each claimed key off whatever held it before, so binding `d`
        // to something new doesn't also mean unbinding it from the old thing
        // by hand.
        for (key, owner) in &claimed {
            for binding in &mut self.normal {
                if binding.action != *owner {
                    binding.keys.retain(|k| k != key);
                }
            }
        }

        (self, warnings)
    }

    /// What a key press means in this mode.
    pub fn action(&self, key: KeyEvent, mode: InputMode) -> Option<Action> {
        let pressed = Key { code: key.code, ctrl: key.modifiers.contains(KeyModifiers::CONTROL) };
        // Ctrl+C quits from anywhere, including mid-typing, and is not
        // rebindable — it is the way out when everything else is confusing.
        if pressed == ctrl('c') {
            return Some(Action::Quit);
        }

        if mode == InputMode::Editing {
            return match key.code {
                KeyCode::Char(c) => Some(Action::Input(c)),
                KeyCode::Backspace => Some(Action::Backspace),
                KeyCode::Enter => Some(Action::Submit),
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Tab => Some(Action::CycleFocus),
                KeyCode::Down => Some(Action::Down),
                KeyCode::Up => Some(Action::Up),
                _ => None,
            };
        }

        // The full-screen view is a view, not a modal: it takes the handful of
        // keys whose meaning changes there and lets the rest through, so
        // pause, skip and seek go on working and go on obeying `[keys]`.
        if mode == InputMode::Now {
            if let Some(binding) = self.now.iter().find(|b| b.keys.contains(&pressed)) {
                return Some(binding.action.clone());
            }
        }

        let table = if mode == InputMode::Panel { &self.panel } else { &self.normal };
        if let Some(binding) = table.iter().find(|b| b.keys.contains(&pressed)) {
            return Some(binding.action.clone());
        }

        // Anything else typed into a panel is the panel's own business (the
        // Auto-DJ sample key, say). Ctrl combinations are not: an unbound one
        // must do nothing rather than arrive as a bare letter.
        if mode == InputMode::Panel && !pressed.ctrl {
            if let KeyCode::Char(c) = key.code {
                return Some(Action::Input(c));
            }
        }
        None
    }

    /// The rows the help screen draws, in table order.
    pub fn help_rows(&self) -> Vec<(String, &'static str)> {
        self.normal
            .iter()
            .filter_map(|binding| {
                let what = binding.help?;
                let keys = binding.keys.iter().map(|k| k.label()).collect::<Vec<_>>().join(" ");
                // A binding with every key taken away has nothing to show.
                (!keys.is_empty()).then_some((keys, what))
            })
            .collect()
    }

    /// The whole map as a `[keys]` section, ready to paste into config.toml.
    pub fn to_config_toml(&self) -> String {
        let mut out = String::from(
            "# Auto-DJ panel and other overlays keep their own keys.\n\
             # Naming an action replaces its keys; give it [] to unbind it.\n\
             [keys]\n",
        );
        for binding in &self.normal {
            let Some(name) = binding.action.name() else { continue };
            let keys: Vec<String> =
                binding.keys.iter().map(|k| format!("\"{}\"", k.spec())).collect();
            out.push_str(&format!("{name} = [{}]\n", keys.join(", ")));
        }
        out
    }
}

/// Map a key press using the *default* bindings.
///
/// Test-only: the player reads `app.keymap`, which may have been rebound.
#[cfg(test)]
pub fn map_key(key: KeyEvent, mode: InputMode) -> Option<Action> {
    static DEFAULTS: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(Keymap::default).action(key, mode)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{DirEntry, FileEntry, FileMetadata, SearchResults, TrackMetadata};

    fn track(path: &str) -> Track {
        Track { filepath: path.to_string(), metadata: TrackMetadata::default() }
    }

    /// The source the app actually asked for. A status has to name it to be
    /// about the track now playing, so tests cannot invent one.
    fn played_url(effects: &[Effect]) -> String {
        effects
            .iter()
            .find_map(|effect| match effect {
                Effect::Audio(AudioCmd::Play { url, .. }) => Some(url.clone()),
                _ => None,
            })
            .expect("expected a play effect")
    }

    fn track_by(path: &str, artist: &str) -> Track {
        Track {
            filepath: path.to_string(),
            metadata: TrackMetadata { artist: Some(artist.to_string()), ..Default::default() },
        }
    }

    /// A session against a fully-featured server. Capabilities are set
    /// explicitly because they change what the UI offers — a default (empty)
    /// set would silently be testing the degraded path.
    fn connected_app() -> App {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.capabilities = crate::api::types::Capabilities {
            discovery: true,
            discovery_path: true,
            discovery_p2p: false,
            federation_discovery: false,
        };
        app
    }

    fn listing(path: &str, dirs: &[&str], files: &[&str]) -> DirListing {
        DirListing {
            path: path.to_string(),
            directories: dirs.iter().map(|d| DirEntry { name: (*d).to_string() }).collect(),
            files: files
                .iter()
                .map(|f| FileEntry {
                    name: (*f).to_string(),
                    kind: Some("mp3".into()),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn listing_becomes_entries_with_qualified_paths() {
        let entries = entries_from_listing(&listing("/lib/Artist/", &["Album"], &["song.mp3"]), "");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], Entry::Parent);
        assert!(matches!(&entries[1], Entry::Dir { path, .. } if path == "lib/Artist/Album"));
        assert!(
            matches!(&entries[2], Entry::Track { track, .. } if track.filepath == "lib/Artist/song.mp3")
        );
    }

    /// Connect for real, so the library list comes from a ping rather than
    /// being poked into the field the browser reads.
    fn app_with_libraries(vpaths: &[&str]) -> App {
        let mut app = connected_app();
        app.apply_event(Event::Connected {
            server: "http://host:3000".into(),
            id: "http://host:3000".into(),
            username: None,
            token: None,
            ping: Box::new(crate::api::types::Ping {
                vpaths: vpaths.iter().map(|v| (*v).to_string()).collect(),
                ..Default::default()
            }),
        });
        app
    }

    #[test]
    fn the_only_library_is_the_top_of_the_browser() {
        let mut app = app_with_libraries(&["library"]);
        app.path = "library".into();
        app.apply_event(Event::Listing(Box::new(listing("library", &["Artist"], &[]))));

        // Nothing above it: the list of libraries would be this one row.
        assert!(!app.files.entries.contains(&Entry::Parent), "{:?}", app.files.entries);
        assert!(app.handle_action(Action::Back).is_empty(), "back asks for nothing");
        assert_eq!(app.path, "library", "and stays put");

        // A folder inside it still has its way back.
        app.handle_action(Action::Activate);
        assert_eq!(app.path, "library/Artist");
        app.apply_event(Event::Listing(Box::new(listing(
            "library/Artist",
            &["Album"],
            &[],
        ))));
        assert!(app.files.entries.contains(&Entry::Parent));
        // No request: the trail already holds the listing we came through.
        assert!(app.handle_action(Action::Back).is_empty());
        assert_eq!(app.path, "library");
        assert!(!app.files.entries.contains(&Entry::Parent), "and the top has no way up");
    }

    #[test]
    fn several_libraries_keep_the_list_of_them_reachable() {
        // With a choice to make, the top-level listing is worth going back to.
        let mut app = app_with_libraries(&["music", "podcasts"]);
        app.path = "music".into();
        app.apply_event(Event::Listing(Box::new(listing("music", &["Artist"], &[]))));

        assert!(app.files.entries.contains(&Entry::Parent));
        assert_eq!(
            app.handle_action(Action::Back),
            vec![Effect::Api(ApiCmd::Browse(String::new()))]
        );
    }

    fn type_filter(app: &mut App, text: &str) {
        app.handle_action(Action::StartFilter);
        for c in text.chars() {
            app.handle_action(Action::Input(c));
        }
    }

    fn labels(app: &App) -> Vec<&str> {
        app.pane().entries.iter().map(Entry::label).collect()
    }

    #[test]
    fn a_filter_narrows_the_list_without_losing_the_way_out() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/",
            &["Bassnectar", "Basshunter", "Portishead"],
            &["bass solo.mp3"],
        ))));

        type_filter(&mut app, "BASS");
        // Case folds both ways, files and folders alike, and `..` is not a
        // result so it is never filtered away.
        assert_eq!(labels(&app), vec!["..", "Bassnectar", "Basshunter", "bass solo.mp3"]);
        assert_eq!(app.pane().counts(), (3, 4));
        assert_eq!(app.input_mode(), InputMode::Editing, "the prompt has the keys");

        // Narrowing further happens on the keystroke, with nothing to submit.
        app.handle_action(Action::Input('h'));
        assert_eq!(labels(&app), vec!["..", "Basshunter"]);

        // Backspacing widens again, from the list that was never thrown away.
        app.handle_action(Action::Backspace);
        assert_eq!(labels(&app).len(), 4);
    }

    #[test]
    fn a_filter_survives_being_typed_but_not_a_new_listing() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));

        type_filter(&mut app, "alp");
        app.handle_action(Action::Submit);
        assert!(!app.filtering, "the prompt closes");
        assert_eq!(app.pane().filter, "alp", "and what was typed stays");
        assert_eq!(app.input_mode(), InputMode::Normal, "the list has the keys back");
        assert_eq!(labels(&app), vec!["..", "Alpha"]);

        // A different list is not the list the filter was typed against.
        app.apply_event(Event::Listing(Box::new(listing("/lib/Alpha/", &["One", "Two"], &[]))));
        assert!(app.pane().filter.is_empty());
        assert_eq!(labels(&app), vec!["..", "One", "Two"]);
    }

    #[test]
    fn escaping_a_filter_puts_the_whole_list_back() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));

        type_filter(&mut app, "alp");
        app.handle_action(Action::Cancel);
        assert!(!app.filtering);
        assert!(app.pane().filter.is_empty());
        assert_eq!(labels(&app), vec!["..", "Alpha", "Beta"]);

        // Backspacing past the start leaves too, having changed nothing.
        type_filter(&mut app, "");
        app.handle_action(Action::Backspace);
        assert!(!app.filtering);
        assert_eq!(labels(&app).len(), 3);
    }

    #[test]
    fn the_column_behind_a_filtered_pick_holds_the_whole_folder() {
        // The filter found the row; the folder it was in is the context worth
        // keeping. Coming back out has nothing hidden and no filter left to
        // explain why.
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/",
            &["Alpha", "Beta", "Betamax"],
            &[],
        ))));

        type_filter(&mut app, "betam");
        assert_eq!(labels(&app), vec!["..", "Betamax"]);
        app.handle_action(Action::Submit);
        app.handle_action(Action::Activate);
        assert_eq!(app.path, "lib/Betamax");

        let step = &app.files.trail[0];
        assert_eq!(
            step.entries.iter().map(Entry::label).collect::<Vec<_>>(),
            vec!["..", "Alpha", "Beta", "Betamax"]
        );
        assert_eq!(step.entries[step.chosen].label(), "Betamax", "marked where we went in");

        app.apply_event(Event::Listing(Box::new(listing("/lib/Betamax/", &["Tape"], &[]))));
        app.handle_action(Action::Back);
        assert_eq!(labels(&app), vec!["..", "Alpha", "Beta", "Betamax"]);
        assert!(app.pane().filter.is_empty(), "and no filter came back with it");
    }

    #[test]
    fn every_tab_filters_its_own_list() {
        use crate::tui::worker::{LibraryData, LibraryNode};
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha", "Beta"], &[]))));
        type_filter(&mut app, "alp");
        app.handle_action(Action::Submit);

        app.handle_action(Action::SelectTab(1));
        app.apply_event(Event::Library {
            node: LibraryNode::Root,
            data: LibraryData::Artists(vec!["Bassnectar".into(), "Portishead".into()]),
        });
        assert!(app.pane().filter.is_empty(), "a tab does not inherit another's filter");
        type_filter(&mut app, "port");
        app.handle_action(Action::Submit);
        assert_eq!(labels(&app), vec!["..", "Portishead"]);

        app.handle_action(Action::SelectTab(0));
        assert_eq!(app.pane().filter, "alp", "and keeps its own when you come back");
        assert_eq!(labels(&app), vec!["..", "Alpha"]);
    }

    #[test]
    fn root_listing_has_no_parent_entry() {
        let entries = entries_from_listing(&listing("/", &["lib"], &[]), "");
        assert_eq!(entries.len(), 1);
        assert!(matches!(&entries[0], Entry::Dir { .. }));
    }

    #[test]
    fn navigating_into_and_back_out_of_directories() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/", &["lib"], &[]))));

        let effects = app.handle_action(Action::Activate);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse("lib".into()))]);
        assert_eq!(app.path, "lib");

        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist"], &[]))));
        app.handle_action(Action::Down); // onto "Artist"
        let effects = app.handle_action(Action::Activate);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse("lib/Artist".into()))]);

        // Back up one level. The listing is already in hand, so this costs
        // nothing and lands on the row we came through rather than the top.
        let effects = app.handle_action(Action::Back);
        assert!(effects.is_empty(), "going back asks the server for nothing");
        assert_eq!(app.path, "lib");
        assert_eq!(app.files.entries.len(), 2, "'..' and Artist, restored");
        assert_eq!(app.files.state.selected(), Some(1), "back on Artist");

        let effects = app.handle_action(Action::Back);
        assert!(effects.is_empty());
        assert_eq!(app.path, "");
        // At the root there is nowhere further up.
        assert!(app.handle_action(Action::Back).is_empty());
    }

    #[test]
    fn enter_on_a_track_queues_the_directory_and_starts_there() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/",
            &["sub"],
            &["a.mp3", "b.mp3", "c.mp3"],
        ))));

        // Rows: [.., sub, a, b, c]. The cursor starts on "sub", not on "..".
        assert_eq!(app.files.state.selected(), Some(1));
        for _ in 0..2 {
            app.handle_action(Action::Down); // onto "b"
        }
        let effects = app.handle_action(Action::Activate);

        assert_eq!(app.queue.items.len(), 3, "only playable rows are queued");
        assert_eq!(app.queue.current, Some(1), "playback starts at the selected track");
        match &effects[0] {
            Effect::Audio(AudioCmd::Play { url, .. }) => {
                assert_eq!(url, "http://host:3000/media/lib/b.mp3?token=tok");
            }
            other => panic!("expected a play effect, got {other:?}"),
        }
    }

    #[test]
    fn queue_advances_on_track_end_and_stops_at_the_end() {
        let mut app = connected_app();
        app.queue.replace(vec![track("lib/a.mp3"), track("lib/b.mp3")]);
        app.play_index(0);

        let effects = app.apply_event(Event::TrackEnded);
        assert_eq!(app.queue.current, Some(1));
        assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));

        // End of the last track with repeat off: stop, don't wrap.
        let effects = app.apply_event(Event::TrackEnded);
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)]);
        assert_eq!(app.queue.current, None);
        assert!(app.now_playing.is_none());
    }

    #[test]
    fn the_track_coming_up_never_wears_the_state_of_the_one_that_ended() {
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b")]);
        let effects = app.handle_action(Action::PlayPause);
        let first = played_url(&effects);
        app.apply_event(Event::Status(PlayerStatus {
            source: first.clone(),
            playing: true,
            position: 174.0,
            duration: 174.0,
            ..Default::default()
        }));
        assert!(app.status.playing);

        // It runs out and the queue moves on. Nothing is sounding yet, and
        // saying so under the next track's name is what read as the player
        // stopping between every song.
        let effects = app.apply_event(Event::TrackEnded);
        assert_eq!(app.queue.current, Some(1));
        assert!(app.is_starting());
        assert_eq!(app.status.position, 0.0, "the old position does not carry over");

        // The poll that comes next is the engine still describing the track
        // that ended -- an empty source, because it cleared it.
        app.apply_event(Event::Status(PlayerStatus::default()));
        assert!(app.is_starting(), "a status about nothing is not the answer");
        assert_eq!(app.status.source, played_url(&effects));

        // The answer names what was asked for.
        app.apply_event(Event::Status(PlayerStatus {
            source: played_url(&effects),
            playing: true,
            ..Default::default()
        }));
        assert!(!app.is_starting());
        assert!(app.status.playing);
    }

    #[test]
    fn a_length_already_known_is_shown_before_the_engine_confirms_it() {
        // The bar had no total until the first status came back, so a track
        // whose length the library already told us started out as `--:--`.
        let mut app = connected_app();
        let mut long = track("a");
        long.metadata.duration = Some(221.0);
        app.queue.replace(vec![long]);
        app.handle_action(Action::PlayPause);
        assert_eq!(app.status.duration, 221.0);
    }

    #[test]
    fn repeat_all_wraps_but_repeat_one_only_traps_automatic_advance() {
        let mut queue = Queue {
            items: vec![track("a"), track("b")],
            current: Some(1),
            ..Default::default()
        };

        assert_eq!(queue.next_index(false), None);
        queue.repeat = Repeat::All;
        assert_eq!(queue.next_index(false), Some(0));

        queue.repeat = Repeat::One;
        queue.current = Some(0);
        assert_eq!(queue.next_index(false), Some(0), "auto-advance repeats the track");
        assert_eq!(queue.next_index(true), Some(1), "a manual skip escapes repeat-one");
    }

    #[test]
    fn previous_restarts_the_track_when_past_the_grace_window() {
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b")]);
        app.play_index(1);

        app.status.position = 10.0;
        assert_eq!(
            app.handle_action(Action::PrevTrack),
            vec![Effect::Audio(AudioCmd::Seek(0.0))]
        );

        app.status.position = 1.0;
        app.handle_action(Action::PrevTrack);
        assert_eq!(app.queue.current, Some(0));
    }

    #[test]
    fn removing_the_playing_track_stops_playback() {
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b"), track("c")]);
        app.play_index(1);
        app.focus = Focus::Queue;
        app.queue.state.select(Some(1));

        let effects = app.handle_action(Action::RemoveFromQueue);
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)]);
        assert_eq!(app.queue.items.len(), 2);
        assert_eq!(app.queue.current, None);
    }

    #[test]
    fn removing_an_earlier_track_keeps_the_current_one_playing() {
        let mut queue = Queue {
            items: vec![track("a"), track("b"), track("c")],
            current: Some(2),
            ..Default::default()
        };
        assert!(!queue.remove(0));
        assert_eq!(queue.current, Some(1), "index follows the still-playing track");
    }

    #[test]
    fn adding_to_an_empty_idle_queue_starts_playback() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &["a.mp3"]))));
        let effects = app.handle_action(Action::AddToQueue);
        assert_eq!(app.queue.items.len(), 1);
        assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));

        // A second add while something is loaded just queues.
        app.status.source = "http://host/media/lib/a.mp3".into();
        let effects = app.handle_action(Action::AddToQueue);
        assert_eq!(app.queue.items.len(), 2);
        assert!(effects.is_empty());
    }

    #[test]
    fn volume_is_clamped() {
        let mut app = connected_app();
        for _ in 0..30 {
            app.handle_action(Action::VolumeUp);
        }
        assert_eq!(app.volume, 1.0);
        for _ in 0..40 {
            app.handle_action(Action::VolumeDown);
        }
        assert_eq!(app.volume, 0.0);
    }

    #[test]
    fn seeking_backward_never_goes_negative() {
        let mut app = connected_app();
        app.status.source = "http://host/a.mp3".into();
        app.status.position = 2.0;
        assert_eq!(
            app.handle_action(Action::SeekBackward),
            vec![Effect::Audio(AudioCmd::Seek(0.0))]
        );
    }

    #[test]
    fn seeking_while_idle_does_nothing() {
        let mut app = connected_app();
        assert!(app.handle_action(Action::SeekForward).is_empty());
    }

    #[test]
    fn every_class_a_search_matched_is_reachable() {
        use crate::api::types::{SearchGroup, SearchTrack};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(3));
        let hit = |p: &str| SearchTrack {
            name: p.to_string(),
            filepath: p.to_string(),
            album_art_file: None,
            metadata: TrackMetadata::default(),
        };
        app.apply_event(Event::SearchResults(Box::new(SearchResults {
            artists: vec![SearchGroup { name: "Moon Hooch".into(), album_art_file: None }],
            albums: vec![],
            title: vec![hit("lib/a.mp3")],
            files: vec![hit("lib/a.mp3"), hit("lib/b.mp3")],
            lyrics: vec![],
        })));

        // The artist hit used to be counted into a sentence and thrown away.
        // Classes that matched nothing stay out of the menu.
        let menu: Vec<&str> = app
            .search
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Search { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(menu, vec!["Artists", "Titles", "Filenames"]);
        assert_eq!(app.search_summary.as_deref(), Some("4 matches"));

        // Opening a class needs no request -- the whole reply is in hand.
        assert!(app.handle_action(Action::Activate).is_empty(), "no request to open a class");
        assert_eq!(app.search_node(), &SearchNode::Class(SearchClass::Artists));

        // And the artist opens the same place the Library tab would, under a
        // command of its own so the reply cannot land in the wrong column.
        let effects = app.handle_action(Action::Activate);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::SearchDrill(LibraryNode::Artist("Moon Hooch".into())))]
        );

        // A track that matched on two classes is listed under both, which is
        // the point: it says the match was found two ways.
        app.handle_action(Action::Back);
        app.handle_action(Action::Back);
        app.handle_action(Action::Down);
        app.handle_action(Action::Down);
        app.handle_action(Action::Activate);
        assert_eq!(app.search_node(), &SearchNode::Class(SearchClass::Files));
        assert_eq!(app.search.entries.len(), 3, "'..' plus both filename hits");
    }

    #[test]
    fn unauthorized_returns_to_the_connect_screen() {
        let mut app = connected_app();
        app.apply_event(Event::Unauthorized);
        assert!(!app.connected);
        assert!(app.token.is_none());
        assert_eq!(app.input_mode(), InputMode::Editing);
    }

    #[test]
    fn a_public_mode_server_picked_from_the_network_connects_outright() {
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
            name: "Open Server".into(),
            base_url: "http://192.168.1.5:3000".into(),
            version: None,
            quick_connect: false,
        }]));
        app.handle_action(Action::Submit);

        app.apply_event(Event::Connected {
            server: "http://192.168.1.5:3000".into(),
            id: "http://192.168.1.5:3000".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        assert!(app.connected, "no login step when the server doesn't want one");
    }

    #[test]
    fn the_first_screen_offers_both_ways_in() {
        let mut app = App::new(None, None, None);
        assert_eq!(app.connect.stage, ConnectStage::Choosing);
        assert_eq!(CONNECT_METHODS.len(), 2);

        // Direct is the default choice.
        app.handle_action(Action::Submit);
        assert_eq!(app.connect.stage, ConnectStage::Direct);

        // Esc returns to the chooser, where Down picks Quick Connect.
        app.handle_action(Action::Cancel);
        assert_eq!(app.connect.stage, ConnectStage::Choosing);
        app.handle_action(Action::Down);
        app.handle_action(Action::Submit);
        assert_eq!(app.connect.stage, ConnectStage::QuickConnect);
    }

    #[test]
    fn the_chooser_selection_stays_in_range() {
        let mut app = App::new(None, None, None);
        for _ in 0..5 {
            app.handle_action(Action::Up);
        }
        assert_eq!(app.connect.choice, 0);
        for _ in 0..5 {
            app.handle_action(Action::Down);
        }
        assert_eq!(app.connect.choice, CONNECT_METHODS.len() - 1);
    }

    #[test]
    fn opening_quick_connect_starts_a_network_search() {
        let mut app = App::new(None, None, None);
        app.handle_action(Action::Down); // onto Quick Connect
        let effects = app.handle_action(Action::Submit);
        assert_eq!(app.connect.stage, ConnectStage::QuickConnect);
        assert!(effects.contains(&Effect::Discover));
        assert!(app.connect.searching);
    }

    #[test]
    fn choosing_a_discovered_server_connects_to_it_directly() {
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
            name: "Living Room".into(),
            base_url: "http://192.168.1.71:3999".into(),
            version: None,
            quick_connect: true,
        }]));
        assert!(!app.connect.searching);

        // A server on this network needs no tunnel and no code.
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Connect {
                server: "http://192.168.1.71:3999".into(),
                token: None,
            })]
        );
    }

    #[test]
    fn late_discovery_results_do_not_move_the_cursor_off_the_paste_row() {
        // Found live: the browse takes seconds, so a pasted code can be
        // submitted before it answers. Row 0 means "paste" with an empty list
        // and "first server" with a populated one, so the arriving results
        // used to retarget Enter at a server the user never chose.
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        for c in "mstr1:abc".chars() {
            app.handle_action(Action::Input(c));
        }
        assert!(app.connect.on_paste_row());

        app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
            name: "Living Room".into(),
            base_url: "http://192.168.1.71:3999".into(),
            version: None,
            quick_connect: true,
        }]));
        assert!(app.connect.on_paste_row(), "still aimed at the code the user pasted");

        // …and Enter still dials the code rather than the newly-found server.
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
        );
    }

    #[test]
    fn a_late_needs_login_cannot_unseat_a_live_session() {
        // Found live: a second connection attempt answered after the tunnel
        // had already connected, and dragged the connected UI to a login form
        // for a server the user had abandoned.
        let mut app = connected_app();
        app.apply_event(Event::NeedsLogin { server: "http://192.168.1.71:3999".into() });
        assert!(app.connected, "the live session survives");
        assert_eq!(app.server, "http://host:3000", "and stays on its own server");
    }

    #[test]
    fn typing_a_code_jumps_past_the_discovered_servers() {
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
            name: "Living Room".into(),
            base_url: "http://host:3999".into(),
            version: None,
            quick_connect: true,
        }]));
        assert_eq!(app.connect.row, 0, "starts on the first server");

        app.handle_action(Action::Input('m'));
        assert!(app.connect.on_paste_row(), "typing means the user has a code");

        // …and Enter now dials rather than connecting to the highlighted server.
        for c in "str1:abc".chars() {
            app.handle_action(Action::Input(c));
        }
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
        );
    }

    #[test]
    fn the_selection_cannot_run_past_the_paste_row() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        for _ in 0..5 {
            app.handle_action(Action::Down);
        }
        assert_eq!(app.connect.row, app.connect.paste_row());
    }

    #[test]
    fn pasting_a_pairing_code_dials_the_tunnel() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        for c in "mstr1:abc".chars() {
            app.handle_action(Action::Input(c));
        }
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::QuickConnect { code: "mstr1:abc".into(), token: None })]
        );
        assert!(app.connecting);
    }

    #[test]
    fn a_tunnel_session_is_remembered_by_identity_not_by_its_loopback_port() {
        // The bug this pins: the loopback bridge got saved as the server, so
        // the next run dialled a port that no longer existed, and the token
        // was filed under a URL that could never match again.
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        for c in "mstr1:abc".chars() {
            app.handle_action(Action::Input(c));
        }
        app.handle_action(Action::Submit);

        app.apply_event(Event::TunnelReady {
            local_url: "http://127.0.0.1:51234".into(),
            id: "mstream+iroh://endpointabc".into(),
        });
        app.connect.username = "alice".into();
        app.connect.password = "pw".into();
        app.handle_action(Action::Submit);
        app.apply_event(Event::Connected {
            server: "http://127.0.0.1:51234".into(),
            id: "mstream+iroh://endpointabc".into(),
            username: Some("alice".into()),
            token: Some("tok".into()),
            ping: Box::new(Default::default()),
        });

        // What gets written down is the identity...
        assert_eq!(app.server_id, "mstream+iroh://endpointabc");
        // ...while requests and stream URLs still go over the bridge.
        assert_eq!(app.server, "http://127.0.0.1:51234");
        assert_eq!(app.tunnel_code.as_deref(), Some("mstr1:abc"), "kept, or there's no way back");
    }

    #[test]
    fn a_public_tunnel_server_is_still_worth_saving() {
        // No login means no token and no username — but without the pairing
        // code stored, the server is unreachable next time.
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        for c in "mstr1:pub".chars() {
            app.handle_action(Action::Input(c));
        }
        app.handle_action(Action::Submit);

        let effects = app.apply_event(Event::Connected {
            server: "http://127.0.0.1:5000".into(),
            id: "mstream+iroh://pubserver".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        assert!(effects.contains(&Effect::SaveSession), "got {effects:?}");
    }

    #[test]
    fn reconnecting_to_a_tunnel_server_dials_its_code_again() {
        let app = App::new(Some("mstream+iroh://endpointabc".into()), Some("tok".into()), None);
        // The identity is not an address, so it must not reach the form or
        // the endpoint — only the dialler.
        assert!(app.server.is_empty(), "nothing can be requested from an identity");
        assert!(app.connect.server.is_empty(), "and it cannot be typed or edited");

        let mut app = app.with_tunnel(Some("mstr1:saved".into()));
        let effects = app.start();
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::QuickConnect {
                code: "mstr1:saved".into(),
                token: Some("tok".into()),
            })],
            "the saved token rides the re-dialled tunnel"
        );
        assert!(app.connecting);
    }

    #[test]
    fn a_remembered_tunnel_with_no_code_says_so_instead_of_hanging() {
        // Credentials deleted, or a config.toml copied to a new machine
        // without the file holding its secrets.
        let mut app = App::new(Some("mstream+iroh://endpointabc".into()), None, None);
        assert!(app.start().is_empty(), "nothing to dial, so nothing is attempted");
        assert!(!app.connecting, "and it doesn't sit on a connecting screen forever");
        assert!(app.server_id.is_empty(), "the unreachable server is let go");
        let message = &app.message.as_ref().unwrap().text;
        assert!(message.contains("pairing code"), "got: {message}");
    }

    #[test]
    fn an_expired_tunnel_session_signs_back_in_over_the_open_bridge() {
        // The tunnel outlives the token: the bridge is still up in the worker,
        // so the login form must aim at it rather than at an identity no HTTP
        // client can dial.
        let mut app = App::new(None, None, None);
        app.connected = true;
        app.server = "http://127.0.0.1:51234".into();
        app.server_id = "mstream+iroh://endpointabc".into();
        app.token = Some("stale".into());

        app.apply_event(Event::Unauthorized);
        assert_eq!(app.connect.stage, ConnectStage::Direct);
        assert_eq!(app.connect.server, "http://127.0.0.1:51234", "aims at the live bridge");

        app.connect.username = "alice".into();
        app.connect.password = "pw".into();
        let effects = app.handle_action(Action::Submit);
        // Loopback, so no plaintext warning stands between the user and a
        // re-login they didn't ask for in the first place.
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Login {
                server: "http://127.0.0.1:51234".into(),
                username: "alice".into(),
                password: "pw".into(),
            })]
        );
    }

    #[test]
    fn a_tunnel_is_shown_by_name_rather_than_by_port() {
        let mut app = App::new(None, None, None);
        app.server = "http://127.0.0.1:51234".into();
        app.server_id = "mstream+iroh://endpointabcdef123456".into();
        let shown = app.server_display();
        assert!(shown.starts_with("quick connect"), "got: {shown}");
        assert!(!shown.contains("127.0.0.1"), "the port is an implementation detail");

        // A direct server is shown as itself.
        let mut app = App::new(None, None, None);
        app.server = "https://demo.mstream.io".into();
        app.server_id = "https://demo.mstream.io".into();
        assert_eq!(app.server_display(), "https://demo.mstream.io");
    }

    #[test]
    fn an_empty_pairing_code_is_refused_without_a_request() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        assert!(app.handle_action(Action::Submit).is_empty());
        assert!(!app.connecting);
    }

    #[test]
    fn an_open_tunnel_leads_to_the_login_form() {
        // The secret gates the pipe, not the API — so the tunnel coming up
        // means "now sign in", not "you're in".
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.connecting = true;

        app.apply_event(Event::TunnelReady {
            local_url: "http://127.0.0.1:51234".into(),
            id: "mstream+iroh://abc123".into(),
        });
        assert_eq!(app.connect.stage, ConnectStage::Direct);
        assert_eq!(app.connect.server, "http://127.0.0.1:51234");
        assert_eq!(app.connect.field, 1, "focus lands on the username");
        assert!(!app.connecting);
        assert_eq!(app.server_id, "mstream+iroh://abc123", "already filed under its identity");
    }

    #[test]
    fn a_picked_server_that_wants_credentials_opens_its_login_form() {
        // Regression: choosing a server found on the network bounced back to
        // "how do you want to connect?", losing the server that was picked —
        // the connect path was reporting "needs a sign-in" as an
        // authorization failure.
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.apply_event(Event::ServersDiscovered(vec![DiscoveredServer {
            name: "Living Room".into(),
            base_url: "http://192.168.1.71:3999".into(),
            version: None,
            quick_connect: true,
        }]));
        app.handle_action(Action::Submit);

        app.apply_event(Event::NeedsLogin { server: "http://192.168.1.71:3999".into() });
        assert_eq!(app.connect.stage, ConnectStage::Direct, "lands on the login form");
        assert_eq!(
            app.connect.server, "http://192.168.1.71:3999",
            "the chosen server is kept, not blanked"
        );
        assert_eq!(app.connect.field, 1, "focus is on the username");
        assert!(!app.connecting);
    }

    #[test]
    fn an_expired_session_offers_a_login_for_the_same_server() {
        let mut app = connected_app();
        app.apply_event(Event::Unauthorized);
        assert!(!app.connected);
        assert_eq!(app.connect.stage, ConnectStage::Direct);
        assert_eq!(app.connect.server, "http://host:3000", "stays on the server in use");
        assert!(app.token.is_none());
    }

    #[test]
    fn connecting_without_a_username_uses_public_mode() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.connect.server = "http://host:3000".into();
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Connect { server: "http://host:3000".into(), token: None })]
        );
    }

    #[test]
    fn the_server_field_starts_empty_for_a_new_user() {
        // It used to be prefilled with a guess at localhost, so the first
        // thing anyone had to do was delete two dozen characters.
        let app = App::new(None, None, None);
        assert!(app.connect.server.is_empty());

        // A server we actually know about is still offered.
        let app = App::new(Some("http://host:3000".into()), None, None);
        assert_eq!(app.connect.server, "http://host:3000");
    }

    #[test]
    fn connect_form_edits_the_focused_field_only() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.handle_action(Action::Input('h'));
        app.handle_action(Action::CycleFocus);
        app.handle_action(Action::Input('u'));
        assert_eq!(app.connect.server, "h");
        assert_eq!(app.connect.username, "u");
        assert!(app.connect.password.is_empty());
    }

    #[test]
    fn login_effect_carries_credentials_and_clears_the_password() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.connect.server = "http://host:3000".into();
        app.connect.username = "alice".into();
        app.connect.password = "secret".into();

        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Login {
                server: "http://host:3000".into(),
                username: "alice".into(),
                password: "secret".into(),
            })]
        );
        assert!(app.connect.password.is_empty(), "password is not kept in memory after use");
    }

    /// A connect screen sitting on `Direct` with the given server text.
    fn at_direct(server: &str) -> App {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.connect.server = server.into();
        app
    }

    #[test]
    fn a_typed_address_is_completed_before_it_is_used() {
        // What used to happen here: "relative URL without a base", after a
        // round trip, with the typed text still on screen.
        let mut app = at_direct("nas:3000");
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Connect { server: "http://nas:3000".into(), token: None })]
        );
        assert_eq!(app.connect.server, "http://nas:3000", "the field shows what was assumed");

        let mut app = at_direct("music.example.com");
        app.handle_action(Action::Submit);
        assert_eq!(app.connect.server, "https://music.example.com");
    }

    #[test]
    fn an_unusable_address_is_refused_without_a_round_trip() {
        let mut app = at_direct("ftp://host");
        assert!(app.handle_action(Action::Submit).is_empty(), "nothing is dispatched");
        assert!(!app.connecting, "and the screen doesn't pretend to be busy");
        let message = app.message.as_ref().unwrap();
        assert_eq!(message.kind, MessageKind::Error);
        assert!(message.text.contains("http://"), "it says what to type instead");

        let mut app = at_direct("   ");
        assert!(app.handle_action(Action::Submit).is_empty());
        assert!(app.message.as_ref().unwrap().text.contains("enter a server address"));
    }

    #[test]
    fn a_username_without_a_password_is_caught_here_not_by_the_server() {
        let mut app = at_direct("http://host:3000");
        app.connect.username = "alice".into();
        assert!(app.handle_action(Action::Submit).is_empty());
        let text = &app.message.as_ref().unwrap().text;
        assert!(text.contains("password"), "got: {text}");
        // The way out is spelled out, since public mode is a real mode.
        assert!(text.contains("public"), "got: {text}");
    }

    #[test]
    fn sending_a_password_over_plain_http_asks_first() {
        let mut app = at_direct("http://music.example.com");
        app.connect.username = "alice".into();
        app.connect.password = "secret".into();

        // First Enter: warned, nothing sent, password still typed.
        assert!(app.handle_action(Action::Submit).is_empty());
        assert!(app.message.as_ref().unwrap().text.contains("unencrypted"));
        assert_eq!(app.connect.password, "secret", "so the answer can just be yes");

        // Second Enter: taken as consent.
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Login {
                server: "http://music.example.com".into(),
                username: "alice".into(),
                password: "secret".into(),
            })]
        );
    }

    #[test]
    fn consent_to_plaintext_does_not_follow_you_to_another_server() {
        let mut app = at_direct("http://music.example.com");
        app.connect.username = "alice".into();
        app.connect.password = "secret".into();
        app.handle_action(Action::Submit); // warned

        app.connect.server = "http://other.example.com".into();
        assert!(app.handle_action(Action::Submit).is_empty(), "the new host warns on its own");
        assert!(app.message.as_ref().unwrap().text.contains("other.example.com"));
    }

    #[test]
    fn signing_in_to_a_lan_server_is_not_interrupted() {
        // http on the LAN is how mStream is normally run: a warning every
        // time would be noise, and noise is what gets clicked through.
        for server in ["http://192.168.1.71:3999", "nas:3000", "http://localhost:3000"] {
            let mut app = at_direct(server);
            app.connect.username = "alice".into();
            app.connect.password = "secret".into();
            let effects = app.handle_action(Action::Submit);
            assert!(
                matches!(effects.as_slice(), [Effect::Api(ApiCmd::Login { .. })]),
                "{server} should sign in without ceremony, got {effects:?}"
            );
        }
    }

    #[test]
    fn a_public_server_over_plain_http_needs_no_warning() {
        // Nothing secret is being sent, so there is nothing to warn about.
        let mut app = at_direct("http://music.example.com");
        let effects = app.handle_action(Action::Submit);
        assert!(matches!(effects.as_slice(), [Effect::Api(ApiCmd::Connect { .. })]));
    }

    #[test]
    fn quitting_shuts_both_workers_down() {
        let mut app = connected_app();
        let effects = app.handle_action(Action::Quit);
        assert!(app.should_quit);
        assert!(effects.contains(&Effect::Audio(AudioCmd::Shutdown)));
        assert!(effects.contains(&Effect::Api(ApiCmd::Shutdown)));
    }

    #[test]
    fn key_mapping_differs_between_normal_and_editing_modes() {
        let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);

        assert_eq!(map_key(key('j'), InputMode::Normal), Some(Action::Down));
        assert_eq!(map_key(key('j'), InputMode::Editing), Some(Action::Input('j')));
        assert_eq!(map_key(key('q'), InputMode::Normal), Some(Action::Quit));
        assert_eq!(map_key(key('q'), InputMode::Editing), Some(Action::Input('q')));

        // Ctrl+C always quits, even mid-typing.
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_c, InputMode::Editing), Some(Action::Quit));

        assert_eq!(map_key(key('2'), InputMode::Normal), Some(Action::SelectTab(1)));
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), InputMode::Normal),
            Some(Action::Activate)
        );
    }

    #[test]
    fn searching_from_the_query_box_emits_one_search() {
        let mut app = connected_app();
        app.handle_action(Action::StartSearch);
        assert_eq!(app.tab, Tab::Search);
        assert_eq!(app.input_mode(), InputMode::Editing);

        for c in "moon".chars() {
            app.handle_action(Action::Input(c));
        }
        let effects = app.handle_action(Action::Submit);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Search("moon".into()))]);
        assert!(!app.editing_query);
    }

    #[test]
    fn opening_the_playlists_tab_loads_them_once() {
        let mut app = connected_app();
        let effects = app.handle_action(Action::SelectTab(2));
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Playlists)]);

        app.apply_event(Event::Playlists(vec![crate::api::types::PlaylistSummary {
            name: "Roadtrip".into(),
        }]));
        // Already loaded: switching back doesn't refetch.
        app.handle_action(Action::SelectTab(0));
        assert!(app.handle_action(Action::SelectTab(2)).is_empty());
    }

    #[test]
    fn a_pane_knows_when_its_contents_are_still_on_the_wire() {
        let mut app = connected_app();
        assert!(!app.playlists.loading);

        app.handle_action(Action::SelectTab(2));
        assert!(app.playlists.loading, "asking marks the pane, not the call site");
        // Only the pane that was asked for — leaving the tab mid-flight must
        // not leave a spinner turning somewhere it was never requested.
        assert!(!app.files.loading && !app.search.loading);

        app.apply_event(Event::Playlists(Vec::new()));
        assert!(!app.playlists.loading, "the reply lands through Pane::set");
    }

    #[test]
    fn a_request_that_fails_is_no_longer_pending() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        // Nothing calls `Pane::set` on the way out of an error, so this is the
        // one path that would otherwise spin forever.
        app.apply_event(Event::Error("nope".into()));
        assert!(!app.playlists.loading);
    }

    #[test]
    fn playlist_tracks_open_and_close() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        app.apply_event(Event::Playlists(vec![crate::api::types::PlaylistSummary {
            name: "Roadtrip".into(),
        }]));

        let effects = app.handle_action(Action::Activate);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::LoadPlaylist("Roadtrip".into()))]);

        app.apply_event(Event::PlaylistTracks {
            name: "Roadtrip".into(),
            tracks: vec![track("lib/a.mp3")],
        });
        assert_eq!(app.playlists.entries.len(), 2); // ".." + one track
        assert_eq!(app.playlist_open.as_deref(), Some("Roadtrip"));

        let effects = app.handle_action(Action::Back);
        assert!(effects.is_empty(), "the playlist list came back off the trail");
        assert!(app.playlist_open.is_none());
        assert_eq!(app.playlists.entries.len(), 1, "the one playlist, restored");
    }

    #[test]
    fn library_tab_opens_on_a_static_menu_without_a_request() {
        let mut app = connected_app();
        let effects = app.handle_action(Action::SelectTab(1));
        assert!(effects.is_empty(), "the mode menu costs no round-trip");
        assert_eq!(app.library.entries.len(), 4);
        assert_eq!(app.library_node(), &LibraryNode::Root);

        let labels: Vec<&str> = app
            .library
            .entries
            .iter()
            .map(|e| match e {
                Entry::Node { label, .. } => label.as_str(),
                _ => "?",
            })
            .collect();
        assert_eq!(labels, ["Artists", "Albums", "Genres", "Recently Added"]);
    }

    #[test]
    fn drilling_from_artists_to_an_album_of_tracks() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));

        // Artists
        let effects = app.handle_action(Action::Activate);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Library(LibraryNode::Artists))]);
        app.apply_event(Event::Library {
            node: LibraryNode::Artists,
            data: LibraryData::Artists(vec!["Signal Chain".into(), "Terminal Test".into()]),
        });
        assert_eq!(app.library.entries.len(), 3); // ".." + two artists

        // One artist's albums
        let effects = app.handle_action(Action::Activate);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Library(LibraryNode::Artist("Signal Chain".into())))]
        );
        app.apply_event(Event::Library {
            node: LibraryNode::Artist("Signal Chain".into()),
            data: LibraryData::Albums(vec![Album {
                name: Some("Second Album".into()),
                artist: Some("Signal Chain".into()),
                year: Some(2025),
                album_art_file: None,
            }]),
        });

        // That album's tracks
        let effects = app.handle_action(Action::Activate);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Library(LibraryNode::Album {
                name: "Second Album".into(),
                artist: Some("Signal Chain".into()),
            }))]
        );
        app.apply_event(Event::Library {
            node: LibraryNode::Album {
                name: "Second Album".into(),
                artist: Some("Signal Chain".into()),
            },
            data: LibraryData::Tracks(vec![track("testlib/a.mp3"), track("testlib/b.mp3")]),
        });

        // Playing from here queues the album and starts at the selected track.
        let effects = app.handle_action(Action::Activate);
        assert_eq!(app.queue.items.len(), 2);
        assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));
    }

    #[test]
    fn back_walks_the_library_stack_to_the_menu() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));
        app.handle_action(Action::Activate); // → Artists
        app.apply_event(Event::Library {
            node: LibraryNode::Artists,
            data: LibraryData::Artists(vec!["Solo".into()]),
        });
        app.handle_action(Action::Activate); // → Artist("Solo")

        let effects = app.handle_action(Action::Back);
        assert!(effects.is_empty(), "the artist list came back off the trail");
        assert_eq!(app.library_node(), &LibraryNode::Artists);

        let effects = app.handle_action(Action::Back);
        assert!(effects.is_empty(), "returning to the static menu needs no request");
        assert_eq!(app.library_node(), &LibraryNode::Root);
        assert_eq!(app.library.entries.len(), 4);

        // Already at the top.
        assert!(app.handle_action(Action::Back).is_empty());
    }

    #[test]
    fn a_reply_for_an_abandoned_view_is_discarded() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));
        app.handle_action(Action::Activate); // asked for Artists
        app.handle_action(Action::Back); // …then changed our mind

        app.apply_event(Event::Library {
            node: LibraryNode::Artists,
            data: LibraryData::Artists(vec!["Ghost".into()]),
        });
        assert_eq!(app.library_node(), &LibraryNode::Root);
        assert_eq!(app.library.entries.len(), 4, "the menu is untouched by the late reply");
    }

    #[test]
    fn genres_show_track_counts_and_lead_to_songs() {
        let mut app = connected_app();
        app.tab = Tab::Library;
        app.library_stack = vec![LibraryNode::Root, LibraryNode::Genres];
        app.apply_event(Event::Library {
            node: LibraryNode::Genres,
            data: LibraryData::Genres(vec![
                Genre { name: "Ambient".into(), track_count: Some(2) },
                Genre { name: "Electronic".into(), track_count: None },
            ]),
        });

        assert_eq!(
            app.library.entries[1],
            Entry::Node { label: "Ambient (2)".into(), node: LibraryNode::Genre("Ambient".into()) }
        );
        assert_eq!(
            app.library.entries[2],
            Entry::Node { label: "Electronic".into(), node: LibraryNode::Genre("Electronic".into()) }
        );

        let effects = app.handle_action(Action::Activate);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Library(LibraryNode::Genre("Ambient".into())))]
        );
    }

    #[test]
    fn albums_without_an_artist_still_resolve() {
        // The all-albums endpoint omits the artist field; the album name alone
        // has to be enough to fetch tracks.
        let mut app = connected_app();
        app.library_stack = vec![LibraryNode::Root, LibraryNode::Albums];
        app.apply_event(Event::Library {
            node: LibraryNode::Albums,
            data: LibraryData::Albums(vec![Album {
                name: Some("Phase Three".into()),
                artist: None,
                year: Some(2026),
                album_art_file: None,
            }]),
        });

        assert_eq!(
            app.library.entries[1],
            Entry::Node {
                label: "Phase Three (2026)".into(),
                node: LibraryNode::Album { name: "Phase Three".into(), artist: None },
            }
        );
    }

    #[test]
    fn recently_added_lists_tracks_directly() {
        let mut app = connected_app();
        app.library_stack = vec![LibraryNode::Root, LibraryNode::Recent];
        app.apply_event(Event::Library {
            node: LibraryNode::Recent,
            data: LibraryData::Tracks(vec![track("testlib/new.mp3")]),
        });
        assert_eq!(app.library.entries.len(), 2); // ".." + the track
        assert!(matches!(app.library.entries[1], Entry::Track { .. }));
    }

    fn autodj_effect(effects: &[Effect]) -> Option<&ApiCmd> {
        effects.iter().find_map(|e| match e {
            Effect::Api(cmd @ ApiCmd::AutoDj { .. }) => Some(cmd),
            _ => None,
        })
    }

    #[test]
    fn remembered_preferences_are_applied_and_handed_back() {
        let saved = crate::config::PlayerPrefs {
            volume: 0.35,
            repeat: "all".into(),
            shuffle: true,
            autodj: "tempo+key".into(),
            dj: Default::default(),
        };
        let app = App::new(None, None, None).with_prefs(&saved);
        assert_eq!(app.volume, 0.35);
        assert_eq!(app.queue.repeat, Repeat::All);
        assert!(app.queue.shuffle);
        assert_eq!(app.autodj, AutoDjMode::BpmKey);

        // What goes out matches what came in, so a restart is a no-op.
        assert_eq!(app.prefs(), saved);
    }

    #[test]
    fn nonsense_preferences_fall_back_rather_than_refusing_to_start() {
        // A hand-edited config shouldn't be able to brick the player.
        let saved = crate::config::PlayerPrefs {
            volume: 9.0,
            repeat: "sideways".into(),
            shuffle: false,
            autodj: "disco".into(),
            dj: Default::default(),
        };
        let app = App::new(None, None, None).with_prefs(&saved);
        assert_eq!(app.volume, 1.0, "volume is clamped");
        assert_eq!(app.queue.repeat, Repeat::Off);
        assert_eq!(app.autodj, AutoDjMode::Off);
    }

    #[test]
    fn autodj_cycles_through_its_modes() {
        let mut app = connected_app();
        assert_eq!(app.autodj, AutoDjMode::Off);
        app.handle_action(Action::ToggleAutoDj);
        assert_eq!(app.autodj, AutoDjMode::Similar);
        app.handle_action(Action::ToggleAutoDj);
        assert_eq!(app.autodj, AutoDjMode::BpmKey);
        app.handle_action(Action::ToggleAutoDj);
        assert_eq!(app.autodj, AutoDjMode::Off);
    }

    /// A browser pane holding tracks, with one highlighted.
    fn browsing(app: &mut App, names: &[&str], selected: usize) {
        let entries: Vec<Entry> = names
            .iter()
            .map(|n| Entry::Track { label: (*n).to_string(), track: Box::new(track(n)) })
            .collect();
        app.files.set(entries);
        app.files.state.select(Some(selected));
    }

    fn stop(path: &str, t: f64) -> crate::api::types::JourneyStop {
        crate::api::types::JourneyStop {
            filepath: path.into(),
            t,
            similarity: 0.9,
            ..Default::default()
        }
    }

    fn similar_artist(name: &str, similarity: f64, ways: &[&str]) -> crate::api::types::SimilarArtist {
        crate::api::types::SimilarArtist {
            artist: name.into(),
            similarity,
            analyzed_count: 12,
            genre_tags: vec!["ambient".into()],
            entry_points: ways.iter().map(|p| track(p)).collect(),
        }
    }

    /// Index of the Discover tab among the visible ones.
    fn discover_tab(app: &App) -> usize {
        app.tabs().iter().position(|t| *t == Tab::Discover).expect("discover is available")
    }

    #[test]
    fn hierarchical_model_tags_are_shown_by_their_leaf() {
        // Live against a real index every row read "Electronic---Dubstep,
        // Electroni…" — the shared prefix filled the column and said nothing.
        assert_eq!(tidy_tag("Electronic---Dubstep"), "Dubstep");
        assert_eq!(tidy_tag("Hip Hop---Trap"), "Trap");
        assert_eq!(tidy_tag("Jazz"), "Jazz");
        assert_eq!(tidy_tag(""), "");
    }

    #[test]
    fn the_discover_tab_is_absent_where_the_server_cannot_serve_it() {
        let app = connected_app();
        assert!(app.tabs().contains(&Tab::Discover));

        let mut plain = connected_app();
        plain.capabilities = Default::default();
        assert!(!plain.tabs().contains(&Tab::Discover));
        // And the numbers stay 1..n so no key points at a gap.
        assert_eq!(plain.tabs().len(), 4);
        assert!(plain.handle_action(Action::SelectTab(4)).is_empty(), "there is no fifth tab");
        assert_ne!(plain.tab, Tab::Discover);
    }

    #[test]
    fn opening_discover_anchors_on_what_is_highlighted() {
        // The cursor was on a track in another tab, and that is the obvious
        // thing to mean by "more like this".
        let mut app = connected_app();
        browsing(&mut app, &["a", "b"], 1);
        app.handle_action(Action::SelectTab(discover_tab(&app)));

        assert_eq!(app.tab, Tab::Discover);
        assert_eq!(app.discover_seed.as_ref().unwrap().filepath, "b");
        // The mode menu costs no request.
        assert_eq!(app.discover.entries.len(), 2);
    }

    #[test]
    fn what_is_playing_wins_over_what_is_merely_highlighted() {
        let mut app = connected_app();
        app.queue.replace(vec![track("playing")]);
        app.play_index(0);
        browsing(&mut app, &["highlighted"], 0);
        app.handle_action(Action::SelectTab(discover_tab(&app)));
        assert_eq!(app.discover_seed.as_ref().unwrap().filepath, "playing");
    }

    #[test]
    fn similar_tracks_are_ordinary_playable_rows() {
        let mut app = connected_app();
        browsing(&mut app, &["seed"], 0);
        app.handle_action(Action::SelectTab(discover_tab(&app)));

        let effects = app.handle_action(Action::Activate);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Api(ApiCmd::Discover { node: DiscoverNode::Tracks, seed })]
                if seed.filepath == "seed"
        ));

        app.apply_event(Event::Discover {
            node: DiscoverNode::Tracks,
            data: DiscoverData::Tracks(vec![track("near-one"), track("near-two")]),
            note: None,
        });
        // Parent row plus the two neighbours, and Enter plays them like any
        // other list — no new concept to learn.
        assert_eq!(app.discover.entries.len(), 3);
        app.discover.state.select(Some(1));
        let effects = app.handle_action(Action::Activate);
        assert_eq!(app.queue.items.len(), 2);
        assert_eq!(app.queue.current, Some(0));
        assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
    }

    #[test]
    fn an_artist_drills_into_its_ways_in_without_asking_again() {
        // The entry points arrived with the artist list, so going in costs
        // nothing — the whole reason the server sends them inline.
        let mut app = connected_app();
        browsing(&mut app, &["seed"], 0);
        app.handle_action(Action::SelectTab(discover_tab(&app)));
        app.discover.state.select(Some(1)); // "Similar artists"
        app.handle_action(Action::Activate);

        app.apply_event(Event::Discover {
            node: DiscoverNode::Artists,
            data: DiscoverData::Artists(vec![
                similar_artist("Near Artist", 0.91, &["in-one", "in-two"]),
                similar_artist("Other", 0.80, &[]),
            ]),
            note: None,
        });
        assert_eq!(app.discover.entries.len(), 3, "parent plus two artists");

        app.discover.state.select(Some(1));
        let effects = app.handle_action(Action::Activate);
        assert!(effects.is_empty(), "no request — the doorways were already here");
        assert_eq!(*app.discover_node(), DiscoverNode::Artist("Near Artist".into()));
        assert_eq!(app.discover.entries.len(), 3, "parent plus two ways in");

        // And back out again, still without asking.
        assert!(app.handle_action(Action::Back).is_empty());
        assert_eq!(*app.discover_node(), DiscoverNode::Artists);
        assert_eq!(app.discover.entries.len(), 3);
    }

    #[test]
    fn a_discover_reply_for_a_view_already_left_is_dropped() {
        let mut app = connected_app();
        browsing(&mut app, &["seed"], 0);
        app.handle_action(Action::SelectTab(discover_tab(&app)));
        app.handle_action(Action::Activate); // into Tracks
        app.handle_action(Action::Back); // and straight back out

        app.apply_event(Event::Discover {
            node: DiscoverNode::Tracks,
            data: DiscoverData::Tracks(vec![track("late")]),
            note: None,
        });
        assert_eq!(*app.discover_node(), DiscoverNode::Root);
        assert_eq!(app.discover.entries.len(), 2, "still the mode menu");
    }

    #[test]
    fn discovery_with_nothing_to_go_on_says_so() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(discover_tab(&app)));
        assert!(app.discover_seed.is_none(), "nothing playing, nothing highlighted");

        assert!(app.handle_action(Action::Activate).is_empty());
        assert!(app.message.as_ref().unwrap().text.contains("play or highlight a track"));
    }

    #[test]
    fn the_browser_opens_where_the_server_thinks_best() {
        // With one library the old opening screen was a list of exactly one
        // row, which everyone stepped through to reach any music.
        let mut app = App::new(Some("http://host:3000".into()), Some("t".into()), None);
        let effects = app.apply_event(Event::Connected {
            server: "http://host:3000".into(),
            id: "http://host:3000".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        assert!(
            effects.contains(&Effect::Api(ApiCmd::Browse(crate::api::BEST_START.into()))),
            "got {effects:?}"
        );

        // A remembered path still wins — being put back where you were beats
        // being taken somewhere tidy.
        let mut app = App::new(Some("http://host:3000".into()), Some("t".into()), None);
        app.path = "library/Artist".into();
        let effects = app.apply_event(Event::Connected {
            server: "http://host:3000".into(),
            id: "http://host:3000".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        assert!(
            effects.contains(&Effect::Api(ApiCmd::Browse("library/Artist".into()))),
            "got {effects:?}"
        );
    }

    #[test]
    fn going_up_from_a_library_still_asks_for_the_list() {
        // `~` and `""` are different questions: the second is the only way to
        // see the other libraries, so navigating up must keep using it.
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &[]))));
        assert_eq!(app.path, "lib");
        let effects = app.handle_action(Action::Back);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse(String::new()))]);
    }

    #[test]
    fn a_listing_carries_the_tags_the_server_sent_with_it() {
        let listing = DirListing {
            path: "/library/Artist/Album".into(),
            directories: Vec::new(),
            files: vec![
                FileEntry {
                    name: "01 - Song.mp3".into(),
                    kind: Some("mp3".into()),
                    metadata: Some(FileMetadata {
                        filepath: "library/Artist/Album/01 - Song.mp3".into(),
                        metadata: Some(TrackMetadata {
                            title: Some("Song".into()),
                            artist: Some("Artist".into()),
                            duration: Some(238.655),
                            bpm: Some(130),
                            ..Default::default()
                        }),
                    }),
                },
                // On disk but not in the database — scanned since, or never.
                // The row still has to work, just without the extras.
                FileEntry {
                    name: "02 - Unscanned.mp3".into(),
                    kind: Some("mp3".into()),
                    metadata: Some(FileMetadata {
                        filepath: "library/Artist/Album/02 - Unscanned.mp3".into(),
                        metadata: None,
                    }),
                },
            ],
        };

        let tracks: Vec<Track> = entries_from_listing(&listing, "")
            .into_iter()
            .filter_map(|e| match e {
                Entry::Track { track, .. } => Some(*track),
                _ => None,
            })
            .collect();

        assert_eq!(tracks[0].metadata.duration, Some(238.655));
        assert_eq!(tracks[0].metadata.bpm, Some(130));
        assert_eq!(tracks[0].display_name(), "Artist - Song");
        assert_eq!(tracks[0].filepath, "library/Artist/Album/01 - Song.mp3");

        assert_eq!(tracks[1].metadata, TrackMetadata::default());
        assert_eq!(tracks[1].display_name(), "02 - Unscanned.mp3", "falls back to the name");
    }

    #[test]
    fn a_listing_without_metadata_still_builds_its_own_paths() {
        // What an older server answers, and what the fallback request gets:
        // no `metadata` key at all. The path has to come from the folder plus
        // the filename, exactly as it did before.
        let listing = DirListing {
            path: "/library/Artist".into(),
            directories: Vec::new(),
            files: vec![FileEntry {
                name: "loose.mp3".into(),
                kind: Some("mp3".into()),
                metadata: None,
            }],
        };
        match &entries_from_listing(&listing, "")[1] {
            Entry::Track { track, .. } => {
                assert_eq!(track.filepath, "library/Artist/loose.mp3");
                assert_eq!(track.metadata, TrackMetadata::default());
            }
            other => panic!("expected a track, got {other:?}"),
        }
    }

    #[test]
    fn a_playlist_file_is_not_offered_as_a_track() {
        // Found live: an album folder held `001-2pac-greatest_hits.m3u`, and
        // Enter queues everything on screen, so the playlist file went into
        // the queue and the decoder rejected it — stopping the album on the
        // first row. mStream's own Auto-DJ excludes these for this reason.
        let listing = DirListing {
            path: "/library/2Pac/Greatest Hits".into(),
            directories: Vec::new(),
            files: vec![
                FileEntry {
                    name: "001-2pac-greatest_hits.m3u".into(),
                    kind: Some("m3u".into()),
                    ..Default::default()
                },
                FileEntry {
                    name: "201 - Keep Ya Head Up.mp3".into(),
                    kind: Some("mp3".into()),
                    ..Default::default()
                },
            ],
        };
        let entries = entries_from_listing(&listing, "");
        let labels: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Track { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, vec!["201 - Keep Ya Head Up.mp3"]);

        // A format this player can't decode is still audio, and still listed:
        // it should fail loudly when played, not disappear from the library.
        assert!(is_audio(Some("opus")));
        assert!(is_audio(Some("mp3")));
        assert!(is_audio(None), "an unclassified file is given the benefit of the doubt");
        for playlist in ["m3u", "M3U", "m3u8", "pls", "cue"] {
            assert!(!is_audio(Some(playlist)), "{playlist} is a list, not a track");
        }
    }

    #[test]
    fn a_track_that_will_not_play_is_skipped_rather_than_stopping_everything() {
        // The queue used to stop dead on the first unplayable file: the
        // message appeared and nothing moved. One bad file should cost one
        // track, not the session.
        let mut app = connected_app();
        app.queue.replace(vec![track("broken"), track("fine"), track("also-fine")]);
        app.play_index(0);

        let effects = app.apply_event(Event::PlaybackFailed("unrecognised format".into()));
        assert_eq!(app.queue.current, Some(1), "moved on to the next track");
        assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
        let message = &app.message.as_ref().unwrap().text;
        assert!(message.contains("skipping"), "got: {message}");
        assert!(message.contains("broken"), "and names the track: {message}");
    }

    #[test]
    fn a_queue_where_nothing_plays_gives_up_instead_of_looping() {
        // With repeat on, skipping past every broken track would go round
        // forever, hammering the server and never making a sound.
        let mut app = connected_app();
        app.queue.repeat = Repeat::All;
        app.queue.replace(vec![track("a"), track("b")]);
        app.play_index(0);

        app.apply_event(Event::PlaybackFailed("nope".into()));
        let effects = app.apply_event(Event::PlaybackFailed("nope".into()));
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Stop)], "it stops rather than wrapping");
        assert_eq!(app.queue.current, None);
        assert!(app.message.as_ref().unwrap().text.contains("nor could the rest"));
    }

    #[test]
    fn one_track_playing_forgives_the_failures_before_it() {
        // The give-up counter is about a *run* of failures. A queue with one
        // bad file every few tracks must keep going indefinitely.
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b")]);
        app.play_index(0);
        let effects = app.apply_event(Event::PlaybackFailed("nope".into()));
        assert_eq!(app.failures, 1);

        let url = played_url(&effects);
        app.apply_event(Event::Status(PlayerStatus { source: url, ..Default::default() }));
        assert_eq!(app.failures, 0, "a track that loaded clears the run");
    }

    #[test]
    fn a_journey_sets_off_from_what_is_playing() {
        // One keypress when something is already playing: that track is the
        // obvious place to leave from.
        let mut app = connected_app();
        app.queue.replace(vec![track("playing")]);
        app.play_index(0);
        browsing(&mut app, &["far-away"], 0);

        let effects = app.handle_action(Action::StartJourney);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Journey {
                start: "playing".into(),
                end: "far-away".into(),
                length: 14,
            })]
        );
        let journey = app.journey.as_ref().unwrap();
        assert!(journey.pending);
        assert_eq!(journey.from.filepath, "playing");
        assert_eq!(journey.to.filepath, "far-away");
    }

    #[test]
    fn with_nothing_playing_a_journey_takes_two_presses() {
        let mut app = connected_app();
        browsing(&mut app, &["a", "b"], 0);

        // First press marks where to set off from and says so.
        assert!(app.handle_action(Action::StartJourney).is_empty());
        assert!(app.journey.is_none(), "no request until there are two ends");
        assert_eq!(app.journey_from.as_ref().unwrap().filepath, "a");
        assert!(app.message.as_ref().unwrap().text.contains("press J"));

        // Second press, on a different track, plots it.
        app.files.state.select(Some(1));
        let effects = app.handle_action(Action::StartJourney);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Api(ApiCmd::Journey { start, end, .. })] if start == "a" && end == "b"
        ));
        assert!(app.journey_from.is_none(), "the pending start is consumed");
    }

    #[test]
    fn a_journey_needs_a_server_that_can_plot_one() {
        let mut app = connected_app();
        app.capabilities.discovery_path = false;
        browsing(&mut app, &["a"], 0);

        assert!(app.handle_action(Action::StartJourney).is_empty());
        assert!(app.journey.is_none());
        assert!(app.message.as_ref().unwrap().text.contains("can't plot journeys"));
    }

    #[test]
    fn a_journey_needs_a_track_not_a_folder() {
        let mut app = connected_app();
        app.files.set(vec![Entry::Dir {
            label: "an album".into(),
            path: "lib/an album".into(),
        }]);
        app.files.state.select(Some(0));

        assert!(app.handle_action(Action::StartJourney).is_empty());
        assert!(app.message.as_ref().unwrap().text.contains("highlight a track"));
    }

    #[test]
    fn changing_the_length_asks_for_a_different_arc() {
        // The stops aren't a list to trim — a shorter journey is a different
        // set of waypoints, so it has to be replotted.
        let mut app = connected_app();
        app.queue.replace(vec![track("from")]);
        app.play_index(0);
        browsing(&mut app, &["to"], 0);
        app.handle_action(Action::StartJourney);
        app.apply_event(Event::Journey {
            stops: vec![stop("from", 0.0), stop("mid", 0.5), stop("to", 1.0)],
            note: None,
        });
        assert!(!app.journey.as_ref().unwrap().pending);

        let effects = app.handle_action(Action::SeekForward);
        assert!(matches!(
            effects.as_slice(),
            [Effect::Api(ApiCmd::Journey { length: 16, .. })]
        ));
        assert!(app.journey.as_ref().unwrap().pending, "waiting on the new arc");

        // And it stops at the ends the server accepts rather than asking for
        // a length it would reject.
        for _ in 0..40 {
            app.handle_action(Action::Back);
        }
        assert_eq!(app.journey.as_ref().unwrap().length, 4);
        assert!(app.handle_action(Action::Back).is_empty(), "no request at the floor");
    }

    #[test]
    fn queueing_a_journey_replaces_the_queue_and_starts_it() {
        let mut app = connected_app();
        app.queue.replace(vec![track("old")]);
        app.play_index(0);
        browsing(&mut app, &["to"], 0);
        app.handle_action(Action::StartJourney);
        app.apply_event(Event::Journey {
            stops: vec![stop("from", 0.0), stop("mid", 0.5), stop("to", 1.0)],
            note: None,
        });

        let effects = app.handle_action(Action::Submit);
        assert!(app.journey.is_none(), "the panel closes once it's the queue");
        assert_eq!(
            app.queue.items.iter().map(|t| t.filepath.as_str()).collect::<Vec<_>>(),
            vec!["from", "mid", "to"],
            "the arc is the queue, in order"
        );
        assert_eq!(app.queue.current, Some(0));
        assert!(effects.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))));
    }

    #[test]
    fn a_journey_reply_that_arrives_after_it_is_closed_is_dropped() {
        let mut app = connected_app();
        app.queue.replace(vec![track("from")]);
        app.play_index(0);
        browsing(&mut app, &["to"], 0);
        app.handle_action(Action::StartJourney);
        app.handle_action(Action::Cancel);

        app.apply_event(Event::Journey { stops: vec![stop("late", 0.0)], note: None });
        assert!(app.journey.is_none(), "it stays closed");
        assert_eq!(app.queue.items.len(), 1, "and nothing is queued behind the user's back");
    }

    #[test]
    fn the_panel_only_offers_rows_the_server_can_honour() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
        let rows = &app.dj_panel.as_ref().unwrap().rows;
        assert!(rows.contains(&DjRow::Tightness), "this server has the index");
        assert!(rows.contains(&DjRow::Anchor));

        // Without it, a row promising a sonic pool would be a lie.
        let mut app = connected_app();
        app.capabilities = Default::default();
        app.handle_action(Action::OpenDjPanel);
        let rows = &app.dj_panel.as_ref().unwrap().rows;
        assert!(!rows.contains(&DjRow::Tightness));
        assert!(!rows.contains(&DjRow::Anchor));
        assert!(rows.contains(&DjRow::Tempo), "the rest is still there");
    }

    /// A `[keys]` section from a config file.
    fn keys(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(name, specs)| {
                ((*name).to_string(), specs.iter().map(|s| (*s).to_string()).collect())
            })
            .collect()
    }

    fn press(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn a_rebound_action_answers_to_its_new_key_and_not_the_old_one() {
        let (map, warnings) = Keymap::default()
            .with_overrides(&keys(&[("next-track", &["b"])]));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.action(press('b'), InputMode::Normal), Some(Action::NextTrack));
        // Naming an action replaces its keys rather than adding to them, so
        // the default is gone — that is what makes a binding movable.
        assert_eq!(map.action(press('n'), InputMode::Normal), None);
        // Everything unmentioned is untouched.
        assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
    }

    #[test]
    fn taking_a_key_from_another_action_just_works() {
        // Binding `d` to something new should not also require unbinding it
        // from remove-from-queue first.
        let (map, warnings) =
            Keymap::default().with_overrides(&keys(&[("next-track", &["d"])]));
        assert!(warnings.is_empty(), "no complaint about the old owner: {warnings:?}");
        assert_eq!(map.action(press('d'), InputMode::Normal), Some(Action::NextTrack));
        // And the action that lost it still works by its other keys, or not
        // at all if it had none.
        assert_eq!(map.action(press('C'), InputMode::Normal), Some(Action::ClearQueue));
    }

    #[test]
    fn an_action_can_be_unbound_entirely() {
        let (map, _) = Keymap::default().with_overrides(&keys(&[("clear-queue", &[])]));
        assert_eq!(map.action(press('C'), InputMode::Normal), None);
        // And it drops off the help, rather than listing a key that is gone.
        assert!(!map.help_rows().iter().any(|(_, what)| *what == "clear the queue"));
    }

    #[test]
    fn a_line_of_nothing_but_typos_leaves_the_binding_alone() {
        // An explicit `[]` means "unbind this". A line where every key was
        // unreadable means the line is wrong, and taking the key away would
        // punish a typo far harder than it deserves.
        let (map, warnings) =
            Keymap::default().with_overrides(&keys(&[("volume-up", &["not a key"])]));
        assert_eq!(map.action(press('+'), InputMode::Normal), Some(Action::VolumeUp));
        assert!(warnings.iter().any(|w| w.contains("left as it was")), "{warnings:?}");

        // But one good key among the bad still takes effect.
        let (map, _) = Keymap::default()
            .with_overrides(&keys(&[("volume-up", &["not a key", "V"])]));
        assert_eq!(map.action(press('V'), InputMode::Normal), Some(Action::VolumeUp));
        assert_eq!(map.action(press('+'), InputMode::Normal), None);
    }

    #[test]
    fn the_help_follows_the_bindings_that_are_actually_in_force() {
        let (map, _) = Keymap::default().with_overrides(&keys(&[("quit", &["ctrl+q", "Z"])]));
        let quit = map
            .help_rows()
            .into_iter()
            .find(|(_, what)| *what == "quit")
            .expect("quit is still listed");
        assert_eq!(quit.0, "^q Z", "the help shows the new keys, in order");
    }

    #[test]
    fn a_broken_keys_section_costs_only_the_broken_line() {
        let (map, warnings) = Keymap::default().with_overrides(&keys(&[
            ("next-track", &["b"]),
            ("teleport", &["t"]),
            ("previous-track", &["not a key"]),
        ]));
        assert!(warnings.iter().any(|w| w.contains("no action called 'teleport'")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("'not a key' is not a key")), "{warnings:?}");
        // The good line still took effect, and the player still works.
        assert_eq!(map.action(press('b'), InputMode::Normal), Some(Action::NextTrack));
        assert_eq!(map.action(press(' '), InputMode::Normal), Some(Action::PlayPause));
        // The line that was all typos kept its default rather than vanishing.
        assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
    }

    #[test]
    fn two_actions_claiming_one_key_is_reported() {
        let (map, warnings) = Keymap::default()
            .with_overrides(&keys(&[("next-track", &["z"]), ("previous-track", &["z"])]));
        assert!(warnings.iter().any(|w| w.contains("bound to both")), "{warnings:?}");
        // The first claim wins outright — stripping the key from each in turn
        // would leave it doing nothing, which is nobody's intent.
        assert_eq!(map.action(press('z'), InputMode::Normal), Some(Action::NextTrack));
        // And the one that lost keeps the binding it already had, rather than
        // being left with none at all.
        assert_eq!(map.action(press('p'), InputMode::Normal), Some(Action::PrevTrack));
    }

    #[test]
    fn ctrl_c_cannot_be_taken_away() {
        // The way out has to survive any config, including a hostile one.
        let (map, _) = Keymap::default().with_overrides(&keys(&[("quit", &["Z"])]));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map.action(ctrl_c, InputMode::Normal), Some(Action::Quit));
        assert_eq!(map.action(ctrl_c, InputMode::Panel), Some(Action::Quit));
        assert_eq!(map.action(ctrl_c, InputMode::Editing), Some(Action::Quit));
    }

    #[test]
    fn key_specs_round_trip_and_forgive_the_obvious_variants() {
        for spec in ["j", "G", "space", "enter", "esc", "tab", "up", "pagedown", "ctrl+d", "?"] {
            let parsed = Key::parse(spec).unwrap_or_else(|| panic!("could not read {spec:?}"));
            // What it writes back must read as the same key — case included,
            // since `G` and `g` are different bindings.
            assert_eq!(Key::parse(&parsed.spec()), Some(parsed), "{spec} did not round-trip");
        }
        assert_eq!(Key::parse("G").unwrap().spec(), "G", "a capital stays capital");
        // Same key, several spellings — this is a hand-edited file.
        for spec in ["ctrl+d", "Ctrl+D", "ctrl-d", "^d"] {
            assert_eq!(Key::parse(spec), Some(ctrl('d')), "{spec}");
        }
        assert_eq!(Key::parse("PgDn"), Some(key(KeyCode::PageDown)));
        assert_eq!(Key::parse("Escape"), Some(key(KeyCode::Esc)));
        // With Ctrl the capital is folded away: the terminal sends the
        // lowercase form, so `ctrl+D` would otherwise never fire.
        assert_eq!(Key::parse("ctrl+D"), Some(ctrl('d')));
        for bad in ["", "  ", "nonsense", "ctrl+"] {
            assert_eq!(Key::parse(bad), None, "{bad:?} is not a key");
        }
    }

    #[test]
    fn every_bindable_action_has_a_name_and_finds_its_way_back() {
        for binding in default_normal() {
            let name = binding
                .action
                .name()
                .unwrap_or_else(|| panic!("{:?} is bound but unnameable", binding.action));
            assert_eq!(
                Action::from_name(name),
                Some(binding.action.clone()),
                "'{name}' does not resolve back to what it names"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "'{name}' is not a kebab-case name"
            );
        }
    }

    #[test]
    fn the_dumped_config_is_the_config_that_would_be_read_back() {
        // `mstream-player keys` is only useful if pasting its output changes
        // nothing — which means the writer and the parser have to agree.
        let dumped = Keymap::default().to_config_toml();
        let parsed: crate::config::Config =
            toml::from_str(&format!("version = 1\n{dumped}")).expect("valid TOML");
        let (map, warnings) = Keymap::default().with_overrides(&parsed.keys);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.help_rows(), Keymap::default().help_rows(), "round trip changed something");
    }

    #[test]
    fn a_modifier_is_part_of_the_key_not_decoration() {
        // Ctrl was only ever checked for `c`, so Ctrl+D arrived as plain `d`
        // and quietly removed a queue entry when a vim user reached for
        // half-page-down.
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_d, InputMode::Normal), Some(Action::HalfPageDown));
        let plain_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert_eq!(map_key(plain_d, InputMode::Normal), Some(Action::RemoveFromQueue));

        // And an unbound Ctrl combination does nothing at all rather than
        // falling through to the bare letter.
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(map_key(ctrl_r, InputMode::Normal), None);
        // Including inside a panel, where bare letters are passed through.
        assert_eq!(map_key(ctrl_r, InputMode::Panel), None);
        let plain_p = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE);
        assert_eq!(map_key(plain_p, InputMode::Panel), Some(Action::Input('p')));
    }

    #[test]
    fn every_binding_is_reachable_and_unambiguous() {
        // Two rows claiming the same key means the second is dead code, and
        // which one wins depends on table order — worth catching here rather
        // than as "that key stopped working".
        for table in [default_normal(), default_panel()] {
            let mut seen = std::collections::HashMap::new();
            for binding in &table {
                for key in &binding.keys {
                    let previous = seen.insert(*key, binding.action.clone());
                    assert!(
                        previous.is_none(),
                        "{} is bound twice: {:?} and {:?}",
                        key.label(),
                        previous.unwrap(),
                        binding.action
                    );
                }
                assert!(!binding.keys.is_empty(), "{:?} has no key", binding.action);
            }
        }
    }

    #[test]
    fn jumping_to_what_is_playing_goes_to_the_queue() {
        // Browsing takes you a long way from the music — several tabs and two
        // drill-downs — so one key has to lead back.
        let mut app = connected_app();
        let named = Track {
            filepath: "b".into(),
            metadata: TrackMetadata {
                artist: Some("Band".into()),
                title: Some("Song".into()),
                ..Default::default()
            },
        };
        app.queue.replace(vec![track("a"), named, track("c")]);
        app.play_index(1);
        // Wander off.
        app.focus = Focus::Browser;
        app.queue.state.select(Some(0));

        assert!(app.handle_action(Action::JumpToPlaying).is_empty());
        assert_eq!(app.focus, Focus::Queue);
        assert_eq!(app.queue.state.selected(), Some(1), "the playing row, not the first");
        assert!(app.message.as_ref().unwrap().text.contains("Band - Song"));
    }

    #[test]
    fn jumping_with_nothing_playing_says_so() {
        let mut app = connected_app();
        app.queue.replace(vec![track("a")]);
        assert!(app.handle_action(Action::JumpToPlaying).is_empty());
        assert_eq!(app.focus, Focus::Browser, "the cursor stays where it was");
        assert!(app.message.as_ref().unwrap().text.contains("nothing is playing"));
    }

    #[test]
    fn the_shifted_seek_keys_move_by_a_minute() {
        let mut app = connected_app();
        app.status.source = "http://host/a.mp3".into();
        app.status.position = 200.0;

        let effects = app.handle_action(Action::SeekForwardFar);
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(260.0))]);
        let effects = app.handle_action(Action::SeekBackwardFar);
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(140.0))]);
        // And the fine keys still move by five.
        let effects = app.handle_action(Action::SeekForward);
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(205.0))]);
    }

    #[test]
    fn half_a_page_is_half_of_a_page() {
        let mut app = connected_app();
        let names: Vec<String> = (0..40).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        browsing(&mut app, &refs, 0);

        app.handle_action(Action::PageDown);
        let after_page = app.files.state.selected().unwrap();
        app.handle_action(Action::First);
        app.handle_action(Action::HalfPageDown);
        assert_eq!(app.files.state.selected(), Some(after_page / 2));
    }

    #[test]
    fn the_panel_binds_its_own_keys_rather_than_the_players() {
        // Found live: `p` reached the panel as "previous track", so the
        // sample key silently did nothing.
        let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(map_key(key('p'), InputMode::Normal), Some(Action::PrevTrack));
        assert_eq!(map_key(key('p'), InputMode::Panel), Some(Action::Input('p')));

        // Left/right come from three shapes, all meaning "adjust".
        for c in ['h', '['] {
            assert_eq!(map_key(key(c), InputMode::Panel), Some(Action::Back));
        }
        for c in ['l', ']'] {
            assert_eq!(map_key(key(c), InputMode::Panel), Some(Action::SeekForward));
        }
        // And the panel's own key closes it, as does Esc.
        assert_eq!(map_key(key('D'), InputMode::Panel), Some(Action::Cancel));
        assert_eq!(
            map_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), InputMode::Panel),
            Some(Action::Cancel)
        );
    }

    #[test]
    fn the_panel_owns_the_keyboard_while_it_is_open() {
        let mut app = connected_app();
        assert_eq!(app.input_mode(), InputMode::Normal);
        app.handle_action(Action::OpenDjPanel);
        assert_eq!(app.input_mode(), InputMode::Panel);
        app.handle_action(Action::Cancel);
        assert_eq!(app.input_mode(), InputMode::Normal);
    }

    #[test]
    fn the_panel_takes_the_keys_the_player_would_otherwise_use() {
        // Space is the toggle inside the genre chooser and pause outside it;
        // arrows move settings, not the browser. Leaking either would make
        // editing settings play music.
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b")]);
        app.handle_action(Action::OpenDjPanel);

        let effects = app.handle_action(Action::PlayPause);
        assert!(effects.is_empty(), "no playback from inside the panel");
        app.handle_action(Action::Down);
        assert_eq!(app.dj_panel.as_ref().unwrap().row, 1, "moves the panel, not the queue");
        assert_eq!(app.queue.current, None);

        app.handle_action(Action::Cancel);
        assert!(app.dj_panel.is_none(), "Esc closes it");
    }

    #[test]
    fn adjusting_a_row_changes_the_setting_it_names() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);

        // Row 0 is the mode; stepping right cycles it.
        app.handle_action(Action::SeekForward);
        assert_eq!(app.autodj, AutoDjMode::Similar);

        // Tightness moves in useful steps and stops at the ends rather than
        // wrapping — a slider that wraps loses your place.
        app.dj_panel.as_mut().unwrap().row = 1;
        assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Tightness);
        app.handle_action(Action::SeekForward);
        assert_eq!(app.dj.sonic_tightness, 5);
        for _ in 0..40 {
            app.handle_action(Action::SeekForward);
        }
        assert_eq!(app.dj.sonic_tightness, 100, "clamped at the top");
        for _ in 0..40 {
            app.handle_action(Action::Back);
        }
        assert_eq!(app.dj.sonic_tightness, 0, "and at the bottom, which is off");
    }

    #[test]
    fn panel_settings_are_remembered() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
        app.dj_panel.as_mut().unwrap().row = 1;
        app.handle_action(Action::SeekForward); // tightness 5

        let saved = app.prefs();
        assert_eq!(saved.dj.sonic_tightness, 5);
        let restored = App::new(None, None, None).with_prefs(&saved);
        assert_eq!(restored.dj, app.dj);
    }

    #[test]
    fn g_and_shift_g_jump_to_the_ends_of_the_panel() {
        // Found live: both keys were bound in panel mode but the settings
        // list ignored them, so `G` silently did nothing.
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
        app.handle_action(Action::Last);
        let panel = app.dj_panel.as_ref().unwrap();
        assert_eq!(panel.selected(), DjRow::Genres, "the last row");
        app.handle_action(Action::First);
        assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Mode);
    }

    #[test]
    fn choosing_a_genre_switches_the_filter_on() {
        // Picking genres while the mode is off would do nothing at all, which
        // reads as the chooser being broken.
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
        app.dj_panel.as_mut().unwrap().row = app.dj_panel.as_ref().unwrap().rows.len() - 1;
        assert_eq!(app.dj_panel.as_ref().unwrap().selected(), DjRow::Genres);

        let effects = app.handle_action(Action::Activate);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Genres)]);
        assert!(app.dj_panel.as_ref().unwrap().genres.as_ref().unwrap().loading);

        app.apply_event(Event::Genres(vec![
            Genre { name: "Ambient".into(), track_count: Some(4) },
            Genre { name: "Techno".into(), track_count: Some(9) },
        ]));
        let picker = app.dj_panel.as_ref().unwrap().genres.as_ref().unwrap();
        assert_eq!(picker.all, vec!["Ambient", "Techno"]);
        assert!(!picker.loading);

        app.handle_action(Action::PlayPause); // toggle "Ambient"
        assert_eq!(app.dj.genres, vec!["Ambient"]);
        assert_eq!(app.dj.genre_mode, dj::GenreMode::Whitelist, "switched on for you");

        // And toggling it back off leaves nothing selected.
        app.handle_action(Action::PlayPause);
        assert!(app.dj.genres.is_empty());

        app.handle_action(Action::Submit);
        assert!(app.dj_panel.as_ref().unwrap().genres.is_none(), "Enter closes the chooser");
        assert!(app.dj_panel.is_some(), "back to the panel, not out of it");
    }

    #[test]
    fn sampling_asks_for_picks_without_queueing_any() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);

        let effects = app.handle_action(Action::Input('p'));
        match effects.as_slice() {
            [Effect::Api(ApiCmd::AutoDjSample { count, .. })] => assert_eq!(*count, 3),
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.dj_panel.as_ref().unwrap().sample_pending);
        // A second press while one is out must not pile on.
        assert!(app.handle_action(Action::Input('p')).is_empty());

        app.apply_event(Event::AutoDjSample {
            tracks: vec![track("one"), track("two")],
            pool: Some(crate::api::types::SonicReport {
                similarity: Some(0.71),
                pool_size: 1247,
            }),
            note: None,
        });
        let panel = app.dj_panel.as_ref().unwrap();
        assert_eq!(panel.sample.len(), 2);
        assert_eq!(panel.pool.as_ref().unwrap().pool_size, 1247);
        assert!(!panel.sample_pending);
        assert!(app.queue.items.is_empty(), "a sample is not a queue");
    }

    #[test]
    fn a_request_carries_the_session_the_panel_is_tuning() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::BpmKey;
        app.dj.sonic_tightness = 50;
        app.dj.artist_cooldown = 2;

        // Two tracks played, newest first, with the artist of each.
        app.queue.replace(vec![
            track_by("a", "Alpha"),
            track_by("b", "Beta"),
            track_by("c", "Gamma"),
        ]);
        app.play_index(0);
        app.play_index(1);
        let effects = app.play_index(2);

        match autodj_effect(&effects).expect("the queue ran out") {
            ApiCmd::AutoDj(request) => {
                assert_eq!(request.anchors, vec!["c", "b", "a"], "newest first");
                assert_eq!(request.recent_artists, vec!["Gamma", "Beta", "Alpha"]);
                assert!(request.sonic_available, "this server has the index");
                assert_eq!(request.settings.sonic_tightness, 50);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn the_cooldown_list_does_not_repeat_an_artist() {
        // Three tracks by the same artist should not spend the whole cooldown.
        let mut app = connected_app();
        app.queue.replace(vec![
            track_by("a", "Alpha"),
            track_by("b", "Alpha"),
            track_by("c", "Beta"),
        ]);
        app.play_index(0);
        app.play_index(1);
        app.play_index(2);
        assert_eq!(app.recent_artists(), vec!["Beta", "Alpha"]);
    }

    #[test]
    fn autodj_skips_a_mode_the_server_cannot_serve() {
        // Default install: no embedding index. Offering "similar" would spend
        // a keystroke and a round trip to land on tempo+key anyway.
        let mut app = connected_app();
        app.capabilities = Default::default();

        app.handle_action(Action::ToggleAutoDj);
        assert_eq!(app.autodj, AutoDjMode::BpmKey, "straight past similar");
        app.handle_action(Action::ToggleAutoDj);
        assert_eq!(app.autodj, AutoDjMode::Off, "and the cycle still closes");
    }

    #[test]
    fn a_remembered_similar_mode_is_dropped_on_a_server_without_the_index() {
        // Preferences are global; capabilities are per-server. Reconnecting
        // elsewhere must not leave a mode selected that does something else.
        let saved = crate::config::PlayerPrefs {
            volume: 1.0,
            repeat: "off".into(),
            shuffle: false,
            autodj: "similar".into(),
            dj: Default::default(),
        };
        let mut app = App::new(None, None, None).with_prefs(&saved);
        assert_eq!(app.autodj, AutoDjMode::Similar);

        app.apply_event(Event::Connected {
            server: "http://plain:3000".into(),
            id: "http://plain:3000".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        assert_eq!(app.autodj, AutoDjMode::BpmKey);
        assert!(app.message.as_ref().unwrap().text.contains("similarity index"));

        // On a server that has one, the remembered mode is left alone.
        let mut app = App::new(None, None, None).with_prefs(&saved);
        app.apply_event(Event::Connected {
            server: "http://rich:3000".into(),
            id: "http://rich:3000".into(),
            username: None,
            token: None,
            ping: Box::new(crate::api::types::Ping {
                discovery: true,
                ..Default::default()
            }),
        });
        assert_eq!(app.autodj, AutoDjMode::Similar);
        assert!(app.capabilities.discovery);
    }

    #[test]
    fn switching_autodj_on_with_an_empty_queue_starts_it() {
        let mut app = connected_app();
        let effects = app.handle_action(Action::ToggleAutoDj);
        match autodj_effect(&effects).expect("a request goes out") {
            ApiCmd::AutoDj(request) => {
                assert_eq!(request.mode, AutoDjMode::Similar);
                assert!(request.seed.is_none());
                assert!(request.ignore_list.is_empty());
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn switching_autodj_on_does_not_jump_a_queue_the_user_built() {
        let mut app = connected_app();
        app.queue.replace(vec![track("a"), track("b")]);
        let effects = app.handle_action(Action::ToggleAutoDj);
        assert!(autodj_effect(&effects).is_none(), "there are tracks waiting already");
    }

    #[test]
    fn autodj_requests_only_once_the_queue_has_nothing_after_the_current_track() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::BpmKey;
        app.queue.replace(vec![track("a"), track("b")]);

        let effects = app.play_index(0);
        assert!(autodj_effect(&effects).is_none(), "one track still waiting");

        let effects = app.play_index(1);
        let cmd = autodj_effect(&effects).expect("the last track should pull in another");
        match cmd {
            ApiCmd::AutoDj(request) => {
                assert_eq!(request.mode, AutoDjMode::BpmKey);
                assert_eq!(
                    request.seed.as_ref().unwrap().filepath,
                    "b",
                    "seeded on what's playing"
                );
            }
            other => panic!("unexpected command {other:?}"),
        }

        // A second trigger while the first is unanswered must not pile on.
        assert!(app.maybe_autodj().is_empty());
    }

    #[test]
    fn autodj_picks_are_appended_and_deduped() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::Similar;
        app.queue.replace(vec![track("a")]);
        app.play_index(0);

        app.apply_event(Event::AutoDjPick {
            // The first candidate is already queued, so the second wins.
            candidates: vec![track("a"), track("b")],
            ignore_list: vec![7],
            note: None,
        });
        assert_eq!(app.queue.items.len(), 2);
        assert_eq!(app.queue.items[1].filepath, "b");
        assert_eq!(app.autodj_ignore, vec![7], "the cursor is kept for the next request");
        assert!(!app.autodj_pending);
    }

    #[test]
    fn an_autodj_pick_starts_playing_when_the_queue_ran_dry() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::Similar;
        // Nothing playing, nothing queued.
        let effects = app.apply_event(Event::AutoDjPick {
            candidates: vec![track("fresh")],
            ignore_list: Vec::new(),
            note: None,
        });
        assert_eq!(app.queue.items.len(), 1);
        assert!(matches!(effects[0], Effect::Audio(AudioCmd::Play { .. })));
        assert_eq!(app.queue.current, Some(0));
    }

    #[test]
    fn a_pick_arriving_after_autodj_is_switched_off_is_dropped() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::Similar;
        app.autodj_pending = true;
        app.autodj = AutoDjMode::Off;

        let effects = app.apply_event(Event::AutoDjPick {
            candidates: vec![track("late")],
            ignore_list: Vec::new(),
            note: None,
        });
        assert!(app.queue.items.is_empty());
        assert!(effects.is_empty());
    }

    #[test]
    fn a_fallback_note_is_surfaced_instead_of_the_track_name() {
        let mut app = connected_app();
        app.autodj = AutoDjMode::Similar;
        app.apply_event(Event::AutoDjPick {
            candidates: vec![track("x")],
            ignore_list: Vec::new(),
            note: Some("this track hasn't been analysed yet — matching tempo and key".into()),
        });
        let message = app.message.as_ref().unwrap();
        assert!(message.text.contains("analysed"), "the user learns why it fell back");
    }

    #[test]
    fn selection_stays_in_bounds() {
        let mut pane = Pane::default();
        pane.set(vec![Entry::Parent, Entry::Playlist { name: "x".into() }]);
        pane.move_by(-5);
        assert_eq!(pane.state.selected(), Some(0));
        pane.move_by(50);
        assert_eq!(pane.state.selected(), Some(1));

        // An empty pane has nothing selected and must not panic.
        pane.set(Vec::new());
        pane.move_by(1);
        assert_eq!(pane.state.selected(), None);
    }
}
