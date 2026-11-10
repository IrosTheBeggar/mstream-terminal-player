//! The waveform index's writer: which card files have a row, what an old slot
//! still holds for, what a run asks (WAVEFORM-INDEX.md 7.3), and 7.3's table
//! against vectors.json's `mpwf.commit`, the whole way from the tracks to the
//! file. Every hash and shape here is made up.

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::device::card::contract::mptg::{self, Record};
use crate::device::card::contract::mpwf::bars_from;
use crate::device::card::contract::prim::fnv1a64;
use crate::device::card::contract::text::Run;

// ── The fixtures ────────────────────────────────────────────────────────────

fn fixture(rel: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/card").join(rel);
    std::fs::read(path).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn golden(name: &str) -> Vec<u8> {
    fixture(&format!("golden/{name}"))
}

fn json(rel: &str) -> Value {
    serde_json::from_slice(&fixture(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// An integer as the fixtures give one: a number, or a string (`0x…` hex).
fn num(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().expect("an integer"),
        Value::String(s) => u64::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex"),
        other => panic!("not an integer: {other}"),
    }
}

/// A description's track's, or a vector's added row's, peaks.
fn peaks_of(t: &Value) -> Vec<u8> {
    t["peaks"].as_array().expect("peaks").iter().map(|p| num(p) as u8).collect()
}

const CARD: u64 = 0x5EED_C0DE_0A1B_2C3D;

// ── The scene ───────────────────────────────────────────────────────────────

/// Recording n's canonical hash, and its shape.
fn recording(n: u8) -> [u8; 16] {
    [n; 16]
}

fn shape(n: u8) -> [u8; BAR_COUNT] {
    [n; BAR_COUNT]
}

fn shapes(ns: &[u8]) -> Shapes {
    ns.iter().map(|&n| (recording(n), shape(n))).collect()
}

/// A ledger row of recording n, on the server's own.
fn row_of(n: u8) -> Ledger {
    Ledger { audio_hash: recording(n), server_path: format!("Lib/{n:03}.mp3"), ..Ledger::default() }
}

fn track(path_hash: u64, length_ms: u32, ledger: &Ledger) -> Track<'_> {
    Track { path_hash, length_ms, ledger: Some(ledger) }
}

fn slot(path_hash: u64, length_ms: u32, n: u8) -> Slot {
    Slot { path_hash, length_ms, bars: shape(n) }
}

/// A file of these slots (path hash, length, recording).
fn file_of(slots: &[(u64, u32, u8)]) -> Vec<u8> {
    let rows = slots.iter().map(|&(h, ms, n)| slot(h, ms, n)).collect();
    mpwf::write(&TableIn { generation: 7, card_id: CARD, minor: 0, rows }).expect("a file")
}

/// That file as a run reuses it: `ledger` the root it began from's records,
/// each path hash and recording.
fn old_of<'a>(file: &'a [u8], ledger: &[(u64, u8)], checkpoint: bool) -> Old<'a> {
    let ledger = ledger.iter().map(|&(path, n)| (path, Some(recording(n))));
    Old::joined(Table::open(file).expect("the file"), ledger, checkpoint)
}

fn reads_mpwf(on: bool) -> DeviceTxt {
    DeviceTxt { read_mpwf: on as u32, ..DeviceTxt::defaults() }
}

// ── The rows ────────────────────────────────────────────────────────────────

#[test]
fn a_row_is_a_card_file_whose_bars_the_writer_has() {
    let (one, two) = (row_of(1), row_of(2));
    let by_file = Ledger { file_hash: recording(3), ..Ledger::default() };
    let no_hash = Ledger::default();
    let tracks = [
        track(30, 1_000, &one),
        // No bars of its recording: no row, and a plain line.
        track(20, 2_000, &two),
        // `hash`, with no `audio-hash`: still a recording.
        track(50, 3_000, &by_file),
        // No hash at all, and no ledger row at all: nothing keys it.
        track(10, 4_000, &no_hash),
        Track { path_hash: 40, length_ms: 5_000, ledger: None },
        // A second copy of a recording has a row of its own, under its own
        // path and its own length.
        track(60, 6_000, &one),
    ];
    let rows = rows_of(&tracks, None, &shapes(&[1, 3]));
    assert_eq!(rows, [slot(30, 1_000, 1), slot(50, 3_000, 3), slot(60, 6_000, 1)]);
    assert!(rows_of(&tracks, None, &Shapes::new()).is_empty());

    // A silent track has a row, all zero (4.3).
    let silent = Shapes::from([(recording(2), bars_from(&[0]))]);
    assert_eq!(rows_of(&tracks, None, &silent), [slot(20, 2_000, 0)]);
}

