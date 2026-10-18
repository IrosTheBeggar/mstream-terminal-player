//! Where a file's bytes come from (the transfer plan's 2.4, 2.5): the
//! engine's [`Fetcher`](super::engine::Fetcher)s. [`http`] is the user's own
//! server over the network, its conversions held to their length
//! ([`length`]); [`local`] reads the server's own library folders when this
//! computer is the server's, and hands everything else to the fetcher it
//! wraps. A federation peer and a local ffmpeg are the parts still to come
//! (B9).

pub(crate) mod http;
pub(crate) mod length;
pub(crate) mod local;
