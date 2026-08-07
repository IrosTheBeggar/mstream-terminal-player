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
