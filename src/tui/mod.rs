//! The interactive terminal player: terminal setup and the event loop.
//!
//! The loop does three things per pass — dispatch pending effects to the
//! workers, draw, then fold in whatever input or worker events arrived. All
//! the decisions live in [`app`].

pub mod app;
pub mod art;
pub mod canvas;
pub mod graphics;
pub mod keymap;
pub mod ui;
pub mod viz;
pub mod worker;

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use ratatui::DefaultTerminal;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyEventKind,
    MouseButton, MouseEventKind,
};
#[cfg(not(target_arch = "wasm32"))]
use ratatui::crossterm::execute;
#[cfg(not(target_arch = "wasm32"))]
use ratatui::layout::Rect;

use crate::config;
use app::App;
#[cfg(not(target_arch = "wasm32"))]
use app::Effect;
#[cfg(not(target_arch = "wasm32"))]
use worker::{ApiCmd, AudioCmd, Event};

/// How long to wait for a key before redrawing anyway. Also sets how quickly
/// the progress bar advances on screen.
const POLL: Duration = Duration::from_millis(100);

/// The same, while something is being drawn from the audio itself. Ten frames
/// a second is fine for a progress bar and visibly steppy for a waveform, and
/// this is the only screen that earns the extra wakeups.
const POLL_DRAWING_AUDIO: Duration = Duration::from_millis(33);

/// How often the spinner steps. Off the wall clock rather than the draw count,
/// so it turns at one speed whether the loop is idle or churning.
#[cfg(not(target_arch = "wasm32"))]
const SPIN_EVERY: Duration = Duration::from_millis(90);

/// How long to wait before drawing again. Shared with the replay harness,
/// which has to draw on the same schedule or it measures the app under one it
/// never runs at: anything that moves on a timer sees a different number of
/// frames there than here, and reports a different answer because of it.
pub(crate) fn poll_interval(app: &App) -> Duration {
    if app.drawing_audio() { POLL_DRAWING_AUDIO } else { POLL }
}

/// Everything remembered about how to start: which server, with what
/// credentials, where we were browsing, and the player's settings.
pub(crate) struct Startup {
    pub server: Option<String>,
    pub token: Option<String>,
    pub username: Option<String>,
    pub last_path: Option<String>,
    pub prefs: config::PlayerPrefs,
    /// Pairing code for the remembered server, when it is one reached through
    /// a tunnel. Without it that server cannot be dialled again.
    pub tunnel_code: Option<String>,
    /// The chosen entry trusts its own TLS certificate.
    pub self_signed: bool,
    /// The `[keys]` section, unvalidated — the app reports what it can't use.
    pub keys: std::collections::BTreeMap<String, Vec<String>>,
    /// The `[theme]` section, likewise.
    pub theme: config::ThemePrefs,
    /// The `[display]` section — which glyphs the font can be trusted with,
    /// and how much the layout is allowed to spend.
    pub display: config::DisplayPrefs,
    /// Whether to ask the terminal to report the mouse.
    pub mouse: config::MousePrefs,
    /// Every saved server, with its token: what a queued track needs to be
    /// reached from a session on another server (contract clause 30).
    pub servers: Vec<app::KnownServer>,
    /// `--bundled-server`: the installer's own server, seeded into the list
    /// and never offered for removal (contract clauses 50–58).
    pub bundled: Option<String>,
    /// The saved queue, when the setting is on and a readable one exists
    /// (contract clause 40).
    pub queue: Option<app::QueueSnapshot>,
}

/// The saved queue as the config left it — `None` for no file, a file from
/// another shape, or one that would not parse (never a reason to stop
/// the player starting).
#[cfg(not(target_arch = "wasm32"))]
fn load_queue_snapshot() -> Option<app::QueueSnapshot> {
    let text = config::load_queue_file().ok().flatten()?;
    serde_json::from_str::<app::QueueSnapshot>(&text).ok()
}

