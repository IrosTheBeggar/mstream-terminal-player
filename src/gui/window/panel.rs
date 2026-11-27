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
use ratatui::layout::{Position, Rect};
use winit::event::WindowEvent;
use winit::window::Window;

use super::covers::Board;
use super::input::{Button, Raw, Wheel};

/// The font family a panel's figures and headings are set in: the
/// system's semibold UI face where there is one, else the regular.
pub(crate) const STRONG: &str = "strong";

/// Whether a page with an egui face draws it in the panel: unless
/// `MSTREAM_WINDOW_CELLS` is set to anything but 0, which keeps every page
/// in cells, the spike's way to put the two side by side. Every face that
/// takes the panel asks the same.
pub(crate) fn wanted() -> bool {
    std::env::var_os("MSTREAM_WINDOW_CELLS").is_none_or(|v| v.is_empty() || v == "0")
}

/// The cell of `cells` under `at`, a point of an egui pass whose screen,
/// `points`, is those cells: the region's size in cells and in points is
/// the scale between the two.
pub(crate) fn cell_under(at: egui::Pos2, points: egui::Rect, cells: Rect) -> Position {
    let along = |v: f32, from: f32, span: f32, n: u16| -> u16 {
        if n == 0 || span <= 0.0 {
            return 0;
        }
        (((v - from) / span * f32::from(n)).floor().max(0.0) as u16).min(n - 1)
    };
    Position {
        x: cells.x + along(at.x, points.min.x, points.width(), cells.width),
        y: cells.y + along(at.y, points.min.y, points.height(), cells.height),
    }
}

/// The cell a caret stands in, by its foot as a page's egui pass reports
/// it (`crate::admin::EguiShown`, the setup wizard's view): the foot is the
/// caret's bottom edge rather than a point of it, so the cell is the one
/// under its last half-point. A foot on a cell's top edge is the cell
/// above, and the candidates float right under the caret, not a row lower.
/// The host notes it on its surface, where the window reads the field with
/// the keyboard (`Face::caret_at`).
pub(crate) fn caret_cell(foot: egui::Pos2, points: egui::Rect, cells: Rect) -> Position {
    cell_under(foot - egui::vec2(0.0, 0.5), points, cells)
}

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
    /// A press began in the region, of this button: its drag and release
    /// are egui's too, and egui holds the button down until it hears the
    /// release.
    captured: Option<egui::PointerButton>,
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
            captured: None,
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
        let on_screen = self.board.panel_on_screen();
        let mut live = self.lock();
        let Live { state, window, captured, cursor, ctx, pending, .. } = &mut *live;
        let (Some(state), Some(window)) = (state.as_mut(), window.as_deref()) else {
            *captured = None;
            return;
        };
        // The window lost the keyboard mid-press: the release may never
        // come, so egui is told the button let go where the pointer stands,
        // or its drag would hold until the next press.
        if matches!(event, WindowEvent::Focused(false)) {
            let ppp = window.scale_factor() as f32 * ctx.zoom_factor();
            let at = cursor.map(|(x, y)| pos2(x, y) / ppp);
            let release = let_go(captured, at, !on_screen);
            if !release.is_empty() {
                state.egui_input_mut().events.extend(release);
                *pending = true;
            }
        }
        let _ = state.on_window_event(window, event);
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
        let Live { state, window, region, cursor, captured, pending, ctx, .. } = &mut *live;
        let (Some(state), Some(window)) = (state.as_mut(), window.as_deref()) else { return false };
        let ppp = window.scale_factor() as f32 * ctx.zoom_factor();
        let inside = match (*region, *cursor) {
            (Some(region), Some((x, y))) => region.contains(pos2(x, y)),
            _ => false,
        };
        let at = cursor.map(|(x, y)| pos2(x, y) / ppp);
        let (events, ours) = routed(raw, on_screen, inside, at, ppp, captured);
        if !events.is_empty() {
            state.egui_input_mut().events.extend(events);
            *pending = true;
        }
        ours
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

