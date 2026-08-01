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
use crate::player::PlayerStatus;

use super::worker::{ApiCmd, AudioCmd, Event, LibraryData, LibraryNode};

const SEEK_STEP: f64 = 5.0;
const VOLUME_STEP: f32 = 0.05;

/// A side effect for the run loop to dispatch to a worker.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Audio(AudioCmd),
    Api(ApiCmd),
    /// Persist the session after a successful login.
    SaveSession,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Editing,
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

/// The connect screen, shown when there is no usable session.
#[derive(Debug, Default)]
pub struct ConnectForm {
    pub server: String,
    pub username: String,
    pub password: String,
    pub field: usize,
    pub submitting: bool,
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

pub struct App {
    pub server: String,
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
            status: PlayerStatus::default(),
            volume: 1.0,
            now_playing: None,
            audio_available: true,
            message: None,
            show_help: false,
            should_quit: false,
        };
        app.connect.server = server.unwrap_or_else(|| "http://localhost:3000".to_string());
        app
    }

    /// Effects to run at startup.
    pub fn start(&mut self) -> Vec<Effect> {
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
        match action {
            Action::Quit => {
                self.should_quit = true;
                vec![Effect::Audio(AudioCmd::Shutdown), Effect::Api(ApiCmd::Shutdown)]
            }
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
            Action::Submit => self.submit_connect(),
            _ => Vec::new(),
        }
    }

    fn submit_connect(&mut self) -> Vec<Effect> {
        let server = self.connect.server.trim().to_string();
        if server.is_empty() {
            self.error("enter a server URL");
            return Vec::new();
        }
        self.connecting = true;
        self.connect.submitting = true;
        self.message = None;

        // No username means the server is expected to be in public mode, where
        // every request authenticates anyway.
        if self.connect.username.trim().is_empty() {
            self.server = server.clone();
            return vec![Effect::Api(ApiCmd::Connect { server, token: None })];
        }
        vec![Effect::Api(ApiCmd::Login {
            server,
            username: self.connect.username.trim().to_string(),
            password: std::mem::take(&mut self.connect.password),
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
        self.now_playing = Some(track);
        vec![Effect::Audio(AudioCmd::Play { url, duration_hint: hint })]
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
            Event::Connected { server, username, token, ping } => {
                self.connected = true;
                self.connecting = false;
                self.connect.submitting = false;
                self.server = server;
                if token.is_some() {
                    self.token = token;
                }
                if username.is_some() {
                    self.username = username;
                }
                let libraries = ping.vpaths.len();
                self.info(format!(
                    "connected to {} ({} librar{})",
                    self.server,
                    libraries,
                    if libraries == 1 { "y" } else { "ies" }
                ));

                let mut effects = vec![
                    Effect::Api(ApiCmd::Browse(self.path.clone())),
                    Effect::Audio(AudioCmd::SetVolume(self.volume)),
                ];
                // Only worth persisting when we hold a token we logged in for.
                if self.token.is_some() && self.username.is_some() {
                    effects.push(Effect::SaveSession);
                }
                effects
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
            Event::Unauthorized => {
                self.connected = false;
                self.connecting = false;
                self.connect.submitting = false;
                self.connect.server = self.server.clone();
                self.token = None;
                self.error("not authorized — sign in again");
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
            KeyCode::Tab | KeyCode::Down => Some(Action::CycleFocus),
            KeyCode::Up => Some(Action::Up),
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

    fn connected_app() -> App {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
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
    fn connecting_without_a_username_uses_public_mode() {
        let mut app = App::new(None, None, None);
        app.connect.server = "http://host:3000".into();
        let effects = app.handle_action(Action::Submit);
        assert_eq!(
            effects,
            vec![Effect::Api(ApiCmd::Connect { server: "http://host:3000".into(), token: None })]
        );
    }

    #[test]
    fn connect_form_edits_the_focused_field_only() {
        let mut app = App::new(None, None, None);
        app.connect.server.clear();
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
