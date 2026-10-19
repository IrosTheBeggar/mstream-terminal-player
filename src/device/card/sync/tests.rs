//! A whole sync, end to end: a loopback mStream of made-up libraries
//! ([`Fake`]: the paged manifest with its ETag and 304, `metadata/batch`,
//! `/media` with Range and HEAD, `/transcode`, album art and embeddings),
//! a host folder as the card ([`FatSim`]), set up by B2's set-up, and the
//! card read back by the kit's reader. The audio is the tag reader's parity
//! corpus and made-up Ogg Opus streams; the covers are made-up pictures;
//! the embeddings are made-up vectors (no real index: AUTODJ-INDEX.md 12.5).
//! The server's federation peers, and their own loopback servers, are
//! [`peers`]'s.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Value, json};

use super::*;
use crate::api::transfer::stub::{Asked, chunked, json as json_reply, reply, routed};
use crate::device::card::cards::Cards;
use crate::device::card::contract::mptg::{BPM_ANALYSED, CONTAINER_MP3, CONTAINER_OPUS};
use crate::device::card::contract::mpth;
use crate::device::card::contract::order::thumb_path_of;
use crate::device::card::contract::prim::{hex, md5};
use crate::device::card::covers::PROBES;
use crate::device::card::contract::mppl::{KIND_PLAY, Slot};
use crate::device::card::covers::tests::picture;
use crate::device::card::engine::Removal;
use crate::device::card::engine::fatsim::FatSim;
use crate::device::card::history::tests::MemStore;
use crate::device::card::setup;
use crate::device::card::source::length::tests::{mp3_bytes, opus_bytes};
use crate::device::card::volume::Inspection;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A folder of its own under the system's temp folder, gone when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mstream-sync-{}-{name}-{n}", std::process::id()));
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

