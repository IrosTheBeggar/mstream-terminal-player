//! One process-lifetime tokio runtime, shared by the HTTP streaming source
//! (engine::http) and the mStream API client — and the TLS provider the
//! same two build their clients on.
//!
//! It lives in a static so download tasks can never outlive it, and it is only
//! built on first use — pure-local serve mode (the jukebox) never starts tokio
//! at all. Everything in this crate calls into async code from ordinary sync
//! threads, so `block_on` is always safe here; it would panic only if called
//! from *inside* the runtime, which nothing does.

use std::future::Future;
use std::sync::{Once, OnceLock};

use tokio::runtime::Runtime;

/// Make ring the process's rustls provider, once. reqwest is built with a
/// provider-less rustls (Cargo.toml: aws-lc stays out of the build,
/// performance audit #132) and panics at `build()` if none is installed,
/// so every client this crate builds calls this first — main() too, before
/// anything can dial, and the unit tests reach it through the same builders.
/// iroh hands its own ring configs to everything it builds; this is the
/// default for our clients alone.
pub(crate) fn install_tls_provider() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        // An Err is a provider someone installed first; theirs stands.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The shared runtime itself, for callers that hand it to code which
/// spawns on it — the tunnel client's in-place operations take a `&Runtime`.
pub(crate) fn handle() -> Result<&'static Runtime, String> {
    runtime()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_client_builds_on_ring() {
        install_tls_provider();
        install_tls_provider(); // a second call is a no-op, not a panic
        let installed = rustls::crypto::CryptoProvider::get_default().expect("a provider");
        let ring = rustls::crypto::ring::default_provider();
        let names = |provider: &rustls::crypto::CryptoProvider| {
            provider.kx_groups.iter().map(|group| group.name()).collect::<Vec<_>>()
        };
        assert_eq!(names(installed), names(&ring));
        // And reqwest, built without a provider of its own, takes it: this
        // panics at the build when nothing is installed.
        reqwest::Client::builder().build().expect("a client on the installed provider");
    }
}
