//! The server's library folders read from this computer's disk, when the
//! terminal runs beside the server (the transfer plan's 2.4, part A9): the
//! files a card takes byte for byte are copied at disk speed, with no HTTP
//! in between. Everything else goes to the fetcher it wraps, `/media` and
//! `/transcode` over loopback: a conversion is always the server's to make.
//!
//! **When** ([`folders`]): all of these hold, or every file goes over the
//! network and the [`Mode`] says why:
//! - the session is an admin's: `GET /api/` sent its admin block (an admin,
//!   while the admin API isn't locked);
//! - the server is on this computer ([`Reached`]): a loopback address
//!   reached directly, never through a tunnel's bridge or as a peer through
//!   its parent, or the installer's `--bundled-server`;
//! - its paths are of this OS's kind (the admin block's `platform`);
//! - `GET api/v1/admin/directories` answers: each library's folder as the
//!   server sees it.
//!
//! **Checked, not trusted** ([`Local::new`]): `vpath/rel` is the library's
//! folder joined with `rel`, but the folder the server names may not be
//! this computer's (a server in Docker names its container's paths). So
//! each library's folder is first held to a sample of the manifest's
//! entries, their sizes and `modified` times as they are on disk, and one
//! that fails is never read. Each file is checked again just before it is
//! read: one that differs from the manifest (changed since the server's
//! scan) comes over the network instead. The ledger's fields are the
//! manifest's either way.
//!
//! **Read only.** A file there is opened for reading and nothing else: no
//! file or folder of the server's is ever created, written, renamed or
//! deleted.
//!
//! What a folder that can be listed adds: 2.14.2's "largest other `.jpg`"
//! for a cover, which HTTP can only guess at by name ([`Local::folder_image`]),
//! and U6's fingerprints for free, two reads of the disk rather than two
//! ranged requests ([`Fetcher::read_range`]).
//!
//! The words a listener reads, the Details log's line naming the mode, are
//! the UX contract's, still to come: until then the mode is codes
//! ([`Mode::code`], as the volumes' refusals are).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::super::contract::MAX_FILE;
use super::super::contract::mptg::Ledger;
use super::super::engine::{CARD_WRITE, Failed, Fetched, Fetcher, Have, Part, WantedFile};
use crate::api::ApiError;
use crate::api::transfer::ManifestEntry;
use crate::api::types::{AdminDirEntry, LayeredAdmin};

/// How many of a library's manifest entries its folder is held to, spread
/// across the library.
const SAMPLE: usize = 8;

/// What a read of a range takes at most at first.
const CHUNK: usize = 256 * 1024;

/// The rank of a cover image with no better name (the device's 3).
const OTHER: u8 = 3;

// ── When ────────────────────────────────────────────────────────────────────

/// How the session reaches its server, as far as reading its folders goes.
/// The caller says it: the GUI's `same_machine` (src/gui/admin.rs) for a
/// loopback server reached directly, `--bundled-server`'s address for the
/// installer's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reached {
    /// A loopback address, directly: not a tunnel's bridge (which listens
    /// on loopback for a server elsewhere), not a peer through its parent.
    Loopback,
    /// The server the installer put beside this player, directly.
    Bundled,
    /// Another computer, a tunnel, a peer, or not known.
    Elsewhere,
}

/// Why every file goes over the network. What it carries is for the debug
/// listing and the tests ([`Mode`]).
#[derive(Debug)]
pub(crate) enum Why {
    /// `/api/` sent no admin block: not an admin, or the admin API locked.
    NotAdmin,
    /// The server isn't on this computer, as far as the session can tell.
    Elsewhere,
    /// The server's paths are another OS's kind: its `platform`, as Node
    /// names it.
    OtherSystem(String),
    /// `admin/directories` didn't answer.
    NoAnswer(ApiError),
    /// No library of the manifest has a folder here that holds its files:
    /// each, and why not.
    NoFolder(Vec<Library>),
}

impl Why {
    /// The reason as one stable word, for the debug listing and the tests.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Why::NotAdmin => "not-admin",
            Why::Elsewhere => "elsewhere",
            Why::OtherSystem(_) => "other-system",
            Why::NoAnswer(_) => "no-answer",
            Why::NoFolder(_) => "no-folder",
        }
    }
}

/// The library folders the server names (vpath → its folder as the server
/// sees it), when the session may read them: `admin` is `/api/`'s admin
/// block, and `ask` asks `admin/directories`, only once everything else
/// holds (a non-admin is refused it anyway).
pub(crate) fn folders(
    admin: Option<&LayeredAdmin>,
    reached: Reached,
    ask: impl FnOnce() -> Result<BTreeMap<String, AdminDirEntry>, ApiError>,
) -> Result<BTreeMap<String, String>, Why> {
    let Some(admin) = admin else { return Err(Why::NotAdmin) };
    if reached == Reached::Elsewhere {
        return Err(Why::Elsewhere);
    }
    // A Windows server's `C:\Music` is no path on a Unix one, nor `/srv`
    // on Windows: a WSL terminal reaches a Windows server at localhost.
    if let Some(platform) = admin.platform.as_deref().filter(|p| (*p == "win32") != cfg!(windows)) {
        return Err(Why::OtherSystem(platform.to_string()));
    }
    let answer = ask().map_err(Why::NoAnswer)?;
    Ok(answer.into_iter().map(|(vpath, dir)| (vpath, dir.root)).collect())
}

// ── The mode ────────────────────────────────────────────────────────────────

/// Where a run's files come from: what the Details log's one line names.
/// Its paths (a [`Folder`]'s, and [`Why`]'s platform and error) are for the
/// debug listing and the tests: a server's folder names this computer's
/// user. The line a listener reads, or a log they may share, is made of the
/// `code()`s and the counts, never a `{:?}` of it.
#[derive(Debug)]
pub(crate) enum Mode {
    /// Every file over the network.
    Network(Why),
    /// Each library of the manifest: some read from their folders here, the
    /// others over the network, and why.
    Disk(Vec<Library>),
}

impl Mode {
    /// The mode as one stable word: `disk`, or why it is the network.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Mode::Network(why) => why.code(),
            Mode::Disk(_) => "disk",
        }
    }
}

/// A library of the manifest, and where its files come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Library {
    pub(crate) vpath: String,
    pub(crate) folder: Folder,
}