fn corpus(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/tags").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

// ── The server ──────────────────────────────────────────────────────────────

/// A track of the fake server.
#[derive(Clone)]
struct Track {
    path: String,
    id: i64,
    bytes: Vec<u8>,
    seconds: f64,
    art: Option<String>,
    /// The full metadata's own fields: sample rate, channels, BPM.
    full: Value,
}

#[derive(Default)]
struct Library {
    vpaths: Vec<String>,
    tracks: Vec<Track>,
    revision: String,
    /// Folder images, by path: served, never listed.
    images: HashMap<String, Vec<u8>>,
    art: HashMap<String, Vec<u8>>,
    /// A conversion's bytes, by its source's path.
    converted: HashMap<String, Vec<u8>>,
    /// Conversions that end short once, as a dying ffmpeg leaves them.
    short_once: HashSet<String>,
    /// Files whose `/media` GET answers 401: the sign-in revoked mid-run.
    refused: HashSet<String>,
    /// Rock's folder on this computer: the session is an admin's, which
    /// may ask `admin/directories`.
    disk: Option<PathBuf>,
    /// Every request, as "METHOD path".
    asked: Vec<String>,
    /// The plays posted to `stats/plays`, as they came, and the ids it has.
    plays: Vec<Value>,
    has: HashSet<String>,
    /// `stats/plays` answers 500: a post that fails.
    stats_down: bool,
    /// `/api/`'s transcode block, when not an older server's (no rates
    /// listed, no bulk queue).
    transcode: Option<Value>,
    /// Each `/transcode` request's query, as it came.
    transcodes: Vec<String>,
    /// Older than the card transfer's batch of mStream changes: no
    /// `instanceId` in `/api/`.
    old: bool,
    /// Its `instanceId`, when not [`ID`].
    instance: Option<&'static str>,
    /// Its federation peers (B9).
    peers: Vec<peers::FedPeer>,
}

/// The fake server's own id.
const ID: &str = "3f2b8c1e-6a4d-4e0f-9b7a-2c5d8e1f0a93";

type Fake = Arc<Mutex<Library>>;

fn etag(bytes: &[u8]) -> String {
    format!("W/\"{:x}-{:08x}\"", bytes.len(), crate::device::card::contract::prim::crc32(bytes))
}

fn entry_json(t: &Track) -> Value {
    json!({
        "filepath": t.path, "id": t.id, "file-size": t.bytes.len(), "modified": 1_790_000_000_000i64 + t.id,
        "hash": hex(&md5(&t.bytes)), "audio-hash": hex(&md5(&[b"audio:".as_slice(), &t.bytes].concat())),
        "hash-v": 2, "format": t.path.rsplit_once('.').map(|(_, e)| e).unwrap_or(""),
        "album-id": 1, "artist-id": 1, "created-at": "2026-10-01 12:00:00",
        "metadata": {"title": t.path, "duration": t.seconds, "album-art": t.art, "bpm": null},
    })
}

/// A made-up embedding: 8 floats from the path.
fn vector(path: &str) -> String {
    let h = crate::device::card::contract::prim::fnv1a64(path.as_bytes());
    let floats: Vec<u8> =
        (0..8).flat_map(|i| (((h >> (i * 8)) & 0xFF) as f32 / 255.0 - 0.5).to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(floats)
}

/// The bytes of `bytes` a Range header asks for, as `/media` answers it.
fn media(asked: &Asked, bytes: &[u8]) -> Vec<u8> {
    let tag = etag(bytes);
    if asked.method == "HEAD" {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: {tag}\r\n", bytes.len());
        return format!("{head}Connection: close\r\n\r\n").into_bytes();
    }
    let range = asked.header("range").and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-'));
    match range {
        Some((a, b)) => {
            let start: usize = a.parse().unwrap_or(0);
            let end = b.parse::<usize>().map_or(bytes.len(), |e| (e + 1).min(bytes.len()));
            if start >= bytes.len() {
                let total = format!("bytes */{}", bytes.len());
                return reply("416 Range Not Satisfiable", &[("Content-Range", &total)], b"");
            }
            let span = format!("bytes {start}-{}/{}", end - 1, bytes.len());
            reply("206 Partial Content", &[("Content-Range", &span), ("ETag", &tag)], &bytes[start..end])
        }
        None => reply("200 OK", &[("ETag", &tag), ("Content-Type", "audio/mpeg")], bytes),
    }
}

fn answer(fake: &Fake, asked: &Asked) -> Vec<u8> {
    let mut lib = fake.lock().unwrap();
    let path = asked.path();
    lib.asked.push(format!("{} {path}", asked.method));
    let not_found = || json_reply("404 Not Found", r#"{"error":"Not Found"}"#);
    match (asked.method.as_str(), path.as_str()) {
        ("GET", "/api/") => {
            let mut body = json!({
                "server": "6.31.1",
                "features": {"sync": true, "discovery": true, "stats": 2,
                             "transcode": lib.transcode.clone().unwrap_or_else(
                                 || json!({"defaultCodec": "opus", "defaultBitrate": "96k"}))},
                "user": {"vpaths": lib.vpaths, "admin": false},
            });
            if !lib.old {
                body["instanceId"] = json!(lib.instance.unwrap_or(ID));
            }
            if lib.disk.is_some() {
                body["user"]["admin"] = json!(true);
                body["admin"] = json!({"platform": if cfg!(windows) { "win32" } else { "linux" }});
            }
            json_reply("200 OK", &body.to_string())
        }
        ("GET", "/api/v1/admin/directories") => match &lib.disk {
            Some(dir) => {
                let body = json!({"Rock": {"id": 1, "root": dir.to_string_lossy(), "followSymlinks": false}});
                json_reply("200 OK", &body.to_string())
            }
            None => json_reply("403 Forbidden", r#"{"error":"Access Denied"}"#),
        },
        ("POST", "/api/v1/sync/manifest") => {
            let body = asked.json();
            let quoted = format!("\"{}\"", lib.revision);
            if body.get("cursor").is_none() && asked.header("if-none-match") == Some(quoted.as_str()) {
                return reply("304 Not Modified", &[], b"");
            }
            let ignore: Vec<String> = serde_json::from_value(body["ignoreVPaths"].clone()).unwrap_or_default();
            let cursor = body["cursor"].as_i64().unwrap_or(0);
            let limit = body["limit"].as_u64().unwrap_or(2000) as usize;
            let mut tracks: Vec<&Track> = lib
                .tracks
                .iter()
                .filter(|t| !ignore.iter().any(|v| t.path.starts_with(&format!("{v}/"))) && t.id > cursor)
                .collect();
            tracks.sort_by_key(|t| t.id);
            let page: Vec<&Track> = tracks.iter().take(limit).copied().collect();
            let next = (tracks.len() > limit).then(|| page.last().map(|t| t.id)).flatten();
            let body = json!({"revision": lib.revision, "scanning": false, "next": next,
                              "entries": page.iter().map(|t| entry_json(t)).collect::<Vec<_>>()});
            let headers = [("Content-Type", "application/json"), ("ETag", quoted.as_str())];
            reply("200 OK", &headers, body.to_string().as_bytes())
        }
        ("POST", "/api/v1/db/metadata/batch") => {
            let paths: Vec<String> = serde_json::from_value(asked.json()).unwrap_or_default();
            let mut out = serde_json::Map::new();
            for p in paths {
                let row = lib.tracks.iter().find(|t| t.path == p).map(|t| {
                    let mut m = t.full.clone();
                    m["duration"] = json!(t.seconds);
                    m["album-art"] = json!(t.art);
                    json!({"filepath": p, "metadata": m})
                });
                out.insert(p, row.unwrap_or(Value::Null));
            }
            json_reply("200 OK", &Value::Object(out).to_string())
        }
        ("POST", "/api/v1/discovery/local/embeddings") => {
            let paths: Vec<String> = serde_json::from_value(asked.json()["filePaths"].clone()).unwrap_or_default();
            let tracks: Vec<Value> = paths
                .iter()
                .map(|p| json!({"filePath": p, "audioHash": null, "notAnalyzed": false, "embedding": vector(p)}))
                .collect();
            let body = json!({"model": {"id": "effnet-discogs", "version": "1"}, "dim": 8, "tracks": tracks});
            json_reply("200 OK", &body.to_string())
        }
        ("GET" | "HEAD", p) if p.starts_with("/media/") => {
            let rel = &p["/media/".len()..];
            if asked.method == "GET" && lib.refused.contains(rel) {
                return json_reply("401 Unauthorized", r#"{"error":"Authentication Error"}"#);
            }
            let bytes = lib.tracks.iter().find(|t| t.path == rel).map(|t| t.bytes.clone());
            match bytes.or_else(|| lib.images.get(rel).cloned()) {
                Some(bytes) => media(asked, &bytes),
                None => not_found(),
            }
        }
        ("GET", p) if p.starts_with("/transcode/") => {
            let rel = p["/transcode/".len()..].to_string();
            lib.transcodes.push(asked.target.split_once('?').map_or("", |(_, q)| q).to_string());
            let Some(bytes) = lib.converted.get(&rel).cloned() else { return not_found() };
            let codec = asked.query("codec").unwrap_or("");
            let kind = if codec == "opus" { "audio/ogg" } else { "audio/mpeg" };
            if lib.short_once.remove(&rel) {
                return chunked("200 OK", &[("Content-Type", kind)], &[&bytes[..bytes.len() / 3]]);
            }
            chunked("200 OK", &[("Content-Type", kind)], &[&bytes])
        }
        ("POST", "/api/v1/stats/plays") if lib.stats_down => {
            json_reply("500 Internal Server Error", r#"{"error":"Server Error"}"#)
        }
        ("POST", "/api/v1/stats/plays") => {
            let plays = asked.json()["plays"].as_array().cloned().unwrap_or_default();
            let (mut accepted, mut duplicates) = (Vec::new(), Vec::new());
            for play in plays {
                let id = play["id"].as_str().unwrap_or_default().to_string();
                if lib.has.insert(id.clone()) { accepted.push(id) } else { duplicates.push(id) }
                lib.plays.push(play);
            }
            let body = json!({"accepted": accepted, "duplicates": duplicates, "rejected": []});
            json_reply("200 OK", &body.to_string())
        }
        ("GET", p) if p.starts_with("/album-art/") => match lib.art.get(&p["/album-art/".len()..]) {
            Some(bytes) => reply("200 OK", &[("Content-Type", "image/png")], bytes),
            None => not_found(),
        },
        (_, p) if p.starts_with("/api/v1/federation/") => {
            let (fed, old) = (lib.peers.clone(), lib.old);
            drop(lib);
            peers::federation(&fed, old, asked)
        }
        _ => not_found(),
    }
}

/// A server of two libraries; Rock holds an album with a folder cover and
/// an album with mStream's art whose WAV must be converted, Jazz one file.
fn server() -> (String, Fake) {
    let full = |source: &str| json!({"sample-rate": 44100, "channels": 2, "bitrate": 320000,
                                     "bpm": 124, "musical-key": "C major", "bpm-source": source});
    let track = |path: &str, id: i64, bytes: Vec<u8>, art: Option<&str>, full: Value| Track {
        path: path.into(),
        id,
        bytes,
        seconds: 181.5,
        art: art.map(str::to_string),
        full,
    };
    let wave = b"RIFF a made-up wave".to_vec();
    let mut wav = track("Rock/Artist C/Album D/01.wav", 3, wave, Some("d.png"), full("tag"));
    wav.full["bpm"] = json!(128);
    wav.full["musical-key"] = json!("A minor");
    let lib = Library {
        vpaths: vec!["Rock".into(), "Jazz".into()],
        tracks: vec![
            track("Rock/Artist A/Album B/01.mp3", 1, corpus("v24_full.mp3"), Some("b.jpg"), full("essentia")),
            track("Rock/Artist A/Album B/02.flac", 2, corpus("flac_full.flac"), Some("b.jpg"), full("tag")),
            wav,
            track("Rock/Artist C/Album D/02.opus", 4, corpus("opus_basic.opus"), Some("d.png"), full("tag")),
            track("Jazz/Artist E/Album F/01.mp3", 5, corpus("cbr.mp3"), None, full("tag")),
        ],
        revision: "1:5:1790000000005:4".into(),
        images: HashMap::from([("Rock/Artist A/Album B/cover.jpg".to_string(), picture(64, 64, true))]),
        art: HashMap::from([("d.png".to_string(), picture(120, 90, false))]),
        converted: HashMap::from([("Rock/Artist C/Album D/01.wav".to_string(), opus_bytes(181_500, 2, true))]),
        ..Library::default()
    };
    let fake = Arc::new(Mutex::new(lib));
    let served = fake.clone();
    let base = routed(move |asked| answer(&served, asked));
    (base, fake)
}

// ── The card ────────────────────────────────────────────────────────────────

const SERIAL: u32 = 0x1A2B_3C4D;

/// Each session's clock and ids: a fixed time, ids that never repeat.
fn envs() -> impl FnMut() -> Env {
    let mut seed = 0x5EED_u64;
    move || {
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut n = seed;
        let id = move || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            n
        };
        Env { now: Box::new(|| 1_791_900_000), id: Box::new(id), producer: "mstream-terminal test".into() }
    }
}

/// A card set up by the transfer, in a player that reads Opus and Auto DJ.
fn card(scratch: &Scratch) -> FatSim {
    let root = scratch.0.join("card");
    let sim = FatSim::new(&root, SERIAL);
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/golden/device.txt");
    std::fs::create_dir_all(root.join(".player")).unwrap();
    std::fs::copy(golden, root.join(".player/device.txt")).unwrap();
    let ask = setup::Ask { label: "Car".into(), new_identity: false };
    setup::set_up(&sim, &ask, &mut envs(), &mut Cards::default(), &mut |_| {}).expect("a set-up card");
    sim
}

fn space(free: u64) -> Option<Space> {
    Some(Space { size: 64 << 30, free, cluster: 32 * 1024, clusters: 0 })
}

/// One sync of `card` from the server at `base`, as the command makes one.
fn sync_now(card: &FatSim, base: &str, ask: &Ask, envs: &mut dyn FnMut() -> Env) -> (Synced, Vec<Event>) {
    sync_via(card, base, ask, envs, Reached::Elsewhere)
}

/// [`sync_now`], the server reached as `reached` says.
fn sync_via(
    card: &FatSim,
    base: &str,
    ask: &Ask,
    envs: &mut dyn FnMut() -> Env,
    reached: Reached,
) -> (Synced, Vec<Event>) {
    sync_plays(card, base, ask, envs, reached, Rc::new(MemStore::default()))
}

/// [`sync_via`], the card's plays in `plays`.
fn sync_plays(
    card: &FatSim,
    base: &str,
    ask: &Ask,
    envs: &mut dyn FnMut() -> Env,
    reached: Reached,
    plays: Rc<MemStore>,
) -> (Synced, Vec<Event>) {
    let client = Arc::new(Client::new(base).unwrap().with_token(Some("tok".into())));
    let mut events = Vec::new();
    let mut cards = Cards::default();
    let mut said = |e| {
        if !matches!(e, Event::Progress { .. }) {
            events.push(e);
        }
    };
    let (fed, space) = (no_peers(), space(32 << 30));
    let synced = sync(card, space, client, reached, ask, &mut cards, plays, &fed, envs, &mut said);
    (synced, events)
}

/// A dial that never reaches a tunnel.
struct NoDial;

impl crate::device::card::source::peer::Dial for NoDial {
    fn dial(&self, _ticket: &str) -> Result<crate::device::card::source::peer::Tunnel, String> {
        Err("no tunnel in this test".into())
    }
}

/// What a sync of no peer needs of this computer: a budget it never opens.
fn no_peers() -> Federation {
    Federation {
        budget: crate::device::card::budget::Budget::at(std::env::temp_dir().join("mstream-sync-never")),
        dial: Box::new(NoDial),
        ffmpeg: super::peers::Ffmpeg::Given(None),
        cards: Vec::new(),
        clock: Arc::new(|| 1_791_900_000),
        jitter: Arc::new(|| 0),
        stream_wait: Duration::from_millis(10),
    }
}

fn look(card: &FatSim) -> Inspection {
    let seen = volume::inspect(card).expect("the card lists");
    let root = seen.root.as_ref().expect("a root");
    assert!(root.tags.is_ok(), "the tags file: {:?}", root.tags.as_ref().err());
    assert!(root.autodj.as_ref().is_none_or(|v| v.is_ok()), "the Auto DJ index: {:?}", root.autodj);
    seen
}

fn ran(synced: &Synced) -> &Outcome {
    match &synced.end {
        End::Ran(outcome) => outcome,
        other => panic!("{other:?}\nnotes: {:?}", synced.notes),
    }
}

fn rock() -> Ask {
    Ask {
        libraries: Some(vec!["Rock".into()]),
        change: Change { nickname: Some("Home".into()), ..Change::default() },
        ..Ask::default()
    }
}

// ── The tests ───────────────────────────────────────────────────────────────

#[test]
fn a_first_sync_fills_the_card_as_the_kit_and_the_player_read_it() {
    let scratch = Scratch::new("first");
    let sim = card(&scratch);
    let (base, fake) = server();
    fake.lock().unwrap().short_once.insert("Rock/Artist C/Album D/01.wav".into());
    let mut envs = envs();
    let set_up = sim.log().len();
    let (synced, events) = sync_now(&sim, &base, &rock(), &mut envs);
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(outcome.stored, Ok(()));
    // One session, its settings stored before it let the card go: run.lock
    // taken once.
    let locks = sim.log()[set_up..].iter().filter(|op| op.starts_with("lock ")).count();
    assert_eq!(locks, 1);
    assert_eq!((outcome.written, outcome.records, outcome.deleted), (4, 4, 0), "{:?}", outcome.report);
    let summary = synced.summary.as_ref().unwrap();
    assert_eq!((summary.listed, summary.fetches, summary.conversions), (Listed::Whole, 4, 1));
    assert_eq!(summary.libraries, ["Rock"]);
    assert!(events.iter().any(|e| matches!(e, Event::Committed { last: true, .. })));

    // The kit reads it: the selection's files, the deselected library's not.
    let seen = look(&sim);
    let root = seen.root.as_ref().unwrap();
    let records = &root.tags.as_ref().unwrap().records;
    let paths: Vec<&str> = records.iter().map(|e| e.path.as_str()).collect();
    let b = ["Artist A/Album B/01.mp3", "Artist A/Album B/02.flac"];
    let d = ["Artist C/Album D/01.opus", "Artist C/Album D/02.opus"];
    assert_eq!(paths, [b, d].concat());
    assert!(root.manifest.autodj.is_some(), "the player reads MPDJ v2: an index, {:?}", synced.notes);
    assert_eq!(root.manifest.server_revision, "1:5:1790000000005:4");

    // The conversion: the ledger says so, and its own BPM, key and the
    // album's cover were put back before the card took it.
    let converted = &records[2];
    let ledger = converted.ledger.as_ref().unwrap();
    assert_eq!((ledger.converted_to, ledger.convert_kbps), (CONTAINER_OPUS, 192));
    assert_eq!(ledger.server_path, "Rock/Artist C/Album D/01.wav");
    assert_eq!((converted.rec.bpm10, converted.rec.camelot), (1280, 8), "the file's own, as tags");
    assert_eq!(converted.rec.flags & BPM_ANALYSED, 0);
    let opus = std::fs::read(scratch.0.join("card/music/Artist C/Album D/01.opus")).unwrap();
    let reading = crate::device::card::tags::read_bytes(&opus, "opus");
    let cover = reading.picture.expect("the album's cover, embedded");
    assert_eq!((cover.pic_type, cover.mime), (3, 1), "a front cover, JPEG");
    assert_eq!(converted.rec.duration_ms / 1000, 181);
    // The short conversion was made again.
    let transcodes = fake.lock().unwrap().asked.iter().filter(|a| a.starts_with("GET /transcode/")).count();
    assert_eq!(transcodes, 2);
    // mStream's analysis for a file whose tags have none.
    let mp3 = &records[0];
    assert!(mp3.rec.bpm10 != 0, "a BPM, its own or mStream's");

    // A thumbnail each album: Album B's from its folder's cover.jpg, Album
    // D's from mStream's art.
    for album in ["Artist A/Album B", "Artist C/Album D"] {
        let thumb = std::fs::read(scratch.0.join("card/.mstream").join(thumb_path_of(album))).unwrap();
        assert!(mpth::check(&thumb, crate::device::card::contract::prim::path_hash(album)), "{album}");
    }
    let asked = fake.lock().unwrap().asked.clone();
    assert!(asked.contains(&"HEAD /media/Rock/Artist A/Album B/cover.jpg".to_string()), "{asked:?}");
    assert!(asked.contains(&"GET /album-art/d.png".to_string()), "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains("Jazz/")), "the deselected library is never read");

    // The settings, stored: no address anywhere.
    let state = State::of(&seen.state).expect("a state file");
    let s = &state.settings;
    assert!(s.stored);
    assert_eq!((s.sources.len(), s.sources[0].nickname.as_str()), (1, "Home"));
    assert_eq!(s.sources[0].libraries, ["Rock"]);
    assert_eq!((s.sources[0].full_read, s.sources[0].runs), (1_791_900_000, 0));
    assert_eq!(s.art.len(), 2);
    let text = std::fs::read_to_string(scratch.0.join("card/.mstream/state.json")).unwrap();
    assert!(!text.contains("127.0.0.1") && !text.contains("http"), "{text}");
}

#[test]
fn an_unchanged_server_answers_304_and_the_card_stays_as_it_is() {
    let scratch = Scratch::new("again");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let first = sync_now(&sim, &base, &rock(), &mut envs).0;
    let generation = ran(&first).generation;
    let image = sim.image();

    // The same selection: a 304, the ledger stands for the listing, and a
    // run that changes nothing makes no commit.
    let again = sync_now(&sim, &base, &Ask { libraries: None, ..Ask::default() }, &mut envs).0;
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::NotModified);
    let outcome = ran(&again);
    assert_eq!((outcome.generation, outcome.written, outcome.deleted), (generation, 0, 0));
    let mut now = sim.image();
    now.retain(|p, _| !p.ends_with("state.json"));
    let mut before = image.clone();
    before.retain(|p, _| !p.ends_with("state.json"));
    assert_eq!(now, before, "only the settings' run count changed");
    let state = State::of(&look(&sim).state).unwrap();
    assert_eq!(state.settings.sources[0].runs, 1);

    // A file the listener deleted is written again from the ledger alone.
    std::fs::remove_file(scratch.0.join("card/music/Artist A/Album B/02.flac")).unwrap();
    let mended = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(mended.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(ran(&mended).written, 1);

    // The server drops a file: a whole read, and the card mirrors it.
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks.retain(|t| !t.path.ends_with("02.opus"));
        lib.revision = "1:4:1790000000005:3".into();
    }
    let mirrored = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let summary = mirrored.summary.as_ref().unwrap();
    assert_eq!(summary.listed, Listed::Whole);
    assert_eq!(summary.removals, ["Artist C/Album D/02.opus"]);
    let outcome = ran(&mirrored);
    assert_eq!((outcome.deleted, outcome.records, outcome.verified.clone()), (1, 3, Ok(())));
}

/// The player's plays of the card's files (2.20): the next sync posts them
/// to the server their rows came from and acknowledges them in the commit
/// it makes anyway, and its state file holds the mark; the listener's own
/// file and a play from before the clock was set are counted, not sent. A
/// sync with more plays and nothing else to do posts them and stores the
/// mark, and makes no commit for them: the ACKS wait for one made anyway.
#[test]
fn a_syncs_commit_acknowledges_the_plays_it_posted_and_makes_none_for_them() {
    let scratch = Scratch::new("plays");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    let mut again = |ask: &Ask| sync_plays(&sim, &base, ask, &mut envs, Reached::Elsewhere, store.clone()).0;
    ran(&again(&rock()));

    let device_id = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01];
    let epoch = 0x1122_3344_5566_7788;
    let seen = look(&sim);
    let records = seen.root.as_ref().unwrap().tags.as_ref().unwrap().records.clone();
    let play = |path: &str, seq: u32, boot_seq: u32, started_at: u64| {
        let row = records.iter().find(|e| e.path == path);
        Slot {
            kind: KIND_PLAY,
            outcome: 1,
            source: 2,
            seq,
            epoch,
            device_id,
            started_at,
            boot_seq,
            uptime_ms: seq * 1000,
            played_ms: 181_000,
            path_hash: crate::device::card::contract::prim::path_hash(path),
            size: row.map_or(1234, |r| r.rec.size),
            fat_time: row.map_or(0x5D47_73D5, |r| r.rec.fat_time),
            ..Slot::default()
        }
        .encode()
    };
    let at = 1_791_950_000_000;
    let log = [
        play("Artist A/Album B/01.mp3", 1, 1, at),
        play("Artist C/Album D/01.opus", 2, 1, at + 200_000),
        play("Mine/song.mp3", 3, 1, at + 400_000),
        play("Artist A/Album B/02.flac", 4, 2, 0),
    ]
    .concat();
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), &log).unwrap();
    {
        let mut lib = fake.lock().unwrap();
        let mut more = lib.tracks[0].clone();
        (more.path, more.id) = ("Rock/Artist A/Album B/03.mp3".into(), 6);
        lib.tracks.push(more);
        lib.revision = "1:6:1790000000006:5".into();
    }
    let second = again(&Ask::default());
    assert_eq!(ran(&second).written, 1, "{:?}", second.notes);
    let t = second.plays.as_ref().unwrap();
    assert_eq!((t.new, t.owed, t.own, t.clock, t.accepted, t.waiting), (4, 2, 1, 1, 2, 0), "{t:?}");
    let posted: Vec<Value> = fake.lock().unwrap().plays.clone();
    let paths: Vec<&str> = posted.iter().map(|p| p["filePath"].as_str().unwrap()).collect();
    assert_eq!(paths, ["Rock/Artist A/Album B/01.mp3", "Rock/Artist C/Album D/01.wav"]);
    assert_eq!(posted[0]["source"], "shuffle");
    let mark = |seq: u32| vec![Ack { device_id, epoch, seq }];
    let seen = look(&sim);
    let generation = seen.root.as_ref().unwrap().manifest.header.generation;
    assert_eq!(seen.root.as_ref().unwrap().manifest.acks, mark(4), "the commit made anyway takes them");
    assert_eq!(State::of(&seen.state).unwrap().plays, mark(4));

    // One more play, and nothing else: posted, the mark stored, no commit.
    let more = [log.clone(), play("Artist A/Album B/01.mp3", 5, 3, at + 900_000).to_vec()].concat();
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), &more).unwrap();
    let third = again(&Ask::default());
    assert_eq!(ran(&third).generation, generation, "no commit for the plays alone");
    assert_eq!(third.plays.as_ref().unwrap().accepted, 1);
    let seen = look(&sim);
    assert_eq!(seen.root.as_ref().unwrap().manifest.acks, mark(4), "the ACKS wait for a commit");
    assert_eq!(State::of(&seen.state).unwrap().plays, mark(5));
    assert_eq!(fake.lock().unwrap().has.len(), 3, "each play credited once");
}

