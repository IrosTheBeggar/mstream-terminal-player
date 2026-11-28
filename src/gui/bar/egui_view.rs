//! The transport bar's egui face in the GUI's own window, hosted by the
//! chrome (src/gui/chrome.rs) in the bar's five rows: paint only, in the
//! very boxes the cells drew and registered (`BarLayout`), whose registry
//! keeps every press, right press, wheel and hand the bar has; lit where the
//! registry says the pointer is (`BarCtx::hot`), so a control lights only
//! where a press would work it, and nothing lights under a modal's guard, a
//! popup's catcher or a pointer a modal withheld.
//!
//! The look is the canvas's: Main.dc.html's BAR, SEEK ROW, AUTO-DJ SPLIT,
//! TRANSPORT GROUP, VOLUME, NOTE SLOT and CARD patterns, with their states
//! on the TransportStates, TransportParts and NoteBanner boards (decisions
//! D11-D17, D22-D24), at the cells' footprint: each piece stands in its own
//! cells, so what egui paints is what the cells would press. The questions
//! the owner has still to rule on are drawn as the code behaves: the pause
//! bars on the play slot with nothing loaded (R14), repeat One in All's look
//! (R15), the idle rule a column wider each side than the seek (R16), no look
//! on the volume's segments under the pointer and their fill from the volume
//! as it stands (R17), the card's chevron on Admin (R19), a byline that never
//! names the album (R20). The quit's countdown on the last row stays the
//! cells' to write: the host cuts the bar round it.

use egui::epaint::{CornerRadius, Mesh, Shape, StrokeKind};
use egui::{Align2, Color32, FontId, Id, Pos2, Stroke, Ui, Vec2, pos2, vec2};
use ratatui::layout::Rect;

use super::{BarLayout, BarView, CardLayout, DJ_WORD, NoteTone, Now, SplitLayout, VolumeLayout, byline, facts, fmt_time, volume_cells, volume_words};
use crate::gui::Act;
use crate::gui::dj::QuickAct;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Mark, Pal};
use crate::tui::art::Art;

/// Whether egui paints the bar (the host draws the cells bar until it does).
pub(crate) const READY: bool = true;

/// What the bar's face is handed besides the view and the layout.
pub(crate) struct BarCtx<'a> {
    /// Where the grid's cells stand in points.
    pub cells: Cells,
    /// The control a press would work, by its cells and its act, which
    /// lights; none at rest, and none where something stands over it.
    pub hot: Option<(Rect, Act)>,
    pub pal: &'a Pal,
}

/// The times' size, in the monospace.
const TIME_SIZE: f32 = 12.0;
/// The auto-dj word's spaced capitals.
const WORD_SIZE: f32 = 12.5;
/// The split's LED and the room between it and the word.
const LAMP: f32 = 6.0;
const LAMP_GAP: f32 = 8.0;
/// A volume segment, at the head of its cell.
const SEGMENT: Vec2 = vec2(6.0, 10.0);
/// The card's lines: the title in the semibold, the byline in the face,
/// the spec and the facts in the monospace.
const TITLE_SIZE: f32 = 13.0;
const BYLINE_SIZE: f32 = 12.5;
const FIGURE_SIZE: f32 = 11.5;
/// A rating star's outer radius, its inner one's share of it, from one
/// star's middle to the next, and the room after the fifth.
const STAR_R: f32 = 5.0;
const STAR_INNER: f32 = 0.45;
const STAR_PITCH: f32 = 11.0;
const STARS_GAP: f32 = 8.0;
/// The cover's corners, the desk's.
const COVER_RADIUS: u8 = 3;

