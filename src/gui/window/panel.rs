//! A region of the window drawn by egui rather than as cells: the egui
//! spike's seam, the stats page its first tenant. Covers (covers.rs) put
//! pictures over cells the drawing path left blank; a panel puts a whole
//! page there. Three hands meet here, all on the window's thread:
//!
//! - the host's (mod.rs): winit's events, fed to egui while a panel is on
//!   screen. The pointer inside the region is egui's alone — a press there,
//!   its drag and its release, the wheel; everything else still goes to the
//!   grid as before, the keys above all, so a page keeps its keymap and the
//!   GUI's own keys still switch rooms.
//! - the face's: while it draws, [`Panel::show`] runs one egui pass over
//!   the cells it names and tessellates it; the face leaves those cells
//!   blank, as it does under a cover.
//! - the post-processor's: after the text blit and the covers, the pass is
//!   painted over the region ([`Painter`], from `CoverPost::process`).
//!
//! The region's place is the cell rect's share of the grid, which is its
//! share of the surface (covers.rs says why), in egui's points. egui is told
//! the region IS its screen, so everything it draws — popups included —
//! stays inside the cells the page owns.
//!
//! What it does not do yet: a GUI overlay that opens over the region (the
//! header's server menu, a modal, a tooltip) stands the panel down for as
//! long as it is up, rather than being painted over it; and an input
//! method never reaches egui, so a panel has no text fields.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use egui::epaint::{ClippedPrimitive, ImageDelta, TextureId};
use egui::{FontData, FontDefinitions, FontFamily, pos2};
use ratatui::layout::Rect;
use winit::event::WindowEvent;
use winit::window::Window;

use super::covers::Board;
use super::input::{Button, Raw, Wheel};

/// The font family a panel's figures and headings are set in: the
/// system's semibold UI face where there is one, else the regular.
pub(crate) const STRONG: &str = "strong";

/// What one pass drew, for the face's frame: whether the pointer is over
/// something egui would act on (the GUI shows the hand for it).
pub(crate) struct Shown {
    pub hand: bool,
}

/// The panel as the host and the face share it.
pub(crate) struct Panel {
    board: Arc<Board>,
    live: Mutex<Live>,
}

struct Live {
    /// Made on attach, styled and given its faces on the first pass.
    ctx: egui::Context,
    fonts_set: bool,
    state: Option<egui_winit::State>,
    window: Option<Arc<Window>>,
    /// The region in physical pixels, as the last pass placed it.
    region: Option<egui::Rect>,
    /// The pointer, in physical pixels, as winit last placed it.
    cursor: Option<(f32, f32)>,
    /// A press began in the region: its drag and release are egui's too.
    captured: bool,
    /// Events egui has been given since its last pass.
    pending: bool,
    /// When egui asked to run again (an animation, a tooltip's delay).
    repaint_at: Option<Instant>,
}

