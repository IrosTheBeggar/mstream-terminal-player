//! The play history on cards in host folders ([`FatSim`]): the shared
//! fixtures' golden roots and tags files with the play log's goldens beside
//! them, or logs written slot by slot from the ledger's own rows; and a
//! loopback mStream that answers `stats/plays` as its ingest does (an id it
//! has is a duplicate, the rest accepted unless told to reject them), which
//! an older server refuses whole for a `device` source.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;
use crate::api::transfer::stub::{Asked, json as json_reply, routed};
use crate::device::card::contract::{mptg, msmf};
use crate::device::card::contract::prim::path_hash;
use crate::device::card::engine::fatsim::FatSim;

// ── The outbox's store ──────────────────────────────────────────────────────

/// The outbox in memory. With `saves_left`, that many saves, then each one
/// refused: what a crash leaves on disk is the last save it took.
#[derive(Default)]
pub(crate) struct MemStore {
    pub(crate) text: RefCell<Option<String>>,
    pub(crate) saves_left: Cell<Option<usize>>,
}

impl Store for MemStore {
    fn load(&self) -> Result<Option<String>, String> {
        Ok(self.text.borrow().clone())
    }

    fn save(&self, text: &str) -> Result<(), String> {
        if let Some(left) = self.saves_left.get() {
            if left == 0 {
                return Err("the process died".into());
            }
            self.saves_left.set(Some(left - 1));
        }
        *self.text.borrow_mut() = Some(text.to_string());
        Ok(())
    }
}

impl MemStore {
    pub(crate) fn outbox(&self) -> Outbox {
        Outbox::load(self).expect("the outbox")
    }
}

// ── The card ────────────────────────────────────────────────────────────────

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A folder of its own under the system's temp folder, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mstream-plays-{}-{name}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture(rel: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card").join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn vectors() -> Value {
    serde_json::from_slice(&fixture("vectors.json")).expect("vectors.json")
}

const SERIAL: u32 = 0x1A2B_3C4D;
const CARD_ID: u64 = 0x5EED_C0DE_0A1B_2C3D;
const ONE: [u8; 6] = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01];
const TWO: [u8; 6] = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x02];
const EPOCH_1: u64 = 0x1122_3344_5566_7788;
const EPOCH_2: u64 = 0xF0E1_D2C3_B4A5_9687;
/// The golden root's server, and another.
const HOME: &str = "http://music.example:3000";
const OTHER: &str = "https://second.example/mstream";

/// The golden root and the files it names: `manifest.bin` (generation 42,
/// one server) or `manifest-servers.bin` (generation 7, a server and two
/// peers besides the root's own).
#[derive(Clone, Copy)]
enum Golden {
    One,
    Several,
}

/// A card of a golden root, its player's `device.txt`, and these log files.
fn card(scratch: &Scratch, golden: Golden, logs: &[(&str, Vec<u8>)]) -> FatSim {
    let root = scratch.0.join("card");
    let files: &[(&str, &str)] = match golden {
        Golden::One => &[
            ("manifest.bin", "manifest.bin"),
            ("tags-0000002a.bin", "tags-0000002a.bin"),
            ("autodj-00000029.bin", "autodj-00000029.bin"),
        ],
        Golden::Several => {
            &[("manifest.bin", "manifest-servers.bin"), ("tags-00000007.bin", "tags-00000007.bin")]
        }
    };
    std::fs::create_dir_all(root.join(".mstream")).unwrap();
    std::fs::create_dir_all(root.join(".player")).unwrap();
    std::fs::create_dir_all(root.join("music")).unwrap();
    for (name, from) in files {
        std::fs::write(root.join(".mstream").join(name), fixture(&format!("golden/{from}"))).unwrap();
    }
    std::fs::write(root.join(".player/device.txt"), fixture("golden/device.txt")).unwrap();
    for (name, bytes) in logs {
        std::fs::write(root.join(".player").join(name), bytes).unwrap();
    }
    FatSim::new(&root, SERIAL)
}

/// A golden log's files, by name (`plays/golden/sync-1`, say).
fn golden_log(dir: &str) -> Vec<(&'static str, Vec<u8>)> {
    let at = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card").join(dir);
    let mut out = Vec::new();
    for name in NAMES {
        if let Ok(bytes) = std::fs::read(at.join(name)) {
            out.push((name, bytes));
        }
    }
    out
}

fn root_of(card: &FatSim) -> Root {
    volume::inspect(card).expect("the card reads").root.expect("a root")
}

fn tags_of(root: &Root) -> &Tags {
    root.tags.as_ref().expect("its tags file")
}

/// A play of the ledger's row at `path`, as the device logs it: the row's
/// size and stamp, a start, boot 1.
fn play_of(root: &Root, path: &str, device_id: [u8; 6], epoch: u64, seq: u32) -> Slot {
    let row = tags_of(root).records.iter().find(|e| e.path == path).unwrap_or_else(|| panic!("{path}"));
    Slot {
        kind: mppl::KIND_PLAY,
        outcome: 1,
        source: 1,
        seq,
        clock: 2,
        epoch,
        device_id,
        started_at: 1_791_900_000_000 + seq as u64 * 200_000,
        boot_seq: 1,
        uptime_ms: seq * 200_000,
        played_ms: 180_000,
        duration_ms: 180_000,
        path_hash: path_hash(path),
        size: row.rec.size,
        fat_time: row.rec.fat_time,
        generation: 42,
        ..Slot::default()
    }
}

fn log_bytes(slots: &[Slot]) -> Vec<u8> {
    slots.iter().flat_map(|s| s.encode()).collect()
}

