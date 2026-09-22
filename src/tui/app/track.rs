//! Track actions (docs/ux-contracts/track-actions.md), the shared half: the
//! rating with its optimistic write and latest-wins revert (clauses 10–11),
//! add to playlist (12–14), and the full block a sheet or Song info asks for
//! (8, 15). Shell-agnostic — the GUI's sheet drives these today; the TUI
//! shares the plumbing and can bind keys to it later.

use super::*;
use crate::api::types::TrackMetadata;
use crate::tui::worker::dj_log;
use rust_i18n::t;

/// A rating write that is out, with what it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatingWrite {
    pub filepath: String,
    pub previous: Option<u32>,
    pub seq: u64,
}

impl App {
    /// Whether a track's server takes the user's verbs at all: a peer's
    /// route is off the federation allowlist, and its rating or playlist
    /// would be someone else's (clauses 3 and 7).
    pub(crate) fn track_is_own(&self, origin: &Origin) -> bool {
        origin.peer.is_none()
    }

    /// Every copy of a track the shells draw — the queue's rows, the panes'
    /// rows shown or filtered, the playing track, the sheet's block — patched
    /// at once (clause 11).
    fn patch_rating(&mut self, filepath: &str, rating: Option<u32>) {
        for item in &mut self.queue.items {
            if item.filepath == filepath {
                item.track.metadata.rating = rating;
            }
        }
        for pane in [&mut self.files, &mut self.library, &mut self.search, &mut self.discover] {
            pane.for_each_track_mut(|track| {
                if track.filepath == filepath {
                    track.metadata.rating = rating;
                }
            });
        }
        if let Some(now) = &mut self.now_playing
            && now.filepath == filepath
        {
            now.metadata.rating = rating;
        }
        if let Some(info) = &mut self.track_info
            && info.filepath == filepath
        {
            info.metadata.rating = rating;
        }
    }

    /// The rating a track wears right now, from whichever copy is nearest.
    pub(crate) fn rating_of(&self, filepath: &str) -> Option<u32> {
        if let Some(info) = &self.track_info
            && info.filepath == filepath
        {
            return info.metadata.rating;
        }
        if let Some(item) = self.queue.items.iter().find(|i| i.filepath == filepath) {
            return item.metadata.rating;
        }
        for pane in [&self.files, &self.library, &self.search, &self.discover] {
            for entry in &pane.entries {
                if let Entry::Track { track, .. } = entry
                    && track.filepath == filepath
                {
                    return track.metadata.rating;
                }
            }
        }
        self.now_playing.as_ref().filter(|t| t.filepath == filepath).and_then(|t| t.metadata.rating)
    }

    /// Rate a track (clauses 10–11): every copy changes at once, the write
    /// goes to the track's own server, and a refusal comes back with the
    /// write's number so only the latest one may revert.
    pub(crate) fn rate_track(&mut self, origin: &Origin, filepath: &str, rating: Option<u32>) -> Vec<Effect> {
        if !self.track_is_own(origin) {
            return Vec::new();
        }
        let rating = rating.map(|r| r.min(10));
        let previous = self.rating_of(filepath);
        self.rating_seq += 1;
        let seq = self.rating_seq;
        self.rating_writes.push(RatingWrite { filepath: filepath.to_string(), previous, seq });
        self.patch_rating(filepath, rating);
        let reach = self.row_reach(origin);
        vec![Effect::Api(ApiCmd::RateSong { filepath: filepath.to_string(), rating, seq, reach })]
    }

    /// The server's answer to a rating write: a refusal puts the previous
    /// value back and says so — unless a newer write for the track has gone
    /// out since, in which case the newer one stands (the record's
    /// latest-tap-wins).
    pub(super) fn consume_rated(
        &mut self,
        filepath: String,
        _rating: Option<u32>,
        seq: u64,
        error: Option<String>,
    ) -> Vec<Effect> {
        let Some(at) = self.rating_writes.iter().position(|w| w.seq == seq) else {
            return Vec::new();
        };
        let write = self.rating_writes.remove(at);
        if let Some(why) = error {
            let newer = self.rating_writes.iter().any(|w| w.filepath == filepath && w.seq > seq);
            if !newer {
                self.patch_rating(&filepath, write.previous);
            }
            dj_log(format!("[rate] {filepath}: {why}"));
            self.error(t!("act.rating_failed"));
        }
        Vec::new()
    }

    /// Add a track to a playlist on its own server (clause 12); the server
    /// creates a playlist it does not know.
    pub(crate) fn add_to_playlist(&mut self, origin: &Origin, filepath: &str, playlist: &str) -> Vec<Effect> {
        if !self.track_is_own(origin) || playlist.trim().is_empty() {
            return Vec::new();
        }
        let reach = self.row_reach(origin);
        vec![Effect::Api(ApiCmd::AddToPlaylist {
            playlist: playlist.trim().to_string(),
            song: filepath.to_string(),
            reach,
        })]
    }

    /// The add landed, or did not (clauses 13–14): one sentence either way,
    /// and an open Playlists view learns of a new name.
    pub(super) fn consume_added_to_playlist(&mut self, playlist: String, error: Option<String>) -> Vec<Effect> {
        match error {
            None => {
                self.info(t!("act.added_to", name = playlist));
                self.consume_playlist_changed()
            }
            Some(why) => {
                self.error(t!("act.add_failed_why", why = why));
                Vec::new()
            }
        }
    }

    /// Ask a track's server for its full block (clause 8).
    pub(crate) fn fetch_track_info(&mut self, origin: &Origin, filepath: &str) -> Vec<Effect> {
        let reach = self.row_reach(origin);
        vec![Effect::Api(ApiCmd::TrackInfo { filepath: filepath.to_string(), reach })]
    }

    /// The block arrived: kept for the sheet, and its facts patched into the
    /// listing's copies where they were thinner.
    pub(super) fn consume_track_info(&mut self, filepath: String, track: Option<Track>) {
        let Some(track) = track else { return };
        if track.filepath != filepath {
            return;
        }
        let block = track.metadata.clone();
        let fill = |m: &mut TrackMetadata| {
            if m.title.is_none() {
                m.title = block.title.clone();
            }
            if m.artist.is_none() {
                m.artist = block.artist.clone();
            }
            if m.album.is_none() {
                m.album = block.album.clone();
            }
            if m.duration.is_none() {
                m.duration = block.duration;
            }
            if m.rating.is_none() {
                m.rating = block.rating;
            }
            if m.bpm.is_none() {
                m.bpm = block.bpm;
            }
            if m.musical_key.is_none() {
                m.musical_key = block.musical_key.clone();
            }
        };
        for item in &mut self.queue.items {
            if item.filepath == filepath {
                fill(&mut item.track.metadata);
            }
        }
        for pane in [&mut self.files, &mut self.library, &mut self.search, &mut self.discover] {
            pane.for_each_track_mut(|t| {
                if t.filepath == filepath {
                    fill(&mut t.metadata);
                }
            });
        }
        self.track_info = Some(track);
    }

    /// Ask a track's server for its playlist names (clause 12).
    pub(crate) fn fetch_playlist_names(&mut self, origin: &Origin) -> Vec<Effect> {
        self.playlist_names = None;
        let reach = self.row_reach(origin);
        vec![Effect::Api(ApiCmd::PlaylistNames { reach })]
    }
}
