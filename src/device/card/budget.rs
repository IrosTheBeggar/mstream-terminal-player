//! `budget.json`, beside `cards.json` in the player's config folder: what
//! this computer has pulled from each federation peer, by UTC day, against
//! a daily budget (the transfer plan's 2.15, part B9).
//!
//! **Why.** A peer's key carries a daily quota that every user of the
//! parent, the parent's own copy plug-in and every guest device share
//! (mStream's `federation-limits.js`), and an older peer says nothing of it
//! to the key's holder until a 429. So a card transfer keeps to a cap of
//! its own: 1,500 MB a day for each peer (three quarters of the 2,048 MB
//! mStream gives a new key), the listener's own figure when they give one,
//! or the key's real quota when the peer shows it (its
//! `user.federationAllowance`, the card transfer's batch of mStream
//! changes). The cap is self-imposed; the peer's 429 is the truth.
//!
//! **Every byte** read from a peer counts, on its UTC day: files, covers,
//! JSON and probes alike, as the peer counts them (its clients' meter,
//! [`Spend`]). A peer whose day is spent, by the budget or by its quota,
//! waits: its `resumeAt` says when it may be asked again — the next 00:00
//! UTC, or the peer's own Retry-After, and a few random minutes
//! ([`resume_after`]).
//!
//! **Two programs, one file.** The GUI's worker and a command-line run may
//! pull from one peer at once, so every read and write of the file is made
//! under an OS lock of `budget.lock` beside it (std's, `LockFileEx` or
//! `flock`), and the file is replaced whole through a temporary one.
//!
//! The file names a peer by its parent's URL key (2.5.1: the FNV-1a of the
//! normalised URL, never the URL) and its id in the parent's peer list.
//! Every member this player doesn't know is kept. A newer player's file is
//! never written over, and no peer is read while it is there: its counts
//! couldn't be kept.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

use super::state::{from_hex, hex};
use crate::api::transfer::Allowance;

/// The file's own `format`, and its schema.
pub(crate) const FORMAT: &str = "mstream-card-budget";
pub(crate) const VERSION: u64 = 1;
/// mStream's MB (`federation-limits.js`): a mebibyte.
pub(crate) const MB: u64 = 1024 * 1024;
/// The daily budget of a peer whose key shows no quota and for which the
/// listener named none (section 6, item 7).
pub(crate) const DEFAULT_MB: u64 = 1_500;
const DAY: u64 = 86_400;
const FILE: &str = "budget.json";
const LOCK: &str = "budget.lock";
const TEMP: &str = "budget.tmp";
/// The members of a peer's entry this player reads and writes.
const KNOWN: [&str; 4] = ["dailyMb", "day", "usedBytes", "resumeAt"];
/// How far the meter runs ahead of the file before a run writes it again.
const SYNC_EVERY: u64 = 4 * MB;

/// The clock a budget is kept by: Unix seconds now.
pub(crate) type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The system's clock.
pub(crate) fn system_clock() -> Clock {
    Arc::new(|| {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        since.map_or(0, |d| d.as_secs())
    })
}

/// The UTC day of `now` (Unix seconds): days since 1970-01-01.
pub(crate) fn utc_day(now: u64) -> u64 {
    now / DAY
}

/// The next 00:00 UTC after `now`.
pub(crate) fn next_midnight(now: u64) -> u64 {
    (utc_day(now) + 1) * DAY
}

/// When a peer whose day is spent may be asked again: after its own
/// Retry-After (the seconds to its UTC midnight), else after the next
/// 00:00 UTC by this computer's clock; and `jitter` seconds more, so the
/// computers of one parent don't all ask at once.
pub(crate) fn resume_after(now: u64, retry_after: Option<u64>, jitter: u64) -> u64 {
    match retry_after {
        Some(secs) => now + secs,
        None => next_midnight(now),
    }
    .saturating_add(jitter)
}

/// A few random minutes: between one and ten.
pub(crate) fn jitter() -> u64 {
    fastrand::u64(60..=600)
}

/// The UTC days a pull of `bytes` spans, today's the first: `left` is what
/// today has left, `cap` a whole day's (None: no cap, so one day).
pub(crate) fn days_for(bytes: u64, left: Option<u64>, cap: Option<u64>) -> u64 {
    match (left, cap) {
        _ if bytes == 0 => 0,
        (None, _) | (_, None) => 1,
        (Some(left), _) if bytes <= left => 1,
        (Some(left), Some(cap)) => 1 + (bytes - left).div_ceil(cap.max(1)),
    }
}

