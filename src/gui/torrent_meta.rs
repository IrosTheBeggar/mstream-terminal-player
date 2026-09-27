//! The Add-torrent room's pure half: what a `.torrent` file or a magnet
//! link says about itself, and how a release name turns into a
//! destination path. A port of the record's `lib/util/torrent_meta.dart`
//! (itself a port of the webapp panel's logic), kept function for
//! function so the three clients agree on what they pre-fill. The
//! template resolver and sanitizer mirror the server's
//! `src/torrent/path-template.js`: the preview must match what
//! `/torrent/add` will accept (the server re-validates, so drift
//! surfaces as a submit-time error, never a silent difference).
//!
//! Contract: docs/ux-contracts/add-torrent.md, clauses 3, 4, 10, 12.

use std::sync::OnceLock;

use regex::Regex;

// ── The .torrent file ───────────────────────────────────────────────────────

/// Index of the dict opener (`d`) belonging to a bencoded `4:info` key —
/// a scan rather than a parse: the info dict is the one structure both
/// the name extractor and the structural gate need.
fn info_dict_index(bytes: &[u8]) -> Option<usize> {
    bytes.windows(7).position(|w| w == b"4:infod").map(|i| i + 6)
}

/// True when `bytes` look like a real .torrent: a bencoded dict carrying
/// an `info` dict. Structural only — enough to reject an mp3 or a PDF
/// picked by mistake (clause 3), not a full bencode validation.
pub(crate) fn is_torrent_file(bytes: &[u8]) -> bool {
    bytes.first() == Some(&b'd') && info_dict_index(bytes).is_some()
}

/// The suggested folder name out of a .torrent's info dict: scan for
/// `4:info`, then `4:name<len>:<value>` — a byte-level walk, never the
/// whole info dict (clause 10). Empty on any parse failure; the user can
/// still type a name.
pub(crate) fn extract_torrent_name(bytes: &[u8]) -> String {
    let Some(info) = info_dict_index(bytes) else { return String::new() };
    let hay = &bytes[info + 1..];
    let Some(j) = hay.windows(6).position(|w| w == b"4:name") else { return String::new() };
    let mut k = j + 6;
    let mut digits = String::new();
    while k < hay.len() && hay[k] != b':' {
        digits.push(hay[k] as char);
        k += 1;
    }
    let Ok(len) = digits.parse::<usize>() else { return String::new() };
    if len == 0 || len > 1024 {
        return String::new();
    }
    let start = k + 1;
    if start + len > hay.len() {
        return String::new();
    }
    String::from_utf8_lossy(&hay[start..start + len]).into_owned()
}

// ── The magnet link ─────────────────────────────────────────────────────────

/// The `(key, value)` pairs of a magnet's query, decoded the way a URI
/// parser decodes them (`+` is a space — `dn` values lean on it).
fn magnet_params(value: &str) -> Vec<(String, String)> {
    let Some((_, query)) = value.trim().split_once('?') else { return Vec::new() };
    url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// A BitTorrent "exact topic": a v1 infohash (40 hex / 32 base32) or a v2
/// multihash. Anything else in an `xt` is a topic the server can't add.
fn is_infohash_topic(xt: &str) -> bool {
    let r = regexes();
    r.btih.is_match(xt) || r.btmh.is_match(xt)
}

/// True when `value` is a magnet link the server could act on: the
/// scheme plus at least one `xt` naming an infohash (clause 4). Trackers,
/// `dn` and the rest are optional; a magnet may carry several topics
/// (`xt`, `xt.1`, …) and one usable infohash among them is enough.
pub(crate) fn is_valid_magnet(value: &str) -> bool {
    let v = value.trim();
    if !v.to_lowercase().starts_with("magnet:?") {
        return false;
    }
    magnet_params(v)
        .iter()
        .filter(|(k, _)| k == "xt" || k.starts_with("xt."))
        .any(|(_, xt)| is_infohash_topic(xt))
}

/// The magnet's display name (`dn`), when it carries one.
pub(crate) fn magnet_display_name(value: &str) -> Option<String> {
    magnet_params(value)
        .into_iter()
        .find(|(k, _)| k == "dn")
        .map(|(_, v)| v)
        .filter(|v| !v.trim().is_empty())
}

// ── The release name ────────────────────────────────────────────────────────

/// How sure the parser is of what it pre-filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Confidence {
    High,
    Low,
    #[default]
    None,
}

