//! `mstream-player gui --window`: the GUI in a native window of its own —
//! the window-mode spike.
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
//! clipboard into the field; a button let go outside the window lets go;
//! a move to a screen of another scale re-sizes the type; and a drag on the
//! window's edge steps by whole cells, where the platform allows it.
//!
//! What the terminal's main does on the way out, the window does itself:
//! the launcher's instance lock is dropped after the App, or in `exiting`
//! when a Cmd-Q on macOS ends the process without returning from the event
//! loop. A panic is printed without the terminal hook's escapes, with
//! where the log is.

mod covers;
mod input;
#[cfg(test)]
mod render_tests;
mod script;
mod stats;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::style::Color;
use ratatui_wgpu::{Builder, ColorTable, Dimensions, Font, Fonts, WgpuBackend};
use unicode_width::UnicodeWidthStr;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowId};

use covers::{Board, CoverPost};
use input::{Grid, Raw, Translator};
use script::{Input, Script, Step};
use stats::{Counted, Lap, Stats};

use super::{Channels, Ctx, Flow, Gui, Host, finish, frame, input, render};
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
/// The frame the fidelity dump waits for: the first has the window at its
/// opening size, the resize to the grid lands a frame or two later.
const DUMP_AT_FRAME: u32 = 5;

type WindowTerminal = Terminal<Counted<WgpuBackend<'static, 'static, CoverPost>>>;

/// The player in a window, from a Gui and workers that `gui::start` has
/// already brought up; the exit code is the terminal's (0, or 1 when a
/// frame failed), or 1 when there is no window to open. `instance` is the
/// launcher's instance lock, if this run holds one: dropped last, after the
/// App, so its sidecar goes on every way out — the Cmd-Q that ends the
/// process inside AppKit included, where `exiting` drops it.
pub(super) fn run(mut gui: Gui, channels: Channels, instance: Option<Instance>) -> i32 {
    // First, so the stats' clock (when the lever is set) starts at entry.
    let mut stats = Stats::from_env();
    let mut lap = Lap::start(&stats);
    install_panic_hook();
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
            return 1;
        }
    };
    lap.mark(&mut stats, "event_loop");
    let early = Early { faces, gpu: Early::gpu(event_loop.owned_display_handle()) };
    let dump = std::env::var_os("MSTREAM_WINDOW_DUMP").map(PathBuf::from);
    // Covers are the window's to draw: every Graphics the GUI forks for a
    // slot is forked from this one, so they all record onto the board.
    let board = Arc::new(Board::default());
    gui.app.graphics = crate::tui::graphics::Graphics::hosted(board.clone());
    let ctx = Ctx::new(&gui.app, channels);
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
        stats,
        instance,
        restore_at: None,
        last_press: None,
        lap,
        early,
        quit_at: None,
        exit_laps: Vec::new(),
        exit_clock: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("gui --window: {e}");
        app.exit_code = 1;
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