#[test]
fn a_record_is_its_rows_path_hash_and_its_own_length() {
    let rec = Record { duration_ms: 181_000, ..Record::default() };
    let path = "Artist/Album/02 - Other.mp3".to_string();
    let ledger = Some(row_of(1));
    let records = [RecordIn { path: path.clone(), rec, run: Run::default(), ledger }];
    assert_eq!(rows(&records, None, &shapes(&[1])), [slot(path_hash(&path), 181_000, 1)]);
    assert_eq!(path_hash(&path), 0xCADB_3EFD_EF4B_2DD9, "row 4 of waves-0000002a.bin");
}

/// A hash two card files share has no row, whichever of them have peaks (4.3),
/// a record with none of its own to give among them.
#[test]
fn a_path_hash_two_card_files_share_has_no_row() {
    let (one, two) = (row_of(1), row_of(2));
    let both = [track(9, 1_000, &one), track(9, 2_000, &two), track(8, 3_000, &one)];
    assert_eq!(rows_of(&both, None, &shapes(&[1, 2])), [slot(8, 3_000, 1)]);
    let only_one_has_bars = [track(9, 1_000, &one), track(9, 2_000, &two)];
    assert!(rows_of(&only_one_has_bars, None, &shapes(&[1])).is_empty());
    let no_ledger = [track(9, 1_000, &one), Track { path_hash: 9, length_ms: 0, ledger: None }];
    assert!(rows_of(&no_ledger, None, &shapes(&[1])).is_empty());

    // So it isn't asked for either, unless a file of the recording may have a
    // row.
    let asked = |tracks: &[Track<'_>]| -> Vec<[u8; 16]> {
        asks_of(tracks, None, |_| true).into_iter().map(|a| a.hash).collect()
    };
    assert!(asked(&only_one_has_bars).is_empty());
    assert_eq!(asked(&both), [recording(1)]);
}

// ── The old file ────────────────────────────────────────────────────────────

#[test]
fn an_old_slot_holds_through_the_ledger_of_the_root_the_run_began_from() {
    let file = file_of(&[(10, 1_000, 1), (20, 2_000, 2), (30, 3_000, 3), (40, 4_000, 4)]);
    // Recording 4's path isn't in that ledger (the listener's now, say).
    let old = old_of(&file, &[(10, 1), (20, 2), (30, 3)], false);
    let (one, two, three, four, five) = (row_of(1), row_of(2), row_of(3), row_of(4), row_of(5));
    let tracks = [
        // Where it was: its bars, its length now from its own record.
        track(10, 1_111, &one),
        // Moved: its bars under its new path hash; a final root's ledger is
        // the file's, so its length doesn't matter.
        track(25, 2_222, &two),
        // Another recording at recording 3's old path: not its bars.
        track(30, 3_000, &five),
        // Recording 3 itself, a second place.
        track(35, 3_000, &three),
        // Its slot is there, but no ledger row says what it was.
        track(40, 4_000, &four),
    ];
    let rows = rows_of(&tracks, Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(10, 1_111, 1), slot(25, 2_222, 2), slot(35, 3_000, 3)]);

    // Peaks fetched in this run win; a new row sits between the old.
    let rows = rows_of(&tracks, Some(&old), &shapes(&[1, 5]));
    assert_eq!(rows[0].bars, shape(1));
    assert_eq!(rows[2], slot(30, 3_000, 5));

    // A path hash two of that ledger's records share says nothing of its path.
    let old = old_of(&file, &[(10, 1), (10, 2), (20, 2)], false);
    let rows = rows_of(&[track(10, 0, &one), track(20, 0, &two)], Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(20, 0, 2)]);

    // No ledger at all: no slot holds.
    let table = Table::open(&file).expect("the file");
    let old = Old::new(table, None, false);
    assert!(rows_of(&tracks, Some(&old), &Shapes::new()).is_empty());
}

