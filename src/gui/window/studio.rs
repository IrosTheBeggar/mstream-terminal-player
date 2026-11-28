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

use std::ops::Range;
use std::sync::Arc;

use egui::emath::GuiRounding;
use egui::epaint::{CornerRadius, Shadow, Shape, StrokeKind};
use egui::text::{CCursor, LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, CursorIcon, FontFamily, FontId, Galley, Id, Layout, Pos2, Rangef, Rect, Response, ScrollArea,
    Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use ratatui::style::Color;
use unicode_segmentation::UnicodeSegmentation;

use super::panel::STRONG;
use crate::kit::theme::th;

/// The desk's corners: square, softened by a point or two.
const RADIUS: u8 = 3;
/// An LED segment's height and the dark gap above it, in points.
pub(crate) const LED: f32 = 4.0;
pub(crate) const LED_GAP: f32 = 2.0;
/// Spaced capitals: this much extra room per letter, as a share of the size.
const TRACK: f32 = 0.12;
/// The small labels' wider tracking (field labels, table headings, chips).
const LABEL_TRACK: f32 = 0.14;
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

/// `c` seen through a modal's scrim, as egui blends it over `c`: what the
/// host's cells beside a room's modal wear, so the whole room reads as
/// under it.
pub(crate) fn under_scrim(c: Color32) -> Color32 {
    let keep = 1.0 - f32::from(SCRIM.a()) / 255.0;
    let mix = |s: u8, d: u8| (f32::from(s) + f32::from(d) * keep).round().min(255.0) as u8;
    Color32::from_rgb(mix(SCRIM.r(), c.r()), mix(SCRIM.g(), c.g()), mix(SCRIM.b(), c.b()))
}

/// `c` lifted toward white by `by` of 255, opaque.
fn lift(c: Color32, by: u8) -> Color32 {
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

/// A scroll bar's thumb as the boards draw every one (their thin
/// `scrollbar-color: #363740 transparent`): [`THUMB_W`] points wide, its
/// ends rounded, on no track, in the strong edge at rest — the dim lifted
/// to [`THUMB_REST`] over the ground, so the whole bar brightens to the dim
/// under the pointer — and the accent in the hand. The server's log draws
/// its own thumb so (server_log/egui_view.rs); egui's areas wear it
/// through [`style_once`].
pub(crate) const THUMB_W: f32 = 6.0;
/// How much of the dim a thumb at rest shows over the ground: the strong
/// edge's lift.
pub(crate) const THUMB_REST: f32 = 0.265;
/// The shortest thumb, a fingertip's.
pub(crate) const THUMB_MIN: f32 = 20.0;
/// The room a list's rows leave at their right while they overflow, for
/// the thumb over it: the thumb's [`THUMB_W`] and 4 points before it.
pub(crate) const BAR_ROOM: f32 = THUMB_W + 4.0;

/// egui's look for the desk, set once per context, which every page that
/// draws in the panel shares: dark; a scroll bar the boards' thumb
/// ([`THUMB_W`]) at the area's right edge whenever there is more than the
/// view shows, over the room the area's rows leave for it ([`BAR_ROOM`]);
/// a tooltip the canvas's, raised on a strong-edged hairline with no
/// shadow.
pub(crate) fn style_once(ctx: &egui::Context, pal: &Pal) {
    let id = Id::new("studio-styled");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_theme(egui::Theme::Dark);
    let (accent, text, ground, edge, raised, dim) = (pal.accent, pal.text, pal.ground, pal.edge_strong, pal.raised, pal.dim);
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = accent;
        style.visuals.extreme_bg_color = ground;
        style.visuals.panel_fill = ground;
        // A tooltip's box: the canvas's (Log.dc.html), padded 6 by 10.
        style.visuals.window_fill = raised;
        style.visuals.window_stroke = Stroke::new(1.0, edge);
        style.visuals.window_corner_radius = square();
        style.visuals.menu_corner_radius = square();
        style.visuals.popup_shadow = Shadow::NONE;
        style.spacing.menu_margin = egui::Margin::symmetric(10, 6);
        style.text_styles.insert(egui::TextStyle::Body, body(13.0));
        let widgets = &mut style.visuals.widgets;
        widgets.noninteractive.fg_stroke.color = text;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            state.fg_stroke.color = text;
            state.corner_radius = CornerRadius::same((THUMB_W / 2.0) as u8);
        }
        // The thumb: the dim, shown at THUMB_REST while the pointer is off
        // the bar (the strong edge over the ground) and whole on it; the
        // accent in the hand.
        widgets.inactive.bg_fill = dim;
        widgets.hovered.bg_fill = dim;
        widgets.active.bg_fill = accent;
        // Floating, so the thumb neither grows under the pointer nor takes
        // room of its own: the rows leave BAR_ROOM for it (Table, Dialog).
        // Never a track.
        style.spacing.scroll = egui::style::ScrollStyle {
            floating: true,
            bar_width: THUMB_W,
            floating_width: THUMB_W,
            floating_allocated_width: 0.0,
            bar_inner_margin: 0.0,
            bar_outer_margin: 0.0,
            handle_min_length: THUMB_MIN,
            foreground_color: false,
            dormant_handle_opacity: THUMB_REST,
            active_handle_opacity: THUMB_REST,
            interact_handle_opacity: 1.0,
            dormant_background_opacity: 0.0,
            active_background_opacity: 0.0,
            interact_background_opacity: 0.0,
            ..egui::style::ScrollStyle::floating()
        };
        style.spacing.item_spacing = vec2(8.0, 0.0);
        style.interaction.tooltip_delay = 0.0;
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// The pointing hand over a control a click acts on: the GUI shows it
/// for the panel as it does for its own cells.
pub(crate) fn hand(response: Response) -> Response {
    response.on_hover_cursor(CursorIcon::PointingHand)
}

// ── Text ──────────────────────────────────────────────────────────────────

/// How egui sets a line in a face: its ascent (from the line's top to its
/// baseline, as egui lays out with it), where a lone line's baseline
/// stands under its top (the ascent as egui rounds it), its row's height,
/// and how far over the baseline the ink of its small letters and of its
/// capitals reaches (an "x"'s and an "H"'s).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Metrics {
    pub ascent: f32,
    pub baseline: f32,
    pub row: f32,
    pub small: f32,
    pub capital: f32,
}

/// `font`'s [`Metrics`], read off a laid-out "xH" (egui keeps it). Where
/// the face draws no ink for either letter, the UI face's usual shares of
/// the size stand in.
pub(crate) fn metrics(ui: &Ui, font: &FontId) -> Metrics {
    let galley = ui.painter().layout_no_wrap("xH".to_string(), font.clone(), Color32::WHITE);
    let glyphs = galley.rows.first().map_or(&[][..], |r| &r.row.glyphs[..]);
    let ink = |k: usize, share: f32| glyphs.get(k).map(|g| -g.uv_rect.offset.y).filter(|h| *h > 0.0).unwrap_or(font.size * share);
    let ascent = glyphs.first().map_or(font.size, |g| g.font_ascent);
    Metrics {
        ascent,
        baseline: first_baseline(&galley).unwrap_or(ascent),
        row: galley.size().y,
        small: ink(0, 0.5),
        capital: ink(1, 0.7),
    }
}

/// Where a laid-out line's first glyph stands on its baseline, under the
/// galley's top. None for an empty one.
fn first_baseline(galley: &egui::Galley) -> Option<f32> {
    galley.rows.first().and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
}

/// How far under its own middle a line centred on a point has its baseline
/// when it shares the UI face's: where egui stands a line of the UI face
/// at `size` centred there. Every line the kit centres stands on it,
/// whatever its face, so a figure in the monospace sits on the baseline of
/// the words beside it rather than half its smaller descent higher.
pub(crate) fn centred_baseline(ui: &Ui, size: f32) -> f32 {
    let face = metrics(ui, &body(size));
    face.baseline - face.row / 2.0
}

/// `s` in one line no wider than `max_w`, an ellipsis where it is cut,
/// `spacing` points between letters, anchored at `pos` by `align` — a line
/// centred on `pos` standing on the UI face's baseline at its size
/// ([`centred_baseline`]), whatever its own face. Returns where it was
/// drawn.
#[allow(clippy::too_many_arguments)]
fn spaced(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, spacing: f32, max_w: f32, align: Align2) -> Rect {
    spaced_inked(ui, pos, s, font, color, spacing, max_w, align).0
}

/// [`spaced`], and where its ink fell: the span its glyphs cover, not the
/// room its line leaves over and under them (nothing for no glyph).
#[allow(clippy::too_many_arguments)]
fn spaced_inked(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, spacing: f32, max_w: f32, align: Align2) -> (Rect, Rect) {
    let restand = font.family != FontFamily::Proportional && align.y() == Align::Center;
    let size = font.size;
    let format = TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() };
    let mut job = LayoutJob::single_section(s.to_string(), format);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let mut rect = align.anchor_size(pos, galley.size());
    if restand && let Some(baseline) = first_baseline(&galley) {
        rect = rect.translate(vec2(0.0, centred_baseline(ui, size) - (baseline - galley.size().y / 2.0)));
    }
    let ink = galley.mesh_bounds.translate(rect.min.to_vec2());
    ui.painter().galley(rect.min, galley, color);
    (rect, ink)
}

/// `s` in one line, as [`spaced`] with no extra room between letters.
pub(crate) fn text(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, s, font, color, 0.0, max_w, align)
}

/// [`text`], and where its ink fell ([`spaced_inked`]).
fn text_inked(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, max_w: f32, align: Align2) -> (Rect, Rect) {
    spaced_inked(ui, pos, s, font, color, 0.0, max_w, align)
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
    small_caps_inked(ui, pos, s, color, max_w, align).0
}

/// [`small_caps`], and where its ink fell ([`spaced_inked`]).
fn small_caps_inked(ui: &Ui, pos: Pos2, s: &str, color: Color32, max_w: f32, align: Align2) -> (Rect, Rect) {
    spaced_inked(ui, pos, &s.to_uppercase(), strong(LABEL_SIZE), color, LABEL_SIZE * LABEL_TRACK, max_w, align)
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

/// `s` bare of the marks the cells set at a word's ends — a state's bullet,
/// a tick, the arrows at a button's — which the desk draws as its own
/// shapes (an LED, a tick box, the button itself) or, as the canvas's
/// buttons, not at all.
pub(crate) fn bare(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || matches!(c, '•' | '●' | '✓' | '◂' | '▸' | '◄' | '►'))
}

/// A button's words bare of the key the cells lead them with ("y copy",
/// "^T test"): the canvas's buttons carry the word alone and name the key
/// in their tooltip. The words, and the key where there was one.
pub(crate) fn unkeyed(s: &str) -> (&str, Option<&str>) {
    let s = s.trim();
    match s.split_once(' ') {
        Some((key, rest)) if !rest.trim().is_empty() && (key.chars().count() == 1 || (key.starts_with('^') && key.chars().count() == 2)) => {
            (rest.trim_start(), Some(key))
        }
        _ => (s, None),
    }
}

/// `s` cut at its start to fit `max_w` in `font`, an ellipsis standing for
/// what went, so a path keeps its leaf in view (the canvas's paths, and the
/// cells'): the line, and whether it was cut. Whole graphemes go, never a
/// part of one, so no accent or joined emoji is left torn after the "…".
pub(crate) fn cut_start(ui: &Ui, s: &str, font: FontId, max_w: f32) -> (String, bool) {
    if width_of(ui, s, font.clone()) <= max_w {
        return (s.to_string(), false);
    }
    // The fewest graphemes dropped from the head that let the rest fit:
    // the narrower the tail the likelier, so a binary search finds it.
    let starts: Vec<usize> = s.grapheme_indices(true).map(|(at, _)| at).collect();
    let tail = |k: usize| starts.get(k).map_or("", |&at| &s[at..]);
    let (mut lo, mut hi) = (1, starts.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if width_of(ui, &format!("…{}", tail(mid)), font.clone()) <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    (format!("…{}", tail(lo)), true)
}

/// One run of a line set in several faces and colours (a state word in its
/// tone, the facts plain, a figure in the monospace): its words, its face
/// and its colour.
pub(crate) type Run = (String, FontId, Color32);

/// The line heights that set runs in `fonts` on one baseline: egui stands
/// each run's baseline its own ascent over its row's foot, so runs of
/// faces with other descents ride above or below one another unless each
/// run's line height is its ascent and one depth all share. The depth is
/// the deepest descent, or what a line `line` tall leaves under the
/// tallest ascent (a paragraph's leading), so the baseline stands that
/// ascent under the line's top. A line of one face lays out as it always
/// did.
fn shared_baseline(ui: &Ui, fonts: &[&FontId], line: Option<f32>) -> Vec<f32> {
    let all: Vec<Metrics> = fonts.iter().map(|f| metrics(ui, f)).collect();
    let top = all.iter().map(|m| m.ascent).fold(0.0, f32::max);
    let depth = line.map_or_else(|| all.iter().map(|m| m.row - m.ascent).fold(0.0, f32::max), |h| h - top);
    all.iter().map(|m| (m.ascent + depth).max(1.0)).collect()
}

/// `parts` as one job, each run in its own face and colour, every run on
/// one baseline ([`shared_baseline`]).
fn run_job(ui: &Ui, parts: &[Run]) -> LayoutJob {
    let parts: Vec<&Run> = parts.iter().filter(|(s, ..)| !s.is_empty()).collect();
    let fonts: Vec<&FontId> = parts.iter().map(|(_, font, _)| font).collect();
    let heights = shared_baseline(ui, &fonts, None);
    let mut job = LayoutJob::default();
    for ((s, font, color), h) in parts.into_iter().zip(heights) {
        job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *color, line_height: Some(h), ..Default::default() });
    }
    job
}

/// How wide `parts` lay out in one line.
pub(crate) fn runs_width(ui: &Ui, parts: &[Run]) -> f32 {
    ui.painter().layout_job(run_job(ui, parts)).size().x
}

/// `parts` in one line no wider than `max_w`, cut with an ellipsis where
/// they run long, anchored at `pos` by `align`, on one baseline — a line
/// centred on `pos` standing on the UI face's at its first run's size, as
/// [`text`] stands one. Returns where it was drawn.
pub(crate) fn runs(ui: &Ui, pos: Pos2, parts: &[Run], max_w: f32, align: Align2) -> Rect {
    let mut job = run_job(ui, parts);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let mut rect = align.anchor_size(pos, galley.size());
    if align.y() == Align::Center
        && let Some((_, font, _)) = parts.iter().find(|(s, ..)| !s.is_empty())
        && let Some(baseline) = first_baseline(&galley)
    {
        rect = rect.translate(vec2(0.0, centred_baseline(ui, font.size) - (baseline - galley.size().y / 2.0)));
    }
    ui.painter().galley(rect.min, galley, Color32::WHITE);
    rect
}

/// [`runs`] standing on `baseline` rather than centred on a middle:
/// anchored at `x` by `align`, its first line's baseline on `baseline`
/// ([`mid_on`]), the prose's beside it. Returns where it was drawn.
pub(crate) fn runs_on(ui: &Ui, x: f32, baseline: f32, parts: &[Run], max_w: f32, align: Align) -> Rect {
    let size = parts.iter().find(|(s, ..)| !s.is_empty()).map_or(13.0, |(_, font, _)| font.size);
    runs(ui, pos2(x, mid_on(ui, baseline, size)), parts, max_w, Align2([align, Align::Center]))
}

/// `s` as runs in `font` and `color`, each of `figures` found in it, in
/// order, set in `figure` instead: a path, a count or a time in the grid's
/// monospace inside a room's sentence, as the canvas sets its figures. A
/// figure not found, or empty, is passed over.
pub(crate) fn with_figures(s: &str, figures: &[&str], font: FontId, figure: FontId, color: Color32) -> Vec<Run> {
    let mut parts = Vec::new();
    let mut rest = s;
    for f in figures.iter().filter(|f| !f.is_empty()) {
        if let Some(at) = rest.find(f) {
            parts.push((rest[..at].to_string(), font.clone(), color));
            parts.push((f.to_string(), figure.clone(), color));
            rest = &rest[at + f.len()..];
        }
    }
    parts.push((rest.to_string(), font, color));
    parts.retain(|(s, ..)| !s.is_empty());
    parts
}

/// One line of words, a row of the layout `h` tall and the column wide,
/// cut with an ellipsis where it runs long. Returns where it was drawn.
pub(crate) fn line(ui: &mut Ui, h: f32, s: &str, font: FontId, color: Color32) -> Rect {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    body_row(ui, row.min.y, row.max.y);
    text(ui, row.left_center(), s, font, color, w, Align2::LEFT_CENTER)
}

/// A label's row: the small capitals' 14 points.
const LABEL_H: f32 = 14.0;

/// A field's or a group's label, a [`LABEL_H`] row of the layout the column
/// wide ([`label_at`]). Returns the row.
pub(crate) fn label(ui: &mut Ui, s: &str, pal: &Pal) -> Rect {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, LABEL_H), Sense::hover());
    label_at(ui, row.left_center(), s, w, pal);
    row
}

/// A label from `pos`, its left middle, no wider than `max_w`: its words
/// up to the first "·" in dim small capitals, the rest — the hint, from
/// its "·" — dim in the UI face a space after them (the canvas's
/// "DESTINATION · another drive, …"), only where some of it fits. In a
/// [`Dialog`]'s body it is a row of its own ([`body_row`]) from the top of
/// its ink to its foot — the capitals and the hint's letters, not the room
/// over and under them — so the view ends between it and what is next to
/// it, not over a label whose capitals it never reached. Returns where it
/// was drawn.
pub(crate) fn label_at(ui: &Ui, pos: Pos2, s: &str, max_w: f32, pal: &Pal) -> Rect {
    let (head, hint) = match s.find('·') {
        Some(at) => (s[..at].trim_end(), &s[at..]),
        None => (s, ""),
    };
    let (mut drawn, mut ink) = small_caps_inked(ui, pos, head, pal.dim, max_w, Align2::LEFT_CENTER);
    let (from, end) = (drawn.max.x + 6.0, pos.x + max_w);
    if !hint.is_empty() && end - from >= 16.0 {
        let (words, words_ink) = text_inked(ui, pos2(from, pos.y), hint, body(13.0), pal.dim, end - from, Align2::LEFT_CENTER);
        (drawn, ink) = (drawn.union(words), ink.union(words_ink));
    }
    body_row(ui, ink.min.y, ink.max.y);
    drawn
}

/// The canvas's prose: words wrapped to a width, in one face and colour or
/// as several [`Run`]s, `line` times the first run's size apart (the
/// canvas's 1.3 by default), in at most `rows` lines with the last cut
/// with an ellipsis, a line that would end below `floor` left out whole
/// rather than cut through (as the cells leave out a row past the column's
/// end), a `lead` mark (a list's "−") standing in a column of its own with
/// the lines wrapped beside it, and words with no spaces (a ticket) broken
/// `anywhere` to fill each line.
pub(crate) struct Para {
    parts: Vec<Run>,
    line: f32,
    rows: usize,
    floor: f32,
    lead: Option<String>,
    anywhere: bool,
}

/// The canvas's prose leading: its body's line-height.
const PARA_LINE: f32 = 1.3;

impl Para {
    pub(crate) fn new(s: &str, font: FontId, color: Color32) -> Para {
        Para::runs(&[(s.to_string(), font, color)])
    }

    pub(crate) fn runs(parts: &[Run]) -> Para {
        Para { parts: parts.to_vec(), line: PARA_LINE, rows: usize::MAX, floor: f32::INFINITY, lead: None, anywhere: false }
    }

    /// Lines `k` times the first run's size apart.
    pub(crate) fn line(self, k: f32) -> Para {
        Para { line: k, ..self }
    }

    /// At most `n` lines, the last cut with an ellipsis.
    pub(crate) fn rows(self, n: usize) -> Para {
        Para { rows: n.max(1), ..self }
    }

    /// Lines that would end below `y` left out whole.
    pub(crate) fn floor(self, y: f32) -> Para {
        Para { floor: y, ..self }
    }

    /// `mark` in a column of its own before the words, 8 points from them,
    /// the wrapped lines hanging under the first one's words.
    pub(crate) fn lead(self, mark: &str) -> Para {
        Para { lead: Some(mark.to_string()), ..self }
    }

    /// Lines broken at any letter, each filled to the width: a ticket, whose
    /// colons and dashes would otherwise end its lines short.
    pub(crate) fn anywhere(self) -> Para {
        Para { anywhere: true, ..self }
    }

    /// A row of the layout the column wide, as tall as its lines, each
    /// line a row of a Dialog's body ([`body_row`]). Returns where they
    /// were drawn.
    pub(crate) fn show(self, ui: &mut Ui) -> Rect {
        let w = ui.available_width();
        self.show_in(ui, w)
    }

    /// [`Para::show`] wrapped at `w`, a measure of its own (a room's pitch,
    /// narrower than the column), the row no wider than the column.
    pub(crate) fn show_in(self, ui: &mut Ui, w: f32) -> Rect {
        let at = ui.cursor().min;
        let (drawn, lines) = self.paint_lines(ui, at, w);
        for line in lines {
            body_row(ui, line.min, line.max);
        }
        let (rect, _) = ui.allocate_exact_size(vec2(w.min(ui.available_width()), drawn.height()), Sense::hover());
        rect
    }

    /// Drawn from `at`, its top left, wrapped at `w`, every run of a line
    /// on one baseline ([`shared_baseline`]) — a figure in the monospace
    /// on its words' — the lead's too. Returns where.
    pub(crate) fn paint(self, ui: &Ui, at: Pos2, w: f32) -> Rect {
        self.paint_lines(ui, at, w).0
    }

    /// Where its first line stands its baseline under its top, as
    /// [`Para::paint`] lays it out: the deepest ascent among its runs, on
    /// the pixel egui rounds it to, whichever of them that line holds. What
    /// stands beside the line — words centred on its row ([`mid_on`]), a
    /// scan's share, an LED leading it — stands on this. A paragraph of no
    /// words has none, and stands it at its top.
    pub(crate) fn baseline(&self, ui: &Ui) -> f32 {
        let Some((_, font, _)) = self.parts.first() else { return 0.0 };
        let (job, _) = self.job(ui, font, font.size * self.line);
        first_baseline(&ui.painter().layout_job(job)).unwrap_or(0.0)
    }

    /// Its words as one job, lines `line_h` apart, every run of a line on
    /// one baseline ([`shared_baseline`]), and the line height the lead's
    /// mark takes in `font`, the first run's face. Its first line is a
    /// whole line tall, as the lead's is, so it stands on the deepest
    /// ascent among the runs even where the run that has it wraps to the
    /// next line.
    fn job(&self, ui: &Ui, font: &FontId, line_h: f32) -> (LayoutJob, f32) {
        let parts: Vec<&Run> = self.parts.iter().filter(|(s, ..)| !s.is_empty()).collect();
        let mut fonts: Vec<&FontId> = parts.iter().map(|(_, font, _)| font).collect();
        fonts.push(font);
        let mut heights = shared_baseline(ui, &fonts, Some(line_h));
        let lead_h = heights.pop().unwrap_or(line_h);
        let mut job = LayoutJob::default();
        for ((s, font, color), h) in parts.into_iter().zip(heights) {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *color, line_height: Some(h), ..Default::default() });
        }
        job.first_row_min_height = line_h;
        (job, lead_h)
    }

    /// [`Para::paint`], and where each of its lines stands, top to foot.
    fn paint_lines(self, ui: &Ui, at: Pos2, w: f32) -> (Rect, Vec<Rangef>) {
        let Some((_, font, color)) = self.parts.first().cloned() else { return (Rect::from_min_size(at, Vec2::ZERO), Vec::new()) };
        let line_h = font.size * self.line;
        let (mut job, lead_h) = self.job(ui, &font, line_h);
        let indent = match &self.lead {
            Some(mark) => {
                let format = TextFormat { font_id: font.clone(), color, line_height: Some(lead_h), ..Default::default() };
                let mut job = LayoutJob::single_section(mark.clone(), format);
                // A whole line tall, so the mark stands on the words' baseline.
                job.first_row_min_height = line_h;
                let galley = ui.painter().layout_job(job);
                let indent = galley.size().x + 8.0;
                if at.y + galley.size().y <= self.floor {
                    ui.painter().galley(at, galley, color);
                }
                indent
            }
            None => 0.0,
        };
        job.wrap = TextWrapping { max_width: (w - indent).max(1.0), max_rows: self.rows, break_anywhere: self.anywhere, overflow_character: Some('…') };
        let galley = ui.painter().layout_job(job);
        let origin = at + vec2(indent, 0.0);
        let rect = Rect::from_min_size(at, vec2(w, galley.size().y));
        let lines = galley.rows.iter().map(|row| Rangef::new(origin.y + row.rect().min.y, origin.y + row.rect().max.y)).collect();
        // The lines that end at or above the floor, whole.
        let fits = galley.rows.iter().map(|row| origin.y + row.rect().max.y).filter(|foot| *foot <= self.floor + 0.01).fold(origin.y, f32::max);
        if fits >= rect.max.y - 0.01 {
            ui.painter().galley(origin, galley, color);
        } else if fits > origin.y {
            ui.painter().with_clip_rect(rect.with_max_y(fits).intersect(ui.clip_rect())).galley(origin, galley, color);
        }
        (rect, lines)
    }
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
/// cut with an ellipsis where it runs long, its figures ([`figured`]: a
/// version, a count, a time) in the monospace a point under its words, as
/// the canvases set a figure in a note. Nothing at all draws nothing.
pub(crate) fn note_line(ui: &Ui, rect: Rect, busy: Option<&str>, note: Option<&(String, bool)>, pal: &Pal) {
    note_line_naming(ui, rect, busy, note, &[], pal);
}

/// A [`note_line`] whose figures take in `also` too: the names no rule can
/// tell from a word (the MP3 tab's ports).
pub(crate) fn note_line_naming(ui: &Ui, rect: Rect, busy: Option<&str>, note: Option<&(String, bool)>, also: &[&str], pal: &Pal) {
    let (words, color) = match (busy, note) {
        (Some(busy), _) => (busy, pal.accent),
        (None, Some((note, is_err))) => (note.as_str(), if *is_err { pal.gold } else { pal.dim }),
        (None, None) => return,
    };
    note_in_naming(ui, rect, words, also, color);
}

/// A note's one line in `rect`, in `color`: its words at 13 points, its
/// figures in the monospace a point under them, cut with an ellipsis, as
/// [`note_line`] sets a page's note (the transport bar's note slot, whose
/// tones are the GUI's own).
pub(crate) fn note_in(ui: &Ui, rect: Rect, words: &str, color: Color32) {
    note_in_naming(ui, rect, words, &[], color);
}

/// [`note_in`] with the names in `also` set as figures too.
fn note_in_naming(ui: &Ui, rect: Rect, words: &str, also: &[&str], color: Color32) {
    runs(ui, rect.left_center(), &figured(words, also, body(13.0), mono(12.0), color), rect.width(), Align2::LEFT_CENTER);
}

/// A room's foot as [`note_line`] draws it and, with nothing busy and no
/// note, `story` (what the cursor's row is, in the room's words) in one
/// run, cut with an ellipsis where it runs long.
pub(crate) fn foot_line(ui: &Ui, rect: Rect, busy: Option<&str>, note: Option<&(String, bool)>, story: &[Run], pal: &Pal) {
    if busy.is_some() || note.is_some() {
        note_line(ui, rect, busy, note, pal);
    } else if !story.is_empty() {
        runs(ui, rect.left_center(), story, rect.width(), Align2::LEFT_CENTER);
    }
}

/// Where a state line's words start: the LED's 6 points and the 10 after.
pub(crate) const STATE_INDENT: f32 = 16.0;

/// A room's state line across `row`: the LED lit `lamp` at its left, then
/// `parts` in one run 16 points in, cut with an ellipsis, and `right` (the
/// reload's [`cadence`]; none for nothing) at the row's end, only where the
/// whole line fits beside it. Every room's state line is this one, so the
/// words stand in one place whichever room is shown. Returns where the
/// words were drawn.
pub(crate) fn state_line(ui: &Ui, row: Rect, lamp: Color32, parts: &[Run], right: &[Run]) -> Rect {
    let mid = row.center().y;
    let lead = parts.first().map_or_else(|| body(14.0), |(_, font, _)| font.clone());
    led(ui, pos2(row.min.x + 3.0, led_mid(ui, mid, &lead, false)), lamp);
    let x = row.min.x + STATE_INDENT;
    let mut end = row.max.x;
    if !right.is_empty() {
        let right_w = runs_width(ui, right);
        if x + runs_width(ui, parts) + 16.0 + right_w <= row.max.x {
            runs(ui, pos2(row.max.x, mid), right, right_w + 1.0, Align2::RIGHT_CENTER);
            end = row.max.x - right_w - 16.0;
        }
    }
    runs(ui, pos2(x, mid), parts, (end - x).max(1.0), Align2::LEFT_CENTER)
}

/// The server's word on what a form was given (a path checked, a client
/// probed) or the form's own refusal, from `at`, its top left, `w` wide:
/// behind its `mark` — the cells' "✓" as the canvas's check (`true`), or
/// their "−" as a short rule (`false`), 6 points in, in `tone` — the words
/// wrapped beside it in up to two lines of 13.5-point `tone`, `figures` in
/// them (a version, a number) in the monospace. With no mark the words
/// start at `at`. Each of its lines is a row of a [`Dialog`]'s body, as a
/// paragraph's is ([`Para::show`]). Returns where it was drawn.
pub(crate) fn verdict(ui: &Ui, at: Pos2, w: f32, words: &str, figures: &[&str], tone: Color32, mark: Option<bool>) -> Rect {
    // The canvas's 12-point mark and the 8 points after it.
    let indent = if mark.is_some() { 20.0 } else { 0.0 };
    let parts = with_figures(words, figures, body(13.5), mono(12.5), tone);
    let (drawn, lines) = Para::runs(&parts).rows(2).paint_lines(ui, at + vec2(indent, 0.0), w - indent);
    for line in lines {
        body_row(ui, line.min, line.max);
    }
    // On the middle of the first line's small letters, over the baseline
    // its words and figures share (the words' ascent, the taller).
    let face = metrics(ui, &body(13.5));
    let centre = pos2(at.x + 6.0, at.y + face.ascent - face.small / 2.0);
    match mark {
        Some(true) => check(ui, centre, tone),
        Some(false) => {
            ui.painter().line_segment([centre - vec2(4.0, 0.0), centre + vec2(4.0, 0.0)], Stroke::new(1.7, tone));
        }
        None => {}
    }
    drawn.union(Rect::from_min_size(at, vec2(indent, 1.0)))
}

/// A room's reload cadence ("polls every 2 s"), for the right of its
/// [`state_line`]: dim, its words in the UI face and its figure — its
/// first number, its digits alone — in the monospace, as the canvas's
/// Backups board sets it; the space before the unit stays the words', so
/// it is no wider than theirs.
pub(crate) fn cadence(s: &str, pal: &Pal) -> Vec<Run> {
    with_figures(s, &[figure_in(s)], body(13.0), mono(12.0), pal.dim)
}

/// The first number in `s`: its digits and the points, commas and colons
/// between them, nothing after its last digit. Empty for none.
fn figure_in(s: &str) -> &str {
    let Some(from) = s.find(|c: char| c.is_ascii_digit()) else { return "" };
    let run = s[from..].find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | ',' | ':'))).map_or(&s[from..], |end| &s[from..from + end]);
    run.trim_end_matches(['.', ',', ':'])
}

// ── The room ──────────────────────────────────────────────────────────────

/// The note line along a room's foot, and the canvas's gap between a
/// room's blocks.
pub(crate) const NOTE_H: f32 = 18.0;
pub(crate) const ROOM_GAP: f32 = 18.0;
/// The canvas's margins round a room in its area: the top, the sides and
/// the foot.
const ROOM_TOP: f32 = 20.0;
const ROOM_SIDE: f32 = 24.0;
const ROOM_FOOT: f32 = 14.0;

