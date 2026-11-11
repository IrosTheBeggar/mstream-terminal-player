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
use std::io::{Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use super::fatsim::{Cut, FatSim};
use super::run::read_pieces;
use super::*;
use crate::device::card::autodj::{self, FileIn, Index, IndexIn, Model, Selection, Work};
use crate::device::card::contract::container;
use crate::device::card::contract::fat::fat_wall;
use crate::device::card::contract::moods::{self, NO_ROW, Scores, Tail};
use crate::device::card::contract::mptg::{
    self, ADOPTED, BPM_ANALYSED, CONTAINER_MP3, CONTAINER_NOT_AUDIO, CONTAINER_OPUS, FOLDER_OWNED,
    FOLDER_THUMB, Meta, Record, RecordIn, SOURCE_SOFTWARE, SOURCE_SYNC_AGENT, UNREADABLE, VERIFIED,
};
use crate::device::card::contract::msmf::{self, Comp};
use crate::device::card::contract::mspd::{self, OP_FOLDER, OP_WRITE, Op, PlanIn};
use crate::device::card::contract::mpwf::{self, bars_from};
use crate::device::card::contract::prim::{fnv1a64, md5, path_hash, qfp_of, u32_at};
use crate::device::card::contract::server::{NO_INSTANCE, TalkPeer, server_url_key};
use crate::device::card::tags::PARSER_VERSION;
use crate::device::card::volume::{self, CardFile, Inspection, Item, LockFile, Serial};
use crate::device::card::waves::{self, Shapes};

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
    /// A later run takes up what a run leaves of its files (2.12.8): each
    /// answered under its bytes' ETag, as `/media` is.
    resumable: bool,
    /// The fetch of this file into a partial stops once this many of its
    /// bytes came (once): the run parks when true, else the file is
    /// skipped.
    cut_after: Option<(String, usize, bool)>,
    /// Each fetch into a partial: the file, the byte it asked from, and the
    /// byte the answer went on from.
    asked: Vec<(String, u64, u64)>,
}

/// The ETag the canned server gives a file's bytes.
fn etag_of(bytes: &[u8]) -> String {
    format!("W/\"{:x}\"", fnv1a64(bytes))
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

    fn resumable(&self, _file: &WantedFile) -> bool {
        self.resumable
    }

    /// If-Range as `/media` answers it: the rest when the partial's
    /// validator is the file's ETag now, else the whole file; nothing when
    /// it holds the whole file already (a 416).
    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        let path = file.ledger.server_path.clone();
        self.fetched.push(path.clone());
        if self.stop_at.as_ref() == Some(&path) {
            return Err(Failed { why: "the day's quota is spent".into(), stop: true });
        }
        if self.failing.contains(&path) {
            return Err(Failed { why: "the server said 500".into(), stop: false });
        }
        let bytes = self.bytes(file);
        let etag = etag_of(&bytes);
        let fresh = have.filter(|h| h.validator == etag);
        let from = fresh.map_or(0, |h| h.bytes as usize).min(bytes.len());
        self.asked.push((path.clone(), have.map_or(0, |h| h.bytes), from as u64));
        let failed = |e: io::Error| Failed { why: e.to_string(), stop: false };
        if from > 0 && from == bytes.len() {
            return Ok(Fetched::default());
        }
        into.answered(from as u64, Some(&etag)).map_err(failed)?;
        let cut = self.cut_after.take_if(|(p, _, _)| *p == path);
        for (k, piece) in bytes[from..].chunks(1000).enumerate() {
            if let Some((_, n, stop)) = &cut
                && k * 1000 >= *n
            {
                return Err(Failed { why: "the day's quota is spent".into(), stop: *stop });
            }
            into.write_all(piece).map_err(failed)?;
        }
        Ok(Fetched::default())
    }
}

/// What the tests' hooks add: a thumbnail for every album without one (a
/// flat colour of its folder's hash and its count of files), none for the
/// album `gone` names, an Auto DJ index of made-up vectors under a made-up
/// model (no real embeddings: AUTODJ-INDEX.md 12.5), a waveform index of
/// made-up peaks, and the mood tails of made-up scores; and a count of the
/// plays hook's calls.
#[derive(Default)]
struct Extras {
    plays: usize,
    gone: Option<&'static str>,
    /// The recordings the waveform step asked for, in order.
    asked: Vec<[u8; 16]>,
    /// The fetch stops once this many are answered (a stop, a token, the
    /// network, a time budget); None: all of them.
    answers: Option<usize>,
    /// Its index steps may keep the card waiting long.
    long: bool,
    /// The mood scores the mood step's fetch brings, by server path (None:
    /// the server has no row), as the sync's overlay lays them: a record
    /// whose server path they name takes that row's tail, the rest keep
    /// theirs. None: no fetch, every tail kept.
    scores: Option<HashMap<String, Option<Scores>>>,
    /// The mood step clears every tail (a card for someone else).
    clear: bool,
    /// How many commits asked the mood step.
    moods_asked: usize,
}

impl Extras {
    /// The tests' hooks, the mood step's fetch bringing [`scores_of`] the
    /// selection.
    fn scored(wanted: &WantedCard) -> Extras {
        Extras { scores: Some(scores_of(wanted, 0)), ..Extras::default() }
    }
}

/// A made-up mood row for each file of the selection, from its id and
/// `salt`: hundredths, so the byte rule's slack is at work (0.29 and 0.58
/// at salt 0, 0.57 at salt 1), and a NULL head for one file in three.
fn scores_of(wanted: &WantedCard, salt: u32) -> HashMap<String, Option<Scores>> {
    let row = |id: u32| -> Scores {
        let score = |k: u32| ((id * 7 + k * 29 + salt * 11) % 101) as f64 / 100.0;
        std::array::from_fn(|k| (id % 3 != 0 || k != moods::PARTY).then(|| score(k as u32)))
    };
    let files = wanted.files.iter();
    files.map(|f| (f.ledger.server_path.clone(), Some(row(f.ledger.mstream_id)))).collect()
}

/// A recording's made-up peaks, the server's M = 800: its hash over and over.
fn peaks_of(hash: &[u8; 16]) -> Vec<u8> {
    (0..800).map(|i| hash[i % 16].wrapping_add((i / 16) as u8)).collect()
}

/// The bars a file of this ledger row's recording has, wherever they came from.
fn bars_of(ledger: &Ledger) -> [u8; mpwf::BAR_COUNT] {
    bars_from(&peaks_of(&ledger.canonical_hash().expect("a recording")))
}

impl Hooks for Extras {
    fn plays(&mut self, _card: &dyn CardFs, _root: Option<&Root>, _skew: i32) {
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

    fn long_index_steps(&self) -> bool {
        self.long
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

    fn waves(&mut self, commit: &Commit<'_>, events: &mut dyn FnMut(Event)) -> Waves {
        let asks = match commit.device.writes_mpwf() {
            true => waves::asks(commit.records, commit.old_waves, |_| true),
            false => Vec::new(),
        };
        let mut fetched = Shapes::new();
        for (k, ask) in asks.iter().take(self.answers.unwrap_or(asks.len())).enumerate() {
            self.asked.push(ask.hash);
            fetched.insert(ask.hash, bars_from(&peaks_of(&ask.hash)));
            events(Event::Waves { done: k + 1, of: asks.len() });
        }
        Waves::from(commit.waves_index(&fetched).expect("a waveform index"))
    }

    fn moods(&mut self, commit: &Commit<'_>) -> Moods {
        self.moods_asked += 1;
        if self.clear {
            return Moods::Clear;
        }
        let Some(scores) = &self.scores else { return Moods::Keep };
        let tail = |r: &RecordIn| {
            let row = scores.get(&r.ledger.as_ref()?.server_path)?;
            Some((r.path.clone(), moods::encode(row.as_ref())))
        };
        Moods::Overlay(commit.records.iter().filter_map(tail).collect())
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

/// Small checkpoints, so a run of a few files makes several; a partial
/// listed again every 4 KiB.
fn knobs() -> Knobs {
    Knobs { checkpoint_files: 2, part_bytes: 4096, deep_check: true, ..Knobs::default() }
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
    recover_with(card, wanted, server, seed, knobs())
}

/// [`recover`], under these knobs.
fn recover_with(
    card: &FatSim,
    wanted: &WantedCard,
    server: &mut Canned,
    seed: u64,
    knobs: Knobs,
) -> Recovered {
    recover_under(card, wanted, server, seed, knobs, Extras::default)
}

/// [`recover_with`], each of its runs under the hooks `extras` makes.
fn recover_under(
    card: &FatSim,
    wanted: &WantedCard,
    server: &mut Canned,
    seed: u64,
    knobs: Knobs,
    extras: impl Fn() -> Extras,
) -> Recovered {
    let start = card.log().len();
    let mut answers = Answers::default();
    loop {
        let before = card.log().len() - start;
        let (hooks, expect, knobs) = (&mut extras(), Expect::default(), knobs.clone());
        let opened = Session::open(card, &expect, knobs, &answers, env(seed), hooks, &mut |_| {});
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
/// must read with its tags file and indexes whole.
fn look(card: &FatSim) -> Inspection {
    let seen = volume::inspect(card).expect("the card lists");
    if let Some(root) = &seen.root {
        assert!(root.tags.is_ok(), "the root's tags file: {:?}", root.tags.as_ref().err());
        assert!(root.autodj.is_none_or(|v| v.is_ok()), "the root's Auto DJ index: {:?}", root.autodj);
        assert!(root.waves.is_none_or(|v| v.is_ok()), "the root's waveform index: {:?}", root.waves);
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
    // The server: Two another recording, of another length (WAVEFORM-INDEX.md
    // 7.3), Loose gone, Three moved to another album, a new album.
    let two = corpus("mpeg2.mp3");
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
/// `/music`'s files, the root's records (their mood tails among them) and
/// folders, and its waveform index's slots (a file of one commit's is
/// another's, but for its header).
#[derive(Debug, PartialEq, Eq)]
struct Finished {
    music: BTreeMap<String, (u64, u64)>,
    records: Vec<(String, u32, u64, String, u8, u16, Tail)>,
    folders: Vec<(String, u32)>,
    autodj: bool,
    waves: Option<Vec<u8>>,
}

/// The bytes of the waveform index the root names, every slot of it good and
/// no other waveform index on the card.
fn waves_file(card: &FatSim) -> Option<Vec<u8>> {
    let root = look(card).root.expect("a root");
    let name = root.manifest.waves.as_ref().map(|c| format!("/.mstream/{}", c.name()));
    let image = card.image();
    let others = image.keys().filter(|p| p.starts_with("/.mstream/waves-") && Some(*p) != name.as_ref());
    assert_eq!(others.collect::<Vec<_>>(), Vec::<&String>::new(), "a file no root names");
    let bytes = card.read(name.as_ref()?, usize::MAX).expect("it reads").expect("the file the root names");
    let table = mpwf::Table::open(&bytes).expect("it opens");
    table.check().expect("every slot of it");
    Some(bytes)
}

/// The rows of the waveform index the root names, by path hash.
fn wave_rows(card: &FatSim) -> BTreeMap<u64, mpwf::Slot> {
    let Some(bytes) = waves_file(card) else { return BTreeMap::new() };
    let table = mpwf::Table::open(&bytes).expect("it opens");
    (0..table.rows).map(|r| table.slot(r).expect("its slot")).map(|s| (s.path_hash, s)).collect()
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
        .chain(image.keys().filter(|p| p.contains("/resume/") || p.ends_with("resume.new")))
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
            let (rec, path) = (&e.rec, e.path.clone());
            (path, rec.size, rec.qfp, l.server_path, l.origin_flags, l.server, rec.moods)
        })
        .collect();
    let flagged = tags.folders.iter().filter(|f| f.flags != 0);
    let folders = flagged.map(|f| (f.path.clone(), f.flags)).collect();
    let waves = waves_file(card).map(|bytes| {
        let table = mpwf::Table::open(&bytes).expect("it opens");
        table.wvsl().to_vec()
    });
    Finished { music, records, folders, autodj: root.autodj.is_some(), waves }
}

// ── The simulated card ──────────────────────────────────────────────────────

/// A large file that isn't the stream its name says (zeros named `.mp3`):
/// its tags are read through a buffer, never a card read a byte.
#[test]
fn a_large_file_that_isnt_its_stream_reads_its_tags_in_large_reads() {
    let scratch = Scratch::new("tags-reads");
    set_up(&scratch.0);
    let card = FatSim::new(&scratch.0, SERIAL);
    let bytes = vec![0u8; 4 << 20];
    let mut server = Canned::default();
    server.add("Lib/Zeros/01.mp3", bytes.clone());
    let wanted = WantedCard { origins: vec![home()], files: vec![track("Lib/Zeros/01.mp3", &bytes, 101)] };
    let before = card.reads();
    let outcome = sync(&card, &wanted, &mut server, 1).expect("it runs");
    assert_eq!((outcome.written, outcome.verified), (1, Ok(())));
    let reads = card.reads() - before;
    assert!(reads < 5_000, "{reads} reads of the card");
}

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
/// stay. The root, where the case has one, is the fixture card's that names
/// the vector's three companions, `manifest-waves.bin`.
#[test]
fn the_clean_up_deletes_what_the_vectors_say() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/vectors.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let cases = v["privateFiles"]["cleanup"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    let golden = ["tags-0000002a.bin", "autodj-00000029.bin", "waves-0000002a.bin"];
    for (k, c) in cases.iter().enumerate() {
        let dir = Scratch::new(&format!("cleanup{k}"));
        let files = c["files"].as_object().unwrap();
        for (rel, size) in files {
            let host = dir.0.join(".mstream").join(rel);
            std::fs::create_dir_all(host.parent().unwrap()).unwrap();
            if rel == "manifest.bin" {
                std::fs::write(&host, fixture("card/golden/manifest-waves.bin")).unwrap();
            } else if golden.contains(&rel.as_str()) {
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
        // The file the root names, the run's to reuse; a card's highest
        // generation counts any companion's name.
        let waves = session.root().map(|r| r.waves);
        let want = files.contains_key("manifest.bin").then_some(Some(Ok(())));
        assert_eq!(waves, want, "case {k}: the old waveform index");
        if files.contains_key("manifest.bin") {
            assert_eq!(session.settled.top, 43, "case {k}: the root's");
        }
        session.close();
        assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "case {k}: the card let go");
    }
}

/// 2.5.2: no generation is used twice, whichever companion's name shows it,
/// and a waveform index the root doesn't name goes at step 4 (2.12.1).
#[test]
fn a_waveform_index_counts_by_its_name() {
    let generation = super::settle::companion_generation;
    assert_eq!(generation("waves-0000002a.bin"), Some(42));
    assert_eq!(generation("autodj-00000029.bin"), Some(41));
    assert_eq!(generation("tags-ffffffff.bin"), Some(u32::MAX));
    for name in ["waves-0000002A.bin", "waves-2a.bin", "waves-old.bin", "wave-0000002a.bin", "waves-0000002a.tmp"] {
        assert_eq!(generation(name), None, "{name}");
    }

    let dir = Scratch::new("waves-name");
    set_up(&dir.0);
    put(&dir.0, "/.mstream/waves-00000063.bin", &fixture("card/golden/waves-one.bin"), LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let session = open(&card, Answers::default(), 1, &mut |_| {}).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(session.settled.top, 0x63, "a name is never used again");
    assert_eq!(session.root().map(|r| r.waves), Some(None), "the root names none");
    session.close();
    assert!(card.stat("/.mstream/waves-00000063.bin").unwrap().is_none(), "a companion the root doesn't name");
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
    assert_eq!(extras.plays, 1, "the plays hook, once, before anything is planned");
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
    assert!(want.waves.is_some() && writes.iter().any(|w| w.starts_with("create /.mstream/waves-")));

    // Each write cut in turn; each rename also between its two directory
    // writes.
    let mut disk_checks = 0;
    let mut replaced = 0;
    let two = recording_of(&wanted, "Lib/Alex Artist/First Album/02 - Two.mp3");
    let mut settled: BTreeSet<(String, bool)> = BTreeSet::new();
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
        // What the card holds is a commit: the one before, a checkpoint, or
        // the last.
        let seen = look(&card);
        assert!(seen.root.is_some(), "{what}: a root");
        dump(&format!("cut-{n:03}-{how:?}"), &card);
        // A checkpoint with Two replaced names the old file again, whose slot
        // of Two is the old recording's (WAVEFORM-INDEX.md 7.3).
        let root = seen.root.as_ref().filter(|r| !r.manifest.is_final() && r.waves == Some(Ok(())));
        let tags = root.and_then(|r| r.tags.as_ref().ok());
        let has = |e: &mptg::Entry| e.ledger.as_ref().and_then(Ledger::canonical_hash) == Some(two);
        replaced += usize::from(tags.is_some_and(|t| t.records.iter().any(has)));

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
    assert!(replaced > 0, "a run taken up from a checkpoint with a recording replaced before it");
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

/// A dry run's card takes no place in this process's hold on the card (the
/// review's TV2): a real run opens beside a dry one's session, and a dry one
/// beside a real one's, as a fit worked out while the settings are applied.
#[test]
fn a_dry_run_keeps_no_real_run_off_the_card() {
    let dir = Scratch::new("dry-beside");
    let (card, ..) = first(&dir.0);
    fn open_on<'c>(card: &'c dyn CardWrite) -> Result<Session<'c>, Stop> {
        Session::open(card, &Expect::default(), knobs(), &Answers::default(), env(8), &mut NoHooks, &mut |_| {})
    }
    let dry = crate::device::card::dry::DryCard::new(&card);
    let dry_session = open_on(&dry).expect("a dry session");
    let real = open(&card, Answers::default(), 7, &mut |_| {}).expect("a real run beside a dry one");
    let again = crate::device::card::dry::DryCard::new(&card);
    let beside = open_on(&again).expect("a dry one beside a real one");
    drop((beside, real, dry_session));
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
    let (session, plan) = plan_of(&card, &wanted, &mut server, yes, strict.clone());
    assert!(plan.guards.is_empty());
    session.close();
    // A yes that named its count passes that count only: the server's list
    // changed since the look asks again.
    let two = Answers { removals_seen: Some(2), ..yes };
    let (session, plan) = plan_of(&card, &wanted, &mut server, two, strict.clone());
    let asked = Guard::Removals { paths: vec!["Loose.mp3".into()], strong: false };
    assert_eq!(plan.guards, [asked], "1 removal, not the 2 allowed");
    session.close();
    let one = Answers { removals_seen: Some(1), ..yes };
    let (session, plan) = plan_of(&card, &wanted, &mut server, one, strict);
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
    // Its count is its paths': 5 passes, 4 asks again.
    let four = Answers { removals_seen: Some(4), ..all };
    let (session, plan) = plan_of(&card, &gone, &mut server, four, knobs());
    assert!(matches!(&plan.guards[..], [Guard::Removals { strong: true, .. }]), "{:?}", plan.guards);
    session.close();
    let five = Answers { removals_seen: Some(5), ..all };
    let (session, plan) = plan_of(&card, &gone, &mut server, five, knobs());
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
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), never.clone());
    assert!(plan.guards.is_empty() && plan.removals().is_empty());
    assert_eq!(plan.report.kept, ["Loose.mp3"]);
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.deleted, outcome.records), (0, 6));
    // But for a file the card's multi-card set lets go (B7): it leaves, past
    // the gate as any removal.
    let (mut session, _) = plan_of(&card, &wanted, &mut server, Answers::default(), never);
    session.let_go(["Loose.mp3".to_string()].into());
    let plan = session.plan(&wanted, &Answers::default(), &mut server).unwrap();
    let gate = Guard::Removals { paths: vec!["Loose.mp3".into()], strong: false };
    assert_eq!((&plan.guards[..], plan.report.kept.len()), (&[gate][..], 0));
    let plan = session.plan(&wanted, &yes, &mut server).unwrap();
    assert_eq!(plan.removals(), ["Loose.mp3"]);
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.deleted, outcome.records), (1, 5));
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

/// 2.11 once the server gives its own id: the id goes into the root when
/// the run reads that server, decides at any address, and outweighs every
/// key; and two servers never share one key in SRVS (2.5.5).
#[test]
fn a_server_is_known_by_its_own_id_at_any_address_and_never_by_another_one() {
    let dir = Scratch::new("instances");
    let (card, wanted, mut server) = first(&dir.0);
    let (a, b) = ([0xA1; 16], [0xB2; 16]);
    let with = |url: &str, instance: [u8; 16]| {
        let mut o = origin(url);
        (o.talk.instance, o.server.instance) = (instance, instance);
        WantedCard { origins: vec![o], files: wanted.files.clone() }
    };
    let root = |card: &FatSim| look(card).root.unwrap().manifest.server_table();
    let (home, tunnel) = ("http://music.example:3000", "https://tunnel.example");

    // The card's server, first read with its id: by its key, and the id is
    // kept from then on. Nothing changes but the root.
    let outcome = sync(&card, &with(home, a), &mut server, 2).unwrap();
    assert_eq!((outcome.written, outcome.deleted, outcome.verified.clone()), (0, 0, Ok(())));
    assert_eq!(root(&card).root, Server::root(a, server_url_key(home)));

    // At another address: the same server by its id, with nothing asked,
    // and the root takes the address it was read at.
    let (session, plan) = plan_of(&card, &with(tunnel, a), &mut server, Answers::default(), knobs());
    assert!(plan.guards.is_empty(), "{:?}", plan.guards);
    assert_eq!((plan.fetches().0, plan.kept().0), (0, 6));
    session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!(root(&card).root, Server::root(a, server_url_key(tunnel)));

    // Another server at that address: never this card's, whatever the
    // listener confirmed of its key, and a server of its own can't share
    // the root's key. Refused, before anything is written.
    let mut other = with(tunnel, b);
    other.origins[0].talk.confirmed = vec![server_url_key(home)];
    let image = card.image();
    let new = Answers { new_servers: true, ..Answers::default() };
    for answers in [Answers::default(), new] {
        let mut session = open(&card, answers, 3, &mut |_| {}).unwrap();
        let refused = session.plan(&other, &answers, &mut server);
        assert!(matches!(&refused, Err(Stop::Refused(why)) if why.contains("address")), "{:?}", refused.err());
        session.close();
    }
    assert_eq!(card.image(), image, "nothing written");

    // At an address of its own: asked about (the card's files are another
    // server's), then a second server, in SRVS with its id.
    let third = with("https://third.example", b);
    let (session, plan) = plan_of(&card, &third, &mut server, Answers::default(), knobs());
    assert_eq!(plan.guards, [Guard::Server { origin: 0, rows: 6 }]);
    session.close();
    let (session, plan) = plan_of(&card, &third, &mut server, new, knobs());
    assert!(plan.guards.is_empty());
    session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    let table = root(&card);
    assert_eq!(table.root, Server::root(a, server_url_key(tunnel)));
    assert_eq!(table.entries, [Server::direct(b, server_url_key("https://third.example"))]);
}

/// The server set beside the root at its tunnel before it gave an id (the
/// listener's "a server of its own"), read at home once it gives one, then
/// at the tunnel: the root by its id there, the entry by its key, so one
/// server. The entry's rows join the root's: a file both hold stays under
/// the root's row and its copy goes, and SRVS loses the entry in that
/// commit (2.5.5) — never a refusal for two servers under one key.
#[test]
fn two_of_the_cards_servers_one_by_its_id_are_one_from_then_on() {
    let dir = Scratch::new("joined");
    let (card, wanted, mut server) = first(&dir.0);
    let a = [0xA1; 16];
    let with = |url: &str, instance: [u8; 16]| {
        let mut o = origin(url);
        (o.talk.instance, o.server.instance) = (instance, instance);
        WantedCard { origins: vec![o], files: wanted.files.clone() }
    };
    let root = |card: &FatSim| look(card).root.unwrap().manifest.server_table();
    let of = |card: &FatSim, server: u16| -> Vec<String> {
        let rows = records(card).into_iter().filter(|e| e.ledger.as_ref().unwrap().server == server);
        rows.map(|e| e.path).collect()
    };
    let (home, tunnel) = ("http://music.example:3000", "https://tunnel.example");
    let new = Answers { new_servers: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &with(tunnel, NO_INSTANCE), &mut server, new, knobs());
    session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    let (ours, copies) = (of(&card, 0), of(&card, 1));
    assert_eq!((ours.len(), copies.len()), (6, 6));

    // At home with its id: the root takes it, and the tunnel's entry, which
    // the id can't tell there, is carried.
    let outcome = sync(&card, &with(home, a), &mut server, 7).unwrap();
    assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 12));
    let table = root(&card);
    assert_eq!(table.root, Server::root(a, server_url_key(home)));
    assert_eq!(table.entries, [Server::direct(NO_INSTANCE, server_url_key(tunnel))]);