#[test]
fn a_damaged_slot_costs_only_itself() {
    let mut file = file_of(&[(10, 1_000, 1), (20, 2_000, 2), (30, 3_000, 3)]);
    file[256 + 256 + 100] ^= 1;
    let ledger = [(10, 1), (20, 2), (30, 3)];
    let old = old_of(&file, &ledger, false);
    let (one, two, three) = (row_of(1), row_of(2), row_of(3));
    let tracks = [track(10, 1_000, &one), track(20, 2_000, &two), track(30, 3_000, &three)];
    let rows = rows_of(&tracks, Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(10, 1_000, 1), slot(30, 3_000, 3)]);
    // The damaged one is asked for again, and the file isn't the rows' bytes:
    // a new one, with or without it, not the old one again.
    let asked = asks_of(&tracks, Some(&old), |_| true);
    assert_eq!(asked.iter().map(|a| a.hash).collect::<Vec<_>>(), [recording(2)]);
    let got = answer(&reads_mpwf(true), 8, CARD, Some(&old), rows).expect("a file");
    assert!(matches!(got, Index::New(_)), "{got:?}");
}

/// A sector the card can't read is left zero in the bytes a run works from: that
/// must cost its slots at any row a file can have, as a damaged one does.
#[test]
fn a_slot_of_zeros_fails_at_every_row() {
    let zeros = [0; mpwf::SLOT_BYTES as usize];
    for r in 0..mpwf::MAX_ROWS {
        assert_ne!(mpwf::slot_crc(&zeros, r), 0, "row {r}");
    }
    let mut file = file_of(&[(10, 1_000, 1), (20, 2_000, 2), (30, 3_000, 3)]);
    file[256 + 256..256 + 2 * 256].fill(0);
    let old = old_of(&file, &[(0, 2), (10, 1), (20, 2), (30, 3)], false);
    let (one, two, three) = (row_of(1), row_of(2), row_of(3));
    let tracks = [track(10, 1_000, &one), track(20, 2_000, &two), track(30, 3_000, &three)];
    let rows = rows_of(&tracks, Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(10, 1_000, 1), slot(30, 3_000, 3)]);
}

/// 7.3: a checkpoint's ledger already shows a recording replaced before it,
/// while the file it names again still holds the old bars.
#[test]
fn under_a_checkpoint_a_slot_needs_its_records_length_too() {
    let file = file_of(&[(10, 1_000, 1), (20, 0, 2), (30, 3_000, 3)]);
    // Recording 7 took path 10 before the checkpoint.
    let old = old_of(&file, &[(10, 7), (20, 2), (30, 3)], true);
    let (seven, two, three) = (row_of(7), row_of(2), row_of(3));
    let tracks = [track(10, 7_000, &seven), track(20, 0, &two), track(30, 3_001, &three)];
    let rows = rows_of(&tracks, Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(20, 0, 2)], "a length of 0 and 0 agree; 7,000 and 3,001 don't");
    let asked = asks_of(&tracks, Some(&old), |_| true);
    assert_eq!(asked.iter().map(|a| a.hash).collect::<Vec<_>>(), [recording(7), recording(3)]);

    // A recording of the same length gets past this, and costs one wrong shape
    // until its path next changes: the old bars, under its hash.
    let same = [track(10, 1_000, &seven)];
    assert_eq!(rows_of(&same, Some(&old), &Shapes::new()), [slot(10, 1_000, 1)]);

    // Under a final root the same slot holds whatever its length.
    let old = old_of(&file, &[(10, 7), (20, 2), (30, 3)], false);
    assert_eq!(rows_of(&tracks, Some(&old), &Shapes::new()).len(), 3);

    // Two copies of a recording, the slots of each its own length: each record
    // finds the one of its length, whichever row comes first.
    let file = file_of(&[(10, 5_000, 4), (20, 5_020, 5)]);
    let old = old_of(&file, &[(10, 9), (20, 9)], true);
    let nine = row_of(9);
    let tracks = [track(30, 5_020, &nine), track(40, 5_000, &nine)];
    let rows = rows_of(&tracks, Some(&old), &Shapes::new());
    assert_eq!(rows, [slot(30, 5_020, 5), slot(40, 5_000, 4)]);
}

