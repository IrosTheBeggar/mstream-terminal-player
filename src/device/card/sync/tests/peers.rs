//! A sync's federation peers, end to end (B9): the fake server of
//! [`super`] as their parent — its peer list, its access route, and its
//! browse, stream and art proxies (an older one's passing on neither the
//! copy purpose nor the peer's Retry-After) — and each peer a loopback
//! server of its own for the direct guest path, which a test dial opens in
//! place of iroh. Made-up libraries, a host folder as the card, a config
//! folder of the test's own for `budget.json`, a clock the test turns.

use std::collections::{BTreeSet, VecDeque};

use super::*;
use crate::api::transfer::stub::Asked;
use crate::device::card::budget::{Budget, PeerKey, next_midnight};
use crate::device::card::contract::server::server_url_key;
use crate::device::card::source::ffmpeg::Convert;
use crate::device::card::source::peer::{Dial, Reach, Tunnel};
use crate::device::card::sync::peers::{Ffmpeg, Status, proxy_says_copies};

// ── A peer ──────────────────────────────────────────────────────────────────

/// A 429 a peer answers a file's GET with, once.
#[derive(Clone)]
pub(super) struct Limit {
    words: &'static str,
    retry_after: Option<u64>,
}

#[derive(Default)]
pub(super) struct PeerLib {
    vpaths: Vec<String>,
    tracks: Vec<Track>,
    art: HashMap<String, Vec<u8>>,
    /// The key's allowance as the peer shows it; None: an older peer.
    allowance: Option<Value>,
    instance: Option<&'static str>,
    /// The key forbids copies.
    copies_off: bool,
    /// The 429s its files' GETs are answered with, in turn.
    limits: VecDeque<Limit>,
    /// Every request, as "via METHOD target": `direct`, or `parent` for one
    /// its parent's proxies forwarded.
    asked: Vec<String>,
    /// Each path `metadata/batch` was asked for.
    batched: Vec<String>,
    /// Each track GET's X-mStream-Purpose and Range, as they came.
    purposes: Vec<Option<String>>,
    ranges: Vec<Option<String>>,
    /// Each other GET under `/media/` (a cover's probe or read): its Range.
    probes: Vec<Option<String>>,
    /// The bytes of its files it served.
    served: u64,
    /// Files its folders hold that its database doesn't have yet.
    unscanned: Vec<String>,
    /// Its folders' images, by path: never listed, served on a GET.
    images: HashMap<String, Vec<u8>>,
    /// A folder made here at its first track's GET: `budget.json`'s
    /// temporary file's place taken, so the book can't be written.
    poison: Option<PathBuf>,
}

pub(super) type PeerFake = Arc<Mutex<PeerLib>>;

/// A peer as its parent lists it, and whether it offers the direct path.
#[derive(Clone)]
pub(super) struct FedPeer {
    id: u32,
    name: &'static str,
    endpoint: &'static str,
    lib: PeerFake,
    direct: bool,
}

/// The peer's own id.
const PEER_ID: &str = "5d0c2b9a-1f3e-4a6b-8c7d-9e0f1a2b3c4d";

fn peer_meta(t: &Track) -> Value {
    let mut m = t.full.clone();
    let audio = md5(&[b"audio:".as_slice(), &t.bytes].concat());
    m["duration"] = json!(t.seconds);
    m["album-art"] = json!(t.art);
    m["file-size"] = json!(t.bytes.len());
    m["modified"] = json!(1_790_000_000_000i64 + t.id);
    m["hash"] = json!(hex(&md5(&t.bytes)));
    m["audio-hash"] = json!(hex(&audio));
    m["created-at"] = json!("2026-10-01 12:00:00");
    m
}

/// The bytes of an answer past its head.
fn body_len(answer: &[u8]) -> u64 {
    answer.windows(4).position(|w| w == b"\r\n\r\n").map_or(0, |at| (answer.len() - at - 4) as u64)
}

