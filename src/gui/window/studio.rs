//! The Studio kit: the look the egui pages wear in the desktop window's
//! panel (panel.rs), and the pieces they draw it with. The Stats page wore
//! it first (src/admin/stats/egui_view.rs); the admin rooms wear it too,
//! after the Claude Design canvas of the Admin screen (2026-11-20).
//!
//! The look is a mixing desk's — square corners, hairline panels, labels
//! in spaced capitals, figures in the grid's own monospace (Hack, already
//! in the binary), and LED meters for bars, each a row or a column of small
//! lit segments. The ground is the kit's, so a panel meets the cells
//! around it with no seam.
//!
//! Nothing here holds a page's state or edits it: every widget draws what
//! it is handed and says what the pointer did to it, and the page turns
//! that into one of its own acts. Keys never reach egui (panel.rs), so a
//! field is drawn from the page's own value and caret, which the page's
//! key handler edits as it always has.
//!
//! Built only with the window: the terminal build has no panel to draw in.

// The kit comes before the rooms' egui faces, which land one by one: until
// each has, some of its pieces have no caller outside the tests.
#![allow(dead_code)]

use std::ops::Range;

use egui::epaint::{CornerRadius, Shadow, Shape, StrokeKind};
use egui::text::{CCursor, LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontFamily, FontId, Id, Layout, Pos2, Rect, Response, ScrollArea, Sense, Stroke,
    Ui, UiBuilder, Vec2, pos2, vec2,
};
use ratatui::style::Color;

use super::panel::STRONG;
use crate::kit::theme::th;

/// The desk's corners: square, softened by a point or two.
pub(crate) const RADIUS: u8 = 3;
/// An LED segment's height and the dark gap above it, in points.
pub(crate) const LED: f32 = 4.0;
pub(crate) const LED_GAP: f32 = 2.0;
/// Spaced capitals: this much extra room per letter, as a share of the size.
pub(crate) const TRACK: f32 = 0.12;
/// The small labels' wider tracking (field labels, table headings, chips).
pub(crate) const LABEL_TRACK: f32 = 0.14;
/// The small labels' size.
pub(crate) const LABEL_SIZE: f32 = 10.5;
/// The scrim a modal lays over its room: the canvas's rgba(9, 10, 15, .62).
pub(crate) const SCRIM: Color32 = Color32::from_rgba_premultiplied(6, 6, 9, 158);
/// A control's click target, square: the canvas's 28-point buttons.
pub(crate) const HIT: f32 = 28.0;

/// The desk's palette. The ground, the ok green, the gold and the danger
/// red are the kit's (the window pins truecolour, so every role is an RGB;
/// the fallbacks are the canvas hexes); the rest is Studio's own.
pub(crate) struct Pal {
    pub ground: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub edge: Color32,
    pub edge_strong: Color32,
    pub text: Color32,
    /// Secondary figures and paths, a step under the text.
    pub soft: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub accent_hot: Color32,
    pub on_accent: Color32,
    pub unlit: Color32,
    pub down: Color32,
    pub up: Color32,
    pub gold: Color32,
    pub ok: Color32,
    /// Destructive hover (a row's remove), never decoration.
    pub danger: Color32,
}

impl Pal {
    pub(crate) fn now() -> Pal {
        let th = th();
        let rgb = |c: Color, fallback: Color32| match c {
            Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
            _ => fallback,
        };
        let ground = th.ground_rgb.map_or(Color32::from_rgb(0x12, 0x13, 0x1c), |(r, g, b)| Color32::from_rgb(r, g, b));
        Pal {
            ground,
            panel: lift(ground, 6),
            raised: lift(ground, 14),
            edge: lift(ground, 22),
            edge_strong: lift(ground, 36),
            text: Color32::from_rgb(0xe6, 0xe6, 0xe3),
            soft: Color32::from_rgb(0xc4, 0xc5, 0xc1),
            dim: Color32::from_rgb(0x9a, 0x9c, 0x98),
            accent: Color32::from_rgb(0xf2, 0xa9, 0x3b),
            accent_hot: Color32::from_rgb(0xff, 0xcf, 0x7a),
            on_accent: Color32::from_rgb(0x1a, 0x12, 0x06),
            unlit: lift(ground, 18),
            down: Color32::from_rgb(0xff, 0x8a, 0x6b),
            up: rgb(th.ok, Color32::from_rgb(0x8f, 0xd1, 0x8b)),
            gold: rgb(th.gold, Color32::from_rgb(0xe0, 0xb4, 0x5a)),
            ok: rgb(th.ok, Color32::from_rgb(0x8f, 0xd1, 0x8b)),
            danger: rgb(th.danger, Color32::from_rgb(0xe0, 0x6c, 0x75)),
        }
    }
}

/// `c` lifted toward white by `by` of 255, opaque.
pub(crate) fn lift(c: Color32, by: u8) -> Color32 {
    let up = |v: u8| v.saturating_add(by);
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// Names and prose: the system's UI face.
pub(crate) fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Labels, set in spaced capitals ([`caps`]): the UI face's semibold.
pub(crate) fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Figures: the grid's monospace.
pub(crate) fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

pub(crate) fn square() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

/// A page's pass begins here: egui styled for the desk (once per context),
/// the ground painted over the whole of `ui`, and the palette to draw with.
pub(crate) fn begin(ui: &Ui) -> Pal {
    let pal = Pal::now();
    style_once(ui.ctx(), &pal);
    ui.painter().rect_filled(ui.max_rect(), 0.0, pal.ground);
    pal
}

/// egui's look for the desk, set once per context, which every page that
/// draws in the panel shares: dark, amber on the scroll bars.
pub(crate) fn style_once(ctx: &egui::Context, pal: &Pal) {
    let id = Id::new("studio-styled");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_theme(egui::Theme::Dark);
    let (accent, text, ground, edge) = (pal.accent, pal.text, pal.ground, pal.edge_strong);
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = accent;
        style.visuals.extreme_bg_color = ground;
        style.visuals.panel_fill = ground;
        style.visuals.window_fill = ground;
        let widgets = &mut style.visuals.widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            state.fg_stroke.color = text;
            state.corner_radius = CornerRadius::same(1);
        }
        widgets.inactive.bg_fill = edge;
        widgets.hovered.bg_fill = accent.gamma_multiply(0.75);
        widgets.active.bg_fill = accent;
        style.spacing.item_spacing = vec2(8.0, 0.0);
        style.interaction.tooltip_delay = 0.0;
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// The pointing hand over a control a click acts on: the GUI shows it
/// for the panel as it does for its own cells.
fn hand(response: Response) -> Response {
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ── Text ──────────────────────────────────────────────────────────────────

/// `s` in one line no wider than `max_w`, an ellipsis where it is cut,
/// `spacing` points between letters, anchored at `pos` by `align`.
/// Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spaced(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, spacing: f32, max_w: f32, align: Align2) -> Rect {
    let format = TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() };
    let mut job = LayoutJob::single_section(s.to_string(), format);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// `s` in one line, as [`spaced`] with no extra room between letters.
pub(crate) fn text(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, s, font, color, 0.0, max_w, align)
}

/// A label in the desk's spaced capitals.
pub(crate) fn caps(ui: &Ui, pos: Pos2, s: &str, size: f32, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, &s.to_uppercase(), strong(size), color, size * TRACK, max_w, align)
}

/// How wide [`caps`] draws `s` at `size`.
pub(crate) fn caps_width(ui: &Ui, s: &str, size: f32) -> f32 {
    tracked_width(ui, s, size, TRACK)
}

