//! The top strip's egui face in the GUI's own window, hosted by the
//! chrome (src/gui/chrome.rs) in row 0's cells: the tabs, the Visualizer
//! item, the server label and [+], their two tooltips, and the server menu
//! (`menu`), drawn from the strip's view and the menu's, their presses the
//! cells' acts.
//!
//! The look is the canvas's (Main's PATTERN: STRIP; the TopStates,
//! TopTips, TopLocales and ServerMenu boards; decisions D1-D10, D22, D23):
//! one 16-point row on the ground with an edge hairline at its foot; the
//! tabs from the grid's second column, each its name with 10 points either
//! side and 2 points apart, the screen that is up marked (the raised fill,
//! its name in the text colour and the semibold, a 2-point amber rule along
//! the strip's foot), the others dim and in the text colour and the
//! semibold under the pointer; the MP3 Player tab's sync figure in the
//! monospace 4 points after its name, amber while the sync runs, dim while
//! it waits paused, on the marked tab too; a hairline, then the Visualizer
//! item, its lamp amber while its window is open; at the right [+] in its
//! cells' box, and the server label right-aligned a blank cell short of it,
//! bare dim words with a chevron while two servers or more can be picked.
//!
//! Every slot is measured in points in the face in use, so a language's
//! words, CJK's among them, take their own room: the label's never reaches
//! the strip, measured with the sync's figure. What a press works is the
//! cells': it acts on the press, only on the cells the cells' registry
//! leaves the strip (none while an MP3 write runs), and the hand and the
//! hover look show only there. Nothing is greyed while the strip takes no
//! press: it is drawn at rest, the marks and the lamp kept.

use std::time::Duration;

use egui::{Align2, Color32, FontId, Id, Pos2, Response, Sense, Stroke, Ui, pos2, vec2};
use ratatui::layout::Rect;
use unicode_segmentation::UnicodeSegmentation;

use super::{StripView, SyncMark};
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Icon, Pal, body, mono, strong};
use crate::gui::{Act, Screen};

mod menu;

pub(crate) use menu::{MenuCtx, menu};

/// The strip's words: the tabs, the Visualizer and the server label.
const SIZE: f32 = 12.5;
/// A sync's figure after the MP3 Player tab's name, in the monospace.
const MARK_SIZE: f32 = 12.0;
/// The room either side of an item's words, and the air between two tabs.
const PAD: f32 = 10.0;
const APART: f32 = 2.0;
/// From the MP3 Player tab's name to its sync's figure.
const MARK_GAP: f32 = 4.0;
/// The hairline before the Visualizer: the air either side of it, and how
/// tall it stands.
const DIVIDER_AIR: f32 = 8.0;
const DIVIDER_H: f32 = 10.0;
/// The Visualizer's lamp (`studio::led`'s 6 points), and the air after it.
const LAMP: f32 = 6.0;
const LAMP_GAP: f32 = 6.0;
/// From the server's name to the menu's chevron, and the chevron's box.
const CHEVRON_GAP: f32 = 6.0;
const CHEVRON_W: f32 = 10.0;
/// [+]'s square in its cells' box, raised under the pointer.
const ADD_SQUARE: f32 = 16.0;
/// The marked tab's amber rule along the strip's foot.
const RULE: f32 = 2.0;

/// What the strip's face is handed besides the view.
pub(crate) struct StripCtx<'a> {
    /// Where the grid's cells stand in points.
    pub cells: Cells,
    /// Where a press on row 0 would act this frame, in points: the cells
    /// the registry left the strip. Empty while it is inert (an MP3 write,
    /// a layer that dropped the cells' registries). Nothing lights, shows
    /// the hand or takes a press outside it.
    pub live: &'a [egui::Rect],
    /// The pointer stood as the cells would have drawn the strip (no modal
    /// stashed it), and no popup holds it: hover may light, a tooltip may
    /// show.
    pub hover: bool,
    /// The time since the last key, which hides a tooltip for the dwell.
    pub since_key: Option<Duration>,
    pub pal: &'a Pal,
}

/// What the strip's face did this pass.
#[derive(Default)]
pub(crate) struct StripOut {
    /// The act of this pass's press, where it pressed a strip item: taken
    /// on the press, as the cells take it.
    pub act: Option<Act>,
    /// Every item where it stands and what a press on it does, for the
    /// script's dumps.
    pub items: Vec<(egui::Rect, Act)>,
    /// The tooltip's box, while one shows: it cuts no hole and takes none.
    pub tip: Option<egui::Rect>,
}