fn peer_answer(fake: &PeerFake, asked: &Asked, via: &str) -> Vec<u8> {
    let mut lib = fake.lock().unwrap();
    let path = asked.path();
    lib.asked.push(format!("{via} {} {}", asked.method, asked.target));
    let not_found = || json_reply("404 Not Found", r#"{"error":"Not Found"}"#);
    let under = |dir: &str| format!("{}/", dir.trim_matches('/'));
    match (asked.method.as_str(), path.as_str()) {
        ("GET", "/api/" | "/api") => {
            let mut body = json!({
                "server": "6.32.0",
                "features": {"transcode": false, "discovery": true},
                "user": {"vpaths": lib.vpaths, "federation": true},
            });
            if let Some(id) = lib.instance {
                body["instanceId"] = json!(id);
            }
            if let Some(allowance) = &lib.allowance {
                body["user"]["federationAllowance"] = allowance.clone();
            }
            json_reply("200 OK", &body.to_string())
        }
        ("POST", "/api/v1/file-explorer") => {
            let prefix = under(asked.json()["directory"].as_str().unwrap_or(""));
            let (mut dirs, mut files) = (BTreeSet::new(), Vec::new());
            for t in lib.tracks.iter().filter(|t| t.path.starts_with(&prefix)) {
                match t.path[prefix.len()..].split_once('/') {
                    Some((dir, _)) => {
                        dirs.insert(dir.to_string());
                    }
                    None => files.push(json!({"name": &t.path[prefix.len()..], "type": "mp3"})),
                }
            }
            let dirs: Vec<Value> = dirs.into_iter().map(|d| json!({"name": d})).collect();
            let body = json!({"path": prefix, "directories": dirs, "files": files});
            json_reply("200 OK", &body.to_string())
        }
        ("POST", "/api/v1/file-explorer/recursive") => {
            let prefix = under(asked.json()["directory"].as_str().unwrap_or(""));
            let paths = lib.tracks.iter().map(|t| &t.path).chain(&lib.unscanned);
            let paths: Vec<&String> = paths.filter(|p| p.starts_with(&prefix)).collect();
            json_reply("200 OK", &json!(paths).to_string())
        }
        ("POST", "/api/v1/db/metadata/batch") => {
            let paths: Vec<String> = serde_json::from_value(asked.json()).unwrap_or_default();
            lib.batched.extend(paths.iter().cloned());
            let mut out = serde_json::Map::new();
            for p in paths {
                let row = |t: &Track| json!({"filepath": p, "metadata": peer_meta(t)});
                let row = lib.tracks.iter().find(|t| t.path == p).map(row);
                out.insert(p, row.unwrap_or(Value::Null));
            }
            json_reply("200 OK", &Value::Object(out).to_string())
        }
        ("POST", "/api/v1/discovery/local/embeddings") => {
            let paths = serde_json::from_value(asked.json()["filePaths"].clone());
            let paths: Vec<String> = paths.unwrap_or_default();
            let row = |p: &String| json!({"filePath": p, "notAnalyzed": false, "embedding": vector(p)});
            let tracks: Vec<Value> = paths.iter().map(row).collect();
            let model = json!({"id": "effnet-discogs", "version": "1"});
            let body = json!({"model": model, "dim": 8, "tracks": tracks});
            json_reply("200 OK", &body.to_string())
        }
        // mStream's federation gate (`federation-auth.js`): `/media/` is
        // for GETs alone, a key's and a guest's alike.
        ("HEAD", p) if p.starts_with("/media/") => {
            json_reply("403 Forbidden", r#"{"error":"Forbidden"}"#)
        }
        ("GET", p) if p.starts_with("/media/") => {
            let rel = p["/media/".len()..].to_string();
            let purpose = asked.header("x-mstream-purpose").map(str::to_string);
            let range = asked.header("range").map(str::to_string);
            let track = lib.tracks.iter().find(|t| t.path == rel).map(|t| t.bytes.clone());
            match track {
                Some(_) => {
                    lib.purposes.push(purpose.clone());
                    lib.ranges.push(range);
                    if let Some(dir) = lib.poison.take() {
                        std::fs::create_dir_all(dir).unwrap();
                    }
                }
                None => lib.probes.push(range),
            }
            if lib.copies_off && purpose.as_deref() == Some("copy") {
                return json_reply("403 Forbidden", r#"{"error":"copies are not allowed with this key"}"#);
            }
            if track.is_some()
                && let Some(limit) = lib.limits.pop_front()
            {
                let body = json!({"error": limit.words}).to_string();
                let after = limit.retry_after.map(|s| s.to_string());
                let headers: Vec<(&str, &str)> =
                    after.iter().map(|a| ("Retry-After", a.as_str())).collect();
                return reply("429 Too Many Requests", &headers, body.as_bytes());
            }
            let Some(bytes) = track.or_else(|| lib.images.get(&rel).cloned()) else {
                return not_found();
            };
            let out = media(asked, &bytes);
            lib.served += body_len(&out);
            out
        }
        ("GET", p) if p.starts_with("/album-art/") => match lib.art.get(&p["/album-art/".len()..]) {
            Some(bytes) => reply("200 OK", &[("Content-Type", "image/png")], bytes),
            None => not_found(),
        },
        _ => not_found(),
    }
}

/// The request with only `names` of its headers, at another target.
fn forwarded(asked: &Asked, method: &str, target: String, names: &[&str]) -> Asked {
    let headers = asked.headers.iter().filter(|(n, _)| names.contains(&n.as_str())).cloned().collect();
    Asked { method: method.into(), target, headers, body: asked.body.clone() }
}

/// An answer without one of its headers, as an older proxy copies back
/// only the six it knows.
fn without(answer: Vec<u8>, name: &str) -> Vec<u8> {
    let Some(at) = answer.windows(4).position(|w| w == b"\r\n\r\n") else { return answer };
    let head = String::from_utf8_lossy(&answer[..at]).to_string();
    let other = |l: &&str| !l.to_ascii_lowercase().starts_with(&format!("{name}:"));
    let kept: Vec<&str> = head.lines().filter(other).collect();
    let mut out = kept.join("\r\n").into_bytes();
    out.extend_from_slice(&answer[at..]);
    out
}

/// The parent's federation routes (mStream's `federation-browse.js` and
/// `federation-stream.js`), `old` an mStream older than the card
/// transfer's batch of changes.
pub(super) fn federation(peers: &[FedPeer], old: bool, asked: &Asked) -> Vec<u8> {
    if asked.path() == "/api/v1/federation/peers" {
        let row = |p: &FedPeer| {
            json!({"id": p.id, "name": p.name, "lastSeen": null, "lastStatus": "ok",
                   "useDiscovery": false, "endpointId": p.endpoint, "direct": p.direct})
        };
        let rows: Vec<Value> = peers.iter().map(row).collect();
        return json_reply("200 OK", &json!({ "peers": rows }).to_string());
    }
    let refused = || json_reply("404 Not Found", r#"{"error":"Peer not found"}"#);
    let Some(rest) = asked.target.strip_prefix("/api/v1/federation/peers/") else { return refused() };
    let (id, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let Some(peer) = peers.iter().find(|p| p.id.to_string() == id) else { return refused() };
    if rest == "access" || rest.starts_with("access?") {
        if !peer.direct {
            let reason = "peer does not offer guest access (an older build)";
            let body = json!({"direct": false, "reason": reason});
            return json_reply("200 OK", &body.to_string());
        }
        let body = json!({
            "direct": true, "endpointTicket": "endpoint", "endpointId": peer.endpoint,
            "guestToken": format!("guest.{id}.token"), "expiresAt": "2026-10-19T13:20:00.000Z",
            "directTicket": format!("mstrfedg1:{id}"),
        });
        return json_reply("200 OK", &body.to_string());
    }
    if let Some(api) = rest.strip_prefix("api/") {
        let fwd = forwarded(asked, &asked.method, format!("/{api}"), &["content-type", "content-length"]);
        return peer_answer(&peer.lib, &fwd, "parent");
    }
    if let Some(file) = rest.strip_prefix("stream/") {
        let names: &[&str] =
            if old { &["range", "if-range"] } else { &["range", "if-range", "x-mstream-purpose"] };
        // Registered for GET only: Express answers a HEAD with the GET.
        let fwd = forwarded(asked, "GET", format!("/media/{file}"), names);
        let answer = peer_answer(&peer.lib, &fwd, "parent");
        return if old { without(answer, "retry-after") } else { answer };
    }
    if let Some(file) = rest.strip_prefix("art/") {
        let fwd = forwarded(asked, "GET", format!("/album-art/{file}"), &[]);
        return peer_answer(&peer.lib, &fwd, "parent");
    }
    refused()
}

// ── The scene ───────────────────────────────────────────────────────────────

/// 2026-10-18 13:20 UTC: the budget's clock.
const NOW: u64 = 1_792_281_600 + 13 * 3600 + 1200;
const JITTER: u64 = 120;
const ALBUM: &str = "Folk/Dana Duo/Album";
const ONE: &str = "Folk/Dana Duo/Album/01.mp3";
const TWO: &str = "Folk/Dana Duo/Album/02.flac";

fn peer_track(path: &str, id: i64, bytes: Vec<u8>) -> Track {
    let full = json!({"sample-rate": 44100, "channels": 2, "bitrate": 320000, "bpm-source": "tag"});
    Track { path: path.into(), id, bytes, seconds: 181.5, art: Some("f.png".into()), full }
}

/// A peer's library, Folk: an album of an MP3 and a FLAC, with mStream's
/// art.
fn folk() -> Vec<Track> {
    vec![peer_track(ONE, 1, corpus("v24_full.mp3")), peer_track(TWO, 2, corpus("flac_full.flac"))]
}

/// The direct guest path's tunnels: each peer's own loopback server.
struct TestDial(HashMap<String, String>);

impl Dial for TestDial {
    fn dial(&self, ticket: &str) -> Result<Tunnel, String> {
        let base = self.0.get(ticket).ok_or("no such tunnel")?;
        Ok(Tunnel { base: base.clone(), local_token: None, relay: false, _keep: None })
    }
}

/// A conversion on this computer that makes a made-up Opus stream of the
/// source's length, and remembers each source's size.
#[derive(Default)]
struct MadeUp(Mutex<Vec<u64>>);

impl Convert for MadeUp {
    fn convert(
        &self,
        from: &Path,
        to: &Path,
        _: TransferCodec,
        _: TransferBitrate,
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        let size = std::fs::metadata(from).map_err(|e| e.to_string())?.len();
        self.0.lock().unwrap().push(size);
        std::fs::write(to, opus_bytes(181_500, 2, true)).map_err(|e| e.to_string())
    }

    fn found(&self) -> &'static str {
        "path"
    }
}

/// The parent (the fake server of two libraries, an older mStream when
/// `old`), its peer 3 serving `tracks` under Folk, offering the direct
/// path when `direct`; the dial to its tunnel.
fn world(direct: bool, old: bool, tracks: Vec<Track>) -> (String, Fake, PeerFake, TestDial) {
    let (base, fake) = server();
    let lib = PeerLib {
        vpaths: vec!["Folk".into()],
        tracks,
        art: HashMap::from([("f.png".to_string(), picture(80, 60, false))]),
        instance: Some(PEER_ID),
        ..PeerLib::default()
    };
    let peer = Arc::new(Mutex::new(lib));
    let served = peer.clone();
    let tunnel = routed(move |asked| peer_answer(&served, asked, "direct"));
    let mut parent = fake.lock().unwrap();
    parent.old = old;
    parent.peers.push(FedPeer { id: 3, name: "Attic", endpoint: "e3", lib: peer.clone(), direct });
    drop(parent);
    (base, fake, peer, TestDial(HashMap::from([("mstrfedg1:3".to_string(), tunnel)])))
}

fn fed(scratch: &Scratch, clock: &Arc<AtomicU64>, dial: TestDial, ffmpeg: Ffmpeg) -> Federation {
    let at = clock.clone();
    Federation {
        budget: Budget::at(scratch.0.join("config")),
        dial: Box::new(dial),
        ffmpeg,
        cards: Vec::new(),
        clock: Arc::new(move || at.load(Ordering::Relaxed)),
        jitter: Arc::new(|| JITTER),
        stream_wait: Duration::from_millis(20),
    }
}

fn sync_fed(
    card: &FatSim,
    base: &str,
    ask: &Ask,
    fed: &Federation,
    envs: &mut dyn FnMut() -> Env,
) -> Synced {
    let client = Arc::new(Client::new(base).unwrap().with_token(Some("tok".into())));
    let plays: Rc<dyn history::Store> = Rc::new(MemStore::default());
    let mut cards = Cards::default();
    sync(card, space(32 << 30), client, Reached::Elsewhere, ask, &mut cards, plays, fed, envs, &mut |_| {})
}

/// The server's Rock, and its peer 3's every library.
fn rock_and_folk() -> Ask {
    Ask { peers: Some(vec![PeerAsk { id: 3, libraries: None }]), ..rock() }
}

fn key_of(base: &str) -> PeerKey {
    PeerKey { parent_key: server_url_key(base), peer_id: 3 }
}

fn asked_of(peer: &PeerFake) -> Vec<String> {
    peer.lock().unwrap().asked.clone()
}

// ── The tests ───────────────────────────────────────────────────────────────

/// A peer read beside the server: its listing through the parent's browse
/// proxy, its files over its own tunnel each saying it is a copy, its
/// folder's cover found there with a one-byte GET (its gate refuses a HEAD)
/// and read; its rows on the card name it (an SRVS peer entry, its own id),
/// every byte read from it is in the budget's book, and the card's settings
/// know it by its parent's key, its id and its identity.
#[test]
fn a_peers_files_go_on_the_card_beside_the_servers_and_every_byte_is_counted() {
    let scratch = Scratch::new("peer-direct");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
    peer.lock().unwrap().images.insert(format!("{ALBUM}/cover.jpg"), picture(64, 48, true));
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.parked.clone()), (Ok(()), None), "{:?}", synced.notes);
    assert_eq!((outcome.written, outcome.records), (6, 6), "{:?}", outcome.report);
    let summary = &synced.peers[0];
    let read = (summary.status.clone(), summary.reach, summary.files, summary.fetches);
    assert_eq!(read, (Status::Read, Some(Reach::Direct), 2, 2));
    assert_eq!(summary.days, 1, "well inside a day's budget");

    // The card: the peer's rows name it, by its parent's key and its own id.
    let seen = look(&sim);
    let root = seen.root.as_ref().unwrap();
    let srvs = &root.manifest.servers;
    assert_eq!(srvs.len(), 1, "{srvs:?}");
    assert!(srvs[0].is_peer() && srvs[0].peer_id == 3 && srvs[0].parent_key == server_url_key(&base));
    assert_eq!(hex(&srvs[0].instance), PEER_ID.replace('-', ""));
    let records = &root.tags.as_ref().unwrap().records;
    let of_peer = |e: &&Entry| e.ledger.as_ref().is_some_and(|l| l.server == 1);
    let peer_rows: Vec<&str> = records.iter().filter(of_peer).map(|e| e.path.as_str()).collect();
    assert_eq!(peer_rows, ["Folk/Dana Duo/Album/01.mp3", "Folk/Dana Duo/Album/02.flac"]);
    assert!(root.manifest.autodj.is_some(), "the peer's rows have their vectors too: {:?}", synced.notes);

    // How it was read.
    let asked = asked_of(&peer);
    let listings = ["GET /api/", "POST /api/v1/file-explorer", "POST /api/v1/db/metadata/batch"];
    for listing in listings.map(|l| format!("parent {l}")) {
        assert!(asked.iter().any(|a| a.starts_with(&listing)), "{listing}: {asked:?}");
    }
    assert!(asked.contains(&"parent POST /api/v1/file-explorer/recursive".to_string()), "{asked:?}");
    assert!(asked.iter().any(|a| a == "direct GET /media/Folk/Dana%20Duo/Album/01.mp3"), "{asked:?}");
    let proxied = asked.iter().any(|a| a.starts_with("parent GET /media/"));
    assert!(!proxied, "no byte through the parent: {asked:?}");
    let cover = "direct GET /media/Folk/Dana%20Duo/Album/cover.jpg";
    assert_eq!(asked.iter().filter(|a| *a == cover).count(), 2, "probed, then read: {asked:?}");
    assert!(!asked.iter().any(|a| a.starts_with("direct HEAD ")), "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains("/album-art/")), "its folder's cover first: {asked:?}");
    let thumb = std::fs::read(scratch.0.join("card/.mstream").join(thumb_path_of(ALBUM)));
    assert!(thumb.is_ok_and(|t| mpth::check(&t, path_hash(ALBUM))), "its thumbnail");
    let lib = peer.lock().unwrap();
    assert!(lib.purposes.iter().all(|p| p.as_deref() == Some("copy")), "{:?}", lib.purposes);
    assert_eq!(lib.probes.first().cloned().flatten().as_deref(), Some("bytes=0-0"), "{:?}", lib.probes);
    let served = lib.served;
    drop(lib);

    // Every byte read from it, counted on its day.
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    let used = book.used_today(key_of(&base), NOW);
    assert!(used > served && served > 0, "{used} counted, {served} of files");
    let text = std::fs::read_to_string(scratch.0.join("config/budget.json")).unwrap();
    assert!(!text.contains("127.0.0.1") && !text.contains("Attic"), "no address, no name: {text}");

    // The card's settings know the peer, and a run that names none reads it again.
    let state = State::of(&seen.state).unwrap();
    let p = &state.settings.peers[0];
    assert_eq!((p.parent_key, p.peer_id, p.endpoint.as_str()), (server_url_key(&base), 3, "e3"));
    assert_eq!((p.nickname.as_str(), p.libraries.as_slice()), ("Attic", ["Folk".to_string()].as_slice()));
    let again = sync_fed(&sim, &base, &rock(), &fed, &mut envs);
    assert_eq!(again.peers[0].status, Status::Read);
    assert_eq!(ran(&again).written, 0);
    let printed = synced_json(&again, false);
    assert_eq!(printed["peers"][0]["status"], "read");
    assert_eq!(printed["peers"][0]["reach"], "direct");
}

/// A peer's files no server serves (a Mac's AppleDouble twin beside each
/// of its files) are left out of its listing before it is asked anything
/// of them, its metadata included; the run counts them with the server's
/// in its one note, and ends clean.
#[test]
fn a_peers_hidden_files_are_left_out_before_it_is_asked_about_them() {
    let scratch = Scratch::new("peer-hidden");
    let sim = card(&scratch);
    let mut tracks = folk();
    let twins = [format!("{ALBUM}/._01.mp3"), format!("{ALBUM}/._02.flac")];
    for (i, twin) in twins.iter().enumerate() {
        tracks.push(peer_track(twin, 3 + i as i64, b"\0\x05\x16\x07 a made-up AppleDouble".to_vec()));
    }
    let (base, fake, peer, dial) = world(true, false, tracks);
    let server_hidden = super::with_hidden(&fake);
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs());
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.parked.clone()), (Ok(()), None), "{:?}", synced.notes);
    assert_eq!((outcome.written, outcome.records), (7, 7), "{:?}", outcome.report);
    assert!(outcome.report.failed.is_empty(), "{:?}", outcome.report);
    let summary = &synced.peers[0];
    assert_eq!((summary.status.clone(), summary.files, summary.hidden), (Status::Read, 2, 2));
    assert_eq!(synced.summary.as_ref().map(|s| s.hidden), Some(2), "the server's own");
    let hidden: Vec<&String> = synced.notes.iter().filter(|n| n.contains("hidden")).collect();
    assert_eq!(hidden, [&plan::hidden_note(4)], "one note for the server's and the peer's");
    assert_eq!(synced_json(&synced, false)["peers"][0]["hidden"], 2);

    // The peer was never asked about them: no metadata, no byte.
    let lib = peer.lock().unwrap();
    assert!(!lib.asked.iter().any(|a| a.contains("._0")), "{:?}", lib.asked);
    assert!(!lib.batched.iter().any(|p| twins.contains(p)), "{:?}", lib.batched);
    assert!(lib.batched.contains(&ONE.to_string()), "the rest were: {:?}", lib.batched);
    drop(lib);
    super::never_asked(&fake, &server_hidden);
}

