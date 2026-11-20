//! The Peer recs room's panes (peer-recs contract §1.10): what the list rect holds when the
//! shown tab has no leads to draw, under the heads and the rule, as Discover draws its own.
//!
//! - Loading, the first ask: four `░` rows per column, runs of fixed lengths so they read as
//!   rows to come (9, 6, 10 and 7 cells under TRACK, 11, 8, 9 and 12 under ARTIST);
//! - No peers yet: the shelf has nothing to walk;
//! - Nothing new, then `↻ new seeds`;
//! - Try again, alone;
//! - the scan-pending sentence, then Check again.
//!
//! The buttons are Discover's again button, accent BOLD, on the frame's surface. Off is never a
//! pane: the room yields to Files instead.

use ratatui::Frame;
use ratatui::layout::Rect;
use rust_i18n::t;

use crate::kit::theme::legacy_conhost;
use crate::kit::{Surface, dim};
use crate::tui::app::peer_recs::rules::RecsPane;

use super::super::discover::{again_button, refresh_glyph, sentence_lines};
use super::super::{Act, put};
use super::PeerAct;
use super::grid::{GridGeom, head_offsets};

/// The skeleton's runs per row, under TRACK (at +2) and under ARTIST (at +19 at 38 cells).
const SKELETON: [(u16, u16); 4] = [(9, 11), (6, 8), (10, 9), (7, 12)];

/// Between `Nothing new` and its button.
const BUTTON_GAP: u16 = 2;

/// The pane `pane` in `list`. `Ready` draws nothing: the grid holds the list then.
pub(super) fn draw(frame: &mut Frame, ui: &mut Surface<Act>, list: Rect, geom: &GridGeom, pane: RecsPane) {
    if list.height == 0 {
        return;
    }
    let (x, y) = (list.x, list.y);
    match pane {
        RecsPane::Ready => {}
        RecsPane::Loading => draw_skeleton(frame, list, geom),
        RecsPane::NoPeers => put(frame, x, y, &t!("gui.peer_recs.no_peers"), dim()),
        RecsPane::NothingNew => {
            let words = t!("gui.peer_recs.nothing_new").to_string();
            put(frame, x, y, &words, dim());
            let seeds = t!("gui.discover.new_seeds").to_string();
            let label = match refresh_glyph() {
                Some(glyph) => format!("{glyph} {seeds}"),
                None => seeds,
            };
            let at = x + crate::kit::width(&words) as u16 + BUTTON_GAP;
            again_button(frame, ui, (at, y), &label, Act::Peer(PeerAct::NewSeeds));
        }
        RecsPane::TryAgain => {
            let label = t!("gui.discover.retry").to_string();
            again_button(frame, ui, (x, y), &label, Act::Peer(PeerAct::TryAgain));
        }
        RecsPane::ScanPending => {
            // The sentence leaves the button its row.
            let short = Rect { height: list.height.saturating_sub(1), ..list };
            let shown = sentence_lines(frame, short, &t!("gui.discover.scan_pending"), dim());
            let label = t!("gui.discover.check_again").to_string();
            again_button(frame, ui, (x, y + shown), &label, Act::Peer(PeerAct::CheckAgain));
        }
    }
}

