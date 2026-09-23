//! Play reporting (docs/ux-contracts/play-reporting.md): one session per
//! song start, folded from the engine's own signals — position ticks,
//! pause and resume, the end of the stream, the user moving on — and posted
//! to the Stats API once the play is over, from an outbox that survives a
//! restart. The server decides whether a play counts.

use super::{App, Effect, Origin, Queued, Reach};
use crate::api::types::Track;
use crate::player::PlayerStatus;
use crate::tui::worker::{ApiCmd, ReportOutcome};

/// A forward step in position bigger than this, per status, is a seek.
pub const SEEK_JUMP_S: f64 = 3.0;
/// Reaching this close to a known end is a completion however the song
/// was left.
pub const END_SLACK_S: f64 = 2.0;
/// Under this much listened, a session is a mis-click and is never posted.
pub const MIN_POST_MS: u64 = 1000;
/// The outbox's cap (oldest out) and a batch's size (the server's MAX_BATCH).
pub const OUTBOX_CAP: usize = 500;
pub const BATCH_MAX: usize = 200;
/// The legacy route's threshold: a server without the Stats API counts a
/// play thirty seconds in.
pub const LEGACY_SCROBBLE_MS: u64 = 30_000;
/// How long a kept batch waits before the next try.
pub const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
pub const CLIENT_NAME: &str = "mstream-terminal-player";
pub const STATS_SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Completed,
    Skipped,
    Stopped,
}

/// What a peer play carries, from the row's metadata — the parent cannot
/// look a peer's track up itself (clause 7).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_file: Option<String>,
}

impl Snapshot {
    fn of(track: &Track) -> Snapshot {
        let m = &track.metadata;
        let text = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        Snapshot {
            title: text(&m.title),
            artist: text(&m.artist),
            album: text(&m.album),
            duration_ms: m.duration.filter(|d| *d > 0.0).map(|d| (d * 1000.0).round() as u64),
            hash: text(&m.hash),
            art_file: text(&m.album_art),
        }
    }
}

/// A finished play as the server takes it (`playSchema`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Play {
    pub id: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<i64>,
    pub started_at: u64,
    pub ended_at: u64,
    pub played_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    pub outcome: Outcome,
    pub source: String,
    pub session_id: String,
    pub pause_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Snapshot>,
}

/// One song start, folded as it plays (clauses 1–6).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub origin: Origin,
    pub file_path: String,
    pub source: String,
    pub session_id: String,
    pub started_at: u64,
    pub played_ms: u64,
    pub pause_count: u32,
    pub paused: bool,
    pub last_pos: Option<f64>,
    pub max_pos: f64,
    pub duration_ms: Option<u64>,
    /// The peer's track, when the row is a peer's.
    pub snapshot: Option<Snapshot>,
    /// The legacy scrobble went out (a server without the Stats API).
    pub scrobbled: bool,
    /// When the file last saw this session: recovery's end time.
    #[serde(default)]
    pub checkpoint_at: Option<u64>,
}

impl Session {
    /// A position report (seconds). Only a forward step small enough to be
    /// playback — not a seek — adds listened time, and only while playing.
    pub fn tick(&mut self, position: f64, playing: bool) {
        if !position.is_finite() || position < 0.0 {
            return;
        }
        if let Some(last) = self.last_pos
            && playing
            && !self.paused
        {
            let delta = position - last;
            if delta > 0.0 && delta <= SEEK_JUMP_S {
                self.played_ms += (delta * 1000.0).round() as u64;
            }
        }
        self.last_pos = Some(position);
        if position > self.max_pos {
            self.max_pos = position;
        }
    }

    pub fn pause(&mut self) {
        if !self.paused {
            self.paused = true;
            self.pause_count += 1;
        }
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    /// The length, once known; the first word stands.
    pub fn with_duration(&mut self, ms: u64) {
        if self.duration_ms.is_none() && ms > 0 {
            self.duration_ms = Some(ms);
        }
    }

    /// The outcome the play is stored with (clause 5): the caller's word
    /// stands, except that reaching the end of a known length is a
    /// completion however the song was left.
    pub fn classify(&self, requested: Outcome) -> Outcome {
        if requested == Outcome::Completed {
            return Outcome::Completed;
        }
        match self.duration_ms {
            Some(d) if self.max_pos * 1000.0 >= d as f64 - END_SLACK_S * 1000.0 => Outcome::Completed,
            _ => requested,
        }
    }

    /// The play as the server takes it, or `None` when too short to post
    /// (clause 6).
    pub fn finish(&self, requested: Outcome, ended_at: u64) -> Option<Play> {
        if self.played_ms < MIN_POST_MS {
            return None;
        }
        Some(Play {
            id: self.id.clone(),
            file_path: self.file_path.clone(),
            peer_id: self.origin.peer,
            started_at: self.started_at,
            ended_at: ended_at.max(self.started_at),
            played_ms: self.played_ms,
            duration_ms: self.duration_ms,
            outcome: self.classify(requested),
            source: self.source.clone(),
            session_id: self.session_id.clone(),
            pause_count: self.pause_count,
            track: self.snapshot.clone(),
        })
    }
}

/// A play waiting to be posted, with the row's origin it goes by.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Owed {
    pub origin: Origin,
    pub play: Play,
}

