//! The set-up on a FAT card in a host folder (the engine's [`FatSim`]: names
//! matched as FAT matches them, stamps at 2 s, cuts on demand), held to what
//! the transfer plan's B2 asks: the kit and the device read the result; a
//! set-up card is only looked at; the listener's music and the player's
//! files stay as they were; a copied card is said to be one, and takes a new
//! identity only when asked. And the state file's write, cut at each of its
//! writes, always reads back as the file before or the file after.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::device::card::contract::MAX_FILE;
use crate::device::card::contract::fat::fat_wall;
use crate::device::card::contract::msmf::{self, Comp, FINAL, Manifest};
use crate::device::card::contract::mptg::{self, Meta, SOURCE_SOFTWARE};
use crate::device::card::contract::mspd::{self, OP_WRITE, Op, PlanIn};
use crate::device::card::engine::fatsim::{Cut, FatSim};
use crate::device::card::volume::{Bus, CardFs, Clock, Disk, Fs, Inspection, Table, Volume};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A folder of its own under the system's temp folder, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("mstream-setup-{pid}-{name}-{n}"));
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

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/golden").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A host file of a card folder, written with a FAT stamp.
fn put(root: &Path, rel: &str, bytes: &[u8], fat_time: u32) {
    let path = root.join(rel.trim_start_matches('/'));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let wall = fat_wall(fat_time).expect("a stamp");
    let at = UNIX_EPOCH + Duration::from_secs((wall + 315_532_800) as u64);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
}

/// A whole folder copied, stamps and all: a card's files copied onto
/// another card.
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

const SERIAL: u32 = 0x1A2B_3C4D;
const OTHER: u32 = 0x5E6F_7A8B;
/// The tests' clock.
const NOW: u64 = 1_791_900_000;
/// The stamp the listener's own files have.
const LATER: u32 = 0x5D48_0000;

/// A session's clock and ids, fixed: the same set-up makes the same bytes.
fn env(seed: u64) -> Env {
    let mut n = seed;
    let id = move || {
        n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        n
    };
    Env { now: Box::new(|| NOW), id: Box::new(id), producer: "mstream-terminal test".into() }
}

/// One fixed env a session, each of its own.
fn envs(seed: u64) -> impl FnMut() -> Env {
    let mut n = seed * 1000;
    move || {
        n += 1;
        env(n)
    }
}

fn ask(label: &str) -> Ask {
    Ask { label: label.into(), new_identity: false }
}

fn run(card: &FatSim, ask: &Ask, cards: &mut Cards, seed: u64) -> Result<SetUp, Stop> {
    set_up(card, ask, &mut envs(seed), cards, &mut |_| {})
}

/// A session as a sync would open one.
fn open<'c>(card: &'c FatSim, answers: &Answers) -> Result<Session<'c>, Stop> {
    Session::open(card, &Expect::default(), Knobs::default(), answers, env(9), &mut NoHooks, &mut |_| {})
}

fn look(card: &dyn CardFs) -> Inspection {
    volume::inspect(card).expect("the card lists")
}

fn bytes(card: &FatSim, name: &str) -> Vec<u8> {
    card.read(&format!("/.mstream/{name}"), MAX_FILE).unwrap().unwrap_or_else(|| panic!("{name}"))
}

/// A card copied out for the firmware repo's reference readers
/// (tools/synthtransfer.py --check), when `MSTREAM_CARD_DUMP=<folder>` asks,
/// as the engine's tests do.
fn dump(name: &str, card: &FatSim) {
    if let Some(dir) = std::env::var_os("MSTREAM_CARD_DUMP") {
        let to = Path::new(&dir).join(name);
        let _ = std::fs::remove_dir_all(&to);
        copy_tree(&card.root(), &to);
    }
}

/// A cut first sync's plan on a card with no root, whose target is a file
/// of `/music` there: what a set-up must leave as it is.
fn cut_sync(dir: &Path) {
    put(dir, "/music/Alex Artist/x.mp3", &[1; 500], LATER);
    let op = Op { op: OP_WRITE, path: "Alex Artist/x.mp3".into(), expected_size: 500 };
    let (generation, card_id, run_id) = (1, 0x5EED, 1);
    let plan = PlanIn { generation, card_id, minor: 0, run_id, base_generation: 0, ops: vec![op] };
    put(dir, "/.mstream/pending.bin", &mspd::write(&plan).unwrap(), LATER);
}

