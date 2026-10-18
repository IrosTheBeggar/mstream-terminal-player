//! Where a file's bytes come from (the transfer plan's 2.4, 2.5): the
//! engine's [`Fetcher`](super::engine::Fetcher)s. [`local`] reads the
//! server's own library folders when this computer is the server's, and
//! hands everything else to the fetcher it wraps; the server over HTTP, a
//! federation peer and a local ffmpeg are the parts still to come (B1, B9).

pub(crate) mod local;
