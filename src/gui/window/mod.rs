//! `mstream-player gui --window`: the GUI in a native window of its own —
//! the window-mode spike. Built only with `--features desktop` (or `window`).
//!
//! Step 1 asked whether the GUI's cell buffer survives the trip into a
//! window: the same `render` the terminal loop calls fills a ratatui buffer,
//! and ratatui-wgpu draws that buffer onto a wgpu surface instead of writing
//! escapes to a tty. Step 2 is the shell around it: the real player, started
//! the way the terminal starts it, run by the terminal loop's own two halves
//! (`frame` and `input` in gui/mod.rs) from winit's event loop. A frame runs
//! on each redraw, and the wait it answers is when the next redraw is asked
//! for, so the terminal's cadence carries over: 10 ms while covers are hot,
//! 33 ms while audio draws, a blinking caret's flip, else the 100 ms poll.
//! Step 3 is input: the keyboard, an input method and the pointer, turned
//! into the crossterm events a terminal would have sent (input.rs) and fed
//! through the same input half. The terminal path does not come through
//! this module at all.
//!
//! Three levers ride along, all hidden. `MSTREAM_WINDOW_SIZE=<cols>,<rows>`
//! opens the window at another grid than 100×30 — 70,20 shows the mini
//! player. `MSTREAM_WINDOW_DUMP=<dir>` writes what the window holds as text,
//! and the same Gui drawn into a `TestBackend` of the same size, a few
//! frames in, and says on stderr whether they match. That is how the spike
//! checks fidelity without reading pixels. `MSTREAM_WINDOW_SCRIPT=<file>`
//! plays keys, text and pointer gestures into the window and dumps what it
//! shows, one step a frame (script.rs has the commands): the spike's way to
//! prove input on a machine that may not send a window synthetic events.
//! A fourth, `MSTREAM_WINDOW_STATS=<path>`, writes what the frames cost
//! when the window closes (stats.rs).
//!
//! Step 5 is album art: the window draws covers as textures over the grid
//! (covers.rs) where it drew the ▀-mosaic, through a `Graphics` that hands
//! each cover to the window instead of encoding it for a terminal.
//!
//! What a terminal does for its programs without being asked, the window
//! does here: an input method's composition is drawn in the field that has
//! the keyboard, and its candidate list floats by that field's caret; the
//! input method is on only while a field has the keyboard, so a Japanese
//! keyboard's digits still switch rooms; Cmd+V (Ctrl+V elsewhere) types the
//! clipboard into the field; Cmd+C (Ctrl+Shift+C or Ctrl+Insert elsewhere)
//! is the Admin log's `y`, to the clipboard and never as OSC 52; a button
//! let go outside the window lets go;
//! a move to a screen of another scale re-sizes the type; a drag on the
//! window's edge steps by whole cells, where the platform allows it; and
//! what is typed or clicked while the window stands blank, its renderer
//! still being built, takes effect once the first frame has drawn (held.rs).
//!
//! What the terminal's main does on the way out, the window does itself:
//! the launcher's instance lock is dropped after the App, or in `exiting`
//! when a Cmd-Q on macOS ends the process without returning from the event
//! loop. A panic is printed without the terminal hook's escapes, with
//! where the log is.

mod covers;
mod held;
mod icon;
mod input;
#[cfg(test)]
mod render_tests;
mod script;
mod stats;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::style::Color;
use ratatui_wgpu::{Builder, Built, ColorTable, Dimensions, Font, Fonts, WgpuBackend};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowId};

use covers::{Board, CoverPost};
use held::{Held, HeldInput};
use input::{Grid, Raw, Translator};
use script::{Input, Script, Step};
use stats::{Counted, Lap, Stats};

use super::{
    Channels, Ctx, Flow, Gui, Host, finish, frame, input, refresh_book, render, saves_config,
};
use crate::instance::Instance;
use crate::kit::theme::th;
use crate::runtime::block_on;

const TITLE: &str = "mStream Player";
/// The text size in points; the backend is handed pixels, so this is
/// multiplied by the display's scale factor, or a Retina screen would get
/// type half the size a terminal shows.
const FONT_PT: f64 = 16.0;
/// The grid the window opens at unless `MSTREAM_WINDOW_SIZE` says
/// otherwise: the installer's own terminal window, and the size the render
/// tests draw at.
const GRID: (u16, u16) = (100, 30);
/// Hack at 16 points is eight points wide: the guess the window opens on,
/// before the backend exists to say what a cell really is.
const CELL_GUESS_PT: f64 = 8.0;
/// How long a redraw that was asked for may stay undelivered before the
/// loop stops spinning on it and sleeps between checks instead.
const STALLED_REDRAW: Duration = Duration::from_millis(100);
/// How long the window stays hidden on Windows waiting for its first
/// presented frame before it is shown anyway — with a frame drawn into it
/// if the backend has come by then, else blank until it does: longer than
/// a cold start's first present (1.6–2.8 s in the stats lever's runs
/// there), and short enough that a first frame that never comes still
/// leaves a window to close.
const SHOW_BY: Duration = Duration::from_secs(4);
/// How often the loop looks at the early threads while the backend waits
/// on them: a wake every few milliseconds for the moments they take, so the
/// backend follows them by no more than that.
const EARLY_POLL: Duration = Duration::from_millis(4);
/// How long `open` waits for the backend's build before it leaves it to the
/// loop's polls. On macOS the loop's next turn after the window is made
/// comes only once AppKit has put the window on screen, 40–50 ms later
/// here, so a build done in its usual 10 ms would sit unclaimed for that
/// long and the first frame would follow the window's show instead of
/// overlapping it: 40 ms later than when the build ran inline. A build
/// still running after this is one the loop must not block on (a slow
/// driver's shader compiles), and the polls take it from there.
const OPEN_WAIT: Duration = Duration::from_millis(50);
/// How long the way out waits for a renderer build still running when
/// the quit came, so what it built is dropped on the loop's thread rather
/// than on its own (`teardown`): far past a build's usual 10 ms, and a
/// slow driver's shader compiles, yet short enough that a hung one does
/// not hold the quit for long.
const BUILD_AT_QUIT: Duration = Duration::from_secs(2);
/// The frame the fidelity dump waits for: the first has the window at its
/// opening size, the resize to the grid lands a frame or two later.
const DUMP_AT_FRAME: u32 = 5;

type WindowTerminal = Terminal<Counted<WgpuBackend<'static, 'static, CoverPost>>>;
/// What the backend's thread hands back: the backend but for its glyph
/// caches, which are not `Send` (the vendored builder's `Built`).
type Parts = Built<'static, 'static, CoverPost>;
/// The builder and the surface made for it, on their way to that thread.
type BuildJob = (Builder<'static, CoverPost>, wgpu::Surface<'static>);

/// The exit code of a window that could not be opened at all: no display
/// (winit's event loop would not start), libxkbcommon-x11 missing on an
/// X11 session (`x11_keyboard_missing`), a window the platform refused, or
/// a backend that never came (no wgpu adapter or backend to draw with) —
/// every way out before the first frame. Its message lines are printed as
/// before. A launcher that gets it falls back to the terminal route
/// (`gui` in a terminal, or the TUI); 1 stays the code for everything else,
/// a frame that failed in a window that was up included, and 2 is clap's
/// usage error.
pub(crate) const NO_WINDOW: i32 = 3;

/// The player in a window, from a Gui and workers that `gui::start` has
/// already brought up; the exit code is the terminal's (0, or 1 when a
/// frame failed), or [`NO_WINDOW`] when there is no window to open. `instance` is the
/// launcher's instance lock, if this run holds one: dropped last, after the
/// App, so its sidecar goes on every way out — the Cmd-Q that ends the
/// process inside AppKit included, where `exiting` drops it.
pub(super) fn run(mut gui: Gui, channels: Channels, instance: Option<Instance>) -> i32 {
    // First, so the stats' clock (when the lever is set) starts at entry.
    let mut stats = Stats::from_env();
    let mut lap = Lap::start(&stats);
    install_panic_hook();
    // The taskbar identity the launcher stub also names (identity.rs), set
    // before any window exists: the taskbar reads it when one first shows.
    #[cfg(all(windows, feature = "desktop"))]
    crate::identity::set_windows_aumid();
    // The window icon's decode, overlapping the loop's start like the faces
    // (icon.rs; on macOS AppKit decodes the Dock's copy itself).
    #[cfg(not(target_os = "macos"))]
    let icon = icon::decode_early();
    // The faces first, on their own thread, so the search overlaps all of
    // the event loop's start and the window's creation.
    let faces = Early::faces(rust_i18n::locale().to_string());
    let grid = opening_grid();
    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("gui --window: no display to open a window on ({e})");
            let ctx = Ctx::new(&gui.app, channels);
            finish(&mut gui, &ctx);
            return NO_WINDOW;
        }
    };
    // From here the player draws into a window of its own, so a copy goes
    // to the pasteboard or a tool, never as OSC 52 into the shell that may
    // have launched it. The fallback above runs in the terminal and keeps
    // the terminal's route.
    crate::kit::clipboard::set_windowed();
    lap.mark(&mut stats, "event_loop");
    let early = Early {
        faces,
        gpu: Early::gpu(event_loop.owned_display_handle()),
        #[cfg(not(target_os = "macos"))]
        icon,
    };
    let dump = std::env::var_os("MSTREAM_WINDOW_DUMP").map(PathBuf::from);
    // Covers are the window's to draw: every Graphics the GUI forks for a
    // slot is forked from this one, so they all record onto the board.
    let board = Arc::new(Board::default());
    gui.app.graphics = crate::tui::graphics::Graphics::hosted(board.clone());
    // And the board hears of every overlay as it is drawn, so a cover a
    // modal opens over is not painted over the modal on the frame it opens.
    let watching = board.clone();
    gui.ui.watch_overlays(move |rect| watching.overlay(rect));
    let ctx = Ctx::new(&gui.app, channels);
    send_early(&mut gui, &ctx);
    let mut app = App {
        gui,
        ctx,
        grid,
        board,
        window: None,
        terminal: None,
        next_frame: Instant::now(),
        asked: None,
        frames: 0,
        dump,
        exit_code: 0,
        done: false,
        translator: Translator::new(),
        modifiers: ModifiersState::empty(),
        preedit: String::new(),
        ime_allowed: false,
        ime_area: None,
        faces: Vec::new(),
        scale: 1.0,
        script: Script::from_env(),
        script_until: None,
        quit_flushed: false,
        min_surface: PhysicalSize::new(1, 1),
        shown: false,
        opened_at: None,
        stats,
        instance,
        restore_at: None,
        last_press: None,
        lap,
        early,
        quit_at: None,
        exit_laps: Vec::new(),
        exit_clock: None,
        held: HeldInput::default(),
        opened_logical: None,
        building: None,
        frozen_until: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("gui --window: {e}");
        // A loop that failed before any frame was drawn never had a window
        // to show; one that failed later had, and a launcher's terminal
        // route would not help it.
        app.exit_code = if app.frames == 0 { NO_WINDOW } else { 1 };
    }
    // `exiting` has torn down already on every way out winit reports; this
    // is for the one it does not.
    app.teardown();
    // The lock goes last, as main drops it after the terminal's run: after
    // the App, its workers' channels and the control face are gone, so a
    // second player the launcher starts once the sidecar is gone finds
    // this one's queue saved and its port free.
    let instance = app.instance.take();
    let code = app.exit_code;
    let mut laps = std::mem::take(&mut app.exit_laps);
    let clock = app.exit_clock.take();
    drop(app);
    let app_dropped = Instant::now();
    drop(instance);
    if let Some(at) = clock {
        laps.push(("app", app_dropped - at));
        laps.push(("lock", app_dropped.elapsed()));
    }
    exit_report(&laps);
    code
}

/// The player's first effects — the connect among them — sent to their
/// workers now, while the window and its renderer are still being built,
/// rather than by the first frame as the terminal's loop sends them. The
/// first frame can come seconds after the window opens (a slow GPU's
/// pipeline build), and the input held until then is replayed right after
/// it: sent by frame 1, the connect was still on the wire for every held
/// key, and a key that needs the server (`/` opens the search's field only
/// once connected) did nothing, so `/ab` typed into a blank window left an
/// empty field. Sent here, the answer is in by frame 1 whenever the server
/// answers faster than the window comes up; frame 1 applies it before the
/// replay. A Save among them reloads the Gui's config copy, as `frame`
/// does after its own dispatch.
fn send_early(gui: &mut Gui, ctx: &Ctx) {
    let saving = saves_config(&gui.pending);
    let ch = &ctx.channels;
    crate::tui::dispatch(&gui.app, &mut gui.pending, &ch.audio_tx, &ch.api_tx, &ch.event_tx);
    if saving && let Ok(fresh) = crate::config::load() {
        gui.config = fresh;
        refresh_book(gui);
    }
}

/// What winit's X11 keyboard needs and loads only at run time, through
/// xkbcommon-dl, by the two names it tries; it panics (exit 101, a
/// backtrace hint and no word of what to install) before any window opens
/// when neither loads. The `.so.0` is the runtime package's file, the bare
/// name the -dev package's link.
#[cfg(any(target_os = "linux", test))]
const XKB_X11: [&std::ffi::CStr; 2] = [c"libxkbcommon-x11.so.0", c"libxkbcommon-x11.so"];