    // At the tunnel: one server, nothing asked.
    let (session, plan) = plan_of(&card, &with(tunnel, a), &mut server, Answers::default(), knobs());
    assert!(plan.guards.is_empty(), "{:?}", plan.guards);
    assert_eq!((plan.fetches().0, plan.kept().0), (0, 6));
    let mut removed: Vec<String> = plan.removals().into_iter().map(String::from).collect();
    removed.sort();
    let mut copied = copies.clone();
    copied.sort();
    assert_eq!(removed, copied, "the copies go, the root's rows stay");
    session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    let table = root(&card);
    assert_eq!((table.root, table.entries.len()), (Server::root(a, server_url_key(tunnel)), 0));
    assert_eq!((of(&card, 0), of(&card, 1).len()), (ours, 0));

    // Either address from then on: the root's server alone.
    for (url, seed) in [(home, 8), (tunnel, 9)] {
        let outcome = sync(&card, &with(url, a), &mut server, seed).unwrap();
        assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 6));
    }
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
    assert_eq!(outcome.report.failed, [(two.clone(), "the server said 500".to_string())]);
    // Its server's revision isn't the card's: the next run reads it whole,
    // not a 304 that would have the ledger, which lacks Two, stand for it.
    assert_eq!(outcome.missed, [(0, two)]);
    let revision = |card: &FatSim| look(card).root.unwrap().manifest.server_revision;
    assert_eq!(revision(&card), "", "a final commit that missed a file names no revision");

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
    assert_eq!(seen.root.as_ref().unwrap().tags.as_ref().unwrap().records.len(), 6, "Two is in");
    assert_eq!(revision(&card), "", "a checkpoint names no revision: its plan's rest isn't in");
    assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "let go");
    server.stop_at = None;
    let done = sync(&card, &more, &mut server, 3).unwrap();
    assert_eq!((done.written, done.records, done.verified), (1, 7, Ok(())));
    assert!(done.missed.is_empty());
    assert_eq!(revision(&card), "\"r-6-1\"", "every file in: the server's revision");
}

/// Hooks that give a run's end the card's settings (B1's): a state file
/// labelled `label`; the card pulled as it is written when `pull`.
struct Stores<'a> {
    card: &'a FatSim,
    label: &'static str,
    pull: bool,
    seq: Option<u64>,
}

impl Hooks for Stores<'_> {
    fn state(&mut self, _outcome: &Outcome, seq: u64) -> Option<Vec<u8>> {
        self.seq = Some(seq);
        if self.pull {
            self.card.cut_at(1, Cut::Fail);
        }
        let (head, label) = (r#""format": "mstream-card-state", "version": 1"#, self.label);
        Some(format!(r#"{{{head}, "seq": {seq}, "label": "{label}"}}"#).into_bytes())
    }
}

/// The tests' hooks, which acknowledge plays too: each ACKS entry raised to
/// the marks.
struct Acking(Vec<Ack>, Extras);

impl Hooks for Acking {
    fn cover(&mut self, album: &Album<'_>) -> Cover {
        self.1.cover(album)
    }

    fn autodj(&mut self, commit: &Commit<'_>) -> AutoDj {
        self.1.autodj(commit)
    }

    fn acks(&mut self, carried: &[Ack]) -> Vec<Ack> {
        let mut out = carried.to_vec();
        for mark in &self.0 {
            match out.iter_mut().find(|a| a.key() == mark.key()) {
                Some(a) => a.seq = a.seq.max(mark.seq),
                None => out.push(*mark),
            }
        }
        out
    }
}

/// ACKS ride on a commit a run makes anyway (2.5.6): plays to acknowledge
/// alone make no commit, since every commit has the device build its
/// library again; the next commit takes them, and the one after carries
/// them forward.
#[test]
fn acknowledged_plays_ride_on_a_commit_and_never_make_one() {
    let dir = Scratch::new("acks");
    let (card, mut wanted, mut server) = first(&dir.0);
    let root = |card: &FatSim| card.image()["/.mstream/manifest.bin"];
    let before = root(&card);
    let ack = Ack { device_id: [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01], epoch: 0x1122_3344_5566_7788, seq: 3 };
    let mut acking = Acking(vec![ack], Extras::default());
    let (ran, events) = sync_with(&card, &wanted, &mut server, &mut acking, knobs(), Answers::default(), 5);
    assert_eq!(ran.expect("a run").written, 0);
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(root(&card), before, "no commit for the acknowledgements alone");
    assert!(look(&card).root.expect("a root").manifest.acks.is_empty());

    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut acking, knobs(), Answers::default(), 6);
    assert_eq!(ran.expect("a run").written, 1);
    let root = look(&card).root.expect("a root");
    assert_eq!(root.manifest.acks, [ack], "the commit made anyway takes them");

    // A run that reads no log carries them unchanged; a hook never lowers
    // one.
    let seven = corpus("v23_utf16.mp3");
    server.add("Lib/Casey Choir/Hymns/02 - Seven.mp3", seven.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/02 - Seven.mp3", &seven, 402));
    assert_eq!(sync(&card, &wanted, &mut server, 7).expect("a run").written, 1);
    assert_eq!(look(&card).root.expect("a root").manifest.acks, [ack]);
    let mut lower = Acking(vec![Ack { seq: 1, ..ack }], Extras::default());
    wanted.files.pop();
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut lower, knobs(), Answers::default(), 8);
    assert_eq!(ran.expect("a run").deleted, 1);
    assert_eq!(look(&card).root.expect("a root").manifest.acks, [ack]);
}

#[test]
fn a_runs_end_stores_the_settings_it_is_given_before_it_lets_the_card_go() {
    let dir = Scratch::new("settings");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut hooks = Stores { card: &card, label: "Car", pull: false, seq: None };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut hooks, knobs(), Answers::default(), 1);
    let outcome = ran.unwrap();
    assert_eq!((outcome.verified, outcome.stored), (Ok(()), Ok(())));
    let seen = look(&card);
    let label = seen.state.value.as_ref().and_then(|v| v["label"].as_str().map(str::to_string));
    assert_eq!((label.as_deref(), Some(seen.state.seq)), (Some("Car"), hooks.seq));
    assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "let go");

    // Pulled as they are written: the run's outcome stands, with why they
    // weren't stored, and the card is never written again (run.lock stays).
    let mut hooks = Stores { card: &card, label: "Van", pull: true, seq: None };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut hooks, knobs(), Answers::default(), 2);
    let outcome = ran.unwrap();
    assert!(outcome.stored.is_err(), "{:?}", outcome.stored);
    card.replug();
    assert!(card.stat("/.mstream/run.lock").unwrap().is_some(), "not let go");
    let seen = look(&card);
    assert_eq!(seen.state.value.as_ref().unwrap()["label"].as_str(), Some("Car"));
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
    assert_eq!(finished(&card), old, "the same music, records, folders and indexes");
    // A waveform index of its own too, from the old one's slots: its bars asked
    // for nowhere.
    let waves = |r: &Root| r.manifest.waves.as_ref().map(|c| c.name());
    assert!(waves(&root).is_some() && waves(&root) != waves(&before), "a waveform index of its own");
    let file = waves_file(&card).expect("the file");
    assert_eq!(mpwf::Table::open(&file).unwrap().header().card_id, done.card_id);
    assert_eq!(extras.asked, Vec::<[u8; 16]>::new());
    // And a plain run after it changes nothing more.
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    let again = session.run(plan, &mut server, &mut Extras::default(), &mut |_| {}).unwrap();
    assert_eq!((again.card_id, again.commit_id), (done.card_id, done.commit_id));
}

// ── The waveform index (WAVEFORM-INDEX.md 7.2, 7.3) ─────────────────────────

/// The generation of the card's root.
fn generation_of(card: &FatSim) -> u32 {
    look(card).root.expect("a root").manifest.header.generation
}

/// What a commit writes, and `/music`: a run that makes none leaves them as
/// they were, stamps and all. The library's loose file has an album of its
/// own, `/music`'s, whose thumbnail every run writes again (no folder but a
/// named one is read back as having THUMB).
fn commit_image(card: &FatSim) -> BTreeMap<String, (u64, u32, u64)> {
    let image = card.image().into_iter();
    image.filter(|(p, _)| p.ends_with(".bin") || p.starts_with("/music/")).collect()
}

/// A run under these hooks and the tests' knobs, its outcome verified.
fn synced(card: &FatSim, wanted: &WantedCard, server: &mut Canned, extras: &mut Extras) -> Vec<Event> {
    let (ran, events) = sync_with(card, wanted, server, extras, knobs(), Answers::default(), 7);
    let outcome = ran.unwrap_or_else(|e| panic!("the run: {e}"));
    assert_eq!(outcome.verified, Ok(()));
    events
}

/// The recording of a server path, as the selection has it.
fn recording_of(wanted: &WantedCard, server_path: &str) -> [u8; 16] {
    let file = wanted.files.iter().find(|f| f.ledger.server_path == server_path);
    file.expect("a file of the selection").ledger.canonical_hash().expect("a recording")
}

/// Every record of the card's root has a row: its own path hash and length,
/// and the bars of its recording.
fn every_record_has_its_row(card: &FatSim) {
    let rows = wave_rows(card);
    let records = records(card);
    assert_eq!(rows.len(), records.len());
    for e in &records {
        let row = rows.get(&e.hash).unwrap_or_else(|| panic!("{}: no row", e.path));
        assert_eq!(row.length_ms, e.rec.duration_ms, "{}", e.path);
        assert_eq!(row.bars, bars_of(e.ledger.as_ref().expect("a ledger row")), "{}", e.path);
    }
}

#[test]
fn a_sync_writes_its_waveform_index_with_the_final_commit_only() {
    let dir = Scratch::new("waves-first");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut extras = Extras::default();
    let events = synced(&card, &wanted, &mut server, &mut extras);
    assert_eq!(extras.asked.len(), 6, "each recording once");
    let said = events.iter().filter_map(|e| match e {
        Event::Waves { done, of } => Some((*done, *of)),
        _ => None,
    });
    assert_eq!(said.collect::<Vec<_>>(), [(1, 6), (2, 6), (3, 6), (4, 6), (5, 6), (6, 6)]);

    // 7.2's order: under its final name after the tags file and the Auto DJ
    // index, before the root that names it. A checkpoint wrote none.
    let log = card.log();
    let at = |what: &str| log.iter().position(|w| w.starts_with(what)).unwrap_or_else(|| panic!("{what}"));
    assert_eq!(log.iter().filter(|w| w.starts_with("create /.mstream/waves-")).count(), 1);
    let last = log.iter().rposition(|w| w.starts_with("create /.mstream/manifest.tmp")).unwrap();
    assert!(at("create /.mstream/tags-00000004.bin") < at("create /.mstream/autodj-00000004.bin"));
    assert!(at("create /.mstream/autodj-00000004.bin") < at("create /.mstream/waves-00000004.bin"));
    assert!(at("sync /.mstream/waves-00000004.bin") < last);

    // The root's row, after the others' in its COMP order, the file's own
    // generation; and each record's row.
    let root = look(&card).root.unwrap();
    assert_eq!(root.waves, Some(Ok(())));
    let entry = root.manifest.waves.as_ref().expect("the root's row");
    assert_eq!((entry.generation, entry.name().as_str()), (4, "waves-00000004.bin"));
    assert_eq!(entry.key, [0; 16]);
    every_record_has_its_row(&card);

    // A converted file takes its source's peaks, and its own length.
    let records = records(&card);
    let five = records.iter().find(|e| e.path == "Blake Band/Live/02 - Five.opus").expect("Five");
    assert_eq!(five.rec.duration_ms, 5_000, "the card's Opus, not the source's 30 s");
    assert_eq!(wave_rows(&card)[&five.hash].length_ms, 5_000);
    assert!(extras.asked.contains(&recording_of(&wanted, "Lib/Blake Band/Live/02 - Five.flac")));
}