/// `stats.json` (clause 9): the owed plays and the checkpointed session,
/// versioned like the queue's file so another shape is ignored.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StatsSnapshot {
    pub version: u32,
    pub instance_id: String,
    pub outbox: Vec<Owed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflight: Option<Session>,
}

/// The App's reporting state.
#[derive(Debug)]
pub struct Stats {
    /// One per install (kept in the file); one per launch.
    pub instance_id: String,
    pub session_id: String,
    pub session: Option<Session>,
    pub outbox: Vec<Owed>,
    /// The batch in flight: its ids.
    pub in_flight: Option<Vec<String>>,
    /// A kept batch's next try (clause 8).
    pub retry_at: Option<crate::clock::Instant>,
    /// Something changed: try the outbox at the next tick.
    pub flush_wanted: bool,
    /// How the open session will be closed by the Play that follows — set
    /// by the end-of-track paths; the next Play otherwise means a skip.
    pub ending: Option<Outcome>,
    counter: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            instance_id: random_id(),
            session_id: random_id(),
            session: None,
            outbox: Vec::new(),
            in_flight: None,
            retry_at: None,
            flush_wanted: false,
            ending: None,
            counter: 0,
        }
    }
}

/// Sixteen hex digits from the standard library's randomly seeded hasher —
/// unique enough for an id the server keeps per user, without a crate.
fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let a = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let b = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{a:016x}{b:016x}")
}

impl App {
    /// A queue row starts (clause 1): the open session closes the way the
    /// last end-of-track path said — else the user moved on — and this
    /// row's opens.
    pub(crate) fn stats_begin(&mut self, item: &Queued) {
        let requested = self.stats.ending.take().unwrap_or(Outcome::Skipped);
        self.stats_end(requested);
        let source = if item.dj.is_some() {
            "autodj"
        } else if self.queue.shuffle {
            "shuffle"
        } else {
            "manual"
        };
        self.stats.counter += 1;
        let now = crate::clock::epoch_ms();
        let short = &self.stats.instance_id[..self.stats.instance_id.len().min(8)];
        self.stats.session = Some(Session {
            id: format!("{short}-{now:x}-{}", self.stats.counter),
            origin: item.origin.clone(),
            file_path: item.filepath.clone(),
            source: source.to_string(),
            session_id: self.stats.session_id.clone(),
            started_at: now,
            played_ms: 0,
            pause_count: 0,
            paused: false,
            last_pos: None,
            max_pos: 0.0,
            duration_ms: item.metadata.duration.filter(|d| *d > 0.0).map(|d| (d * 1000.0).round() as u64),
            snapshot: item.origin.peer.map(|_| Snapshot::of(&item.track)),
            scrobbled: false,
            checkpoint_at: None,
        });
    }

    /// The engine's status (clauses 2–4): listened time from the position,
    /// a pause from the paused edge, the length once known — and, for a
    /// server without the Stats API, the legacy scrobble at thirty seconds
    /// (clause 10).
    pub(crate) fn stats_tick(&mut self, status: &PlayerStatus) -> Vec<Effect> {
        let Some(session) = self.stats.session.as_mut() else { return Vec::new() };
        if status.paused {
            session.pause();
        } else {
            session.resume();
        }
        session.tick(status.position, status.playing);
        if status.duration > 0.0 {
            session.with_duration((status.duration * 1000.0).round() as u64);
        }
        let due = !session.scrobbled && session.played_ms >= LEGACY_SCROBBLE_MS;
        let (origin, filepath) = (session.origin.clone(), session.file_path.clone());
        if !due || self.capabilities.stats || self.session.peer.is_some() || !self.is_session_origin(&origin) {
            return Vec::new();
        }
        if let Some(session) = self.stats.session.as_mut() {
            session.scrobbled = true;
        }
        match self.reach(&Origin { server: origin.server, peer: None }) {
            Ok(reach) => vec![Effect::Api(ApiCmd::Scrobble { reach, filepath })],
            Err(_) => Vec::new(),
        }
    }

