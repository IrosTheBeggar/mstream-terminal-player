//! The GUI player — the mouse-first surface the Windows/macOS installers
//! launch in the branded terminal window (`mstream-player gui`).
//!
//! Built the wizard's way: the kit's fixed palette and OSC 11 ground lease,
//! a `Surface` per frame, every action clickable AND keyed. Underneath it is
//! the SAME `App` and worker pair as the classic TUI — the GUI is a second
//! front end on the proven state machine (the wasm shell and the replay
//! harness are the other two), so queueing, crossfade announcements,
//! track-end advance and session handling are shared, not re-implemented.
//! Mouse clicks translate to the App's own actions (select the row, then
//! `Activate`), which keeps `handle_action`'s follow-up work — waveform
//! prefetch, the crossfade announcement — running exactly as the TUI's.
//!
//! The shell: the left nav's rooms (each a module of its own under this
//! one), the live queue panel, both bottom bars against real playback, and
//! the modals the rooms open. `MSTREAM_GUI_DEMO=1` seats a fixed track for
//! looking at the bars with no server at hand.
//!
//! Design: the "mStream Player GUI" canvas + docs/ui-kit.md.

mod albums;
mod bar;
mod actions;
mod cover;
mod dj;
mod library;
mod now;
mod playlists;
mod queue;
mod servers;
mod sonic;
mod torrent;
mod torrent_meta;

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use rust_i18n::t;

use crate::config::{self, Config};
use crate::kit::{
    GroundGuard, ListView, POINTER_RESET, Surface, dim, input_display, scroll_list, set_pointer_shape,
};
use crate::kit::theme::{self, legacy_conhost, th};
use crate::tui::app::{
    Action, App, Effect, Entry, MessageKind, SEARCH_CLASSES, SearchClass, SearchNode, Tab,
};
use crate::tui::worker::{AudioCmd, Event};
use crate::tui::{self, worker};

use bar::{BarView, Now};

/// Below this the layout has nowhere honest to put the bar. The installer's
/// own window is 100×30; anyone smaller is asked for more room, like the
/// wizard.
const MIN_W: u16 = 100;
const MIN_H: u16 = 24;

const POLL: Duration = Duration::from_millis(100);

// ── Actions ─────────────────────────────────────────────────────────────────

/// Everything a click or key can mean, for the whole surface.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Act {
    Nav(usize),
    ToggleQueue,
    PlayPause,
    Prev,
    Next,
    Shuffle,
    Repeat,
    AutoDj,
    VolDown,
    VolUp,
    /// A click on volume cell `i` sets the level to (i+1)/10.
    VolSet(u8),
    /// A click on a seek cell: the fraction of the track it means.
    Seek(f64),
    /// A browse pane's row, by PANE index (a class filter upstream maps
    /// clicks through), with the verb the click meant: the row itself —
    /// select and Activate, the TUI's Enter aimed by the mouse: open the
    /// folder, drill the class or the artist, or play from the track — or
    /// one of its hover verbs. One act for every list of the App's rows.
    PaneRow(List, usize, RowVerb),
    /// A list's scrollbar and wheel (kit `scroll_list`): a step, a jump.
    ScrollBy(List, i32),
    ScrollTo(List, usize),
    /// A class chip: put the chip cursor there and flip the class.
    Chip(usize),
    /// The query card: start (or resume) editing the search text.
    EditQuery,
    /// A top-bar tab: the Library, or Now Playing.
    Screen(Screen),
    /// A settings row, activated (click, Enter, Space).
    Row(usize),
    BlendDown,
    BlendUp,
    // ── Albums (see gui::albums) ────────────────────────────────────────
    /// Turn the album wall a page: -1 back, +1 forward.
    AlbPage(i32),
    /// The wall's letter strip: the visible album to bring the page to.
    AlbJump(usize),
    /// A cell on the current page, clicked: open that album.
    AlbCell(usize),
    // ── The browser bar (docs/ux-contracts/browser-top-bar.md) ──────────
    /// The bar's back ◂ — the crumb's way out, h's clickable twin.
    BarBack,
    /// Open the live list filter (the App's own prompt).
    BarFilter,
    /// Clear the filter — the [X], and Esc once typing is done.
    BarClear,
    /// The whole-list verbs, gated on the list holding playable rows.
    BarPlay,
    BarQueueAll,
    BarShuffle,
    // ── Playlists (see gui::playlists) ──────────────────────────────────
    /// The affirmative card: open the New-playlist name dialog.
    PlNew,
    /// A playlist row: select and drill into its tracks.
    PlRow(usize),
    /// The row's hover verbs — the record's ⋮ menu, worn inline.
    PlRename(usize),
    PlDelete(usize),
    /// The name dialog's action and the modals' way out.
    PlOk,
    PlCancel,
    /// The delete gate's destructive yes.
    PlConfirm,
    /// A queue row: click plays it, its hover [x] removes it.
    QueueRow(usize),
    QueueRemove(usize),
    /// Auto DJ's empty-queue chooser (contract clause 2) and the empty
    /// queue's openers (clause 16).
    DjChoose(usize),
    DjRemember,
    DjCancel,
    DjSurprise,
    DjPick,
    /// Track actions (track-actions contract): a pane row's, a queue row's
    /// or the playing card's sheet; its rows, its stars, its picker and
    /// Song info; the queue's grip and clear.
    More(crate::tui::app::Tab, usize),
    QueueMore(usize),
    QueueGrip(usize),
    QueueClear,
    NowMore,
    MoreKey,
    SheetVerb(actions::SheetAction),
    Rate(u32),
    SheetClose,
    PickPlaylist(String),
    PickNew,
    PickClose,
    InfoClose,
    /// The Library rooms (library-rooms contract): the strip's jump and
    /// the way back — the rows are [`Act::PaneRow`]s.
    LibJump(usize),
    LibBack,
    /// The Auto DJ room (auto-dj contract, clauses 40–53): its rows, bars,
    /// radios, chips, the keyword field, the genre and server pickers —
    /// starting with its one primary: Start through the toggle (the
    /// opening question included), Stop wherever the DJ is armed.
    DjStartStop,
    DjFocus(dj::Item),
    DjStep(crate::tui::app::DjRow, i32),
    DjSet(crate::tui::app::DjRow, u32),
    DjAnchor(crate::dj::SonicAnchor),
    DjEmpty(crate::dj::EmptyQueueStart),
    DjGenreMode(crate::dj::GenreMode),
    DjGenre(String),
    DjPickGenres,
    DjGenresClose,
    DjKeywordFocus,
    DjKeywordAdd,
    DjKeywordRemove(String),
    DjSource(String),
    DjPreview,
    DjServerPick,
    DjServer(usize),
    DjServerClose,
    // ── Sonic path (see gui::sonic) ─────────────────────────────────────
    /// A setup card's body or a results chip: open the pick-methods menu.
    SonMenu(crate::tui::app::SonicSide),
    /// The menu's rows — the three ways an end gets its song.
    SonUse(crate::tui::app::SonicSide),
    SonRandom(crate::tui::app::SonicSide),
    SonBrowse(crate::tui::app::SonicSide),
    /// The filled card's [X].
    SonClear(crate::tui::app::SonicSide),
    SonMenuClose,
    /// A length-bar cell, clicked: the stop count it means.
    SonLen(u32),
    /// Build the journey — also the results' Regenerate and the failure
    /// states' Retry, which are Build by other names.
    SonBuild,
    SonStartOver,
    /// A stop row: play the journey from there. The hover [+] queues it.
    SonRow(usize),
    SonQueueStop(usize),
    SonPlay,
    SonQueueAll,
    SonSave,
    /// The save prompt's button pair; typing forwards to the App's line.
    SonSaveOk,
    SonSaveCancel,
    // ── Servers (see gui::servers) ──────────────────────────────────────
    /// The header's server label: toggle the switcher dropdown.
    SrvMenu,
    /// The room's "Try again" on a server that would not answer.
    SrvRetry,
    /// Open the add-server form (the header [+], the dropdown's last row,
    /// the room's add row, the no-server screen's button).
    SrvAdd,
    SrvCloseDrop,
    /// Room rows: select, and the selected row's action words. Switch is
    /// also what a dropdown row means.
    SrvRow(usize),
    SrvSwitch(usize),
    SrvEdit(usize),
    SrvDefault(usize),
    SrvQr(usize),
    /// Opens the remove confirmation — for a saved server, and for a
    /// peer's record once its parent stopped listing it, where the word
    /// is Forget (contract clause 25); the bool answers it.
    SrvRemove(usize),
    SrvConfirm(bool),
    /// A federated peer's verbs: park it, offer it again (contract clauses
    /// 23–24).
    SrvHide(usize),
    SrvShow(usize),
    /// The form's fields, checkboxes and buttons.
    FormFocus(usize),
    FormToggle(usize),
    /// The chooser page's two ways in: 0 standard, 1 Quick Connect.
    FormMethod(usize),
    /// A discovered server's row: carry it to the standard page.
    FormPick(usize),
    /// One page back (closing from the chooser).
    FormBack,
    FormSubmit,
    FormCancel,
    QrClose,
    // ── Add torrent (see gui::torrent) ──────────────────────────────────
    /// The room's ◂ — back to the Settings rows (Esc's twin).
    TorBack,
    /// A form row, clicked: the keyboard cursor lands there.
    TorRow(torrent::Row),
    /// The library picker's arrows (and the name, which cycles).
    TorLib(i32),
    /// The native file dialog (falls back to the typed picker), the typed
    /// picker on purpose, drop the loaded file, the chip's two verbs.
    TorPick,
    TorType,
    TorUnload,
    TorHandOff,
    TorDetect,
    TorToggleRename,
    TorToggleForce,
    TorSubmit,
    /// The file picker's controls.
    TorPickerClose,
    TorPickerSuggest(usize),
    TorPickerScrollBy(i32),
    TorPickerScrollTo(usize),
    /// The partial-match picker: a location, download fresh, or back.
    TorMatch(usize),
    TorMatchFresh,
    TorMatchClose,
    /// The arrival chooser: add here, hand it on, the don't-ask box.
    TorChooseAdd,
    TorChooseHandOff,
    TorChooseAsk,
    TorChooseClose,
    /// A modal's whole-screen backdrop: swallow the click.
    Guard,
}

/// The lists the shell scrolls, one per viewport — the App's browse panes
/// as each room shows them, and the shell's own (the queue panel, the
/// sonic results, the DJ room's body and its genre picker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum List {
    Files,
    Search,
    Library,
    AlbumTracks,
    Playlists,
    PlaylistTracks,
    Queue,
    Sonic,
    DjRoom,
    DjGenres,
}

impl List {
    /// The App pane a list's rows live in — `None` for the shell's own
    /// lists, which carry no [`Act::PaneRow`].
    fn tab(self) -> Option<Tab> {
        match self {
            List::Files => Some(Tab::Files),
            List::Search => Some(Tab::Search),
            List::Library | List::AlbumTracks | List::Playlists | List::PlaylistTracks => Some(Tab::Library),
            List::Queue | List::Sonic | List::DjRoom | List::DjGenres => None,
        }
    }
}

/// The top bar's two screens: the Library — the nav column and its rooms —
/// and Now Playing, the playing track large. The queue panel and the bar
/// stand under both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Library,
    NowPlaying,
}

/// The sub-view a Settings room shows in place of its rows. At most one is
/// open, so the flags this replaces can no longer disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsRoom {
    Servers,
    Torrent,
}

/// What a click on a pane row meant: the row itself, or one of the hover
/// verbs the track-actions contract names (clause 32 of multi-server for
/// their meanings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowVerb {
    Open,
    Queue,
    Next,
    Now,
}

// ── Navigation ──────────────────────────────────────────────────────────────

/// The sidebar, in draw order — every row is a room of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavId {
    Files,
    Albums,
    Artists,
    Genres,
    Recent,
    Playlists,
    Search,
    Settings,
    /// Capability-gated, drawn under TOOLS but LAST of the digits: they
    /// assign by index, and a room that comes and goes with the server
    /// must never renumber the rooms that don't (contract §1).
    Sonic,
    /// The Auto DJ room (auto-dj contract, entry point 2): the first row of
    /// the TOOLS group, where the record's browse root and desktop rail
    /// keep it. The tenth room has no digit — `0` is the Now Playing
    /// screen — and answers `D`.
    Dj,
}

const NAV: [NavId; 10] = [
    NavId::Files,
    NavId::Albums,
    NavId::Artists,
    NavId::Genres,
    NavId::Recent,
    NavId::Playlists,
    NavId::Search,
    NavId::Settings,
    NavId::Sonic,
    NavId::Dj,
];

impl NavId {
    fn label(self) -> String {
        match self {
            NavId::Files => t!("gui.nav.files").to_string(),
            NavId::Albums => t!("gui.nav.albums").to_string(),
            NavId::Artists => t!("gui.nav.artists").to_string(),
            NavId::Genres => t!("gui.nav.genres").to_string(),
            NavId::Recent => t!("gui.nav.recent").to_string(),
            NavId::Playlists => t!("gui.nav.playlists").to_string(),
            NavId::Search => t!("gui.nav.search").to_string(),
            NavId::Settings => t!("gui.nav.settings").to_string(),
            NavId::Sonic => t!("gui.nav.sonic").to_string(),
            NavId::Dj => t!("gui.nav.dj").to_string(),
        }
    }
}

const FILES_NAV: usize = 0;
const ALBUMS_NAV: usize = 1;
const ARTISTS_NAV: usize = 2;
const GENRES_NAV: usize = 3;
const RECENT_NAV: usize = 4;
const SEARCH_NAV: usize = 6;
const PLAYLISTS_NAV: usize = 5;
const SETTINGS_NAV: usize = 7;
const SONIC_NAV: usize = 8;
const DJ_NAV: usize = 9;

/// A class's slot in [`SEARCH_CLASSES`] — the chip order.
fn class_idx(class: SearchClass) -> usize {
    SEARCH_CLASSES.iter().position(|c| *c == class).unwrap_or(0)
}

fn class_label(class: SearchClass) -> String {
    match class {
        SearchClass::Artists => t!("gui.class.artists").to_string(),
        SearchClass::Albums => t!("gui.class.albums").to_string(),
        SearchClass::Titles => t!("gui.class.titles").to_string(),
        SearchClass::Files => t!("gui.class.files").to_string(),
        SearchClass::Lyrics => t!("gui.class.lyrics").to_string(),
    }
}

// ── Settings rows ───────────────────────────────────────────────────────────

/// The Settings room's rows, by index: the crossfade group — the same
/// knobs the classic TUI's Settings tab drives, read from and written to
/// the shared App — then the servers doorway.
const ROW_BLEND: usize = 0;
const ROW_GAPLESS: usize = 1;
const ROW_BLEND_SKIPS: usize = 2;
const ROW_PAUSE_FADE: usize = 3;
/// The queue and the place in it come back on launch (contract clause 39).
const ROW_RESUME: usize = 4;
const ROW_MANAGE: usize = 5;
/// The torrents group: the Add-torrent doorway (the room, like Manage
/// servers) and the ask-me switch for torrents arriving from outside.
const ROW_TORRENT: usize = 6;
const ROW_ASK: usize = 7;
const ROW_HINTS: usize = 8;
const SET_ROWS: usize = 9;

/// Seconds of blend as a person reads them (the TUI's own spelling).
fn fmt_blend(seconds: f32) -> String {
    if seconds <= 0.0 {
        t!("gui.set.blend_off").to_string()
    } else if seconds.fract() == 0.0 {
        format!("{seconds:.0}s")
    } else {
        format!("{seconds:.1}s")
    }
}

// ── State ───────────────────────────────────────────────────────────────────

