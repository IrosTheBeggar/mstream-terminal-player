//! The file name a YouTube download lands under, and whether the folder
//! already has it.
//!
//! The server hands yt-dlp `-o '%(title)s.%(ext)s' --restrict-filenames`
//! over the video's RAW title (never the edited one: the route re-tags the
//! file, it does not rename it). A server with the new job list says the
//! stem in its lookup (`fileStem`); an older one never says what the file
//! will be called until it has landed, so the name is predicted here: a
//! port of yt-dlp's `sanitize_filename(restricted=True)` for the title
//! field (the phone's `lib/util/ytdl_filename.dart`, whose fixtures came
//! from the real yt-dlp 2026.08.19).
//!
//! No crate here normalises Unicode, so the NFKC step yt-dlp starts with is
//! done, as on the phone, for the part that changes a restricted name:
//! compatibility forms that land on ASCII (fullwidth, ligatures, super- and
//! subscripts, "…", "™", circled and "fancy" math letters), and canonical
//! composition of Latin letters with combining marks ("e" + U+0301 is "é",
//! which keeps its "e"; "a" + U+0304 is "ā", which does not). Every other
//! compatibility form becomes "_" or nothing either way — or is too rare to
//! carry (the CJK squared words, U+3200–33FF and U+1F200–1F2FF, and the
//! squared abbreviations of U+1F12A–1F1FF).

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use super::{Label, Tier, Tone, file_name_for};

// ── sanitize_filename, restricted ───────────────────────────────────────────

/// yt-dlp's `sanitize_filename(restricted=True)` for the `%(title)s` field:
/// accented Latin letters transliterated, every other letter outside A–Z
/// made `_` (a title written fully in Cyrillic is `_`), a run of the same
/// substitute squeezed to one and substitutes trimmed off the ends — the
/// title's own `_` and `-` are left as they are.
pub(crate) fn restricted_name(title: &str) -> String {
    // sanitize_filename's own first line (a YouTube title is never empty).
    if title.is_empty() {
        return String::new();
    }
    let t = tables();
    let s: String = nfkc(title).into_iter().collect();
    // "12:34" keeps its digits apart as "12_34", not "12_-34".
    let s = t.timestamp.replace_all(&s, |m: &regex::Captures| m[0].replace(':', "_"));
    let mut out = String::new();
    for c in s.chars() {
        replace_insane(c, &mut out);
    }
    let out = squeeze_substitutes(&out);
    let out = t.edge_substitutes.replace_all(&out, "").replace('\0', "");
    if out.is_empty() { "_".into() } else { out }
}

/// `<restricted title>.<ext>`: the name the server's yt-dlp gives the file
/// for `raw_title`.
pub(crate) fn predicted_file_name(raw_title: &str, codec: &str) -> String {
    file_name_for(&restricted_name(raw_title), codec)
}

/// A run of the same substitute (`\0_` or `\0-`) as one — yt-dlp's
/// `(\0.)(?:(?=\1)..)+`, which the `regex` crate can't say.
fn squeeze_substitutes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\0' && i + 1 < chars.len() {
            let sub = chars[i + 1];
            out.push('\0');
            out.push(sub);
            i += 2;
            while i + 1 < chars.len() && chars[i] == '\0' && chars[i + 1] == sub {
                i += 2;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// yt-dlp's `replace_insane` for restricted names. `\0` marks a substitute,
/// so a run of them can be squeezed and trimmed without touching the
/// title's own `_` and `-`.
fn replace_insane(c: char, out: &mut String) {
    let t = tables();
    if let Some(spelled) = t.accents.get(&c) {
        out.push_str(spelled);
        return;
    }
    let r = c as u32;
    match c {
        '?' | '"' => {}
        _ if r < 32 || r == 127 => {}
        // "Björk: Army of Me" → Bjork_-_Army_of_Me
        ':' => out.push_str("\0_\0-"),
        '\\' | '/' | '|' | '*' | '<' | '>' => out.push_str("\0_"),
        _ if r > 127 || " !&'()[]{}$;`^,#".contains(c) => {
            if !other_or_mark(c) {
                out.push_str("\0_");
            }
        }
        _ => out.push(c),
    }
}

/// Unicode categories C (control, format, private use, unassigned) and M
/// (combining marks): dropped, where any other character outside ASCII is
/// replaced.
fn other_or_mark(c: char) -> bool {
    let mut buf = [0; 4];
    tables().other_or_mark.is_match(c.encode_utf8(&mut buf))
}

fn letter(c: char) -> bool {
    let mut buf = [0; 4];
    tables().letter.is_match(c.encode_utf8(&mut buf))
}

/// yt-dlp's ACCENT_CHARS, zipped the way it zips them.
const ACCENT_FROM: &str = "ÂÃÄÀÁÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖŐØŒÙÚÛÜŰÝÞßàáâãäåæçèéêëìíîïðñòóôõöőøœùúûüűýþÿ";
const ACCENT_TO: &[&str] = &[
    "A", "A", "A", "A", "A", "A", "AE", "C", "E", "E", "E", "E", "I", "I", "I", "I", "D", "N", "O", "O", "O", "O", "O", "O", "O",
    "OE", "U", "U", "U", "U", "U", "Y", "TH", "ss", "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i",
    "o", "n", "o", "o", "o", "o", "o", "o", "o", "oe", "u", "u", "u", "u", "u", "y", "th", "y",
];

// ── letters dropped, and the clash ──────────────────────────────────────────

/// "letters outside A–Z are dropped from file names": the restricted rule
/// loses letters of the raw title. With the server's `fileStem` the stem
/// decides: each letter of the title (after the NFKC step) gives at least
/// one letter of the stem — its own, or yt-dlp's spelling of it (ß → ss) —
/// and a lost one gives none, so a stem with fewer letters than the title's
/// lost some; that holds whatever letters a newer yt-dlp learns to spell.
/// Without a stem, the title alone: a letter neither ASCII nor in yt-dlp's
/// table (the phone's rule).
pub(crate) fn letters_dropped(title: &str, stem: Option<&str>) -> bool {
    if title.is_empty() {
        return false;
    }
    let accents = &tables().accents;
    let mut expected = 0;
    let mut foreign = false;
    for c in nfkc(title) {
        if c.is_ascii_alphabetic() {
            expected += 1;
            continue;
        }
        if !letter(c) {
            continue;
        }
        match accents.get(&c) {
            Some(spelled) => expected += spelled.chars().count(),
            None => {
                foreign = true;
                expected += 1;
            }
        }
    }
    match stem.filter(|s| !s.is_empty()) {
        None => foreign,
        Some(stem) => stem.chars().filter(|c| letter(*c)).count() < expected,
    }
}

/// `name` is in the folder's listing (`files`: `None` when the target
/// didn't list or is new), ignoring case as the phone does.
pub(crate) fn clashes(name: &str, files: Option<&[String]>) -> bool {
    let name = name.to_lowercase();
    !name.is_empty() && files.is_some_and(|files| files.iter().any(|f| f.to_lowercase() == name))
}

/// The room's file line: no file name unless letters drop or it clashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NameCheck {
    /// `One_More_Time.mp3`: the server's stem, else the predicted one; `""`
    /// with neither (no lookup).
    pub name: String,
    pub clash: bool,
    /// Before 6.30 the server re-tags the file that is there instead of
    /// adding one: Add is off ([`super::AddBlock::ClashOlder`]).
    pub blocks: bool,
    /// From 6.30 the server refuses the clash (after the download before
    /// the new job list, before it with one): the primary reads "Add
    /// anyway", and "f change format" makes another file.
    pub add_anyway: bool,
    pub letters_dropped: bool,
    /// The GOLD clash line, or the letters note; `None` with neither.
    pub line: Option<(Tone, Label)>,
}

