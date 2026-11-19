//! The stats page drawn by egui — the egui spike's first page. The same
//! [`Page`] as the cells page above (its state, its worker, its keys);
//! only the drawing is new, in pixels, inside the window's panel
//! (src/gui/window/panel.rs). Every click is one of the page's own [`Act`]s,
//! handed to the page after the pass, so the keyboard and the pointer stay
//! one model: ←→ still change the tab, `p` still drops the period list,
//! `x` still opens the forget gate, and egui draws whatever the page holds.
//! What the cells page decides in words — the tiles, the series and their
//! axes, the periods, a row's note — is called here, never copied.
//!
//! The look is "Studio", chosen from the style canvas (2026-11-19): a
//! mixing desk's — square corners, hairline panels, labels in spaced
//! capitals, figures in the grid's own monospace (Hack, already in the
//! binary), and LED meters for bars, each a column of small lit segments
//! in amber. The ground is the kit's, so the panel meets the cells of the
//! top bar with no seam.
//!
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::{CornerRadius, Shape, StrokeKind};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, FontFamily, FontId, Id, Layout, Pos2, Rect, Response, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, Vec2, pos2, vec2,
};
use ratatui::style::Color;

use super::*;
use crate::gui::window::panel::STRONG;

/// The page's margin inside the panel, in points.
const PAD: Vec2 = vec2(22.0, 14.0);
/// The note line's height along the panel's foot.
const NOTE_H: f32 = 30.0;
const GAP: f32 = 10.0;
const ROW_H: f32 = 30.0;
/// The desk's corners: square, softened by a point or two.
const RADIUS: u8 = 3;
/// An LED segment's height and the dark gap above it, in points.
const LED: f32 = 4.0;
const LED_GAP: f32 = 2.0;
/// Spaced capitals: this much extra room per letter, as a share of the size.
const TRACK: f32 = 0.12;

/// The desk's palette. The ground, the body text's ok green and the gold
/// are the kit's (the window pins truecolour, so every role is an RGB; the
/// fallbacks are the canvas hexes); the rest is Studio's own.
struct Pal {
    ground: Color32,
    panel: Color32,
    raised: Color32,
    edge: Color32,
    edge_strong: Color32,
    text: Color32,
    dim: Color32,
    accent: Color32,
    accent_hot: Color32,
    on_accent: Color32,
    unlit: Color32,
    down: Color32,
    up: Color32,
    gold: Color32,
    ok: Color32,
}

impl Pal {
    fn now() -> Pal {
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
            dim: Color32::from_rgb(0x9a, 0x9c, 0x98),
            accent: Color32::from_rgb(0xf2, 0xa9, 0x3b),
            accent_hot: Color32::from_rgb(0xff, 0xcf, 0x7a),
            on_accent: Color32::from_rgb(0x1a, 0x12, 0x06),
            unlit: lift(ground, 18),
            down: Color32::from_rgb(0xff, 0x8a, 0x6b),
            up: rgb(th.ok, Color32::from_rgb(0x8f, 0xd1, 0x8b)),
            gold: rgb(th.gold, Color32::from_rgb(0xe0, 0xb4, 0x5a)),
            ok: rgb(th.ok, Color32::from_rgb(0x8f, 0xd1, 0x8b)),
        }
    }
}

/// `c` lifted toward white by `by` of 255, opaque.
fn lift(c: Color32, by: u8) -> Color32 {
    let up = |v: u8| v.saturating_add(by);
    Color32::from_rgb(up(c.r()), up(c.g()), up(c.b()))
}

/// Names and prose: the system's UI face.
fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Labels, set in spaced capitals ([`caps`]): the UI face's semibold.
fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Figures: the grid's monospace.
fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn square() -> CornerRadius {
    CornerRadius::same(RADIUS)
}

/// One pass of the page over the whole of `ui` — the panel's region.
pub(crate) fn show(ui: &mut Ui, page: &mut Page) {
    let pal = Pal::now();
    style_once(ui.ctx(), &pal);
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, pal.ground);
    let column = full.shrink2(PAD);
    let body_rect = Rect::from_min_max(column.min, pos2(column.max.x, column.max.y - NOTE_H));
    let note_rect = Rect::from_min_max(pos2(column.min.x, column.max.y - NOTE_H), column.max);

    let mut acts: Vec<Act> = Vec::new();
    let mut ui_body = ui.new_child(UiBuilder::new().max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    ui_body.set_clip_rect(body_rect.expand(1.0));
    let period_at = header(&mut ui_body, page, &pal, &mut acts);
    ui_body.add_space(12.0);
    if page.no_api {
        sentence(&mut ui_body, &t!("sta.noapi_state"), &t!("sta.noapi_detail"), &pal);
    } else {
        match page.tab {
            Tab::Overview => overview(&mut ui_body, page, &pal),
            Tab::Top => top(&mut ui_body, page, &pal, &mut acts),
            Tab::Recent => recent(&mut ui_body, page, &pal, &mut acts),
        }
    }
    note_line(ui, note_rect, page, &pal, &mut acts);

    // A modal makes the page beneath inert: what was clicked under it
    // this pass is dropped, as the cells page drops its registrations.
    let modal = page.modal.clone();
    if !matches!(modal, Modal::None) {
        acts.clear();
    }
    match modal {
        Modal::None => {}
        Modal::Period(cursor) => period_list(ui, page, &pal, full, period_at, cursor, &mut acts),
        Modal::Forget(row) => forget(ui, page, &pal, full, row, &mut acts),
    }
    for act in acts {
        let _ = page.act(act);
    }
}

/// egui's look for the page, set once: dark, amber on the scroll bars.
fn style_once(ctx: &egui::Context, pal: &Pal) {
    let id = Id::new("stats-egui-styled");
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

// ── Painting helpers ──────────────────────────────────────────────────────

/// `s` in one line no wider than `max_w`, an ellipsis where it is cut,
/// `spacing` points between letters, anchored at `pos` by `align`.
/// Returns where it was drawn.
#[allow(clippy::too_many_arguments)]
fn spaced(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, spacing: f32, max_w: f32, align: Align2) -> Rect {
    let format = TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() };
    let mut job = LayoutJob::single_section(s.to_string(), format);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// `s` in one line, as [`spaced`] with no extra room between letters.
fn text(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, s, font, color, 0.0, max_w, align)
}

/// A label in the desk's spaced capitals.
fn caps(ui: &Ui, pos: Pos2, s: &str, size: f32, color: Color32, max_w: f32, align: Align2) -> Rect {
    spaced(ui, pos, &s.to_uppercase(), strong(size), color, size * TRACK, max_w, align)
}

/// How wide [`caps`] draws `s` at `size`.
fn caps_width(ui: &Ui, s: &str, size: f32) -> f32 {
    let format =
        TextFormat { font_id: strong(size), color: Color32::WHITE, extra_letter_spacing: size * TRACK, ..Default::default() };
    ui.painter().layout_job(LayoutJob::single_section(s.to_uppercase(), format)).size().x
}

/// How wide `s` is in `font`.
fn width_of(ui: &Ui, s: &str, font: FontId) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), font, Color32::WHITE).size().x
}

