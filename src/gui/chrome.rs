//! The GUI's own chrome drawn by egui in its window: the top strip (row 0,
//! its tooltips and the server menu), the transport bar and quick config,
//! on the panel's chrome layer (window/panel.rs), whose screen is the whole
//! window and whose one pass runs at the very end of the frame's draw, once
//! the cells have registered everything that stands over it.
//!
//! The Gui's state stays the model, and the keys never reach egui. Each
//! part draws from the view its cells face draws from (`strip::view`,
//! `servers::menu_view`, `bar::layout`, `quick::view`), and what a press on
//! it does is what a press on its cells does, through the same path:
//!
//! - the strip and the popups take the pointer in egui, but only on the
//!   cells the cells' registry leaves them ([`claims`]): a part's slot in
//!   the cells' draw order is marked as the frame draws (`kit::Mark`), and
//!   whatever the cells registered after that mark — a modal's guard, a
//!   whole-screen catcher, a sheet's backdrop — keeps its cells. A press the
//!   chrome takes runs the cells' press preamble, then its act through
//!   `Gui::act`, after the frame's draw ([`after_draw`]).
//! - the bar takes no pointer at all: the cells draw and register it as
//!   always, then blank its rows, and egui paints the very layout they
//!   registered, lit from what the registry says the pointer is over
//!   ([`bar_hot`]).
//!
//! Each part's paint is cut around the overlays the cells registered after
//! its slot ([`holes`]), so a modal stays over the bar and the menu stays
//! over a layer the cells draw under it. Where there is no window yet, it
//! has no size, or `MSTREAM_WINDOW_CELLS=1` set the panel aside, the cells
//! draw every part as they always have ([`faces`]).

use std::time::Instant;

use egui::{Align, Layout, UiBuilder};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};

use super::bar::{self, BarLayout, BarView, Now};
use super::dj::quick::{self, QuickView};
use super::servers::{self, MenuView};
use super::strip::{self, StripView};
use super::window::panel::{Cells, Claims, Part};
use super::window::studio::{self, Pal};
use super::{Act, Faces, Gui, device};
use crate::kit::Mark;

/// The chrome's state on the Gui: where the cells would have drawn each
/// part this frame, what its pass claims of the pointer, and what the pass
/// took that the frame's duties still have to apply.
#[derive(Default)]
pub(crate) struct ChromeUi {
    /// The registry where the strip would have drawn, and whether the
    /// pointer stood then (the Library's modals stash it for the screen).
    strip: Option<(Mark, bool)>,
    /// The bar's registrations this frame, and the layout they registered.
    bar: Option<BarMark>,
    /// The registry at the server menu's slot, and at quick config's, while
    /// each is up.
    menu: Option<Mark>,
    quick: Option<Mark>,
    /// The presses the chrome's passes took since the frame's duties last
    /// ran, each at its cell with the act a face reported for it.
    pub(super) presses: Vec<(Position, Option<Act>)>,
    /// The pointer is over something of the chrome's that a press works.
    pub(super) hand: bool,
    /// What the last pass claimed of the pointer.
    pub(super) claims: Claims,
    /// The bar's control the pointer is over, as the last frame lit it.
    pub(super) hot: Option<(Rect, Act)>,
    /// The chrome's items in cells, in the order they stack, for the
    /// script's dumps (`GuiFace::hit_debug`).
    pub(super) spots: Vec<(Rect, Act)>,
    /// When the last key came: no tooltip of the chrome's shows in the
    /// dwell after one.
    pub(super) key_at: Option<Instant>,
    /// The parts egui draws, whatever the window, for the tests.
    #[cfg(test)]
    pub(super) forced: Option<Faces>,
}

/// The bar as the cells registered it this frame.
pub(super) struct BarMark {
    pub before: Mark,
    pub after: Mark,
    /// The pointer stood as the bar drew (no modal had stashed it).
    pub pointer: bool,
    pub layout: BarLayout,
}

impl ChromeUi {
    /// A frame begins to draw: the last one's marks, items, claims, lit
    /// control and hand are gone, so a frame that draws no chrome (the mini
    /// player, which returns before the host) claims nothing of the pointer
    /// and leaves the hand and the script's dumps to the cells. The presses
    /// wait for the frame's duties.
    pub(super) fn begin_frame(&mut self) {
        self.strip = None;
        self.bar = None;
        self.menu = None;
        self.quick = None;
        self.spots.clear();
        self.hand = false;
        self.claims = Claims::default();
        self.hot = None;
    }
}