/// A run's plays to send that no outbox holds (its saves fail) and the
/// server doesn't settle (its post fails): the sync stops before it plans,
/// the card as it was, since its checkpoint would replace the rows they map
/// through. Once the outbox takes them the sync runs, and they wait there
/// for the server.
#[test]
fn a_sync_whose_plays_no_outbox_holds_stops_before_it_plans() {
    let scratch = Scratch::new("unstored");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    let mut again = |ask: &Ask| {
        sync_plays(&sim, &base, ask, &mut envs, Reached::Elsewhere, store.clone()).0
    };
    ran(&again(&rock()));

    let (device_id, epoch) = ([0x00, 0x00, 0x5E, 0x00, 0x53, 0x01], 0x1122_3344_5566_7788);
    let seen = look(&sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let row = records.iter().find(|e| e.path == "Artist A/Album B/01.mp3").unwrap();
    let play = Slot {
        kind: KIND_PLAY,
        outcome: 1,
        source: 1,
        seq: 1,
        epoch,
        device_id,
        started_at: 1_791_950_000_000,
        boot_seq: 1,
        uptime_ms: 1000,
        played_ms: 181_000,
        path_hash: crate::device::card::contract::prim::path_hash(&row.path),
        size: row.rec.size,
        fat_time: row.rec.fat_time,
        ..Slot::default()
    };
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), play.encode()).unwrap();
    fake.lock().unwrap().stats_down = true;
    store.saves_left.set(Some(0));
    let without_lock = |mut image: BTreeMap<String, (u64, u32, u64)>| {
        image.retain(|p, _| !p.ends_with("run.lock"));
        image
    };
    let image = without_lock(sim.image());
    let stopped = again(&Ask::default());
    let End::Halted(Halt::Refused(why)) = &stopped.end else { panic!("{:?}", stopped.end) };
    assert!(why.contains("1 of the card's plays to send aren't in the plays' outbox"), "{why}");
    assert_eq!(without_lock(sim.image()), image, "stopped before a write");
    assert_eq!(stopped.plays.as_ref().map(|t| (t.owed, t.failed)), Some((1, true)));

    // The outbox takes them: the run goes on, and they wait for the server.
    store.saves_left.set(None);
    let waited = again(&Ask::default());
    ran(&waited);
    assert_eq!(store.outbox().cards.values().map(|o| o.plays.len()).sum::<usize>(), 1);
    assert_eq!(State::of(&look(&sim).state).unwrap().plays, []);
    fake.lock().unwrap().stats_down = false;
    let posted = again(&Ask::default());
    assert_eq!(posted.plays.as_ref().unwrap().accepted, 1);
    assert_eq!(State::of(&look(&sim).state).unwrap().plays, [Ack { device_id, epoch, seq: 1 }]);
}

