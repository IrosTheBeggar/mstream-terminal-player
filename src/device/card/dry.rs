//! A dry run's card: the card as it is, read through, and every write a
//! session makes kept in memory instead — run.lock's note, a rename that
//! step 1 finishes, step 4's clean-up, `/.mstream` made on a card without
//! it — so the engine opens, settles and plans as it would, and nothing
//! reaches the card. What it wrote reads back as written, under FAT's
//! case-insensitive names, for as long as it lives.
//!
//! It holds the real card's key, so a dry run and a real one in this
//! process never hold one card at once; it takes no OS lock, so a program
//! writing the card meanwhile only makes the plan stale.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use super::names::match_key;
use super::volume::{CardFile, CardFs, CardWrite, Item, LockFile, Serial, same_entry};

/// A card path by its names' match keys: one entry to FAT.
fn key(path: &str) -> Vec<String> {
    path.split('/').filter(|n| !n.is_empty()).map(match_key).collect()
}

fn last(path: &str) -> String {
    path.rsplit('/').find(|n| !n.is_empty()).unwrap_or_default().to_string()
}

fn missing() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "no such entry")
}

/// A file written to the dry card, as a listing shows it: no stamp.
fn file_item(name: &str, bytes: &[u8]) -> Item {
    Item { name: name.to_string(), dir: false, size: bytes.len() as u64, fat_time: 0 }
}

#[derive(Default)]
struct Over {
    /// Files written, by key: their name as written, and their bytes.
    files: BTreeMap<Vec<String>, (String, Vec<u8>)>,
    /// Folders made, by key: their name.
    dirs: BTreeMap<Vec<String>, String>,
    /// The card's own entries deleted or renamed away, by key.
    gone: HashSet<Vec<String>>,
}

impl Over {
    /// The card's entry at `k` is gone: it, or a folder above it.
    fn hides(&self, k: &[String]) -> bool {
        (1..=k.len()).any(|n| self.gone.contains(&k[..n]))
    }
}

/// See the module's note.
pub(crate) struct DryCard<'c> {
    real: &'c dyn CardWrite,
    over: RefCell<Over>,
}

impl<'c> DryCard<'c> {
    pub(crate) fn new(real: &'c dyn CardWrite) -> DryCard<'c> {
        DryCard { real, over: RefCell::new(Over::default()) }
    }

    /// The writes it kept, for a test: files and folders, by key.
    #[cfg(test)]
    fn kept(&self) -> usize {
        let over = self.over.borrow();
        over.files.len() + over.dirs.len() + over.gone.len()
    }

    fn real_stat(&self, path: &str) -> io::Result<Option<Item>> {
        if self.over.borrow().hides(&key(path)) {
            return Ok(None);
        }
        self.real.stat(path)
    }
}

impl CardFs for DryCard<'_> {
    fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
        let k = key(dir);
        let over = self.over.borrow();
        let made = over.dirs.contains_key(&k);
        let mut items = match over.hides(&k) {
            true if !made => return Err(missing()),
            true => Vec::new(),
            false => match self.real.list(dir) {
                Ok(items) => items,
                Err(e) if e.kind() == io::ErrorKind::NotFound && made => Vec::new(),
                Err(e) => return Err(e),
            },
        };
        let child = |name: &str| {
            let mut c = k.clone();
            c.push(match_key(name));
            c
        };
        items.retain(|i| {
            let c = child(&i.name);
            !over.hides(&c) && !over.files.contains_key(&c) && !over.dirs.contains_key(&c)
        });
        let below = |c: &&Vec<String>| c.len() == k.len() + 1 && c[..k.len()] == k[..];
        for (name, bytes) in over.files.iter().filter(|(c, _)| below(c)).map(|(_, v)| v) {
            items.push(file_item(name, bytes));
        }
        for name in over.dirs.iter().filter(|(c, _)| below(c)).map(|(_, v)| v) {
            items.push(Item { name: name.clone(), dir: true, size: 0, fat_time: 0 });
        }
        Ok(items)
    }

    fn stat(&self, path: &str) -> io::Result<Option<Item>> {
        let k = key(path);
        {
            let over = self.over.borrow();
            if let Some((name, bytes)) = over.files.get(&k) {
                return Ok(Some(file_item(name, bytes)));
            }
            if let Some(name) = over.dirs.get(&k) {
                return Ok(Some(Item { name: name.clone(), dir: true, size: 0, fat_time: 0 }));
            }
        }
        self.real_stat(path)
    }

    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        let k = key(path);
        let over = self.over.borrow();
        if let Some((_, bytes)) = over.files.get(&k) {
            return Ok((bytes.len() <= max).then(|| bytes.clone()));
        }
        if over.hides(&k) {
            return Ok(None);
        }
        drop(over);
        self.real.read(path, max)
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let k = key(path);
        let over = self.over.borrow();
        if let Some((_, bytes)) = over.files.get(&k) {
            let start = (offset as usize).min(bytes.len());
            return Ok(bytes[start..(start + len).min(bytes.len())].to_vec());
        }
        if over.hides(&k) {
            return Err(missing());
        }
        drop(over);
        self.real.read_at(path, offset, len)
    }
}

/// A file of the dry card open to write: its bytes kept when it is synced
/// or closed.
struct DryFile<'d> {
    over: &'d RefCell<Over>,
    key: Vec<String>,
    name: String,
    bytes: Cursor<Vec<u8>>,
}

