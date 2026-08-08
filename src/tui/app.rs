//! TUI state and the rules that change it.
//!
//! Deliberately I/O-free: actions and worker events go in, state changes and
//! [`Effect`]s come out, and the run loop is the only thing that touches
//! channels. That keeps the interesting behaviour — navigation, queue
//! advancement, repeat/shuffle — testable without a terminal or a server.

use std::collections::HashMap;
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
use crate::tui::art::Art;

use super::worker::{
    ApiCmd, AudioCmd, AutoDjMode, DiscoverData, DiscoverNode, DjRequest, Event, LibraryData,
    LibraryNode,
};

const SEEK_STEP: f64 = 5.0;
/// The shifted seek keys. Five seconds is the wrong unit for a long mix or a
/// set recording, where the interesting distance is minutes.
const SEEK_STEP_FAR: f64 = 60.0;
/// How long a just-issued seek outvotes the position status reports when
/// the next seek computes its base. Covers the round trip through the
/// worker and a status poll or two; short enough that a target the engine
/// lawfully landed short of (its end-of-track runway clamp) stops steering
/// follow-up presses once it has gone stale.
const SEEK_CHAIN: std::time::Duration = std::time::Duration::from_millis(2500);
const VOLUME_STEP: f32 = 0.05;
/// Rows a page key moves. Ctrl+u/d move half of this, as they do in vim.
const PAGE_STEP: isize = 10;
/// Covers held before the cache is emptied wholesale. An evening of
/// listening crosses fewer albums than this; the point is only that a
/// player left running for a week cannot grow without bound. Wholesale
/// rather than LRU because correctness needs only the bound, and by the
/// time it is hit the oldest entries are hours stale anyway.
const ART_CACHE_CAP: usize = 64;

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
    SonicPath,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Files,
        Tab::Library,
        Tab::Playlists,
        Tab::Search,
        Tab::Discover,
        Tab::SonicPath,
        Tab::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Library => "Library",
            Tab::Playlists => "Playlists",
            Tab::Search => "Search",
            Tab::Discover => "Discover",
            Tab::SonicPath => "Sonic Path",
            Tab::Settings => "Settings",
        }
    }

    /// Whether this server can serve the tab at all. A tab that can only ever
    /// say "not available here" is worse than no tab.
    pub fn available(self, capabilities: crate::api::types::Capabilities) -> bool {
        match self {
            Tab::Discover => capabilities.discovery,
            // Its own flag, not `discovery`: the server reports the two
            // separately and an index without paths is a real configuration.
            Tab::SonicPath => capabilities.discovery_path,
            _ => true,
        }
    }
}

/// Seconds of blend as a person reads them: whole when whole, one decimal
/// when the config was hand-written fractional.
fn fmt_blend(seconds: f32) -> String {
    if seconds.fract() == 0.0 {
        format!("{seconds:.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}

/// A place in the Settings tab's little hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsNode {
    /// The menu of settings groups — one group so far.
    Root,
    /// The crossfade group: how tracks hand over.
    Crossfade,
}

/// What a Settings row is, and so what Enter and ←→ do to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    /// The root row that opens the crossfade group.
    CrossfadeMenu,
    /// Seconds of blend; ← and → walk it, Enter steps it up.
    BlendLength,
    /// Sample-tight boundaries when no blend is set; anything toggles it.
    Gapless,
    /// Manual skips blend for a second instead of breathing.
    BlendSkips,
    /// Pause and resume ride a short ramp instead of landing mid-wave.
    PauseFade,
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

/// The one place the config file's repeat meets the shared advance rules.
impl From<Repeat> for crate::advance::Loop {
    fn from(repeat: Repeat) -> Self {
        match repeat {
            Repeat::Off => crate::advance::Loop::Off,
            Repeat::All => crate::advance::Loop::All,
            Repeat::One => crate::advance::Loop::One,
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
    /// One of the full-screen view's tabs, by position among the visible
    /// ones — the same rule the browser's numbers follow, so they stay 1..n
    /// with no gaps whichever tabs this track and this server allow.
    SelectNowTab(usize),
    /// ←/→ in the full-screen view, offered to whichever tab is in front.
    /// Auto-DJ takes them to adjust the row under the cursor; the rest have
    /// no use for them.
    ///
    /// They used to switch tabs, with Auto-DJ as the exception — which meant
    /// the one screen you could get *stuck* on was the one whose escape key
    /// was different. Navigation is the numbers now.
    NowLeft,
    NowRight,
    VolumeUp,
    VolumeDown,
    RemoveFromQueue,
    ClearQueue,
    ToggleRepeat,
    ToggleShuffle,
    ToggleAutoDj,
    /// `J` — open the Sonic Path tab aimed at the highlighted track.
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
    /// A row in the Settings tab: a drill into a settings group, or a live
    /// value. `detail` carries the current value and its meaning.
    Setting { label: String, detail: String, row: SettingRow },
    /// A row in the Sonic Path tab — a chosen end, a length, or a thing to
    /// do with the path. Same shape as [`Entry::Setting`] and for the same
    /// reason: a label on the left, what it is set to on the right.
    Sonic { label: String, detail: String, row: SonicRow },
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
            Entry::Setting { label, .. } => label,
            Entry::Sonic { label, .. } => label,
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
    ///
    /// The index arithmetic is [`crate::advance::shift_current`]'s, shared
    /// with the engine's queue; dropping `current` on `RemovedCurrent` —
    /// stop, rather than restart in place — is this side's policy.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.items.remove(index);

        let was_current = match self.current {
            Some(cur) => {
                use crate::advance::RemoveOutcome;
                let (shifted, outcome) =
                    crate::advance::shift_current(self.items.len(), cur, index);
                match outcome {
                    // Emptying the queue removed the playing row too — a
                    // one-track queue has nowhere else to point.
                    RemoveOutcome::EmptiedQueue | RemoveOutcome::RemovedCurrent => {
                        self.current = None;
                        true
                    }
                    _ => {
                        self.current = Some(shifted);
                        false
                    }
                }
            }
            None => false,
        };

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

    /// Next track to play. The rules live in [`crate::advance`], shared with
    /// the engine's queue — this used to be its own copy, synced by hand
    /// (audit #62). This is the queue that counts its shuffle pass, so
    /// repeat-off ends even shuffled (audit #35).
    pub fn next_index(&self, manual: bool) -> Option<usize> {
        crate::advance::pick_next(
            self.items.len(),
            self.current,
            self.shuffle,
            self.repeat.into(),
            manual,
            Some(self.played),
        )
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

// Auto-DJ lives in `autodj` (audit #57) — named for what it does, since `dj`
// is already the crate's Camelot/BPM helpers. The Sonic Path tab, which used
// to share that file as an overlay, is `sonic`. The way in — the connect
// screen, and the [`Session`] it produces — lives in `session` (audit #56),
// re-exported so every caller keeps saying `app::ConnectForm`.
mod autodj;
mod entries;
mod nav;
mod session;
mod sonic;

// The row builders moved out whole (audit #61); the app and its tests
// go on naming them exactly as they did.
use entries::*;
pub use nav::Drill;
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
    /// Not a setting — the row that asks what these settings actually pick.
    /// A row rather than a key of its own, because in a tab (as opposed to
    /// the modal this used to be) every key that means something has to be
    /// one the rest of the player is not already using.
    Sample,
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
            DjRow::Sample => "Sample",
        }
    }
}

/// The Auto-DJ tab's own state. Rows shown depend on what the server can do,
/// so they are rebuilt when a ping says what that is.
#[derive(Debug)]
pub struct DjPanel {
    pub rows: Vec<DjRow>,
    pub row: usize,
    /// Genre chooser, when open over the tab. The one modal left in Auto-DJ:
    /// a list you toggle through needs the keyboard to itself.
    pub genres: Option<GenrePicker>,
    /// Sample picks from the current settings, and what the pool looked like.
    pub sample: Vec<Track>,
    pub sample_pending: bool,
    pub pool: Option<crate::api::types::SonicReport>,
}

impl Default for DjPanel {
    fn default() -> Self {
        DjPanel {
            rows: DjPanel::rows_for(crate::api::types::Capabilities::default()),
            row: 0,
            genres: None,
            sample: Vec::new(),
            sample_pending: false,
            pool: None,
        }
    }
}

impl DjPanel {
    fn rows_for(capabilities: crate::api::types::Capabilities) -> Vec<DjRow> {
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
            DjRow::Sample,
        ]);
        rows
    }

    /// Fit the rows to what this server offers, keeping the cursor on screen.
    /// Called when a ping lands, which is the only thing that can change the
    /// answer.
    pub(super) fn rebuild(&mut self, capabilities: crate::api::types::Capabilities) {
        self.rows = DjPanel::rows_for(capabilities);
        self.row = self.row.min(self.rows.len().saturating_sub(1));
    }

    pub fn selected(&self) -> DjRow {
        self.rows.get(self.row).copied().unwrap_or(DjRow::Mode)
    }
}

// ── Sonic Path ──────────────────────────────────────────────────────────────

/// Which end of the path a row or an armed picker is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonicSide {
    Start,
    End,
}

