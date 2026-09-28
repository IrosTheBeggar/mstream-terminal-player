//! The browser build: the same App and drawing code, rendered by ratzilla.
//!
//! The real player's run loop (tui::event_loop) owns the terminal and polls;
//! a browser owns *us*, so the loop inverts — the browser calls a pass back
//! on a timer or an animation frame ([`Alarm`]), and the key handler as
//! events arrive. Each pass does exactly what one wakeup of the native loop
//! does: dispatch pending effects, fold in worker events, and draw when the
//! poll tick or something the user did says so.
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
mod colours;
mod pace;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell as BufferCell;
use ratatui::layout::{Position, Size};
use ratzilla::backend::webgl2::WebGl2BackendOptions;
use ratzilla::{DomBackend, FontAtlasConfig, WebGl2Backend};
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

/// index.html's font stack and size, for the WebGL renderer's glyph atlas:
/// the canvas draws the same face the DOM grid would have (beamterm adds
/// the generic `monospace` after these, as the page's CSS does).
const FONTS: [&str; 4] = ["Cascadia Mono", "JetBrains Mono", "Consolas", "DejaVu Sans Mono"];
const FONT_PX: f32 = 16.0;

/// Asks the shell's loop for a pass on the next frame: what an api reply
/// or an audio refusal landing in its queue calls.
pub(crate) type Waker = Rc<dyn Fn()>;

struct Shell {
    app: App,
    audio: WebAudioPlayer,
    api: WebApi,
    /// Replies from the api worker's futures, drained each pass.
    replies: Rc<RefCell<VecDeque<Event>>>,
    pending: Vec<Effect>,
    spun: Instant,
    /// Something the user did is waiting to be seen — draw now, not at the
    /// next poll tick.
    dirty: bool,
    /// Everything goes out again on the next draw: the window changed
    /// size, or the canvas lost its pixels with its context.
    repaint: bool,
    last_render: Instant,
}

/// What the loop needs of a backend beyond ratatui's trait.
trait Surface: Backend + 'static {
    /// About to repaint everything: take the window's size first.
    fn follow_resize(&mut self) {}
}

/// The DOM grid follows a resize by itself: its own listener has it
/// rebuild the whole grid on the next draw.
impl Surface for DomBackend {}

/// The WebGL renderer, presenting only frames that changed.
///
/// Its `flush` redraws and presents the whole canvas every time, where the
/// DOM grid did nothing for a draw that changed nothing — and at rest nearly
/// every draw is one: the poll tick redraws a screen that has not moved. So
/// a flush goes through only after a draw with cells in it, or a clear
/// (which is how a resize and a restored context repaint).
struct Canvas {
    gl: WebGl2Backend,
    changed: bool,
}

impl Surface for Canvas {
    /// The canvas learns its new size inside `flush`, after a frame at the
    /// old size has been diffed and drawn; asked first, the draw that
    /// follows is already the right size.
    fn follow_resize(&mut self) {
        let _ = self.gl.resize_canvas();
    }
}

impl Backend for Canvas {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a BufferCell)>,
    {
        let mut content = content.peekable();
        self.changed |= content.peek().is_some();
        self.gl.draw(content)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if std::mem::take(&mut self.changed) { self.gl.flush() } else { Ok(()) }
    }

    fn clear(&mut self) -> std::io::Result<()> {
        self.changed = true;
        self.gl.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
        self.changed = true;
        self.gl.clear_region(clear_type)
    }

    // The player never shows a cursor; one that did would be a change.
    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.changed = true;
        self.gl.show_cursor()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        self.changed = true;
        self.gl.set_cursor_position(position)
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.gl.hide_cursor()
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        self.gl.get_cursor_position()
    }

    fn size(&self) -> std::io::Result<Size> {
        self.gl.size()
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        self.gl.window_size()
    }
}