/// Paint the bar in the boxes the cells registered, on `ui` (its child over
/// the bar's rows, clipped to them, the ground painted): the panel's fill
/// over all five rows, the seek row, the split, the group, the volume, the
/// note and the card. It takes no pointer: its presses are the cells
/// registry's.
pub(crate) fn show(ui: &mut Ui, view: &BarView, layout: &BarLayout, cx: &BarCtx) {
    let hot = cx.hot.as_ref().map(|(_, act)| act);
    // No hairline over it: the seek row is the bar's edge.
    ui.painter().rect_filled(cx.cells.rect(layout.rect), 0.0, cx.pal.panel);
    seek_row(ui, view, layout, cx, hot);
    split(ui, &layout.dj, view, cx, hot);
    group(ui, layout, view, cx, hot);
    volume(ui, &layout.volume, view.volume, cx, hot);
    if let Some((words, tone)) = &view.note {
        studio::note_in(ui, cx.cells.rect(layout.note), words, tone_color(*tone, cx.pal));
    }
    card(ui, &layout.card, view, cx, hot);
}

/// A note's tone in the desk's palette: the shell's own note dim, an error
/// in the gold; the App's Info dim, Busy in the accent, a landing's Success
/// in the ok green, Plain in the text colour, as the cells tone them.
fn tone_color(tone: NoteTone, pal: &Pal) -> Color32 {
    match tone {
        NoteTone::Dim => pal.dim,
        NoteTone::Error => pal.gold,
        NoteTone::Busy => pal.accent,
        NoteTone::Success => pal.ok,
        NoteTone::Plain => pal.text,
    }
}

/// The seek row (Main's SEEK ROW, D11). While something is up: the times in
/// the monospace, dim, in slots as wide as the total, the elapsed one at its
/// slot's right ([`elapsed_time`]); between them the track, gold over the share played to its
/// thumb, an unknown length playing none of it, as the cells' playhead stands
/// on their first cell. Where the pointer would land a press, the ghost thumb
/// at the share that press seeks to, and in the elapsed slot the time it
/// would land at, in the text colour: the cells' preview, cell for cell.
/// Idle, the gold hairline across the whole row, the cells' rule, with no
/// times and nothing to press.
fn seek_row(ui: &Ui, view: &BarView, layout: &BarLayout, cx: &BarCtx, hot: Option<&Act>) {
    let pal = cx.pal;
    let (Some(seek), Some(now)) = (&layout.seek, view.now) else {
        let row = cx.cells.rect(Rect { height: 1, ..layout.rect });
        let mid = row.center().y;
        ui.painter().rect_filled(egui::Rect::from_min_max(pos2(row.min.x, mid - 1.0), pos2(row.max.x, mid)), 0.0, pal.gold);
        return;
    };
    let ghost = match hot {
        Some(Act::Seek(frac)) => Some(*frac),
        _ => None,
    };
    let (time, color) = match ghost {
        Some(frac) => (fmt_time(frac * now.duration), pal.text),
        None => (fmt_time(now.elapsed), pal.dim),
    };
    let track = cx.cells.rect(seek.track);
    elapsed_time(ui, cx.cells.rect(seek.elapsed), track.min.x, &time, color);
    let total = cx.cells.rect(seek.total);
    studio::text(ui, total.left_center(), &fmt_time(now.duration), studio::mono(TIME_SIZE), pal.dim, total.width(), Align2::LEFT_CENTER);
    studio::seek_track(ui, track, played_share(now), ghost.map(|frac| frac as f32), pal);
}

/// The elapsed time, or the time a press would land at, in its `slot`: at
/// the slot's right where it fits, as the cells pad it there. A time wider
/// than its slot (an unknown length's past ten minutes, whose slot is as
/// wide as its total's "0:00") the cells write on from the slot's left into
/// the gap before the track, whole: so does the face, cut only where the
/// track begins at `track_x`, never to "12:…" in the slot.
fn elapsed_time(ui: &Ui, slot: egui::Rect, track_x: f32, time: &str, color: Color32) {
    let font = studio::mono(TIME_SIZE);
    if studio::width_of(ui, time, font.clone()) <= slot.width() {
        studio::text(ui, slot.right_center(), time, font, color, slot.width(), Align2::RIGHT_CENTER);
    } else {
        studio::text(ui, slot.left_center(), time, font, color, (track_x - slot.min.x).max(1.0), Align2::LEFT_CENTER);
    }
}