impl SonicSide {
    pub fn label(self) -> &'static str {
        match self {
            SonicSide::Start => "Start song",
            SonicSide::End => "End song",
        }
    }

    /// How the arming banner names it — shouted, because the banner is the
    /// only thing on screen saying why Enter is not doing its usual job.
    pub fn shout(self) -> &'static str {
        match self {
            SonicSide::Start => "START",
            SonicSide::End => "END",
        }
    }
}

/// Where the Sonic Path tab is in its own little hierarchy. Mirrors
/// [`SettingsNode`]: a root list of rows, and one drill-in per end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonicNode {
    /// Picking the two ends and a length, or looking at the path that came
    /// back — which of those is [`SonicPath::view`], not a different node,
    /// because Back out of either means "leave the tab", not "un-build".
    Root,
    /// One end's little menu: use what's playing, pick from the library,
    /// clear it.
    Side(SonicSide),
}

/// Which of the tab's two faces is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonicView {
    /// Choose the ends and the length.
    Setup,
    /// The path itself, with what can be done to it.
    Results,
}

/// One actionable row in the Sonic Path tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonicRow {
    /// Setup: drill into an end's menu.
    End(SonicSide),
    /// Setup and results both: how many stops to ask for.
    Length,
    Build,
    /// Inside an end's menu.
    UsePlaying,
    PickFromLibrary,
    Clear,
    /// Results.
    Play,
    QueueAll,
    SavePlaylist,
    Regenerate,
    StartOver,
    /// Not a control — how the plot went, on a row of its own. Enter does
    /// nothing on it, which is the point: an unanalysed end or a library
    /// that ran out of waypoints is an answer, and it wants saying where
    /// the answer would have been.
    Status,
}

/// The Sonic Path tab's state: two ends, a length, and whatever the server
/// last drew between them.
///
/// The webapp's panel, in the shapes this player already has — the ends are
/// [`Track`]s rather than cards, the slider is a row with ←→ on it, and the
/// stops become ordinary track rows so `Enter`, `a` and the queue keys all
/// go on meaning what they mean everywhere else.
#[derive(Debug)]
pub struct SonicPath {
    pub view: SonicView,
    pub start: Option<Track>,
    pub end: Option<Track>,
    /// Total stops asked for, both ends included.
    pub length: u32,
    pub stops: Vec<crate::api::types::JourneyStop>,
    pub pending: bool,
    /// Whether a build has ever come back, so an empty list can tell "no
    /// path between these two" from "you haven't asked yet".
    pub fetched: bool,
    /// What the server's answer needs explaining as — an unanalysed end, two
    /// copies of one recording, a library that ran out of waypoints. Kept on
    /// the panel rather than flashed in the footer: it is the state of what
    /// is on screen, not news.
    pub note: Option<String>,
}

impl Default for SonicPath {
    fn default() -> Self {
        SonicPath {
            view: SonicView::Setup,
            start: None,
            end: None,
            length: crate::api::types::JOURNEY_DEFAULT_LENGTH,
            stops: Vec::new(),
            pending: false,
            fetched: false,
            note: None,
        }
    }
}

impl SonicPath {
    pub fn side(&self, side: SonicSide) -> Option<&Track> {
        match side {
            SonicSide::Start => self.start.as_ref(),
            SonicSide::End => self.end.as_ref(),
        }
    }

    fn set_side(&mut self, side: SonicSide, track: Option<Track>) {
        match side {
            SonicSide::Start => self.start = track,
            SonicSide::End => self.end = track,
        }
    }

    /// Both ends chosen — the only state [`SonicRow::Build`] can act from.
    pub fn ready(&self) -> bool {
        self.start.is_some() && self.end.is_some()
    }

    /// The stops as a queue.
    pub fn tracks(&self) -> Vec<Track> {
        self.stops.iter().map(|stop| stop.to_track()).collect()
    }
}

/// Choosing which genres the filter applies to.
#[derive(Debug, Default)]
pub struct GenrePicker {
    /// Every genre the server knows, alphabetical as it sent them.
    pub all: Vec<String>,
    pub row: usize,
    pub loading: bool,
}

