//! Keep a copy in the App: the gate, the jobs a copy runs as and the marks
//! they leave — on the Discover room's Connected servers rows and tiles,
//! and on a connected server's rooms — the Keep a copy flow, the Copies
//! list and Open elsewhere (docs/ux-contracts/discovery-page.md, clauses
//! 100–149; the keep-a-copy-from-browsing spec, `design-term-fed`).
//!
//! Nothing here draws. The GUI reads [`KeepState`] through the `keep_*`,
//! `copies_*` and `links_*` methods and calls them for every key and click;
//! the room's numbers (counts, percentages, steps) reach it only through
//! the Keep flow's view and the Copies list — the room itself is handed
//! words' worth of state: a [`JobMark`] per row and tile and a
//! [`JobsIndicator`] (clause 113, the 2026-10-19 log: no counts on the
//! page).
//!
//! The state belongs to the user's OWN server it was filled for — one
//! [`KeepState`] per own server, kept across moves onto its connected
//! servers and between them (spec §3.2): `App::keep` is the one in front,
//! the others wait parked ([`KeepShelf`]) until their server is in front
//! again. Every plug-in command goes to that server's reach and comes back
//! stamped with its state's [`KeepState::epoch`], so an answer lands in the
//! state it was asked for, whichever server is browsed by then.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::taste::TastePeerAlbum;
use super::*;
use crate::api::PluginFailure;
use crate::api::types::{
    Album, AlbumCredit, ArtistAlbumRow, COPY_PLUGIN, CancelAnswer, Capabilities, CollectionDestination,
    CollectionDestinationView, DiscoveryPluginsResponse, FolderSong, JobAction, JobLanding, JobLookup, JobMark,
    JobOutcome, JobParams, JobScope, JobStarted, JobState, Landed, Landing, LayoutTags, LeftOut, PluginJob,
    PluginJobList, PluginLink, Run, SongTag, TasteLead, artist_ownership, credit_of_rows, credit_of_songs, into_landing,
    job_for_album, mirror_landing, missing_albums, name_key, natural_order, normalize_peer_path, plan_artist_albums, plan_folder,
    sanitize_segment, server_folder, various_artists,
};
use crate::tui::worker::{KeepFacts, KeepFactsAsk, KeepSong};

/// The jobs poll's beat while a Keep or Copies modal is open and a copy
/// is live (clause 112; the webapp's 1.5 s).
pub const KEEP_POLL_MODAL: Duration = Duration::from_millis(1500);
/// …while the room shows a live mark or the indicator, and no modal.
pub const KEEP_POLL_ROOM: Duration = Duration::from_secs(4);
/// Failures back the beat off, doubling, to this.
pub const KEEP_POLL_MAX: Duration = Duration::from_secs(60);

/// What the GUI shows of the copies right now — the poll's beat follows
/// it (clause 112). Set by the GUI each frame, from whichever room shows
/// them; the TUI never sets it, so it never polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeepWatch {
    /// No copy's state on screen: no poll.
    #[default]
    Away,
    /// A room shows copies' marks or a page's state — the Discover room, a
    /// connected server's page — no Keep or Copies modal.
    Room,
    /// The Copies modal is open, or a Keep a copy modal shows a copy of
    /// its own moving (its start out, the job it follows live).
    Modal,
}

/// A job's plug-in and scope: a row holds one job per slot, the newest
/// (the lookup's "newest per plug-in and scope").
pub type JobSlot = (String, JobScope);

/// One connected-server row's jobs (the webapp's `jobBadges` row): the
/// keys its lookup answered and the newest job per slot under them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowJobs {
    pub keys: Vec<String>,
    pub held: BTreeMap<JobSlot, PluginJob>,
    /// A folder's row: its folder key alone, folder jobs alone — a folder
    /// kept as an album or an artist never marks it (spec §5.4). Every
    /// other row holds Discover's scopes and never a folder job.
    pub folders: bool,
}

impl RowJobs {
    /// A lookup's answer as a row's jobs: the first job per slot, as the
    /// server orders them (newest first) — copies only, Discover's scopes
    /// only (never a folder job, clause 104).
    pub fn from_lookup(found: &JobLookup) -> RowJobs {
        let mut row = RowJobs { keys: found.row_keys(), ..RowJobs::default() };
        row.hold(&found.jobs);
        row
    }

    /// A folder's lookup as its row's jobs: its folder key, and the folder
    /// copies under it.
    pub fn from_folder_lookup(found: &JobLookup) -> RowJobs {
        let mut row = RowJobs { keys: found.folder_key().into_iter().collect(), folders: true, ..RowJobs::default() };
        row.hold(&found.jobs);
        row
    }

    /// The scopes this row holds.
    fn holds(&self, scope: JobScope) -> bool {
        if self.folders { scope == JobScope::Folder } else { scope.discover() }
    }

    /// A lookup's jobs, the first per slot under this row's own keys.
    fn hold(&mut self, jobs: &[PluginJob]) {
        for job in jobs {
            let Some(scope) = job.scope().filter(|s| self.holds(*s)) else { continue };
            if job.plugin != COPY_PLUGIN || (self.folders && !job.key.as_ref().is_some_and(|k| self.keys.contains(k))) {
                continue;
            }
            self.held.entry((job.plugin.clone(), scope)).or_insert_with(|| job.clone());
        }
    }

    /// A job from the poll, a start or a cancel, merged by key: a job
    /// under one of this row's keys replaces the one its slot holds when it
    /// is as new or newer. True when it landed.
    pub fn merge(&mut self, job: &PluginJob) -> bool {
        let Some(key) = job.key.as_deref() else { return false };
        let Some(scope) = job.scope().filter(|s| self.holds(*s)) else { return false };
        if job.plugin != COPY_PLUGIN || !self.keys.iter().any(|k| k == key) {
            return false;
        }
        let slot = (job.plugin.clone(), scope);
        if self.held.get(&slot).is_some_and(|have| have.id > job.id) {
            return false;
        }
        self.held.insert(slot, job.clone());
        true
    }

    /// The slot's job.
    pub fn slot(&self, scope: JobScope) -> Option<&PluginJob> {
        self.held.get(&(COPY_PLUGIN.to_string(), scope))
    }
}

/// The best of the marks some jobs leave (clause 113): the lowest rank,
/// the newest job on a tie.
fn best_mark(jobs: impl Iterator<Item = PluginJob>) -> Option<JobMark> {
    jobs.filter_map(|job| job.outcome().mark().map(|mark| (mark, job.id)))
        .min_by(|a, b| a.0.rank().cmp(&b.0.rank()).then(b.1.cmp(&a.1)))
        .map(|(mark, _)| mark)
}

/// The mark a connected server's row wears on its LEAD (clause 113): its
/// song job, and the album and artist jobs of ITS server — another
/// server's album or artist copy never marks it (clause 121) — a finished
/// artist job speaking for the row's album only as its walk did
/// ([`job_for_album`], clause 115).
pub fn row_mark(jobs: &RowJobs, peer: i64, album: Option<&str>) -> Option<JobMark> {
    best_mark(
        jobs.held
            .values()
            .filter(|job| !job.other_peer(Some(peer)))
            .filter_map(|job| job_for_album(job, album)),
    )
}

/// The mark a connected server's tile wears on its artist row (clause
/// 114): the album and artist copies of THAT server found on any of its
/// rows — never a song job (one song is not the album), never another
/// server's copy — the newest per slot, each speaking for the tile's
/// album as its walk did.
pub fn tile_mark<'a>(rows: impl IntoIterator<Item = &'a RowJobs>, peer: i64, album: &str) -> Option<JobMark> {
    let mut held: BTreeMap<&JobSlot, &PluginJob> = BTreeMap::new();
    for row in rows {
        for (slot, job) in &row.held {
            if slot.1 == JobScope::Song || job.other_peer(Some(peer)) {
                continue;
            }
            if held.get(slot).is_none_or(|have| job.id > have.id) {
                held.insert(slot, job);
            }
        }
    }
    best_mark(held.values().filter_map(|job| job_for_album(job, Some(album))))
}

/// The best of some marks, each with its job's id: the lowest rank, the
/// newest job on a tie (clause 113).
fn best_of(marks: impl Iterator<Item = (JobMark, i64)>) -> Option<JobMark> {
    marks.min_by(|a, b| a.0.rank().cmp(&b.0.rank()).then(b.1.cmp(&a.1))).map(|(mark, _)| mark)
}

/// What a finished many-song copy did with one song, as its lists name
/// the song's file on its server (`from`): copied — in your collection;
/// skipped as owned, or a file in the way — already yours; failed. `None`
/// where its lists do not name it (they stop at 500, and a run stopped
/// before it never reached it).
fn song_in(job: &PluginJob, from: &str) -> Option<JobMark> {
    let songs = job.copy_result().songs?;
    let named = |entry: &serde_json::Value| entry.get("from").and_then(serde_json::Value::as_str) == Some(from);
    if songs.copied.iter().any(named) {
        return Some(JobMark::Kept);
    }
    if songs.skipped.iter().any(named) {
        return Some(JobMark::Yours);
    }
    songs.failed.iter().any(named).then_some(JobMark::Failed)
}

/// A song row's mark on a connected server's page (spec §5.3; the user's
/// 2026-10-27 call extends it to Recent, a genre's page, Search's song hits
/// and Files' file rows): its own song copy — the recording, from any
/// server — live or ended; and this server's album, artist and
/// only-missing copies of its album only once they have ended, read
/// through the song ([`song_in`]): copied, already yours, failed. Never a
/// wider copy while it runs — the page's own state says that one.
pub fn song_mark(jobs: &RowJobs, peer: i64, album: Option<&str>, filepath: &str) -> Option<JobMark> {
    let own = jobs.slot(JobScope::Song).and_then(|job| job.outcome().mark().map(|mark| (mark, job.id)));
    let wider = jobs
        .held
        .values()
        .filter(|job| job.scope().is_some_and(JobScope::wide) && !job.live() && !job.other_peer(Some(peer)))
        .filter_map(|job| job_for_album(job, album))
        .filter_map(|one| song_in(&one, filepath).map(|mark| (mark, one.id)));
    best_of(own.into_iter().chain(wider))
}

/// An album card's mark — and an album page's state in its header (spec
/// §5.1–5.2): this server's album copies, and its artist and only-missing
/// copies once they have ended, each as its walk saw the album
/// ([`job_for_album`]) — never a song copy, never another server's, and
/// nothing from an artist copy while it runs.
pub fn card_mark(jobs: &RowJobs, peer: i64, album: &str) -> Option<JobMark> {
    best_mark(
        jobs.held
            .values()
            .filter(|job| job.scope().is_some_and(JobScope::wide) && !job.other_peer(Some(peer)))
            .filter(|job| job.scope() == Some(JobScope::Album) || !job.live())
            .filter_map(|job| job_for_album(job, Some(album))),
    )
}

/// An artist's wall's state in its header (spec §5.1): the newest of this
/// server's every-album and only-missing copies of the artist.
pub fn artist_mark(jobs: &RowJobs, peer: i64) -> Option<JobMark> {
    [JobScope::Artist, JobScope::ArtistMissing]
        .into_iter()
        .filter_map(|scope| jobs.slot(scope).filter(|job| !job.other_peer(Some(peer))))
        .fold(None, |best: Option<&PluginJob>, job| match best {
            Some(b) if !newer(job, b) => Some(b),
            _ => Some(job),
        })
        .and_then(|job| job.outcome().mark())
}

/// A folder row's mark, and the open folder's state (spec §5.4): its
/// folder copy alone.
pub fn folder_mark(jobs: &RowJobs) -> Option<JobMark> {
    jobs.slot(JobScope::Folder).and_then(|job| job.outcome().mark())
}

/// `a` moved after `b`: its finish, else its last update (ms epochs) —
/// and where either carries neither, or both the same, the higher id (the
/// server's own order).
fn newer(a: &PluginJob, b: &PluginJob) -> bool {
    let moved = |j: &PluginJob| j.finished_at.or(j.updated_at);
    match (moved(a), moved(b)) {
        (Some(x), Some(y)) if x != y => x > y,
        _ => a.id > b.id,
    }
}

/// One song's line in a many-song copy's lists, as a song copy's result
/// reads it — found by the song's file on the server it came from (`from`,
/// federation-copy's `copySongs`): copied (the entry carries the landed
/// file), or skipped `owned` (`at`: where the library has it) or `exists`
/// (`at`: the file in the way). A failed song, or one the lists do not
/// name, is `None`.
fn song_account(job: &PluginJob, from: &str) -> Option<serde_json::Value> {
    let songs = job.copy_result().songs?;
    let named = |entry: &&serde_json::Value| entry.get("from").and_then(serde_json::Value::as_str) == Some(from);
    let peer = job.result.as_ref().and_then(|r| r.get("peer")).cloned().unwrap_or(serde_json::Value::Null);
    if let Some(copied) = songs.copied.iter().find(named) {
        return Some(serde_json::json!({ "copied": copied, "peer": peer }));
    }
    let skipped = songs.skipped.iter().find(named)?;
    let at = skipped.get("at").cloned().unwrap_or(serde_json::Value::Null);
    match skipped.get("why").and_then(serde_json::Value::as_str) {
        Some("owned") => Some(serde_json::json!({ "skipped": "owned", "existing": { "filepath": at }, "peer": peer })),
        Some("exists") => Some(serde_json::json!({ "skipped": "exists", "filepath": at, "peer": peer })),
        _ => None,
    }
}

/// A finished WIDER copy of the subject's server as it speaks for one
/// choice of the Keep modal — the marks' rule (clause 115) carried into the
/// modal (the user's 2026-10-20 report: an only-missing copy had copied the
/// album, and Album still offered Add album). `None` where it says nothing
/// for that choice; else the job to draw: the copy's id, the choice's scope
/// and recommendation, the choice's own account.
/// - **Album**: a done artist or only-missing copy through
///   [`job_for_album`] — the album's own account where the walk took it,
///   already yours where only-missing left it out — and only where that
///   view marks the row: a guest spot, an album never walked or a listing
///   that failed mid-walk say nothing. A failed or cancelled artist copy is
///   the whole run's state, never this album's: nothing.
/// - **This song**: a done album copy, or a done artist copy's view of the
///   row's album, whose lists name this song's file ([`song_account`]):
///   copied → in your collection, its landed file; skipped `owned` →
///   already yours; `exists` → a file in the way. The lists stop at 500,
///   so a song they do not name is never claimed. A failed or cancelled
///   run says nothing new here (the webapp's and the phone's rule too).
///
/// A live copy, another server's, another plug-in's and every other pair of
/// scopes: `None`.
pub fn covered_view(job: &PluginJob, subject: &KeepSubject, scope: JobScope) -> Option<PluginJob> {
    if job.live() || job.plugin != COPY_PLUGIN || job.other_peer(Some(subject.peer)) {
        return None;
    }
    let view = match (scope, job.scope()?) {
        (JobScope::Album, JobScope::Artist | JobScope::ArtistMissing) if job.state == JobState::Done => {
            let one = job_for_album(job, subject.album.as_deref())?;
            let params = JobParams { scope: Some(JobScope::Album.as_str().to_string()), ..one.params.clone() };
            PluginJob { params, ..one }
        }
        (JobScope::Song, JobScope::Album | JobScope::Artist | JobScope::ArtistMissing) if job.state == JobState::Done => {
            let from = subject.filepath.as_deref().filter(|p| !p.trim().is_empty())?;
            let one = job_for_album(job, subject.album.as_deref())?;
            let result = song_account(&one, from)?;
            PluginJob {
                params: JobParams { scope: None, ..one.params.clone() },
                state: JobState::Done,
                result: Some(result),
                error: None,
                progress: None,
                status_text: None,
                cancel_requested: false,
                ..one
            }
        }
        _ => return None,
    };
    let view = PluginJob { recommendation: subject.rec.clone(), ..view };
    view.outcome().mark().is_some().then_some(view)
}

/// What a Keep modal opening on `scope` follows (clauses 109, 115): its own
/// copy of that scope from this server — always while that is live — else
/// the newest finished wider copy of this server that speaks for the
/// choice ([`covered_view`]) unless its own is newer; `None` leaves the
/// choice. So a choice a wider copy already put in the library opens on
/// that, never on Add.
pub fn opening_phase(jobs: &RowJobs, subject: &KeepSubject, scope: JobScope) -> Option<KeepPhase> {
    let own = jobs.slot(scope).filter(|j| !j.other_peer(Some(subject.peer)));
    if let Some(own) = own.filter(|j| j.live()) {
        return Some(KeepPhase::Following(own.id));
    }
    let wider = jobs
        .held
        .values()
        .filter(|j| covered_view(j, subject, scope).is_some())
        .fold(None, |best: Option<&PluginJob>, j| match best {
            Some(b) if !newer(j, b) => Some(b),
            _ => Some(j),
        });
    match (own, wider) {
        (Some(own), Some(wider)) if newer(own, wider) => Some(KeepPhase::Following(own.id)),
        (_, Some(wider)) => Some(KeepPhase::Covered(wider.id)),
        (Some(own), None) => Some(KeepPhase::Following(own.id)),
        (None, None) => None,
    }
}

/// The verbs line's indicator (clause 116): words only, never a count —
/// shown while a copy is live or one failed, hidden otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobsIndicator {
    /// A copy is queued, running or stopping.
    pub live: bool,
    /// A copy failed (its state failed, or it copied nothing and something
    /// failed) and no newer job of it stands — the GOLD part.
    pub failed: bool,
}

/// The gate's two reads (clause 101).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeepGate {
    /// The listing, once it answered.
    pub plugins: Option<DiscoveryPluginsResponse>,
    /// The destination's view — read again with every Keep a copy modal,
    /// replaced by a save's answer (clauses 105, 144): its `destination`
    /// `None` is the server's word that this account may put files
    /// nowhere; `None` not asked or not answered.
    pub view: Option<CollectionDestinationView>,
    /// The ask whose answer `view` is — the gate's, a read, a save, a
    /// start's 409 — by its seq: an answer to an ask sent before it is
    /// older news and draws nothing (a save's answer is newer than any
    /// read sent before it; clauses 105, 144). Every answer lands on its
    /// own thread, in any order.
    view_seq: u64,
    /// The gate's ask, by its seq, once sent.
    asked: u64,
    /// The gate's ask is out.
    pub asking: bool,
    /// The last ask failed — every entry point stays hidden, and the next
    /// visit asks again.
    pub failed: Option<PluginFailure>,
}

/// The caller's copies as the Copies list and the indicator read them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobBook {
    /// Every copy job the last list answered, with each start, cancel and
    /// poll since folded in; by id.
    pub jobs: BTreeMap<i64, PluginJob>,
    /// The list has answered once.
    pub listed: bool,
    /// The last list ask failed; the poll backs off (clause 112).
    pub failed: Option<PluginFailure>,
    /// The list ask out.
    asking: Option<u64>,
    failures: u32,
    last_ask: Option<crate::clock::Instant>,
    /// Jobs seen live: one of them seen finished is news (the webapp's
    /// `finishedSince` — a page load is not).
    seen_live: BTreeSet<i64>,
    /// Cancels out.
    cancelling: BTreeSet<i64>,
    /// Starts out from the Copies list (Retry, Copy again, Start again):
    /// a refusal is said as a note there.
    starts: BTreeSet<u64>,
    /// Starts out from a Keep a copy window closed before they answered
    /// (Esc on `[ Adding… ]`): the status line says how each went
    /// (clause 118).
    unseen: BTreeSet<u64>,
}

/// A folder's folders as the picker asked them (clause 143).
#[derive(Debug, Clone, PartialEq)]
pub enum Folders {
    Asking,
    /// Their names, in the server's order.
    Listed(Vec<String>),
    Failed(PluginFailure),
}

/// The picker's asks (clauses 142–144) — "Where copies land", the GUI's:
/// the destination read again, the save, the library's folders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DestState {
    /// The read out: the last one wins.
    reading: Option<u64>,
    /// The last save sent, while it is out — a save sent after it (a
    /// picker opened again meanwhile) supersedes it.
    saving: Option<u64>,
    /// How the last save went, by its seq, for the GUI to take once: its
    /// view replaced the gate's, or the server's words — the picker that
    /// sent it says so, or the status line once that picker is gone.
    saved: Option<(u64, Result<(), PluginFailure>)>,
    /// Each folder's folders as listed, by the path asked
    /// (`/{library}/{folder}`), while the picker is open.
    folders: BTreeMap<String, Folders>,
    /// The listing out — one at a time — and the one wanted after it.
    listing: Option<(u64, String)>,
    wanted: Option<String>,
}

/// A thing the Copies modal or the room says once, then forgets.
#[derive(Debug, Clone, PartialEq)]
pub enum KeepNote {
    /// A cancel lost the race: the job had already finished (clause 133).
    TooLate { title: Option<String> },
    CancelFailed(PluginFailure),
    ClearFailed(PluginFailure),
    /// A Retry / Copy again / Start again from the Copies list was refused.
    StartFailed(PluginFailure),
}

/// What a Keep a copy flow (or Open elsewhere) is about: a Discover
/// room's connected-server row, or its tile on the strip (album focus) —
/// or, while a connected server is browsed, one of its things (spec §4):
/// a song (a browse row, a queue item, the playing card), an album (an
/// album page, a wall's card), an artist (an artist's wall), a folder
/// (Files). A browse target carries all it needs: nothing re-finds it.
/// The App builds them ([`App::keep_track`], [`App::keep_album_page`],
/// [`App::keep_album_card`], [`App::keep_artist_wall`],
/// [`App::keep_folder`]) — each only where its way in may stand.
#[derive(Debug, Clone, PartialEq)]
pub enum KeepTarget {
    Row { group: usize, row: usize },
    /// By its index in [`super::taste::TasteState::peer_albums`].
    Tile(usize),
    /// A connected server's song (`origin.peer` names it), with the credit
    /// of the album page it sits on, where one is open: what This song
    /// copies, and what Album joins with (spec §6.3).
    Track { origin: Origin, track: Box<Track>, album_artist: AlbumCredit },
    /// A connected server's album with a known credit — album focus.
    Album { origin: Origin, name: String, album_artist: String, year: Option<i32>, compilation: bool, art: Option<String> },
    /// An artist on a connected server — artist focus.
    Artist { origin: Origin, name: String },
    /// A folder below a library's top on a connected server — folder focus:
    /// its path there (its library first, no leading slash) and its name.
    Folder { origin: Origin, path: String, name: String },
}

/// A target names one thing whole — a song by its file — so equal is
/// equal: no length the server sends is ever NaN.
impl Eq for KeepTarget {}

impl KeepTarget {
    /// The connected server a browse target is from; `None` for the
    /// Discover room's own.
    pub fn origin(&self) -> Option<&Origin> {
        match self {
            KeepTarget::Row { .. } | KeepTarget::Tile(_) => None,
            KeepTarget::Track { origin, .. }
            | KeepTarget::Album { origin, .. }
            | KeepTarget::Artist { origin, .. }
            | KeepTarget::Folder { origin, .. } => Some(origin),
        }
    }

    /// What its window focuses on.
    pub fn focus(&self) -> KeepFocus {
        match self {
            KeepTarget::Row { .. } | KeepTarget::Track { .. } => KeepFocus::Song,
            KeepTarget::Tile(_) | KeepTarget::Album { .. } => KeepFocus::Album,
            KeepTarget::Artist { .. } => KeepFocus::Artist,
            KeepTarget::Folder { .. } => KeepFocus::Folder,
        }
    }
}

/// What a target named when a surface that stays open took it — the
/// track-actions sheet, the tile's list: the row's server and file, or
/// the tile's album key, with the server's name as it read then. A new
/// peers answer can land under such a surface (the old answer stands until
/// the new one does), and its indices then name other rows; the pin finds
/// what the surface shows wherever it now stands, or nothing, so Keep a
/// copy and Open elsewhere always act on the row the user acted on. A
/// browse target is its own pin ([`KeepPin::Item`]): it names its thing
/// whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepPin {
    Row { peer: i64, filepath: String, server: String },
    Tile { key: (i64, String, String), server: String },
    /// A browse target (Track, Album, Artist, Folder), and its server's
    /// name as it read when pinned.
    Item { target: Box<KeepTarget>, server: String },
}

impl KeepPin {
    /// The server the pinned row or album is from, as it read when pinned.
    pub fn server(&self) -> &str {
        match self {
            KeepPin::Row { server, .. } | KeepPin::Tile { server, .. } | KeepPin::Item { server, .. } => server,
        }
    }
}

/// What a Keep a copy window is about (spec §6): a song (This song first,
/// Album and Every album by joining), an album, an artist (the box alone),
/// a folder (its three landings).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeepFocus {
    #[default]
    Song,
    Album,
    Artist,
    Folder,
}

/// A folder on a connected server a folder copy takes: its path there, its
/// library first ("peerlib/Boukmanflow/Borrowed Tapes", no leading slash —
/// as its recommendation carries it), and its own name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderRef {
    pub path: String,
    pub name: String,
}

impl FolderRef {
    /// `path` as a folder Keep a copy can take — below a library's top
    /// (spec §4.1.3: a library and a folder at least); either separator, a
    /// slash at either end or none. `None` for a library's top and `/`.
    pub fn of(path: &str) -> Option<FolderRef> {
        let path = normalize_peer_path(path);
        let name = path.rsplit('/').next()?.to_string();
        (path.contains('/') && !name.is_empty()).then_some(FolderRef { path, name })
    }
}

/// A folder copy's landing as the window's choice (spec §6.5): the place
/// "Into a folder…" names is the flow's ([`IntoPlace`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderLanding {
    /// "As on {server}".
    Mirror,
    /// "Into a folder…".
    Into,
    /// "By your layout".
    Tags,
}

impl FolderLanding {
    /// A job's own landing, as a choice.
    pub fn of(landing: &JobLanding) -> FolderLanding {
        match landing {
            JobLanding::Mirror => FolderLanding::Mirror,
            JobLanding::Into { .. } => FolderLanding::Into,
            JobLanding::Tags => FolderLanding::Tags,
        }
    }
}

/// Where "Into a folder…" puts a folder: a library and a folder in it
/// (`""` its top). Chosen in the window's chooser, for this copy only —
/// nothing is saved; first, the saved library and a new folder named as
/// the pressed one, made safe (the webapp's `folderIntoDefault`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IntoPlace {
    pub vpath: String,
    pub path: String,
}

impl IntoPlace {
    /// As a start sends it.
    pub fn landing(&self) -> JobLanding {
        JobLanding::Into { vpath: Some(self.vpath.clone()).filter(|v| !v.is_empty()), path: self.path.clone() }
    }
}

/// The Keep modal's choice (clause 106): the song, its album, or every
/// album by its artist ("only the albums you don't have" is the artist
/// choice's checkbox) — or one of a folder's three landings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepChoice {
    Song,
    Album,
    Artist,
    Folder(FolderLanding),
}

impl KeepChoice {
    fn scope(self, only_missing: bool) -> JobScope {
        match self {
            KeepChoice::Song => JobScope::Song,
            KeepChoice::Album => JobScope::Album,
            KeepChoice::Artist if only_missing => JobScope::ArtistMissing,
            KeepChoice::Artist => JobScope::Artist,
            KeepChoice::Folder(_) => JobScope::Folder,
        }
    }
}

/// What a flow copies, frozen when it opens: an answer that lands while the
/// modal is up never changes what it copies — only what the window learns
/// of it (a song window's album credit, found among its facts).
#[derive(Debug, Clone, PartialEq)]
pub struct KeepSubject {
    /// The connected server's id, and its name.
    pub peer: i64,
    pub server: String,
    /// What the window is about (spec §6).
    pub focus: KeepFocus,
    /// The song's title — an artist focus's artist, a folder focus's folder
    /// name; `None` for album focus (clause 106).
    pub title: Option<String>,
    /// The song's artist — album focus: the album's credit unless it is a
    /// compilation; artist focus: the artist.
    pub track_artist: Option<String>,
    pub album: Option<String>,
    /// The album's credit where it is known: the row's or the tile's; a
    /// browse song's once its album page, its own tags or its server's
    /// albums say it (spec §6.3).
    pub album_artist: Option<String>,
    pub year: Option<i32>,
    pub duration: Option<f64>,
    /// The song's path on its server.
    pub filepath: Option<String>,
    /// The cover's file name on its server.
    pub art: Option<String>,
    /// A folder focus's folder.
    pub folder: Option<FolderRef>,
    /// Opened while a connected server is browsed (spec §4): Album needs
    /// the album's own credit — never the track's artist (rule 1) — where
    /// the Discover room's rows keep their own rule ([`Self::album_credit`]).
    pub browse: bool,
    /// The album's listing called it a compilation, or its credit is
    /// Various Artists.
    pub listed_compilation: bool,
    /// The album page it was opened from says its album's credits disagree
    /// (Split Series): no Album, and nothing the window finds makes one
    /// (rule 1) — one artist's share of the album is no album.
    pub credits_disagree: bool,
    /// What every job of the flow sends: the row's raw taste JSON exactly
    /// as it came — a tile's best row (clause 103); a browse target's own
    /// recommendation (spec §5.0).
    pub rec: serde_json::Value,
}

impl KeepSubject {
    fn of(peer: i64, server: &str, lead: &TasteLead, album_focus: bool) -> KeepSubject {
        KeepSubject {
            peer,
            server: server.to_string(),
            focus: if album_focus { KeepFocus::Album } else { KeepFocus::Song },
            title: if album_focus { None } else { Some(lead.title.clone().unwrap_or_default()) },
            track_artist: lead.artist.clone(),
            album: lead.album.clone(),
            album_artist: lead.album_artist.clone(),
            year: lead.year,
            duration: lead.duration,
            filepath: lead.filepath.clone(),
            art: lead.album_art.clone(),
            folder: None,
            browse: false,
            listed_compilation: false,
            credits_disagree: false,
            rec: lead.raw.clone(),
        }
    }

    pub fn album_focus(&self) -> bool {
        self.focus == KeepFocus::Album
    }

    /// The credit the album goes by — its album artist; on a Discover row
    /// or tile, else the track artist (the tile's artist row): what Album
    /// needs beside the album name (clause 104; spec rule 1 for browsing).
    pub fn album_credit(&self) -> Option<&str> {
        if self.credits_disagree {
            return None;
        }
        let fallback = if self.browse { None } else { self.track_artist.as_deref() };
        self.album_artist.as_deref().or(fallback).filter(|a| !a.trim().is_empty())
    }

    /// A browse song whose album's credit its window must find: its album's
    /// songs are asked by the album's name and year alone (spec §4.1.3 step
    /// 2 over the whole album) — never by the track artist, which narrows a
    /// split album or a compilation to that artist's share.
    pub fn whole_album(&self) -> bool {
        self.browse && self.focus == KeepFocus::Song && self.album_artist.is_none() && !self.credits_disagree
    }

