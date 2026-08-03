//! TUI state and the rules that change it.
//!
//! Deliberately I/O-free: actions and worker events go in, state changes and
//! [`Effect`]s come out, and the run loop is the only thing that touches
//! channels. That keeps the interesting behaviour — navigation, queue
//! advancement, repeat/shuffle — testable without a terminal or a server.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;

use crate::api::types::{Album, DirListing, Genre, Track, TrackMetadata};
use crate::api::urls;
use crate::discovery::DiscoveredServer;
use crate::dj;
use crate::player::PlayerStatus;

use super::worker::{
    ApiCmd, AudioCmd, AutoDjMode, DjRequest, Event, LibraryData, LibraryNode,
};

const SEEK_STEP: f64 = 5.0;
const VOLUME_STEP: f32 = 0.05;

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
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Files, Tab::Library, Tab::Playlists, Tab::Search];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Files => "Files",
            Tab::Library => "Library",
            Tab::Playlists => "Playlists",
            Tab::Search => "Search",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
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
    VolumeUp,
    VolumeDown,
    RemoveFromQueue,
    ClearQueue,
    ToggleRepeat,
    ToggleShuffle,
    ToggleAutoDj,
    OpenDjPanel,
    StartSearch,
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
    Track { label: String, track: Box<Track> },
}

#[derive(Debug, Default)]
pub struct Pane {
    pub entries: Vec<Entry>,
    pub state: ListState,
}

impl Pane {
    pub fn set(&mut self, entries: Vec<Entry>) {
        // Start on the first real row rather than on "..", so entering a
        // folder and pressing Enter doesn't just walk back out of it.
        let selected = match entries.first() {
            None => None,
            Some(Entry::Parent) if entries.len() > 1 => Some(1),
            Some(_) => Some(0),
        };
        self.entries = entries;
        self.state.select(selected);
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
    pub playlists: Pane,
    pub playlist_open: Option<String>,
    pub search: Pane,
    pub query: String,
    pub editing_query: bool,
    pub search_summary: Option<String>,

    pub queue: Queue,
    /// What the connected server offers. Default (nothing) until a ping says
    /// otherwise, so an optional feature is never assumed present.
    pub capabilities: crate::api::types::Capabilities,
    pub autodj: AutoDjMode,
    /// How Auto-DJ chooses, beyond the mode.
    pub dj: dj::Settings,
    /// The settings panel, when it is open.
    pub dj_panel: Option<DjPanel>,
    /// Tracks played recently, newest first. Feeds both the sonic anchor and
    /// the artist cooldown, so the session anchors on where it has been
    /// rather than only on the song currently sounding.
    autodj_recent: Vec<Track>,
    /// Round-trip cursor the random-songs picker uses to avoid repeats.
    autodj_ignore: Vec<u32>,
    /// A request is in flight; don't pile on another.
    autodj_pending: bool,
    pub status: PlayerStatus,
    pub volume: f32,
    pub now_playing: Option<Track>,
    pub audio_available: bool,

    pub message: Option<Message>,
    pub show_help: bool,
    pub should_quit: bool,
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
            playlists: Pane::default(),
            playlist_open: None,
            search: Pane::default(),
            query: String::new(),
            editing_query: false,
            search_summary: None,
            queue: Queue::default(),
            capabilities: Default::default(),
            autodj: AutoDjMode::Off,
            dj: dj::Settings::default(),
            dj_panel: None,
            autodj_recent: Vec::new(),
            autodj_ignore: Vec::new(),
            autodj_pending: false,
            status: PlayerStatus::default(),
            volume: 1.0,
            now_playing: None,
            audio_available: true,
            message: None,
            show_help: false,
            should_quit: false,
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
        if !self.connected || self.editing_query {
            InputMode::Editing
        } else if self.dj_panel.is_some() {
            InputMode::Panel
        } else {
            InputMode::Normal
        }
    }

    pub fn pane(&self) -> &Pane {
        match self.tab {
            Tab::Files => &self.files,
            Tab::Library => &self.library,
            Tab::Playlists => &self.playlists,
            Tab::Search => &self.search,
        }
    }

    fn pane_mut(&mut self) -> &mut Pane {
        match self.tab {
            Tab::Files => &mut self.files,
            Tab::Library => &mut self.library,
            Tab::Playlists => &mut self.playlists,
            Tab::Search => &mut self.search,
        }
    }

