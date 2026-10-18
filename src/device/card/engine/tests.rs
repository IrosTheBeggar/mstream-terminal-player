//! The engine on a FAT card in a host folder ([`FatSim`]), fed by a server
//! of canned files (the tag reader's parity corpus, so every record is a
//! real reading), and held to what docs/METADATA.md 2.12 says a card may be
//! left as: every cut leaves a card the kit's reader takes as a commit, and
//! the next run finishes the plan; a swapped card is never written; two
//! runs never hold one card.
//!
//! Golden card images (test/golden/card-engine/*.txt) list every file of a
//! card after a run: its path, size, stamp and the FNV-1a 64 of its bytes.
//! `MSTREAM_BLESS=1` writes them again; `MSTREAM_CARD_DUMP=<folder>` copies
//! the cards the tests make there, for the firmware repo's reference
//! readers (tools/synthtransfer.py --check).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use super::fatsim::{Cut, FatSim};
use super::*;
use crate::device::card::autodj::{self, FileIn, Index, IndexIn, Model, Selection, Work};
use crate::device::card::contract::fat::fat_wall;
use crate::device::card::contract::mptg::{
    self, ADOPTED, BPM_ANALYSED, CONTAINER_OPUS, FOLDER_OWNED, FOLDER_THUMB, VERIFIED,
};
use crate::device::card::contract::mspd::{self, OP_FOLDER, OP_WRITE, Op, PlanIn};
use crate::device::card::contract::prim::{md5, path_hash};
use crate::device::card::contract::server::{NO_INSTANCE, server_url_key};
use crate::device::card::volume::{self, Inspection, Serial};

// ── The scene ───────────────────────────────────────────────────────────────

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A folder of its own under the system's temp folder, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("mstream-engine-{pid}-{name}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures").join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn corpus(name: &str) -> Vec<u8> {
    fixture(&format!("tags/{name}"))
}

/// The stamp the listener's own writes get.
const LATER: u32 = 0x5D48_0000;

/// A host file of a card folder, written with a FAT stamp.
fn put(root: &Path, rel: &str, bytes: &[u8], fat_time: u32) {
    let path = root.join(rel.trim_start_matches('/'));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let wall = fat_wall(fat_time).expect("a stamp");
    let at = UNIX_EPOCH + Duration::from_secs((wall + 315_532_800) as u64);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
}

/// A whole folder copied, stamps and all (a card's tree, for a run that
/// may be cut).
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
            let stamp = entry.metadata().unwrap().modified().unwrap();
            std::fs::File::options().write(true).open(&target).unwrap().set_modified(stamp).unwrap();
        }
    }
}

/// A card set up for the transfer (B2's shape: the fixtures' empty first
/// commit, a checkpoint that names an OWNED empty folder) in a player that
/// reads Opus and Auto DJ.
fn set_up(root: &Path) {
    put(root, "/.mstream/manifest.bin", &fixture("card/golden/manifest-min.bin"), 0x5D47_73D5);
    put(root, "/.mstream/tags-00000001.bin", &fixture("card/golden/tags-00000001.bin"), 0x5D47_73D5);
    put(root, "/.player/device.txt", &fixture("card/golden/device.txt"), 0x5D47_73D5);
    std::fs::create_dir_all(root.join("music/Owned Empty")).unwrap();
}

/// The fixture card's id, which the set-up card has.
const CARD_ID: u64 = 0x5EED_C0DE_0A1B_2C3D;
const SERIAL: u32 = 0x1A2B_3C4D;

fn origin(url: &str) -> Origin {
    let key = server_url_key(url);
    Origin {
        talk: Talk { url_key: key, ..Talk::default() },
        server: Server::direct(NO_INSTANCE, key),
        revision: "\"r-6-1\"".into(),
        ..Origin::default()
    }
}

fn home() -> Origin {
    origin("http://music.example:3000")
}

/// A file of the selection, the server's bytes on the card.
fn track(server_path: &str, bytes: &[u8], id: u32) -> WantedFile {
    let ledger = Ledger {
        server_path: server_path.into(),
        mstream_id: id,
        album_id: id / 10,
        artist_id: id / 100,
        audio_hash: md5(&[b"audio:".as_slice(), bytes].concat()),
        file_hash: md5(bytes),
        server_modified: 1_790_000_000_000 + id as u64,
        server_size: bytes.len() as u64,
        created_at: 1_790_000_000,
        hash_v: 2,
        ..Ledger::default()
    };
    let (size, bpm10) = (bytes.len() as u64, 1200 + id as u16);
    WantedFile { origin: 0, ledger, size, bpm10, camelot: 8, ..WantedFile::default() }
}

/// A server of canned files: its bytes by server path, and a conversion's
/// bytes by the file it is of.
#[derive(Default)]
struct Canned {
    files: HashMap<String, Vec<u8>>,
    converted: HashMap<String, Vec<u8>>,
    /// Fetches by server path, in order.
    fetched: Vec<String>,
    /// Files whose fetch fails, and one that stops the run.
    failing: BTreeSet<String>,
    stop_at: Option<String>,
}

impl Canned {
    fn add(&mut self, server_path: &str, bytes: Vec<u8>) {
        self.files.insert(server_path.into(), bytes);
    }

    fn bytes(&self, file: &WantedFile) -> Vec<u8> {
        let path = &file.ledger.server_path;
        match file.bytes {
            Bytes::Converted(_) => self.converted[path].clone(),
            _ => self.files[path].clone(),
        }
    }
}

impl Fetcher for Canned {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn io::Write) -> Result<Fetched, Failed> {
        let path = file.ledger.server_path.clone();
        self.fetched.push(path.clone());
        if self.stop_at.as_ref() == Some(&path) {
            return Err(Failed { why: "the day's quota is spent".into(), stop: true });
        }
        if self.failing.contains(&path) {
            return Err(Failed { why: "the server said 500".into(), stop: false });
        }
        // In pieces, as a download comes.
        for piece in self.bytes(file).chunks(1000) {
            into.write_all(piece).map_err(|e| Failed { why: e.to_string(), stop: false })?;
        }
        Ok(Fetched { content_type: Some("audio/ogg".into()) })
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        let bytes = self.bytes(file);
        let start = (start as usize).min(bytes.len());
        Ok(bytes[start..(start + len).min(bytes.len())].to_vec())
    }
}

/// What the tests' hooks add: a thumbnail for every album without one (a
/// flat colour of its folder's hash and its count of files), none for the
/// album `gone` names, and an Auto DJ index of made-up vectors under a
/// made-up model (no real embeddings: AUTODJ-INDEX.md 12.5); and a count
/// of the plays hook's calls.
#[derive(Default)]
struct Extras {
    plays: usize,
    gone: Option<&'static str>,
}

impl Hooks for Extras {
    fn plays(&mut self, _card: &dyn CardFs, _root: Option<&Root>) {
        self.plays += 1;
    }

    fn cover(&mut self, album: &Album<'_>) -> Cover {
        if self.gone == Some(album.folder) {
            return Cover::Gone;
        }
        if album.on_card {
            return Cover::Keep;
        }
        let colour = (path_hash(album.folder) as u16) ^ album.files.len() as u16;
        let pixels = Pixels { small: vec![colour; 40 * 40], large: vec![colour; 96 * 96] };
        Cover::New { pixels, source_bytes: 1_000 }
    }

    fn autodj(&mut self, commit: &Commit<'_>) -> AutoDj {
        let of = |r: &RecordIn| FileIn::of(&r.path, &r.rec, &r.run, r.ledger.as_ref());
        let files: Vec<FileIn> = commit.records.iter().filter_map(of).collect();
        let vector = |h: &[u8; 16]| (0..8).map(|i| (h[i] as f32 - 127.5) / 128.0).collect::<Vec<f32>>();
        let input = IndexIn {
            generation: commit.generation,
            card_id: commit.card_id,
            built_time: commit.now,
            tags_generation: commit.generation,
            model: Model {
                id: "synthetic".into(),
                version: "1".into(),
                metric: "cosine".into(),
                license: "CC0-1.0".into(),
                attribution: "made-up vectors for a test".into(),
            },
            selection: Selection::top(2),
            vectors: files.iter().map(|f| (f.hash, vector(&f.hash))).collect(),
            files,
        };
        let work = Work { threads: 1, ..Work::default() };
        match autodj::index(commit.device, &input, commit.old, &work) {
            Ok(Index::New { file, .. }) => AutoDj::New(file),
            Ok(Index::Same) => AutoDj::Same,
            _ => AutoDj::None,
        }
    }
}

/// The run's clock and ids, fixed: the same run makes the same bytes.
fn env(seed: u64) -> Env {
    let mut n = seed;
    let id = move || {
        n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        n
    };
    Env { now: Box::new(|| 1_791_900_000), id: Box::new(id), producer: "mstream-terminal test".into() }
}

