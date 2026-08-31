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
//! This slice: the left nav with a WORKING Files browser (browse, click to
//! play, `a` to queue), the live queue panel, both bottom bars against real
//! playback, and the Settings room. `MSTREAM_GUI_DEMO=1` still seats a
//! fixed track for looking at the bars with no server at hand.
//!
//! Design: the "mStream Player GUI" canvas + docs/ui-kit.md.

mod albums;
mod bar;
mod playlists;
mod servers;
mod sonic;

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
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use rust_i18n::t;

use crate::config::{self, Config};
use crate::kit::{
    GroundGuard, POINTER_RESET, Surface, dim, input_display, scroll_list, set_pointer_shape,
    table_view,
};
use crate::kit::theme::{self, legacy_conhost, th};
use crate::tui::app::{
    Action, App, Effect, Entry, MessageKind, SEARCH_CLASSES, SearchClass, SearchNode, Tab,
};
use crate::tui::worker::{AudioCmd, AutoDjMode, Event};
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
    /// A Files row, clicked: select it and Activate (open the folder, or
    /// play from the track) — the TUI's Enter, aimed by the mouse.
    FileRow(usize),
    /// The hovered track row's revealed [+]: queue just that one.
    FileQueue(usize),
    /// The Files scrollbar (kit `scroll_list`): step and jump.
    FScrollBy(i32),
    FScrollTo(usize),
    /// A Search pane row, by PANE index (clicks map through the class
    /// filter): select and Activate — drill a class or an artist, or play
    /// from a track.
    SearchRow(usize),
    SearchQueue(usize),
    SScrollBy(i32),
    SScrollTo(usize),
    /// A class chip: put the chip cursor there and flip the class.
    Chip(usize),
    /// The query card: start (or resume) editing the search text.
    EditQuery,
    /// A settings row, activated (click, Enter, Space).
    Row(usize),
    BlendDown,
    BlendUp,
    // ── Albums (see gui::albums) ────────────────────────────────────────
    /// Turn the album wall a page: -1 back, +1 forward.
    AlbPage(i32),
    /// A cell on the current page, clicked: open that album.
    AlbCell(usize),
    /// The album track list: select + Activate (row 0 is the Parent row,
    /// so it doubles as Back), the hover [+], and the kit scrollbar.
    AlbTrackRow(usize),
    AlbTrackQueue(usize),
    AlbScrollBy(i32),
    AlbScrollTo(usize),
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
    /// A drilled track row, and its hover [+].
    PlTrackRow(usize),
    PlTrackQueue(usize),
    /// Whichever level is on screen scrolls.
    PlScrollBy(i32),
    PlScrollTo(usize),
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
    SonBuild,
    SonRegen,
    /// The failure states' Retry — Build by another name.
    SonRetry,
    SonStartOver,
    /// A stop row: play the journey from there. The hover [+] queues it.
    SonRow(usize),
    SonQueueStop(usize),
    SonScrollBy(i32),
    SonScrollTo(usize),
    SonPlay,
    SonQueueAll,
    SonSave,
    /// The save prompt's button pair; typing forwards to the App's line.
    SonSaveOk,
    SonSaveCancel,
    // ── Servers (see gui::servers) ──────────────────────────────────────
    /// The header's server label: toggle the switcher dropdown.
    SrvMenu,
    /// A dropdown row: switch the session to saved server `i`.
    SrvDrop(usize),
    /// Open the add-server form (the header [+], the dropdown's last row,
    /// the room's add row, the no-server screen's button).
    SrvAdd,
    SrvCloseDrop,
    /// Room rows: select, and the selected row's action words.
    SrvRow(usize),
    SrvSwitch(usize),
    SrvEdit(usize),
    SrvDefault(usize),
    SrvQr(usize),
    /// Opens the remove confirmation; the bool answers it.
    SrvRemove(usize),
    SrvConfirm(bool),
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
    /// A modal's whole-screen backdrop: swallow the click.
    Guard,
}

// ── Navigation ──────────────────────────────────────────────────────────────

/// The sidebar, in draw order. Files and Settings are the working rooms
/// this slice; the rest name where the tag-based browse lands and say so
/// when asked.
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
    /// Capability-gated, drawn under Search but LAST in the array: digits
    /// assign by index, and a room that comes and goes with the server
    /// must never renumber the rooms that don't (contract §1).
    Sonic,
}