/// How much of the track is played: the share of its length elapsed, held
/// to the track; none while the length is unknown.
fn played_share(now: &Now) -> f32 {
    if now.duration > 0.0 { (now.elapsed / now.duration).clamp(0.0, 1.0) as f32 } else { 0.0 }
}

/// A tall control's frame in its three rows' `band`: [`studio::TALL_H`]
/// tall, centred in the 48 points the rows give at the opening size, and as
/// wide as its cells.
fn framed(band: egui::Rect) -> egui::Rect {
    band.shrink2(vec2(0.0, ((band.height() - studio::TALL_H) / 2.0).max(0.0)))
}

/// The auto-dj split (Main's AUTO-DJ SPLIT, D13): one frame round both
/// halves and a hairline between them at the middle of the rule's column,
/// in the strong edge, or the ok green while the DJ is armed. The word's
/// half: its LED, unlit or ok, then the cells' word in spaced capitals, dim
/// or ok, whole in any face ([`word_size`]). The chevron's half: the chevron
/// up while quick config is down and down while it is up, dim, in the text
/// colour while it is up. Each half takes its own raised fill under the
/// pointer, an idle word going to the text colour (the cells light the whole
/// frame from the word's half: ruling R18).
fn split(ui: &Ui, at: &SplitLayout, view: &BarView, cx: &BarCtx, hot: Option<&Act>) {
    let pal = cx.pal;
    let frame = framed(cx.cells.rect(at.frame));
    let inner = frame.shrink(1.0);
    let rule = cx.cells.rect(Rect { x: at.rule_x, width: 1, ..at.frame }).center().x;
    let word_hot = hot == Some(&Act::AutoDj);
    let chevron_hot = hot == Some(&Act::DjQuick(QuickAct::Toggle));
    let edge = if view.autodj { pal.ok } else { pal.edge_strong };
    let painter = ui.painter();
    if word_hot {
        let half = egui::Rect::from_min_max(inner.min, pos2(rule - 1.0, inner.max.y));
        painter.rect_filled(half, CornerRadius { nw: 2, sw: 2, ne: 0, se: 0 }, pal.raised);
    }
    if chevron_hot {
        let half = egui::Rect::from_min_max(pos2(rule, inner.min.y), inner.max);
        painter.rect_filled(half, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, pal.raised);
    }
    painter.rect_stroke(frame, studio::square(), Stroke::new(1.0, edge), StrokeKind::Inside);
    painter.rect_filled(egui::Rect::from_min_max(pos2(rule - 1.0, inner.min.y), pos2(rule, inner.max.y)), 0.0, edge);
    let mid = frame.center().y;
    let word = DJ_WORD.trim();
    let ink = match (view.autodj, word_hot) {
        (true, _) => pal.ok,
        (false, true) => pal.text,
        (false, false) => pal.dim,
    };
    let size = word_size(ui, word, rule - inner.min.x);
    let words_w = studio::caps_width(ui, word, size);
    let left = ((frame.min.x + rule) / 2.0 - (LAMP + LAMP_GAP + words_w) / 2.0).max(inner.min.x + 1.0);
    let lamp = if view.autodj { pal.ok } else { pal.unlit };
    studio::led(ui, pos2(left + LAMP / 2.0, studio::led_mid(ui, mid, &studio::strong(size), true)), lamp);
    let x = left + LAMP + LAMP_GAP;
    studio::caps(ui, pos2(x, mid), word, size, ink, (rule - 2.0 - x).max(1.0), Align2::LEFT_CENTER);
    let chevron = if view.quick_open || chevron_hot { pal.text } else { pal.dim };
    studio::chevron(ui, pos2((rule + frame.max.x) / 2.0, mid), chevron, !view.quick_open);
}