    /// The open session closes (clause 5) and, unless too short, its play
    /// joins the outbox; the next tick posts.
    pub(crate) fn stats_end(&mut self, requested: Outcome) {
        let Some(session) = self.stats.session.take() else { return };
        if let Some(play) = session.finish(requested, crate::clock::epoch_ms()) {
            self.stats_enqueue(Owed { origin: session.origin, play });
        }
    }

    fn stats_enqueue(&mut self, owed: Owed) {
        self.stats.outbox.retain(|o| o.play.id != owed.play.id);
        self.stats.outbox.push(owed);
        while self.stats.outbox.len() > OUTBOX_CAP {
            self.stats.outbox.remove(0);
        }
        self.stats.flush_wanted = true;
    }

    /// The tick's turn (clause 8): post what is owed once something changed
    /// or a kept batch's minute is up, one batch at a time.
    pub(crate) fn stats_flush_due(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        if self.stats.outbox.is_empty() || self.stats.in_flight.is_some() || !self.connected {
            return Vec::new();
        }
        let due = self.stats.flush_wanted || self.stats.retry_at.is_none_or(|at| now >= at);
        if !due {
            return Vec::new();
        }
        self.stats.flush_wanted = false;
        let effects = self.stats_flush();
        if effects.is_empty() {
            // Nothing could go out (no way to reach a server): the minute.
            self.stats.retry_at = Some(now + RETRY_AFTER);
        }
        effects
    }

    /// One batch for the first server that can be reached: the plays'
    /// own, a peer's parent (clauses 8, 11). A server the ping said has no
    /// Stats API cannot take them, and its plays leave the outbox.
    pub(crate) fn stats_flush(&mut self) -> Vec<Effect> {
        let mut passed: Vec<String> = Vec::new();
        loop {
            let Some(server) = self
                .stats
                .outbox
                .iter()
                .map(|o| o.origin.server.clone())
                .find(|s| !passed.iter().any(|p| crate::config::same_server(p, s)))
            else {
                return Vec::new();
            };
            let target = Origin { server: server.clone(), peer: None };
            if self.is_session_origin(&target) && !self.capabilities.stats {
                self.stats.outbox.retain(|o| !crate::config::same_server(&o.origin.server, &server));
                continue;
            }
            let reach = match self.reach(&target) {
                Ok(reach) => Reach { peer: None, ..reach },
                Err(_) => {
                    passed.push(server);
                    continue;
                }
            };
            let batch: Vec<Play> = self
                .stats
                .outbox
                .iter()
                .filter(|o| crate::config::same_server(&o.origin.server, &server))
                .take(BATCH_MAX)
                .map(|o| o.play.clone())
                .collect();
            let ids: Vec<String> = batch.iter().map(|p| p.id.clone()).collect();
            let body = serde_json::json!({
                "client": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION"), "instanceId": self.stats.instance_id },
                "plays": batch,
            });
            self.stats.in_flight = Some(ids.clone());
            return vec![Effect::Api(ApiCmd::ReportPlays { reach, body, ids })];
        }
    }

    /// The server's word on the batch in flight (clause 8).
    pub(crate) fn stats_reported(&mut self, ids: Vec<String>, outcome: ReportOutcome) {
        self.stats.in_flight = None;
        match outcome {
            ReportOutcome::Settled(settled) => {
                self.stats.outbox.retain(|o| !settled.contains(&o.play.id));
                self.stats.retry_at = None;
                self.stats.flush_wanted = true;
            }
            ReportOutcome::Dropped(_) => {
                self.stats.outbox.retain(|o| !ids.contains(&o.play.id));
                self.stats.retry_at = None;
                self.stats.flush_wanted = true;
            }
            ReportOutcome::Kept(_) => {
                self.stats.retry_at = Some(crate::clock::Instant::now() + RETRY_AFTER);
            }
        }
    }

