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
//! Built only with the window: the terminal build has no panel to draw in.

use egui::epaint::{CornerRadius, StrokeKind};
use egui::text::{LayoutJob, TextWrapping};
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
const GAP: f32 = 12.0;
const ROW_H: f32 = 30.0;
const RADIUS: u8 = 12;

/// The kit's palette as egui colours: the window pins truecolour, so every
/// role is an RGB; the fallbacks are the canvas hexes.
struct Pal {
    ground: Color32,
    card: Color32,
    edge: Color32,
    popup: Color32,
    text: Color32,
    dim: Color32,
    faint: Color32,
    accent: Color32,
    on_accent: Color32,
    bright: Color32,
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
        let dim = rgb(th.dim, Color32::from_rgb(0x8a, 0x92, 0xae));
        Pal {
            ground,
            card: Color32::from_white_alpha(7),
            edge: Color32::from_white_alpha(16),
            popup: lift(ground, 14),
            text: rgb(th.text, Color32::from_rgb(0xd8, 0xde, 0xe9)),
            dim,
            faint: dim.gamma_multiply(0.45),
            accent: rgb(th.accent, Color32::from_rgb(0x7a, 0xab, 0xdf)),
            on_accent: rgb(th.on_accent, Color32::from_rgb(0x0d, 0x10, 0x17)),
            bright: rgb(th.bright, Color32::WHITE),
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

fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
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
    ui_body.add_space(14.0);
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

/// egui's look for the page, set once: dark, the kit's accent on the
/// scroll bars and the selection.
fn style_once(ctx: &egui::Context, pal: &Pal) {
    let id = Id::new("stats-egui-styled");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_theme(egui::Theme::Dark);
    let (accent, text, ground) = (pal.accent, pal.text, pal.ground);
    ctx.all_styles_mut(|style| {
        style.visuals.selection.bg_fill = accent;
        style.visuals.extreme_bg_color = ground;
        style.visuals.panel_fill = ground;
        style.visuals.window_fill = ground;
        let widgets = &mut style.visuals.widgets;
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active] {
            state.fg_stroke.color = text;
        }
        widgets.inactive.bg_fill = Color32::from_white_alpha(40);
        widgets.hovered.bg_fill = accent.gamma_multiply(0.7);
        widgets.active.bg_fill = accent;
        style.spacing.item_spacing = vec2(8.0, 0.0);
        style.interaction.tooltip_delay = 0.0;
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

// ── Painting helpers ──────────────────────────────────────────────────────

/// `s` in one line no wider than `max_w`, an ellipsis where it is cut,
/// anchored at `pos` by `align`. Returns where it was drawn.
fn text(ui: &Ui, pos: Pos2, s: &str, font: FontId, color: Color32, max_w: f32, align: Align2) -> Rect {
    let mut job = LayoutJob::simple_singleline(s.to_string(), font, color);
    job.wrap = TextWrapping::truncate_at_width(max_w.max(1.0));
    let galley = ui.painter().layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    ui.painter().galley(rect.min, galley, color);
    rect
}

/// How wide `s` is in `font`.
fn width_of(ui: &Ui, s: &str, font: FontId) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), font, Color32::WHITE).size().x
}

/// How wide [`segmented`] draws `labels`.
fn segmented_width(ui: &Ui, labels: &[String]) -> f32 {
    labels.iter().map(|l| width_of(ui, l, body(14.0)) + 26.0).sum::<f32>() + 6.0
}

/// A pill-shaped segmented control at `left`, vertically centred on
/// `mid_y`: one segment per label, `chosen` filled with the accent.
/// Returns the control's rect and the segment clicked, if any.
fn segmented(ui: &Ui, id: Id, left: f32, mid_y: f32, labels: &[String], chosen: usize, pal: &Pal) -> (Rect, Option<usize>) {
    let font = body(14.0);
    let h = 30.0;
    let pad = 3.0;
    let widths: Vec<f32> = labels.iter().map(|l| width_of(ui, l, font.clone()) + 26.0).collect();
    let total = segmented_width(ui, labels);
    let outer = Rect::from_min_size(pos2(left, mid_y - h / 2.0), vec2(total, h));
    ui.painter().rect_filled(outer, CornerRadius::same((h / 2.0) as u8), Color32::from_white_alpha(8));
    let mut x = outer.min.x + pad;
    let mut clicked = None;
    for (i, (label, w)) in labels.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(pos2(x, outer.min.y + pad), vec2(*w, h - 2.0 * pad));
        let response = ui.interact(seg, id.with(i), Sense::click());
        let on = i == chosen;
        let radius = CornerRadius::same(((h - 2.0 * pad) / 2.0) as u8);
        let color = if on {
            ui.painter().rect_filled(seg, radius, pal.accent);
            pal.on_accent
        } else if response.hovered() {
            ui.painter().rect_filled(seg, radius, Color32::from_white_alpha(12));
            pal.bright
        } else {
            pal.dim
        };
        let font = if on { strong(14.0) } else { font.clone() };
        text(ui, seg.center(), label, font, color, *w, Align2::CENTER_CENTER);
        if response.clicked() && !on {
            clicked = Some(i);
        }
        x += w;
    }
    (outer, clicked)
}

