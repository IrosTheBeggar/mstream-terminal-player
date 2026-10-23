//! The Discover room's plug-ins in the App: the Keep a copy gate, the jobs a
//! copy runs as and the marks they leave on the Connected servers rows and
//! tiles, the Keep a copy flow, the Copies list and Open elsewhere
//! (docs/ux-contracts/discovery-page.md, clauses 100–149).
//!
//! Nothing here draws. The GUI reads [`KeepState`] through the `keep_*`,
//! `copies_*` and `links_*` methods and calls them for every key and click;
//! the room's numbers (counts, percentages, steps) reach it only through
//! the Keep flow's view and the Copies list — the room itself is handed
//! words' worth of state: a [`JobMark`] per row and tile and a
//! [`JobsIndicator`] (clause 113, the 2026-10-19 log: no counts on the
//! page).
//!
//! The state lives inside [`super::taste::TasteState`], so it follows the
//! session the room was filled under: a server switch drops it with the
//! answer (clause 31).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use super::taste::TastePeerAlbum;
use super::*;
use crate::api::PluginFailure;
use crate::api::types::{
    COPY_PLUGIN, CancelAnswer, CollectionDestination, CollectionDestinationView, DiscoveryPluginsResponse, JobAction,
    JobLookup, JobMark, JobOutcome, JobParams, JobScope, JobStarted, JobState, Landed, Landing, LayoutTags, PluginJob,
    PluginJobList, PluginLink, SongTag, TasteLead, artist_ownership, job_for_album, missing_albums, safe_file_name,
    sanitize_segment,
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
/// it (clause 112). Set by the GUI each frame; the TUI never sets it, so
/// it never polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeepWatch {
    /// Not the Discover room: no poll.
    #[default]
    Away,
    /// The room, no Keep or Copies modal.
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
}

impl RowJobs {
    /// A lookup's answer as a row's jobs: the first job per slot, as the
    /// server orders them (newest first) — copies only, Discover's scopes
    /// only (never a folder job, clause 104).
    pub fn from_lookup(found: &JobLookup) -> RowJobs {
        let mut held: BTreeMap<JobSlot, PluginJob> = BTreeMap::new();
        for job in &found.jobs {
            let Some(scope) = job.scope().filter(|s| s.discover()) else { continue };
            if job.plugin != COPY_PLUGIN {
                continue;
            }
            held.entry((job.plugin.clone(), scope)).or_insert_with(|| job.clone());
        }
        RowJobs { keys: found.row_keys(), held }
    }

    /// A job from the poll, a start or a cancel, merged by key: a job
    /// under one of this row's keys replaces the one its slot holds when it
    /// is as new or newer. True when it landed.
    pub fn merge(&mut self, job: &PluginJob) -> bool {
        let Some(key) = job.key.as_deref() else { return false };
        let Some(scope) = job.scope().filter(|s| s.discover()) else { return false };
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
}

/// A folder's folders as the picker asked them (clause 143).
#[derive(Debug, Clone, PartialEq)]
pub enum Folders {
    Asking,
    /// Their names, in the server's order.
    Listed(Vec<String>),
    Failed(PluginFailure),
}

/// The picker's asks (clauses 142–144) — "Where files land", the GUI's:
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

/// What a Keep a copy flow (or Open elsewhere) is about: a connected
/// server's row, or its tile on the strip (album focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepTarget {
    Row { group: usize, row: usize },
    /// By its index in [`super::taste::TasteState::peer_albums`].
    Tile(usize),
}

/// What a target named when a surface that stays open took it — the
/// track-actions sheet, the tile's list: the row's server and file, or
/// the tile's album key, with the server's name as it read then. A new
/// peers answer can land under such a surface (the old answer stands until
/// the new one does), and its indices then name other rows; the pin finds
/// what the surface shows wherever it now stands, or nothing, so Keep a
/// copy and Open elsewhere always act on the row the user acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepPin {
    Row { peer: i64, filepath: String, server: String },
    Tile { key: (i64, String, String), server: String },
}

impl KeepPin {
    /// The server the pinned row or album is from, as it read when pinned.
    pub fn server(&self) -> &str {
        match self {
            KeepPin::Row { server, .. } | KeepPin::Tile { server, .. } => server,
        }
    }
}

/// The Keep modal's choice (clause 106): the song, its album, or every
/// album by its artist ("only the albums you don't have" is the artist
/// choice's checkbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepChoice {
    Song,
    Album,
    Artist,
}

impl KeepChoice {
    fn scope(self, only_missing: bool) -> JobScope {
        match self {
            KeepChoice::Song => JobScope::Song,
            KeepChoice::Album => JobScope::Album,
            KeepChoice::Artist if only_missing => JobScope::ArtistMissing,
            KeepChoice::Artist => JobScope::Artist,
        }
    }
}

/// What a flow copies, frozen when it opens: an answer that lands while the
/// modal is up never changes what it copies.
#[derive(Debug, Clone, PartialEq)]
pub struct KeepSubject {
    /// The connected server's id, and its name.
    pub peer: i64,
    pub server: String,
    /// The song's title; `None` for a tile — album focus (clause 106).
    pub title: Option<String>,
    pub track_artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<i32>,
    pub duration: Option<f64>,
    /// The song's path on its server.
    pub filepath: Option<String>,
    /// The cover's file name on its server.
    pub art: Option<String>,
    /// What every job of the flow sends: the row's raw taste JSON exactly
    /// as it came — a tile's best row (clause 103).
    pub rec: serde_json::Value,
}

impl KeepSubject {
    fn of(peer: i64, server: &str, lead: &TasteLead, album_focus: bool) -> KeepSubject {
        KeepSubject {
            peer,
            server: server.to_string(),
            title: if album_focus { None } else { Some(lead.title.clone().unwrap_or_default()) },
            track_artist: lead.artist.clone(),
            album: lead.album.clone(),
            album_artist: lead.album_artist.clone(),
            year: lead.year,
            duration: lead.duration,
            filepath: lead.filepath.clone(),
            art: lead.album_art.clone(),
            rec: lead.raw.clone(),
        }
    }

    pub fn album_focus(&self) -> bool {
        self.title.is_none()
    }

    /// The credit the album goes by — its album artist, else the track
    /// artist: the tile's artist row, and what Album needs beside the album
    /// name (clause 104).
    pub fn album_credit(&self) -> Option<&str> {
        self.album_artist.as_deref().or(self.track_artist.as_deref()).filter(|a| !a.trim().is_empty())
    }

