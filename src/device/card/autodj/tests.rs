//! The Auto DJ index's writer against the player's reference writer
//! (`tools/mpdj.py`): every Auto DJ golden of test/fixtures/card built byte
//! for byte from its description, a wider synthetic library
//! (test/fixtures/autodj) against what the reference made of it at four
//! selections, the keys of AUTODJ-INDEX.md 2.4, the kernel and the
//! selection against the rule written plainly, and 7.3's table.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::*;
use crate::device::card::contract::mptg::{Ledger, Record};
use crate::device::card::contract::prim::{fnv1a64, hex, hex16};
use crate::device::card::contract::text::{RUN_FIELDS, Run};

// ── The fixtures ────────────────────────────────────────────────────────────

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures").join(rel)
}

fn bytes(rel: &str) -> Vec<u8> {
    std::fs::read(fixture(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_slice(&bytes(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// An integer as the fixtures give one: a number, or a string (`0x…` hex).
fn num(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().expect("an integer"),
        Value::String(s) => u64::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex"),
        other => panic!("not an integer: {other}"),
    }
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// What a description gives for checking (13.2): a row's primary path,
/// artist nameKey and song key (null: none).
struct Given {
    hash: [u8; 16],
    primary: String,
    name_key: String,
    song_key: Option<String>,
}

/// A description (13.2) as the writer's input: `dir` holds its vectors
/// file when it names one (rows in the description's order); else each
/// row's numbers, read as the nearest f64 and then the nearest f32 (5.2).
fn described(d: &Value, dir: &str) -> (IndexIn, Vec<Given>) {
    let rows = d["rows"].as_array().expect("rows");
    let dim = num(&d["dim"]) as usize;
    let sidecar: Option<Vec<f32>> = d["vectors"].as_str().map(|name| {
        let raw = bytes(&format!("{dir}/{name}"));
        assert_eq!(raw.len(), rows.len() * dim * 4, "{name}");
        raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    });
    let mut files = Vec::new();
    let mut vectors = HashMap::new();
    let mut given = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let hash = hex16(text(&r["hash"])).expect("a hash");
        let vector: Vec<f32> = match &sidecar {
            Some(all) => all[i * dim..(i + 1) * dim].to_vec(),
            None => r["embedding"].as_array().expect("an embedding").iter().map(|x| x.as_f64().expect("a number") as f32).collect(),
        };
        vectors.insert(hash, vector);
        for f in r["files"].as_array().expect("files") {
            files.push(FileIn {
                path: text(&f["path"]).to_owned(),
                hash,
                title: text(&f["title"]).to_owned(),
                artists: f["artists"].as_array().expect("artists").iter().map(|a| text(a).to_owned()).collect(),
                bpm10: f.get("bpm10").map_or(0, num) as u16,
                camelot: f.get("camelot").map_or(0, num) as u8,
                analysed: f["bpmAnalysed"].as_bool().unwrap_or(false),
            });
        }
        given.push(Given {
            hash,
            primary: text(&r["primaryPath"]).to_owned(),
            name_key: text(&r["artistNameKey"]).to_owned(),
            song_key: r["songKey"].as_str().map(str::to_owned),
        });
    }
    let k = d.get("k").map_or(169, num) as u32;
    let kind = d.get("selKind").map_or(1, num) as u8;
    let selection = if kind == mpdj::SEL_TOP {
        Selection::top(k)
    } else {
        let head = d.get("selHead").map_or(84, num) as u32;
        Selection { k, kind, head, floor: d.get("selFloor").map_or(140, num) as u32 }
    };
    let model = Model {
        id: text(&d["modelId"]).to_owned(),
        version: text(&d["modelVersion"]).to_owned(),
        metric: d["metric"].as_str().unwrap_or("cosine").to_owned(),
        license: text(&d["license"]).to_owned(),
        attribution: text(&d["attribution"]).to_owned(),
    };
    let input = IndexIn {
        generation: num(&d["generation"]) as u32,
        card_id: num(&d["cardId"]),
        built_time: d.get("builtTime").map_or(0, num) as u32,
        tags_generation: d.get("tagsGeneration").map_or(0, num) as u32,
        model,
        selection,
        files,
        vectors,
    };
    (input, given)
}

/// The keys the description gives agree with the writer's, row by row.
fn assert_keys(p: &Prepared, given: &[Given], what: &str) {
    let by_hash: HashMap<[u8; 16], &Given> = given.iter().map(|g| (g.hash, g)).collect();
    assert_eq!(p.rows.len(), given.len(), "{what}");
    for row in &p.rows {
        let g = by_hash[&row.hash];
        let h = hex(&row.hash);
        assert_eq!(row.primary, g.primary, "{what}: {h}'s primary file");
        assert_eq!(row.artist_name_key, g.name_key, "{what}: {h}'s artist nameKey");
        assert_eq!(row.song_key, g.song_key, "{what}: {h}'s song key");
    }
}

fn one_thread() -> Work<'static> {
    Work { threads: 1, ..Work::default() }
}

/// A card whose device.txt reads major 2 of the index.
fn reads_v2() -> DeviceTxt {
    DeviceTxt::read(Some(b"contract=1\nread.mpdj=1,2\n"))
}

// ── The player's goldens (13.4, 13.5) ───────────────────────────────────────

/// Every Auto DJ description of test/fixtures/card is built byte for byte
/// into its golden, at one thread and at three, its keys the ones it
/// gives.
#[test]
fn goldens_are_built_byte_for_byte() {
    let mut built = 0;
    let mut names: Vec<String> = std::fs::read_dir(fixture("card/libraries"))
        .expect("libraries")
        .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("autodj-") && n.ends_with(".json"))
        .collect();
    names.sort();
    for name in &names {
        let d = json(&format!("card/libraries/{name}"));
        assert_eq!(text(&d["format"]), "MPDJ", "{name}");
        let (input, given) = described(&d, "card/libraries");
        let file = text(&d["file"]);
        let want = bytes(&format!("card/golden/{file}"));
        let p = prepare(&input).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_keys(&p, &given, file);
        for threads in [1, 3] {
            let got = p.build(&Work { threads, ..Work::default() }).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(got == want, "{file} at {threads} threads: the bytes differ from the golden's");
        }
        // The whole step makes the same file for a card with no index yet.
        let step = index(&reads_v2(), &input, None, &one_thread()).expect("a step");
        assert_eq!(step, Index::New { file: want, lists_kept: false }, "{file}");
        built += 1;
    }
    assert_eq!(built, 7);
}

/// 13.4's hand-checkable file, read back as its tables give it: the lists,
/// the slot CRCs, the header, the signature's text (vectors.json's).
#[test]
fn the_hand_checkable_file() {
    let (input, _) = described(&json("card/libraries/autodj-00000029.json"), "card/libraries");
    let p = prepare(&input).expect("prepared");
    let file = p.build(&one_thread()).expect("built");
    let t = Table::open(&file).expect("opened");
    let lists: [&[(u16, u8)]; 6] = [
        &[(2, 200), (5, 185), (4, 162), (3, 100)],
        &[(0, 100), (2, 51), (4, 40), (3, 0)],
        &[(0, 200), (5, 171), (4, 150), (3, 128)],
        &[(2, 128), (0, 100), (1, 0)],
        &[(0, 162), (2, 150), (1, 40)],
        &[(0, 185), (2, 171), (1, 0)],
    ];
    let crcs = [0xE487_C86F, 0x993E_690F, 0x8273_6E17, 0x7907_48C6, 0x9D3D_DF4D, 0x8ABB_6A85];
    for (r, (list, crc)) in lists.iter().zip(crcs).enumerate() {
        let entries: Vec<(u16, u8)> = t.slot(r as u32).expect("a slot").iter().map(|e| (e.row, e.level)).collect();
        assert_eq!(entries, *list, "row {r}");
        let raw = t.slot_raw(r as u32).expect("a slot");
        assert_eq!(u32::from_le_bytes(raw[252..].try_into().expect("4 bytes")), crc, "row {r}");
    }
    assert_eq!((t.header().header_bytes, t.header().header_crc, file.len()), (128, 0x02EC_B029, 1973));
    assert_eq!(hex(&t.sig), "af4752829aa50afa2b5f2e6aecc56d59");
    assert_eq!(p.groups, [vec![3, 4, 5]]);
    let v = json("card/vectors.json");
    assert_eq!(p.sig_text, text(&v["mpdj"]["signature"]["text"]));
    assert!(p.sig_text.ends_with("fedcba9876543210fedcba9876543210\nsame 3 4 5\n"));
}

// ── The cross-check (test/fixtures/autodj) ──────────────────────────────────

/// A library of 517 rows built at four selections as the reference built
/// it: the length, the SHA-256, the header CRC, the signature; on a
/// difference, the first row whose slot isn't the reference's.
#[test]
fn a_wider_library_is_the_references() {
    let d = json("autodj/wide.json");
    let (mut input, given) = described(&d, "autodj");
    let expected = json("autodj/expected.json");
    let raw = bytes("autodj/wide.f32");
    assert_eq!(hex(&Sha256::digest(&raw)), text(&expected["vectorsSha256"]));
    let builds = expected["builds"].as_array().expect("builds");
    assert_eq!(builds.len(), 4);
    for (i, b) in builds.iter().enumerate() {
        let k = num(&b["k"]) as u32;
        input.selection = match num(&b["selKind"]) as u8 {
            mpdj::SEL_TOP => Selection::top(k),
            kind => Selection { k, kind, head: num(&b["selHead"]) as u32, floor: num(&b["selFloor"]) as u32 },
        };
        let p = prepare(&input).expect("prepared");
        if i == 0 {
            assert_keys(&p, &given, "wide");
            assert_eq!(p.groups.len(), 4);
        }
        let file = p.build(&Work { threads: 1 + i, ..Work::default() }).expect("built");
        let what = format!("wide at k {k}, selKind {}", text(&b["note"]));
        let t = Table::open(&file).expect("opened");
        let crcs: Vec<&str> = text(&b["slotCrcs"]).split(' ').collect();
        assert_eq!(crcs.len(), t.row_count as usize, "{what}");
        for (r, want) in crcs.iter().enumerate() {
            let raw = t.slot_raw(r as u32).expect("a slot");
            let got = format!("{:08x}", u32::from_le_bytes(raw[raw.len() - 4..].try_into().expect("4 bytes")));
            assert_eq!(got, *want, "{what}: row {r}'s slot isn't the reference's");
        }
        assert_eq!(file.len() as u64, num(&b["bytes"]), "{what}");
        assert_eq!(hex(&t.sig), text(&b["selectionSig"]), "{what}");
        assert_eq!(t.header().header_crc as u64, num(&b["headerCrc"]), "{what}");
        assert_eq!(hex(&Sha256::digest(&file)), text(&b["sha256"]), "{what}");
    }
}

// ── The keys (2.4) ──────────────────────────────────────────────────────────

#[test]
fn keys_as_the_index_gives_them() {
    assert_eq!(name_key("  The Band\u{2019}s  \u{201C}Best\u{201D} \u{2013} Live "), "the band's \"best\" - live");
    assert_eq!(name_key("ΣΟΦΙΑΣ"), "σοφιας");
    assert_eq!(name_key("\u{2018}\u{2019}\u{201A}\u{201B}\u{2032}"), "'''''");
    assert_eq!(name_key("\u{201C}\u{201D}\u{201E}\u{201F}\u{2033}"), "\"\"\"\"\"");
    assert_eq!(name_key("\u{2010}\u{2011}\u{2012}\u{2013}\u{2014}\u{2015}\u{2212}"), "-------");
    // Every White_Space run is one space; no accent folding, no
    // normalisation.
    assert_eq!(name_key("\tA\u{85}\u{3000}\u{2028}b\u{a0}\u{202F}"), "a b");
    assert_eq!(name_key("Caf\u{e9}"), "caf\u{e9}");
    assert_ne!(name_key("Caf\u{e9}"), name_key("Cafe\u{301}"));
    // U+0130's full lowercase is two code points, and a closing capital
    // sigma is final: autodj-tiny's two titles are one song.
    assert_eq!(name_key("SAMPLE T\u{130}TLE 31 τιτλος"), name_key("Sample Ti\u{307}tle 31 ΤΙΤΛΟΣ"));
    assert_eq!(name_key("T\u{130}"), "ti\u{307}");

    let three = ["Generated Artist 04", "Generated Artist 05", "generated  artist 04"];
    assert_eq!(artist_display(&three), "Generated Artist 04, Generated Artist 05");
    assert_eq!(artist_display(&["A\u{a0}", "B"]), "A, B");
    assert_eq!(artist_display(&["  ", " Solo\u{a0}"]), "Solo");
    assert_eq!(artist_display::<&str>(&[]), "");
    assert_eq!(song_key(&["Generated Artist 01"], "Sample Title 01").as_deref(), Some("generated artist 01|sample title 01"));
    assert_eq!(song_key::<&str>(&[], "Sample Café").as_deref(), Some("|sample café"));
    assert_eq!(song_key(&["A\u{a0}", "B"], "T").as_deref(), Some("a, b|t"));
    assert_eq!(song_key(&["Generated Artist 01"], "   "), None);
    assert_eq!(song_key(&["Generated Artist 01"], ""), None);

    assert_eq!(fnv1a64(b"generated artist 01"), 0xF8EE_BFC1_B460_A770);
    assert_eq!(artist_key(&["Generated Artist 01"]), 0xB460_A770);
    assert_eq!(artist_key(&["   ", "Generated Artist 03"]), 0xB460_AAD6);
    assert_eq!(artist_key(&["Generated Artist 03"]), 0xB460_AAD6);
    assert_eq!(artist_key::<&str>(&[]), 0);
    assert_eq!(artist_name_key(&["\u{a0}", "GENERATED  ARTIST 03 "]), "generated artist 03");
}

// ── The rule, written plainly ───────────────────────────────────────────────

/// Row a's candidates in order (5.3): every other row but those whose song
/// key is a's, by level descending, then row ascending.
fn candidates(emb: &[&[f32]], keys: &[Option<String>], a: usize) -> Vec<(u16, u8)> {
    let mut c: Vec<(u16, u8)> = (0..emb.len())
        .filter(|&b| b != a && !(keys[a].is_some() && keys[b] == keys[a]))
        .map(|b| (b as u16, mpdj::level_of(emb[a], emb[b])))
        .collect();
    c.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    c
}

/// 5.4's selection from a row's ordered candidates.
fn selected(c: &[(u16, u8)], sel: &Selection) -> Vec<(u16, u8)> {
    if sel.kind == mpdj::SEL_STRIDED {
        let m = c.iter().take_while(|e| e.1 as u32 >= sel.floor).count() as u32;
        if m > sel.k {
            let (k, h) = (sel.k, sel.head);
            return (0..k).map(|j| c[(if j < h { j } else { h + (j - h) * (m - 1 - h) / (k - 1 - h) }) as usize]).collect();
        }
    }
    c[..c.len().min(sel.k as usize)].to_vec()
}

/// A made-up library: `n` rows of `dim` components that are multiples of
/// 1/8 (so cosines tie often, exactly), drawn from a few vectors, with song
/// keys from a small pool (some rows without one) and a second path for
/// some rows.
fn made_up(rng: &mut fastrand::Rng, n: usize, dim: usize, selection: Selection) -> IndexIn {
    let pool: Vec<Vec<f32>> =
        (0..(n / 3).max(2)).map(|_| (0..dim).map(|_| rng.i32(-8..=8) as f32 / 8.0).collect()).collect();
    let mut files = Vec::new();
    let mut vectors = HashMap::new();
    for g in 0..n {
        let mut hash = [0u8; 16];
        rng.fill(&mut hash);
        let mut v = pool[rng.usize(..pool.len())].clone();
        if rng.bool() {
            v[rng.usize(..dim)] += 0.125;
        }
        vectors.insert(hash, v);
        let song = rng.usize(..n / 2 + 2);
        let file = FileIn {
            path: format!("Generated Artist {:02}/{g:03} - Sample Title.mp3", song % 7),
            hash,
            title: if song % 5 == 0 { String::new() } else { format!("Sample Title {song}") },
            artists: vec![format!("Generated Artist {:02}", song % 7)],
            bpm10: rng.u16(..2000),
            camelot: rng.u8(..30),
            analysed: rng.bool(),
        };
        if rng.u8(..8) == 0 {
            files.push(FileIn { path: format!("Copies/{g:03}.flac"), ..file.clone() });
        }
        files.push(file);
    }
    let model = Model {
        id: "synthetic-embedding".into(),
        version: "1".into(),
        metric: "cosine".into(),
        license: "CC0-1.0".into(),
        attribution: "synthetic test vectors".into(),
    };
    IndexIn { generation: 3, card_id: 7, built_time: 1, tags_generation: 3, model, selection, files, vectors }
}

/// The kernel's sixteen sums are each the rule's sum to the bit, at every
/// row of every panel, the last one's short.
#[test]
fn the_kernel_is_the_rule() {
    let mut rng = fastrand::Rng::with_seed(0x00C0_FFEE);
    for (n, dim) in [(1, 1), (5, 3), (9, 17), (13, 40), (7, 1280)] {
        let rows: Vec<Vec<f32>> = (0..n)
            .map(|_| (0..dim).map(|_| (rng.f32() - 0.5) * 2f32.powi(rng.i32(-30..=30))).collect())
            .collect();
        let refs: Vec<&[f32]> = rows.iter().map(Vec::as_slice).collect();
        let panels = Panels::new(&refs, dim);
        for p in 0..panels.count() {
            let a: Vec<f64> = panels.panel(p).iter().map(|&x| x as f64).collect();
            for q in 0..panels.count() {
                let sums = dots(&a, panels.panel(q), dim);
                for (x, y) in (0..W * W).map(|i| (i / W, i % W)) {
                    let (ra, rb) = (p * W + x, q * W + y);
                    if ra < n && rb < n {
                        let want = mpdj::cosine(&rows[ra], &rows[rb]);
                        assert_eq!(sums[x][y].to_bits(), want.to_bits(), "{n} x {dim}: {ra}, {rb}");
                    }
                }
            }
        }
    }
}

/// Made-up libraries with ties, songs and second paths: every row's list
/// is the plain rule's, at every selection kind, on any number of threads,
/// and the file reads as a whole.
#[test]
fn the_lists_are_the_rule() {
    let selections = [
        Selection::top(2),
        Selection::top(5),
        Selection::top(84),
        Selection { k: 4, kind: mpdj::SEL_STRIDED, head: 1, floor: 100 },
        Selection { k: 10, kind: mpdj::SEL_STRIDED, head: 3, floor: 1 },
        Selection { k: 3, kind: mpdj::SEL_STRIDED, head: 1, floor: 255 },
        Selection { k: 24, kind: mpdj::SEL_STRIDED, head: 8, floor: 120 },
        Selection::DEFAULT,
    ];
    let mut rng = fastrand::Rng::with_seed(20261018);
    for round in 0..24 {
        let sel = selections[round % selections.len()];
        let (n, dim) = (rng.usize(1..=150), rng.usize(1..=6));
        let input = made_up(&mut rng, n, dim, sel);
        let p = prepare(&input).expect("prepared");
        let file = p.build(&Work { threads: rng.usize(1..=5), ..Work::default() }).expect("built");
        check(&file).expect("a whole file");
        let t = Table::open(&file).expect("opened");
        let emb: Vec<&[f32]> = p.rows.iter().map(|r| input.vectors[&r.hash].as_slice()).collect();
        let keys: Vec<Option<String>> = p.rows.iter().map(|r| r.song_key.clone()).collect();
        for a in 0..p.rows.len() {
            let want = selected(&candidates(&emb, &keys, a), &sel);
            let got: Vec<(u16, u8)> = t.slot(a as u32).expect("a slot").iter().map(|e| (e.row, e.level)).collect();
            assert_eq!(got, want, "round {round}, {n} rows, {sel:?}: row {a}");
        }
    }
}

/// DJPH: every card file of a row once by path hash, sorted; a hash two
/// files share, in one row or two, is left out, and the row stays.
#[test]
fn shared_path_hashes_are_left_out() {
    assert_eq!(path_map(vec![(9, 2), (5, 0), (3, 1), (5, 2), (7, 1), (3, 1)]), [(7, 1), (9, 2)]);
    assert!(path_map(vec![(1, 0), (1, 0)]).is_empty());
    assert!(path_map(Vec::new()).is_empty());
    // In a file: autodj-00000029's row 1 has two paths, both listed.
    let (input, _) = described(&json("card/libraries/autodj-00000029.json"), "card/libraries");
    let t_file = prepare(&input).expect("prepared").build(&one_thread()).expect("built");
    let t = Table::open(&t_file).expect("opened");
    assert_eq!((t.row_count, t.path_count), (6, 7));
    assert_eq!(t.paths().iter().filter(|(_, r)| *r == 1).count(), 2);
}

// ── 7.3: when a run writes a new file ───────────────────────────────────────

#[test]
fn a_run_writes_an_index_only_when_it_must() {
    let golden = bytes("card/golden/autodj-00000029.bin");
    let old = Some(golden.as_slice());
    let (input, _) = described(&json("card/libraries/autodj-00000029.json"), "card/libraries");
    let work = one_thread();
    let run = |device: &DeviceTxt, input: &IndexIn, old: Option<&[u8]>| {
        index(device, input, old, &work).expect("a step")
    };

    // 7.4: only a card that reads major 2 gets one.
    let older: [Option<&[u8]>; 3] =
        [None, Some(b"contract=1\nfirmware=0.10.0\nread.mpdj=1\n"), Some(b"contract=1\nread.mpdj=\n")];
    for device in older {
        assert_eq!(run(&DeviceTxt::read(device), &input, None), Index::None(NoIndex::NotRead));
    }
    let only_v2 = DeviceTxt::read(Some(b"contract=1\nread.mpdj=2\n"));
    assert_eq!(run(&only_v2, &input, None), Index::New { file: golden.clone(), lists_kept: false });

    // Nothing changes, or only the commit's own header values: named again.
    let device = reads_v2();
    assert_eq!(run(&device, &input, old), Index::Same);
    let later = IndexIn { generation: 42, tags_generation: 42, built_time: 1_800_000_000, ..input.clone() };
    assert_eq!(run(&device, &later, old), Index::Same);

    // A retag, a moved file, another card: the old slots carried into a new
    // file, with their builtTime; the same file a full build makes then.
    let mut retag = later.clone();
    retag.files.iter_mut().find(|f| f.path.starts_with("Generated Artist 01/")).expect("row 0's file").bpm10 = 1250;
    let mut moved = later.clone();
    moved.files.iter_mut().find(|f| f.path.contains("SAMPLE TITLE 03")).expect("row 4's file").path =
        "Generated Artist 03/Moved/07 - SAMPLE TITLE 03.flac".into();
    let copied = IndexIn { card_id: 0x0123_4567_89AB_CDEF, ..later.clone() };
    for changed in [&retag, &moved, &copied] {
        let Index::New { file, lists_kept: true } = run(&device, changed, old) else { panic!("carried") };
        let fresh = IndexIn { built_time: 1_790_000_000, ..changed.clone() };
        assert_eq!(file, prepare(&fresh).expect("prepared").build(&work).expect("built"));
        let t = Table::open(&file).expect("opened");
        assert_eq!((t.header().generation, t.tags_generation, t.built_time), (42, 42, 1_790_000_000));
        assert_ne!(file, golden);
    }

    // A new vector, another selection, an old file that reads as absent:
    // the lists computed again.
    let mut revector = later.clone();
    revector.vectors.values_mut().next().expect("a vector")[0] += 0.0625;
    let reselect = IndexIn { selection: Selection::top(4), ..later.clone() };
    let mut broken = golden.clone();
    broken[300] ^= 1;
    for (changed, old) in [(&revector, &golden), (&reselect, &golden), (&later, &broken)] {
        let Index::New { file, lists_kept: false } = run(&device, changed, Some(old)) else {
            panic!("computed")
        };
        assert_eq!(file, prepare(changed).expect("prepared").build(&work).expect("built"));
    }

    // No row, or more than u16 indexes hold: no index.
    let unembedded = IndexIn { vectors: HashMap::new(), ..input.clone() };
    assert_eq!(run(&device, &unembedded, None), Index::None(NoIndex::NoRows));
    let mut wide = IndexIn { files: Vec::new(), vectors: HashMap::new(), ..input.clone() };
    for i in 0..65_536u32 {
        let mut hash = [0u8; 16];
        hash[..4].copy_from_slice(&i.to_le_bytes());
        wide.files.push(FileIn { path: format!("F/{i}.mp3"), hash, ..FileIn::default() });
        wide.vectors.insert(hash, vec![1.0]);
    }
    assert_eq!(run(&device, &wide, None), Index::None(NoIndex::TooManyRows(65_536)));
}

// ── The writer's refusals, and its work ─────────────────────────────────────

#[test]
fn the_writer_refuses_what_no_reader_would_take() {
    let (input, _) = described(&json("card/libraries/autodj-00000029.json"), "card/libraries");
    assert!(prepare(&input).is_ok());
    let refused = |i: &IndexIn| prepare(i).err().map(|e| e.0);
    for selection in [
        Selection::top(1),
        Selection::top(170),
        Selection { k: 4, kind: mpdj::SEL_TOP, head: 3, floor: 0 },
        Selection { k: 4, kind: mpdj::SEL_TOP, head: 4, floor: 140 },
        Selection { k: 4, kind: mpdj::SEL_STRIDED, head: 0, floor: 100 },
        Selection { k: 4, kind: mpdj::SEL_STRIDED, head: 3, floor: 100 },
        Selection { k: 4, kind: mpdj::SEL_STRIDED, head: 1, floor: 0 },
        Selection { k: 4, kind: mpdj::SEL_STRIDED, head: 1, floor: 256 },
        Selection { k: 2, kind: mpdj::SEL_STRIDED, head: 1, floor: 100 },
        Selection { k: 4, kind: 7, head: 2, floor: 9 },
    ] {
        assert!(refused(&IndexIn { selection, ..input.clone() }).is_some(), "{selection:?}");
    }
    assert!(prepare(&IndexIn { selection: Selection::top(2), ..input.clone() }).is_ok());
    let long = Model { attribution: "x".repeat(256), ..input.model.clone() };
    assert!(refused(&IndexIn { model: long, ..input.clone() }).is_some());
    assert!(prepare(&IndexIn { model: Model { attribution: "x".repeat(255), ..input.model.clone() }, ..input.clone() }).is_ok());
    let nul = Model { license: "CC0\0".into(), ..input.model.clone() };
    assert!(refused(&IndexIn { model: nul, ..input.clone() }).is_some());
    for path in ["", "/Generated Artist 01/x.mp3", "A//x.mp3", "A/./x.mp3", "A/x.mp3/"] {
        let mut bad = input.clone();
        bad.files[0].path = path.into();
        assert!(refused(&bad).is_some(), "{path:?}");
    }
    let mut twice = input.clone();
    twice.files.push(twice.files[0].clone());
    assert!(refused(&twice).is_some());
    for value in [f32::NAN, f32::INFINITY] {
        let mut bad = input.clone();
        bad.vectors.values_mut().next().expect("a vector")[3] = value;
        assert!(refused(&bad).is_some(), "{value}");
    }
    let mut mixed = input.clone();
    mixed.vectors.values_mut().next().expect("a vector").push(0.0);
    assert!(refused(&mixed).is_some());
    // A vector no file has is ignored, whatever it holds.
    let mut extra = input.clone();
    extra.vectors.insert([0xEE; 16], vec![f32::NAN]);
    assert_eq!(prepare(&extra).expect("prepared").sig, prepare(&input).expect("prepared").sig);
    assert!(refused(&IndexIn { vectors: HashMap::new(), ..input.clone() }).is_some());
}

#[test]
fn a_build_stops_when_asked_and_counts_its_rows() {
    let (input, _) = described(&json("card/libraries/autodj-mid.json"), "card/libraries");
    let p = prepare(&input).expect("prepared");
    let stop = AtomicBool::new(true);
    assert_eq!(p.lists(&Work { threads: 2, stop: Some(&stop), done: None }), Err(BuildError::Stopped));
    let done = AtomicUsize::new(0);
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    let file = p.build(&Work { threads: 0, stop: Some(&stop), done: Some(&done) }).expect("built");
    assert_eq!(done.into_inner(), 200);
    assert!(file == bytes("card/golden/autodj-mid.bin"));
}

// ── The model's strings, and a tags record's file ───────────────────────────

#[test]
fn the_default_models_strings() {
    let m = Model::known("effnet-discogs", "1").expect("the default model");
    assert_eq!((m.metric.as_str(), m.license.as_str(), m.attribution.len()), ("cosine", "CC-BY-NC-SA-4.0", 92));
    assert!(m.attribution.starts_with("Discogs-EffNet by Music") && m.attribution.ends_with("Fabra (essentia.upf.edu/models)"));
    assert_eq!(Model::known("effnet-discogs", "2"), None);
    assert_eq!(Model::known("another-model", "1"), None);
    // With them STRS is 134 bytes (4.1), and the file's size is 10.2's.
    let (mut input, _) = described(&json("card/libraries/autodj-small-default.json"), "card/libraries");
    input.model = m;
    let p = prepare(&input).expect("prepared");
    assert_eq!(p.strs.len(), 134);
    let file = p.build(&one_thread()).expect("built");
    let (rows, paths) = (6, 7);
    assert_eq!(file.len(), 512 + rows * 512 + 8 * rows + 10 * paths + (8 - 2 * paths % 8) % 8 + 134);
    let strings = Table::open(&file).expect("opened").strings();
    assert_eq!(strings[0].as_deref(), Some("effnet-discogs"));
}

#[test]
fn a_tags_records_file() {
    let mut raw: [Vec<&str>; RUN_FIELDS] = Default::default();
    raw[TITLE] = vec![" Sample Title 01"];
    raw[ARTIST] = vec!["Generated Artist 01", "Generated Artist 02"];
    let (run, _) = Run::build(&raw);
    let rec = Record { bpm10: 1210, camelot: 27, flags: BPM_ANALYSED, ..Record::default() };
    let ledger = Ledger { file_hash: [0xAB; 16], ..Ledger::default() };
    let f = FileIn::of("Generated Artist 01/01.mp3", &rec, &run, Some(&ledger)).expect("a file with a row");
    assert_eq!(f.hash, [0xAB; 16]);
    assert_eq!((f.title.as_str(), f.artists.len(), f.bpm10, f.camelot, f.analysed), (" Sample Title 01", 2, 1210, 27, true));
    let audio = Ledger { audio_hash: [0xCD; 16], ..ledger.clone() };
    assert_eq!(FileIn::of("x.mp3", &rec, &run, Some(&audio)).expect("a row").hash, [0xCD; 16]);
    assert_eq!(FileIn::of("x.mp3", &rec, &run, Some(&Ledger::default())), None);
    assert_eq!(FileIn::of("x.mp3", &rec, &run, None), None);
    let untagged = FileIn::of("x.mp3", &Record::default(), &Run::default(), Some(&ledger)).expect("a row");
    assert_eq!((untagged.title.as_str(), untagged.artists.len()), ("", 0));
}
