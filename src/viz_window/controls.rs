//! The window's controls (docs/ux-contracts/visualizer-window.md, clauses
//! 9–15): a bar along the bottom — the arrows either side of the preset
//! dropdown, the tuning toggle, fullscreen — and the tuning panel on the
//! right, the mobile app's: the response curve every preset hears, and the
//! knobs the preset in front declares.
//!
//! The bar shows while the pointer moves and fades once it rests, so the
//! picture is the picture. Nothing here touches the GPU or the window: a
//! pass takes egui's input, changes the [`Tuning`] in place and returns
//! what the window must do — which is what lets the tests click the bar
//! without a display.

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Area, Button, Color32, ComboBox, CornerRadius, Frame, Id, Layout, Margin, Order, Panel,
    Pos2, Rect, Response, RichText, ScrollArea, Sense, Shadow, Slider, Stroke, Ui, Vec2, pos2, vec2,
};
use rust_i18n::t;

use crate::config::VisualizerPrefs;
use crate::shader::audio::Curve;
use crate::shader::library::BUILTIN;
use crate::shader::preset::{Param, Preset};

/// Seconds the bar stays after the pointer last moved (clause 10).
const LINGER: f64 = 2.5;
/// Seconds the bar takes to fade either way.
const FADE: f32 = 0.18;
/// The tuning panel's width in points: the mobile panel's.
const PANEL_WIDTH: f32 = 340.0;
/// The dropdown's width: the longest title but one shown whole, that one
/// cut with an ellipsis.
const PICK_WIDTH: f32 = 250.0;
/// How tall the dropdown's list may grow before it scrolls: every preset
/// the library has, and room for more.
const LIST_HEIGHT: f32 = 400.0;
/// An icon button's side.
const ICON: f32 = 32.0;

/// The response curve's ranges — the mobile panel's (`_globalRanges` in
/// mstream_music's visualizer_screen.dart).
pub const FLOOR: RangeInclusive<f32> = -120.0..=-50.0;
pub const CEILING: RangeInclusive<f32> = -60.0..=-5.0;
pub const SMOOTHING: RangeInclusive<f32> = 0.0..=0.95;
/// The least room the floor and ceiling sliders leave between them. The
/// texture refuses an empty window and keeps the last; here the sliders
/// never make one, so what the panel shows is what the texture uses.
const MIN_WINDOW: f32 = 1.0;

// The kit's canvas colours (src/kit/theme.rs), so the window reads as the
// player's; the dim one a step lighter, to hold up over a bright preset.
const GROUND: Color32 = Color32::from_rgba_unmultiplied_const(0x12, 0x13, 0x1c, 0xe6);
/// The panel's ground: a sheet the window's height, so a shade more of the
/// picture shows through it (the mobile panel's is black at 190/255).
const SHEET: Color32 = Color32::from_rgba_unmultiplied_const(0x12, 0x13, 0x1c, 0xd4);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xde, 0xe9);
const DIM: Color32 = Color32::from_rgb(0x8a, 0x92, 0xae);
const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xab, 0xdf);
const ON_ACCENT: Color32 = Color32::from_rgb(0x0d, 0x10, 0x17);
const EDGE: Color32 = Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 0x14);

/// One preset as the controls offer it.
pub struct Entry {
    /// The library's file name: how the choices are saved.
    pub file: &'static str,
    /// The file's own title.
    pub title: String,
    /// "03 Plasma Pulse": the number both apps name a preset by, and its
    /// title — the dropdown's row and the window's title.
    pub label: String,
    /// The `// param:` knobs, in `iParams[]` order.
    pub params: Vec<Param>,
    /// This GPU would not draw it: listed, never offered (clause 11).
    pub refused: bool,
}

/// Every preset the library carries, as the controls offer them — none
/// refused until this GPU has been asked.
pub fn entries() -> Vec<Entry> {
    BUILTIN
        .iter()
        .map(|builtin| {
            let preset = Preset::parse(builtin.source).ok();
            let title =
                preset.as_ref().and_then(|p| p.title.clone()).unwrap_or_else(|| builtin.file.to_string());
            Entry {
                file: builtin.file,
                label: format!("{} {title}", &builtin.file[..2]),
                title,
                params: preset.map(|p| p.params).unwrap_or_default(),
                refused: false,
            }
        })
        .collect()
}

