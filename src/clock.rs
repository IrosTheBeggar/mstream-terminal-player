//! `Instant`, spelled once for both targets.
//!
//! `std::time::Instant::now()` panics on wasm32-unknown-unknown — the target
//! has no clock — so anything the browser build draws on a timer reads its
//! time from here instead. web-time is the same API over
//! `performance.now()`; on native this is std's Instant, untouched.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

#[cfg(target_arch = "wasm32")]
pub use web_time::Instant;

/// Milliseconds since the Unix epoch — wall-clock time for what is told
/// to a server (a play's start and end). Zero if the clock is before 1970.
pub fn epoch_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    #[cfg(target_arch = "wasm32")]
    let since = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH);
    since.map(|d| d.as_millis() as u64).unwrap_or(0)
}