/// The strip in row 0, on `ui` (its child over row 0, clipped to it, the
/// ground painted): the foot's hairline, the tabs, the hairline before the
/// Visualizer and the item, [+], and the server label; a tooltip of the
/// Visualizer's or [+]'s once the pointer has rested on it for the dwell.
pub(crate) fn show(ui: &mut Ui, view: &StripView, cx: &StripCtx) -> StripOut {
    let mut out = StripOut::default();
    let pal = cx.pal;
    let slots = slots(ui, view, cx.cells);
    let strip = slots.strip;
    let mid = strip.center().y;
    studio::rule(ui, strip.min.x, strip.max.x, strip.max.y - 1.0, pal);

    for (tab, &at) in view.tabs.iter().zip(&slots.tabs) {
        let id = Id::new(("strip-tab", format!("{:?}", tab.screen)));
        // The tab that is up still takes its press, which changes nothing,
        // and shows the hand for it, as its cells do; it never lights.
        let item = take(ui, cx, &mut out, at, id, Act::Screen(tab.screen), view.live);
        if tab.up {
            ui.painter().rect_filled(at, 0.0, pal.raised);
            ui.painter().rect_filled(egui::Rect::from_min_max(pos2(at.min.x, at.max.y - RULE), at.max), 0.0, pal.accent);
        }
        let (font, ink) = if tab.up || item.hot { (strong(SIZE), pal.text) } else { (body(SIZE), pal.dim) };
        let mark = view.mark.filter(|_| tab.screen == Screen::Device);
        tab_words(ui, at, &tab.label, mark, font, ink, pal);
    }

    ui.painter().vline(slots.divider + 0.5, (mid - DIVIDER_H / 2.0)..=(mid + DIVIDER_H / 2.0), Stroke::new(1.0, pal.edge));
    let viz = slots.viz;
    let item = take(ui, cx, &mut out, viz, Id::new("strip-viz"), Act::VizWindow, view.live);
    // Lit is not a tab's mark: the lamp and the semibold while the window
    // lives, which the pointer does not change; closed, the word brightens
    // under the pointer and the lamp stays unlit.
    let (font, ink) = match (view.viz.lit, item.hot) {
        (true, _) => (strong(SIZE), pal.text),
        (false, true) => (body(SIZE), pal.text),
        (false, false) => (body(SIZE), pal.dim),
    };
    studio::led(ui, pos2(viz.min.x + PAD + LAMP / 2.0, studio::led_mid(ui, mid, &font, false)), if view.viz.lit { pal.accent } else { pal.unlit });
    let x = viz.min.x + PAD + LAMP + LAMP_GAP;
    studio::text(ui, pos2(x, mid), &view.viz.label, font, ink, viz.max.x - x, Align2::LEFT_CENTER);
    if let Some(response) = item.response.as_ref().filter(|_| item.hot) {
        out.tip = studio::tip_keyed(ui, response, &view.viz.tip, view.key_hints, cx.since_key, pal);
    }

    let plus = slots.plus;
    let item = take(ui, cx, &mut out, plus, Id::new("server-add"), Act::SrvAdd, true);
    if item.hot {
        ui.painter().rect_filled(egui::Rect::from_center_size(plus.center(), egui::Vec2::splat(ADD_SQUARE)), studio::square(), pal.raised);
    }
    studio::icon(ui, plus.center(), Icon::Add, if item.hot { pal.text } else { pal.dim });
    // A plain tip with no key, as the kit hangs one: short in every
    // language, it lays out as the Visualizer's would.
    if let Some(response) = item.response.as_ref().filter(|_| item.hot)
        && out.tip.is_none()
    {
        out.tip = studio::tip_under(response, &view.add_tip, cx.since_key, pal);
    }

    if let (Some(server), Some(label)) = (&view.server, &slots.label) {
        // A single server's label is words, no control: no hover, no hand.
        // No tooltip either: one matured right where the menu opens and
        // sat on top of its first rows.
        let hot = server.menu && take(ui, cx, &mut out, label.rect, Id::new("server-label"), Act::SrvMenu, true).hot;
        let (font, ink) = if hot { (strong(SIZE), pal.text) } else { (body(SIZE), pal.dim) };
        studio::text(ui, pos2(label.words_right, mid), &server.name, font, ink, label.room, Align2::RIGHT_CENTER);
        // On the row's middle, as [+] beside it stands its plus and the
        // kit's select box its chevron: on the small letters' it hung low,
        // a "v" set after the name.
        if let Some(chevron) = label.chevron {
            studio::chevron(ui, pos2(chevron.center().x, mid), ink, false);
        }
    }
    out
}

