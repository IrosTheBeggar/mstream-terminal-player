//! Input that came before there was anything to give it to.
//!
//! The window is on screen (and taking keys and clicks) before its renderer
//! is: the early threads and the backend's build run while the loop answers
//! the window, and on a slow GPU that is a second or more. Until the first
//! grid exists the GUI cannot be fed — a click is a cell only on a grid,
//! and a key acts on a screen that has drawn — so what came meanwhile is
//! held here, in order, and replayed through the window's own handler once
//! the first frame has drawn: a `/` typed while the window was blank opens
//! the search, and a click lands on the cell it would have hit.
//!
//! What is held is the window's own [`Raw`], not winit's `WindowEvent`
//! (which is `Clone` in 0.30 and could be): a key's meaning is read with
//! the modifiers held when it arrived, which the replay could no longer
//! know, and the script lever's inputs are `Raw` already, so both reach the
//! replay the same way. A resize and a scale change are held as the fact
//! that one came: the window is asked its size and scale again when they
//! are replayed, since only the latest of either still describes it.

use std::collections::VecDeque;

use super::input::Raw;

/// How many inputs are held at most. A person types and clicks a few dozen
/// in a slow start; past this the oldest go, counted, rather than the
/// queue growing without end while a GPU that never comes is waited on.
const HOLD: usize = 256;

/// One held input.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Held {
    Raw(Raw),
    /// The window was resized: its size now is what the replay applies.
    Resized,
    /// The display's scale changed: the window's scale now is what the
    /// replay applies.
    Scale,
}

/// The inputs held while the renderer is built, oldest first.
#[derive(Default)]
pub(super) struct HeldInput {
    items: VecDeque<Held>,
    /// Inputs dropped from the front because the queue was full.
    dropped: u64,
}

impl HeldInput {
    /// Hold one more. A pointer move straight after another replaces it —
    /// only where the pointer ended up matters to what comes next, and a
    /// drag across the blank window would otherwise fill the queue with
    /// moves — while a move with a button between keeps its place, so a
    /// press and its release each land where they were made. A resize or
    /// a scale change replaces any held before it, wherever it stood: only
    /// the last describes the window. Two leaves in a row are one.
    pub(super) fn push(&mut self, item: Held) {
        match (&item, self.items.back()) {
            (Held::Raw(Raw::Move { .. }), Some(Held::Raw(Raw::Move { .. })))
            | (Held::Raw(Raw::Leave), Some(Held::Raw(Raw::Leave))) => {
                self.items.pop_back();
            }
            (Held::Resized, _) => self.items.retain(|held| *held != Held::Resized),
            (Held::Scale, _) => self.items.retain(|held| *held != Held::Scale),
            _ => {}
        }
        if self.items.len() >= HOLD {
            self.items.pop_front();
            self.dropped += 1;
        }
        self.items.push_back(item);
    }

    /// Everything held, oldest first, and how many were dropped on the way;
    /// the queue is left empty.
    pub(super) fn take(&mut self) -> (VecDeque<Held>, u64) {
        (std::mem::take(&mut self.items), std::mem::take(&mut self.dropped))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::window::input::{Button, Mods};

    fn key(c: char) -> Held {
        Held::Raw(Raw::Key {
            named: None,
            text: Some(c.to_string()),
            bare: Some(c),
            physical: None,
            mods: Mods::default(),
            pressed: true,
            repeat: false,
        })
    }

    fn moved(x: f64) -> Held {
        Held::Raw(Raw::Move { x, y: 1.0 })
    }

    fn button(down: bool) -> Held {
        Held::Raw(Raw::Button { button: Button::Left, down })
    }

    #[test]
    fn held_inputs_come_back_in_the_order_they_came() {
        let mut held = HeldInput::default();
        for item in [key('/'), moved(3.0), button(true), button(false), key('q')] {
            held.push(item);
        }
        let (items, dropped) = held.take();
        assert_eq!(
            Vec::from(items),
            [key('/'), moved(3.0), button(true), button(false), key('q')]
        );
        assert_eq!(dropped, 0);
        assert!(held.is_empty(), "taking empties the queue");
    }

    #[test]
    fn a_full_queue_drops_its_oldest_and_counts_them() {
        let mut held = HeldInput::default();
        let letters: Vec<char> = ('a'..='z').cycle().take(HOLD + 3).collect();
        for &c in &letters {
            held.push(key(c));
        }
        let (items, dropped) = held.take();
        assert_eq!(items.len(), HOLD);
        assert_eq!(dropped, 3);
        // The three oldest went; the newest is last.
        assert_eq!(items.front(), Some(&key(letters[3])));
        assert_eq!(items.back(), Some(&key(*letters.last().unwrap())));
        assert_eq!(held.take().1, 0, "the count is taken with the queue");
    }

    #[test]
    fn only_the_last_of_a_run_of_moves_and_of_each_resize_and_scale_is_kept() {
        let mut held = HeldInput::default();
        for item in [
            moved(1.0),
            moved(2.0),
            Held::Resized,
            Held::Scale,
            moved(3.0),
            button(true),
            moved(4.0),
            moved(5.0),
            button(false),
            Held::Raw(Raw::Leave),
            Held::Raw(Raw::Leave),
            Held::Resized,
            Held::Scale,
        ] {
            held.push(item);
        }
        let (items, _) = held.take();
        // The run of two moves is its last; the move after the resize and
        // the scale is a run of its own (either may move the grid under
        // the pointer); the
        // press and the release each keep the move before them; the two
        // leaves are one; the resize and the scale are their last, last.
        assert_eq!(
            Vec::from(items),
            [
                moved(2.0),
                moved(3.0),
                button(true),
                moved(5.0),
                button(false),
                Held::Raw(Raw::Leave),
                Held::Resized,
                Held::Scale,
            ]
        );
    }
}
