//! Auto DJ's seed lists (auto-dj contract, clauses 23, 64–66): More like
//! and Less like, their pins, the Stay close freeze, Less like's clock.
//!
//! Pure: the App's glue is autodj.rs. The lists outlive the lane (a lane
//! reset wipes `DjLane` whole), and what `[player.dj]` keeps outlives the
//! run: the pins, and every row while Stay close holds them where a Seeds
//! sheet shows them (clause 66, decision 41).
//! Stay close freezes both lists without touching a stored pin, so the
//! user's own pins come back with Drift.

use std::time::Duration;

use crate::api::types::{Track, TrackMetadata};
use crate::clock::Instant;
use crate::config::SeedPrefs;

use super::{Origin, Queued};

/// Rows a list holds at most; also the server's cap on `similarTo` and on
/// `lessLike`. More like passes it only in Stay close, for its one song,
/// up to [`SEEDS_STORED_MAX`] (decision 42).
pub const SEEDS_MAX: usize = 8;
/// More like's rows at most while Stay close's one song carries it past
/// [`SEEDS_MAX`] — one song a lane that had nothing to stay close to —
/// and so the most a Stay close load brings back (the phone's
/// `maxStoredPerList`, decision 42).
pub const SEEDS_STORED_MAX: usize = 2 * SEEDS_MAX;
/// More like's unpinned rows at most (min(this, [`SEEDS_MAX`] − pinned)) —
/// the rolling anchor's five DJ picks of before (`dj::HISTORY_LEN`).
pub const MORE_LIKE_ROLLING: usize = crate::dj::HISTORY_LEN;
/// An unpinned Less like row's life, in Drift time (clause 23).
pub const LESS_LIKE_TTL: Duration = Duration::from_secs(30 * 60);

/// One of the two lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeedList {
    /// The positives: what picks should sound like.
    MoreLike,
    /// The negatives: what picks should not.
    LessLike,
}

impl SeedList {
    pub fn other(self) -> SeedList {
        match self {
            SeedList::MoreLike => SeedList::LessLike,
            SeedList::LessLike => SeedList::MoreLike,
        }
    }
}

/// How a row got there (the record's "origin"; [`Origin`] is the server
/// here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedFrom {
    /// A DJ pick that started playing, in Drift.
    Pick,
    /// The song a lane opened on, in Drift: a seeded start's first song, or
    /// the song current at a lane's first real pick (clause 15).
    Opener,
    /// Stay close's one song, for a lane with nothing to stay close to.
    Anchor,
    /// A quick skip, with Learn from skips on.
    Skip,
    /// The user's: Add song, or a move from the other list.
    Manual,
}

/// A seed's identity: its server (and peer) and its path in wire form —
/// the server identity without a trailing '/', the path without a leading
/// '/', the form `similarTo` and `lessLike` carry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeedKey {
    pub server: String,
    pub peer: Option<i64>,
    pub path: String,
}

impl SeedKey {
    pub fn new(origin: &Origin, filepath: &str) -> SeedKey {
        SeedKey {
            server: origin.server.trim_end_matches('/').to_string(),
            peer: origin.peer,
            path: filepath.trim_start_matches('/').to_string(),
        }
    }

    /// The server the row lives on, as a queue row names it.
    #[cfg_attr(not(test), allow(dead_code))] // the shared API: the GUI may name a row's server by it
    pub fn origin(&self) -> Origin {
        Origin { server: self.server.clone(), peer: self.peer }
    }

    /// Same server (`config::same_server`) and same peer.
    pub fn is_on(&self, origin: &Origin) -> bool {
        crate::config::same_server(&self.server, &origin.server) && self.peer == origin.peer
    }
}

/// One row of a list.
#[derive(Debug, Clone, PartialEq)]
pub struct Seed {
    pub key: SeedKey,
    /// What the sheet shows (title, artist; a stored row has only those,
    /// its album and its path).
    pub track: Track,
    /// The user's own pin — the stored flag. Stay close draws every row
    /// pinned without touching it ([`Seeds::shown_pinned`]).
    pub pinned: bool,
    pub from: SeedFrom,
    /// When the row came in. Less like's clock: restarted by an unpin and
    /// by a re-skip (with `ran`), moved forward past a Stay close pause.
    pub since: Instant,
    /// Drift time this row's clock had run before `since` — what an unpinned
    /// skip brings back from a restart in Stay close (clause 66, decision 36).
    /// Zero for a row that came in this run, and again whenever its clock
    /// restarts.
    pub ran: Duration,
}

/// A capture pick the lists refused, for the GUI to word once (clause 65).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedRefusal {
    /// The pick came from another server than the room's: its name.
    OtherServer(String),
    /// The list can take no row (all shown pinned and full).
    Full,
}

/// Both lists, newest first, and the Stay close freeze.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seeds {
    pub more_like: Vec<Seed>,
    pub less_like: Vec<Seed>,
    /// Since when Stay close has frozen both lists; `None` in Drift.
    frozen_at: Option<Instant>,
}

impl Seeds {
    /// Whether Stay close has frozen both lists.
    pub fn frozen(&self) -> bool {
        self.frozen_at.is_some()
    }

    pub fn list(&self, list: SeedList) -> &[Seed] {
        match list {
            SeedList::MoreLike => &self.more_like,
            SeedList::LessLike => &self.less_like,
        }
    }

    fn list_mut(&mut self, list: SeedList) -> &mut Vec<Seed> {
        match list {
            SeedList::MoreLike => &mut self.more_like,
            SeedList::LessLike => &mut self.less_like,
        }
    }

    /// How a row is drawn: pinned when the user pinned it, and every row
    /// while Stay close locks them.
    pub fn shown_pinned(&self, seed: &Seed) -> bool {
        self.frozen() || seed.pinned
    }

    pub fn holds(&self, list: SeedList, key: &SeedKey) -> bool {
        self.list(list).iter().any(|s| s.key == *key)
    }

    /// Whether a new row may come into `list`: it has room, or (in Drift)
    /// an unpinned row to make room with. A full list that is all shown
    /// pinned takes none.
    pub fn can_take(&self, list: SeedList) -> bool {
        let rows = self.list(list);
        rows.len() < SEEDS_MAX || (!self.frozen() && rows.iter().any(|s| !s.pinned))
    }

    /// More like's rows on `origin` a pick there can send (clause 23(b),
    /// decision 42), newest first, in wire form: those `refused` lets
    /// through, at most [`SEEDS_MAX`]. Past it the oldest row whose stored
    /// pin is false stays home first, the oldest of all only when every one
    /// is pinned — so a pin is never left out for Stay close's one song.
    /// The refused rows go before the cut: cut first, a list of refused pins
    /// would leave the one usable song out.
    pub fn usable_positives(&self, origin: &Origin, refused: impl Fn(&str) -> bool) -> Vec<String> {
        let mut rows: Vec<&Seed> =
            self.more_like.iter().filter(|s| s.key.is_on(origin) && !refused(&s.key.path)).collect();
        trim_to(&mut rows, SEEDS_MAX, |s| s.pinned);
        rows.into_iter().map(|s| s.key.path.clone()).collect()
    }

    /// More like's paths on `origin` with nothing refused — the positives a
    /// pick there may carry ([`Seeds::usable_positives`]).
    pub fn positives(&self, origin: &Origin) -> Vec<String> {
        self.usable_positives(origin, |_| false)
    }

    /// The same of Less like: the negatives.
    pub fn negatives(&self, origin: &Origin) -> Vec<String> {
        on(&self.less_like, origin)
    }

    /// A DJ pick started playing, in Drift (clause 23): it comes in newest
    /// and unpinned; one already there moves to newest and keeps its pin.
    /// A song in Less like is not pushed (the user's Less like wins), and
    /// nothing is while frozen. True when the row stands in More like.
    pub fn push_pick(&mut self, key: SeedKey, track: Track, from: SeedFrom, now: Instant) -> bool {
        if self.frozen() || self.holds(SeedList::LessLike, &key) {
            return false;
        }
        if let Some(at) = self.more_like.iter().position(|s| s.key == key) {
            let mut seed = self.more_like.remove(at);
            seed.track = track;
            self.more_like.insert(0, seed);
            return true;
        }
        self.more_like.insert(0, Seed { key: key.clone(), track, pinned: false, from, since: now, ran: Duration::ZERO });
        self.fit(SeedList::MoreLike, None);
        self.holds(SeedList::MoreLike, &key)
    }

    /// Whether Stay close's one song could come in ([`Seeds::admit_anchor`]):
    /// a song neither list holds, while More like is under
    /// [`SEEDS_STORED_MAX`] rows — what Preview reads of the next real pick
    /// (clause 23(c)). Eight is no bar: the App asks only for a lane with no
    /// usable row, so a list full of rows it cannot use takes the song as one
    /// more (the owner's "let it in as a 9th row", decision 42).
    pub fn admits_anchor(&self, key: &SeedKey) -> bool {
        !self.holds(SeedList::LessLike, key)
            && !self.holds(SeedList::MoreLike, key)
            && self.more_like.len() < SEEDS_STORED_MAX
    }

    /// Stay close's one song to start (clause 23): a lane with nothing to
    /// stay close to gets it, unpinned and drawn locked. It ignores the
    /// freeze — it is the freeze's own bootstrap — and never evicts: past
    /// eight rows it comes in as one more, up to [`SEEDS_STORED_MAX`]
    /// (decision 42); a row already there, a Less like song or sixteen rows
    /// refuse it.
    pub fn admit_anchor(&mut self, key: SeedKey, track: Track, now: Instant) -> bool {
        if !self.admits_anchor(&key) {
            return false;
        }
        let seed = Seed { key, track, pinned: false, from: SeedFrom::Anchor, since: now, ran: Duration::ZERO };
        self.more_like.insert(0, seed);
        true
    }

    /// A quick skip of a DJ pick, in Drift (clause 23): out of More like
    /// unless the user pinned it there (then nothing more happens), and —
    /// when `learn` — into Less like unpinned, or newest with its clock
    /// restarted when it is there already. A full Less like evicts its
    /// oldest unpinned row; eight pins take no skip. The App calls it too
    /// for the user's own song a lane's start blended in, with `learn`
    /// false: nothing is learned from a song the user put on (decision 39).
    pub fn quick_skip(&mut self, key: &SeedKey, track: &Track, learn: bool, now: Instant) -> bool {
        if self.frozen() {
            return false;
        }
        let mut changed = false;
        if let Some(at) = self.more_like.iter().position(|s| s.key == *key) {
            if self.more_like[at].pinned {
                return false;
            }
            self.more_like.remove(at);
            changed = true;
        }
        if !learn {
            return changed;
        }
        if let Some(at) = self.less_like.iter().position(|s| s.key == *key) {
            let mut seed = self.less_like.remove(at);
            seed.since = now;
            seed.ran = Duration::ZERO;
            self.less_like.insert(0, seed);
            return true;
        }
        if self.less_like.len() >= SEEDS_MAX {
            let Some(oldest) = self.less_like.iter().rposition(|s| !s.pinned) else { return changed };
            self.less_like.remove(oldest);
        }
        let seed = Seed {
            key: key.clone(),
            track: track.clone(),
            pinned: false,
            from: SeedFrom::Skip,
            since: now,
            ran: Duration::ZERO,
        };
        self.less_like.insert(0, seed);
        true
    }