const NAV: [NavId; 9] = [
    NavId::Files,
    NavId::Albums,
    NavId::Artists,
    NavId::Genres,
    NavId::Recent,
    NavId::Playlists,
    NavId::Search,
    NavId::Settings,
    NavId::Sonic,
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
        }
    }
}

const FILES_NAV: usize = 0;
const ALBUMS_NAV: usize = 1;
const SEARCH_NAV: usize = 6;
const PLAYLISTS_NAV: usize = 5;
const SETTINGS_NAV: usize = 7;
const SONIC_NAV: usize = 8;

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
const ROW_MANAGE: usize = 4;
const SET_ROWS: usize = 5;

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
    /// The bar's view of what is playing, rebuilt each pass from the App
    /// (or the demo seat when nothing real is on).
    bar_now: Option<Now>,
    /// `MSTREAM_GUI_DEMO=1`: a fixed track shown while the App is idle.
    demo: Option<Now>,
    demo_paused: bool,
    /// The Files list's wheel offset, and whether the keyboard cursor moved
    /// this pass (a move reveals; the wheel scrolls freely — the kit's
    /// `table_view` contract).
    fscroll: usize,
    freveal: bool,
    /// The Search pane's wheel offset and reveal flag, its own list.
    sscroll: usize,
    sreveal: bool,
    /// The chip cursor among the five class chips, and which classes the
    /// menu shows — the search params. The server answers every class in
    /// one reply, so the choice is instant and free to change.
    chip: usize,
    classes_on: [bool; 5],
    /// The queue panel's wheel offset, and the playing index last seen —
    /// the panel reveals the playing row only when it CHANGES, so the
    /// wheel can roam freely in between (the kit's table contract).
    qscroll: usize,
    last_current: Option<usize>,
    /// The size of the last drawn frame, for hit zones the event loop
    /// needs outside a draw (the wheel's queue-vs-content split, the
    /// album wall's grid geometry).
    last_width: u16,
    last_height: u16,
    /// The saved-server surfaces: dropdown, form, room, pairing QR.
    servers: servers::ServersUi,
    /// The album wall: its page, cell cursor, and per-slot cover caches.
    albums: albums::AlbumsUi,
    /// The sonic path room: its menu, setup cursor and results wheel.
    sonic: sonic::SonicUi,
    /// The playlists room: its dialogs and the two levels' wheels.
    playlists: playlists::PlaylistsUi,
    /// The last frame left paced work unfinished (covers still waiting to
    /// upgrade to pixels): the event loop shortens its idle wait so the
    /// next frame comes promptly instead of a poll tick later.
    hot: bool,
}

