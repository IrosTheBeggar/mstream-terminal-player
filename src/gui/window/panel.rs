//! What of the window egui draws rather than the cells: the egui spike's
//! seam. Covers (covers.rs) put pictures over cells the drawing path left
//! blank; a panel puts egui's paint there, in two layers, each with a
//! context of its own:
//!
//! - the PAGE layer: a page hosted in a region of the grid (the Stats
//!   page, the MP3 Player's, the Admin rooms and the log, the setup wizard
//!   over the whole grid), its pass run mid-draw where the face draws the
//!   page ([`Panel::show`]). egui is told the region IS its screen, so
//!   everything it draws, popups included, stays inside the cells the page
//!   owns; and any cells overlay that touches the region (a modal, the
//!   cells' server menu, a tooltip) stands the whole layer down.
//! - the CHROME layer: the GUI's own top strip, transport bar and their
//!   popups (src/gui/chrome.rs), whose screen is the whole window, so a
//!   popup goes anywhere. Its one pass runs at the very end of the face's
//!   draw ([`Panel::show_chrome`]), once the cells have registered all that
//!   stands over it; it stays painted under the cells' overlays, each part
//!   cut around those the cells stacked over it ([`Part`]).
//!
//! Three hands meet here, all on the window's thread:
//!
//! - the host's (mod.rs): winit's events, fed to egui while a layer is on
//!   screen. The router ([`Panel::route`]) gives the page the pointer inside
//!   its region — a press there, its drag and its release, the wheel — and
//!   the chrome the primary presses on the cells it claims ([`Claims`]);
//!   everything else still goes to the grid as before, the keys above all,
//!   so a page keeps its keymap and the GUI's own keys still switch rooms.
//! - the face's: while it draws, each layer's pass runs and tessellates;
//!   the face leaves those cells blank, as it does under a cover.
//! - the post-processor's: after the text blit and the covers, the page's
//!   paint then the chrome's, in one pass of one renderer ([`Painter`],
//!   from `CoverPost::process`).
//!
//! A layer's place is the cell rect's share of the grid, which is its share
//! of the surface (covers.rs says why), in egui's points. An input method
//! never reaches egui, so a panel has no text fields of egui's own.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive, TextureId};
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

/// The grid in egui's points as the chrome's pass holds it, its screen
/// the whole window: where any cell stands. A cell is the window's share
/// of the grid, 8 by 16 points at the window's opening size but not
/// always; `ppp` is the pixels in a point, which the edges round to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cells {
    pub points: egui::Rect,
    pub grid: Rect,
    pub ppp: f32,
}

impl Cells {
    /// A cell's size in points.
    pub(crate) fn cell(&self) -> egui::Vec2 {
        egui::vec2(
            self.points.width() / f32::from(self.grid.width.max(1)),
            self.points.height() / f32::from(self.grid.height.max(1)),
        )
    }

    /// `cells` in points, each edge on the whole pixel [`Panel::show`]
    /// rounds a region's edge to, so the strip's foot is row 1's top and a
    /// part meets the page beside it with no seam.
    pub(crate) fn rect(&self, cells: Rect) -> egui::Rect {
        self.edges(cells, f32::round, f32::round)
    }

    /// `cells` in points with its edges rounded out to whole pixels: a hole
    /// cut for the cells' own drawing never leaves a sliver of it covered.
    pub(crate) fn hole(&self, cells: Rect) -> egui::Rect {
        self.edges(cells, f32::floor, f32::ceil)
    }

    fn edges(&self, cells: Rect, low: fn(f32) -> f32, high: fn(f32) -> f32) -> egui::Rect {
        let ppp = self.ppp.max(f32::MIN_POSITIVE);
        let along = |c: u16, from: u16, n: u16, origin: f32, span: f32, round: fn(f32) -> f32| {
            let share = f32::from(c.saturating_sub(from)) / f32::from(n.max(1));
            origin + round(share * span * ppp) / ppp
        };
        let (g, p) = (self.grid, self.points);
        egui::Rect::from_min_max(
            pos2(along(cells.x, g.x, g.width, p.min.x, p.width(), low), along(cells.y, g.y, g.height, p.min.y, p.height(), low)),
            pos2(
                along(cells.right(), g.x, g.width, p.min.x, p.width(), high),
                along(cells.bottom(), g.y, g.height, p.min.y, p.height(), high),
            ),
        )
    }

    /// The cell under `at`.
    pub(crate) fn under(&self, at: egui::Pos2) -> Position {
        cell_under(at, self.points, self.grid)
    }
}

/// Where the chrome takes the pointer, from its pass until the next, in
/// cells of the grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Claims {
    /// The cells where a primary press is the chrome's: the strip's live
    /// runs of row 0, and while a popup is up the whole grid less the cells
    /// of what the cells stacked over it.
    pub live: Vec<Rect>,
    /// A popup holds the pointer: the page layer hears none of it, and the
    /// hand is the chrome's alone.
    pub popup: bool,
}

impl Claims {
    /// Whether a press on `cell` is the chrome's.
    pub(crate) fn holds(&self, cell: Position) -> bool {
        self.live.iter().any(|rect| rect.contains(cell))
    }
}

/// A part of the chrome's paint — the strip, the bar, a popup, a tooltip —
/// by where it stands, in points, and the rects in points cut out of it
/// (what the cells stacked over that part this frame, [`Cells::hole`]). A
/// primitive belongs to the part whose rect holds its clip rect.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Part {
    pub rect: egui::Rect,
    pub holes: Vec<egui::Rect>,
}

/// What the chrome's pass drew, for the face's frame: whether the pointer
/// is over something it would act on, on a cell the chrome claims.
pub(crate) struct ChromeShown {
    pub hand: bool,
}