/// What the tuning panel turns.
#[derive(Debug, Clone, PartialEq)]
pub struct Tuning {
    pub curve: Curve,
    /// Per entry, its knobs' values in `// param:` order.
    pub knobs: Vec<Vec<f32>>,
}

impl Tuning {
    /// The saved choices over the files' defaults, each held to its range
    /// (clause 14): a hand-edited value past a slider's end is where the
    /// slider would put it, and a knob the file no longer declares is
    /// dropped.
    pub fn from_prefs(prefs: &VisualizerPrefs, entries: &[Entry]) -> Tuning {
        let knobs = entries
            .iter()
            .map(|entry| {
                let saved = prefs.knobs.get(entry.file);
                entry
                    .params
                    .iter()
                    .map(|param| {
                        let value = saved.and_then(|s| s.get(&param.name)).copied().unwrap_or(param.default);
                        within(value, param.min..=param.max, param.default)
                    })
                    .collect()
            })
            .collect();
        Tuning { curve: curve_from(prefs), knobs }
    }

    /// Entry `i`'s knobs that sit away from the file's defaults: what the
    /// player keeps (an empty list forgets the preset's line).
    pub fn turned(&self, entries: &[Entry], i: usize) -> Vec<(String, f32)> {
        entries[i]
            .params
            .iter()
            .zip(&self.knobs[i])
            .filter(|(param, value)| **value != param.default)
            .map(|(param, value)| (param.name.clone(), *value))
            .collect()
    }

    /// The panel's Reset: the curve, and the preset in front's knobs, back
    /// to the defaults — the mobile panel's reach, no further.
    fn reset(&mut self, entries: &[Entry], i: usize) {
        self.curve = Curve::default();
        self.knobs[i] = entries[i].params.iter().map(|p| p.default).collect();
    }
}

/// The curve the saved choices make, each value held to its slider's
/// range, and the calibrated window where the floor and ceiling leave no
/// room between them. The window draws with this and the player builds its
/// texture with this: one function, so the two cannot disagree.
pub fn curve_from(prefs: &VisualizerPrefs) -> Curve {
    let default = Curve::default();
    let min_db = within(prefs.min_db.unwrap_or(default.min_db), FLOOR, default.min_db);
    let max_db = within(prefs.max_db.unwrap_or(default.max_db), CEILING, default.max_db);
    let smoothing = within(prefs.smoothing.unwrap_or(default.smoothing), SMOOTHING, default.smoothing);
    if max_db - min_db < MIN_WINDOW {
        return Curve { smoothing, ..default };
    }
    Curve { min_db, max_db, smoothing }
}

/// Clamped into `range`; a value that is not a number is the fallback.
fn within(value: f32, range: RangeInclusive<f32>, fallback: f32) -> f32 {
    if value.is_nan() { fallback } else { value.clamp(*range.start(), *range.end()) }
}

/// What the bar asks of the window, which owns the window and the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `‹` or `›`: the next preset that draws, that way.
    Step(isize),
    /// A row of the dropdown, by entry.
    Pick(usize),
    Fullscreen,
}

/// What the controls need to know that they do not own.
pub struct View<'a> {
    pub entries: &'a [Entry],
    /// The preset in front, by entry.
    pub current: usize,
    pub fullscreen: bool,
}

/// The controls' own state between passes.
pub struct Controls {
    /// The tuning panel (clause 12).
    pub tuning_open: bool,
    /// The player's `[gui] key_hints`: whether a tooltip keeps its
    /// ` — key` tail, the GUI's rule for its own tooltips.
    pub key_hints: bool,
    /// egui's clock when something last woke the bar.
    woke: f64,
    /// A wake asked for between passes — the window opening, a key that
    /// changed the preset — taken at the next pass's time.
    wake_pending: bool,
    /// Whether a dropdown was open when the last pass ended: Esc closes it
    /// before anything else (clause 15).
    popup_open: bool,
    /// Where the pointer was in the last pass: a move is a change of it.
    pointer: Option<Pos2>,
    /// Where the bar was drawn, so a pointer resting on it keeps it.
    bar: Option<Rect>,
    /// Whether the bar showed in the last pass.
    showing: bool,
    /// Where the widgets landed in the last pass.
    pub(crate) seen: Seen,
}