/// Small checkpoints, so a run of a few files makes several.
fn knobs() -> Knobs {
    Knobs { checkpoint_files: 2, deep_check: true, ..Knobs::default() }
}

/// Opens, plans and runs, as a caller would: the result, and what was said.
fn sync_with(
    card: &FatSim,
    wanted: &WantedCard,
    server: &mut Canned,
    hooks: &mut dyn Hooks,
    knobs: Knobs,
    answers: Answers,
    seed: u64,
) -> (Result<Outcome, Stop>, Vec<Event>) {
    let mut events = Vec::new();
    let expect = Expect::default();
    let opened = Session::open(card, &expect, knobs, &answers, env(seed), hooks, &mut |e| events.push(e));
    let mut session = match opened {
        Ok(session) => session,
        Err(stop) => return (Err(stop), events),
    };
    let plan = match session.plan(wanted, &answers, server) {
        Ok(plan) => plan,
        Err(stop) => return (Err(stop), events),
    };
    let ran = session.run(plan, server, hooks, &mut |e| events.push(e));
    (ran, events)
}

fn sync(card: &FatSim, wanted: &WantedCard, server: &mut Canned, seed: u64) -> Result<Outcome, Stop> {
    sync_with(card, wanted, server, &mut Extras::default(), knobs(), Answers::default(), seed).0
}

/// The run after a cut, as the listener makes it: a stop for a disk check
/// answered once the check has run.
struct Recovered {
    ran: Result<Outcome, Stop>,
    /// It stopped for a disk check first.
    asked: bool,
    /// Its writes, from its first; and which of them its open made:
    /// run.lock's note, then steps 1-4's own, what settles the card.
    writes: Vec<String>,
    opened: std::ops::Range<usize>,
}

fn recover(card: &FatSim, wanted: &WantedCard, server: &mut Canned, seed: u64) -> Recovered {
    let start = card.log().len();
    let mut answers = Answers::default();
    loop {
        let before = card.log().len() - start;
        let (hooks, expect) = (&mut Extras::default(), Expect::default());
        let opened = Session::open(card, &expect, knobs(), &answers, env(seed), hooks, &mut |_| {});
        let (ran, opened) = match opened {
            Err(Stop::Guard(Guard::DiskCheck(_))) if !answers.disk_checked => {
                answers.disk_checked = true;
                continue;
            }
            Err(stop) => (Err(stop), before..card.log().len() - start),
            Ok(mut session) => {
                let opened = before..card.log().len() - start;
                let plan = session.plan(wanted, &answers, server);
                (plan.and_then(|plan| session.run(plan, server, hooks, &mut |_| {})), opened)
            }
        };
        let writes = card.log()[start..].to_vec();
        return Recovered { ran, asked: answers.disk_checked, writes, opened };
    }
}

/// A write as the log says it, the 8 hex digits of a generation or a stage
/// file's number left out: what kind of write it is.
fn kind(write: &str) -> String {
    let (mut out, mut hex) = (String::new(), String::new());
    for c in write.chars().chain(['\n']) {
        if c.is_ascii_hexdigit() {
            hex.push(c);
            continue;
        }
        out.push_str(if hex.len() == 8 { "#" } else { &hex });
        out.push(c);
        hex.clear();
    }
    out.pop();
    out
}

/// A session as the tests open one: no hooks, the default knobs.
fn open<'c>(
    card: &'c FatSim,
    answers: Answers,
    seed: u64,
    events: &mut dyn FnMut(Event),
) -> Result<Session<'c>, Stop> {
    Session::open(card, &Expect::default(), knobs(), &answers, env(seed), &mut NoHooks, events)
}

/// A session and its plan, for the tests that look at a plan.
fn plan_of<'c>(
    card: &'c FatSim,
    wanted: &WantedCard,
    server: &mut Canned,
    answers: Answers,
    k: Knobs,
) -> (Session<'c>, Plan) {
    let opened = Session::open(card, &Expect::default(), k, &answers, env(5), &mut NoHooks, &mut |_| {});
    let mut session = opened.unwrap_or_else(|e| panic!("open: {e}"));
    let plan = session.plan(wanted, &answers, server).unwrap_or_else(|e| panic!("plan: {e}"));
    (session, plan)
}

/// The card as the kit reads it (the volume layer's look): its root, which
/// must read with its tags file and Auto DJ index whole.
fn look(card: &FatSim) -> Inspection {
    let seen = volume::inspect(card).expect("the card lists");
    if let Some(root) = &seen.root {
        assert!(root.tags.is_ok(), "the root's tags file: {:?}", root.tags.as_ref().err());
        assert!(root.autodj.is_none_or(|v| v.is_ok()), "the root's Auto DJ index: {:?}", root.autodj);
    }
    seen
}

fn records(card: &FatSim) -> Vec<mptg::Entry> {
    look(card).root.expect("a root").tags.expect("its tags file").records
}

/// A card image as its golden file lists it.
fn image_text(card: &FatSim) -> String {
    let mut out = String::new();
    for (path, (size, fat_time, fnv)) in card.image() {
        out += &format!("{path}\t{size}\t{fat_time:08X}\t{fnv:016X}\n");
    }
    out
}

fn golden(name: &str, card: &FatSim) {
    dump(name, card);
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join(format!("test/golden/card-engine/{name}.txt"));
    let got = image_text(card);
    if std::env::var_os("MSTREAM_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let want = want.replace("\r\n", "\n");
    assert_eq!(got, want, "the card image of {name} (MSTREAM_BLESS=1 writes it again)");
}

/// A card copied out for the reference readers, when asked to.
fn dump(name: &str, card: &FatSim) {
    if let Some(dir) = std::env::var_os("MSTREAM_CARD_DUMP") {
        let to = Path::new(&dir).join(name);
        let _ = std::fs::remove_dir_all(&to);
        copy_tree(&card.root(), &to);
    }
}

// ── The library ────────────────────────────────────────────────────────────

/// Six files of one library: two albums of one artist, a live record (an
/// Opus stream the server keeps as `.ogg`, and a FLAC converted to Opus),
/// a loose file.
fn library(server: &mut Canned) -> WantedCard {
    let mut files = Vec::new();
    let mut add = |path: &str, corpus_name: &str, id: u32| {
        let bytes = corpus(corpus_name);
        server.add(path, bytes.clone());
        files.push(track(path, &bytes, id));
    };
    add("Lib/Alex Artist/First Album/01 - One.mp3", "cbr.mp3", 101);
    add("Lib/Alex Artist/First Album/02 - Two.mp3", "v24_full.mp3", 102);
    add("Lib/Alex Artist/Second Album/01 - Three.flac", "flac_full.flac", 103);
    add("Lib/Blake Band/Live/01 - Four.ogg", "opus_basic.opus", 201);
    add("Lib/Blake Band/Live/02 - Five.flac", "flac_text.flac", 202);
    add("Lib/Loose.mp3", "multivalue.mp3", 301);
    files[3].bytes = Bytes::Original { ogg_opus: true };
    // Five goes on the card as Opus.
    files[4].bytes = Bytes::Converted(CONTAINER_OPUS);
    files[4].kbps = 192;
    let opus = corpus("opus_pages.opus");
    files[4].size = opus.len() as u64;
    server.converted.insert(files[4].ledger.server_path.clone(), opus);
    WantedCard { origins: vec![home()], files }
}

/// The first sync onto a set-up card.
fn first(dir: &Path) -> (FatSim, WantedCard, Canned) {
    set_up(dir);
    let card = FatSim::new(dir, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let outcome = sync(&card, &wanted, &mut server, 1);
    let outcome = outcome.unwrap_or_else(|e| panic!("the first sync: {e}"));
    assert_eq!(outcome.verified, Ok(()));
    (card, wanted, server)
}

const MOVED: &str = "Lib/Alex Artist/Second Album (Deluxe)/01 - Three.flac";

/// What changed after the first sync, on the server and on the card: each
/// row of the plan table once.
fn second(dir: &Path, wanted: &mut WantedCard, server: &mut Canned) {
    // The server: Two retagged, Loose gone, Three moved to another album,
    // a new album.
    let two = corpus("v23_utf16.mp3");
    server.add("Lib/Alex Artist/First Album/02 - Two.mp3", two.clone());
    wanted.files[1] = track("Lib/Alex Artist/First Album/02 - Two.mp3", &two, 102);
    wanted.files.retain(|f| f.ledger.server_path != "Lib/Loose.mp3");
    server.add(MOVED, server.files["Lib/Alex Artist/Second Album/01 - Three.flac"].clone());
    wanted.files[2].ledger.server_path = MOVED.into();
    wanted.files[2].ledger.mstream_id = 1103;
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));

    // The card: the listener renamed a folder's case, retagged One, deleted
    // Five, moved Four, and copied in a song of their own.
    let music = dir.join("music");
    std::fs::rename(music.join("Blake Band"), music.join("tmp")).unwrap();
    std::fs::rename(music.join("tmp"), music.join("BLAKE BAND")).unwrap();
    let mut one = std::fs::read(music.join("Alex Artist/First Album/01 - One.mp3")).unwrap();
    one.extend_from_slice(b"TAG edited by the listener");
    put(dir, "/music/Alex Artist/First Album/01 - One.mp3", &one, LATER);
    std::fs::remove_file(music.join("BLAKE BAND/Live/02 - Five.opus")).unwrap();
    std::fs::create_dir_all(music.join("Moved")).unwrap();
    let four = music.join("BLAKE BAND/Live/01 - Four.opus");
    std::fs::rename(four, music.join("Moved/01 - Four.opus")).unwrap();
    put(dir, "/music/Mine/song.mp3", &corpus("latin1.mp3"), LATER);
}

