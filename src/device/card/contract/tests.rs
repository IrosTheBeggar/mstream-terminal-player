//! The kit against the contract's shared fixtures, test/fixtures/card (made
//! by the firmware repo's tools/card_fixtures.py and copied from there; the
//! folder's README names the commit): so far the vectors of 2.18 that its
//! primitives bind.

use std::path::PathBuf;

use serde_json::Value;

use super::fat::{fat_from_wall, fat_time, fat_valid, fat_wall, skew, time_matches};
use super::numbers;
use super::order::{album_folder, compare_file_paths, compare_names};
use super::prim::{self, crc32, crc32_after, fnv1a64, hex, md5, path_hash, qfp_of};
use super::text::{FIELD_NAMES, RUN_FIELDS, Run, build_field, is_list};

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

fn spaced_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

fn raw_tags(tags: &Value) -> [Vec<String>; RUN_FIELDS] {
    let mut raw: [Vec<String>; RUN_FIELDS] = Default::default();
    for (f, name) in FIELD_NAMES.iter().enumerate() {
        if let Some(values) = tags.get(*name).and_then(Value::as_array) {
            raw[f] = values.iter().map(|v| text(v).to_owned()).collect();
        }
    }
    raw
}

// ── The vectors (2.18) ──────────────────────────────────────────────────────

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