impl Panel {
    pub(super) fn new(board: Arc<Board>) -> Panel {
        let live = Live {
            ctx: egui::Context::default(),
            fonts_set: false,
            state: None,
            window: None,
            region: None,
            cursor: None,
            captured: false,
            pending: false,
            repaint_at: None,
        };
        Panel { board, live: Mutex::new(live) }
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The window, once it exists: egui's input is read against it.
    pub(super) fn attach(&self, window: Arc<Window>) {
        let mut live = self.lock();
        let state = egui_winit::State::new(
            live.ctx.clone(),
            egui::ViewportId::ROOT,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        live.state = Some(state);
        live.window = Some(window);
    }

    /// One of winit's events that is not the pointer's — the modifiers,
    /// focus, the size and the scale — for egui's own bookkeeping. The
    /// grid sees it too: nothing here is egui's alone.
    pub(super) fn observe(&self, event: &WindowEvent) {
        if !matches!(
            event,
            WindowEvent::ModifiersChanged(_)
                | WindowEvent::Focused(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
        ) {
            return;
        }
        let mut live = self.lock();
        let Live { state, window, captured, .. } = &mut *live;
        if matches!(event, WindowEvent::Focused(false)) {
            *captured = false;
        }
        if let (Some(state), Some(window)) = (state.as_mut(), window.as_deref()) {
            let _ = state.on_window_event(window, event);
        }
    }

    /// The pointer, as the window hands it to the grid (input.rs's [`Raw`],
    /// a person's or the script's), egui's to see while a panel is on
    /// screen. True when it is egui's alone and the grid must not see it: a
    /// press in the region, the release of a press that began there, the
    /// wheel over it. A move goes to both, so the GUI's hovers let go.
    pub(super) fn route(&self, raw: &Raw) -> bool {
        let on_screen = self.board.panel_on_screen();
        let mut live = self.lock();
        // The pointer's place is kept while the panel stands down too (an
        // overlay over it, or no panel up): a press that comes after with no
        // move between, on the panel back on screen, lands where it is and
        // not where the pointer stood before the stand-down.
        if let Raw::Move { x, y } = raw {
            live.cursor = Some((*x as f32, *y as f32));
        }
        if !on_screen {
            live.captured = false;
            return false;
        }
        let Live { state, window, region, cursor, captured, pending, ctx, .. } = &mut *live;
        let (Some(state), Some(window)) = (state.as_mut(), window.as_deref()) else { return false };
        let ppp = window.scale_factor() as f32 * ctx.zoom_factor();
        let inside = match (*region, *cursor) {
            (Some(region), Some((x, y))) => region.contains(pos2(x, y)),
            _ => false,
        };
        let at = cursor.map(|(x, y)| pos2(x, y) / ppp);
        let input = state.egui_input_mut();
        // No page here acts on a modified click or wheel yet.
        let modifiers = egui::Modifiers::default();
        let event = match raw {
            Raw::Move { x, y } => {
                input.events.push(egui::Event::PointerMoved(pos2(*x as f32, *y as f32) / ppp));
                *pending = true;
                return false;
            }
            Raw::Leave => {
                *captured = false;
                input.events.push(egui::Event::PointerGone);
                *pending = true;
                return false;
            }
            Raw::Button { button, down } if (*down && inside) || (!*down && *captured) => {
                *captured = *down;
                let button = match button {
                    Button::Left => egui::PointerButton::Primary,
                    Button::Right => egui::PointerButton::Secondary,
                    Button::Middle => egui::PointerButton::Middle,
                };
                let Some(pos) = at else { return false };
                egui::Event::PointerButton { pos, button, pressed: *down, modifiers }
            }
            Raw::Wheel(wheel) if inside => {
                let (unit, delta) = match wheel {
                    Wheel::Lines(lines) => (egui::MouseWheelUnit::Line, egui::vec2(0.0, *lines)),
                    Wheel::Pixels(px) => (egui::MouseWheelUnit::Point, egui::vec2(0.0, *px as f32 / ppp)),
                };
                egui::Event::MouseWheel { unit, delta, phase: egui::TouchPhase::Move, modifiers }
            }
            _ => return false,
        };
        input.events.push(event);
        *pending = true;
        true
    }

    /// The face's frame must run its pass though the player has nothing
    /// new: egui was given input, or asked to run again by now.
    pub(super) fn wants_pass(&self) -> bool {
        if !self.board.panel_on_screen() {
            return false;
        }
        let live = self.lock();
        live.pending || live.repaint_at.is_some_and(|at| at <= Instant::now())
    }

    /// When egui wants its next pass, while a panel is on screen.
    pub(super) fn wake_at(&self) -> Option<Instant> {
        if !self.board.panel_on_screen() {
            return None;
        }
        self.lock().repaint_at
    }

    /// One egui pass over `area`, a cell rect of `grid` the face leaves
    /// blank this frame, with `run` drawing into it; its paint waits on the
    /// board for the post-processor. None before there is a window, when
    /// the face draws the cells itself.
    pub(crate) fn show(&self, area: Rect, grid: Rect, run: &mut dyn FnMut(&mut egui::Ui)) -> Option<Shown> {
        let mut live = self.lock();
        if !live.fonts_set {
            fonts(&live.ctx);
            live.fonts_set = true;
        }
        let Live { ctx, state, window, region, pending, repaint_at, .. } = &mut *live;
        let (Some(state), Some(window)) = (state.as_mut(), window.clone()) else { return None };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 || area.width == 0 || area.height == 0 {
            return None;
        }
        let (cols, rows) = (f32::from(grid.width.max(1)), f32::from(grid.height.max(1)));
        let (w, h) = (size.width as f32, size.height as f32);
        let px = egui::Rect::from_min_max(
            pos2((f32::from(area.x) / cols * w).round(), (f32::from(area.y) / rows * h).round()),
            pos2((f32::from(area.right()) / cols * w).round(), (f32::from(area.bottom()) / rows * h).round()),
        );
        *region = Some(px);
        let ppp = window.scale_factor() as f32 * ctx.zoom_factor();
        let mut input = state.take_egui_input(&window);
        input.screen_rect = Some(egui::Rect::from_min_max(px.min / ppp, px.max / ppp));
        let mut output = ctx.run_ui(input, |ui| run(ui));
        *pending = false;
        *repaint_at = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|viewport| viewport.repaint_delay)
            .filter(|delay| *delay < Duration::from_secs(60))
            .map(|delay| Instant::now() + delay);
        let hand = output.platform_output.cursor_icon == egui::CursorIcon::PointingHand;
        let jobs = ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        self.board.panel_drawn(area, Paint::pass(jobs, output.pixels_per_point, output.textures_delta));
        Some(Shown { hand })
    }
}

/// What the face's pass leaves the post-processor, on the board. Texture
/// changes pile up until a process takes them, shown or not: the font
/// atlas grows by deltas, and one dropped is a glyph lost for good.
#[derive(Default)]
pub(crate) struct Paint {
    /// The panel's cells this frame; None while no panel is on screen.
    pub area: Option<Rect>,
    /// A new pass since the post-processor last took one: the shapes to
    /// paint from now on (empty to paint none).
    pub jobs: Option<Vec<ClippedPrimitive>>,
    pub pixels_per_point: f32,
    pub set: Vec<(TextureId, ImageDelta)>,
    pub free: Vec<TextureId>,
    /// Something drawn over the base layer this frame touches the region:
    /// the panel is not painted while it is up.
    pub under: bool,
    /// The face's draw showed the panel this frame.
    pub shown: bool,
    /// The overlays drawn so far this frame, for a panel shown after them.
    pub overlays: Vec<Rect>,
    /// `under` as the last drawn frame left it, put back when the face
    /// skips its draw.
    pub was_under: bool,
}

impl Paint {
    fn pass(jobs: Vec<ClippedPrimitive>, pixels_per_point: f32, mut delta: egui::TexturesDelta) -> Paint {
        Paint {
            jobs: Some(jobs),
            pixels_per_point,
            // One art's deltas in the order they came, which is the order
            // they must be applied in.
            set: delta.set.drain().flat_map(|(id, deltas)| deltas.into_iter().map(move |d| (id, d))).collect(),
            free: delta.free.drain().collect(),
            ..Paint::default()
        }
    }

    /// The post-processor must present though no cell changed.
    pub fn is_news(&self) -> bool {
        self.jobs.is_some() || !self.set.is_empty()
    }
}

/// egui's paint on the backend's device, owned by the post-processor. The
/// renderer is made on the first pass there is something to paint, so a
/// window that never shows a panel never compiles egui's pipeline.
#[derive(Default)]
pub(crate) struct Painter {
    renderer: Option<egui_wgpu::Renderer>,
    /// The last pass's shapes: painted again on every process, since the
    /// text blit before it paints the whole surface.
    jobs: Vec<ClippedPrimitive>,
    pixels_per_point: f32,
}

impl Painter {
    pub(super) fn process(
        &mut self,
        board: &Board,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface_config: &wgpu::SurfaceConfiguration,
        surface_view: &wgpu::TextureView,
    ) {
        let (set, free, visible) = {
            let mut paint = board.panel();
            if let Some(jobs) = paint.jobs.take() {
                self.jobs = jobs;
                self.pixels_per_point = paint.pixels_per_point;
            }
            if paint.area.is_none() {
                self.jobs.clear();
            }
            (std::mem::take(&mut paint.set), std::mem::take(&mut paint.free), !paint.under)
        };
        if set.is_empty() && free.is_empty() && self.jobs.is_empty() {
            return;
        }
        let renderer = self.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(device, surface_config.format, egui_wgpu::RendererOptions::default())
        });
        for (id, delta) in &set {
            renderer.update_texture(device, queue, *id, delta);
        }
        if visible && !self.jobs.is_empty() {
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [surface_config.width, surface_config.height],
                pixels_per_point: self.pixels_per_point,
            };
            // egui's own callbacks would want buffers submitted first; the
            // panels paint none.
            let _ = renderer.update_buffers(device, queue, encoder, &self.jobs, &screen);
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Panel Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    // Over the text and the covers, never instead of them.
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                ..Default::default()
            });
            renderer.render(&mut pass.forget_lifetime(), &self.jobs, &screen);
        }
        // What the pass let go of was not in its shapes: freed now, before
        // the submission, is no texture a draw above still samples.
        for id in &free {
            renderer.free_texture(id);
        }
    }
}