fn talk(url: &str) -> Talk {
    Talk { url_key: server_url_key(url), ..Talk::default() }
}

fn ack(device_id: [u8; 6], epoch: u64, seq: u32) -> Ack {
    Ack { device_id, epoch, seq }
}

/// A card's state file with no marks.
fn none() -> (u64, Vec<Ack>) {
    (CARD_ID, Vec::new())
}

// ── The server ──────────────────────────────────────────────────────────────

/// A loopback mStream's play ingest, and its `/api/` and manifest.
#[derive(Default)]
struct Mstream {
    /// The plays it has, by id: posted again, a duplicate.
    has: HashSet<String>,
    /// Ids it rejects, and why.
    reject: HashMap<String, &'static str>,
    /// Older than the card transfer's batch: no `instanceId`, and a batch
    /// with a `device` source refused whole (a 400).
    old: bool,
    /// No Stats API: nothing to post to.
    no_stats: bool,
    /// The manifest's entries.
    entries: Vec<Value>,
    /// Each `stats/plays` body, as it came.
    posts: Vec<Value>,
}

type Fake = Arc<Mutex<Mstream>>;

fn answer(fake: &Fake, asked: &Asked) -> Vec<u8> {
    let mut m = fake.lock().unwrap();
    match (asked.method.as_str(), asked.path().as_str()) {
        ("GET", "/api/") => {
            let features = json!({"sync": true});
            let mut body = json!({"server": "6.31.1", "features": features, "user": {"vpaths": ["Music"]}});
            if !m.no_stats {
                body["features"]["stats"] = json!(2);
            }
            if !m.old {
                body["instanceId"] = json!("3f2b8c1e-6a4d-4e0f-9b7a-2c5d8e1f0a93");
            }
            json_reply("200 OK", &body.to_string())
        }
        ("POST", "/api/v1/sync/manifest") => {
            let body = json!({"revision": "1:1", "scanning": false, "next": null, "entries": m.entries});
            json_reply("200 OK", &body.to_string())
        }
        ("POST", "/api/v1/stats/plays") => {
            let body = asked.json();
            m.posts.push(body.clone());
            let plays = body["plays"].as_array().cloned().unwrap_or_default();
            if m.old && plays.iter().any(|p| p["source"] == "device") {
                return json_reply("400 Bad Request", r#"{"error":"Validation Error"}"#);
            }
            let (mut accepted, mut duplicates, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
            for p in plays {
                let id = p["id"].as_str().unwrap_or_default().to_string();
                if let Some(reason) = m.reject.get(&id) {
                    rejected.push(json!({"id": id, "reason": reason}));
                } else if !m.has.insert(id.clone()) {
                    duplicates.push(id);
                } else {
                    accepted.push(id);
                }
            }
            let body = json!({"accepted": accepted, "duplicates": duplicates, "rejected": rejected});
            json_reply("200 OK", &body.to_string())
        }
        _ => json_reply("404 Not Found", r#"{"error":"Not Found"}"#),
    }
}

fn server(m: Mstream) -> (Client, Fake) {
    let fake = Arc::new(Mutex::new(m));
    let served = fake.clone();
    let base = routed(move |asked| answer(&served, asked));
    (Client::new(&base).unwrap().with_token(Some("tok".into())), fake)
}

/// The ids of each post, in order.
fn posted_ids(fake: &Fake) -> Vec<Vec<String>> {
    let m = fake.lock().unwrap();
    let ids = |body: &Value| -> Vec<String> {
        let plays = body["plays"].as_array().cloned().unwrap_or_default();
        plays.iter().map(|p| p["id"].as_str().unwrap_or_default().to_string()).collect()
    };
    m.posts.iter().map(ids).collect()
}

fn id(device_id: [u8; 6], epoch: u64, seq: u32) -> String {
    Slot { device_id, epoch, seq, ..Slot::default() }.id()
}

// ── The contract's vectors ──────────────────────────────────────────────────

fn num(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().expect("an integer"),
        Value::String(s) => match s.strip_prefix("0x") {
            Some(h) => u64::from_str_radix(h, 16).expect("hex"),
            None => s.parse().expect("decimal"),
        },
        _ => 0,
    }
}

fn marks_in(v: &Value) -> Vec<Ack> {
    let device = |v: &Value| device_of(&v.as_str().unwrap().replace(':', "")).expect("a device id");
    let mark = |e: &Value| {
        Ack { device_id: device(&e["deviceId"]), epoch: num(&e["epoch"]), seq: num(&e["seq"]) as u32 }
    };
    v.as_array().into_iter().flatten().map(mark).collect()
}

fn log_of(files: &[(&str, Vec<u8>)]) -> Log {
    let files: Vec<LogFile<'_>> = files.iter().map(|(n, b)| LogFile::whole(n, b)).collect();
    mppl::read_log(&files)
}

/// `plays.resolve`: a golden log through the golden tags file — each new
/// play sent with its row's server path and its start, or not ('file': no
/// row, or not the row's file; 'clock': no start) — then the marks once the
/// server settled the ones sent; and the drop notes' sum.
#[test]
fn vectors_plays_resolve_through_a_golden_tags_file() {
    let v = vectors();
    let servers = msmf_root("manifest.bin").server_table();
    for case in v["plays"]["resolve"].as_array().expect("resolve") {
        let log = log_of(&golden_log(case["log"].as_str().unwrap()));
        let ledger = fixture(&format!("golden/{}", case["ledger"].as_str().unwrap()));
        let tags = mptg::read(&ledger, mptg::Uses::ALL).expect("the golden tags file");
        let posted = marks_in(&case["posted"]);
        let skew = num(&case["skew"]) as i32;
        let resolved = resolve(&log, &tags, &servers, skew, &posted);
        let want = case["plays"].as_array().expect("plays");
        assert_eq!(resolved.len(), want.len(), "{}", case["log"]);
        let mut not_sent = HashSet::new();
        for ((slot, fate), w) in resolved.iter().zip(want) {
            assert_eq!(slot.id(), w["id"].as_str().unwrap());
            match (fate, w["send"].as_bool().unwrap()) {
                (Ok(owed), true) => {
                    assert_eq!(owed.file_path, w["filePath"].as_str().unwrap(), "{}", w["id"]);
                    assert_eq!(owed.started_at, num(&w["startedAt"]), "{}", w["id"]);
                    assert_eq!(owed.to, To::of(&servers, 0).unwrap());
                }
                (Err(why), false) => {
                    let word = if *why == Unsent::Clock { "clock" } else { "file" };
                    assert!(matches!(why, Unsent::Own | Unsent::Changed | Unsent::Clock));
                    assert_eq!(word, w["why"].as_str().unwrap(), "{}: {}", w["id"], w["note"]);
                    not_sent.insert(slot.id());
                }
                (fate, send) => panic!("{}: {fate:?}, wanted sent {send}", w["id"]),
            }
        }
        let settled = case["settled"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string());
        let settled: HashSet<String> = settled.collect();
        let done = |p: &Slot| not_sent.contains(&p.id()) || settled.contains(&p.id());
        assert_eq!(mppl::next_marks(&log.plays, &posted, &done), marks_in(&case["marks"]));
        assert_eq!(log.counts.dropped, num(&case["dropped"]), "{}", case["log"]);
    }

    // `resolveCases`: single plays of one row, each held to the row.
    let base = &v["plays"]["resolveCasesBase"];
    let tags = mptg::read(&fixture("golden/tags-0000002a.bin"), mptg::Uses::ALL).unwrap();
    let path = base["path"].as_str().unwrap();
    let row = tags.records.iter().find(|e| e.path == path).unwrap();
    for c in v["plays"]["resolveCases"].as_array().unwrap() {
        let s = &c["slot"];
        let field = |key: &str, or: u64| s.get(key).map_or(or, num);
        let slot = Slot {
            kind: 1,
            outcome: 2,
            source: 1,
            seq: field("seq", num(&base["seq"])) as u32,
            clock: field("clock", 2) as u8,
            epoch: num(&base["epoch"]),
            device_id: device_of(&base["deviceId"].as_str().unwrap().replace(':', "")).unwrap(),
            started_at: field("startedAt", 1_791_900_190_000),
            boot_seq: 1,
            uptime_ms: 190_000,
            played_ms: 12_000,
            path_hash: path_hash(path),
            size: field("size", row.rec.size as u64) as u32,
            fat_time: field("fatTime", row.rec.fat_time as u64) as u32,
            generation: 42,
            ..Slot::default()
        };
        let log = Log { plays: vec![slot], counts: mppl::Counts::default() };
        let posted = c.get("posted").map(|p| ack(slot.device_id, slot.epoch, num(p) as u32));
        let posted: Vec<Ack> = posted.into_iter().collect();
        let resolved = resolve(&log, &tags, &servers, num(&c["skew"]) as i32, &posted);
        match c["send"].as_bool() {
            None => assert!(resolved.is_empty(), "{}", c["note"]),
            Some(true) => assert!(resolved[0].1.is_ok(), "{}", c["note"]),
            Some(false) => {
                let why = resolved[0].1.as_ref().unwrap_err();
                let word = if *why == Unsent::Clock { "clock" } else { "file" };
                assert_eq!(word, c["why"].as_str().unwrap(), "{}", c["note"]);
            }
        }
    }
}

fn msmf_root(name: &str) -> msmf::Manifest {
    msmf::Manifest::read(&fixture(&format!("golden/{name}")), msmf::Uses::ALL).expect("a golden root")
}

/// `plays.peers`: on the card of several servers, a play of each row goes
/// to its row's server, and a peer's to the peer's parent with its id.
#[test]
fn vectors_plays_peers_go_to_their_parent() {
    let v = vectors();
    let servers = msmf_root("manifest-servers.bin").server_table();
    let tags = mptg::read(&fixture("golden/tags-00000007.bin"), mptg::Uses::ALL).unwrap();
    for row in v["plays"]["peers"]["rows"].as_array().unwrap() {
        let path = row["path"].as_str().unwrap();
        let entry = tags.records.iter().find(|e| e.path == path).unwrap();
        let index = entry.ledger.as_ref().unwrap().server as usize;
        assert_eq!(index as u64, num(&row["server"]), "{path}");
        let to = To::of(&servers, index).expect("a server the card tells");
        let parent = servers.get(num(&row["to"]["server"]) as usize).unwrap();
        assert_eq!(to.server, Server::direct(parent.instance, parent.url_key), "{path}");
        assert_eq!(to.peer.map(u64::from), row["to"]["peerId"].as_u64(), "{path}");
        assert!(!to.server.is_peer());
    }
}

// ── A look and a post ───────────────────────────────────────────────────────

/// The golden first sync's log: seven plays, five of them sent (the first
/// dated by the second), the listener's file and a file whose row's stamp
/// is invalid counted; the server's answers settle them, and the mark rises
/// to the last. The post says who sent it, and each play as the device
/// knew it.
#[test]
fn the_new_plays_go_to_their_server_and_the_mark_follows_the_answers() {
    let scratch = Scratch::new("first");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let store = Rc::new(MemStore::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    let t = &history.tally;
    assert_eq!((t.new, t.owed, t.own, t.changed, t.clock, t.server), (7, 5, 1, 1, 0, 0), "{t:?}");
    let saved = store.outbox();
    let owing = &saved.cards[&CARD_ID];
    assert_eq!(owing.plays.len(), 5, "the mapping saved before anything is posted");
    assert_eq!(owing.firmware, "0.12.0");
    assert_eq!(history.marks(), [], "the first play waits");

    let (client, fake) = server(Mstream::default());
    history.post(&client, &talk(HOME), true, true);
    let t = &history.tally;
    assert_eq!((t.accepted, t.duplicates, t.rejected, t.due, t.waiting, t.failed), (5, 0, 0, 0, 0, false));
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 7)]);
    assert_eq!(history.acks(&[]), [ack(ONE, EPOCH_1, 7)]);
    let owing = &store.outbox().cards[&CARD_ID];
    assert!(owing.plays.is_empty());
    assert_eq!(owing.done.len(), 5, "settled, until a sync stores marks past them on the card");

    let body = fake.lock().unwrap().posts[0].clone();
    let client = json!({"name": "mstream-core2", "version": "0.12.0", "instanceId": "5eedc0de0a1b2c3d"});
    assert_eq!(body["client"], client);
    let plays = body["plays"].as_array().unwrap();
    let first = &plays[0];
    assert_eq!(first["id"], "core2-00005e005301-1122334455667788-1");
    assert_eq!(first["filePath"], "Music/Artist/Album/01 - Title.mp3");
    let (started, outcome) = (first["startedAt"].as_u64(), first["outcome"].as_str());
    assert_eq!((started, outcome), (Some(1_791_900_005_000), Some("completed")));
    assert!(first.get("peerId").is_none() && first.get("track").is_none());
    let third = &plays[2];
    assert_eq!((third["outcome"].as_str(), third["source"].as_str()), (Some("stopped"), Some("autodj")));
    assert_eq!((third["pauseCount"].as_u64(), third["playedMs"].as_u64()), (Some(2), Some(90_500)));
    assert_eq!(plays[4]["filePath"], "Music/Loose Intro.flac", "a converted file: its row's server path");

    // The next look, with no state file's mark yet: the plays are new
    // again, and settled already; nothing goes twice.
    let mut again = History::new(store.clone(), false, none());
    again.look(&sim, Some(&root), 0);
    assert_eq!((again.tally.new, again.tally.owed, again.due(&talk(HOME))), (7, 5, 0));
    assert_eq!(again.marks(), [ack(ONE, EPOCH_1, 7)]);
    // Once a sync has stored the mark on the card, the outbox forgets them.
    let mut stored = History::new(store.clone(), false, (CARD_ID, vec![ack(ONE, EPOCH_1, 7)]));
    stored.look(&sim, Some(&root), 0);
    assert_eq!(stored.tally.new, 0);
    assert!(store.outbox().cards.is_empty());
}