/// How many writes a card's log holds up to and including `write`: where a
/// test swaps the card in the reader.
fn through(log: &[String], write: &str) -> u64 {
    let at = log.iter().position(|w| w == write).unwrap_or_else(|| panic!("{write}: {log:?}"));
    at as u64 + 1
}

/// The card's files under a folder, each with its size, stamp and bytes'
/// hash.
fn under(card: &FatSim, prefix: &str) -> BTreeMap<String, (u64, u32, u64)> {
    card.image().into_iter().filter(|(p, _)| p.starts_with(prefix)).collect()
}

#[test]
fn a_new_card_is_set_up_as_the_kit_and_the_device_read_it() {
    let dir = Scratch::new("new");
    let card = FatSim::new(&dir.0, SERIAL);
    let mut cards = Cards::default();
    let done = run(&card, &ask("  Car  "), &mut cards, 1).unwrap();
    assert!(done.made_music && done.committed && done.wrote_state);
    assert_eq!((done.generation, done.label.as_str(), done.was), (1, "Car", None));
    assert_eq!((done.serial.clone(), done.copied.clone()), (Some(Serial::Fat(SERIAL)), vec![]));
    assert_ne!(done.card_id, 0);

    // `/music`, the root, its tags file, the state file: nothing else, and
    // no leftover (run.lock, a `.new`, a `.tmp`).
    let files: Vec<String> = card.image().into_keys().collect();
    let want = ["/.mstream/manifest.bin", "/.mstream/state.json", "/.mstream/tags-00000001.bin"];
    assert_eq!(files, want);
    assert_eq!(card.list("/music").unwrap(), [], "an empty /music");
    // Made in that order: /music, the commit, then the state file — and no
    // write anywhere else.
    let log = card.log();
    let at = |w: &str| log.iter().position(|l| l == w).unwrap_or_else(|| panic!("{w}: {log:?}"));
    let commit = at("rename /.mstream/manifest.tmp -> /.mstream/manifest.bin");
    assert!(at("mkdir /music") < commit && commit < at("rename /.mstream/state.new -> /.mstream/state.json"));
    assert!(log.iter().all(|w| w.contains(" /.mstream") || w == "mkdir /music"), "{log:?}");

    // The kit reads it as the card's root, final, of no server, no library
    // root, no Auto DJ index, no plays acknowledged.
    let seen = look(&card);
    assert!(seen.newer.is_empty() && seen.pending.is_none());
    let root = seen.root.as_ref().expect("a root");
    let m = &root.manifest;
    assert_eq!((root.file, root.twice, m.is_final()), ("manifest.bin", false, true));
    assert_eq!((m.header.generation, m.header.card_id, m.commit_id), (1, done.card_id, done.commit_id));
    assert_eq!((m.base_generation, m.server_url_key, m.server_instance), (0, 0, [0; 16]));
    assert!(m.roots.is_empty() && m.servers.is_empty() && m.acks.is_empty() && m.autodj.is_none());
    let named = (m.tags.name(), m.commit_time, m.producer.as_str());
    assert_eq!(named, (String::from("tags-00000001.bin"), NOW as u32, "mstream-terminal test"));
    let tags = root.tags.as_ref().expect("its tags file");
    assert_eq!((tags.source, tags.records.len(), tags.has_ledger), (SOURCE_SOFTWARE, 0, false));
    let folders: Vec<(&str, u32)> = tags.folders.iter().map(|f| (f.path.as_str(), f.flags)).collect();
    assert_eq!(folders, [("", 0)], "FOLD holds /music alone");

    // The device reads both as it would: the root, and the tags file it
    // names.
    let device = Manifest::read(&bytes(&card, "manifest.bin"), msmf::Uses::DEVICE).expect("the root");
    let theirs = mptg::read(&bytes(&card, "tags-00000001.bin"), mptg::Uses::DEVICE).expect("the tags");
    assert!(msmf::companion_matches(&device.tags, &theirs.header, theirs.source, None));

    // The fixtures' shape: the golden empty commit as the same writer makes
    // it, with this card's id and writer and without its OWNED folder; the
    // golden checkpoint root naming it, made final.
    let golden = mptg::read(&fixture("tags-00000001.bin"), mptg::Uses::ALL).unwrap();
    assert_eq!((golden.header.generation, golden.source, golden.records.len()), (1, SOURCE_SOFTWARE, 0));
    let (meta, records, _) = golden.to_input();
    let (card_id, parser_version, producer) = (done.card_id, tags.parser_version, tags.producer.clone());
    let meta = Meta { card_id, parser_version, producer, ..meta };
    let ours = bytes(&card, "tags-00000001.bin");
    assert_eq!(mptg::write(&meta, &records, &[]).unwrap(), ours);
    let mut min = Manifest::read(&fixture("manifest-min.bin"), msmf::Uses::ALL).unwrap().to_input();
    assert_eq!((min.generation, min.flags, min.server_url_key, min.roots.len()), (1, 0, 0, 0));
    (min.card_id, min.commit_id, min.flags) = (done.card_id, done.commit_id, FINAL);
    (min.commit_time, min.producer) = (m.commit_time, m.producer.clone());
    min.comps = Comp::of(&ours).into_iter().collect();
    assert_eq!(msmf::write(&min).unwrap(), bytes(&card, "manifest.bin"));

    // The state file: the card's id, the label, the serial, when.
    let state = State::of(&seen.state).expect("a state file");
    assert_eq!(state, State { seq: 1, ..State::new(done.card_id, "Car", Some(&Serial::Fat(SERIAL)), NOW) });
    let text = String::from_utf8(bytes(&card, "state.json")).unwrap();
    assert!(!text.contains("://") && !text.contains("example"), "no address: {text}");
    dump("setup-new", &card);
    // And this computer's cache.
    let seen = &cards.cards[&done.card_id];
    let cached = (seen.label.as_str(), seen.last_seen, seen.generation, seen.commit_id);
    assert_eq!(cached, ("Car", NOW, 1, done.commit_id));
}

