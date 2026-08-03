//! The interactive terminal player: terminal setup and the event loop.
//!
//! The loop does three things per pass — dispatch pending effects to the
//! workers, draw, then fold in whatever input or worker events arrived. All
//! the decisions live in [`app`].

pub mod app;
pub mod ui;
pub mod worker;

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyEventKind};

use crate::config;
use app::{App, Effect, map_key};
use worker::{ApiCmd, AudioCmd, Event};

/// How long to wait for a key before redrawing anyway. Also sets how quickly
/// the progress bar advances on screen.
const POLL: Duration = Duration::from_millis(100);

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
}

/// Resolve the starting point from stored config plus any overrides. Shared
/// with the replay harness so a scripted run begins exactly where the real
/// binary would.
pub(crate) fn startup(server: Option<String>, token: Option<String>) -> Startup {
    let config = match config::load() {
        Ok(config) => config,
        Err(e) => {
            // A config we can't read shouldn't stop the player starting; the
            // worst case is being asked where the server is again.
            eprintln!("warning: {e}");
            config::Config::default()
        }
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
        None => config::most_recent_server(&config).cloned(),
    };
    let (server, username, last_path) = match chosen {
        Some(entry) => (Some(entry.url), entry.username, entry.last_path),
        None => (None, None, None),
    };
    let token = token
        .or_else(|| server.as_deref().and_then(|url| config::token_for(&credentials, url)));
    let tunnel_code = server
        .as_deref()
        .filter(|s| crate::quickconnect::is_tunnel_id(s))
        .and_then(|id| config::pairing_for(&credentials, id));

    Startup { server, token, username, last_path, prefs: config.player, tunnel_code }
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
        .with_tunnel(start.tunnel_code);
    if let Some(path) = start.last_path {
        // Pick up where the last session left off; `start` browses this.
        app.path = path;
    }
    app
}

pub fn run(server: Option<String>, token: Option<String>) -> i32 {
    let start = startup(server, token);

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let audio_tx = worker::spawn_audio(event_tx.clone());
    let api_tx = worker::spawn_api(event_tx.clone());

    let mut app = app_from(start);
    let pending = app.start();

    let mut terminal = ratatui::init();
    let result =
        event_loop(&mut terminal, &mut app, &event_rx, &audio_tx, &api_tx, &event_tx, pending);
    ratatui::restore();

    remember(&app);

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
    }
}

/// Write preferences and where we were, on the way out. Saving here rather
/// than on every keystroke keeps a volume nudge from becoming a disk write.
pub(crate) fn remember(app: &App) {
    let mut config = config::load().unwrap_or_default();
    config.player = app.prefs();
    // Keyed on the identity, never the endpoint: a tunnel session's loopback
    // port is meaningless by the next run.
    if !app.server_id.is_empty() {
        config::touch_server(&mut config, &app.server_id, app.username.clone());
        config::set_last_path(&mut config, &app.server_id, &app.path);
    }
    if let Err(e) = config::save(&config) {
        eprintln!("warning: could not save settings: {e}");
    }
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    events: &Receiver<Event>,
    audio_tx: &Sender<AudioCmd>,
    api_tx: &Sender<ApiCmd>,
    event_tx: &Sender<Event>,
    mut pending: Vec<Effect>,
) -> std::io::Result<()> {
    loop {
        dispatch(app, &mut pending, audio_tx, api_tx, event_tx);

        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(POLL)? {
            match event::read()? {
                // Windows reports key releases as well as presses; without this
                // filter every keystroke would act twice.
                TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(action) = map_key(key, app.input_mode()) {
                        pending.extend(app.handle_action(action));
                    }
                }
                _ => {}
            }
        }

        while let Ok(event) = events.try_recv() {
            pending.extend(app.apply_event(event));
        }

        if app.should_quit {
            dispatch(app, &mut pending, audio_tx, api_tx, event_tx);
            return Ok(());
        }
    }
}

pub(crate) fn save_login(app: &App) -> Result<(), String> {
    let mut config = config::load()?;
    config::touch_server(&mut config, &app.server_id, app.username.clone());
    config.player = app.prefs();
    config::save(&config)?;

    let mut credentials = config::load_credentials()?;
    config::store_token(&mut credentials, &app.server_id, app.token.clone());
    // The pairing code goes in beside the token: both are secrets, and the
    // code is what turns a remembered tunnel identity back into a connection.
    if crate::quickconnect::is_tunnel_id(&app.server_id) {
        config::store_pairing(&mut credentials, &app.server_id, app.tunnel_code.clone());
    }
    config::save_credentials(&credentials)
}

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
                let _ = audio_tx.send(cmd);
            }
            Effect::Api(cmd) => {
                let _ = api_tx.send(cmd);
            }
            Effect::Discover => worker::spawn_discovery(event_tx.clone()),
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
