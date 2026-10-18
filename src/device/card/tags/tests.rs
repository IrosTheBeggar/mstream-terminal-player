//! The reader against the parity corpus, test/fixtures/tags (made by the
//! firmware repo's tools/tag_corpus.py, its records by tools/tagref, and
//! copied from there): every file's record field for field, but the five
//! picture fields of a file that has a picture, which wait for its anchor
//! (the module's note; the elected picture itself is held to the corpus's
//! FNV, type and MIME); then the reader's own seams (a path and its bytes
//! alike, a card that fails mid-read, the key table) and the writer that
//! puts back what a conversion dropped.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::ogg::OpusFile;
use lofty::tag::TagExt;
use serde_json::Value;

use super::super::contract::fat::fat_time;
use super::super::contract::mptg::{self, Ledger, Meta, NO_TAGS, READ_RULES, Record, RecordIn};
use super::super::contract::mptg::{KNOWN_RULES1, SOURCE_SOFTWARE, UNREADABLE, Uses};
use super::super::contract::prim::{fnv1a64, qfp_of};
use super::super::contract::text::{ALBUM, RUN_FIELDS, TITLE};
use super::{PARSER_VERSION, Reading, Restore};
use super::{camelot, read_bytes, read_from, read_path, restore};

fn corpus(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/tags").join(rel)
}