/// Metadata parsed from a torrent name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TorrentMeta {
    pub artist: String,
    pub album: String,
    pub year: String,
    pub confidence: Confidence,
}

impl TorrentMeta {
    pub(crate) fn new(artist: &str, album: &str, year: &str, confidence: Confidence) -> Self {
        TorrentMeta {
            artist: artist.trim().to_string(),
            album: album.trim().to_string(),
            year: year.trim().to_string(),
            confidence,
        }
    }
}

/// One ladder rung: a pattern, and what its captures mean.
type Rung = (Regex, fn(&regex::Captures) -> TorrentMeta);

struct Regexes {
    btih: Regex,
    btmh: Regex,
    scene_year: Regex,
    scene_space: Regex,
    feat_bracketed: Regex,
    feat_trailing: Regex,
    bracketed_square: Regex,
    bracketed_round: Regex,
    bare_trailing: Regex,
    spaces: Regex,
    patterns: Vec<Rung>,
    illegal: Regex,
    whitespace: Regex,
    edge_dots: Regex,
    template_var: Regex,
    separators: Regex,
    segment_edge: Regex,
    alnum: Regex,
}

/// The parser's patterns, compiled once: they run on every keystroke in
/// the magnet field.
fn regexes() -> &'static Regexes {
    static CELL: OnceLock<Regexes> = OnceLock::new();
    CELL.get_or_init(build_regexes)
}

fn build_regexes() -> Regexes {
    // Format/quality/codec tokens (safe to strip anywhere). The bracketed
    // set also covers editions/types; the bare trailing-strip set omits
    // those, since an album can be named "Live"/"Bonus"/"Deluxe" — but
    // not "FLAC".
    const FMT: &str = "FLAC|WEBFLAC|MP3|AAC|M4A|M4B|MP4A|OGG|OGA|Vorbis|OPUS|ALAC|APE|WV|\
        WavPack|WAV|PCM|AIFF|WMA|AC3|DTS|TAK|TTA|DSD|320|256|224|192|160|128|\
        V0|V1|V2|CBR|VBR|APS|APX|Q[5-9]|Q10|Lossless|Hi-?Res|\
        24[\\s-]?bit|16[\\s-]?bit|\\d+(?:\\.\\d+)?\\s?kHz";
    const SRC: &str = "WEB|CDRip|CDS|CDM|MCD|CDR|CD|VINYL|VLS|LP|SACD|HDCD|BLURAY|BD|DVDA|DVD|\
        HDDVD|TAPE|CASSETTE|DAT";
    const DISC: &str = "\\d+x?CD|CD\\d+|Dis[ck]\\s?\\d+";
    const STATUS: &str = "PROPER|REPACK|DIRFIX|NFOFIX|RERIP|READNFO|INTERNAL|iNT|INT";
    const EDITION: &str = "Retail|Reissue|Remaster(?:ed)?|Remix(?:ed)?|Promo|Advance|Sampler|Bonus|\
        Bootleg|Live|Demo|Deluxe|Anniversary|Mono|Stereo|Limited|Special|\
        Collectors?|Edition|Explicit|Clean|OST|Soundtrack";
    let bracketed = format!("{FMT}|{SRC}|{DISC}|{STATUS}|{EDITION}");
    let bare = format!("{FMT}|{SRC}|{DISC}|{STATUS}");
    let re = |p: &str| Regex::new(p).expect("a torrent-name pattern compiles");

    fn c<'h>(m: &regex::Captures<'h>, i: usize) -> &'h str {
        m.get(i).map_or("", |g| g.as_str())
    }
    fn dot(s: &str) -> String {
        s.replace('.', " ")
    }
    let patterns: Vec<Rung> = vec![
        // "Artist - Album (1973)". The separator requires surrounding
        // spaces so a hyphenated artist (Jay-Z, AC-DC) isn't split on its
        // own dash.
        (re(r"^(.+?)\s+-\s+(.+?)\s*\((\d{4})\)\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 2), c(m, 3), Confidence::High)
        }),
        // "Artist - Album [1973]"
        (re(r"^(.+?)\s+-\s+(.+?)\s*\[(\d{4})\]\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 2), c(m, 3), Confidence::High)
        }),
        // "Artist - 1973 - Album"
        (re(r"^(.+?)\s+-\s+(\d{4})\s+-\s+(.+?)\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 3), c(m, 2), Confidence::High)
        }),
        // "Artist - Album - 1973"
        (re(r"^(.+?)\s+-\s+(.+?)\s+-\s+(\d{4})\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 2), c(m, 3), Confidence::High)
        }),
        // "Artist - Album 1973" (trailing year, space-separated; 19xx/20xx).
        (re(r"^(.+?)\s+-\s+(.+?)\s+((?:19|20)\d{2})\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 2), c(m, 3), Confidence::High)
        }),
        // "Artist.Album.1973" (dot-separated)
        (re(r"^([^.]+)\.([^.]+(?:\.[^.\d][^.]*)*)\.(\d{4})\s*$"), |m| {
            TorrentMeta::new(&dot(c(m, 1)), &dot(c(m, 2)), c(m, 3), Confidence::High)
        }),
        // bare "Artist - Album" (low — software/etc. also match)
        (re(r"^(.+?)\s+-\s+(.+?)\s*$"), |m| {
            TorrentMeta::new(c(m, 1), c(m, 2), "", Confidence::Low)
        }),
    ];

    Regexes {
        btih: re(r"(?i)^urn:btih:([0-9a-f]{40}|[a-z2-7]{32})$"),
        btmh: re(r"(?i)^urn:btmh:1220[0-9a-f]{64}$"),
        scene_year: re(r"^(?:19|20)\d{2}$"),
        scene_space: re(r"[_.]"),
        feat_bracketed: re(r"(?i)\s*[\(\[]\s*(?:featuring|feat|ft|with)\.?\s[^\)\]]*[\)\]]"),
        feat_trailing: re(r"(?i)\s+(?:featuring|feat|ft)\.?\s.*$"),
        bracketed_square: re(&format!("(?i)\\[(?:{bracketed})[^\\]]*\\]")),
        bracketed_round: re(&format!("(?i)\\((?:{bracketed})[^)]*\\)")),
        bare_trailing: re(&format!("(?i)(?:[\\s._-]+(?:{bare}))+[\\s._-]*$")),
        spaces: re(r"\s{2,}"),
        patterns,
        illegal: re("[/\\\\:*?<>|\"\\x00-\\x1f]+"),
        whitespace: re(r"\s+"),
        edge_dots: re(r"^[.\s]+|[.\s]+$"),
        template_var: re(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}"),
        separators: re(r"[/\\]+"),
        segment_edge: re(r"^[\s._-]+|[\s._-]+$"),
        alnum: re(r"[\p{L}\p{N}]"),
    }
}