    /// Less like's unpinned rows past their life in Drift time go (clause
    /// 23) — the Drift time a row ran before `since` (a restart in Stay
    /// close) counts. Nothing goes while frozen.
    pub fn prune(&mut self, now: Instant) -> bool {
        if self.frozen() {
            return false;
        }
        let before = self.less_like.len();
        self.less_like
            .retain(|s| s.pinned || s.ran.saturating_add(now.saturating_duration_since(s.since)) < LESS_LIKE_TTL);
        self.less_like.len() != before
    }

    /// Stay close: both lists freeze, from `now`.
    pub fn freeze(&mut self, now: Instant) -> bool {
        if self.frozen() {
            return false;
        }
        self.frozen_at = Some(now);
        true
    }

    /// Back to Drift: the stored pins show again, every unpinned Less like
    /// row's clock skips the pause, and More like trims to its caps, oldest
    /// unpinned first.
    pub fn thaw(&mut self, now: Instant) -> bool {
        let Some(frozen_at) = self.frozen_at.take() else { return false };
        for seed in self.less_like.iter_mut().filter(|s| !s.pinned) {
            // The Drift time a row lived through stays its age: the pause
            // is skipped (a row can only predate the freeze, but one that
            // somehow came in during it starts its clock at the thaw).
            let paused = now.saturating_duration_since(seed.since.max(frozen_at));
            seed.since = seed.since.checked_add(paused).unwrap_or(seed.since);
        }
        self.fit(SeedList::MoreLike, None);
        self.fit(SeedList::LessLike, None);
        true
    }

    /// A new lane (clause 10) or a new queue (clause 12): in Drift the DJ's
    /// own unpinned More like rows go — its picks, the opener, the song a
    /// lane opened on — and the pins and Less like's unpinned rows stay,
    /// their clocks running. While Stay close holds the lists, every row
    /// stays where a Seeds sheet shows them (`shown`: the owner's "keep
    /// everything shown", clause 66); with no sheet (the TUI) Stay close's
    /// More like keeps its pins alone, as before the second answers
    /// (decision 41).
    pub fn new_lane(&mut self, shown: bool) -> bool {
        if self.frozen() && shown {
            return false;
        }
        let before = self.more_like.len();
        self.more_like.retain(|s| s.pinned);
        self.more_like.len() != before
    }

    /// The ticked box (clause 66): every row drawn pinned goes from both
    /// lists — the user's pins in Drift, unpinned rows staying (Less like's
    /// with their clocks); every row in Stay close where a sheet draws every
    /// row pinned (`shown`); with no sheet the stored pins alone, frozen or
    /// not (decision 41). The freeze stays.
    pub fn without_pinned(&mut self, shown: bool) -> bool {
        let all = self.frozen() && shown;
        let before = self.more_like.len() + self.less_like.len();
        self.more_like.retain(|s| !all && !s.pinned);
        self.less_like.retain(|s| !all && !s.pinned);
        self.more_like.len() + self.less_like.len() != before
    }

    /// The rows `gone` names leave both lists — a removed server's,
    /// pinned or not (clause 66).
    pub fn drop_where(&mut self, gone: impl Fn(&SeedKey) -> bool) -> bool {
        let before = self.more_like.len() + self.less_like.len();
        self.more_like.retain(|s| !gone(&s.key));
        self.less_like.retain(|s| !gone(&s.key));
        self.more_like.len() + self.less_like.len() != before
    }

    /// The saved server `old` was re-addressed as `new`: its rows follow
    /// (contract clause 1 of multi-server: the URL is the identity).
    pub fn rename_server(&mut self, old: &str, new: &str) -> bool {
        let mut changed = false;
        for seed in self.more_like.iter_mut().chain(self.less_like.iter_mut()) {
            if let Some(renamed) = crate::config::renamed_identity(&seed.key.server, old, new) {
                seed.key.server = renamed.trim_end_matches('/').to_string();
                changed = true;
            }
        }
        if changed {
            self.dedupe();
        }
        changed
    }

    /// A song's tags arrived (clause 66, decision 44): its rows in either list
    /// take the title, artist and album they lack — a row the remote queued
    /// bare joined under its file name. True when a row changed; a pin, and
    /// any row Stay close keeps, then writes them to the file.
    pub fn fill_tags(&mut self, key: &SeedKey, tags: &TrackMetadata) -> bool {
        let mut changed = false;
        for seed in self.more_like.iter_mut().chain(self.less_like.iter_mut()).filter(|s| s.key == *key) {
            let have = &mut seed.track.metadata;
            for (slot, value) in [(&mut have.title, &tags.title), (&mut have.artist, &tags.artist), (&mut have.album, &tags.album)] {
                if slot.is_none() && value.is_some() {
                    *slot = value.clone();
                    changed = true;
                }
            }
        }
        changed
    }

    /// The user's own row (clause 64: Add song, through the library
    /// capture): pinned and newest. One the list holds already becomes
    /// pinned and newest; one the other list holds moves over, pin or not —
    /// the user's latest word. In either mode, but only into a list that
    /// can take a row ([`Seeds::can_take`]).
    pub fn add_manual(&mut self, list: SeedList, key: SeedKey, track: Track, now: Instant) -> bool {
        if let Some(at) = self.list(list).iter().position(|s| s.key == key) {
            let rows = self.list_mut(list);
            let mut seed = rows.remove(at);
            seed.pinned = true;
            rows.insert(0, seed);
            return true;
        }
        if !self.can_take(list) {
            return false;
        }
        self.list_mut(list.other()).retain(|s| s.key != key);
        let seed = Seed { key, track, pinned: true, from: SeedFrom::Manual, since: now, ran: Duration::ZERO };
        self.list_mut(list).insert(0, seed);
        self.fit(list, None);
        true
    }

    /// A row's pin, from the sheet (clause 64). Refused while Stay close
    /// locks the pins. Unpinning restarts a Less like row's clock, and
    /// More like's trim then spares the row just unpinned.
    pub fn set_pinned(&mut self, list: SeedList, key: &SeedKey, pinned: bool, now: Instant) -> bool {
        if self.frozen() {
            return false;
        }
        let Some(seed) = self.list_mut(list).iter_mut().find(|s| s.key == *key) else { return false };
        if seed.pinned == pinned {
            return false;
        }
        seed.pinned = pinned;
        if !pinned {
            match list {
                SeedList::LessLike => {
                    seed.since = now;
                    seed.ran = Duration::ZERO;
                }
                SeedList::MoreLike => self.fit(SeedList::MoreLike, Some(key)),
            }
        }
        true
    }

    /// A row deleted (clause 64), in either mode.
    pub fn remove(&mut self, list: SeedList, key: &SeedKey) -> bool {
        let rows = self.list_mut(list);
        let before = rows.len();
        rows.retain(|s| s.key != *key);
        rows.len() != before
    }

    /// A row to the other list, pinned and newest (the row menu's "More
    /// like this" / "Less like this"), in either mode — when that list
    /// can take a row.
    pub fn move_to_other(&mut self, from: SeedList, key: &SeedKey, now: Instant) -> bool {
        let to = from.other();
        if !self.can_take(to) {
            return false;
        }
        let Some(at) = self.list(from).iter().position(|s| s.key == *key) else { return false };
        let seed = self.list_mut(from).remove(at);
        let moved = Seed { pinned: true, from: SeedFrom::Manual, since: now, ran: Duration::ZERO, ..seed };
        self.list_mut(to).insert(0, moved);
        self.fit(to, None);
        true
    }

    /// What `[player.dj]` keeps (clause 66): in Drift the stored pins of
    /// each list; in Stay close every row, each with its pin, an unpinned
    /// skip with its Drift time (decision 36) — in list order, More like's,
    /// then Less like's (`shown`: a Seeds sheet's shell keeps every row Stay
    /// close shows; without one, the pins — decision 41).
    pub fn to_prefs(&self, shown: bool) -> (Vec<SeedPrefs>, Vec<SeedPrefs>) {
        let (mut more, mut less) = (Vec::new(), Vec::new());
        for (list, seed) in self.kept(shown) {
            let prefs = prefs_of(seed, self.drift_ms(list, seed));
            match list {
                SeedList::MoreLike => more.push(prefs),
                SeedList::LessLike => less.push(prefs),
            }
        }
        (more, less)
    }

    /// The lists a launch starts with (clause 66): in Drift the stored pins
    /// alone; in Stay close every stored row with its own pin where a sheet
    /// shows them, the pins alone where none does (decision 41) — frozen
    /// from `now` either way, an unpinned skip keeping the Drift time it had
    /// run (decision 36). Each row's clock starts at `now`. At most
    /// [`SEEDS_MAX`] a list — More like up to [`SEEDS_STORED_MAX`] where it
    /// loads every row frozen (Stay close's one song, decision 42) — past
    /// which the oldest row whose stored pin is false goes first (only a
    /// hand-edited file gets there). A row with no server or no path is let
    /// go, and so is a duplicate — a key keeps its first row, and a key in
    /// both lists keeps More like's.
    pub fn from_prefs(more: &[SeedPrefs], less: &[SeedPrefs], frozen: bool, shown: bool, now: Instant) -> Seeds {
        let all = frozen && shown;
        // Once each, then trimmed (decision 42): a duplicate takes no room,
        // and a key More like keeps is not Less like's.
        let mut kept = std::collections::HashSet::new();
        let mut rows = |list: SeedList, prefs: &[SeedPrefs], cap: usize| -> Vec<Seed> {
            let mut once = std::collections::HashSet::new();
            let mut rows: Vec<Seed> = prefs
                .iter()
                .filter(|prefs| all || prefs.pinned)
                .filter_map(|prefs| seed_of(list, prefs, now))
                .filter(|seed| !kept.contains(&seed.key) && once.insert(seed.key.clone()))
                .collect();
            trim_to(&mut rows, cap, |s| s.pinned);
            kept.extend(rows.iter().map(|seed| seed.key.clone()));
            rows
        };
        // Stay close's one song can carry More like past eight (decision
        // 42): a More like kept in Stay close loads up to sixteen rows, all
        // the live list can hold. Less like never passes eight in a run —
        // the phone's merge keeps it there too.
        let more_like = rows(SeedList::MoreLike, more, if all { SEEDS_STORED_MAX } else { SEEDS_MAX });
        let less_like = rows(SeedList::LessLike, less, SEEDS_MAX);
        Seeds { more_like, less_like, frozen_at: frozen.then_some(now) }
    }