/// A peer whose key forbids copies refuses the first one with a 403: the
/// transfer stops for that peer and says why, and the server's own files
/// go on. A key whose allowance says so is never even listed.
#[test]
fn a_peer_that_refuses_copies_is_stopped_and_says_why() {
    let scratch = Scratch::new("peer-copies");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
    peer.lock().unwrap().copies_off = true;
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.parked.clone(), outcome.records), (Ok(()), None, 4));
    let why = "the peer's owner doesn't let this server's key copy its files";
    let said = &synced.peers[0];
    assert_eq!((said.status.clone(), said.why.as_deref()), (Status::Refused, Some(why)));
    let failed = outcome.report.failed.iter().filter(|(_, w)| w == why).map(|(p, _)| p.as_str());
    let failed: Vec<&str> = failed.collect();
    assert_eq!(failed, [ONE, TWO], "{:?}", outcome.report.failed);
    assert_eq!(peer.lock().unwrap().purposes.len(), 1, "asked once: the next is skipped unasked");
    assert_eq!(synced.resume_at, None, "nothing to wait for");

    // Its allowance says so: nothing listed.
    let allowance = json!({"dailyMb": 2048, "remainingTodayBytes": 1 << 30, "allowCopies": false});
    peer.lock().unwrap().allowance = Some(allowance);
    let before = asked_of(&peer).len();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(synced.peers[0].status, Status::Refused);
    let after: Vec<String> = asked_of(&peer)[before..].to_vec();
    assert_eq!(after, ["parent GET /api/"], "its /api/ alone");
}