#[test]
fn a_set_up_card_is_only_looked_at() {
    let dir = Scratch::new("again");
    let card = FatSim::new(&dir.0, SERIAL);
    let mut cards = Cards::default();
    let done = run(&card, &ask("Car"), &mut cards, 1).unwrap();
    let (image, writes) = (card.image(), card.log().len());
    for seed in [2, 3] {
        let again = run(&card, &ask("Another name"), &mut cards, seed).unwrap();
        assert!(!again.made_music && !again.committed && !again.wrote_state && again.copied.is_empty());
        let ids = (again.card_id, again.generation, again.commit_id, again.label.as_str());
        assert_eq!(ids, (done.card_id, 1, done.commit_id, "Car"), "its id and its label stay");
        assert_eq!(card.image(), image);
        assert_eq!(card.log().len(), writes, "not a write, run.lock included");
    }

    // A card another tool set up (the fixtures' checkpoint root and its
    // tags file, with an OWNED empty folder) keeps its root and id: only
    // its state file is new. Then it too is only looked at.
    let other = Scratch::new("golden");
    put(&other.0, "/.mstream/manifest.bin", &fixture("manifest-min.bin"), LATER);
    put(&other.0, "/.mstream/tags-00000001.bin", &fixture("tags-00000001.bin"), LATER);
    std::fs::create_dir_all(other.0.join("music/Owned Empty")).unwrap();
    let card = FatSim::new(&other.0, SERIAL);
    let before = under(&card, "/");
    let done = run(&card, &ask("Gym"), &mut cards, 4).unwrap();
    assert!(!done.made_music && !done.committed && done.wrote_state);
    assert_eq!((done.card_id, done.generation, done.commit_id), (0x5EED_C0DE_0A1B_2C3D, 1, 1));
    let after = under(&card, "/");
    let added: Vec<&String> = after.keys().filter(|p| !before.contains_key(*p)).collect();
    assert_eq!(added, ["/.mstream/state.json"]);
    assert!(before.iter().all(|(p, f)| after.get(p) == Some(f)), "the root and its tags file as they were");
    assert!(card.stat("/music/Owned Empty").unwrap().is_some());
    let state = State::of(&look(&card).state).unwrap();
    assert_eq!((state.card_id, state.label.as_str()), (0x5EED_C0DE_0A1B_2C3D, "Gym"));
    let writes = card.log().len();
    assert!(!run(&card, &ask(""), &mut cards, 5).unwrap().wrote_state);
    assert_eq!(card.log().len(), writes);

    // A state file of another identity (a set-up cut between a new id's
    // commit and its state file) is the card's again, its label kept.
    let stale = State::new(0x0BAD, "Gym", Some(&Serial::Fat(SERIAL)), 3);
    put(&other.0, "/.mstream/state.json", &stale.text(4), LATER);
    let done = run(&card, &ask("Not this"), &mut cards, 6).unwrap();
    assert!(done.wrote_state && !done.committed);
    let state = State::of(&look(&card).state).unwrap();
    assert_eq!((state.card_id, state.seq, state.label.as_str()), (0x5EED_C0DE_0A1B_2C3D, 5, "Gym"));

    // A label this software may not write (an address, put there by hand)
    // isn't written again: the listener's is.
    let by_hand = State::new(0x0BAD, "alex@music.example", Some(&Serial::Fat(SERIAL)), 3);
    put(&other.0, "/.mstream/state.json", &by_hand.text(6), LATER);
    let done = run(&card, &ask("Gym"), &mut cards, 7).unwrap();
    assert_eq!((done.wrote_state, done.label.as_str()), (true, "Gym"));
    let text = String::from_utf8(bytes(&card, "state.json")).unwrap();
    assert!(!text.contains("example") && text.contains("\"label\": \"Gym\""), "{text}");
}