    /// A fingerprint of what `[player.dj]` keeps of the lists
    /// ([`Seeds::to_prefs`], `shown` as there): the GUI saves soon when it
    /// moves (decision 38). A Drift pick, which the file does not keep,
    /// leaves it as it was.
    pub fn kept_hash(&self, shown: bool) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        for (list, seed) in self.kept(shown) {
            list.hash(&mut hasher);
            seed.key.hash(&mut hasher);
            seed.pinned.hash(&mut hasher);
            let tags = &seed.track.metadata;
            (tags.title.as_deref(), tags.artist.as_deref(), tags.album.as_deref()).hash(&mut hasher);
            self.drift_ms(list, seed).hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The rows `[player.dj]` keeps, More like's then Less like's, each list
    /// newest first: every row while frozen in the sheet's shell, the stored
    /// pins otherwise (clause 66, decision 41).
    fn kept(&self, shown: bool) -> impl Iterator<Item = (SeedList, &Seed)> {
        let all = self.frozen() && shown;
        let more = self.more_like.iter().map(|seed| (SeedList::MoreLike, seed));
        let less = self.less_like.iter().map(|seed| (SeedList::LessLike, seed));
        more.chain(less).filter(move |(_, seed)| all || seed.pinned)
    }

    /// An unpinned skip's Drift time at the freeze, in ms — on unpinned Less
    /// like rows of a frozen set only, at most [`LESS_LIKE_TTL`] (decision
    /// 36).
    fn drift_ms(&self, list: SeedList, seed: &Seed) -> Option<u64> {
        let frozen_at = self.frozen_at?;
        (list == SeedList::LessLike && !seed.pinned).then(|| {
            let ran = seed.ran.saturating_add(frozen_at.saturating_duration_since(seed.since));
            ran.min(LESS_LIKE_TTL).as_millis() as u64
        })
    }

    /// The caps (clause 23), never while frozen. More like: while its
    /// unpinned rows exceed min([`MORE_LIKE_ROLLING`], [`SEEDS_MAX`] −
    /// pinned), the oldest unpinned leaves — but never `spare`. Less like:
    /// while over [`SEEDS_MAX`], the oldest unpinned leaves.
    fn fit(&mut self, list: SeedList, spare: Option<&SeedKey>) {
        if self.frozen() {
            return;
        }
        match list {
            SeedList::MoreLike => loop {
                let pinned = self.more_like.iter().filter(|s| s.pinned).count();
                let unpinned = self.more_like.len() - pinned;
                if unpinned <= MORE_LIKE_ROLLING.min(SEEDS_MAX.saturating_sub(pinned)) {
                    break;
                }
                let Some(oldest) = self.more_like.iter().rposition(|s| !s.pinned && Some(&s.key) != spare) else {
                    break;
                };
                self.more_like.remove(oldest);
            },
            SeedList::LessLike => {
                while self.less_like.len() > SEEDS_MAX {
                    let Some(oldest) = self.less_like.iter().rposition(|s| !s.pinned) else { break };
                    self.less_like.remove(oldest);
                }
            }
        }
    }

    /// A key once per list and in one list only, More like's row winning —
    /// after a rename merged two servers' spellings.
    fn dedupe(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.more_like.retain(|s| seen.insert(s.key.clone()));
        self.less_like.retain(|s| seen.insert(s.key.clone()));
    }
}

/// A list's paths on `origin`, newest first, in wire form, at most
/// [`SEEDS_MAX`] — Less like's negatives (More like's go through
/// [`Seeds::usable_positives`]).
fn on(rows: &[Seed], origin: &Origin) -> Vec<String> {
    rows.iter().filter(|s| s.key.is_on(origin)).map(|s| s.key.path.clone()).take(SEEDS_MAX).collect()
}

/// `rows`, newest first, down to `cap` by the list-trim rule (decision
/// 42): the oldest row whose stored pin is false leaves first, the oldest
/// of all only when every one is pinned.
fn trim_to<T>(rows: &mut Vec<T>, cap: usize, pinned: impl Fn(&T) -> bool) {
    while rows.len() > cap {
        let at = rows.iter().rposition(|row| !pinned(row)).unwrap_or(rows.len() - 1);
        rows.remove(at);
    }
}

/// A row as `[player.dj]` keeps it: its key, its tags, its pin, and an
/// unpinned skip's Drift time when Stay close keeps it (decision 36).
fn prefs_of(seed: &Seed, drift_ms: Option<u64>) -> SeedPrefs {
    let tag = |v: &Option<String>| v.clone().filter(|s| !s.trim().is_empty());
    SeedPrefs {
        server: seed.key.server.clone(),
        peer: seed.key.peer,
        path: seed.key.path.clone(),
        title: tag(&seed.track.metadata.title),
        artist: tag(&seed.track.metadata.artist),
        album: tag(&seed.track.metadata.album),
        pinned: seed.pinned,
        drift_ms,
    }
}

/// A stored row as a seed with its own pin, its clock at `now` — an
/// unpinned skip with the Drift time it had run (decision 36, at most
/// [`LESS_LIKE_TTL`]); `None` for one with no server or path.
fn seed_of(list: SeedList, prefs: &SeedPrefs, now: Instant) -> Option<Seed> {
    let origin = Origin { server: prefs.server.trim().to_string(), peer: prefs.peer };
    let key = SeedKey::new(&origin, prefs.path.trim());
    if key.server.is_empty() || key.path.is_empty() {
        return None;
    }
    let metadata = TrackMetadata {
        title: prefs.title.clone(),
        artist: prefs.artist.clone(),
        album: prefs.album.clone(),
        ..Default::default()
    };
    let track = Track { filepath: key.path.clone(), metadata };
    let ran = if list == SeedList::LessLike && !prefs.pinned {
        Duration::from_millis(prefs.drift_ms.unwrap_or(0)).min(LESS_LIKE_TTL)
    } else {
        Duration::ZERO
    };
    Some(Seed { key, track, pinned: prefs.pinned, from: SeedFrom::Manual, since: now, ran })
}

/// The owner's "blend in new music" (clause 15, decision 35): whether the
/// song current as a lane's first real pick is composed joins More like —
/// in Drift, at that first pick, a row on the DJ's server that neither list
/// holds and that is not one of this lane's DJ picks (those join as they
/// start).
pub fn blends_at_lane_start(
    drift: bool,
    lane_start: bool,
    on_dj_server: bool,
    in_less_like: bool,
    in_more_like: bool,
    lane_pick: bool,
) -> bool {
    drift && lane_start && on_dj_server && !in_less_like && !in_more_like && !lane_pick
}

/// The owner's "add the chosen song" (clause 15, decision 37): a seeded
/// start in Stay close adds a song the user chose — Choose or Recently
/// played, never Random — to More like pinned, as Add song would, when More
/// like can take a row — where a Seeds sheet shows the pin (`sheet`: the
/// GUI); the TUI keeps the old rule, the song held to as Stay close's one
/// song by the first real pick (decision 41).
pub fn adds_chosen_song(sheet: bool, stay_close: bool, chosen: bool, can_take: bool) -> bool {
    sheet && stay_close && chosen && can_take
}

/// The owner's "take it out" (clause 23(d), decision 39): whether a quick
/// skip of the playing row, keyed `key`, is one of the song this lane's
/// start blended into More like (`blended`, clause 15) — the user's own
/// row (`dj_pick` false: a DJ pick the start blended in, an earlier lane's
/// or run's row or the song served again, keeps the DJ pick's rule, Learn
/// included), while More like holds it unpinned (one the user pinned since
/// stays put, and its skip says nothing).
pub fn quick_skips_blend(key: &SeedKey, blended: Option<&SeedKey>, dj_pick: bool, unpinned_in_more_like: bool) -> bool {
    !dj_pick && blended == Some(key) && unpinned_in_more_like
}

/// The wire paths a pick on `origin` passes over (clause 26, decision 39):
/// the blended songs the user quick-skipped out of More like in this lane,
/// on that server. The DJ never served them, so no cursor id keeps them
/// out, and out of More like they are no seeds.
pub fn pass_over_on(passed: &[SeedKey], origin: &Origin) -> Vec<String> {
    passed.iter().filter(|key| key.is_on(origin)).map(|key| key.path.clone()).collect()
}

/// Plan D6: whether a Next on the playing row is a quick skip (clause 23).
/// `dj_row` is "a DJ pick on the DJ's server, with the DJ armed" — or the
/// user's own song the lane's start blended in ([`quick_skips_blend`],
/// decision 39); `heard_ms` and `duration_ms` are the playing row's open
/// play session's.
/// Heard for a second or more and not a play by the server's default rule,
/// with the Next really moving to another row: not under repeat-one, not
/// a wrap to the top, not off the last row.
pub fn is_quick_skip(
    dj_row: bool,
    heard_ms: u64,
    duration_ms: Option<u64>,
    repeat_one: bool,
    current: usize,
    next: Option<usize>,
    shuffle: bool,
) -> bool {
    dj_row
        && heard_ms >= super::stats::MIN_POST_MS
        && !super::stats::counts_as_play(heard_ms, duration_ms)
        && !repeat_one
        && next.is_some_and(|n| n != current && (shuffle || n > current))
}

/// Plan D11: the rows a re-roll replaces, nearest first — the DJ rows
/// strictly after `current`; none under shuffle, and none with no current
/// row (everything appends then).
pub fn reroll_slots(items: &[Queued], current: Option<usize>, shuffle: bool) -> Vec<usize> {
    let Some(current) = current.filter(|_| !shuffle) else { return Vec::new() };
    items.iter().enumerate().skip(current + 1).filter(|(_, row)| row.dj.is_some()).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::DjMark;

    const HOST: &str = "http://host:3000";
    /// The GUI's shell, which shows the Seeds sheet, and the TUI's, which
    /// shows none (decision 41).
    const SHEET: bool = true;
    const NO_SHEET: bool = false;

    fn origin() -> Origin {
        Origin { server: HOST.into(), peer: None }
    }

    fn key(path: &str) -> SeedKey {
        SeedKey::new(&origin(), path)
    }

    /// Another server, as a move of the DJ leaves its rows behind.
    fn far() -> Origin {
        Origin { server: "http://far:3000".into(), peer: None }
    }

    fn far_key(path: &str) -> SeedKey {
        SeedKey::new(&far(), path)
    }

    /// A stored row on the test server.
    fn stored(path: &str, pinned: bool) -> SeedPrefs {
        SeedPrefs { server: HOST.into(), path: path.into(), pinned, ..Default::default() }
    }

    /// Stay close's one song pushed onto the front by hand, as an
    /// admission past eight leaves it — the request cut's tests alone.
    fn stand_in(seeds: &mut Seeds, path: &str, now: Instant) {
        let seed = Seed { key: key(path), track: track(path), pinned: false, from: SeedFrom::Anchor, since: now, ran: Duration::ZERO };
        seeds.more_like.insert(0, seed);
    }

    fn track(path: &str) -> Track {
        Track {
            filepath: path.into(),
            metadata: TrackMetadata { title: Some(path.to_uppercase()), ..Default::default() },
        }
    }

    fn mins(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    fn paths(rows: &[Seed]) -> Vec<&str> {
        rows.iter().map(|s| s.key.path.as_str()).collect()
    }

    /// `n` picks, oldest first: the last pushed is newest.
    fn picked(seeds: &mut Seeds, names: &[&str], now: Instant) {
        for name in names {
            seeds.push_pick(key(name), track(name), SeedFrom::Pick, now);
        }
    }

    fn pin(seeds: &mut Seeds, list: SeedList, name: &str, now: Instant) {
        assert!(seeds.set_pinned(list, &key(name), true, now), "{name} pinned");
    }

    #[test]
    fn a_key_is_its_server_and_its_path_in_wire_form() {
        assert_eq!(key("/a.mp3"), key("a.mp3"), "a leading slash is no other song");
        let slash = SeedKey::new(&Origin { server: format!("{HOST}/"), peer: None }, "a.mp3");
        assert_eq!(slash, key("a.mp3"), "a trailing slash is no other server");
        assert!(slash.is_on(&origin()) && slash.is_on(&Origin { server: format!("{HOST}/"), peer: None }));
        let peer = SeedKey::new(&Origin { server: HOST.into(), peer: Some(4) }, "a.mp3");
        assert_ne!(peer, key("a.mp3"), "a peer's song is the peer's");
        assert!(!peer.is_on(&origin()) && peer.is_on(&Origin { server: HOST.into(), peer: Some(4) }));
        assert_eq!(peer.origin(), Origin { server: HOST.into(), peer: Some(4) });
        assert_eq!(SeedList::MoreLike.other(), SeedList::LessLike);
        assert_eq!(SeedList::LessLike.other(), SeedList::MoreLike);
    }

    #[test]
    fn a_pick_comes_in_newest_and_the_oldest_unpinned_leaves_past_five() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        picked(&mut seeds, &["a", "b", "c", "d", "e"], now);
        assert_eq!(paths(&seeds.more_like), ["e", "d", "c", "b", "a"], "newest first");
        assert!(seeds.more_like.iter().all(|s| !s.pinned && s.from == SeedFrom::Pick));
        assert!(seeds.push_pick(key("f"), track("f"), SeedFrom::Pick, now));
        assert_eq!(paths(&seeds.more_like), ["f", "e", "d", "c", "b"], "five unpinned at most: a left");

        // A re-pick moves to newest and keeps its pin.
        pin(&mut seeds, SeedList::MoreLike, "c", now);
        assert!(seeds.push_pick(key("c"), track("c"), SeedFrom::Pick, now));
        assert_eq!(paths(&seeds.more_like), ["c", "f", "e", "d", "b"]);
        assert!(seeds.more_like[0].pinned, "the pin stays");
    }

    #[test]
    fn the_unpinned_cap_is_five_or_what_the_pins_leave_of_eight() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        for name in ["p1", "p2", "p3", "p4"] {
            assert!(seeds.add_manual(SeedList::MoreLike, key(name), track(name), now));
        }
        picked(&mut seeds, &["a", "b", "c", "d", "e"], now);
        // Four pins leave room for four unpinned of eight.
        assert_eq!(seeds.more_like.len(), 8);
        assert_eq!(paths(&seeds.more_like)[..4], ["e", "d", "c", "b"], "a, the oldest unpinned, left");
        for name in ["p5", "p6", "p7", "p8"] {
            assert!(seeds.add_manual(SeedList::MoreLike, key(name), track(name), now));
        }
        assert_eq!(seeds.more_like.len(), 8, "every pin took an unpinned row's place");
        assert!(seeds.more_like.iter().all(|s| s.pinned));
        assert!(!seeds.push_pick(key("z"), track("z"), SeedFrom::Pick, now), "with eight pins a pick does not stay");
        assert!(!seeds.holds(SeedList::MoreLike, &key("z")));
        assert!(!seeds.can_take(SeedList::MoreLike), "eight pins take nothing more");
        assert!(!seeds.add_manual(SeedList::MoreLike, key("y"), track("y"), now));
    }