/// The small label over a field, a table's heading, a chip's word: the
/// semibold face at [`LABEL_SIZE`] in capitals tracked wider than [`caps`].
pub(crate) fn small_caps(ui: &Ui, pos: Pos2, s: &str, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, &s.to_uppercase(), strong(LABEL_SIZE), color, LABEL_SIZE * LABEL_TRACK, max_w, align)
}

/// How wide [`small_caps`] draws `s`.
pub(crate) fn small_caps_width(ui: &Ui, s: &str) -> f32 {
    tracked_width(ui, s, LABEL_SIZE, LABEL_TRACK)
}

fn tracked_width(ui: &Ui, s: &str, size: f32, track: f32) -> f32 {
    let format =
        TextFormat { font_id: strong(size), color: Color32::WHITE, extra_letter_spacing: size * track, ..Default::default() };
    ui.painter().layout_job(LayoutJob::single_section(s.to_uppercase(), format)).size().x
}

/// How wide `s` is in `font`.
pub(crate) fn width_of(ui: &Ui, s: &str, font: FontId) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), font, Color32::WHITE).size().x
}

/// One gold sentence for a state with nothing else to show.
pub(crate) fn sentence(ui: &mut Ui, head: &str, tail: &str, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let head_rect = text(ui, rect.left_center(), head, strong(15.0), pal.gold, w, Align2::LEFT_CENTER);
    text(ui, pos2(head_rect.max.x, rect.center().y), tail, body(15.0), pal.gold, rect.max.x - head_rect.max.x, Align2::LEFT_CENTER);
}

/// A room's foot, one line across `rect`: what it is busy with in the
/// accent wins, else its note — an error in gold, any other note dim —
/// cut with an ellipsis where it runs long. Nothing at all draws nothing.
pub(crate) fn note_line(ui: &Ui, rect: Rect, busy: Option<&str>, note: Option<&(String, bool)>, pal: &Pal) {
    let (words, color) = match (busy, note) {
        (Some(busy), _) => (busy, pal.accent),
        (None, Some((note, is_err))) => (note.as_str(), if *is_err { pal.gold } else { pal.dim }),
        (None, None) => return,
    };
    text(ui, rect.left_center(), words, body(13.0), color, rect.width(), Align2::LEFT_CENTER);
}

// ── Shapes ────────────────────────────────────────────────────────────────

/// A panel of the desk: the lifted ground and its hairline.
pub(crate) fn panel(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_filled(rect, square(), pal.panel);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
}

/// The keyboard's ring: an inset accent hairline round `rect`, as the
/// canvas marks the cursor's row and a focused control.
pub(crate) fn ring(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
}

/// The desk's LED, centred on `at`: a 6-point square, its corners softened
/// by one, lit in `color`.
pub(crate) fn led(ui: &Ui, at: Pos2, color: Color32) {
    ui.painter().rect_filled(Rect::from_center_size(at, vec2(6.0, 6.0)), CornerRadius::same(1), color);
}

/// A chevron, two strokes, centred on `at`, pointing down (or up).
pub(crate) fn chevron(ui: &Ui, at: Pos2, color: Color32, up: bool) {
    let (dx, dy) = (4.0, if up { -2.5 } else { 2.5 });
    let stroke = Stroke::new(1.5, color);
    ui.painter().line_segment([at + vec2(-dx, -dy), at + vec2(0.0, dy)], stroke);
    ui.painter().line_segment([at + vec2(0.0, dy), at + vec2(dx, -dy)], stroke);
}

/// A small solid triangle at `at`, pointing down (a fall) or up (a rise).
pub(crate) fn arrow(ui: &Ui, at: Pos2, color: Color32, down: bool) {
    let (w, h) = (4.0, 3.5);
    let points = if down {
        vec![at + vec2(-w, -h), at + vec2(w, -h), at + vec2(0.0, h)]
    } else {
        vec![at + vec2(-w, h), at + vec2(w, h), at + vec2(0.0, -h)]
    };
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// The canvas's small icons, drawn in 12 points with 1.6-point strokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    /// A window's close: the cross.
    Close,
    /// An add.
    Plus,
    /// A row's remove: the cross, red under the pointer.
    Remove,
    /// The way back: a chevron pointing left.
    Back,
    /// A list to drop: a chevron pointing down.
    Chevron,
    /// A copy: two sheets, one over the other.
    Copy,
    /// A download: an arrow down onto a line.
    Download,
}

/// `icon` in `color`, centred on `at`.
pub(crate) fn draw_icon(ui: &Ui, at: Pos2, icon: Icon, color: Color32) {
    // The canvas's 12-point boxes, their paths verbatim.
    let o = at - vec2(6.0, 6.0);
    let p = |x: f32, y: f32| o + vec2(x, y);
    let stroke = Stroke::new(1.6, color);
    let painter = ui.painter();
    match icon {
        Icon::Close | Icon::Remove => {
            painter.line_segment([p(2.5, 2.5), p(9.5, 9.5)], stroke);
            painter.line_segment([p(9.5, 2.5), p(2.5, 9.5)], stroke);
        }
        Icon::Plus => {
            painter.line_segment([p(6.0, 2.0), p(6.0, 10.0)], stroke);
            painter.line_segment([p(2.0, 6.0), p(10.0, 6.0)], stroke);
        }
        Icon::Back => {
            painter.add(Shape::line(vec![p(7.5, 2.5), p(4.0, 6.0), p(7.5, 9.5)], stroke));
        }
        Icon::Chevron => {
            painter.add(Shape::line(vec![p(2.0, 4.0), p(6.0, 8.0), p(10.0, 4.0)], stroke));
        }
        // The canvas draws the tools' two in a finer stroke.
        Icon::Copy => {
            let thin = Stroke::new(1.3, color);
            painter.rect_stroke(Rect::from_min_max(p(3.5, 3.5), p(10.5, 10.5)), 0.0, thin, StrokeKind::Middle);
            painter.add(Shape::line(vec![p(1.5, 8.0), p(1.5, 1.5), p(8.0, 1.5)], thin));
        }
        Icon::Download => {
            let thin = Stroke::new(1.3, color);
            painter.line_segment([p(6.0, 1.5), p(6.0, 8.0)], thin);
            painter.add(Shape::line(vec![p(3.2, 5.4), p(6.0, 8.2), p(8.8, 5.4)], thin));
            painter.line_segment([p(1.5, 10.5), p(10.5, 10.5)], thin);
        }
    }
}

// ── Meters ────────────────────────────────────────────────────────────────

/// An LED meter standing on `base`, centred on `cx`: `lit` segments of
/// `color` from the bottom, each [`LED`] tall with a [`LED_GAP`] above it.
pub(crate) fn led_column(ui: &Ui, cx: f32, base: f32, width: f32, lit: usize, color: Color32) {
    for k in 0..lit {
        let bottom = base - k as f32 * (LED + LED_GAP);
        let seg = Rect::from_min_max(pos2(cx - width / 2.0, bottom - LED), pos2(cx + width / 2.0, bottom));
        ui.painter().rect_filled(seg, 0.0, color);
    }
}

/// A horizontal LED meter across `track`: `share` of its segments lit.
pub(crate) fn led_row(ui: &Ui, track: Rect, share: f32, lit_color: Color32, unlit: Color32) {
    let step = LED + LED_GAP - 1.0;
    let count = ((track.width() + LED_GAP - 1.0) / step).floor().max(1.0) as usize;
    let lit = if share > 0.0 { ((share * count as f32).round() as usize).clamp(1, count) } else { 0 };
    for k in 0..count {
        let x = track.min.x + k as f32 * step;
        let seg = Rect::from_min_size(pos2(x, track.min.y), vec2(LED - 1.0, track.height()));
        ui.painter().rect_filled(seg, 0.0, if k < lit { lit_color } else { unlit });
    }
}