/// What [`Panel::route`] makes of one of the pointer's events: what egui is
/// given of it, and whether it is egui's alone. `at` is the pointer's place
/// in egui's points, `ppp` the pixels in a point, `captured` the button a
/// press in the region holds down for egui.
///
/// A held button is egui's until the hand lets it go, whatever comes over
/// the panel meanwhile. An overlay that comes up mid-drag (a GUI tooltip
/// that ripens, a menu) stands the panel down, and the release still goes
/// to egui, where the pointer is when it comes, so egui's press ends with
/// the hand's: dropped, egui's button would stay down and the log's hold or
/// a room's drag stick until the next press. It is not let go at the
/// stand-down itself: the hand still holds it, and a stand-down that ends
/// first finds the drag going on. Only where the release may never be
/// reported — the pointer gone from the window — is it let go at once.
/// Egui was shown none of the moves under the overlay, so it hears where
/// the pointer went before the release ([`let_go`]).
fn routed(
    raw: &Raw,
    on_screen: bool,
    inside: bool,
    at: Option<egui::Pos2>,
    ppp: f32,
    captured: &mut Option<egui::PointerButton>,
) -> (Vec<egui::Event>, bool) {
    // No page here acts on a modified click or wheel yet.
    let modifiers = egui::Modifiers::default();
    match raw {
        Raw::Leave => {
            let mut events = let_go(captured, at, !on_screen);
            events.push(egui::Event::PointerGone);
            (events, false)
        }
        Raw::Button { button, down: false } if *captured == Some(pointer_button(button)) => {
            (let_go(captured, at, !on_screen), true)
        }
        // Another button while one is held is the panel's too, and egui
        // acts on one at a time.
        Raw::Button { .. } if captured.is_some() => (Vec::new(), true),
        _ if !on_screen => (Vec::new(), false),
        Raw::Move { .. } => (at.map(egui::Event::PointerMoved).into_iter().collect(), false),
        Raw::Button { button, down: true } if inside => {
            let Some(pos) = at else { return (Vec::new(), false) };
            let button = pointer_button(button);
            *captured = Some(button);
            (vec![egui::Event::PointerButton { pos, button, pressed: true, modifiers }], true)
        }
        Raw::Wheel(wheel) if inside => {
            let (unit, delta) = match wheel {
                Wheel::Lines(lines) => (egui::MouseWheelUnit::Line, egui::vec2(0.0, *lines)),
                Wheel::Pixels(px) => (egui::MouseWheelUnit::Point, egui::vec2(0.0, *px as f32 / ppp)),
            };
            (vec![egui::Event::MouseWheel { unit, delta, phase: egui::TouchPhase::Move, modifiers }], true)
        }
        _ => (Vec::new(), false),
    }
}

/// The release of the button egui holds, at `at`, if it holds one: the
/// panel lets go of it. `unseen` while the panel stood down, when egui was
/// shown none of the pointer's moves: it hears where the pointer went
/// first, so a press dragged off its control under an overlay is no click
/// on it, as it would not be with the moves seen.
fn let_go(captured: &mut Option<egui::PointerButton>, at: Option<egui::Pos2>, unseen: bool) -> Vec<egui::Event> {
    let (Some(button), Some(pos)) = (captured.take(), at) else { return Vec::new() };
    let moved = unseen.then_some(egui::Event::PointerMoved(pos));
    let release = egui::Event::PointerButton { pos, button, pressed: false, modifiers: egui::Modifiers::default() };
    moved.into_iter().chain([release]).collect()
}