/// What winit's X11 keyboard needs and loads only at run time, through
/// xkbcommon-dl, by the two names it tries; it panics (exit 101, a
/// backtrace hint and no word of what to install) before any window opens
/// when neither loads. The `.so.0` is the runtime package's file, the bare
/// name the -dev package's link.
#[cfg(target_os = "linux")]
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
    if set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET") || !set("DISPLAY") {
        return None;
    }
    if XKB_X11.iter().any(|name| loads(name)) {
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
    /// The startup's work begun before the loop, until `open` joins it.
    early: Early,
    /// When the quit was decided, with the stats lever: the way out's
    /// clock, which the teardown laps (`exit_laps`) and leaves running.
    quit_at: Option<Instant>,
    exit_laps: Vec<(&'static str, Duration)>,
    exit_clock: Option<Instant>,
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
/// and device. The window is on screen from its creation in `resumed`,
/// blank until the first present, and the loop answers nothing meanwhile
/// (on Windows, the white window the Windows report saw for two to three
/// seconds, which DWM marks Not Responding under load); this work then
/// overlaps the event loop's start and the window's creation instead of
/// following them, and `open` only joins it.
struct Early {
    faces: Option<JoinHandle<Result<Faces, String>>>,
    gpu: Option<JoinHandle<Gpu>>,
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
            .spawn(move || Gpu::prepare(display))
            .ok()
    }
}

/// The regular faces the window draws with, and what finding them took.
struct Faces {
    /// Hack first for everything it has, then a system face for the few
    /// symbols it lacks, then the borrowed faces for kana, hanzi and
    /// hangul, whatever the language. `with_regular_fonts` keeps that
    /// order, where `with_fonts` would sort by width and could put a
    /// borrowed face's own Latin in front of Hack's. Hack is also the
    /// builder's last resort, which is what bold and italic cells fall back
    /// to with faked styles.
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
    let mut fonts = vec![hack()?];
    lap("faces.hack");
    fonts.extend(symbol_fallback());
    lap("faces.symbols");
    fonts.extend(script_fallbacks(lang));
    lap("faces.scripts");
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
    fn prepare(display: OwnedDisplayHandle) -> Gpu {
        let picked = Picked::pick(Box::new(display));
        let started = Instant::now();
        let device = picked.adapter.and_then(|adapter| {
            let descriptor =
                wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() };
            let (device, queue) = block_on(adapter.request_device(&descriptor)).ok()?.ok()?;
            Some((adapter, device, queue))
        });
        let mut took = picked.took;
        took.push(("gpu.device", started.elapsed()));
        Gpu { instance: picked.instance, device, took }
    }

    /// The instance alone, the builder to find the rest.
    fn without_device(display: Box<dyn wgpu::wgt::WgpuHasDisplayHandle>) -> Gpu {
        Gpu { instance: Picked::pick(display).instance, device: None, took: Vec::new() }
    }
}