impl Gui {
    fn new(config: Config, config_ok: bool, app: App) -> Self {
        Gui {
            app,
            pending: Vec::new(),
            config,
            config_ok,
            ui: Surface::new(),
            active: FILES_NAV,
            cursor: None,
            queue_open: true,
            note: None,
            bar_now: None,
            demo: None,
            demo_paused: false,
            fscroll: 0,
            freveal: false,
            sscroll: 0,
            sreveal: false,
            chip: 0,
            classes_on: [true; 5],
            qscroll: 0,
            last_current: None,
            last_width: MIN_W,
            last_height: MIN_H,
            servers: servers::ServersUi::new(),
            albums: albums::AlbumsUi::new(),
            sonic: sonic::SonicUi::new(),
            playlists: playlists::PlaylistsUi::new(),
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
            _ => {}
        }
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
    fn act(&mut self, act: Act) -> bool {
        if servers::act(self, &act) {
            return false;
        }
        if albums::act(self, &act) {
            return false;
        }
        if sonic::act(self, &act) {
            return false;
        }
        if playlists::act(self, &act) {
            return false;
        }
        match act {
            Act::Nav(i) => {
                // The gated room: with the flag gone the row isn't drawn,
                // and its digit must be as dead as the row (contract §1).
                if i == SONIC_NAV && !self.app.capabilities.discovery_path {
                    return false;
                }
                self.active = i;
                // Leaving for a section stows every servers surface; the
                // room is a Settings sub-view, not a place to come back to.
                self.servers.drop_open = false;
                self.servers.room = false;
                self.note = (!matches!(
                    i,
                    FILES_NAV | ALBUMS_NAV | SEARCH_NAV | SETTINGS_NAV | SONIC_NAV
                        | PLAYLISTS_NAV
                ))
                .then(|| (t!("gui.coming", name = NAV[i].label()).to_string(), false));
                if i != SETTINGS_NAV {
                    self.cursor = None;
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
                    let duration = self.bar_now.as_ref().map_or(0.0, |n| n.duration);
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
            Act::FileRow(i) => {
                self.app.tab = Tab::Files;
                self.app.files.state.select(Some(i));
                self.freveal = true;
                self.forward_capturing(Action::Activate);
            }
            Act::FileQueue(i) => {
                self.app.tab = Tab::Files;
                self.app.files.state.select(Some(i));
                self.forward(Action::AddToQueue);
            }
            Act::FScrollBy(delta) => {
                self.fscroll =
                    if delta < 0 { self.fscroll.saturating_sub(1) } else { self.fscroll + 1 };
            }
            Act::FScrollTo(first) => self.fscroll = first,
            Act::SearchRow(i) => {
                self.app.tab = Tab::Search;
                self.app.search.state.select(Some(i));
                self.sreveal = true;
                self.forward_capturing(Action::Activate);
            }
            Act::SearchQueue(i) => {
                self.app.tab = Tab::Search;
                self.app.search.state.select(Some(i));
                self.forward(Action::AddToQueue);
            }
            Act::SScrollBy(delta) => {
                self.sscroll =
                    if delta < 0 { self.sscroll.saturating_sub(1) } else { self.sscroll + 1 };
            }
            Act::SScrollTo(first) => self.sscroll = first,
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
    /// waveform — or the demo seat while the App is idle.
    fn refresh_bar_now(&mut self) {
        self.bar_now = match &self.app.now_playing {
            Some(track) => {
                let duration = if self.app.status.duration > 0.0 {
                    self.app.status.duration
                } else {
                    track.metadata.duration.unwrap_or(0.0)
                };
                Some(Now {
                    title: track
                        .metadata
                        .display_title()
                        .map(str::to_string)
                        .unwrap_or_else(|| track.file_name().to_string()),
                    artist: track.metadata.artist.clone().unwrap_or_default(),
                    elapsed: self.app.status.position,
                    duration,
                })
            }
            None => self.demo.clone(),
        };
    }

    fn bar_paused(&self) -> bool {
        if self.app.now_playing.is_some() { self.app.status.paused } else { self.demo_paused }
    }
}

/// The demo seat (`MSTREAM_GUI_DEMO=1`): a fixed track shown while nothing
/// real is playing, so the bar can be seen and the seek ridden with no
/// server at hand. Same fiction as the design canvas.
fn demo_now() -> Now {
    Now {
        title: "Cassini IV".to_string(),
        artist: "Vela — Cassini".to_string(),
        elapsed: 47.0,
        duration: 302.0,
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

fn put(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    frame.render_widget(
        Paragraph::new(Span::styled(text.to_string(), style)),
        Rect { x, y, width, height: 1 },
    );
}

fn bright_bold() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
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
fn content_rect(width: u16, height: u16, queue_open: bool) -> Rect {
    let right = if queue_open { width - 36 } else { width - 3 };
    Rect { x: 17, y: 2, width: right - 17, height: height - 10 }
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
    gui.refresh_bar_now();
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

    put(frame, 1, 0, "mStream", Style::default().fg(th().accent).add_modifier(Modifier::BOLD));
    servers::draw_header(frame, gui, area);

    draw_nav(frame, gui, area);

    // The content column, between the nav rule and the queue (when open).
    let content = content_rect(area.width, area.height, gui.queue_open);
    match gui.active {
        FILES_NAV => draw_files(frame, gui, content),
        ALBUMS_NAV => albums::draw(frame, gui, content),
        SEARCH_NAV => draw_search(frame, gui, content),
        SETTINGS_NAV => draw_settings(frame, gui, content),
        SONIC_NAV => sonic::draw(frame, gui, content),
        PLAYLISTS_NAV => playlists::draw(frame, gui, content),
        i => {
            put(frame, content.x, content.y, &NAV[i].label(), Style::default().add_modifier(Modifier::BOLD));
            put(frame, content.x, content.y + 2, &t!("gui.coming", name = NAV[i].label()), dim());
        }
    }

    if gui.queue_open {
        draw_queue(frame, gui, area);
    }

    // The note sits above the bar (gui's own first, else the App's words);
    // the keyboard tips take the very bottom row. An armed pick outranks
    // both: the banner is the mode, not news (clauses 4, 10–13).
    if let Some(crate::tui::app::Capture::Sonic(side)) = gui.app.capture {
        let banner = match side {
            crate::tui::app::SonicSide::Start => t!("gui.sonic.pick_banner_start"),
            crate::tui::app::SonicSide::End => t!("gui.sonic.pick_banner_end"),
        };
        put(
            frame,
            1,
            area.height - 7,
            &bar::clip(&banner, area.width as usize - 2),
            Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        );
    } else if let Some((text, is_err)) = gui.note.clone().or_else(|| {
        gui.app
            .message
            .as_ref()
            .map(|m| (m.text.clone(), matches!(m.kind, MessageKind::Error)))
    }) {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        put(frame, 1, area.height - 7, &bar::clip(&text, area.width as usize - 2), style);
    }
    let tips = if gui.servers.modal_open() {
        t!("gui.tips.form")
    } else if sonic::modal_open(gui) {
        std::borrow::Cow::from(sonic::tips(gui))
    } else if playlists::modal_open(gui) {
        std::borrow::Cow::from(playlists::tips(gui))
    } else if matches!(gui.app.capture, Some(crate::tui::app::Capture::Sonic(_))) {
        t!("gui.tips.sonic_pick")
    } else if gui.servers.room && gui.active == SETTINGS_NAV {
        t!("gui.tips.servers")
    } else {
        match gui.active {
            SETTINGS_NAV if gui.cursor.is_some() => t!("gui.tips.rows"),
            FILES_NAV if gui.app.filtering => t!("gui.tips.filter"),
            FILES_NAV => t!("gui.tips.files"),
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
    put(frame, 1, area.height - 1, &tips, dim());

    // While the pairing QR is up, the card cover stands down: the graphics
    // encode cache holds ONE image, and two per frame thrash it.
    let has_art = playing_cover_ready(&gui.app) && gui.servers.qr.is_none();
    let view = BarView {
        now: gui.bar_now.as_ref(),
        paused: gui.bar_paused(),
        volume: gui.app.volume,
        shuffle: gui.app.queue.shuffle,
        repeat: gui.app.queue.repeat != crate::tui::app::Repeat::Off,
        autodj: gui.app.autodj != AutoDjMode::Off,
        queue_open: gui.queue_open,
        has_art,
    };
    bar::draw(frame, &mut gui.ui, area, &view);
    if has_art {
        draw_card_cover(frame, bar::cover_rect(area), &mut gui.app);
    }

    // Overlays draw (and register) last, so their rects win the pointer.
    playlists::draw_modals(frame, gui, area);
    sonic::draw_modals(frame, gui, area);
    servers::draw_dropdown(frame, gui, area);
    servers::draw_modals(frame, gui, area);

    // The tooltip draws over everything, once the dwell matures — the
    // wizard's order.
    if let Some((target, text)) = gui.ui.ripe_tooltip() {
        crate::kit::draw_tooltip(frame, area, target, text);
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
    let forward = if legacy_conhost() { ">" } else { "▸" };
    let set_y = area.height - 9;
    for (i, id) in NAV.iter().enumerate() {
        // The sonic room rides the ping's flag: absent is absent — no
        // placeholder row, and digit 9 goes dead with it (contract §1).
        if i == SONIC_NAV && !gui.app.capabilities.discovery_path {
            continue;
        }
        let y = match i {
            0 => 2,
            1..=5 => 4 + i as u16,
            6 => 11,
            SONIC_NAV => 12,
            _ => set_y,
        };
        let label = id.label();
        let active = i == gui.active;
        let x = if active { 1 } else { 3 };
        let text = if active { format!("{forward} {label}") } else { label };
        let rect = Rect { x, y, width: text.chars().count() as u16, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let style = match (active, hover) {
            (true, _) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => dim(),
        };
        put(frame, x, y, &text, style);
        gui.ui.click(rect, Act::Nav(i));
    }
    for y in 2..area.height - 8 {
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
        let text = if gui.app.connecting {
            (t!("busy.reaching").to_string(), accent())
        } else {
            (t!("gui.no_server").to_string(), dim())
        };
        put(frame, content.x, content.y + 2, &bar::clip(&text.0, content.width as usize), text.1);
        if !gui.app.connecting {
            // The way in, right where the absence is explained.
            let at = Rect { x: content.x, y: content.y + 4, width: content.width, height: 3 };
            crate::kit::tall_button(frame, &mut gui.ui, at, &t!("gui.srv.add"), true, Act::SrvAdd);
        }
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
    let reveal = gui.freveal.then_some(selected).flatten();
    gui.freveal = false;
    let (first, visible) = table_view(entries.len(), reveal, gui.fscroll, list.height as usize);
    gui.fscroll = first;

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
        Act::FileRow,
        Act::FileQueue,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::FScrollBy(-1),
        Act::FScrollBy(1),
        |first| Act::FScrollTo(first),
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
    let hover = gui.ui.pointer.is_some_and(|p| back.contains(p));
    let glyph = if legacy_conhost() { "<" } else { "◂" };
    put(frame, x, y, glyph, if hover { bright_bold() } else { dim() });
    gui.ui.click(back, Act::BarBack);
    gui.ui.tip(back, t!("gui.bar.back_tip").to_string());
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
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(frame, x, y, "[X]", if hover { bright_bold() } else { dim() });
        gui.ui.click(rect, Act::BarClear);
        gui.ui.tip(rect, t!("gui.bar.clear_tip").to_string());
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
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
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
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        put(frame, content.x, y, &label, if hover { bright_bold() } else { dim() });
        gui.ui.click(rect, Act::BarFilter);
        gui.ui.tip(rect, format!("{label} — f"));
    }

    // Right: the verbs, dropped from the tail when the room is squeezed
    // (shuffle first, then queue all — play holds out longest).
    if gui.app.pane().tracks_with_offset().0.is_empty() {
        return;
    }
    let forward_glyph = if legacy_conhost() { ">" } else { "▸" };
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
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
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
        gui.ui.tip(rect, format!("{label} — {key}"));
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

    let forward_glyph = if legacy_conhost() { ">" } else { "▸" };
    let mut x = content.x;
    if !gui.app.path.is_empty() {
        x += draw_bar_back(frame, gui, x, y);
    }
    let crumb = if gui.app.path.is_empty() {
        t!("gui.nav.files").to_string()
    } else {
        format!("{} {} {}", t!("gui.nav.files"), forward_glyph, gui.app.path)
    };
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
    row_act: fn(usize) -> Act,
    queue_act: fn(usize) -> Act,
    // An armed pick consumes the next activation outright (clause 10) —
    // the hover [+] must not offer to queue what a click would capture.
    queue_plus: bool,
) {
    for (row, (index, entry)) in rows.iter().enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let hover = ui.pointer.is_some_and(|p| rect.contains(p));
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
                    let mark = if legacy_conhost() { ">" } else { "▸" };
                    put(frame, list.x, y, mark, if is_sel { sel().add_modifier(Modifier::BOLD) } else { Style::default().fg(th().ok).add_modifier(Modifier::BOLD) });
                }
                put(frame, list.x + 2, y, &bar::clip(label, name_width), style);
                if hover && !is_sel && queue_plus {
                    let plus = Rect { x: rect.right() - 3, y, width: 3, height: 1 };
                    put(frame, plus.x, y, "[+]", dim());
                    ui.click(rect, row_act(*index));
                    ui.click(plus, queue_act(*index));
                    ui.tip(plus, t!("gui.files.queue_tip").to_string());
                } else {
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
                put(frame, list.x + 2, y, &bar::clip(other.label(), name_width), style);
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
        let text = if gui.app.connecting {
            (t!("busy.reaching").to_string(), accent())
        } else {
            (t!("gui.no_server").to_string(), dim())
        };
        put(frame, content.x, content.y, &bar::clip(&text.0, content.width as usize), text.1);
        return;
    }

    // The query card: a kit input — accent border and caret while it takes
    // keys, dim at rest, one click to wake it.
    let editing = gui.app.editing_query;
    let card = Rect { x: content.x, y: content.y, width: content.width, height: 3 };
    let card_hover = gui.ui.pointer.is_some_and(|p| card.contains(p));
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
    gui.ui.tip(card, t!("gui.search.edit_tip").to_string());

    // The class chips: toggle words wearing their state (the bar's toggle
    // grammar), the chip cursor as the one selection bg.
    let mut x = content.x;
    let chips_y = content.y + 3;
    for (i, class) in SEARCH_CLASSES.iter().enumerate() {
        let label = class_label(*class);
        let rect = Rect { x, y: chips_y, width: label.chars().count() as u16, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
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
    let reveal_row = gui
        .sreveal
        .then_some(selected)
        .flatten()
        .and_then(|sel| visible_rows.iter().position(|(i, _)| *i == sel));
    gui.sreveal = false;
    let (first, visible) = table_view(visible_rows.len(), reveal_row, gui.sscroll, list.height as usize);
    gui.sscroll = first;
    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.as_str());
    draw_pane_rows(
        frame,
        &mut gui.ui,
        playing,
        &visible_rows[first..first + visible],
        list,
        selected,
        Act::SearchRow,
        Act::SearchQueue,
        gui.app.capture.is_none(),
    );
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        visible_rows.len(),
        visible,
        first,
        Act::SScrollBy(-1),
        Act::SScrollBy(1),
        |first| Act::SScrollTo(first),
    );
}

/// The queue panel: the App's real queue, the playing row marked. Rows are
/// not yet clickable — jumping the queue lands with the queue slice.
fn draw_queue(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let x = area.width - 32;
    for y in 2..area.height - 8 {
        put(frame, area.width - 34, y, "│", dim());
    }
    put(frame, x, 2, &t!("gui.queue.title"), dim());
    put(frame, x, 3, &"─".repeat(31), dim());
    let items = &gui.app.queue.items;
    if items.is_empty() {
        put(frame, x, 4, &t!("gui.queue.empty"), dim());
        return;
    }
    let total: f64 = items.iter().filter_map(|t| t.metadata.duration).sum();
    let head = format!("{} · {}", items.len(), bar::fmt_time(total));
    put(frame, area.width - 2 - head.chars().count() as u16, 2, &head, dim());

    let avail = (area.height - 13) as usize;
    let current = gui.app.queue.current;
    let reveal = (current != gui.last_current).then_some(current).flatten();
    gui.last_current = current;
    let (first, visible) = table_view(items.len(), reveal, gui.qscroll, avail);
    gui.qscroll = first;
    for (row, (index, track)) in items.iter().enumerate().skip(first).take(visible).enumerate() {
        let y = 4 + row as u16;
        let is_current = gui.app.queue.current == Some(index);
        let title = track.metadata.display_title().unwrap_or_else(|| track.file_name());
        if is_current {
            let mark = if legacy_conhost() { ">" } else { "▸" };
            put(frame, x, y, mark, Style::default().fg(th().ok).add_modifier(Modifier::BOLD));
            put(frame, x + 2, y, &bar::clip(title, 22), Style::default().fg(th().ok).add_modifier(Modifier::BOLD));
        } else {
            put(frame, x + 2, y, &bar::clip(title, 22), Style::default());
        }
        let time = track.metadata.duration.map(bar::fmt_time).unwrap_or_default();
        put(frame, area.width - 2 - time.chars().count() as u16, y, &time, dim());
    }
}

fn draw_settings(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if gui.servers.room {
        return servers::draw_room(frame, gui, content);
    }
    let (check_on, check_off) = if legacy_conhost() { ("[x]", "[ ]") } else { ("[✓]", "[ ]") };
    put(frame, content.x, content.y, &t!("gui.set.playback"), dim());
    put(frame, content.x, content.y + 6, &t!("gui.set.servers_group"), dim());

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
            format!("{} {}", t!("gui.srv.manage"), if legacy_conhost() { ">" } else { "▸" }),
            t!("gui.srv.manage_desc").to_string(),
        ),
    ];

    for (i, (label, desc)) in rows.iter().enumerate() {
        let y = row_y(content.y, i);
        let rect = Rect { x: content.x, y, width: content.width, height: 1 };
        let selected = gui.cursor == Some(i);
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
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
        // full sentence returns the moment the queue folds away.
        let desc_x = content.x + 27;
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

/// Where settings row `i` draws, under its section label.
fn row_y(top: u16, i: usize) -> u16 {
    if i < ROW_MANAGE { top + 1 + i as u16 } else { top + 7 }
}

// ── Input ───────────────────────────────────────────────────────────────────

/// Keys, contextual and few — the letters the tips line names. Returns
/// true to quit.
fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    // The servers surfaces answer first: an open modal owns the keyboard
    // outright, the room takes its row keys, and everything else falls
    // through untouched.
    if let Some(quit) = servers::handle_key(gui, key) {
        return quit;
    }
    let browse = gui.browse_room()
        && gui.app.connected
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
                gui.freveal = true;
                gui.playlists.lreveal = true;
                gui.forward(Action::Down);
            }
            KeyCode::Up => {
                gui.freveal = true;
                gui.playlists.lreveal = true;
                gui.forward(Action::Up);
            }
            KeyCode::Char(c) => gui.forward(Action::Input(c)),
            _ => {}
        }
        return false;
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
                gui.sreveal = true;
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
            gui.freveal = true;
            gui.forward(Action::Down);
        }
        KeyCode::Up if files => {
            gui.freveal = true;
            gui.forward(Action::Up);
        }
        KeyCode::PageDown if files => {
            gui.freveal = true;
            gui.forward(Action::PageDown);
        }
        KeyCode::PageUp if files => {
            gui.freveal = true;
            gui.forward(Action::PageUp);
        }
        KeyCode::Enter if files => gui.forward_capturing(Action::Activate),
        KeyCode::Char('h') | KeyCode::Backspace if files => gui.forward(Action::Back),
        KeyCode::Char('a') if files => gui.forward(Action::AddToQueue),
        // Search browsing: the same pane keys as Files, plus the chip
        // cursor on ←/→ and `t` to flip the class under it.
        KeyCode::Down if search => {
            gui.sreveal = true;
            gui.forward(Action::Down);
        }
        KeyCode::Up if search => {
            gui.sreveal = true;
            gui.forward(Action::Up);
        }
        KeyCode::Enter if search => gui.forward_capturing(Action::Activate),
        KeyCode::Char('h') | KeyCode::Backspace if search => gui.forward(Action::Back),
        KeyCode::Char('a') if search => gui.forward(Action::AddToQueue),
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
    loop {
        // A SaveSession about to be dispatched writes the config behind
        // this copy's back — a Quick Connect add mints a whole new entry
        // there. Reload after, so the dropdown and the room list it.
        let saving = gui.pending.iter().any(|e| matches!(e, Effect::SaveSession));
        tui::dispatch(&gui.app, &mut gui.pending, audio_tx, api_tx, event_tx);
        if saving && let Ok(fresh) = config::load() {
            gui.config = fresh;
        }
        terminal.draw(|frame| render(frame, gui))?;

        while let Ok(ev) = event_rx.try_recv() {
            // The servers layer looks first: session answers that would
            // land on the TUI's connect screen open the GUI's form instead.
            servers::observe(gui, &ev);
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
                            if let Some(act) = gui.ui.hit(at) {
                                if gui.act(act) {
                                    return Ok(());
                                }
                            }
                            gui.ui.arm_bars(at);
                        }
                        MouseEventKind::Moved => gui.ui.motion(at),
                        MouseEventKind::Drag(_) => {
                            gui.ui.motion(at);
                            if let Some(act) = gui.ui.drag_action(at) {
                                gui.act(act);
                            }
                        }
                        MouseEventKind::Up(_) => gui.ui.release(),
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                            let delta = if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                            gui.wheel(at, delta);
                        }
                        _ => {}
                    }
                }
                // A resized window changes the cell-to-pixel mapping the
                // cover encodes against — the card's and every album
                // slot's alike.
                TermEvent::Resize(..) => {
                    gui.app.graphics.refresh();
                    gui.albums.on_resize();
                }
                _ => {}
            }
        }
    }
}

impl Gui {
    /// Where the queue panel begins, matched to `draw_queue`'s separator
    /// on the last drawn frame — the wheel's queue-vs-content split.
    fn queue_panel_x(&self) -> u16 {
        self.last_width.saturating_sub(34)
    }

