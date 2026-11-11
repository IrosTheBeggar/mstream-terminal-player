//! The kit against the contract's shared fixtures, test/fixtures/card (made
//! by the firmware repo's tools/card_fixtures.py and copied from there; the
//! folder's README names the commit): every vector of 2.18, every golden
//! written byte for byte from its description, every hardening file read as
//! absent or present for the player's own reason, every golden truncated at
//! every byte and every bit of it flipped, moods.json's mood tails (2.6.4),
//! and round trips of generated libraries in any input order.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

use super::container::{self, MPDJ, MPTG, MPWF, MSMF, MSPD, unknown_major};
use super::device_txt::{DeviceTxt, Format};
use super::fat::{fat_from_wall, fat_time, fat_valid, fat_wall, skew, time_matches};
use super::moods::{self, Scores, Tail};
use super::mpdj::{self, Table};
use super::mppl;
use super::mptg::{self, FolderIn, Ledger, Meta, Record, RecordIn, Tags};
use super::msmf::{self, Ack, Candidate, Comp, Manifest, ManifestIn, Pick, elect};
use super::mspd::{self, Op, Plan, PlanIn};
use super::mpth;
use super::mpwf;
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

/// A server row as the fixtures give one: 8 numbers or nulls in `heads`
/// order.
fn scores_of(v: &Value) -> Scores {
    let row = v.as_array().expect("a row of scores");
    assert_eq!(row.len(), moods::HEADS.len());
    std::array::from_fn(|i| row[i].as_f64())
}

/// A tags description (raw tag values: the writer's caller applies 2.3.6
/// and sets TRUNCATED, as the tag reader will). A record's `scores` are its
/// server row, from which the writer's caller makes its mood tail, as the
/// sync will; null, or none given, is no row.
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
            moods: moods::encode(r.get("scores").filter(|s| !s.is_null()).map(scores_of).as_ref()),
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
        read_mpwf: majors("read.mpwf"),
        read_mpth: majors("read.mpth"),
        codecs: text(&d["codecs"]).to_owned(),
        extensions: text(&d["extensions"]).to_owned(),
        max_rate: num(&d["max_rate"]) as u32,
        max_channels: num(&d["max_channels"]) as u32,
        history: majors("history"),
    }
}

/// A track of an MPWF description (WAVEFORM-INDEX.md 13.2), or an `add` of a
/// `commit` vector: a path or the hash itself, its length, its bars from its
/// peaks.
fn waves_row(t: &Value) -> mpwf::Slot {
    let peaks: Vec<u8> = t["peaks"].as_array().expect("peaks").iter().map(|p| num(p) as u8).collect();
    mpwf::Slot {
        path_hash: t.get("hash").map_or_else(|| path_hash(text(&t["path"])), num),
        length_ms: num(&t["lengthMs"]) as u32,
        bars: mpwf::bars_from(&peaks),
    }
}