/// The panel's faces: the system's UI face where it has one — Segoe UI on
/// Windows, San Francisco on a Mac, DejaVu or Noto elsewhere — so a page
/// reads as the platform's, with egui's own Ubuntu Light and Hack behind
/// it, and the system's CJK face for Japanese and Chinese (as the
/// visualizer's controls borrow it). [`STRONG`] is the semibold face, or
/// the regular where there is none. Each borrowed face is checked the way
/// egui will read it, since egui panics on one it cannot parse. A page's
/// tests lay it out with the same.
pub(crate) fn fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert("Ubuntu-Light".into(), Arc::new(FontData::from_static(epaint_default_fonts::UBUNTU_LIGHT)));
    fonts.font_data.insert("Hack".into(), Arc::new(FontData::from_static(epaint_default_fonts::HACK_REGULAR)));
    let (regular, strong) = system_ui_faces();
    let mut proportional: Vec<String> = Vec::new();
    let mut bold: Vec<String> = Vec::new();
    if let Some(face) = regular.into_iter().find_map(|path| readable(&path, 0)) {
        fonts.font_data.insert("system-ui".into(), Arc::new(face));
        proportional.push("system-ui".into());
    }
    if let Some(face) = strong.into_iter().find_map(|path| readable(&path, 0)) {
        fonts.font_data.insert("system-ui-strong".into(), Arc::new(face));
        bold.push("system-ui-strong".into());
    }
    bold.extend(proportional.iter().cloned());
    for list in [&mut proportional, &mut bold] {
        list.push("Ubuntu-Light".into());
        list.push("Hack".into());
    }
    let lang = rust_i18n::locale().to_string();
    let cjk = crate::viz_window::overlay::cjk_faces(&lang).into_iter().find_map(|(path, index)| readable(&path, index));
    if let Some(mut face) = cjk {
        face.tweak.y_offset_factor = 0.12;
        fonts.font_data.insert("system-cjk".into(), Arc::new(face));
        proportional.push("system-cjk".into());
        bold.push("system-cjk".into());
    }
    let monospace = vec!["Hack".to_string(), "Ubuntu-Light".to_string()];
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts.families.insert(FontFamily::Name(STRONG.into()), bold);
    ctx.set_fonts(fonts);
}