/// The widgets' rectangles from the last pass: what the tests click.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default, Debug)]
pub(crate) struct Seen {
    pub prev: Option<Rect>,
    pub next: Option<Rect>,
    pub pick: Option<Rect>,
    pub rows: Vec<Rect>,
    pub tune: Option<Rect>,
    pub full: Option<Rect>,
    pub reset: Option<Rect>,
    pub close: Option<Rect>,
    pub sliders: Vec<Rect>,
}

impl Default for Controls {
    fn default() -> Self {
        Controls {
            tuning_open: false,
            key_hints: false,
            woke: f64::NEG_INFINITY,
            // A window opens with its controls showing, so they are found.
            wake_pending: true,
            popup_open: false,
            pointer: None,
            bar: None,
            showing: false,
            seen: Seen::default(),
        }
    }
}

impl Controls {
    /// Show the bar for a while — after a key changed the preset, so the
    /// new name is seen even in fullscreen.
    pub fn wake(&mut self) {
        self.wake_pending = true;
    }

    /// `t` (clause 15).
    pub fn toggle_tuning(&mut self) {
        self.tuning_open = !self.tuning_open;
        self.wake();
    }

    /// Esc, innermost first: an open dropdown (egui closes it on the same
    /// key), then the tuning panel. `false` means neither was open, and the
    /// key is the window's (clause 15).
    pub fn escape(&mut self) -> bool {
        if self.popup_open {
            return true;
        }
        if self.tuning_open {
            self.tuning_open = false;
            return true;
        }
        false
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn showing(&self) -> bool {
        self.showing
    }

    /// A tooltip that names its key, the GUI's way (`kit::Ui::tip_keyed`):
    /// the ` — key` tail only while the player shows key hints.
    fn keyed(&self, tip: impl Into<String>) -> String {
        let tip = tip.into();
        match tip.rsplit_once(" — ") {
            Some((label, _)) if !self.key_hints => label.to_string(),
            _ => tip,
        }
    }
}

/// The look, once per window: egui's dark theme in the kit's colours, the
/// text a size up from egui's — read over a moving picture, often from a
/// sofa.
pub fn style(ctx: &egui::Context) {
    use egui::{FontId, TextStyle};
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.slider_rail_height = 4.0;
        style.spacing.button_padding = vec2(8.0, 4.0);
        let visuals = &mut style.visuals;
        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = Stroke::new(1.0, ON_ACCENT);
        visuals.slider_trailing_fill = true;
        visuals.window_fill = GROUND.to_opaque();
        visuals.window_stroke = Stroke::new(1.0, EDGE);
        visuals.panel_fill = GROUND;
        let widgets = &mut visuals.widgets;
        widgets.inactive.weak_bg_fill = Color32::from_white_alpha(14);
        widgets.inactive.bg_fill = Color32::from_white_alpha(38);
        widgets.hovered.weak_bg_fill = Color32::from_white_alpha(28);
        widgets.active.weak_bg_fill = Color32::from_white_alpha(40);
        for state in [&mut widgets.inactive, &mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            state.fg_stroke.color = TEXT;
            state.corner_radius = CornerRadius::same(8);
        }
        widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.6));
        widgets.open.weak_bg_fill = Color32::from_white_alpha(28);
    });
}

/// One pass of the controls over the whole window. The tuning is changed
/// in place (the caller compares it to see what moved); everything else the
/// window must do comes back as commands.
pub fn show(ui: &mut Ui, controls: &mut Controls, view: &View, tuning: &mut Tuning) -> Vec<Command> {
    let ctx = ui.ctx().clone();
    controls.seen = Seen::default();
    let (now, pressed, pointer) = ctx.input(|i| {
        let pressed = i.pointer.any_pressed() || i.pointer.any_down() || i.smooth_scroll_delta != Vec2::ZERO;
        (i.time, pressed, i.pointer.hover_pos())
    });
    // Arriving counts as moving: egui gives a pointer's first position no
    // delta.
    let moved = pointer.is_some() && pointer != controls.pointer;
    controls.pointer = pointer;
    if moved || pressed || std::mem::take(&mut controls.wake_pending) {
        controls.woke = now;
    }
    let resting_on_bar = pointer.zip(controls.bar).is_some_and(|(p, bar)| bar.expand(8.0).contains(p));
    let wanted = controls.tuning_open
        || controls.popup_open
        || resting_on_bar
        || ctx.egui_is_using_pointer()
        || now - controls.woke < LINGER;
    let opacity = ctx.animate_bool_with_time(Id::new("viz-bar-fade"), wanted, FADE);
    controls.showing = opacity > 0.0;
    // Fullscreen, the resting pointer goes with the bar (clause 10).
    if view.fullscreen && !controls.showing && pointer.is_some() {
        ctx.set_cursor_icon(egui::CursorIcon::None);
    }

    let mut commands = Vec::new();
    // The panel first: the bar centres itself on what the panel leaves.
    let beside = panel(ui, controls, view, tuning);
    controls.bar = None;
    if controls.showing {
        bar(&ctx, controls, view, opacity, beside, &mut commands);
    }
    controls.popup_open = ctx.any_popup_open();
    commands
}