    /// The album is credited to someone other than this song's artist (a
    /// compilation) — or its listing says it is one: "Every album by" would
    /// copy the track artist's albums, not this album's, so an album focus
    /// never offers it.
    pub fn compilation(&self) -> bool {
        if self.listed_compilation {
            return true;
        }
        match (self.album_artist.as_deref(), self.track_artist.as_deref()) {
            (Some(album), Some(track)) => {
                crate::api::types::name_key(Some(album)) != crate::api::types::name_key(Some(track))
            }
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// The song's format, by its file's extension ("mp3") — the Song
    /// choice's size beside its length.
    pub fn format(&self) -> Option<String> {
        let path = self.filepath.as_deref()?;
        let name = path.rsplit('/').next()?;
        let (_, ext) = name.rsplit_once('.')?;
        (!ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .then(|| ext.to_ascii_lowercase())
    }

    /// The recommendation a job of `scope` sends: the subject's own — and
    /// for an album copy of a browse song whose album's credit the window
    /// found, that credit as its `albumArtist`, so the copy lists and keys
    /// the album by it, never by the song's artist (spec rule 1).
    pub fn rec_for(&self, scope: JobScope) -> serde_json::Value {
        let mut rec = self.rec.clone();
        if scope == JobScope::Album
            && self.browse
            && rec.get("albumArtist").and_then(serde_json::Value::as_str).is_none_or(|a| a.trim().is_empty())
            && let (Some(credit), Some(fields)) = (self.album_artist.clone(), rec.as_object_mut())
        {
            fields.insert("albumArtist".into(), serde_json::Value::String(credit));
        }
        rec
    }
}

/// Which choices a subject offers under a listing when its window opens
/// (clauses 104, 106; spec §6): the song on a song's window only (album
/// focus has no "This song"); Album when the album and its credit are both
/// known; Every album by in album focus — the album is theirs, so the
/// artist has one at least (spec §6.2) — unless it is a compilation or
/// credited to Various Artists; an artist's window the artist alone (the
/// box is its only stop); a folder's its three landings — each only where
/// the copy plug-in runs that scope. A song's window never offers Every
/// album by at open: it joins once the connected server's albums of the
/// artist answer with one of theirs ([`joined_choices`], the user's
/// 2026-10-27 decision, Discover too).
pub fn keep_choices(listing: &DiscoveryPluginsResponse, subject: &KeepSubject) -> Vec<KeepChoice> {
    let named = |v: Option<&str>| v.is_some_and(|s| !s.trim().is_empty());
    match subject.focus {
        KeepFocus::Folder => {
            if subject.folder.is_none() || !listing.copies(JobScope::Folder) {
                return Vec::new();
            }
            [FolderLanding::Mirror, FolderLanding::Into, FolderLanding::Tags].into_iter().map(KeepChoice::Folder).collect()
        }
        KeepFocus::Artist => {
            let artist = subject.track_artist.as_deref();
            (named(artist) && !various_artists(artist) && listing.copies(JobScope::Artist))
                .then_some(KeepChoice::Artist)
                .into_iter()
                .collect()
        }
        KeepFocus::Song | KeepFocus::Album => {
            let mut out = Vec::new();
            if subject.focus == KeepFocus::Song && listing.copies(JobScope::Song) {
                out.push(KeepChoice::Song);
            }
            if named(subject.album.as_deref()) && subject.album_credit().is_some() && listing.copies(JobScope::Album) {
                out.push(KeepChoice::Album);
            }
            let artist = subject.track_artist.as_deref();
            if subject.album_focus()
                && named(artist)
                && !various_artists(artist)
                && !subject.compilation()
                && listing.copies(JobScope::Artist)
            {
                out.push(KeepChoice::Artist);
            }
            out
        }
    }
}

/// The choices a song's window gains once its facts answer (spec §6.3,
/// "choices append and never leave"): Album once the album's credit is
/// known; Every album by once the connected server's albums of the artist
/// answer with one of theirs at least — never for Various Artists. An
/// album's window has all of its own at open. In the order a window lists
/// them.
pub fn joined_choices(listing: &DiscoveryPluginsResponse, subject: &KeepSubject, facts: &KeepFacts) -> Vec<KeepChoice> {
    if subject.focus != KeepFocus::Song {
        return Vec::new();
    }
    let mut out = keep_choices(listing, subject);
    let artist = subject.track_artist.as_deref().filter(|a| !a.trim().is_empty());
    let theirs = match (artist, facts.peer_albums.as_deref()) {
        (Some(artist), Some(rows)) => plan_artist_albums(rows, artist).theirs.len(),
        _ => 0,
    };
    if theirs > 0 && !various_artists(artist) && listing.copies(JobScope::Artist) {
        out.push(KeepChoice::Artist);
    }
    out
}

/// Where a flow stands.
#[derive(Debug, Clone, PartialEq)]
pub enum KeepPhase {
    /// The choice: radios and the primary.
    Choose,
    /// The start is out; no button stands until it answers.
    Starting { ticket: u64 },
    /// The start was refused: the server's words above the buttons, the
    /// choice still editable (clause 108).
    Refused(PluginFailure),
    /// The start was refused 403: a right the account no longer has — one
    /// plain line and Close, never the choice again (clause 147).
    Off(OffWhy),
    /// Following this job to its outcome.
    Following(i64),
    /// A finished wider copy of this server speaks for the choice it
    /// opened on ([`covered_view`]): its outcome for that choice, with the
    /// choice's own buttons — Retry and Copy again start the choice's copy.
    Covered(i64),
}

/// The right a flow found gone (clause 147), as its one plain line says
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffWhy {
    /// Files may land nowhere for this account: a destination read or a
    /// 409 that names none, a start refused 403 in words that name
    /// uploading ("Uploading Disabled"), the upload right gone.
    Uploads,
    /// This account may not start copies: any other 403 at start, or a gate
    /// asked again that finds jobs not allowed.
    Jobs,
    /// The copy plug-in, or the chosen scope, is no longer listed — a gate
    /// asked again (after a 404 at start, or any other time) says so.
    Plugin,
}

/// A 403 at start, by the server's words: uploading named — ASCII, any
/// case — is the upload right, anything else the right to start jobs.
fn off_for_403(words: &str) -> OffWhy {
    if words.to_ascii_lowercase().contains("upload") { OffWhy::Uploads } else { OffWhy::Jobs }
}

/// Another server's live copy of the chosen album or artist (clause 121):
/// whose it is, and its cancel ("x cancel that copy").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherCopy {
    pub server: String,
    pub job_id: i64,
    /// Its cancel is out, or it is already stopping: nothing to press.
    pub cancelling: bool,
}

/// One open Keep a copy modal (clauses 106–110).
#[derive(Debug, Clone, PartialEq)]
pub struct KeepFlow {
    pub target: KeepTarget,
    pub subject: KeepSubject,
    pub choices: Vec<KeepChoice>,
    pub choice: KeepChoice,
    pub only_missing: bool,
    pub facts: KeepFacts,
    pub facts_asking: bool,
    facts_seq: u64,
    /// The album's songs asked again by the credit the window found (step
    /// 3), while that is out: Album's count waits for it.
    songs_seq: Option<u64>,
    /// Its own lookup, when its rows had none yet.
    lookup_seq: Option<u64>,
    /// Its subject's jobs, merged as the room's rows are.
    pub jobs: RowJobs,
    pub phase: KeepPhase,
    /// The place its last start sent ([`ApiCmd::JobStart`]'s): what LANDS
    /// IN reads while the start is out.
    sent: Option<Option<CollectionDestination>>,
    /// "Into a folder…"'s place for a folder copy (spec §6.6): the chooser
    /// sets it, nothing saves it.
    pub into: IntoPlace,
    /// "By your layout"'s tags for a folder's songs (spec §6.5.3): read
    /// once, the first time that landing is chosen, at most 100 songs —
    /// `None` until then, or with no answer.
    pub folder_tags: Option<Vec<KeepSong>>,
    /// That read: `Some(seq)` once sent (asked once, answered or not).
    tags_seq: Option<u64>,
    /// The folder copy's songs as the copy takes them ([`plan_folder`]),
    /// planned once its listing landed.
    folder_plan: Option<Vec<FolderSong>>,
    /// A song with no tags (a Files row, its metadata off or unknown to its
    /// server's database): its tags are read as the window opens (spec
    /// §6.3), and its subject learns them when they land.
    untagged: bool,
}

impl KeepFlow {
    /// A covered choice looks again whenever its jobs change: its own copy
    /// started since (live, or newer) takes over, and a wider copy that no
    /// longer speaks for it gives the choice back.
    fn resettle(&mut self, gate: &KeepGate) {
        if matches!(self.phase, KeepPhase::Covered(_)) {
            self.phase = opening_phase(&self.jobs, &self.subject, self.scope(gate)).unwrap_or(KeepPhase::Choose);
        }
    }

    /// "Only the albums you don't have" (clause 107), offered with its
    /// count: with the artist choice, where the plug-in runs it, the
    /// library holds some of the artist's albums and the other server one
    /// it lacks by name — or where such a copy still wants the user; never
    /// while the sizes are asked, a copy held or not. Kept by that copy
    /// alone, nothing missing by name (or the sizes unknown), its count is
    /// 0: the window says none.
    fn missing_box(&self, gate: &KeepGate) -> Option<usize> {
        let runs = gate.plugins.as_ref().is_some_and(|p| p.copies(JobScope::ArtistMissing))
            && self.choices.contains(&KeepChoice::Artist);
        let held = self.jobs.slot(JobScope::ArtistMissing).filter(|j| !j.other_peer(Some(self.subject.peer))).is_some_and(|j| {
            let o = j.outcome();
            o.live() || o.troubled()
        });
        let facts = &self.facts;
        let local = facts.local_albums.as_ref().map(Vec::len).unwrap_or(0);
        let missing = match (&facts.peer_albums, &facts.local_albums, self.subject.track_artist.as_deref()) {
            (Some(remote), Some(local), Some(artist)) => artist_ownership(artist, remote, local).1,
            _ => 0,
        };
        (runs && !self.facts_asking && ((local > 0 && missing > 0) || held)).then_some(missing)
    }

    /// The scope the chosen radio sends: Every album by takes the box's
    /// tick only while the box is offered.
    fn scope(&self, gate: &KeepGate) -> JobScope {
        self.choice.scope(self.only_missing && self.missing_box(gate).is_some())
    }

    /// The facts landed: a browse song learns its album's credit (spec
    /// §4.1.3 steps 2–3, inside the window) and the choices that joined
    /// are appended — never one removed, the dot never moved. A credit
    /// found by step 3 (the album's songs, asked by its name and year alone,
    /// did not say it) is returned: the album's songs are asked again by
    /// it, so Album counts and files what its copy takes.
    fn learn(&mut self, gate: &KeepGate) -> Option<String> {
        if let (Some(folder), Some(files)) = (self.subject.folder.as_ref(), self.facts.folder_files.as_deref()) {
            self.folder_plan = Some(plan_folder(files, &folder.path).songs);
        }
        let mut again = None;
        if self.subject.browse
            && self.subject.focus == KeepFocus::Song
            && self.subject.album_artist.is_none()
            && !self.subject.credits_disagree
            && let (AlbumCredit::Known(credit), said) = window_credit(&self.subject, &self.facts)
        {
            if !said {
                again = Some(credit.clone());
            }
            self.subject.album_artist = Some(credit);
        }
        if let Some(listing) = gate.plugins.as_ref() {
            for choice in joined_choices(listing, &self.subject, &self.facts) {
                if !self.choices.contains(&choice) {
                    self.choices.push(choice);
                }
            }
        }
        again
    }

    /// The folder copy's songs, as the copy takes them, once the connected
    /// server listed the folder.
    fn folder_songs(&self) -> Option<&[FolderSong]> {
        self.folder_plan.as_deref()
    }

}

/// A browse song's album credit as its window finds it (spec §4.1.3, the
/// album page's steps 2–3 on the window's own facts): the songs of its
/// album — asked by its name and year alone, never narrowed by the track
/// artist, whose share of a split album or a compilation would agree on
/// that artist — their own `album-artist` agreeing (step 2: `true`, the
/// songs said it); else the connected server's albums of its artist
/// naming one album of its name and year (step 3: `false`). Unknown until
/// then.
fn window_credit(subject: &KeepSubject, facts: &KeepFacts) -> (AlbumCredit, bool) {
    if let Some(songs) = facts.album_songs.as_deref()
        && let Some(said) = credit_of_songs(songs.iter().map(|song| song.album_artist.as_deref()))
    {
        return (said, true);
    }
    match (subject.album.as_deref(), facts.peer_albums.as_deref()) {
        (Some(album), Some(rows)) => (credit_of_rows(rows, album, subject.year).0, false),
        _ => (AlbumCredit::Unknown, false),
    }
}

/// A size the modal shows DIM beside a choice: "…" while it is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact<T> {
    Asking,
    Known(T),
    Unknown,
}

/// A choice's size (clause 106): numbers live in this modal.
#[derive(Debug, Clone, PartialEq)]
pub enum ChoiceSize {
    /// "0:08 · mp3".
    Song { seconds: Option<f64>, format: Option<String> },
    /// "3 songs".
    Album { songs: Fact<usize> },
    /// "1 album · you have 3": the other server's albums an artist copy
    /// takes — theirs, by the copy's own rule ([`plan_artist_albums`]: an
    /// album they only appear on, and the singles bucket, stay out; spec
    /// gap 26) — and how many the library holds.
    Artist { albums: Fact<usize>, have: Fact<usize> },
    /// "3 songs": what a folder copy takes, once the connected server
    /// listed the folder ([`plan_folder`]) — every landing alike.
    Folder { songs: Fact<usize> },
}

/// The primary button's verb, which follows the choice (clause 106): Add
/// song · Add album · Add artist · Add 1 album · Add folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepVerb {
    Song,
    Album,
    Artist,
    /// "Add 1 album" — only the ones you don't have; 0, no count to say
    /// ("Add the albums you don't have").
    Missing(usize),
    /// "Add folder · 3 songs" ([`ChoiceSize::Folder`]).
    Folder,
}

/// A folder past the plug-in's cap (spec §6.5.5): its songs and the cap —
/// before any start, the window's whole body, `[ Close ]` its primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderCap {
    pub count: usize,
    pub cap: u64,
}

/// Where a flow's view stands (clauses 107–110).
#[derive(Debug, Clone, PartialEq)]
pub enum KeepStage {
    /// The radios and the primary. `other` is the other server's live copy
    /// of the same album or artist that covers this one — the primary is
    /// hidden until it ends (clause 121), its cancel offered.
    Choose { verb: Option<KeepVerb>, other: Option<OtherCopy> },
    Starting,
    Refused { failure: PluginFailure, verb: Option<KeepVerb> },
    /// A job of this flow's scope: its outcome, and the action the window
    /// offers for it ([`window_action`]).
    Job { job: Box<PluginJob>, outcome: JobOutcome, action: Option<JobAction> },
    /// A right gone — a re-read or a 409 that names nowhere, a start
    /// refused 403, a gate asked again that no longer holds — under the
    /// choice, or under an ended copy whose Retry or Add again would start
    /// nothing now: one plain line and Close (clause 147).
    Off(OffWhy),
}

/// Everything the Keep modal draws, worked out (clauses 106–110).
#[derive(Debug, Clone, PartialEq)]
pub struct KeepView {
    pub subject: KeepSubject,
    pub choices: Vec<(KeepChoice, ChoiceSize)>,
    pub choice: KeepChoice,
    pub scope: JobScope,
    /// "Only the albums you don't have": `None` hidden — never grey — else
    /// whether it is ticked and how many albums it would add: 0 where only
    /// a copy of it that still wants the user keeps it offered, none
    /// missing by name or the sizes unknown — no count to say.
    pub missing: Option<(bool, usize)>,
    /// The one album "Only the albums you don't have" would copy, when it
    /// is one, and its year: where it lands names it.
    pub one_missing: Option<(String, Option<i32>)>,
    /// Where this account's copies land now (the gate's, as last read) —
    /// what a start sends and the SOURCE row's word (clause 105).
    pub destination: Option<CollectionDestination>,
    /// The album's songs on the connected server with the tags each files
    /// under, once the facts answer (`None` until then, or where the read
    /// failed): whether an album copy keeps to one folder (clause 105).
    pub album_songs: Option<Vec<KeepSong>>,
    /// The LANDS IN line: the account's place while choosing, the place a
    /// start sent while it is out, a copy's pinned place while it runs and
    /// the folder its files landed in once it ended — never the account's
    /// place now for a copy that stands (clause 105). `None` hides it.
    pub landing: Option<Landing>,
    /// Where Retry would land — the account's place now — where that is
    /// not the place the copy ran with (the webapp's `retryPath`, clause
    /// 145): drawn as "Retry adds to", so a press never lands somewhere the
    /// window did not show. `None` where it is the same — and a file in the
    /// way offers Retry only where this stands ([`window_action`]).
    pub again: Option<Landing>,
    pub stage: KeepStage,
    /// The server a song copy came from where it is not the subject's —
    /// a song is the recording wherever it came from (clause 115), so the
    /// window says whose ("Copied from Stage Peer C").
    pub copied_from: Option<String>,
    /// The wider copy's scope where its account speaks for this choice
    /// ([`KeepPhase::Covered`]): "with your copy of every album by …",
    /// "… of …'s missing albums", "With your album copy".
    pub borrowed: Option<JobScope>,
    /// "Leaves out …" (spec §6.4): what an artist copy will not take —
    /// shown while Every album by is the choice and something stays out.
    pub left_out: Option<LeftOut>,
    /// A folder past the plug-in's cap, before any start (spec §6.5.5): the
    /// stage is then the choice with no primary, the window draws this.
    pub cap: Option<FolderCap>,
    /// "Into a folder…"'s place (spec §6.6), a folder focus's.
    pub into: Option<IntoPlace>,
    /// "By your layout"'s tags for a folder's songs, once read.
    pub folder_tags: Option<Vec<KeepSong>>,
}

impl KeepView {
    /// Whether `d change` stands on the place line while choosing (spec
    /// §6.5.1): everywhere but a folder kept as it is on its server — no
    /// layout variable applies there.
    pub fn changes(&self) -> bool {
        self.choice != KeepChoice::Folder(FolderLanding::Mirror)
    }

    /// The landing a folder copy of the chosen radio sends; `None` for any
    /// other choice.
    pub fn job_landing(&self) -> Option<JobLanding> {
        match self.choice {
            KeepChoice::Folder(FolderLanding::Mirror) => Some(JobLanding::Mirror),
            KeepChoice::Folder(FolderLanding::Into) => Some(self.into.clone().unwrap_or_default().landing()),
            KeepChoice::Folder(FolderLanding::Tags) => Some(JobLanding::Tags),
            _ => None,
        }
    }

    /// Where a folder copy lands at `place` by `landing` (spec §6.5.1):
    /// its mirror under the layout's leading folders; the chosen folder;
    /// the account's layout by the folder rule ([`Self::scope_tags`]).
    pub fn folder_landing_at(&self, place: &CollectionDestination, landing: &JobLanding) -> Landing {
        let folder = self.subject.folder.as_ref().map(|f| f.path.as_str()).unwrap_or_default();
        match landing {
            JobLanding::Mirror => mirror_landing(place, folder),
            JobLanding::Into { vpath, path } => into_landing(place, vpath.as_deref(), path),
            JobLanding::Tags => {
                let (tags, by_album) = self.scope_tags(JobScope::Folder);
                place.landing(&tags, by_album)
            }
        }
    }

    /// Where a copy of `scope` lands at `place` — a folder job's by its own
    /// landing (spec §6.7: "Into" reads the job's, never the radio's).
    fn landing_of(&self, place: &CollectionDestination, job: Option<&PluginJob>, scope: JobScope) -> Landing {
        match job {
            Some(job) if scope == JobScope::Folder => self.folder_landing_at(place, &job.params.landing),
            _ => self.landing_at(place, scope),
        }
    }

    /// Whether `c choose again` stands (clause 110): an ended copy, where the
    /// subject offers two or more choices — never after a refusal (any
    /// other scope meets the same refusal) or a cancel (Add again brings
    /// the choice back), nor for a result this client cannot read.
    pub fn choose_again(&self) -> bool {
        let KeepStage::Job { job, outcome, .. } = &self.stage else { return false };
        self.choices.len() >= 2
            && !job.live()
            && !matches!(
                outcome,
                JobOutcome::Refused { .. } | JobOutcome::Cancelled { .. } | JobOutcome::Done | JobOutcome::Unknown
            )
    }

    /// The tags a copy of `scope` renders the layout with (the webapp's
    /// `dmScopeTags`), and whether it lands album by album. The server
    /// files each song by its own file's tags (destination.js
    /// `tagsForLayout`), so a variable a copy's songs need not agree on is
    /// a slot ([`SongTag`]), never one value promising one folder (the
    /// 2026-10-22 Night Market Sampler: "copies › From friends ›
    /// Boukmanflow" for three artists' songs):
    ///
    /// - **the song**: its own tags — ARTIST as its file has it where the
    ///   album's listing has its row ([`KeepSong::artist`]), else the row's;
    ///   GENRE its genre where that row names one, else a slot;
    ///   SERVERFOLDER its own folder on the peer ([`server_folder`]);
    /// - **its album**: ALBUM, ALBUMARTIST (its credit — albumArtist, else
    ///   the row's artist — so a compilation stays in one folder) and PEER
    ///   as the row says; ARTIST and YEAR the value every song of the
    ///   listing files under ([`agreed`]: a song with no artist under the
    ///   credit, as the copy files it; one with no year under none, its
    ///   folder dropped) — before the listing answers, the row's own unless
    ///   it is a compilation — else a slot; GENRE a slot unless every song
    ///   names the same one; SERVERFOLDER the folder every song of the
    ///   listing sits in — before it answers, the row's — else a slot;
    /// - **the artist scopes**: the track artist as ARTIST and ALBUMARTIST,
    ///   never a compilation's credit, and no album or year — each album
    ///   has its own — unless only-missing takes one album, which is named
    ///   with its year (the webapp's `scopeTarget` `oneAlbum`); GENRE a
    ///   slot; SERVERFOLDER each album's own, so [`Landing::of`] cuts at it
    ///   — a slot where only-missing names one album (its songs' folder is
    ///   nowhere this client reads);
    /// - **a folder by the layout** (spec §6.5.3, the webapp's folder rule):
    ///   over its songs' tags, read once at most 100 songs, ARTIST, YEAR,
    ///   GENRE and SERVERFOLDER the value every song agrees on, else a slot
    ///   (GENRE only where every song names one); ALBUMARTIST the songs'
    ///   agreeing credit, else as ARTIST renders; ALBUM by name where every
    ///   song agrees, else the first folder naming an album and all after
    ///   it are "one folder per album". No tags: every one a slot, and one
    ///   folder per album.
    fn scope_tags(&self, scope: JobScope) -> (LayoutTags, bool) {
        let s = &self.subject;
        let peer = Some(s.server.clone());
        let year = s.year.filter(|y| *y > 0).map(|y| y.to_string());
        // The row's own folder on the peer; a row with no path has none, as
        // the server reckons it.
        let row_folder = server_folder(s.filepath.as_deref().unwrap_or_default());
        match scope {
            JobScope::Folder => {
                let songs = self.folder_tags.as_deref().filter(|songs| !songs.is_empty());
                let shared = |value: fn(&KeepSong) -> Option<String>| songs.and_then(|songs| agreed(songs.iter().map(value)));
                let every = |value: fn(&KeepSong) -> Option<String>| {
                    songs.is_some_and(|songs| songs.iter().all(|song| !sanitize_segment(value(song).as_deref()).is_empty()))
                };
                let artist = shared(|song| song.artist.clone());
                let year = shared(|song| song.year.map(|y| y.to_string()));
                let album = shared(|song| song.album.clone());
                let genre = if every(|song| song.genre.clone()) { shared(|song| song.genre.clone()) } else { None };
                let credit = if every(|song| song.album_artist.clone()) { shared(|song| song.album_artist.clone()) } else { None };
                let folder = songs.and_then(|songs| {
                    let first = server_folder(&songs[0].filepath);
                    songs.iter().all(|song| server_folder(&song.filepath) == first).then_some(first)
                });
                let slots = [(artist.is_none(), SongTag::Artist), (year.is_none(), SongTag::Year), (genre.is_none(), SongTag::Genre), (folder.is_none(), SongTag::ServerFolder)]
                    .into_iter()
                    .filter_map(|(slot, tag)| slot.then_some(tag))
                    .collect();
                let tags = LayoutTags {
                    artist: artist.flatten(),
                    album: album.clone().flatten(),
                    year: year.flatten(),
                    genre: genre.flatten(),
                    album_artist: credit.flatten(),
                    peer,
                    server_folder: folder,
                    slots,
                };
                (tags, album.is_none())
            }
            JobScope::Artist | JobScope::ArtistMissing => {
                let one = self.one_missing.clone().filter(|_| scope == JobScope::ArtistMissing);
                let by_album = one.is_none();
                let (album, year) = match one {
                    Some((name, year)) => (Some(name), year.map(|y| y.to_string())),
                    None => (None, None),
                };
                let tags = LayoutTags {
                    artist: s.track_artist.clone(),
                    album_artist: s.track_artist.clone(),
                    album,
                    year,
                    peer,
                    slots: vec![SongTag::Genre, SongTag::ServerFolder],
                    ..LayoutTags::default()
                };
                (tags, by_album)
            }
            JobScope::Album => {
                let credit = s.album_credit().map(str::to_string);
                let songs = self.album_songs.as_deref().filter(|songs| !songs.is_empty());
                let (artist, year, genre) = match songs {
                    // A listed song with no artist files under the credit the
                    // listing was asked by (federation-copy `albumListing`'s
                    // `fallbackArtist`); one with no year has none to fall
                    // back to.
                    Some(songs) => (
                        agreed(songs.iter().map(|song| song.artist.clone().or_else(|| credit.clone()))),
                        agreed(songs.iter().map(|song| song.year.map(|y| y.to_string()))),
                        if songs.iter().all(|song| song.genre.is_some()) { agreed(songs.iter().map(|song| song.genre.clone())) } else { None },
                    ),
                    None if s.compilation() => (None, None, None),
                    None => (Some(s.track_artist.clone()), Some(year), None),
                };
                // The folders are the peer's, already clean: they agree
                // only where they are the same folders.
                let folder = match songs {
                    Some(songs) => {
                        let first = server_folder(&songs[0].filepath);
                        songs.iter().all(|song| server_folder(&song.filepath) == first).then_some(first)
                    }
                    None => Some(row_folder),
                };
                let slots = [(&artist, SongTag::Artist), (&year, SongTag::Year), (&genre, SongTag::Genre)]
                    .into_iter()
                    .filter_map(|(value, slot)| value.is_none().then_some(slot))
                    .chain(folder.is_none().then_some(SongTag::ServerFolder))
                    .collect();
                let tags = LayoutTags {
                    artist: artist.flatten(),
                    album: s.album.clone(),
                    year: year.flatten(),
                    genre: genre.flatten(),
                    album_artist: credit,
                    peer,
                    server_folder: folder,
                    slots,
                };
                (tags, false)
            }
            _ => {
                // The song's own row of the album's listing, where it
                // answered: its artist as its file has it, and its genre.
                let own = self.album_songs.iter().flatten().find(|song| s.filepath.as_deref() == Some(song.filepath.as_str()));
                let genre = own.and_then(|song| song.genre.clone());
                let slots = if genre.is_none() { vec![SongTag::Genre] } else { Vec::new() };
                let tags = LayoutTags {
                    artist: own.and_then(|song| song.artist.clone()).or_else(|| s.track_artist.clone()),
                    album: s.album.clone(),
                    year,
                    genre,
                    album_artist: s.album_artist.clone(),
                    peer,
                    server_folder: Some(row_folder),
                    slots,
                };
                (tags, false)
            }
        }
    }

    /// Where a copy of `scope` lands at a place — a folder copy by the
    /// chosen landing.
    fn landing_at(&self, place: &CollectionDestination, scope: JobScope) -> Landing {
        if scope == JobScope::Folder {
            return self.folder_landing_at(place, &self.job_landing().unwrap_or_default());
        }
        let (tags, by_album) = self.scope_tags(scope);
        Landing { one_song: scope == JobScope::Song, ..place.landing(&tags, by_album) }
    }

    /// Where a copy of the chosen scope would land with the picker's
    /// fields as typed — its PREVIEW (clause 143): a variable `known`
    /// refuses kept as typed.
    pub fn landing_for(&self, vpath: &str, base: &str, layout: &str, known: &dyn Fn(&str) -> bool) -> Landing {
        let (tags, by_album) = self.scope_tags(self.scope);
        Landing { one_song: self.scope == JobScope::Song, ..Landing::of(vpath, base, layout, &tags, by_album, known) }
    }

    /// The layout's path for this copy as the server measures it against
    /// its cap (destination.js `renderTarget`, the webapp's
    /// `previewTarget`): the layout alone — never a base folder — rendered
    /// whole, never cut at an artist copy's album folder, every variable
    /// by this copy's own value (a slot's too, where it has one; none
    /// drops out), in characters.
    pub fn rendered_len_for(&self, layout: &str, known: &dyn Fn(&str) -> bool) -> usize {
        let (mut tags, _) = self.scope_tags(self.scope);
        tags.slots.clear();
        Landing::of("", "", layout, &tags, false, known).rendered_len()
    }
}

/// The one value every song files under — each cleaned as the server
/// cleans a folder name, so two spellings of one folder agree — or `None`
/// where any two differ (a slot). `Some(None)`: none of them has one, and
/// the folder drops for all alike.
fn agreed(values: impl Iterator<Item = Option<String>>) -> Option<Option<String>> {
    let mut first: Option<Option<String>> = None;
    for value in values {
        match &first {
            None => first = Some(value),
            Some(have) if sanitize_segment(have.as_deref()) != sanitize_segment(value.as_deref()) => return None,
            Some(_) => {}
        }
    }
    first
}

/// Open elsewhere's list (clause 140).
#[derive(Debug, Clone, PartialEq)]
pub enum LinksStatus {
    Asking,
    /// https links only; empty is "no links for this song".
    Ready(Vec<PluginLink>),
    Failed(PluginFailure),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinksState {
    /// The row the list was asked for, as it was sent: Try again sends it
    /// again, whatever answer has landed since.
    rec: serde_json::Value,
    seq: u64,
    pub status: LinksStatus,
}

/// A row of the Copies list (clause 117).
#[derive(Debug, Clone, PartialEq)]
pub struct CopyRow {
    pub job: PluginJob,
    pub outcome: JobOutcome,
    pub title: CopyTitle,
    /// The server it copies from.
    pub from: Option<String>,
    pub action: Option<JobAction>,
    /// Its cancel is out.
    pub cancelling: bool,
}

/// What a Copies row is titled by, per scope (clause 117): the song, the
/// album, "every album by {artist}" (only the ones you don't have) — or a
/// folder, by its name (spec §6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTitle {
    Song(String),
    Album(String),
    Artist { artist: String, missing_only: bool },
    Folder(String),
}

impl CopyTitle {
    pub fn of(job: &PluginJob) -> CopyTitle {
        let text = |k: &str| job.rec_text(k).unwrap_or_default();
        match job.scope() {
            Some(JobScope::Album) => CopyTitle::Album(text("album")),
            Some(JobScope::Artist) => CopyTitle::Artist { artist: text("artist"), missing_only: false },
            Some(JobScope::ArtistMissing) => CopyTitle::Artist { artist: text("artist"), missing_only: true },
            // A folder's own name, else the last of its path's.
            Some(JobScope::Folder) => CopyTitle::Folder(job.rec_text("title").unwrap_or_else(|| {
                normalize_peer_path(&text("filepath")).rsplit('/').next().unwrap_or_default().to_string()
            })),
            _ => CopyTitle::Song(job.rec_text("title").unwrap_or_else(|| text("album"))),
        }
    }

    /// The plain name: a status note's tail.
    pub fn name(&self) -> &str {
        match self {
            CopyTitle::Song(s) | CopyTitle::Album(s) | CopyTitle::Folder(s) => s,
            CopyTitle::Artist { artist, .. } => artist,
        }
    }
}

/// A rows lookup out: its seq, the rows it asked in order, and the peers
/// answer they belong to.
#[derive(Debug, Clone, PartialEq)]
struct RowsLookup {
    seq: u64,
    rows: Vec<(usize, usize)>,
    landed: u64,
}

/// Everything Keep a copy holds for one of the user's own servers: its
/// gate, its jobs, the marks its rows wear, the open window, the picker —
/// kept across moves onto its connected servers (spec §3.2).
#[derive(Debug, Clone, Default)]
pub struct KeepState {
    /// The generation its commands carry now; `epochs` every one it ever
    /// carried — an answer lands in the state whose generations hold its
    /// own, whichever state is in front by then.
    pub epoch: u64,
    epochs: BTreeSet<u64>,
    /// The own server it belongs to (its identity); empty until one claims
    /// it.
    pub owner: String,
    seq: u64,
    pub gate: KeepGate,
    /// The Connected servers rows' jobs, by (group, row) in the peers
    /// answer they were looked up for.
    rows: BTreeMap<(usize, usize), RowJobs>,
    /// Which peers answer is on screen: bumped each time one lands, so a
    /// lookup made for an earlier one fills nothing.
    peers_landed: u64,
    /// The lookup out.
    lookup: Option<RowsLookup>,
    /// A connected server's page's marks (spec §5; the user's 2026-10-27
    /// decision on the song rows).
    pub marks: BrowseMarks,
    pub book: JobBook,
    pub watch: KeepWatch,
    pub flow: Option<KeepFlow>,
    /// The picker's asks.
    pub dest: DestState,
    pub links: Option<LinksState>,
    pub note: Option<KeepNote>,
    /// Jobs seen live and now finished, oldest first, for the status line
    /// (clause 118).
    news: Vec<PluginJob>,
    /// How the starts of a window closed before they answered went, oldest
    /// first, for the status line (clause 118): the job, or the refusal.
    landed: Vec<Result<PluginJob, PluginFailure>>,
}

impl KeepState {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// A job's state as anything here knows it, newest knowledge first.
    fn job(&self, id: i64) -> Option<&PluginJob> {
        self.book.jobs.get(&id).or_else(|| {
            self.flow.as_ref().and_then(|f| f.jobs.held.values().find(|j| j.id == id)).or_else(|| {
                self.rows
                    .values()
                    .chain(self.marks.rows.values().filter_map(|m| m.jobs.as_ref()))
                    .flat_map(|r| r.held.values())
                    .find(|j| j.id == id)
            })
        })
    }