/// A panel of the desk: the lifted ground and its hairline.
fn panel(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_filled(rect, square(), pal.panel);
    ui.painter().rect_stroke(rect, square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
}

/// How [`segmented`] draws: the chosen segment lit amber, or only marked
/// by an amber rule along its foot.
#[derive(Clone, Copy, PartialEq)]
enum Seg {
    Lit,
    Marked,
}

const SEG_H: f32 = 30.0;
const SEG_SIZE: f32 = 12.5;

/// How wide [`segmented`] draws `labels`.
fn segmented_width(ui: &Ui, labels: &[String]) -> f32 {
    labels.iter().map(|l| caps_width(ui, l, SEG_SIZE) + 30.0).sum::<f32>() + 2.0
}

/// A row of square buttons joined by hairlines at `left`, centred on
/// `mid_y`: one per label in spaced capitals, `chosen` lit (or marked).
/// Returns the control's rect and the segment clicked, if any.
#[allow(clippy::too_many_arguments)]
fn segmented(ui: &Ui, id: Id, left: f32, mid_y: f32, labels: &[String], chosen: usize, kind: Seg, pal: &Pal) -> (Rect, Option<usize>) {
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

/// A chevron, two strokes, centred on `at`, pointing down (or up).
fn chevron(ui: &Ui, at: Pos2, color: Color32, up: bool) {
    let (dx, dy) = (4.0, if up { -2.5 } else { 2.5 });
    let stroke = Stroke::new(1.5, color);
    ui.painter().line_segment([at + vec2(-dx, -dy), at + vec2(0.0, dy)], stroke);
    ui.painter().line_segment([at + vec2(0.0, dy), at + vec2(dx, -dy)], stroke);
}

/// A small solid triangle at `at`, pointing down (a fall) or up (a rise).
fn arrow(ui: &Ui, at: Pos2, color: Color32, down: bool) {
    let (w, h) = (4.0, 3.5);
    let points = if down {
        vec![at + vec2(-w, -h), at + vec2(w, -h), at + vec2(0.0, h)]
    } else {
        vec![at + vec2(-w, h), at + vec2(w, h), at + vec2(0.0, -h)]
    };
    ui.painter().add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// A square button at `rect` in spaced capitals: lit amber when
/// `primary`, an outline in `tone` otherwise.
fn button(ui: &Ui, id: Id, rect: Rect, label: &str, primary: bool, tone: Color32, pal: &Pal) -> Response {
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

/// One gold sentence for a state with nothing else to show.
fn sentence(ui: &mut Ui, head: &str, tail: &str, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let head_rect = text(ui, rect.left_center(), head, strong(15.0), pal.gold, w, Align2::LEFT_CENTER);
    text(ui, pos2(head_rect.max.x, rect.center().y), tail, body(15.0), pal.gold, rect.max.x - head_rect.max.x, Align2::LEFT_CENTER);
}

/// The body's own spinner while the first numbers are on their way.
fn loading(ui: &mut Ui, page: &Page, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 60.0), Sense::hover());
    if page.busy.is_some() || page.in_flight {
        let spin = Rect::from_center_size(rect.center() - vec2(0.0, 8.0), vec2(22.0, 22.0));
        ui.put(spin, egui::Spinner::new().size(22.0).color(pal.accent));
    }
}

/// An LED meter standing on `base`, centred on `cx`: `lit` segments of
/// `color` from the bottom, each [`LED`] tall with a [`LED_GAP`] above it.
fn led_column(ui: &Ui, cx: f32, base: f32, width: f32, lit: usize, color: Color32) {
    for k in 0..lit {
        let bottom = base - k as f32 * (LED + LED_GAP);
        let seg = Rect::from_min_max(pos2(cx - width / 2.0, bottom - LED), pos2(cx + width / 2.0, bottom));
        ui.painter().rect_filled(seg, 0.0, color);
    }
}

/// A horizontal LED meter across `track`: `share` of its segments lit.
fn led_row(ui: &Ui, track: Rect, share: f32, lit_color: Color32, unlit: Color32) {
    let step = LED + LED_GAP - 1.0;
    let count = ((track.width() + LED_GAP - 1.0) / step).floor().max(1.0) as usize;
    let lit = if share > 0.0 { ((share * count as f32).round() as usize).clamp(1, count) } else { 0 };
    for k in 0..count {
        let x = track.min.x + k as f32 * step;
        let seg = Rect::from_min_size(pos2(x, track.min.y), vec2(LED - 1.0, track.height()));
        ui.painter().rect_filled(seg, 0.0, if k < lit { lit_color } else { unlit });
    }
}

// ── The header: tabs, the tab's note, the origin, the period ──────────────

/// The tabs on the left; on the right the tab's note, the origin (on the
/// Overview while the log holds peer plays) and the period, a box that
/// drops the list. Returns where the period's box is, for the list.
fn header(ui: &mut Ui, page: &Page, pal: &Pal, acts: &mut Vec<Act>) -> Option<Rect> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
    let labels: Vec<String> = Tab::ALL.iter().map(|t| t.name()).collect();
    let chosen = Tab::ALL.iter().position(|t| *t == page.tab).unwrap_or(0);
    let (tabs, clicked) = segmented(ui, Id::new("stats-tabs"), row.min.x, row.center().y, &labels, chosen, Seg::Lit, pal);
    if let Some(i) = clicked {
        acts.push(Act::Tab(Tab::ALL[i]));
    }
    let control = page.data.is_some() && !page.no_api && !page.no_plays_ever();
    let mut right = row.max.x;
    let mut period_at = None;
    if control {
        let name = page.period.name.to_uppercase();
        let box_w = width_of(ui, &name, mono(13.0)) + 44.0;
        let at = Rect::from_min_size(pos2(right - box_w, row.center().y - SEG_H / 2.0), vec2(box_w, SEG_H));
        let response = ui.interact(at, Id::new("stats-period"), Sense::click());
        let open = matches!(page.modal, Modal::Period(_));
        let fill = if open || response.hovered() { pal.raised } else { pal.panel };
        ui.painter().rect_filled(at, square(), fill);
        let edge = if open { pal.accent } else { pal.edge_strong };
        ui.painter().rect_stroke(at, square(), Stroke::new(1.0, edge), StrokeKind::Inside);
        text(ui, pos2(at.min.x + 12.0, at.center().y), &name, mono(13.0), pal.text, box_w - 36.0, Align2::LEFT_CENTER);
        chevron(ui, pos2(at.max.x - 15.0, at.center().y), if open { pal.accent } else { pal.dim }, open);
        if response.clicked() {
            acts.push(Act::PeriodList);
        }
        period_at = Some(at);
        right = at.min.x - 12.0;
    }
    if control && page.tab == Tab::Overview && page.peer_plays() {
        let options: Vec<String> = Origin::ALL.iter().map(|o| o.label()).collect();
        let seg_w = segmented_width(ui, &options);
        let label = t!("sta.ctl_origin").to_string();
        let label_w = caps_width(ui, &label, 11.5);
        if right - seg_w - label_w - 10.0 > tabs.max.x + 24.0 {
            let (seg, clicked) = segmented(
                ui,
                Id::new("stats-origin"),
                right - seg_w,
                row.center().y,
                &options,
                page.origin.index(),
                Seg::Marked,
                pal,
            );
            if let Some(i) = clicked {
                acts.push(Act::Origin(i));
            }
            caps(ui, pos2(seg.min.x - 10.0, row.center().y), &label, 11.5, pal.dim, label_w + 1.0, Align2::RIGHT_CENTER);
            right = seg.min.x - label_w - 22.0;
        }
    }
    let mut parts: Vec<String> = page.tab.note().into_iter().collect();
    if page.tab == Tab::Recent && page.zone.is_none() {
        parts.push(t!("sta.zone_utc").to_string());
    }
    if !parts.is_empty() {
        let room = right - tabs.max.x - 16.0;
        if room > 40.0 {
            text(ui, pos2(right, row.center().y), &parts.join(" · "), mono(12.0), pal.dim, room, Align2::RIGHT_CENTER);
        }
    }
    period_at
}

// ── Overview ──────────────────────────────────────────────────────────────

fn overview(ui: &mut Ui, page: &Page, pal: &Pal) {
    let Some(data) = page.data.as_ref() else {
        loading(ui, page, pal);
        return;
    };
    let w = ui.available_width();
    if page.no_plays_ever() {
        sentence(ui, &t!("sta.empty_state"), &t!("sta.empty_state_detail"), pal);
        ui.add_space(12.0);
        for key in ["sta.empty_1", "sta.empty_2"] {
            let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
            text(ui, rect.left_center(), &t!(key), body(15.0), pal.text, w, Align2::LEFT_CENTER);
        }
        ui.add_space(12.0);
        let zone = if page.zone.is_some() { t!("sta.zone_local") } else { t!("sta.zone_utc") };
        let (rect, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
        text(ui, rect.left_center(), &t!("sta.empty_prov", zone = zone), mono(12.5), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }
    if data.summary.events == 0 {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        let title = t!("sta.nothing_title", period = page.shown.name.clone());
        caps(ui, rect.left_center(), &title, 15.0, pal.text, w, Align2::LEFT_CENTER);
        let begins = data
            .periods
            .as_ref()
            .and_then(|p| p.earliest.as_deref())
            .and_then(iso_unix)
            .map(|t| date_long(t, page.offset_at(t)));
        let hint = match begins {
            Some(date) => t!("sta.nothing_hint", date = date).to_string(),
            None => t!("sta.nothing_hint_plain").to_string(),
        };
        let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        text(ui, rect.left_center(), &hint, body(14.0), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }

    // The six readouts: one block of two rows of three, joined by hairlines.
    let versus = versus_label(&page.shown, &page.options);
    let tiles = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
    let cell_h = 82.0;
    let (block, _) = ui.allocate_exact_size(vec2(w, 2.0 * cell_h + 3.0), Sense::hover());
    ui.painter().rect_filled(block, square(), pal.edge);
    let cell_w = (w - 4.0) / 3.0;
    for (i, (value, label, detail)) in tiles.iter().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        let rect = Rect::from_min_size(
            pos2(block.min.x + 1.0 + col * (cell_w + 1.0), block.min.y + 1.0 + row * (cell_h + 1.0)),
            vec2(cell_w, cell_h),
        );
        ui.painter().rect_filled(rect, CornerRadius::same(2), pal.panel);
        let inner = rect.shrink2(vec2(14.0, 9.0));
        caps(ui, inner.min, label, 11.5, pal.dim, inner.width(), Align2::LEFT_TOP);
        let figure = text(ui, inner.min + vec2(0.0, 17.0), value, mono(24.0), pal.text, inner.width(), Align2::LEFT_TOP);
        // A comparison's sign becomes the desk's arrow, in the words' own
        // tone; everything else is left as the page words it.
        let (sign, rest) = match detail.chars().next() {
            Some('+') => (Some(false), &detail['+'.len_utf8()..]),
            Some('−') => (Some(true), &detail['−'.len_utf8()..]),
            Some('-') => (Some(true), &detail['-'.len_utf8()..]),
            _ => (None, detail.as_str()),
        };
        // Under the figure as laid out: Hack's line is taller than its
        // capitals, so a line placed from the cell's foot sat on them.
        let foot = pos2(inner.min.x, figure.max.y + 1.0);
        match sign {
            Some(down) => {
                let tone = if down { pal.down } else { pal.up };
                arrow(ui, foot + vec2(4.0, 8.0), tone, down);
                text(ui, foot + vec2(13.0, 0.0), rest.trim_start(), mono(12.0), tone, inner.width() - 13.0, Align2::LEFT_TOP);
            }
            None => {
                text(ui, foot, rest, mono(12.0), pal.dim, inner.width(), Align2::LEFT_TOP);
            }
        }
    }
    ui.add_space(GAP);

    // The meters share what is left: the period's days (or weeks, or
    // months) and the hours — stacked when the window is tall, the hours
    // with where the tracks live beside them; side by side when it is not.
    // A few points short of the foot, so the panels clear the note's rule.
    let avail = ui.available_height() - 8.0;
    let min_h = 120.0;
    if avail < 100.0 {
        return;
    }
    let hours = hour_series(&data.hours);
    let hour_labels: Vec<(usize, String)> = [0, 6, 12, 18, 23].iter().map(|h| (*h, format!("{h:02}"))).collect();
    let note = hours_note(&data.summary).unwrap_or_default();
    let tip = |i: usize, v: u64| format!("{} · {}", hour_label(i as i32), plays_words(v));
    if avail < 2.0 * min_h + GAP {
        let (band, _) = ui.allocate_exact_size(vec2(w, avail.min(300.0)), Sense::hover());
        let days_w = (w - GAP) * 0.58;
        calendar_chart(ui, Rect::from_min_size(band.min, vec2(days_w, band.height())), page, data, pal);
        let hours_rect = Rect::from_min_max(pos2(band.min.x + days_w + GAP, band.min.y), band.max);
        meter_chart(ui, Id::new("stats-hours"), hours_rect, &t!("sta.chart_hours"), &note, &hours, &hour_labels, &tip, pal);
        return;
    }
    let chart_h = ((avail - GAP) / 2.0).min(300.0);
    let (band, _) = ui.allocate_exact_size(vec2(w, chart_h), Sense::hover());
    calendar_chart(ui, band, page, data, pal);
    ui.add_space(GAP);
    let (band, _) = ui.allocate_exact_size(vec2(w, chart_h), Sense::hover());
    let split = page.peer_plays();
    let hours_w = if split { (w - GAP) * 0.6 } else { w };
    let hours_rect = Rect::from_min_size(band.min, vec2(hours_w, chart_h));
    meter_chart(ui, Id::new("stats-hours"), hours_rect, &t!("sta.chart_hours"), &note, &hours, &hour_labels, &tip, pal);
    if split {
        let rect = Rect::from_min_max(pos2(hours_rect.max.x + GAP, band.min.y), band.max);
        origins_panel(ui, rect, data, pal);
    }
}

/// Plays per day, week or month across the shown period: a meter a
/// bucket, buckets folded together only when a meter would be narrower
/// than three points.
fn calendar_chart(ui: &Ui, rect: Rect, page: &Page, data: &Loaded, pal: &Pal) {
    let from = match (page.shown.period.as_str(), data.periods.as_ref().and_then(|p| p.earliest.as_deref())) {
        ("all", Some(earliest)) => earliest.to_string(),
        _ => data.summary.period.from.clone(),
    };
    let (keys, values, bucket) = calendar_series(&data.series, &from, &data.summary.period.to, |t| page.offset_at(t));
    let plot_w = rect.width() - 2.0 * 14.0 - GUTTER;
    let fold = values.len().div_ceil((plot_w / 3.0).max(1.0) as usize).max(1);
    let folded: Vec<u64> = values.chunks(fold).map(|c| c.iter().sum()).collect();
    let title = match (bucket, fold) {
        ("day", 1) => t!("sta.chart_days").to_string(),
        ("day", n) => t!("sta.chart_days_folded", n = n).to_string(),
        ("week", _) => t!("sta.chart_weeks").to_string(),
        _ => t!("sta.chart_months").to_string(),
    };
    let labels: Vec<(usize, String)> = match bucket {
        "day" => day_axis(&keys, fold),
        "week" => week_axis(&keys, fold),
        _ => month_axis(&keys, fold),
    };
    let name = |key: &str| -> String {
        match bucket {
            "month" => {
                let m: u32 = key.get(5..7).and_then(|m| m.parse().ok()).unwrap_or(1);
                format!("{} {}", month_name(m, "sta.months_long"), key.get(0..4).unwrap_or(""))
            }
            _ => day_key_label(key),
        }
    };
    let tip = |i: usize, v: u64| {
        let first = keys.get(i * fold).map(|k| name(k)).unwrap_or_default();
        let last = keys.get(((i + 1) * fold).min(keys.len()).saturating_sub(1)).map(|k| name(k)).unwrap_or_default();
        let when = if fold > 1 && first != last { format!("{first} – {last}") } else { first };
        format!("{when} · {}", plays_words(v))
    };
    meter_chart(ui, Id::new("stats-calendar"), rect, &title, &days_note(&data.summary), &folded, &labels, &tip, pal);
}

/// The y axis's gutter, in points.
const GUTTER: f32 = 30.0;

/// A rounded ceiling for an axis: 1, 2 or 5 times a power of ten.
fn nice(v: u64) -> u64 {
    let v = v.max(1);
    let mut p = 1u64;
    while p.saturating_mul(10) <= v {
        p *= 10;
    }
    [1, 2, 5, 10].iter().map(|m| m * p).find(|n| *n >= v).unwrap_or(10 * p)
}

/// A meter chart on a panel: the title in spaced capitals and its note in
/// figures on the top line, dashed rules at the axis's half and top, a
/// column of LED segments per value with its label under it, and the
/// column under the pointer lit brighter with its value in a tip.
#[allow(clippy::too_many_arguments)]
fn meter_chart(
    ui: &Ui,
    id: Id,
    rect: Rect,
    title: &str,
    note: &str,
    values: &[u64],
    labels: &[(usize, String)],
    tip: &dyn Fn(usize, u64) -> String,
    pal: &Pal,
) {
    panel(ui, rect, pal);
    let inner = rect.shrink2(vec2(14.0, 11.0));
    let title_rect = caps(ui, inner.min, title, 12.0, pal.text, inner.width(), Align2::LEFT_TOP);
    if !note.is_empty() {
        let room = inner.max.x - title_rect.max.x - 16.0;
        if room > 60.0 {
            text(ui, pos2(inner.max.x, inner.min.y), note, mono(11.5), pal.dim, room, Align2::RIGHT_TOP);
        }
    }
    let plot = Rect::from_min_max(pos2(inner.min.x + GUTTER, inner.min.y + 26.0), pos2(inner.max.x, inner.max.y - 18.0));
    if plot.height() < 20.0 || values.is_empty() {
        return;
    }
    // Whole segments only: the ceiling is the top of the last one that fits.
    let capacity = ((plot.height() + LED_GAP) / (LED + LED_GAP)).floor().max(1.0);
    let top = nice(values.iter().copied().max().unwrap_or(0));
    let lit_of = |v: u64| -> usize {
        if v == 0 { 0 } else { ((v as f32 / top as f32 * capacity).round() as usize).max(1) }
    };
    let crown = plot.max.y - capacity * (LED + LED_GAP) + LED_GAP;
    let dashed = Stroke::new(1.0, pal.edge_strong);
    for (v, y) in [(top, crown), (top / 2, (crown + plot.max.y) / 2.0)] {
        ui.painter().extend(Shape::dashed_line(&[pos2(plot.min.x, y), pos2(plot.max.x, y)], dashed, 3.0, 3.0));
        text(ui, pos2(plot.min.x - 8.0, y), &fmt_count(v), mono(10.5), pal.dim, GUTTER - 8.0, Align2::RIGHT_CENTER);
    }
    ui.painter().line_segment([pos2(plot.min.x, plot.max.y + 1.0), pos2(plot.max.x, plot.max.y + 1.0)], Stroke::new(1.0, pal.edge_strong));
    text(ui, pos2(plot.min.x - 8.0, plot.max.y), "0", mono(10.5), pal.dim, GUTTER - 8.0, Align2::RIGHT_CENTER);
    let n = values.len();
    let slot = plot.width() / n as f32;
    let width = (slot * 0.72).clamp(2.0, 22.0);
    let hover = ui.interact(plot.expand2(vec2(0.0, 8.0)), id, Sense::hover()).hover_pos();
    let hovered = hover.map(|p| (((p.x - plot.min.x) / slot).floor().max(0.0) as usize).min(n - 1));
    for (i, v) in values.iter().enumerate() {
        let cx = plot.min.x + (i as f32 + 0.5) * slot;
        let lit = lit_of(*v);
        if lit == 0 {
            let floor = Rect::from_min_max(pos2(cx - width / 2.0, plot.max.y - 2.0), pos2(cx + width / 2.0, plot.max.y));
            ui.painter().rect_filled(floor, 0.0, pal.unlit);
            continue;
        }
        let color = match hovered {
            Some(h) if h == i => pal.accent_hot,
            Some(_) => pal.accent.gamma_multiply(0.55),
            None => pal.accent,
        };
        led_column(ui, cx, plot.max.y, width, lit, color);
    }
    let mut last_label: Option<Rect> = None;
    for (i, label) in labels {
        if *i >= n {
            continue;
        }
        let cx = plot.min.x + (*i as f32 + 0.5) * slot;
        let size = vec2(width_of(ui, label, mono(10.5)), 14.0);
        let at = Align2::CENTER_TOP.anchor_size(pos2(cx, plot.max.y + 5.0), size);
        if last_label.is_some_and(|last| last.expand2(vec2(4.0, 0.0)).intersects(at)) {
            continue;
        }
        text(ui, pos2(cx, plot.max.y + 5.0), label, mono(10.5), pal.dim, 80.0, Align2::CENTER_TOP);
        last_label = Some(at);
    }
    if let Some(i) = hovered {
        let words = tip(i, values[i]);
        let font = mono(12.0);
        let size = vec2(width_of(ui, &words, font.clone()) + 16.0, 22.0);
        let cx = plot.min.x + (i as f32 + 0.5) * slot;
        let lit = lit_of(values[i]) as f32;
        let crest = plot.max.y - lit * (LED + LED_GAP);
        let y = (crest - 6.0).max(rect.min.y + size.y + 4.0);
        let x = (cx - size.x / 2.0).clamp(rect.min.x + 4.0, rect.max.x - size.x - 4.0);
        let tip_rect = Rect::from_min_size(pos2(x, y - size.y), size);
        ui.painter().rect_filled(tip_rect, CornerRadius::same(2), pal.raised);
        ui.painter().rect_stroke(tip_rect, CornerRadius::same(2), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        text(ui, tip_rect.center(), &words, font, pal.text, size.x, Align2::CENTER_CENTER);
    }
}

/// Where the tracks live: this server's own library against federated
/// peers', a horizontal meter each, beside the hours.
fn origins_panel(ui: &Ui, rect: Rect, data: &Loaded, pal: &Pal) {
    panel(ui, rect, pal);
    let inner = rect.shrink2(vec2(14.0, 11.0));
    caps(ui, inner.min, &t!("sta.origins_title"), 12.0, pal.text, inner.width(), Align2::LEFT_TOP);
    let o = &data.summary.origins;
    let total = (o.local.plays + o.peers.plays).max(1) as f32;
    let rows = [
        (t!("sta.origin_local_row").to_string(), o.local.plays, o.local.listened_ms, vec![t!("sta.origin_local_note").to_string()]),
        (
            t!("sta.origin_peers_row").to_string(),
            o.peers.plays,
            o.peers.listened_ms,
            vec![t!("sta.origin_peers_note_1").to_string(), t!("sta.origin_peers_note_2").to_string()],
        ),
    ];
    let mut y = inner.min.y + 30.0;
    for (name, plays, ms, notes) in rows {
        if y + 40.0 > inner.max.y {
            break;
        }
        caps(ui, pos2(inner.min.x, y), &name, 11.5, pal.dim, inner.width() * 0.5, Align2::LEFT_TOP);
        let words = t!("sta.plays_time", plays = plays_words(plays), time = fmt_duration(ms)).to_string();
        text(ui, pos2(inner.max.x, y), &words, mono(12.0), pal.text, inner.width() * 0.5, Align2::RIGHT_TOP);
        let track = Rect::from_min_size(pos2(inner.min.x, y + 20.0), vec2(inner.width(), 8.0));
        led_row(ui, track, plays as f32 / total, pal.accent, pal.unlit);
        y += 36.0;
        for note in notes {
            if y + 16.0 > inner.max.y {
                break;
            }
            text(ui, pos2(inner.min.x, y), &note, body(12.0), pal.dim, inner.width(), Align2::LEFT_TOP);
            y += 17.0;
        }
        y += 8.0;
    }
}

// ── Tables ────────────────────────────────────────────────────────────────

/// A column: its heading, its width (zero for the one that takes the
/// rest) and whether it is right-aligned.
struct Col {
    title: String,
    w: f32,
    right: bool,
}

fn col(title: impl Into<String>, w: f32, right: bool) -> Col {
    Col { title: title.into(), w, right }
}

/// The columns' x ranges across `x0..x1`.
fn spans(x0: f32, x1: f32, cols: &[Col]) -> Vec<(f32, f32)> {
    let gap = 16.0;
    let fixed: f32 = cols.iter().map(|c| c.w).sum::<f32>() + gap * cols.len().saturating_sub(1) as f32;
    let flex = (x1 - x0 - fixed).max(60.0);
    let mut x = x0;
    cols.iter()
        .map(|c| {
            let w = if c.w == 0.0 { flex } else { c.w };
            let span = (x, w);
            x += w + gap;
            span
        })
        .collect()
}

/// What one row hands its painter: the row, its cells, and its state.
struct RowCtx<'a> {
    rect: Rect,
    cells: &'a [(f32, f32)],
    selected: bool,
    hovered: bool,
}

/// A table under its headings in spaced capitals, rows scrolled by egui,
/// the page's cursor lit amber on the selected row and kept in view as the
/// keys move it. A click selects. Returns the last row index on screen,
/// for paging.
#[allow(clippy::too_many_arguments)]
fn table(
    ui: &mut Ui,
    id: Id,
    cols: &[Col],
    len: usize,
    sel: Option<usize>,
    pal: &Pal,
    acts: &mut Vec<Act>,
    mut draw_row: impl FnMut(&Ui, usize, &RowCtx),
) -> usize {
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let cells = spans(head.min.x + 10.0, head.max.x - 10.0, cols);
    for (c, (x, cw)) in cols.iter().zip(&cells) {
        let (pos, align) =
            if c.right { (pos2(x + cw, head.center().y), Align2::RIGHT_CENTER) } else { (pos2(*x, head.center().y), Align2::LEFT_CENTER) };
        caps(ui, pos, &c.title, 11.0, pal.dim, *cw, align);
    }
    ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, pal.edge_strong));
    ui.add_space(4.0);

    // The cursor moved since the last pass: the offset that brings it in.
    let memory = id.with("view");
    let (last_sel, last_offset, last_height): (Option<usize>, f32, f32) =
        ui.ctx().data(|d| d.get_temp(memory)).unwrap_or((None, 0.0, 0.0));
    let mut scroll = ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    if sel != last_sel
        && let Some(s) = sel
        && last_height > 0.0
    {
        let (row_top, row_bottom) = (s as f32 * ROW_H, (s + 1) as f32 * ROW_H);
        if row_top < last_offset {
            scroll = scroll.vertical_scroll_offset(row_top);
        } else if row_bottom > last_offset + last_height {
            scroll = scroll.vertical_scroll_offset(row_bottom - last_height);
        }
    }
    let mut last_on_screen = 0;
    let output = scroll.show_rows(ui, ROW_H, len, |ui, range| {
        last_on_screen = range.end.saturating_sub(1);
        for i in range {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            let selected = sel == Some(i);
            let hovered = !selected && response.hovered();
            if selected {
                ui.painter().rect_filled(rect, CornerRadius::same(2), pal.accent);
            } else if hovered {
                ui.painter().rect_filled(rect, CornerRadius::same(2), pal.raised);
            }
            if !selected {
                let rule = [pos2(rect.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)];
                ui.painter().line_segment(rule, Stroke::new(1.0, pal.panel));
            }
            let cells = spans(rect.min.x + 10.0, rect.max.x - 10.0, cols);
            draw_row(ui, i, &RowCtx { rect, cells: &cells, selected, hovered });
            if response.clicked() {
                acts.push(Act::Select(i));
            }
        }
    });
    let state = (sel, output.state.offset.y, output.inner_rect.height());
    ui.ctx().data_mut(|d| d.insert_temp(memory, state));
    last_on_screen
}

/// A cell's text colour on its row: the selection's ink, else its own.
fn ink(row: &RowCtx, own: Color32, pal: &Pal) -> Color32 {
    if row.selected {
        pal.on_accent
    } else if row.hovered && own == pal.text {
        Color32::WHITE
    } else {
        own
    }
}

/// A cell's text: left or right in its column, cut to fit.
fn cell(ui: &Ui, row: &RowCtx, at: usize, s: &str, font: FontId, color: Color32, right: bool) -> Rect {
    let (x, w) = row.cells[at];
    let y = row.rect.center().y;
    if right {
        text(ui, pos2(x + w, y), s, font, color, w, Align2::RIGHT_CENTER)
    } else {
        text(ui, pos2(x, y), s, font, color, w, Align2::LEFT_CENTER)
    }
}

/// A name and, after it, the dim `via` a peer's track carries.
fn name_cell(ui: &Ui, row: &RowCtx, at: usize, name: &str, via: Option<&str>, pal: &Pal) {
    let (x, w) = row.cells[at];
    let font = if row.selected { strong(14.0) } else { body(14.0) };
    let via_w = via.map_or(0.0, |v| width_of(ui, v, body(12.5)) + 6.0).min(w * 0.4);
    let drawn = text(ui, pos2(x, row.rect.center().y), name, font, ink(row, pal.text, pal), w - via_w, Align2::LEFT_CENTER);
    if let Some(via) = via {
        text(ui, pos2(drawn.max.x + 6.0, row.rect.center().y), via, body(12.5), ink(row, pal.dim, pal), via_w, Align2::LEFT_CENTER);
    }
}

// ── Top ───────────────────────────────────────────────────────────────────

fn top(ui: &mut Ui, page: &Page, pal: &Pal, acts: &mut Vec<Act>) {
    if page.data.is_none() {
        loading(ui, page, pal);
        return;
    }
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
    let show = caps(ui, row.left_center(), &t!("sta.ctl_show"), 11.5, pal.dim, w, Align2::LEFT_CENTER);
    let entities: Vec<String> = Entity::ALL.iter().map(|e| e.label()).collect();
    let (shown, clicked) = segmented(
        ui,
        Id::new("stats-entity"),
        show.max.x + 10.0,
        row.center().y,
        &entities,
        page.entity.index(),
        Seg::Marked,
        pal,
    );
    if let Some(i) = clicked {
        acts.push(Act::Entity(i));
    }
    let metrics = [Metric::Plays.label(), Metric::Time.label()];
    let chosen = if page.metric == Metric::Plays { 0 } else { 1 };
    let metric_w = segmented_width(ui, &metrics);
    let rank = t!("sta.ctl_rank").to_string();
    let rank_w = caps_width(ui, &rank, 11.5);
    let left = row.max.x - metric_w;
    if left - rank_w - 10.0 > shown.max.x + 24.0 {
        caps(ui, pos2(left - 10.0, row.center().y), &rank, 11.5, pal.dim, rank_w + 1.0, Align2::RIGHT_CENTER);
        let (_, clicked) = segmented(ui, Id::new("stats-metric"), left, row.center().y, &metrics, chosen, Seg::Marked, pal);
        if let Some(i) = clicked {
            acts.push(Act::Metric(i));
        }
    }
    ui.add_space(14.0);

    let tracks = page.entity == Entity::Tracks;
    let mut cols = vec![col(t!("sta.col_rank"), 26.0, true), col(page.entity.header(), 0.0, false)];
    cols.push(if tracks { col(t!("sta.col_last"), 110.0, false) } else { col(t!("sta.col_tracks"), 90.0, false) });
    cols.push(col(t!("sta.col_share"), 120.0, false));
    cols.push(col(t!("sta.col_plays"), 64.0, true));
    cols.push(col(t!("sta.col_time"), 80.0, true));
    if page.top.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        text(ui, rect.left_center() + vec2(10.0, 0.0), &t!("sta.nothing_here"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }
    let metric = page.metric;
    let top_value = page.top.iter().map(|i| metric.of(i)).max().unwrap_or(0).max(1);
    let now = unix_now();
    table(ui, Id::new("stats-top"), &cols, page.top.len(), page.sel, pal, acts, |ui, i, row| {
        let item = &page.top[i];
        let (name, via) = if tracks {
            let (title, artist, _, _) = track_words(item.track.as_ref());
            (join_artist(&artist, &title), (item.origin.as_deref() == Some("peer")).then(|| via_words(item.peer_name.as_deref())))
        } else {
            (item.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| t!("sta.unknown").to_string()), None)
        };
        cell(ui, row, 0, &format!("{:02}", i + 1), mono(12.5), ink(row, pal.dim, pal), true);
        name_cell(ui, row, 1, &printable(&name, TEXT_MAX), via.as_deref(), pal);
        let extra = if tracks {
            item.last_played
                .as_deref()
                .and_then(iso_unix)
                .map(|t| day_label(t, now, page.offset_at(t), page.offset_at(now)))
                .unwrap_or_default()
        } else {
            tracks_words(item.tracks.unwrap_or(0))
        };
        cell(ui, row, 2, &extra, mono(12.5), ink(row, pal.dim, pal), false);
        let (x, cw) = row.cells[3];
        let track = Rect::from_center_size(pos2(x + cw / 2.0, row.rect.center().y), vec2(cw, 8.0));
        let share = metric.of(item) as f32 / top_value as f32;
        let (lit, unlit) = if row.selected {
            (pal.on_accent, pal.on_accent.gamma_multiply(0.22))
        } else {
            (pal.accent, pal.unlit)
        };
        led_row(ui, track, share, lit, unlit);
        cell(ui, row, 4, &fmt_count(item.plays), mono(13.5), ink(row, pal.text, pal), true);
        cell(ui, row, 5, &fmt_duration(item.listened_ms), mono(12.5), ink(row, pal.dim, pal), true);
    });
}

// ── Recent ────────────────────────────────────────────────────────────────

fn recent(ui: &mut Ui, page: &mut Page, pal: &Pal, acts: &mut Vec<Act>) {
    if page.data.is_none() {
        loading(ui, page, pal);
        return;
    }
    let w = ui.available_width();
    let cols = [
        col(t!("sta.col_when"), 48.0, false),
        col(t!("sta.col_day"), 104.0, false),
        col(t!("sta.col_track"), 0.0, false),
        col(t!("sta.col_outcome"), 150.0, false),
        col(t!("sta.col_listened"), 110.0, false),
        col(t!("sta.col_client"), 120.0, false),
    ];
    if page.history.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        text(ui, rect.left_center() + vec2(10.0, 0.0), &t!("sta.no_rows_recent"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }
    let now = unix_now();
    let len = page.history.len();
    let shown: &Page = page;
    let last = table(ui, Id::new("stats-recent"), &cols, len, shown.sel, pal, acts, |ui, i, row| {
        let r = recent_row(shown, &shown.history[i], now);
        cell(ui, row, 0, &r.time, mono(13.5), ink(row, pal.text, pal), false);
        cell(ui, row, 1, &r.day, mono(12.5), ink(row, pal.dim, pal), false);
        name_cell(ui, row, 2, &r.track, r.via.as_deref(), pal);
        let outcome = if r.completed { pal.ok } else { pal.dim };
        cell(ui, row, 3, &r.outcome, body(13.0), ink(row, outcome, pal), false);
        let listened = if r.counted { pal.text } else { pal.dim };
        cell(ui, row, 4, &r.listened, mono(12.5), ink(row, listened, pal), false);
        let client = if r.counted { r.client.clone() } else { t!("sta.not_counted").to_string() };
        cell(ui, row, 5, &client, body(12.5), ink(row, pal.dim, pal), false);
    });
    // The log pages as its end comes into view, as the cursor nearing the
    // end pages it on the cells page.
    if last + PAGE_AHEAD >= len {
        more(page);
    }
}

/// The log's next page, once, if the server has one.
fn more(page: &mut Page) {
    if page.tab != Tab::Recent || page.in_flight || page.queued.is_some() {
        return;
    }
    let Some(before) = page.next.take() else { return };
    let op = Op::History { seq: page.seq, period: page.period.clone(), origin: page.origin, tz: page.tz.clone(), before };
    page.queue(op, t!("sta.busy_more"));
}

// ── The note line ─────────────────────────────────────────────────────────

/// The panel's foot: what the page is busy with, its note, or the selected
/// row in words — and on the log, the way to forget the selected play.
fn note_line(ui: &mut Ui, rect: Rect, page: &Page, pal: &Pal, acts: &mut Vec<Act>) {
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, pal.edge));
    let mid = rect.center().y + 2.0;
    let mut right = rect.max.x;
    if page.tab == Tab::Recent && page.sel.is_some() && matches!(page.modal, Modal::None) {
        let label = t!("sta.forget_go").to_string();
        let bw = caps_width(ui, &label, 12.5) + 26.0;
        let at = Rect::from_center_size(pos2(right - bw / 2.0, mid), vec2(bw, 24.0));
        if button(ui, Id::new("stats-forget-open"), at, &label, false, pal.down, pal).clicked() {
            acts.push(Act::Forget);
        }
        right = at.min.x - 12.0;
    }
    let room = right - rect.min.x;
    if let Some(busy) = &page.busy {
        let spin = Rect::from_center_size(pos2(rect.min.x + 8.0, mid), vec2(14.0, 14.0));
        ui.put(spin, egui::Spinner::new().size(14.0).color(pal.accent));
        text(ui, pos2(rect.min.x + 24.0, mid), busy, mono(12.5), pal.accent, room - 24.0, Align2::LEFT_CENTER);
    } else if let Some((note, is_err)) = &page.note {
        text(ui, pos2(rect.min.x, mid), note, mono(12.5), if *is_err { pal.gold } else { pal.dim }, room, Align2::LEFT_CENTER);
    } else if let (Some(i), Modal::None) = (page.sel, &page.modal)
        && let Some(words) = row_words(page, i)
    {
        text(ui, pos2(rect.min.x, mid), &words, mono(12.5), pal.dim, room, Align2::LEFT_CENTER);
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The period list, dropped under the period's box: every period with
/// data, the current one marked, the keys' cursor lit amber. A click on a
/// row picks it; a click anywhere else closes the list.
#[allow(clippy::too_many_arguments)]
fn period_list(ui: &mut Ui, page: &Page, pal: &Pal, full: Rect, anchor: Option<Rect>, cursor: usize, acts: &mut Vec<Act>) {
    let options = page.choices();
    let foot = page
        .data
        .as_ref()
        .and_then(|d| d.periods.as_ref())
        .and_then(|p| p.earliest.as_deref())
        .and_then(iso_unix)
        .map(|t| t!("sta.period_foot", date = date_long(t, page.offset_at(t))).to_string());
    let width = 320.0_f32.min(full.width() - 2.0 * PAD.x);
    let anchor = anchor.unwrap_or(Rect::from_min_size(full.min + PAD + vec2(width, 0.0), vec2(0.0, 34.0)));
    let pos = pos2((anchor.max.x - width).max(full.min.x + PAD.x), anchor.max.y + 4.0);
    let max_h = (full.max.y - pos.y - 16.0 - if foot.is_some() { 28.0 } else { 0.0 }).max(ROW_H * 3.0);
    let ctx = ui.ctx().clone();
    let shown = egui::Area::new(Id::new("stats-period-list"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain_to(full)
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(pal.panel)
                .stroke(Stroke::new(1.0, pal.accent))
                .corner_radius(square())
                .inner_margin(egui::Margin::same(4))
                .shadow(egui::Shadow { offset: [0, 6], blur: 16, spread: 0, color: Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let moved = ui.ctx().data(|d| d.get_temp::<usize>(Id::new("stats-period-cursor"))) != Some(cursor);
                    ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        for (i, option) in options.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                            let on = i == cursor;
                            if on {
                                ui.painter().rect_filled(rect, CornerRadius::same(2), pal.accent);
                                if moved {
                                    response.scroll_to_me(None);
                                }
                            } else if response.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(2), pal.raised);
                            }
                            if option.is(&page.period.period, page.period.offset) {
                                let mark = Rect::from_center_size(pos2(rect.min.x + 13.0, rect.center().y), vec2(6.0, 6.0));
                                ui.painter().rect_filled(mark, 0.0, if on { pal.on_accent } else { pal.accent });
                            }
                            let name_color = if on { pal.on_accent } else { pal.text };
                            let name = text(ui, pos2(rect.min.x + 26.0, rect.center().y), &option.name, strong(14.0), name_color, rect.width() - 36.0, Align2::LEFT_CENTER);
                            if let Some(detail) = &option.detail {
                                let room = rect.max.x - 10.0 - name.max.x - 12.0;
                                if room > 30.0 {
                                    let color = if on { pal.on_accent } else { pal.dim };
                                    text(ui, pos2(rect.max.x - 10.0, rect.center().y), detail, mono(12.0), color, room, Align2::RIGHT_CENTER);
                                }
                            }
                            if response.clicked() {
                                acts.push(Act::PeriodPick(i));
                            }
                        }
                    });
                    if let Some(foot) = &foot {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                        text(ui, pos2(rect.min.x + 26.0, rect.center().y + 2.0), foot, mono(11.5), pal.dim, rect.width() - 34.0, Align2::LEFT_CENTER);
                    }
                    ui.ctx().data_mut(|d| d.insert_temp(Id::new("stats-period-cursor"), cursor));
                });
        });
    let list = shown.response.rect;
    let outside = ui.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !list.contains(p) && !anchor.contains(p)));
    if outside {
        acts.push(Act::PeriodClose);
    }
    // A click on the period's box toggles the list shut (the page's PeriodList).
    let pill = ui.interact(anchor, Id::new("stats-period"), Sense::click());
    if pill.clicked() {
        acts.push(Act::PeriodList);
    }
}