/// On an X11 session, the line to leave on when libxkbcommon-x11 cannot be
/// loaded, naming the package that brings it; `None` when it loads or the
/// session is not X11. The session is read as winit reads it: Wayland
/// when `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set (winit's Wayland path
/// needs no xkbcommon-x11), X11 when only `DISPLAY` is; with neither,
/// winit's own error says there is no display. The deb and rpm recommend
/// the package rather than require it (Cargo.toml says why), so a desktop
/// without it is a case to meet with words, not a crash.
///
/// `gui::run` asks before the player starts, so a window that cannot open
/// leaves before any worker, audio device or connection is up.
#[cfg(target_os = "linux")]
pub(super) fn x11_keyboard_missing() -> Option<String> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    x11_keyboard_missing_with(set, loads)
}

/// [`x11_keyboard_missing`] over the environment (`set`) and the loader
/// (`loads`), passed in so the rule is a pure function — and built for the
/// tests on every OS, so each CI leg runs them.
#[cfg(any(target_os = "linux", test))]
fn x11_keyboard_missing_with(
    set: impl Fn(&str) -> bool,
    loads: impl Fn(&std::ffi::CStr) -> bool,
) -> Option<String> {
    if set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET") || !set("DISPLAY") {
        return None;
    }
    if XKB_X11.into_iter().any(loads) {
        return None;
    }
    Some(
        "gui --window: the window needs libxkbcommon-x11, which is not installed — install \
         libxkbcommon-x11-0 (Debian, Ubuntu) or libxkbcommon-x11 (Fedora), or run \
         `mstream-player gui` in a terminal"
            .to_string(),
    )
}

/// Whether the dynamic loader can open a library by name: opened, then
/// closed again at once, for xkbcommon-dl to open for itself. The loader
/// is libc's own (`dlopen` is in glibc's libc.so.6 since 2.34, and in
/// musl's libc), declared here rather than through a crate for these two
/// calls; the binary links it already for every other `dlopen` its
/// dependencies make.
#[cfg(target_os = "linux")]
fn loads(name: &std::ffi::CStr) -> bool {
    use std::ffi::{c_char, c_int, c_void};
    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }
    /// Resolve symbols as they are used, and keep them out of the global
    /// namespace: the probe binds nothing.
    const RTLD_LAZY: c_int = 0x0001;
    // SAFETY: `name` is a NUL-terminated C string that outlives the call,
    // and a handle dlopen returns is closed exactly once. Opening runs the
    // library's initialisers, which xkbcommon's are not beyond: it is the
    // library winit opens in the next breath anyway.
    unsafe {
        let handle = dlopen(name.as_ptr(), RTLD_LAZY);
        if handle.is_null() {
            return false;
        }
        dlclose(handle);
    }
    true
}

/// The window's panic hook, chained in front of the one in place (the
/// default, which prints the message and where). Not the terminal's
/// (`tui::install_panic_hook`): that one hands mouse capture back and pops
/// a window title the player pushed, and with no terminal claimed the
/// first is escape noise on whatever launched the window and the second
/// pops a title that is not ours. What it shares is the filter: a panic on
/// a thread whose panics are caught (the audio thread's, which becomes an
/// AudioFailed the GUI shows; the decoder's prepare) is not printed, since
/// the player goes on and says so itself. Any other panic is printed, then
/// where the log is, if one is being written, for whoever files the bug.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if crate::tui::worker::panics_are_caught(std::thread::current().name()) {
            return;
        }
        previous(info);
        if let Some(note) = panic_log_note(crate::logging::active_now().as_deref()) {
            eprintln!("{note}");
        }
    }));
}

/// The line after a panic's own that says where the log is: only for a
/// log file that is there to be read.
fn panic_log_note(log: Option<&Path>) -> Option<String> {
    let log = log.filter(|path| path.is_file())?;
    Some(format!("mstream-player: the log is at {}", log.display()))
}

/// The grid to open at: `MSTREAM_WINDOW_SIZE=<cols>,<rows>`, or [`GRID`].
/// A value that does not read as two sizes of at least one cell is said
/// so and ignored.
fn opening_grid() -> (u16, u16) {
    let Some(raw) = std::env::var_os("MSTREAM_WINDOW_SIZE") else { return GRID };
    let raw = raw.to_string_lossy();
    let parsed = raw.split_once(',').and_then(|(cols, rows)| {
        let cols: u16 = cols.trim().parse().ok()?;
        let rows: u16 = rows.trim().parse().ok()?;
        (cols > 0 && rows > 0).then_some((cols, rows))
    });
    parsed.unwrap_or_else(|| {
        eprintln!(
            "gui --window: MSTREAM_WINDOW_SIZE={raw} is not <cols>,<rows>; opening at 100,30"
        );
        GRID
    })
}