#[test]
fn the_listeners_music_and_the_players_files_stay_as_they_are() {
    let dir = Scratch::new("music");
    put(&dir.0, "/Music/Alex Artist/First Album/01 - One.mp3", &[7; 3000], LATER);
    put(&dir.0, "/Music/Mine/notes.txt", b"the listener's", LATER);
    put(&dir.0, "/.player/device.txt", &fixture("device.txt"), LATER);
    put(&dir.0, "/.player/tags.bin", &fixture("tags-device.bin"), LATER);
    put(&dir.0, "/notes.txt", b"beside the music", LATER);
    let card = FatSim::new(&dir.0, SERIAL);
    let before = card.image();
    let done = run(&card, &ask("Car"), &mut Cards::default(), 1).unwrap();
    assert!(done.committed && done.wrote_state);
    assert!(!done.made_music, "the card's own /Music, in its case");
    let after = card.image();
    for (path, file) in &before {
        assert_eq!(after.get(path), Some(file), "{path}");
    }
    assert!(after.keys().filter(|p| !before.contains_key(*p)).all(|p| p.starts_with("/.mstream/")));
    assert!(card.log().iter().all(|w| w.contains(" /.mstream")), "{:?}", card.log());
    dump("setup-music", &card);
    // The music is the listener's: no ledger lists it.
    let seen = look(&card);
    assert_eq!((seen.music.audio, seen.music.unlisted), (1, 1));
    assert_eq!(seen.root.unwrap().tags.unwrap().records.len(), 0);

    // A cut sync's plan whose target is on the card: the set-up won't
    // finish it (that would delete a file of `/music`), and writes nothing
    // there.
    let cut = Scratch::new("cut");
    cut_sync(&cut.0);
    let card = FatSim::new(&cut.0, SERIAL);
    let before = under(&card, "/music/");
    let stop = run(&card, &ask("Car"), &mut Cards::default(), 2).unwrap_err();
    assert!(matches!(stop, Stop::Refused(_)), "{stop}");
    assert_eq!(under(&card, "/music/"), before);
    let seen = look(&card);
    assert!(seen.root.is_none() && seen.state.value.is_none() && seen.pending.is_some());
    assert!(card.stat("/.mstream/run.lock").unwrap().is_none(), "the card let go");
}