/// A peer only an older parent's proxy reaches (it offers no direct path,
/// and the proxy can't say a copy is one) is asked about and skipped; once
/// the listener allows it, it is read through the proxies — one-byte probes,
/// never a HEAD there, the art's 256 px copy — and the card keeps the answer.
/// A parent of the card transfer's batch passes the copy purpose on, and is
/// read without asking: here one of the batch's own builds from before
/// `features.federationProxy`, known by its `instanceId` at 6.31.1.
#[test]
fn a_peer_only_the_proxy_reaches_is_asked_about_once() {
    let scratch = Scratch::new("peer-proxy");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = world(false, true, folk());
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(ran(&synced).records, 4, "the server's own go in");
    assert_eq!(synced.peers[0].status, Status::AskProxy);
    assert!(asked_of(&peer).is_empty(), "nothing asked of the peer");
    let endpoint = |sim: &FatSim| endpoint_of(&look(sim).state);
    assert_eq!(endpoint(&sim), "", "never read: known by no identity yet");

    // Paired again meanwhile: read all the same, by its new identity.
    fake.lock().unwrap().peers[0].endpoint = "e9";
    let allowed = Ask { proxy_copies: vec![3], ..rock_and_folk() };
    let synced = sync_fed(&sim, &base, &allowed, &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.written), (Ok(()), 2), "{:?}", synced.notes);
    assert_eq!(synced.peers[0].reach, Some(Reach::Undeclared));
    let asked = asked_of(&peer);
    assert!(asked.iter().any(|a| a == "parent GET /media/Folk/Dana%20Duo/Album/01.mp3"), "{asked:?}");
    let probe = "parent GET /media/Folk/Dana%20Duo/Album/cover.jpg";
    assert!(asked.iter().any(|a| a == probe), "a probe: {asked:?}");
    assert!(!asked.iter().any(|a| a.starts_with("direct ")), "{asked:?}");
    assert!(asked.contains(&"parent GET /album-art/f.png?compress=l".to_string()), "{asked:?}");
    let parent = fake.lock().unwrap().asked.clone();
    let head = parent.iter().any(|a| a.starts_with("HEAD /api/v1/federation/"));
    assert!(!head, "no HEAD through a proxy: {parent:?}");
    let lib = peer.lock().unwrap();
    let probes = lib.probes.iter().filter(|r| r.as_deref() == Some("bytes=0-0")).count();
    assert!(probes >= 1, "{:?}", lib.probes);
    assert!(lib.purposes.iter().all(Option::is_none), "the older proxy said nothing: {:?}", lib.purposes);
    drop(lib);
    let state = State::of(&look(&sim).state).unwrap();
    assert!(state.settings.peers[0].proxy_copies, "the answer is the card's");
    assert_eq!(endpoint(&sim), "e9");

    // Asked once: the card's own peers, read without the flag.
    let synced = sync_fed(&sim, &base, &rock(), &fed, &mut envs);
    let read = (synced.peers[0].status.clone(), synced.peers[0].reach);
    assert_eq!(read, (Status::Read, Some(Reach::Undeclared)));

    // A parent that passes the purpose on: read through it, the copy said.
    fake.lock().unwrap().old = false;
    let mut more = folk();
    more.push(peer_track("Folk/Dana Duo/Album/03.mp3", 3, corpus("cbr.mp3")));
    peer.lock().unwrap().tracks = more;
    let fresh = Ask { proxy_copies: Vec::new(), ..rock() };
    let synced = sync_fed(&sim, &base, &fresh, &fed, &mut envs);
    assert_eq!(ran(&synced).written, 1);
    assert_eq!(synced.peers[0].reach, Some(Reach::Proxy));
    assert_eq!(peer.lock().unwrap().purposes.last().unwrap().as_deref(), Some("copy"));
}