/// 7.3: a moved file keeps its bars under its new path hash; a new recording,
/// and a new file, are what a run asks for. An unchanged card then makes no
/// commit, and no asks.
#[test]
fn a_moved_file_keeps_its_bars_and_an_unchanged_card_makes_no_commit() {
    let dir = Scratch::new("waves-moved");
    let (card, mut wanted, mut server) = first(&dir.0);
    let before = wave_rows(&card);
    second(&dir.0, &mut wanted, &mut server);
    let mut extras = Extras::default();
    synced(&card, &wanted, &mut server, &mut extras);
    let want = [
        recording_of(&wanted, "Lib/Alex Artist/First Album/02 - Two.mp3"),
        recording_of(&wanted, "Lib/Casey Choir/Hymns/01 - Six.mp3"),
    ];
    assert_eq!(BTreeSet::from_iter(extras.asked), BTreeSet::from(want), "the rest came from the old slots");
    every_record_has_its_row(&card);

    // Three moved on the server, Four by the listener; each row's hash a new
    // one, its CRC for its new place, its bars as they were.
    let rows = wave_rows(&card);
    let by_path = |rel: &str| rows.get(&path_hash(rel)).unwrap_or_else(|| panic!("{rel}: no row"));
    for (old, new) in [
        ("Alex Artist/Second Album/01 - Three.flac", "Alex Artist/Second Album (Deluxe)/01 - Three.flac"),
        ("Blake Band/Live/01 - Four.opus", "Moved/01 - Four.opus"),
    ] {
        assert!(!rows.contains_key(&path_hash(old)), "{old}: no row at its old path");
        assert_eq!(before[&path_hash(old)].bars, by_path(new).bars, "{new}");
    }
    // Two's own path, its new recording's bars.
    let two = "Alex Artist/First Album/02 - Two.mp3";
    assert_ne!(before[&path_hash(two)].bars, by_path(two).bars);
    // Loose left the card, and the listener's edit of One: no row.
    assert!(!rows.contains_key(&path_hash("Loose.mp3")));
    assert!(!rows.contains_key(&path_hash("Alex Artist/First Album/01 - One.mp3")));
    // The old file went at the clean-up (`wave_rows` looked).

    // Nothing new on either side: no ask, no commit, the card as it was.
    let image = card.image();
    let generation = generation_of(&card);
    let mut again = Extras::default();
    let events = synced(&card, &wanted, &mut server, &mut again);
    assert_eq!(again.asked, Vec::<[u8; 16]>::new());
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(events.last(), Some(&Event::Unchanged), "and it says so: its last look comes next");
    assert_eq!(card.image(), image, "not even a stamp");
    assert_eq!(generation_of(&card), generation);
}

/// A card synced before its player read MPWF, or whose server had no answers
/// then, and nothing else to do: that run commits, because its bytes change
/// (12). A commit makes the device build its library again, so nothing less
/// makes one.
#[test]
fn a_card_that_lacks_waves_gets_a_commit_for_them_alone() {
    let dir = Scratch::new("waves-lacks");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    synced(&card, &wanted, &mut server, &mut none);
    let root = look(&card).root.unwrap();
    assert_eq!((root.manifest.waves.clone(), root.waves), (None, None), "no row: no MPWF");
    assert!(waves_file(&card).is_none());
    let (generation, fetched, music) = (generation_of(&card), server.fetched.len(), finished(&card).music);

    // A second run of no answers: nothing to commit.
    let image = commit_image(&card);
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    let events = synced(&card, &wanted, &mut server, &mut none);
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(commit_image(&card), image);

    // Now it answers: the one thing that changes is the file, and it commits.
    let mut extras = Extras::default();
    let events = synced(&card, &wanted, &mut server, &mut extras);
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Committed { last: true, .. })).count(), 1);
    assert_eq!(generation_of(&card), generation + 1);
    assert_eq!(extras.asked.len(), 6);
    every_record_has_its_row(&card);
    assert_eq!((server.fetched.len(), finished(&card).music), (fetched, music), "no music moved");

    // And the run after that makes none.
    let image = commit_image(&card);
    let events = synced(&card, &wanted, &mut server, &mut Extras::default());
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(commit_image(&card), image);
}

/// 12: a fetch that stops is never a failed transfer. What came stays, the old
/// rows that hold stay, the next run asks for the rest; and the old file is
/// never deleted while it names a useful row.
#[test]
fn a_fetch_that_stops_early_keeps_what_came_and_what_holds() {
    let dir = Scratch::new("waves-stops");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let mut wanted = library(&mut server);

    // Two answers of six.
    let mut extras = Extras { answers: Some(2), ..Extras::default() };
    synced(&card, &wanted, &mut server, &mut extras);
    let rows = wave_rows(&card);
    assert_eq!(rows.len(), 2);

    // One more: a new file, the first two carried; asked only the rest.
    let mut extras = Extras { answers: Some(1), ..Extras::default() };
    let generation = generation_of(&card);
    synced(&card, &wanted, &mut server, &mut extras);
    assert_eq!(extras.asked.len(), 1);
    let more = wave_rows(&card);
    assert_eq!(more.len(), 3);
    assert!(rows.iter().all(|(h, row)| more.get(h) == Some(row)));
    assert_eq!(generation_of(&card), generation + 1);

    // Nothing gained and no old row changed: the bytes are equal, the file
    // named again, and no commit.
    let image = commit_image(&card);
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    let events = synced(&card, &wanted, &mut server, &mut none);
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(commit_image(&card), image);

    // Nothing gained, one old row gone (its recording replaced): a new file
    // leaves that row out, and keeps the others.
    let kept = records(&card).into_iter().filter(|e| more.contains_key(&e.hash));
    let rel = kept.map(|e| e.path).find(|p| p.ends_with(".mp3")).expect("an MP3 of the three");
    let path = format!("Lib/{rel}");
    let bytes = corpus("mpeg2.mp3");
    server.add(&path, bytes.clone());
    let file = wanted.files.iter_mut().find(|f| f.ledger.server_path == path).expect("its file");
    *file = track(&path, &bytes, 777);
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    let generation = generation_of(&card);
    synced(&card, &wanted, &mut server, &mut none);
    let left = wave_rows(&card);
    assert_eq!(left.len(), 2);
    assert!(!left.contains_key(&path_hash(&rel)), "its old bars aren't its new recording's");
    assert_eq!(generation_of(&card), generation + 1);

    // The whole selection gone and no answer: no row left, so no MPWF, and the
    // clean-up takes the file.
    wanted.files.clear();
    let answers = Answers { removals: true, mass_removals: true, ..Answers::default() };
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut none, knobs(), answers, 8);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    assert_eq!(look(&card).root.unwrap().manifest.waves, None);
    assert!(waves_file(&card).is_none(), "no row, no file: there is no file of 0 rows");
}

/// 7.4: a player that doesn't read it gets none; one that stops listing it
/// loses the file at the clean-up, whatever else the card holds.
#[test]
fn a_card_whose_player_doesnt_read_the_file_gets_none() {
    let dir = Scratch::new("waves-gate");
    let (card, wanted, mut server) = first(&dir.0);
    assert!(waves_file(&card).is_some());
    let text = String::from_utf8(fixture("card/golden/device.txt")).unwrap();
    assert!(text.contains("read.mpwf=1"));
    put(&dir.0, "/.player/device.txt", text.replace("read.mpwf=1", "read.mpwf=2").as_bytes(), LATER);
    let generation = generation_of(&card);
    let mut extras = Extras::default();
    synced(&card, &wanted, &mut server, &mut extras);
    assert_eq!(extras.asked, Vec::<[u8; 16]>::new());
    assert_eq!(look(&card).root.unwrap().manifest.waves, None);
    assert!(waves_file(&card).is_none());
    assert_eq!(generation_of(&card), generation + 1, "the row's going is a commit's");

    // None comes back while it says so.
    let image = commit_image(&card);
    synced(&card, &wanted, &mut server, &mut Extras::default());
    assert_eq!(commit_image(&card), image);
}

/// 7.2, 7.3: a bad old file is absent, never fatal. The frame or the row: no old
/// slot holds, and everything is asked for again. One slot: that slot alone. A
/// major this writer doesn't know is another matter: it stops (9).
#[test]
fn a_bad_old_file_is_absent_and_the_run_goes_on() {
    let base = Scratch::new("waves-bad-base");
    let (card, wanted, mut server) = first(&base.0);
    let want = finished(&card);
    let name = look(&card).root.unwrap().manifest.waves.expect("the root's row").name();
    let good = card.read(&format!("/.mstream/{name}"), usize::MAX).unwrap().expect("the file");
    drop(card);
    let rel = format!("/.mstream/{name}");
    let all = wanted.files.len();
    let flip_at = |at: usize| {
        let mut b = good.clone();
        b[at] ^= 1;
        b
    };
    // Whatever the card holds under its name, with the frame good or not. A
    // magic of another format is a bad file, not a newer one.
    let cases: [(&str, Option<Vec<u8>>, usize, bool); 6] = [
        ("a flipped header byte", Some(flip_at(44)), all, false),
        ("another magic", Some(flip_at(0)), all, false),
        ("cut short", Some(good[..good.len() - 100].to_vec()), all, false),
        ("another file", Some(fixture("card/golden/waves-0000002a.bin")), all, false),
        ("gone", None, all, false),
        ("a damaged slot", Some(flip_at(256 + 2 * 256 + 100)), 1, true),
    ];
    for (what, bytes, asked, frame_good) in &cases {
        let dir = Scratch::new("waves-bad");
        copy_tree(&base.0, &dir.0);
        match bytes {
            Some(b) => put(&dir.0, &rel, b, LATER),
            None => std::fs::remove_file(dir.0.join(rel.trim_start_matches('/'))).unwrap(),
        }
        let card = FatSim::new(&dir.0, SERIAL);
        let seen = volume::inspect(&card).expect("the card lists");
        assert_eq!(seen.root.as_ref().unwrap().waves == Some(Ok(())), *frame_good, "{what}");

        let mut extras = Extras::default();
        let (ran, _) = sync_with(&card, &wanted, &mut server, &mut extras, knobs(), Answers::default(), 7);
        let outcome = ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(extras.asked.len(), *asked, "{what}");
        assert_eq!(finished(&card), want, "{what}: the rows again, a new file, and the bad one gone");
    }

    // A checkpoint names a good old file again, and no bad one: a run parked by
    // its source ends in one.
    let mut more = wanted.clone();
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    more.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    server.stop_at = Some("Lib/Casey Choir/Hymns/01 - Six.mp3".into());
    let row = look(&FatSim::new(&base.0, SERIAL)).root.unwrap().manifest.waves;
    for (bytes, named) in [(&good, row), (&flip_at(44), None)] {
        let dir = Scratch::new("waves-bad-parked");
        copy_tree(&base.0, &dir.0);
        put(&dir.0, &rel, bytes, LATER);
        let card = FatSim::new(&dir.0, SERIAL);
        let mut extras = Extras::default();
        let (ran, _) = sync_with(&card, &more, &mut server, &mut extras, knobs(), Answers::default(), 7);
        assert!(ran.expect("the run").parked.is_some());
        assert_eq!(extras.asked, Vec::<[u8; 16]>::new(), "no index step");
        let root = volume::inspect(&card).unwrap().root.unwrap();
        assert!(!root.manifest.is_final());
        assert_eq!(root.manifest.waves, named);
    }
    server.stop_at = None;

    // A major this writer doesn't know stops it before any write.
    let mut newer = good.clone();
    newer[4] = 2;
    let dir = Scratch::new("waves-bad-major");
    copy_tree(&base.0, &dir.0);
    put(&dir.0, &rel, &newer, LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let image = card.image();
    let ran = sync(&card, &wanted, &mut server, 7);
    let stopped = matches!(ran, Err(Stop::Guard(Guard::Newer(ref n))) if n == std::slice::from_ref(&name));
    assert!(stopped, "{ran:?}");
    assert_eq!(card.image(), image);
}

/// 8.3: a read that fails says nothing of the file, and is never taken for its
/// absence, which would ask for everything again and, with no answer, have the
/// clean-up delete a file the device can still read (12). What the card does
/// read of it stays of use; its frame out of reach stops the run, as any other
/// file the root names does.
#[test]
fn a_read_error_of_the_old_file_costs_what_it_reaches_and_no_more() {
    let base = Scratch::new("waves-read-base");
    let (card, wanted, mut server) = first(&base.0);
    let want = finished(&card);
    let rows = wave_rows(&card);
    let name = look(&card).root.unwrap().manifest.waves.expect("the root's row").name();
    let rel = format!("/.mstream/{name}");
    drop(card);
    let in_copy = || {
        let dir = Scratch::new("waves-read");
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        (dir, card)
    };

    // A whole read that fails where its pieces don't: nothing lost, nothing
    // asked, no commit.
    let (_dir, card) = in_copy();
    let image = commit_image(&card);
    card.stumbles(&rel);
    let mut extras = Extras::default();
    let events = synced(&card, &wanted, &mut server, &mut extras);
    card.heal();
    assert_eq!(extras.asked, Vec::<[u8; 16]>::new());
    assert_eq!(events.last(), Some(&Event::Unchanged), "{events:?}");
    assert_eq!(commit_image(&card), image);

    // A bad sector, of rows 1 and 2: its frame reads, those two are asked for
    // and the rest hold.
    let (_dir, card) = in_copy();
    card.bad_sector(&rel, 512);
    let seen = volume::inspect(&card).expect("the card lists");
    assert_eq!(seen.root.as_ref().unwrap().waves, Some(Ok(())), "the file is valid");
    let mut extras = Extras::default();
    synced(&card, &wanted, &mut server, &mut extras);
    card.heal();
    assert_eq!(extras.asked.len(), 2);
    assert_eq!(finished(&card), want, "the rows again, a new file, and the bad one gone");

    // And with no answer to anything: a run with no answers isn't a card with no
    // shapes. A new file of the rows that hold, never none.
    let (_dir, card) = in_copy();
    card.bad_sector(&rel, 512);
    let mut none = Extras { answers: Some(0), ..Extras::default() };
    synced(&card, &wanted, &mut server, &mut none);
    card.heal();
    let left = wave_rows(&card);
    assert_eq!(left.len(), rows.len() - 2);
    assert!(left.iter().all(|(h, row)| rows.get(h) == Some(row)));

    // Its frame: the settling stops with nothing written, not even run.lock
    // left. Once the card reads it again, nothing was lost.
    let (_dir, card) = in_copy();
    let image = card.image();
    card.bad_sector(&rel, 0);
    let mut extras = Extras::default();
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut extras, knobs(), Answers::default(), 7);
    assert!(matches!(ran, Err(Stop::Card(_))), "{ran:?}");
    card.heal();
    assert_eq!(card.image(), image);
    assert_eq!(extras.asked, Vec::<[u8; 16]>::new());
    let events = synced(&card, &wanted, &mut server, &mut Extras::default());
    assert_eq!(events.last(), Some(&Event::Unchanged), "{events:?}");
    assert_eq!(wave_rows(&card), rows);

    // Its frame out of reach only by the final commit, after a settling that
    // read it: the run stops there too, the root as it was.
    struct Breaks<'c>(&'c FatSim, &'c str);
    impl Hooks for Breaks<'_> {
        fn cover(&mut self, _album: &Album<'_>) -> Cover {
            self.0.bad_sector(self.1, 0);
            Cover::Keep
        }
    }
    let (_dir, card) = in_copy();
    let image = commit_image(&card);
    let hooks = &mut Breaks(&card, &rel);
    let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, knobs(), Answers::default(), 7);
    assert!(matches!(ran, Err(Stop::Card(_))), "{ran:?}");
    card.heal();
    assert_eq!(commit_image(&card), image);

    // device.txt's read failing isn't a player that stopped reading the file,
    // whose run would drop it.
    let (_dir, card) = in_copy();
    let image = card.image();
    card.bad_sector("/.player/device.txt", 0);
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut Extras::default(), knobs(), Answers::default(), 7);
    assert!(matches!(ran, Err(Stop::Card(_))), "{ran:?}");
    card.heal();
    assert_eq!(card.image(), image);
}

/// A file the card can't read whole, read in pieces: all it loses is the sector
/// it can't read, wherever that lies, and a card that turns out to be gone ends
/// the reading.
#[test]
fn a_file_read_in_pieces_loses_only_the_sector_the_card_cant_read() {
    let dir = Scratch::new("pieces");
    let card = FatSim::new(&dir.0, SERIAL);
    let path = "/.mstream/file.bin";
    let len = (3 << 20) + 700;
    let bytes: Vec<u8> = (0..len as u32).map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8).collect();
    put(&dir.0, path, &bytes, LATER);
    let got = read_pieces(&card, path, len, &mut || Ok(())).expect("it reads");
    assert!(got == bytes, "all of it");
    for at in [1024, (2 << 20) + 1000, (3 << 20) + 600] {
        card.bad_sector(path, at as u64);
        let mut failed = 0;
        let count = &mut || {
            failed += 1;
            Ok(())
        };
        let got = read_pieces(&card, path, len, count).expect("it reads");
        assert_eq!(failed, 1, "one piece, of the file's four");
        let start = at / 512 * 512;
        let mut want = bytes.clone();
        want[start..(start + 512).min(len)].fill(0);
        assert!(got == want, "the sector of byte {at}, and no other");
    }
    let got = read_pieces(&card, path, len, &mut || Err(Stop::Pulled));
    assert!(matches!(got, Err(Stop::Pulled)));
}