pub(crate) struct Gui {
    /// The shared player state machine — the same App the TUI drives.
    app: App,
    /// Effects the App handed back, waiting for the next dispatch.
    pending: Vec<Effect>,
    config: Config,
    /// A config that failed to LOAD is never written back — the wizard and
    /// player rule, kept here too.
    config_ok: bool,
    ui: Surface<Act>,
    /// Index into [`NAV`].
    active: usize,
    /// The Settings keyboard cursor: None is stowed (the kit's resting
    /// state — ↓ picks it up, Esc stows it).
    cursor: Option<usize>,
    queue_open: bool,
    /// One line above the bar: (text, is_error). Gui-local; the App's own
    /// message shows when this is empty.
    note: Option<(String, bool)>,
    /// `MSTREAM_GUI_DEMO=1`: a fixed track shown while the App is idle.
    demo: Option<Now>,
    demo_paused: bool,
    /// The Files and Search lists' viewports (the kit's table contract: a
    /// keyboard move reveals, the wheel scrolls freely).
    files_view: ListView,
    search_view: ListView,
    /// The chip cursor among the five class chips, and which classes the
    /// menu shows — the search params. The server answers every class in
    /// one reply, so the choice is instant and free to change.
    chip: usize,
    classes_on: [bool; 5],
    /// The queue panel's viewport, and the playing index last seen — the
    /// panel reveals the playing row only when it CHANGES, so the wheel
    /// can roam freely in between (the kit's table contract).
    queue_view: ListView,
    last_current: Option<usize>,
    /// The size of the last drawn frame, for hit zones the event loop
    /// needs outside a draw (the wheel's queue-vs-content split, the
    /// album wall's grid geometry).
    last_width: u16,
    last_height: u16,
    /// Which of the top bar's screens is up.
    screen: Screen,
    /// The Now Playing screen's own state: its cover slot.
    now: now::NowUi,
    /// The Settings sub-view standing in for its rows, when one is open.
    settings_room: Option<SettingsRoom>,
    /// The saved-server surfaces: dropdown, form, room, pairing QR.
    servers: servers::ServersUi,
    /// The album wall: its page, cell cursor, and per-slot cover caches.
    albums: albums::AlbumsUi,
    /// The queue panel: its per-row cover caches.
    queue: queue::QueueUi,
    /// The sonic path room: its menu, setup cursor and results wheel.
    sonic: sonic::SonicUi,
    /// The playlists room: its dialogs and the two levels' wheels.
    playlists: playlists::PlaylistsUi,
    /// The Add-torrent room: its form, picker, chooser and threads.
    torrent: torrent::TorrentUi,
    /// The Auto DJ room: its cursor, scroll, inputs and pickers.
    dj: dj::DjUi,
    /// The Library rooms: the list's scroll and the artist wall's state.
    library: library::LibraryUi,
    /// Track actions: the sheet, its picker and info, a grip drag.
    actions: actions::ActionsUi,
    /// The queue's highlighted row last drawn, so a new one is revealed.
    last_qsel: Option<usize>,
    /// The last frame left paced work unfinished (covers still waiting to
    /// upgrade to pixels): the event loop shortens its idle wait so the
    /// next frame comes promptly instead of a poll tick later.
    hot: bool,
}

impl Gui {
    fn new(config: Config, config_ok: bool, mut app: App) -> Self {
        // No connect screen here: the servers surfaces are this shell's own,
        // and the transport keeps working while a session is down.
        app.connect_screen = false;
        let mut ui = Surface::new();
        ui.key_hints = config.gui.key_hints;
        Gui {
            app,
            pending: Vec::new(),
            config,
            config_ok,
            ui,
            active: FILES_NAV,
            cursor: None,
            queue_open: true,
            note: None,
            demo: None,
            demo_paused: false,
            files_view: ListView::default(),
            search_view: ListView::default(),
            chip: 0,
            classes_on: [true; 5],
            queue_view: ListView::default(),
            last_current: None,
            last_width: MIN_W,
            last_height: MIN_H,
            screen: Screen::Library,
            now: now::NowUi::new(),
            settings_room: None,
            servers: servers::ServersUi::new(),
            albums: albums::AlbumsUi::new(),
            queue: queue::QueueUi::new(),
            sonic: sonic::SonicUi::new(),
            playlists: playlists::PlaylistsUi::new(),
            torrent: torrent::TorrentUi::new(),
            dj: dj::DjUi::new(),
            library: library::LibraryUi::new(),
            actions: actions::ActionsUi::new(),
            last_qsel: None,
            hot: false,
        }
    }

    fn pend(&mut self, effects: Vec<Effect>) {
        self.pending.extend(effects);
    }

    fn forward(&mut self, action: Action) {
        let effects = self.app.handle_action(action);
        self.pend(effects);
    }

    /// Forward an Activate that may answer an armed sonic pick — and when
    /// it does, follow the answer home to the room that asked (clause 12;
    /// the record re-pushes its screen the same way).
    fn forward_capturing(&mut self, action: Action) {
        let was_armed = matches!(self.app.capture, Some(crate::tui::app::Capture::Sonic(_)));
        self.forward(action);
        if was_armed && self.app.capture.is_none() {
            self.active = SONIC_NAV;
            self.app.tab = Tab::SonicPath;
        }
    }

    /// Settings changes persist as they happen — a GUI that loses a choice
    /// to a crash feels broken in a way a TUI never quite does.
    ///
    /// Loads fresh before writing: other flows save behind this copy's back
    /// (a connect's SaveSession touches the server list, the servers room
    /// edits it), and writing the boot-time copy wholesale would undo them.
    fn save_now(&mut self) {
        if !self.config_ok {
            return;
        }
        let mut config = match config::load() {
            Ok(config) => config,
            Err(e) => {
                self.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
                return;
            }
        };
        config.player.adopt(self.app.prefs());
        config.gui.key_hints = self.config.gui.key_hints;
        match config::save(&config) {
            Ok(()) => self.config = config,
            Err(e) => {
                self.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
            }
        }
    }

    /// The blend walks whole seconds and snaps toward the pressed direction
    /// (the TUI's rule: a hand-written 4.5 steps to 5 and 4, never 5.5).
    fn adjust_blend(&mut self, delta: i32) {
        let current = self.app.crossfade;
        let snapped = if delta > 0 { current.floor() + 1.0 } else { current.ceil() - 1.0 };
        self.app.crossfade = snapped.clamp(0.0, 30.0);
        let set = AudioCmd::SetCrossfade(self.app.crossfade);
        self.pend(vec![Effect::Audio(set)]);
        self.save_now();
    }

    fn adjust_row(&mut self, row: usize, delta: i32) {
        match row {
            // ← on a doorway row would "adjust" into the room; only an
            // activation (Enter, click, →) opens it.
            ROW_MANAGE if delta > 0 => servers::open_room(self),
            ROW_TORRENT if delta > 0 => torrent::open_room(self),
            ROW_ASK => {
                let ask = !self.config.torrent.ask;
                torrent::set_ask(self, ask);
            }
            ROW_BLEND => self.adjust_blend(delta),
            ROW_GAPLESS => {
                self.app.gapless = !self.app.gapless;
                let cmd = AudioCmd::SetGapless(self.app.gapless);
                self.pend(vec![Effect::Audio(cmd)]);
                self.save_now();
            }
            ROW_BLEND_SKIPS => {
                self.app.blend_skips = !self.app.blend_skips;
                let cmd = AudioCmd::SetBlendSkips(self.app.blend_skips);
                self.pend(vec![Effect::Audio(cmd)]);
                self.save_now();
            }
            ROW_PAUSE_FADE => {
                self.app.pause_fade = !self.app.pause_fade;
                let cmd = AudioCmd::SetPauseFade(self.app.pause_fade);
                self.pend(vec![Effect::Audio(cmd)]);
                self.save_now();
            }
            ROW_RESUME => {
                self.app.resume_queue = !self.app.resume_queue;
                self.save_now();
            }
            ROW_HINTS => self.set_key_hints(!self.config.gui.key_hints),
            _ => {}
        }
    }

    /// Show or hide the keyboard's names — the footer line and the tails
    /// on tooltips — and remember it in the shell's own section.
    fn set_key_hints(&mut self, on: bool) {
        self.config.gui.key_hints = on;
        self.ui.key_hints = on;
        self.save_now();
    }

    /// Volume set directly (the ten cells) — the one write that goes past
    /// `handle_action`, because a parameterized action has no place in the
    /// keymap's name tables. No follow-up work depends on volume, so the
    /// funnel loses nothing.
    fn set_volume(&mut self, volume: f32) {
        self.app.volume = volume.clamp(0.0, 1.0);
        let cmd = AudioCmd::SetVolume(self.app.volume);
        self.pend(vec![Effect::Audio(cmd)]);
    }

    /// Everything a click or key resolved to. Returns true to quit.
    pub(super) fn act(&mut self, act: Act) -> bool {
        if actions::act(self, &act) {
            return false;
        }
        if dj::act(self, &act) {
            return false;
        }
        if servers::act(self, &act) {
            return false;
        }
        if albums::act(self, &act) {
            return false;
        }
        if library::act(self, &act) {
            return false;
        }
        if sonic::act(self, &act) {
            return false;
        }
        if playlists::act(self, &act) {
            return false;
        }
        if torrent::act(self, &act) {
            return false;
        }
        match act {
            Act::Screen(screen) => {
                self.screen = screen;
                if screen == Screen::NowPlaying {
                    self.cursor = None;
                    self.servers.drop_open = false;
                }
            }
            Act::Nav(i) => {
                // A nav row is the Library's: it brings that screen back.
                self.screen = Screen::Library;
                // The gated room: with the flag gone the row isn't drawn,
                // and its digit must be as dead as the row (contract §1).
                if i == SONIC_NAV && !self.app.capabilities.discovery_path {
                    return false;
                }
                // A peer has no playlists to offer (contract clause 26): the
                // row is not drawn, and its digit is as dead as the row.
                if i == PLAYLISTS_NAV && self.app.session.peer.is_some() {
                    return false;
                }
                // A filter describes the list it was typed against —
                // leaving the room closes the prompt and lets the value
                // go (browser-top-bar clause 24, the nav half).
                if i != self.active {
                    self.app.filtering = false;
                    self.app.files.clear_filter();
                    self.app.library.clear_filter();
                }
                self.active = i;
                // Leaving for a section stows every servers surface; the
                // room is a Settings sub-view, not a place to come back to.
                self.servers.drop_open = false;
                self.settings_room = None;
                // The shell's own note was about the room being left.
                self.note = None;
                // The Library rooms open their root list fresh on every
                // visit (library-rooms contract, entry point 1).
                if let Some(root) = library::root_of(i)
                    && self.app.connected
                {
                    library::open(self, root);
                }
                if i != SETTINGS_NAV {
                    self.cursor = None;
                }
                // The Auto DJ room opens with its cursor stowed and, with
                // the DJ off, asks the session's server what it offers so
                // the rows fit it (auto-dj contract, clause 50).
                if i == DJ_NAV {
                    dj::open_room(self);
                }
                // The album wall: fetch the list on the first visit; a
                // return finds it standing (and a drill left open resumes
                // on its track list). The tab must point at Library either
                // way, so Activate on a track row lands on the right pane.
                if i == ALBUMS_NAV && self.app.connected {
                    if self.app.albums.is_none() {
                        let effects = self
                            .app
                            .open_library_node(crate::tui::worker::LibraryNode::Albums, true);
                        self.pend(effects);
                    } else {
                        self.app.tab = Tab::Library;
                    }
                }
                // A fresh visit to Search opens straight into the query box;
                // coming back to results leaves them standing.
                if i == SEARCH_NAV && self.app.connected {
                    if self.app.search_hits.is_none() {
                        self.forward(Action::StartSearch);
                    } else {
                        self.app.tab = Tab::Search;
                    }
                }
                // The sonic room keeps the App's tab honest for the shared
                // machinery (capture answers check the pane in focus).
                if i == SONIC_NAV {
                    self.app.tab = Tab::SonicPath;
                }
                // Playlists: a fresh visit fetches the list; a return finds
                // it (or a drilled playlist) standing — the albums pattern.
                if i == PLAYLISTS_NAV && self.app.connected {
                    let holding = matches!(
                        self.app.library_node(),
                        crate::tui::worker::LibraryNode::Playlists
                            | crate::tui::worker::LibraryNode::Playlist(_)
                    );
                    if holding {
                        self.app.tab = Tab::Library;
                    } else {
                        let effects = self
                            .app
                            .open_library_node(crate::tui::worker::LibraryNode::Playlists, true);
                        self.pend(effects);
                    }
                }
            }
            Act::ToggleQueue => self.queue_open = !self.queue_open,
            Act::PlayPause => {
                if self.app.now_playing.is_some() {
                    self.forward(Action::PlayPause);
                } else if self.demo.is_some() {
                    self.demo_paused = !self.demo_paused;
                }
            }
            Act::Prev => self.forward(Action::PrevTrack),
            Act::Next => self.forward(Action::NextTrack),
            Act::Shuffle => self.forward(Action::ToggleShuffle),
            Act::Repeat => self.forward(Action::ToggleRepeat),
            Act::AutoDj => self.forward(Action::ToggleAutoDj),
            Act::VolDown => self.set_volume(self.app.volume - 0.05),
            Act::VolUp => self.set_volume(self.app.volume + 0.05),
            Act::VolSet(i) => self.set_volume((i as f32 + 1.0) / 10.0),
            Act::Seek(frac) => {
                if self.app.now_playing.is_some() {
                    let duration = self.bar_now().map_or(0.0, |n| n.duration);
                    let effects = self.app.seek_to(frac * duration);
                    self.pend(effects);
                } else if let Some(demo) = &mut self.demo {
                    demo.elapsed = frac * demo.duration;
                }
            }
            Act::BarBack => {
                self.app.tab = self.browse_tab();
                self.forward(Action::Back);
            }
            Act::BarFilter => {
                self.app.tab = self.browse_tab();
                self.forward(Action::StartFilter);
            }
            Act::BarClear => {
                self.app.tab = self.browse_tab();
                if self.app.filtering {
                    self.forward(Action::Cancel);
                } else {
                    self.app.pane_mut().clear_filter();
                }
            }
            Act::BarPlay => {
                self.app.tab = self.browse_tab();
                let effects = self.app.play_listing(false);
                self.pend(effects);
            }
            Act::BarShuffle => {
                self.app.tab = self.browse_tab();
                let effects = self.app.play_listing(true);
                self.pend(effects);
            }
            Act::BarQueueAll => {
                self.app.tab = self.browse_tab();
                let effects = self.app.queue_listing();
                self.pend(effects);
            }
            Act::PaneRow(list, i, verb) => {
                self.aim(list, i);
                match verb {
                    RowVerb::Open => self.forward_capturing(Action::Activate),
                    RowVerb::Queue => self.forward(Action::AddToQueue),
                    RowVerb::Next => self.forward(Action::AddNext),
                    RowVerb::Now => self.forward(Action::PlayNow),
                }
            }
            Act::ScrollBy(list, delta) => self.list_mut(list).step(delta),
            Act::ScrollTo(list, first) => self.list_mut(list).scroll = first,
            Act::QueueRow(i) => {
                // A click plays the row and hands the panel the keys
                // (track-actions contract, clauses 17 and 19).
                self.app.focus = crate::tui::app::Focus::Queue;
                self.app.queue.state.select(Some(i));
                let effects = self.app.play_index(i);
                self.pend(effects);
            }
            Act::QueueRemove(i) => {
                let effects = self.app.remove_queue_row(i);
                self.pend(effects);
            }
            Act::Chip(i) => {
                self.chip = i;
                self.classes_on[i] = !self.classes_on[i];
            }
            Act::EditQuery => {
                if self.app.connected {
                    self.forward(Action::StartSearch);
                }
            }
            // Activation is the TUI's Enter: toggles flip, the blend steps
            // up, radios choose.
            Act::Row(i) => self.adjust_row(i, 1),
            Act::BlendDown => self.adjust_blend(-1),
            Act::BlendUp => self.adjust_blend(1),
            // The servers acts were consumed by servers::act above.
            _ => {}
        }
        false
    }

    /// The bar's view of what is playing: the App's track, timestamps and
    /// waveform — or the demo seat while the App is idle. Computed when
    /// asked: nothing in it outlives the frame.
    fn bar_now(&self) -> Option<Now> {
        match &self.app.now_playing {
            Some(track) => {
                let duration = if self.app.status.duration > 0.0 {
                    self.app.status.duration
                } else {
                    track.metadata.duration.unwrap_or(0.0)
                };
                // A restored queue shows its place before anything plays
                // (contract clause 40): the spot, paused.
                let elapsed = match self.app.resume_spot {
                    Some((_, position)) if self.app.status.is_idle() => position,
                    _ => self.app.status.position,
                };
                let m = &track.metadata;
                Some(Now {
                    title: track.title_or_file().to_string(),
                    artist: m.artist.clone().unwrap_or_default(),
                    album: m.album.clone().unwrap_or_default(),
                    elapsed,
                    duration,
                    year: m.year.filter(|y| *y > 0),
                    spec: actions::spec_parts(m).join(" · "),
                    // The App's copy first: an optimistic rating lands here
                    // the moment it is given (track-actions clause 11).
                    rating: self.app.rating_of(&track.filepath).or(m.rating),
                    key: m.musical_key.clone(),
                    bpm: m.bpm,
                })
            }
            None => self.demo.clone(),
        }
    }

    fn bar_paused(&self) -> bool {
        if self.app.now_playing.is_some() {
            self.app.status.paused || (self.app.status.is_idle() && self.app.resume_spot.is_some())
        } else {
            self.demo_paused
        }
    }
}