/// A parent says in its `/api/` that its stream proxy passes the copy
/// purpose on (`features.federationProxy`): a peer only that proxy reaches
/// is read through it, each file said to be a copy, with nothing asked of
/// the listener. A parent past the version the batch was cut from, with
/// the batch's `instanceId` and no such key, has a proxy that drops the
/// header: it is asked about, as an older parent is.
#[test]
fn a_proxy_that_says_it_declares_copies_is_read_through_and_one_that_doesnt_is_asked_about() {
    let scratch = Scratch::new("peer-proxy-says");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = world(false, false, folk());
    fake.lock().unwrap().version = Some("6.32.0");
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(ran(&synced).records, 4, "the server's own go in");
    assert_eq!(synced.peers[0].status, Status::AskProxy, "the id alone says nothing past the base");
    assert!(asked_of(&peer).is_empty(), "nothing asked of the peer");

    // It says so: read through the proxy, every file a declared copy.
    fake.lock().unwrap().proxy_says = Some(json!({"purpose": true}));
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.verified.clone(), outcome.written), (Ok(()), 2), "{:?}", synced.notes);
    let said = (synced.peers[0].status.clone(), synced.peers[0].reach);
    assert_eq!(said, (Status::Read, Some(Reach::Proxy)));
    let asked = asked_of(&peer);
    let proxied = "parent GET /media/Folk/Dana%20Duo/Album/01.mp3";
    assert!(asked.iter().any(|a| a == proxied), "{asked:?}");
    let purposes = peer.lock().unwrap().purposes.clone();
    let copies = purposes.iter().all(|p| p.as_deref() == Some("copy"));
    assert!(!purposes.is_empty() && copies, "{purposes:?}");
    let state = State::of(&look(&sim).state).unwrap();
    assert!(!state.settings.peers[0].proxy_copies, "the listener was never asked");
}

/// What says a parent's proxy passes the copy purpose on: `/api/`'s
/// `features.federationProxy.purpose`, as mStream says it; without the
/// key, the batch's `instanceId` only on the batch's own builds, which say
/// 6.31.1, the version it was cut from.
#[test]
fn the_proxys_purpose_is_its_apis_and_the_id_stands_for_it_on_the_batchs_own_builds_alone() {
    let says = |body: Value| proxy_says_copies(&serde_json::from_value(body).unwrap());
    let api = |server: &str, id: bool, proxy: Option<Value>| {
        let user = json!({"vpaths": ["Rock"]});
        let mut body = json!({"server": server, "features": {"sync": true}, "user": user});
        if id {
            body["instanceId"] = json!(ID);
        }
        if let Some(proxy) = proxy {
            body["features"]["federationProxy"] = proxy;
        }
        body
    };
    let declares = || Some(json!({"purpose": true}));
    assert!(says(api("6.32.0", true, declares())));
    assert!(says(api("6.32.0", false, declares())), "the key alone");
    assert!(says(api("6.31.1", true, None)), "the batch's own build, from before the key");
    assert!(says(api("6.31.1-dev", true, None)), "and a dev build of it");
    assert!(!says(api("6.32.0", true, None)), "past the base, the id and no key: it drops it");
    assert!(!says(api("6.31.1", false, None)), "6.31.1 as released: no batch");
    assert!(!says(api("a fork", true, None)), "a version that doesn't read: no guess");
    assert!(!says(json!({"instanceId": ID, "features": {}})), "no version");
    let not = |proxy: Value| api("6.32.0", true, Some(proxy));
    assert!(!says(not(json!({"purpose": false}))), "as said, though mStream never says it");
    assert!(!says(not(json!({}))), "a key with no purpose");
    assert!(!says(not(json!(true))), "a key of another shape costs the key alone");
}