// ── What a run asks ─────────────────────────────────────────────────────────

#[test]
fn a_run_asks_once_for_each_recording_no_old_slot_holds_for() {
    let file = file_of(&[(10, 1_000, 1)]);
    let old = old_of(&file, &[(10, 1)], false);
    let (one, two, three) = (row_of(1), row_of(2), row_of(3));
    let peer = Ledger { server: 3, ..row_of(4) };
    let no_path = Ledger { server_path: String::new(), ..row_of(5) };
    let no_hash = Ledger { server_path: "Lib/none.mp3".into(), ..Ledger::default() };
    // A converted file's ledger row is its source's: the route's path is the
    // source's, and it has the source's recording.
    let converted = Ledger { converted_to: mptg::CONTAINER_OPUS, convert_kbps: 192, ..row_of(6) };
    let another = Ledger { server_path: "Lib/other copy.mp3".into(), ..row_of(2) };
    let tracks = [
        track(10, 1_000, &one),
        track(11, 1_000, &one),
        track(20, 2_000, &two),
        track(21, 2_000, &two),
        track(22, 2_000, &another),
        track(30, 3_000, &three),
        track(40, 4_000, &peer),
        track(50, 5_000, &no_path),
        track(60, 6_000, &no_hash),
        track(70, 7_020, &converted),
    ];
    let client = |server: u16| server == 0;
    let ask = |n: u8| Ask {
        hash: recording(n),
        server_paths: vec![format!("Lib/{n:03}.mp3")],
        server: 0,
    };
    // Each path of a recording, once, for when the route has nothing for one.
    let mut both = ask(2);
    both.server_paths.push("Lib/other copy.mp3".into());
    let want = [both.clone(), ask(3), ask(6)];
    assert_eq!(asks_of(&tracks, Some(&old), client), want);
    // With no old file, recording 1 too; and once.
    let want = [ask(1), both, ask(3), ask(6)];
    assert_eq!(asks_of(&tracks, None, client), want);
    // A peer is asked only when a caller says it can be.
    let all = asks_of(&tracks, Some(&old), |_| true);
    assert!(all.iter().any(|a| a.hash == recording(4) && a.server == 3));

    // What came, with no old slot: one fetch fills every copy, a converted file
    // takes its source's bars under its own record's length.
    let rows = rows_of(&tracks, Some(&old), &shapes(&[2, 6]));
    assert!(rows.contains(&slot(21, 2_000, 2)) && rows.contains(&slot(22, 2_000, 2)));
    assert!(rows.contains(&slot(70, 7_020, 6)));
    assert_eq!(rows.len(), 6, "1 twice from its slot, 2 three times, 6");
}

// ── The file on the card ────────────────────────────────────────────────────