/// The panel as the host and the face share it.
pub(crate) struct Panel {
    board: Arc<Board>,
    live: Mutex<Live>,
}

/// One egui layer: its context and winit state, the button a press held
/// for it, and when it asked to run again.
struct Layer {
    /// Made with the panel, styled and given its faces on the first pass.
    ctx: egui::Context,
    fonts_set: bool,
    state: Option<egui_winit::State>,
    /// A press this layer took, of this button: its drag and release are
    /// the layer's too, and egui holds the button down until it hears the
    /// release.
    captured: Option<egui::PointerButton>,
    /// Events egui has been given since its last pass.
    pending: bool,
    /// When egui asked to run again (an animation, a tooltip's delay).
    repaint_at: Option<Instant>,
}

impl Layer {
    fn new() -> Layer {
        Layer { ctx: egui::Context::default(), fonts_set: false, state: None, captured: None, pending: false, repaint_at: None }
    }

    /// Events for egui's next pass, and a pass owed.
    fn hear(&mut self, events: Vec<egui::Event>) {
        if events.is_empty() {
            return;
        }
        if let Some(state) = self.state.as_mut() {
            state.egui_input_mut().events.extend(events);
            self.pending = true;
        }
    }

    /// Whether this layer's next pass is owed now.
    fn owed(&self) -> bool {
        self.pending || self.repaint_at.is_some_and(|at| at <= Instant::now())
    }
}

struct Live {
    page: Layer,
    chrome: Layer,
    window: Option<Arc<Window>>,
    /// The page's region in physical pixels, as its last pass placed it.
    region: Option<egui::Rect>,
    /// The chrome's screen as its last pass placed it: the grid, and the
    /// window in physical pixels.
    screen: Option<(Rect, egui::Rect)>,
    /// What the chrome's last pass claims of the pointer.
    claims: Claims,
    /// A press the grid took, of this button: the chrome takes nothing
    /// until its release.
    grid_held: Option<egui::PointerButton>,
    /// The pointer, in physical pixels, as winit last placed it.
    cursor: Option<(f32, f32)>,
    /// The areas the chrome's last pass showed: one new in a pass was laid
    /// out unseen in it, and the next pass is owed at once.
    areas: HashSet<egui::LayerId>,
}

/// The top bit of a texture id the chrome's paint is moved to: its
/// context numbers its textures from `Managed(0)` as the page's does, and
/// one renderer draws both ([`remapped`]). No id the renderer hands out
/// for a native texture of its own reaches it.
const CHROME_TEXTURES: u64 = 1 << 62;

impl Panel {
    pub(super) fn new(board: Arc<Board>) -> Panel {
        let live = Live {
            page: Layer::new(),
            chrome: Layer::new(),
            window: None,
            region: None,
            screen: None,
            claims: Claims::default(),
            grid_held: None,
            cursor: None,
            areas: HashSet::new(),
        };
        Panel { board, live: Mutex::new(live) }
    }

    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The window, once it exists: egui's input is read against it. The
    /// faces the passes set their words in are read meanwhile, off the
    /// window's thread, so the first frame finds them read.
    pub(super) fn attach(&self, window: Arc<Window>) {
        let mut live = self.lock();
        let Live { page, chrome, .. } = &mut *live;
        for layer in [page, chrome] {
            let state = egui_winit::State::new(
                layer.ctx.clone(),
                egui::ViewportId::ROOT,
                &*window,
                Some(window.scale_factor() as f32),
                window.theme(),
                None,
            );
            layer.state = Some(state);
        }
        live.window = Some(window);
        let lang = rust_i18n::locale().to_string();
        let _ = std::thread::Builder::new().name("window-faces".into()).spawn(move || {
            let _ = definitions(&lang);
        });
    }

    /// A window attached, of some size: a pass would run.
    pub(crate) fn ready(&self) -> bool {
        let live = self.lock();
        let size = live.window.as_ref().map(|window| window.inner_size());
        live.page.state.is_some() && size.is_some_and(|size| size.width > 0 && size.height > 0)
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
        let page_on = self.board.panel_on_screen();
        let chrome_on = self.board.chrome_on_screen();
        let mut live = self.lock();
        let Live { page, chrome, window, cursor, grid_held, .. } = &mut *live;
        let Some(window) = window.as_deref() else {
            page.captured = None;
            chrome.captured = None;
            *grid_held = None;
            return;
        };
        // The window lost the keyboard mid-press: the release may never
        // come, so each layer is told the button let go where the pointer
        // stands, or its drag would hold until the next press.
        if matches!(event, WindowEvent::Focused(false)) {
            *grid_held = None;
            for (layer, on) in [(&mut *page, page_on), (&mut *chrome, chrome_on)] {
                let ppp = window.scale_factor() as f32 * layer.ctx.zoom_factor();
                let at = cursor.map(|(x, y)| pos2(x, y) / ppp);
                let release = let_go(&mut layer.captured, at, !on);
                layer.hear(release);
            }
        }
        for layer in [page, chrome] {
            if let Some(state) = layer.state.as_mut() {
                let _ = state.on_window_event(window, event);
            }
        }
    }

