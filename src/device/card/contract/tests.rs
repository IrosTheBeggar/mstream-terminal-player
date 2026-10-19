//! The kit against the contract's shared fixtures, test/fixtures/card (made
//! by the firmware repo's tools/card_fixtures.py and copied from there; the
//! folder's README names the commit): every vector of 2.18, every golden
//! written byte for byte from its description, every hardening file read as
//! absent or present for the player's own reason, every golden truncated at
//! every byte and every bit of it flipped, and round trips of generated
//! libraries in any input order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

use super::container::{self, MPDJ, MPTG, MSMF, MSPD, unknown_major};
use super::device_txt::{DeviceTxt, Format};
use super::fat::{fat_from_wall, fat_time, fat_valid, fat_wall, skew, time_matches};
use super::mpdj::{self, Table};
use super::mppl;
use super::mptg::{self, FolderIn, Ledger, Meta, Record, RecordIn, Tags};
use super::msmf::{self, Ack, Candidate, Comp, Manifest, ManifestIn, Pick, elect};
use super::mspd::{self, Op, Plan, PlanIn};
use super::mpth;
use super::numbers;
use super::order::{album_folder, compare_file_paths, compare_names, thumb_path};
use super::prim::{self, crc32, crc32_after, fnv1a64, hex, hex16, md5, path_hash, qfp_of};
use super::server::{
    self, PEER, Server, Servers, Talk, TalkPeer, Verdict, normalise_url, server_url_key,
};
use super::text::{FIELD_NAMES, RUN_FIELDS, Run, build_field, is_list};
use super::{MAX_FILE, Why};

// ── The fixtures ────────────────────────────────────────────────────────────

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card").join(rel)
}

fn bytes(rel: &str) -> Vec<u8> {
    std::fs::read(fixture(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_slice(&bytes(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn golden(name: &str) -> Vec<u8> {
    bytes(&format!("golden/{name}"))
}

fn library(name: &str) -> Value {
    json(&format!("libraries/{name}.json"))
}

fn vectors() -> Value {
    json("vectors.json")
}

/// An integer as the fixtures give one: a number, or a string (`0x…` hex,
/// else decimal: integers past 2^53 are hex strings).
fn num(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_i64().map(|i| i as u64)).expect("an integer"),
        Value::String(s) => match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(h) => u64::from_str_radix(h, 16).expect("hex"),
            None => s.parse().expect("decimal"),
        },
        Value::Null => 0,
        other => panic!("not an integer: {other}"),
    }
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap_or_else(|| num(v) as i64)
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// Space-separated hex bytes, `4D 50 54 47`.
fn spaced(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|b| u8::from_str_radix(b, 16).expect("a hex byte")).collect()
}

fn spaced_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

fn uuid(v: &Value) -> [u8; 16] {
    v.as_str().map_or([0; 16], |s| hex16(s).expect("a UUID"))
}

fn url_key(v: &Value) -> u64 {
    v.as_str().map_or(0, server_url_key)
}

/// A device id as the fixtures give one: 12 hex digits, with or without
/// colons (`00:00:5e:00:53:01`).
fn device_id(v: &Value) -> [u8; 6] {
    let digits: String = text(v).chars().filter(|&c| c != ':').collect();
    let mut id = [0; 6];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(&digits[2 * i..2 * i + 2], 16).expect("a hex byte");
    }
    id
}

/// ACKS entries as the fixtures list them: deviceId, epoch, seq.
fn acks_of(v: &Value) -> Vec<Ack> {
    let ack = |e: &Value| {
        Ack { device_id: device_id(&e["deviceId"]), epoch: num(&e["epoch"]), seq: num(&e["seq"]) as u32 }
    };
    v.as_array().into_iter().flatten().map(ack).collect()
}

// ── Descriptions into the writers' inputs ──────────────────────────────────

fn raw_tags(tags: &Value) -> [Vec<String>; RUN_FIELDS] {
    let mut raw: [Vec<String>; RUN_FIELDS] = Default::default();
    for (f, name) in FIELD_NAMES.iter().enumerate() {
        if let Some(values) = tags.get(*name).and_then(Value::as_array) {
            raw[f] = values.iter().map(|v| text(v).to_owned()).collect();
        }
    }
    raw
}

fn ledger_of(l: &Value) -> Ledger {
    Ledger {
        server_path: text(&l["serverPath"]).to_owned(),
        mstream_id: num(&l["mstreamId"]) as u32,
        album_id: num(&l["albumId"]) as u32,
        artist_id: num(&l["artistId"]) as u32,
        audio_hash: l["audioHash"].as_str().map_or([0; 16], |h| hex16(h).expect("a digest")),
        file_hash: l["fileHash"].as_str().map_or([0; 16], |h| hex16(h).expect("a digest")),
        server_modified: num(&l["serverModified"]),
        server_size: num(&l["serverSize"]),
        created_at: num(&l["createdAt"]) as u32,
        hash_v: num(&l["hashV"]) as u16,
        origin_flags: num(&l["originFlags"]) as u8,
        converted_to: num(&l["convertedTo"]) as u8,
        convert_kbps: num(&l["convertKbps"]) as u16,
        server: num(&l["server"]) as u16,
    }
}

/// A tags description (raw tag values: the writer's caller applies 2.3.6
/// and sets TRUNCATED, as the tag reader will).
fn tags_input(d: &Value) -> (Meta, Vec<RecordIn>, Vec<FolderIn>) {
    let source = num(&d["source"]) as u8;
    let meta = Meta {
        source,
        parser_version: num(&d["parserVersion"]) as u16,
        read_rules: d.get("readRules").map_or(1, num) as u16,
        generation: num(&d["generation"]) as u32,
        card_id: num(&d["cardId"]),
        minor: num(&d["minor"]) as u16,
        producer: text(&d["producer"]).to_owned(),
    };
    let mut records = Vec::new();
    for r in d["records"].as_array().into_iter().flatten() {
        let (run, truncated) = Run::build(&raw_tags(&r["tags"]));
        let pic = &r["picture"];
        let rec = Record {
            size: num(&r["size"]) as u32,
            fat_time: num(&r["fatTime"]) as u32,
            duration_ms: num(&r["durationMs"]) as u32,
            qfp: num(&r["qfp"]),
            known: num(&r["known"]) as u32,
            flags: num(&r["flags"]) as u16 | if truncated { mptg::TRUNCATED } else { 0 },
            year: num(&r["year"]) as u16,
            track: num(&r["track"]) as u16,
            track_total: num(&r["trackTotal"]) as u16,
            disc: num(&r["disc"]) as u16,
            disc_total: num(&r["discTotal"]) as u16,
            bpm10: num(&r["bpm10"]) as u16,
            rg_track_gain: int(&r["rgTrackGain"]) as i16,
            rg_album_gain: int(&r["rgAlbumGain"]) as i16,
            rg_track_peak: num(&r["rgTrackPeak"]) as u16,
            rg_album_peak: num(&r["rgAlbumPeak"]) as u16,
            container: num(&r["container"]) as u8,
            camelot: num(&r["camelot"]) as u8,
            pic_offset: num(&pic["offset"]) as u32,
            pic_length: num(&pic["length"]) as u32,
            pic_type: num(&pic["type"]) as u8,
            pic_mime: num(&pic["mime"]) as u8,
            pic_coding: num(&pic["coding"]) as u8,
        };
        let ledger = (source != mptg::SOURCE_DEVICE).then(|| ledger_of(&r["ledger"]));
        records.push(RecordIn { path: text(&r["path"]).to_owned(), rec, run, ledger });
    }
    let folders = d["folders"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| FolderIn { path: text(&f["path"]).to_owned(), flags: num(&f["flags"]) as u32 })
        .collect();
    (meta, records, folders)
}

fn server_entry(s: &Value) -> Server {
    Server {
        instance: uuid(&s["serverInstance"]),
        url_key: url_key(&s["serverUrl"]),
        parent_key: url_key(&s["parentUrl"]),
        peer_id: num(&s["peerId"]) as u32,
        flags: num(&s["flags"]) as u32,
    }
}

/// A root description's servers: server 0 from its header fields, then
/// its SRVS entries, each key from the URL given (2.5.1).
fn servers_of(card: &Value) -> Servers {
    let key = match card["serverUrl"].is_string() {
        true => url_key(&card["serverUrl"]),
        false => num(&card["serverUrlKey"]),
    };
    Servers {
        root: Server::root(uuid(&card["serverInstance"]), key),
        entries: card["servers"].as_array().into_iter().flatten().map(server_entry).collect(),
    }
}

fn manifest_input(d: &Value) -> ManifestIn {
    let servers = servers_of(d);
    ManifestIn {
        generation: num(&d["generation"]) as u32,
        card_id: num(&d["cardId"]),
        minor: num(&d["minor"]) as u16,
        commit_time: num(&d["commitTime"]) as u32,
        flags: num(&d["flags"]) as u32,
        commit_id: num(&d["commitId"]),
        server_instance: servers.root.instance,
        server_url_key: servers.root.url_key,
        producer: text(&d["producer"]).to_owned(),
        server_revision: text(&d["serverRevision"]).to_owned(),
        base_generation: num(&d["baseGeneration"]) as u32,
        comps: d["companions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| Comp::of(&golden(text(&c["file"]))).expect("a companion"))
            .collect(),
        roots: d["roots"].as_array().into_iter().flatten().map(|r| text(r).to_owned()).collect(),
        servers: servers.entries,
        acks: acks_of(&d["acks"]),
    }
}

fn plan_input(d: &Value) -> PlanIn {
    let op = |name: &str| match name {
        "write" => mspd::OP_WRITE,
        "delete" => mspd::OP_DELETE,
        "folder" => mspd::OP_FOLDER,
        other => panic!("an op: {other}"),
    };
    PlanIn {
        generation: num(&d["generation"]) as u32,
        card_id: num(&d["cardId"]),
        minor: 0,
        run_id: num(&d["runId"]),
        base_generation: num(&d["baseGeneration"]) as u32,
        ops: d["ops"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| Op {
                op: op(text(&o["op"])),
                path: text(&o["path"]).to_owned(),
                expected_size: num(&o["expectedSize"]) as u32,
            })
            .collect(),
    }
}

/// 2.17's synthetic picture: pixel (x, y) of an n × n slot is RGB
/// (x·255/(n−1), y·255/(n−1), (x+y)·255/(2n−2)), integer division.
fn thumb_pixels(n: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let (r, g, b) = (x * 255 / (n - 1), y * 255 / (n - 1), (x + y) * 255 / (2 * n - 2));
            out.push(mpth::rgb565(r as u8, g as u8, b as u8));
        }
    }
    out
}

fn device_input(d: &Value) -> DeviceTxt {
    // The majors as the parser reads a list.
    let majors = |k: &str| {
        let line = format!("contract=1\nread.mpdj={}\n", text(&d[k]));
        DeviceTxt::read(Some(line.as_bytes())).read_mpdj
    };
    DeviceTxt {
        present: true,
        contract: num(&d["contract"]) as u16,
        firmware: text(&d["firmware"]).to_owned(),
        read_msmf: majors("read.msmf"),
        read_mptg: majors("read.mptg"),
        read_mpdj: majors("read.mpdj"),
        read_mpth: majors("read.mpth"),
        codecs: text(&d["codecs"]).to_owned(),
        extensions: text(&d["extensions"]).to_owned(),
        max_rate: num(&d["max_rate"]) as u32,
        max_channels: num(&d["max_channels"]) as u32,
        history: majors("history"),
    }
}

/// A file read by its format's reader with the uses a hardening entry
/// names: Ok, or why it is absent.
fn read_as(format: &str, file: &[u8], device: bool) -> Why {
    let r = match format {
        "MPTG" => mptg::read(file, if device { mptg::Uses::DEVICE } else { mptg::Uses::ALL }).map(drop),
        "MSMF" => Manifest::read(file, if device { msmf::Uses::DEVICE } else { msmf::Uses::ALL }).map(drop),
        "MSPD" => Plan::read(file).map(drop),
        "MPDJ" => {
            let uses = if device { mpdj::Uses::DEVICE } else { mpdj::Uses::ALL };
            Table::open(file).and_then(|t| t.check(uses))
        }
        other => panic!("a format: {other}"),
    };
    r.err().unwrap_or(Why::Ok)
}

