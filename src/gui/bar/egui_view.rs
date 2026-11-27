//! The transport bar's egui face in the GUI's own window, hosted by the
//! chrome (src/gui/chrome.rs): paint only, in the very boxes the cells drew
//! and registered (`BarLayout`), whose registry keeps every press, right
//! press and hand the bar has; lit where the registry says the pointer is.
//!
//! The face is still to be drawn: the Studio look of Main's BAR, SEEK ROW,
//! AUTO-DJ SPLIT, TRANSPORT GROUP, VOLUME, NOTE SLOT and CARD patterns and
//! the TransportStates, TransportParts and NoteBanner boards (decisions
//! D11-D17) is not here yet. What stands here is an honest placeholder: the
//! bar's words in their boxes, its controls in the kit's frames and the
//! seek line on the kit's track, the cover in its slot. [`READY`] stays
//! false until the face is drawn: the cells draw the bar meanwhile.

use egui::{Align2, Color32, Id, Ui};
use ratatui::layout::Rect;

use super::{BarLayout, BarView, NoteTone, byline, cell_fraction, facts, fmt_time, volume_cells, volume_words};
use crate::gui::Act;
use crate::gui::window::panel::Cells;
use crate::gui::window::studio::{self, Mark, Pal};

/// Whether egui paints the bar (the host draws the cells bar until it does).
pub(crate) const READY: bool = false;

/// What the bar's face is handed besides the view and the layout.
pub(crate) struct BarCtx<'a> {
    pub cells: Cells,
    /// The control a press would work, by its cells and its act, which
    /// lights; none at rest, and none where something stands over it.
    pub hot: Option<(Rect, Act)>,
    pub pal: &'a Pal,
}

/// Paint the bar in the boxes the cells registered, on `ui` (its child over
/// the bar's rows, clipped to them, the ground painted). It takes no
/// pointer: its presses are the cells registry's. A placeholder: see the
/// module's words.
pub(crate) fn show(ui: &mut Ui, view: &BarView, layout: &BarLayout, cx: &BarCtx) {
    let pal = cx.pal;
    let at = |cells: Rect| cx.cells.rect(cells);
    let hot = |act: &Act| cx.hot.as_ref().is_some_and(|(_, hot)| hot == act);
    let words = |ui: &Ui, cells: Rect, s: &str, color: Color32| {
        let rect = at(cells);
        studio::text(ui, rect.left_center(), s, studio::body(13.0), color, rect.width(), Align2::LEFT_CENTER);
    };
    match (&layout.seek, view.now) {
        (Some(seek), Some(now)) => {
            words(ui, seek.elapsed, &fmt_time(now.elapsed), pal.dim);
            let played = if now.duration > 0.0 { (now.elapsed / now.duration) as f32 } else { 0.0 };
            let ghost = cx.hot.as_ref().and_then(|(rect, act)| match act {
                Act::Seek(_) => Some(cell_fraction(usize::from(rect.x.saturating_sub(seek.track.x)), seek.cells) as f32),
                _ => None,
            });
            studio::seek_track(ui, at(seek.track), played, ghost, pal);
            words(ui, seek.total, &fmt_time(now.duration), pal.dim);
        }
        _ => {
            let line = at(Rect { height: 1, ..layout.rect });
            ui.painter().hline(line.x_range(), line.center().y, egui::Stroke::new(1.0, pal.gold));
        }
    }
    let dj = if view.autodj { pal.ok } else { pal.dim };
    words(ui, layout.dj.word, " auto-dj", if hot(&Act::AutoDj) { pal.text } else { dj });
    let chevron = at(layout.dj.chevron).center();
    studio::chevron(ui, chevron, if view.quick_open { pal.text } else { pal.dim }, !view.quick_open);
    // The play slot's mark as the cells' glyph (`play_glyphs`): the pause
    // whenever the bar is not paused, idle included (ruling R14, open).
    let marks = [Mark::Repeat, Mark::Prev, if view.paused { Mark::Play } else { Mark::Pause }, Mark::Next, Mark::Shuffle];
    for ((rect, act), mark) in layout.group.iter().zip(marks) {
        let (frame, weight) = match act {
            Act::PlayPause => (pal.gold, 2.0),
            Act::Repeat if view.repeat => (pal.ok, 1.0),
            Act::Shuffle if view.shuffle => (pal.ok, 1.0),
            Act::Repeat | Act::Shuffle => (pal.dim, 1.0),
            _ => (pal.edge_strong, 1.0),
        };
        studio::frame_button(ui, at(*rect).shrink(1.0), mark, frame, pal.text, weight, hot(act), pal);
    }
    words(ui, layout.volume.minus, "−", pal.dim);
    let lit = volume_cells(view.volume);
    for (i, cell) in layout.volume.cells.iter().enumerate() {
        let rect = at(*cell).shrink2(egui::vec2(1.0, 3.0));
        ui.painter().rect_filled(rect, 1.0, if i < lit { pal.accent } else { pal.unlit });
    }
    words(ui, layout.volume.plus, "+", pal.dim);
    words(ui, layout.volume.readout, &volume_words(view.volume), pal.dim);
    if let Some((note, tone)) = &view.note {
        let color = match tone {
            NoteTone::Dim => pal.dim,
            NoteTone::Error => pal.gold,
            NoteTone::Busy => pal.accent,
            NoteTone::Success => pal.ok,
            NoteTone::Plain => pal.text,
        };
        studio::note_in(ui, at(layout.note), note, color);
    }
    let card = &layout.card;
    let cover = at(card.cover);
    let picture = view.art.and_then(|art| {
        studio::texture(ui.ctx(), Id::new("bar-cover"), art.id(), art.is_crisp(), || {
            Some(egui::ColorImage::from_rgb([art.width() as usize, art.height() as usize], art.rgb()))
        })
    });
    match picture {
        Some(texture) => {
            studio::picture_in(ui, cover, &texture, 3);
        }
        None => {
            ui.painter().rect_stroke(cover, studio::square(), egui::Stroke::new(1.0, pal.edge_strong), egui::StrokeKind::Inside);
        }
    }
    let lit = hot(&Act::ToggleQueue);
    match view.now {
        Some(now) => {
            words(ui, card.lines[0], &now.title, if lit { pal.text } else { pal.soft });
            words(ui, card.lines[1], &byline(now), pal.dim);
            words(ui, card.lines[2], &now.spec, pal.dim);
            let stars = super::super::actions::stars(now.rating.filter(|r| *r > 0));
            let fourth = match facts(now) {
                Some(facts) if !stars.is_empty() => format!("{stars}  {facts}"),
                Some(facts) => facts,
                None => stars,
            };
            words(ui, card.lines[3], &fourth, pal.dim);
        }
        None => words(ui, card.lines[0], &rust_i18n::t!("gui.nothing_playing"), pal.dim),
    }
    studio::chevron(ui, at(card.chevron).center(), if lit { pal.text } else { pal.dim }, !view.queue_open);
}