/// Keeps `queue.json` current for the shell (contract clause 39): a write
/// 800 ms after the queue last changed, a checkpoint every ten seconds
/// while playing, a flush on the way out — and the file gone once a queue
/// that existed this session is cleared, or the setting turned off.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct QueueSaver {
    signature: u64,
    dirty_since: Option<std::time::Instant>,
    last_write: std::time::Instant,
    /// A queue existed this session: only then does an empty one delete
    /// the file — the empty queue a failed restore leaves behind must not
    /// destroy the snapshot it failed to read.
    had_queue: bool,
    /// The setting as last seen, so turning it off deletes the file once.
    enabled: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl QueueSaver {
    const DEBOUNCE: Duration = Duration::from_millis(800);
    const CHECKPOINT: Duration = Duration::from_secs(10);

    pub(crate) fn new(app: &App) -> Self {
        QueueSaver {
            signature: Self::signature(app),
            dirty_since: None,
            last_write: std::time::Instant::now(),
            had_queue: !app.queue.items.is_empty(),
            enabled: app.resume_queue,
        }
    }

    /// What a change to the queue looks like from outside: the rows, the
    /// playing one, the modes and a held spot. Cheap enough per tick.
    fn signature(app: &App) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for item in &app.queue.items {
            item.origin.server.hash(&mut h);
            item.origin.peer.hash(&mut h);
            item.filepath.hash(&mut h);
        }
        app.queue.current.hash(&mut h);
        app.queue.shuffle.hash(&mut h);
        app.queue.repeat.label().hash(&mut h);
        app.resume_spot.map(|(i, _)| i).hash(&mut h);
        h.finish()
    }

    /// Once per loop iteration.
    pub(crate) fn tick(&mut self, app: &App) {
        let now = std::time::Instant::now();
        if !app.resume_queue {
            if self.enabled {
                self.enabled = false;
                let _ = config::delete_queue_file();
            }
            return;
        }
        self.enabled = true;
        let signature = Self::signature(app);
        if signature != self.signature {
            self.signature = signature;
            self.dirty_since.get_or_insert(now);
        }
        let due = match self.dirty_since {
            Some(since) => now.duration_since(since) >= Self::DEBOUNCE,
            None => {
                app.status.playing
                    && !app.status.paused
                    && !app.queue.items.is_empty()
                    && now.duration_since(self.last_write) >= Self::CHECKPOINT
            }
        };
        if due {
            self.write(app);
        }
    }

    /// Write now — quitting, or the debounce that just elapsed.
    pub(crate) fn flush(&mut self, app: &App) {
        if !app.resume_queue {
            return;
        }
        self.write(app);
    }

    fn write(&mut self, app: &App) {
        self.dirty_since = None;
        self.last_write = std::time::Instant::now();
        match app.queue_snapshot() {
            Some(snapshot) => {
                self.had_queue = true;
                // A read-only config directory costs the next launch its
                // queue and nothing else; the screen is not the place to say so.
                if let Ok(body) = serde_json::to_string(&snapshot) {
                    let _ = config::save_queue_file(&body);
                }
            }
            None if self.had_queue => {
                let _ = config::delete_queue_file();
            }
            None => {}
        }
    }
}

/// `--bundled-server` on boot (contract clause 51): the packaged server
/// gets an entry when it has none — without credentials, made the default
/// — and an entry it already has is used as it stands, so a default the
/// user chose later keeps standing. Returns the identity the mode guards.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn seed_bundled(config: &mut config::Config, url: &str) -> Option<String> {
    let url = crate::api::server_url::normalize(url).ok()?;
    let known = config.servers.iter().any(|entry| config::same_server(&entry.url, &url));
    if !known {
        // Seeded at the front and, being brand new, as the default; the
        // MRU order sorts itself out from the first session on.
        config::touch_server(config, &url, None);
        config::set_default_server(config, Some(&url));
        if let Err(e) = config::save(config) {
            eprintln!("warning: could not save the bundled server: {e}");
        }
    }
    Some(url)
}

/// The saved servers as the App's queue needs them (contract clause 30):
/// each entry's identity, its token, and whether its certificate is
/// trusted by choice.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn known_servers(
    config: &config::Config,
    credentials: &config::Credentials,
) -> Vec<app::KnownServer> {
    config
        .servers
        .iter()
        .map(|entry| app::KnownServer {
            id: entry.url.clone(),
            name: config::display_name(entry),
            token: config::token_for(credentials, &entry.url),
            self_signed: entry.self_signed,
            peer: entry.peer.as_ref().map(|p| (p.parent.clone(), p.id)),
            // What a queued row on this server is dialled with, when the
            // session is elsewhere (contract clause 38).
            pairing: if crate::quickconnect::is_tunnel_id(&entry.url) {
                config::pairing_for(credentials, &entry.url)
            } else {
                None
            },
        })
        .collect()
}