/// Where a section lies, found by its type in the directory: its offset and
/// its length.
fn section_span(file: &[u8], kind: u32) -> (usize, usize) {
    let h = container::Header::parse(file).expect("a frame");
    for i in 0..h.section_count as usize {
        let e = &file[h.header_bytes as usize + 32 * i..];
        if prim::u32_at(e, 0) == kind {
            return (prim::u32_at(e, 8) as usize, prim::u32_at(e, 12) as usize);
        }
    }
    panic!("no such section")
}

/// A section's bytes.
fn section_of(file: &[u8], kind: u32) -> &[u8] {
    let (off, len) = section_span(file, kind);
    &file[off..off + len]
}

fn format_of(file: &[u8]) -> &'static str {
    match container::Header::parse(file).map(|h| h.magic) {
        Some(MPTG) => "MPTG",
        Some(MSMF) => "MSMF",
        Some(MSPD) => "MSPD",
        Some(MPDJ) => "MPDJ",
        _ => "?",
    }
}

// ── The vectors (2.18) ──────────────────────────────────────────────────────

/// Every group of vectors.json is one this file checks, or one that binds
/// another part: a group added to the fixtures fails here until it is
/// placed.
#[test]
fn every_vector_group_is_placed() {
    let v = vectors();
    let ours: BTreeSet<&str> = [
        "note",
        "crc32",
        "fnv1a64",
        "pathHash",
        "serverUrlKey",
        "qfp",
        "qfpNote",
        "fatTime",
        "fatTimeInvalid",
        "wallDelta",
        "skew",
        "skewNote",
        "bytes",
        "canonicalOrder",
        "stringRuns",
        "fieldRules",
        "numbers",
        "hashSampled",
        "rootElection",
        "albumFolder",
        "servers",
        "deviceTxt",
        "mpdj",
        "softwareMatching",
        // Of the play log's group: ACKS, device.txt's history key, the
        // slots, the ids and the reader's cases. A play resolved through a
        // tags file and where it goes are the play-history sync's
        // (history.rs: `resolve`, `resolveCases`, `peers`); the seq and
        // bootSeq at boot and the device's model of log.json are the
        // device's (`atBoot`, `deviceOps`).
        "plays",
    ]
    .into();
    // Groups the pieces beside the kit check: names.rs the card paths
    // (2.8.3-2.8.6); the volume layer the private files' read, the group's
    // `settle` cases (2.12.7). The rest of that group (the names a private
    // file may have, step 4's clean-up, a partial's fetch: 2.12.7, 2.12.8)
    // is the engine's and set-up's (A8, A12, B2), not built yet.
    let beside: BTreeSet<&str> = ["names", "privateFiles"].into();
    assert!(beside.iter().all(|g| v[*g]["who"] == "software"));
    // The device's builder (2.9), milestone N2 of the player: never the
    // software's.
    let theirs: BTreeSet<&str> = ["builderPrecedence"].into();
    assert_eq!(v["builderPrecedence"]["who"], "builder");
    let groups: BTreeSet<&str> = v.as_object().expect("an object").keys().map(String::as_str).collect();
    let placed: BTreeSet<&str> = [ours, beside, theirs].iter().flatten().copied().collect();
    assert_eq!(groups, placed);
}

#[test]
fn vectors_checksums_and_hashes() {
    let v = vectors();
    for e in v["crc32"].as_array().expect("crc32") {
        assert_eq!(crc32(text(&e["ascii"]).as_bytes()) as u64, num(&e["crc"]));
    }
    // The chained CRC is the CRC of the two parts in a row (zlib's).
    assert_eq!(crc32_after(crc32(b"1234"), b"56789"), crc32(b"123456789"));
    for e in v["fnv1a64"].as_array().expect("fnv1a64") {
        assert_eq!(fnv1a64(text(&e["utf8"]).as_bytes()), num(&e["hash"]), "{}", e["utf8"]);
    }
    for e in v["pathHash"].as_array().expect("pathHash") {
        assert_eq!(path_hash(text(&e["rel"])), num(&e["hash"]), "{}", e["rel"]);
    }
    for e in v["serverUrlKey"].as_array().expect("serverUrlKey") {
        assert_eq!(normalise_url(text(&e["url"])).as_deref(), Some(text(&e["normalised"])));
        assert_eq!(server_url_key(text(&e["url"])), num(&e["key"]));
    }
    // A URL with no host has no key: 0, unknown.
    assert_eq!(server_url_key("music.example"), 0);
    assert_eq!(server_url_key("http://"), 0);
    assert_eq!(server_url_key("http://host:3a/"), 0);
    for e in v["qfp"].as_array().expect("qfp") {
        let data: Vec<u8> = (0..num(&e["size"]) as usize).map(|i| (i & 0xFF) as u8).collect();
        assert_eq!(qfp_of(&data), num(&e["qfp"]), "qfp of {}", e["size"]);
    }
    let r = prim::qfp_ranges(8193);
    assert_eq!((r.head, r.tail_offset, r.tail), (4096, 4097, 4096));
    let r = prim::qfp_ranges(5000);
    assert_eq!((r.head, r.tail_offset, r.tail), (4096, 4096, 904));
}

#[test]
fn vectors_hash_sampled_and_md5() {
    for e in vectors()["hashSampled"].as_array().expect("hashSampled") {
        let sampled = prim::hash_sampled(num(&e["hashV"]) as u16, num(&e["fileSize"]));
        assert_eq!(sampled, e["sampled"].as_bool().expect("a bool"), "{e}");
    }
    assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
    // The descriptions' made-up digests are MD5s of a seed (card_fixtures.py's
    // h16): the ledger's audioHash of a record is the MD5 of "a" + its path.
    let d = library("tags-0000002a");
    let r = &d["records"][0];
    let seed = format!("a{}", text(&r["path"]));
    assert_eq!(hex(&md5(seed.as_bytes())), text(&r["ledger"]["audioHash"]));
    let mut sum = prim::Md5Sum::new();
    sum.update(b"a");
    sum.update(text(&r["path"]).as_bytes());
    assert_eq!(hex(&sum.finish()), text(&r["ledger"]["audioHash"]));
}

#[test]
fn vectors_fat_time() {
    let v = vectors();
    for e in v["fatTime"].as_array().expect("fatTime") {
        let (date, time) = text(&e["time"]).split_once(' ').expect("a time");
        let d: Vec<u32> = date.split('-').map(|x| x.parse().expect("a number")).collect();
        let t: Vec<u32> = time.split(':').map(|x| x.parse().expect("a number")).collect();
        let stamp = fat_time(d[0] as i32, d[1], d[2], t[0], t[1], t[2]);
        assert_eq!(stamp as u64, num(&e["fatTime"]), "{}", e["time"]);
    }
    for e in v["fatTimeInvalid"].as_array().expect("fatTimeInvalid") {
        assert!(!fat_valid(num(e) as u32) && fat_wall(num(e) as u32).is_none(), "{e}");
    }
    for e in v["wallDelta"].as_array().expect("wallDelta") {
        let wall = |k: &str| fat_wall(num(&e[k]) as u32).expect("valid");
        assert_eq!(wall("b") - wall("a"), int(&e["delta"]));
    }
    // A wall time and back; an odd W rounds down; past 2107 there is none.
    let t = fat_time(2026, 10, 7, 14, 30, 42);
    assert_eq!(fat_from_wall(fat_wall(t).expect("valid")), t);
    assert_eq!(fat_from_wall(fat_wall(t).expect("valid") + 1), t);
    assert_eq!(fat_time(2024, 2, 29, 0, 0, 0) >> 16 & 31, 29);
    assert_eq!(fat_time(2023, 2, 29, 0, 0, 0), 0);
    assert_eq!(fat_time(2108, 1, 1, 0, 0, 0), 0);
    assert!(!time_matches(0, 0, 0));
}

/// The skew vectors' pairs (vectors.json's skewNote): recorded = 2026-10-07
/// 14:30:42 + 2 days × i, observed = recorded + the group's delta, unless
/// the group pins either stamp.
fn pairs_for(groups: &Value) -> Vec<(u32, u32)> {
    let base = fat_wall(fat_time(2026, 10, 7, 14, 30, 42)).expect("valid");
    let mut out = Vec::new();
    for g in groups.as_array().expect("groups") {
        for _ in 0..num(&g["count"]) {
            let mut rec = fat_from_wall(base + 2 * 86_400 * out.len() as i64);
            let mut obs = fat_from_wall(fat_wall(rec).expect("valid") + int(&g["delta"]));
            if g.get("recorded").is_some() {
                rec = num(&g["recorded"]) as u32;
            }
            if g.get("observed").is_some() {
                obs = num(&g["observed"]) as u32;
            }
            out.push((rec, obs));
        }
    }
    out
}

#[test]
fn vectors_skew() {
    for e in vectors()["skew"].as_array().expect("skew") {
        let pairs = pairs_for(&e["groups"]);
        let d = skew(pairs.iter().copied());
        assert_eq!(d as i64, int(&e["skew"]), "{}", e["groups"]);
        let matching = pairs.iter().filter(|(r, o)| time_matches(*r, *o, d)).count();
        assert_eq!(matching as u64, num(&e["matching"]), "{}", e["groups"]);
        // The rule's answer doesn't depend on the order of the pairs.
        let mut reversed = pairs.clone();
        reversed.reverse();
        assert_eq!(skew(reversed), d);
    }
}

#[test]
fn vectors_bytes() {
    let v = vectors();
    let b = &v["bytes"];
    assert_eq!(spaced_of(&MPTG.to_le_bytes()), text(&b["MPTG"]));
    assert_eq!(MPTG as u64, num(&b["MPTGu32"]));
    assert_eq!(container::MPTH as u64, num(&b["MPTHu32"]));
    let t = &b["thumbPath"];
    let h = path_hash(text(&t["folder"]));
    assert_eq!(format!("/.mstream/{}", thumb_path(h)), text(&t["path"]));
    let px = mpth::Pixels { small: thumb_pixels(mpth::SMALL), large: thumb_pixels(mpth::LARGE) };
    let file = mpth::write(h, 0, &px).expect("a thumbnail");
    assert_eq!(file.len() as u64, num(&t["bytes"]));
    assert_eq!(spaced_of(&file[..16]), text(&t["head0to15"]));
    assert_eq!(spaced_of(&file[20..24]), text(&t["head20to23"]));
    let si = &b["serverInstance"];
    assert_eq!(spaced_of(&hex16(text(&si["uuid"])).expect("a UUID")), text(&si["stored"]));
    // mStream's `instanceId` reads the same way, and only as a UUID's text.
    let uuid = text(&si["uuid"]);
    assert_eq!(spaced_of(&server::instance_of(Some(uuid))), text(&si["stored"]));
    assert_eq!(server::instance_of(Some(&uuid.to_ascii_uppercase())), server::instance_of(Some(uuid)));
    let unknown = [
        "",
        "00112233445566778899aabbccddeeff",
        "{00112233-4455-6677-8899-aabbccddeeff}",
        "00112233-4455-6677-8899-aabbccddeef",
        "0011223-34455-6677-8899-aabbccddeeff",
        "00112233-4455-6677-8899-aabbccddeegg",
        " 00112233-4455-6677-8899-aabbccddeeff",
        "00000000-0000-0000-0000-000000000000",
    ];
    for text in unknown {
        assert_eq!(server::instance_of(Some(text)), server::NO_INSTANCE, "{text:?}");
    }
    assert_eq!(server::instance_of(None), server::NO_INSTANCE);
    let ah = &b["audioHash"];
    assert_eq!(spaced_of(&hex16(text(&ah["audioHash"])).expect("a digest")), text(&ah["stored"]));
    let dj = golden("autodj-00000029.bin");
    let table = Table::open(&dj).expect("the golden index");
    assert_eq!(spaced_of(&table.sig), text(&b["selectionSigStored"]));
    assert_eq!(hex(&hex16(&hex(&table.sig)).expect("hex")), hex(&table.sig));
    assert_eq!(hex16("0123"), None);
    assert_eq!(hex16("zz23456789abcdef0123456789abcdef"), None);
}

