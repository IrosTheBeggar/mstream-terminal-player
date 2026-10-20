//! A sync's throughput, measured: ignored by default, as it takes a minute
//! and says numbers rather than checking them (`cargo test
//! sync_throughput -- --ignored --nocapture`). The server is the loopback
//! mStream of [`super`], its answers sent at a LAN's pace ([`Pace`]: one
//! stream at the user's measured 16.5 MB/s, three at 18.5 MB/s together);
//! the card is a [`FatSim`] whose every write costs a fixed time per call
//! and its bytes at the card's rate ([`Slow`]: a card in a reader Windows
//! writes through, with no cache, so each call waits for the card). Reads
//! cost nothing: the measured run read nothing from the card, the OS's
//! cache answering. It prints the copying phase's rate and each fetched
//! file's card calls by kind.
//!
//! The defaults, and the variables that change them:
//! - `MSTREAM_BENCH_FILES` (40) files of `MSTREAM_BENCH_MB` (9) MB, ten an
//!   album;
//! - `MSTREAM_BENCH_CALL_MS` (30): a write call's cost on the card, beside
//!   its bytes — the FAT and the directory entry the file system writes
//!   through with each call, and the I/Os a call is cut into;
//! - `MSTREAM_BENCH_CARD_MBS` (15): the card's write rate;
//! - `MSTREAM_BENCH_LINK_MBS` (16.5) a stream, `MSTREAM_BENCH_LINK_ALL_MBS`
//!   (18.5) every stream together.
//!
//! The card's two figures are the ones that bring the code before the
//! large writes and the downloads ahead (0b378c5) to the user's measured
//! 4.5-5 MB/s, writing what `/media` gives it a read at a time (about
//! 300 KB here); a card that takes 8 ms a call and 20 MB/s gives it
//! 8.4 MB/s.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::time::Instant;

use super::*;
use crate::api::transfer::stub::{Pace, routed_at};
use crate::device::card::volume::{CardFile, Item, LockFile, Serial};

const MB: f64 = 1_000_000.0;

fn knob(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// What a card's write costs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Costs {
    /// Each call that writes: a file's bytes, a create, a rename, a sync.
    pub(crate) call: Duration,
    /// Bytes a second.
    pub(crate) rate: f64,
}

/// A card that takes its time: each write call costs [`Costs::call`] and
/// its bytes at [`Costs::rate`], paid in sleeps that make up for the OS's
/// timer as they go. Every call is counted by its kind and the part of the
/// card it touches.
pub(crate) struct Slow<'c> {
    card: &'c FatSim,
    costs: Costs,
    /// Seconds owed, paid once a few milliseconds are.
    owed: Cell<f64>,
    /// Calls and bytes by "op part".
    calls: RefCell<BTreeMap<String, (u64, u64)>>,
}

/// The part of the card a path is in.
fn part(path: &str) -> &'static str {
    if path.contains("/stage/") {
        "stage"
    } else if path.contains("/resume/") {
        "resume"
    } else if path.starts_with("/music") {
        "music"
    } else {
        "mstream"
    }
}

impl<'c> Slow<'c> {
    pub(crate) fn new(card: &'c FatSim, costs: Costs) -> Slow<'c> {
        Slow { card, costs, owed: Cell::new(0.0), calls: RefCell::new(BTreeMap::new()) }
    }

    fn count(&self, op: &str, path: &str, bytes: usize) {
        let mut calls = self.calls.borrow_mut();
        let seen = calls.entry(format!("{op} {}", part(path))).or_default();
        seen.0 += 1;
        seen.1 += bytes as u64;
    }

    /// A write call of `bytes`, counted and paid for.
    fn pay(&self, op: &str, path: &str, bytes: usize) {
        self.count(op, path, bytes);
        let owed = self.owed.get() + self.costs.call.as_secs_f64() + bytes as f64 / self.costs.rate;
        if owed < 0.002 {
            self.owed.set(owed);
            return;
        }
        let t = Instant::now();
        std::thread::sleep(Duration::from_secs_f64(owed));
        self.owed.set(owed - t.elapsed().as_secs_f64());
    }

    pub(crate) fn calls(&self) -> BTreeMap<String, (u64, u64)> {
        self.calls.borrow().clone()
    }
}

struct SlowFile<'a> {
    file: Box<dyn CardFile + 'a>,
    slow: &'a Slow<'a>,
    path: String,
}

impl Read for SlowFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for SlowFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl Write for SlowFile<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.slow.pay("write", &self.path, n);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl CardFile for SlowFile<'_> {
    fn sync(&mut self) -> io::Result<()> {
        self.file.sync()?;
        self.slow.pay("sync", &self.path, 0);
        Ok(())
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.file.set_len(len)?;
        self.slow.pay("truncate", &self.path, 0);
        Ok(())
    }
}

struct SlowLock<'a> {
    lock: Box<dyn LockFile + 'a>,
    slow: &'a Slow<'a>,
}

impl LockFile for SlowLock<'_> {
    fn try_lock(&mut self) -> io::Result<bool> {
        self.lock.try_lock()
    }

    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        self.lock.read_all()
    }

    fn replace(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.lock.replace(bytes)?;
        self.slow.pay("lock", "/.mstream/run.lock", bytes.len());
        Ok(())
    }

    fn alive(&mut self) -> bool {
        self.lock.alive()
    }
}

impl CardFs for Slow<'_> {
    fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
        self.count("list", dir, 0);
        self.card.list(dir)
    }

    fn stat(&self, path: &str) -> io::Result<Option<Item>> {
        self.count("stat", path, 0);
        self.card.stat(path)
    }

    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        let read = self.card.read(path, max)?;
        self.count("read", path, read.as_ref().map_or(0, Vec::len));
        Ok(read)
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let read = self.card.read_at(path, offset, len)?;
        self.count("read", path, read.len());
        Ok(read)
    }
}

