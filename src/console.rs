//! Who may write to stderr.
//!
//! While the TUI owns the terminal, a stray line lands in raw mode on the
//! alternate screen: unscrolled, half-positioned, stamped across the UI
//! until the next full redraw. The engine's open failures and the tunnel's
//! routine teardowns both fire from background threads mid-session, so a
//! healthy tunneled evening slowly wallpapered the player with diagnostics
//! (audit #43, #44). The flag here is how those threads know whether the
//! terminal is currently a screen or a log.

use std::sync::atomic::{AtomicBool, Ordering};

static TUI_OWNS_TERMINAL: AtomicBool = AtomicBool::new(false);

/// The TUI is drawing; a stray stderr line would land on the picture.
pub fn claim_terminal() {
    TUI_OWNS_TERMINAL.store(true, Ordering::Relaxed);
}

/// The terminal is a terminal again.
pub fn release_terminal() {
    TUI_OWNS_TERMINAL.store(false, Ordering::Relaxed);
}

/// Whether a diagnostic line can print without defacing anything.
pub fn stderr_free() -> bool {
    !TUI_OWNS_TERMINAL.load(Ordering::Relaxed)
}

/// Like `eprintln!`, for diagnostics that can fire while the TUI owns the
/// terminal. Silent then: the only reader is the person the smear would
/// land on, and in the TUI the same news travels as an event instead.
/// The CLI and serve modes, which can print, still get their lines.
#[macro_export]
macro_rules! stderrln {
    ($($arg:tt)*) => {
        if $crate::console::stderr_free() {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_is_free_until_the_tui_claims_the_terminal() {
        // Process-global, so this test owns the whole story: free by
        // default (the CLI and serve modes never claim), silent while
        // claimed, free again once released.
        assert!(stderr_free(), "the CLI modes print as they always did");
        claim_terminal();
        assert!(!stderr_free(), "claimed: diagnostics hold their tongue");
        release_terminal();
        assert!(stderr_free(), "released: the shell is a log again");
    }
}
