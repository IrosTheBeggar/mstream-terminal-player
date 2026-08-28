//! The GUI player — the mouse-first surface the Windows/macOS installers
//! launch in the branded terminal window (`mstream-player gui`).
//!
//! Built the wizard's way: the kit's fixed palette and OSC 11 ground lease,
//! a `Surface` per frame, every action clickable AND keyed. This slice is
//! the shell — the left nav, the two bottom bars (a Settings choice), and
//! the Settings room itself, wired to the real audio engine. Browse, queue
//! and playback arrive in the next slices; until then the bar rests idle
//! (`MSTREAM_GUI_DEMO=1` seats a demo track so the bars can be seen).
//!
//! Design: the "mStream Player GUI" canvas + docs/ui-kit.md.

mod bar;

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
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
use crate::kit::{GroundGuard, POINTER_RESET, Surface, dim, set_pointer_shape};
use crate::kit::theme::{self, legacy_conhost, th};
use crate::tui::worker::{AudioCmd, Event, spawn_audio};

use bar::{BarStyle, BarView, Now};

/// Below this the layout has nowhere honest to put the bar. The installer's
/// own window is 100×30; anyone smaller is asked for more room, like the
/// wizard.
const MIN_W: u16 = 100;
const MIN_H: u16 = 24;

const POLL: Duration = Duration::from_millis(100);
const VOLUME_STEP: f32 = 0.05;

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
    /// A settings row, activated (click, Enter, Space).
    Row(usize),
    BlendDown,
    BlendUp,
}

// ── Navigation ──────────────────────────────────────────────────────────────

/// The sidebar, in draw order. Settings is the working room this slice;
/// the rest name where browse lands and say so when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NavId {
    Albums,
    Artists,
    Genres,
    Recent,
    Playlists,
    Search,
    Settings,
}

const NAV: [NavId; 7] = [
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

const SETTINGS_NAV: usize = 6;

// ── Settings rows ───────────────────────────────────────────────────────────

/// The Settings room's rows, by index: the bar radios, then the crossfade
/// group — the same knobs the classic TUI's Settings tab drives.
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
    paused: bool,
    now: Option<Now>,
    volume: f32,
    shuffle: bool,
    repeat: bool,
    autodj: bool,
    crossfade: f32,
    gapless: bool,
    blend_skips: bool,
    pause_fade: bool,
    /// One line above the tips: (text, is_error).
    note: Option<(String, bool)>,
    audio: Sender<AudioCmd>,
}

impl Gui {
    fn new(config: Config, config_ok: bool, audio: Sender<AudioCmd>) -> Self {
        let p = &config.player;
        let gui = Gui {
            bar_style: BarStyle::from_config(&config.gui.bar),
            volume: p.volume.clamp(0.0, 1.0),
            shuffle: p.shuffle,
            repeat: p.repeat != "off",
            autodj: p.autodj != "off",
            crossfade: p.crossfade_seconds.clamp(0.0, 30.0),
            gapless: p.gapless,
            blend_skips: p.blend_skips,
            pause_fade: p.pause_fade,
            config,
            config_ok,
            ui: Surface::new(),
            active: SETTINGS_NAV,
            cursor: None,
            queue_open: true,
            paused: true,
            now: None,
            note: None,
            audio,
        };
        // The engine starts from the config, exactly like the TUI's five
        // settings pushes on connect.
        gui.send(AudioCmd::SetVolume(gui.volume));
        gui.send(AudioCmd::SetCrossfade(gui.crossfade));
        gui.send(AudioCmd::SetGapless(gui.gapless));
        gui.send(AudioCmd::SetBlendSkips(gui.blend_skips));
        gui.send(AudioCmd::SetPauseFade(gui.pause_fade));
        gui
    }

    fn send(&self, cmd: AudioCmd) {
        let _ = self.audio.send(cmd);
    }