#[test]
fn vectors_canonical_order() {
    let v = vectors();
    let o = &v["canonicalOrder"];
    let siblings: Vec<&str> = o["siblings"].as_array().expect("siblings").iter().map(text).collect();
    let mut sorted = siblings.clone();
    sorted.sort_by(|a, b| compare_names(a.as_bytes(), b.as_bytes()));
    assert_eq!(sorted, siblings);
    let files: Vec<&str> = o["files"].as_array().expect("files").iter().map(text).collect();
    let mut sorted = files.clone();
    sorted.reverse();
    sorted.sort_by(|a, b| compare_file_paths(a, b));
    assert_eq!(sorted, files);
}

#[test]
fn vectors_text() {
    let v = vectors();
    for e in v["stringRuns"].as_array().expect("stringRuns") {
        let (run, _) = Run::build(&raw_tags(&e["fields"]));
        assert_eq!(spaced_of(&run.encode()), text(&e["run"]), "{}", e["fields"]);
    }
    for e in v["fieldRules"].as_array().expect("fieldRules") {
        let f = FIELD_NAMES.iter().position(|n| *n == text(&e["field"])).expect("a field");
        let values: Vec<&str> = e["values"].as_array().expect("values").iter().map(text).collect();
        let (stored, truncated) = build_field(&values, is_list(f));
        assert_eq!(stored, text(&e["stored"]).as_bytes(), "{}", e["field"]);
        assert_eq!(truncated, e["truncated"].as_bool().expect("a bool"), "{}", e["field"]);
        if let Some(n) = e.get("bytes") {
            assert_eq!(stored.len() as u64, num(n));
        }
    }
}

#[test]
fn vectors_numbers() {
    for e in vectors()["numbers"].as_array().expect("numbers") {
        let got: Option<i64> = match text(&e["kind"]) {
            "gain" => numbers::gain(text(&e["text"])).map(i64::from),
            "peak" => numbers::peak(text(&e["text"])).map(i64::from),
            "bpm" => numbers::bpm10(text(&e["text"])).map(i64::from),
            "r128" => Some(numbers::r128_to_gain(int(&e["q"]) as i16) as i64),
            other => panic!("a kind: {other}"),
        };
        assert_eq!(got, e["value"].as_i64(), "{e}");
    }
    // R128 from its tag's text, and the edges of each range.
    assert_eq!(numbers::r128(" -1312 "), Some(-13));
    assert_eq!(numbers::r128("32768"), None);
    assert_eq!(numbers::r128("-32768"), Some(numbers::r128_to_gain(-32768)));
    assert_eq!(numbers::r128("1.5"), None);
    assert_eq!(numbers::gain("-327.68"), Some(-32768));
    assert_eq!(numbers::gain("327.68"), None);
    assert_eq!(numbers::gain("99999999999999999999999"), None);
    assert_eq!(numbers::bpm10("299.5"), Some(3000));
    assert_eq!(numbers::bpm10("-0"), None);
    assert_eq!(numbers::peak("-0.0"), Some(0));
    assert_eq!(numbers::peak(".00005"), Some(1));
    assert_eq!(numbers::bpm10("."), None);
    assert_eq!(numbers::bpm10("+"), None);
}

#[test]
fn vectors_root_election() {
    for e in vectors()["rootElection"].as_array().expect("rootElection") {
        let cand = |c: &Value| {
            c["valid"].as_bool().expect("valid").then(|| Candidate {
                generation: num(&c["generation"]) as u32,
                id: num(&c["commitId"]),
                header_crc: num(&c["headerCrc"]) as u32,
            })
        };
        let got = elect(cand(&e["bin"]), cand(&e["tmp"]));
        let want = match text(&e["pick"]) {
            "bin" => Pick::Bin,
            "tmp" => Pick::Tmp,
            _ => Pick::None,
        };
        assert_eq!(got.pick, want, "{e}");
        assert_eq!(got.same_commit, e["sameCommit"].as_bool().expect("a bool"), "{e}");
        // A file of a newer major: absent to a v1 reader, and the software
        // refuses to write (2.4.5), checked on its first 6 bytes alone.
        if e["softwareRefuses"].as_bool() == Some(true) {
            let mut head = golden("manifest.bin");
            head[4..6].copy_from_slice(&(num(&e["tmp"]["major"]) as u16).to_le_bytes());
            assert!(unknown_major(&head[..6]));
            assert_eq!(Manifest::read(&head, msmf::Uses::ALL).err(), Some(Why::Major));
        }
    }
}

#[test]
fn vectors_album_folder() {
    for e in vectors()["albumFolder"].as_array().expect("albumFolder") {
        let roots: Vec<&str> = e["roots"].as_array().into_iter().flatten().map(text).collect();
        let album = album_folder(text(&e["rel"]), &roots);
        assert_eq!(album, text(&e["album"]), "{}", e["rel"]);
        assert_eq!(path_hash(album), num(&e["hash"]), "{}", e["rel"]);
    }
}

#[test]
fn vectors_software_matching() {
    // These four bind the matching of 2.10.2 and the names of 2.8.3; what of
    // them is the kit's is that a name is recorded as stored and hashed so:
    // the NFC spelling of a macOS listing's `Cafe` + U+0301.
    let v = vectors();
    assert_eq!(v["softwareMatching"]["who"], "software");
    assert_eq!(v["softwareMatching"]["cases"].as_array().map(Vec::len), Some(4));
    assert_eq!(path_hash("Caf\u{e9}/Album/01 - Title.mp3"), 0xF1B2_4FC7_5749_4F2B);
    assert_eq!(path_hash("Cafe\u{301}/Album/01 - Title.mp3"), 0x5701_7C0C_C011_BB1B);
}

fn talk_of(t: &Value) -> Talk {
    Talk {
        url_key: url_key(&t["url"]),
        instance: uuid(&t["serverInstance"]),
        confirmed: t["confirmed"].as_array().into_iter().flatten().map(num).collect(),
        peer: t.get("peerId").map(|id| TalkPeer { id: num(id) as u32, instance: uuid(&t["peerInstance"]) }),
    }
}

fn verdict(v: &Value) -> Verdict {
    match text(v) {
        "same" => Verdict::Same,
        "different" => Verdict::Different,
        "unknown" => Verdict::Unknown,
        other => panic!("a verdict: {other}"),
    }
}