/// The loop's one alarm (performance audit #125).
///
/// ratzilla's `draw_web` ran a pass, and handed ratatui a whole frame to
/// diff, on every animation frame — 60 or 120 times a second, on a page
/// with nothing to show. The shell schedules itself instead: the next
/// animation frame when something is waiting to be seen or the visualizer
/// is moving, a timer for the next poll tick otherwise — ten wakeups a
/// second at rest, as natively. At most one wake is ever pending: a frame
/// supersedes a timer (which is cleared), and a timer never supersedes a
/// frame.
///
/// Frames still go through `window.requestAnimationFrame`, so index.html's
/// hidden-page stand-in keeps a backgrounded tab's frames coming; its
/// timers the browser throttles to about one a second, which is plenty.
#[derive(Clone)]
struct Alarm(Rc<AlarmState>);

struct AlarmState {
    /// The pass. Set once, when the loop starts; a wake asked for before
    /// that is the first pass's to answer.
    pass: RefCell<Option<Closure<dyn FnMut()>>>,
    pending: Cell<Pending>,
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    Nothing,
    Frame,
    Timer(i32),
}

impl Alarm {
    fn new() -> Self {
        Alarm(Rc::new(AlarmState { pass: RefCell::new(None), pending: Cell::new(Pending::Nothing) }))
    }

    fn waker(&self) -> Waker {
        let alarm = self.clone();
        Rc::new(move || alarm.frame())
    }

    fn set(&self, wake: pace::Wake) {
        match wake {
            pace::Wake::Frame => self.frame(),
            pace::Wake::After(delay) => self.after(delay),
        }
    }

    /// A pass on the next animation frame.
    fn frame(&self) {
        let Some(window) = ratzilla::web_sys::window() else { return };
        match self.0.pending.get() {
            Pending::Frame => return,
            Pending::Timer(id) => window.clear_timeout_with_handle(id),
            Pending::Nothing => {}
        }
        self.0.pending.set(Pending::Nothing);
        let pass = self.0.pass.borrow();
        let Some(pass) = pass.as_ref() else { return };
        if window.request_animation_frame(pass.as_ref().unchecked_ref()).is_ok() {
            self.0.pending.set(Pending::Frame);
        }
    }

    /// A pass after `delay`, unless a frame is already on its way.
    fn after(&self, delay: Duration) {
        let Some(window) = ratzilla::web_sys::window() else { return };
        match self.0.pending.get() {
            Pending::Frame => return,
            Pending::Timer(id) => window.clear_timeout_with_handle(id),
            Pending::Nothing => {}
        }
        self.0.pending.set(Pending::Nothing);
        let pass = self.0.pass.borrow();
        let Some(pass) = pass.as_ref() else { return };
        // Rounded up: a timer that fires a fraction early finds the tick not
        // yet due and costs a second wakeup to reach it.
        let ms = delay.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
        if let Ok(id) =
            window.set_timeout_with_callback_and_timeout_and_arguments_0(pass.as_ref().unchecked_ref(), ms)
        {
            self.0.pending.set(Pending::Timer(id));
        }
    }

    /// The pending wake has fired: the pass is running.
    fn fired(&self) {
        self.0.pending.set(Pending::Nothing);
    }
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
            // The browser's fetch owns TLS; nothing to register.
            Effect::Trust(_) => {}
            // One server, no saved list to fold peers into.
            Effect::SavePeers { .. } => {}
            Effect::SaveDjLibrary { .. } => {}
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
        stats: None,
        peer: None,
        server_id: None,
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
        // One server, the session's; nothing else to reach.
        servers: Vec::new(),
        bundled: None,
        queue: None,
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

    let alarm = Alarm::new();
    let replies: Rc<RefCell<VecDeque<Event>>> = Rc::new(RefCell::new(VecDeque::new()));
    let shell = Rc::new(RefCell::new(Shell {
        app,
        audio: WebAudioPlayer::new(tap, alarm.waker()),
        api: WebApi::new(replies.clone(), alarm.waker()),
        replies,
        pending,
        spun: Instant::now(),
        dirty: true,
        repaint: false,
        last_render: Instant::now(),
    }));

