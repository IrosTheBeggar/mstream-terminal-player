//! How a full-screen page's loop paces itself between frames.
//!
//! Every page waits on crossterm's poll, and that poll wakes for terminal
//! input and nothing else: an answer a worker sends mid-wait sits in its
//! channel until the wait runs out. At the ordinary pace that was most of
//! a poll for a reply the server gave in a millisecond — the performance
//! audit measured 101 ms from answer to screen in the player and 203 ms in
//! the GUI, whose loop also drew before it read its channel (#82). So
//! while an answer is expected the loop waits a millisecond, then as long
//! again as the answer has been out, up to [`BRISK`] at a time: an answer
//! from a server on the same machine is drawn about a millisecond after it
//! lands, and one from across a network within [`BRISK`].
//!
//! No thread reads the terminal on the loop's behalf, which would have
//! woken it on the answer itself: crossterm's reader is one global lock,
//! held for as long as a read blocks, and the startup probes (the OSC 11
//! ground lease, the pixel query) and a second page in the same process
//! (the admin sign-in, then its room) read the terminal themselves. A
//! brisk wait needs none of that, and costs nothing while nothing is
//! asked.

use std::time::{Duration, Instant};

/// The first wait once a request is out: long enough that an answer from
/// the same machine has usually landed by its end.
pub const BRISK_FIRST: Duration = Duration::from_millis(1);

/// The longest wait while a worker's answer is on its way: half a 60 Hz
/// frame at most between the answer landing and the loop drawing it.
pub const BRISK: Duration = Duration::from_millis(8);

/// How long a request keeps the loop brisk. Past it, an answer the network
/// is holding up — a tunnel dial, a native folder dialog left open, a slow
/// scan — costs a poll's delay again rather than a hundred wakeups a
/// second for as long as it takes.
pub const BRISK_FOR: Duration = Duration::from_secs(1);

/// When the loop last asked a worker something whose answer the screen
/// waits on — the clock behind [`BRISK`].
#[derive(Debug, Default, Clone, Copy)]
pub struct Expecting(Option<Instant>);

impl Expecting {
    /// A request just went out: brisk from now.
    pub fn arm(&mut self) {
        self.0 = Some(Instant::now());
    }

    /// For a loop whose worker says when it is busy (one call in flight at
    /// a time — the wizard's, an admin room's): brisk from the moment a
    /// call starts, ordinary again the moment it is answered. Asked after
    /// the answers are folded in and again after the next call goes out,
    /// so a call that follows its predecessor's answer restarts the clock.
    pub fn track(&mut self, busy: bool) {
        match (busy, self.0) {
            (true, None) => self.arm(),
            (false, Some(_)) => self.0 = None,
            _ => {}
        }
    }

    /// How long the answer has been out, while it is still expected soon
    /// enough to wait briskly for.
    fn out_for(&self) -> Option<Duration> {
        self.0.map(|at| at.elapsed()).filter(|out| *out < BRISK_FOR)
    }

    /// The loop's next wait: `normal`, cut while an answer is expected to
    /// as long as it has been out — the checks double, from [`BRISK_FIRST`]
    /// to [`BRISK`], so a quick answer is seen about as soon as it lands
    /// and a slow one costs a few wakeups, not a hundred.
    pub fn wait(&self, normal: Duration) -> Duration {
        match self.out_for() {
            Some(out) => normal.min(out.clamp(BRISK_FIRST, BRISK)),
            None => normal,
        }
    }

    /// Backdate the clock, so a test can step past [`BRISK_FOR`].
    #[cfg(test)]
    pub fn backdate(&mut self, by: Duration) {
        self.0 = self.0.and_then(|at| at.checked_sub(by));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLL: Duration = Duration::from_millis(100);

    fn brisk(expecting: &Expecting) -> bool {
        expecting.wait(POLL) < POLL
    }

    #[test]
    fn nothing_asked_waits_the_ordinary_poll() {
        let expecting = Expecting::default();
        assert_eq!(expecting.wait(POLL), POLL);
    }

    #[test]
    fn a_request_waits_briskly_until_its_window_closes() {
        let mut expecting = Expecting::default();
        expecting.arm();
        let first = expecting.wait(POLL);
        assert!(first >= BRISK_FIRST && first < BRISK, "just asked: the first check comes soon ({first:?})");
        expecting.backdate(Duration::from_millis(3));
        let third = expecting.wait(POLL);
        assert!(third >= Duration::from_millis(3) && third < BRISK, "then as long again as it has been out ({third:?})");
        expecting.backdate(Duration::from_millis(50));
        assert_eq!(expecting.wait(POLL), BRISK, "and never longer than BRISK while it is expected");
        // A shorter wait of the loop's own (a caret flip due sooner) wins.
        assert_eq!(expecting.wait(Duration::from_millis(3)), Duration::from_millis(3));
        expecting.backdate(BRISK_FOR);
        assert_eq!(expecting.wait(POLL), POLL, "a request the network holds up costs a poll again");
        expecting.arm();
        assert!(expecting.wait(POLL) < BRISK, "the next request re-arms it");
    }

    #[test]
    fn a_tracked_worker_is_brisk_from_its_call_to_its_answer() {
        let mut expecting = Expecting::default();
        expecting.track(false);
        assert!(!brisk(&expecting));
        expecting.track(true);
        assert!(brisk(&expecting), "a call went out");
        expecting.backdate(Duration::from_millis(600));
        expecting.track(true);
        expecting.backdate(Duration::from_millis(600));
        assert!(!brisk(&expecting), "still the same call: its clock kept running");
        expecting.track(false);
        expecting.track(true);
        assert!(brisk(&expecting), "answered, and the next call restarted it");
        expecting.track(false);
        assert!(!brisk(&expecting), "answered: back to the ordinary pace at once");
    }
}
