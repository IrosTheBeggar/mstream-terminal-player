//! A FAT card in a host folder, for the engine's tests: names matched as
//! FAT matches them (case-insensitively, by the names layer's match key,
//! whatever the host's own filesystem does), stamps at 2 s from a clock of
//! its own that a PC may list skewed (2.3.4), a rename that refuses to
//! replace another file, and the card's failures on demand: pulled at any
//! write, a rename cut between its two directory writes (both names left,
//! one file's bytes under each: 2.12.1, "Cut renames"), or another card
//! swapped into the reader after any write. run.lock's OS lock is the
//! host's own, so two runs contend for it for real.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use super::super::contract::fat::{fat_from_wall, fat_wall};
use super::super::contract::prim::fnv1a64;
use super::super::names::match_key;
use super::super::volume::{CardFile, CardFs, CardWrite, Item, LockFile, Serial};

/// 1980-01-01 in Unix seconds: FAT's epoch.
const FAT_EPOCH: i64 = 315_532_800;

/// How a cut write fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cut {
    /// The write doesn't happen (a write of bytes: half of them land).
    Fail,
    /// A rename stops between its two directory writes: the new name is
    /// there, the old one too. Any other write fails as [`Cut::Fail`].
    Between,
}

struct State {
    root: PathBuf,
    serial: u32,
    /// Bumped by a pull or a swap: handles opened before it no longer
    /// reach the card.
    epoch: u32,
    /// Writes so far.
    ops: u64,
    cut: Option<(u64, Cut)>,
    pulled: bool,
    /// After write n, this folder is the card in the reader, with this
    /// serial.
    swap: Option<(u64, PathBuf, u32)>,
    /// The wall clock of the next stamp, in seconds since 1980 (W).
    clock: i64,
    /// What this PC's listing adds to every stamp.
    skew: i64,
}

pub(crate) struct FatSim {
    key: String,
    state: RefCell<State>,
    /// The writes a test may count on: (op, what).
    log: RefCell<Vec<String>>,
    quiet: Cell<bool>,
}

fn pulled() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "the card was pulled")
}

impl FatSim {
    /// A card in `dir` (made if it isn't there) with this serial; its clock
    /// starts at 2026-10-07 14:30:42.
    pub(crate) fn new(dir: &Path, serial: u32) -> FatSim {
        std::fs::create_dir_all(dir).expect("a card folder");
        let clock = fat_wall(0x5D47_73D5).expect("a stamp");
        let state = State {
            root: dir.to_path_buf(),
            serial,
            epoch: 0,
            ops: 0,
            cut: None,
            pulled: false,
            swap: None,
            clock,
            skew: 0,
        };
        FatSim {
            key: dir.display().to_string(),
            state: RefCell::new(state),
            log: RefCell::new(Vec::new()),
            quiet: Cell::new(false),
        }
    }

    pub(crate) fn root(&self) -> PathBuf {
        self.state.borrow().root.clone()
    }

    /// The `n`th write from now on fails, as `how` says, and the card is
    /// pulled with it.
    pub(crate) fn cut_at(&self, n: u64, how: Cut) {
        let mut s = self.state.borrow_mut();
        s.cut = Some((s.ops + n, how));
    }

    /// The card back in the reader, uncut: handles from before stay dead.
    pub(crate) fn replug(&self) {
        let mut s = self.state.borrow_mut();
        (s.pulled, s.cut) = (false, None);
    }

    /// After the `n`th write from now on, `dir` is the card in the reader.
    pub(crate) fn swap_after(&self, n: u64, dir: &Path, serial: u32) {
        let mut s = self.state.borrow_mut();
        s.swap = Some((s.ops + n, dir.to_path_buf(), serial));
    }

    /// The PC lists every stamp `seconds` off (a time zone, 2.3.4).
    pub(crate) fn set_skew(&self, seconds: i64) {
        self.state.borrow_mut().skew = seconds;
    }