/// The next track, as announced to the engine for a crossfade — plus
/// everything the pick was made against, so its staleness can be seen from
/// here. The pick is rolled once and then *held*: recomputed each refresh,
/// a shuffled next would come up different every time, and the engine would
/// open a track that stops being the answer before it plays.
struct AnnouncedNext {
    /// The playing row this pick is "next" relative to.
    for_current: usize,
    index: usize,
    /// The track that was at `index` — how a queue edit shows up.
    filepath: String,
    url: String,
    hint: Option<f64>,
    /// The rules of the roll; either changing re-rolls it.
    shuffle: bool,
    repeat: Repeat,
    /// Whether this announced the track to itself — the gapless repeat-one
    /// seam. Snapshotted because the seam is only lawful under the mode
    /// that made it: left standing when crossfade comes on, it fed the
    /// engine a self to blend into, the exact invisible transition the
    /// duplicate refusal exists to prevent.
    self_seam: bool,
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
    pub library_stack: Drill<LibraryNode>,
    /// The whole search reply, kept rather than flattened. Every class comes
    /// back in one response, so moving between them costs nothing.
    pub search_hits: Option<Box<crate::api::types::SearchResults>>,
    pub search_stack: Drill<SearchNode>,
    /// Whether the queue is showing as the last column. It is the end of the
    /// same chain -- artist, album, track, queued -- so it reads better as one
    /// more column than as a separate pane that is always there.
    pub queue_column: bool,
    pub playlists: Pane,
    pub playlist_open: Option<String>,
    pub discover: Pane,
    pub settings: Pane,
    /// Breadcrumb through the Discover tab, mirroring `library_stack`.
    pub discover_stack: Drill<DiscoverNode>,
 settings_stack: Drill<SettingsNode>,
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
    /// The query whose results are still wanted — the last one submitted.
    /// Replies answer on their own threads and can pass each other, so a
    /// result set has to name the search it belongs to.
    search_submitted: Option<String>,

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
    /// The Auto-DJ tab of the full-screen view: which row the cursor is on,
    /// what a sample produced, the genre chooser when it is open. Always
    /// present now that the tab *is* the panel — there is no separate modal
    /// to be open or shut.
    pub dj_panel: DjPanel,
    /// The Sonic Path tab: two ends, a length, and the path between them.
    pub sonic: SonicPath,
    pub sonic_pane: Pane,
    pub sonic_stack: Drill<SonicNode>,
    /// An armed song picker: the next track chosen anywhere in the browser
    /// fills this end instead of playing. The webapp's `songCapture`, and
    /// the reason it lives on the App rather than on [`SonicPath`] — it is
    /// answered from whichever tab the user wanders into.
    pub sonic_capture: Option<SonicSide>,
    /// The playlist name being typed, when the save prompt is up.
    pub sonic_playlist_name: Option<String>,
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
    /// How the Quick Connect tunnel is reaching the server, when this
    /// session rides one. Fed by the worker's sampler; the header shows it
    /// beside the session name. None outside tunnel sessions.
    pub tunnel_path: Option<crate::quickconnect::TunnelPath>,
    /// How to walk the file browser back if the Browse in flight fails: the
    /// path the click replaced, and whether it pushed a trail column on the
    /// way in. A tunnel that flakes mid-browse otherwise left the navigation
    /// standing — path one level deep, a phantom column beside a listing
    /// that never changed — and every retry of the click stacked another
    /// copy. Cleared by the listing that answers; consumed by the error
    /// that doesn't. A late success for an undone path is dropped by the
    /// listing handler's own path guard.
    browse_undo: Option<(String, bool)>,
    /// Where the last seek was told to land, so the next press builds on it.
    /// Status refreshes a few times a second, and seek keys compute their
    /// target from the last position heard — so a quick `}}}` all read the
    /// same stale base and moved one minute, not three. Cleared when a
    /// status shows the seek arrived (or the source changed under it), and
    /// trusted only briefly: the engine may lawfully land a forward seek
    /// short of the ask (its end-of-track runway clamp), and an expired
    /// goal must not keep outvoting reality.
    seek_goal: Option<(f64, std::time::Instant)>,
    pub volume: f32,
    /// Seconds of blend between tracks, from `crossfade_seconds` in
    /// config.toml; 0 is off. Also adjustable in the Auto-DJ panel (C4).
    pub crossfade: f32,
    /// Sample-tight boundaries when no blend is set, from `gapless` in
    /// config.toml. The announcement machinery serves both.
    pub gapless: bool,
    /// Manual skips blend for a second instead of breathing (C6).
    pub blend_skips: bool,
    /// Pause and resume ride a short ramp (C6).
    pub pause_fade: bool,
    /// The next track as told to the engine, so a blend can open it early.
    /// Held to keep the answer stable: recomputing on every refresh would
    /// re-roll a shuffled pick each time, and to know whether the engine's
    /// announcement is stale without asking it.
    announced: Option<AnnouncedNext>,
    pub now_playing: Option<Track>,
    /// Covers fetched this session, keyed by the server's art filename.
    /// `None` records both "asked, nothing there" and "asked, still
    /// waiting" — the two draw the same, and the entry is what stops a
    /// second request either way.
    pub art: HashMap<String, Option<Art>>,
    /// Track shapes fetched this session, keyed by filepath. `None` records
    /// both "asked, nothing there" and "asked, still waiting" — the bar draws
    /// the same either way, and the entry is what stops a second request.
    ///
    /// Keyed by filepath rather than by an art file, because a waveform
    /// belongs to one recording where a cover belongs to a whole album — so
    /// this turns over faster than [`App::art`] does.
    pub waveforms: HashMap<String, Option<Vec<u8>>>,
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
    /// The tap's latest audio, refilled in place each frame. The visualiser
    /// asks thirty times a second and the answer is the same size every
    /// time, so the buffer is kept rather than allocated per frame — and
    /// held across the reads that miss (the tap only ever *tries* its
    /// lock), so one busy tick freezes the picture instead of blinking it
    /// into placeholder text.
    pub heard: crate::engine::tap::TapFrame,
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
            library_stack: Drill::new(LibraryNode::Root),
            search_hits: None,
            search_stack: Drill::new(SearchNode::Root),
            queue_column: false,
            playlists: Pane::default(),
            playlist_open: None,
            discover: Pane::default(),
            discover_stack: Drill::new(DiscoverNode::Root),
            settings: Pane::default(),
            settings_stack: Drill::new(SettingsNode::Root),
            discover_seed: None,
            discover_artists: Vec::new(),
            search: Pane::default(),
            query: String::new(),
            editing_query: false,
            filtering: false,
            search_summary: None,
            search_submitted: None,
            queue: Queue::default(),
            capabilities: Default::default(),
            libraries: Vec::new(),
            autodj: AutoDjMode::Off,
            dj: dj::Settings::default(),
            dj_panel: DjPanel::default(),
            sonic: SonicPath::default(),
            sonic_pane: Pane::default(),
            sonic_stack: Drill::new(SonicNode::Root),
            sonic_capture: None,
            sonic_playlist_name: None,
            autodj_recent: Vec::new(),
            autodj_ignore: Vec::new(),
            autodj_pending: false,
            status: PlayerStatus::default(),
            starting: None,
            failures: 0,
            browse_undo: None,
            tunnel_path: None,
            seek_goal: None,
            volume: 1.0,
            crossfade: 0.0,
            gapless: false,
            blend_skips: false,
            pause_fade: false,
            announced: None,
            now_playing: None,
            art: HashMap::new(),
            waveforms: HashMap::new(),
            audio_available: true,
            tap: None,
            pointer: None,
            viz: Default::default(),
            heard: Default::default(),
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
        // The same clamp as the engine's, so the file and the behavior
        // agree; anything unreadable costs the blend and nothing else.
        self.crossfade = if prefs.crossfade_seconds.is_finite() {
            prefs.crossfade_seconds.clamp(0.0, 30.0)
        } else {
            0.0
        };
        self.gapless = prefs.gapless;
        self.blend_skips = prefs.blend_skips;
        self.pause_fade = prefs.pause_fade;
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
            crossfade_seconds: self.crossfade,
            gapless: self.gapless,
            blend_skips: self.blend_skips,
            pause_fade: self.pause_fade,
            dj: self.dj.to_prefs(),
            // Settings from a newer player belong to the file, not to this
            // app's state; `PlayerPrefs::adopt` is what carries them across.
            ..Default::default()
        }
    }

    /// Effects to run at startup.
    pub fn start(&mut self) -> Vec<Effect> {
        let effects = self.begin();
        self.note_pending(&effects);
        effects
    }

    pub fn input_mode(&self) -> InputMode {
        if !self.connected
            || self.editing_query
            || self.filtering
            || self.sonic_playlist_name.is_some()
        {
            InputMode::Editing
        } else if self.dj_panel.genres.is_some() {
            // The last modal in the player, and it is drawn over the
            // full-screen view, so it still owns the keyboard there.
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

    // The Tab → pane mapping lives in this adjacent pair and nowhere else
    // (audit #59). It used to be five tables — two of them uncheckable, so
    // a missed arm was a spinner that never lit or never stopped — and
    // every other reader now routes through these exhaustive matches.

    pub fn pane(&self) -> &Pane {
        self.pane_for(self.tab)
    }

    fn pane_mut(&mut self) -> &mut Pane {
        let tab = self.tab;
        self.pane_for_mut(tab)
    }

    pub(crate) fn pane_for(&self, tab: Tab) -> &Pane {
        match tab {
            Tab::Files => &self.files,
            Tab::Library => &self.library,
            Tab::Playlists => &self.playlists,
            Tab::Search => &self.search,
            Tab::Discover => &self.discover,
            Tab::SonicPath => &self.sonic_pane,
            Tab::Settings => &self.settings,
        }
    }

    pub(crate) fn pane_for_mut(&mut self, tab: Tab) -> &mut Pane {
        match tab {
            Tab::Files => &mut self.files,
            Tab::Library => &mut self.library,
            Tab::Playlists => &mut self.playlists,
            Tab::Search => &mut self.search,
            Tab::Discover => &mut self.discover,
            Tab::SonicPath => &mut self.sonic_pane,
            Tab::Settings => &mut self.settings,
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
                Effect::Api(ApiCmd::Library { dest, .. }) => *dest,
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
    ///
    /// Not while it is standing still. Paused, the picture falls to nothing
    /// within a couple of seconds and every frame after that is identical —
    /// but the loop went on waking thirty times a second to copy the ring
    /// and run a 2048-point transform over silence, for as long as the tab
    /// was left open (finding #51). The visualiser reports when it has
    /// settled, and the wait goes back to the ordinary one until something
    /// sounds again.
    pub fn drawing_audio(&self) -> bool {
        self.fullscreen && self.now_tab() == NowTab::Visualizer && !self.viz.still()
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
        for tab in Tab::ALL {
            self.pane_for_mut(tab).loading = false;
        }
        // The Sonic Path tab keeps its own wait, since what it is waiting for
        // is a path rather than a listing — and a wait nothing will answer
        // leaves the tab saying "plotting…" for the rest of the session.
        if self.sonic.pending {
            self.sonic.pending = false;
            self.sonic.fetched = true;
            self.refresh_sonic_rows();
        }
    }

    /// Where the Search tab is: the class menu, one class, or something
    /// opened out of it.
    pub fn search_node(&self) -> &SearchNode {
        self.search_stack.here()
    }

    /// The library view currently on screen.
    pub fn library_node(&self) -> &LibraryNode {
        self.library_stack.here()
    }

    pub fn settings_node(&self) -> &SettingsNode {
        self.settings_stack.here()
    }

    pub fn discover_node(&self) -> &DiscoverNode {
        self.discover_stack.here()
    }

    pub fn sonic_node(&self) -> &SonicNode {
        self.sonic_stack.here()
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
        let mut effects = self.act(action);
        effects.extend(self.refresh_prepared());
        // Rides the same funnel as the announcement it reads, so a queue
        // edit that changes what comes next also changes which shape is
        // being fetched ahead of it.
        effects.extend(self.prefetch_waveform());
        self.note_pending(&effects);
        effects
    }

    fn act(&mut self, action: Action) -> Vec<Effect> {
        // The connect screen swallows everything except quit.
        if !self.connected {
            return self.handle_connect_action(action);
        }
        // The genre chooser is modal: it owns the arrow keys and the letters
        // it uses, so playback shortcuts can't fire while it is up.
        if self.dj_panel.genres.is_some() {
            return self.handle_genre_action(action);
        }
        if self.sonic_playlist_name.is_some()
            && let Some(effects) = self.handle_playlist_name_action(&action)
        {
            return effects;
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
                // An armed picker is the loudest thing on screen, so Esc
                // means "stop picking" before it means anything else.
                if self.sonic_capture.take().is_some() {
                    self.message = None;
                    return self.open_sonic_tab();
                }
                // The guaranteed way out of the Crossfade rows, where the
                // left arrow has been given to adjustment.
                if self.tab == Tab::Settings && *self.settings_node() == SettingsNode::Crossfade {
                    return self.go_back();
                }
                if self.tab == Tab::SonicPath && !self.sonic_stack.wants(&SonicNode::Root) {
                    return self.go_back();
                }
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
                        NowTab::AutoDj => self.dj_panel.row = 0,
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
                    // the two that are lists.
                    match self.now_tab() {
                        NowTab::Queue => self.queue.select_last(),
                        NowTab::AutoDj => {
                            self.dj_panel.row = self.dj_panel.rows.len().saturating_sub(1);
                        }
                        _ => {}
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
            Action::Back => {
                // On a Settings or Sonic Path VALUE row, ← means "less" —
                // the way out is Esc, h on the `..` row, or the row above it.
                if let Some(effects) = self.settings_step(-1) {
                    effects
                } else if let Some(effects) = self.sonic_step(-1) {
                    effects
                } else {
                    self.go_back()
                }
            }
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
            Action::SelectNowTab(index) => {
                if let Some(tab) = self.now_tabs().get(index).copied() {
                    self.now_tab = tab;
                    self.now_scroll = 0;
                }
                Vec::new()
            }
            // Offered to the tab in front; only Auto-DJ has anything to do
            // with them.
            Action::NowLeft if self.on_dj_tab() => self.adjust_dj_row(-1),
            Action::NowRight if self.on_dj_tab() => self.adjust_dj_row(1),
            Action::NowLeft | Action::NowRight => Vec::new(),
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
                self.search_stack.reset();
                self.search.trail.clear();
                self.search_submitted = Some(query.clone());
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
            Tab::Library if self.library_stack.unopened() => {
                self.library_stack.enter(LibraryNode::Root);
                self.library.set(library_root_entries());
                Vec::new()
            }
            Tab::Discover => {
                self.set_discover_seed(carried);
                if self.discover_stack.unopened() {
                    self.discover_stack.enter(DiscoverNode::Root);
                }
                if *self.discover_node() == DiscoverNode::Root {
                    self.discover.set(self.discover_root_entries());
                }
                Vec::new()
            }
            Tab::SonicPath => {
                if self.sonic_stack.unopened() {
                    self.sonic_stack.enter(SonicNode::Root);
                }
                // Landing on a pristine tab with something playing: seed the
                // start, the way "Use playing song" would. Only on a panel
                // nobody has touched — clearing an end and stepping away
                // must not have it quietly filled back in.
                if self.sonic.view == SonicView::Setup
                    && self.sonic.start.is_none()
                    && self.sonic.end.is_none()
                    && let Some(track) = self.now_playing.clone()
                {
                    self.sonic.start = Some(track);
                }
                self.refresh_sonic_rows();
                Vec::new()
            }
            Tab::Settings => {
                if self.settings_stack.unopened() {
                    self.settings_stack.enter(SettingsNode::Root);
                }
                // Values may have moved since the tab was last looked at
                // (the config loaded them, this tab edits them), so the
                // rows are rebuilt on every visit — in place, keeping the
                // cursor where it was left.
                self.refresh_settings_rows();
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
            // The Auto-DJ tab is a list of settings, not a wall of text: the
            // arrows walk its rows, the way they do in every other list.
            if self.now_tab() == NowTab::AutoDj {
                let last = self.dj_panel.rows.len().saturating_sub(1);
                let row = (self.dj_panel.row as isize + delta).clamp(0, last as isize);
                self.dj_panel.row = row as usize;
                return Vec::new();
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
        // so Enter means the one thing the tab in front of you can mean.
        if self.fullscreen {
            if self.on_dj_tab() {
                return self.activate_dj_row();
            }
            if self.now_tab() != NowTab::Queue {
                return Vec::new();
            }
            return match self.queue.state.selected() {
                Some(index) => self.play_index(index),
                None => Vec::new(),
            };
        }

        // An armed picker takes the next track chosen anywhere, exactly as
        // the webapp's does — the browser is where the songs are, so this is
        // where the arming is answered.
        if let Some(side) = self.sonic_capture
            && self.focus == Focus::Browser
            && let Some(Entry::Track { track, .. }) = self.pane().selected().cloned()
        {
            return self.capture_sonic_side(side, *track);
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
                let pushed = self.push_trail();
                let prev = std::mem::replace(&mut self.path, path.clone());
                self.browse_undo = Some((prev, pushed));
                // Empty it, so what shows while the reply is out is this
                // folder's spinner and not the last folder's contents. The
                // trail column beside it has already moved, so leaving the
                // old rows there read as a folder that opened into itself —
                // and on a fast server it flashed past too quickly to be
                // anything but a glitch. Every other tab's drill-in has
                // always cleared; the file browser was the one that didn't.
                self.pane_mut().set(Vec::new());
                vec![Effect::Api(ApiCmd::Browse(path))]
            }
            Entry::Node { node, label } if self.tab == Tab::Search => {
                self.push_trail();
                self.search_stack.enter(SearchNode::Library(node.clone()));
                self.search.set(Vec::new());
                self.info(format!("loading {label}…"));
                vec![Effect::Api(ApiCmd::Library { node, dest: Tab::Search })]
            }
            Entry::Node { node, label } => {
                self.push_trail();
                self.library_stack.enter(node.clone());
                self.library.set(Vec::new());
                self.info(format!("loading {label}…"));
                vec![Effect::Api(ApiCmd::Library { node, dest: Tab::Library })]
            }
            Entry::Search { node, label, .. } => {
                self.push_trail();
                self.search_stack.enter(node.clone());
                match node {
                    // Every class is already in hand -- no request.
                    SearchNode::Class(_) | SearchNode::Root => {
                        self.search.set(self.search_class_entries());
                        Vec::new()
                    }
                    SearchNode::Library(node) => {
                        self.search.set(Vec::new());
                        self.info(format!("loading {label}…"));
                        vec![Effect::Api(ApiCmd::Library { node, dest: Tab::Search })]
                    }
                }
            }
            Entry::Discover { node, label, .. } => {
                self.push_trail();
                self.open_discover(node, &label)
            }
            Entry::Playlist { name } => {
                self.push_trail();
                // Same rule as every other drill-in: the list of playlists is
                // not this playlist's tracks, and showing it until they land
                // is a wrong answer rather than a slow one.
                self.playlists.set(Vec::new());
                self.info(format!("loading playlist {name}…"));
                // Open now rather than when the tracks land, so this is a
                // record of where the user went instead of a record of what
                // last answered — which is what lets a late reply be told
                // apart from a wanted one, and lets Back close a playlist
                // that has not answered yet.
                self.playlist_open = Some(name.clone());
                vec![Effect::Api(ApiCmd::LoadPlaylist(name))]
            }
            Entry::Setting { row, .. } => match row {
                SettingRow::CrossfadeMenu => {
                    self.push_trail();
                    self.settings_stack.enter(SettingsNode::Crossfade);
                    self.settings.set(self.crossfade_setting_entries());
                    Vec::new()
                }
                // Enter walks a value the same way → does; ← walks it back.
                SettingRow::BlendLength
                | SettingRow::Gapless
                | SettingRow::BlendSkips
                | SettingRow::PauseFade => self.adjust_setting(1),
            },
            Entry::Sonic { row, .. } => self.activate_sonic_row(row),
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
    fn push_trail(&mut self) -> bool {
        let pane = self.pane_mut();
        if pane.entries.is_empty() {
            return false;
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
        true
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
            // Nowhere further out — but a trail column here is an orphan
            // (a failed browse from before the undo existed, or any bug
            // that strands one), Back is the only key anyone would reach
            // for, and refusing to pop left it on screen for the rest of
            // the session. Drain it instead.
            if let Some(step) = self.pane_mut().trail.pop() {
                let pane = self.pane_mut();
                pane.filter.clear();
                pane.unfiltered = None;
                pane.entries = step.entries;
                pane.state.select(Some(step.chosen));
                pane.loading = false;
            }
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
                // Going out is answered from the trail; if this refresh
                // fails, path and pane still agree on the parent. Nothing
                // to walk back — and an undo left armed here would fire on
                // some later unrelated error and yank the path deeper.
                self.browse_undo = None;
                vec![Effect::Api(ApiCmd::Browse(parent))]
            }
            Tab::Search => {
                let Some(node) = self.search_stack.back() else {
                    return None; // already at the class menu
                };
                match node {
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
                        vec![Effect::Api(ApiCmd::Library { node, dest: Tab::Search })]
                    }
                }
            }
            Tab::Library => {
                let Some(node) = self.library_stack.back() else {
                    return None; // already at the mode menu
                };
                match node {
                    LibraryNode::Root => {
                        self.library.set(library_root_entries());
                        Vec::new()
                    }
                    node => {
                        self.library.set(Vec::new());
                        vec![Effect::Api(ApiCmd::Library { node, dest: Tab::Library })]
                    }
                }
            }
            Tab::Playlists if self.playlist_open.is_some() => {
                self.playlist_open = None;
                vec![Effect::Api(ApiCmd::Playlists)]
            }
            Tab::Discover => {
                let Some(node) = self.discover_stack.back() else {
                    return None; // already at the mode menu
                };
                match node {
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
            Tab::SonicPath => {
                let Some(_) = self.sonic_stack.back() else {
                    return None; // already at the path itself
                };
                self.refresh_sonic_rows();
                Vec::new()
            }
            Tab::Settings => {
                let Some(node) = self.settings_stack.back() else {
                    return None; // already at the settings menu
                };
                match node {
                    SettingsNode::Root => self.settings.set(self.settings_root_entries()),
                    SettingsNode::Crossfade => {
                        self.settings.set(self.crossfade_setting_entries())
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        })
    }

    // ── Settings ────────────────────────────────────────────────────────────

    fn settings_root_entries(&self) -> Vec<Entry> {
        vec![Entry::Setting {
            label: "Crossfade".into(),
            detail: format!("{} · how tracks hand over", self.crossfade_summary()),
            row: SettingRow::CrossfadeMenu,
        }]
    }

    /// The state at a glance, for the root row.
    fn crossfade_summary(&self) -> String {
        if self.crossfade > 0.0 {
            format!("{} blend", fmt_blend(self.crossfade))
        } else if self.gapless {
            "gapless".to_string()
        } else {
            "off".to_string()
        }
    }

    fn crossfade_setting_entries(&self) -> Vec<Entry> {
        let blend = if self.crossfade <= 0.0 {
            "off · tracks cut over · → for a blend".to_string()
        } else {
            format!(
                "{} · equal-power, when a track ends on its own · ←→ adjust",
                fmt_blend(self.crossfade)
            )
        };
        let gapless = if self.crossfade > 0.0 {
            format!(
                "{} · crossfade wins while it is on",
                if self.gapless { "on" } else { "off" }
            )
        } else if self.gapless {
            "on · sample-tight boundaries".to_string()
        } else {
            "off · Enter to turn on".to_string()
        };
        let blend_skips = if self.blend_skips {
            "on · a skip crosses in a second".to_string()
        } else {
            "off · a skip is a clean cut".to_string()
        };
        let pause_fade = if self.pause_fade {
            "on · pause and resume ride a short ramp".to_string()
        } else {
            "off · pause lands at once".to_string()
        };
        vec![
            Entry::Parent,
            Entry::Setting {
                label: "Blend length".into(),
                detail: blend,
                row: SettingRow::BlendLength,
            },
            Entry::Setting { label: "Gapless".into(), detail: gapless, row: SettingRow::Gapless },
            Entry::Setting {
                label: "Blend skips".into(),
                detail: blend_skips,
                row: SettingRow::BlendSkips,
            },
            Entry::Setting {
                label: "Pause fade".into(),
                detail: pause_fade,
                row: SettingRow::PauseFade,
            },
        ]
    }

    /// Rebuild whichever settings view is showing, in place: the values in
    /// the details are live, and [`Pane::set`] would throw the cursor away.
    fn refresh_settings_rows(&mut self) {
        let entries = match self.settings_node() {
            SettingsNode::Root => self.settings_root_entries(),
            SettingsNode::Crossfade => self.crossfade_setting_entries(),
        };
        let selected = self.settings.state.selected().unwrap_or(0).min(entries.len() - 1);
        self.settings.entries = entries;
        self.settings.state.select(Some(selected));
    }

    /// The ← route into adjustment: Some only when the cursor stands on a
    /// Settings value row, so Back keeps meaning back everywhere else.
    fn settings_step(&mut self, delta: i32) -> Option<Vec<Effect>> {
        if self.tab != Tab::Settings || *self.settings_node() != SettingsNode::Crossfade {
            return None;
        }
        match self.pane().selected() {
            Some(Entry::Setting { row, .. }) if *row != SettingRow::CrossfadeMenu => {
                Some(self.adjust_setting(delta))
            }
            _ => None,
        }
    }

    /// Move the selected value and tell the engine in the same keystroke.
    /// The announcement machinery is refreshed by the handle_action funnel
    /// this returns through, so a blend toggled on withdraws a standing
    /// seam in the same breath (the pending-seam rule).
    fn adjust_setting(&mut self, delta: i32) -> Vec<Effect> {
        let Some(Entry::Setting { row, .. }) = self.pane().selected() else {
            return Vec::new();
        };
        let effect = match row {
            SettingRow::BlendLength => {
                // Snap toward the pressed direction: a hand-written 4.5
                // steps to 5 and 4, never 5.5 forever.
                let snapped = if delta > 0 {
                    self.crossfade.floor() + 1.0
                } else {
                    self.crossfade.ceil() - 1.0
                };
                self.crossfade = snapped.clamp(0.0, 30.0);
                Effect::Audio(AudioCmd::SetCrossfade(self.crossfade))
            }
            SettingRow::Gapless => {
                self.gapless = !self.gapless;
                Effect::Audio(AudioCmd::SetGapless(self.gapless))
            }
            SettingRow::BlendSkips => {
                self.blend_skips = !self.blend_skips;
                Effect::Audio(AudioCmd::SetBlendSkips(self.blend_skips))
            }
            SettingRow::PauseFade => {
                self.pause_fade = !self.pause_fade;
                Effect::Audio(AudioCmd::SetPauseFade(self.pause_fade))
            }
            SettingRow::CrossfadeMenu => return Vec::new(),
        };
        self.refresh_settings_rows();
        vec![effect]
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
            self.discover_stack.enter(node);
            self.discover.set(entries);
            self.message = None;
            return Vec::new();
        }
        self.discover_stack.enter(node.clone());
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

    /// One row's media URL, by the same road [`App::play_index`] builds the
    /// one it plays. None only when the URL cannot be built at all, which
    /// play_index would have refused too.
    fn queue_url(&self, track: &Track) -> Option<String> {
        urls::media_url(&self.session.server, &track.filepath, self.session.token.as_deref()).ok()
    }

    /// The queue row whose media URL is `url`, if any. A scan, but of an
    /// in-memory queue, on an event that fires once per blend. Rows after
    /// the playing one are asked first: duplicate filepaths are legal, and
    /// adopting an earlier copy would walk the cursor backwards into
    /// tracks already heard (review finding: the fallback rewound).
    fn index_of_url(&self, url: &str) -> Option<usize> {
        let ahead = self.queue.current.map_or(0, |current| current + 1);
        (ahead..self.queue.items.len())
            .find(|&i| self.queue_url(&self.queue.items[i]).as_deref() == Some(url))
            .or_else(|| {
                self.queue.items[..ahead.min(self.queue.items.len())]
                    .iter()
                    .position(|t| self.queue_url(t).as_deref() == Some(url))
            })
    }

    /// The standing announcement, if everything it was made against still
    /// holds: crossfade still on, same playing row, same track at the
    /// announced index, same shuffle and repeat rules — and, for linear
    /// play, still the row the rules would pick. Linear picks are
    /// deterministic, so asking again costs nothing and catches what the
    /// snapshots cannot: a push that changes the answer without touching
    /// anything they see, like the repeat-all wrap when a track lands
    /// behind the last row (review finding: the new track was skipped).
    /// Shuffle is the one that must NOT be re-asked — that rolls the dice
    /// — which is the entire reason the announcement is held at all.
    fn announced_still_valid(&self) -> Option<usize> {
        let a = self.announced.as_ref()?;
        let holds = (self.crossfade > 0.0 || self.gapless)
            && self.queue.current == Some(a.for_current)
            && self.queue.shuffle == a.shuffle
            && self.queue.repeat == a.repeat
            && self.queue.items.get(a.index).is_some_and(|t| t.filepath == a.filepath)
            && (self.queue.shuffle || self.queue.next_index(false) == Some(a.index))
            // A seam announcement is only lawful in the mode that made it:
            // gapless, no blend. The repeat snapshot above cannot catch a
            // crossfade toggle, and a stale seam under a blend is a track
            // told to blend into itself.
            && (!a.self_seam || (self.gapless && self.crossfade <= 0.0));
        holds.then_some(a.index)
    }

    /// Roll the pick an announcement would carry, or None when nothing
    /// should follow: crossfade off, nothing playing, repeat-one (a track
    /// never blends into itself), or no next at all.
    fn pick_announcement(&self) -> Option<AnnouncedNext> {
        // The announcement serves both transitions: a blend needs it early,
        // gapless needs it to have something to append.
        if self.crossfade <= 0.0 && !self.gapless {
            return None;
        }
        let for_current = self.queue.current?;
        self.now_playing.as_ref()?;
        // The one self-transition that is WANTED: gapless repeat-one loops
        // its seam sample-tight, and its cursor never has to move — which
        // is exactly what makes it safe where the duplicate-rows case is
        // not (C4 review: the loop seam, the case gapless exists for,
        // always gapped).
        // Repeat-one, or its disguise: a one-track queue under repeat-all
        // loops exactly the same seam (the engine's candidate logic already
        // treats them alike; the fix-round review caught this side starving
        // the disguised case).
        let looping_alone = self.queue.repeat == Repeat::One
            || (self.queue.repeat == Repeat::All && self.queue.items.len() == 1);
        let self_seam = self.gapless && self.crossfade <= 0.0 && looping_alone;
        if self.queue.repeat == Repeat::One && !self_seam {
            return None;
        }
        let index = self.queue.next_index(false)?;
        if index == for_current && !self_seam {
            // A one-track queue under repeat-all: repeat-one in effect.
            return None;
        }
        let track = self.queue.items.get(index)?;
        // The same file queued twice in a row: a blend into it would change
        // nothing status can show — no HandedOver would ever fire, the
        // cursor would stall, and the missed-blend path would play the copy
        // again (review finding: duplicates played three times). Refuse,
        // and the ordinary TrackEnded road walks the cursor forward. The
        // seam is exempt: its cursor stays where it is.
        if !self_seam
            && self.now_playing.as_ref().is_some_and(|t| t.filepath == track.filepath)
        {
            return None;
        }
        Some(AnnouncedNext {
            for_current,
            index,
            filepath: track.filepath.clone(),
            url: self.queue_url(track)?,
            hint: track.metadata.duration,
            shuffle: self.queue.shuffle,
            repeat: self.queue.repeat,
            self_seam,
        })
    }

    /// Keep the engine's idea of what comes next in step with this queue.
    /// Runs after every action and event — dispatch is the one funnel that
    /// everything able to change the answer passes through — and is cheap
    /// on the quiet path: a standing announcement that still holds is left
    /// alone, which is also what keeps a shuffled pick from re-rolling.
    fn refresh_prepared(&mut self) -> Option<Effect> {
        if self.announced_still_valid().is_some() {
            return None;
        }
        match (self.announced.take(), self.pick_announcement()) {
            (None, None) => None,
            (Some(_), None) => Some(Effect::Audio(AudioCmd::ClearNext)),
            // Re-announcing an unchanged URL is a no-op engine-side, so a
            // pick that survived a re-roll costs nothing extra.
            (_, Some(next)) => {
                let effect = Effect::Audio(AudioCmd::PrepareNext {
                    url: next.url.clone(),
                    duration_hint: next.hint,
                });
                self.announced = Some(next);
                Some(effect)
            }
        }
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
        // Taken before the track moves into `now_playing`; the shape is
        // asked for by path, so nothing else about the track is needed.
        let filepath = track.filepath.clone();
        self.remember_played(&track);
        self.now_playing = Some(track);
        // Every Play wipes the engine's pending next (play_source clears
        // it), so whatever announcement stood is now this side's belief
        // alone. Drop it and the trailing refresh re-announces — free when
        // the pick is unchanged, since the engine treats a repeated URL as
        // a no-op. Without this, restarting the playing track left the
        // engine holding nothing while the app believed otherwise, and the
        // next transition silently lost its blend (review finding).
        self.announced = None;
        // And a seek aimed at the old track has nothing to say about this
        // one.
        self.seek_goal = None;

        let mut effects = vec![Effect::Audio(AudioCmd::Play { url, duration_hint: hint })];
        effects.extend(self.fetch_art());
        effects.extend(self.fetch_waveform(&filepath));
        effects.extend(self.maybe_autodj());
        effects
    }

    /// Ask for the cover of what just started, unless the cache already
    /// holds it — or already holds the placeholder a previous ask left, so
    /// skipping n-n-n through one album costs one request, not five.
    fn fetch_art(&mut self) -> Option<Effect> {
        let file = self.now_playing.as_ref()?.metadata.album_art.clone()?;
        if self.art.contains_key(&file) {
            return None;
        }
        if self.art.len() >= ART_CACHE_CAP {
            self.art.clear();
        }
        self.art.insert(file.clone(), None);
        Some(Effect::Api(ApiCmd::AlbumArt { file }))
    }

    /// Ask for a track's shape, unless the cache already holds it — or the
    /// placeholder a previous ask left, which is what stops the same track
    /// being asked for twice.
    ///
    /// Takes a filepath rather than reading `now_playing`, because the whole
    /// point is that it is also called for the track that has not started
    /// yet (see [`App::prefetch_waveform`]).
    fn fetch_waveform(&mut self, filepath: &str) -> Option<Effect> {
        if self.waveforms.contains_key(filepath) {
            return None;
        }
        if self.waveforms.len() >= ART_CACHE_CAP {
            self.waveforms.clear();
        }
        self.waveforms.insert(filepath.to_string(), None);
        Some(Effect::Api(ApiCmd::Waveform { filepath: filepath.to_string() }))
    }

    /// Ask for the *next* track's shape while this one is still playing.
    ///
    /// A waveform the server has not built before costs an ffmpeg decode —
    /// up to half a minute — so a shape fetched when the track starts can
    /// arrive well into it, and the bar visibly changes under the playhead.
    ///
    /// Deliberately **not** simply "whatever was announced". The
    /// announcement only exists when a blend or a gapless seam needs one
    /// (see [`App::pick_announcement`]), and someone listening with
    /// crossfade off is exactly the person who would notice a bar arriving
    /// late. So it falls back to the plain next row — except under shuffle,
    /// where the pick is rolled fresh on every call and prefetching would
    /// mean fetching the library one dispatch at a time. That is the whole
    /// reason the announcement is *held* rather than recomputed, and with
    /// no announcement to borrow there is nothing stable to ask for.
    fn prefetch_waveform(&mut self) -> Option<Effect> {
        // Only while something is actually on. With nothing playing there is
        // no "next" to be ahead of — the row after a stopped cursor is just
        // the top of the queue, and asking for it turns every keystroke on a
        // stopped player into a request.
        self.now_playing.as_ref()?;
        let next = match &self.announced {
            Some(next) => next.filepath.clone(),
            None if !self.queue.shuffle => {
                let index = self.queue.next_index(false)?;
                self.queue.items.get(index)?.filepath.clone()
            }
            None => return None,
        };
        self.fetch_waveform(&next)
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
        // Auto-advance plays what was announced whenever one stands. The
        // announcement is a commitment: even when the blend missed (this
        // path), honoring it keeps one roll of the shuffle dice per
        // transition instead of one per code path. Manual skips are the
        // user overruling all of that.
        if !manual {
            if let Some(index) = self.announced_still_valid() {
                return self.play_index(index);
            }
        }
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
        // Build on the seek still in flight, not the position it is about
        // to replace: status refreshes a few times a second, so a quick
        // `}}}` all read the same stale base and landed as one press.
        let base = match self.seek_goal {
            Some((goal, at)) if at.elapsed() < SEEK_CHAIN => goal,
            _ => self.status.position,
        };
        let mut target = (base + delta).max(0.0);
        // The bar's end is as far as chaining may bank; where exactly a
        // near-end landing stops is the engine's call (its transition
        // runway clamp), not something to duplicate here.
        if self.status.duration > 0.0 {
            target = target.min(self.status.duration);
        }
        self.seek_goal = Some((target, std::time::Instant::now()));
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
        let mut effects = self.consume(event);
        effects.extend(self.refresh_prepared());
        effects.extend(self.prefetch_waveform());
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
                // A seek goal has served once status catches up to it — or
                // once the source changes under it, where the old track's
                // target would steer presses on the new one.
                if let Some((goal, _)) = self.seek_goal {
                    if status.source != self.status.source || status.position >= goal - 1.0 {
                        self.seek_goal = None;
                    }
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
            Event::HandedOver { from, to } => {
                // A handover out of a track we are no longer on is stale —
                // the same rule TrackEnded lives by — and one that raced a
                // play the user just asked for is superseded: `starting`
                // covers the gap where status still names the old track
                // while the user's own pick is opening.
                if !self.is_current_source(&from) || self.starting.is_some() {
                    return Vec::new();
                }
                // The engine crossfaded into the announced track by itself.
                // Move the cursor the way play_index would — minus the Play,
                // which the audio has already performed.
                // The announcement already carries the exact URL it sent
                // the engine, so the happy path is one string compare —
                // no rebuilt URL (review note: the rebuild was waste).
                let adopted = self
                    .announced_still_valid()
                    .filter(|_| self.announced.as_ref().is_some_and(|a| a.url == to))
                    .or_else(|| self.index_of_url(&to));
                match adopted {
                    Some(index) => {
                        let track = self.queue.items[index].clone();
                        self.queue.start(index);
                        self.remember_played(&track);
                        self.now_playing = Some(track);
                        // Spent; the refresh at the end of this dispatch
                        // announces whatever follows the new track.
                        self.announced = None;
                        self.failures = 0;
                        let mut effects: Vec<Effect> = Vec::new();
                        effects.extend(self.fetch_art());
                        effects.extend(self.maybe_autodj());
                        effects
                    }
                    None => {
                        // The queue was edited under a blend already in the
                        // air, and the engine finished a plan this queue no
                        // longer describes. Put the right track on properly —
                        // a hard cut, once, in a race that takes deliberate
                        // timing to hit.
                        self.announced = None;
                        self.skip(false)
                    }
                }
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
            // Everything about who we are connected to and how goes
            // through one door into session.rs, which owns the connect
            // screen those replies land on (audit #60).
            event @ (Event::Connected { .. }
            | Event::ServersDiscovered(_)
            | Event::TunnelReady { .. }
            | Event::NeedsLogin { .. }
            | Event::Unauthorized) => self.consume_session(event),
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
                self.browse_undo = None;
                self.path = path.to_string();
                let root = self.browser_root().to_string();
                self.files.set(entries_from_listing(&listing, &root));
                Vec::new()
            }
            Event::Library { node, dest, data } => {
                // Drop a reply for a view the user has already navigated away
                // from, so a slow request can't overwrite the current screen.
                // Which drill answers depends on who asked — the Search tab
                // files library nodes under its own trail.
                let fresh = match dest {
                    Tab::Search => self.search_stack.wants(&SearchNode::Library(node)),
                    _ => self.library_stack.wants(&node),
                };
                if !fresh {
                    return Vec::new();
                }
                self.pane_for_mut(dest).set(entries_from_library(data));
                self.message = None;
                Vec::new()
            }
            Event::Discover { node, data, note } => {
                // Drop a reply for a view the user has already left, the same
                // rule the Library tab follows.
                if !self.discover_stack.wants(&node) {
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
            Event::AlbumArt { file, art } => {
                // Keyed by the server's own filename, an answer is never
                // stale: one that lands after the player has moved on just
                // means the next track off that album finds its cover
                // already here.
                self.art.insert(file, art);
                Vec::new()
            }
            // Same rule, and here it is the whole point: a shape asked for
            // ahead of the track lands while something else is still
            // playing, and is filed for when it starts.
            Event::Waveform { filepath, bars } => {
                self.waveforms.insert(filepath, bars);
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
            Event::PlaylistSaved { name, count } => self.consume_playlist_saved(name, count),
            Event::SearchResults { query, results } => {
                // Replies can pass each other now that each answers on its
                // own thread; only the search still standing in the box is
                // the one anybody is waiting for.
                if self.search_submitted.as_deref() != Some(query.as_str()) {
                    return Vec::new();
                }
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
                self.search_stack.restart();
                self.search.trail.clear();
                self.search.set(self.search_root_entries());
                self.message = None;
                Vec::new()
            }
            Event::TunnelPath(path) => {
                // The old bridge outlives a switch to a direct server (its
                // Drop would cut a session mid-handover), and its sampler
                // keeps reporting. A verdict about a tunnel this session is
                // not on belongs to nobody — without this, a direct URL
                // wore the last tunnel's badge (pre-merge review).
                if crate::quickconnect::is_tunnel_id(&self.session.server_id) {
                    self.tunnel_path = Some(path);
                }
                Vec::new()
            }
            Event::Error(e) => {
                self.connecting = false;
                self.connect.submitting = false;
                // The browse this error answers will never repaint the pane,
                // so its navigation must not stand: put the path back and
                // take the phantom column with it (see `browse_undo`).
                //
                // The rows come back too, off that same column — the way in
                // emptied the pane so the wait could show a spinner, and
                // without this a failed browse would leave the user staring
                // at "(empty directory)" for a folder that is not empty and
                // is not even the one they are standing in.
                if let Some((path, pushed)) = self.browse_undo.take() {
                    self.path = path;
                    // The spinner is already out: `clear_pending` runs ahead
                    // of every error, for exactly this reason.
                    if pushed && let Some(step) = self.files.trail.pop() {
                        self.files.entries = step.entries;
                        self.files.state.select(Some(step.chosen));
                    }
                }
                self.error(e);
                Vec::new()
            }
        }
    }
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