/// A 429 of the peer's stream cap is waited out and the file asked again;
/// one of its daily quota parks the peer — its partial kept, the server's
/// own files in, a checkpoint whose plan keeps the peer's — until its own
/// Retry-After, or, through an older proxy that drops it, the next 00:00
/// UTC, and a few minutes. A run before then asks it nothing; one after
/// takes it up.
#[test]
fn a_peers_429s_are_waited_out_or_park_it_until_its_day_is_new() {
    let scratch = Scratch::new("peer-429");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = world(true, false, folk());
    let streams = Limit { words: "Too many concurrent streams", retry_after: Some(1) };
    let unsaid = Limit { words: "Too many concurrent streams", retry_after: None };
    peer.lock().unwrap().limits = VecDeque::from([streams, unsaid]);
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written), (None, 6), "{:?}", outcome.report);
    assert_eq!(peer.lock().unwrap().purposes.len(), 4, "two 429s, each waited out");

    // The quota, by its Retry-After: parked until then.
    let quota = Limit { words: "Daily transfer quota exceeded", retry_after: Some(30_000) };
    let three = "Folk/Dana Duo/Album/03.mp3";
    {
        let mut lib = peer.lock().unwrap();
        lib.tracks.push(peer_track(three, 3, corpus("cbr.mp3")));
        lib.limits = VecDeque::from([quota.clone()]);
    }
    fake.lock().unwrap().tracks.push(Track {
        path: "Rock/Artist G/Album H/01.mp3".into(),
        id: 9,
        bytes: corpus("multivalue.mp3"),
        seconds: 181.5,
        art: None,
        full: json!({"sample-rate": 44100, "channels": 2}),
    });
    fake.lock().unwrap().revision = "1:6:1790000000009:4".into();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    let why = "the peer's daily quota is spent: Daily transfer quota exceeded";
    let parked = (outcome.parked.as_deref(), outcome.written);
    assert_eq!(parked, (Some(why), 1), "the server's new file went in");
    let until = NOW + 30_000 + JITTER;
    assert_eq!((synced.peers[0].status.clone(), synced.peers[0].resume_at), (Status::Waiting, Some(until)));
    assert_eq!(synced.resume_at, Some(until));
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    assert_eq!(book.waiting(key_of(&base), NOW), Some(until));
    assert!(look(&sim).pending.is_some(), "its file waits in the plan");
    assert_eq!(synced_json(&synced, false)["resumeAt"], until);

    // Before then: nothing asked of it.
    let before = asked_of(&peer).len();
    clock.store(until - 1, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(synced.peers[0].status, Status::Waiting);
    assert_eq!(asked_of(&peer).len(), before, "not even its listing");

    // After: taken up.
    clock.store(until + 1, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written, outcome.verified.clone()), (None, 1, Ok(())));
    assert_eq!(synced.peers[0].status, Status::Read);

    // An older proxy drops the Retry-After: the words say the quota, and
    // it waits for the next 00:00 UTC.
    fake.lock().unwrap().old = true;
    {
        let mut lib = peer.lock().unwrap();
        lib.tracks.push(peer_track("Folk/Dana Duo/Album/04.mp3", 4, corpus("cbr.mp3")));
        lib.limits = VecDeque::from([quota]);
    }
    let proxied = Ask { proxy_copies: vec![3], ..rock_and_folk() };
    fake.lock().unwrap().peers[0].direct = false;
    let synced = sync_fed(&sim, &base, &proxied, &fed, &mut envs);
    let now = until + 1;
    assert_eq!(synced.peers[0].resume_at, Some(next_midnight(now) + JITTER), "{:?}", synced.peers[0]);
    assert_eq!(ran(&synced).parked.as_deref(), Some(why));
}

/// The most bytes one read of the HTTP client hands on: hyper's largest
/// read buffer (8 KiB, doubled as reads fill it, to 408 KiB). The chunk that
/// came while some of the day was left is written whole, so a file is cut
/// at the budget only when more than this of it lies past the budget.
const READ_MAX: u64 = 8192 + 4096 * 100;

/// This computer's budget for a peer, spent partway through a file: its
/// bytes so far kept in the card's partial, the server's files in, the peer
/// waiting for the next 00:00 UTC; a run the same day asks it nothing; the
/// next day's takes the partial up where it stopped, and finishes.
#[test]
fn a_peers_budget_spent_parks_it_and_the_next_day_takes_it_up() {
    let scratch = Scratch::new("peer-budget");
    let sim = card(&scratch);
    // Two files of 876 KB on a MiB a day: the first whole, the second past
    // the budget by some 700 KB, more than any one read, so it is cut
    // whatever size the reads come in. At 625 KB a file the budget ran out
    // some 200 KB before the second's end, and a read that long took the
    // file whole (one run in six).
    let big = |n: usize| mp3_bytes(2100 + n, true);
    let size = big(1).len() as u64;
    assert!(2 * size - (1 << 20) > READ_MAX + (64 << 10) && size < (1 << 20) - (64 << 10));
    let tracks = vec![
        peer_track("Folk/Dana Duo/Album/01.mp3", 1, big(0)),
        peer_track("Folk/Dana Duo/Album/02.mp3", 2, big(1)),
    ];
    let (base, _fake, peer, dial) = world(true, false, tracks);
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let one_mib = Ask { budget_mb: Some(1), ..rock_and_folk() };
    let synced = sync_fed(&sim, &base, &one_mib, &fed, &mut envs);
    let outcome = ran(&synced);
    let why = "the day's budget for the peer is spent";
    assert_eq!((outcome.parked.as_deref(), outcome.records), (Some(why), 5), "{:?}", outcome.report);
    let until = next_midnight(NOW) + JITTER;
    assert_eq!((synced.peers[0].status.clone(), synced.resume_at), (Status::Waiting, Some(until)));
    assert_eq!(synced.peers[0].cap, Some(1 << 20));
    assert_eq!(synced.peers[0].days, 2, "two files of 876 KB on a MiB a day");
    let seen = look(&sim);
    assert!(seen.pending.is_some() && !seen.root.as_ref().unwrap().manifest.is_final());
    let have = seen.resume.value.as_ref().unwrap()["parts"][0]["have"].as_u64().unwrap();
    assert!(have > 0 && have < size, "a part of it: {have}");
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    let used = book.used_today(key_of(&base), NOW);
    let read_or_two = 1 << 20..(1 << 20) + 2 * READ_MAX;
    assert!(read_or_two.contains(&used), "the budget, and a read or two: {used}");

    // The same day: nothing asked.
    let before = asked_of(&peer).len();
    clock.store(NOW + 3600, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), asked_of(&peer).len()), (Status::Waiting, before));

    // The next: taken up from its partial, and finished.
    clock.store(until + 60, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written, outcome.verified.clone()), (None, 1, Ok(())));
    let range = format!("bytes={have}-");
    assert_eq!(peer.lock().unwrap().ranges.last().unwrap().as_deref(), Some(range.as_str()));
    let on_card = std::fs::read(scratch.0.join("card/music/Folk/Dana Duo/Album/02.mp3")).unwrap();
    assert_eq!(on_card, big(1));
    let seen = look(&sim);
    assert!(seen.pending.is_none() && seen.root.as_ref().unwrap().manifest.is_final());
}