/// The demo seat (`MSTREAM_GUI_DEMO=1`): a fixed track shown while nothing
/// real is playing, so the bar can be seen and the seek ridden with no
/// server at hand. Same fiction as the design canvas.
fn demo_now() -> Now {
    Now {
        title: "Cassini IV".to_string(),
        artist: "Vela — Cassini".to_string(),
        album: "Cassini".to_string(),
        elapsed: 47.0,
        duration: 302.0,
        year: Some(2019),
        spec: "FLAC · 912 kbps · 44.1 kHz".to_string(),
        rating: Some(8),
        key: Some("8A".to_string()),
        bpm: Some(120),
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// One run of text at a cell, clipped at the frame's edge — written into
/// the buffer directly: the hub's primitive runs a few hundred times a
/// frame, and a `Paragraph` per call was a handful of allocations each.
/// The top bar's tabs at the left: the kit's tab slab for the screen that
/// is up, dim text for the other, bright under the pointer.
fn draw_top_tabs(frame: &mut Frame, gui: &mut Gui) {
    let mut x = 1;
    for (screen, label) in [(Screen::Library, t!("gui.top.library")), (Screen::NowPlaying, t!("gui.top.now"))] {
        let text = format!(" {label} ");
        let rect = Rect { x, y: 0, width: text.chars().count() as u16, height: 1 };
        let style = if gui.screen == screen {
            sel().add_modifier(Modifier::BOLD)
        } else if gui.ui.hovers(rect) {
            bright_bold()
        } else {
            dim()
        };
        put(frame, x, 0, &text, style);
        gui.ui.click(rect, Act::Screen(screen));
        x += rect.width + 1;
    }
}

fn put(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    let buf = frame.buffer_mut();
    if !buf.area.contains(Position { x, y }) {
        return;
    }
    let width = text.chars().count();
    buf.set_stringn(x, y, text, width, style);
}

fn bright_bold() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
}

/// The shell's glyph pairs, by terminal: the fancy form, or the CP437
/// stand-in legacy conhost can draw (see `setup::g`).
fn forward_glyph() -> &'static str {
    if legacy_conhost() { ">" } else { "▸" }
}

fn back_glyph() -> &'static str {
    if legacy_conhost() { "<" } else { "◂" }
}

/// The checkbox pair: checked, unchecked.
fn check_glyphs() -> (&'static str, &'static str) {
    if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") }
}

/// A 1-row text button: dim at rest, bright under the pointer; the
/// accent when it is the row's one way forward. Returns its width.
fn text_button(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, label: &str, lead: bool, act: Act) -> u16 {
    let rect = Rect { x, y, width: label.chars().count() as u16, height: 1 };
    let hover = gui.ui.hovers(rect);
    let style = match (hover, lead) {
        (true, _) => bright_bold(),
        (false, true) => accent(),
        (false, false) => dim(),
    };
    put(frame, x, y, label, style);
    gui.ui.click(rect, act);
    rect.width
}

fn sel() -> Style {
    Style::default().bg(th().accent).fg(th().on_accent)
}

fn accent() -> Style {
    Style::default().fg(th().accent)
}

/// The content column's rect for a frame this size — between the nav rule
/// and the queue (when open). One computation, shared by the draw and by
/// the key handling that must agree with it about geometry.
/// The content column: from under the header to the row above the bar's
/// seek line, and one row shorter when the screen keeps its tips line.
fn content_rect(width: u16, height: u16, queue_open: bool, footer: bool) -> Rect {
    let right = if queue_open { width - 36 } else { width - 3 };
    Rect { x: 17, y: 2, width: right - 17, height: height - 2 - bar::BAR_ROWS - u16::from(footer) }
}

/// A path clipped LEADING, so the leaf stays visible (the kit's path law:
/// ten identical prefixes say nothing).
fn clip_lead(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let tail: String = chars[chars.len() - max.saturating_sub(1)..].iter().collect();
    let mark = if legacy_conhost() { '»' } else { '…' };
    format!("{mark}{tail}")
}

/// One frame. Public to the crate so render tests can drive it.
pub(crate) fn render(frame: &mut Frame, gui: &mut Gui) {
    gui.ui.begin_frame();
    gui.hot = false; // this frame's draws re-raise it if work remains
    let area = frame.area();
    gui.last_width = area.width;
    gui.last_height = area.height;
    if let Some(ground) = th().ground.filter(|_| theme::ground_owned()) {
        frame.render_widget(
            ratatui::widgets::Block::default().style(Style::default().bg(ground).fg(th().text)),
            area,
        );
    }
    if area.width < MIN_W || area.height < MIN_H {
        frame.render_widget(Paragraph::new(t!("resize").to_string()).style(dim()), area);
        return;
    }

    draw_top_tabs(frame, gui);
    servers::draw_header(frame, gui, area);

    match gui.screen {
        Screen::Library => {
            draw_nav(frame, gui, area);
            // The content column, between the nav rule and the queue (when
            // open). Exhaustive over the nav, like the wheel: a room cannot
            // ship without a body.
            let content = content_rect(area.width, area.height, gui.queue_open, gui.footer());
            match NAV[gui.active] {
                NavId::Files => draw_files(frame, gui, content),
                NavId::Albums => albums::draw(frame, gui, content),
                NavId::Artists | NavId::Genres | NavId::Recent => library::draw(frame, gui, content),
                NavId::Search => draw_search(frame, gui, content),
                NavId::Settings => draw_settings(frame, gui, content),
                NavId::Sonic => sonic::draw(frame, gui, content),
                NavId::Playlists => playlists::draw(frame, gui, content),
                NavId::Dj => dj::draw_room(frame, gui, content),
            }
        }
        Screen::NowPlaying => {
            let stage = now::stage_rect(area, gui.queue_open, gui.footer());
            now::draw(frame, gui, stage);
        }
    }

    if gui.queue_open {
        queue::draw(frame, gui, area);
    }

    // The note rides the bar's bottom row (gui's own first, else the App's
    // words); the keyboard tips, when shown, take the very last row. An
    // armed pick outranks both: the banner is the mode, not news (clauses
    // 4, 10–13).
    let note = bar::note_rect(area, gui.footer());
    let pick_banner = match gui.app.capture {
        Some(crate::tui::app::Capture::Sonic(crate::tui::app::SonicSide::Start)) => {
            Some(t!("gui.sonic.pick_banner_start").to_string())
        }
        Some(crate::tui::app::Capture::Sonic(crate::tui::app::SonicSide::End)) => {
            Some(t!("gui.sonic.pick_banner_end").to_string())
        }
        _ => dj::banner(gui),
    };
    if let Some(banner) = pick_banner {
        put(
            frame,
            note.x,
            note.y,
            &bar::clip(&banner, note.width as usize),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        );
    } else if let Some((text, is_err)) = gui.note.clone().or_else(|| {
        gui.app
            .message
            .as_ref()
            .map(|m| (m.text.clone(), matches!(m.kind, MessageKind::Error)))
    }) {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        put(frame, note.x, note.y, &bar::clip(&text, note.width as usize), style);
    }
    let tips = if let Some(tip) = actions::tips(gui) {
        std::borrow::Cow::from(tip)
    } else if let Some(tip) = dj::tips(gui) {
        std::borrow::Cow::from(tip)
    } else if gui.servers.modal_open() {
        t!("gui.tips.form")
    } else if gui.torrent.modal_open() {
        std::borrow::Cow::from(torrent::tips(gui))
    } else if sonic::modal_open(gui) {
        std::borrow::Cow::from(sonic::tips(gui))
    } else if playlists::modal_open(gui) {
        std::borrow::Cow::from(playlists::tips(gui))
    } else if matches!(gui.app.capture, Some(crate::tui::app::Capture::Sonic(_))) {
        t!("gui.tips.sonic_pick")
    } else if gui.screen == Screen::NowPlaying {
        t!("gui.tips.now")
    } else if gui.in_settings_room(SettingsRoom::Servers) {
        // The bundled server's row has no remove key to name; a peer's row
        // has its own verbs.
        if servers::cursor_on_bundled(gui) {
            t!("gui.tips.servers_bundled")
        } else {
            match servers::cursor_peer_state(gui) {
                Some(true) => t!("gui.tips.servers_peer_missing"),
                Some(false) => t!("gui.tips.servers_peer"),
                None => t!("gui.tips.servers"),
            }
        }
    } else if gui.in_settings_room(SettingsRoom::Torrent) {
        std::borrow::Cow::from(torrent::tips(gui))
    } else {
        match gui.active {
            SETTINGS_NAV if gui.cursor.is_some() => t!("gui.tips.rows"),
            FILES_NAV if gui.app.filtering => t!("gui.tips.filter"),
            FILES_NAV => t!("gui.tips.files"),
            ARTISTS_NAV | GENRES_NAV | RECENT_NAV if gui.app.connected => library::tips(gui),
            ALBUMS_NAV if gui.app.connected => {
                if matches!(
                    gui.app.library_stack.here(),
                    crate::tui::worker::LibraryNode::Album { .. }
                ) {
                    t!("gui.tips.album_tracks")
                } else {
                    t!("gui.tips.albums")
                }
            }
            SEARCH_NAV if gui.app.editing_query => t!("gui.tips.search_edit"),
            SEARCH_NAV => t!("gui.tips.search"),
            SONIC_NAV => std::borrow::Cow::from(sonic::tips(gui)),
            PLAYLISTS_NAV if gui.app.connected => {
                std::borrow::Cow::from(playlists::tips(gui))
            }
            _ => t!("gui.tips.base"),
        }
    };
    // The footer of keys only when asked for: this surface is the
    // pointer's, and the classic TUI is the keyboard's room.
    if gui.config.gui.key_hints {
        put(frame, 1, area.height - 1, &tips, dim());
    }

    // While the pairing QR is up, the card cover stands down: the graphics
    // encode cache holds ONE image, and two per frame thrash it.
    let has_art = playing_cover_ready(&gui.app) && gui.servers.qr.is_none();
    let now = gui.bar_now();
    let view = BarView {
        now: now.as_ref(),
        paused: gui.bar_paused(),
        volume: gui.app.volume,
        shuffle: gui.app.queue.shuffle,
        repeat: gui.app.queue.repeat != crate::tui::app::Repeat::Off,
        autodj: gui.app.dj_armed(),
        queue_open: gui.queue_open,
        has_art,
        footer: gui.footer(),
    };
    bar::draw(frame, &mut gui.ui, area, &view);
    if has_art {
        draw_card_cover(frame, bar::cover_rect(area, gui.footer()), &mut gui.app);
    }

    // Overlays draw (and register) last, so their rects win the pointer.
    playlists::draw_modals(frame, gui, area);
    sonic::draw_modals(frame, gui, area);
    torrent::draw_modals(frame, gui, area);
    servers::draw_dropdown(frame, gui, area);
    servers::draw_modals(frame, gui, area);
    dj::draw_modals(frame, gui, area);
    actions::draw_modals(frame, gui, area);

    // The tooltip draws over everything, once the dwell matures — the
    // wizard's order.
    if let Some((target, text)) = gui.ui.ripe_tooltip() {
        let footprint = crate::kit::draw_tooltip(frame, area, target, text);
        gui.ui.overlay(footprint);
    }
}

/// Whether the playing track's cover is decoded and waiting in the cache.
fn playing_cover_ready(app: &App) -> bool {
    app.now_playing
        .as_ref()
        .and_then(|track| track.metadata.album_art.as_deref())
        .and_then(|file| app.art.get(file))
        .is_some_and(|art| art.is_some())
}

/// The card's album art: real pixels through the graphics probe where the
/// terminal can (kitty · sixel · iTerm2), the ▀-mosaic everywhere else —
/// the same two paths the TUI's facts column walks. The kit's rule holds:
/// pixels are for album art only, never chrome.
fn draw_card_cover(frame: &mut Frame, rect: Rect, app: &mut App) {
    // Field by field, the way the TUI spells it: the art cache's borrow
    // must be visibly disjoint from the graphics and cover-pane fields
    // taken mutably below.
    let cover = app
        .now_playing
        .as_ref()
        .and_then(|track| track.metadata.album_art.as_deref())
        .and_then(|file| app.art.get(file))
        .and_then(|art| art.as_ref());
    let Some(cover) = cover else {
        return;
    };
    if app.graphics.draw(frame, rect, cover) {
        return;
    }
    let mut canvas = crate::tui::canvas::Canvas::new(rect);
    if !canvas.is_empty() {
        app.cover_pane.draw(&mut canvas, cover);
        frame.render_widget(Paragraph::new(canvas.into_lines()), rect);
    }
}

fn draw_nav(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    put(frame, 1, 4, &t!("gui.nav.library"), dim());
    // The tools, the record's desktop-rail group: Auto DJ, then the
    // capability-gated sonic room.
    put(frame, 1, 13, &t!("gui.nav.tools"), dim());
    let forward = forward_glyph();
    // The Settings row sits on the content's last row, above the bar.
    let set_y = content_rect(area.width, area.height, gui.queue_open, gui.footer()).bottom() - 1;
    for (i, id) in NAV.iter().enumerate() {
        // The sonic room rides the ping's flag: absent is absent — no
        // placeholder row, and digit 9 goes dead with it (contract §1).
        if i == SONIC_NAV && !gui.app.capabilities.discovery_path {
            continue;
        }
        if i == PLAYLISTS_NAV && gui.app.session.peer.is_some() {
            continue;
        }
        let y = match i {
            FILES_NAV => 2,
            1..=5 => 4 + i as u16,
            SEARCH_NAV => 11,
            DJ_NAV => 14,
            SONIC_NAV => 15,
            _ => set_y,
        };
        let label = id.label();
        let active = i == gui.active;
        let x = if active { 1 } else { 3 };
        let text = if active { format!("{forward} {label}") } else { label };
        // The Auto DJ row wears the DJ's live state, the record's card
        // line: a dot in the ok colour while it is armed anywhere.
        let dot = (i == DJ_NAV && gui.app.dj_armed()).then_some(" •");
        let text_w = text.chars().count() as u16;
        let width = text_w + dot.map_or(0, |d| d.chars().count() as u16);
        let rect = Rect { x, y, width, height: 1 };
        let hover = gui.ui.hovers(rect);
        let style = match (active, hover) {
            (true, _) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => dim(),
        };
        put(frame, x, y, &text, style);
        if let Some(dot) = dot {
            put(frame, x + text_w, y, dot, Style::default().fg(th().ok));
        }
        gui.ui.click(rect, Act::Nav(i));
    }
    // The rule between the nav and the content runs down to the bar.
    for y in 2..bar::top(area, gui.footer()) {
        put(frame, 15, y, "│", dim());
    }
}

/// The Files browser: the App's Files pane, drawn kit-style. Clicking a
/// row is the TUI's Enter aimed by the mouse; the hovered track row
/// reveals a [+] that queues just that one.
fn draw_files(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if gui.app.connected {
        draw_files_bar(frame, gui, content);
    } else {
        put(frame, content.x, content.y, &t!("gui.nav.files"), dim());
    }

    if !gui.app.connected {
        servers::draw_disconnected(frame, gui, content, 2);
        return;
    }
    if gui.app.files.loading {
        put(frame, content.x, content.y + 3, &t!("busy.listing"), accent());
        return;
    }

    let entries = &gui.app.files.entries;
    if entries.is_empty() {
        put(frame, content.x, content.y + 3, &t!("gui.files.empty"), dim());
        return;
    }

    let list = Rect {
        x: content.x,
        y: content.y + 3,
        width: content.width - 2,
        height: content.height - 3,
    };
    let selected = gui.app.files.state.selected();
    let (first, visible) = gui.files_view.window(entries.len(), selected, list.height as usize);

    let len = entries.len();
    let rows: Vec<(usize, &Entry)> =
        entries.iter().enumerate().skip(first).take(visible).collect();
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &rows,
        list,
        selected,
        List::Files,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::ScrollBy(List::Files, -1),
        Act::ScrollBy(List::Files, 1),
        |first| Act::ScrollTo(List::Files, first),
    );
}

/// The honest count for a browse bar's first line: the room's own plain
/// wording, or `n of m` while a filter narrows the view
/// (docs/ux-contracts/browser-top-bar.md, clause 3).
fn bar_count(gui: &Gui, plain: String) -> String {
    let pane = gui.app.pane();
    if (gui.app.filtering && gui.browse_room()) || !pane.filter.is_empty() {
        let (shown, total) = pane.counts();
        t!("gui.bar.of", shown = shown, total = total).to_string()
    } else {
        plain
    }
}

/// The bar's back ◂ at (x, y). Returns how far the crumb moves over.
fn draw_bar_back(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16) -> u16 {
    let back = Rect { x, y, width: 1, height: 1 };
    let hover = gui.ui.hovers(back);
    let glyph = back_glyph();
    put(frame, x, y, glyph, if hover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::BarBack);
    gui.ui.tip_keyed(back, t!("gui.bar.back_tip").to_string());
    2
}