/// The size the split's `word` is set at in a word's half `half` points
/// wide, from its frame's inside to the rule: the board's [`WORD_SIZE`]
/// where the face's capitals fit beside the LED there, else as much smaller
/// as lets them stand whole. The half is as wide as its cells whatever the
/// face, and a wide bold (DejaVu Sans Bold, Linux's) sets the board's 12.5
/// points wider than it holds, which egui would cut to "AUTO-D…". The rule's
/// own point and a point each side of the words are kept, and one spare,
/// since a size scaled down sets its letters on pixels a little apart from
/// the share it was scaled by.
fn word_size(ui: &Ui, word: &str, half: f32) -> f32 {
    let room = half - 1.0 - 2.0 - LAMP - LAMP_GAP - 1.0;
    let whole = studio::caps_width(ui, word, WORD_SIZE);
    if whole <= room || room <= 0.0 { WORD_SIZE } else { WORD_SIZE * room / whole }
}

/// The transport group (Main's TRANSPORT GROUP, D12), each control's frame
/// in its cells, its mark at the middle. Repeat and shuffle: the strong edge
/// and a dim mark off, the ok green on (repeat One wears All's look: ruling
/// R15). Prev and next: the strong edge and the text colour. Play/pause, the
/// one primary: a 2-point gold frame and a gold mark, the pause bars
/// whenever the bar is not paused, nothing loaded included (ruling R14), the
/// play triangle while it is, in a slot of one width. Under the pointer the
/// raised fill and a step brighter: the strong edge to the dim, a dim mark
/// to the text colour, the gold to the hot accent; the ok green has no
/// brighter step.
fn group(ui: &Ui, layout: &BarLayout, view: &BarView, cx: &BarCtx, hot: Option<&Act>) {
    let pal = cx.pal;
    for (rect, act) in &layout.group {
        let (mark, frame, ink, weight) = match act {
            Act::PlayPause => (if view.paused { Mark::Play } else { Mark::Pause }, pal.gold, pal.gold, 2.0),
            Act::Repeat | Act::Shuffle => {
                let (mark, on) = if *act == Act::Repeat { (Mark::Repeat, view.repeat) } else { (Mark::Shuffle, view.shuffle) };
                if on { (mark, pal.ok, pal.ok, 1.0) } else { (mark, pal.edge_strong, pal.dim, 1.0) }
            }
            Act::Prev => (Mark::Prev, pal.edge_strong, pal.text, 1.0),
            _ => (Mark::Next, pal.edge_strong, pal.text, 1.0),
        };
        let at = framed(cx.cells.rect(*rect));
        let lit = hot == Some(act);
        if lit {
            ui.painter().rect_filled(at, studio::square(), pal.raised);
        }
        let (frame, ink) = if lit { (brighter(frame, pal), brighter(ink, pal)) } else { (frame, ink) };
        studio::frame_button(ui, at, mark, frame, ink, weight, false, pal);
    }
}

/// `c` a step brighter, as the boards light a control under the pointer:
/// the strong edge to the dim, the dim to the text colour, the gold to the
/// hot accent; the text colour and the ok green are as bright as they go.
fn brighter(c: Color32, pal: &Pal) -> Color32 {
    match c {
        c if c == pal.edge_strong => pal.dim,
        c if c == pal.dim => pal.text,
        c if c == pal.gold => pal.accent_hot,
        c => c,
    }
}

/// The volume (Main's VOLUME, D14) on the bar's bottom row, in the cells'
/// columns: − and + as quiet marks, dim, in the text colour under the
/// pointer; ten LED segments, one at the head of each of their cells, as
/// many lit amber as the cells fill (`volume_cells`) and the rest unlit,
/// with no look of their own under the pointer, as the cells give them none
/// (ruling R17); the readout in the monospace, dim, at its slot's right.
fn volume(ui: &Ui, at: &VolumeLayout, volume: f32, cx: &BarCtx, hot: Option<&Act>) {
    let pal = cx.pal;
    let tone = |act: Act| if hot == Some(&act) { pal.text } else { pal.dim };
    quiet(ui, cx.cells.rect(at.minus).center(), false, tone(Act::VolDown));
    let lit = volume_cells(volume);
    for (i, cell) in at.cells.iter().enumerate() {
        let cell = cx.cells.rect(*cell);
        let segment = egui::Rect::from_min_size(pos2(cell.min.x, cell.center().y - SEGMENT.y / 2.0), SEGMENT);
        ui.painter().rect_filled(segment, CornerRadius::same(1), if i < lit { pal.accent } else { pal.unlit });
    }
    quiet(ui, cx.cells.rect(at.plus).center(), true, tone(Act::VolUp));
    let readout = cx.cells.rect(at.readout);
    let words = volume_words(volume);
    studio::text(ui, readout.right_center(), words.trim_start(), studio::mono(TIME_SIZE), pal.dim, readout.width(), Align2::RIGHT_CENTER);
}

