//! Where a file's bytes come from (the transfer plan's 2.4, 2.5): the
//! engine's [`Fetcher`]s. [`http`] is the user's own server over the
//! network, its conversions held to their length ([`length`]); [`local`]
//! reads the server's own library folders when this computer is the
//! server's, and hands everything else to the fetcher it wraps; [`peer`] is
//! a federation peer's files under its daily budget, its conversions made
//! here by a local ffmpeg ([`ffmpeg`]). [`Sources`] gives each origin of a
//! run its own.

pub(crate) mod ffmpeg;
pub(crate) mod http;
pub(crate) mod length;
pub(crate) mod local;
pub(crate) mod peer;

use std::io::Write;

use super::engine::{Failed, Fetched, Fetcher, Have, Part, WantedFile};

/// A run's sources: the user's own server's, when the run reads it, and
/// each peer's, by the origin of the file asked for.
#[derive(Default)]
pub(crate) struct Sources {
    pub(crate) own: Option<http::Http>,
    pub(crate) peers: Vec<peer::Peer>,
}

impl Sources {
    /// The fetcher of `origin`'s files.
    fn of(&mut self, origin: usize) -> Option<&mut dyn Fetcher> {
        if let Some(own) = self.own.as_mut().filter(|h| h.origin() == origin) {
            return Some(own);
        }
        let peer = self.peers.iter_mut().find(|p| p.origin() == origin)?;
        Some(peer)
    }

    fn of_ref(&self, origin: usize) -> Option<&dyn Fetcher> {
        if let Some(own) = self.own.as_ref().filter(|h| h.origin() == origin) {
            return Some(own);
        }
        let peer = self.peers.iter().find(|p| p.origin() == origin)?;
        Some(peer)
    }
}

fn no_source() -> Failed {
    Failed { why: "a file of a server this run doesn't read".into(), stop: false }
}

impl Fetcher for Sources {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        self.of(file.origin).ok_or_else(no_source)?.fetch(file, into)
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        self.of(file.origin).ok_or_else(no_source)?.read_range(file, start, len)
    }

    fn resumable(&self, file: &WantedFile) -> bool {
        self.of_ref(file.origin).is_some_and(|f| f.resumable(file))
    }

    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        self.of(file.origin).ok_or_else(no_source)?.fetch_part(file, have, into)
    }

    fn waits(&self, origin: usize) -> Option<String> {
        self.of_ref(origin).and_then(|f| f.waits(origin))
    }
}