/// What a card holds once a run is over, its stamps and ids aside:
/// `/music`'s files, and the root's records and folders.
#[derive(Debug, PartialEq, Eq)]
struct Finished {
    music: BTreeMap<String, (u64, u64)>,
    records: Vec<(String, u32, u64, String, u8, u16)>,
    folders: Vec<(String, u32)>,
    autodj: bool,
}

fn finished(card: &FatSim) -> Finished {
    let image = card.image();
    let music = image
        .iter()
        .filter(|(p, _)| p.starts_with("/music/"))
        .map(|(p, (size, _, fnv))| (p.clone(), (*size, *fnv)))
        .collect();
    let leftovers: Vec<&String> = image
        .keys()
        .filter(|p| p.starts_with("/.mstream/"))
        .filter(|p| p.contains("/stage/") || p.ends_with(".tmp") || p.contains("pending."))
        .chain(image.keys().filter(|p| p.ends_with("run.lock")))
        .collect();
    assert!(leftovers.is_empty(), "leftovers after a finished run: {leftovers:?}");
    let seen = look(card);
    assert!(seen.pending.is_none(), "no plan left");
    let root = seen.root.expect("a root");
    assert!(root.manifest.is_final(), "the root is a final commit");
    let tags = root.tags.expect("its tags file");
    let records = tags
        .records
        .iter()
        .map(|e| {
            let l = e.ledger.clone().unwrap_or_default();
            (e.path.clone(), e.rec.size, e.rec.qfp, l.server_path, l.origin_flags, l.server)
        })
        .collect();
    let flagged = tags.folders.iter().filter(|f| f.flags != 0);
    let folders = flagged.map(|f| (f.path.clone(), f.flags)).collect();
    Finished { music, records, folders, autodj: root.autodj.is_some() }
}

// ── The simulated card ──────────────────────────────────────────────────────

#[test]
fn the_simulated_card_matches_names_and_renames_as_fat_does() {
    let dir = Scratch::new("sim");
    let card = FatSim::new(&dir.0, 7);
    card.mkdir("/music").unwrap();
    card.mkdir("/music/ACME").unwrap();
    assert_eq!(card.mkdir("/music/Acme").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    let mut f = card.create("/music/acme/x.mp3").unwrap();
    f.write_all(b"one").unwrap();
    f.sync().unwrap();
    drop(f);
    // Found under any case, listed as stored.
    let item = card.stat("/MUSIC/Acme/X.MP3").unwrap().unwrap();
    assert_eq!((item.name.as_str(), item.size), ("x.mp3", 3));
    assert_eq!(card.list("/music/ACME").unwrap()[0].name, "x.mp3");
    // A rename never replaces another file; one of the case alone is the
    // file keeping its place.
    let mut g = card.create("/music/ACME/y.mp3").unwrap();
    g.write_all(b"two").unwrap();
    drop(g);
    let taken = card.rename("/music/ACME/y.mp3", "/music/ACME/X.mp3").unwrap_err();
    assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
    card.rename("/music/ACME/x.mp3", "/music/ACME/X.mp3").unwrap();
    let names: BTreeSet<String> =
        card.list("/music/ACME").unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(names, BTreeSet::from(["X.mp3".to_string(), "y.mp3".to_string()]));
    // Stamps at 2 s, and a PC that lists them an hour off.
    let stamp = card.stat("/music/ACME/X.mp3").unwrap().unwrap().fat_time;
    assert!(fat_wall(stamp).is_some());
    card.set_skew(3600);
    let skewed = card.stat("/music/ACME/X.mp3").unwrap().unwrap().fat_time;
    assert_eq!(fat_wall(skewed).unwrap() - fat_wall(stamp).unwrap(), 3600);
    card.set_skew(0);
    // A cut write pulls the card: everything fails until it is back.
    card.cut_at(1, Cut::Fail);
    assert!(card.delete("/music/ACME/y.mp3").is_err());
    assert!(card.list("/music").is_err());
    card.replug();
    assert!(card.stat("/music/ACME/y.mp3").unwrap().is_some(), "the cut delete didn't happen");
    // A rename cut between its two directory writes leaves both names.
    card.cut_at(1, Cut::Between);
    assert!(card.rename("/music/ACME/y.mp3", "/music/ACME/z.mp3").is_err());
    card.replug();
    let read = |p: &str| card.read(p, 99).unwrap();
    let both = (read("/music/ACME/y.mp3"), read("/music/ACME/z.mp3"));
    assert_eq!(both, (Some(b"two".to_vec()), Some(b"two".to_vec())));
}

// ── Steps 1-4 ───────────────────────────────────────────────────────────────

/// vectors.json's privateFiles.cleanup: step 4 over a `/.mstream` of
/// leftovers deletes exactly the vector's files, and the private files
/// stay. The root, where the case has one, is the fixture card's.
#[test]
fn the_clean_up_deletes_what_the_vectors_say() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/vectors.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = v["privateFiles"]["cleanup"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    let golden = ["manifest.bin", "tags-0000002a.bin", "autodj-00000029.bin"];
    for (k, c) in cases.iter().enumerate() {
        let dir = Scratch::new(&format!("cleanup{k}"));
        let files = c["files"].as_object().unwrap();
        for (rel, size) in files {
            let host = dir.0.join(".mstream").join(rel);
            std::fs::create_dir_all(host.parent().unwrap()).unwrap();
            if golden.contains(&rel.as_str()) {
                std::fs::write(&host, fixture(&format!("card/golden/{rel}"))).unwrap();
            } else if rel == "resume.json" && c["resume"].is_string() {
                std::fs::write(&host, c["resume"].as_str().unwrap()).unwrap();
            } else {
                // Junk of the size listed, no two files alike (a twin pair
                // would stop for a disk check); zeros past its head.
                let size = size.as_u64().unwrap();
                let mut file = std::fs::File::create(&host).unwrap();
                let fill = 0x41 + (rel.len() % 26) as u8;
                file.write_all(&vec![fill; size.min(64) as usize]).unwrap();
                file.write_all(rel.as_bytes()).unwrap();
                file.set_len(size).unwrap();
            }
        }
        let card = FatSim::new(&dir.0, 9);
        let mut events = Vec::new();
        let opened = open(&card, Answers::default(), 3, &mut |e| events.push(e));
        let session = opened.unwrap_or_else(|e| panic!("case {k}: {e}"));
        let image = card.image();
        let left: BTreeSet<String> =
            image.keys().filter_map(|p| p.strip_prefix("/.mstream/")).map(String::from).collect();
        let gone: BTreeSet<String> =
            c["delete"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
        let mut want: BTreeSet<String> = files.keys().filter(|p| !gone.contains(*p)).cloned().collect();
        want.insert("run.lock".into());
        assert_eq!(left, want, "case {k}: {}", c["note"]);
        let took = events.iter().any(|e| matches!(e, Event::TookOver(_)));
        assert_eq!(took, files.contains_key("run.lock"), "case {k}: a stale run.lock is taken over");
        session.close();
        assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "case {k}: the card let go");
    }
}

#[test]
fn a_newer_format_or_a_cut_rename_stops_before_anything_is_written() {
    let dir = Scratch::new("stops");
    set_up(&dir.0);
    let mut newer = fixture("card/golden/tags-00000007.bin");
    newer[4] = 2;
    put(&dir.0, "/.mstream/tags-00000099.bin", &newer, LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let before = card.image();
    let opened = open(&card, Answers::default(), 1, &mut |_| {});
    assert!(matches!(opened, Err(Stop::Guard(Guard::Newer(ref n))) if n == &["tags-00000099.bin"]));
    assert_eq!(card.image(), before, "not even run.lock");
    std::fs::remove_file(dir.0.join(".mstream/tags-00000099.bin")).unwrap();

    // A cut run's plan whose write op 5 (Loose New.mp3, 12 bytes) has its
    // stage file and its target alike: a rename cut between its two
    // directory writes, perhaps.
    let plan = fixture("card/golden/pending.bin");
    put(&dir.0, "/.mstream/pending.bin", &plan, LATER);
    put(&dir.0, "/.mstream/stage/00000005.tmp", b"twelve bytes", LATER);
    put(&dir.0, "/music/Loose New.mp3", b"twelve bytes", LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let before = card.image();
    let opened = open(&card, Answers::default(), 1, &mut |_| {});
    let pairs = vec![("/.mstream/stage/00000005.tmp".to_string(), "/music/Loose New.mp3".to_string())];
    assert!(matches!(opened, Err(Stop::Guard(Guard::DiskCheck(ref p))) if *p == pairs));
    assert_eq!(card.image(), before, "nothing deleted, run.lock gone again");

    // Once the disk check has run, the leftovers go: the stage file, and
    // the cut run's target no plan wants now.
    let answers = Answers { disk_checked: true, ..Answers::default() };
    let mut server = Canned::default();
    let wanted = WantedCard { origins: vec![home()], files: Vec::new() };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut NoHooks, knobs(), answers, 1);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    let image = card.image();
    assert!(!image.contains_key("/.mstream/stage/00000005.tmp"));
    assert!(!image.contains_key("/music/Loose New.mp3"), "the software's half-finished work, deleted");
}

// ── Whole runs ──────────────────────────────────────────────────────────────

#[test]
fn a_first_sync_fills_a_set_up_card() {
    let dir = Scratch::new("first");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut extras = Extras::default();
    let answers = Answers::default();
    let (ran, events) = sync_with(&card, &wanted, &mut server, &mut extras, knobs(), answers, 1);
    let outcome = ran.unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(outcome.verified, Ok(()));
    assert_eq!((outcome.written, outcome.renamed, outcome.deleted, outcome.records), (6, 0, 0, 6));
    assert_eq!(outcome.card_id, CARD_ID, "the card's id, from its set-up");
    assert_eq!(extras.plays, 1, "the plays hook, once, before anything is written");
    let commits: Vec<(u32, bool)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Committed { generation, last } => Some((*generation, *last)),
            _ => None,
        })
        .collect();
    assert_eq!(commits, [(2, false), (3, false), (4, true)], "a checkpoint every 2 files");

    let records = records(&card);
    let paths: Vec<&str> = records.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "Loose.mp3",
            "Alex Artist/First Album/01 - One.mp3",
            "Alex Artist/First Album/02 - Two.mp3",
            "Alex Artist/Second Album/01 - Three.flac",
            "Blake Band/Live/01 - Four.opus",
            "Blake Band/Live/02 - Five.opus",
        ]
    );
    let by = |p: &str| records.iter().find(|e| e.path == p).unwrap().clone();
    let one = by("Alex Artist/First Album/01 - One.mp3");
    let l = one.ledger.unwrap();
    assert_eq!((l.origin_flags, l.converted_to, l.mstream_id), (VERIFIED, 0, 101), "the MD5 matched");
    assert_eq!(one.rec.size as usize, corpus("cbr.mp3").len());
    let four = by("Blake Band/Live/01 - Four.opus");
    let copied = four.ledger.unwrap().converted_to;
    assert_eq!(copied, 0, "an Opus stream kept as .ogg: copied, named .opus");
    let five = by("Blake Band/Live/02 - Five.opus");
    let l = five.ledger.unwrap();
    assert_eq!((l.converted_to, l.convert_kbps, l.origin_flags), (CONTAINER_OPUS, 192, 0));
    assert_eq!(five.rec.container, CONTAINER_OPUS, "read from the card's bytes");
    // mStream's analysis where the file's tags have neither BPM nor key.
    assert!(records.iter().any(|e| e.rec.flags & BPM_ANALYSED != 0 && e.rec.bpm10 != 0));

    let seen = look(&card);
    let root = seen.root.unwrap();
    assert_eq!((root.manifest.header.generation, root.manifest.base_generation), (4, 3));
    assert_eq!(outcome.commit_id, root.manifest.commit_id);
    assert_eq!(root.manifest.server_url_key, server_url_key("http://music.example:3000"));
    assert_eq!(root.manifest.server_revision, "\"r-6-1\"");
    assert!(root.manifest.roots.is_empty(), "one library: /music is the only root");
    assert_eq!(root.autodj, Some(Ok(())));
    let flagged = root.tags.unwrap().folders.into_iter().filter(|f| f.flags != 0);
    let folders: BTreeMap<String, u32> = flagged.map(|f| (f.path, f.flags)).collect();
    let owned_thumb = FOLDER_OWNED | FOLDER_THUMB;
    let want = BTreeMap::from([
        ("".to_string(), FOLDER_THUMB),
        ("Alex Artist".to_string(), FOLDER_OWNED),
        ("Alex Artist/First Album".to_string(), owned_thumb),
        ("Alex Artist/Second Album".to_string(), owned_thumb),
        ("Blake Band".to_string(), FOLDER_OWNED),
        ("Blake Band/Live".to_string(), owned_thumb),
    ]);
    assert_eq!(folders, want, "the set-up's empty OWNED folder, collected");
    assert!(card.stat("/music/Owned Empty").unwrap().is_none());
    golden("first", &card);
}

