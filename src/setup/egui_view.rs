//! The wizard drawn by egui, in the desktop window's panel over the whole
//! grid (face.rs hosts it): the same [`Wizard`] the cells view draws (the
//! parent module: its state, its worker, its keys and its words), only the
//! drawing is new, in pixels, on the Studio kit (src/gui/window/studio.rs),
//! after the Claude Design canvas "Server Setup in egui" (its shell and its
//! boards).
//!
//! This module is the page's frame, which every screen but Done shares:
//! the top strip (the language chip where the cells show it, the step), the
//! centred column with the wordmark at the head of Folders, the status
//! line, the tips line, the rule and the bar (the scan, the way back and
//! the way forward). Each screen's body is a module of its own under it
//! (folders, extras, login), Done draws its whole page (done), and each
//! modal its own window over the kit's scrim (modals, and login's skip
//! gate).
//!
//! Every click is one of the wizard's own [`Act`]s, the one the cells view
//! registers for the same control, applied through [`Wizard::act`] at the
//! end of the pass; one that ends the wizard is handed to the host as the
//! input half's Quit is. Keys never reach egui: the wizard's keymap still
//! moves every cursor, edits every field and answers every modal, and egui
//! draws whatever the wizard holds. What the view keeps between passes
//! (where the table stands, the window over a field's line, the wordmark's
//! texture) it keeps in egui's memory under its own ids, and the one thing
//! egui cannot know, when a key was last pressed, in [`View`], which the
//! host holds; the wizard holds nothing for it.
//!
//! Built only with the window: the terminal build has no panel to draw in.

mod done;
mod extras;
mod folders;
mod login;
mod modals;

use std::time::{Duration, Instant};

use egui::epaint::StrokeKind;
use egui::{Align, Align2, CursorIcon, Id, Layout, Pos2, Rect, Response, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use rust_i18n::t;

use super::cells::footer_hint;
use super::{Act, LANGS, LOGO_PNG, Modal, Outcome, ScanWidget, Screen, Wizard};
use crate::gui::window::studio::{self, FORM_BUTTON_H, Pal, Para, body, mono};
use crate::kit::TIP_DELAY;

/// The cells' floor in points: 58 by 20 of the window's 8 by 16 point
/// cells. On a smaller page the frame gives way to the words the cells say
/// there.
const FLOOR: Vec2 = vec2(464.0, 320.0);
/// The column's widest: the boards' 720 points, the cells' 74 cells.
const COLUMN_W: f32 = 720.0;
/// The page's sides, the boards' 24 points.
const SIDE: f32 = 24.0;
/// The boards' measures hold on a page at least this tall; on a shorter
/// one the frame's margins and its bar tighten.
const ROOMY: f32 = 640.0;
/// The top strip's height, and one line of the foot's.
const STRIP_H: f32 = 28.0;
const LINE_H: f32 = 18.0;
/// The wordmark's box at its full height and the room under it (the
/// boards' 372 by 93 and 32), and what the column keeps for the screen
/// under it before the wordmark takes any: the add card and its gap, the
/// table's headings and three of its rows.
const LOGO_H: f32 = 93.0;
const LOGO_GAP: f32 = 32.0;
const LOGO_ROOM: f32 = 56.0 + 18.0 + 34.0 + 3.0 * 40.0;
/// The least the wordmark is drawn at: on a column with less room for it
/// the screen goes without.
const LOGO_LEAST: f32 = 24.0;
/// A tooltip's widest: the boards', the room for the cells' 40 cells of
/// words, which [`tip`] wraps them at.
const TIP_W: f32 = 300.0;

/// What the view keeps between frames that egui's memory cannot: when a
/// key was last pressed. Keys never reach egui; the host hears them
/// (face.rs), and the tooltips wait out the dwell after one ([`tip`]).
#[derive(Default)]
pub(super) struct View {
    keyed: Option<Instant>,
}

impl View {
    /// A key was pressed: every tooltip hides for the dwell after it, as
    /// the cells' surface hides its own on a key.
    pub(super) fn key(&mut self) {
        self.keyed = Some(Instant::now());
    }

    /// Whether a key was ever pressed, for the host's tests.
    #[cfg(test)]
    pub(super) fn key_pressed(&self) -> bool {
        self.keyed.is_some()
    }
}

/// What one pass of the view tells its host.
pub(super) struct Shown {
    /// The page in points: the pass's screen, which is the grid's cells.
    pub points: Rect,
    /// The pointer is over something a click acts on.
    pub hand: bool,
    /// The foot of the caret of the field with the keyboard, in points;
    /// none when no field has it.
    pub caret: Option<Pos2>,
    /// A click ended the wizard, as the input half's Quit does.
    pub outcome: Option<Outcome>,
}

impl Shown {
    /// Another run of the same frame's pass: egui may run it twice (a
    /// modal taking its body's height), and a click either run applied
    /// stands; the rest is the last run's.
    pub(super) fn and(self, next: Shown) -> Shown {
        Shown { outcome: self.outcome.or(next.outcome), ..next }
    }
}

/// What the frame hands each part of the page beside the wizard.
struct Page<'a> {
    pal: Pal,
    /// The input method's uncommitted text, for the field with the
    /// keyboard to draw (`studio::Input::focused`).
    composition: &'a str,
    /// Tooltips name the key that does the same, as the cells' do: the
    /// wizard's surface always says so ([`tip_keyed`]).
    key_hints: bool,
    /// How long ago a key was last pressed, if one was.
    since_key: Option<Duration>,
}

/// What a part of the page hands back to the frame. A click that ends the
/// wizard is an act like any other (the cells' Finish), and the hand is
/// egui's own, from the cursor each control asks for.
#[derive(Default)]
struct Drawn {
    /// What its clicks asked for: the cells' acts for the same controls,
    /// applied in this order at the pass's end.
    acts: Vec<Act>,
    /// The foot of the caret of its field with the keyboard, in points.
    caret: Option<Pos2>,
}