    /// Live state back into the config shapes (called before every save).
    fn sync_config(&mut self) {
        self.config.gui.bar = self.bar_style.config_name().to_string();
        let p = &mut self.config.player;
        p.volume = (self.volume * 100.0).round() / 100.0;
        p.shuffle = self.shuffle;
        p.repeat = if self.repeat { "all" } else { "off" }.to_string();
        p.autodj = if self.autodj { "similar" } else { "off" }.to_string();
        p.crossfade_seconds = self.crossfade;
        p.gapless = self.gapless;
        p.blend_skips = self.blend_skips;
        p.pause_fade = self.pause_fade;
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
        let snapped = if delta > 0 { self.crossfade.floor() + 1.0 } else { self.crossfade.ceil() - 1.0 };
        self.crossfade = snapped.clamp(0.0, 30.0);
        self.send(AudioCmd::SetCrossfade(self.crossfade));
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
                self.gapless = !self.gapless;
                self.send(AudioCmd::SetGapless(self.gapless));
                self.save_now();
            }
            ROW_BLEND_SKIPS => {
                self.blend_skips = !self.blend_skips;
                self.send(AudioCmd::SetBlendSkips(self.blend_skips));
                self.save_now();
            }
            ROW_PAUSE_FADE => {
                self.pause_fade = !self.pause_fade;
                self.send(AudioCmd::SetPauseFade(self.pause_fade));
                self.save_now();
            }
            _ => {}
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.send(AudioCmd::SetVolume(self.volume));
    }

    /// Everything a click or key resolved to. Returns true to quit.
    fn act(&mut self, act: Act) -> bool {
        match act {
            Act::Nav(i) => {
                self.active = i;
                self.note = (i != SETTINGS_NAV)
                    .then(|| (t!("gui.coming", name = NAV[i].label()).to_string(), false));
                if i != SETTINGS_NAV {
                    self.cursor = None;
                }
            }
            Act::ToggleQueue => self.queue_open = !self.queue_open,
            Act::PlayPause => {
                if self.now.is_some() {
                    self.paused = !self.paused;
                    self.send(if self.paused { AudioCmd::Pause } else { AudioCmd::Resume });
                }
            }
            // Nothing to skip to until the queue slice lands; stay silent
            // rather than promising.
            Act::Prev | Act::Next => {}
            Act::Shuffle => self.shuffle = !self.shuffle,
            Act::Repeat => self.repeat = !self.repeat,
            Act::AutoDj => self.autodj = !self.autodj,
            Act::VolDown => self.set_volume(self.volume - VOLUME_STEP),
            Act::VolUp => self.set_volume(self.volume + VOLUME_STEP),
            Act::VolSet(i) => self.set_volume((i as f32 + 1.0) / 10.0),
            Act::Seek(frac) => {
                if let Some(now) = &mut self.now {
                    now.elapsed = frac * now.duration;
                    let target = now.elapsed;
                    self.send(AudioCmd::Seek(target));
                }
            }
            // Activation is the TUI's Enter: toggles flip, the blend steps
            // up, radios choose.
            Act::Row(i) => self.adjust_row(i, 1),
            Act::BlendDown => self.adjust_blend(-1),
            Act::BlendUp => self.adjust_blend(1),
        }
        false
    }
}

/// The demo seat (`MSTREAM_GUI_DEMO=1`): a fixed track so the bars can be
/// seen and the seek ridden before the queue slice exists. Same fiction as
/// the design canvas.
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

/// One frame. Public to the crate so render tests can drive it.
pub(crate) fn render(frame: &mut Frame, gui: &mut Gui) {
    gui.ui.begin_frame();
    let area = frame.area();
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

    draw_nav(frame, gui, area);

    // The content column, between the nav rule and the queue (when open).
    let right = if gui.queue_open { area.width - 36 } else { area.width - 3 };
    let content = Rect { x: 17, y: 2, width: right - 17, height: area.height - 10 };
    if gui.active == SETTINGS_NAV {
        draw_settings(frame, gui, content);
    } else {
        put(frame, content.x, content.y, &NAV[gui.active].label(), Style::default().add_modifier(Modifier::BOLD));
        put(
            frame,
            content.x,
            content.y + 2,
            &t!("gui.coming", name = NAV[gui.active].label()),
            dim(),
        );
    }

    if gui.queue_open {
        draw_queue(frame, area);
    }

    // Note above the tips, tips above the bar — the wizard's chrome order.
    if let Some((text, is_err)) = gui.note.clone() {
        let style = if is_err { Style::default().fg(th().gold) } else { dim() };
        put(frame, 1, area.height - 7, &text, style);
    }
    let tips = if gui.active == SETTINGS_NAV && gui.cursor.is_some() {
        t!("gui.tips.rows")
    } else {
        t!("gui.tips.base")
    };
    put(frame, 1, area.height - 6, &tips, dim());

    let view = BarView {
        now: gui.now.as_ref(),
        paused: gui.paused,
        volume: gui.volume,
        shuffle: gui.shuffle,
        repeat: gui.repeat,
        autodj: gui.autodj,
        queue_open: gui.queue_open,
    };
    bar::draw(frame, &mut gui.ui, area, gui.bar_style, &view);
}