/// The forget gate, gold because it is destructive: the play in words,
/// the one fact, Keep and Forget. A click off the card keeps the play.
fn forget(ui: &mut Ui, page: &Page, pal: &Pal, full: Rect, row: usize, acts: &mut Vec<Act>) {
    let words = page.history.get(row).map(|item| {
        let r = recent_row(page, item, unix_now());
        let mut w = t!("sta.row_recent", track = r.track, day = r.day, time = r.time, outcome = r.outcome).to_string();
        if !r.completed && item.duration_ms.is_some_and(|d| d > 0) {
            w.push_str(&t!("sta.row_recent_of", total = fmt_clock(item.duration_ms.unwrap_or(0))));
        }
        w
    });
    let ctx = ui.ctx().clone();
    egui::Area::new(Id::new("stats-forget"))
        .order(egui::Order::Foreground)
        .fixed_pos(full.min)
        .constrain_to(full)
        .show(&ctx, |ui| {
            let backdrop = ui.allocate_rect(full, Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha(150));
            let card_w = 480.0_f32.min(full.width() - 40.0);
            let card_rect = Rect::from_center_size(full.center(), vec2(card_w, 196.0));
            ui.painter().rect_filled(card_rect, square(), pal.panel);
            ui.painter().rect_stroke(card_rect, square(), Stroke::new(1.0, pal.gold), StrokeKind::Inside);
            let inner = card_rect.shrink(20.0);
            caps(ui, inner.min, &t!("sta.forget_title"), 14.0, pal.gold, inner.width(), Align2::LEFT_TOP);
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(inner.min + vec2(0.0, 30.0), pos2(inner.max.x, inner.max.y - 40.0))));
            child.label(egui::RichText::new(words.unwrap_or_default()).font(body(14.0)).color(pal.text));
            child.add_space(8.0);
            child.label(egui::RichText::new(t!("sta.forget_fact")).font(body(13.0)).color(pal.gold));
            let go = t!("sta.forget_go").to_string();
            let keep = t!("sta.forget_keep").to_string();
            let go_w = caps_width(ui, &go, 12.5) + 32.0;
            let keep_w = caps_width(ui, &keep, 12.5) + 32.0;
            let go_rect = Rect::from_min_size(pos2(inner.max.x - go_w, inner.max.y - 30.0), vec2(go_w, 30.0));
            let keep_rect = Rect::from_min_size(pos2(go_rect.min.x - 10.0 - keep_w, go_rect.min.y), vec2(keep_w, 30.0));
            let kept = button(ui, Id::new("stats-forget-keep"), keep_rect, &keep, true, pal.accent, pal).clicked();
            let gone = button(ui, Id::new("stats-forget-go"), go_rect, &go, false, pal.down, pal).clicked();
            let on_card = ui.input(|i| i.pointer.interact_pos().is_some_and(|p| card_rect.contains(p)));
            if gone {
                acts.push(Act::ForgetConfirm);
            } else if kept || (backdrop.clicked() && !on_card) {
                acts.push(Act::ForgetCancel);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::super::tests::{english, ready};
    use super::*;

    /// The panel's region in the default 100×30 window, in points.
    const SMALL: Vec2 = vec2(800.0, 420.0);
    const LARGE: Vec2 = vec2(1400.0, 860.0);

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::gui::window::panel::fonts(&ctx);
        ctx
    }

    /// One pass of the page over a region of `size`, with `events`.
    fn pass(ctx: &egui::Context, page: &mut Page, size: Vec2, events: Vec<egui::Event>) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| show(ui, page));
        // No GPU here to take the font atlas: let go of it on purpose, or
        // epaint's debug check fails the drop.
        out.textures_delta.clear();
        out
    }

    /// A press and a release at `at`, a pass each after the move, as hands
    /// would make them.
    fn click(ctx: &egui::Context, page: &mut Page, size: Vec2, at: Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let _ = pass(ctx, page, size, vec![egui::Event::PointerMoved(at)]);
        let _ = pass(ctx, page, size, vec![button(true)]);
        let _ = pass(ctx, page, size, vec![button(false)]);
    }

    /// Every text the pass drew, as laid out.
    fn drawn(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_tab_draws_at_the_default_window_and_a_large_one() {
        let _en = english();
        for size in [SMALL, LARGE] {
            let ctx = context();
            let mut page = ready().hosted();
            for tab in Tab::ALL {
                page.act(Act::Tab(tab));
                // Twice: egui sizes some things on the first pass.
                let _ = pass(&ctx, &mut page, size, Vec::new());
                let out = pass(&ctx, &mut page, size, Vec::new());
                assert!(!out.shapes.is_empty(), "{tab:?} at {size:?} drew nothing");
            }
        }
    }

    #[test]
    fn a_click_on_a_tab_is_the_pages_own_act() {
        let _en = english();
        let ctx = context();
        let mut page = ready().hosted();
        let _ = pass(&ctx, &mut page, SMALL, Vec::new());
        let top = ctx.read_response(Id::new("stats-tabs").with(1)).expect("the Top segment was drawn");
        click(&ctx, &mut page, SMALL, top.rect.center());
        assert_eq!(page.tab, Tab::Top);
        let recent = ctx.read_response(Id::new("stats-tabs").with(2)).expect("the Recent segment was drawn");
        click(&ctx, &mut page, SMALL, recent.rect.center());
        assert_eq!(page.tab, Tab::Recent);
    }

    #[test]
    fn the_period_box_drops_the_list_and_a_click_off_it_closes_it() {
        let _en = english();
        let ctx = context();
        let mut page = ready().hosted();
        let _ = pass(&ctx, &mut page, SMALL, Vec::new());
        let pill = ctx.read_response(Id::new("stats-period")).expect("the period box was drawn");
        click(&ctx, &mut page, SMALL, pill.rect.center());
        assert!(matches!(page.modal, Modal::Period(_)), "the list dropped");
        // The keys still walk it: the page's own model, whatever draws it.
        page.key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Down,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        let _ = pass(&ctx, &mut page, SMALL, Vec::new());
        click(&ctx, &mut page, SMALL, pos2(40.0, SMALL.y - 60.0));
        assert_eq!(page.modal, Modal::None, "a click off the list closed it");
    }

    #[test]
    fn the_readouts_are_the_cells_pages_words() {
        // What the tiles say is decided once, in `tiles`: the egui page
        // lays the same words out — the labels in the desk's capitals, a
        // comparison's sign drawn as its arrow — so the two faces cannot
        // disagree.
        let _en = english();
        let ctx = context();
        let mut page = ready().hosted();
        let out = pass(&ctx, &mut page, SMALL, Vec::new());
        let data = page.data.as_ref().expect("loaded");
        let versus = versus_label(&page.shown, &page.options);
        let words = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
        let drawn = drawn(&out);
        for (value, label, detail) in &words {
            assert!(drawn.iter().any(|d| d == value), "{value} drawn: {drawn:?}");
            assert!(drawn.iter().any(|d| *d == label.to_uppercase()), "{label} drawn in capitals");
            let rest = detail.trim_start_matches(['+', '−', '-']).trim_start();
            assert!(drawn.iter().any(|d| d == rest), "{detail} drawn (its sign as an arrow)");
        }
    }
}