/// One pass of the wizard over the whole of `ui`, the window's grid, with
/// `composition` the input method's uncommitted text and `key_hints`
/// whether tooltips name their keys. The page is drawn, then the modal on
/// top of it, which leaves the page inert: what was clicked under it is
/// dropped and its field has the keys no more. The acts asked for are
/// applied; what the host needs is handed back.
pub(super) fn show(ui: &mut Ui, wizard: &mut Wizard, view: &View, composition: &str, key_hints: bool) -> Shown {
    let pal = studio::begin(ui);
    faces(ui.ctx());
    dwell(ui.ctx());
    let full = ui.max_rect();
    let page = Page { pal, composition, key_hints, since_key: view.keyed.map(|at| at.elapsed()) };
    let mut shown = Shown { points: full, hand: false, caret: None, outcome: None };
    // Under the cells' floor the page gives way to their words for it, and
    // nothing on it answers a click; the keys still act on what is hidden,
    // a modal's included, as they do in cells.
    if full.width() < FLOOR.x || full.height() < FLOOR.y {
        Para::new(&t!("resize"), body(13.0), page.pal.dim).paint(ui, full.min + vec2(12.0, 12.0), (full.width() - 24.0).max(1.0));
        return shown;
    }
    let mut drawn = match wizard.screen {
        Screen::Done => done::show(ui, wizard, &page),
        _ => frame(ui, wizard, &page),
    };
    if !matches!(wizard.modal, Modal::None) {
        drawn = match wizard.modal {
            Modal::Language(_) => modals::language(ui, full, wizard, &page),
            Modal::PathEntry(_) => modals::path(ui, full, wizard, &page),
            Modal::Browser(_) => modals::browser(ui, full, wizard, &page),
            Modal::SkipWarning => login::skip_gate(ui, full, wizard, &page),
            Modal::None => Drawn::default(),
        };
    }
    for act in drawn.acts {
        shown.outcome = shown.outcome.or(wizard.act(act));
    }
    shown.caret = drawn.caret.filter(|_| field_up(wizard));
    shown.hand = ui.ctx().output(|o| o.cursor_icon == CursorIcon::PointingHand);
    shown
}

/// Whether a field has the wizard's keyboard as its keys stand now: the
/// path modal's, a folder's name being edited, the login form's. A click
/// that closed a field in this pass took the keyboard with it, and no
/// caret is noted for a field that is gone.
fn field_up(wizard: &Wizard) -> bool {
    match &wizard.modal {
        Modal::PathEntry(_) => true,
        Modal::None => wizard.editing.is_some() || wizard.screen == Screen::Login,
        Modal::Language(_) | Modal::Browser(_) | Modal::SkipWarning => false,
    }
}

/// The faces this view draws with: the panel's, with a CJK face behind
/// them whatever the language (`panel::fonts_every_script`), since the
/// chip and the language list name 日本語 and 中文 in every language and a
/// click can switch the page to either. Set on the context's first pass
/// and again on a switch of language, which egui takes from its next pass.
fn faces(ctx: &egui::Context) {
    let id = Id::new("setup-faces");
    let lang = rust_i18n::locale().to_string();
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() == Some(lang.as_str()) {
        return;
    }
    crate::gui::window::panel::fonts_every_script(ctx);
    ctx.data_mut(|d| d.insert_temp(id, lang));
    ctx.request_repaint();
}