    /// The writes so far, each as "op path".
    pub(crate) fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }

    fn settle_swap(&self) {
        let mut s = self.state.borrow_mut();
        if let Some((n, dir, serial)) = s.swap.clone()
            && s.ops >= n
        {
            (s.root, s.serial, s.swap) = (dir, serial, None);
            s.epoch += 1;
        }
    }

    fn reading(&self) -> io::Result<()> {
        self.settle_swap();
        if self.state.borrow().pulled { Err(pulled()) } else { Ok(()) }
    }

    /// A write begins: counted, cut when its turn comes, and the clock
    /// moves on 2 s. The cut it is, if it is.
    fn write_op(&self, what: &str, path: &str) -> io::Result<Option<Cut>> {
        self.settle_swap();
        let mut s = self.state.borrow_mut();
        if s.pulled {
            return Err(pulled());
        }
        s.ops += 1;
        s.clock += 2;
        if !self.quiet.get() {
            self.log.borrow_mut().push(format!("{what} {path}"));
        }
        match s.cut {
            Some((n, how)) if n == s.ops => {
                s.pulled = true;
                s.epoch += 1;
                Ok(Some(how))
            }
            _ => Ok(None),
        }
    }

    fn epoch(&self) -> u32 {
        self.state.borrow().epoch
    }

    /// A card path on the host, each name found as FAT finds it: the entry
    /// spelled alike, else the one of its match key. The last name may be
    /// missing (`made`), and is then spelled as given.
    fn host(&self, path: &str, made: bool) -> io::Result<PathBuf> {
        let names: Vec<&str> = path.split('/').filter(|n| !n.is_empty()).collect();
        let mut at = self.state.borrow().root.clone();
        for (k, name) in names.iter().enumerate() {
            if *name == "." || *name == ".." {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a card path"));
            }
            let listed: Vec<String> = std::fs::read_dir(&at)?
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect();
            let key = match_key(name);
            let alike = || listed.iter().find(|n| match_key(n) == key);
            let found = listed.iter().find(|n| n == name).or_else(alike);
            match found {
                Some(found) => at.push(found),
                None if made && k + 1 == names.len() => at.push(name),
                None => return Err(io::ErrorKind::NotFound.into()),
            }
        }
        Ok(at)
    }

    fn item(&self, name: String, meta: &std::fs::Metadata) -> Item {
        let skew = self.state.borrow().skew;
        let since = |t: std::time::SystemTime| t.duration_since(UNIX_EPOCH).ok();
        let utc = meta.modified().ok().and_then(since).map_or(0, |d| d.as_secs() as i64);
        let fat_time = if meta.is_dir() { 0 } else { fat_from_wall(utc - FAT_EPOCH + skew) };
        Item { name, dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, fat_time }
    }

    /// The stamp of a write now, as the card keeps it (2 s).
    fn stamp(&self) -> std::time::SystemTime {
        let w = self.state.borrow().clock & !1;
        UNIX_EPOCH + Duration::from_secs((w + FAT_EPOCH) as u64)
    }

    /// Every file on the card: its card path, size, stamp and the FNV-1a 64
    /// of its bytes, by path. A card in a folder, read as it stands.
    pub(crate) fn image(&self) -> BTreeMap<String, (u64, u32, u64)> {
        self.quiet.set(true);
        let mut out = BTreeMap::new();
        let mut stack = vec![String::new()];
        while let Some(dir) = stack.pop() {
            let mut items = self.list(if dir.is_empty() { "/" } else { &dir }).unwrap_or_default();
            items.sort_by(|a, b| a.name.cmp(&b.name));
            for i in items {
                let path = format!("{dir}/{}", i.name);
                if i.dir {
                    stack.push(path);
                } else {
                    let bytes = self.read(&path, usize::MAX).ok().flatten().unwrap_or_default();
                    out.insert(path, (i.size, i.fat_time, fnv1a64(&bytes)));
                }
            }
        }
        self.quiet.set(false);
        out
    }
}

impl CardFs for FatSim {
    fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
        self.reading()?;
        let mut items = Vec::new();
        for entry in std::fs::read_dir(self.host(dir, false)?)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|_| io::Error::other("not UTF-8"))?;
            items.push(self.item(name, &entry.metadata()?));
        }
        Ok(items)
    }

    fn stat(&self, path: &str) -> io::Result<Option<Item>> {
        self.reading()?;
        let host = match self.host(path, false) {
            Ok(host) => host,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let name = host.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        Ok(Some(self.item(name, &std::fs::metadata(&host)?)))
    }

    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        self.reading()?;
        let host = match self.host(path, false) {
            Ok(host) => host,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if host.is_dir() {
            return Ok(None);
        }
        let bytes = std::fs::read(host)?;
        Ok((bytes.len() <= max).then_some(bytes))
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.reading()?;
        let mut file = File::open(self.host(path, false)?)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(len);
        file.take(len as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// A file of the card open to write: each write a write of the card's, the
/// stamp set as the card sets it when it is closed.
struct SimFile<'a> {
    sim: &'a FatSim,
    file: File,
    path: String,
    epoch: u32,
    stamp: Option<std::time::SystemTime>,
}

impl SimFile<'_> {
    fn live(&self) -> io::Result<()> {
        self.sim.reading()?;
        if self.sim.epoch() != self.epoch { Err(pulled()) } else { Ok(()) }
    }
}

impl Read for SimFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.live()?;
        self.file.read(buf)
    }
}

impl Seek for SimFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.live()?;
        self.file.seek(pos)
    }
}