fn waves_input(d: &Value) -> mpwf::TableIn {
    mpwf::TableIn {
        generation: num(&d["generation"]) as u32,
        card_id: num(&d["cardId"]),
        minor: num(&d["minor"]) as u16,
        rows: d["tracks"].as_array().expect("tracks").iter().map(waves_row).collect(),
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
        // The device opens the frame, and checks each slot as it reads it.
        "MPWF" => mpwf::Table::open(file).and_then(|t| if device { Ok(()) } else { t.check() }),
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
        Some(MPWF) => "MPWF",
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
        "mpwf",
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
    assert!(file.decodes("OPUS") && file.lists("opus") && file.writes_mpdj() && file.writes_mpwf());
    assert!(file.reads(Format::Msmf, 1) && !file.reads(Format::Mpdj, 1) && file.reads(Format::Mpdj, 2));
    assert!(file.reads(Format::Mpwf, 1) && !file.reads(Format::Mpwf, 2));
    // A firmware that keeps a play log: `history` lists major 1 (2.15).
    assert!(file.logs_plays() && file.history == 1);
    // No file: the defaults, which noFile lists key by key; no play log, and
    // no waveform index.
    let none = DeviceTxt::read(None);
    let text_of = none.to_text();
    let keys: BTreeMap<&str, &str> = text_of.lines().filter_map(|l| l.split_once('=')).collect();
    for (k, want) in d["noFile"].as_object().expect("noFile") {
        assert_eq!(keys.get(k.as_str()).copied(), Some(text(want)), "{k}");
    }
    assert!(!none.present && !none.writes_mpdj() && !none.decodes("opus") && !none.logs_plays());
    assert!(!none.writes_mpwf() && !none.reads(Format::Mpwf, 1));
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
            "MPWF" => mpwf::write(&waves_input(&d)).expect("an index"),
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
    assert_eq!(goldens.len(), 23);
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
    // A file with no mood tail: minor 0, rows of 72.
    assert_eq!((tags.header.minor, section_of(&tags_file, mptg::RECS).len()), (0, 72 * tags.records.len()));

    // Mood tails (minor 1, 2.6.4): rows of 80 bytes; MOOD-FILTERS.md 5.7's
    // worked example; a row of NULLs, analysed; none for no server row, nor
    // for a file with no track. The device reads the same tails, and
    // carried forward the file is itself again.
    let moods_file = golden("tags-moods.bin");
    let scored = mptg::read(&moods_file, mptg::Uses::ALL).expect("a minor 1 file");
    assert_eq!(scored.header.minor, 1);
    assert_eq!(section_of(&moods_file, mptg::RECS).len(), 80 * scored.records.len());
    let tail = |path: &str| scored.records.iter().find(|e| e.path == path).expect("a record").rec.moods;
    assert_eq!(spaced_of(&tail("Artist A/song.flac")), "03 3A 46 1D 31 FF 46 00");
    assert_eq!(spaced_of(&tail("Artist A/all-null.flac")), "01 FF FF FF FF FF FF 00");
    assert_eq!((tail("Artist A/no-row.flac"), tail("Artist B/Album/cover.jpg")), (moods::NO_ROW, moods::NO_ROW));
    assert_eq!(scored.records.iter().filter(|e| e.rec.has_moods()).count(), 8);
    let as_device = mptg::read(&moods_file, mptg::Uses::DEVICE).expect("the device's reading");
    assert!(as_device.records.iter().zip(&scored.records).all(|(d, e)| d.rec == e.rec));
    let (meta, records, folders) = scored.to_input();
    assert_eq!((meta.minor, mptg::minor_of(&records)), (1, 1));
    assert!(mptg::write(&meta, &records, &folders).expect("carried forward") == moods_file);

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
    let roots = [
        "manifest.bin",
        "manifest-servers.bin",
        "manifest-min.bin",
        "manifest-acks.bin",
        "manifest-waves.bin",
    ];
    for name in roots {
        let m = Manifest::read(&golden(name), msmf::Uses::DEVICE).expect("a root");
        assert!(m.servers.is_empty() && m.acks.is_empty());
    }
    let t = mptg::read(&tags_file, mptg::Uses::DEVICE).expect("the tags");
    assert!(!t.has_ledger && t.records.iter().all(|e| e.ledger.is_none()));

    // No contract file of the fixtures has an unknown major.
    for name in ["manifest.bin", "tags-0000002a.bin", "pending.bin", "autodj-00000029.bin", "waves-0000002a.bin"] {
        assert!(!unknown_major(&golden(name)), "{name}");
    }
    assert!(MAX_FILE >= 34 * 1024 * 1024);
    assert!(mpwf::file_bytes_for(mpwf::MAX_ROWS) as usize <= MAX_FILE);
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

/// Two waveform indexes read the same (the hardening's `same`): every slot's
/// hash, length and bars, found by its hash.
fn assert_same_waves(want: &[u8], got: &[u8], what: &str) {
    let (w, g) = (mpwf::Table::open(want).expect("the golden"), mpwf::Table::open(got).expect(what));
    assert_eq!(w.rows, g.rows, "{what}");
    for r in 0..w.rows {
        let slot = w.slot(r).expect("the golden's slot");
        let found = g.find(slot.path_hash);
        assert_eq!(found.slot.as_ref(), Some(&slot), "{what}: row {r}");
    }
}

/// A lookup's answer as the fixtures spell it.
fn lookup_name(l: mpwf::Lookup) -> &'static str {
    match l {
        mpwf::Lookup::Hit => "hit",
        mpwf::Lookup::Miss => "miss",
        mpwf::Lookup::Damaged => "damaged",
        mpwf::Lookup::Broken => "broken",
    }
}

#[test]
fn hardening_files_read_as_listed() {
    let index = json("hardening/index.json");
    let entries = index.as_array().expect("the index");
    assert_eq!(entries.len(), 198);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let (mut waves_why, mut waves_lookups) = (BTreeSet::new(), 0);
    for e in entries {
        let file = text(&e["file"]);
        let b = bytes(&format!("hardening/{file}"));
        let format = text(&e["format"]);
        let device = text(&e["uses"]) == "device";
        let what = format!("{file}: {}", text(&e["check"]));
        assert_eq!(read_as(format, &b, device).name(), text(&e["why"]), "{what}");
        assert_eq!(text(&e["expect"]) == "present", text(&e["why"]) == "ok", "{what}");
        *counts.entry(format.to_owned()).or_default() += 1;
        if format == "MPWF" {
            waves_why.insert(text(&e["why"]).to_owned());
        }
        if let Some(same) = e.get("same").and_then(Value::as_str) {
            if format == "MPDJ" {
                assert_same_index(&golden(same), &b, &what);
            } else if format == "MPWF" {
                assert_same_waves(&golden(same), &b, &what);
            } else {
                let want = mptg::read(&golden(same), mptg::Uses::ALL).expect("the golden");
                let got = mptg::read(&b, mptg::Uses::ALL).expect(&what);
                assert_same_tags(&want, &got, &what);
            }
        }
        if let Some(slot) = e.get("slot").filter(|_| format == "MPWF") {
            // One lookup as the device makes it: the frame good, the slots
            // read as the search asks for them.
            let t = mpwf::Table::open(&b).expect(&what);
            let found = t.find(num(&slot["hash"]));
            assert_eq!(lookup_name(found.lookup), text(&slot["read"]), "{what}");
            assert_eq!(found.probes.len() as u64, num(&slot["reads"]), "{what}");
            if text(&e["why"]) != "ok" {
                assert_eq!(found.lookup.why().name(), text(&e["why"]), "{what}");
            }
            waves_lookups += 1;
        } else if let Some(slot) = e.get("slot") {
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
        ("MPTG".to_owned(), 55),
        ("MPWF".to_owned(), 50),
        ("MSMF".to_owned(), 34),
        ("MSPD".to_owned(), 9),
    ]);
    // Every reason of WAVEFORM-INDEX.md 13.5's table, and the present ones.
    assert_eq!(waves_lookups, 12);
    let reasons: BTreeSet<String> = [
        "ok",
        "short",
        "magic",
        "major",
        "headerBytes",
        "sectionCount",
        "fileBytes",
        "headerCrc",
        "layout",
        "unknownRequired",
        "shape",
        "missing",
        "enum",
        "wvLayout",
        "wvSlotCrc",
        "wvSlotOrder",
        "sectionCrc",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(waves_why, reasons);

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
    assert_eq!(table.all_same(&talk), [0], "a broken entry is never the server read");
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

    // The mood tail's files, read as the firmware reads them (FOLD, RECS and
    // STRS alone): rows of 88 bytes give their 80-byte prefix's tails, the
    // golden's every one; a tail a reader refuses gives none, nor does a row
    // of 76, nor mptg-minor-1's 0xAB.
    let tails = |file: &[u8]| -> Vec<Tail> {
        let t = mptg::read(file, mptg::Uses::DEVICE).expect("the records");
        t.records.iter().map(|e| e.rec.moods).collect()
    };
    let want = tails(&golden("tags-moods.bin"));
    assert_eq!(want.iter().filter(|t| **t != moods::NO_ROW).count(), 8);
    assert_eq!(tails(&bytes("hardening/mptg-moods-longer.bin")), want);
    for file in ["mptg-tail-invalid.bin", "mptg-stride-76.bin", "mptg-minor-1.bin"] {
        assert!(tails(&bytes(&format!("hardening/{file}"))).iter().all(|t| *t == moods::NO_ROW), "{file}");
    }
    // Carried forward, what a reader took is kept and what it refused goes:
    // the golden each was made from, at its own stride.
    for (file, golden_name) in [
        ("mptg-moods-longer.bin", "tags-moods.bin"),
        ("mptg-tail-invalid.bin", "tags-0000002a.bin"),
        ("mptg-stride-76.bin", "tags-0000002a.bin"),
    ] {
        let t = mptg::read(&bytes(&format!("hardening/{file}")), mptg::Uses::ALL).expect("the records");
        let (meta, records, folders) = t.to_input();
        assert!(mptg::write(&meta, &records, &folders).expect("carried forward") == golden(golden_name), "{file}");
    }
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
    let roots = [
        "manifest.bin",
        "manifest-min.bin",
        "manifest-servers.bin",
        "manifest-acks.bin",
        "manifest-waves.bin",
    ];
    for name in roots {
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

/// Every golden but the large ones, cut at every byte and with every bit
/// flipped in turn: never a panic, always absent — but where the bit is in a
/// gap no CRC covers, where the file reads as it did.
#[test]
fn goldens_truncated_or_flipped_are_absent() {
    let names = [
        "tags-0000002a.bin",
        "tags-device.bin",
        "tags-00000001.bin",
        "tags-00000007.bin",
        "tags-moods.bin",
        "manifest.bin",
        "manifest-min.bin",
        "manifest-servers.bin",
        "manifest-waves.bin",
        "pending.bin",
        "autodj-00000029.bin",
        "autodj-small-top.bin",
        "autodj-small-default.bin",
        "autodj-tiny.bin",
        "autodj-pair.bin",
        "waves-one.bin",
        "waves-0000002a.bin",
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

/// An MPWF file's slot CRCs made right again (when WVSL's count and place make
/// sense), so a mutation of a slot meets the sort rule (W2), not W1. Before
/// [`reseal`]: the section's own CRC covers the slots.
fn reseal_slots(b: &mut [u8]) {
    let (first, stride) = (mpwf::FIRST_SLOT as usize, mpwf::SLOT_BYTES as usize);
    if format_of(b) != "MPWF" || b.len() < first {
        return;
    }
    let (kind, offset, rows) = (prim::u32_at(b, 224), prim::u32_at(b, 232), prim::u32_at(b, 240));
    if kind != mpwf::WVSL || offset as u64 + rows as u64 * stride as u64 > b.len() as u64 {
        return;
    }
    for r in 0..rows {
        let slot = &mut b[offset as usize + r as usize * stride..][..stride];
        let crc = mpwf::slot_crc(slot, r);
        slot[stride - 4..].copy_from_slice(&crc.to_le_bytes());
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
        "tags-moods.bin",
        "manifest.bin",
        "manifest-servers.bin",
        "manifest-acks.bin",
        "manifest-waves.bin",
        "pending.bin",
        "autodj-00000029.bin",
        "autodj-small-default.bin",
        "waves-0000002a.bin",
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
            reseal_slots(&mut b);
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
            if format == "MPWF"
                && let Ok(t) = mpwf::Table::open(&b)
            {
                // Every lookup returns, its answer in the file's rows.
                for h in [0, u64::MAX, rng.u64(..), path_hash("Artist/x.mp3")] {
                    let found = t.find(h);
                    assert!(found.probes.len() <= 24 && found.probes.iter().all(|&r| r < t.rows));
                }
                let _ = (t.slot(t.rows), t.slot_raw(t.rows));
            }
        }
    }
    for _ in 0..2000 {
        let mut b: Vec<u8> = (0..rng.usize(40..600)).map(|_| rng.u8(..)).collect();
        let magic = [MPTG, MSMF, MSPD, MPDJ, MPWF][rng.usize(..5)];
        b[..4].copy_from_slice(&magic.to_le_bytes());
        b[4..6].copy_from_slice(&(if magic == MPDJ { 2u16 } else { 1 }).to_le_bytes());
        let hb = [56u32, 72, 96, 104, 128, 224][rng.usize(..6)];
        b[8..12].copy_from_slice(&hb.to_le_bytes());
        b[12..16].copy_from_slice(&rng.u32(..8).to_le_bytes());
        let len = b.len() as u32;
        b[16..20].copy_from_slice(&len.to_le_bytes());
        reseal(&mut b);
        for format in ["MPTG", "MSMF", "MSPD", "MPDJ", "MPWF"] {
            let _ = read_as(format, &b, false);
        }
    }
}

// ── The waveform index (WAVEFORM-INDEX.md) ──────────────────────────────────

/// Plain hex bytes, `4d5057`.
fn hex_bytes(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("a hex byte")).collect()
}

/// A slot of a file: its 256 bytes, with no reader's help.
fn slot_at(file: &[u8], row: usize) -> &[u8] {
    &file[256 + 256 * row..256 + 256 * (row + 1)]
}

/// The `scale` vectors' uniform hashes: the next of SplitMix64.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let z = (*state ^ (*state >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A file of these hashes (the bars don't matter: all zero).
fn table_of(hashes: &[u64]) -> Vec<u8> {
    let row = |&path_hash: &u64| mpwf::Slot { path_hash, length_ms: 0, bars: [0; mpwf::BAR_COUNT] };
    let t = mpwf::TableIn { generation: 9, rows: hashes.iter().map(row).collect(), ..mpwf::TableIn::default() };
    mpwf::write(&t).expect("an index")
}

/// The /.player/device.txt of a `deviceTxt` or `commit` vector (13.3): its
/// `text`; None when it has no file; else its readMpwf under a contract line,
/// with no such key when it says so.
fn mpwf_device_text(case: &Value) -> Option<String> {
    if let Some(text) = case["text"].as_str() {
        return Some(text.to_owned());
    }
    if case["noFile"].as_bool() == Some(true) {
        return None;
    }
    let key = match case["keyMissing"].as_bool() {
        Some(true) => String::new(),
        _ => format!("read.mpwf={}\n", text(&case["readMpwf"])),
    };
    Some(format!("contract=1\n{key}"))
}

/// The rows of a `commit` vector (13.3): those of the golden file it names,
/// less `drop`, with `lengthMs` {row: value} applied, then `add`.
fn commit_rows(case: &Value) -> Vec<mpwf::Slot> {
    let mut rows = Vec::new();
    if let Some(name) = case["rows"].as_str() {
        let file = golden(name);
        let t = mpwf::Table::open(&file).expect("the golden");
        let dropped: Vec<u64> = case["drop"].as_array().into_iter().flatten().map(num).collect();
        for r in (0..t.rows).filter(|&r| !dropped.contains(&(r as u64))) {
            let mut slot = t.slot(r).expect("the golden's slot");
            if let Some(ms) = case["lengthMs"].get(r.to_string()) {
                slot.length_ms = num(ms) as u32;
            }
            rows.push(slot);
        }
    }
    rows.extend(case["add"].as_array().into_iter().flatten().map(waves_row));
    rows
}

#[test]
fn vectors_mpwf_names_sizes_and_bars() {
    let v = vectors();
    let m = &v["mpwf"];
    for e in m["names"].as_array().expect("names") {
        assert_eq!(msmf::companion_name(MPWF, num(&e["generation"]) as u32), text(&e["name"]));
    }
    for e in m["sizes"].as_array().expect("sizes") {
        assert_eq!(mpwf::file_bytes_for(num(&e["rows"]) as u32) as u64, num(&e["fileBytes"]));
    }
    for group in m["barRanges"].as_array().expect("barRanges") {
        for e in group["cases"].as_array().expect("cases") {
            let got = mpwf::bar_range(num(&e["i"]) as usize, num(&group["m"]) as usize);
            assert_eq!((got.0 as u64, got.1 as u64), (num(&e["start"]), num(&e["end"])), "{group}");
        }
    }
    // M at least 240: the ranges part M, no gap, no overlap. Below: one peak a
    // bar.
    for count in 1..=2400 {
        let mut next = 0;
        for i in 0..mpwf::BAR_COUNT {
            let (start, end) = mpwf::bar_range(i, count);
            assert!(start < end && end <= count, "{i} of {count}");
            if count >= mpwf::BAR_COUNT {
                assert_eq!(start, next, "{i} of {count}");
                next = end;
            } else {
                assert_eq!(end - start, 1, "{i} of {count}");
            }
        }
        assert!(count < mpwf::BAR_COUNT || next == count, "{count}");
    }

    // The peaks and bars in hex, or a map of the ones not 0.
    let cases = m["bars"].as_array().expect("bars");
    assert!(cases.len() >= 10);
    for e in cases {
        let note = text(&e["note"]);
        let sparse = |at: &str, into: &mut [u8]| {
            for (i, b) in e[at].as_object().into_iter().flatten() {
                into[i.parse::<usize>().expect("an index")] = num(b) as u8;
            }
        };
        let mut peaks = e["peaks"].as_str().map_or(vec![0; num(&e["m"]) as usize], hex_bytes);
        sparse("peaksAt", &mut peaks);
        let mut want = e["bars"].as_str().map_or(vec![0; mpwf::BAR_COUNT], hex_bytes);
        sparse("barsAt", &mut want);
        assert_eq!(peaks.len() as u64, num(&e["m"]), "{note}");
        assert_eq!(mpwf::bars_from(&peaks).as_slice(), want.as_slice(), "{note}");
    }
    // The integer reading of the rule agrees, on random peaks of many widths:
    // a bar is the largest v with w (2v − 1)^2 ≤ 4n, or 0.
    let mut rng = fastrand::Rng::with_seed(20_261_101);
    for _ in 0..50 {
        for m in [1, 2, 3, 7, 239, 240, 241, 480, 800, 1000, 1600] {
            let top = rng.u16(1..=255);
            let peaks: Vec<u8> = (0..m).map(|_| rng.u16(0..=top) as u8).collect();
            let bars = mpwf::bars_from(&peaks);
            for (i, &bar) in bars.iter().enumerate() {
                let (start, end) = mpwf::bar_range(i, m);
                let n: u64 = peaks[start..end].iter().map(|&b| b as u64 * b as u64).sum();
                let w = (end - start) as u64;
                let holds = |v: u64| v == 0 || w * (2 * v - 1) * (2 * v - 1) <= 4 * n;
                let v = (0..=255).rev().find(|&v| holds(v)).expect("0 always holds");
                assert_eq!(bar as u64, v, "bar {i} of M = {m}");
            }
        }
    }
    // No peaks: zeros, not a division by 0.
    assert_eq!(mpwf::bars_from(&[]), [0; mpwf::BAR_COUNT]);
}

#[test]
fn vectors_mpwf_slots_and_sections() {
    let v = vectors();
    let m = &v["mpwf"];
    let s = &m["slot"];
    let raw = hex_bytes(text(&s["bytes"]));
    let row = num(&s["row"]) as u32;
    assert_eq!(raw, slot_at(&golden(text(&s["file"])), row as usize));
    assert_eq!(mpwf::slot_crc(&raw, row) as u64, num(&s["crc"]));
    assert_eq!(prim::u32_at(&raw, 252) as u64, num(&s["crc"]));
    assert_eq!(crc32(&raw[..252]) as u64, num(&s["unseeded"]));
    assert_ne!(mpwf::slot_crc(&raw, row + 1) as u64, num(&s["crc"]), "another row's");
    let slot = mpwf::Slot::decode(&raw);
    assert_eq!(slot.path_hash, num(&s["pathHash"]));
    assert_eq!(slot.path_hash, path_hash(text(&s["path"])));
    assert_eq!(slot.length_ms as u64, num(&s["lengthMs"]));
    assert!(slot.bars.iter().all(|&b| b as u64 == num(&s["barsAll"])));
    assert_eq!(slot.encode(row).as_slice(), raw.as_slice());

    let s = &m["slotCrc"];
    let file = golden(text(&s["file"]));
    let row = num(&s["row"]) as usize;
    let raw = slot_at(&file, row);
    assert_eq!(mpwf::slot_crc(raw, row as u32) as u64, num(&s["crc"]));
    assert_eq!(prim::u32_at(raw, 252) as u64, num(&s["crc"]));
    assert_eq!(crc32(&raw[..252]) as u64, num(&s["unseeded"]));

    // WVSL's CRC depends only on the row count when every slot passes: two
    // files that share nothing but that have one, and so one header CRC (4.2).
    let sections = m["sectionCrc"].as_array().expect("sectionCrc");
    assert_eq!(sections.len(), 2);
    for e in sections {
        let file = golden(text(&e["file"]));
        let t = mpwf::Table::open(&file).expect("the golden");
        assert_eq!(t.rows as u64, num(&e["rows"]));
        assert_eq!(prim::u32_at(&file, 224 + 24) as u64, num(&e["crc"]));
        assert_eq!(crc32(t.wvsl()) as u64, num(&e["crc"]));
        let hashes: Vec<u64> = (1..=t.rows as u64).map(|r| r * 0x0012_3456_789A_BCDE).collect();
        let other = table_of(&hashes);
        assert_eq!(prim::u32_at(&other, 224 + 24) as u64, num(&e["crc"]));
    }
}

#[test]
fn vectors_mpwf_search() {
    let v = vectors();
    let m = &v["mpwf"];
    let picks = m["pick"].as_array().expect("pick");
    assert!(picks.len() >= 10);
    for e in picks {
        let field = |k: &str| num(&e[k]) as u32;
        let got = mpwf::pick(num(&e["h"]), field("lo"), field("hi"), num(&e["loH"]), num(&e["hiH"]), field("probes"));
        assert_eq!(got as u64, num(&e["row"]), "{}", e["note"]);
    }
    // In 64 bits at the largest product: every answer inside lo..hi, whatever
    // the hash and the bounds.
    let mut rng = fastrand::Rng::with_seed(20_261_102);
    for _ in 0..200_000 {
        let (a, b) = (rng.u64(..), rng.u64(..));
        let (a, b) = (a.min(b), a.max(b));
        let h = rng.u64(a..=b);
        let lo = rng.u32(..mpwf::MAX_ROWS);
        let hi = rng.u32(lo..mpwf::MAX_ROWS);
        let probes = rng.u32(..6);
        let r = mpwf::pick(h, lo, hi, a, b, probes);
        assert!((lo..=hi).contains(&r), "{h} {lo} {hi} {a} {b} {probes}");
        assert!(probes < 4 || r == lo + (hi - lo) / 2);
    }

    // Each lookup's answer and the rows it reads, in order.
    let files = m["search"]["files"].as_object().expect("files");
    assert_eq!(files.len(), 3);
    let mut lookups = 0;
    for (name, cases) in files {
        let file = golden(name);
        let t = mpwf::Table::open(&file).expect("the golden");
        for e in cases.as_array().expect("cases") {
            let what = format!("{name} {}", e["hash"]);
            let found = t.find(num(&e["hash"]));
            assert_eq!(lookup_name(found.lookup), text(&e["read"]), "{what}");
            assert_eq!(found.lookup.why(), Why::Ok, "{what}");
            let probes: Vec<u64> = e["probes"].as_array().expect("probes").iter().map(num).collect();
            assert_eq!(found.probes.iter().map(|&r| r as u64).collect::<Vec<_>>(), probes, "{what}");
            if let Some(row) = e.get("row") {
                let r = num(row) as u32;
                assert_eq!(found.row, Some(r), "{what}");
                let slot = found.slot.as_ref().expect("a hit's slot");
                assert_eq!(slot.path_hash, num(&e["hash"]), "{what}");
                assert_eq!(slot.encode(r).as_slice(), slot_at(&file, r as usize), "{what}");
            }
            lookups += 1;
        }
    }
    assert!(lookups >= 20);

    // What the search costs: the same reads, summed, on the two sets of hashes
    // built by rule (13.3).
    let scale = &m["scale"];
    let first: Vec<u64> = scale["splitmix64"].as_array().expect("splitmix64").iter().map(num).collect();
    let mut state = 0;
    assert_eq!((0..3).map(|_| splitmix64(&mut state)).collect::<Vec<_>>(), first);
    let reads = |file: &[u8], hashes: &[u64], want: mpwf::Lookup| {
        let t = mpwf::Table::open(file).expect("the table");
        let mut out = (0u64, 0usize);
        for &h in hashes {
            let found = t.find(h);
            assert_eq!(found.lookup, want, "{h:#x}");
            out = (out.0 + found.probes.len() as u64, out.1.max(found.probes.len()));
        }
        out
    };
    let check = |e: &Value, rows: &[u64], misses: &[u64]| {
        assert_eq!((rows.len() as u64, misses.len() as u64), (num(&e["rows"]), num(&e["misses"])));
        let file = table_of(rows);
        let (hit_reads, hit_max) = reads(&file, rows, mpwf::Lookup::Hit);
        let (miss_reads, miss_max) = reads(&file, misses, mpwf::Lookup::Miss);
        assert_eq!((hit_reads, hit_max as u64), (num(&e["hitReads"]), num(&e["hitMax"])));
        assert_eq!((miss_reads, miss_max as u64), (num(&e["missReads"]), num(&e["missMax"])));
    };
    let e = &scale["uniform"];
    let mut state = 0;
    let rows: Vec<u64> = (0..num(&e["rows"])).map(|_| splitmix64(&mut state)).collect();
    let misses: Vec<u64> = (0..num(&e["misses"])).map(|_| splitmix64(&mut state)).collect();
    check(e, &rows, &misses);
    assert_eq!(text(&scale["pattern"]), "Generated Artist %03d/Sample Album %d/%d - Sample Title.mp3");
    assert_eq!(text(&scale["missPattern"]), "Hand Copied/%05d.flac");
    let mut rows = Vec::new();
    for artist in 0..400 {
        for album in 0..5 {
            for track in 0..10 {
                let rel = format!("Generated Artist {artist:03}/Sample Album {album}/{track} - Sample Title.mp3");
                rows.push(path_hash(&rel));
            }
        }
    }
    let misses: Vec<u64> = (0..5000).map(|i| path_hash(&format!("Hand Copied/{i:05}.flac"))).collect();
    check(&scale["paths"], &rows, &misses);
}

/// The row limit (13.3): 250,000 rows open and 250,001 read `enum`, from the
/// vectors' first 256 bytes and no slot; a writer writes the one and refuses
/// the other. The most a lookup can take, at the most rows, consecutive
/// hashes at either end: 22 reads (6).
#[test]
fn vectors_mpwf_row_limit() {
    let v = vectors();
    let cases = v["mpwf"]["rowLimit"].as_array().expect("rowLimit");
    assert_eq!(cases.len(), 2);
    for e in cases {
        // Zero pages the system gives lazily: no 64 MB touched.
        let mut file = vec![0u8; num(&e["fileBytes"]) as usize];
        file[..256].copy_from_slice(&hex_bytes(text(&e["head"])));
        let t = mpwf::Table::open(&file);
        assert_eq!(t.as_ref().err().copied().unwrap_or(Why::Ok).name(), text(&e["why"]));
        assert_eq!(t.map_or(0, |t| t.rows) as u64, if text(&e["why"]) == "ok" { num(&e["rows"]) } else { 0 });
    }

    let at_most = mpwf::MAX_ROWS as u64;
    let want = hex_bytes(text(&cases[0]["head"]));
    let row = |path_hash: u64| mpwf::Slot { path_hash, length_ms: 0, bars: [0; mpwf::BAR_COUNT] };
    for base in [0, u64::MAX - (at_most - 1)] {
        let rows: Vec<mpwf::Slot> = (0..at_most).map(|r| row(base + r)).collect();
        let (generation, card_id) = (prim::u32_at(&want, 20), prim::u64_at(&want, 24));
        let mut t = mpwf::TableIn { generation, card_id, minor: 0, rows };
        let file = mpwf::write(&t).expect("the most rows");
        assert_eq!(file.len() as u64, num(&cases[0]["fileBytes"]));
        // Its frame is the vector's but for WVSL's CRC (0 there: no reader of
        // that head reads the slots) and so the header's.
        assert!(file[..32] == want[..32] && file[36..248] == want[36..248] && file[252..256] == want[252..256]);
        let table = mpwf::Table::open(&file).expect("the most rows");
        let mut most = 0;
        for r in (0..at_most).step_by(251).chain(0..64).chain(at_most - 64..at_most) {
            let found = table.find(base + r);
            assert_eq!((found.lookup, found.row), (mpwf::Lookup::Hit, Some(r as u32)));
            most = most.max(found.probes.len());
        }
        assert_eq!(most, 22, "the guard of 24 can't fire on a valid file");
        t.rows.push(row(base.wrapping_sub(1)));
        assert!(mpwf::write(&t).is_err(), "250,001 rows");
    }
}

/// A file's first bytes and its length say what the whole file does: no check
/// of 8.1 or 8.2 reads a slot, and a head of 256 bytes opens 250,000 rows.
#[test]
fn mpwf_files_read_the_same_from_their_head() {
    let head = |b: &[u8]| mpwf::open_head(&b[..b.len().min(mpwf::HEAD_BYTES)], b.len() as u64);
    let whole = |b: &[u8]| mpwf::Table::open(b).map(|t| *t.header());
    let index = json("hardening/index.json");
    let files = index.as_array().expect("the index").iter().filter(|e| text(&e["format"]) == "MPWF");
    assert_eq!(files.clone().count(), 50);
    for e in files {
        let b = bytes(&format!("hardening/{}", text(&e["file"])));
        assert_eq!(head(&b), whole(&b), "{}", text(&e["file"]));
    }
    for name in ["waves-one.bin", "waves-0000002a.bin", "waves-skew.bin"] {
        let b = golden(name);
        assert_eq!(head(&b), whole(&b), "{name}");
        assert!(head(&b).is_ok(), "{name}");
        // Its length is checked, whatever the head says.
        assert_eq!(mpwf::open_head(&b[..256], b.len() as u64 + 256), Err(Why::FileBytes), "{name}");
        // A head that stops short of the directory can't vouch for it.
        assert_eq!(mpwf::open_head(&b[..200], b.len() as u64), Err(Why::Short), "{name}");
    }
    let v = vectors();
    for e in v["mpwf"]["rowLimit"].as_array().expect("rowLimit") {
        let got = mpwf::open_head(&hex_bytes(text(&e["head"])), num(&e["fileBytes"]));
        assert_eq!(got.err().unwrap_or(Why::Ok).name(), text(&e["why"]), "{e}");
    }
}

#[test]
fn vectors_mpwf_device_txt_and_commit() {
    let v = vectors();
    let m = &v["mpwf"];
    assert_eq!(m["deviceTxt"]["who"], "software");
    for e in m["deviceTxt"]["cases"].as_array().expect("deviceTxt") {
        let txt = mpwf_device_text(e);
        let t = DeviceTxt::read(txt.as_deref().map(str::as_bytes));
        assert_eq!(t.writes_mpwf(), e["writes"].as_bool().expect("a bool"), "{e}");
        assert_eq!(t.reads(Format::Mpwf, 1), t.writes_mpwf(), "{e}");
    }
    // The fixture card's, every key in 2.15's place: read.mpwf after read.mpdj,
    // history last.
    let file = DeviceTxt::read(Some(&golden("device.txt")));
    assert_eq!(file.read_mpwf, 1);
    let txt = file.to_text();
    let keys: Vec<&str> = txt.lines().filter_map(|l| l.split_once('=')).map(|kv| kv.0).collect();
    assert_eq!(keys, [
        "contract",
        "firmware",
        "read.msmf",
        "read.mptg",
        "read.mpdj",
        "read.mpwf",
        "read.mpth",
        "codecs",
        "extensions",
        "max_rate",
        "max_channels",
        "history",
    ]);

    // 7.3's table, the first line that holds.
    assert_eq!(m["commit"]["who"], "software");
    let cases = m["commit"]["cases"].as_array().expect("commit");
    assert!(cases.len() >= 8);
    for e in cases {
        let note = text(&e["note"]);
        let txt = mpwf_device_text(e);
        let writes = DeviceTxt::read(txt.as_deref().map(str::as_bytes)).writes_mpwf();
        let old = e["old"].as_str().map(golden);
        let table = old.as_ref().map(|b| mpwf::Table::open(b).expect("the golden"));
        let want = match text(&e["outcome"]) {
            "none" => mpwf::Outcome::None,
            "same" => mpwf::Outcome::Same,
            "new" => mpwf::Outcome::New,
            other => panic!("an outcome: {other}"),
        };
        assert_eq!(mpwf::outcome(writes, table.as_ref(), &commit_rows(e)), want, "{note}");
    }
}

/// What a reader takes from a root's COMP (7.2, `rootRow`): a bad row costs
/// only itself, and its name is still a string of the root.
#[test]
fn vectors_mpwf_root_row() {
    let v = vectors();
    let r = &v["mpwf"]["rootRow"];
    let mut cases: Vec<(String, &Value)> = vec![(format!("golden/{}", text(&r["golden"]["file"])), &r["golden"])];
    let rows = r["hardening"].as_array().expect("hardening");
    cases.extend(rows.iter().map(|e| (format!("hardening/{}", text(&e["file"])), e)));
    assert_eq!(cases.len(), 12);
    for (rel, e) in cases {
        let file = bytes(&rel);
        // The device takes the row too: it reads this kind.
        for uses in [msmf::Uses::ALL, msmf::Uses::DEVICE] {
            let m = Manifest::read(&file, uses).expect(&rel);
            assert_eq!(m.tags.name(), "tags-0000002a.bin", "{rel}");
            if !rel.contains("no-mpdj") {
                assert_eq!(m.autodj.as_ref().map(Comp::name).as_deref(), Some("autodj-00000029.bin"), "{rel}");
            }
            let Some(name) = e["waves"].as_str() else {
                assert_eq!(m.waves, None, "{rel}");
                continue;
            };
            let row = m.waves.as_ref().expect(&rel);
            assert_eq!((row.kind, row.name().as_str()), (MPWF, name), "{rel}");
            // The row against the golden file of that name; the signature an
            // MPDJ row compares isn't looked at.
            let waves = golden(name);
            let t = mpwf::Table::open(&waves).expect("the golden");
            let matches = e["matches"].as_bool().expect("a bool");
            assert_eq!(msmf::companion_matches(row, t.header(), 0, None), matches, "{rel}");
            assert_eq!(msmf::companion_matches(row, t.header(), 0, Some(&[1; 16])), matches, "{rel}");
        }
    }
    // The two whose name breaks a string rule fail the root, as for any kind.
    for (name, why) in [("msmf-waves-name-range.bin", Why::String), ("msmf-waves-name-order.bin", Why::StringOrder)] {
        assert_eq!(read_as("MSMF", &bytes(&format!("hardening/{name}")), false), why, "{name}");
    }
    // An unknown kind stays ignored, and doesn't count as a second row.
    let root = Manifest::read(&bytes("hardening/msmf-comp-unknown-kind.bin"), msmf::Uses::ALL).expect("a root");
    assert_eq!(root.waves, None);

    // manifest-waves.bin carries its file from generation 42: the file named
    // again, older than the root. The fixture card's own root names none.
    let m = Manifest::read(&golden("manifest-waves.bin"), msmf::Uses::ALL).expect("a root");
    let row = m.waves.clone().expect("its row");
    assert_eq!((m.header.generation, row.generation, row.key), (43, 42, [0; 16]));
    assert_eq!(Comp::of(&golden("waves-0000002a.bin")).as_ref(), Some(&row));
    assert_eq!(Manifest::read(&golden("manifest.bin"), msmf::Uses::ALL).expect("a root").waves, None);
    // The row against a header: the file named, and any other not.
    let t = mpwf::Table::open(&golden("waves-0000002a.bin")).map(|t| *t.header()).expect("the golden");
    assert!(msmf::companion_matches(&row, &t, 0, None));
    let others = [
        container::Header { magic: MPDJ, ..t },
        container::Header { generation: t.generation ^ 1, ..t },
        container::Header { file_bytes: t.file_bytes ^ 1, ..t },
        container::Header { header_crc: t.header_crc ^ 1, ..t },
    ];
    assert!(others.iter().all(|h| !msmf::companion_matches(&row, h, 0, None)));
    let mut keyed = row.clone();
    keyed.key[15] = 1;
    assert!(!msmf::companion_matches(&keyed, &t, 0, None));

    // Carried forward, a row reads as this writer writes it: after the MPTG
    // and the MPDJ, its name after theirs, its key zeros. A row that didn't
    // count goes.
    let golden_root = golden("manifest-waves.bin");
    for name in ["msmf-waves-between.bin", "msmf-waves-first.bin"] {
        let m = Manifest::read(&bytes(&format!("hardening/{name}")), msmf::Uses::ALL).expect(name);
        assert!(msmf::write(&m.to_input()).expect("carried forward") == golden_root, "{name}");
    }
    let m = Manifest::read(&bytes("hardening/msmf-waves-key.bin"), msmf::Uses::ALL).expect("a root");
    assert_ne!(m.waves.as_ref().map(|c| c.key), Some([0; 16]));
    assert!(msmf::write(&m.to_input()).expect("carried forward") == golden_root);
    let m = Manifest::read(&bytes("hardening/msmf-waves-two.bin"), msmf::Uses::ALL).expect("a root");
    let again = msmf::write(&m.to_input()).expect("carried forward");
    assert_eq!(Manifest::read(&again, msmf::Uses::ALL).expect("a root").waves, None);
}

/// 13.4's facts about the goldens, each spelled out here and not taken from
/// the reader; and what the device's reader makes of a cut or a flipped bit:
/// it opens only the first 256 bytes (13.6).
#[test]
fn the_waves_goldens_read_back() {
    let file = golden("waves-0000002a.bin");
    let t = mpwf::Table::open(&file).expect("the golden");
    t.check().expect("a whole file");
    let h = t.header();
    assert_eq!((h.major, h.minor, h.header_bytes, h.section_count), (1, 0, 224, 1));
    assert_eq!((h.file_bytes, h.generation, h.card_id), (2048, 42, 0x5EED_C0DE_0A1B_2C3D));
    assert_eq!(t.rows, 7);
    let rows = [
        ("Cafe\u{301}/Album/01 - Title.mp3", 0x5701_7C0C_C011_BB1B, 193_000),
        ("Artist/Album/CD1/01 - Disc One.flac", 0x6137_B0E2_A671_5752, 183_000),
        ("A/x.mp3", 0x954A_7DE3_A079_A42E, 186_000),
        ("Artist/x.mp3", 0xC486_74AC_937F_2ADA, 0),
        ("Artist/Album/02 - Other.mp3", 0xCADB_3EFD_EF4B_2DD9, 181_000),
        ("Artist/Album/01 - Title.mp3", 0xDA70_B312_7601_88D8, 180_000),
        ("Caf\u{e9}/Album/01 - Title.mp3", 0xF1B2_4FC7_5749_4F2B, 192_000),
    ];
    let slots: Vec<mpwf::Slot> = (0..7).map(|r| t.slot(r).expect("a good slot")).collect();
    for (r, (path, hash, ms)) in rows.iter().enumerate() {
        assert_eq!((path_hash(path), slots[r].path_hash, slots[r].length_ms), (*hash, *hash, *ms), "{path}");
        assert_eq!(t.find(*hash).row, Some(r as u32), "{path}");
    }
    // Their bars: M = 3, 80 of each; silence has a slot; a brick; every third
    // bar a tie, rounded up; a ramp to 254; M = 2, 120 of each.
    let of = |k: u8, n: usize| vec![k; n];
    assert_eq!(slots[0].bars.as_slice(), [of(30, 80), of(40, 80), of(50, 80)].concat());
    assert!(slots[1].bars.iter().any(|&b| b != 0) && slots[1].bars.iter().all(|&b| b <= 239));
    assert_eq!(slots[2].bars, [0; mpwf::BAR_COUNT]);
    assert_eq!(slots[3].bars, [255; mpwf::BAR_COUNT]);
    for (k, &b) in slots[4].bars.iter().enumerate() {
        assert_eq!(b as usize, if k % 3 == 2 { k / 3 + 1 } else { 0 }, "bar {k}");
    }
    assert!(slots[5].bars.is_sorted() && slots[5].bars[239] == 254);
    assert_eq!(slots[6].bars.as_slice(), [of(10, 120), of(20, 120)].concat());

    // waves-one.bin, the smallest file: 13.4's dump.
    let one = golden("waves-one.bin");
    assert_eq!(one.len(), 512);
    assert_eq!(spaced_of(&one[..16]), "4D 50 57 46 01 00 00 00 E0 00 00 00 01 00 00 00");
    assert_eq!(spaced_of(&one[16..32]), "00 02 00 00 01 00 00 00 3D 2C 1B 0A DE C0 ED 5E");
    assert_eq!(spaced_of(&one[32..48]), "26 AC D6 9D 00 00 00 00 00 01 00 00 F0 00 00 00");
    assert!(one[48..224].iter().all(|&b| b == 0));
    assert_eq!(spaced_of(&one[224..240]), "57 56 53 4C 01 00 00 00 00 01 00 00 00 01 00 00");
    assert_eq!(spaced_of(&one[240..256]), "01 00 00 00 00 01 00 00 5F 2A 7B BD 00 00 00 00");
    assert_eq!(spaced_of(&one[256..272]), "D8 88 01 76 12 B3 70 DA 20 BF 02 00 40 40 40 40");
    assert_eq!(spaced_of(&one[508..]), "1C 4F EF B0");

    // waves-skew.bin: 49 rows from hash 0 to 2^64 − 1, and the two tracks of
    // one hash left out.
    let skew = golden("waves-skew.bin");
    let t = mpwf::Table::open(&skew).expect("the golden");
    t.check().expect("a whole file");
    let hash = |r: u32| t.slot(r).expect("a good slot").path_hash;
    assert_eq!((t.rows, hash(0), hash(40), hash(48)), (49, 0, 1 << 62, u64::MAX));
    assert_eq!(t.find(1 << 63).lookup, mpwf::Lookup::Miss);
    assert_eq!(t.find(20).probes, [0, 1, 2, 3, 26, 14, 20], "num is 0, so r = lo, four times, then it bisects");

    // The bytes after the first 256 are WVSL's: the device reads them when it
    // needs a slot, and the frame alone stands or falls on the rest.
    for name in ["waves-one.bin", "waves-0000002a.bin"] {
        let file = golden(name);
        assert!((0..file.len()).all(|n| read_as("MPWF", &file[..n], true) != Why::Ok), "{name} cut");
        let mut b = file.clone();
        for at in 0..b.len() {
            for bit in 0..8 {
                b[at] ^= 1 << bit;
                let why = read_as("MPWF", &b, true);
                if at < 256 {
                    assert_ne!(why, Why::Ok, "{name}: byte {at} bit {bit}");
                } else {
                    assert_eq!(why, Why::Ok, "{name}: byte {at} bit {bit}");
                }
                b[at] ^= 1 << bit;
            }
        }
    }
}

/// The checks of a slot in their place (8.3, 8.4), and the search on tables of
/// every shape against a walk from row 0: every row found at its row, nothing
/// else found.
#[test]
fn a_damaged_slot_a_broken_writer_and_tables_of_every_shape() {
    // A damaged slot costs only the lookups that read it: the file stays, and
    // only a whole-file check reads it as the file's absence.
    let file = golden("waves-0000002a.bin");
    let t = mpwf::Table::open(&file).expect("the golden");
    let hashes: Vec<u64> = (0..7).map(|r| t.slot(r).expect("a good slot").path_hash).collect();
    for bad in 0..7usize {
        let mut b = file.clone();
        b[256 + bad * 256 + 100] ^= 0x10;
        let t = mpwf::Table::open(&b).expect("its frame is good");
        assert_eq!(t.check(), Err(Why::WvSlotCrc));
        assert_eq!(t.slot(bad as u32), Err(Why::WvSlotCrc));
        let mut damaged = 0;
        for (r, &h) in hashes.iter().enumerate() {
            let found = t.find(h);
            let read_bad = found.probes.contains(&(bad as u32));
            let want = if read_bad { mpwf::Lookup::Damaged } else { mpwf::Lookup::Hit };
            assert_eq!(found.lookup, want, "slot {bad} damaged, row {r}");
            assert_eq!(found.lookup.why(), if read_bad { Why::WvSlotCrc } else { Why::Ok });
            damaged += read_bad as u32;
        }
        assert!(damaged >= 1, "its own row at least");
    }
    assert_eq!(t.slot(t.rows), Err(Why::WvSlotCrc), "no such row");
    assert_eq!(t.slot_raw(t.rows), None);

    // A good CRC over a hash that breaks the sort rule: at the first bound
    // that meets it, whichever bound it is, a hash equal to a bound too. A
    // slot a lookup doesn't read can't stop it.
    let good: Vec<u64> = (0..7).map(|i| 0x1000_0000_0000_0000 + i * 0x2000_0000_0000_0000).collect();
    let good = table_of(&good);
    let with_hash = |row: usize, hash: u64| {
        let mut b = good.clone();
        let slot = &mut b[256 + row * 256..][..256];
        slot[..8].copy_from_slice(&hash.to_le_bytes());
        let crc = mpwf::slot_crc(slot, row as u32);
        slot[252..].copy_from_slice(&crc.to_le_bytes());
        b
    };
    let cases = [
        (1, 0x6000_0000_0000_0000, 0x4000_0000_0000_0000, mpwf::Lookup::Broken),
        (1, 0x5000_0000_0000_0000, 0x4000_0000_0000_0000, mpwf::Lookup::Broken),
        (4, 0x6000_0000_0000_0000, 0x8000_0000_0000_0000, mpwf::Lookup::Broken),
        (4, 0x7000_0000_0000_0000, 0x8000_0000_0000_0000, mpwf::Lookup::Broken),
        (1, 0x6000_0000_0000_0000, 0xB000_0000_0000_0000, mpwf::Lookup::Hit),
    ];
    for (row, hash, look_for, want) in cases {
        let b = with_hash(row, hash);
        let t = mpwf::Table::open(&b).expect("its frame is good");
        assert_eq!(t.find(look_for).lookup, want, "{row} {hash:#x} {look_for:#x}");
        assert_eq!(t.check(), Err(Why::WvSlotOrder), "{row} {hash:#x}");
    }
    // What a whole-file check meets first: the CRC before the order, row by
    // row; WVSL's CRC last.
    let mut b = table_of(&[1 << 60, 2 << 60, 3 << 60]);
    b[256 + 256 + 7] = 0x90;
    assert_eq!(mpwf::Table::open(&b).expect("its frame").check(), Err(Why::WvSlotCrc));
    reseal_slots(&mut b);
    assert_eq!(mpwf::Table::open(&b).expect("its frame").check(), Err(Why::WvSlotOrder));
    b[256 + 256 + 7] = 0x20;
    reseal_slots(&mut b);
    assert_eq!(mpwf::Table::open(&b).expect("its frame").check(), Ok(()));

    // Every small count, the extremes in place; skewed: everything low,
    // everything high, a clump, powers of two, clumps at both ends, spans
    // around the 2^32 where the interpolation gives way. The search never goes
    // wrong on them, only slower.
    let walk = |hashes: &[u64], what: &str| {
        let mut sorted = hashes.to_vec();
        sorted.sort_unstable();
        let file = table_of(hashes);
        assert_eq!(file.len() as u32, mpwf::file_bytes_for(sorted.len() as u32), "{what}");
        let t = mpwf::Table::open(&file).expect(what);
        t.check().expect(what);
        for (r, &h) in sorted.iter().enumerate() {
            let found = t.find(h);
            assert_eq!((found.lookup, found.row), (mpwf::Lookup::Hit, Some(r as u32)), "{what}: {h:#x}");
            assert!(found.probes.len() <= 22, "{what}: {h:#x}");
            for near in [h.wrapping_sub(1), h.wrapping_add(1)] {
                if sorted.binary_search(&near).is_err() {
                    let found = t.find(near);
                    assert_eq!(found.lookup, mpwf::Lookup::Miss, "{what}: {near:#x}");
                    assert!(found.probes.len() <= 22, "{what}: {near:#x}");
                }
            }
        }
    };
    let mut rng = fastrand::Rng::with_seed(20_261_103);
    for n in 1..=70 {
        let mut hashes: BTreeSet<u64> = BTreeSet::new();
        while hashes.len() < n {
            hashes.insert(rng.u64(..));
        }
        let mut hashes: Vec<u64> = hashes.into_iter().collect();
        walk(&hashes, "uniform");
        hashes[0] = 0;
        walk(&hashes, "with hash 0");
        *hashes.last_mut().expect("a row") = u64::MAX;
        walk(&hashes, "with hash 0 and 2^64 - 1");
    }
    let low: Vec<u64> = (0..3000).collect();
    walk(&low, "consecutive from 0");
    walk(&low.iter().map(|i| u64::MAX - i).collect::<Vec<_>>(), "consecutive up to 2^64 - 1");
    walk(&low.iter().map(|i| 0x7000_0000_0000_0000 + i * 3).collect::<Vec<_>>(), "a clump");
    walk(&low.iter().map(|&i| if i % 2 == 1 { i } else { u64::MAX - i }).collect::<Vec<_>>(), "clumps at both ends");
    walk(&(0..64).map(|bit| 1u64 << bit).collect::<Vec<_>>(), "powers of two");
    walk(&(0..2000).map(|i| 0x1_2345_6789 + i * 0x100_0000).collect::<Vec<_>>(), "under and over 2^32");
}

/// The writer's edges (4, 7.3): no row is no file; a hash two rows share is
/// left out, both, whichever order they came in; each CRC is made for its
/// place; the header's padding and the reserved fields are zeros.
#[test]
fn the_waves_writer_at_its_edges() {
    let d = library("waves-0000002a");
    let mut input = waves_input(&d);
    let want = golden("waves-0000002a.bin");
    assert!(mpwf::write(&input).expect("an index") == want);
    let mut rng = fastrand::Rng::with_seed(13);
    for _ in 0..8 {
        rng.shuffle(&mut input.rows);
        assert!(mpwf::write(&input).expect("in any order") == want);
    }

    let row = |path_hash, length_ms| mpwf::Slot { path_hash, length_ms, bars: [7; mpwf::BAR_COUNT] };
    let with = |rows| mpwf::TableIn { generation: 3, card_id: 0x1122_3344_5566_7788, minor: 2, rows };
    assert!(mpwf::write(&with(Vec::new())).is_err(), "a writer with no row writes no file");
    assert!(mpwf::write(&with(vec![row(3, 0), row(3, 1), row(3, 2)])).is_err(), "nothing left");
    let rows = vec![row(3, 300), row(1, 111), row(2, 200), row(9, 900), row(2, 222)];
    assert_eq!(
        mpwf::in_row_order(&rows).iter().map(|s| s.path_hash).collect::<Vec<_>>(),
        [1, 3, 9],
        "a shared hash left out"
    );
    let file = mpwf::write(&with(rows)).expect("an index");
    let t = mpwf::Table::open(&file).expect("reads");
    t.check().expect("a whole file");
    let h = t.header();
    assert_eq!((t.rows, h.minor, h.generation, h.card_id), (3, 2, 3, 0x1122_3344_5566_7788));
    assert_eq!(t.find(2).lookup, mpwf::Lookup::Miss);
    assert_eq!(t.find(1).slot.as_ref().map(|s| s.length_ms), Some(111));
    assert_eq!((prim::u32_at(&file, 36), prim::u32_at(&file, 40), prim::u32_at(&file, 44)), (0, 256, 240));
    assert!(file[48..224].iter().all(|&b| b == 0));
    assert_eq!(prim::u32_at(&file, 224 + 28), 0);

    // The root's writer: COMP's order is a writer's rule (7.2).
    let tags = Comp::of(&golden("tags-0000002a.bin")).expect("a companion");
    let dj = Comp::of(&golden("autodj-00000029.bin")).expect("a companion");
    let waves = Comp::of(&want).expect("a companion");
    assert_eq!((waves.kind, waves.name().as_str(), waves.file_bytes), (MPWF, "waves-0000002a.bin", 2048));
    let root = |comps: &[&Comp]| {
        let comps = comps.iter().map(|c| (*c).clone()).collect();
        msmf::write(&ManifestIn { generation: 43, comps, ..ManifestIn::default() })
    };
    assert!(root(&[&tags]).is_ok() && root(&[&tags, &dj]).is_ok());
    for comps in [&[&tags, &waves][..], &[&tags, &dj, &waves][..]] {
        let m = Manifest::read(&root(comps).expect("a root"), msmf::Uses::ALL).expect("reads");
        assert_eq!(m.waves.as_ref(), Some(&waves));
    }
    for comps in [
        &[&waves][..],
        &[&waves, &tags][..],
        &[&tags, &waves, &dj][..],
        &[&tags, &waves, &waves][..],
        &[&tags, &dj, &dj][..],
        &[&tags, &dj, &waves, &waves][..],
    ] {
        assert!(root(comps).is_err(), "{:?}", comps.iter().map(|c| c.name()).collect::<Vec<_>>());
    }
    // A key is an MPDJ row's alone: a writer writes zeros.
    let mut keyed = waves.clone();
    keyed.key[3] = 0xFF;
    let m = Manifest::read(&root(&[&tags, &keyed]).expect("a root"), msmf::Uses::ALL).expect("reads");
    assert_eq!(m.waves.as_ref(), Some(&waves));
}

// ── The mood tail (2.6.4; the player's MOOD-FILTERS.md 5.3) ─────────────────

/// A tail as the fixtures spell one, `03 3A 46 1D 31 FF 46 00`.
fn tail_of(v: &Value) -> Tail {
    spaced(text(v)).try_into().expect("a tail of 8 bytes")
}

/// The player's mood word of a tail (METADATA 3.4.3, `lib/core/
/// MoodFilter.cpp`'s wordOfTail). The device makes it, never this kit: it
/// is here to show that the tails this kit writes and takes are those whose
/// words the fixtures give. 0 for a tail a reader refuses; else analysed, a
/// scale's field side << 2 | bucket (4 for NULL, its order bit ignored, and
/// bucket 0 with no side), the voice NULL bit for a NULL instrumental, and
/// a NULL tag 0.
fn mood_word(t: &Tail) -> u16 {
    if moods::read_tail(t).is_none() {
        return 0;
    }
    let bucket = |v: u8| [30, 50, 70].iter().filter(|&&bar| v >= bar).count() as u16;
    let scale = |v: u8, second: bool| match (v, bucket(v)) {
        (moods::NULL, _) => 4,
        (_, b) if b > 0 && second => 4 | b,
        (_, b) => b,
    };
    let tag = |v: u8| if v == moods::NULL { 0 } else { bucket(v) };
    let valence = scale(t[1], t[0] & moods::SAD_WINS != 0) << 11;
    let energy = scale(t[2], t[0] & moods::AGGRESSIVE_WINS != 0) << 8;
    let voice = if t[3] == moods::NULL { 1 << 14 } else { bucket(t[3]) << 6 };
    1 << 15 | valence | energy | voice | tag(t[4]) << 4 | tag(t[5]) << 2 | tag(t[6])
}

/// Every row of moods.json: its scores make its tail, which a reader takes
/// (none for no row), whose word is the row's.
#[test]
fn moods_json_rows_make_their_tails_and_words() {
    let m = json("moods.json");
    let heads: Vec<&str> = m["heads"].as_array().expect("heads").iter().map(text).collect();
    assert_eq!(heads, moods::HEADS, "a row's slots");
    let rows = m["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 40);
    for r in rows {
        let what = text(&r["note"]);
        let scores = (!r["scores"].is_null()).then(|| scores_of(&r["scores"]));
        let tail = moods::encode(scores.as_ref());
        assert_eq!(spaced_of(&tail), text(&r["tail"]), "{what}");
        assert_eq!(moods::read_tail(&tail), scores.map(|_| tail), "{what}: a tail this kit writes reads");
        assert_eq!(u64::from(mood_word(&tail)), num(&r["word"]), "{what}");
    }
}

/// Every tail of moods.json is taken or refused as listed, and a tail taken
/// is kept as it came, an order bit beside a NULL included.
#[test]
fn moods_json_tails_read_as_listed() {
    let tails = json("moods.json")["tails"].as_array().expect("tails").clone();
    assert_eq!(tails.len(), 23);
    for e in &tails {
        let (t, what) = (tail_of(&e["tail"]), text(&e["note"]));
        let read = moods::read_tail(&t);
        assert_eq!(read.is_some(), e["valid"] == true, "{what}");
        assert!(read.is_none_or(|r| r == t), "{what}");
        assert_eq!(u64::from(mood_word(&t)), num(&e["word"]), "{what}");
    }
    assert_eq!(moods::read_tail(&[1, 0, 0, 0, 0, 0, 0]), None, "seven bytes are no tail");
}

/// moods.json's values: each score's byte by the slack floor, clamped, 255
/// for NULL.
#[test]
fn moods_json_values_follow_the_value_rule() {
    for v in json("moods.json")["values"].as_array().expect("values") {
        assert_eq!(u64::from(moods::value(v["score"].as_f64())), num(&v["value"]), "{v}");
    }
    // The products a plain floor gets wrong.
    let floor = |s: f64| (s * 100.0).floor();
    assert_eq!([0.29, 0.57, 0.58].map(floor), [28.0, 56.0, 57.0]);
    assert_eq!([0.29, 0.57, 0.58].map(|s| moods::value(Some(s))), [29, 57, 58]);
}

/// A tail a reader refuses costs its record the scores and nothing more:
/// each of moods.json's tails in turn on one record of tags-moods.bin, the
/// file resealed, reads for the software and the device alike, every record
/// as it was but that one's tail, taken or not as listed.
#[test]
fn a_tail_counts_only_when_it_validates() {
    let file = golden("tags-moods.bin");
    let want = mptg::read(&file, mptg::Uses::ALL).expect("the golden");
    let (off, len) = section_span(&file, mptg::RECS);
    assert_eq!(len, 80 * want.records.len());
    let row = want.records.iter().position(|e| e.path == "Artist A/scored.flac").expect("the record");
    for e in json("moods.json")["tails"].as_array().expect("tails") {
        let (t, what) = (tail_of(&e["tail"]), text(&e["note"]));
        let mut b = file.clone();
        b[off + 80 * row + 72..][..8].copy_from_slice(&t);
        reseal(&mut b);
        for uses in [mptg::Uses::ALL, mptg::Uses::DEVICE] {
            let got = mptg::read(&b, uses).unwrap_or_else(|w| panic!("{what}: {w}"));
            for (i, (g, w)) in got.records.iter().zip(&want.records).enumerate() {
                let taken = if e["valid"] == true { t } else { moods::NO_ROW };
                let moods = if i == row { taken } else { w.rec.moods };
                assert_eq!(g.rec, Record { moods, ..w.rec }, "{what}: {}", w.path);
                assert_eq!(g.rec.has_moods(), moods != moods::NO_ROW, "{what}");
            }
        }
    }
}

/// 72 stays the smallest RECS stride a reader takes (2.4.5): rows of 72,
/// 76, 80 and 88 all read, the tail only from rows of 80 or more, and a
/// writer with no tail to write writes 72, as every card before minor 1.
#[test]
fn the_stride_constant_stays_72() {
    assert_eq!((mptg::RECS_STRIDE, mptg::RECS_MOODS_STRIDE), (72, 80));
    assert_eq!((moods::TAIL_AT, moods::TAIL_AT + moods::TAIL_BYTES), (72, 80));
    let files = [
        ("golden/tags-0000002a.bin", 72, false),
        ("hardening/mptg-stride-76.bin", 76, false),
        ("golden/tags-moods.bin", 80, true),
        ("hardening/mptg-moods-longer.bin", 88, true),
    ];
    for (name, stride, scored) in files {
        let b = bytes(name);
        let t = mptg::read(&b, mptg::Uses::ALL).expect(name);
        assert_eq!(section_of(&b, mptg::RECS).len(), stride * t.records.len(), "{name}");
        assert_eq!(t.records.iter().any(|e| e.rec.has_moods()), scored, "{name}");
    }
    let meta = Meta { source: mptg::SOURCE_SOFTWARE, read_rules: 1, generation: 2, ..Meta::default() };
    let file = mptg::write(&meta, &[RecordIn { path: "a.mp3".into(), ..RecordIn::default() }], &[]).expect("a file");
    assert_eq!(section_of(&file, mptg::RECS).len(), 72);
}

// ── Round trips and the canonical order ─────────────────────────────────────

/// A made-up library: folders from a small alphabet that holds the order's
/// traps (`A`, `A B`, `A-`, `a`, `É`, NFC and NFD), records with tag values
/// that need 2.3.6 (control characters, repeats, long values), ledger rows,
/// OWNED folders and, in half the software's files, mood tails from server
/// rows with NULL heads, ties and scores outside 0-1 (the minor theirs).
fn made_up_library(rng: &mut fastrand::Rng, records: usize) -> (Meta, Vec<RecordIn>, Vec<FolderIn>) {
    const NAMES: [&str; 9] = ["A", "A B", "A-", "B", "a", "\u{c9}", "Caf\u{e9}", "Cafe\u{301}", "Disc 1"];
    let source = [mptg::SOURCE_DEVICE, mptg::SOURCE_SOFTWARE, mptg::SOURCE_SYNC_AGENT][rng.usize(..3)];
    let scored = source != mptg::SOURCE_DEVICE && rng.bool();
    let mut meta = Meta {
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
            moods: match scored && rng.bool() {
                true => moods::encode(Some(&std::array::from_fn(|_| match rng.u8(..6) {
                    0 => None,
                    1 => Some(0.5),
                    _ => Some(rng.f64() * 1.2 - 0.1),
                }))),
                false => moods::NO_ROW,
            },
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
    meta.minor = mptg::minor_of(&out);
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
        // Rows of 80 bytes and minor 1 with a tail (each record's, 8 zero
        // bytes where it has none), else 72 and minor 0.
        let stride = if meta.minor == 1 { 80 } else { 72 };
        assert_eq!((tags.header.minor, section_of(&file, mptg::RECS).len()), (meta.minor, stride * n));
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
    // A mood tail (2.6.4): taken in a software file of minor 1, beside a
    // record with none; refused where a reader would refuse it, in the
    // device's own file, and under a minor its records don't make.
    let tail = moods::encode(Some(&[Some(0.5); 8]));
    let scored = |path: &str, moods: Tail| RecordIn { rec: Record { moods, ..Record::default() }, ..rec(path) };
    let minor1 = Meta { minor: 1, ..meta.clone() };
    let file = mptg::write(&minor1, &[scored("a.mp3", tail), rec("b.mp3")], &[]).expect("a tail beside none");
    let back = mptg::read(&file, mptg::Uses::ALL).expect("read");
    assert_eq!(back.records.iter().map(|e| e.rec.moods).collect::<Vec<_>>(), [tail, moods::NO_ROW]);
    for e in json("moods.json")["tails"].as_array().expect("tails") {
        let t = tail_of(&e["tail"]);
        if t != moods::NO_ROW {
            let written = mptg::write(&minor1, &[scored("a.mp3", t)], &[]);
            assert_eq!(written.is_ok(), e["valid"] == true, "{}", text(&e["note"]));
        }
    }
    assert!(mptg::write(&meta, &[scored("a.mp3", tail)], &[]).is_err(), "under minor 0");
    assert!(mptg::write(&Meta { minor: 2, ..meta.clone() }, &[scored("a.mp3", tail)], &[]).is_err());
    assert!(mptg::write(&minor1, &[rec("a.mp3")], &[]).is_err(), "minor 1 with no tail");
    let device1 = Meta { minor: 1, ..device.clone() };
    assert!(mptg::write(&device1, &[scored("a.mp3", tail)], &[]).is_err(), "in the device's own file");
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

#[test]
fn a_reader_reserves_no_more_than_its_cap_whatever_a_header_says() {
    assert_eq!(super::reserve(12), 12);
    assert_eq!(super::reserve(u32::MAX), super::RESERVE_MAX);
}