    /// Fold one job in everywhere it belongs; true when it is news — seen
    /// live, now finished. Only the copy plug-in's jobs in a scope this
    /// client knows ([`listed_copy`]) — a folder copy among them (spec
    /// §6.7): another plug-in's, or a newer server's scope, never lists,
    /// lights the indicator, keeps the poll going or words a note, since
    /// nothing here could retry it.
    fn fold(&mut self, job: &PluginJob) -> bool {
        if !listed_copy(job) {
            return false;
        }
        if let Some(have) = self.book.jobs.get(&job.id)
            && have.updated_at.is_some()
            && job.updated_at.is_some()
            && have.updated_at > job.updated_at
        {
            return false;
        }
        self.book.jobs.insert(job.id, job.clone());
        for row in self.rows.values_mut() {
            row.merge(job);
        }
        self.marks.merge(job);
        if let Some(flow) = self.flow.as_mut() {
            flow.jobs.merge(job);
            flow.resettle(&self.gate);
        }
        if job.live() {
            self.book.seen_live.insert(job.id);
            return false;
        }
        if self.book.seen_live.remove(&job.id) {
            self.book.cancelling.remove(&job.id);
            self.news.push(job.clone());
            self.marks.ended(job.id);
            return true;
        }
        false
    }

    /// Anything still moving: a listed or held job, or a start out.
    fn any_live(&self) -> bool {
        self.book.jobs.values().any(PluginJob::live)
            || self.rows.values().flat_map(|r| r.held.values()).any(PluginJob::live)
            || self.marks.live()
            || self.flow.as_ref().is_some_and(|f| {
                matches!(f.phase, KeepPhase::Starting { .. }) || f.jobs.held.values().any(PluginJob::live)
            })
            || !self.book.starts.is_empty()
            || !self.book.unseen.is_empty()
    }

    /// The open Keep a copy modal shows a copy of its own moving: its
    /// start is out, the job it follows is live, or one of its subject's
    /// slots holds a live job — what earns the modal's quick beat (clause
    /// 112). A modal still choosing, over a copy that is not its own, does
    /// not.
    fn flow_live(&self) -> bool {
        let Some(flow) = self.flow.as_ref() else { return false };
        match flow.phase {
            KeepPhase::Starting { .. } => return true,
            KeepPhase::Following(id) if self.job(id).is_some_and(PluginJob::live) => return true,
            _ => {}
        }
        flow.jobs.held.values().any(PluginJob::live)
    }
}

/// A copy this client lists, polls and retries: the copy plug-in's, in a
/// scope it knows — Discover's four and a folder (spec §6.7) — never a
/// scope a newer server added.
fn listed_copy(job: &PluginJob) -> bool {
    job.plugin == COPY_PLUGIN && job.scope().is_some()
}

/// A Copies row's action (clause 117): the outcome's, except that Copy
/// again — which takes only the gaps — stands only where a gap is left (a
/// song failed, an album's listing failed, songs not reached), and a file
/// in the way offers nothing: the list's Retry sends the place the copy ran
/// with, so it would meet the same file — Keep a copy is where the place
/// changes first.
pub fn copies_action(outcome: &JobOutcome) -> Option<JobAction> {
    match outcome {
        JobOutcome::CopiedMany(n) if n.failed == 0 && n.albums_failed == 0 && n.total <= n.copied + n.skipped => None,
        JobOutcome::Exists { .. } => None,
        other => other.action(),
    }
}

/// The Keep a copy window's action for an ended copy (clause 110) — its
/// own table, not the Copies list's: a many-song copy with a gap offers
/// Retry (it takes only the gaps); one without, Play your copy for an
/// album whose result names files to play — the whole album from the
/// user's library — and Close for the artist scopes (no one queue) or a
/// result that names none; a file in the way, Retry only where the
/// account's place moved since (`again`: the same place would meet the
/// same file); the rest, the outcome's own — Cancelled's Start again reads
/// "Add again".
fn window_action(view: &KeepView) -> Option<JobAction> {
    let KeepStage::Job { job, outcome, .. } = &view.stage else { return None };
    match outcome {
        JobOutcome::CopiedMany(_) if copies_action(outcome).is_some() => Some(JobAction::Retry),
        // An album's, or a folder's (spec §6.7): your copies of its songs.
        JobOutcome::CopiedMany(_) => {
            let plays = matches!(job.scope(), Some(JobScope::Album | JobScope::Folder)) && !job.copy_result().kept_songs().is_empty();
            plays.then_some(JobAction::Play)
        }
        JobOutcome::Exists { .. } => view.again.is_some().then_some(JobAction::Retry),
        other => other.action(),
    }
}

/// Where Retry would land (clause 145): the account's place now — what it
/// sends — for an ended copy the window's Retry may stand for (a failure,
/// a stop, a gap; a file in the way, whose Retry stands only once this is
/// said), where that is not the place the copy ran with (its result's,
/// else its pin); `None` where it is the same. Where no such place is
/// known (a server from before the pin) a failure, a stop or a gap says
/// it all the same; a file in the way only where the place now renders
/// another folder than the one the file sits in — Retry there would meet
/// the same file, and an unknown is no difference.
fn keep_again_landing(view: &KeepView) -> Option<Landing> {
    let KeepStage::Job { job, outcome, .. } = &view.stage else { return None };
    let exists = matches!(outcome, JobOutcome::Exists { .. });
    if !exists && !matches!(copies_action(outcome), Some(JobAction::Retry | JobAction::CopyAgain)) {
        return None;
    }
    let now = view.destination.as_ref().filter(|_| !job.live())?;
    let again = view.landing_of(now, Some(job), job.scope().unwrap_or(view.scope));
    let same = match job.copy_result().destination.or_else(|| job.params.destination.clone().flatten()) {
        Some(ran) => ran.same_place(now),
        None if exists => job.copy_result().landed().and_then(|landed| names_folder(&again, &landed)).unwrap_or(true),
        None => false,
    };
    (!same).then_some(again)
}

/// Whether a place, as rendered for a copy, is the folder a file landed in
/// — `None` where nothing can say: the place holds a slot or a variable it
/// keeps as typed, or the file's library is unknown.
fn names_folder(place: &Landing, landed: &Landed) -> Option<bool> {
    let library = landed.vpath.as_deref()?;
    if place.folders.iter().flatten().any(|(_, run)| *run != Run::Text) {
        return None;
    }
    let folders: Vec<String> = place.folders.iter().map(|folder| folder.iter().map(|(text, _)| text.as_str()).collect()).collect();
    Some(place.library == library && folders == landed.dirs)
}

/// An album copy's songs in the user's library as Play your copy queues
/// them ([`CopyResult::kept_songs`](crate::api::types::CopyResult::kept_songs)):
/// in the album's own order where its listing names them — found by the
/// song's file on its server, which the result's lists carry for every
/// song (a song already there carries no title) — the rest after, in the
/// result's order. A copied song keeps the tags it landed with; one
/// already there is named by its album alone.
fn album_tracks(job: &PluginJob, listing: Option<&[KeepSong]>) -> Vec<Track> {
    let place = |from: &Option<String>| {
        listing
            .and_then(|songs| songs.iter().position(|song| from.as_deref() == Some(song.filepath.as_str())))
            .unwrap_or(usize::MAX)
    };
    let mut kept = job.copy_result().kept_songs();
    kept.sort_by_key(|song| place(&song.from));
    kept.into_iter()
        .map(|song| Track {
            filepath: song.filepath,
            metadata: crate::api::types::TrackMetadata {
                title: song.title,
                artist: song.artist,
                album: song.album.or_else(|| job.rec_text("album")),
                ..Default::default()
            },
        })
        .collect()
}

/// An album target's mark key: its name and credit by name key, its year.
fn album_key(target: &KeepTarget) -> Option<MarkKey> {
    let KeepTarget::Album { name, album_artist, year, .. } = target else { return None };
    Some(MarkKey::Album { name: name_key(Some(name)), credit: name_key(Some(album_artist)), year: *year })
}

/// The poll's wait after `failures` failures in a row: the beat doubled
/// each time, never past [`KEEP_POLL_MAX`].
pub fn keep_backoff(beat: Duration, failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures.min(16)).unwrap_or(u32::MAX);
    beat.saturating_mul(factor).min(KEEP_POLL_MAX)
}

/// What Keep a copy holds beside the state in front (spec §3.2): the other
/// own servers' states, parked until theirs is in front again; each own
/// server's flags as its last ping said them (gap 9); its pings out; the
/// connected servers each own server listed when last asked (gate rule
/// 6); the album page's credit ask out.
#[derive(Debug, Clone, Default)]
pub struct KeepShelf {
    parked: Vec<KeepState>,
    caps: BTreeMap<String, Capabilities>,
    pinging: BTreeSet<String>,
    listed: BTreeMap<String, Vec<i64>>,
    credit: Option<CreditAsk>,
    /// The last generation handed out apart from the session's own.
    fresh: u64,
}

/// The album page's credit, step 3, while it is out (spec §4.1.3): the
/// page it was asked for, and since when — 8 s without an answer is no
/// answer.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditAsk {
    seq: u64,
    generation: u64,
    node: LibraryNode,
    since: crate::clock::Instant,
}

/// "By your layout" reads a folder's songs' tags only up to this many
/// (spec §6.5.3).
pub const TAGS_MAX: usize = 100;

/// The album page's credit step 3 waits this long, and no longer.
pub const CREDIT_WAIT: Duration = Duration::from_secs(8);

/// A connected server's page whose rows wear marks (spec §5.0): the marks
/// holder's lifetime — its marks and their asks go when the page, the room
/// or the session does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarksPage {
    /// A Library pane view: an album page, an artist's wall, the Albums
    /// wall, Recent, a genre's page.
    Library(LibraryNode),
    /// Files at a folder (its path).
    Files(String),
    /// Search, its query's hits — and a drill inside them.
    Search(String),
}