/// A UTC day as the file spells it: `2026-10-18`.
fn date(day: u64) -> String {
    // The civil date of a day count (Howard Hinnant's algorithm).
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// [`date`]'s text back to its day; None for anything else.
fn day_of(text: &str) -> Option<u64> {
    let seconds = super::plan::unix_seconds(&format!("{text} 00:00:00"))?;
    (date(utc_day(seconds as u64)) == text).then(|| utc_day(seconds as u64))
}

// ── The file ────────────────────────────────────────────────────────────────

/// A peer, as the file names it: its parent's URL key, and its id there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PeerKey {
    pub(crate) parent_key: u64,
    pub(crate) peer_id: u32,
}

impl PeerKey {
    /// `0x00AA…/3`.
    fn text(&self) -> String {
        format!("{}/{}", hex(self.parent_key), self.peer_id)
    }

    fn parse(text: &str) -> Option<PeerKey> {
        let (parent, id) = text.split_once('/')?;
        let peer_id = id.parse().ok().filter(|&id| id != 0)?;
        Some(PeerKey { parent_key: from_hex(parent).filter(|&k| k != 0)?, peer_id })
    }
}

/// One peer's day.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Account {
    /// The listener's own budget, in MiB a day; None: the key's quota where
    /// the peer shows it, else [`DEFAULT_MB`].
    pub(crate) daily_mb: Option<u64>,
    /// The UTC day `used` is of.
    pub(crate) day: u64,
    /// The bytes pulled from the peer that day.
    pub(crate) used: u64,
    /// Unix seconds before which the peer isn't asked; 0: none.
    pub(crate) resume_at: u64,
    keep: Map<String, Value>,
}

/// The file's whole contents.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Book {
    pub(crate) peers: BTreeMap<PeerKey, Account>,
    keep: Map<String, Value>,
    /// A newer player's file: read as none, and never written over.
    newer: bool,
}