#[test]
fn a_second_sync_follows_the_plan_table() {
    let dir = Scratch::new("second");
    let (card, mut wanted, mut server) = first(&dir.0);
    second(&dir.0, &mut wanted, &mut server);
    let (mut session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    assert!(plan.guards.is_empty(), "{:?}", plan.guards);
    assert_eq!(plan.removals(), ["Loose.mp3"], "Q1's list: Loose left the selection");
    assert_eq!(plan.report.edited, ["Alex Artist/First Album/01 - One.mp3"], "U5: the listener's now");
    assert_eq!(plan.renames(), 1, "Three, moved on the server, is renamed on the card");
    // Two (changed), Five (deleted by the listener), Six (new), and One
    // beside the listener's edit of it.
    assert_eq!(plan.fetches().0, 4);
    drop(session.plan(&wanted, &Answers::default(), &mut server).unwrap());
    let fetched_before = server.fetched.len();
    let outcome = session.run(plan, &mut server, &mut Extras::default(), &mut |_| {}).unwrap();
    assert_eq!(outcome.verified, Ok(()));
    assert_eq!((outcome.written, outcome.renamed, outcome.deleted), (4, 1, 1));
    assert_eq!(server.fetched.len() - fetched_before, 4);

    let paths: BTreeSet<String> = records(&card).into_iter().map(|e| e.path).collect();
    let want: BTreeSet<String> = [
        "Moved/01 - Four.opus",
        "Alex Artist/First Album/01 - One (2).mp3",
        "Alex Artist/First Album/02 - Two.mp3",
        "Alex Artist/Second Album (Deluxe)/01 - Three.flac",
        "BLAKE BAND/Live/02 - Five.opus",
        "Casey Choir/Hymns/01 - Six.mp3",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(paths, want, "the listener's move followed, the folder's new case taken");
    let image = card.image();
    assert!(image.contains_key("/music/Alex Artist/First Album/01 - One.mp3"), "the edited file stays");
    assert!(image.contains_key("/music/Mine/song.mp3"), "the listener's own file stays");
    assert!(!image.contains_key("/music/Loose.mp3"));
    let emptied = card.stat("/music/Alex Artist/Second Album").unwrap();
    assert!(emptied.is_none(), "emptied, OWNED: collected");
    golden("second", &card);

    // Nothing new on either side: no commit, and the card as it was.
    let before = card.image();
    let again = sync(&card, &wanted, &mut server, 9).unwrap();
    let did = (again.written, again.renamed, again.deleted, again.generation);
    assert_eq!(did, (0, 0, 0, outcome.generation));
    assert_eq!(card.image(), before, "a run with nothing to do makes no commit");
}

#[test]
fn every_cut_leaves_a_card_the_kit_reads_and_the_next_run_finishes() {
    let base = Scratch::new("cut-base");
    let (card, mut wanted, mut server) = first(&base.0);
    drop(card);
    second(&base.0, &mut wanted, &mut server);

    // The run uncut: its writes, and where it leaves the card.
    let whole = Scratch::new("cut-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    sync(&card, &wanted, &mut server, 2).expect("uncut");
    let writes = card.log();
    let want = finished(&card);

    // Each write cut in turn; each rename also between its two directory
    // writes.
    let mut disk_checks = 0;
    let mut settled: BTreeSet<(String, bool)> = BTreeSet::new();
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
        // What the card holds is a commit: the one before, a checkpoint, or
        // the last.
        let seen = look(&card);
        assert!(seen.root.is_some(), "{what}: a root");
        dump(&format!("cut-{n:03}-{how:?}"), &card);

        // The next run brings it to where the uncut run did.
        let again = recover(&card, &wanted, &mut server, 3);
        if again.asked {
            assert_eq!(how, Cut::Between, "{what}: only a cut rename asks for a disk check");
            disk_checks += 1;
        }
        let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");

        // That run cut in its turn at a write of its own settling — a
        // rename finished, a leftover deleted (2.12.1, steps 1-4; 2.12.2) —
        // each kind of write once, wherever it first comes (the same cut
        // card made again): the run after still finishes.
        let own = |c: &(u64, Cut)| c.0 > again.opened.start as u64 + 1;
        for (m, then) in cuts_of(&again.writes, again.opened.end).filter(own) {
            let at = &again.writes[m as usize - 1];
            if !settled.insert((kind(at), then == Cut::Between)) {
                continue;
            }
            let what = format!("{what}, then at write {m} of the next run ({then:?}, {at})");
            let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
            card.cut_at(m, then);
            let _ = recover(&card, &wanted, &mut server, 3);
            card.replug();
            assert!(look(&card).root.is_some(), "{what}: a root");
            let outcome = recover(&card, &wanted, &mut server, 4).ran;
            let outcome = outcome.unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(outcome.verified, Ok(()), "{what}");
            assert_eq!(finished(&card), want, "{what}");
        }
    }
    assert!(disk_checks > 0, "a rename cut between its directory writes stops for a disk check");
    for rename in ["manifest", "pending"] {
        let at = format!("rename /.mstream/{rename}.tmp -> /.mstream/{rename}.bin");
        assert!(settled.contains(&(at, true)), "a settling's own rename, cut: {settled:?}");
    }
    assert!(settled.contains(&("delete /.mstream/stage/#.tmp".into(), false)), "{settled:?}");
}

/// A copy of the card in `base` that a sync left cut at its `n`th write, as
/// `how` says, back in the reader: the same card each time.
fn cut_card(
    base: &Path,
    n: u64,
    how: Cut,
    wanted: &WantedCard,
    server: &mut Canned,
) -> (Scratch, FatSim) {
    let dir = Scratch::new("cut");
    copy_tree(base, &dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    card.cut_at(n, how);
    let _ = sync(&card, wanted, server, 2);
    card.replug();
    (dir, card)
}

/// The cuts of a run's first `upto` writes: each write, and each rename
/// also between its two directory writes.
fn cuts_of(writes: &[String], upto: usize) -> impl Iterator<Item = (u64, Cut)> + '_ {
    let fail = (1..=upto as u64).map(|n| (n, Cut::Fail));
    let renames = writes[..upto].iter().enumerate().filter(|(_, w)| w.starts_with("rename"));
    fail.chain(renames.map(|(i, _)| (i as u64 + 1, Cut::Between)))
}

/// 2.12.1 on a card with no root at all: the run mints the card's id, and
/// numbers its first commit past every name on the card. Each write up to
/// that first checkpoint and the plan written after it, cut: the card
/// reads as before or as that commit, and the next run finishes.
#[test]
fn every_cut_of_a_first_sync_on_a_card_with_no_root_is_finished() {
    let base = Scratch::new("rootless-base");
    put(&base.0, "/.player/device.txt", &fixture("card/golden/device.txt"), 0x5D47_73D5);
    let mut server = Canned::default();
    let wanted = library(&mut server);

    let whole = Scratch::new("rootless-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    let outcome = sync(&card, &wanted, &mut server, 2).expect("uncut");
    assert_eq!((outcome.written, outcome.verified), (6, Ok(())));
    let writes = card.log();
    let want = finished(&card);
    let committed = writes.iter().position(|w| w.starts_with("rename /.mstream/manifest.tmp"));
    let committed = committed.expect("a commit");
    let rest = |w: &String| w.starts_with("rename /.mstream/pending.tmp");
    let upto = committed + writes[committed..].iter().position(rest).expect("the plan's rest") + 1;

    let mut roots = 0;
    for (n, how) in cuts_of(&writes, upto) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
        roots += usize::from(look(&card).root.is_some());
        let outcome = recover(&card, &wanted, &mut server, 3).ran;
        let outcome = outcome.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
    }
    assert!(roots > 0, "the later cuts leave the first commit");
}

#[test]
fn a_card_swapped_mid_run_is_never_written() {
    let base = Scratch::new("swap-base");
    let (card, mut wanted, mut server) = first(&base.0);
    drop(card);
    second(&base.0, &mut wanted, &mut server);
    let theirs = Scratch::new("swap-theirs");
    set_up(&theirs.0);
    put(&theirs.0, "/music/Theirs/a.mp3", &corpus("cbr.mp3"), LATER);

    let whole = Scratch::new("swap-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    sync(&card, &wanted, &mut server, 2).expect("uncut");
    let log = card.log();
    let last = log.last().map(String::as_str);
    assert_eq!(last, Some("delete /.mstream/run.lock"), "the run's last write");
    let writes = log.len() as u64;
    for after in 1..writes {
        let dir = Scratch::new("swap");
        copy_tree(&base.0, &dir.0);
        let other = Scratch::new("swapped");
        copy_tree(&theirs.0, &other.0);
        let before = FatSim::new(&other.0, 0x0BAD_CAFE).image();
        let card = FatSim::new(&dir.0, SERIAL);
        card.swap_after(after, &other.0, 0x0BAD_CAFE);
        let ran = sync(&card, &wanted, &mut server, 2);
        // Swapped before a write of the run's own: it stops there, or, when
        // what it reads of the other card leaves it nothing to write, its
        // last look finds the card isn't the one it wrote. (After its last
        // write, only the delete of run.lock is left, which a card no
        // longer bound never gets.)
        let stopped = matches!(ran, Err(Stop::Pulled | Stop::DifferentCard));
        let unverified = matches!(&ran, Ok(o) if o.verified.is_err());
        assert!(stopped || unverified || after + 1 == writes, "swapped after write {after}: {ran:?}");
        let after_run = FatSim::new(&other.0, 0x0BAD_CAFE).image();
        assert_eq!(after_run, before, "swapped after write {after}: untouched");
    }
}

#[test]
fn the_run_is_bound_to_the_card_the_caller_names() {
    let dir = Scratch::new("bound");
    let (card, wanted, mut server) = first(&dir.0);
    let open = |expect: Expect| {
        let answers = Answers::default();
        let opened = Session::open(&card, &expect, knobs(), &answers, env(4), &mut NoHooks, &mut |_| {});
        opened.map(Session::close)
    };
    assert!(matches!(open(Expect { card_id: Some(1), ..Expect::default() }), Err(Stop::DifferentCard)));
    let serial = Some(Serial::Fat(SERIAL ^ 1));
    assert!(matches!(open(Expect { serial, ..Expect::default() }), Err(Stop::DifferentCard)));
    let serial = Some(Serial::Fat(SERIAL));
    open(Expect { serial, card_id: Some(CARD_ID), commit_id: None }).expect("the card named");

    // Another computer commits between the plan and the run: the run
    // writes nothing more, run.lock included.
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    put(&dir.0, "/.mstream/manifest.bin", &fixture("card/golden/manifest-min.bin"), LATER);
    // run.lock aside: Windows keeps another handle from reading a locked
    // file.
    let image = || {
        card.image().into_iter().filter(|(p, _)| !p.ends_with("run.lock")).collect::<BTreeMap<_, _>>()
    };
    let before = image();
    let ran = session.run(plan, &mut server, &mut NoHooks, &mut |_| {});
    assert!(matches!(ran, Err(Stop::DifferentCard)), "{ran:?}");
    assert_eq!(image(), before);
    assert!(card.stat("/.mstream/run.lock").unwrap().is_some(), "left alone: a card no longer bound");
}

#[test]
fn a_stale_lock_is_taken_over_and_a_live_one_keeps_the_card() {
    let dir = Scratch::new("locks");
    let (card, wanted, mut server) = first(&dir.0);
    let note = r#"{"format": "mstream-card-run", "runId": "0x00C0FFEE00C0FFEE", "started": 1791900000, "#
        .to_string()
        + r#""producer": "x"}"#;
    put(&dir.0, "/.mstream/run.lock", note.as_bytes(), LATER);
    let (ran, events) =
        sync_with(&card, &wanted, &mut server, &mut NoHooks, knobs(), Answers::default(), 6);
    ran.expect("a run past a stale lock");
    assert!(events.contains(&Event::TookOver(note)), "it says so");
    assert!(card.stat("/.mstream/run.lock").unwrap().is_none());

    // Another program on this computer holds run.lock's OS lock.
    put(&dir.0, "/.mstream/run.lock", b"{}", LATER);
    let lock = dir.0.join(".mstream/run.lock");
    let held = std::fs::File::options().read(true).write(true).open(lock).unwrap();
    held.try_lock().unwrap();
    let before = card.image();
    assert!(matches!(sync(&card, &wanted, &mut server, 6), Err(Stop::Busy)));
    assert_eq!(card.image(), before, "the card untouched");
    drop(held);
    sync(&card, &wanted, &mut server, 6).expect("free again");
}

#[test]
fn one_run_holds_a_card_at_a_time() {
    let dir = Scratch::new("one-run");
    let (card, wanted, mut server) = first(&dir.0);
    let identity = |card: &FatSim| open(card, Answers::default(), 7, &mut |_| {}).map(|s| s.identity());
    let mut first_run = open(&card, Answers::default(), 7, &mut |_| {}).unwrap();
    assert!(matches!(identity(&card), Err(Stop::Busy)));
    // The same card under another handle: the GUI's worker and a command.
    let again = FatSim::new(&dir.0, SERIAL);
    assert!(matches!(identity(&again), Err(Stop::Busy)));
    let plan = first_run.plan(&wanted, &Answers::default(), &mut server).unwrap();
    first_run.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    let (serial, card_id, _) = identity(&card).expect("let go");
    assert_eq!((serial, card_id), (Some(Serial::Fat(SERIAL)), CARD_ID));
}

// ── The guards ──────────────────────────────────────────────────────────────

#[test]
fn removals_ask_past_their_gates() {
    let dir = Scratch::new("removals");
    let (card, mut wanted, mut server) = first(&dir.0);
    wanted.files.retain(|f| f.ledger.server_path != "Lib/Loose.mp3");
    let strict = Knobs { gate: 0, ..knobs() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), strict.clone());
    let gate = Guard::Removals { paths: vec!["Loose.mp3".into()], strong: false };
    assert_eq!(plan.guards, [gate.clone()]);
    let ran = session.run(plan, &mut server, &mut NoHooks, &mut |_| {});
    assert!(matches!(ran, Err(Stop::Unanswered(g)) if g == [gate]));
    let yes = Answers { removals: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, yes, strict);
    assert!(plan.guards.is_empty());
    session.close();

    // Most of the ledger: the stronger gate, which the first answer doesn't
    // pass.
    let gone = WantedCard { origins: vec![home()], files: wanted.files[..1].to_vec() };
    let (session, plan) = plan_of(&card, &gone, &mut server, yes, knobs());
    assert!(matches!(&plan.guards[..], [Guard::Removals { paths, strong: true }] if paths.len() == 5));
    session.close();
    let all = Answers { mass_removals: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &gone, &mut server, all, knobs());
    assert!(plan.guards.is_empty());
    session.close();

    // A server whose whole manifest lacks most of the card's files.
    let mut emptied = wanted.clone();
    emptied.origins[0].whole = Some(std::collections::HashSet::new());
    let (session, plan) = plan_of(&card, &emptied, &mut server, yes, knobs());
    assert!(matches!(&plan.guards[..], [Guard::Removals { strong: true, .. }]));
    session.close();

    // Q1's other setting: nothing leaves the card.
    let never = Knobs { removal: Removal::Never, gate: 0, ..knobs() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), never);
    assert!(plan.guards.is_empty() && plan.removals().is_empty());
    assert_eq!(plan.report.kept, ["Loose.mp3"]);
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.deleted, outcome.records), (0, 6));
}