#[test]
fn a_dry_run_plans_and_writes_nothing() {
    let scratch = Scratch::new("dry");
    let sim = card(&scratch);
    let (base, fake) = server();
    let before = sim.image();
    let writes = sim.log().len();
    let ask = Ask { dry_run: true, ..rock() };
    let synced = sync_now(&sim, &base, &ask, &mut envs()).0;
    assert!(matches!(synced.end, End::Dry), "{:?}", synced.end);
    let summary = synced.summary.as_ref().unwrap();
    assert_eq!((summary.fetches, summary.conversions, summary.files), (4, 1, 4));
    assert!(summary.fit.is_some_and(|f| f.fits()));
    assert_eq!(sim.image(), before, "nothing reached the card");
    assert_eq!(sim.log().len(), writes);
    let asked = fake.lock().unwrap().asked.clone();
    assert!(!asked.iter().any(|a| a.starts_with("GET /transcode/") || a.starts_with("GET /media/")), "{asked:?}");
    let printed = synced_json(&synced, true);
    assert_eq!(printed["plan"]["fetch"]["files"], 4);
    assert_eq!(printed["dryRun"], true);
}

#[test]
fn a_plan_that_doesnt_fit_or_removes_most_of_the_card_stops_before_any_write() {
    let scratch = Scratch::new("guards");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let client = || Arc::new(Client::new(&base).unwrap());
    let tight = |c: &FatSim, envs: &mut dyn FnMut() -> Env| {
        let (cards, plays) = (&mut Cards::default(), Rc::new(MemStore::default()));
        let (fed, space, elsewhere) = (no_peers(), space(1 << 20), Reached::Elsewhere);
        sync(c, space, client(), elsewhere, &rock(), cards, plays, &fed, envs, &mut |_| {})
    };
    let image = sim.image();
    let synced = tight(&sim, &mut envs);
    let End::Halted(Halt::Fit(fit)) = synced.end else { panic!("{:?}", synced.end) };
    assert!(!fit.fits() && fit.short() > 0);
    assert_eq!(sim.image(), image, "refused before a write");

    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    // The server loses most of the library: the stronger gate asks first.
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks.retain(|t| t.path.ends_with("01.mp3"));
        lib.revision = "1:2:1790000000001:1".into();
    }
    let image = sim.image();
    let asked = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let End::Halted(Halt::Card(Stop::Unanswered(guards))) = &asked.end else { panic!("{:?}", asked.end) };
    assert!(matches!(&guards[..], [Guard::Removals { strong: true, paths }] if paths.len() == 3), "{guards:?}");
    let mut now = sim.image();
    now.retain(|p, _| !p.ends_with("run.lock"));
    assert_eq!(now, image);
    // Answered: the card mirrors the server.
    let yes = Ask { answers: Answers { mass_removals: true, ..Answers::default() }, ..Ask::default() };
    let outcome = sync_now(&sim, &base, &yes, &mut envs).0;
    assert_eq!(ran(&outcome).records, 1);
}