/// Resolve the starting point from stored config plus any overrides. Shared
/// with the replay harness so a scripted run begins exactly where the real
/// binary would.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn startup(
    server: Option<String>,
    token: Option<String>,
    bundled: Option<String>,
) -> Startup {
    let (mut config, config_ok) = match config::load() {
        Ok(config) => (config, true),
        Err(e) => {
            // A config we can't read shouldn't stop the player starting; the
            // worst case is being asked where the server is again.
            eprintln!("warning: {e}");
            (config::Config::default(), false)
        }
    };
    // A config that failed to load is never written back — seeding into it
    // would replace a file the user can still fix with a stub.
    let bundled = match bundled {
        Some(url) if config_ok => seed_bundled(&mut config, &url),
        Some(url) => crate::api::server_url::normalize(&url).ok(),
        None => None,
    };
    let credentials = config::load_credentials().unwrap_or_default();

    // `--server nas:3000` gets the same completion the connect screen gives.
    // An address too broken to normalise is passed through untouched, so the
    // connect screen can show it back with the reason.
    let server = server.map(|s| crate::api::server_url::normalize(&s).unwrap_or(s));

    // An explicit --server wins; a stored token is looked up per server, so
    // one is never sent to a host that didn't issue it.
    let chosen = match &server {
        Some(server) => config
            .servers
            .iter()
            .find(|entry| config::same_server(&entry.url, server))
            .cloned()
            .or(Some(config::ServerEntry { url: server.clone(), ..Default::default() })),
        None => config::preferred_server(&config).cloned(),
    };
    let (server, username, last_path, self_signed) = match chosen {
        Some(entry) => (Some(entry.url), entry.username, entry.last_path, entry.self_signed),
        None => (None, None, None, false),
    };
    let token = token
        .or_else(|| server.as_deref().and_then(|url| config::token_for(&credentials, url)));
    let tunnel_code = server
        .as_deref()
        .filter(|s| crate::quickconnect::is_tunnel_id(s))
        .and_then(|id| config::pairing_for(&credentials, id));

    let servers = known_servers(&config, &credentials);
    let queue = if config.player.resume_queue { load_queue_snapshot() } else { None };
    Startup {
        server,
        token,
        username,
        last_path,
        prefs: config.player,
        tunnel_code,
        self_signed,
        keys: config.keys,
        theme: config.theme,
        display: config.display,
        mouse: config.mouse,
        servers,
        bundled,
        queue,
    }
}

/// The bindings a `[keys]` section produces, with anything wrong reported to
/// stderr — which the player can't do mid-draw, but a command can.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn keymap_for(
    keys: &std::collections::BTreeMap<String, Vec<String>>,
) -> keymap::Keymap {
    let (keymap, warnings) = keymap::Keymap::default().with_overrides(keys);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    keymap
}

/// The palette in force, with anything unreadable reported to stderr — which
/// the player cannot do mid-draw, but a start-up path can.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn theme_for(prefs: &config::ThemePrefs) -> ui::Theme {
    let (theme, warnings) = ui::Theme::from_prefs(prefs);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    theme
}

/// The glyph set in force, reporting an unreadable `display.glyphs` the same
/// way an unreadable colour is reported: on stderr, before the terminal is
/// claimed.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn glyphs_for(prefs: &config::DisplayPrefs) -> ui::Glyphs {
    let (glyphs, warnings) = ui::Glyphs::from_prefs(prefs);
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    glyphs
}