fn bytes(rel: &str) -> Vec<u8> {
    std::fs::read(corpus(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn expected() -> serde_json::Map<String, Value> {
    let v: Value = serde_json::from_slice(&bytes("expected.json")).expect("expected.json is JSON");
    v.as_object().expect("an object of records").clone()
}

fn ext(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or("")
}

/// The corpus's audio files, by name.
fn audio_files() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(corpus(""))
        .expect("the corpus folder")
        .map(|e| e.expect("an entry").file_name().to_string_lossy().into_owned())
        .filter(|n| matches!(ext(n), "mp3" | "flac" | "opus"))
        .collect();
    names.sort();
    names
}

// ── Parity (2.17, item 3) ───────────────────────────────────────────────────

/// Every key a record of expected.json has: a key the corpus adds later
/// fails the test until this reader is held to it too.
const KEYS: [&str; 24] = [
    "bpm10", "camelot", "container", "disc", "discTotal", "durationMs", "error", "fields", "flags",
    "known", "note", "picCoding", "picFnv", "picLength", "picMime", "picOffset", "picType",
    "rgAlbumGain", "rgAlbumPeak", "rgTrackGain", "rgTrackPeak", "track", "trackTotal", "year",
];

/// What differs between a reading and its expected record, one line each.
fn differences(got: &Reading, want: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut eq = |what: &str, got: i64, want: i64| {
        if got != want {
            out.push(format!("{what}: {got}, expected {want}"));
        }
    };
    let n = |key: &str| want[key].as_i64().unwrap_or_else(|| panic!("{key} is an integer"));
    let r = &got.rec;
    eq("container", r.container as i64, n("container"));
    eq("flags", r.flags as i64, n("flags"));
    eq("known", r.known as i64, n("known"));
    // The same lofty measures the same length: no ±100 ms needed here.
    eq("durationMs", r.duration_ms as i64, n("durationMs"));
    eq("year", r.year as i64, n("year"));
    eq("track", r.track as i64, n("track"));
    eq("trackTotal", r.track_total as i64, n("trackTotal"));
    eq("disc", r.disc as i64, n("disc"));
    eq("discTotal", r.disc_total as i64, n("discTotal"));
    eq("bpm10", r.bpm10 as i64, n("bpm10"));
    eq("camelot", r.camelot as i64, n("camelot"));
    eq("rgTrackGain", r.rg_track_gain as i64, n("rgTrackGain"));
    eq("rgAlbumGain", r.rg_album_gain as i64, n("rgAlbumGain"));
    eq("rgTrackPeak", r.rg_track_peak as i64, n("rgTrackPeak"));
    eq("rgAlbumPeak", r.rg_album_peak as i64, n("rgAlbumPeak"));
    // No anchor: the record's five picture fields are absent. A file with
    // no picture is the reference's record exactly; one with a picture
    // waits for the anchor locator.
    let fnv = want["picFnv"].as_str().expect("picFnv");
    let anchor = |key: &str| if fnv.is_empty() { n(key) } else { 0 };
    eq("picOffset", r.pic_offset as i64, anchor("picOffset"));
    eq("picLength", r.pic_length as i64, anchor("picLength"));
    eq("picType", r.pic_type as i64, anchor("picType"));
    eq("picMime", r.pic_mime as i64, anchor("picMime"));
    eq("picCoding", r.pic_coding as i64, anchor("picCoding"));
    // The elected picture itself, as lofty read it.
    match &got.picture {
        None if fnv.is_empty() => {}
        Some(p) if format!("{:016x}", p.fnv) == fnv => {
            eq("elected picType", p.pic_type as i64, n("picType"));
            eq("elected picMime", p.mime as i64, n("picMime"));
        }
        other => out.push(format!("picture: {other:?}, expected picFnv {fnv:?}")),
    }
    let fields = want["fields"].as_array().expect("fields");
    assert_eq!(fields.len(), RUN_FIELDS);
    for (i, f) in fields.iter().enumerate() {
        let f = f.as_str().expect("a field");
        if got.run.get(i) != f.as_bytes() {
            out.push(format!("field {i}: {:?}, expected {f:?}", got.run.text(i)));
        }
    }
    let error = want.get("error").and_then(Value::as_str);
    if got.error.as_deref() != error {
        out.push(format!("error: {:?}, expected {error:?}", got.error));
    }
    out
}

#[test]
fn every_corpus_file_reads_as_the_reference_reads_it() {
    let want = expected();
    let mut wrong = Vec::new();
    for (name, rec) in &want {
        for key in rec.as_object().expect("a record").keys() {
            assert!(KEYS.contains(&key.as_str()), "{name}: a key this test doesn't check, {key}");
        }
        let got = read_path(&corpus(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        for d in differences(&got, rec) {
            wrong.push(format!("{name}: {d}"));
        }
    }
    assert!(wrong.is_empty(), "{} differences:\n{}", wrong.len(), wrong.join("\n"));
    // The corpus is all there, and nothing in its folder goes unchecked.
    let names: Vec<String> = want.keys().cloned().collect();
    assert_eq!(names, audio_files());
    assert_eq!(names.len(), 86);
}

#[test]
fn the_corpus_covers_the_flags_and_the_known_bits() {
    // The parity test above is only as good as what the corpus exercises.
    let mut seen = 0u16;
    let mut pictures = 0;
    for name in audio_files() {
        let r = read_path(&corpus(&name)).expect("a corpus file");
        seen |= r.rec.flags;
        pictures += r.picture.is_some() as usize;
        let readable = r.rec.flags & UNREADABLE == 0;
        // 2.6.4: a readable file says it looked for every field, a picture
        // included; an unreadable one, for none.
        assert_eq!(r.rec.known, if readable { KNOWN_RULES1 } else { 0 }, "{name}");
    }
    // Compilation yes and no, both gains, R128, NO_TAGS, UNREADABLE and
    // TRUNCATED: bits 0-7, all of them.
    assert_eq!(seen & 0xFF, 0xFF);
    assert!(pictures >= 10, "{pictures} files with a picture");
}

#[test]
fn the_readings_make_a_tags_file_the_kit_reads_back() {
    // What the engine will do with them: the card's own facts filled in,
    // every record and run into one tags file, read back as written.
    let mut records = Vec::new();
    for name in audio_files() {
        let data = bytes(&name);
        let reading = read_bytes(&data, ext(&name));
        let (size, qfp) = (data.len() as u32, qfp_of(&data));
        let rec = Record { size, fat_time: fat_time(2025, 1, 1, 13, 36, 6), qfp, ..reading.rec };
        let ledger = Some(Ledger::default());
        records.push(RecordIn { path: format!("Corpus/{name}"), rec, run: reading.run, ledger });
    }
    let meta = Meta {
        source: SOURCE_SOFTWARE,
        parser_version: PARSER_VERSION,
        read_rules: READ_RULES,
        generation: 1,
        card_id: 0x0123_4567_89AB_CDEF,
        minor: 0,
        producer: "the tag reader's test".into(),
    };
    let file = mptg::write(&meta, &records, &[]).expect("every reading is a record the kit writes");
    let tags = mptg::read(&file, Uses::ALL).expect("and reads back");
    assert_eq!(tags.records.len(), records.len());
    for e in &tags.records {
        let want = records.iter().find(|r| r.path == e.path).expect("a record written");
        assert_eq!((&e.rec, &e.run), (&want.rec, &want.run), "{}", e.path);
    }
}

// ── The reader's seams ──────────────────────────────────────────────────────

#[test]
fn a_path_and_its_bytes_read_alike_in_any_case() {
    for name in audio_files() {
        let by_path = read_path(&corpus(&name)).expect("a corpus file");
        let data = bytes(&name);
        assert_eq!(read_bytes(&data, ext(&name)), by_path, "{name}");
        assert_eq!(read_bytes(&data, &ext(&name).to_ascii_uppercase()), by_path, "{name}");
    }
    // An extension the reader doesn't know is no audio it can read.
    let r = read_bytes(&bytes("v24_full.mp3"), "wav");
    assert_eq!((r.rec.flags, r.rec.known, r.rec.container), (UNREADABLE, 0, 0));
    assert!(r.run.is_empty() && r.picture.is_none() && r.error.is_some());
}

/// A card pulled mid-read: every read from `cut` on fails.
struct Pulled {
    data: Cursor<Vec<u8>>,
    cut: u64,
}

impl Read for Pulled {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.data.position() >= self.cut {
            return Err(io::Error::other("the card is gone"));
        }
        let room = (self.cut - self.data.position()) as usize;
        let n = buf.len().min(room);
        self.data.read(&mut buf[..n])
    }
}

impl Seek for Pulled {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.data.seek(to)
    }
}

#[test]
fn a_card_that_fails_mid_read_is_an_error_not_an_unreadable_file() {
    // Cut in the tag the repair pass reads, in the head lofty reads after
    // it, and in the tail it seeks to (an MP3's ID3v1 and APE, an Opus
    // stream's last page): never a reading, whatever lofty makes of the
    // bytes it was given.
    for name in ["v24_unsync_tag.mp3", "no_tags.mp3", "opus_pages.opus", "flac_full.flac"] {
        let data = bytes(name);
        let whole = read_bytes(&data, ext(name));
        let len = data.len() as u64;
        let read = |cut| read_from(Pulled { data: Cursor::new(data.clone()), cut }, ext(name));
        let cuts = match ext(name) {
            "flac" => vec![5, 40, flac_audio(&data) - 1],
            _ => vec![5, 40, len / 2, len - 20],
        };
        for cut in cuts {
            let err = read(cut).expect_err(&format!("{name} cut at {cut}"));
            assert_eq!(err.to_string(), "the card is gone", "{name} cut at {cut}");
        }
        assert_eq!(read(u64::MAX).expect("uncut"), whole, "{name}");
    }
    // A FLAC is read no further than its metadata blocks: cut where its
    // audio starts, it reads whole. No file is read through.
    let data = bytes("flac_full.flac");
    let cut = flac_audio(&data);
    assert!(cut < data.len() as u64);
    let src = Pulled { data: Cursor::new(data.clone()), cut };
    assert_eq!(read_from(src, "flac").expect("the metadata whole"), read_bytes(&data, "flac"));
}

/// Where a FLAC's audio frames start: past `fLaC` and its metadata blocks.
fn flac_audio(data: &[u8]) -> u64 {
    assert_eq!(&data[..4], b"fLaC");
    let mut at = 4;
    loop {
        let last = data[at] & 0x80 != 0;
        at += 4 + u32::from_be_bytes([0, data[at + 1], data[at + 2], data[at + 3]]) as usize;
        if last {
            return at as u64;
        }
    }
}

#[test]
fn keys_by_the_camelot_rule() {
    let cases = [
        ("8A", 8),
        ("A minor", 8),
        ("am", 8),
        (" Amin ", 8),
        ("8B", 20),
        ("C", 20),
        ("c major", 20),
        ("Abm", 1),
        ("G#m", 1),
        ("1B", 13),
        ("12A", 12),
        ("Dbm", 12),
        ("12B", 24),
        ("E major", 24),
        ("Gb", 14),
        ("H minor", 0),
        ("", 0),
        // Its first 12 characters only: "A minor" padded past them is none.
        ("A minor     x", 0),
        ("A minor", 8),
    ];
    for (text, key) in cases {
        assert_eq!(camelot(text), key, "{text:?}");
    }
}

// ── Putting back what a conversion dropped ──────────────────────────────────

fn jpeg() -> Vec<u8> {
    let pixel = |x: u32, y: u32| image::Rgb([(x * 16) as u8, (y * 20) as u8, 90]);
    let img = image::RgbImage::from_fn(16, 12, pixel);
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).expect("a JPEG");
    out.into_inner()
}

/// `name` with `what` put back, and its readings before and after.
fn restored(name: &str, what: &Restore<'_>) -> (Reading, Reading) {
    let before = read_bytes(&bytes(name), ext(name));
    let mut file = Cursor::new(bytes(name));
    restore(&mut file, ext(name), what).unwrap_or_else(|e| panic!("{name}: {e}"));
    (before, read_bytes(file.get_ref(), ext(name)))
}

#[test]
fn a_converted_file_gets_its_cover_bpm_and_key_back() {
    let cover = jpeg();
    let what = Restore { cover: Some(&cover), bpm: Some("128"), key: Some("C major") };
    // An Opus stream's comments, an MP3 with no ID3v2 yet, and one whose
    // ID3v2 has a cover, a BPM and a key already (each replaced).
    for name in ["opus_basic.opus", "no_tags.mp3", "v24_full.mp3"] {
        let (before, after) = restored(name, &what);
        assert_eq!((after.rec.bpm10, after.rec.camelot), (1280, 20), "{name}");
        let p = after.picture.expect("the cover");
        let want = (3, 1, fnv1a64(&cover), cover.len());
        assert_eq!((p.pic_type, p.mime, p.fnv, p.bytes), want, "{name}");
        // Nothing else moves: the fields, the numbers, the length.
        assert_eq!(after.run, before.run, "{name}");
        let same = |r: &Reading| {
            let r = r.rec;
            (r.duration_ms, r.year, r.track, r.track_total, r.disc, r.disc_total, r.rg_track_gain)
        };
        assert_eq!(same(&after), same(&before), "{name}");
        let peaks = |r: &Reading| (r.rec.rg_album_gain, r.rec.rg_track_peak, r.rec.rg_album_peak);
        assert_eq!(peaks(&after), peaks(&before), "{name}");
        // A file with no tags has some now.
        assert_eq!(after.rec.flags, before.rec.flags & !NO_TAGS, "{name}");
    }
}

#[test]
fn only_what_is_given_is_put_back() {
    // A BPM alone leaves the key and the cover as they were.
    let bpm = Restore { bpm: Some("90.4"), ..Restore::default() };
    let (before, after) = restored("v24_full.mp3", &bpm);
    assert_eq!(after.rec.bpm10, 900);
    assert_eq!((after.rec.camelot, after.picture), (before.rec.camelot, before.picture));
    // An Opus KEY comment gives way to the INITIALKEY written: lofty reads
    // both as the key, and the first would win.
    let mut keyed = Cursor::new(bytes("opus_basic.opus"));
    let opus = OpusFile::read_from(&mut keyed, ParseOptions::new()).expect("Opus");
    let mut comments = opus.vorbis_comments().clone();
    comments.push("KEY".into(), "Am".into());
    keyed.rewind().expect("rewind");
    comments.save_to(&mut keyed, WriteOptions::default()).expect("a KEY comment");
    assert_eq!(read_bytes(keyed.get_ref(), "opus").rec.camelot, 8);
    let key = Restore { key: Some("Fm"), ..Restore::default() };
    restore(&mut keyed, "opus", &key).expect("restored");
    assert_eq!(read_bytes(keyed.get_ref(), "opus").rec.camelot, 4);
    // Nothing given: the file reads as it did.
    let (before, after) = restored("opus_basic.opus", &Restore::default());
    assert_eq!(after, before);
}

#[test]
fn what_restore_refuses() {
    let png = {
        let img = image::RgbImage::from_fn(4, 4, |_, _| image::Rgb([1, 2, 3]));
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).expect("a PNG");
        out.into_inner()
    };
    let mut file = Cursor::new(bytes("no_tags.mp3"));
    let refused = restore(&mut file, "mp3", &Restore { cover: Some(&png), ..Restore::default() });
    assert_eq!(refused, Err("the cover isn't a JPEG".to_string()));
    assert_eq!(file.get_ref(), &bytes("no_tags.mp3"), "nothing written");
    // A FLAC is never a conversion's output.
    let bpm = Restore { bpm: Some("100"), ..Restore::default() };
    let mut file = Cursor::new(bytes("flac_full.flac"));
    assert!(restore(&mut file, "flac", &bpm).is_err());
    assert_eq!(file.get_ref(), &bytes("flac_full.flac"), "nothing written");
    // Nor is a file lofty can't read written into.
    let mut file = Cursor::new(bytes("lofty_fails_nobom.mp3"));
    assert!(restore(&mut file, "mp3", &bpm).is_err());
    assert_eq!(file.get_ref(), &bytes("lofty_fails_nobom.mp3"), "nothing written");
}

#[test]
fn a_restore_keeps_the_id3v1_fill() {
    // Only the ID3v2 tag is rewritten: the ID3v1 after the audio still
    // fills what the ID3v2 leaves blank (its title, album, year, track and
    // genre here; the artist is the ID3v2's own).
    let bpm = Restore { bpm: Some("100"), ..Restore::default() };
    let (before, after) = restored("v1_fill.mp3", &bpm);
    assert_eq!(before.run.text(TITLE), "Filled Title");
    assert_eq!(before.run.text(ALBUM), "Filled Album");
    assert_eq!(after.run, before.run);
    assert_eq!((after.rec.year, after.rec.track), (1994, 6));
    assert_eq!(after.rec.bpm10, 1000);
}