    /// What `stats.json` keeps (clause 9), or `None` when there is nothing
    /// to keep and the file may go.
    pub fn stats_snapshot(&self) -> Option<StatsSnapshot> {
        if self.stats.outbox.is_empty() && self.stats.session.is_none() {
            return None;
        }
        let inflight = self.stats.session.clone().map(|mut s| {
            s.checkpoint_at = Some(crate::clock::epoch_ms());
            s
        });
        Some(StatsSnapshot {
            version: STATS_SNAPSHOT_VERSION,
            instance_id: self.stats.instance_id.clone(),
            outbox: self.stats.outbox.clone(),
            inflight,
        })
    }

    /// The file's contents at launch (clause 9): the owed plays, the
    /// install's id, and a session left behind as a `stopped` play,
    /// listened to its last checkpoint.
    pub fn restore_stats(&mut self, snapshot: StatsSnapshot) {
        if snapshot.version != STATS_SNAPSHOT_VERSION {
            return;
        }
        if !snapshot.instance_id.is_empty() {
            self.stats.instance_id = snapshot.instance_id;
        }
        for owed in snapshot.outbox {
            self.stats_enqueue(owed);
        }
        if let Some(session) = snapshot.inflight {
            let ended = session.checkpoint_at.unwrap_or_else(crate::clock::epoch_ms);
            if let Some(play) = session.finish(Outcome::Stopped, ended) {
                self.stats_enqueue(Owed { origin: session.origin, play });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            id: "s1".into(),
            origin: Origin { server: "http://host:3000".into(), peer: None },
            file_path: "a/b.mp3".into(),
            source: "manual".into(),
            session_id: "sess".into(),
            started_at: 1_000,
            played_ms: 0,
            pause_count: 0,
            paused: false,
            last_pos: None,
            max_pos: 0.0,
            duration_ms: Some(200_000),
            snapshot: None,
            scrobbled: false,
            checkpoint_at: None,
        }
    }

    #[test]
    fn listened_time_is_forward_playback_only() {
        let mut s = session();
        for p in [0.0, 1.0, 2.0, 3.0] {
            s.tick(p, true);
        }
        assert_eq!(s.played_ms, 3000);
        s.tick(60.0, true); // a seek: adds nothing, but the furthest point moves
        s.tick(61.0, true);
        assert_eq!(s.played_ms, 4000);
        assert_eq!(s.max_pos, 61.0);
        s.tick(10.0, true); // backwards: nothing
        s.tick(11.0, true);
        assert_eq!(s.played_ms, 5000);
        s.pause();
        s.pause();
        s.tick(12.0, true); // paused: nothing, one pause counted
        assert_eq!((s.played_ms, s.pause_count), (5000, 1));
        s.resume();
        s.tick(13.0, false); // not playing: nothing
        assert_eq!(s.played_ms, 5000);
    }

    #[test]
    fn the_outcome_is_the_callers_word_unless_the_end_was_reached() {
        let mut s = session();
        assert_eq!(s.classify(Outcome::Skipped), Outcome::Skipped);
        assert_eq!(s.classify(Outcome::Stopped), Outcome::Stopped);
        assert_eq!(s.classify(Outcome::Completed), Outcome::Completed);
        s.max_pos = 198.5; // within two seconds of 200 s
        assert_eq!(s.classify(Outcome::Skipped), Outcome::Completed, "a skip in the fade-out is not a skip");
        let mut unknown = session();
        unknown.duration_ms = None;
        unknown.max_pos = 500.0;
        assert_eq!(unknown.classify(Outcome::Skipped), Outcome::Skipped, "no length, no completion by position");
    }

    #[test]
    fn too_short_is_never_a_play_and_a_play_carries_the_fold() {
        let mut s = session();
        s.played_ms = 999;
        assert!(s.finish(Outcome::Skipped, 5_000).is_none());
        s.played_ms = 1000;
        s.pause_count = 2;
        let play = s.finish(Outcome::Skipped, 5_000).expect("a second listened is a play");
        assert_eq!((play.played_ms, play.pause_count, play.outcome), (1000, 2, Outcome::Skipped));
        assert_eq!((play.started_at, play.ended_at), (1_000, 5_000));
        assert_eq!(play.duration_ms, Some(200_000));
        let json = serde_json::to_value(&play).unwrap();
        assert_eq!(json["filePath"], "a/b.mp3");
        assert_eq!(json["outcome"], "skipped");
        assert!(json.get("peerId").is_none() && json.get("track").is_none(), "a local play carries no peer fields: {json}");
        let early = s.finish(Outcome::Stopped, 10).unwrap();
        assert_eq!(early.ended_at, 1_000, "an end before the start is the start");
    }
}