/// A downward chevron, two strokes, centred on `at`.
fn chevron(ui: &Ui, at: Pos2, color: Color32, up: bool) {
    let (dx, dy) = (4.0, if up { -2.5 } else { 2.5 });
    let stroke = Stroke::new(1.6, color);
    ui.painter().line_segment([at + vec2(-dx, -dy), at + vec2(0.0, dy)], stroke);
    ui.painter().line_segment([at + vec2(0.0, dy), at + vec2(dx, -dy)], stroke);
}

/// A rounded card: the page's tiles and charts sit on these.
fn card(ui: &Ui, rect: Rect, pal: &Pal) {
    ui.painter().rect_filled(rect, CornerRadius::same(RADIUS), pal.card);
    ui.painter().rect_stroke(rect, CornerRadius::same(RADIUS), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
}

/// A button at `rect`: filled with the accent when `primary`, an outline
/// in `tone` otherwise.
fn button(ui: &Ui, id: Id, rect: Rect, label: &str, primary: bool, tone: Color32, pal: &Pal) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let radius = CornerRadius::same(8);
    let hovered = response.hovered();
    let color = if primary {
        let fill = if hovered { lift(pal.accent, 18) } else { pal.accent };
        ui.painter().rect_filled(rect, radius, fill);
        pal.on_accent
    } else {
        if hovered {
            ui.painter().rect_filled(rect, radius, tone.gamma_multiply(0.15));
        }
        ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, tone), StrokeKind::Inside);
        tone
    };
    text(ui, rect.center(), label, strong(14.0), color, rect.width(), Align2::CENTER_CENTER);
    response
}

/// One gold sentence for a state with nothing else to show.
fn sentence(ui: &mut Ui, head: &str, tail: &str, pal: &Pal) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let head_rect = text(ui, rect.left_center(), head, strong(16.0), pal.gold, w, Align2::LEFT_CENTER);
    text(ui, pos2(head_rect.max.x, rect.center().y), tail, body(16.0), pal.gold, rect.max.x - head_rect.max.x, Align2::LEFT_CENTER);
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

// ── The header: tabs, the tab's note, the period ──────────────────────────