/// A face egui can read, from `path`'s `index`th.
fn readable(path: &std::path::Path, index: u32) -> Option<FontData> {
    let bytes = std::fs::read(path).ok()?;
    if skrifa::FontRef::from_index(&bytes, index).is_err() {
        eprintln!("gui --window: {} is not a face egui can read", path.display());
        return None;
    }
    let mut face = FontData::from_owned(bytes);
    face.index = index;
    Some(face)
}

/// Where each platform keeps its UI face, regular and semibold, best first.
fn system_ui_faces() -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>) {
    use std::path::PathBuf;
    if cfg!(windows) {
        let fonts = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("Fonts");
        (vec![fonts.join("segoeui.ttf")], vec![fonts.join("seguisb.ttf"), fonts.join("segoeuib.ttf")])
    } else if cfg!(target_os = "macos") {
        let sf = PathBuf::from("/System/Library/Fonts/SFNS.ttf");
        let helvetica = PathBuf::from("/System/Library/Fonts/Helvetica.ttc");
        (vec![sf.clone(), helvetica.clone()], vec![sf, helvetica])
    } else {
        let regular = [
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        ];
        let strong = [
            "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
        ];
        (regular.iter().map(PathBuf::from).collect(), strong.iter().map(PathBuf::from).collect())
    }
}