/// The cells' dwell for every tooltip of this view, set once on egui's
/// context (the kit's style shows one at once): [`TIP_DELAY`] at rest on
/// its target, and anew on the next target, as the cells' dwell restarts
/// on each.
fn dwell(ctx: &egui::Context) {
    let id = Id::new("setup-dwell");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.all_styles_mut(|style| {
        style.interaction.tooltip_delay = TIP_DELAY.as_secs_f32();
        style.interaction.tooltip_grace_time = 0.0;
    });
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// The cells' tooltip for `response`: `words` in the kit's box under the
/// target's middle, over it where there is no room under it, in the cells'
/// lines (wrapped at their 40 cells, `kit::wrap_tip`, a word wider than
/// that, a path, broken between its letters), no wider than the boards'
/// [`TIP_W`], once the pointer has rested on the target for the cells'
/// dwell, and never in the dwell after a key, which hides it as it hides
/// the cells'. The next pass is asked for when that dwell ends.
fn tip(page: &Page, response: &Response, words: &str) {
    if let Some(since) = page.since_key.filter(|since| *since < TIP_DELAY) {
        response.ctx.request_repaint_after(TIP_DELAY - since);
        return;
    }
    let mut tooltip = egui::Tooltip::for_enabled(response);
    tooltip.popup = tooltip.popup.align(egui::RectAlign::BOTTOM).align_alternatives(&[egui::RectAlign::TOP]).gap(8.0);
    let words = egui::RichText::new(crate::kit::wrap_tip(words).join("\n")).font(body(12.5)).color(page.pal.text);
    let _ = tooltip.show(|ui| {
        ui.set_max_width(TIP_W);
        ui.add(egui::Label::new(words));
    });
}

/// [`tip`] for words that end in " — key", the key that does the same:
/// whole while the wizard names its keys, cut at the dash where it does
/// not, as the cells' surface registers them (`Surface::tip_keyed`).
fn tip_keyed(page: &Page, response: &Response, words: &str) {
    let words = if page.key_hints { words } else { words.rsplit_once(" — ").map_or(words, |(label, _)| label) };
    tip(page, response, words);
}

// ── The frame ─────────────────────────────────────────────────────────────

/// Where the frame's parts stand on a page: the top strip, the column the
/// screen's body lays out in, the status line, the tips line, the rule and
/// the bar. The boards' measures on a page at least [`ROOMY`] tall (on 1280
/// by 800 the strip y 16 to 44, the column 720 wide from y 64, the status
/// line at 640, the tips at 664, the rule at 692, the bar 704 to 776); on a
/// shorter one the margins and the bar tighten, so the column keeps what
/// room there is. The column centres and narrows with the page, and the
/// foot stays on its bottom.
struct Parts {
    strip: Rect,
    column: Rect,
    status: Rect,
    tips: Rect,
    rule: Rect,
    bar: Rect,
}

fn parts(full: Rect) -> Parts {
    let roomy = full.height() >= ROOMY;
    let k = |at_ease: f32, tight: f32| if roomy { at_ease } else { tight };
    let (left, right) = (full.min.x + SIDE, full.max.x - SIDE);
    let top = full.min.y + k(16.0, 10.0);
    let strip = Rect::from_min_max(pos2(left, top), pos2(right, top + STRIP_H));
    let foot = full.max.y - k(24.0, 10.0);
    let bar = Rect::from_min_max(pos2(left, foot - k(72.0, 56.0)), pos2(right, foot));
    let rule_y = bar.min.y - k(11.0, 6.0) - 1.0;
    let rule = Rect::from_min_max(pos2(left, rule_y), pos2(right, rule_y + 1.0));
    let tips_y = rule.min.y - k(10.0, 6.0) - LINE_H;
    let tips = Rect::from_min_max(pos2(left, tips_y), pos2(right, tips_y + LINE_H));
    let status_y = tips.min.y - k(6.0, 4.0) - LINE_H;
    let status = Rect::from_min_max(pos2(left, status_y), pos2(right, status_y + LINE_H));
    let w = COLUMN_W.min(full.width() - 2.0 * SIDE).max(0.0);
    let column_top = strip.max.y + k(20.0, 12.0);
    let column = Rect::from_min_max(
        pos2(full.center().x - w / 2.0, column_top),
        pos2(full.center().x + w / 2.0, (status.min.y - 8.0).max(column_top)),
    );
    Parts { strip, column, status, tips, rule, bar }
}

/// The page frame of Folders, Extras and Login, the shell of the canvas's
/// boards: the top strip, the column with the screen's body (under the
/// wordmark on Folders), the status line, the tips line, the rule and the
/// bar. What it hands back is theirs together.
fn frame(ui: &mut Ui, wizard: &mut Wizard, page: &Page) -> Drawn {
    let at = parts(ui.max_rect());
    let mut drawn = Drawn::default();
    strip(ui, at.strip, wizard, page, &mut drawn.acts);

    let mut body_rect = at.column;
    if wizard.screen == Screen::Folders {
        body_rect.min.y = logo(ui, at.column).min(body_rect.max.y);
    }
    let mut body_ui =
        ui.new_child(UiBuilder::new().id_salt("setup-body").max_rect(body_rect).layout(Layout::top_down(Align::Min)));
    body_ui.set_clip_rect(body_rect.expand(1.0).intersect(ui.clip_rect()));
    let part = match wizard.screen {
        Screen::Folders => folders::show(&mut body_ui, wizard, page),
        Screen::Extras => extras::show(&mut body_ui, wizard, page),
        Screen::Login => login::show(&mut body_ui, wizard, page),
        Screen::Done => Drawn::default(),
    };
    drawn.acts.extend(part.acts);
    drawn.caret = part.caret;

    status_line(ui, at.status, wizard, &page.pal);
    tips_line(ui, at.tips, wizard, &page.pal);
    // The one rule, a hairline: in the kit's look gold is a warning.
    ui.painter().line_segment([at.rule.left_center(), at.rule.right_center()], Stroke::new(1.0, page.pal.edge));
    bar(ui, at.bar, wizard, page, &mut drawn.acts);
    drawn
}

/// The top strip across `rect`: the language chip at its left where the
/// cells draw it (Folders alone, where the key that opens the list, l, is
/// too), and the step at its right on the wizard's pages (the Quick Connect
/// page is no step of anything).
fn strip(ui: &Ui, rect: Rect, wizard: &Wizard, page: &Page, acts: &mut Vec<Act>) {
    if wizard.screen == Screen::Folders && language_chip(ui, rect.left_center(), wizard, page).clicked() {
        acts.push(Act::OpenLanguage);
    }
    if !wizard.standalone {
        step_line(ui, rect.right_center(), wizard.screen.step(), &page.pal);
    }
}

/// The language chip, its left middle at `at`, as the canvas draws it: the
/// chevron and the language's own name in a box the strip's height on the
/// panel's fill inside an edge hairline, raised under the pointer with the
/// name in the text colour and the hand, the cells' tooltip on it. As wide
/// as its name: 日本語 is narrower than Deutsch.
fn language_chip(ui: &Ui, at: Pos2, wizard: &Wizard, page: &Page) -> Response {
    let pal = &page.pal;
    let name = LANGS[wizard.lang].1;
    let w = 10.0 + 12.0 + 8.0 + studio::width_of(ui, name, body(13.0)) + 10.0;
    let rect = Rect::from_min_size(pos2(at.x, at.y - STRIP_H / 2.0), vec2(w, STRIP_H));
    let response = studio::hand(ui.interact(rect, Id::new("setup-language"), Sense::click()));
    let hovered = response.hovered();
    ui.painter().rect_filled(rect, studio::square(), if hovered { pal.raised } else { pal.panel });
    ui.painter().rect_stroke(rect, studio::square(), Stroke::new(1.0, pal.edge), StrokeKind::Inside);
    let ink = if hovered { pal.text } else { pal.soft };
    studio::chevron(ui, pos2(rect.min.x + 16.0, at.y), ink, false);
    studio::text(ui, pos2(rect.min.x + 30.0, at.y), name, body(13.0), ink, w, Align2::LEFT_CENTER);
    tip(page, &response, &t!("lang.chip_tip"));
    response
}

/// The step a page is, ending at `right_mid`: the kit's four step LEDs and,
/// 10 points on, the cells' counter "N / 4" in the dim monospace. Not a
/// control. Returns where it was drawn.
fn step_line(ui: &Ui, right_mid: Pos2, step: u8, pal: &Pal) -> Rect {
    let counter = format!("{step} / 4");
    let words = studio::text(ui, right_mid, &counter, mono(12.5), pal.dim, 80.0, Align2::RIGHT_CENTER);
    let mid = studio::led_mid(ui, right_mid.y, &mono(12.5), true);
    studio::step_leds(ui, words.min.x - 10.0, mid, usize::from(step), 4, pal).union(words)
}

/// The wordmark at the head of `column`, as the cells draw it on Folders:
/// the picture (src/setup/mstream-logo-dark.png) as a texture made once and
/// laid over the ground with its own alpha, whole and centred, as tall as
/// the boards' 93 points where the column has the room and shorter where it
/// has not, none at all under [`LOGO_LEAST`]. Returns where the column
/// goes on under it and its gap.
fn logo(ui: &Ui, column: Rect) -> f32 {
    let h = ((column.height() - LOGO_ROOM) * 0.75).min(LOGO_H);
    if h < LOGO_LEAST {
        return column.min.y;
    }
    let band = Rect::from_min_size(column.min, vec2(column.width(), h));
    if let Some(texture) = logo_texture(ui.ctx(), band) {
        studio::picture(ui, band, &texture);
    }
    band.max.y + LOGO_GAP * h / LOGO_H
}

/// The wordmark's texture for `band`: the picture halved as many times as
/// leaves it at most twice the pixels it is drawn at, each half made once
/// and kept. The window's textures have no mipmaps, and one sampled at
/// under half its size skips texels (covers.rs fits a cover down for the
/// same reason): the 1224 pixels drawn 372 points wide on a screen at 100%
/// would lose the wordmark's thin strokes, which their half keeps.
fn logo_texture(ctx: &egui::Context, band: Rect) -> Option<egui::TextureHandle> {
    let halvings = halvings(logo_size(ctx)?, band, ctx.pixels_per_point());
    studio::texture(ctx, Id::new("setup-logo").with(halvings), 0, false, || logo_image(halvings))
}

/// How many times a picture `size` pixels is halved to be drawn whole in
/// `band` at `ppp` pixels a point: as often as leaves it at most twice the
/// pixels it is drawn at, and never once where it is drawn at its own size
/// or larger.
fn halvings(size: Vec2, band: Rect, ppp: f32) -> u32 {
    let drawn = size.x * (band.width() / size.x).min(band.height() / size.y) * ppp;
    (size.x / drawn.max(1.0)).log2().floor().clamp(0.0, 8.0) as u32
}

/// The wordmark's size in pixels, read once off its header and kept.
fn logo_size(ctx: &egui::Context) -> Option<Vec2> {
    let id = Id::new("setup-logo-size");
    if let Some(kept) = ctx.data(|d| d.get_temp::<Option<Vec2>>(id)) {
        return kept;
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(LOGO_PNG)).with_guessed_format().ok();
    let size = reader.and_then(|r| r.into_dimensions().ok()).map(|(w, h)| vec2(w as f32, h as f32));
    ctx.data_mut(|d| d.insert_temp(id, size));
    size
}

/// The wordmark's pixels with their alpha, as egui takes them, halved
/// `halvings` times: each half a texel the mean of the four under it,
/// premultiplied, so the clear pixels round its strokes lend the edges
/// none of their colour (a mipmap's level, made by hand).
fn logo_image(halvings: u32) -> Option<egui::ColorImage> {
    let image = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let mut image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    for _ in 0..halvings {
        image = halved(&image);
    }
    Some(image)
}

/// `image` at half its size, each texel the mean of the (premultiplied)
/// two by two under it: an odd last row or column is left out, as a
/// mipmap's level leaves it, and a side one texel long is kept.
fn halved(image: &egui::ColorImage) -> egui::ColorImage {
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
            pixels.push(egui::Color32::from_rgba_premultiplied(r, g, b, a));
        }
    }
    egui::ColorImage::new([hw, hh], pixels)
}