fn draw_nav(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    put(frame, 1, 2, &t!("gui.nav.library"), dim());
    let forward = if legacy_conhost() { ">" } else { "▸" };
    let set_y = area.height - 9;
    for (i, id) in NAV.iter().enumerate() {
        let y = match i {
            0..=4 => 3 + i as u16,
            5 => 9,
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

fn draw_queue(frame: &mut Frame, area: Rect) {
    let x = area.width - 32;
    for y in 2..area.height - 8 {
        put(frame, area.width - 34, y, "│", dim());
    }
    put(frame, x, 2, &t!("gui.queue.title"), dim());
    put(frame, x, 3, &"─".repeat(31), dim());
    // The empty state keeps the header and says so in words (kit rule) —
    // the queue itself arrives with the browse slice.
    put(frame, x, 4, &t!("gui.queue.empty"), dim());
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
            format!("{:<14} -  {:>4}  +", t!("gui.set.blend"), fmt_blend(gui.crossfade)),
            t!("gui.set.blend_desc").to_string(),
        ),
        (
            format!("{} {}", if gui.gapless { check_on } else { check_off }, t!("gui.set.gapless")),
            t!("gui.set.gapless_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.blend_skips { check_on } else { check_off },
                t!("gui.set.blend_skips")
            ),
            t!("gui.set.blend_skips_desc").to_string(),
        ),
        (
            format!(
                "{} {}",
                if gui.pause_fade { check_on } else { check_off },
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
    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Tab => return gui.act(Act::ToggleQueue),
        KeyCode::Char(c @ '1'..='7') => {
            return gui.act(Act::Nav(c as usize - '1' as usize));
        }
        KeyCode::Down if settings => {
            gui.cursor = Some(gui.cursor.map_or(0, |c| (c + 1).min(SET_ROWS - 1)));
        }
        KeyCode::Up if settings => {
            gui.cursor = Some(gui.cursor.map_or(SET_ROWS - 1, |c| c.saturating_sub(1)));
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

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    gui: &mut Gui,
    mouse_on: bool,
    from_audio: &Receiver<Event>,
) -> std::io::Result<()> {
    let mut hand = false;
    loop {
        terminal.draw(|frame| render(frame, gui))?;

        loop {
            match from_audio.try_recv() {
                Ok(Event::AudioFailed(e)) => {
                    gui.note = Some((t!("gui.audio_failed", error = e).to_string(), true));
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
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
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn run() -> i32 {
    let (config, config_ok) = match config::load() {
        Ok(config) => (config, true),
        Err(e) => {
            // Broken config: run on defaults but never write over the file
            // someone hand-edited (the player's own rule).
            eprintln!("mstream-player: {e} — running on defaults; changes will not be saved");
            (Config::default(), false)
        }
    };

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    let (audio, _tap) = spawn_audio(events_tx);
    let mut gui = Gui::new(config, config_ok, audio);
    if std::env::var("MSTREAM_GUI_DEMO").is_ok_and(|v| v == "1") {
        gui.now = Some(demo_now());
        gui.paused = false;
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

    let outcome = event_loop(&mut terminal, &mut gui, mouse_on, &events_rx);

    if mouse_on {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        let _ = execute!(std::io::stdout(), ratatui::crossterm::style::Print(POINTER_RESET));
    }
    ratatui::restore();
    crate::console::release_terminal();
    drop(ground_guard);
    gui.send(AudioCmd::Shutdown);
    // The exit save carries the session-local knobs (volume, the toggles)
    // the way the TUI's remember() does.
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn test_gui() -> Gui {
        let (audio, _keep) = std::sync::mpsc::channel();
        // The receiver leaks on purpose: sends must not error mid-test.
        std::mem::forget(_keep);
        let mut gui = Gui::new(Config::default(), false, audio);
        gui.now = Some(demo_now());
        gui.paused = false;
        gui
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

    #[test]
    fn the_wave_bar_draws_the_demo_track_honestly() {
        let mut gui = test_gui();
        gui.bar_style = BarStyle::Wave;
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("mStream"), "the wordmark anchors the shell");
        assert!(all.contains("0:47") && all.contains("5:02"), "times bracket the wave");
        assert!(all.contains('▁') || all.contains('▄'), "amplitude glyphs are on screen");
        assert!(all.contains("auto-dj"), "the toggles row is drawn");
        assert!(all.contains("Cassini IV"), "the card names the track");
        assert!(all.contains('▾'), "the open queue wears the collapse chevron");
        assert!(all.contains(&t!("gui.queue.empty").to_string()), "the queue says it is empty");
    }

    #[test]
    fn the_gold_bar_puts_the_seek_on_the_rule_and_fattens_the_controls() {
        let mut gui = test_gui();
        gui.bar_style = BarStyle::GoldLine;
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        // The seek line carries times; the tall buttons bring rounded frames
        // into the bar rows.
        assert!(all.contains("0:47") && all.contains("5:02"));
        let bar_rows = &rows[rows.len() - 5..];
        let frames = bar_rows.join("").matches('╭').count();
        assert!(frames >= 6, "prev/play/next and three toggles draw tall: {frames} frames");
        assert!(!bar_rows.join("").contains('▁'), "no waveform in the gold-line bar");
    }

    #[test]
    fn settings_descriptions_take_the_room_the_queue_leaves() {
        let mut gui = test_gui();
        gui.queue_open = false;
        let all = draw(&mut gui).join("\n");
        assert!(
            all.contains(&t!("gui.set.bar_wave_desc").to_string()),
            "with the queue folded the full sentence fits"
        );
        gui.queue_open = true;
        let all = draw(&mut gui).join("\n");
        assert!(
            all.contains("the track's wa"),
            "with the queue open the description clips instead of vanishing"
        );
    }

    #[test]
    fn idle_is_honest_silence() {
        let mut gui = test_gui();
        gui.now = None;
        let all = draw(&mut gui).join("\n");
        assert!(all.contains(&t!("gui.nothing_playing").to_string()));
        assert!(!all.contains("0:47"), "no timestamps without a track");
    }

    #[test]
    fn the_bar_radio_switches_and_remembers() {
        let mut gui = test_gui();
        assert_eq!(gui.bar_style, BarStyle::Wave);
        gui.act(Act::Row(ROW_BAR_GOLD));
        assert_eq!(gui.bar_style, BarStyle::GoldLine);
        gui.sync_config();
        assert_eq!(gui.config.gui.bar, "gold-line");
        // Choosing the chosen one is not a toggle.
        gui.act(Act::Row(ROW_BAR_GOLD));
        assert_eq!(gui.bar_style, BarStyle::GoldLine);
    }

    #[test]
    fn the_blend_walks_whole_seconds_and_snaps() {
        let mut gui = test_gui();
        gui.crossfade = 4.5;
        gui.adjust_blend(1);
        assert_eq!(gui.crossfade, 5.0, "a hand-written 4.5 steps to 5");
        gui.adjust_blend(-1);
        assert_eq!(gui.crossfade, 4.0, "and to 4, never 5.5 forever");
        gui.crossfade = 0.0;
        gui.adjust_blend(-1);
        assert_eq!(gui.crossfade, 0.0, "off is the floor");
        gui.crossfade = 30.0;
        gui.adjust_blend(1);
        assert_eq!(gui.crossfade, 30.0, "the engine's own ceiling");
    }

    #[test]
    fn toggles_flip_and_sync_into_the_config_words() {
        let mut gui = test_gui();
        gui.act(Act::Row(ROW_GAPLESS));
        assert!(!gui.gapless, "gapless starts on and toggles off");
        gui.act(Act::Shuffle);
        gui.act(Act::Repeat);
        gui.act(Act::AutoDj);
        gui.sync_config();
        assert!(gui.config.player.shuffle);
        assert_eq!(gui.config.player.repeat, "all");
        assert_eq!(gui.config.player.autodj, "similar");
    }

    #[test]
    fn volume_clicks_and_steps_stay_inside_the_bar() {
        let mut gui = test_gui();
        gui.act(Act::VolSet(6));
        assert_eq!(gui.volume, 0.7);
        gui.volume = 0.02;
        gui.act(Act::VolDown);
        assert_eq!(gui.volume, 0.0);
        gui.volume = 1.0;
        gui.act(Act::VolUp);
        assert_eq!(gui.volume, 1.0);
    }

    #[test]
    fn seeking_moves_the_demo_playhead() {
        let mut gui = test_gui();
        gui.act(Act::Seek(0.5));
        assert_eq!(gui.now.as_ref().unwrap().elapsed, 151.0);
    }

    #[test]
    fn the_settings_cursor_picks_up_stows_and_clamps() {
        let mut gui = test_gui();
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
            let (audio, keep) = std::sync::mpsc::channel();
            std::mem::forget(keep);
            let mut gui = Gui::new(Config::default(), false, audio);
            gui.now = Some(demo_now());
            gui.paused = false;
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