    /// The pointer, as the window hands it to the grid (input.rs's [`Raw`],
    /// a person's or the script's), egui's to see while a layer is on
    /// screen ([`route_both`]). True when it is egui's alone and the grid
    /// must not see it: a press a layer took, the release of a press one
    /// took, the wheel over the page. A move goes to the grid too, so the
    /// GUI's hovers let go.
    pub(super) fn route(&self, raw: &Raw) -> bool {
        let page_on = self.board.panel_on_screen();
        let chrome_on = self.board.chrome_on_screen();
        let mut live = self.lock();
        // The pointer's place is kept while the layers stand down too (an
        // overlay over the page, or no layer up): a press that comes after
        // with no move between, on a layer back on screen, lands where it is
        // and not where the pointer stood before the stand-down.
        if let Raw::Move { x, y } = raw {
            live.cursor = Some((*x as f32, *y as f32));
        }
        let Live { page, chrome, window, region, screen, claims, grid_held, cursor, .. } = &mut *live;
        let Some(window) = window.as_deref() else { return false };
        if page.state.is_none() {
            return false;
        }
        let ppp = window.scale_factor() as f32 * page.ctx.zoom_factor();
        let pointer = cursor.map(|(x, y)| pos2(x, y));
        let in_page = match (*region, pointer) {
            (Some(region), Some(at)) => region.contains(at),
            _ => false,
        };
        let claimed = match (*screen, pointer) {
            (Some((grid, px)), Some(at)) if px.contains(at) => claims.holds(cell_under(at, px, grid)),
            _ => false,
        };
        let seat = Seat { page_on, chrome_on, in_page, claimed, popup: chrome_on && claims.popup, at: pointer.map(|at| at / ppp), ppp };
        let mut holds = Holds { page: page.captured, chrome: chrome.captured, grid: *grid_held };
        let routed = route_both(raw, seat, &mut holds);
        (page.captured, chrome.captured, *grid_held) = (holds.page, holds.chrome, holds.grid);
        page.hear(routed.page);
        chrome.hear(routed.chrome);
        routed.ours
    }

    /// The face's frame must run its passes though the player has nothing
    /// new: a layer on screen was given input, or asked to run again by
    /// now.
    pub(super) fn wants_pass(&self) -> bool {
        let page_on = self.board.panel_on_screen();
        let chrome_on = self.board.chrome_on_screen();
        let live = self.lock();
        (page_on && live.page.owed()) || (chrome_on && live.chrome.owed())
    }

    /// When egui wants its next pass, of a layer on screen.
    pub(super) fn wake_at(&self) -> Option<Instant> {
        let page_on = self.board.panel_on_screen();
        let chrome_on = self.board.chrome_on_screen();
        let live = self.lock();
        let page = live.page.repaint_at.filter(|_| page_on);
        let chrome = live.chrome.repaint_at.filter(|_| chrome_on);
        page.into_iter().chain(chrome).min()
    }

    /// One egui pass of the page layer over `area`, a cell rect of `grid`
    /// the face leaves blank this frame, with `run` drawing into it; its
    /// paint waits on the board for the post-processor. None before there
    /// is a window, when the face draws the cells itself.
    pub(crate) fn show(&self, area: Rect, grid: Rect, run: &mut dyn FnMut(&mut egui::Ui)) -> Option<Shown> {
        let mut live = self.lock();
        let Live { page, window, region, .. } = &mut *live;
        if !page.fonts_set {
            fonts(&page.ctx);
            page.fonts_set = true;
        }
        let Layer { ctx, state, pending, repaint_at, .. } = page;
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

    /// The chrome layer's pass for this frame, over the whole window, the
    /// `grid` its screen: `run` draws it once (egui may run it again) on a
    /// `Ui` the whole window, handed where the cells stand ([`Cells`]),
    /// and says where each part of its paint stands with what to cut out of
    /// it ([`Part`]); a primitive in none of them is cut out of `every`, the
    /// safe side. From this pass to the next, a primary press on the cells
    /// `claims` holds is the chrome's ([`Panel::route`]). The paint waits
    /// on the board for the post-processor, after the page's. None before
    /// there is a window, when the face draws the chrome in cells.
    pub(crate) fn show_chrome(
        &self,
        grid: Rect,
        claims: Claims,
        every: &[Rect],
        run: &mut dyn FnMut(&mut egui::Ui, Cells) -> Vec<Part>,
    ) -> Option<ChromeShown> {
        let page_on = self.board.panel_on_screen();
        let mut live = self.lock();
        let Live { page, chrome, window, screen, claims: kept, cursor, areas, .. } = &mut *live;
        if !chrome.fonts_set {
            fonts(&chrome.ctx);
            chrome.fonts_set = true;
        }
        let (Some(state), Some(window)) = (chrome.state.as_mut(), window.clone()) else { return None };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 || grid.width == 0 || grid.height == 0 {
            return None;
        }
        let px = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size.width as f32, size.height as f32));
        let ppp = window.scale_factor() as f32 * chrome.ctx.zoom_factor();
        let cells = Cells { points: egui::Rect::from_min_max(px.min / ppp, px.max / ppp), grid, ppp };
        let mut input = state.take_egui_input(&window);
        input.screen_rect = Some(cells.points);
        let mut parts = Vec::new();
        let mut output = chrome.ctx.run_ui(input, |ui| parts = run(ui, cells));
        chrome.pending = false;
        let shown = chrome.ctx.memory(|memory| memory.areas().visible_layer_ids());
        let appeared = shown.iter().any(|layer| !areas.contains(layer));
        *areas = shown.into_iter().collect();
        let delay = output.viewport_output.get(&egui::ViewportId::ROOT).map(|viewport| viewport.repaint_delay);
        chrome.repaint_at = chrome_repaint(delay, appeared).map(|delay| Instant::now() + delay);
        // egui shows the hand over its own rows wherever they are; the cells
        // under a hole cut for an overlay of theirs have no control of the
        // chrome's, so the hand stops at what the chrome claims.
        let at = cursor.map(|(x, y)| pos2(x, y)).filter(|at| px.contains(*at));
        let claimed = at.is_some_and(|at| claims.holds(cell_under(at, px, grid)));
        let hand = claimed && output.platform_output.cursor_icon == egui::CursorIcon::PointingHand;
        let jobs = chrome.ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let every: Vec<egui::Rect> = every.iter().map(|rect| cells.hole(*rect)).collect();
        let jobs = holed(jobs, &parts, &every);
        // A popup that comes up takes the pointer off the page: egui's last
        // hover there lets go.
        if claims.popup && !kept.popup && page_on {
            page.hear(vec![egui::Event::PointerGone]);
        }
        *kept = claims;
        *screen = Some((grid, px));
        let paint = remapped(Paint::pass(jobs, output.pixels_per_point, output.textures_delta));
        self.board.chrome_drawn(grid, paint);
        Some(ChromeShown { hand })
    }
}