/// The status line across `rect`, the cells' one row of it: what the
/// wizard is busy with in the accent, else its note, gold for an error and
/// dim otherwise (the kit's note line, cut with an ellipsis). A busy line
/// with no words (a quiet op's) hides no note, as the cells' draws nothing
/// over it.
fn status_line(ui: &Ui, rect: Rect, wizard: &Wizard, pal: &Pal) {
    let busy = wizard.busy.as_deref().filter(|busy| !busy.is_empty());
    studio::note_line(ui, rect, busy, wizard.note.as_ref(), pal);
}

/// The tips line across `rect`: the cells' footer hint (the modal on top
/// decides it, else the screen and its cursor), each part's key in the
/// monospace a step brighter than its dim words, the parts " · " apart as
/// the cells join them, cut with an ellipsis where it runs long.
fn tips_line(ui: &Ui, rect: Rect, wizard: &Wizard, pal: &Pal) {
    let line = footer_hint(wizard);
    let mut parts = Vec::new();
    for (k, part) in line.split(" · ").enumerate() {
        if k > 0 {
            parts.push((" · ".to_string(), body(12.5), pal.dim));
        }
        let (key, words) = key_of(part);
        parts.push((key.to_string(), mono(12.5), pal.soft));
        parts.push((words.to_string(), body(12.5), pal.dim));
    }
    studio::runs(ui, rect.left_center(), &parts, rect.width(), Align2::LEFT_CENTER);
}

/// A tips part's key and its words: the part's first word, and the arrows
/// after it ("↑ ↓" is one key), then the rest from its space. Every hint
/// in every language leads its parts with the key.
fn key_of(part: &str) -> (&str, &str) {
    let arrow = |word: &str| matches!(word, "↑" | "↓" | "←" | "→");
    let mut end = part.find(' ').unwrap_or(part.len());
    while let Some(next) = part[end..].strip_prefix(' ') {
        let word = next.split(' ').next().unwrap_or("");
        if !arrow(word) {
            break;
        }
        end += 1 + word.len();
    }
    part.split_at(end)
}

/// The bar across `rect`: the screen's way forward as the kit's lit form
/// button at its right (at rest, with the cells' tooltip saying why, where
/// the cells disable it), the way back 12 points left of it with the cells'
/// tooltip, and the scan's widget at its left whenever the wizard holds one,
/// as the cells draw it on all three screens (on Folders, after a way back,
/// as it stood when it was left).
fn bar(ui: &Ui, rect: Rect, wizard: &Wizard, page: &Page, acts: &mut Vec<Act>) {
    let pal = &page.pal;
    let (forward, act, dead, back) = match wizard.screen {
        Screen::Folders if wizard.folders.is_empty() => {
            (t!("folders.continue_disabled"), Act::ContinueFolders, Some(t!("folders.tip_continue_disabled")), None)
        }
        Screen::Folders => (t!("folders.continue_enabled"), Act::ContinueFolders, None, None),
        Screen::Extras => {
            (t!("extras.continue"), Act::ContinueExtras, None, Some((t!("extras.back"), Act::BackToFolders, t!("extras.tip_back"))))
        }
        Screen::Login => {
            (t!("login.create"), Act::CreateAdmin, None, Some((t!("login.back"), Act::BackToExtras, t!("login.tip_back"))))
        }
        Screen::Done => return,
    };
    let mid = rect.center().y;
    let w = studio::form_button_width(ui, &forward);
    let at = Rect::from_min_size(pos2(rect.max.x - w, mid - FORM_BUTTON_H / 2.0), vec2(w, FORM_BUTTON_H));
    match dead {
        Some(why) => {
            let response = studio::form_button_off(ui, Id::new("setup-forward"), at, &forward, pal);
            tip(page, &response, &why);
        }
        None => {
            if studio::form_button(ui, Id::new("setup-forward"), at, &forward, true, pal).clicked() {
                acts.push(act);
            }
        }
    }
    let mut left = at.min.x;
    if let Some((label, act, why)) = back {
        let w = studio::form_button_width(ui, &label);
        let at = Rect::from_min_size(pos2(left - 12.0 - w, mid - FORM_BUTTON_H / 2.0), vec2(w, FORM_BUTTON_H));
        let response = studio::form_button(ui, Id::new("setup-back"), at, &label, false, pal);
        tip(page, &response, &why);
        if response.clicked() {
            acts.push(act);
        }
        left = at.min.x;
    }
    if let Some(scan) = &wizard.scan {
        scan_widget(ui, Rect::from_min_max(rect.min, pos2((left - 24.0).max(rect.min.x), rect.max.y)), scan, pal);
    }
}