impl CardWrite for Slow<'_> {
    fn key(&self) -> String {
        self.card.key()
    }

    fn serial(&self) -> io::Result<Option<Serial>> {
        self.card.serial()
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        self.count("open", path, 0);
        let file = self.card.open(path)?;
        Ok(Box::new(SlowFile { file, slow: self, path: path.to_string() }))
    }

    fn create(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        let file = self.card.create(path)?;
        self.pay("create", path, 0);
        Ok(Box::new(SlowFile { file, slow: self, path: path.to_string() }))
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        self.card.delete(path)?;
        self.pay("delete", path, 0);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.card.rename(from, to)?;
        self.pay("rename", to, 0);
        Ok(())
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        self.card.mkdir(path)?;
        self.pay("mkdir", path, 0);
        Ok(())
    }

    fn rmdir(&self, path: &str) -> io::Result<()> {
        self.card.rmdir(path)?;
        self.pay("rmdir", path, 0);
        Ok(())
    }

    fn lock_file(&self, path: &str) -> io::Result<Box<dyn LockFile + '_>> {
        let lock = self.card.lock_file(path)?;
        Ok(Box::new(SlowLock { lock, slow: self }))
    }
}

/// `files` MP3s of about `size` bytes each, ten an album, each one's bytes
/// its own (its number in a frame's payload, which no reader looks at).
fn library(files: usize, size: usize) -> Library {
    let frames = size / 417;
    let tracks = (0..files)
        .map(|n| {
            let mut bytes = mp3_bytes(frames, true);
            let at = 20 + 417 + 100;
            bytes[at..at + 8].copy_from_slice(&(n as u64).to_le_bytes());
            let album = n / 10;
            Track {
                path: format!("Rock/Artist {album}/Album {album}/{:02}.mp3", n % 10 + 1),
                id: n as i64 + 1,
                bytes,
                seconds: frames as f64 * 1152.0 / 44_100.0,
                art: None,
                full: json!({"sample-rate": 44100, "channels": 2, "bitrate": 128000}),
            }
        })
        .collect();
    let revision = format!("1:{files}:1790000000000:1");
    Library { vpaths: vec!["Rock".into()], tracks, revision, ..Library::default() }
}

#[test]
#[ignore = "a benchmark: run with --ignored --nocapture and read the numbers"]
fn sync_throughput() {
    let files = knob("MSTREAM_BENCH_FILES", 40.0) as usize;
    let size = (knob("MSTREAM_BENCH_MB", 9.0) * MB) as usize;
    let costs = Costs {
        call: Duration::from_secs_f64(knob("MSTREAM_BENCH_CALL_MS", 30.0) / 1000.0),
        rate: knob("MSTREAM_BENCH_CARD_MBS", 15.0) * MB,
    };
    let each = knob("MSTREAM_BENCH_LINK_MBS", 16.5);
    let all = knob("MSTREAM_BENCH_LINK_ALL_MBS", 18.5);

    let fake: Fake = Arc::new(Mutex::new(library(files, size)));
    let served = fake.clone();
    let pace = Pace::new(each * MB, all * MB);
    let base = routed_at(Some(pace), move |asked| super::answer(&served, asked));
    let scratch = Scratch::new("bench");
    let sim = card(&scratch);
    let slow = Slow::new(&sim, costs);
    let client = Arc::new(Client::new(&base).unwrap().with_token(Some("tok".into())));

    let (mut planned, mut last, mut wrote) = (None, None, 0u64);
    let mut said = |e: Event| match e {
        Event::Planned { .. } => planned = Some(Instant::now()),
        Event::Wrote { bytes, .. } => {
            last = Some(Instant::now());
            wrote += bytes;
        }
        _ => {}
    };
    let t0 = Instant::now();
    let plays = Rc::new(MemStore::default());
    let (fed, space) = (no_peers(), space(32 << 30));
    let mut cards = Cards::default();
    let (ask, envs, at) = (rock(), &mut envs(), Reached::Elsewhere);
    let synced = sync(&slow, space, client, at, &ask, &mut cards, plays, &fed, envs, &mut said);
    let whole = t0.elapsed();
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()));
    assert_eq!(outcome.written, files);

    let copying = last.zip(planned).map(|(l, p)| l - p).unwrap_or_default();
    let rate = |bytes: f64, d: Duration| bytes / MB / d.as_secs_f64().max(1e-9);
    println!(
        "sync throughput: {files} files of {:.1} MB; card {:.1} ms a write call + {:.1} MB/s; \
         link {each} MB/s a stream, {all} MB/s in all",
        size as f64 / MB,
        costs.call.as_secs_f64() * 1000.0,
        costs.rate / MB,
    );
    println!(
        "  copying: {:.1} MB in {:.1} s = {:.2} MB/s, {:.2} files/s; the whole sync {:.1} s",
        wrote as f64 / MB,
        copying.as_secs_f64(),
        rate(wrote as f64, copying),
        files as f64 / copying.as_secs_f64().max(1e-9),
        whole.as_secs_f64(),
    );
    println!("  card calls a fetched file (count, KB a call):");
    for (kind, (n, bytes)) in slow.calls() {
        let each = if n > 0 { bytes as f64 / n as f64 / 1000.0 } else { 0.0 };
        println!("    {kind:<16} {:>8.2}  {each:>9.1}", n as f64 / files as f64);
    }
}