struct App {
    gui: Gui,
    ctx: Ctx,
    grid: (u16, u16),
    /// This frame's covers, between the drawing path and the backend's
    /// post-processor.
    board: Arc<Board>,
    window: Option<Arc<Window>>,
    terminal: Option<WindowTerminal>,
    /// When the last frame's wait runs out.
    next_frame: Instant,
    /// When a redraw was asked for that has not arrived yet.
    asked: Option<Instant>,
    frames: u32,
    dump: Option<PathBuf>,
    exit_code: i32,
    /// The teardown has run.
    done: bool,
    /// Pixels to cells, and the mouse state a terminal keeps.
    translator: Translator,
    /// The modifiers held, as winit last reported them: its key events
    /// carry none of their own.
    modifiers: ModifiersState,
    /// What an input method is composing, not yet committed; the kit draws
    /// it in the focused field from its own copy (`Surface::composition`).
    preedit: String,
    /// The input method is on: last frame drew a field with the keyboard.
    ime_allowed: bool,
    /// The caret cell the input method was last told of, in pixels: the
    /// top-left and the size.
    ime_area: Option<((u32, u32), (u32, u32))>,
    /// The regular faces the backend was built with, Hack first, kept to
    /// build them again at another size when the display's scale changes.
    faces: Vec<Font<'static>>,
    /// The display scale the type is sized for.
    scale: f64,
    script: Option<Script>,
    /// When the script's current `wait` runs out.
    script_until: Option<Instant>,
    /// A quit came through the input half, which flushed the saver: the
    /// teardown does not flush it again, and input stops.
    quit_flushed: bool,
    /// One cell, in pixels: the least surface the backend can draw on.
    min_surface: PhysicalSize<u32>,
    /// The window is on screen. On Windows it is made hidden and shown by
    /// `show` once a frame has been presented to it, so the blank window of
    /// the GPU's setup is never seen; elsewhere it is visible from its
    /// creation, as winit makes it, and this is true from `open`.
    shown: bool,
    /// When the window was made: the deadline a hidden one is shown by
    /// counts from here.
    opened_at: Option<Instant>,
    /// What the frames cost, while `MSTREAM_WINDOW_STATS` is set.
    stats: Option<Stats>,
    /// The launcher's instance lock, held until the App is gone.
    instance: Option<Instance>,
    /// When the script's `minimise` restores the window.
    restore_at: Option<Instant>,
    /// The last button press, as the dump reports it: the cell, what the
    /// GUI's hit found there, and whether a drag began. A press on the
    /// queue's grip that reads as the row's click shows up here.
    last_press: Option<String>,
    /// The startup's stopwatch (the stats lever's): it runs from the
    /// event loop's creation to the backend's first frame.
    lap: Lap,
    /// The startup's work begun before the loop, until `build` joins it.
    early: Early,
    /// When the quit was decided, with the stats lever: the way out's
    /// clock, which the teardown laps (`exit_laps`) and leaves running.
    quit_at: Option<Instant>,
    exit_laps: Vec<(&'static str, Duration)>,
    exit_clock: Option<Instant>,
    /// What the window was given before its first frame drew, to replay
    /// after it (held.rs).
    held: HeldInput,
    /// The window's size in points as it was made: one that differs when
    /// the backend lands was resized meanwhile, and keeps that size.
    opened_logical: Option<LogicalSize<f64>>,
    /// The backend's build, while its thread runs.
    building: Option<Building>,
    /// The script's `freeze`: no frame until then.
    frozen_until: Option<Instant>,
}

/// The backend being built on a thread of its own, once the loop has made
/// the surface (which only the window's thread may): the device checked
/// against the surface, the surface configured, the atlas and the
/// pipelines. That is about 10 ms on this Mac, but it is GPU driver work
/// — shader compiles, a swapchain on DX12 — and a loop that waited on it
/// would answer nothing meanwhile, the blank Not Responding window lane 6
/// took the early threads off the loop for. `open` waits for it a moment
/// ([`OPEN_WAIT`]); after that the loop polls it as it polls those
/// (`about_to_wait`).
struct Building {
    /// What the thread built, sent as it ends. A thread that panicked
    /// sends nothing and drops its sender, which the receiver reads as
    /// disconnected. A quit while the thread runs waits for it a while
    /// ([`BUILD_AT_QUIT`]) rather than drop this: the thread's send would
    /// fail, and what it built would be dropped on that thread.
    done: Receiver<Result<Parts, String>>,
    /// The type size and the surface the build was begun for: the window
    /// may have been resized while it ran.
    font_px: u32,
    size: PhysicalSize<u32>,
}

/// What a thread's channel gave in a bounded wait for it.
#[derive(Debug, PartialEq)]
enum Drained<T> {
    /// It sent this.
    Came(T),
    /// It ended without sending (it panicked): nothing is left to drop.
    Ended,
    /// It was still running when the wait ran out.
    Running,
}

/// Whatever `done` sends within `bound`, or why nothing came.
fn drain<T>(done: &Receiver<T>, bound: Duration) -> Drained<T> {
    match done.recv_timeout(bound) {
        Ok(sent) => Drained::Came(sent),
        Err(RecvTimeoutError::Disconnected) => Drained::Ended,
        Err(RecvTimeoutError::Timeout) => Drained::Running,
    }
}

/// The build's thread: everything after the surface (the vendored
/// builder's `build_parts_with_surface`), blocking on its futures. The job
/// is taken from its slot (`App::build`), where only one hand takes it.
fn build_parts(slot: &Mutex<Option<BuildJob>>) -> Result<Parts, String> {
    let job = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    let (builder, surface) = job.ok_or("the renderer's build was taken twice")?;
    block_on(builder.build_parts_with_surface(surface))?
        .map_err(|e| format!("the window has nothing to draw with: {e}"))
}

/// The way out's steps and what each took, on stderr, just before the
/// process exits: the time from the quit to the event loop's return, the
/// GPU and window dropped, the player's own teardown, the App and the lock.
fn exit_report(laps: &[(&'static str, Duration)]) {
    if laps.is_empty() {
        return;
    }
    let steps: Vec<String> = laps
        .iter()
        .map(|(stage, took)| format!("{stage} {:.1}", took.as_secs_f64() * 1000.0))
        .collect();
    eprintln!("gui --window: way out (ms): {}", steps.join(", "));
}

/// The startup's work that needs no window, begun on threads of its own
/// before the event loop runs: the faces, and the GPU's instance, adapter
/// and device. It overlaps the event loop's start and the window's
/// creation instead of following them. The backend's build begins from it
/// once both threads are done: at once in `open` when they already are (on
/// this Mac, warm or cold, they are), else from the loop, which polls them
/// (`about_to_wait`) rather than block on a join; the build itself runs on
/// a thread of its own too, polled the same way ([`Building`]). A blocked
/// loop answered nothing — on Windows, the white window the Windows report
/// saw for two to three seconds, which DWM marks Not Responding under load
/// — and could not show a hidden window by its deadline either: a loaded
/// run there showed it 17 s after it was made, blank.
struct Early {
    faces: Option<JoinHandle<Result<Faces, String>>>,
    gpu: Option<JoinHandle<Gpu>>,
    /// The window icon's pixels (icon.rs). Not part of [`Early::ready`]: a
    /// window opens without its icon rather than wait for one, and takes it
    /// as soon as the decode is done ([`Early::icon`]).
    #[cfg(not(target_os = "macos"))]
    icon: Option<JoinHandle<Option<icon::Rgba>>>,
}

impl Early {
    fn faces(lang: String) -> Option<JoinHandle<Result<Faces, String>>> {
        std::thread::Builder::new()
            .name("window-faces".into())
            .spawn(move || find_faces(&lang))
            .ok()
    }

    fn gpu(display: OwnedDisplayHandle) -> Option<JoinHandle<Gpu>> {
        std::thread::Builder::new()
            .name("window-gpu".into())
            .spawn(move || Gpu::prepare(Box::new(display)))
            .ok()
    }

    /// Both threads are done (or never started): joining them now does not
    /// wait. A thread that panicked is done too, and its work is redone on
    /// the loop's thread when it is joined.
    fn ready(&self) -> bool {
        self.faces.as_ref().is_none_or(JoinHandle::is_finished)
            && self.gpu.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// The window icon, once: at once when the decode is done, or when
    /// `wait` says to join it (the backend's build, which follows the far
    /// longer GPU thread, so the join finds it done); else `None` and the
    /// thread kept for the next ask.
    #[cfg(not(target_os = "macos"))]
    fn icon(&mut self, wait: bool) -> Option<winit::window::Icon> {
        if !wait && !self.icon.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        let decoded = self.icon.take()?.join().ok().flatten()?;
        icon::winit_icon(decoded)
    }
}

/// The regular faces the window draws with, and what finding them took.
struct Faces {
    /// Hack first for everything it has, then the bundled symbol face for
    /// the few symbols it lacks, then a system face for symbols beyond
    /// those (a title's ♥ or ♪), then the borrowed faces for kana, hanzi
    /// and hangul, whatever the language, and last the system's colour
    /// emoji face. `with_regular_fonts` keeps that order, where
    /// `with_fonts` would sort by width and could put a borrowed face's
    /// own Latin in front of Hack's. Hack is also the builder's last
    /// resort, which is what bold and italic cells fall back to with faked
    /// styles. The emoji face comes last so a character a text face also
    /// has (✔, ㊗) keeps its text form, as a terminal draws it; a cluster
    /// only the emoji face has all of (an emoji with VS16, a flag, a ZWJ
    /// family) still goes to it whole, since the backend picks the first
    /// face that has every character of a cell.
    fonts: Vec<Font<'static>>,
    took: Vec<(&'static str, Duration)>,
}

/// The faces, found at the known paths each platform keeps them at
/// ([`symbol_faces`], [`script_faces`]): a handful of files mapped, never
/// a scan of the fonts folder, which on Windows holds hundreds.
fn find_faces(lang: &str) -> Result<Faces, String> {
    let mut took = Vec::with_capacity(3);
    let mut clock = Instant::now();
    let mut lap = |stage: &'static str| {
        let now = Instant::now();
        took.push((stage, now - clock));
        clock = now;
    };
    let mut fonts = vec![hack()?, symbols()?];
    lap("faces.hack");
    fonts.extend(symbol_fallback());
    lap("faces.symbols");
    fonts.extend(script_fallbacks(lang));
    lap("faces.scripts");
    fonts.extend(emoji_fallback());
    lap("faces.emoji");
    Ok(Faces { fonts, took })
}

/// The GPU before the window: the instance, and an adapter with a device
/// and queue requested from it, which the builder takes if the adapter can
/// present to the window's surface and otherwise replaces with its own.
/// None of it needs the window, and on Windows (two GPUs) it is the
/// likeliest part of the blank seconds: here it costs about 15 ms, and the
/// stats lever's `early.gpu.*` stages say what it costs there.
struct Gpu {
    instance: wgpu::Instance,
    device: Option<(wgpu::Adapter, wgpu::Device, wgpu::Queue)>,
    took: Vec<(&'static str, Duration)>,
}

impl Gpu {
    /// The instance and adapter by the rule every window of ours follows
    /// ([`crate::gpu_pick`]: on Windows DX12 or Vulkan alone, hardware
    /// before WARP, never GL), then a device from that adapter. `display`
    /// is the event loop's on the early thread, or the window's when that
    /// thread did not run; the two are the same display, and on Wayland and
    /// X11 the instance needs it before a surface can exist.
    fn prepare(display: Box<dyn wgpu::wgt::WgpuHasDisplayHandle>) -> Gpu {
        let options = wgpu::RequestAdapterOptions::default();
        let choice = crate::gpu_pick::choose(Some(display), |instance| {
            let adapter = block_on(instance.request_adapter(&options)).ok()?.ok()?;
            Some((adapter, ()))
        });
        let started = Instant::now();
        let device = choice.found.and_then(|(adapter, ())| {
            let descriptor =
                wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() };
            let (device, queue) = block_on(adapter.request_device(&descriptor)).ok()?.ok()?;
            Some((adapter, device, queue))
        });
        let took = vec![
            ("gpu.instance", choice.instance_took),
            ("gpu.adapter", choice.adapter_took),
            ("gpu.device", started.elapsed()),
        ];
        Gpu { instance: choice.instance, device, took }
    }
}

/// The window as the loop's [`Host`]: the pointer over something
/// clickable is the platform's hand, as the terminal's OSC 22 shape is.
struct WindowHost<'a>(&'a Window);

impl Host for WindowHost<'_> {
    fn pointer(&mut self, hand: bool) {
        self.0.set_cursor(if hand { CursorIcon::Pointer } else { CursorIcon::Default });
    }
}

impl App {
    /// The window, and the backend that draws cells onto it: the backend's
    /// build begun at once if the early threads are done, and installed if
    /// it is done within [`OPEN_WAIT`]; else each as soon as it can be, from
    /// the loop (`about_to_wait`), which goes on answering the window
    /// meanwhile.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let (cols, rows) = self.grid;
        let opening = LogicalSize::new(f64::from(cols) * CELL_GUESS_PT, f64::from(rows) * FONT_PT);
        let attributes = Window::default_attributes().with_title(TITLE).with_inner_size(opening);
        // The app id, as both halves of X11's WM_CLASS and as Wayland's
        // app_id: what a desktop shell matches to the desktop entry of the
        // same name, for its icon and its dock grouping (identity.rs). One
        // call serves both: winit's X11 and Wayland `with_name` set the
        // same attribute, which whichever backend the session picks reads
        // (Wayland takes the first name and ignores the second).
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let attributes = {
            use crate::identity::APP_ID;
            use winit::platform::x11::WindowAttributesExtX11;
            attributes.with_name(APP_ID, APP_ID)
        };
        // The icon with the window when its decode is done by now (the
        // build sets it otherwise, before the first frame).
        #[cfg(not(target_os = "macos"))]
        let attributes = attributes.with_window_icon(self.early.icon(false));
        // On Windows the window is made hidden and `show` puts it on screen
        // once a frame has been presented to it; the other platforms show
        // it now, as before (macOS presents into it within a frame, and
        // winit's `Occluded(false)` there presents it again as it comes into
        // view).
        let attributes = if cfg!(windows) { attributes.with_visible(false) } else { attributes };
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| format!("the window would not open: {e}"))?,
        );
        self.lap.mark(&mut self.stats, "window");
        self.opened_at = Some(Instant::now());
        self.opened_logical = Some(window.inner_size().to_logical(window.scale_factor()));
        self.shown = !cfg!(windows);
        self.window = Some(window);
        self.poll_build(OPEN_WAIT)
    }

    /// The backend's build begun, onto the window `open` made, from what
    /// the early threads found: the surface here, on the window's thread
    /// (the one part that must be), the rest on a thread of its own that
    /// the loop polls ([`Building`]); `install` takes it from there. Called
    /// once both early threads are done, so the joins do not wait.
    fn build(&mut self) -> Result<(), String> {
        let Some(window) = self.window.clone() else { return Ok(()) };
        // The loop's wait for the early threads, when it had to wait: none
        // when `open` builds at once. The joins after it then wait for
        // nothing, and their stages say so.
        self.lap.mark(&mut self.stats, "threads.wait");
        // Read now rather than at the window's creation: the loop may have
        // run in between, and a scale change then found no backend to tell.
        self.scale = window.scale_factor();
        let font_px = font_px(self.scale);
        // What `run` began before the loop: the faces, and the GPU's
        // instance, adapter and device. Whatever did not come (a thread
        // that could not start, or panicked) is done here instead.
        let faces = self.early.faces.take().and_then(|thread| thread.join().ok());
        let faces = match faces {
            Some(found) => found?,
            None => find_faces(&rust_i18n::locale())?,
        };
        self.lap.mark(&mut self.stats, "faces.wait");
        let gpu = self.early.gpu.take().and_then(|thread| thread.join().ok());
        let gpu = gpu.unwrap_or_else(|| Gpu::prepare(Box::new(window.clone())));
        self.lap.mark(&mut self.stats, "gpu.wait");
        #[cfg(not(target_os = "macos"))]
        if let Some(icon) = self.early.icon(true) {
            window.set_window_icon(Some(icon));
        }
        if let Some(stats) = self.stats.as_mut() {
            for (stage, took) in faces.took.iter().chain(&gpu.took) {
                stats.stage(&format!("early.{stage}"), *took);
            }
        }
        let size = window.inner_size();
        let dimensions = Dimensions {
            width: NonZeroU32::new(size.width.max(1)).expect("at least one"),
            height: NonZeroU32::new(size.height.max(1)).expect("at least one"),
        };
        self.faces.clone_from(&faces.fonts);
        // The pinned truecolour palette always has a ground; black is only
        // the answer to a palette that somehow resolved without one.
        let theme = th();
        let mut builder =
            Builder::<CoverPost>::from_font_and_user_data(hack()?, self.board.clone())
                .with_regular_fonts(faces.fonts)
                .with_font_size_px(font_px)
                .with_width_and_height(dimensions)
                .with_bg_color(theme.ground.unwrap_or(Color::Black))
                .with_fg_color(theme.text)
                .with_color_table(named_colours(theme))
                .with_instance(gpu.instance);
        if let Some((adapter, device, queue)) = gpu.device {
            builder = builder.with_device(adapter, device, queue);
        }
        let surface = builder
            .create_surface(window.clone())
            .map_err(|e| format!("the window has nothing to draw with: {e}"))?;
        self.lap.mark(&mut self.stats, "surface");
        // Handed over through a slot rather than moved into the closure,
        // so a thread that cannot start leaves the job here to be done on
        // the loop's thread instead, as the early threads' work is.
        let job: Arc<Mutex<Option<BuildJob>>> = Arc::new(Mutex::new(Some((builder, surface))));
        let theirs = job.clone();
        let (sender, done) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("window-backend".into()).spawn(move || {
            let _ = sender.send(build_parts(&theirs));
        });
        match spawned {
            Ok(_) => {
                self.building = Some(Building { done, font_px, size });
                Ok(())
            }
            Err(_) => self.install(build_parts(&job), font_px, size),
        }
    }

    /// The build begun once the early threads are done, and the backend
    /// installed once its thread is, waiting for that up to `wait`: `open`
    /// waits a moment ([`OPEN_WAIT`]), the loop's polls not at all.
    fn poll_build(&mut self, wait: Duration) -> Result<(), String> {
        if self.building.is_none() && self.early.ready() {
            self.build()?;
        }
        let Some(building) = &self.building else { return Ok(()) };
        let built = match building.done.recv_timeout(wait) {
            Ok(built) => built,
            Err(RecvTimeoutError::Timeout) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => Err("the renderer's build panicked".into()),
        };
        let Some(Building { font_px, size, .. }) = self.building.take() else { return Ok(()) };
        self.install(built, font_px, size)
    }

    /// The backend the build made, onto the window: then the window sized
    /// to the grid and the first frame asked for. `font_px` and `size` are
    /// what the build was begun with.
    fn install(
        &mut self,
        built: Result<Parts, String>,
        font_px: u32,
        size: PhysicalSize<u32>,
    ) -> Result<(), String> {
        let Some(window) = self.window.clone() else { return Ok(()) };
        // The glyph caches, made here: the one part that could not cross
        // from the build's thread.
        let backend = built?.finish();
        // From the surface to here: the build's thread and the poll that
        // found it done.
        self.lap.mark(&mut self.stats, "backend");
        // Which adapter, through which backend: the one line a report needs
        // (otherwise it is only in wgpu's own log, at debug level). Asked of
        // the backend, since the build drops the adapter it was given when
        // that cannot present to the window, and requests its own.
        let info = backend.adapter_info();
        let software = if info.device_type == wgpu::DeviceType::Cpu { ", in software" } else { "" };
        eprintln!("gui --window: drawing with {} through {:?}{software}", info.name, info.backend);
        if let Some(stats) = self.stats.as_mut() {
            for (stage, took) in backend.build_timings() {
                stats.stage(&format!("backend.{stage}"), *took);
            }
        }
        let timed = self.stats.is_some();
        let mut terminal =
            Terminal::new(Counted::new(backend, timed)).map_err(|e| e.to_string())?;
        // A window resized while the build ran: the surface to its size
        // now, before anything is measured on it.
        let now = window.inner_size();
        if now != size {
            terminal.backend_mut().resize(now.width.max(1), now.height.max(1));
        }

        // The cell's width comes back from the backend, not from
        // arithmetic here: it is the narrowest of the faces it was given,
        // which the backend keeps to itself. The surface's width over its
        // columns is that width exactly, because the spare pixels (fewer
        // than a cell) are fewer than the columns for any grid wider than a
        // cell is. Rows are not so lucky — 30 rows of 32 px can have 31
        // spare — but a row is the type's size exactly, which is what the
        // builder was given. The window is then set to the grid exactly —
        // grown when the opening guess fell short, trimmed when it
        // overshot — so the window and the render tests draw the same
        // number of cells.
        let (cols, rows) = self.grid;
        let reported = terminal.backend_mut().window_size().map_err(|e| e.to_string())?;
        let (got_cols, got_rows) =
            (reported.columns_rows.width.max(1), reported.columns_rows.height.max(1));
        let cell = (u32::from(reported.pixels.width) / u32::from(got_cols), font_px);
        let want = fit(cell, (cols, rows));
        // Unless the window was resized while the backend was built — by
        // hand, by a tiling window manager, by the script's `resize` — when
        // the size it was given is kept, as a terminal keeps the size it is
        // dragged to, and the grid is what fits it. A change of scale alone
        // keeps the size in points, so it is not one.
        let points = now.to_logical::<f64>(window.scale_factor());
        let resized = self.opened_logical.is_some_and(|opened| {
            (points.width - opened.width).abs() > 0.5 || (points.height - opened.height).abs() > 0.5
        });
        if resized {
            eprintln!(
                "gui --window: {font_px} px type at scale {}, cell {}×{} px; resized to {}×{} px \
                 while the renderer was built, {got_cols}×{got_rows} cells, kept",
                window.scale_factor(),
                cell.0,
                cell.1,
                now.width,
                now.height,
            );
        } else {
            eprintln!(
                "gui --window: {font_px} px type at scale {}, cell {}×{} px, opened at \
                 {got_cols}×{got_rows} cells; sizing to {cols}×{rows}, {}×{} px",
                window.scale_factor(),
                cell.0,
                cell.1,
                want.width,
                want.height,
            );
            if want != now
                && let Some(now) = window.request_inner_size(want)
            {
                terminal.backend_mut().resize(now.width, now.height);
            }
        }

        // A terminal is never less than a cell, and ratatui-wgpu counts on
        // that: a surface with room for no whole cell reports a 0×0 grid,
        // and its cursor clamp (`width - 1`) then underflows. The window is
        // held to one cell for the platforms that honour a minimum, and
        // `resized` holds the surface to it for any that do not.
        self.min_surface = PhysicalSize::new(cell.0.max(1), cell.1.max(1));
        window.set_min_inner_size(Some(self.min_surface));
        snap_to_cells(&window, self.min_surface, self.scale);
        // The input method stays off until a field has the keyboard
        // (`sync_ime`, after each frame); winit opens a window with it off.

        window.request_redraw();
        self.asked = Some(Instant::now());
        self.terminal = Some(terminal);
        self.lap.mark(&mut self.stats, "sizing");
        Ok(())
    }

    /// What came while there was no frame to read it against — the
    /// renderer being built, and the first frame not drawn — through the
    /// handler it would have gone through, in order, now that the first
    /// frame has drawn: a click is a cell only on a grid, and lands on what
    /// that frame registered there. A resize or a scale change is the
    /// window's size or scale now, whatever it was then.
    ///
    /// One act a frame (held.rs, `next_step`), the next frame asked for at
    /// once: a key that opens a room acts on what the next frame draws, so
    /// a frame runs between two keys as it would for hands. Called after
    /// every frame while the queue drains; the first call says how much
    /// was held.
    fn replay(&mut self, event_loop: &ActiveEventLoop) {
        if self.held.is_empty() {
            return;
        }
        if self.frames == 1 {
            let (count, dropped) = self.held.count();
            let lost =
                if dropped > 0 { format!(" ({dropped} older dropped)") } else { String::new() };
            eprintln!(
                "gui --window: {count} inputs came before the first frame; replayed one act a \
                 frame{lost}"
            );
            if let Some(stats) = self.stats.as_mut() {
                stats.held(count as u64, dropped);
            }
        }
        for item in self.held.next_step() {
            if self.quit_flushed {
                break;
            }
            let Some(window) = self.window.clone() else { break };
            match item {
                Held::Raw(raw) => self.feed_now(event_loop, raw),
                Held::Resized => self.resized(event_loop, window.inner_size()),
                Held::Scale => {
                    let scale = window.scale_factor();
                    if (scale - self.scale).abs() > f64::EPSILON
                        && let Some(want) = self.rescale(event_loop, scale)
                        && let Some(now) = window.request_inner_size(want)
                    {
                        self.resized(event_loop, now);
                    }
                }
            }
        }
        self.ask_redraw();
    }

    /// Every cell to the GPU on the next frame: ratatui's clear forgets
    /// the last frame, so the next draw is a whole one.
    fn repaint_all(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = terminal.clear();
        }
        self.ask_redraw();
    }

    /// The screen the backend holds presented again on the next frame,
    /// though nothing changed: the frame is asked for now.
    fn present_again(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.backend_mut().owe_present();
        }
        self.ask_redraw();
    }

    fn ask_redraw(&mut self) {
        if let Some(window) = &self.window {
            window.request_redraw();
            self.asked.get_or_insert_with(Instant::now);
        }
    }

    /// The loop's frame half, and the time the next one is wanted by. The
    /// clock is read only for the stats lever.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if self.terminal.is_none() || self.frozen() {
            // A redraw before the backend exists (the window was asked to
            // repaint as it came into view) draws nothing and is not timed;
            // nor does one while the script holds the frame on screen.
            self.asked = None;
            return;
        }
        let started = self.stats.is_some().then(Instant::now);
        self.redraw_inner(event_loop, started);
        if let (Some(stats), Some(started)) = (self.stats.as_mut(), started) {
            stats.redraw(started.elapsed());
        }
    }

    fn redraw_inner(&mut self, event_loop: &ActiveEventLoop, started: Option<Instant>) {
        self.asked = None;
        let (Some(window), Some(terminal)) = (self.window.as_deref(), self.terminal.as_mut())
        else {
            return;
        };
        if self.frames == 0 {
            self.lap.mark(&mut self.stats, "to_first_frame");
        }
        self.board.begin_frame();
        // An owed present this frame's flush makes is a present though no
        // cell changed: the stats lever counts it as one.
        let owed = terminal.backend().owes_present();
        let framed = frame(terminal, &mut self.gui, &mut self.ctx, &mut WindowHost(window));
        let (cells, flush) = terminal.backend_mut().take_cells();
        let repaid = owed && !terminal.backend().owes_present();
        if let (Some(stats), Some(started)) = (self.stats.as_mut(), started) {
            stats.frame(cells, repaid, flush, started.elapsed());
        }
        match framed {
            Ok(wait) => {
                self.next_frame = Instant::now() + wait;
                if let Some(until) = self.script_until {
                    self.next_frame = self.next_frame.min(until);
                }
            }
            // The terminal's way with a frame that fails: the player
            // tears down and exits 1.
            Err(e) => {
                eprintln!("mstream-player: {e}");
                self.exit_code = 1;
                event_loop.exit();
                return;
            }
        }
        self.frames += 1;
        if self.frames == DUMP_AT_FRAME
            && let Some(dir) = &self.dump
            && let Err(e) = dump(dir, terminal, &mut self.gui)
        {
            eprintln!("gui --window: the dump failed: {e}");
        }
        // The first frame that handed the backend cells has been presented
        // (the same moment the stats lever calls the first present): a
        // window made hidden goes on screen now.
        if !self.shown && cells > 0 {
            self.show(true);
        }
        // What came before the first frame, now that it has drawn: one act
        // after each frame until the queue drains.
        self.replay(event_loop);
        self.sync_ime();
        self.play(event_loop);
    }

    /// The script's `freeze` holds the frame on screen.
    fn frozen(&self) -> bool {
        self.frozen_until.is_some_and(|until| Instant::now() < until)
    }

    /// The window onto the screen — on Windows, where `open` made it hidden
    /// — after its first presented frame, or at the deadline without one.
    /// Windows sends no `Occluded(false)` for a window coming into view, so
    /// the full repaint that event brings on macOS is asked for here. The
    /// stats lever is told what is seen: the frame just presented, when it
    /// was the one that showed the window, else the next one presented.
    fn show(&mut self, presented: bool) {
        self.shown = true;
        let Some(window) = &self.window else { return };
        window.set_visible(true);
        if let Some(opened) = self.opened_at {
            let why = match (presented, self.terminal.is_some()) {
                (true, _) => "after its first present",
                (false, true) => "at the deadline, unpresented",
                (false, false) => "at the deadline, before the GPU was ready",
            };
            eprintln!(
                "gui --window: shown {:.0} ms after the window was made, {why}",
                opened.elapsed().as_secs_f64() * 1000.0,
            );
        }
        if let Some(stats) = self.stats.as_mut() {
            if presented { stats.seen_as_presented() } else { stats.visible() }
        }
        self.repaint_all();
    }

    /// The hidden window's deadline has passed.
    fn show_due(&self) -> bool {
        self.opened_at.is_some_and(|at| at.elapsed() >= SHOW_BY)
    }

    /// One input through the loop's input half; false once it quit. A
    /// quit has flushed the saver already; the teardown in `exiting` does
    /// the rest. Anything else wants a frame now, as the terminal draws
    /// after every batch.
    fn feed(&mut self, event_loop: &ActiveEventLoop, event: TermEvent) -> bool {
        if self.quit_flushed {
            return false;
        }
        let fed = self.stats.is_some().then(Instant::now);
        if input(&mut self.gui, &mut self.ctx, event) == Flow::Quit {
            self.quit_flushed = true;
            self.quit_at = fed;
            event_loop.exit();
            return false;
        }
        self.ask_redraw();
        true
    }

    /// Whether a text field has the keyboard: the last frame noted the
    /// cell of a field in its topmost layer. Every field with the keyboard
    /// notes it — `gui::text_field` as it draws the caret, the DJ's genre
    /// filter while it shows its placeholder — and a modal laid over the
    /// page clears what a field beneath it noted, so a chooser with no
    /// field of its own gets its keys as keys, never a paste or a
    /// composition. Whether a caret drew would not do: a field under a
    /// modal still draws one. Nor would `App::input_mode`'s Editing, which
    /// is also what a player with no server reports, field or none.
    fn editing(&self) -> bool {
        self.gui.ui.caret_at().is_some()
    }

    /// The input method on while a field has the keyboard and off while
    /// none does, and told where the field's caret is: after each frame,
    /// and only when either changed. Off, a Japanese or Chinese keyboard's
    /// keys reach the GUI as the keys they are — 2 is the Albums room, not
    /// a full-width ２ waiting for Enter — as a terminal's do, whose input
    /// method a person turns off for a TUI's keys by hand.
    fn sync_ime(&mut self) {
        let editing = self.editing();
        let place = self.gui.ui.caret_at();
        let grid = self.grid();
        let Some(window) = self.window.clone() else { return };
        if editing != self.ime_allowed {
            // winit drops what was composing as it turns the input method
            // off, and says so with `Ime::Disabled`; the composition here
            // goes with it at once, so a field that lost the keyboard
            // never draws it.
            window.set_ime_allowed(editing);
            self.ime_allowed = editing;
            self.ime_area = None;
            if !editing {
                self.set_preedit("");
            }
        }
        // The candidate list floats by the caret's cell — after any
        // composition, which the kit draws before the caret — rather than
        // at the window's corner. The kit reports the cell as the field
        // draws it, in the blink's off phase too, where the glyph is not
        // on screen to be found.
        if let (true, Some(place), Some(grid)) = (editing, place, grid) {
            let area = grid.cell_rect(place.x, place.y);
            if self.ime_area != Some(area) {
                let ((x, y), (width, height)) = area;
                window.set_ime_cursor_area(
                    PhysicalPosition::new(x, y),
                    PhysicalSize::new(width, height),
                );
                self.ime_area = Some(area);
            }
        }
    }

    /// An input method's composition, to the kit to draw in the focused
    /// field, and said on stderr once per change for whoever is watching.
    fn set_preedit(&mut self, text: &str) {
        if text == self.preedit {
            return;
        }
        eprintln!("gui --window: preedit {text:?}");
        self.preedit.clear();
        self.preedit.push_str(text);
        self.gui.ui.set_composition(text);
        self.ask_redraw();
    }

    /// What the window saw: fed now ([`App::feed_now`]), or held behind
    /// what the replay has still to feed.
    fn feed_raw(&mut self, event_loop: &ActiveEventLoop, raw: Raw) {
        // Before the first frame there is no grid to read a pointer
        // against and nothing registered to hit: held, and replayed once
        // the first frame has drawn (`replay`). While that replay drains,
        // one act a frame, what comes joins the queue behind it rather
        // than overtake it.
        if self.frames == 0 || !self.held.is_empty() {
            self.held.push(Held::Raw(raw));
            return;
        }
        self.feed_now(event_loop, raw);
    }

    /// What the window saw, translated and fed: the replay's way in, and a
    /// live input's when nothing is held. A composition in progress stops
    /// here, for the kit to draw; the paste chord reads the clipboard while
    /// a field has the keyboard and is nothing otherwise (off a Mac, Ctrl+V
    /// with no field is still the key it was).
    fn feed_now(&mut self, event_loop: &ActiveEventLoop, mut raw: Raw) {
        if let Raw::ImePreedit(text) = &raw {
            let text = text.clone();
            self.set_preedit(&text);
            return;
        }
        if let Raw::ImeCommit(_) = raw {
            self.set_preedit("");
        }
        // The copy chord is always the window's: the Admin log's copy when
        // that log can take it, else nothing but what any key does to the
        // header's open server menu, which closes. Off a Mac, Ctrl+Shift+C
        // would otherwise reach the GUI as Ctrl+C and quit.
        if input::is_copy(&raw) {
            if super::admin::copy_chord(&mut self.gui) {
                self.ask_redraw();
            }
            return;
        }
        if input::is_paste(&raw) {
            if self.editing() {
                let Some(text) = clipboard_text() else { return };
                raw = Raw::Paste(text);
            } else if cfg!(target_os = "macos") {
                return;
            }
        }
        let Some(grid) = self.grid() else { return };
        // A press is recorded for the script's dumps as the GUI is about to
        // read it: the cell, and what the last frame registered there,
        // before the press acts.
        let press = match raw {
            Raw::Button { button, down: true } if self.script.is_some() => {
                let (x, y) = self.translator.pointer(grid);
                let hit = self.gui.ui.hit(ratatui::layout::Position { x, y });
                let hit = hit.map_or_else(|| "none".to_string(), |act| format!("{act:?}"));
                Some(format!("{button:?} at {x},{y}, hit {hit}"))
            }
            _ => None,
        };
        let events = self.translator.translate(raw, grid);
        let mut fed = true;
        for event in events {
            if !self.feed(event_loop, event) {
                fed = false;
                break;
            }
        }
        if let Some(press) = press {
            let drag = if self.gui.actions.drag.is_some() { "drag began" } else { "no drag" };
            self.last_press = Some(format!("{press}, {drag}"));
        }
        if fed {
            self.ask_redraw();
        }
    }

    /// The grid as the backend holds it now: the whole surface and the
    /// cells stretched over it (input.rs's [`Grid`] says why no cell size).
    fn grid(&mut self) -> Option<Grid> {
        let reported = self.terminal.as_mut()?.backend_mut().window_size().ok()?;
        Some(Grid {
            width: u32::from(reported.pixels.width).max(1),
            height: u32::from(reported.pixels.height).max(1),
            cols: reported.columns_rows.width.max(1),
            rows: reported.columns_rows.height.max(1),
        })
    }

    /// The script's next steps, after a frame: everything up to the next
    /// input or wait, so each input is drawn before the one after it.
    fn play(&mut self, event_loop: &ActiveEventLoop) {
        if self.quit_flushed {
            return;
        }
        if let Some(until) = self.script_until {
            if Instant::now() < until {
                return;
            }
            self.script_until = None;
        }
        // Before the first frame — the renderer still being built — only
        // the steps that need no frame run: their inputs are held and
        // replayed after it, as a person's are. The first that needs one
        // waits for it.
        let early = self.frames == 0;
        while let Some(step) = self.script.as_mut().and_then(Script::next) {
            if early && !step.needs_no_frame() {
                if let Some(script) = self.script.as_mut() {
                    script.push_front(step);
                }
                break;
            }
            match step {
                Step::Wait(time) => {
                    let until = Instant::now() + time;
                    self.script_until = Some(until);
                    self.next_frame = self.next_frame.min(until);
                    break;
                }
                Step::Inputs(inputs) => {
                    // A cell needs the grid; a step that names one runs
                    // only once there is one (`needs_no_frame`).
                    let grid = self.grid();
                    for step_input in inputs {
                        let raw = match (step_input, grid) {
                            (Input::Raw(raw), _) => raw,
                            (Input::MoveTo(col, row), Some(grid)) => {
                                let (x, y) = grid.centre(col, row);
                                Raw::Move { x, y }
                            }
                            (Input::MoveTo(..), None) => continue,
                        };
                        self.feed_raw(event_loop, raw);
                    }
                    // One input a frame; the redraw it asked for comes back
                    // here.
                    self.ask_redraw();
                    break;
                }
                Step::Dump(path) => {
                    if let Err(e) = self.script_dump(&path) {
                        eprintln!("gui --window: script dump {}: {e}", path.display());
                    }
                }
                Step::Say(text) => eprintln!("gui --window: script says {text}"),
                // The window asks the platform for a size, as a drag on its
                // corner would; the platform answers with `Resized` (or at
                // once, where it can), and the step waits for the frame
                // after.
                Step::Resize(width, height) => {
                    let Some(window) = self.window.clone() else { break };
                    eprintln!("gui --window: script asks for {width}×{height} px");
                    if let Some(now) = window.request_inner_size(PhysicalSize::new(width, height)) {
                        self.resized(event_loop, now);
                    }
                    self.ask_redraw();
                    break;
                }
                // The same handler the platform's word runs, at the factor
                // given: then the window asks for the size the grid wants
                // at the new type, as the platform would set it.
                Step::Scale(factor) => {
                    let Some(window) = self.window.clone() else { break };
                    eprintln!("gui --window: script scales to {factor}");
                    if let Some(want) = self.rescale(event_loop, factor)
                        && let Some(now) = window.request_inner_size(want)
                    {
                        self.resized(event_loop, now);
                    }
                    self.ask_redraw();
                    break;
                }
                // Hidden as Cmd+M hides it; `about_to_wait` restores it on
                // the loop's clock, which turns whether or not redraws come.
                Step::Minimise(time) => {
                    let Some(window) = self.window.clone() else { break };
                    eprintln!("gui --window: script minimises for {} ms", time.as_millis());
                    window.set_minimized(true);
                    self.restore_at = Some(Instant::now() + time);
                    self.ask_redraw();
                    break;
                }
                // The next step after the next frame, which comes when the
                // last one's wait runs out: nothing is asked for here.
                Step::Frame => break,
                // No frame for that long: the one just drawn stays on
                // screen, for a screenshot of it. The next step runs on the
                // frame after.
                Step::Freeze(time) => {
                    eprintln!("gui --window: script freezes the frame for {} ms", time.as_millis());
                    self.frozen_until = Some(Instant::now() + time);
                    break;
                }
                Step::Quit => {
                    self.quit_at = self.stats.is_some().then(Instant::now);
                    event_loop.exit();
                    break;
                }
            }
        }
        if self.script.as_ref().is_some_and(Script::is_done) {
            eprintln!("gui --window: script done");
            self.script = None;
        }
    }

    /// A new surface size: to the backend, then a whole repaint, then the
    /// GUI's own resize bookkeeping through the input half.
    fn resized(&mut self, event_loop: &ActiveEventLoop, size: PhysicalSize<u32>) {
        // Before the backend there is no surface to size: the fact is
        // held, and the window's size then applied by the replay — kept,
        // since `install` keeps a size the window was given meanwhile.
        let Some(terminal) = self.terminal.as_mut() else {
            self.held.push(Held::Resized);
            return;
        };
        // A zero side (a window minimised on Windows) goes through as it
        // is: the backend keeps its last surface for it. Anything else
        // smaller than a cell becomes one cell, drawn scaled into the
        // sliver, rather than a grid of none.
        let side = |got: u32, least: u32| if got == 0 { 0 } else { got.max(least) };
        let (width, height) =
            (side(size.width, self.min_surface.width), side(size.height, self.min_surface.height));
        terminal.backend_mut().resize(width, height);
        let _ = terminal.clear();
        let Ok(grid) = terminal.backend_mut().size() else { return };
        self.feed(event_loop, TermEvent::Resize(grid.width, grid.height));
    }

    /// The surface is this size already. Not asked of a resize that comes
    /// with new type (`rescale`): the cells change there though the pixels
    /// may not.
    fn surface_is(&mut self, size: PhysicalSize<u32>) -> bool {
        let Some(terminal) = self.terminal.as_mut() else { return false };
        terminal.backend_mut().window_size().is_ok_and(|now| {
            (u32::from(now.pixels.width), u32::from(now.pixels.height))
                == (size.width, size.height)
        })
    }

    /// The display's scale changed (the window moved to another screen):
    /// the type at the new size, from the same faces, and the surface held
    /// to the same grid of cells, as a terminal keeps its rows and columns
    /// when it moves; then the GUI's resize bookkeeping, which re-measures
    /// the cell for covers. The size the grid wants is returned for the
    /// caller to ask the platform for — through winit's writer when winit
    /// asked, or as a request when the script did.
    fn rescale(&mut self, event_loop: &ActiveEventLoop, scale: f64) -> Option<PhysicalSize<u32>> {
        let px = font_px(scale);
        let mut fonts = Fonts::new(hack().ok()?, px);
        fonts.add_regular_fonts(self.faces.iter().cloned());
        let terminal = self.terminal.as_mut()?;
        let grid = terminal.backend_mut().size().ok()?;
        terminal.backend_mut().update_fonts(fonts);
        // The cell from the backend, as at open: the surface's width over
        // the columns it now has room for is the new cell's width exactly
        // while the spare pixels (fewer than a cell) are fewer than the
        // columns, which any surface the grid held already is.
        let reported = terminal.backend_mut().window_size().ok()?;
        let cols = u32::from(reported.columns_rows.width.max(1));
        let cell = (u32::from(reported.pixels.width) / cols, px);
        let want = fit(cell, (grid.width, grid.height));
        eprintln!(
            "gui --window: scale {scale}: {px} px type, cell {}×{} px; {}×{} cells at {}×{} px",
            cell.0, cell.1, grid.width, grid.height, want.width, want.height
        );
        self.min_surface = PhysicalSize::new(cell.0.max(1), cell.1.max(1));
        if let Some(window) = &self.window {
            window.set_min_inner_size(Some(self.min_surface));
            snap_to_cells(window, self.min_surface, scale);
        }
        // The surface to the new size now, not when the platform answers:
        // the backend's cells and its text texture are sized by the cell,
        // and a frame drawn between the new type and the new surface would
        // be a grid of another size on the old one.
        self.resized(event_loop, want);
        self.ime_area = None;
        // The pointer has not moved, but its physical pixel has: the same
        // place on the screen is that many more (or fewer) pixels in. The
        // grid is the same, so the cell under it is too.
        self.translator.rescale_pixel(scale / self.scale);
        self.scale = scale;
        Some(want)
    }

    /// The window's text, row by row, then what the pointer is doing: the
    /// cell it is over, whether it is the hand, and any composition.
    fn script_dump(&mut self, path: &Path) -> std::io::Result<()> {
        let grid = self.grid();
        let Some(terminal) = self.terminal.as_ref() else { return Ok(()) };
        let mut text = terminal.backend().get_text();
        let pointer = grid.map_or((0, 0), |grid| self.translator.pointer(grid));
        let surface = grid.map_or((0, 0), |grid| (grid.width, grid.height));
        // The covers this frame painted as pictures, as x,y w×h in cells,
        // and how many more it placed under something drawn over them.
        let (placed, under) = self.board.placed_rects();
        let covers = if placed.is_empty() {
            "none".to_string()
        } else {
            let rects: Vec<String> = placed
                .iter()
                .map(|r| format!("{},{} {}×{}", r.x, r.y, r.width, r.height))
                .collect();
            format!("{} at {}", placed.len(), rects.join(" "))
        };
        let covers = format!("{covers}, {under} under an overlay");
        let minimised = match self.window.as_ref().and_then(|window| window.is_minimized()) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        text.push_str(&format!(
            "-- pointer {},{} at {:.1},{:.1} px; surface {}×{} px; cursor {}; preedit {:?}; \
             held {:?}; ime {}; minimised {minimised}; covers {covers}; last press {}\n",
            pointer.0,
            pointer.1,
            self.translator.pixel().0,
            self.translator.pixel().1,
            surface.0,
            surface.1,
            if self.ctx.hand { "hand" } else { "default" },
            self.preedit,
            self.translator.held(),
            match (self.ime_allowed, self.ime_area) {
                (false, _) => "off".to_string(),
                (true, None) => "on".to_string(),
                (true, Some(((x, y), (w, h)))) => format!("on at {x},{y} px, {w}×{h}"),
            },
            self.last_press.as_deref().unwrap_or("none"),
        ));
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, text)?;
        eprintln!("gui --window: script dumped {}", path.display());
        Ok(())
    }

    /// The end of gui::run, minus the terminal's own steps: the window and
    /// its surface go first, the way the terminal is restored first, then
    /// the player's teardown. The close button and Cmd-Q arrive here
    /// without passing through a quit in `input`, so the saver is flushed
    /// here — unless a quit that did pass through flushed it already.
    fn teardown(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if let Some(stats) = &self.stats {
            let covers = self.terminal.as_ref().map(|t| t.backend().post_processor().report());
            stats.write(covers);
        }
        // With the stats lever, what each step of the way out took, from the
        // quit itself: a quit that takes seconds on one machine says where.
        let mut clock = self.quit_at.take();
        let mut lap = |laps: &mut Vec<(&'static str, Duration)>, stage: &'static str| {
            if let Some(at) = clock.as_mut() {
                let now = Instant::now();
                laps.push((stage, now - *at));
                *at = now;
            }
        };
        let mut laps = Vec::new();
        lap(&mut laps, "loop");
        // A build still running when the quit came is waited for, a while,
        // and what it made is dropped here, on the loop's thread, before
        // the window. Left to end on its own, its send would find no one
        // listening and drop what it built there: the surface, which holds
        // a clone of the window, and with the window gone from here by
        // then, the last one — AppKit's window released off the main
        // thread. Past the bound (a hung driver must not hold the way out
        // for long) the window is leaked instead: with this clone never
        // dropped, whatever the thread drops is never the last, and the
        // process's end reclaims it. That is simpler than handing the
        // window back across threads, and a quit is the only way here.
        if let Some(building) = self.building.take() {
            match drain(&building.done, BUILD_AT_QUIT) {
                Drained::Came(built) => drop(built),
                Drained::Ended => {}
                Drained::Running => {
                    eprintln!(
                        "gui --window: the renderer's build was still running {} ms after the \
                         quit; the window is left to the process's end",
                        BUILD_AT_QUIT.as_millis()
                    );
                    std::mem::forget(self.window.take());
                }
            }
        }
        self.terminal = None;
        lap(&mut laps, "gpu");
        self.window = None;
        lap(&mut laps, "window");
        if !self.quit_flushed {
            self.ctx.saver.flush(&self.gui.app);
        }
        finish(&mut self.gui, &self.ctx);
        lap(&mut laps, "finish");
        self.exit_laps = laps;
        self.exit_clock = clock;
        // The instance lock is not dropped here: `run` drops it after the
        // App, or `exiting` does when the platform ends the process.
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.lap.mark(&mut self.stats, "to_resumed");
        // The Dock icon of a bare binary, before its window: the earliest
        // point AppKit keeps it (icon.rs).
        #[cfg(target_os = "macos")]
        {
            icon::set_dock_icon();
            self.lap.mark(&mut self.stats, "dock_icon");
        }
        if let Err(e) = self.open(event_loop) {
            eprintln!("gui --window: {e}");
            self.exit_code = NO_WINDOW;
            event_loop.exit();
        }
    }

    // Esc does not close the window: the GUI's own keys give Esc to the
    // modals and rooms that back out with it, as the terminal does. The
    // close button, the app menu's Quit and the GUI's own quit keys close
    // it.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.quit_at = self.stats.is_some().then(Instant::now);
                event_loop.exit();
            }
            // The new size to the surface, then a whole repaint: ratatui-wgpu
            // marks rows clean even when a present fails, and a present
            // against a surface mid-resize can, so the next frame draws
            // every cell. The GUI's own resize bookkeeping runs through the
            // input half with the grid the surface now holds.
            // A size the surface has already — the one `install` sized the
            // window to, which the platform then reports back — moves no
            // cell, and the repaint and the GUI's bookkeeping are skipped:
            // on macOS that repaint was the frame the window came into view
            // with, a full one, 40 ms in a debug build here. A frame is still
            // asked for, which costs only the cells that changed: a Windows
            // restore from minimise lands here too — the minimise's
            // Resized(0, 0) left the surface at its old size (the backend
            // keeps it for a zero side), so the restore's size is the one
            // it has — and without a frame the window came back to nothing
            // until the next input or tick.
            WindowEvent::Resized(size) if self.surface_is(size) => self.ask_redraw(),
            WindowEvent::Resized(size) => self.resized(event_loop, size),
            // Before the backend, held: `build` read the scale it builds
            // for, and the replay applies the window's scale if it differs.
            WindowEvent::ScaleFactorChanged { .. } if self.terminal.is_none() => {
                self.held.push(Held::Scale)
            }
            // The scale the type is sized for already: macOS reports the
            // window's scale as it comes on screen though `build` read the
            // same one. The type, the surface and the grid all stand, and
            // the window keeps its size rather than take winit's suggestion;
            // rebuilding the type for it repainted every cell, a full frame
            // that was the one the window first showed the player with.
            WindowEvent::ScaleFactorChanged { scale_factor, mut inner_size_writer }
                if (scale_factor - self.scale).abs() <= f64::EPSILON =>
            {
                if let Some(window) = &self.window {
                    let _ = inner_size_writer.request_inner_size(window.inner_size());
                }
            }
            // winit's suggested size keeps the window's logical size, which
            // is the grid only to within a pixel or so of rounding; the
            // grid's own size goes back instead, and the `Resized` that
            // follows carries it to the surface.
            WindowEvent::ScaleFactorChanged { scale_factor, mut inner_size_writer } => {
                if let Some(want) = self.rescale(event_loop, scale_factor) {
                    let _ = inner_size_writer.request_inner_size(want);
                }
            }
            // A button let go out there may never come back as a release
            // (Windows and X11 report one only while the pointer is
            // captured; a window that lost the keyboard to another hears
            // nothing): every held button is let go where the pointer last
            // was, so a drag ends rather than following the pointer back.
            WindowEvent::CursorLeft { .. } | WindowEvent::Focused(false) => {
                self.feed_raw(event_loop, Raw::Leave)
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            // A synthetic key is one winit makes up for a key already held
            // when the window gained focus (X11, Windows): no terminal would
            // report it, and a held Enter must not open whatever is under
            // the cursor.
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                let raw = input::from_winit_key(&event, self.modifiers);
                self.feed_raw(event_loop, raw);
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                self.feed_raw(event_loop, Raw::ImePreedit(text))
            }
            WindowEvent::Ime(Ime::Commit(text)) => self.feed_raw(event_loop, Raw::ImeCommit(text)),
            // An input method switched off or away drops what it was
            // composing.
            WindowEvent::Ime(Ime::Enabled | Ime::Disabled) => {
                self.feed_raw(event_loop, Raw::ImePreedit(String::new()))
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.feed_raw(event_loop, Raw::Move { x: position.x, y: position.y })
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = input::from_winit_button(button) {
                    let down = state == ElementState::Pressed;
                    self.feed_raw(event_loop, Raw::Button { button, down });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.feed_raw(event_loop, Raw::Wheel(input::from_winit_wheel(delta)))
            }
            // A present that finds the window not on screen yet (wgpu's
            // `Occluded`, which the very first frame gets on macOS) is owed
            // by the vendored crate rather than lost (VENDORED.md, fix 5):
            // its next flush presents whatever changed. That flush is the
            // next frame, though, which a still screen may not ask for for
            // a whole poll, so the window coming into view asks for one now
            // — and owes the present whether or not the last one failed,
            // since one that went to a window the compositor never showed
            // failed nowhere (VENDORED.md, change 20). That puts the whole
            // screen up again from what the backend holds, a few
            // milliseconds, where repainting every cell cost a full frame
            // (about 40 ms in a debug build here) before the player was seen.
            WindowEvent::Occluded(false) => {
                if let Some(stats) = self.stats.as_mut() {
                    stats.visible();
                }
                self.present_again();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    /// The loop's clock: a redraw once the last frame's wait runs out, and
    /// sleep until then. A redraw just asked for keeps the loop turning so
    /// it arrives now — on macOS winit delivers asked-for redraws only when
    /// the loop wakes — unless the platform has held it back for longer
    /// than a poll, when the loop sleeps between checks rather than spin.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        // Until the backend exists the loop polls the early threads, begins
        // its build on the turn they are done, and installs it on the turn
        // the build's thread is; meanwhile it answers the window, and a
        // hidden one is shown by its deadline, blank, rather than kept off
        // screen for as long as the GPU takes.
        if self.terminal.is_none() {
            // Before the first frame: the window never showed the player.
            if let Err(e) = self.poll_build(Duration::ZERO) {
                eprintln!("gui --window: {e}");
                self.exit_code = NO_WINDOW;
                event_loop.exit();
                return;
            }
            if self.terminal.is_none() {
                // The script goes on meanwhile, as far as it can without a
                // frame; what it types is held like a person's.
                self.play(event_loop);
                if !self.shown && self.show_due() {
                    self.show(false);
                }
                event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + EARLY_POLL));
                return;
            }
        }
        // A hidden window gets no WM_PAINT, so a redraw asked of it never
        // comes back as `RedrawRequested`: until the window is shown the
        // frames are the loop's own, one a turn, and the first that presents
        // shows it (`redraw_inner`). The deadline's turn draws one too
        // before it shows the window anyway, so a backend that came late
        // still puts a frame in it.
        if !self.shown {
            self.redraw(event_loop);
            if !self.shown {
                if !self.show_due() {
                    event_loop.set_control_flow(ControlFlow::Poll);
                    return;
                }
                self.show(false);
            }
        }
        let now = Instant::now();
        // The script's `freeze`: the loop sleeps until it ends, asking for
        // nothing, and draws again after.
        if let Some(until) = self.frozen_until {
            if now < until {
                event_loop.set_control_flow(ControlFlow::WaitUntil(until));
                return;
            }
            self.frozen_until = None;
        }
        if let Some(at) = self.restore_at.filter(|&at| now >= at) {
            self.restore_at = None;
            if let Some(window) = &self.window {
                window.set_minimized(false);
                eprintln!(
                    "gui --window: script restores the window, {} ms late",
                    now.duration_since(at).as_millis()
                );
            }
        }
        if self.asked.is_none() && now >= self.next_frame {
            self.ask_redraw();
        }
        let flow = match self.asked {
            Some(at) if now.duration_since(at) < STALLED_REDRAW => ControlFlow::Poll,
            Some(_) => ControlFlow::WaitUntil(now + STALLED_REDRAW),
            None => ControlFlow::WaitUntil(self.next_frame),
        };
        let flow = match (flow, self.restore_at) {
            (ControlFlow::WaitUntil(wake), Some(restore)) => {
                ControlFlow::WaitUntil(wake.min(restore))
            }
            (flow, _) => flow,
        };
        event_loop.set_control_flow(flow);
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        self.teardown();
        // An exit the window asked for (a quit key, the close button, the
        // script) returns from the event loop, and `run` drops the lock
        // after the App. One it did not ask for is the platform's: a Cmd-Q
        // on macOS never returns, AppKit ending the process once this
        // returns, so the lock goes now or its sidecar stays behind.
        if !event_loop.exiting() {
            self.instance = None;
            exit_report(&self.exit_laps);
        }
    }
}