/// Which of the chrome's parts egui draws this frame: every one, once the
/// window's panel would run a pass (a window attached, of some size); none
/// without a panel, which `MSTREAM_WINDOW_CELLS=1` sets aside. The strip
/// covers its tooltips and the server menu. A test may force any of them.
pub(super) fn faces(gui: &Gui) -> Faces {
    #[cfg(test)]
    if let Some(forced) = gui.chrome.forced {
        return forced;
    }
    let ready = gui.panel.as_ref().is_some_and(|panel| panel.ready());
    Faces { strip: ready, bar: ready, quick: ready }
}

/// The strip's slot in the cells' draw order, where egui draws it instead.
pub(super) fn strip_mark(gui: &mut Gui) {
    gui.chrome.strip = Some((gui.ui.mark(), gui.ui.pointer.is_some()));
}

/// The bar's registrations, from `before` to now, and the layout the cells
/// drew and registered; `pointer` whether the pointer stood as it drew.
pub(super) fn bar_mark(gui: &mut Gui, before: Mark, pointer: bool, layout: BarLayout) {
    gui.chrome.bar = Some(BarMark { before, after: gui.ui.mark(), pointer, layout });
}

/// The server menu's slot, where the cells would draw the dropdown.
pub(super) fn menu_mark(gui: &mut Gui) {
    gui.chrome.menu = gui.servers.drop_open.then(|| gui.ui.mark());
}

/// Quick config's slot among the DJ's layers.
pub(super) fn quick_mark(gui: &mut Gui) {
    gui.chrome.quick = gui.dj.quick.is_some().then(|| gui.ui.mark());
}

/// What of the grid a primary press is the chrome's, by part.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Live {
    /// Row 0 less what the cells laid over it after the strip's slot.
    pub strip: Vec<Rect>,
    /// While a popup is up, the grid less what the cells stacked over it.
    pub popup: Vec<Rect>,
}

/// Where the chrome takes the pointer this frame, read off the registry at
/// the frame's end ([`Live`]): during an MP3 write nothing (the MP3 tab
/// holds every press, and the cells' strip registers nothing); where a
/// layer dropped the registries since a part's slot, nothing of that part
/// (the cells' part would be gone with them); else the part's cells less
/// those of every click registered after its slot.
pub(super) fn live(gui: &Gui, grid: Rect) -> Live {
    live_with(gui, grid, device::writing(gui))
}

fn live_with(gui: &Gui, grid: Rect, writing: bool) -> Live {
    let mut live = Live::default();
    if writing {
        return live;
    }
    let after = |mark: Mark| gui.ui.clicks_since(mark).iter().map(|(rect, _)| *rect).collect::<Vec<Rect>>();
    let faces = gui.faces;
    if let Some((mark, _)) = gui.chrome.strip.filter(|(mark, _)| faces.strip && gui.ui.kept(*mark)) {
        let row = Rect { x: grid.x, y: grid.y, width: grid.width, height: 1.min(grid.height) };
        live.strip = cut(row, &after(mark));
    }
    let popups = [
        (faces.strip && gui.servers.drop_open, gui.chrome.menu),
        (faces.quick && gui.dj.quick.is_some(), gui.chrome.quick),
    ];
    for (up, mark) in popups {
        if let Some(mark) = mark.filter(|mark| up && gui.ui.kept(*mark)) {
            live.popup.extend(cut(grid, &after(mark)));
        }
    }
    live
}

/// [`live`] as the panel's router holds it.
pub(super) fn claims(live: &Live) -> Claims {
    Claims { live: live.strip.iter().chain(&live.popup).copied().collect(), popup: !live.popup.is_empty() }
}