/// The first ask's rows: fixed runs where TRACK and ARTIST will stand in each column, TRACK
/// alone where the cells are narrow. `·` on the legacy console, whose fonts lack `░`.
fn draw_skeleton(frame: &mut Frame, list: Rect, geom: &GridGeom) {
    let glyph = if legacy_conhost() { "·" } else { "░" };
    let run = |cells: u16| glyph.repeat(usize::from(cells));
    let (track_x, artist_x) = head_offsets(geom);
    for (row, (track, artist)) in SKELETON.into_iter().enumerate().take(usize::from(list.height)) {
        let y = list.y + row as u16;
        for x in [geom.ax, geom.bx] {
            put(frame, x + track_x, y, &run(track), dim());
            if let Some(artist_x) = artist_x {
                put(frame, x + artist_x, y, &run(artist), dim());
            }
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;

    use super::super::grid::tests::{cells, rects};
    use super::super::grid::{draw_heads, geom};
    use super::super::kit::english;
    use super::*;
    use crate::kit::boards::{Area, assert_frame};

    /// The heads, the rule and `pane` at `width × height` (key hints off), with the surface
    /// they registered on.
    fn drawn(pane: RecsPane, (width, height): (u16, u16), queue_open: bool) -> (Buffer, Surface<Act>) {
        crate::kit::theme::pin_modern_terminal();
        let (content, heads_y, list) = rects(width, height, queue_open);
        let geom = geom(list, true);
        let mut ui = Surface::new();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal
            .draw(|frame| {
                draw_heads(frame, content, &geom, heads_y);
                draw(frame, &mut ui, list, &geom, pane);
            })
            .expect("the pane draws");
        (terminal.backend().buffer().clone(), ui)
    }

    fn at(x: u16, y: u16) -> Position {
        Position { x, y }
    }

    #[test]
    fn panes_draw_their_words_and_buttons() {
        let _locale = english();
        let peer = |act| Some(Act::Peer(act));
        let (buf, ui) = drawn(RecsPane::NoPeers, (100, 30), false);
        assert_frame(&buf, "S1", "", Area::rect(17, 99, 7, 11));
        assert_eq!(ui.hit(at(17, 9)), None, "No peers yet is words alone");

        let (buf, ui) = drawn(RecsPane::NothingNew, (100, 30), false);
        assert_frame(&buf, "S2", "", Area::rect(17, 99, 7, 11));
        assert_eq!(ui.hit(at(17, 9)), None);
        assert_eq!(ui.hit(at(30, 9)), peer(PeerAct::NewSeeds));
        assert_eq!(ui.hit(at(40, 9)), peer(PeerAct::NewSeeds));
        assert_eq!(ui.hit(at(41, 9)), None);

        let (buf, ui) = drawn(RecsPane::TryAgain, (100, 30), false);
        assert_frame(&buf, "S3", "", Area::rect(17, 99, 7, 11));
        assert_eq!(ui.hit(at(17, 9)), peer(PeerAct::TryAgain));
        assert_eq!(ui.hit(at(25, 9)), peer(PeerAct::TryAgain));

        let (buf, ui) = drawn(RecsPane::ScanPending, (100, 30), false);
        assert_frame(&buf, "S4", "", Area::rect(17, 99, 7, 11));
        assert_eq!(ui.hit(at(9, 9)), None);
        assert_eq!(ui.hit(at(17, 11)), peer(PeerAct::CheckAgain));
        assert_eq!(ui.hit(at(17, 10)), None);

        // The words follow the list: at the floor it starts a row higher, and with the queue
        // open the sentence wraps in its 45 cells.
        let (buf, _) = drawn(RecsPane::NoPeers, (100, 24), false);
        assert_eq!(cells(&buf, 17, 8, 12), "No peers yet");
        let (buf, ui) = drawn(RecsPane::ScanPending, (100, 30), true);
        assert!(cells(&buf, 17, 9, 47).trim_end().chars().count() <= 45);
        let shown = (9..=24).take_while(|&y| !cells(&buf, 17, y, 45).trim().is_empty()).count() as u16;
        assert_eq!(cells(&buf, 17, 9 + shown - 1, 11), "Check again");
        assert_eq!(ui.hit(at(17, 9 + shown - 1)), peer(PeerAct::CheckAgain));

        // Ready is the grid's: the pane draws nothing under the rule.
        let (buf, ui) = drawn(RecsPane::Ready, (100, 30), false);
        assert!((9..=24).all(|y| cells(&buf, 17, y, 80).trim().is_empty()));
        assert_eq!(ui.hit(at(17, 9)), None);
    }

    #[test]
    fn the_loading_skeleton_runs_are_fixed() {
        let _locale = english();
        let (buf, ui) = drawn(RecsPane::Loading, (100, 30), false);
        assert_frame(&buf, "W1", "", Area::rect(17, 99, 7, 13));
        assert_eq!(ui.hit(at(19, 9)), None, "the skeleton presses nothing");
        // Four rows at most, fewer when the list is shorter.
        let runs = |buf: &Buffer, y: u16| cells(buf, 17, y, 80).chars().filter(|c| *c == '░').count();
        assert_eq!([runs(&buf, 9), runs(&buf, 10), runs(&buf, 11), runs(&buf, 12)], [40, 28, 38, 38]);
        // With the queue open ARTIST drops, and its runs with it.
        let (buf, _) = drawn(RecsPane::Loading, (100, 30), true);
        assert_eq!(cells(&buf, 19, 9, 9), "░".repeat(9));
        assert_eq!(cells(&buf, 42, 9, 9), "░".repeat(9));
        assert_eq!(runs(&buf, 9), 18);
        assert!((9..=12).all(|y| cells(&buf, 17, y, 47).chars().filter(|c| *c == '░').count() < 30));
    }
}
