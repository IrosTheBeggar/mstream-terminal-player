//! One cover drawn into a rect: real pixels where the terminal draws them
//! and the frame's budget allows, the ▀-mosaic otherwise — the card
//! cover's two paths, paced. Shared by every surface that draws MANY
//! covers in one frame (the album wall, the queue panel): each gives every
//! cell a [`Slot`] of its own, because the pixel cache holds ONE encoded
//! picture and the mosaic pane ONE resampled grid, so slots may not share.
//!
//! Feasibility (2026-08-29, measured by the wall's tests): a page of
//! fifteen covers renders fine down both paths. The mosaic path is
//! character cells — tens of milliseconds a frame for a page. The pixel
//! path is encoding, render-time work on the thread the keyboard waits
//! on, and a debug-build sixel page measured two whole seconds. So
//! encodes are PACED: each frame spends at most [`ENCODE_BUDGET`] starting
//! new encodes, the mosaic stands in for slots not yet paid for, and a
//! page turn upgrades to pixels over a few frames instead of freezing on
//! the first.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::tui::art::Art;

/// How long one frame may spend STARTING cover encodes. Kitty pays ~1 ms
/// a grid-sized cover in release and the whole page lands in one frame;
/// debug-build sixel pays ~140 ms each and fills in one or two a frame —
/// which is the point: the budget bounds the frame, not the page. Each
/// surface paces its own frame's work; two on screen at once (the wall
/// with the queue open) can spend two budgets, which kitty never reaches
/// and sixel rations either way.
const ENCODE_BUDGET: Duration = Duration::from_millis(40);

/// One slot's drawing state: its own pixel-protocol cache and its own
/// mosaic grid, so neither thrashes when a whole page draws in one frame.
pub(super) struct Slot {
    graphics: crate::tui::graphics::Graphics,
    pane: crate::tui::viz::CoverPane,
    /// What this slot last drew as pixels — (art id, cell size). Matching
    /// means the protocol cache below is warm and drawing is free; a
    /// mismatch is an encode, which only the frame's budget may buy.
    pub(super) key: Option<(u64, u16, u16)>,
}

impl Slot {
    pub(super) fn new(graphics: crate::tui::graphics::Graphics) -> Slot {
        Slot { graphics, pane: Default::default(), key: None }
    }

    /// A resize changes the cell-to-pixel mapping the slot encoded
    /// against, and kitty forgets its transmitted images with the old
    /// terminal — the card cover's rule, per slot.
    pub(super) fn on_resize(&mut self) {
        self.graphics.refresh();
        self.key = None;
    }

    /// The cover, pixels where the terminal draws them and the budget
    /// allows, the ▀-mosaic otherwise.
    pub(super) fn draw_paced(&mut self, frame: &mut Frame, rect: Rect, art: &Art, pace: &Pace) {
        let want = (art.id(), rect.width, rect.height);
        if self.key == Some(want) {
            // Warm: drawing is the cached protocol entry, microseconds.
            if self.graphics.draw(frame, rect, art) {
                return;
            }
            self.key = None;
        } else if pace.allows() {
            if self.graphics.draw(frame, rect, art) {
                self.key = Some(want);
                return;
            }
            self.key = None;
        } else {
            // Starving only happens once real encode time has been spent
            // this frame — a mosaic-only terminal never elapses the
            // budget, so this can't spin an idle session hot.
            pace.starved.set(true);
        }
        self.draw_mosaic(frame, rect, art);
    }

    /// The ▀-mosaic alone: plain cells, which layer under anything drawn
    /// over them the way text does. What a slot draws while an overlay
    /// stands over its column — a picture's cells are marked skipped for
    /// the terminal writer (ratatui-image's rule), so a modal's text over
    /// them would never land.
    pub(super) fn draw_mosaic(&mut self, frame: &mut Frame, rect: Rect, art: &Art) {
        // The pixel path's cache is stale once cells were drawn over it;
        // the next pixel draw must transmit again.
        self.key = None;
        let mut canvas = crate::tui::canvas::Canvas::new(rect);
        if !canvas.is_empty() {
            self.pane.draw(&mut canvas, art);
            frame.render_widget(ratatui::widgets::Paragraph::new(canvas.into_lines()), rect);
        }
    }
}

/// One frame's encode allowance, shared by every slot it draws. A slot
/// the budget turned away marks it STARVED — the caller's cue to run the
/// next frame promptly instead of idling out the poll, so a page turn
/// finishes upgrading in tens of milliseconds of wall clock rather than
/// one encode per 100 ms tick.
pub(super) struct Pace {
    start: Instant,
    budget: Duration,
    starved: std::cell::Cell<bool>,
}

impl Pace {
    pub(super) fn frame() -> Pace {
        Pace { start: Instant::now(), budget: ENCODE_BUDGET, starved: std::cell::Cell::new(false) }
    }

    fn allows(&self) -> bool {
        self.start.elapsed() < self.budget
    }

    pub(super) fn starved(&self) -> bool {
        self.starved.get()
    }
}