/// The scan's widget in `rect`, the cells' three lines as the canvas sets
/// them, 18 points each and centred down the bar: the step; the kit's
/// meter of ten LED segments, lit by the whole tenths done, in the accent
/// while the scan runs and the ok green once it is complete (all unlit, and
/// no share, with no estimate: never a fake 0%), the share after it in the
/// monospace; and the counts, dim, their figures in the monospace.
fn scan_widget(ui: &Ui, rect: Rect, scan: &ScanWidget, pal: &Pal) {
    let w = rect.width();
    if w < 24.0 {
        return;
    }
    let line = |k: f32| rect.center().y + (k - 1.0) * LINE_H;
    studio::text(ui, pos2(rect.min.x, line(0.0)), &scan.step, body(13.0), pal.text, w, Align2::LEFT_CENTER);
    // Complete is the idle answer's full meter with no counts under it.
    let complete = scan.pct == Some(100) && scan.detail.is_empty();
    let tone = if complete { pal.ok } else { pal.accent };
    let lit = scan.pct.map_or(0, |pct| studio::meter_lit(u64::from(pct), 100, 10));
    let meter = studio::meter(ui, pos2(rect.min.x, line(1.0)), 10, lit, vec2(5.0, 10.0), 2.0, tone, pal);
    if let Some(pct) = scan.pct {
        let x = meter.max.x + 8.0;
        studio::text(ui, pos2(x, line(1.0)), &format!("{pct}%"), mono(12.0), pal.dim, (rect.max.x - x).max(1.0), Align2::LEFT_CENTER);
    }
    let figures: Vec<&str> = scan.detail.split(|c: char| !c.is_ascii_digit()).filter(|f| !f.is_empty()).collect();
    let counts = studio::with_figures(&scan.detail, &figures, body(12.5), mono(12.0), pal.dim);
    studio::runs(ui, pos2(rect.min.x, line(2.0)), &counts, w, Align2::LEFT_CENTER);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use egui::epaint::Shape;
    use egui::{Color32, FullOutput};

    use super::*;
    use crate::api::Client;
    use crate::setup::Op;
    use crate::setup::tests::LOCALE_LOCK;

    /// The boards' page, in points.
    pub(super) const BOARD: Vec2 = vec2(1280.0, 800.0);

    /// The view run headless, as the window's panel runs it: a context with
    /// the panel's faces, the page's size in points, a clock the passes move
    /// on (a tooltip's dwell is egui's own, read off it), and what the host
    /// hands the view beside the wizard.
    pub(super) struct Probe {
        pub(super) ctx: egui::Context,
        pub(super) size: Vec2,
        clock: Cell<f64>,
        pub(super) view: View,
        pub(super) composition: String,
    }

    impl Probe {
        pub(super) fn new(size: Vec2) -> Probe {
            let ctx = egui::Context::default();
            crate::gui::window::panel::fonts(&ctx);
            Probe { ctx, size, clock: Cell::new(0.0), view: View::default(), composition: String::new() }
        }

        /// One pass with `events`, `dt` seconds after the last: what egui
        /// drew, and what the view told its host over the pass's runs.
        pub(super) fn pass_after(&self, wizard: &mut Wizard, events: Vec<egui::Event>, dt: f64) -> (FullOutput, Shown) {
            self.clock.set(self.clock.get() + dt);
            let screen = Rect::from_min_size(Pos2::ZERO, self.size);
            let input = egui::RawInput { screen_rect: Some(screen), events, time: Some(self.clock.get()), ..Default::default() };
            let mut seen: Option<Shown> = None;
            let mut out = self.ctx.run_ui(input, |ui| {
                let next = show(ui, wizard, &self.view, &self.composition, true);
                seen = Some(match seen.take() {
                    Some(last) => last.and(next),
                    None => next,
                });
            });
            // No GPU here to take the font atlas or the wordmark: let go of
            // them on purpose, or epaint's debug check fails the drop.
            out.textures_delta.clear();
            (out, seen.expect("the pass ran"))
        }

        pub(super) fn pass(&self, wizard: &mut Wizard, events: Vec<egui::Event>) -> (FullOutput, Shown) {
            self.pass_after(wizard, events, 1.0 / 60.0)
        }

        /// Two passes (egui sizes some things on the first), the second's.
        pub(super) fn settle(&self, wizard: &mut Wizard) -> (FullOutput, Shown) {
            let _ = self.pass(wizard, Vec::new());
            self.pass(wizard, Vec::new())
        }

        /// A press and a release at `at`, a pass each after the move, as
        /// hands make them: an outcome over the three, and the release's
        /// caret, the pass the click acted in.
        pub(super) fn press(&self, wizard: &mut Wizard, at: Pos2) -> Shown {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::default(),
            };
            let mut told: Option<Shown> = None;
            for events in [vec![egui::Event::PointerMoved(at)], vec![button(true)], vec![button(false)]] {
                let (_, next) = self.pass(wizard, events);
                told = Some(match told.take() {
                    Some(last) => last.and(next),
                    None => next,
                });
            }
            told.expect("three passes")
        }

        /// [`Probe::press`] on the middle of what `id` drew last pass.
        pub(super) fn click(&self, wizard: &mut Wizard, id: Id) -> Shown {
            self.press(wizard, self.rect(id).center())
        }

        /// Where `id` was drawn last pass.
        pub(super) fn rect(&self, id: Id) -> Rect {
            self.ctx.read_response(id).unwrap_or_else(|| panic!("{id:?} was drawn")).rect
        }

        /// The pointer brought to `at`, then left there `rest` seconds, and
        /// the pass egui asks for after that (a tooltip ripe on one pass
        /// is laid out unseen and shown on the next): that pass's drawing.
        pub(super) fn rest(&self, wizard: &mut Wizard, at: Pos2, rest: f64) -> FullOutput {
            let _ = self.pass(wizard, vec![egui::Event::PointerMoved(at)]);
            let mut left = rest;
            while left > 0.0 {
                let dt = left.min(0.2);
                let _ = self.pass_after(wizard, Vec::new(), dt);
                left -= dt;
            }
            self.pass(wizard, Vec::new()).0
        }
    }

    pub(super) fn wizard() -> Wizard {
        Wizard::new(Client::new("http://127.0.0.1:9").expect("client"))
    }

    /// Every text a pass drew, as laid out.
    pub(super) fn drawn(out: &FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    pub(super) fn has(drawn: &[String], words: &str) -> bool {
        drawn.iter().any(|d| d == words)
    }

    /// `words` as a tooltip draws them: the cells' lines.
    pub(super) fn tipped(words: &str) -> String {
        crate::kit::wrap_tip(words).join("\n")
    }

    /// The colour `words` were drawn in, where the pass drew them.
    pub(super) fn color_of(out: &FullOutput, words: &str) -> Option<Color32> {
        out.shapes.iter().find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words => text.galley.job.sections.first().map(|s| s.format.color),
            _ => None,
        })
    }

    /// The meter's lit stripes in `color`: three to a lit segment.
    fn stripes(out: &FullOutput, color: Color32) -> usize {
        out.shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                Shape::Rect(r) => r.fill == color && (r.rect.width() - 5.0).abs() < 0.01 && (r.rect.height() - 2.0).abs() < 0.01,
                _ => false,
            })
            .count()
    }

    #[test]
    fn the_frame_stands_where_the_boards_put_it_and_keeps_its_foot_on_a_small_page() {
        let at = parts(Rect::from_min_size(Pos2::ZERO, BOARD));
        assert_eq!(at.strip, Rect::from_min_max(pos2(24.0, 16.0), pos2(1256.0, 44.0)), "the top strip");
        assert_eq!((at.column.min, at.column.max.x), (pos2(280.0, 64.0), 1000.0), "the column, 720 wide from y 64");
        let ys = [at.status.min.y, at.tips.min.y, at.rule.min.y, at.bar.min.y, at.bar.max.y];
        assert_eq!(ys, [640.0, 664.0, 692.0, 704.0, 776.0], "the status, the tips, the rule and the bar");
        assert!(at.column.max.y <= at.status.min.y, "the column ends over the foot");

        // At the cells' floor the column narrows and stays centred, and the
        // foot keeps to the bottom.
        let small = parts(Rect::from_min_size(Pos2::ZERO, FLOOR));
        assert_eq!(small.column.width(), FLOOR.x - 2.0 * SIDE);
        assert_eq!(small.column.center().x, FLOOR.x / 2.0);
        assert_eq!(small.bar.max.y, FLOOR.y - 10.0, "pinned to the bottom");
        assert!(small.strip.max.y < small.column.min.y && small.column.max.y <= small.status.min.y, "nothing overlaps");
        assert!(small.status.max.y <= small.tips.min.y && small.tips.max.y <= small.rule.min.y && small.rule.max.y <= small.bar.min.y);
    }

    #[test]
    fn the_folders_page_draws_its_frame_in_the_cells_words() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        wizard.queue(Op::Ping, t!("busy.reaching"));
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        let hint = footer_hint(&wizard);
        for w in ["English", "1 / 4", "Add a music folder", "NAME", "FOLDER", "(nothing added yet)", "reaching the server…", "CONTINUE", hint.as_str()] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        let pal = Pal::now();
        assert_eq!(color_of(&out, "reaching the server…"), Some(pal.accent), "busy in the accent");
        assert_eq!(color_of(&out, "CONTINUE"), Some(pal.dim), "a dead Continue, as the cells' with nothing to continue with");
        // The tips line's keys in the monospace, its words in the face.
        let tips = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == hint => Some(t.galley.job.clone()),
            _ => None,
        });
        let tips = tips.expect("the tips line");
        let key = tips.sections.iter().find(|s| &tips.text[s.byte_range.start.0..s.byte_range.end.0] == "b").expect("the key b");
        assert_eq!(key.format.font_id, mono(12.5));
        // The wordmark, a texture of its own, whole at the boards' 372 by
        // 93, centred at the column's head, from the picture's half (612
        // by 153): at one pixel a point the whole 1224 would be sampled at
        // under half its size.
        let logo = out.shapes.iter().find_map(|c| match &c.shape {
            Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => Some((mesh.calc_bounds(), mesh.texture_id)),
            _ => None,
        });
        let (logo, texture) = logo.expect("the wordmark drawn");
        assert!((logo.width() - 372.0).abs() < 1.0 && (logo.height() - 93.0).abs() < 1.0, "{logo:?}");
        assert!((logo.center().x - 640.0).abs() < 0.5 && (logo.min.y - 64.0).abs() < 0.5, "{logo:?}");
        assert_eq!(probe.ctx.tex_manager().read().meta(texture).map(|m| m.size), Some([612, 153]));
        assert!(shown.caret.is_none() && shown.outcome.is_none(), "no field, nothing ended");
    }

    #[test]
    fn the_status_line_is_busy_else_the_note_gold_for_an_error_and_dim_else() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        let unreachable = "could not reach the server: could not reach server: error sending request";
        wizard.note = Some((unreachable.into(), true));
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, unreachable), Some(pal.gold), "an error in gold");
        wizard.note = Some(("that folder is already in the list".into(), false));
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(color_of(&out, "that folder is already in the list"), Some(pal.dim), "anything else dim");
        // A quiet op's busy line hides no note, as the cells' draws nothing.
        wizard.busy = Some(String::new());
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "that folder is already in the list"));
        wizard.busy = Some("opening the folder picker…".into());
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, "opening the folder picker…") && !has(&words, "that folder is already in the list"), "busy wins: {words:?}");
    }

    #[test]
    fn each_screens_bar_is_the_cells_and_a_click_on_it_does_what_the_cells_do() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let forward = Id::new("setup-forward");
        // Nothing to continue with: a click on the dead Continue does
        // nothing, as the cells register none for it.
        let _ = probe.settle(&mut wizard);
        assert!(probe.click(&mut wizard, forward).outcome.is_none());
        assert_eq!((wizard.queued.clone(), wizard.note.clone()), (None, None), "the dead Continue took no click");
        wizard.add_folder("/srv/music".into());
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, forward);
        assert_eq!(wizard.queued, Some(Op::CommitFolders), "Continue commits the folders");
        wizard.queued = None;

        // Extras: the step, Back and Continue, no chip, the body's title.
        wizard.screen = Screen::Extras;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        for w in ["2 / 4", "BACK", "CONTINUE", "Extras"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "English"), "the chip is Folders' alone, as the cells': {words:?}");
        probe.click(&mut wizard, forward);
        assert_eq!(wizard.queued, Some(Op::CommitExtras));
        wizard.queued = None;
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, Id::new("setup-back"));
        assert_eq!(wizard.screen, Screen::Folders, "Back to the folders");

        // Login: the step, Back and Create Admin.
        wizard.screen = Screen::Login;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        for w in ["3 / 4", "BACK", "CREATE ADMIN", "Create Admin User"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        probe.click(&mut wizard, forward);
        assert_eq!(wizard.note, Some((t!("login_problem.username").to_string(), true)), "the form checked first, as on Enter");
        probe.click(&mut wizard, Id::new("setup-back"));
        assert_eq!(wizard.screen, Screen::Extras, "Back to the extras");
    }

    #[test]
    fn a_tip_waits_out_the_cells_dwell_and_a_key_hides_it_for_another() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let mut probe = Probe::new(BOARD);
        let mut wizard = wizard();
        let _ = probe.settle(&mut wizard);
        let chip = probe.rect(Id::new("setup-language")).center();
        let tip = t!("lang.chip_tip").to_string();
        assert!(!has(&drawn(&probe.rest(&mut wizard, chip, 0.3)), &tip), "not before the dwell");
        assert!(has(&drawn(&probe.rest(&mut wizard, chip, 0.6)), &tip), "after it");
        // A key just pressed hides it, and the dwell after it brings it
        // back. (Just pressed for as long as the test runs: an instant
        // ahead reads as no time since.)
        probe.view.keyed = Some(Instant::now() + Duration::from_secs(600));
        assert!(!has(&drawn(&probe.pass(&mut wizard, Vec::new()).0), &tip), "a key hides it");
        assert!(!has(&drawn(&probe.rest(&mut wizard, chip, 0.6)), &tip), "for the dwell after it, the pointer resting or not");
        probe.view.keyed = Instant::now().checked_sub(TIP_DELAY + Duration::from_millis(10));
        assert!(has(&drawn(&probe.rest(&mut wizard, chip, 0.6)), &tip), "and its dwell after brings it back");

        // The disabled Continue keeps the cells' tooltip, the one a
        // disabled control carries; Extras' Back has its own.
        let dead = probe.rect(Id::new("setup-forward")).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, dead, 0.6)), &t!("folders.tip_continue_disabled")));
        wizard.screen = Screen::Extras;
        let _ = probe.settle(&mut wizard);
        let back = probe.rect(Id::new("setup-back")).center();
        assert!(has(&drawn(&probe.rest(&mut wizard, back, 0.6)), &t!("extras.tip_back")));

        // A click on the chip opens the language list on the language shown.
        wizard.screen = Screen::Folders;
        let _ = probe.settle(&mut wizard);
        probe.click(&mut wizard, Id::new("setup-language"));
        assert_eq!(wizard.modal, Modal::Language(0));
    }

    #[test]
    fn the_scan_shows_whenever_the_wizard_holds_one_amber_while_it_runs_and_green_once_done() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let pal = Pal::now();
        let mut wizard = wizard();
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!(stripes(&out, pal.accent) + stripes(&out, pal.ok), 0, "no scan news, no widget");
        // On Folders after a way back, as it stood when it was left.
        wizard.scan = Some(ScanWidget { step: "Scanning media".into(), pct: Some(44), detail: "1204 tracks so far".into() });
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        for w in ["Scanning media", "44%", "1204 tracks so far"] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert_eq!(stripes(&out, pal.accent), 4 * 3, "four whole tenths lit in the accent");
        wizard.scan = Some(ScanWidget { step: t!("scan.complete").into(), pct: Some(100), detail: String::new() });
        let (out, _) = probe.settle(&mut wizard);
        assert_eq!((stripes(&out, pal.ok), stripes(&out, pal.accent)), (10 * 3, 0), "complete: all ten in green");
        wizard.scan = Some(ScanWidget { step: "Downloading album art".into(), pct: None, detail: "57 so far".into() });
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, "57 so far") && !words.iter().any(|w| w.ends_with('%')), "no estimate, no share: {words:?}");
        assert_eq!(stripes(&out, pal.accent), 0, "and none lit");
    }

    #[test]
    fn under_the_cells_floor_the_page_says_what_the_cells_say_and_answers_nothing() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(vec2(FLOOR.x - 1.0, 400.0));
        let mut wizard = wizard();
        let (out, shown) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, &t!("resize")), "{words:?}");
        assert!(!has(&words, "Add a music folder") && probe.ctx.read_response(Id::new("setup-add")).is_none(), "nothing else");
        assert!(!shown.hand && shown.caret.is_none());
        // At the floor itself, the page.
        let probe = Probe::new(FLOOR);
        let (out, _) = probe.settle(&mut wizard);
        assert!(has(&drawn(&out), "Add a music folder"));
    }

    #[test]
    fn each_modal_draws_its_title_over_an_inert_page_and_its_close_does_the_cells_close() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        type Open = fn(&mut Wizard);
        // Each modal, the screen it opens over and a live control there.
        let modals: [(Screen, &str, Open, &str, Id); 4] = [
            (Screen::Folders, "setup-add", |w| w.modal = Modal::Language(0), "lang.modal_title", Id::new("setup-language-list")),
            (Screen::Folders, "setup-add", |w| w.modal = Modal::PathEntry(Default::default()), "path_modal.title", Id::new("setup-path-entry")),
            (Screen::Folders, "setup-add", |w| w.modal = Modal::Browser(Default::default()), "browse.title", Id::new("setup-browser")),
            (Screen::Login, "setup-forward", |w| w.modal = Modal::SkipWarning, "skip_modal.title", Id::new("setup-skip")),
        ];
        for (screen, beneath, open, title, id) in modals {
            let mut wizard = wizard();
            wizard.screen = screen;
            let _ = probe.settle(&mut wizard);
            let beneath = probe.rect(Id::new(beneath)).center();
            open(&mut wizard);
            let (out, shown) = probe.settle(&mut wizard);
            let words = drawn(&out);
            assert!(has(&words, &t!(title)), "{title} drawn: {words:?}");
            assert!(has(&words, &footer_hint(&wizard)), "the modal's hint on the tips line: {words:?}");
            assert_eq!(shown.caret.is_some(), matches!(wizard.modal, Modal::PathEntry(_)), "{title}: a caret for the path's field alone");
            // The page beneath is inert: a click on its control is lost.
            probe.press(&mut wizard, beneath);
            assert!(wizard.queued.is_none() && wizard.note.is_none() && wizard.modal != Modal::None, "{title}: the scrim took it");
            let _ = probe.settle(&mut wizard);
            probe.click(&mut wizard, id.with("close"));
            assert_eq!(wizard.modal, Modal::None, "{title}: closed");
        }
    }

    #[test]
    fn done_draws_its_own_page_and_the_quick_connect_page_is_no_step() {
        let _guard = LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        let probe = Probe::new(BOARD);
        let mut wizard = wizard();
        wizard.screen = Screen::Done;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        // No heading: the wordmark heads the page, as in the cells' two
        // columns; the message opens it.
        let (opening, hint) = (t!("done.running_port", port = 9).to_string(), footer_hint(&wizard));
        for w in ["4 / 4", opening.as_str(), hint.as_str()] {
            assert!(has(&words, w), "{w} drawn: {words:?}");
        }
        assert!(!has(&words, "CONTINUE") && !has(&words, "English"), "no bar, no chip: {words:?}");
        assert!(!has(&words, &t!("done.heading_wizard")), "{words:?}");
        wizard.standalone = true;
        let (out, _) = probe.settle(&mut wizard);
        let words = drawn(&out);
        assert!(has(&words, &opening) && !has(&words, "4 / 4") && !has(&words, &t!("done.heading_qr")), "{words:?}");
    }

    #[test]
    fn every_hints_parts_lead_with_their_key_in_every_language() {
        assert_eq!(key_of("↑ ↓ select"), ("↑ ↓", " select"));
        assert_eq!(key_of("↑ ↓ 行"), ("↑ ↓", " 行"));
        assert_eq!(key_of("Ctrl+S skip"), ("Ctrl+S", " skip"));
        assert_eq!(key_of("a add this folder"), ("a", " add this folder"));
        assert_eq!(key_of("c"), ("c", ""));
        let keys = [
            "hint.browser", "hint.path", "hint.skip", "hint.folders_empty", "hint.folders_rows", "hint.folders_selected", "hint.login",
            "hint.extras", "hint.extras_selected", "hint.done_qr", "hint.done_wizard", "lang.hint",
        ];
        for (code, _) in LANGS {
            for key in keys {
                for part in t!(key, locale = code).split(" · ") {
                    let (k, words) = key_of(part);
                    assert!(!k.is_empty() && format!("{k}{words}") == part, "{code} {key}: {part:?}");
                    assert!(words.is_empty() || words.starts_with(' '), "{code} {key}: {part:?}");
                    assert!(!k.contains(' ') || k.split(' ').skip(1).all(|w| matches!(w, "↑" | "↓")), "{code} {key}: {k:?} is one key");
                }
            }
        }
    }

    /// The wordmark is never sampled at under half its pixels (the window's
    /// textures have no mipmaps): halved until it is at most twice what it
    /// is drawn at, whole where the screen's pixels are as many, each half
    /// the mean of the premultiplied texels under it.
    #[test]
    fn the_wordmark_is_halved_to_within_twice_the_pixels_it_is_drawn_at() {
        let size = vec2(1224.0, 306.0);
        let band = |h: f32| Rect::from_min_size(pos2(280.0, 64.0), vec2(720.0, h));
        assert_eq!(halvings(size, band(93.0), 1.0), 1, "372 pixels wide: the 612 half");
        assert_eq!(halvings(size, band(93.0), 1.5), 1, "558 wide: still the half");
        assert_eq!(halvings(size, band(93.0), 2.0), 0, "744 wide: the whole");
        assert_eq!(halvings(size, band(93.0), 3.0), 0, "never more than whole");
        assert_eq!(halvings(size, band(24.0), 1.0), 3, "the least it is drawn at: an eighth");
        assert_eq!(halvings(size, Rect::from_min_size(Pos2::ZERO, vec2(100.0, 93.0)), 1.0), 3, "a narrow band, by its width");
        for (h, ppp) in [(93.0, 1.0), (93.0, 1.25), (60.0, 1.0), (24.0, 2.0)] {
            let n = halvings(size, band(h), ppp);
            let ratio = size.y / 2f32.powi(n as i32) / (h * ppp);
            assert!(ratio <= 2.0 + 1e-3, "{h} at {ppp}: {n} halvings sample at {ratio}");
        }
        let image = logo_image(1).expect("the half");
        assert_eq!(image.size, [612, 153]);
        // Each texel the mean of the four under it, an odd last column
        // left out.
        let red = Color32::from_rgba_premultiplied(200, 0, 0, 200);
        let two = egui::ColorImage::new([2, 2], vec![red, Color32::TRANSPARENT, Color32::TRANSPARENT, Color32::TRANSPARENT]);
        assert_eq!(halved(&two).pixels, [Color32::from_rgba_premultiplied(50, 0, 0, 50)]);
        let three = egui::ColorImage::new([3, 1], vec![red, red, Color32::TRANSPARENT]);
        assert_eq!((halved(&three).size, halved(&three).pixels), ([1, 1], vec![red]));
    }

    #[test]
    fn an_outcome_from_either_run_of_a_pass_stands() {
        let at = |outcome| Shown { points: Rect::NOTHING, hand: false, caret: None, outcome };
        let caret = Shown { caret: Some(pos2(3.0, 4.0)), ..at(None) };
        let both = at(Some(Outcome::Quit)).and(caret);
        assert!(matches!(both.outcome, Some(Outcome::Quit)), "the first run's click stands");
        assert_eq!(both.caret, Some(pos2(3.0, 4.0)), "the caret is the last run's");
        assert!(matches!(at(None).and(at(Some(Outcome::Quit))).outcome, Some(Outcome::Quit)));
    }
}