/// A room's two parts in `full`, its area, inside the canvas's margins: its
/// body, then the note line along its foot a [`ROOM_GAP`] under it. Every
/// room lays itself out the same way, so a room switched to keeps its
/// blocks where the last one had them.
pub(crate) fn room_rects(full: Rect) -> (Rect, Rect) {
    let column = Rect::from_min_max(full.min + vec2(ROOM_SIDE, ROOM_TOP), (full.max - vec2(ROOM_SIDE, ROOM_FOOT)).max(full.min));
    let note = Rect::from_min_max(pos2(column.min.x, (column.max.y - NOTE_H).max(column.min.y)), column.max);
    let body = Rect::from_min_max(column.min, pos2(column.max.x, (note.min.y - ROOM_GAP).max(column.min.y)));
    (body, note)
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

/// Where an [`led`] leading words in `font` set centred on `mid` stands
/// to centre on them: on the middle of their small letters, or of their
/// capitals for words set in capitals, over the baseline [`text`] stands
/// them on — not on the middle of their line, which their ascent's room
/// for accents puts above the ink.
pub(crate) fn led_mid(ui: &Ui, mid: f32, font: &FontId, capitals: bool) -> f32 {
    let own = metrics(ui, font);
    let ink = if capitals { own.capital } else { own.small };
    mid + centred_baseline(ui, font.size) - ink / 2.0
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

/// The canvas's check in its 10-point box, centred on `at`.
pub(crate) fn check(ui: &Ui, at: Pos2, color: Color32) {
    // M1.5 5.2 l2.4 2.4 L8.6 2.6.
    let o = at - vec2(5.0, 5.0);
    let points = vec![o + vec2(1.5, 5.2), o + vec2(3.9, 7.6), o + vec2(8.6, 2.6)];
    ui.painter().add(Shape::line(points, Stroke::new(1.7, color)));
}

/// The canvas's cross in its 12-point box, centred on `at`: the cells' "✗"
/// as a mark beside words (a failed write's tab), which the panel's faces
/// have no glyph for.
pub(crate) fn cross(ui: &Ui, at: Pos2, color: Color32) {
    draw_icon(ui, at, Icon::Close, color);
}

/// A small solid triangle pointing right, centred on `at`: the cells' "▸"
/// before a folder in a list, and before the keyboard's pick.
pub(crate) fn pointer(ui: &Ui, at: Pos2, color: Color32) {
    let points = vec![at + vec2(-2.5, -4.0), at + vec2(3.5, 0.0), at + vec2(-2.5, 4.0)];
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// The canvas's small icons, drawn in 12 points with 1.6-point strokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    /// A window's close: the cross.
    Close,
    /// A row's remove: the cross, red under the pointer.
    Remove,
    /// A copy: two sheets, one over the other.
    Copy,
    /// A download: an arrow down onto a line.
    Download,
    /// An add: the plus, the top bar's [+].
    Add,
}

/// `icon` in `color`, centred on `at`.
fn draw_icon(ui: &Ui, at: Pos2, icon: Icon, color: Color32) {
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
        // M6 1.5v9M1.5 6h9.
        Icon::Add => {
            painter.line_segment([p(6.0, 1.5), p(6.0, 10.5)], stroke);
            painter.line_segment([p(1.5, 6.0), p(10.5, 6.0)], stroke);
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

/// How many segments a horizontal LED meter `width` points long holds:
/// each [`LED`] less a point wide, [`LED_GAP`] less a point apart.
fn led_count(width: f32) -> usize {
    let step = LED + LED_GAP - 1.0;
    ((width + LED_GAP - 1.0) / step).floor().max(1.0) as usize
}

/// A horizontal LED meter across `track`: `share` of its segments lit.
pub(crate) fn led_row(ui: &Ui, track: Rect, share: f32, lit_color: Color32, unlit: Color32) {
    let step = LED + LED_GAP - 1.0;
    let count = led_count(track.width());
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

/// A room on but with no one found yet (searching, connecting), as an
/// 18-point row of the layout: ten [`meter`] segments 5 by 10 points, 2
/// apart, none lit — never a share of anything — and `words` in the accent
/// 10 points after them.
pub(crate) fn searching(ui: &mut Ui, words: &str, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    searching_at(ui, row.left_center(), words, w, pal);
}

/// The searching shape's segments and the room after them.
const SEARCHING_LEAD: f32 = 10.0 * 5.0 + 9.0 * 2.0 + 10.0;

/// [`searching`] on a line of a page's own, its left middle at `at`, no
/// wider than `max_w` (the MP3 tab's scans that wait on an answer: the
/// port watch starting, GitHub asked, a fit asked). Returns where it was
/// drawn.
pub(crate) fn searching_at(ui: &Ui, at: Pos2, words: &str, max_w: f32, pal: &Pal) -> Rect {
    let bar = meter(ui, at, 10, 0, vec2(5.0, 10.0), 2.0, pal.accent, pal);
    let x = bar.max.x + 10.0;
    let room = (at.x + max_w - x).max(1.0);
    bar.union(text(ui, pos2(x, at.y), words, body(13.0), pal.accent, room, Align2::LEFT_CENTER))
}

/// How wide [`searching_at`] draws `words` whole.
pub(crate) fn searching_width(ui: &Ui, words: &str) -> f32 {
    SEARCHING_LEAD + width_of(ui, words, body(13.0))
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
    caps_width(ui, bare(label), FORM_BUTTON_SIZE) + 36.0
}

/// A form's button at `rect` (the canvas's): the one act lit amber when
/// `primary`, else a hairline in the strong edge with dim capitals that
/// brighten under the pointer. `label` is drawn [`bare`] of the cells'
/// arrows, as no board draws one on a button.
pub(crate) fn form_button(ui: &Ui, id: Id, rect: Rect, label: &str, primary: bool, pal: &Pal) -> Response {
    let label = bare(label);
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
pub(crate) const BOX: f32 = 16.0;

/// A tick box at `rect` on `fill`: a dim edge when off, the ok green's
/// edge and [`check`] when on. Under the pointer its edge brightens as the
/// boards' dim brightens to the text colour — off, the text colour's edge;
/// on, the green lifted near white — so the box shows its own hover
/// wherever it stands, beside a row's or a card's. Alone it takes no
/// click: a page's flag column can show its state while the row takes
/// the click.
pub(crate) fn tick_box(ui: &Ui, rect: Rect, on: bool, hovered: bool, fill: Color32, pal: &Pal) {
    tick_box_in(ui, rect, on, hovered, fill, pal.ok, pal);
}

/// The edge a tick box wears: its `tone` when on, else the dim; under the
/// pointer the tone lifted near white, or the text colour.
fn tick_edge(on: bool, hovered: bool, tone: Color32, pal: &Pal) -> Color32 {
    match (on, hovered) {
        (true, false) => tone,
        (true, true) => lift(tone, 70),
        (false, false) => pal.dim,
        (false, true) => pal.text,
    }
}

/// [`tick_box`] with its tick in `tone`: the ok green for a lever, the
/// dim for a flag that is only information.
fn tick_box_in(ui: &Ui, rect: Rect, on: bool, hovered: bool, fill: Color32, tone: Color32, pal: &Pal) {
    let edge = tick_edge(on, hovered, tone, pal);
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    if on {
        check(ui, rect.center(), edge);
    }
}

/// A tick box alone, centred on `at`, that takes no click of its own — a
/// table's flag the row's click stands for (Users' five, Torrents'
/// access) — on the panel's fill, its tick in `tone`, lit under the
/// pointer over its [`HIT`] square as a [`checkbox`] is, wherever its row
/// is raised. Whether the pointer is on it.
pub(crate) fn tick_mark(ui: &Ui, at: Pos2, on: bool, tone: Color32, pal: &Pal) -> bool {
    let hovered = ui.rect_contains_pointer(Rect::from_center_size(at, Vec2::splat(HIT)));
    tick_box_in(ui, Rect::from_center_size(at, Vec2::splat(BOX)), on, hovered, pal.panel, tone, pal);
    hovered
}

/// The keyboard's look on a control of a form: the raised fill and the
/// accent's ring round `rect`.
pub(crate) fn cursor_look(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_filled(rect, square(), pal.raised);
    ring(ui, rect, pal);
}

/// A tick at `rect` with its words, a form's choice or a list's (a library
/// of a grant or a ticket): a [`list_row_at`] with the box 8 points in on
/// the panel's fill and `label` beside it, cut to the row. The form's focus
/// or the list's cursor wears the keyboard's look — the raised fill, the
/// accent ring, the label semibold — and the pointer raises it; the whole
/// row takes the click (the canvas's FederationMint).
pub(crate) fn tick_row(ui: &Ui, id: Id, rect: Rect, on: bool, label: &str, focused: bool, pal: &Pal) -> Response {
    tick_row_drawn(ui, id, rect, on, label, focused, pal).0
}

/// [`tick_row`], and what of its rect it drew on: its box and its words'
/// ink, or the whole rect where it wears the keyboard's look.
fn tick_row_drawn(ui: &Ui, id: Id, rect: Rect, on: bool, label: &str, focused: bool, pal: &Pal) -> (Response, Rect) {
    let response = list_row_at(ui, id, rect, focused, pal);
    let mark = Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - BOX / 2.0), Vec2::splat(BOX));
    tick_box(ui, mark, on, response.hovered(), pal.panel, pal);
    let x = mark.max.x + 10.0;
    let font = if focused { strong(14.0) } else { body(14.0) };
    let (_, ink) = text_inked(ui, pos2(x, rect.center().y), label, font, pal.text, (rect.max.x - x - 8.0).max(1.0), Align2::LEFT_CENTER);
    (response, if focused { rect } else { mark.union(ink) })
}

/// Each line of a form's ticks a row of a [`Dialog`]'s body ([`body_row`])
/// over what its ticks drew (`drawn`, each tick's on its `line`): their
/// boxes and their words, not the room over and under them, so the view
/// never ends above a line whose ticks it shows whole; a line that has the
/// keyboard's tick, its look, whole.
fn tick_lines(ui: &Ui, drawn: &[(usize, Rect)]) {
    let lines = drawn.iter().map(|(line, _)| line + 1).max().unwrap_or(0);
    for line in 0..lines {
        let span = drawn.iter().filter(|(l, _)| *l == line).fold(Rect::NOTHING, |a, (_, r)| a.union(*r));
        body_row(ui, span.min.y, span.max.y);
    }
}

/// A form's tick's height where no board sets one — a control's [`HIT`],
/// the Users add form's and Discovery's federate form's; the mint board
/// sets its own 32 — then the room between lines of ticks, and between a
/// grid's columns (the mint form's 4 and 8).
pub(crate) const TICK_H: f32 = HIT;
const TICK_GAP: f32 = 4.0;
const TICK_COL_GAP: f32 = 8.0;
/// What a [`tick_row`] takes beside its words: its box 8 points in, the
/// 10 after the box, and 8 after the words.
const TICK_PAD: f32 = 8.0 + BOX + 10.0 + 8.0;

/// What a form's ticks' pass gives back: the one a click landed on, and
/// where each was drawn, in order (the page brings its keyboard's into
/// view).
pub(crate) struct Ticks {
    pub clicked: Option<usize>,
    pub rects: Vec<Rect>,
}

/// How wide a tick wants to be for `label`: whole even in the semibold
/// the keyboard's tick wears.
fn tick_want(ui: &Ui, label: &str) -> f32 {
    TICK_PAD + width_of(ui, label, strong(14.0))
}

/// A form's ticks in a grid (the mint form's), a row of the layout the
/// column wide and as tall as its lines: at most `most` columns 8 points
/// apart ([`grid_columns`]), in reading order, lines `tick_h` tall (the
/// mint board's 32, or [`TICK_H`] where a form has no board) and
/// [`TICK_GAP`] apart, each a [`tick_row`], `focus` in the keyboard's
/// look.
pub(crate) fn tick_grid(ui: &mut Ui, id: Id, items: &[(String, bool)], focus: Option<usize>, most: usize, tick_h: f32, pal: &Pal) -> Ticks {
    let w = ui.available_width();
    let wants: Vec<f32> = items.iter().map(|(label, _)| tick_want(ui, label)).collect();
    let cols = grid_columns(&wants, w, TICK_COL_GAP, most);
    let n = cols.len();
    let lines = items.len().div_ceil(n);
    let h = (lines as f32 * (tick_h + TICK_GAP) - TICK_GAP).max(0.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut ticks = Ticks { clicked: None, rects: Vec::new() };
    let mut drawn = Vec::new();
    for (i, (label, on)) in items.iter().enumerate() {
        let (line, col) = (i / n, i % n);
        let x = area.min.x + cols[..col].iter().sum::<f32>() + col as f32 * TICK_COL_GAP;
        let rect = Rect::from_min_size(pos2(x, area.min.y + line as f32 * (tick_h + TICK_GAP)), vec2(cols[col], tick_h));
        let (response, span) = tick_row_drawn(ui, id.with(i), rect, *on, label, focus == Some(i), pal);
        if response.clicked() {
            ticks.clicked = Some(i);
        }
        drawn.push((line, span));
        ticks.rects.push(rect);
    }
    tick_lines(ui, &drawn);
    ticks
}

/// A grid's column widths for ticks that want `wants` each, in at most
/// `most` columns `gap` apart across `room`: even columns, as the boards'
/// grids are, where each column's widest tick fits its share; else each
/// column as wide as its widest tick and the room left shared among them;
/// else a column fewer, and so on — so a tick is cut only where one
/// column cannot hold it.
fn grid_columns(wants: &[f32], room: f32, gap: f32, most: usize) -> Vec<f32> {
    let most = most.min(wants.len()).max(1);
    for n in (1..=most).rev() {
        let gaps = gap * (n - 1) as f32;
        let widest: Vec<f32> = (0..n).map(|c| wants.iter().skip(c).step_by(n).fold(0.0_f32, |a, w| a.max(*w))).collect();
        let even = (room - gaps) / n as f32;
        if widest.iter().all(|w| *w <= even) {
            return vec![even; n];
        }
        let need = widest.iter().sum::<f32>() + gaps;
        if need <= room {
            let spare = (room - need) / n as f32;
            return widest.iter().map(|w| w + spare).collect();
        }
    }
    vec![room.max(0.0)]
}

/// A form's ticks in a flow (the Users add form's), a row of the layout
/// the column wide and as tall as its lines: each tick as wide as its
/// words want, its box at the column's edge and its look reaching 8 points
/// past it either side, 4 points apart, a tick that would pass the
/// column's end starting a line [`TICK_GAP`] under the last ([`flow`]), so
/// none is left off; each a [`tick_row`], `focus` in the keyboard's look.
pub(crate) fn tick_flow(ui: &mut Ui, id: Id, items: &[(String, bool)], focus: Option<usize>, pal: &Pal) -> Ticks {
    let w = ui.available_width();
    let wants: Vec<f32> = items.iter().map(|(label, _)| tick_want(ui, label)).collect();
    let placed = flow(&wants, w + 16.0, 4.0);
    let lines = placed.last().map_or(0, |(line, ..)| line + 1);
    let h = (lines as f32 * (TICK_H + TICK_GAP) - TICK_GAP).max(0.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let mut ticks = Ticks { clicked: None, rects: Vec::new() };
    let mut drawn = Vec::new();
    for (i, ((label, on), (line, x, tw))) in items.iter().zip(placed).enumerate() {
        let rect = Rect::from_min_size(pos2(area.min.x - 8.0 + x, area.min.y + line as f32 * (TICK_H + TICK_GAP)), vec2(tw, TICK_H));
        let (response, span) = tick_row_drawn(ui, id.with(i), rect, *on, label, focus == Some(i), pal);
        if response.clicked() {
            ticks.clicked = Some(i);
        }
        drawn.push((line, span));
        ticks.rects.push(rect);
    }
    tick_lines(ui, &drawn);
    ticks
}

/// Where a flow of ticks wanting `wants` each stands in lines `room`
/// wide, `gap` apart: each tick's line, its left and its width. A tick
/// that would pass the line's end starts the next; one wider than a line
/// takes one, cut to it.
fn flow(wants: &[f32], room: f32, gap: f32) -> Vec<(usize, f32, f32)> {
    let (mut line, mut x) = (0, 0.0);
    wants
        .iter()
        .map(|want| {
            if x > 0.0 && x + want > room {
                (line, x) = (line + 1, 0.0);
            }
            let w = want.min(room);
            let placed = (line, x, w);
            x += w + gap;
            placed
        })
        .collect()
}

/// How tall a line of words a row stands on its middle inks: a 16-point
/// mark or a line of words up to 14 points, and a point over and under.
pub(crate) const WORDS_BAND: f32 = BOX + 2.0;

/// A line of words (or a 16-point mark) standing on `mid`, a row of a
/// [`Dialog`]'s body ([`body_row`]): the [`WORDS_BAND`] it inks, not the
/// room its row leaves over and under it, so the view never ends above a
/// line it shows whole. A room's own rows of words say so with it (a
/// sheet's facts, a list's last word).
pub(crate) fn words_row(ui: &Ui, mid: f32) {
    body_row(ui, mid - WORDS_BAND / 2.0, mid + WORDS_BAND / 2.0);
}

/// A row of a list in a modal, `h` points of the layout the column wide,
/// that takes the click: raised under the pointer, the keyboard's look
/// when `on` (its cursor), the hand over it. The page paints its words in
/// the rect, on its middle; whether the pointer is on it is the
/// response's. In a [`Dialog`]'s body its row is the line its words stand
/// on ([`words_row`]), the cursor's its whole rect, its look whole.
pub(crate) fn list_row(ui: &mut Ui, id: Id, h: f32, on: bool, pal: &Pal) -> (Rect, Response) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    if on || h <= WORDS_BAND {
        body_row(ui, rect.min.y, rect.max.y);
    } else {
        words_row(ui, rect.center().y);
    }
    let response = list_row_at(ui, id, rect, on, pal);
    (rect, response)
}

/// [`list_row`] at `rect`.
fn list_row_at(ui: &Ui, id: Id, rect: Rect, on: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    if on {
        cursor_look(ui, rect, pal);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, square(), pal.raised);
    }
    response
}

/// The ring round what the layout drew since `top` — a group of choices
/// that has the keyboard as one — reaching 8 points past the column either
/// side, as a [`radio`] row's hover does. Returns where.
pub(crate) fn ring_since(ui: &Ui, top: f32, pal: &Pal) -> Rect {
    let column = ui.max_rect();
    let rect = Rect::from_min_max(pos2(column.min.x - 8.0, top - 2.0), pos2(column.max.x + 8.0, ui.cursor().min.y + 2.0));
    ring(ui, rect, pal);
    rect
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
/// words take the click, not the row's empty end; while `focused` (the
/// form's keyboard on it) they wear the keyboard's look, reaching 6 points
/// past them either side, the label semibold. In a [`Dialog`]'s body it is
/// a row ([`body_row`]) from its box's and its words' top to their foot,
/// not the room over and under them, so the view never ends above a line
/// it shows whole; the keyboard's line, its look, whole.
pub(crate) fn check_line(ui: &mut Ui, id: Id, on: bool, label: &str, detail: &str, focused: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
    let (response, ink) = check_line_drawn(ui, id, row, on, label, detail, focused, pal);
    let span = if focused { row } else { ink };
    body_row(ui, span.min.y, span.max.y);
    response
}

/// [`check_line`] across `row`, a page's own row of a height its form
/// sets (the MP3 tab's Advanced sheet's erase line, its board gate's): the
/// box at the row's left on its middle, the detail left out where the label
/// leaves it no room.
#[allow(clippy::too_many_arguments)]
pub(crate) fn check_line_at(ui: &Ui, id: Id, row: Rect, on: bool, label: &str, detail: &str, focused: bool, pal: &Pal) -> Response {
    check_line_drawn(ui, id, row, on, label, detail, focused, pal).0
}

/// [`check_line_at`], and where its box and its words' ink fell.
#[allow(clippy::too_many_arguments)]
fn check_line_drawn(ui: &Ui, id: Id, row: Rect, on: bool, label: &str, detail: &str, focused: bool, pal: &Pal) -> (Response, Rect) {
    let w = row.width();
    let font = if focused { strong(14.0) } else { body(14.0) };
    let words = width_of(ui, label, font.clone()) + if detail.is_empty() { 0.0 } else { width_of(ui, detail, body(14.0)) };
    let hit = Rect::from_min_size(row.min, vec2((BOX + 10.0 + words).min(w), row.height()));
    if focused {
        cursor_look(ui, hit.expand2(vec2(6.0, 0.0)), pal);
    }
    let response = hand(ui.interact(hit, id, Sense::click()));
    let mark = Rect::from_min_size(pos2(row.min.x, row.center().y - BOX / 2.0), Vec2::splat(BOX));
    tick_box(ui, mark, on, response.hovered(), pal.ground, pal);
    let x = mark.max.x + 10.0;
    let (drawn, mut ink) = text_inked(ui, pos2(x, row.center().y), label, font, pal.text, (row.max.x - x).max(1.0), Align2::LEFT_CENTER);
    if !detail.is_empty() && drawn.max.x + 1.0 < row.max.x {
        let (_, more) = text_inked(ui, pos2(drawn.max.x, row.center().y), detail, body(14.0), pal.dim, row.max.x - drawn.max.x, Align2::LEFT_CENTER);
        ink = ink.union(more);
    }
    (response, mark.union(ink))
}

/// Where a choice's words start in its row: past its 14-point mark and the
/// 10 after it. And how far its row's look reaches past the column either
/// side, as the canvas's does.
pub(crate) const RADIO_IN: f32 = 24.0;
pub(crate) const RADIO_REACH: f32 = 8.0;

/// A choice's round mark, centred on `at`, as every group of choices draws
/// it: 14 points on the ground's fill, the strong edge's ring when not
/// `on`; when on, a ring and a 6-point dot in `lit` — the accent in the
/// group that has the keyboard, the text colour in any other, as the
/// cells' focus colours their chosen mark.
pub(crate) fn radio_mark(ui: &Ui, at: Pos2, on: bool, lit: Color32, pal: &Pal) {
    ui.painter().circle_filled(at, 7.0, pal.ground);
    ui.painter().circle_stroke(at, 6.5, Stroke::new(1.0, if on { lit } else { pal.edge_strong }));
    if on {
        ui.painter().circle_filled(at, 3.0, lit);
    }
}

/// One choice of a group, a 30-point row of the layout: its
/// [`radio_mark`] in the accent, `label` (semibold when on) and `detail`
/// dim after it, cut to the row. The whole row takes the click and is
/// raised under the pointer.
pub(crate) fn radio(ui: &mut Ui, id: Id, on: bool, label: &str, detail: &str, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    body_row(ui, row.min.y, row.max.y);
    // The canvas's row reaches 8 points past the column either side.
    let hit = row.expand2(vec2(RADIO_REACH, 0.0));
    let response = hand(ui.interact(hit, id, Sense::click()));
    if response.hovered() {
        ui.painter().rect_filled(hit, square(), pal.raised);
    }
    radio_mark(ui, pos2(row.min.x + 7.0, row.center().y), on, pal.accent, pal);
    let x = row.min.x + RADIO_IN;
    let font = if on { strong(13.5) } else { body(13.5) };
    let drawn = text(ui, pos2(x, row.center().y), label, font, pal.text, row.max.x - x, Align2::LEFT_CENTER);
    if !detail.is_empty() {
        let from = drawn.max.x + 10.0;
        text(ui, pos2(from, row.center().y), detail, body(13.0), pal.dim, row.max.x - from, Align2::LEFT_CENTER);
    }
    response
}

/// The 16-point glyph a room's [`card`] leads its words with: each room's
/// own on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    /// The words alone.
    None,
    /// A folder with a plus in it (Main's "Add a music folder").
    Folder,
    /// A person with a plus beside them (Users').
    User,
    /// An arrow down into a tray (Backups').
    Tray,
    /// A ticket, notched at its sides, its stub perforated (Federation's).
    Ticket,
    /// A sheet, its corner folded, with a plus on it (Torrents' "Add a
    /// .torrent file").
    File,
}

/// `glyph` in `color`, its 16-point box centred on `at`: the canvas's
/// paths, stroked 1.5.
fn draw_glyph(ui: &Ui, at: Pos2, glyph: Glyph, color: Color32) {
    let o = at - vec2(8.0, 8.0);
    let p = |x: f32, y: f32| o + vec2(x, y);
    let stroke = Stroke::new(1.5, color);
    let painter = ui.painter();
    match glyph {
        Glyph::None => {}
        Glyph::Folder => {
            // M2 4.5h4.5l1.5 1.5H14v7H2z, and M8 7.8v3.6M6.2 9.6h3.6.
            painter.add(Shape::closed_line(vec![p(2.0, 4.5), p(6.5, 4.5), p(8.0, 6.0), p(14.0, 6.0), p(14.0, 13.0), p(2.0, 13.0)], stroke));
            painter.line_segment([p(8.0, 7.8), p(8.0, 11.4)], stroke);
            painter.line_segment([p(6.2, 9.6), p(9.8, 9.6)], stroke);
        }
        Glyph::User => {
            // A head (r 2.6 at 6,5), the shoulders' arc from 1.5,14 over to
            // 10.5,14, and M12.6 4.4v4.2M10.5 6.5h4.2.
            painter.circle_stroke(p(6.0, 5.0), 2.6, stroke);
            let arc = (0..=12).map(|k| {
                let a = std::f32::consts::PI * (1.0 + k as f32 / 12.0);
                p(6.0 + 4.5 * a.cos(), 14.0 + 4.6 * a.sin())
            });
            painter.add(Shape::line(arc.collect(), stroke));
            painter.line_segment([p(12.6, 4.4), p(12.6, 8.6)], stroke);
            painter.line_segment([p(10.5, 6.5), p(14.7, 6.5)], stroke);
        }
        Glyph::Tray => {
            // M2 9.5h12v4H2z, and M8 2v5.4M5.6 5.1L8 7.5l2.4-2.4.
            painter.rect_stroke(Rect::from_min_max(p(2.0, 9.5), p(14.0, 13.5)), 0.0, stroke, StrokeKind::Middle);
            painter.line_segment([p(8.0, 2.0), p(8.0, 7.4)], stroke);
            painter.add(Shape::line(vec![p(5.6, 5.1), p(8.0, 7.5), p(10.4, 5.1)], stroke));
        }
        Glyph::Ticket => {
            // A notch of radius 1.6 into the side at x, bulging `inward`.
            let notch = |x: f32, inward: f32| -> Vec<Pos2> {
                (0..=8)
                    .map(|k| {
                        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 8.0;
                        p(x + inward * 1.6 * a.cos(), 8.0 + 1.6 * a.sin())
                    })
                    .collect()
            };
            let mut outline = vec![p(1.5, 4.0), p(14.5, 4.0)];
            outline.extend(notch(14.5, -1.0));
            outline.extend([p(14.5, 12.0), p(1.5, 12.0)]);
            outline.extend(notch(1.5, 1.0).into_iter().rev());
            painter.add(Shape::closed_line(outline, stroke));
            painter.extend(Shape::dashed_line(&[p(10.5, 5.0), p(10.5, 11.0)], stroke, 1.5, 1.5));
        }
        Glyph::File => {
            // In the family's paths: M3.5 1.5h6l3 3v10h-9z, its fold
            // M9.5 1.5v3h3, and M8 7.2v4.6M5.7 9.5h4.6.
            painter.add(Shape::closed_line(vec![p(3.5, 1.5), p(9.5, 1.5), p(12.5, 4.5), p(12.5, 14.5), p(3.5, 14.5)], stroke));
            painter.add(Shape::line(vec![p(9.5, 1.5), p(9.5, 4.5), p(12.5, 4.5)], stroke));
            painter.line_segment([p(8.0, 7.2), p(8.0, 11.8)], stroke);
            painter.line_segment([p(5.7, 9.5), p(10.3, 9.5)], stroke);
        }
    }
}

/// A [`card`]'s height.
pub(crate) const CARD_H: f32 = 56.0;

/// A room's call to act, the column wide and [`CARD_H`] tall: a hairline in
/// `tone` with `glyph` and `label` (bare of the cells' arrows) in its
/// semibold at the centre, washed in the tone under the pointer. While not
/// `live` it is dim and takes no click: its act on its way.
pub(crate) fn card(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, tone: Color32, live: bool, pal: &Pal) -> Response {
    card_of(ui, id, label, glyph, tone, live, CARD_H, pal)
}

/// A room's add card: a [`card`] in the ok green with its room's glyph
/// (the canvas's "Add a music folder").
pub(crate) fn add_card(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, pal: &Pal) -> Response {
    card(ui, id, label, glyph, pal.ok, true, pal)
}

/// An [`add_card`] a control's [`HIT`] tall, for a page too short for the
/// tall one: the same hairline, glyph and words, as the wizard's Done
/// shortens its link cards where it has no room for theirs.
pub(crate) fn short_add_card(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, pal: &Pal) -> Response {
    card_of(ui, id, label, glyph, pal.ok, true, HIT, pal)
}

/// A [`card`] `h` points tall.
#[allow(clippy::too_many_arguments)]
fn card_of(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, tone: Color32, live: bool, h: f32, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let response = if live { hand(ui.interact(rect, id, Sense::click())) } else { ui.interact(rect, id, Sense::hover()) };
    let tone = if live { tone } else { pal.dim };
    if live && response.hovered() {
        ui.painter().rect_filled(rect, square(), tone.gamma_multiply(0.10));
    }
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, tone), StrokeKind::Inside);
    let label = bare(label);
    let font = strong(14.0);
    let lead = if glyph == Glyph::None { 0.0 } else { 26.0 };
    let words = width_of(ui, label, font.clone()).min((w - 60.0 - lead).max(0.0));
    let left = rect.center().x - (lead + words) / 2.0;
    draw_glyph(ui, pos2(left + 8.0, rect.center().y), glyph, tone);
    text(ui, pos2(left + lead, rect.center().y), label, font, tone, words + 1.0, Align2::LEFT_CENTER);
    response
}

/// A card that is one flag's tick, the column wide and 60 points tall, on
/// a [`card_ground`] (the keyboard's look while `focused`, a page's own
/// cursor over its cards): a tick box 24 points in, `label` semibold beside
/// it and `hint` dim under the label — the whole of it the click that flips
/// the flag (the canvas's Federation inbox, the wizard's Extras).
pub(crate) fn tick_card(ui: &mut Ui, id: Id, on: bool, label: &str, hint: &str, focused: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
    let response = hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    card_ground(ui, rect, hovered, focused, pal);
    let top = rect.min.y + 20.0;
    let mark = Rect::from_center_size(pos2(rect.min.x + 24.0, top), Vec2::splat(BOX));
    tick_box(ui, mark, on, hovered, pal.ground, pal);
    let x = mark.max.x + 12.0;
    let room = (rect.max.x - x - 16.0).max(1.0);
    text(ui, pos2(x, top), label, strong(14.0), pal.text, room, Align2::LEFT_CENTER);
    text(ui, pos2(x, top + 21.0), hint, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
    response
}

/// The room between a foot's buttons, beside one another and a row apart.
const FOOT_GAP: f32 = 10.0;

/// A form's foot, a row of the layout the column wide and as tall as its
/// rows ([`foot_at`]), each row of its buttons a row of a [`Dialog`]'s
/// body down to their words ([`foot_rows`]). The button clicked, counting
/// `start` first.
pub(crate) fn foot(ui: &mut Ui, id: Id, start: &[(&str, bool)], end: &[(&str, bool)], note: Option<&str>, pal: &Pal) -> Option<usize> {
    let w = ui.available_width();
    let (laid, h) = foot_rects(ui, start, end, w);
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    foot_rows(ui, row.min, &laid);
    foot_at(ui, id, row, start, end, note, pal)
}

/// Each row of a form's foot, its buttons `laid` from its top left at
/// `at`, a row of a [`Dialog`]'s body ([`body_row`]) from the buttons' top
/// to the foot of the line of words on their middle (their capitals, and a
/// refusal beside them; [`WORDS_BAND`]): the view ends over a button whose
/// words it would cut, never on a sliver of it, and lets one whose words
/// it shows whole stand, its foot under the view's until the body is
/// scrolled — a button that can be read and clicked is never hidden for
/// its edge.
fn foot_rows(ui: &Ui, at: Pos2, laid: &[Rect]) {
    let words = FORM_BUTTON_H / 2.0 + WORDS_BAND / 2.0;
    let mut tops: Vec<f32> = laid.iter().filter(|r| r.is_positive()).map(|r| r.min.y).collect();
    tops.sort_by(f32::total_cmp);
    tops.dedup_by(|a, b| (*a - *b).abs() < 0.01);
    for top in tops {
        body_row(ui, at.y + top, at.y + top + words);
    }
}

/// A modal's foot in `area`, its rows from its top as [`foot_rects`] lays
/// them: form buttons, each as wide as its [`bare`] words and 10 apart, the lit
/// one where its flag is set — `start` laid from the left, `end` ending at
/// the right, a button that will not fit starting a row of its own — and
/// with nothing at the start, `note` (a refusal) in gold at the first
/// row's left, cut 16 points short of its first button. Button `k`,
/// counting `start` first, is `id.with(k)`. The button clicked.
fn foot_at(ui: &Ui, id: Id, area: Rect, start: &[(&str, bool)], end: &[(&str, bool)], note: Option<&str>, pal: &Pal) -> Option<usize> {
    let (laid, _) = foot_rects(ui, start, end, area.width());
    let mut clicked = None;
    for (k, ((label, primary), at)) in start.iter().chain(end).zip(&laid).enumerate() {
        if form_button(ui, id.with(k), at.translate(area.min.to_vec2()), label, *primary, pal).clicked() {
            clicked = Some(k);
        }
    }
    if let Some(note) = note.filter(|_| start.is_empty()) {
        let first = laid.iter().filter(|at| at.min.y <= 0.0).map(|at| at.min.x).fold(area.width() + FOOT_GAP, f32::min);
        let room = first - 16.0;
        if room > 16.0 {
            text(ui, pos2(area.min.x, area.min.y + FORM_BUTTON_H / 2.0), note, body(13.0), pal.gold, room, Align2::LEFT_CENTER);
        }
    }
    clicked
}

/// Where a foot `w` points wide lays its form buttons ([`foot_layout`]
/// with each one's [`form_button_width`]): each button's rect from the
/// foot's top left, counting `start` first, and the foot's height.
pub(crate) fn foot_rects(ui: &Ui, start: &[(&str, bool)], end: &[(&str, bool)], w: f32) -> (Vec<Rect>, f32) {
    let widths = |buttons: &[(&str, bool)]| buttons.iter().map(|(label, _)| form_button_width(ui, label)).collect::<Vec<f32>>();
    foot_layout(&widths(start), &widths(end), w)
}

/// Where buttons `start` and `end` wide each stand in a foot `w` wide, from
/// its top left, counting `start` first, and the foot's height: `start`
/// from the left and `end` ending at the right, [`FOOT_GAP`] apart and all
/// on one row where they fit, as they do on every board's page. A start
/// button that would run past the row's end starts a row of its own under
/// the last, from the left; an end button that would run into what stands
/// left of it starts one, ending at the right, so the one that ends a foot
/// (a gate's going through) keeps the first row and the rest come under it.
/// A button wider than the foot takes a row and is cut to it. Never one
/// past the window's edge, where its clip would cut it through a letter.
fn foot_layout(start: &[f32], end: &[f32], w: f32) -> (Vec<Rect>, f32) {
    let at = |x: f32, row: usize, bw: f32| Rect::from_min_size(pos2(x, row as f32 * (FORM_BUTTON_H + FOOT_GAP)), vec2(bw, FORM_BUTTON_H));
    let mut laid = vec![Rect::NOTHING; start.len() + end.len()];
    let (mut row, mut x) = (0, 0.0);
    for (k, want) in start.iter().enumerate() {
        let bw = want.min(w);
        if x > 0.0 && x + bw > w {
            (row, x) = (row + 1, 0.0);
        }
        laid[k] = at(x, row, bw);
        x += bw + FOOT_GAP;
    }
    // The end's buttons from the right of the row the start ended on, clear
    // of its last button.
    let (mut left, mut right) = (x, w);
    for (k, want) in end.iter().enumerate().rev() {
        let bw = want.min(w);
        if right - bw < left - 0.01 && (left > 0.0 || right < w) {
            (row, left, right) = (row + 1, 0.0, w);
        }
        laid[start.len() + k] = at(right - bw, row, bw);
        right -= bw + FOOT_GAP;
    }
    let rows = if laid.is_empty() { 1 } else { row + 1 };
    (laid, rows as f32 * FORM_BUTTON_H + (rows - 1) as f32 * FOOT_GAP)
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
    /// A filter's box: the canvas's magnifier 10 points in, the line 30.
    pub search: bool,
    /// The value in the accent while focused, as the caret is: a line a
    /// sheet opens in a row's place to edit it (the MP3 tab's Name and path
    /// fields), which the cells set in the accent.
    pub lit: bool,
}

impl<'a> Input<'a> {
    pub(crate) fn new(value: &'a str, caret: usize) -> Self {
        Input { value, caret, focused: false, composition: "", masked: false, mono: false, enabled: true, placeholder: "", search: false, lit: false }
    }

    /// One of a page's own inputs: its value and its caret.
    pub(crate) fn of(input: &'a tui_input::Input) -> Self {
        Input::new(input.value(), input.cursor())
    }

    /// The field has the keyboard, and the input method is composing
    /// `composition` (empty for nothing). A field without the keyboard
    /// draws no composition, whatever it is handed, so a page may hand
    /// every field the one it has.
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

    pub(crate) fn search(self) -> Self {
        Input { search: true, ..self }
    }

    pub(crate) fn lit(self) -> Self {
        Input { lit: true, ..self }
    }
}

/// What a field's pass gives back.
pub(crate) struct Typed {
    /// The box's response: a click on it is the page's to focus the field.
    pub response: Response,
    /// The foot of the caret, in egui's points, while the field is focused:
    /// where the host floats the input method's candidates.
    pub caret: Option<Pos2>,
}

/// A field with its label over it: the label's row, 6 points, the box.
pub(crate) const FIELD_ROW_H: f32 = LABEL_H + 6.0 + FIELD_H;

/// A field as the canvas draws it, a [`FIELD_ROW_H`] row of the layout the
/// full width: its [`label`], then its box ([`field_box`]).
pub(crate) fn field(ui: &mut Ui, id: Id, label: &str, input: &Input, pal: &Pal) -> Typed {
    let w = ui.available_width();
    let (slot, _) = ui.allocate_exact_size(vec2(w, FIELD_ROW_H), Sense::hover());
    field_at(ui, id, slot, label, input, pal)
}

/// [`field`] in `slot`, a [`FIELD_ROW_H`] rect: a form's fields side by
/// side. In a [`Dialog`]'s body its label and its box are two rows
/// ([`body_row`]), as [`field_beside`]'s are: a view whose top cuts the
/// label leaves the box standing whole under it, its caret in sight,
/// rather than taking the box away with the label.
pub(crate) fn field_at(ui: &Ui, id: Id, slot: Rect, label: &str, input: &Input, pal: &Pal) -> Typed {
    label_at(ui, pos2(slot.min.x, slot.min.y + LABEL_H / 2.0), label, slot.width(), pal);
    let rect = Rect::from_min_size(pos2(slot.min.x, slot.max.y - FIELD_H), vec2(slot.width(), FIELD_H));
    body_row(ui, rect.min.y, rect.max.y);
    field_box(ui, id, rect, input, pal)
}

/// A field whose value is short (a name, a number), a row of the layout:
/// its [`label`], then a box `box_w` wide (less where the column is
/// narrower) and `note` dim 16 points beside it where it fits whole, else
/// on a line of its own under it (the canvas's name and its charset).
pub(crate) fn field_beside(ui: &mut Ui, id: Id, label: &str, input: &Input, box_w: f32, note: &str, pal: &Pal) -> Typed {
    let w = ui.available_width();
    self::label(ui, label, pal);
    ui.add_space(6.0);
    let box_w = box_w.min(w);
    let (row, _) = ui.allocate_exact_size(vec2(w, FIELD_H), Sense::hover());
    body_row(ui, row.min.y, row.max.y);
    let typed = field_box(ui, id, Rect::from_min_size(row.min, vec2(box_w, FIELD_H)), input, pal);
    if !note.is_empty() {
        let note_w = width_of(ui, note, body(13.0));
        if box_w + 16.0 + note_w <= w {
            let at = pos2(row.min.x + box_w + 16.0, row.center().y);
            text(ui, at, note, body(13.0), pal.dim, note_w + 1.0, Align2::LEFT_CENTER);
        } else {
            ui.add_space(6.0);
            line(ui, 18.0, note, body(13.0), pal.dim);
        }
    }
    typed
}

/// A field's box at `rect`: the ground's fill in a strong-edged hairline,
/// the accent's while focused and a brighter one under the pointer (the
/// pointer wins); the value in the UI face (or the monospace), in the
/// accent while a lit field is focused, bullets for a masked one, a
/// composition underlined before the caret, and a steady 1-point caret
/// while focused. The line is windowed so the caret stays in view, the
/// window kept from pass to pass under `id` as a text field's is.
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
        (pal.ground, edge, if focused && input.lit { pal.accent } else { pal.text })
    } else {
        (pal.panel, pal.edge, pal.dim)
    };
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);

    let mut view = rect.shrink2(vec2(FIELD_PAD, 0.0));
    if input.search {
        // The canvas's 14-point magnifier: a ring r 4.2 at 6,6 and its
        // handle to 12.5,12.5, stroked 1.4, the words 30 points in.
        let o = pos2(rect.min.x + 10.0, rect.center().y - 7.0);
        let stroke = Stroke::new(1.4, pal.dim);
        ui.painter().circle_stroke(o + vec2(6.0, 6.0), 4.2, stroke);
        ui.painter().line_segment([o + vec2(9.2, 9.2), o + vec2(12.5, 12.5)], stroke);
        view.min.x = rect.min.x + 30.0;
    }

    let font = if input.mono { mono(12.5) } else { body(14.0) };
    let (line, caret_at, composing) = field_line(input);
    let clip = ui.painter().with_clip_rect(view.intersect(ui.clip_rect()));
    // A line's first letter may reach left of where it stands (a "V", a
    // "W" or a quote in some faces, Tahoma's among them): where the line
    // starts at the view's edge, the clip leaves it half the box's padding,
    // so its ink is never cut there.
    let reach = ui.painter().with_clip_rect(view.with_min_x(view.min.x - FIELD_PAD / 2.0).intersect(ui.clip_rect()));
    if line.is_empty() && !input.placeholder.is_empty() {
        let galley = ui.painter().layout_no_wrap(input.placeholder.to_string(), font.clone(), pal.dim);
        reach.galley(pos2(view.min.x, rect.center().y - galley.size().y / 2.0), galley, pal.dim);
    }
    let mut job = field_job(&line, composing, font.clone(), ink);
    if !focused {
        // Unfocused, a long line is cut with an ellipsis, as the cells cut
        // it, never cut through a letter at the box's edge.
        job.wrap = TextWrapping::truncate_at_width(view.width().max(1.0));
    }
    let galley = ui.painter().layout_job(job);
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
    // A letter scrolled partly off the view's left edge is left out whole,
    // as the cells' field scrolls by whole letters: none shows torn.
    if offset > 0.0 {
        let from = view.min.x + whole_glyph(&galley, offset) - offset;
        clip.with_clip_rect(clip.clip_rect().with_min_x(from.max(clip.clip_rect().min.x))).galley(origin, galley, ink);
    } else {
        reach.galley(origin, galley, ink);
    }

    let caret = focused.then(|| {
        let x = (origin.x + caret_x).round();
        let h = font.size + 5.0;
        let bar = Rect::from_min_size(pos2(x, rect.center().y - h / 2.0), vec2(1.0, h));
        ui.painter().rect_filled(bar, 0.0, pal.accent);
        pos2(x, bar.max.y)
    });
    Typed { response, caret }
}