/// What the sixteen named ANSI colours draw as. A terminal answers a named
/// colour from its own scheme; the window has none, and ratatui-wgpu's
/// default table is the SVG keywords (Blue is #0000ff, Gray #808080), which
/// is no terminal's. The names the kit's ANSI tier gives a role (theme.rs:
/// LightBlue the accent, Cyan the bright, DarkGray the dim, Yellow the
/// gold, Green ok, Red danger, Black on-accent) take that role's colour from
/// the palette the window runs on, so a named colour in the visualizer (its
/// default theme is Cyan, DarkGray and Blue) or in the kit matches the
/// colours around it. The rest, and any role the palette gives no RGB, are
/// VS Code's integrated-terminal defaults for a dark theme
/// (`terminal.ansi*`): a standard terminal palette, legible on a dark
/// ground.
fn named_colours(theme: &crate::kit::theme::Theme) -> ColorTable {
    let role = |colour: Color, standard: [u8; 3]| match colour {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => standard,
    };
    ColorTable {
        BLACK: role(theme.on_accent, [0x00, 0x00, 0x00]),
        RED: role(theme.danger, [0xcd, 0x31, 0x31]),
        GREEN: role(theme.ok, [0x0d, 0xbc, 0x79]),
        YELLOW: role(theme.gold, [0xe5, 0xe5, 0x10]),
        BLUE: [0x24, 0x72, 0xc8],
        MAGENTA: [0xbc, 0x3f, 0xbc],
        CYAN: role(theme.bright, [0x11, 0xa8, 0xcd]),
        GRAY: [0xe5, 0xe5, 0xe5],
        DARKGRAY: role(theme.dim, [0x66, 0x66, 0x66]),
        LIGHTRED: [0xf1, 0x4c, 0x4c],
        LIGHTGREEN: [0x23, 0xd1, 0x8b],
        LIGHTYELLOW: [0xf5, 0xf5, 0x43],
        LIGHTBLUE: role(theme.accent, [0x3b, 0x8e, 0xea]),
        LIGHTMAGENTA: [0xd6, 0x70, 0xd6],
        LIGHTCYAN: [0x29, 0xb8, 0xdb],
        WHITE: [0xe5, 0xe5, 0xe5],
    }
}