/// 2.10.2, step 5: the server's library replaced under the same paths
/// (other audio, so every row pairs and is replaced in place) is a mass
/// deletion too when its whole manifest says so: asked before anything
/// goes, with nothing left the selection.
#[test]
fn a_library_replaced_at_the_same_paths_asks_before_anything_goes() {
    let dir = Scratch::new("replaced");
    let (card, wanted, mut server) = first(&dir.0);
    let mut other = wanted.clone();
    for f in &mut other.files {
        f.ledger.audio_hash[0] ^= 0xFF;
        f.ledger.server_modified += 1;
    }
    // Not read whole: changed files, replaced, as any run does.
    let (session, plan) = plan_of(&card, &other, &mut server, Answers::default(), knobs());
    assert!(plan.guards.is_empty() && plan.removals().is_empty(), "{:?}", plan.guards);
    assert_eq!(plan.fetches().0, 6);
    session.close();

    other.origins[0].whole = Some(other.files.iter().map(|f| f.ledger.audio_hash).collect());
    let (session, plan) = plan_of(&card, &other, &mut server, Answers::default(), knobs());
    let mut paths: Vec<String> = records(&card).into_iter().map(|e| e.path).collect();
    paths.sort();
    assert_eq!(plan.guards, [Guard::Removals { paths, strong: true }]);
    assert!(plan.removals().is_empty(), "none left the selection");
    session.close();
    let all = Answers { mass_removals: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &other, &mut server, all, knobs());
    assert!(plan.guards.is_empty());
    session.close();
}