fn pointer_button(button: &Button) -> egui::PointerButton {
    match button {
        Button::Left => egui::PointerButton::Primary,
        Button::Right => egui::PointerButton::Secondary,
        Button::Middle => egui::PointerButton::Middle,
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
/// visualizer's controls borrow it), behind the monospace too, where the
/// server's log sets file names in any script. [`STRONG`] is the semibold
/// face, or the regular where there is none. Each borrowed face is checked
/// the way egui will read it, since egui panics on one it cannot parse. A
/// page's tests lay it out with the same.
pub(crate) fn fonts(ctx: &egui::Context) {
    install_fonts(ctx, &rust_i18n::locale());
}

/// [`fonts`] with a CJK face behind the others whatever the language: for
/// a page that names languages in their own scripts and may switch to one
/// of them while it is up (the setup wizard's chip and list name 日本語 and
/// 中文 in every language, as the window's cells do with their own
/// fallbacks). The Chinese face in Chinese, else the Japanese one, whose
/// Han draws Chinese's name too.
pub(crate) fn fonts_every_script(ctx: &egui::Context) {
    let lang = if &*rust_i18n::locale() == "zh" { "zh" } else { "ja" };
    install_fonts(ctx, lang);
}

/// The faces [`fonts`] describes, with the CJK one for `lang` where it has
/// one.
fn install_fonts(ctx: &egui::Context, lang: &str) {
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
    let mut monospace = vec!["Hack".to_string(), "Ubuntu-Light".to_string()];
    let cjk = crate::viz_window::overlay::cjk_faces(lang).into_iter().find_map(|(path, index)| readable(&path, index));
    if let Some(mut face) = cjk {
        let words = fonts.font_data.get(&proportional[0]);
        face.tweak.y_offset_factor = words.and_then(|words| cjk_drop(words, &face)).unwrap_or(0.0);
        fonts.font_data.insert("system-cjk".into(), Arc::new(face));
        for list in [&mut proportional, &mut bold, &mut monospace] {
            list.push("system-cjk".into());
        }
    }
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(FontFamily::Monospace, monospace);
    fonts.families.insert(FontFamily::Name(STRONG.into()), bold);
    ctx.set_fonts(fonts);
}

/// Ideographs that Japanese and Chinese share, whose ink together spans
/// the square an ideograph is drawn in.
const IDEOGRAPHS: &str = "日本語中文";

/// How far the CJK face `cjk` is set down behind `words`, the face its
/// glyphs share a line with, as a share of the size (its tweak's
/// `y_offset_factor`), so that its ideographs stand centred on the capitals
/// of `words`, as a Latin name and a CJK one stand on one row of a list.
/// egui sets a fallback face's baseline where the two faces' line boxes
/// share a middle, which stands each CJK face as high or as low as its line
/// box is deep: Yu Gothic's ideographs rode 3 points over Segoe UI's
/// capitals at 14 points and YaHei's 2 under them with the one share the
/// panel used to give every face. Read off both faces' own metrics and
/// outlines ([`ink_under_middle`]); none where either cannot say. One face
/// takes one share, so the monospace's lines, Hack's, take the UI face's,
/// which stands their ideographs about a point under Hack's capitals'
/// middle at the 12.5 points the pages set paths in, a twelfth of the size
/// (they stood over 2 points off before); a share of their own would take a
/// second copy of the CJK collection, 14 megabytes for Yu Gothic and 20
/// for YaHei.
fn cjk_drop(words: &FontData, cjk: &FontData) -> Option<f32> {
    Some(ink_under_middle(words, "H")? - ink_under_middle(cjk, IDEOGRAPHS)?)
}

/// How far under the middle of the line box egui gives `face` the middle
/// of the ink of `sample`'s glyphs stands, as a share of the size: the
/// line box from the face's ascent over its baseline to its descent and
/// leading under it, the ink the outlines' control points (an outline's
/// own reach, for the straight strokes of an "H" and an ideograph).
fn ink_under_middle(face: &FontData, sample: &str) -> Option<f32> {
    use skrifa::MetadataProvider;
    use skrifa::instance::{LocationRef, Size};
    use skrifa::outline::DrawSettings;
    use skrifa::outline::pen::ControlBoundsPen;
    let font = skrifa::FontRef::from_index(&face.font, face.index).ok()?;
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    let em = f32::from(metrics.units_per_em);
    let (charmap, outlines) = (font.charmap(), font.outline_glyphs());
    let mut pen = ControlBoundsPen::new();
    for c in sample.chars() {
        let glyph = outlines.get(charmap.map(c)?)?;
        glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
    }
    let ink = pen.bounding_box().filter(|_| em > 0.0)?;
    let middle = (metrics.ascent + metrics.descent - metrics.leading) / 2.0;
    Some((middle - (ink.y_min + ink.y_max) / 2.0) / em)
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

#[cfg(test)]
mod tests {
    use super::*;

    const PPP: f32 = 2.0;
    /// A scale whose pixels are an eighth of a point: what is laid out at
    /// it lands on a grid too fine to blur what the faces' places are.
    const FINE: f32 = 8.0;

    /// Where the middle of `s`'s ink stands under the top of its line, laid
    /// out alone in `font`.
    fn ink_middle(ui: &egui::Ui, s: &str, font: egui::FontId) -> f32 {
        let galley = ui.painter().layout_no_wrap(s.to_string(), font, egui::Color32::WHITE);
        let row = &galley.rows[0];
        let (mut top, mut foot) = (f32::MAX, f32::MIN);
        for glyph in &row.row.glyphs {
            let head = row.pos.y + glyph.pos.y + glyph.uv_rect.offset.y;
            (top, foot) = (top.min(head), foot.max(head + glyph.uv_rect.size.y));
        }
        (top + foot) / 2.0
    }

    /// A CJK name stands centred on the capitals of a Latin one beside it,
    /// whichever CJK face the system lends (Yu Gothic for Japanese, YaHei
    /// for Chinese on Windows), as the language list's rows set 日本語 and
    /// 中文 between Deutsch and English: in the UI face, regular and
    /// semibold, within a quarter of a point on the fine grid (on the
    /// window's, at a scale of 2, within the pixel and a half that each
    /// ink's edges and the baseline landing on it leaves), where they stood
    /// 3 points off with the share every face was given; in the monospace,
    /// which takes the UI face's share, as far off Hack's capitals as the
    /// two faces' capitals stand off their lines' middles, read off their
    /// own outlines and metrics, within the same quarter of a point — about
    /// a twelfth of the size under them in Segoe UI, a seventh over them in
    /// a face as short over its baseline as Calibri. A system with no CJK
    /// face has nothing to check.
    #[test]
    fn a_cjk_name_stands_centred_on_the_capitals_beside_it() {
        let words = system_ui_faces().0.into_iter().find_map(|path| readable(&path, 0));
        let words = words.unwrap_or_else(|| FontData::from_static(epaint_default_fonts::UBUNTU_LIGHT));
        let hack = FontData::from_static(epaint_default_fonts::HACK_REGULAR);
        let share = ink_under_middle(&words, "H").expect("the UI face's capitals") - ink_under_middle(&hack, "H").expect("Hack's capitals");
        for lang in ["ja", "zh"] {
            if !crate::viz_window::overlay::cjk_faces(lang).into_iter().any(|(path, index)| readable(&path, index).is_some()) {
                continue;
            }
            let ctx = egui::Context::default();
            install_fonts(&ctx, lang);
            ctx.set_pixels_per_point(FINE);
            ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
            let mut out = ctx.run_ui(Default::default(), |ui| {
                let off = |font: egui::FontId| ink_middle(ui, IDEOGRAPHS, font.clone()) - ink_middle(ui, "H", font);
                for size in [12.0, 12.5, 13.0, 14.0, 16.0, 22.0] {
                    for font in [egui::FontId::proportional(size), egui::FontId::new(size, FontFamily::Name(STRONG.into()))] {
                        let off = off(font.clone());
                        assert!(off.abs() <= 0.25, "{lang} {font:?}: the ideographs' middle {off} under the capitals'");
                    }
                    let (off, want) = (off(egui::FontId::monospace(size)), share * size);
                    assert!((off - want).abs() <= 0.25, "{lang} the monospace at {size}: the ideographs' middle {off} under the capitals', not the UI face's {want}");
                }
            });
            out.textures_delta.clear();
        }
    }

    /// The pointer's events in order, the panel on screen or stood down for
    /// each, the pointer inside the region throughout: what egui was given
    /// of them, and what the grid was kept from.
    fn feed(steps: &[(Raw, bool)], captured: &mut Option<egui::PointerButton>) -> (Vec<egui::Event>, Vec<bool>) {
        let at = Some(pos2(40.0, 30.0));
        let (mut given, mut ours) = (Vec::new(), Vec::new());
        for (raw, on_screen) in steps {
            let (events, alone) = routed(raw, *on_screen, true, at, PPP, captured);
            given.extend(events);
            ours.push(alone);
        }
        (given, ours)
    }

    fn button(down: bool) -> Raw {
        Raw::Button { button: Button::Left, down }
    }

    fn release_at(event: &egui::Event) -> Option<egui::Pos2> {
        match event {
            egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, .. } => Some(*pos),
            _ => None,
        }
    }

    #[test]
    fn a_release_while_the_panel_stands_down_still_lets_go_of_eguis_press() {
        let mut captured = None;
        let moved = Raw::Move { x: 80.0, y: 60.0 };
        // Pressed on the panel, then an overlay comes up mid-drag: the moves
        // under it are not egui's, and the press is not let go by the
        // stand-down itself.
        let (given, ours) = feed(&[(button(true), true), (moved.clone(), false)], &mut captured);
        assert_eq!(ours, [true, false], "the press is egui's, a move under the overlay the grid's too");
        assert_eq!(given.len(), 1, "only the press: {given:?}");
        assert_eq!(captured, Some(egui::PointerButton::Primary), "the hand still holds it");
        // The release comes under the overlay: egui hears it, where the
        // pointer is, and the grid does not.
        let (given, ours) = feed(&[(button(false), false)], &mut captured);
        assert_eq!(ours, [true]);
        assert_eq!(given.iter().find_map(release_at), Some(pos2(40.0, 30.0)), "{given:?}");
        assert_eq!(captured, None);
        // Nothing held, a release under an overlay is the grid's.
        let (given, ours) = feed(&[(button(false), false)], &mut captured);
        assert!(given.is_empty() && ours == [false], "{given:?}");
    }

    #[test]
    fn a_stand_down_that_ends_first_finds_the_drag_going_on() {
        let mut captured = None;
        let moved = Raw::Move { x: 80.0, y: 60.0 };
        let steps = [(button(true), true), (moved.clone(), false), (moved, true), (button(false), true)];
        let (given, ours) = feed(&steps, &mut captured);
        assert_eq!(ours, [true, false, false, true]);
        assert!(matches!(given[1], egui::Event::PointerMoved(_)), "the drag goes on once the panel is back: {given:?}");
        assert!(release_at(&given[2]).is_some(), "and ends with the hand's release: {given:?}");
        assert_eq!(captured, None);
    }

    #[test]
    fn the_pointer_gone_from_the_window_lets_go_at_once_and_other_buttons_wait() {
        let mut captured = None;
        let right = Raw::Button { button: Button::Right, down: true };
        let (given, ours) = feed(&[(button(true), true), (right, true)], &mut captured);
        assert_eq!(ours, [true, true], "a second button while one is held is the panel's");
        assert_eq!(given.len(), 1, "and egui acts on one: {given:?}");
        let (given, _) = feed(&[(Raw::Leave, false)], &mut captured);
        assert!(given.iter().any(|e| release_at(e).is_some()), "a release that may never come is made now: {given:?}");
        assert!(matches!(given.last(), Some(egui::Event::PointerGone)), "and the pointer gone after it: {given:?}");
        assert_eq!(captured, None);
    }

    #[test]
    fn a_press_dragged_off_its_control_under_an_overlay_is_no_click() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let control = egui::Rect::from_min_size(pos2(30.0, 20.0), egui::vec2(20.0, 20.0));
        let clicks = std::cell::Cell::new(0);
        let run = |events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                if ui.interact(control, egui::Id::new("probe"), egui::Sense::click()).clicked() {
                    clicks.set(clicks.get() + 1);
                }
            });
            out.textures_delta.clear();
        };
        let (on, off) = (pos2(40.0, 30.0), pos2(150.0, 80.0));
        let mut captured = None;
        let mut step = |raw: Raw, on_screen: bool, at: egui::Pos2| {
            let (events, _) = routed(&raw, on_screen, control.contains(at), Some(at), PPP, &mut captured);
            run(events);
        };
        // On screen, pressed and let go on the control: a click, the probe
        // sees it.
        step(Raw::Move { x: 80.0, y: 60.0 }, true, on);
        step(button(true), true, on);
        step(button(false), true, on);
        assert_eq!(clicks.get(), 1, "the probe's own click");
        // Pressed on it, an overlay up, the pointer far off it under the
        // overlay (egui shown none of it), let go there: no click.
        step(button(true), true, on);
        step(Raw::Move { x: 300.0, y: 160.0 }, false, off);
        step(button(false), false, off);
        assert_eq!(clicks.get(), 1, "a press dragged off its control is no click");
        assert!(!ctx.input(|i| i.pointer.any_down()), "and egui holds no button");
    }

    #[test]
    fn an_egui_caret_lifts_to_the_cell_under_it() {
        // The room's cells under its first row at 100×30, and the same
        // region in points at 8×16 points a cell, starting at 136,32.
        let cells = Rect::new(17, 2, 83, 22);
        let points = egui::Rect::from_min_size(egui::pos2(136.0, 32.0), egui::vec2(83.0 * 8.0, 22.0 * 16.0));
        assert_eq!(cell_under(egui::pos2(136.0, 32.0), points, cells), Position { x: 17, y: 2 });
        assert_eq!(cell_under(egui::pos2(136.0 + 8.0 * 10.0 + 3.0, 32.0 + 16.0 * 5.0 + 15.9), points, cells), Position { x: 27, y: 7 });
        // A point on the region's far edge is in its last cell, never past.
        assert_eq!(cell_under(points.max, points, cells), Position { x: 99, y: 23 });
        assert_eq!(cell_under(egui::pos2(0.0, 0.0), points, cells), Position { x: 17, y: 2 }, "before it, its first");
        // A caret whose foot is the top edge of row 8 stands in row 7.
        let foot = egui::pos2(136.0 + 8.0 * 10.0 + 3.0, 32.0 + 16.0 * 6.0);
        assert_eq!(caret_cell(foot, points, cells), Position { x: 27, y: 7 }, "the caret's own row, not the one under it");
        assert_eq!(caret_cell(foot + egui::vec2(0.0, 1.0), points, cells), Position { x: 27, y: 8 });
    }

    #[test]
    fn egui_holds_no_button_once_it_hears_the_forwarded_release() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let run = |events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |_| {});
            out.textures_delta.clear();
        };
        let mut captured = None;
        let (pressed, _) = feed(&[(button(true), true)], &mut captured);
        run(pressed);
        assert!(ctx.input(|i| i.pointer.primary_down()), "egui holds the press");
        let (released, _) = feed(&[(button(false), false)], &mut captured);
        run(released);
        assert!(!ctx.input(|i| i.pointer.any_down()), "and lets it go with the hand");
    }
}