/// The bar's controls line, shared by every browse room: the filter on the
/// LEFT — the affordance, the standing chip, or (while typing) the whole
/// line as the field — and the whole-list verbs on the right, gated on the
/// pane holding playable rows (clauses 2, 13, 20–23).
fn draw_bar_controls(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let filtering = gui.app.filtering && gui.browse_room();
    let filter = gui.app.pane().filter.clone();

    // The [X] that clears the filter, shared by the field and the chip.
    let close = |frame: &mut Frame, gui: &mut Gui, x: u16| {
        let rect = Rect { x, y, width: 3, height: 1 };
        let hover = gui.ui.hovers(rect);
        put(frame, x, y, "[X]", if hover { bright_bold() } else { dim() });
        gui.ui.click(rect, Act::BarClear);
        gui.ui.tip_keyed(rect, t!("gui.bar.clear_tip").to_string());
    };

    if filtering {
        // The line is the field (clause 20): live narrowing, nothing to
        // submit — Enter keeps the narrowed list, Esc lets go of it.
        put(frame, content.x, y, "/", accent().add_modifier(Modifier::BOLD));
        let close_x = content.right().saturating_sub(4);
        close(frame, gui, close_x);
        let width = close_x.saturating_sub(content.x + 3);
        put(
            frame,
            content.x + 2,
            y,
            &input_display(&filter, filter.chars().count(), width),
            Style::default(),
        );
        return;
    }

    // Left: the filter — a standing one wears its chip (clickable to keep
    // typing; the prompt reopens on what was typed), an idle one its
    // affordance.
    if !filter.is_empty() {
        let chip = format!("/ {}", bar::clip(&filter, 24));
        let rect = Rect { x: content.x, y, width: chip.chars().count() as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        put(
            frame,
            content.x,
            y,
            &chip,
            if hover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) },
        );
        gui.ui.click(rect, Act::BarFilter);
        close(frame, gui, rect.right() + 1);
    } else {
        let label = format!("/ {}", t!("gui.bar.filter"));
        let rect = Rect { x: content.x, y, width: label.chars().count() as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        put(frame, content.x, y, &label, if hover { bright_bold() } else { dim() });
        gui.ui.click(rect, Act::BarFilter);
        gui.ui.tip_keyed(rect, format!("{label} — f"));
    }

    // Right: the verbs, dropped from the tail when the room is squeezed
    // (shuffle first, then queue all — play holds out longest).
    if !gui.app.pane().has_tracks() {
        return;
    }
    let forward_glyph = forward_glyph();
    let shuffle_glyph = if legacy_conhost() { "" } else { "⇄ " };
    let mut verbs: Vec<(String, Act)> = vec![
        (format!("{forward_glyph} {}", t!("gui.bar.play")), Act::BarPlay),
        (format!("+ {}", t!("gui.bar.queue")), Act::BarQueueAll),
        (format!("{shuffle_glyph}{}", t!("gui.shuffle_word")), Act::BarShuffle),
    ];
    let sep = " · ";
    let width_of = |verbs: &[(String, Act)]| -> u16 {
        let labels: u16 = verbs.iter().map(|(l, _)| l.chars().count() as u16).sum();
        labels + sep.chars().count() as u16 * verbs.len().saturating_sub(1) as u16
    };
    let left_edge = content.x
        + if filter.is_empty() {
            3 + t!("gui.bar.filter").chars().count() as u16
        } else {
            3 + bar::clip(&filter, 24).chars().count() as u16 + 5
        };
    while verbs.len() > 1 && content.right().saturating_sub(left_edge + 2) < width_of(&verbs) {
        verbs.pop();
    }
    let mut vx = content.right().saturating_sub(width_of(&verbs) + 1);
    for (i, (label, act)) in verbs.iter().enumerate() {
        if i > 0 {
            put(frame, vx, y, sep, dim());
            vx += sep.chars().count() as u16;
        }
        let rect = Rect { x: vx, y, width: label.chars().count() as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        let accent_verb = matches!(act, Act::BarPlay);
        let style = match (hover, accent_verb) {
            (true, _) => bright_bold(),
            (false, true) => accent(),
            (false, false) => dim(),
        };
        put(frame, vx, y, label, style);
        gui.ui.click(rect, act.clone());
        // The key rides the tooltip, the close-control's own pattern —
        // the tips line has no room for more entries.
        let key = match act {
            Act::BarPlay => "p",
            Act::BarQueueAll => "A",
            _ => "S",
        };
        gui.ui.tip_keyed(rect, format!("{label} — {key}"));
        vx += rect.width;
    }
}

/// The Files room's bar: line 1 the crumb row it always was — back ◂, the
/// path, the honest count — line 2 the shared controls.
fn draw_files_bar(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let y = content.y;
    let count = bar_count(gui, t!("gui.files.items", count = gui.app.files.counts().1).to_string());
    let count_x = content.right().saturating_sub(count.chars().count() as u16);
    put(frame, count_x, y, &count, dim());

    let forward_glyph = forward_glyph();
    let mut x = content.x;
    if !gui.app.path.is_empty() {
        x += draw_bar_back(frame, gui, x, y);
    }
    let mut crumb = if gui.app.path.is_empty() {
        t!("gui.nav.files").to_string()
    } else {
        format!("{} {} {}", t!("gui.nav.files"), forward_glyph, gui.app.path)
    };
    // A peer is read-only, and the crumb says so (contract clause 26).
    if gui.app.session.peer.is_some() {
        crumb = format!("{crumb} · {}", t!("gui.srv.read_only"));
    }
    put(frame, x, y, &clip_lead(&crumb, count_x.saturating_sub(x + 2) as usize), dim());

    draw_bar_controls(frame, gui, content, content.y + 1);
}

/// The shared list renderer for the App's browse panes: kit table rows —
/// tracks with the playing marker, durations and the hover [+]; drill rows
/// (folders, classes, artists…) with their dim detail column. `rows` pairs
/// each drawn row with its PANE index, so a class filter upstream costs
/// clicks nothing.
///
/// Takes the surface and the rows apart rather than the whole `Gui`, so
/// callers can hand it BORROWED entries: this runs every frame, and the
/// per-frame clone of every visible row it used to require was measurable
/// drawing time spent on nothing.
#[allow(clippy::too_many_arguments)]
fn draw_pane_rows(
    frame: &mut Frame,
    ui: &mut Surface<Act>,
    playing: Option<&str>,
    rows: &[(usize, &Entry)],
    list: Rect,
    selected: Option<usize>,
    pane: List,
    // An armed pick consumes the next activation outright (clause 10) —
    // the hover verbs must not offer to queue what a click would capture.
    queue_plus: bool,
) {
    let tab = pane.tab().unwrap_or(Tab::Files);
    let row_act = |i: usize| Act::PaneRow(pane, i, RowVerb::Open);
    let queue_act = |i: usize| Act::PaneRow(pane, i, RowVerb::Queue);
    let next_act = |i: usize| Act::PaneRow(pane, i, RowVerb::Next);
    let now_act = |i: usize| Act::PaneRow(pane, i, RowVerb::Now);
    let more_act = |i: usize| Act::More(tab, i);
    for (row, (index, entry)) in rows.iter().enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let hover = ui.hovers(rect);
        let is_sel = *index == selected.unwrap_or(usize::MAX);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let name_width = list.width as usize - 10;
        match entry {
            Entry::Track { label, track } => {
                let is_playing = playing == Some(track.filepath.as_str());
                let (marker, style) = match (is_sel, is_playing, hover) {
                    (true, playing, _) => (playing, sel().add_modifier(Modifier::BOLD)),
                    (false, true, _) => (true, Style::default().fg(th().ok).add_modifier(Modifier::BOLD)),
                    (false, false, true) => (false, bright_bold()),
                    (false, false, false) => (false, Style::default()),
                };
                if marker {
                    let mark = forward_glyph();
                    put(frame, list.x, y, mark, if is_sel { sel().add_modifier(Modifier::BOLD) } else { Style::default().fg(th().ok).add_modifier(Modifier::BOLD) });
                }
                // A right click opens the row's sheet (track-actions
                // contract, entry point 1).
                ui.context(rect, more_act(*index));
                if hover && !is_sel && queue_plus {
                    // Play now · Add next · Add to the end (contract clause
                    // 32) · the sheet, each with the key its dwell tooltip
                    // names.
                    let verbs = [
                        (if legacy_conhost() { "[>]" } else { "[▸]" }, now_act(*index), t!("gui.files.now_tip")),
                        (if legacy_conhost() { "[^]" } else { "[»]" }, next_act(*index), t!("gui.files.next_tip")),
                        ("[+]", queue_act(*index), t!("gui.files.queue_tip")),
                        (if legacy_conhost() { "[.]" } else { "[⋯]" }, more_act(*index), t!("gui.act.more_tip")),
                    ];
                    put(frame, list.x + 2, y, &bar::clip(label, name_width.saturating_sub(12)), style);
                    ui.click(rect, row_act(*index));
                    let mut x = rect.right() - 3;
                    for (glyph, act, tip) in verbs.into_iter().rev() {
                        let cell = Rect { x, y, width: 3, height: 1 };
                        // Dim at rest, bright under the hand — the text
                        // button's rule, so the verb about to fire says so.
                        put(frame, x, y, glyph, if ui.hovers(cell) { bright_bold() } else { dim() });
                        ui.click(cell, act);
                        ui.tip_keyed(cell, tip.to_string());
                        x = x.saturating_sub(4);
                    }
                } else {
                    put(frame, list.x + 2, y, &bar::clip(label, name_width), style);
                    let time = track.metadata.duration.map(bar::fmt_time).unwrap_or_default();
                    let tstyle = if is_sel { sel() } else { dim() };
                    put(frame, rect.right() - 1 - time.chars().count() as u16, y, &time, tstyle);
                    ui.click(rect, row_act(*index));
                }
            }
            other => {
                let style = match (is_sel, hover) {
                    (true, _) => sel().add_modifier(Modifier::BOLD),
                    (false, true) => bright_bold(),
                    (false, false) if matches!(other, Entry::Parent) => dim(),
                    (false, false) => Style::default(),
                };
                // A drill row's trailing count or year — "Ambient (12)",
                // "Dummy (1994)" — reads dim (library-rooms contract, clause 2).
                let label = other.label();
                let (main, suffix) = match (matches!(other, Entry::Node { .. }), label.rfind(" (")) {
                    (true, Some(at)) if label.ends_with(')') => (&label[..at], &label[at..]),
                    _ => (label, ""),
                };
                let shown = bar::clip(main, name_width.saturating_sub(suffix.chars().count()));
                put(frame, list.x + 2, y, &shown, style);
                if !suffix.is_empty() {
                    let sstyle = if is_sel { sel() } else { dim() };
                    put(frame, list.x + 2 + shown.chars().count() as u16, y, suffix, sstyle);
                }
                // Drill rows carry a dim right-hand column — a hit count,
                // a closeness — the kit table's detail spot.
                let detail = match other {
                    Entry::Search { detail, .. }
                    | Entry::Discover { detail, .. }
                    | Entry::Setting { detail, .. }
                    | Entry::Sonic { detail, .. } => Some(detail.as_str()),
                    _ => None,
                };
                if let Some(detail) = detail.filter(|d| !d.is_empty()) {
                    let shown = bar::clip(detail, 14);
                    let dstyle = if is_sel { sel() } else { dim() };
                    put(frame, rect.right() - 1 - shown.chars().count() as u16, y, &shown, dstyle);
                }
                ui.click(rect, row_act(*index));
            }
        }
    }
}