#[test]
fn a_server_that_may_be_the_cards_own_is_asked_about() {
    let dir = Scratch::new("servers");
    let (card, wanted, mut server) = first(&dir.0);
    // The same library at another address (a tunnel, say).
    let mut tunnel = wanted.clone();
    tunnel.origins = vec![origin("https://tunnel.example")];
    let (session, plan) = plan_of(&card, &tunnel, &mut server, Answers::default(), knobs());
    assert_eq!(plan.guards, [Guard::Server { origin: 0, rows: 6 }]);
    session.close();

    // The listener says it is this card's server: nothing to do.
    tunnel.origins[0].talk.confirmed = vec![server_url_key("http://music.example:3000")];
    let (session, plan) = plan_of(&card, &tunnel, &mut server, Answers::default(), knobs());
    assert!(plan.guards.is_empty());
    assert_eq!((plan.fetches().0, plan.kept().0), (0, 6));
    session.close();

    // Or a server of its own: its files go beside the others', and the
    // root lists it in SRVS.
    tunnel.origins[0].talk.confirmed.clear();
    let other = Answers { new_servers: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &tunnel, &mut server, other, knobs());
    assert!(plan.guards.is_empty());
    assert_eq!(plan.fetches().0, 6);
    session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    let root = look(&card).root.unwrap();
    assert_eq!(root.manifest.servers.len(), 1);
    let records = root.tags.unwrap().records;
    let second_server = records.iter().filter(|e| e.ledger.as_ref().unwrap().server == 1);
    let theirs: Vec<&str> = second_server.map(|e| e.path.as_str()).collect();
    assert_eq!(theirs.len(), 6);
    assert!(theirs.contains(&"Alex Artist/First Album/01 - One (2).mp3"), "{theirs:?}");
}