/// What a library's folder came to. Its paths are this computer's, kept out
/// of what a listener reads ([`Mode`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Folder {
    /// Read from here: `matched` of the `sampled` entries were the
    /// manifest's.
    Read { root: PathBuf, matched: usize, sampled: usize },
    /// `admin/directories` names no folder for it.
    Unlisted,
    /// A folder that is no full path here (another OS's, or a relative one).
    Foreign(String),
    /// No such folder here.
    Missing(PathBuf),
    /// No entry of the manifest to hold it to: none has a size and a time.
    Unchecked(PathBuf),
    /// Too few of the sampled entries are the manifest's: a container's
    /// path, or another tree at the same place.
    Mismatch { root: PathBuf, matched: usize, sampled: usize },
}

impl Folder {
    /// The outcome as one stable word, for the debug listing and the tests.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Folder::Read { .. } => "read",
            Folder::Unlisted => "unlisted",
            Folder::Foreign(_) => "foreign",
            Folder::Missing(_) => "missing",
            Folder::Unchecked(_) => "unchecked",
            Folder::Mismatch { .. } => "mismatch",
        }
    }
}

/// Each library of the manifest, its folder held to a sample of its
/// entries: read when at least three in four are on disk as the manifest
/// says. A file changed since the server's scan fails its check without the
/// folder being wrong (each file is checked again anyway); a folder that is
/// another computer's fails all of them.
fn check(folders: &BTreeMap<String, String>, manifest: &[ManifestEntry]) -> Vec<Library> {
    let mut libraries: BTreeMap<&str, Vec<(&str, u64, u64)>> = BTreeMap::new();
    for entry in manifest {
        let Some((vpath, rel)) = entry.filepath.split_once('/') else { continue };
        let known = libraries.entry(vpath).or_default();
        let modified = entry.modified.and_then(|ms| u64::try_from(ms).ok());
        if let (Some(size), Some(modified)) = (entry.file_size, modified) {
            known.push((rel, size, modified));
        }
    }
    let judge = |root: Option<&String>, known: &[(&str, u64, u64)]| {
        let Some(root) = root else { return Folder::Unlisted };
        let path = PathBuf::from(root);
        if !path.is_absolute() {
            return Folder::Foreign(root.clone());
        }
        if !fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            return Folder::Missing(path);
        }
        let (n, sampled) = (known.len(), known.len().min(SAMPLE));
        if sampled == 0 {
            return Folder::Unchecked(path);
        }
        let same_here = |&(rel, size, modified): &(&str, u64, u64)| {
            join(&path, rel).is_some_and(|file| same(&file, size, modified))
        };
        let matched = (0..sampled).filter(|i| same_here(&known[i * n / sampled])).count();
        if matched * 4 >= sampled * 3 {
            Folder::Read { root: path, matched, sampled }
        } else {
            Folder::Mismatch { root: path, matched, sampled }
        }
    };
    let library = |(vpath, known): (&str, Vec<_>)| {
        Library { vpath: vpath.to_string(), folder: judge(folders.get(vpath), &known) }
    };
    libraries.into_iter().map(library).collect()
}

/// `rel`, a manifest path past its vpath (always `/`-separated), under
/// `root`; None when it could leave it: every part a plain name, never
/// empty, `.` or `..`, nor on Windows one with a `\` or a `:` (a drive, a
/// stream).
fn join(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in rel.split('/') {
        let odd = cfg!(windows) && part.contains(['\\', ':']);
        if part.is_empty() || part == "." || part == ".." || part.contains('\0') || odd {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

/// The file at `path` is the one the manifest describes: a file of its
/// size, modified when it says. mStream's `modified` is the mtime in whole
/// ms (`Math.trunc(stat.mtimeMs)`, its scanner); 1 ms either way takes in
/// the float Node's `mtimeMs` is, which can round up past a whole ms.
fn same(path: &Path, size: u64, modified: u64) -> bool {
    let Ok(meta) = fs::metadata(path) else { return false };
    let since = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
    let near = since.is_some_and(|d| d.as_millis().abs_diff(modified as u128) <= 1);
    meta.is_file() && meta.len() == size && near
}

// ── The source ──────────────────────────────────────────────────────────────

/// How a run's files came, so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tally {
    /// Files copied from this computer's disk.
    pub(crate) disk: usize,
    /// Files of a library read from here that went over the network: not on
    /// disk as the manifest says (changed since the scan), or unreadable.
    pub(crate) network: usize,
}

/// A folder's cover image, read from disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FolderImage {
    /// Its name in the folder.
    pub(crate) name: String,
    /// Its bytes: what `mpth::from_image` decodes, and their count MPTH's
    /// `sourceBytes`.
    pub(crate) bytes: Vec<u8>,
}

/// The files of one server, read from its folders where they can be and
/// from `network` where they can't.
pub(crate) struct Local<F> {
    /// The server whose folders these are: an index into the wanted card's
    /// origins. Another origin's files are never read from here.
    origin: usize,
    /// The libraries read from here: each folder, by vpath.
    roots: BTreeMap<String, PathBuf>,
    mode: Mode,
    tally: Tally,
    network: F,
}

impl<F: Fetcher> Local<F> {
    /// The source of a run's files of `origin`: `folders` is what
    /// [`folders`] said, each library's folder held to `manifest` (the
    /// server's sync manifest, as read for this run) before any file of it
    /// is read. With no folder that holds its files, every file comes from
    /// `network`, and the mode says why.
    pub(crate) fn new(
        folders: Result<BTreeMap<String, String>, Why>,
        origin: usize,
        manifest: &[ManifestEntry],
        network: F,
    ) -> Local<F> {
        let libraries = match folders {
            Ok(folders) => check(&folders, manifest),
            Err(why) => return Local::over_network(why, origin, network),
        };
        let read = |l: &Library| match &l.folder {
            Folder::Read { root, .. } => Some((l.vpath.clone(), root.clone())),
            _ => None,
        };
        let roots: BTreeMap<String, PathBuf> = libraries.iter().filter_map(read).collect();
        if roots.is_empty() {
            return Local::over_network(Why::NoFolder(libraries), origin, network);
        }
        Local { origin, roots, mode: Mode::Disk(libraries), tally: Tally::default(), network }
    }

    fn over_network(why: Why, origin: usize, network: F) -> Local<F> {
        let (roots, tally) = (BTreeMap::new(), Tally::default());
        Local { origin, roots, mode: Mode::Network(why), tally, network }
    }

    /// Where the files come from, for the Details log's line.
    pub(crate) fn mode(&self) -> &Mode {
        &self.mode
    }

    pub(crate) fn tally(&self) -> Tally {
        self.tally
    }

    /// The fetcher everything else goes to.
    pub(crate) fn network(&mut self) -> &mut F {
        &mut self.network
    }

    /// The covers of the libraries read from here, apart from the source:
    /// what a run's cover step reads while the engine holds the source.
    pub(crate) fn images(&self) -> Images {
        Images(self.roots.clone())
    }

    /// Where `file` would be on disk: the server's bytes (a conversion is
    /// the server's to make), of the server whose folders these are, in a
    /// library read from here. Whether it is the manifest's file is
    /// [`same`]'s to say.
    fn path_of(&self, file: &WantedFile) -> Option<PathBuf> {
        if file.origin != self.origin || !file.original() {
            return None;
        }
        let (vpath, rel) = file.ledger.server_path.split_once('/')?;
        join(self.roots.get(vpath)?, rel)
    }

    /// `file` is one this computer's disk may give: the server's bytes, in a
    /// library read from here. Its fetch still checks it is the manifest's
    /// file, and takes it over the network when it isn't.
    pub(crate) fn reads(&self, file: &WantedFile) -> bool {
        self.path_of(file).is_some()
    }

    /// The image a server folder's cover is made from (2.14.2, item 1), as
    /// the device elects one in a folder: `cover`, `folder`, then `front`,
    /// else the largest other `.jpg` ([`image_rank`]). `folder` is a folder
    /// of this server's (`vpath/rel`: a ledger path's folder, or a library's
    /// own). None: not a library read from here, no such image, or one past
    /// [`MAX_FILE`]; the caller then probes over HTTP a folder that isn't
    /// read from here ([`Images::lists`]), or takes mStream's album art.
    pub(crate) fn folder_image(&self, folder: &str) -> Option<FolderImage> {
        image_in(&self.roots, folder)
    }
}

/// The covers of the libraries a [`Local`] reads from this computer's disk
/// ([`Local::images`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Images(BTreeMap<String, PathBuf>);

impl Images {
    /// [`Local::folder_image`].
    pub(crate) fn folder_image(&self, folder: &str) -> Option<FolderImage> {
        image_in(&self.0, folder)
    }

    /// `folder` (`vpath/rel`) is in a library read from here: its listing
    /// says what images it has, and nothing need be probed for.
    pub(crate) fn lists(&self, folder: &str) -> bool {
        self.0.contains_key(folder.split_once('/').map_or(folder, |(vpath, _)| vpath))
    }
}

/// [`Local::folder_image`], in the libraries of `roots`.
fn image_in(roots: &BTreeMap<String, PathBuf>, folder: &str) -> Option<FolderImage> {
    let (vpath, rel) = folder.split_once('/').unwrap_or((folder, ""));
    let root = roots.get(vpath)?;
    let dir = if rel.is_empty() { root.clone() } else { join(root, rel)? };
    let mut images = Vec::new();
    for entry in fs::read_dir(&dir).ok()?.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        // A hidden name is never a cover: the device's walk skips it
        // (macOS's `._cover.jpg` beside `cover.jpg`).
        let Some(rank) = image_rank(&name).filter(|_| !name.starts_with('.')) else { continue };
        let Ok(meta) = fs::metadata(entry.path()) else { continue };
        if meta.is_file() {
            images.push((rank, name, meta.len()));
        }
    }
    let (_, name, size) = elect(images)?;
    if size > MAX_FILE as u64 {
        return None;
    }
    // The listing's size may be stale by now (a cover replaced since):
    // never more than MAX_FILE of it read into memory.
    let mut bytes = Vec::new();
    let file = File::open(dir.join(&name)).ok()?;
    file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= MAX_FILE).then_some(FolderImage { name, bytes })
}