/// Where the first letter whole in view starts on a field's line scrolled
/// `offset` points left: the next letter's left edge from there.
fn whole_glyph(galley: &egui::Galley, offset: f32) -> f32 {
    if offset <= 0.0 {
        return 0.0;
    }
    let row = galley.rows.first();
    row.and_then(|r| r.row.glyphs.iter().map(|g| r.pos.x + g.pos.x).find(|x| *x >= offset - 0.01)).unwrap_or(offset)
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
fn scroll_to_caret(offset: f32, caret_x: f32, line_w: f32, view_w: f32) -> f32 {
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
pub(crate) const COL_GAP: f32 = 16.0;
pub(crate) const CELL_PAD: f32 = 10.0;
/// How far a room's table's first row stands under its top: the 30-point
/// headings and the 4 points under them.
pub(crate) const TABLE_HEAD: f32 = 34.0;

/// The columns' x ranges across `x0..x1`.
fn spans(x0: f32, x1: f32, cols: &[Col]) -> Vec<(f32, f32)> {
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

/// A view `room` points tall cut down to whole rows `row_h` tall, so a
/// table never ends on a sliver of one; the room as it is where not one
/// row fits it.
fn whole_rows(room: f32, row_h: f32) -> f32 {
    let n = ((room + 0.01) / row_h.max(1.0)).floor();
    if n >= 1.0 { n * row_h } else { room }
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
    unmarked: Option<usize>,
    look: Look,
    row_h: f32,
    trail: f32,
    clickable: bool,
    reveal: Option<usize>,
    empty: Option<&'a str>,
    headless: bool,
    height: Option<f32>,
    content: Option<Id>,
}

/// What a table's pass saw.
#[derive(Default)]
pub(crate) struct TableShown {
    /// The row a click landed on, if one did.
    pub row: Option<usize>,
    /// The last row on screen, for paging.
    pub last_on_screen: usize,
}

impl<'a> Table<'a> {
    /// The canvas's table: 30-point headings in small capitals over a
    /// hairline, 40-point rows, the cursor's row raised and ringed in the
    /// accent, a row under the pointer raised, a click on a row reported.
    pub(crate) fn new(id: Id, cols: &'a [Col], len: usize) -> Self {
        Table {
            id,
            cols,
            len,
            cursor: None,
            unmarked: None,
            look: Look::Ring,
            row_h: 40.0,
            trail: 0.0,
            clickable: true,
            reveal: None,
            empty: None,
            headless: false,
            height: None,
            content: None,
        }
    }

    /// The keyboard cursor's row.
    pub(crate) fn cursor(self, cursor: Option<usize>) -> Self {
        Table { cursor, ..self }
    }

    /// Row `row` drawn as no cursor's row though the cursor is on it, a
    /// row whose own control is its mark for now (a name being renamed in
    /// its field): the cursor is still the cursor, so the view moves with
    /// it as it moves and stands where it stands when the mark comes back.
    pub(crate) fn unmarked(self, row: Option<usize>) -> Self {
        Table { unmarked: row, ..self }
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
    /// The trail is no part of the row's target, as the cells register a
    /// row's click left of its remove column: a click there beside the
    /// button selects nothing, and shows no hand.
    pub(crate) fn trail(self, trail: f32) -> Self {
        Table { trail, ..self }
    }

    /// Rows that take no click as a whole, only through the controls the
    /// page draws in them (the canvas's Libraries): no hover, no row click.
    pub(crate) fn inert(self) -> Self {
        Table { clickable: false, ..self }
    }

    /// Row `reveal` brought into view on this pass, just far enough, the
    /// cursor left where it is: a fresh add's row, which the cells show
    /// without selecting it.
    pub(crate) fn reveal(self, reveal: Option<usize>) -> Self {
        Table { reveal, ..self }
    }

    /// With no rows, the headings and `words` dim on the first row's line,
    /// as the cells say an empty list is empty, the table no taller than
    /// that; "" for the headings alone, the page saying more under them.
    pub(crate) fn empty(self, words: &'a str) -> Self {
        Table { empty: Some(words), ..self }
    }

    /// No headings and no rule: a list.
    pub(crate) fn headless(self) -> Self {
        Table { headless: true, ..self }
    }

    /// The rows' view `h` points tall, rather than all the layout has left.
    pub(crate) fn height(self, h: f32) -> Self {
        Table { height: Some(h), ..self }
    }

    /// What the rows are (a folder's listing, a destination's runs): a new
    /// `key`, or a table that was not drawn on the last pass, starts at its
    /// top and brings its cursor in from there, as the cells' lists open —
    /// never where the wheel last left it, nor at another listing's place.
    pub(crate) fn content(self, key: Id) -> Self {
        Table { content: Some(key), ..self }
    }

    /// Lay the table out in what is left of `ui`, `draw_row` painting each
    /// row on screen: its view as many whole rows tall as fit, so it never
    /// ends on a sliver of one, and while the rows overflow it, they and
    /// their headings [`BAR_ROOM`] short of its right edge, where the
    /// thumb stands clear of the cursor's ring. A wheel's notch moves the
    /// view a row.
    pub(crate) fn show(self, ui: &mut Ui, pal: &Pal, mut draw_row: impl FnMut(&Ui, usize, &RowCtx)) -> TableShown {
        let Table { id, cols, len, cursor, unmarked, look, row_h, trail, clickable, reveal, empty, headless, height, content } = self;
        let w = ui.available_width();
        let head_h = match (headless, look) {
            (true, _) => 0.0,
            (false, Look::Lit) => 26.0,
            (false, Look::Ring) => 30.0,
        };
        let below = if headless { 0.0 } else { head_h + 4.0 };
        let room_h = height.unwrap_or_else(|| ui.available_height() - below);
        let room_h = if room_h.is_finite() { room_h.max(0.0) } else { len as f32 * row_h };
        let view_h = whole_rows(room_h, row_h);
        let overflows = len as f32 * row_h > view_h + 0.5;
        let bar = if overflows { BAR_ROOM } else { 0.0 };
        if !headless {
            let (head, _) = ui.allocate_exact_size(vec2(w, head_h), Sense::hover());
            let cells = spans(head.min.x + CELL_PAD, head.max.x - CELL_PAD - trail - bar, cols);
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
        }
        if len == 0
            && let Some(words) = empty
        {
            if !words.is_empty() {
                let (row, _) = ui.allocate_exact_size(vec2(w, row_h), Sense::hover());
                let at = pos2(row.min.x + CELL_PAD, row.center().y);
                text(ui, at, words, body(14.0), pal.dim, (w - 2.0 * CELL_PAD).max(1.0), Align2::LEFT_CENTER);
            }
            return TableShown::default();
        }

        // Where the view should stand: a fresh listing at its top first,
        // then the cursor brought in on the pass it moved and a revealed row
        // on the pass it is asked for, each just far enough.
        let memory = id.with("view");
        let now = ui.ctx().cumulative_pass_nr();
        let (last_pass, last_content, last_cursor, last_offset): (u64, Id, Option<usize>, f32) =
            ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((0, Id::NULL, None, 0.0));
        let fresh = content.is_some_and(|key| key != last_content || last_pass + 1 != now);
        let (mut offset, mut moved) = if fresh { (0.0, true) } else { (last_offset, false) };
        let mut bring = |row: usize| {
            let (top, bottom) = (row as f32 * row_h, (row + 1) as f32 * row_h);
            if top < offset {
                (offset, moved) = (top, true);
            } else if bottom > offset + view_h {
                (offset, moved) = (bottom - view_h, true);
            }
        };
        if let Some(s) = cursor.filter(|_| fresh || cursor != last_cursor) {
            bring(s);
        }
        if let Some(r) = reveal {
            bring(r);
        }
        // A notch a row, as the cells' wheel steps their lists.
        let notch = ui.ctx().options(|o| o.input_options.line_scroll_speed).max(1.0);
        let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).wheel_scroll_multiplier(vec2(1.0, row_h / notch));
        if height.is_some() || overflows {
            scroll = scroll.max_height(view_h).min_scrolled_height(view_h);
        }
        if moved {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        let mut shown = TableShown::default();
        let sense = if clickable { Sense::click() } else { Sense::hover() };
        let output = scroll.show_rows(ui, row_h, len, |ui, range| {
            shown.last_on_screen = range.end.saturating_sub(1);
            for i in range {
                let (full, _) = ui.allocate_exact_size(vec2(ui.available_width() - bar, row_h), Sense::hover());
                let rect = Rect::from_min_max(full.min, pos2((full.max.x - trail).max(full.min.x), full.max.y));
                let trail_rect = Rect::from_min_max(pos2(rect.max.x, full.min.y), full.max);
                let response = ui.interact(rect, id.with("row").with(i), sense);
                // The Stats page's controls show no hand yet, its rows
                // among them; the rooms' do.
                let response = if clickable && look == Look::Ring { hand(response) } else { response };
                let selected = cursor == Some(i) && unmarked != Some(i);
                // A room's row stays raised while the pointer is on a
                // control the page draws in it (a tick box), which egui
                // hovers in its stead: the row would blink out under it.
                // Nothing covers a Stats row.
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
                }
            }
        });
        let state = (now, content.unwrap_or(Id::NULL), cursor, output.state.offset.y);
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

/// A row's name: semibold on the cursor's row alone (the canvas's), in the
/// text colour, white under the pointer ([`ink`]).
pub(crate) fn name_cell(ui: &Ui, row: &RowCtx, at: usize, s: &str, pal: &Pal) -> Rect {
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    cell(ui, row, at, s, font, ink(row, pal.text, pal), false)
}

/// A cell that is words behind an LED (a status, a confidence, a last
/// run's outcome): the LED lit `lamp` 3 points in where the cells draw a
/// glyph, the words 14 points in; with no lamp, the words from the cell's
/// left.
pub(crate) fn led_cell(ui: &Ui, row: &RowCtx, at: usize, lamp: Option<Color32>, words: &str, font: FontId, color: Color32) -> Rect {
    let (x, w) = row.cells[at];
    let mid = row.rect.center().y;
    let from = match lamp {
        Some(lamp) => {
            led(ui, pos2(x + 3.0, led_mid(ui, mid, &font, false)), lamp);
            x + 14.0
        }
        None => x,
    };
    text(ui, pos2(from, mid), words, font, color, (x + w - from).max(1.0), Align2::LEFT_CENTER)
}

/// A cell whose words may not fit: cut with an ellipsis, and `tip` in a
/// tooltip over the cell only when they were cut, as the cells register
/// one over a clipped cell.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cell_tipped(ui: &Ui, row: &RowCtx, at: usize, s: &str, tip: &str, font: FontId, color: Color32, id: Id) -> Rect {
    let drawn = cell(ui, row, at, s, font.clone(), color, false);
    let rect = row.cell(at);
    if width_of(ui, s, font) > rect.width() {
        cell_tip(&ui.interact(rect, id, Sense::hover()), tip);
    }
    drawn
}

/// A path's cell: cut at its start ([`cut_start`]) so its leaf stays, and
/// whole in a tooltip, in the cell's own face, only when it was cut.
pub(crate) fn path_cell(ui: &Ui, row: &RowCtx, at: usize, path: &str, font: FontId, color: Color32, id: Id) -> Rect {
    let rect = row.cell(at);
    let (shown, cut) = cut_start(ui, path, font.clone(), rect.width());
    let drawn = text(ui, rect.left_center(), &shown, font.clone(), color, rect.width(), Align2::LEFT_CENTER);
    if cut {
        cell_tip(&ui.interact(rect, id, Sense::hover()), egui::RichText::new(path).font(font));
    }
    drawn
}

/// A cut cell's tooltip under the pointer, where the canvas sets its
/// tooltips (the log's: 8 points under the control, their right edges
/// one): under the cell, its right edge at the cell's, so it reaches left
/// over the cells before it — words — and covers none of the next row's
/// controls in the columns after it; over the cell where there is no room
/// under it.
fn cell_tip(response: &Response, tip: impl Into<egui::WidgetText>) {
    let mut tooltip = egui::Tooltip::for_enabled(response);
    tooltip.popup = tooltip.popup.align(egui::RectAlign::BOTTOM_END).align_alternatives(&[egui::RectAlign::TOP_END]).gap(8.0);
    let tip = tip.into();
    let _ = tooltip.show(|ui| {
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(egui::Label::new(tip));
    });
}

/// `cols` for a table `w` wide: their fixed widths narrowed together, to
/// no less than `floor` of themselves, where the flexible column would
/// have less than `least` (the default window is narrower than the
/// canvas's 1440).
pub(crate) fn fitted(w: f32, cols: Vec<Col>, least: f32, floor: f32) -> Vec<Col> {
    let gaps = COL_GAP * cols.len().saturating_sub(1) as f32 + 2.0 * CELL_PAD;
    let fixed: f32 = cols.iter().map(|c| c.w).sum();
    let room = w - gaps - least;
    if fixed <= room || fixed <= 0.0 {
        return cols;
    }
    let k = (room / fixed).clamp(floor, 1.0);
    cols.into_iter().map(|c| Col { w: (c.w * k).floor(), ..c }).collect()
}

// ── Folders ───────────────────────────────────────────────────────────────

/// The server's folder browser's window, every room's that picks a folder.
pub(crate) const BROWSER_W: f32 = 640.0;
/// A row of a modal's list of folders, and the most a browser shows at
/// once where its page asks for no fewer (the cells' eleven).
pub(crate) const FOLDER_ROW_H: f32 = 30.0;
pub(crate) const BROWSE_ROWS: usize = 11;
/// Where a modal's list row stands its mark (a folder's "▸", a choice's
/// LED) and its words: the mark centred 13 points in, the words 26 in and
/// cut 10 short of the row's end.
pub(crate) const ROW_MARK_IN: f32 = 13.0;
pub(crate) const ROW_WORDS_IN: f32 = 26.0;
pub(crate) const ROW_WORDS_AFTER: f32 = 10.0;

/// A folder in a modal's list at `rect`: the cells' "▸" ([`pointer`])
/// [`ROW_MARK_IN`], in the accent on the keyboard's row (`on`), and `name`
/// [`ROW_WORDS_IN`], in `color`, the text colour on the keyboard's row.
pub(crate) fn folder_row(ui: &Ui, rect: Rect, name: &str, on: bool, color: Color32, pal: &Pal) {
    pointer(ui, pos2(rect.min.x + ROW_MARK_IN, rect.center().y), if on { pal.accent } else { pal.dim });
    let color = if on { pal.text } else { color };
    let room = (rect.width() - ROW_WORDS_IN - ROW_WORDS_AFTER).max(1.0);
    text(ui, pos2(rect.min.x + ROW_WORDS_IN, rect.center().y), name, body(14.0), color, room, Align2::LEFT_CENTER);
}

/// What a folder browser's pass gives back.
pub(crate) struct Browsed {
    /// The folder a click landed on.
    pub row: Option<usize>,
    /// The button of its foot clicked, counted from the left.
    pub button: Option<usize>,
}

/// A folder browser's path line, and the room it leaves under that line
/// and over its foot where the window gives it them.
const BROWSE_PATH_H: f32 = 18.0;
const BROWSE_UNDER_PATH: f32 = 12.0;
const BROWSE_OVER_FOOT: f32 = 20.0;

/// The body of the server's folder browser, as every page that picks a
/// folder draws it, rows of the layout: `path`, the folder listed, cut at
/// its start in the dim monospace; 12 points under it its folders as
/// [`folder_row`]s in a headless [`Table`] under `id.with("list")`, at most
/// `most` on screen (the cells' [`BROWSE_ROWS`], or fewer where a page's
/// board asks) and no more than leave its foot room under it, the
/// keyboard's `sel` ringed and kept in view and a new listing starting at
/// its top; with none, `empty` dim on the first row's line; then, 20
/// points on, its foot under `id.with("foot")`, `buttons` from the left
/// ([`foot`]). Where the window is too narrow for them on one row the foot
/// stacks, and where it is short too the list gives way to the rows that
/// adds, down to one row. Where even one row leaves the foot under the
/// body's fold, the 12 and the 20 close up as far as that takes and no
/// further ([`browser_gaps`]), so the whole foot stands in the window's
/// view wherever their 32 points are enough; at every size where one row
/// fits with them, they are as drawn.
#[allow(clippy::too_many_arguments)]
pub(crate) fn folder_browser(
    ui: &mut Ui,
    id: Id,
    path: &str,
    names: &[String],
    sel: usize,
    empty: &str,
    most: usize,
    buttons: &[(&str, bool)],
    pal: &Pal,
) -> Browsed {
    let w = ui.available_width();
    let (_, foot_h) = foot_rects(ui, buttons, &[], w);
    // The least the list takes: a row, or none for no folders and no words.
    let least = if names.is_empty() && empty.is_empty() { 0.0 } else { FOLDER_ROW_H };
    let (under_path, over_foot) = browser_gaps(ui.available_height(), least + foot_h);
    let (shown, _) = cut_start(ui, path, mono(12.5), w);
    line(ui, BROWSE_PATH_H, &shown, mono(12.5), pal.dim);
    ui.add_space(under_path);
    let rows = names.len().min(most) as f32 * FOLDER_ROW_H;
    let list = Table::new(id.with("list"), &[], names.len())
        .headless()
        .row_height(FOLDER_ROW_H)
        .height(rows.min(ui.available_height() - over_foot - foot_h).max(FOLDER_ROW_H))
        .cursor(Some(sel))
        .content(Id::new(path))
        .empty(empty);
    let picked = list.show(ui, pal, |ui, i, row| folder_row(ui, row.rect, &names[i], row.selected, ink(row, pal.text, pal), pal));
    ui.add_space(over_foot);
    let button = foot(ui, id.with("foot"), buttons, &[], None, pal);
    Browsed { row: picked.row, button }
}

/// The room a folder browser leaves under its path and over its foot in a
/// body `room` points tall, where the least its list takes and its foot
/// are `rest` tall together: the 12 and the 20 wherever the path and the
/// rest stand whole with them, as on every board's page; where they do
/// not, the two closed up together, in whole points and each in its share
/// of the 32, by what the body runs over and no more, so the foot's last
/// row ends less than a point over the body's end. Where the body runs
/// over by more than the two together, none: the body scrolls, the foot as
/// near the view as it can stand.
fn browser_gaps(room: f32, rest: f32) -> (f32, f32) {
    let gaps = BROWSE_UNDER_PATH + BROWSE_OVER_FOOT;
    let over = BROWSE_PATH_H + gaps + rest - room;
    // A hundredth's grace, for the room's sums.
    let cut = (over - 0.01).ceil().clamp(0.0, gaps);
    let over_foot = (BROWSE_OVER_FOOT * (gaps - cut) / gaps).round();
    (gaps - cut - over_foot, over_foot)
}

// ── Modals ────────────────────────────────────────────────────────────────

/// A modal window's padding, the room under its title, and the title row.
const MODAL_PAD: f32 = 24.0;
const MODAL_GAP: f32 = 20.0;
const TITLE_H: f32 = 20.0;
/// The room a modal's close takes at its title's right.
const CLOSE_ROOM: f32 = 40.0;
/// How far a modal's scrolled body reaches into the window's right
/// padding for its thumb: the thumb 10 points past the body, 8 in from the
/// window's edge.
const BODY_BAR: f32 = MODAL_PAD - 8.0;

/// What a modal's pass gives back.
pub(crate) struct ModalShown<R> {
    /// The close button was clicked: the page cancels, as on Esc.
    pub closed: bool,
    /// What the body's closure returned.
    pub inner: R,
    /// Where the window stood, so a page can tell what it leaves clear.
    pub window: Rect,
}

/// Where a modal's body stood on this pass, for [`reveal`]: the top of its
/// content and its view as it ends between rows, in the screen's points,
/// how tall its whole view is, and whether the modal came up on this pass.
#[derive(Clone, Copy)]
struct BodyView {
    origin: f32,
    view: Rect,
    full: f32,
    opened: bool,
}

/// The body a [`Dialog`] is laying out now, for [`body_row`]: the modal's
/// id, the top of its body's content and its view's clip, in the screen's
/// points.
#[derive(Clone, Copy)]
struct Laying {
    id: Id,
    origin: f32,
    clip: Rect,
}

/// Where the body being laid out is kept while its closure runs.
fn laying_key() -> Id {
    Id::new("studio-dialog-laying")
}

/// A row of the body a [`Dialog`] is laying out, from `top` to `bottom` in
/// the screen's points: a place the body's view never ends through at rest
/// (the Dialog ends it over the row's top instead, between rows). The
/// kit's own rows say so themselves (a field's label and its box, a check
/// line, a choice, a line, a label, a paragraph, a row of ticks or of a
/// list, a form's buttons down to their words) — a label, a check line and
/// a row of ticks or of a list by what they draw, not the empty room over
/// and under it, so a view that reaches only that room shows them still;
/// a body that sets its rows at places of its own says so for each
/// ([`words_row`] for a line of words). A gate's answers say none
/// ([`gate`]). A row laid out anywhere else — outside a Dialog's body, or
/// in a scrolled list of the body's own, whose view is its own — is let
/// be.
pub(crate) fn body_row(ui: &Ui, top: f32, bottom: f32) {
    let ctx = ui.ctx();
    let Some(laying) = ctx.data(|d| d.get_temp::<Laying>(laying_key())) else { return };
    if ui.clip_rect() != laying.clip || bottom <= top {
        return;
    }
    let row = Rangef::new(top - laying.origin, bottom - laying.origin);
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<Rangef>>(laying.id.with("rows-now")).push(row));
}

/// Where a view `h` tall scrolled `offset` down its content ends over the
/// body's `rows` (each from its top to its bottom in the content's points):
/// its own end, or where that falls inside a row the row's top, and so on
/// up; its own end still where the rows would leave it nothing to show.
fn row_end(rows: &[Rangef], offset: f32, h: f32) -> f32 {
    let mut end = offset + h;
    loop {
        let cut = rows.iter().filter(|r| inside(r, end)).map(|r| r.min).fold(f32::INFINITY, f32::min);
        if !cut.is_finite() {
            break;
        }
        end = cut;
    }
    if end <= offset + 0.01 { offset + h } else { end }
}

/// Where a view scrolled `offset` down its content and ending at `end`
/// starts over the body's `rows`: its own top, or where that falls inside a
/// row the row's foot, and so on down — the body scrolled as far as it
/// goes, its last row on the view's foot, can leave a row torn over its
/// top; its own top still where the rows would leave it nothing to show.
fn row_start(rows: &[Rangef], offset: f32, end: f32) -> f32 {
    let mut start = offset;
    loop {
        let cut = rows.iter().filter(|r| inside(r, start)).map(|r| r.max).fold(f32::NEG_INFINITY, f32::max);
        if !cut.is_finite() {
            break;
        }
        start = cut;
    }
    if start >= end - 0.01 { offset } else { start }
}

/// Whether an edge at `y` cuts `row`: more than half a point inside it at
/// either end, so the scrolled view's rounding to the pixel never reads as a
/// cut.
fn inside(row: &Rangef, y: f32) -> bool {
    row.min + 0.5 < y && y < row.max - 0.5
}

/// A modal over `over` only — the room's area, never the window's — as the
/// canvas draws one: the scrim across `over`, and centred in it a window
/// `width` wide (less where `over` is narrower) on the panel's fill in a
/// strong-edged hairline with the canvas's shadow, 24 points in: `title`
/// in 16-point semibold (wrapped where it runs long, never cut short of its
/// question) with a close button at its right (none on a gate), then the
/// body laid out top-down under it, scrolled where it is taller than
/// `over` allows.
///
/// The window takes the height its body asked for on the last pass (egui
/// runs the pass again when that changed, so it never shows misplaced).
/// Its body has the same room on every pass, to the 32nd of a point,
/// wherever that height moves the window, so a body that fits itself to
/// its room ([`folder_browser`]) asks for one height and keeps it. A
/// press anywhere in `over` outside the window is swallowed: what is under
/// the scrim is inert. A modal comes up with its body at its top, as the
/// cells draw a modal afresh each time, wherever it was scrolled to when it
/// was last up.
///
/// A scrolled body's view ends between its rows ([`body_row`]), never
/// through one: where its foot would cut a row, it ends over that row's
/// top, the room under it left blank, and the row comes in whole as the
/// body scrolls to it; where its top would (the body scrolled to its end),
/// it starts under that row's foot. It stands so wherever it stands — at
/// its top, where the keys or [`reveal`] took it, under the wheel — by the
/// rows the last pass laid out, the pass run again where they moved, so
/// the body never shows a row torn and the window never moves for it.
pub(crate) struct Dialog<'a> {
    id: Id,
    over: Rect,
    width: f32,
    title: &'a str,
    tone: Option<Color32>,
    close: bool,
    hold: Option<f32>,
    afresh: Option<&'a str>,
    head: f32,
    foot: f32,
}

/// What a [`Dialog`] measures over its room before it is drawn: its
/// window's width, its body's, its title set, what its padding, its title
/// and the room under the title take together, its window's most height,
/// and the height its body is given.
struct Measured {
    width: f32,
    inner_w: f32,
    title: Title,
    fixed: f32,
    max_h: f32,
    budget: f32,
}

impl<'a> Dialog<'a> {
    pub(crate) fn new(id: Id, over: Rect, width: f32, title: &'a str) -> Self {
        Dialog { id, over, width, title, tone: None, close: true, hold: None, afresh: None, head: 0.0, foot: 0.0 }
    }

    /// A head `h` points tall, the room under it with it, pinned under the
    /// title over the body's view rather than scrolled with the body: a
    /// sheet's tabs and what they say of it stand whole in view at every
    /// size, so the open tab can always be read, and the body scrolls under
    /// them where it runs long. [`Dialog::show_pinned`] draws it.
    pub(crate) fn head(self, h: f32) -> Self {
        Dialog { head: h.max(0.0), ..self }
    }

    /// A foot `h` points tall, the room over it with it, pinned under the
    /// body's view rather than scrolled with the body: a form's buttons
    /// stand whole in view at every size, as the cells keep a sheet's
    /// buttons and cut its rows over them, and the body scrolls above them
    /// where it runs long. [`Dialog::show_footed`] draws it.
    pub(crate) fn foot(self, h: f32) -> Self {
        Dialog { foot: h.max(0.0), ..self }
    }

    /// A warning gate, as the cells draw every one: its title and frame in
    /// the warning's gold, and no close, since a gate is answered — Esc and
    /// the safe button keep — never dismissed. A room draws its gates with
    /// [`gate`] or [`gate_with`], which wear this.
    pub(crate) fn gate(self, pal: &Pal) -> Self {
        Dialog { tone: Some(pal.gold), close: false, ..self }
    }

    /// The window's top held where a window with a body `body_h` tall would
    /// stand centred, its foot following its body: a list under a field
    /// that grows and shrinks moves nothing above it (the cells anchor
    /// their path entry so).
    pub(crate) fn hold(self, body_h: f32) -> Self {
        Dialog { hold: Some(body_h), ..self }
    }

    /// The body brought back to its top on any pass whose `key` is not the
    /// last pass's, as on the pass the modal comes up: another of its kind
    /// drawn in its place under the same id (a gate a chained answer
    /// opened, which the cells draw afresh) never opens where the last one
    /// was scrolled to.
    pub(crate) fn afresh(self, key: &'a str) -> Self {
        Dialog { afresh: Some(key), ..self }
    }

    /// The window and its body measured over `over`, the title set in
    /// `color`.
    fn measure(&self, ui: &Ui, color: Color32) -> Measured {
        let over = self.over;
        let width = self.width.min(over.width() - 32.0).max(over.width().min(160.0));
        let inner_w = (width - 2.0 * MODAL_PAD).max(1.0);
        // The title wrapped where it runs long, its first line standing
        // where a one-line title does.
        let title_w = (inner_w - if self.close { CLOSE_ROOM } else { 0.0 }).max(1.0);
        let title = modal_title(ui, self.title, title_w, color);
        let fixed = 2.0 * MODAL_PAD + title.h + MODAL_GAP;
        let max_h = (over.height() - 32.0).max(fixed);
        // The body's view is a whole number of 32nds of a point tall
        // (egui's grid), so it is as tall on every pass, wherever the
        // height the window last took has moved it. The floor has a
        // 1024th's grace: `over`'s height is a difference of two of its
        // edges, so a room a whole number of 32nds tall can come to at most
        // a few ten-thousandths of a point under it, and the floor would
        // take a whole 32nd from the body for that. A view up to a 1024th
        // taller than the room costs nothing: the window it asks for is
        // held to `max_h`. A pinned head's and foot's room is the body's no
        // more.
        let budget = ((max_h - fixed - self.head - self.foot).max(0.0) + 1.0 / 1024.0).floor_ui();
        Measured { width, inner_w, title, fixed, max_h, budget }
    }

    /// The room the window gives its body over its room, known before it
    /// is drawn: the body's width, and its height to the 32nd of a point
    /// (a pinned head's and foot's room aside), as [`Dialog::show`] hands
    /// them to it —
    /// what a body that chooses its own measures for its room (the MP3
    /// tab's Advanced and Settings sheets) chooses them by.
    pub(crate) fn body_room(&self, ui: &Ui) -> Vec2 {
        let measured = self.measure(ui, Color32::WHITE);
        vec2(measured.inner_w, measured.budget)
    }

    pub(crate) fn show<R>(self, ui: &Ui, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<R> {
        self.show_footed(ui, pal, body, |_, _| {})
    }

    /// [`Dialog::show`] with its pinned [`Dialog::foot`]: `foot` lays it
    /// out top-down in its room right under the body's view, the window
    /// wide, handed what the body returned to add to.
    pub(crate) fn show_footed<R>(self, ui: &Ui, pal: &Pal, body: impl FnOnce(&mut Ui) -> R, foot: impl FnOnce(&mut Ui, &mut R)) -> ModalShown<R> {
        self.show_pinned(ui, pal, |_| {}, body, foot)
    }

    /// [`Dialog::show_footed`] with its pinned [`Dialog::head`] too: `head`
    /// lays it out top-down in its room right under the title, the body's
    /// width, and the body's view stands under it.
    pub(crate) fn show_pinned<R>(
        self,
        ui: &Ui,
        pal: &Pal,
        head: impl FnOnce(&mut Ui),
        body: impl FnOnce(&mut Ui) -> R,
        foot: impl FnOnce(&mut Ui, &mut R),
    ) -> ModalShown<R> {
        let title_color = self.tone.unwrap_or(pal.text);
        let Measured { width, title, fixed, max_h, budget, .. } = self.measure(ui, title_color);
        let Dialog { id, over, tone, close, hold, afresh, head: head_h, foot: foot_h, .. } = self;
        let ctx = ui.ctx().clone();
        let size_key = id.with("height");
        let known: Option<f32> = ctx.data(|d| d.get_temp(size_key));
        let h = known.unwrap_or(max_h).min(max_h);
        let window = match hold {
            Some(body_h) => {
                let top = over.center().y - (fixed + body_h).min(max_h) / 2.0;
                Rect::from_min_size(pos2(over.center().x - width / 2.0, top), vec2(width, h))
            }
            None => Rect::from_center_size(over.center(), vec2(width, h)),
        };
        // Up on this pass when it was not drawn on the last, or another of
        // its kind took its place there.
        let now = ctx.cumulative_pass_nr();
        let drawn_key = id.with("drawn");
        let mut opened = ctx.data(|d| d.get_temp::<u64>(drawn_key)).is_none_or(|last| last + 1 != now);
        ctx.data_mut(|d| d.insert_temp(drawn_key, now));
        if let Some(key) = afresh {
            let key_at = id.with("afresh");
            let was: Option<String> = ctx.data(|d| d.get_temp(key_at));
            opened |= was.as_deref() != Some(key);
            ctx.data_mut(|d| d.insert_temp(key_at, key.to_string()));
        }
        // The body's rows as the last pass laid them out, in its content's
        // points, and a control the keys moved to, which comes in with the
        // view's top on a row's.
        let rows: Vec<Rangef> = ctx.data(|d| d.get_temp(id.with("rows"))).unwrap_or_default();
        let reveal_to: Option<(f32, f32, bool)> = ctx.data_mut(|d| d.remove_temp(id.with("reveal-to")));
        let reveal_to = reveal_to.map(|(top, bottom, down)| {
            // The control's own rows whole: from the top of the row its top
            // falls in to the foot of the row its foot does.
            let top = rows.iter().filter(|r| inside(r, top)).map(|r| r.min).fold(top, f32::min);
            let bottom = rows.iter().filter(|r| inside(r, bottom)).map(|r| r.max).fold(bottom, f32::max);
            if down { row_top(&rows, bottom - budget, top) } else { top }
        });

        // Up at once, as the canvas's and the cells' modals are: egui fades an
        // area in by default, over frames the panel would have to run for it.
        let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false);
        let shown = area.show(&ctx, |ui| {
            modal_window(ui, over, window, tone.unwrap_or(pal.edge_strong), pal);
            let inner = window.shrink(MODAL_PAD);
            let title_row = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
            // The canvas pulls the close button 6 points past the padding.
            let closed = close && {
                let at = pos2(inner.max.x - 8.0, title_row.center().y);
                icon_button(ui, id.with("close"), at, Icon::Close, pal.dim, pal).clicked()
            };
            let title_h = title.h;
            title.paint(ui, inner.min);
            // The pinned head right under the title, the body's width.
            let head_top = (inner.min.y + title_h + MODAL_GAP).round_to_pixels(ui.pixels_per_point()).round_ui();
            if head_h > 0.0 {
                let room = Rect::from_min_size(pos2(inner.min.x, head_top), vec2(inner.width(), head_h));
                let mut child = ui.new_child(UiBuilder::new().id_salt(id.with("head")).max_rect(room).layout(Layout::top_down(Align::Min)));
                head(&mut child);
            }
            // The body's view starts where its content does, on the pixel
            // egui puts that on, and is `budget` tall on every pass.
            let top = (head_top + head_h).round_to_pixels(ui.pixels_per_point()).round_ui();
            // The scrolled area reaches into the window's padding, where its
            // thumb stands 8 points in from the window's edge, clear of the
            // body, which keeps the inner width.
            let body_rect = Rect::from_min_size(pos2(inner.min.x, top), vec2(inner.width() + BODY_BAR, budget));
            let mut child =
                ui.new_child(UiBuilder::new().id_salt(id.with("body")).max_rect(body_rect).layout(Layout::top_down(Align::Min)));
            let mut scroll = ScrollArea::vertical().id_salt(id.with("scroll")).max_height(budget).auto_shrink([false, true]);
            if opened {
                scroll = scroll.vertical_scroll_offset(0.0);
            } else if let Some(to) = reveal_to {
                scroll = scroll.vertical_scroll_offset(to.max(0.0));
            }
            let out = scroll.show(&mut child, |ui| {
                ui.set_max_width(inner.width());
                // The view ends between the rows the last pass laid out,
                // over the top of a row its foot would cut, and starts
                // between them, under the foot of one its top would.
                let origin = ui.max_rect().min.y;
                let full = ui.clip_rect();
                let offset = full.min.y - origin;
                let end = row_end(&rows, offset, full.height());
                let start = row_start(&rows, offset, end);
                let view = Rect::from_min_max(pos2(full.min.x, (origin + start).max(full.min.y)), pos2(full.max.x, (origin + end).min(full.max.y)));
                ui.set_clip_rect(view);
                ui.ctx().data_mut(|d| {
                    d.insert_temp(id.with("view"), BodyView { origin, view, full: full.height(), opened });
                    d.insert_temp(laying_key(), Laying { id, origin, clip: view });
                    d.remove_temp::<Vec<Rangef>>(id.with("rows-now"));
                });
                // The body laid out in a room exactly as tall as its view.
                // egui rounds the scrolled rect it hands the body to pixels
                // at its top and its bottom apart, so that rect's height
                // moves by up to a pixel with where the window stands and
                // how far the body is scrolled.
                let room = Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), budget));
                let said = ui.scope_builder(UiBuilder::new().max_rect(room), body).inner;
                ui.ctx().data_mut(|d| d.remove::<Laying>(laying_key()));
                (said, offset, full.height(), (start, end))
            });
            // The pinned foot right under the body's view, the body's width.
            let shown_h = out.content_size.y.min(budget);
            let (mut said, offset, full_h, ends) = out.inner;
            if foot_h > 0.0 {
                let room = Rect::from_min_size(pos2(inner.min.x, top + shown_h), vec2(inner.width(), foot_h));
                let mut child = ui.new_child(UiBuilder::new().id_salt(id.with("pinned")).max_rect(room).layout(Layout::top_down(Align::Min)));
                foot(&mut child, &mut said);
            }
            // How low a list that hangs over the body may reach: over a
            // pinned foot, else inside the window's padding.
            let floor = if foot_h > 0.0 { top + shown_h } else { inner.max.y };
            ui.ctx().data_mut(|d| d.insert_temp(id.with("hang-floor"), floor));
            let wanted = fixed + head_h + shown_h + foot_h;
            (closed, said, wanted.min(max_h), (offset, full_h, ends))
        });
        let (closed, inner, wanted, (offset, full_h, (start, end))) = shown.inner;
        // The rows this pass laid out: where the view would start or end
        // elsewhere over them than it did over the last pass's, the pass
        // runs again.
        let laid: Vec<Rangef> = ctx.data_mut(|d| d.remove_temp(id.with("rows-now"))).unwrap_or_default();
        let laid_end = row_end(&laid, offset, full_h);
        let moved = (laid_end - end).abs() > 0.01 || (row_start(&laid, offset, laid_end) - start).abs() > 0.01;
        ctx.data_mut(|d| d.insert_temp(id.with("rows"), laid));
        if moved {
            ctx.request_discard("a modal's view ends between its body's rows");
            if !ctx.will_discard() {
                ctx.request_repaint();
            }
        }
        if known.is_none_or(|h| (h - wanted).abs() > 0.5) {
            ctx.data_mut(|d| d.insert_temp(size_key, wanted));
            ctx.request_discard("a modal took its body's height");
        }
        ModalShown { closed, inner, window }
    }
}

/// Where a body's view scrolled down to show a control from `top` to its
/// foot starts, its view's top at least at `low` (where the control's foot
/// meets the view's): the highest top of the body's `rows` from there that
/// still shows the control whole — so no row is torn over it — else, where
/// the body says of no row there (rows it sets at places of its own, the
/// Users room's grant), `low` itself, the control's foot on the view's, so
/// the view moves just as far as it must and never a page at a step; and
/// the control's own top where it is taller than the view.
fn row_top(rows: &[Rangef], low: f32, top: f32) -> f32 {
    let t = rows.iter().map(|r| r.min).filter(|t| *t >= low - 0.01 && *t <= top + 0.01).fold(f32::INFINITY, f32::min);
    if t.is_finite() { t } else { low.min(top) }
}

/// How low a list that hangs over the modal `modal`'s body may reach, as
/// the modal stood when it was last shown (this pass, for a list drawn
/// after it): over its pinned foot, else inside its window's padding
/// ([`hung_over`] holds its list to it). None before it was ever shown.
pub(crate) fn hung_floor(ui: &Ui, modal: Id) -> Option<f32> {
    ui.ctx().data(|d| d.get_temp(modal.with("hang-floor")))
}

/// How wide a [`Dialog`] wants to be to hold a body `body_w` points wide
/// and `title` on one line beside its close: the narrowest window a list of
/// names and its title stand whole in (the wizard's language list).
pub(crate) fn dialog_width(ui: &Ui, title: &str, body_w: f32) -> f32 {
    body_w.max(width_of(ui, title, strong(16.0)) + CLOSE_ROOM) + 2.0 * MODAL_PAD
}

/// A warning gate's window, every room's: room for the longest of their
/// sentences to break into whole lines. Then the room between its
/// sentences, and over its answers.
pub(crate) const GATE_W: f32 = 600.0;
pub(crate) const GATE_APART: f32 = 6.0;
pub(crate) const GATE_OVER_FOOT: f32 = 20.0;

/// A warning gate over `over`, as the cells draw every one and every room
/// draws it alike: a [`Dialog`] in the warning's gold with no close,
/// [`GATE_W`] wide, asking `title`; its `sentences` (each a [`Para`]: its
/// colour, a list's lead) 6 points apart; and 20 points under them its
/// foot, `answers` the safe choice lit and then the one that goes through,
/// buttons 0 and 1 of `id.with("foot")`, side by side at its right, or,
/// where the two will not fit, the one that goes through on a row of its
/// own above the safe one, as the cells move their confirmation up
/// ([`foot`]). Its answers say no row of the body's, as the MP3 tab's
/// gates' ([`answers`]): a gate is there to be answered, so the view never
/// ends over them and hides them. Esc keeps, as the room's keys have it.
/// How a click answered it, its `inner`: `Some(true)` goes through,
/// `Some(false)` keeps.
pub(crate) fn gate(ui: &Ui, id: Id, over: Rect, title: &str, sentences: Vec<Para>, answers: [&str; 2], pal: &Pal) -> ModalShown<Option<bool>> {
    let shown = Dialog::new(id, over, GATE_W, title).gate(pal).show(ui, pal, |ui| {
        for (k, sentence) in sentences.into_iter().enumerate() {
            if k > 0 {
                ui.add_space(GATE_APART);
            }
            sentence.show(ui);
        }
        ui.add_space(GATE_OVER_FOOT);
        let [keep, go] = answers;
        let buttons = [(keep, true), (go, false)];
        let w = ui.available_width();
        let (_, h) = foot_rects(ui, &[], &buttons, w);
        let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        foot_at(ui, id.with("foot"), row, &[], &buttons, None, pal)
    });
    ModalShown { closed: shown.closed, inner: shown.inner.map(|k| k == 1), window: shown.window }
}

/// The keyboard's control in the body of the modal `modal` at `rect`,
/// brought just into the body's view, at once, on the pass the keyboard
/// moved to it (`at`, the page's own number for it: a field's, a list's
/// cursor) or the modal came up or changed height — never after, so the
/// wheel keeps the body where it puts it. The move lands within the frame:
/// the pass runs again with the body where it goes, or where egui runs no
/// more passes this frame, the next frame is asked for. A control under
/// the view comes in with the view's top on a row's top, its foot ending
/// between rows ([`Dialog`]), so the move tears no row at either end; in a
/// body that says of no row there ([`body_row`]), with its foot on the
/// view's foot.
pub(crate) fn reveal(ui: &Ui, modal: Id, at: usize, rect: Rect) {
    let ctx = ui.ctx();
    let Some(body) = ctx.data(|d| d.get_temp::<BodyView>(modal.with("view"))) else { return };
    let h = body.full;
    let key = modal.with("revealed");
    let last: Option<(usize, f32)> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, (at, h)));
    if !body.opened && last.is_some_and(|(was, h_was)| was == at && (h_was - h).abs() < 0.5) {
        return;
    }
    let offset = body.view.min.y - body.origin;
    // In view is over where the view ends between rows, not its full foot.
    let end = body.view.max.y - body.origin;
    let (top, bottom) = (rect.min.y - body.origin, rect.max.y - body.origin);
    let down = if top < offset {
        false
    } else if bottom > end + 0.01 {
        true
    } else {
        return;
    };
    ctx.data_mut(|d| d.insert_temp(modal.with("reveal-to"), (top, bottom, down)));
    ctx.request_discard("a modal brought its keyboard's control into view");
    if !ctx.will_discard() {
        ctx.request_repaint();
    }
}