/// The file line for the video's raw title, the lookup's `fileStem` (a
/// server with the new job list; else the name is predicted), the format,
/// the target folder's own files (the folder check's), and the tier.
pub(crate) fn name_check(
    raw_title: &str,
    file_stem: Option<&str>,
    codec: &str,
    files: Option<&[String]>,
    tier: Tier,
) -> NameCheck {
    let stem = file_stem.filter(|s| !s.is_empty());
    let name = match stem {
        Some(stem) => file_name_for(stem, codec),
        None if !raw_title.is_empty() => predicted_file_name(raw_title, codec),
        None => String::new(),
    };
    let clash = clashes(&name, files);
    let dropped = letters_dropped(raw_title, stem);
    let line = if clash {
        let key = if tier == Tier::Older { "gui.ytdl.file.clash_older" } else { "gui.ytdl.file.clash" };
        Some((Tone::Warn, Label::new(key).with("name", name.clone())))
    } else if dropped && !name.is_empty() {
        Some((Tone::Note, Label::new("gui.ytdl.file.letters_dropped").with("name", name.clone())))
    } else {
        None
    };
    let older = tier == Tier::Older;
    NameCheck { clash, blocks: clash && older, add_anyway: clash && !older, letters_dropped: dropped, line, name }
}

/// `file_name` cut in the middle to at most `max` characters, keeping its
/// start, and its end with the extension and up to eight characters before
/// it: `Gorillaz_-_Feel_Good…Video_HD.mp3`. Unchanged when it fits. Counts
/// characters, so an emoji is never split; a cell budget is the caller's.
pub(crate) fn middle_ellipsis(file_name: &str, max: usize) -> String {
    let runes: Vec<char> = file_name.chars().collect();
    if runes.len() <= max {
        return file_name.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let keep = max - 1; // one goes to the ellipsis
    let dot = runes.iter().rposition(|c| *c == '.');
    // ".mp3", ".opus": a dot further back is part of the title.
    let ext_len = match dot {
        Some(d) if d > 0 && runes.len() - d <= 6 => runes.len() - d,
        _ => 0,
    };
    let end = |n: usize| runes[runes.len() - n..].iter().collect::<String>();
    if keep <= ext_len {
        return format!("…{}", end(keep));
    }
    // The extension and up to eight characters before it, but not more
    // than half of what fits, so a short limit still shows how it starts.
    let tail = (ext_len + 8).min((ext_len + 2).max(keep / 2)).min(keep - 1);
    format!("{}…{}", runes[..keep - tail].iter().collect::<String>(), end(tail))
}

// ── NFKC, the part that matters here ────────────────────────────────────────

struct Tables {
    timestamp: Regex,
    edge_substitutes: Regex,
    other_or_mark: Regex,
    letter: Regex,
    accents: HashMap<char, &'static str>,
    decomposition: HashMap<char, (char, char)>,
    composition: HashMap<(char, char), char>,
    compat_one: HashMap<char, char>,
}

fn tables() -> &'static Tables {
    static CELL: OnceLock<Tables> = OnceLock::new();
    CELL.get_or_init(|| {
        let accents = ACCENT_FROM.chars().zip(ACCENT_TO.iter().copied()).collect();
        let mut decomposition = HashMap::new();
        let mut composition = HashMap::new();
        for (mark, pairs) in LATIN_PAIRS {
            let mark = char::from_u32(*mark).expect("a combining mark");
            let runes: Vec<char> = pairs.chars().collect();
            for pair in runes.chunks(2) {
                decomposition.insert(pair[1], (pair[0], mark));
                composition.insert((pair[0], mark), pair[1]);
            }
        }
        Tables {
            timestamp: Regex::new(r"[0-9]+(?::[0-9]+)+").expect("the timestamp pattern"),
            // Substitutes, with any literal ' ', '_' or '-' next to them, at
            // either end — only where the end itself is a substitute
            // ("-Leading dash" keeps its '-', "_underscores_" its '_').
            edge_substitutes: Regex::new(r"^\x00.(?:\x00.|[ _-])*|(?:\x00.|[ _-])*\x00.$").expect("the edge pattern"),
            other_or_mark: Regex::new(r"^[\p{C}\p{M}]$").expect("the category pattern"),
            letter: Regex::new(r"^\p{L}$").expect("the letter pattern"),
            accents,
            decomposition,
            composition,
            compat_one: COMPAT_ONE_FROM.chars().zip(COMPAT_ONE_TO.chars()).collect(),
        }
    })
}

