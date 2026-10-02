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
//! The replay is paced: one act a frame ([`HeldInput::next_step`]), as the
//! script lever plays its steps, because hands are never faster than a
//! frame and the GUI counts on that — a key that opens a room acts on what
//! the next frame draws, so `/ab` fed in one burst opened the search and
//! lost `ab`. What came while the queue drains joins it at the back, so
//! nothing overtakes what was held.
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

    /// The next frame's share of the replay, oldest first: everything up to
    /// and including the next act ([`acts`]), or what is left when no act
    /// is. The moves, releases and window changes before an act ride with
    /// it — they change nothing the next input lands on that a frame must
    /// draw first — so a frame runs between any two acts and no more
    /// frames than acts are spent.
    pub(super) fn next_step(&mut self) -> Vec<Held> {
        let end = self.items.iter().position(acts).map_or(self.items.len(), |at| at + 1);
        self.items.drain(..end).collect()
    }

    /// How many are held, and how many were dropped from the front on the
    /// way (that count is taken: it reads zero after).
    pub(super) fn count(&mut self) -> (usize, u64) {
        (self.items.len(), std::mem::take(&mut self.dropped))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// An input that acts on the screen as the last frame drew it, and may
/// change what the next one lands on: a key or a button pressed, text an
/// input method or the clipboard gave. A frame must draw between two of
/// them (a `/` opens the search, whose field the next key types into; a
/// click opens a room the next click is on).
fn acts(held: &Held) -> bool {
    matches!(
        held,
        Held::Raw(
            Raw::Key { pressed: true, .. }
                | Raw::Button { down: true, .. }
                | Raw::ImeCommit(_)
                | Raw::Paste(_)
        )
    )
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

    fn release(c: char) -> Held {
        match key(c) {
            Held::Raw(Raw::Key { named, text, bare, physical, mods, repeat, .. }) => {
                Held::Raw(Raw::Key { named, text, bare, physical, mods, pressed: false, repeat })
            }
            _ => unreachable!(),
        }
    }

    /// Everything held, oldest first, step by step: what the replay feeds.
    fn take(held: &mut HeldInput) -> Vec<Held> {
        std::iter::from_fn(|| Some(held.next_step()).filter(|step| !step.is_empty()))
            .flatten()
            .collect()
    }

    #[test]
    fn held_inputs_come_back_in_the_order_they_came() {
        let mut held = HeldInput::default();
        for item in [key('/'), moved(3.0), button(true), button(false), key('q')] {
            held.push(item);
        }
        assert_eq!(held.count(), (5, 0));
        assert_eq!(take(&mut held), [key('/'), moved(3.0), button(true), button(false), key('q')]);
        assert!(held.is_empty(), "taking empties the queue");
    }

    #[test]
    fn a_full_queue_drops_its_oldest_and_counts_them() {
        let mut held = HeldInput::default();
        let letters: Vec<char> = ('a'..='z').cycle().take(HOLD + 3).collect();
        for &c in &letters {
            held.push(key(c));
        }
        assert_eq!(held.count(), (HOLD, 3));
        assert_eq!(held.count().1, 0, "the dropped count is taken");
        let items = take(&mut held);
        assert_eq!(items.len(), HOLD);
        // The three oldest went; the newest is last.
        assert_eq!(items.first(), Some(&key(letters[3])));
        assert_eq!(items.last(), Some(&key(*letters.last().unwrap())));
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
        let items = take(&mut held);
        // The run of two moves is its last; the move after the resize and
        // the scale is a run of its own (either may move the grid under
        // the pointer); the
        // press and the release each keep the move before them; the two
        // leaves are one; the resize and the scale are their last, last.
        assert_eq!(
            items,
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

    /// `/ab` typed into a blank window, with the pointer moving and the
    /// keys let go between: the replay feeds one key press a frame, each
    /// with the moves and releases that came before it, so the `/`'s
    /// search is drawn before `a` reaches its field.
    #[test]
    fn the_replay_feeds_one_act_a_frame() {
        let mut held = HeldInput::default();
        for item in [
            moved(1.0),
            key('/'),
            release('/'),
            moved(2.0),
            key('a'),
            release('a'),
            key('b'),
            release('b'),
            button(true),
            button(false),
            Held::Resized,
        ] {
            held.push(item);
        }
        let steps: Vec<Vec<Held>> =
            std::iter::from_fn(|| Some(held.next_step()).filter(|step| !step.is_empty())).collect();
        assert_eq!(
            steps,
            [
                vec![moved(1.0), key('/')],
                vec![release('/'), moved(2.0), key('a')],
                vec![release('a'), key('b')],
                vec![release('b'), button(true)],
                // No act is left: the tail goes in one frame.
                vec![button(false), Held::Resized],
            ]
        );
        assert!(held.is_empty());
        assert!(held.next_step().is_empty(), "an empty queue has no step");
    }

    /// Text from an input method and the clipboard is an act like a key:
    /// each is a frame of its own.
    #[test]
    fn text_given_whole_is_an_act_too() {
        let mut held = HeldInput::default();
        for item in [
            Held::Raw(Raw::ImePreedit("か".into())),
            Held::Raw(Raw::ImeCommit("火".into())),
            Held::Raw(Raw::Paste("x".into())),
            Held::Raw(Raw::Wheel(crate::gui::window::input::Wheel::Lines(1.0))),
        ] {
            held.push(item);
        }
        assert_eq!(held.next_step().len(), 2, "the preedit rides with the commit");
        assert_eq!(held.next_step(), [Held::Raw(Raw::Paste("x".into()))]);
        assert_eq!(held.next_step().len(), 1, "the wheel is the tail");
        assert!(held.is_empty());
    }
}