impl<F: Fetcher> Fetcher for Local<F> {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
        let Some(path) = self.path_of(file) else { return self.network.fetch(file, into) };
        let l = &file.ledger;
        if !same(&path, l.server_size, l.server_modified) {
            self.tally.network += 1;
            return self.network.fetch(file, into);
        }
        match copy(&path, l, into) {
            Ok(()) => {
                self.tally.disk += 1;
                Ok(Fetched::default())
            }
            Err(Cut::Unread) => {
                self.tally.network += 1;
                self.network.fetch(file, into)
            }
            Err(Cut::Partway(why)) => Err(Failed { why, stop: false }),
        }
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        let l = &file.ledger;
        let path = self.path_of(file).filter(|p| same(p, l.server_size, l.server_modified));
        match path.and_then(|p| read_at(&p, start, len).ok()) {
            Some(bytes) => Ok(bytes),
            None => self.network.read_range(file, start, len),
        }
    }

    /// The network's say (a peer's file is never read from here).
    fn resumable(&self, file: &WantedFile) -> bool {
        self.network.resumable(file)
    }

    /// A copy from disk is whole: the partial starts over, and the file is
    /// copied as [`Fetcher::fetch`] copies it. Anything else is the
    /// network's to take up.
    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        let l = &file.ledger;
        match self.path_of(file) {
            Some(path) if same(&path, l.server_size, l.server_modified) => {
                let started = into.answered(0, None);
                started.map_err(|e| Failed { why: e.to_string(), stop: false })?;
                self.fetch(file, into)
            }
            Some(_) => {
                self.tally.network += 1;
                self.network.fetch_part(file, have, into)
            }
            None => self.network.fetch_part(file, have, into),
        }
    }

    /// The network's say: a peer's day is never read from here.
    fn waits(&self, file: &WantedFile) -> Option<String> {
        self.network.waits(file)
    }
}

/// Why a copy from disk ended early.
#[derive(Debug)]
enum Cut {
    /// Before a byte of it was written: the network can still have it.
    Unread,
    /// Partway, or the card refused a write: the file is skipped this run.
    Partway(String),
}

/// The file at `path` into `into`, read only, and still the manifest's
/// once read (a file rewritten as it was read is never taken).
fn copy(path: &Path, l: &Ledger, into: &mut dyn Write) -> Result<(), Cut> {
    // A byte past the manifest's size is enough to know it grew.
    let mut f = File::open(path).map_err(|_| Cut::Unread)?.take(l.server_size.saturating_add(1));
    let copied = pour(&mut f, into)?;
    if copied == l.server_size && same(path, l.server_size, l.server_modified) {
        return Ok(());
    }
    // Emptied since its check: nothing reached the card.
    if copied == 0 {
        return Err(Cut::Unread);
    }
    Err(Cut::Partway("it changed on disk while it was read".into()))
}