/// The keyboard's control in the modal `modal` that stands outside its
/// body's view, always in sight (a button of its pinned foot), by
/// [`reveal`]'s number for it, `at`: nothing is brought into view, but the
/// keyboard is noted there as [`reveal`] notes it, so the control in the
/// body the keys go to next is a move and comes into view, wherever the
/// wheel left the body meanwhile.
pub(crate) fn reveal_pinned(ui: &Ui, modal: Id, at: usize) {
    let ctx = ui.ctx();
    let Some(body) = ctx.data(|d| d.get_temp::<BodyView>(modal.with("view"))) else { return };
    ctx.data_mut(|d| d.insert_temp(modal.with("revealed"), (at, body.full)));
}

// ── Pictures, steps and a button at rest ─────────────────────────────────
//
// What the setup wizard's frame needed first (src/setup/egui_view.rs) and
// nothing of which is the wizard's own: a picture made into a texture once
// and drawn whole, a row of LEDs that says which step a page is at, and a
// form's button that cannot be pressed yet.

/// A picture as egui holds it, made once under `id` and kept in egui's
/// memory: `make` is asked for its pixels only when nothing is kept under
/// `id` yet or `key` (what the picture is of) has changed, so a page drawn
/// every frame uploads it once, and a new picture under the same id lets
/// the old texture go. A picture `make` cannot give is kept as none, and
/// not asked for again until `key` changes. `crisp` samples it by nearest
/// neighbour, so a code's modules stay square at any scale; else it is
/// smoothed.
pub(crate) fn texture(
    ctx: &egui::Context,
    id: Id,
    key: u64,
    crisp: bool,
    make: impl FnOnce() -> Option<egui::ColorImage>,
) -> Option<egui::TextureHandle> {
    let kept: Option<(u64, Option<egui::TextureHandle>)> = ctx.data(|d| d.get_temp(id));
    if let Some((was, handle)) = kept
        && was == key
    {
        return handle;
    }
    let options = if crisp { egui::TextureOptions::NEAREST } else { egui::TextureOptions::LINEAR };
    let handle = make().map(|image| ctx.load_texture(format!("picture {id:?}"), image, options));
    ctx.data_mut(|d| d.insert_temp(id, (key, handle.clone())));
    handle
}

/// `texture` drawn whole in `rect`: as large as fits with its shape kept,
/// centred in it, never cropped and never stretched, laid over what is
/// under it with its own alpha. Returns where it was drawn.
pub(crate) fn picture(ui: &Ui, rect: Rect, texture: &egui::TextureHandle) -> Rect {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 || !rect.is_positive() {
        return Rect::from_center_size(rect.center(), Vec2::ZERO);
    }
    let k = (rect.width() / size.x).min(rect.height() / size.y);
    let at = Rect::from_center_size(rect.center(), size * k);
    ui.painter().image(texture.id(), at, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    at
}

/// The room between a step line's LEDs.
const STEP_GAP: f32 = 4.0;

/// Which step a page is at, as a row of `total` [`led`]s [`STEP_GAP`]
/// points apart that ends at `right`, their middles on `mid`: the steps
/// before `at` (counted from 1) lit in the ok green, `at` in the accent,
/// the steps after it unlit. Returns where it was drawn.
pub(crate) fn step_leds(ui: &Ui, right: f32, mid: f32, at: usize, total: usize, pal: &Pal) -> Rect {
    let w = total as f32 * 6.0 + total.saturating_sub(1) as f32 * STEP_GAP;
    let left = right - w;
    for k in 0..total {
        let step = k + 1;
        let color = match step.cmp(&at) {
            std::cmp::Ordering::Less => pal.ok,
            std::cmp::Ordering::Equal => pal.accent,
            std::cmp::Ordering::Greater => pal.unlit,
        };
        led(ui, pos2(left + 3.0 + k as f32 * (6.0 + STEP_GAP), mid), color);
    }
    Rect::from_min_max(pos2(left, mid - 3.0), pos2(right, mid + 3.0))
}

/// A form's button that cannot be pressed now (the canvas's dead primary):
/// `label` [`bare`] in dim capitals on the panel's fill inside an edge
/// hairline, with no hover look, no hand and no click. Its response senses
/// the pointer only for the tooltip that says why, the one tooltip a
/// disabled control keeps.
pub(crate) fn form_button_off(ui: &Ui, id: Id, rect: Rect, label: &str, pal: &Pal) -> Response {
    let response = ui.interact(rect, id, Sense::hover());
    ui.painter().rect_filled(rect, square(), pal.panel);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
    caps(ui, rect.center(), bare(label), FORM_BUTTON_SIZE, pal.dim, rect.width(), Align2::CENTER_CENTER);
    response
}

// ── Cards, headings and pictures halved ──────────────────────────────────
//
// What the setup wizard's screens drew first (src/setup/egui_view/) and
// share, or that no page owns: a card's ground and a card that is one
// link, a button that is only its words, a page's heading and its body
// scrolled to the keyboard's control, a chip that opens a list, a sentence
// hung beside its mark, and a picture halved for drawing small.

/// A card's ground at `rect`, as every card that takes a click wears it:
/// the panel's fill in an edge hairline at rest, raised with its edge
/// brightened to the dim under the pointer, and the keyboard's look while
/// `focused` (the raised fill and the accent's ring), which wins over the
/// pointer's, as the kit's list rows have it.
fn card_ground(ui: &Ui, rect: Rect, hovered: bool, focused: bool, pal: &Pal) {
    if focused {
        cursor_look(ui, rect, pal);
        return;
    }
    ui.painter().rect_filled(rect, square(), if hovered { pal.raised } else { pal.panel });
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, if hovered { pal.dim } else { pal.edge }), StrokeKind::Inside);
}

/// A link's room inside its card: its words 18 points in.
const LINK_PAD: f32 = 18.0;

/// A card that is one link at `rect` (the wizard's Done: the stores and
/// the admin panel), on a [`card_ground`], the keyboard's look while
/// `focused`: `label` in the semibold text colour [`LINK_PAD`] in, cut with
/// an ellipsis, never changing colour. The whole card takes the click, with
/// the hand.
pub(crate) fn link_card(ui: &Ui, id: Id, rect: Rect, label: &str, focused: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    card_ground(ui, rect, response.hovered(), focused, pal);
    let room = (rect.width() - 2.0 * LINK_PAD).max(1.0);
    text(ui, pos2(rect.min.x + LINK_PAD, rect.center().y), label, strong(14.0), pal.text, room, Align2::LEFT_CENTER);
    response
}

/// A button that is only its words (the canvas's QUIET BUTTON, the cells'
/// one-row text button), their left middle at `at`: `label` in the
/// 13-point dim with no frame and no fill, cut with an ellipsis at `max_w`,
/// on a target as wide as the words (no wider than `max_w`) and [`HIT`]
/// tall; under the pointer the words in the text colour and the hand.
pub(crate) fn quiet_button(ui: &Ui, id: Id, at: Pos2, label: &str, max_w: f32, pal: &Pal) -> Response {
    let w = width_of(ui, label, body(13.0)).min(max_w).max(8.0);
    let rect = Rect::from_min_size(pos2(at.x, at.y - HIT / 2.0), vec2(w, HIT));
    let response = hand(ui.interact(rect, id, Sense::click()));
    let ink = if response.hovered() { pal.text } else { pal.dim };
    text(ui, at, label, body(13.0), ink, max_w.max(1.0), Align2::LEFT_CENTER);
    response
}

/// A page's heading, as the wizard's boards set it: the title's 28-point
/// row, 6 points, the subtitle's 18-point row, and the 24 before the first
/// control: [`HEADING_H`] in all.
const HEADING_TITLE: f32 = 28.0;
const HEADING_SUB_GAP: f32 = 6.0;
const HEADING_SUB: f32 = 18.0;
const HEADING_GAP: f32 = 24.0;
pub(crate) const HEADING_H: f32 = HEADING_TITLE + HEADING_SUB_GAP + HEADING_SUB + HEADING_GAP;

/// A page's heading, rows of the layout the column wide: `title` in the
/// 22-point semibold, `subtitle` dim under it, each cut with an ellipsis
/// where it runs long, then the room before the first control
/// ([`HEADING_H`] in all).
pub(crate) fn heading(ui: &mut Ui, title: &str, subtitle: &str, pal: &Pal) {
    line(ui, HEADING_TITLE, title, strong(22.0), pal.text);
    ui.add_space(HEADING_SUB_GAP);
    line(ui, HEADING_SUB, subtitle, body(14.0), pal.dim);
    ui.add_space(HEADING_GAP);
}

/// A page's body `body_h` points tall, laid out by `add` in `ui`, its
/// column: whole where the column has the room, else scrolled by the wheel
/// under egui's thumb, its rows [`BAR_ROOM`] short of the column's right
/// edge, where the thumb stands. The keyboard's control, `keep` (the page's
/// number for it and where it runs down the body), is brought just into
/// view on the pass the keyboard moved to it, the body came up or the
/// column changed height, and never after, so the wheel keeps the body
/// where it puts it ([`reveal`] does the same in a modal's body).
pub(crate) fn scrolled<R>(ui: &mut Ui, id: Id, body_h: f32, keep: Option<(usize, Rangef)>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let view_h = ui.available_height().max(0.0);
    let overflows = body_h > view_h + 0.5;
    let memory = id.with("view");
    let now = ui.ctx().cumulative_pass_nr();
    let (last_pass, last_keep, last_h, last_offset): (u64, Option<usize>, f32, f32) =
        ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((0, None, 0.0, 0.0));
    // Up on this pass when it was not drawn on the last: from its top.
    let fresh = last_pass + 1 != now;
    let bring = keep.filter(|(at, _)| fresh || last_keep != Some(*at) || (last_h - view_h).abs() > 0.5);
    let (offset, moved) = kept_offset(fresh, last_offset, bring.map(|(_, run)| run), view_h);
    let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if moved {
        scroll = scroll.vertical_scroll_offset(offset.max(0.0));
    }
    let output = scroll.show(ui, |ui| {
        // The rows stand where `keep` says, whatever egui's spacing: a
        // context's first pass lays out in its default (the kit styles it
        // during that pass), which sets rows 3 points apart.
        ui.spacing_mut().item_spacing.y = 0.0;
        if overflows {
            ui.set_max_width((ui.available_width() - BAR_ROOM).max(1.0));
        }
        add(ui)
    });
    let state = (now, keep.map(|(at, _)| at), view_h, output.state.offset.y);
    ui.ctx().data_mut(|d| d.insert_temp(memory, state));
    output.inner
}

/// Where a [`scrolled`] body's view stands, `view_h` tall, and whether it
/// moved: from its top when `fresh`, else where it was (`last`); and with
/// `bring` (where the control to bring in runs down the body) just far
/// enough to show it, its top where it is taller than the view.
fn kept_offset(fresh: bool, last: f32, bring: Option<Rangef>, view_h: f32) -> (f32, bool) {
    let (mut offset, mut moved) = if fresh { (0.0, true) } else { (last, false) };
    if let Some(run) = bring {
        if run.min < offset {
            (offset, moved) = (run.min, true);
        } else if run.max > offset + view_h {
            (offset, moved) = ((run.max - view_h).min(run.min), true);
        }
    }
    (offset, moved)
}

/// A chip that opens a list (the wizard's language), its left middle at
/// `at`: the chevron and `words` in a box [`HIT`] tall on the panel's fill
/// inside an edge hairline, raised under the pointer with its words in the
/// text colour and the hand; as wide as its words ([`select_chip_width`]).
pub(crate) fn select_chip(ui: &Ui, id: Id, at: Pos2, words: &str, pal: &Pal) -> Response {
    let w = select_chip_width(ui, words);
    let rect = Rect::from_min_size(pos2(at.x, at.y - HIT / 2.0), vec2(w, HIT));
    let response = hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    ui.painter().rect_filled(rect, square(), if hovered { pal.raised } else { pal.panel });
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
    let ink = if hovered { pal.text } else { pal.soft };
    chevron(ui, pos2(rect.min.x + 16.0, at.y), ink, false);
    text(ui, pos2(rect.min.x + 30.0, at.y), words, body(13.0), ink, w, Align2::LEFT_CENTER);
    response
}

/// How wide [`select_chip`] draws `words`: 10 points, the chevron's 12, 8,
/// the words, and 10.
pub(crate) fn select_chip_width(ui: &Ui, words: &str) -> f32 {
    10.0 + 12.0 + 8.0 + width_of(ui, words, body(13.0)) + 10.0
}

/// The marks a sentence of a list is led by in the cells: an upside's "+"
/// and a downside's "−".
const LIST_MARKS: [&str; 2] = ["+", "−"];

impl Para {
    /// A sentence of a gate's or a form's in `font` and `color`: one the
    /// cells lead with a "+" or a "−" and a space hangs its lines beside
    /// the mark, as a list item does ([`Para::lead`]), the mark in the
    /// line's colour; any other is set as it is.
    pub(crate) fn listed(s: &str, font: FontId, color: Color32) -> Para {
        match s.split_once(' ') {
            Some((mark, rest)) if LIST_MARKS.contains(&mark) => Para::new(rest.trim_start(), font, color).lead(mark),
            _ => Para::new(s, font, color),
        }
    }
}

/// How many times a picture `size` pixels is halved ([`halved`]) to be
/// drawn whole in `band` at `ppp` pixels a point: as often as leaves it at
/// most twice the pixels it is drawn at, and never once where it is drawn
/// at its own size or larger. The window's textures have no mipmaps, and
/// one sampled at under half its size skips texels (covers.rs fits a cover
/// down for the same reason): a wordmark's thin strokes would go.
pub(crate) fn halvings(size: Vec2, band: Rect, ppp: f32) -> u32 {
    let drawn = size.x * (band.width() / size.x).min(band.height() / size.y) * ppp;
    (size.x / drawn.max(1.0)).log2().floor().clamp(0.0, 8.0) as u32
}

/// `image` at half its size, each texel the mean of the (premultiplied)
/// two by two under it, so the clear pixels round a stroke lend its edges
/// none of their colour (a mipmap's level, made by hand): an odd last row
/// or column is left out, as a mipmap's level leaves it, and a side one
/// texel long is kept.
pub(crate) fn halved(image: &egui::ColorImage) -> egui::ColorImage {
    let [w, h] = image.size;
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let at = |x: usize, y: usize| image.pixels[y.min(h - 1) * w + x.min(w - 1)].to_array();
    let mut pixels = Vec::with_capacity(hw * hh);
    for y in 0..hh {
        for x in 0..hw {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                for (s, v) in sum.iter_mut().zip(at(2 * x + dx, 2 * y + dy)) {
                    *s += u32::from(v);
                }
            }
            let [r, g, b, a] = sum.map(|s| ((s + 2) / 4) as u8);
            pixels.push(Color32::from_rgba_premultiplied(r, g, b, a));
        }
    }
    egui::ColorImage::new([hw, hh], pixels)
}

// ── Figures, text buttons, the tall primary and a share's meter ──────────
//
// What the MP3 Player tab's egui frame needed first (src/device/page/
// egui_view.rs) and nothing of which is the tab's own: a sentence's
// figures found and set in the monospace, a button that is only its words
// with the mark a door wears, the page's one tall act, and an LED row lit
// for a share the way the cells fill their bar.

/// Where the figure in `word` runs, if it holds one: from its first letter
/// or digit (or a `/`), the letters and digits and the marks that join a
/// figure's parts (`.,:-+_/`), less the marks it ends on (a sentence's
/// full stop) — when that run starts with a digit (`21.4`, `1,284`,
/// `14:03`, a serial, and a percentage with its sign, `62%`), is a version
/// (`v0.14.0`, `v0.14.0-5-g4e94418`), is a hex string of six or more with
/// a digit in it (an ELF), or is one of `also` (a port's name or a file's,
/// which no rule can tell from a word), matched whole. Any other run that
/// starts with a digit ends before a point a letter follows: a file named
/// for its number keeps its extension in the words' face (`12.flac`'s
/// figure is its `12`), unless `also` names it.
fn figure_span(word: &str, also: &[&str]) -> Option<Range<usize>> {
    let start = word.find(|c: char| c.is_ascii_alphanumeric() || c == '/')?;
    let joins = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | ',' | ':' | '-' | '+' | '_' | '/');
    let len = word[start..].find(|c: char| !joins(c)).unwrap_or(word.len() - start);
    let whole = word[start..start + len].trim_end_matches(['.', ',', ':', '-']);
    if !whole.is_empty() && also.contains(&whole) {
        return Some(start..start + whole.len());
    }
    let mut core = whole;
    if core.starts_with(|c: char| c.is_ascii_digit())
        && let Some(at) = core.as_bytes().windows(2).position(|pair| pair[0] == b'.' && pair[1].is_ascii_alphabetic())
    {
        core = &core[..at];
    }
    let mut chars = core.chars();
    let first = chars.next()?;
    let digit = first.is_ascii_digit();
    let version = first == 'v' && chars.next().is_some_and(|c| c.is_ascii_digit());
    let hex = core.len() >= 6 && core.chars().all(|c| c.is_ascii_hexdigit()) && core.chars().any(|c| c.is_ascii_digit());
    let end = start + core.len();
    let end = if digit && word[end..].starts_with('%') { end + 1 } else { end };
    (digit || version || hex).then(|| start..end)
}

/// `s` as runs in `font` and `color`, each figure in it ([`figure_span`]:
/// a size, a version, a count, a time, a serial, a port or a file named in
/// `also`) set in `figure` instead, as the canvas sets a page's figures in
/// the grid's monospace inside its sentences. The words between keep their
/// spaces, so the line says what the cells say.
pub(crate) fn figured(s: &str, also: &[&str], font: FontId, figure: FontId, color: Color32) -> Vec<Run> {
    let mut parts: Vec<Run> = Vec::new();
    let mut plain = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        let lead = rest.len() - rest.trim_start().len();
        plain.push_str(&rest[..lead]);
        rest = &rest[lead..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        match figure_span(word, also) {
            Some(span) => {
                plain.push_str(&word[..span.start]);
                if !plain.is_empty() {
                    parts.push((std::mem::take(&mut plain), font.clone(), color));
                }
                parts.push((word[span.clone()].to_string(), figure.clone(), color));
                plain.push_str(&word[span.end..]);
            }
            None => plain.push_str(word),
        }
        rest = &rest[end..];
    }
    if !plain.is_empty() {
        parts.push((plain, font, color));
    }
    parts
}

/// What leads a [`text_button`]'s words, 8 points before them: the kit's
/// [`pointer`] (the cells' ▸, a door shut) or its [`arrow`] down (their ▾,
/// the door open).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lead {
    Shut,
    Open,
}

/// A [`text_button`]'s height, its words' size, and the room its lead
/// takes: the mark's 8 points and the 8 after it.
pub(crate) const TEXT_BUTTON_H: f32 = HIT;
pub(crate) const TEXT_BUTTON_SIZE: f32 = 13.5;
const LEAD_W: f32 = 16.0;

/// How wide [`text_button`] draws `words` led by `lead`, in the semibold it
/// wears under the pointer, the widest it gets.
pub(crate) fn text_button_width(ui: &Ui, words: &str, lead: Option<Lead>, also: &[&str]) -> f32 {
    text_button_width_sized(ui, words, lead, also, TEXT_BUTTON_SIZE)
}

/// How wide [`text_button_sized`] draws `words` at `size`.
pub(crate) fn text_button_width_sized(ui: &Ui, words: &str, lead: Option<Lead>, also: &[&str], size: f32) -> f32 {
    let lead_w = if lead.is_some() { LEAD_W } else { 0.0 };
    lead_w + runs_width(ui, &figured(words, also, strong(size), mono(12.5), Color32::WHITE))
}

/// A button that is only its words, the canvas's TEXT BUTTON (a door, a
/// link that acts), its left middle at `at`: `words` dim with no frame and
/// no fill, their figures in the monospace ([`figured`], `also` among
/// them), led by `lead`; in the text colour and the semibold under the
/// pointer, with the hand. Its target is as wide as it draws, no wider
/// than `max_w`, where its words are cut with an ellipsis, and
/// [`TEXT_BUTTON_H`] tall. Not `live`, it is drawn at rest and senses the
/// pointer for nothing but a hover no look answers: something lies over
/// it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_button(
    ui: &Ui,
    id: Id,
    at: Pos2,
    words: &str,
    lead: Option<Lead>,
    also: &[&str],
    max_w: f32,
    live: bool,
    pal: &Pal,
) -> Response {
    text_button_sized(ui, id, at, words, lead, also, max_w, live, pal, TEXT_BUTTON_SIZE)
}

/// A [`text_button`] whose words are `size` points rather than the card's
/// 13.5 (its figures stay the card's 12.5): one that shares a slot with
/// plain words of that size, as Update all shares the MP3 tab's row's right
/// edge with its counts at 13.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_button_sized(
    ui: &Ui,
    id: Id,
    at: Pos2,
    words: &str,
    lead: Option<Lead>,
    also: &[&str],
    max_w: f32,
    live: bool,
    pal: &Pal,
    size: f32,
) -> Response {
    text_button_in(ui, id, at, words, lead, also, max_w, live, false, pal, size)
}

/// A [`text_button`] that wears the keyboard's look while `focused` (the
/// chooser the keys are on, the MP3 tab's Advanced sheet's, as the cells
/// light it): its words in the accent and the semibold, the pointer's look
/// winning over it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_button_keyed(ui: &Ui, id: Id, at: Pos2, words: &str, also: &[&str], max_w: f32, focused: bool, pal: &Pal) -> Response {
    text_button_in(ui, id, at, words, None, also, max_w, true, focused, pal, TEXT_BUTTON_SIZE)
}

#[allow(clippy::too_many_arguments)]
fn text_button_in(
    ui: &Ui,
    id: Id,
    at: Pos2,
    words: &str,
    lead: Option<Lead>,
    also: &[&str],
    max_w: f32,
    live: bool,
    focused: bool,
    pal: &Pal,
    size: f32,
) -> Response {
    let w = text_button_width_sized(ui, words, lead, also, size).min(max_w).max(8.0);
    let rect = Rect::from_min_size(pos2(at.x, at.y - TEXT_BUTTON_H / 2.0), vec2(w, TEXT_BUTTON_H));
    let response = if live { hand(ui.interact(rect, id, Sense::click())) } else { ui.interact(rect, id, Sense::hover()) };
    let hot = live && response.hovered();
    let ink = match (hot, focused) {
        (true, _) => pal.text,
        (false, true) => pal.accent,
        (false, false) => pal.dim,
    };
    let font = if hot || focused { strong(size) } else { body(size) };
    let mut x = at.x;
    match lead {
        // The pointer's 6 points and the arrow's 8 each stand at the lead's
        // left, its middle on the words' small letters.
        Some(Lead::Shut) => pointer(ui, pos2(x + 2.5, led_mid(ui, at.y, &font, false)), ink),
        Some(Lead::Open) => arrow(ui, pos2(x + 4.0, led_mid(ui, at.y, &font, false)), ink, true),
        None => {}
    }
    if lead.is_some() {
        x += LEAD_W;
    }
    let room = (at.x + max_w - x).max(1.0);
    runs(ui, pos2(x, at.y), &figured(words, also, font, mono(12.5), ink), room, Align2::LEFT_CENTER);
    response
}

/// The tall primary's height, its capitals' size, the room either side of
/// its words, and the pointer's room after them: its 6 points and the 10
/// before it.
pub(crate) const TALL_H: f32 = 44.0;
const TALL_SIZE: f32 = 12.5;
const TALL_PAD: f32 = 22.0;
const TALL_MARK: f32 = 16.0;

/// How wide [`tall_button`] wants to be for `label`, with its pointer
/// while `enabled`.
pub(crate) fn tall_button_width(ui: &Ui, label: &str, enabled: bool) -> f32 {
    caps_width(ui, bare(label), TALL_SIZE) + 2.0 * TALL_PAD + if enabled { TALL_MARK } else { 0.0 }
}

/// A page's one act at `rect`, the canvas's TALL PRIMARY: a 1-point accent
/// frame with no fill, `label` [`bare`] in accent capitals and the kit's
/// [`pointer`] after them, the frame, the words and the pointer heating to
/// the hot accent under the pointer, with the hand. Not `enabled` (its act
/// waits on another): the frame and the words dim, no pointer, no hand and
/// no click — its response senses the pointer only for the tooltip that
/// says why. Not `live`, it is drawn at rest and takes nothing: something
/// lies over it.
pub(crate) fn tall_button(ui: &Ui, id: Id, rect: Rect, label: &str, enabled: bool, live: bool, pal: &Pal) -> Response {
    let acts = enabled && live;
    let response = ui.interact(rect, id, if acts { Sense::click() } else { Sense::hover() });
    let response = if acts { hand(response) } else { response };
    let ink = match (enabled, acts && response.hovered()) {
        (false, _) => pal.dim,
        (true, true) => pal.accent_hot,
        (true, false) => pal.accent,
    };
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, ink), StrokeKind::Inside);
    let label = bare(label);
    let words = caps_width(ui, label, TALL_SIZE);
    let mark = if enabled { TALL_MARK } else { 0.0 };
    let left = (rect.center().x - (words + mark) / 2.0).max(rect.min.x + 1.0);
    let room = (rect.max.x - left - mark).max(1.0);
    let drawn = caps(ui, pos2(left, rect.center().y), label, TALL_SIZE, ink, room, Align2::LEFT_CENTER);
    if enabled {
        pointer(ui, pos2(drawn.max.x + 10.0 + 2.5, led_mid(ui, rect.center().y, &strong(TALL_SIZE), true)), ink);
    }
    response
}

/// How many of a meter's `count` segments `permille` lights, as the cells
/// fill their bar: the nearest whole segments, one at least for a sliver,
/// and never the last before the whole — a card 99.6% full shows one
/// segment unlit.
pub(crate) fn fill_lit(permille: u32, count: usize) -> usize {
    let count = count as u64;
    let mut lit = (count * u64::from(permille.min(1000)) + 500) / 1000;
    if permille > 0 {
        lit = lit.max(1);
    }
    if permille < 1000 {
        lit = lit.min(count.saturating_sub(1));
    }
    lit as usize
}

/// An LED row across `track` ([`led_row`]'s segments) lit for `permille` of
/// it as [`fill_lit`] counts, in `lit` and `unlit`: the cells' bar in the
/// desk's look.
pub(crate) fn led_fill(ui: &Ui, track: Rect, permille: u32, lit: Color32, unlit: Color32) {
    let count = led_count(track.width());
    led_row(ui, track, fill_lit(permille, count) as f32 / count as f32, lit, unlit);
}

// ── Gates' answers, lists that hang, and a card's frame ──────────────────
//
// What the MP3 Player tab's gates, sheets and cards share (src/device/page/
// egui_view/), and nothing of which is the tab's own: a modal's title and
// its window for a gate that lays its own body out, the keep led back with
// its figures in their own case and a gate's two answers with the go that
// waits on a box, the gate whose body is the page's own, the keyboard's
// ring round a foot's button, a closed list's box and the list that hangs
// from it inside a modal, a list's thumb the wheel moves, a card's
// hairline frame and its head, and prose spaced by its line's height.

/// A modal's title as every modal sets it, set once and drawn where its
/// window says ([`modal_title`]).
pub(crate) struct Title {
    galley: Arc<Galley>,
    line: f32,
    color: Color32,
    /// How tall its rows stand: the title's row, and a line more for each
    /// line it wraps to.
    pub h: f32,
}

/// `title` as a modal's ([`Dialog`]'s, and a gate's that lays its own
/// window out, the MP3 tab's board gate): the 16-point semibold in
/// `color`, wrapped at `w` where it runs long, never cut short of its
/// question but past three lines, its first line standing where a one-line
/// title does.
pub(crate) fn modal_title(ui: &Ui, title: &str, w: f32, color: Color32) -> Title {
    let format = TextFormat { font_id: strong(16.0), color, ..Default::default() };
    let mut job = LayoutJob::single_section(title.to_string(), format);
    job.wrap = TextWrapping { max_width: w.max(1.0), max_rows: 3, break_anywhere: false, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    let lines = galley.rows.len().max(1);
    let line = galley.size().y / lines as f32;
    Title { galley, line, color, h: TITLE_H + (lines - 1) as f32 * line }
}

impl Title {
    /// Drawn from `at`, its row's top left, its first line centred on the
    /// title's row.
    pub(crate) fn paint(self, ui: &Ui, at: Pos2) {
        ui.painter().galley(pos2(at.x, at.y + TITLE_H / 2.0 - self.line / 2.0), self.galley, self.color);
    }
}

/// A modal's window at `window` over `over`, as [`Dialog`] draws every
/// one: the scrim across `over`, taking every press there outside the
/// window (what is under it is inert), and the window on the panel's fill
/// in an `edge` hairline (the strong edge, a gate's gold) with the
/// canvas's shadow. For a gate that lays its own body out in it.
pub(crate) fn modal_window(ui: &mut Ui, over: Rect, window: Rect, edge: Color32, pal: &Pal) {
    let _scrim = ui.allocate_rect(over, Sense::click_and_drag());
    ui.painter().rect_filled(over, 0.0, SCRIM);
    ui.painter().add(lifted().as_shape(window, square()));
    ui.painter().rect_filled(window, square(), pal.panel);
    ui.painter().rect_stroke(window, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
}

/// The canvas's shadow under what floats over a page: a modal's window, a
/// list that hangs.
fn lifted() -> Shadow {
    Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(115) }
}

/// The kit's [`pointer`] turned back, centred on `at`: the cells' "◂"
/// before a keep, the way back to what stays.
pub(crate) fn back_pointer(ui: &Ui, at: Pos2, color: Color32) {
    let points = vec![at + vec2(-3.5, 0.0), at + vec2(2.5, -4.0), at + vec2(2.5, 4.0)];
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// The room either side of a form button's words, and a keep's lead: the
/// back pointer's 6 points and the 8 after it.
const FORM_PAD: f32 = 18.0;
const KEEP_LEAD: f32 = 14.0;

/// A keep's words as runs, each with whether it is a figure: the words in
/// the form buttons' spaced capitals, a figure among them (a version, a
/// port in `also`) in the monospace in its own case.
fn keep_runs(words: &str, also: &[&str]) -> Vec<(String, bool)> {
    let font = strong(FORM_BUTTON_SIZE);
    figured(words, also, font.clone(), mono(FORM_BUTTON_SIZE), Color32::WHITE).into_iter().map(|(s, f, _)| (s, f != font)).collect()
}

/// How wide a keep's words draw ([`keep_runs`]).
fn keep_words_width(ui: &Ui, words: &str, also: &[&str]) -> f32 {
    let run_w = |(s, figure): &(String, bool)| if *figure { width_of(ui, s, mono(FORM_BUTTON_SIZE)) } else { caps_width(ui, s, FORM_BUTTON_SIZE) };
    keep_runs(words, also).iter().map(run_w).sum()
}

/// How wide [`keep_button`] wants to be for `words`, led back or not.
pub(crate) fn keep_button_width(ui: &Ui, words: &str, lead: bool, also: &[&str]) -> f32 {
    2.0 * FORM_PAD + if lead { KEEP_LEAD } else { 0.0 } + keep_words_width(ui, words, also)
}

/// A gate's safe answer at `rect`, the modal's one lit act, as the MP3
/// tab's boards draw it (KEEP v0.13.0): a form button's accent fill, the
/// hot accent under the pointer, with the hand; on it in the ink on the
/// accent, where `lead` (the cells lead it with "◂"), the [`back_pointer`]
/// 8 points before its words, the words in the form buttons' spaced
/// capitals and their figures in the monospace in their own case, cut at
/// the button's padding. `words` come bare of the cells' arrow.
pub(crate) fn keep_button(ui: &Ui, id: Id, rect: Rect, words: &str, lead: bool, also: &[&str], pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    ui.painter().rect_filled(rect, square(), if response.hovered() { pal.accent_hot } else { pal.accent });
    let ink = pal.on_accent;
    let lead_w = if lead { KEEP_LEAD } else { 0.0 };
    let want = lead_w + keep_words_width(ui, words, also);
    let mid = rect.center().y;
    let end = rect.max.x - FORM_PAD;
    let mut x = (rect.center().x - want / 2.0).max(rect.min.x + FORM_PAD);
    if lead {
        // Its point at the lead's left, its middle on the capitals'.
        back_pointer(ui, pos2(x + 3.5, led_mid(ui, mid, &strong(FORM_BUTTON_SIZE), true)), ink);
        x += lead_w;
    }
    for (s, figure) in keep_runs(words, also) {
        let room = end - x;
        if room < 1.0 {
            break;
        }
        let drawn = if figure {
            text(ui, pos2(x, mid), &s, mono(FORM_BUTTON_SIZE), ink, room, Align2::LEFT_CENTER)
        } else {
            caps(ui, pos2(x, mid), &s, FORM_BUTTON_SIZE, ink, room, Align2::LEFT_CENTER)
        };
        x = drawn.max.x;
    }
    response
}

/// A gate's two answers as the cells give them: the safe one, `keep`, its
/// words bare of the cells' "◂" and led back where they had it (`lead`),
/// and the one that goes through, `go` — none while it waits (the stronger
/// gate's, on its box) — the figures among them (`also`: the ports) in the
/// monospace.
#[derive(Clone, Copy)]
pub(crate) struct Answers<'a> {
    pub keep: &'a str,
    pub lead: bool,
    pub go: Option<&'a str>,
    pub also: &'a [&'a str],
}

/// Where a foot `w` wide lays a gate's answers, from its top left, as
/// [`foot_layout`] lays a gate's two: side by side at its end, the go last
/// as the kit's gate has it; where the two will not fit, the go keeps the
/// first row and the keep comes under it, as the cells move their
/// confirmation up. Each answer's rect, the keep's first, and the foot's
/// height.
pub(crate) fn answers_rects(ui: &Ui, a: &Answers, w: f32) -> (Vec<Rect>, f32) {
    let mut wants = vec![keep_button_width(ui, a.keep, a.lead, a.also)];
    wants.extend(a.go.map(|go| form_button_width(ui, go)));
    foot_layout(&[], &wants, w)
}

/// A gate's answers in a foot from `at`, its top left, `w` wide, as
/// [`answers_rects`] lays them: the keep lit ([`keep_button`]) as
/// `id.with(0usize)` and the go as the hairline ([`form_button`]) as
/// `id.with(1usize)`. How a click answered: `Some(true)` goes through,
/// `Some(false)` keeps.
pub(crate) fn answers_at(ui: &Ui, id: Id, at: Pos2, a: &Answers, w: f32, pal: &Pal) -> Option<bool> {
    let (laid, _) = answers_rects(ui, a, w);
    let mut answer = None;
    if keep_button(ui, id.with(0usize), laid[0].translate(at.to_vec2()), a.keep, a.lead, a.also, pal).clicked() {
        answer = Some(false);
    }
    if let (Some(go), Some(rect)) = (a.go, laid.get(1))
        && form_button(ui, id.with(1usize), rect.translate(at.to_vec2()), go, false, pal).clicked()
    {
        answer = Some(true);
    }
    answer
}