/// Build the app from a resolved [`Startup`].
///
/// Shared with the replay harness rather than copied into it: a scripted run
/// is only worth anything if it begins exactly where the real binary would,
/// and a second copy of this drifted once already — it kept its own `App`
/// construction and silently stopped restoring tunnel sessions.
pub(crate) fn app_from(start: Startup) -> App {
    let mut app = App::new(start.server, start.token, start.username)
        .with_prefs(&start.prefs)
        .with_keys(&start.keys)
        .with_tunnel(start.tunnel_code);
    app.session.self_signed = start.self_signed;
    app.servers = start.servers;
    app.bundled_server = start.bundled;
    // After the servers, which decide which rows can come back at all.
    if let Some(snapshot) = start.queue {
        app.restore_queue(snapshot);
    }
    if let Some(path) = start.last_path {
        // Pick up where the last session left off; `start` browses this.
        app.path = path;
    }
    app
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run(server: Option<String>, token: Option<String>, bundled: Option<String>) -> i32 {
    let start = startup(server, token, bundled);

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (audio_tx, tap) = worker::spawn_audio(event_tx.clone());
    let api_tx = worker::spawn_api(event_tx.clone());

    ui::set_theme(theme_for(&start.theme));
    ui::set_glyphs(glyphs_for(&start.display));
    ui::set_sizing(ui::Sizing::from_prefs(&start.display));
    let mouse = start.mouse.enabled;

    let mut app = app_from(start);
    app.tap = Some(tap);
    let pending = app.start();

    // The player's palette is its interface: not subject to NO_COLOR.
    crate::console::keep_colors();
    let mut terminal = ratatui::init();
    // After init and before anything is drawn — not because the query
    // needs raw mode (the crate flips termios itself; graphics-probe calls
    // it cooked and works), but because the alternate screen is already up,
    // so a terminal that answers something unexpected makes its mess
    // somewhere that gets thrown away on restore — and because init's
    // saved state is what cleans up if the query's own restore is lost.
    app.graphics = graphics::Graphics::probe();
    // Asking for mouse reports takes click-drag selection away from the
    // terminal, so a failure to turn it on is not worth refusing to start
    // over — the player is a keyboard app that also answers a pointer.
    if mouse && execute!(std::io::stdout(), EnableMouseCapture).is_err() {
        eprintln!("warning: this terminal would not report the mouse");
    }
    crate::console::claim_terminal();
    push_window_title();

    install_panic_hook();

    let result =
        event_loop(&mut terminal, &mut app, &event_rx, &audio_tx, &api_tx, &event_tx, pending);
    pop_window_title();
    if mouse {
        // Left on, the terminal goes on emitting escape sequences at whatever
        // runs next, which looks like the shell has been broken.
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    crate::console::release_terminal();

    // The boot line scrolled away under the alternate screen; now that the
    // terminal is a terminal again, say it once more for the person about
    // to go looking for the file.
    if let Some(path) = crate::logging::active() {
        eprintln!("diagnostics were logged to {}", path.display());
    }

    remember(&app);

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

/// What a mouse event means, given where on the screen it happened.
///
/// One thing: a click on the progress bar seeks there. The wheel scrolled
/// lists for a while and was taken out again — moving a cursor three rows a
/// notch is not what a wheel feels like it should do, and the lists are
/// already well served by the keys.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn on_mouse(app: &mut App, mouse: event::MouseEvent, area: Rect) -> Vec<Effect> {
    let at = ratatui::layout::Position { x: mouse.column, y: mouse.row };
    // Every event, moves included: the bar lights under the pointer, and
    // that only works if we know where the pointer went.
    app.note_pointer(at);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let bar = ui::progress_area(app, area);
            if !bar.contains(at) {
                return Vec::new();
            }
            match ui::seek_target(app, bar.width, mouse.column - bar.x) {
                Some(position) => app.seek_to(position),
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Write preferences and where we were, on the way out. Saving here rather
/// than on every keystroke keeps a volume nudge from becoming a disk write.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remember(app: &App) {
    // Never a default here. Startup runs on defaults when the file won't
    // parse, so falling back a second time would write those defaults over a
    // config we only failed to *read* — servers, keys and theme deleted by a
    // typo, on the way out of a session that otherwise went fine. A file too
    // broken to load is the user's to fix; this run just doesn't get saved.
    let mut config = match config::load() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("warning: settings not saved, and the file left as it is — {e}");
            return;
        }
    };
    config.player.adopt(app.prefs());
    adopt_log_level(&mut config, &app);
    // Keyed on the identity, never the endpoint: a tunnel session's loopback
    // port is meaningless by the next run.
    if !app.session.server_id.is_empty() {
        config::touch_server(&mut config, &app.session.server_id, app.session.username.clone());
        config::set_last_path(&mut config, &app.session.server_id, &app.path);
    }
    if let Err(e) = config::save(&config) {
        eprintln!("warning: could not save settings: {e}");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    events: &Receiver<Event>,
    audio_tx: &Sender<AudioCmd>,
    api_tx: &Sender<ApiCmd>,
    event_tx: &Sender<Event>,
    mut pending: Vec<Effect>,
) -> std::io::Result<()> {
    let mut title = String::new();
    let mut spun = Instant::now();
    let mut saver = QueueSaver::new(app);
    loop {
        // A save changes what the book knows — a peer just reconciled, a
        // token just signed in for — and the queue's rows resolve against it.
        let saving = pending.iter().any(|e| matches!(e, Effect::SaveSession | Effect::SavePeers { .. }));
        dispatch(app, &mut pending, audio_tx, api_tx, event_tx);
        if saving && let Ok(fresh) = config::load() {
            let credentials = config::load_credentials().unwrap_or_default();
            app.servers = known_servers(&fresh, &credentials);
        }
        saver.tick(app);
        pending.extend(app.tick());

        if spun.elapsed() >= SPIN_EVERY {
            app.spinner = app.spinner.wrapping_add(1);
            spun = Instant::now();
        }

        // The player spends most of its life behind another window, where the
        // title bar is the only part of it still on screen.
        let wanted = window_title(app);
        if wanted != title {
            set_window_title(&wanted);
            title = wanted;
        }

        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(poll_interval(app))? {
            // Everything already queued is handled before the next draw.
            // Mouse capture arms any-motion tracking, so a sweep of the
            // pointer is one event per cell crossed — serviced one frame
            // apiece, with every keystroke waiting in line behind them.
            let mut inputs = vec![event::read()?];
            while event::poll(Duration::ZERO)? {
                inputs.push(event::read()?);
            }
            for input in collapse_moves(inputs) {
                match input {
                    // Windows reports key releases as well as presses; without
                    // this filter every keystroke would act twice.
                    TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        if let Some(action) = app.keymap.action(key, app.input_mode()) {
                            pending.extend(app.handle_action(action));
                        }
                    }
                    // Only arrives when capture is on, so there is nothing to
                    // check here. Where a thing was drawn is worked out from the
                    // screen size rather than remembered by the drawing, which is
                    // what keeps this to a handful of lines.
                    TermEvent::Mouse(mouse) => {
                        let size = terminal.size()?;
                        let area = Rect { x: 0, y: 0, width: size.width, height: size.height };
                        pending.extend(on_mouse(app, mouse, area));
                    }
                    // A resize is also how a tmux reattach announces a new
                    // terminal behind the same tty — one whose image store
                    // never saw kitty's transmit-once upload. Let graphics
                    // re-send where that matters; ratatui handles the
                    // layout side itself.
                    TermEvent::Resize(..) => app.graphics.refresh(),
                    _ => {}
                }
            }
        }

        while let Ok(event) = events.try_recv() {
            pending.extend(app.apply_event(event));
        }

        if app.should_quit {
            dispatch(app, &mut pending, audio_tx, api_tx, event_tx);
            saver.flush(app);
            return Ok(());
        }
    }
}

/// Runs of pointer moves collapse to where the pointer ended up. The places
/// it passed through between two draws could never have been seen, and each
/// one kept is a full frame of work. Everything else — clicks, keys, and the
/// order they came in — is kept exactly.
#[cfg(not(target_arch = "wasm32"))]
fn collapse_moves(inputs: Vec<TermEvent>) -> Vec<TermEvent> {
    let mut kept: Vec<TermEvent> = Vec::with_capacity(inputs.len());
    for input in inputs {
        if let TermEvent::Mouse(now) = &input
            && now.kind == MouseEventKind::Moved
            && let Some(TermEvent::Mouse(last)) = kept.last_mut()
            && last.kind == MouseEventKind::Moved
        {
            *last = *now;
            continue;
        }
        kept.push(input);
    }
    kept
}

/// What the terminal's title bar should read. Paused is worth saying, because
/// from outside the window silence and a paused track look the same.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn window_title(app: &App) -> String {
    match &app.now_playing {
        Some(track) if app.status.paused => format!("⏸ {}", track.display_name()),
        Some(track) => format!("▶ {}", track.display_name()),
        None => "mstream-player".to_string(),
    }
}

