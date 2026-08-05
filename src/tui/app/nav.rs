//! The one drill-down, written once.
//!
//! Library, Search and Discover each hand-rolled the same machine around a
//! Vec of nodes: a last-or-root accessor, a push on the way in, a back that
//! floors at the root menu, and — the rule that actually matters — a guard
//! that drops a reply for a view the user has already left. Nine blocks,
//! one concept, and the guard was maintained by hand in three places while
//! a fourth copy never got written at all (audits #58 and #22).
//!
//! What stays per-tab is what genuinely differs: which views are static
//! menus, which need refetching on the way back, and Discover's re-anchor
//! at the top. Those live in the tabs' own arms, around this.

/// Where a tab is in its node hierarchy, and the rules for moving.
#[derive(Debug)]
pub struct Drill<N> {
    /// The trail in, oldest first. The tab's opening view is planted as the
    /// first element on first visit, so the floor of `back` is the root
    /// menu itself, never past it.
    stack: Vec<N>,
    /// What [`Drill::here`] answers before the tab has ever been opened.
    root: N,
}

impl<N: PartialEq + Clone> Drill<N> {
    pub fn new(root: N) -> Self {
        Drill { stack: Vec::new(), root }
    }

    /// The view on screen.
    pub fn here(&self) -> &N {
        self.stack.last().unwrap_or(&self.root)
    }

    /// Whether the tab has never been visited — the caller plants the root
    /// (and its entries) on the first look.
    pub fn unopened(&self) -> bool {
        self.stack.is_empty()
    }

    /// Step in.
    pub fn enter(&mut self, node: N) {
        self.stack.push(node);
    }

    /// Step out, to the view now on screen. `None` is the floor: the first
    /// entry never pops, so Back always ends at the tab's own root menu.
    pub fn back(&mut self) -> Option<N> {
        if self.stack.len() <= 1 {
            return None;
        }
        self.stack.pop();
        Some(self.here().clone())
    }

    /// Whether a reply about `node` is about the view being looked at.
    /// This is the stale-reply rule: anything else describes a view the
    /// user has already left, and applying it would overwrite the screen
    /// they went to instead.
    pub fn wants(&self, node: &N) -> bool {
        self.here() == node
    }

    /// Forget the whole trail, as a fresh search does.
    pub fn reset(&mut self) {
        self.stack.clear();
    }

    /// Start over at an opened root menu — the state a first visit plants.
    pub fn restart(&mut self) {
        self.stack.clear();
        self.stack.push(self.root.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drill_starts_at_root_floors_there_and_guards_what_it_shows() {
        let mut drill = Drill::new("root");
        assert_eq!(drill.here(), &"root", "unopened still answers with the root");
        assert!(drill.unopened());
        assert!(drill.back().is_none(), "nowhere to come back from");

        drill.enter("root"); // the first visit plants the root menu
        drill.enter("artists");
        drill.enter("albums");
        assert_eq!(drill.here(), &"albums");
        assert!(drill.wants(&"albums"), "the reply for the view on screen lands");
        assert!(!drill.wants(&"artists"), "a reply for a view left behind does not");

        assert_eq!(drill.back(), Some("artists"));
        assert_eq!(drill.back(), Some("root"));
        assert!(drill.back().is_none(), "the root menu is the end of Back");
        assert_eq!(drill.here(), &"root", "and still the view on screen");

        drill.reset();
        assert!(drill.unopened(), "a reset forgets the whole trail");
    }
}