    #[test]
    fn a_less_like_song_is_never_pushed_and_nothing_is_while_frozen() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        assert!(seeds.quick_skip(&key("a"), &track("a"), true, now));
        assert!(!seeds.push_pick(key("a"), track("a"), SeedFrom::Pick, now), "the user's Less like wins");
        assert!(seeds.more_like.is_empty());
        seeds.freeze(now);
        assert!(!seeds.push_pick(key("b"), track("b"), SeedFrom::Pick, now));
        assert!(seeds.more_like.is_empty());
    }

    #[test]
    fn stay_closes_one_song_ignores_the_freeze_and_comes_in_past_eight_but_never_evicts() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        seeds.freeze(now);
        // What Preview reads of it is the admission's own rule.
        assert!(seeds.admits_anchor(&key("a")));
        assert!(seeds.admit_anchor(key("a"), track("a"), now), "the freeze's own bootstrap");
        assert_eq!(seeds.more_like[0].from, SeedFrom::Anchor);
        assert!(!seeds.more_like[0].pinned && seeds.shown_pinned(&seeds.more_like[0]), "drawn locked");
        assert!(!seeds.admits_anchor(&key("a")));
        assert!(!seeds.admit_anchor(key("a"), track("a"), now), "a row already there");

        let mut skipped = Seeds::default();
        skipped.quick_skip(&key("b"), &track("b"), true, now);
        assert!(!skipped.admits_anchor(&key("b")));
        assert!(!skipped.admit_anchor(key("b"), track("b"), now), "a Less like song");

        // Eight rows nobody here can use — another server's, as a move of
        // the DJ leaves them: the song comes in as a ninth row (the owner's
        // "let it in as a 9th row", decision 42), and nothing is evicted.
        let mut full = Seeds::default();
        for i in 1..=SEEDS_MAX {
            let name = format!("p{i}");
            assert!(full.add_manual(SeedList::MoreLike, far_key(&name), track(&name), now));
        }
        full.freeze(now);
        let eight = full.more_like.clone();
        assert!(full.admits_anchor(&key("c")));
        assert!(full.admit_anchor(key("c"), track("c"), now), "a ninth row");
        assert_eq!(full.more_like.len(), SEEDS_MAX + 1);
        assert_eq!(full.more_like[0].key, key("c"));
        assert_eq!(full.more_like[1..], eight[..], "nothing removed");
        assert_eq!(full.positives(&origin()), ["c"]);
        // Full all the same: the user's own rows find no room.
        assert!(!full.can_take(SeedList::MoreLike));
        assert!(!full.add_manual(SeedList::MoreLike, key("d"), track("d"), now));

        // The file keeps it, and nothing at load trims it.
        let (more, less) = full.to_prefs(SHEET);
        assert_eq!(more.len(), SEEDS_MAX + 1);
        assert_eq!(Seeds::from_prefs(&more, &less, true, SHEET, now).more_like.len(), SEEDS_MAX + 1, "nothing at load trims it");
        let pins: Vec<String> = (1..=SEEDS_MAX).rev().map(|i| format!("p{i}")).collect();
        let tui = Seeds::from_prefs(&more, &less, true, NO_SHEET, now);
        assert_eq!(paths(&tui.more_like), pins, "the TUI loads the pins alone");
        let drift = Seeds::from_prefs(&more, &less, false, SHEET, now);
        assert_eq!(paths(&drift.more_like), pins, "so does Drift");

        // Back in Drift the caps apply: eight pins leave no unpinned row.
        full.thaw(now);
        assert_eq!(full.more_like.len(), SEEDS_MAX);
        assert!(!full.holds(SeedList::MoreLike, &key("c")));
    }

    #[test]
    fn stay_closes_one_song_stops_at_sixteen_rows() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        for i in 1..=SEEDS_MAX {
            let name = format!("p{i}");
            seeds.add_manual(SeedList::MoreLike, far_key(&name), track(&name), now);
        }
        seeds.freeze(now);
        // A server move a lane, eight of them: one song each.
        for i in 1..=SEEDS_MAX {
            let name = format!("a{i}");
            assert!(seeds.admit_anchor(far_key(&name), track(&name), now), "{name}");
        }
        assert_eq!(seeds.more_like.len(), SEEDS_STORED_MAX);
        assert!(!seeds.admits_anchor(&key("c")));
        assert!(!seeds.admit_anchor(key("c"), track("c"), now), "sixteen rows: all a restart brings back");
        assert_eq!(seeds.more_like.len(), SEEDS_STORED_MAX);
        assert!(!seeds.can_take(SeedList::MoreLike));
        assert!(seeds.remove(SeedList::MoreLike, &far_key("a1")));
        assert!(seeds.admits_anchor(&key("c")), "a row removed: room again");
    }

    #[test]
    fn a_stay_close_load_keeps_sixteen_more_like_rows_and_lets_unpinned_ones_go_first() {
        let l = Instant::now();
        // Seventeen More like rows, newest first: pins at the even places,
        // the oldest (m16) among them; unpinned rows at the odd ones.
        let mut more: Vec<SeedPrefs> = (0..=SEEDS_STORED_MAX).map(|i| stored(&format!("m{i}"), i % 2 == 0)).collect();
        // Nine Less like rows, the oldest a pin.
        let mut less: Vec<SeedPrefs> = (0..=SEEDS_MAX).map(|i| stored(&format!("l{i}"), i == SEEDS_MAX)).collect();
        let loaded = Seeds::from_prefs(&more, &less, true, SHEET, l);
        let want: Vec<String> = (0..=SEEDS_STORED_MAX).filter(|i| *i != 15).map(|i| format!("m{i}")).collect();
        assert_eq!(paths(&loaded.more_like), want, "a hand-edited file past the bound loses an unpinned row before any pin");
        let want_less: Vec<String> = (0..=SEEDS_MAX).filter(|i| *i != 7).map(|i| format!("l{i}")).collect();
        assert_eq!(paths(&loaded.less_like), want_less, "Less like keeps eight, its oldest unpinned gone");

        // In Drift, or where no sheet shows them: the pins alone, eight at
        // most — every one pinned, so the oldest goes.
        let pins: Vec<String> = (0..SEEDS_STORED_MAX).step_by(2).map(|i| format!("m{i}")).collect();
        for (frozen, shown, what) in [(false, SHEET, "Drift"), (true, NO_SHEET, "the TUI")] {
            let back = Seeds::from_prefs(&more, &less, frozen, shown, l);
            assert_eq!(paths(&back.more_like), pins, "{what}");
            assert_eq!(paths(&back.less_like), [format!("l{SEEDS_MAX}")], "{what}");
        }

        // A duplicate takes no room, and a key in both lists keeps More
        // like's row.
        more.insert(1, stored("/m0", true));
        less.insert(0, stored("m2", true));
        let again = Seeds::from_prefs(&more, &less, true, SHEET, l);
        assert_eq!(again, loaded);
    }

    #[test]
    fn a_pick_past_eight_rows_leaves_the_oldest_unpinned_out_and_the_refused_before_it() {
        let now = Instant::now();
        let pins = |seeds: &mut Seeds, n: usize| {
            for i in 1..=n {
                let name = format!("p{i}");
                seeds.add_manual(SeedList::MoreLike, key(&name), track(&name), now);
            }
        };
        let newest_first = |n: usize| (1..=n).rev().map(|i| format!("p{i}")).collect::<Vec<String>>();
        // (i) Eight pins and the stand-in: no pin is left out for it.
        let mut seeds = Seeds::default();
        pins(&mut seeds, SEEDS_MAX);
        seeds.freeze(now);
        stand_in(&mut seeds, "two", now);
        assert_eq!(seeds.positives(&origin()), newest_first(SEEDS_MAX), "no pin is left out for the stand-in");
        // (iii) The same set, its pins refused: the refusals go before the cut.
        assert_eq!(seeds.usable_positives(&origin(), |p| p.starts_with('p')), ["two"], "the refusals out before the cut");

        // (ii) A frozen pick, the oldest row, seven pins and the stand-in:
        // the oldest unpinned row stays home.
        let mut seeds = Seeds::default();
        seeds.push_pick(key("k"), track("k"), SeedFrom::Pick, now);
        pins(&mut seeds, SEEDS_MAX - 1);
        seeds.freeze(now);
        stand_in(&mut seeds, "two", now);
        let mut want = vec!["two".to_string()];
        want.extend(newest_first(SEEDS_MAX - 1));
        assert_eq!(seeds.positives(&origin()), want, "the oldest unpinned row stays home: here a frozen pick");

        // (iv) Eight pins of another server's: the pick here sends the
        // stand-in, a pick there the eight.
        let mut seeds = Seeds::default();
        for i in 1..=SEEDS_MAX {
            let name = format!("p{i}");
            seeds.add_manual(SeedList::MoreLike, far_key(&name), track(&name), now);
        }
        seeds.freeze(now);
        stand_in(&mut seeds, "two", now);
        assert_eq!(seeds.positives(&origin()), ["two"]);
        assert_eq!(seeds.positives(&far()), newest_first(SEEDS_MAX));
    }

    #[test]
    fn a_quick_skip_leaves_more_like_and_with_learn_joins_less_like() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        picked(&mut seeds, &["a", "b"], now);
        // Learn off: More like only.
        assert!(seeds.quick_skip(&key("b"), &track("b"), false, now));
        assert_eq!(paths(&seeds.more_like), ["a"]);
        assert!(seeds.less_like.is_empty());
        assert!(!seeds.quick_skip(&key("x"), &track("x"), false, now), "nothing to change");

        // A More like pin stays, and nothing joins Less like.
        pin(&mut seeds, SeedList::MoreLike, "a", now);
        assert!(!seeds.quick_skip(&key("a"), &track("a"), true, now));
        assert_eq!(paths(&seeds.more_like), ["a"]);
        assert!(seeds.less_like.is_empty());

        // Learn on: unpinned, newest, a skip.
        assert!(seeds.quick_skip(&key("c"), &track("c"), true, now));
        assert!(seeds.quick_skip(&key("d"), &track("d"), true, now));
        assert_eq!(paths(&seeds.less_like), ["d", "c"]);
        assert!(seeds.less_like.iter().all(|s| !s.pinned && s.from == SeedFrom::Skip));

        // A re-skip moves to newest with its clock restarted, keeping a pin.
        let later = now + mins(10);
        pin(&mut seeds, SeedList::LessLike, "c", later);
        assert!(seeds.quick_skip(&key("c"), &track("c"), true, later));
        assert_eq!(paths(&seeds.less_like), ["c", "d"]);
        assert_eq!(seeds.less_like[0].since, later);
        assert!(seeds.less_like[0].pinned);

        // Frozen: nothing at all.
        seeds.freeze(later);
        let before = seeds.clone();
        assert!(!seeds.quick_skip(&key("e"), &track("e"), true, later));
        assert_eq!(seeds, before);
    }

    #[test]
    fn a_full_less_like_evicts_its_oldest_unpinned_and_eight_pins_take_no_skip() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        for i in 0..SEEDS_MAX {
            seeds.quick_skip(&key(&format!("s{i}")), &track("s"), true, now);
        }
        assert_eq!(seeds.less_like.len(), SEEDS_MAX);
        assert!(seeds.quick_skip(&key("new"), &track("new"), true, now));
        assert_eq!(seeds.less_like.len(), SEEDS_MAX);
        assert_eq!(seeds.less_like[0].key, key("new"));
        assert!(!seeds.holds(SeedList::LessLike, &key("s0")), "the oldest unpinned left");

        let mut pinned = Seeds::default();
        for i in 0..SEEDS_MAX {
            let name = format!("p{i}");
            pinned.add_manual(SeedList::LessLike, key(&name), track(&name), now);
        }
        assert!(!pinned.quick_skip(&key("new"), &track("new"), true, now), "eight pins: the skip is not added");
        assert_eq!(pinned.less_like.len(), SEEDS_MAX);
        assert!(!pinned.holds(SeedList::LessLike, &key("new")));
    }

    #[test]
    fn an_unpinned_less_like_row_lives_thirty_minutes_and_a_pin_forever() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        seeds.quick_skip(&key("a"), &track("a"), true, now);
        seeds.quick_skip(&key("p"), &track("p"), true, now);
        pin(&mut seeds, SeedList::LessLike, "p", now);
        assert!(!seeds.prune(now + mins(30) - Duration::from_secs(1)), "29:59 kept");
        assert!(seeds.holds(SeedList::LessLike, &key("a")));
        assert!(seeds.prune(now + mins(30)), "30:00 gone");
        assert_eq!(paths(&seeds.less_like), ["p"], "a pin never expires");
        assert!(!seeds.prune(now + mins(600)));

        let mut frozen = Seeds::default();
        frozen.quick_skip(&key("a"), &track("a"), true, now);
        frozen.freeze(now);
        assert!(!frozen.prune(now + mins(60)), "frozen: never");
        assert!(frozen.holds(SeedList::LessLike, &key("a")));
    }

    #[test]
    fn stay_close_pauses_less_likes_clock_and_drift_takes_it_up() {
        let t0 = Instant::now();
        let mut seeds = Seeds::default();
        seeds.quick_skip(&key("a"), &track("a"), true, t0);
        assert!(seeds.freeze(t0 + mins(5)));
        assert!(!seeds.freeze(t0 + mins(6)), "idempotent");
        assert!(seeds.thaw(t0 + mins(65)));
        assert!(!seeds.thaw(t0 + mins(66)), "idempotent");
        // Five minutes before the pause, twenty-five after it: gone at 1:30.
        assert!(!seeds.prune(t0 + mins(90) - Duration::from_secs(1)), "not at 1:29:59");
        assert!(seeds.prune(t0 + mins(90)), "gone at 1:30");
    }

    #[test]
    fn stay_close_locks_the_pins_without_touching_them_and_drift_trims_more_like() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        seeds.add_manual(SeedList::MoreLike, key("p"), track("p"), now);
        picked(&mut seeds, &["a", "b", "c", "d", "e"], now);
        seeds.freeze(now);
        assert!(seeds.more_like.iter().all(|s| seeds.shown_pinned(s)), "every row drawn pinned");
        assert_eq!(seeds.more_like.iter().filter(|s| s.pinned).count(), 1, "the stored pins untouched");
        assert!(!seeds.set_pinned(SeedList::MoreLike, &key("b"), true, now), "locked");
        assert!(!seeds.set_pinned(SeedList::MoreLike, &key("p"), false, now), "locked");
        // While frozen the user may still add, and nothing is evicted for it…
        assert!(seeds.add_manual(SeedList::MoreLike, key("m"), track("m"), now));
        assert!(seeds.admit_anchor(key("x"), track("x"), now));
        assert_eq!(seeds.more_like.len(), 8, "six unpinned beside two pins, frozen");
        assert!(!seeds.can_take(SeedList::MoreLike), "eight drawn pinned take nothing");
        // …and back in Drift the unpinned rows trim to five, the oldest
        // unpinned leaving first.
        seeds.thaw(now);
        let unpinned: Vec<&str> =
            seeds.more_like.iter().filter(|s| !s.pinned).map(|s| s.key.path.as_str()).collect();
        assert_eq!(unpinned, ["x", "e", "d", "c", "b"], "a left");
        assert_eq!(seeds.more_like.iter().filter(|s| s.pinned).count(), 2, "the pins came back as they were");
        assert!(seeds.can_take(SeedList::MoreLike), "room again");
    }

    #[test]
    fn a_new_lane_lets_the_djs_own_rows_go_and_keeps_the_rest() {
        // Drift: the DJ's own unpinned rows go — its picks, the song a lane
        // opened on — and the pin and Less like stay, the skip's clock
        // running.
        let now = Instant::now();
        let mut seeds = Seeds::default();
        picked(&mut seeds, &["a", "b"], now);
        seeds.push_pick(key("o"), track("o"), SeedFrom::Opener, now);
        pin(&mut seeds, SeedList::MoreLike, "a", now);
        seeds.quick_skip(&key("s"), &track("s"), true, now);
        seeds.add_manual(SeedList::LessLike, key("l"), track("l"), now);
        assert!(seeds.new_lane(SHEET));
        assert_eq!(paths(&seeds.more_like), ["a"], "the pin stays");
        assert_eq!(paths(&seeds.less_like), ["l", "s"], "Less like untouched");
        assert_eq!(seeds.less_like[1].since, now, "its clock running");
        assert!(!seeds.new_lane(SHEET));

        // Stay close keeps everything it shows (clause 66): a pin, a frozen
        // pick, its one song and an unpinned skip, each as it stood.
        let mut frozen = Seeds::default();
        frozen.add_manual(SeedList::MoreLike, key("p"), track("p"), now);
        picked(&mut frozen, &["b"], now);
        frozen.quick_skip(&key("s"), &track("s"), true, now);
        frozen.freeze(now + mins(1));
        assert!(frozen.admit_anchor(key("x"), track("x"), now + mins(2)));
        let before = frozen.clone();
        assert!(!frozen.new_lane(SHEET), "nothing goes while Stay close holds the lists");
        assert_eq!(frozen, before, "every row with its flag, its clock and its Drift time");
        assert!(frozen.frozen());
    }

    #[test]
    fn the_ticked_clear_lets_the_pins_go_and_keeps_the_skips_clocks() {
        let now = Instant::now();
        let later = now + mins(3);
        // A pick, a pinned pick, an unpinned skip, a Less like pin.
        let rows = || {
            let mut seeds = Seeds::default();
            picked(&mut seeds, &["a", "b"], now);
            pin(&mut seeds, SeedList::MoreLike, "a", now);
            seeds.quick_skip(&key("s"), &track("s"), true, now);
            seeds.add_manual(SeedList::LessLike, key("l"), track("l"), later);
            seeds
        };
        // Drift: the stored pins go; the unpinned rows stay, the skip with
        // its clock.
        let mut seeds = rows();
        assert!(seeds.without_pinned(SHEET));
        assert_eq!(paths(&seeds.more_like), ["b"]);
        assert_eq!(paths(&seeds.less_like), ["s"]);
        assert_eq!(seeds.less_like[0].since, now, "the clock untouched");
        assert!(!seeds.without_pinned(SHEET), "no pin left");

        // Stay close draws every row pinned, so every row goes; the freeze
        // stays, and Drift takes up from nothing.
        let mut frozen = rows();
        frozen.freeze(later);
        assert!(frozen.without_pinned(SHEET));
        assert!(frozen.more_like.is_empty() && frozen.less_like.is_empty());
        assert!(frozen.frozen(), "the mode is not the box's");
        assert!(frozen.thaw(later + mins(1)));
        assert!(frozen.more_like.is_empty() && frozen.less_like.is_empty());

        // The clear as built, a new lane then the box: in Drift exactly the
        // unpinned Less like rows are left; in Stay close nothing; the file
        // keeps nothing either way.
        let mut cleared = rows();
        cleared.new_lane(SHEET);
        cleared.without_pinned(SHEET);
        assert!(cleared.more_like.is_empty());
        assert_eq!(paths(&cleared.less_like), ["s"], "exactly the unpinned Less like rows");
        assert_eq!(cleared.to_prefs(SHEET), (Vec::new(), Vec::new()), "no pin left to keep");
        let mut cleared = rows();
        cleared.freeze(later);
        cleared.new_lane(SHEET);
        cleared.without_pinned(SHEET);
        assert!(cleared.more_like.is_empty() && cleared.less_like.is_empty(), "nothing left in Stay close");
        assert_eq!(cleared.to_prefs(SHEET), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_new_queues_song_comes_in_newest_beside_the_pins() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        for name in ["p1", "p2"] {
            seeds.add_manual(SeedList::MoreLike, key(name), track(name), now);
        }
        picked(&mut seeds, &["a", "b", "c", "d", "e"], now);
        assert!(seeds.push_pick(key("x"), track("x"), SeedFrom::Opener, now));
        assert_eq!(paths(&seeds.more_like), ["x", "e", "d", "c", "b", "p2", "p1"], "a, the oldest unpinned, left");
        assert!(!seeds.more_like[0].pinned && seeds.more_like[0].from == SeedFrom::Opener);
        assert!(seeds.more_like[5..].iter().all(|s| s.pinned), "the pins stay, in their order");

        // Eight pins: the song does not stay, and nothing moves.
        let mut full = Seeds::default();
        for i in 0..SEEDS_MAX {
            let name = format!("p{i}");
            full.add_manual(SeedList::MoreLike, key(&name), track(&name), now);
        }
        let before = full.clone();
        assert!(!full.push_pick(key("x"), track("x"), SeedFrom::Opener, now));
        assert_eq!(full, before);
    }

    #[test]
    fn whether_a_lanes_first_pick_blends_its_song() {
        // Drift, the lane's first pick, on the DJ's server, in neither list,
        // not one of this lane's picks.
        let yes = [true, true, true, false, false, false];
        let blends = |v: [bool; 6]| blends_at_lane_start(v[0], v[1], v[2], v[3], v[4], v[5]);
        assert!(blends(yes));
        let why = [
            "Stay close",
            "a later pick of the lane",
            "another server's row",
            "a Less like song: the user's word wins",
            "already in More like: it keeps its place and its pin",
            "this lane's own pick joins as it starts",
        ];
        for (i, why) in why.iter().enumerate() {
            let mut flipped = yes;
            flipped[i] = !flipped[i];
            assert!(!blends(flipped), "{why}");
        }
    }

    #[test]
    fn whether_a_chosen_first_song_is_added() {
        // A sheet that shows the pin, Stay close, a song the user chose,
        // room in More like.
        let yes = [true, true, true, true];
        let adds = |v: [bool; 4]| adds_chosen_song(v[0], v[1], v[2], v[3]);
        assert!(adds(yes));
        let why = [
            "the TUI shows no pin: the first real pick holds to it as the one song",
            "Drift: the opener joins unpinned",
            "Random's song is the DJ's",
            "More like can take no row",
        ];
        for (i, why) in why.iter().enumerate() {
            let mut flipped = yes;
            flipped[i] = !flipped[i];
            assert!(!adds(flipped), "{why}");
        }
    }

    #[test]
    fn with_no_sheet_stay_close_keeps_the_rules_from_before_the_second_answers() {
        // A pin, a frozen pick, an unpinned skip and Stay close's one song.
        let t0 = Instant::now();
        let mut seeds = Seeds::default();
        seeds.add_manual(SeedList::MoreLike, key("p"), track("p"), t0);
        picked(&mut seeds, &["b"], t0);
        seeds.quick_skip(&key("s"), &track("s"), true, t0);
        seeds.freeze(t0 + mins(1));
        assert!(seeds.admit_anchor(key("x"), track("x"), t0 + mins(2)));

        // The file keeps the pins alone: no unpinned row, no Drift time.
        let (more, less) = seeds.to_prefs(NO_SHEET);
        let rows: Vec<(&str, bool, Option<u64>)> = more.iter().map(|p| (p.path.as_str(), p.pinned, p.drift_ms)).collect();
        assert_eq!(rows, [("p", true, None)]);
        assert!(less.is_empty(), "the skip is not kept");
        assert_ne!(seeds.kept_hash(NO_SHEET), seeds.kept_hash(SHEET));
        let pins_only = seeds.kept_hash(NO_SHEET);
        let mut another = seeds.clone();
        assert!(another.admit_anchor(key("y"), track("y"), t0 + mins(3)));
        assert_eq!(another.kept_hash(NO_SHEET), pins_only, "Stay close's one song is no pin");
        let sheet_file = seeds.to_prefs(SHEET);

        // A new lane or a new queue: More like's pins alone; the skip stays,
        // its clock as it stood; still frozen.
        let mut lane = seeds.clone();
        assert!(lane.new_lane(NO_SHEET));
        assert_eq!(paths(&lane.more_like), ["p"], "the one song and the frozen pick go");
        assert_eq!(paths(&lane.less_like), ["s"]);
        assert_eq!(lane.less_like[0].since, t0);
        assert!(lane.frozen());
        // The box: the stored pins go, the skip stays (round 1's).
        assert!(lane.without_pinned(NO_SHEET));
        assert!(lane.more_like.is_empty());
        assert_eq!(paths(&lane.less_like), ["s"]);
        assert!(lane.frozen(), "the mode is not the box's");

        // A file the GUI's Stay close wrote: its pins alone here, frozen.
        let l = t0 + mins(60);
        let back = Seeds::from_prefs(&sheet_file.0, &sheet_file.1, true, NO_SHEET, l);
        assert!(back.frozen());
        assert_eq!(paths(&back.more_like), ["p"]);
        assert!(back.less_like.is_empty());

        // Drift is the same in both shells.
        let mut drift = Seeds::default();
        drift.add_manual(SeedList::MoreLike, key("p"), track("p"), t0);
        picked(&mut drift, &["b"], t0);
        drift.quick_skip(&key("s"), &track("s"), true, t0);
        assert_eq!(drift.to_prefs(SHEET), drift.to_prefs(NO_SHEET));
        assert_eq!(drift.kept_hash(SHEET), drift.kept_hash(NO_SHEET));
        let (mut sheet, mut tui) = (drift.clone(), drift.clone());
        assert!(sheet.new_lane(SHEET) && tui.new_lane(NO_SHEET));
        assert_eq!(sheet, tui);
        let (mut sheet, mut tui) = (drift.clone(), drift.clone());
        assert!(sheet.without_pinned(SHEET) && tui.without_pinned(NO_SHEET));
        assert_eq!(sheet, tui);
    }

    #[test]
    fn stay_close_keeps_every_row_through_the_file_and_drift_takes_up_its_clocks() {
        let t0 = Instant::now();
        let mut seeds = Seeds::default();
        seeds.quick_skip(&key("s"), &track("s"), true, t0);
        seeds.add_manual(SeedList::MoreLike, key("p"), track("p"), t0);
        // The most Drift lets stand beside one pin: five.
        picked(&mut seeds, &["a", "b", "c", "d", "e"], t0);
        seeds.freeze(t0 + mins(5));
        // The freeze's own bootstrap may stand past the ring.
        assert!(seeds.admit_anchor(key("x"), track("x"), t0 + mins(5)));

        let (more, less) = seeds.to_prefs(SHEET);
        fn flags(rows: &[SeedPrefs]) -> Vec<(&str, bool)> {
            rows.iter().map(|p| (p.path.as_str(), p.pinned)).collect()
        }
        assert_eq!(
            flags(&more),
            [("x", false), ("e", false), ("d", false), ("c", false), ("b", false), ("a", false), ("p", true)],
            "every row, newest first, each with its pin"
        );
        assert_eq!(flags(&less), [("s", false)]);
        assert_eq!(less[0].drift_ms, Some(300_000), "the five minutes its clock ran before the freeze");
        assert!(more.iter().all(|p| p.drift_ms.is_none()), "a Drift time on unpinned skips only");

        // A restart: every row back with its flag, frozen from the launch,
        // the skip with its five minutes.
        let l = t0 + mins(60);
        let mut back = Seeds::from_prefs(&more, &less, true, SHEET, l);
        assert!(back.frozen());
        assert_eq!(paths(&back.more_like), ["x", "e", "d", "c", "b", "a", "p"]);
        assert_eq!(back.more_like.iter().filter(|s| s.pinned).count(), 1, "the user's own pin alone");
        assert_eq!((back.less_like[0].pinned, back.less_like[0].ran, back.less_like[0].since), (false, mins(5), l));
        assert_eq!(back.to_prefs(SHEET), (more.clone(), less.clone()), "and round again");

        // Back in Drift: More like back to five unpinned beside the pin, and
        // the skip's clock resumes — 25 minutes left.
        assert!(back.thaw(l + mins(10)));
        assert_eq!(paths(&back.more_like), ["x", "e", "d", "c", "b", "p"], "a, the oldest unpinned, left");
        let (kept_more, kept_less) = back.to_prefs(SHEET);
        assert_eq!(flags(&kept_more), [("p", true)], "Drift keeps the pins alone");
        assert!(kept_less.is_empty() && kept_more.iter().all(|p| p.drift_ms.is_none()));
        assert!(!back.prune(l + mins(10) + mins(25) - Duration::from_secs(1)), "kept at 24:59");
        assert!(back.holds(SeedList::LessLike, &key("s")));
        assert!(back.prune(l + mins(10) + mins(25)), "gone at 25:00");

        // The same file read in Drift: the pin alone.
        let drift = Seeds::from_prefs(&more, &less, false, SHEET, l);
        assert!(!drift.frozen());
        assert_eq!(paths(&drift.more_like), ["p"]);
        assert!(drift.less_like.is_empty());
    }

    #[test]
    fn a_skips_clock_restarts_whole_on_a_re_skip_and_an_unpin() {
        // Two skips loaded from Stay close with 20 of their 30 minutes run,
        // then thawed: due to go ten minutes later.
        let l = Instant::now();
        let row = |path: &str| SeedPrefs {
            server: HOST.into(),
            path: path.into(),
            pinned: false,
            drift_ms: Some(20 * 60 * 1000),
            ..Default::default()
        };
        let thawed = || {
            let mut seeds = Seeds::from_prefs(&[], &[row("r"), row("u")], true, SHEET, l);
            let t = l + mins(1);
            assert!(seeds.thaw(t));
            (seeds, t)
        };
        let (seeds, t) = thawed();
        assert!(seeds.less_like.iter().all(|s| s.ran == mins(20)));
        assert!(!seeds.clone().prune(t + mins(10) - Duration::from_secs(1)));
        assert!(seeds.clone().prune(t + mins(10)), "due ten minutes after the thaw");

        // A re-skip restarts the clock whole.
        let (mut seeds, t) = thawed();
        let later = t + mins(5);
        assert!(seeds.quick_skip(&key("r"), &track("r"), true, later));
        let r = &seeds.less_like[0];
        assert_eq!((r.key.path.as_str(), r.ran, r.since), ("r", Duration::ZERO, later));
        seeds.prune(later + mins(30) - Duration::from_secs(1));
        assert!(seeds.holds(SeedList::LessLike, &key("r")), "kept at 29:59");
        assert!(!seeds.holds(SeedList::LessLike, &key("u")), "the other's clock ran on");
        assert!(seeds.prune(later + mins(30)), "gone at 30:00");

        // So does an unpin, from a clock that had run.
        let (mut seeds, t) = thawed();
        let later = t + mins(5);
        assert!(seeds.set_pinned(SeedList::LessLike, &key("u"), true, later));
        let u = |seeds: &Seeds| seeds.less_like.iter().find(|s| s.key == key("u")).cloned();
        assert_eq!(u(&seeds).unwrap().ran, mins(20), "a pin keeps what its clock had run");
        assert!(seeds.set_pinned(SeedList::LessLike, &key("u"), false, later));
        let unpinned = u(&seeds).unwrap();
        assert_eq!((unpinned.ran, unpinned.since), (Duration::ZERO, later));
        seeds.prune(later + mins(10));
        assert!(u(&seeds).is_some(), "a clock that kept its past would go at 10:00");
        seeds.prune(later + mins(30) - Duration::from_secs(1));
        assert!(u(&seeds).is_some(), "kept at 29:59");
        seeds.prune(later + mins(30));
        assert!(u(&seeds).is_none(), "gone at 30:00");
    }

    #[test]
    fn what_the_file_keeps_moves_its_fingerprint_and_nothing_else_does() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        let empty = seeds.kept_hash(SHEET);
        assert_eq!(Seeds::default().kept_hash(SHEET), empty, "equal sets, equal fingerprints");
        // Drift keeps the pins alone: a pick and a skip leave it.
        picked(&mut seeds, &["a"], now);
        seeds.quick_skip(&key("s"), &track("s"), true, now);
        assert_eq!(seeds.kept_hash(SHEET), empty, "a Drift pick and an unpinned skip");
        pin(&mut seeds, SeedList::MoreLike, "a", now);
        let pinned = seeds.kept_hash(SHEET);
        assert_ne!(pinned, empty, "a pin");
        assert!(seeds.add_manual(SeedList::LessLike, key("l"), track("l"), now));
        let added = seeds.kept_hash(SHEET);
        assert_ne!(added, pinned, "Add song");
        assert_eq!(seeds.clone().kept_hash(SHEET), added);

        // Stay close keeps every row: its one song moves it, time does not.
        seeds.freeze(now + mins(1));
        let frozen = seeds.kept_hash(SHEET);
        assert_ne!(frozen, added, "frozen, the unpinned rows are kept too");
        assert!(seeds.admit_anchor(key("x"), track("x"), now + mins(2)));
        let admitted = seeds.kept_hash(SHEET);
        assert_ne!(admitted, frozen, "Stay close's one song");
        assert!(!seeds.prune(now + mins(90)));
        assert_eq!(seeds.kept_hash(SHEET), admitted, "time passing while frozen");

        // Back in Drift the unpinned rows leave what is kept.
        seeds.thaw(now + mins(3));
        assert_eq!(seeds.kept_hash(SHEET), added, "the pins alone again");
    }

    #[test]
    fn a_songs_tags_fill_its_rows_in_both_lists_and_nothing_else() {
        let now = Instant::now();
        let bare = |path: &str| Track { filepath: path.into(), metadata: TrackMetadata::default() };
        let tags = |title: &str, artist: &str, album: &str| TrackMetadata {
            title: Some(title.into()),
            artist: Some(artist.into()),
            album: Some(album.into()),
            ..Default::default()
        };
        let shown = |seed: &Seed| {
            let m = &seed.track.metadata;
            (m.title.clone(), m.artist.clone(), m.album.clone())
        };
        let some = |v: &str| Some(v.to_string());
        let mut seeds = Seeds::default();
        seeds.push_pick(key("a"), bare("a"), SeedFrom::Pick, now);
        let own = Track { filepath: "c".into(), metadata: TrackMetadata { title: some("Own"), ..Default::default() } };
        seeds.push_pick(key("c"), own, SeedFrom::Pick, now);
        seeds.push_pick(far_key("a"), bare("a"), SeedFrom::Pick, now);
        seeds.quick_skip(&key("b"), &bare("b"), true, now);
        let row = |seeds: &Seeds, list: SeedList, key: &SeedKey| seeds.list(list).iter().find(|s| s.key == *key).cloned().unwrap();

        assert!(seeds.fill_tags(&key("a"), &tags("A", "Band", "LP")));
        assert_eq!(shown(&row(&seeds, SeedList::MoreLike, &key("a"))), (some("A"), some("Band"), some("LP")));
        assert_eq!(shown(&row(&seeds, SeedList::MoreLike, &far_key("a"))), (None, None, None), "another server's song");
        assert!(!seeds.fill_tags(&key("a"), &tags("A", "Band", "LP")), "nothing left to fill");
        assert!(seeds.fill_tags(&key("c"), &tags("C", "Band", "LP")));
        assert_eq!(shown(&row(&seeds, SeedList::MoreLike, &key("c"))), (some("Own"), some("Band"), some("LP")), "its own title stays");
        assert!(seeds.fill_tags(&key("b"), &tags("B", "Band", "LP")));
        assert_eq!(shown(&row(&seeds, SeedList::LessLike, &key("b"))), (some("B"), some("Band"), some("LP")), "Less like's row too");
        assert!(!seeds.fill_tags(&key("nope"), &tags("N", "Band", "LP")));

        // A pin's tags are what the file keeps: the fingerprint moves. An
        // unpinned Drift row's are not.
        let mut pinned = Seeds::default();
        pinned.push_pick(key("a"), bare("a"), SeedFrom::Pick, now);
        assert!(pinned.set_pinned(SeedList::MoreLike, &key("a"), true, now));
        let before = pinned.kept_hash(SHEET);
        assert!(pinned.fill_tags(&key("a"), &tags("A", "Band", "LP")));
        assert_ne!(pinned.kept_hash(SHEET), before);
        let mut drift = Seeds::default();
        drift.push_pick(key("d"), bare("d"), SeedFrom::Pick, now);
        let before = drift.kept_hash(SHEET);
        assert!(drift.fill_tags(&key("d"), &tags("D", "Band", "LP")));
        assert_eq!(drift.kept_hash(SHEET), before);
    }

    #[test]
    fn the_users_own_row_comes_in_pinned_and_takes_its_song_from_the_other_list() {
        let now = Instant::now();
        let later = now + mins(1);
        let mut seeds = Seeds::default();
        assert!(seeds.add_manual(SeedList::MoreLike, key("a"), track("a"), now));
        assert!(seeds.more_like[0].pinned && seeds.more_like[0].from == SeedFrom::Manual);
        // The other list's row moves over, pin and all.
        seeds.add_manual(SeedList::LessLike, key("b"), track("b"), now);
        assert!(seeds.add_manual(SeedList::MoreLike, key("b"), track("b"), later));
        assert_eq!(paths(&seeds.more_like), ["b", "a"]);
        assert!(seeds.less_like.is_empty(), "a song in one list only");
        // One the list holds becomes pinned and newest.
        picked(&mut seeds, &["c"], now);
        assert!(seeds.add_manual(SeedList::MoreLike, key("a"), track("a"), later));
        assert_eq!(paths(&seeds.more_like), ["a", "c", "b"]);
        assert!(seeds.add_manual(SeedList::MoreLike, key("c"), track("c"), later));
        assert!(seeds.more_like[0].pinned);
        assert_eq!(seeds.more_like[0].from, SeedFrom::Pick, "its origin stays");

        // Frozen and full: refused; in Drift, full with an unpinned row:
        // the oldest unpinned makes room.
        let mut full = Seeds::default();
        for i in 0..SEEDS_MAX {
            full.quick_skip(&key(&format!("s{i}")), &track("s"), true, now);
        }
        full.freeze(now);
        assert!(!full.can_take(SeedList::LessLike));
        assert!(!full.add_manual(SeedList::LessLike, key("m"), track("m"), now), "frozen and full");
        full.thaw(now);
        assert!(full.add_manual(SeedList::LessLike, key("m"), track("m"), now));
        assert_eq!(full.less_like.len(), SEEDS_MAX);
        assert!(!full.holds(SeedList::LessLike, &key("s0")), "evicted");
    }

    #[test]
    fn a_pin_restarts_less_likes_clock_when_it_goes_and_its_trim_spares_the_row() {
        let now = Instant::now();
        let later = now + mins(20);
        let mut seeds = Seeds::default();
        seeds.add_manual(SeedList::LessLike, key("l"), track("l"), now);
        assert!(!seeds.set_pinned(SeedList::LessLike, &key("l"), true, later), "unchanged");
        assert!(!seeds.set_pinned(SeedList::LessLike, &key("nope"), false, later), "not there");
        assert!(seeds.set_pinned(SeedList::LessLike, &key("l"), false, later));
        assert_eq!(seeds.less_like[0].since, later, "unpinning restarts its clock");
        assert!(!seeds.prune(later + mins(29)));
        assert!(seeds.prune(later + mins(30)));

        // Five unpinned stand; unpinning a sixth trims the oldest OTHER one.
        let mut more = Seeds::default();
        more.add_manual(SeedList::MoreLike, key("old"), track("old"), now);
        picked(&mut more, &["a", "b", "c", "d", "e"], now);
        assert_eq!(more.more_like.len(), 6);
        assert_eq!(more.more_like.last().unwrap().key, key("old"), "the pinned row is the oldest");
        assert!(more.set_pinned(SeedList::MoreLike, &key("old"), false, later));
        assert!(more.holds(SeedList::MoreLike, &key("old")), "the row just unpinned stays under the click");
        assert_eq!(paths(&more.more_like), ["e", "d", "c", "b", "old"], "a, the oldest other, left");
    }

    #[test]
    fn a_row_moves_pinned_to_the_other_list_and_remove_deletes_it_in_either_mode() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        picked(&mut seeds, &["a", "b"], now);
        assert!(seeds.move_to_other(SeedList::MoreLike, &key("a"), now));
        assert_eq!(paths(&seeds.more_like), ["b"]);
        assert_eq!(paths(&seeds.less_like), ["a"]);
        assert!(seeds.less_like[0].pinned && seeds.less_like[0].from == SeedFrom::Manual);
        assert!(!seeds.move_to_other(SeedList::MoreLike, &key("nope"), now));
        seeds.freeze(now);
        assert!(seeds.move_to_other(SeedList::LessLike, &key("a"), now), "frozen: still the user's");
        assert_eq!(paths(&seeds.more_like), ["a", "b"]);
        assert!(seeds.remove(SeedList::MoreLike, &key("b")), "frozen: still the user's");
        assert!(!seeds.remove(SeedList::MoreLike, &key("b")));
        assert_eq!(paths(&seeds.more_like), ["a"]);

        // A full list shown pinned takes no move.
        let mut full = Seeds::default();
        for i in 0..SEEDS_MAX {
            let name = format!("p{i}");
            full.add_manual(SeedList::LessLike, key(&name), track(&name), now);
        }
        full.push_pick(key("m"), track("m"), SeedFrom::Pick, now);
        assert!(!full.move_to_other(SeedList::MoreLike, &key("m"), now));
        assert!(full.holds(SeedList::MoreLike, &key("m")), "the row stays where it was");
    }

    #[test]
    fn the_wire_holds_a_servers_rows_newest_first_and_eight_at_most() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        seeds.push_pick(key("/one.mp3"), track("one.mp3"), SeedFrom::Pick, now);
        let peer = Origin { server: HOST.into(), peer: Some(2) };
        seeds.push_pick(SeedKey::new(&peer, "theirs.mp3"), track("theirs.mp3"), SeedFrom::Pick, now);
        let other = Origin { server: "http://other:3000".into(), peer: None };
        seeds.push_pick(SeedKey::new(&other, "far.mp3"), track("far.mp3"), SeedFrom::Pick, now);
        seeds.push_pick(key("two.mp3"), track("two.mp3"), SeedFrom::Pick, now);
        assert_eq!(seeds.positives(&origin()), ["two.mp3", "one.mp3"], "wire form, newest first, this server only");
        assert_eq!(seeds.positives(&Origin { server: format!("{HOST}/"), peer: None }), ["two.mp3", "one.mp3"]);
        assert_eq!(seeds.positives(&peer), ["theirs.mp3"], "a peer's own");
        assert_eq!(seeds.positives(&other), ["far.mp3"]);
        for i in 0..SEEDS_MAX {
            seeds.quick_skip(&key(&format!("s{i}")), &track("s"), true, now);
        }
        seeds.quick_skip(&SeedKey::new(&other, "s9"), &track("s"), true, now);
        let negatives = seeds.negatives(&origin());
        assert_eq!(negatives.len(), 7, "the other server's row took one of the eight");
        assert_eq!(negatives[0], "s7");
        assert!(seeds.negatives(&peer).is_empty());
    }

    #[test]
    fn the_stored_pins_round_trip_in_order_and_a_bad_row_is_let_go() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        picked(&mut seeds, &["a", "b", "c"], now);
        pin(&mut seeds, SeedList::MoreLike, "a", now);
        pin(&mut seeds, SeedList::MoreLike, "c", now);
        let peer = Origin { server: HOST.into(), peer: Some(7) };
        let theirs = Track {
            filepath: "/x.mp3".into(),
            metadata: TrackMetadata {
                title: Some("X".into()),
                artist: Some("Y".into()),
                album: Some("Z".into()),
                ..Default::default()
            },
        };
        seeds.add_manual(SeedList::LessLike, SeedKey::new(&peer, "/x.mp3"), theirs, now);
        seeds.quick_skip(&key("s"), &track("s"), true, now);
        let (more, less) = seeds.to_prefs(SHEET);
        assert_eq!(more.iter().map(|p| p.path.as_str()).collect::<Vec<_>>(), ["c", "a"], "stored pins only, in order");
        assert_eq!(
            less,
            [SeedPrefs {
                server: HOST.into(),
                peer: Some(7),
                path: "x.mp3".into(),
                title: Some("X".into()),
                artist: Some("Y".into()),
                album: Some("Z".into()),
                pinned: true,
                drift_ms: None,
            }],
            "the skip is not kept"
        );

        let later = now + mins(1);
        let back = Seeds::from_prefs(&more, &less, false, SHEET, later);
        assert_eq!(paths(&back.more_like), ["c", "a"]);
        assert!(back.more_like.iter().chain(&back.less_like).all(|s| s.pinned && s.since == later));
        assert_eq!(back.less_like[0].key, SeedKey::new(&peer, "x.mp3"));
        assert_eq!(back.less_like[0].track.metadata.artist.as_deref(), Some("Y"));
        assert_eq!(back.to_prefs(SHEET), (more.clone(), less.clone()), "and round again");
        assert!(!back.frozen());
        assert!(Seeds::from_prefs(&more, &less, true, SHEET, later).frozen(), "Stay close loads frozen");

        // Empty rows, duplicates (across lists too) and the cap.
        let row = |server: &str, path: &str| SeedPrefs { server: server.into(), path: path.into(), ..Default::default() };
        let mut many: Vec<SeedPrefs> = (0..10).map(|i| row(HOST, &format!("m{i}"))).collect();
        many.insert(0, row("", "nowhere"));
        many.insert(1, row(HOST, " "));
        many.insert(2, row(&format!("{HOST}/"), "/m0"));
        let loaded = Seeds::from_prefs(&many, &[row(HOST, "m1"), row(HOST, "l")], false, SHEET, later);
        assert_eq!(loaded.more_like.len(), SEEDS_MAX, "eight at most");
        assert_eq!(loaded.more_like[0].key, key("m0"), "a key keeps its first row");
        assert_eq!(paths(&loaded.less_like), ["l"], "a key in both lists keeps More like's");
    }

    #[test]
    fn a_removed_or_renamed_server_takes_its_rows_along() {
        let now = Instant::now();
        let mut seeds = Seeds::default();
        let other = Origin { server: "http://other:3000".into(), peer: None };
        let peer = Origin { server: HOST.into(), peer: Some(3) };
        seeds.add_manual(SeedList::MoreLike, key("a"), track("a"), now);
        seeds.add_manual(SeedList::MoreLike, SeedKey::new(&other, "b"), track("b"), now);
        seeds.add_manual(SeedList::LessLike, SeedKey::new(&peer, "c"), track("c"), now);
        assert!(seeds.rename_server(&format!("{HOST}/"), "http://new:3000"));
        assert_eq!(seeds.more_like[1].key.server, "http://new:3000");
        assert_eq!(seeds.less_like[0].key, SeedKey::new(&Origin { server: "http://new:3000".into(), peer: Some(3) }, "c"));
        assert_eq!(seeds.more_like[0].key, SeedKey::new(&other, "b"), "another server's rows stay");
        assert!(!seeds.rename_server("http://gone:1", "http://x:1"));
        assert!(seeds.drop_where(|k| crate::config::same_server(&k.server, "http://new:3000") && k.peer == Some(3)));
        assert!(seeds.less_like.is_empty() && seeds.more_like.len() == 2, "the peer's rows only");
        assert!(seeds.drop_where(|k| crate::config::same_server(&k.server, "http://new:3000")));
        assert_eq!(paths(&seeds.more_like), ["b"], "pinned or not");
        assert!(!seeds.drop_where(|_| false));

        // A rename onto a spelling the lists already hold keeps one row.
        let mut twice = Seeds::default();
        twice.add_manual(SeedList::LessLike, key("a"), track("a"), now);
        twice.add_manual(SeedList::MoreLike, SeedKey::new(&other, "a"), track("a"), now);
        twice.rename_server("http://other:3000", HOST);
        assert_eq!(paths(&twice.more_like), ["a"]);
        assert!(twice.less_like.is_empty(), "More like's row wins");
    }

    #[test]
    fn whether_a_quick_skip_takes_out_the_lanes_blended_song() {
        let two = key("two");
        assert!(quick_skips_blend(&two, Some(&key("two")), false, true), "the user's own blended song, unpinned in More like");
        assert!(quick_skips_blend(&two, Some(&key("/two")), false, true), "one key with or without the leading slash");
        assert!(!quick_skips_blend(&two, None, false, true), "a lane that blended nothing");
        assert!(!quick_skips_blend(&two, Some(&key("other")), false, true), "another song");
        assert!(!quick_skips_blend(&two, Some(&two), false, false), "pinned since, removed, moved or rotated out");
        assert!(!quick_skips_blend(&two, Some(&two), true, true), "a DJ pick the start blended in keeps the DJ pick's rule");
    }

    #[test]
    fn the_songs_a_lane_passes_over_are_those_on_the_picks_server() {
        let peer = Origin { server: HOST.into(), peer: Some(4) };
        let passed = [key("a"), far_key("a"), SeedKey::new(&peer, "b")];
        assert_eq!(pass_over_on(&passed, &origin()), ["a"]);
        assert_eq!(pass_over_on(&passed, &far()), ["a"]);
        assert_eq!(pass_over_on(&passed, &peer), ["b"], "a peer's own");
        assert!(pass_over_on(&[], &origin()).is_empty(), "no mark");
        assert_eq!(pass_over_on(&[key("/a")], &origin()), ["a"], "wire form");
    }

    #[test]
    fn a_quick_skip_is_a_short_listen_that_moves_on() {
        let quick = |heard: u64, duration: Option<u64>| is_quick_skip(true, heard, duration, false, 0, Some(1), false);
        assert!(!quick(999, None), "under a second is a mis-click");
        assert!(quick(1_000, None));
        assert!(quick(29_900, None));
        assert!(!quick(30_000, None), "thirty seconds is a play");
        assert!(!quick(10_000, Some(20_000)), "half a known length is a play");
        assert!(quick(9_999, Some(20_000)));
        assert!(quick(29_999, Some(0)), "an unknown length: thirty seconds only");
        assert!(!is_quick_skip(true, 5_000, None, true, 0, Some(1), false), "repeat-one");
        assert!(!is_quick_skip(true, 5_000, None, false, 3, Some(0), false), "a wrap");
        assert!(!is_quick_skip(true, 5_000, None, false, 3, None, false), "the last row");
        assert!(!is_quick_skip(true, 5_000, None, false, 3, Some(3), true), "the same row");
        assert!(!is_quick_skip(false, 5_000, None, false, 0, Some(1), false), "not a DJ pick");
        assert!(is_quick_skip(true, 5_000, None, false, 3, Some(1), true), "shuffle may go backward");
    }

    #[test]
    fn a_reroll_replaces_the_dj_rows_after_the_playing_one() {
        let row = |dj: bool| Queued {
            origin: origin(),
            dj: dj.then_some(DjMark { sonic: true }),
            track: track("t"),
        };
        let items = vec![row(false), row(true), row(true), row(false), row(true)];
        assert_eq!(reroll_slots(&items, Some(1), false), [2, 4], "nearest first, user rows skipped");
        assert_eq!(reroll_slots(&items, Some(0), false), [1, 2, 4]);
        assert!(reroll_slots(&items, Some(4), false).is_empty(), "nothing after the last row");
        assert!(reroll_slots(&items, Some(1), true).is_empty(), "nothing under shuffle");
        assert!(reroll_slots(&items, None, false).is_empty(), "no playing row: everything appends");
    }
}