/// `s` as characters after the compatibility mappings below, with Latin
/// letters and their combining marks decomposed, the marks put in
/// canonical order and composed again — NFKC for every character that
/// changes a restricted name.
fn nfkc(s: &str) -> Vec<char> {
    let mut decomposed = Vec::new();
    for c in s.chars() {
        match compat(c) {
            None => decompose(c, &mut decomposed),
            Some(mapped) => {
                for m in mapped.chars() {
                    decompose(m, &mut decomposed);
                }
            }
        }
    }
    canonical_order(&mut decomposed);
    compose(&decomposed)
}

fn decompose(c: char, out: &mut Vec<char>) {
    match tables().decomposition.get(&c) {
        None => out.push(c),
        Some(&(base, mark)) => {
            decompose(base, out);
            out.push(mark);
        }
    }
}

/// Each run of combining marks sorted by combining class, stably.
fn canonical_order(s: &mut [char]) {
    for i in 1..s.len() {
        let class = combining_class(s[i]);
        if class == 0 {
            continue;
        }
        let mut j = i;
        while j > 0 {
            let before = combining_class(s[j - 1]);
            if before <= class {
                break;
            }
            s.swap(j - 1, j);
            j -= 1;
        }
    }
}

/// Unicode's canonical composition: a mark joins the last starter unless a
/// mark of the same or a higher class stands between them.
fn compose(s: &[char]) -> Vec<char> {
    let composition = &tables().composition;
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    let mut starter: Option<usize> = None;
    let mut last_class = 0;
    for &c in s {
        let class = combining_class(c);
        if let Some(at) = starter
            && let Some(&composite) = composition.get(&(out[at], c))
            && (out.len() == at + 1 || last_class < class)
        {
            out[at] = composite;
            continue;
        }
        if class == 0 {
            starter = Some(out.len());
            last_class = 0;
        } else {
            last_class = class;
        }
        out.push(c);
    }
    out
}

/// Canonical combining classes of U+0300–036F (UnicodeData); every other
/// character counts as a starter here (the marks of other scripts never
/// compose with a Latin letter).
fn combining_class(c: char) -> u8 {
    let r = c as u32;
    if !(0x300..=0x36F).contains(&r) {
        return 0;
    }
    MARK_CLASSES.iter().find(|(from, to, _)| (*from..=*to).contains(&r)).map_or(0, |(_, _, class)| *class)
}

const MARK_CLASSES: &[(u32, u32, u8)] = &[
    (0x300, 0x314, 230), (0x315, 0x315, 232), (0x316, 0x319, 220),
    (0x31A, 0x31A, 232), (0x31B, 0x31B, 216), (0x31C, 0x320, 220),
    (0x321, 0x322, 202), (0x323, 0x326, 220), (0x327, 0x328, 202),
    (0x329, 0x333, 220), (0x334, 0x338, 1), (0x339, 0x33C, 220),
    (0x33D, 0x344, 230), (0x345, 0x345, 240), (0x346, 0x346, 230),
    (0x347, 0x349, 220), (0x34A, 0x34C, 230), (0x34D, 0x34E, 220),
    (0x34F, 0x34F, 0), (0x350, 0x352, 230), (0x353, 0x356, 220),
    (0x357, 0x357, 230), (0x358, 0x358, 232), (0x359, 0x35A, 220),
    (0x35B, 0x35B, 230), (0x35C, 0x35C, 233), (0x35D, 0x35E, 234),
    (0x35F, 0x35F, 233), (0x360, 0x361, 234), (0x362, 0x362, 233),
    (0x363, 0x36F, 230),
];

