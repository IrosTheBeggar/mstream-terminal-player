//! Card paths from mStream's paths (the firmware repo's docs/METADATA.md
//! 2.8.3-2.8.6). mStream names a file `vpath/rel`; the card has `/music`,
//! FAT's rules for a name, and a player that walks 8 folder levels and
//! skips a path over 255 bytes. 2.8.3's steps, and where each is done:
//! 1. split at `/`, the vpath dropped or kept as a root by the [`Layout`]
//!    (2.8.6);
//! 2. each component made a safe name: [`clean_name`];
//! 3. the extension gated by `device.txt` (2.15), by what the bytes are
//!    ([`Bytes`]);
//! 4. folders past the 8th level joined with ` - `;
//! 5. the path cut to 255 bytes, each cut name marked `~XXXX`;
//! 6. collisions by the [`match_key`], FAT's case-insensitive rule: folders
//!    merge, and of two files the lower (server, mStream id) keeps the name;
//! 7. the names read back after writing: the engine's, through
//!    [`stored_name`] (2.8.5).
//!
//! A [`Namer`] names one run's new files against what the card lists, and
//! reports what 2.8.4 says to (big folders, files FAT32 can't hold). The
//! names it gives are recorded in the ledger, which — not these rules — is
//! authoritative for a file already on the card: the namer is seeded with
//! the card's listing, and learns from each kept ledger row how the
//! server's folders were named there (a cut, a ` (2)`, the listener's own
//! renaming), so a new file joins its album's folder whatever that is
//! called. Pure code: nothing here touches a card.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use unicode_normalization::{UnicodeNormalization, is_nfc};

use super::contract::device_txt::DeviceTxt;
use super::contract::mptg::{CONTAINER_FLAC, CONTAINER_MP3, CONTAINER_OPUS};
use super::contract::order::{MAX_CARD_PATH, valid_rel_path};
use super::contract::prim::fnv1a64;

// ── The limits (2.8.2, 2.8.4, 2.8.6) ────────────────────────────────────────

/// The folder levels the device walks below `/music` (its `kMaxDepth`).
pub(crate) const MAX_DEPTH: usize = 8;
/// FAT's longest name, in UTF-16 code units.
pub(crate) const MAX_NAME_UNITS: usize = 255;
/// The library roots the device keeps from LIBR (its `cardroot::kMaxRoots`):
/// a root past them would index under `/music`.
pub(crate) const MAX_ROOTS: usize = 16;
/// FAT32's largest file: 4 GiB − 1 bytes.
pub(crate) const MAX_FILE_BYTES: u64 = u32::MAX as u64;
/// The directory entries one FAT32 folder holds.
pub(crate) const MAX_DIR_ENTRIES: usize = 65_536;
/// A folder of more entries than this is reported: the device searches a
/// folder linearly, so the software keeps them under about 2,000.
pub(crate) const BIG_FOLDER: usize = 2_000;

/// What joins the 8th and deeper folders into one (2.8.3, step 4).
const JOIN: &str = " - ";
/// `/music/`, which every path's 255 bytes count.
const MUSIC: usize = "/music/".len();
/// The room a folder's cut leaves below it (step 5). A folder is cut for
/// the file whose path was too long, but it is the folder of all its
/// files: cut just enough for a short name, it would leave a sibling with
/// a longer one nothing to cut into. So a folder is cut to end its path
/// this much short of 255 bytes, whatever is below it, and every sibling
/// fits, its own stem cut if need be. Only a folder still the longest
/// name at that is cut again, as far as the path needs.
const ROOM_BELOW_CUT: usize = 64;

// ── Names (2.8.3 step 2, 2.8.5) ─────────────────────────────────────────────

/// `text` in NFC, borrowed when it is already.
pub(crate) fn nfc(text: &str) -> Cow<'_, str> {
    if is_nfc(text) { Cow::Borrowed(text) } else { Cow::Owned(text.nfc().collect()) }
}

/// One component of mStream's path as a card name (2.8.3, step 2): NFC;
/// U+0000-U+001F, U+007F and `" * / : < > ? \ |` to `_`; leading spaces,
/// and trailing spaces and dots, dropped (FAT and Windows drop trailing
/// ones silently); leading dots to `_`, or the device would hide the name;
/// `_` for what is left empty; and `_` after the stem of a Windows device
/// name (`CON` to `CON_`, `con.mp3` to `con_.mp3`). Never empty, `.` or
/// `..`, and no `/` or NUL: a valid name (2.4.3).
pub(crate) fn clean_name(raw: &str) -> String {
    let swapped: String = nfc(raw).chars().map(|c| if illegal(c) { '_' } else { c }).collect();
    let trimmed = swapped.trim_start_matches(' ').trim_end_matches([' ', '.']);
    let undotted = trimmed.trim_start_matches('.');
    let mut name = "_".repeat(trimmed.len() - undotted.len()) + undotted;
    if name.is_empty() {
        name.push('_');
    }
    if let Some(at) = device_name_end(&name) {
        name.insert(at, '_');
    }
    name
}

fn illegal(c: char) -> bool {
    matches!(c, '\0'..='\u{1F}' | '\u{7F}' | '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|')
}

/// Where the Windows device name ends when `name`'s stem is one, in any
/// case: `CON PRN AUX NUL COM1-COM9 LPT1-LPT9`. The stem runs to the first
/// dot, its trailing spaces aside (Windows reads `CON .mp3` as `CON`).
fn device_name_end(name: &str) -> Option<usize> {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let up = stem.to_ascii_uppercase();
    let numbered = |p: &str| up.len() == 4 && up.starts_with(p) && matches!(up.as_bytes()[3], b'1'..=b'9');
    let device = matches!(up.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT");
    device.then_some(stem.len())
}

/// A name's match key (2.8.3, step 6): its NFC form, then each code point's
/// Unicode simple upper case. That is FAT's case-insensitive rule, and a
/// little wider: FAT keeps an NFC and an NFD spelling apart, the key merges
/// them, so no look-alike twin is ever made. Two names are one to FAT (and
/// to the software's matching, 2.10.2) when their keys are equal.
pub(crate) fn match_key(name: &str) -> String {
    nfc(name).chars().map(simple_upper).collect()
}

/// A code point's simple upper case (UnicodeData.txt's mapping), taken from
/// Rust's std as the player's nameKey tables are (the firmware repo's
/// tools/unicode_case.rs). std gives the full mapping, SpecialCasing.txt's
/// first; the code points it maps to several have a simple mapping of their
/// own, which is the code point itself but for the Greek vowels with
/// ypogegrammeni, whose simple upper case is their title case form with
/// prosgegrammeni: U+1F80 to U+1F88, U+1FB3 to U+1FBC. So U+00DF (sharp s)
/// stays itself, never `SS`.
fn simple_upper(c: char) -> char {
    let mut full = c.to_uppercase();
    match (full.next(), full.next()) {
        (Some(up), None) => up,
        _ => match c as u32 {
            0x1F80..=0x1F87 | 0x1F90..=0x1F97 | 0x1FA0..=0x1FA7 => char::from_u32(c as u32 + 8).unwrap_or(c),
            0x1FB3 | 0x1FC3 | 0x1FF3 => char::from_u32(c as u32 + 9).unwrap_or(c),
            _ => c,
        },
    }
}

/// How a PC lists the long names a card stores (2.8.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Listing {
    /// Unchanged: Windows, and Linux on a vfat mount with `utf8` (one
    /// without lists escapes, and the volume layer refuses it).
    AsStored,
    /// Decomposed: macOS stores names precomposed and lists them NFD, as it
    /// does on HFS+ (the reason for Git's `core.precomposeUnicode`).
    Decomposed,
}

impl Listing {
    /// This platform's.
    pub(crate) fn here() -> Listing {
        if cfg!(target_os = "macos") { Listing::Decomposed } else { Listing::AsStored }
    }
}

/// A listed name brought back to its stored spelling (2.8.5): the one FatFs
/// returns on the device, which every path hash and record uses. NFC for a
/// macOS listing: exact for every name the software makes and every name
/// macOS or Windows wrote. A name another system stored NFD reads wrong on
/// a Mac; the device then scans that file itself.
pub(crate) fn stored_name(listed: &str, listing: Listing) -> Cow<'_, str> {
    match listing {
        Listing::AsStored => Cow::Borrowed(listed),
        Listing::Decomposed => nfc(listed),
    }
}