/// When the chrome's next pass is owed, after one whose egui asked for one
/// after `delay`: a delay egui gave is kept (a tooltip's dwell), and the
/// immediate pass it asks after every pass that had input is not, since a
/// move draws a frame of the GUI's anyway and egui hit-tests it against
/// this pass's paint; but where an area `appeared` in this pass, which egui
/// laid out unseen, the next pass is owed at once, to show it.
fn chrome_repaint(delay: Option<Duration>, appeared: bool) -> Option<Duration> {
    if appeared {
        return Some(Duration::ZERO);
    }
    delay.filter(|delay| !delay.is_zero() && *delay < Duration::from_secs(60))
}

/// `jobs`, each primitive cut out of the holes of the part it belongs to:
/// where its clip rect meets a hole, the primitive is drawn once for each
/// piece of its clip rect left round the holes, and not at all where none
/// is left. It belongs to the smallest of `parts` whose rect holds its clip
/// rect, and to none, the safe side, takes `every` hole.
fn holed(jobs: Vec<ClippedPrimitive>, parts: &[Part], every: &[egui::Rect]) -> Vec<ClippedPrimitive> {
    let mut out = Vec::with_capacity(jobs.len());
    for job in jobs {
        let clip = job.clip_rect;
        let holds = |rect: &egui::Rect| rect.expand(0.01).contains_rect(clip);
        let part = parts.iter().filter(|part| holds(&part.rect)).min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()));
        let holes = part.map_or(every, |part| &part.holes[..]);
        let meets: Vec<egui::Rect> = holes.iter().copied().filter(|hole| hole.intersects(clip) && hole.intersect(clip).is_positive()).collect();
        if meets.is_empty() {
            out.push(job);
            continue;
        }
        for piece in without(clip, &meets) {
            out.push(ClippedPrimitive { clip_rect: piece, primitive: job.primitive.clone() });
        }
    }
    out
}

/// `rect` less `holes`: the pieces of it no hole covers, none overlapping.
fn without(rect: egui::Rect, holes: &[egui::Rect]) -> Vec<egui::Rect> {
    let mut pieces = vec![rect];
    for hole in holes {
        let mut next = Vec::with_capacity(pieces.len() + 3);
        for piece in pieces {
            let cut = piece.intersect(*hole);
            if !cut.is_positive() {
                next.push(piece);
                continue;
            }
            let band = |min: egui::Pos2, max: egui::Pos2| egui::Rect::from_min_max(min, max);
            if cut.min.y > piece.min.y {
                next.push(band(piece.min, pos2(piece.max.x, cut.min.y)));
            }
            if cut.max.y < piece.max.y {
                next.push(band(pos2(piece.min.x, cut.max.y), piece.max));
            }
            if cut.min.x > piece.min.x {
                next.push(band(pos2(piece.min.x, cut.min.y), pos2(cut.min.x, cut.max.y)));
            }
            if cut.max.x < piece.max.x {
                next.push(band(pos2(cut.max.x, cut.min.y), pos2(piece.max.x, cut.max.y)));
            }
        }
        pieces = next;
    }
    pieces
}

/// The chrome's paint with its textures moved to ids of their own
/// ([`CHROME_TEXTURES`]): its deltas, its frees and its meshes alike, so
/// one renderer holds both layers' textures and the page's are never
/// touched.
fn remapped(mut paint: Paint) -> Paint {
    let remap = |id: TextureId| match id {
        TextureId::Managed(n) => TextureId::User(CHROME_TEXTURES | n),
        user => user,
    };
    for (id, _) in &mut paint.set {
        *id = remap(*id);
    }
    for id in &mut paint.free {
        *id = remap(*id);
    }
    for job in paint.jobs.iter_mut().flatten() {
        if let Primitive::Mesh(mesh) = &mut job.primitive {
            mesh.texture_id = remap(mesh.texture_id);
        }
    }
    paint
}

/// Where the pointer is, as the router reads it for one event.
#[derive(Clone, Copy, Debug)]
struct Seat {
    /// The page layer is on screen and painted (`Board::panel_on_screen`).
    page_on: bool,
    /// The chrome layer is on screen (`Board::chrome_on_screen`).
    chrome_on: bool,
    /// The pointer is inside the page's region.
    in_page: bool,
    /// The pointer's cell is one the chrome claims.
    claimed: bool,
    /// A chrome popup holds the pointer.
    popup: bool,
    /// The pointer, in egui's points.
    at: Option<egui::Pos2>,
    ppp: f32,
}

/// The button a press held for each of the two layers, or for the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Holds {
    page: Option<egui::PointerButton>,
    chrome: Option<egui::PointerButton>,
    grid: Option<egui::PointerButton>,
}

/// What each layer is given of one of the pointer's events, and whether
/// it was egui's alone.
#[derive(Debug, Default)]
struct Routed {
    page: Vec<egui::Event>,
    chrome: Vec<egui::Event>,
    ours: bool,
}