/// How many of a meter's `n` segments `done` of `total` lights: whole
/// segments only, rounded down — 41 of 100 lights four of ten, 99 of 100
/// nine — so the last one lights only when the work is done. Nothing to do
/// lights none.
pub(crate) fn meter_lit(done: u64, total: u64, n: usize) -> usize {
    if total == 0 {
        return 0;
    }
    (u128::from(done.min(total)) * n as u128 / u128::from(total)) as usize
}

/// A progress meter in a row, its left edge's middle at `at`: `n`
/// segments `seg` points each and `gap` apart, the first `lit` in `color`
/// and the rest unlit, each segment a stack of 2-point lines a point apart
/// from its foot up, as the desk's LEDs are lit (the canvas's Backups run
/// and Torrents progress). [`meter_lit`] says how many. Returns where it
/// was drawn.
#[allow(clippy::too_many_arguments)]
pub(crate) fn meter(ui: &Ui, at: Pos2, n: usize, lit: usize, seg: Vec2, gap: f32, color: Color32, pal: &Pal) -> Rect {
    let w = n as f32 * seg.x + n.saturating_sub(1) as f32 * gap;
    let rect = Rect::from_min_size(pos2(at.x, at.y - seg.y / 2.0), vec2(w, seg.y));
    for k in 0..n {
        let x = rect.min.x + k as f32 * (seg.x + gap);
        let color = if k < lit { color } else { pal.unlit };
        let mut foot = rect.max.y;
        while foot - 2.0 >= rect.min.y - 0.01 {
            ui.painter().rect_filled(Rect::from_min_max(pos2(x, foot - 2.0), pos2(x + seg.x, foot)), 0.0, color);
            foot -= 3.0;
        }
    }
    rect
}

// ── Controls ──────────────────────────────────────────────────────────────

/// How [`segmented`] draws: the chosen segment lit amber, or only marked
/// by an amber rule along its foot.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Seg {
    Lit,
    Marked,
}

pub(crate) const SEG_H: f32 = 30.0;
const SEG_SIZE: f32 = 12.5;

/// How wide [`segmented`] draws `labels`.
pub(crate) fn segmented_width(ui: &Ui, labels: &[String]) -> f32 {
    labels.iter().map(|l| caps_width(ui, l, SEG_SIZE) + 30.0).sum::<f32>() + 2.0
}

/// A row of square buttons joined by hairlines at `left`, centred on
/// `mid_y`: one per label in spaced capitals, `chosen` lit (or marked).
/// Returns the control's rect and the segment clicked, if any.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented(
    ui: &Ui,
    id: Id,
    left: f32,
    mid_y: f32,
    labels: &[String],
    chosen: usize,
    kind: Seg,
    pal: &Pal,
) -> (Rect, Option<usize>) {
    let total = segmented_width(ui, labels);
    let outer = Rect::from_min_size(pos2(left, mid_y - SEG_H / 2.0), vec2(total, SEG_H));
    ui.painter().rect_filled(outer, square(), pal.panel);
    let mut x = outer.min.x + 1.0;
    let mut clicked = None;
    for (i, label) in labels.iter().enumerate() {
        let w = caps_width(ui, label, SEG_SIZE) + 30.0;
        let seg = Rect::from_min_size(pos2(x, outer.min.y + 1.0), vec2(w, SEG_H - 2.0));
        let response = ui.interact(seg, id.with(i), Sense::click());
        let on = i == chosen;
        let color = match (on, kind) {
            (true, Seg::Lit) => {
                ui.painter().rect_filled(seg, CornerRadius::same(2), pal.accent);
                pal.on_accent
            }
            (true, Seg::Marked) => {
                ui.painter().rect_filled(seg, CornerRadius::same(2), pal.raised);
                let rule = Rect::from_min_max(pos2(seg.min.x, seg.max.y - 2.0), seg.max);
                ui.painter().rect_filled(rule, 0.0, pal.accent);
                pal.accent
            }
            (false, _) if response.hovered() => {
                ui.painter().rect_filled(seg, CornerRadius::same(2), pal.raised);
                pal.text
            }
            (false, _) => pal.dim,
        };
        if i > 0 {
            ui.painter().line_segment([pos2(x, outer.min.y + 1.0), pos2(x, outer.max.y - 1.0)], Stroke::new(1.0, pal.edge_strong));
        }
        caps(ui, seg.center(), label, SEG_SIZE, color, w, Align2::CENTER_CENTER);
        if response.clicked() && !on {
            clicked = Some(i);
        }
        x += w;
    }
    ui.painter().rect_stroke(outer, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    (outer, clicked)
}

/// A square button at `rect` in spaced capitals: lit amber when
/// `primary`, an outline in `tone` otherwise.
pub(crate) fn button(ui: &Ui, id: Id, rect: Rect, label: &str, primary: bool, tone: Color32, pal: &Pal) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let hovered = response.hovered();
    let color = if primary {
        ui.painter().rect_filled(rect, square(), if hovered { pal.accent_hot } else { pal.accent });
        pal.on_accent
    } else {
        if hovered {
            ui.painter().rect_filled(rect, square(), tone.gamma_multiply(0.14));
        }
        ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, tone), StrokeKind::Inside);
        tone
    };
    caps(ui, rect.center(), label, 12.5, color, rect.width(), Align2::CENTER_CENTER);
    response
}

/// A form's button height, and its capitals' size.
pub(crate) const FORM_BUTTON_H: f32 = 34.0;
const FORM_BUTTON_SIZE: f32 = 11.5;

/// How wide [`form_button`] wants to be for `label`.
pub(crate) fn form_button_width(ui: &Ui, label: &str) -> f32 {
    caps_width(ui, label, FORM_BUTTON_SIZE) + 36.0
}

/// A form's button at `rect` (the canvas's): the one act lit amber when
/// `primary`, else a hairline in the strong edge with dim capitals that
/// brighten under the pointer.
pub(crate) fn form_button(ui: &Ui, id: Id, rect: Rect, label: &str, primary: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    let color = if primary {
        ui.painter().rect_filled(rect, square(), if hovered { pal.accent_hot } else { pal.accent });
        pal.on_accent
    } else {
        if hovered {
            ui.painter().rect_filled(rect, square(), pal.raised);
        }
        ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
        if hovered { pal.text } else { pal.dim }
    };
    caps(ui, rect.center(), label, FORM_BUTTON_SIZE, color, rect.width(), Align2::CENTER_CENTER);
    response
}

/// A button that is only its icon, centred on `at`: a [`HIT`]-square
/// target, `icon` in `tone`, raised under the pointer with the icon in the
/// text colour — or in the danger red, for a remove.
pub(crate) fn icon_button(ui: &Ui, id: Id, at: Pos2, icon: Icon, tone: Color32, pal: &Pal) -> Response {
    let rect = Rect::from_center_size(at, Vec2::splat(HIT));
    let response = hand(ui.interact(rect, id, Sense::click()));
    let color = if response.hovered() {
        ui.painter().rect_filled(rect, square(), pal.raised);
        if icon == Icon::Remove { pal.danger } else { pal.text }
    } else {
        tone
    };
    draw_icon(ui, at, icon, color);
    response
}

/// What a [`tool_button`] takes of the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    /// The hover, the hand and the click.
    Live,
    /// Nothing while its act runs: drawn in the accent, the kit's busy
    /// colour, with no hand and no click.
    Busy,
    /// Nothing while something lies over it (a room's modal): drawn at
    /// rest, whatever the pointer does.
    Inert,
}