/// An instance, the adapter it answered with if any, and what the two took
/// (`gpu.instance`, `gpu.adapter`), from [`Picked::pick`].
struct Picked {
    instance: wgpu::Instance,
    adapter: Option<wgpu::Adapter>,
    took: Vec<(&'static str, Duration)>,
}

impl Picked {
    /// The instance the window draws with, and its adapter. wgpu brings up
    /// every backend in the instance's mask when the instance is made, and
    /// `request_adapter` with no preference answers the first adapter it
    /// finds, so the choice is made with the mask. On Windows that is DX12
    /// alone first, and Vulkan alone only when DX12 has no adapter: never
    /// GL, and never the two together. The GL backend costs nothing to draw
    /// with and much to have in the process: its hidden window puts a hook
    /// on the window procedures (opengl32 in the stack of PR #41's Windows
    /// retest), and with it a keyboard-layout change request posted to the
    /// player's window — what the taskbar's language indicator posts —
    /// never returned from `DefWindowProc`, on Windows 10, every time;
    /// with DX12 alone or Vulkan alone the same request switches the
    /// layout and the window answers. DX12 before Vulkan because of the
    /// stats lever: on a hybrid box the Vulkan instance alone took half a
    /// second against DX12's 50 ms, and a mask holding both would have
    /// paid for Vulkan and been answered by it. `WGPU_BACKEND` still
    /// overrides the mask, through wgpu's own reading of it; Linux and
    /// macOS keep the env descriptor as before, where the display handle
    /// it carries is what Wayland and X11 need to make a surface later.
    fn pick(display: Box<dyn wgpu::wgt::WgpuHasDisplayHandle>) -> Picked {
        let options = wgpu::RequestAdapterOptions::default();
        if cfg!(windows) && wgpu::Backends::from_env().is_none() {
            let mut took = vec![("gpu.instance", Duration::ZERO), ("gpu.adapter", Duration::ZERO)];
            let mut picked = None;
            for backends in [wgpu::Backends::DX12, wgpu::Backends::VULKAN] {
                // `with_env` keeps the mask given here, since `WGPU_BACKEND`
                // is unset on this path, and still reads the rest of wgpu's
                // environment (validation, backend options).
                let descriptor = wgpu::InstanceDescriptor {
                    backends,
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                }
                .with_env();
                let started = Instant::now();
                let instance = wgpu::Instance::new(descriptor);
                took[0].1 += started.elapsed();
                let started = Instant::now();
                let adapter =
                    block_on(instance.request_adapter(&options)).ok().and_then(Result::ok);
                took[1].1 += started.elapsed();
                let found = adapter.is_some();
                picked = Some((instance, adapter));
                if found {
                    break;
                }
            }
            let (instance, adapter) = picked.expect("two backends were tried");
            return Picked { instance, adapter, took };
        }
        let started = Instant::now();
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(display),
        );
        let instance_took = started.elapsed();
        let started = Instant::now();
        let adapter = block_on(instance.request_adapter(&options)).ok().and_then(Result::ok);
        let took = vec![("gpu.instance", instance_took), ("gpu.adapter", started.elapsed())];
        Picked { instance, adapter, took }
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
    /// The window, and the backend that draws cells onto it.
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let (cols, rows) = self.grid;
        let opening = LogicalSize::new(f64::from(cols) * CELL_GUESS_PT, f64::from(rows) * FONT_PT);
        let attributes = Window::default_attributes().with_title(TITLE).with_inner_size(opening);
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| format!("the window would not open: {e}"))?,
        );
        self.lap.mark(&mut self.stats, "window");
        self.scale = window.scale_factor();
        let font_px = font_px(self.scale);
        // What `run` began before the loop, joined: the faces, and the GPU's
        // instance, adapter and device. Whatever did not come (a thread that
        // could not start, or panicked) is done here instead, as it all was
        // before; the window waits the same either way.
        let faces = self.early.faces.take().and_then(|thread| thread.join().ok());
        let faces = match faces {
            Some(found) => found?,
            None => find_faces(&rust_i18n::locale())?,
        };
        self.lap.mark(&mut self.stats, "faces.wait");
        let gpu = self.early.gpu.take().and_then(|thread| thread.join().ok());
        // The display handle goes in with the instance, as the visualizer's
        // window does it: on Wayland and X11 the backend needs it before a
        // surface can exist. The event loop's handle (what `run` gave the
        // early thread) is the same display as the window's.
        let gpu = gpu.unwrap_or_else(|| Gpu::without_device(Box::new(window.clone())));
        self.lap.mark(&mut self.stats, "gpu.wait");
        // Which adapter, through which backend: the one line a report needs
        // (otherwise it is only in wgpu's own log, at debug level).
        if let Some((adapter, _, _)) = &gpu.device {
            let info = adapter.get_info();
            eprintln!("gui --window: drawing with {} through {:?}", info.name, info.backend);
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
        let backend = block_on(builder.build_with_target(window.clone()))?
            .map_err(|e| format!("the window has nothing to draw with: {e}"))?;
        self.lap.mark(&mut self.stats, "backend");
        if let Some(stats) = self.stats.as_mut() {
            for (stage, took) in backend.build_timings() {
                stats.stage(&format!("backend.{stage}"), *took);
            }
        }
        let timed = self.stats.is_some();
        let mut terminal =
            Terminal::new(Counted::new(backend, timed)).map_err(|e| e.to_string())?;

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
        let reported = terminal.backend_mut().window_size().map_err(|e| e.to_string())?;
        let (got_cols, got_rows) =
            (reported.columns_rows.width.max(1), reported.columns_rows.height.max(1));
        let cell = (u32::from(reported.pixels.width) / u32::from(got_cols), font_px);
        let want = fit(cell, (cols, rows));
        eprintln!(
            "gui --window: {font_px} px type at scale {}, cell {}×{} px, opened at \
             {got_cols}×{got_rows} cells; sizing to {cols}×{rows}, {}×{} px",
            window.scale_factor(),
            cell.0,
            cell.1,
            want.width,
            want.height,
        );
        if want != size
            && let Some(now) = window.request_inner_size(want)
        {
            terminal.backend_mut().resize(now.width, now.height);
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
        self.window = Some(window);
        self.terminal = Some(terminal);
        self.lap.mark(&mut self.stats, "sizing");
        Ok(())
    }

    /// Every cell to the GPU on the next frame: ratatui's clear forgets
    /// the last frame, so the next draw is a whole one.
    fn repaint_all(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            let _ = terminal.clear();
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
        let framed = frame(terminal, &mut self.gui, &mut self.ctx, &mut WindowHost(window));
        let (cells, flush) = terminal.backend_mut().take_cells();
        if let (Some(stats), Some(started)) = (self.stats.as_mut(), started) {
            stats.frame(cells, flush, started.elapsed());
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
        self.sync_ime();
        self.play(event_loop);
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

    /// What the window saw, translated and fed. A composition in progress
    /// stops here, for the kit to draw; the paste chord reads the
    /// clipboard while a field has the keyboard and is nothing otherwise
    /// (off a Mac, Ctrl+V with no field is still the key it was).
    fn feed_raw(&mut self, event_loop: &ActiveEventLoop, mut raw: Raw) {
        if let Raw::ImePreedit(text) = &raw {
            let text = text.clone();
            self.set_preedit(&text);
            return;
        }
        if let Raw::ImeCommit(_) = raw {
            self.set_preedit("");
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
        while let Some(step) = self.script.as_mut().and_then(Script::next) {
            match step {
                Step::Wait(time) => {
                    let until = Instant::now() + time;
                    self.script_until = Some(until);
                    self.next_frame = self.next_frame.min(until);
                    break;
                }
                Step::Inputs(inputs) => {
                    let Some(grid) = self.grid() else { break };
                    for step_input in inputs {
                        let raw = match step_input {
                            Input::Raw(raw) => raw,
                            Input::MoveTo(col, row) => {
                                let (x, y) = grid.centre(col, row);
                                Raw::Move { x, y }
                            }
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
        let Some(terminal) = self.terminal.as_mut() else { return };
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
        // The covers this frame painted as pictures, as x,y w×h in cells.
        let placed = self.board.placed_rects();
        let covers = if placed.is_empty() {
            "none".to_string()
        } else {
            let rects: Vec<String> = placed
                .iter()
                .map(|r| format!("{},{} {}×{}", r.x, r.y, r.width, r.height))
                .collect();
            format!("{} at {}", placed.len(), rects.join(" "))
        };
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
        if let Err(e) = self.open(event_loop) {
            eprintln!("gui --window: {e}");
            self.exit_code = 1;
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
            WindowEvent::Resized(size) => self.resized(event_loop, size),
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
            // — and repaints every cell with it, which costs one full frame
            // and spares a compositor that dropped the hidden window's
            // contents a screen of stale rows.
            WindowEvent::Occluded(false) => {
                if let Some(stats) = self.stats.as_mut() {
                    stats.visible();
                }
                self.repaint_all();
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
        let now = Instant::now();
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
/// complete as the GUI grows new glyphs.
const BEYOND_HACK: &str = "★☆✓";

/// Where each platform keeps a face with [`BEYOND_HACK`]'s glyphs, best
/// first. A monospace face leads where there is one, so the stars keep a
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

/// The system face that fills in [`BEYOND_HACK`], for every language: the
/// first on the platform's list that has all of it, or failing that the
/// one that has the most, so as few cells as possible are boxes. The maps
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
    let Some((path, bytes, missing)) = best else {
        eprintln!("gui --window: no system face has {BEYOND_HACK}; they will be boxes");
        return None;
    };
    let font = Font::new(bytes)?;
    if missing.is_empty() {
        eprintln!("gui --window: {BEYOND_HACK} from {}", path.display());
    } else {
        eprintln!("gui --window: {BEYOND_HACK} from {}, which has no {missing}", path.display());
    }
    Some(font)
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
                covered = symbol.width().saturating_sub(1);
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
}