/// A crash after the server took a batch, before the outbox said so: the
/// next run posts the same plays again, the server answers duplicates, and
/// they settle; nothing is credited twice and the mark comes out the same.
#[test]
fn a_crash_between_a_post_and_its_mark_posts_again_as_duplicates() {
    let scratch = Scratch::new("crash");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    // One save: the look's. The post's answer is never saved.
    let store = Rc::new(MemStore { saves_left: Cell::new(Some(1)), ..MemStore::default() });
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    history.post(&client, &talk(HOME), true, true);
    assert_eq!(history.tally.accepted, 5);
    assert!(history.tally.failed, "the outbox wasn't saved: {:?}", history.tally.notes);
    let on_disk = store.outbox();
    assert_eq!((on_disk.cards[&CARD_ID].plays.len(), on_disk.cards[&CARD_ID].done.len()), (5, 0));

    store.saves_left.set(None);
    let mut after = History::new(store.clone(), false, none());
    after.look(&sim, Some(&root), 0);
    assert_eq!(after.due(&talk(HOME)), 5, "still owed, as the disk says");
    after.post(&client, &talk(HOME), true, true);
    let t = &after.tally;
    assert_eq!((t.accepted, t.duplicates, t.failed), (0, 5, false));
    assert_eq!(after.marks(), [ack(ONE, EPOCH_1, 7)]);
    let ids = posted_ids(&fake);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], ids[1], "the same plays, the same ids");
}