/// A tool button's height, the canvas's, and its capitals' size.
pub(crate) const TOOL_H: f32 = 28.0;
const TOOL_SIZE: f32 = 11.0;
/// A tool's icon, the gap after it, and the room either side.
const TOOL_ICON: f32 = 12.0;
const TOOL_GAP: f32 = 6.0;
const TOOL_PAD: f32 = 10.0;

/// How wide [`tool_button`] wants to be for `label`.
pub(crate) fn tool_button_width(ui: &Ui, label: &str) -> f32 {
    TOOL_ICON + TOOL_GAP + caps_width(ui, label, TOOL_SIZE) + 2.0 * TOOL_PAD
}

/// A panel's small tool at `rect`, as the canvas puts copy and download
/// over the server's log: `icon` and `label` in spaced capitals in a
/// strong-edged hairline, dim, its ink in the text colour under the
/// pointer (the canvas lifts no fill under it), the hand over it. A busy
/// one is in the accent and takes no click; an inert one stays at rest
/// (see [`Tool`]).
pub(crate) fn tool_button(ui: &Ui, id: Id, rect: Rect, icon: Icon, label: &str, tool: Tool, pal: &Pal) -> Response {
    let live = tool == Tool::Live;
    let response = ui.interact(rect, id, if live { Sense::click() } else { Sense::hover() });
    let response = if live { hand(response) } else { response };
    let hovered = live && response.hovered();
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    let ink = match tool {
        Tool::Busy => pal.accent,
        _ if hovered => pal.text,
        _ => pal.dim,
    };
    let words = caps_width(ui, label, TOOL_SIZE);
    let left = rect.center().x - (TOOL_ICON + TOOL_GAP + words) / 2.0;
    draw_icon(ui, pos2(left + TOOL_ICON / 2.0, rect.center().y), icon, ink);
    let at = pos2(left + TOOL_ICON + TOOL_GAP, rect.center().y);
    caps(ui, at, label, TOOL_SIZE, ink, words + 1.0, Align2::LEFT_CENTER);
    response
}

/// The tick box's side.
const BOX: f32 = 16.0;

/// A tick box at `rect` on `fill`: a dim edge when off, the ok green's
/// edge and check when on, each brighter under the pointer.
fn tick_box(ui: &Ui, rect: Rect, on: bool, hovered: bool, fill: Color32, pal: &Pal) {
    let edge = if on { pal.ok } else { pal.dim };
    let edge = if hovered { lift(edge, 40) } else { edge };
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    if on {
        // The canvas's check in its 10-point box: M1.5 5.2 l2.4 2.4 L8.6 2.6.
        let o = rect.center() - vec2(5.0, 5.0);
        let points = vec![o + vec2(1.5, 5.2), o + vec2(3.9, 7.6), o + vec2(8.6, 2.6)];
        let check = if hovered { lift(pal.ok, 40) } else { pal.ok };
        ui.painter().add(Shape::line(points, Stroke::new(1.7, check)));
    }
}

/// A tick box alone, centred on `at` — a table's flag column — on the
/// panel's fill, its target [`HIT`] points square. The page flips its
/// flag on the click.
pub(crate) fn checkbox(ui: &Ui, id: Id, at: Pos2, on: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(Rect::from_center_size(at, Vec2::splat(HIT)), id, Sense::click()));
    tick_box(ui, Rect::from_center_size(at, Vec2::splat(BOX)), on, response.hovered(), pal.panel, pal);
    response
}

/// A form's tick box and its words, a 28-point row of the layout: the box
/// on the ground's fill, `label` in the text colour and `detail` dim right
/// after it (so it carries its own separator, " — …"). The box and its
/// words take the click, not the row's empty end.
pub(crate) fn check_line(ui: &mut Ui, id: Id, on: bool, label: &str, detail: &str, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
    let words = width_of(ui, label, body(14.0)) + if detail.is_empty() { 0.0 } else { width_of(ui, detail, body(14.0)) };
    let hit = Rect::from_min_size(row.min, vec2((BOX + 10.0 + words).min(w), row.height()));
    let response = hand(ui.interact(hit, id, Sense::click()));
    let mark = Rect::from_min_size(pos2(row.min.x, row.center().y - BOX / 2.0), Vec2::splat(BOX));
    tick_box(ui, mark, on, response.hovered(), pal.ground, pal);
    let x = mark.max.x + 10.0;
    let drawn = text(ui, pos2(x, row.center().y), label, body(14.0), pal.text, row.max.x - x, Align2::LEFT_CENTER);
    if !detail.is_empty() {
        text(ui, pos2(drawn.max.x, row.center().y), detail, body(14.0), pal.dim, row.max.x - drawn.max.x, Align2::LEFT_CENTER);
    }
    response
}

/// One choice of a group, a 30-point row of the layout: a 14-point round
/// mark (the accent's ring and dot when `on`, the strong edge's ring when
/// not), `label` (semibold when on) and `detail` dim after it, cut to the
/// row. The whole row takes the click and is raised under the pointer.
pub(crate) fn radio(ui: &mut Ui, id: Id, on: bool, label: &str, detail: &str, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    // The canvas's row reaches 8 points past the column either side.
    let hit = row.expand2(vec2(8.0, 0.0));
    let response = hand(ui.interact(hit, id, Sense::click()));
    if response.hovered() {
        ui.painter().rect_filled(hit, square(), pal.raised);
    }
    let centre = pos2(row.min.x + 7.0, row.center().y);
    ui.painter().circle_filled(centre, 7.0, pal.ground);
    ui.painter().circle_stroke(centre, 6.5, Stroke::new(1.0, if on { pal.accent } else { pal.edge_strong }));
    if on {
        ui.painter().circle_filled(centre, 3.0, pal.accent);
    }
    let x = row.min.x + 24.0;
    let font = if on { strong(13.5) } else { body(13.5) };
    let drawn = text(ui, pos2(x, row.center().y), label, font, pal.text, row.max.x - x, Align2::LEFT_CENTER);
    if !detail.is_empty() {
        let from = drawn.max.x + 10.0;
        text(ui, pos2(from, row.center().y), detail, body(13.0), pal.dim, row.max.x - from, Align2::LEFT_CENTER);
    }
    response
}

/// A room's add card, the full column wide and 56 points tall: an ok-green
/// hairline with a plus and `label` in semibold green at its centre,
/// washed green under the pointer (the canvas's "Add a music folder").
pub(crate) fn add_card(ui: &mut Ui, id: Id, label: &str, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
    let response = hand(ui.interact(rect, id, Sense::click()));
    if response.hovered() {
        ui.painter().rect_filled(rect, square(), pal.ok.gamma_multiply(0.10));
    }
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.ok), StrokeKind::Inside);
    let font = strong(14.0);
    let words = width_of(ui, label, font.clone()).min((w - 60.0).max(0.0));
    let left = rect.center().x - (16.0 + 10.0 + words) / 2.0;
    // The canvas's 16-point plus, stroked 1.5.
    let plus = pos2(left + 8.0, rect.center().y);
    let stroke = Stroke::new(1.5, pal.ok);
    ui.painter().line_segment([plus + vec2(0.0, -5.5), plus + vec2(0.0, 5.5)], stroke);
    ui.painter().line_segment([plus + vec2(-5.5, 0.0), plus + vec2(5.5, 0.0)], stroke);
    text(ui, pos2(left + 26.0, rect.center().y), label, font, pal.ok, words + 1.0, Align2::LEFT_CENTER);
    response
}