#[test]
fn a_card_not_set_up_or_a_server_with_no_manifest_is_refused() {
    let scratch = Scratch::new("refused");
    let sim = FatSim::new(&scratch.0.join("bare"), SERIAL);
    let (base, fake) = server();
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    let refused = matches!(&synced.end, End::Halted(Halt::Refused(why)) if why.contains("card-setup"));
    assert!(refused, "{:?}", synced.end);

    let set_up = card(&scratch);
    let unknown = Ask { libraries: Some(vec!["Blues".into()]), ..Ask::default() };
    let synced = sync_now(&set_up, &base, &unknown, &mut envs()).0;
    assert!(matches!(&synced.end, End::Halted(Halt::Refused(why)) if why.contains("Blues")), "{:?}", synced.end);
    drop(fake);

    let old = routed(|asked| {
        if asked.path() == "/api/" {
            return json_reply("200 OK", r#"{"server":"6.0.0","features":{},"user":{"vpaths":["Rock"]}}"#);
        }
        json_reply("404 Not Found", "{}")
    });
    let synced = sync_now(&set_up, &old, &rock(), &mut envs()).0;
    assert!(matches!(synced.end, End::Halted(Halt::Server(_))), "{:?}", synced.end);
}

#[test]
fn a_lost_thumbnail_is_made_again_and_an_album_with_nothing_is_looked_at_once() {
    let scratch = Scratch::new("thumbs");
    let sim = card(&scratch);
    let (base, fake) = server();
    {
        // Album B: its folder's cover.jpg, and no art mStream names. Album
        // H: neither.
        let mut lib = fake.lock().unwrap();
        for t in lib.tracks.iter_mut().filter(|t| t.path.contains("/Album B/")) {
            t.art = None;
        }
        let mut bare = lib.tracks[4].clone();
        (bare.path, bare.id) = ("Rock/Artist G/Album H/01.mp3".into(), 6);
        lib.tracks.push(bare);
        lib.revision = "1:6:1790000000006:5".into();
    }
    let (b, d, h) = ("Artist A/Album B", "Artist C/Album D", "Artist G/Album H");
    let thumb = |album: &str| scratch.0.join("card/.mstream").join(thumb_path_of(album));
    let made = |album: &str| std::fs::read(thumb(album)).is_ok_and(|t| mpth::check(&t, path_hash(album)));
    let art = |sim: &FatSim| State::of(&look(sim).state).unwrap().settings.art;
    let probes = |album: &str| {
        let head = format!("HEAD /media/Rock/{album}/");
        fake.lock().unwrap().asked.iter().filter(|a| a.starts_with(&head)).count()
    };
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    assert!(made(b) && made(d) && !made(h));
    let first = art(&sim);
    assert_eq!(first.get(&path_hash(b)), Some(&FOLDER_IMAGE));
    assert_eq!(first.get(&path_hash(d)), Some(&art_key(["d.png"])));
    assert_eq!(first.get(&path_hash(h)), Some(&NO_ART));
    assert_eq!(probes(h), PROBES.len());

    // Both lost: a 304's run makes them again, from the folder's image and
    // from the art the files' metadata names now (step 10's check).
    std::fs::remove_file(thumb(b)).unwrap();
    std::fs::remove_file(thumb(d)).unwrap();
    let mended = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(mended.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(ran(&mended).verified, Ok(()));
    assert!(made(b) && made(d));
    assert_eq!(art(&sim), first);

    // Lost again, and the manifest read whole: made again.
    std::fs::remove_file(thumb(b)).unwrap();
    let whole = sync_now(&sim, &base, &Ask { whole: true, ..Ask::default() }, &mut envs).0;
    assert_eq!(whole.summary.as_ref().unwrap().listed, Listed::Whole);
    assert_eq!(ran(&whole).verified, Ok(()));
    assert!(made(b));
    assert_eq!(art(&sim), first);
    // The album with nothing to make one from was looked at once.
    assert_eq!(probes(h), PROBES.len());
}

#[test]
fn another_address_confirmed_as_the_same_server_is_one_source_from_then_on() {
    let scratch = Scratch::new("same");
    let sim = card(&scratch);
    let (base, fake) = server();
    // A server that doesn't say its own id: the listener says it.
    fake.lock().unwrap().old = true;
    let served = fake.clone();
    let other = routed(move |asked| answer(&served, asked));
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);

    // Another address: a server of its own, or this one? Asked (U7).
    let asked = sync_now(&sim, &other, &Ask::default(), &mut envs).0;
    let End::Halted(Halt::Card(Stop::Unanswered(guards))) = &asked.end else { panic!("{:?}", asked.end) };
    assert!(matches!(guards[..], [Guard::Server { .. }]), "{guards:?}");
    let same = Ask { same_server: true, ..Ask::default() };
    let synced = sync_now(&sim, &other, &same, &mut envs).0;
    let outcome = ran(&synced);
    assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 4));

    // Either address from now on, with no question: one source, which
    // answers both.
    for address in [&base, &other, &base] {
        let synced = sync_now(&sim, address, &Ask::default(), &mut envs).0;
        let outcome = ran(&synced);
        assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 4));
    }
    let s = State::of(&look(&sim).state).unwrap().settings;
    assert_eq!(s.sources.len(), 1, "{:?}", s.sources);
    let source = &s.sources[0];
    let keys = (server_url_key(&base), server_url_key(&other));
    assert_eq!((source.url_key, source.confirmed.clone()), (keys.0, vec![keys.1]));
    assert_eq!((source.nickname.as_str(), source.libraries.clone()), ("Home", vec!["Rock".to_string()]));
}