#[test]
fn vectors_servers() {
    let v = vectors();
    let s = &v["servers"];
    for e in s["origServer"].as_array().expect("origServer") {
        let row = Ledger { server: num(&e["server"]) as u16, ..Ledger::default() };
        let (meta, mut records, _) = tags_input(&library("tags-00000007"));
        records.truncate(1);
        records[0].ledger = Some(row);
        let file = mptg::write(&meta, &records, &[]).expect("a tags file");
        let tags = mptg::read(&file, mptg::Uses::ALL).expect("read back");
        assert_eq!(tags.records[0].ledger.as_ref().map(|l| l.server), Some(num(&e["server"]) as u16));
        // Bytes 74-79 of the row, read straight from the file.
        assert_eq!(spaced_of(&section_of(&file, mptg::ORIG)[74..80]), text(&e["bytes74to79"]));
    }

    let rows = &s["rows"];
    let tags = mptg::read(&golden(text(&rows["file"])), mptg::Uses::ALL).expect("the golden tags file");
    let got: Vec<(String, u16)> =
        tags.records.iter().map(|e| (e.path.clone(), e.ledger.as_ref().expect("a ledger").server)).collect();
    let want: Vec<(String, u16)> = rows["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| (text(&r["path"]).to_owned(), num(&r["server"]) as u16))
        .collect();
    assert_eq!(got, want);

    let srvs = &s["srvs"];
    let m = Manifest::read(&golden(text(&srvs["file"])), msmf::Uses::ALL).expect("the golden root");
    let stored: Vec<String> = m.servers.iter().map(|e| spaced_of(&e.encode())).collect();
    let entries = srvs["entries"].as_array().expect("entries");
    let want: Vec<String> = entries.iter().map(|e| text(&e["stored"]).to_owned()).collect();
    assert_eq!(stored, want);
    assert_eq!(num(&srvs["stride"]), 40);
    // The root and its tags file agree: SRVS lists exactly the servers the
    // rows name besides the root's.
    assert!(msmf::servers_match_rows(m.servers.len(), got.iter().map(|(_, s)| *s)));
    assert!(!msmf::servers_match_rows(2, got.iter().map(|(_, s)| *s)));
    assert!(!msmf::servers_match_rows(4, got.iter().map(|(_, s)| *s)));

    let same = &s["sameServer"];
    let card = library(text(&same["card"]).trim_end_matches(".json"));
    let servers = servers_of(&card);
    assert_eq!(servers, m.server_table(), "the golden root is its description's");
    for e in same["cases"].as_array().expect("cases") {
        let index = num(&e["index"]) as usize;
        assert_eq!(servers.same_server(index, &talk_of(&e["talk"])), verdict(&e["verdict"]), "{e}");
    }
    for e in s["brokenEntries"]["cases"].as_array().expect("brokenEntries") {
        let servers = servers_of(&e["card"]);
        let index = num(&e["index"]) as usize;
        assert_eq!(servers.same_server(index, &talk_of(&e["talk"])), verdict(&e["verdict"]), "{}", e["note"]);
    }
}

#[test]
fn vectors_device_txt() {
    let v = vectors();
    let d = &v["deviceTxt"];
    let file = DeviceTxt::read(Some(text(&d["text"]).as_bytes()));
    assert!(file.present);
    assert_eq!(file.to_text(), text(&d["text"]));
    assert!(file.decodes("OPUS") && file.lists("opus") && file.writes_mpdj());
    assert!(file.reads(Format::Msmf, 1) && !file.reads(Format::Mpdj, 1) && file.reads(Format::Mpdj, 2));
    // 0.12.0 keeps a play log: `history` lists major 1 (2.15).
    assert!(file.logs_plays() && file.history == 1);
    // No file: the defaults, which noFile lists key by key; no play log.
    let none = DeviceTxt::read(None);
    let text_of = none.to_text();
    let keys: BTreeMap<&str, &str> = text_of.lines().filter_map(|l| l.split_once('=')).collect();
    for (k, want) in d["noFile"].as_object().expect("noFile") {
        assert_eq!(keys.get(k.as_str()).copied(), Some(text(want)), "{k}");
    }
    assert!(!none.present && !none.writes_mpdj() && !none.decodes("opus") && !none.logs_plays());
    assert!(!keys.contains_key("history"));
    // A file with no contract line isn't one; a key missing keeps its
    // default; an empty value says none; CRLF is read.
    assert_eq!(DeviceTxt::read(Some(b"read.mpdj=2\n")), DeviceTxt::defaults());
    let t = DeviceTxt::read(Some(b"contract=1\r\ncodecs=mp3,flac,opus\r\nread.mpth=\r\n"));
    assert!(t.present && t.decodes("opus") && !t.reads(Format::Mpth, 1) && t.reads(Format::Mptg, 1));
    assert_eq!(t.max_rate, 48_000);
    assert!(DeviceTxt::read(Some(b"contract=1\nread.mpdj= 1 , 2\n")).writes_mpdj());
    assert!(!DeviceTxt::read(Some(b"contract=1\nread.mpdj=x2\n")).writes_mpdj());
}

/// The play log's parts the kit reads: MSMF's ACKS (2.5.6) and device.txt's
/// `history` key (2.15).
#[test]
fn vectors_plays_acks_and_history() {
    let v = vectors();
    let a = &v["plays"]["acks"];
    // One entry's bytes.
    let e = &a["entry"];
    let ack = acks_of(&Value::Array(vec![e.clone()]))[0];
    assert_eq!(spaced_of(&ack.encode()), text(&e["stored"]));
    assert_eq!(Ack::decode(&spaced(text(&e["stored"]))), ack);
    // The golden root's entries in ACKS's order, unsigned epochs (its top
    // bit set, EPOCH_2 sorts after EPOCH_1); the description gives them out
    // of order. The device before its play log skips the section.
    let g = &a["golden"];
    let file = golden(text(&g["file"]));
    let m = Manifest::read(&file, msmf::Uses::ALL).expect("the golden root");
    assert_eq!(m.acks, acks_of(&g["entries"]));
    assert_eq!((num(&g["stride"]), section_of(&file, msmf::ACKS).len()), (24, 24 * m.acks.len()));
    assert!(Manifest::read(&file, msmf::Uses::DEVICE).expect("the root").acks.is_empty());
    // Each commit carries the root before's entries forward (2.5.6), in any
    // order given; a device and epoch twice is refused.
    assert!(msmf::write(&m.to_input()).expect("carried forward") == file);
    let mut reversed = m.to_input();
    reversed.acks.reverse();
    assert!(msmf::write(&reversed).expect("sorted") == file);
    let mut twice = m.to_input();
    twice.acks.push(Ack { seq: 9, ..twice.acks[0] });
    assert!(msmf::write(&twice).is_err());
    // The hardening files: the root reads, its ACKS taken or ignored.
    for h in a["hardening"].as_array().expect("hardening") {
        let name = text(&h["file"]);
        assert_eq!(h["root"], "present", "{name}");
        let m = Manifest::read(&bytes(&format!("hardening/{name}")), msmf::Uses::ALL).expect(name);
        assert_eq!(m.acks, acks_of(&h["acks"]), "{name}: {}", h["note"]);
    }
    // Rows of a newer minor, carried forward: this writer's 24-byte rows.
    let minor = bytes("hardening/msmf-acks-minor-1.bin");
    let minor = Manifest::read(&minor, msmf::Uses::ALL).expect("a newer minor");
    assert!(msmf::write(&minor.to_input()).expect("carried forward") == file);

    for c in v["plays"]["deviceTxt"].as_array().expect("deviceTxt") {
        let t = DeviceTxt::read(c["text"].as_str().map(str::as_bytes));
        let want = c["history"].as_array().expect("majors").iter().fold(0, |m, h| m | 1 << (num(h) - 1));
        assert_eq!((t.history, t.logs_plays()), (want, want != 0), "{}", c["text"]);
        if t.present {
            assert_eq!(DeviceTxt::read(Some(t.to_text().as_bytes())), t, "{}", c["text"]);
        }
    }
}

/// A root's ACKS damaged anywhere in its rows: the root still reads, the
/// same but for its acknowledgements, which are ignored (2.5.6). Anywhere
/// else a flipped bit makes the root absent, but in a gap no CRC covers.
#[test]
fn a_damaged_acks_never_makes_the_root_absent() {
    let file = golden("manifest-acks.bin");
    let whole = Manifest::read(&file, msmf::Uses::ALL).expect("the golden root");
    let (off, len) = section_span(&file, msmf::ACKS);
    let skip = gaps(&file);
    let mut b = file.clone();
    for at in 0..b.len() {
        for bit in 0..8 {
            b[at] ^= 1 << bit;
            let read = Manifest::read(&b, msmf::Uses::ALL);
            if (off..off + len).contains(&at) {
                let m = read.expect("the root reads");
                assert!(m.acks.is_empty(), "byte {at} bit {bit}");
                assert_eq!(Manifest { acks: whole.acks.clone(), ..m }, whole);
            } else if skip[at] {
                assert_eq!(read.ok().as_ref(), Some(&whole), "a gap's byte {at}");
            } else {
                assert!(read.is_err(), "byte {at} bit {bit}");
            }
            b[at] ^= 1 << bit;
        }
    }
    // Resealed, so its CRC passes: two entries swapped are out of order.
    let mut swapped = file.clone();
    swapped[off..off + 48].copy_from_slice(&[&file[off + 24..off + 48], &file[off..off + 24]].concat());
    reseal(&mut swapped);
    assert!(Manifest::read(&swapped, msmf::Uses::ALL).expect("the root reads").acks.is_empty());
}

/// A play slot's fields as the fixtures describe them.
fn slot_of(f: &Value) -> mppl::Slot {
    mppl::Slot {
        kind: num(&f["kind"]) as u8,
        outcome: num(&f["outcome"]) as u8,
        source: num(&f["source"]) as u8,
        seq: num(&f["seq"]) as u32,
        pause_count: num(&f["pauseCount"]) as u16,
        clock: num(&f["clock"]) as u8,
        epoch: num(&f["epoch"]),
        device_id: device_id(&f["deviceId"]),
        started_at: num(&f["startedAt"]),
        boot_seq: num(&f["bootSeq"]) as u32,
        uptime_ms: num(&f["uptimeMs"]) as u32,
        played_ms: num(if f["kind"] == 2 { &f["dropped"] } else { &f["playedMs"] }) as u32,
        duration_ms: num(&f["durationMs"]) as u32,
        path_hash: num(&f["pathHash"]),
        size: num(&f["size"]) as u32,
        fat_time: num(&f["fatTime"]) as u32,
        generation: num(&f["generation"]) as u32,
    }
}

/// A case's files of plays/index.json, by name.
fn play_files(case: &Value) -> Vec<(String, Vec<u8>)> {
    let files = case["files"].as_object().expect("files");
    files.iter().map(|(name, at)| (name.clone(), bytes(&format!("plays/{}", text(at))))).collect()
}

fn read_plays(files: &[(String, Vec<u8>)]) -> mppl::Log {
    let files: Vec<mppl::LogFile<'_>> = files.iter().map(|(n, b)| mppl::LogFile::whole(n, b)).collect();
    mppl::read_log(&files)
}

/// The play log's slot, a drop note, the ids and the enums' names (2.20.3,
/// 2.20.4): each slot written from its fields is the fixtures' bytes, and
/// read back is its fields.
#[test]
fn vectors_plays_slots_and_ids() {
    let v = vectors();
    let p = &v["plays"];
    let s = &p["slot"];
    let slot = slot_of(&s["fields"]);
    let bytes = slot.encode();
    assert_eq!(spaced_of(&bytes), text(&s["bytes"]));
    assert_eq!(crc32(&bytes[..76]) as u64, num(&s["crc"]));
    assert_eq!(mppl::decode(&bytes), mppl::Verdict::Ok(slot));
    assert_eq!(slot.id(), text(&s["id"]));
    let note = slot_of(&p["dropNote"]["fields"]);
    assert_eq!(spaced_of(&note.encode()), text(&p["dropNote"]["bytes"]));
    assert_eq!(mppl::decode(&note.encode()), mppl::Verdict::Ok(note));
    for c in p["ids"].as_array().expect("ids") {
        let slot = mppl::Slot {
            device_id: device_id(&c["deviceId"]),
            epoch: num(&c["epoch"]),
            seq: num(&c["seq"]) as u32,
            ..mppl::Slot::default()
        };
        assert_eq!(slot.id(), text(&c["id"]));
        assert!(slot.id().len() <= 46);
    }
    for (value, name) in p["names"]["outcome"].as_object().expect("outcomes") {
        assert_eq!(mppl::outcome_name(value.parse().unwrap()), text(name));
    }
    for (value, name) in p["names"]["source"].as_object().expect("sources") {
        assert_eq!(mppl::source_name(value.parse().unwrap()), text(name));
    }
    // The reader's verdicts in its order: a tail, another magic, a newer
    // major (its CRC unchecked), a bad CRC, an unknown kind.
    assert_eq!(mppl::decode(&bytes[..79]), mppl::Verdict::Short);
    let with = |at: usize, value: u8| {
        let mut b = bytes;
        b[at] = value;
        b
    };
    assert_eq!(mppl::decode(&with(0, b'X')), mppl::Verdict::Damaged);
    assert_eq!(mppl::decode(&with(4, 2)), mppl::Verdict::Newer(2));
    assert_eq!(mppl::decode(&with(40, 9)), mppl::Verdict::Damaged);
    let mut kind = with(5, 9);
    let crc = crc32(&kind[..76]);
    kind[76..].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(mppl::decode(&kind), mppl::Verdict::Kind(9));
}

/// plays/index.json's cases through the reader: the plays each once, by
/// their ids in order, what it skipped counted as tools/playlog.py counts
/// it, the enums' names and the dates; and every valid slot of every file
/// of the fixtures written again from its fields is its own bytes.
#[test]
fn the_play_logs_goldens_read_as_the_reference_reads_them() {
    let cases = json("plays/index.json");
    for case in cases.as_array().expect("cases") {
        let name = text(&case["case"]);
        let log = read_plays(&play_files(case));
        let ids: Vec<String> = log.plays.iter().map(mppl::Slot::id).collect();
        let want: Vec<&str> = case["ids"].as_array().expect("ids").iter().map(text).collect();
        assert_eq!(ids, want, "{name}");
        let c = &case["counts"];
        let counts = mppl::Counts {
            slots: num(&c["slots"]),
            damaged: num(&c["damaged"]),
            tail: num(&c["tail"]),
            newer: num(&c["newer"]),
            unknown_kinds: num(&c["unknownKinds"]),
            unusable: num(&c["unusable"]),
            twins: num(&c["twins"]),
            conflicts: num(&c["conflicts"]),
            dropped: num(&c["dropped"]),
        };
        assert_eq!(log.counts, counts, "{name}: {}", case["note"]);
        for (id, names) in case["names"].as_object().into_iter().flatten() {
            let play = log.plays.iter().find(|p| p.id() == *id).expect("the play");
            let got = [mppl::outcome_name(play.outcome), mppl::source_name(play.source)];
            let want: Vec<&str> = names.as_array().expect("two names").iter().map(text).collect();
            assert_eq!(got.as_slice(), want.as_slice(), "{name}: {id}");
        }
        let dates = mppl::dates(&log.plays);
        for (id, when) in case["dates"].as_object().into_iter().flatten() {
            let play = log.plays.iter().find(|p| p.id() == *id).expect("the play");
            assert_eq!(dates[&play.key()], when.as_u64(), "{name}: {id}");
        }
        // The order of the files changes nothing.
        let mut files = play_files(case);
        files.reverse();
        assert_eq!(read_plays(&files), log, "{name}");
        for (file, data) in play_files(case) {
            for raw in data.chunks(mppl::SLOT) {
                if let mppl::Verdict::Ok(slot) = mppl::decode(raw) {
                    assert_eq!(slot.encode().as_slice(), raw, "{name}: {file}");
                }
            }
        }
    }
    // A name a reader doesn't read (a twin's) is left out.
    let golden = bytes("plays/golden/sync-1/plays.jnl");
    let twin = mppl::read_log(&[mppl::LogFile::whole("plays.xl1", &golden)]);
    assert_eq!(twin, mppl::Log::default());
}

/// A file longer than the cap: what is past it is counted as damaged slots,
/// one for each 80 bytes or part of them, and never read, whether the
/// reader was given those bytes or only the file's length (2.20.6, item 1).
#[test]
fn a_play_log_past_its_cap_is_read_to_the_cap() {
    let golden = bytes("plays/golden/sync-1/plays.jnl");
    let mut long = vec![0u8; mppl::FILE_BYTES - golden.len()];
    long.extend_from_slice(&golden);
    let played = read_plays(&[("plays.jnl".into(), long.clone())]);
    assert_eq!(played.plays.len(), 3, "the golden's three plays, at the cap's end");
    long.extend_from_slice(&golden[..81]);
    let past = read_plays(&[("plays.jnl".into(), long.clone())]);
    let cap = mppl::FILE_SLOTS as u64;
    assert_eq!((past.counts.slots, past.counts.damaged), (cap + 2, cap - 3 + 2));
    assert_eq!(past.plays, played.plays, "the slot past the cap isn't read");
    let head = &long[..mppl::FILE_BYTES];
    let given = mppl::LogFile { name: "plays.jnl", bytes: head, size: long.len() as u64 };
    assert_eq!(mppl::read_log(&[given]), past);
}

/// The posted mark of each device and epoch rises over the plays settled
/// or not sent, and stops at the first that waits; a seq the log doesn't
/// hold doesn't hold it back, and a mark is never lowered (2.20.6, item 10).
#[test]
fn a_posted_mark_rises_to_the_first_play_still_waiting() {
    let one = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x01];
    let two = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x02];
    let play = |device_id, epoch, seq| mppl::Slot { kind: 1, device_id, epoch, seq, ..mppl::Slot::default() };
    let plays = [play(one, 7, 1), play(one, 7, 2), play(one, 7, 4), play(one, 7, 5), play(two, 9, 1)];
    let ack = |device_id, epoch, seq| Ack { device_id, epoch, seq };
    let every = mppl::next_marks(&plays, &[], &|_| true);
    assert_eq!(every, [ack(one, 7, 5), ack(two, 9, 1)], "seq 3 lost: no hold");
    let waits = mppl::next_marks(&plays, &[], &|p| p.seq != 4);
    assert_eq!(waits, [ack(one, 7, 2), ack(two, 9, 1)]);
    let posted = [ack(one, 7, 4), ack(one, 3, 8)];
    let kept = mppl::next_marks(&plays, &posted, &|p| p.seq != 5);
    assert_eq!(kept, [ack(one, 3, 8), ack(one, 7, 4), ack(two, 9, 1)], "never lowered, another epoch's kept");
    assert_eq!(mppl::next_marks(&plays, &[], &|_| false), []);
}