/// A post that fails (the server out of reach) keeps every play and the
/// mark where it was; a later post settles them. The server's 'rejected'
/// and 'duplicates' settle as 'accepted' does.
#[test]
fn a_post_that_fails_keeps_every_play_and_a_rejected_one_settles() {
    let scratch = Scratch::new("offline");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let store = Rc::new(MemStore::default());
    let gone = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let offline = Client::new(&gone).unwrap().with_token(Some("tok".into()));
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    history.post(&offline, &talk(HOME), true, true);
    let t = &history.tally;
    assert!(t.failed && t.notes.iter().any(|n| n.contains("weren't posted")), "{t:?}");
    assert_eq!((t.due, t.waiting), (5, 5));
    assert_eq!(history.marks(), []);
    assert_eq!(store.outbox().cards[&CARD_ID].plays.len(), 5, "the mapping kept");

    // Back online: one play the server already has, one it rejects (a
    // start past its retention), the rest new.
    let mut m = Mstream::default();
    m.has.insert(id(ONE, EPOCH_1, 2));
    m.reject.insert(id(ONE, EPOCH_1, 3), "bad-time");
    let (client, _fake) = server(m);
    let mut later = History::new(store.clone(), false, none());
    later.look(&sim, Some(&root), 0);
    later.post(&client, &talk(HOME), true, true);
    let t = &later.tally;
    assert_eq!((t.accepted, t.duplicates, t.rejected, t.waiting), (3, 1, 1, 0));
    assert_eq!(later.marks(), [ack(ONE, EPOCH_1, 7)]);
}