/// A dragged resize in whole cells, as a terminal window's is: the
/// platform is asked to step the window's size by one cell, so a drag
/// never leaves a sliver of a column or row at the edge. Given in logical
/// units (the cell over the display's scale), which is how the platforms
/// that honour it keep it — macOS (the content view's resize increments)
/// and X11 (the WM_NORMAL_HINTS size increments, which the window manager
/// may or may not respect). Windows and Wayland ignore it, and a drag there
/// may leave a part-cell margin the backend draws as ground. Set at open
/// and again whenever the scale changes the cell.
fn snap_to_cells(window: &Window, cell: PhysicalSize<u32>, scale: f64) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let step = LogicalSize::new(f64::from(cell.width) / scale, f64::from(cell.height) / scale);
    window.set_resize_increments(Some(step));
}

/// The type size in pixels at a display scale: [`FONT_PT`] points, so a
/// Retina screen (scale 2) draws 32 px type where a standard one draws 16.
fn font_px(scale: f64) -> u32 {
    (FONT_PT * scale).round().max(1.0) as u32
}

/// The surface that holds a grid of cells exactly.
fn fit(cell: (u32, u32), (cols, rows): (u16, u16)) -> PhysicalSize<u32> {
    PhysicalSize::new(cell.0 * u32::from(cols), cell.1 * u32::from(rows))
}