/// Where the strip's items stand this pass, in points.
#[derive(Clone, Debug)]
struct Slots {
    /// Row 0.
    strip: egui::Rect,
    /// Library, Stats, Admin, MP3 Player, as the view lists them.
    tabs: [egui::Rect; 4],
    /// The left edge of the hairline before the Visualizer.
    divider: f32,
    viz: egui::Rect,
    /// The server label, while a server is named and room enough is left
    /// for it.
    label: Option<Label>,
    /// [+]'s box: its cells.
    plus: egui::Rect,
}

/// The server label's place: its words and the chevron, what a press on
/// it works from; where the words end, right-aligned, and the room they are
/// cut to; and the chevron's box, while there is a menu.
#[derive(Clone, Debug)]
struct Label {
    rect: egui::Rect,
    words_right: f32,
    room: f32,
    chevron: Option<egui::Rect>,
}

/// The strip's [`Slots`] for `view` over `cells`: the tabs from the grid's
/// second column, each its name in the face it wears at rest (the marked
/// one's semibold) with [`PAD`] either side, the MP3 Player tab's sync
/// figure among its words, [`APART`] between them; the hairline and the
/// Visualizer item after them, its word in the semibold while it is lit;
/// [+] in its cells, three columns from the grid's fifth-last, as the cells
/// stand it; and the label right-aligned to end a blank cell short of [+],
/// starting no nearer the Visualizer than a blank cell ([`fit_label`]).
fn slots(ui: &Ui, view: &StripView, cells: Cells) -> Slots {
    let grid = cells.grid;
    let column = |x: u16| cells.rect(Rect { x, y: grid.y, width: 1, height: 1 }).min.x;
    let strip = cells.rect(Rect { x: grid.x, y: grid.y, width: grid.width, height: 1.min(grid.height) });
    let height = strip.height();
    let mut x = column(grid.x + 1.min(grid.width));
    let mut tabs = [egui::Rect::NOTHING; 4];
    for (slot, tab) in tabs.iter_mut().zip(&view.tabs) {
        let font = if tab.up { strong(SIZE) } else { body(SIZE) };
        let mark = view.mark.filter(|_| tab.screen == Screen::Device).map_or(0.0, |mark| MARK_GAP + mark_width(ui, &mark));
        let w = PAD + studio::width_of(ui, &tab.label, font) + mark + PAD;
        *slot = egui::Rect::from_min_size(pos2(x, strip.min.y), vec2(w, height));
        x += w + APART;
    }
    let divider = tabs[3].max.x + DIVIDER_AIR;
    let font = if view.viz.lit { strong(SIZE) } else { body(SIZE) };
    let w = PAD + LAMP + LAMP_GAP + studio::width_of(ui, &view.viz.label, font) + PAD;
    let viz = egui::Rect::from_min_size(pos2(divider + 1.0 + DIVIDER_AIR, strip.min.y), vec2(w, height));
    let plus_cells = Rect { x: grid.x + grid.width.saturating_sub(5), y: grid.y, width: 3.min(grid.width), height: 1 };
    let plus = cells.rect(plus_cells);
    let end = column(plus_cells.x.saturating_sub(1));
    let start = viz.max.x + cells.cell().x;
    let label = view.server.as_ref().and_then(|server| fit_label(ui, &server.name, server.menu, start, end, strip));
    Slots { strip, tabs, divider, viz, label, plus }
}

/// How wide a sync's figure lays out after the tab's name.
fn mark_width(ui: &Ui, mark: &SyncMark) -> f32 {
    studio::width_of(ui, &mark.words(), mono(MARK_SIZE))
}

