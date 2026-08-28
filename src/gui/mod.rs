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

mod bar;

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
use crate::kit::{GroundGuard, POINTER_RESET, Surface, dim, scroll_list, set_pointer_shape, table_view};
use crate::kit::theme::{self, legacy_conhost, th};
use crate::tui::app::{Action, App, Effect, Entry, MessageKind, Tab};
use crate::tui::worker::{AudioCmd, AutoDjMode, Event};
use crate::tui::{self, worker};

use bar::{BarStyle, BarView, Now};

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
    /// A settings row, activated (click, Enter, Space).
    Row(usize),
    BlendDown,
    BlendUp,
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
}

const NAV: [NavId; 8] = [
    NavId::Files,
    NavId::Albums,
    NavId::Artists,
    NavId::Genres,
    NavId::Recent,
    NavId::Playlists,
    NavId::Search,
    NavId::Settings,
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
        }
    }
}

const FILES_NAV: usize = 0;
const SETTINGS_NAV: usize = 7;

// ── Settings rows ───────────────────────────────────────────────────────────

/// The Settings room's rows, by index: the bar radios, then the crossfade
/// group — the same knobs the classic TUI's Settings tab drives, read from
/// and written to the shared App.
const ROW_BAR_WAVE: usize = 0;
const ROW_BAR_GOLD: usize = 1;
const ROW_BLEND: usize = 2;
const ROW_GAPLESS: usize = 3;
const ROW_BLEND_SKIPS: usize = 4;
const ROW_PAUSE_FADE: usize = 5;
const SET_ROWS: usize = 6;

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
    bar_style: BarStyle,
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
    /// The queue panel's wheel offset, and the playing index last seen —
    /// the panel reveals the playing row only when it CHANGES, so the
    /// wheel can roam freely in between (the kit's table contract).
    qscroll: usize,
    last_current: Option<usize>,
    /// The width of the last drawn frame, for hit zones the event loop
    /// needs outside a draw (the wheel's queue-vs-content split).
    last_width: u16,
}

impl Gui {
    fn new(config: Config, config_ok: bool, app: App) -> Self {
        Gui {
            bar_style: BarStyle::from_config(&config.gui.bar),
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
            qscroll: 0,
            last_current: None,
            last_width: MIN_W,
        }
    }

    fn pend(&mut self, effects: Vec<Effect>) {
        self.pending.extend(effects);
    }

    fn forward(&mut self, action: Action) {
        let effects = self.app.handle_action(action);
        self.pend(effects);
    }

    /// Live state back into the config shapes (called before every save).
    /// Player prefs come from the App itself — the same rebuild the TUI's
    /// `remember` does — so the two front ends can never disagree.
    fn sync_config(&mut self) {
        self.config.gui.bar = self.bar_style.config_name().to_string();
        self.config.player.adopt(self.app.prefs());
    }