/// [`answers_at`] as a row of the layout the column wide. Like the kit's
/// [`gate`]'s, its answers say no row of the body's: a gate is there to be
/// answered, so the view never ends over them and hides them.
pub(crate) fn answers(ui: &mut Ui, id: Id, a: &Answers, pal: &Pal) -> Option<bool> {
    let w = ui.available_width();
    let (_, h) = answers_rects(ui, a, w);
    let (row, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    answers_at(ui, id, row.min, a, w, pal)
}

/// A warning gate whose body is the page's own — the stronger gate's,
/// whose box among its sentences ([`check_line`]) the go waits on, as much
/// as any other — over `over`: a [`Dialog`] in the gate's look, [`GATE_W`]
/// wide, asking `title`, its body back at its top whenever another gate
/// takes its place ([`Dialog::afresh`], by its title); `body` laid out
/// top-down, then [`GATE_OVER_FOOT`] under it the gate's answers `a`
/// ([`answers`], under `id.with("foot")`), the body scrolled where it is
/// taller than `over` allows. Its `inner`: how a click answered
/// (`Some(true)` goes through, `Some(false)` keeps), and what `body`
/// returned.
pub(crate) fn gate_with<R>(ui: &Ui, id: Id, over: Rect, title: &str, a: &Answers, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<(Option<bool>, R)> {
    Dialog::new(id, over, GATE_W, title).gate(pal).afresh(title).show(ui, pal, |ui| {
        let inner = body(ui);
        ui.add_space(GATE_OVER_FOOT);
        (answers(ui, id.with("foot"), a, pal), inner)
    })
}

/// How far out of a foot's button the keyboard's ring stands.
const RING_OUT: f32 = 3.0;

/// The keyboard on a foot's button at `rect` (a sheet's Apply, its Use
/// defaults): the accent's ring 3 points out of it, as the cells put the
/// keyboard's button on their slab.
pub(crate) fn button_ring(ui: &Ui, rect: Rect, pal: &Pal) {
    ring(ui, rect.expand(RING_OUT), pal);
}

/// A closed list's box: its least width (the Settings board's, so it keeps
/// its width from its first words to a short pick), the room either side
/// of its words and before its chevron, and the chevron's.
pub(crate) const SELECT_LEAST: f32 = 120.0;
const SELECT_PAD: f32 = 10.0;
const SELECT_CHEVRON: f32 = 12.0;

/// How wide [`select_box`] draws `words` at `size`, whole in the semibold
/// the keyboard's look wears: [`SELECT_LEAST`] at least.
pub(crate) fn select_box_width(ui: &Ui, words: &str, also: &[&str], size: f32) -> f32 {
    let words_w = runs_width(ui, &figured(words, also, strong(size), mono(12.5), Color32::WHITE));
    (3.0 * SELECT_PAD + SELECT_CHEVRON + words_w).max(SELECT_LEAST)
}

/// A closed list's box at `rect`, as the canvas's Settings board draws its
/// dropdown and every closed list on the MP3 tab wears it (From's server,
/// Into's codec, the Advanced sheet's release): the ground's fill in a
/// strong-edged hairline, raised while `hovered`; `words` at `size` 10
/// points in, their figures in the monospace — in the accent's semibold
/// while `lit` (the keyboard's), the semibold under the pointer — and the
/// kit's chevron at its end, dim, in the text colour under the pointer.
/// It takes nothing itself: [`select_button`] is one that takes the click.
#[allow(clippy::too_many_arguments)]
pub(crate) fn select_box(ui: &Ui, rect: Rect, words: &str, also: &[&str], size: f32, lit: bool, hovered: bool, pal: &Pal) {
    ui.painter().rect_filled(rect, square(), if hovered { pal.raised } else { pal.ground });
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    let (font, ink) = match (lit, hovered) {
        (true, _) => (strong(size), pal.accent),
        (false, true) => (strong(size), pal.text),
        (false, false) => (body(size), pal.text),
    };
    let mid = rect.center().y;
    let room = (rect.width() - 3.0 * SELECT_PAD - SELECT_CHEVRON).max(1.0);
    runs(ui, pos2(rect.min.x + SELECT_PAD, mid), &figured(words, also, font, mono(12.5), ink), room, Align2::LEFT_CENTER);
    let chevron_ink = if hovered { pal.text } else { pal.dim };
    chevron(ui, pos2(rect.max.x - SELECT_PAD - SELECT_CHEVRON / 2.0, mid), chevron_ink, false);
}

/// A [`select_box`] at `rect` that takes the click itself, with the hand,
/// raised under the pointer: a closed list that is a control of its own in
/// its row.
#[allow(clippy::too_many_arguments)]
pub(crate) fn select_button(ui: &Ui, id: Id, rect: Rect, words: &str, also: &[&str], size: f32, lit: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    select_box(ui, rect, words, also, size, lit, response.hovered(), pal);
    response
}

/// An entry of a list that hangs, and the room at either side of its
/// words; then the frame's room round its entries, its hairline and a
/// point.
pub(crate) const HUNG_ROW_H: f32 = 28.0;
pub(crate) const HUNG_PAD: f32 = 10.0;
const HUNG_IN: f32 = 2.0;

/// How many whole entries a list hanging from `top` holds over `bottom`.
pub(crate) fn hung_fits(top: f32, bottom: f32) -> usize {
    ((bottom - top - 2.0 * HUNG_IN) / HUNG_ROW_H).floor().max(0.0) as usize
}

/// The frame of a list hanging from `at`, its top left, `w` wide with `n`
/// entries.
pub(crate) fn hung_frame(at: Pos2, w: f32, n: usize) -> Rect {
    Rect::from_min_size(at, vec2(w, n as f32 * HUNG_ROW_H + 2.0 * HUNG_IN))
}

/// The gutter a list that hangs keeps at its right for its thumb while it
/// holds fewer entries than it has: the thumb and 2 points either side.
pub(crate) const HUNG_GUTTER: f32 = THUMB_W + 4.0;

/// The gutter a list that hangs keeps for its thumb, holding `n` of its
/// `len` entries: [`HUNG_GUTTER`] where it holds fewer, none where it
/// holds them all.
fn hung_gutter(n: usize, len: usize) -> f32 {
    if n < len { HUNG_GUTTER } else { 0.0 }
}

/// How wide a list that hangs is for entries whose words are `words_w`
/// wide at most, holding `n` of its `len`: the words, their padding, the
/// thumb's gutter while it holds fewer than its all, and the frame's room.
pub(crate) fn hung_width(words_w: f32, n: usize, len: usize) -> f32 {
    words_w + 2.0 * HUNG_PAD + hung_gutter(n, len) + 2.0 * HUNG_IN
}

/// Where a list that hangs from a closed list's box whose left is
/// `box_left` stands its left, for its entries' words to stand under the
/// box's: the box's padding, less the frame's room and an entry's padding.
pub(crate) fn hung_left(box_left: f32) -> f32 {
    box_left + SELECT_PAD - HUNG_IN - HUNG_PAD
}

/// The first of `len` entries a list that hangs shows when it holds `n` of
/// them: from its top, as far down as keeps the keyboard's `cursor` in
/// sight.
pub(crate) fn hung_first(cursor: Option<usize>, n: usize, len: usize) -> usize {
    (cursor.unwrap_or(0) + 1).saturating_sub(n).min(len.saturating_sub(n))
}

/// The list that hangs open from a closed list's box inside a modal, as
/// the MP3 tab's two sheets hang theirs, in `frame` ([`hung_frame`]): the
/// panel's fill in the accent's hairline with the canvas's shadow, `n`
/// entries [`HUNG_ROW_H`] tall 2 points in, from entry `first` of the
/// list's `len`; the keyboard's `cursor` filled in the accent, another
/// raised under the pointer, with the hand, where `id_of` gives the entry
/// an id — none takes nothing, a line that picks nothing. Holding fewer
/// than its all, it shows the kit's thumb at rest at its right, where it
/// stands among them ([`list_thumb`]), in a gutter of its own the entries
/// and their cursor stop short of ([`HUNG_GUTTER`]).
/// `words(ui, k, room, on)` paints entry `k`'s words in `room`, the entry
/// less its padding, `on` while it is the cursor's, which wears the ink on
/// the accent. The list is the page's keys', as egui's own popup is not (it
/// closes on a click outside it and takes keys of its own); the page says
/// what a click off its entries does. The entry a click landed on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hung_list(
    ui: &Ui,
    frame: Rect,
    first: usize,
    n: usize,
    len: usize,
    cursor: Option<usize>,
    id_of: impl Fn(usize) -> Option<Id>,
    pal: &Pal,
    words: impl FnMut(&Ui, usize, Rect, bool),
) -> Option<usize> {
    hung_list_by(ui, frame, first, n, len, cursor, id_of, Pick::Click, pal, words)
}

/// [`hung_list`] picking its entry on the click or, as the cells' lists
/// pick theirs, on the press ([`Pick`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn hung_list_by(
    ui: &Ui,
    frame: Rect,
    first: usize,
    n: usize,
    len: usize,
    cursor: Option<usize>,
    id_of: impl Fn(usize) -> Option<Id>,
    pick: Pick,
    pal: &Pal,
    mut words: impl FnMut(&Ui, usize, Rect, bool),
) -> Option<usize> {
    ui.painter().add(lifted().as_shape(frame, square()));
    ui.painter().rect_filled(frame, square(), pal.panel);
    ui.painter().rect_stroke(frame, square(), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
    let gutter = hung_gutter(n, len);
    let mut picked = None;
    for k in first..first + n {
        let top = frame.min.y + HUNG_IN + (k - first) as f32 * HUNG_ROW_H;
        let row = Rect::from_min_size(pos2(frame.min.x + HUNG_IN, top), vec2((frame.width() - 2.0 * HUNG_IN - gutter).max(1.0), HUNG_ROW_H));
        let response = id_of(k).map(|id| hand(ui.interact(row, id, Sense::click())));
        let on = cursor == Some(k);
        if on {
            ui.painter().rect_filled(row, CornerRadius::same(2), pal.accent);
        } else if response.as_ref().is_some_and(Response::hovered) {
            ui.painter().rect_filled(row, CornerRadius::same(2), pal.raised);
        }
        words(ui, k, row.shrink2(vec2(HUNG_PAD, 0.0)), on);
        let took = |r: &Response| match pick {
            Pick::Click => r.clicked(),
            Pick::Press => pressed(r),
        };
        if response.is_some_and(|r| took(&r)) {
            picked = Some(k);
        }
    }
    if n < len {
        // The thumb in the middle of its gutter, as far in from the frame's
        // top and foot as from its sides.
        let pane = frame.shrink(HUNG_IN + (gutter - THUMB_W) / 2.0);
        list_thumb(ui, pane, n, len, first, pal);
    }
    picked
}

/// [`hung_list`] on a layer of its own over the modal `parent`'s, so it
/// hangs over the modal's lower rows past its body's view, over the window
/// whichever of the two was last pressed (egui raises an area pressed on to
/// the top of its order): what the list takes is its own, a press beside it
/// the modal's. Its whole frame is its own too: a press on its thumb, on
/// the gutter the thumb stands in or on the frame's edge, off its entries,
/// takes nothing and never reaches the modal's row hidden under it. It
/// shows `n` of its `len` entries from the one that keeps the `cursor` in
/// sight ([`hung_first`]), and never more than stand whole inside the
/// modal's frame, over its pinned foot ([`hung_floor`]): an entry that
/// would reach past them is left out, as a list's frame drawn over the
/// window's edge or its buttons would hide them. Size the list with
/// [`hung_floor`] so its width keeps the thumb's gutter it then needs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn hung_over(
    ui: &Ui,
    id: Id,
    parent: Id,
    frame: Rect,
    cursor: Option<usize>,
    n: usize,
    len: usize,
    id_of: impl Fn(usize) -> Option<Id>,
    pal: &Pal,
    words: impl FnMut(&Ui, usize, Rect, bool),
) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let n = hung_floor(ui, parent).map_or(n, |floor| n.min(hung_fits(frame.min.y, floor)));
    if n == 0 {
        return None;
    }
    let frame = hung_frame(frame.min, frame.width(), n);
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(frame.min).constrain(false).fade_in(false);
    let first = hung_first(cursor, n, len);
    let shown = area.show(&ctx, |ui| {
        // Made before its entries, which so take their own presses over it:
        // egui gives a press to the topmost layer with a control under it,
        // and with nothing of the list's there that was the modal's row
        // beneath, whose setting it turned unseen.
        let _ = ui.interact(frame, id.with("frame"), Sense::click());
        hung_list(ui, frame, first, n, len, cursor, id_of, pal, words)
    });
    ctx.set_sublayer(egui::LayerId::new(egui::Order::Foreground, parent), egui::LayerId::new(egui::Order::Foreground, id));
    shown.inner
}

/// A list's thumb at `pane`'s right that the wheel moves and that takes
/// nothing itself (the MP3 tab's logs): the kit's thumb at rest,
/// [`THUMB_W`] wide, as long as the share of its `len` lines its `rows`
/// show and no shorter than [`THUMB_MIN`], standing where the view does,
/// `first` the line at its top.
pub(crate) fn list_thumb(ui: &Ui, pane: Rect, rows: usize, len: usize, first: usize, pal: &Pal) {
    let h = (pane.height() * rows as f32 / len.max(1) as f32).max(THUMB_MIN).min(pane.height());
    let travel = pane.height() - h;
    let top = pane.min.y + travel * first as f32 / len.saturating_sub(rows).max(1) as f32;
    let rect = Rect::from_min_size(pos2(pane.max.x - THUMB_W, top), vec2(THUMB_W, h));
    ui.painter().rect_filled(rect, CornerRadius::same((THUMB_W / 2.0) as u8), pal.dim.gamma_multiply(THUMB_REST));
}

/// A card's frame as the MP3 tab's boards draw every card: a 1-point
/// hairline in the edge colour round `rect`, never filled and never
/// changing.
pub(crate) fn hairline(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
}

/// A card's head's row: the name's 16 points on 22, and the least room
/// between the name and what stands at its right.
pub(crate) const HEAD_H: f32 = 22.0;
const HEAD_CLEAR: f32 = 16.0;

/// A card's head, a [`HEAD_H`] row from `at`, its top left, `w` wide:
/// `name` in the 16-point semibold at its left and, dim in the monospace at
/// its right on the name's baseline, `right` (a port, a serial, a drive and
/// its size), left out whole where it would come within 16 points of the
/// name. Returns where the next row starts.
pub(crate) fn card_head(ui: &Ui, at: Pos2, w: f32, name: &str, right: &str, pal: &Pal) -> f32 {
    let mid = at.y + HEAD_H / 2.0;
    let right_w = width_of(ui, right, mono(12.5));
    let name_w = width_of(ui, name, strong(16.0));
    let keep = !right.is_empty() && name_w + HEAD_CLEAR + right_w <= w;
    let room = if keep { w - right_w - HEAD_CLEAR } else { w };
    text(ui, pos2(at.x, mid), name, strong(16.0), pal.text, room, Align2::LEFT_CENTER);
    if keep {
        // A line centred on a point stands on the UI face's baseline at its
        // own size there, the smaller the higher: the figure is centred as
        // far under the name's middle as stands it on the name's.
        let at = pos2(at.x + w, mid + centred_baseline(ui, 16.0) - centred_baseline(ui, 12.5));
        text(ui, at, right, mono(12.5), pal.dim, right_w + 1.0, Align2::RIGHT_CENTER);
    }
    at.y + HEAD_H
}

/// A rule across a card or a sheet from `left` to `right` (a card's
/// Details, a sheet's help): a hairline in the edge colour, on the pixel
/// row from `top`.
pub(crate) fn rule(ui: &Ui, left: f32, right: f32, top: f32, pal: &Pal) {
    ui.painter().hline(left..=right, top + 0.5, Stroke::new(1.0, pal.edge));
}

/// A dim line's row: an explanation that never wraps, a choice's facts.
pub(crate) const DIM_LINE_H: f32 = 18.0;

/// One dim line from `at`, its row's top left, `w` wide, as the MP3 tab
/// sets every explanation that never wraps (the board's identity under
/// its head, the Advanced sheet's facts and notes): 13 points on
/// [`DIM_LINE_H`], its figures in the monospace (`also` among them), cut
/// with an ellipsis. Returns where the next row starts.
pub(crate) fn dim_line(ui: &Ui, at: Pos2, w: f32, words: &str, also: &[&str], pal: &Pal) -> f32 {
    let parts = figured(words, also, body(13.0), mono(12.5), pal.dim);
    runs(ui, pos2(at.x, at.y + DIM_LINE_H / 2.0), &parts, w.max(1.0), Align2::LEFT_CENTER);
    at.y + DIM_LINE_H
}

/// The cells' scan widget leading a line, in the desk's look: ten
/// [`meter`] segments 5 by 10 points, 2 apart, `lit` of them in the ok
/// green, from `at`, the line's left middle; and after them its `share` (a
/// percentage) dim in the monospace, standing on `share_on`, its words'
/// baseline ([`Para::baseline`]), and no further than `end`. Where the
/// line's words start: 10 points after the last of them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_lead(ui: &Ui, at: Pos2, lit: usize, share: Option<&str>, share_on: f32, end: f32, pal: &Pal) -> f32 {
    let bar = meter(ui, at, 10, lit, vec2(5.0, 10.0), 2.0, pal.ok, pal);
    let mut x = bar.max.x + 10.0;
    if let Some(share) = share {
        let mid = mid_on(ui, share_on, 12.5);
        x = text(ui, pos2(x, mid), share, mono(12.5), pal.dim, (end - x).max(1.0), Align2::LEFT_CENTER).max.x + 10.0;
    }
    x
}

impl Para {
    /// Lines `h` points apart whatever the first run's size: a sentence that
    /// opens on a figure ("21.4 of 59.6 GB used", a port) keeps its words'
    /// leading rather than the monospace's smaller one.
    pub(crate) fn spaced(self, h: f32) -> Para {
        let first = self.parts.first().map_or(h, |(_, font, _)| font.size);
        Para { line: h / first.max(1.0), ..self }
    }
}

/// Where a line set centred on its middle ([`text`], [`runs`], a
/// [`text_button`]'s words) whose first run is `size` points stands that
/// middle for its baseline to fall on `baseline`: a centred line stands on
/// the UI face's baseline at its size, under its middle, so words at a
/// row's right, a scan's share or a button beside a line stand on the
/// baseline of the prose at its left ([`Para::baseline`]) — the pixel egui
/// rounds the prose's to, whatever the face's ascent and whichever faces
/// the two are set in.
pub(crate) fn mid_on(ui: &Ui, baseline: f32, size: f32) -> f32 {
    baseline - centred_baseline(ui, size)
}

// ── The chrome's pieces: presses, tips, transport marks, a seek track ─────
//
// What the GUI's own top strip and transport bar drawn by egui need that no
// page did (src/gui/chrome.rs and the faces it hosts): a control that acts
// on the press, as the cells' controls do, and a list that picks so; the
// cells' tooltip dwell on any context, and a tooltip that says where it
// stood; the transport's marks, the frame they stand in and the seek line's
// track; a picture with its corners rounded. Nothing here knows the chrome.

/// How a list picks its entry: on the click, egui's way, or on the press,
/// the cells' way (the top bar's menu, whose cells act on the press).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pick {
    Click,
    Press,
}

/// Whether the primary button went down on `response`'s control in this
/// pass: the press the cells act on, not egui's click, which waits for the
/// release. The control senses clicks and was the topmost under the pointer
/// as the press came; a press and its release landing in the same pass are
/// a press too. A pass egui runs again hears no press, so a press counts
/// once.
pub(crate) fn pressed(response: &Response) -> bool {
    response.ctx.input(|i| i.pointer.primary_pressed()) && (response.is_pointer_button_down_on() || response.clicked())
}

/// The cells' tooltip dwell, set once on `ctx` after [`style_once`] (whose
/// tooltip shows at once): [`crate::kit::TIP_DELAY`] at rest on a target,
/// anew on each target as the cells' dwell restarts on each, and counted
/// from the pointer's last move however it moves after (egui's own wait for
/// a still pointer asks for a pass on every move, which a host that runs
/// passes only for news never gives it).
pub(crate) fn dwell(ctx: &egui::Context) {
    let id = Id::new("studio-dwell");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = crate::kit::TIP_DELAY.as_secs_f32();
        style.interaction.tooltip_grace_time = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// How far under its target a tooltip hangs, and the widest it grows.
pub(crate) const TIP_UNDER: f32 = 4.0;
pub(crate) const TIP_W: f32 = 300.0;
/// A tooltip's room round its words: the canvas's 6 by 10.
const TIP_PAD: Vec2 = vec2(10.0, 6.0);

/// The cells' tooltip for `response`, under [`dwell`]: `words` in Studio's
/// box ([`style_once`]'s: raised, on a strong-edged hairline) [`TIP_UNDER`]
/// points under the target's middle, pulled in at the screen's edge, in the
/// cells' lines (wrapped at their 40 cells, `kit::wrap_tip`) and no wider
/// than [`TIP_W`], once the pointer has rested on the target for the dwell,
/// and never in the dwell after a key (`since_key`, the time since the last
/// one), which hides it as it hides the cells'; the pass is asked for when
/// that dwell ends. Painted on the tooltips' layer, clipped to its own box,
/// which it returns while it shows: a host that cuts its paint around what
/// stands over it finds the tooltip's by it.
pub(crate) fn tip_under(response: &Response, words: &str, since_key: Option<std::time::Duration>, pal: &Pal) -> Option<Rect> {
    let ctx = &response.ctx;
    if let Some(since) = since_key.filter(|since| *since < crate::kit::TIP_DELAY) {
        ctx.request_repaint_after(crate::kit::TIP_DELAY - since);
        return None;
    }
    if words.is_empty() || !egui::Tooltip::should_show_tooltip(response, false) {
        return None;
    }
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Tooltip::tooltip_id(response.id, 0));
    let painter = ctx.layer_painter(layer);
    let lines = crate::kit::wrap_tip(words).join("\n");
    let galley = painter.layout(lines, body(12.5), pal.text, TIP_W - 2.0 * TIP_PAD.x);
    let size = galley.size() + 2.0 * TIP_PAD;
    let screen = ctx.content_rect();
    let target = response.interact_rect;
    let left = (target.center().x - size.x / 2.0).min(screen.max.x - size.x).max(screen.min.x);
    let tip = Rect::from_min_size(pos2(left, target.max.y + TIP_UNDER), size).round_to_pixels(ctx.pixels_per_point());
    let painter = painter.with_clip_rect(tip);
    painter.rect_filled(tip, square(), pal.raised);
    painter.rect_stroke(tip, square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
    painter.galley(tip.min + TIP_PAD, galley, pal.text);
    Some(tip)
}

/// `icon` in `color`, centred on `at`, with no button round it: an icon in
/// a slot too short for [`icon_button`]'s (the top bar's [+] in its row).
pub(crate) fn icon(ui: &Ui, at: Pos2, icon: Icon, color: Color32) {
    draw_icon(ui, at, icon, color);
}

/// The transport's marks, drawn in 14 points from the canvas's 16-unit
/// paths (Main.dc.html's TRANSPORT GROUP).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Prev,
    Next,
    Play,
    Pause,
    Repeat,
    Shuffle,
}

/// `mark` in `color`, centred on `at`: the solid ones filled, repeat's and
/// shuffle's arrows in 1.6-point strokes.
pub(crate) fn mark(ui: &Ui, at: Pos2, mark: Mark, color: Color32) {
    let k = 14.0 / 16.0;
    let o = at - vec2(7.0, 7.0);
    let p = |x: f32, y: f32| o + vec2(x, y) * k;
    let painter = ui.painter();
    let solid = |points: Vec<Pos2>| {
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    };
    let bar = |x: f32, w: f32| painter.rect_filled(Rect::from_min_max(p(x, 3.0), p(x + w, 13.0)), CornerRadius::ZERO, color);
    let stroke = Stroke::new(1.6, color);
    let path = |points: Vec<Pos2>| {
        painter.add(Shape::line(points, stroke));
    };
    // A quarter turn of radius 2 from `from` about `centre`, a few points.
    let arc = |centre: (f32, f32), from: f32, to: f32| -> Vec<Pos2> {
        (0..=4).map(|i| {
            let a = from + (to - from) * i as f32 / 4.0;
            p(centre.0 + 2.0 * a.cos(), centre.1 + 2.0 * a.sin())
        }).collect()
    };
    use std::f32::consts::{FRAC_PI_2, PI};
    match mark {
        // rect 3,3 2x10 and M13 3L6 8l7 5z.
        Mark::Prev => {
            bar(3.0, 2.0);
            solid(vec![p(13.0, 3.0), p(6.0, 8.0), p(13.0, 13.0)]);
        }
        Mark::Next => {
            bar(11.0, 2.0);
            solid(vec![p(3.0, 3.0), p(10.0, 8.0), p(3.0, 13.0)]);
        }
        // rects 4,3 and 9,3, 3x10.
        Mark::Pause => {
            bar(4.0, 3.0);
            bar(9.0, 3.0);
        }
        // The paused ▸, its ink's middle on the box's.
        Mark::Play => solid(vec![p(5.0, 3.0), p(13.0, 8.0), p(5.0, 13.0)]),
        // M3 7V6a2 2 0 0 1 2-2h7 M10 2l2 2-2 2 M13 9v1a2 2 0 0 1-2 2H4 M6 14l-2-2 2-2.
        Mark::Repeat => {
            let mut top = vec![p(3.0, 7.0)];
            top.extend(arc((5.0, 6.0), PI, PI + FRAC_PI_2));
            top.push(p(12.0, 4.0));
            path(top);
            path(vec![p(10.0, 2.0), p(12.0, 4.0), p(10.0, 6.0)]);
            let mut foot = vec![p(13.0, 9.0)];
            foot.extend(arc((11.0, 10.0), 0.0, FRAC_PI_2));
            foot.push(p(4.0, 12.0));
            path(foot);
            path(vec![p(6.0, 14.0), p(4.0, 12.0), p(6.0, 10.0)]);
        }
        // M2 4h3l6 8h3 M2 12h3l1.6-2.1 M9.4 6.1L11 4h3 M12 2l2 2-2 2 M12 10l2 2-2 2.
        Mark::Shuffle => {
            path(vec![p(2.0, 4.0), p(5.0, 4.0), p(11.0, 12.0), p(14.0, 12.0)]);
            path(vec![p(2.0, 12.0), p(5.0, 12.0), p(6.6, 9.9)]);
            path(vec![p(9.4, 6.1), p(11.0, 4.0), p(14.0, 4.0)]);
            path(vec![p(12.0, 2.0), p(14.0, 4.0), p(12.0, 6.0)]);
            path(vec![p(12.0, 10.0), p(14.0, 12.0), p(12.0, 14.0)]);
        }
    }
}

/// A transport button's look in `rect`, paint only (the cells take its
/// press): a `weight`-point frame in `frame` with the desk's corners and
/// `mark` in `ink` at its middle; while `hot`, the raised fill under it and
/// the frame a step brighter.
#[allow(clippy::too_many_arguments)]
pub(crate) fn frame_button(ui: &Ui, rect: Rect, mark: Mark, frame: Color32, ink: Color32, weight: f32, hot: bool, pal: &Pal) {
    let painter = ui.painter();
    let edge = if hot {
        painter.rect_filled(rect, square(), pal.raised);
        lift(frame, 40)
    } else {
        frame
    };
    painter.rect_stroke(rect, square(), Stroke::new(weight, edge), StrokeKind::Inside);
    self::mark(ui, rect.center(), mark, ink);
}

/// The seek line's track across `track`, paint only: a 2-point line at its
/// middle, gold over the `played` share and the strong edge after it, the
/// gold thumb ([`THUMB_W`] by 12, its corners softened by one) at the
/// playhead, and where the pointer would land a press (`ghost`, a share
/// too) the thumb's outline in the text colour. Shares are held to the
/// track.
pub(crate) fn seek_track(ui: &Ui, track: Rect, played: f32, ghost: Option<f32>, pal: &Pal) {
    let painter = ui.painter();
    let mid = track.center().y;
    let x = |share: f32| track.min.x + share.clamp(0.0, 1.0) * track.width();
    let thumb = |share: f32| {
        let centre = x(share).clamp(track.min.x + THUMB_W / 2.0, (track.max.x - THUMB_W / 2.0).max(track.min.x + THUMB_W / 2.0));
        Rect::from_center_size(pos2(centre, mid), vec2(THUMB_W, 12.0))
    };
    painter.line_segment([pos2(track.min.x, mid), pos2(track.max.x, mid)], Stroke::new(2.0, pal.edge_strong));
    painter.line_segment([pos2(track.min.x, mid), pos2(x(played), mid)], Stroke::new(2.0, pal.gold));
    painter.rect_filled(thumb(played), CornerRadius::same(1), pal.gold);
    if let Some(ghost) = ghost {
        painter.rect_stroke(thumb(ghost), CornerRadius::same(1), Stroke::new(1.0, pal.text), StrokeKind::Inside);
    }
}