/// The bar along the bottom (clause 10), centred on the width the tuning
/// panel leaves, `beside` being the panel's.
fn bar(
    ctx: &egui::Context,
    controls: &mut Controls,
    view: &View,
    opacity: f32,
    beside: f32,
    commands: &mut Vec<Command>,
) {
    let current = &view.entries[view.current];
    let shown = Area::new(Id::new("viz-bar"))
        .anchor(Align2::CENTER_BOTTOM, vec2(-beside / 2.0, -18.0))
        .order(Order::Foreground)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            plate().inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // The dropdown stands as tall as the icon buttons beside it.
                    ui.spacing_mut().interact_size.y = ICON;
                    let prev = icon_button(ui, Icon::Prev, false, controls.keyed(t!("gui.viz.prev_tip")));
                    if prev.clicked() {
                        commands.push(Command::Step(-1));
                    }
                    let picked = ComboBox::from_id_salt("viz-preset")
                        .selected_text(RichText::new(&current.label).color(TEXT))
                        .width(PICK_WIDTH)
                        .height(LIST_HEIGHT)
                        .truncate()
                        .show_ui(ui, |ui| {
                            let mut picked = None;
                            for (i, entry) in view.entries.iter().enumerate() {
                                let row = Button::selectable(i == view.current, entry.label.as_str());
                                let row = ui.add_enabled(!entry.refused, row);
                                controls.seen.rows.push(row.rect);
                                let row = row.on_disabled_hover_text(t!("gui.viz.refused"));
                                if row.clicked() && i != view.current {
                                    picked = Some(i);
                                }
                            }
                            picked
                        });
                    let pick = picked.response.on_hover_text(t!("gui.viz.pick_tip"));
                    if let Some(Some(i)) = picked.inner {
                        commands.push(Command::Pick(i));
                    }
                    let next = icon_button(ui, Icon::Next, false, controls.keyed(t!("gui.viz.next_tip")));
                    if next.clicked() {
                        commands.push(Command::Step(1));
                    }
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    let tip = controls.keyed(t!("gui.viz.tuning_tip"));
                    let tune = icon_button(ui, Icon::Tune, controls.tuning_open, tip);
                    if tune.clicked() {
                        controls.tuning_open = !controls.tuning_open;
                    }
                    let (icon, tip) = if view.fullscreen {
                        (Icon::Windowed, t!("gui.viz.windowed_tip"))
                    } else {
                        (Icon::Fullscreen, t!("gui.viz.fullscreen_tip"))
                    };
                    let full = icon_button(ui, icon, false, controls.keyed(tip));
                    if full.clicked() {
                        commands.push(Command::Fullscreen);
                    }
                    let seen = &mut controls.seen;
                    (seen.prev, seen.pick, seen.next) = (Some(prev.rect), Some(pick.rect), Some(next.rect));
                    (seen.tune, seen.full) = (Some(tune.rect), Some(full.rect));
                });
            });
        });
    controls.bar = Some(shown.response.rect);
}