    /// Which App tab the active browse room's pane rides — the bar's acts
    /// and keys seat it before forwarding (docs/ux-contracts/browser-top-bar.md).
    fn browse_tab(&self) -> Tab {
        if self.active == FILES_NAV { Tab::Files } else { Tab::Library }
    }

    /// Whether the active room wears the browse bar at all.
    fn browse_room(&self) -> bool {
        matches!(self.active, FILES_NAV | ALBUMS_NAV | PLAYLISTS_NAV)
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
            self.qscroll =
                if delta < 0 { self.qscroll.saturating_sub(1) } else { self.qscroll + 1 };
            return;
        }
        // The nav column scrolls nothing.
        if at.x < 17 {
            return;
        }
        match NAV[self.active] {
            NavId::Files => {
                self.act(Act::FScrollBy(delta));
            }
            NavId::Albums => albums::wheel(self, delta),
            NavId::Search => {
                self.act(Act::SScrollBy(delta));
            }
            NavId::Sonic => sonic::wheel(self, delta),
            NavId::Playlists => playlists::wheel(self, delta),
            // Nothing scrollable in these rooms — said here, on the
            // record, rather than by falling through a router.
            NavId::Artists | NavId::Genres | NavId::Recent | NavId::Settings => {}
        }
    }
}

