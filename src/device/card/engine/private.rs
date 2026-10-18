//! A private file written by 2.12.7's rule (docs/METADATA.md): the card's
//! settings, `state.json`, through `state.new` — a leftover `.new` deleted
//! first, the new file written whole with the next `seq` and flushed, the old
//! one deleted, then the rename. A cut at any write leaves a card whose read
//! (settle's, 2.12.7's recovery) takes the file before or the file after,
//! never a torn one. What the file holds is the caller's (B2's set-up, B1's
//! settings); the engine holds only its form and its order.

use std::io::Write as _;

use super::super::volume::{self, Private, STATE_VERSION};
use super::settle::ms;
use super::{Session, Stop};

impl Session<'_> {
    /// `state.json` replaced by `bytes`: a valid file of this software's
    /// version whose `seq` is one more than the file it replaces (2.12.7),
    /// or refused before anything is written. The card is checked whole
    /// first: a private file is the card's own, never another's.
    pub(crate) fn write_state(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        let new = volume::settle_private(Some(bytes), None, "state", STATE_VERSION);
        let next = self.settled.state.seq + 1;
        if new.value.is_none() || new.seq != next {
            return Err(Stop::Refused(format!("state.json must be a valid file of seq {next}")));
        }
        self.bound.check(true)?;
        let (json, temp) = (ms("state.json"), ms("state.new"));
        let card = self.bound.alive()?;
        if card.stat(&temp).map_err(|e| self.bound.failed(e))?.is_some() {
            self.bound.alive()?.delete(&temp).map_err(|e| self.bound.failed(e))?;
        }
        let mut file = self.bound.alive()?.create(&temp).map_err(|e| self.bound.failed(e))?;
        let wrote = file.write_all(bytes).and_then(|()| file.sync());
        drop(file);
        wrote.map_err(|e| self.bound.failed(e))?;
        let card = self.bound.alive()?;
        if card.stat(&json).map_err(|e| self.bound.failed(e))?.is_some() {
            self.bound.alive()?.delete(&json).map_err(|e| self.bound.failed(e))?;
        }
        self.bound.alive()?.rename(&temp, &json).map_err(|e| self.bound.failed(e))?;
        self.settled.state = Private { file: Some("state.json".into()), ..new };
        Ok(())
    }
}