/// The tuning panel (clause 12): the mobile app's, a sheet down the right
/// edge the window's height, sliding in and out, its rows scrolling where
/// the window is short. Returns how wide it stands this pass.
fn panel(ui: &mut Ui, controls: &mut Controls, view: &View, tuning: &mut Tuning) -> f32 {
    let entry = &view.entries[view.current];
    let sheet = Frame::new().fill(SHEET).stroke(Stroke::new(1.0, EDGE)).inner_margin(Margin::same(16));
    let mut open = controls.tuning_open;
    let shown = Panel::right(Id::new("viz-tuning"))
        .exact_size(PANEL_WIDTH)
        .resizable(false)
        .show_separator_line(false)
        .frame(sheet)
        .show_collapsible(ui, &mut open, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(t!("gui.viz.tuning")).size(18.0).strong().color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = icon_button(ui, Icon::Close, false, controls.keyed(t!("gui.viz.close_tip")));
                    if close.clicked() {
                        controls.tuning_open = false;
                    }
                    let reset = ui.button(RichText::new(t!("gui.viz.reset")).color(ACCENT));
                    if reset.clicked() {
                        tuning.reset(view.entries, view.current);
                    }
                    (controls.seen.close, controls.seen.reset) = (Some(close.rect), Some(reset.rect));
                });
            });
            ui.add_space(6.0);
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                heading(ui, &t!("gui.viz.curve"));
                // Each end of the dB window stops short of the other.
                let curve = &mut tuning.curve;
                let floor = *FLOOR.start()..=FLOOR.end().min(curve.max_db - MIN_WINDOW);
                let ceiling = CEILING.start().max(curve.min_db + MIN_WINDOW)..=*CEILING.end();
                let rows = [
                    (t!("gui.viz.floor"), t!("gui.viz.floor_tip"), &mut curve.min_db, floor),
                    (t!("gui.viz.ceiling"), t!("gui.viz.ceiling_tip"), &mut curve.max_db, ceiling),
                    (t!("gui.viz.smoothing"), t!("gui.viz.smoothing_tip"), &mut curve.smoothing, SMOOTHING),
                ];
                for (label, tip, value, range) in rows {
                    let slider = slider_row(ui, &label, Some(&tip), value, range);
                    controls.seen.sliders.push(slider.rect);
                }
                ui.add_space(10.0);
                heading(ui, &entry.title);
                if entry.params.is_empty() {
                    ui.label(RichText::new(t!("gui.viz.no_knobs")).color(DIM));
                }
                for (param, value) in entry.params.iter().zip(tuning.knobs[view.current].iter_mut()) {
                    let slider = slider_row(ui, &param.name, None, value, param.min..=param.max);
                    controls.seen.sliders.push(slider.rect);
                }
            });
        });
    shown.map_or(0.0, |shown| shown.response.rect.width())
}

/// The translucent plate the bar and the panel sit on.
fn plate() -> Frame {
    Frame::new()
        .fill(GROUND)
        .stroke(Stroke::new(1.0, EDGE))
        .corner_radius(CornerRadius::same(12))
        .shadow(Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(90) })
}

/// A section's name, the mobile panel's way: small capitals in the accent.
fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text.to_uppercase()).size(11.5).strong().color(ACCENT));
    ui.add_space(2.0);
}

/// The mobile panel's row: the name, its value on the right, and the
/// slider under them the panel's width. The name's tooltip, where it has
/// one, says which way to turn it.
fn slider_row(
    ui: &mut Ui,
    label: &str,
    tip: Option<&str>,
    value: &mut f32,
    range: RangeInclusive<f32>,
) -> Response {
    ui.horizontal(|ui| {
        let name = ui.label(RichText::new(label).color(TEXT));
        if let Some(tip) = tip {
            name.on_hover_text(tip);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(format!("{value:.2}")).monospace().color(DIM));
        });
    });
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = ui.available_width();
        ui.add(Slider::new(value, range).show_value(false))
    })
    .inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Prev,
    Next,
    Tune,
    Fullscreen,
    Windowed,
    Close,
}

/// A square button with a drawn icon — drawn rather than a glyph, so no
/// font has to carry it — its tooltip naming the key.
fn icon_button(ui: &mut Ui, icon: Icon, on: bool, tip: impl Into<egui::WidgetText>) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON), Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = if on {
            ACCENT
        } else if response.is_pointer_button_down_on() {
            Color32::from_white_alpha(40)
        } else if response.hovered() {
            Color32::from_white_alpha(24)
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(8), fill);
        let colour = if on { ON_ACCENT } else { TEXT };
        let ink = Stroke::new(1.8, colour);
        let c = rect.center();
        let at = |x: f32, y: f32| pos2(c.x + x, c.y + y);
        match icon {
            Icon::Prev => {
                painter.line(vec![at(3.0, -7.0), at(-4.0, 0.0), at(3.0, 7.0)], ink);
            }
            Icon::Next => {
                painter.line(vec![at(-3.0, -7.0), at(4.0, 0.0), at(-3.0, 7.0)], ink);
            }
            // The mobile app's `Icons.tune`: three rails, a knob on each.
            Icon::Tune => {
                for (y, knob) in [(-5.0, -3.0), (0.0, 3.5), (5.0, -0.5)] {
                    painter.line_segment([at(-7.5, y), at(7.5, y)], ink);
                    painter.circle_filled(at(knob, y), 2.6, colour);
                }
            }
            Icon::Fullscreen => corners(painter, c, true, ink),
            Icon::Windowed => corners(painter, c, false, ink),
            Icon::Close => {
                painter.line_segment([at(-5.5, -5.5), at(5.5, 5.5)], ink);
                painter.line_segment([at(-5.5, 5.5), at(5.5, -5.5)], ink);
            }
        }
    }
    response.on_hover_text(tip)
}