/// A chip anchored at `at` by `align`: `s` in tracked small capitals in a
/// 1-point outline of `color`, 20 points tall (the gold BETA beside a
/// room's state line). Returns where it was drawn.
pub(crate) fn chip(ui: &Ui, at: Pos2, align: Align2, s: &str, color: Color32) -> Rect {
    let w = small_caps_width(ui, s) + 16.0;
    let rect = align.anchor_size(at, vec2(w, 20.0));
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, color), StrokeKind::Inside);
    small_caps(ui, rect.center(), s, color, w, Align2::CENTER_CENTER);
    rect
}

// ── Fields ────────────────────────────────────────────────────────────────

/// A field's box height.
pub(crate) const FIELD_H: f32 = 34.0;
/// The room between a field's edge and its text.
const FIELD_PAD: f32 = 10.0;
/// What a masked field shows for each character.
const MASK: char = egui::epaint::text::PASSWORD_REPLACEMENT_CHAR;

/// What a field draws: the page's own value and caret, never edited here.
/// Keys never reach egui; the page's key handler edits its value as it
/// always has, and the next pass draws the result.
#[derive(Clone, Copy)]
pub(crate) struct Input<'a> {
    pub value: &'a str,
    /// Where the caret stands, in characters.
    pub caret: usize,
    /// The field has the page's keyboard: the accent's edge and the caret.
    pub focused: bool,
    /// The input method's uncommitted text, drawn underlined before the
    /// caret while the field is focused.
    pub composition: &'a str,
    pub masked: bool,
    /// The figures' monospace rather than the UI face: paths, numbers.
    pub mono: bool,
    /// A disabled field is dim, on the panel's fill, and takes no click.
    pub enabled: bool,
    /// Drawn dim while the value is empty.
    pub placeholder: &'a str,
}

impl<'a> Input<'a> {
    pub(crate) fn new(value: &'a str, caret: usize) -> Self {
        Input { value, caret, focused: false, composition: "", masked: false, mono: false, enabled: true, placeholder: "" }
    }

    /// The field has the keyboard, and the input method is composing
    /// `composition` (empty for nothing).
    pub(crate) fn focused(self, focused: bool, composition: &'a str) -> Self {
        Input { focused, composition, ..self }
    }

    pub(crate) fn masked(self) -> Self {
        Input { masked: true, ..self }
    }

    pub(crate) fn mono(self) -> Self {
        Input { mono: true, ..self }
    }

    pub(crate) fn enabled(self, enabled: bool) -> Self {
        Input { enabled, ..self }
    }

    pub(crate) fn placeholder(self, placeholder: &'a str) -> Self {
        Input { placeholder, ..self }
    }
}

/// What a field's pass gives back.
pub(crate) struct Typed {
    /// The box's response: a click on it is the page's to focus the field.
    pub response: Response,
    /// The foot of the caret, in egui's points, while the field is focused:
    /// where the host floats the input method's candidates.
    pub caret: Option<Pos2>,
    /// The character a click on the box landed nearest, for the page to put
    /// its caret at. None while a composition is drawn in the line.
    pub pick: Option<usize>,
}

/// A field as the canvas draws it, a row of the layout the full width:
/// `label` in dim small capitals, then its box ([`field_box`]).
pub(crate) fn field(ui: &mut Ui, id: Id, label: &str, input: &Input, pal: &Pal) -> Typed {
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    small_caps(ui, head.left_center(), label, pal.dim, w, Align2::LEFT_CENTER);
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
    field_box(ui, id, rect, input, pal)
}

/// A field's box at `rect`: the ground's fill in a strong-edged hairline,
/// the accent's while focused and a brighter one under the pointer (the
/// pointer wins); the value in the UI face (or the monospace), bullets for
/// a masked one, a composition underlined before the caret, and a steady
/// 1-point caret while focused. The line is windowed so the caret stays in
/// view, the window kept from pass to pass under `id` as a text field's is.
pub(crate) fn field_box(ui: &Ui, id: Id, rect: Rect, input: &Input, pal: &Pal) -> Typed {
    let sense = if input.enabled { Sense::click() } else { Sense::hover() };
    let mut response = ui.interact(rect, id, sense);
    if input.enabled {
        response = response.on_hover_cursor(CursorIcon::Text);
    }
    let hovered = input.enabled && response.hovered();
    let focused = input.enabled && input.focused;
    let (fill, edge, ink) = if input.enabled {
        let edge = if hovered {
            pal.dim
        } else if focused {
            pal.accent
        } else {
            pal.edge_strong
        };
        (pal.ground, edge, pal.text)
    } else {
        (pal.panel, pal.edge, pal.dim)
    };
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);

    let view = rect.shrink2(vec2(FIELD_PAD, 0.0));
    let font = if input.mono { mono(12.5) } else { body(14.0) };
    let (line, caret_at, composing) = field_line(input);
    let clip = ui.painter().with_clip_rect(view.intersect(ui.clip_rect()));
    if line.is_empty() && !input.placeholder.is_empty() {
        let galley = ui.painter().layout_no_wrap(input.placeholder.to_string(), font.clone(), pal.dim);
        clip.galley(pos2(view.min.x, rect.center().y - galley.size().y / 2.0), galley, pal.dim);
    }
    let galley = ui.painter().layout_job(field_job(&line, composing.clone(), font.clone(), ink));
    let caret_x = galley.pos_from_cursor(CCursor::new(caret_at)).min.x;
    let memory = id.with("window");
    let offset = if focused {
        let last = ui.ctx().data(|d| d.get_temp::<f32>(memory)).unwrap_or(0.0);
        let offset = scroll_to_caret(last, caret_x, galley.size().x, view.width());
        ui.ctx().data_mut(|d| d.insert_temp(memory, offset));
        offset
    } else {
        ui.ctx().data_mut(|d| d.remove::<f32>(memory));
        0.0
    };
    let origin = pos2(view.min.x - offset, rect.center().y - galley.size().y / 2.0);
    clip.galley(origin, galley.clone(), ink);

    let caret = focused.then(|| {
        let x = (origin.x + caret_x).round();
        let h = font.size + 5.0;
        let bar = Rect::from_min_size(pos2(x, rect.center().y - h / 2.0), vec2(1.0, h));
        ui.painter().rect_filled(bar, 0.0, pal.accent);
        pos2(x, bar.max.y)
    });
    let pick = if response.clicked() && composing.is_empty() {
        response.interact_pointer_pos().map(|p| galley.cursor_from_pos(p - origin).index.0.min(input.value.chars().count()))
    } else {
        None
    };
    Typed { response, caret, pick }
}

/// The line a field lays out, the caret's index in it and where the
/// composition runs in it, both in characters: the value (or its bullets)
/// with the composition (or its bullets) spliced in at the caret while
/// the field is focused.
fn field_line(input: &Input) -> (String, usize, Range<usize>) {
    let bullets = |s: &str| -> String { s.chars().map(|_| MASK).collect() };
    let shown = if input.masked { bullets(input.value) } else { input.value.to_string() };
    let caret = input.caret.min(shown.chars().count());
    if !(input.focused && input.enabled) || input.composition.is_empty() {
        return (shown, caret, caret..caret);
    }
    let composing = if input.masked { bullets(input.composition) } else { input.composition.to_string() };
    let n = composing.chars().count();
    let byte = shown.char_indices().nth(caret).map_or(shown.len(), |(i, _)| i);
    (format!("{}{composing}{}", &shown[..byte], &shown[byte..]), caret + n, caret..caret + n)
}