#[test]
fn music_without_a_ledger_is_adopted_only_when_the_listener_says() {
    let dir = Scratch::new("adopt");
    set_up(&dir.0);
    // The listener copied One by hand, and a Two of their own.
    put(&dir.0, "/music/Alex Artist/First Album/01 - One.mp3", &corpus("cbr.mp3"), LATER);
    put(&dir.0, "/music/Alex Artist/First Album/02 - Two.mp3", &corpus("latin1.mp3"), LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    assert_eq!(plan.guards, [Guard::Adopt(vec!["Alex Artist/First Album/01 - One.mp3".into()])]);
    session.close();

    let no = Answers { adopt: Some(false), ..Answers::default() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, no, knobs());
    assert!(plan.guards.is_empty());
    assert_eq!(plan.fetches().0, 6, "written beside the listener's");
    session.close();

    let yes = Answers { adopt: Some(true), ..Answers::default() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, yes, knobs());
    assert_eq!((plan.fetches().0, plan.kept().1), (5, 1));
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!(outcome.verified, Ok(()));
    let records = records(&card);
    let one = records.iter().find(|e| e.path == "Alex Artist/First Album/01 - One.mp3").unwrap();
    assert_eq!(one.ledger.as_ref().unwrap().origin_flags, ADOPTED, "adopted, not verified");
    assert!(records.iter().any(|e| e.path == "Alex Artist/First Album/02 - Two (2).mp3"));
    let theirs = records.iter().any(|e| e.path == "Alex Artist/First Album/02 - Two.mp3");
    assert!(!theirs, "the listener's own");
}

// ── Recovery, skew, failures ────────────────────────────────────────────────

/// 2.12.3, and vectors.json's softwareMatching: a cut run's write target on
/// the card that the ledger doesn't list is replaced in place (no ` (2)`),
/// one no plan wants is deleted, and the folders it made become OWNED.
#[test]
fn a_cut_runs_targets_are_replaced_in_place_and_its_folders_owned() {
    let dir = Scratch::new("recover");
    let (card, mut wanted, mut server) = first(&dir.0);
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    let root = look(&card).root.unwrap().manifest;
    let ops = vec![
        Op { op: OP_FOLDER, path: "Casey Choir".into(), expected_size: 0 },
        Op { op: OP_FOLDER, path: "Casey Choir/Hymns".into(), expected_size: 0 },
        Op { op: OP_FOLDER, path: "Gone".into(), expected_size: 0 },
        Op { op: OP_WRITE, path: "Casey Choir/Hymns/01 - Six.mp3".into(), expected_size: 560 },
        Op { op: OP_WRITE, path: "Gone/x.mp3".into(), expected_size: 9 },
    ];
    let pending = mspd::write(&PlanIn {
        generation: root.header.generation + 1,
        card_id: CARD_ID,
        minor: 0,
        run_id: 77,
        base_generation: root.header.generation,
        ops,
    })
    .unwrap();
    put(&dir.0, "/.mstream/pending.bin", &pending, LATER);
    put(&dir.0, "/music/Casey Choir/Hymns/01 - Six.mp3", b"half of it", LATER);
    put(&dir.0, "/music/Gone/x.mp3", b"leftovers", LATER);
    let outcome = sync(&card, &wanted, &mut server, 8).unwrap();
    assert_eq!(outcome.verified, Ok(()));
    let image = card.image();
    let six_now = image["/music/Casey Choir/Hymns/01 - Six.mp3"];
    assert_eq!(six_now.0, six.len() as u64, "replaced in place");
    assert!(!image.keys().any(|p| p.contains("Six (2)")));
    let gone = card.stat("/music/Gone").unwrap();
    assert!(gone.is_none(), "its target deleted, its folder OWNED and collected");
    let folders = look(&card).root.unwrap().tags.unwrap().folders;
    let owned = |p: &str| folders.iter().any(|f| f.path == p && f.flags & FOLDER_OWNED != 0);
    assert!(owned("Casey Choir") && owned("Casey Choir/Hymns"));
}

/// 2.12.3 when the fresh copy doesn't come: a cut run's half-finished
/// target is never left on the card unclaimed, which the run after would
/// take for the listener's file and write ` (2)` beside. A run that parks
/// leaves it in the plan on the card; one whose fetch fails deletes it.
#[test]
fn a_cut_runs_target_is_never_left_for_the_listeners() {
    let dir = Scratch::new("recover-fails");
    let (card, mut wanted, mut server) = first(&dir.0);
    let six = corpus("trck_0.mp3");
    let (from, to) = ("Lib/Casey Choir/Hymns/01 - Six.mp3", "Casey Choir/Hymns/01 - Six.mp3");
    server.add(from, six.clone());
    wanted.files.push(track(from, &six, 401));
    let root = look(&card).root.unwrap().manifest;
    let ops = vec![
        Op { op: OP_FOLDER, path: "Casey Choir".into(), expected_size: 0 },
        Op { op: OP_FOLDER, path: "Casey Choir/Hymns".into(), expected_size: 0 },
        Op { op: OP_WRITE, path: to.into(), expected_size: 560 },
    ];
    let pending = mspd::write(&PlanIn {
        generation: root.header.generation + 1,
        card_id: CARD_ID,
        minor: 0,
        run_id: 77,
        base_generation: root.header.generation,
        ops,
    })
    .unwrap();
    put(&dir.0, "/.mstream/pending.bin", &pending, LATER);
    put(&dir.0, &format!("/music/{to}"), b"half of it", LATER);

    server.stop_at = Some(from.into());
    let parked = sync(&card, &wanted, &mut server, 8).unwrap();
    assert!(parked.parked.is_some());
    let plan = look(&card).pending.expect("the plan left on the card");
    assert!(plan.ops.iter().any(|o| o.op == OP_WRITE && o.path == to), "{:?}", plan.ops);
    assert!(card.stat(&format!("/music/{to}")).unwrap().is_some(), "still the software's");

    server.stop_at = None;
    server.failing.insert(from.into());
    let failed = sync(&card, &wanted, &mut server, 9).unwrap();
    assert_eq!((failed.verified, failed.deleted, failed.report.failed.len()), (Ok(()), 1, 1));
    assert!(card.stat(&format!("/music/{to}")).unwrap().is_none(), "the half-finished work, gone");
    assert!(look(&card).pending.is_none());

    server.failing.clear();
    let done = sync(&card, &wanted, &mut server, 10).unwrap();
    assert_eq!((done.written, done.verified), (1, Ok(())));
    assert!(records(&card).iter().any(|e| e.path == to));
    assert!(!card.image().keys().any(|p| p.contains("Six (2)")));
}

/// 2.8.3, step 6 at step 8.3: a file found at a new name's place since the
/// listing — the listener's, copied in under another case here; to a PC,
/// another file's 8.3 alias — is a collision. It stays as it is, and the
/// file waits for the next run, which names it around it.
#[test]
fn a_file_found_at_a_new_name_is_never_deleted() {
    let dir = Scratch::new("collision");
    let (card, mut wanted, mut server) = first(&dir.0);
    let six = corpus("trck_0.mp3");
    let from = "Lib/Alex Artist/First Album/03 - Six.mp3";
    server.add(from, six.clone());
    wanted.files.push(track(from, &six, 401));
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    assert_eq!(plan.fetches().0, 1);
    let theirs = corpus("latin1.mp3");
    let path = "/music/Alex Artist/First Album/03 - SIX.MP3";
    put(&dir.0, path, &theirs, LATER);
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.verified, outcome.written, outcome.deleted), (Ok(()), 0, 0));
    let failed: Vec<&str> = outcome.report.failed.iter().map(|f| f.0.as_str()).collect();
    assert_eq!(failed, [from]);
    assert_eq!(card.read(path, 1 << 20).unwrap(), Some(theirs.clone()), "the listener's, as it was");

    sync(&card, &wanted, &mut server, 5).unwrap();
    assert!(records(&card).iter().any(|e| e.path == "Alex Artist/First Album/03 - Six (2).mp3"));
    assert_eq!(card.read(path, 1 << 20).unwrap(), Some(theirs));
}

/// 2.11: a ledger file is the software's only while it is the one listed.
/// One the listener changes after the listing, before the run replaces or
/// moves it, is theirs: left as it is, out of the ledger, reported; one
/// they delete is gone from the ledger.
#[test]
fn a_file_changed_since_the_listing_is_neither_replaced_nor_moved() {
    let two = "Alex Artist/First Album/02 - Two.mp3";
    let three = "Alex Artist/Second Album/01 - Three.flac";
    for (step, deleted) in [("replace", false), ("move", false), ("move", true)] {
        let what = format!("{step}, deleted: {deleted}");
        let dir = Scratch::new("changed-since");
        let (card, mut wanted, mut server) = first(&dir.0);
        let rel = if step == "replace" {
            let bytes = corpus("v23_utf16.mp3");
            server.add("Lib/Alex Artist/First Album/02 - Two.mp3", bytes.clone());
            wanted.files[1] = track("Lib/Alex Artist/First Album/02 - Two.mp3", &bytes, 102);
            two
        } else {
            server.add(MOVED, server.files["Lib/Alex Artist/Second Album/01 - Three.flac"].clone());
            wanted.files[2].ledger.server_path = MOVED.into();
            three
        };
        let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
        let planned = (plan.fetches().0, plan.renames());
        assert_eq!(planned, if step == "replace" { (1, 0) } else { (0, 1) }, "{what}");
        let path = format!("/music/{rel}");
        let mut edited = card.read(&path, 1 << 20).unwrap().unwrap();
        edited.extend_from_slice(b"TAG edited by the listener");
        match deleted {
            true => std::fs::remove_file(dir.0.join(&path[1..])).unwrap(),
            false => put(&dir.0, &path, &edited, LATER),
        }
        let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!((outcome.written, outcome.renamed), (0, 0), "{what}");
        let report = if deleted { &outcome.report.gone } else { &outcome.report.spared };
        assert_eq!(report, &[rel], "{what}");
        let kept = card.read(&path, 1 << 20).unwrap();
        assert_eq!(kept, (!deleted).then_some(edited), "{what}: as the listener left it");
        let paths: Vec<String> = records(&card).into_iter().map(|e| e.path).collect();
        assert_eq!(paths.len(), 5, "{what}: {paths:?}");
        let name = rel.rsplit('/').next().unwrap();
        assert!(!paths.iter().any(|p| p.ends_with(name)), "{what}: {paths:?}");
    }
}