/// Chain the terminal-cleanup hook in front of whatever hook is current —
/// ratatui's, once init has run. ratatui puts back raw mode and the
/// alternate screen, and nothing else: mouse capture and the pushed window
/// title were turned on outside that pair, so a panic used to drop the
/// shell into a stream of motion escapes wearing our title (audit #31).
/// The audio thread is the exception — its panics are caught and become
/// AudioFailed, so the hook stands back rather than tearing the terminal
/// down under a UI that is still running (audit #32).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if worker::panics_are_caught(std::thread::current().name()) {
            return;
        }
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        pop_window_title();
        crate::console::release_terminal();
        previous(info);
    }));
}

#[cfg(not(target_arch = "wasm32"))]
fn set_window_title(title: &str) {
    use ratatui::crossterm::{execute, terminal::SetTitle};
    let _ = execute!(std::io::stdout(), SetTitle(title));
}

/// The push/set/pop title discipline as one droppable claim, for sessions
/// with a single fixed title (the setup wizard, the QR page) — the player
/// itself calls the pieces directly because its title tracks the playing
/// song. Unwind-safe: a panicking session still restores the title through
/// the drop, same posture as its ground lease.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct WindowTitle;

#[cfg(not(target_arch = "wasm32"))]
impl WindowTitle {
    pub(crate) fn claim(title: &str) -> Self {
        push_window_title();
        set_window_title(title);
        WindowTitle
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for WindowTitle {
    fn drop(&mut self) {
        pop_window_title();
    }
}

/// Save and restore the title the terminal had before us (XTWINOPS 22/23), so
/// quitting doesn't leave the last track's name in the tab. Terminals that
/// don't implement it ignore the sequence, which costs nothing.
#[cfg(not(target_arch = "wasm32"))]
fn push_window_title() {
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x1b[22;2t");
    let _ = std::io::stdout().flush();
}

#[cfg(not(target_arch = "wasm32"))]
fn pop_window_title() {
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x1b[23;2t");
    let _ = std::io::stdout().flush();
}

/// Carry the session's chosen log level into a config about to be saved —
/// only a level the user set in Settings, so an environment-forced one
/// never silently becomes configuration. Off writes as empty, which keeps
/// the [log] section out of the file entirely. Native beside its callers:
/// the browser build has no config file to write.
#[cfg(not(target_arch = "wasm32"))]
fn adopt_log_level(config: &mut config::Config, app: &App) {
    // Each switch persists only if this session's Settings actually set it:
    // walking the level under MSTREAM_LOG=1 must not also write `write =
    // true` and turn on disk logging for every run afterwards.
    if let Some(write) = app.chosen_log_write() {
        config.log.write = Some(write);
    }
    if let Some(level) = app.chosen_log_level() {
        config.log.level = level.label().to_string();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn save_login(app: &App) -> Result<(), String> {
    let mut config = config::load()?;
    config::touch_server(&mut config, &app.session.server_id, app.session.username.clone());
    config.player.adopt(app.prefs());
    adopt_log_level(&mut config, app);
    config::save(&config)?;

    let mut credentials = config::load_credentials()?;
    config::store_token(&mut credentials, &app.session.server_id, app.session.token.clone());
    // The pairing code goes in beside the token: both are secrets, and the
    // code is what turns a remembered tunnel identity back into a connection.
    if crate::quickconnect::is_tunnel_id(&app.session.server_id) {
        let session = &app.session;
        config::store_pairing(&mut credentials, &session.server_id, session.tunnel_code.clone());
    }
    config::save_credentials(&credentials)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn dispatch(
    app: &App,
    pending: &mut Vec<Effect>,
    audio_tx: &Sender<AudioCmd>,
    api_tx: &Sender<ApiCmd>,
    event_tx: &Sender<Event>,
) {
    for effect in pending.drain(..) {
        match effect {
            Effect::Audio(cmd) => {
                // A send with nobody listening means the audio thread died
                // un-caught. Say so through the ordinary event path — the
                // silent discard here was every later keypress vanishing
                // with no word as to why (audit #32).
                if audio_tx.send(cmd).is_err() {
                    let _ = event_tx.send(Event::AudioFailed("the audio thread is gone".into()));
                }
            }
            Effect::Api(cmd) => {
                // A server that presents its own certificate needs the
                // STREAM client to extend the same trust the api client is
                // about to — the flag rides the command either way, and
                // this is the one native funnel both front ends share.
                if let ApiCmd::Connect { server, self_signed: true, .. }
                | ApiCmd::Login { server, self_signed: true, .. } = &cmd
                {
                    crate::engine::http::trust_server(server);
                }
                let _ = api_tx.send(cmd);
            }
            Effect::Discover => worker::spawn_discovery(event_tx.clone()),
            // A queued track's server that presents its own certificate:
            // the stream client extends the trust the entry opted into.
            Effect::Trust(server) => crate::engine::http::trust_server(&server),
            // A parent's peer list folds into the saved servers; nothing is
            // written when nothing changed (contract clauses 20–23).
            Effect::SavePeers { parent, listed } => {
                if let Ok(mut config) = config::load()
                    && config::reconcile_peers(&mut config, &parent, &listed)
                    && let Err(e) = config::save(&config)
                {
                    eprintln!("warning: could not save the peer list: {e}");
                }
            }
            Effect::SaveSession => {
                // A read-only config directory shouldn't take the app down;
                // the sign-in just won't survive to the next run.
                if let Err(e) = save_login(app) {
                    eprintln!("warning: could not save sign-in: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Track, TrackMetadata};

    fn mouse_at(kind: MouseEventKind, column: u16, row: u16) -> event::MouseEvent {
        event::MouseEvent {
            kind,
            column,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        }
    }

    /// `cargo test the_panic_hook -- --ignored` — swaps the process-global
    /// panic hook, so it must run alone, not beside parallel tests.
    /// (The cleanup bytes go to the real stdout: a mouse-off and a
    /// title-pop, both no-ops on a terminal that never turned them on.)
    #[test]
    #[ignore = "swaps the process-global panic hook; run alone"]
    fn the_panic_hook_cleans_up_then_hands_over_except_where_panics_are_caught() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let base_ran = Arc::new(AtomicUsize::new(0));
        let counting = base_ran.clone();
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |_| {
            counting.fetch_add(1, Ordering::SeqCst);
        }));
        install_panic_hook();

        // An ordinary thread's panic runs the cleanup and reaches the
        // hook that was installed before ours — ratatui's, in real life.
        let _ = std::thread::Builder::new()
            .name("smoke-ordinary".into())
            .spawn(|| panic!("ordinary"))
            .unwrap()
            .join();
        assert_eq!(base_ran.load(Ordering::SeqCst), 1, "chained through to the previous hook");

        // The audio thread's panic is caught downstream and reported as
        // AudioFailed; the hook must not "recover" a terminal that is
        // still being drawn, so nothing runs — ratatui's restore included.
        let _ = std::thread::Builder::new()
            .name(worker::AUDIO_THREAD.into())
            .spawn(|| panic!("caught elsewhere"))
            .unwrap()
            .join();
        assert_eq!(base_ran.load(Ordering::SeqCst), 1, "stood back for the audio thread");

        let _ = std::panic::take_hook();
        std::panic::set_hook(original);
    }

    #[test]
    fn a_sweep_of_pointer_moves_collapses_to_where_it_ended() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let moved = |x, y| TermEvent::Mouse(mouse_at(MouseEventKind::Moved, x, y));
        let click = TermEvent::Mouse(mouse_at(MouseEventKind::Down(MouseButton::Left), 5, 5));
        let key = TermEvent::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

        // Two sweeps around a click and a key: each sweep keeps only its
        // final position, and nothing changes order or goes missing.
        let inputs = vec![
            moved(1, 1),
            moved(2, 1),
            moved(3, 1),
            click.clone(),
            moved(4, 2),
            moved(9, 9),
            key.clone(),
        ];
        assert_eq!(collapse_moves(inputs), vec![moved(3, 1), click, moved(9, 9), key]);
    }

    #[test]
    fn a_click_on_either_row_of_a_mirrored_bar_seeks() {
        // On a tall terminal the bar is two rows — the shape and the
        // scrubber — and the whole band is the control. The handler takes
        // the column and asks `contains` about the row, so the extra row
        // costs it nothing and buys a bigger target.
        let height = config::DEFAULT_MIRROR_MIN_HEIGHT + 4;
        let area = Rect { x: 0, y: 0, width: 80, height };
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.status.duration = 300.0;
        app.status.source = "http://host/a.mp3".into();

        let bar = ui::progress_area(&app, area);
        for row in bar.y..bar.y + bar.height {
            let effects = on_mouse(
                &mut app,
                mouse_at(MouseEventKind::Down(MouseButton::Left), bar.x, row),
                area,
            );
            assert_eq!(
                effects,
                vec![Effect::Audio(AudioCmd::Seek(0.0))],
                "row {row} of the band is the same control"
            );
        }

        // And the row above the band is the track's name, which is not.
        let effects = on_mouse(
            &mut app,
            mouse_at(MouseEventKind::Down(MouseButton::Left), bar.x, bar.y - 1),
            area,
        );
        assert!(effects.is_empty(), "the name is not a scrubber");
    }

    #[test]
    fn a_click_on_the_bar_seeks_and_nothing_else_does_anything() {
        let area = Rect { x: 0, y: 0, width: 80, height: 24 };
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.files.set(
            ["A", "B", "C", "D", "E", "F"]
                .iter()
                .map(|label| app::Entry::Dir { label: (*label).into(), path: (*label).into() })
                .collect(),
        );
        app.status.duration = 300.0;
        app.status.source = "http://host/a.mp3".into();

        // A click on the bar seeks to where it was clicked.
        let bar = ui::progress_area(&app, area);
        let effects = on_mouse(
            &mut app,
            mouse_at(MouseEventKind::Down(MouseButton::Left), bar.x, bar.y),
            area,
        );
        assert_eq!(effects, vec![Effect::Audio(AudioCmd::Seek(0.0))], "the left edge is the start");

        let effects = on_mouse(
            &mut app,
            mouse_at(MouseEventKind::Down(MouseButton::Left), bar.x + 20, bar.y),
            area,
        );
        let Some(Effect::Audio(AudioCmd::Seek(to))) = effects.first() else {
            panic!("expected a seek, got {effects:?}");
        };
        assert!(*to > 60.0 && *to < 120.0, "a fifth or so of the way in: {to}");

        // A click anywhere else is not a seek, and nor is one on the clock.
        let body = ui::regions(area).body;
        for (column, row) in [(4, body.y + 2), (4, 0), (bar.right() - 2, bar.y)] {
            let kind = MouseEventKind::Down(MouseButton::Left);
            assert!(
                on_mouse(&mut app, mouse_at(kind, column, row), area).is_empty(),
                "({column}, {row}) should not seek"
            );
        }

        // Moving the pointer about is not an event worth an effect.
        assert!(on_mouse(&mut app, mouse_at(MouseEventKind::Moved, 10, 10), area).is_empty());
    }

    #[test]
    fn the_queue_saver_writes_the_snapshot_and_removes_it_when_cleared_or_off() {
        let _scratch = crate::config::testing::Scratch::new("queue-saver");
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.push_queue(Track { filepath: "music/a.mp3".into(), metadata: Default::default() });
        app.push_queue(Track { filepath: "music/b.mp3".into(), metadata: Default::default() });
        app.queue.current = Some(1);
        app.status = crate::player::PlayerStatus {
            playing: true,
            position: 9.0,
            source: "http://host:3000/media/music/b.mp3?token=tok".into(),
            ..Default::default()
        };

        let mut saver = QueueSaver::new(&app);
        saver.flush(&app);
        let saved = load_queue_snapshot().expect("the snapshot is on disk");
        assert_eq!(saved.items.len(), 2);
        assert_eq!((saved.index, saved.position), (Some(1), 9.0));
        assert_eq!(saved.items[1].origin.server, "http://host:3000");

        // A tick with nothing changed writes nothing new; a change is
        // written once the debounce has passed (forced here by flushing).
        app.queue.clear();
        app.status = Default::default();
        saver.flush(&app);
        assert!(load_queue_snapshot().is_none(), "a cleared queue takes the file with it");

        // Off: the file goes and stays gone.
        app.push_queue(Track { filepath: "music/c.mp3".into(), metadata: Default::default() });
        saver.flush(&app);
        assert!(load_queue_snapshot().is_some());
        app.resume_queue = false;
        saver.tick(&app);
        assert!(load_queue_snapshot().is_none(), "the setting off drops the snapshot");
        saver.flush(&app);
        assert!(load_queue_snapshot().is_none(), "and nothing is written while it is off");

        // And startup brings a saved queue back only while the setting is on.
        app.resume_queue = true;
        let mut saver = QueueSaver::new(&app);
        saver.flush(&app);
        let start = startup(None, None, None);
        assert!(start.queue.is_some(), "the default setting is on");
    }

    #[test]
    fn the_bundled_server_is_seeded_once_as_the_default() {
        let _scratch = crate::config::testing::Scratch::new("bundled-seed");
        // First boot: no servers saved. The bundled one is created without
        // credentials and made the default (contract clause 51), and the
        // session opens on it.
        let start = startup(None, None, Some("nas.local:3000".into()));
        assert_eq!(start.bundled.as_deref(), Some("http://nas.local:3000"));
        assert_eq!(start.server.as_deref(), Some("http://nas.local:3000"));
        let config = config::load().unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.default_server.as_deref(), Some("http://nas.local:3000"));
        assert!(config.servers[0].username.is_none(), "no credentials seeded");

        // The user later chose another default: the next boot leaves that
        // standing, and the bundled entry is not seeded a second time.
        let mut config = config::load().unwrap();
        config::touch_server(&mut config, "http://office.local:3000", None);
        config::set_default_server(&mut config, Some("http://office.local:3000"));
        config::save(&config).unwrap();
        let start = startup(None, None, Some("http://nas.local:3000".into()));
        assert_eq!(start.server.as_deref(), Some("http://office.local:3000"), "the chosen default stands");
        assert_eq!(start.bundled.as_deref(), Some("http://nas.local:3000"), "still guarded");
        assert_eq!(config::load().unwrap().servers.len(), 2, "seeded once, not twice");

        // Without the flag the same config boots as it always did: nothing
        // about the mode persists.
        let start = startup(None, None, None);
        assert!(start.bundled.is_none());
    }

    #[test]
    fn a_config_that_would_not_load_is_left_alone_rather_than_replaced() {
        let scratch = crate::config::testing::Scratch::new("remember-broken");
        let path = config::config_path().unwrap();
        // A typo in a file the docs invite you to edit. Startup runs on
        // defaults after this, so the danger is quitting: `remember` used to
        // save those defaults back and take the servers and keys with them.
        let broken = "[theme\naccent = \"cyan\"\n\n[[server]]\nurl = \"http://host:3000\"\n";
        std::fs::write(&path, broken).unwrap();

        let mut app = App::new(Some("http://host:3000".into()), None, Some("alice".into()));
        app.session.server_id = "http://host:3000".into();
        app.path = "music/Artist".into();
        remember(&app);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "the file is the user's to fix, byte for byte"
        );
        assert!(scratch.dir.join("config.tmp").symlink_metadata().is_err(), "and no temp left");

        // A config that *does* load is still written, or this would prove
        // nothing but that saving is broken.
        std::fs::write(&path, "version = 1\n").unwrap();
        remember(&app);
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("music/Artist"), "got: {saved}");
    }

    #[test]
    fn the_window_title_says_what_is_playing_and_whether_it_stopped() {
        let mut app = App::new(Some("http://host:3000".into()), None, None);
        assert_eq!(window_title(&app), "mstream-player");

        app.now_playing = Some(Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata {
                title: Some("Moonlight".into()),
                artist: Some("Trio".into()),
                ..Default::default()
            },
        });
        assert_eq!(window_title(&app), "▶ Trio - Moonlight");

        // From outside the window a paused track and silence look identical,
        // so the title is the only place that can tell them apart.
        app.status.paused = true;
        assert_eq!(window_title(&app), "⏸ Trio - Moonlight");
    }
}