/// A card pulled during the Auto DJ index's step, minutes long with no card
/// access: its waveforms aren't asked for, which could take half an hour more
/// before a write found the card gone.
#[test]
fn a_card_pulled_ahead_of_the_waveforms_step_isnt_asked_for_them() {
    struct Pulls<'c> {
        card: &'c FatSim,
        asked: bool,
    }
    impl Hooks for Pulls<'_> {
        fn autodj(&mut self, _commit: &Commit<'_>) -> AutoDj {
            self.card.pull();
            AutoDj::None
        }
        fn waves(&mut self, _commit: &Commit<'_>, _events: &mut dyn FnMut(Event)) -> Waves {
            self.asked = true;
            Waves::Same
        }
    }
    let dir = Scratch::new("waves-pulled-first");
    let (card, wanted, mut server) = first(&dir.0);
    let mut hooks = Pulls { card: &card, asked: false };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut hooks, knobs(), Answers::default(), 7);
    assert!(matches!(ran, Err(Stop::Pulled)), "{ran:?}");
    assert!(!hooks.asked);
}

/// A fetch of half an hour comes ahead of the final commit's first write: what a
/// run copied is named by a checkpoint before it, so a card pulled in that wait
/// keeps it, and the next run fetches none of it again. A run that copied
/// nothing has nothing to name.
#[test]
fn a_run_names_what_it_copied_ahead_of_a_long_index_step() {
    let rare = Knobs { checkpoint_files: 500, ..knobs() };
    let dir = Scratch::new("waves-long");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut extras = Extras { long: true, ..Extras::default() };
    let answers = Answers::default();
    let (ran, events) = sync_with(&card, &wanted, &mut server, &mut extras, rare.clone(), answers, 1);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    let of_it = |e: &&Event| matches!(e, Event::Committed { .. } | Event::Waves { done: 1, .. });
    let said: Vec<&Event> = events.iter().filter(of_it).collect();
    let (checkpoint, ask) = (Event::Committed { generation: 2, last: false }, Event::Waves { done: 1, of: 6 });
    assert_eq!(said, [&checkpoint, &ask, &Event::Committed { generation: 3, last: true }]);
    every_record_has_its_row(&card);

    let mut extras = Extras { long: true, ..Extras::default() };
    let (ran, events) = sync_with(&card, &wanted, &mut server, &mut extras, rare.clone(), answers, 2);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    assert_eq!(events.last(), Some(&Event::Unchanged), "{events:?}");

    // Pulled as it asks.
    struct Pulls<'c>(&'c FatSim);
    impl Hooks for Pulls<'_> {
        fn long_index_steps(&self) -> bool {
            true
        }
        fn waves(&mut self, _commit: &Commit<'_>, _events: &mut dyn FnMut(Event)) -> Waves {
            self.0.pull();
            Waves::Same
        }
    }
    let dir = Scratch::new("waves-long-pulled");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let hooks = &mut Pulls(&card);
    let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, rare, answers, 1);
    assert!(matches!(ran, Err(Stop::Pulled)), "{ran:?}");
    card.replug();
    let seen = look(&card);
    let root = seen.root.as_ref().expect("a root");
    assert!(!root.manifest.is_final(), "the checkpoint");
    assert_eq!(root.tags.as_ref().unwrap().records.len(), wanted.files.len(), "every file named");
    assert!(seen.pending.as_ref().expect("the plan").ops.is_empty(), "with none left to do");
    let fetched = server.fetched.len();
    let again = recover(&card, &wanted, &mut server, 3);
    assert_eq!(again.ran.expect("the next run").verified, Ok(()));
    assert_eq!(server.fetched.len(), fetched, "not one fetched again");
    every_record_has_its_row(&card);
}

/// Each write of that checkpoint and what comes after it, cut: the next run
/// finishes, as the uncut run did. The writes before it are any run's.
#[test]
fn every_cut_of_a_run_that_names_its_files_ahead_of_the_fetch_is_finished() {
    let base = Scratch::new("waves-long-base");
    let (card, mut wanted, mut server) = first(&base.0);
    drop(card);
    second(&base.0, &mut wanted, &mut server);
    let rare = Knobs { checkpoint_files: 500, ..knobs() };
    let answers = Answers::default();
    let long = || Extras { long: true, ..Extras::default() };

    let whole = Scratch::new("waves-long-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    let (ran, events) = sync_with(&card, &wanted, &mut server, &mut long(), rare.clone(), answers, 2);
    assert_eq!(ran.expect("uncut").verified, Ok(()));
    let checkpoints = events.iter().filter(|e| matches!(e, Event::Committed { last: false, .. }));
    assert_eq!(checkpoints.count(), 1, "{events:?}");
    let writes = card.log();
    let want = finished(&card);
    let tags = writes.iter().position(|w| w.starts_with("create /.mstream/tags-"));
    let from = tags.expect("the checkpoint's tags file") as u64;

    for (n, how) in cuts_of(&writes, writes.len()).filter(|(n, _)| *n > from) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let dir = Scratch::new("waves-long");
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        card.cut_at(n, how);
        let _ = sync_with(&card, &wanted, &mut server, &mut long(), rare.clone(), answers, 2);
        card.replug();
        assert!(look(&card).root.is_some(), "{what}: a root");
        let outcome = recover(&card, &wanted, &mut server, 3).ran;
        let outcome = outcome.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
    }
}

/// A checkpoint names the old file again (7.2), its ledger already showing a
/// recording replaced before it, while the file still holds the old bars: a run
/// that begins from it reads the slot's length too (7.3). The rest still hold.
#[test]
fn a_recording_replaced_before_a_checkpoint_is_asked_for_again() {
    let base = Scratch::new("waves-checkpoint-base");
    let (card, mut wanted, mut server) = first(&base.0);
    let name = look(&card).root.unwrap().manifest.waves.expect("the root's row").name();
    let old_rows = wave_rows(&card);
    drop(card);
    let two = "Lib/Alex Artist/First Album/02 - Two.mp3";
    let bytes = corpus("mpeg2.mp3");
    server.add(two, bytes.clone());
    wanted.files[1] = track(two, &bytes, 102);
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    let want = [recording_of(&wanted, two), recording_of(&wanted, "Lib/Casey Choir/Hymns/01 - Six.mp3")];
    let every_file = Knobs { checkpoint_files: 1, ..knobs() };
    let answers = Answers::default();

    // The run uncut: where its first checkpoint's commit ends.
    let whole = Scratch::new("waves-checkpoint-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    let hooks = &mut Extras::default();
    let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, every_file.clone(), answers, 2);
    assert_eq!(ran.expect("uncut").verified, Ok(()));
    let writes = card.log();
    let uncut = finished(&card);
    let commit = writes.iter().position(|w| w.starts_with("rename /.mstream/manifest.tmp"));
    let commit = commit.expect("a commit") as u64;

    // Cut right after it: Two replaced, Six not yet there.
    let dir = Scratch::new("waves-checkpoint");
    copy_tree(&base.0, &dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    card.cut_at(commit + 2, Cut::Fail);
    let hooks = &mut Extras::default();
    let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, every_file, answers, 2);
    assert!(ran.is_err(), "cut");
    card.replug();

    // The root is that checkpoint: it names the old file again, whose slot of
    // Two is the old recording's, and its ledger already says the new one's.
    let seen = look(&card);
    let root = seen.root.as_ref().unwrap();
    assert!(!root.manifest.is_final());
    assert_eq!(root.manifest.waves.as_ref().map(Comp::name), Some(name));
    assert_eq!(root.waves, Some(Ok(())));
    let tags = root.tags.as_ref().unwrap();
    let rel = "Alex Artist/First Album/02 - Two.mp3";
    let replaced = tags.records.iter().find(|e| e.path == rel).expect("Two");
    let hash = replaced.ledger.as_ref().and_then(Ledger::canonical_hash);
    assert_eq!(hash, Some(want[0]), "the ledger's the new recording's");
    let slot = &old_rows[&path_hash(rel)];
    assert_ne!(slot.length_ms, replaced.rec.duration_ms, "the slot's length is the old one's");
    assert_ne!(slot.bars, bars_of(replaced.ledger.as_ref().unwrap()), "and so are its bars");

    // The next run asks for it, and for the new file, and for nothing else.
    let mut extras = Extras::default();
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut extras, knobs(), Answers::default(), 3);
    assert_eq!(ran.expect("the next run").verified, Ok(()));
    assert_eq!(BTreeSet::from_iter(extras.asked), BTreeSet::from(want));
    every_record_has_its_row(&card);
    assert_eq!(finished(&card), uncut, "where the uncut run left it");
}

/// Each write of a run that does nothing but add the file, cut: the card reads
/// as before or as that commit, and the next run finishes.
#[test]
fn every_cut_of_a_run_that_only_adds_waves_is_finished() {
    let base = Scratch::new("waves-only-base");
    set_up(&base.0);
    let card = FatSim::new(&base.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    synced(&card, &wanted, &mut server, &mut Extras { answers: Some(0), ..Extras::default() });
    drop(card);

    let whole = Scratch::new("waves-only-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    synced(&card, &wanted, &mut server, &mut Extras::default());
    let writes = card.log();
    let want = finished(&card);
    assert!(want.waves.is_some());
    assert!(writes.iter().any(|w| w.starts_with("create /.mstream/waves-")), "{writes:?}");

    let mut named = 0;
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let dir = Scratch::new("waves-only");
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        card.cut_at(n, how);
        let _ = sync(&card, &wanted, &mut server, 7);
        card.replug();
        let seen = look(&card);
        named += usize::from(seen.root.expect("a root").manifest.waves.is_some());
        let again = recover(&card, &wanted, &mut server, 3);
        let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
    }
    assert!(named > 0, "the later cuts leave the commit that names it");
}

/// A hook's new file is checked before anything names it: one that doesn't read
/// whole, or is another commit's, stops the run with the card as it was.
#[test]
fn a_new_waveform_index_is_checked_before_a_root_names_it() {
    struct Hands(Vec<u8>);
    impl Hooks for Hands {
        fn waves(&mut self, _commit: &Commit<'_>, _events: &mut dyn FnMut(Event)) -> Waves {
            Waves::New(self.0.clone())
        }
    }
    let good = fixture("card/golden/waves-one.bin");
    let mut damaged = good.clone();
    damaged[256 + 100] ^= 1;
    let short = good[..100].to_vec();
    for (what, bytes) in [("another commit's", good), ("a damaged slot", damaged), ("short", short)] {
        let dir = Scratch::new("waves-refused");
        let (card, wanted, mut server) = first(&dir.0);
        let root = look(&card).root.unwrap().manifest;
        let hooks = &mut Hands(bytes);
        let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, knobs(), Answers::default(), 7);
        assert!(matches!(ran, Err(Stop::Refused(_))), "{what}: {ran:?}");
        assert_eq!(look(&card).root.unwrap().manifest, root, "{what}");
        assert!(wave_rows(&card).len() == 6, "{what}: the old file stays");
    }
}

/// A run with no hook of its own, as a set-up's: no answer to anything, so the
/// old file's rows are what it has. A copied card's new identity carries them.
#[test]
fn a_run_with_no_hooks_keeps_the_rows_that_hold() {
    let dir = Scratch::new("waves-no-hooks");
    let (card, mut wanted, mut server) = first(&dir.0);
    let want = wave_rows(&card);
    let old_id = look(&card).root.unwrap().manifest.header.card_id;

    // A run with nothing else to do: the file is named again.
    let name = |card: &FatSim| look(card).root.unwrap().manifest.waves.map(|c| c.name());
    let was = name(&card);
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut NoHooks, knobs(), Answers::default(), 7);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    assert_eq!((name(&card), wave_rows(&card)), (was, want.clone()));

    // A new identity: a new file of the same rows.
    let answers = Answers { new_card_id: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, answers, knobs());
    let done = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!(done.verified, Ok(()));
    assert!(done.card_id != old_id);
    assert_eq!(wave_rows(&card), want);
    let file = waves_file(&card).expect("the file");
    assert_eq!(mpwf::Table::open(&file).unwrap().header().card_id, done.card_id);

    // A file that left the selection: its row goes, a new file; the others
    // stay.
    let gone = wanted.files.pop().expect("a file");
    let answers = Answers { removals: true, ..Answers::default() };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut NoHooks, knobs(), answers, 8);
    assert_eq!(ran.expect("the run").verified, Ok(()));
    let rows = wave_rows(&card);
    assert_eq!(rows.len(), want.len() - 1, "{gone:?}");
    assert!(rows.iter().all(|(h, row)| want.get(h) == Some(row)));
}

// ── Records another reader made (2.6.1) ─────────────────────────────────────

/// The card's root as another reader would have left it: its tags file
/// under `source` and `parser_version`, its records through `edit`, and
/// the root naming that file again.
fn left_by(
    dir: &Path,
    card: &FatSim,
    source: u8,
    parser_version: u16,
    edit: impl FnOnce(&mut Vec<RecordIn>),
) {
    let root = look(card).root.expect("a root");
    let (meta, mut records, folders) = root.tags.expect("its tags file").to_input();
    edit(&mut records);
    let meta = Meta { source, parser_version, ..meta };
    let tags = mptg::write(&meta, &records, &folders).expect("a tags file");
    let mut m = root.manifest.to_input();
    m.comps[0] = Comp::of(&tags).expect("its entry");
    put(dir, &format!("/.mstream/{}", m.comps[0].name()), &tags, LATER);
    put(dir, &format!("/.mstream/{}", root.file), &msmf::write(&m).expect("a root"), LATER);
}

/// Every record with its picture written as absent, as parser 1 wrote it.
fn unanchored(records: &mut Vec<RecordIn>) {
    for r in records {
        let rec = &mut r.rec;
        (rec.pic_offset, rec.pic_length, rec.pic_coding) = (0, 0, 0);
        (rec.pic_type, rec.pic_mime) = (0, 0);
    }
}

/// A tags file this reader didn't make, an older version's or the
/// device's as the sync agent (2.12.4), has every record it carries read
/// again from its card file before the next commit (2.6.1): a picture the
/// older reader wrote as absent is anchored, mStream's analysed BPM stays
/// where the tags have none, a file that isn't audio is left as it is, and
/// the header is this reader's. A file of this reader's version is carried
/// as it stands, and a run then has nothing to commit.
#[test]
fn records_another_reader_made_are_read_again_before_the_next_commit() {
    let dir = Scratch::new("reread");
    let (card, wanted, mut server) = first(&dir.0);
    let ours = records(&card);
    let anchored = |records: &[mptg::Entry]| {
        records.iter().filter(|e| e.rec.pic_offset != 0).count()
    };
    assert_eq!(anchored(&ours), 2, "Two's and Three's pictures");
    let analysed = ours.iter().filter(|e| e.rec.flags & BPM_ANALYSED != 0).count();
    assert!(analysed > 0, "mStream's BPM where the tags have none");

    // The older reader's: no anchors, and a cover the software owns.
    let cover = b"\xFF\xD8\xFF, a cover as far as the records go";
    let (size, qfp) = (cover.len() as u32, qfp_of(cover));
    let container = CONTAINER_NOT_AUDIO;
    let jpg = Record { size, fat_time: LATER, qfp, container, ..Record::default() };
    put(&dir.0, "/music/Alex Artist/First Album/cover.jpg", cover, LATER);
    left_by(&dir.0, &card, SOURCE_SOFTWARE, 1, |records| {
        unanchored(records);
        let (path, ledger) = ("Alex Artist/First Album/cover.jpg".into(), Some(Ledger::default()));
        records.push(RecordIn { path, rec: jpg, run: Default::default(), ledger });
    });
    let generation = look(&card).root.unwrap().manifest.header.generation;
    let done = sync(&card, &wanted, &mut server, 2).expect("a run");
    assert_eq!((done.written, done.deleted, done.verified), (0, 0, Ok(())));
    assert_eq!(done.generation, generation + 1, "a commit for the records alone");
    let tags = look(&card).root.unwrap().tags.unwrap();
    assert_eq!((tags.source, tags.parser_version), (SOURCE_SOFTWARE, PARSER_VERSION));
    assert_eq!(tags.records.len(), ours.len() + 1);
    for e in &tags.records {
        let Some(o) = ours.iter().find(|o| o.path == e.path) else {
            assert_eq!((e.path.as_str(), e.rec), ("Alex Artist/First Album/cover.jpg", jpg));
            continue;
        };
        assert_eq!((&e.rec, &e.run), (&o.rec, &o.run), "{}", e.path);
    }
    // Nothing more to do: no commit.
    assert_eq!(sync(&card, &wanted, &mut server, 3).expect("a run").generation, generation + 1);

    // This reader's version, its records as they stand (the anchors gone
    // here): carried, nothing to commit.
    left_by(&dir.0, &card, SOURCE_SOFTWARE, PARSER_VERSION, unanchored);
    assert_eq!(sync(&card, &wanted, &mut server, 4).expect("a run").generation, generation + 1);
    assert_eq!(anchored(&records(&card)), 0);
    // The device's reader, whatever version it calls its own: read again.
    left_by(&dir.0, &card, SOURCE_SYNC_AGENT, PARSER_VERSION, unanchored);
    assert_eq!(sync(&card, &wanted, &mut server, 5).expect("a run").generation, generation + 2);
    let tags = look(&card).root.unwrap().tags.unwrap();
    assert_eq!((tags.source, anchored(&tags.records)), (SOURCE_SOFTWARE, 2));
}