/// `rect` less every one of `over`: the pieces of it nothing covers.
fn cut(rect: Rect, over: &[Rect]) -> Vec<Rect> {
    let mut pieces = vec![rect];
    for hole in over {
        let mut next = Vec::with_capacity(pieces.len() + 3);
        for piece in pieces {
            let cut = piece.intersection(*hole);
            if cut.is_empty() {
                next.push(piece);
                continue;
            }
            let band = |x: u16, y: u16, right: u16, bottom: u16| Rect { x, y, width: right - x, height: bottom - y };
            if cut.y > piece.y {
                next.push(band(piece.x, piece.y, piece.right(), cut.y));
            }
            if cut.bottom() < piece.bottom() {
                next.push(band(piece.x, cut.bottom(), piece.right(), piece.bottom()));
            }
            if cut.x > piece.x {
                next.push(band(piece.x, cut.y, cut.x, cut.bottom()));
            }
            if cut.right() < piece.right() {
                next.push(band(cut.right(), cut.y, piece.right(), cut.bottom()));
            }
        }
        pieces = next;
    }
    pieces
}

/// The bar's control the pointer is over, which lights: the topmost click
/// under the pointer, when the bar registered it and nothing the cells drew
/// after covers it there; none while a popup holds the pointer, where the
/// pointer did not stand as the bar drew (a modal stashed it), or where a
/// layer dropped the registries since. The bar's presses need nothing of
/// this: they are the cells registry's.
pub(super) fn bar_hot(gui: &Gui, claims: &Claims) -> Option<(Rect, Act)> {
    if claims.popup {
        return None;
    }
    let mark = gui.chrome.bar.as_ref().filter(|mark| mark.pointer)?;
    gui.ui.hit_between(gui.ui.pointer?, mark.before, mark.after)
}

/// The cells each part's paint is cut out of: what the cells registered
/// over it after its slot (a hole each), and for paint in no part every
/// overlay of the frame. Tooltips take none: the cells draw theirs last.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Holes {
    pub strip: Vec<Rect>,
    pub bar: Vec<Rect>,
    pub menu: Vec<Rect>,
    pub quick: Vec<Rect>,
    pub every: Vec<Rect>,
}

/// This frame's [`Holes`]: the overlays registered after each part's slot.
/// While a quit waits for a sync's save the cells write the countdown on
/// the grid's last row (the bar's bottom row with key hints off), so the
/// bar is cut round it too.
pub(super) fn holes(gui: &Gui, grid: Rect) -> Holes {
    let since = |mark: Option<Mark>| mark.map(|mark| gui.ui.overlays_since(mark).to_vec()).unwrap_or_default();
    let mut holes = Holes {
        strip: since(gui.chrome.strip.map(|(mark, _)| mark)),
        bar: since(gui.chrome.bar.as_ref().map(|mark| mark.after)),
        menu: since(gui.chrome.menu),
        quick: since(gui.chrome.quick),
        every: since(Some(Mark::default())),
    };
    if device::quit_words(gui).is_some() && grid.height > 0 {
        let row = Rect { x: grid.x + 1, y: grid.bottom() - 1, width: grid.width.saturating_sub(2), height: 1 };
        holes.bar.push(row);
        holes.every.push(row);
    }
    holes
}

/// What the chrome's pass draws from: each part's view while egui draws
/// it, where a press is the chrome's, and what is cut out of each part.
pub(super) struct Views<'a> {
    pub strip: Option<StripView>,
    pub menu: Option<MenuView>,
    pub bar: Option<(BarView<'a>, &'a BarLayout)>,
    pub hot: Option<(Rect, Act)>,
    pub quick: Option<QuickView>,
    pub live: Live,
    /// The strip lights under the pointer: the pointer stood as the cells
    /// would have drawn it, and no popup holds it.
    pub hover: bool,
    /// The time since the last key.
    pub since_key: Option<std::time::Duration>,
    pub holes: Holes,
}

/// What one run of the chrome's pass gave: the presses it heard, each with
/// the act a face reported for it (egui may run a pass twice, and only the
/// first run hears them, so these add up across runs); and what describes
/// the screen, which is the last run's: the items, the menu's scroll, the
/// parts of the paint, and when a strip tooltip's dwell ends, the pass it
/// is owed then.
#[derive(Default)]
pub(super) struct Out {
    pub presses: Vec<(egui::Pos2, Option<Act>)>,
    pub spots: Vec<(egui::Rect, Act)>,
    pub scroll: Option<usize>,
    pub parts: Vec<Part>,
    pub wake: Option<std::time::Duration>,
}

