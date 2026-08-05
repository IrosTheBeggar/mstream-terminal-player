//! What plays next, and where "current" lands when a row is removed — the
//! rules the engine's queue and the TUI's queue share.
//!
//! They were written twice and synced by hand, with a comment on each side
//! admitting it (audit #62). The copies had already drifted once, and then
//! drifted deliberately: the TUI's shuffle learned to count its pass
//! (audit #35) while the engine still plays a shuffled queue forever
//! (audit #9, deferred). That difference is a parameter here, not a fork.
//!
//! Loop modes stay per-side — the engine's speaks the serve API's "none",
//! the TUI's speaks the config file's "off" — and each maps to [`Loop`]
//! exactly once, next to its own definition.

/// Loop/repeat, in the only three shapes either queue has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loop {
    Off,
    All,
    One,
}

/// Pick the next index to play.
///
/// `manual` marks a user-pressed skip, which is never trapped by loop-one —
/// that mode only applies to automatic track-end advancement. `pass_spent`
/// is how many tracks the current shuffle pass has started, for the queue
/// that counts one; `None` means nobody is counting and a shuffled queue
/// under `Loop::Off` never ends.
pub fn pick_next(
    len: usize,
    current: Option<usize>,
    shuffle: bool,
    looping: Loop,
    manual: bool,
    pass_spent: Option<usize>,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(current) = current else {
        return Some(0);
    };
    if !manual && looping == Loop::One {
        return Some(current);
    }
    if shuffle {
        // No position to run out of, so the end is counted instead: the
        // pass is over once as many tracks have started as the queue
        // holds. Picks repeat, so this bounds the pass rather than
        // promising that everything in it was played.
        if looping == Loop::Off && pass_spent.is_some_and(|spent| spent >= len) {
            return None;
        }
        if len <= 1 {
            return Some(0);
        }
        let offset = fastrand::usize(1..len);
        return Some((current + offset) % len);
    }
    let next = current + 1;
    if next < len {
        Some(next)
    } else if looping == Loop::All {
        Some(0)
    } else {
        None
    }
}

/// What removing a row did, relative to the one playing.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveOutcome {
    EmptiedQueue,
    RemovedBeforeCurrent,
    RemovedCurrent,
    RemovedAfterCurrent,
}

/// Where `current` lands after `removed` was taken out of a queue now
/// `len_after` long (the caller has already removed the row and
/// bounds-checked it).
///
/// The removed-the-current-row case is policy, not arithmetic — the engine
/// clamps and restarts in place, the TUI stops — so the index returned for
/// it is only a safe place to stand (clamped into range), and the caller
/// decides what standing there means.
pub fn shift_current(
    len_after: usize,
    current: usize,
    removed: usize,
) -> (usize, RemoveOutcome) {
    if len_after == 0 {
        return (0, RemoveOutcome::EmptiedQueue);
    }
    if removed < current {
        return (current - 1, RemoveOutcome::RemovedBeforeCurrent);
    }
    if removed == current {
        return (current.min(len_after - 1), RemoveOutcome::RemovedCurrent);
    }
    (current, RemoveOutcome::RemovedAfterCurrent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_forward_ends_wraps_or_stays_by_loop_mode() {
        assert_eq!(pick_next(3, Some(1), false, Loop::Off, false, None), Some(2));
        assert_eq!(pick_next(3, Some(2), false, Loop::Off, false, None), None, "the end ends");
        assert_eq!(pick_next(3, Some(2), false, Loop::All, false, None), Some(0), "all wraps");
        assert_eq!(pick_next(3, Some(1), false, Loop::One, false, None), Some(1), "one repeats");
        assert_eq!(
            pick_next(3, Some(1), false, Loop::One, true, None),
            Some(2),
            "a manual skip is never trapped by loop-one"
        );
        assert_eq!(pick_next(0, None, false, Loop::All, false, None), None, "nothing to pick");
        assert_eq!(pick_next(3, None, false, Loop::Off, false, None), Some(0), "start fresh");
    }

    #[test]
    fn shuffle_moves_and_knows_when_the_pass_is_over_if_anyone_counts() {
        for _ in 0..20 {
            let picked = pick_next(5, Some(2), true, Loop::Off, false, None).unwrap();
            assert_ne!(picked, 2, "a shuffle pick is never the track just played");
            assert!(picked < 5);
        }
        assert_eq!(pick_next(1, Some(0), true, Loop::All, false, None), Some(0));

        // The counted pass: over at len starts. Uncounted: never over —
        // the engine's shuffle still plays forever (audit #9, deferred).
        assert_eq!(pick_next(3, Some(1), true, Loop::Off, false, Some(3)), None);
        assert!(pick_next(3, Some(1), true, Loop::Off, false, Some(2)).is_some());
        assert!(pick_next(3, Some(1), true, Loop::Off, false, None).is_some());
        // Counting binds Off alone; All keeps going, One keeps trapping.
        assert!(pick_next(3, Some(1), true, Loop::All, false, Some(9)).is_some());
        assert_eq!(pick_next(3, Some(1), true, Loop::One, false, Some(9)), Some(1));
    }

    #[test]
    fn removing_a_row_shifts_current_by_where_it_sat() {
        assert_eq!(shift_current(0, 0, 0), (0, RemoveOutcome::EmptiedQueue));
        assert_eq!(shift_current(3, 2, 0), (1, RemoveOutcome::RemovedBeforeCurrent));
        assert_eq!(shift_current(3, 1, 2), (1, RemoveOutcome::RemovedAfterCurrent));
        assert_eq!(shift_current(3, 1, 1), (1, RemoveOutcome::RemovedCurrent));
        // Removing the last row while on it: the safe place to stand is the
        // new end; whether to play it is the caller's policy.
        assert_eq!(shift_current(2, 2, 2), (1, RemoveOutcome::RemovedCurrent));
    }
}