/// One of the pointer's events, routed between the page layer, the chrome
/// layer and the grid. In order:
///
/// 1. A held button stays with whoever took its press, until its release:
///    the chrome, the page (the page's own rules, [`routed`]), or the grid,
///    which the chrome then leaves alone: a page's press dragged onto row 0
///    stays the page's, and a grid press whose release lands after a chrome
///    popup came up is still the grid's, so its grip or bar hears the Up.
/// 2. A primary press is the chrome's where the chrome is on screen,
///    nothing holds a button and the press's cell is one it claims.
/// 3. Otherwise it is the page's by the page's rules, which hears nothing
///    while a chrome popup holds the pointer; else the grid's.
/// 4. A secondary or middle press and the wheel are never the chrome's.
/// 5. A move reaches the chrome unless the page or the grid holds a button
///    (the chrome heard the pointer go at that press, and hears where it
///    rests at the release that lets go of the hold), and the page as its
///    rules say unless a popup holds the pointer or the chrome holds a
///    button; the grid always hears it.
/// 6. The pointer leaving the window lets go of every hold.
fn route_both(raw: &Raw, seat: Seat, holds: &mut Holds) -> Routed {
    let page_on = seat.page_on && !seat.popup;
    let page = |raw: &Raw, holds: &mut Holds| routed(raw, page_on, seat.in_page, seat.at, seat.ppp, &mut holds.page);
    let gone = || if seat.chrome_on { vec![egui::Event::PointerGone] } else { Vec::new() };
    let mut out = Routed::default();
    match raw {
        Raw::Leave => {
            (out.page, _) = page(raw, holds);
            out.chrome = let_go(&mut holds.chrome, seat.at, !seat.chrome_on);
            out.chrome.extend(gone());
            holds.grid = None;
        }
        Raw::Button { button, down } => {
            let pressed = pointer_button(button);
            if holds.chrome.is_some() {
                if !*down && holds.chrome == Some(pressed) {
                    out.chrome = let_go(&mut holds.chrome, seat.at, !seat.chrome_on);
                }
                out.ours = true;
                return out;
            }
            let free = holds.page.is_none() && holds.grid.is_none();
            if *down && pressed == egui::PointerButton::Primary && free && seat.chrome_on && seat.claimed {
                if let Some(pos) = seat.at {
                    holds.chrome = Some(pressed);
                    out.chrome = vec![egui::Event::PointerButton { pos, button: pressed, pressed: true, modifiers: egui::Modifiers::default() }];
                    if page_on {
                        out.page = vec![egui::Event::PointerGone];
                    }
                    out.ours = true;
                    return out;
                }
            }
            let held = holds.page.is_some() || holds.grid.is_some();
            (out.page, out.ours) = page(raw, holds);
            if *down {
                if !out.ours {
                    holds.grid.get_or_insert(pressed);
                }
                // Another layer's press: nothing of the chrome lights while
                // it lasts.
                if !held {
                    out.chrome = gone();
                }
            } else {
                if !out.ours && holds.grid == Some(pressed) {
                    holds.grid = None;
                }
                // The press that held it is let go: the chrome hears where
                // the pointer rests, which may be on what that press opened
                // (quick config over the split's chevron), and lights it
                // and shows the hand without waiting for a move.
                if held && holds.page.is_none() && holds.grid.is_none() && seat.chrome_on {
                    out.chrome = seat.at.map(egui::Event::PointerMoved).into_iter().collect();
                }
            }
        }
        Raw::Move { .. } => {
            if holds.chrome.is_none() {
                (out.page, _) = page(raw, holds);
            }
            if seat.chrome_on && holds.page.is_none() && holds.grid.is_none() {
                out.chrome = seat.at.map(egui::Event::PointerMoved).into_iter().collect();
            }
        }
        _ => (out.page, out.ours) = page(raw, holds),
    }
    out
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

/// egui's paint on the backend's device, owned by the post-processor: the
/// page layer's, then the chrome's over it, through one renderer (the
/// chrome's textures moved to ids of their own, [`remapped`]). The renderer
/// is made on the first pass there is something to paint, so a window that
/// never shows egui never compiles its pipeline.
#[derive(Default)]
pub(crate) struct Painter {
    renderer: Option<egui_wgpu::Renderer>,
    /// Each layer's last pass's shapes: painted again on every process,
    /// since the text blit before them paints the whole surface.
    page: Vec<ClippedPrimitive>,
    chrome: Vec<ClippedPrimitive>,
    pixels_per_point: f32,
    /// What one pass paints: the page's shapes while it is not stood down,
    /// then the chrome's; made again when either changes.
    jobs: Vec<ClippedPrimitive>,
    page_shown: bool,
    stale: bool,
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
        let (mut set, mut free, page_shown) = {
            let mut paint = board.panel();
            if let Some(jobs) = paint.jobs.take() {
                self.page = jobs;
                self.pixels_per_point = paint.pixels_per_point;
                self.stale = true;
            }
            if paint.area.is_none() && !self.page.is_empty() {
                self.page.clear();
                self.stale = true;
            }
            (std::mem::take(&mut paint.set), std::mem::take(&mut paint.free), !paint.under)
        };
        {
            let mut paint = board.chrome();
            if let Some(jobs) = paint.jobs.take() {
                self.chrome = jobs;
                self.pixels_per_point = paint.pixels_per_point;
                self.stale = true;
            }
            if paint.area.is_none() && !self.chrome.is_empty() {
                self.chrome.clear();
                self.stale = true;
            }
            set.append(&mut paint.set);
            free.append(&mut paint.free);
        }
        if page_shown != self.page_shown {
            self.page_shown = page_shown;
            self.stale = true;
        }
        if std::mem::take(&mut self.stale) {
            let page = self.page.iter().filter(|_| page_shown);
            self.jobs = page.chain(&self.chrome).cloned().collect();
        }
        if set.is_empty() && free.is_empty() && self.jobs.is_empty() {
            return;
        }
        let renderer = self.renderer.get_or_insert_with(|| {
            egui_wgpu::Renderer::new(device, surface_config.format, egui_wgpu::RendererOptions::default())
        });
        for (id, delta) in &set {
            renderer.update_texture(device, queue, *id, delta);
        }
        if !self.jobs.is_empty() {
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
    ctx.set_fonts(definitions(lang));
}

/// [`install_fonts`]'s faces as egui is given them: the system's borrowed
/// from the shared copy read once for the process ([`readable`]), so the
/// page's context and the chrome's, and a page's faces set again, hold one
/// copy of each between them.
fn definitions(lang: &str) -> FontDefinitions {
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
    fonts
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

/// The faces read from disk, once each for the process's life, by path:
/// their bytes leaked, so egui borrows them (`FontData::from_static`),
/// where it would copy owned bytes into every set of fonts it builds — one
/// copy of a 14 to 20 megabyte CJK collection for each context, and again
/// for each install. At most a UI face's pair and a CJK collection or two
/// are ever read. None for a path that would not read.
static FACES: Mutex<Vec<(PathBuf, Option<&'static [u8]>)>> = Mutex::new(Vec::new());

/// A face egui can read, from `path`'s `index`th, borrowed from the shared
/// copy ([`FACES`]), which the first ask reads; an ask while another
/// thread reads one waits for it.
fn readable(path: &Path, index: u32) -> Option<FontData> {
    let mut faces = FACES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let bytes = match faces.iter().find(|(read, _)| read == path) {
        Some((_, bytes)) => *bytes,
        None => {
            let bytes = std::fs::read(path).ok().map(|bytes| &*Box::leak(bytes.into_boxed_slice()));
            faces.push((path.to_path_buf(), bytes));
            bytes
        }
    }?;
    drop(faces);
    if skrifa::FontRef::from_index(bytes, index).is_err() {
        eprintln!("gui --window: {} is not a face egui can read", path.display());
        return None;
    }
    let mut face = FontData::from_static(bytes);
    face.index = index;
    Some(face)
}

/// Where each platform keeps its UI face, regular and semibold, best first.
fn system_ui_faces() -> (Vec<PathBuf>, Vec<PathBuf>) {
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

    // ── The two layers ──────────────────────────────────────────────────

    /// The pointer on screen with both layers up, inside the page's region
    /// or not, on a cell the chrome claims or not.
    fn seat(in_page: bool, claimed: bool) -> Seat {
        Seat { page_on: true, chrome_on: true, in_page, claimed, popup: false, at: Some(pos2(40.0, 30.0)), ppp: PPP }
    }

    fn right(down: bool) -> Raw {
        Raw::Button { button: Button::Right, down }
    }

    fn pressed_by(events: &[egui::Event]) -> bool {
        events.iter().any(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. }))
    }

    /// A primary press on a cell the chrome claims is the chrome's alone,
    /// and so are its drag's moves and its release; the page hears that the
    /// pointer went; a second button meanwhile is the chrome's to swallow.
    #[test]
    fn a_primary_press_on_a_claimed_cell_is_the_chromes_with_its_release() {
        let mut holds = Holds::default();
        let press = route_both(&button(true), seat(false, true), &mut holds);
        assert!(press.ours && pressed_by(&press.chrome), "{press:?}");
        assert_eq!(holds.chrome, Some(egui::PointerButton::Primary));
        assert!(matches!(press.page[..], [egui::Event::PointerGone]), "the page lets go of its hover: {press:?}");
        let moved = route_both(&Raw::Move { x: 1.0, y: 1.0 }, seat(true, false), &mut holds);
        assert!(!moved.ours && moved.page.is_empty() && !moved.chrome.is_empty(), "the drag is the chrome's, the grid hears it too: {moved:?}");
        let other = route_both(&right(true), seat(true, false), &mut holds);
        assert!(other.ours && other.page.is_empty() && other.chrome.is_empty(), "another button while it is held: {other:?}");
        let release = route_both(&button(false), seat(true, false), &mut holds);
        assert!(release.ours && release.chrome.iter().any(|e| release_at(e).is_some()), "{release:?}");
        assert_eq!(holds, Holds::default());
    }

    /// A press off the chrome's cells goes where it went before: inside the
    /// page's region the page's, outside it the grid's, which the chrome
    /// leaves alone until its release — though the release lands on a cell
    /// the chrome claims, and a press there meanwhile is not the chrome's.
    /// The release gives the chrome back where the pointer rests, no button
    /// with it: quick config, which the split's chevron opens on the grid's
    /// press, showed no hand under a pointer that had not moved since.
    #[test]
    fn a_press_the_grid_took_is_the_grids_until_its_release() {
        let rests = |events: &[egui::Event]| matches!(events, [egui::Event::PointerMoved(at)] if *at == pos2(40.0, 30.0));
        let mut holds = Holds::default();
        let page = route_both(&button(true), seat(true, false), &mut holds);
        assert!(page.ours && pressed_by(&page.page) && holds.page.is_some(), "{page:?}");
        assert!(matches!(page.chrome[..], [egui::Event::PointerGone]), "the chrome lets go of its hover: {page:?}");
        let dragged = route_both(&Raw::Move { x: 1.0, y: 1.0 }, seat(false, true), &mut holds);
        assert!(dragged.chrome.is_empty() && !dragged.page.is_empty(), "a page's drag onto the strip is the page's: {dragged:?}");
        let release = route_both(&button(false), seat(false, true), &mut holds);
        assert!(release.ours && release.page.iter().any(|e| release_at(e).is_some()), "and so is its release: {release:?}");
        assert!(rests(&release.chrome), "the chrome hears where the pointer rests: {release:?}");

        let grid = route_both(&button(true), seat(false, false), &mut holds);
        assert!(!grid.ours && grid.page.is_empty() && holds.grid == Some(egui::PointerButton::Primary), "{grid:?}");
        let moved = route_both(&Raw::Move { x: 1.0, y: 1.0 }, seat(false, true), &mut holds);
        assert!(moved.chrome.is_empty() && !moved.ours, "the chrome hears no move while the grid holds: {moved:?}");
        let other = route_both(&right(true), seat(false, true), &mut holds);
        assert!(other.chrome.is_empty() && !other.ours, "another button meanwhile is the grid's too: {other:?}");
        let other = route_both(&right(false), seat(false, true), &mut holds);
        assert!(other.chrome.is_empty(), "its release leaves the primary's hold: {other:?}");
        let up = route_both(&button(false), seat(false, true), &mut holds);
        assert!(!up.ours && rests(&up.chrome), "the grid's release, on a claimed cell, the pointer back with no button: {up:?}");
        assert_eq!(holds, Holds::default());
        let next = route_both(&button(true), seat(false, true), &mut holds);
        assert!(next.ours && pressed_by(&next.chrome), "free again: {next:?}");
    }

    /// The chrome never takes a secondary or middle press nor the wheel,
    /// claimed cell or not: they go where they went before. While a popup
    /// holds the pointer the page hears none of them, nor the moves.
    #[test]
    fn the_wheel_and_other_buttons_are_never_the_chromes_and_a_popup_quiets_the_page() {
        let mut holds = Holds::default();
        for raw in [right(true), Raw::Wheel(Wheel::Lines(1.0)), Raw::Button { button: Button::Middle, down: true }] {
            let out = route_both(&raw, seat(false, true), &mut holds);
            assert!(out.chrome.iter().all(|e| matches!(e, egui::Event::PointerGone)), "{raw:?}: {out:?}");
            assert!(!out.ours, "{raw:?} is the grid's");
            holds = Holds::default();
        }
        let wheel = route_both(&Raw::Wheel(Wheel::Lines(1.0)), seat(true, false), &mut holds);
        assert!(wheel.ours && !wheel.page.is_empty(), "the page's wheel as before: {wheel:?}");
        let popup = Seat { popup: true, ..seat(true, false) };
        for raw in [Raw::Wheel(Wheel::Lines(1.0)), Raw::Move { x: 1.0, y: 1.0 }, right(true)] {
            let out = route_both(&raw, popup, &mut holds);
            assert!(out.page.is_empty() && !out.ours, "{raw:?} under a popup: {out:?}");
            holds = Holds::default();
        }
        let claimed = Seat { popup: true, ..seat(true, true) };
        let press = route_both(&button(true), claimed, &mut holds);
        assert!(press.ours && pressed_by(&press.chrome) && press.page.is_empty(), "the popup's press: {press:?}");
    }

    /// The chrome off screen (the mini player) takes nothing and hears
    /// nothing, as the page layer off screen; the pointer leaving the
    /// window lets go of every hold.
    #[test]
    fn a_chrome_off_screen_takes_nothing_and_leaving_lets_go_of_everything() {
        let mut holds = Holds::default();
        let off = Seat { chrome_on: false, ..seat(false, true) };
        let press = route_both(&button(true), off, &mut holds);
        assert!(!press.ours && press.chrome.is_empty(), "{press:?}");
        assert!(route_both(&Raw::Move { x: 1.0, y: 1.0 }, off, &mut holds).chrome.is_empty());
        let mut holds = Holds::default();
        route_both(&button(true), seat(false, true), &mut holds);
        let gone = route_both(&Raw::Leave, seat(false, true), &mut holds);
        assert!(gone.chrome.iter().any(|e| release_at(e).is_some()), "a release that may never come is made now: {gone:?}");
        assert!(matches!(gone.chrome.last(), Some(egui::Event::PointerGone)));
        assert_eq!(holds, Holds::default());
    }

    fn mesh_job(clip: egui::Rect) -> ClippedPrimitive {
        ClippedPrimitive { clip_rect: clip, primitive: Primitive::Mesh(egui::Mesh::default()) }
    }

    /// A primitive crossing a hole of its part is drawn in the pieces round
    /// it; one inside a hole is not drawn; one apart from every hole is
    /// drawn as it was; a part's holes are its own, so a popup's primitive
    /// ignores what only the strip is cut around; and a primitive in no
    /// part takes every hole.
    #[test]
    fn the_chromes_paint_is_cut_round_the_holes_of_its_own_part() {
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| egui::Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
        let strip = Part { rect: r(0.0, 0.0, 800.0, 16.0), holes: vec![r(100.0, 0.0, 200.0, 480.0)] };
        let menu = Part { rect: r(500.0, 16.0, 790.0, 200.0), holes: vec![] };
        let every = [r(100.0, 0.0, 200.0, 480.0), r(600.0, 100.0, 700.0, 150.0)];
        let jobs = vec![mesh_job(strip.rect), mesh_job(r(120.0, 2.0, 180.0, 14.0)), mesh_job(r(300.0, 0.0, 400.0, 16.0)), mesh_job(menu.rect), mesh_job(r(0.0, 300.0, 800.0, 480.0))];
        let out = holed(jobs, &[strip.clone(), menu.clone()], &every);
        let clips: Vec<egui::Rect> = out.iter().map(|job| job.clip_rect).collect();
        assert!(clips.contains(&r(0.0, 0.0, 100.0, 16.0)) && clips.contains(&r(200.0, 0.0, 800.0, 16.0)), "the strip round its hole: {clips:?}");
        assert!(!clips.iter().any(|clip| clip.intersects(r(100.5, 0.5, 199.5, 15.5)) && clip.max.y <= 16.0), "nothing of it in the hole: {clips:?}");
        assert!(clips.contains(&r(300.0, 0.0, 400.0, 16.0)), "apart from the hole, whole");
        assert!(clips.contains(&menu.rect), "the menu ignores the overlays under it: {clips:?}");
        let loose: Vec<_> = clips.iter().filter(|clip| clip.min.y >= 300.0).collect();
        assert_eq!(loose.len(), 2, "in no part, cut round every hole that meets it: {loose:?}");
        assert!(!loose.iter().any(|clip| clip.intersects(r(100.5, 300.5, 199.5, 479.5))));
    }

    /// The chrome's textures move to ids of their own, in its deltas, its
    /// frees and its meshes alike, and a page's id never is one of them.
    #[test]
    fn the_chromes_textures_never_land_on_a_pages_ids() {
        let delta = ImageDelta::full(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]), egui::TextureOptions::LINEAR);
        let mut mesh = egui::Mesh::with_texture(TextureId::Managed(0));
        mesh.colored_vertex(pos2(0.0, 0.0), egui::Color32::WHITE);
        let job = ClippedPrimitive { clip_rect: egui::Rect::EVERYTHING, primitive: Primitive::Mesh(mesh) };
        let paint = Paint { jobs: Some(vec![job]), set: vec![(TextureId::Managed(0), delta)], free: vec![TextureId::Managed(3)], ..Paint::default() };
        let paint = remapped(paint);
        let chrome = |n: u64| TextureId::User(CHROME_TEXTURES | n);
        assert_eq!(paint.set[0].0, chrome(0));
        assert_eq!(paint.free, [chrome(3)]);
        let Some(Primitive::Mesh(mesh)) = paint.jobs.as_ref().and_then(|jobs| jobs.first()).map(|job| &job.primitive) else { panic!("a mesh") };
        assert_eq!(mesh.texture_id, chrome(0));
        for n in [0, 1, 7, 1 << 20] {
            assert_ne!(chrome(n), TextureId::Managed(n), "never a page's");
            assert_ne!(chrome(n), TextureId::User(n), "nor a native texture's");
        }
    }

    /// The chrome asks for no pass for the input it just heard: the GUI
    /// draws a frame for every move anyway. It keeps a delay egui gave (a
    /// tooltip's dwell), and asks at once where an area came up unseen.
    #[test]
    fn the_chrome_asks_for_a_pass_only_for_a_dwell_or_an_area_that_came_up() {
        assert_eq!(chrome_repaint(Some(Duration::ZERO), false), None, "the pass after input: none");
        assert_eq!(chrome_repaint(None, false), None);
        assert_eq!(chrome_repaint(Some(Duration::from_millis(500)), false), Some(Duration::from_millis(500)), "the dwell kept");
        assert_eq!(chrome_repaint(Some(Duration::MAX), false), None, "never is never");
        assert_eq!(chrome_repaint(Some(Duration::ZERO), true), Some(Duration::ZERO), "an area laid out unseen");
        // egui's own: a pass that heard a move asks at once, and one that
        // shows an area for the first time sees it among the visible ones.
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let run = |events: Vec<egui::Event>, area: bool| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                if area {
                    egui::Area::new(egui::Id::new("probe-area")).fixed_pos(pos2(10.0, 10.0)).show(ui.ctx(), |ui| ui.label("x"));
                }
            });
            out.textures_delta.clear();
            let visible = ctx.memory(|m| m.areas().visible_layer_ids());
            (out.viewport_output[&egui::ViewportId::ROOT].repaint_delay, visible)
        };
        let (delay, before) = run(vec![egui::Event::PointerMoved(pos2(5.0, 5.0))], false);
        assert_eq!(delay, Duration::ZERO, "egui asks at once after a move");
        let (_, after) = run(Vec::new(), true);
        assert!(after.iter().any(|layer| !before.contains(layer)), "the area is visible in its first pass");
    }

    /// The cells' edges in points: on the pixels a page's region rounds to,
    /// so the strip's foot is row 1's top; a hole's rounded out; the cell
    /// under a point.
    #[test]
    fn the_cells_stand_on_whole_pixels_and_holes_round_out() {
        let cells = Cells { points: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 480.0)), grid: Rect::new(0, 0, 100, 30), ppp: 1.0 };
        assert_eq!(cells.cell(), egui::vec2(8.0, 16.0));
        assert_eq!(cells.rect(Rect::new(0, 0, 100, 1)), egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 16.0)));
        assert_eq!(cells.rect(Rect::new(0, 0, 100, 1)).max.y, cells.rect(Rect::new(0, 1, 100, 1)).min.y);
        // 1003×487 pixels at a scale of 1.25 is no whole number of points a cell.
        let odd = Cells { points: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(802.4, 389.6)), grid: Rect::new(0, 0, 101, 26), ppp: 1.25 };
        let row = odd.rect(Rect::new(3, 5, 7, 2));
        for edge in [row.min.x, row.min.y, row.max.x, row.max.y] {
            assert!(((edge * 1.25) - (edge * 1.25).round()).abs() < 1e-3, "{edge} on a whole pixel");
        }
        let hole = odd.hole(Rect::new(3, 5, 7, 2));
        assert!(hole.contains_rect(row), "{hole:?} round {row:?}");
        assert_eq!(cells.under(pos2(17.0, 33.0)), Position { x: 2, y: 2 });
    }

    /// Two contexts' faces, and a page's set again, borrow one copy of
    /// each face's bytes.
    #[test]
    fn every_context_borrows_one_copy_of_each_face() {
        let (one, two) = (definitions("ja"), definitions("ja"));
        assert_eq!(one.font_data.len(), two.font_data.len());
        for (name, data) in &one.font_data {
            let (std::borrow::Cow::Borrowed(a), std::borrow::Cow::Borrowed(b)) = (&data.font, &two.font_data[name].font) else {
                panic!("{name} is borrowed, never owned");
            };
            assert!(std::ptr::eq(*a, *b), "{name}: one copy");
        }
    }
}
