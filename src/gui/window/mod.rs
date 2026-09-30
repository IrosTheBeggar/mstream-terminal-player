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

mod input;
mod script;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::Event as TermEvent;
use ratatui::style::Color;
use ratatui_wgpu::{Builder, Dimensions, Font, WgpuBackend};
use unicode_width::UnicodeWidthStr;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window, WindowId};

use input::{Grid, Raw, Translator};
use script::{Input, Script, Step};

use super::{Channels, Ctx, Flow, Gui, Host, finish, frame, input, render};
use crate::kit::theme::th;
use crate::runtime::block_on;
use crate::viz_window::overlay::cjk_faces;

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

type WindowTerminal = Terminal<WgpuBackend<'static, 'static>>;

/// The player in a window, from a Gui and workers that `gui::start` has
/// already brought up; the exit code is the terminal's (0, or 1 when a
/// frame failed), or 1 when there is no window to open.
pub(super) fn run(mut gui: Gui, channels: Channels) -> i32 {
    // No panic hook of the terminal's kind: `tui::install_panic_hook` hands
    // mouse capture back and pops a window title the player pushed — with
    // no terminal claimed, the first is noise on whatever launched the
    // window and the second pops a title that is not ours. The default
    // hook prints every panic, the audio thread's caught ones included,
    // which with no screen to deface is what a window wants; that thread
    // still reports its own death as an event, as it does under the
    // terminal.
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
    let dump = std::env::var_os("MSTREAM_WINDOW_DUMP").map(PathBuf::from);
    let ctx = Ctx::new(&gui.app, channels);
    let mut app = App {
        gui,
        ctx,
        grid,
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
        script: Script::from_env(),
        script_until: None,
        quit_flushed: false,
        min_surface: PhysicalSize::new(1, 1),
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("gui --window: {e}");
        app.exit_code = 1;
    }
    // `exiting` has torn down already on every way out winit reports; this
    // is for the one it does not.
    app.teardown();
    app.exit_code
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
    /// What an input method is composing, not yet committed.
    preedit: String,
    script: Option<Script>,
    /// When the script's current `wait` runs out.
    script_until: Option<Instant>,
    /// A quit came through the input half, which flushed the saver: the
    /// teardown does not flush it again, and input stops.
    quit_flushed: bool,
    /// One cell, in pixels: the least surface the backend can draw on.
    min_surface: PhysicalSize<u32>,
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
        let font_px = (FONT_PT * window.scale_factor()).round() as u32;
        // The display handle goes in with the instance, as the visualizer's
        // window does it: on Wayland and X11 the backend needs it before a
        // surface can exist.
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let size = window.inner_size();
        let dimensions = Dimensions {
            width: NonZeroU32::new(size.width.max(1)).expect("at least one"),
            height: NonZeroU32::new(size.height.max(1)).expect("at least one"),
        };
        // Hack first for everything it has, then a system face for the few
        // symbols it lacks, then the borrowed CJK face. `with_regular_fonts`
        // keeps that order, where `with_fonts` would sort by width and could
        // put a borrowed face's own Latin in front of Hack's. Hack is also
        // the builder's last resort, which is what bold and italic cells
        // fall back to with faked styles.
        let mut faces = vec![hack()?];
        faces.extend(symbol_fallback());
        faces.extend(cjk_fallback(&rust_i18n::locale()));
        // The pinned truecolour palette always has a ground; black is only
        // the answer to a palette that somehow resolved without one.
        let theme = th();
        let builder = Builder::from_font(hack()?)
            .with_regular_fonts(faces)
            .with_font_size_px(font_px)
            .with_width_and_height(dimensions)
            .with_bg_color(theme.ground.unwrap_or(Color::Black))
            .with_fg_color(theme.text)
            .with_instance(instance);
        let backend = block_on(builder.build_with_target(window.clone()))?
            .map_err(|e| format!("the window has nothing to draw with: {e}"))?;
        let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

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
        let want = PhysicalSize::new(cell.0 * u32::from(cols), cell.1 * u32::from(rows));
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

        // The input method's text arrives as its own events from here on
        // (dead keys on macOS among them). Its candidate window wants the
        // caret's place, but the kit draws its caret as a glyph and never
        // tells the terminal where it is, so there is no caret cell to
        // hand over cheaply: the window's top-left, one cell big, until
        // the kit reports one.
        // A terminal is never less than a cell, and ratatui-wgpu counts on
        // that: a surface with room for no whole cell reports a 0×0 grid,
        // and its cursor clamp (`width - 1`) then underflows. The window is
        // held to one cell for the platforms that honour a minimum, and
        // `resized` holds the surface to it for any that do not.
        self.min_surface = PhysicalSize::new(cell.0.max(1), cell.1.max(1));
        window.set_min_inner_size(Some(self.min_surface));

        window.set_ime_allowed(true);
        window.set_ime_cursor_area(PhysicalPosition::new(0, 0), PhysicalSize::new(cell.0, cell.1));

        window.request_redraw();
        self.asked = Some(Instant::now());
        self.window = Some(window);
        self.terminal = Some(terminal);
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

    /// The loop's frame half, and the time the next one is wanted by.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.asked = None;
        let (Some(window), Some(terminal)) = (self.window.as_deref(), self.terminal.as_mut())
        else {
            return;
        };
        match frame(terminal, &mut self.gui, &mut self.ctx, &mut WindowHost(window)) {
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
        if input(&mut self.gui, &mut self.ctx, event) == Flow::Quit {
            self.quit_flushed = true;
            event_loop.exit();
            return false;
        }
        self.ask_redraw();
        true
    }

    /// What the window saw, translated and fed. Composition in progress
    /// stops here: the kit has no way to draw uncommitted text yet, so the
    /// preedit is kept and said on stderr once per change — which is how a
    /// person testing an input method sees composition happen at all.
    fn feed_raw(&mut self, event_loop: &ActiveEventLoop, raw: Raw) {
        if let Raw::ImePreedit(text) = &raw {
            if *text != self.preedit {
                eprintln!("gui --window: preedit {text:?}");
                self.preedit.clone_from(text);
            }
            return;
        }
        if let Raw::ImeCommit(_) = raw {
            self.preedit.clear();
        }
        let Some(grid) = self.grid() else { return };
        for event in self.translator.translate(raw, grid) {
            if !self.feed(event_loop, event) {
                return;
            }
        }
        self.ask_redraw();
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
                Step::Quit => {
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

    /// The window's text, row by row, then what the pointer is doing: the
    /// cell it is over, whether it is the hand, and any composition.
    fn script_dump(&mut self, path: &Path) -> std::io::Result<()> {
        let grid = self.grid();
        let Some(terminal) = self.terminal.as_ref() else { return Ok(()) };
        let mut text = terminal.backend().get_text();
        let pointer = grid.map_or((0, 0), |grid| self.translator.pointer(grid));
        let surface = grid.map_or((0, 0), |grid| (grid.width, grid.height));
        text.push_str(&format!(
            "-- pointer {},{} at {:.1},{:.1} px; surface {}×{} px; cursor {}; preedit {:?}\n",
            pointer.0,
            pointer.1,
            self.translator.pixel().0,
            self.translator.pixel().1,
            surface.0,
            surface.1,
            if self.ctx.hand { "hand" } else { "default" },
            self.preedit,
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
        self.terminal = None;
        self.window = None;
        if !self.quit_flushed {
            self.ctx.saver.flush(&self.gui.app);
        }
        finish(&mut self.gui, &self.ctx);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
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
            WindowEvent::CloseRequested => event_loop.exit(),
            // The new size to the surface, then a whole repaint: ratatui-wgpu
            // marks rows clean even when a present fails, and a present
            // against a surface mid-resize can, so the next frame draws
            // every cell. The GUI's own resize bookkeeping runs through the
            // input half with the grid the surface now holds.
            WindowEvent::Resized(size) => self.resized(event_loop, size),
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
            // ratatui-wgpu presents only cells that changed, and a present
            // that finds the window not on screen yet (wgpu's `Occluded`,
            // which the very first frame gets on macOS) is dropped with the
            // rows already marked clean — so a still screen would stay
            // blank. The window coming into view repaints every cell.
            WindowEvent::Occluded(false) => self.repaint_all(),
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
        if self.asked.is_none() && now >= self.next_frame {
            self.ask_redraw();
        }
        let flow = match self.asked {
            Some(at) if now.duration_since(at) < STALLED_REDRAW => ControlFlow::Poll,
            Some(_) => ControlFlow::WaitUntil(now + STALLED_REDRAW),
            None => ControlFlow::WaitUntil(self.next_frame),
        };
        event_loop.set_control_flow(flow);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.teardown();
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
/// Every path is face 0, the only face ratatui-wgpu opens.
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
    // Windows' font folder is under wherever Windows is.
    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
    paths
        .iter()
        .map(|path| if cfg!(windows) { windows.join(path) } else { PathBuf::from(path) })
        .collect()
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
/// one that has the most, so as few cells as possible are boxes.
fn symbol_fallback() -> Option<Font<'static>> {
    let mut best: Option<(PathBuf, Vec<u8>, String)> = None;
    for path in symbol_faces() {
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Some(missing) = missing_from(&bytes, BEYOND_HACK) else { continue };
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
    // Leaked for the reason the CJK face is (below): the backend borrows
    // it for as long as the process lives.
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let font = Font::new(bytes)?;
    if missing.is_empty() {
        eprintln!("gui --window: {BEYOND_HACK} from {}", path.display());
    } else {
        eprintln!("gui --window: {BEYOND_HACK} from {}, which has no {missing}", path.display());
    }
    Some(font)
}

/// A system face for Japanese or Chinese, found where the visualizer's
/// controls find theirs; `None` for every other language, which Hack
/// covers. ratatui-wgpu opens the first face of a collection only, so a
/// face deeper in one — Noto's Chinese, on Linux — is passed over for the
/// next choice.
fn cjk_fallback(lang: &str) -> Option<Font<'static>> {
    for (path, index) in cjk_faces(lang) {
        if index != 0 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        // The backend borrows its faces for as long as it lives, and it
        // lives as long as the process: the bytes are leaked to say so.
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        match Font::new(bytes) {
            Some(font) => {
                eprintln!("gui --window: {lang} glyphs from {}", path.display());
                return Some(font);
            }
            None => {
                eprintln!("gui --window: {} is not a face the backend can read", path.display())
            }
        }
    }
    if matches!(lang, "ja" | "zh") {
        eprintln!("gui --window: no system face for {lang}; its glyphs will be boxes");
    }
    None
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
}