/// Canonicalize a Various-Artists marker (scene releases use "VA";
/// trackers use "Various Artists") so compilations resolve consistently.
fn canonical_artist(artist: &str) -> String {
    let low = artist.trim().to_lowercase().replace('.', "");
    if low == "va" || low == "various" || low == "various artists" {
        return "Various Artists".to_string();
    }
    artist.trim().to_string()
}

/// Strip a featured-artist clause from an album title — "Album (feat. X)"
/// or "Album feat. X & Y" → "Album". Conservative: bare "with" is left
/// alone (it's common in real titles); only its bracketed form goes.
fn strip_feat(album: &str) -> String {
    let r = regexes();
    let once = r.feat_bracketed.replace_all(album, "");
    r.feat_trailing.replace_all(&once, "").trim().to_string()
}

/// Scene dirnames use a documented two-tier grammar — `_` (and `.`) is the
/// space INSIDE a field and `-` separates fields:
/// `Artist-Title-(Cat)-TYPE-SOURCE-FORMAT-YEAR-GROUP` (no spaces anywhere).
/// That makes the multi-word artist/album split deterministic, which the
/// spaced/dot heuristics can't manage. `None` when the name isn't
/// scene-shaped, so the heuristics run instead.
fn parse_scene_name(raw: &str) -> Option<TorrentMeta> {
    let name = raw.trim();
    // A space means the human-readable display form, not a scene dirname.
    if name.is_empty() || name.contains(' ') || !name.contains('-') {
        return None;
    }
    let fields: Vec<&str> = name.split('-').collect();
    if fields.len() < 3 {
        return None;
    }
    let r = regexes();
    // The standalone YEAR field (rightmost) is the anchor: everything
    // after it is the release group, everything between title and year
    // is source/format/type.
    let year_idx = fields.iter().rposition(|f| r.scene_year.is_match(f))?;
    if year_idx < 2 {
        return None; // an artist and a title field before the year
    }
    let field = |i: usize| -> String {
        let spaced = r.scene_space.replace_all(fields[i], " ");
        r.spaces.replace_all(&spaced, " ").trim().to_string()
    };
    let artist = field(0);
    let album = field(1);
    if artist.is_empty() || album.is_empty() {
        return None;
    }
    Some(TorrentMeta::new(&artist, &album, fields[year_idx], Confidence::High))
}