impl Book {
    /// The book a file holds: none when it doesn't read as one.
    pub(crate) fn parse(text: &str) -> Book {
        let Ok(Value::Object(top)) = serde_json::from_str::<Value>(text) else {
            return Book::default();
        };
        if top.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return Book::default();
        }
        match top.get("version").and_then(Value::as_u64) {
            Some(VERSION) => {}
            Some(v) if v > VERSION => return Book { newer: true, ..Book::default() },
            _ => return Book::default(),
        }
        let mut peers = BTreeMap::new();
        for (name, entry) in top.get("peers").and_then(Value::as_object).into_iter().flatten() {
            let (Some(key), Some(entry)) = (PeerKey::parse(name), entry.as_object()) else {
                continue;
            };
            let number = |k: &str| entry.get(k).and_then(Value::as_u64);
            let keep = entry.iter().filter(|(n, _)| !KNOWN.contains(&n.as_str()));
            let account = Account {
                daily_mb: number("dailyMb").filter(|&mb| mb > 0),
                day: entry.get("day").and_then(Value::as_str).and_then(day_of).unwrap_or(0),
                used: number("usedBytes").unwrap_or(0),
                resume_at: number("resumeAt").unwrap_or(0),
                keep: keep.map(|(n, v)| (n.clone(), v.clone())).collect(),
            };
            peers.insert(key, account);
        }
        let keep = top.iter().filter(|(n, _)| !matches!(n.as_str(), "format" | "version" | "peers"));
        let keep = keep.map(|(n, v)| (n.clone(), v.clone())).collect();
        Book { peers, keep, newer: false }
    }

    /// The file's text.
    pub(crate) fn text(&self) -> String {
        let mut peers = Map::new();
        for (key, a) in &self.peers {
            let mut entry = a.keep.clone();
            if let Some(mb) = a.daily_mb {
                entry.insert("dailyMb".into(), mb.into());
            }
            entry.insert("day".into(), date(a.day).into());
            entry.insert("usedBytes".into(), a.used.into());
            if a.resume_at != 0 {
                entry.insert("resumeAt".into(), a.resume_at.into());
            }
            peers.insert(key.text(), Value::Object(entry));
        }
        let mut top = self.keep.clone();
        top.insert("format".into(), FORMAT.into());
        top.insert("version".into(), VERSION.into());
        top.insert("peers".into(), Value::Object(peers));
        serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_default() + "\n"
    }

    fn account(&mut self, key: PeerKey) -> &mut Account {
        self.peers.entry(key).or_default()
    }

    /// The bytes pulled from the peer on `now`'s UTC day.
    pub(crate) fn used_today(&self, key: PeerKey, now: u64) -> u64 {
        self.peers.get(&key).filter(|a| a.day == utc_day(now)).map_or(0, |a| a.used)
    }

    /// `bytes` more pulled from the peer, on `now`'s day.
    pub(crate) fn spend(&mut self, key: PeerKey, bytes: u64, now: u64) {
        let a = self.account(key);
        if a.day != utc_day(now) {
            (a.day, a.used) = (utc_day(now), 0);
        }
        a.used = a.used.saturating_add(bytes);
    }

    /// A whole day's budget of the peer, in bytes; None: no cap. The
    /// listener's own, else the key's quota when the peer shows it (none
    /// for a key without one), else [`DEFAULT_MB`].
    pub(crate) fn cap(&self, key: PeerKey, allowance: Option<&Allowance>) -> Option<u64> {
        if let Some(mb) = self.peers.get(&key).and_then(|a| a.daily_mb) {
            return Some(mb.saturating_mul(MB));
        }
        match allowance.map(|a| a.daily_mb) {
            Some(Some(0)) => None,
            Some(Some(mb)) => Some(mb.saturating_mul(MB)),
            _ => Some(DEFAULT_MB * MB),
        }
    }

    /// What is left of the peer's budget on `now`'s day; None: no cap.
    pub(crate) fn left(&self, key: PeerKey, now: u64, allowance: Option<&Allowance>) -> Option<u64> {
        self.cap(key, allowance).map(|cap| cap.saturating_sub(self.used_today(key, now)))
    }

    /// The listener's own daily budget for the peer, in MiB; None: back to
    /// the key's quota or the default.
    pub(crate) fn set_limit(&mut self, key: PeerKey, mb: Option<u64>) {
        self.account(key).daily_mb = mb.filter(|&mb| mb > 0);
    }

    /// The peer isn't asked before `until` (Unix seconds).
    pub(crate) fn park(&mut self, key: PeerKey, until: u64) {
        self.account(key).resume_at = until;
    }

    /// When the peer may be asked again, if it is waiting at `now`.
    pub(crate) fn waiting(&self, key: PeerKey, now: u64) -> Option<u64> {
        self.peers.get(&key).map(|a| a.resume_at).filter(|&at| at > now)
    }
}

/// The book in its folder: [`Budget::system`]'s is the player's config
/// folder, a test's its own.
#[derive(Clone, Debug)]
pub(crate) struct Budget {
    dir: PathBuf,
}

impl Budget {
    pub(crate) fn at(dir: impl Into<PathBuf>) -> Budget {
        Budget { dir: dir.into() }
    }

    /// This computer's, beside `cards.json`.
    pub(crate) fn system() -> Result<Budget, String> {
        crate::config::config_dir().map(Budget::at)
    }

    /// `f` on the book, under the lock: read, changed, and written back
    /// whole when it changed. Err for a newer player's book, which is left
    /// as it is.
    pub(crate) fn with<T>(&self, f: impl FnOnce(&mut Book) -> T) -> Result<T, String> {
        let file = self.dir.join(FILE);
        let at = |what: &str, e: io::Error| format!("{what} {}: {e}", file.display());
        fs::create_dir_all(&self.dir).map_err(|e| at("could not make the folder of", e))?;
        let mut options = File::options();
        let lock = options.create(true).truncate(false).write(true).open(self.dir.join(LOCK));
        let lock = lock.map_err(|e| at("could not lock", e))?;
        lock.lock().map_err(|e| at("could not lock", e))?;
        let text = match fs::read_to_string(self.dir.join(FILE)) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(at("could not read", e)),
        };
        let mut book = text.as_deref().map(Book::parse).unwrap_or_default();
        if book.newer {
            return Err(format!("{} is a newer player's: its counts can't be kept", file.display()));
        }
        let before = book.clone();
        let out = f(&mut book);
        if book != before {
            let temp = self.dir.join(TEMP);
            let write = || -> io::Result<()> {
                let mut file = File::create(&temp)?;
                file.write_all(book.text().as_bytes())?;
                file.sync_all()?;
                fs::rename(&temp, self.dir.join(FILE))
            };
            write().map_err(|e| at("could not write", e))?;
        }
        drop(lock);
        Ok(out)
    }

    /// The book as it is.
    pub(crate) fn read(&self) -> Result<Book, String> {
        self.with(|book| book.clone())
    }
}