/// The server read now isn't the one that filled the rows: nothing goes,
/// the plays wait and hold the mark; the same server under another address
/// takes them once the listener has said it is (2.11's key rule).
#[test]
fn plays_of_another_server_wait_for_it() {
    let scratch = Scratch::new("other");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let store = Rc::new(MemStore::default());
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    history.post(&client, &talk(OTHER), true, true);
    assert!(fake.lock().unwrap().posts.is_empty());
    assert_eq!((history.tally.due, history.tally.waiting), (0, 5));
    assert_eq!(history.marks(), []);
    assert_eq!(history.acks(&[]), [], "nothing acknowledged");

    let mut confirmed = talk("http://192.0.2.10:3000");
    confirmed.confirmed = vec![server_url_key(HOME)];
    history.post(&client, &confirmed, true, true);
    assert_eq!(history.tally.accepted, 5);
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 7)]);
}

/// On a card of several servers: the server read now takes its own rows'
/// plays and those of its peers' rows, each with the peer's id and the
/// track's snapshot; the other server's play waits, and holds its device's
/// mark until a run reads that server.
#[test]
fn a_peers_play_goes_to_its_parent_and_another_servers_waits() {
    let scratch = Scratch::new("peers");
    let empty = card(&scratch, Golden::Several, &[]);
    let root = root_of(&empty);
    let mine = play_of(&root, "Artist/Album/01 - Title.mp3", ONE, EPOCH_1, 1);
    let peer = play_of(&root, "Peer Artist/Peer Album/01 - Tune.opus", ONE, EPOCH_1, 2);
    let second = play_of(&root, "Second Artist/Album/01 - Song.flac", ONE, EPOCH_1, 3);
    let far = play_of(&root, "Far Artist/Far Album/01 - Far.opus", TWO, EPOCH_1, 1);
    let log = log_bytes(&[mine, peer, second, far]);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), &log).unwrap();
    let store = Rc::new(MemStore::default());
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&empty, Some(&root), 0);
    assert_eq!(history.tally.owed, 4);
    history.post(&client, &talk(HOME), true, true);
    let ids = posted_ids(&fake);
    assert_eq!(ids, [vec![mine.id(), peer.id()]]);
    let body = fake.lock().unwrap().posts[0].clone();
    let tune = &body["plays"][1];
    assert_eq!(tune["peerId"], 3);
    let row = tags_of(&root).records.iter().find(|e| e.path.contains("Tune")).unwrap();
    assert_eq!(tune["filePath"], row.ledger.as_ref().unwrap().server_path);
    let track = &tune["track"];
    assert_eq!(track["durationMs"].as_u64(), Some(180_000), "{track}");
    assert!(body["plays"][0].get("track").is_none(), "the parent looks its own up");
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 2)], "the second server's play holds it");

    // The second server's turn: its play, and the peer of it.
    let (client, fake) = server(Mstream::default());
    history.post(&client, &talk(OTHER), true, true);
    assert_eq!(posted_ids(&fake), [vec![second.id(), far.id()]]);
    assert_eq!(fake.lock().unwrap().posts[0]["plays"][1]["peerId"], 1);
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 3), ack(TWO, EPOCH_1, 1)]);
}

/// Two devices on one card, and an epoch a flasher's erase made: each
/// device and epoch has a mark of its own, the seq starting again at 1
/// never taken for plays sent before; the rotation's drop note is
/// reported.
#[test]
fn two_devices_and_a_new_epoch_keep_marks_of_their_own() {
    let scratch = Scratch::new("epochs");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/end"));
    let root = root_of(&sim);
    let store = Rc::new(MemStore::default());
    let (client, _fake) = server(Mstream::default());
    // The card's state file says the first epoch's seven were posted.
    let mut history = History::new(store.clone(), false, (CARD_ID, vec![ack(ONE, EPOCH_1, 7)]));
    history.look(&sim, Some(&root), 0);
    assert_eq!((history.tally.new, history.tally.owed, history.tally.read.dropped), (5, 5, 4));
    history.post(&client, &talk(HOME), true, true);
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 7), ack(ONE, EPOCH_2, 9)]);
    // The root before acknowledged the first epoch: carried, and the new
    // one's added.
    assert_eq!(history.acks(&[ack(ONE, EPOCH_1, 7)]), [ack(ONE, EPOCH_1, 7), ack(ONE, EPOCH_2, 9)]);

    // A second device's plays of the same files, at the same seqs.
    let a = play_of(&root, "Artist/Album/01 - Title.mp3", TWO, EPOCH_2, 5);
    let b = play_of(&root, "Artist/Album/02 - Other.mp3", TWO, EPOCH_2, 6);
    std::fs::write(scratch.0.join("card/.player/plays.2.jnl"), log_bytes(&[a, b])).unwrap();
    let mut both = History::new(store.clone(), false, (CARD_ID, vec![ack(ONE, EPOCH_1, 7)]));
    both.look(&sim, Some(&root), 0);
    assert_eq!((both.tally.new, both.due(&talk(HOME))), (7, 2), "the first device's settled already");
    both.post(&client, &talk(HOME), true, true);
    assert_eq!(both.tally.accepted, 2, "another device's ids: new to the server");
    assert_eq!(both.marks(), [ack(ONE, EPOCH_1, 7), ack(ONE, EPOCH_2, 9), ack(TWO, EPOCH_2, 6)]);
}