/// Best-effort artist/album/year extraction from a music release name:
/// the deterministic scene grammar first, then a ladder of spaced/dot
/// heuristics; well-named releases mostly parse, the rest fall through to
/// manual entry. Various-Artists and featured-artist noise are
/// normalized. The parse only pre-fills; it gates nothing (clause 10).
pub(crate) fn parse_music_name(raw: &str) -> TorrentMeta {
    if raw.trim().is_empty() {
        return TorrentMeta::default();
    }
    let core = parse_core(raw);
    TorrentMeta {
        artist: canonical_artist(&core.artist),
        album: strip_feat(&core.album),
        year: core.year,
        confidence: core.confidence,
    }
}

fn parse_core(raw: &str) -> TorrentMeta {
    if let Some(scene) = parse_scene_name(raw) {
        return scene;
    }
    let r = regexes();
    let cleaned = r.bracketed_square.replace_all(raw, "");
    let cleaned = r.bracketed_round.replace_all(&cleaned, "");
    // Strip a trailing run of bare format/source tags ("… 2020 FLAC WEB").
    let cleaned = r.bare_trailing.replace_all(&cleaned, "");
    let cleaned = r.spaces.replace_all(&cleaned, " ").trim().to_string();
    for (pattern, map) in &r.patterns {
        if let Some(m) = pattern.captures(&cleaned) {
            return map(&m);
        }
    }
    // Fallback: the whole name is the album.
    TorrentMeta::new("", &cleaned, "", Confidence::None)
}

// ── The destination path ────────────────────────────────────────────────────

/// Strip filesystem-illegal characters from a path segment (the server's
/// `sanitizeSegment`, mirrored).
pub(crate) fn sanitize_segment(s: &str) -> String {
    let r = regexes();
    let v = r.illegal.replace_all(s, "-");
    let v = r.whitespace.replace_all(&v, " ");
    let v = r.edge_dots.replace_all(&v, "");
    v.chars().take(200).collect()
}

/// Substitute `{{ARTIST}}` / `{{ALBUM}}` / `{{YEAR}}` / `{{GENRE}}` /
/// `{{ALBUMARTIST}}` into `template` (sanitized), then normalize slashes.
pub(crate) fn resolve_template(template: &str, meta: &TorrentMeta, genre: Option<&str>) -> String {
    if template.is_empty() {
        return String::new();
    }
    let r = regexes();
    let lookup = |var: &str| -> String {
        match var.to_uppercase().as_str() {
            "ARTIST" | "ALBUMARTIST" => sanitize_segment(&meta.artist),
            "ALBUM" => sanitize_segment(&meta.album),
            "YEAR" => sanitize_segment(&meta.year),
            "GENRE" => sanitize_segment(genre.unwrap_or("")),
            _ => String::new(),
        }
    };
    let substituted = r.template_var.replace_all(template, |m: &regex::Captures| lookup(&m[1]));
    r.separators
        .split(&substituted)
        // Strip leading/trailing separator junk so a template literal
        // left dangling by an empty variable ("{{ARTIST}} - {{ALBUM}}"
        // with no album → "Artist -") doesn't become a folder name.
        .map(|s| r.segment_edge.replace_all(s, "").into_owned())
        // A segment with no letters or digits left is template
        // punctuation wrapped around empty variables — "({{YEAR}})" with
        // no year → "()" — and would create a junk folder. "(1979)"
        // survives.
        .filter(|s| r.alnum.is_match(s))
        .collect::<Vec<_>>()
        .join("/")
}

/// The destination path: the per-library `template` when one is
/// configured, else the legacy `artist/album` layout (clause 12).
pub(crate) fn compute_path(template: Option<&str>, meta: &TorrentMeta) -> String {
    if let Some(template) = template.filter(|t| !t.is_empty()) {
        return resolve_template(template, meta, None);
    }
    let a = sanitize_segment(&meta.artist);
    let b = sanitize_segment(&meta.album);
    match (a.is_empty(), b.is_empty()) {
        (false, false) => format!("{a}/{b}"),
        (true, false) => b,
        (false, true) => a,
        (true, true) => String::new(),
    }
}

