//! The browser build: the same App and drawing code, rendered by ratzilla.
//!
//! The real player's run loop (tui::event_loop) owns the terminal and polls;
//! a browser owns *us*, so the loop inverts — ratzilla calls the draw closure
//! once per animation frame and the key handler as events arrive. Each frame
//! does exactly what one pass of the native loop does: dispatch pending
//! effects, fold in worker events, draw.
//!
//! The api worker is real ([`api_worker`]): the same command→endpoint logic
//! the native thread runs, awaited on the browser's event loop against
//! whatever server the page came from (a trunk/static-host proxy in front of
//! a real mStream — see Trunk.toml). Audio stays a stub ([`stub`]): playback
//! is a clock and the visualizer draws a synthesised signal, until the
//! WebAudio milestone lands.

mod api_worker;
mod canned;
mod stub;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ratzilla::{DomBackend, WebRenderer};

use crate::clock::Instant;
use crate::config;
use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::tui::app::{App, Effect};
use crate::tui::ui;
use crate::tui::worker::Event;
use crate::tui::{Startup, app_from};
use api_worker::WebApi;
use stub::Stub;

/// The native loop steps its spinner off the wall clock at this cadence
/// (tui::SPIN_EVERY); the demo matches it so the two feel the same.
const SPIN_EVERY_MS: u128 = 90;

struct Shell {
    app: App,
    audio: Stub,
    api: WebApi,
    /// Replies from the api worker's futures, drained each frame.
    replies: Rc<RefCell<VecDeque<Event>>>,
    pending: Vec<Effect>,
    spun: Instant,
}

impl Shell {
    fn dispatch(&mut self, effect: Effect) {
        match effect {
            Effect::Audio(cmd) => self.audio.dispatch(cmd),
            Effect::Api(cmd) => self.api.dispatch(cmd),
            // mDNS cannot exist in a browser; a canned answer keeps the
            // Discover-servers view demonstrating itself instead of hanging.
            Effect::Discover => {
                self.replies.borrow_mut().push_back(Event::ServersDiscovered(canned::lan_servers()));
            }
            // Nothing durable to save to in a spike. localStorage is the
            // obvious home when this grows up.
            Effect::SaveSession => {}
        }
    }
}

pub fn run() {
    console_error_panic_hook::set_once();
    if let Err(e) = run_inner() {
        // The hook above routes panics to the console; use the same channel
        // for a refusal to start.
        panic!("mstream-player web demo failed to start: {e}");
    }
}

fn run_inner() -> Result<(), Box<dyn std::error::Error>> {
    // The server is wherever this page came from: the host proxies the
    // mStream routes (see Trunk.toml), so the app talks same-origin and no
    // CORS is involved. Connecting elsewhere still works for any server
    // that answers preflights.
    let origin = ratzilla::web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .ok_or("no window.location.origin — not running in a browser?")?;

    let start = Startup {
        server: Some(origin),
        token: None,
        username: None,
        last_path: None,
        prefs: config::PlayerPrefs::default(),
        tunnel_code: None,
        keys: Default::default(),
        theme: config::ThemePrefs::default(),
        mouse: config::MousePrefs::default(),
    };
    let (theme, _warnings) = ui::Theme::from_prefs(&start.theme);
    ui::set_theme(theme);

    let mut app = app_from(start);
    let tap = crate::engine::tap::AudioTap::new();
    app.tap = Some(tap.clone());
    let pending = app.start();

    let replies: Rc<RefCell<VecDeque<Event>>> = Rc::new(RefCell::new(VecDeque::new()));
    let shell = Rc::new(RefCell::new(Shell {
        app,
        audio: Stub::new(tap),
        api: WebApi::new(replies.clone()),
        replies,
        pending,
        spun: Instant::now(),
    }));

    let backend = DomBackend::new()?;
    let mut terminal = ratatui::Terminal::new(backend)?;

    let on_key = shell.clone();
    terminal.on_key_event(move |event| {
        let mut shell = on_key.borrow_mut();
        let Some(key) = translate(event) else { return };
        if let Some(action) = shell.app.keymap.action(key, shell.app.input_mode()) {
            let effects = shell.app.handle_action(action);
            shell.pending.extend(effects);
        }
    })?;

    let on_frame = shell;
    terminal.draw_web(move |frame| {
        let mut shell = on_frame.borrow_mut();
        let shell = &mut *shell;

        for effect in std::mem::take(&mut shell.pending) {
            shell.dispatch(effect);
        }
        for event in shell.audio.tick() {
            shell.pending.extend(shell.app.apply_event(event));
        }
        loop {
            // Popped one at a time rather than held borrowed: apply_event can
            // queue effects whose replies want this same queue.
            let Some(event) = shell.replies.borrow_mut().pop_front() else { break };
            shell.pending.extend(shell.app.apply_event(event));
        }
        if shell.spun.elapsed().as_millis() >= SPIN_EVERY_MS {
            shell.app.spinner = shell.app.spinner.wrapping_add(1);
            shell.spun = Instant::now();
        }
        // There is no process to quit in a tab; parking the flag turns Quit
        // into a no-op instead of a frozen screen.
        shell.app.should_quit = false;

        ui::render(frame, &mut shell.app);
    });

    Ok(())
}

/// Browser key events, translated to the crate's input types. `None` is a key
/// the player has no meaning for — dropped here, the same way an unbound key
/// falls through the keymap.
fn translate(event: ratzilla::event::KeyEvent) -> Option<KeyEvent> {
    use ratzilla::event::KeyCode as Web;

    let code = match event.code {
        Web::Char(c) => KeyCode::Char(c),
        Web::Enter => KeyCode::Enter,
        Web::Esc => KeyCode::Esc,
        Web::Tab => KeyCode::Tab,
        Web::Backspace => KeyCode::Backspace,
        Web::Up => KeyCode::Up,
        Web::Down => KeyCode::Down,
        Web::Left => KeyCode::Left,
        Web::Right => KeyCode::Right,
        Web::Home => KeyCode::Home,
        Web::End => KeyCode::End,
        Web::PageUp => KeyCode::PageUp,
        Web::PageDown => KeyCode::PageDown,
        Web::F(n) => KeyCode::F(n),
        Web::Delete => KeyCode::Delete,
        Web::Unidentified => return None,
    };

    let mut modifiers = KeyModifiers::NONE;
    if event.ctrl {
        modifiers = modifiers | KeyModifiers::CONTROL;
    }
    if event.shift {
        modifiers = modifiers | KeyModifiers::SHIFT;
    }
    if event.alt {
        modifiers = modifiers | KeyModifiers::ALT;
    }
    Some(KeyEvent::new(code, modifiers))
}