/// Every precomposed letter of Latin-1, Latin Extended-A/B and Latin
/// Extended Additional with its two-character canonical decomposition
/// (UnicodeData, composition exclusions left out), keyed by the mark:
/// pairs of base, composite. Bases may be composites themselves
/// (Vietnamese "ệ" is "ẹ" + U+0302).
const LATIN_PAIRS: &[(u32, &str)] = &[
    (0x300, "AÀEÈIÌOÒUÙaàeèiìoòuùÜǛüǜNǸnǹĒḔēḕŌṐōṑWẀwẁÂẦâầĂẰăằÊỀêềÔỒôồƠỜơờƯỪưừYỲyỳ"),
    (0x301, concat!(
        "AÁEÉIÍOÓUÚYÝaáeéiíoóuúyýCĆcćLĹlĺNŃnńRŔrŕSŚsśZŹzźÜǗüǘGǴgǵÅǺåǻÆǼæǽØǾøǿ",
        "ÇḈçḉĒḖēḗÏḮïḯKḰkḱMḾmḿÕṌõṍŌṒōṓPṔpṕŨṸũṹWẂwẃÂẤâấĂẮăắÊẾêếÔỐôốƠỚơớƯỨưứ",
    )),
    (0x302, "AÂEÊIÎOÔUÛaâeêiîoôuûCĈcĉGĜgĝHĤhĥJĴjĵSŜsŝWŴwŵYŶyŷZẐzẑẠẬạậẸỆẹệỌỘọộ"),
    (0x303, "AÃNÑOÕaãnñoõIĨiĩUŨuũVṼvṽÂẪâẫĂẴăẵEẼeẽÊỄêễÔỖôỗƠỠơỡƯỮưữYỸyỹ"),
    (0x304, "AĀaāEĒeēIĪiīOŌoōUŪuūÜǕüǖÄǞäǟȦǠȧǡÆǢæǣǪǬǫǭÖȪöȫÕȬõȭȮȰȯȱYȲyȳGḠgḡḶḸḷḹṚṜṛṝ"),
    (0x306, "AĂaăEĔeĕGĞgğIĬiĭOŎoŏUŬuŭȨḜȩḝẠẶạặ"),
    (0x307, concat!(
        "CĊcċEĖeėGĠgġIİZŻzżAȦaȧOȮoȯBḂbḃDḊdḋFḞfḟHḢhḣMṀmṁNṄnṅPṖpṗRṘrṙSṠsṡŚṤśṥ",
        "ŠṦšṧṢṨṣṩTṪtṫWẆwẇXẊxẋYẎyẏſẛ",
    )),
    (0x308, "AÄEËIÏOÖUÜaäeëiïoöuüyÿYŸHḦhḧÕṎõṏŪṺūṻWẄwẅXẌxẍtẗ"),
    (0x309, "AẢaảÂẨâẩĂẲăẳEẺeẻÊỂêểIỈiỉOỎoỏÔỔôổƠỞơởUỦuủƯỬưửYỶyỷ"),
    (0x30A, "AÅaåUŮuůwẘyẙ"),
    (0x30B, "OŐoőUŰuű"),
    (0x30C, concat!("CČcčDĎdďEĚeěLĽlľNŇnňRŘrřSŠsšTŤtťZŽzžAǍaǎIǏiǐOǑoǒUǓuǔÜǙüǚGǦgǧKǨkǩƷǮʒǯ", "jǰHȞhȟ")),
    (0x30F, "AȀaȁEȄeȅIȈiȉOȌoȍRȐrȑUȔuȕ"),
    (0x311, "AȂaȃEȆeȇIȊiȋOȎoȏRȒrȓUȖuȗ"),
    (0x31B, "OƠoơUƯuư"),
    (0x323, concat!("BḄbḅDḌdḍHḤhḥKḲkḳLḶlḷMṂmṃNṆnṇRṚrṛSṢsṣTṬtṭVṾvṿWẈwẉZẒzẓAẠaạEẸeẹIỊiịOỌoọ", "ƠỢơợUỤuụƯỰưựYỴyỵ")),
    (0x324, "UṲuṳ"),
    (0x325, "AḀaḁ"),
    (0x326, "SȘsșTȚtț"),
    (0x327, "CÇcçGĢgģKĶkķLĻlļNŅnņRŖrŗSŞsşTŢtţEȨeȩDḐdḑHḨhḩ"),
    (0x328, "AĄaąEĘeęIĮiįUŲuųOǪoǫ"),
    (0x32D, "DḒdḓEḘeḙLḼlḽNṊnṋTṰtṱUṶuṷ"),
    (0x32E, "HḪhḫ"),
    (0x330, "EḚeḛIḬiḭUṴuṵ"),
    (0x331, "BḆbḇDḎdḏKḴkḵLḺlḻNṈnṉRṞrṟTṮtṯZẔzẕhẖ"),
];

/// NFKC's compatibility mapping, as UnicodeData gives it, for a character
/// whose mapping changes a restricted name; `None` for one NFKC keeps (or
/// maps to something that becomes `_` or nothing just the same).
fn compat(c: char) -> Option<String> {
    if let Some(&one) = tables().compat_one.get(&c) {
        return Some(one.to_string());
    }
    if let Some((_, many)) = COMPAT_MANY.iter().find(|(from, _)| *from == c) {
        return Some(many.to_string());
    }
    let r = c as u32;
    let ch = |n: u32| char::from_u32(n).expect("an ASCII letter").to_string();
    match r {
        // Fullwidth ASCII: "（Ｌｉｖｅ）" is "(Live)".
        0xFF01..=0xFF5E => Some(ch(r - 0xFEE0)),
        // Roman numerals: "Ⅶ" is "VII".
        0x2160..=0x217F => {
            let n = ROMAN[((r - 0x2160) % 16) as usize];
            Some(if r < 0x2170 { n.to_string() } else { n.to_lowercase() })
        }
        0x2460..=0x24EA => Some(enclosed(r)),
        // Mathematical letters, the "fancy font" of a title: 13 alphabets
        // of A–Z a–z, with holes where the letter lives in Letterlike
        // Symbols.
        0x1D400..=0x1D6A3 if !MATH_HOLES.contains(&r) => {
            let i = (r - 0x1D400) % 52;
            Some(ch(if i < 26 { 0x41 + i } else { 0x61 + i - 26 }))
        }
        0x1D7CE..=0x1D7FF => Some(((r - 0x1D7CE) % 10).to_string()),
        0x1F100 => Some("0.".into()),
        0x1F101..=0x1F10A => Some(format!("{},", r - 0x1F101)),
        0x1F110..=0x1F129 => Some(format!("({})", ch(0x41 + r - 0x1F110))),
        0x1F130..=0x1F149 => Some(ch(0x41 + r - 0x1F130)),
        0x1FBF0..=0x1FBF9 => Some((r - 0x1FBF0).to_string()),
        _ => None,
    }
}