    // Not ratzilla's on_key_event: that hangs the listener on the element
    // the backend draws into — a canvas that would need focus, or the DOM
    // grid, which the backend replaces wholesale on every resize without
    // moving the listener (one window resize and the keyboard is dead). The
    // document outlives every grid, and listening there also ends the focus
    // dance a child listener needed.
    let on_key = shell.clone();
    let key_alarm = alarm.clone();
    let keydown =
        Closure::<dyn FnMut(_)>::new(move |event: ratzilla::web_sys::KeyboardEvent| {
            let mut shell = on_key.borrow_mut();
            let Some(key) = translate(event.into()) else { return };
            if let Some(action) = shell.app.keymap.action(key, shell.app.input_mode()) {
                let effects = shell.app.handle_action(action);
                shell.pending.extend(effects);
                shell.dirty = true;
                key_alarm.frame();
            }
        });
    let window = ratzilla::web_sys::window().ok_or("no window to run in")?;
    window
        .document()
        .ok_or("no document to listen for keys on")?
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .map_err(|_| "could not attach the keydown listener")?;
    // One listener for the life of the tab; there is no teardown to hold a
    // handle for.
    keydown.forget();

    // A resize is drawn on the next frame, not the next poll tick.
    let on_resize = shell.clone();
    let resize_alarm = alarm.clone();
    let resize = Closure::<dyn FnMut()>::new(move || {
        let mut shell = on_resize.borrow_mut();
        shell.dirty = true;
        shell.repaint = true;
        resize_alarm.frame();
    });
    window
        .add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref())
        .map_err(|_| "could not attach the resize listener")?;
    // The canvas takes the same way back from a lost GPU context: beamterm
    // rebuilds its resources in the next flush, and a repaint is a flush
    // with every cell in it. Captured, since the event does not bubble.
    window
        .add_event_listener_with_callback_and_bool(
            "webglcontextrestored",
            resize.as_ref().unchecked_ref(),
            true,
        )
        .map_err(|_| "could not attach the context listener")?;
    resize.forget();

    // WebGL2 where the browser has it (performance audit #124): the DOM
    // grid rewrites an element's markup and inline style for every changed
    // cell and makes the browser restyle and lay out every row it touched —
    // 4-7 ms a frame at 1920×1080 with the visualizer up, thirty times a
    // second — and throws away and rebuilds all ten thousand of its
    // elements on every resize event. The canvas takes a changed cell as a
    // few bytes of a GPU buffer and a resize as a new viewport. The DOM grid
    // stays as the fallback.
    match webgl_terminal() {
        Ok(terminal) => run_loop(terminal, shell, alarm),
        Err(why) => {
            ratzilla::web_sys::console::warn_1(
                &format!("mstream-player: no WebGL2 ({why}); drawing with the DOM grid").into(),
            );
            // A failed context still left its canvas behind, full-window
            // and in front of the grid that is about to be built.
            if let Some(document) = window.document() {
                while let Ok(Some(canvas)) = document.query_selector("canvas") {
                    canvas.remove();
                }
            }
            run_loop(Terminal::new(DomBackend::new()?)?, shell, alarm);
        }
    }
    Ok(())
}

/// The WebGL2 renderer, glyphs rasterized on first use from the page's own
/// font, so anything the UI draws — braille, block elements, box drawing,
/// CJK — has a glyph (the static atlas beamterm ships knows a fixed set).
/// The canvas is sized by index.html's CSS, not by the renderer, so it
/// follows the window.
///
/// It costs the download about a megabyte: beamterm's builder falls back to
/// its embedded static atlas (1 MB, barely compressible) in a branch no
/// optimisation removes — fat LTO keeps it too — so the atlas ships though
/// this build never loads it.
fn webgl_terminal() -> Result<Terminal<Canvas>, Box<dyn std::error::Error>> {
    let options = WebGl2BackendOptions::new()
        .font_atlas_config(FontAtlasConfig::dynamic(&FONTS, FONT_PX))
        .canvas_padding_color(colours::PAGE)
        .disable_auto_css_resize();
    let gl = WebGl2Backend::new_with_options(options)?;
    Ok(Terminal::new(Canvas { gl, changed: true })?)
}