/// A peer's file the player can't play: with no ffmpeg on this computer it
/// is skipped and listed; with one, the peer's bytes come whole, are made
/// into Opus here, held to their length and given back their cover.
#[test]
fn a_peers_file_to_convert_is_skipped_without_ffmpeg_and_made_here_with_one() {
    let scratch = Scratch::new("peer-ffmpeg");
    let sim = card(&scratch);
    let wav = "Folk/Dana Duo/Album/03.wav";
    let mut tracks = folk();
    tracks.push(peer_track(wav, 3, b"RIFF a made-up wave of the peer's".to_vec()));
    let (base, _fake, peer, dial) = world(true, false, tracks);
    let clock = Arc::new(AtomicU64::new(NOW));
    let none = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &none, &mut envs);
    assert_eq!(ran(&synced).written, 6);
    let summary = &synced.peers[0];
    let skipped = vec![(wav.to_string(), Skip::NoTranscode)];
    assert_eq!((summary.skipped.clone(), summary.ffmpeg), (skipped, None));
    let printed = synced_json(&synced, false);
    assert_eq!(printed["peers"][0]["skipped"][0]["why"], "no-transcode");

    let made = Arc::new(MadeUp::default());
    let dial = TestDial(HashMap::from([("mstrfedg1:3".to_string(), tunnel_of(&peer))]));
    let with = fed(&scratch, &clock, dial, Ffmpeg::Given(Some(made.clone())));
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &with, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.written, outcome.verified.clone()), (1, Ok(())), "{:?}", outcome.report);
    assert_eq!(synced.peers[0].ffmpeg, Some("path"));
    assert_eq!(*made.0.lock().unwrap(), [33], "the peer's bytes, whole");
    let records = records_of(&sim);
    let opus = records.iter().find(|e| e.path == "Folk/Dana Duo/Album/03.opus").expect("made into Opus");
    let ledger = opus.ledger.as_ref().unwrap();
    let made = (ledger.converted_to, ledger.convert_kbps, ledger.server_path.as_str());
    assert_eq!(made, (CONTAINER_OPUS, 192, wav));
    let bytes = std::fs::read(scratch.0.join("card/music/Folk/Dana Duo/Album/03.opus")).unwrap();
    let cover = crate::device::card::tags::read_bytes(&bytes, "opus").picture;
    assert!(cover.is_some(), "its cover, from the peer's art");
}

/// A peer's file to convert whose source is more than what is left of its
/// day (it can't be taken up partway): it alone waits, not failed, the
/// peer's later file that fits comes all the same, and the peer is parked
/// at the run's end until the next 00:00 UTC; a run the same day asks it
/// nothing; the next day's makes it, and the transfer is finished.
#[test]
fn a_peers_conversion_larger_than_what_is_left_waits_alone() {
    let scratch = Scratch::new("peer-later");
    let sim = card(&scratch);
    let (wav, three) = ("Folk/Dana Duo/Album/02.wav", "Folk/Dana Duo/Album/03.mp3");
    let mut source = b"RIFF a made-up wave of the peer's".to_vec();
    source.resize(700 << 10, 0);
    let tracks = vec![
        peer_track(ONE, 1, mp3_bytes(1500, true)),
        peer_track(wav, 2, source),
        peer_track(three, 3, corpus("cbr.mp3")),
    ];
    let (base, _fake, peer, dial) = world(true, false, tracks);
    let clock = Arc::new(AtomicU64::new(NOW));
    let made = Arc::new(MadeUp::default());
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(Some(made.clone())));
    let mut envs = envs();
    let one_mib = Ask { budget_mb: Some(1), ..rock_and_folk() };
    let synced = sync_fed(&sim, &base, &one_mib, &fed, &mut envs);
    let outcome = ran(&synced);
    let why = "a file to convert is larger than what is left of the peer's day";
    assert_eq!((outcome.parked.as_deref(), outcome.records), (Some(why), 6), "{:?}", outcome.report);
    assert!(outcome.report.failed.is_empty(), "it waits: {:?}", outcome.report.failed);
    assert!(made.0.lock().unwrap().is_empty(), "none of its source asked for");
    let until = next_midnight(NOW) + JITTER;
    let waiting = (synced.peers[0].status.clone(), synced.peers[0].why.as_deref());
    assert_eq!((waiting, synced.resume_at), ((Status::Waiting, Some(why)), Some(until)));
    let on_card = |rel: &str| scratch.0.join("card/music").join(rel).exists();
    assert!(on_card(ONE) && on_card(three), "the files that fit");
    assert!(!on_card("Folk/Dana Duo/Album/02.opus"));
    assert!(look(&sim).pending.is_some(), "a checkpoint whose plan keeps it");

    // The same day: nothing asked.
    let before = asked_of(&peer).len();
    clock.store(NOW + 3600, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), asked_of(&peer).len()), (Status::Waiting, before));

    // The next: its source whole, made here, and the transfer finished.
    clock.store(until + 60, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written, outcome.verified.clone()), (None, 1, Ok(())));
    assert_eq!(*made.0.lock().unwrap(), [700 << 10]);
    assert!(on_card("Folk/Dana Duo/Album/02.opus"));
    let seen = look(&sim);
    assert!(seen.pending.is_none() && seen.root.as_ref().unwrap().manifest.is_final());
}

/// A second tunnel to a peer's own server, for a second dial.
fn tunnel_of(peer: &PeerFake) -> String {
    let served = peer.clone();
    routed(move |asked| peer_answer(&served, asked, "direct"))
}

/// The endpoint the card's settings know its first peer by.
fn endpoint_of(state: &Private) -> String {
    State::of(state).unwrap().settings.peers[0].endpoint.clone()
}

fn records_of(card: &FatSim) -> Vec<crate::device::card::contract::mptg::Entry> {
    look(card).root.expect("a root").tags.expect("its tags file").records
}

/// A peer is read as the card knows it: a listing its database is behind
/// (a file its folders hold that a scan hasn't reached) deletes nothing,
/// and the next whole one does; a server its parent lists under the card's
/// peer's id with another iroh identity isn't read at all.
#[test]
fn a_peer_is_read_only_as_the_card_knows_it() {
    let scratch = Scratch::new("peer-known");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = world(true, false, folk());
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(ran(&synced).records, 6);

    // One gone from the peer, while a scan is behind its folders: nothing goes.
    {
        let mut lib = peer.lock().unwrap();
        lib.tracks.retain(|t| t.path != ONE);
        lib.unscanned.push("Folk/Dana Duo/Album/03.mp3".into());
    }
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.records), (0, 6), "{:?}", outcome.report);
    // The scan caught up: now it goes.
    peer.lock().unwrap().unscanned.clear();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.deleted, outcome.records), (1, 5), "{:?}", outcome.report);

    // Its parent's peer 3 is another server now: not read, and the card's
    // rows of the one before stay.
    fake.lock().unwrap().peers[0].endpoint = "e9";
    let before = asked_of(&peer).len();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(synced.peers[0].status, Status::OtherPeer);
    assert_eq!(asked_of(&peer).len(), before, "nothing asked of it");
    assert_eq!(ran(&synced).records, 5);
}

/// A peer read whose files the card holds none of, paired again under its
/// id (another identity): read by the new one; once the card holds its
/// files, another identity again isn't read.
#[test]
fn a_peer_paired_again_is_read_while_the_card_holds_none_of_its_files() {
    let scratch = Scratch::new("peer-again");
    let sim = card(&scratch);
    let (base, fake, peer, dial) = world(true, false, Vec::new());
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), synced.peers[0].files), (Status::Read, 0));
    let endpoint = |sim: &FatSim| endpoint_of(&look(sim).state);
    assert_eq!(endpoint(&sim), "e3");

    fake.lock().unwrap().peers[0].endpoint = "e9";
    peer.lock().unwrap().tracks = folk();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), ran(&synced).written), (Status::Read, 2));
    assert_eq!(endpoint(&sim), "e9");

    fake.lock().unwrap().peers[0].endpoint = "e7";
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(synced.peers[0].status, Status::OtherPeer);
    assert_eq!(endpoint(&sim), "e9");
}

