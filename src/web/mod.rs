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
//! a real mStream — see Trunk.toml). Audio is real too ([`audio`]): the
//! browser's own decoder plays the stream, and analyser taps hand the
//! visualizer what is actually sounding.

mod api_worker;
mod audio;
mod canned;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use ratzilla::{DomBackend, WebRenderer};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::clock::Instant;
use crate::config;
use crate::input::{KeyCode, KeyEvent, KeyModifiers};
use crate::tui::app::{App, Effect};
use crate::tui::ui;
use crate::tui::worker::Event;
use crate::tui::{Startup, app_from};
use api_worker::WebApi;
use audio::WebAudioPlayer;

/// The native loop steps its spinner off the wall clock at this cadence
/// (tui::SPIN_EVERY); the demo matches it so the two feel the same.
const SPIN_EVERY_MS: u128 = 90;

struct Shell {
    app: App,
    audio: WebAudioPlayer,
    api: WebApi,
    /// Replies from the api worker's futures, drained each frame.
    replies: Rc<RefCell<VecDeque<Event>>>,
    pending: Vec<Effect>,
    spun: Instant,
    /// Something the user did is waiting to be seen — draw now, not at the
    /// next poll tick.
    dirty: bool,
    last_render: Instant,
    /// What the last real render produced. Browsers call the draw closure at
    /// display rate, and building the whole TUI sixty times a second is
    /// wasted work the native player never does — between poll ticks the
    /// stored buffer is handed back, ratzilla diffs it against itself, and
    /// nothing touches the DOM.
    last_buffer: Option<ratatui::buffer::Buffer>,
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
    boot();
}

/// Start the shell — once there is somewhere to draw.
///
/// An embedded or backgrounded page can finish loading before its pane has
/// any layout: the window measures 0×0, ratzilla builds a zero-cell grid
/// from it, and the only thing that would ever rebuild the grid is a resize
/// event a pane that is merely *revealed* may never send. Worse, a viewport
/// that appears between the backend measuring and the first draw leaves the
/// buffer and the grid disagreeing about the size, and the draw indexes out
/// of ratzilla's empty cell list — the whole tab down. So: no layout yet, no
/// boot; try again next frame. (index.html keeps animation frames ticking
/// while the page is hidden, so the retry runs even unseen.)
fn boot() {
    let window = ratzilla::web_sys::window();
    let px = |v: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>| {
        v.ok().and_then(|v| v.as_f64()).unwrap_or(0.0)
    };
    let laid_out = window
        .as_ref()
        .map(|w| px(w.inner_width()) > 0.0 && px(w.inner_height()) > 0.0)
        // No window at all is not a layout still on its way — let run_inner
        // say so in its own words instead of retrying forever.
        .unwrap_or(true);
    if !laid_out {
        let retry = Closure::once_into_js(boot);
        if let Some(w) = window
            && w.request_animation_frame(retry.unchecked_ref()).is_ok()
        {
            return;
        }
        // Could not schedule the retry: fall through and let the boot take
        // its chances rather than silently never starting.
    }
    if let Err(e) = run_inner() {
        // The panic hook routes panics to the console; use the same channel
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
        self_signed: false,
        keys: Default::default(),
        theme: config::ThemePrefs::default(),
        display: config::DisplayPrefs::default(),
        mouse: config::MousePrefs::default(),
    };
    let (theme, _warnings) = ui::Theme::from_prefs(&start.theme);
    ui::set_theme(theme);
    // The page picks its own font, and it is a modern one — so this resolves
    // to the full set rather than the console-safe fallback.
    let (glyphs, _warnings) = ui::Glyphs::from_prefs(&start.display);
    ui::set_glyphs(glyphs);
    ui::set_sizing(ui::Sizing::from_prefs(&start.display));

    let mut app = app_from(start);
    let tap = crate::engine::tap::AudioTap::new();
    app.tap = Some(tap.clone());
    let pending = app.start();

    let replies: Rc<RefCell<VecDeque<Event>>> = Rc::new(RefCell::new(VecDeque::new()));
    let shell = Rc::new(RefCell::new(Shell {
        app,
        audio: WebAudioPlayer::new(tap),
        api: WebApi::new(replies.clone()),
        replies,
        pending,
        spun: Instant::now(),
        dirty: true,
        last_render: Instant::now(),
        last_buffer: None,
    }));

    let backend = DomBackend::new()?;
    let terminal = ratatui::Terminal::new(backend)?;

    // Not ratzilla's on_key_event: that hangs the listener on the grid
    // element the DOM backend created, and the backend replaces that element
    // wholesale on every resize without moving the listener — one window
    // resize and the keyboard is dead. The document outlives every grid, and
    // listening there also ends the focus dance a child listener needed.
    let on_key = shell.clone();
    let keydown =
        Closure::<dyn FnMut(_)>::new(move |event: ratzilla::web_sys::KeyboardEvent| {
            let mut shell = on_key.borrow_mut();
            let Some(key) = translate(event.into()) else { return };
            if let Some(action) = shell.app.keymap.action(key, shell.app.input_mode()) {
                let effects = shell.app.handle_action(action);
                shell.pending.extend(effects);
                shell.dirty = true;
            }
        });
    ratzilla::web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document to listen for keys on")?
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .map_err(|_| "could not attach the keydown listener")?;
    // One listener for the life of the tab; there is no teardown to hold a
    // handle for.
    keydown.forget();

    let on_frame = shell;
    terminal.draw_web(move |frame| {
        let mut shell = on_frame.borrow_mut();
        let shell = &mut *shell;

        for effect in std::mem::take(&mut shell.pending) {
            shell.dispatch(effect);
        }
        // Audio status arrives every frame by construction; it waits for the
        // poll tick like it does natively. Api replies are answers to
        // something the user asked — those show up straight away.
        for event in shell.audio.tick() {
            shell.pending.extend(shell.app.apply_event(event));
        }
        loop {
            // Popped one at a time rather than held borrowed: apply_event can
            // queue effects whose replies want this same queue.
            let Some(event) = shell.replies.borrow_mut().pop_front() else { break };
            shell.pending.extend(shell.app.apply_event(event));
            shell.dirty = true;
        }
        if shell.spun.elapsed().as_millis() >= SPIN_EVERY_MS {
            shell.app.spinner = shell.app.spinner.wrapping_add(1);
            shell.spun = Instant::now();
        }
        // There is no process to quit in a tab; parking the flag turns Quit
        // into a no-op instead of a frozen screen.
        shell.app.should_quit = false;

        // The native loop draws once per poll wakeup; matching it here is
        // both the frame budget and the feel. A resize invalidates the
        // stored buffer, so a stale size never gets blitted back.
        let due = shell.dirty
            || shell.last_render.elapsed() >= crate::tui::poll_interval(&shell.app)
            || shell.last_buffer.as_ref().is_none_or(|b| b.area != frame.area());
        if due {
            ui::render(frame, &mut shell.app);
            shell.last_buffer = Some(frame.buffer_mut().clone());
            shell.last_render = Instant::now();
            shell.dirty = false;
        } else if let Some(buffer) = &shell.last_buffer {
            *frame.buffer_mut() = buffer.clone();
        }
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
        // The browser reports Shift+Tab as Tab with the shift flag; crossterm
        // gives it a code of its own, and the keymap's `Key` carries only
        // ctrl — so the split has to happen here, or Shift+Tab would arrive
        // as plain Tab and the Auto-DJ tab would have no way back.
        Web::Tab if event.shift => KeyCode::BackTab,
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