/// All of `from` into `into`, [`CARD_WRITE`] a read, as the card's stage
/// file takes it a write: how many bytes.
fn pour(from: &mut dyn Read, into: &mut dyn Write) -> Result<u64, Cut> {
    let mut buf = vec![0u8; CARD_WRITE];
    let mut copied = 0u64;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(copied),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) if copied == 0 => return Err(Cut::Unread),
            Err(e) => return Err(Cut::Partway(format!("its read from disk failed: {e}"))),
        };
        into.write_all(&buf[..n]).map_err(|e| Cut::Partway(e.to_string()))?;
        copied += n as u64;
    }
}

/// `len` bytes from `start` of the file at `path`, fewer at its end.
fn read_at(path: &Path, start: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(start))?;
    let mut out = Vec::with_capacity(len.min(CHUNK));
    f.take(len as u64).read_to_end(&mut out)?;
    Ok(out)
}

// ── Covers ──────────────────────────────────────────────────────────────────

/// How good a cover a file's name makes, 0 best, as the device ranks one
/// (`LibraryIndex::imageRank`): `cover`, `folder`, `front`, then any other
/// name, of a `.jpg` or `.jpeg`, in any case. None: no cover image (PNG
/// included: the device never takes one from a folder).
pub(crate) fn image_rank(name: &str) -> Option<u8> {
    let (stem, ext) = name.rsplit_once('.')?;
    let jpeg = ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg");
    if stem.is_empty() || !jpeg {
        return None;
    }
    let named = ["cover", "folder", "front"].iter().position(|n| stem.eq_ignore_ascii_case(n));
    Some(named.map_or(OTHER, |at| at as u8))
}