/// A carried record whose card file reads UNREADABLE when it is read
/// again (its bytes gone bad under the same size and stamp) keeps none of
/// mStream's analysis: an UNREADABLE record's fields are all absent
/// (2.6.4), as for a file read the first time.
#[test]
fn a_file_read_again_as_unreadable_keeps_no_analysed_bpm() {
    let dir = Scratch::new("reread-bad");
    let (card, wanted, mut server) = first(&dir.0);
    let ours = records(&card);
    let analysed = |e: &&mptg::Entry| e.rec.flags & BPM_ANALYSED != 0;
    let bad = ours.iter().filter(analysed).find(|e| e.rec.container == CONTAINER_MP3);
    let bad = bad.expect("an MP3 with mStream's BPM");
    let path = dir.0.join("music").join(&bad.path);
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
    let zeros = vec![0u8; bad.rec.size as usize];
    std::fs::write(&path, &zeros).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(stamp).unwrap();
    left_by(&dir.0, &card, SOURCE_SOFTWARE, 1, |_| {});
    let generation = look(&card).root.unwrap().manifest.header.generation;
    let done = sync(&card, &wanted, &mut server, 2).expect("a run");
    assert_eq!((done.written, done.generation), (0, generation + 1));
    let tags = records(&card);
    assert_eq!(tags.len(), ours.len());
    for e in &tags {
        let o = ours.iter().find(|o| o.path == e.path).expect("a record carried");
        if e.path != bad.path {
            assert_eq!((&e.rec, &e.run), (&o.rec, &o.run), "{}", e.path);
            continue;
        }
        let (size, fat_time, qfp) = (o.rec.size, o.rec.fat_time, qfp_of(&zeros));
        let (container, flags) = (CONTAINER_MP3, UNREADABLE);
        let unreadable = Record { size, fat_time, qfp, container, flags, ..Record::default() };
        assert_eq!(e.rec, unreadable, "{}", e.path);
    }
}

// ── The mood tails (MOOD-FILTERS.md 5.2, 5.3) ───────────────────────────────

/// The root's records' mood tails, by path.
fn tails(card: &FatSim) -> BTreeMap<String, Tail> {
    records(card).into_iter().map(|e| (e.path, e.rec.moods)).collect()
}

/// The root's tags file's minor and RECS stride: (1, 80) with a mood tail,
/// (0, 72) with none.
fn tags_shape(card: &FatSim) -> (u16, u32) {
    let root = look(card).root.expect("a root");
    let name = format!("/.mstream/{}", root.manifest.tags.name());
    let bytes = card.read(&name, usize::MAX).expect("it reads").expect("the file the root names");
    let h = container::Header::parse(&bytes).expect("a frame");
    let entry = |i: u32| &bytes[h.header_bytes as usize + 32 * i as usize..][..32];
    let recs = (0..h.section_count).map(entry).find(|e| u32_at(e, 0) == mptg::RECS);
    (h.minor, u32_at(recs.expect("RECS"), 20))
}

/// The first sync onto a set-up card, its mood step bringing [`scores_of`]
/// the selection: every record with its row's tail, in rows of 80 at minor 1.
fn scored_card(dir: &Path) -> (FatSim, WantedCard, Canned) {
    set_up(dir);
    let card = FatSim::new(dir, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut extras = Extras::scored(&wanted);
    synced(&card, &wanted, &mut server, &mut extras);
    assert_eq!(extras.moods_asked, 1, "the final commit's, and no checkpoint's");
    let rows = scores_of(&wanted, 0);
    for e in records(&card) {
        let row = &rows[&e.ledger.as_ref().expect("a ledger row").server_path];
        assert!(e.rec.has_moods(), "{}", e.path);
        assert_eq!(e.rec.moods, moods::encode(row.as_ref()), "{}", e.path);
    }
    assert_eq!(tags_shape(&card), (1, 80));
    (card, wanted, server)
}

/// 5.2: the tails are laid over the records before the final commit is
/// weighed, so the scores the card has already make no commit.
#[test]
fn equal_scores_make_no_commit() {
    let dir = Scratch::new("moods-equal");
    let (card, wanted, mut server) = scored_card(&dir.0);
    let (image, generation) = (commit_image(&card), generation_of(&card));
    let mut again = Extras::scored(&wanted);
    let events = synced(&card, &wanted, &mut server, &mut again);
    assert_eq!(again.moods_asked, 1);
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(events.last(), Some(&Event::Unchanged));
    assert_eq!((commit_image(&card), generation_of(&card)), (image, generation));
}

/// One head of one track moved on the server, and nothing else: one commit
/// (13's score-only commit), whose one change is that record's tail.
#[test]
fn a_score_changed_alone_makes_one_commit() {
    let dir = Scratch::new("moods-changed");
    let (card, wanted, mut server) = scored_card(&dir.0);
    let (before, generation, fetched) = (records(&card), generation_of(&card), server.fetched.len());
    let mut changed = Extras::scored(&wanted);
    let rows = changed.scores.as_mut().expect("rows");
    let row = rows.get_mut("Lib/Alex Artist/First Album/02 - Two.mp3").and_then(Option::as_mut);
    let row = row.expect("Two's row");
    row[moods::SAD] = Some(0.99);
    let tail = moods::encode(Some(&*row));
    let events = synced(&card, &wanted, &mut server, &mut changed);
    let commits: Vec<&Event> = events.iter().filter(|e| matches!(e, Event::Committed { .. })).collect();
    assert_eq!(commits, [&Event::Committed { generation: generation + 1, last: true }]);
    assert_eq!(server.fetched.len(), fetched, "no music moved");
    let after = records(&card);
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(&before) {
        let rec = Record { moods: b.rec.moods, ..a.rec };
        assert_eq!((&a.path, &rec, &a.run, &a.ledger), (&b.path, &b.rec, &b.run, &b.ledger));
        let two = a.path == "Alex Artist/First Album/02 - Two.mp3";
        assert_eq!(a.rec.moods != b.rec.moods, two, "{}", a.path);
        if two {
            assert_eq!(a.rec.moods, tail);
        }
    }

    // The run after that has nothing to do.
    let image = commit_image(&card);
    let events = synced(&card, &wanted, &mut server, &mut changed);
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(commit_image(&card), image);
}

/// A file moved, on the server (a rename on the card) or by the listener
/// (kept at its new path), carries its tail, as it carries its bars; so
/// does a row's file fetched again, the scores being the server's, never the
/// bytes'. A file new to the card starts with none, and only the hook gives
/// it one.
#[test]
fn a_moved_file_keeps_its_scores() {
    let dir = Scratch::new("moods-moved");
    let (card, mut wanted, mut server) = scored_card(&dir.0);
    let before = tails(&card);
    second(&dir.0, &mut wanted, &mut server);
    // The step brings a row for the new file alone.
    let six = "Lib/Casey Choir/Hymns/01 - Six.mp3";
    let row = scores_of(&wanted, 0)[six];
    let scores = Some(HashMap::from([(six.to_string(), row)]));
    synced(&card, &wanted, &mut server, &mut Extras { scores, ..Extras::default() });
    let after = tails(&card);
    assert_eq!(after.len(), 6);
    for (old, new) in [
        ("Alex Artist/Second Album/01 - Three.flac", "Alex Artist/Second Album (Deluxe)/01 - Three.flac"),
        ("Blake Band/Live/01 - Four.opus", "Moved/01 - Four.opus"),
    ] {
        assert_ne!(before[old], NO_ROW, "{old}");
        assert_eq!(after[new], before[old], "{new}");
    }
    assert_eq!(after["Casey Choir/Hymns/01 - Six.mp3"], moods::encode(row.as_ref()));
    // Fetched again, the step bringing nothing for them: Two (another
    // recording) in place, Five (the listener deleted it) in the folder's new
    // case, each with its row's tail.
    for (old, new) in [
        ("Alex Artist/First Album/02 - Two.mp3", "Alex Artist/First Album/02 - Two.mp3"),
        ("Blake Band/Live/02 - Five.opus", "BLAKE BAND/Live/02 - Five.opus"),
    ] {
        assert_ne!(before[old], NO_ROW, "{old}");
        assert_eq!(after[new], before[old], "{new}");
    }
    // One beside the listener's edit of it, the edit theirs now: a new file.
    assert_eq!(after["Alex Artist/First Album/01 - One (2).mp3"], NO_ROW);
    assert_eq!(tags_shape(&card), (1, 80));
}

/// A row's file fetched again keeps the row's tail under every way a run
/// brings no scores for it (Moods::Keep: a fetch that failed, a server
/// without them, `--moods false`, the listener's stop): replaced in place
/// (Two, another recording), and deleted and written again under another
/// extension (Five, its conversion gone); the record is the new bytes' all
/// the same. An answer that names it lays its own; one that leaves it out
/// keeps the row's.
#[test]
fn a_file_fetched_again_keeps_its_scores_till_an_answer_lays_others() {
    let base = Scratch::new("moods-again-base");
    let (card, mut wanted, mut server) = scored_card(&base.0);
    let before = records(&card);
    drop(card);
    let two = "Lib/Alex Artist/First Album/02 - Two.mp3";
    let bytes = corpus("mpeg2.mp3");
    server.add(two, bytes.clone());
    wanted.files[1] = track(two, &bytes, 102);
    let five = "Lib/Blake Band/Live/02 - Five.flac";
    wanted.files[4] = track(five, &server.files[five].clone(), 202);
    let two_at = "Alex Artist/First Album/02 - Two.mp3";
    let (old_five, new_five) = ("Blake Band/Live/02 - Five.opus", "Blake Band/Live/02 - Five.flac");
    let was = |rel: &str| before.iter().find(|e| e.path == rel).expect("a record").clone();
    let mut run = |name: &str, hooks: &mut dyn Hooks| {
        let dir = Scratch::new(name);
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, knobs(), Answers::default(), 7);
        let done = ran.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((done.written, done.verified), (2, Ok(())), "{name}");
        (dir, card)
    };

    // Moods::Keep: no hook of its own, and the tests' hooks with no fetch.
    let keeps: [(&str, &mut dyn Hooks); 2] =
        [("moods-again-none", &mut NoHooks), ("moods-again-keep", &mut Extras::default())];
    for (name, hooks) in keeps {
        let (_dir, card) = run(name, hooks);
        let after = records(&card);
        assert_eq!(after.len(), before.len(), "{name}");
        assert!(!after.iter().any(|e| e.path == old_five), "{name}: the Opus gone");
        for (old, new) in [(two_at, two_at), (old_five, new_five)] {
            let (o, e) = (was(old), after.iter().find(|e| e.path == new).expect("written again"));
            assert!(o.rec.has_moods(), "{name}: {old}");
            assert_eq!(e.rec.moods, o.rec.moods, "{name}: {new} keeps its row's");
            assert_ne!(e.rec.size, o.rec.size, "{name}: {new} read from its new bytes");
        }
        for e in after.iter().filter(|e| e.path != two_at && e.path != new_five) {
            assert_eq!(e.rec, was(&e.path).rec, "{name}: {}", e.path);
        }
        assert_eq!(tags_shape(&card), (1, 80), "{name}");
    }

    // An answer naming Two alone: Two takes its row, Five keeps the card's.
    let row = scores_of(&wanted, 1)[two];
    let scores = Some(HashMap::from([(two.to_string(), row)]));
    let (_dir, card) = run("moods-again-answer", &mut Extras { scores, ..Extras::default() });
    let after = tails(&card);
    assert_eq!(after[two_at], moods::encode(row.as_ref()));
    assert_ne!(after[two_at], was(two_at).rec.moods);
    assert_eq!(after[new_five], was(old_five).rec.moods);
}

/// A record read again from its card file (another reader's tags file:
/// 2.6.1) is a new reading of the file, and keeps the tail it had: the
/// scores are the server's. So does a file that reads UNREADABLE now (the
/// player takes no scores from its record: METADATA.md 2.9).
#[test]
fn a_record_read_again_keeps_its_scores() {
    let dir = Scratch::new("moods-reread");
    let (card, wanted, mut server) = scored_card(&dir.0);
    let before = records(&card);
    // One MP3's bytes gone bad under the same size and stamp.
    let bad = before.iter().find(|e| e.rec.container == CONTAINER_MP3).expect("an MP3");
    let path = dir.0.join("music").join(&bad.path);
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, vec![0u8; bad.rec.size as usize]).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(stamp).unwrap();
    left_by(&dir.0, &card, SOURCE_SOFTWARE, 1, unanchored);
    let generation = generation_of(&card);
    let done = sync(&card, &wanted, &mut server, 2).expect("a run");
    assert_eq!((done.written, done.generation, done.verified), (0, generation + 1, Ok(())));
    let after = records(&card);
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(&before) {
        assert_eq!((&a.path, a.rec.moods), (&b.path, b.rec.moods));
    }
    let read = after.iter().find(|e| e.path == bad.path).expect("the bad one");
    assert_eq!(read.rec.flags, UNREADABLE, "{}: read again", read.path);
    assert!(read.rec.has_moods());
    assert_eq!(tags_shape(&card), (1, 80));
}

/// A card whose file at `at` grows by `more` the moment a rename puts it
/// there: a size that moves between the check of a server move's file and
/// its read back.
struct Grows<'c> {
    card: &'c FatSim,
    at: &'static str,
    more: &'static [u8],
}

impl CardFs for Grows<'_> {
    fn list(&self, dir: &str) -> io::Result<Vec<Item>> {
        self.card.list(dir)
    }

    fn stat(&self, path: &str) -> io::Result<Option<Item>> {
        self.card.stat(path)
    }

    fn read(&self, path: &str, max: usize) -> io::Result<Option<Vec<u8>>> {
        self.card.read(path, max)
    }

    fn read_at(&self, path: &str, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        self.card.read_at(path, offset, len)
    }
}

impl CardWrite for Grows<'_> {
    fn key(&self) -> String {
        self.card.key()
    }

    fn serial(&self) -> io::Result<Option<Serial>> {
        self.card.serial()
    }

    fn open(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        self.card.open(path)
    }

    fn create(&self, path: &str) -> io::Result<Box<dyn CardFile + '_>> {
        self.card.create(path)
    }

    fn delete(&self, path: &str) -> io::Result<()> {
        self.card.delete(path)
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.card.rename(from, to)?;
        if to == self.at {
            let mut file = self.card.open(to)?;
            file.seek(io::SeekFrom::End(0))?;
            file.write_all(self.more)?;
            file.sync()?;
        }
        Ok(())
    }

    fn mkdir(&self, path: &str) -> io::Result<()> {
        self.card.mkdir(path)
    }

    fn rmdir(&self, path: &str) -> io::Result<()> {
        self.card.rmdir(path)
    }

    fn lock_file(&self, path: &str) -> io::Result<Box<dyn LockFile + '_>> {
        self.card.lock_file(path)
    }
}

/// A server move whose file's size moved by its read back: the record is
/// read again from the file's bytes, and keeps its tail.
#[test]
fn a_rename_whose_size_moved_keeps_its_scores() {
    let dir = Scratch::new("moods-rename-grew");
    let (card, mut wanted, mut server) = scored_card(&dir.0);
    let before = records(&card);
    let three = before.iter().find(|e| e.path == "Alex Artist/Second Album/01 - Three.flac");
    let three = three.expect("Three");
    server.add(MOVED, server.files["Lib/Alex Artist/Second Album/01 - Three.flac"].clone());
    wanted.files[2].ledger.server_path = MOVED.into();
    let more = b"bytes another writer put there".as_slice();
    let at = "/music/Alex Artist/Second Album (Deluxe)/01 - Three.flac";
    let grows = Grows { card: &card, at, more };
    let (hooks, answers) = (&mut Extras::default(), Answers::default());
    let opened = Session::open(&grows, &Expect::default(), knobs(), &answers, env(7), hooks, &mut |_| {});
    let mut session = opened.unwrap_or_else(|e| panic!("open: {e}"));
    let plan = session.plan(&wanted, &answers, &mut server).unwrap_or_else(|e| panic!("plan: {e}"));
    assert_eq!((plan.renames(), plan.fetches().0), (1, 0), "a server move, and nothing else");
    let done = session.run(plan, &mut server, hooks, &mut |_| {}).unwrap_or_else(|e| panic!("run: {e}"));
    assert_eq!((done.renamed, done.verified), (1, Ok(())));
    let after = records(&card);
    let moved = after.iter().find(|e| e.path == "Alex Artist/Second Album (Deluxe)/01 - Three.flac");
    let moved = moved.expect("Three, moved");
    assert_eq!(moved.rec.size, three.rec.size + more.len() as u32, "read again at its new size");
    assert!(moved.rec.has_moods());
    assert_eq!(moved.rec.moods, three.rec.moods);
}

/// A checkpoint carries each record's tail as the run holds it and asks for
/// none: a cut run's root has the old scores, a file it fetched again keeping
/// its row's, and none for a file new to the card; the next run's final
/// commit lays the new ones.
#[test]
fn a_checkpoint_carries_scores_and_adds_none() {
    let base = Scratch::new("moods-checkpoint-base");
    let (card, mut wanted, mut server) = scored_card(&base.0);
    let old = tails(&card);
    drop(card);
    let two = "Lib/Alex Artist/First Album/02 - Two.mp3";
    let bytes = corpus("mpeg2.mp3");
    server.add(two, bytes.clone());
    wanted.files[1] = track(two, &bytes, 102);
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    wanted.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    let every_file = Knobs { checkpoint_files: 1, ..knobs() };
    let rows = scores_of(&wanted, 1);
    let new = || Extras { scores: Some(rows.clone()), ..Extras::default() };
    let answers = Answers::default();

    // The run uncut: where its first checkpoint's commit ends.
    let whole = Scratch::new("moods-checkpoint-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut new(), every_file.clone(), answers, 2);
    assert_eq!(ran.expect("uncut").verified, Ok(()));
    let writes = card.log();
    let uncut = finished(&card);
    let commit = writes.iter().position(|w| w.starts_with("rename /.mstream/manifest.tmp"));
    let commit = commit.expect("a commit") as u64;

    // Cut right after it: Two replaced, Six not yet there, no ask made.
    let dir = Scratch::new("moods-checkpoint");
    copy_tree(&base.0, &dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    card.cut_at(commit + 2, Cut::Fail);
    let mut cut = new();
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut cut, every_file, answers, 2);
    assert!(ran.is_err(), "cut");
    assert_eq!(cut.moods_asked, 0);
    card.replug();
    assert!(!look(&card).root.expect("a root").manifest.is_final(), "the checkpoint");
    let fetched = "Alex Artist/First Album/02 - Two.mp3";
    let two_now = records(&card).into_iter().find(|e| e.path == fetched).expect("Two");
    assert_eq!(two_now.rec.size as usize, bytes.len(), "fetched again by the checkpoint");
    let held = tails(&card);
    assert!(!held.contains_key("Casey Choir/Hymns/01 - Six.mp3"));
    assert_eq!(held, old, "each carried, the file fetched again its row's");
    assert_eq!(tags_shape(&card), (1, 80));

    // The next run's final commit lays the new scores, once.
    let mut next = new();
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut next, knobs(), answers, 3);
    assert_eq!(ran.expect("the next run").verified, Ok(()));
    assert_eq!(next.moods_asked, 1);
    assert_eq!(finished(&card), uncut, "where the uncut run left it");
    for e in records(&card) {
        let row = &rows[&e.ledger.as_ref().expect("a ledger row").server_path];
        assert_eq!(e.rec.moods, moods::encode(row.as_ref()), "{}", e.path);
        assert_ne!(old.get(&e.path), Some(&e.rec.moods), "{}: the new scores", e.path);
    }
}

