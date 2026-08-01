//! One process-lifetime tokio runtime, shared by the HTTP streaming source
//! (engine::http) and the mStream API client.
//!
//! It lives in a static so download tasks can never outlive it, and it is only
//! built on first use — pure-local serve mode (the jukebox) never starts tokio
//! at all. Everything in this crate calls into async code from ordinary sync
//! threads, so `block_on` is always safe here; it would panic only if called
//! from *inside* the runtime, which nothing does.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

fn runtime() -> Result<&'static Runtime, String> {
    // An init failure (no threads available) is unrecoverable, so caching the
    // error rather than retrying per call is fine.
    static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("mstream-io")
                .enable_all()
                .build()
                .map_err(|e| format!("failed to start async runtime: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// Run a future to completion on the shared runtime.
pub(crate) fn block_on<F: Future>(fut: F) -> Result<F::Output, String> {
    Ok(runtime()?.block_on(fut))
}