    /// The album is credited to someone other than this song's artist (a
    /// compilation): "Every album by" would copy the track artist's albums,
    /// not this album's, so a tile never offers it.
    pub fn compilation(&self) -> bool {
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
}

/// Which choices a subject offers under a listing (clauses 104, 106): the
/// song on a row only (album focus has no "This song"); Album when the
/// album and its credit are both named; every album by the artist when the
/// track artist is named and — on a tile — the album is not a compilation;
/// each only where the copy plug-in runs that scope. Never Folder.
pub fn keep_choices(listing: &DiscoveryPluginsResponse, subject: &KeepSubject) -> Vec<KeepChoice> {
    let named = |v: Option<&str>| v.is_some_and(|s| !s.trim().is_empty());
    let mut out = Vec::new();
    if !subject.album_focus() && listing.copies(JobScope::Song) {
        out.push(KeepChoice::Song);
    }
    if named(subject.album.as_deref()) && subject.album_credit().is_some() && listing.copies(JobScope::Album) {
        out.push(KeepChoice::Album);
    }
    let artist_ok = named(subject.track_artist.as_deref()) && !(subject.album_focus() && subject.compilation());
    if artist_ok && listing.copies(JobScope::Artist) {
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
    /// Following this job to its outcome.
    Following(i64),
    /// A finished wider copy of this server speaks for the choice it
    /// opened on ([`covered_view`]): its outcome for that choice, with the
    /// choice's own buttons — Retry and Copy again start the choice's copy.
    Covered(i64),
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
    /// Its own lookup, when its rows had none yet.
    lookup_seq: Option<u64>,
    /// Its subject's jobs, merged as the room's rows are.
    pub jobs: RowJobs,
    pub phase: KeepPhase,
    /// The place its last start sent ([`ApiCmd::JobStart`]'s): what LANDS
    /// IN reads while the start is out.
    sent: Option<Option<CollectionDestination>>,
}

impl KeepFlow {
    /// A covered choice looks again whenever its jobs change: its own copy
    /// started since (live, or newer) takes over, and a wider copy that no
    /// longer speaks for it gives the choice back.
    fn resettle(&mut self) {
        if matches!(self.phase, KeepPhase::Covered(_)) {
            self.phase = opening_phase(&self.jobs, &self.subject, self.choice.scope(self.only_missing))
                .unwrap_or(KeepPhase::Choose);
        }
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
    /// "1 album · you have 3": the other server's albums of the artist and
    /// how many the library holds.
    Artist { albums: Fact<usize>, have: Fact<usize> },
}

/// The primary button's verb, which follows the choice (clause 106): Add
/// song · Add album · Add artist · Add 1 album.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepVerb {
    Song,
    Album,
    Artist,
    /// "Add 1 album" — only the ones you don't have.
    Missing(usize),
}

/// Where a flow's view stands (clauses 107–110).
#[derive(Debug, Clone, PartialEq)]
pub enum KeepStage {
    /// The radios and the primary. `other` names the server whose live
    /// copy of the same album or artist covers this one — the primary is
    /// hidden until it ends (clause 121).
    Choose { verb: Option<KeepVerb>, other: Option<String> },
    Starting,
    Refused { failure: PluginFailure, verb: Option<KeepVerb> },
    /// A job of this flow's scope: its outcome, and the action it offers.
    Job { job: Box<PluginJob>, outcome: JobOutcome, action: Option<JobAction> },
    /// The account may put files nowhere any more (a re-read, a start's
    /// 409): one plain line and Close (clause 147).
    Nowhere,
}

/// Everything the Keep modal draws, worked out (clauses 106–110).
#[derive(Debug, Clone, PartialEq)]
pub struct KeepView {
    pub subject: KeepSubject,
    pub choices: Vec<(KeepChoice, ChoiceSize)>,
    pub choice: KeepChoice,
    pub scope: JobScope,
    /// "Only the albums you don't have": `None` hidden — never grey — else
    /// whether it is ticked and how many albums it would add.
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
    /// Where Retry or Copy again would land — the account's place now —
    /// where that is not the place the copy ran with (the webapp's
    /// `retryPath`, clause 145): drawn under LANDS IN, so a press never
    /// lands somewhere the modal did not show. `None` where it is the same.
    pub again: Option<Landing>,
    pub stage: KeepStage,
}

impl KeepView {
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
    /// - **its album**: ALBUM, ALBUMARTIST (its credit — albumArtist, else
    ///   the row's artist — so a compilation stays in one folder) and PEER
    ///   as the row says; ARTIST and YEAR the value every song of the
    ///   listing files under ([`agreed`]: a song with no artist under the
    ///   credit, as the copy files it; one with no year under none, its
    ///   folder dropped) — before the listing answers, the row's own unless
    ///   it is a compilation — else a slot; GENRE a slot unless every song
    ///   names the same one;
    /// - **the artist scopes**: the track artist as ARTIST and ALBUMARTIST,
    ///   never a compilation's credit, and no album or year — each album
    ///   has its own — unless only-missing takes one album, which is named
    ///   with its year (the webapp's `scopeTarget` `oneAlbum`); GENRE a
    ///   slot.
    fn scope_tags(&self, scope: JobScope) -> (LayoutTags, bool) {
        let s = &self.subject;
        let peer = Some(s.server.clone());
        let year = s.year.filter(|y| *y > 0).map(|y| y.to_string());
        match scope {
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
                    slots: vec![SongTag::Genre],
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
                let slots = [(&artist, SongTag::Artist), (&year, SongTag::Year), (&genre, SongTag::Genre)]
                    .into_iter()
                    .filter_map(|(value, slot)| value.is_none().then_some(slot))
                    .collect();
                let tags = LayoutTags {
                    artist: artist.flatten(),
                    album: s.album.clone(),
                    year: year.flatten(),
                    genre: genre.flatten(),
                    album_artist: credit,
                    peer,
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
                    slots,
                };
                (tags, false)
            }
        }
    }

    /// Where a copy of `scope` lands at a place.
    fn landing_at(&self, place: &CollectionDestination, scope: JobScope) -> Landing {
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

    /// The song's file as it would land — the PREVIEW's second row, a
    /// song's alone.
    pub fn file_name(&self) -> Option<String> {
        if self.scope != JobScope::Song {
            return None;
        }
        self.subject.filepath.as_deref().map(safe_file_name)
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
/// album, or "every album by {artist}" (only the ones you don't have).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTitle {
    Song(String),
    Album(String),
    Artist { artist: String, missing_only: bool },
}

impl CopyTitle {
    fn of(job: &PluginJob) -> CopyTitle {
        let text = |k: &str| job.rec_text(k).unwrap_or_default();
        match job.scope() {
            Some(JobScope::Album) => CopyTitle::Album(text("album")),
            Some(JobScope::Artist) => CopyTitle::Artist { artist: text("artist"), missing_only: false },
            Some(JobScope::ArtistMissing) => CopyTitle::Artist { artist: text("artist"), missing_only: true },
            _ => CopyTitle::Song(job.rec_text("title").unwrap_or_else(|| text("album"))),
        }
    }

    /// The plain name: a status note's tail.
    pub fn name(&self) -> &str {
        match self {
            CopyTitle::Song(s) | CopyTitle::Album(s) => s,
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

/// Everything the room's plug-ins hold, reset with the room's answer.
#[derive(Debug, Clone, Default)]
pub struct KeepState {
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
                self.rows.values().flat_map(|r| r.held.values()).find(|j| j.id == id)
            })
        })
    }

    /// Fold one job in everywhere it belongs; true when it is news — seen
    /// live, now finished. Only the copy plug-in's jobs in Discover's own
    /// scopes: a folder copy (the webapp's peer browser) or a scope this
    /// client does not know is no Discover copy — it never lists, lights
    /// the indicator, keeps the poll going or words a note, since nothing
    /// here could retry it (never Folder, clause 104).
    fn fold(&mut self, job: &PluginJob) -> bool {
        if !discover_copy(job) {
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
        if let Some(flow) = self.flow.as_mut() {
            flow.jobs.merge(job);
            flow.resettle();
        }
        if job.live() {
            self.book.seen_live.insert(job.id);
            return false;
        }
        if self.book.seen_live.remove(&job.id) {
            self.book.cancelling.remove(&job.id);
            self.news.push(job.clone());
            return true;
        }
        false
    }

    /// Anything still moving: a listed or held job, or a start out.
    fn any_live(&self) -> bool {
        self.book.jobs.values().any(PluginJob::live)
            || self.rows.values().flat_map(|r| r.held.values()).any(PluginJob::live)
            || self.flow.as_ref().is_some_and(|f| {
                matches!(f.phase, KeepPhase::Starting { .. }) || f.jobs.held.values().any(PluginJob::live)
            })
            || !self.book.starts.is_empty()
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

/// A job Discover offers, lists and retries: the copy plug-in's, in one of
/// its own scopes — never a folder copy, nor a scope this client does not
/// know (clause 104).
fn discover_copy(job: &PluginJob) -> bool {
    job.plugin == COPY_PLUGIN && job.scope().is_some_and(JobScope::discover)
}

/// A Copies row's action (clause 117): the outcome's, except that Copy
/// again — which takes only the gaps — stands only where a gap is left (a
/// song failed, an album's listing failed, songs not reached).
pub fn copies_action(outcome: &JobOutcome) -> Option<JobAction> {
    match outcome {
        JobOutcome::CopiedMany(n) if n.failed == 0 && n.albums_failed == 0 && n.total <= n.copied + n.skipped => None,
        other => other.action(),
    }
}

/// Where Retry or Copy again would land (clause 145): the account's place
/// now — what they send — for an ended copy that offers either, where that
/// is not the place the copy ran with (its result's, else its pin) or no
/// such place is known; `None` where it is the same.
fn keep_again_landing(view: &KeepView) -> Option<Landing> {
    let KeepStage::Job { job, action: Some(JobAction::Retry | JobAction::CopyAgain), .. } = &view.stage else {
        return None;
    };
    let now = view.destination.as_ref().filter(|_| !job.live())?;
    let ran = job.copy_result().destination.or_else(|| job.params.destination.clone().flatten());
    if ran.is_some_and(|ran| ran.same_place(now)) {
        return None;
    }
    Some(view.landing_at(now, job.scope().unwrap_or(view.scope)))
}

/// The poll's wait after `failures` failures in a row: the beat doubled
/// each time, never past [`KEEP_POLL_MAX`].
pub fn keep_backoff(beat: Duration, failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures.min(16)).unwrap_or(u32::MAX);
    beat.saturating_mul(factor).min(KEEP_POLL_MAX)
}

impl App {
    // ── The gate (clause 101) ───────────────────────────────────────────────

    /// The listing says copies run here: the ping's `discoveryPlugins`, the
    /// user's own session, the copy plug-in listed and jobs allowed — what
    /// the lookups, the marks and the poll need.
    pub fn keep_listed(&self) -> bool {
        self.connected
            && self.capabilities.discovery_plugins
            && self.session.peer.is_none()
            && self.taste.keep.gate.plugins.as_ref().is_some_and(|p| p.jobs.allowed && p.copy_plugin().is_some())
    }

    /// Every Keep a copy entry point's gate (clause 101): the listing's
    /// half, the account's upload right and a library to land in. Hidden,
    /// never grey, while any is missing.
    pub fn keep_gate(&self) -> bool {
        self.keep_listed() && self.capabilities.uploads && self.keep_destination().is_some()
    }

    /// Where copies land, while the server names somewhere.
    pub fn keep_destination(&self) -> Option<&CollectionDestination> {
        self.keep_dest_view()?.destination.as_ref()
    }

    /// The destination's view as last read — what the picker draws from:
    /// the libraries, their templates, the variables (clause 142).
    pub fn keep_dest_view(&self) -> Option<&CollectionDestinationView> {
        self.taste.keep.gate.view.as_ref()
    }

    /// Open elsewhere's gate (clause 140): the flag and a links plug-in.
    pub fn links_gate(&self) -> bool {
        self.connected
            && self.capabilities.discovery_plugins
            && self.session.peer.is_none()
            && self.taste.keep.gate.plugins.as_ref().is_some_and(|p| p.links_plugin().is_some())
    }

    /// Ask the gate (a visit's first frame, clause 101): the listing, then
    /// the destination behind it. One at a time; nothing without the flag.
    pub(crate) fn keep_ask_gate(&mut self) -> Vec<Effect> {
        if !self.connected || !self.capabilities.discovery_plugins || self.session.peer.is_some() {
            return Vec::new();
        }
        if self.taste.keep.gate.asking {
            return Vec::new();
        }
        let seq = self.taste.keep.next_seq();
        let gate = &mut self.taste.keep.gate;
        gate.asking = true;
        gate.asked = seq;
        vec![Effect::Api(ApiCmd::KeepGate { generation: self.session_gen })]
    }

    /// The subject a target names, from the answer on screen.
    fn keep_subject(&self, target: KeepTarget) -> Option<KeepSubject> {
        match target {
            KeepTarget::Row { group, row } => {
                let (peer, lead) = self.taste.peer_row(group, row)?;
                lead.filepath.as_ref()?;
                let server = self.taste.peer_name(group).unwrap_or_default().to_string();
                Some(KeepSubject::of(peer, &server, lead, false))
            }
            KeepTarget::Tile(index) => {
                let album = self.taste.peer_albums().into_iter().nth(index)?;
                let &(group, row) = album.rows.first()?;
                let (_, lead) = self.taste.peer_row(group, row)?;
                let mut subject = KeepSubject::of(album.peer, &album.server, lead, true);
                // The tile's own spellings: its album and its credit.
                subject.album = Some(album.album.clone());
                subject.album_artist = album.album_artist.clone();
                subject.art = album.art.clone().or(subject.art);
                Some(subject)
            }
        }
    }

    /// Pin what `target` names now, for a surface that stays open over a
    /// new answer (the sheet, the tile's list): `None` where it names
    /// nothing a copy could start from.
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
        }
    }

    /// Where the pinned row or album stands in the answer on screen —
    /// the same server's same file, the same server's same album and
    /// credit — or `None` once a new answer left it out: the surface then
    /// offers nothing that would start a copy or ask for links.
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
        }
    }

    /// The choices a target's Keep a copy offers — empty, and every entry
    /// point to it hidden, while the gate is closed (clauses 101–104).
    pub fn keep_choices(&self, target: KeepTarget) -> Vec<KeepChoice> {
        if !self.keep_gate() {
            return Vec::new();
        }
        let (Some(listing), Some(subject)) = (self.taste.keep.gate.plugins.as_ref(), self.keep_subject(target)) else {
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
        let jobs = self.taste.keep.rows.get(&(group, row))?;
        let (peer, lead) = self.taste.peer_row(group, row)?;
        row_mark(jobs, peer, lead.album.as_deref())
    }

    /// A Connected servers tile's mark.
    pub fn keep_tile_mark(&self, album: &TastePeerAlbum) -> Option<JobMark> {
        if !self.keep_listed() {
            return None;
        }
        let rows = album.rows.iter().filter_map(|at| self.taste.keep.rows.get(at));
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

    // ── Lookups and the poll (clauses 111–112) ──────────────────────────────

    /// Look the Connected servers rows up (clause 111): after every peers
    /// answer, when the gate opens over one, after a copy ends and after a
    /// clear. Each row's raw JSON goes unchanged; the rows a new answer
    /// replaced lose their jobs at once.
    pub(crate) fn keep_lookup_rows(&mut self) -> Vec<Effect> {
        let landed = self.taste.keep.peers_landed;
        if !self.keep_listed() {
            return Vec::new();
        }
        let lines = self.taste.peer_lines();
        let recs: Vec<serde_json::Value> = lines
            .iter()
            .filter_map(|&(g, r)| self.taste.peer_row(g, r).map(|(_, lead)| lead.raw.clone()))
            .collect();
        if recs.is_empty() || recs.len() != lines.len() {
            self.taste.keep.rows.clear();
            return Vec::new();
        }
        let seq = self.taste.keep.next_seq();
        self.taste.keep.lookup = Some(RowsLookup { seq, rows: lines, landed });
        vec![Effect::Api(ApiCmd::JobsLookup { seq, generation: self.session_gen, recs })]
    }

    /// A peers answer landed (clause 111): the rows are other songs now —
    /// their jobs go at once, and the new rows are looked up.
    pub(crate) fn keep_peers_landed(&mut self) -> Vec<Effect> {
        let keep = &mut self.taste.keep;
        keep.peers_landed += 1;
        keep.rows.clear();
        keep.lookup = None;
        self.keep_lookup_rows()
    }

    /// Ask the caller's jobs now (the Copies modal's first ask, a refresh
    /// after a cancel lost its race or a clear) — one ask at a time.
    fn keep_ask_jobs(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        if !self.keep_listed() || self.taste.keep.book.asking.is_some() {
            return Vec::new();
        }
        let seq = self.taste.keep.next_seq();
        let book = &mut self.taste.keep.book;
        book.asking = Some(seq);
        book.last_ask = Some(now);
        vec![Effect::Api(ApiCmd::Jobs { seq, generation: self.session_gen })]
    }

    /// What the GUI shows of the copies — the poll's beat (clause 112).
    pub(crate) fn keep_watch(&mut self, watch: KeepWatch) {
        self.taste.keep.watch = watch;
    }

    /// The open Keep a copy modal shows a copy of its own moving — its
    /// start out, the job it follows live, a live job in one of its
    /// subject's slots: the modal's quick beat holds only then (clause
    /// 112); a modal still choosing over someone else's copy keeps the
    /// room's.
    pub fn keep_flow_live(&self) -> bool {
        self.taste.keep.flow_live()
    }

    /// Once a tick: the jobs poll, only while a copy is live — every 1.5 s
    /// with a Keep or Copies modal open, every 4 s in the room, never
    /// elsewhere — backing off, doubling up to a minute, on failures.
    pub(crate) fn keep_tick(&mut self, now: crate::clock::Instant) -> Vec<Effect> {
        if self.taste.generation != self.session_gen || !self.keep_listed() {
            return Vec::new();
        }
        let keep = &self.taste.keep;
        let beat = match keep.watch {
            KeepWatch::Away => return Vec::new(),
            KeepWatch::Room => KEEP_POLL_ROOM,
            KeepWatch::Modal => KEEP_POLL_MODAL,
        };
        if keep.book.asking.is_some() || !keep.any_live() {
            return Vec::new();
        }
        let wait = keep_backoff(beat, keep.book.failures);
        if keep.book.last_ask.is_some_and(|at| now.saturating_duration_since(at) < wait) {
            return Vec::new();
        }
        self.keep_ask_jobs(now)
    }

    /// How long until the poll tries again after a failure — the Copies
    /// modal's "next try in 8 s" — `None` while nothing failed.
    pub fn keep_next_try(&self, now: crate::clock::Instant) -> Option<Duration> {
        let keep = &self.taste.keep;
        keep.book.failed.as_ref()?;
        let beat = if keep.watch == KeepWatch::Modal { KEEP_POLL_MODAL } else { KEEP_POLL_ROOM };
        let wait = keep_backoff(beat, keep.book.failures);
        let since = keep.book.last_ask.map(|at| now.saturating_duration_since(at)).unwrap_or_default();
        Some(wait.saturating_sub(since))
    }

    /// The jobs seen live and finished since the last call — the status
    /// line's notes (clause 118), for the GUI to word.
    pub(crate) fn keep_take_news(&mut self) -> Vec<PluginJob> {
        std::mem::take(&mut self.taste.keep.news)
    }

    /// The Copies modal's and the room's one-off note, taken.
    pub(crate) fn keep_take_note(&mut self) -> Option<KeepNote> {
        self.taste.keep.note.take()
    }

    // ── The Copies list (clause 117) ────────────────────────────────────────

    /// The Copies modal opened: its first ask, now.
    pub(crate) fn copies_open(&mut self) -> Vec<Effect> {
        self.taste_sync();
        self.taste.keep.note = None;
        self.keep_ask_jobs(crate::clock::Instant::now())
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
        let book = &self.taste.keep.book;
        let jobs: Vec<&PluginJob> = book.jobs.values().filter(|j| discover_copy(j)).collect();
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
        if !self.keep_listed() || self.taste.keep.book.cancelling.contains(&id) {
            return Vec::new();
        }
        if !self.taste.keep.job(id).is_some_and(PluginJob::live) {
            return Vec::new();
        }
        self.taste.keep.book.cancelling.insert(id);
        vec![Effect::Api(ApiCmd::JobCancel { generation: self.session_gen, id })]
    }

    /// Clear finished: the server drops the caller's finished jobs.
    pub(crate) fn copies_clear(&mut self) -> Vec<Effect> {
        if !self.keep_listed() {
            return Vec::new();
        }
        vec![Effect::Api(ApiCmd::JobsClear { generation: self.session_gen })]
    }

    /// A Copies row's action: Retry, Copy again and Start again start the
    /// same copy again (an album or artist copy takes only the gaps) where
    /// it ran — the list shows no place, so one moved since is the server's
    /// 409 to say, and Keep a copy's to show (clause 146); Play your copy
    /// plays it; Cancel cancels.
    pub(crate) fn copies_act(&mut self, id: i64) -> Vec<Effect> {
        let Some(job) = self.taste.keep.job(id).cloned() else { return Vec::new() };
        match copies_action(&job.outcome()) {
            Some(JobAction::Cancel) => self.keep_cancel_job(id),
            Some(JobAction::Play) => self.keep_play_copy(&job, true),
            Some(JobAction::Retry | JobAction::CopyAgain | JobAction::StartAgain) => {
                let place = self.keep_ran_with(&job);
                let (effects, ticket) = self.keep_restart(&job, place);
                if let Some(ticket) = ticket {
                    self.taste.keep.book.starts.insert(ticket);
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
        let Some(scope) = job.scope().filter(|s| s.discover()) else { return (Vec::new(), None) };
        let (effect, ticket) = self.keep_start_job(job.recommendation.clone(), scope, destination);
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
    /// (clause 145), a ticket.
    fn keep_start_job(
        &mut self,
        rec: serde_json::Value,
        scope: JobScope,
        destination: Option<Option<CollectionDestination>>,
    ) -> (Option<Effect>, Option<u64>) {
        if !self.keep_gate() || rec.is_null() || !scope.discover() {
            return (None, None);
        }
        let ticket = self.taste.keep.next_seq();
        let effect = Effect::Api(ApiCmd::JobStart {
            ticket,
            generation: self.session_gen,
            plugin: COPY_PLUGIN.to_string(),
            rec: Box::new(rec),
            scope,
            destination,
        });
        (Some(effect), Some(ticket))
    }

    /// Play (or queue) a copied song from the user's own library (clause
    /// 110's Play your copy and Add to queue), with the session's origin —
    /// `job` as the surface shows it: a song copy, or a wider copy's account
    /// of this song ([`covered_view`]).
    pub(crate) fn keep_play_copy(&mut self, job: &PluginJob, play: bool) -> Vec<Effect> {
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
        let origin = self.origin();
        if play { self.queue_track_next(&origin, track, true) } else { self.queue_track_end(&origin, track) }
    }

    // ── The Keep a copy flow (clauses 106–110) ──────────────────────────────

    /// Open Keep a copy on a row or a tile — `c`, the sheet's row, the
    /// tile's list (clauses 102–103). Nothing opens while the gate is
    /// closed or the target offers no choice. The sizes are asked now; the
    /// flow follows its scope's job at once when one of this server's
    /// stands (clause 107).
    pub(crate) fn keep_open(&mut self, target: KeepTarget) -> Vec<Effect> {
        self.taste_sync();
        let choices = self.keep_choices(target);
        let Some(subject) = self.keep_subject(target).filter(|_| !choices.is_empty()) else { return Vec::new() };
        // The jobs its rows already hold, merged newest per slot.
        let rows: Vec<(usize, usize)> = match target {
            KeepTarget::Row { group, row } => vec![(group, row)],
            KeepTarget::Tile(index) => self.taste.peer_albums().into_iter().nth(index).map(|a| a.rows).unwrap_or_default(),
        };
        let mut jobs = RowJobs::default();
        let mut looked_up = false;
        for at in &rows {
            let Some(row) = self.taste.keep.rows.get(at) else { continue };
            looked_up = true;
            for key in &row.keys {
                if !jobs.keys.contains(key) {
                    jobs.keys.push(key.clone());
                }
            }
            for job in row.held.values() {
                jobs.merge(job);
            }
        }
        let choice = if subject.album_focus() { KeepChoice::Album } else { KeepChoice::Song };
        let choice = if choices.contains(&choice) { choice } else { choices[0] };
        let peer = subject.peer;
        let phase = opening_phase(&jobs, &subject, choice.scope(false)).unwrap_or(KeepPhase::Choose);
        let mut effects = Vec::new();
        let facts_seq = self.taste.keep.next_seq();
        let origin = Origin { server: self.origin().server, peer: Some(peer) };
        let ask = KeepFactsAsk {
            album: subject.album.clone(),
            artist: subject.track_artist.clone(),
            album_artist: subject.album_artist.clone(),
            year: subject.year,
        };
        effects.push(Effect::Api(ApiCmd::KeepFacts {
            seq: facts_seq,
            generation: self.session_gen,
            reach: self.reach(&origin).ok(),
            ask: Box::new(ask),
        }));
        let lookup_seq = if looked_up {
            None
        } else {
            let seq = self.taste.keep.next_seq();
            effects.push(Effect::Api(ApiCmd::JobsLookup {
                seq,
                generation: self.session_gen,
                recs: vec![subject.rec.clone()],
            }));
            Some(seq)
        };
        self.taste.keep.flow = Some(KeepFlow {
            target,
            subject,
            choices,
            choice,
            only_missing: false,
            facts: KeepFacts::default(),
            facts_asking: true,
            facts_seq,
            lookup_seq,
            jobs,
            phase,
            sent: None,
        });
        // Where files land, read again with every open: the last place
        // known draws meanwhile (clause 105).
        effects.extend(self.keep_dest_read());
        effects
    }

    /// The open flow's cover, asked of its own server through the parent's
    /// art proxy (the strip's way) — `None` when it names none, the cache
    /// holds or awaits it, or the server cannot be reached.
    pub(crate) fn keep_cover(&mut self) -> Option<Effect> {
        let flow = self.taste.keep.flow.as_ref()?;
        let file = flow.subject.art.clone()?;
        if self.art.contains_key(&file) {
            return None;
        }
        let origin = Origin { server: self.origin().server, peer: Some(flow.subject.peer) };
        let reach = self.reach(&origin).ok()?;
        self.fetch_art_from(&file, Some(reach))
    }

    /// The Keep modal closed (Esc, `[X]`, Close, Cancel): the copy keeps
    /// running on the server (clause 109); the picker's folders go with it.
    pub(crate) fn keep_close(&mut self) {
        self.taste.keep.flow = None;
        self.keep_dest_forget();
    }

    /// Pick a radio (Space, Enter, a click). Choosing is not toggling:
    /// the album or the song clear "only the albums you don't have".
    pub(crate) fn keep_pick(&mut self, choice: KeepChoice) {
        let Some(flow) = self.taste.keep.flow.as_mut() else { return };
        if !flow.choices.contains(&choice) || !matches!(flow.phase, KeepPhase::Choose | KeepPhase::Refused(_)) {
            return;
        }
        if flow.choice != choice {
            flow.only_missing = false;
        }
        flow.choice = choice;
    }

    /// Tick or untick "only the albums you don't have" — ticking it picks
    /// Every album by (clause 107). Nothing while it is hidden.
    pub(crate) fn keep_set_missing(&mut self, on: bool) {
        let shown = self.keep_view().is_some_and(|v| v.missing.is_some());
        let Some(flow) = self.taste.keep.flow.as_mut() else { return };
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
        let (effect, ticket) = self.keep_start_job(view.subject.rec, view.scope, shown.clone());
        let Some(ticket) = ticket else { return Vec::new() };
        if let Some(flow) = self.taste.keep.flow.as_mut() {
            flow.phase = KeepPhase::Starting { ticket };
            flow.sent = shown;
        }
        effect.into_iter().collect()
    }

    /// `x` or Cancel copy: cancel the followed copy while it is live.
    pub(crate) fn keep_cancel(&mut self) -> Vec<Effect> {
        let Some(KeepPhase::Following(id)) = self.taste.keep.flow.as_ref().map(|f| f.phase.clone()) else {
            return Vec::new();
        };
        self.keep_cancel_job(id)
    }

    /// The finished copy's button (clause 110): Retry and Copy again start
    /// the same copy again (taking only the gaps) — a wider copy's account
    /// of the choice ([`covered_view`]) carries the choice's scope and the
    /// modal's row, so it starts the choice's own copy — at the account's
    /// place now, which the modal shows under LANDS IN where it is not the
    /// one the copy ran with (`KeepView::again`, clause 145); Start again
    /// brings the choice back, Play your copy plays it.
    pub(crate) fn keep_again(&mut self) -> Vec<Effect> {
        let Some(view) = self.keep_view() else { return Vec::new() };
        let KeepStage::Job { job, action, .. } = view.stage else { return Vec::new() };
        match action {
            Some(JobAction::StartAgain) => {
                if let Some(flow) = self.taste.keep.flow.as_mut() {
                    flow.phase = KeepPhase::Choose;
                }
                Vec::new()
            }
            Some(JobAction::Retry | JobAction::CopyAgain) => {
                let shown = Some(view.destination.clone());
                let (effects, ticket) = self.keep_restart(&job, shown.clone());
                if let (Some(ticket), Some(flow)) = (ticket, self.taste.keep.flow.as_mut()) {
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
        let flow = self.taste.keep.flow.as_ref()?;
        let s = &flow.subject;
        let facts = &flow.facts;
        let fact = |known: Option<usize>| match known {
            Some(n) => Fact::Known(n),
            None if flow.facts_asking => Fact::Asking,
            None => Fact::Unknown,
        };
        let ownership = match (&facts.peer_albums, &facts.local_albums, s.track_artist.as_deref()) {
            (Some(remote), Some(local), Some(artist)) => Some(artist_ownership(artist, remote, local)),
            _ => None,
        };
        let choices = flow
            .choices
            .iter()
            .map(|&choice| {
                let size = match choice {
                    KeepChoice::Song => ChoiceSize::Song { seconds: s.duration, format: s.format() },
                    KeepChoice::Album => ChoiceSize::Album { songs: fact(facts.album_songs.as_ref().map(Vec::len)) },
                    KeepChoice::Artist => ChoiceSize::Artist {
                        albums: fact(facts.peer_albums.as_ref().map(Vec::len)),
                        have: fact(facts.local_albums.as_ref().map(Vec::len)),
                    },
                };
                (choice, size)
            })
            .collect();
        // "Only the albums you don't have" (clause 107): with the artist
        // choice, where the plug-in runs it, the library holds some of the
        // artist's albums and the other server one it lacks by name — or
        // where such a copy still wants the user.
        let runs_missing = self.taste.keep.gate.plugins.as_ref().is_some_and(|p| p.copies(JobScope::ArtistMissing))
            && flow.choices.contains(&KeepChoice::Artist);
        let held_missing = flow.jobs.slot(JobScope::ArtistMissing).filter(|j| !j.other_peer(Some(s.peer))).is_some_and(|j| {
            let o = j.outcome();
            o.live() || o.troubled()
        });
        let local = facts.local_albums.as_ref().map(Vec::len).unwrap_or(0);
        let missing_count = ownership.map(|(_, missing)| missing).unwrap_or(0);
        let missing = (runs_missing && ((local > 0 && missing_count > 0) || held_missing))
            .then_some((flow.only_missing && flow.choice == KeepChoice::Artist, missing_count));
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
            _ => KeepVerb::Artist,
        };
        // Another server's live copy of the same album or artist covers
        // this one: say so, and hold the primary back until it ends.
        let other = flow
            .jobs
            .slot(scope)
            .filter(|j| j.other_peer(Some(s.peer)) && j.live())
            .map(|j| j.peer_name().unwrap_or_default());
        let followed = |id: i64| {
            self.taste.keep.book.jobs.get(&id).or_else(|| flow.jobs.held.values().find(|j| j.id == id)).cloned()
        };
        let stage = match &flow.phase {
            KeepPhase::Choose => KeepStage::Choose { verb: other.is_none().then_some(verb), other },
            KeepPhase::Starting { .. } => KeepStage::Starting,
            KeepPhase::Refused(failure) => KeepStage::Refused { failure: failure.clone(), verb: Some(verb) },
            KeepPhase::Following(id) => match followed(*id) {
                // A start answered with another server's live copy: the
                // choice, its words, no primary until it ends.
                Some(job) if job.other_peer(Some(s.peer)) => {
                    let other = job.live().then(|| job.peer_name().unwrap_or_default());
                    KeepStage::Choose { verb: other.is_none().then_some(verb), other }
                }
                Some(job) => {
                    let outcome = job.outcome();
                    let action = outcome.action();
                    KeepStage::Job { job: Box::new(job), outcome, action }
                }
                // Gone from everything known (cleared elsewhere): the
                // choice again.
                None => KeepStage::Choose { verb: other.is_none().then_some(verb), other },
            },
            // A wider copy's account of this choice; the choice again once
            // it no longer gives one (cleared elsewhere).
            KeepPhase::Covered(id) => match followed(*id).and_then(|job| covered_view(&job, s, scope)) {
                Some(view) => {
                    let outcome = view.outcome();
                    let action = outcome.action();
                    KeepStage::Job { job: Box::new(view), outcome, action }
                }
                None => KeepStage::Choose { verb: other.is_none().then_some(verb), other },
            },
        };
        // Nowhere to put a file any more (a re-read, a start's 409): the
        // choice gives way to one plain line (clause 147).
        let destination = self.keep_destination().cloned();
        let stage = match stage {
            KeepStage::Choose { .. } | KeepStage::Refused { .. } if destination.is_none() => KeepStage::Nowhere,
            stage => stage,
        };
        let mut view = KeepView {
            subject: s.clone(),
            choices,
            choice: flow.choice,
            scope,
            missing,
            one_missing,
            destination,
            album_songs: facts.album_songs.clone(),
            landing: None,
            again: None,
            stage,
        };
        view.landing = self.keep_landing(&view, &flow.sent);
        view.again = keep_again_landing(&view);
        Some(view)
    }

    /// The LANDS IN line for a view's stage (clause 105): the account's
    /// place while choosing; the place a start sent while it is out; a
    /// copy's pinned place while it runs (a job from before the pin: the
    /// account's), and once it ended the folder its files landed in, else
    /// the place its result or its pin names — never the account's place
    /// now. A pin to nowhere hides it.
    fn keep_landing(&self, view: &KeepView, sent: &Option<Option<CollectionDestination>>) -> Option<Landing> {
        let current = view.destination.as_ref();
        let pinned = |pin: &Option<Option<CollectionDestination>>, scope: JobScope| match pin {
            Some(Some(place)) => Some(view.landing_at(place, scope)),
            Some(None) => None,
            None => current.map(|place| view.landing_at(place, scope)),
        };
        match &view.stage {
            KeepStage::Choose { .. } | KeepStage::Refused { .. } => current.map(|place| view.landing_at(place, view.scope)),
            KeepStage::Starting => pinned(sent, view.scope),
            KeepStage::Nowhere => None,
            KeepStage::Job { job, .. } => {
                let scope = job.scope().unwrap_or(view.scope);
                if job.live() {
                    return pinned(&job.params.destination, scope);
                }
                let result = job.copy_result();
                if let Some(Landed { vpath: Some(library), dirs, .. }) = result.landed() {
                    return Some(Landing::landed(&library, &dirs));
                }
                let ran = result.destination.as_ref().or(match &job.params.destination {
                    Some(Some(place)) => Some(place),
                    _ => None,
                });
                ran.map(|place| view.landing_at(place, scope))
            }
        }
    }

    // ── Where files land (clauses 105, 142–148) ─────────────────────────────

    /// Read the destination again — every Keep a copy modal that opens, and
    /// after a start's 409: the last place known draws until it answers,
    /// and a failed read keeps it.
    pub(crate) fn keep_dest_read(&mut self) -> Vec<Effect> {
        if !self.keep_listed() {
            return Vec::new();
        }
        let seq = self.taste.keep.next_seq();
        self.taste.keep.dest.reading = Some(seq);
        vec![Effect::Api(ApiCmd::DestinationRead { seq, generation: self.session_gen })]
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
        let seq = self.taste.keep.next_seq();
        let dest = &mut self.taste.keep.dest;
        dest.saving = Some(seq);
        dest.saved = None;
        (vec![Effect::Api(ApiCmd::DestinationSave { seq, generation: self.session_gen, destination })], Some(seq))
    }

    /// The save out, by its seq: its picker draws "saving…", no buttons.
    pub fn keep_dest_saving(&self) -> Option<u64> {
        self.taste.keep.dest.saving
    }

    /// How the last save went, once, by its seq: its view already
    /// replaced the gate's, or the server's words.
    pub(crate) fn keep_dest_take_saved(&mut self) -> Option<(u64, Result<(), PluginFailure>)> {
        self.taste.keep.dest.saved.take()
    }

    /// Ask the folders of `path` (`/{library}/{folder}`) for the picker's
    /// completion (clause 143): once per folder while the picker is open,
    /// one ask at a time — a second waits for the first.
    pub(crate) fn keep_dest_list(&mut self, path: &str) -> Vec<Effect> {
        if !self.keep_listed() || self.taste.keep.dest.folders.contains_key(path) {
            return Vec::new();
        }
        if self.taste.keep.dest.listing.is_some() {
            self.taste.keep.dest.wanted = Some(path.to_string());
            return Vec::new();
        }
        let seq = self.taste.keep.next_seq();
        let dest = &mut self.taste.keep.dest;
        dest.listing = Some((seq, path.to_string()));
        dest.folders.insert(path.to_string(), Folders::Asking);
        vec![Effect::Api(ApiCmd::DestinationFolders { seq, generation: self.session_gen, path: path.to_string() })]
    }

    /// The folders of `path` as asked, `None` not asked.
    pub fn keep_dest_folders(&self, path: &str) -> Option<&Folders> {
        self.taste.keep.dest.folders.get(path)
    }

    /// The picker closed: its folders go (the save, if one is out, still
    /// folds in when it answers, and how it went is still said).
    pub(crate) fn keep_dest_forget(&mut self) {
        let dest = &mut self.taste.keep.dest;
        dest.folders.clear();
        dest.wanted = None;
        dest.listing = None;
    }

    /// The destination's view from an answer to the ask sent as `seq`,
    /// unless an answer to a later ask is already drawn.
    fn keep_dest_fold(&mut self, seq: u64, view: CollectionDestinationView) {
        let gate = &mut self.taste.keep.gate;
        if seq > gate.view_seq {
            gate.view = Some(view);
            gate.view_seq = seq;
        }
    }

    // ── Open elsewhere (clause 140) ─────────────────────────────────────────

    /// Open the links list for a row: asked now, never before.
    pub(crate) fn links_open(&mut self, target: KeepTarget) -> Vec<Effect> {
        self.taste_sync();
        if !self.links_gate() {
            return Vec::new();
        }
        let Some(subject) = self.keep_subject(target) else { return Vec::new() };
        self.links_ask(subject.rec)
    }

    /// Try again: the same row asked again, as it was first sent — a new
    /// answer landing under the list never retargets it.
    pub(crate) fn links_retry(&mut self) -> Vec<Effect> {
        let Some(rec) = self.taste.keep.links.as_ref().map(|l| l.rec.clone()) else { return Vec::new() };
        if !self.links_gate() {
            return Vec::new();
        }
        self.links_ask(rec)
    }

    fn links_ask(&mut self, rec: serde_json::Value) -> Vec<Effect> {
        let Some(plugin) = self.taste.keep.gate.plugins.as_ref().and_then(|p| p.links_plugin()).map(|p| p.name.clone())
        else {
            return Vec::new();
        };
        let seq = self.taste.keep.next_seq();
        self.taste.keep.links = Some(LinksState { rec: rec.clone(), seq, status: LinksStatus::Asking });
        vec![Effect::Api(ApiCmd::PluginLinks { seq, generation: self.session_gen, plugin, rec: Box::new(rec) })]
    }

    pub(crate) fn links_close(&mut self) {
        self.taste.keep.links = None;
    }

    // ── Replies ─────────────────────────────────────────────────────────────

    /// Drop every wait nothing will answer — an error cleared them all.
    pub(super) fn clear_keep_waits(&mut self) {
        let keep = &mut self.taste.keep;
        keep.gate.asking = false;
        keep.lookup = None;
        keep.book.asking = None;
        keep.book.cancelling.clear();
        keep.book.starts.clear();
        keep.dest.reading = None;
        keep.dest.saving = None;
        keep.dest.listing = None;
        keep.dest.wanted = None;
        keep.dest.folders.retain(|_, folders| *folders != Folders::Asking);
        if let Some(flow) = keep.flow.as_mut() {
            flow.facts_asking = false;
            flow.lookup_seq = None;
            if matches!(flow.phase, KeepPhase::Starting { .. }) {
                flow.phase = KeepPhase::Choose;
            }
        }
    }

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
            | Event::DestinationRead { generation, .. }
            | Event::DestinationSaved { generation, .. }
            | Event::DestinationFolders { generation, .. } => *generation,
            _ => return Vec::new(),
        };
        if generation != self.session_gen {
            return Vec::new();
        }
        self.taste_sync();
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
                    self.taste.keep.book.jobs.retain(|_, job| !job.state.finished());
                    let mut effects = self.copies_try_again();
                    effects.extend(self.keep_lookup_rows());
                    effects
                }
                Err(failure) => {
                    self.taste.keep.note = Some(KeepNote::ClearFailed(failure));
                    Vec::new()
                }
            },
            Event::PluginLinks { seq, answer, .. } => {
                if let Some(links) = self.taste.keep.links.as_mut().filter(|l| l.seq == seq) {
                    links.status = match answer {
                        Ok(list) => LinksStatus::Ready(list),
                        Err(failure) => LinksStatus::Failed(failure),
                    };
                }
                Vec::new()
            }
            Event::KeepFacts { seq, facts, .. } => {
                if let Some(flow) = self.taste.keep.flow.as_mut().filter(|f| f.facts_seq == seq) {
                    flow.facts = *facts;
                    flow.facts_asking = false;
                }
                Vec::new()
            }
            // A read the last one asked: its view replaces what is drawn —
            // unless a save sent after it answered first; a failed one keeps
            // what is drawn.
            Event::DestinationRead { seq, answer, .. } => {
                if self.taste.keep.dest.reading == Some(seq) {
                    self.taste.keep.dest.reading = None;
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
                let dest = &mut self.taste.keep.dest;
                if dest.saving == Some(seq) {
                    dest.saving = None;
                    dest.saved = Some((seq, went));
                }
                Vec::new()
            }
            Event::DestinationFolders { seq, path, answer, .. } => {
                let dest = &mut self.taste.keep.dest;
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
        let gate = &mut self.taste.keep.gate;
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
        if self.keep_listed() && !self.taste.keep.book.listed {
            effects.extend(self.keep_ask_jobs(crate::clock::Instant::now()));
        }
        effects
    }

    fn keep_lookup_landed(&mut self, seq: u64, answer: Result<Vec<JobLookup>, PluginFailure>) -> Vec<Effect> {
        // The flow's own lookup.
        if let Some(flow) = self.taste.keep.flow.as_mut().filter(|f| f.lookup_seq == Some(seq)) {
            flow.lookup_seq = None;
            if let Some(found) = answer.as_ref().ok().and_then(|list| list.first()) {
                let mut jobs = RowJobs::from_lookup(found);
                for job in self.taste.keep.book.jobs.values() {
                    jobs.merge(job);
                }
                if flow.phase == KeepPhase::Choose
                    && let Some(phase) = opening_phase(&jobs, &flow.subject, flow.choice.scope(flow.only_missing))
                {
                    flow.phase = phase;
                }
                flow.jobs = jobs;
                flow.resettle();
            }
            return self.keep_seen(answer.as_deref().unwrap_or(&[]));
        }
        let Some(RowsLookup { seq: asked, rows: lines, landed }) = self.taste.keep.lookup.clone() else {
            return Vec::new();
        };
        if asked != seq {
            return Vec::new();
        }
        self.taste.keep.lookup = None;
        // A newer peers answer replaced these rows: its own lookup follows.
        if landed != self.taste.keep.peers_landed {
            return Vec::new();
        }
        let Ok(results) = answer else { return Vec::new() };
        let mut rows = BTreeMap::new();
        for (at, found) in lines.into_iter().zip(results.iter()) {
            let mut jobs = RowJobs::from_lookup(found);
            for job in self.taste.keep.book.jobs.values() {
                jobs.merge(job);
            }
            rows.insert(at, jobs);
        }
        self.taste.keep.rows = rows;
        self.keep_seen(&results)
    }

    /// A lookup's live jobs are seen live: their ending is news.
    fn keep_seen(&mut self, results: &[JobLookup]) -> Vec<Effect> {
        for job in results.iter().flat_map(|r| &r.jobs) {
            if job.live() && discover_copy(job) {
                self.taste.keep.book.seen_live.insert(job.id);
            }
        }
        Vec::new()
    }

    fn keep_jobs_landed(&mut self, seq: u64, answer: Result<Box<PluginJobList>, PluginFailure>) -> Vec<Effect> {
        if self.taste.keep.book.asking != Some(seq) {
            return Vec::new();
        }
        self.taste.keep.book.asking = None;
        let list = match answer {
            Ok(list) => list,
            Err(failure) => {
                let book = &mut self.taste.keep.book;
                book.failures = book.failures.saturating_add(1);
                book.failed = Some(failure);
                return Vec::new();
            }
        };
        let keep = &mut self.taste.keep;
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
        let from_list = self.taste.keep.book.starts.remove(&ticket);
        let ours = self
            .taste
            .keep
            .flow
            .as_ref()
            .is_some_and(|f| f.phase == KeepPhase::Starting { ticket });
        match answer {
            Ok(started) => {
                let job = started.job;
                let ended = self.taste.keep.fold(&job);
                if ours && let Some(flow) = self.taste.keep.flow.as_mut() {
                    flow.jobs.merge(&job);
                    // A job the server keyed under none of the flow's keys
                    // (none looked up yet) is still the one it follows.
                    if let Some(scope) = job.scope() {
                        flow.jobs.held.entry((job.plugin.clone(), scope)).or_insert_with(|| job.clone());
                    }
                    flow.phase = KeepPhase::Following(job.id);
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
                    let gate = &mut self.taste.keep.gate;
                    if let (Some(now), Some(view)) = (moved, gate.view.as_mut())
                        && ticket > gate.view_seq
                    {
                        view.destination = now.clone();
                        gate.view_seq = ticket;
                    }
                    effects = self.keep_dest_read();
                }
                if ours && let Some(flow) = self.taste.keep.flow.as_mut() {
                    flow.phase = KeepPhase::Refused(failure);
                } else if from_list {
                    self.taste.keep.note = Some(KeepNote::StartFailed(failure));
                }
                effects
            }
        }
    }

    fn keep_cancelled(&mut self, id: i64, answer: Result<CancelAnswer, PluginFailure>) -> Vec<Effect> {
        self.taste.keep.book.cancelling.remove(&id);
        match answer {
            Ok(CancelAnswer::Cancelled(cancelled)) => {
                let ended = cancelled.job.as_ref().is_some_and(|job| self.taste.keep.fold(job));
                if ended { self.keep_lookup_rows() } else { Vec::new() }
            }
            // It had already finished: refresh, and say so once.
            Ok(CancelAnswer::AlreadyFinished) => {
                let title = self.taste.keep.job(id).map(|job| CopyTitle::of(job).name().to_string());
                self.taste.keep.note = Some(KeepNote::TooLate { title });
                let mut effects = self.copies_try_again();
                effects.extend(self.keep_lookup_rows());
                effects
            }
            Err(failure) => {
                self.taste.keep.note = Some(KeepNote::CancelFailed(failure));
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

    #[test]
    fn the_choices_follow_the_rows_fields_and_the_plugins_scopes() {
        let all = listing(&["song", "album", "artist", "artist-missing", "folder"]);
        let row = subject(Some("Late Train Verse"), Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        assert_eq!(keep_choices(&all, &row), vec![KeepChoice::Song, KeepChoice::Album, KeepChoice::Artist]);
        assert_eq!(row.format().as_deref(), Some("mp3"));
        // No artist: no album copy (every album of that name would come back) and no artist copy.
        assert_eq!(keep_choices(&all, &subject(Some("T"), None, Some("Borrowed Tapes"), None)), vec![KeepChoice::Song]);
        // A compilation row: album by its album artist; the artist choice is the track artist's.
        let comp = subject(Some("Kim Song"), Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        assert_eq!(keep_choices(&all, &comp), vec![KeepChoice::Song, KeepChoice::Album, KeepChoice::Artist]);
        assert_eq!(comp.album_credit(), Some("Various Artists"));
        // A tile: album focus, no This song; a compilation tile offers no artist copy.
        let tile = subject(None, Some("Boukmanflow"), Some("Borrowed Tapes"), None);
        assert_eq!(keep_choices(&all, &tile), vec![KeepChoice::Album, KeepChoice::Artist]);
        let comp_tile = subject(None, Some("Peer Kim"), Some("Mixtape Vol 1"), Some("Various Artists"));
        assert_eq!(keep_choices(&all, &comp_tile), vec![KeepChoice::Album]);
        assert!(comp_tile.compilation() && !tile.compilation());
        // The plug-in runs the song alone (an older server): Song only; a tile then offers nothing.
        let old = listing(&[]);
        assert_eq!(keep_choices(&old, &row), vec![KeepChoice::Song]);
        assert!(keep_choices(&old, &tile).is_empty());
        // Jobs refused: nothing.
        let mut refused = all.clone();
        refused.jobs.allowed = false;
        assert!(keep_choices(&refused, &row).is_empty());
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
        };
        let crumbs = |view: &KeepView, layout: &str, scope: JobScope| {
            let landing = view.landing_at(&place(layout), scope);
            (landing.crumbs(), landing.per_album)
        };
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(crumbs(&view, "{{ARTIST}}/{{YEAR}} - {{ALBUM}}", JobScope::Song), (names(&["copies", "from-peers", "Boukmanflow", "2019 - Borrowed Tapes"]), false));
        assert_eq!(view.file_name().as_deref(), Some("02 - Late Train Verse.MP3"));
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
        assert_eq!(KeepView { scope: JobScope::Album, ..view }.file_name(), None, "a song's file alone");
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
}