/// A run with no hook of its own, as a set-up's: no fetch, so each record
/// keeps the tail it has; and a copied card's new identity carries them.
#[test]
fn no_hooks_keep_the_scores() {
    let dir = Scratch::new("moods-no-hooks");
    let (card, wanted, mut server) = scored_card(&dir.0);
    let (want, generation) = (tails(&card), generation_of(&card));
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut NoHooks, knobs(), Answers::default(), 7);
    let done = ran.expect("the run");
    assert_eq!((done.generation, done.verified), (generation + 1, Ok(())), "a commit: no Auto DJ index");
    assert_eq!((tails(&card), tags_shape(&card)), (want.clone(), (1, 80)));

    let answers = Answers { new_card_id: true, ..Answers::default() };
    let (session, plan) = plan_of(&card, &wanted, &mut server, answers, knobs());
    let done = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!(done.verified, Ok(()));
    assert_eq!(look(&card).root.unwrap().manifest.header.card_id, done.card_id);
    assert_eq!((tails(&card), tags_shape(&card)), (want, (1, 80)));
}

/// A card for someone else: every tail cleared in one commit, the tags file
/// back to rows of 72 at minor 0, every other field of every record as it
/// was; and the run after that has nothing to do.
#[test]
fn clear_goes_back_to_stride_72_and_minor_0() {
    let dir = Scratch::new("moods-clear");
    let (card, wanted, mut server) = scored_card(&dir.0);
    let (before, generation) = (records(&card), generation_of(&card));
    let clear = || Extras { clear: true, ..Extras::default() };
    let events = synced(&card, &wanted, &mut server, &mut clear());
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Committed { .. })).count(), 1);
    assert_eq!((generation_of(&card), tags_shape(&card)), (generation + 1, (0, 72)));
    let after = records(&card);
    assert!(after.iter().all(|e| !e.rec.has_moods()));
    let unscored = |r: &[mptg::Entry]| -> Vec<(String, Record)> {
        r.iter().map(|e| (e.path.clone(), Record { moods: NO_ROW, ..e.rec })).collect()
    };
    assert_eq!(unscored(&after), unscored(&before));

    let image = commit_image(&card);
    let events = synced(&card, &wanted, &mut server, &mut clear());
    assert!(!events.iter().any(|e| matches!(e, Event::Committed { .. })), "{events:?}");
    assert_eq!(commit_image(&card), image);
}

/// Each write of a run that does nothing but add the tails, cut: the card
/// reads as before or as that commit, and the next run finishes.
#[test]
fn every_cut_of_a_run_that_only_adds_moods_is_finished() {
    let base = Scratch::new("moods-only-base");
    set_up(&base.0);
    let card = FatSim::new(&base.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    synced(&card, &wanted, &mut server, &mut Extras::default());
    assert_eq!(tags_shape(&card), (0, 72));
    drop(card);

    let whole = Scratch::new("moods-only-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    synced(&card, &wanted, &mut server, &mut Extras::scored(&wanted));
    let writes = card.log();
    let want = finished(&card);
    assert_eq!(tags_shape(&card), (1, 80));
    assert!(!writes.iter().any(|w| w.contains(" /music/")), "no music moved: {writes:?}");

    let mut laid = 0;
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let dir = Scratch::new("moods-only");
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        card.cut_at(n, how);
        let hooks = &mut Extras::scored(&wanted);
        let _ = sync_with(&card, &wanted, &mut server, hooks, knobs(), Answers::default(), 7);
        card.replug();
        laid += usize::from(tags_shape(&card) == (1, 80));
        let again = recover_under(&card, &wanted, &mut server, 3, knobs(), || Extras::scored(&wanted));
        let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
    }
    assert!(laid > 0, "the later cuts leave the commit that lays them");
}

/// Each write of a run that fetches Two again, its mood step bringing
/// nothing (Moods::Keep), cut: the next run, under Keep too, finishes it as
/// the uncut run did, Two with its row's tail however the cut left it (the
/// old file still there; the new one in place under the root's old record,
/// the cut run's half-finished work: 2.12.3; or a checkpoint's record).
#[test]
fn every_cut_of_a_run_that_fetches_a_file_again_keeps_its_scores() {
    let base = Scratch::new("moods-again-cut-base");
    let (card, mut wanted, mut server) = scored_card(&base.0);
    let want = tails(&card);
    drop(card);
    let two = "Lib/Alex Artist/First Album/02 - Two.mp3";
    let bytes = corpus("mpeg2.mp3");
    server.add(two, bytes.clone());
    wanted.files[1] = track(two, &bytes, 102);
    let two_at = "Alex Artist/First Album/02 - Two.mp3";

    let whole = Scratch::new("moods-again-cut-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    synced(&card, &wanted, &mut server, &mut Extras::default());
    let writes = card.log();
    let uncut = finished(&card);
    assert_eq!(tails(&card), want);

    // The cuts that leave Two's new bytes in place under the root's old record.
    let mut ours = 0;
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let dir = Scratch::new("moods-again-cut");
        copy_tree(&base.0, &dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        card.cut_at(n, how);
        let hooks = &mut Extras::default();
        let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, knobs(), Answers::default(), 7);
        card.replug();
        if ran.is_err() {
            let on = card.read(&format!("/music/{two_at}"), 1 << 20).ok().flatten();
            let rec = records(&card).into_iter().find(|e| e.path == two_at);
            let old = rec.is_some_and(|e| e.rec.size as usize != bytes.len());
            ours += usize::from(on.as_deref() == Some(bytes.as_slice()) && old);
        }
        let again = recover_under(&card, &wanted, &mut server, 3, knobs(), Extras::default);
        let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), uncut, "{what}");
        assert_eq!(tails(&card), want, "{what}");
    }
    assert!(ours > 0, "a cut leaves the new bytes as the cut run's work");
}

/// A tail a reader would refuse never reaches the card: the hook's answer
/// stops the run before the commit's first write, the card as it was.
#[test]
fn a_tail_a_reader_refuses_stops_the_run_with_the_card_as_it_was() {
    struct Hands;
    impl Hooks for Hands {
        fn moods(&mut self, commit: &Commit<'_>) -> Moods {
            let first = commit.records.first().expect("a record");
            Moods::Overlay(HashMap::from([(first.path.clone(), [0xAB; 8])]))
        }
    }
    let dir = Scratch::new("moods-refused");
    let (card, wanted, mut server) = first(&dir.0);
    let root = look(&card).root.unwrap().manifest;
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut Hands, knobs(), Answers::default(), 7);
    assert!(matches!(&ran, Err(Stop::Refused(why)) if why.contains("mood tail")), "{ran:?}");
    assert_eq!(look(&card).root.unwrap().manifest, root);
}

// ── Partials a later run takes up (2.12.8) ──────────────────────────────────

const BIG: &str = "Lib/Peer/Album/01 - Big.mp3";
const SMALL: &str = "Lib/Peer/Album/02 - Small.mp3";

/// Two files of a server whose bytes a later run takes up where a run left
/// them: an MP3 of 20 KB with its picture, and one of 6 KB.
fn taken_up(server: &mut Canned) -> WantedCard {
    server.resumable = true;
    let mut files = Vec::new();
    for (path, name, id) in [(BIG, "picture_first.mp3", 601), (SMALL, "cbr.mp3", 602)] {
        let bytes = corpus(name);
        server.add(path, bytes.clone());
        files.push(track(path, &bytes, id));
    }
    WantedCard { origins: vec![home()], files }
}

/// The card's `resume.json`, read as the volume layer settles it.
fn resume_of(card: &FatSim) -> serde_json::Value {
    look(card).resume.value.unwrap_or(serde_json::Value::Null)
}

/// The partials on the card, name to bytes.
fn partials(card: &FatSim) -> BTreeMap<String, Vec<u8>> {
    let items = card.list("/.mstream/resume").unwrap_or_default();
    let read = |name: &str| {
        card.read(&format!("/.mstream/resume/{name}"), 1 << 20).unwrap().expect("a partial")
    };
    items.into_iter().filter(|i| !i.dir).map(|i| (i.name.clone(), read(&i.name))).collect()
}

fn on_card(card: &FatSim, rel: &str) -> Vec<u8> {
    card.read(&format!("/music/{rel}"), 1 << 20).unwrap().expect("the file is on the card")
}

/// A run parked by the source's quota keeps what came of the file in its
/// partial, flushed and listed, and leaves its plan; the next run takes it
/// up where it stopped (a 206 under its ETag), or, when the server's file
/// changed, starts it over: without asking when the manifest says so, and
/// from the whole file the server answers when only If-Range does.
#[test]
fn a_parked_download_is_taken_up_and_a_changed_file_starts_over() {
    let dir = Scratch::new("part-park");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = taken_up(&mut server);
    server.cut_after = Some((BIG.into(), 8000, true));
    let parked = sync(&card, &wanted, &mut server, 1).unwrap();
    assert_eq!(parked.parked.as_deref(), Some("the day's quota is spent"));
    let big = corpus("picture_first.mp3");
    let listed = resume_of(&card);
    let parts = listed["parts"].as_array().expect("partials listed");
    assert_eq!(parts.len(), 1, "{listed}");
    let p = &parts[0];
    let said = (p["serverPath"].as_str(), p["size"].as_u64(), p["have"].as_u64());
    assert_eq!(said, (Some(BIG), Some(20_579), Some(8000)));
    assert_eq!(p["etag"].as_str(), Some(etag_of(&big).as_str()));
    assert_eq!(p["modified"].as_u64(), Some(wanted.files[0].ledger.server_modified));
    let name = p["name"].as_str().unwrap().to_string();
    assert_eq!(partials(&card), BTreeMap::from([(name.clone(), big[..8000].to_vec())]));
    assert!(look(&card).pending.is_some(), "the plan waits on the card");
    assert!(!card.log().iter().any(|w| w.contains("/stage/")), "never through stage/");

    let parked_card = Scratch::new("part-park-base");
    copy_tree(&dir.0, &parked_card.0);

    // The server's file as it was: on from byte 8000, the rest only.
    let done = sync(&card, &wanted, &mut server, 2).unwrap();
    assert_eq!((done.written, done.verified, done.parked), (2, Ok(()), None));
    assert_eq!(server.asked[1], (BIG.to_string(), 8000, 8000), "{:?}", server.asked);
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), big);
    let rec = records(&card).into_iter().find(|e| e.path == "Peer/Album/01 - Big.mp3").unwrap();
    assert_eq!(rec.ledger.unwrap().origin_flags, VERIFIED, "the MD5 of the whole file, read back");
    let _ = finished(&card);
    assert_eq!(resume_of(&card)["parts"], serde_json::json!([]), "its entry gone");

    // Changed on the server, as its manifest says: started over without
    // asking, the old partial gone.
    let changed = Scratch::new("part-park-changed");
    copy_tree(&parked_card.0, &changed.0);
    let card = FatSim::new(&changed.0, SERIAL);
    let mut server = Canned::default();
    let mut wanted = taken_up(&mut server);
    let new = corpus("many_txxx.mp3");
    server.add(BIG, new.clone());
    wanted.files[0] = track(BIG, &new, 601);
    let done = sync(&card, &wanted, &mut server, 3).unwrap();
    assert_eq!((done.written, done.verified), (2, Ok(())));
    assert_eq!(server.asked[0], (BIG.to_string(), 0, 0));
    let gone = format!("delete /.mstream/resume/{name}");
    assert!(card.log().contains(&gone), "the old partial");
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), new);
    let _ = finished(&card);

    // Changed with the manifest the same (one not scanned again yet): asked
    // from byte 8000, answered whole, the listing cut back before the
    // partial is — never the old part stitched to the new.
    let stale = Scratch::new("part-park-stale");
    copy_tree(&parked_card.0, &stale.0);
    let card = FatSim::new(&stale.0, SERIAL);
    let mut server = Canned::default();
    let mut wanted = taken_up(&mut server);
    let mut new = big.clone();
    new[100] ^= 0xFF;
    server.add(BIG, new.clone());
    wanted.files[0].ledger.file_hash = md5(&new);
    let done = sync(&card, &wanted, &mut server, 4).unwrap();
    assert_eq!((done.written, done.verified), (2, Ok(())));
    assert_eq!(server.asked[0], (BIG.to_string(), 8000, 0));
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), new);
    let log = card.log();
    let truncated = log.iter().position(|w| *w == format!("truncate /.mstream/resume/{name}"));
    let truncated = truncated.expect("the partial cut back to 0");
    let listing = "rename /.mstream/resume.new";
    let relisted = log[..truncated].iter().rposition(|w| w.starts_with(listing));
    let opened = log.iter().position(|w| w.starts_with("lock")).unwrap();
    assert!(relisted.is_some_and(|r| r > opened), "listed at 0 first: {log:#?}");
    let _ = finished(&card);
}

/// A file's bytes go to the card [`CARD_WRITE`] at a time however they
/// come (the canned server's are 1000 a write): a stage file's in whole
/// pieces and then the rest, as a partial's are between its listings. Both
/// land whole, their MD5 the server's.
#[test]
fn a_files_bytes_go_to_the_card_in_large_pieces() {
    let size = 2 * CARD_WRITE + 123_456;
    let mut bytes = corpus("cbr.mp3");
    bytes.resize(size, 0x55);
    let rel = "Peer/Album/01 - Big.mp3";
    for resumable in [false, true] {
        let dir = Scratch::new("pieces");
        set_up(&dir.0);
        let card = FatSim::new(&dir.0, SERIAL);
        let mut server = Canned { resumable, ..Canned::default() };
        server.add(BIG, bytes.clone());
        let wanted = WantedCard { origins: vec![home()], files: vec![track(BIG, &bytes, 601)] };
        // A partial listed every 8 MiB, as a run's is.
        let k = Knobs { part_bytes: 8 << 20, ..knobs() };
        let hooks = &mut Extras::default();
        let (ran, _) = sync_with(&card, &wanted, &mut server, hooks, k, Answers::default(), 1);
        let ran = ran.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((ran.written, ran.verified), (1, Ok(())));
        let into = if resumable { "/resume/" } else { "/stage/" };
        let sizes = card.sizes(|p| p.contains(into));
        assert_eq!(sizes, [CARD_WRITE, CARD_WRITE, 123_456], "{into}");
        let on = card.read(&format!("/music/{rel}"), usize::MAX).unwrap().expect("on the card");
        assert!(on == bytes, "{into}: the server's bytes");
        let rec = records(&card).into_iter().find(|e| e.path == rel).expect("its record");
        assert_eq!(rec.ledger.unwrap().origin_flags, VERIFIED, "{into}");
        let _ = finished(&card);
    }
}

/// Each piece of a large file's bytes cut in turn (a torn write: half of it
/// lands, and the card is pulled), into a stage file and into a partial
/// listed every 6 MiB — between two pieces, so a listing first writes out
/// the bytes held: its listing never vouches for a byte the card doesn't
/// hold, and the next run finishes as the uncut run did, a partial taken up
/// from its listing.
#[test]
fn every_cut_inside_a_large_files_pieces_is_finished() {
    let size = 2 * CARD_WRITE + 123_456;
    let mut bytes = corpus("cbr.mp3");
    bytes.resize(size, 0x55);
    for resumable in [false, true] {
        let base = Scratch::new("pieces-cut-base");
        set_up(&base.0);
        let mut server = Canned { resumable, ..Canned::default() };
        server.add(BIG, bytes.clone());
        let wanted = WantedCard { origins: vec![home()], files: vec![track(BIG, &bytes, 601)] };
        let k = Knobs { part_bytes: 6 << 20, ..knobs() };
        let run = |card: &FatSim, server: &mut Canned| {
            let hooks = &mut Extras::default();
            sync_with(card, &wanted, server, hooks, k.clone(), Answers::default(), 1).0
        };
        let whole = Scratch::new("pieces-cut-whole");
        copy_tree(&base.0, &whole.0);
        let card = FatSim::new(&whole.0, SERIAL);
        run(&card, &mut server).expect("uncut");
        let want = finished(&card);
        let into = if resumable { "/resume/" } else { "/stage/" };
        let pieces: Vec<u64> = card
            .log()
            .iter()
            .enumerate()
            .filter(|(_, w)| w.starts_with("write ") && w.contains(into))
            .map(|(n, _)| n as u64 + 1)
            .collect();
        assert_eq!(pieces.len(), 3, "{into}: {:?}", card.sizes(|p| p.contains(into)));

        let mut taken_up = 0;
        for n in pieces {
            let what = format!("{into}: cut at write {n}");
            let dir = Scratch::new("pieces-cut");
            copy_tree(&base.0, &dir.0);
            let card = FatSim::new(&dir.0, SERIAL);
            card.cut_at(n, Cut::Fail);
            let _ = run(&card, &mut server);
            card.replug();
            let mut have = 0;
            if resumable {
                have = resume_of(&card)["parts"][0]["have"].as_u64().unwrap_or(0);
                let items = card.list("/.mstream/resume").unwrap_or_default();
                let part = items.iter().find(|i| !i.dir).expect("its partial");
                let path = format!("/.mstream/resume/{}", part.name);
                let on = card.read(&path, usize::MAX).unwrap().expect("its bytes");
                assert!(on.len() as u64 > have, "{what}: the torn write's bytes past {have}");
                let listed = on[..have as usize] == bytes[..have as usize];
                assert!(listed, "{what}: what is listed is on the card");
            }
            let asked = server.asked.len();
            let again = recover_with(&card, &wanted, &mut server, 3, k.clone());
            let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(outcome.verified, Ok(()), "{what}");
            assert_eq!(finished(&card), want, "{what}");
            if have > 0 {
                assert_eq!(server.asked[asked], (BIG.to_string(), have, have), "{what}");
                taken_up += 1;
            }
        }
        let after = "the last piece's cut, after a listing";
        assert_eq!(taken_up, usize::from(resumable), "{into}: {after}");
    }
}