/// Search: the query card, the five class chips (the search params — the
/// server answers every class at once, so the choice is instant), and the
/// App's Search pane beneath: the class menu with hit counts, then
/// whatever a drill opened — artists into albums into tracks, all through
/// the shared state machine.
fn draw_search(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        servers::draw_disconnected(frame, gui, content, 0);
        return;
    }

    // The query card: a kit input — accent border and caret while it takes
    // keys, dim at rest, one click to wake it.
    let editing = gui.app.editing_query;
    let card = Rect { x: content.x, y: content.y, width: content.width, height: 3 };
    let card_hover = gui.ui.hovers(card);
    let border = if editing {
        Style::default().fg(th().accent)
    } else if card_hover {
        Style::default().fg(th().bright)
    } else {
        dim()
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(border);
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let field = inner.width.saturating_sub(2) as usize;
    if editing {
        let cursor = gui.app.query.chars().count();
        put(frame, inner.x + 1, inner.y, &input_display(&gui.app.query, cursor, field as u16), Style::default());
    } else if gui.app.query.is_empty() {
        put(frame, inner.x + 1, inner.y, &t!("gui.search.placeholder"), dim());
    } else {
        put(frame, inner.x + 1, inner.y, &bar::clip(&gui.app.query, field), Style::default());
    }
    gui.ui.click(card, Act::EditQuery);
    gui.ui.tip_keyed(card, t!("gui.search.edit_tip").to_string());

    // The class chips: toggle words wearing their state (the bar's toggle
    // grammar), the chip cursor as the one selection bg.
    let mut x = content.x;
    let chips_y = content.y + 3;
    for (i, class) in SEARCH_CLASSES.iter().enumerate() {
        let label = class_label(*class);
        let rect = Rect { x, y: chips_y, width: label.chars().count() as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        let style = if gui.chip == i && gui.cursor.is_none() && !editing {
            sel().add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else if gui.classes_on[i] {
            Style::default().fg(th().ok).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, x, chips_y, &label, style);
        gui.ui.click(rect, Act::Chip(i));
        x += rect.width + 2;
    }

    // The summary (the App's own words) — or the busy line while the
    // reply is out.
    if let Some(summary) = gui.app.search_summary.clone() {
        put(frame, content.x, chips_y + 1, &bar::clip(&summary, content.width as usize), dim());
    }

    if gui.app.search_hits.is_none() {
        return;
    }

    // The pane, class-filtered at the MENU level only: a chip turned off
    // hides its class row; inside a drill every row shows. Rows pair with
    // their pane index, so clicks land on the right entry either way.
    let entries = &gui.app.search.entries;
    let at_menu = entries.iter().all(|e| matches!(e, Entry::Search { .. }));
    let visible_rows: Vec<(usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| match e {
            Entry::Search { node: SearchNode::Class(c), .. } if at_menu => {
                gui.classes_on[class_idx(*c)]
            }
            _ => true,
        })
        .collect();

    let list = Rect {
        x: content.x,
        y: chips_y + 3,
        width: content.width - 2,
        height: content.height.saturating_sub(6 + 2),
    };
    if visible_rows.is_empty() {
        put(frame, list.x, list.y, &t!("gui.files.empty"), dim());
        return;
    }
    let selected = gui.app.search.state.selected();
    let sel_pos = selected.and_then(|sel| visible_rows.iter().position(|(i, _)| *i == sel));
    let (first, visible) = gui.search_view.window(visible_rows.len(), sel_pos, list.height as usize);
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &visible_rows[first..first + visible],
        list,
        selected,
        List::Search,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        visible_rows.len(),
        visible,
        first,
        Act::ScrollBy(List::Search, -1),
        Act::ScrollBy(List::Search, 1),
        |first| Act::ScrollTo(List::Search, first),
    );
}

fn draw_settings(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    match gui.settings_room {
        Some(SettingsRoom::Servers) => return servers::draw_room(frame, gui, content),
        Some(SettingsRoom::Torrent) => return torrent::draw_room(frame, gui, content),
        None => {}
    }
    let (check_on, check_off) = check_glyphs();
    put(frame, content.x, content.y, &t!("gui.set.playback"), dim());
    put(frame, content.x, content.y + 7, &t!("gui.set.servers_group"), dim());
    put(frame, content.x, content.y + 10, &t!("gui.set.torrents_group"), dim());
    put(frame, content.x, content.y + 14, &t!("gui.set.display_group"), dim());

    let rows: [(String, String); SET_ROWS] = [
        (
            format!("{:<14} -  {:>4}  +", t!("gui.set.blend"), fmt_blend(gui.app.crossfade)),
            t!("gui.set.blend_desc").to_string(),
        ),
        (
            format!("{} {}", if gui.app.gapless { check_on } else { check_off }, t!("gui.set.gapless")),
            t!("gui.set.gapless_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.app.blend_skips { check_on } else { check_off },
                t!("gui.set.blend_skips")
            ),
            t!("gui.set.blend_skips_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.app.pause_fade { check_on } else { check_off },
                t!("gui.set.pause_fade")
            ),
            t!("gui.set.pause_fade_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.app.resume_queue { check_on } else { check_off },
                t!("gui.set.resume")
            ),
            t!("gui.set.resume_desc").to_string(),
        ),
        (
            format!("{} {}", t!("gui.srv.manage"), forward_glyph()),
            t!("gui.srv.manage_desc").to_string(),
        ),
        (
            format!("{} {}", t!("gui.set.tor_add"), forward_glyph()),
            t!("gui.set.tor_add_desc").to_string(),
        ),
        (
            format!("{} {}", if gui.config.torrent.ask { check_on } else { check_off }, t!("gui.set.tor_ask")),
            t!("gui.set.tor_ask_desc").to_string(),
        ),
        (
            format!("{} {}", if gui.config.gui.key_hints { check_on } else { check_off }, t!("gui.set.key_hints")),
            t!("gui.set.key_hints_desc").to_string(),
        ),
    ];

    for (i, (label, desc)) in rows.iter().enumerate() {
        let y = row_y(content.y, i);
        let rect = Rect { x: content.x, y, width: content.width, height: 1 };
        let selected = gui.cursor == Some(i);
        let hover = gui.ui.hovers(rect);
        if selected {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let (label_style, desc_style) = if selected {
            (sel().add_modifier(Modifier::BOLD), sel())
        } else if hover {
            (bright_bold(), Style::default().fg(th().bright))
        } else {
            (Style::default(), dim())
        };
        put(frame, content.x, y, label, label_style);
        // The description takes whatever room the row has left, clipped at
        // the cell edge — with the queue open that is not much, and the
        // full sentence returns the moment the queue folds away. A label
        // wider than the column (a translation, the torrents switch)
        // pushes it over rather than being written through.
        let desc_x = content.x + (label.chars().count() as u16 + 2).max(27);
        let avail = rect.right().saturating_sub(desc_x) as usize;
        if avail >= 10 {
            put(frame, desc_x, y, &bar::clip(desc, avail), desc_style);
        }
        gui.ui.click(rect, Act::Row(i));
        // The blend's - and + are their own targets, drawn after the row so
        // the later rect wins the hit (the kit's overlay rule).
        if i == ROW_BLEND {
            let minus = Rect { x: content.x + 15, y, width: 1, height: 1 };
            let plus = Rect { x: content.x + 24, y, width: 1, height: 1 };
            gui.ui.click(minus, Act::BlendDown);
            gui.ui.click(plus, Act::BlendUp);
        }
    }
}

/// The sheet's door on each pane's rows (track-actions contract, entry
/// point 1): the App's pane, by tab, and the row.
/// Where settings row `i` draws, under its section label.
fn row_y(top: u16, i: usize) -> u16 {
    match i {
        i if i < ROW_MANAGE => top + 1 + i as u16,
        ROW_MANAGE => top + 8,
        ROW_TORRENT => top + 11,
        ROW_HINTS => top + 15,
        _ => top + 12,
    }
}

// ── Input ───────────────────────────────────────────────────────────────────

/// Keys, contextual and few — the letters the tips line names. Returns
/// true to quit.
fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    // Auto DJ's chooser owns the keyboard while it is up, and Esc leaves the
    // opening-song road; then the servers surfaces: an open modal owns the
    // keyboard outright, the room takes its row keys, and everything else
    // falls through untouched.
    if let Some(quit) = actions::handle_key(gui, key) {
        return quit;
    }
    if let Some(quit) = dj::handle_key(gui, key) {
        return quit;
    }
    if let Some(quit) = servers::handle_key(gui, key) {
        return quit;
    }
    // The torrent surfaces next: the arrival chooser can be up over any
    // room, and the room itself owns the keys while it is open.
    if let Some(quit) = torrent::handle_key(gui, key) {
        return quit;
    }
    // The Now Playing screen has no rooms: the queue's keys when it holds
    // them, then the screen's own.
    if gui.screen == Screen::NowPlaying {
        if let Some(quit) = actions::queue_key(gui, key) {
            return quit;
        }
        return now::handle_key(gui, key);
    }
    let browse = gui.browse_room()
        && gui.app.connected
        && !actions::modal_open(gui)
        && !dj::modal_open(gui)
        && !sonic::modal_open(gui)
        && !playlists::modal_open(gui);
    // The bar's filter owns the keyboard while it is taking text (the
    // App's prompt claims the editing keys; the arrows fall through so
    // the narrowed list can be walked mid-thought). This gate outranks
    // the room handlers — a typed letter must never queue a row.
    if browse && gui.app.filtering {
        gui.app.tab = gui.browse_tab();
        match key.code {
            KeyCode::Enter => gui.forward(Action::Submit),
            KeyCode::Esc => gui.forward(Action::Cancel),
            KeyCode::Backspace => gui.forward(Action::Backspace),
            KeyCode::Down => {
                gui.files_view.reveal = true;
                gui.playlists.list.reveal = true;
                gui.forward(Action::Down);
            }
            KeyCode::Up => {
                gui.files_view.reveal = true;
                gui.playlists.list.reveal = true;
                gui.forward(Action::Up);
            }
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            _ => {}
        }
        return false;
    }
    // The queue panel while it has the keys (track-actions contract,
    // clause 19) — ahead of every room, so a room's Esc or Enter never
    // takes what the panel was handed, whichever room is up.
    if let Some(quit) = actions::queue_key(gui, key) {
        return quit;
    }
    // The bar's own keys, every browse room alike; Esc clears a standing
    // filter before it means anything else in the room.
    if browse {
        match key.code {
            KeyCode::Char('f') => return gui.act(Act::BarFilter),
            KeyCode::Char('p') => return gui.act(Act::BarPlay),
            KeyCode::Char('A') => return gui.act(Act::BarQueueAll),
            KeyCode::Char('S') => return gui.act(Act::BarShuffle),
            KeyCode::Esc if !gui.app.pane().filter.is_empty() => {
                return gui.act(Act::BarClear);
            }
            _ => {}
        }
    }
    if gui.active == ALBUMS_NAV
        && gui.app.connected
        && let Some(quit) = albums::handle_key(gui, key)
    {
        return quit;
    }
    if let Some(quit) = library::handle_key(gui, key) {
        return quit;
    }
    if let Some(quit) = sonic::handle_key(gui, key) {
        return quit;
    }
    if let Some(quit) = playlists::handle_key(gui, key) {
        return quit;
    }
    // An armed pick is the loudest thing on screen: Esc stops picking
    // before it means anything else, and goes home to the room that asked
    // — the App's own Cancel contract, followed on this surface too.
    if key.code == KeyCode::Esc
        && matches!(gui.app.capture, Some(crate::tui::app::Capture::Sonic(_)))
    {
        gui.forward(Action::Cancel);
        gui.active = SONIC_NAV;
        gui.app.tab = Tab::SonicPath;
        return false;
    }
    let settings = gui.active == SETTINGS_NAV;
    let files = gui.active == FILES_NAV;
    let search = gui.active == SEARCH_NAV;
    // The query box owns the keyboard while it is taking text — q, the
    // digits and the transport letters are all just letters here.
    if search && gui.app.editing_query {
        match key.code {
            KeyCode::Enter => {
                gui.forward(Action::Submit);
                gui.search_view.reveal = true;
            }
            KeyCode::Esc => gui.forward(Action::Cancel),
            KeyCode::Backspace => gui.forward(Action::Backspace),
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            _ => {}
        }
        return false;
    }
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Tab => return gui.act(Act::ToggleQueue),
        KeyCode::Char(c @ '1'..='9') => {
            return gui.act(Act::Nav(c as usize - '1' as usize));
        }
        KeyCode::Char('0') => return gui.act(Act::Screen(Screen::NowPlaying)),
        // The tenth room has no digit; `D` is the capital beside `A`'s toggle.
        KeyCode::Char('D') => return gui.act(Act::Nav(DJ_NAV)),
        // `/` is the search key everywhere, the TUI's own habit: land on
        // Search with the query box open.
        KeyCode::Char('/') => {
            gui.active = SEARCH_NAV;
            gui.cursor = None;
            return gui.act(Act::EditQuery);
        }
        KeyCode::Down if settings => {
            gui.cursor = Some(gui.cursor.map_or(0, |c| (c + 1).min(SET_ROWS - 1)));
        }
        KeyCode::Up if settings => {
            gui.cursor = Some(gui.cursor.map_or(SET_ROWS - 1, |c| c.saturating_sub(1)));
        }
        // The Files list is the App's own pane: arrows, Enter, back and
        // queue-add forward straight to the shared state machine.
        KeyCode::Down if files => {
            gui.files_view.reveal = true;
            gui.forward(Action::Down);
        }
        KeyCode::Up if files => {
            gui.files_view.reveal = true;
            gui.forward(Action::Up);
        }
        KeyCode::PageDown if files => {
            gui.files_view.reveal = true;
            gui.forward(Action::PageDown);
        }
        KeyCode::PageUp if files => {
            gui.files_view.reveal = true;
            gui.forward(Action::PageUp);
        }
        KeyCode::Enter if files => gui.forward_capturing(Action::Activate),
        KeyCode::Char('h') | KeyCode::Backspace if files => gui.forward(Action::Back),
        KeyCode::Char('a') if files => gui.forward(Action::AddToQueue),
        KeyCode::Char('N') if files => gui.forward(Action::AddNext),
        KeyCode::Char('P') if files => gui.forward(Action::PlayNow),
        // Search browsing: the same pane keys as Files, plus the chip
        // cursor on ←/→ and `t` to flip the class under it.
        KeyCode::Down if search => {
            gui.search_view.reveal = true;
            gui.forward(Action::Down);
        }
        KeyCode::Up if search => {
            gui.search_view.reveal = true;
            gui.forward(Action::Up);
        }
        KeyCode::Enter if search => gui.forward_capturing(Action::Activate),
        KeyCode::Char('h') | KeyCode::Backspace if search => gui.forward(Action::Back),
        KeyCode::Char('a') if search => gui.forward(Action::AddToQueue),
        KeyCode::Char('N') if search => gui.forward(Action::AddNext),
        KeyCode::Char('P') if search => gui.forward(Action::PlayNow),
        KeyCode::Left if search => gui.chip = gui.chip.saturating_sub(1),
        KeyCode::Right if search => gui.chip = (gui.chip + 1).min(SEARCH_CLASSES.len() - 1),
        KeyCode::Char('t') if search => {
            let chip = gui.chip;
            return gui.act(Act::Chip(chip));
        }
        KeyCode::Esc => gui.cursor = None,
        KeyCode::Left => {
            if let Some(row) = gui.cursor.filter(|_| settings) {
                gui.adjust_row(row, -1);
            }
        }
        KeyCode::Right => {
            if let Some(row) = gui.cursor.filter(|_| settings) {
                gui.adjust_row(row, 1);
            }
        }
        KeyCode::Enter => {
            if let Some(row) = gui.cursor.filter(|_| settings) {
                return gui.act(Act::Row(row));
            }
        }
        // Space toggles under the cursor (the kit's checkbox law); stowed,
        // it is the transport key.
        KeyCode::Char(' ') => {
            return match gui.cursor.filter(|_| settings) {
                Some(row) => gui.act(Act::Row(row)),
                None => gui.act(Act::PlayPause),
            };
        }
        KeyCode::Char('p') => return gui.act(Act::Prev),
        KeyCode::Char('n') => return gui.act(Act::Next),
        KeyCode::Char('s') => return gui.act(Act::Shuffle),
        KeyCode::Char('r') => return gui.act(Act::Repeat),
        KeyCode::Char('A') => return gui.act(Act::AutoDj),
        KeyCode::Char('m') => return gui.act(Act::MoreKey),
        KeyCode::Char('-') => return gui.act(Act::VolDown),
        KeyCode::Char('+') | KeyCode::Char('=') => return gui.act(Act::VolUp),
        _ => {}
    }
    false
}

// ── The loop and the room it runs in ────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    gui: &mut Gui,
    mouse_on: bool,
    event_rx: &Receiver<Event>,
    audio_tx: &Sender<AudioCmd>,
    api_tx: &Sender<worker::ApiCmd>,
    event_tx: &Sender<Event>,
) -> std::io::Result<()> {
    let mut hand = false;
    let mut saver = tui::QueueSaver::new(&gui.app);
    loop {
        // A SaveSession about to be dispatched writes the config behind
        // this copy's back — a Quick Connect add mints a whole new entry
        // there. Reload after, so the dropdown and the room list it.
        let saving = gui
            .pending
            .iter()
            .any(|e| matches!(e, Effect::SaveSession | Effect::SavePeers { .. } | Effect::SaveDjLibrary { .. }));
        tui::dispatch(&gui.app, &mut gui.pending, audio_tx, api_tx, event_tx);
        saver.tick(&gui.app);
        let ticked = gui.app.tick();
        gui.pend(ticked);
        if saving && let Ok(fresh) = config::load() {
            gui.config = fresh;
            refresh_book(gui);
        }
        terminal.draw(|frame| render(frame, gui))?;

        while let Ok(ev) = event_rx.try_recv() {
            // The servers layer looks first: session answers that would
            // land on the TUI's connect screen open the GUI's form instead.
            servers::observe(gui, &ev);
            torrent::observe(gui, &ev);
            // A random pick that lands while results are up owes them a
            // rebuild — clause 22's promise, kept here because the App
            // consumes the pick into the setup view first.
            let sonic_random = matches!(ev, Event::SonicRandom { .. });
            let was_results = gui.app.sonic.view == crate::tui::app::SonicView::Results;
            let effects = gui.app.apply_event(ev);
            gui.pend(effects);
            if sonic_random {
                sonic::random_landed(gui, was_results);
            }
        }
        servers::poll(gui);
        torrent::poll(gui);

        let over = gui.ui.hovering_clickable();
        if over != hand {
            hand = over;
            set_pointer_shape(hand, mouse_on);
        }
        if let Some(act) = gui.ui.hold_action() {
            gui.act(act);
        }
        gui.ui.dwell_tick();

        // While covers are still upgrading to pixels, the next frame is
        // wanted promptly — idling out the full poll would stretch a page
        // turn's ~50 ms of encode work across a second of ticks.
        let wait = if gui.hot { Duration::from_millis(10) } else { POLL };
        if !event::poll(wait)? {
            continue;
        }
        // Drain everything queued before the next draw (the wizard's
        // collapse-moves lesson: pointer sweeps are one event per cell).
        let mut inputs = vec![event::read()?];
        while event::poll(Duration::ZERO)? {
            inputs.push(event::read()?);
        }
        for input in inputs {
            match input {
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    gui.ui.dismiss_tooltip();
                    if handle_key(gui, key) {
                        saver.flush(&gui.app);
                        return Ok(());
                    }
                }
                TermEvent::Mouse(mouse) => {
                    let at = Position { x: mouse.column, y: mouse.row };
                    match mouse.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            if !gui.ui.begin_press(at) {
                                continue;
                            }
                            // A click in the content column hands the keys
                            // back from the queue (track-actions contract,
                            // clause 19).
                            if gui.app.focus == crate::tui::app::Focus::Queue
                                && !actions::modal_open(gui)
                                && !(gui.queue_open && at.x >= gui.queue_panel_x())
                            {
                                gui.app.focus = crate::tui::app::Focus::Browser;
                            }
                            if let Some(act) = gui.ui.hit(at) {
                                if gui.act(act) {
                                    saver.flush(&gui.app);
                                    return Ok(());
                                }
                            }
                            gui.ui.arm_bars(at);
                        }
                        // A right click on a row is its sheet (entry point 1).
                        MouseEventKind::Down(MouseButton::Right) => {
                            if let Some(act) = gui.ui.hit_context(at)
                                && gui.act(act)
                            {
                                saver.flush(&gui.app);
                                return Ok(());
                            }
                        }
                        MouseEventKind::Moved => gui.ui.motion(at),
                        MouseEventKind::Drag(_) => {
                            gui.ui.motion(at);
                            if gui.actions.drag.is_some() {
                                actions::drag_to(gui, at);
                            } else if let Some(act) = gui.ui.drag_action(at) {
                                gui.act(act);
                            }
                        }
                        MouseEventKind::Up(_) => {
                            gui.ui.release();
                            actions::drop(gui);
                        }
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            let delta = if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                            gui.wheel(at, delta);
                        }
                        _ => {}
                    }
                }
                // A resized window changes the cell-to-pixel mapping the
                // cover encodes against — the card's, every album slot's
                // and every queue row's alike.
                TermEvent::Resize(..) => {
                    gui.app.graphics.refresh();
                    gui.albums.on_resize();
                    gui.queue.on_resize();
                    gui.actions.on_resize();
                    gui.now.on_resize();
                }
                _ => {}
            }
        }
    }
}

impl Gui {
    /// Where the queue panel begins, matched to `draw_queue`'s separator
    /// on the last drawn frame — the wheel's queue-vs-content split.
    pub(crate) fn queue_panel_x(&self) -> u16 {
        self.last_width.saturating_sub(34)
    }