#[test]
fn a_copied_card_is_said_to_be_one_and_takes_a_new_identity_when_asked() {
    let a = Scratch::new("a");
    let card_a = FatSim::new(&a.0, SERIAL);
    let mut cards = Cards::default();
    let first = run(&card_a, &ask("Car"), &mut cards, 1).unwrap();
    put(&a.0, "/music/Alex Artist/01 - One.mp3", &[7; 3000], LATER);
    let image_a = card_a.image();

    // Its files copied onto a card of another serial, looked at on another
    // computer: the state file names the serial it was written on.
    let b = Scratch::new("b");
    copy_tree(&a.0, &b.0);
    let card_b = FatSim::new(&b.0, OTHER);
    let mut elsewhere = Cards::default();
    let seen = run(&card_b, &ask(""), &mut elsewhere, 2).unwrap();
    let sign = Copied::Serial { was: Serial::Fat(SERIAL), now: Serial::Fat(OTHER) };
    assert_eq!(seen.copied, [sign.clone()]);
    assert!(!seen.committed && !seen.wrote_state && seen.card_id == first.card_id);
    assert!(card_b.log().is_empty(), "said, and nothing written");

    // The listener's answer: an identity of its own — a commit of the same
    // files under a new id, the state file rewritten for it, its label kept.
    let music = under(&card_b, "/music/");
    let new = Ask { label: "Ignored".into(), new_identity: true };
    let done = run(&card_b, &new, &mut elsewhere, 3).unwrap();
    assert!(done.committed && done.wrote_state && !done.made_music);
    assert_eq!((done.was, done.generation, done.copied.clone()), (Some(first.card_id), 2, vec![sign]));
    assert!(done.card_id != first.card_id && done.card_id != 0);
    let seen = look(&card_b);
    let root = seen.root.as_ref().unwrap();
    assert_eq!((root.manifest.header.card_id, root.manifest.commit_id), (done.card_id, done.commit_id));
    assert!(root.manifest.is_final());
    assert_eq!(root.tags.as_ref().unwrap().header.card_id, done.card_id, "the tags file carries it");
    let state = State::of(&seen.state).unwrap();
    let id = (state.card_id, state.seq, state.label.as_str(), state.fat_serial, state.created);
    assert_eq!(id, (done.card_id, 2, "Car", Some(OTHER), NOW));
    assert_eq!(under(&card_b, "/music/"), music, "the music as it was");
    assert!(!under(&card_b, "/.mstream/").contains_key("/.mstream/tags-00000001.bin"), "collected");
    dump("setup-new-identity", &card_b);
    // No longer a copy: looked at, nothing to say.
    let writes = card_b.log().len();
    let again = run(&card_b, &ask(""), &mut elsewhere, 4).unwrap();
    assert!(again.copied.is_empty() && !again.committed && !again.wrote_state);
    assert_eq!(card_b.log().len(), writes);
    // The card it was copied from keeps its own.
    assert_eq!(card_a.image(), image_a);
    assert!(run(&card_a, &ask(""), &mut cards, 5).unwrap().copied.is_empty());

    // A byte-level clone keeps the serial: only this computer's cache, which
    // saw the card id at generation 1 with another commit, tells it.
    let c = Scratch::new("c");
    copy_tree(&a.0, &c.0);
    let mut root = Manifest::read(&bytes(&card_a, "manifest.bin"), msmf::Uses::ALL).unwrap().to_input();
    root.commit_id ^= 0xFFFF;
    put(&c.0, "/.mstream/manifest.bin", &msmf::write(&root).unwrap(), LATER);
    let clone = FatSim::new(&c.0, SERIAL);
    let seen = run(&clone, &ask(""), &mut cards, 6).unwrap();
    let sign = Copied::Diverged { generation: 1, seen: first.commit_id, now: first.commit_id ^ 0xFFFF };
    assert_eq!(seen.copied, [sign.clone()]);
    assert!(!seen.committed && !seen.wrote_state);
    // The cache keeps the commit it saw first, at every look: the card the
    // clone was made from is its own, and the clone is still the copy.
    for seed in [7, 8] {
        assert!(run(&card_a, &ask(""), &mut cards, seed).unwrap().copied.is_empty());
        assert_eq!(run(&clone, &ask(""), &mut cards, seed).unwrap().copied, [sign.clone()]);
    }
    assert_eq!(cards.cards[&first.card_id].commit_id, first.commit_id);
}