/// A vector's rows (13.3), the whole way a run gets them: each hash a card
/// file, whose recording the old file's join knows or whose bars came from the
/// fetch; then the table, and what its file holds.
#[test]
fn vectors_mpwf_commit_from_the_tracks_to_the_file() {
    // A made-up recording of a path hash.
    let hash_of = |h: u64| {
        let mut out = [0; 16];
        out[..8].copy_from_slice(&h.to_le_bytes());
        out[8..].copy_from_slice(&fnv1a64(&h.to_le_bytes()).to_le_bytes());
        out
    };
    let v = json("vectors.json");
    let cases = v["mpwf"]["commit"]["cases"].as_array().expect("commit");
    assert_eq!(cases.len(), 8);
    for e in cases {
        let note = e["note"].as_str().expect("a note");
        let txt = format!("contract=1\nread.mpwf={}\n", e["readMpwf"].as_str().expect("readMpwf"));
        let device = DeviceTxt::read(Some(txt.as_bytes()));
        let old_bytes = e["old"].as_str().map(golden);
        let old = old_bytes.as_deref().map(|b| {
            let t = Table::open(b).expect("the golden");
            let paths = (0..t.rows).map(|r| t.slot(r).expect("its slot").path_hash);
            let paths: Vec<u64> = paths.collect();
            Old::joined(t, paths.into_iter().map(|h| (h, Some(hash_of(h)))), false)
        });
        let same_file = e["old"].is_string() && e["old"] == e["rows"];
        let mut ledgers = Vec::new();
        let mut fetched = Shapes::new();
        let mut files: Vec<(u64, u32)> = Vec::new();
        if let Some(name) = e["rows"].as_str() {
            let bytes = golden(name);
            let t = Table::open(&bytes).expect("the golden");
            let dropped: Vec<u64> = e["drop"].as_array().into_iter().flatten().map(num).collect();
            for r in (0..t.rows).filter(|&r| !dropped.contains(&(r as u64))) {
                let row = t.slot(r).expect("its slot");
                let ms = e["lengthMs"].get(r.to_string()).map_or(row.length_ms, |ms| num(ms) as u32);
                files.push((row.path_hash, ms));
                // The old file's own rows are its slots' to give.
                if !same_file {
                    fetched.insert(hash_of(row.path_hash), row.bars);
                }
            }
        }
        for a in e["add"].as_array().into_iter().flatten() {
            files.push((num(&a["hash"]), num(&a["lengthMs"]) as u32));
            fetched.insert(hash_of(num(&a["hash"])), bars_from(&peaks_of(a)));
        }
        for (h, _) in &files {
            ledgers.push(Ledger { audio_hash: hash_of(*h), ..Ledger::default() });
        }
        let tracks: Vec<Track<'_>> =
            files.iter().zip(&ledgers).map(|(&(h, ms), l)| track(h, ms, l)).collect();
        let rows = rows_of(&tracks, old.as_ref(), &fetched);
        assert_eq!(rows.len(), files.len(), "{note}");
        let got = answer(&device, 43, CARD, old.as_ref(), rows.clone()).expect(note);
        match (e["outcome"].as_str().expect("an outcome"), &got) {
            ("none", Index::None(why)) => {
                let want = if device.writes_mpwf() { NoIndex::NoRows } else { NoIndex::NotRead };
                assert_eq!(*why, want, "{note}");
            }
            ("same", Index::Same) => {}
            ("new", Index::New(file)) => {
                // Its header from this commit, whole, the rows sorted and each
                // CRC for its new place.
                let t = Table::open(file).expect(note);
                t.check().expect(note);
                let h = t.header();
                assert_eq!((h.generation, h.card_id, t.rows as usize), (43, CARD, rows.len()), "{note}");
                for r in 0..t.rows {
                    let s = t.slot(r).expect(note);
                    assert!(rows.contains(&s), "{note}");
                }
            }
            (want, got) => panic!("{note}: {want} is not {got:?}"),
        }
    }
}

/// The fixture card's description through the writer, and the golden back: a
/// new file byte for byte, then that file with the ledger of its card named
/// again.
#[test]
fn the_fixture_cards_file_is_made_byte_for_byte_and_named_again() {
    let d = json("libraries/waves-0000002a.json");
    let (generation, card_id) = (num(&d["generation"]) as u32, num(&d["cardId"]));
    let mut records = Vec::new();
    let mut fetched = Shapes::new();
    for (i, t) in d["tracks"].as_array().expect("tracks").iter().enumerate() {
        let hash = recording(i as u8 + 1);
        fetched.insert(hash, bars_from(&peaks_of(t)));
        let rec = Record { duration_ms: num(&t["lengthMs"]) as u32, ..Record::default() };
        let server_path = format!("Lib/{i}.mp3");
        let ledger = Some(Ledger { audio_hash: hash, server_path, ..Ledger::default() });
        let path = t["path"].as_str().expect("a path").to_string();
        records.push(RecordIn { path, rec, run: Run::default(), ledger });
    }
    assert_eq!(records.len(), 7);
    let want = golden("waves-0000002a.bin");
    let device = reads_mpwf(true);
    let input = IndexIn { generation, card_id, records: &records, old: None, fetched: &fetched };
    assert_eq!(index(&device, &input).expect("a file"), Index::New(want.clone()));

    // That file on the card, the ledger the root it came with: a run with no
    // answers makes the same rows, and names it again.
    let table = Table::open(&want).expect("the golden");
    let hash = |r: &RecordIn| r.ledger.as_ref().and_then(Ledger::canonical_hash);
    let old = Old::joined(table, records.iter().map(|r| (path_hash(&r.path), hash(r))), false);
    let none = Shapes::new();
    let (generation, old_ref) = (generation + 1, Some(&old));
    let input = IndexIn { generation, card_id, records: &records, old: old_ref, fetched: &none };
    assert_eq!(index(&device, &input).expect("the table"), Index::Same);
    assert_eq!(index(&reads_mpwf(false), &input).expect("the table"), Index::None(NoIndex::NotRead));

    // Under a new card id the rows are the same, and the file isn't: every file
    // carries its card's. Nothing is asked for again.
    let card_id = card_id ^ 1;
    let input = IndexIn { generation, card_id, records: &records, old: old_ref, fetched: &none };
    let Index::New(file) = index(&device, &input).expect("a file") else { panic!("a new file") };
    let t = Table::open(&file).expect("the file");
    assert_eq!((t.header().generation, t.header().card_id), (generation, card_id));
    assert_eq!(t.wvsl(), old.table().wvsl(), "the slots, untouched");
    assert!(asks(&records, Some(&old), |_| true).is_empty());
}

