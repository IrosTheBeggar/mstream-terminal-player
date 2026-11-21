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
use unicode_segmentation::UnicodeSegmentation;

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
/// draws in the panel shares: dark; a scroll bar a slim thumb in the strong
/// edge whenever there is more than the view shows (the log's, and the
/// cells' bar, which never hides), amber under the pointer and in the hand;
/// a tooltip the canvas's, raised on a strong-edged hairline with no
/// shadow.
pub(crate) fn style_once(ctx: &egui::Context, pal: &Pal) {
    let id = Id::new("studio-styled");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_theme(egui::Theme::Dark);
    let (accent, text, ground, edge, raised) = (pal.accent, pal.text, pal.ground, pal.edge_strong, pal.raised);
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
            state.corner_radius = CornerRadius::same(1);
        }
        widgets.inactive.bg_fill = edge;
        widgets.hovered.bg_fill = accent.gamma_multiply(0.75);
        widgets.active.bg_fill = accent;
        // The thumb at rest too, never its track: egui's floating bar shows
        // nothing until the pointer comes, and draws its thumb in the text
        // colour.
        style.spacing.scroll = egui::style::ScrollStyle {
            floating_width: 4.0,
            bar_width: 6.0,
            foreground_color: false,
            dormant_handle_opacity: 1.0,
            active_handle_opacity: 1.0,
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

/// `parts` as one job, each run in its own face and colour.
pub(crate) fn run_job(parts: &[Run]) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (s, font, color) in parts {
        if !s.is_empty() {
            job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *color, ..Default::default() });
        }
    }
    job
}

/// How wide `parts` lay out in one line.
pub(crate) fn runs_width(ui: &Ui, parts: &[Run]) -> f32 {
    ui.painter().layout_job(run_job(parts)).size().x
}

/// `parts` in one line no wider than `max_w`, cut with an ellipsis where
/// they run long, anchored at `pos` by `align`. Returns where it was drawn.
pub(crate) fn runs(ui: &Ui, pos: Pos2, parts: &[Run], max_w: f32, align: Align2) -> Rect {
    let mut job = run_job(parts);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, Color32::WHITE);
    rect
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
    text(ui, row.left_center(), s, font, color, w, Align2::LEFT_CENTER)
}

/// A label's row: the small capitals' 14 points.
pub(crate) const LABEL_H: f32 = 14.0;

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
/// "DESTINATION · another drive, …"), only where some of it fits. Returns
/// where it was drawn.
pub(crate) fn label_at(ui: &Ui, pos: Pos2, s: &str, max_w: f32, pal: &Pal) -> Rect {
    let (head, hint) = match s.find('·') {
        Some(at) => (s[..at].trim_end(), &s[at..]),
        None => (s, ""),
    };
    let drawn = small_caps(ui, pos, head, pal.dim, max_w, Align2::LEFT_CENTER);
    let (from, end) = (drawn.max.x + 6.0, pos.x + max_w);
    if hint.is_empty() || end - from < 16.0 {
        return drawn;
    }
    drawn.union(text(ui, pos2(from, pos.y), hint, body(13.0), pal.dim, end - from, Align2::LEFT_CENTER))
}

/// The canvas's prose: words wrapped to a width, in one face and colour or
/// as several [`Run`]s, `line` times the first run's size apart (the
/// canvas's 1.3 by default), in at most `rows` lines with the last cut
/// with an ellipsis, a line that would end below `floor` left out whole
/// rather than cut through (as the cells leave out a row past the column's
/// end), and a `lead` mark (a list's "−") standing in a column of its own
/// with the lines wrapped beside it.
pub(crate) struct Para {
    parts: Vec<Run>,
    line: f32,
    rows: usize,
    floor: f32,
    lead: Option<String>,
}

/// The canvas's prose leading: its body's line-height.
const PARA_LINE: f32 = 1.3;

impl Para {
    pub(crate) fn new(s: &str, font: FontId, color: Color32) -> Para {
        Para::runs(&[(s.to_string(), font, color)])
    }