// ── A run's spending ────────────────────────────────────────────────────────

/// One peer's spending in a run: the meter its clients add every byte they
/// read to, put into the book as it grows, and what is left of the day —
/// the book's, and the peer's own figure when it gave one.
pub(crate) struct Spend {
    budget: Budget,
    key: PeerKey,
    meter: Arc<AtomicU64>,
    clock: Clock,
    /// The meter's bytes the book has.
    counted: u64,
    /// What the book left the day at its last read, and the day; None: no
    /// cap.
    left: Option<u64>,
    day: u64,
    allowance: Option<Allowance>,
    /// The key's quota left when the peer said so, and the meter then; for
    /// the day it was read on.
    quota: Option<(u64, u64)>,
}

impl Spend {
    /// The peer's account, its allowance as the peer showed it (None: an
    /// older peer), the book read once.
    pub(crate) fn new(
        budget: Budget,
        key: PeerKey,
        meter: Arc<AtomicU64>,
        allowance: Option<Allowance>,
        clock: Clock,
    ) -> Result<Spend, String> {
        let counted = meter.load(Ordering::Relaxed);
        let quota = allowance.as_ref().and_then(|a| a.remaining_today).map(|r| (r, counted));
        let mut spend =
            Spend { budget, key, meter, clock, counted, left: None, day: 0, allowance, quota };
        spend.sync()?;
        Ok(spend)
    }

    pub(crate) fn key(&self) -> PeerKey {
        self.key
    }

    /// The meter's bytes since the last write put into the book, and the
    /// day's rest read again (another program may have spent some). A new
    /// day forgets the peer's figure, which was of the day before.
    pub(crate) fn sync(&mut self) -> Result<(), String> {
        let now = (self.clock)();
        let read = self.meter.load(Ordering::Relaxed);
        let delta = read.saturating_sub(self.counted);
        let (key, allowance) = (self.key, self.allowance.clone());
        let left = self.budget.with(|b| {
            b.spend(key, delta, now);
            b.left(key, now, allowance.as_ref())
        })?;
        if self.day != 0 && self.day != utc_day(now) {
            self.quota = None;
        }
        (self.counted, self.left, self.day) = (read, left, utc_day(now));
        Ok(())
    }

    /// [`Spend::sync`] once the meter has run [`SYNC_EVERY`] ahead.
    pub(crate) fn tick(&mut self) -> Result<(), String> {
        match self.meter.load(Ordering::Relaxed).saturating_sub(self.counted) >= SYNC_EVERY {
            true => self.sync(),
            false => Ok(()),
        }
    }