/// The tabs on the left; on the right the tab's note and the period, a
/// pill that drops the list. Returns where the period's pill is, for the
/// list to hang from.
fn header(ui: &mut Ui, page: &Page, pal: &Pal, acts: &mut Vec<Act>) -> Option<Rect> {
    let w = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
    let labels: Vec<String> = Tab::ALL.iter().map(|t| t.name()).collect();
    let chosen = Tab::ALL.iter().position(|t| *t == page.tab).unwrap_or(0);
    let (tabs, clicked) = segmented(ui, Id::new("stats-tabs"), row.min.x, row.center().y, &labels, chosen, pal);
    if let Some(i) = clicked {
        acts.push(Act::Tab(Tab::ALL[i]));
    }
    let control = page.data.is_some() && !page.no_api && !page.no_plays_ever();
    let mut right = row.max.x;
    let mut period_at = None;
    if control {
        let name = page.period.name.clone();
        let pill_w = width_of(ui, &name, strong(14.0)) + 48.0;
        let pill = Rect::from_min_size(pos2(right - pill_w, row.center().y - 15.0), vec2(pill_w, 30.0));
        let response = ui.interact(pill, Id::new("stats-period"), Sense::click());
        let open = matches!(page.modal, Modal::Period(_));
        let fill = if open {
            Color32::from_white_alpha(22)
        } else if response.hovered() {
            Color32::from_white_alpha(16)
        } else {
            Color32::from_white_alpha(9)
        };
        ui.painter().rect_filled(pill, CornerRadius::same(15), fill);
        if open {
            ui.painter().rect_stroke(pill, CornerRadius::same(15), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        }
        text(ui, pos2(pill.min.x + 16.0, pill.center().y), &name, strong(14.0), pal.text, pill_w - 40.0, Align2::LEFT_CENTER);
        chevron(ui, pos2(pill.max.x - 18.0, pill.center().y), if response.hovered() { pal.bright } else { pal.dim }, open);
        if response.clicked() {
            acts.push(Act::PeriodList);
        }
        period_at = Some(pill);
        right = pill.min.x - 14.0;
    }
    // Whose plays, on the Overview while the log holds peer plays: beside
    // the period, where the cells page puts it on its controls row.
    if control && page.tab == Tab::Overview && page.peer_plays() {
        let options: Vec<String> = Origin::ALL.iter().map(|o| o.label()).collect();
        let seg_w = segmented_width(ui, &options);
        if right - seg_w > tabs.max.x + 24.0 {
            let (seg, clicked) =
                segmented(ui, Id::new("stats-origin"), right - seg_w, row.center().y, &options, page.origin.index(), pal);
            if let Some(i) = clicked {
                acts.push(Act::Origin(i));
            }
            right = seg.min.x - 14.0;
        }
    }
    let mut parts: Vec<String> = page.tab.note().into_iter().collect();
    if page.tab == Tab::Recent && page.zone.is_none() {
        parts.push(t!("sta.zone_utc").to_string());
    }
    if !parts.is_empty() {
        let room = right - tabs.max.x - 16.0;
        if room > 40.0 {
            text(ui, pos2(right, row.center().y), &parts.join(" · "), body(13.0), pal.dim, room, Align2::RIGHT_CENTER);
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
        text(ui, rect.left_center(), &t!("sta.empty_prov", zone = zone), body(13.0), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }
    if data.summary.events == 0 {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        let title = t!("sta.nothing_title", period = page.shown.name.clone());
        text(ui, rect.left_center(), &title, strong(18.0), pal.text, w, Align2::LEFT_CENTER);
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

    // The six tiles: two rows of three cards, each the figure with its
    // label beside it and the comparison under them.
    let versus = versus_label(&page.shown, &page.options);
    let tiles = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
    let tile_w = (w - 2.0 * GAP) / 3.0;
    let tile_h = 68.0;
    for row in tiles.chunks(3) {
        let (band, _) = ui.allocate_exact_size(vec2(w, tile_h), Sense::hover());
        for (col, (value, label, detail)) in row.iter().enumerate() {
            let rect = Rect::from_min_size(pos2(band.min.x + col as f32 * (tile_w + GAP), band.min.y), vec2(tile_w, tile_h));
            card(ui, rect, pal);
            let inner = rect.shrink2(vec2(16.0, 10.0));
            let figure = text(ui, inner.min, value, strong(24.0), pal.text, inner.width() * 0.7, Align2::LEFT_TOP);
            let room = inner.max.x - figure.max.x - 8.0;
            if room > 30.0 {
                let base = pos2(figure.max.x + 8.0, figure.max.y - 3.0);
                text(ui, base, label, body(14.0), pal.dim, room, Align2::LEFT_BOTTOM);
            }
            let tone = if detail.starts_with('+') {
                pal.ok
            } else if detail.starts_with('−') || detail.starts_with('-') {
                pal.gold
            } else {
                pal.faint
            };
            text(ui, pos2(inner.min.x, inner.max.y), detail, body(12.5), tone, inner.width(), Align2::LEFT_BOTTOM);
        }
        ui.add_space(GAP);
    }

    // The charts share what is left: the period's days (or weeks, or
    // months) and the hours — stacked when the window is tall, the hours
    // with where the tracks live beside them; side by side when it is not.
    // A few points short of the foot, so the cards clear the note's rule.
    let avail = ui.available_height() - 8.0;
    let min_h = 120.0;
    if avail < 100.0 {
        return;
    }
    let hours = hour_series(&data.hours);
    let hour_labels: Vec<(usize, String)> = [0, 6, 12, 18, 23].iter().map(|h| (*h, hour_label(*h as i32))).collect();
    let note = hours_note(&data.summary).unwrap_or_default();
    let tip = |i: usize, v: u64| format!("{} · {}", hour_label(i as i32), plays_words(v));
    if avail < 2.0 * min_h + GAP {
        let (band, _) = ui.allocate_exact_size(vec2(w, avail.min(280.0)), Sense::hover());
        let days_w = (w - GAP) * 0.58;
        calendar_chart(ui, Rect::from_min_size(band.min, vec2(days_w, band.height())), page, data, pal);
        let hours_rect = Rect::from_min_max(pos2(band.min.x + days_w + GAP, band.min.y), band.max);
        bar_chart(ui, Id::new("stats-hours"), hours_rect, &t!("sta.chart_hours"), &note, &hours, &hour_labels, &tip, pal);
        return;
    }
    let chart_h = ((avail - GAP) / 2.0).min(280.0);
    let (band, _) = ui.allocate_exact_size(vec2(w, chart_h), Sense::hover());
    calendar_chart(ui, band, page, data, pal);
    ui.add_space(GAP);
    let (band, _) = ui.allocate_exact_size(vec2(w, chart_h), Sense::hover());
    let split = page.peer_plays();
    let hours_w = if split { (w - GAP) * 0.6 } else { w };
    let hours_rect = Rect::from_min_size(band.min, vec2(hours_w, chart_h));
    bar_chart(ui, Id::new("stats-hours"), hours_rect, &t!("sta.chart_hours"), &note, &hours, &hour_labels, &tip, pal);
    if split {
        let rect = Rect::from_min_max(pos2(hours_rect.max.x + GAP, band.min.y), band.max);
        origins_card(ui, rect, data, pal);
    }
}

/// Plays per day, week or month across the shown period: a bar a bucket,
/// buckets folded together only when a bar would be narrower than three
/// points.
fn calendar_chart(ui: &Ui, rect: Rect, page: &Page, data: &Loaded, pal: &Pal) {
    let from = match (page.shown.period.as_str(), data.periods.as_ref().and_then(|p| p.earliest.as_deref())) {
        ("all", Some(earliest)) => earliest.to_string(),
        _ => data.summary.period.from.clone(),
    };
    let (keys, values, bucket) = calendar_series(&data.series, &from, &data.summary.period.to, |t| page.offset_at(t));
    let plot_w = rect.width() - 2.0 * 16.0 - GUTTER;
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
    bar_chart(ui, Id::new("stats-calendar"), rect, &title, &days_note(&data.summary), &folded, &labels, &tip, pal);
}

/// The y axis's gutter, in points.
const GUTTER: f32 = 36.0;

/// A rounded ceiling for an axis: 1, 2 or 5 times a power of ten.
fn nice(v: u64) -> u64 {
    let v = v.max(1);
    let mut p = 1u64;
    while p.saturating_mul(10) <= v {
        p *= 10;
    }
    [1, 2, 5, 10].iter().map(|m| m * p).find(|n| *n >= v).unwrap_or(10 * p)
}

/// A bar chart on a card: the title and its note on the top line, a
/// gridline at the axis's half and top, the labels under their bars, and
/// the bar under the pointer brightened with its value in a tip above it.
#[allow(clippy::too_many_arguments)]
fn bar_chart(
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
    card(ui, rect, pal);
    let inner = rect.shrink2(vec2(16.0, 12.0));
    let title_rect = text(ui, inner.min, title, strong(14.0), pal.text, inner.width(), Align2::LEFT_TOP);
    if !note.is_empty() {
        let room = inner.max.x - title_rect.max.x - 16.0;
        if room > 60.0 {
            text(ui, pos2(inner.max.x, inner.min.y + 1.0), note, body(12.5), pal.dim, room, Align2::RIGHT_TOP);
        }
    }
    let plot = Rect::from_min_max(pos2(inner.min.x + GUTTER, inner.min.y + 30.0), pos2(inner.max.x, inner.max.y - 18.0));
    if plot.height() < 20.0 || values.is_empty() {
        return;
    }
    let top = nice(values.iter().copied().max().unwrap_or(0));
    for (v, y) in [(0, plot.max.y), (top / 2, plot.center().y), (top, plot.min.y)] {
        let line = if v == 0 { pal.edge } else { Color32::from_white_alpha(9) };
        ui.painter().line_segment([pos2(plot.min.x, y), pos2(plot.max.x, y)], Stroke::new(1.0, line));
        if v > 0 || top > 0 {
            text(ui, pos2(plot.min.x - 8.0, y), &fmt_count(v), body(11.0), pal.faint, GUTTER - 8.0, Align2::RIGHT_CENTER);
        }
    }
    let n = values.len();
    let slot = plot.width() / n as f32;
    let bar_w = (slot * 0.68).clamp(1.0, 28.0);
    let hover = ui.interact(plot.expand2(vec2(0.0, 8.0)), id, Sense::hover()).hover_pos();
    let hovered = hover.map(|p| (((p.x - plot.min.x) / slot).floor().max(0.0) as usize).min(n - 1));
    for (i, v) in values.iter().enumerate() {
        let cx = plot.min.x + (i as f32 + 0.5) * slot;
        let h = if *v == 0 { 2.0 } else { (*v as f32 / top as f32 * plot.height()).max(3.0) };
        let bar = Rect::from_min_max(pos2(cx - bar_w / 2.0, plot.max.y - h), pos2(cx + bar_w / 2.0, plot.max.y));
        let r = (bar_w / 2.0).min(4.0) as u8;
        let radius = CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
        let color = if *v == 0 {
            pal.faint.gamma_multiply(0.6)
        } else if hovered == Some(i) {
            lift(pal.accent, 40)
        } else if hovered.is_some() {
            pal.accent.gamma_multiply(0.7)
        } else {
            pal.accent
        };
        ui.painter().rect_filled(bar, radius, color);
    }
    let mut last_label: Option<Rect> = None;
    for (i, label) in labels {
        if *i >= n {
            continue;
        }
        let cx = plot.min.x + (*i as f32 + 0.5) * slot;
        let size = vec2(width_of(ui, label, body(11.0)), 14.0);
        let at = Align2::CENTER_TOP.anchor_size(pos2(cx, plot.max.y + 4.0), size);
        if last_label.is_some_and(|last| last.expand2(vec2(4.0, 0.0)).intersects(at)) {
            continue;
        }
        text(ui, pos2(cx, plot.max.y + 4.0), label, body(11.0), pal.dim, 80.0, Align2::CENTER_TOP);
        last_label = Some(at);
    }
    if let Some(i) = hovered {
        let words = tip(i, values[i]);
        let font = body(12.5);
        let size = vec2(width_of(ui, &words, font.clone()) + 18.0, 24.0);
        let cx = plot.min.x + (i as f32 + 0.5) * slot;
        let h = if values[i] == 0 { 2.0 } else { (values[i] as f32 / top as f32 * plot.height()).max(3.0) };
        let y = (plot.max.y - h - 8.0).max(rect.min.y + size.y + 4.0);
        let x = (cx - size.x / 2.0).clamp(rect.min.x + 4.0, rect.max.x - size.x - 4.0);
        let tip_rect = Rect::from_min_size(pos2(x, y - size.y), size);
        ui.painter().rect_filled(tip_rect, CornerRadius::same(6), pal.popup);
        ui.painter().rect_stroke(tip_rect, CornerRadius::same(6), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
        text(ui, tip_rect.center(), &words, font, pal.text, size.x, Align2::CENTER_CENTER);
    }
}

/// Where the tracks live: this server's own library against federated
/// peers', a bar each, beside the hours.
fn origins_card(ui: &Ui, rect: Rect, data: &Loaded, pal: &Pal) {
    card(ui, rect, pal);
    let inner = rect.shrink2(vec2(16.0, 12.0));
    text(ui, inner.min, &t!("sta.origins_title"), strong(14.0), pal.text, inner.width(), Align2::LEFT_TOP);
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
    let mut y = inner.min.y + 34.0;
    for (name, plays, ms, notes) in rows {
        if y + 40.0 > inner.max.y {
            break;
        }
        text(ui, pos2(inner.min.x, y), &name, strong(13.5), pal.text, inner.width() * 0.5, Align2::LEFT_TOP);
        let words = t!("sta.plays_time", plays = plays_words(plays), time = fmt_duration(ms)).to_string();
        text(ui, pos2(inner.max.x, y), &words, body(12.5), pal.dim, inner.width() * 0.5, Align2::RIGHT_TOP);
        let track = Rect::from_min_size(pos2(inner.min.x, y + 22.0), vec2(inner.width(), 6.0));
        ui.painter().rect_filled(track, CornerRadius::same(3), Color32::from_white_alpha(12));
        let filled = Rect::from_min_size(track.min, vec2(track.width() * plays as f32 / total, track.height()));
        ui.painter().rect_filled(filled, CornerRadius::same(3), pal.accent);
        y += 34.0;
        for note in notes {
            if y + 16.0 > inner.max.y {
                break;
            }
            text(ui, pos2(inner.min.x, y), &note, body(12.0), pal.faint, inner.width(), Align2::LEFT_TOP);
            y += 17.0;
        }
        y += 10.0;
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

/// A table under its headings, rows scrolled by egui, the page's cursor
/// on the selected row and kept in view as the keys move it. A click
/// selects. Returns the last row index on screen, for paging.
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
    let cells = spans(head.min.x + 12.0, head.max.x - 12.0, cols);
    for (c, (x, cw)) in cols.iter().zip(&cells) {
        let (pos, align) = if c.right { (pos2(x + cw, head.center().y), Align2::RIGHT_CENTER) } else { (pos2(*x, head.center().y), Align2::LEFT_CENTER) };
        text(ui, pos, &c.title, strong(12.0), pal.faint, *cw, align);
    }
    ui.painter().line_segment([head.left_bottom(), head.right_bottom()], Stroke::new(1.0, pal.edge));
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
            let radius = CornerRadius::same(8);
            if selected {
                ui.painter().rect_filled(rect, radius, pal.accent);
            } else if hovered {
                ui.painter().rect_filled(rect, radius, Color32::from_white_alpha(10));
            }
            let cells = spans(rect.min.x + 12.0, rect.max.x - 12.0, cols);
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
        pal.bright
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
    let show = text(ui, row.left_center(), &t!("sta.ctl_show"), body(13.0), pal.dim, w, Align2::LEFT_CENTER);
    let entities: Vec<String> = Entity::ALL.iter().map(|e| e.label()).collect();
    let (shown, clicked) = segmented(ui, Id::new("stats-entity"), show.max.x + 12.0, row.center().y, &entities, page.entity.index(), pal);
    if let Some(i) = clicked {
        acts.push(Act::Entity(i));
    }
    let metrics = [Metric::Plays.label(), Metric::Time.label()];
    let chosen = if page.metric == Metric::Plays { 0 } else { 1 };
    let metric_w: f32 = metrics.iter().map(|m| width_of(ui, m, body(14.0)) + 26.0).sum::<f32>() + 6.0;
    let rank = t!("sta.ctl_rank").to_string();
    let rank_w = width_of(ui, &rank, body(13.0));
    let left = row.max.x - metric_w;
    if left - rank_w - 12.0 > shown.max.x + 24.0 {
        text(ui, pos2(left - 12.0, row.center().y), &rank, body(13.0), pal.dim, rank_w + 1.0, Align2::RIGHT_CENTER);
        let (_, clicked) = segmented(ui, Id::new("stats-metric"), left, row.center().y, &metrics, chosen, pal);
        if let Some(i) = clicked {
            acts.push(Act::Metric(i));
        }
    }
    ui.add_space(16.0);

    let tracks = page.entity == Entity::Tracks;
    let mut cols = vec![col(t!("sta.col_rank"), 26.0, true), col(page.entity.header(), 0.0, false)];
    cols.push(if tracks { col(t!("sta.col_last"), 110.0, false) } else { col(t!("sta.col_tracks"), 90.0, false) });
    cols.push(col(t!("sta.col_share"), 120.0, false));
    cols.push(col(t!("sta.col_plays"), 64.0, true));
    cols.push(col(t!("sta.col_time"), 80.0, true));
    if page.top.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        text(ui, rect.left_center() + vec2(12.0, 0.0), &t!("sta.nothing_here"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
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
        cell(ui, row, 0, &(i + 1).to_string(), body(13.0), ink(row, pal.faint, pal), true);
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
        cell(ui, row, 2, &extra, body(13.0), ink(row, pal.dim, pal), false);
        let (x, cw) = row.cells[3];
        let track = Rect::from_center_size(pos2(x + cw / 2.0, row.rect.center().y), vec2(cw, 6.0));
        let bar_ground = if row.selected { pal.on_accent.gamma_multiply(0.25) } else { Color32::from_white_alpha(12) };
        ui.painter().rect_filled(track, CornerRadius::same(3), bar_ground);
        let share = metric.of(item) as f32 / top_value as f32;
        let filled = Rect::from_min_size(track.min, vec2((track.width() * share).max(3.0), track.height()));
        ui.painter().rect_filled(filled, CornerRadius::same(3), if row.selected { pal.on_accent } else { pal.accent });
        cell(ui, row, 4, &fmt_count(item.plays), strong(13.5), ink(row, pal.text, pal), true);
        cell(ui, row, 5, &fmt_duration(item.listened_ms), body(13.0), ink(row, pal.dim, pal), true);
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
        text(ui, rect.left_center() + vec2(12.0, 0.0), &t!("sta.no_rows_recent"), body(14.0), pal.dim, w, Align2::LEFT_CENTER);
        return;
    }
    let now = unix_now();
    let len = page.history.len();
    let shown: &Page = page;
    let last = table(ui, Id::new("stats-recent"), &cols, len, shown.sel, pal, acts, |ui, i, row| {
        let r = recent_row(shown, &shown.history[i], now);
        cell(ui, row, 0, &r.time, strong(13.5), ink(row, pal.text, pal), false);
        cell(ui, row, 1, &r.day, body(13.0), ink(row, pal.dim, pal), false);
        name_cell(ui, row, 2, &r.track, r.via.as_deref(), pal);
        let outcome = if r.completed { pal.ok } else { pal.dim };
        cell(ui, row, 3, &r.outcome, body(13.0), ink(row, outcome, pal), false);
        let listened = if r.counted { pal.text } else { pal.dim };
        cell(ui, row, 4, &r.listened, body(13.0), ink(row, listened, pal), false);
        let client = if r.counted { r.client.clone() } else { t!("sta.not_counted").to_string() };
        cell(ui, row, 5, &client, body(12.5), ink(row, pal.faint, pal), false);
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
        let bw = width_of(ui, &label, strong(14.0)) + 28.0;
        let at = Rect::from_center_size(pos2(right - bw / 2.0, mid), vec2(bw, 24.0));
        if button(ui, Id::new("stats-forget-open"), at, &label, false, pal.gold, pal).clicked() {
            acts.push(Act::Forget);
        }
        right = at.min.x - 12.0;
    }
    let room = right - rect.min.x;
    if let Some(busy) = &page.busy {
        let spin = Rect::from_center_size(pos2(rect.min.x + 8.0, mid), vec2(14.0, 14.0));
        ui.put(spin, egui::Spinner::new().size(14.0).color(pal.accent));
        text(ui, pos2(rect.min.x + 24.0, mid), busy, body(13.0), pal.accent, room - 24.0, Align2::LEFT_CENTER);
    } else if let Some((note, is_err)) = &page.note {
        text(ui, pos2(rect.min.x, mid), note, body(13.0), if *is_err { pal.gold } else { pal.dim }, room, Align2::LEFT_CENTER);
    } else if let (Some(i), Modal::None) = (page.sel, &page.modal)
        && let Some(words) = row_words(page, i)
    {
        text(ui, pos2(rect.min.x, mid), &words, body(13.0), pal.dim, room, Align2::LEFT_CENTER);
    }
}

// ── Modals ────────────────────────────────────────────────────────────────

/// The period list, dropped under the pill: every period with data, the
/// current one marked, the keys' cursor on the accent row. A click on a
/// row picks it; a click anywhere else closes the list.
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
    let pos = pos2((anchor.max.x - width).max(full.min.x + PAD.x), anchor.max.y + 6.0);
    let max_h = (full.max.y - pos.y - 16.0 - if foot.is_some() { 28.0 } else { 0.0 }).max(ROW_H * 3.0);
    let ctx = ui.ctx().clone();
    let shown = egui::Area::new(Id::new("stats-period-list"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain_to(full)
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(pal.popup)
                .stroke(Stroke::new(1.0, pal.accent.gamma_multiply(0.6)))
                .corner_radius(CornerRadius::same(RADIUS))
                .inner_margin(egui::Margin::same(6))
                .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(120) })
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let moved = ui.ctx().data(|d| d.get_temp::<usize>(Id::new("stats-period-cursor"))) != Some(cursor);
                    ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                        for (i, option) in options.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                            let on = i == cursor;
                            let radius = CornerRadius::same(8);
                            if on {
                                ui.painter().rect_filled(rect, radius, pal.accent);
                                if moved {
                                    response.scroll_to_me(None);
                                }
                            } else if response.hovered() {
                                ui.painter().rect_filled(rect, radius, Color32::from_white_alpha(12));
                            }
                            if option.is(&page.period.period, page.period.offset) {
                                let dot = pos2(rect.min.x + 14.0, rect.center().y);
                                ui.painter().circle_filled(dot, 3.5, if on { pal.on_accent } else { pal.accent });
                            }
                            let name_color = if on { pal.on_accent } else { pal.text };
                            let name = text(ui, pos2(rect.min.x + 28.0, rect.center().y), &option.name, strong(14.0), name_color, rect.width() - 40.0, Align2::LEFT_CENTER);
                            if let Some(detail) = &option.detail {
                                let room = rect.max.x - 12.0 - name.max.x - 12.0;
                                if room > 30.0 {
                                    let color = if on { pal.on_accent } else { pal.dim };
                                    text(ui, pos2(rect.max.x - 12.0, rect.center().y), detail, body(12.5), color, room, Align2::RIGHT_CENTER);
                                }
                            }
                            if response.clicked() {
                                acts.push(Act::PeriodPick(i));
                            }
                        }
                    });
                    if let Some(foot) = &foot {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                        text(ui, pos2(rect.min.x + 28.0, rect.center().y + 2.0), foot, body(12.0), pal.faint, rect.width() - 36.0, Align2::LEFT_CENTER);
                    }
                    ui.ctx().data_mut(|d| d.insert_temp(Id::new("stats-period-cursor"), cursor));
                });
        });
    let list = shown.response.rect;
    let outside = ui.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !list.contains(p) && !anchor.contains(p)));
    if outside {
        acts.push(Act::PeriodClose);
    }
    // A click on the pill toggles the list shut (the page's PeriodList).
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
            ui.painter().rect_filled(full, 0.0, Color32::from_black_alpha(140));
            let card_w = 480.0_f32.min(full.width() - 40.0);
            let card_rect = Rect::from_center_size(full.center(), vec2(card_w, 200.0));
            ui.painter().rect_filled(card_rect, CornerRadius::same(RADIUS), pal.popup);
            ui.painter().rect_stroke(card_rect, CornerRadius::same(RADIUS), Stroke::new(1.0, pal.gold.gamma_multiply(0.7)), StrokeKind::Inside);
            let inner = card_rect.shrink(22.0);
            text(ui, inner.min, &t!("sta.forget_title"), strong(18.0), pal.gold, inner.width(), Align2::LEFT_TOP);
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(inner.min + vec2(0.0, 34.0), pos2(inner.max.x, inner.max.y - 40.0))));
            child.label(egui::RichText::new(words.unwrap_or_default()).font(body(14.0)).color(pal.text));
            child.add_space(8.0);
            child.label(egui::RichText::new(t!("sta.forget_fact")).font(body(13.0)).color(pal.gold));
            let go = t!("sta.forget_go").to_string();
            let keep = t!("sta.forget_keep").to_string();
            let go_w = width_of(ui, &go, strong(14.0)) + 32.0;
            let keep_w = width_of(ui, &keep, strong(14.0)) + 32.0;
            let go_rect = Rect::from_min_size(pos2(inner.max.x - go_w, inner.max.y - 32.0), vec2(go_w, 32.0));
            let keep_rect = Rect::from_min_size(pos2(go_rect.min.x - 10.0 - keep_w, go_rect.min.y), vec2(keep_w, 32.0));
            let kept = button(ui, Id::new("stats-forget-keep"), keep_rect, &keep, true, pal.accent, pal).clicked();
            let gone = button(ui, Id::new("stats-forget-go"), go_rect, &go, false, pal.gold, pal).clicked();
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
    fn the_period_pill_drops_the_list_and_a_click_off_it_closes_it() {
        let _en = english();
        let ctx = context();
        let mut page = ready().hosted();
        let _ = pass(&ctx, &mut page, SMALL, Vec::new());
        let pill = ctx.read_response(Id::new("stats-period")).expect("the period pill was drawn");
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
    fn the_tiles_figures_are_the_cells_pages_words() {
        // What the tiles say is decided once, in `tiles`: the egui page
        // lays the same words out, so the two faces cannot disagree.
        let _en = english();
        let ctx = context();
        let mut page = ready().hosted();
        let out = pass(&ctx, &mut page, SMALL, Vec::new());
        let data = page.data.as_ref().expect("loaded");
        let versus = versus_label(&page.shown, &page.options);
        let words = tiles(&data.summary, data.previous.as_ref(), versus.as_deref());
        let drawn: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        for (value, label, _) in &words {
            assert!(drawn.iter().any(|d| d == value), "{value} drawn: {drawn:?}");
            assert!(drawn.iter().any(|d| d == label), "{label} drawn");
        }
    }
}
