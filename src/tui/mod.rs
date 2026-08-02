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

use crate::api::session::{self, Session};
use app::{App, Effect, map_key};
use worker::{ApiCmd, AudioCmd, Event};

/// How long to wait for a key before redrawing anyway. Also sets how quickly
/// the progress bar advances on screen.
const POLL: Duration = Duration::from_millis(100);

pub fn run(server: Option<String>, token: Option<String>) -> i32 {
    let saved = match session::load() {
        Ok(saved) => saved,
        Err(e) => {
            eprintln!("warning: {e}");
            None
        }
    };

    // An explicit --server wins; a saved token only travels with its own server.
    let (server, token, username) = match server {
        Some(server) => {
            let same = saved.as_ref().filter(|s| s.server.trim_end_matches('/') == server.trim_end_matches('/'));
            let token = token.or_else(|| same.as_ref().and_then(|s| s.token.clone()));
            let username = same.and_then(|s| s.username.clone());
            (Some(server), token, username)
        }
        None => match saved {
            Some(s) => (Some(s.server), token.or(s.token), s.username),
            None => (None, token, None),
        },
    };

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let audio_tx = worker::spawn_audio(event_tx.clone());
    let api_tx = worker::spawn_api(event_tx.clone());

    let mut app = App::new(server, token, username);
    let pending = app.start();

    let mut terminal = ratatui::init();
    let result =
        event_loop(&mut terminal, &mut app, &event_rx, &audio_tx, &api_tx, &event_tx, pending);
    ratatui::restore();

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("mstream-player: {e}");
            1
        }
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

fn dispatch(
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
                let session = Session {
                    server: app.server.clone(),
                    username: app.username.clone(),
                    token: app.token.clone(),
                };
                // A read-only config directory shouldn't take the app down;
                // the session just won't survive to the next run.
                if let Err(e) = session::save(&session) {
                    eprintln!("warning: could not save session: {e}");
                }
            }
        }
    }
}