#[test]
fn a_card_swapped_in_the_reader_mid_set_up_is_never_written() {
    // A new card's set-up, as the cards below make it: the commit's session
    // lets the card go, then a session of its own opens for the state file.
    let reference = Scratch::new("reference");
    let card = FatSim::new(&reference.0, SERIAL);
    run(&card, &ask("Car"), &mut Cards::default(), 1).unwrap();
    let log = card.log();
    let released = through(&log, "delete /.mstream/run.lock");
    assert_eq!(log[released as usize], "lock /.mstream/run.lock", "{log:?}");

    // Swapped there for a card of the same serial — an empty one (what the
    // card was before its commit) or another set-up card: the second
    // session is bound to the commit just made, and stops before it writes.
    let theirs = Scratch::new("theirs");
    run(&FatSim::new(&theirs.0, SERIAL), &ask("Gym"), &mut Cards::default(), 2).unwrap();
    let empty = Scratch::new("empty");
    for other in [&empty, &theirs] {
        let before = FatSim::new(&other.0, SERIAL).image();
        let dir = Scratch::new("first");
        let card = FatSim::new(&dir.0, SERIAL);
        card.swap_after(released, &other.0, SERIAL);
        let mut cards = Cards::default();
        let stop = run(&card, &ask("Car"), &mut cards, 1).unwrap_err();
        assert!(matches!(stop, Stop::DifferentCard), "{stop}");
        assert_eq!(card.log().len() as u64, released, "not a write after the swap");
        assert_eq!(FatSim::new(&other.0, SERIAL).image(), before);
        assert!(std::fs::read_dir(&empty.0).unwrap().next().is_none(), "not even /.mstream");
        assert!(cards.cards.is_empty(), "nothing cached of a set-up that stopped");
        // The first card has its commit and no state file: the next set-up
        // writes it, the commit kept.
        let first = FatSim::new(&dir.0, SERIAL);
        let seen = look(&first);
        let m = seen.root.expect("its commit").manifest;
        assert!(m.is_final() && seen.state.value.is_none());
        let done = run(&first, &ask("Car"), &mut cards, 3).unwrap();
        assert!(!done.committed && done.wrote_state);
        assert_eq!((done.card_id, done.commit_id), (m.header.card_id, m.commit_id));
    }

    // A new identity's commit, the card swapped before it is read back:
    // after the run's last write, the collection of a thumbnail no album
    // names. The set-up stops, and says the id the card was given.
    let a = Scratch::new("a");
    run(&FatSim::new(&a.0, SERIAL), &ask("Car"), &mut Cards::default(), 4).unwrap();
    put(&a.0, "/.mstream/thumbs/A/A1B2C3D4.565", &[0; 64], LATER);
    let new = Ask { label: String::new(), new_identity: true };
    let reference = Scratch::new("reference-copy");
    copy_tree(&a.0, &reference.0);
    let card = FatSim::new(&reference.0, OTHER);
    run(&card, &new, &mut Cards::default(), 5).unwrap();
    let collected = through(&card.log(), "delete /.mstream/thumbs/A/A1B2C3D4.565");
    assert_eq!(card.log()[collected as usize], "delete /.mstream/run.lock", "then the read back");
    let (b, c) = (Scratch::new("b"), Scratch::new("c"));
    copy_tree(&a.0, &b.0);
    copy_tree(&a.0, &c.0);
    let before = FatSim::new(&c.0, OTHER).image();
    let card = FatSim::new(&b.0, OTHER);
    card.swap_after(collected, &c.0, OTHER);
    let stop = run(&card, &new, &mut Cards::default(), 5).unwrap_err();
    assert_eq!(card.log().len() as u64, collected, "not a write after the swap");
    assert_eq!(FatSim::new(&c.0, OTHER).image(), before);
    let card = FatSim::new(&b.0, OTHER);
    let m = look(&card).root.expect("the new identity's commit").manifest;
    let Stop::Refused(why) = stop else { panic!("{stop}") };
    assert!(why.contains(&hex(m.header.card_id)) && why.contains(&hex(m.commit_id)), "{why}");
    assert!(why.contains("doesn't read back"), "{why}");
    // Nothing is lost: the next look writes the state file of the id the
    // card was given, its label kept.
    let done = run(&card, &ask(""), &mut Cards::default(), 6).unwrap();
    assert!(!done.committed && done.wrote_state);
    assert_eq!((done.card_id, done.generation, done.label.as_str()), (m.header.card_id, 2, "Car"));
}