/// 2.11 with the server's own id (the card transfer's batch of mStream
/// changes): the id goes into the root as it is read; the server at another
/// address is the card's by it, with nothing asked and its selection kept;
/// another server at the card's server's address is never taken for it.
#[test]
fn a_server_is_known_by_its_own_id_at_any_address() {
    let scratch = Scratch::new("by-id");
    let sim = card(&scratch);
    let (base, fake) = server();
    let served = fake.clone();
    let other = routed(move |asked| answer(&served, asked));
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let table = look(&sim).root.unwrap().manifest.server_table();
    assert_eq!((table.root.instance, table.root.url_key), (instance_of(Some(ID)), server_url_key(&base)));

    // Its other address: the same server by its id. Nothing asked, nothing
    // written but the root, which takes the address it was read at.
    let synced = sync_now(&sim, &other, &Ask::default(), &mut envs).0;
    let outcome = ran(&synced);
    assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 4), "{:?}", synced.notes);
    let seen = look(&sim);
    let table = seen.root.unwrap().manifest.server_table();
    assert_eq!((table.root.instance, table.root.url_key), (instance_of(Some(ID)), server_url_key(&other)));
    assert!(table.entries.is_empty());
    let s = State::of(&seen.state).unwrap().settings;
    assert_eq!(s.sources.len(), 1, "one source answers both: {:?}", s.sources);
    assert_eq!(s.sources[0].libraries, ["Rock"], "its selection, carried");

    // Another server set up at that address: never the card's, and a
    // server of its own can't share the card's server's address. Refused
    // before anything is written, whatever the answers.
    fake.lock().unwrap().instance = Some("8899aabb-ccdd-4eff-8011-223344556677");
    let image = sim.image();
    let new_server = Answers { new_servers: true, ..Answers::default() };
    for ask in [Ask::default(), Ask { same_server: true, answers: new_server, ..Ask::default() }] {
        let refused = sync_now(&sim, &other, &ask, &mut envs).0;
        let End::Halted(Halt::Card(Stop::Refused(why))) = &refused.end else { panic!("{:?}", refused.end) };
        assert!(why.contains("address"), "{why}");
    }
    assert_eq!(sim.image(), image, "nothing written");

    // At an address of its own, the other server is asked about (U7).
    let served = fake.clone();
    let third = routed(move |asked| answer(&served, asked));
    let asked = sync_now(&sim, &third, &Ask::default(), &mut envs).0;
    let End::Halted(Halt::Card(Stop::Unanswered(guards))) = &asked.end else { panic!("{:?}", asked.end) };
    assert_eq!(guards[..], [Guard::Server { origin: 0, rows: 4 }]);
}