#[test]
fn vectors_mpdj() {
    let v = vectors();
    let m = &v["mpdj"];
    for e in m["levels"].as_array().expect("levels") {
        assert_eq!(mpdj::level(e["cos"].as_f64().expect("a cosine")) as u64, num(&e["level"]), "{e}");
    }
    for e in m["floors"]["cases"].as_array().expect("floors") {
        assert_eq!(mpdj::floor_level(num(&e["percent"]) as u32) as u64, num(&e["level"]));
    }
    for e in m["slotBytes"].as_array().expect("slotBytes") {
        assert_eq!(mpdj::slot_bytes_for(num(&e["k"]) as u32) as u64, num(&e["slotBytes"]), "{e}");
    }
    for e in m["headerBytes"].as_array().expect("headerBytes") {
        let field = |k: &str| num(&e[k]) as u32;
        let hb = mpdj::header_bytes_for(field("sectionCount"), field("slotBytes"), field("minimum"));
        assert_eq!(hb as u64, num(&e["headerBytes"]), "{e}");
    }
    for e in m["stridedRanks"].as_array().expect("stridedRanks") {
        let (k, head, mm) = (num(&e["k"]) as u32, num(&e["head"]) as u32, num(&e["m"]) as u32);
        let ranks: Vec<u64> = (0..k).map(|j| mpdj::strided_rank(j, k, head, mm) as u64).collect();
        if let Some(want) = e["ranks"].as_array() {
            assert_eq!(ranks, want.iter().map(num).collect::<Vec<_>>(), "{k} {head} {mm}");
        } else {
            for p in e["pairs"].as_array().expect("pairs") {
                assert_eq!(ranks[num(&p["j"]) as usize], num(&p["rank"]), "{p}");
            }
        }
    }
    let sc = &m["slotCrc"];
    let file = golden(text(&sc["file"]));
    let table = Table::open(&file).expect("the golden index");
    let raw = table.slot_raw(num(&sc["row"]) as u32).expect("the slot");
    assert_eq!(mpdj::slot_crc(raw, num(&sc["row"]) as u32) as u64, num(&sc["crc"]));
    assert_eq!(crc32(&raw[..raw.len() - 4]) as u64, num(&sc["unseeded"]));

    let sig = &m["signature"];
    assert_eq!(hex(&mpdj::signature(text(&sig["text"]))), text(&sig["sig"]));
    let signed_file = golden(text(&sig["file"]));
    let signed = Table::open(&signed_file).expect("the golden index");
    assert_eq!(hex(&signed.sig), text(&sig["sig"]));

    // The vectors of the index's rows, in row order: by their hash's bytes.
    let vec = &m["vec"];
    let d = library(text(&vec["file"]).trim_end_matches(".bin"));
    let mut described: Vec<&Value> = d["rows"].as_array().expect("rows").iter().collect();
    described.sort_by_key(|r| hex16(text(&r["hash"])).expect("a hash"));
    let embedding = |r: &Value| -> Vec<f32> {
        let values = r["embedding"].as_array().expect("an embedding");
        values.iter().map(|x| x.as_f64().expect("a float") as f32).collect()
    };
    let rows: Vec<Vec<f32>> = described.into_iter().map(embedding).collect();
    assert!(rows.iter().all(|r| r.len() as u64 == num(&vec["dim"])));
    assert_eq!(hex(&mpdj::vec_digest(rows.iter().map(Vec::as_slice))), text(&vec["sha256"]));

    for e in m["deviceTxt"]["cases"].as_array().expect("deviceTxt") {
        let t = if e["noFile"].as_bool() == Some(true) {
            DeviceTxt::read(None)
        } else if e["keyMissing"].as_bool() == Some(true) {
            DeviceTxt::read(Some(b"contract=1\nfirmware=0.11.0\n"))
        } else {
            DeviceTxt::read(Some(format!("contract=1\nread.mpdj={}\n", text(&e["readMpdj"])).as_bytes()))
        };
        assert_eq!(t.writes_mpdj(), e["writesV2"].as_bool().expect("a bool"), "{e}");
    }

    // The float32 trap (13.5): the rule's level of the two rows is 140, a
    // float32 sum's 139. The sidecar holds the vectors in the description's
    // order; a row is the index's, the rows sorted by their hash's bytes.
    let tv = &m["trap"];
    let d = library(text(&tv["file"]).trim_end_matches(".bin"));
    let dim = num(&d["dim"]) as usize;
    let f32s = bytes(&format!("libraries/{}", text(&d["vectors"])));
    let mut order: Vec<(Vec<u8>, usize)> = d["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .enumerate()
        .map(|(i, r)| (hex16(text(&r["hash"])).expect("a hash").to_vec(), i))
        .collect();
    order.sort();
    let row = |r: u64| -> Vec<f32> {
        let at = order[r as usize].1 * dim * 4;
        f32s[at..at + dim * 4].chunks(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    };
    let (a, b) = (row(num(&tv["rowA"])), row(num(&tv["rowB"])));
    assert_eq!(mpdj::cosine(&a, &b), tv["cos"].as_f64().expect("a cosine"));
    assert_eq!(mpdj::level_of(&a, &b) as u64, num(&tv["level"]));
    let s32 = a.iter().zip(&b).fold(0f32, |s, (x, y)| s + x * y);
    assert_eq!(s32 as f64, tv["cosF32"].as_f64().expect("a cosine"));
    assert_eq!(mpdj::level(s32 as f64) as u64, num(&tv["levelF32"]));
}

// ── The goldens (2.17, item 2) ──────────────────────────────────────────────

/// Every golden is written byte for byte from its description by this
/// kit's writers — the tags files, the roots, the plan, the thumbnail (from
/// the synthetic pixels) and device.txt — and every Auto DJ index (whose
/// builder is the Auto DJ writer's) reads as valid, its frame laid out the
/// same again by the container writer.
#[test]
fn goldens_are_written_byte_for_byte() {
    let mut seen = BTreeSet::new();
    let mut names: Vec<String> = std::fs::read_dir(fixture("libraries"))
        .expect("libraries")
        .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    for name in &names {
        let d = library(name.trim_end_matches(".json"));
        let file = text(&d["file"]).to_owned();
        let want = golden(&file);
        let got = match text(&d["format"]) {
            "MPTG" => {
                let (meta, records, folders) = tags_input(&d);
                mptg::write(&meta, &records, &folders).expect("a tags file")
            }
            "MSMF" => msmf::write(&manifest_input(&d)).expect("a root"),
            "MSPD" => mspd::write(&plan_input(&d)).expect("a plan"),
            "MPTH" => {
                let px = mpth::Pixels { small: thumb_pixels(mpth::SMALL), large: thumb_pixels(mpth::LARGE) };
                let h = path_hash(text(&d["folder"]));
                assert_eq!(file, thumb_path(h).rsplit('/').next().expect("a name"));
                mpth::write(h, num(&d["sourceBytes"]) as u32, &px).expect("a thumbnail")
            }
            "device.txt" => device_input(&d).to_text().into_bytes(),
            "MPDJ" => relaid_index(&want),
            other => panic!("a format: {other}"),
        };
        assert!(got == want, "{file}: the writer's bytes differ from the golden's");
        seen.insert(file);
    }
    let goldens: BTreeSet<String> = std::fs::read_dir(fixture("golden"))
        .expect("golden")
        .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(seen, goldens, "a golden with no description, or the other way");
    assert_eq!(goldens.len(), 18);
}

/// An Auto DJ golden read and checked whole, then laid out again from its
/// own header and sections: the alignment rule's headerBytes, the
/// directory, the gaps, every CRC.
fn relaid_index(file: &[u8]) -> Vec<u8> {
    let t = Table::open(file).expect("the golden index");
    t.check(mpdj::Uses::ALL).expect("a whole index");
    let h = *t.header();
    let sections = t.sections();
    let hb = mpdj::header_bytes_for(sections.len() as u32, t.slot_bytes, mpdj::MIN_HEADER_BYTES);
    assert_eq!(hb, h.header_bytes, "the smallest headerBytes");
    let out: Vec<container::SectionOut> = sections
        .iter()
        .map(|&(kind, flags, count, stride, data)| container::SectionOut { kind, flags, count, stride, data })
        .collect();
    let (major, minor, generation, card_id) = (h.major, h.minor, h.generation, h.card_id);
    let meta = container::FileMeta { magic: MPDJ, major, minor, generation, card_id };
    container::write(&meta, &file[40..mpdj::MIN_HEADER_BYTES as usize], hb, &out).expect("the frame")
}

#[test]
fn goldens_read_back() {
    // The fixture card's root names its tags file and its index, each valid
    // as its companion (2.5.3).
    let root = Manifest::read(&golden("manifest.bin"), msmf::Uses::ALL).expect("the root");
    assert!(root.is_final());
    assert_eq!(root.roots, ["Lib A", "Lib B"]);
    assert_eq!(root.server_url_key, server_url_key("HTTP://Music.Example:3000/"));
    assert_eq!(root.producer, "mstream-terminal 0.13.0");
    let tags_file = golden(&root.tags.name());
    let tags = mptg::read(&tags_file, mptg::Uses::ALL).expect("its tags file");
    assert!(msmf::companion_matches(&root.tags, &tags.header, tags.source, None));
    let dj = root.autodj.clone().expect("an index");
    assert_eq!(dj.name(), "autodj-00000029.bin");
    let dj_file = golden(&dj.name());
    let table = Table::open(&dj_file).expect("its index");
    assert!(msmf::companion_matches(&dj, table.header(), 0, Some(&table.sig)));
    // Not with another key, nor a device's records.
    assert!(!msmf::companion_matches(&dj, table.header(), 0, Some(&[0; 16])));
    assert!(!msmf::companion_matches(&root.tags, &tags.header, 1, None));
    let min = Manifest::read(&golden("manifest-min.bin"), msmf::Uses::ALL).expect("the first commit");
    assert!(!min.is_final() && min.autodj.is_none() && min.server_url_key == 0);
    let first = mptg::read(&golden("tags-00000001.bin"), mptg::Uses::ALL).expect("its tags file");
    assert!(first.records.is_empty() && !first.has_ledger);
    let folders: Vec<(&str, u32)> = first.folders.iter().map(|f| (f.path.as_str(), f.flags)).collect();
    assert_eq!(folders, [("", 0), ("Owned Empty", 1)]);
    // The election between the two roots: the higher generation.
    assert_eq!(elect(Some(min.candidate()), Some(root.candidate())).pick, Pick::Tmp);
    assert!(elect(Some(root.candidate()), Some(root.candidate())).same_commit);

    // What a record holds, read back: the fixture's every-field record, the
    // folder flags, the hashes, the ledger.
    let e = &tags.records.iter().find(|e| e.path == "Artist/Album/01 - Title.mp3").expect("the record");
    assert_eq!(e.hash, 0xDA70_B312_7601_88D8);
    let artists: Vec<&[u8]> = e.run.values(super::text::ARTIST).collect();
    assert_eq!(artists, [&b"Lantern Choir"[..], &b"Guest Voice"[..]]);
    let r = &e.rec;
    assert_eq!((r.bpm10, r.camelot_key(), r.rg_track_gain, r.compilation()), (1210, 8, -679, 2));
    let l = e.ledger.as_ref().expect("a ledger");
    let row = (l.server_path.as_str(), l.mstream_id, l.server);
    assert_eq!(row, ("Music/Artist/Album/01 - Title.mp3", 100, 0));
    assert_eq!(l.canonical_hash(), Some(l.audio_hash));
    let album = tags.folders.iter().find(|f| f.path == "Artist/Album").expect("the album");
    assert_eq!((album.flags, album.hash), (mptg::FOLDER_THUMB, 0xB1F7_E69F_BD46_6B59));
    let truncated: Vec<&str> =
        tags.records.iter().filter(|e| e.rec.flags & mptg::TRUNCATED != 0).map(|e| e.path.as_str()).collect();
    assert_eq!(truncated, ["A-/z.flac", "B/w.opus", "a/v.mp3"]);
    // The canonical order: `A` and its files before `A B`.
    let order: Vec<&str> = tags.records.iter().map(|e| e.path.as_str()).take(4).collect();
    assert_eq!(order, ["Loose Intro.mp3", "A/x.mp3", "A B/y.mp3", "A-/z.flac"]);
    // A device's tags file: no ledger.
    let device = mptg::read(&golden("tags-device.bin"), mptg::Uses::ALL).expect("a device's records");
    assert!(device.source == mptg::SOURCE_DEVICE && !device.has_ledger && device.records.len() == 4);

    // The plan, in 2.12.5's order.
    let plan = Plan::read(&golden("pending.bin")).expect("the plan");
    let ops: Vec<(u8, &str)> = plan.ops.iter().map(|o| (o.op, o.path.as_str())).collect();
    assert_eq!(ops, [
        (2, "A/x.mp3"),
        (2, "Artist/x.mp3"),
        (3, "A B/Sub"),
        (3, "New Artist"),
        (3, "New Artist/New Album"),
        (1, "Loose New.mp3"),
        (1, "A/z.mp3"),
        (1, "A B/y.mp3"),
        (1, "New Artist/New Album/01 - New.mp3"),
    ]);
    assert!(!plan.stops_software() && plan.run_id == 0x00C0_FFEE_00C0_FFEE && plan.base_generation == 42);
    // A pending.tmp of a higher generation wins.
    let tmp = Candidate { generation: plan.header.generation + 1, ..plan.candidate() };
    assert_eq!(elect(Some(plan.candidate()), Some(tmp)).pick, Pick::Tmp);

    // The thumbnail is its folder's, and no other's.
    let thumb = golden("B1F7E69F.565");
    assert!(mpth::check(&thumb, path_hash("Artist/Album")));
    assert!(!mpth::check(&thumb, path_hash("Artist/Album/CD1")));
    assert!(!mpth::check(&thumb[..thumb.len() - 1], path_hash("Artist/Album")));

    // The device reads the roots and the tags files without the sections it
    // ignores.
    for name in ["manifest.bin", "manifest-servers.bin", "manifest-min.bin", "manifest-acks.bin"] {
        let m = Manifest::read(&golden(name), msmf::Uses::DEVICE).expect("a root");
        assert!(m.servers.is_empty() && m.acks.is_empty());
    }
    let t = mptg::read(&tags_file, mptg::Uses::DEVICE).expect("the tags");
    assert!(!t.has_ledger && t.records.iter().all(|e| e.ledger.is_none()));

    // No contract file of the fixtures has an unknown major.
    for name in ["manifest.bin", "tags-0000002a.bin", "pending.bin", "autodj-00000029.bin"] {
        assert!(!unknown_major(&golden(name)), "{name}");
    }
    assert!(MAX_FILE >= 34 * 1024 * 1024);
}

// ── Hardening (2.17, item 4) ────────────────────────────────────────────────

/// Two tags files read the same: the same folders and records, offsets
/// aside, and the same ledger but for each row's server (the guard's, not
/// the record's).
fn assert_same_tags(want: &Tags, got: &Tags, what: &str) {
    assert_eq!(want.folders, got.folders, "{what}");
    assert_eq!(want.records.len(), got.records.len(), "{what}");
    for (a, b) in want.records.iter().zip(&got.records) {
        let record = |e: &mptg::Entry| (e.path.clone(), e.hash, e.folder, e.rec, e.run.clone());
        assert_eq!(record(a), record(b), "{what}");
        let strip = |l: &Option<Ledger>| l.clone().map(|l| Ledger { server: 0, ..l });
        assert_eq!(strip(&a.ledger), strip(&b.ledger), "{what}: {}", a.path);
    }
}

/// Two Auto DJ indexes read the same (the hardening's `same`): every row's
/// filter record and slot entries, every DJPH hash's row, the five strings.
fn assert_same_index(want: &[u8], got: &[u8], what: &str) {
    let (w, g) = (Table::open(want).expect("the golden"), Table::open(got).expect(what));
    assert_eq!((w.row_count, w.k), (g.row_count, g.k), "{what}");
    for r in 0..w.row_count {
        assert_eq!(w.filter(r), g.filter(r), "{what}: row {r}");
        assert_eq!(w.slot(r), g.slot(r), "{what}: row {r}");
    }
    assert_eq!(w.paths(), g.paths(), "{what}");
    assert_eq!(w.strings(), g.strings(), "{what}");
}

#[test]
fn hardening_files_read_as_listed() {
    let index = json("hardening/index.json");
    let entries = index.as_array().expect("the index");
    assert_eq!(entries.len(), 132);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for e in entries {
        let file = text(&e["file"]);
        let b = bytes(&format!("hardening/{file}"));
        let format = text(&e["format"]);
        let device = text(&e["uses"]) == "device";
        let what = format!("{file}: {}", text(&e["check"]));
        assert_eq!(read_as(format, &b, device).name(), text(&e["why"]), "{what}");
        assert_eq!(text(&e["expect"]) == "present", text(&e["why"]) == "ok", "{what}");
        *counts.entry(format.to_owned()).or_default() += 1;
        if let Some(same) = e.get("same").and_then(Value::as_str) {
            if format == "MPDJ" {
                assert_same_index(&golden(same), &b, &what);
            } else {
                let want = mptg::read(&golden(same), mptg::Uses::ALL).expect("the golden");
                let got = mptg::read(&b, mptg::Uses::ALL).expect(&what);
                assert_same_tags(&want, &got, &what);
            }
        }
        if let Some(slot) = e.get("slot") {
            // One slot as the device reads it, the join's sections good:
            // damaged (its CRC) or broken (its list, under a good CRC).
            let t = Table::open(&b).expect(&what);
            t.check(mpdj::Uses::DEVICE).expect(&what);
            let want = match text(&slot["read"]) {
                "damaged" => mpdj::SlotRead::Damaged,
                "broken" => mpdj::SlotRead::Broken,
                other => panic!("a read: {other}"),
            };
            let (read, why) = t.slot(num(&slot["row"]) as u32).expect_err(&what);
            assert_eq!(read, want, "{what}");
            if text(&e["why"]) != "ok" {
                assert_eq!(why.name(), text(&e["why"]), "{what}");
            }
        }
        // The writer's guard reads the magic and the major alone.
        let major = text(&e["why"]) == "major";
        assert_eq!(unknown_major(&b), major && file != "mpdj-major-1.bin", "{what}");
    }
    assert_eq!(counts.into_iter().collect::<Vec<_>>(), [
        ("MPDJ".to_owned(), 50),
        ("MPTG".to_owned(), 52),
        ("MSMF".to_owned(), 21),
        ("MSPD".to_owned(), 9),
    ]);

    // The files a present verdict says more about.
    let plan = Plan::read(&bytes("hardening/mspd-op-9.bin")).expect("a newer writer's plan");
    assert!(plan.stops_software());
    let m = Manifest::read(&bytes("hardening/msmf-comp-unknown-kind.bin"), msmf::Uses::ALL).expect("a root");
    assert_eq!(m.tags.name(), "tags-0000002a.bin");
    assert_eq!(m.autodj.map(|c| c.name()).as_deref(), Some("autodj-00000029.bin"));
    let m = Manifest::read(&bytes("hardening/msmf-minor-1.bin"), msmf::Uses::ALL).expect("a newer minor");
    let golden_root = Manifest::read(&golden("manifest-servers.bin"), msmf::Uses::ALL).expect("the golden");
    assert_eq!(m.servers, golden_root.servers, "rows of 48 bytes read by their 40-byte prefix");
    // Entries that break 2.5.5's writer rules: the root reads, their
    // servers are unknown.
    let m = Manifest::read(&bytes("hardening/msmf-srvs-rules.bin"), msmf::Uses::ALL).expect("the root reads");
    let table = m.server_table();
    assert!((1..=3).all(|i| table.get(i).is_none()));
    let talk = Talk { url_key: server_url_key("http://music.example:3000"), ..Talk::default() };
    assert_eq!(table.same_server(1, &talk), Verdict::Unknown);
    assert_eq!(table.same_root(&talk), Verdict::Same);
    // The device reads past a broken SRVS; the software doesn't.
    assert_eq!(read_as("MSMF", &bytes("hardening/msmf-srvs-ignored.bin"), false), Why::Shape);
    // Rows naming servers no SRVS entry names: the same records, their
    // servers unknown to the guard.
    let t = mptg::read(&bytes("hardening/mptg-orig-server.bin"), mptg::Uses::ALL).expect("the records");
    let servers: Vec<u16> = t.records.iter().map(|e| e.ledger.as_ref().expect("a ledger").server).collect();
    let n = servers.len();
    assert_eq!((servers[1], servers[3], servers[n - 1]), (1, 2, 0xFFFF));
    let root = Manifest::read(&golden("manifest.bin"), msmf::Uses::ALL).expect("the root").server_table();
    assert_eq!(root.same_server(0xFFFF, &talk), Verdict::Unknown);
    // A newer minor's longer rows and header are read by their prefix: the
    // 13th run field is past the twelve readRules 1 knows. Carried forward,
    // it is this writer's file again: what the minor added is dropped
    // (2.4.5), byte for byte the file it was made from. So is a newer
    // minor's root, its SRVS rows of 48 bytes.
    let minor = mptg::read(&bytes("hardening/mptg-minor-1.bin"), mptg::Uses::ALL).expect("a newer minor");
    assert_eq!(minor.header.minor, 1);
    let (meta, records, folders) = minor.to_input();
    let carried = mptg::write(&meta, &records, &folders).expect("carried forward");
    assert!(carried == golden("tags-0000002a.bin"));
    let m = Manifest::read(&bytes("hardening/msmf-minor-1.bin"), msmf::Uses::ALL).expect("a newer minor");
    let carried = msmf::write(&m.to_input()).expect("carried forward");
    assert!(carried == golden("manifest-servers.bin"));
}

/// ORs `bits` into every row of a section, at `at` in the row.
fn or_rows(file: &mut [u8], kind: u32, stride: usize, at: usize, bits: &[u8]) {
    let (off, len) = section_span(file, kind);
    for row in (off..off + len).step_by(stride) {
        for (b, x) in file[row + at..].iter_mut().zip(bits) {
            *b |= x;
        }
    }
}

/// A newer minor's flag bits (2.4.5), set in every flag word of the
/// fixture's tags file and of its root of several servers: older readers
/// ignore them, so both read; carried forward they go, since a writer sets
/// only the bits it defines, and each is its golden again. Given as read,
/// with the bits, the writers refuse them.
#[test]
fn newer_flag_bits_are_dropped_when_carried() {
    // Minor 1; FOLD's flags, RECS's known and flags, ORIG's originFlags.
    let want = golden("tags-0000002a.bin");
    let mut b = want.clone();
    b[6..8].copy_from_slice(&1u16.to_le_bytes());
    or_rows(&mut b, mptg::FOLD, 16, 8, &[0xFC, 0, 0, 0x80]);
    or_rows(&mut b, mptg::RECS, 72, 32, &[0, 0, 0xFE, 0xFF, 0, 0xFC]);
    or_rows(&mut b, mptg::ORIG, 80, 70, &[0xF8]);
    reseal(&mut b);
    assert!(mptg::read(&b, mptg::Uses::DEVICE).is_ok());
    let tags = mptg::read(&b, mptg::Uses::ALL).expect("a newer minor's bits are ignored");
    assert!(tags.folders.iter().all(|f| f.flags & 0x8000_00FC == 0x8000_00FC));
    for e in &tags.records {
        let origin = e.ledger.as_ref().map_or(0, |l| l.origin_flags);
        assert_eq!((e.rec.flags >> 10, e.rec.known >> 17, origin >> 3), (0x3F, 0x7FFF, 0x1F), "{}", e.path);
    }
    let (meta, records, folders) = tags.to_input();
    assert!(mptg::write(&meta, &records, &folders).expect("carried forward") == want);
    let mut as_read = records.clone();
    for (r, e) in as_read.iter_mut().zip(&tags.records) {
        (r.rec, r.ledger) = (e.rec, e.ledger.clone());
    }
    assert!(mptg::write(&meta, &as_read, &folders).is_err());

    // Minor 1; the header's flags, SRVS's.
    let want = golden("manifest-servers.bin");
    let mut b = want.clone();
    b[6..8].copy_from_slice(&1u16.to_le_bytes());
    b[44] |= 0xFE;
    or_rows(&mut b, msmf::SRVS, 40, 36, &[0xFE, 0, 0, 0x80]);
    reseal(&mut b);
    let root = Manifest::read(&b, msmf::Uses::ALL).expect("a newer minor's bits are ignored");
    assert!(root.is_final() && root.servers.iter().all(|s| s.flags & !PEER == 0x8000_00FE));
    // A reserved bit is not a broken writer rule: every server reads.
    let table = root.server_table();
    assert!((1..=root.servers.len()).all(|i| table.get(i).is_some()));
    let golden_root = Manifest::read(&want, msmf::Uses::ALL).expect("the golden");
    let peers = |m: &Manifest| m.servers.iter().map(Server::is_peer).collect::<Vec<_>>();
    assert_eq!(peers(&root), peers(&golden_root));
    assert!(msmf::write(&root.to_input()).expect("carried forward") == want);
    assert!(msmf::write(&ManifestIn { servers: root.servers.clone(), ..root.to_input() }).is_err());
    assert!(msmf::write(&ManifestIn { flags: root.flags, ..root.to_input() }).is_err());
    // The goldens' roots, carried forward unchanged.
    for name in ["manifest.bin", "manifest-min.bin", "manifest-servers.bin", "manifest-acks.bin"] {
        let m = Manifest::read(&golden(name), msmf::Uses::ALL).expect("a root");
        assert!(msmf::write(&m.to_input()).expect("carried forward") == golden(name), "{name}");
    }
}

/// The byte ranges no reader checks: the zero gaps between sections.
fn gaps(file: &[u8]) -> Vec<bool> {
    let h = container::Header::parse(file).expect("a frame");
    let mut skip = vec![false; file.len()];
    let mut end = (h.header_bytes + 32 * h.section_count) as usize;
    for i in 0..h.section_count as usize {
        let e = &file[h.header_bytes as usize + 32 * i..];
        let (off, len) = (prim::u32_at(e, 8) as usize, prim::u32_at(e, 12) as usize);
        skip[end..off].iter_mut().for_each(|s| *s = true);
        end = off + len;
    }
    skip
}

/// Every golden but the two large indexes, cut at every byte and with every
/// bit flipped in turn: never a panic, always absent — but where the bit is
/// in a gap no CRC covers, where the file reads as it did.
#[test]
fn goldens_truncated_or_flipped_are_absent() {
    let names = [
        "tags-0000002a.bin",
        "tags-device.bin",
        "tags-00000001.bin",
        "tags-00000007.bin",
        "manifest.bin",
        "manifest-min.bin",
        "manifest-servers.bin",
        "pending.bin",
        "autodj-00000029.bin",
        "autodj-small-top.bin",
        "autodj-small-default.bin",
        "autodj-tiny.bin",
        "autodj-pair.bin",
    ];
    for name in names {
        let file = golden(name);
        let format = format_of(&file);
        assert_eq!(read_as(format, &file, false), Why::Ok, "{name}");
        for n in 0..file.len() {
            assert_ne!(read_as(format, &file[..n], false), Why::Ok, "{name} cut at {n}");
        }
        let skip = gaps(&file);
        let mut b = file.clone();
        for at in 0..b.len() {
            for bit in 0..8 {
                b[at] ^= 1 << bit;
                let why = read_as(format, &b, false);
                if skip[at] {
                    assert_eq!(why, Why::Ok, "{name}: a gap's byte {at}");
                } else {
                    assert_ne!(why, Why::Ok, "{name}: byte {at} bit {bit}");
                }
                b[at] ^= 1 << bit;
            }
        }
    }
}

/// Every section's CRC and the header's made right again after an edit of
/// a file's bytes (where its directory entry lies inside the file), so the
/// edit reaches the structural checks.
fn reseal(b: &mut [u8]) {
    let Some(h) = container::Header::parse(b) else { return };
    let dir = h.header_bytes as u64 + 32 * h.section_count as u64;
    if h.section_count > 64 || dir > b.len() as u64 {
        return;
    }
    for i in 0..h.section_count as usize {
        let at = h.header_bytes as usize + 32 * i;
        let (off, len) = (prim::u32_at(b, at + 8) as u64, prim::u32_at(b, at + 12) as u64);
        if off + len <= b.len() as u64 {
            let crc = crc32(&b[off as usize..(off + len) as usize]);
            b[at + 24..at + 28].copy_from_slice(&crc.to_le_bytes());
        }
    }
    b[32..36].fill(0);
    let crc = crc32(&b[..dir as usize]);
    b[32..36].copy_from_slice(&crc.to_le_bytes());
}

/// Hostile input: random edits of every golden, resealed so their CRCs
/// pass, and random bytes behind a valid-looking frame. Whatever a reader
/// makes of them, it returns: no panic, no endless loop.
#[test]
fn mutated_files_never_panic() {
    let mut rng = fastrand::Rng::with_seed(0x5EED_C0DE);
    let names = [
        "tags-0000002a.bin",
        "tags-00000007.bin",
        "tags-device.bin",
        "manifest.bin",
        "manifest-servers.bin",
        "manifest-acks.bin",
        "pending.bin",
        "autodj-00000029.bin",
        "autodj-small-default.bin",
    ];
    for name in names {
        let file = golden(name);
        let format = format_of(&file);
        for _ in 0..3000 {
            let mut b = file.clone();
            for _ in 0..rng.usize(1..5) {
                let at = rng.usize(..b.len());
                b[at] = match rng.u8(..4) {
                    0 => 0,
                    1 => 0xFF,
                    2 => b[at] ^ (1 << rng.u8(..8)),
                    _ => rng.u8(..),
                };
            }
            if rng.bool() {
                b.truncate(rng.usize(..=b.len()));
            }
            reseal(&mut b);
            for device in [false, true] {
                let _ = read_as(format, &b, device);
            }
            if format == "MPDJ"
                && let Ok(t) = Table::open(&b)
            {
                for r in 0..t.row_count.min(16) {
                    let _ = t.slot(r);
                }
                let _ = (t.strings(), t.paths(), t.filter(0));
            }
        }
    }
    for _ in 0..2000 {
        let mut b: Vec<u8> = (0..rng.usize(40..600)).map(|_| rng.u8(..)).collect();
        let magic = [MPTG, MSMF, MSPD, MPDJ][rng.usize(..4)];
        b[..4].copy_from_slice(&magic.to_le_bytes());
        b[4..6].copy_from_slice(&(if magic == MPDJ { 2u16 } else { 1 }).to_le_bytes());
        let hb = [56u32, 72, 96, 104, 128][rng.usize(..5)];
        b[8..12].copy_from_slice(&hb.to_le_bytes());
        b[12..16].copy_from_slice(&rng.u32(..8).to_le_bytes());
        let len = b.len() as u32;
        b[16..20].copy_from_slice(&len.to_le_bytes());
        reseal(&mut b);
        for format in ["MPTG", "MSMF", "MSPD", "MPDJ"] {
            let _ = read_as(format, &b, false);
        }
    }
}

// ── Round trips and the canonical order ─────────────────────────────────────

/// A made-up library: folders from a small alphabet that holds the order's
/// traps (`A`, `A B`, `A-`, `a`, `É`, NFC and NFD), records with tag values
/// that need 2.3.6 (control characters, repeats, long values), ledger rows
/// and OWNED folders.
fn made_up_library(rng: &mut fastrand::Rng, records: usize) -> (Meta, Vec<RecordIn>, Vec<FolderIn>) {
    const NAMES: [&str; 9] = ["A", "A B", "A-", "B", "a", "\u{c9}", "Caf\u{e9}", "Cafe\u{301}", "Disc 1"];
    let source = [mptg::SOURCE_DEVICE, mptg::SOURCE_SOFTWARE, mptg::SOURCE_SYNC_AGENT][rng.usize(..3)];
    let meta = Meta {
        source,
        parser_version: rng.u16(..),
        read_rules: 1,
        generation: rng.u32(..),
        card_id: rng.u64(..),
        minor: 0,
        producer: if rng.bool() { "mstream-terminal 0.13.0".into() } else { String::new() },
    };
    let mut paths = BTreeSet::new();
    let mut out = Vec::new();
    while out.len() < records {
        let depth = rng.usize(0..4);
        let mut parts: Vec<String> = (0..depth).map(|_| NAMES[rng.usize(..NAMES.len())].to_owned()).collect();
        parts.push(format!("{:02} - {}.mp3", rng.u8(..20), NAMES[rng.usize(..NAMES.len())]));
        let path = parts.join("/");
        if !paths.insert(path.clone()) {
            continue;
        }
        let mut raw: [Vec<String>; RUN_FIELDS] = Default::default();
        for values in raw.iter_mut() {
            for _ in 0..rng.usize(0..4) {
                values.push(match rng.u8(..6) {
                    0 => String::new(),
                    1 => "tab\there".into(),
                    2 => "x".repeat(rng.usize(200..300)),
                    3 => "same".into(),
                    _ => NAMES[rng.usize(..NAMES.len())].into(),
                });
            }
        }
        let (run, truncated) = Run::build(&raw);
        let rec = Record {
            size: rng.u32(..),
            fat_time: rng.u32(..),
            duration_ms: rng.u32(..),
            qfp: rng.u64(..),
            known: mptg::KNOWN_RULES1,
            flags: rng.u16(..) & mptg::FLAGS_V1 & !mptg::TRUNCATED
                | if truncated { mptg::TRUNCATED } else { 0 },
            year: rng.u16(..),
            track: rng.u16(..),
            rg_track_gain: rng.i16(..),
            container: rng.u8(..),
            camelot: rng.u8(..),
            pic_offset: rng.u32(..),
            pic_coding: rng.u8(..),
            ..Record::default()
        };
        let ledger = (source != mptg::SOURCE_DEVICE && rng.bool()).then(|| Ledger {
            server_path: if rng.bool() { format!("Music/{path}") } else { String::new() },
            mstream_id: rng.u32(..),
            audio_hash: md5(path.as_bytes()),
            server_size: rng.u64(..),
            hash_v: 2,
            server: rng.u16(..4),
            ..Ledger::default()
        });
        out.push(RecordIn { path, rec, run, ledger });
    }
    let folders = (0..rng.usize(0..4))
        .map(|i| {
            let path = format!("Owned {i}/{}", NAMES[rng.usize(..NAMES.len())]);
            FolderIn { path, flags: mptg::FOLDER_OWNED }
        })
        .collect();
    (meta, out, folders)
}

#[test]
fn tags_round_trip_in_any_order() {
    let mut rng = fastrand::Rng::with_seed(20_261_018);
    for round in 0..60 {
        let n = rng.usize(0..40);
        let (meta, mut records, mut folders) = made_up_library(&mut rng, n);
        let file = mptg::write(&meta, &records, &folders).expect("a tags file");
        let tags = mptg::read(&file, mptg::Uses::ALL).unwrap_or_else(|w| panic!("round {round}: {w}"));
        assert!(mptg::read(&file, mptg::Uses::DEVICE).is_ok());
        // Read back as written: each record by its path, its hash the path's.
        assert_eq!(tags.records.len(), records.len());
        for e in &tags.records {
            let r = records.iter().find(|r| r.path == e.path).expect("a record written");
            assert_eq!((e.hash, &e.rec, &e.run), (path_hash(&r.path), &r.rec, &r.run), "{}", e.path);
            // A software file has a row a record, a zeroed one where none
            // was given.
            let device = meta.source == mptg::SOURCE_DEVICE;
            let want = (!device).then(|| r.ledger.clone().unwrap_or_default());
            assert_eq!(e.ledger, want);
        }
        // The canonical order: records by folder, then name; the folders in
        // pre-order.
        for w in tags.records.windows(2) {
            assert!(compare_file_paths(&w[0].path, &w[1].path).is_lt(), "{} {}", w[0].path, w[1].path);
        }
        for w in tags.folders.windows(2) {
            assert!(super::order::compare_folder_paths(&w[0].path, &w[1].path).is_lt());
        }
        // Any input order, the same bytes; read and written again, the same.
        rng.shuffle(&mut records);
        rng.shuffle(&mut folders);
        assert!(mptg::write(&meta, &records, &folders).expect("again") == file, "round {round}: order");
        let (m2, r2, f2) = tags.to_input();
        assert!(mptg::write(&m2, &r2, &f2).expect("again") == file, "round {round}: read and written again");
    }
}

#[test]
fn tags_writer_refuses_what_its_reader_would() {
    let meta = Meta { source: mptg::SOURCE_SOFTWARE, read_rules: 1, generation: 2, ..Meta::default() };
    let rec = |path: &str| RecordIn { path: path.into(), ..RecordIn::default() };
    let long = format!("{}/{}.mp3", "L".repeat(120), "M".repeat(124));
    for bad in ["", "/x.mp3", "a//b.mp3", "a/./b.mp3", "../b.mp3", "a/", long.as_str()] {
        assert!(mptg::write(&meta, &[rec(bad)], &[]).is_err(), "{bad:?}");
    }
    assert!(mptg::write(&meta, &[rec("a.mp3"), rec("a.mp3")], &[]).is_err());
    assert!(mptg::write(&meta, &[rec("a"), rec("a/b.mp3")], &[]).is_err(), "a file and a folder");
    let unowned = FolderIn { path: "Empty".into(), flags: mptg::FOLDER_THUMB };
    assert!(mptg::write(&meta, &[], &[unowned]).is_err());
    let mut bad_run = rec("a.mp3");
    bad_run.run.fields[0] = b"tab\there".to_vec();
    assert!(mptg::write(&meta, &[bad_run], &[]).is_err());
    let device = Meta { source: mptg::SOURCE_DEVICE, ..meta.clone() };
    let with_ledger = RecordIn { ledger: Some(Ledger::default()), ..rec("a.mp3") };
    assert!(mptg::write(&device, &[with_ledger], &[]).is_err());
    let nul = Meta { producer: "a\0b".into(), ..meta.clone() };
    assert!(mptg::write(&nul, &[rec("a.mp3")], &[]).is_err());
    assert!(mptg::write(&Meta { source: 0, ..meta.clone() }, &[], &[]).is_err());
    // A flag bit v1 doesn't define, in any of the four words (2.4.5).
    let with = |r: Record, origin: u8| {
        let ledger = Some(Ledger { origin_flags: origin, ..Ledger::default() });
        RecordIn { rec: r, ledger, ..rec("a.mp3") }
    };
    let all = Record { flags: mptg::FLAGS_V1, known: mptg::KNOWN_RULES1, ..Record::default() };
    assert!(mptg::write(&meta, &[with(all, mptg::ORIGIN_FLAGS_V1)], &[]).is_ok());
    for (r, origin) in [
        (Record { flags: 1 << 10, ..Record::default() }, 0),
        (Record { known: mptg::KNOWN_RULES1 + 1, ..Record::default() }, 0),
        (Record::default(), 1 << 3),
    ] {
        assert!(mptg::write(&meta, &[with(r, origin)], &[]).is_err(), "{r:?} {origin}");
    }
    let owned = |flags| FolderIn { path: "Owned".into(), flags };
    assert!(mptg::write(&meta, &[], &[owned(mptg::FOLDER_FLAGS_V1)]).is_ok());
    assert!(mptg::write(&meta, &[], &[owned(mptg::FOLDER_OWNED | 4)]).is_err());
    // The longest path the device sees is written and read.
    let longest = format!("{}/{}.mp3", "L".repeat(120), "M".repeat(123));
    let file = mptg::write(&meta, &[rec(&longest)], &[]).expect("255 bytes with /music/");
    assert_eq!(mptg::read(&file, mptg::Uses::ALL).expect("read").records[0].path, longest);
}

#[test]
fn roots_and_plans_round_trip() {
    let mut rng = fastrand::Rng::with_seed(7);
    let tags = Comp::of(&golden("tags-0000002a.bin")).expect("a companion");
    let dj = Comp::of(&golden("autodj-00000029.bin")).expect("a companion");
    for _ in 0..40 {
        let lib = |i: usize, rng: &mut fastrand::Rng| format!("Lib {i}/{}", ["A", "A B", "a"][rng.usize(..3)]);
        let mut roots: Vec<String> = (0..rng.usize(0..5)).map(|i| lib(i, &mut rng)).collect();
        roots.sort();
        roots.dedup();
        let servers: Vec<Server> = (0..rng.usize(0..4))
            .map(|i| match rng.bool() {
                true => Server::direct([i as u8 + 1; 16], 0x1000 + i as u64),
                false => Server::peer([0; 16], 0x2000 + i as u64, 1 + i as u32),
            })
            .collect();
        let mut m = ManifestIn {
            generation: rng.u32(1..),
            card_id: rng.u64(..),
            commit_time: rng.u32(..),
            flags: rng.u32(..2),
            commit_id: rng.u64(..),
            server_instance: [0xEE; 16],
            server_url_key: 0xED90_1EA3_EE76_3AC7,
            producer: "mstream-terminal 0.13.0".into(),
            server_revision: "\"r-1-2-3-4\"".into(),
            base_generation: rng.u32(..),
            comps: if rng.bool() { vec![tags.clone(), dj.clone()] } else { vec![tags.clone()] },
            roots: roots.clone(),
            servers: servers.clone(),
            ..ManifestIn::default()
        };
        let file = msmf::write(&m).expect("a root");
        let back = Manifest::read(&file, msmf::Uses::ALL).expect("read back");
        assert_eq!((back.roots, back.servers), (roots, servers));
        assert_eq!((back.commit_id, back.tags), (m.commit_id, tags.clone()));
        rng.shuffle(&mut m.roots);
        assert!(msmf::write(&m).expect("again") == file);
    }
    // Refused: COMP out of its order, a root twice, an entry for the root's
    // own server, a flag bit v1 doesn't define.
    let comps = vec![tags.clone()];
    let base = ManifestIn { generation: 1, comps, server_url_key: 5, ..ManifestIn::default() };
    assert!(msmf::write(&ManifestIn { comps: vec![dj.clone(), tags.clone()], ..base.clone() }).is_err());
    assert!(msmf::write(&ManifestIn { comps: vec![], ..base.clone() }).is_err());
    assert!(msmf::write(&ManifestIn { roots: vec!["A".into(), "A".into()], ..base.clone() }).is_err());
    assert!(msmf::write(&ManifestIn { roots: vec!["/A".into()], ..base.clone() }).is_err());
    assert!(msmf::write(&ManifestIn { servers: vec![Server::direct([0; 16], 5)], ..base.clone() }).is_err());
    let flagged = Server { flags: 2, ..Server::direct([0; 16], 6) };
    assert!(msmf::write(&ManifestIn { servers: vec![flagged], ..base.clone() }).is_err());
    assert!(msmf::write(&ManifestIn { servers: vec![Server::peer([0; 16], 7, 1)], ..base.clone() }).is_ok());
    assert_eq!(PEER, 1);

    for _ in 0..40 {
        let mut ops: Vec<Op> = Vec::new();
        for i in 0..rng.usize(0..12) {
            let op = [mspd::OP_WRITE, mspd::OP_DELETE, mspd::OP_FOLDER][rng.usize(..3)];
            let ext = if op == mspd::OP_FOLDER { "" } else { ".mp3" };
            let path = format!("{}/{i}{ext}", ["A", "A B", "a", "A/B"][rng.usize(..4)]);
            ops.push(Op { op, path, expected_size: if op == mspd::OP_WRITE { rng.u32(..) } else { 0 } });
        }
        let run_id = rng.u64(..);
        let p = PlanIn { generation: 3, card_id: 9, run_id, base_generation: 2, ops: ops.clone(), ..PlanIn::default() };
        let file = mspd::write(&p).expect("a plan");
        let back = Plan::read(&file).expect("read back");
        assert_eq!(back.ops.len(), ops.len());
        let mut shuffled = p.clone();
        rng.shuffle(&mut shuffled.ops);
        assert!(mspd::write(&shuffled).expect("again") == file);
        let again = PlanIn { ops: back.ops.clone(), ..p.clone() };
        assert!(mspd::write(&again).expect("again") == file);
    }
    let op = |op, path: &str| Op { op, path: path.into(), expected_size: 0 };
    assert!(mspd::write(&PlanIn { ops: vec![op(4, "a")], ..PlanIn::default() }).is_err());
    assert!(mspd::write(&PlanIn { ops: vec![op(2, "../a")], ..PlanIn::default() }).is_err());
    assert!(mspd::write(&PlanIn { ops: vec![op(2, "a"), op(2, "a")], ..PlanIn::default() }).is_err());
}

// ── Thumbnails from images ──────────────────────────────────────────────────

fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, y| image::Rgb(pixel(x, y)));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).expect("a PNG");
    out.into_inner()
}