#[test]
fn the_state_file_is_written_by_its_rule_and_every_cut_reads_back_whole() {
    let template = Scratch::new("template");
    let card = FatSim::new(&template.0, SERIAL);
    run(&card, &ask("Car"), &mut Cards::default(), 1).unwrap();

    // Refused before any write: not the next seq, not a state file at all.
    let mut session = open(&card, &Answers::default()).unwrap();
    let old = State::of(session.state()).unwrap();
    let start = card.log().len();
    let renamed = State { label: "Car, renamed".into(), ..old.clone() };
    for bad in [renamed.text(1), renamed.text(3), b"{}".to_vec(), b"[1]".to_vec()] {
        assert!(matches!(session.write_state(&bad), Err(Stop::Refused(_))));
    }
    assert_eq!(card.log().len(), start);
    // The write: `.new` whole and flushed, the old file deleted, the rename.
    session.write_state(&renamed.text(2)).unwrap();
    assert_eq!(session.state().seq, 2);
    let third = State { label: "Car, again".into(), ..old.clone() };
    session.write_state(&third.text(3)).unwrap();
    session.close();
    let want = [
        "create /.mstream/state.new",
        "write /.mstream/state.new",
        "sync /.mstream/state.new",
        "delete /.mstream/state.json",
        "rename /.mstream/state.new -> /.mstream/state.json",
    ];
    let log = card.log();
    assert_eq!(log[start..start + 5], want);
    assert_eq!(State::of(&look(&card).state).unwrap().label, "Car, again");
    // A leftover `.new` is deleted first.
    let mut session = open(&card, &Answers::default()).unwrap();
    put(&template.0, "/.mstream/state.new", b"torn", LATER);
    let start = card.log().len();
    session.write_state(&old.text(4)).unwrap();
    session.close();
    assert_eq!(card.log()[start], "delete /.mstream/state.new");

    // A cut at each of the write's five, of either kind: the card reads the
    // file before or the file after, and the next session's recovery leaves
    // `state.json` alone, that same one. Both come about.
    let mut seen = Vec::new();
    for n in 1..=5 {
        for how in [Cut::Fail, Cut::Between] {
            let dir = Scratch::new("cut");
            copy_tree(&template.0, &dir.0);
            let card = FatSim::new(&dir.0, SERIAL);
            let mut session = open(&card, &Answers::default()).unwrap();
            card.cut_at(n, how);
            let stop = session.write_state(&renamed.text(5)).unwrap_err();
            assert!(matches!(stop, Stop::Pulled), "{n} {how:?}: {stop}");
            drop(session);
            card.replug();
            let label = |seen: &Inspection| State::of(&seen.state).map(|s| (s.seq, s.label));
            let cut = label(&look(&card)).unwrap_or_else(|| panic!("{n} {how:?}: no state"));
            assert!([(4, "Car".to_string()), (5, "Car, renamed".to_string())].contains(&cut), "{n} {how:?}");
            open(&card, &Answers { disk_checked: true, ..Answers::default() }).unwrap().close();
            assert_eq!(label(&look(&card)), Some(cut.clone()), "{n} {how:?}");
            let left = under(&card, "/.mstream/");
            assert!(!left.contains_key("/.mstream/state.new") || n == 5 && how == Cut::Between, "{n} {how:?}");
            seen.push(cut.0);
        }
    }
    // Cut before the new file is whole, the file before; once it is, the new
    // one.
    assert_eq!(seen, [4, 4, 4, 4, 5, 5, 5, 5, 5, 5]);
}

