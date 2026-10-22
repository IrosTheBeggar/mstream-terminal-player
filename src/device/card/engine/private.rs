//! A private file written by 2.12.7's rule (docs/METADATA.md): the card's
//! settings, `state.json`, through `state.new`, and the partial downloads a
//! later run takes up, `resume.json` through `resume.new` (2.12.8) — a
//! leftover `.new` deleted first, the new file written whole with the next
//! `seq` and flushed, the old one deleted, then the rename. A cut at any
//! write leaves a card whose read (settle's, 2.12.7's recovery) takes the
//! file before or the file after, never a torn one. What `state.json` holds
//! is the caller's (B2's set-up, B1's settings), and `resume.json` the run's
//! own table of partials; the engine holds their form and their order.

use std::io::Write as _;

use super::super::volume::{self, Private, RESUME_VERSION, STATE_VERSION};
use super::settle::ms;
use super::{Session, Stop};

impl Session<'_> {
    /// `state.json` replaced by `bytes`: a valid file of this software's
    /// version whose `seq` is one more than the file it replaces (2.12.7),
    /// or refused before anything is written. The card is checked whole
    /// first: a private file is the card's own, never another's.
    pub(crate) fn write_state(&mut self, bytes: &[u8]) -> Result<(), Stop> {
        let new = volume::settle_private(Some(bytes), None, "state", STATE_VERSION);
        let next = volume::next_seq(self.settled.state.seq);
        if new.value.is_none() || new.seq != next {
            return Err(Stop::Refused(format!("state.json must be a valid file of seq {next}")));
        }
        self.bound.check(true)?;
        self.write_private("state", bytes)?;
        self.settled.state = Private { file: Some("state.json".into()), ..new };
        Ok(())
    }

    /// `resume.json` as the run's table of partials says now (2.12.8), its
    /// `seq` one more. Checked as a rename into `/music` is: the binding,
    /// the root, the serial when its last read is old.
    pub(super) fn write_resume(&mut self) -> Result<(), Stop> {
        let bytes = self.settled.parts.text();
        let new = volume::settle_private(Some(&bytes), None, "resume", RESUME_VERSION);
        if new.value.is_none() || new.seq != self.settled.parts.next_seq() {
            return Err(Stop::Refused("resume.json didn't come out a valid file".into()));
        }
        self.bound.check(false)?;
        self.write_private("resume", &bytes)?;
        self.settled.parts.written();
        Ok(())
    }

    /// 2.12.7's write of `X.json`, through `X.new`.
    fn write_private(&mut self, x: &str, bytes: &[u8]) -> Result<(), Stop> {
        let (json, temp) = (ms(&format!("{x}.json")), ms(&format!("{x}.new")));
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
        self.bound.alive()?.rename(&temp, &json).map_err(|e| self.bound.failed(e))
    }
}