/// The clipboard's text, for a paste; `None`, said on stderr, when there
/// is none or no clipboard to read. A clipboard per paste: the X11 one is
/// a connection of its own, and a paste is rare enough to open it then.
/// On a Wayland session this is XWayland's clipboard (Cargo.toml says why).
fn clipboard_text() -> Option<String> {
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
        Ok(text) => Some(text),
        Err(e) => {
            eprintln!("gui --window: nothing to paste ({e})");
            None
        }
    }
}

/// egui's monospace face, already in the binary for the visualizer's
/// controls: the window's terminal font.
fn hack() -> Result<Font<'static>, String> {
    Font::new(epaint_default_fonts::HACK_REGULAR)
        .ok_or_else(|| "the Hack face would not load".to_string())
}

/// What the GUI draws that Hack has no glyph for, CJK aside: the rating
/// stars and the checkbox tick. A terminal borrows these from its own font
/// fallback without being asked; the window has to be handed a face that
/// has them, or they draw as boxes. The census test below keeps the list
/// complete as the GUI grows new glyphs, and checks that [`SYMBOLS`] has
/// every one of them.
const BEYOND_HACK: &str = "★☆✓";

/// The window's own symbol face (assets/fonts/, made by
/// scripts/symbol-font.py on Hack's metrics, under the OFL): every glyph of
/// [`BEYOND_HACK`], drawn the same on every platform, where a borrowed
/// system face drew Menlo's stars on a Mac, DejaVu's on Linux, Segoe's on
/// Windows, and boxes on a system with none of them. 1.6 kB, and only in a
/// binary with the window in it.
const SYMBOLS: &[u8] = include_bytes!("../../../assets/fonts/mStreamSymbols-Regular.ttf");

/// The bundled symbol face, as the backend reads it.
fn symbols() -> Result<Font<'static>, String> {
    Font::new(SYMBOLS).ok_or_else(|| "the bundled symbol face would not load".to_string())
}

/// Where each platform keeps a face with more symbols than [`SYMBOLS`]
/// draws, best first: not for the GUI's own glyphs, which the bundled face
/// has, but for whatever a song's title or an artist's name carries (♥, ♪,
/// ☯). A monospace face leads where there is one, so those keep a
/// terminal's shapes: Menlo and DejaVu Sans Mono are Hack's own ancestors.
/// Every path is read at face 0, which in each of these is the regular one.
fn symbol_faces() -> Vec<PathBuf> {
    let paths: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/Menlo.ttc",
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ]
    } else if cfg!(windows) {
        &["Fonts/seguisym.ttf", "Fonts/DejaVuSansMono.ttf", "Fonts/arialuni.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
            "/usr/share/fonts/noto/NotoSansSymbols2-Regular.ttf",
        ]
    };
    paths.iter().map(|path| system_path(path)).collect()
}