    /// Settings changes persist as they happen — a GUI that loses a choice
    /// to a crash feels broken in a way a TUI never quite does.
    fn save_now(&mut self) {
        if !self.config_ok {
            return;
        }
        self.sync_config();
        if let Err(e) = config::save(&self.config) {
            self.note = Some((t!("note.settings_save_failed", err = e).to_string(), true));
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

    fn set_bar(&mut self, style: BarStyle) {
        // Choosing is not toggling: the chosen radio ignores re-choice.
        if self.bar_style != style {
            self.bar_style = style;
            self.save_now();
        }
    }

    fn adjust_row(&mut self, row: usize, delta: i32) {
        match row {
            ROW_BAR_WAVE => self.set_bar(BarStyle::Wave),
            ROW_BAR_GOLD => self.set_bar(BarStyle::GoldLine),
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
        match act {
            Act::Nav(i) => {
                self.active = i;
                self.note = (!matches!(i, FILES_NAV | SETTINGS_NAV))
                    .then(|| (t!("gui.coming", name = NAV[i].label()).to_string(), false));
                if i != SETTINGS_NAV {
                    self.cursor = None;
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
            Act::FileRow(i) => {
                self.app.tab = Tab::Files;
                self.app.files.state.select(Some(i));
                self.freveal = true;
                self.forward(Action::Activate);
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
            // Activation is the TUI's Enter: toggles flip, the blend steps
            // up, radios choose.
            Act::Row(i) => self.adjust_row(i, 1),
            Act::BlendDown => self.adjust_blend(-1),
            Act::BlendUp => self.adjust_blend(1),
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
                    wave: self.app.waveforms.get(&track.filepath).and_then(|w| w.clone()),
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
/// real is playing, so the bars can be seen and the seek ridden with no
/// server at hand. Same fiction as the design canvas.
fn demo_now() -> Now {
    // Two beating waves multiplied, so the amplitudes swing the whole
    // ▁..█ range the way music does instead of hovering near the top.
    let wave: Vec<u8> = (0..128)
        .map(|i| {
            let i = i as f32;
            (((i * 0.7).sin() * (i * 0.23).cos()).abs() * 235.0 + 20.0) as u8
        })
        .collect();
    Now {
        title: "Cassini IV".to_string(),
        artist: "Vela — Cassini".to_string(),
        elapsed: 47.0,
        duration: 302.0,
        wave: Some(wave),
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
    let area = frame.area();
    gui.last_width = area.width;
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
    if gui.app.connected {
        let server = crate::quickconnect::display_server(&gui.app.session.server_id);
        let width = server.chars().count() as u16;
        put(frame, area.width - 2 - width, 0, &server, dim());
    }

    draw_nav(frame, gui, area);

    // The content column, between the nav rule and the queue (when open).
    let right = if gui.queue_open { area.width - 36 } else { area.width - 3 };
    let content = Rect { x: 17, y: 2, width: right - 17, height: area.height - 10 };
    match gui.active {
        FILES_NAV => draw_files(frame, gui, content),
        SETTINGS_NAV => draw_settings(frame, gui, content),
        i => {
            put(frame, content.x, content.y, &NAV[i].label(), Style::default().add_modifier(Modifier::BOLD));
            put(frame, content.x, content.y + 2, &t!("gui.coming", name = NAV[i].label()), dim());
        }
    }

    if gui.queue_open {
        draw_queue(frame, gui, area);
    }

    // The note sits above the bar (gui's own first, else the App's words);
    // the keyboard tips take the very bottom row.
    let note = gui.note.clone().or_else(|| {
        gui.app
            .message
            .as_ref()
            .map(|m| (m.text.clone(), matches!(m.kind, MessageKind::Error)))
    });
    if let Some((text, is_err)) = note {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        put(frame, 1, area.height - 7, &bar::clip(&text, area.width as usize - 2), style);
    }
    let tips = match gui.active {
        SETTINGS_NAV if gui.cursor.is_some() => t!("gui.tips.rows"),
        FILES_NAV => t!("gui.tips.files"),
        _ => t!("gui.tips.base"),
    };
    put(frame, 1, area.height - 1, &tips, dim());

    let view = BarView {
        now: gui.bar_now.as_ref(),
        paused: gui.bar_paused(),
        volume: gui.app.volume,
        shuffle: gui.app.queue.shuffle,
        repeat: gui.app.queue.repeat != crate::tui::app::Repeat::Off,
        autodj: gui.app.autodj != AutoDjMode::Off,
        queue_open: gui.queue_open,
    };
    bar::draw(frame, &mut gui.ui, area, gui.bar_style, &view);
}

fn draw_nav(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    put(frame, 1, 4, &t!("gui.nav.library"), dim());
    let forward = if legacy_conhost() { ">" } else { "▸" };
    let set_y = area.height - 9;
    for (i, id) in NAV.iter().enumerate() {
        let y = match i {
            0 => 2,
            1..=5 => 4 + i as u16,
            6 => 11,
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
    let crumb = if gui.app.path.is_empty() {
        t!("gui.nav.files").to_string()
    } else {
        format!("{} {} {}", t!("gui.nav.files"), if legacy_conhost() { ">" } else { "▸" }, gui.app.path)
    };
    put(frame, content.x, content.y, &clip_lead(&crumb, content.width as usize - 12), dim());

    if !gui.app.connected {
        let text = if gui.app.connecting {
            (t!("busy.reaching").to_string(), accent())
        } else {
            (t!("gui.no_server").to_string(), dim())
        };
        put(frame, content.x, content.y + 2, &bar::clip(&text.0, content.width as usize), text.1);
        return;
    }
    if gui.app.files.loading {
        put(frame, content.x, content.y + 2, &t!("busy.listing"), accent());
        return;
    }

    let entries = &gui.app.files.entries;
    if entries.is_empty() {
        put(frame, content.x, content.y + 2, &t!("gui.files.empty"), dim());
        return;
    }
    put(
        frame,
        content.right() - 12,
        content.y,
        &format!("{:>10}", t!("gui.files.items", count = entries.len())),
        dim(),
    );

    let list = Rect {
        x: content.x,
        y: content.y + 2,
        width: content.width - 2,
        height: content.height - 2,
    };
    let selected = gui.app.files.state.selected();
    let reveal = gui.freveal.then_some(selected).flatten();
    gui.freveal = false;
    let (first, visible) = table_view(entries.len(), reveal, gui.fscroll, list.height as usize);
    gui.fscroll = first;

    let playing = gui.app.now_playing.as_ref().map(|t| t.filepath.clone());
    let mut rows: Vec<(usize, Entry)> = Vec::with_capacity(visible);
    for (offset, entry) in entries.iter().enumerate().skip(first).take(visible) {
        rows.push((offset, entry.clone()));
    }
    for (row, (index, entry)) in rows.iter().enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let hover = gui.ui.pointer.is_some_and(|p| rect.contains(p));
        let is_sel = *index == selected.unwrap_or(usize::MAX);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let name_width = list.width as usize - 10;
        match entry {
            Entry::Track { label, track } => {
                let is_playing = playing.as_deref() == Some(track.filepath.as_str());
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
                if hover && !is_sel {
                    let plus = Rect { x: rect.right() - 3, y, width: 3, height: 1 };
                    put(frame, plus.x, y, "[+]", dim());
                    gui.ui.click(rect, Act::FileRow(*index));
                    gui.ui.click(plus, Act::FileQueue(*index));
                    gui.ui.tip(plus, t!("gui.files.queue_tip").to_string());
                } else {
                    let time = track.metadata.duration.map(bar::fmt_time).unwrap_or_default();
                    let tstyle = if is_sel { sel() } else { dim() };
                    put(frame, rect.right() - 1 - time.chars().count() as u16, y, &time, tstyle);
                    gui.ui.click(rect, Act::FileRow(*index));
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
                gui.ui.click(rect, Act::FileRow(*index));
            }
        }
    }
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right() - 1, y: list.y, width: 1, height: list.height },
        entries.len(),
        visible,
        first,
        Act::FScrollBy(-1),
        Act::FScrollBy(1),
        |first| Act::FScrollTo(first),
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
        let title = track
            .metadata
            .display_title()
            .map(str::to_string)
            .unwrap_or_else(|| track.file_name().to_string());
        if is_current {
            let mark = if legacy_conhost() { ">" } else { "▸" };
            put(frame, x, y, mark, Style::default().fg(th().ok).add_modifier(Modifier::BOLD));
            put(frame, x + 2, y, &bar::clip(&title, 22), Style::default().fg(th().ok).add_modifier(Modifier::BOLD));
        } else {
            put(frame, x + 2, y, &bar::clip(&title, 22), Style::default());
        }
        let time = track.metadata.duration.map(bar::fmt_time).unwrap_or_default();
        put(frame, area.width - 2 - time.chars().count() as u16, y, &time, dim());
    }
}

fn draw_settings(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let (radio_on, radio_off, check_on, check_off) = if legacy_conhost() {
        ("(*)", "( )", "[x]", "[ ]")
    } else {
        ("(•)", "( )", "[✓]", "[ ]")
    };
    put(frame, content.x, content.y, &t!("gui.set.bar_group"), dim());
    put(frame, content.x, content.y + 4, &t!("gui.set.playback"), dim());

    let rows: [(String, String); SET_ROWS] = [
        (
            format!(
                "{} {}",
                if gui.bar_style == BarStyle::Wave { radio_on } else { radio_off },
                t!("gui.set.bar_wave")
            ),
            t!("gui.set.bar_wave_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.bar_style == BarStyle::GoldLine { radio_on } else { radio_off },
                t!("gui.set.bar_gold")
            ),
            t!("gui.set.bar_gold_desc").to_string(),
        ),
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
    if i < 2 { top + 1 + i as u16 } else { top + 5 + (i as u16 - 2) }
}

// ── Input ───────────────────────────────────────────────────────────────────

/// Keys, contextual and few — the letters the tips line names. Returns
/// true to quit.
fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    let settings = gui.active == SETTINGS_NAV;
    let files = gui.active == FILES_NAV;
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Tab => return gui.act(Act::ToggleQueue),
        KeyCode::Char(c @ '1'..='8') => {
            return gui.act(Act::Nav(c as usize - '1' as usize));
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
        KeyCode::Enter if files => gui.forward(Action::Activate),
        KeyCode::Char('h') | KeyCode::Backspace if files => gui.forward(Action::Back),
        KeyCode::Char('a') if files => gui.forward(Action::AddToQueue),
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
        tui::dispatch(&gui.app, &mut gui.pending, audio_tx, api_tx, event_tx);
        terminal.draw(|frame| render(frame, gui))?;

        while let Ok(ev) = event_rx.try_recv() {
            let effects = gui.app.apply_event(ev);
            gui.pend(effects);
        }

        let over = gui.ui.hovering_clickable();
        if over != hand {
            hand = over;
            set_pointer_shape(hand, mouse_on);
        }
        if let Some(act) = gui.ui.hold_action() {
            gui.act(act);
        }
        gui.ui.dwell_tick();

        if !event::poll(POLL)? {
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
                            gui.ui.pointer = Some(at);
                            let delta = if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 };
                            // The wheel scrolls the view under the pointer,
                            // never the selection (the kit's table law).
                            if at.x >= gui.queue_panel_x() && gui.queue_open {
                                gui.qscroll = if delta < 0 {
                                    gui.qscroll.saturating_sub(1)
                                } else {
                                    gui.qscroll + 1
                                };
                            } else if gui.active == FILES_NAV && at.x >= 17 {
                                gui.act(Act::FScrollBy(delta));
                            }
                        }
                        _ => {}
                    }
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
        gui.app.waveforms.insert("music/a.mp3".into(), Some(vec![10, 200, 40, 180]));
        gui.refresh_bar_now();
        let now = gui.bar_now.as_ref().unwrap();
        assert_eq!(now.title, "Night Drive");
        assert_eq!(now.elapsed, 63.0);
        assert!(now.wave.is_some(), "the waveform came from the App's cache");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("1:03") && all.contains("4:12"), "real timestamps on the bar");
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
    fn the_bar_radio_switches_and_remembers() {
        let mut gui = test_gui();
        assert_eq!(gui.bar_style, BarStyle::Wave);
        gui.act(Act::Row(ROW_BAR_GOLD));
        assert_eq!(gui.bar_style, BarStyle::GoldLine);
        gui.sync_config();
        assert_eq!(gui.config.gui.bar, "gold-line");
        gui.act(Act::Row(ROW_BAR_GOLD));
        assert_eq!(gui.bar_style, BarStyle::GoldLine, "choosing the chosen is not a toggle");
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
}

/// A developer's eyeball: `cargo test dump_frames -- --ignored --nocapture`
/// prints both bars as full frames, the closest thing to a screenshot a
/// TestBackend gives. Ignored so the battery never spends time on it.
#[cfg(test)]
mod dump_tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    #[ignore]
    fn dump_frames() {
        for style in [BarStyle::Wave, BarStyle::GoldLine] {
            let mut gui = Gui::new(Config::default(), false, App::new(None, None, None));
            gui.demo = Some(demo_now());
            gui.bar_style = style;
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
            terminal.draw(|frame| render(frame, &mut gui)).unwrap();
            let buffer = terminal.backend().buffer();
            println!("==== {:?} ====", style);
            for y in 0..30u16 {
                let row: String = (0..100u16).map(|x| buffer[(x, y)].symbol()).collect();
                println!("|{row}|");
            }
        }
    }
}