/// The field's line as egui lays it out: one run, the composition's
/// characters underlined.
fn field_job(line: &str, composing: Range<usize>, font: FontId, color: Color32) -> LayoutJob {
    let byte = |c: usize| line.char_indices().nth(c).map_or(line.len(), |(i, _)| i);
    let (from, to) = (byte(composing.start), byte(composing.end));
    let plain = TextFormat { font_id: font, color, ..Default::default() };
    let under = TextFormat { underline: Stroke::new(1.0, color), ..plain.clone() };
    let mut job = LayoutJob::default();
    for (part, format) in [(&line[..from], &plain), (&line[from..to], &under), (&line[to..], &plain)] {
        if !part.is_empty() {
            job.append(part, 0.0, format.clone());
        }
    }
    if job.sections.is_empty() {
        // An empty line still has a row: the caret stands in it.
        job.append("", 0.0, plain);
    }
    job
}

/// How far a field's line is scrolled left so its caret, `caret_x` points
/// into a line `line_w` wide, stays in a view `view_w` wide, from where it
/// stood (`offset`): unmoved while the caret is in view, moved just far
/// enough to bring it back when not, and never past the line's end, where
/// a point is kept for the caret.
pub(crate) fn scroll_to_caret(offset: f32, caret_x: f32, line_w: f32, view_w: f32) -> f32 {
    let most = (line_w + 1.0 - view_w).max(0.0);
    let mut at = offset.clamp(0.0, most);
    if caret_x < at {
        at = caret_x;
    } else if caret_x + 1.0 > at + view_w {
        at = caret_x + 1.0 - view_w;
    }
    at.clamp(0.0, most)
}

// ── Tables ────────────────────────────────────────────────────────────────

/// A column of a [`Table`]: its heading, its width (zero for the one that
/// takes what the others leave) and where its cells sit in it.
pub(crate) struct Col {
    pub title: String,
    pub w: f32,
    pub align: Align,
}

impl Col {
    pub(crate) fn left(title: impl Into<String>, w: f32) -> Col {
        Col { title: title.into(), w, align: Align::Min }
    }

    pub(crate) fn right(title: impl Into<String>, w: f32) -> Col {
        Col { title: title.into(), w, align: Align::Max }
    }

    pub(crate) fn center(title: impl Into<String>, w: f32) -> Col {
        Col { title: title.into(), w, align: Align::Center }
    }
}

/// The gap between columns, and a row's padding either side.
const COL_GAP: f32 = 16.0;
const CELL_PAD: f32 = 10.0;

/// The columns' x ranges across `x0..x1`.
pub(crate) fn spans(x0: f32, x1: f32, cols: &[Col]) -> Vec<(f32, f32)> {
    let fixed: f32 = cols.iter().map(|c| c.w).sum::<f32>() + COL_GAP * cols.len().saturating_sub(1) as f32;
    let flex = (x1 - x0 - fixed).max(60.0);
    let mut x = x0;
    cols.iter()
        .map(|c| {
            let w = if c.w == 0.0 { flex } else { c.w };
            let span = (x, w);
            x += w + COL_GAP;
            span
        })
        .collect()
}

/// What one row hands its painter: the row's selection area, its cells,
/// and its state.
pub(crate) struct RowCtx<'a> {
    /// The row less its trail (the columns after the selection area).
    pub rect: Rect,
    /// The trail's rect: a remove button's column, say. Empty for none.
    pub trail: Rect,
    pub cells: &'a [(f32, f32)],
    /// The keyboard's cursor is on the row.
    pub selected: bool,
    /// The row is selected and lit amber (a [`Table::lit`] table), so its
    /// text is the selection's ink ([`ink`]).
    pub lit: bool,
    pub hovered: bool,
}

impl RowCtx<'_> {
    /// Cell `at`'s rect on the row.
    pub(crate) fn cell(&self, at: usize) -> Rect {
        let (x, w) = self.cells[at];
        Rect::from_min_max(pos2(x, self.rect.min.y), pos2(x + w, self.rect.max.y))
    }
}

/// How a table marks its keyboard cursor's row.
#[derive(Clone, Copy, PartialEq)]
enum Look {
    /// The canvas's admin rooms: the raised fill and an inset accent ring.
    Ring,
    /// The Stats page: the row lit amber, its text in the selection's ink.
    Lit,
}

/// A table under its headings, its rows scrolled by egui and drawn one at
/// a time by the page's closure, the keyboard cursor's row kept in view on
/// the pass it moved.
pub(crate) struct Table<'a> {
    id: Id,
    cols: &'a [Col],
    len: usize,
    cursor: Option<usize>,
    look: Look,
    row_h: f32,
    trail: f32,
    clickable: bool,
}

/// What a table's pass saw.
#[derive(Default)]
pub(crate) struct TableShown {
    /// The row a click landed on, if one did.
    pub row: Option<usize>,
    /// The row and the column a click landed in, if it was in a cell.
    pub cell: Option<(usize, usize)>,
    /// The last row on screen, for paging.
    pub last_on_screen: usize,
}

impl<'a> Table<'a> {
    /// The canvas's table: 30-point headings in small capitals over a
    /// hairline, 40-point rows, the cursor's row raised and ringed in the
    /// accent, a row under the pointer raised, a click on a row reported.
    pub(crate) fn new(id: Id, cols: &'a [Col], len: usize) -> Self {
        Table { id, cols, len, cursor: None, look: Look::Ring, row_h: 40.0, trail: 0.0, clickable: true }
    }

    /// The keyboard cursor's row.
    pub(crate) fn cursor(self, cursor: Option<usize>) -> Self {
        Table { cursor, ..self }
    }

    /// The Stats page's table: 26-point headings, 30-point rows ruled
    /// apart, the cursor's row lit amber.
    pub(crate) fn lit(self) -> Self {
        Table { look: Look::Lit, row_h: 30.0, ..self }
    }

    pub(crate) fn row_height(self, row_h: f32) -> Self {
        Table { row_h, ..self }
    }

    /// Points at the right of every row outside its selection area, for a
    /// column of row buttons (a remove) the cursor's ring stops short of.
    pub(crate) fn trail(self, trail: f32) -> Self {
        Table { trail, ..self }
    }

    /// Rows that take no click as a whole, only through the controls the
    /// page draws in them (the canvas's Libraries): no hover, no row click.
    pub(crate) fn inert(self) -> Self {
        Table { clickable: false, ..self }
    }