/// A font path as the platform spells it: Windows' font folder is under
/// wherever Windows is, every other list is absolute already.
fn system_path(path: &str) -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join(path)
    } else {
        PathBuf::from(path)
    }
}

/// A system font file, mapped for the rest of the process. The backend
/// borrows its faces for as long as it lives, and it lives as long as the
/// process, so the map is leaked to say so. Mapped rather than read: the
/// CJK collections run to tens of megabytes (Apple SD Gothic Neo is 55),
/// and a read would put all of it on the heap where a map makes resident
/// only the pages a glyph lookup touches, and those are clean pages the
/// system can drop and fetch again.
fn map_font(path: &Path) -> Option<&'static [u8]> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: the map is read-only and the file is a system font, which
    // nothing rewrites while the player runs. Were one replaced in place,
    // the map would show the new bytes (or fault on a truncation): the
    // exposure every program accepts that maps its fonts, which through the
    // platform's own font stack is every program.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let map: &'static memmap2::Mmap = Box::leak(Box::new(map));
    Some(&map[..])
}

/// The glyphs of `wanted` that a face has no glyph for, read the way the
/// backend will read it (face 0); `None` when the bytes are not a face.
fn missing_from(bytes: &[u8], wanted: &str) -> Option<String> {
    use skrifa::MetadataProvider;
    let face = skrifa::FontRef::from_index(bytes, 0).ok()?;
    let charmap = face.charmap();
    Some(wanted.chars().filter(|&ch| charmap.map(ch).is_none()).collect())
}

/// The system face behind [`SYMBOLS`], for every language: the first on
/// the platform's list that has all of [`BEYOND_HACK`], or failing that the
/// one that has the most. The list is the measure of a fuller symbol face,
/// not a need: the bundled face draws those glyphs whichever this finds,
/// and none at all leaves only a title's rarer symbols as boxes. The maps
/// of the faces passed over are leaked too; they are a handful of small
/// files' address space, nothing resident past the character map read.
fn symbol_fallback() -> Option<Font<'static>> {
    let mut best: Option<(PathBuf, &'static [u8], String)> = None;
    for path in symbol_faces() {
        let Some(bytes) = map_font(&path) else { continue };
        let Some(missing) = missing_from(bytes, BEYOND_HACK) else { continue };
        let better = best
            .as_ref()
            .is_none_or(|(_, _, fewest)| missing.chars().count() < fewest.chars().count());
        if better {
            let done = missing.is_empty();
            best = Some((path, bytes, missing));
            if done {
                break;
            }
        }
    }
    let Some((path, bytes, _)) = best else {
        eprintln!("gui --window: no system symbol face; {BEYOND_HACK} from the bundled one");
        return None;
    };
    let font = Font::new(bytes)?;
    eprintln!("gui --window: symbols beyond the bundled face from {}", path.display());
    Some(font)
}

/// Where each platform keeps its colour emoji face. Apple Color Emoji is
/// sbix (PNG bitmaps per size), Noto Color Emoji is CBDT (PNG bitmaps) or,
/// in its newer builds, COLRv1 vector layers, and Segoe UI Emoji is COLR
/// (v0 layers, v1 on Windows 11): the backend draws all of them, bitmaps
/// through the `png` decoder its `png` feature brings and layers through
/// the font reader's painter. Face 0 throughout.
fn emoji_faces() -> Vec<PathBuf> {
    let paths: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/Apple Color Emoji.ttc"]
    } else if cfg!(windows) {
        &["Fonts/seguiemj.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto/NotoColorEmoji.ttf",
            "/usr/share/fonts/google-noto-color-emoji-fonts/NotoColorEmoji.ttf",
            "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf",
            "/usr/share/fonts/noto-emoji/NotoColorEmoji.ttf",
            "/usr/share/fonts/TTF/NotoColorEmoji.ttf",
            "/usr/share/fonts/truetype/noto-color-emoji/NotoColorEmoji.ttf",
        ]
    };
    paths.iter().map(|path| system_path(path)).collect()
}

/// Where fontconfig keeps Noto Color Emoji, on a Linux whose packages put
/// it somewhere none of [`emoji_faces`]'s paths name. Asked only when those
/// all miss, and only of an `fc-match` that is there; its answer is the
/// best match, which is some other face when the system has no emoji face
/// at all, so [`has_colour`] judges it before it is used.
fn fontconfig_emoji() -> Option<PathBuf> {
    if cfg!(any(target_os = "macos", windows)) {
        return None;
    }
    let out = std::process::Command::new("fc-match")
        .args(["--format=%{file}", "Noto Color Emoji:style=Regular"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
}

/// Whether a face draws in colour: it has bitmaps (sbix, CBDT) or layers
/// (COLR). A face with none of them is a text face, which fontconfig may
/// offer in an emoji face's place.
fn has_colour(bytes: &[u8]) -> bool {
    let Ok(face) = skrifa::FontRef::from_index(bytes, 0) else { return false };
    [b"sbix", b"CBDT", b"COLR"]
        .into_iter()
        .any(|tag| face.table_data(skrifa::Tag::new(tag)).is_some())
}

/// The colour emoji face, for every language, mapped as the CJK faces are
/// (a 190 MB collection on a Mac, of which only the glyphs drawn become
/// resident), with where it came from; `None` on a system without one,
/// where an emoji draws as Hack's box as it did before.
fn emoji_face() -> Option<(PathBuf, &'static [u8])> {
    emoji_faces()
        .into_iter()
        .chain(std::iter::from_fn({
            let mut asked = false;
            move || (!std::mem::replace(&mut asked, true)).then(fontconfig_emoji).flatten()
        }))
        .find_map(|path| {
            let bytes = map_font(&path)?;
            has_colour(bytes).then_some((path, bytes))
        })
}

/// [`emoji_face`] as the backend reads it, said on stderr like the others.
fn emoji_fallback() -> Option<Font<'static>> {
    let Some((path, bytes)) = emoji_face() else {
        eprintln!("gui --window: no colour emoji face; emoji will be boxes");
        return None;
    };
    let font = Font::new(bytes);
    match &font {
        Some(_) => eprintln!("gui --window: emoji from {}", path.display()),
        None => eprintln!("gui --window: {} is not a face the backend can read", path.display()),
    }
    font
}

/// The scripts Hack has nothing for, each with the system faces that draw
/// it, best first: a file and the family to find in it. A family rather
/// than a face index, because collections differ in order between
/// platforms and versions (Noto CJK's is JP 0, KR 1, SC 2, TC 3 in the
/// Debian and Arch packages; Yu Gothic UI shares YuGothR.ttc with Yu
/// Gothic): the face is found by its name and opened at its index.
fn script_faces() -> [(&'static str, Vec<(PathBuf, &'static str)>); 3] {
    let list = |faces: &[(&str, &'static str)]| -> Vec<(PathBuf, &'static str)> {
        faces.iter().map(|&(path, family)| (system_path(path), family)).collect()
    };
    if cfg!(target_os = "macos") {
        // Arial Unicode last for all three: every Mac has it in
        // Supplemental, and it has kana, hanzi and hangul alike.
        let arial = ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", "Arial Unicode MS");
        [
            (
                "ja",
                list(&[("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", "Hiragino Sans"), arial]),
            ),
            (
                "zh",
                list(&[
                    ("/System/Library/Fonts/Hiragino Sans GB.ttc", "Hiragino Sans GB"),
                    ("/System/Library/Fonts/STHeiti Light.ttc", "Heiti SC"),
                    arial,
                ]),
            ),
            (
                "ko",
                list(&[
                    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", "Apple SD Gothic Neo"),
                    ("/System/Library/Fonts/Supplemental/AppleGothic.ttf", "AppleGothic"),
                    arial,
                ]),
            ),
        ]
    } else if cfg!(windows) {
        [
            (
                "ja",
                list(&[
                    ("Fonts/YuGothR.ttc", "Yu Gothic UI"),
                    ("Fonts/meiryo.ttc", "Meiryo UI"),
                    ("Fonts/msgothic.ttc", "MS Gothic"),
                ]),
            ),
            (
                "zh",
                list(&[("Fonts/msyh.ttc", "Microsoft YaHei UI"), ("Fonts/simsun.ttc", "SimSun")]),
            ),
            ("ko", list(&[("Fonts/malgun.ttf", "Malgun Gothic"), ("Fonts/gulim.ttc", "Gulim")])),
        ]
    } else {
        let noto = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-sans-cjk-fonts/NotoSansCJK-Regular.ttc",
        ];
        // WenQuanYi and Droid's fallback carry hangul as well as hanzi and
        // kana, so they stand behind every script.
        let others = [
            ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", "WenQuanYi Micro Hei"),
            ("/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc", "WenQuanYi Micro Hei"),
            ("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf", "Droid Sans Fallback"),
        ];
        let with = |family: &'static str, extra: &[(&'static str, &'static str)]| {
            let faces: Vec<(&str, &'static str)> = noto
                .iter()
                .map(|&path| (path, family))
                .chain(extra.iter().copied())
                .chain(others)
                .collect();
            list(&faces)
        };
        [
            ("ja", with("Noto Sans CJK JP", &[])),
            ("zh", with("Noto Sans CJK SC", &[])),
            (
                "ko",
                with(
                    "Noto Sans CJK KR",
                    &[("/usr/share/fonts/truetype/nanum/NanumGothic.ttf", "NanumGothic")],
                ),
            ),
        ]
    }
}

/// The face of `family` in a font file, by index: one whose family name
/// (the legacy or the typographic one) is `family`, the regular one when
/// the family has several weights, else the first.
fn face_of(bytes: &[u8], family: &str) -> Option<u32> {
    use skrifa::MetadataProvider;
    use skrifa::string::StringId;
    let named = |face: &skrifa::FontRef, id: StringId, name: &str| {
        face.localized_strings(id).any(|s| s.chars().eq(name.chars()))
    };
    let file = skrifa::raw::FileRef::new(bytes).ok()?;
    let mut first = None;
    for (index, face) in file.fonts().enumerate() {
        let Ok(face) = face else { continue };
        if !named(&face, StringId::FAMILY_NAME, family)
            && !named(&face, StringId::TYPOGRAPHIC_FAMILY_NAME, family)
        {
            continue;
        }
        let index = index as u32;
        if named(&face, StringId::SUBFAMILY_NAME, "Regular") {
            return Some(index);
        }
        first.get_or_insert(index);
    }
    first
}

/// The faces for kana, hanzi and hangul, for every language: a title or an
/// artist's name is in whatever script it was tagged in, not the one the
/// player speaks, so an English player draws a Korean album too. The
/// locale's own script leads, so the characters the scripts share (the
/// Han ideographs) take its forms; then Japanese, Chinese, Korean. One
/// face per script, the first of its list that the system has. The maps
/// cost address space, not memory: nothing of a face is resident until a
/// glyph from it is drawn.
fn script_fallbacks(lang: &str) -> Vec<Font<'static>> {
    let mut scripts = script_faces();
    scripts.sort_by_key(|(script, _)| *script != lang);
    let mut opened: Vec<(PathBuf, u32)> = Vec::new();
    let mut fonts = Vec::new();
    for (script, faces) in scripts {
        let found = faces.into_iter().find_map(|(path, family)| {
            let bytes = map_font(&path)?;
            let index = face_of(bytes, family)?;
            Some((path, index, bytes))
        });
        let Some((path, index, bytes)) = found else {
            eprintln!("gui --window: no system face for {script}; its glyphs will be boxes");
            continue;
        };
        // Arial Unicode (or WenQuanYi) standing in for two scripts is one
        // face: the second copy would only be searched after the first.
        if opened.contains(&(path.clone(), index)) {
            eprintln!("gui --window: {script} glyphs from {} (already open)", path.display());
            continue;
        }
        match Font::new_at(bytes, index) {
            Some(font) => {
                eprintln!("gui --window: {script} glyphs from {} face {index}", path.display());
                opened.push((path, index));
                fonts.push(font);
            }
            None => eprintln!(
                "gui --window: {} face {index} is not a face the backend can read",
                path.display()
            ),
        }
    }
    fonts
}

/// The fidelity check: what the window holds, as text, beside the same Gui
/// drawn into a `TestBackend` of the same size, both written to `dir` and
/// compared row by row with trailing blanks trimmed.
fn dump(dir: &Path, terminal: &WindowTerminal, gui: &mut Gui) -> Result<(), String> {
    let lang = rust_i18n::locale().to_string();
    let window_text = terminal.backend().get_text();
    let window_rows: Vec<&str> = window_text.lines().collect();

    let size = terminal.backend().size().map_err(|e| e.to_string())?;
    let mut test =
        Terminal::new(TestBackend::new(size.width, size.height)).map_err(|e| e.to_string())?;
    test.draw(|frame| render(frame, gui)).map_err(|e| e.to_string())?;
    let test_rows = shown_rows(test.backend().buffer());

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("window-text-{lang}.txt")), &window_text)
        .map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("test-rows-{lang}.txt")), test_rows.join("\n") + "\n")
        .map_err(|e| e.to_string())?;

    let verdict = compare(&window_rows, &test_rows);
    eprintln!("gui --window: {}×{} cells, {lang}: {verdict}", size.width, size.height);
    let line = format!("{}×{} {verdict}\n", size.width, size.height);
    std::fs::write(dir.join(format!("compare-{lang}.txt")), line).map_err(|e| e.to_string())
}

