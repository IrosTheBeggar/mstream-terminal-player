//! The reader against the parity corpus, test/fixtures/tags (made by the
//! firmware repo's tools/tag_corpus.py, its records by tools/tagref, and
//! copied from there): every file's record field for field, the picture's
//! anchor included, and each anchor read back as the device reads it to
//! the image the corpus put there; the anchor locator on cut and mutated
//! copies, across Ogg pages, and in what lofty read; then the reader's own
//! seams (a path and its bytes alike, a card that fails mid-read, the key
//! table) and the writer that puts back what a conversion dropped.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;

use lofty::config::{ParseOptions, ParsingMode, WriteOptions};
use lofty::file::AudioFile;
use lofty::flac::FlacFile;
use lofty::mpeg::MpegFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::{OggPictureStorage, OpusFile};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{TagExt, TagType};
use serde_json::Value;

use super::super::contract::fat::fat_time;
use super::super::contract::mptg::{self, Ledger, Meta, NO_TAGS, READ_RULES, Record, RecordIn};
use super::super::contract::mptg::{CODING_APE, CODING_OGG_BASE64, CODING_RAW, CODING_UNSYNC};
use super::super::contract::mptg::{KNOWN_RULES1, SOURCE_SOFTWARE, UNREADABLE, Uses};
use super::super::contract::prim::{fnv1a64, qfp_of};
use super::super::contract::text::{ALBUM, RUN_FIELDS, TITLE};
use super::{PARSER_VERSION, Reading, Restore, anchors};
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