pub fn run(server: Option<String>, token: Option<String>) -> i32 {
    // The GUI's own config read (the [gui] section, and the save guard);
    // `startup` below does its own tolerant load for the player prefs.
    let (config, config_ok) = match config::load() {
        Ok(config) => (config, true),
        Err(_) => (Config::default(), false),
    };

    let start = tui::startup(server, token);
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
        gui.act(Act::FileRow(1));
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
        gui.act(Act::FileQueue(2));
        assert_eq!(gui.app.queue.items.len(), 1, "the one track was queued");
        assert_eq!(gui.app.queue.items[0].filepath, "music/a.mp3");
    }

    #[test]
    fn the_queue_panel_shows_the_real_queue_with_the_playing_marker() {
        let mut gui = browsing_gui();
        gui.act(Act::FileQueue(2));
        gui.act(Act::FileQueue(3));
        gui.app.queue.current = Some(1);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("2 · 8:00"), "count and total time head the panel");
        assert!(all.contains("Aurora"), "queued titles are rows");
    }

    #[test]
    fn the_bar_reads_the_app_when_something_real_plays() {
        let mut gui = browsing_gui();
        gui.app.now_playing = Some(track("music/a.mp3", "Night Drive", 252.0));
        gui.app.status.position = 63.0;
        gui.app.status.duration = 252.0;
        gui.app.status.paused = false;
        gui.refresh_bar_now();
        let now = gui.bar_now.as_ref().unwrap();
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
        gui.act(Act::SearchRow(titles_row));
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
        let slot_row: String = rows[26].chars().skip(68).take(6).collect();
        assert!(slot_row.contains('╭'), "the slot frame waits for the art: {slot_row:?}");

        // A real decode, so the art carries what both draw paths want; the
        // TestBackend has no pixel protocol, so the ▀-mosaic is the path.
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        gui.app.art.insert("aa.jpeg".into(), Some(art));

        let rows = draw(&mut gui);
        let cover_row: String = rows[26].chars().skip(68).take(6).collect();
        assert!(!cover_row.contains('╭'), "the frame yields to the picture: {cover_row:?}");
        // A solid test image mosaics as full blocks; a busy one mixes ▀.
        assert!(
            cover_row.chars().all(|c| "█▀▄".contains(c)),
            "the mosaic holds the cells: {cover_row:?}"
        );
    }

    #[test]
    fn the_demo_seat_yields_to_real_playback() {
        let mut gui = test_gui();
        gui.refresh_bar_now();
        assert_eq!(gui.bar_now.as_ref().unwrap().title, "Cassini IV");
        gui.app.now_playing = Some(track("music/a.mp3", "Night Drive", 252.0));
        gui.refresh_bar_now();
        assert_eq!(gui.bar_now.as_ref().unwrap().title, "Night Drive");
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
        gui.act(Act::FileRow(3));
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
            gui.act(Act::SonScrollBy(1));
        }
        let text = draw(&mut gui).join("\n");
        assert!(!text.contains("Stop 00"), "the wheel moved the window:\n{text}");
        assert!(text.contains("Stop 07"), "later stops rolled into view:\n{text}");
        assert!(gui.sonic.rcursor.is_none(), "the wheel never touches the selection");

        // And back up past the top clamps instead of wrapping.
        for _ in 0..30 {
            gui.act(Act::SonScrollBy(-1));
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
        assert_eq!(gui.sonic.scroll, 1, "the room's wheel");
        // Over the open queue, the queue answers — the room stands still.
        gui.wheel(Position { x: 90, y: 10 }, 1);
        assert_eq!(gui.qscroll, 1, "the queue's wheel");
        assert_eq!(gui.sonic.scroll, 1, "the room did not move");
        // Over the nav column, nothing scrolls.
        gui.wheel(Position { x: 5, y: 10 }, 1);
        assert_eq!((gui.sonic.scroll, gui.qscroll), (1, 1), "the nav scrolls nothing");
        // With the queue folded, its columns belong to the room again.
        gui.queue_open = false;
        gui.wheel(Position { x: 90, y: 10 }, 1);
        assert_eq!(gui.sonic.scroll, 2, "the split follows the fold");
        assert_eq!(gui.qscroll, 1);
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