    /// Lay the table out in what is left of `ui`, `draw_row` painting each
    /// row on screen.
    pub(crate) fn show(self, ui: &mut Ui, pal: &Pal, mut draw_row: impl FnMut(&Ui, usize, &RowCtx)) -> TableShown {
        let Table { id, cols, len, cursor, look, row_h, trail, clickable } = self;
        let w = ui.available_width();
        let head_h = if look == Look::Lit { 26.0 } else { 30.0 };
        let (head, _) = ui.allocate_exact_size(vec2(w, head_h), Sense::hover());
        let cells = spans(head.min.x + CELL_PAD, head.max.x - CELL_PAD - trail, cols);
        for (c, (x, cw)) in cols.iter().zip(&cells) {
            let y = head.center().y;
            let (pos, align) = match c.align {
                Align::Min => (pos2(*x, y), Align2::LEFT_CENTER),
                Align::Center => (pos2(x + cw / 2.0, y), Align2::CENTER_CENTER),
                Align::Max => (pos2(x + cw, y), Align2::RIGHT_CENTER),
            };
            match look {
                Look::Lit => caps(ui, pos, &c.title, 11.0, pal.dim, *cw, align),
                Look::Ring => small_caps(ui, pos, &c.title, pal.dim, *cw, align),
            };
        }
        let rule = if look == Look::Lit { pal.edge_strong } else { pal.edge };
        ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, rule));
        ui.add_space(4.0);

        // The cursor moved since the last pass: the offset that brings it in.
        let memory = id.with("view");
        let (last_cursor, last_offset, last_height): (Option<usize>, f32, f32) =
            ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((None, 0.0, 0.0));
        let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
        if cursor != last_cursor
            && let Some(s) = cursor
            && last_height > 0.0
        {
            let (row_top, row_bottom) = (s as f32 * row_h, (s + 1) as f32 * row_h);
            if row_top < last_offset {
                scroll = scroll.vertical_scroll_offset(row_top);
            } else if row_bottom > last_offset + last_height {
                scroll = scroll.vertical_scroll_offset(row_bottom - last_height);
            }
        }
        let mut shown = TableShown::default();
        let sense = if clickable { Sense::click() } else { Sense::hover() };
        let output = scroll.show_rows(ui, row_h, len, |ui, range| {
            shown.last_on_screen = range.end.saturating_sub(1);
            for i in range {
                let (full, response) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), sense);
                // The Stats page's controls show no hand yet, its rows
                // among them; the rooms' do.
                let response = if clickable && look == Look::Ring { hand(response) } else { response };
                let rect = Rect::from_min_max(full.min, pos2((full.max.x - trail).max(full.min.x), full.max.y));
                let trail_rect = Rect::from_min_max(pos2(rect.max.x, full.min.y), full.max);
                let selected = cursor == Some(i);
                // A room's row stays raised while the pointer is on a
                // control the page draws in it (a tick box, a remove), which
                // egui hovers in its stead: the row would blink out under
                // it. Nothing covers a Stats row.
                let over = response.hovered() || (look == Look::Ring && response.contains_pointer());
                let hovered = clickable && !selected && over;
                match look {
                    Look::Lit => {
                        if selected {
                            ui.painter().rect_filled(rect, CornerRadius::same(2), pal.accent);
                        } else if hovered {
                            ui.painter().rect_filled(rect, CornerRadius::same(2), pal.raised);
                        }
                        if !selected {
                            let rule = [pos2(rect.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)];
                            ui.painter().line_segment(rule, Stroke::new(1.0, pal.panel));
                        }
                    }
                    Look::Ring => {
                        if selected || hovered {
                            ui.painter().rect_filled(rect, square(), pal.raised);
                        }
                        if selected {
                            ring(ui, rect, pal);
                        }
                    }
                }
                let cells = spans(rect.min.x + CELL_PAD, rect.max.x - CELL_PAD, cols);
                let lit = selected && look == Look::Lit;
                draw_row(ui, i, &RowCtx { rect, trail: trail_rect, cells: &cells, selected, lit, hovered });
                if response.clicked() {
                    shown.row = Some(i);
                    let x = response.interact_pointer_pos().map_or(f32::NAN, |p| p.x);
                    shown.cell = cells.iter().position(|(cx, cw)| x >= *cx && x < cx + cw).map(|c| (i, c));
                }
            }
        });
        let state = (cursor, output.state.offset.y, output.inner_rect.height());
        ui.ctx().data_mut(|d| d.insert_temp(memory, state));
        shown
    }
}

/// A cell's text colour on its row: the selection's ink on a lit row,
/// white for the text colour under the pointer, else its own.
pub(crate) fn ink(row: &RowCtx, own: Color32, pal: &Pal) -> Color32 {
    if row.lit {
        pal.on_accent
    } else if row.hovered && own == pal.text {
        Color32::WHITE
    } else {
        own
    }
}