/// The pass's body on `ui`, the whole window: the strip in row 0, the bar
/// in its rows, each in a child clipped to its cells on the ground, then
/// the open popup over them. The presses are read off the pass's events
/// first (`studio::pass_presses`), as a tap's press and release may come
/// in one pass; each is paired with the act a face reported (none where it
/// pressed nothing, a gap of row 0 or a popup's frame). Testable without a
/// window: a test runs it in `Context::run_ui`.
pub(super) fn draw(ui: &mut egui::Ui, cells: Cells, views: &Views, out: &mut Out) {
    out.spots.clear();
    out.parts.clear();
    out.scroll = None;
    out.wake = None;
    let presses = studio::pass_presses(ui);
    let pal = Pal::now();
    studio::style_once(ui.ctx(), &pal);
    studio::dwell(ui.ctx());
    let points = |rects: &[Rect]| rects.iter().map(|rect| cells.rect(*rect)).collect::<Vec<egui::Rect>>();
    let holes = |rects: &[Rect]| rects.iter().map(|rect| cells.hole(*rect)).collect::<Vec<egui::Rect>>();
    let mut acts: Vec<Act> = Vec::new();
    if let Some(view) = &views.strip {
        let rect = cells.rect(Rect { x: cells.grid.x, y: cells.grid.y, width: cells.grid.width, height: 1 });
        let mut part = part_ui(ui, rect, "chrome-strip");
        let live = points(&views.live.strip);
        let cx = strip::egui_view::StripCtx { cells, live: &live, hover: views.hover, since_key: views.since_key, pal: &pal };
        let shown = strip::egui_view::show(&mut part, view, &cx);
        out.parts.push(Part { rect, holes: holes(&views.holes.strip) });
        if let Some(tip) = shown.tip {
            out.parts.push(Part { rect: tip, holes: Vec::new() });
        }
        out.wake = shown.wake;
        out.spots.extend(shown.items);
        acts.extend(shown.act);
    }
    if let Some((view, layout)) = &views.bar {
        let rect = cells.rect(layout.rect);
        let mut part = part_ui(ui, rect, "chrome-bar");
        let cx = bar::egui_view::BarCtx { cells, hot: views.hot.clone(), pal: &pal };
        bar::egui_view::show(&mut part, view, layout, &cx);
        out.parts.push(Part { rect, holes: holes(&views.holes.bar) });
    }
    let live = points(&views.live.popup);
    if let Some(view) = &views.menu {
        let cx = strip::egui_view::MenuCtx { cells, live: &live, pal: &pal };
        let shown = strip::egui_view::menu(ui, view, &cx);
        if let Some(frame) = shown.frame {
            out.parts.push(Part { rect: frame, holes: holes(&views.holes.menu) });
        }
        out.scroll = shown.scroll;
        out.spots.extend(shown.items);
        acts.extend(shown.act);
    }
    if let Some(view) = &views.quick {
        let cx = quick::egui_view::QuickCtx { cells, live: &live, pal: &pal };
        let shown = quick::egui_view::show(ui, view, &cx);
        if let Some(frame) = shown.frame {
            out.parts.push(Part { rect: frame, holes: holes(&views.holes.quick) });
        }
        out.spots.extend(shown.items);
        acts.extend(shown.act);
    }
    let mut acts = acts.into_iter();
    out.presses.extend(presses.into_iter().map(|at| (at, acts.next())));
}

/// A part's child of `ui` over `rect`, clipped to it, on the ground.
fn part_ui(ui: &mut egui::Ui, rect: egui::Rect, salt: &str) -> egui::Ui {
    let mut part = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(Layout::top_down(Align::Min)));
    part.set_clip_rect(rect.intersect(ui.clip_rect()));
    let _ = studio::begin(&part);
    part
}

/// `rect`, in points, as the cells it covers.
fn cells_of(cells: Cells, rect: egui::Rect) -> Rect {
    let first = cells.under(rect.min);
    let last = cells.under(rect.max - egui::vec2(0.01, 0.01));
    Rect { x: first.x, y: first.y, width: last.x.saturating_sub(first.x) + 1, height: last.y.saturating_sub(first.y) + 1 }
}