/// Four corner marks round `c`. Out (fullscreen): each mark's apex at a
/// corner of the square, its arms back along the edges. In (back to a
/// window): each apex drawn in toward the middle, its arms reaching out.
fn corners(painter: &egui::Painter, c: Pos2, out: bool, ink: Stroke) {
    const SIDE: f32 = 7.0;
    const ARM: f32 = 4.0;
    for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
        let reach = if out { SIDE } else { SIDE - ARM };
        let apex = pos2(c.x + sx * reach, c.y + sy * reach);
        let toward = if out { -ARM } else { ARM };
        let ends = (pos2(apex.x + sx * toward, apex.y), pos2(apex.x, apex.y + sy * toward));
        painter.line(vec![ends.0, apex, ends.1], ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use egui::{Event, Modifiers, PointerButton, RawInput};

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }

    /// A window with egui in it and no display: each pass is one frame,
    /// a sixtieth of a second after the last.
    struct Rig {
        /// The labels' widths move the widgets: no other test may switch
        /// the language under this one's clicks.
        _locale: std::sync::MutexGuard<'static, ()>,
        ctx: egui::Context,
        controls: Controls,
        tuning: Tuning,
        entries: Vec<Entry>,
        current: usize,
        fullscreen: bool,
        time: f64,
    }

    impl Rig {
        fn new() -> Rig {
            let locale = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            rust_i18n::set_locale("en");
            let entries = entries();
            let tuning = Tuning::from_prefs(&VisualizerPrefs::default(), &entries);
            Rig {
                _locale: locale,
                ctx: super::super::overlay::context(),
                controls: Controls::default(),
                tuning,
                entries,
                current: 0,
                fullscreen: false,
                time: 0.0,
            }
        }

        fn pass(&mut self, events: Vec<Event>) -> (Vec<Command>, egui::FullOutput) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(960.0, 540.0))),
                time: Some(self.time),
                events,
                ..RawInput::default()
            };
            let mut commands = Vec::new();
            let view = View { entries: &self.entries, current: self.current, fullscreen: self.fullscreen };
            let (controls, tuning) = (&mut self.controls, &mut self.tuning);
            let mut output = self.ctx.run_ui(input, |ui| commands = show(ui, controls, &view, tuning));
            // No GPU to hand the font atlas to; egui insists it be said so.
            output.textures_delta.clear();
            self.time += 1.0 / 60.0;
            (commands, output)
        }

        /// Let the fades run out and a new area measure itself.
        fn settle(&mut self) {
            for _ in 0..30 {
                self.pass(Vec::new());
            }
        }

        /// A click: the pointer arrives, presses, lets go.
        fn click(&mut self, at: Pos2) -> Vec<Command> {
            self.pass(vec![Event::PointerMoved(at)]);
            let (mut commands, _) = self.pass(vec![button(at, true)]);
            commands.extend(self.pass(vec![button(at, false)]).0);
            commands.extend(self.pass(Vec::new()).0);
            commands
        }

        /// A drag from one point to another, let go there.
        fn drag(&mut self, from: Pos2, to: Pos2) {
            self.pass(vec![Event::PointerMoved(from)]);
            self.pass(vec![button(from, true)]);
            self.pass(vec![Event::PointerMoved(to)]);
            self.pass(vec![button(to, false)]);
        }
    }

    #[test]
    fn the_arrows_and_the_fullscreen_mark_ask_the_window() {
        let mut rig = Rig::new();
        rig.settle();
        let next = rig.controls.seen.next.expect("the bar shows at open").center();
        assert_eq!(rig.click(next), [Command::Step(1)]);
        let prev = rig.controls.seen.prev.unwrap().center();
        assert_eq!(rig.click(prev), [Command::Step(-1)]);
        let full = rig.controls.seen.full.unwrap().center();
        assert_eq!(rig.click(full), [Command::Fullscreen]);
        let tune = rig.controls.seen.tune.unwrap().center();
        assert!(rig.click(tune).is_empty(), "the tuning mark is the controls' own");
        assert!(rig.controls.tuning_open);
    }

    #[test]
    fn the_dropdown_lists_every_preset_and_a_row_picks_one() {
        let mut rig = Rig::new();
        rig.entries[5].refused = true;
        rig.settle();
        let pick = rig.controls.seen.pick.unwrap().center();
        assert!(rig.click(pick).is_empty(), "opening the list is not a pick");
        assert!(rig.controls.popup_open, "the list is open");
        assert_eq!(rig.controls.seen.rows.len(), BUILTIN.len(), "every preset is a row");

        // A row this GPU refused is listed, and a click on it picks nothing.
        let refused = rig.controls.seen.rows[5].center();
        assert!(rig.click(refused).is_empty());

        if !rig.controls.popup_open {
            let pick = rig.controls.seen.pick.unwrap().center();
            rig.click(pick);
        }
        let third = rig.controls.seen.rows[2].center();
        assert_eq!(rig.click(third), [Command::Pick(2)]);
        assert!(!rig.controls.popup_open, "a pick closes the list");
    }

    #[test]
    fn esc_closes_the_innermost_thing_first() {
        let mut rig = Rig::new();
        rig.settle();
        let pick = rig.controls.seen.pick.unwrap().center();
        rig.click(pick);
        rig.controls.toggle_tuning();
        rig.settle();
        assert!(rig.controls.popup_open && rig.controls.tuning_open);
        assert!(rig.controls.escape(), "the open list takes the first Esc");
        let esc = Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        rig.pass(vec![esc]);
        rig.pass(Vec::new());
        assert!(!rig.controls.popup_open, "egui closed it on the same key");
        assert!(rig.controls.escape(), "the panel takes the second");
        assert!(!rig.controls.tuning_open);
        assert!(!rig.controls.escape(), "and the third is the window's");
    }

    #[test]
    fn the_bar_shows_at_open_rests_away_and_wakes_on_the_pointer() {
        let mut rig = Rig::new();
        rig.settle();
        assert!(rig.controls.showing(), "a new window shows its controls");
        rig.time += LINGER + 1.0;
        rig.settle();
        assert!(!rig.controls.showing(), "and puts them away when nothing moves");
        rig.pass(vec![Event::PointerMoved(pos2(480.0, 270.0))]);
        rig.settle();
        assert!(rig.controls.showing(), "the pointer brings them back");

        // A key that changed the preset wakes the bar too, to show its name.
        rig.time += LINGER + 1.0;
        rig.settle();
        assert!(!rig.controls.showing());
        rig.controls.wake();
        rig.settle();
        assert!(rig.controls.showing());

        // A pointer resting on the bar keeps it.
        let on_bar = rig.controls.seen.pick.unwrap().center();
        rig.pass(vec![Event::PointerMoved(on_bar)]);
        rig.time += LINGER + 1.0;
        rig.settle();
        assert!(rig.controls.showing());
    }

    #[test]
    fn fullscreen_the_resting_pointer_hides_with_the_bar() {
        let mut rig = Rig::new();
        rig.fullscreen = true;
        rig.pass(vec![Event::PointerMoved(pos2(100.0, 100.0))]);
        rig.settle();
        rig.time += LINGER + 1.0;
        rig.settle();
        let (_, output) = rig.pass(Vec::new());
        assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::None);

        rig.fullscreen = false;
        let (_, output) = rig.pass(Vec::new());
        assert_ne!(output.platform_output.cursor_icon, egui::CursorIcon::None, "a window keeps its pointer");
    }

    #[test]
    fn a_tooltip_names_its_key_only_with_the_players_key_hints_on() {
        let mut controls = Controls::default();
        assert_eq!(controls.keyed("Next preset — →"), "Next preset");
        assert_eq!(controls.keyed("Choose a preset"), "Choose a preset");
        controls.key_hints = true;
        assert_eq!(controls.keyed("Next preset — →"), "Next preset — →");
    }

    #[test]
    fn the_panel_turns_the_curve_and_the_knobs_and_resets_them() {
        let mut rig = Rig::new();
        rig.controls.toggle_tuning();
        rig.settle();
        // Three curve rows, then Spectrum Bars' two knobs.
        assert_eq!(rig.controls.seen.sliders.len(), 3 + 2);

        let smoothing = rig.controls.seen.sliders[2];
        rig.drag(smoothing.left_center(), smoothing.right_center() + vec2(40.0, 0.0));
        assert_eq!(rig.tuning.curve.smoothing, *SMOOTHING.end());

        let contrast = rig.controls.seen.sliders[3];
        rig.drag(contrast.center(), contrast.left_center() - vec2(40.0, 0.0));
        assert_eq!(rig.tuning.knobs[0][0], rig.entries[0].params[0].min);
        assert_eq!(rig.tuning.turned(&rig.entries, 0), [("contrast".to_string(), 0.5)]);

        let reset = rig.controls.seen.reset.unwrap().center();
        rig.click(reset);
        assert_eq!(rig.tuning.curve, Curve::default());
        assert!(rig.tuning.turned(&rig.entries, 0).is_empty());

        let close = rig.controls.seen.close.unwrap().center();
        rig.click(close);
        assert!(!rig.controls.tuning_open);
    }

    #[test]
    fn a_preset_without_knobs_says_so() {
        let mut rig = Rig::new();
        rig.entries[0].params.clear();
        rig.tuning.knobs[0].clear();
        rig.controls.toggle_tuning();
        rig.settle();
        assert_eq!(rig.controls.seen.sliders.len(), 3, "the curve's rows alone");
    }

    #[test]
    fn the_floor_and_ceiling_never_cross() {
        let mut rig = Rig::new();
        rig.tuning.curve = Curve { min_db: -80.0, max_db: -55.0, smoothing: 0.27 };
        rig.controls.toggle_tuning();
        rig.settle();
        // The floor dragged all the way up stops a window short of the ceiling.
        let floor = rig.controls.seen.sliders[0];
        rig.drag(floor.center(), floor.right_center() + vec2(60.0, 0.0));
        assert_eq!(rig.tuning.curve.min_db, -55.0 - MIN_WINDOW);
        // And the ceiling dragged all the way down stops a window above it.
        let ceiling = rig.controls.seen.sliders[1];
        rig.drag(ceiling.center(), ceiling.left_center() - vec2(60.0, 0.0));
        assert_eq!(rig.tuning.curve.max_db, -55.0, "held where the floor now stands");

        // From a low floor, the ceiling reaches its own end of the range;
        // then the floor stops short of it.
        rig.tuning.curve = Curve { min_db: -100.0, max_db: -20.0, smoothing: 0.27 };
        rig.settle();
        rig.drag(ceiling.center(), ceiling.left_center() - vec2(60.0, 0.0));
        assert_eq!(rig.tuning.curve.max_db, *CEILING.start());
        rig.settle();
        rig.drag(floor.center(), floor.right_center() + vec2(60.0, 0.0));
        assert_eq!(rig.tuning.curve.min_db, *CEILING.start() - MIN_WINDOW);
    }

    #[test]
    fn saved_choices_are_held_to_their_ranges_and_a_stale_knob_is_dropped() {
        let entries = entries();
        let mut prefs = VisualizerPrefs {
            min_db: Some(-500.0),
            max_db: Some(-10.0),
            smoothing: Some(f32::NAN),
            ..VisualizerPrefs::default()
        };
        let bars = prefs.knobs.entry("01-spectrum-bars.glsl".into()).or_default();
        bars.insert("bars".into(), 400.0);
        bars.insert("gone".into(), 1.0);
        let tuning = Tuning::from_prefs(&prefs, &entries);
        assert_eq!(tuning.curve, Curve { min_db: *FLOOR.start(), max_db: -10.0, smoothing: 0.27 });
        assert_eq!(tuning.knobs[0], [1.51, 96.0], "bars held to its 12–96, contrast at its default");
        assert_eq!(tuning.turned(&entries, 0), [("bars".to_string(), 96.0)]);
        assert!(tuning.knobs.iter().zip(&entries).all(|(knobs, e)| knobs.len() == e.params.len()));

        // A floor over the ceiling is no window at all: the defaults stand,
        // as the texture would have kept them.
        let crossed =
            VisualizerPrefs { min_db: Some(-50.0), max_db: Some(-55.0), ..VisualizerPrefs::default() };
        assert_eq!(curve_from(&crossed), Curve::default());
    }
}