    pub(crate) fn runs(parts: &[Run]) -> Para {
        Para { parts: parts.to_vec(), line: PARA_LINE, rows: usize::MAX, floor: f32::INFINITY, lead: None }
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

    /// A row of the layout the column wide, as tall as its lines. Returns
    /// where they were drawn.
    pub(crate) fn show(self, ui: &mut Ui) -> Rect {
        let w = ui.available_width();
        let at = ui.cursor().min;
        let drawn = self.paint(ui, at, w);
        let (rect, _) = ui.allocate_exact_size(vec2(w, drawn.height()), Sense::hover());
        rect
    }

    /// Drawn from `at`, its top left, wrapped at `w`. Returns where.
    pub(crate) fn paint(self, ui: &Ui, at: Pos2, w: f32) -> Rect {
        let Some((_, font, color)) = self.parts.first().cloned() else { return Rect::from_min_size(at, Vec2::ZERO) };
        let line_h = font.size * self.line;
        let indent = match &self.lead {
            Some(mark) => {
                let format = TextFormat { font_id: font.clone(), color, line_height: Some(line_h), ..Default::default() };
                let galley = ui.painter().layout_job(LayoutJob::single_section(mark.clone(), format));
                let indent = galley.size().x + 8.0;
                if at.y + galley.size().y <= self.floor {
                    ui.painter().galley(at, galley, color);
                }
                indent
            }
            None => 0.0,
        };
        let mut job = LayoutJob::default();
        for (s, font, color) in &self.parts {
            if !s.is_empty() {
                job.append(s, 0.0, TextFormat { font_id: font.clone(), color: *color, line_height: Some(line_h), ..Default::default() });
            }
        }
        job.wrap = TextWrapping { max_width: (w - indent).max(1.0), max_rows: self.rows, break_anywhere: false, overflow_character: Some('…') };
        let galley = ui.painter().layout_job(job);
        let origin = at + vec2(indent, 0.0);
        let rect = Rect::from_min_size(at, vec2(w, galley.size().y));
        // The lines that end at or above the floor, whole.
        let fits = galley.rows.iter().map(|row| origin.y + row.rect().max.y).filter(|foot| *foot <= self.floor + 0.01).fold(origin.y, f32::max);
        if fits >= rect.max.y - 0.01 {
            ui.painter().galley(origin, galley, color);
        } else if fits > origin.y {
            ui.painter().with_clip_rect(rect.with_max_y(fits).intersect(ui.clip_rect())).galley(origin, galley, color);
        }
        rect
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
/// cut with an ellipsis where it runs long. Nothing at all draws nothing.
pub(crate) fn note_line(ui: &Ui, rect: Rect, busy: Option<&str>, note: Option<&(String, bool)>, pal: &Pal) {
    let (words, color) = match (busy, note) {
        (Some(busy), _) => (busy, pal.accent),
        (None, Some((note, is_err))) => (note.as_str(), if *is_err { pal.gold } else { pal.dim }),
        (None, None) => return,
    };
    text(ui, rect.left_center(), words, body(13.0), color, rect.width(), Align2::LEFT_CENTER);
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

/// A room's state line across `row`: the LED lit `lamp` at its left, then
/// `parts` in one run from `indent` points in, cut with an ellipsis, and
/// `right` (the reload's cadence) at the row's end, only where the whole
/// line fits beside it. Returns where the words were drawn.
pub(crate) fn state_line(ui: &Ui, row: Rect, lamp: Color32, indent: f32, parts: &[Run], right: Option<&Run>) -> Rect {
    let mid = row.center().y;
    led(ui, pos2(row.min.x + 3.0, mid), lamp);
    let x = row.min.x + indent;
    let mut end = row.max.x;
    if let Some((words, font, color)) = right {
        let right_w = width_of(ui, words, font.clone());
        if x + runs_width(ui, parts) + 16.0 + right_w <= row.max.x {
            text(ui, pos2(row.max.x, mid), words, font.clone(), *color, right_w + 1.0, Align2::RIGHT_CENTER);
            end = row.max.x - right_w - 16.0;
        }
    }
    runs(ui, pos2(x, mid), parts, (end - x).max(1.0), Align2::LEFT_CENTER)
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

/// A room on but with no one found yet (searching, connecting), as an
/// 18-point row of the layout: ten [`meter`] segments 5 by 10 points, 2
/// apart, none lit — never a share of anything — and `words` in the accent
/// 10 points after them.
pub(crate) fn searching(ui: &mut Ui, words: &str, pal: &Pal) {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
    let bar = meter(ui, row.left_center(), 10, 0, vec2(5.0, 10.0), 2.0, pal.accent, pal);
    let x = bar.max.x + 10.0;
    text(ui, pos2(x, row.center().y), words, body(13.0), pal.accent, (row.max.x - x).max(1.0), Align2::LEFT_CENTER);
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
/// edge and [`check`] when on, each brighter under the pointer. Alone it
/// takes no click: a page's flag column can show its state while the row
/// takes the click.
pub(crate) fn tick_box(ui: &Ui, rect: Rect, on: bool, hovered: bool, fill: Color32, pal: &Pal) {
    let edge = if on { pal.ok } else { pal.dim };
    let edge = if hovered { lift(edge, 40) } else { edge };
    ui.painter().rect_filled(rect, square(), fill);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    if on {
        check(ui, rect.center(), if hovered { lift(pal.ok, 40) } else { pal.ok });
    }
}

/// The keyboard's look on a control of a form: the raised fill and the
/// accent's ring round `rect`.
fn cursor_look(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_filled(rect, square(), pal.raised);
    ring(ui, rect, pal);
}

/// A tick at `rect` with its words, a form's choice (a library of a grant,
/// a permission): the box 8 points in on the panel's fill and `label`
/// beside it, cut to the row. The form's focus wears the keyboard's look —
/// the raised fill, the accent ring, the label semibold — and the pointer
/// raises it; the whole row takes the click (the canvas's FederationMint).
pub(crate) fn tick_row(ui: &Ui, id: Id, rect: Rect, on: bool, label: &str, focused: bool, pal: &Pal) -> Response {
    let response = hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    if focused {
        cursor_look(ui, rect, pal);
    } else if hovered {
        ui.painter().rect_filled(rect, square(), pal.raised);
    }
    let mark = Rect::from_min_size(pos2(rect.min.x + 8.0, rect.center().y - BOX / 2.0), Vec2::splat(BOX));
    tick_box(ui, mark, on, hovered, pal.panel, pal);
    let x = mark.max.x + 10.0;
    let font = if focused { strong(14.0) } else { body(14.0) };
    text(ui, pos2(x, rect.center().y), label, font, pal.text, (rect.max.x - x - 8.0).max(1.0), Align2::LEFT_CENTER);
    response
}

/// A row of a list in a modal, `h` points of the layout the column wide,
/// that takes the click: raised under the pointer, the keyboard's look
/// when `on` (its cursor), the hand over it. The page paints its words in
/// the rect; whether the pointer is on it is the response's.
pub(crate) fn list_row(ui: &mut Ui, id: Id, h: f32, on: bool, pal: &Pal) -> (Rect, Response) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let response = list_row_at(ui, id, rect, on, pal);
    (rect, response)
}

/// [`list_row`] at `rect`.
pub(crate) fn list_row_at(ui: &Ui, id: Id, rect: Rect, on: bool, pal: &Pal) -> Response {
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
/// past them either side, the label semibold.
pub(crate) fn check_line(ui: &mut Ui, id: Id, on: bool, label: &str, detail: &str, focused: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, HIT), Sense::hover());
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
    let drawn = text(ui, pos2(x, row.center().y), label, font, pal.text, row.max.x - x, Align2::LEFT_CENTER);
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
    }
}

/// A room's call to act, the column wide and 56 points tall: a hairline in
/// `tone` with `glyph` and `label` (bare of the cells' arrows) in its
/// semibold at the centre, washed in the tone under the pointer. While not
/// `live` it is dim and takes no click: its act on its way.
pub(crate) fn card(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, tone: Color32, live: bool, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 56.0), Sense::hover());
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

/// A room's add card: a [`card`] in the ok green with its room's glyph
/// (the canvas's "Add a music folder").
pub(crate) fn add_card(ui: &mut Ui, id: Id, label: &str, glyph: Glyph, pal: &Pal) -> Response {
    card(ui, id, label, glyph, pal.ok, true, pal)
}

/// A card that is one flag's tick, the column wide and 60 points tall: the
/// panel's fill, raised under the pointer with its edge brightened, a tick
/// box 24 points in, `label` semibold beside it and `hint` dim under the
/// label — the whole of it the click that flips the flag (the canvas's
/// Federation inbox).
pub(crate) fn tick_card(ui: &mut Ui, id: Id, on: bool, label: &str, hint: &str, pal: &Pal) -> Response {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
    let response = hand(ui.interact(rect, id, Sense::click()));
    let hovered = response.hovered();
    ui.painter().rect_filled(rect, square(), if hovered { pal.raised } else { pal.panel });
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, if hovered { pal.dim } else { pal.edge }), StrokeKind::Inside);
    let top = rect.min.y + 20.0;
    let mark = Rect::from_center_size(pos2(rect.min.x + 24.0, top), Vec2::splat(BOX));
    tick_box(ui, mark, on, hovered, pal.ground, pal);
    let x = mark.max.x + 12.0;
    let room = (rect.max.x - x - 16.0).max(1.0);
    text(ui, pos2(x, top), label, strong(14.0), pal.text, room, Align2::LEFT_CENTER);
    text(ui, pos2(x, top + 21.0), hint, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
    response
}

/// A modal's foot, a [`FORM_BUTTON_H`] row of the layout the column wide
/// ([`foot_at`]). The button clicked, counting `start` first.
pub(crate) fn foot(ui: &mut Ui, id: Id, start: &[(&str, bool)], end: &[(&str, bool)], note: Option<&str>, pal: &Pal) -> Option<usize> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, FORM_BUTTON_H), Sense::hover());
    foot_at(ui, id, row, start, end, note, pal)
}