    /// What is left of the day now, the meter's bytes since the last sync
    /// counted, as if the last `pending` of them weren't; None: no cap.
    pub(crate) fn left_before(&self, pending: u64) -> Option<u64> {
        let read = self.meter.load(Ordering::Relaxed).saturating_sub(pending);
        let book = self.left.map(|l| l.saturating_sub(read.saturating_sub(self.counted)));
        let quota = self.quota.map(|(rest, at)| rest.saturating_sub(read.saturating_sub(at)));
        match (book, quota) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// What is left of the day now; None: no cap.
    pub(crate) fn left(&self) -> Option<u64> {
        self.left_before(0)
    }

    /// A whole day's budget; None: no cap.
    pub(crate) fn cap(&self) -> Option<u64> {
        let book = self.budget.read().ok();
        let cap = book.as_ref().and_then(|b| b.cap(self.key, self.allowance.as_ref()));
        match book {
            Some(_) => cap,
            None => Some(DEFAULT_MB * MB),
        }
    }

    /// The peer isn't asked before `until`: written to the book, with what
    /// the meter took.
    pub(crate) fn park(&mut self, until: u64) -> Result<(), String> {
        self.sync()?;
        let key = self.key;
        self.budget.with(|b| b.park(key, until))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A folder of its own under the system's temp folder, gone when dropped.
    pub(crate) struct Scratch(pub(crate) PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Scratch {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let dir = std::env::temp_dir().join(format!("mstream-budget-{pid}-{name}-{n}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const KEY: PeerKey = PeerKey { parent_key: 0x00AA_BBCC_DDEE_FF11, peer_id: 3 };
    /// 2026-10-18 13:20 UTC.
    const NOW: u64 = 1_792_281_600 + 13 * 3600 + 1200;

    #[test]
    fn days_and_their_midnights_are_utc() {
        assert_eq!(date(utc_day(NOW)), "2026-10-18");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(utc_day(951_825_600)), "2000-02-29");
        assert_eq!(day_of("2026-10-18"), Some(utc_day(NOW)));
        assert_eq!(day_of("2026-02-30"), None, "no such day");
        assert_eq!(next_midnight(NOW), 1_792_368_000);
        assert_eq!(resume_after(NOW, None, 300), 1_792_368_300, "the next 00:00 UTC, and 5 minutes");
        assert_eq!(resume_after(NOW, Some(38_400), 120), NOW + 38_520, "the peer's own word");
        assert!((60..=600).contains(&jitter()));
        let mb = MB;
        assert_eq!(days_for(0, Some(0), Some(mb)), 0);
        assert_eq!(days_for(mb, None, None), 1, "no cap: today");
        assert_eq!(days_for(mb, Some(mb), Some(mb)), 1);
        assert_eq!(days_for(mb + 1, Some(mb), Some(mb)), 2);
        assert_eq!(days_for(5 * mb, Some(0), Some(2 * mb)), 4, "nothing left today, then three days");
    }

    #[test]
    fn the_book_reads_back_and_keeps_what_it_doesnt_know() {
        let mut book = Book::default();
        book.spend(KEY, 1234, NOW);
        book.set_limit(KEY, Some(700));
        book.park(KEY, NOW + 3600);
        let text = book.text();
        assert!(text.contains("\"0x00AABBCCDDEEFF11/3\""), "{text}");
        assert!(text.contains("\"day\": \"2026-10-18\"") && text.contains("\"usedBytes\": 1234"), "{text}");
        assert!(!text.contains("http"), "no address: {text}");
        assert_eq!(Book::parse(&text), book);
        // A newer member kept, at the top and in an entry; a newer file read
        // as none; broken ones too.
        let mut value: Value = serde_json::from_str(&text).unwrap();
        value["sets"] = serde_json::json!([1]);
        value["peers"]["0x00AABBCCDDEEFF11/3"]["note"] = "kept".into();
        value["peers"]["not a key"] = serde_json::json!({});
        let read = Book::parse(&value.to_string());
        assert_eq!(read.peers.len(), 1);
        let again: Value = serde_json::from_str(&read.text()).unwrap();
        assert_eq!(again["sets"], serde_json::json!([1]));
        assert_eq!(again["peers"]["0x00AABBCCDDEEFF11/3"]["note"], "kept");
        value["version"] = 2.into();
        assert!(Book::parse(&value.to_string()).newer);
        for bad in ["", "[]", r#"{"format":"other","version":1}"#] {
            assert_eq!(Book::parse(bad), Book::default(), "{bad}");
        }
    }

    #[test]
    fn a_day_is_capped_by_the_listener_the_keys_quota_or_the_default() {
        let mut book = Book::default();
        assert_eq!(book.left(KEY, NOW, None), Some(DEFAULT_MB * MB), "1,500 MB a day");
        book.spend(KEY, 500 * MB, NOW);
        assert_eq!(book.left(KEY, NOW, None), Some(1000 * MB));
        // The next day starts at 0; one past its cap leaves nothing.
        assert_eq!(book.used_today(KEY, NOW + DAY), 0);
        book.spend(KEY, 2000 * MB, NOW + DAY);
        assert_eq!(book.left(KEY, NOW + DAY, None), Some(0));
        // The key's quota where the peer shows it: its own figure, or none.
        let shown = Allowance { daily_mb: Some(2048), ..Allowance::default() };
        assert_eq!(book.cap(KEY, Some(&shown)), Some(2048 * MB));
        let none = Allowance { daily_mb: Some(0), ..Allowance::default() };
        assert_eq!(book.cap(KEY, Some(&none)), None, "a key without a quota");
        assert_eq!(book.cap(KEY, Some(&Allowance::default())), Some(DEFAULT_MB * MB));
        // The listener's own comes first.
        book.set_limit(KEY, Some(100));
        assert_eq!(book.cap(KEY, Some(&none)), Some(100 * MB));
        book.set_limit(KEY, None);
        assert_eq!(book.cap(KEY, None), Some(DEFAULT_MB * MB));
        // A wait, until its time.
        book.park(KEY, NOW + 60);
        assert_eq!(book.waiting(KEY, NOW), Some(NOW + 60));
        assert_eq!(book.waiting(KEY, NOW + 60), None);
    }

    #[test]
    fn two_writers_of_one_book_take_turns() {
        let dir = Scratch::new("turns");
        let writers: Vec<_> = (0..2)
            .map(|_| {
                let budget = Budget::at(&dir.0);
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        budget.with(|b| b.spend(KEY, 1, NOW)).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert_eq!(Budget::at(&dir.0).read().unwrap().used_today(KEY, NOW), 100, "not one lost");
        // A newer player's file is left as it is, and nothing is counted in
        // it: no peer is read.
        let newer = format!(r#"{{"format":"{FORMAT}","version":9,"peers":{{}}}}"#);
        fs::write(dir.0.join(FILE), &newer).unwrap();
        let refused = Budget::at(&dir.0).with(|b| b.spend(KEY, 5, NOW));
        assert!(refused.unwrap_err().contains("newer"));
        assert_eq!(fs::read_to_string(dir.0.join(FILE)).unwrap(), newer);
    }

    #[test]
    fn a_runs_spending_follows_its_meter_and_the_peers_own_figure() {
        let dir = Scratch::new("spend");
        let budget = Budget::at(&dir.0);
        budget.with(|b| b.set_limit(KEY, Some(10))).unwrap();
        let meter = Arc::new(AtomicU64::new(0));
        let clock_at = Arc::new(AtomicU64::new(NOW));
        let at = clock_at.clone();
        let clock: Clock = Arc::new(move || at.load(Ordering::Relaxed));
        let mut spend = Spend::new(budget.clone(), KEY, meter.clone(), None, clock.clone()).unwrap();
        assert_eq!(spend.left(), Some(10 * MB));
        meter.fetch_add(3 * MB, Ordering::Relaxed);
        assert_eq!(spend.left(), Some(7 * MB), "the meter counts before the book does");
        assert_eq!(spend.left_before(MB), Some(8 * MB));
        spend.tick().unwrap();
        assert_eq!(budget.read().unwrap().used_today(KEY, NOW), 0, "not yet 4 MiB on");
        meter.fetch_add(MB, Ordering::Relaxed);
        spend.tick().unwrap();
        assert_eq!(budget.read().unwrap().used_today(KEY, NOW), 4 * MB);
        // Another program's spending shows at the next sync.
        budget.with(|b| b.spend(KEY, 5 * MB, NOW)).unwrap();
        spend.sync().unwrap();
        assert_eq!(spend.left(), Some(MB));
        // The peer's own figure, lower than the book's.
        let shown = Allowance { daily_mb: Some(2048), remaining_today: Some(2 * MB), ..Default::default() };
        let other = PeerKey { peer_id: 4, ..KEY };
        let mut theirs = Spend::new(budget.clone(), other, meter.clone(), Some(shown), clock).unwrap();
        assert_eq!(theirs.cap(), Some(2048 * MB));
        assert_eq!(theirs.left(), Some(2 * MB));
        meter.fetch_add(MB / 2, Ordering::Relaxed);
        assert_eq!(theirs.left(), Some(3 * MB / 2));
        // A new day forgets it (the bytes the sync finds go on the new day).
        clock_at.store(NOW + DAY, Ordering::Relaxed);
        theirs.sync().unwrap();
        assert_eq!(theirs.left(), Some(2048 * MB - MB / 2));
        theirs.park(NOW + DAY + 10).unwrap();
        assert_eq!(budget.read().unwrap().waiting(other, NOW + DAY), Some(NOW + DAY + 10));
    }
}