/// ①–⑳, ⑴–⒇, ⒈–⒛, ⒜–⒵, Ⓐ–ⓩ and ⓪.
fn enclosed(r: u32) -> String {
    let ch = |n: u32| char::from_u32(n).expect("an ASCII letter");
    match r {
        ..=0x2473 => (r - 0x2460 + 1).to_string(),
        ..=0x2487 => format!("({})", r - 0x2474 + 1),
        ..=0x249B => format!("{}.", r - 0x2488 + 1),
        ..=0x24B5 => format!("({})", ch(0x61 + r - 0x249C)),
        ..=0x24CF => ch(0x41 + r - 0x24B6).to_string(),
        ..=0x24E9 => ch(0x61 + r - 0x24D0).to_string(),
        _ => "0".into(),
    }
}

const ROMAN: [&str; 16] = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII", "L", "C", "D", "M"];

const MATH_HOLES: [u32; 24] = [
    0x1D455, 0x1D49D, 0x1D4A0, 0x1D4A1, 0x1D4A3, 0x1D4A4, 0x1D4A7, 0x1D4A8,
    0x1D4AD, 0x1D4BA, 0x1D4BC, 0x1D4C4, 0x1D506, 0x1D50B, 0x1D50C, 0x1D515,
    0x1D51D, 0x1D53A, 0x1D53F, 0x1D545, 0x1D547, 0x1D548, 0x1D549, 0x1D551,
];

/// One character for one, zipped with [`COMPAT_ONE_TO`]: superscript and
/// subscript letters and digits, letterlike symbols (the Kelvin and
/// Ångström signs included), small and vertical presentation forms,
/// half-width sound marks.
const COMPAT_ONE_FROM: &str = concat!(
    "ª\u{B2}\u{B3}\u{B9}ºſʰʲʳʷʸˡˢˣᴬᴭᴮᴰᴱᴳᴴᴵᴶᴷᴸᴹᴺᴼᴾᴿᵀᵁᵂᵃᵇᵈᵉᵍᵏᵐᵒᵖᵗᵘᵛᵢᵣᵤᵥᶜᶞᶠᶻ",
    "\u{2024}\u{2070}ⁱ\u{2074}\u{2075}\u{2076}\u{2077}\u{2078}\u{2079}",
    "\u{207A}\u{207C}ⁿ\u{2080}\u{2081}\u{2082}\u{2083}\u{2084}\u{2085}",
    "\u{2086}\u{2087}\u{2088}\u{2089}\u{208A}\u{208C}ₐₑₒₓₕₖₗₘₙₚₛₜ",
    "ℂℊℋℌℍℎℐℑℒℓℕℙℚℛℜℝℤℨ\u{212A}\u{212B}ℬℭℯℰℱℳℴℹⅅⅆⅇⅈⅉⱼⱽꟹ",
    "\u{FB29}\u{FE13}\u{FE16}\u{FE33}\u{FE34}\u{FE4D}\u{FE4E}\u{FE4F}",
    "\u{FE52}\u{FE55}\u{FE56}\u{FE62}\u{FE63}\u{FE66}\u{FE6A}\u{FE6B}ﾞﾟ",
);
const COMPAT_ONE_TO: &str = concat!(
    "a231oshjrwylsxAÆBDEGHIJKLMNOPRTUWabdegkmoptuviruvcðfz",
    ".0i456789",
    "+=n012345",
    "6789+=aeoxhklmnpst",
    "CgHHHhIILlNPQRRRZZKÅBCeEFMoiDdeijjVœ",
    "+:?_____",
    ".:?+-=%@\u{3099}\u{309A}",
);