/// A buffer's rows as a screen shows them: the cell after a wide glyph is
/// the glyph's second half, not a character of its own, so it is skipped —
/// which is also how the window's `get_text` spells it (an empty cell).
fn shown_rows(buffer: &Buffer) -> Vec<String> {
    let area = *buffer.area();
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut covered = 0;
            for x in 0..area.width {
                if covered > 0 {
                    covered -= 1;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                covered = crate::kit::grapheme_cells(symbol).saturating_sub(1);
            }
            row
        })
        .collect()
}

fn compare(window: &[&str], test: &[String]) -> String {
    if window.len() != test.len() {
        return format!("UNEQUAL: {} window rows, {} test rows", window.len(), test.len());
    }
    match window.iter().zip(test).position(|(w, t)| w.trim_end() != t.trim_end()) {
        None => "EQUAL".to_string(),
        Some(y) => format!(
            "UNEQUAL from row {y}:\n  window: {:?}\n  test:   {:?}",
            window[y].trim_end(),
            test[y].trim_end()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The way out's wait on the build's thread: what it sends in time
    /// comes back to be dropped by the caller, a thread that ended without
    /// sending is not waited on, and one still running is given up on at
    /// the bound, not after.
    #[test]
    fn the_quit_waits_for_the_build_a_bounded_while() {
        let (sender, done) = mpsc::channel();
        let late = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            let _ = sender.send(7);
        });
        assert_eq!(drain(&done, Duration::from_secs(5)), Drained::Came(7));
        late.join().unwrap();

        let (sender, done) = mpsc::channel::<u8>();
        drop(sender);
        let started = Instant::now();
        assert_eq!(drain(&done, Duration::from_secs(5)), Drained::Ended);
        assert!(started.elapsed() < Duration::from_secs(1), "a dead thread is waited on");

        let (sender, done) = mpsc::channel::<u8>();
        let started = Instant::now();
        assert_eq!(drain(&done, Duration::from_millis(50)), Drained::Running);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(50), "gave up early, {waited:?}");
        assert!(waited < Duration::from_secs(2), "the bound did not hold, {waited:?}");
        drop(sender);
    }

    /// Every glyph the GUI can put on screen that Hack cannot draw, other
    /// than the wide CJK the borrowed CJK face is for, must be in
    /// [`BEYOND_HACK`], or the window would draw it as a box where a
    /// terminal would not. The census reads what the GUI can draw from
    /// its sources — the string and char literals of the GUI and the kit,
    /// with whole-line comments dropped — and from every locale's strings.
    #[test]
    fn every_glyph_hack_lacks_has_a_fallback() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let literal = regex::Regex::new(r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)'"#).unwrap();
        let mut drawn = String::new();
        let mut dirs = vec![root.join("src/gui"), root.join("src/kit")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let source = std::fs::read_to_string(&path).unwrap();
                    let code: String = source
                        .lines()
                        .filter(|line| !line.trim_start().starts_with("//"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    literal.find_iter(&code).for_each(|m| drawn.push_str(m.as_str()));
                }
            }
        }
        for entry in std::fs::read_dir(root.join("locales")).unwrap().map(Result::unwrap) {
            drawn.push_str(&std::fs::read_to_string(entry.path()).unwrap());
        }
        let narrow: String = {
            let mut chars: Vec<char> = drawn
                .chars()
                .filter(|c| !c.is_ascii() && !c.is_whitespace())
                .filter(|&c| unicode_width::UnicodeWidthChar::width(c) == Some(1))
                .collect();
            chars.sort_unstable();
            chars.dedup();
            chars.into_iter().collect()
        };
        let lacking = missing_from(epaint_default_fonts::HACK_REGULAR, &narrow).unwrap();
        let uncovered: String = lacking.chars().filter(|c| !BEYOND_HACK.contains(*c)).collect();
        assert!(
            uncovered.is_empty(),
            "Hack has no {uncovered}, and the window has no fallback for it"
        );
        // And the other way: a glyph Hack has needs no fallback, so the
        // list stays the short one it claims to be.
        let needless: String = BEYOND_HACK.chars().filter(|c| !lacking.contains(*c)).collect();
        assert!(
            needless.is_empty(),
            "{needless} is in BEYOND_HACK but Hack has it, or the GUI no longer draws it"
        );
        // And the window's own face has every one of them, so none of it
        // waits on what the system has.
        let unbundled = missing_from(SYMBOLS, BEYOND_HACK).expect("the bundled face reads");
        assert!(
            unbundled.is_empty(),
            "the bundled symbol face has no {unbundled}: add it to scripts/symbol-font.py"
        );
    }

    /// The bundled symbol face is a face both readers take — the backend's
    /// (rustybuzz, through `Font::new`) and skrifa — on Hack's metrics, so
    /// it sits on Hack's grid at Hack's size (the backend scales a face on
    /// Hack's em, line and ascender exactly as it scales Hack), and it is
    /// a text face, not a colour one.
    #[test]
    fn the_bundled_symbol_face_is_on_hacks_metrics() {
        use skrifa::MetadataProvider;
        use skrifa::instance::{LocationRef, Size};
        let metrics = |bytes| {
            let face = skrifa::FontRef::new(bytes).unwrap();
            let m = face.metrics(Size::unscaled(), LocationRef::default());
            (m.units_per_em, m.ascent, m.descent)
        };
        assert_eq!(metrics(SYMBOLS), metrics(epaint_default_fonts::HACK_REGULAR));
        assert!(symbols().is_ok());
        assert!(!has_colour(SYMBOLS) && !has_colour(epaint_default_fonts::HACK_REGULAR));
        // Every glyph advances one of Hack's cells.
        let face = skrifa::FontRef::new(SYMBOLS).unwrap();
        let hack = skrifa::FontRef::new(epaint_default_fonts::HACK_REGULAR).unwrap();
        let advance = |face: &skrifa::FontRef, c| {
            let glyph = face.charmap().map(c).unwrap_or_default();
            face.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(glyph)
        };
        for c in BEYOND_HACK.chars() {
            assert_eq!(advance(&face, c), advance(&hack, 'm'), "{c}");
        }
    }

    /// On a Mac the emoji face is Apple Color Emoji, found and judged a
    /// colour face; a text face found in its place would not be.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_finds_its_colour_emoji_face() {
        let (path, bytes) = emoji_face().expect("every Mac has Apple Color Emoji");
        assert!(path.ends_with("Apple Color Emoji.ttc"), "{}", path.display());
        assert!(has_colour(bytes));
        let menlo = map_font(Path::new("/System/Library/Fonts/Menlo.ttc")).unwrap();
        assert!(!has_colour(menlo));
    }

    /// The probe finds what the loader can open and not what it cannot:
    /// glibc's libc itself is always there; a name no system has is not.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn the_loader_probe_tells_a_library_from_none() {
        assert!(loads(c"libc.so.6"), "libc is loaded already");
        assert!(!loads(c"libmstream-no-such-library.so.0"));
    }

    /// After a panic, the log's path is said only for a file that is
    /// there: none when no log is written, none for one since removed.
    #[test]
    fn a_panic_names_the_log_only_when_there_is_one() {
        assert_eq!(panic_log_note(None), None);
        let dir = std::env::temp_dir().join(format!("mstream-panic-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("player.log");
        assert_eq!(panic_log_note(Some(&log)), None, "a path with no file");
        std::fs::write(&log, "a line\n").unwrap();
        let note = panic_log_note(Some(&log)).unwrap();
        assert!(note.ends_with(&log.display().to_string()), "{note}");
        assert_eq!(panic_log_note(Some(&dir)), None, "a directory is not a log");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `cargo test the_window_panic_hook -- --ignored` — swaps the
    /// process-global panic hook, so it must run alone. A caught thread's
    /// panic (the audio thread's) reaches nothing; another thread's reaches
    /// the hook before it, and the hook writes no escapes of its own.
    #[test]
    #[ignore = "swaps the process-global panic hook; run alone"]
    fn the_window_panic_hook_stands_back_for_caught_panics_only() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let base_ran = Arc::new(AtomicUsize::new(0));
        let counting = base_ran.clone();
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |_| {
            counting.fetch_add(1, Ordering::SeqCst);
        }));
        install_panic_hook();
        let _ = std::thread::Builder::new()
            .name("smoke-ordinary".into())
            .spawn(|| panic!("ordinary"))
            .unwrap()
            .join();
        assert_eq!(base_ran.load(Ordering::SeqCst), 1, "chained through to the previous hook");
        let _ = std::thread::Builder::new()
            .name(crate::tui::worker::AUDIO_THREAD.into())
            .spawn(|| panic!("caught elsewhere"))
            .unwrap()
            .join();
        assert_eq!(base_ran.load(Ordering::SeqCst), 1, "stood back for the audio thread");
        let _ = std::panic::take_hook();
        std::panic::set_hook(original);
    }

    /// The named colours a role owns come from the palette, the others from
    /// the standard table, and none is the SVG keyword the crate defaults to.
    #[test]
    fn named_colours_take_the_palettes_roles() {
        let rgb = |r, g, b| Color::Rgb(r, g, b);
        let theme = crate::kit::theme::Theme {
            accent: rgb(1, 0, 0),
            bright: rgb(2, 0, 0),
            dim: rgb(3, 0, 0),
            gold: rgb(4, 0, 0),
            ok: rgb(5, 0, 0),
            danger: rgb(6, 0, 0),
            text: rgb(7, 0, 0),
            ground: None,
            ground_rgb: None,
            on_accent: Color::Black,
        };
        let table = named_colours(&theme);
        assert_eq!(
            [table.LIGHTBLUE, table.CYAN, table.DARKGRAY, table.YELLOW, table.GREEN, table.RED],
            [[1, 0, 0], [2, 0, 0], [3, 0, 0], [4, 0, 0], [5, 0, 0], [6, 0, 0]]
        );
        // A role the palette names rather than spells takes the standard.
        assert_eq!(table.BLACK, [0, 0, 0]);
        assert_eq!(table.BLUE, [0x24, 0x72, 0xc8], "not the SVG #0000ff");
    }

    /// The type follows the display's scale, and the surface holds the
    /// grid whatever the cell: the window's own numbers at scales 1 and 2,
    /// and a fractional screen's.
    #[test]
    fn a_scale_sizes_the_type_and_the_grid_the_surface() {
        assert_eq!((font_px(1.0), font_px(2.0), font_px(1.5), font_px(1.25)), (16, 32, 24, 20));
        assert_eq!(font_px(0.01), 1, "never no type at all");
        assert_eq!(fit((8, 16), GRID), PhysicalSize::new(800, 480));
        assert_eq!(fit((16, 32), GRID), PhysicalSize::new(1600, 960));
        assert_eq!(fit((12, 24), (70, 20)), PhysicalSize::new(840, 480));
    }

    /// On a Mac the fallback is always there: Menlo ships with the system
    /// and has every glyph Hack lacks.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_finds_every_symbol_in_menlo() {
        let first = symbol_faces().into_iter().next().unwrap();
        let bytes = std::fs::read(&first).unwrap();
        assert_eq!(missing_from(&bytes, BEYOND_HACK).as_deref(), Some(""), "{}", first.display());
        assert!(symbol_fallback().is_some());
    }

    /// On a Mac every script has its face, found by name at the index the
    /// collection keeps it: Hiragino Sans and Hiragino Sans GB lead their
    /// files, Apple SD Gothic Neo's regular is the first of eighteen.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_finds_a_face_for_every_script() {
        for (script, faces) in script_faces() {
            let (path, family) = &faces[0];
            let bytes = map_font(path).unwrap();
            assert_eq!(face_of(bytes, family), Some(0), "{script}: {}", path.display());
        }
        let menlo = map_font(Path::new("/System/Library/Fonts/Menlo.ttc")).unwrap();
        assert_eq!(face_of(menlo, "Hiragino Sans"), None, "a family the file lacks");
        // One face each, in the locale's order: three for any language.
        assert_eq!(script_fallbacks("en").len(), 3);
        assert_eq!(script_fallbacks("ko").len(), 3);
    }

    /// The no-window code is one code, apart from the others a launcher
    /// can see: 0 a clean quit, 1 any other failure, 2 clap's usage error
    /// (101 a panic). A launcher's fallback keys on it, so it is pinned.
    #[test]
    fn a_window_that_cannot_open_has_its_own_exit_code() {
        assert_eq!(NO_WINDOW, 3);
        assert!(![0, 1, 2, 101].contains(&NO_WINDOW));
    }

    /// The X11 keyboard probe: a line only on an X11 session whose loader
    /// finds neither library name, and the line names both packages.
    #[test]
    fn the_x11_keyboard_probe_speaks_only_for_x11_without_the_library() {
        let session = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        let none = |_: &std::ffi::CStr| false;
        let line = x11_keyboard_missing_with(session(&["DISPLAY"]), none).expect("X11, no library");
        assert!(line.contains("libxkbcommon-x11-0") && line.contains("libxkbcommon-x11 (Fedora)"));
        // Either name loading is enough; the runtime package's first.
        let all = |_: &std::ffi::CStr| true;
        assert_eq!(x11_keyboard_missing_with(session(&["DISPLAY"]), all), None);
        let dev_only = |name: &std::ffi::CStr| name == c"libxkbcommon-x11.so";
        assert_eq!(x11_keyboard_missing_with(session(&["DISPLAY"]), dev_only), None);
        // Wayland (XWayland's DISPLAY beside it or not) and no display at
        // all are not this probe's to answer.
        assert_eq!(x11_keyboard_missing_with(session(&["DISPLAY", "WAYLAND_DISPLAY"]), none), None);
        assert_eq!(x11_keyboard_missing_with(session(&["WAYLAND_SOCKET"]), none), None);
        assert_eq!(x11_keyboard_missing_with(session(&[]), none), None);
    }
}