/// Plays from before the clock was ever set: one dated by a later play of
/// its boot goes; one with no later dated play in its boot is counted and
/// not sent, and the mark passes it.
#[test]
fn plays_from_before_the_clock_was_set_are_counted_not_sent() {
    let scratch = Scratch::new("clock");
    let empty = card(&scratch, Golden::One, &[]);
    let root = root_of(&empty);
    let mut early = play_of(&root, "Artist/Album/01 - Title.mp3", ONE, EPOCH_1, 1);
    (early.started_at, early.clock, early.uptime_ms) = (0, 0, 0xFFFF_F000);
    let mut dated = play_of(&root, "Artist/Album/02 - Other.mp3", ONE, EPOCH_1, 2);
    (dated.started_at, dated.uptime_ms) = (1_792_100_000_000, 0x1000);
    let mut lost = play_of(&root, "A/x.mp3", ONE, EPOCH_1, 3);
    (lost.started_at, lost.clock, lost.boot_seq) = (0, 0, 2);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log_bytes(&[early, dated, lost])).unwrap();
    let store = Rc::new(MemStore::default());
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&empty, Some(&root), 0);
    assert_eq!((history.tally.owed, history.tally.clock), (2, 1));
    history.post(&client, &talk(HOME), true, true);
    assert_eq!(posted_ids(&fake), [vec![early.id(), dated.id()]]);
    let started = fake.lock().unwrap().posts[0]["plays"][0]["startedAt"].as_u64();
    assert_eq!(started, Some(1_792_100_000_000 - 8_192), "dated across the uptime's wrap");
    assert_eq!(history.marks(), [ack(ONE, EPOCH_1, 3)], "the undated play settles as it is");
}

/// Lost state: with the card's state file and this computer's outbox both
/// gone, every play is new again; posted again, they come back as
/// duplicates and the marks come out the same.
#[test]
fn a_lost_state_posts_again_and_comes_out_the_same() {
    let scratch = Scratch::new("lost");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(Rc::new(MemStore::default()), false, none());
    history.look(&sim, Some(&root), 0);
    history.post(&client, &talk(HOME), true, true);
    let marks = history.marks();

    let mut again = History::new(Rc::new(MemStore::default()), false, none());
    again.look(&sim, Some(&root), 0);
    assert_eq!(again.tally.new, 7);
    again.post(&client, &talk(HOME), true, true);
    assert_eq!((again.tally.accepted, again.tally.duplicates), (0, 5));
    assert_eq!(again.marks(), marks);
    assert_eq!(fake.lock().unwrap().has.len(), 5, "each play credited once");

    // A state file of another identity (a cut set-up's, a copy's): its
    // marks say nothing of this card's log.
    let mut foreign = History::new(Rc::new(MemStore::default()), false, (7, vec![ack(ONE, EPOCH_1, 7)]));
    foreign.look(&sim, Some(&root), 0);
    assert_eq!(foreign.tally.new, 7);
}

/// A card copied with its id, played on apart: each copy's plays are its
/// own, and the plays the server settled of one never pass for the
/// other's, whichever was read first.
#[test]
fn a_card_copied_with_its_id_never_takes_the_others_plays_for_settled() {
    let scratch = Scratch::new("copies");
    let first = card(&scratch, Golden::One, &[]);
    let root = root_of(&first);
    let later = [play_of(&root, "A/x.mp3", ONE, EPOCH_1, 3), play_of(&root, "A B/y.mp3", ONE, EPOCH_1, 4)];
    let earlier = [play_of(&root, "A/x.mp3", ONE, EPOCH_1, 1), play_of(&root, "A B/y.mp3", ONE, EPOCH_1, 2)];
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log_bytes(&later)).unwrap();
    let copy = Scratch::new("copy");
    let second = card(&copy, Golden::One, &[]);
    std::fs::write(copy.0.join("card/.player/plays.jnl"), log_bytes(&earlier)).unwrap();
    let store = Rc::new(MemStore::default());
    let (client, fake) = server(Mstream::default());
    let mut one = History::new(store.clone(), false, none());
    one.look(&first, Some(&root), 0);
    one.post(&client, &talk(HOME), true, true);
    assert_eq!(one.marks(), [ack(ONE, EPOCH_1, 4)]);
    let mut two = History::new(store.clone(), false, none());
    two.look(&second, Some(&root_of(&second)), 0);
    two.post(&client, &talk(HOME), true, true);
    assert_eq!((two.tally.new, two.tally.accepted), (2, 2));
    assert_eq!(two.marks(), [ack(ONE, EPOCH_1, 2)]);
    assert_eq!(fake.lock().unwrap().has.len(), 4);
}