/// A file of `size` bytes fits FAT32 (2.8.4); a larger one is skipped, or
/// converted.
pub(crate) fn fits_fat32(size: u64) -> bool {
    size <= MAX_FILE_BYTES
}

/// The directory entries a name takes in a FAT folder (2.8.4): its short
/// entry, and a long one for each 13 UTF-16 units.
pub(crate) fn dir_entries(name: &str) -> usize {
    1 + utf16_len(name).div_ceil(13)
}

fn utf16_len(name: &str) -> usize {
    name.chars().map(char::len_utf16).sum()
}

/// A cut name's mark (2.8.3, step 5): `~` and the low 16 bits of the
/// FNV-1a 64 of the name's bytes as steps 2-4 left it, uncut, as 4
/// uppercase hex digits: a folder's whole name (a joined one too), a file's
/// stem without its extension.
fn cut_mark(original: &str) -> String {
    format!("~{:04X}", fnv1a64(original.as_bytes()) & 0xFFFF)
}

/// The extension of a format by its container code (2.6.4).
fn container_extension(container: u8) -> Option<&'static str> {
    match container {
        CONTAINER_MP3 => Some("mp3"),
        CONTAINER_FLAC => Some("flac"),
        CONTAINER_OPUS => Some("opus"),
        _ => None,
    }
}

// ── What the namer is given and gives ───────────────────────────────────────

/// Where a card keeps each library (2.8.6). The device names a folder album
/// by its depth below a root: the first folder is the artist, the second
/// the album.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// The vpath dropped: the card mirrors each library's own tree, `/music`
    /// is the only root, and same-named folders of two libraries merge.
    Default,
    /// A `/music/<vpath>/` folder for each library, listed in the root's
    /// LIBR section ([`Namer::roots`]): the libraries by server index and
    /// vpath, at most [`MAX_ROOTS`]. Two libraries' folders never merge.
    Vpath(Vec<(u16, String)>),
}

/// A layout the device can't keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LayoutError(pub(crate) &'static str);

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for LayoutError {}

/// What a file's bytes on the card are, which says its extension (2.8.3,
/// step 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bytes {
    /// The server's file byte for byte (ORIG's `convertedTo` 0): its
    /// extension stays as the server has it, and must be one `device.txt`
    /// lists. `ogg_opus`: an Opus stream the server stores as `.ogg` or
    /// `.oga` (mStream's `format` is the extension, so it says `ogg` or
    /// `oga`), copied as it is and named `.opus`.
    Original { ogg_opus: bool },
    /// A conversion, by the container code it was converted to (2.6.4): the
    /// name takes that format's extension.
    Converted(u8),
    /// Not audio (a file the software makes, such as a cover): its name
    /// stays as it is, extension and all.
    NotAudio,
}

/// A file a run puts on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Wanted<'a> {
    /// The card's index of its server (2.5.5: 0 for the root's own).
    pub(crate) server: u16,
    /// mStream's id of the file (the ledger's `mstreamId`); `u32::MAX` for a
    /// file of the software's own, named after every track.
    pub(crate) id: u32,
    /// mStream's `filepath`: `vpath/rel`.
    pub(crate) server_path: &'a str,
    /// The bytes that go on the card (a conversion's, once known).
    pub(crate) size: u64,
    pub(crate) bytes: Bytes,
}

/// Why a file gets no name on the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Its server path has no name below the vpath.
    NoName,
    /// Under the vpath layout, its library isn't one the layout lists.
    NoRoot,
    /// Its extension isn't one `device.txt` lists (2.8.3, step 3): an audio
    /// file like it MUST NOT reach the card. A conversion may.
    Unlisted,
    /// 4 GiB or more, past FAT32's limit (2.8.4): skipped, or converted.
    TooBig,
    /// Its path can't be cut to 255 bytes: the folders already on the card
    /// leave no room (2.8.3, step 5).
    TooLong,
    /// Its folder has no directory entries left for it (2.8.4).
    FolderFull,
}

/// A file refused, as the report lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Skipped {
    pub(crate) server: u16,
    pub(crate) server_path: String,
    pub(crate) size: u64,
    pub(crate) why: Refused,
}

/// What 2.8.4 says to report, and every file refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// The folders the naming added to that hold more than [`BIG_FOLDER`]
    /// entries, with their counts, by path.
    pub(crate) big_folders: Vec<(String, usize)>,
    /// The files refused, in the order they were named.
    pub(crate) skipped: Vec<Skipped>,
}

impl Report {
    /// The files of 4 GiB or more (2.8.4).
    pub(crate) fn too_big(&self) -> impl Iterator<Item = &Skipped> {
        self.skipped.iter().filter(|s| s.why == Refused::TooBig)
    }
}

// ── The namer ───────────────────────────────────────────────────────────────

/// One folder of the card, as listed and as the naming grows it.
struct Folder {
    /// Relative to `/music` (`""` for `/music` itself).
    path: String,
    /// Its entries by match key: each one's name, and a folder's index.
    entries: HashMap<String, Vec<(String, Option<usize>)>>,
    count: usize,
    /// The FAT directory entries it uses, `.` and `..` among them.
    dir_entries: usize,
}

impl Folder {
    fn new(path: String) -> Folder {
        Folder { path, entries: HashMap::new(), count: 0, dir_entries: 2 }
    }

    /// The entry spelled exactly `name`.
    fn exact(&self, name: &str) -> Option<&(String, Option<usize>)> {
        self.entries.get(&match_key(name))?.iter().find(|(n, _)| n == name)
    }

    /// The folder a folder named `name` (of key `key`) merges with: the one
    /// spelled `name`, else the first by bytes of those with its key.
    fn merge_with(&self, key: &str, name: &str) -> Option<usize> {
        let folders = || self.entries.get(key).into_iter().flatten().filter(|(_, f)| f.is_some());
        let exact = folders().find(|(n, _)| n == name);
        exact.or_else(|| folders().min_by(|a, b| a.0.cmp(&b.0))).and_then(|(_, f)| *f)
    }

    fn add(&mut self, name: String, folder: Option<usize>) {
        self.count += 1;
        self.dir_entries += dir_entries(&name);
        self.entries.entry(match_key(&name)).or_default().push((name, folder));
    }

    /// The file entry spelled exactly `name` is the folder `f` instead.
    fn make_folder(&mut self, name: &str, f: usize) {
        let entries = self.entries.get_mut(&match_key(name));
        if let Some(entry) = entries.and_then(|e| e.iter_mut().find(|(n, _)| n == name)) {
            entry.1 = Some(f);
        }
    }
}

/// One name of a path being fitted (2.8.3, steps 5 and 6): a folder to
/// make, or the file.
struct Part {
    /// The clean name, uncut; the file's stem.
    body: String,
    /// The file's extension with its dot; "" for a folder.
    ext: String,
    /// The bytes of `body` kept, once it is cut.
    kept: Option<usize>,
    /// The cut's mark: of the uncut name, the file's stem alone.
    mark: String,
    /// The collision's number: ` (n)` from 2, before the extension.
    n: u32,
}

impl Part {
    fn new(body: String, ext: String) -> Part {
        let mark = cut_mark(&body);
        Part { body, ext, kept: None, mark, n: 0 }
    }

    /// The bytes of the name that can be cut.
    fn cuttable(&self) -> usize {
        self.kept.unwrap_or(self.body.len())
    }

    /// Cuts the name `by` bytes shorter (a little more to stop at a code
    /// point boundary; at most to its mark alone), and marks it.
    fn cut(&mut self, by: usize) {
        let mark = if self.kept.is_none() { self.mark.len() } else { 0 };
        let mut keep = self.cuttable().saturating_sub(by + mark);
        while !self.body.is_char_boundary(keep) {
            keep -= 1;
        }
        self.kept = Some(keep);
    }

    fn name(&self) -> String {
        let mut name = match self.kept {
            None => self.body.clone(),
            Some(k) => format!("{}{}", &self.body[..k], self.mark),
        };
        if self.n >= 2 {
            name += &format!(" ({})", self.n);
        }
        name + &self.ext
    }

    fn bump(&mut self) {
        self.n = self.n.max(1) + 1;
    }
}