/// One character for several: fractions, ligatures, "…", "™", "№".
const COMPAT_MANY: &[(char, &str)] = &[
    ('\u{BC}', "1\u{2044}4"), ('\u{BD}', "1\u{2044}2"), ('\u{BE}', "3\u{2044}4"),
    ('\u{132}', "IJ"), ('\u{133}', "ij"), ('\u{13F}', "L\u{B7}"), ('\u{140}', "l\u{B7}"),
    ('\u{149}', "ʼn"), ('\u{1C4}', "DŽ"), ('\u{1C5}', "Dž"), ('\u{1C6}', "dž"), ('\u{1C7}', "LJ"),
    ('\u{1C8}', "Lj"), ('\u{1C9}', "lj"), ('\u{1CA}', "NJ"), ('\u{1CB}', "Nj"), ('\u{1CC}', "nj"),
    ('\u{1F1}', "DZ"), ('\u{1F2}', "Dz"), ('\u{1F3}', "dz"), ('\u{1E9A}', "aʾ"),
    ('\u{2025}', ".."), ('\u{2026}', "..."), ('\u{2047}', "??"), ('\u{20A8}', "Rs"),
    ('\u{2100}', "a/c"), ('\u{2101}', "a/s"), ('\u{2103}', "\u{B0}C"), ('\u{2105}', "c/o"),
    ('\u{2106}', "c/u"), ('\u{2109}', "\u{B0}F"), ('\u{2116}', "No"), ('\u{2120}', "SM"),
    ('\u{2121}', "TEL"), ('\u{2122}', "TM"), ('\u{213B}', "FAX"),
    ('\u{2150}', "1\u{2044}7"), ('\u{2151}', "1\u{2044}9"), ('\u{2152}', "1\u{2044}10"),
    ('\u{2153}', "1\u{2044}3"), ('\u{2154}', "2\u{2044}3"), ('\u{2155}', "1\u{2044}5"),
    ('\u{2156}', "2\u{2044}5"), ('\u{2157}', "3\u{2044}5"), ('\u{2158}', "4\u{2044}5"),
    ('\u{2159}', "1\u{2044}6"), ('\u{215A}', "5\u{2044}6"), ('\u{215B}', "1\u{2044}8"),
    ('\u{215C}', "3\u{2044}8"), ('\u{215D}', "5\u{2044}8"), ('\u{215E}', "7\u{2044}8"),
    ('\u{215F}', "1\u{2044}"), ('\u{2189}', "0\u{2044}3"),
    ('\u{2A74}', "::="), ('\u{2A75}', "=="), ('\u{2A76}', "==="),
    ('\u{FB00}', "ff"), ('\u{FB01}', "fi"), ('\u{FB02}', "fl"), ('\u{FB03}', "ffi"), ('\u{FB04}', "ffl"),
    ('\u{FB05}', "st"), ('\u{FB06}', "st"), ('\u{FE19}', "..."), ('\u{FE30}', ".."),
    // The tone-mark singletons are canonical, not compatibility, mappings,
    // but NFKC makes them the plain marks before composing: "n" + U+0341
    // is "ń" (a substitute), not "n".
    ('\u{340}', "\u{300}"), ('\u{341}', "\u{301}"), ('\u{343}', "\u{313}"),
    ('\u{344}', "\u{308}\u{301}"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Names the real yt-dlp 2026.08.19 gave the build's fixture titles
    /// (the phone's `fixtures`, copied).
    const FIXTURES: &[(&str, &str)] = &[
        ("One More Time", "One_More_Time"),
        ("Gorillaz - Feel Good Inc. (Official Video) [HD]", "Gorillaz_-_Feel_Good_Inc._Official_Video_HD"),
        ("Кино – Группа крови", "_"),
        ("Sweater Weather (Official Video)", "Sweater_Weather_Official_Video"),
        ("Daft Punk - Around the World (Official Audio)", "Daft_Punk_-_Around_the_World_Official_Audio"),
        ("Beyoncé - Déjà Vu", "Beyonce_-_Deja_Vu"),
        ("Mötley Crüe – Kickstart My Heart", "Motley_Crue_Kickstart_My_Heart"),
        ("Björk: Army of Me", "Bjork_-_Army_of_Me"),
        ("Live at 12:34:56 - full set", "Live_at_12_34_56_-_full_set"),
        ("Track 1:23", "Track_1_23"),
        ("-Leading dash", "-Leading_dash"),
        ("--Double leading", "--Double_leading"),
        ("Foo - _Bar", "Foo_-__Bar"),
        ("  spaced  out  ", "spaced_out"),
        ("AC/DC - Back In Black", "AC_DC_-_Back_In_Black"),
        ("What?! Why?", "What_Why"),
        ("\"Quoted\" title", "Quoted_title"),
        ("A|B*C<D>E", "A_B_C_D_E"),
        ("Ｆｕｌｌｗｉｄｔｈ ＡＢＣ", "Fullwidth_ABC"),
        ("ﬁligree ligature", "filigree_ligature"),
        ("Æon Flux Œuvre ß", "AEon_Flux_OEuvre_ss"),
        ("Sigur Rós – Hoppípolla", "Sigur_Ros_Hoppipolla"),
        ("米津玄師 - Lemon", "Lemon"),
        ("Lemon / 米津玄師", "Lemon"),
        ("official髭男dism - Pretender", "official_dism_-_Pretender"),
        ("Ελληνικά - τραγούδι", "_"),
        ("Rock & Roll (feat. Someone) [2011 Remaster]", "Rock_Roll_feat._Someone_2011_Remaster"),
        ("..dots.at.start", "..dots.at.start"),
        ("_underscores_", "_underscores_"),
        ("100% Pure #1 $hit {x} `y` ^z ;w ,v", "100%_Pure_1_hit_x_y_z_w_v"),
        ("Tab\there", "Tabhere"),
        ("emoji \u{1F3B5} song", "emoji_song"),
        ("Café del Mar – Energy 52", "Cafe_del_Mar_Energy_52"),
        ("Naïve façade", "Naive_facade"),
        ("Ø Ð Þ ð þ", "O_D_TH_o_th"),
        ("İstanbul ı", "stanbul"),
        ("a  -  b", "a_-_b"),
        ("x — y – z", "x_y_z"),
        ("Ñandú", "Nandu"),
        ("Über Ärger Öl", "Uber_Arger_Ol"),
        ("مرحبا - عمرو دياب", "_"),
        ("שלום - שיר", "_"),
        ("Song (Lyrics) | Artist", "Song_Lyrics_Artist"),
        ("Hello: World: Again", "Hello_-_World_-_Again"),
        ("2:00 AM", "2_00_AM"),
    ];

    /// More titles run through the same real yt-dlp for the phone's port.
    const ORACLE: &[(&str, &str)] = &[
        ("Cafe\u{301} au lait", "Cafe_au_lait"),
        ("Ma\u{304}ori", "M_ori"),
        ("Vie\u{323}\u{302}t Nam", "Vi_t_Nam"),
        ("Viê\u{323}t", "Vi_t"),
        ("م\u{64E}ر\u{652}ح\u{64E}ب\u{64B}ا Hi", "Hi"),
        ("A\u{64E}B", "AB"),
        ("नमस\u{94D}त\u{947} Namaste", "Namaste"),
        ("안녕 Hello \u{1100}\u{1161}", "Hello"),
        ("No\u{A0}Break", "No_Break"),
        ("Zero\u{200D}Width", "ZeroWidth"),
        ("Heart \u{2764}\u{FE0F} Song", "Heart_Song"),
        ("\u{2764}\u{FE0F}", "_"),
        ("Wait… What", "Wait..._What"),
        ("Brand\u{2122} Song", "BrandTM_Song"),
        ("𝐁𝐨𝐥𝐝 Text", "Bold_Text"),
        ("Final Fantasy Ⅶ", "Final_Fantasy_VII"),
        ("① First", "1_First"),
        ("１２\u{FF1A}３４ Ｌｉｖｅ", "12_34_Live"),
        ("x² + y²", "x2_+_y2"),
        ("½ Price", "1_2_Price"),
        ("สว\u{E31}สด\u{E35} Thai", "Thai"),
        ("A\u{FE0F}B ℹ\u{FE0F} Info", "AB_i_Info"),
        ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} Family", "Family"),
        ("Ae\u{303}\u{301} Pho\u{31B}\u{309} o\u{302}\u{301}", "A_Ph"),
    ];

    /// Cases taken from the algorithm (yt-dlp's code with Python's NFKC),
    /// not run through yt-dlp itself. The phone's lone-surrogate case
    /// ("a\u{D800}b") has no Rust string to hold it.
    const DERIVED: &[(&str, &str)] = &[
        ("Absolute \u{212A} zero", "Absolute_K_zero"),
        ("\u{212B}ngström", "Angstrom"),
        ("\u{24C2}usic ①②③", "Music_123"),
        ("⑴ ⒈ \u{249C}", "1_1._a"),
        ("xa\u{336}\u{304}y", "x_y"),
        ("aſ\u{307}b", "a_b"),
        ("a\u{1D455}b", "ab"),
        ("𝟎𝟘 𝔸", "00_A"),
        ("\u{1F130}\u{1F131} \u{1F110}", "AB_A"),
        ("No. \u{2116} 1 \u{2122} \u{2120}", "No._No_1_TM_SM"),
        ("Room 10½", "Room_101_2"),
        ("ﬀﬃﬄﬅ", "ffffifflst"),
        ("Na\u{308}\u{304}ive", "N_ive"),
        ("Song ~ 100% @ home = +1", "Song_~_100%_@_home_=_+1"),
        ("trailing -_ :", "trailing"),
        (": leading colon", "leading_colon"),
        ("Ellipsis…", "Ellipsis..."),
        ("Ｓｏｎｇ\u{FF08}Ｌｉｖｅ\u{FF09}", "Song_Live"),
        ("Vertical\u{FE19} forms\u{FE30}", "Vertical..._forms.."),
        ("Ĳsselmeer ǅ", "IJsselmeer_D"),
        ("ᵀᴴᴱ small caps", "THE_small_caps"),
        ("H₂O x²", "H2O_x2"),
        ("Pin\u{303}a n\u{341}", "Pina"),
        ("Ste\u{340}le Ange\u{341}lique", "Stele_Angelique"),
        ("u\u{344}", "_"),
    ];

    #[test]
    fn the_restricted_name_matches_the_real_yt_dlp() {
        for (title, stem) in FIXTURES.iter().chain(ORACLE).chain(DERIVED) {
            assert_eq!(restricted_name(title), *stem, "{title:?}");
        }
    }

    #[test]
    fn nothing_left_is_an_underscore_and_nothing_to_start_with_is_nothing() {
        assert_eq!(restricted_name(""), "");
        assert_eq!(restricted_name("???"), "_");
        assert_eq!(restricted_name("   "), "_");
        assert_eq!(restricted_name("1:2:3"), "1_2_3", "a timestamp keeps its digits apart");
        assert_eq!(restricted_name("Ratio 16:9 and A:B"), "Ratio_16_9_and_A_-B");
    }

    #[test]
    fn the_tables_zip_one_for_one() {
        assert_eq!(ACCENT_FROM.chars().count(), ACCENT_TO.len());
        assert_eq!(COMPAT_ONE_FROM.chars().count(), COMPAT_ONE_TO.chars().count());
        for (_, pairs) in LATIN_PAIRS {
            assert_eq!(pairs.chars().count() % 2, 0, "{pairs}");
        }
    }

    #[test]
    fn the_predicted_name_is_the_raw_title_restricted_with_the_extension() {
        assert_eq!(predicted_file_name("One More Time", "mp3"), "One_More_Time.mp3");
        assert_eq!(predicted_file_name("Кино – Группа крови", "mp3"), "_.mp3");
        assert_eq!(predicted_file_name("Gorillaz - Feel Good Inc. (Official Video) [HD]", "aac"), "Gorillaz_-_Feel_Good_Inc._Official_Video_HD.m4a");
        assert_eq!(predicted_file_name("Sweater Weather (Official Video)", "opus"), "Sweater_Weather_Official_Video.opus");
    }

    #[test]
    fn letters_dropped_by_the_stem_agree_with_the_title_alone_for_every_real_name() {
        let mut dropped = 0;
        let mut n = 0;
        for (title, stem) in FIXTURES.iter().chain(ORACLE).chain(DERIVED) {
            n += 1;
            let by_stem = letters_dropped(title, Some(stem));
            assert_eq!(by_stem, letters_dropped(title, None), "{title:?} → {stem:?}");
            dropped += usize::from(by_stem);
        }
        assert_eq!(n, 94, "every fixture read");
        assert!(dropped > 10 && dropped < n, "some drop letters and some don't ({dropped})");
        assert!(letters_dropped("Кино – Группа крови", Some("_")));
        assert!(!letters_dropped("Beyoncé - Déjà Vu", Some("Beyonce_-_Deja_Vu")));
        assert!(!letters_dropped("Ø Ð Þ ð þ ß Æ Œ", Some("O_D_TH_o_th_ss_AE_OE")), "two-letter spellings count as kept");
    }

    #[test]
    fn the_phones_letters_cases_on_the_title_alone() {
        for title in ["Кино – Группа крови", "米津玄師 - Lemon", "official髭男dism - Pretender", "İstanbul", "Māori", "Ma\u{304}ori", "Viê\u{323}t", "안녕 Hello", "Ελληνικά"] {
            assert!(letters_dropped(title, None), "{title}");
        }
        for title in ["One More Time", "Beyoncé - Déjà Vu", "Cafe\u{301} au lait", "Ø Ð Þ ð þ ß Æ Œ", "ﬁligree", "𝐁𝐨𝐥𝐝 Text", "Ｆｕｌｌｗｉｄｔｈ", "Nº ª", "Heart \u{2764}\u{FE0F} \u{1F3B5}", "12:34 — ½ ™", ""] {
            assert!(!letters_dropped(title, None), "{title}");
        }
    }

    #[test]
    fn a_letter_a_newer_yt_dlp_learns_to_spell_is_kept_by_the_stem() {
        assert!(letters_dropped("Łódź", None), "the table has no Ł or ź");
        assert!(!letters_dropped("Łódź", Some("Lodz")), "the stem kept every letter");
        assert!(letters_dropped("Łódź", Some("_od_")));
        assert!(letters_dropped("Łódź", Some("")), "an empty stem is none");
    }

    #[test]
    fn the_clash_is_the_name_in_the_listing_ignoring_case() {
        let files: Vec<String> = ["one_more_time.MP3", "Feel_Good_Inc.opus"].map(String::from).to_vec();
        assert!(clashes("One_More_Time.mp3", Some(&files)));
        assert!(!clashes("One_More_Time.opus", Some(&files)), "a different format makes a different file");
        assert!(clashes("Feel_Good_Inc.opus", Some(&files)));
        assert!(!clashes("One_More_Time.mp3", None), "a folder that did not list");
        assert!(!clashes("", Some(&[".mp3".to_string()])), "no name, no clash");
        let m4a = vec!["One_More_Time.m4a".to_string()];
        assert!(name_check("One More Time", Some("One_More_Time"), "aac", Some(&m4a), Tier::New).clash, "AAC lands as .m4a");
        assert!(name_check("One More Time", Some("One_More_Time"), "m4b", Some(&m4a), Tier::New).clash);
    }

    #[test]
    fn the_file_line_names_the_clash_else_the_letters_else_nothing() {
        let there = vec!["One_More_Time.mp3".to_string()];
        let check = name_check("One More Time", Some("One_More_Time"), "mp3", Some(&there), Tier::New);
        assert_eq!(check.name, "One_More_Time.mp3");
        assert!(check.clash && check.add_anyway && !check.blocks && !check.letters_dropped);
        let (tone, label) = check.line.expect("a clash line");
        assert_eq!((tone, label.key, label.param("name")), (Tone::Warn, "gui.ytdl.file.clash", Some("One_More_Time.mp3")));
        let kino = name_check("Кино – Группа крови", Some("_"), "mp3", Some(&[]), Tier::New);
        assert_eq!((kino.name.as_str(), kino.clash, kino.letters_dropped), ("_.mp3", false, true));
        assert_eq!(kino.line.map(|(t, l)| (t, l.key)), Some((Tone::Note, "gui.ytdl.file.letters_dropped")));
        let both = name_check("Кино – Группа крови", Some("_"), "mp3", Some(&["_.MP3".to_string()]), Tier::New);
        assert_eq!(both.line.map(|(_, l)| l.key), Some("gui.ytdl.file.clash"));
        assert_eq!(name_check("One More Time", Some("One_More_Time"), "mp3", Some(&[]), Tier::New).line, None);
        assert_eq!(name_check("", None, "mp3", Some(&[]), Tier::New).line, None, "no lookup: no name to show");
    }

    #[test]
    fn an_older_server_without_a_stem_gets_the_predicted_name_and_its_clash_blocks() {
        let there = vec!["One_More_Time.mp3".to_string()];
        let older = name_check("One More Time", None, "mp3", Some(&there), Tier::Older);
        assert_eq!(older.name, "One_More_Time.mp3", "predicted");
        assert!(older.clash && older.blocks && !older.add_anyway);
        assert_eq!(older.line.map(|(t, l)| (t, l.key)), Some((Tone::Warn, "gui.ytdl.file.clash_older")));
        let full = name_check("One More Time", None, "mp3", Some(&there), Tier::Full);
        assert!(full.clash && full.add_anyway && !full.blocks, "6.30 refuses it after the download: Add anyway");
        assert_eq!(full.line.map(|(_, l)| l.key), Some("gui.ytdl.file.clash"));
        assert!(!name_check("One More Time", None, "opus", Some(&there), Tier::Older).clash);
    }

    #[test]
    fn the_middle_ellipsis_keeps_the_start_the_extension_and_eight_before_it() {
        let gorillaz = "Gorillaz_-_Feel_Good_Inc._Official_Video_HD.mp3";
        assert_eq!(middle_ellipsis(gorillaz, 33), "Gorillaz_-_Feel_Good…Video_HD.mp3");
        assert_eq!(middle_ellipsis(gorillaz, gorillaz.len()), gorillaz);
        assert_eq!(middle_ellipsis(gorillaz, 100), gorillaz);
        assert_eq!(middle_ellipsis("_.mp3", 5), "_.mp3");
        for max in 8..gorillaz.len() {
            let cut = middle_ellipsis(gorillaz, max);
            assert_eq!(cut.chars().count(), max, "{max}");
            assert!(cut.ends_with(".mp3") && cut.contains('…') && cut.starts_with('G'), "{cut}");
        }
        assert_eq!(middle_ellipsis(gorillaz, 15), "Gorilla…_HD.mp3");
        assert_eq!(middle_ellipsis(gorillaz, 3), "…p3");
        assert_eq!(middle_ellipsis(gorillaz, 1), "…");
        assert_eq!(middle_ellipsis(gorillaz, 0), "");
        assert_eq!(middle_ellipsis("abcdefghijklmnopqrstuvwxyz", 10), "abcde…wxyz");
        assert_eq!(middle_ellipsis("Mr.Brightside_The_Killers_Official", 12), "Mr.Bri…icial");
        let emoji = middle_ellipsis(&"\u{1F3B5}".repeat(12), 7);
        assert_eq!(emoji.chars().count(), 7);
        assert_eq!(emoji, "\u{1F3B5}\u{1F3B5}\u{1F3B5}…\u{1F3B5}\u{1F3B5}\u{1F3B5}");
    }
}