/// A quiet − or + centred on `at`, as the canvas draws the volume's steps:
/// strokes 6 points long and 1.5 thick, in the cell the cells give each (the
/// kit's add icon is wider than it).
fn quiet(ui: &Ui, at: Pos2, plus: bool, color: Color32) {
    let stroke = Stroke::new(1.5, color);
    ui.painter().line_segment([at - vec2(3.0, 0.0), at + vec2(3.0, 0.0)], stroke);
    if plus {
        ui.painter().line_segment([at - vec2(0.0, 3.0), at + vec2(0.0, 3.0)], stroke);
    }
}

/// The now-playing card (Main's CARD, D15, D16), one control in its cells.
/// The cover ([`cover`]); then four lines from 16 points past it, each in
/// its row and cut with an ellipsis at its cells' end: the title in the
/// semibold, in the text colour; the byline, dim; the spec in the
/// monospace, dim; the rating's stars in gold, then the key and the tempo
/// in the monospace, dim ([`rating_line`]). With nothing up, the cells'
/// words for it, dim, alone. The queue's chevron in its cell on the first
/// line, down while the queue is open. Under the pointer the raised fill
/// behind the whole card and the byline and the chevron in the text colour,
/// as the cells light the title and the byline (the title wears it already).
fn card(ui: &Ui, at: &CardLayout, view: &BarView, cx: &BarCtx, hot: Option<&Act>) {
    let pal = cx.pal;
    let lit = hot == Some(&Act::ToggleQueue);
    if lit {
        ui.painter().rect_filled(cx.cells.rect(at.rect), studio::square(), pal.raised);
    }
    cover(ui, cx.cells.rect(at.cover), view.art, pal);
    let line = |i: usize| cx.cells.rect(at.lines[i]);
    let sub = if lit { pal.text } else { pal.dim };
    match view.now {
        Some(now) => {
            words(ui, line(0), &now.title, studio::strong(TITLE_SIZE), pal.text);
            words(ui, line(1), &byline(now), studio::body(BYLINE_SIZE), sub);
            words(ui, line(2), &now.spec, studio::mono(FIGURE_SIZE), pal.dim);
            rating_line(ui, line(3), now, pal);
        }
        None => words(ui, line(0), &rust_i18n::t!("gui.nothing_playing"), studio::body(BYLINE_SIZE), sub),
    }
    studio::chevron(ui, cx.cells.rect(at.chevron).center(), if lit { pal.text } else { pal.dim }, !view.queue_open);
}

/// One of the card's lines in its `row`, from the row's left, cut with an
/// ellipsis at its end.
fn words(ui: &Ui, row: egui::Rect, s: &str, font: FontId, color: Color32) {
    if !s.is_empty() {
        studio::text(ui, row.left_center(), s, font, color, row.width(), Align2::LEFT_CENTER);
    }
}