/// A card pulled mid-download keeps the bytes its last listing names; the
/// next run cuts off what came after them and asks for the rest.
#[test]
fn a_run_cut_mid_download_takes_up_what_was_listed() {
    let base = Scratch::new("part-cut-base");
    set_up(&base.0);
    let mut server = Canned::default();
    let wanted = taken_up(&mut server);
    let whole = Scratch::new("part-cut-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    sync(&card, &wanted, &mut server, 2).expect("uncut");
    let want = finished(&card);
    let writes = card.log();
    // The first write of the big file's bytes after its first listing.
    let listed = writes.iter().position(|w| w.starts_with("rename /.mstream/resume.new")).unwrap();
    let bytes = |w: &String| w.starts_with("write /.mstream/resume/");
    let after = writes[listed..].iter().position(bytes).unwrap();
    let n = (listed + after + 1) as u64;

    let (_dir, card) = cut_card(&base.0, n, Cut::Fail, &wanted, &mut server);
    let have = resume_of(&card)["parts"][0]["have"].as_u64().expect("a listed partial");
    assert!(have >= 4096, "{have}");
    let (name, bytes) = partials(&card).into_iter().next().unwrap();
    assert!(bytes.len() as u64 > have, "the torn write's bytes past have: {name}");
    let asked = server.asked.len();
    let again = recover(&card, &wanted, &mut server, 3);
    assert_eq!(again.ran.expect("the next run").verified, Ok(()));
    assert_eq!(server.asked[asked], (BIG.to_string(), have, have));
    assert_eq!(finished(&card), want);
}

/// Every write of a run into a partial cut in turn, each rename also
/// between its two directory writes: the card reads as a commit, and the
/// next run finishes it as the uncut run did, taking up what was listed.
/// One file, listed once, has every kind of write the path makes.
#[test]
fn every_cut_of_a_run_into_partials_is_finished() {
    let base = Scratch::new("part-cuts-base");
    set_up(&base.0);
    let mut server = Canned::default();
    let mut wanted = taken_up(&mut server);
    wanted.files.remove(0);
    let whole = Scratch::new("part-cuts-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    sync(&card, &wanted, &mut server, 2).expect("uncut");
    let want = finished(&card);
    let writes = card.log();
    assert!(writes.iter().any(|w| w.starts_with("rename /.mstream/resume.new")), "{writes:#?}");

    let (mut taken, mut disk_checks) = (0, 0);
    for (n, how) in cuts_of(&writes, writes.len()) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
        assert!(look(&card).root.is_some(), "{what}: a root");
        let asked = server.asked.len();
        let again = recover(&card, &wanted, &mut server, 3);
        disk_checks += usize::from(again.asked);
        let outcome = again.ran.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(outcome.verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
        taken += usize::from(server.asked[asked..].iter().any(|a| a.2 > 0));
    }
    assert!(taken > 0, "a cut after a listing is taken up from it");
    assert!(disk_checks > 0, "a partial's rename cut between its directory writes asks for one");
}

/// A partial longer than its listing: past `have`, the bytes no listing
/// vouched for are cut off and the rest asked for; past `size`, step 4
/// deletes it, and the file starts over.
#[test]
fn a_partial_longer_than_listed_is_cut_back_or_started_over() {
    let dir = Scratch::new("part-long");
    set_up(&dir.0);
    let mut server = Canned::default();
    let wanted = taken_up(&mut server);
    let (big, small) = (corpus("picture_first.mp3"), corpus("cbr.mp3"));
    let entry = |name: &str, file: &WantedFile, have: u64| {
        let l = &file.ledger;
        let key = home().server.url_key;
        format!(
            r#"{{"name": "{name}", "server": {{"serverInstance": "", "serverUrlKey": "0x{key:016X}",
            "parentKey": "0x0", "peerId": 0}}, "serverPath": "{}", "size": {}, "etag": {},
            "modified": {}, "have": {have}}}"#,
            l.server_path,
            l.server_size,
            serde_json::to_string(&etag_of(&server.files[&l.server_path])).unwrap(),
            l.server_modified,
        )
    };
    let text = format!(
        r#"{{"format": "mstream-card-resume", "version": 1, "seq": 2, "parts": [{}, {}]}}"#,
        entry("0000000a.part", &wanted.files[0], 6000),
        entry("0000000b.part", &wanted.files[1], 2000),
    );
    put(&dir.0, "/.mstream/resume.json", text.as_bytes(), LATER);
    let mut past_have = big[..6000].to_vec();
    past_have.extend_from_slice(&[0xEE; 3000]);
    put(&dir.0, "/.mstream/resume/0000000a.part", &past_have, LATER);
    let mut past_size = small.clone();
    past_size.extend_from_slice(b"0123456789");
    put(&dir.0, "/.mstream/resume/0000000b.part", &past_size, LATER);

    let card = FatSim::new(&dir.0, SERIAL);
    let done = sync(&card, &wanted, &mut server, 1).unwrap();
    assert_eq!((done.written, done.verified), (2, Ok(())));
    assert_eq!(server.asked, [(BIG.to_string(), 6000, 6000), (SMALL.to_string(), 0, 0)]);
    assert!(card.log().contains(&"delete /.mstream/resume/0000000b.part".to_string()), "step 4's");
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), big);
    assert_eq!(on_card(&card, "Peer/Album/02 - Small.mp3"), small);
    let _ = finished(&card);
}

/// A `resume.json` that no longer says what the card and the server hold:
/// an entry whose partial is gone is dropped, a partial of a file that left
/// the selection is collected, one whose file changed on the server or
/// whose entry isn't sound starts over, one of a server the run doesn't
/// read stays as it is, and no new partial takes a name the card showed.
#[test]
fn a_stale_resume_json_is_set_right_by_the_next_run() {
    let dir = Scratch::new("part-stale");
    set_up(&dir.0);
    let mut server = Canned::default();
    let wanted = taken_up(&mut server);
    let home_key = home().server.url_key;
    let other_key = server_url_key("https://other.example");
    let entry = |name: &str, key: u64, path: &str, size: u64, modified: u64, have: &str| {
        format!(
            r#"{{"name": "{name}", "server": {{"serverInstance": "", "serverUrlKey": "0x{key:016X}",
            "parentKey": "0x0", "peerId": 0}}, "serverPath": "{path}", "size": {size}, "etag": "",
            "modified": {modified}, "have": {have}, "kept": "a newer software's"}}"#
        )
    };
    let (b, s) = (&wanted.files[0].ledger, &wanted.files[1].ledger);
    let parts = [
        entry("00000001.part", home_key, BIG, b.server_size, b.server_modified - 1, "100"),
        entry("00000002.part", home_key, "Lib/Peer/Album/03 - Gone.mp3", 900, 1, "100"),
        entry("00000003.part", other_key, "Lib/Theirs/01.mp3", 900, 1, "100"),
        entry("00000004.part", home_key, SMALL, s.server_size, s.server_modified, "100"),
        entry("00000005.part", home_key, SMALL, s.server_size, s.server_modified, "-5"),
    ];
    let head = r#""format": "mstream-card-resume", "version": 1, "seq": 4"#;
    let text = format!(r#"{{{head}, "parts": [{}], "note": "kept"}}"#, parts.join(", "));
    put(&dir.0, "/.mstream/resume.json", text.as_bytes(), LATER);
    let older = r#"{"format": "mstream-card-resume", "version": 1, "seq": 3, "parts": []}"#;
    put(&dir.0, "/.mstream/resume.new", older.as_bytes(), LATER);
    for (name, fill) in [("00000001", b'x'), ("00000002", 2), ("00000003", 3)] {
        put(&dir.0, &format!("/.mstream/resume/{name}.part"), &[fill; 100], LATER);
    }
    put(&dir.0, "/.mstream/resume/00000005.part", &corpus("cbr.mp3")[..100], LATER);
    put(&dir.0, "/.mstream/resume/00000006.part", b"listed nowhere", LATER);

    let card = FatSim::new(&dir.0, SERIAL);
    let done = sync(&card, &wanted, &mut server, 1).unwrap();
    assert_eq!((done.written, done.verified), (2, Ok(())));
    let over = [(BIG.to_string(), 0, 0), (SMALL.to_string(), 0, 0)];
    assert_eq!(server.asked, over, "both start over");
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), corpus("picture_first.mp3"));
    assert_eq!(on_card(&card, "Peer/Album/02 - Small.mp3"), corpus("cbr.mp3"));

    let left = partials(&card);
    assert_eq!(left.keys().collect::<Vec<_>>(), ["00000003.part"], "another server's stays");
    let listed = resume_of(&card);
    assert!(listed["seq"].as_u64().unwrap() > 4);
    assert_eq!(listed["note"], "kept", "a newer software's member, kept");
    let entries = listed["parts"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{listed}");
    let theirs: serde_json::Value = serde_json::from_str(&parts[2]).unwrap();
    assert_eq!(entries[0], theirs, "as it was");
    // Named none of the names the card's resume.json listed.
    let made = card.log().into_iter().filter(|w| w.starts_with("create /.mstream/resume/"));
    let made: Vec<String> = made.collect();
    let listed_names = |w: &String| (1..=5).any(|n| w.ends_with(&format!("{n:08x}.part")));
    assert!(made.len() == 2 && !made.iter().any(listed_names), "{made:?}");
    assert!(!card.image().contains_key("/.mstream/resume.new"));
}

/// A partial shorter than its listing (one a disk check cut, say) is taken
/// up from what it holds, and listed so before a byte lands past that: a
/// cut anywhere before its next flush leaves no listing that vouches for a
/// byte the card wasn't given and flushed.
#[test]
fn a_partial_shorter_than_listed_is_listed_with_what_it_holds_first() {
    let base = Scratch::new("part-short-base");
    set_up(&base.0);
    let mut server = Canned::default();
    let mut wanted = taken_up(&mut server);
    wanted.files.truncate(1);
    let (big, l) = (corpus("picture_first.mp3"), &wanted.files[0].ledger);
    let key = home().server.url_key;
    let etag = serde_json::to_string(&etag_of(&big)).unwrap();
    let text = format!(
        r#"{{"format": "mstream-card-resume", "version": 1, "seq": 2, "parts": [{{
        "name": "0000000a.part", "server": {{"serverInstance": "", "serverUrlKey": "0x{key:016X}",
        "parentKey": "0x0", "peerId": 0}}, "serverPath": "{BIG}", "size": {}, "etag": {etag},
        "modified": {}, "have": 6000}}]}}"#,
        l.server_size, l.server_modified,
    );
    put(&base.0, "/.mstream/resume.json", text.as_bytes(), LATER);
    let (name, part) = ("0000000a.part", "/.mstream/resume/0000000a.part");
    put(&base.0, part, &big[..3000], LATER);

    let whole = Scratch::new("part-short-whole");
    copy_tree(&base.0, &whole.0);
    let card = FatSim::new(&whole.0, SERIAL);
    let done = sync(&card, &wanted, &mut server, 2).expect("uncut");
    assert_eq!((done.written, done.verified), (1, Ok(())));
    assert_eq!(server.asked, [(BIG.to_string(), 3000, 3000)]);
    let want = finished(&card);
    let writes = card.log();
    let first = writes.iter().position(|w| *w == format!("write {part}")).expect("its bytes");
    let listing = |w: &String| w.starts_with("rename /.mstream/resume.new");
    assert!(writes[..first].iter().any(listing), "listed at 3000 first: {writes:#?}");
    let flushed = writes[first..].iter().position(|w| *w == format!("sync {part}")).unwrap();

    // Every cut up to its first flush: what the next run trusts of it (the
    // smaller of its listing's have and its size) is what was flushed.
    for (n, how) in cuts_of(&writes, first + flushed + 1) {
        let what = format!("cut at write {n} ({how:?}, {})", writes[n as usize - 1]);
        let (_dir, card) = cut_card(&base.0, n, how, &wanted, &mut server);
        let listed = resume_of(&card);
        let entry = listed["parts"].as_array().and_then(|p| p.iter().find(|e| e["name"] == name));
        let have = entry.and_then(|e| e["have"].as_u64()).unwrap_or(0);
        let size = partials(&card).get(name).map_or(0, |b| b.len() as u64);
        assert!(have.min(size) <= 3000, "{what}: have {have} of {size} bytes");
        let again = recover(&card, &wanted, &mut server, 3);
        assert_eq!(again.ran.unwrap_or_else(|e| panic!("{what}: {e}")).verified, Ok(()), "{what}");
        assert_eq!(finished(&card), want, "{what}");
    }
}

/// A peer's partial is held to 2.11's test as its rows are, its parent the
/// card's server of its parentKey: one whose parent moved to a new URL,
/// known there by its instance, is taken up there, not left behind as a
/// stranger's while the file starts over.
#[test]
fn a_peers_partial_is_taken_up_when_its_parent_moved() {
    let dir = Scratch::new("part-peer");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let instance = [0x42; 16];
    let at = |url: &str| {
        let key = server_url_key(url);
        let talk = Talk { url_key: key, instance, ..Talk::default() };
        let server = Server::direct(instance, key);
        let own = Origin { talk: talk.clone(), server, ..origin(url) };
        let peer = Some(TalkPeer { id: 3, instance: NO_INSTANCE });
        let server = Server::peer(NO_INSTANCE, key, 3);
        vec![own, Origin { talk: Talk { peer, ..talk }, server, ..origin(url) }]
    };
    let mut server = Canned::default();
    let mut wanted = taken_up(&mut server);
    wanted.files.truncate(1);
    wanted.files[0].origin = 1;
    wanted.origins = at("http://music.example:3000");
    server.cut_after = Some((BIG.into(), 8000, true));
    let parked = sync(&card, &wanted, &mut server, 1).unwrap();
    assert_eq!(parked.parked.as_deref(), Some("the day's quota is spent"));
    let listed = resume_of(&card);
    let parent = format!("0x{:016X}", server_url_key("http://music.example:3000"));
    assert_eq!(listed["parts"][0]["server"]["parentKey"].as_str(), Some(parent.as_str()));

    // The server at a new URL, the same by its instance; its peer through it.
    wanted.origins = at("http://music.example:4000");
    let done = sync(&card, &wanted, &mut server, 2).unwrap();
    assert_eq!((done.written, done.verified), (1, Ok(())));
    assert_eq!(server.asked[1], (BIG.to_string(), 8000, 8000), "{:?}", server.asked);
    assert_eq!(on_card(&card, "Peer/Album/01 - Big.mp3"), corpus("picture_first.mp3"));
    let _ = finished(&card);
}

/// A partial whose file the selection still has, unchanged and not on the
/// card, outlives a run that didn't take it up (its source couldn't, and
/// the file was skipped), and the next run goes on from it. Once its file
/// is on the card, it goes.
#[test]
fn a_partial_the_selection_still_wants_outlives_a_run_that_skips_it() {
    let dir = Scratch::new("part-wanted");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = taken_up(&mut server);
    server.cut_after = Some((BIG.into(), 8000, true));
    sync(&card, &wanted, &mut server, 1).unwrap();
    let name = resume_of(&card)["parts"][0]["name"].as_str().unwrap().to_string();
    let parked = Scratch::new("part-wanted-base");
    copy_tree(&dir.0, &parked.0);

    (server.resumable, server.failing) = (false, BTreeSet::from([BIG.to_string()]));
    let done = sync(&card, &wanted, &mut server, 2).unwrap();
    assert_eq!((done.written, done.verified, done.parked), (1, Ok(()), None));
    assert_eq!(partials(&card).keys().collect::<Vec<_>>(), [&name], "it stays");
    assert_eq!(resume_of(&card)["parts"][0]["have"].as_u64(), Some(8000));

    (server.resumable, server.failing) = (true, BTreeSet::new());
    let asked = server.asked.len();
    let done = sync(&card, &wanted, &mut server, 3).unwrap();
    assert_eq!((done.written, done.verified), (1, Ok(())));
    assert_eq!(server.asked[asked..], [(BIG.to_string(), 8000, 8000)]);
    let _ = finished(&card);

    // Fetched whole by a run whose source can't take it up: on the card,
    // its partial goes.
    let card = FatSim::new(&parked.0, SERIAL);
    server.resumable = false;
    let done = sync(&card, &wanted, &mut server, 4).unwrap();
    assert_eq!((done.written, done.verified), (2, Ok(())));
    let _ = finished(&card);
    assert_eq!(resume_of(&card)["parts"], serde_json::json!([]));
}