/// A play whose source the device didn't say goes as `device` to a server
/// of the card transfer's batch; an older one refuses the batch whole, and
/// it goes again as `other`.
#[test]
fn a_source_not_said_is_device_where_the_server_takes_it() {
    let scratch = Scratch::new("source");
    let empty = card(&scratch, Golden::One, &[]);
    let root = root_of(&empty);
    let mut quiet = play_of(&root, "Artist/Album/01 - Title.mp3", ONE, EPOCH_1, 1);
    quiet.source = 9;
    let shuffled = Slot { source: 2, ..play_of(&root, "A/x.mp3", ONE, EPOCH_1, 2) };
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log_bytes(&[quiet, shuffled])).unwrap();
    let sources = |fake: &Fake| -> Vec<Vec<String>> {
        let posts = fake.lock().unwrap().posts.clone();
        let word = |p: &Value| p["source"].as_str().unwrap().to_string();
        let words = |b: &Value| b["plays"].as_array().unwrap().iter().map(word).collect();
        posts.iter().map(words).collect()
    };
    let (client, fake) = server(Mstream::default());
    let mut history = History::new(Rc::new(MemStore::default()), false, none());
    history.look(&empty, Some(&root), 0);
    history.post(&client, &talk(HOME), true, true);
    assert_eq!(sources(&fake), [vec!["device", "shuffle"]]);
    assert_eq!(history.tally.accepted, 2);

    let (client, fake) = server(Mstream { old: true, ..Mstream::default() });
    let mut older = History::new(Rc::new(MemStore::default()), false, none());
    older.look(&empty, Some(&root), 0);
    older.post(&client, &talk(HOME), true, true);
    assert_eq!(sources(&fake), [vec!["device", "shuffle"], vec!["other", "shuffle"]]);
    assert_eq!((older.tally.accepted, older.tally.failed), (2, false));

    // A server with no Stats API: nothing posted, the plays wait.
    let (client, fake) = server(Mstream { no_stats: true, ..Mstream::default() });
    let mut none = History::new(Rc::new(MemStore::default()), false, none());
    none.look(&empty, Some(&root), 0);
    none.post(&client, &talk(HOME), false, false);
    assert!(fake.lock().unwrap().posts.is_empty());
    assert_eq!((none.tally.waiting, none.marks()), (2, vec![]));
}

/// A path the server moved since the card's ledger was written is found
/// again by its audio hash in a whole listing, when one entry has it; a
/// path still listed, one of a library the listing left out, and an audio
/// hash two entries share stay as they are.
#[test]
fn a_path_the_server_moved_is_found_again_by_its_audio_hash() {
    let scratch = Scratch::new("moved");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let mut history = History::new(Rc::new(MemStore::default()), false, none());
    history.look(&sim, Some(&root), 0);
    let owed: Vec<Owed> = history.owing.plays.clone();
    let audio = |i: usize| hex(&owed[i].audio_hash);
    let entry = |path: &str, audio: String| ManifestEntry {
        filepath: path.into(),
        id: 1,
        metadata: Default::default(),
        file_size: None,
        modified: None,
        hash: None,
        audio_hash: Some(audio),
        hash_v: None,
        format: None,
        album_id: None,
        artist_id: None,
        created_at: None,
    };
    let entries = vec![
        entry("Music/Moved/01 - Title.mp3", audio(0)),
        entry(&owed[1].file_path, audio(1)),
        entry("Music/One/x.flac", audio(2)),
        entry("Music/Two/x.flac", audio(2)),
    ];
    history.remap(&talk(HOME), &entries, Some(&["Music".to_string()]));
    let paths: Vec<&str> = history.owing.plays.iter().map(|o| o.file_path.as_str()).collect();
    assert_eq!(paths[0], "Music/Moved/01 - Title.mp3");
    assert_eq!(&paths[1..3], [owed[1].file_path.as_str(), owed[2].file_path.as_str()]);
    assert_eq!(history.tally.remapped, 1);
    // Another server's listing moves nothing; a listing of other libraries
    // either.
    let mut other = History::new(Rc::new(MemStore::default()), false, none());
    other.look(&sim, Some(&root), 0);
    other.remap(&talk(OTHER), &entries, None);
    other.remap(&talk(HOME), &entries, Some(&["Jazz".to_string()]));
    assert_eq!(other.tally.remapped, 0);
}

/// A dry run maps and counts, and saves and posts nothing; a card with no
/// log, or with a log it can't read whole, has nothing mapped.
#[test]
fn a_dry_run_saves_nothing_and_a_short_read_maps_nothing() {
    let scratch = Scratch::new("dry");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let root = root_of(&sim);
    let store = Rc::new(MemStore::default());
    let (client, fake) = server(Mstream::default());
    let mut dry = History::new(store.clone(), true, none());
    dry.look(&sim, Some(&root), 0);
    dry.post(&client, &talk(HOME), true, true);
    assert_eq!((dry.tally.new, dry.tally.due), (7, 5));
    assert!(store.text.borrow().is_none() && fake.lock().unwrap().posts.is_empty());

    let bare = Scratch::new("bare");
    let no_log = card(&bare, Golden::One, &[]);
    let mut history = History::new(store.clone(), false, none());
    history.look(&no_log, Some(&root_of(&no_log)), 0);
    assert_eq!((history.tally.new, history.log.is_none()), (0, true));
    assert_eq!(history.acks(&[ack(ONE, EPOCH_1, 3)]), [ack(ONE, EPOCH_1, 3)], "carried as they are");
    assert!(store.text.borrow().is_none());
}