/// A cell's text: left or right in its column, cut to fit.
pub(crate) fn cell(ui: &Ui, row: &RowCtx, at: usize, s: &str, font: FontId, color: Color32, right: bool) -> Rect {
    let (x, w) = row.cells[at];
    let y = row.rect.center().y;
    if right {
        text(ui, pos2(x + w, y), s, font, color, w, Align2::RIGHT_CENTER)
    } else {
        text(ui, pos2(x, y), s, font, color, w, Align2::LEFT_CENTER)
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// A modal window's padding, the room under its title, and the title row.
const MODAL_PAD: f32 = 24.0;
const MODAL_GAP: f32 = 20.0;
const TITLE_H: f32 = 20.0;

/// What a modal's pass gives back.
pub(crate) struct ModalShown<R> {
    /// The close button was clicked: the page cancels, as on Esc.
    pub closed: bool,
    /// Where the window stood.
    pub window: Rect,
    /// What the body's closure returned.
    pub inner: R,
}

/// A modal over `over` only — the room's area, never the window's — as the
/// canvas draws one: the scrim across `over`, and centred in it a window
/// `width` wide (less where `over` is narrower) on the panel's fill in a
/// strong-edged hairline with the canvas's shadow, 24 points in: `title`
/// in 16-point semibold with a close button at its right, then `body` laid
/// out top-down under it, scrolled where it is taller than `over` allows.
/// The window takes the height its body asked for on the last pass (egui
/// runs the pass again when that changed, so it never shows misplaced). A
/// press anywhere in `over` outside the window is swallowed: what is
/// under the scrim is inert.
pub(crate) fn modal<R>(ui: &Ui, id: Id, over: Rect, width: f32, title: &str, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<R> {
    let ctx = ui.ctx().clone();
    let width = width.min(over.width() - 32.0).max(over.width().min(160.0));
    let max_h = (over.height() - 32.0).max(2.0 * MODAL_PAD + TITLE_H);
    let size_key = id.with("height");
    let known: Option<f32> = ctx.data(|d| d.get_temp(size_key));
    let window = Rect::from_center_size(over.center(), vec2(width, known.unwrap_or(max_h).min(max_h)));
    // Up at once, as the canvas's and the cells' modals are: egui fades an
    // area in by default, over frames the panel would have to run for it.
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false);
    let shown = area.show(&ctx, |ui| {
        let _scrim = ui.allocate_rect(over, Sense::click_and_drag());
        ui.painter().rect_filled(over, 0.0, SCRIM);
        let shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(115) };
        ui.painter().add(shadow.as_shape(window, square()));
        ui.painter().rect_filled(window, square(), pal.panel);
        ui.painter().rect_stroke(window, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
        let inner = window.shrink(MODAL_PAD);
        let title_row = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
        // The canvas pulls the close button 6 points past the padding.
        let close = icon_button(ui, id.with("close"), pos2(inner.max.x - 8.0, title_row.center().y), Icon::Close, pal.dim, pal);
        text(ui, title_row.left_center(), title, strong(16.0), pal.text, inner.width() - 40.0, Align2::LEFT_CENTER);
        let budget = (max_h - 2.0 * MODAL_PAD - TITLE_H - MODAL_GAP).max(0.0);
        let body_rect = Rect::from_min_size(pos2(inner.min.x, title_row.max.y + MODAL_GAP), vec2(inner.width(), budget));
        let mut child =
            ui.new_child(UiBuilder::new().id_salt(id.with("body")).max_rect(body_rect).layout(Layout::top_down(Align::Min)));
        let out = ScrollArea::vertical().id_salt(id.with("scroll")).max_height(budget).auto_shrink([false, true]).show(&mut child, body);
        let wanted = 2.0 * MODAL_PAD + TITLE_H + MODAL_GAP + out.content_size.y.min(budget);
        (close.clicked(), out.inner, wanted.min(max_h))
    });
    let (closed, inner, wanted) = shown.inner;
    if known.is_none_or(|h| (h - wanted).abs() > 0.5) {
        ctx.data_mut(|d| d.insert_temp(size_key, wanted));
        ctx.request_discard("a modal took its body's height");
    }
    ModalShown { closed, window: Rect::from_center_size(over.center(), vec2(width, wanted)), inner }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        super::super::panel::fonts(&ctx);
        ctx
    }

    /// One pass over a `size` screen with `events`, `run` drawing it.
    fn pass(ctx: &egui::Context, size: Vec2, events: Vec<egui::Event>, run: &mut dyn FnMut(&mut Ui)) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| run(ui));
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        out
    }

    #[test]
    fn a_meter_lights_whole_segments_only_and_all_of_them_only_when_done() {
        assert_eq!(meter_lit(41, 100, 10), 4, "the canvas's 41%");
        assert_eq!(meter_lit(1204, 2930, 10), 4);
        assert_eq!(meter_lit(99, 100, 10), 9, "a tenth short lights nine");
        assert_eq!(meter_lit(100, 100, 10), 10);
        assert_eq!(meter_lit(7, 3, 10), 10, "past the total is done, no more");
        assert_eq!(meter_lit(0, 100, 10), 0);
        assert_eq!(meter_lit(5, 0, 10), 0, "nothing to do lights nothing");
        assert_eq!(meter_lit(u64::MAX, u64::MAX, 10), 10, "no overflow on the way");
    }

    #[test]
    fn the_window_moves_only_to_keep_the_caret_in_view() {
        // A short line never scrolls.
        assert_eq!(scroll_to_caret(0.0, 40.0, 80.0, 200.0), 0.0);
        // The caret at the end of a long line: just in view, a point spare.
        assert_eq!(scroll_to_caret(0.0, 500.0, 500.0, 200.0), 301.0);
        // Back a little: the window stays where it was.
        assert_eq!(scroll_to_caret(301.0, 450.0, 500.0, 200.0), 301.0);
        // Back past its left edge: the window follows the caret, no further.
        assert_eq!(scroll_to_caret(301.0, 120.0, 500.0, 200.0), 120.0);
        // The line shrank under a window scrolled far: never past its end.
        assert_eq!(scroll_to_caret(301.0, 150.0, 150.0, 200.0), 0.0);
    }

    #[test]
    fn a_composition_is_spliced_in_at_the_caret_and_masked_with_the_value() {
        let input = Input::new("abcd", 2).focused(true, "xy");
        assert_eq!(field_line(&input), ("abxycd".to_string(), 4, 2..4));
        let masked = Input::new("pässe", 9).focused(true, "ü").masked();
        assert_eq!(field_line(&masked), ("••••••".to_string(), 6, 5..6), "the caret clamped to the value's end");
        let idle = Input::new("abcd", 2).focused(false, "xy");
        assert_eq!(field_line(&idle), ("abcd".to_string(), 2, 2..2), "an unfocused field composes nothing");
    }

    #[test]
    fn the_caret_point_stays_in_the_box_and_follows_a_composition() {
        let ctx = context();
        let size = vec2(400.0, 200.0);
        let rect = Rect::from_min_size(pos2(20.0, 40.0), vec2(220.0, FIELD_H));
        let long = "/Volumes/NAS/Field Recordings 2024/a very long folder name/track.flac";
        let caret_of = |ctx: &egui::Context, input: Input| -> Pos2 {
            let mut at = None;
            for _ in 0..2 {
                let _ = pass(ctx, size, Vec::new(), &mut |ui| {
                    let pal = Pal::now();
                    at = field_box(ui, Id::new("probe-field"), rect, &input, &pal).caret;
                });
            }
            at.expect("a focused field says where its caret is")
        };
        let at_start = caret_of(&ctx, Input::new(long, 0).focused(true, ""));
        assert!((at_start.x - (rect.min.x + FIELD_PAD)).abs() <= 1.0, "the caret at the start of the box: {at_start:?}");
        assert!(at_start.y > rect.center().y && at_start.y <= rect.max.y, "the caret's foot, inside the box: {at_start:?}");

        let at_end = caret_of(&ctx, Input::new(long, long.chars().count()).focused(true, ""));
        let view = rect.shrink2(vec2(FIELD_PAD, 0.0));
        assert!(at_end.x >= view.min.x && at_end.x < view.max.x, "a long line scrolls the caret into view: {at_end:?}");
        assert!(at_end.x > view.max.x - 4.0, "at the view's right end: {at_end:?}");

        let plain = caret_of(&ctx, Input::new("ab", 1).focused(true, ""));
        let composing = caret_of(&ctx, Input::new("ab", 1).focused(true, "かな"));
        assert!(composing.x > plain.x + 8.0, "the caret stands after the composition: {plain:?} {composing:?}");

        let mut unfocused = Some(Pos2::ZERO);
        let _ = pass(&ctx, size, Vec::new(), &mut |ui| {
            unfocused = field_box(ui, Id::new("idle-field"), rect, &Input::new(long, 3), &Pal::now()).caret;
        });
        assert_eq!(unfocused, None, "no caret without the keyboard");
    }

    #[test]
    fn a_modal_swallows_presses_beside_its_window_and_reports_its_close() {
        let ctx = context();
        let size = vec2(800.0, 500.0);
        let over = Rect::from_min_max(pos2(200.0, 0.0), pos2(800.0, 500.0));
        let beneath_clicked = Cell::new(false);
        let closed = Cell::new(false);
        let window = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            if ui.interact(over, Id::new("beneath"), Sense::click()).clicked() {
                beneath_clicked.set(true);
            }
            let shown = modal(ui, Id::new("probe-modal"), over, 480.0, "Add a user", &pal, |ui| {
                ui.add_space(120.0);
            });
            if shown.closed {
                closed.set(true);
            }
            window.set(shown.window);
        };
        let click = |at: Pos2| -> Vec<Vec<egui::Event>> {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            vec![vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]]
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let at = window.get();
        assert!(over.contains_rect(at), "the window stands in the room: {at:?}");
        assert!((at.center().x - over.center().x).abs() < 1.0, "centred in the room, not the screen");
        assert!((at.height() - (2.0 * MODAL_PAD + TITLE_H + MODAL_GAP + 120.0)).abs() < 1.0, "its body's height: {at:?}");
        for events in click(pos2(over.min.x + 20.0, over.min.y + 20.0)) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(!beneath_clicked.get(), "the scrim swallowed the press");
        assert!(!closed.get());
        let close = pos2(at.max.x - MODAL_PAD - 8.0, at.min.y + MODAL_PAD + TITLE_H / 2.0);
        for events in click(close) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(closed.get(), "the close button reports its click");
    }

    #[test]
    fn a_rooms_row_stays_raised_while_the_pointer_is_on_a_control_in_it() {
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let cols = [Col::left("User", 0.0), Col::center("Admin", 60.0)];
        let hovered = Cell::new(false);
        let tick_at = Cell::new(Pos2::ZERO);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            Table::new(Id::new("probe-table"), &cols, 3).show(ui, &pal, |ui, i, row| {
                let at = row.cell(1).center();
                let _ = checkbox(ui, Id::new("probe-tick").with(i), at, false, &pal);
                if i == 0 {
                    hovered.set(row.hovered);
                    tick_at.set(at);
                }
            });
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let at = tick_at.get();
        let mut out = None;
        for _ in 0..2 {
            out = Some(pass(&ctx, size, vec![egui::Event::PointerMoved(at)], &mut run));
        }
        assert_eq!(out.map(|o| o.platform_output.cursor_icon), Some(CursorIcon::PointingHand), "the tick box has the pointer");
        assert!(hovered.get(), "and its row stays raised under it");
    }
}