/// What differs between a reading of `file` and its expected record, one
/// line each.
fn differences(got: &Reading, want: &Value, file: &[u8], ogg: bool) -> Vec<String> {
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
    eq("picOffset", r.pic_offset as i64, n("picOffset"));
    eq("picLength", r.pic_length as i64, n("picLength"));
    eq("picType", r.pic_type as i64, n("picType"));
    eq("picMime", r.pic_mime as i64, n("picMime"));
    eq("picCoding", r.pic_coding as i64, n("picCoding"));
    // The elected picture itself, as lofty read it, and the image its
    // anchor reads back to.
    let fnv = want["picFnv"].as_str().expect("picFnv");
    match &got.picture {
        None if fnv.is_empty() => {}
        Some(p) if format!("{:016x}", p.fnv) == fnv => {
            eq("elected picType", p.pic_type as i64, n("picType"));
            eq("elected picMime", p.mime as i64, n("picMime"));
        }
        other => out.push(format!("picture: {other:?}, expected picFnv {fnv:?}")),
    }
    if !fnv.is_empty() {
        let read = read_anchor(file, r, ogg).map(|image| format!("{:016x}", fnv1a64(&image)));
        if read.as_deref() != Some(fnv) {
            out.push(format!("the anchor reads {read:?}, expected picFnv {fnv:?}"));
        }
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
        for d in differences(&got, rec, &bytes(name), ext(name) == "opus") {
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
    let mut codings = [0; 4];
    for name in audio_files() {
        let r = read_path(&corpus(&name)).expect("a corpus file");
        seen |= r.rec.flags;
        pictures += r.picture.is_some() as usize;
        if r.rec.pic_offset != 0 {
            codings[r.rec.pic_coding as usize] += 1;
        }
        let readable = r.rec.flags & UNREADABLE == 0;
        // 2.6.4: a readable file says it looked for every field, a picture
        // included; an unreadable one, for none.
        assert_eq!(r.rec.known, if readable { KNOWN_RULES1 } else { 0 }, "{name}");
    }
    // Compilation yes and no, both gains, R128, NO_TAGS, UNREADABLE and
    // TRUNCATED: bits 0-7, all of them.
    assert_eq!(seen & 0xFF, 0xFF);
    assert!(pictures >= 10, "{pictures} files with a picture");
    // Every picture anchored, under each of the four codings: raw, ID3
    // unsynchronised, base64 across Ogg pages (and in a FLAC's comments),
    // APEv2.
    assert_eq!(codings.iter().sum::<usize>(), pictures);
    assert!(codings.iter().all(|&n| n > 0), "codings {codings:?}");
}

// ── The anchors, read back ──────────────────────────────────────────────────

/// The image a record's anchor names, read the way the device reads it
/// (2.6.4) and apart from the locator (the firmware's TagFixtures reader):
/// raw bytes, unsynchronised ones, or base64 characters stepping over the
/// Ogg page headers they meet, decoded, and a FLAC PICTURE's image taken
/// from them. None when the anchor doesn't read.
fn read_anchor(file: &[u8], r: &Record, ogg: bool) -> Option<Vec<u8>> {
    let (at, n) = (r.pic_offset as usize, r.pic_length as usize);
    match r.pic_coding {
        CODING_RAW | CODING_APE => file.get(at..at + n).map(<[u8]>::to_vec),
        CODING_UNSYNC => {
            let (mut out, mut ff) = (Vec::new(), false);
            for &c in file.get(at..at + n)? {
                if ff && c == 0 {
                    ff = false;
                    continue;
                }
                out.push(c);
                ff = c == 0xFF;
            }
            Some(out)
        }
        CODING_OGG_BASE64 => base64_picture(file, at, n, ogg),
        _ => None,
    }
}

/// An Ogg page's body at `at`, its header first: where it starts, how long.
fn page_body(file: &[u8], at: usize) -> Option<(usize, usize)> {
    let h = file.get(at..at + 27).filter(|h| h.starts_with(b"OggS"))?;
    let segs = h[26] as usize;
    let lacing = file.get(at + 27..at + 27 + segs)?;
    Some((at + 27 + segs, lacing.iter().map(|&l| l as usize).sum()))
}

fn base64_picture(file: &[u8], at: usize, n: usize, ogg: bool) -> Option<Vec<u8>> {
    // Where the anchor's page ends: the characters go on in the next one.
    let mut end = file.len();
    if ogg {
        let mut page = 0;
        loop {
            let (start, body) = page_body(file, page)?;
            if (start..start + body).contains(&at) {
                end = start + body;
                break;
            }
            page = start + body;
        }
    }
    let (mut chars, mut pos) = (Vec::new(), at);
    while chars.len() < n {
        if ogg && pos == end {
            let (start, body) = page_body(file, pos)?;
            (pos, end) = (start, start + body);
            continue;
        }
        chars.push(*file.get(pos)?);
        pos += 1;
    }
    let mut block = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in chars.into_iter().take_while(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        (acc, bits) = (acc << 6 | v as u32, bits + 6);
        if bits >= 8 {
            bits -= 8;
            block.push((acc >> bits) as u8);
        }
    }
    let be32 = |at: usize| block.get(at..at + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()));
    if block.len() < 32 {
        return None;
    }
    let mut at = 8 + be32(4)? as usize;
    let described = be32(at)? as usize;
    // lofty skips the description only when it fits before the rest.
    let rest = block.len().checked_sub(at + 4)?;
    at += 4 + if described > 0 && described < rest { described } else { 0 } + 16;
    let len = be32(at)? as usize;
    block.get(at + 4..at + 4 + len).map(<[u8]>::to_vec)
}

/// Where the Ogg pages of `file` have a body starting at `at`.
fn starts_a_page(file: &[u8], at: usize) -> bool {
    let mut page = 0;
    while let Some((start, body)) = page_body(file, page) {
        if start == at {
            return true;
        }
        page = start + body;
    }
    false
}

/// A file's bytes edited at random, mostly in the tags at its head (the
/// device's fuzz pass): a bit flipped, a byte set, a size field blown up,
/// a byte put in or taken out, the file cut.
fn mutate(full: &[u8], rng: &mut fastrand::Rng) -> Vec<u8> {
    let mut b = full.to_vec();
    for _ in 0..rng.usize(1..=8) {
        if b.is_empty() {
            break;
        }
        let span = if rng.usize(..4) != 0 { b.len().min(2048) } else { b.len() };
        let at = rng.usize(..span);
        match rng.u8(..9) {
            0 => b[at] ^= 1 << rng.u8(..8),
            1 => b[at] = rng.u8(..),
            2 => b[at] = 0xFF,
            3 => b[at] = 0,
            4 => b[at] = 0x80,
            5 => (at..b.len().min(at + 4)).for_each(|k| b[k] = rng.u8(..) | 0x40),
            6 => b.insert(at, rng.u8(..)),
            7 => drop(b.remove(at)),
            _ => b.truncate(at),
        }
    }
    b
}

/// What 2.6.4 holds a reading of `file` to: an anchor reads back to the
/// picture elected, its type and MIME that picture's; no anchor, none of
/// the five fields.
fn anchor_holds(file: &[u8], r: &Reading, ogg: bool) -> Result<(), String> {
    let rec = &r.rec;
    if rec.pic_offset == 0 {
        let rest = (rec.pic_length, rec.pic_type, rec.pic_mime, rec.pic_coding);
        return if rest == (0, 0, 0, 0) { Ok(()) } else { Err(format!("no anchor, but {rest:?}")) };
    }
    let p = r.picture.ok_or("an anchor and no picture elected")?;
    if (rec.pic_type, rec.pic_mime) != (p.pic_type, p.mime) {
        return Err(format!("type and MIME {:?}, elected {p:?}", (rec.pic_type, rec.pic_mime)));
    }
    match read_anchor(file, rec, ogg) {
        Some(image) if fnv1a64(&image) == p.fnv => Ok(()),
        Some(_) => Err("the anchor reads another picture's bytes".into()),
        None => Err("the anchor doesn't read".into()),
    }
}

#[test]
fn cut_and_mutated_files_anchor_only_their_elected_picture() {
    // Each file with a picture, cut at a stride and mutated: the reader
    // holds to 2.6.4 whatever it is given. And the locator alone, under
    // every kind of tag the file's format has, never anchors the original
    // picture where its bytes don't read back.
    let mut rng = fastrand::Rng::with_seed(0x7A6B);
    let (mut reads, mut anchored, mut wrong) = (0, 0, Vec::new());
    for name in audio_files() {
        let full = bytes(&name);
        let (ext, ogg) = (ext(&name), ext(&name) == "opus");
        let Some(original) = read_bytes(&full, ext).picture else { continue };
        let kinds: &[TagType] = match ext {
            "mp3" => &[TagType::Id3v2, TagType::Ape],
            "flac" => &[TagType::Id3v2, TagType::VorbisComments],
            _ => &[TagType::VorbisComments],
        };
        let cuts = (0..full.len()).step_by(full.len() / 150 + 1);
        let mut cases: Vec<Vec<u8>> = cuts.map(|n| full[..n].to_vec()).collect();
        cases.extend((0..60).map(|_| mutate(&full, &mut rng)));
        for (i, case) in cases.iter().enumerate() {
            let r = read_bytes(case, ext);
            reads += 1;
            anchored += (r.rec.pic_offset != 0) as usize;
            if let Err(e) = anchor_holds(case, &r, ogg) {
                wrong.push(format!("{name}, case {i}: {e}"));
            }
            for &kind in kinds {
                let mut file = Cursor::new(case.as_slice());
                let found = anchors::locate(&mut file, &[], ext, kind, &original);
                let Some(a) = found.expect("a slice reads") else { continue };
                let (pic_offset, pic_length, pic_coding) = (a.offset, a.length, a.coding);
                let rec = Record { pic_offset, pic_length, pic_coding, ..Record::default() };
                if read_anchor(case, &rec, ogg).map(|image| fnv1a64(&image)) != Some(original.fnv) {
                    wrong.push(format!("{name}, case {i}, {kind:?}: {a:?} isn't the picture"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
    assert!(reads > 3000 && anchored > reads / 4, "{anchored} of {reads} readings anchored");
}

#[test]
fn an_opus_picture_is_anchored_wherever_its_pages_cut_its_comment() {
    // lofty's writer fills a page with 8,160 bytes of a packet (and keeps
    // the file's vendor string): a comment before the picture's, of each
    // length about that, puts the page's end in the picture comment's
    // key, at its '=', and at each of the value's first characters in turn.
    let cover = jpeg();
    let mut at_a_page = 0;
    for pad in 8085..8115 {
        let mut file = Cursor::new(bytes("opus_basic.opus"));
        let mut tag = VorbisComments::default();
        tag.insert("COMMENT".into(), "c".repeat(pad));
        let pic = Picture::unchecked(cover.clone()).pic_type(PictureType::CoverFront);
        tag.insert_picture(pic.mime_type(MimeType::Jpeg).build(), None).expect("a picture");
        tag.save_to(&mut file, WriteOptions::default()).expect("written");
        let data = file.into_inner();
        let r = read_bytes(&data, "opus");
        assert_eq!(r.rec.pic_coding, CODING_OGG_BASE64, "pad {pad}");
        anchor_holds(&data, &r, true).unwrap_or_else(|e| panic!("pad {pad}: {e}"));
        assert_eq!(r.picture.map(|p| p.fnv), Some(fnv1a64(&cover)), "pad {pad}");
        at_a_page += starts_a_page(&data, r.rec.pic_offset as usize) as usize;
    }
    // One value starts a page: its anchor is that page's first body byte,
    // past the header the packet steps over.
    assert_eq!(at_a_page, 1);
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

/// A card that keeps the byte ranges read from it, and refuses a read past
/// those of `only` when given them.
struct Watched {
    data: Cursor<Vec<u8>>,
    read: Vec<(u64, u64)>,
    only: Option<Vec<(u64, u64)>>,
}

impl Read for Watched {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let at = self.data.position();
        let end = (at + buf.len() as u64).min(self.data.get_ref().len() as u64);
        if let Some(only) = &self.only {
            // Inside their union: from `at`, range after range, to `end`.
            let mut reach = at;
            for &(s, e) in only {
                if s <= reach && reach < e {
                    reach = e;
                }
            }
            if reach < end {
                return Err(io::Error::other(format!("{at}..{end}, read again")));
            }
        }
        let n = self.data.read(buf)?;
        self.read.push((at, at + n as u64));
        Ok(n)
    }
}

impl Seek for Watched {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.data.seek(to)
    }
}

#[test]
fn the_anchor_is_found_in_what_lofty_read() {
    // Each file with a picture read by lofty alone, the byte ranges it read
    // kept; then whole, the anchor too, from a card that refuses any other
    // byte: the walk takes its bytes from what lofty read, and never reads
    // the card again for them.
    let mut pictures = 0;
    for name in audio_files() {
        let data = bytes(&name);
        if read_bytes(&data, ext(&name)).rec.pic_offset == 0 {
            continue;
        }
        let mut alone = Watched { data: Cursor::new(data.clone()), read: Vec::new(), only: None };
        let opts = ParseOptions::new().parsing_mode(ParsingMode::Relaxed);
        let parsed = match ext(&name) {
            "mp3" => MpegFile::read_from(&mut alone, opts).map(drop),
            "flac" => FlacFile::read_from(&mut alone, opts).map(drop),
            _ => OpusFile::read_from(&mut alone, opts).map(drop),
        };
        parsed.unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut only = alone.read;
        only.sort();
        let watched = Watched { data: Cursor::new(data.clone()), read: Vec::new(), only: Some(only) };
        let reading = read_from(watched, ext(&name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(reading, read_bytes(&data, ext(&name)), "{name}");
        pictures += 1;
    }
    assert!(pictures >= 10, "{pictures} files with a picture");
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

/// `name` with `what` put back, and its readings before and after; the
/// picture's anchor after holds to 2.6.4.
fn restored(name: &str, what: &Restore<'_>) -> (Reading, Reading) {
    let before = read_bytes(&bytes(name), ext(name));
    let mut file = Cursor::new(bytes(name));
    restore(&mut file, ext(name), what).unwrap_or_else(|e| panic!("{name}: {e}"));
    let after = read_bytes(file.get_ref(), ext(name));
    let ogg = ext(name) == "opus";
    anchor_holds(file.get_ref(), &after, ogg).unwrap_or_else(|e| panic!("{name}: {e}"));
    (before, after)
}

#[test]
fn a_converted_file_gets_its_cover_bpm_and_key_back() {
    let cover = jpeg();
    let what = Restore { cover: Some(&cover), bpm: Some("128"), key: Some("C major") };
    // An Opus stream's comments, an MP3 with no ID3v2 yet, and one whose
    // ID3v2 has a cover, a BPM and a key already (each replaced).
    let files = [
        ("opus_basic.opus", CODING_OGG_BASE64),
        ("no_tags.mp3", CODING_RAW),
        ("v24_full.mp3", CODING_RAW),
    ];
    for (name, coding) in files {
        let (before, after) = restored(name, &what);
        assert_eq!((after.rec.bpm10, after.rec.camelot), (1280, 20), "{name}");
        let p = after.picture.expect("the cover");
        let want = (3, 1, fnv1a64(&cover), cover.len());
        assert_eq!((p.pic_type, p.mime, p.fnv, p.bytes), want, "{name}");
        // Anchored where it was written: the record's picture is the cover.
        let r = after.rec;
        assert_eq!((r.pic_type, r.pic_mime, r.pic_coding), (3, 1, coding), "{name}");
        assert_ne!(r.pic_offset, 0, "{name}");
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