/// Hand the loop its terminal and start it.
fn run_loop<B: Surface>(mut terminal: Terminal<B>, shell: Rc<RefCell<Shell>>, alarm: Alarm) {
    let on_pass = shell;
    let pass_alarm = alarm.clone();
    let pass = Closure::<dyn FnMut()>::new(move || {
        pass_alarm.fired();
        let mut shell = on_pass.borrow_mut();
        let shell = &mut *shell;

        for effect in std::mem::take(&mut shell.pending) {
            shell.dispatch(effect);
        }
        // Api replies are answers to something the user asked — those show
        // up straight away.
        loop {
            // Popped one at a time rather than held borrowed: apply_event can
            // queue effects whose replies want this same queue.
            let Some(event) = shell.replies.borrow_mut().pop_front() else { break };
            shell.pending.extend(shell.app.apply_event(event));
            shell.dirty = true;
        }

        // The native loop wakes once a poll, or for input; matching it here
        // is both the frame budget and the feel. A pass that is neither —
        // the visualizer's frames between its ticks — only books the next.
        // Audio status waits for the poll tick like it does natively; news
        // from the player (a command's fresh status, a refusal) does not.
        let poll = crate::tui::poll_interval(&shell.app);
        let due = shell.dirty || shell.audio.has_news() || shell.last_render.elapsed() >= poll;
        if due {
            for event in shell.audio.tick() {
                shell.pending.extend(shell.app.apply_event(event));
            }
            if shell.spun.elapsed().as_millis() >= SPIN_EVERY_MS {
                shell.app.spinner = shell.app.spinner.wrapping_add(1);
                shell.spun = Instant::now();
            }
            // The plays owed go out from here too (play-reporting clause
            // 8): the native shells post from their tick, and this pass is
            // the browser's. Not the whole tick — its reconcile reads the
            // system clock, which wasm32 has none of.
            let owed = shell.app.stats_flush_due(Instant::now());
            shell.pending.extend(owed);
            // There is no process to quit in a tab; parking the flag turns
            // Quit into a no-op instead of a frozen screen.
            shell.app.should_quit = false;

            // A resize repaints everything: the backend is told first, and
            // ratatui forgets what it last drew, so every cell goes out
            // again. The DOM grid needs that even when the size in cells did
            // not change — it rebuilt itself blank, and a diff against the
            // old frame would leave it that way.
            if std::mem::take(&mut shell.repaint) {
                terminal.backend_mut().follow_resize();
                terminal.clear().expect("the backend refused to clear");
            }
            // Only now, and only here, does ratatui diff a frame: between
            // draws nothing is handed to it at all.
            terminal
                .draw(|frame| {
                    ui::render(frame, &mut shell.app);
                    // In the page's colours, whichever surface paints them:
                    // the DOM fallback looks as the canvas does.
                    colours::settle(frame.buffer_mut());
                })
                .expect("the backend refused a frame");
            shell.last_render = Instant::now();
            shell.dirty = false;
        }

        let urgent = !shell.pending.is_empty()
            || shell.audio.has_news()
            || !shell.replies.borrow().is_empty();
        pass_alarm.set(pace::next_wake(
            urgent,
            shell.app.drawing_audio(),
            shell.last_render.elapsed(),
            crate::tui::poll_interval(&shell.app),
        ));
    });
    *alarm.0.pass.borrow_mut() = Some(pass);
    alarm.frame();
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