impl Write for SimFile<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.live()?;
        if let Some(_cut) = self.sim.write_op("write", &self.path)? {
            // Torn: half the bytes land.
            let _ = self.file.write_all(&buf[..buf.len() / 2]);
            return Err(pulled());
        }
        self.file.write_all(buf)?;
        self.stamp = Some(self.sim.stamp());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl CardFile for SimFile<'_> {
    fn sync(&mut self) -> io::Result<()> {
        self.live()?;
        if self.sim.write_op("sync", &self.path)?.is_some() {
            return Err(pulled());
        }
        self.file.sync_all()
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.live()?;
        if self.sim.write_op("truncate", &self.path)?.is_some() {
            return Err(pulled());
        }
        self.file.set_len(len)?;
        self.stamp = Some(self.sim.stamp());
        Ok(())
    }
}

impl Drop for SimFile<'_> {
    fn drop(&mut self) {
        if let Some(stamp) = self.stamp {
            let _ = self.file.set_modified(stamp);
        }
    }
}

/// run.lock on the card: the host's own advisory lock, dead once the card
/// is pulled or swapped.
struct SimLock<'a> {
    sim: &'a FatSim,
    file: File,
    epoch: u32,
}

impl LockFile for SimLock<'_> {
    fn try_lock(&mut self) -> io::Result<bool> {
        match self.file.try_lock() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn replace(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !self.alive() {
            return Err(pulled());
        }
        if self.sim.write_op("lock", "/.mstream/run.lock")?.is_some() {
            return Err(pulled());
        }
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(bytes)?;
        self.file.sync_all()?;
        let _ = self.file.set_modified(self.sim.stamp());
        Ok(())
    }

    fn alive(&mut self) -> bool {
        self.sim.settle_swap();
        let s = self.sim.state.borrow();
        !s.pulled && s.epoch == self.epoch
    }
}

impl CardWrite for FatSim {
    fn key(&self) -> String {
        self.key.clone()
    }

    fn serial(&self) -> io::Result<Option<Serial>> {
        self.reading()?;
        Ok(Some(Serial::Fat(self.state.borrow().serial)))
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        self.reading()?;
        let file = File::options().read(true).write(true).open(self.host(path, false)?)?;
        let path = path.to_string();
        Ok(Box::new(SimFile { sim: self, file, path, epoch: self.epoch(), stamp: None }))
    }

    fn create(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        self.reading()?;
        let host = self.host(path, true)?;
        if self.write_op("create", path)?.is_some() {
            return Err(pulled());
        }
        let file = File::options().read(true).write(true).create(true).truncate(true).open(host)?;
        let stamp = Some(self.stamp());
        Ok(Box::new(SimFile { sim: self, file, path: path.to_string(), epoch: self.epoch(), stamp }))
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        self.reading()?;
        let host = self.host(path, false)?;
        if self.write_op("delete", path)?.is_some() {
            return Err(pulled());
        }
        std::fs::remove_file(host)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.reading()?;
        let a = self.host(from, false)?;
        let b = self.host(to, true)?;
        // One entry to FAT: found as the same host file, or named alike in
        // one folder (a host that keeps case apart).
        let key = |p: &Path| p.file_name().map(|n| match_key(&n.to_string_lossy()));
        let same = a == b || (a.parent() == b.parent() && key(&a) == key(&b));
        if !same && b.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file of that name is there"));
        }
        match self.write_op("rename", &format!("{from} -> {to}"))? {
            Some(Cut::Between) => {
                // The new entry is written, the old one not yet removed: one
                // file's bytes under both names.
                let stamp = std::fs::metadata(&a)?.modified()?;
                std::fs::copy(&a, &b)?;
                let _ = File::options().write(true).open(&b)?.set_modified(stamp);
                Err(pulled())
            }
            Some(Cut::Fail) => Err(pulled()),
            None if same => {
                // Only the case: FAT renames the entry where it stands.
                let tmp = a.with_file_name(format!(".rename-{}", std::process::id()));
                std::fs::rename(&a, &tmp)?;
                std::fs::rename(&tmp, to_host_name(&a, to))
            }
            None => std::fs::rename(&a, &b),
        }
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        self.reading()?;
        let host = self.host(path, true)?;
        if host.exists() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        if self.write_op("mkdir", path)?.is_some() {
            return Err(pulled());
        }
        std::fs::create_dir(host)
    }

    fn rmdir(&self, path: &str) -> io::Result<()> {
        self.reading()?;
        let host = self.host(path, false)?;
        if self.write_op("rmdir", path)?.is_some() {
            return Err(pulled());
        }
        std::fs::remove_dir(host)
    }

    fn lock_file(&self, path: &str) -> io::Result<Box<dyn LockFile + '_>> {
        self.reading()?;
        let host = self.host(path, true)?;
        let file = File::options().read(true).write(true).create(true).truncate(false).open(host)?;
        Ok(Box::new(SimLock { sim: self, file, epoch: self.epoch() }))
    }
}

/// A host path's file name replaced by the last name of a card path.
fn to_host_name(host: &Path, card: &str) -> PathBuf {
    host.with_file_name(card.rsplit('/').next().unwrap_or(card))
}