    /// The viewport a list scrolls — the album wall's track list is
    /// whichever wall is on screen, the Albums room's or an artist's.
    fn list_mut(&mut self, list: List) -> &mut ListView {
        match list {
            List::Files => &mut self.files_view,
            List::Search => &mut self.search_view,
            List::Library => &mut self.library.view,
            List::AlbumTracks => &mut albums::wall(self).tracks,
            List::Playlists => &mut self.playlists.list,
            List::PlaylistTracks => &mut self.playlists.tracks,
            List::Queue => &mut self.queue_view,
            List::Sonic => &mut self.sonic.results,
            List::DjRoom => &mut self.dj.body,
            List::DjGenres => &mut self.dj.genres,
        }
    }

    /// Seat a pane row for a forwarded verb: the App's tab and pane cursor
    /// go there, the keys come back to the browser (track-actions contract,
    /// clause 19), and the list reveals the row.
    fn aim(&mut self, list: List, index: usize) {
        let Some(tab) = list.tab() else { return };
        self.app.tab = tab;
        self.app.focus = crate::tui::app::Focus::Browser;
        self.app.pane_for_mut(tab).state.select(Some(index));
        self.list_mut(list).reveal = true;
    }

    /// Whether the screen keeps its tips line under the bar — the bar and
    /// the content sit one row higher while it does.
    fn footer(&self) -> bool {
        self.config.gui.key_hints
    }

    /// Whether `room` stands where the Settings rows would be.
    pub(crate) fn in_settings_room(&self, room: SettingsRoom) -> bool {
        self.active == SETTINGS_NAV && self.settings_room == Some(room)
    }

    /// Which App tab the active browse room's pane rides — the bar's acts
    /// and keys seat it before forwarding (docs/ux-contracts/browser-top-bar.md).
    fn browse_tab(&self) -> Tab {
        if self.active == FILES_NAV { Tab::Files } else { Tab::Library }
    }

    /// Whether the active room wears the browse bar at all.
    fn browse_room(&self) -> bool {
        self.screen == Screen::Library
            && matches!(self.active, FILES_NAV | ALBUMS_NAV | ARTISTS_NAV | GENRES_NAV | RECENT_NAV | PLAYLISTS_NAV)
    }

    /// The wheel scrolls the view under the pointer, never the selection
    /// (the kit's table law). The open queue outranks the rooms at its own
    /// columns; inside the content column every room answers for itself —
    /// and the match is exhaustive over [`NavId`], so a room cannot ship
    /// without deciding what its wheel does (the sonic room did exactly
    /// that, and swipes silently went nowhere).
    fn wheel(&mut self, at: Position, delta: i32) {
        self.ui.pointer = Some(at);
        if self.queue_open && at.x >= self.queue_panel_x() {
            self.act(Act::ScrollBy(List::Queue, delta));
            return;
        }
        // The Now Playing screen scrolls nothing yet; the nav column never.
        if self.screen == Screen::NowPlaying || at.x < 17 {
            return;
        }
        match NAV[self.active] {
            NavId::Files => {
                self.act(Act::ScrollBy(List::Files, delta));
            }
            NavId::Albums => albums::wheel(self, delta),
            NavId::Search => {
                self.act(Act::ScrollBy(List::Search, delta));
            }
            NavId::Sonic => sonic::wheel(self, delta),
            NavId::Playlists => playlists::wheel(self, delta),
            // Settings scrolls nothing itself; its Add-torrent room's file
            // picker does. The Auto DJ room's body and genre picker scroll.
            NavId::Settings => torrent::wheel(self, delta),
            NavId::Dj => {
                dj::wheel(self, delta);
            }
            NavId::Artists | NavId::Genres | NavId::Recent => library::wheel(self, delta),
        }
    }
}

/// `torrent` is the `--torrent` seam: a `.torrent` path or a magnet link
/// the player was opened WITH, the way the OS hands one to the app it
/// registered for them (docs/ux-contracts/add-torrent.md, entry point 2).
/// The App's book of saved servers follows the config: a peer the reconcile
/// just wrote, a token a sign-in just saved, a pairing code — the queue's
/// rows and the header name servers from it (contract clauses 20, 30).
pub(crate) fn refresh_book(gui: &mut Gui) {
    let credentials = config::load_credentials().unwrap_or_default();
    gui.app.servers = tui::known_servers(&gui.config, &credentials);
}

