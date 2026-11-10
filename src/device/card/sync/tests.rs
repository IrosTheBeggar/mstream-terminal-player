//! A whole sync, end to end: a loopback mStream of made-up libraries
//! ([`Fake`]: the paged manifest with its ETag and 304, `metadata/batch`,
//! `/media` with Range and HEAD, `/transcode`, album art, embeddings,
//! waveforms and mood scores), a host folder as the card ([`FatSim`]), set up
//! by B2's set-up, and the card read back by the kit's reader. The audio is the
//! tag reader's parity corpus and made-up Ogg Opus streams; the covers are
//! made-up pictures; the embeddings are made-up vectors (no real index:
//! AUTODJ-INDEX.md 12.5), the peaks made-up shapes, the mood scores made-up
//! rows. The server's federation peers, and their own loopback servers, are
//! [`peers`]'s; multi-card sets, several cards of one selection, [`span`]'s;
//! the waveforms' fetch, [`waves`]'s; the mood tails, [`moods`]'s.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine;
use serde_json::{Value, json};

use super::*;
use crate::api::transfer::stub::{Asked, chunked, json as json_reply, reply, routed};
use crate::api::transfer::{MOOD_HEADS, MoodRow};
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
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Scratch {
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

/// What the fake's waveform route says of a track.
#[derive(Clone, Debug)]
pub(crate) enum Wave {
    /// 200, these peaks.
    Peaks(Vec<u8>),
    /// These words, at once: `404 Not Found`, `503 Service Unavailable`.
    Says(&'static str),
    /// These words, with this Retry-After in seconds.
    After(&'static str, u64),
    /// This answer, this long after it was asked for.
    Slow(Duration, Box<Wave>),
    /// This answer, once this many asks are in flight (5 s at most).
    Together(usize, Box<Wave>),
    /// No answer: the connection closes.
    Dropped,
}

/// A track of the fake server.
#[derive(Clone)]
pub(crate) struct Track {
    pub(crate) path: String,
    pub(crate) id: i64,
    pub(crate) bytes: Vec<u8>,
    pub(crate) seconds: f64,
    pub(crate) art: Option<String>,
    /// The full metadata's own fields: sample rate, channels, BPM.
    pub(crate) full: Value,
}

#[derive(Default)]
pub(crate) struct Library {
    pub(crate) vpaths: Vec<String>,
    pub(crate) tracks: Vec<Track>,
    pub(crate) revision: String,
    /// Folder images, by path: served, never listed.
    pub(crate) images: HashMap<String, Vec<u8>>,
    pub(crate) art: HashMap<String, Vec<u8>>,
    /// A conversion's bytes, by its source's path.
    pub(crate) converted: HashMap<String, Vec<u8>>,
    /// Conversions that end short once, as a dying ffmpeg leaves them.
    pub(crate) short_once: HashSet<String>,
    /// Files whose `/media` GET answers 401: the sign-in revoked mid-run.
    pub(crate) refused: HashSet<String>,
    /// Files whose `/media` GET answers 500: a file that fails, the run
    /// going on.
    pub(crate) broken: HashSet<String>,
    /// Files whose `/media` GET with a Range answers 500: a sniff of their
    /// first bytes that fails.
    pub(crate) unranged: HashSet<String>,
    /// `/api/` names no transcode block: the server converts nothing (its
    /// ffmpeg still starting, say).
    pub(crate) no_transcode: bool,
    /// The manifest gives no `file-size` (mStream's null for a row it has
    /// none for).
    pub(crate) no_sizes: bool,
    /// Rock's folder on this computer: the session is an admin's, which
    /// may ask `admin/directories`.
    pub(crate) disk: Option<PathBuf>,
    /// Every request, as "METHOD path".
    pub(crate) asked: Vec<String>,
    /// Each path `metadata/batch` was asked for.
    pub(crate) batched: Vec<String>,
    /// The plays posted to `stats/plays`, as they came, and the ids it has.
    /// A play of a path it has no track at (or hasn't scanned) is rejected
    /// as `unknown-track`, as mStream's ingest answers it.
    pub(crate) plays: Vec<Value>,
    pub(crate) has: HashSet<String>,
    /// A scan is under way: the manifest says so.
    pub(crate) scanning: bool,
    /// The tracks the scan hasn't reached, by path: on its disk, in no
    /// listing yet, and unknown to `stats/plays`.
    pub(crate) unindexed: HashSet<String>,
    /// `stats/plays` answers 500: a post that fails.
    pub(crate) stats_down: bool,
    /// The sign-in lapsed: every route answers 401.
    pub(crate) signed_out: bool,
    /// No Stats API: `/api/` names none, and nothing takes plays.
    pub(crate) no_stats: bool,
    /// `/api/`'s transcode block, when not an older server's (no rates
    /// listed, no bulk queue).
    pub(crate) transcode: Option<Value>,
    /// Each `/transcode` request's query, as it came.
    pub(crate) transcodes: Vec<String>,
    /// What the embeddings route says in place of an answer (`500 Internal
    /// Server Error`), and how many times it was asked.
    pub(crate) embeddings_say: Option<&'static str>,
    pub(crate) embedding_calls: usize,
    /// Paths whose vector has a value that isn't finite.
    pub(crate) bad_vectors: HashSet<String>,
    /// Set as the embeddings route is asked: the listener's stop, during
    /// the Auto DJ step.
    pub(crate) stop_at_embeddings: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Its bulk queue is full for good: every `/transcode` is a 503 in the
    /// queue's words, to ask again in a second; from the second, this stop
    /// is set (the listener's, during a wait for the queue).
    pub(crate) queue_full: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Older than the card transfer's batch of mStream changes: no
    /// `instanceId` in `/api/`.
    pub(crate) old: bool,
    /// Its `instanceId`, when not [`ID`].
    pub(crate) instance: Option<&'static str>,
    /// The version `/api/` says, when not 6.31.1 (the one the card
    /// transfer's batch was cut from, which its builds say).
    pub(crate) version: Option<&'static str>,
    /// `/api/`'s `features.federationProxy`, when it says one.
    pub(crate) proxy_says: Option<Value>,
    /// What the waveform route says of each track, by its path: each in turn,
    /// then the last for as long as it is asked. A track with none: 404.
    pub(crate) waves: HashMap<String, Vec<Wave>>,
    /// Each path the waveform route was asked for, as it came.
    pub(crate) waved: Vec<String>,
    /// The asks it is answering now, and how many there were as each came.
    pub(crate) wave_flight: Arc<AtomicUsize>,
    pub(crate) wave_seen: Vec<usize>,
    /// `/api/`'s `features.moodScores`, when it says one. The mood route
    /// answers either way, as a server's never would, so a sync that asks it
    /// without the flag shows.
    pub(crate) moods_flag: Option<u32>,
    /// What the mood route gives each path, as its JSON (a row in
    /// [`MOOD_HEADS`] order, or null); a path with none is null, as a track
    /// with no row is.
    pub(crate) moods: HashMap<String, Value>,
    /// Paths the mood route leaves out of its answer, as no mStream does.
    pub(crate) moods_left_out: HashSet<String>,
    /// What the mood route says in place of an answer: `403 Forbidden`,
    /// `500 Internal Server Error`.
    pub(crate) moods_say: Option<&'static str>,
    /// The paths of each call of the mood route, as they came.
    pub(crate) mood_calls: Vec<Vec<String>>,
    /// Its federation peers (B9).
    peers: Vec<peers::FedPeer>,
}

/// The fake server's own id.
const ID: &str = "3f2b8c1e-6a4d-4e0f-9b7a-2c5d8e1f0a93";

pub(crate) type Fake = Arc<Mutex<Library>>;

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

/// The waveform route's answer: `flight` the asks it is answering now.
fn wave_reply(wave: &Wave, flight: &AtomicUsize) -> Vec<u8> {
    match wave {
        Wave::Peaks(peaks) => json_reply("200 OK", &json!({ "waveform": peaks }).to_string()),
        Wave::Says(words) => json_reply(words, r#"{"error":"a made-up refusal"}"#),
        Wave::After(words, secs) => reply(words, &[("Retry-After", &secs.to_string())], b"{}"),
        Wave::Slow(wait, then) => {
            std::thread::sleep(*wait);
            wave_reply(then, flight)
        }
        Wave::Together(n, then) => {
            let t0 = std::time::Instant::now();
            while flight.load(Ordering::SeqCst) < *n && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
            wave_reply(then, flight)
        }
        Wave::Dropped => Vec::new(),
    }
}

fn answer(fake: &Fake, asked: &Asked) -> Vec<u8> {
    let mut lib = fake.lock().unwrap();
    let path = asked.path();
    lib.asked.push(format!("{} {path}", asked.method));
    if lib.signed_out {
        return json_reply("401 Unauthorized", r#"{"error":"Authentication Error"}"#);
    }
    let not_found = || json_reply("404 Not Found", r#"{"error":"Not Found"}"#);
    match (asked.method.as_str(), path.as_str()) {
        ("GET", "/api/") => {
            let mut body = json!({
                "server": lib.version.unwrap_or("6.31.1"),
                "features": {"sync": true, "discovery": true, "stats": 2,
                             "transcode": lib.transcode.clone().unwrap_or_else(
                                 || json!({"defaultCodec": "opus", "defaultBitrate": "96k"}))},
                "user": {"vpaths": lib.vpaths, "admin": false},
            });
            if !lib.old {
                body["instanceId"] = json!(lib.instance.unwrap_or(ID));
            }
            if lib.no_stats {
                body["features"].as_object_mut().expect("its features").remove("stats");
            }
            if lib.no_transcode {
                body["features"].as_object_mut().expect("its features").remove("transcode");
            }
            if let Some(proxy) = &lib.proxy_says {
                body["features"]["federationProxy"] = proxy.clone();
            }
            if let Some(flag) = lib.moods_flag {
                body["features"]["moodScores"] = json!(flag);
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
                .filter(|t| !lib.unindexed.contains(&t.path))
                .collect();
            tracks.sort_by_key(|t| t.id);
            let page: Vec<&Track> = tracks.iter().take(limit).copied().collect();
            let next = (tracks.len() > limit).then(|| page.last().map(|t| t.id)).flatten();
            let entry = |t: &Track| {
                let mut e = entry_json(t);
                if lib.no_sizes {
                    e["file-size"] = Value::Null;
                }
                e
            };
            let body = json!({"revision": lib.revision, "scanning": lib.scanning, "next": next,
                              "entries": page.iter().map(|t| entry(t)).collect::<Vec<_>>()});
            let headers = [("Content-Type", "application/json"), ("ETag", quoted.as_str())];
            reply("200 OK", &headers, body.to_string().as_bytes())
        }
        ("POST", "/api/v1/db/metadata/batch") => {
            let paths: Vec<String> = serde_json::from_value(asked.json()).unwrap_or_default();
            lib.batched.extend(paths.iter().cloned());
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
            lib.embedding_calls += 1;
            if let Some(stop) = &lib.stop_at_embeddings {
                stop.store(true, Ordering::SeqCst);
            }
            if let Some(words) = lib.embeddings_say {
                return json_reply(words, r#"{"error":"a made-up refusal"}"#);
            }
            let paths: Vec<String> = serde_json::from_value(asked.json()["filePaths"].clone()).unwrap_or_default();
            let nan = base64::engine::general_purpose::STANDARD.encode([f32::NAN.to_le_bytes(); 8].concat());
            let embedding = |p: &String| if lib.bad_vectors.contains(p) { nan.clone() } else { vector(p) };
            let tracks: Vec<Value> = paths
                .iter()
                .map(|p| json!({"filePath": p, "audioHash": null, "notAnalyzed": false, "embedding": embedding(p)}))
                .collect();
            let body = json!({"model": {"id": "effnet-discogs", "version": "1"}, "dim": 8, "tracks": tracks});
            json_reply("200 OK", &body.to_string())
        }
        ("POST", "/api/v1/db/mood-scores") => {
            let paths: Vec<String> =
                serde_json::from_value(asked.json()["filepaths"].clone()).unwrap_or_default();
            lib.mood_calls.push(paths.clone());
            if let Some(words) = lib.moods_say {
                return json_reply(words, r#"{"error":"a made-up refusal"}"#);
            }
            let given = paths.iter().filter(|p| !lib.moods_left_out.contains(*p));
            let tracks: serde_json::Map<String, Value> =
                given.map(|p| (p.clone(), lib.moods.get(p).cloned().unwrap_or(Value::Null))).collect();
            let body = json!({"model": {"id": "effnet-discogs", "version": "1"}, "moodVersion": "1",
                              "heads": MOOD_HEADS, "tracks": tracks});
            json_reply("200 OK", &body.to_string())
        }
        ("GET", "/api/v1/db/waveform") => {
            let query = asked.target.split_once('?').map_or("", |(_, q)| q);
            let pairs = url::form_urlencoded::parse(query.as_bytes());
            let filepath = pairs.into_iter().find(|(key, _)| key == "filepath");
            let filepath = filepath.map_or_else(String::new, |(_, value)| value.into_owned());
            lib.waved.push(filepath.clone());
            let wave = match lib.waves.get_mut(&filepath) {
                Some(script) if script.len() > 1 => script.remove(0),
                Some(script) => script[0].clone(),
                None => Wave::Says("404 Not Found"),
            };
            let flight = lib.wave_flight.clone();
            let now = flight.fetch_add(1, Ordering::SeqCst) + 1;
            lib.wave_seen.push(now);
            drop(lib);
            let out = wave_reply(&wave, &flight);
            flight.fetch_sub(1, Ordering::SeqCst);
            out
        }
        ("GET" | "HEAD", p) if p.starts_with("/media/") => {
            let rel = &p["/media/".len()..];
            if asked.method == "GET" && lib.refused.contains(rel) {
                return json_reply("401 Unauthorized", r#"{"error":"Authentication Error"}"#);
            }
            let ranged = asked.header("range").is_some();
            if asked.method == "GET" && (lib.broken.contains(rel) || (ranged && lib.unranged.contains(rel))) {
                return json_reply("500 Internal Server Error", r#"{"error":"Server Error"}"#);
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
            if let Some(stop) = lib.queue_full.as_ref() {
                if lib.transcodes.len() >= 2 {
                    stop.store(true, Ordering::SeqCst);
                }
                let words = br#"{"error":"too many bulk transcodes queued - retry later"}"#;
                return reply("503 Service Unavailable", &[("Retry-After", "1")], words);
            }
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
            let (mut accepted, mut duplicates, mut rejected) = (Vec::new(), Vec::new(), Vec::new());
            for play in plays {
                let id = play["id"].as_str().unwrap_or_default().to_string();
                let path = play["filePath"].as_str().unwrap_or_default();
                let scanned = lib.tracks.iter().any(|t| t.path == path) && !lib.unindexed.contains(path);
                if play.get("peerId").is_none() && !scanned {
                    rejected.push(json!({"id": id, "reason": "unknown-track"}));
                } else if lib.has.insert(id.clone()) {
                    accepted.push(id)
                } else {
                    duplicates.push(id)
                }
                lib.plays.push(play);
            }
            let body = json!({"accepted": accepted, "duplicates": duplicates, "rejected": rejected});
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
pub(crate) fn server() -> (String, Fake) {
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

/// A track's 800 made-up peaks, from its path: 0-255, as the route gives them.
pub(crate) fn peaks_of(path: &str) -> Vec<u8> {
    let h = crate::device::card::contract::prim::fnv1a64(path.as_bytes());
    (0..800u64).map(|i| (h.rotate_left((i * 7 % 64) as u32) >> 3) as u8).collect()
}

/// The waveform route gives every track its peaks.
pub(crate) fn with_waves(fake: &Fake) {
    let mut lib = fake.lock().unwrap();
    let paths: Vec<String> = lib.tracks.iter().map(|t| t.path.clone()).collect();
    for path in paths {
        let peaks = peaks_of(&path);
        lib.waves.insert(path, vec![Wave::Peaks(peaks)]);
    }
}

/// A track's made-up mood row, from its path: in [`MOOD_HEADS`] order, each
/// score on the server's grid of 4 places, and a NULL party for about one
/// track in three.
pub(crate) fn mood_row(path: &str) -> MoodRow {
    let h = crate::device::card::contract::prim::fnv1a64(path.as_bytes());
    let party = crate::device::card::contract::moods::PARTY;
    std::array::from_fn(|k| {
        let score = (h.rotate_left(8 * k as u32) % 10_001) as f64 / 10_000.0;
        (k != party || h % 3 != 0).then_some(score)
    })
}

/// The server says `moodScores`, and its mood route gives every track its
/// [`mood_row`].
pub(crate) fn with_moods(fake: &Fake) {
    let mut lib = fake.lock().unwrap();
    lib.moods_flag = Some(1);
    let paths: Vec<String> = lib.tracks.iter().map(|t| t.path.clone()).collect();
    for path in paths {
        let row = json!(mood_row(&path));
        lib.moods.insert(path, row);
    }
}

/// What a Mac leaves in a library it copied, which mStream's scanner lists
/// and its `/media` answers 404 for: an AppleDouble twin beside a file, and
/// a file in a dot-folder (an Ogg, so a sniff of its first bytes would be a
/// request too). Beside them a file whose name only has a dot in it, which
/// is served. The paths the server can't serve come back.
pub(crate) fn with_hidden(fake: &Fake) -> [&'static str; 2] {
    let twin = "Rock/Artist A/Album B/._01.mp3";
    let in_dot_folder = "Rock/Artist A/.hidden/03.ogg";
    let mut lib = fake.lock().unwrap();
    let like = lib.tracks[0].clone();
    let track = |path: &str, id: i64, bytes: Vec<u8>| {
        Track { path: path.into(), id, bytes, ..like.clone() }
    };
    let apple_double = b"\0\x05\x16\x07\0\x02\0\0Mac OS X a made-up AppleDouble".to_vec();
    lib.tracks.push(track(twin, 6, apple_double));
    lib.tracks.push(track(in_dot_folder, 7, opus_bytes(1000, 2, true)));
    lib.tracks.push(track("Rock/Artist A/Album B/03 Mr. Blue.mp3", 8, corpus("latin1.mp3")));
    lib.revision = "1:8:1790000000008:7".into();
    [twin, in_dot_folder]
}

/// No request of the server named one of `paths`: no `/media` GET or HEAD,
/// no conversion, no `metadata/batch`.
fn never_asked(fake: &Fake, paths: &[&str]) {
    let lib = fake.lock().unwrap();
    for path in paths {
        let asked: Vec<&String> = lib.asked.iter().filter(|a| a.contains(path)).collect();
        assert!(asked.is_empty(), "{path}: {asked:?}");
        assert!(!lib.batched.iter().any(|b| b == path), "{path}: its metadata was asked for");
    }
}

// ── The card ────────────────────────────────────────────────────────────────

pub(crate) const SERIAL: u32 = 0x1A2B_3C4D;

/// Each session's clock and ids: a fixed time, ids that never repeat.
pub(crate) fn envs() -> impl FnMut() -> Env {
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
pub(crate) fn card(scratch: &Scratch) -> FatSim {
    let root = scratch.0.join("card");
    let sim = FatSim::new(&root, SERIAL);
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card/golden/device.txt");
    std::fs::create_dir_all(root.join(".player")).unwrap();
    std::fs::copy(golden, root.join(".player/device.txt")).unwrap();
    let ask = setup::Ask { label: "Car".into(), new_identity: false };
    setup::set_up(&sim, &ask, &mut envs(), &mut Cards::default(), &mut |_| {}).expect("a set-up card");
    sim
}

pub(crate) fn space(free: u64) -> Option<Space> {
    Some(Space { size: 64 << 30, free, cluster: 32 * 1024, clusters: 0 })
}

/// One sync of `card` from the server at `base`, as the command makes one.
pub(crate) fn sync_now(card: &FatSim, base: &str, ask: &Ask, envs: &mut dyn FnMut() -> Env) -> (Synced, Vec<Event>) {
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
pub(crate) fn no_peers() -> Federation {
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

pub(crate) fn rock() -> Ask {
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

/// The device and epoch the plays of these tests are logged by.
pub(crate) const DEVICE: [u8; 6] = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01];
pub(crate) const EPOCH: u64 = 0x1122_3344_5566_7788;

/// A play of the card's file whose path ends with `path`, as the player logs
/// it: its row's hash, size and stamp from the card's own tags file.
pub(crate) fn logged(card: &FatSim, path: &str, seq: u32, started_at: u64) -> Vec<u8> {
    let seen = look(card);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let row = records.iter().find(|e| e.path.ends_with(path)).unwrap_or_else(|| panic!("{path} on the card"));
    let slot = Slot {
        kind: KIND_PLAY,
        outcome: 1,
        source: 2,
        seq,
        epoch: EPOCH,
        device_id: DEVICE,
        started_at,
        boot_seq: 1,
        uptime_ms: seq * 1000,
        played_ms: 181_000,
        path_hash: row.hash,
        size: row.rec.size,
        fat_time: row.rec.fat_time,
        ..Slot::default()
    };
    slot.encode().to_vec()
}

/// One more track on the fake server, in Rock, under a new revision: the
/// next sync has a file to fetch, and so a commit to make.
fn one_more_track(fake: &Fake, id: i64) {
    let mut lib = fake.lock().unwrap();
    let mut more = lib.tracks[0].clone();
    (more.path, more.id) = (format!("Rock/Artist A/Album B/{id:02}.mp3"), id);
    lib.tracks.push(more);
    lib.revision = format!("1:{id}:179000000000{id}:{}", id - 1);
}

/// A new recording on the fake server, in Rock, under a new revision: a
/// row the card's Auto DJ index doesn't have.
fn a_new_recording(fake: &Fake, id: i64) {
    one_more_track(fake, id);
    let mut lib = fake.lock().unwrap();
    lib.tracks.last_mut().unwrap().bytes = corpus("cbr.mp3");
}

/// The card's root's Auto DJ index entry.
fn autodj_named(sim: &FatSim) -> Option<crate::device::card::contract::msmf::Comp> {
    look(sim).root.unwrap().manifest.autodj
}

#[test]
fn a_vector_the_index_cant_hold_is_left_out_and_counted_never_the_index() {
    let scratch = Scratch::new("dj-nan");
    let sim = card(&scratch);
    let (base, fake) = server();
    fake.lock().unwrap().bad_vectors.insert("Rock/Artist A/Album B/02.flac".into());
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    assert_eq!(ran(&synced).verified, Ok(()));
    assert!(autodj_named(&sim).is_some(), "an index of the others: {:?}", synced.notes);
    let said = synced.notes.iter().any(|n| n.starts_with("1 tracks have no Auto DJ neighbours"));
    assert!(said, "{:?}", synced.notes);
}

#[test]
fn an_answers_vectors_are_of_its_dim_or_its_most_common_length_and_finite() {
    use crate::api::transfer::Embedding;
    let paths = ["a", "b", "c", "d", "e"];
    let hash_of: HashMap<&str, [u8; 16]> = paths.iter().enumerate().map(|(i, p)| (*p, [i as u8 + 1; 16])).collect();
    let hashes: HashSet<[u8; 16]> = hash_of.values().copied().collect();
    let row = |path: &str, v: Vec<f32>| Embedding { file_path: path.into(), vector: Some(v), ..Embedding::default() };
    let tracks = vec![
        row("a", vec![0.5; 4]),
        row("b", vec![0.5; 3]),
        row("c", vec![0.5, f32::INFINITY, 0.5, 0.5]),
        row("d", vec![0.5; 4]),
        Embedding { file_path: "e".into(), not_analyzed: true, ..Embedding::default() },
    ];
    // No dim named: the most common length, 4; the odd one and the one not
    // finite are counted, the one not analysed isn't.
    let mut dim = None;
    let found = Embeddings { tracks: tracks.clone(), ..Embeddings::default() };
    let (usable, bad) = usable_vectors(found, &hash_of, &hashes, &mut dim);
    let mut rows: Vec<[u8; 16]> = usable.iter().map(|(h, _)| *h).collect();
    rows.sort();
    assert_eq!((rows, bad, dim), (vec![[1; 16], [4; 16]], 2, Some(4)));
    // A dim named is the run's; another origin's answer of another is none.
    let mut dim = None;
    let found = Embeddings { dim: 3, tracks: tracks.clone(), ..Embeddings::default() };
    let (usable, bad) = usable_vectors(found, &hash_of, &hashes, &mut dim);
    assert_eq!((usable.len(), bad, dim), (1, 3, Some(3)));
    let found = Embeddings { dim: 4, tracks, ..Embeddings::default() };
    let (usable, bad) = usable_vectors(found, &hash_of, &hashes, &mut dim);
    assert_eq!((usable.len(), bad), (0, 4));
}

#[test]
fn vectors_that_dont_come_keep_the_cards_index_while_it_holds_for_the_files() {
    let scratch = Scratch::new("dj-down");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let index = autodj_named(&sim);
    assert!(index.is_some());

    // A recording more, and the vectors don't come: each chunk tried three
    // times, then the card's index stays (the new file has no neighbours).
    a_new_recording(&fake, 6);
    {
        let mut lib = fake.lock().unwrap();
        (lib.embeddings_say, lib.embedding_calls) = (Some("500 Internal Server Error"), 0);
    }
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!((ran(&synced).written, ran(&synced).verified.clone()), (1, Ok(())));
    assert_eq!(autodj_named(&sim), index, "named again: {:?}", synced.notes);
    assert!(synced.notes.iter().any(|n| n.contains("stays as it was")), "{:?}", synced.notes);
    assert_eq!(fake.lock().unwrap().embedding_calls, 3);
    assert!(dj_owed(&sim), "kept for files it wasn't made for: owed");

    // The vectors come again: a run with nothing new (a 304) makes the index
    // owed, and only that.
    fake.lock().unwrap().embeddings_say = None;
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(synced.summary.as_ref().unwrap().listed, Listed::NotModified);
    assert_eq!(ran(&synced).written, 0);
    let made = autodj_named(&sim);
    assert!(made.is_some() && made != index, "made again: {:?}", synced.notes);
    assert!(!dj_owed(&sim));
    sync_now(&sim, &base, &Ask::default(), &mut envs);
    assert_eq!(autodj_named(&sim), made, "nothing owed: as it is");

    // A file it maps is another recording now: it no longer holds.
    {
        let mut lib = fake.lock().unwrap();
        lib.embeddings_say = Some("500 Internal Server Error");
        lib.tracks[0].bytes = corpus("cbr.mp3");
        lib.revision = "1:7:1790000000007:6".into();
    }
    let synced = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(ran(&synced).written, 1, "{:?}", ran(&synced).report);
    assert_eq!(autodj_named(&sim), None);
    let said = synced.notes.iter().any(|n| n.starts_with("no Auto DJ index: the server's embeddings didn't come"));
    assert!(said, "{:?}", synced.notes);
}

/// The card's state says its Auto DJ index is owed.
fn dj_owed(sim: &FatSim) -> bool {
    State::of(&look(sim).state).unwrap().autodj_owed
}

#[test]
fn an_index_kept_for_the_files_it_was_made_for_is_owed_nothing() {
    let scratch = Scratch::new("dj-held");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let index = autodj_named(&sim);
    // A whole read whose vectors don't come, the files as they were.
    fake.lock().unwrap().embeddings_say = Some("500 Internal Server Error");
    let synced = sync_now(&sim, &base, &Ask { whole: true, ..Ask::default() }, &mut envs).0;
    assert_eq!(synced.summary.as_ref().unwrap().listed, Listed::Whole);
    assert_eq!(autodj_named(&sim), index);
    assert!(!dj_owed(&sim), "{:?}", synced.notes);
}

#[test]
fn a_stop_in_the_auto_dj_step_keeps_the_cards_index() {
    let scratch = Scratch::new("dj-stop");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let index = autodj_named(&sim);
    assert!(index.is_some());

    // Stopped as the vectors are asked for: the lists aren't made, and the
    // card's index stays, whatever came.
    a_new_recording(&fake, 6);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    fake.lock().unwrap().stop_at_embeddings = Some(stop.clone());
    let synced = sync_now(&sim, &base, &Ask { stop: Some(stop), ..Ask::default() }, &mut envs).0;
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written), (None, 1), "{:?}", outcome.report);
    assert_eq!(autodj_named(&sim), index, "{:?}", synced.notes);
    let said = synced.notes.iter().any(|n| n.contains("stays as it was: stopped at the listener's word"));
    assert!(said, "{:?}", synced.notes);
    assert!(dj_owed(&sim), "a stop that kept it owes it");
}

/// How many times the fake's `stats/plays` was asked.
fn play_posts(fake: &Fake) -> usize {
    fake.lock().unwrap().asked.iter().filter(|a| a.ends_with("/stats/plays")).count()
}

/// A card for someone else (clause 61): its plays are counted, sent to
/// nobody, and settled as they are. No request carries one; the commit the
/// run makes acknowledges them and the state file's marks pass them, at
/// the sync that marks the card and at the next, which has no flag. The
/// sync that makes the card the listener's own again settles what its log
/// holds the same way: nothing played while it was someone else's ever
/// goes. What is played after that is the listener's, and goes.
#[test]
fn a_card_for_someone_else_posts_no_play_and_its_commit_acknowledges_them_unposted() {
    let scratch = Scratch::new("gift-plays");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    let mut again = |ask: &Ask| sync_plays(&sim, &base, ask, &mut envs, Reached::Elsewhere, store.clone()).0;
    ran(&again(&rock()));
    let at = 1_791_950_000_000;
    let log = scratch.0.join("card/.player/plays.jnl");
    let mut plays = vec![logged(&sim, "Album B/01.mp3", 1, at), logged(&sim, "Album D/02.opus", 2, at + 200_000)];
    std::fs::write(&log, plays.concat()).unwrap();
    let for_whom = |gift: bool| Ask {
        change: Change { for_someone_else: Some(gift), ..Change::default() },
        ..Ask::default()
    };
    let mark = |seq: u32| vec![Ack { device_id: DEVICE, epoch: EPOCH, seq }];

    one_more_track(&fake, 6);
    let marked = again(&for_whom(true));
    assert_eq!(ran(&marked).written, 1, "{:?}", marked.notes);
    let t = marked.plays.as_ref().unwrap();
    assert_eq!((t.new, t.gift, t.owed, t.due, t.accepted, t.waiting), (2, 2, 0, 0, 0, 0), "{t:?}");
    let said = "2 plays not sent: the card is for someone else";
    assert!(marked.notes.iter().any(|n| n == said), "{:?}", marked.notes);
    assert_eq!(play_posts(&fake), 0, "no request carries a play");
    let seen = look(&sim);
    assert_eq!(seen.root.as_ref().unwrap().manifest.acks, mark(2), "acknowledged, unposted");
    let state = State::of(&seen.state).unwrap();
    assert_eq!((state.plays.clone(), state.settings.for_someone_else), (mark(2), true));

    // The next sync has no flag: the card's settings say it. One more play
    // of a server's file: counted, settled, and in no request.
    plays.push(logged(&sim, "Album B/01.mp3", 3, at + 400_000));
    std::fs::write(&log, plays.concat()).unwrap();
    one_more_track(&fake, 7);
    let still = again(&Ask::default());
    assert_eq!(ran(&still).written, 1, "{:?}", still.notes);
    assert_eq!(still.plays.as_ref().map(|t| (t.new, t.gift, t.owed, t.due)), Some((1, 1, 0, 0)));
    assert_eq!(play_posts(&fake), 0);
    let seen = look(&sim);
    assert_eq!(seen.root.as_ref().unwrap().manifest.acks, mark(3));
    assert_eq!(State::of(&seen.state).unwrap().plays, mark(3));

    // Played once more by someone else, then made the listener's own by
    // the sync itself: what the log holds is of the time before, and that
    // sync settles it unsent.
    plays.push(logged(&sim, "Album D/02.opus", 4, at + 600_000));
    std::fs::write(&log, plays.concat()).unwrap();
    let mine = again(&for_whom(false));
    ran(&mine);
    let t = mine.plays.as_ref().unwrap();
    assert_eq!((t.new, t.gift, t.owed, t.due, t.accepted), (1, 1, 0, 0, 0), "{t:?}");
    assert_eq!(play_posts(&fake), 0, "nothing of the time it was someone else's");
    let state = State::of(&look(&sim).state).unwrap();
    assert_eq!((state.plays.clone(), state.settings.for_someone_else), (mark(4), false));

    // The listener's own plays, from then on, go.
    plays.push(logged(&sim, "Album B/01.mp3", 5, at + 800_000));
    std::fs::write(&log, plays.concat()).unwrap();
    let own = again(&Ask::default());
    ran(&own);
    let t = own.plays.as_ref().unwrap();
    assert_eq!((t.new, t.gift, t.accepted, t.waiting), (1, 0, 1, 0), "{t:?}");
    let posted: Vec<Value> = fake.lock().unwrap().plays.clone();
    let ids: Vec<&str> = posted.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["core2-00005e005301-1122334455667788-5"], "that one alone, ever");
    assert_eq!(State::of(&look(&sim).state).unwrap().plays, mark(5));
}

/// A card for someone else whose play log can't be read is not made the
/// listener's own: nothing would settle the plays the log holds, and the
/// next look would send them. The sync that would have done it is refused
/// before it writes anything, and so is the settings' store (the sheet's
/// Apply): the card stays marked. Once the log reads, the store settles
/// those plays as it makes the card the listener's own. Its marks pass
/// them, written with the settings, and no sync sends one.
#[test]
fn a_card_whose_log_cant_be_read_stays_for_someone_else_and_the_store_settles_its_plays() {
    let scratch = Scratch::new("gift-blind");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    let for_whom = |gift: bool| Change { for_someone_else: Some(gift), ..Change::default() };
    let gift = Ask { change: Change { nickname: Some("Home".into()), ..for_whom(true) }, ..rock() };
    ran(&sync_plays(&sim, &base, &gift, &mut envs, Reached::Elsewhere, store.clone()).0);
    // Played by someone else, and no sync since.
    let at = 1_791_950_000_000;
    let log = [logged(&sim, "Album B/01.mp3", 1, at), logged(&sim, "Album D/02.opus", 2, at + 200_000)];
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log.concat()).unwrap();

    sim.bad_sector("/.player/plays.jnl", 0);
    let mine = Ask { change: for_whom(false), ..Ask::default() };
    let refused = sync_plays(&sim, &base, &mine, &mut envs, Reached::Elsewhere, store.clone()).0;
    let unread = |why: &str| why.contains("play log can't be read") && why.contains("for someone else");
    assert!(matches!(&refused.end, End::Halted(Halt::Refused(why)) if unread(why)), "{:?}", refused.end);
    let mut own = |state: &mut State| state.settings.for_someone_else = false;
    let stored = setup::store(&sim, &mut own, store.clone(), &mut envs, &mut |_| {});
    assert!(matches!(&stored, Err(Stop::Refused(why)) if unread(why)), "{:?}", stored.err());
    let state = State::of(&look(&sim).state).unwrap();
    assert_eq!((state.settings.for_someone_else, state.plays.clone()), (true, vec![]), "as it was");
    // A dry run writes nothing, and is no run to refuse.
    let dry = Ask { dry_run: true, ..mine.clone() };
    let planned = sync_plays(&sim, &base, &dry, &mut envs, Reached::Elsewhere, store.clone()).0;
    assert!(matches!(planned.end, End::Dry), "{:?}", planned.end);

    // The log reads again: the sheet's Apply makes the card the listener's
    // own, the plays its log holds settled first.
    sim.heal();
    let stored = setup::store(&sim, &mut own, store.clone(), &mut envs, &mut |_| {}).expect("stored");
    let mark = vec![Ack { device_id: DEVICE, epoch: EPOCH, seq: 2 }];
    assert_eq!((stored.settings.for_someone_else, stored.plays.clone()), (false, mark.clone()));
    assert_eq!(State::of(&look(&sim).state).unwrap().plays, mark, "written with the settings");
    let after = sync_plays(&sim, &base, &Ask::default(), &mut envs, Reached::Elsewhere, store.clone()).0;
    ran(&after);
    assert_eq!(after.plays.as_ref().map(|t| (t.new, t.due)), Some((0, 0)));
    assert_eq!(play_posts(&fake), 0, "nothing of the time it was someone else's");
    // A store that leaves the card whose it is looks at no play.
    sim.bad_sector("/.player/plays.jnl", 0);
    let mut renamed = |state: &mut State| state.label = "Gym".into();
    let stored = setup::store(&sim, &mut renamed, store.clone(), &mut envs, &mut |_| {}).expect("stored");
    assert_eq!((stored.label.as_str(), stored.plays.clone()), ("Gym", mark));
}

/// A server in a scan (the listing says so, and lacks the tracks the scan
/// hasn't reached) would answer that it doesn't know those tracks: their
/// plays aren't sent, and wait. Nothing settles them, the state file's mark
/// stays below the first, and no ACKS passes it. The scan over, they go. A
/// track the server dropped for good is another matter: a whole listing,
/// no scan running, lacks it, and the server's answer settles its play. And
/// a list that stood (a 304) says nothing of any path: a play the server
/// doesn't know waits.
#[test]
fn plays_of_tracks_a_scan_hasnt_reached_wait_and_a_dropped_tracks_play_settles() {
    let scratch = Scratch::new("scan-plays");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    let mut again = |ask: &Ask| sync_plays(&sim, &base, ask, &mut envs, Reached::Elsewhere, store.clone()).0;
    ran(&again(&rock()));
    let at = 1_791_850_000_000;
    let mut log = [
        logged(&sim, "Album B/01.mp3", 1, at),
        logged(&sim, "Album B/02.flac", 2, at + 200_000),
        logged(&sim, "Album D/02.opus", 3, at + 400_000),
    ]
    .concat();
    let plays_file = scratch.0.join("card/.player/plays.jnl");
    std::fs::write(&plays_file, &log).unwrap();
    let mark = |seq: u32| vec![Ack { device_id: DEVICE, epoch: EPOCH, seq }];
    let marks = || {
        let seen = look(&sim);
        (State::of(&seen.state).unwrap().plays, seen.root.as_ref().unwrap().manifest.acks.clone())
    };
    let posted = |fake: &Fake| -> Vec<String> {
        let plays = fake.lock().unwrap().plays.clone();
        plays.iter().map(|p| p["filePath"].as_str().unwrap().to_string()).collect()
    };

    // The server scans its library again, and has reached one track of it.
    let (flac, opus) = ("Rock/Artist A/Album B/02.flac", "Rock/Artist C/Album D/02.opus");
    {
        let mut lib = fake.lock().unwrap();
        lib.scanning = true;
        lib.unindexed = [flac, "Rock/Artist C/Album D/01.wav", opus].map(String::from).into();
        lib.revision = "1:1:1790000000001:9".into();
    }
    let scanning = again(&Ask::default());
    assert_eq!(ran(&scanning).deleted, 0, "a scan's listing removes nothing: {:?}", scanning.notes);
    let t = scanning.plays.as_ref().unwrap();
    let counts = (t.new, t.owed, t.accepted, t.rejected, t.unknown, t.waiting);
    assert_eq!(counts, (3, 3, 1, 0, 2, 2), "{t:?}");
    assert_eq!(posted(&fake), ["Rock/Artist A/Album B/01.mp3"], "the two it hasn't scanned are in no request");
    let said = "2 plays wait: the server doesn't know their tracks, and no listing says they are gone \
                (a scan may be under way)";
    assert!(scanning.notes.iter().any(|n| n == said), "{:?}", scanning.notes);
    let (stored, acks) = marks();
    assert_eq!(stored, mark(1), "the mark stays below the first that waits");
    assert!(acks.is_empty() || acks == mark(1), "{acks:?}");
    assert_eq!(store.outbox().cards.values().map(|o| o.plays.len()).sum::<usize>(), 2);

    // The scan over: they go, and the mark follows.
    {
        let mut lib = fake.lock().unwrap();
        (lib.scanning, lib.revision) = (false, "1:5:1790000000005:9".into());
        lib.unindexed.clear();
    }
    let done = again(&Ask::default());
    ran(&done);
    let t = done.plays.as_ref().unwrap();
    assert_eq!((t.accepted, t.rejected, t.unknown, t.waiting), (2, 0, 0, 0), "{t:?}");
    assert_eq!(posted(&fake)[1..], [flac, opus]);
    assert_eq!(marks().0, mark(3));

    // The server drops a track for good: a whole listing with no scan
    // running lacks it, and its answer settles the play.
    log.extend(logged(&sim, "Album D/02.opus", 4, at + 600_000));
    std::fs::write(&plays_file, &log).unwrap();
    {
        let mut lib = fake.lock().unwrap();
        lib.tracks.retain(|t| t.path != opus);
        lib.revision = "1:4:1790000000005:9".into();
    }
    let dropped = again(&Ask::default());
    assert_eq!(ran(&dropped).deleted, 1, "{:?}", dropped.notes);
    let t = dropped.plays.as_ref().unwrap();
    assert_eq!((t.accepted, t.rejected, t.unknown, t.waiting), (0, 1, 0, 0), "{t:?}");
    assert!(dropped.notes.iter().all(|n| !n.contains("the server doesn't know")), "{:?}", dropped.notes);
    assert_eq!(marks().0, mark(4));

    // A list that stood says nothing of any path: the play waits.
    log.extend(logged(&sim, "Album B/02.flac", 5, at + 800_000));
    std::fs::write(&plays_file, &log).unwrap();
    fake.lock().unwrap().unindexed.insert(flac.into());
    let stood = again(&Ask::default());
    ran(&stood);
    assert_eq!(stood.summary.as_ref().map(|s| s.listed), Some(Listed::NotModified));
    let t = stood.plays.as_ref().unwrap();
    assert_eq!((t.accepted, t.rejected, t.unknown, t.waiting), (0, 0, 1, 1), "{t:?}");
    assert_eq!(marks().0, mark(4), "held by the play that waits");
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
    // The run's notes say the post failed, as they say what else it
    // passed by.
    store.saves_left.set(None);
    let waited = again(&Ask::default());
    ran(&waited);
    let unposted = |n: &String| n.starts_with("the plays weren't posted: ");
    assert!(waited.notes.iter().any(unposted), "{:?}", waited.notes);
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

/// The files the server lists and can't serve (a dot part past the vpath)
/// are left out of the selection before anything is asked of them: never
/// planned, fetched, sniffed or looked up, in the dry run as in the run;
/// counted with their places in the summary (no note: the plan says them),
/// never a failure, and the run ends clean. A name with a dot inside it is
/// synced, and so is a library whose own name has one (the card names it
/// by 2.8.3's rule).
#[test]
fn the_files_the_server_cant_serve_are_left_out_and_counted_with_their_places() {
    let scratch = Scratch::new("hidden");
    let sim = card(&scratch);
    let (base, fake) = server();
    let hidden = with_hidden(&fake);
    {
        let mut lib = fake.lock().unwrap();
        lib.vpaths.push(".Attic".into());
        let like = lib.tracks[0].clone();
        let bytes = corpus("mpeg2.mp3");
        lib.tracks.push(Track { path: ".Attic/Artist G/Album H/01.mp3".into(), id: 9, bytes, ..like });
    }
    // Where they are: the dot-folder first, then the twin.
    let left_out = plan::Hidden {
        n: 2,
        sample: vec!["Rock/Artist A/.hidden/".into(), "Rock/Artist A/Album B/._01.mp3".into()],
        more: false,
    };
    let told = |synced: &Synced| synced.notes.iter().filter(|n| n.contains("hidden")).count();
    let ask = Ask { libraries: Some(vec!["Rock".into(), ".Attic".into()]), ..rock() };
    let mut envs = envs();

    // The dry run: the plan has the rest, and says how many it left out.
    let image = sim.image();
    let dry = sync_now(&sim, &base, &Ask { dry_run: true, ..ask.clone() }, &mut envs).0;
    assert!(matches!(dry.end, End::Dry), "{:?}", dry.end);
    let summary = dry.summary.as_ref().unwrap();
    assert_eq!((summary.files, summary.fetches), (6, 6), "{summary:?}");
    assert_eq!(summary.hidden, left_out);
    assert!(summary.skipped.is_empty(), "{:?}", summary.skipped);
    assert_eq!(told(&dry), 0, "{:?}", dry.notes);
    let rock: Vec<&(String, usize, u64)> = summary.totals.iter().filter(|t| t.0 == "Rock").collect();
    assert_eq!(rock[0].1, 5, "Rock's files the server serves");
    let printed = synced_json(&dry, true);
    assert_eq!(printed["plan"]["hidden"], 2);
    assert_eq!(printed["plan"]["hiddenSample"], json!(left_out.sample));
    assert_eq!(sim.image(), image, "nothing reached the card");
    never_asked(&fake, &hidden);

    // The run: the same, nothing fetched of them, nothing failed.
    let (synced, events) = sync_now(&sim, &base, &ask, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.parked.clone()), (Ok(()), None), "{:?}", synced.notes);
    assert_eq!((outcome.written, outcome.records), (6, 6), "{:?}", outcome.report);
    let report = &outcome.report;
    assert!(report.failed.is_empty() && report.refused.is_empty(), "{report:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::Failed { .. })), "{events:?}");
    assert_eq!(synced.summary.as_ref().map(|s| &s.hidden), Some(&left_out));
    assert_eq!(told(&synced), 0, "{:?}", synced.notes);
    never_asked(&fake, &hidden);
    let asked = fake.lock().unwrap().asked.clone();
    for served in ["Rock/Artist A/Album B/03 Mr. Blue.mp3", ".Attic/Artist G/Album H/01.mp3"] {
        assert!(asked.contains(&format!("GET /media/{served}")), "{served}: {asked:?}");
    }
    let seen = look(&sim);
    let records = &seen.root.as_ref().unwrap().tags.as_ref().unwrap().records;
    let paths: Vec<&str> = records.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&"_Attic/Artist G/Album H/01.mp3"), "{paths:?}");
    assert!(paths.contains(&"Rock/Artist A/Album B/03 Mr. Blue.mp3"), "{paths:?}");
    let dotted = |p: &&str| p.split('/').any(|part| part.starts_with('.'));
    assert!(!paths.iter().any(dotted), "{paths:?}");

    // Read whole again: still left out, counted again, and nothing to retry.
    let again = sync_now(&sim, &base, &Ask { whole: true, ..Ask::default() }, &mut envs).0;
    let outcome = ran(&again);
    assert_eq!((outcome.written, outcome.deleted, outcome.verified.clone()), (0, 0, Ok(())));
    assert!(outcome.report.failed.is_empty(), "{:?}", outcome.report);
    let listed = again.summary.as_ref().map(|s| (s.listed, s.hidden.clone()));
    assert_eq!(listed, Some((Listed::Whole, left_out)));
    assert_eq!(told(&again), 0, "{:?}", again.notes);
    never_asked(&fake, &hidden);
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

/// A run that stops because the card's server would change (clause 58's
/// take-over gate) read the other server: its tally still counts the
/// card's plays, which wait in the outbox for the server the card leaves,
/// and none goes to the one read. The gate says that count (the worker's
/// `plays_elsewhere`).
#[test]
fn a_run_stopped_for_another_server_counts_the_plays_that_wait_for_the_cards_own() {
    let scratch = Scratch::new("other-plays");
    let sim = card(&scratch);
    let (base, fake) = server();
    // A server that doesn't say its own id: another address asks first.
    fake.lock().unwrap().old = true;
    let served = fake.clone();
    let other = routed(move |asked| answer(&served, asked));
    let mut envs = envs();
    let store = Rc::new(MemStore::default());
    ran(&sync_plays(&sim, &base, &rock(), &mut envs, Reached::Elsewhere, store.clone()).0);
    let at = 1_791_950_000_000;
    let log = [logged(&sim, "Album B/01.mp3", 1, at), logged(&sim, "Album D/02.opus", 2, at + 200_000)];
    std::fs::write(scratch.0.join("card/.player/plays.jnl"), log.concat()).unwrap();

    let asked = sync_plays(&sim, &other, &Ask::default(), &mut envs, Reached::Elsewhere, store.clone()).0;
    let End::Halted(Halt::Card(Stop::Unanswered(guards))) = &asked.end else { panic!("{:?}", asked.end) };
    assert!(matches!(guards[..], [Guard::Server { .. }]), "{guards:?}");
    let t = asked.plays.as_ref().expect("the run's tally");
    assert_eq!((t.new, t.waiting, t.due, t.accepted), (2, 2, 0, 0), "{t:?}");
    assert_eq!(play_posts(&fake), 0, "none goes to a server that isn't theirs");
    assert_eq!(store.outbox().cards.values().map(|o| o.plays.len()).sum::<usize>(), 2);
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
    // Its checkpoint names no revision, and its plan is on the card: the
    // listing is read whole, never a 304 that forgets the plan's rest.
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::Whole);
    let outcome = ran(&again);
    assert_eq!((outcome.parked.clone(), outcome.verified.clone(), outcome.records), (None, Ok(()), 4));
    let seen = look(&sim);
    assert!(seen.pending.is_none());
    assert_eq!(State::of(&seen.state).unwrap().settings.sources[0].full_read, 1_791_900_000);
}

/// The card's own server's settings as its state file holds them now.
fn home_source(sim: &FatSim) -> state::Source {
    State::of(&look(sim).state).unwrap().settings.sources[0].clone()
}

/// The root's serverRevision.
fn root_revision(sim: &FatSim) -> String {
    look(sim).root.unwrap().manifest.server_revision
}

#[test]
fn a_file_that_failed_is_found_again_until_it_failed_three_runs_in_a_row() {
    let scratch = Scratch::new("failing");
    let sim = card(&scratch);
    let (base, fake) = server();
    let bad = "Rock/Artist A/Album B/01.mp3";
    fake.lock().unwrap().broken.insert(bad.into());
    let mut envs = envs();
    // Each run misses it: no revision is stored, nor the schedule, so the
    // next run reads the manifest whole and asks for it again (a 304 would
    // have the ledger, which lacks it, stand for the selection).
    for tries in 1..=3 {
        let ask = if tries == 1 { rock() } else { Ask::default() };
        let synced = sync_now(&sim, &base, &ask, &mut envs).0;
        assert_eq!(synced.summary.as_ref().unwrap().listed, Listed::Whole, "run {tries}");
        let outcome = ran(&synced);
        assert_eq!(outcome.missed, [(0, bad.to_string())], "run {tries}");
        assert_eq!(root_revision(&sim), "", "run {tries}");
        let source = home_source(&sim);
        assert_eq!((source.full_read, source.failing.len()), (0, 1), "run {tries}");
        assert_eq!((source.failing[0].path.as_str(), source.failing[0].tries), (bad, tries));
    }
    // The fourth passes it over, as listed: the revision is the card's again.
    let given_up = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let summary = given_up.summary.as_ref().unwrap();
    assert_eq!(summary.skipped, [(bad.to_string(), Skip::Failing)]);
    assert!(ran(&given_up).missed.is_empty());
    assert_ne!(root_revision(&sim), "");
    let source = home_source(&sim);
    assert_eq!((source.full_read, source.failing.len()), (1_791_900_000, 1));
    let quiet = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(quiet.summary.as_ref().unwrap().listed, Listed::NotModified);

    // Changed on the server, it is asked for again, and goes in.
    {
        let mut lib = fake.lock().unwrap();
        lib.broken.clear();
        let track = lib.tracks.iter_mut().find(|t| t.path == bad).unwrap();
        track.bytes = corpus("cbr.mp3");
        lib.revision = "1:5:1790000000005:5".into();
    }
    let mended = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    let outcome = ran(&mended);
    assert_eq!((outcome.written, outcome.records, outcome.missed.len()), (1, 4, 0));
    assert!(home_source(&sim).failing.is_empty());
}

#[test]
fn a_server_that_cant_convert_now_leaves_the_cards_conversions_where_they_are() {
    let scratch = Scratch::new("no-transcode");
    let sim = card(&scratch);
    let (base, fake) = server();
    let mut envs = envs();
    ran(&sync_now(&sim, &base, &rock(), &mut envs).0);
    let converted = scratch.0.join("card/music/Artist C/Album D/01.opus");
    assert!(converted.exists());

    // A whole read under a changed setting (no row stands for its entry by
    // the plan) while its ffmpeg is still starting: the WAV can't be
    // converted, and the card's conversion of it stays, the listing's
    // revision not stored (the next run reads it whole).
    fake.lock().unwrap().no_transcode = true;
    let change = Change { bitrate: Some(TransferBitrate::K128), ..Change::default() };
    let whole = Ask { whole: true, change, ..Ask::default() };
    let synced = sync_now(&sim, &base, &whole, &mut envs).0;
    let summary = synced.summary.as_ref().unwrap();
    let wav = "Rock/Artist C/Album D/01.wav".to_string();
    assert_eq!(summary.skipped, [(wav, Skip::NoTranscode)]);
    assert!(summary.removals.is_empty(), "{summary:?}");
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.records, outcome.verified.clone()), (0, 4, Ok(())));
    assert!(converted.exists());
    assert_eq!(root_revision(&sim), "");
    fake.lock().unwrap().no_transcode = false;
    let again = sync_now(&sim, &base, &Ask::default(), &mut envs).0;
    assert_eq!(again.summary.as_ref().unwrap().listed, Listed::Whole);
    assert!(again.summary.as_ref().unwrap().skipped.is_empty());
    assert_ne!(root_revision(&sim), "");
}

#[test]
fn an_ogg_whose_first_bytes_dont_come_is_passed_over_never_converted_on_a_guess() {
    let scratch = Scratch::new("unsniffed");
    let sim = card(&scratch);
    let (base, fake) = server();
    let ogg = "Rock/Artist C/Album D/03.ogg";
    {
        let mut lib = fake.lock().unwrap();
        let like = lib.tracks[3].clone();
        lib.tracks.push(Track { path: ogg.into(), id: 6, bytes: opus_bytes(1000, 2, true), ..like });
        lib.revision = "1:6:1790000000006:5".into();
        lib.unranged.insert(ogg.into());
    }
    let synced = sync_now(&sim, &base, &rock(), &mut envs()).0;
    let summary = synced.summary.as_ref().unwrap();
    assert_eq!(summary.skipped, [(ogg.to_string(), Skip::Unread)]);
    let lib = fake.lock().unwrap();
    let asked = lib.asked.iter().filter(|a| a.ends_with(ogg)).count();
    assert_eq!(asked, 2, "asked twice: {:?}", lib.asked);
    assert!(lib.transcodes.iter().all(|q| !q.contains("03.ogg")));
    drop(lib);
    let outcome = ran(&synced);
    assert_eq!((outcome.records, outcome.verified.clone()), (4, Ok(())));
    assert_eq!(root_revision(&sim), "", "the next run asks again");
}

#[test]
fn a_listing_with_no_sizes_is_copied_whole_and_read_again_as_the_same() {
    let scratch = Scratch::new("no-sizes");
    let sim = card(&scratch);
    let (base, fake) = server();
    fake.lock().unwrap().no_sizes = true;
    let mut envs = envs();
    let first = sync_now(&sim, &base, &rock(), &mut envs).0;
    // The fit takes each at its length and bitrate, not at nothing.
    assert!(first.summary.as_ref().unwrap().fetch_bytes > 1_000_000);
    let outcome = ran(&first);
    let done = (outcome.written, outcome.records, outcome.verified.clone(), outcome.missed.len());
    assert_eq!(done, (4, 4, Ok(()), 0), "{:?}", outcome.report);
    let mp3 = std::fs::read(scratch.0.join("card/music/Artist A/Album B/01.mp3")).unwrap();
    assert_eq!(mp3, corpus("v24_full.mp3"));
    // Its rows say no size, and the next whole read finds them unchanged.
    let whole = Ask { whole: true, ..Ask::default() };
    let again = sync_now(&sim, &base, &whole, &mut envs).0;
    assert_eq!((ran(&again).written, ran(&again).deleted), (0, 0));
}

#[test]
fn a_whole_read_is_due_after_seven_days_or_when_the_last_was_in_the_future() {
    let now = 1_791_900_000;
    assert!(!whole_due(now - 60, now));
    assert!(whole_due(now - WHOLE_EVERY_SECS, now));
    assert!(whole_due(now + 3600, now), "this computer's clock was ahead then");
}

/// One sync of `card` from `base` whose events go to `said` as they come.
fn sync_saying(card: &FatSim, base: &str, ask: &Ask, said: &mut dyn FnMut(Event)) -> Synced {
    let client = Arc::new(Client::new(base).unwrap().with_token(Some("tok".into())));
    let (fed, space, plays) = (no_peers(), space(32 << 30), Rc::new(MemStore::default()));
    let mut cards = Cards::default();
    sync(card, space, client, Reached::Elsewhere, ask, &mut cards, plays, &fed, &mut envs(), said)
}

#[test]
fn a_stop_before_the_run_writes_nothing_and_one_under_way_parks_at_a_checkpoint() {
    let scratch = Scratch::new("stopped");
    let sim = card(&scratch);
    let (base, _fake) = server();
    let mut before = sim.image();
    before.retain(|p, _| !p.ends_with("run.lock"));

    // Stopped before the run: the plan is made and nothing is written.
    let stop = Arc::new(AtomicBool::new(true));
    let ask = Ask { stop: Some(stop.clone()), ..rock() };
    let halted = sync_saying(&sim, &base, &ask, &mut |_| {});
    assert!(matches!(halted.end, End::Halted(Halt::Stopped)), "{:?}", halted.end);
    let fetches = halted.summary.as_ref().map(|s| s.fetches);
    assert_eq!(fetches, Some(4), "the plan said what it would do");
    let mut after = sim.image();
    after.retain(|p, _| !p.ends_with("run.lock"));
    assert_eq!(after, before, "nothing reached the card");

    // Stopped once the first file is in: it parks at a checkpoint, the
    // plan on the card, and says so.
    stop.store(false, Ordering::SeqCst);
    let mut seen: Vec<Event> = Vec::new();
    let mut said = |e: Event| {
        if matches!(e, Event::Wrote { .. }) {
            stop.store(true, Ordering::SeqCst);
        }
        if !matches!(e, Event::Progress { .. }) {
            seen.push(e);
        }
    };
    let parked = sync_saying(&sim, &base, &ask, &mut said);
    let outcome = ran(&parked);
    assert_eq!(outcome.parked.as_deref(), Some(STOPPED));
    assert_eq!((outcome.written, outcome.records), (1, 1), "the file in flight, and no more");
    let planned = seen.iter().position(|e| matches!(e, Event::Planned { files: 4, .. }));
    let listed = seen.iter().position(|e| matches!(e, Event::Listed));
    let wrote = seen.iter().position(|e| matches!(e, Event::Wrote { .. }));
    assert!(listed < planned && planned < wrote && listed.is_some(), "{seen:?}");
    assert!(seen.iter().any(|e| matches!(e, Event::Committed { last: false, .. })), "a checkpoint");
    assert!(volume::inspect(&sim).unwrap().pending.is_some(), "the plan stays on the card");
    let summary = parked.summary.as_ref().unwrap();
    assert_eq!(summary.offered, ["Rock", "Jazz"]);
    let on_rock: Vec<&(String, usize, u64)> =
        summary.totals.iter().filter(|t| t.0 == "Rock").collect();
    assert_eq!(on_rock.len(), 1);
    assert_eq!(on_rock[0].1, 4, "Rock's four files on the server");

    // The next sync, unstopped, goes on from there and finishes.
    let again = sync_saying(&sim, &base, &Ask { stop: None, ..rock() }, &mut |_| {});
    let outcome = ran(&again);
    let ended = (outcome.parked.clone(), outcome.verified.clone(), outcome.records);
    assert_eq!(ended, (None, Ok(()), 4));
    assert!(look(&sim).pending.is_none());
}

/// A stop under way asks for no album's cover: the thumbnail step keeps
/// what the card has, and the next run makes them.
#[test]
fn a_stop_under_way_asks_for_no_cover() {
    let scratch = Scratch::new("stopped-covers");
    let sim = card(&scratch);
    let (base, fake) = server();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ask = Ask { stop: Some(stop.clone()), ..rock() };
    let mut said = |e: Event| {
        if matches!(e, Event::Wrote { .. }) {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let parked = sync_saying(&sim, &base, &ask, &mut said);
    assert_eq!(ran(&parked).parked.as_deref(), Some(STOPPED));
    let asked = fake.lock().unwrap().asked.clone();
    let covers = asked.iter().filter(|a| a.contains("cover.jpg") || a.contains("/album-art/"));
    assert_eq!(covers.collect::<Vec<_>>(), Vec::<&String>::new());
    let state = State::of(&look(&sim).state).unwrap();
    assert!(state.settings.art.is_empty(), "no thumbnail's art: {:?}", state.settings.art);

    // The next run makes them.
    let again = sync_saying(&sim, &base, &Ask { stop: None, ..rock() }, &mut |_| {});
    assert_eq!(ran(&again).verified, Ok(()));
    assert_eq!(State::of(&look(&sim).state).unwrap().settings.art.len(), 2);
}

/// A stop while a conversion waits for the server's full queue ends the
/// wait: the run parks at a checkpoint, the file in its plan, and nothing of
/// it is remembered as a file that failed.
#[test]
fn a_stop_while_a_conversion_waits_for_a_full_queue_parks_and_fails_no_file() {
    let scratch = Scratch::new("queued");
    let sim = card(&scratch);
    let (base, fake) = server();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let mut lib = fake.lock().unwrap();
        lib.transcode = Some(json!({"defaultCodec": "opus", "defaultBitrate": "96k", "bulk": {"concurrency": 1}}));
        lib.queue_full = Some(stop.clone());
    }
    let ask = Ask { stop: Some(stop.clone()), ..rock() };
    let parked = sync_saying(&sim, &base, &ask, &mut |_| {});
    let outcome = ran(&parked);
    assert_eq!(outcome.parked.as_deref(), Some(STOPPED), "{:?}", outcome.report);
    assert!(outcome.missed.is_empty() && outcome.report.failed.is_empty(), "{:?}", outcome.report);
    assert!(look(&sim).pending.is_some(), "the conversion stays in the plan on the card");
    assert!(home_source(&sim).failing.is_empty(), "a stop is no file's failure");

    // The queue has room again: the next run goes on and finishes.
    fake.lock().unwrap().queue_full = None;
    let again = sync_saying(&sim, &base, &Ask { stop: None, ..rock() }, &mut |_| {});
    let outcome = ran(&again);
    assert_eq!((outcome.parked.clone(), outcome.verified.clone(), outcome.records), (None, Ok(()), 4));
    assert!(home_source(&sim).failing.is_empty());
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

#[test]
fn the_hidden_commands_rate_line_says_where_the_copying_stands() {
    let t0 = std::time::Instant::now();
    let at = |secs: u64| t0 + Duration::from_secs(secs);
    let wrote = |bytes: u64| Event::Wrote { path: "Rock/a.mp3".into(), bytes };
    let mut rate = Rate::default();
    assert_eq!(rate.heard(&Event::Planned { files: 4, bytes: 400_000_000_000 }, at(0)), None);
    assert_eq!(rate.heard(&wrote(300_000_000), at(10)), None, "not 30 s yet");
    let line = rate.heard(&wrote(300_000_000), at(30));
    let want = "rate: 2 of 4 files, 0.6 of 400.0 GB; 20.0 MB/s the last 30 s, 20.0 MB/s in all; \
                about 5 h 32 min to go";
    assert_eq!(line.as_deref(), Some(want));
    let failed = Event::Failed { server_path: "Rock/b.mp3".into(), why: "gone".into() };
    assert_eq!(rate.heard(&failed, at(40)), None);
    assert_eq!(rate.heard(&Event::Listed, at(80)), None);
    // The last file in: a line at once, with no time to go.
    let line = rate.heard(&wrote(1_000_000_000), at(90));
    let want = "rate: 4 of 4 files, 1.6 of 400.0 GB; 16.7 MB/s the last 60 s, 17.8 MB/s in all";
    assert_eq!(line.as_deref(), Some(want));
}

mod bench;
mod moods;
mod peers;
mod span;
mod waves;