impl DryFile<'_> {
    fn keep(&mut self) {
        let bytes = self.bytes.get_ref().clone();
        self.over.borrow_mut().files.insert(self.key.clone(), (self.name.clone(), bytes));
    }
}

impl Read for DryFile<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.bytes.read(buf)
    }
}

impl Write for DryFile<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for DryFile<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.bytes.seek(pos)
    }
}

impl CardFile for DryFile<'_> {
    fn sync(&mut self) -> io::Result<()> {
        self.keep();
        Ok(())
    }

    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.bytes.get_mut().resize(len as usize, 0);
        Ok(())
    }
}

impl Drop for DryFile<'_> {
    fn drop(&mut self) {
        self.keep();
    }
}

/// run.lock on a dry card: always taken, its note kept in memory.
struct DryLock(Vec<u8>);

impl LockFile for DryLock {
    fn try_lock(&mut self) -> io::Result<bool> {
        Ok(true)
    }

    fn read_all(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.0.clone())
    }

    fn replace(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0 = bytes.to_vec();
        Ok(())
    }

    fn alive(&mut self) -> bool {
        true
    }
}

impl CardWrite for DryCard<'_> {
    fn key(&self) -> String {
        self.real.key()
    }

    fn serial(&self) -> io::Result<Option<Serial>> {
        self.real.serial()
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        let bytes = Cursor::new(self.read(path, usize::MAX)?.ok_or_else(missing)?);
        Ok(Box::new(DryFile { over: &self.over, key: key(path), name: last(path), bytes }))
    }

    fn create(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        let k = key(path);
        let name = self.stat(path)?.map_or_else(|| last(path), |i| i.name);
        self.over.borrow_mut().gone.remove(&k);
        let file = DryFile { over: &self.over, key: k, name, bytes: Cursor::new(Vec::new()) };
        Ok(Box::new(file))
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        let k = key(path);
        let real = self.real_stat(path)?.is_some();
        let mut over = self.over.borrow_mut();
        let ours = over.files.remove(&k).is_some();
        if real {
            over.gone.insert(k);
        }
        if ours || real { Ok(()) } else { Err(missing()) }
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let bytes = self.read(from, usize::MAX)?.ok_or_else(missing)?;
        if !same_entry(from, to) && self.stat(to)?.is_some() {
            let taken = "a file of that name is there";
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, taken));
        }
        self.delete(from)?;
        let mut over = self.over.borrow_mut();
        over.gone.remove(&key(to));
        over.files.insert(key(to), (last(to), bytes));
        Ok(())
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        if self.stat(path)?.is_some() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "it is there"));
        }
        let mut over = self.over.borrow_mut();
        over.gone.remove(&key(path));
        over.dirs.insert(key(path), last(path));
        Ok(())
    }

    fn rmdir(&self, path: &str) -> io::Result<()> {
        let k = key(path);
        let real = self.real_stat(path)?.is_some();
        let mut over = self.over.borrow_mut();
        let ours = over.dirs.remove(&k).is_some();
        if real {
            over.gone.insert(k);
        }
        if ours || real { Ok(()) } else { Err(missing()) }
    }

    fn lock_file(&self, path: &str) -> io::Result<Box<dyn LockFile + '_>> {
        let note = self.read(path, 64 * 1024)?.unwrap_or_default();
        Ok(Box::new(DryLock(note)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::card::engine::fatsim::FatSim;

    #[test]
    fn a_dry_card_reads_what_it_wrote_and_writes_nothing_to_the_card() {
        let dir = std::env::temp_dir().join(format!("mstream-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sim = FatSim::new(&dir, 0x1A2B_3C4D);
        std::fs::create_dir_all(dir.join("music/Artist")).unwrap();
        std::fs::write(dir.join("music/Artist/01.mp3"), b"one").unwrap();
        std::fs::write(dir.join("music/Artist/02.mp3"), b"two").unwrap();
        let before = sim.image();
        let dry = DryCard::new(&sim);

        dry.mkdir("/.mstream").unwrap();
        let mut f = dry.create("/.mstream/state.new").unwrap();
        f.write_all(b"{}").unwrap();
        drop(f);
        dry.rename("/.mstream/state.new", "/.mstream/STATE.JSON").unwrap();
        assert_eq!(dry.read("/.mstream/state.json", 100).unwrap().as_deref(), Some(b"{}".as_slice()));
        assert!(dry.stat("/.mstream/state.new").unwrap().is_none());
        dry.delete("/music/artist/01.MP3").unwrap();
        assert!(dry.stat("/music/Artist/01.mp3").unwrap().is_none());
        let names: Vec<String> = dry.list("/music/Artist").unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["02.mp3"]);
        assert!(dry.rename("/music/Artist/02.mp3", "/.mstream/state.json").is_err(), "never over a file");
        dry.rmdir("/music/Artist").unwrap();
        assert!(dry.list("/music/Artist").is_err());
        assert!(dry.delete("/music/none.mp3").is_err());
        let mut lock = dry.lock_file("/.mstream/run.lock").unwrap();
        assert!(lock.try_lock().unwrap());
        lock.replace(b"note").unwrap();
        assert!(dry.kept() > 0);

        assert_eq!(sim.image(), before, "nothing reached the card");
        assert!(sim.log().is_empty(), "{:?}", sim.log());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