/// The outbox reads back what it wrote, keeps what a newer player wrote
/// beside it, and is never written over by this one when it is a newer
/// player's.
#[test]
fn the_outbox_reads_back_and_keeps_what_it_doesnt_know() {
    let scratch = Scratch::new("outbox");
    let sim = card(&scratch, Golden::Several, &[]);
    let root = root_of(&sim);
    let peer = play_of(&root, "Peer Artist/Peer Album/01 - Tune.opus", ONE, EPOCH_1, 2);
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log_bytes(&[peer])).unwrap();
    let store = Rc::new(MemStore::default());
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    let text = store.text.borrow().clone().unwrap();
    let outbox = Outbox::parse(&text);
    assert_eq!(outbox.cards[&CARD_ID].plays, history.owing.plays);
    assert_eq!(outbox.text(), text);
    let o = &outbox.cards[&CARD_ID].plays[0];
    assert_eq!((o.to.peer, o.track.is_some()), (Some(3), true));

    let mut v: Value = serde_json::from_str(&text).unwrap();
    v["sets"] = json!({"a": 1});
    let card = id_hex(CARD_ID);
    v["cards"][&card]["queue"] = json!([1]);
    v["cards"][&card]["plays"].as_array_mut().unwrap().push(json!({"id": "a newer one's"}));
    let kept = Outbox::parse(&v.to_string());
    let again: Value = serde_json::from_str(&kept.text()).unwrap();
    assert_eq!((&again["sets"], &again["cards"][&card]["queue"]), (&json!({"a": 1}), &json!([1])));
    assert_eq!(again["cards"][&card]["plays"].as_array().unwrap().len(), 2);

    let newer = r#"{"version": 2, "cards": {}}"#;
    *store.text.borrow_mut() = Some(newer.into());
    let mut history = History::new(store.clone(), false, none());
    history.look(&sim, Some(&root), 0);
    assert!(history.tally.failed && history.tally.notes.iter().any(|n| n.contains("newer")));
    assert_eq!(store.text.borrow().as_deref(), Some(newer), "never written over");
    assert_eq!(key_of_id("core2-00005e005301-1122334455667788-7"), Some((ONE, EPOCH_1, 7)));
    for bad in ["core2-00005E005301-1122334455667788-7", "core2-00005e005301-1122334455667788-07", "x-1"] {
        assert_eq!(key_of_id(bad), None, "{bad}");
    }
}

/// The hidden command: a volume the rule refuses is never read; one it
/// accepts is read, never written — the plays posted, this computer's
/// outbox holding the marks, the card's ACKS and state file left to the
/// next sync's commit. A dry run saves and posts nothing.
#[test]
fn the_hidden_command_reads_only_a_volume_the_rule_accepts_and_never_writes_it() {
    use crate::device::card::volume::{Bus, Clock, Disk, Found, Fs, Serial, Table, Volume};
    let scratch = Scratch::new("command");
    let sim = card(&scratch, Golden::One, &golden_log("plays/golden/sync-1"));
    let (client, fake) = server(Mstream::default());
    // The card's settings say the stub's address is the golden root's
    // server under another address.
    let mine = server_url_key(&client.server());
    let state = json!({"format": "mstream-card-state", "version": 1, "seq": 2, "cardId": id_hex(CARD_ID),
        "label": "Car", "plays": [{"deviceId": "00005e005301", "epoch": id_hex(EPOCH_1), "seq": 1}],
        "sources": [{"serverUrlKey": id_hex(mine), "confirmed": [id_hex(server_url_key(HOME))]}]});
    std::fs::write(scratch.0.join("card/.mstream/state.json"), state.to_string()).unwrap();
    let image = sim.image();
    let at = |device: &str, fs: Fs| {
        let volume = Volume {
            path: scratch.0.join("card"),
            device: device.into(),
            fs,
            table: Table::Mbr,
            label: "CARD".into(),
            serial: Some(Serial::Fat(SERIAL)),
            space: None,
            read_only: false,
            utf8: Some(true),
            clock: Clock::Offset(0),
        };
        let (system, others) = (Vec::new(), Vec::new());
        let (id, bus) = ("sdz".to_string(), Bus::Usb);
        let disk = Disk { id, removable: true, bus, read_only: false, system, others };
        Found::new(volume, disk)
    };
    let found = [at("sdy1", Fs::ExFat), at("sdz1", Fs::Fat32)];
    let store = Rc::new(MemStore::default());
    let base = client.server();
    let asked = Cell::new(0);
    let call = |named: &str, dry_run: bool| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut saved = || {
            asked.set(asked.get() + 1);
            Client::new(&base).map(|c| c.with_token(Some("tok".into()))).map_err(|e| e.to_string())
        };
        let code = on(&found, named, dry_run, &mut saved, store.clone(), &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    };
    for dry_run in [true, false] {
        let (code, out, err) = call("sdy1", dry_run);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("is refused: exfat"), "{err}");
        let (code, _, err) = call("not-a-volume", dry_run);
        assert!(code == 2 && err.contains("isn't a volume"), "{err}");
    }
    assert_eq!(asked.get(), 0, "the session is asked for only once a volume is taken");

    let (code, out, err) = call("sdz1", true);
    assert_eq!(code, 0, "{err}");
    let printed: Value = serde_json::from_str(&out).unwrap();
    let said = (printed["dryRun"].as_bool(), printed["new"].as_u64(), printed["due"].as_u64());
    assert_eq!(said, (Some(true), Some(6), Some(4)));
    assert!(store.text.borrow().is_none() && fake.lock().unwrap().posts.is_empty());

    let (code, out, err) = call("sdz1", false);
    assert_eq!(code, 0, "{err}");
    let printed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(printed["posted"]["accepted"].as_u64(), Some(4), "{printed}");
    assert_eq!(printed["marks"], json!([{"deviceId": "00005e005301", "epoch": id_hex(EPOCH_1), "seq": 7}]));
    assert!(printed["acks"].as_str().unwrap().contains("writes nothing"));
    assert_eq!(posted_ids(&fake)[0][0], id(ONE, EPOCH_1, 2), "the state file's mark: play 1 posted before");
    assert_eq!(sim.image(), image, "the card, read and never written");
    assert_eq!(store.outbox().cards[&CARD_ID].done.len(), 4, "until a sync stores the marks");
}