/// The card's fourth line in its `row`: while the track is rated, its five
/// stars in gold ([`star`], from the cells' `stars`), centred on the facts'
/// capitals; then, 8 points after them, the key and the tempo in the
/// monospace, dim, cut at the row's end. Neither, nothing.
fn rating_line(ui: &Ui, row: egui::Rect, now: &Now, pal: &Pal) {
    let mid = row.center().y;
    let figures = studio::mono(FIGURE_SIZE);
    let mut x = row.min.x;
    if let Some(rating) = now.rating.filter(|r| *r > 0) {
        let y = studio::led_mid(ui, mid, &figures, true);
        for (k, glyph) in crate::gui::actions::stars(Some(rating)).chars().enumerate() {
            let fill = match glyph {
                '★' | '*' => 1.0,
                '½' | 'h' => 0.5,
                _ => 0.0,
            };
            star(ui, pos2(x + STAR_R + k as f32 * STAR_PITCH, y), fill, pal.gold);
        }
        x += 4.0 * STAR_PITCH + 2.0 * STAR_R + STARS_GAP;
    }
    if let Some(facts) = facts(now) {
        studio::text(ui, pos2(x, mid), &facts, figures, pal.dim, (row.max.x - x).max(1.0), Align2::LEFT_CENTER);
    }
}

/// A rating star, its ink's middle at `at`: filled in `color` for `fill` of
/// it from the left (whole, half or none), its outline over that. Drawn,
/// for none of the panel's faces has the cells' ★ or ☆ (Segoe UI, Hack and
/// Ubuntu Light lack both), and a borrowed one would set them in another
/// size on each platform. The fill is a fan from the middle, the star being
/// all in sight of it; the outline's stroke smooths its edge.
fn star(ui: &Ui, at: Pos2, fill: f32, color: Color32) {
    use std::f32::consts::{FRAC_PI_2, PI};
    // The ink's middle sits over the points' centre: the top point reaches
    // the radius, the feet only its cosine of a tenth of a turn.
    let centre = at + vec2(0.0, STAR_R * (1.0 - (PI / 5.0).cos()) / 2.0);
    let points: Vec<Pos2> = (0..10)
        .map(|k| {
            let a = -FRAC_PI_2 + k as f32 * PI / 5.0;
            let r = if k % 2 == 0 { STAR_R } else { STAR_R * STAR_INNER };
            centre + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    if fill > 0.0 {
        let mut mesh = Mesh::default();
        mesh.colored_vertex(centre, color);
        for point in &points {
            mesh.colored_vertex(*point, color);
        }
        for k in 0..10u32 {
            mesh.add_triangle(0, 1 + k, 1 + (k + 1) % 10);
        }
        let reach = egui::Rect::from_center_size(centre, Vec2::splat(2.0 * STAR_R + 2.0));
        let shown = reach.with_max_x(reach.min.x + 1.0 + fill * 2.0 * STAR_R);
        ui.painter().with_clip_rect(shown.intersect(ui.clip_rect())).add(Shape::mesh(mesh));
    }
    ui.painter().add(Shape::closed_line(points, Stroke::new(1.0, color)));
}

/// The cover in `slot` (D15): the playing track's picture as a texture, made
/// once for each picture at the size the slot draws it (`studio::halvings`,
/// as the window's other pictures are fitted down) and drawn whole with the
/// desk's corners; until it is decoded, the empty slot, a hairline in the
/// strong edge.
fn cover(ui: &Ui, slot: egui::Rect, art: Option<&Art>, pal: &Pal) {
    let picture = art.and_then(|art| {
        let crisp = art.is_crisp();
        let size = vec2(art.width() as f32, art.height() as f32);
        let halvings = if crisp { 0 } else { studio::halvings(size, slot, ui.ctx().pixels_per_point()) };
        studio::texture(ui.ctx(), Id::new("bar-cover").with(halvings), art.id(), crisp, || {
            let mut image = egui::ColorImage::from_rgb([art.width() as usize, art.height() as usize], art.rgb());
            for _ in 0..halvings {
                image = studio::halved(&image);
            }
            Some(image)
        })
    });
    match picture {
        Some(texture) => {
            studio::picture_in(ui, slot, &texture, COVER_RADIUS);
        }
        None => {
            ui.painter().rect_stroke(slot, studio::square(), Stroke::new(1.0, pal.edge_strong), StrokeKind::Inside);
        }
    }
}

#[cfg(test)]
mod tests;