#[test]
fn thumbnails_from_images() {
    // One colour stays that colour, at both sizes.
    let px = mpth::from_image(&png(300, 200, |_, _| [200, 100, 50])).expect("a thumbnail");
    let want = mpth::rgb565(200, 100, 50);
    assert!(px.small.iter().chain(&px.large).all(|&v| v == want));
    assert_eq!((px.small.len(), px.large.len()), (40 * 40, 96 * 96));
    // A wide image is cropped to its centre: its red edges never show.
    let banded = png(300, 100, |x, _| if (100..200).contains(&x) { [0, 255, 0] } else { [255, 0, 0] });
    let px = mpth::from_image(&banded).expect("a thumbnail");
    assert!(px.large.iter().all(|&v| v == mpth::rgb565(0, 255, 0)));
    // A box filter: grown, each pixel takes the one it lies in; shrunk, the
    // mean of what it covers (a fine checker of black and white is grey).
    let checker = mpth::from_rgb(&[0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0], 2, 2).expect("pixels");
    assert_eq!(checker.small[0], mpth::rgb565(0, 0, 0));
    let cell = |i: usize| if (i % 80 + i / 80) % 2 == 0 { [0u8; 3] } else { [255u8; 3] };
    let fine: Vec<u8> = (0..80 * 80).flat_map(cell).collect();
    let down = mpth::from_rgb(&fine, 80, 80).expect("pixels");
    assert!(down.small.iter().all(|&v| v == mpth::rgb565(128, 128, 128)));
    // A thumbnail from them is its folder's.
    let h = path_hash("Artist/Album");
    let file = mpth::write(h, 1234, &px).expect("a thumbnail");
    assert!(mpth::check(&file, h) && file.len() == mpth::BYTES);
    // Not an image, or too large an image, makes no thumbnail.
    assert!(matches!(mpth::from_image(b"not an image"), Err(mpth::ImageError::Decode(_))));
    assert!(matches!(mpth::from_image(&png(13_000, 1, |_, _| [0, 0, 0])), Err(mpth::ImageError::TooLarge)));
    // The cap holds for the RGB image made from the decoded one too: a grey
    // image decodes to a third of its RGB bytes, an RGB one to all of them.
    let img = image::GrayImage::from_fn(400, 400, |x, y| image::Luma([(x ^ y) as u8]));
    let mut grey = std::io::Cursor::new(Vec::new());
    img.write_to(&mut grey, image::ImageFormat::Png).expect("a PNG");
    let too_large = |r: Result<_, mpth::ImageError>| matches!(r, Err(mpth::ImageError::TooLarge));
    assert!(too_large(mpth::decode_within(grey.get_ref(), 400 * 400 * 3 - 1)));
    assert!(mpth::decode_within(grey.get_ref(), 400 * 400 * 3).is_ok());
    let rgb = png(400, 400, |x, y| [x as u8, y as u8, 0]);
    assert!(too_large(mpth::decode_within(&rgb, 400 * 400 * 3 - 1)));
    assert!(mpth::decode_within(&rgb, 400 * 400 * 3).is_ok());
    assert!(mpth::from_rgb(&[], 0, 0).is_none());
    assert!(mpth::write(h, 0, &mpth::Pixels { small: vec![], large: vec![] }).is_err());
    // A JPEG too: the decoder the player's art already uses.
    let img = image::RgbImage::from_fn(64, 48, |_, _| image::Rgb([16, 160, 240]));
    let mut jpeg = std::io::Cursor::new(Vec::new());
    img.write_to(&mut jpeg, image::ImageFormat::Jpeg).expect("a JPEG");
    let px = mpth::from_image(jpeg.get_ref()).expect("a thumbnail");
    let (r, g, b) = (px.large[0] >> 11, px.large[0] >> 5 & 63, px.large[0] & 31);
    let near = r.abs_diff(16 >> 3) <= 1 && g.abs_diff(160 >> 2) <= 2 && b.abs_diff(240 >> 3) <= 1;
    assert!(near, "{r} {g} {b}");
}