/// One row a page asks its mark of — the rows on screen, or near them; a
/// row asked once is never asked again on that page until a copy of it
/// that was live ends.
#[derive(Debug, Clone, Copy)]
pub enum MarkAsk<'a> {
    /// A song row, in any room ([`App::keep_song_mark`]).
    Song(&'a Track),
    /// A wall's card ([`App::keep_card_mark`]): one with a known credit.
    Card(&'a Album),
    /// The album page on screen, for its header ([`App::keep_page_state`]).
    AlbumPage,
    /// The artist's wall on screen, for its header.
    ArtistWall,
    /// A folder row, or the open folder (its path; [`App::keep_folder_mark`]).
    Folder(&'a str),
}

/// A marked row's identity within its page.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MarkKey {
    Song(String),
    Album { name: String, credit: String, year: Option<i32> },
    Artist(String),
    Folder(String),
}

/// What a marked row's mark is read through: a song by its file and its
/// album ([`song_mark`]), an album by its name ([`card_mark`]), an artist
/// ([`artist_mark`]), a folder ([`folder_mark`]).
#[derive(Debug, Clone, Default)]
enum MarkOf {
    Song { filepath: String, album: Option<String> },
    Album(String),
    #[default]
    Artist,
    Folder,
}

/// One marked row: what its lookup sends, its jobs once it answered, and
/// the mark they leave — worked out each time they change, never per
/// frame (a done artist copy's lists are read through each song).
#[derive(Debug, Clone, Default)]
struct MarkRow {
    rec: serde_json::Value,
    /// `None` until its lookup answers — and for good after one that
    /// failed: a failed lookup draws nothing.
    jobs: Option<RowJobs>,
    asked: bool,
    /// A copy it held while live has ended: it is asked again once the
    /// page lists it again — on screen, or near.
    stale: bool,
    of: MarkOf,
    mark: Option<JobMark>,
}

impl MarkRow {
    /// The mark its jobs leave now, on the page of the server `peer`.
    fn remark(&mut self, peer: i64) {
        self.mark = self.jobs.as_ref().and_then(|jobs| match &self.of {
            MarkOf::Song { filepath, album } => song_mark(jobs, peer, album.as_deref(), filepath),
            MarkOf::Album(name) => card_mark(jobs, peer, name),
            MarkOf::Artist => artist_mark(jobs, peer),
            MarkOf::Folder => folder_mark(jobs),
        });
    }
}

/// A connected server's page's marks (spec §5.0; the user's 2026-10-27
/// decision on Recent, a genre's page, Search's song hits and Files' file
/// rows): rows asked as they come on screen (or near), one lookup of at
/// most [`LOOKUP_BATCH`] at a time, kept for the page; the poll keeps a live
/// one current, and only a row whose copy was live is asked again.
#[derive(Debug, Clone, Default)]
pub struct BrowseMarks {
    /// The page, and the session generation it was asked under.
    page: Option<(MarksPage, u64)>,
    /// The page's server: its marks are this server's.
    peer: i64,
    rows: BTreeMap<MarkKey, MarkRow>,
    /// The rows not asked yet, in the order they came — one asked again
    /// first: taken [`LOOKUP_BATCH`] at a time.
    pending: std::collections::VecDeque<MarkKey>,
    /// The lookup out: its seq, and the rows it asked in order.
    lookup: Option<(u64, Vec<MarkKey>)>,
    /// Bumped whenever rows went stale: the page lists its rows again.
    stirred: u64,
    /// Marks worked out — a frame with nothing new works out none.
    #[cfg(test)]
    worked: usize,
}

impl MarksPage {
    /// The same page — an album page whose credit its resolver found since
    /// is still the page it was (its marks are kept; its own ask joins).
    fn same(&self, other: &MarksPage) -> bool {
        match (self, other) {
            (
                MarksPage::Library(LibraryNode::Album { name: a, artist: x, year: y, .. }),
                MarksPage::Library(LibraryNode::Album { name: b, artist: w, year: z, .. }),
            ) => a == b && x == w && y == z,
            _ => self == other,
        }
    }

    /// The pane its song rows are drawn in — an album page lends its credit
    /// to its own rows there alone.
    fn pane(&self) -> Option<Tab> {
        match self {
            MarksPage::Library(_) => Some(Tab::Library),
            MarksPage::Search(_) => Some(Tab::Search),
            MarksPage::Files(_) => None,
        }
    }
}

/// At most this many rows a lookup of a page's marks asks (the route's
/// batch, `LOOKUP_BATCH_MAX`).
pub const LOOKUP_BATCH: usize = crate::api::types::LOOKUP_BATCH_MAX;

/// The rows a page's list asks marks of at a time: one block and the
/// page's own ask fill one lookup.
pub const MARKS_BLOCK: usize = LOOKUP_BATCH - 1;

/// The rows a list asks its marks of (spec §5.0; the user's Q2: the rows on
/// screen, or near): the whole blocks of [`MARKS_BLOCK`] rows that hold the
/// `visible` rows from `first` on, of a list of `len` — a scroll asks a block
/// at a time, never a row at a time, however it moves.
pub fn marks_block(first: usize, visible: usize, len: usize) -> std::ops::Range<usize> {
    if visible == 0 || first >= len {
        return 0..0;
    }
    let last = (first + visible - 1).min(len - 1);
    first / MARKS_BLOCK * MARKS_BLOCK..((last / MARKS_BLOCK + 1) * MARKS_BLOCK).min(len)
}

impl BrowseMarks {
    /// A held job still moving.
    fn live(&self) -> bool {
        self.rows.values().filter_map(|row| row.jobs.as_ref()).flat_map(|jobs| jobs.held.values()).any(PluginJob::live)
    }

    /// A row's lookup answered: its jobs, and the mark they leave.
    fn land(&mut self, key: &MarkKey, jobs: RowJobs) {
        let peer = self.peer;
        if let Some(row) = self.rows.get_mut(key) {
            row.jobs = Some(jobs);
            row.remark(peer);
            #[cfg(test)]
            {
                self.worked += 1;
            }
        }
    }

    /// A job from the poll, a start or a cancel, merged into each row that
    /// holds its key — their marks worked out again.
    fn merge(&mut self, job: &PluginJob) {
        let peer = self.peer;
        for row in self.rows.values_mut() {
            if row.jobs.as_mut().is_some_and(|jobs| jobs.merge(job)) {
                row.remark(peer);
                #[cfg(test)]
                {
                    self.worked += 1;
                }
            }
        }
    }

    /// A copy that was live ended: the rows that held it go stale — asked
    /// again once the page lists them again (spec §5.0, "re-ask only the
    /// live recs"), the ones scrolled far off never.
    fn ended(&mut self, id: i64) {
        let mut stirred = false;
        for row in self.rows.values_mut() {
            if row.asked && row.jobs.as_ref().is_some_and(|jobs| jobs.held.values().any(|j| j.id == id)) {
                row.stale = true;
                stirred = true;
            }
        }
        if stirred {
            self.stirred += 1;
        }
    }

    /// Ask `key` again, first: its window closed, or its lookup went
    /// unanswered.
    fn again(&mut self, key: &MarkKey) {
        if let Some(row) = self.rows.get_mut(key)
            && row.asked
        {
            row.asked = false;
            row.stale = false;
            self.pending.push_front(key.clone());
        }
    }

    fn jobs(&self, key: &MarkKey) -> Option<&RowJobs> {
        self.rows.get(key)?.jobs.as_ref()
    }

    /// A row's mark, as last worked out.
    fn mark(&self, key: &MarkKey) -> Option<JobMark> {
        self.rows.get(key)?.mark
    }
}

impl App {
    // ── Whose state (spec §3.2) ─────────────────────────────────────────────

    /// The user's own server the session is on — or browses a connected
    /// server of: that server's parent.
    pub(crate) fn keep_session_owner(&self) -> String {
        self.origin().server
    }

    /// The state in front is the session's own server's, and the session
    /// IS that server: its commands ride the session, its flags are the
    /// session's.
    fn keep_is_session(&self) -> bool {
        self.session.peer.is_none()
            && (self.keep.owner.is_empty() || crate::config::same_server(&self.keep.owner, &self.keep_session_owner()))
    }

    /// The own server a target belongs to: a connected server's thing, its
    /// parent's; a Discover row's, the session's.
    fn keep_target_owner(&self, target: &KeepTarget) -> String {
        target.origin().map(|o| o.server.clone()).unwrap_or_else(|| self.keep_session_owner())
    }

    /// Where a copy of the state in front lives: its own server — a copied
    /// song is played from there (Play your copy), whichever server is
    /// browsed.
    fn keep_home(&self) -> Origin {
        let owner = if self.keep.owner.is_empty() { self.keep_session_owner() } else { self.keep.owner.clone() };
        Origin { server: owner, peer: None }
    }

    /// The state in front follows the session (spec §3.2): a move to
    /// another own server parks it and brings that server's back — or a
    /// fresh one; a move onto a connected server of the same one keeps it.
    /// Every surface that asks something calls this first.
    pub(crate) fn keep_sync(&mut self) {
        let owner = self.keep_session_owner();
        // A window open on another own server's song (a queue item's, spec
        // §4.8) holds that server in front until it closes.
        let held = self.keep.flow.is_some() && !self.keep.owner.is_empty() && !crate::config::same_server(&self.keep.owner, &owner);
        if !owner.is_empty() && !held {
            self.keep_focus(&owner);
        }
    }

    /// The state of `owner` in front (a queue item's own server's, while
    /// its window is open; the session's otherwise), stamped with the
    /// session's generation where no other state holds it — so a reply
    /// lands in the state it was asked for, by its generation alone.
    fn keep_focus(&mut self, owner: &str) {
        if !self.keep.owner.is_empty() && !crate::config::same_server(&self.keep.owner, owner) {
            let back = self.keep_shelf.parked.iter().position(|k| crate::config::same_server(&k.owner, owner));
            let state = match back {
                Some(at) => self.keep_shelf.parked.remove(at),
                None => KeepState::default(),
            };
            let left = std::mem::replace(&mut self.keep, state);
            self.keep_shelf.parked.push(left);
        }
        if self.keep.owner.is_empty() {
            self.keep.owner = owner.to_string();
        }
        let generation = self.session_gen;
        if self.keep.epochs.contains(&generation) {
            self.keep.epoch = generation;
            return;
        }
        let taken = self.keep_shelf.parked.iter().any(|k| k.epochs.contains(&generation));
        let epoch = if taken {
            // Apart from every session generation: from the top down.
            self.keep_shelf.fresh += 1;
            u64::MAX - self.keep_shelf.fresh
        } else {
            generation
        };
        self.keep.epoch = epoch;
        self.keep.epochs.insert(epoch);
    }

    /// The state an own server's things are judged by: the one in front, or
    /// its parked one.
    fn keep_state_for(&self, owner: &str) -> Option<&KeepState> {
        if self.keep.owner.is_empty() || crate::config::same_server(&self.keep.owner, owner) {
            return Some(&self.keep);
        }
        self.keep_shelf.parked.iter().find(|k| crate::config::same_server(&k.owner, owner))
    }

    /// An own server's flags: the session's while it is that server, else
    /// its last ping's (gap 9); nothing known, nothing granted.
    fn keep_caps_for(&self, owner: &str) -> Option<Capabilities> {
        if self.session.peer.is_none() && (owner.is_empty() || crate::config::same_server(owner, &self.keep_session_owner())) {
            return Some(self.capabilities);
        }
        self.keep_shelf.caps.iter().find(|(id, _)| crate::config::same_server(id, owner)).map(|(_, caps)| *caps)
    }

    /// The flags Keep a copy is gated on: the state in front's own
    /// server's.
    pub fn keep_caps(&self) -> Capabilities {
        self.keep_caps_for(&self.keep.owner).unwrap_or_default()
    }

    /// What every plug-in command of the state in front carries: its
    /// generation, and its own server's reach — `None`, the session, while
    /// the session is that server. `None` where that server cannot be
    /// reached now: nothing is sent.
    fn keep_cmd(&self) -> Option<(u64, Option<Reach>)> {
        if self.keep_is_session() {
            return Some((self.keep.epoch, None));
        }
        let reach = self.reach(&Origin { server: self.keep.owner.clone(), peer: None }).ok()?;
        Some((self.keep.epoch, Some(reach)))
    }

    /// A session came up (its `Connected`): an own server's flags are kept
    /// for the times one of its connected servers is browsed (gap 9), and
    /// the gate is asked of the own server — on a connected server through
    /// its parent's flags, a probe ping first where none are known (spec
    /// §3.2) — so the queue's sheet and Copies stand in every room.
    pub(crate) fn keep_connected(&mut self) -> Vec<Effect> {
        if self.session.peer.is_none() {
            let id = self.keep_session_owner();
            if !id.is_empty() {
                self.keep_shelf.caps.insert(id, self.capabilities);
            }
        }
        self.keep_sync();
        self.keep_ask_gate()
    }

    /// One probe ping of the state in front's own server, for its flags.
    fn keep_own_ping(&mut self) -> Vec<Effect> {
        let owner = self.keep.owner.clone();
        if owner.is_empty() || self.keep_shelf.pinging.contains(&owner) {
            return Vec::new();
        }
        let Ok(reach) = self.reach(&Origin { server: owner.clone(), peer: None }) else { return Vec::new() };
        self.keep_shelf.pinging.insert(owner.clone());
        vec![Effect::Api(ApiCmd::OwnPing { server: owner, reach })]
    }

    /// An own server's probe ping answered: its flags are kept, and its
    /// gate asked where its state is in front.
    pub(super) fn keep_own_pinged(&mut self, server: String, ping: Option<Box<crate::api::types::Ping>>) -> Vec<Effect> {
        self.keep_shelf.pinging.remove(&server);
        let Some(ping) = ping else { return Vec::new() };
        let caps = Capabilities::from(ping.as_ref());
        self.keep_shelf.caps.retain(|id, _| !crate::config::same_server(id, &server));
        self.keep_shelf.caps.insert(server.clone(), caps);
        if crate::config::same_server(&self.keep.owner, &server) {
            return self.keep_ask_gate();
        }
        // A parked own server's — a queue item's (W10): asked in its own
        // state, the session's put back in front.
        if self.keep_shelf.parked.iter().any(|k| crate::config::same_server(&k.owner, &server)) {
            self.keep_focus(&server);
            let effects = self.keep_ask_gate();
            self.keep_sync();
            return effects;
        }
        Vec::new()
    }

    /// A queue item's sheet opened, its connected server's own server not
    /// the session's (W10): that server's gate, where nothing asked it yet —
    /// its probe ping first — so the sheet's Keep a copy… row stands the
    /// next time it opens (a sheet's row never comes or goes while it is
    /// open, spec §3.2).
    pub(crate) fn keep_ask_owner(&mut self, origin: &Origin) -> Vec<Effect> {
        let owner = origin.server.clone();
        if origin.peer.is_none() || owner.is_empty() || crate::config::same_server(&owner, &self.keep_session_owner()) || self.keep.flow.is_some() {
            return Vec::new();
        }
        let known = self.keep_shelf.parked.iter().find(|k| crate::config::same_server(&k.owner, &owner));
        if known.is_some_and(|k| k.gate.plugins.is_some() || k.gate.asking) || self.keep_shelf.pinging.contains(&owner) {
            return Vec::new();
        }
        self.keep_focus(&owner);
        let effects = self.keep_ask_gate();
        self.keep_sync();
        effects
    }

    /// The connected servers `parent` lists now (gate rule 6).
    pub(crate) fn keep_peers_listed(&mut self, parent: &str, ids: Vec<i64>) {
        self.keep_shelf.listed.retain(|id, _| !crate::config::same_server(id, parent));
        self.keep_shelf.listed.insert(parent.to_string(), ids);
    }

    /// `origin` is a connected server its parent still lists (gate rule
    /// 6): a saved one, and — once the parent answered — on its list.
    pub fn keep_peer_listed(&self, origin: &Origin) -> bool {
        let Some(id) = origin.peer else { return false };
        let saved = self
            .servers
            .iter()
            .any(|s| s.peer.as_ref().is_some_and(|(parent, pid)| *pid == id && crate::config::same_server(parent, &origin.server)));
        let listed = self.keep_shelf.listed.iter().find(|(parent, _)| crate::config::same_server(parent, &origin.server));
        saved && listed.is_none_or(|(_, ids)| ids.contains(&id))
    }

    /// A connected server's name, as the saved list calls it.
    fn keep_peer_name(&self, origin: &Origin) -> String {
        let Some(id) = origin.peer else { return String::new() };
        self.servers
            .iter()
            .find(|s| s.peer.as_ref().is_some_and(|(parent, pid)| *pid == id && crate::config::same_server(parent, &origin.server)))
            .map(|s| s.name.clone())
            .unwrap_or_default()
    }

    // ── The gate (clause 101) ───────────────────────────────────────────────

    /// `state`'s listing says copies run there: its own server's ping's
    /// `discoveryPlugins`, the copy plug-in listed and jobs allowed — what
    /// the lookups, the marks and the poll need.
    fn keep_listed_in(&self, state: &KeepState) -> bool {
        self.connected
            && self.keep_caps_for(&state.owner).is_some_and(|caps| caps.discovery_plugins)
            && state.gate.plugins.as_ref().is_some_and(|p| p.jobs.allowed && p.copy_plugin().is_some())
    }

    /// `state`'s every entry point's gate (clause 101).
    fn keep_gate_in(&self, state: &KeepState) -> bool {
        self.keep_listed_in(state)
            && self.keep_caps_for(&state.owner).is_some_and(|caps| caps.uploads)
            && state.gate.view.as_ref().and_then(|v| v.destination.as_ref()).is_some()
    }

    /// The listing says copies run on the own server in front (the one the
    /// session is, or browses a connected server of): the ping's
    /// `discoveryPlugins`, the copy plug-in listed and jobs allowed — what
    /// the lookups, the marks and the poll need.
    pub fn keep_listed(&self) -> bool {
        self.keep_listed_in(&self.keep)
    }

    /// Every Keep a copy entry point's gate (clause 101): the listing's
    /// half, the account's upload right and a library to land in. Hidden,
    /// never grey, while any is missing. Asked of the own server whichever
    /// server is browsed (spec §3.1).
    pub fn keep_gate(&self) -> bool {
        self.keep_gate_in(&self.keep)
    }

    /// Where copies land, while the server names somewhere.
    pub fn keep_destination(&self) -> Option<&CollectionDestination> {
        self.keep_dest_view()?.destination.as_ref()
    }

    /// The destination's view as last read — what the picker draws from:
    /// the libraries, their templates, the variables (clause 142).
    pub fn keep_dest_view(&self) -> Option<&CollectionDestinationView> {
        self.keep.gate.view.as_ref()
    }

    /// Open elsewhere's gate (clause 140): the flag and a links plug-in —
    /// the Discover room's alone.
    pub fn links_gate(&self) -> bool {
        self.connected
            && self.capabilities.discovery_plugins
            && self.session.peer.is_none()
            && self.keep.gate.plugins.as_ref().is_some_and(|p| p.links_plugin().is_some())
    }

    /// Ask the gate (a visit's first frame, clause 101; a session's
    /// `Connected`; a move onto a connected server, spec §3.2): the listing,
    /// then the destination behind it — of the own server, its flags from
    /// its last ping or one probe ping first. One at a time; nothing
    /// without the flag.
    pub(crate) fn keep_ask_gate(&mut self) -> Vec<Effect> {
        if !self.connected {
            return Vec::new();
        }
        let Some(caps) = self.keep_caps_for(&self.keep.owner) else { return self.keep_own_ping() };
        if !caps.discovery_plugins || self.keep.gate.asking {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        let gate = &mut self.keep.gate;
        gate.asking = true;
        gate.asked = seq;
        vec![Effect::Api(ApiCmd::KeepGate { generation, reach })]
    }

    // ── Targets: the ways in (spec §4) ──────────────────────────────────────

    /// A connected server's song as Keep a copy takes it (W2, W8–W11): a
    /// browse row, a queue item, the playing card — a row drawn in the pane
    /// `from` with the credit of the album page that pane shows, where it is
    /// one of that page's own songs (the Library's drill, Search's drilled
    /// album); any other song with none (its window finds it). `None` for
    /// the user's own server's song.
    pub fn keep_track(&self, origin: &Origin, track: &Track, from: Option<Tab>) -> Option<KeepTarget> {
        origin.peer?;
        let album_artist = from.filter(|_| self.is_session_origin(origin)).and_then(|tab| self.page_credit(tab, track));
        Some(KeepTarget::Track { origin: origin.clone(), track: Box::new(track.clone()), album_artist: album_artist.unwrap_or_default() })
    }

    /// The credit of the album page the pane `tab` shows, for one of that
    /// page's own songs — by its file among the page's rows, never by its
    /// album's name: another album of that name is another album.
    fn page_credit(&self, tab: Tab, track: &Track) -> Option<AlbumCredit> {
        let (node, entries) = match tab {
            Tab::Library => (self.library_stack.here(), &self.library.entries),
            Tab::Search => match self.search_stack.here() {
                SearchNode::Library(node) => (node, &self.search.entries),
                _ => return None,
            },
            _ => return None,
        };
        let LibraryNode::Album { album_artist, .. } = node else { return None };
        let own = entries.iter().any(|entry| matches!(entry, Entry::Track { track: row, .. } if row.filepath == track.filepath));
        own.then(|| album_artist.clone())
    }

    /// The album page on screen as Keep a copy takes it (W1): on a
    /// connected server, once its credit is known — never one track's
    /// artist (rule 1), never the singles bucket.
    pub fn keep_album_page(&self) -> Option<KeepTarget> {
        let origin = self.origin();
        origin.peer?;
        let LibraryNode::Album { name, album_artist: AlbumCredit::Known(credit), year, compilation, .. } = self.library_stack.here() else {
            return None;
        };
        if name.trim().is_empty() {
            return None;
        }
        let art = self.library.entries.iter().find_map(|e| match e {
            Entry::Track { track, .. } => track.metadata.album_art.clone(),
            _ => None,
        });
        Some(KeepTarget::Album {
            origin,
            name: name.clone(),
            album_artist: credit.clone(),
            year: *year,
            compilation: *compilation || various_artists(Some(credit)),
            art,
        })
    }

    /// A wall's card as Keep a copy takes it (W3, W5): on a connected
    /// server, its listing's credit known — a null credit (its rows
    /// disagree) and the singles bucket keep `c` inert.
    pub fn keep_album_card(&self, card: &Album) -> Option<KeepTarget> {
        let origin = self.origin();
        origin.peer?;
        let name = card.name.clone().filter(|n| !n.trim().is_empty())?;
        let AlbumCredit::Known(credit) = card.credit() else { return None };
        Some(KeepTarget::Album {
            origin,
            name,
            album_artist: credit.clone(),
            year: card.year.filter(|y| *y > 0),
            compilation: card.compilation || various_artists(Some(&credit)),
            art: card.album_art_file.clone(),
        })
    }

    /// The artist's wall on screen as Keep a copy takes it (W4): on a
    /// connected server, once its own listing answered naming credits, with
    /// one album of theirs at least by the copy's rule — never Various
    /// Artists (spec §4.1.3).
    pub fn keep_artist_wall(&self) -> Option<KeepTarget> {
        let origin = self.origin();
        origin.peer?;
        let LibraryNode::Artist(name) = self.library_stack.here() else { return None };
        if name.trim().is_empty() || various_artists(Some(name)) {
            return None;
        }
        let rows = self.keep_wall_rows(name)?;
        let theirs = rows.iter().any(|row| row.credit_listed) && !plan_artist_albums(&rows, name).theirs.is_empty();
        theirs.then(|| KeepTarget::Artist { origin, name: name.clone() })
    }

    /// The artist's wall's own listing, as the Keep facts read it.
    fn keep_wall_rows(&self, artist: &str) -> Option<Vec<ArtistAlbumRow>> {
        let (listed, albums) = self.artist_albums.as_ref()?;
        (name_key(Some(listed)) == name_key(Some(artist))).then(|| albums.iter().map(Album::as_row).collect())
    }

    /// A Files folder as Keep a copy takes it (W6, W7): on a connected
    /// server, below a library's top — `path` as Files names it (the open
    /// folder's [`App::path`], or a folder row's). A library's top, and `/`,
    /// are none.
    pub fn keep_folder(&self, path: &str) -> Option<KeepTarget> {
        let origin = self.origin();
        origin.peer?;
        let folder = FolderRef::of(path)?;
        Some(KeepTarget::Folder { origin, path: folder.path, name: folder.name })
    }

    /// A browse target's recommendation (spec §5.0): `source` federation,
    /// its server `{ id, name }`, then its own — a song's path there (no
    /// leading slash), title (its file's name for an untagged one), primary
    /// artist, album, the album page's known credit, year, length; an
    /// album's name and credit, its credit as `artist` too unless it is a
    /// compilation, its year; an artist; a folder's path and name. A year
    /// the route would refuse is left out.
    fn keep_rec(&self, target: &KeepTarget) -> Option<serde_json::Value> {
        let origin = target.origin()?;
        let peer = serde_json::json!({ "id": origin.peer?, "name": self.keep_peer_name(origin) });
        let year = |y: Option<i32>| y.filter(|y| (1000..=9999).contains(y));
        let mut rec = serde_json::json!({ "source": "federation", "peer": peer });
        let fields = rec.as_object_mut()?;
        let mut put = |key: &str, value: Option<serde_json::Value>| {
            if let Some(value) = value {
                fields.insert(key.to_string(), value);
            }
        };
        let text = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(|s| serde_json::Value::String(s.to_string()));
        match target {
            KeepTarget::Track { track, album_artist, .. } => {
                let md = &track.metadata;
                put("filepath", text(Some(track.filepath.trim_start_matches('/'))));
                put("title", text(Some(track.title_or_file())));
                put("artist", text(md.artist.as_deref()));
                put("album", text(md.album.as_deref()));
                put("albumArtist", text(album_artist.known()));
                put("year", year(md.year).map(serde_json::Value::from));
                put("duration", md.duration.filter(|d| d.is_finite() && *d > 0.0).map(serde_json::Value::from));
            }
            KeepTarget::Album { name, album_artist, year: y, compilation, .. } => {
                put("album", text(Some(name)));
                put("albumArtist", text(Some(album_artist)));
                put("artist", Some(if *compilation { serde_json::Value::Null } else { serde_json::Value::String(album_artist.clone()) }));
                put("year", year(*y).map(serde_json::Value::from));
            }
            KeepTarget::Artist { name, .. } => put("artist", text(Some(name))),
            KeepTarget::Folder { path, name, .. } => {
                put("filepath", text(Some(path)));
                put("title", text(Some(name)));
            }
            KeepTarget::Row { .. } | KeepTarget::Tile(_) => return None,
        }
        Some(rec)
    }

    /// The subject a target names: a Discover row or tile from the answer
    /// on screen; a browse target from itself.
    fn keep_subject(&self, target: &KeepTarget) -> Option<KeepSubject> {
        match target {
            KeepTarget::Row { group, row } => {
                let (peer, lead) = self.taste.peer_row(*group, *row)?;
                lead.filepath.as_ref()?;
                let server = self.taste.peer_name(*group).unwrap_or_default().to_string();
                Some(KeepSubject::of(peer, &server, lead, false))
            }
            KeepTarget::Tile(index) => {
                let album = self.taste.peer_albums().into_iter().nth(*index)?;
                let &(group, row) = album.rows.first()?;
                let (_, lead) = self.taste.peer_row(group, row)?;
                let mut subject = KeepSubject::of(album.peer, &album.server, lead, true);
                // The tile's own spellings: its album and its credit.
                subject.album = Some(album.album.clone());
                subject.album_artist = album.album_artist.clone();
                subject.art = album.art.clone().or(subject.art);
                Some(subject)
            }
            _ => {
                let origin = target.origin()?;
                let base = KeepSubject {
                    peer: origin.peer?,
                    server: self.keep_peer_name(origin),
                    focus: target.focus(),
                    title: None,
                    track_artist: None,
                    album: None,
                    album_artist: None,
                    year: None,
                    duration: None,
                    filepath: None,
                    art: None,
                    folder: None,
                    browse: true,
                    listed_compilation: false,
                    credits_disagree: false,
                    rec: self.keep_rec(target)?,
                };
                let named = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
                Some(match target {
                    KeepTarget::Track { track, album_artist, .. } => {
                        let md = &track.metadata;
                        KeepSubject {
                            title: Some(track.title_or_file().to_string()),
                            track_artist: named(&md.artist),
                            album: named(&md.album),
                            album_artist: album_artist.known().map(str::to_string),
                            credits_disagree: matches!(album_artist, AlbumCredit::Disagree(_)),
                            year: md.year.filter(|y| *y > 0),
                            duration: md.duration,
                            filepath: Some(track.filepath.trim_start_matches('/').to_string()),
                            art: named(&md.album_art),
                            ..base
                        }
                    }
                    KeepTarget::Album { name, album_artist, year, compilation, art, .. } => KeepSubject {
                        track_artist: (!compilation).then(|| album_artist.clone()),
                        album: Some(name.clone()),
                        album_artist: Some(album_artist.clone()),
                        year: *year,
                        art: art.clone(),
                        listed_compilation: *compilation,
                        ..base
                    },
                    KeepTarget::Artist { name, .. } => KeepSubject { title: Some(name.clone()), track_artist: Some(name.clone()), ..base },
                    KeepTarget::Folder { path, name, .. } => KeepSubject {
                        title: Some(name.clone()),
                        folder: Some(FolderRef { path: path.clone(), name: name.clone() }),
                        ..base
                    },
                    KeepTarget::Row { .. } | KeepTarget::Tile(_) => return None,
                })
            }
        }
    }

    /// Pin what `target` names now, for a surface that stays open over a
    /// new answer (the sheet, the tile's list): `None` where it names
    /// nothing a copy could start from. A browse target is its own pin.
    pub fn keep_pin(&self, target: KeepTarget) -> Option<KeepPin> {
        match target {
            KeepTarget::Row { group, row } => {
                let (peer, lead) = self.taste.peer_row(group, row)?;
                Some(KeepPin::Row {
                    peer,
                    filepath: lead.filepath.clone()?,
                    server: self.taste.peer_name(group).unwrap_or_default().to_string(),
                })
            }
            KeepTarget::Tile(index) => {
                let album = self.taste.peer_albums().into_iter().nth(index)?;
                let key = super::taste::peer_album_key(album.peer, &album.album, album.artist.as_deref());
                Some(KeepPin::Tile { key, server: album.server })
            }
            item => {
                let server = self.keep_peer_name(item.origin()?);
                Some(KeepPin::Item { target: Box::new(item), server })
            }
        }
    }

    /// Where the pinned row or album stands in the answer on screen —
    /// the same server's same file, the same server's same album and
    /// credit — or `None` once a new answer left it out: the surface then
    /// offers nothing that would start a copy or ask for links. A browse
    /// pin is its target.
    pub fn keep_find(&self, pin: &KeepPin) -> Option<KeepTarget> {
        match pin {
            KeepPin::Row { peer, filepath, .. } => self.taste.peer_lines().into_iter().find_map(|(group, row)| {
                let (id, lead) = self.taste.peer_row(group, row)?;
                (id == *peer && lead.filepath.as_deref() == Some(filepath.as_str()))
                    .then_some(KeepTarget::Row { group, row })
            }),
            KeepPin::Tile { key, .. } => self
                .taste
                .peer_albums()
                .iter()
                .position(|a| super::taste::peer_album_key(a.peer, &a.album, a.artist.as_deref()) == *key)
                .map(KeepTarget::Tile),
            KeepPin::Item { target, .. } => Some((**target).clone()),
        }
    }

    /// The choices a target's Keep a copy offers at open — empty, and every
    /// entry point to it hidden, while the gate is closed (clauses 101–104;
    /// spec §3.1: its own server's gate, whichever server is browsed —
    /// and a connected server its parent still lists).
    pub fn keep_choices(&self, target: KeepTarget) -> Vec<KeepChoice> {
        let owner = self.keep_target_owner(&target);
        let Some(state) = self.keep_state_for(&owner) else { return Vec::new() };
        if !self.keep_gate_in(state) || target.origin().is_some_and(|origin| !self.keep_peer_listed(origin)) {
            return Vec::new();
        }
        let (Some(listing), Some(subject)) = (state.gate.plugins.as_ref(), self.keep_subject(&target)) else {
            return Vec::new();
        };
        keep_choices(listing, &subject)
    }

    // ── Marks (clauses 113–116) ─────────────────────────────────────────────

    /// A Connected servers row's mark, `None` for none (or no lookup yet).
    pub fn keep_row_mark(&self, group: usize, row: usize) -> Option<JobMark> {
        if !self.keep_listed() {
            return None;
        }
        let jobs = self.keep.rows.get(&(group, row))?;
        let (peer, lead) = self.taste.peer_row(group, row)?;
        row_mark(jobs, peer, lead.album.as_deref())
    }

    /// A Connected servers tile's mark.
    pub fn keep_tile_mark(&self, album: &TastePeerAlbum) -> Option<JobMark> {
        if !self.keep_listed() {
            return None;
        }
        let rows = album.rows.iter().filter_map(|at| self.keep.rows.get(at));
        tile_mark(rows, album.peer, &album.album)
    }

    /// The verbs line's indicator, `None` hidden (clause 116): the copy
    /// gate open and a copy live or failed.
    pub fn keep_indicator(&self) -> Option<JobsIndicator> {
        if !self.keep_gate() {
            return None;
        }
        let rows = self.copies_rows();
        let live = rows.iter().any(|r| r.outcome.live());
        // State failed (a refused song copy among them), or nothing copied
        // and something failed; a stopped copy is done — its row and tile
        // say so.
        let failed = rows
            .iter()
            .any(|r| r.job.state == crate::api::types::JobState::Failed || matches!(r.outcome, JobOutcome::Failed { .. }));
        (live || failed).then_some(JobsIndicator { live, failed })
    }

    // ── A connected server's page's marks (spec §5) ─────────────────────────

    /// Marks show on a connected server's pages while its own server's ping
    /// says plug-ins are on — the lookup route asks nothing else (gap 28):
    /// with the gate closed, earlier copies keep their marks.
    fn keep_marks_shown(&self) -> bool {
        self.connected && self.session.peer.is_some() && self.keep_caps().discovery_plugins
    }

    /// The key a mark is held by — cheap, no target built: `None` where the
    /// row has none to ask by (a card or a page whose credit is not known,
    /// the singles bucket, a library's top).
    fn keep_mark_key_of(&self, ask: &MarkAsk<'_>) -> Option<MarkKey> {
        Some(match ask {
            MarkAsk::Song(track) => MarkKey::Song(track.filepath.trim_start_matches('/').to_string()),
            MarkAsk::Card(card) => {
                let name = card.name.as_deref().filter(|n| !n.trim().is_empty())?;
                let Some(Some(credit)) = &card.album_artist else { return None };
                MarkKey::Album { name: name_key(Some(name)), credit: name_key(Some(credit)), year: card.year.filter(|y| *y > 0) }
            }
            MarkAsk::AlbumPage => match self.library_stack.here() {
                LibraryNode::Album { name, album_artist: AlbumCredit::Known(credit), year, .. } if !name.trim().is_empty() => {
                    MarkKey::Album { name: name_key(Some(name)), credit: name_key(Some(credit)), year: *year }
                }
                _ => return None,
            },
            MarkAsk::ArtistWall => match self.library_stack.here() {
                LibraryNode::Artist(name) if !name.trim().is_empty() => MarkKey::Artist(name_key(Some(name))),
                _ => return None,
            },
            MarkAsk::Folder(path) => MarkKey::Folder(FolderRef::of(path)?.path),
        })
    }

    /// A new row's recommendation, and what its mark is read through: a
    /// song row's credit is the page's where it is one of the album page's
    /// own rows; any other song row is keyed by its own `album-artist`, so a
    /// compilation's songs off its page wear what its album copy did to them
    /// (the webapp keys them so) — the server's song key ignores it, and a
    /// split album's per-row credit names no album copy.
    fn keep_mark_row(&self, page: &MarksPage, ask: &MarkAsk<'_>) -> Option<MarkRow> {
        let (target, of) = match ask {
            MarkAsk::Song(track) => {
                let of = MarkOf::Song { filepath: track.filepath.trim_start_matches('/').to_string(), album: track.metadata.album.clone() };
                (self.keep_track(&self.origin(), track, page.pane())?, of)
            }
            MarkAsk::Card(card) => (self.keep_album_card(card)?, MarkOf::Album(card.name.clone()?)),
            MarkAsk::AlbumPage => {
                let target = self.keep_album_page()?;
                let KeepTarget::Album { name, .. } = &target else { return None };
                let of = MarkOf::Album(name.clone());
                (target, of)
            }
            MarkAsk::ArtistWall => match self.library_stack.here() {
                LibraryNode::Artist(name) => (KeepTarget::Artist { origin: self.origin(), name: name.clone() }, MarkOf::Artist),
                _ => return None,
            },
            MarkAsk::Folder(path) => (self.keep_folder(path)?, MarkOf::Folder),
        };
        let mut rec = self.keep_rec(&target)?;
        if let MarkAsk::Song(track) = ask
            && rec.get("albumArtist").is_none()
            && let Some(credit) = track.metadata.album_artist.as_deref().map(str::trim).filter(|c| !c.is_empty())
            && let Some(fields) = rec.as_object_mut()
        {
            fields.insert("albumArtist".into(), serde_json::Value::String(credit.to_string()));
        }
        Some(MarkRow { rec, of, ..MarkRow::default() })
    }

    /// Ask the marks of a connected server's page's rows on screen (or
    /// near) — spec §5.0, and the user's 2026-10-27 decision on song rows
    /// everywhere: the page's holder starts afresh when the page, the room
    /// or the session is another; a row is asked once, in one lookup of at
    /// most [`LOOKUP_BATCH`] rows in the order they came (the page's own
    /// ask first, then its rows), never a request per row, one lookup out
    /// at a time — the rest wait for the next call; after that only a row
    /// whose copy was live and has ended is asked again, once the page
    /// lists it again. An album page waits while its credit is still asked,
    /// so its rows carry it and its first lookup its own ask. Cheap when
    /// nothing is new: no request, nothing walked. Call it every frame the
    /// page shows.
    pub(crate) fn keep_marks_ask(&mut self, page: &MarksPage, rows: &[MarkAsk<'_>]) -> Vec<Effect> {
        if !self.keep_marks_shown() {
            return Vec::new();
        }
        self.keep_sync();
        let generation = self.session_gen;
        let same = self.keep.marks.page.as_ref().is_some_and(|(have, at)| *at == generation && have.same(page));
        if !same {
            self.keep.marks = BrowseMarks { peer: self.origin().peer.unwrap_or_default(), ..BrowseMarks::default() };
        }
        self.keep.marks.page = Some((page.clone(), generation));
        if matches!(page, MarksPage::Library(LibraryNode::Album { .. })) && self.album_credit_resolving() {
            return Vec::new();
        }
        for ask in rows {
            let Some(key) = self.keep_mark_key_of(ask) else { continue };
            if let Some(row) = self.keep.marks.rows.get_mut(&key) {
                // A row whose copy ended, on the page again: asked again.
                if row.stale {
                    row.stale = false;
                    row.asked = false;
                    self.keep.marks.pending.push_back(key);
                }
                continue;
            }
            let Some(row) = self.keep_mark_row(page, ask) else { continue };
            self.keep.marks.pending.push_back(key.clone());
            self.keep.marks.rows.insert(key, row);
        }
        if self.keep.marks.lookup.is_some() || self.keep.marks.pending.is_empty() {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let marks = &mut self.keep.marks;
        let (mut keys, mut recs) = (Vec::new(), Vec::new());
        while keys.len() < LOOKUP_BATCH
            && let Some(key) = marks.pending.pop_front()
        {
            let Some(row) = marks.rows.get_mut(&key).filter(|row| !row.asked) else { continue };
            row.asked = true;
            recs.push(row.rec.clone());
            keys.push(key);
        }
        if keys.is_empty() {
            return Vec::new();
        }
        let seq = self.keep.next_seq();
        self.keep.marks.lookup = Some((seq, keys));
        vec![Effect::Api(ApiCmd::JobsLookup { seq, generation, reach, recs })]
    }

    /// The marks of the page on screen, while its holder is that page's.
    fn keep_marks_here(&self) -> Option<&BrowseMarks> {
        let marks = &self.keep.marks;
        (self.keep_marks_shown() && marks.page.as_ref().is_some_and(|(_, generation)| *generation == self.session_gen)).then_some(marks)
    }

    /// The page on screen holds a copy still moving — its marks' poll.
    pub fn keep_marks_live(&self) -> bool {
        self.keep_marks_here().is_some_and(BrowseMarks::live)
    }

    /// Bumped whenever rows of the page went stale: the page lists its rows
    /// again, so those on screen (or near) are asked again.
    pub fn keep_marks_stirred(&self) -> u64 {
        self.keep.marks.stirred
    }

    /// How many marks the page's holder worked out so far.
    #[cfg(test)]
    pub(crate) fn keep_marks_worked(&self) -> usize {
        self.keep.marks.worked
    }

    /// No page asked its marks this frame (another room, another screen):
    /// its holder goes, and with it the poll its live marks kept (spec
    /// §5.0: the page, the room or the session changed).
    pub(crate) fn keep_marks_drop(&mut self) {
        if self.keep.marks.page.is_some() {
            self.keep.marks = BrowseMarks::default();
        }
    }

    /// A song row's mark on a connected server's page ([`song_mark`]):
    /// `None` for none, or before its lookup answered.
    pub fn keep_song_mark(&self, track: &Track) -> Option<JobMark> {
        self.keep_marks_here()?.mark(&MarkKey::Song(track.filepath.trim_start_matches('/').to_string()))
    }

    /// A wall card's mark, in its year line ([`card_mark`]).
    pub fn keep_card_mark(&self, card: &Album) -> Option<JobMark> {
        self.keep_marks_here()?.mark(&self.keep_mark_key_of(&MarkAsk::Card(card))?)
    }

    /// A folder row's mark ([`folder_mark`]): its folder copies alone.
    pub fn keep_folder_mark(&self, path: &str) -> Option<JobMark> {
        self.keep_marks_here()?.mark(&MarkKey::Folder(FolderRef::of(path)?.path))
    }

    /// The page's own copy's state, for the spot its header word stands in
    /// (spec §5.1): the album page's ([`card_mark`]), the artist's wall's
    /// ([`artist_mark`]), the open folder's ([`folder_mark`]) — with the
    /// gate open or closed; `None` for none.
    pub fn keep_page_state(&self) -> Option<JobMark> {
        let marks = self.keep_marks_here()?;
        match &marks.page.as_ref()?.0 {
            MarksPage::Library(LibraryNode::Album { .. }) => marks.mark(&self.keep_mark_key_of(&MarkAsk::AlbumPage)?),
            MarksPage::Library(LibraryNode::Artist(_)) => marks.mark(&self.keep_mark_key_of(&MarkAsk::ArtistWall)?),
            MarksPage::Files(path) => self.keep_folder_mark(path),
            _ => None,
        }
    }

    // ── The album page's credit (spec §4.1.3; the user's 2026-10-27 Q7) ─────

    /// An album page's tracks landed: its credit, where its opener did not
    /// know it, is found — step 2, the songs' own `album-artist`, at once;
    /// on a connected server, step 3, one ask of its albums by the first
    /// song's artist — and stored on the page's node, so Back and Keep a
    /// copy reuse it. Unknown stands where neither says.
    pub(crate) fn album_credit_landed(&mut self, node: &LibraryNode, tracks: &[Track]) -> Vec<Effect> {
        let LibraryNode::Album { name, album_artist: AlbumCredit::Unknown, .. } = node else { return Vec::new() };
        if name.trim().is_empty() || !self.library_stack.wants(node) {
            return Vec::new();
        }
        if let Some(credit) = credit_of_songs(tracks.iter().map(|t| t.metadata.album_artist.as_deref())) {
            self.album_credit_store(node, credit, false);
            return Vec::new();
        }
        let artist = tracks.iter().find_map(|t| t.metadata.artist.clone().filter(|a| !a.trim().is_empty()));
        let (Some(artist), Some(_)) = (artist, self.session.peer.as_ref()) else { return Vec::new() };
        let seq = self.keep.next_seq();
        self.keep_shelf.credit = Some(CreditAsk { seq, generation: self.session_gen, node: node.clone(), since: crate::clock::Instant::now() });
        vec![Effect::Api(ApiCmd::AlbumCredit { seq, generation: self.session_gen, artist })]
    }

    /// Step 3 answered: the one album of the page's name and year among
    /// the artist's — its credit, or that its rows disagree; anything
    /// else, unknown. Nothing once the page is another, or past its wait.
    pub(super) fn album_credit_answered(&mut self, seq: u64, generation: u64, rows: Option<Vec<ArtistAlbumRow>>) -> Vec<Effect> {
        let Some(ask) = self.keep_shelf.credit.take_if(|ask| ask.seq == seq && ask.generation == generation) else {
            return Vec::new();
        };
        if let (Some(rows), LibraryNode::Album { name, year, .. }) = (rows, &ask.node) {
            let (credit, compilation) = credit_of_rows(&rows, name, *year);
            self.album_credit_store(&ask.node, credit, compilation);
        }
        Vec::new()
    }

    /// The credit found, on the page's node — while the page is still it.
    fn album_credit_store(&mut self, node: &LibraryNode, credit: AlbumCredit, listed: bool) {
        if !self.library_stack.wants(node) {
            return;
        }
        if let Some(LibraryNode::Album { album_artist, compilation, .. }) = self.library_stack.here_mut() {
            *compilation |= listed || various_artists(credit.known());
            *album_artist = credit;
        }
    }

    /// The album page's credit is still being asked of its server.
    pub fn album_credit_resolving(&self) -> bool {
        self.keep_shelf.credit.as_ref().is_some_and(|ask| self.library_stack.wants(&ask.node))
    }

    /// The album page on screen's credit, as it draws beside the album's
    /// name (the user's 2026-10-27 decision): its credit, or the artists its
    /// rows credit joined ", " where they disagree; nothing while it is
    /// asked or not known.
    pub fn album_credit_words(&self) -> Option<String> {
        match self.library_stack.here() {
            LibraryNode::Album { album_artist, .. } => album_artist.words(),
            _ => None,
        }
    }

    // ── Lookups and the poll (clauses 111–112) ──────────────────────────────

    /// Look the Connected servers rows up (clause 111): after every peers
    /// answer, when the gate opens over one, after a copy ends and after a
    /// clear. Each row's raw JSON goes unchanged; the rows a new answer
    /// replaced lose their jobs at once.
    pub(crate) fn keep_lookup_rows(&mut self) -> Vec<Effect> {
        let landed = self.keep.peers_landed;
        // The room is the session's own server's: never a parked state's.
        if !self.keep_listed() || !self.keep_is_session() {
            return Vec::new();
        }
        let lines = self.taste.peer_lines();
        let recs: Vec<serde_json::Value> = lines
            .iter()
            .filter_map(|&(g, r)| self.taste.peer_row(g, r).map(|(_, lead)| lead.raw.clone()))
            .collect();
        if recs.is_empty() || recs.len() != lines.len() {
            self.keep.rows.clear();
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        self.keep.lookup = Some(RowsLookup { seq, rows: lines, landed });
        vec![Effect::Api(ApiCmd::JobsLookup { seq, generation, reach, recs })]
    }

    /// The Discover room's answer went (a server switch, clause 31): its
    /// rows' marks go with it, and a lookup out for them fills nothing.
    pub(super) fn keep_rows_reset(&mut self) {
        let keep = &mut self.keep;
        keep.peers_landed += 1;
        keep.rows.clear();
        keep.lookup = None;
    }

    /// A peers answer landed (clause 111): the rows are other songs now —
    /// their jobs go at once, and the new rows are looked up.
    pub(crate) fn keep_peers_landed(&mut self) -> Vec<Effect> {
        let keep = &mut self.keep;
        keep.peers_landed += 1;
        keep.rows.clear();
        keep.lookup = None;
        self.keep_lookup_rows()
    }

    /// Ask the caller's jobs now (the Copies modal's first ask, a refresh
    /// after a cancel lost its race or a clear, the poll) — one ask at a
    /// time; where copies run, or while a page's mark is live with the gate
    /// closed (its marks need only the flag, gap 28).
    fn keep_ask_jobs(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        let marked = self.keep_marks_shown() && self.keep.marks.live();
        if !(self.keep_listed() || marked) || self.keep.book.asking.is_some() {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        let book = &mut self.keep.book;
        book.asking = Some(seq);
        book.last_ask = Some(now);
        vec![Effect::Api(ApiCmd::Jobs { seq, generation, reach })]
    }

    /// What the GUI shows of the copies — the poll's beat (clause 112).
    pub(crate) fn keep_watch(&mut self, watch: KeepWatch) {
        self.keep.watch = watch;
    }

    /// The open Keep a copy modal shows a copy of its own moving — its
    /// start out, the job it follows live, a live job in one of its
    /// subject's slots: the modal's quick beat holds only then (clause
    /// 112); a modal still choosing over someone else's copy keeps the
    /// room's.
    pub fn keep_flow_live(&self) -> bool {
        self.keep.flow_live()
    }

    /// Once a tick: the jobs poll, only while a copy is live — every 1.5 s
    /// with a Keep or Copies modal open, every 4 s while a room shows a
    /// copy's state, never otherwise — backing off, doubling up to a
    /// minute, on failures; a folder window's "By your layout" tags, the
    /// first time that landing is chosen; and the album page's credit ask
    /// given up after its wait.
    pub(crate) fn keep_tick(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        if self.keep_shelf.credit.as_ref().is_some_and(|ask| now.saturating_duration_since(ask.since) >= CREDIT_WAIT) {
            self.keep_shelf.credit = None;
        }
        if !self.connected {
            return Vec::new();
        }
        self.keep_sync();
        let mut effects = self.keep_folder_tags();
        if !(self.keep_listed() || (self.keep_marks_shown() && self.keep.marks.live())) {
            return effects;
        }
        let keep = &self.keep;
        let beat = match keep.watch {
            KeepWatch::Away => return effects,
            KeepWatch::Room => KEEP_POLL_ROOM,
            KeepWatch::Modal => KEEP_POLL_MODAL,
        };
        if keep.book.asking.is_some() || !keep.any_live() {
            return effects;
        }
        let wait = keep_backoff(beat, keep.book.failures);
        if keep.book.last_ask.is_some_and(|at| now.saturating_duration_since(at) < wait) {
            return effects;
        }
        effects.extend(self.keep_ask_jobs(now));
        effects
    }

    /// "By your layout" chosen in a folder window: its songs' tags are read
    /// once (spec §6.5.3) — at most 100 songs, once the folder is listed;
    /// past 100, or with no listing, never (its variables stay slots).
    fn keep_folder_tags(&mut self) -> Vec<Effect> {
        let Some(flow) = self.keep.flow.as_ref() else { return Vec::new() };
        if flow.choice != KeepChoice::Folder(FolderLanding::Tags) || flow.tags_seq.is_some() {
            return Vec::new();
        }
        let Some(songs) = flow.folder_songs().filter(|songs| !songs.is_empty() && songs.len() <= TAGS_MAX) else {
            return Vec::new();
        };
        let filepaths: Vec<String> = songs.iter().map(|song| song.filepath.clone()).collect();
        let origin = Origin { server: self.keep_home().server, peer: Some(flow.subject.peer) };
        let Ok(reach) = self.reach(&origin) else { return Vec::new() };
        let generation = self.keep.epoch;
        let seq = self.keep.next_seq();
        if let Some(flow) = self.keep.flow.as_mut() {
            flow.tags_seq = Some(seq);
        }
        vec![Effect::Api(ApiCmd::KeepFolderTags { seq, generation, reach: Some(reach), filepaths })]
    }

    /// How long until the poll tries again after a failure — the Copies
    /// modal's "next try in 8 s" — `None` while nothing failed.
    pub fn keep_next_try(&self, now: crate::clock::Instant) -> Option<Duration> {
        let keep = &self.keep;
        keep.book.failed.as_ref()?;
        let beat = if keep.watch == KeepWatch::Modal { KEEP_POLL_MODAL } else { KEEP_POLL_ROOM };
        let wait = keep_backoff(beat, keep.book.failures);
        let since = keep.book.last_ask.map(|at| now.saturating_duration_since(at)).unwrap_or_default();
        Some(wait.saturating_sub(since))
    }

    /// The jobs seen live and finished since the last call — the status
    /// line's notes (clause 118), for the GUI to word.
    pub(crate) fn keep_take_news(&mut self) -> Vec<PluginJob> {
        std::mem::take(&mut self.keep.news)
    }

    /// The starts a closed window sent that answered since — the job, or
    /// the refusal — for the status line (clause 118), taken once.
    pub(crate) fn keep_take_landed(&mut self) -> Vec<Result<PluginJob, PluginFailure>> {
        std::mem::take(&mut self.keep.landed)
    }

    /// The Copies modal's and the room's one-off note, taken.
    pub(crate) fn keep_take_note(&mut self) -> Option<KeepNote> {
        self.keep.note.take()
    }

    // ── The Copies list (clause 117) ────────────────────────────────────────

    /// The Copies modal opened — `C` in any room while the gate is open,
    /// on the user's own server or one of its connected servers (the user's
    /// 2026-10-27 decision; [`App::keep_copies_ready`]): its first ask,
    /// now. While it shows, the GUI holds [`KeepWatch::Modal`], and the
    /// poll runs at its beat from whichever room it is open over.
    pub(crate) fn copies_open(&mut self) -> Vec<Effect> {
        self.taste_sync();
        self.keep_sync();
        self.keep.note = None;
        self.keep_ask_jobs(crate::clock::Instant::now())
    }

    /// Whether `C` opens the Copies list where the user is: the gate of the
    /// own server in front, open — whichever room, whichever of its servers
    /// is browsed.
    pub fn keep_copies_ready(&self) -> bool {
        self.keep_gate()
    }

    /// Ask again now — the error line's Try again.
    pub(crate) fn copies_try_again(&mut self) -> Vec<Effect> {
        self.keep_ask_jobs(crate::clock::Instant::now())
    }

    /// The Copies list: running, then waiting in slot order, then the GOLD
    /// rows (failed, stopped, refused), then the finished; newest first in
    /// each. A finished job a newer job of the same plug-in, key and server
    /// supersedes leaves (a retried failure is old news); the same album
    /// from two servers is two rows.
    pub fn copies_rows(&self) -> Vec<CopyRow> {
        let book = &self.keep.book;
        let jobs: Vec<&PluginJob> = book.jobs.values().filter(|j| listed_copy(j)).collect();
        let superseded = |job: &PluginJob| {
            !job.live()
                && job.key.is_some()
                && jobs.iter().any(|other| {
                    other.id > job.id && other.key == job.key && other.peer_id() == job.peer_id() && other.scope() == job.scope()
                })
        };
        let mut rows: Vec<CopyRow> = jobs
            .iter()
            .filter(|job| !superseded(job))
            .map(|job| {
                let outcome = job.outcome();
                CopyRow {
                    title: CopyTitle::of(job),
                    from: job.peer_name(),
                    action: copies_action(&outcome),
                    cancelling: book.cancelling.contains(&job.id),
                    outcome,
                    job: (*job).clone(),
                }
            })
            .collect();
        let class = |row: &CopyRow| match &row.outcome {
            JobOutcome::Running { .. } => 0,
            JobOutcome::Queued => 1,
            o if o.troubled() => 2,
            _ => 3,
        };
        rows.sort_by(|a, b| {
            class(a).cmp(&class(b)).then_with(|| {
                if class(a) == 1 { a.job.id.cmp(&b.job.id) } else { b.job.id.cmp(&a.job.id) }
            })
        });
        rows
    }

    /// Whether Clear finished stands: a finished row is listed.
    pub fn copies_clearable(&self) -> bool {
        self.copies_rows().iter().any(|r| r.job.state.finished())
    }

    /// Cancel a copy — the Copies row's `[X]` or `x`, the Keep modal's
    /// Cancel copy (clause 109). A 409 is the server's word that it had
    /// already finished.
    pub(crate) fn keep_cancel_job(&mut self, id: i64) -> Vec<Effect> {
        if !self.keep_listed() || self.keep.book.cancelling.contains(&id) {
            return Vec::new();
        }
        if !self.keep.job(id).is_some_and(PluginJob::live) {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        self.keep.book.cancelling.insert(id);
        vec![Effect::Api(ApiCmd::JobCancel { generation, reach, id })]
    }

    /// Clear finished: the server drops the caller's finished jobs.
    pub(crate) fn copies_clear(&mut self) -> Vec<Effect> {
        if !self.keep_listed() {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        vec![Effect::Api(ApiCmd::JobsClear { generation, reach })]
    }

    /// A Copies row's action: Retry, Copy again and Start again start the
    /// same copy again (an album or artist copy takes only the gaps) where
    /// it ran — the list shows no place, so one moved since is the server's
    /// 409 to say, and Keep a copy's to show (clause 146); Play your copy
    /// plays it; Cancel cancels.
    pub(crate) fn copies_act(&mut self, id: i64) -> Vec<Effect> {
        let Some(job) = self.keep.job(id).cloned() else { return Vec::new() };
        match copies_action(&job.outcome()) {
            Some(JobAction::Cancel) => self.keep_cancel_job(id),
            Some(JobAction::Play) => self.keep_play_copy(&job, true),
            Some(JobAction::Retry | JobAction::CopyAgain | JobAction::StartAgain) => {
                let place = self.keep_ran_with(&job);
                let (effects, ticket) = self.keep_restart(&job, place);
                if let Some(ticket) = ticket {
                    self.keep.book.starts.insert(ticket);
                }
                effects
            }
            None => Vec::new(),
        }
    }

    /// Start `job`'s copy again, as it was started, sending `destination`
    /// as the place shown (clause 145). A job the other server failed
    /// because it is no longer paired asks the room again instead — the
    /// rows carry its new id then (clause 128).
    fn keep_restart(
        &mut self,
        job: &PluginJob,
        destination: Option<Option<CollectionDestination>>,
    ) -> (Vec<Effect>, Option<u64>) {
        if job.unpaired() {
            return (self.taste_ask_again(), None);
        }
        let Some(scope) = job.scope() else { return (Vec::new(), None) };
        // A folder copy again where it landed: its own landing (spec §6.7).
        let landing = (scope == JobScope::Folder).then(|| job.params.landing.clone());
        let (effect, ticket) = self.keep_start_job(job.recommendation.clone(), scope, destination, landing);
        (effect.into_iter().collect(), ticket)
    }

    /// The place a copy ran with, as a restart from the Copies list sends
    /// it: its pin; a job from before the pin, the place its result names;
    /// else the account's place as last read — never no place at all, so
    /// the server checks it (clause 145).
    fn keep_ran_with(&self, job: &PluginJob) -> Option<Option<CollectionDestination>> {
        job.params
            .destination
            .clone()
            .or_else(|| job.copy_result().destination.map(Some))
            .or_else(|| Some(self.keep_destination().cloned()))
    }

    /// One start: the recommendation as given, the scope, the place shown
    /// (clause 145), a folder copy's landing, a ticket.
    fn keep_start_job(
        &mut self,
        rec: serde_json::Value,
        scope: JobScope,
        destination: Option<Option<CollectionDestination>>,
        landing: Option<JobLanding>,
    ) -> (Option<Effect>, Option<u64>) {
        if !self.keep_gate() || rec.is_null() {
            return (None, None);
        }
        let Some((generation, reach)) = self.keep_cmd() else { return (None, None) };
        let ticket = self.keep.next_seq();
        let effect = Effect::Api(ApiCmd::JobStart {
            ticket,
            generation,
            reach,
            plugin: COPY_PLUGIN.to_string(),
            rec: Box::new(rec),
            scope,
            destination,
            landing: landing.filter(|_| scope == JobScope::Folder),
        });
        (Some(effect), Some(ticket))
    }

    /// Play (or queue) a copied song from the user's own library (clause
    /// 110's Play your copy and Add to queue), with the session's origin —
    /// `job` as the surface shows it: a song copy, or a wider copy's account
    /// of this song ([`covered_view`]). An album copy — its own, or a wider
    /// copy's account of the album — plays the whole album from the
    /// library: what it copied and what it found already there.
    pub(crate) fn keep_play_copy(&mut self, job: &PluginJob, play: bool) -> Vec<Effect> {
        if matches!(job.outcome(), JobOutcome::CopiedMany(_)) && matches!(job.scope(), Some(JobScope::Album | JobScope::Folder)) {
            return self.keep_play_album(job, play);
        }
        let JobOutcome::Copied { filepath, .. } = job.outcome() else { return Vec::new() };
        let copied = job.copy_result().copied.unwrap_or_default();
        let track = Track {
            filepath,
            metadata: crate::api::types::TrackMetadata {
                title: copied.title.or_else(|| job.rec_text("title")),
                artist: copied.artist.or_else(|| job.rec_text("artist")),
                album: copied.album.or_else(|| job.rec_text("album")),
                ..Default::default()
            },
        };
        let origin = self.keep_home();
        if play { self.queue_track_next(&origin, track, true) } else { self.queue_track_end(&origin, track) }
    }

    /// Play your copy after an album or a folder: its songs in the user's
    /// library ([`album_tracks`]) — an album's in its listing's order, a
    /// folder's in path order (spec §6.7) — right after the playing row, at
    /// the end when nothing plays, the first started; queued (`play`
    /// false), appended. Nothing where the result names no files: the
    /// window offers Close then ([`window_action`]).
    fn keep_play_album(&mut self, job: &PluginJob, play: bool) -> Vec<Effect> {
        let listing: Option<Vec<KeepSong>> = match job.scope() {
            Some(JobScope::Folder) => {
                let paths: Vec<String> = match self.keep.flow.as_ref().and_then(KeepFlow::folder_songs) {
                    Some(songs) => songs.iter().map(|song| song.filepath.clone()).collect(),
                    None => {
                        let mut from: Vec<String> = job.copy_result().kept_songs().into_iter().filter_map(|song| song.from).collect();
                        from.sort_by(|a, b| natural_order(a, b));
                        from
                    }
                };
                Some(paths.into_iter().map(|filepath| KeepSong { filepath, ..KeepSong::default() }).collect())
            }
            _ => self.keep.flow.as_ref().and_then(|f| f.facts.album_songs.clone()),
        };
        let tracks = album_tracks(job, listing.as_deref());
        if tracks.is_empty() {
            return Vec::new();
        }
        let count = tracks.len();
        let origin = self.keep_home();
        let mut items = tracks.into_iter().map(|track| Queued { dj: None, origin: origin.clone(), track });
        if !play {
            let was_empty = self.queue.items.is_empty();
            for item in items {
                self.queue.push(item);
            }
            self.info(format!("queued {count}"));
            return if was_empty && self.status.is_idle() { self.play_index(0) } else { Vec::new() };
        }
        let Some(first) = items.next() else { return Vec::new() };
        let at = self.queue.insert_next(first);
        for (i, item) in items.enumerate() {
            self.queue.items.insert(at + 1 + i, item);
        }
        // Whatever was announced as next has just been cut in front of.
        self.announced = None;
        self.play_index(at)
    }

    // ── The Keep a copy flow (clauses 106–110) ──────────────────────────────

    /// Open Keep a copy on a target — a Discover row or tile (`c`, the
    /// sheet's row, the tile's list, clauses 102–103), or a connected
    /// server's song, album, artist or folder (spec §4: a header word, a
    /// held row or card, a sheet's row). Nothing opens while the gate of its
    /// own server is closed or the target offers no choice. The sizes are
    /// asked now — of the connected server through its reach, of the user's
    /// own library through the own server's — and an untagged song's tags;
    /// the flow follows its scope's job at once when one of this server's
    /// stands (clause 107).
    pub(crate) fn keep_open(&mut self, target: KeepTarget) -> Vec<Effect> {
        self.taste_sync();
        self.keep_sync();
        // A queue item's own server's state, whichever is browsed (spec
        // §4.8): in front while its window is open.
        let owner = self.keep_target_owner(&target);
        let choices = self.keep_choices(target.clone());
        let Some(subject) = self.keep_subject(&target).filter(|_| !choices.is_empty()) else { return Vec::new() };
        // One window at a time: one open on another own server's state never
        // waits parked with it.
        if self.keep.flow.is_some() {
            self.keep_close_quietly();
        }
        self.keep_focus(&owner);
        // The jobs its rows already hold, merged newest per slot: a
        // Discover row's or tile's; a browse target's own row on its page.
        let rows: Vec<RowJobs> = match &target {
            KeepTarget::Row { group, row } => self.keep.rows.get(&(*group, *row)).cloned().into_iter().collect(),
            KeepTarget::Tile(index) => {
                let at = self.taste.peer_albums().into_iter().nth(*index).map(|a| a.rows).unwrap_or_default();
                at.iter().filter_map(|at| self.keep.rows.get(at).cloned()).collect()
            }
            browse => self.keep_mark_key(browse).and_then(|key| self.keep.marks.jobs(&key).cloned()).into_iter().collect(),
        };
        let folders = subject.focus == KeepFocus::Folder;
        let untagged = matches!(&target, KeepTarget::Track { track, .. } if track.metadata.display_title().is_none());
        let target_again = target.clone();
        let mut jobs = RowJobs { folders, ..RowJobs::default() };
        let looked_up = !rows.is_empty();
        for row in &rows {
            for key in &row.keys {
                if !jobs.keys.contains(key) {
                    jobs.keys.push(key.clone());
                }
            }
            for job in row.held.values() {
                jobs.merge(job);
            }
        }
        let choice = match subject.focus {
            KeepFocus::Song => KeepChoice::Song,
            KeepFocus::Album => KeepChoice::Album,
            KeepFocus::Artist => KeepChoice::Artist,
            KeepFocus::Folder => KeepChoice::Folder(FolderLanding::Mirror),
        };
        let choice = if choices.contains(&choice) { choice } else { choices[0] };
        let peer = subject.peer;
        let phase = opening_phase(&jobs, &subject, choice.scope(false)).unwrap_or(KeepPhase::Choose);
        let mut effects = Vec::new();
        let facts_seq = self.keep.next_seq();
        let origin = Origin { server: self.keep_home().server, peer: Some(peer) };
        let generation = self.keep.epoch;
        // An artist's wall's own listing is the artist's albums there: not
        // asked again (spec §6.1).
        let mut facts = KeepFacts::default();
        if subject.focus == KeepFocus::Artist
            && let Some(artist) = subject.track_artist.as_deref()
        {
            facts.peer_albums = self.keep_wall_rows(artist);
        }
        let ask = KeepFactsAsk {
            album: subject.album.clone(),
            artist: subject.track_artist.clone(),
            album_artist: subject.album_artist.clone(),
            year: subject.year,
            whole: subject.whole_album(),
            listed: facts.peer_albums.is_some(),
            folder: subject.folder.as_ref().map(|f| f.path.clone()),
        };
        let own = self.keep_cmd().and_then(|(_, reach)| reach);
        effects.push(Effect::Api(ApiCmd::KeepFacts {
            seq: facts_seq,
            generation,
            reach: self.reach(&origin).ok(),
            own,
            ask: Box::new(ask),
        }));
        let lookup_seq = if looked_up {
            None
        } else {
            match self.keep_cmd() {
                Some((generation, reach)) => {
                    let seq = self.keep.next_seq();
                    effects.push(Effect::Api(ApiCmd::JobsLookup { seq, generation, reach, recs: vec![subject.rec.clone()] }));
                    Some(seq)
                }
                None => None,
            }
        };
        // "Into a folder…" starts on the saved library, in a new folder
        // named as the pressed one, made safe (the webapp's
        // `folderIntoDefault`).
        let into = IntoPlace {
            vpath: self.keep_destination().map(|d| d.vpath.clone()).unwrap_or_default(),
            path: subject.folder.as_ref().map(|f| sanitize_segment(Some(&f.name))).unwrap_or_default(),
        };
        self.keep.flow = Some(KeepFlow {
            target,
            subject,
            choices,
            choice,
            // Ticked from the start, as on the webapp and the phone: it
            // counts once Every album by is chosen and the box is offered.
            only_missing: true,
            facts,
            facts_asking: true,
            facts_seq,
            songs_seq: None,
            lookup_seq,
            jobs,
            phase,
            sent: None,
            into,
            folder_tags: None,
            tags_seq: None,
            folder_plan: None,
            untagged,
        });
        // An untagged song's tags, from its own server (spec §6.3).
        if let (true, KeepTarget::Track { origin, track, .. }) = (untagged, &target_again) {
            effects.extend(self.fetch_track_info(origin, &track.filepath));
        }
        // Where copies land, read again with every open: the last place
        // known draws meanwhile (clause 105).
        effects.extend(self.keep_dest_read());
        effects
    }

    /// A page's word pressed after its gate closed during the visit (spec
    /// §3.2: the word stays for the visit): the window on its one plain line
    /// — the first part of the gate that no longer holds (R3–R5) — and
    /// Close; nothing is asked, nothing can start.
    pub(crate) fn keep_open_off(&mut self, target: KeepTarget) {
        self.keep_sync();
        let Some(subject) = self.keep_subject(&target) else { return };
        if self.keep.flow.is_some() {
            self.keep_close_quietly();
        }
        let owner = self.keep_target_owner(&target);
        self.keep_focus(&owner);
        let choice = match subject.focus {
            KeepFocus::Song => KeepChoice::Song,
            KeepFocus::Album => KeepChoice::Album,
            KeepFocus::Artist => KeepChoice::Artist,
            KeepFocus::Folder => KeepChoice::Folder(FolderLanding::Mirror),
        };
        // A connected server its parent no longer lists leaves no part of
        // the gate to name: copies from it are off.
        let why = self.keep_lost(choice.scope(false)).unwrap_or(OffWhy::Plugin);
        let folders = subject.focus == KeepFocus::Folder;
        self.keep.flow = Some(KeepFlow {
            target,
            subject,
            choices: vec![choice],
            choice,
            only_missing: true,
            facts: KeepFacts::default(),
            facts_asking: false,
            facts_seq: 0,
            songs_seq: None,
            lookup_seq: None,
            jobs: RowJobs { folders, ..RowJobs::default() },
            phase: KeepPhase::Off(why),
            sent: None,
            into: IntoPlace::default(),
            folder_tags: None,
            tags_seq: None,
            folder_plan: None,
            untagged: false,
        });
    }

    /// A song's full block landed (`Event::TrackInfo`): an untagged song's
    /// window learns its tags — its title and byline, its recommendation —
    /// and asks its facts again, so its choices append (spec §6.3).
    pub(crate) fn keep_track_info_landed(&mut self, track: &Track) -> Vec<Effect> {
        let home = self.keep_home().server;
        let Some(flow) = self.keep.flow.as_mut().filter(|f| f.untagged && f.subject.filepath.as_deref() == Some(track.filepath.trim_start_matches('/'))) else {
            return Vec::new();
        };
        flow.untagged = false;
        let md = &track.metadata;
        let named = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        let subject = &mut flow.subject;
        if let Some(title) = named(&md.title) {
            subject.title = Some(title);
        }
        subject.track_artist = named(&md.artist).or(subject.track_artist.take());
        subject.album = named(&md.album).or(subject.album.take());
        subject.year = md.year.filter(|y| *y > 0).or(subject.year);
        subject.duration = md.duration.or(subject.duration);
        subject.art = named(&md.album_art).or(subject.art.take());
        let rec_track = Track { filepath: track.filepath.clone(), metadata: md.clone() };
        let origin = Origin { server: home, peer: Some(flow.subject.peer) };
        let target = KeepTarget::Track { origin: origin.clone(), track: Box::new(rec_track), album_artist: AlbumCredit::Unknown };
        if let (Some(rec), Some(flow)) = (self.keep_rec(&target), self.keep.flow.as_mut()) {
            flow.subject.rec = rec;
        }
        let Some(flow) = self.keep.flow.as_ref() else { return Vec::new() };
        let ask = KeepFactsAsk {
            album: flow.subject.album.clone(),
            artist: flow.subject.track_artist.clone(),
            album_artist: flow.subject.album_artist.clone(),
            year: flow.subject.year,
            whole: flow.subject.whole_album(),
            ..KeepFactsAsk::default()
        };
        let generation = self.keep.epoch;
        let own = self.keep_cmd().and_then(|(_, reach)| reach);
        let reach = self.reach(&origin).ok();
        let seq = self.keep.next_seq();
        if let Some(flow) = self.keep.flow.as_mut() {
            flow.facts_seq = seq;
            flow.facts_asking = true;
        }
        vec![Effect::Api(ApiCmd::KeepFacts { seq, generation, reach, own, ask: Box::new(ask) })]
    }

    /// A browse target's row on its page's marks, by its key.
    fn keep_mark_key(&self, target: &KeepTarget) -> Option<MarkKey> {
        match target {
            KeepTarget::Track { track, .. } => Some(MarkKey::Song(track.filepath.trim_start_matches('/').to_string())),
            KeepTarget::Album { .. } => album_key(target),
            KeepTarget::Artist { name, .. } => Some(MarkKey::Artist(name_key(Some(name)))),
            KeepTarget::Folder { path, .. } => Some(MarkKey::Folder(path.clone())),
            KeepTarget::Row { .. } | KeepTarget::Tile(_) => None,
        }
    }

    /// The open flow's cover, asked of its own server through the parent's
    /// art proxy (the strip's way) — nothing when it names none, the cache
    /// holds or awaits it, or the server cannot be reached.
    pub(crate) fn keep_cover(&mut self) -> Vec<Effect> {
        let Some(flow) = self.keep.flow.as_ref() else { return Vec::new() };
        let Some(file) = flow.subject.art.clone() else { return Vec::new() };
        if self.art.contains_key(&file) {
            return Vec::new();
        }
        let origin = Origin { server: self.keep_home().server, peer: Some(flow.subject.peer) };
        let Ok(reach) = self.reach(&origin) else { return Vec::new() };
        self.claim_art(&file, Some(reach))
    }

    /// The Keep modal closed (Esc, "Esc close", Close): the copy keeps
    /// running on the server (clause 109); the picker's folders go with it.
    /// A start still out lands all the same, and the status line says how
    /// it went (clause 118) — the window that would have is gone.
    /// A browse window's own row on its page is asked again at once (spec
    /// §5.0), and the session's own server's state comes back in front
    /// where a queue item's window had brought another's (spec §4.8).
    pub(crate) fn keep_close(&mut self) {
        if let Some(flow) = self.keep.flow.take() {
            if let KeepPhase::Starting { ticket } = flow.phase {
                self.keep.book.unseen.insert(ticket);
            }
            if let Some(key) = self.keep_mark_key(&flow.target) {
                self.keep.marks.again(&key);
            }
        }
        self.keep_dest_forget();
        self.keep_sync();
    }

    /// Pick a radio (Space, Enter, a click). "Only the albums you don't
    /// have" keeps its own tick: it counts only under Every album by.
    pub(crate) fn keep_pick(&mut self, choice: KeepChoice) {
        let Some(flow) = self.keep.flow.as_mut() else { return };
        if !flow.choices.contains(&choice) || !matches!(flow.phase, KeepPhase::Choose | KeepPhase::Refused(_)) {
            return;
        }
        flow.choice = choice;
    }

    /// "Into a folder…"'s place, as its chooser leaves it (spec §6.6): the
    /// library and the folder in it (`""` its top), for this copy only —
    /// nothing is saved. Kept whatever is chosen; it counts under Into a
    /// folder… alone.
    pub(crate) fn keep_set_into(&mut self, vpath: &str, path: &str) {
        if let Some(flow) = self.keep.flow.as_mut() {
            flow.into = IntoPlace { vpath: vpath.to_string(), path: path.to_string() };
        }
    }

    /// Tick or untick "only the albums you don't have" — ticking it picks
    /// Every album by (clause 107). Nothing while it is hidden.
    pub(crate) fn keep_set_missing(&mut self, on: bool) {
        let shown = self.keep_view().is_some_and(|v| v.missing.is_some());
        let Some(flow) = self.keep.flow.as_mut() else { return };
        if !shown || !matches!(flow.phase, KeepPhase::Choose | KeepPhase::Refused(_)) {
            return;
        }
        flow.only_missing = on;
        if on {
            flow.choice = KeepChoice::Artist;
        }
    }

    /// The primary: start the chosen copy (clause 108). Nothing while the
    /// primary is hidden — a start out, another server's copy covering it.
    pub(crate) fn keep_start(&mut self) -> Vec<Effect> {
        let Some(view) = self.keep_view() else { return Vec::new() };
        let verb = match &view.stage {
            KeepStage::Choose { verb, .. } | KeepStage::Refused { verb, .. } => *verb,
            _ => None,
        };
        if verb.is_none() {
            return Vec::new();
        }
        // The place LANDS IN drew: one no longer in force is the server's
        // 409, and nothing starts (clause 145).
        let shown = Some(view.destination.clone());
        let (effect, ticket) = self.keep_start_job(view.subject.rec_for(view.scope), view.scope, shown.clone(), view.job_landing());
        let Some(ticket) = ticket else { return Vec::new() };
        if let Some(flow) = self.keep.flow.as_mut() {
            flow.phase = KeepPhase::Starting { ticket };
            flow.sent = shown;
        }
        effect.into_iter().collect()
    }

    /// `x` or Cancel copy: cancel the followed copy while it is live.
    pub(crate) fn keep_cancel(&mut self) -> Vec<Effect> {
        let Some(KeepPhase::Following(id)) = self.keep.flow.as_ref().map(|f| f.phase.clone()) else {
            return Vec::new();
        };
        self.keep_cancel_job(id)
    }

    /// `x cancel that copy`: cancel the other server's live copy the choice
    /// waits on ([`OtherCopy`]), its server named on screen — once, while
    /// it is shown; found by a lookup, or the copy a start answered
    /// `created: false` with.
    pub(crate) fn keep_cancel_other(&mut self) -> Vec<Effect> {
        let Some(KeepStage::Choose { other: Some(other), .. }) = self.keep_view().map(|v| v.stage) else {
            return Vec::new();
        };
        if other.cancelling {
            return Vec::new();
        }
        self.keep_cancel_job(other.job_id)
    }

    /// `c choose again` (clause 110): an ended copy's window back on the
    /// choice — the dot and the box where they stood, the place the
    /// account's now. Nothing where the word does not stand
    /// ([`KeepView::choose_again`]).
    pub(crate) fn keep_choose_again(&mut self) {
        if !self.keep_view().is_some_and(|v| v.choose_again()) {
            return;
        }
        if let Some(flow) = self.keep.flow.as_mut() {
            flow.phase = KeepPhase::Choose;
        }
    }

    /// The finished copy's primary (clause 110, [`window_action`]): Retry
    /// starts the same copy again (taking only the gaps) — a wider copy's
    /// account of the choice ([`covered_view`]) carries the choice's scope
    /// and the window's row, so it starts the choice's own copy — at the
    /// account's place now, which the window says first where it is not
    /// the one the copy ran with (`KeepView::again`, clause 145); Start
    /// again ("Add again") brings the choice back; Play your copy plays it
    /// — an album's, the whole album from the library.
    pub(crate) fn keep_again(&mut self) -> Vec<Effect> {
        let Some(view) = self.keep_view() else { return Vec::new() };
        let KeepStage::Job { job, action, .. } = view.stage else { return Vec::new() };
        match action {
            // A folder copy's "Add again": the dot on its own landing, its
            // Into place as it was (spec §6.7).
            Some(JobAction::StartAgain) => {
                if let Some(flow) = self.keep.flow.as_mut() {
                    flow.phase = KeepPhase::Choose;
                    if job.scope() == Some(JobScope::Folder) {
                        let landing = FolderLanding::of(&job.params.landing);
                        if flow.choices.contains(&KeepChoice::Folder(landing)) {
                            flow.choice = KeepChoice::Folder(landing);
                        }
                        if let JobLanding::Into { vpath, path } = &job.params.landing {
                            flow.into = IntoPlace { vpath: vpath.clone().unwrap_or_else(|| flow.into.vpath.clone()), path: path.clone() };
                        }
                    }
                }
                Vec::new()
            }
            Some(JobAction::Retry | JobAction::CopyAgain) => {
                let shown = Some(view.destination.clone());
                let (effects, ticket) = self.keep_restart(&job, shown.clone());
                if let (Some(ticket), Some(flow)) = (ticket, self.keep.flow.as_mut()) {
                    flow.phase = KeepPhase::Starting { ticket };
                    flow.sent = shown;
                }
                effects
            }
            Some(JobAction::Play) => self.keep_play_copy(&job, true),
            Some(JobAction::Cancel) => self.keep_cancel_job(job.id),
            None => Vec::new(),
        }
    }

    /// The open flow, worked out for drawing (clauses 106–110).
    pub fn keep_view(&self) -> Option<KeepView> {
        let flow = self.keep.flow.as_ref()?;
        let s = &flow.subject;
        let facts = &flow.facts;
        let fact = |known: Option<usize>| match known {
            Some(n) => Fact::Known(n),
            None if flow.facts_asking => Fact::Asking,
            None => Fact::Unknown,
        };
        // Every album by takes theirs alone (gap 26); a folder its planned
        // songs, whichever landing.
        let theirs = match (facts.peer_albums.as_deref(), s.track_artist.as_deref()) {
            (Some(rows), Some(artist)) => Some(plan_artist_albums(rows, artist).theirs.len()),
            _ => None,
        };
        let folder_count = flow.folder_songs().map(<[FolderSong]>::len);
        let choices = flow
            .choices
            .iter()
            .map(|&choice| {
                let size = match choice {
                    KeepChoice::Song => ChoiceSize::Song { seconds: s.duration, format: s.format() },
                    KeepChoice::Album => ChoiceSize::Album {
                        songs: match facts.album_songs.as_ref() {
                            None if flow.songs_seq.is_some() => Fact::Asking,
                            songs => fact(songs.map(Vec::len)),
                        },
                    },
                    KeepChoice::Artist => ChoiceSize::Artist {
                        albums: fact(theirs),
                        have: fact(facts.local_albums.as_ref().map(Vec::len)),
                    },
                    KeepChoice::Folder(_) => ChoiceSize::Folder { songs: fact(folder_count) },
                };
                (choice, size)
            })
            .collect();
        // "Only the albums you don't have" (clause 107): drawn with its own
        // tick whatever the choice; it counts only under Every album by.
        let missing = flow.missing_box(&self.keep.gate).map(|count| (flow.only_missing, count));
        let missing_count = missing.map(|(_, count)| count).unwrap_or(0);
        let only_missing = missing.is_some_and(|(on, _)| on);
        let one_missing = match (&facts.peer_albums, &facts.local_albums, s.track_artist.as_deref()) {
            (Some(remote), Some(local), Some(artist)) => match missing_albums(artist, remote, local).as_slice() {
                [one] => one.name.clone().map(|name| (name, one.year_of())),
                _ => None,
            },
            _ => None,
        };
        let scope = flow.choice.scope(only_missing);
        let verb = match scope {
            JobScope::Song => KeepVerb::Song,
            JobScope::Album => KeepVerb::Album,
            JobScope::ArtistMissing => KeepVerb::Missing(missing_count),
            JobScope::Folder => KeepVerb::Folder,
            JobScope::Artist => KeepVerb::Artist,
        };
        // A folder past the plug-in's cap, before any start (spec §6.5.5):
        // no primary; the window says so, with Close. A server that names
        // no cap has no such state — its own refusal says it.
        let cap = match (&flow.phase, folder_count, self.keep.gate.plugins.as_ref().and_then(|p| p.jobs.max_folder_songs)) {
            (KeepPhase::Choose | KeepPhase::Refused(_), Some(count), Some(cap)) if count as u64 > cap => Some(FolderCap { count, cap }),
            _ => None,
        };
        let verb = cap.is_none().then_some(verb);
        // Another server's live copy of the same album or artist covers
        // this one: say whose, offer its cancel, and hold the primary back
        // until it ends.
        let other_copy = |j: &PluginJob| OtherCopy {
            server: j.peer_name().unwrap_or_default(),
            job_id: j.id,
            cancelling: self.keep.book.cancelling.contains(&j.id) || j.stopping(),
        };
        let other = flow.jobs.slot(scope).filter(|j| j.other_peer(Some(s.peer)) && j.live()).map(&other_copy);
        let followed = |id: i64| {
            self.keep.book.jobs.get(&id).or_else(|| flow.jobs.held.values().find(|j| j.id == id)).cloned()
        };
        // The window's action is its own, read once the view knows where
        // Retry would land ([`window_action`]).
        let ended = |job: PluginJob| {
            let outcome = job.outcome();
            KeepStage::Job { job: Box::new(job), outcome, action: None }
        };
        let stage = match &flow.phase {
            KeepPhase::Choose => KeepStage::Choose { verb: verb.filter(|_| other.is_none()), other },
            KeepPhase::Starting { .. } => KeepStage::Starting,
            KeepPhase::Refused(failure) => KeepStage::Refused { failure: failure.clone(), verb },
            KeepPhase::Off(why) => KeepStage::Off(*why),
            KeepPhase::Following(id) => match followed(*id) {
                Some(job) => ended(job),
                // Gone from everything known (cleared elsewhere): the
                // choice again.
                None => KeepStage::Choose { verb: verb.filter(|_| other.is_none()), other },
            },
            // A wider copy's account of this choice; the choice again once
            // it no longer gives one (cleared elsewhere).
            KeepPhase::Covered(id) => match followed(*id).and_then(|job| covered_view(&job, s, scope)) {
                Some(view) => ended(view),
                None => KeepStage::Choose { verb: verb.filter(|_| other.is_none()), other },
            },
        };
        // A right gone (a re-read, a 409, the gate asked again): the choice
        // gives way to one plain line, the first part of the gate that no
        // longer holds (clause 147) — over a start refused 403's own word.
        // A copy that stands keeps showing it (an ended one whose primary
        // would start it again gives way below, once its action is known).
        let lost = self.keep_lost(scope);
        let stage = match (stage, lost) {
            (KeepStage::Choose { .. } | KeepStage::Refused { .. } | KeepStage::Off(_), Some(why)) => KeepStage::Off(why),
            (stage, _) => stage,
        };
        // Who else the outcome speaks for: a song copied from another
        // server, a wider copy's account of this choice.
        let copied_from = match &stage {
            KeepStage::Job { job, .. } if job.scope() == Some(JobScope::Song) => {
                let elsewhere = match job.peer_id() {
                    Some(id) => id != s.peer,
                    None => job.peer_name().is_some_and(|name| name != s.server),
                };
                job.peer_name().filter(|_| elsewhere)
            }
            _ => None,
        };
        let borrowed = match (&flow.phase, &stage) {
            (KeepPhase::Covered(id), KeepStage::Job { .. }) => followed(*id).and_then(|job| job.scope()),
            _ => None,
        };
        // "Leaves out …" (spec §6.4): while Every album by is the choice —
        // an artist's window's only one — once the artist's albums there
        // answered, and something stays out; in the windows entered from
        // browsing alone (spec §6: the Discover room's stand as built).
        let left_out = match (flow.choice, facts.peer_albums.as_deref(), s.track_artist.as_deref()) {
            (KeepChoice::Artist, Some(rows), Some(artist)) if s.browse => LeftOut::of(rows, artist),
            _ => None,
        };
        let mut view = KeepView {
            subject: s.clone(),
            choices,
            choice: flow.choice,
            scope,
            missing,
            one_missing,
            destination: self.keep_destination().cloned(),
            album_songs: facts.album_songs.clone(),
            landing: None,
            again: None,
            stage,
            copied_from,
            borrowed,
            left_out,
            cap,
            into: (s.focus == KeepFocus::Folder).then(|| {
                let into = flow.into.clone();
                // No library chosen yet: the place in force's.
                match (into.vpath.is_empty(), self.keep_destination()) {
                    (true, Some(place)) => IntoPlace { vpath: place.vpath.clone(), ..into },
                    _ => into,
                }
            }),
            folder_tags: flow.folder_tags.clone(),
        };
        view.landing = self.keep_landing(&view, &flow.sent);
        view.again = keep_again_landing(&view);
        let action = window_action(&view);
        if let KeepStage::Job { action: slot, .. } = &mut view.stage {
            *slot = action;
        }
        // An ended copy whose primary would start a copy again — Retry,
        // Add again — with a right gone: it would start nothing, so the one
        // plain line stands instead (clause 147). Play your copy plays from
        // the library, and Close closes: those stand.
        if let (KeepStage::Job { job, action: Some(JobAction::Retry | JobAction::CopyAgain | JobAction::StartAgain), .. }, Some(why)) =
            (&view.stage, lost)
            && !job.live()
        {
            view.stage = KeepStage::Off(why);
            view.landing = None;
            view.again = None;
        }
        Some(view)
    }

    /// The first part of the gate (clause 101) that no longer holds for a
    /// copy of `scope`, as the window's one plain line words it (clause
    /// 147): the upload right; jobs allowed; the copy plug-in listed with
    /// that scope (a listing that failed lists nothing); a destination
    /// that names somewhere. `None` while it all holds.
    fn keep_lost(&self, scope: JobScope) -> Option<OffWhy> {
        let listing = self.keep.gate.plugins.as_ref();
        // The own server's flags, whichever server is browsed.
        let caps = self.keep_caps();
        if !caps.uploads {
            return Some(OffWhy::Uploads);
        }
        if listing.is_some_and(|p| !p.jobs.allowed) {
            return Some(OffWhy::Jobs);
        }
        if !caps.discovery_plugins || !listing.is_some_and(|p| p.copies(scope)) {
            return Some(OffWhy::Plugin);
        }
        self.keep_destination().is_none().then_some(OffWhy::Uploads)
    }

    /// The LANDS IN line for a view's stage (clause 105): the account's
    /// place while choosing; the place a start sent while it is out; a
    /// copy's pinned place while it runs (a job from before the pin: the
    /// account's), and once it ended the folder its files landed in, else
    /// the place its result or its pin names — never the account's place
    /// now. A pin to nowhere hides it.
    fn keep_landing(&self, view: &KeepView, sent: &Option<Option<CollectionDestination>>) -> Option<Landing> {
        // Past the cap: no place at all (spec §6.5.5).
        if view.cap.is_some() {
            return None;
        }
        let current = view.destination.as_ref();
        let pinned = |pin: &Option<Option<CollectionDestination>>, job: Option<&PluginJob>, scope: JobScope| match pin {
            Some(Some(place)) => Some(view.landing_of(place, job, scope)),
            Some(None) => None,
            None => current.map(|place| view.landing_of(place, job, scope)),
        };
        match &view.stage {
            KeepStage::Choose { .. } | KeepStage::Refused { .. } => current.map(|place| view.landing_at(place, view.scope)),
            KeepStage::Starting => pinned(sent, None, view.scope),
            KeepStage::Off(_) => None,
            KeepStage::Job { job, .. } => {
                let scope = job.scope().unwrap_or(view.scope);
                if job.live() {
                    return pinned(&job.params.destination, Some(job), scope);
                }
                // A folder copy's own word for where its files went (spec
                // §6.7); one by the layout, where its copied files share a
                // folder.
                let result = job.copy_result();
                if let Some(Landed { vpath: Some(library), dirs, .. }) = result.folder_landed().or_else(|| result.landed()) {
                    return Some(Landing::landed(&library, &dirs));
                }
                let ran = result.destination.as_ref().or(match &job.params.destination {
                    Some(Some(place)) => Some(place),
                    _ => None,
                });
                ran.map(|place| view.landing_of(place, Some(job), scope))
            }
        }
    }

    // ── Where copies land (clauses 105, 142–148) ────────────────────────────

    /// Read the destination again — every Keep a copy modal that opens, and
    /// after a start's 409: the last place known draws until it answers,
    /// and a failed read keeps it.
    pub(crate) fn keep_dest_read(&mut self) -> Vec<Effect> {
        if !self.keep_listed() {
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        self.keep.dest.reading = Some(seq);
        vec![Effect::Api(ApiCmd::DestinationRead { seq, generation, reach })]
    }

    /// The picker's Save (clause 144): the place, or `None` — back to the
    /// library default — and its seq, which the picker keeps to know its
    /// own answer. A save sent while another is out supersedes it (the
    /// picker that sent that one is gone); the answer is taken once
    /// ([`App::keep_dest_take_saved`]).
    pub(crate) fn keep_dest_save(&mut self, destination: Option<CollectionDestination>) -> (Vec<Effect>, Option<u64>) {
        if !self.keep_listed() {
            return (Vec::new(), None);
        }
        let Some((generation, reach)) = self.keep_cmd() else { return (Vec::new(), None) };
        let seq = self.keep.next_seq();
        let dest = &mut self.keep.dest;
        dest.saving = Some(seq);
        dest.saved = None;
        (vec![Effect::Api(ApiCmd::DestinationSave { seq, generation, reach, destination })], Some(seq))
    }

    /// The save out, by its seq: its picker draws "saving…", no buttons.
    pub fn keep_dest_saving(&self) -> Option<u64> {
        self.keep.dest.saving
    }

    /// How the last save went, once, by its seq: its view already
    /// replaced the gate's, or the server's words.
    pub(crate) fn keep_dest_take_saved(&mut self) -> Option<(u64, Result<(), PluginFailure>)> {
        self.keep.dest.saved.take()
    }

    /// Ask the folders of `path` (`/{library}/{folder}`) for the picker's
    /// completion (clause 143) and the Into a folder chooser's list (spec
    /// §6.6) — the user's own server's file explorer, whichever server is
    /// browsed (gap 23): once per folder while the picker is open, one ask
    /// at a time — a second waits for the first.
    pub(crate) fn keep_dest_list(&mut self, path: &str) -> Vec<Effect> {
        if !self.keep_listed() || self.keep.dest.folders.contains_key(path) {
            return Vec::new();
        }
        if self.keep.dest.listing.is_some() {
            self.keep.dest.wanted = Some(path.to_string());
            return Vec::new();
        }
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        let dest = &mut self.keep.dest;
        dest.listing = Some((seq, path.to_string()));
        dest.folders.insert(path.to_string(), Folders::Asking);
        vec![Effect::Api(ApiCmd::DestinationFolders { seq, generation, reach, path: path.to_string() })]
    }

    /// The folders of `path` as asked, `None` not asked.
    pub fn keep_dest_folders(&self, path: &str) -> Option<&Folders> {
        self.keep.dest.folders.get(path)
    }

    /// The picker closed: its folders go (the save, if one is out, still
    /// folds in when it answers, and how it went is still said).
    pub(crate) fn keep_dest_forget(&mut self) {
        let dest = &mut self.keep.dest;
        dest.folders.clear();
        dest.wanted = None;
        dest.listing = None;
    }

    /// The destination's view from an answer to the ask sent as `seq`,
    /// unless an answer to a later ask is already drawn.
    fn keep_dest_fold(&mut self, seq: u64, view: CollectionDestinationView) {
        let gate = &mut self.keep.gate;
        if seq > gate.view_seq {
            gate.view = Some(view);
            gate.view_seq = seq;
        }
    }

    // ── Open elsewhere (clause 140) ─────────────────────────────────────────

    /// Open the links list for a row: asked now, never before.
    pub(crate) fn links_open(&mut self, target: KeepTarget) -> Vec<Effect> {
        self.taste_sync();
        self.keep_sync();
        if !self.links_gate() {
            return Vec::new();
        }
        let Some(subject) = self.keep_subject(&target) else { return Vec::new() };
        self.links_ask(subject.rec)
    }

    /// Try again: the same row asked again, as it was first sent — a new
    /// answer landing under the list never retargets it.
    pub(crate) fn links_retry(&mut self) -> Vec<Effect> {
        let Some(rec) = self.keep.links.as_ref().map(|l| l.rec.clone()) else { return Vec::new() };
        if !self.links_gate() {
            return Vec::new();
        }
        self.links_ask(rec)
    }

    fn links_ask(&mut self, rec: serde_json::Value) -> Vec<Effect> {
        let Some(plugin) = self.keep.gate.plugins.as_ref().and_then(|p| p.links_plugin()).map(|p| p.name.clone())
        else {
            return Vec::new();
        };
        let Some((generation, reach)) = self.keep_cmd() else { return Vec::new() };
        let seq = self.keep.next_seq();
        self.keep.links = Some(LinksState { rec: rec.clone(), seq, status: LinksStatus::Asking });
        vec![Effect::Api(ApiCmd::PluginLinks { seq, generation, reach, plugin, rec: Box::new(rec) })]
    }

    pub(crate) fn links_close(&mut self) {
        self.keep.links = None;
    }

    // ── Replies ─────────────────────────────────────────────────────────────

    /// Drop every wait nothing will answer — an error cleared them all.
    pub(super) fn clear_keep_waits(&mut self) {
        let keep = &mut self.keep;
        keep.gate.asking = false;
        keep.lookup = None;
        keep.book.asking = None;
        keep.book.cancelling.clear();
        keep.book.starts.clear();
        keep.book.unseen.clear();
        keep.dest.reading = None;
        keep.dest.saving = None;
        keep.dest.listing = None;
        keep.dest.wanted = None;
        keep.dest.folders.retain(|_, folders| *folders != Folders::Asking);
        // The page's lookup will not answer: its rows are asked again, first.
        if let Some((_, keys)) = keep.marks.lookup.take() {
            for key in keys.iter().rev() {
                keep.marks.again(key);
            }
        }
        if let Some(flow) = keep.flow.as_mut() {
            flow.facts_asking = false;
            flow.songs_seq = None;
            flow.lookup_seq = None;
            if matches!(flow.phase, KeepPhase::Starting { .. }) {
                flow.phase = KeepPhase::Choose;
            }
        }
        self.keep_shelf.credit = None;
        self.keep_shelf.pinging.clear();
    }

    /// A move away from the server the session was on (`shed_server_state`):
    /// the window and its picker close, the page's marks go, and the state
    /// in front follows the session (spec §3.2) — kept for a connected
    /// server of the same own server, another's brought back otherwise.
    pub(super) fn keep_session_moved(&mut self) {
        self.keep_close_quietly();
        self.links_close();
        self.keep.marks = BrowseMarks::default();
        self.keep_shelf.credit = None;
        self.keep_sync();
    }

    /// The window closed with its server left behind: a start still out is
    /// said on the status line when it lands, as any closed window's is.
    fn keep_close_quietly(&mut self) {
        if let Some(KeepPhase::Starting { ticket }) = self.keep.flow.take().map(|f| f.phase) {
            self.keep.book.unseen.insert(ticket);
        }
        self.keep_dest_forget();
    }

    /// A plug-in reply, landed in the state it was asked for — the one in
    /// front, or a parked one, brought forward for it and put back — by its
    /// generation alone; one no state asked fills nothing.
    pub(super) fn consume_keep(&mut self, event: Event) -> Vec<Effect> {
        let generation = match &event {
            Event::KeepGate { generation, .. }
            | Event::JobsLookup { generation, .. }
            | Event::Jobs { generation, .. }
            | Event::JobStarted { generation, .. }
            | Event::JobCancelled { generation, .. }
            | Event::JobsCleared { generation, .. }
            | Event::PluginLinks { generation, .. }
            | Event::KeepFacts { generation, .. }
            | Event::KeepFolderTags { generation, .. }
            | Event::DestinationRead { generation, .. }
            | Event::DestinationSaved { generation, .. }
            | Event::DestinationFolders { generation, .. } => *generation,
            _ => return Vec::new(),
        };
        self.taste_sync();
        self.keep_sync();
        if self.keep.epochs.contains(&generation) {
            return self.keep_landed(event);
        }
        let Some(at) = self.keep_shelf.parked.iter().position(|k| k.epochs.contains(&generation)) else { return Vec::new() };
        let parked = self.keep_shelf.parked.remove(at);
        let front = std::mem::replace(&mut self.keep, parked);
        let effects = self.keep_landed(event);
        let parked = std::mem::replace(&mut self.keep, front);
        self.keep_shelf.parked.insert(at, parked);
        effects
    }

    fn keep_landed(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::KeepGate { plugins, destination, .. } => self.keep_gate_landed(plugins, destination),
            Event::JobsLookup { seq, answer, .. } => self.keep_lookup_landed(seq, answer),
            Event::Jobs { seq, answer, .. } => self.keep_jobs_landed(seq, answer),
            Event::JobStarted { ticket, answer, .. } => self.keep_started(ticket, answer),
            Event::JobCancelled { id, answer, .. } => self.keep_cancelled(id, answer),
            Event::JobsCleared { answer, .. } => match answer {
                Ok(_) => {
                    // What went is gone from the book now; the list and the
                    // rows are asked again for the rest.
                    self.keep.book.jobs.retain(|_, job| !job.state.finished());
                    let mut effects = self.copies_try_again();
                    effects.extend(self.keep_lookup_rows());
                    effects
                }
                Err(failure) => {
                    self.keep.note = Some(KeepNote::ClearFailed(failure));
                    Vec::new()
                }
            },
            Event::PluginLinks { seq, answer, .. } => {
                if let Some(links) = self.keep.links.as_mut().filter(|l| l.seq == seq) {
                    links.status = match answer {
                        Ok(list) => LinksStatus::Ready(list),
                        Err(failure) => LinksStatus::Failed(failure),
                    };
                }
                Vec::new()
            }
            // What it learns joins what it knew (an artist's wall's listing,
            // handed in at open); a browse song learns its album's credit,
            // and the choices that joined append (spec §6.3).
            Event::KeepFacts { seq, facts, .. } => {
                let gate = self.keep.gate.clone();
                // The album's songs asked again by the credit found: Album
                // counts what its copy takes.
                if let Some(flow) = self.keep.flow.as_mut().filter(|f| f.songs_seq == Some(seq)) {
                    flow.songs_seq = None;
                    flow.facts.album_songs = facts.album_songs;
                    return Vec::new();
                }
                let Some(flow) = self.keep.flow.as_mut().filter(|f| f.facts_seq == seq) else { return Vec::new() };
                flow.facts.merge(*facts);
                flow.facts_asking = false;
                let Some(credit) = flow.learn(&gate) else { return Vec::new() };
                // Step 3 found the credit: the songs of its name and year may
                // be more than its album's — its copy's own listing, by it.
                let ask = KeepFactsAsk { album: flow.subject.album.clone(), album_artist: Some(credit), year: flow.subject.year, listed: true, ..KeepFactsAsk::default() };
                let peer = flow.subject.peer;
                let origin = Origin { server: self.keep_home().server, peer: Some(peer) };
                let generation = self.keep.epoch;
                let own = self.keep_cmd().and_then(|(_, reach)| reach);
                let reach = self.reach(&origin).ok();
                let seq = self.keep.next_seq();
                if let Some(flow) = self.keep.flow.as_mut() {
                    flow.songs_seq = Some(seq);
                    flow.facts.album_songs = None;
                }
                vec![Effect::Api(ApiCmd::KeepFacts { seq, generation, reach, own, ask: Box::new(ask) })]
            }
            Event::KeepFolderTags { seq, tracks, .. } => {
                if let Some(flow) = self.keep.flow.as_mut().filter(|f| f.tags_seq == Some(seq)) {
                    flow.folder_tags = tracks.map(|tracks| tracks.iter().map(KeepSong::of).collect());
                }
                Vec::new()
            }
            // A read the last one asked: its view replaces what is drawn —
            // unless a save sent after it answered first; a failed one keeps
            // what is drawn.
            Event::DestinationRead { seq, answer, .. } => {
                if self.keep.dest.reading == Some(seq) {
                    self.keep.dest.reading = None;
                    if let Ok(view) = answer {
                        self.keep_dest_fold(seq, *view);
                    }
                }
                Vec::new()
            }
            // A save's view is the server's word after it, whichever picker
            // sent it; how it went is the last save's to say.
            Event::DestinationSaved { seq, answer, .. } => {
                let went = answer.map(|view| self.keep_dest_fold(seq, *view));
                let dest = &mut self.keep.dest;
                if dest.saving == Some(seq) {
                    dest.saving = None;
                    dest.saved = Some((seq, went));
                }
                Vec::new()
            }
            Event::DestinationFolders { seq, path, answer, .. } => {
                let dest = &mut self.keep.dest;
                if dest.listing.as_ref().is_none_or(|(asked, _)| *asked != seq) {
                    return Vec::new();
                }
                dest.listing = None;
                let folders = match answer {
                    Ok(names) => Folders::Listed(names),
                    Err(failure) => Folders::Failed(failure),
                };
                dest.folders.insert(path, folders);
                match dest.wanted.take() {
                    Some(next) => self.keep_dest_list(&next),
                    None => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn keep_gate_landed(
        &mut self,
        plugins: Result<Box<DiscoveryPluginsResponse>, PluginFailure>,
        destination: Option<Result<Box<crate::api::types::CollectionDestinationView>, PluginFailure>>,
    ) -> Vec<Effect> {
        let gate = &mut self.keep.gate;
        gate.asking = false;
        // A read or a save answered since the gate was asked: its view is
        // newer than this one, which draws nothing.
        let fresh = gate.asked > gate.view_seq;
        match plugins {
            Ok(listing) => {
                gate.failed = None;
                gate.plugins = Some(*listing);
                match destination {
                    Some(Ok(view)) if fresh => gate.view = Some(*view),
                    Some(Ok(_)) => {}
                    // Not asked (no copy plug-in), or not answered: no
                    // destination known — Keep a copy stays hidden.
                    Some(Err(failure)) => {
                        gate.failed = Some(failure);
                        if fresh {
                            gate.view = None;
                        }
                    }
                    None => gate.view = None,
                }
                gate.view_seq = gate.view_seq.max(gate.asked);
            }
            Err(failure) => {
                gate.failed = Some(failure);
                gate.plugins = None;
                gate.view = None;
            }
        }
        // The gate open over rows already on screen: look them up, and ask
        // the list once for the indicator.
        let mut effects = self.keep_lookup_rows();
        if self.keep_listed() && !self.keep.book.listed {
            effects.extend(self.keep_ask_jobs(crate::clock::Instant::now()));
        }
        effects
    }

    fn keep_lookup_landed(&mut self, seq: u64, answer: Result<Vec<JobLookup>, PluginFailure>) -> Vec<Effect> {
        // The flow's own lookup.
        if let Some(flow) = self.keep.flow.as_mut().filter(|f| f.lookup_seq == Some(seq)) {
            flow.lookup_seq = None;
            if let Some(found) = answer.as_ref().ok().and_then(|list| list.first()) {
                // A folder's window holds its folder copies alone.
                let mut jobs = if flow.jobs.folders { RowJobs::from_folder_lookup(found) } else { RowJobs::from_lookup(found) };
                for job in self.keep.book.jobs.values() {
                    jobs.merge(job);
                }
                if flow.phase == KeepPhase::Choose
                    && let Some(phase) = opening_phase(&jobs, &flow.subject, flow.scope(&self.keep.gate))
                {
                    flow.phase = phase;
                }
                flow.jobs = jobs;
                flow.resettle(&self.keep.gate);
            }
            return self.keep_seen(answer.as_deref().unwrap_or(&[]));
        }
        // A page's marks: each row asked, in order — a folder's by its folder
        // key alone. A failed lookup leaves them unmarked: it draws nothing.
        if let Some((_, keys)) = self.keep.marks.lookup.take_if(|(asked, _)| *asked == seq) {
            let Ok(results) = answer else { return Vec::new() };
            for (key, found) in keys.iter().zip(results.iter()) {
                let mut jobs = match key {
                    MarkKey::Folder(_) => RowJobs::from_folder_lookup(found),
                    _ => RowJobs::from_lookup(found),
                };
                for job in self.keep.book.jobs.values() {
                    jobs.merge(job);
                }
                self.keep.marks.land(key, jobs);
            }
            return self.keep_seen(&results);
        }
        let Some(RowsLookup { seq: asked, rows: lines, landed }) = self.keep.lookup.clone() else {
            return Vec::new();
        };
        if asked != seq {
            return Vec::new();
        }
        self.keep.lookup = None;
        // A newer peers answer replaced these rows: its own lookup follows.
        if landed != self.keep.peers_landed {
            return Vec::new();
        }
        let Ok(results) = answer else { return Vec::new() };
        let mut rows = BTreeMap::new();
        for (at, found) in lines.into_iter().zip(results.iter()) {
            let mut jobs = RowJobs::from_lookup(found);
            for job in self.keep.book.jobs.values() {
                jobs.merge(job);
            }
            rows.insert(at, jobs);
        }
        self.keep.rows = rows;
        self.keep_seen(&results)
    }

    /// A lookup's live jobs are seen live: their ending is news.
    fn keep_seen(&mut self, results: &[JobLookup]) -> Vec<Effect> {
        for job in results.iter().flat_map(|r| &r.jobs) {
            if job.live() && listed_copy(job) {
                self.keep.book.seen_live.insert(job.id);
            }
        }
        Vec::new()
    }

    fn keep_jobs_landed(&mut self, seq: u64, answer: Result<Box<PluginJobList>, PluginFailure>) -> Vec<Effect> {
        if self.keep.book.asking != Some(seq) {
            return Vec::new();
        }
        self.keep.book.asking = None;
        let list = match answer {
            Ok(list) => list,
            Err(failure) => {
                let book = &mut self.keep.book;
                book.failures = book.failures.saturating_add(1);
                book.failed = Some(failure);
                return Vec::new();
            }
        };
        let keep = &mut self.keep;
        keep.book.failures = 0;
        keep.book.failed = None;
        keep.book.listed = true;
        // The list is the server's word on the caller's copies: what it no
        // longer lists (cleared) leaves the book.
        let listed: BTreeSet<i64> = list.jobs.iter().map(|j| j.id).collect();
        keep.book.jobs.retain(|id, _| listed.contains(id));
        let mut ended = false;
        for job in list.jobs.iter().filter(|j| j.plugin == COPY_PLUGIN) {
            ended |= keep.fold(job);
        }
        keep.book.seen_live.retain(|id| listed.contains(id));
        // A copy ended: the rows are looked up again, its lookups' slots
        // being the ones that changed (clause 111).
        if ended { self.keep_lookup_rows() } else { Vec::new() }
    }

    fn keep_started(&mut self, ticket: u64, answer: Result<Box<JobStarted>, PluginFailure>) -> Vec<Effect> {
        let from_list = self.keep.book.starts.remove(&ticket);
        // Its window closed while it was out: the status line says it.
        let unseen = self.keep.book.unseen.remove(&ticket);
        let ours = self
            .keep
            .flow
            .as_ref()
            .is_some_and(|f| f.phase == KeepPhase::Starting { ticket });
        match answer {
            Ok(started) => {
                let job = started.job;
                let ended = self.keep.fold(&job);
                if unseen {
                    self.keep.landed.push(Ok(job.clone()));
                }
                if ours && let Some(flow) = self.keep.flow.as_mut() {
                    flow.jobs.merge(&job);
                    // A job the server keyed under none of the flow's keys
                    // (none looked up yet) is still its slot's newest.
                    if let Some(scope) = job.scope() {
                        let slot = flow.jobs.held.entry((job.plugin.clone(), scope)).or_insert_with(|| job.clone());
                        if slot.id < job.id {
                            *slot = job.clone();
                        }
                    }
                    // This server's copy is followed. Another server's live
                    // one (`created: false`) leaves the choice the user's,
                    // as a lookup that found it first does: its two lines
                    // from the slot, its cancel "x cancel that copy", ↑ ↓
                    // to a scope it does not cover — and Add back once it
                    // ends (clause 108).
                    flow.phase = if job.other_peer(Some(flow.subject.peer)) {
                        KeepPhase::Choose
                    } else {
                        KeepPhase::Following(job.id)
                    };
                }
                if ended { self.keep_lookup_rows() } else { Vec::new() }
            }
            Err(failure) => {
                // The place shown is no longer where files land: LANDS IN
                // redraws from the answer's at once, and the view is read
                // again (clause 146).
                // An answer that names none this client reads keeps what is
                // drawn until the read answers.
                let mut effects = Vec::new();
                if let PluginFailure::DestinationChanged(moved) = &failure {
                    let gate = &mut self.keep.gate;
                    if let (Some(now), Some(view)) = (moved, gate.view.as_mut())
                        && ticket > gate.view_seq
                    {
                        view.destination = now.clone();
                        gate.view_seq = ticket;
                    }
                    effects = self.keep_dest_read();
                }
                // A 403 or a 404 is the server's word that a part of the
                // gate went (a right, the plug-in): the gate is asked again,
                // and every way in follows its answer (clause 147).
                let gone = matches!(failure, PluginFailure::Refused { status: 403 | 404, .. });
                if ours && let Some(flow) = self.keep.flow.as_mut() {
                    // A 403 is a right the account no longer has: one
                    // plain line, never the choice again. A 404 reads in
                    // the server's words until the gate says the plug-in
                    // went.
                    flow.phase = match &failure {
                        PluginFailure::Refused { status: 403, words } => KeepPhase::Off(off_for_403(words)),
                        _ => KeepPhase::Refused(failure),
                    };
                } else if from_list {
                    self.keep.note = Some(KeepNote::StartFailed(failure));
                } else if unseen {
                    self.keep.landed.push(Err(failure));
                }
                if gone {
                    effects.extend(self.keep_ask_gate());
                }
                effects
            }
        }
    }

    fn keep_cancelled(&mut self, id: i64, answer: Result<CancelAnswer, PluginFailure>) -> Vec<Effect> {
        self.keep.book.cancelling.remove(&id);
        match answer {
            Ok(CancelAnswer::Cancelled(cancelled)) => {
                let ended = cancelled.job.as_ref().is_some_and(|job| self.keep.fold(job));
                if ended { self.keep_lookup_rows() } else { Vec::new() }
            }
            // It had already finished: refresh, and say so once.
            Ok(CancelAnswer::AlreadyFinished) => {
                let title = self.keep.job(id).map(|job| CopyTitle::of(job).name().to_string());
                self.keep.note = Some(KeepNote::TooLate { title });
                let mut effects = self.copies_try_again();
                effects.extend(self.keep_lookup_rows());
                effects
            }
            Err(failure) => {
                self.keep.note = Some(KeepNote::CancelFailed(failure));
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::ManyCounts;

    fn job(id: i64, key: &str, scope: Option<&str>, state: &str, peer: i64, result: serde_json::Value) -> PluginJob {
        serde_json::from_value(serde_json::json!({
            "id": id, "plugin": COPY_PLUGIN, "key": key, "state": state,
            "params": scope.map(|s| serde_json::json!({ "scope": s })),
            "recommendation": { "artist": "Boukmanflow", "title": "Late Train Verse", "album": "Borrowed Tapes",
                "peer": { "id": peer, "name": if peer == 1 { "Stage Peer B" } else { "Stage Peer C" } } },
            "result": result, "progress": if state == "running" { serde_json::json!(0.4) } else { serde_json::Value::Null },
        }))
        .unwrap()
    }

    fn album_done(copied: u64) -> serde_json::Value {
        serde_json::json!({ "scope": "album", "counts": { "total": 3, "copied": copied, "skipped": 3 - copied, "failed": 0 }, "stopped": null })
    }

    fn row_of(keys: &[&str], jobs: Vec<PluginJob>) -> RowJobs {
        let found = JobLookup {
            key: keys.first().map(|k| k.to_string()),
            keys: keys.iter().map(|k| (k.split(':').next().unwrap().to_string(), serde_json::json!(k))).collect(),
            jobs,
        };
        RowJobs::from_lookup(&found)
    }

    #[test]
    fn a_rows_jobs_keep_the_newest_per_slot_and_merge_by_key() {
        let mut row = row_of(
            &["text:late", "album:tapes", "artist:bouk"],
            vec![
                job(9, "album:tapes", Some("album"), "running", 1, serde_json::Value::Null),
                job(4, "album:tapes", Some("album"), "done", 1, album_done(3)),
                job(5, "folder:x", Some("folder"), "running", 1, serde_json::Value::Null),
            ],
        );
        assert_eq!(row.slot(JobScope::Album).map(|j| j.id), Some(9), "the first per slot, as the server orders them");
        assert_eq!(row.held.len(), 1, "never a folder job");
        // The poll: a newer job under the row's key replaces; another key or an older id does not.
        assert!(row.merge(&job(12, "album:tapes", Some("album"), "done", 1, album_done(2))));
        assert_eq!(row.slot(JobScope::Album).map(|j| j.id), Some(12));
        assert!(!row.merge(&job(11, "album:tapes", Some("album"), "failed", 1, serde_json::Value::Null)));
        assert!(!row.merge(&job(13, "album:other", Some("album"), "running", 1, serde_json::Value::Null)));
        assert!(row.merge(&job(14, "text:late", None, "queued", 1, serde_json::Value::Null)));
        assert_eq!(row.slot(JobScope::Song).map(|j| j.id), Some(14));
        // Another plug-in's job is not a copy.
        let mut youtube = job(15, "text:late", None, "running", 1, serde_json::Value::Null);
        youtube.plugin = "youtube".into();
        assert!(!row.merge(&youtube));
    }

    #[test]
    fn a_folder_row_holds_its_folder_copies_alone_and_a_song_row_never_one() {
        let found = JobLookup {
            key: Some("text:tapes".into()),
            keys: [("song".to_string(), serde_json::json!("text:tapes")), ("folder".to_string(), serde_json::json!("folder:x"))].into_iter().collect(),
            jobs: vec![
                job(5, "folder:x", Some("folder"), "running", 1, serde_json::Value::Null),
                job(6, "text:tapes", None, "done", 1, serde_json::json!({ "copied": { "filepath": "copies/x.mp3" } })),
            ],
        };
        let mut folder = RowJobs::from_folder_lookup(&found);
        assert_eq!((folder.keys.clone(), folder.held.len()), (vec!["folder:x".to_string()], 1));
        assert_eq!(folder_mark(&folder), Some(JobMark::Copying));
        // The poll: its folder's copies merge; a song or an album never does.
        assert!(!folder.merge(&job(7, "text:tapes", None, "running", 1, serde_json::Value::Null)));
        assert!(folder.merge(&job(8, "folder:x", Some("folder"), "done", 1, serde_json::json!({ "scope": "folder", "counts": { "total": 1, "copied": 1, "skipped": 0, "failed": 0 } }))));
        assert_eq!(folder_mark(&folder), Some(JobMark::Kept));
        let song = RowJobs::from_lookup(&found);
        assert_eq!((song.slot(JobScope::Folder), folder_mark(&song)), (None, None), "a song row never holds a folder copy");
    }

    #[test]
    fn a_rows_and_cards_marks_on_a_connected_servers_pages() {
        let late = "peerlib/Boukmanflow/Borrowed Tapes/02.mp3";
        let account = |copied: serde_json::Value, skipped: serde_json::Value, failed: serde_json::Value| {
            serde_json::json!({ "scope": "album", "stopped": null, "songs": songs(copied, skipped, failed) })
        };
        // The album copying: the song rows say nothing — the page's state does.
        let live = row_of(&["text:late", "album:tapes"], vec![job(9, "album:tapes", Some("album"), "running", 1, serde_json::Value::Null)]);
        assert_eq!(song_mark(&live, 1, Some("Borrowed Tapes"), late), None);
        assert_eq!(card_mark(&live, 1, "Borrowed Tapes"), Some(JobMark::Copying));
        // Ended: each song as its lists name it.
        let ended = |copied, skipped, failed| row_of(&["text:late", "album:tapes"], vec![job(9, "album:tapes", Some("album"), "done", 1, account(copied, skipped, failed))]);
        let mine = serde_json::json!([{ "from": late, "filepath": "copies/a.mp3" }]);
        assert_eq!(song_mark(&ended(mine, serde_json::json!([]), serde_json::json!([])), 1, Some("Borrowed Tapes"), late), Some(JobMark::Kept));
        let owned = serde_json::json!([{ "from": late, "why": "owned", "at": "music/a.mp3" }]);
        assert_eq!(song_mark(&ended(serde_json::json!([]), owned, serde_json::json!([])), 1, Some("Borrowed Tapes"), late), Some(JobMark::Yours));
        let failed = serde_json::json!([{ "from": late, "error": "gone" }]);
        assert_eq!(song_mark(&ended(serde_json::json!([]), serde_json::json!([]), failed), 1, Some("Borrowed Tapes"), late), Some(JobMark::Failed));
        assert_eq!(song_mark(&ended(serde_json::json!([]), serde_json::json!([]), serde_json::json!([])), 1, Some("Borrowed Tapes"), late), None, "not named: nothing");
        // A live album copy says nothing through its songs, however far its
        // lists got.
        let partial = row_of(&["text:late", "album:tapes"], vec![job(9, "album:tapes", Some("album"), "running", 1, account(serde_json::json!([{ "from": late }]), serde_json::json!([]), serde_json::json!([])))]);
        assert_eq!(song_mark(&partial, 1, Some("Borrowed Tapes"), late), None);
        // Another server's album copy marks neither; a song copy from it is the recording.
        let theirs = row_of(&["text:late", "album:tapes"], vec![job(9, "album:tapes", Some("album"), "done", 4, account(serde_json::json!([{ "from": late }]), serde_json::json!([]), serde_json::json!([])))]);
        assert_eq!((song_mark(&theirs, 1, Some("Borrowed Tapes"), late), card_mark(&theirs, 1, "Borrowed Tapes")), (None, None));
        let song = row_of(&["text:late"], vec![job(3, "text:late", None, "queued", 4, serde_json::Value::Null)]);
        assert_eq!(song_mark(&song, 1, Some("Borrowed Tapes"), late), Some(JobMark::Waiting));
        assert_eq!(card_mark(&song, 1, "Borrowed Tapes"), None, "one song is not the album");
        // An artist copy: a card speaks for it once it ended, as its walk did; never while it runs.
        let running = row_of(&["artist:bouk"], vec![job(11, "artist:bouk", Some("artist"), "running", 1, serde_json::Value::Null)]);
        assert_eq!((card_mark(&running, 1, "Borrowed Tapes"), artist_mark(&running, 1)), (None, Some(JobMark::Copying)));
        // The artist's wall: the newest of every-album and only-missing.
        let both = row_of(
            &["artist:bouk", "artist-missing:bouk"],
            vec![
                at(job(12, "artist:bouk", Some("artist"), "failed", 1, serde_json::Value::Null), 1_000),
                at(job(13, "artist-missing:bouk", Some("artist-missing"), "cancelled", 1, serde_json::Value::Null), 2_000),
            ],
        );
        assert_eq!(artist_mark(&both, 1), Some(JobMark::Cancelled));
        assert_eq!(artist_mark(&both, 4), None, "another server's");
    }

    #[test]
    fn a_row_wears_its_best_mark_and_never_another_servers_album_copy() {
        // Live outranks finished; a song job is the recording from any server.
        let row = row_of(
            &["text:late", "album:tapes"],
            vec![job(5, "text:late", None, "done", 4, serde_json::json!({ "copied": { "filepath": "demo/x.mp3" } })), job(7, "album:tapes", Some("album"), "running", 1, serde_json::Value::Null)],
        );
        assert_eq!(row_mark(&row, 1, Some("Borrowed Tapes")), Some(JobMark::Copying));
        // Stage Peer C's album copy never marks Stage Peer B's row, running or done.
        let other = row_of(&["album:tapes"], vec![job(8, "album:tapes", Some("album"), "running", 4, serde_json::Value::Null)]);
        assert_eq!(row_mark(&other, 1, Some("Borrowed Tapes")), None);
        assert_eq!(row_mark(&other, 4, Some("Borrowed Tapes")), Some(JobMark::Copying), "its own server's row shows it");
        let song_elsewhere = row_of(&["text:late"], vec![job(5, "text:late", None, "done", 4, serde_json::json!({ "copied": { "filepath": "demo/x.mp3" } }))]);
        assert_eq!(row_mark(&song_elsewhere, 1, Some("Borrowed Tapes")), Some(JobMark::Kept));
        // Failed outranks kept; cancelled is the last word; nothing to copy is no mark.
        let mixed = row_of(
            &["text:late", "album:tapes", "artist:bouk"],
            vec![
                job(5, "text:late", None, "failed", 1, serde_json::Value::Null),
                job(6, "album:tapes", Some("album"), "done", 1, album_done(1)),
            ],
        );
        assert_eq!(row_mark(&mixed, 1, Some("Borrowed Tapes")), Some(JobMark::Failed));
        let cancelled = row_of(&["text:late"], vec![job(5, "text:late", None, "cancelled", 1, serde_json::Value::Null)]);
        assert_eq!(row_mark(&cancelled, 1, None), Some(JobMark::Cancelled));
        let nothing = row_of(&["artist:bouk"], vec![job(5, "artist:bouk", Some("artist"), "done", 1,
            serde_json::json!({ "albums": [], "skippedAlbums": [], "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } }))]);
        assert_eq!(row_mark(&nothing, 1, Some("Borrowed Tapes")), None);
        assert_eq!(row_mark(&RowJobs::default(), 1, None), None);
    }

    #[test]
    fn a_tile_wears_its_album_and_artist_copies_only() {
        let late = row_of(&["text:late", "album:tapes", "artist:bouk"], vec![job(3, "text:late", None, "done", 1, serde_json::json!({ "copied": { "filepath": "demo/x.mp3" } }))]);
        let corner = row_of(&["text:corner", "album:tapes", "artist:bouk"], vec![]);
        assert_eq!(tile_mark([&late, &corner], 1, "Borrowed Tapes"), None, "one song of the album is not the album");
        let album = row_of(&["text:corner", "album:tapes"], vec![job(9, "album:tapes", Some("album"), "queued", 1, serde_json::Value::Null)]);
        assert_eq!(tile_mark([&late, &album], 1, "Borrowed Tapes"), Some(JobMark::Waiting));
        // Another server's copy of the same album: not this tile's.
        let theirs = row_of(&["album:tapes"], vec![job(9, "album:tapes", Some("album"), "running", 4, serde_json::Value::Null)]);
        assert_eq!(tile_mark([&theirs], 1, "Borrowed Tapes"), None);
        // An artist copy speaks for the tile's album as its walk did: a guest spot is no mark.
        let walk = serde_json::json!({ "scope": "artist", "albums": [{ "name": "Borrowed Tapes", "songs": { "total": 2, "copied": [{}, {}], "skipped": [], "failed": [] } }],
            "skippedAlbums": [{ "name": "Block Party Comp", "why": "appearance" }], "songs": { "total": 2, "copied": [{}, {}], "skipped": [], "failed": [] }, "stopped": null });
        let artist = row_of(&["artist:bouk"], vec![job(11, "artist:bouk", Some("artist"), "done", 1, walk)]);
        assert_eq!(tile_mark([&artist], 1, "Borrowed Tapes"), Some(JobMark::Kept));
        assert_eq!(tile_mark([&artist], 1, "Block Party Comp"), None);
        // The newest per slot across the tile's rows.
        let old = row_of(&["album:tapes"], vec![job(4, "album:tapes", Some("album"), "failed", 1, serde_json::Value::Null)]);
        let new = row_of(&["album:tapes"], vec![job(8, "album:tapes", Some("album"), "done", 1, album_done(3))]);
        assert_eq!(tile_mark([&old, &new], 1, "Borrowed Tapes"), Some(JobMark::Kept), "a retried failure is old news");
    }

    #[test]
    fn the_poll_backs_off_doubling_to_a_minute() {
        assert_eq!(keep_backoff(KEEP_POLL_MODAL, 0), Duration::from_millis(1500));
        assert_eq!(keep_backoff(KEEP_POLL_MODAL, 1), Duration::from_secs(3));
        assert_eq!(keep_backoff(KEEP_POLL_ROOM, 2), Duration::from_secs(16));
        assert_eq!(keep_backoff(KEEP_POLL_ROOM, 5), KEEP_POLL_MAX);
        assert_eq!(keep_backoff(KEEP_POLL_ROOM, 400), KEEP_POLL_MAX);
    }

    fn subject(title: Option<&str>, artist: Option<&str>, album: Option<&str>, album_artist: Option<&str>) -> KeepSubject {
        KeepSubject {
            peer: 1,
            server: "Stage Peer B".into(),
            focus: if title.is_some() { KeepFocus::Song } else { KeepFocus::Album },
            folder: None,
            browse: false,
            listed_compilation: false,
            credits_disagree: false,
            title: title.map(str::to_string),
            track_artist: artist.map(str::to_string),
            album: album.map(str::to_string),
            album_artist: album_artist.map(str::to_string),
            year: Some(2019),
            duration: Some(8.0),
            filepath: Some("peerlib/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.MP3".into()),
            art: None,
            rec: serde_json::json!({}),
        }
    }

    fn listing(scopes: &[&str]) -> DiscoveryPluginsResponse {
        serde_json::from_value(serde_json::json!({
            "plugins": [{ "name": COPY_PLUGIN, "capabilities": ["acquire"], "scopes": scopes }], "jobs": { "allowed": true }
        }))
        .unwrap()
    }

    /// The connected server's albums of an artist, as the facts read them.
    fn rows_of(rows: &[(&str, Option<&str>, &[&str], bool)]) -> Vec<ArtistAlbumRow> {
        rows.iter()
            .map(|&(name, credit, artists, compilation)| ArtistAlbumRow {
                name: (!name.is_empty()).then(|| name.to_string()),
                album_artist: credit.map(str::to_string),
                credit_listed: true,
                artists: artists.iter().map(|a| a.to_string()).collect(),
                compilation,
                ..ArtistAlbumRow::default()
            })
            .collect()
    }

    #[test]
    fn the_choices_follow_the_rows_fields_and_the_plugins_scopes() {
        let all = listing(&["song", "album", "artist", "artist-missing", "folder"]);
        let row = subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        // At open: This song and Album — Every album by joins once the
        // connected server's albums of the artist answer (the user's
        // 2026-10-27 decision: choices append, Discover too).
        assert_eq!(keep_choices(&all, &row), vec![KeepChoice::Song, KeepChoice::Album]);
        assert_eq!(row.format().as_deref(), Some("mp3"));
        // No artist: no album copy (every album of that name would come back) and no artist copy.
        assert_eq!(keep_choices(&all, &subject(Some("T"), None, Some("Borrowed Tapes"), None)), vec![KeepChoice::Song]);
        // A compilation row: album by its album artist.
        let comp = subject(Some("Kim Song"), Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        assert_eq!(keep_choices(&all, &comp), vec![KeepChoice::Song, KeepChoice::Album]);
        assert_eq!(comp.album_credit(), Some("Various Artists"));
        // A tile: album focus, no This song — Every album by at open, the
        // album being the artist's own (spec §6.2); a compilation's never,
        // nor Various Artists'.
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        assert_eq!(keep_choices(&all, &tile), vec![KeepChoice::Album, KeepChoice::Artist]);
        let comp_tile = subject(None, Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        assert_eq!(keep_choices(&all, &comp_tile), vec![KeepChoice::Album]);
        assert!(comp_tile.compilation() && !tile.compilation());
        let va_tile = subject(None, Some("Various Artists"), Some("Night Market Sampler"), None);
        assert_eq!(keep_choices(&all, &va_tile), vec![KeepChoice::Album]);
        // The plug-in runs the song alone (an older server): Song only; a tile then offers nothing.
        let old = listing(&[]);
        assert_eq!(keep_choices(&old, &row), vec![KeepChoice::Song]);
        assert!(keep_choices(&old, &tile).is_empty());
        // Jobs refused: nothing.
        let mut refused = all.clone();
        refused.jobs.allowed = false;
        assert!(keep_choices(&refused, &row).is_empty());
    }

    #[test]
    fn every_album_by_joins_once_the_artist_has_an_album_of_theirs_never_for_various_artists() {
        let all = listing(&["song", "album", "artist", "artist-missing", "folder"]);
        let facts = |rows: Vec<ArtistAlbumRow>| KeepFacts { peer_albums: Some(rows), ..KeepFacts::default() };
        let theirs = facts(rows_of(&[("Borrowed Tapes", Some("Boukmanflow"), &["Boukmanflow"], false)]));
        let guest = facts(rows_of(&[("Night Market Sampler", Some("Various Artists"), &["Color Out", "Boukmanflow"], true)]));
        let row = subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        // Nothing answered: nothing joins; one of theirs: Every album by, appended.
        assert_eq!(joined_choices(&all, &row, &KeepFacts::default()), vec![KeepChoice::Song, KeepChoice::Album]);
        assert_eq!(joined_choices(&all, &row, &theirs), vec![KeepChoice::Song, KeepChoice::Album, KeepChoice::Artist]);
        // Theirs = 0 (Lantern Choir only appears on a compilation): never.
        assert_eq!(joined_choices(&all, &row, &guest), vec![KeepChoice::Song, KeepChoice::Album]);
        // Various Artists: never, whatever the listing says.
        let va = subject(Some("Lantern Street"), Some("Various Artists"), Some("Night Market Sampler"), None);
        let va_theirs = facts(rows_of(&[("Night Market Sampler", Some("Various Artists"), &["Various Artists"], false)]));
        assert!(!joined_choices(&all, &va, &va_theirs).contains(&KeepChoice::Artist));
        // An album focus has its choices at open and gains none: on a
        // compilation never Every album by, even where its track artist has
        // albums of theirs (it would copy them, not the compilation).
        let comp_tile = subject(None, Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        let kims = facts(rows_of(&[("Kim's Own", Some("Peer Kim"), &["Peer Kim"], false)]));
        assert_eq!(keep_choices(&all, &comp_tile), vec![KeepChoice::Album]);
        assert!(joined_choices(&all, &comp_tile, &kims).is_empty());
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        assert!(joined_choices(&all, &tile, &theirs).is_empty());
        // A compilation's song keeps its own artist's Every album by.
        let kim_song = subject(Some("Kim Song"), Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        assert_eq!(joined_choices(&all, &kim_song, &kims), vec![KeepChoice::Song, KeepChoice::Album, KeepChoice::Artist]);
        // A plug-in that runs no artist copy: never.
        assert_eq!(joined_choices(&listing(&["song", "album"]), &row, &theirs), vec![KeepChoice::Song, KeepChoice::Album]);
    }

    #[test]
    fn a_browse_song_needs_its_albums_own_credit_and_finds_it_in_its_facts() {
        let all = listing(&["song", "album", "artist", "artist-missing", "folder"]);
        // A connected server's song row with no album page credit: no Album
        // at open — never the song's artist (spec rule 1).
        let browse = KeepSubject { browse: true, ..subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None) };
        assert_eq!(browse.album_credit(), None);
        assert_eq!(keep_choices(&all, &browse), vec![KeepChoice::Song]);
        // Step 2: the album's songs agree on their album-artist — known.
        let song = |credit: Option<&str>| KeepSong { filepath: "x.mp3".into(), album_artist: credit.map(str::to_string), ..KeepSong::default() };
        let agree = KeepFacts { album_songs: Some(vec![song(Some("Boukmanflow")), song(Some("boukmanflow "))]), ..KeepFacts::default() };
        assert_eq!(window_credit(&browse, &agree), (AlbumCredit::Known("Boukmanflow".into()), true));
        // They disagree: never an album copy.
        let split = KeepFacts { album_songs: Some(vec![song(Some("ALM")), song(Some("Boukmanflow"))]), ..KeepFacts::default() };
        assert_eq!(window_credit(&browse, &split), (AlbumCredit::Disagree(vec!["ALM".into(), "Boukmanflow".into()]), true));
        // One names none (an older server): step 3, the artist's albums there.
        let older = KeepFacts {
            album_songs: Some(vec![song(Some("Boukmanflow")), song(None)]),
            peer_albums: Some(rows_of(&[("Borrowed Tapes", Some("Boukmanflow"), &["Boukmanflow"], false), ("Second Shift", Some("Boukmanflow"), &[], false)])),
            ..KeepFacts::default()
        };
        assert_eq!(window_credit(&browse, &older), (AlbumCredit::Known("Boukmanflow".into()), false));
        // The flow learns it: the credit on the subject, Album appended, and
        // the album copy's recommendation carries it as its albumArtist.
        let mut flow = KeepFlow {
            target: KeepTarget::Tile(0),
            subject: browse.clone(),
            choices: vec![KeepChoice::Song],
            choice: KeepChoice::Song,
            only_missing: true,
            facts: older,
            facts_asking: false,
            facts_seq: 0,
            songs_seq: None,
            lookup_seq: None,
            jobs: RowJobs::default(),
            phase: KeepPhase::Choose,
            sent: None,
            into: IntoPlace::default(),
            folder_tags: None,
            tags_seq: None,
            folder_plan: None,
            untagged: false,
        };
        let gate = KeepGate { plugins: Some(all.clone()), ..KeepGate::default() };
        flow.learn(&gate);
        assert_eq!(flow.subject.album_artist.as_deref(), Some("Boukmanflow"));
        assert_eq!(flow.choices, vec![KeepChoice::Song, KeepChoice::Album, KeepChoice::Artist], "appended, the dot where it was");
        assert_eq!(flow.choice, KeepChoice::Song);
        assert_eq!(flow.subject.rec_for(JobScope::Album)["albumArtist"], "Boukmanflow");
        assert!(flow.subject.rec_for(JobScope::Song).get("albumArtist").is_none(), "a song copy sends its row as it is");
        // Learning again never takes a choice away.
        flow.facts = KeepFacts::default();
        flow.learn(&gate);
        assert_eq!(flow.choices.len(), 3);
    }

    /// The subject's song on its server (the `subject` helper's file).
    const LATE: &str = "peerlib/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.MP3";
    const LANDED: &str = "copies/Boukmanflow/Borrowed Tapes/02 - Late Train Verse.mp3";

    /// One album's songs as copySongs accounts them.
    fn songs(copied: serde_json::Value, skipped: serde_json::Value, failed: serde_json::Value) -> serde_json::Value {
        let n = [&copied, &skipped, &failed].iter().map(|l| l.as_array().map_or(0, Vec::len)).sum::<usize>();
        serde_json::json!({ "total": n, "copied": copied, "skipped": skipped, "failed": failed })
    }

    fn landed() -> serde_json::Value {
        serde_json::json!([{ "from": LATE, "vpath": "copies", "filepath": LANDED, "title": "Late Train Verse", "album": "Borrowed Tapes" }])
    }

    /// A finished artist walk: the albums it took (`name`, its songs) and
    /// the ones it left out.
    fn walk(scope: &str, albums: Vec<(&str, serde_json::Value)>, left: serde_json::Value) -> serde_json::Value {
        let albums: Vec<serde_json::Value> =
            albums.into_iter().map(|(name, s)| serde_json::json!({ "name": name, "songs": s, "stopped": null })).collect();
        serde_json::json!({ "scope": scope, "albums": albums, "skippedAlbums": left,
            "songs": songs(serde_json::json!([]), serde_json::json!([]), serde_json::json!([])),
            "stopped": null, "peer": { "id": 1, "name": "Stage Peer B" } })
    }

    fn at(mut job: PluginJob, finished: i64) -> PluginJob {
        job.finished_at = Some(finished);
        job.updated_at = Some(finished);
        job
    }

    const KEYS: [&str; 5] = ["text:late", "album:tapes", "artist:bouk", "artist-missing:bouk", "folder:x"];

    #[test]
    fn a_finished_artist_copy_speaks_for_the_album_choice_as_its_walk_did() {
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        let phase = |jobs: Vec<PluginJob>| opening_phase(&row_of(&KEYS, jobs), &tile, JobScope::Album);
        // The user's case: only-missing copied the album — in your collection, Copy again.
        let missing = job(20, "artist-missing:bouk", Some("artist-missing"), "done", 1,
            walk("artist-missing", vec![("Borrowed Tapes", songs(landed(), serde_json::json!([]), serde_json::json!([])))], serde_json::json!([])));
        assert_eq!(phase(vec![missing.clone()]), Some(KeepPhase::Covered(20)));
        let view = covered_view(&missing, &tile, JobScope::Album).unwrap();
        assert_eq!((view.id, view.scope(), &view.recommendation), (20, Some(JobScope::Album), &tile.rec), "the choice's scope and row");
        assert!(matches!(view.outcome(), JobOutcome::CopiedMany(ManyCounts { copied: 1, total: 1, .. })), "{:?}", view.outcome());
        assert_eq!(view.outcome().action(), Some(JobAction::CopyAgain));
        // Only-missing left it out as already here: already yours, Close.
        let owned = job(21, "artist-missing:bouk", Some("artist-missing"), "done", 1,
            walk("artist-missing", vec![], serde_json::json!([{ "name": "borrowed  TAPES", "why": "owned" }])));
        assert_eq!(phase(vec![owned.clone()]), Some(KeepPhase::Covered(21)));
        let view = covered_view(&owned, &tile, JobScope::Album).unwrap();
        assert!(matches!(view.outcome(), JobOutcome::Owned { many: Some(_), .. }), "{:?}", view.outcome());
        assert_eq!(view.outcome().action(), None);
        // Its walk failed every song of the album: it says so (its mark does), Retry.
        let failed = job(22, "artist:bouk", Some("artist"), "done", 1,
            walk("artist", vec![("Borrowed Tapes", songs(serde_json::json!([]), serde_json::json!([]), serde_json::json!([{ "from": LATE, "error": "gone" }])))], serde_json::json!([])));
        assert!(matches!(covered_view(&failed, &tile, JobScope::Album).map(|v| v.outcome()), Some(JobOutcome::Failed { .. })));
        // A guest spot, an album the walk never took: nothing — Add album stays.
        let guest = job(23, "artist:bouk", Some("artist"), "done", 1, walk("artist", vec![], serde_json::json!([{ "name": "Borrowed Tapes", "why": "appearance" }])));
        assert_eq!(phase(vec![guest]), None);
        let elsewhere = job(24, "artist:bouk", Some("artist"), "done", 1,
            walk("artist", vec![("Second Wind", songs(serde_json::json!([{ "from": "x" }]), serde_json::json!([]), serde_json::json!([])))], serde_json::json!([])));
        assert_eq!(phase(vec![elsewhere]), None);
        // Another server's finished artist copy: not this one's.
        let mut theirs = missing.clone();
        theirs.recommendation["peer"]["id"] = serde_json::json!(4);
        assert_eq!(phase(vec![theirs]), None);
        // A live, failed or cancelled artist copy is the whole run's state, never the album's.
        for state in ["running", "failed", "cancelled"] {
            let mut j = missing.clone();
            j.state = serde_json::from_value(serde_json::json!(state)).unwrap();
            assert_eq!(phase(vec![j]), None, "{state}");
        }
        // Only the album choice reads a walk this way; an album copy is the choice's own.
        assert_eq!(covered_view(&missing, &tile, JobScope::Artist), None);
        assert_eq!(covered_view(&job(25, "album:tapes", Some("album"), "done", 1, album_done(3)), &tile, JobScope::Album), None);
    }

    #[test]
    fn the_choices_own_copy_wins_when_it_is_live_or_newer() {
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        let phase = |jobs: Vec<PluginJob>| opening_phase(&row_of(&KEYS, jobs), &tile, JobScope::Album);
        let kept = |id: i64| {
            job(id, "artist-missing:bouk", Some("artist-missing"), "done", 1,
                walk("artist-missing", vec![("Borrowed Tapes", songs(landed(), serde_json::json!([]), serde_json::json!([])))], serde_json::json!([])))
        };
        // The artist copy finished after the album's own: the artist copy speaks.
        let album = job(10, "album:tapes", Some("album"), "done", 1, album_done(0));
        assert_eq!(phase(vec![at(album.clone(), 1_000), at(kept(20), 2_000)]), Some(KeepPhase::Covered(20)));
        // The album's own finished after it (its timestamps, whatever the ids): the own copy.
        assert_eq!(phase(vec![at(album.clone(), 3_000), at(kept(20), 2_000)]), Some(KeepPhase::Following(10)));
        // No timestamps: the server's order.
        assert_eq!(phase(vec![job(30, "album:tapes", Some("album"), "done", 1, album_done(0)), kept(20)]), Some(KeepPhase::Following(30)));
        assert_eq!(phase(vec![album.clone(), kept(20)]), Some(KeepPhase::Covered(20)));
        // Its own live copy always wins.
        let live = job(5, "album:tapes", Some("album"), "queued", 1, serde_json::Value::Null);
        assert_eq!(phase(vec![live, at(kept(20), 2_000)]), Some(KeepPhase::Following(5)));
        // A wider copy that says nothing for the album leaves the own copy as it was.
        let guest = job(40, "artist:bouk", Some("artist"), "done", 1, walk("artist", vec![], serde_json::json!([{ "name": "Borrowed Tapes", "why": "appearance" }])));
        assert_eq!(phase(vec![album.clone(), guest]), Some(KeepPhase::Following(10)));
        // Another server's album copy is not its own: the wider copy of this server speaks.
        let theirs = job(50, "album:tapes", Some("album"), "done", 4, album_done(3));
        assert_eq!(phase(vec![theirs, kept(20)]), Some(KeepPhase::Covered(20)));
        // Nothing at all: the choice.
        assert_eq!(phase(vec![]), None);
    }

    #[test]
    fn the_newest_wider_copy_that_speaks_wins_and_a_walk_that_marks_nothing_never_does() {
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        let phase = |jobs: Vec<PluginJob>| opening_phase(&row_of(&KEYS, jobs), &tile, JobScope::Album);
        let none = || serde_json::json!([]);
        // Every album took it; a later only-missing copy left it out as already here: the later speaks.
        let took = job(20, "artist:bouk", Some("artist"), "done", 1, walk("artist", vec![("Borrowed Tapes", songs(landed(), none(), none()))], none()));
        let left = job(21, "artist-missing:bouk", Some("artist-missing"), "done", 1,
            walk("artist-missing", vec![], serde_json::json!([{ "name": "Borrowed Tapes", "why": "owned" }])));
        assert_eq!(phase(vec![took.clone(), left.clone()]), Some(KeepPhase::Covered(21)));
        // Their finish times say the every-album copy ended last: it speaks.
        assert_eq!(phase(vec![at(took, 3_000), at(left, 2_000)]), Some(KeepPhase::Covered(20)));
        // A walk that took the album and found no song in it marks nothing: Add album stays.
        let empty = job(22, "artist:bouk", Some("artist"), "done", 1, walk("artist", vec![("Borrowed Tapes", songs(none(), none(), none()))], none()));
        assert_eq!(covered_view(&empty, &tile, JobScope::Album), None);
        assert_eq!(phase(vec![empty]), None);
    }

    #[test]
    fn a_wider_copy_speaks_for_this_song_only_where_its_lists_name_the_songs_file() {
        let row = subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        let phase = |jobs: Vec<PluginJob>| opening_phase(&row_of(&KEYS, jobs), &row, JobScope::Song);
        let album = |id: i64, state: &str, s: serde_json::Value, peer: i64| {
            job(id, "album:tapes", Some("album"), state, peer, serde_json::json!({ "scope": "album", "songs": s, "stopped": null, "peer": { "id": peer } }))
        };
        // The album copy copied it: in your collection, its landed file, Play your copy.
        let copied = album(30, "done", songs(landed(), serde_json::json!([{ "from": "peerlib/x.mp3", "why": "owned" }]), serde_json::json!([])), 1);
        assert_eq!(phase(vec![copied.clone()]), Some(KeepPhase::Covered(30)));
        let view = covered_view(&copied, &row, JobScope::Song).unwrap();
        assert_eq!(view.scope(), Some(JobScope::Song));
        assert_eq!(view.outcome(), JobOutcome::Copied { filepath: LANDED.into(), vpath: Some("copies".into()) });
        assert_eq!(view.outcome().action(), Some(JobAction::Play));
        // Skipped as owned: already yours, where the library has it; a file in the way: that.
        let owned = album(31, "done", songs(serde_json::json!([]), serde_json::json!([{ "from": LATE, "why": "owned", "at": "music/Late.mp3" }]), serde_json::json!([])), 1);
        assert_eq!(covered_view(&owned, &row, JobScope::Song).map(|v| v.outcome()), Some(JobOutcome::Owned { filepath: Some("music/Late.mp3".into()), many: None }));
        let exists = album(32, "done", songs(serde_json::json!([]), serde_json::json!([{ "from": LATE, "why": "exists", "at": LANDED }]), serde_json::json!([])), 1);
        assert_eq!(covered_view(&exists, &row, JobScope::Song).map(|v| v.outcome()), Some(JobOutcome::Exists { filepath: Some(LANDED.into()) }));
        // Failed, another song, another spelling of the path: never claimed.
        let failed = album(33, "done", songs(serde_json::json!([]), serde_json::json!([]), serde_json::json!([{ "from": LATE, "error": "gone" }])), 1);
        let other = album(34, "done", songs(serde_json::json!([{ "from": "peerlib/Boukmanflow/Borrowed Tapes/01 - Corner Store Loop.MP3", "filepath": "copies/x.mp3" }]), serde_json::json!([]), serde_json::json!([])), 1);
        let cased = album(35, "done", songs(serde_json::json!([{ "from": LATE.to_lowercase(), "filepath": LANDED }]), serde_json::json!([]), serde_json::json!([])), 1);
        for j in [failed, other, cased] {
            assert_eq!(phase(vec![j.clone()]), None, "{}", j.id);
        }
        // Another server's album copy naming the same path, a live one: not this choice's.
        assert_eq!(phase(vec![album(36, "done", songs(landed(), serde_json::json!([]), serde_json::json!([])), 4)]), None);
        assert_eq!(phase(vec![album(37, "running", songs(landed(), serde_json::json!([]), serde_json::json!([])), 1)]), None);
        // A failed or cancelled run says nothing new for the song, whatever
        // its lists name (rule 3; the webapp's and the phone's rule too).
        for state in ["cancelled", "failed"] {
            let ended = album(38, state, songs(landed(), serde_json::json!([]), serde_json::json!([])), 1);
            assert_eq!(phase(vec![ended]), None, "{state}");
            let mut run = job(38, "artist-missing:bouk", Some("artist-missing"), state, 1,
                walk("artist-missing", vec![("Borrowed Tapes", songs(landed(), serde_json::json!([]), serde_json::json!([])))], serde_json::json!([])));
            run.result.as_mut().unwrap()["songs"] = songs(landed(), serde_json::json!([]), serde_json::json!([]));
            assert_eq!(phase(vec![run]), None, "artist {state}");
        }
        // An artist copy speaks through its walk of the row's album.
        let walked = job(39, "artist-missing:bouk", Some("artist-missing"), "done", 1,
            walk("artist-missing", vec![("Borrowed Tapes", songs(landed(), serde_json::json!([]), serde_json::json!([])))], serde_json::json!([])));
        assert_eq!(phase(vec![walked]), Some(KeepPhase::Covered(39)));
        // The song's own copy wins when live or newer; an older one gives way.
        let own = |id: i64, state: &str| job(id, "text:late", None, state, 1, serde_json::json!({ "skipped": "owned" }));
        assert_eq!(phase(vec![own(50, "done"), copied.clone()]), Some(KeepPhase::Following(50)));
        assert_eq!(phase(vec![own(5, "done"), copied.clone()]), Some(KeepPhase::Covered(30)));
        assert_eq!(phase(vec![own(5, "running"), copied.clone()]), Some(KeepPhase::Following(5)));
        // A row naming no file: nothing to find.
        let nameless = KeepSubject { filepath: None, ..row.clone() };
        assert_eq!(covered_view(&copied, &nameless, JobScope::Song), None);
    }

    #[test]
    fn the_landing_line_renders_the_scopes_tags() {
        let place = |layout: &str| CollectionDestination {
            vpath: "copies".into(),
            base: "from-peers".into(),
            layout: layout.into(),
            source: None,
        };
        let view = KeepView {
            subject: subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None),
            choices: Vec::new(),
            choice: KeepChoice::Song,
            scope: JobScope::Song,
            missing: None,
            one_missing: None,
            destination: None,
            album_songs: None,
            landing: None,
            again: None,
            stage: KeepStage::Starting,
            copied_from: None,
            borrowed: None,
            left_out: None,
            cap: None,
            into: None,
            folder_tags: None,
        };
        let crumbs = |view: &KeepView, layout: &str, scope: JobScope| {
            let landing = view.landing_at(&place(layout), scope);
            (landing.crumbs(), landing.per_album)
        };
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(crumbs(&view, "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", JobScope::Song), (names(&["copies", "from-peers", "Boukmanflow", "2019 - Borrowed Tapes"]), false));
        // The album's credit is its ALBUMARTIST: a compilation stays in one folder.
        let comp = KeepView { subject: subject(Some("Kim Song"), Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists")), ..view.clone() };
        assert_eq!(crumbs(&comp, "{{ALBUMARTIST}}/{{ALBUM}}", JobScope::Album), (names(&["copies", "from-peers", "Various Artists", "Mixtape Vol 1"]), false));
        // An artist copy: the track artist as both credits, then one folder per album.
        assert_eq!(crumbs(&comp, "{{ALBUMARTIST}}/{{ALBUM}}", JobScope::Artist), (names(&["copies", "from-peers", "Peer Kim"]), true));
        assert_eq!(crumbs(&view, "{{PEER}}/{{ARTIST}}", JobScope::Artist), (names(&["copies", "from-peers", "Stage Peer B", "Boukmanflow"]), false));
        // Only the albums you don't have, taking one: that album, named —
        // and its year, as the webapp's scopeTarget names it.
        let one = KeepView { one_missing: Some(("Second Shift".into(), Some(2021))), ..view.clone() };
        assert_eq!(crumbs(&one, "{{ARTIST}}/{{ALBUM}}", JobScope::ArtistMissing), (names(&["copies", "from-peers", "Boukmanflow", "Second Shift"]), false));
        assert_eq!(
            crumbs(&one, "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", JobScope::ArtistMissing),
            (names(&["copies", "from-peers", "Boukmanflow", "2021 - Second Shift"]), false),
            "the album's own year, never the song's"
        );
        let undated = KeepView { one_missing: Some(("Second Shift".into(), None)), ..view.clone() };
        assert_eq!(crumbs(&undated, "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", JobScope::ArtistMissing), (names(&["copies", "from-peers", "Boukmanflow", "- Second Shift"]), false));
        assert_eq!(crumbs(&one, "{{ARTIST}}/{{ALBUM}}", JobScope::Artist), (names(&["copies", "from-peers", "Boukmanflow"]), true), "every album: no name");
        // A song with no year leaves {{YEAR}} out, and the picker's preview
        // names it (the webapp's missingVars).
        let undated = KeepView { subject: KeepSubject { year: None, ..view.subject.clone() }, ..view.clone() };
        assert_eq!(undated.landing_at(&place("{{ARTIST}}/{{YEAR}}/{{ALBUM}}"), JobScope::Song).missing, ["YEAR"]);
    }

    #[test]
    fn a_variable_each_song_fills_from_its_own_tags_is_a_slot_where_they_need_not_agree() {
        let place = |layout: &str| CollectionDestination { vpath: "copies".into(), layout: layout.into(), ..CollectionDestination::default() };
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let song = |filepath: &str, artist: Option<&str>, year: Option<i32>, genre: Option<&str>| KeepSong {
            filepath: filepath.into(),
            artist: artist.map(str::to_string),
            year,
            genre: genre.map(str::to_string),
            ..Default::default()
        };
        // The 2026-10-22 Night Market Sampler: a compilation whose row is
        // Boukmanflow's song, its three songs by three artists.
        let sampler = KeepView {
            subject: KeepSubject {
                filepath: Some("peerlib/Night Market Sampler/02.mp3".into()),
                ..subject(Some("Lanmè"), Some("Boukmanflow"), Some("Night Market Sampler"), Some("Various Artists"))
            },
            choices: Vec::new(),
            choice: KeepChoice::Album,
            scope: JobScope::Album,
            missing: None,
            one_missing: None,
            destination: None,
            album_songs: None,
            landing: None,
            again: None,
            stage: KeepStage::Starting,
            copied_from: None,
            borrowed: None,
            left_out: None,
            cap: None,
            into: None,
            folder_tags: None,
        };
        let at = |view: &KeepView, layout: &str, scope: JobScope| {
            let landing = view.landing_at(&place(layout), scope);
            (landing.crumbs(), landing.slots())
        };
        let friends = "From friends/{{ARTIST}}";
        // Before the listing answers: a compilation's ARTIST and YEAR are
        // slots, its album and credit what the row says; GENRE always one.
        assert_eq!(at(&sampler, friends, JobScope::Album), (names(&["copies", "From friends", "‹artist›"]), vec![SongTag::Artist]));
        assert_eq!(
            at(&sampler, "{{ALBUMARTIST}}/{{YEAR}} - {{ALBUM}}/{{GENRE}}", JobScope::Album),
            (names(&["copies", "Various Artists", "‹year› - Night Market Sampler", "‹genre›"]), vec![SongTag::Year, SongTag::Genre])
        );
        // Not a compilation (no credit, or the credit is the row's artist): the row's own.
        let own = KeepView { subject: KeepSubject { album_artist: None, ..sampler.subject.clone() }, ..sampler.clone() };
        assert_eq!(at(&own, "{{ARTIST}}/{{YEAR}}", JobScope::Album), (names(&["copies", "Boukmanflow", "2019"]), vec![]));
        let credited = KeepView { subject: KeepSubject { album_artist: Some("boukmanflow".into()), ..sampler.subject.clone() }, ..sampler.clone() };
        assert_eq!(at(&credited, "{{ARTIST}}", JobScope::Album).0, names(&["copies", "Boukmanflow"]));
        // The listing answered: three artists, so a slot; a song with no
        // year has none to fall back to (its folder drops), so a slot too.
        let three = KeepView {
            album_songs: Some(vec![
                song("peerlib/Night Market Sampler/01.mp3", Some("Color Out"), Some(2024), Some("Electronic")),
                song("peerlib/Night Market Sampler/02.mp3", Some("Boukmanflow"), Some(2024), Some("Hip-Hop")),
                song("peerlib/Night Market Sampler/03.mp3", Some("ALM"), None, Some("Electronic")),
            ]),
            subject: KeepSubject { year: Some(2024), ..sampler.subject.clone() },
            ..sampler.clone()
        };
        assert_eq!(
            at(&three, "{{ARTIST}}/{{YEAR}}/{{GENRE}}", JobScope::Album),
            (names(&["copies", "‹artist›", "‹year›", "‹genre›"]), vec![SongTag::Artist, SongTag::Year, SongTag::Genre]),
            "a song with no year never files under the row's"
        );
        // Every song agrees — folded as the server folds a folder name — so
        // even a compilation keeps the value.
        let agree = KeepView {
            album_songs: Some(vec![
                song("a.mp3", Some("AC/DC"), Some(1980), Some("Rock")),
                song("b.mp3", Some("AC:DC"), Some(1980), Some("Rock")),
                song("c.mp3", Some("AC-DC"), Some(1980), Some("Rock")),
            ]),
            subject: KeepSubject { track_artist: Some("AC-DC".into()), year: Some(1980), ..sampler.subject.clone() },
            ..sampler.clone()
        };
        assert_eq!(at(&agree, "{{ARTIST}}/{{YEAR}}/{{GENRE}}", JobScope::Album), (names(&["copies", "AC-DC", "1980", "Rock"]), vec![]));
        // No song has a year: the folder drops for every one alike, no slot.
        let undated = KeepView {
            album_songs: Some(vec![song("a.mp3", Some("AC-DC"), None, None), song("b.mp3", Some("AC-DC"), None, None)]),
            ..agree.clone()
        };
        assert_eq!(at(&undated, "{{ARTIST}}/{{YEAR}}", JobScope::Album), (names(&["copies", "AC-DC"]), vec![]));
        // A genre is known only where every song names one.
        let unnamed = KeepView {
            album_songs: Some(vec![song("a.mp3", Some("ALM"), None, Some("Rock")), song("b.mp3", Some("ALM"), None, None)]),
            ..agree.clone()
        };
        assert_eq!(at(&unnamed, "{{GENRE}}", JobScope::Album), (names(&["copies", "‹genre›"]), vec![SongTag::Genre]));
        // A listed song with no artist files under the album's credit, as the
        // copy files it: a compilation's "Various Artists" is a second
        // folder; an album credited to its row's artist keeps one.
        let alm = |album_artist: Option<&str>| KeepView {
            subject: KeepSubject { track_artist: Some("ALM".into()), album_artist: album_artist.map(str::to_string), ..sampler.subject.clone() },
            album_songs: Some(vec![song("a.mp3", Some("ALM"), None, None), song("b.mp3", Some("ALM"), None, None), song("c.mp3", None, None, None)]),
            ..sampler.clone()
        };
        assert_eq!(at(&alm(Some("Various Artists")), friends, JobScope::Album), (names(&["copies", "From friends", "‹artist›"]), vec![SongTag::Artist]));
        assert_eq!(at(&alm(None), friends, JobScope::Album), (names(&["copies", "From friends", "ALM"]), vec![]));
        // The listing as the peer sends it: a featured song's ARTIST tag as
        // written ("artist-display") is its folder, not the primary artist
        // its `artist` names; a genre tag the library split in two is no
        // folder this client can name.
        let listed = |filepath: &str, artist: &str, display: Option<&str>, genres: &[&str]| {
            let track: crate::api::types::Track = serde_json::from_value(serde_json::json!({
                "filepath": filepath, "metadata": { "artist": artist, "artist-display": display, "year": 2019, "genres": genres }
            }))
            .unwrap();
            KeepSong::of(&track)
        };
        let feat = KeepView {
            subject: KeepSubject { album_artist: None, album: Some("Borrowed Tapes".into()), ..sampler.subject.clone() },
            album_songs: Some(vec![
                listed("peerlib/Borrowed Tapes/01.mp3", "Boukmanflow", Some("Boukmanflow"), &["Hip-Hop"]),
                listed("peerlib/Borrowed Tapes/02.mp3", "Boukmanflow", None, &["Hip-Hop", "Rap"]),
                listed("peerlib/Night Market Sampler/02.mp3", "Boukmanflow", Some("Boukmanflow feat. ALM"), &["Hip-Hop", "Rap"]),
            ]),
            ..sampler.clone()
        };
        assert_eq!(
            at(&feat, "From friends/{{ARTIST}}/{{GENRE}}", JobScope::Album),
            (names(&["copies", "From friends", "‹artist›", "‹genre›"]), vec![SongTag::Artist, SongTag::Genre])
        );
        assert_eq!(at(&feat, friends, JobScope::Song), (names(&["copies", "From friends", "Boukmanflow feat. ALM"]), vec![]), "the song's own tag");
        let one_genre = KeepView {
            album_songs: Some(vec![listed("a.mp3", "ALM", None, &["Hip-Hop"]), listed("b.mp3", "ALM", None, &["Hip-Hop", " "])]),
            ..feat.clone()
        };
        assert_eq!(at(&one_genre, "{{GENRE}}", JobScope::Album), (names(&["copies", "Hip-Hop"]), vec![]));
        // The album's credit stays the album's whatever its songs' artists:
        // no albumArtist, so the row's artist, never a slot.
        assert_eq!(at(&feat, "{{ALBUMARTIST}}/{{ALBUM}}", JobScope::Album), (names(&["copies", "Boukmanflow", "Borrowed Tapes"]), vec![]));
        // An empty listing knows nothing: the row's rule stands — a slot for a
        // compilation, the row's own for an album that is none.
        let empty = KeepView { album_songs: Some(Vec::new()), ..sampler.clone() };
        assert_eq!(at(&empty, friends, JobScope::Album).1, vec![SongTag::Artist]);
        let empty_own = KeepView { album_songs: Some(Vec::new()), ..own.clone() };
        assert_eq!(at(&empty_own, "{{ARTIST}}/{{YEAR}}", JobScope::Album), (names(&["copies", "Boukmanflow", "2019"]), vec![]));
        // The song: its own values; GENRE its listing row's, else a slot.
        assert_eq!(at(&sampler, "{{ARTIST}}/{{YEAR}}/{{GENRE}}", JobScope::Song), (names(&["copies", "Boukmanflow", "2019", "‹genre›"]), vec![SongTag::Genre]));
        assert_eq!(at(&three, "{{ARTIST}}/{{GENRE}}", JobScope::Song), (names(&["copies", "Boukmanflow", "Hip-Hop"]), vec![]));
        // A song's landing says so, for its note; a wider copy's does not.
        assert!(sampler.landing_at(&place("{{GENRE}}"), JobScope::Song).one_song);
        assert!(!sampler.landing_at(&place("{{GENRE}}"), JobScope::Album).one_song);
        assert!(KeepView { scope: JobScope::Song, ..sampler.clone() }.landing_for("copies", "", "{{GENRE}}", &|_| true).one_song);
        // The artist scopes: today's rendering, GENRE before the cut a slot.
        let bouk = KeepView { subject: KeepSubject { album_artist: None, ..sampler.subject.clone() }, choice: KeepChoice::Artist, ..three.clone() };
        let landing = bouk.landing_at(&place("{{GENRE}}/{{ARTIST}}/{{YEAR}}/{{ALBUM}}"), JobScope::Artist);
        assert_eq!((landing.crumbs(), landing.per_album, landing.slots()), (names(&["copies", "‹genre›", "Boukmanflow"]), true, vec![SongTag::Genre]));
        // A layout naming none of them shows none.
        assert_eq!(at(&sampler, "{{ALBUMARTIST}}/{{ALBUM}}", JobScope::Album), (names(&["copies", "Various Artists", "Night Market Sampler"]), vec![]));
    }

    #[test]
    fn the_server_folder_is_the_songs_own_and_an_albums_where_every_song_shares_one() {
        let place = |layout: &str| CollectionDestination { vpath: "copies".into(), layout: layout.into(), ..CollectionDestination::default() };
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let song = |filepath: &str| KeepSong { filepath: filepath.into(), artist: Some("Boukmanflow".into()), year: Some(2019), genre: Some("Hip-Hop".into()), ..Default::default() };
        let tapes = KeepView {
            subject: subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None),
            choices: Vec::new(),
            choice: KeepChoice::Song,
            scope: JobScope::Song,
            missing: None,
            one_missing: None,
            destination: None,
            album_songs: None,
            landing: None,
            again: None,
            stage: KeepStage::Starting,
            copied_from: None,
            borrowed: None,
            left_out: None,
            cap: None,
            into: None,
            folder_tags: None,
        };
        let at = |view: &KeepView, layout: &str, scope: JobScope| {
            let landing = view.landing_at(&place(layout), scope);
            (landing.crumbs(), landing.slots())
        };
        let mirror = "{{SERVERFOLDER}}";
        // The song: its own folder on the peer, the peer's library dropped —
        // several folders, each its own; "From friends/…" under the words.
        assert_eq!(at(&tapes, mirror, JobScope::Song), (names(&["copies", "Boukmanflow", "Borrowed Tapes"]), vec![]));
        assert_eq!(at(&tapes, "From friends/{{SERVERFOLDER}}", JobScope::Song).0, names(&["copies", "From friends", "Boukmanflow", "Borrowed Tapes"]));
        // A song at the peer's root: no folder, as the server drops it.
        let root = KeepView { subject: KeepSubject { filepath: Some("peerlib/Late Train Verse.mp3".into()), ..tapes.subject.clone() }, ..tapes.clone() };
        assert_eq!(at(&root, "From friends/{{SERVERFOLDER}}", JobScope::Song), (names(&["copies", "From friends"]), vec![]));
        // The album before its listing answers: the row's folder.
        assert_eq!(at(&tapes, mirror, JobScope::Album), (names(&["copies", "Boukmanflow", "Borrowed Tapes"]), vec![]));
        // Every song in one folder: that folder, a slot nowhere.
        let one = KeepView {
            album_songs: Some(vec![song("peerlib/Boukmanflow/Borrowed Tapes/01.mp3"), song("peerlib/Boukmanflow/Borrowed Tapes/02.mp3")]),
            ..tapes.clone()
        };
        assert_eq!(at(&one, mirror, JobScope::Album), (names(&["copies", "Boukmanflow", "Borrowed Tapes"]), vec![]));
        // Spread over folders (a disc each): the slot, inside its folder's
        // own words; the other variables keep their values.
        let discs = KeepView {
            album_songs: Some(vec![song("peerlib/Boukmanflow/Borrowed Tapes/CD1/01.mp3"), song("peerlib/Boukmanflow/Borrowed Tapes/CD2/01.mp3")]),
            ..tapes.clone()
        };
        assert_eq!(
            at(&discs, "Copy of {{SERVERFOLDER}}/{{ARTIST}}", JobScope::Album),
            (names(&["copies", "Copy of ‹server folder›", "Boukmanflow"]), vec![SongTag::ServerFolder])
        );
        // Folders that clean to one name are still two folders: a song
        // under "AC/DC" sits two deep, one under "AC-DC" one.
        let split = KeepView { album_songs: Some(vec![song("peerlib/AC/DC/01.mp3"), song("peerlib/AC-DC/02.mp3")]), ..tapes.clone() };
        assert_eq!(at(&split, mirror, JobScope::Album).1, vec![SongTag::ServerFolder]);
        // The listing's own song decides, not the row: a row elsewhere on
        // the peer still lands where its album's songs sit.
        let moved = KeepView { subject: KeepSubject { filepath: Some("peerlib/Singles/02.mp3".into()), ..tapes.subject.clone() }, ..one.clone() };
        assert_eq!(at(&moved, mirror, JobScope::Album).0, names(&["copies", "Boukmanflow", "Borrowed Tapes"]));
        assert_eq!(at(&moved, mirror, JobScope::Song).0, names(&["copies", "Singles"]), "the song's copy: its own");
        // An album copy's slots in the path's order, the folder's among them.
        let three = KeepView { album_songs: Some(vec![KeepSong { artist: Some("ALM".into()), ..song("peerlib/a/1.mp3") }, song("peerlib/b/2.mp3")]), ..tapes.clone() };
        assert_eq!(at(&three, "{{SERVERFOLDER}}/{{ARTIST}}", JobScope::Album).1, vec![SongTag::ServerFolder, SongTag::Artist]);
        // The artist scopes: each album sits in its own folder — the first
        // folder naming it and all after are "one folder per album".
        let landing = tapes.landing_at(&place("From friends/{{SERVERFOLDER}}/x"), JobScope::Artist);
        assert_eq!((landing.crumbs(), landing.per_album, landing.slots()), (names(&["copies", "From friends"]), true, vec![]));
        // Only-missing naming one album: its songs' folder is not known here.
        let missing = KeepView { one_missing: Some(("Second Shift".into(), Some(2021))), ..tapes.clone() };
        let landing = missing.landing_at(&place("{{SERVERFOLDER}}/{{ALBUM}}"), JobScope::ArtistMissing);
        assert_eq!((landing.crumbs(), landing.per_album, landing.slots()), (names(&["copies", "‹server folder›", "Second Shift"]), false, vec![SongTag::ServerFolder]));
    }

    /// An ended copy's view on the subject's row, the window's action
    /// still to read: `again` stands for "Retry adds to" said.
    fn ended_view(job: PluginJob, choices: Vec<KeepChoice>, again: bool) -> KeepView {
        let outcome = job.outcome();
        KeepView {
            subject: subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None),
            choices: choices.into_iter().map(|c| (c, ChoiceSize::Album { songs: Fact::Unknown })).collect(),
            choice: KeepChoice::Song,
            scope: JobScope::Song,
            missing: None,
            one_missing: None,
            destination: None,
            album_songs: None,
            landing: None,
            again: again.then(Landing::default),
            stage: KeepStage::Job { job: Box::new(job), outcome, action: None },
            copied_from: None,
            borrowed: None,
            left_out: None,
            cap: None,
            into: None,
            folder_tags: None,
        }
    }

    #[test]
    fn the_windows_action_is_its_own_table_row_by_row() {
        let action = |job: PluginJob| window_action(&ended_view(job, Vec::new(), false));
        let none = || serde_json::json!([]);
        let many = |scope: &str, state: &str, result: serde_json::Value| {
            let key = format!("{scope}:x");
            job(2, &key, Some(scope), state, 1, result)
        };
        let counts = |total: u64, copied: u64, skipped: u64, failed: u64| {
            serde_json::json!({ "total": total, "copied": copied, "skipped": skipped, "failed": failed })
        };
        let owned = serde_json::json!([{ "from": "peerlib/Boukmanflow/Borrowed Tapes/01 - Rooftop Cypher.MP3", "why": "owned", "at": "music/Rooftop Cypher.mp3" }]);
        // One song copied: Play your copy.
        let song = job(1, "text:late", None, "done", 1, serde_json::json!({ "copied": { "vpath": "copies", "filepath": LANDED } }));
        assert_eq!(action(song), Some(JobAction::Play));
        // An album copied, no gap: Play your copy — the files it names to play.
        let album = many("album", "done", serde_json::json!({ "scope": "album", "songs": songs(landed(), owned.clone(), none()), "counts": counts(2, 1, 1, 0), "stopped": null }));
        assert_eq!(action(album), Some(JobAction::Play));
        // …and Close where the result names none (counts alone).
        let counted = many("album", "done", serde_json::json!({ "scope": "album", "counts": counts(3, 3, 0, 0), "stopped": null }));
        assert_eq!(action(counted), None);
        // Every album by / only the missing, no gap: Close — no one queue,
        // though the result names the files an album's would play.
        for scope in ["artist", "artist-missing"] {
            let mut walked = many(scope, "done", walk(scope, vec![("Borrowed Tapes", songs(landed(), none(), none()))], none()));
            walked.result.as_mut().unwrap()["counts"] = counts(1, 1, 0, 0);
            walked.result.as_mut().unwrap()["songs"] = songs(landed(), none(), none());
            assert!(matches!(walked.outcome(), JobOutcome::CopiedMany(_)), "{scope}");
            assert!(!walked.copy_result().kept_songs().is_empty(), "{scope}: files to play");
            assert_eq!(action(walked), None, "{scope}");
        }
        // A gap — a song failed, a listing failed, songs not reached: Retry, whatever the scope.
        let failed_song = serde_json::json!([{ "from": "x", "error": "gone" }]);
        let gap = many("album", "done", serde_json::json!({ "scope": "album", "songs": songs(landed(), none(), failed_song.clone()), "stopped": null }));
        assert_eq!(action(gap.clone()), Some(JobAction::Retry));
        assert_eq!(copies_action(&gap.outcome()), Some(JobAction::CopyAgain), "the Copies list keeps its own words");
        let mut artist_gap = many("artist", "done", walk("artist", vec![("Borrowed Tapes", songs(landed(), none(), failed_song))], none()));
        artist_gap.result.as_mut().unwrap()["counts"] = counts(2, 1, 0, 1);
        assert_eq!(action(artist_gap), Some(JobAction::Retry));
        let short = many("album", "done", serde_json::json!({ "scope": "album", "songs": songs(landed(), none(), none()), "counts": counts(3, 1, 0, 0), "stopped": null }));
        assert_eq!(action(short), Some(JobAction::Retry), "songs not reached are a gap");
        // Already yours: Close.
        let yours = job(3, "text:late", None, "done", 1, serde_json::json!({ "skipped": "owned", "existing": { "filepath": "music/x.mp3" } }));
        assert_eq!(action(yours), None);
        // A file in the way: Close — Retry only once the place moved.
        let exists = job(4, "text:late", None, "done", 1, serde_json::json!({ "skipped": "exists", "filepath": LANDED }));
        assert_eq!(action(exists.clone()), None);
        assert_eq!(window_action(&ended_view(exists.clone(), Vec::new(), true)), Some(JobAction::Retry));
        assert_eq!(copies_action(&exists.outcome()), None, "the Copies list's Retry would meet the same file");
        // Stopped (limit, busy, went away, a word this client does not know): Retry.
        for word in ["quota", "busy", "peer", "tea-break"] {
            let stopped = many("album", "done", serde_json::json!({ "scope": "album", "counts": counts(3, 1, 0, 0), "stopped": word }));
            assert_eq!(action(stopped), Some(JobAction::Retry), "{word}");
        }
        // Refused by the peer — a song's stop word, an older server's failure, a many-song stop: Close.
        let refused_song = job(5, "text:late", None, "done", 1, serde_json::json!({ "stopped": "refused" }));
        let mut refused_old = job(5, "text:late", None, "failed", 1, serde_json::Value::Null);
        refused_old.error = Some("Stage Peer B does not allow copies with this server's key".into());
        let refused_many = many("album", "done", serde_json::json!({ "scope": "album", "counts": counts(3, 1, 0, 0), "stopped": "refused" }));
        for refused in [refused_song, refused_old, refused_many] {
            assert!(matches!(refused.outcome(), JobOutcome::Refused { .. }));
            assert_eq!(action(refused), None);
        }
        // Failed: Retry. Cancelled: Add again (StartAgain).
        assert_eq!(action(job(6, "text:late", None, "failed", 1, serde_json::Value::Null)), Some(JobAction::Retry));
        assert_eq!(action(job(7, "text:late", None, "cancelled", 1, serde_json::Value::Null)), Some(JobAction::StartAgain));
        // Nothing to copy, nothing of theirs: Close.
        let nothing = many("album", "done", serde_json::json!({ "scope": "album", "counts": counts(0, 0, 0, 0), "stopped": null }));
        assert!(matches!(nothing.outcome(), JobOutcome::Nothing { appearances: 0, .. }));
        assert_eq!(action(nothing), None);
        let guest = many("artist", "done", walk("artist", vec![], serde_json::json!([{ "name": "Block Party Comp", "why": "appearance" }])));
        assert!(matches!(guest.outcome(), JobOutcome::Nothing { appearances: 1, .. }));
        assert_eq!(action(guest), None);
        // A result this client cannot read, a state it does not know: Close.
        assert_eq!(action(job(8, "text:late", None, "done", 1, serde_json::json!({}))), None);
        assert_eq!(action(job(9, "text:late", None, "paused", 1, serde_json::Value::Null)), None);
        // Live: the cancel, as before; stopping, nothing to press.
        assert_eq!(action(job(10, "text:late", None, "queued", 1, serde_json::Value::Null)), Some(JobAction::Cancel));
        let mut stopping = job(11, "text:late", None, "running", 1, serde_json::Value::Null);
        assert_eq!(action(stopping.clone()), Some(JobAction::Cancel));
        stopping.cancel_requested = true;
        assert_eq!(action(stopping), None);
    }

    #[test]
    fn a_file_in_the_way_from_a_server_that_pinned_no_place_retries_only_into_another_folder() {
        // A job from before the pin: neither its params nor its result name
        // the place it ran with. The file in the way sits in copies ›
        // Boukmanflow › Borrowed Tapes.
        let exists = job(4, "text:late", None, "done", 1, serde_json::json!({ "skipped": "exists", "filepath": LANDED }));
        assert!(exists.params.destination.is_none() && exists.copy_result().destination.is_none());
        let place = |layout: &str| CollectionDestination { vpath: "copies".into(), base: String::new(), layout: layout.into(), source: None };
        let view = |layout: &str| KeepView { destination: Some(place(layout)), ..ended_view(exists.clone(), Vec::new(), false) };
        // The place now renders that same folder: Retry would meet the same
        // file — nothing said, Close.
        let same = view("{{ALBUMARTIST}}/{{ALBUM}}");
        assert_eq!(keep_again_landing(&same), None);
        assert_eq!(window_action(&KeepView { again: keep_again_landing(&same), ..same }), None);
        // Another folder: where Retry adds, and Retry.
        let moved = view("{{PEER}}/{{ALBUMARTIST}}/{{ALBUM}}");
        let again = keep_again_landing(&moved).expect("Retry adds elsewhere");
        assert_eq!(again.crumbs(), ["copies", "Stage Peer B", "Boukmanflow", "Borrowed Tapes"]);
        assert_eq!(window_action(&KeepView { again: Some(again), ..moved }), Some(JobAction::Retry));
        // A place nothing can compare (a slot each song fills): no Retry.
        let slotted = view("{{GENRE}}/{{ALBUM}}");
        assert_eq!(keep_again_landing(&slotted), None);
        // A failure from such a server still says where Retry adds: an
        // unknown place may differ.
        let failed = job(6, "text:late", None, "failed", 1, serde_json::Value::Null);
        let failed = KeepView { destination: Some(place("{{ALBUMARTIST}}/{{ALBUM}}")), ..ended_view(failed, Vec::new(), false) };
        assert!(keep_again_landing(&failed).is_some());
    }

    #[test]
    fn choose_again_stands_after_an_outcome_only_where_two_choices_are_offered_and_never_after_a_refusal_or_a_cancel() {
        let two = || vec![KeepChoice::Song, KeepChoice::Album];
        let stands = |job: PluginJob, choices: Vec<KeepChoice>| ended_view(job, choices, false).choose_again();
        let kept = job(1, "text:late", None, "done", 1, serde_json::json!({ "copied": { "vpath": "copies", "filepath": LANDED } }));
        assert!(stands(kept.clone(), two()));
        assert!(!stands(kept, vec![KeepChoice::Album]), "a compilation's tile: one choice, nothing to choose again");
        for (state, result) in [("failed", serde_json::Value::Null), ("done", serde_json::json!({ "skipped": "exists", "filepath": LANDED })), ("done", serde_json::json!({ "skipped": "owned" }))] {
            assert!(stands(job(2, "text:late", None, state, 1, result), two()), "{state}");
        }
        let nothing = job(3, "album:x", Some("album"), "done", 1, serde_json::json!({ "counts": { "total": 0, "copied": 0, "skipped": 0, "failed": 0 } }));
        assert!(stands(nothing, two()));
        // Never after a refusal, a cancel, a result this client cannot read — nor while it runs.
        let refused = job(4, "text:late", None, "done", 1, serde_json::json!({ "stopped": "refused" }));
        for job in [
            refused,
            job(5, "text:late", None, "cancelled", 1, serde_json::Value::Null),
            job(6, "text:late", None, "done", 1, serde_json::json!({})),
            job(7, "text:late", None, "running", 1, serde_json::Value::Null),
            job(8, "text:late", None, "queued", 1, serde_json::Value::Null),
        ] {
            assert!(!stands(job.clone(), two()), "{:?}", job.outcome());
        }
        // Not a copy's window at all: the choice, a right gone.
        let choosing = KeepView { stage: KeepStage::Choose { verb: Some(KeepVerb::Song), other: None }, ..ended_view(job(9, "text:late", None, "failed", 1, serde_json::Value::Null), two(), false) };
        assert!(!choosing.choose_again());
        assert!(!KeepView { stage: KeepStage::Off(OffWhy::Jobs), ..choosing }.choose_again());
    }

    #[test]
    fn a_403_names_the_upload_right_by_its_words_and_anything_else_the_right_to_start_copies() {
        assert_eq!(off_for_403("Uploading Disabled"), OffWhy::Uploads);
        assert_eq!(off_for_403("your account may not UPLOAD here"), OffWhy::Uploads);
        assert_eq!(off_for_403("jobs are not allowed for this account"), OffWhy::Jobs);
        assert_eq!(off_for_403(""), OffWhy::Jobs);
    }

    #[test]
    fn an_albums_songs_play_in_its_listings_order_and_else_the_results() {
        let file = |title: &str| format!("peerlib/Boukmanflow/Borrowed Tapes/{title}.mp3");
        let copied = |title: &str| serde_json::json!({ "from": file(title), "vpath": "copies", "filepath": format!("copies/Boukmanflow/Borrowed Tapes/{title}.mp3"), "title": title, "artist": "Boukmanflow", "album": "Borrowed Tapes" });
        let result = serde_json::json!({ "scope": "album", "stopped": null, "songs": songs(
            serde_json::json!([copied("Rooftop Cypher"), copied("Late Train Verse")]),
            serde_json::json!([
                { "from": file("Corner Store Loop"), "why": "owned", "at": "music/Boukmanflow/Corner Store Loop.mp3" },
                { "from": file("Skit"), "why": "exists", "at": "copies/Boukmanflow/Borrowed Tapes/Skit.mp3" },
                { "from": file("Outro"), "why": "owned" },
            ]),
            serde_json::json!([]),
        ) });
        let album = job(2, "album:tapes", Some("album"), "done", 1, result);
        let paths = |tracks: Vec<Track>| tracks.into_iter().map(|t| t.filepath).collect::<Vec<_>>();
        let listing: Vec<KeepSong> = ["Late Train Verse", "Corner Store Loop", "Rooftop Cypher"]
            .iter()
            .map(|title| KeepSong { filepath: file(title), ..KeepSong::default() })
            .collect();
        // The listing's order; a file in the way and an owned song with no
        // file named are never played.
        assert_eq!(
            paths(album_tracks(&album, Some(&listing))),
            ["copies/Boukmanflow/Borrowed Tapes/Late Train Verse.mp3", "music/Boukmanflow/Corner Store Loop.mp3", "copies/Boukmanflow/Borrowed Tapes/Rooftop Cypher.mp3"]
        );
        // No listing: the result's order, what it copied first.
        assert_eq!(
            paths(album_tracks(&album, None)),
            ["copies/Boukmanflow/Borrowed Tapes/Rooftop Cypher.mp3", "copies/Boukmanflow/Borrowed Tapes/Late Train Verse.mp3", "music/Boukmanflow/Corner Store Loop.mp3"]
        );
        // A song the listing does not name goes after the ones it does.
        let partial = &listing[1..2];
        assert_eq!(paths(album_tracks(&album, Some(partial)))[0], "music/Boukmanflow/Corner Store Loop.mp3");
        // The tags a copied song landed with; one already there, its album.
        let tracks = album_tracks(&album, Some(&listing));
        assert_eq!((tracks[0].metadata.title.as_deref(), tracks[0].metadata.artist.as_deref()), (Some("Late Train Verse"), Some("Boukmanflow")));
        assert_eq!((tracks[1].metadata.title.as_deref(), tracks[1].metadata.album.as_deref()), (None, Some("Borrowed Tapes")));
    }
}