/// The views of this frame, from the Gui as it stands after the cells drew.
fn views<'a>(gui: &'a Gui, grid: Rect, now: Option<&'a Now>, live: Live, hot: Option<(Rect, Act)>, holes: Holes) -> Views<'a> {
    let faces = gui.faces;
    let bar = gui.chrome.bar.as_ref().filter(|_| faces.bar).map(|mark| {
        let mut view = super::bar_view(gui, now);
        view.art = super::playing_art(&gui.app);
        (view, &mark.layout)
    });
    let popup = !live.popup.is_empty();
    Views {
        strip: faces.strip.then(|| strip::view(gui)),
        menu: if faces.strip { servers::menu_view(gui) } else { None },
        bar,
        hot,
        quick: if faces.quick { quick::view(gui, grid) } else { None },
        hover: gui.chrome.strip.is_some_and(|(_, pointer)| pointer) && !popup,
        live,
        since_key: gui.chrome.key_at.map(|at| at.elapsed()),
        holes,
    }
}

/// The chrome's pass at the end of the frame's draw: what it claims of
/// the pointer, what lights on the bar and what each part is cut round,
/// read off the registry whether or not there is a window to draw in;
/// then, where egui draws a part and the panel has a window, the pass,
/// whose presses wait for the frame's duties ([`after_draw`]).
pub(super) fn host(frame: &Frame, gui: &mut Gui) {
    let grid = frame.area();
    let live = live(gui, grid);
    let claims = claims(&live);
    let hot = bar_hot(gui, &claims);
    let holes = holes(gui, grid);
    gui.chrome.claims = claims.clone();
    gui.chrome.hot = hot.clone();
    let faces = gui.faces;
    if !(faces.strip || faces.bar || faces.quick) {
        return;
    }
    let Some(panel) = gui.panel.clone() else { return };
    let now = gui.bar_now();
    let mut presses: Vec<(Position, Option<Act>)> = Vec::new();
    let mut spots: Vec<(Rect, Act)> = Vec::new();
    let mut scroll = None;
    let shown = {
        let views = views(gui, grid, now.as_ref(), live, hot, holes);
        panel.show_chrome(grid, claims, &views.holes.every, &mut |ui, cells| {
            let mut run = Out::default();
            draw(ui, cells, &views, &mut run);
            presses.extend(run.presses.into_iter().map(|(at, act)| (cells.under(at), act)));
            spots = run.spots.into_iter().map(|(rect, act)| (cells_of(cells, rect), act)).collect();
            scroll = run.scroll;
            (run.parts, run.wake)
        })
    };
    // Appended, not set: a pass run outside the loop's frame (the script's
    // dump draws the face into a test terminal) leaves its press for the
    // next frame's duties.
    gui.chrome.presses.extend(presses);
    gui.chrome.spots = spots;
    gui.chrome.hand = shown.is_some_and(|shown| shown.hand);
    if let Some(scroll) = scroll {
        gui.servers.drop_scroll = scroll;
    }
}

/// The frame's duties for the presses the chrome's passes took, after the
/// draw, in order: each runs the cells' press preamble (the keys handed back
/// from the queue, a lost grip's end), then its act through `Gui::act`, the
/// path a press on the cells takes. True when there were any: the next frame
/// is owed at the brisk pace, which shows what they did (a popup they
/// opened among it). No act quits at this commit; a quit from one would end
/// the face through `Face::ended`.
pub(super) fn after_draw(gui: &mut Gui) -> bool {
    let presses = std::mem::take(&mut gui.chrome.presses);
    if presses.is_empty() {
        return false;
    }
    for (at, act) in presses {
        if gui.press_preamble(at)
            && let Some(act) = act
        {
            let _ = gui.act(act);
        }
    }
    gui.hot = true;
    true
}

/// The pointer's hand: on a cell a chrome popup claims, the chrome's
/// alone (the cells beneath the popup are not what a press there works);
/// elsewhere the cells' or the chrome's.
pub(super) fn hand(gui: &Gui, cells: bool) -> bool {
    let claims = &gui.chrome.claims;
    let popup_cell = claims.popup && gui.ui.pointer.is_some_and(|at| claims.holds(at));
    if popup_cell { gui.chrome.hand } else { cells || gui.chrome.hand }
}

/// What a press at `at` works, for the script's dumps, where the chrome
/// claims the cell: its item there, the last that stacks there (a popup's
/// catcher under its rows), or nothing. None where the cell is the cells
/// registry's to answer.
pub(super) fn hit_debug(gui: &Gui, at: Position) -> Option<Option<Act>> {
    if !gui.chrome.claims.holds(at) {
        return None;
    }
    Some(gui.chrome.spots.iter().rev().find(|(rect, _)| rect.contains(at)).map(|(_, act)| act.clone()))
}

#[cfg(test)]
mod tests;