pub fn run(
    server: Option<String>,
    token: Option<String>,
    torrent: Option<String>,
    bundled: Option<String>,
) -> i32 {
    // The player's own tolerant load first — it may seed the bundled
    // server — then the GUI's read of what is on disk (the [gui] section,
    // and the save guard).
    let start = tui::startup(server, token, bundled);
    let (config, config_ok) = match config::load() {
        Ok(config) => (config, true),
        Err(_) => (Config::default(), false),
    };
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (audio_tx, tap) = worker::spawn_audio(event_tx.clone());
    let api_tx = worker::spawn_api(event_tx.clone());

    let mut app = tui::app_from(start);
    app.tap = Some(tap);
    let pending = app.start();

    let mut gui = Gui::new(config, config_ok, app);
    gui.pending = pending;
    if std::env::var("MSTREAM_GUI_DEMO").is_ok_and(|v| v == "1") {
        gui.demo = Some(demo_now());
    }
    if let Some(arg) = torrent {
        torrent::arrive(&mut gui, &arg);
    }

    let _title = crate::tui::WindowTitle::claim("mStream Player");
    // The OSC 11 ground lease runs before ratatui takes the terminal — the
    // wizard's ordering, for the wizard's reasons.
    let claim = theme::acquire_ground();
    let ground_guard = GroundGuard;
    let mut terminal = ratatui::init();
    // After init, like the player: a terminal that answers the pixel probe
    // strangely makes its mess on the alternate screen, which restore
    // throws away.
    gui.app.graphics = crate::tui::graphics::Graphics::probe();
    crate::console::claim_terminal();
    let mouse_on = execute!(std::io::stdout(), EnableMouseCapture).is_ok();
    if let Some(seq) = claim {
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(seq));
    }
    set_pointer_shape(false, mouse_on);
    tui::install_panic_hook();

    let outcome = event_loop(
        &mut terminal,
        &mut gui,
        mouse_on,
        &event_rx,
        &audio_tx,
        &api_tx,
        &event_tx,
    );

    if mouse_on {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(POINTER_RESET));
    }
    ratatui::restore();
    crate::console::release_terminal();
    drop(ground_guard);
    let _ = audio_tx.send(AudioCmd::Shutdown);
    // The player prefs, the session and the last path persist the TUI's own
    // way; the GUI's bar choice rides its own section afterwards.
    tui::remember(&gui.app);
    gui.save_now();

    match outcome {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Track, TrackMetadata};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn test_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.demo = Some(demo_now());
        gui
    }

    fn track(filepath: &str, title: &str, duration: f64) -> Track {
        Track {
            filepath: filepath.to_string(),
            metadata: TrackMetadata {
                title: Some(title.to_string()),
                duration: Some(duration),
                ..TrackMetadata::default()
            },
        }
    }

    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        rows(&terminal)
    }

    /// The frame with its styles, for the tests that read colour.
    fn draw_buffer(gui: &mut Gui) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// A connected App with a listed Files pane, no server involved.
    fn browsing_gui() -> Gui {
        let mut gui = test_gui();
        gui.app.connected = true;
        gui.app.files.set(vec![
            Entry::Parent,
            Entry::Dir { label: "Ambient".into(), path: "music/Ambient".into() },
            Entry::Track { label: "Night Drive".into(), track: Box::new(track("music/a.mp3", "Night Drive", 252.0)) },
            Entry::Track { label: "Aurora".into(), track: Box::new(track("music/b.mp3", "Aurora", 228.0)) },
        ]);
        gui
    }

    #[test]
    fn the_files_browser_lists_what_the_app_holds() {
        let mut gui = browsing_gui();
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Ambient"), "directories are rows");
        assert!(all.contains("Night Drive") && all.contains("4:12"), "tracks carry durations");
        assert!(all.contains(".."), "the way out is a row");
    }

    #[test]
    fn clicking_a_row_selects_it_and_activates_through_the_app() {
        let mut gui = browsing_gui();
        gui.act(Act::PaneRow(List::Files, 1, RowVerb::Open));
        // Activate on a directory asks the server for a listing — the
        // effect is queued for dispatch, and the pane goes loading with
        // its cursor cleared (the App's own open semantics): proof the
        // click went through the shared state machine, not around it.
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Api(_))),
            "the click became an API effect: {:?}",
            gui.pending
        );
        assert!(gui.app.files.loading, "the pane is waiting on the listing");
    }

    #[test]
    fn queueing_from_the_hover_control_uses_add_to_queue() {
        let mut gui = browsing_gui();
        gui.act(Act::PaneRow(List::Files, 2, RowVerb::Queue));
        assert_eq!(gui.app.queue.items.len(), 1, "the one track was queued");
        assert_eq!(gui.app.queue.items[0].filepath, "music/a.mp3");
    }

    #[test]
    fn the_hover_verbs_add_next_play_now_and_the_queue_rows_answer_clicks() {
        // Contract clause 32, GUI-side: the verbs beside [+], a click on a
        // queue row playing it, and its hover [x] removing it.
        let mut gui = browsing_gui();
        // Rows resolve against their server, so the session needs one.
        gui.app.session.server = "http://host:3000".into();
        gui.app.session.server_id = "http://host:3000".into();
        gui.act(Act::PaneRow(List::Files, 2, RowVerb::Queue));
        gui.act(Act::PaneRow(List::Files, 3, RowVerb::Queue));
        gui.act(Act::QueueRow(1));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { url, .. }) if url.contains("b.mp3"))), "{:?}", gui.pending);
        assert_eq!(gui.app.queue.current, Some(1));
        gui.pending.clear();
        gui.app.status = crate::player::PlayerStatus { playing: true, source: "x".into(), ..Default::default() };

        gui.act(Act::PaneRow(List::Files, 2, RowVerb::Next));
        assert_eq!(gui.app.queue.items.len(), 3);
        assert_eq!(gui.app.queue.items[2].filepath, "music/a.mp3", "in right after the playing row");
        assert!(!gui.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { .. }))), "add next never plays");

        gui.act(Act::PaneRow(List::Files, 3, RowVerb::Now));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::Play { url, .. }) if url.contains("b.mp3"))), "play now plays: {:?}", gui.pending);
        assert_eq!(gui.app.queue.current, Some(2));

        gui.act(Act::QueueRemove(0));
        assert_eq!(gui.app.queue.items.len(), 3);
        assert_eq!(gui.app.queue.current, Some(1), "current followed its track down");

        // The hover verbs are drawn, and named, over a hovered row.
        gui.queue_open = false;
        gui.ui.pointer = Some(Position { x: 30, y: 7 });
        let all = draw(&mut gui).join("\n");
        let hovered = all.lines().nth(7).unwrap_or("");
        assert!(hovered.contains("Night Drive"), "the hovered row is a track: {hovered}");
        assert!(hovered.contains("[+]"), "the queue-add: {hovered}");
        assert!(hovered.contains("[»]") || hovered.contains("[^]"), "add next: {hovered}");
        assert!(hovered.contains("[▸]") || hovered.contains("[>]"), "play now: {hovered}");

        // Under the hand a verb brightens like every text button, and its
        // neighbours stay dim — the one about to fire says so.
        let col = hovered.char_indices().position(|(i, _)| hovered[i..].starts_with("[+]")).unwrap() as u16;
        gui.ui.pointer = Some(Position { x: col + 1, y: 7 });
        let buf = draw_buffer(&mut gui);
        assert_eq!(buf[(col, 7)].fg, th().bright, "the hovered verb brightens");
        assert!(buf[(col, 7)].modifier.contains(Modifier::BOLD), "and takes weight");
        assert_ne!(buf[(col - 4, 7)].fg, th().bright, "the verb beside it stays dim");
    }

    #[test]
    fn keyboard_hints_are_off_by_default_and_the_settings_row_brings_them_back() {
        // This surface is the pointer's: no footer of keys and no key on a
        // tooltip until asked; the classic TUI is the keyboard's room.
        let mut gui = browsing_gui();
        let rows = draw(&mut gui);
        assert!(!rows[29].contains("q quit"), "no footer of keys — the bar's bottom row instead: {:?}", rows[29]);
        assert!(rows[29].contains('%'), "the volume rides the bar's bottom row: {:?}", rows[29]);
        assert!(gui.ui.tips.iter().all(|(_, t)| !t.contains(" — ")), "no key on a tooltip: {:?}", gui.ui.tips);
        assert!(gui.ui.tips.iter().any(|(_, t)| t == "/ filter"), "the filter's tip is its label alone: {:?}", gui.ui.tips);

        gui.act(Act::Nav(SETTINGS_NAV));
        let rows = draw(&mut gui);
        let y = rows.iter().position(|r| r.contains("Keyboard hints")).unwrap();
        assert!(rows[y].contains("[ ]"), "the switch reads off: {:?}", rows[y]);
        gui.act(Act::Row(ROW_HINTS));
        assert!(gui.config.gui.key_hints, "the row flips the setting");
        let rows = draw(&mut gui);
        assert!(!rows[y].contains("[ ]"), "and reads on: {:?}", rows[y]);
        assert!(rows[29].contains("q quit"), "the footer names the keys: {:?}", rows[29]);

        gui.act(Act::Nav(FILES_NAV));
        draw(&mut gui);
        assert!(gui.ui.tips.iter().any(|(_, t)| t == "/ filter — f"), "the filter's tip names its key: {:?}", gui.ui.tips);
    }

    #[test]
    fn the_queue_panel_shows_the_real_queue_headed_by_its_count() {
        let mut gui = browsing_gui();
        gui.act(Act::PaneRow(List::Files, 2, RowVerb::Queue));
        gui.act(Act::PaneRow(List::Files, 3, RowVerb::Queue));
        gui.app.queue.current = Some(1);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("2 tracks · 8:00"), "count and total time head the panel: {all}");
        assert!(all.contains("Aurora"), "queued titles are rows");
    }

    #[test]
    fn the_bar_reads_the_app_when_something_real_plays() {
        let mut gui = browsing_gui();
        gui.app.now_playing = Some(track("music/a.mp3", "Night Drive", 252.0));
        gui.app.status.position = 63.0;
        gui.app.status.duration = 252.0;
        gui.app.status.paused = false;
        let now = gui.bar_now().unwrap();
        assert_eq!(now.title, "Night Drive");
        assert_eq!(now.elapsed, 63.0);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("1:03") && all.contains("4:12"), "real timestamps on the bar");
    }

    /// A search driven end to end through the real App: open the box, type,
    /// submit, and answer the wire with a two-class result set.
    fn searched_gui() -> Gui {
        use crate::api::types::{SearchGroup, SearchResults, SearchTrack};
        let mut gui = test_gui();
        gui.app.connected = true;
        gui.act(Act::Nav(SEARCH_NAV));
        assert!(gui.app.editing_query, "a fresh visit opens the query box");
        for c in "moon".chars() {
            handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let results = SearchResults {
            artists: vec![SearchGroup { name: "Moon Parade".into(), album_art_file: None }],
            albums: Vec::new(),
            title: vec![SearchTrack {
                name: "Moonstruck".into(),
                filepath: "m/a.mp3".into(),
                album_art_file: None,
                metadata: TrackMetadata { duration: Some(224.0), ..TrackMetadata::default() },
            }],
            files: Vec::new(),
            lyrics: Vec::new(),
        };
        let effects = gui
            .app
            .apply_event(Event::SearchResults { query: "moon".into(), results: Box::new(results) });
        gui.pend(effects);
        gui
    }

    #[test]
    fn search_flows_through_the_shared_app() {
        let mut gui = searched_gui();
        assert_eq!(gui.app.query, "moon", "typed characters reached the App's box");
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Api(_))),
            "Submit rode out as the App's own search effect"
        );
        assert!(gui.app.search_hits.is_some(), "the reply landed in the shared cache");
        assert!(
            gui.app.search.entries.iter().any(|e| matches!(e, Entry::Search { .. })),
            "the class menu stands ready"
        );
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("moon"), "the card shows the query");
        for class in SEARCH_CLASSES {
            assert!(all.contains(&class_label(class)), "every chip is on screen");
        }
    }

    #[test]
    fn the_query_box_owns_the_letters_while_it_is_open() {
        let mut gui = test_gui();
        gui.app.connected = true;
        gui.act(Act::Nav(SEARCH_NAV));
        let quit = handle_key(&mut gui, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!quit, "q is a letter here, not the quit key");
        assert_eq!(gui.app.query, "q");
        handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!gui.app.editing_query, "Esc hands the keyboard back");
    }

    #[test]
    fn chips_filter_the_menu_and_clicks_still_land_on_the_pane() {
        let mut gui = searched_gui();
        // Turn Artists off: its menu row hides, and the first visible class
        // row's PANE index still reaches the right entry.
        gui.act(Act::Chip(0));
        assert!(!gui.classes_on[0]);
        let all = draw(&mut gui).join("\n");
        let titles_row = gui
            .app
            .search
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Search { node: SearchNode::Class(SearchClass::Titles), .. }))
            .expect("the Titles class is in the menu");
        let _ = all;
        gui.act(Act::PaneRow(List::Search, titles_row, RowVerb::Open));
        assert!(
            gui.app
                .search
                .entries
                .iter()
                .any(|e| matches!(e, Entry::Track { track, .. } if track.filepath == "m/a.mp3")),
            "drilling the class listed its tracks ({} entries)",
            gui.app.search.entries.len()
        );
    }

    #[test]
    fn the_card_wears_the_cover_once_it_is_decoded() {
        let mut gui = browsing_gui();
        let mut playing = track("music/a.mp3", "Night Drive", 252.0);
        playing.metadata.album_art = Some("aa.jpeg".into());
        gui.app.now_playing = Some(playing);

        // Nothing fetched yet: the empty slot frame holds the cells.
        let rows = draw(&mut gui);
        // The card at the right edge: its cover is eight by four cells,
        // starting on the row under the seek line.
        let cover = bar::cover_rect(Rect { x: 0, y: 0, width: 100, height: 30 }, false);
        let slot_row: String = rows[cover.y as usize].chars().skip(cover.x as usize).take(cover.width as usize).collect();
        assert!(slot_row.contains('╭'), "the slot frame waits for the art: {slot_row:?}");

        // A real decode, so the art carries what both draw paths want; the
        // TestBackend has no pixel protocol, so the ▀-mosaic is the path.
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        gui.app.art.insert("aa.jpeg".into(), Some(art));

        let rows = draw(&mut gui);
        let cover_row: String = rows[cover.y as usize].chars().skip(cover.x as usize).take(cover.width as usize).collect();
        assert!(!cover_row.contains('╭'), "the frame yields to the picture: {cover_row:?}");
        // A solid test image mosaics as full blocks; a busy one mixes ▀.
        assert!(
            cover_row.chars().all(|c| "█▀▄".contains(c)),
            "the mosaic holds the cells: {cover_row:?}"
        );
    }

    #[test]
    fn auto_dj_holds_the_edge_and_the_transport_is_one_group_with_a_gold_play() {
        // The "Player bar options" canvas, I: auto-dj framed at the left
        // edge; then one centred group of frames — repeat, prev, play, next,
        // shuffle — play in the thick frame in gold, prev and next bold in
        // the rounded one, the toggles in their state colours. The card
        // wears no verb: a right click is its sheet.
        let mut gui = browsing_gui();
        let buf = draw_buffer(&mut gui);
        let area = *buf.area();
        let y = bar::top(area, false) + 2; // the controls' middle row
        let line = rows_of(&buf, y);
        let at = |needle: &str| line.char_indices().position(|(i, _)| line[i..].starts_with(needle)).map(|p| p as u16);
        let (dj, repeat, prev, play, next, shuffle) =
            (at("│ auto-dj │"), at("│ ↻ │"), at("│ ◂◂ │"), at("┃ ▮▮ ┃"), at("│ ▸▸ │"), at("│ ⇄ │"));
        assert_eq!(dj, Some(bar::DJ_X), "auto-dj a few columns in from the left edge: {line:?}");
        assert!(repeat < prev && prev < play && play < next && next < shuffle, "repeat, prev, play, next, shuffle: {line:?}");
        let play = play.unwrap();
        assert_eq!(buf[(play, y)].fg, th().gold, "play's frame is gold");
        assert!(buf[(play + 2, y)].modifier.contains(Modifier::BOLD), "and its glyph bold");
        assert!(buf[(prev.unwrap() + 2, y)].modifier.contains(Modifier::BOLD), "prev's glyph is bold");
        assert!(!buf[(shuffle.unwrap() + 2, y)].modifier.contains(Modifier::BOLD), "shuffle off is dim");
        gui.ui.pointer = Some(Position { x: area.width - 20, y: y - 1 });
        let card = rows_of(&draw_buffer(&mut gui), y - 1);
        assert!(!card.contains("[⋯]"), "the hovered card shows no verb: {card:?}");
        assert_eq!(gui.ui.hit_context(Position { x: area.width - 20, y: y - 1 }), Some(Act::NowMore), "a right click is its sheet");
    }

    fn rows_of(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area().width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn the_top_bar_switches_between_the_library_and_now_playing() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut gui = browsing_gui();
        let rows = draw(&mut gui);
        assert!(!rows[0].contains("mStream"), "no wordmark: {:?}", rows[0]);
        let lx = rows[0].char_indices().position(|(i, _)| rows[0][i..].starts_with(" Library ")).unwrap() as u16;
        let nx = rows[0].char_indices().position(|(i, _)| rows[0][i..].starts_with(" Now Playing ")).unwrap() as u16;
        let buf = draw_buffer(&mut gui);
        assert_eq!(buf[(lx, 0)].bg, th().accent, "the Library tab wears the slab");
        assert_ne!(buf[(nx, 0)].bg, th().accent, "the other tab does not");
        assert!(rows.iter().any(|r| r.contains("Albums")), "the nav column is up");
        assert_eq!(gui.ui.hit(Position { x: nx + 1, y: 0 }), Some(Act::Screen(Screen::NowPlaying)));

        // Now Playing: the nav and the room go, the demo seat's track stands
        // large — its title, its byline and its facts under the cover slot.
        gui.act(Act::Screen(Screen::NowPlaying));
        let rows = draw(&mut gui);
        assert!(!rows.iter().any(|r| r.contains("Albums")), "the nav column is gone:\n{}", rows.join("\n"));
        // The stage's title starts its row (the bar's card has frames before
        // its own copy).
        let title = rows.iter().position(|r| r.trim_start().starts_with("Cassini IV")).expect("the title stands on its own row");
        assert!(rows[title + 1].contains("Vela — Cassini · Cassini · 2019"), "the byline: {:?}", rows[title + 1]);
        assert!(rows[title + 2].contains("FLAC · 912 kbps"), "the spec: {:?}", rows[title + 2]);
        assert!(rows[title + 3].contains("★★★★☆") && rows[title + 3].contains("120 BPM"), "the facts: {:?}", rows[title + 3]);
        assert!(rows[3].contains('╭'), "the cover slot waits for the art: {:?}", rows[3]);

        // Esc and 0 go back and forth; a nav digit is the Library's.
        super::handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(gui.screen, Screen::Library);
        super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        assert_eq!(gui.screen, Screen::NowPlaying);
        super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(gui.screen, Screen::Library);
    }

    #[test]
    fn the_demo_seat_yields_to_real_playback() {
        let mut gui = test_gui();
        assert_eq!(gui.bar_now().unwrap().title, "Cassini IV");
        gui.app.now_playing = Some(track("music/a.mp3", "Night Drive", 252.0));
        assert_eq!(gui.bar_now().unwrap().title, "Night Drive");
    }

    #[test]
    fn transport_and_toggles_forward_to_the_app() {
        let mut gui = browsing_gui();
        gui.act(Act::Shuffle);
        assert!(gui.app.queue.shuffle);
        gui.act(Act::Repeat);
        assert_ne!(gui.app.queue.repeat, crate::tui::app::Repeat::Off);
        gui.act(Act::VolSet(6));
        assert_eq!(gui.app.volume, 0.7);
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Audio(AudioCmd::SetVolume(v)) if (*v - 0.7).abs() < 0.001)),
            "the volume reached the engine as an effect"
        );
    }

    #[test]
    fn the_blend_walks_whole_seconds_and_snaps() {
        let mut gui = test_gui();
        gui.app.crossfade = 4.5;
        gui.adjust_blend(1);
        assert_eq!(gui.app.crossfade, 5.0, "a hand-written 4.5 steps to 5");
        gui.adjust_blend(-1);
        assert_eq!(gui.app.crossfade, 4.0, "and to 4, never 5.5 forever");
        gui.app.crossfade = 0.0;
        gui.adjust_blend(-1);
        assert_eq!(gui.app.crossfade, 0.0, "off is the floor");
    }

    #[test]
    fn no_server_is_said_in_words_not_a_blank_screen() {
        let mut gui = test_gui();
        gui.demo = None;
        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.nothing_playing").to_string()));
        let hint = t!("gui.no_server").to_string();
        let lead: String = hint.chars().take(20).collect();
        assert!(all.contains(&lead), "the Files room explains itself");
    }

    #[test]
    fn the_settings_cursor_picks_up_stows_and_clamps() {
        let mut gui = test_gui();
        gui.active = SETTINGS_NAV;
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        handle_key(&mut gui, down);
        assert_eq!(gui.cursor, Some(0), "↓ picks the cursor up at the top");
        for _ in 0..10 {
            handle_key(&mut gui, down);
        }
        assert_eq!(gui.cursor, Some(SET_ROWS - 1), "the bottom clamps");
        handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(gui.cursor, None, "Esc stows it");
        handle_key(&mut gui, up);
        assert_eq!(gui.cursor, Some(SET_ROWS - 1), "↑ picks it up at the bottom");
    }

    #[test]
    fn files_keys_drive_the_shared_pane() {
        // The pane arrives with the App's own resting cursor already
        // picked; ↓ walks it forward one — GUI keys ARE the TUI's keys.
        let mut gui = browsing_gui();
        let before = gui.app.files.state.selected().expect("the pane rests on a row");
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        handle_key(&mut gui, down);
        assert_eq!(gui.app.files.state.selected(), Some(before + 1), "↓ moved the shared cursor");
    }

    #[test]
    fn a_small_window_asks_for_room_instead_of_breaking() {
        let mut gui = test_gui();
        let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
        terminal.draw(|frame| render(frame, &mut gui)).unwrap();
        let all = rows(&terminal).join("\n");
        assert!(all.contains(&t!("resize").to_string()));
        assert!(!all.contains("auto-dj"), "no bar in a window this small");
    }
    // ── The browser bar ─────────────────────────────────────────────────

    #[test]
    fn the_bar_gates_its_verbs_on_playable_rows() {
        let mut gui = browsing_gui();
        gui.queue_open = false;
        let text = draw(&mut gui).join("\n");
        for verb in ["▸ play", "+ queue all", "⇄ shuffle", "/ filter"] {
            assert!(text.contains(verb), "missing {verb:?}:\n{text}");
        }
        assert!(text.contains("3 items"), "the count skips the parent row:\n{text}");
        let lines = draw(&mut gui);
        let controls: String = lines[3].chars().skip(17).collect();
        assert!(
            controls.starts_with("/ filter"),
            "the filter leads the controls line: {:?}",
            lines[3]
        );

        // A listing of containers keeps a clean bar — no play button with
        // nothing to play; the filter stays, folders are findable too.
        gui.app.files.set(vec![
            Entry::Parent,
            Entry::Dir { label: "Ambient".into(), path: "music/Ambient".into() },
            Entry::Dir { label: "Jazz".into(), path: "music/Jazz".into() },
        ]);
        let text = draw(&mut gui).join("\n");
        assert!(!text.contains("▸ play"), "nothing playable, no play:\n{text}");
        assert!(!text.contains("+ queue all"), "got:\n{text}");
        assert!(text.contains("/ filter"), "the filter is always real:\n{text}");
    }

    #[test]
    fn the_filter_narrows_live_and_enter_keeps_it() {
        let mut gui = browsing_gui();
        gui.queue_open = false;
        handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(gui.app.filtering, "f opens the App's own prompt");
        for c in "au".chars() {
            handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("1 of 3"), "the narrowed view never impersonates the whole:\n{text}");
        assert!(text.contains("Aurora"), "got:\n{text}");
        assert!(!text.contains("Night Drive"), "narrowed out:\n{text}");

        // Enter keeps the narrowed list; the bar wears the query as a chip
        // and the verbs act on what you see (clause 13).
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!gui.app.filtering, "typing is done");
        assert_eq!(gui.app.files.filter, "au", "the filter stands");
        gui.act(Act::BarPlay);
        assert_eq!(gui.app.queue.items.len(), 1, "what you see is what plays");
        assert_eq!(gui.app.queue.items[0].filepath, "music/b.mp3");

        // Esc clears the standing filter and the whole list returns.
        handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(gui.app.files.filter.is_empty(), "Esc lets go of it");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Night Drive"), "the whole list is back:\n{text}");
    }

    #[test]
    fn leaving_the_room_lets_go_of_the_filter() {
        // Changing screens with the prompt open (or a filter standing)
        // closes it and clears the value — a filter describes the list it
        // was typed against, and that list just left the screen.
        let mut gui = browsing_gui();
        gui.queue_open = false;
        handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        for c in "au".chars() {
            handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(gui.app.filtering);
        gui.act(Act::Nav(SETTINGS_NAV));
        assert!(!gui.app.filtering, "the prompt closed with the room");
        assert!(gui.app.files.filter.is_empty(), "and the value went with it");

        // A standing (submitted) filter goes the same way.
        gui.act(Act::Nav(FILES_NAV));
        handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        handle_key(&mut gui, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(gui.app.files.filter, "a");
        gui.act(Act::Nav(ALBUMS_NAV));
        assert!(gui.app.files.filter.is_empty(), "the standing filter cleared too");
    }

    #[test]
    fn the_bar_back_walks_up_and_the_crumb_leads() {
        let mut gui = browsing_gui();
        gui.queue_open = false;
        gui.app.path = "music".into();
        let lines = draw(&mut gui);
        assert!(lines[2].contains("◂"), "somewhere to go, so the way out shows: {:?}", lines[2]);
        assert!(lines[2].contains("Files ▸ music"), "got: {:?}", lines[2]);
        gui.act(Act::BarBack);
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Api(worker::ApiCmd::Browse(_)))),
            "back asks for the parent listing: {:?}",
            gui.pending
        );

        gui.app.path = String::new();
        gui.app.files.loading = false;
        gui.pending.clear();
        let lines = draw(&mut gui);
        assert!(!lines[2].contains("◂"), "at the root there is nowhere to go: {:?}", lines[2]);
    }

    #[test]
    fn a_drill_lets_go_of_the_filter() {
        // Clause 24: a filter describes the list it was typed against.
        let mut gui = browsing_gui();
        gui.queue_open = false;
        handle_key(&mut gui, KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        for c in "amb".chars() {
            handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(gui.app.files.filter, "amb");
        // The narrowed list holds the one folder; Enter opens it.
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let effects = gui.app.apply_event(Event::Listing(Box::new(
            crate::api::types::DirListing {
                path: "/music/Ambient".into(),
                directories: vec![],
                files: vec![],
                ..Default::default()
            },
        )));
        gui.pend(effects);
        assert!(
            gui.app.files.filter.is_empty(),
            "the drill let go of the filter: {:?}",
            gui.app.files.filter
        );
    }

    // ── The sonic path room ─────────────────────────────────────────────

    /// A connected App that can plot paths, standing in the sonic room.
    fn sonic_gui() -> Gui {
        let mut gui = test_gui();
        gui.app.connected = true;
        gui.app.capabilities.discovery_path = true;
        gui.active = SONIC_NAV;
        gui.app.tab = Tab::SonicPath;
        gui
    }

    fn stop(path: &str, artist: &str, title: &str, t: f64, similarity: f64) -> crate::api::types::JourneyStop {
        crate::api::types::JourneyStop {
            filepath: path.to_string(),
            t,
            similarity,
            metadata: TrackMetadata {
                artist: Some(artist.to_string()),
                title: Some(title.to_string()),
                ..TrackMetadata::default()
            },
        }
    }

    #[test]
    fn the_sonic_room_rides_the_pings_flag() {
        // Without the capability the row is absent and its digit is dead —
        // and the rooms that don't come and go keep their digits.
        let mut gui = test_gui();
        gui.app.connected = true;
        let text = draw(&mut gui).join("\n");
        assert!(!text.contains("Sonic path"), "got:\n{text}");
        gui.act(Act::Nav(SONIC_NAV));
        assert_ne!(gui.active, SONIC_NAV, "a dead digit goes nowhere");

        gui.app.capabilities.discovery_path = true;
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Sonic path"), "got:\n{text}");
        gui.act(Act::Nav(SONIC_NAV));
        assert_eq!(gui.active, SONIC_NAV);
    }

    #[test]
    fn the_setup_cards_offer_the_ways_in_and_build_waits_for_both() {
        let mut gui = sonic_gui();
        // At full width (queue folded) the methods sit inline on the card.
        gui.queue_open = false;
        let text = draw(&mut gui).join("\n");
        for needed in ["(not set)", "Use playing song", "Random song", "Browse library…"] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        assert!(text.contains("Build the journey"), "got:\n{text}");
        assert!(!text.contains("Build the journey ▸"), "not ready yet:\n{text}");

        gui.app.sonic.start = Some(track("lib/a.mp3", "Departure", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "Arrival", 210.0));
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Build the journey ▸"), "ready wears the arrow:\n{text}");
        // A filled card hides its methods and wears the clear instead.
        assert!(text.contains("[X]"), "got:\n{text}");
        assert!(!text.contains("Use playing song"), "filled cards hide the methods:\n{text}");

        // Squeezed by the open queue, an empty card offers the menu
        // instead of clipping the method row mid-word.
        gui.queue_open = true;
        gui.app.sonic.start = None;
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("click or Enter to choose…"), "got:\n{text}");
        assert!(!text.contains("Use playing song"), "no clipped inline row:\n{text}");
    }

    #[test]
    fn the_results_wear_seed_tags_meters_and_the_verbs() {
        let mut gui = sonic_gui();
        gui.app.sonic.start = Some(track("lib/a.mp3", "A", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "B", 210.0));
        gui.app.sonic.view = crate::tui::app::SonicView::Results;
        gui.app.sonic.fetched = true;
        gui.app.sonic.stops = vec![
            stop("lib/a.mp3", "Vela", "Cassini IV", 0.0, 1.0),
            stop("lib/m.mp3", "Nadir", "Aphelion", 0.5, 0.84),
            stop("lib/b.mp3", "Boukman", "6AM", 1.0, 1.0),
        ];
        let text = draw(&mut gui).join("\n");
        for needed in [
            "(start)", "(end)", "▇  84", "Play ▸", "Queue all", "Save as playlist…",
            "Regenerate", "Start over", "TRACK", "MATCH",
        ] {
            assert!(text.contains(needed), "missing {needed:?}:\n{text}");
        }
        assert!(text.contains("Vela - Cassini IV"), "got:\n{text}");
    }

    #[test]
    fn the_failure_states_name_themselves_and_retry_where_it_helps() {
        let mut gui = sonic_gui();
        gui.app.sonic.start = Some(track("lib/a.mp3", "A", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "B", 210.0));
        gui.app.sonic.view = crate::tui::app::SonicView::Results;

        gui.app.sonic.probe = true;
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Asking the server why…"), "got:\n{text}");
        assert!(!text.contains("Retry"), "no promises while probing:\n{text}");

        gui.app.sonic.probe = false;
        gui.app.sonic.fetched = true;
        gui.app.sonic.empty = crate::tui::app::SonicEmpty::ScanPending;
        gui.app.sonic.note =
            Some("the server hasn't analyzed any music yet — a path needs the discovery scan to have run".into());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("hasn't analyzed any music"), "got:\n{text}");
        assert!(text.contains("Retry"), "scan-pending is worth retrying:\n{text}");

        gui.app.sonic.empty = crate::tui::app::SonicEmpty::TurnedOff;
        gui.app.sonic.note = Some("sonic discovery has been switched off on this server".into());
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("switched off"), "got:\n{text}");
        assert!(!text.contains("Retry"), "nothing to retry when the feature is gone:\n{text}");
    }

    #[test]
    fn an_armed_pick_banners_the_browse_and_stows_the_queue_add() {
        let mut gui = browsing_gui();
        gui.app.capabilities.discovery_path = true;
        gui.app.capture = Some(crate::tui::app::Capture::Sonic(crate::tui::app::SonicSide::Start));
        // Hover the Aurora row: without an armed pick this reveals the [+].
        gui.ui.pointer = Some(Position { x: 30, y: 7 });
        let lines = draw(&mut gui);
        assert!(lines.join("\n").contains("Pick the start song"), "the banner is the mode");
        assert!(
            !lines[7].contains("[+]"),
            "an armed pick must not offer to queue: {:?}",
            lines[7]
        );

        gui.app.capture = None;
        let lines = draw(&mut gui);
        assert!(
            lines[7].contains("[+]"),
            "the hover [+] returns with the pick disarmed: {:?}",
            lines[7]
        );
    }

    #[test]
    fn a_browse_pick_lands_on_the_card_and_returns_home() {
        let mut gui = browsing_gui();
        gui.app.capabilities.discovery_path = true;
        gui.active = SONIC_NAV;
        gui.app.tab = Tab::SonicPath;
        gui.act(Act::SonBrowse(crate::tui::app::SonicSide::Start));
        assert_eq!(gui.active, FILES_NAV, "arming drops into the browser");
        assert!(gui.app.capture.is_some());

        // Clicking the Aurora track row answers the pick and goes home.
        gui.act(Act::PaneRow(List::Files, 3, RowVerb::Open));
        assert!(gui.app.capture.is_none(), "the pick was consumed");
        assert_eq!(gui.active, SONIC_NAV, "the answer returns to the room that asked");
        assert_eq!(
            gui.app.sonic.start.as_ref().map(|t| t.filepath.as_str()),
            Some("music/b.mp3")
        );
    }

    #[test]
    fn esc_cancels_a_pick_and_goes_home_too() {
        let mut gui = browsing_gui();
        gui.app.capabilities.discovery_path = true;
        gui.active = SONIC_NAV;
        gui.app.tab = Tab::SonicPath;
        gui.act(Act::SonBrowse(crate::tui::app::SonicSide::End));
        assert_eq!(gui.active, FILES_NAV);
        let quit = handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!quit);
        assert!(gui.app.capture.is_none(), "Esc disarms");
        assert_eq!(gui.active, SONIC_NAV, "and follows the cancel home");
        assert!(gui.app.sonic.end.is_none(), "nothing landed");
    }

    #[test]
    fn an_in_place_edit_in_results_rebuilds_immediately() {
        // Clause 22: the old journey is wrong the moment its anchor moved.
        let mut gui = sonic_gui();
        gui.app.sonic.start = Some(track("lib/a.mp3", "A", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "B", 210.0));
        gui.app.sonic.view = crate::tui::app::SonicView::Results;
        gui.app.sonic.fetched = true;
        gui.app.now_playing = Some(track("lib/n.mp3", "Now", 190.0));
        gui.act(Act::SonUse(crate::tui::app::SonicSide::Start));
        assert_eq!(gui.app.sonic.view, crate::tui::app::SonicView::Results, "rebuilt in place");
        assert!(gui.app.sonic.pending, "a fresh build is on the wire");
        assert!(
            gui.pending.iter().any(|e| matches!(
                e,
                Effect::Api(crate::tui::worker::ApiCmd::Journey { .. })
            )),
            "the rebuild was asked for"
        );
    }

    #[test]
    fn the_menu_opens_from_a_chip_and_the_keyboard_walks_it() {
        let mut gui = sonic_gui();
        gui.act(Act::SonMenu(crate::tui::app::SonicSide::End));
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("End song"), "the menu names its side:\n{text}");
        assert!(text.contains("Random song"), "got:\n{text}");
        // ↓ ↓ Enter picks Browse library — the third row.
        handle_key(&mut gui, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        handle_key(&mut gui, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        handle_key(&mut gui, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(gui.sonic.menu.is_none(), "the choice closes the menu");
        assert!(
            matches!(gui.app.capture, Some(crate::tui::app::Capture::Sonic(crate::tui::app::SonicSide::End))),
            "browse arms the capture for the side that asked"
        );
    }

    #[test]
    fn the_results_list_scrolls_under_the_wheel() {
        // The wheel scrolls the view, never the selection (the kit's table
        // law) — and the sonic list is one of the wheel's rooms, exactly
        // like Files. Regression: the room shipped without its arm in the
        // wheel router, so trackpad swipes did nothing here.
        let mut gui = sonic_gui();
        gui.app.sonic.start = Some(track("lib/a.mp3", "A", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "B", 210.0));
        gui.app.sonic.view = crate::tui::app::SonicView::Results;
        gui.app.sonic.fetched = true;
        gui.app.sonic.stops = (0..20)
            .map(|i| stop(&format!("lib/{i}.mp3"), "Artist", &format!("Stop {i:02}"), f64::from(i) / 19.0, 0.8))
            .collect();

        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Stop 00"), "the top of the list first:\n{text}");

        for _ in 0..6 {
            gui.act(Act::ScrollBy(List::Sonic, 1));
        }
        let text = draw(&mut gui).join("\n");
        assert!(!text.contains("Stop 00"), "the wheel moved the window:\n{text}");
        assert!(text.contains("Stop 07"), "later stops rolled into view:\n{text}");
        assert!(gui.sonic.rcursor.is_none(), "the wheel never touches the selection");

        // And back up past the top clamps instead of wrapping.
        for _ in 0..30 {
            gui.act(Act::ScrollBy(List::Sonic, -1));
        }
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Stop 00"), "scrolled home:\n{text}");
    }

    #[test]
    fn the_wheel_router_splits_queue_nav_and_room() {
        // The refactor's dividend: the router is a method, so the split is
        // testable instead of living inline in the event loop.
        let mut gui = sonic_gui();
        gui.app.sonic.view = crate::tui::app::SonicView::Results;
        gui.app.sonic.fetched = true;
        gui.app.sonic.stops = (0..20)
            .map(|i| stop(&format!("lib/{i}.mp3"), "A", &format!("S{i}"), 0.5, 0.8))
            .collect();
        draw(&mut gui); // seats last_width for the queue split

        // Over the content column, the active room answers.
        gui.wheel(Position { x: 40, y: 10 }, 1);
        assert_eq!(gui.sonic.results.scroll, 1, "the room's wheel");
        // Over the open queue, the queue answers — the room stands still.
        gui.wheel(Position { x: 90, y: 10 }, 1);
        assert_eq!(gui.queue_view.scroll, 1, "the queue's wheel");
        assert_eq!(gui.sonic.results.scroll, 1, "the room did not move");
        // Over the nav column, nothing scrolls.
        gui.wheel(Position { x: 5, y: 10 }, 1);
        assert_eq!((gui.sonic.results.scroll, gui.queue_view.scroll), (1, 1), "the nav scrolls nothing");
        // With the queue folded, its columns belong to the room again.
        gui.queue_open = false;
        gui.wheel(Position { x: 90, y: 10 }, 1);
        assert_eq!(gui.sonic.results.scroll, 2, "the split follows the fold");
        assert_eq!(gui.queue_view.scroll, 1);
    }

    #[test]
    fn the_save_prompt_rides_the_apps_line() {
        let mut gui = sonic_gui();
        gui.app.sonic.start = Some(track("lib/a.mp3", "Departure", 200.0));
        gui.app.sonic.end = Some(track("lib/b.mp3", "Arrival", 210.0));
        gui.app.sonic.view = crate::tui::app::SonicView::Results;
        gui.app.sonic.fetched = true;
        gui.app.sonic.stops = vec![
            stop("lib/a.mp3", "V", "Departure", 0.0, 1.0),
            stop("lib/b.mp3", "B", "Arrival", 1.0, 1.0),
        ];
        gui.act(Act::SonSave);
        assert!(gui.app.sonic_playlist_name.is_some(), "the prompt opened, pre-filled");
        let text = draw(&mut gui).join("\n");
        assert!(text.contains("Save as playlist"), "got:\n{text}");
        assert!(
            text.contains("Departure → Arrival"),
            "the suggested name is the journey's own:\n{text}"
        );
        handle_key(&mut gui, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(gui.app.sonic_playlist_name.is_none(), "Esc closes it");
    }
}

/// A developer's eyeball: `cargo test dump_frames -- --ignored --nocapture`
/// prints both bars as full frames, the closest thing to a screenshot a
/// TestBackend gives. Ignored so the battery never spends time on it.
#[cfg(test)]
mod dump_tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The album wall, same eyeball:
    /// `cargo test dump_album_wall -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_album_wall() {
        use crate::api::types::{Album, Track, TrackMetadata};
        use crate::tui::worker::{LibraryData, LibraryNode};

        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.app.connected = true;
        gui.queue_open = false;
        gui.demo = Some(demo_now());
        gui.act(Act::Nav(ALBUMS_NAV));
        let names = [
            "Random Access Memories",
            "Currents",
            "Discovery",
            "In Rainbows",
            "Lonerism",
            "Homework",
            "Blackstar",
            "Kid A",
            "Kind of Blue",
            "Aja",
            "Rumours",
            "Untrue",
        ];
        let albums: Vec<Album> = names
            .iter()
            .enumerate()
            .map(|(i, name)| Album {
                name: Some(name.to_string()),
                artist: Some(format!("Artist {i:02}")),
                year: Some(1959 + i as i32 * 5),
                album_art_file: Some(format!("aa{i:02}.jpeg")),
            })
            .collect();
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Albums,
            dest: Tab::Library,
            data: LibraryData::Albums(albums),
        });
        gui.pend(effects);
        // Distinct covers, so the mosaic shading differs cell to cell.
        for i in 0..names.len() as u32 {
            let mut pixels = image::RgbImage::new(64, 64);
            for (x, y, pixel) in pixels.enumerate_pixels_mut() {
                let v = ((x * (5 + i)) ^ (y * (11 + i))) as u8;
                *pixel = image::Rgb([v, 255 - v, (i * 23) as u8]);
            }
            let mut bytes = std::io::Cursor::new(Vec::new());
            pixels.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
            let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
            gui.app.art.insert(format!("aa{i:02}.jpeg"), Some(art));
        }

        let dump = |gui: &mut Gui, title: &str| {
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| render(frame, gui)).unwrap();
            let buffer = terminal.backend().buffer();
            println!("==== {title} ====");
            for y in 0..30u16 {
                let row: String = (0..100u16).map(|x| buffer[(x, y)].symbol()).collect();
                println!("|{row}|");
            }
        };
        dump(&mut gui, "album wall, page 1");
        gui.act(Act::AlbPage(1));
        dump(&mut gui, "album wall, page 2");

        gui.act(Act::AlbPage(-1));
        gui.act(Act::AlbCell(2)); // Discovery
        let effects = gui.app.apply_event(Event::Library {
            node: LibraryNode::Album {
                name: "Discovery".into(),
                artist: Some("Artist 02".into()),
            },
            dest: Tab::Library,
            data: LibraryData::Tracks(
                ["One More Time", "Aerodynamic", "Digital Love", "Harder Better"]
                    .iter()
                    .enumerate()
                    .map(|(i, title)| Track {
                        filepath: format!("music/d/{i}.mp3"),
                        metadata: TrackMetadata {
                            title: Some(title.to_string()),
                            duration: Some(200.0 + i as f64 * 40.0),
                            ..Default::default()
                        },
                    })
                    .collect(),
            ),
        });
        gui.pend(effects);
        dump(&mut gui, "album tracks");
    }

    /// The servers surfaces, same eyeball:
    /// `cargo test dump_server_frames -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_server_frames() {
        let mut config = Config::default();
        config.servers = vec![
            config::ServerEntry {
                url: "http://attic.local:3000".into(),
                username: Some("paul".into()),
                ..Default::default()
            },
            config::ServerEntry { url: "http://office.local:3000".into(), ..Default::default() },
        ];
        config.default_server = Some("http://attic.local:3000".into());
        let mut gui = Gui::new(
            config,
            false,
            App::new(Some("http://attic.local:3000".into()), None, None),
        );
        gui.app.connected = true;
        gui.demo = Some(demo_now());
        gui.active = SETTINGS_NAV;

        let dump = |gui: &mut Gui, title: &str| {
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| render(frame, gui)).unwrap();
            let buffer = terminal.backend().buffer();
            println!("==== {title} ====");
            for y in 0..30u16 {
                let row: String = (0..100u16).map(|x| buffer[(x, y)].symbol()).collect();
                println!("|{row}|");
            }
        };

        dump(&mut gui, "settings + manage row");
        gui.act(Act::SrvMenu);
        dump(&mut gui, "header dropdown");
        gui.act(Act::SrvCloseDrop);
        gui.act(Act::Row(ROW_MANAGE));
        dump(&mut gui, "manage servers room");
        gui.act(Act::SrvAdd);
        dump(&mut gui, "add chooser");
        gui.act(Act::FormMethod(1));
        servers::observe(
            &mut gui,
            &Event::ServersDiscovered(vec![crate::discovery::DiscoveredServer {
                name: "attic".into(),
                base_url: "http://attic.local:3000".into(),
                version: Some("5.13.2".into()),
                quick_connect: true,
            }]),
        );
        dump(&mut gui, "quick connect page");
        gui.act(Act::FormBack);
        gui.act(Act::FormMethod(0));
        gui.act(Act::FormToggle(4));
        dump(&mut gui, "standard page, public checked");
    }

    #[test]
    #[ignore]
    fn dump_frames() {
        let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
        gui.demo = Some(demo_now());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, &mut gui)).unwrap();
        let buffer = terminal.backend().buffer();
        println!("==== the bar ====");
        for y in 0..30u16 {
            let row: String = (0..100u16).map(|x| buffer[(x, y)].symbol()).collect();
            println!("|{row}|");
        }
    }

}