/// What is left of a peer's day holds all that is asked of it, not its
/// files alone: a folder image larger than it is probed and never read
/// (mStream's small art instead), and a day its covers spent parks the peer
/// at the run's end, its vectors not asked; a run the same day asks it
/// nothing.
#[test]
fn a_peers_day_holds_its_covers_and_vectors_too() {
    let scratch = Scratch::new("peer-covers-day");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
    {
        let mut lib = peer.lock().unwrap();
        lib.images.insert(format!("{ALBUM}/cover.jpg"), vec![0xFF; 1200 << 10]);
        lib.art.insert("f.png".into(), vec![7; 1100 << 10]);
    }
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let one_mib = Ask { budget_mb: Some(1), ..rock_and_folk() };
    let synced = sync_fed(&sim, &base, &one_mib, &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.parked.clone(), outcome.written), (None, 6), "{:?}", outcome.report);
    let until = next_midnight(NOW) + JITTER;
    let said = &synced.peers[0];
    let waits = (said.status.clone(), said.resume_at, synced.resume_at);
    assert_eq!(waits, (Status::Waiting, Some(until), Some(until)), "{said:?}");
    let asked = asked_of(&peer);
    let cover = "direct GET /media/Folk/Dana%20Duo/Album/cover.jpg";
    assert_eq!(asked.iter().filter(|a| *a == cover).count(), 1, "probed, never read: {asked:?}");
    let probes = peer.lock().unwrap().probes.clone();
    assert!(probes.iter().all(|r| r.as_deref() == Some("bytes=0-0")), "{probes:?}");
    assert_eq!(asked.iter().filter(|a| a.contains("/album-art/")).count(), 1, "{asked:?}");
    assert!(!asked.iter().any(|a| a.contains("embeddings")), "no vectors past its day: {asked:?}");
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    assert_eq!(book.waiting(key_of(&base), NOW), Some(until));

    // The same day: nothing asked.
    let before = asked_of(&peer).len();
    clock.store(NOW + 3600, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), asked_of(&peer).len()), (Status::Waiting, before));
}

/// A key with nothing left of its quota today, as the peer says it (its
/// allowance's `remainingTodayBytes`): the peer waits for the next 00:00
/// UTC, its `/api/` alone asked (and counted); a run the same day asks it
/// nothing.
#[test]
fn a_peer_whose_key_has_nothing_left_today_isnt_listed() {
    let scratch = Scratch::new("peer-quota-none");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
    let allowance = json!({"dailyMb": 2048, "remainingTodayBytes": 0, "allowCopies": true});
    peer.lock().unwrap().allowance = Some(allowance);
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!(ran(&synced).records, 4, "the server's own go in");
    let until = next_midnight(NOW) + JITTER;
    let said = &synced.peers[0];
    let waits = (said.status.clone(), said.resume_at, synced.resume_at);
    assert_eq!(waits, (Status::Waiting, Some(until), Some(until)), "{said:?}");
    assert_eq!(asked_of(&peer), ["parent GET /api/"], "its /api/ alone");
    let used = Budget::at(scratch.0.join("config")).read().unwrap().used_today(key_of(&base), NOW);
    assert!(used > 0, "its /api/, counted");

    clock.store(NOW + 3600, Ordering::Relaxed);
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), asked_of(&peer).len()), (Status::Waiting, 1));
}

/// One parent the listener reaches at two addresses (the server known by
/// its own id at both) keeps one budget for its peer: a day spent through
/// one is spent through the other, and the peer is asked nothing there.
#[test]
fn a_peers_day_is_one_at_any_address_of_its_parent() {
    let scratch = Scratch::new("peer-alias");
    let sim = card(&scratch);
    let big = |n: usize| mp3_bytes(1500 + n, true);
    let tracks = vec![peer_track(ONE, 1, big(0)), peer_track("Folk/Dana Duo/Album/02.mp3", 2, big(1))];
    let (base, fake, peer, dial) = world(true, false, tracks);
    let served = fake.clone();
    let other = routed(move |asked| answer(&served, asked));
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let one_mib = Ask { budget_mb: Some(1), ..rock_and_folk() };
    let synced = sync_fed(&sim, &base, &one_mib, &fed, &mut envs);
    let until = next_midnight(NOW) + JITTER;
    assert_eq!((synced.peers[0].status.clone(), synced.resume_at), (Status::Waiting, Some(until)));

    let before = asked_of(&peer).len();
    let synced = sync_fed(&sim, &other, &rock_and_folk(), &fed, &mut envs);
    let said = &synced.peers[0];
    let waits = (said.status.clone(), said.resume_at);
    assert_eq!(waits, (Status::Waiting, Some(until)), "{:?}", synced.notes);
    assert_eq!(asked_of(&peer).len(), before, "nothing asked of it");
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    let there = PeerKey { parent_key: server_url_key(&other), peer_id: 3 };
    assert_eq!((book.peers.len(), book.waiting(there, NOW)), (1, Some(until)), "one account: {book:?}");
}

/// A budget the book can't keep (`budget.json` can't be written partway
/// through a run): the file that came whole stays, the peer is stopped for
/// the run and says why, and nothing more is asked of it; the next run, the
/// book whole again, takes up the rest.
#[test]
fn a_peer_whose_budget_cant_be_kept_is_stopped() {
    let scratch = Scratch::new("peer-unkept");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
    let blocked = scratch.0.join("config").join("budget.tmp");
    peer.lock().unwrap().poison = Some(blocked.clone());
    let clock = Arc::new(AtomicU64::new(NOW));
    let fed = fed(&scratch, &clock, dial, Ffmpeg::Given(None));
    let mut envs = envs();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    let outcome = ran(&synced);
    assert_eq!((outcome.written, outcome.parked.clone()), (5, None), "{:?}", outcome.report);
    let said = &synced.peers[0];
    let why = said.why.clone().unwrap_or_default();
    assert_eq!(said.status, Status::Refused, "{why}");
    assert!(why.starts_with("its budget can't be kept"), "{why}");
    let failed: Vec<&str> = outcome.report.failed.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(failed, [TWO], "{:?}", outcome.report.failed);
    let lib = peer.lock().unwrap();
    assert_eq!((lib.purposes.len(), lib.probes.len()), (1, 0), "nothing more asked of it");
    drop(lib);
    assert_eq!(synced.resume_at, None);

    std::fs::remove_dir(&blocked).unwrap();
    let synced = sync_fed(&sim, &base, &rock_and_folk(), &fed, &mut envs);
    assert_eq!((synced.peers[0].status.clone(), ran(&synced).written), (Status::Read, 1));
}