/// A plan of another card (a `/.mstream` copied from one) makes none of
/// this card's files the software's: the listener's file at one of its
/// targets stays, and the plan goes.
#[test]
fn a_plan_of_another_card_owns_nothing_here() {
    let dir = Scratch::new("foreign");
    let (card, wanted, mut server) = first(&dir.0);
    let root = look(&card).root.unwrap().manifest;
    let other = 0x0123_4567_89AB_CDEF;
    let ops = vec![Op { op: OP_WRITE, path: "Mine/song.mp3".into(), expected_size: 540 }];
    let pending = mspd::write(&PlanIn {
        generation: root.header.generation + 1,
        card_id: other,
        minor: 0,
        run_id: 78,
        base_generation: root.header.generation,
        ops,
    })
    .unwrap();
    put(&dir.0, "/.mstream/pending.bin", &pending, LATER);
    let mine = corpus("latin1.mp3");
    put(&dir.0, "/music/Mine/song.mp3", &mine, LATER);
    let extras = &mut Extras::default();
    let (ran, events) = sync_with(&card, &wanted, &mut server, extras, knobs(), Answers::default(), 6);
    assert_eq!(ran.unwrap().verified, Ok(()));
    assert!(events.contains(&Event::ForeignPlan(other)), "{events:?}");
    assert_eq!(card.read("/music/Mine/song.mp3", 1 << 20).unwrap(), Some(mine), "the listener's");
    assert!(look(&card).pending.is_none(), "the plan gone");
}

/// 2.3.4's skew: the PC lists every stamp an hour off (a time zone change);
/// with eight files or more the skew is found, and nothing is fetched again.
#[test]
fn stamps_an_hour_off_are_still_the_same_files() {
    let dir = Scratch::new("skew");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let mut files = Vec::new();
    let corpus_files = [
        "cbr.mp3",
        "v24_full.mp3",
        "latin1.mp3",
        "unicode.mp3",
        "v22.mp3",
        "trck_1.mp3",
        "tcon_0.mp3",
        "date_1.mp3",
        "numbers_0.mp3",
        "v1_only.mp3",
    ];
    for (i, name) in corpus_files.iter().enumerate() {
        let path = format!("Lib/Dana/Album/{i:02} - {name}");
        let bytes = corpus(name);
        server.add(&path, bytes.clone());
        files.push(track(&path, &bytes, 500 + i as u32));
    }
    let wanted = WantedCard { origins: vec![home()], files };
    sync(&card, &wanted, &mut server, 1).unwrap();
    let before = records(&card);
    card.set_skew(3600);
    let fetched = server.fetched.len();
    let outcome = sync(&card, &wanted, &mut server, 2).unwrap();
    assert_eq!((outcome.written, outcome.records), (0, 10));
    assert_eq!(server.fetched.len(), fetched, "nothing fetched again");
    let after = records(&card);
    for (a, b) in before.iter().zip(&after) {
        let moved = fat_wall(b.rec.fat_time).unwrap() - fat_wall(a.rec.fat_time).unwrap();
        assert_eq!(moved, 3600, "{}", a.path);
        assert_eq!((a.rec.qfp, &a.path), (b.rec.qfp, &b.path));
    }
}

#[test]
fn a_failed_file_is_reported_and_a_stopped_source_parks_the_run() {
    let dir = Scratch::new("failures");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    server.failing.insert("Lib/Alex Artist/First Album/02 - Two.mp3".into());
    let outcome = sync(&card, &wanted, &mut server, 1).unwrap();
    assert_eq!(outcome.verified, Ok(()));
    assert_eq!(outcome.records, 5);
    let two = "Lib/Alex Artist/First Album/02 - Two.mp3".to_string();
    assert_eq!(outcome.report.failed, [(two, "the server said 500".to_string())]);

    // The next run writes it; then one stopped by its source parks after a
    // checkpoint, its plan left on the card for the run after.
    server.failing.clear();
    let six = corpus("trck_0.mp3");
    let mut more = wanted.clone();
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    more.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    server.stop_at = Some("Lib/Casey Choir/Hymns/01 - Six.mp3".into());
    let parked = sync(&card, &more, &mut server, 2).unwrap();
    assert_eq!(parked.parked.as_deref(), Some("the day's quota is spent"));
    let seen = look(&card);
    assert!(seen.pending.is_some(), "the device says the last transfer didn't finish");
    assert!(!seen.root.as_ref().unwrap().manifest.is_final(), "a checkpoint");
    assert_eq!(seen.root.unwrap().tags.unwrap().records.len(), 6, "Two is in");
    assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "let go");
    server.stop_at = None;
    let done = sync(&card, &more, &mut server, 3).unwrap();
    assert_eq!((done.written, done.records, done.verified), (1, 7, Ok(())));
}

#[test]
fn a_session_says_what_the_card_holds_and_covers_follow_the_hook() {
    let dir = Scratch::new("session");
    let (card, wanted, mut server) = first(&dir.0);
    let label = r#"{"format": "mstream-card-state", "version": 1, "seq": 3, "label": "Car"}"#;
    put(&dir.0, "/.mstream/state.json", label.as_bytes(), LATER);
    let session = open(&card, Answers::default(), 11, &mut |_| {}).unwrap();
    assert!(session.device().present && session.device().writes_mpdj());
    let state = session.state().value.as_ref().expect("the card's settings");
    assert_eq!((state["label"].as_str(), session.state().seq), (Some("Car"), 3));
    session.close();

    // An album whose art went: its thumbnail goes, THUMB with it.
    let thumbs = |card: &FatSim| card.image().keys().filter(|p| p.ends_with(".565")).count();
    assert_eq!(thumbs(&card), 4);
    let mut extras = Extras { gone: Some("Blake Band/Live"), ..Extras::default() };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut extras, knobs(), Answers::default(), 12);
    assert_eq!(ran.unwrap().verified, Ok(()));
    assert_eq!(thumbs(&card), 3);
    let folders = look(&card).root.unwrap().tags.unwrap().folders;
    let live = folders.iter().find(|f| f.path == "Blake Band/Live").unwrap();
    assert_eq!(live.flags, FOLDER_OWNED);

    // The system's clock and ids.
    let mut system = Env::system();
    assert!(system.id() != 0 && system.now() > 1_790_000_000);
    assert_eq!(system.producer, format!("mstream-terminal {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn a_new_card_id_commits_the_same_files_and_is_a_run_of_its_own() {
    let dir = Scratch::new("new-id");
    let (card, mut wanted, mut server) = first(&dir.0);
    let before = look(&card).root.unwrap();
    let (old_id, old) = (before.manifest.header.card_id, finished(&card));
    assert!(old.autodj, "the first sync's index");
    let answers = Answers { new_card_id: true, ..Answers::default() };

    // Beside a file to write, refused before anything is written: a cut
    // would leave the plan under one id and the root under another.
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    let (session, plan) = plan_of(&card, &wanted, &mut server, answers, knobs());
    let image = card.image();
    let stop = session.run(plan, &mut server, &mut Extras::default(), &mut |_| {}).unwrap_err();
    assert!(matches!(stop, Stop::Refused(_)), "{stop}");
    let image_now = card.image();
    let kept = |(p, f): (&String, &(u64, u32, u64))| p.ends_with("run.lock") || image_now.get(p) == Some(f);
    assert!(image.iter().all(kept) && !image_now.contains_key("/.mstream/run.lock"));

    // Alone: the same files under a new id. The index the old id's was is
    // named no more; the hook builds one for the new id.
    wanted.files.pop();
    let (session, plan) = plan_of(&card, &wanted, &mut server, answers, knobs());
    let mut events = Vec::new();
    let mut extras = Extras::default();
    let done = session.run(plan, &mut server, &mut extras, &mut |e| events.push(e)).unwrap();
    assert_eq!((done.written, done.deleted, done.renamed, done.verified), (0, 0, 0, Ok(())));
    assert!(done.card_id != old_id && done.card_id != 0);
    let generation = before.manifest.header.generation + 1;
    assert_eq!(events, [Event::Committed { generation, last: true }]);
    let root = look(&card).root.unwrap();
    let ids = (root.manifest.header.card_id, root.tags.as_ref().unwrap().header.card_id);
    assert_eq!(ids, (done.card_id, done.card_id), "the root and its tags file carry it");
    let index = |r: &Root| r.manifest.autodj.as_ref().map(|c| c.name());
    assert!(index(&root).is_some() && index(&root) != index(&before), "an index of its own");
    assert_eq!(finished(&card), old, "the same music, records, folders and an index");
    // And a plain run after it changes nothing more.
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    let again = session.run(plan, &mut server, &mut Extras::default(), &mut |_| {}).unwrap();
    assert_eq!((again.card_id, again.commit_id), (done.card_id, done.commit_id));
}