/// [`picture`] with its corners rounded by `radius`: drawn whole in `rect`,
/// its shape kept, centred, never cropped or stretched. Returns where it
/// was drawn.
pub(crate) fn picture_in(ui: &Ui, rect: Rect, texture: &egui::TextureHandle, radius: u8) -> Rect {
    let size = texture.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 || !rect.is_positive() {
        return Rect::from_center_size(rect.center(), Vec2::ZERO);
    }
    let k = (rect.width() / size.x).min(rect.height() / size.y);
    let at = Rect::from_center_size(rect.center(), size * k);
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let shape = egui::epaint::RectShape::filled(at, CornerRadius::same(radius), Color32::WHITE).with_texture(texture.id(), uv);
    ui.painter().add(shape);
    at
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
        // Styled as every host styles the context first.
        style_once(ctx, &Pal::now());
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
        let id = Id::new("probe-modal");
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            if ui.interact(over, Id::new("beneath"), Sense::click()).clicked() {
                beneath_clicked.set(true);
            }
            let shown = Dialog::new(id, over, 480.0, "Add a user").show(ui, &pal, |ui| {
                ui.add_space(120.0);
            });
            if shown.closed {
                closed.set(true);
            }
        };
        // Where the window stood: centred on `over`, its close at its top
        // right, as tall as it last took.
        let window = |ctx: &egui::Context| {
            let close = ctx.read_response(id.with("close")).expect("the close is drawn").rect.center();
            let h = ctx.data(|d| d.get_temp::<f32>(id.with("height"))).expect("the window took a height");
            let w = 2.0 * (close.x + 8.0 + MODAL_PAD - over.center().x);
            Rect::from_center_size(over.center(), vec2(w, h))
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
        let at = window(&ctx);

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

    /// Every text a pass drew, as laid out, with where.
    fn texts(out: &egui::FullOutput) -> Vec<(String, Rect)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()))),
                _ => None,
            })
            .collect()
    }

    fn click_events(at: Pos2) -> Vec<Vec<egui::Event>> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        vec![vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]]
    }

    #[test]
    fn the_cells_marks_and_keys_come_off_a_label() {
        assert_eq!(bare("• running"), "running");
        assert_eq!(bare("◂ Keep it"), "Keep it");
        assert_eq!(bare("Add folder ▸"), "Add folder");
        assert_eq!(bare("✓ claimed "), "claimed");
        assert_eq!(unkeyed("y copy"), ("copy", Some("y")));
        assert_eq!(unkeyed("^T test"), ("test", Some("^T")));
        assert_eq!(unkeyed("/ filter"), ("filter", Some("/")));
        assert_eq!(unkeyed("Switch client ▸"), ("Switch client ▸", None), "a word of its own is no key");
        assert_eq!(unkeyed("open"), ("open", None));
    }

    #[test]
    fn figures_are_cut_out_of_a_sentence_in_order_and_the_rest_kept() {
        let (font, figure, c) = (body(14.0), mono(12.5), Color32::WHITE);
        let parts = with_figures(" — music → /srv/b · 3m 2s", &["/srv/b", "3m 2s"], font.clone(), figure.clone(), c);
        let words: Vec<(&str, bool)> = parts.iter().map(|(s, f, _)| (s.as_str(), *f == figure)).collect();
        assert_eq!(words, [(" — music → ", false), ("/srv/b", true), (" · ", false), ("3m 2s", true)]);
        let none = with_figures("nothing here", &["", "zz"], font.clone(), figure, c);
        assert_eq!(none.len(), 1, "a figure not found is passed over");
    }

    #[test]
    fn columns_narrow_together_only_where_the_flexible_one_would_starve() {
        let cols = || vec![Col::left("A", 200.0), Col::left("B", 0.0), Col::left("C", 100.0)];
        let wide = fitted(1000.0, cols(), 120.0, 0.4);
        assert_eq!(wide.iter().map(|c| c.w).collect::<Vec<_>>(), [200.0, 0.0, 100.0], "room enough: the canvas's");
        let narrow = fitted(400.0, cols(), 120.0, 0.4);
        let (a, c) = (narrow[0].w, narrow[2].w);
        assert!(a < 200.0 && c < 100.0 && (a / c - 2.0).abs() < 0.05, "narrowed together: {a} {c}");
        let floor = fitted(100.0, cols(), 120.0, 0.4);
        assert_eq!(floor[0].w, 80.0, "never under the floor's share");
    }

    #[test]
    fn a_rooms_parts_stand_inside_the_canvas_margins() {
        let (body, note) = room_rects(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 420.0)));
        assert_eq!(body.min, pos2(24.0, 20.0));
        assert_eq!(note.max, pos2(776.0, 406.0));
        assert_eq!(note.height(), NOTE_H);
        assert_eq!(note.min.y - body.max.y, ROOM_GAP);
    }

    #[test]
    fn text_pieces_cut_label_and_wrap_as_the_canvas_sets_them() {
        let ctx = context();
        let size = vec2(600.0, 400.0);
        let mut got = Vec::new();
        let out = pass(&ctx, size, Vec::new(), &mut |ui| {
            // A path cut at its start keeps its leaf, and no accent torn off
            // its letter after the ellipsis.
            let path = "/srv/e\u{301}te\u{301}/recordings/leaf";
            let (cut, was) = cut_start(ui, path, mono(12.5), 90.0);
            got.push((cut.clone(), was));
            let (whole, was) = cut_start(ui, "/srv", mono(12.5), 90.0);
            got.push((whole, was));
            let pal = Pal::now();
            label_at(ui, pos2(10.0, 20.0), "Destination · another drive", 400.0, &pal);
            // Two lines at most, the second cut; and a floor that leaves out
            // a line that would cross it.
            let long = "word ".repeat(80);
            let two = Para::new(&long, body(13.0), pal.text).rows(2).paint(ui, pos2(10.0, 60.0), 200.0);
            got.push((format!("{}", (two.height() / (13.0 * 1.3)).round()), false));
            let floored = Para::new(&long, body(13.0), pal.text).floor(60.0 + 3.0 * 13.0 * 1.3 + 0.5).paint(ui, pos2(300.0, 60.0), 200.0);
            got.push((format!("{}", floored.height() > 3.0 * 13.0 * 1.3), false));
            let hung = Para::new("Users who hold it lose it", body(13.0), pal.text).lead("−").paint(ui, pos2(10.0, 200.0), 400.0);
            got.push((format!("{}", hung.min.x), false));
        });
        assert!(got[0].1 && got[0].0.starts_with('…') && got[0].0.ends_with("leaf"), "{:?}", got[0]);
        assert!(!got[0].0.starts_with("…\u{301}"), "no mark torn from its letter");
        assert_eq!(got[1], ("/srv".to_string(), false), "a short path stays whole");
        let drawn = texts(&out);
        assert!(drawn.iter().any(|(t, _)| t == "DESTINATION"), "the head in capitals: {drawn:?}");
        let hint = drawn.iter().find(|(t, _)| t == "· another drive").expect("the hint, from its dot");
        let head = drawn.iter().find(|(t, _)| t == "DESTINATION").expect("the head").1;
        assert!(hint.1.min.x > head.max.x + 4.0, "a space after the head");
        assert_eq!(got[2].0, "2", "two lines, no more");
        let dash = drawn.iter().find(|(t, _)| t == "−").expect("the lead in a column of its own").1;
        let words = drawn.iter().find(|(t, _)| t.starts_with("Users who")).expect("the words").1;
        assert!(words.min.x >= dash.max.x + 7.0, "the words hang beside the dash: {dash:?} {words:?}");
    }

    #[test]
    fn a_foot_lays_its_buttons_from_either_end_and_says_which_was_clicked() {
        let ctx = context();
        let size = vec2(600.0, 200.0);
        let row = Rect::from_min_size(pos2(20.0, 50.0), vec2(500.0, FORM_BUTTON_H));
        let clicked = Cell::new(None);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let got = foot_at(ui, Id::new("probe-foot"), row, &[("◂ up", false)], &[("close", false), ("Add ▸", true)], None, &pal);
            if got.is_some() {
                clicked.set(got);
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let drawn = texts(&out);
        assert!(drawn.iter().any(|(t, _)| t == "UP") && drawn.iter().any(|(t, _)| t == "ADD"), "bare words: {drawn:?}");
        let first = ctx.read_response(Id::new("probe-foot").with(0usize)).expect("start").rect;
        let last = ctx.read_response(Id::new("probe-foot").with(2usize)).expect("end").rect;
        assert_eq!((first.min.x, last.max.x), (row.min.x, row.max.x), "from the left, to the right");
        let close = ctx.read_response(Id::new("probe-foot").with(1usize)).expect("close").rect;
        assert!((last.min.x - close.max.x - 10.0).abs() < 0.01, "10 points apart");
        for events in click_events(close.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(clicked.get(), Some(1), "counted from the start");
    }

    #[test]
    fn an_unfocused_long_field_is_cut_with_an_ellipsis_and_a_focused_one_shows_no_torn_letter() {
        let ctx = context();
        let size = vec2(400.0, 200.0);
        let rect = Rect::from_min_size(pos2(20.0, 40.0), vec2(160.0, FIELD_H));
        let long = "mstrfed1:eyJ0IjoiYWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoifQ";
        let out = pass(&ctx, size, Vec::new(), &mut |ui| {
            let _ = field_box(ui, Id::new("probe-idle"), rect, &Input::new(long, 0).mono(), &Pal::now());
        });
        let shape = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == long => Some(t.galley.clone()),
                _ => None,
            })
            .expect("the line drawn");
        assert!(shape.size().x <= rect.width() - 2.0 * FIELD_PAD + 0.5, "cut to the box: {}", shape.size().x);
        assert!(shape.rows[0].row.glyphs.last().is_some_and(|g| g.chr == '…'), "with an ellipsis");
        // Scrolled to its end, the first letter in view starts at or after
        // the view's edge.
        let galley = shape;
        assert_eq!(whole_glyph(&galley, 0.0), 0.0);
        let x = whole_glyph(&galley, 3.0);
        assert!(x >= 3.0 && galley.rows[0].row.glyphs.iter().any(|g| (g.pos.x - x).abs() < 0.01), "a letter's edge: {x}");
    }

    #[test]
    fn a_modal_comes_up_at_its_top_however_it_was_left_scrolled() {
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-scroller");
        let up = Cell::new(true);
        let at = Cell::new(0usize);
        // Where the last pass drew rows 0 and 29 (egui's own record of a
        // widget can be a discarded pass's).
        let first = Cell::new(Rect::NOTHING);
        let last = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            if !up.get() {
                return;
            }
            let pal = Pal::now();
            let _ = Dialog::new(id, over, 400.0, "Rows").show(ui, &pal, |ui| {
                for i in 0..30 {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
                    match i {
                        0 => first.set(rect),
                        29 => last.set(rect),
                        _ => {}
                    }
                    if i == at.get() {
                        reveal(ui, id, i, rect);
                    }
                }
            });
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let home = first.get().min.y;
        // The keys move to the last row: within the one frame, with no
        // other input, the body stands where it shows it.
        at.set(29);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let row = last.get();
        assert!(row.max.y <= size.y - 16.0 - 24.0 + 0.5, "the cursor's row whole in the body: {row:?}");
        assert!(first.get().min.y < home - 100.0, "the body scrolled");
        // Down for a pass, then up again: at its top on its first pass.
        up.set(false);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        up.set(true);
        at.set(0);
        let _ = pass(&ctx, size, Vec::new(), &mut run);

        assert!((first.get().min.y - home).abs() < 0.5, "at its top: {:?} for {home}", first.get());
    }

    #[test]
    fn a_gate_has_no_close_and_wears_the_warnings_gold_and_its_title_wraps_whole() {
        let ctx = context();
        let size = vec2(600.0, 400.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-gate");
        let title = "Remove Boards of Canada - Music Has the Right to Children (1998) [FLAC] from Transmission?";
        let answered = Cell::new(None);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let sentences = vec![Para::new("The files stay.", body(14.0), pal.gold), Para::new("Only the record goes.", body(14.0), pal.gold)];
            if let Some(go) = gate(ui, id, over, title, sentences, ["Keep", "Remove"], &pal).inner {
                answered.set(Some(go));
            }
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert!(ctx.read_response(id.with("close")).is_none(), "no close");
        let galley = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == title => Some(t.galley.clone()),
                _ => None,
            })
            .expect("the title");
        assert!(galley.rows.len() >= 2, "wrapped, its question kept");
        assert!(!galley.elided, "never cut short");
        assert_eq!(galley.job.sections[0].format.color, Pal::now().gold, "in the warning's gold");
        let drawn = texts(&out);
        let at = |s: &str| drawn.iter().find(|(t, _)| t == s).unwrap_or_else(|| panic!("{s} drawn: {drawn:?}")).1;
        assert!((at("Only the record goes.").min.y - at("The files stay.").max.y - 6.0).abs() < 0.5, "its sentences 6 points apart");
        let keep = ctx.read_response(id.with("foot").with(0usize)).expect("the safe choice").rect;
        assert!(keep.min.y - at("Only the record goes.").max.y >= 20.0 - 0.5, "the foot 20 points under them");
        for events in click_events(keep.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(answered.get(), Some(false), "the safe choice keeps");
    }

    #[test]
    fn a_held_modal_keeps_its_top_as_its_body_grows() {
        let ctx = context();
        let size = vec2(600.0, 500.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-held");
        let rows = Cell::new(0usize);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let _ = Dialog::new(id, over, 400.0, "Path").hold(200.0).show(ui, &pal, |ui| {
                ui.add_space(20.0);
                for _ in 0..rows.get() {
                    ui.add_space(30.0);
                }
            });
        };
        let close_y = |ctx: &egui::Context| ctx.read_response(id.with("close")).expect("the close").rect.center().y;
        for _ in 0..2 {
            let _ = pass(&ctx, size, Vec::new(), &mut run);
        }
        let short = close_y(&ctx);
        rows.set(6);
        for _ in 0..2 {
            let _ = pass(&ctx, size, Vec::new(), &mut run);
        }
        assert!((close_y(&ctx) - short).abs() < 0.5, "the title held where it stood");
    }

    #[test]
    fn a_pinned_foot_stands_whole_under_the_bodys_view_while_a_long_body_scrolls_over_it() {
        let ctx = context();
        let id = Id::new("probe-footed");
        for (h, rows) in [(500.0, 3), (300.0, 30)] {
            let size = vec2(600.0, h);
            let over = Rect::from_min_size(Pos2::ZERO, size);
            let mut run = |ui: &mut Ui| {
                let pal = Pal::now();
                let dialog = Dialog::new(id, over, 400.0, "Sheet").foot(54.0);
                let room = dialog.body_room(ui);
                assert!(room.y <= h - 32.0 - 2.0 * MODAL_PAD - TITLE_H - MODAL_GAP - 54.0 + 0.01, "the foot's room is the body's no more: {room:?}");
                let _ = dialog.show_footed(
                    ui,
                    &pal,
                    |ui| {
                        for k in 0..rows {
                            line(ui, 24.0, &format!("row {k:02}"), body(14.0), pal.text);
                        }
                    },
                    |ui, _| {
                        ui.add_space(20.0);
                        line(ui, 34.0, "FOOT", body(14.0), pal.text);
                    },
                );
            };
            let mut out = pass(&ctx, size, Vec::new(), &mut run);
            for _ in 0..2 {
                out = pass(&ctx, size, Vec::new(), &mut run);
            }
            let clipped = |words: &str| {
                out.shapes.iter().find_map(|c| match &c.shape {
                    Shape::Text(t) if t.galley.text() == words => Some((Rect::from_min_size(t.pos, t.galley.size()), c.clip_rect)),
                    _ => None,
                })
            };
            let (foot, clip) = clipped("FOOT").expect("the foot drawn");
            assert!(clip.contains_rect(foot) && foot.max.y <= h - 16.0 - MODAL_PAD + 0.5, "{h}: the foot whole in the window: {foot:?} in {clip:?}");
            let (first, view) = clipped("row 00").expect("the body drawn");
            assert!(first.min.y >= view.min.y - 0.5, "{h}: the body from its top");
            if rows == 30 {
                assert!(view.max.y <= foot.min.y - 20.0 + 0.5, "{h}: the scrolled body's view ends over the foot's room: {view:?} over {foot:?}");
            } else {
                // Row 00's middle 12 under the body's top; the foot's line
                // 20 under the three rows, its middle 17 into it.
                let want = first.center().y - 12.0 + 3.0 * 24.0 + 20.0 + 17.0;
                assert!((foot.center().y - want).abs() < 1.0, "a short body: the foot right under it: {first:?} {foot:?}");
            }
        }
    }

    /// Every one-line text a pass drew with where it stands and what
    /// clipped it.
    fn clipped_texts(out: &egui::FullOutput) -> Vec<(String, Rect, Rect)> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.rows.len() == 1 => Some((t.galley.text().to_string(), Rect::from_min_size(t.pos, t.galley.size()), c.clip_rect)),
                _ => None,
            })
            .collect()
    }

    /// The one-line texts a pass drew torn by their clip's foot or its top:
    /// in view in part, not whole.
    fn torn(out: &egui::FullOutput) -> Vec<String> {
        let cut = |r: Rect, edge: f32| edge > r.min.y + 0.5 && edge < r.max.y - 0.5;
        clipped_texts(out).into_iter().filter(|(_, r, clip)| clip.intersects(*r) && (cut(*r, clip.max.y) || cut(*r, clip.min.y))).map(|(t, ..)| t).collect()
    }

    #[test]
    fn a_pinned_head_stands_whole_over_the_bodys_view_wherever_the_body_is_scrolled() {
        let ctx = context();
        let id = Id::new("probe-headed");
        let size = vec2(600.0, 340.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let at = Cell::new(0usize);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let dialog = Dialog::new(id, over, 400.0, "Sheet").head(44.0).foot(54.0);
            let room = dialog.body_room(ui);
            assert!(room.y <= size.y - 32.0 - 2.0 * MODAL_PAD - TITLE_H - MODAL_GAP - 44.0 - 54.0 + 0.01, "the head's room is the body's no more: {room:?}");
            let _ = dialog.show_pinned(
                ui,
                &pal,
                |ui| {
                    line(ui, 30.0, "HEAD", body(14.0), pal.text);
                },
                |ui| {
                    for k in 0..30 {
                        let row = line(ui, 24.0, &format!("row {k:02}"), body(14.0), pal.text);
                        if k == at.get() {
                            reveal(ui, id, k, row);
                        }
                    }
                },
                |ui, _| {
                    ui.add_space(20.0);
                    line(ui, 34.0, "FOOT", body(14.0), pal.text);
                },
            );
        };
        let settle = |run: &mut dyn FnMut(&mut Ui)| {
            let mut out = pass(&ctx, size, Vec::new(), run);
            for _ in 0..2 {
                out = pass(&ctx, size, Vec::new(), run);
            }
            out
        };
        let out = settle(&mut run);
        let find = |out: &egui::FullOutput, words: &str| clipped_texts(out).into_iter().find(|(t, ..)| t == words).unwrap_or_else(|| panic!("{words:?} drawn"));
        let (_, head, clip) = find(&out, "HEAD");
        assert!(clip.contains_rect(head), "the head whole: {head:?} in {clip:?}");
        let (_, first, view) = find(&out, "row 00");
        assert!(first.min.y >= head.max.y && view.min.y >= head.max.y - 0.5, "the body under the head: {first:?} {view:?}");
        // The keys at the last row: the body scrolls under the head, which
        // stands where it stood, whole.
        at.set(29);
        let out = settle(&mut run);
        let (_, moved, clip) = find(&out, "HEAD");
        assert!((moved.min.y - head.min.y).abs() < 0.5 && clip.contains_rect(moved), "the head held whole: {moved:?} in {clip:?}");
        let (_, last, view) = find(&out, "row 29");
        assert!(view.contains_rect(last) && view.min.y >= moved.max.y - 0.5, "the last row whole under the head: {last:?} in {view:?}");
        let (_, gone, view) = find(&out, "row 00");
        assert!(!view.intersects(gone), "the first rows scrolled away under it");
    }

    #[test]
    fn a_scrolled_bodys_view_ends_between_its_rows_and_the_keys_bring_a_row_in_tearing_none() {
        let ctx = context();
        let id = Id::new("probe-rows");
        // A body 180 points tall for rows 28 points apart: its foot falls
        // inside the seventh.
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let at = Cell::new(0usize);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let _ = Dialog::new(id, over, 400.0, "Rows").show(ui, &pal, |ui| {
                for k in 0..30 {
                    if k > 0 {
                        ui.add_space(4.0);
                    }
                    let row = line(ui, 24.0, &format!("row {k:02}"), body(14.0), pal.text);
                    if k == at.get() {
                        reveal(ui, id, k, row);
                    }
                }
            });
        };
        let settle = |run: &mut dyn FnMut(&mut Ui)| {
            let mut out = pass(&ctx, size, Vec::new(), run);
            for _ in 0..2 {
                out = pass(&ctx, size, Vec::new(), run);
            }
            out
        };
        let out = settle(&mut run);
        let view = |out: &egui::FullOutput| clipped_texts(out).into_iter().find(|(t, ..)| t == "row 00").map(|(.., clip)| clip).expect("the body drawn");
        let shown = |out: &egui::FullOutput| clipped_texts(out).into_iter().filter(|(t, r, clip)| t.starts_with("row ") && clip.contains_rect(*r)).map(|(t, ..)| t).collect::<Vec<_>>();
        assert!(torn(&out).is_empty(), "at its top, no row torn: {:?}", torn(&out));
        assert_eq!(shown(&out).len(), 6, "six rows whole, the seventh under the view's foot: {:?}", shown(&out));
        let full = size.y - 32.0 - 2.0 * MODAL_PAD - TITLE_H - MODAL_GAP;
        assert!(view(&out).height() < full - 1.0, "the view ended over the row it would cut: {:?}", view(&out));
        // The keys far down and back up: each row comes in whole, none torn
        // at either end of the view.
        for k in [20, 29, 3, 0] {
            at.set(k);
            let out = settle(&mut run);
            let row = format!("row {k:02}");
            assert!(shown(&out).contains(&row), "{row} in view: {:?}", shown(&out));
            assert!(torn(&out).is_empty(), "at {row}, no row torn: {:?}", torn(&out));
        }
        // At rest the pass asks for no other: no repaint loop.
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay);
        assert_eq!(delay, Some(std::time::Duration::MAX), "a body at rest asks for no pass of its own");
    }

    #[test]
    fn a_body_whose_rows_say_nothing_scrolls_a_row_at_a_time_as_the_keys_step_past_its_foot() {
        let ctx = context();
        let id = Id::new("probe-silent-rows");
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let at = Cell::new(0usize);
        let (first, cursor) = (Cell::new(Rect::NOTHING), Cell::new(Rect::NOTHING));
        // Rows set at places of the body's own, as the Users room's grant
        // sets its libraries: none says where it is.
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let _ = Dialog::new(id, over, 400.0, "Rows").show(ui, &pal, |ui| {
                let w = ui.available_width();
                for k in 0..30 {
                    let (_, rect) = ui.allocate_space(vec2(w, 30.0));
                    if k == 0 {
                        first.set(rect);
                    }
                    if k == at.get() {
                        cursor.set(rect);
                        reveal(ui, id, k, rect);
                    }
                }
            });
        };
        let settle = |run: &mut dyn FnMut(&mut Ui)| {
            for _ in 0..3 {
                let _ = pass(&ctx, size, Vec::new(), run);
            }
        };
        settle(&mut run);
        let home = first.get().min.y;
        let foot = ctx.data(|d| d.get_temp::<BodyView>(id.with("view"))).expect("the body's view").view.max.y;
        let mut was = 0.0;
        let mut moved = 0;
        for k in 1..16 {
            at.set(k);
            settle(&mut run);
            let scrolled = home - first.get().min.y;
            assert!(cursor.get().max.y <= foot + 0.5, "row {k} whole in view: {:?} over {foot}", cursor.get());
            assert!(scrolled - was <= 30.0 + 0.5, "a step down moves the body a row at most, not a page: {was} then {scrolled} at row {k}");
            if scrolled > was + 0.5 {
                moved += 1;
            }
            was = scrolled;
        }
        assert!(moved >= 8, "the body followed the keys down: {moved} steps moved it");
    }

    /// The ink of the text `words` a pass drew, and what clipped it.
    fn ink_of(out: &egui::FullOutput, words: &str) -> (Rect, Rect) {
        out.shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == words => Some((t.visual_bounding_rect(), c.clip_rect)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{words:?} drawn: {:?}", texts(out)))
    }

    /// A modal's body wheeled as far down as it goes, and the passes egui
    /// eases the wheel over.
    fn wheel_to_end(ctx: &egui::Context, size: Vec2, run: &mut dyn FnMut(&mut Ui)) -> egui::FullOutput {
        let turn = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -5000.0), phase: egui::TouchPhase::Move, modifiers: egui::Modifiers::default() };
        let _ = pass(ctx, size, vec![egui::Event::PointerMoved(pos2(size.x / 2.0, size.y / 2.0)), turn], run);
        for _ in 0..60 {
            let _ = pass(ctx, size, Vec::new(), run);
        }
        pass(ctx, size, vec![egui::Event::PointerGone], run)
    }

    #[test]
    fn a_fields_box_stands_whole_under_a_view_whose_top_cuts_only_its_label() {
        // The Torrents room's entries at the GUI's floor: the body scrolls
        // only a few points, its view's top through the field's label. The
        // label goes; its box, with the caret, stays in sight.
        let ctx = context();
        let id = Id::new("probe-field-cut");
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let (boxed, caret) = (Cell::new(Rect::NOTHING), Cell::new(None));
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let dialog = Dialog::new(id, over, 400.0, "Map vinyl");
            // Half a label more than the view holds: the body at its end has
            // its view's top through the label's capitals.
            let room = dialog.body_room(ui).y;
            let _ = dialog.show(ui, &pal, |ui| {
                let typed = field(ui, Id::new("probe-path"), "PATH AS SEEN", &Input::new("/srv", 4).focused(true, ""), &pal);
                boxed.set(typed.response.rect);
                caret.set(typed.caret);
                ui.add_space(room - FIELD_ROW_H + LABEL_H / 2.0);
            });
        };
        for _ in 0..3 {
            let _ = pass(&ctx, size, Vec::new(), &mut run);
        }
        let out = wheel_to_end(&ctx, size, &mut run);
        let view = ctx.data(|d| d.get_temp::<BodyView>(id.with("view"))).expect("the body's view").view;
        let (label, _) = ink_of(&out, "PATH AS SEEN");
        assert!(label.min.y < view.min.y, "the body scrolled into its label: {label:?} over {view:?}");
        assert!(view.min.y >= label.max.y - 0.5, "the label left out whole, none of its capitals torn: {label:?} under {view:?}");
        let boxed = boxed.get();
        assert!(view.expand(0.5).contains_rect(boxed), "the box whole in view: {boxed:?} in {view:?}");
        let caret = caret.get().expect("the focused field's caret");
        assert!(view.contains(caret - vec2(0.0, 1.0)), "its caret in sight, where the candidates float: {caret:?} in {view:?}");
    }

    #[test]
    fn a_line_shows_whole_where_the_views_foot_cuts_only_the_empty_room_under_its_words() {
        // The Federation mint at the GUI's default size: the view's foot
        // reached a line of ticks' room under its boxes, never its boxes or
        // its words, and the line went. A label, a check line, a line of
        // ticks and a list's row each say where their words are, not their
        // row's room, so each stands whole there; the keyboard's check
        // line, its look as tall as its row, goes whole instead.
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        for (k, (piece, row_h)) in [("label", LABEL_H), ("check", HIT), ("ticks", 32.0), ("list", 34.0), ("focused", HIT)].into_iter().enumerate() {
            let id = Id::new("probe-room-under").with(k);
            let mut run = |ui: &mut Ui| {
                let pal = Pal::now();
                let dialog = Dialog::new(id, over, 400.0, "Rows");
                let room = dialog.body_room(ui).y;
                let _ = dialog.show(ui, &pal, |ui| {
                    // The row's foot 2 points under the view's.
                    ui.add_space(room - row_h + 2.0);
                    match piece {
                        "label" => {
                            label(ui, "LIBRARIES", &pal);
                        }
                        "check" | "focused" => {
                            let _ = check_line(ui, Id::new("probe-check"), true, "server audio", "", piece == "focused", &pal);
                        }
                        "ticks" => {
                            let items = [("kids".to_string(), false), ("radio-archive".to_string(), true)];
                            let _ = tick_grid(ui, Id::new("probe-ticks"), &items, None, 3, row_h, &pal);
                        }
                        _ => {
                            let (rect, _) = list_row(ui, Id::new("probe-list"), row_h, false, &pal);
                            text(ui, pos2(rect.min.x + 12.0, rect.center().y), "b11c0d0e0b", mono(12.5), pal.text, rect.width(), Align2::LEFT_CENTER);
                        }
                    }
                    ui.add_space(60.0);
                });
            };
            let mut out = pass(&ctx, size, Vec::new(), &mut run);
            for _ in 0..2 {
                out = pass(&ctx, size, Vec::new(), &mut run);
            }
            let words = match piece {
                "label" => "LIBRARIES",
                "check" | "focused" => "server audio",
                "ticks" => "radio-archive",
                _ => "b11c0d0e0b",
            };
            let (ink, clip) = ink_of(&out, words);
            if piece == "focused" {
                assert!(clip.max.y <= ink.min.y + 0.5, "the keyboard's line goes whole, its look never torn: {ink:?} under {clip:?}");
            } else {
                assert!(clip.expand(0.5).contains_rect(ink), "{piece}: its words whole in view: {ink:?} in {clip:?}");
            }
        }
    }

    #[test]
    fn a_views_foot_ends_over_a_forms_button_whose_words_it_would_cut_and_never_hides_a_gates() {
        // Federation's new ticket at the GUI's default size: the view's foot
        // ran a few points into its buttons, a sliver of them under the
        // ticket. A form's buttons are a row down to their words: a view
        // whose foot would cut the words ends over the buttons; one whose
        // foot falls under the words lets them stand, read and clicked; a
        // gate's answers are never hidden for the view's foot.
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        for (k, (gate_foot, shown)) in [(false, 6.0), (false, 28.0), (true, 6.0)].into_iter().enumerate() {
            let id = Id::new("probe-foot-cut").with(k);
            let button = Cell::new(Rect::NOTHING);
            let mut run = |ui: &mut Ui| {
                let pal = Pal::now();
                let dialog = Dialog::new(id, over, 400.0, "Ticket");
                let room = dialog.body_room(ui).y;
                let _ = dialog.show(ui, &pal, |ui| {
                    // The buttons' top `shown` points over the view's foot.
                    ui.add_space(room - shown);
                    button.set(Rect::from_min_size(ui.cursor().min, vec2(1.0, FORM_BUTTON_H)));
                    if gate_foot {
                        let _ = answers(ui, Id::new("probe-answers"), &Answers { keep: "Keep", lead: false, go: Some("Remove"), also: &[] }, &pal);
                    } else {
                        let _ = foot(ui, Id::new("probe-foot"), &[], &[("Done", true)], None, &pal);
                    }
                    ui.add_space(60.0);
                });
            };
            for _ in 0..3 {
                let _ = pass(&ctx, size, Vec::new(), &mut run);
            }
            let view = ctx.data(|d| d.get_temp::<BodyView>(id.with("view"))).expect("the body's view").view;
            let top = button.get().min.y;
            if k == 0 {
                assert!(view.max.y <= top + 0.5, "no sliver of the button: the view ends over it: {view:?} over {top}");
            } else {
                let why = if gate_foot { "a gate's answers" } else { "the words whole" };
                assert!(view.max.y >= top + shown - 0.5, "{why}: the buttons stand under the view's foot: {view:?} over {top}");
            }
        }
    }

    #[test]
    fn a_list_hung_over_a_modal_stops_inside_its_frame_over_its_pinned_foot() {
        let ctx = context();
        let size = vec2(600.0, 400.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        for footed in [true, false] {
            let id = Id::new("probe-hung-over").with(footed);
            let (window, floor, foot) = (Cell::new(Rect::NOTHING), Cell::new(0.0), Cell::new(Rect::NOTHING));
            let mut run = |ui: &mut Ui| {
                let pal = Pal::now();
                let dialog = Dialog::new(id, over, 400.0, "Sheet").foot(if footed { 54.0 } else { 0.0 });
                let shown = dialog.show_footed(
                    ui,
                    &pal,
                    |ui| {
                        for k in 0..6 {
                            line(ui, 24.0, &format!("row {k}"), body(14.0), pal.text);
                        }
                    },
                    |ui, _| {
                        ui.add_space(20.0);
                        foot.set(line(ui, 34.0, "FOOT", body(14.0), pal.text));
                    },
                );
                window.set(shown.window);
                floor.set(hung_floor(ui, id).expect("the modal's floor"));
                // Ten entries hung from under the body's first row: more
                // than stand over the foot or inside the frame.
                let top = shown.window.min.y + MODAL_PAD + TITLE_H + MODAL_GAP + 24.0;
                let frame = hung_frame(pos2(shown.window.min.x + 120.0, top), 160.0, 10);
                let words = |ui: &Ui, k: usize, room: Rect, _on: bool| {
                    text(ui, room.left_center(), &format!("entry {k}"), body(14.0), pal.text, room.width(), Align2::LEFT_CENTER);
                };
                let _ = hung_over(ui, id.with("list"), id, frame, Some(0), 10, 10, |_| None, &pal, words);
            };
            let mut out = pass(&ctx, size, Vec::new(), &mut run);
            for _ in 0..2 {
                out = pass(&ctx, size, Vec::new(), &mut run);
            }
            let entries: Vec<Rect> = clipped_texts(&out).into_iter().filter(|(t, ..)| t.starts_with("entry ")).map(|(_, r, _)| r).collect();
            assert!(!entries.is_empty() && entries.len() < 10, "{footed}: some entries, not all: {}", entries.len());
            let lowest = entries.iter().map(|r| r.max.y).fold(0.0, f32::max);
            assert!(lowest <= floor.get() && floor.get() <= window.get().max.y - MODAL_PAD + 0.5, "{footed}: inside the frame: {lowest} over {} in {:?}", floor.get(), window.get());
            if footed {
                assert!(floor.get() <= foot.get().min.y - 20.0 + 0.5, "over the pinned foot's room: {} over {:?}", floor.get(), foot.get());
            }
        }
    }

    #[test]
    fn a_list_that_hangs_shows_the_entries_that_keep_its_cursor_in_sight() {
        assert_eq!(hung_first(None, 5, 8), 0);
        assert_eq!(hung_first(Some(2), 5, 8), 0, "the cursor in the first five: from the top");
        assert_eq!(hung_first(Some(5), 5, 8), 1, "the cursor just past them: the list a step down");
        assert_eq!(hung_first(Some(7), 5, 8), 3, "on the last: the last five");
        assert_eq!(hung_first(Some(7), 10, 8), 0, "room for all: from the top");
    }

    #[test]
    fn a_lit_field_inks_its_words_in_the_accent_only_while_it_has_the_keyboard() {
        let ctx = context();
        let size = vec2(400.0, 200.0);
        let pal = Pal::now();
        for (focused, lit, ink) in [(true, true, pal.accent), (false, true, pal.text), (true, false, pal.text)] {
            let mut run = |ui: &mut Ui| {
                let input = Input::new("Car music", 9).focused(focused, "");
                let input = if lit { input.lit() } else { input };
                let _ = field_box(ui, Id::new("probe-lit"), Rect::from_min_size(pos2(20.0, 20.0), vec2(300.0, FIELD_H)), &input, &Pal::now());
            };
            let out = pass(&ctx, size, Vec::new(), &mut run);
            let colors: Vec<Color32> = out
                .shapes
                .iter()
                .find_map(|c| match &c.shape {
                    Shape::Text(t) if t.galley.text() == "Car music" => Some(t.galley.job.sections.iter().map(|s| s.format.color).collect()),
                    _ => None,
                })
                .unwrap_or_default();
            assert!(!colors.is_empty() && colors.iter().all(|c| *c == ink), "focused {focused}, lit {lit}: {colors:?}");
        }
    }

    #[test]
    fn an_empty_table_says_so_on_its_first_rows_line_and_takes_no_more_room() {
        let ctx = context();
        let size = vec2(600.0, 400.0);
        let cols = [Col::left("Name", 0.0)];
        let after = Cell::new(0.0);
        let out = pass(&ctx, size, Vec::new(), &mut |ui| {
            let top = ui.cursor().min.y;
            let _ = Table::new(Id::new("probe-empty"), &cols, 0).empty("(nothing added yet)").show(ui, &Pal::now(), |_, _, _| {});
            after.set(ui.cursor().min.y - top);
        });
        let words = texts(&out);
        let said = words.iter().find(|(t, _)| t == "(nothing added yet)").expect("the words").1;
        assert!((said.center().y - (TABLE_HEAD + 20.0)).abs() < 1.0, "on the first row's line: {said:?}");
        assert!((after.get() - (TABLE_HEAD + 40.0)).abs() < 0.5, "the headings and one row: {}", after.get());
    }

    #[test]
    fn a_fresh_listing_starts_at_its_top_and_a_revealed_row_comes_in_on_its_pass() {
        let ctx = context();
        let size = vec2(400.0, 400.0);
        let key = Cell::new(1u32);
        let cursor = Cell::new(Some(25usize));
        let reveal_row = Cell::new(None);
        let rows = std::cell::RefCell::new(Vec::new());
        let mut run = |ui: &mut Ui| {
            rows.borrow_mut().clear();
            let t = Table::new(Id::new("probe-list"), &[], 40)
                .headless()
                .row_height(30.0)
                .height(150.0)
                .cursor(cursor.get())
                .reveal(reveal_row.take())
                .content(Id::new(key.get()));
            let _ = t.show(ui, &Pal::now(), |_, i, row| {
                // Whole in the view: what a row shows.
                if row.rect.min.y >= -0.5 && row.rect.max.y <= 150.5 {
                    rows.borrow_mut().push(i);
                }
            });
        };
        let drawn = |i: usize| rows.borrow().contains(&i);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!(drawn(25) && !drawn(0), "the cursor brought in on the first pass");
        // Another listing, its cursor on its first row: at its top.
        key.set(2);
        cursor.set(Some(0));
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!(drawn(0) && !drawn(25), "a new listing at its top");
        // A row revealed, the cursor left where it is.
        reveal_row.set(Some(30));
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!(drawn(30), "revealed on the pass it is asked for");
    }

    #[test]
    fn a_click_in_a_rows_trail_is_no_rows_and_shows_no_hand() {
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let cols = [Col::left("User", 0.0)];
        let picked = Cell::new(None);
        let mut run = |ui: &mut Ui| {
            let shown = Table::new(Id::new("probe-trail"), &cols, 3).trail(50.0).show(ui, &Pal::now(), |_, _, _| {});
            if shown.row.is_some() {
                picked.set(shown.row);
            }
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let in_trail = pos2(size.x - 20.0, 34.0 + 20.0);
        let mut last = None;
        for events in click_events(in_trail) {
            last = Some(pass(&ctx, size, events, &mut run));
        }
        assert_eq!(picked.get(), None, "the trail selects nothing");
        assert_ne!(last.map(|o| o.platform_output.cursor_icon), Some(CursorIcon::PointingHand), "and shows no hand");
        for events in click_events(pos2(40.0, 34.0 + 20.0)) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(picked.get(), Some(0), "the selection area does");
    }

    #[test]
    fn a_state_lines_cadence_stands_only_where_the_whole_line_fits_beside_it() {
        let ctx = context();
        let pal = Pal::now();
        let parts = [("running".to_string(), strong(14.0), pal.ok), (" — music → /srv/backup".to_string(), body(14.0), pal.text)];
        let polls = cadence("polls every 2 s", &pal);
        let figure: Vec<(&str, bool)> = polls.iter().map(|(s, f, _)| (s.as_str(), *f == mono(12.0))).collect();
        assert_eq!(figure, [("polls every ", false), ("2", true), (" s", false)], "the figure alone in the monospace, the unit's space the words'");
        assert_eq!(figure_in("reloads every 30 s"), "30");
        assert_eq!(figure_in("every 1,5 s."), "1,5", "a decimal comma kept, the sentence's point not");
        assert_eq!(figure_in("at 12:30, daily"), "12:30");
        assert_eq!(figure_in("never"), "");
        let (wide, narrow) = (Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 20.0)), Rect::from_min_size(pos2(0.0, 40.0), vec2(220.0, 20.0)));
        let out = pass(&ctx, vec2(800.0, 200.0), Vec::new(), &mut |ui| {
            let _ = state_line(ui, wide, pal.ok, &parts, &polls);
            let _ = state_line(ui, narrow, pal.ok, &parts, &polls);
        });
        let drawn = texts(&out);
        let right: Vec<Rect> = drawn.iter().filter(|(t, _)| t == "polls every 2 s").map(|(_, r)| *r).collect();
        assert_eq!(right.len(), 1, "on the wide row only: {drawn:?}");
        assert!((right[0].max.x - wide.max.x).abs() < 1.0 && wide.contains(right[0].center()), "at the wide row's end: {right:?}");
        let words: Vec<Rect> = drawn.iter().filter(|(t, _)| t.starts_with("running — music")).map(|(_, r)| *r).collect();
        assert_eq!(words.len(), 2, "the state in one run on each row: {drawn:?}");
        assert!(words.iter().all(|r| (r.min.x - 16.0).abs() < 0.5), "from the indent: {words:?}");
        assert!(words[1].max.x <= narrow.max.x + 0.5, "and cut to the narrow row: {words:?}");
    }

    #[test]
    fn a_rooms_foot_tells_its_story_only_with_nothing_busy_and_no_note() {
        let ctx = context();
        let pal = Pal::now();
        let story = [("music → /srv/backup".to_string(), body(13.0), pal.text)];
        let row = |y: f32| Rect::from_min_size(pos2(0.0, y), vec2(500.0, NOTE_H));
        let note = ("could not reach the server".to_string(), true);
        let out = pass(&ctx, vec2(600.0, 200.0), Vec::new(), &mut |ui| {
            foot_line(ui, row(0.0), None, None, &story, &pal);
            foot_line(ui, row(40.0), Some("saving…"), Some(&note), &story, &pal);
            foot_line(ui, row(80.0), None, Some(&note), &story, &pal);
        });
        let drawn = texts(&out);
        // The rows each line stands on, told by its middle, which sits within
        // a point and a half of its row's: the system's UI face stands a run a
        // point higher or lower on one platform than on another (San Francisco
        // on a Mac, Segoe on Windows), and which row says what is the point.
        let rows = |s: &str| {
            drawn.iter().filter(|(t, _)| t == s).map(|(_, r)| r.center().y)
                .map(|y| {
                    let top = (y / 40.0).floor() * 40.0;
                    assert!((y - (top + NOTE_H / 2.0)).abs() <= 1.5, "{s:?} centred in its row: {drawn:?}");
                    top
                })
                .collect::<Vec<f32>>()
        };
        assert_eq!(rows(&story[0].0), [0.0], "the story on the first row alone: {drawn:?}");
        assert_eq!(rows("saving…"), [40.0], "busy wins over the note");
        assert_eq!(rows(&note.0), [80.0], "the note over the story");
    }

    #[test]
    fn a_short_fields_note_stands_beside_its_box_where_it_fits_and_under_it_where_not() {
        let ctx = context();
        let note = "a-z, 0-9 and dashes";
        let boxes = std::cell::RefCell::new(Vec::new());
        let out = pass(&ctx, vec2(800.0, 400.0), Vec::new(), &mut |ui| {
            let pal = Pal::now();
            let input = Input::new("field-recordings", 0);
            for (k, (top, w)) in [(0.0, 600.0), (200.0, 300.0)].into_iter().enumerate() {
                let column = Rect::from_min_size(pos2(20.0, top), vec2(w, 150.0));
                let mut child = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
                let typed = field_beside(&mut child, Id::new("probe-beside").with(k), "Name", &input, 264.0, note, &pal);
                boxes.borrow_mut().push(typed.response.rect);
            }
        });
        let boxes = boxes.into_inner();
        let notes: Vec<Rect> = texts(&out).into_iter().filter(|(t, _)| t == note).map(|(_, r)| r).collect();
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert_eq!(boxes[0].width(), 264.0, "the box its own width, not the column's");
        assert!((notes[0].min.x - (boxes[0].max.x + 16.0)).abs() < 0.5, "16 points beside the box: {notes:?} {boxes:?}");
        assert!((notes[0].center().y - boxes[0].center().y).abs() < 1.0, "on its line");
        assert!(notes[1].min.y >= boxes[1].max.y && (notes[1].min.x - boxes[1].min.x).abs() < 0.5, "under it where it does not fit");
    }

    #[test]
    fn a_cells_tip_is_registered_only_where_its_words_were_cut() {
        let ctx = context();
        let cols = [Col::left("Name", 60.0), Col::left("Path", 0.0)];
        let _ = pass(&ctx, vec2(600.0, 300.0), Vec::new(), &mut |ui| {
            let _ = Table::new(Id::new("probe-tips"), &cols, 1).show(ui, &Pal::now(), |ui, _, row| {
                let _ = cell_tipped(ui, row, 0, "a long name that runs on", "whole", body(14.0), Color32::WHITE, Id::new("probe-cut"));
                let _ = cell_tipped(ui, row, 1, "short", "whole", body(14.0), Color32::WHITE, Id::new("probe-whole"));
            });
        });
        assert!(ctx.read_response(Id::new("probe-cut")).is_some(), "a tip over the cut cell");
        assert!(ctx.read_response(Id::new("probe-whole")).is_none(), "none over the whole one");
    }

    #[test]
    fn a_groups_ring_reaches_past_the_column_round_what_was_drawn_since_its_top() {
        let ctx = context();
        let got = Cell::new(Rect::NOTHING);
        let _ = pass(&ctx, vec2(600.0, 400.0), Vec::new(), &mut |ui| {
            let column = Rect::from_min_max(pos2(100.0, 50.0), pos2(400.0, 300.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
            let top = child.cursor().min.y;
            child.add_space(40.0);
            got.set(ring_since(&child, top, &Pal::now()));
        });
        assert_eq!(got.get(), Rect::from_min_max(pos2(92.0, 48.0), pos2(408.0, 92.0)));
    }

    #[test]
    fn a_ticket_broken_anywhere_fills_its_lines_where_its_marks_would_end_them_short() {
        let ctx = context();
        let ticket = format!("mstrfed1:{}", "eyJ0-Ijoi_YWJj".repeat(12));
        let glyph = Cell::new(0.0);
        let out = pass(&ctx, vec2(800.0, 400.0), Vec::new(), &mut |ui| {
            glyph.set(width_of(ui, "m", mono(12.5)));
            let _ = Para::new(&ticket, mono(12.5), Color32::WHITE).anywhere().paint(ui, pos2(10.0, 10.0), 200.0);
            let _ = Para::new(&ticket, mono(12.5), Color32::WHITE).paint(ui, pos2(400.0, 10.0), 200.0);
        });
        let galleys: Vec<(Pos2, std::sync::Arc<egui::Galley>)> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == ticket => Some((t.pos, t.galley.clone())),
                _ => None,
            })
            .collect();
        // The narrowest of the lines a wrap ended, the last left out.
        let shortest = |left: bool| {
            let galley = &galleys.iter().find(|(at, _)| (at.x < 200.0) == left).expect("both drawn").1;
            let ended = &galley.rows[..galley.rows.len() - 1];
            ended.iter().map(|r| r.rect().width()).fold(f32::INFINITY, f32::min)
        };
        let glyph = glyph.get();
        assert!(shortest(true) > 200.0 - 2.0 * glyph, "every line filled: {}", shortest(true));
        assert!(shortest(false) < 200.0 - 4.0 * glyph, "a dash ends the default's lines short: {}", shortest(false));
    }

    #[test]
    fn a_verdict_hangs_its_words_beside_its_mark_in_two_lines_at_most() {
        let ctx = context();
        let pal = Pal::now();
        let long = "connection failed: ".to_string() + &"the daemon refused the request ".repeat(12);
        let out = pass(&ctx, vec2(800.0, 300.0), Vec::new(), &mut |ui| {
            let _ = verdict(ui, pos2(10.0, 10.0), 300.0, "reachable — Transmission 4.0.6", &["4.0.6"], pal.ok, Some(true));
            let _ = verdict(ui, pos2(10.0, 100.0), 300.0, &long, &[], pal.gold, Some(false));
            let _ = verdict(ui, pos2(400.0, 10.0), 300.0, "checking the path…", &[], pal.dim, None);
        });
        let drawn = texts(&out);
        let at = |s: &str| drawn.iter().find(|(t, _)| t.starts_with(s)).unwrap_or_else(|| panic!("{s} drawn: {drawn:?}")).1;
        assert!((at("reachable").min.x - 30.0).abs() < 0.5, "the words 20 points in, beside the mark");
        assert!((at("checking").min.x - 400.0).abs() < 0.5, "no mark, no indent");
        assert!(at("connection failed").height() < 3.0 * 13.5 * 1.3 - 1.0, "two lines at most");
        // What the pass drew that is no text, by where it stands.
        let marks_in = |x0: f32, x1: f32| {
            out.shapes
                .iter()
                .filter(|c| !matches!(c.shape, Shape::Text(_)))
                .map(|c| c.shape.visual_bounding_rect())
                .filter(|r| r.is_positive() && r.min.x >= x0 && r.max.x <= x1)
                .count()
        };
        assert_eq!(marks_in(5.0, 25.0), 2, "a check and a dash, each before its words");
        assert_eq!(marks_in(395.0, 415.0), 0, "and none for the third");
    }

    #[test]
    fn a_folder_browser_leaves_its_foot_room_and_says_when_a_folder_is_empty() {
        let ctx = context();
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let buttons = [("up", false), ("open", false), ("Add this folder", true), ("close", false)];
        let ended = Cell::new(0.0);
        let empty_at = Cell::new(Rect::NOTHING);
        let out = pass(&ctx, vec2(800.0, 600.0), Vec::new(), &mut |ui| {
            let pal = Pal::now();
            // A short modal body: the list gives way to the foot under it.
            let short = Rect::from_min_size(pos2(20.0, 20.0), vec2(400.0, 260.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(short).layout(Layout::top_down(Align::Min)));
            let _ = folder_browser(&mut child, Id::new("probe-browse"), "/srv", &names, 0, "(no folders in here)", BROWSE_ROWS, &buttons, &pal);
            ended.set(child.cursor().min.y);
            let other = Rect::from_min_size(pos2(440.0, 20.0), vec2(300.0, 260.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(other).layout(Layout::top_down(Align::Min)));
            let top = child.cursor().min.y;
            let _ = folder_browser(&mut child, Id::new("probe-empty"), "/srv/empty", &[], 0, "(no folders in here)", BROWSE_ROWS, &buttons, &pal);
            empty_at.set(Rect::from_min_max(pos2(other.min.x, top), child.cursor().min));
        });
        assert!(ended.get() <= 20.0 + 260.0 + 0.5, "the foot whole under the list: {}", ended.get());
        let foot = ctx.read_response(Id::new("probe-browse").with("foot").with(3usize)).expect("the foot's last").rect;
        assert!(foot.max.y <= 20.0 + 260.0 + 0.5, "{foot:?}");
        let said = texts(&out).into_iter().find(|(t, _)| t == "(no folders in here)").expect("the empty words").1;
        assert!((said.min.x - (440.0 + CELL_PAD)).abs() < 0.5, "on the first row's line, as a table's: {said:?}");
        let first_row = empty_at.get().min.y + 18.0 + 12.0;
        assert!((said.center().y - (first_row + FOLDER_ROW_H / 2.0)).abs() < 1.0, "{said:?} {:?}", empty_at.get());
    }

    /// Every glyph a pass drew, by the text it belongs to: its letter and
    /// its baseline on the screen.
    fn baselines(out: &egui::FullOutput) -> Vec<(String, Vec<(char, f32)>)> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(t) => {
                    let glyphs = t.galley.rows.iter().flat_map(|row| row.row.glyphs.iter().map(move |g| (g.chr, t.pos.y + row.pos.y + g.pos.y)));
                    Some((t.galley.text().to_string(), glyphs.collect()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn figures_stand_on_their_words_baseline_and_an_led_on_their_small_letters() {
        let ctx = context();
        let pal = Pal::now();
        let mut leds = Vec::new();
        let out = pass(&ctx, vec2(1200.0, 400.0), Vec::new(), &mut |ui| {
            // A figure in its sentence, wrapped as a verdict wraps it.
            let parts = with_figures("reachable — Transmission 4.0.6 · RPC 17", &["4.0.6", "17"], body(13.5), mono(12.5), pal.ok);
            let _ = Para::runs(&parts).rows(2).paint(ui, pos2(10.0, 20.0), 900.0);
            // And in one line of runs, centred on its row.
            let _ = runs(ui, pos2(10.0, 100.0), &with_figures("polls every 30 s", &["30"], body(13.0), mono(12.0), pal.dim), 600.0, Align2::LEFT_CENTER);
            // Two lines centred on one point, apart, at a size that shows
            // a face's own centring.
            let _ = text(ui, pos2(10.0, 200.0), "words", body(40.0), pal.text, 400.0, Align2::LEFT_CENTER);
            let _ = text(ui, pos2(500.0, 200.0), "4096", mono(40.0), pal.text, 400.0, Align2::LEFT_CENTER);
            leds = vec![led_mid(ui, 200.0, &body(40.0), false), led_mid(ui, 200.0, &strong(40.0), true)];
        });
        let lines = baselines(&out);
        let all = |s: &str| lines.iter().find(|(t, _)| t == s).unwrap_or_else(|| panic!("{s:?} drawn: {lines:?}")).1.clone();
        let spread = |glyphs: &[(char, f32)]| {
            let ys = glyphs.iter().filter(|(c, _)| c.is_alphanumeric()).map(|(_, y)| *y);
            ys.clone().fold(f32::MIN, f32::max) - ys.fold(f32::MAX, f32::min)
        };
        let verdict = all("reachable — Transmission 4.0.6 · RPC 17");
        assert!(spread(&verdict) <= 1.0, "the version and the number on the words' baseline: {verdict:?}");
        let cadence = all("polls every 30 s");
        assert!(spread(&cadence) <= 1.0, "{cadence:?}");
        let (words, figure) = (all("words"), all("4096"));
        let (b_words, b_figure) = (words[0].1, figure[0].1);
        assert!((b_words - b_figure).abs() <= 1.0, "a monospace line centred beside words sits on their baseline: {b_words} {b_figure}");
        // The LED: on the middle of the small letters' ink, as "words" drew
        // them ("w", "o", "r" and "s"; the "d" reaches higher), within a
        // point, the band's edges and the baseline each landing on a pixel —
        // not on the line's middle, which the room a face leaves for accents
        // over them puts several points higher in Segoe UI, and the room
        // Calibri leaves under them 4 lower; and higher on capitals than on
        // small letters.
        let small = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "words" => {
                    let row = &t.galley.rows[0];
                    let ink = row.row.glyphs.iter().filter(|g| g.chr != 'd').map(|g| {
                        let top = t.pos.y + row.pos.y + g.pos.y + g.uv_rect.offset.y;
                        (top, top + g.uv_rect.size.y)
                    });
                    Some(ink.fold((f32::MAX, f32::MIN), |(a, b), (top, foot)| (a.min(top), b.max(foot))))
                }
                _ => None,
            })
            .expect("\"words\" drawn");
        let middle = (small.0 + small.1) / 2.0;
        assert!((leds[0] - middle).abs() <= 1.0, "on the small letters' middle: {leds:?}, their ink {small:?}, baseline {b_words}");
        assert!(leds[1] < leds[0], "a capital's middle is higher: {leds:?}");
    }

    #[test]
    fn a_table_ends_on_whole_rows_and_leaves_its_thumb_room_while_it_overflows() {
        assert_eq!(whole_rows(250.0, 40.0), 240.0);
        assert_eq!(whole_rows(240.0, 40.0), 240.0, "a view of whole rows keeps them");
        assert_eq!(whole_rows(30.0, 40.0), 30.0, "less than a row stays what it is");

        let ctx = context();
        let size = vec2(600.0, 30.0 + 4.0 + 250.0);
        let cols = [Col::left("Name", 0.0)];
        let drawn: std::cell::RefCell<Vec<(usize, Rect)>> = Default::default();
        let clip = Cell::new(Rect::NOTHING);
        let run_with = |len: usize| {
            drawn.borrow_mut().clear();
            let out = pass(&ctx, size, Vec::new(), &mut |ui| {
                let _ = Table::new(Id::new("probe-whole").with(len), &cols, len).cursor(Some(0)).show(ui, &Pal::now(), |ui, i, row| {
                    drawn.borrow_mut().push((i, row.rect));
                    clip.set(ui.clip_rect());
                });
            });
            (out, drawn.borrow().clone())
        };
        // Twenty rows in 250 points: a view of six whole rows, none cut at
        // its foot, every row BAR_ROOM short of the edge, and the thumb
        // there.
        // (egui brings a bar in over a few passes.)
        for _ in 0..8 {
            let _ = run_with(20);
        }
        let (out, rows) = run_with(20);
        let view_foot = 34.0 + 240.0;
        assert!((clip.get().max.y - view_foot).abs() < 0.5, "the view ends with the sixth row: {:?}", clip.get());
        let seen: Vec<Rect> = rows.iter().map(|(_, r)| *r).filter(|r| r.intersects(clip.get().shrink(0.5))).collect();
        assert!(seen.iter().all(|r| clip.get().contains_rect(*r)), "whole rows only: {rows:?}");
        assert_eq!(seen.len(), 6, "{rows:?}");
        assert!(rows.iter().all(|(_, r)| (r.max.x - (size.x - BAR_ROOM)).abs() < 0.5), "room for the thumb: {rows:?}");
        let ring = rows[0].1;
        let thumbs: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if (r.rect.width() - THUMB_W).abs() < 0.5 && r.rect.max.x > size.x - 1.0 && r.fill.a() > 0 => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(thumbs.len(), 1, "the thumb at the edge: {thumbs:?}");
        assert!(!thumbs[0].intersects(ring.shrink(0.5)), "clear of the cursor's ring: {:?} {ring:?}", thumbs[0]);
        assert!(thumbs[0].max.y <= view_foot + 0.5, "in the view: {:?}", thumbs[0]);
        // Three rows fit: the whole width, and no thumb.
        let (_, rows) = run_with(3);
        assert!(rows.iter().all(|(_, r)| (r.max.x - size.x).abs() < 0.5), "{rows:?}");
    }

    #[test]
    fn a_tick_box_lights_its_own_edge_under_the_pointer_wherever_it_stands() {
        let ctx = context();
        let pal = Pal::now();
        let size = vec2(600.0, 300.0);
        let cols = [Col::left("User", 0.0), Col::center("Admin", 60.0)];
        let tick_at = Cell::new(Pos2::ZERO);
        let card_at = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            Table::new(Id::new("probe-ticks"), &cols, 2).height(80.0).show(ui, &pal, |ui, i, row| {
                let at = row.cell(1).center();
                let _ = tick_mark(ui, at, false, pal.ok, &pal);
                if i == 0 {
                    tick_at.set(at);
                }
            });
            ui.add_space(20.0);
            let top = ui.cursor().min.y;
            let _ = tick_card(ui, Id::new("probe-card"), true, "Accept requests", "anyone may ask", false, &pal);
            card_at.set(Rect::from_min_size(pos2(ui.max_rect().min.x, top), vec2(ui.max_rect().width(), 60.0)));
        };
        // The edge each tick box was drawn with, by its centre.
        let edges = |out: &egui::FullOutput| -> Vec<(Pos2, Color32)> {
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Rect(r) if (r.rect.width() - BOX).abs() < 0.5 && r.stroke.width > 0.0 => Some((r.rect.center(), r.stroke.color)),
                    _ => None,
                })
                .collect()
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let box_at = tick_at.get();
        let edge_of = |out: &egui::FullOutput, at: Pos2| edges(out).into_iter().find(|(c, _)| c.distance(at) < 1.0).map(|(_, e)| e);
        // On the box: its own edge lights, though the row takes the click.
        let mut out = None;
        for _ in 0..2 {
            out = Some(pass(&ctx, size, vec![egui::Event::PointerMoved(box_at + vec2(5.0, 5.0))], &mut run));
        }
        let out = out.expect("a pass");
        assert_eq!(edge_of(&out, box_at), Some(pal.text), "the box under the pointer");
        // On the row's words: the row is raised, the box stays at rest.
        let out = pass(&ctx, size, vec![egui::Event::PointerMoved(pos2(40.0, box_at.y))], &mut run);
        assert_eq!(edge_of(&out, box_at), Some(pal.dim), "the box at rest beside the pointer");
        // A tick card: the box lights with the card the click is.
        let card = card_at.get();
        let mark = pos2(card.min.x + 24.0, card.min.y + 20.0);
        let rest = pass(&ctx, size, vec![egui::Event::PointerMoved(pos2(5.0, 5.0))], &mut run);
        assert_eq!(edge_of(&rest, mark), Some(pal.ok));
        let lit = pass(&ctx, size, vec![egui::Event::PointerMoved(card.center())], &mut run);
        assert_eq!(edge_of(&lit, mark), Some(lift(pal.ok, 70)), "the box's own edge, not only the card's");
    }

    #[test]
    fn ticks_wrap_and_narrow_their_columns_before_they_cut_a_name() {
        // The federate form's libraries at its 512 points: the long name's
        // column as wide as it wants, the others sharing what is left.
        let wants = [87.0, 117.0, 181.0, 80.0, 99.0, 70.0];
        let cols = grid_columns(&wants, 512.0, 8.0, 3);
        assert_eq!(cols.len(), 3);
        assert!(cols[2] >= 181.0 && cols[0] >= 87.0 && cols[1] >= 117.0, "no name cut: {cols:?}");
        assert!((cols.iter().sum::<f32>() + 16.0 - 512.0).abs() < 0.01, "the grid spans the column: {cols:?}");
        assert_eq!(grid_columns(&[60.0, 70.0, 80.0, 90.0], 512.0, 8.0, 3), vec![(512.0 - 16.0) / 3.0; 3], "short names: even columns");
        assert_eq!(grid_columns(&[300.0, 300.0, 300.0], 512.0, 8.0, 3).len(), 1, "a column fewer, then one");
        assert_eq!(grid_columns(&[700.0], 512.0, 8.0, 3), vec![512.0], "one too long for any column is cut");

        // The add form's six libraries in a line too short for them: two
        // lines, every one placed, none past the line's end.
        let wants = [110.0, 120.0, 180.0, 95.0, 105.0, 90.0];
        let placed = flow(&wants, 608.0, 4.0);
        assert_eq!(placed.len(), 6, "every library has its tick");
        assert_eq!(placed.iter().map(|(line, ..)| *line).max(), Some(1), "wrapped once: {placed:?}");
        assert!(placed.iter().all(|(_, x, w)| x + w <= 608.0 + 0.01), "{placed:?}");
        assert_eq!(placed[4], (1, 0.0, 105.0), "the next line starts at the left");
        assert_eq!(flow(&[700.0, 50.0], 608.0, 4.0), vec![(0, 0.0, 608.0), (1, 0.0, 50.0)], "one wider than a line is cut to it");

        // Drawn: the long name whole, beside the others.
        let ctx = context();
        let names = ["archive", "audiobooks", "field-recordings-2024", "music", "podcasts", "vinyl"];
        let items: Vec<(String, bool)> = names.iter().map(|n| (n.to_string(), true)).collect();
        let out = pass(&ctx, vec2(600.0, 300.0), Vec::new(), &mut |ui| {
            let column = Rect::from_min_size(pos2(20.0, 20.0), vec2(512.0, 200.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
            let _ = tick_grid(&mut child, Id::new("probe-grid"), &items, Some(2), 3, TICK_H, &Pal::now());
        });
        let drawn: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(names.iter().all(|n| drawn.iter().any(|t| t == n)), "every name whole, the focused one in its semibold too: {drawn:?}");
    }

    #[test]
    fn a_cut_cells_tooltip_hangs_under_it_clear_of_the_next_rows_controls() {
        let ctx = context();
        let size = vec2(900.0, 400.0);
        // The folder column narrower than its path's tooltip, as Libraries'
        // is: a tooltip left-aligned under it would reach the box beyond.
        let cols = [Col::left("Name", 0.0), Col::left("Folder", 300.0), Col::center("Symlinks", 80.0)];
        let path = "/Volumes/NAS/Archive/Lossless Rips/2019 Remasters/Box Sets/and a good deal more besides";
        let boxes: std::cell::RefCell<Vec<Rect>> = Default::default();
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            boxes.borrow_mut().clear();
            Table::new(Id::new("probe-tips"), &cols, 3).inert().show(ui, &pal, |ui, i, row| {
                let _ = path_cell(ui, row, 1, path, mono(12.5), pal.soft, Id::new("probe-tip").with(i));
                let at = row.cell(2).center();
                let _ = checkbox(ui, Id::new("probe-box").with(i), at, false, &pal);
                boxes.borrow_mut().push(Rect::from_center_size(at, Vec2::splat(HIT)));
            });
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        // The pointer on the first row's folder, near the cell's right.
        let over = pos2(size.x - CELL_PAD - 80.0 - COL_GAP - 30.0, 34.0 + 20.0);
        let mut out = None;
        for _ in 0..3 {
            out = Some(pass(&ctx, size, vec![egui::Event::PointerMoved(over)], &mut run));
        }
        let out = out.expect("a pass");
        let tip = texts(&out).into_iter().find(|(t, _)| t == path).expect("the whole path in its tooltip").1;
        let next = boxes.borrow()[1];
        assert!(!tip.intersects(next), "the tooltip over the next row's box: {tip:?} {next:?}");
        assert!(tip.min.y > 34.0 + 40.0 - 0.5, "under the row: {tip:?}");
    }

    #[test]
    fn every_cards_glyph_stands_in_its_box() {
        let ctx = context();
        let centre = pos2(100.0, 100.0);
        for glyph in [Glyph::Folder, Glyph::User, Glyph::Tray, Glyph::Ticket, Glyph::File] {
            let out = pass(&ctx, vec2(200.0, 200.0), Vec::new(), &mut |ui| draw_glyph(ui, centre, glyph, Color32::WHITE));
            let bounds = out.shapes.iter().map(|c| c.shape.visual_bounding_rect()).fold(Rect::NOTHING, |a, b| a.union(b));
            assert!(bounds.is_positive(), "{glyph:?} draws");
            // Its 16 points and half its stroke either side.
            assert!(Rect::from_center_size(centre, Vec2::splat(16.0 + 2.0)).contains_rect(bounds), "{glyph:?} in its 16 points: {bounds:?}");
        }
    }

    #[test]
    fn a_scrim_over_the_ground_is_what_the_capture_measured() {
        assert_eq!(under_scrim(Color32::from_rgb(0x12, 0x13, 0x1c)), Color32::from_rgb(13, 13, 20));
        assert_eq!(under_scrim(Color32::BLACK), Color32::from_rgb(SCRIM.r(), SCRIM.g(), SCRIM.b()), "black shows the scrim alone");
    }

    // ── Pictures, steps and a button at rest ──────────────────────────────

    /// One pass that says what it uploaded and freed, which the pass
    /// helper lets go of unread: the uploads of an image `size` big, and
    /// how many textures it freed. The rest is let go of as there.
    fn pass_keeping(ctx: &egui::Context, size: [usize; 2], run: &mut dyn FnMut(&mut Ui)) -> (Vec<egui::epaint::ImageDelta>, usize) {
        style_once(ctx, &Pal::now());
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| run(ui));
        let set = out.textures_delta.set.values().flatten().filter(|d| d.image.size() == size).cloned().collect();
        let freed = out.textures_delta.free.len();
        out.textures_delta.clear();
        (set, freed)
    }

    #[test]
    fn a_picture_is_uploaded_once_and_let_go_when_it_changes() {
        let ctx = context();
        let made = Cell::new(0);
        let key = Cell::new(1u64);
        let id = Id::new("probe-picture");
        let size = [8, 2];
        let mut run = |ui: &mut Ui| {
            let handle = texture(ui.ctx(), id, key.get(), true, || {
                made.set(made.get() + 1);
                Some(egui::ColorImage::filled(size, Color32::RED))
            });
            assert!(handle.is_some(), "the picture is there");
        };
        // Uploaded on its first pass, by nearest neighbour as a crisp one;
        // never on the passes after it.
        let (first, _) = pass_keeping(&ctx, size, &mut run);
        assert_eq!(first.len(), 1, "uploaded on its first pass");
        assert_eq!(first[0].options, egui::TextureOptions::NEAREST, "a crisp one by nearest neighbour");
        for _ in 0..3 {
            let (later, _) = pass_keeping(&ctx, size, &mut run);
            assert!(later.is_empty(), "never again");
        }
        assert_eq!(made.get(), 1, "made once");
        // Another picture under the same id: made, and the old one let go.
        key.set(2);
        let (changed, freed_then) = pass_keeping(&ctx, size, &mut run);
        assert_eq!((made.get(), changed.len()), (2, 1), "the new one made and uploaded");
        let (_, freed_after) = pass_keeping(&ctx, size, &mut run);
        assert_eq!(freed_then + freed_after, 1, "and the old one freed");
        // One that cannot be made is not asked for again.
        let tries = Cell::new(0);
        for _ in 0..3 {
            let _ = pass_keeping(&ctx, size, &mut |ui| {
                let none = texture(ui.ctx(), Id::new("probe-none"), 7, false, || {
                    tries.set(tries.get() + 1);
                    None
                });
                assert!(none.is_none());
            });
        }
        assert_eq!(tries.get(), 1, "a picture that cannot be made is asked for once");
    }

    #[test]
    fn a_picture_is_drawn_whole_and_centred_in_its_rect() {
        let ctx = context();
        let drawn = Cell::new(Rect::NOTHING);
        let tall = Cell::new(Rect::NOTHING);
        let _ = pass_keeping(&ctx, [400, 100], &mut |ui| {
            let wide = texture(ui.ctx(), Id::new("probe-wide"), 1, false, || Some(egui::ColorImage::filled([400, 100], Color32::WHITE)));
            let wide = wide.expect("made");
            drawn.set(picture(ui, Rect::from_min_size(pos2(10.0, 10.0), vec2(200.0, 200.0)), &wide));
            tall.set(picture(ui, Rect::from_min_size(pos2(10.0, 10.0), vec2(800.0, 50.0)), &wide));
        });
        assert_eq!(drawn.get(), Rect::from_min_size(pos2(10.0, 85.0), vec2(200.0, 50.0)), "the width it has, its shape kept, centred");
        assert_eq!(tall.get(), Rect::from_min_size(pos2(310.0, 10.0), vec2(200.0, 50.0)), "the height it has, centred across");
    }

    #[test]
    fn a_step_line_lights_the_steps_done_the_one_at_and_none_after() {
        let ctx = context();
        let pal = Pal::now();
        let at = Cell::new(Rect::NOTHING);
        let out = pass(&ctx, vec2(400.0, 100.0), Vec::new(), &mut |ui| at.set(step_leds(ui, 300.0, 50.0, 2, 4, &pal)));
        let leds: Vec<(f32, Color32)> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                Shape::Rect(r) if (r.rect.width() - 6.0).abs() < 0.01 && (r.rect.height() - 6.0).abs() < 0.01 => Some((r.rect.center().x, r.fill)),
                _ => None,
            })
            .collect();
        assert_eq!(leds.iter().map(|(_, c)| *c).collect::<Vec<_>>(), [pal.ok, pal.accent, pal.unlit, pal.unlit]);
        assert!(leds.windows(2).all(|w| (w[1].0 - w[0].0 - 10.0).abs() < 0.01), "6 points and 4 apart: {leds:?}");
        assert_eq!(at.get(), Rect::from_min_max(pos2(300.0 - 36.0, 47.0), pos2(300.0, 53.0)), "ending at its right");
    }

    #[test]
    fn a_button_at_rest_takes_no_click_and_shows_no_hand_but_is_hovered_for_its_tip() {
        let ctx = context();
        let size = vec2(400.0, 200.0);
        let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(140.0, FORM_BUTTON_H));
        let id = Id::new("probe-off");
        let (clicked, hovered) = (Cell::new(false), Cell::new(false));
        let mut run = |ui: &mut Ui| {
            let response = form_button_off(ui, id, rect, "Continue ▸", &Pal::now());
            clicked.set(clicked.get() || response.clicked());
            hovered.set(response.hovered());
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let words = texts(&out);
        let label = words.iter().find(|(t, _)| t == "CONTINUE").expect("the bare capitals");
        let color = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == "CONTINUE" => t.galley.job.sections.first().map(|s| s.format.color),
            _ => None,
        });
        assert!(rect.contains(label.1.center()), "{label:?}");
        assert_eq!(color, Some(Pal::now().dim), "dim");
        let mut last = None;
        for events in click_events(rect.center()) {
            last = Some(pass(&ctx, size, events, &mut run));
        }
        assert!(!clicked.get(), "no click");
        assert!(hovered.get(), "hovered, for the tip");
        assert_ne!(last.map(|o| o.platform_output.cursor_icon), Some(CursorIcon::PointingHand), "no hand");
    }

    // ── Feet that stack ───────────────────────────────────────────────────

    /// Where a foot's rects stand, as (x, row, width) with the rows 44
    /// points apart: a button and the 10 under it.
    fn placed(laid: &[Rect]) -> Vec<(f32, usize, f32)> {
        laid.iter().map(|r| (r.min.x, (r.min.y / (FORM_BUTTON_H + FOOT_GAP)).round() as usize, r.width())).collect()
    }

    #[test]
    fn a_foot_keeps_one_row_where_it_fits_and_stacks_whole_where_it_does_not() {
        // All on one row: the start from the left, the end ending at the
        // right, as every board's page lays them.
        let (laid, h) = foot_layout(&[60.0, 70.0], &[80.0, 90.0], 500.0);
        assert_eq!(placed(&laid), [(0.0, 0, 60.0), (70.0, 0, 70.0), (320.0, 0, 80.0), (410.0, 0, 90.0)]);
        assert_eq!(h, FORM_BUTTON_H);
        // The browser's four from the left, in a window too narrow for
        // them: the last starts a row of its own from the left, whole.
        let (laid, h) = foot_layout(&[60.0, 80.0, 220.0, 100.0], &[], 420.0);
        assert_eq!(placed(&laid), [(0.0, 0, 60.0), (70.0, 0, 80.0), (160.0, 0, 220.0), (0.0, 1, 100.0)]);
        assert_eq!(h, 2.0 * FORM_BUTTON_H + FOOT_GAP);
        // A gate's two at the right: side by side where they fit; else the
        // one that goes through keeps the first row and the safe one comes
        // under it, both ending at the right edge.
        let (laid, _) = foot_layout(&[], &[150.0, 120.0], 400.0);
        assert_eq!(placed(&laid), [(120.0, 0, 150.0), (280.0, 0, 120.0)]);
        let (laid, h) = foot_layout(&[], &[250.0, 200.0], 400.0);
        assert_eq!(placed(&laid), [(150.0, 1, 250.0), (200.0, 0, 200.0)]);
        assert_eq!(h, 2.0 * FORM_BUTTON_H + FOOT_GAP);
        // The end clear of the start's last button, the gap between, or on
        // a row of its own.
        let (laid, _) = foot_layout(&[200.0], &[190.0], 400.0);
        assert_eq!(placed(&laid), [(0.0, 0, 200.0), (210.0, 0, 190.0)], "just fits");
        let (laid, _) = foot_layout(&[200.0], &[191.0], 400.0);
        assert_eq!(placed(&laid), [(0.0, 0, 200.0), (209.0, 1, 191.0)], "a point over");
        // A button wider than the foot takes a row, cut to it.
        let (laid, _) = foot_layout(&[], &[300.0, 40.0], 100.0);
        assert_eq!(placed(&laid), [(0.0, 1, 100.0), (60.0, 0, 40.0)]);
        assert_eq!(foot_layout(&[], &[], 300.0).1, FORM_BUTTON_H, "an empty foot keeps its row");
    }

    #[test]
    fn a_gate_in_a_narrow_room_stacks_its_answers_and_each_still_answers() {
        let ctx = context();
        let size = vec2(420.0, 500.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-narrow-gate");
        let (keep, go) = ("Keep everything where it is", "Remove it anyway");
        let answered = Cell::new(None);
        let window = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let shown = gate(ui, id, over, "Remove it?", vec![Para::new("It goes.", body(14.0), pal.gold)], [keep, go], &pal);
            window.set(shown.window);
            if let Some(went) = shown.inner {
                answered.set(Some(went));
            }
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let rect = |k: usize| ctx.read_response(id.with("foot").with(k)).unwrap_or_else(|| panic!("answer {k} drawn")).rect;
        let (safe, through) = (rect(0), rect(1));
        assert!((safe.max.x - through.max.x).abs() < 0.01 && (safe.min.y - through.max.y - FOOT_GAP).abs() < 0.01, "going through above: {safe:?} {through:?}");
        assert!(window.get().contains_rect(safe) && window.get().contains_rect(through), "both in the window: {:?}", window.get());
        let words: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        assert!(words.iter().any(|t| *t == keep.to_uppercase()) && words.iter().any(|t| *t == go.to_uppercase()), "whole: {words:?}");
        for events in click_events(through.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(answered.take(), Some(true), "going through goes through");
        for events in click_events(safe.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(answered.take(), Some(false), "the safe one keeps");
    }

    #[test]
    fn a_short_narrow_browser_gives_its_list_up_to_the_whole_stacked_foot() {
        let ctx = context();
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let buttons = [("up", false), ("open", false), ("Add this very folder here", true), ("close it", false)];
        let body = Rect::from_min_size(pos2(20.0, 20.0), vec2(300.0, 230.0));
        let ended = Cell::new(0.0);
        let _ = pass(&ctx, vec2(400.0, 400.0), Vec::new(), &mut |ui| {
            let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
            let _ = folder_browser(&mut child, Id::new("probe-stack"), "/srv", &names, 0, "", 9, &buttons, &Pal::now());
            ended.set(child.cursor().min.y);
        });
        let foot = |k: usize| ctx.read_response(Id::new("probe-stack").with("foot").with(k)).expect("drawn").rect;
        assert!(foot(3).min.y > foot(0).max.y, "stacked: {:?} {:?}", foot(0), foot(3));
        assert!(ended.get() <= body.max.y + 0.5, "the whole foot in the body: {}", ended.get());
        let row = ctx.read_response(Id::new("probe-stack").with("list").with("row").with(0usize)).expect("a row kept").rect;
        assert!(foot(0).min.y - row.max.y >= 20.0 - 0.5, "the list 20 over the foot at least: {row:?}");
    }

    #[test]
    fn a_browsers_gaps_close_up_only_as_far_as_its_foot_needs() {
        // A row of the list and a foot of three rows, 122 points.
        let rest = 30.0 + 122.0;
        // Room for the path and the rest with the 12 and the 20, or more,
        // or no end to it: as drawn.
        assert_eq!(browser_gaps(18.0 + 12.0 + 20.0 + rest, rest), (12.0, 20.0));
        assert_eq!(browser_gaps(300.0, 30.0 + 78.0), (12.0, 20.0));
        assert_eq!(browser_gaps(f32::INFINITY, 30.0 + 34.0), (12.0, 20.0));
        // Two points short (a modal at the cells' floor, 200 points of body):
        // a point from each.
        assert_eq!(browser_gaps(200.0, rest), (11.0, 19.0));
        // A sum's crumbs short take nothing; half a point takes a whole one.
        assert_eq!(browser_gaps(202.0 - 0.001, rest), (12.0, 20.0));
        assert_eq!(browser_gaps(199.5, rest), (11.0, 18.0));
        // Half the two: each halved.
        assert_eq!(browser_gaps(18.0 + 16.0 + 30.0 + 34.0, 30.0 + 34.0), (6.0, 10.0));
        // More than the two together: none, and the body scrolls.
        assert_eq!(browser_gaps(150.0, rest), (0.0, 0.0));
    }

    #[test]
    fn a_browser_a_little_short_for_its_foot_closes_its_gaps_and_keeps_its_row_and_the_whole_foot() {
        let ctx = context();
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let buttons = [("up", false), ("open", false), ("Add this very folder here", true), ("close it", false)];
        let (left, w) = (20.0, 300.0);
        let foot_h = Cell::new(0.0);
        let ended = Cell::new(0.0);
        // The body three points short of the path, a row and the foot with
        // the 12 and the 20, whatever the face makes of the foot.
        let mut run = |ui: &mut Ui| {
            foot_h.set(foot_rects(ui, &buttons, &[], w).1);
            let body = Rect::from_min_size(pos2(left, 20.0), vec2(w, 18.0 + 12.0 + 30.0 + 20.0 + foot_h.get() - 3.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
            let _ = folder_browser(&mut child, Id::new("probe-tight"), "/srv", &names, 5, "", 9, &buttons, &Pal::now());
            ended.set(child.cursor().min.y);
        };
        let _ = pass(&ctx, vec2(400.0, 400.0), Vec::new(), &mut run);
        let _ = pass(&ctx, vec2(400.0, 400.0), Vec::new(), &mut run);
        let bottom = 20.0 + 18.0 + 12.0 + 30.0 + 20.0 + foot_h.get() - 3.0;
        let foot = |k: usize| ctx.read_response(Id::new("probe-tight").with("foot").with(k)).expect("drawn").rect;
        let row = ctx.read_response(Id::new("probe-tight").with("list").with("row").with(5usize)).expect("the cursor's row").rect;
        assert!((row.height() - FOLDER_ROW_H).abs() < 0.01, "one row, the cursor's, whole: {row:?}");
        assert!((row.min.y - (20.0 + 18.0 + 11.0)).abs() < 0.01, "11 under the path: {row:?}");
        assert!((foot(0).min.y - row.max.y - 18.0).abs() < 0.01, "18 over the foot: {row:?} {:?}", foot(0));
        let last = (0..4).map(foot).fold(f32::MIN, |y, r| y.max(r.max.y));
        assert!(last <= bottom + 0.01 && ended.get() <= bottom + 0.01, "the whole foot in the body: {last} {} {bottom}", ended.get());
    }

    /// With no folders and no words for none, the list draws no row and
    /// takes no room, so a body just tall enough for the path and the foot
    /// with the 12 and the 20 keeps them both.
    #[test]
    fn a_browser_with_no_row_keeps_its_gaps_in_a_body_just_tall_enough_without_one() {
        let ctx = context();
        let buttons = [("up", false), ("open", false), ("Add this very folder here", true), ("close it", false)];
        let (left, w) = (20.0, 300.0);
        let foot_h = Cell::new(0.0);
        let mut run = |ui: &mut Ui| {
            foot_h.set(foot_rects(ui, &buttons, &[], w).1);
            let body = Rect::from_min_size(pos2(left, 20.0), vec2(w, 18.0 + 12.0 + 20.0 + foot_h.get()));
            let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
            let _ = folder_browser(&mut child, Id::new("probe-bare"), "/srv/bare", &[], 0, "", 9, &buttons, &Pal::now());
        };
        let _ = pass(&ctx, vec2(400.0, 400.0), Vec::new(), &mut run);
        let foot = |k: usize| ctx.read_response(Id::new("probe-bare").with("foot").with(k)).expect("drawn").rect;
        assert!((foot(0).min.y - (20.0 + 18.0 + 12.0 + 20.0)).abs() < 0.01, "the foot 12 and 20 under the path: {:?}", foot(0));
        let last = (0..4).map(foot).fold(f32::MIN, |y, r| y.max(r.max.y));
        assert!((last - (20.0 + 18.0 + 12.0 + 20.0 + foot_h.get())).abs() < 0.01, "the whole foot in the body, to its end: {last}");
    }

    /// A folder browser in a modal over a room that stands off the pixels
    /// (an admin room's, its cells' rect in points) settles wherever it
    /// stands and however far its gaps close, at 100% and at 125%: a
    /// further pass asks for no other height, and the whole foot stands in
    /// the body's view. The height the modal remembers moves its window by
    /// fractions of a pixel, and the body has the same room wherever the
    /// window stands.
    #[test]
    fn a_browser_in_a_modal_off_the_pixels_settles_with_its_whole_foot() {
        let names: Vec<String> = (0..40).map(|i| format!("d{i:02}")).collect();
        let buttons = [("up", false), ("open", false), ("Add this very folder here", true), ("close it", false)];
        let size = vec2(800.0, 700.0);
        // A window 400 wide with a one-line title: 88 points of padding
        // and title over and under its body.
        let (width, fixed) = (400.0, 2.0 * MODAL_PAD + TITLE_H + MODAL_GAP);
        for zoom in [1.0, 1.25] {
            let ctx = context();
            ctx.set_zoom_factor(zoom);
            let mut foot_h = 0.0;
            let _ = pass(&ctx, size, Vec::new(), &mut |ui| foot_h = foot_rects(ui, &buttons, &[], width - 2.0 * MODAL_PAD).1);
            // The body too short for the path, a row and the foot with the
            // 12 and the 20 by a fraction of a point, by a point and more,
            // by half the two gaps, by all of them but a fraction, and by
            // all of them, where the body is exactly as tall as the path,
            // the row and the foot.
            for (case, short) in [0.3, 1.3, 16.7, 31.3, 32.0].into_iter().enumerate() {
                for step in 0..6 {
                    let id = Id::new("probe-off-pixels").with(case).with(step);
                    let budget = 18.0 + 12.0 + 30.0 + 20.0 + foot_h - short;
                    let over = Rect::from_min_size(pos2(0.0, 30.519 + 0.13 * step as f32), vec2(600.0, budget + fixed + 32.0));
                    let mut run = |ui: &mut Ui| {
                        let pal = Pal::now();
                        let _ = Dialog::new(id, over, width, "Folders").show(ui, &pal, |ui| {
                            folder_browser(ui, id, "/srv", &names, 5, "", 9, &buttons, &pal)
                        });
                    };
                    for _ in 0..4 {
                        let _ = pass(&ctx, size, Vec::new(), &mut run);
                    }
                    let out = pass(&ctx, size, Vec::new(), &mut run);
                    let at = format!("at {zoom}, over from {} and {} tall", over.min.y, over.height());
                    let settled = out.platform_output.num_completed_passes == 1 && !out.platform_output.requested_discard();
                    assert!(settled, "{at}: settled, not {:?}", out.platform_output.request_discard_reasons);
                    let view = ctx.data(|d| d.get_temp::<BodyView>(id.with("view"))).expect("the body").view;
                    for k in 0..4usize {
                        let b = ctx.read_response(id.with("foot").with(k)).expect("drawn").rect;
                        assert!(b.min.y >= view.min.y - 0.01 && b.max.y <= view.max.y + 0.01, "{at}: button {k} whole in view, {b:?} in {view:?}");
                    }
                }
            }
        }
    }

    // ── Cards, headings and pictures halved ──────────────────────────────

    /// The edge and the fill a pass drew round `rect`, the last of each.
    fn look_of(out: &egui::FullOutput, rect: Rect) -> (Option<Color32>, Option<Color32>) {
        let at = |r: &Rect| (r.min - rect.min).length() < 0.01 && (r.max - rect.max).length() < 0.01;
        let shapes = || out.shapes.iter().filter_map(|c| if let Shape::Rect(r) = &c.shape { at(&r.rect).then_some(r) } else { None });
        let edge = shapes().filter(|r| r.stroke.width > 0.0).last().map(|r| r.stroke.color);
        let fill = shapes().filter(|r| r.fill != Color32::TRANSPARENT).last().map(|r| r.fill);
        (edge, fill)
    }

    #[test]
    fn a_cards_keyboard_look_wins_over_the_pointer_on_a_link_and_a_tick_alike() {
        let ctx = context();
        let pal = Pal::now();
        let size = vec2(600.0, 400.0);
        let link = Rect::from_min_size(pos2(20.0, 20.0), vec2(420.0, 56.0));
        let focus = Cell::new(false);
        let tick = Cell::new(Rect::NOTHING);
        let clicked = Cell::new(false);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            if link_card(ui, Id::new("probe-link"), link, "The admin panel", focus.get(), &pal).clicked() {
                clicked.set(true);
            }
            let column = Rect::from_min_size(pos2(20.0, 120.0), vec2(500.0, 100.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::top_down(Align::Min)));
            let _ = tick_card(&mut child, Id::new("probe-tick-card"), false, "Updates", "check daily", focus.get(), &pal);
            tick.set(Rect::from_min_size(column.min, vec2(500.0, 60.0)));
        };
        let at_rest = pass(&ctx, size, vec![egui::Event::PointerMoved(pos2(590.0, 390.0))], &mut run);
        assert_eq!(look_of(&at_rest, link), (Some(pal.edge), Some(pal.panel)), "at rest");
        assert_eq!(look_of(&at_rest, tick.get()), (Some(pal.edge), Some(pal.panel)));
        let words: Vec<(String, Rect)> = texts(&at_rest);
        let label = words.iter().find(|(t, _)| t == "The admin panel").expect("the label").1;
        assert!((label.min.x - (link.min.x + LINK_PAD)).abs() < 0.5, "18 in: {label:?}");
        for (card, name) in [(link, "the link"), (tick.get(), "the tick")] {
            focus.set(false);
            let _ = pass(&ctx, size, vec![egui::Event::PointerMoved(card.center())], &mut run);
            let hovered = pass(&ctx, size, Vec::new(), &mut run);
            assert_eq!(look_of(&hovered, card), (Some(pal.dim), Some(pal.raised)), "{name} under the pointer");
            focus.set(true);
            let both = pass(&ctx, size, Vec::new(), &mut run);
            assert_eq!(look_of(&both, card), (Some(pal.accent), Some(pal.raised)), "{name}: the keyboard's look wins");
            let _ = pass(&ctx, size, vec![egui::Event::PointerMoved(pos2(590.0, 390.0))], &mut run);
            let focused = pass(&ctx, size, Vec::new(), &mut run);
            assert_eq!(look_of(&focused, card), (Some(pal.accent), Some(pal.raised)), "{name}: the keyboard's look alone");
        }
        for events in click_events(link.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(clicked.get(), "the whole card is the click");
    }

    #[test]
    fn a_quiet_button_is_as_wide_as_its_words_and_lights_them_under_the_pointer() {
        let ctx = context();
        let size = vec2(600.0, 200.0);
        let at = pos2(40.0, 60.0);
        let (clicked, words_w) = (Cell::new(false), Cell::new(0.0));
        let mut run = |ui: &mut Ui| {
            words_w.set(width_of(ui, "Skip for now", body(13.0)));
            if quiet_button(ui, Id::new("probe-quiet"), at, "Skip for now", 300.0, &Pal::now()).clicked() {
                clicked.set(true);
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let rect = ctx.read_response(Id::new("probe-quiet")).expect("drawn").rect;
        assert_eq!(rect, Rect::from_min_size(pos2(at.x, at.y - HIT / 2.0), vec2(words_w.get(), HIT)), "the words' width, a control's height");
        let color = |out: &egui::FullOutput| {
            out.shapes.iter().find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Skip for now" => t.galley.job.sections.first().map(|s| s.format.color),
                _ => None,
            })
        };
        assert_eq!(color(&out), Some(Pal::now().dim), "dim at rest");
        let _ = pass(&ctx, size, vec![egui::Event::PointerMoved(rect.center())], &mut run);
        assert_eq!(color(&pass(&ctx, size, Vec::new(), &mut run)), Some(Pal::now().text), "lit under the pointer");
        // Past the words' end is no part of it.
        for events in click_events(pos2(rect.max.x + 6.0, at.y)) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(!clicked.get());
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(clicked.get());
    }

    #[test]
    fn a_heading_takes_its_height_and_a_scrolled_body_brings_its_control_just_into_view() {
        let ctx = context();
        let after = Cell::new(0.0);
        let _ = pass(&ctx, vec2(600.0, 400.0), Vec::new(), &mut |ui| {
            let top = ui.cursor().min.y;
            heading(ui, "Extras", "Optional features", &Pal::now());
            after.set(ui.cursor().min.y - top);
        });
        assert!((after.get() - HEADING_H).abs() < 0.01, "{}", after.get());
        // Fresh: from the top, brought down to a control under the view.
        let run = |a: f32, b: f32| Rangef::new(a, b);
        assert_eq!(kept_offset(true, 90.0, None, 200.0), (0.0, true), "up afresh: its top");
        assert_eq!(kept_offset(false, 90.0, None, 200.0), (90.0, false), "where the wheel left it");
        assert_eq!(kept_offset(true, 0.0, Some(run(260.0, 320.0)), 200.0), (120.0, true), "just far enough down");
        assert_eq!(kept_offset(false, 120.0, Some(run(60.0, 120.0)), 200.0), (60.0, true), "just far enough up");
        assert_eq!(kept_offset(false, 120.0, Some(run(150.0, 210.0)), 200.0), (120.0, false), "in view: unmoved");
        assert_eq!(kept_offset(false, 0.0, Some(run(300.0, 600.0)), 200.0), (300.0, true), "taller than the view: its top");
    }

    #[test]
    fn a_select_chip_is_as_wide_as_its_words_and_takes_the_click() {
        let ctx = context();
        let size = vec2(400.0, 100.0);
        let clicked = Cell::new(false);
        let wants = Cell::new((0.0, 0.0));
        let mut run = |ui: &mut Ui| {
            wants.set((select_chip_width(ui, "日本語"), select_chip_width(ui, "Deutsch")));
            if select_chip(ui, Id::new("probe-chip"), pos2(24.0, 30.0), "Deutsch", &Pal::now()).clicked() {
                clicked.set(true);
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let rect = ctx.read_response(Id::new("probe-chip")).expect("drawn").rect;
        assert_eq!(rect, Rect::from_min_size(pos2(24.0, 30.0 - HIT / 2.0), vec2(wants.get().1, HIT)));
        assert!(wants.get().0 < wants.get().1, "as wide as its words: {:?}", wants.get());
        let words = texts(&out).into_iter().find(|(t, _)| t == "Deutsch").expect("the words").1;
        assert!((words.min.x - (24.0 + 30.0)).abs() < 0.5 && rect.contains_rect(words), "{words:?} in {rect:?}");
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert!(clicked.get());
    }

    #[test]
    fn a_listed_sentence_hangs_beside_its_mark_and_any_other_is_set_as_it_is() {
        let ctx = context();
        let c = Color32::WHITE;
        let out = pass(&ctx, vec2(800.0, 300.0), Vec::new(), &mut |ui| {
            let _ = Para::listed("+ Anyone can reach it", body(14.0), c).paint(ui, pos2(10.0, 10.0), 400.0);
            let _ = Para::listed("− Nobody signs in", body(14.0), c).paint(ui, pos2(10.0, 60.0), 400.0);
            let _ = Para::listed("+3 is no mark", body(14.0), c).paint(ui, pos2(10.0, 110.0), 400.0);
        });
        let drawn: Vec<String> = texts(&out).into_iter().map(|(t, _)| t).collect();
        for w in ["+", "Anyone can reach it", "−", "Nobody signs in", "+3 is no mark"] {
            assert!(drawn.iter().any(|t| t == w), "{w} drawn: {drawn:?}");
        }
    }

    /// A picture is never sampled at under half its pixels (the window's
    /// textures have no mipmaps): halved until it is at most twice what it
    /// is drawn at, whole where the screen's pixels are as many, each half
    /// the mean of the premultiplied texels under it.
    #[test]
    fn a_picture_is_halved_to_within_twice_the_pixels_it_is_drawn_at() {
        let size = vec2(1224.0, 306.0);
        let band = |h: f32| Rect::from_min_size(pos2(280.0, 64.0), vec2(720.0, h));
        assert_eq!(halvings(size, band(93.0), 1.0), 1, "372 pixels wide: the 612 half");
        assert_eq!(halvings(size, band(93.0), 1.5), 1, "558 wide: still the half");
        assert_eq!(halvings(size, band(93.0), 2.0), 0, "744 wide: the whole");
        assert_eq!(halvings(size, band(93.0), 3.0), 0, "never more than whole");
        assert_eq!(halvings(size, band(24.0), 1.0), 3, "drawn 24 tall: an eighth");
        assert_eq!(halvings(size, Rect::from_min_size(Pos2::ZERO, vec2(100.0, 93.0)), 1.0), 3, "a narrow band, by its width");
        for (h, ppp) in [(93.0, 1.0), (93.0, 1.25), (60.0, 1.0), (24.0, 2.0)] {
            let n = halvings(size, band(h), ppp);
            let ratio = size.y / 2f32.powi(n as i32) / (h * ppp);
            assert!(ratio <= 2.0 + 1e-3, "{h} at {ppp}: {n} halvings sample at {ratio}");
        }
        // Each texel the mean of the four under it, an odd last column
        // left out.
        let red = Color32::from_rgba_premultiplied(200, 0, 0, 200);
        let two = egui::ColorImage::new([2, 2], vec![red, Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT]);
        assert_eq!(halved(&two).pixels, [Color32::from_rgba_premultiplied(50, 0, 0, 50)]);
        let three = egui::ColorImage::new([3, 1], vec![red, red, Color32::TRANSPARENT]);
        assert_eq!((halved(&three).size, halved(&three).pixels), ([1, 1], vec![red]));
    }

    #[test]
    fn a_dialog_as_wide_as_its_width_says_keeps_its_title_on_one_line() {
        let ctx = context();
        let size = vec2(1000.0, 500.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let title = "Sprache wählen, bitte, für die ganze Einrichtung";
        let (width, bare_body) = (Cell::new(0.0), Cell::new(0.0));
        let mut run = |ui: &mut Ui| {
            width.set(dialog_width(ui, title, 120.0));
            bare_body.set(dialog_width(ui, "", 300.0));
            let _ = Dialog::new(Id::new("probe-width"), over, width.get(), title).show(ui, &Pal::now(), |ui| ui.add_space(40.0));
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert!(width.get() > 120.0 + 2.0 * MODAL_PAD, "the title wants more than the body: {}", width.get());
        assert_eq!(bare_body.get(), 300.0 + 2.0 * MODAL_PAD, "a body wider than its title: the body and the padding");
        let galley = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == title => Some(t.galley.clone()),
            _ => None,
        });
        assert_eq!(galley.expect("the title").rows.len(), 1, "one line beside the close");
    }

    /// The runs of `s` [`figured`] sets in the figure face.
    fn figures_in(s: &str, also: &[&str]) -> Vec<String> {
        let figure = mono(12.5);
        let parts = figured(s, also, body(13.5), figure.clone(), Color32::WHITE);
        let whole: String = parts.iter().map(|(s, ..)| s.as_str()).collect();
        assert_eq!(whole, s, "the words kept whole, spaces and all");
        parts.into_iter().filter(|(_, font, _)| *font == figure).map(|(s, ..)| s).collect()
    }

    #[test]
    fn a_sentences_figures_are_set_in_the_monospace_and_its_words_are_not() {
        assert_eq!(figures_in("21.4 of 59.6 GB used · 1,284 tracks", &[]), ["21.4", "59.6", "1,284"]);
        assert_eq!(figures_in("Update available · v0.13.0 → v0.14.0", &[]), ["v0.13.0", "v0.14.0"]);
        assert_eq!(figures_in("v0.14.0-5-g4e94418 (dev build)", &[]), ["v0.14.0-5-g4e94418"], "a dev build whole");
        let port = "COM3 · CH9102 · serial 5B1F007751 · 921600 baud";
        assert_eq!(figures_in(port, &["COM3"]), ["COM3", "5B1F007751", "921600"], "a port only by name; a chip is a word");
        assert_eq!(figures_in("mstream-mp3-player v0.14.0 · QIO · ELF fa4e0000.", &[]), ["v0.14.0", "fa4e0000"], "the full stop left out");
        assert!(figures_in("M5Stack Core2 · FAT32 · a decade", &[]).is_empty(), "names and words with a digit in them stay words");
        assert_eq!(figures_in("64 % battery · set at 14:03 (it was 3.2 s off)", &[]), ["64", "14:03", "3.2"]);
        assert_eq!(figures_in("writing v0.14.0… 62% · about 15 s left", &[]), ["v0.14.0", "62%", "15"], "a percentage with its sign");
        assert_eq!(figures_in("v0.8.0に更新しました", &[]), ["v0.8.0"], "a figure ends where its script does");
        // A note's path: the file named for its number keeps its extension
        // in the words' face, so a cut falls in the words, not in a figure.
        let note = "EACCES: permission denied, open '/srv/music/2024/07 Track 12.flac' (errno -13)";
        assert_eq!(figures_in(note, &[]), ["12", "13"], "a path and a file's extension stay words");
        assert_eq!(figures_in("ELF 11c35a4a.bin · 2026.11.25.log · 1.5e3", &[]), ["11c35a4a", "2026.11.25", "1.5e3"], "a point a letter follows ends a figure, any other joins it");
        // A name the caller names a figure is one whole, its extension and
        // all, whatever it starts with; one it does not name keeps the cut.
        let build = "4MB-merged.bin · v0.8.0 · 1,048,576 B";
        assert_eq!(figures_in(build, &["4MB-merged.bin"]), ["4MB-merged.bin", "v0.8.0", "1,048,576"], "a file named in `also` whole");
        assert_eq!(figures_in("0.8.1-rc1.bin, read", &["0.8.1-rc1.bin"]), ["0.8.1-rc1.bin"], "and less the mark it ends on");
        assert_eq!(figures_in(build, &["firmware.bin"]), ["4MB-merged", "v0.8.0", "1,048,576"], "another file's extension stays a word");
    }

    #[test]
    fn a_share_lights_whole_segments_a_sliver_one_and_the_last_only_when_whole() {
        assert_eq!(fill_lit(0, 114), 0);
        assert_eq!(fill_lit(1, 114), 1, "a sliver still shows a segment");
        assert_eq!(fill_lit(359, 114), 41, "the canvas's 21.4 of 59.6 GB");
        assert_eq!(fill_lit(996, 114), 113, "not quite full never shows full");
        assert_eq!(fill_lit(1000, 114), 114);
        assert_eq!(fill_lit(2000, 10), 10, "past the whole is whole, no more");
    }

    #[test]
    fn a_text_button_is_its_words_and_answers_the_pointer_only_while_live() {
        let ctx = context();
        let size = vec2(500.0, 200.0);
        let clicked = Cell::new(0);
        let live = Cell::new(true);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let at = pos2(20.0, 50.0);
            if text_button(ui, Id::new("probe-text"), at, "Write v0.14.0 again", Some(Lead::Shut), &[], 400.0, live.get(), &pal).clicked() {
                clicked.set(clicked.get() + 1);
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let drawn = texts(&out);
        let words = drawn.iter().find(|(t, _)| t == "Write v0.14.0 again").expect("its words, the figure's run among them");
        assert!(words.1.min.x >= 20.0 + LEAD_W - 0.5, "after its lead: {:?}", words.1);
        let rect = ctx.read_response(Id::new("probe-text")).expect("its target").rect;
        assert_eq!(rect.height(), TEXT_BUTTON_H);
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(clicked.get(), 1, "a click");
        let out = pass(&ctx, size, vec![egui::Event::PointerMoved(rect.center())], &mut run);
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand, "the hand over it");
        live.set(false);
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(clicked.get(), 1, "at rest under something, it takes no click");
    }

    #[test]
    fn the_tall_primary_acts_when_enabled_and_says_why_not_otherwise() {
        let ctx = context();
        let size = vec2(500.0, 200.0);
        let rect = Rect::from_min_size(pos2(200.0, 40.0), vec2(180.0, TALL_H));
        let clicked = Cell::new(0);
        let enabled = Cell::new(true);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let response = tall_button(ui, Id::new("probe-tall"), rect, "Update ▸", enabled.get(), true, &pal);
            // The caller's why, while it waits.
            let response = if enabled.get() { response } else { response.on_hover_text("one board at a time") };
            if response.clicked() {
                clicked.set(clicked.get() + 1);
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert!(texts(&out).iter().any(|(t, _)| t == "UPDATE"), "the cells' arrow off, in capitals: {:?}", texts(&out));
        assert!(tall_button_width_probe(&ctx, "Update ▸", true) > tall_button_width_probe(&ctx, "Update ▸", false), "the pointer's room");
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(clicked.get(), 1);
        enabled.set(false);
        for events in click_events(rect.center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(clicked.get(), 1, "disabled: no click");
        let out = pass(&ctx, size, vec![egui::Event::PointerMoved(rect.center())], &mut run);
        assert_ne!(out.platform_output.cursor_icon, CursorIcon::PointingHand, "and no hand");
        // But it still senses the pointer, so the caller's tooltip says why
        // (in a context of its own: egui shows none where the pointer
        // rests after a click).
        let ctx = context();
        let _ = pass(&ctx, size, vec![egui::Event::PointerMoved(rect.center())], &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert!(ctx.read_response(Id::new("probe-tall")).is_some_and(|r| r.hovered()), "the waiting primary under the pointer");
        assert!(texts(&out).iter().any(|(t, _)| t == "one board at a time"), "its why in the tooltip: {:?}", texts(&out));
    }

    fn tall_button_width_probe(ctx: &egui::Context, label: &str, enabled: bool) -> f32 {
        let w = Cell::new(0.0);
        let _ = pass(ctx, vec2(400.0, 100.0), Vec::new(), &mut |ui| w.set(tall_button_width(ui, label, enabled)));
        w.get()
    }

    /// What `measure` gives back of a pass over a room of `size`.
    fn measured<T: Default>(ctx: &egui::Context, size: Vec2, measure: impl Fn(&Ui) -> T) -> T {
        let got = std::cell::RefCell::new(T::default());
        let _ = pass(ctx, size, Vec::new(), &mut |ui| *got.borrow_mut() = measure(ui));
        got.into_inner()
    }

    #[test]
    fn a_gates_answers_stand_side_by_side_or_stack_and_its_go_waits() {
        let ctx = context();
        let size = vec2(800.0, 300.0);
        let laid = |w: f32, go: Option<&'static str>| {
            measured(&ctx, size, |ui| answers_rects(ui, &Answers { keep: "Keep v0.13.0", lead: true, go, also: &[] }, w))
        };
        // Room for both: side by side at the foot's end, the go last.
        let (both, h) = laid(500.0, Some("Update"));
        assert_eq!(h, FORM_BUTTON_H);
        let (keep, go) = (both[0], both[1]);
        assert!((go.max.x - 500.0).abs() < 0.01 && (go.min.x - keep.max.x - FOOT_GAP).abs() < 0.01 && keep.min.y == go.min.y, "{both:?}");
        // Short of that: the go keeps the first row, the keep under it, both
        // ending at the foot's end.
        let (stacked, h) = laid(keep.width() + go.width(), Some("Update"));
        assert_eq!(h, 2.0 * FORM_BUTTON_H + FOOT_GAP);
        assert!(stacked[1].min.y == 0.0 && stacked[0].min.y == FORM_BUTTON_H + FOOT_GAP, "{stacked:?}");
        assert!((stacked[0].max.x - stacked[1].max.x).abs() < 0.01, "{stacked:?}");
        // While the go waits on the gate's box: the keep alone at the end.
        let (alone, h) = laid(500.0, None);
        assert_eq!((alone.len(), h), (1, FORM_BUTTON_H));
        assert!((alone[0].max.x - 500.0).abs() < 0.01 && (alone[0].width() - keep.width()).abs() < 0.01);
        // The keep's lead takes its pointer's 6 points and the 8 after it.
        let led = measured(&ctx, size, |ui| keep_button_width(ui, "Keep v0.13.0", true, &[]) - keep_button_width(ui, "Keep v0.13.0", false, &[]));
        assert!((led - KEEP_LEAD).abs() < 0.01, "{led}");
    }

    #[test]
    fn a_gate_of_the_pages_own_answers_with_its_keep_led_back_and_its_go_only_once_offered() {
        let ctx = context();
        let size = vec2(800.0, 400.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-gate-with");
        let go = Cell::new(false);
        let answered = Cell::new(None);
        let ticked = Cell::new(0);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let a = Answers { keep: "Not now", lead: true, go: go.get().then_some("Remove 214 and sync"), also: &[] };
            let shown = gate_with(ui, id, over, "Remove 214 files from the card?", &a, &pal, |ui| {
                Para::new("They are gone from the server.", body(14.0), pal.text).show(ui);
                check_line(ui, id.with("box"), go.get(), "Remove them anyway", "", false, &pal).clicked()
            });
            if let Some(went) = shown.inner.0 {
                answered.set(Some(went));
            }
            if shown.inner.1 {
                ticked.set(ticked.get() + 1);
            }
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let pal = Pal::now();
        assert!(texts(&out).iter().any(|(t, _)| t == "NOT NOW"), "the keep's words in capitals: {:?}", texts(&out));
        let lead = out.shapes.iter().any(|c| matches!(&c.shape, Shape::Path(p) if p.fill == pal.on_accent && p.points.len() == 3));
        assert!(lead, "led by the pointer turned back");
        assert!(ctx.read_response(id.with("foot").with(1usize)).is_none(), "no go while it waits");
        let rect = |id: Id| ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} drawn")).rect;
        for events in click_events(rect(id.with("box")).center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(ticked.get(), 1, "the box's click is the page's");
        go.set(true);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        for events in click_events(rect(id.with("foot").with(1usize)).center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(answered.take(), Some(true), "the go, once offered, goes through");
        for events in click_events(rect(id.with("foot").with(0usize)).center()) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(answered.take(), Some(false), "the keep keeps");
    }

    #[test]
    fn a_modal_drawn_afresh_for_another_key_comes_back_to_its_top() {
        let ctx = context();
        let size = vec2(600.0, 300.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let id = Id::new("probe-afresh");
        let key = std::cell::RefCell::new("the first".to_string());
        let at = Cell::new(0usize);
        let first = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let key = key.borrow().clone();
            let _ = Dialog::new(id, over, 400.0, "Rows").gate(&pal).afresh(&key).show(ui, &pal, |ui| {
                for i in 0..30 {
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
                    if i == 0 {
                        first.set(rect);
                    }
                    if i == at.get() {
                        reveal(ui, id, i, rect);
                    }
                }
            });
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let home = first.get().min.y;
        at.set(29);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!(first.get().min.y < home - 100.0, "the body scrolled: {:?}", first.get());
        // The same one again, nothing brought into view: it stays.
        at.set(usize::MAX);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!(first.get().min.y < home - 100.0, "and stays: {:?}", first.get());
        // Another in its place, under the same id: at its top.
        *key.borrow_mut() = "the next".to_string();
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!((first.get().min.y - home).abs() < 0.5, "afresh at its top: {:?} for {home}", first.get());
    }

    #[test]
    fn a_dialogs_body_room_is_the_room_it_hands_its_body() {
        let ctx = context();
        let size = vec2(560.0, 333.3);
        let over = Rect::from_min_max(pos2(0.0, 0.7), size.to_pos2());
        let title = "A sheet's title long enough to wrap beside its close in a narrow window";
        let (known, handed) = (Cell::new(Vec2::ZERO), Cell::new(Vec2::ZERO));
        let mut run = |ui: &mut Ui| {
            let dialog = Dialog::new(Id::new("probe-room"), over, 640.0, title);
            known.set(dialog.body_room(ui));
            let _ = dialog.show(ui, &Pal::now(), |ui| {
                handed.set(ui.max_rect().size());
                ui.add_space(900.0);
            });
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        assert!((known.get() - handed.get()).length() < 0.01, "known {:?}, handed {:?}", known.get(), handed.get());
    }

    #[test]
    fn a_list_that_hangs_holds_whole_entries_lights_its_cursor_and_takes_a_click_only_on_an_entry_with_an_id() {
        assert_eq!(hung_fits(100.0, 100.0 + 2.0 * HUNG_IN + 3.0 * HUNG_ROW_H), 3);
        assert_eq!(hung_fits(100.0, 100.0 + 2.0 * HUNG_IN + 3.0 * HUNG_ROW_H - 0.5), 2, "part of an entry is no room");
        assert_eq!(hung_fits(100.0, 90.0), 0);
        assert_eq!(hung_width(100.0, 3, 3), 100.0 + 2.0 * HUNG_PAD + 2.0 * HUNG_IN);
        assert_eq!(hung_width(100.0, 3, 8), 100.0 + 2.0 * HUNG_PAD + HUNG_GUTTER + 2.0 * HUNG_IN, "the thumb's gutter while it holds fewer");
        assert_eq!(hung_left(50.0) + HUNG_IN + HUNG_PAD, 50.0 + SELECT_PAD, "its words under the box's");
        let frame = hung_frame(pos2(40.0, 60.0), 200.0, 3);
        assert_eq!(frame.height(), 3.0 * HUNG_ROW_H + 2.0 * HUNG_IN);
        let ctx = context();
        let size = vec2(400.0, 300.0);
        let picked = Cell::new(None);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let id_of = |k: usize| (k != 1).then(|| Id::new("probe-hung").with(k));
            let words = |ui: &Ui, k: usize, room: Rect, on: bool| {
                let ink = if on { pal.on_accent } else { pal.text };
                text(ui, room.left_center(), &format!("entry {k}"), body(14.0), ink, room.width(), Align2::LEFT_CENTER);
            };
            if let Some(k) = hung_list(ui, frame, 0, 3, 3, Some(2), id_of, &pal, words) {
                picked.set(Some(k));
            }
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let pal = Pal::now();
        let at = |words: &str| texts(&out).into_iter().find(|(t, _)| t == words).unwrap_or_else(|| panic!("{words:?} drawn")).1;
        let lit = |r: Rect| out.shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(s) if s.fill == pal.accent && s.rect.contains(r.center())));
        assert!(lit(at("entry 2")) && !lit(at("entry 0")), "the cursor's entry lit");
        assert!((at("entry 0").min.x - (frame.min.x + HUNG_IN + HUNG_PAD)).abs() < 0.5, "its words padded in");
        let (zero, one) = (at("entry 0").center(), at("entry 1").center());
        for events in click_events(one) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(picked.get(), None, "an entry with no id takes nothing");
        for events in click_events(zero) {
            let _ = pass(&ctx, size, events, &mut run);
        }
        assert_eq!(picked.get(), Some(0));
    }

    #[test]
    fn a_hung_lists_thumb_stands_in_a_gutter_of_its_own_its_cursor_stopping_short_of_it() {
        let ctx = context();
        let size = vec2(400.0, 300.0);
        let frame = hung_frame(pos2(40.0, 60.0), hung_width(120.0, 3, 8), 3);
        let room = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            let pal = Pal::now();
            let words = |_: &Ui, k: usize, r: Rect, _: bool| {
                if k == 0 {
                    room.set(r);
                }
            };
            let _ = hung_list(ui, frame, 0, 3, 8, Some(0), |_| None, &pal, words);
        };
        let out = pass(&ctx, size, Vec::new(), &mut run);
        let pal = Pal::now();
        let fills = |c: Color32| out.shapes.iter().filter_map(|s| match &s.shape { Shape::Rect(r) if r.fill == c => Some(r.rect), _ => None }).collect::<Vec<_>>();
        let (cursor, thumb) = (fills(pal.accent), fills(pal.dim.gamma_multiply(THUMB_REST)));
        assert_eq!((cursor.len(), thumb.len()), (1, 1), "the cursor lit and the thumb drawn");
        assert!(cursor[0].max.x + 1.0 <= thumb[0].min.x, "the cursor stops short of the thumb: {:?} {:?}", cursor[0], thumb[0]);
        assert!(thumb[0].max.x <= frame.max.x - HUNG_IN, "the thumb inside the frame: {:?}", thumb[0]);
        assert!((room.get().width() - 120.0).abs() < 0.01, "the entry's words keep their room beside the gutter: {:?}", room.get());
    }

    #[test]
    fn a_closed_lists_box_keeps_its_least_width_and_grows_with_its_words() {
        let ctx = context();
        let size = vec2(600.0, 100.0);
        assert_eq!(measured(&ctx, size, |ui| select_box_width(ui, "—", &[], 14.0)), SELECT_LEAST);
        let words = "A server with a rather long name";
        let (long, want) = measured(&ctx, size, |ui| (select_box_width(ui, words, &[], 14.0), 3.0 * SELECT_PAD + SELECT_CHEVRON + width_of(ui, words, strong(14.0))));
        assert!(long > SELECT_LEAST && (long - want).abs() < 0.01, "{long} for {want}");
    }

    #[test]
    fn spaced_prose_keeps_its_words_leading_where_it_opens_on_a_figure() {
        let ctx = context();
        let tail = " and a tail of words long enough to wrap onto a second line here";
        let pitch = |s: &str| {
            measured(&ctx, vec2(600.0, 300.0), |ui| {
                let parts = figured(s, &[], body(13.5), mono(12.5), Color32::WHITE);
                let galley = Para::runs(&parts).spaced(20.0).paint(ui, Pos2::ZERO, 220.0);
                galley.height()
            })
        };
        let (figure, words) = (pitch(&format!("41.2 of 63.8 GB used{tail}")), pitch(&format!("Used, 41.2 of 63.8 GB{tail}")));
        assert!(figure > 30.0 && (figure - words).abs() < 0.01, "both wrap 20 points apart: {figure} and {words}");
    }

    #[test]
    fn a_cards_head_stands_its_right_figure_on_the_names_baseline_or_leaves_it_out() {
        let ctx = context();
        let size = vec2(600.0, 100.0);
        let (name, right) = ("M5Stack Core2", "COM3");
        let w = Cell::new(400.0);
        let next = Cell::new(0.0);
        let mut run = |ui: &mut Ui| next.set(card_head(ui, pos2(20.0, 30.0), w.get(), name, right, &Pal::now()));
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert_eq!(next.get(), 30.0 + HEAD_H);
        let lines = baselines(&out);
        let baseline = |s: &str| lines.iter().find(|(t, _)| t == s).and_then(|(_, g)| g.first().map(|(_, y)| *y)).unwrap_or_else(|| panic!("{s:?} drawn"));
        assert!((baseline(name) - baseline(right)).abs() < 0.5, "on the name's baseline");
        let at = texts(&out).into_iter().find(|(t, _)| t == right).map(|(_, r)| r).expect("the figure");
        assert!((at.max.x - 420.0).abs() < 1.0, "at the head's end: {at:?}");
        let (name_w, right_w) = measured(&ctx, size, |ui| (width_of(ui, name, strong(16.0)), width_of(ui, right, mono(12.5))));
        w.set(name_w + HEAD_CLEAR + right_w - 1.0);
        let out = pass(&ctx, size, Vec::new(), &mut run);
        assert!(!texts(&out).iter().any(|(t, _)| t == right), "within 16 points of the name: left out");
    }

    #[test]
    fn a_lists_thumb_is_as_long_as_its_share_and_stands_where_its_view_does() {
        let ctx = context();
        let pane = Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 180.0));
        let thumb = |first: usize| {
            let out = pass(&ctx, vec2(400.0, 300.0), Vec::new(), &mut |ui| list_thumb(ui, pane, 10, 40, first, &Pal::now()));
            out.shapes.iter().find_map(|c| match &c.shape {
                Shape::Rect(r) if r.rect.width() == THUMB_W => Some(r.rect),
                _ => None,
            })
        };
        let top = thumb(0).expect("a thumb");
        assert_eq!((top.min.y, top.height(), top.max.x), (pane.min.y, 45.0, pane.max.x), "a quarter of the pane at its top");
        let foot = thumb(30).expect("a thumb");
        assert!((foot.max.y - pane.max.y).abs() < 0.01, "the newest lines: at its foot");
    }

    #[test]
    fn a_keyed_text_button_wears_the_accent_and_the_pointer_wins_over_it() {
        let ctx = context();
        let size = vec2(500.0, 200.0);
        let at = pos2(20.0, 50.0);
        let ink = |out: &egui::FullOutput| {
            out.shapes.iter().find_map(|c| match &c.shape {
                Shape::Text(t) if t.galley.text() == "Choose a file…" => Some(t.galley.job.sections[0].format.color),
                _ => None,
            })
        };
        let mut run = |ui: &mut Ui| {
            let _ = text_button_keyed(ui, Id::new("probe-keyed"), at, "Choose a file…", &[], 300.0, true, &Pal::now());
        };
        let pal = Pal::now();
        assert_eq!(ink(&pass(&ctx, size, Vec::new(), &mut run)), Some(pal.accent), "the keyboard's");
        assert_eq!(ink(&pass(&ctx, size, vec![egui::Event::PointerMoved(at + vec2(10.0, 0.0))], &mut run)), Some(pal.text), "the pointer's");
    }

    /// What stands beside a paragraph on its baseline ([`Para::baseline`])
    /// — a line of the UI face or of the monospace centred on the middle
    /// [`mid_on`] gives, runs led by a figure set by [`runs_on`] — stands
    /// on it: on the pixel egui rounds the paragraph's first line to,
    /// whether its words are the UI face's alone or led by a figure, its
    /// top on a whole point or between, within a hundredth of a point.
    /// Centred by the face's ascent, as the MP3 tab once set its free
    /// figure and its scan's share, they stood up to a point off it in
    /// Segoe UI, more in a face whose ascent egui rounds the other way.
    #[test]
    fn what_stands_beside_a_paragraph_stands_on_its_baseline() {
        let ctx = context();
        let white = Color32::WHITE;
        let mut placed = Vec::new();
        let out = pass(&ctx, vec2(900.0, 1200.0), Vec::new(), &mut |ui| {
            let mut y = 20.0;
            for size in [12.5, 13.0, 13.5, 14.0] {
                for nudge in [0.0, 0.3, 0.6] {
                    for led in [false, true] {
                        let k = placed.len();
                        let words = format!("Para{k}");
                        let parts = if led { vec![(format!("12.{k}"), mono(12.5), white), (format!(" {words}"), body(size), white)] } else { vec![(words, body(size), white)] };
                        let para = Para::runs(&parts).spaced(20.0);
                        let top = y + nudge;
                        let on = top + para.baseline(ui);
                        let whole: String = parts.iter().map(|(s, ..)| s.as_str()).collect();
                        let _ = para.paint(ui, pos2(10.0, top), 300.0);
                        let _ = text(ui, pos2(320.0, mid_on(ui, on, size)), &format!("Text{k}"), body(size), white, 200.0, Align2::LEFT_CENTER);
                        let _ = text(ui, pos2(440.0, mid_on(ui, on, 12.5)), &format!("Mono{k}"), mono(12.5), white, 200.0, Align2::LEFT_CENTER);
                        let _ = runs_on(ui, 880.0, on, &[(format!("38.{k}"), mono(12.5), white), (format!(" free{k}"), body(13.5), white)], 300.0, Align::Max);
                        placed.push((whole, [format!("Text{k}"), format!("Mono{k}"), format!("38.{k} free{k}")]));
                        y += 24.0;
                    }
                }
            }
        });
        let lines = baselines(&out);
        let on = |s: &str| lines.iter().find(|(t, _)| t == s).unwrap_or_else(|| panic!("{s:?} drawn")).1[0].1;
        for (para, beside) in &placed {
            for b in beside {
                assert!((on(b) - on(para)).abs() < 0.01, "{b:?} on {para:?}'s baseline: {} and {}", on(b), on(para));
            }
        }
    }
}
/// The chrome's pieces, each held to what it promises.
#[cfg(test)]
mod chrome_pieces {
    use super::*;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        super::super::panel::fonts(&ctx);
        style_once(&ctx, &Pal::now());
        dwell(&ctx);
        ctx
    }

    const SCREEN: Vec2 = vec2(400.0, 200.0);

    /// One pass at `time` seconds with `events`; its shapes.
    fn pass(ctx: &egui::Context, time: f64, events: Vec<egui::Event>, run: &mut dyn FnMut(&mut Ui)) -> Vec<egui::epaint::ClippedShape> {
        let screen = Rect::from_min_size(Pos2::ZERO, SCREEN);
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(time), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| run(ui));
        out.textures_delta.clear();
        out.shapes
    }

    fn moved(at: Pos2) -> egui::Event {
        egui::Event::PointerMoved(at)
    }

    fn button(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::default() }
    }

    /// A control acts on the press, as the cells' do: once, on the pass the
    /// press came in, not on hover nor on the release; a press and its
    /// release in one pass are a press too; and a pass egui runs again
    /// hears none of it.
    #[test]
    fn a_control_is_pressed_on_the_press_once() {
        let ctx = context();
        let control = Rect::from_min_size(pos2(40.0, 20.0), vec2(60.0, 20.0));
        let on = control.center();
        let presses = std::cell::Cell::new(0);
        let run = |ui: &mut Ui| {
            if pressed(&ui.interact(control, Id::new("probe"), Sense::click())) {
                presses.set(presses.get() + 1);
            }
        };
        pass(&ctx, 0.0, vec![moved(on)], &mut |ui| run(ui));
        pass(&ctx, 0.1, Vec::new(), &mut |ui| run(ui));
        assert_eq!(presses.get(), 0, "hover is no press");
        pass(&ctx, 0.2, vec![button(on, true)], &mut |ui| run(ui));
        assert_eq!(presses.get(), 1, "the press");
        pass(&ctx, 0.3, Vec::new(), &mut |ui| run(ui));
        pass(&ctx, 0.4, vec![button(on, false)], &mut |ui| run(ui));
        assert_eq!(presses.get(), 1, "held and released: nothing more");
        pass(&ctx, 0.9, vec![button(on, true), button(on, false)], &mut |ui| run(ui));
        assert_eq!(presses.get(), 2, "a tap inside one pass");
        let runs = std::cell::Cell::new(0);
        pass(&ctx, 1.5, vec![button(on, true)], &mut |ui| {
            run(ui);
            runs.set(runs.get() + 1);
            ui.ctx().request_discard("a second run");
        });
        assert_eq!(runs.get(), 2, "egui ran the pass again");
        assert_eq!(presses.get(), 3, "once, though egui ran the pass again");
        pass(&ctx, 1.6, vec![button(on, false)], &mut |ui| run(ui));
        let off = pos2(300.0, 150.0);
        pass(&ctx, 2.0, vec![moved(off), button(off, true)], &mut |ui| run(ui));
        assert_eq!(presses.get(), 3, "a press off it is not its");
    }

    /// A list that picks on the press picks its entry as the button goes
    /// down; one that picks on the click waits for the release.
    #[test]
    fn a_hung_list_picks_on_the_press_or_on_the_click() {
        let ctx = context();
        let pal = Pal::now();
        let frame = hung_frame(pos2(20.0, 20.0), 160.0, 3);
        let second = pos2(60.0, frame.min.y + 2.0 + HUNG_ROW_H * 1.5);
        for (round, (pick, on_press)) in [(Pick::Press, Some(1)), (Pick::Click, None)].into_iter().enumerate() {
            let mut picked = Vec::new();
            let mut run = |ui: &mut Ui| {
                let id_of = |k: usize| Some(Id::new(("probe-row", k)));
                picked.push(hung_list_by(ui, frame, 0, 3, 3, None, id_of, pick, &pal, |_, _, _, _| {}));
            };
            // egui's clock runs on from one round to the next.
            let t = 10.0 * round as f64;
            pass(&ctx, t, vec![moved(second)], &mut run);
            pass(&ctx, t + 0.1, Vec::new(), &mut run);
            pass(&ctx, t + 0.2, vec![button(second, true)], &mut run);
            pass(&ctx, t + 0.3, vec![button(second, false)], &mut run);
            pass(&ctx, t + 1.0, vec![moved(pos2(390.0, 190.0))], &mut run);
            let released = if on_press.is_some() { None } else { Some(1) };
            assert_eq!((picked[2], picked[3]), (on_press, released), "{pick:?}: on the press, then on the release");
        }
    }

    /// The tooltip waits out the cells' dwell on its target, then hangs
    /// under its middle, inside the screen, its shapes clipped to the box
    /// it returns; none in the dwell after a key.
    #[test]
    fn a_tooltip_hangs_under_its_target_after_the_dwell() {
        let ctx = context();
        let pal = Pal::now();
        let target = Rect::from_min_size(pos2(340.0, 0.0), vec2(40.0, 16.0));
        let shown = std::cell::Cell::new(None);
        let run = |ui: &mut Ui, since_key: Option<std::time::Duration>| {
            let response = ui.interact(target, Id::new("probe-tip"), Sense::click());
            shown.set(tip_under(&response, "Add a server", since_key, &pal));
        };
        pass(&ctx, 0.0, vec![moved(target.center())], &mut |ui| run(ui, None));
        pass(&ctx, 0.2, Vec::new(), &mut |ui| run(ui, None));
        assert_eq!(shown.get(), None, "not before the dwell");
        let shapes = pass(&ctx, 0.8, Vec::new(), &mut |ui| run(ui, None));
        let tip = shown.get().expect("after it");
        assert!((tip.min.y - (target.max.y + TIP_UNDER)).abs() <= 1.0, "{TIP_UNDER} points under the target: {tip:?}");
        assert!(tip.max.x <= SCREEN.x && tip.min.x >= 0.0, "pulled in at the edge: {tip:?}");
        assert!(tip.width() <= TIP_W, "{tip:?}");
        assert!(tip.center().x < target.center().x + 0.5, "it would have passed the edge centred: {tip:?}");
        let boxed = shapes.iter().filter(|shape| shape.clip_rect == tip).count();
        assert!(boxed >= 3, "the fill, the hairline and the words, clipped to the box: {boxed} of {}", shapes.len());
        pass(&ctx, 0.9, Vec::new(), &mut |ui| run(ui, Some(std::time::Duration::from_millis(100))));
        assert_eq!(shown.get(), None, "none in the dwell after a key");
    }

    /// The transport's paint: the frame's fill only while hot, each mark
    /// in its 14 points at its middle; the seek track's thumb at the played
    /// share, the ghost's outline where a press would land; a picture
    /// fitted whole with its corners rounded.
    #[test]
    fn the_transports_pieces_paint_where_they_say() {
        let ctx = context();
        let pal = Pal::now();
        let button = Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 44.0));
        let fills = |shapes: &[egui::epaint::ClippedShape]| {
            shapes.iter().filter(|s| matches!(&s.shape, Shape::Rect(r) if r.fill == pal.raised)).count()
        };
        let rest = pass(&ctx, 0.0, Vec::new(), &mut |ui| frame_button(ui, button, Mark::Play, pal.gold, pal.gold, 2.0, false, &pal));
        let hot = pass(&ctx, 0.1, Vec::new(), &mut |ui| frame_button(ui, button, Mark::Play, pal.gold, pal.gold, 2.0, true, &pal));
        assert_eq!((fills(&rest), fills(&hot)), (0, 1), "the raised fill while hot only");
        for mark in [Mark::Prev, Mark::Next, Mark::Play, Mark::Pause, Mark::Repeat, Mark::Shuffle] {
            let shapes = pass(&ctx, 0.2, Vec::new(), &mut |ui| self::mark(ui, button.center(), mark, pal.text));
            let ink = shapes.iter().map(|s| s.shape.visual_bounding_rect()).fold(Rect::NOTHING, |a, b| a.union(b));
            assert!(ink.width() <= 16.0 && ink.height() <= 16.0, "{mark:?} in its 14 points: {ink:?}");
            assert!((ink.center() - button.center()).length() < 1.5, "{mark:?} at the middle: {ink:?}");
        }
        let track = Rect::from_min_size(pos2(20.0, 100.0), vec2(200.0, 16.0));
        let shapes = pass(&ctx, 0.3, Vec::new(), &mut |ui| seek_track(ui, track, 0.25, Some(0.75), &pal));
        let thumbs: Vec<Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Rect(r) if r.rect.width() == THUMB_W => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(thumbs.len(), 2, "the thumb and the ghost: {thumbs:?}");
        assert!((thumbs[0].center().x - (track.min.x + 50.0)).abs() < 0.5, "the thumb at a quarter: {thumbs:?}");
        assert!((thumbs[1].center().x - (track.min.x + 150.0)).abs() < 0.5, "the ghost at three quarters: {thumbs:?}");
        assert!((thumbs[0].center().y - track.center().y).abs() < 0.5);
        let image = egui::ColorImage::new([8, 4], vec![Color32::RED; 32]);
        let texture = ctx.load_texture("probe-picture", image, egui::TextureOptions::LINEAR);
        let slot = Rect::from_min_size(pos2(100.0, 100.0), vec2(64.0, 64.0));
        let mut at = Rect::NOTHING;
        let shapes = pass(&ctx, 0.4, Vec::new(), &mut |ui| at = picture_in(ui, slot, &texture, 3));
        assert_eq!(at, Rect::from_center_size(slot.center(), vec2(64.0, 32.0)), "whole, its shape kept");
        let textured = shapes.iter().any(|s| match &s.shape {
            Shape::Rect(r) => r.rect == at && r.corner_radius == CornerRadius::same(3) && r.fill_texture_id() == texture.id(),
            _ => false,
        });
        assert!(textured, "a textured rect with its corners rounded");
    }
}