// ── The hidden command ──────────────────────────────────────────────────────

fn volume_at(path: &Path, device: &str, fs: Fs) -> Volume {
    Volume {
        path: path.to_path_buf(),
        device: device.into(),
        fs,
        table: Table::Mbr,
        label: "CARD".into(),
        serial: Some(Serial::Fat(SERIAL)),
        space: None,
        read_only: false,
        utf8: Some(true),
        clock: Clock::Offset(0),
    }
}

fn card_disk() -> Disk {
    let (system, others) = (Vec::new(), Vec::new());
    Disk { id: "sdz".into(), removable: true, bus: Bus::Usb, read_only: false, system, others }
}

#[test]
fn the_hidden_command_sets_up_only_a_volume_the_rule_accepts() {
    let (good, bad, cut) = (Scratch::new("good"), Scratch::new("exfat"), Scratch::new("cut"));
    let found = [
        Found::new(volume_at(&bad.0, "sdy1", Fs::ExFat), card_disk()),
        Found::new(volume_at(&good.0, "sdz1", Fs::Fat32), card_disk()),
        Found::new(volume_at(&cut.0, "sdx1", Fs::Fat32), card_disk()),
    ];
    // What a run that crashed leaves: run.lock with no lock behind it.
    let stale = br#"{"format": "mstream-card-run", "runId": "0x0000000000000001", "started": 1}"#;
    let mut cards = Cards::default();
    let mut call = |named: &str, label: Option<&str>| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = on(&found, named, label, false, &mut cards, &mut envs(1), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    };
    let none = |dir: &Path| std::fs::read_dir(dir).unwrap().next().is_none();

    let (code, out, err) = call("not-a-volume", None);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.contains("isn't a volume"), "{err}");
    let (code, _, err) = call(&bad.0.to_string_lossy(), None);
    assert_eq!(code, 2);
    assert!(err.contains("exfat"), "{err}");
    assert!(none(&bad.0), "a refused volume is never opened");
    let (code, _, err) = call("sdz1", Some("http://music.example:3000"));
    assert_eq!(code, 1);
    assert!(err.contains("label-address"), "{err}");
    assert!(none(&good.0), "a label that names an address writes nothing");

    // By where it is mounted, with a separator after it; its own label. The
    // stale run.lock is taken over, and the JSON says so.
    put(&good.0, "/.mstream/run.lock", stale, LATER);
    let named = format!("{}{}", good.0.display(), std::path::MAIN_SEPARATOR);
    let (code, out, err) = call(&named, None);
    assert_eq!((code, err.as_str()), (0, ""));
    let said: Value = serde_json::from_str(&out).unwrap();
    let fields = (said["committed"].as_bool(), said["label"].as_str(), said["generation"].as_u64());
    assert_eq!(fields, (Some(true), Some("CARD"), Some(1)));
    assert!(good.0.join(".mstream").join("manifest.bin").is_file() && good.0.join("music").is_dir());
    let events = said["events"].as_array().expect("the events");
    assert_eq!(events[0], json!({ "event": "tookOver", "note": String::from_utf8_lossy(stale) }));
    assert_eq!(events.last().unwrap()["event"], "committed", "{events:?}");
    // Again, by the OS's name for it: looked at.
    let (code, out, _) = call("sdz1", Some("Car"));
    let again: Value = serde_json::from_str(&out).unwrap();
    let fields = (code, again["committed"].as_bool(), again["label"].as_str());
    assert_eq!(fields, (0, Some(false), Some("CARD")));
    assert_eq!(again["cardId"], said["cardId"]);
    assert_eq!(again["events"], json!([]));

    // A set-up that stops says the engine's events before why.
    cut_sync(&cut.0);
    put(&cut.0, "/.mstream/run.lock", stale, LATER);
    let (code, out, err) = call("sdx1", None);
    assert_eq!((code, out.as_str()), (1, ""));
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 2, "{err}");
    assert!(lines[0].contains("\"tookOver\"") && lines[1].contains("the set-up stopped"), "{err}");
    assert_eq!(cards.cards.len(), 1, "the one set up");
}