/// A private file at its last seq (2^53 − 1: a copied file, or a hostile
/// one) is still replaced, its next write at seq 1.
#[test]
fn a_private_file_at_its_last_seq_is_written_again_from_one() {
    let dir = Scratch::new("last-seq");
    set_up(&dir.0);
    let last = (1u64 << 53) - 1;
    let head = |x: &str| format!(r#""format": "mstream-card-{x}", "version": 1, "seq": {last}"#);
    let state = format!(r#"{{{}, "label": "Car"}}"#, head("state"));
    put(&dir.0, "/.mstream/state.json", state.as_bytes(), LATER);
    // An entry whose partial is gone: the run's end writes the file again.
    let gone = r#"[{"name": "00000001.part", "size": 9}]"#;
    let resume = format!(r#"{{{}, "parts": {gone}}}"#, head("resume"));
    put(&dir.0, "/.mstream/resume.json", resume.as_bytes(), LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    let mut hooks = Stores { card: &card, label: "Van", pull: false, seq: None };
    let (ran, _) = sync_with(&card, &wanted, &mut server, &mut hooks, knobs(), Answers::default(), 1);
    let outcome = ran.unwrap();
    assert_eq!((outcome.verified, outcome.stored, hooks.seq), (Ok(()), Ok(()), Some(1)));
    let seen = look(&card);
    assert_eq!((seen.state.seq, seen.resume.seq), (1, 1));
    assert_eq!(seen.state.value.unwrap()["label"], "Van");
    assert_eq!(seen.resume.value.unwrap()["parts"], serde_json::json!([]));
}

/// A source of two origins whose second's day is spent once `cut` bytes of
/// its first file came (a peer's budget): that file cut there and kept in
/// its partial, every later one of it waiting, while the first origin's go.
/// Or one file of the second its day is too short for (`short`: a peer's
/// conversion larger than what is left), which waits alone once asked.
struct Day {
    server: Canned,
    cut: Option<usize>,
    spent: bool,
    /// Each origin-1 file asked for while spent.
    asked_spent: usize,
    short: Option<&'static str>,
    /// The times `short` was asked for.
    asked_short: usize,
}

impl Day {
    fn of(server: Canned, cut: Option<usize>, short: Option<&'static str>) -> Day {
        Day { server, cut, spent: false, asked_spent: 0, short, asked_short: 0 }
    }

    /// The file is `short`, too large for what is left of its day.
    fn too_large(&mut self, file: &WantedFile) -> Option<Failed> {
        let short = file.origin == 1 && self.short == Some(file.ledger.server_path.as_str());
        short.then(|| {
            self.asked_short += 1;
            Failed { why: "too large for what is left of the day".into(), stop: false }
        })
    }
}

impl Fetcher for Day {
    fn fetch(&mut self, file: &WantedFile, into: &mut dyn io::Write) -> Result<Fetched, Failed> {
        if file.origin == 1 && self.spent {
            self.asked_spent += 1;
            return Err(Failed { why: "the day is spent".into(), stop: false });
        }
        if let Some(failed) = self.too_large(file) {
            return Err(failed);
        }
        self.server.fetch(file, into)
    }

    fn read_range(&mut self, file: &WantedFile, start: u64, len: usize) -> Result<Vec<u8>, Failed> {
        self.server.read_range(file, start, len)
    }

    fn resumable(&self, file: &WantedFile) -> bool {
        self.server.resumable(file)
    }

    fn fetch_part(
        &mut self,
        file: &WantedFile,
        have: Option<&Have>,
        into: &mut dyn Part,
    ) -> Result<Fetched, Failed> {
        if file.origin == 1 {
            if self.spent {
                self.asked_spent += 1;
                return Err(Failed { why: "the day is spent".into(), stop: false });
            }
            if let Some(failed) = self.too_large(file) {
                return Err(failed);
            }
            if let Some(n) = self.cut.take() {
                self.server.cut_after = Some((file.ledger.server_path.clone(), n, false));
                let got = self.server.fetch_part(file, have, into);
                self.spent = got.is_err();
                return got;
            }
        }
        self.server.fetch_part(file, have, into)
    }

    fn waits(&self, file: &WantedFile) -> Option<String> {
        if file.origin != 1 {
            return None;
        }
        if self.spent {
            return Some("the peer's day is spent".to_string());
        }
        let short = self.asked_short > 0 && self.short == Some(file.ledger.server_path.as_str());
        short.then(|| "the day is too short for one of its files".to_string())
    }
}

/// Opens, plans and runs with any source.
fn run_from(
    card: &FatSim,
    wanted: &WantedCard,
    source: &mut dyn Fetcher,
    seed: u64,
) -> Result<Outcome, Stop> {
    let (mut hooks, answers, expect) = (Extras::default(), Answers::default(), Expect::default());
    let opened = Session::open(card, &expect, knobs(), &answers, env(seed), &mut hooks, &mut |_| {});
    let mut session = opened?;
    let plan = session.plan(wanted, &answers, source)?;
    session.run(plan, source, &mut hooks, &mut |_| {})
}

/// A peer's files, of a library that sorts before the server's own on the card.
const EARLY: &str = "Folk/Dana Duo/Album/01 - Big.mp3";
const LATE: &str = "Folk/Dana Duo/Album/02 - Small.mp3";

/// A source that waits (a peer whose day is spent, 2.12.8's parking of one
/// source alone): the other origin's files all go in, its albums get their
/// thumbnails, and the run ends at a checkpoint whose plan keeps the
/// waiting files and whose partial keeps what came; a run while it still
/// waits asks it nothing and, changing nothing, commits nothing; the next
/// day's takes up the partial and finishes.
#[test]
fn a_source_that_waits_parks_the_run_once_the_others_are_in() {
    let dir = Scratch::new("waits");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned { resumable: true, ..Canned::default() };
    let mut wanted = library(&mut server);
    let key = server_url_key("http://music.example:3000");
    let peer = Some(TalkPeer { id: 3, instance: NO_INSTANCE });
    wanted.origins.push(Origin {
        talk: Talk { url_key: key, peer, ..Talk::default() },
        server: Server::peer(NO_INSTANCE, key, 3),
        ..Origin::default()
    });
    for (path, name, id) in [(EARLY, "picture_first.mp3", 601), (LATE, "cbr.mp3", 602)] {
        let bytes = corpus(name);
        server.add(path, bytes.clone());
        wanted.files.push(WantedFile { origin: 1, ..track(path, &bytes, id) });
    }
    let mut day = Day::of(server, Some(8000), None);
    let parked = run_from(&card, &wanted, &mut day, 1).unwrap();
    assert_eq!(parked.parked.as_deref(), Some("the peer's day is spent"));
    assert_eq!((parked.written, parked.records), (6, 6), "the server's own files, all of them, after it");
    let at = |path: &str| day.server.asked.iter().position(|a| a.0 == path);
    let one = "Lib/Alex Artist/First Album/01 - One.mp3";
    assert!(at(EARLY) < at(one) && at(one).is_some(), "cut before them: {:?}", day.server.asked);
    assert_eq!(day.asked_spent, 0, "a waiting source's later files aren't asked for");
    let seen = look(&card);
    assert!(seen.pending.is_some(), "the device says the last transfer didn't finish");
    let root = seen.root.as_ref().unwrap();
    assert!(!root.manifest.is_final(), "a checkpoint");
    let tags = root.tags.as_ref().unwrap();
    let thumbed = |f: &&mptg::Folder| f.flags & FOLDER_THUMB != 0;
    assert!(tags.folders.iter().filter(thumbed).count() >= 3, "the albums in got their thumbnails");
    assert_eq!(resume_of(&card)["parts"][0]["have"], 8000, "what came is kept");
    let generation = root.manifest.header.generation;

    // Still the same day: nothing asked of it, and nothing changed.
    let still = run_from(&card, &wanted, &mut day, 2).unwrap();
    assert_eq!((still.parked.as_deref(), still.written), (Some("the peer's day is spent"), 0));
    assert_eq!(day.asked_spent, 0);
    let seen = look(&card);
    assert_eq!(seen.root.unwrap().manifest.header.generation, generation, "no commit for nothing");
    assert!(seen.pending.is_some());

    // The next day: taken up where it stopped, and finished.
    day.spent = false;
    let done = run_from(&card, &wanted, &mut day, 3).unwrap();
    assert_eq!((done.parked, done.written, done.records, done.verified), (None, 2, 8, Ok(())));
    assert_eq!(day.server.asked.last(), Some(&(LATE.to_string(), 0, 0)));
    assert!(day.server.asked.contains(&(EARLY.to_string(), 8000, 8000)), "{:?}", day.server.asked);
    assert_eq!(on_card(&card, "Folk/Dana Duo/Album/01 - Big.mp3"), corpus("picture_first.mp3"));
    let _ = finished(&card);
}

/// One file its source's day is too short for (a peer's conversion larger
/// than what is left of it): it alone waits, the source's later files and
/// the other origin's go in, and the run ends at a checkpoint whose plan
/// keeps it; a run while it still waits doesn't ask it again and, changing
/// nothing, commits nothing; the next day's takes it, and finishes.
#[test]
fn a_file_its_day_is_too_short_for_waits_alone() {
    let dir = Scratch::new("waits-alone");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned { resumable: true, ..Canned::default() };
    let mut wanted = library(&mut server);
    let key = server_url_key("http://music.example:3000");
    let peer = Some(TalkPeer { id: 3, instance: NO_INSTANCE });
    wanted.origins.push(Origin {
        talk: Talk { url_key: key, peer, ..Talk::default() },
        server: Server::peer(NO_INSTANCE, key, 3),
        ..Origin::default()
    });
    for (path, name, id) in [(EARLY, "picture_first.mp3", 601), (LATE, "cbr.mp3", 602)] {
        let bytes = corpus(name);
        server.add(path, bytes.clone());
        wanted.files.push(WantedFile { origin: 1, ..track(path, &bytes, id) });
    }
    let mut day = Day::of(server, None, Some(EARLY));
    let parked = run_from(&card, &wanted, &mut day, 1).unwrap();
    let why = "the day is too short for one of its files";
    assert_eq!(parked.parked.as_deref(), Some(why));
    assert_eq!((parked.written, parked.records), (7, 7), "all but it: {:?}", parked.report);
    assert!(parked.report.failed.is_empty(), "it waits: {:?}", parked.report.failed);
    assert_eq!(day.asked_short, 1);
    assert!(day.server.asked.iter().any(|a| a.0 == LATE), "its source's next file all the same");
    let seen = look(&card);
    assert!(seen.pending.is_some() && !seen.root.as_ref().unwrap().manifest.is_final());
    let generation = seen.root.unwrap().manifest.header.generation;

    // Still too short: not asked again, and nothing changed.
    let still = run_from(&card, &wanted, &mut day, 2).unwrap();
    assert_eq!((still.parked.as_deref(), still.written), (Some(why), 0));
    assert_eq!(day.asked_short, 1);
    assert_eq!(look(&card).root.unwrap().manifest.header.generation, generation);

    // The next day: taken, and finished.
    day.short = None;
    let done = run_from(&card, &wanted, &mut day, 3).unwrap();
    assert_eq!((done.parked, done.written, done.records, done.verified), (None, 1, 8, Ok(())));
    assert_eq!(on_card(&card, "Folk/Dana Duo/Album/01 - Big.mp3"), corpus("picture_first.mp3"));
    let _ = finished(&card);
}

/// A cut run's leftover no write takes again (2.12.3), with the plan that
/// names it: the card after a first sync, and the plan's ops.
fn with_leftover(dir: &Path, card: &FatSim) {
    let root = look(card).root.unwrap().manifest;
    let ops = vec![
        Op { op: OP_FOLDER, path: "Gone".into(), expected_size: 0 },
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
    put(dir, "/.mstream/pending.bin", &pending, LATER);
    put(dir, "/music/Gone/x.mp3", b"leftovers", LATER);
}

/// 2.12.3 and 2.11 at step 7: a cut run's leftovers go before this run's
/// plan replaces the one that names them (a cut among them leaves them its
/// targets still), and each only while it is the file listed: one written
/// since is the listener's.
#[test]
fn a_cut_runs_leftovers_go_first_and_only_as_listed() {
    let dir = Scratch::new("leftovers");
    let (card, wanted, mut server) = first(&dir.0);
    with_leftover(&dir.0, &card);
    let start = card.log().len();
    let outcome = sync(&card, &wanted, &mut server, 8).unwrap();
    assert_eq!((outcome.verified, outcome.deleted), (Ok(()), 1));
    let log = card.log()[start..].to_vec();
    let at = |w: &str| log.iter().position(|l| l.starts_with(w)).unwrap_or_else(|| panic!("{w}: {log:?}"));
    assert!(at("delete /music/Gone/x.mp3") < at("create /.mstream/pending.tmp"), "{log:?}");

    // Written again between the listing and its turn: spared, not deleted.
    let dir = Scratch::new("leftovers-changed");
    let (card, wanted, mut server) = first(&dir.0);
    with_leftover(&dir.0, &card);
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    put(&dir.0, "/music/Gone/x.mp3", b"the listener's own now", LATER + 2);
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.verified, outcome.deleted), (Ok(()), 0));
    assert_eq!(outcome.report.spared, ["Gone/x.mp3"]);
    let kept = card.read("/music/Gone/x.mp3", 1 << 20).unwrap();
    assert_eq!(kept.as_deref(), Some(b"the listener's own now".as_slice()));
}

/// 2.12.3 at a collision: the plan on the card is written again at once,
/// without the write whose name the listener's file has, so a cut before
/// the next checkpoint never leaves that file named as the run's work.
#[test]
fn a_collision_leaves_the_plan_on_the_card_without_its_write() {
    let dir = Scratch::new("collision-plan");
    let (card, mut wanted, mut server) = first(&dir.0);
    let six = corpus("trck_0.mp3");
    let from = "Lib/Alex Artist/First Album/03 - Six.mp3";
    server.add(from, six.clone());
    wanted.files.push(track(from, &six, 401));
    let (session, plan) = plan_of(&card, &wanted, &mut server, Answers::default(), knobs());
    put(&dir.0, "/music/Alex Artist/First Album/03 - SIX.MP3", &corpus("latin1.mp3"), LATER);
    let start = card.log().len();
    let outcome = session.run(plan, &mut server, &mut NoHooks, &mut |_| {}).unwrap();
    assert_eq!((outcome.verified, outcome.written), (Ok(()), 0));
    let log = card.log()[start..].to_vec();
    let plans = log.iter().filter(|l| l.starts_with("rename /.mstream/pending.tmp")).count();
    assert_eq!(plans, 2, "step 6's, and the one written at the collision: {log:?}");
}

/// 2.12.1, step 12: the root's rename is flushed to the card before the
/// tags file it replaced goes.
#[test]
fn a_commits_root_is_flushed_before_the_tags_file_before_it_goes() {
    let dir = Scratch::new("flushed");
    let (card, mut wanted, mut server) = first(&dir.0);
    second(&dir.0, &mut wanted, &mut server);
    let start = card.log().len();
    sync(&card, &wanted, &mut server, 2).unwrap();
    let log = card.log()[start..].to_vec();
    let mut renames = log.iter().enumerate().filter(|(_, l)| l.contains("manifest.tmp -> "));
    let (at, _) = renames.next().expect("a commit");
    assert_eq!(log[at + 1], "sync /.mstream/manifest.bin", "{log:?}");
    let old_tags = log.iter().position(|l| l.starts_with("delete /.mstream/tags-")).unwrap();
    assert!(old_tags > at + 1, "{log:?}");
}

/// What the card refuses around the final commit, the card still in the
/// reader, never stops the run: a folder it won't remove stays OWNED for
/// the next run, a thumbnail it can't read is made again, and a leftover
/// it won't delete after the commit is a note.
#[test]
fn what_the_card_refuses_around_the_last_commit_is_the_next_runs_to_finish() {
    // The set-up card's empty OWNED folder, which the first sync prunes.
    let dir = Scratch::new("refused-prune");
    set_up(&dir.0);
    let card = FatSim::new(&dir.0, SERIAL);
    let mut server = Canned::default();
    let wanted = library(&mut server);
    card.refuse(Some("rmdir /music/Owned Empty"));
    let outcome = sync(&card, &wanted, &mut server, 1).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(outcome.verified, Ok(()));
    let folders = look(&card).root.unwrap().tags.unwrap().folders;
    let owned = folders.iter().any(|f| f.path == "Owned Empty" && f.flags & FOLDER_OWNED != 0);
    assert!(owned, "still the software's: {folders:?}");
    card.refuse(None);
    let mut more = wanted.clone();
    let six = corpus("trck_0.mp3");
    server.add("Lib/Casey Choir/Hymns/01 - Six.mp3", six.clone());
    more.files.push(track("Lib/Casey Choir/Hymns/01 - Six.mp3", &six, 401));
    sync(&card, &more, &mut server, 2).unwrap();
    assert!(card.stat("/music/Owned Empty").unwrap().is_none(), "the next run takes it");

    // A thumbnail it can't read.
    let thumb = card.image().into_keys().find(|p| p.starts_with("/.mstream/thumbs/")).expect("one");
    card.stumbles(&thumb);
    let mut again = more.clone();
    let seven = corpus("cbr.mp3");
    server.add("Lib/Casey Choir/Hymns/02 - Seven.mp3", seven.clone());
    again.files.push(track("Lib/Casey Choir/Hymns/02 - Seven.mp3", &seven, 402));
    let outcome = sync(&card, &again, &mut server, 3).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(outcome.verified, Ok(()));
    card.heal();

    // A leftover it won't delete once the commit is made.
    card.refuse(Some("delete /.mstream/pending.bin"));
    let mut last = again.clone();
    last.files.retain(|f| !f.ledger.server_path.ends_with("02 - Seven.mp3"));
    let outcome = sync(&card, &last, &mut server, 4).unwrap_or_else(|e| panic!("{e}"));
    card.refuse(None);
    assert_eq!(outcome.verified, Ok(()));
    assert!(outcome.notes.iter().any(|n| n.contains("clean-up")), "{:?}", outcome.notes);
}

/// 2.6.3's 248 bytes: a file the listener moved, or spelled, at a path no
/// record may name is never followed there (the commit would refuse it, and
/// every run after): it is the listener's, named in the report, and the
/// file is written again where the ledger can have it.
#[test]
fn a_file_moved_where_no_record_may_name_it_is_the_listeners_and_the_run_goes_on() {
    let dir = Scratch::new("long-move");
    let (card, wanted, mut server) = first(&dir.0);
    let one = "Alex Artist/First Album/01 - One.mp3";
    assert!(records(&card).iter().any(|e| e.path == one));
    let long = format!("Alex Artist/{}/01 - One.mp3", "x".repeat(230));
    std::fs::create_dir_all(dir.0.join("music/Alex Artist").join("x".repeat(230))).unwrap();
    std::fs::rename(dir.0.join("music").join(one), dir.0.join("music").join(&long)).unwrap();
    let outcome = sync(&card, &wanted, &mut server, 2).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(outcome.verified, Ok(()));
    assert_eq!(outcome.report.unfollowed, [long.clone()]);
    assert_eq!(outcome.written, 1, "written again where the ledger can have it");
    assert!(card.stat(&format!("/music/{long}")).unwrap().is_some(), "the listener's, as it was");
    assert!(records(&card).iter().any(|e| e.path == one));
    let again = sync(&card, &wanted, &mut server, 3).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((again.verified, again.written), (Ok(()), 0));
}