/// A modal's or a page's foot across `row`: form buttons, each as wide as
/// its [`bare`] words and 10 points apart, the lit one where its flag is
/// set — `start` laid from the row's left, `end` ending at its right — and
/// with nothing at the start, `note` (a refusal) in gold at the left, cut
/// 16 points short of the first button. Button `k`, counting `start` first,
/// is `id.with(k)`. The button clicked.
pub(crate) fn foot_at(ui: &Ui, id: Id, row: Rect, start: &[(&str, bool)], end: &[(&str, bool)], note: Option<&str>, pal: &Pal) -> Option<usize> {
    let mut clicked = None;
    let mut x = row.min.x;
    for (k, (label, primary)) in start.iter().enumerate() {
        let bw = form_button_width(ui, label);
        let rect = Rect::from_min_size(pos2(x, row.min.y), vec2(bw, row.height()));
        if form_button(ui, id.with(k), rect, label, *primary, pal).clicked() {
            clicked = Some(k);
        }
        x += bw + 10.0;
    }
    let mut x = row.max.x;
    for (k, (label, primary)) in end.iter().enumerate().rev() {
        let bw = form_button_width(ui, label);
        let rect = Rect::from_min_size(pos2(x - bw, row.min.y), vec2(bw, row.height()));
        if form_button(ui, id.with(start.len() + k), rect, label, *primary, pal).clicked() {
            clicked = Some(start.len() + k);
        }
        x -= bw + 10.0;
    }
    if let Some(note) = note.filter(|_| start.is_empty()) {
        let room = x + 10.0 - 16.0 - row.min.x;
        if room > 16.0 {
            text(ui, row.left_center(), note, body(13.0), pal.gold, room, Align2::LEFT_CENTER);
        }
    }
    clicked
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
}