/// Step 5: while `/music/`, the folder `prefix` bytes long and the parts
/// below it are over 255 bytes, or a name is over 255 UTF-16 units, cut the
/// longest part (ties: the deepest; the file by its stem) and mark it: a
/// folder to end its path [`ROOM_BELOW_CUT`] short of 255 bytes, which the
/// parts below it don't change; the file, or a folder ending there or
/// sooner, as far as the path needs. False when nothing is left to cut.
fn fit(prefix: usize, parts: &mut [Part]) -> bool {
    let above = MUSIC + prefix + usize::from(prefix > 0);
    loop {
        let lens: Vec<usize> = parts.iter().map(|p| p.name().len()).collect();
        let total = above + lens.iter().sum::<usize>() + lens.len() - 1;
        let long_name = parts.iter().any(|p| utf16_len(&p.name()) > MAX_NAME_UNITS);
        if total <= MAX_CARD_PATH && !long_name {
            return true;
        }
        let cuttable = (0..parts.len()).filter(|&i| parts[i].cuttable() > 0);
        let Some(i) = cuttable.max_by_key(|&i| (parts[i].cuttable(), i)) else { return false };
        let over = total.saturating_sub(MAX_CARD_PATH).max(1);
        let end = above + lens[..=i].iter().sum::<usize>() + i;
        let to_room = end.saturating_sub(MAX_CARD_PATH - ROOM_BELOW_CUT);
        parts[i].cut(if i + 1 < parts.len() && to_room > 0 { to_room } else { over });
    }
}

/// A library's root folder under the vpath layout: its vpath, the path's
/// first component (step 1), as a clean name, and one too long to leave
/// [`ROOM_BELOW_CUT`] below it cut as step 5 cuts a folder. It depends on
/// the vpath alone, so every run finds the root an earlier one made.
fn root_name(vpath: &str) -> String {
    let mut root = Part::new(clean_name(vpath), String::new());
    let over = (MUSIC + root.body.len()).saturating_sub(MAX_CARD_PATH - ROOM_BELOW_CUT);
    if over > 0 {
        root.cut(over);
    }
    root.name()
}

/// Names a run's new files on one card (2.8.3-2.8.6). Seed it with what the
/// card lists ([`Namer::seed`]) and the ledger rows the run keeps
/// ([`Namer::seed_kept`]), the plan's deletes left out, then [`Namer::place`]
/// the files to write. A write the OS refuses as "already exists" is a
/// collision too (step 6): placed again, the file takes the next name, the
/// one it was given counting as taken.
pub(crate) struct Namer {
    vpath_layout: bool,
    /// The vpath layout's libraries, by (server index, vpath).
    libraries: Vec<(u16, String)>,
    device: DeviceTxt,
    folders: Vec<Folder>,
    /// The folder each server folder has, by its parent and the match key
    /// of the clean, uncut name the rules give it: the folders this naming
    /// made, and those a kept row showed.
    known: HashMap<(usize, String), usize>,
    /// Each library's root folder, once it has one.
    roots: HashMap<(u16, String), usize>,
    /// The folders the naming added an entry to.
    grown: BTreeSet<usize>,
    skipped: Vec<Skipped>,
}

impl Namer {
    /// A namer for a card of this layout and this `device.txt` (its
    /// `extensions` gate the names: 2.15).
    pub(crate) fn new(layout: Layout, device: &DeviceTxt) -> Result<Namer, LayoutError> {
        let (vpath_layout, mut libraries) = match layout {
            Layout::Default => (false, Vec::new()),
            Layout::Vpath(libraries) => (true, libraries),
        };
        if libraries.len() > MAX_ROOTS {
            return Err(LayoutError("more libraries than the device keeps roots (16)"));
        }
        // Of two libraries whose folders would be called alike, the lower
        // server index, then the vpath's bytes, keeps the name.
        libraries.sort();
        if libraries.windows(2).any(|w| w[0] == w[1]) {
            return Err(LayoutError("a library twice"));
        }
        Ok(Namer {
            vpath_layout,
            libraries,
            device: device.clone(),
            folders: vec![Folder::new(String::new())],
            known: HashMap::new(),
            roots: HashMap::new(),
            grown: BTreeSet::new(),
            skipped: Vec::new(),
        })
    }

    /// An entry the card lists, relative to `/music` and spelled as stored
    /// ([`stored_name`]): a folder, or a file. Its folders come with it.
    pub(crate) fn seed(&mut self, rel: &str, folder: bool) {
        let names: Vec<&str> = rel.split('/').filter(|n| !n.is_empty()).collect();
        let Some((last, folders)) = names.split_last() else { return };
        let mut at = 0;
        for name in folders {
            at = self.seed_folder(at, name);
        }
        if folder {
            self.seed_folder(at, last);
        } else if self.folders[at].exact(last).is_none() {
            self.folders[at].add((*last).to_owned(), None);
        }
    }

    fn seed_folder(&mut self, at: usize, name: &str) -> usize {
        match self.folders[at].exact(name) {
            Some(&(_, Some(f))) => f,
            // Seeded as a file too: FAT holds one or the other, and the
            // folder stands, in either order (`seed` adds no file over it).
            Some(&(_, None)) => {
                let f = self.folder_below(at, name);
                self.folders[at].make_folder(name, f);
                f
            }
            None => self.new_folder(at, name.to_owned()),
        }
    }

    /// A ledger row the run keeps: its file on the card (`rel`, as stored)
    /// and its server's path for it. Seeded, and its folders' names
    /// learned when its path has the folder levels the rules give the
    /// server's (the listener may have moved it), so the server's next
    /// files in those folders go where it is.
    pub(crate) fn seed_kept(&mut self, rel: &str, server: u16, server_path: &str) {
        self.seed(rel, false);
        let Some((vpath, folders, _)) = self.split_server(server_path) else { return };
        let card: Vec<&str> = rel.split('/').filter(|n| !n.is_empty()).collect();
        let mut card = card[..card.len().saturating_sub(1)].iter();
        let mut at = 0;
        if self.vpath_layout {
            let library = (server, vpath.to_owned());
            if !self.libraries.contains(&library) || card.len() != folders.len() + 1 {
                return;
            }
            let Some(root) = card.next().and_then(|n| self.child_folder(0, n)) else { return };
            match self.roots.get(&library) {
                Some(&r) if r != root => return,
                Some(_) => {}
                None if self.roots.values().any(|&r| r == root) => return,
                None => {
                    self.roots.insert(library, root);
                }
            }
            at = root;
        } else if card.len() != folders.len() {
            return;
        }
        for (clean, name) in folders.iter().zip(card) {
            let Some(f) = self.child_folder(at, name) else { return };
            self.known.entry((at, match_key(clean))).or_insert(f);
            at = f;
        }
    }

    fn child_folder(&self, at: usize, name: &str) -> Option<usize> {
        self.folders[at].exact(name).and_then(|e| e.1)
    }

    fn new_folder(&mut self, parent: usize, name: String) -> usize {
        let at = self.folder_below(parent, &name);
        self.folders[parent].add(name, Some(at));
        at
    }

    /// A folder named `name` in `parent`, not yet among its entries.
    fn folder_below(&mut self, parent: usize, name: &str) -> usize {
        let above = &self.folders[parent].path;
        let path = if above.is_empty() { name.to_owned() } else { format!("{above}/{name}") };
        self.folders.push(Folder::new(path));
        self.folders.len() - 1
    }