/// A folder's cover, of its images (rank, name, size): the best rank, and
/// of an equal one the first by name; of the other `.jpg`s the largest
/// (`Thumbs.cpp`'s `pickLargest`), then the first by name.
fn elect(images: Vec<(u8, String, u64)>) -> Option<(u8, String, u64)> {
    let key = |(rank, name, size): &(u8, String, u64)| {
        let largest = if *rank == OTHER { *size } else { 0 };
        (*rank, std::cmp::Reverse(largest), name.clone())
    };
    images.into_iter().min_by_key(key)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::api::Client;
    use crate::api::transfer::stub::{canned, json};
    use crate::device::card::contract::mptg::CONTAINER_OPUS;
    use crate::device::card::contract::prim::{qfp, qfp_of, qfp_ranges};
    use crate::device::card::names::Bytes;

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A folder of its own under the system's temp folder, gone when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let dir = std::env::temp_dir().join(format!("mstream-local-{pid}-{name}-{n}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("a scratch folder");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// When the tests' files were last changed, epoch ms, as the server's
    /// scan read it.
    const SCANNED: u64 = 1_790_000_000_123;

    /// A file of a library folder, with its mtime.
    fn put(root: &Path, rel: &str, bytes: &[u8], modified: u64) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let at = UNIX_EPOCH + Duration::from_millis(modified);
        File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
    }

    /// The bytes of a made-up track: long enough for a fingerprint's tail.
    fn track(n: u8) -> Vec<u8> {
        (0..10_000u32).map(|i| (i as u8).wrapping_mul(n).wrapping_add(n)).collect()
    }

    /// A library of three tracks in `root`, and the manifest's entries for
    /// it under `vpath`, as the server sends them.
    fn library(root: &Path, vpath: &str) -> Vec<ManifestEntry> {
        let rels = ["Artist A/Album B/01.flac", "Artist A/Album B/02.flac", "Artist C/03.mp3"];
        let files: Vec<(String, Vec<u8>)> =
            rels.iter().zip(1u8..).map(|(rel, n)| (rel.to_string(), track(n))).collect();
        for (rel, bytes) in &files {
            put(root, rel, bytes, SCANNED);
        }
        let entry = |(rel, bytes): &(String, Vec<u8>)| (format!("{vpath}/{rel}"), bytes.len());
        entries(&files.iter().map(entry).collect::<Vec<_>>())
    }

    /// Manifest entries of these paths and sizes, all scanned at [`SCANNED`].
    fn entries(paths: &[(String, usize)]) -> Vec<ManifestEntry> {
        let entry = |(id, (path, size)): (usize, &(String, usize))| {
            let row = serde_json::json!({
                "filepath": path, "id": id + 1, "file-size": size, "modified": SCANNED,
                "hash-v": 2,
            });
            serde_json::from_value(row).expect("an entry as the server sends one")
        };
        paths.iter().enumerate().map(entry).collect()
    }

    /// A file of the selection, as the sync makes one of an entry (only
    /// what the source reads).
    fn wanted(entry: &ManifestEntry) -> WantedFile {
        let ledger = Ledger {
            server_path: entry.filepath.clone(),
            server_size: entry.file_size.unwrap(),
            server_modified: entry.modified.unwrap() as u64,
            hash_v: 2,
            ..Ledger::default()
        };
        WantedFile { size: ledger.server_size, ledger, ..WantedFile::default() }
    }

    fn here(folders: &[(&str, PathBuf)]) -> Result<BTreeMap<String, String>, Why> {
        Ok(folders.iter().map(|(v, p)| (v.to_string(), p.to_string_lossy().into_owned())).collect())
    }

    /// The source of a server whose one library, `lib`, is at `root`.
    fn lib_at(root: &Path, manifest: &[ManifestEntry]) -> Local<Network> {
        Local::new(here(&[("lib", root.to_path_buf())]), 0, manifest, Network::default())
    }

    /// The network's side: what it was asked for, its own bytes; when
    /// `resuming`, a partial taken up from what it holds, each fetch into
    /// one kept with the bytes it held.
    #[derive(Default)]
    struct Network {
        fetched: Vec<String>,
        ranged: Vec<String>,
        resuming: bool,
        parts: Vec<(String, Option<u64>)>,
    }

    const OVER_THE_NETWORK: &[u8] = b"over the network";

    impl Fetcher for Network {
        fn fetch(&mut self, file: &WantedFile, into: &mut dyn Write) -> Result<Fetched, Failed> {
            self.fetched.push(file.ledger.server_path.clone());
            let failed = |e: io::Error| Failed { why: e.to_string(), stop: false };
            into.write_all(OVER_THE_NETWORK).map_err(failed)?;
            Ok(Fetched { content_type: Some("audio/flac".into()) })
        }

        fn read_range(&mut self, file: &WantedFile, _: u64, _: usize) -> Result<Vec<u8>, Failed> {
            self.ranged.push(file.ledger.server_path.clone());
            Ok(OVER_THE_NETWORK.to_vec())
        }

        fn resumable(&self, _file: &WantedFile) -> bool {
            self.resuming
        }

        fn fetch_part(
            &mut self,
            file: &WantedFile,
            have: Option<&Have>,
            into: &mut dyn Part,
        ) -> Result<Fetched, Failed> {
            self.parts.push((file.ledger.server_path.clone(), have.map(|h| h.bytes)));
            let failed = |e: io::Error| Failed { why: e.to_string(), stop: false };
            let from = have.map_or(0, |h| h.bytes);
            into.answered(from, Some("\"e\"")).map_err(failed)?;
            into.write_all(&OVER_THE_NETWORK[from as usize..]).map_err(failed)?;
            Ok(Fetched::default())
        }
    }

    /// A card's partial in memory: its bytes, and where each answer went on
    /// from.
    #[derive(Default)]
    struct Partial {
        bytes: Vec<u8>,
        answers: Vec<u64>,
    }

    impl Write for Partial {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Part for Partial {
        fn answered(&mut self, from: u64, _validator: Option<&str>) -> io::Result<()> {
            self.bytes.truncate(from as usize);
            self.answers.push(from);
            Ok(())
        }
    }

    fn fetched(source: &mut Local<Network>, file: &WantedFile) -> Vec<u8> {
        let mut into = Vec::new();
        source.fetch(file, &mut into).expect("the file's bytes");
        into
    }

    fn admin_here() -> LayeredAdmin {
        LayeredAdmin { platform: Some(if cfg!(windows) { "win32" } else { "linux" }.into()) }
    }

    fn codes(libraries: &[Library]) -> Vec<(&str, &str)> {
        libraries.iter().map(|l| (l.vpath.as_str(), l.folder.code())).collect()
    }

    #[test]
    fn the_folders_are_asked_for_only_as_an_admin_beside_the_server() {
        let asked = Cell::new(0);
        let ask = || {
            asked.set(asked.get() + 1);
            let dir = AdminDirEntry { id: 1, root: "/srv/music".into(), follow_symlinks: false };
            Ok(BTreeMap::from([("lib".to_string(), dir)]))
        };
        let admin = admin_here();

        let why = folders(None, Reached::Loopback, ask).unwrap_err();
        assert_eq!(why.code(), "not-admin");
        let why = folders(Some(&admin), Reached::Elsewhere, ask).unwrap_err();
        assert_eq!(why.code(), "elsewhere");
        let other = if cfg!(windows) { "linux" } else { "win32" };
        let away = LayeredAdmin { platform: Some(other.into()) };
        let why = folders(Some(&away), Reached::Loopback, ask).unwrap_err();
        assert!(matches!(&why, Why::OtherSystem(p) if p == other), "{why:?}");
        assert_eq!(asked.get(), 0, "admin/directories is asked only once the rest holds");

        for reached in [Reached::Loopback, Reached::Bundled] {
            let got = folders(Some(&admin), reached, ask).expect("the folders");
            assert_eq!(got, BTreeMap::from([("lib".to_string(), "/srv/music".to_string())]));
        }
        // An older server's admin block names no platform: the folders say.
        let unnamed = LayeredAdmin { platform: None };
        assert!(folders(Some(&unnamed), Reached::Bundled, ask).is_ok());
        assert_eq!(asked.get(), 3);

        let refused = || Err(ApiError::Forbidden("Admin API Disabled".into()));
        let why = folders(Some(&admin), Reached::Loopback, refused).unwrap_err();
        assert!(matches!(why, Why::NoAnswer(ApiError::Forbidden(_))), "{why:?}");
        assert_eq!(why.code(), "no-answer");
    }

    #[test]
    fn the_folders_come_from_admin_directories_on_a_loopback_server() {
        let scratch = Scratch::new("ask");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        let lib = serde_json::json!({
            "id": 1, "root": root.to_string_lossy(), "type": "music", "followSymlinks": false,
        });
        let answer = serde_json::json!({ "lib": lib });
        let (base, seen) = canned(vec![
            json("200 OK", &answer.to_string()),
            json("403 Forbidden", r#"{"error":"Access Denied"}"#),
        ]);
        let client = Client::new(&base).unwrap().with_token(Some("tok".into()));
        let admin = admin_here();

        let found = folders(Some(&admin), Reached::Loopback, || client.admin_directories());
        let request = seen.recv_timeout(Duration::from_secs(5)).expect("the request came");
        assert!(request.starts_with("GET /api/v1/admin/directories HTTP/1.1\r\n"), "{request}");
        let mut source = Local::new(found, 0, &manifest, Network::default());
        assert_eq!(source.mode().code(), "disk");
        assert_eq!(fetched(&mut source, &wanted(&manifest[0])), track(1));

        // Not an admin after all (the admin block was stale): the network.
        let found = folders(Some(&admin), Reached::Loopback, || client.admin_directories());
        let mut source = Local::new(found, 0, &manifest, Network::default());
        assert!(matches!(source.mode(), Mode::Network(Why::NoAnswer(ApiError::Forbidden(_)))));
        assert_eq!(fetched(&mut source, &wanted(&manifest[0])), OVER_THE_NETWORK);
    }

    #[test]
    fn a_folder_that_holds_the_manifests_files_is_read_from_disk() {
        let scratch = Scratch::new("read");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        let mut source = lib_at(&root, &manifest);

        let Mode::Disk(libraries) = source.mode() else { panic!("{:?}", source.mode()) };
        let read = Folder::Read { root: root.clone(), matched: 3, sampled: 3 };
        assert_eq!(libraries, &[Library { vpath: "lib".into(), folder: read }]);
        for (n, entry) in (1u8..).zip(&manifest) {
            let mut into = Vec::new();
            let got = source.fetch(&wanted(entry), &mut into).expect("the bytes");
            assert_eq!(into, track(n), "{}", entry.filepath);
            assert_eq!(got, Fetched::default(), "no answer, so no type to check");
        }
        assert_eq!(source.tally(), Tally { disk: 3, network: 0 });

        // U6's fingerprint, for free: the server's side read from disk.
        let file = wanted(&manifest[1]);
        let r = qfp_ranges(file.ledger.server_size);
        let head = source.read_range(&file, 0, r.head as usize).unwrap();
        let tail = source.read_range(&file, r.tail_offset, r.tail as usize).unwrap();
        assert_eq!(qfp(file.ledger.server_size, &head, &tail), qfp_of(&track(2)));
        let end = source.read_range(&file, 9_990, 4096).unwrap();
        assert_eq!(end, track(2)[9_990..], "fewer at its end");
        assert!(source.network.fetched.is_empty() && source.network.ranged.is_empty());
    }

    #[test]
    fn a_missing_folder_or_another_computers_goes_over_the_network() {
        let scratch = Scratch::new("missing");
        let home = scratch.0.join("Music");
        let mut manifest = library(&home, "lib");
        // A server in a container: its folder names a tree that isn't this
        // one, though a folder of that name is here, holding other files.
        let boxed = scratch.0.join("srv");
        for entry in library(&boxed, "box") {
            let rel = entry.filepath.split_once('/').unwrap().1;
            put(&boxed, rel, b"a file of the host's own", SCANNED);
            manifest.push(entry);
        }
        manifest.extend(entry_of("gone/x.flac"));
        manifest.extend(entry_of("away/x.flac"));
        let mut unknown = entry_of("bare/x.flac");
        unknown[0].modified = None;
        manifest.extend(unknown);

        let nowhere = scratch.0.join("Nowhere");
        let folders = [("lib", nowhere.clone()), ("box", boxed.clone()), ("bare", home.clone())];
        let folders = here(&folders);
        let mut folders = folders.unwrap();
        folders.insert("away".into(), "music".into());
        let mut source = Local::new(Ok(folders), 0, &manifest, Network::default());

        let Mode::Network(Why::NoFolder(libraries)) = source.mode() else {
            panic!("{:?}", source.mode())
        };
        let want = [
            ("away", "foreign"),
            ("bare", "unchecked"),
            ("box", "mismatch"),
            ("gone", "unlisted"),
            ("lib", "missing"),
        ];
        assert_eq!(codes(libraries), want);
        assert!(libraries.iter().any(|l| l.folder == Folder::Missing(nowhere.clone())));
        let mismatch = Folder::Mismatch { root: boxed.clone(), matched: 0, sampled: 3 };
        assert!(libraries.iter().any(|l| l.folder == mismatch));
        assert_eq!(source.mode().code(), "no-folder");

        for entry in &manifest[..4] {
            assert_eq!(fetched(&mut source, &wanted(entry)), OVER_THE_NETWORK);
        }
        let paths: Vec<&str> = manifest[..4].iter().map(|e| e.filepath.as_str()).collect();
        assert_eq!(source.network.fetched, paths);
        assert_eq!(source.tally(), Tally::default(), "no file of a library read from here");
    }

    fn entry_of(path: &str) -> Vec<ManifestEntry> {
        entries(&[(path.to_string(), 10)])
    }

    #[test]
    fn a_file_changed_since_the_scan_comes_over_the_network() {
        let scratch = Scratch::new("changed");
        let root = scratch.0.join("Music");
        let mut manifest = library(&root, "lib");
        let fourth = "lib/Artist C/04.mp3".to_string();
        put(&root, "Artist C/04.mp3", &track(4), SCANNED);
        manifest.extend(entries(&[(fourth, 10_000)]));
        // Re-tagged after the scan: three in four still hold, so the folder
        // is this computer's.
        put(&root, "Artist A/Album B/02.flac", &track(9), SCANNED + 5_000);
        let mut source = lib_at(&root, &manifest);
        let Mode::Disk(libraries) = source.mode() else { panic!("{:?}", source.mode()) };
        let read = Folder::Read { root: root.clone(), matched: 3, sampled: 4 };
        assert_eq!(libraries[0].folder, read);

        // And one deleted since: each file is checked again as it is read.
        fs::remove_file(root.join("Artist C/03.mp3")).unwrap();
        let got: Vec<Vec<u8>> = manifest.iter().map(|e| fetched(&mut source, &wanted(e))).collect();
        assert_eq!(got, [track(1), OVER_THE_NETWORK.to_vec(), OVER_THE_NETWORK.to_vec(), track(4)]);
        assert_eq!(source.network.fetched, [manifest[1].filepath.as_str(), &manifest[2].filepath]);
        assert_eq!(source.tally(), Tally { disk: 2, network: 2 });

        // A fingerprint of a changed file is the server's, over the network.
        assert_eq!(source.read_range(&wanted(&manifest[1]), 0, 4096).unwrap(), OVER_THE_NETWORK);
        // The scan's ms, as Node's float may round it: still the file.
        let mut near = wanted(&manifest[0]);
        near.ledger.server_modified += 1;
        assert_eq!(fetched(&mut source, &near), track(1));
        near.ledger.server_modified += 1;
        assert_eq!(fetched(&mut source, &near), OVER_THE_NETWORK);
    }

    /// A card's side of a copy, as a test makes it: `room` bytes, then a
    /// refusal. `first` runs as the first bytes come, to change the file on
    /// disk while it is read.
    struct Card<'a> {
        got: Vec<u8>,
        room: usize,
        first: Option<Box<dyn FnOnce() + 'a>>,
    }

    impl<'a> Card<'a> {
        fn new(room: usize, first: impl FnOnce() + 'a) -> Card<'a> {
            Card { got: Vec::new(), room, first: Some(Box::new(first)) }
        }
    }

    impl Write for Card<'_> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(first) = self.first.take() {
                first();
            }
            let n = buf.len().min(self.room - self.got.len());
            if n == 0 {
                return Err(io::Error::other("the card is full"));
            }
            self.got.extend_from_slice(&buf[..n]);
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_file_that_changes_as_it_is_read_is_skipped_never_taken() {
        let scratch = Scratch::new("cut");
        let root = scratch.0.join("Music");
        let mut manifest = library(&root, "lib");
        put(&root, "Artist C/04.mp3", &track(4), SCANNED);
        manifest.extend(entries(&[("lib/Artist C/04.mp3".into(), 10_000)]));
        let mut source = lib_at(&root, &manifest);
        assert_eq!(source.mode().code(), "disk");

        // Each changed as its first bytes reach the card (the reader's own
        // handle still open): it grew, it shrank, it was rewritten at its own
        // size (past 25 MiB no hash of the whole would catch that); and a
        // card that refuses a write, which the network can't mend.
        let file = |n: usize| root.join(manifest[n].filepath.split_once('/').unwrap().1);
        let open = |n: usize| File::options().write(true).open(file(n)).unwrap();
        let append = |n: usize| File::options().append(true).open(file(n)).unwrap();
        let at = UNIX_EPOCH + Duration::from_millis(SCANNED + 60_000);
        let cases: Vec<(usize, usize, Box<dyn FnOnce() + '_>, &str)> = vec![
            (0, usize::MAX, Box::new(|| append(0).write_all(b"x").unwrap()), "changed on disk"),
            (1, usize::MAX, Box::new(|| open(1).set_len(5_000).unwrap()), "changed on disk"),
            (2, usize::MAX, Box::new(|| open(2).set_modified(at).unwrap()), "changed on disk"),
            (3, 4_000, Box::new(|| ()), "the card is full"),
        ];
        for (n, room, change, why) in cases {
            let mut card = Card::new(room, change);
            let failed = source.fetch(&wanted(&manifest[n]), &mut card).unwrap_err();
            assert!(!failed.stop && failed.why.contains(why), "{n}: {failed:?}");
        }
        let grew = fs::metadata(file(0)).unwrap().len();
        assert_eq!(grew, 10_001, "the change was made while the file was read");
        assert!(source.network.fetched.is_empty(), "never again over the network");
        assert_eq!(source.tally(), Tally::default(), "neither from disk nor the network");

        // The next run: the changed files over the network, the one the card
        // refused from disk.
        let got: Vec<Vec<u8>> = manifest.iter().map(|e| fetched(&mut source, &wanted(e))).collect();
        let over = OVER_THE_NETWORK.to_vec();
        assert_eq!(got, [over.clone(), over.clone(), over, track(4)]);
        assert_eq!(source.tally(), Tally { disk: 1, network: 3 });
    }

    /// A file a later run may take up (a peer's: another origin's, never
    /// read from here) is the network's to say so and to take up, from the
    /// bytes its partial holds; one read from disk starts its partial over,
    /// copied whole.
    #[test]
    fn a_partial_is_the_networks_to_take_up_and_a_disk_copy_starts_it_over() {
        let scratch = Scratch::new("part");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        let mut source = lib_at(&root, &manifest);
        let theirs = WantedFile { origin: 1, ..wanted(&manifest[0]) };
        assert!(!source.resumable(&theirs));
        source.network.resuming = true;
        assert!(source.resumable(&theirs), "the network's say");
        let have = Have { bytes: 4, validator: "\"e\"".into() };
        let mut part = Partial { bytes: OVER_THE_NETWORK[..4].to_vec(), ..Partial::default() };
        source.fetch_part(&theirs, Some(&have), &mut part).expect("taken up");
        assert_eq!(source.network.parts, [(theirs.ledger.server_path.clone(), Some(4))]);
        assert_eq!((part.bytes.as_slice(), part.answers.as_slice()), (OVER_THE_NETWORK, &[4][..]));

        // The server's own file, as the disk has it: whole, from byte 0.
        let mut part = Partial { bytes: b"stale".to_vec(), ..Partial::default() };
        source.fetch_part(&wanted(&manifest[0]), Some(&have), &mut part).expect("copied");
        assert_eq!((part.bytes, part.answers), (track(1), vec![0]));
        assert_eq!(source.network.parts.len(), 1, "the network isn't asked");
        assert_eq!(source.tally(), Tally { disk: 1, network: 0 });

        // Changed since the scan: the network's again, counted so.
        fs::remove_file(root.join("Artist C/03.mp3")).unwrap();
        let gone = wanted(&manifest[2]);
        source.fetch_part(&gone, None, &mut Partial::default()).expect("over the network");
        assert_eq!(source.network.parts[1], (gone.ledger.server_path.clone(), None));
        assert_eq!(source.tally(), Tally { disk: 1, network: 1 });
    }

    /// A disk whose read fails once `ok` bytes have come, after an
    /// interruption first if `interrupted`.
    struct Disk {
        ok: usize,
        interrupted: bool,
    }

    impl Read for Disk {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.interrupted) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.ok == 0 {
                return Err(io::Error::other("a bad sector"));
            }
            let n = buf.len().min(self.ok);
            buf[..n].fill(b'a');
            self.ok -= n;
            Ok(n)
        }
    }

    #[test]
    fn a_copy_cut_before_its_first_byte_can_still_come_over_the_network() {
        let scratch = Scratch::new("pour");
        let path = scratch.0.join("01.flac");
        let l = Ledger { server_size: 10_000, server_modified: SCANNED, ..Ledger::default() };
        let mut into = Vec::new();

        // Gone, emptied or unreadable since its check: nothing on the card.
        assert!(matches!(copy(&path, &l, &mut into), Err(Cut::Unread)));
        put(&scratch.0, "01.flac", b"", SCANNED);
        assert!(matches!(copy(&path, &l, &mut into), Err(Cut::Unread)));
        let unread = pour(&mut Disk { ok: 0, interrupted: true }, &mut into);
        assert!(matches!(unread, Err(Cut::Unread)), "{unread:?}");
        assert!(into.is_empty());

        // Some of it on the card: shorter or longer than the manifest says,
        // or a read that fails partway.
        put(&scratch.0, "01.flac", &track(1)[..4_000], SCANNED);
        assert!(matches!(copy(&path, &l, &mut into), Err(Cut::Partway(_))));
        assert_eq!(into.len(), 4_000);
        into.clear();
        let mut longer = track(1);
        longer.extend(track(2));
        put(&scratch.0, "01.flac", &longer, SCANNED);
        assert!(matches!(copy(&path, &l, &mut into), Err(Cut::Partway(_))));
        assert_eq!(into, longer[..10_001], "one byte past its size, and no more");
        into.clear();
        let partway = pour(&mut Disk { ok: 3_000, interrupted: false }, &mut into);
        assert!(matches!(&partway, Err(Cut::Partway(why)) if why.contains("a bad sector")));
        assert_eq!(into.len(), 3_000);

        // An empty file the manifest says is empty: taken.
        let empty = Ledger { server_size: 0, ..l };
        put(&scratch.0, "01.flac", b"", SCANNED);
        into.clear();
        assert!(copy(&path, &empty, &mut into).is_ok() && into.is_empty());
    }

    #[test]
    fn conversions_and_other_servers_files_are_never_read_from_disk() {
        let scratch = Scratch::new("convert");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        let mut source = lib_at(&root, &manifest);

        let mut converted = wanted(&manifest[0]);
        converted.bytes = Bytes::Converted(CONTAINER_OPUS);
        let mut into = Vec::new();
        let got = source.fetch(&converted, &mut into).unwrap();
        assert_eq!(into, OVER_THE_NETWORK);
        assert_eq!(got.content_type.as_deref(), Some("audio/flac"), "the network's own answer");

        let mut peers = wanted(&manifest[1]);
        peers.origin = 1;
        assert_eq!(fetched(&mut source, &peers), OVER_THE_NETWORK);
        assert_eq!(source.read_range(&peers, 0, 4096).unwrap(), OVER_THE_NETWORK);

        // An Opus stream stored as .ogg is the server's bytes: from disk.
        let mut ogg = wanted(&manifest[2]);
        ogg.bytes = Bytes::Original { ogg_opus: true };
        assert_eq!(fetched(&mut source, &ogg), track(3));
        assert_eq!(source.network.fetched, [manifest[0].filepath.as_str(), &manifest[1].filepath]);
        assert_eq!(source.tally(), Tally { disk: 1, network: 0 }, "neither was the disk's to read");
    }

    #[test]
    fn a_path_never_leaves_its_folder() {
        let root = Path::new(if cfg!(windows) { r"C:\Music" } else { "/srv/music" });
        assert_eq!(join(root, "Artist A/01.flac"), Some(root.join("Artist A").join("01.flac")));
        let climbs = ["", "../x.flac", "a/../../x.flac", "a/./x.flac", "a//x.flac", "a/"];
        for rel in climbs.into_iter().chain(["x\0.flac"]) {
            assert_eq!(join(root, rel), None, "{rel:?}");
        }
        if cfg!(windows) {
            for rel in [r"a\..\..\x.flac", "C:x.flac", "x.flac:stream", r"\\server\share"] {
                assert_eq!(join(root, rel), None, "{rel:?}");
            }
        }

        // A manifest path that would climb out: never read, whatever is there.
        let scratch = Scratch::new("climb");
        let lib = scratch.0.join("Music");
        let mut manifest = library(&lib, "lib");
        put(&scratch.0, "secret.flac", &track(7), SCANNED);
        manifest.extend(entries(&[("lib/../secret.flac".into(), 10_000)]));
        let mut source = lib_at(&lib, &manifest);
        assert_eq!(source.mode().code(), "disk");
        assert_eq!(fetched(&mut source, &wanted(&manifest[3])), OVER_THE_NETWORK);
    }

    /// Everything under `root`: each path, its bytes (none for a folder)
    /// and its mtime (a folder's moves when a name in it comes or goes).
    fn tree(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
        let mut out = BTreeMap::new();
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            out.insert(dir.clone(), (Vec::new(), fs::metadata(&dir).unwrap().modified().unwrap()));
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    dirs.push(entry.path());
                } else {
                    let bytes = fs::read(entry.path()).unwrap();
                    out.insert(entry.path(), (bytes, meta.modified().unwrap()));
                }
            }
        }
        out
    }

    /// Files made read-only for as long as it lives, writable again when it
    /// drops (a failed assertion included), so their scratch folder can go.
    /// Held after the [`Scratch`], it drops first.
    struct ReadOnly(Vec<PathBuf>);

    impl ReadOnly {
        fn new(files: Vec<PathBuf>) -> ReadOnly {
            for path in &files {
                read_only(path, true).expect("a file made read-only");
            }
            ReadOnly(files)
        }
    }

    impl Drop for ReadOnly {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = read_only(path, false);
            }
        }
    }

    fn read_only(path: &Path, yes: bool) -> io::Result<()> {
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_readonly(yes);
        fs::set_permissions(path, perms)
    }

    #[test]
    fn the_source_is_only_read() {
        let scratch = Scratch::new("only-read");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        put(&root, "Artist A/Album B/cover.jpg", b"a cover", SCANNED);
        // Its files read-only, as a server's library may be to this user:
        // an open for writing would fail.
        let files: Vec<PathBuf> = tree(&root).into_keys().filter(|p| p.is_file()).collect();
        let _held = ReadOnly::new(files);
        let before = tree(&root);

        let mut source = lib_at(&root, &manifest);
        let asked: Vec<WantedFile> = manifest.iter().map(wanted).collect();
        let copies: Vec<Vec<u8>> = asked.iter().map(|w| fetched(&mut source, w)).collect();
        let heads: Vec<_> = asked.iter().map(|w| source.read_range(w, 0, 16).unwrap()).collect();
        let cover = source.folder_image("lib/Artist A/Album B").map(|c| c.bytes);
        let after = tree(&root);

        assert_eq!(copies, [track(1), track(2), track(3)]);
        let head = |n: u8| track(n)[..16].to_vec();
        assert_eq!(heads, [head(1), head(2), head(3)]);
        assert_eq!(cover.as_deref(), Some(b"a cover".as_slice()));
        assert_eq!(after, before, "nothing there was written, added or touched");
        assert_eq!(source.tally(), Tally { disk: 3, network: 0 });
    }

    #[test]
    fn a_folders_cover_is_the_one_the_device_would_elect() {
        let ranks = [
            ("cover.jpg", Some(0)),
            ("COVER.JPEG", Some(0)),
            ("Folder.jpg", Some(1)),
            ("front.JPG", Some(2)),
            ("back.jpg", Some(3)),
            ("front cover.jpeg", Some(3)),
            ("cover.png", None),
            ("cover", None),
            (".jpg", None),
            ("cover.jpg.txt", None),
        ];
        for (name, rank) in ranks {
            assert_eq!(image_rank(name), rank, "{name}");
        }

        let scratch = Scratch::new("covers");
        let root = scratch.0.join("Music");
        let manifest = library(&root, "lib");
        let image = |rel: &str, size: usize| put(&root, rel, &vec![b'j'; size], SCANNED);
        // Named covers rank above any size, and PNG is never one.
        image("Artist A/Album B/cover.png", 900);
        image("Artist A/Album B/scan.jpg", 800);
        image("Artist A/Album B/Front.jpeg", 20);
        image("Artist A/Album B/folder.jpg", 10);
        // No good name: the largest, the first by name of two that size;
        // never a hidden one.
        image("Artist C/back.jpg", 10);
        image("Artist C/cd.jpeg", 30);
        image("Artist C/booklet.jpg", 30);
        image("Artist C/._booklet.jpg", 100);
        image("Artist C/booklet.png", 100);
        let source = lib_at(&root, &manifest);

        let named = |folder: &str| source.folder_image(folder).map(|i| (i.name, i.bytes.len()));
        assert_eq!(named("lib/Artist A/Album B"), Some(("folder.jpg".into(), 10)));
        assert_eq!(named("lib/Artist C"), Some(("booklet.jpg".into(), 30)));
        assert_eq!(named("lib/Artist A"), None, "a folder with no image");
        assert_eq!(named("lib/Artist D"), None, "no such folder");
        assert_eq!(named("lib/../Music/Artist C"), None, "never outside a library");
        assert_eq!(named("other/Artist C"), None, "not a library read from here");

        let tied = vec![(0, "cover.jpeg".to_string(), 1), (0, "Cover.jpg".to_string(), 9)];
        assert_eq!(elect(tied).map(|(_, name, _)| name).as_deref(), Some("Cover.jpg"));
        assert_eq!(elect(Vec::new()), None);
    }
}