/// Split a computed path for `/torrent/add`: the last segment is the
/// `directoryName`, everything before it the `subPath`. Every segment is
/// sanitized so a hand-typed or torrent-supplied `..`, `\`, control
/// character or the like can't escape the library — defense in depth
/// even though the server re-validates.
pub(crate) fn split_path(path: &str) -> (String, String) {
    let r = regexes();
    let segs: Vec<String> = r
        .separators
        .split(path)
        .map(sanitize_segment)
        .filter(|s| !s.is_empty())
        .collect();
    match segs.split_last() {
        None => (String::new(), String::new()),
        Some((last, rest)) => (rest.join("/"), last.clone()),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal single-file torrent, bencoded by hand.
    fn torrent(name: &str) -> Vec<u8> {
        let mut bytes = b"d8:announce18:http://t.example/4:infod6:lengthi1234e4:name".to_vec();
        bytes.extend(format!("{}:{name}", name.len()).into_bytes());
        bytes.extend(b"12:piece lengthi16384e6:pieces20:");
        bytes.extend([0u8; 20]);
        bytes.extend(b"ee");
        bytes
    }

    #[test]
    fn the_structural_gate_knows_a_torrent_from_an_mp3() {
        assert!(is_torrent_file(&torrent("Vela - Cassini (2020)")));
        assert!(!is_torrent_file(b"ID3\x03\x00\x00\x00"), "an mp3 is not a torrent");
        assert!(!is_torrent_file(b"d4:infoi1ee"), "an info that is not a dict fails the gate");
        assert!(!is_torrent_file(b""), "empty is not a torrent");
    }

    #[test]
    fn the_name_comes_out_of_the_info_dict() {
        assert_eq!(extract_torrent_name(&torrent("Vela - Cassini (2020)")), "Vela - Cassini (2020)");
        assert_eq!(extract_torrent_name(&torrent("Fläche")), "Fläche", "utf-8 names survive");
        assert_eq!(extract_torrent_name(b"d4:infodee"), "", "no name, no guess");
        assert_eq!(extract_torrent_name(b"not bencode"), "");
    }

    #[test]
    fn a_magnet_needs_an_infohash_topic() {
        let v1 = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Vela+-+Cassini";
        assert!(is_valid_magnet(v1));
        assert!(is_valid_magnet("MAGNET:?xt=urn:btih:ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"), "base32, any case");
        assert!(is_valid_magnet(&format!("magnet:?xt=urn:btmh:1220{}", "ab".repeat(32))), "a v2 multihash");
        assert!(is_valid_magnet("magnet:?xt.1=urn:sha1:zzz&xt.2=urn:btih:0123456789abcdef0123456789abcdef01234567"), "one usable topic among several");
        assert!(!is_valid_magnet("magnet:?dn=only+a+name"), "a name is not a topic");
        assert!(!is_valid_magnet("magnet:?xt=urn:btih:tooshort"));
        assert!(!is_valid_magnet("http://example.com/a.torrent"));
        assert!(!is_valid_magnet(""));
        assert_eq!(magnet_display_name(v1).as_deref(), Some("Vela - Cassini"), "dn decodes its pluses");
        assert_eq!(magnet_display_name("magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"), None);
    }

    #[test]
    fn the_scene_grammar_splits_deterministically() {
        let m = parse_music_name("Some_Artist-The_Long_Album_Title-(CAT123)-WEB-FLAC-2021-GRP");
        assert_eq!((m.artist.as_str(), m.album.as_str(), m.year.as_str()), ("Some Artist", "The Long Album Title", "2021"));
        assert_eq!(m.confidence, Confidence::High);
        assert!(parse_scene_name("Artist - Album (2020)").is_none(), "spaces mean the display form");
        assert!(parse_scene_name("Artist-Album").is_none(), "no year, no anchor");
    }

    #[test]
    fn the_spaced_and_dot_forms_walk_the_ladder() {
        let cases: [(&str, (&str, &str, &str), Confidence); 8] = [
            ("Vela - Cassini (2020)", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Vela - Cassini [2020] [FLAC]", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Vela - 2020 - Cassini", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Vela - Cassini - 2020", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Vela - Cassini 2020 FLAC WEB", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Vela.Cassini.2020", ("Vela", "Cassini", "2020"), Confidence::High),
            ("Jay-Z - The Blueprint", ("Jay-Z", "The Blueprint", ""), Confidence::Low),
            ("Untitled Release", ("", "Untitled Release", ""), Confidence::None),
        ];
        for (name, (artist, album, year), confidence) in cases {
            let m = parse_music_name(name);
            assert_eq!((m.artist.as_str(), m.album.as_str(), m.year.as_str()), (artist, album, year), "{name}");
            assert_eq!(m.confidence, confidence, "{name}");
        }
        assert_eq!(parse_music_name("  "), TorrentMeta::default());
    }

    #[test]
    fn compilations_and_features_normalize() {
        assert_eq!(parse_music_name("VA - Summer Hits (2019)").artist, "Various Artists");
        assert_eq!(parse_music_name("Various - Summer Hits (2019)").artist, "Various Artists");
        assert_eq!(parse_music_name("Artist - Album (feat. Someone) (2019)").album, "Album");
        assert_eq!(parse_music_name("Artist - Album feat. Someone & Other - 2019").album, "Album");
        assert_eq!(parse_music_name("Artist - Live with Friends (2019)").album, "Live with Friends", "bare with stays");
    }

    #[test]
    fn segments_are_sanitized_the_servers_way() {
        assert_eq!(sanitize_segment("AC/DC: Back <in> Black?"), "AC-DC- Back -in- Black-", "every illegal run becomes one dash");
        assert_eq!(sanitize_segment("  ..dots..  "), "dots");
        assert_eq!(sanitize_segment("two   spaces"), "two spaces");
        assert_eq!(sanitize_segment(&"x".repeat(300)).chars().count(), 200);
        assert_eq!(sanitize_segment(""), "");
    }

    #[test]
    fn templates_resolve_and_drop_their_dangling_punctuation() {
        let meta = TorrentMeta::new("Vela", "Cassini", "2020", Confidence::High);
        assert_eq!(resolve_template("{{ARTIST}}/{{ALBUM}} ({{YEAR}})", &meta, None), "Vela/Cassini (2020)");
        let no_year = TorrentMeta::new("Vela", "Cassini", "", Confidence::Low);
        // The record's own rule: punctuation inside a segment that still
        // has letters stays (the server's template resolver agrees); only
        // an all-punctuation segment is dropped.
        assert_eq!(resolve_template("{{ARTIST}}/{{ALBUM}} ({{YEAR}})", &no_year, None), "Vela/Cassini ()");
        assert_eq!(resolve_template("{{ARTIST}}/({{YEAR}})/{{ALBUM}}", &no_year, None), "Vela/Cassini", "an all-punctuation segment is dropped");
        let nothing = TorrentMeta::default();
        assert_eq!(resolve_template("{{ARTIST}} - {{ALBUM}}", &nothing, None), "");
        assert_eq!(resolve_template("{{ albumartist }}\\{{unknown}}\\{{ALBUM}}", &meta, None), "Vela/Cassini", "case and backslashes normalize");
        assert_eq!(resolve_template("", &meta, None), "");
    }

    #[test]
    fn the_path_falls_back_to_artist_slash_album() {
        let meta = TorrentMeta::new("Vela", "Cassini", "2020", Confidence::High);
        assert_eq!(compute_path(None, &meta), "Vela/Cassini");
        assert_eq!(compute_path(Some(""), &meta), "Vela/Cassini");
        assert_eq!(compute_path(Some("{{ALBUM}}"), &meta), "Cassini");
        assert_eq!(compute_path(None, &TorrentMeta::new("", "Cassini", "", Confidence::None)), "Cassini");
        assert_eq!(compute_path(None, &TorrentMeta::new("Vela", "", "", Confidence::None)), "Vela");
        assert_eq!(compute_path(None, &TorrentMeta::default()), "");
    }

    #[test]
    fn the_split_names_the_folder_and_cannot_escape() {
        assert_eq!(split_path("Vela/Cassini (2020)"), ("Vela".into(), "Cassini (2020)".into()));
        assert_eq!(split_path("Cassini"), (String::new(), "Cassini".into()));
        assert_eq!(split_path("../../etc/passwd"), ("etc".into(), "passwd".into()), "dot-dot segments vanish");
        assert_eq!(split_path("a\\b\\c"), ("a/b".into(), "c".into()), "backslashes are separators too");
        assert_eq!(split_path("///"), (String::new(), String::new()));
    }
}