    /// mStream's path in the rules' terms (steps 1, 2 and 4): its vpath, its
    /// folders as clean names joined past the 8th level (a root counts as
    /// one), and the file's clean name. None with no name below the vpath.
    fn split_server<'p>(&self, server_path: &'p str) -> Option<(&'p str, Vec<String>, String)> {
        let mut components = server_path.split('/');
        let vpath = components.next()?;
        let rest: Vec<&str> = components.collect();
        let (file, folders) = rest.split_last()?;
        let mut folders: Vec<String> = folders.iter().map(|c| clean_name(c)).collect();
        let levels = MAX_DEPTH - usize::from(self.vpath_layout);
        if folders.len() > levels {
            let deep = folders.split_off(levels - 1).join(JOIN);
            folders.push(deep);
        }
        Some((vpath, folders, clean_name(file)))
    }

    /// The folder of `parent` a server folder of this clean name goes in:
    /// one this naming or a kept row gave it, else the card's of its key.
    fn existing(&self, parent: usize, clean: &str) -> Option<usize> {
        let key = match_key(clean);
        let known = self.known.get(&(parent, key.clone())).copied();
        known.or_else(|| self.folders[parent].merge_with(&key, clean))
    }

    /// Gives each library of the vpath layout its root ([`root_name`]): the
    /// card's own folder of that name when there is one that isn't
    /// another's root (they merge, and its spelling stays), else a new
    /// folder, ` (2)` after the name when its key is taken.
    fn give_roots(&mut self) {
        for k in 0..self.libraries.len() {
            if self.roots.contains_key(&self.libraries[k]) {
                continue;
            }
            let name = root_name(&self.libraries[k].1);
            let taken: Vec<usize> = self.roots.values().copied().collect();
            let theirs = self.folders[0].entries.get(&match_key(&name)).into_iter().flatten();
            let free = theirs.filter_map(|(n, f)| f.filter(|f| !taken.contains(f)).map(|f| (n, f)));
            let root = match free.min_by_key(|&(n, _)| (n != &name, n)).map(|(_, f)| f) {
                Some(f) => f,
                None => {
                    let name = self.free_name(0, &name);
                    debug_assert!(valid_rel_path(name.as_bytes()), "{name:?}");
                    self.grown.insert(0);
                    self.new_folder(0, name)
                }
            };
            self.roots.insert(self.libraries[k].clone(), root);
        }
    }

    /// `name`, else `name (2)`, `name (3)`…: the first whose key no entry
    /// of the folder has.
    fn free_name(&self, at: usize, name: &str) -> String {
        let mut n = 1;
        loop {
            let candidate = if n == 1 { name.to_owned() } else { format!("{name} ({n})") };
            if !self.folders[at].entries.contains_key(&match_key(&candidate)) {
                return candidate;
            }
            n += 1;
        }
    }

    /// The vpath layout's roots for LIBR (2.8.6), relative to `/music`, by
    /// their bytes: each library's folder once it has one.
    pub(crate) fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.roots.values().map(|&f| self.folders[f].path.clone()).collect();
        roots.sort();
        roots
    }

    /// Names `files`, each a path relative to `/music` or why it gets none,
    /// in the order given. They are named by (server, mStream id), so of two
    /// that collide the lower id keeps the name (2.8.3, step 6: of two
    /// servers' files, the lower server index first, since ids are each
    /// server's own); the others take ` (2)`, ` (3)`… before the extension,
    /// as they do beside a file the card already has.
    pub(crate) fn place(&mut self, files: &[Wanted<'_>]) -> Vec<Result<String, Refused>> {
        self.give_roots();
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by_key(|&i| (files[i].server, files[i].id));
        let mut out = vec![Err(Refused::NoName); files.len()];
        for i in order {
            let w = &files[i];
            out[i] = self.place_one(w);
            if let Err(why) = &out[i] {
                let server_path = w.server_path.to_owned();
                self.skipped.push(Skipped { server: w.server, server_path, size: w.size, why: *why });
            }
        }
        out
    }

    fn place_one(&mut self, w: &Wanted<'_>) -> Result<String, Refused> {
        if !fits_fat32(w.size) {
            return Err(Refused::TooBig);
        }
        let (vpath, folders, file) = self.split_server(w.server_path).ok_or(Refused::NoName)?;
        let mut at = match self.vpath_layout {
            false => 0,
            true => *self.roots.get(&(w.server, vpath.to_owned())).ok_or(Refused::NoRoot)?,
        };
        let (stem, ext) = self.extension(&file, w.bytes)?;
        // The folders the card has, or this naming or a kept row gave: their
        // names stand. The rest are new, and each may be cut.
        let mut rest = folders.as_slice();
        while let Some(f) = rest.first().and_then(|clean| self.existing(at, clean)) {
            at = f;
            rest = &rest[1..];
        }
        let mut parts: Vec<Part> = rest.iter().map(|c| Part::new(c.clone(), String::new())).collect();
        parts.push(Part::new(stem, ext));
        // Steps 5 and 6: fit the path, then settle its first new name in a
        // folder the card has (the names below it are in new folders).
        loop {
            if !fit(self.folders[at].path.len(), &mut parts) {
                return Err(Refused::TooLong);
            }
            let name = parts[0].name();
            let key = match_key(&name);
            if parts.len() > 1 {
                if let Some(f) = self.folders[at].merge_with(&key, &name) {
                    // The name it was cut to is a folder's already: they merge.
                    let part = parts.remove(0);
                    self.known.entry((at, match_key(&part.body))).or_insert(f);
                    at = f;
                    continue;
                }
            }
            if self.folders[at].entries.contains_key(&key) {
                parts[0].bump();
                continue;
            }
            break;
        }
        if self.folders[at].dir_entries + dir_entries(&parts[0].name()) > MAX_DIR_ENTRIES {
            return Err(Refused::FolderFull);
        }
        self.grown.insert(at);
        let (last, new) = parts.split_last().expect("the file's part");
        for part in new {
            let f = self.new_folder(at, part.name());
            self.known.insert((at, match_key(&part.body)), f);
            at = f;
            self.grown.insert(at);
        }
        let name = last.name();
        let above = &self.folders[at].path;
        let path = if above.is_empty() { name.clone() } else { format!("{above}/{name}") };
        self.folders[at].add(name, None);
        Ok(path)
    }

    /// Step 3: the file's stem and its extension on the card (with its
    /// dot), or [`Refused::Unlisted`].
    fn extension(&self, file: &str, bytes: Bytes) -> Result<(String, String), Refused> {
        // A clean name neither starts nor ends with a dot.
        let (stem, ext) = match file.rfind('.') {
            Some(k) => (&file[..k], Some(&file[k + 1..])),
            None => (file, None),
        };
        let ogg = ext.is_some_and(|e| e.eq_ignore_ascii_case("ogg") || e.eq_ignore_ascii_case("oga"));
        let ext = match bytes {
            Bytes::NotAudio => return Ok((stem.to_owned(), ext.map_or(String::new(), |e| format!(".{e}")))),
            Bytes::Original { ogg_opus: true } if ogg => "opus",
            Bytes::Original { .. } => ext.ok_or(Refused::Unlisted)?,
            Bytes::Converted(container) => container_extension(container).ok_or(Refused::Unlisted)?,
        };
        if !self.device.lists(ext) {
            return Err(Refused::Unlisted);
        }
        Ok((stem.to_owned(), format!(".{ext}")))
    }

    /// The report: 2.8.4's big folders among those the naming added to, and
    /// every file refused so far.
    pub(crate) fn report(&self) -> Report {
        let grown = self.grown.iter().map(|&f| &self.folders[f]);
        let mut big_folders: Vec<(String, usize)> =
            grown.filter(|f| f.count > BIG_FOLDER).map(|f| (f.path.clone(), f.count)).collect();
        big_folders.sort();
        Report { big_folders, skipped: self.skipped.clone() }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use super::*;
    use crate::device::card::contract::order::valid_name;
    use crate::device::card::contract::prim::path_hash;

    fn fixture(rel: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card").join(rel);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    fn json(rel: &str) -> Value {
        serde_json::from_slice(&fixture(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    /// The fixture card's `device.txt`: mp3, flac and opus.
    fn device() -> DeviceTxt {
        DeviceTxt::read(Some(&fixture("golden/device.txt")))
    }

    fn namer() -> Namer {
        Namer::new(Layout::Default, &device()).expect("the default layout")
    }

    fn track(server: u16, id: u32, server_path: &str) -> Wanted<'_> {
        Wanted { server, id, server_path, size: 4_000_000, bytes: Bytes::Original { ogg_opus: false } }
    }

    fn one(n: &mut Namer, w: Wanted<'_>) -> Result<String, Refused> {
        n.place(&[w]).remove(0)
    }

    fn ok(path: &str) -> Result<String, Refused> {
        Ok(path.to_owned())
    }

    fn mark(original: &str) -> String {
        format!("~{:04X}", fnv1a64(original.as_bytes()) & 0xFFFF)
    }

    /// A vector run's file, as `Wanted` borrows it.
    fn wanted(f: &Value) -> Wanted<'_> {
        let bytes = match f["bytes"].as_str().unwrap_or("original") {
            "original" => Bytes::Original { ogg_opus: false },
            "oggOpus" => Bytes::Original { ogg_opus: true },
            "notAudio" => Bytes::NotAudio,
            "mp3" => Bytes::Converted(CONTAINER_MP3),
            "flac" => Bytes::Converted(CONTAINER_FLAC),
            "opus" => Bytes::Converted(CONTAINER_OPUS),
            other => panic!("bytes: {other}"),
        };
        Wanted {
            server: f["server"].as_u64().expect("a server") as u16,
            id: f["id"].as_u64().expect("an id") as u32,
            server_path: f["serverPath"].as_str().expect("a server path"),
            size: f["size"].as_u64().unwrap_or(4_000_000),
            bytes,
        }
    }

    fn refusal(code: &str) -> Refused {
        match code {
            "noName" => Refused::NoName,
            "noRoot" => Refused::NoRoot,
            "unlisted" => Refused::Unlisted,
            "tooBig" => Refused::TooBig,
            "tooLong" => Refused::TooLong,
            "folderFull" => Refused::FolderFull,
            other => panic!("a refusal: {other}"),
        }
    }

    /// The 16-digit hex token in a vector's prose, as an integer.
    fn hash_in(prose: &str) -> u64 {
        let token = prose.split(|c: char| !c.is_ascii_hexdigit()).find(|t| t.len() == 16).expect("a hash");
        u64::from_str_radix(token, 16).expect("hex")
    }

    /// The fixtures' `names` vectors, made by the firmware repo's
    /// tools/cardnames.py: each run on its card names its files as there.
    #[test]
    fn the_fixtures_name_vectors() {
        let v = json("vectors.json");
        let names = &v["names"];
        assert_eq!(names["who"], "software");
        for m in names["marks"].as_array().expect("marks") {
            let component = m["component"].as_str().expect("a component");
            assert_eq!(cut_mark(component), m["mark"].as_str().expect("a mark"), "{}", m["name"]);
            let name = m["name"].as_str().expect("a name");
            let part = Part::new(component.to_owned(), name[component.len()..].to_owned());
            assert_eq!(part.mark, cut_mark(component), "a file's mark is its stem's");
        }
        let runs = names["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 12);
        for run in runs {
            let note = run["note"].as_str().expect("a note");
            let layout = match &run["layout"] {
                Value::String(s) if s == "default" => Layout::Default,
                l => {
                    let lib = |lib: &Value| {
                        let server = lib["server"].as_u64().expect("a server") as u16;
                        (server, lib["vpath"].as_str().expect("a vpath").to_owned())
                    };
                    Layout::Vpath(l["vpath"].as_array().expect("libraries").iter().map(lib).collect())
                }
            };
            let extensions = run["extensions"].as_str().expect("extensions");
            let device = DeviceTxt::read(Some(format!("contract=1\nextensions={extensions}\n").as_bytes()));
            let mut n = Namer::new(layout, &device).expect(note);
            for e in run["card"].as_array().into_iter().flatten() {
                n.seed(e["path"].as_str().expect("a path"), e["folder"].as_bool().unwrap_or(false));
            }
            let files: Vec<Wanted<'_>> = run["files"].as_array().expect("files").iter().map(wanted).collect();
            let want: Vec<Result<String, Refused>> = run["names"]
                .as_array()
                .expect("names")
                .iter()
                .map(|w| match w.as_str() {
                    Some(path) => Ok(path.to_owned()),
                    None => Err(refusal(w["refused"].as_str().expect("a refusal"))),
                })
                .collect();
            assert_eq!(n.place(&files), want, "{note}");
        }
    }

    #[test]
    fn each_name_is_made_safe_by_step_2() {
        let cases = [
            // 1. NFC: `e` and U+0301 compose to U+00E9.
            ("Cafe\u{301}", "Caf\u{E9}"),
            // 2. Control characters and FAT's illegal ones; U+0080-U+009F stay.
            ("a\"b*c:d<e>f?g\\h|i", "a_b_c_d_e_f_g_h_i"),
            ("tab\there\u{1}\u{1F}\u{7F}", "tab_here___"),
            ("\u{80}\u{9F} kept", "\u{80}\u{9F} kept"),
            // 3. Leading spaces, trailing spaces and dots.
            ("  Name . . ", "Name"),
            ("Trailing.", "Trailing"),
            ("Mid.dle. Dots", "Mid.dle. Dots"),
            // 4. Leading dots, each one.
            (".hidden", "_hidden"),
            ("..two", "__two"),
            ("  .after spaces", "_after spaces"),
            // 5. Nothing left.
            ("", "_"),
            ("...", "_"),
            ("   ", "_"),
            // 6. Windows' device names, any case, with or without an extension.
            ("CON", "CON_"),
            ("con.mp3", "con_.mp3"),
            ("Aux.Live.flac", "Aux_.Live.flac"),
            ("nul", "nul_"),
            ("LPT9.opus", "LPT9_.opus"),
            ("com1", "com1_"),
            ("CON .mp3", "CON_ .mp3"),
            ("COM0.mp3", "COM0.mp3"),
            ("COM10.mp3", "COM10.mp3"),
            ("CONSOLE.mp3", "CONSOLE.mp3"),
            (".con", "_con"),
        ];
        for (raw, want) in cases {
            let got = clean_name(raw);
            assert_eq!(got, want, "{raw:?}");
            assert!(valid_name(got.as_bytes()), "{raw:?}");
            assert_eq!(clean_name(&got), got, "cleaned twice: {raw:?}");
        }
    }

    #[test]
    fn the_match_key_is_nfc_then_simple_upper_case() {
        let alike = [
            ("Acme", "ACME"),
            ("Caf\u{E9}", "Cafe\u{301}"),
            ("CAF\u{C9}", "cafe\u{301}"),
            ("x.MP3", "X.mp3"),
            // Small sigma and final sigma: both capital sigma.
            ("\u{3C3}", "\u{3C2}"),
            // dz with caron, and its title case: both the capital.
            ("\u{1C6}", "\u{1C5}"),
            // Alpha with ypogegrammeni: its simple upper case has prosgegrammeni.
            ("\u{1FB3}", "\u{1FBC}"),
            // Dotless i and long s upper-case to I and S.
            ("\u{131}", "i"),
            ("\u{17F}", "s"),
            // NFC makes the Kelvin sign a K.
            ("\u{212A}", "k"),
        ];
        for (a, b) in alike {
            assert_eq!(match_key(a), match_key(b), "{a:?} {b:?}");
        }
        let apart = [
            // Sharp s and the ligature fi have no simple upper case.
            ("\u{DF}", "SS"),
            ("\u{FB01}", "FI"),
            // Capital I with a dot is its own upper case.
            ("\u{130}", "i"),
            ("A", "\u{391}"),
        ];
        for (a, b) in apart {
            assert_ne!(match_key(a), match_key(b), "{a:?} {b:?}");
        }
    }

    #[test]
    fn the_simple_upper_case_departs_from_std_only_where_std_gives_several() {
        // SpecialCasing.txt's upper cases of several code points, as std's
        // Unicode 17 has them: a toolchain whose Unicode adds one fails
        // here, on purpose, before a name is keyed by a guess; its simple
        // mapping is to be looked up in UnicodeData.txt.
        let changed = |c: char| {
            let v = char::UNICODE_VERSION;
            format!(
                "U+{:04X}: SpecialCasing.txt changed in std's Unicode {}.{}.{}; give simple_upper \
                 this code point's simple upper case from UnicodeData.txt, and list it here",
                c as u32, v.0, v.1, v.2
            )
        };
        let several: &[(u32, u32)] = &[
            (0xDF, 0xDF), (0x149, 0x149), (0x1F0, 0x1F0), (0x390, 0x390), (0x3B0, 0x3B0),
            (0x587, 0x587), (0x1E96, 0x1E9A), (0x1F50, 0x1F50), (0x1F52, 0x1F52), (0x1F54, 0x1F54),
            (0x1F56, 0x1F56), (0x1F80, 0x1FAF), (0x1FB2, 0x1FB4), (0x1FB6, 0x1FB7), (0x1FBC, 0x1FBC),
            (0x1FC2, 0x1FC4), (0x1FC6, 0x1FC7), (0x1FCC, 0x1FCC), (0x1FD2, 0x1FD3), (0x1FD6, 0x1FD7),
            (0x1FE2, 0x1FE4), (0x1FE6, 0x1FE7), (0x1FF2, 0x1FF4), (0x1FF6, 0x1FF7), (0x1FFC, 0x1FFC),
            (0xFB00, 0xFB06), (0xFB13, 0xFB17),
        ];
        let mut departures = 0;
        for c in (0..=0x10FFFF).filter_map(char::from_u32) {
            let full: Vec<char> = c.to_uppercase().collect();
            let listed = several.iter().any(|&(a, b)| (a..=b).contains(&(c as u32)));
            assert_eq!(full.len() > 1, listed, "{}", changed(c));
            let up = simple_upper(c);
            if full.len() == 1 {
                assert_eq!(up, full[0], "U+{:04X}", c as u32);
            } else if up != c {
                // The title case form, whose lower case is the code point.
                assert_eq!(up.to_lowercase().collect::<Vec<_>>(), [c], "U+{:04X}", c as u32);
                departures += 1;
            }
        }
        assert_eq!(departures, 27, "of std's several, those whose simple upper case is another");
    }

    #[test]
    fn a_mac_listing_is_brought_back_to_nfc() {
        // vectors.json's softwareMatching: on macOS, a listed 'Cafe' +
        // U+0301 is recorded in NFC (C3 A9), with its hash.
        let v = json("vectors.json");
        let cases = v["softwareMatching"]["cases"].as_array().expect("cases");
        let case = cases.iter().filter_map(Value::as_str).find(|c| c.contains("macOS")).expect("the case");
        let listed = "Cafe\u{301}";
        let stored = stored_name(listed, Listing::Decomposed);
        assert_eq!(stored.as_bytes(), b"Caf\xC3\xA9");
        assert_eq!(path_hash(&format!("{stored}/Album/01 - Title.mp3")), hash_in(case));
        assert_eq!(stored_name(listed, Listing::AsStored), listed);
    }

    #[test]
    fn the_extension_is_one_the_device_lists() {
        let original = Bytes::Original { ogg_opus: false };
        let ogg_opus = Bytes::Original { ogg_opus: true };
        let ours = device();
        // A card with no device.txt: mp3 and flac only.
        let none = DeviceTxt::defaults();
        let cases: &[(&DeviceTxt, &str, Bytes, Result<&str, Refused>)] = &[
            (&ours, "V/A/x.MP3", original, Ok("A/x.MP3")),
            (&ours, "V/A/x.Flac", original, Ok("A/x.Flac")),
            (&ours, "V/A/x.opus", original, Ok("A/x.opus")),
            (&none, "V/A/x.opus", original, Err(Refused::Unlisted)),
            // vectors.json's softwareMatching: an Opus stream the server
            // stores as x.ogg goes on the card as x.opus, its bytes unchanged.
            (&ours, "V/A/x.ogg", ogg_opus, Ok("A/x.opus")),
            (&ours, "V/A/x.OGA", ogg_opus, Ok("A/x.opus")),
            (&none, "V/A/x.ogg", ogg_opus, Err(Refused::Unlisted)),
            // An Ogg that isn't Opus, and what the device doesn't list.
            (&ours, "V/A/x.ogg", original, Err(Refused::Unlisted)),
            (&ours, "V/A/x.wav", original, Err(Refused::Unlisted)),
            (&ours, "V/A/x.m4a", original, Err(Refused::Unlisted)),
            (&ours, "V/A/plain", original, Err(Refused::Unlisted)),
            // A dot name is `_flac` before its extension is looked at.
            (&ours, "V/A/.flac", original, Err(Refused::Unlisted)),
            // A conversion takes its format's extension.
            (&ours, "V/A/x.m4a", Bytes::Converted(CONTAINER_OPUS), Ok("A/x.opus")),
            (&ours, "V/A/x.wav", Bytes::Converted(CONTAINER_MP3), Ok("A/x.mp3")),
            (&ours, "V/A/plain", Bytes::Converted(CONTAINER_FLAC), Ok("A/plain.flac")),
            (&none, "V/A/x.wav", Bytes::Converted(CONTAINER_OPUS), Err(Refused::Unlisted)),
            (&ours, "V/A/x.wav", Bytes::Converted(0), Err(Refused::Unlisted)),
            (&none, "V/A/con.flac", Bytes::Converted(CONTAINER_MP3), Ok("A/con_.mp3")),
            // Not audio: as it is.
            (&ours, "V/A/cover.jpg", Bytes::NotAudio, Ok("A/cover.jpg")),
            (&ours, "V/A/notes", Bytes::NotAudio, Ok("A/notes")),
        ];
        for &(dev, path, bytes, want) in cases {
            let mut n = Namer::new(Layout::Default, dev).expect("a layout");
            let got = one(&mut n, Wanted { bytes, ..track(0, 1, path) });
            assert_eq!(got, want.map(str::to_owned), "{path} {bytes:?}");
        }
    }

    #[test]
    fn folders_past_the_eighth_level_are_joined() {
        let deep = "V/1/2/3/4/5/6/7/8/9/10/x.mp3";
        assert_eq!(one(&mut namer(), track(0, 1, deep)), ok("1/2/3/4/5/6/7/8 - 9 - 10/x.mp3"));
        let eight = "V/1/2/3/4/5/6/7/8/x.mp3";
        assert_eq!(one(&mut namer(), track(0, 1, eight)), ok("1/2/3/4/5/6/7/8/x.mp3"));
        // Each folder is cleaned before the join.
        let dotted = "V/1/2/3/4/5/6/7/8./ 9 /x.mp3";
        assert_eq!(one(&mut namer(), track(0, 1, dotted)), ok("1/2/3/4/5/6/7/8 - 9/x.mp3"));
        // Under the vpath layout the root is the first level.
        let mut n = Namer::new(Layout::Vpath(vec![(0, "V".into())]), &device()).expect("a layout");
        assert_eq!(one(&mut n, track(0, 1, deep)), ok("V/1/2/3/4/5/6/7 - 8 - 9 - 10/x.mp3"));
    }

    #[test]
    fn a_long_path_is_cut_with_a_mark() {
        // The file is the longest: its stem is cut, the extension kept, to
        // 255 bytes with /music/.
        let stem = "a".repeat(300);
        let got = one(&mut namer(), track(0, 1, &format!("V/Artist/Album/{stem}.mp3"))).expect("a name");
        let cut = format!("Artist/Album/{}{}.mp3", "a".repeat(226), mark(&stem));
        assert_eq!((got.as_str(), MUSIC + got.len()), (cut.as_str(), 255));
        // At a code point boundary: 3-byte characters, one byte short.
        let euros = "\u{20AC}".repeat(100);
        let got = one(&mut namer(), track(0, 1, &format!("V/Artist/Album/{euros}.mp3"))).expect("a name");
        let cut = format!("Artist/Album/{}{}.mp3", "\u{20AC}".repeat(75), mark(&euros));
        assert_eq!((got.as_str(), MUSIC + got.len()), (cut.as_str(), 254));
        // Ties go to the deepest: the file.
        let (folder, stem) = ("B".repeat(200), "c".repeat(200));
        let got = one(&mut namer(), track(0, 1, &format!("V/{folder}/{stem}.mp3"))).expect("a name");
        assert_eq!(got, format!("{folder}/{}{}.mp3", "c".repeat(38), mark(&stem)));
    }

    #[test]
    fn a_folder_cut_for_one_file_leaves_room_for_the_others() {
        // The album folder is the longest: cut, it leaves 64 bytes below it.
        let album = "F".repeat(250);
        let cut = format!("{}{}", "F".repeat(177), mark(&album));
        let mut n = namer();
        let first = one(&mut n, track(0, 1, &format!("V/A/{album}/x.mp3")));
        assert_eq!(first, Ok(format!("A/{cut}/x.mp3")));
        assert_eq!(MUSIC + "A/".len() + cut.len(), 255 - 64);
        // A sibling of 63 bytes fits whole; a longer one is cut itself.
        let fits = format!("{}.mp3", "s".repeat(59));
        assert_eq!(one(&mut n, track(0, 2, &format!("V/A/{album}/{fits}"))), Ok(format!("A/{cut}/{fits}")));
        let long = "t".repeat(100);
        let got = one(&mut n, track(0, 3, &format!("V/A/{album}/{long}.mp3"))).expect("a name");
        assert_eq!(got, format!("A/{cut}/{}{}.mp3", "t".repeat(54), mark(&long)));
        assert_eq!(MUSIC + got.len(), 255);
        // A folder the card has that leaves no room: refused, not misnamed.
        let mut n = namer();
        let full = "G".repeat(247);
        n.seed(&full, true);
        assert_eq!(one(&mut n, track(0, 1, &format!("V/{full}/x.mp3"))), Err(Refused::TooLong));
    }

    #[test]
    fn a_cut_folder_is_named_alike_whatever_its_first_file() {
        // A run with no kept row from the last may first name another of the
        // album's files: the folder ends 64 bytes short of 255 whichever, so
        // a later run's files join the folder an earlier one cut.
        let album = "F".repeat(250);
        let cut = format!("{}{}", "F".repeat(177), mark(&album));
        let long = "t".repeat(200);
        let long_cut = format!("{}{}.mp3", "t".repeat(54), mark(&long));
        let firsts = [
            ("x.mp3".to_owned(), "x.mp3".to_owned()),
            (format!("{}.mp3", "s".repeat(59)), format!("{}.mp3", "s".repeat(59))),
            // Longer than the folder once cut, the file is the next cut.
            (format!("{long}.mp3"), long_cut.clone()),
        ];
        for (file, want) in &firsts {
            let got = one(&mut namer(), track(0, 1, &format!("V/A/{album}/{file}")));
            assert_eq!(got, Ok(format!("A/{cut}/{want}")), "{file}");
        }
        let mut n = namer();
        n.seed(&format!("A/{cut}/x.mp3"), false);
        let got = one(&mut n, track(0, 2, &format!("V/A/{album}/{long}.mp3")));
        assert_eq!(got, Ok(format!("A/{cut}/{long_cut}")));
        // A folder still the longest name at that is cut again, as far as
        // its file needs (step 5): only a kept row then keeps its name.
        let file = format!("{}.mp3", "u".repeat(80));
        let got = one(&mut namer(), track(0, 1, &format!("V/A/{album}/{file}")));
        assert_eq!(got, Ok(format!("A/{}{}/{file}", "F".repeat(156), mark(&album))));
    }

    #[test]
    fn of_two_files_alike_the_lower_id_keeps_the_name() {
        let mut n = namer();
        let files = [track(0, 7, "V/A/One.mp3"), track(0, 3, "V/A/one.MP3"), track(0, 5, "W/a/ONE.mp3")];
        let got = n.place(&files);
        assert_eq!(got, [ok("A/One (3).mp3"), ok("A/one.MP3"), ok("A/ONE (2).mp3")]);
        // Of two servers' files, the lower server first: ids are each one's own.
        let got = n.place(&[track(1, 1, "V/B/x.mp3"), track(0, 9, "V/B/x.mp3")]);
        assert_eq!(got, [ok("B/x (2).mp3"), ok("B/x.mp3")]);
        // An NFC and an NFD spelling are one name.
        let got = n.place(&[track(0, 2, "V/Cafe\u{301}.mp3"), track(0, 1, "V/Caf\u{E9}.mp3")]);
        assert_eq!(got, [ok("Caf\u{E9} (2).mp3"), ok("Caf\u{E9}.mp3")]);
        // A file the card has already, the listener's: the new one moves aside.
        n.seed("Mine/Song.mp3", false);
        assert_eq!(one(&mut n, track(0, 1, "V/Mine/song.mp3")), ok("Mine/song (2).mp3"));
        // A file named like a folder, and a folder named like a file.
        n.seed("Mine/Live.mp3", true);
        assert_eq!(one(&mut n, track(0, 1, "V/Mine/Live.mp3")), ok("Mine/Live (2).mp3"));
        n.seed("Intro", false);
        assert_eq!(one(&mut n, track(0, 1, "V/Intro/x.mp3")), ok("Intro (2)/x.mp3"));
        assert_eq!(one(&mut n, track(0, 2, "V/Intro/y.mp3")), ok("Intro (2)/y.mp3"));
    }

    #[test]
    fn folders_merge_and_keep_the_cards_spelling() {
        // vectors.json's softwareMatching: the card lists Acme/Hits/01.mp3
        // where the server says ACME/Hits/01.mp3. To FAT they are one, so
        // the album's new files go beside it.
        let mut n = namer();
        n.seed("Acme/Hits/01.mp3", false);
        assert_eq!(match_key("ACME"), match_key("Acme"));
        assert_eq!(one(&mut n, track(0, 2, "Music/ACME/Hits/02.mp3")), ok("Acme/Hits/02.mp3"));
        // Under the default layout two libraries' folders merge.
        assert_eq!(one(&mut n, track(0, 3, "Other/acme/HITS/03.mp3")), ok("Acme/Hits/03.mp3"));
        // Of two folders alike on the card (an NFD one some other system
        // made), the one spelled alike, else the first by bytes.
        let mut n = namer();
        n.seed("Cafe\u{301}/a.mp3", false);
        assert_eq!(one(&mut n, track(0, 1, "V/Caf\u{E9}/b.mp3")), ok("Cafe\u{301}/b.mp3"));
        n.seed("Caf\u{E9}/c.mp3", false);
        assert_eq!(one(&mut n, track(0, 2, "V/Caf\u{E9}/d.mp3")), ok("Caf\u{E9}/d.mp3"));
        assert_eq!(one(&mut n, track(0, 3, "V/CAF\u{C9}/e.mp3")), ok("Cafe\u{301}/e.mp3"));
    }

    #[test]
    fn kept_rows_say_where_their_folders_went() {
        let mut n = namer();
        // The listener renamed the album's folder, and the ledger followed
        // (2.10.2): the album's next file goes there too.
        n.seed_kept("A/Album (2019)/01.mp3", 0, "Music/A/Album/01.mp3");
        assert_eq!(one(&mut n, track(0, 9, "Music/A/Album/02.mp3")), ok("A/Album (2019)/02.mp3"));
        // A folder an earlier run cut keeps that cut.
        let album = "F".repeat(250);
        let cut = format!("{}{}", "F".repeat(100), mark(&album));
        n.seed_kept(&format!("B/{cut}/x.mp3"), 0, &format!("Music/B/{album}/x.mp3"));
        let got = one(&mut n, track(0, 10, &format!("Music/B/{album}/y.mp3")));
        assert_eq!(got, Ok(format!("B/{cut}/y.mp3")));
        // A row at another depth than its server path's says nothing.
        n.seed_kept("Loose.mp3", 0, "Music/C/D/Loose.mp3");
        assert_eq!(one(&mut n, track(0, 11, "Music/C/D/z.mp3")), ok("C/D/z.mp3"));
    }

    #[test]
    fn a_name_seeded_as_a_file_and_a_folder_is_the_folder() {
        // FAT holds one or the other, but a listing and the kept rows may
        // say both, in either order: one entry, a folder.
        let mut n = namer();
        n.seed("X", false);
        n.seed("X/a.mp3", false);
        n.seed("X/b.mp3", false);
        n.seed("Y", true);
        n.seed("Y", false);
        let root = &n.folders[0];
        assert_eq!((root.count, root.dir_entries), (2, 2 + dir_entries("X") + dir_entries("Y")));
        assert_eq!(n.folders.len(), 3);
        assert_eq!(n.folders[n.child_folder(0, "X").expect("a folder")].count, 2);
        assert_eq!(one(&mut n, track(0, 1, "V/X/c.mp3")), ok("X/c.mp3"));
        assert_eq!(one(&mut n, track(0, 2, "V/Y/c.mp3")), ok("Y/c.mp3"));
    }

    #[test]
    fn the_vpath_layout_keeps_a_root_for_each_library() {
        let libraries = ["Music", "Music", "Audio Books", "a:b"].iter().zip([1, 0, 0, 0]);
        let libraries = libraries.map(|(vpath, server)| (server, vpath.to_string())).collect();
        let mut n = Namer::new(Layout::Vpath(libraries), &device()).expect("four roots");
        // The listener's own folder of a library's name becomes its root.
        n.seed("music/mine.mp3", false);
        let files = [
            track(0, 1, "Music/A/x.mp3"),
            track(1, 1, "Music/A/x.mp3"),
            track(0, 2, "Audio Books/B/y.mp3"),
            track(0, 3, "a:b/z.mp3"),
            track(2, 1, "Music/A/x.mp3"),
            track(0, 4, "Other/x.mp3"),
            track(0, 5, "Music"),
        ];
        let got = n.place(&files);
        let want = [
            ok("music/A/x.mp3"),
            ok("Music (2)/A/x.mp3"),
            ok("Audio Books/B/y.mp3"),
            ok("a_b/z.mp3"),
            Err(Refused::NoRoot),
            Err(Refused::NoRoot),
            Err(Refused::NoName),
        ];
        assert_eq!(got, want);
        assert_eq!(n.roots(), ["Audio Books", "Music (2)", "a_b", "music"]);
        // A kept row says which folder is a library's root.
        let two = Layout::Vpath(vec![(0, "V".into()), (1, "V".into())]);
        let mut n = Namer::new(two, &device()).expect("two roots");
        n.seed_kept("V (2)/A/x.mp3", 0, "V/A/x.mp3");
        let got = n.place(&[track(0, 2, "V/A/y.mp3"), track(1, 1, "V/A/y.mp3")]);
        assert_eq!(got, [ok("V (2)/A/y.mp3"), ok("V/A/y.mp3")]);
        // The device keeps 16 roots; a library is listed once.
        let many = (0..=MAX_ROOTS).map(|i| (0, format!("V{i}"))).collect();
        assert!(Namer::new(Layout::Vpath(many), &device()).is_err());
        assert!(Namer::new(Layout::Vpath(vec![(0, "V".into()), (0, "V".into())]), &device()).is_err());
    }

    #[test]
    fn a_vpath_too_long_for_a_root_is_cut_like_a_folder() {
        let long = "L".repeat(300);
        let root = format!("{}{}", "L".repeat(179), mark(&long));
        assert_eq!(MUSIC + root.len(), 255 - ROOM_BELOW_CUT);
        let layout = Layout::Vpath(vec![(0, long.clone())]);
        let mut n = Namer::new(layout.clone(), &device()).expect("a layout");
        assert_eq!(one(&mut n, track(0, 1, &format!("{long}/A/x.mp3"))), Ok(format!("{root}/A/x.mp3")));
        assert_eq!(n.roots(), [root.clone()]);
        // A later run finds it, with no kept row to say so.
        let mut n = Namer::new(layout, &device()).expect("a layout");
        n.seed(&format!("{root}/A/x.mp3"), false);
        assert_eq!(one(&mut n, track(0, 2, &format!("{long}/A/y.mp3"))), Ok(format!("{root}/A/y.mp3")));
        assert_eq!(n.roots(), [root]);
    }

    #[test]
    fn the_fixture_cards_paths_are_the_rules_but_where_made_by_hand() {
        // The fixture libraries' files that have a server path, named afresh
        // on an empty card in one run, get the paths the fixtures give them
        // — but for two the fixture made by hand, which show why the ledger,
        // not the rules, is authoritative for a file already on a card.
        let by_hand: &[(&str, &str)] = &[
            // Folder `A` (id 106) came first: the folders merge.
            ("a/v.mp3", "A/v.mp3"),
            // The NFD spelling is made NFC, and that is id 112's name.
            ("Cafe\u{301}/Album/01 - Title.mp3", "Caf\u{E9}/Album/01 - Title (2).mp3"),
        ];
        for library in ["tags-00000007", "tags-0000002a"] {
            let d = json(&format!("libraries/{library}.json"));
            let mut rows = Vec::new();
            for r in d["records"].as_array().expect("records") {
                let l = &r["ledger"];
                let server_path = l["serverPath"].as_str().unwrap_or("");
                if server_path.is_empty() {
                    continue; // a file of the software's own
                }
                let converted = l["convertedTo"].as_u64().unwrap_or(0) as u8;
                let bytes = match converted {
                    0 => Bytes::Original { ogg_opus: false },
                    to => Bytes::Converted(to),
                };
                let w = Wanted {
                    server: l["server"].as_u64().unwrap_or(0) as u16,
                    id: l["mstreamId"].as_u64().expect("an id") as u32,
                    server_path,
                    size: r["size"].as_u64().expect("a size"),
                    bytes,
                };
                rows.push((r["path"].as_str().expect("a path"), w));
            }
            assert!(rows.len() >= 6, "{library}");
            let wanted: Vec<Wanted<'_>> = rows.iter().map(|r| r.1).collect();
            let got = namer().place(&wanted);
            for ((path, w), got) in rows.iter().zip(got) {
                let want = by_hand.iter().find(|h| h.0 == *path).map_or(*path, |h| h.1);
                assert_eq!(got.as_deref(), Ok(want), "{library}: {}", w.server_path);
            }
        }
        // The NFC name hashes as the vectors say.
        let v = json("vectors.json");
        let nfc_path = "/music/Caf\u{E9}/Album/01 - Title.mp3";
        let vector = v["fnv1a64"].as_array().expect("fnv1a64").iter().find(|e| e["utf8"] == nfc_path);
        let hex = vector.expect("the vector")["hash"].as_str().expect("hex");
        let got = one(&mut namer(), track(0, 1, "Music/Cafe\u{301}/Album/01 - Title.mp3")).expect("a name");
        assert_eq!(format!("0x{:016X}", path_hash(&got)), hex);
    }

    #[test]
    fn big_folders_and_big_files_are_reported() {
        let paths: Vec<String> = (0..=BIG_FOLDER).map(|i| format!("V/Big/{i:04}.mp3")).collect();
        let mut files: Vec<Wanted<'_>> =
            paths.iter().enumerate().map(|(i, p)| track(0, i as u32, p)).collect();
        files.push(Wanted { size: MAX_FILE_BYTES + 1, ..track(0, 9_000, "V/Small/huge.flac") });
        files.push(Wanted { size: MAX_FILE_BYTES, ..track(0, 9_001, "V/Small/just.flac") });
        let mut n = namer();
        let got = n.place(&files);
        assert_eq!(got[BIG_FOLDER + 1], Err(Refused::TooBig));
        assert_eq!(got[BIG_FOLDER + 2], ok("Small/just.flac"));
        let report = n.report();
        assert_eq!(report.big_folders, [("Big".to_owned(), BIG_FOLDER + 1)]);
        let big: Vec<(&str, u64)> = report.too_big().map(|s| (s.server_path.as_str(), s.size)).collect();
        assert_eq!(big, [("V/Small/huge.flac", MAX_FILE_BYTES + 1)]);
        assert!(fits_fat32((1 << 32) - 1) && !fits_fat32(1 << 32));
    }

    #[test]
    fn a_full_folder_takes_no_more() {
        // 1 + ⌈UTF-16 length / 13⌉ entries a name; a code point past the BMP
        // is two units.
        assert_eq!(dir_entries("x.mp3"), 2);
        assert_eq!(dir_entries(&"a".repeat(13)), 2);
        assert_eq!(dir_entries(&"a".repeat(14)), 3);
        assert_eq!(dir_entries(&"\u{1F3B5}".repeat(7)), 3);
        // Names of 26 units take 3 entries: with `.` and `..`, 21,844 of them
        // fill a folder to 65,534, and one more would pass 65,536.
        let paths: Vec<String> = (0..21_845).map(|i| format!("V/F/{i:022}.mp3")).collect();
        let files: Vec<Wanted<'_>> = paths.iter().enumerate().map(|(i, p)| track(0, i as u32, p)).collect();
        let mut n = namer();
        let got = n.place(&files);
        assert!(got[..21_844].iter().all(Result::is_ok));
        assert_eq!(got[21_844], Err(Refused::FolderFull));
        assert_eq!(n.report().skipped.len(), 1);
    }

    #[test]
    fn whatever_the_server_path_the_card_path_is_one_the_device_reads() {
        // Server paths made of awkward pieces, named in one run: every path
        // keeps to 2.8.2 and its names to 2.8.3's rules, and no two are one
        // to FAT.
        let pieces = [
            "Artist", "A\u{301}lbum", "\u{C1}lbum", "ALBUM", " lead", "trail. ", ".dot", "con", "LPT1.x",
            "a:b?c", "tab\tname", "\u{1F3B5}", "\u{3A3}\u{3C2}", "", "...", "Disc 1",
        ];
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        let mut paths = Vec::new();
        for _ in 0..600 {
            let mut path = String::from("V");
            for _ in 0..1 + next(12) {
                let piece = pieces[next(pieces.len())];
                path += "/";
                path += &piece.repeat(1 + next(3) * next(30));
            }
            path += ["x.mp3", "x.FLAC", "x.opus"][next(3)];
            paths.push(path);
        }
        let files: Vec<Wanted<'_>> = paths.iter().enumerate().map(|(i, p)| track(0, i as u32, p)).collect();
        let got = namer().place(&files);
        let mut keys = BTreeSet::new();
        for (path, got) in paths.iter().zip(&got) {
            let rel = got.as_ref().unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
            assert!(valid_rel_path(rel.as_bytes()) && MUSIC + rel.len() <= MAX_CARD_PATH, "{rel:?}");
            let names: Vec<&str> = rel.split('/').collect();
            assert!(names.len() <= MAX_DEPTH + 1, "{rel:?}");
            for name in names {
                assert!(!name.starts_with(['.', ' ']) && !name.ends_with(['.', ' ']), "{rel:?}");
                assert!(!name.chars().any(illegal) && device_name_end(name).is_none(), "{rel:?}");
                assert!(utf16_len(name) <= MAX_NAME_UNITS, "{rel:?}");
            }
            assert!(keys.insert(match_key(rel)), "twice: {rel:?}");
        }
    }
}