/// The fixture card as a run finds it: its waveform index joined through its
/// tags file (13.4: seven of its records, each lengthMs that record's
/// durationMs). A run with no answer makes the same rows, and names it again.
#[test]
fn the_fixture_cards_file_holds_through_its_own_tags_file() {
    let tags = mptg::read(&golden("tags-0000002a.bin"), mptg::Uses::ALL).expect("the tags file");
    let file = golden("waves-0000002a.bin");
    let (_, records, _) = tags.to_input();
    let (generation, card_id) = (43, tags.header.card_id);
    let old = Old::new(Table::open(&file).expect("the file"), Some(&tags), false);
    let none = Shapes::new();
    let got = rows(&records, Some(&old), &none);
    assert_eq!(got.len(), 7, "the card's other records have no slot");
    let input = IndexIn { generation, card_id, records: &records, old: Some(&old), fetched: &none };
    assert_eq!(index(&reads_mpwf(true), &input).expect("the table"), Index::Same);
    // Those seven's bars stay, and the others are what a run asks for.
    let asked = asks(&records, Some(&old), |_| true);
    let can = |r: &&RecordIn| r.ledger.as_ref().is_some_and(|l| !l.server_path.is_empty());
    let room: HashSet<[u8; 16]> =
        records.iter().filter(can).filter_map(|r| r.ledger.as_ref()?.canonical_hash()).collect();
    assert_eq!(asked.len() + 7, room.len());
    let table = Table::open(&file).expect("the file");
    for row in &got {
        assert_eq!(table.find(row.path_hash).slot.as_ref(), Some(row));
    }
}

/// An old row that no longer holds: a new file leaves it out (7.3: a fetch that
/// stops early has no line of its own). No row left: no file.
#[test]
fn a_fetch_with_no_answer_keeps_what_holds_and_names_nothing_only_with_nothing_left() {
    let file = file_of(&[(10, 1_000, 1), (20, 2_000, 2)]);
    let old = old_of(&file, &[(10, 1), (20, 2)], false);
    let device = reads_mpwf(true);
    let (one, two) = (row_of(1), row_of(2));
    let none = Shapes::new();
    let table = |tracks: &[Track<'_>]| {
        answer(&device, 8, CARD, Some(&old), rows_of(tracks, Some(&old), &none)).expect("the table")
    };
    assert_eq!(table(&[track(10, 1_000, &one), track(20, 2_000, &two)]), Index::Same);
    // One left the card.
    let Index::New(smaller) = table(&[track(10, 1_000, &one)]) else { panic!("a new file") };
    assert_eq!(Table::open(&smaller).expect("the file").rows, 1);
    // One's length changed, and nothing else: no CRC says so (4.2).
    assert!(matches!(table(&[track(10, 1_001, &one), track(20, 2_000, &two)]), Index::New(_)));
    // Every one gone.
    assert_eq!(table(&[track(30, 1_000, &row_of(9))]), Index::None(NoIndex::NoRows));
}