/// A server the listener called a server of its own at another address
/// before it gave its id, which then gives it: read at the card's address
/// the root takes the id, and read at the other the card's two servers are
/// one. Its files join the root's, read whole (a 304 stands for the root's
/// rows alone), its copies go, the two sources become one, and the run's
/// notes say both.
#[test]
fn a_server_set_beside_the_cards_own_is_joined_to_it_once_its_id_says_so() {
    let scratch = Scratch::new("joined");
    let sim = card(&scratch);
    let (base, fake) = server();
    fake.lock().unwrap().old = true;
    let served = fake.clone();
    let other = routed(move |asked| answer(&served, asked));
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let own = Ask { answers: Answers { new_servers: true, ..Answers::default() }, ..rock() };
    let beside = sync_now(&sim, &other, &own, &mut envs).0;
    assert_eq!(ran(&beside).records, 8, "{:?}", beside.notes);

    // The server gives its id: at the card's address, the root takes it.
    fake.lock().unwrap().old = false;
    let at_home = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(ran(&at_home).records, 8);
    let table = look(&sim).root.unwrap().manifest.server_table();
    assert_eq!((table.root.instance, table.entries.len()), (instance_of(Some(ID)), 1));

    // At the other address: one server, its copies gone, read whole.
    let joined = sync_now(&sim, &other, &Ask::default(), &mut envs).0;
    let outcome = ran(&joined);
    assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 4, 4), "{:?}", joined.notes);
    assert_eq!(joined.summary.as_ref().unwrap().listed, Listed::Whole);
    let (here, there) = (id_hex(server_url_key(&other)), id_hex(server_url_key(&base)));
    let notes = joined.notes.join("\n");
    assert!(notes.contains(&format!("answers {here} and {there}")), "{notes}");
    assert!(notes.contains(&format!("server at {here} is this server")), "{notes}");
    let seen = look(&sim);
    let root = seen.root.as_ref().unwrap();
    let table = root.manifest.server_table();
    assert_eq!((table.root.url_key, table.entries.len()), (server_url_key(&other), 0));
    let paths: Vec<&str> = root.tags.as_ref().unwrap().records.iter().map(|e| e.path.as_str()).collect();
    let b = ["Artist A/Album B/01.mp3", "Artist A/Album B/02.flac"];
    let d = ["Artist C/Album D/01.opus", "Artist C/Album D/02.opus"];
    assert_eq!(paths, [b, d].concat(), "the root's own files stay");
    let s = State::of(&seen.state).unwrap().settings;
    assert_eq!(s.sources.len(), 1, "{:?}", s.sources);

    // Either address from then on, nothing to do.
    for address in [&base, &other] {
        let again = sync_now(&sim, address, &Ask::default(), &mut envs).0;
        let outcome = ran(&again);
        assert_eq!((outcome.written, outcome.deleted, outcome.records), (0, 0, 4), "{:?}", again.notes);
    }
}

#[test]
fn a_card_that_never_removes_keeps_what_the_server_dropped() {
    let scratch = Scratch::new("never");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let never = Ask { change: Change { removal: Some(Removal::Never), ..rock().change }, ..rock() };
    ran(&sync_now(&sim, &base, &never, &mut envs).0);
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks.retain(|t| !t.path.ends_with("02.opus"));
        lib.revision = "1:4:1790000000005:3".into();
    }
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let summary = synced.summary.as_ref().unwrap();
    assert_eq!(summary.listed, Listed::Whole);
    assert!(summary.removals.is_empty() && summary.guards.is_empty(), "{summary:?}");
    assert_eq!(summary.report.kept.len(), 1, "{:?}", summary.report);
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.records, outcome.verified.clone()), (0, 4, Ok(())));
    assert!(scratch.0.join("card/music/Artist C/Album D/02.opus").exists());
    assert_eq!(State::of(&look(&sim).state).unwrap().settings.removal, Removal::Never);
}

#[test]
fn a_state_file_of_another_identity_is_refused() {
    let scratch = Scratch::new("identity");
    let sim = card(&scratch);
    let (base, _fake) = server();
    let mut state = State::of(&volume::inspect(&sim).unwrap().state).unwrap();
    state.card_id ^= 1;
    std::fs::write(scratch.0.join("card/.mstream/state.json"), state.text(state.seq + 1)).unwrap();
    let image = sim.image();
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    let refused = matches!(&synced.end, End::Halted(Halt::Refused(why)) if why.contains("another identity"));
    assert!(refused, "{:?}", synced.end);
    assert_eq!(sim.image(), image, "nothing written");
}

#[test]
fn a_run_the_server_stops_parks_and_the_next_takes_it_up() {
    let scratch = Scratch::new("parked");
    let sim = card(&scratch);
    let (base, fake) = server();
    fake.lock().unwrap().refused.insert("Rock/Artist C/Album D/02.opus".into());
    let mut envs = envs();
    let parked = sync_now(&sim, &base, &rock(), &mut envs).0;
    let outcome = ran(&parked);
    let why = outcome.parked.as_deref().expect("parked");
    assert!(why.contains("sign-in"), "{why}");
    assert_eq!(outcome.stored, Ok(()));
    let seen = volume::inspect(&sim).unwrap();
    assert!(seen.pending.is_some(), "the plan stays on the card");
    // What the run was asked is stored; the schedule waits for a run that
    // ends.
    let s = State::of(&seen.state).unwrap().settings;
    assert_eq!(s.sources[0].libraries, ["Rock"]);
    assert_eq!((s.sources[0].full_read, s.sources[0].plan_key), (0, 0));

    fake.lock().unwrap().refused.clear();
    let again = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&again);
    assert_eq!((outcome.parked.clone(), outcome.verified.clone(), outcome.records), (None, Ok(()), 4));
    let seen = look(&sim);
    assert!(seen.pending.is_none());
    assert_eq!(State::of(&seen.state).unwrap().settings.sources[0].full_read, 1_791_900_000);
}

#[test]
fn converting_everything_to_mp3_converts_each_file_and_puts_back_what_it_dropped() {
    let scratch = Scratch::new("mp3");
    let sim = card(&scratch);
    let (base, fake) = server();
    {
        // Ten seconds each; 383 frames of 1152 samples at 44.1 kHz: 10.005 s.
        let mut lib = fake.lock().unwrap();
        let rock = lib.tracks.iter().map(|t| t.path.clone()).filter(|p| p.starts_with("Rock/"));
        let rock: Vec<String> = rock.collect();
        for t in lib.tracks.iter_mut() {
            t.seconds = 10.0;
        }
        for path in rock {
            lib.converted.insert(path, mp3_bytes(383, false));
        }
    }
    let change = Change {
        convert: Some(Convert::Everything),
        codec: Some(TransferCodec::Mp3),
        bitrate: Some(TransferBitrate::K128),
        ..rock().change
    };
    let synced = sync_now(&sim, &base, &Ask { change, ..rock() }, &mut envs()).0;
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()), "{:?} {:?}", synced.notes, outcome.report);
    assert_eq!((outcome.written, synced.summary.as_ref().unwrap().conversions), (4, 4));
    let seen = look(&sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let paths: Vec<&str> = records.iter().map(|e| e.path.as_str()).collect();
    let b = ["Artist A/Album B/01.mp3", "Artist A/Album B/02.mp3"];
    let d = ["Artist C/Album D/01.mp3", "Artist C/Album D/02.mp3"];
    assert_eq!(paths, [b, d].concat());
    for e in records {
        let ledger = e.ledger.as_ref().unwrap();
        assert_eq!((ledger.converted_to, ledger.convert_kbps), (CONTAINER_MP3, 128), "{}", e.path);
        let bytes = std::fs::read(scratch.0.join("card/music").join(&e.path)).unwrap();
        let cover = crate::device::card::tags::read_bytes(&bytes, "mp3").picture;
        assert_eq!(cover.map(|c| (c.pic_type, c.mime)), Some((3, 1)), "{}: a JPEG front cover", e.path);
    }
    // The WAV's own BPM and key, as tags; mStream's analysis for the MP3
    // whose BPM was mStream's.
    assert_eq!((records[2].rec.bpm10, records[2].rec.camelot), (1280, 8));
    assert_eq!(records[2].rec.flags & BPM_ANALYSED, 0);
    assert_ne!(records[0].rec.flags & BPM_ANALYSED, 0);
}

