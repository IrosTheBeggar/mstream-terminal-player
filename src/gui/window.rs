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
//! The only input fed so far is the window's own resize; the keyboard and
//! the pointer are later steps. The terminal path does not come through
//! this file at all.
//!
//! Two levers ride along, both hidden. `MSTREAM_WINDOW_SIZE=<cols>,<rows>`
//! opens the window at another grid than 100×30 — 70,20 shows the mini
//! player. `MSTREAM_WINDOW_DUMP=<dir>` writes what the window holds as text,
//! and the same Gui drawn into a `TestBackend` of the same size, a few
//! frames in, and says on stderr whether they match. That is how the spike
//! checks fidelity without reading pixels.

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
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, Window, WindowId};

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

        // The cell comes back from the backend, not from arithmetic here:
        // its width is the narrowest of the faces it was given. The window
        // is then set to the grid exactly — grown when the opening guess
        // fell short, trimmed when it overshot — so the window and the
        // render tests draw the same number of cells.
        let reported = terminal.backend_mut().window_size().map_err(|e| e.to_string())?;
        let (got_cols, got_rows) =
            (reported.columns_rows.width.max(1), reported.columns_rows.height.max(1));
        let cell = (
            u32::from(reported.pixels.width) / u32::from(got_cols),
            u32::from(reported.pixels.height) / u32::from(got_rows),
        );
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
            Ok(wait) => self.next_frame = Instant::now() + wait,
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
    }

    /// One input through the loop's input half. A quit has flushed the
    /// saver already; the teardown in `exiting` does the rest. Anything
    /// else wants a frame now, as the terminal draws after every batch.
    fn feed(&mut self, event_loop: &ActiveEventLoop, event: TermEvent) {
        if input(&mut self.gui, &mut self.ctx, event) == Flow::Quit {
            event_loop.exit();
            return;
        }
        self.ask_redraw();
    }

    /// The end of gui::run, minus the terminal's own steps: the window and
    /// its surface go first, the way the terminal is restored first, then
    /// the player's teardown. The close button and Cmd-Q arrive here
    /// without passing through a quit in `input`, so the saver is flushed
    /// here; after a quit that did pass through, this writes the same
    /// files again.
    fn teardown(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        self.terminal = None;
        self.window = None;
        self.ctx.saver.flush(&self.gui.app);
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

    // Esc does not close the window: the GUI's own keys, a later step,
    // give Esc to the modals and rooms that back out with it, as the
    // terminal does. The close button and the app menu's Quit close it.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // The new size to the surface, then a whole repaint: ratatui-wgpu
            // marks rows clean even when a present fails, and a present
            // against a surface mid-resize can, so the next frame draws
            // every cell. The GUI's own resize bookkeeping runs through the
            // input half with the grid the surface now holds.
            WindowEvent::Resized(size) => {
                let Some(terminal) = self.terminal.as_mut() else { return };
                terminal.backend_mut().resize(size.width, size.height);
                let _ = terminal.clear();
                let Ok(grid) = terminal.backend_mut().size() else { return };
                self.feed(event_loop, TermEvent::Resize(grid.width, grid.height));
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