    /// The library view currently on screen.
    pub fn library_node(&self) -> &LibraryNode {
        self.library_stack.last().unwrap_or(&LibraryNode::Root)
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
        // The connect screen swallows everything except quit.
        if !self.connected {
            return self.handle_connect_action(action);
        }
        // The panel is modal: it owns the arrow keys and the letters it uses,
        // so playback shortcuts can't fire while someone is editing settings.
        if self.dj_panel.is_some() {
            return self.handle_dj_action(action);
        }
        if self.editing_query {
            if let Some(effects) = self.handle_query_action(&action) {
                return effects;
            }
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
            Action::PageUp => self.move_selection(-10),
            Action::PageDown => self.move_selection(10),
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

            Action::StartSearch => {
                self.tab = Tab::Search;
                self.focus = Focus::Browser;
                self.editing_query = true;
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
                Some(vec![Effect::Api(ApiCmd::Search(query))])
            }
            _ => None,
        }
    }

    fn select_tab(&mut self, index: usize) -> Vec<Effect> {
        let Some(tab) = Tab::ALL.get(index).copied() else {
            return Vec::new();
        };
        self.tab = tab;
        self.focus = Focus::Browser;

        // Load a tab's contents the first time it's opened.
        match tab {
            Tab::Library if self.library_stack.is_empty() => {
                self.library_stack.push(LibraryNode::Root);
                self.library.set(library_root_entries());
                Vec::new()
            }
            Tab::Playlists if self.playlists.entries.is_empty() => {
                vec![Effect::Api(ApiCmd::Playlists)]
            }
            Tab::Search if self.search.entries.is_empty() && self.query.is_empty() => {
                self.editing_query = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn move_selection(&mut self, delta: isize) -> Vec<Effect> {
        match self.focus {
            Focus::Browser => self.pane_mut().move_by(delta),
            Focus::Queue => {
                if !self.queue.items.is_empty() {
                    let last = self.queue.items.len() as isize - 1;
                    let current = self.queue.state.selected().unwrap_or(0) as isize;
                    self.queue.state.select(Some((current + delta).clamp(0, last) as usize));
                }
            }
        }
        Vec::new()
    }

    fn activate(&mut self) -> Vec<Effect> {
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
                self.path = path.clone();
                vec![Effect::Api(ApiCmd::Browse(path))]
            }
            Entry::Node { node, label } => {
                self.library_stack.push(node.clone());
                self.library.set(Vec::new());
                self.info(format!("loading {label}…"));
                vec![Effect::Api(ApiCmd::Library(node))]
            }
            Entry::Playlist { name } => {
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

    fn go_back(&mut self) -> Vec<Effect> {
        match self.tab {
            Tab::Files => {
                if self.path.is_empty() {
                    return Vec::new();
                }
                let parent = match self.path.rsplit_once('/') {
                    Some((head, _)) => head.to_string(),
                    None => String::new(),
                };
                self.path = parent.clone();
                vec![Effect::Api(ApiCmd::Browse(parent))]
            }
            Tab::Library => {
                if self.library_stack.len() <= 1 {
                    return Vec::new(); // already at the mode menu
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
            _ => Vec::new(),
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
        if self.focus != Focus::Queue {
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
        match event {
            Event::Status(status) => {
                self.status = status;
                Vec::new()
            }
            Event::TrackEnded => self.skip(false),
            Event::AudioFailed(e) => {
                self.audio_available = false;
                self.error(format!("audio unavailable: {e}"));
                Vec::new()
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
                    Effect::Api(ApiCmd::Browse(self.path.clone())),
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
                self.files.set(entries_from_listing(&listing));
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
                let group_hits = results.artists.len() + results.albums.len();
                let mut tracks: Vec<Track> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for hit in results
                    .title
                    .iter()
                    .chain(results.files.iter())
                    .chain(results.lyrics.iter())
                {
                    if seen.insert(hit.filepath.clone()) {
                        tracks.push(Track {
                            filepath: hit.filepath.clone(),
                            metadata: hit.metadata.clone(),
                        });
                    }
                }
                self.search_summary = Some(if group_hits > 0 {
                    format!("{} tracks, {group_hits} artist/album matches", tracks.len())
                } else {
                    format!("{} tracks", tracks.len())
                });
                self.search.set(
                    tracks
                        .into_iter()
                        .map(|t| Entry::Track { label: t.display_name(), track: Box::new(t) })
                        .collect(),
                );
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

/// Join a directory prefix and an entry name into a library path.
fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

fn entries_from_listing(listing: &DirListing) -> Vec<Entry> {
    let prefix = listing.path.trim_matches('/');
    let mut entries = Vec::new();
    if !prefix.is_empty() {
        entries.push(Entry::Parent);
    }
    for dir in &listing.directories {
        entries.push(Entry::Dir {
            label: dir.name.clone(),
            path: qualify(prefix, &dir.name),
        });
    }
    for file in &listing.files {
        let filepath = qualify(prefix, &file.name);
        entries.push(Entry::Track {
            label: file.name.clone(),
            track: Box::new(Track { filepath, metadata: TrackMetadata::default() }),
        });
    }
    entries
}

/// Map a key press to an action. Editing mode routes printable keys to text
/// input; everything else is a normal binding.
pub fn map_key(key: KeyEvent, mode: InputMode) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c')) {
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

    // A modal overlay gets its own bindings rather than borrowing the
    // player's. Sharing them meant `p` reached the panel as "previous track"
    // and its sample key did nothing at all.
    if mode == InputMode::Panel {
        return match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Esc | KeyCode::Char('D') => Some(Action::Cancel),
            KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
            KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
            KeyCode::Left | KeyCode::Char('h' | '[') => Some(Action::Back),
            KeyCode::Right | KeyCode::Char('l' | ']') => Some(Action::SeekForward),
            KeyCode::Home | KeyCode::Char('g') => Some(Action::First),
            KeyCode::End | KeyCode::Char('G') => Some(Action::Last),
            KeyCode::Enter => Some(Action::Submit),
            KeyCode::Char(' ') => Some(Action::PlayPause),
            KeyCode::Char(c) => Some(Action::Input(c)),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('?') => Some(Action::ToggleHelp),
        KeyCode::Esc => Some(Action::Cancel),
        KeyCode::Tab => Some(Action::CycleFocus),
        KeyCode::Char(c @ '1'..='4') => Some(Action::SelectTab(c as usize - '1' as usize)),

        KeyCode::Char('j') | KeyCode::Down => Some(Action::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::Up),
        KeyCode::Char('g') | KeyCode::Home => Some(Action::First),
        KeyCode::Char('G') | KeyCode::End => Some(Action::Last),
        KeyCode::PageUp => Some(Action::PageUp),
        KeyCode::PageDown => Some(Action::PageDown),

        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Some(Action::Activate),
        KeyCode::Char('h') | KeyCode::Left => Some(Action::Back),
        KeyCode::Char('a') => Some(Action::AddToQueue),

        KeyCode::Char(' ') => Some(Action::PlayPause),
        KeyCode::Char('n') => Some(Action::NextTrack),
        KeyCode::Char('p') => Some(Action::PrevTrack),
        KeyCode::Char(']') => Some(Action::SeekForward),
        KeyCode::Char('[') => Some(Action::SeekBackward),
        KeyCode::Char('+' | '=') => Some(Action::VolumeUp),
        KeyCode::Char('-') => Some(Action::VolumeDown),

        KeyCode::Char('d') => Some(Action::RemoveFromQueue),
        KeyCode::Char('C') => Some(Action::ClearQueue),
        KeyCode::Char('r') => Some(Action::ToggleRepeat),
        KeyCode::Char('s') => Some(Action::ToggleShuffle),
        KeyCode::Char('A') => Some(Action::ToggleAutoDj),
        KeyCode::Char('D') => Some(Action::OpenDjPanel),
        KeyCode::Char('/') => Some(Action::StartSearch),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{DirEntry, FileEntry, SearchResults};

    fn track(path: &str) -> Track {
        Track { filepath: path.to_string(), metadata: TrackMetadata::default() }
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
                .map(|f| FileEntry { name: (*f).to_string(), kind: Some("mp3".into()) })
                .collect(),
        }
    }

    #[test]
    fn listing_becomes_entries_with_qualified_paths() {
        let entries = entries_from_listing(&listing("/lib/Artist/", &["Album"], &["song.mp3"]));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], Entry::Parent);
        assert!(matches!(&entries[1], Entry::Dir { path, .. } if path == "lib/Artist/Album"));
        assert!(
            matches!(&entries[2], Entry::Track { track, .. } if track.filepath == "lib/Artist/song.mp3")
        );
    }

    #[test]
    fn root_listing_has_no_parent_entry() {
        let entries = entries_from_listing(&listing("/", &["lib"], &[]));
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

        // Back up one level, then back to the root.
        let effects = app.handle_action(Action::Back);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse("lib".into()))]);
        let effects = app.handle_action(Action::Back);
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Browse(String::new()))]);
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
    fn search_results_dedupe_across_categories() {
        use crate::api::types::SearchTrack;
        let mut app = connected_app();
        let hit = |p: &str| SearchTrack {
            name: p.to_string(),
            filepath: p.to_string(),
            album_art_file: None,
            metadata: TrackMetadata::default(),
        };
        let results = SearchResults {
            artists: vec![Default::default()],
            albums: vec![],
            title: vec![hit("lib/a.mp3")],
            files: vec![hit("lib/a.mp3"), hit("lib/b.mp3")],
            lyrics: vec![],
        };
        app.apply_event(Event::SearchResults(Box::new(results)));

        assert_eq!(app.search.entries.len(), 2, "the same track is listed once");
        assert_eq!(app.search_summary.as_deref(), Some("2 tracks, 1 artist/album matches"));
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
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Playlists)]);
        assert!(app.playlist_open.is_none());
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
        assert_eq!(effects, vec![Effect::Api(ApiCmd::Library(LibraryNode::Artists))]);

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