/// MP3 past 192k: asked for only of a server that lists the rate, and in
/// its bulk queue only where it has one (the card transfer's batch of
/// mStream changes). An older server's conversions are as they always were.
#[test]
fn mp3_at_320k_is_asked_only_of_a_server_that_lists_it_and_in_its_bulk_queue() {
    let scratch = Scratch::new("320k");
    let sim = card(&scratch);
    let (base, fake) = server();
    {
        let mut lib = fake.lock().unwrap();
        for t in lib.tracks.iter_mut() {
            t.seconds = 10.0;
        }
        lib.converted.insert("Rock/Artist C/Album D/01.wav".into(), mp3_bytes(383, false));
    }
    let kbps = |sim: &FatSim| {
        let seen = look(sim);
        let records = seen.root.unwrap().tags.unwrap().records;
        let converted = records.iter().filter_map(|e| e.ledger.clone()).find(|l| l.converted_to != 0);
        converted.map(|l| (l.converted_to, l.convert_kbps))
    };
    let mp3 = Change { codec: Some(TransferCodec::Mp3), bitrate: Some(TransferBitrate::K320), ..rock().change };
    let mut envs = envs();
    let synced = sync_now(&sim, &base, &Ask { change: mp3, ..rock() }, &mut envs).0;
    assert_eq!(ran(&synced).verified, Ok(()), "{:?}", synced.notes);
    assert_eq!(kbps(&sim), Some((CONTAINER_MP3, 192)), "the most an older server makes");
    assert_eq!(std::mem::take(&mut fake.lock().unwrap().transcodes), ["codec=mp3&bitrate=192k"]);

    // The server lists 320k and queues bulk work: the plan is another, read
    // whole, and the conversion made again at the setting's rate.
    let rates = json!(["64k", "96k", "128k", "192k", "256k", "320k"]);
    fake.lock().unwrap().transcode = Some(json!({
        "defaultCodec": "opus", "defaultBitrate": "96k", "bulk": {"concurrency": 1},
        "codecs": {"mp3": {"bitrates": rates}, "opus": {"bitrates": ["64k", "96k", "128k", "192k"]}},
    }));
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&synced);
    assert_eq!((outcome.written, outcome.verified.clone()), (1, Ok(())), "{:?}", outcome.report);
    assert_eq!(synced.summary.as_ref().map(|s| s.listed), Some(Listed::Whole));
    assert_eq!(kbps(&sim), Some((CONTAINER_MP3, 320)));
    assert_eq!(fake.lock().unwrap().transcodes, ["codec=mp3&bitrate=320k&bulk=1"]);
    let settings = State::of(&look(&sim).state).unwrap().settings;
    assert_eq!((settings.conversion.codec, settings.conversion.bitrate), (TransferCodec::Mp3, TransferBitrate::K320));
}

#[test]
fn beside_its_server_a_sync_reads_the_files_from_the_servers_own_folder() {
    let scratch = Scratch::new("disk");
    let sim = card(&scratch);
    let (base, fake) = server();
    let folder = scratch.0.join("server/Rock");
    {
        let mut lib = fake.lock().unwrap();
        for t in lib.tracks.iter().filter(|t| t.path.starts_with("Rock/")) {
            let path = folder.join(&t.path["Rock/".len()..]);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &t.bytes).unwrap();
            let at = UNIX_EPOCH + Duration::from_millis(1_790_000_000_000 + t.id as u64);
            File::options().write(true).open(&path).unwrap().set_modified(at).unwrap();
        }
        let cover = lib.images["Rock/Artist A/Album B/cover.jpg"].clone();
        std::fs::write(folder.join("Artist A/Album B/cover.jpg"), cover).unwrap();
        lib.disk = Some(folder.clone());
    }
    let synced = sync_via(&sim, &base, &rock(), &mut envs(), Reached::Loopback).0;
    let outcome = ran(&synced);
    assert_eq!(outcome.verified, Ok(()), "{:?}", synced.notes);
    assert_eq!((synced.summary.as_ref().unwrap().source, outcome.written), ("disk", 4));
    // The originals and the folder's cover from disk, nothing probed; the
    // conversion is the server's to make.
    let asked = fake.lock().unwrap().asked.clone();
    let media: Vec<&String> = asked.iter().filter(|a| a.contains(" /media/")).collect();
    assert!(media.is_empty(), "{media:?}");
    assert!(asked.contains(&"GET /transcode/Rock/Artist C/Album D/01.wav".to_string()), "{asked:?}");
    for album in ["Artist A/Album B", "Artist C/Album D"] {
        let thumb = std::fs::read(scratch.0.join("card/.mstream").join(thumb_path_of(album))).unwrap();
        assert!(mpth::check(&thumb, path_hash(album)), "{album}");
    }
}

// ── The hidden command ──────────────────────────────────────────────────────

#[test]
fn the_hidden_command_syncs_only_a_volume_the_rule_accepts() {
    use crate::device::card::volume::{Bus, Clock, Disk, Found, Fs, Serial, Table, Volume};
    let scratch = Scratch::new("command");
    let sim = card(&scratch);
    let image = sim.image();
    let at = |device: &str, fs: Fs| {
        let volume = Volume {
            path: scratch.0.join("card"),
            device: device.into(),
            fs,
            table: Table::Mbr,
            label: "CARD".into(),
            serial: Some(Serial::Fat(SERIAL)),
            space: space(32 << 30),
            read_only: false,
            utf8: Some(true),
            clock: Clock::Offset(0),
        };
        let disk = Disk {
            id: "sdz".into(),
            removable: true,
            bus: Bus::Usb,
            read_only: false,
            system: Vec::new(),
            others: Vec::new(),
        };
        Found::new(volume, disk)
    };
    // The card's own folder, once as an exFAT volume the rule refuses and
    // once as a FAT32 one it takes.
    let found = [at("sdy1", Fs::ExFat), at("sdz1", Fs::Fat32)];
    let mut asked = 0;
    let mut call = |named: &str, dry_run: bool| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let ask = Ask { dry_run, ..rock() };
        let mut saved = || {
            asked += 1;
            Err("none here".to_string())
        };
        let code = on(&found, named, &ask, &mut saved, &mut out, &mut err);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    };

    for dry_run in [true, false] {
        let (code, out, err) = call("not-a-volume", dry_run);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("isn't a volume"), "{err}");
        let (code, out, err) = call("sdy1", dry_run);
        assert_eq!((code, out.as_str()), (2, ""));
        assert!(err.contains("is refused: exfat"), "{err}");
    }
    // A volume it takes asks for the saved session; the refused ones never
    // got that far.
    let (code, _, err) = call("sdz1", true);
    assert_eq!(code, 2);
    assert!(err.contains("no saved session: none here"), "{err}");
    assert_eq!(asked, 1);
    assert_eq!(sim.image(), image, "nothing reached the card");
}

mod peers;
