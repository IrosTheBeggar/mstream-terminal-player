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
use crate::device::card::sync::peers::{Ffmpeg, Status};

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
    /// Each file GET's X-mStream-Purpose and Range, as they came.
    purposes: Vec<Option<String>>,
    ranges: Vec<Option<String>>,
    /// The bytes of its files it served.
    served: u64,
    /// Files its folders hold that its database doesn't have yet.
    unscanned: Vec<String>,
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
        ("GET" | "HEAD", p) if p.starts_with("/media/") => {
            let rel = p["/media/".len()..].to_string();
            let purpose = asked.header("x-mstream-purpose").map(str::to_string);
            if asked.method == "GET" {
                lib.purposes.push(purpose.clone());
                lib.ranges.push(asked.header("range").map(str::to_string));
            }
            if lib.copies_off && purpose.as_deref() == Some("copy") {
                return json_reply("403 Forbidden", r#"{"error":"copies are not allowed with this key"}"#);
            }
            if asked.method == "GET"
                && lib.tracks.iter().any(|t| t.path == rel)
                && let Some(limit) = lib.limits.pop_front()
            {
                let body = json!({"error": limit.words}).to_string();
                let after = limit.retry_after.map(|s| s.to_string());
                let headers: Vec<(&str, &str)> =
                    after.iter().map(|a| ("Retry-After", a.as_str())).collect();
                return reply("429 Too Many Requests", &headers, body.as_bytes());
            }
            let Some(bytes) = lib.tracks.iter().find(|t| t.path == rel).map(|t| t.bytes.clone()) else {
                return not_found();
            };
            let out = media(asked, &bytes);
            if asked.method == "GET" {
                lib.served += body_len(&out);
            }
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
/// covers probed there with HEAD and its art the 256 px copy through the
/// parent's art proxy; its rows on the card name it (an SRVS peer entry,
/// its own id), every byte read from it is in the budget's book, and the
/// card's settings know it by its parent's key, its id and its identity.
#[test]
fn a_peers_files_go_on_the_card_beside_the_servers_and_every_byte_is_counted() {
    let scratch = Scratch::new("peer-direct");
    let sim = card(&scratch);
    let (base, _fake, peer, dial) = world(true, false, folk());
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
    let probe = "direct HEAD /media/Folk/Dana%20Duo/Album/cover.jpg".to_string();
    assert!(asked.contains(&probe), "{asked:?}");
    assert!(asked.contains(&"parent GET /album-art/f.png?compress=l".to_string()), "{asked:?}");
    let lib = peer.lock().unwrap();
    assert!(lib.purposes.iter().all(|p| p.as_deref() == Some("copy")), "{:?}", lib.purposes);
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
/// read without asking.
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
    let probes = lib.ranges.iter().filter(|r| r.as_deref() == Some("bytes=0-0")).count();
    assert!(probes >= 1, "{:?}", lib.ranges);
    assert!(lib.purposes.iter().all(Option::is_none), "the older proxy said nothing: {:?}", lib.purposes);
    drop(lib);
    let state = State::of(&look(&sim).state).unwrap();
    assert!(state.settings.peers[0].proxy_copies, "the answer is the card's");

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

/// This computer's budget for a peer, spent partway through a file: its
/// bytes so far kept in the card's partial, the server's files in, the peer
/// waiting for the next 00:00 UTC; a run the same day asks it nothing; the
/// next day's takes the partial up where it stopped, and finishes.
#[test]
fn a_peers_budget_spent_parks_it_and_the_next_day_takes_it_up() {
    let scratch = Scratch::new("peer-budget");
    let sim = card(&scratch);
    let big = |n: usize| mp3_bytes(1500 + n, true);
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
    assert_eq!(synced.peers[0].days, 2, "two files of 625 KB on a MiB a day");
    let seen = look(&sim);
    assert!(seen.pending.is_some() && !seen.root.as_ref().unwrap().manifest.is_final());
    let have = seen.resume.value.as_ref().unwrap()["parts"][0]["have"].as_u64().unwrap();
    assert!(have > 0 && have < 625_520, "a part of it: {have}");
    let book = Budget::at(scratch.0.join("config")).read().unwrap();
    let used = book.used_today(key_of(&base), NOW);
    assert!((1 << 20..(1 << 20) + (256 << 10)).contains(&used), "the budget, and a chunk or two: {used}");

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

/// A second tunnel to a peer's own server, for a second dial.
fn tunnel_of(peer: &PeerFake) -> String {
    let served = peer.clone();
    routed(move |asked| peer_answer(&served, asked, "direct"))
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