/// Where the server label stands between `start` and `end`, the room the
/// strip leaves it, in `strip`'s row: right-aligned to `end`, its chevron
/// last with a menu; the name whole where it fits the room left, else cut
/// at its end with an ellipsis, the chevron kept after it; nothing where
/// not even its first letter and the ellipsis fit, as the cells give up
/// under two cells. Measured in the face the label wears at rest.
fn fit_label(ui: &Ui, name: &str, menu: bool, start: f32, end: f32, strip: egui::Rect) -> Option<Label> {
    let mark = if menu { CHEVRON_GAP + CHEVRON_W } else { 0.0 };
    let room = end - start - mark;
    let whole = studio::width_of(ui, name, body(SIZE));
    let words = if whole <= room {
        whole
    } else {
        let first = name.graphemes(true).next()?;
        if studio::width_of(ui, &format!("{first}…"), body(SIZE)) > room {
            return None;
        }
        studio::cut_width(ui, name, body(SIZE), room)
    };
    let words_right = end - mark;
    let rect = egui::Rect::from_min_max(pos2(words_right - words, strip.min.y), pos2(end, strip.max.y));
    let chevron = menu.then(|| egui::Rect::from_min_max(pos2(end - CHEVRON_W, strip.min.y), pos2(end, strip.max.y)));
    Some(Label { rect, words_right, room, chevron })
}

/// A tab's words centred in its slot `at`: its name in `font` and `ink`,
/// and for the MP3 Player tab a sync's figure [`MARK_GAP`] after it in the
/// monospace on the name's baseline, amber while the sync runs and dim
/// while it waits, whatever the tab's look.
fn tab_words(ui: &Ui, at: egui::Rect, name: &str, mark: Option<SyncMark>, font: FontId, ink: Color32, pal: &Pal) {
    let mid = at.center().y;
    let figure = mark.map(|mark| (mark.words(), if mark.paused { pal.dim } else { pal.accent }));
    let tail = figure.as_ref().map_or(0.0, |(words, _)| MARK_GAP + studio::width_of(ui, words, mono(MARK_SIZE)));
    let w = studio::width_of(ui, name, font.clone()) + tail;
    let x = at.center().x - w / 2.0;
    let drawn = studio::text(ui, pos2(x, mid), name, font, ink, at.width(), Align2::LEFT_CENTER);
    if let Some((words, tone)) = figure {
        // A line centred on the row stands on the UI face's baseline at its
        // size there: the figure stands on the name's.
        let baseline = mid + studio::centred_baseline(ui, SIZE);
        let x = drawn.max.x + MARK_GAP;
        studio::text(ui, pos2(x, studio::mid_on(ui, baseline, MARK_SIZE)), &words, mono(MARK_SIZE), tone, (at.max.x - x).max(1.0), Align2::LEFT_CENTER);
    }
}

/// What the pointer does to an item: its response while it senses one, and
/// whether it lights.
struct Item {
    response: Option<Response>,
    hot: bool,
}

/// Whether `at` is on a cell where the strip takes a press this frame.
fn live_at(cx: &StripCtx, at: Pos2) -> bool {
    cx.live.iter().any(|rect| rect.contains(at))
}

/// An item at `at` whose press is `act`, where the strip takes a press
/// there (`cx.live`, and `live`, which an MP3 write turns off for the tabs
/// and the Visualizer): it senses the click and is listed for the dumps;
/// the hand shows where the pointer stands on a live cell of it, and it
/// lights there while hover may ([`StripCtx::hover`]); the press that
/// lands on a live cell of it is taken, on the press, as the cells take
/// theirs, a tap's too ([`studio::pass_press`]: without it a tap pressed
/// nothing). Where none of it is live it senses nothing and is drawn at rest.
/// Only the item's own points answer for it: egui would give it the press
/// and the hover of a gap beside it, within its reach for a fingertip, and
/// the cells' gaps answer nothing.
fn take(ui: &Ui, cx: &StripCtx, out: &mut StripOut, at: egui::Rect, id: Id, act: Act, live: bool) -> Item {
    if !live || !cx.live.iter().any(|rect| rect.intersect(at).is_positive()) {
        return Item { response: None, hot: false };
    }
    let on = |pos: Pos2| at.contains(pos) && live_at(cx, pos);
    let response = ui.interact(at, id, Sense::click());
    let under = response.hover_pos().is_some_and(on);
    let response = if under { studio::hand(response) } else { response };
    out.items.push((at, act.clone()));
    if studio::pressed(&response) && studio::pass_press(ui).is_some_and(on) {
        out.act = Some(act);
    }
    Item { hot: cx.hover && under, response: Some(response) }
}

#[cfg(test)]
mod tests;