impl<'a> Input<'a> {
    pub(crate) fn new(value: &'a str, caret: usize) -> Self {
        Input { value, caret, focused: false, composition: "", masked: false, mono: false, enabled: true, placeholder: "", search: false }
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
/// side.
pub(crate) fn field_at(ui: &Ui, id: Id, slot: Rect, label: &str, input: &Input, pal: &Pal) -> Typed {
    label_at(ui, pos2(slot.min.x, slot.min.y + LABEL_H / 2.0), label, slot.width(), pal);
    let rect = Rect::from_min_size(pos2(slot.min.x, slot.max.y - FIELD_H), vec2(slot.width(), FIELD_H));
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
    if line.is_empty() && !input.placeholder.is_empty() {
        let galley = ui.painter().layout_no_wrap(input.placeholder.to_string(), font.clone(), pal.dim);
        clip.galley(pos2(view.min.x, rect.center().y - galley.size().y / 2.0), galley, pal.dim);
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
    let from = view.min.x + whole_glyph(&galley, offset) - offset;
    clip.with_clip_rect(clip.clip_rect().with_min_x(from.max(clip.clip_rect().min.x))).galley(origin, galley, ink);

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
pub(crate) const COL_GAP: f32 = 16.0;
pub(crate) const CELL_PAD: f32 = 10.0;
/// How far a room's table's first row stands under its top: the 30-point
/// headings and the 4 points under them.
pub(crate) const TABLE_HEAD: f32 = 34.0;

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

//// A table under its headings, its rows scrolled by egui and drawn one at
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
    /// row on screen.
    pub(crate) fn show(self, ui: &mut Ui, pal: &Pal, mut draw_row: impl FnMut(&Ui, usize, &RowCtx)) -> TableShown {
        let Table { id, cols, len, cursor, look, row_h, trail, clickable, reveal, empty, headless, height, content } = self;
        let w = ui.available_width();
        if !headless {
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
        let view_h = height.unwrap_or_else(|| ui.available_height());
        let view_h = if view_h.is_finite() { view_h.max(0.0) } else { len as f32 * row_h };
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
        let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
        if let Some(h) = height {
            scroll = scroll.max_height(h).min_scrolled_height(h);
        }
        if moved {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        let mut shown = TableShown::default();
        let sense = if clickable { Sense::click() } else { Sense::hover() };
        let output = scroll.show_rows(ui, row_h, len, |ui, range| {
            shown.last_on_screen = range.end.saturating_sub(1);
            for i in range {
                let (full, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                let rect = Rect::from_min_max(full.min, pos2((full.max.x - trail).max(full.min.x), full.max.y));
                let trail_rect = Rect::from_min_max(pos2(rect.max.x, full.min.y), full.max);
                let response = ui.interact(rect, id.with("row").with(i), sense);
                // The Stats page's controls show no hand yet, its rows
                // among them; the rooms' do.
                let response = if clickable && look == Look::Ring { hand(response) } else { response };
                let selected = cursor == Some(i);
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
            led(ui, pos2(x + 3.0, mid), lamp);
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
        let _ = ui.interact(rect, id, Sense::hover()).on_hover_text(tip);
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
        let _ = ui.interact(rect, id, Sense::hover()).on_hover_text(egui::RichText::new(path).font(font));
    }
    drawn
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

// ── Modals ────────────────────────────────────────────────────────────────

/// A modal window's padding, the room under its title, and the title row.
const MODAL_PAD: f32 = 24.0;
const MODAL_GAP: f32 = 20.0;
const TITLE_H: f32 = 20.0;

/// What a modal's pass gives back.
pub(crate) struct ModalShown<R> {
    /// The close button was clicked: the page cancels, as on Esc.
    pub closed: bool,
    /// The modal came up on this pass: it was not drawn on the last one.
    pub opened: bool,
    /// What the body's closure returned.
    pub inner: R,
}

/// Where a modal's body stood on this pass, for [`reveal`]: the top of its
/// content and its view, in the screen's points, and whether the modal
/// came up on this pass.
#[derive(Clone, Copy)]
struct BodyView {
    origin: f32,
    view: Rect,
    opened: bool,
}

/// A modal over `over` only — the room's area, never the window's — as the
/// canvas draws one: the scrim across `over`, and centred in it a window
/// `width` wide (less where `over` is narrower) on the panel's fill in a
/// strong-edged hairline with the canvas's shadow, 24 points in: `title`
/// in 16-point semibold (wrapped where it runs long, never cut short of its
/// question) with a close button at its right, then the body laid out
/// top-down under it, scrolled where it is taller than `over` allows.
///
/// The window takes the height its body asked for on the last pass (egui
/// runs the pass again when that changed, so it never shows misplaced). A
/// press anywhere in `over` outside the window is swallowed: what is under
/// the scrim is inert. A modal comes up with its body at its top, as the
/// cells draw a modal afresh each time, wherever it was scrolled to when it
/// was last up.
pub(crate) struct Dialog<'a> {
    id: Id,
    over: Rect,
    width: f32,
    title: &'a str,
    tone: Option<Color32>,
    close: bool,
    hold: Option<f32>,
}

impl<'a> Dialog<'a> {
    pub(crate) fn new(id: Id, over: Rect, width: f32, title: &'a str) -> Self {
        Dialog { id, over, width, title, tone: None, close: true, hold: None }
    }

    /// The title and the frame in `color`.
    pub(crate) fn tone(self, color: Color32) -> Self {
        Dialog { tone: Some(color), ..self }
    }

    /// With no close button: what it asks is answered with its own
    /// buttons and keys.
    pub(crate) fn closable(self, close: bool) -> Self {
        Dialog { close, ..self }
    }

    /// A warning gate, as the cells draw every one: its title and frame in
    /// the warning's gold, and no close, since a gate is answered — Esc and
    /// the safe button keep — never dismissed.
    pub(crate) fn gate(self, pal: &Pal) -> Self {
        self.tone(pal.gold).closable(false)
    }

    /// The window's top held where a window with a body `body_h` tall would
    /// stand centred, its foot following its body: a list under a field
    /// that grows and shrinks moves nothing above it (the cells anchor
    /// their path entry so).
    pub(crate) fn hold(self, body_h: f32) -> Self {
        Dialog { hold: Some(body_h), ..self }
    }

    pub(crate) fn show<R>(self, ui: &Ui, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<R> {
        let Dialog { id, over, width, title, tone, close, hold } = self;
        let ctx = ui.ctx().clone();
        let width = width.min(over.width() - 32.0).max(over.width().min(160.0));
        let inner_w = (width - 2.0 * MODAL_PAD).max(1.0);
        // The title wrapped where it runs long, its first line standing
        // where a one-line title does.
        let title_color = tone.unwrap_or(pal.text);
        let title_w = (inner_w - if close { 40.0 } else { 0.0 }).max(1.0);
        let format = TextFormat { font_id: strong(16.0), color: title_color, ..Default::default() };
        let mut job = LayoutJob::single_section(title.to_string(), format);
        job.wrap = TextWrapping { max_width: title_w, max_rows: 3, break_anywhere: false, overflow_character: Some('…') };
        let galley = ui.painter().layout_job(job);
        let lines = galley.rows.len().max(1);
        let line_h = galley.size().y / lines as f32;
        let title_h = TITLE_H + (lines - 1) as f32 * line_h;
        let fixed = 2.0 * MODAL_PAD + title_h + MODAL_GAP;

        let max_h = (over.height() - 32.0).max(fixed);
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
        // Up on this pass when it was not drawn on the last.
        let now = ctx.cumulative_pass_nr();
        let drawn_key = id.with("drawn");
        let opened = ctx.data(|d| d.get_temp::<u64>(drawn_key)).is_none_or(|last| last + 1 != now);
        ctx.data_mut(|d| d.insert_temp(drawn_key, now));
        let reveal_to: Option<f32> = ctx.data_mut(|d| d.remove_temp(id.with("reveal-to")));

        // Up at once, as the canvas's and the cells' modals are: egui fades an
        // area in by default, over frames the panel would have to run for it.
        let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(over.min).constrain(false).fade_in(false);
        let shown = area.show(&ctx, |ui| {
            let _scrim = ui.allocate_rect(over, Sense::click_and_drag());
            ui.painter().rect_filled(over, 0.0, SCRIM);
            let shadow = Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(115) };
            ui.painter().add(shadow.as_shape(window, square()));
            ui.painter().rect_filled(window, square(), pal.panel);
            ui.painter().rect_stroke(window, square(), Stroke::new(1.0, tone.unwrap_or(pal.edge_strong)), StrokeKind::Inside);
            let inner = window.shrink(MODAL_PAD);
            let title_row = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
            // The canvas pulls the close button 6 points past the padding.
            let closed = close && {
                let at = pos2(inner.max.x - 8.0, title_row.center().y);
                icon_button(ui, id.with("close"), at, Icon::Close, pal.dim, pal).clicked()
            };
            ui.painter().galley(pos2(inner.min.x, title_row.center().y - line_h / 2.0), galley, title_color);
            let budget = (max_h - fixed).max(0.0);
            let body_rect = Rect::from_min_size(pos2(inner.min.x, inner.min.y + title_h + MODAL_GAP), vec2(inner.width(), budget));
            let mut child =
                ui.new_child(UiBuilder::new().id_salt(id.with("body")).max_rect(body_rect).layout(Layout::top_down(Align::Min)));
            let mut scroll = ScrollArea::vertical().id_salt(id.with("scroll")).max_height(budget).auto_shrink([false, true]);
            if opened {
                scroll = scroll.vertical_scroll_offset(0.0);
            } else if let Some(to) = reveal_to {
                scroll = scroll.vertical_scroll_offset(to);
            }
            let out = scroll.show(&mut child, |ui| {
                let view = BodyView { origin: ui.max_rect().min.y, view: ui.clip_rect(), opened };
                ui.ctx().data_mut(|d| d.insert_temp(id.with("view"), view));
                body(ui)
            });
            let wanted = fixed + out.content_size.y.min(budget);
            (closed, out.inner, wanted.min(max_h))
        });
        let (closed, inner, wanted) = shown.inner;
        if known.is_none_or(|h| (h - wanted).abs() > 0.5) {
            ctx.data_mut(|d| d.insert_temp(size_key, wanted));
            ctx.request_discard("a modal took its body's height");
        }
        ModalShown { closed, opened, inner }
    }
}

/// A [`Dialog`] with nothing but its title: the plain modal, with its close.
pub(crate) fn modal<R>(ui: &Ui, id: Id, over: Rect, width: f32, title: &str, pal: &Pal, body: impl FnOnce(&mut Ui) -> R) -> ModalShown<R> {
    Dialog::new(id, over, width, title).show(ui, pal, body)
}

/// The keyboard's control in the body of the modal `modal` at `rect`,
/// brought just into the body's view, at once, on the pass the keyboard
/// moved to it (`at`, the page's own number for it: a field's, a list's
/// cursor) or the modal came up or changed height — never after, so the
/// wheel keeps the body where it puts it. The move lands within the frame:
/// the pass runs again with the body where it goes, or where egui runs no
/// more passes this frame, the next frame is asked for.
pub(crate) fn reveal(ui: &Ui, modal: Id, at: usize, rect: Rect) {
    let ctx = ui.ctx();
    let Some(body) = ctx.data(|d| d.get_temp::<BodyView>(modal.with("view"))) else { return };
    let h = body.view.height();
    let key = modal.with("revealed");
    let last: Option<(usize, f32)> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, (at, h)));
    if !body.opened && last.is_some_and(|(was, h_was)| was == at && (h_was - h).abs() < 0.5) {
        return;
    }
    let offset = body.view.min.y - body.origin;
    let (top, bottom) = (rect.min.y - body.origin, rect.max.y - body.origin);
    let to = if top < offset {
        top
    } else if bottom > offset + h {
        (bottom - h).min(top)
    } else {
        return;
    };
    ctx.data_mut(|d| d.insert_temp(modal.with("reveal-to"), to.max(0.0)));
    ctx.request_discard("a modal brought its keyboard's control into view");
    if !ctx.will_discard() {
        ctx.request_repaint();
    }
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
            let shown = modal(ui, id, over, 480.0, "Add a user", &pal, |ui| {
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
        let opened = Cell::new(false);
        // Where the last pass drew rows 0 and 29 (egui's own record of a
        // widget can be a discarded pass's).
        let first = Cell::new(Rect::NOTHING);
        let last = Cell::new(Rect::NOTHING);
        let mut run = |ui: &mut Ui| {
            if !up.get() {
                return;
            }
            let pal = Pal::now();
            let shown = Dialog::new(id, over, 400.0, "Rows").show(ui, &pal, |ui| {
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
            opened.set(shown.opened);
        };
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let _ = pass(&ctx, size, Vec::new(), &mut run);
        let home = first.get().min.y;
        assert!(!opened.get(), "up since the pass before");
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
        assert!(opened.get(), "it came up on this pass");
        assert!((first.get().min.y - home).abs() < 0.5, "at its top: {:?} for {home}", first.get());
    }

    #[test]
    fn a_gate_has_no_close_and_wears_the_warnings_gold_and_its_title_wraps_whole() {
        let ctx = context();
        let size = vec2(600.0, 400.0);
        let over = Rect::from_min_size(Pos2::ZERO, size);
        let title = "Remove Boards of Canada - Music Has the Right to Children (1998) [FLAC] from Transmission?";
        let out = pass(&ctx, size, Vec::new(), &mut |ui| {
            let pal = Pal::now();
            let _ = Dialog::new(Id::new("probe-gate"), over, 420.0, title).gate(&pal).show(ui, &pal, |ui| ui.add_space(40.0));
        });
        assert!(ctx.read_response(Id::new("probe-gate").with("close")).is_none(), "no close");
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
}
