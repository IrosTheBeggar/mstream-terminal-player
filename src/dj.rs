//! Harmonic-mixing helpers for Auto-DJ: musical keys on the Camelot wheel and
//! tempo windows.
//!
//! mStream's `/api/v1/db/random-songs` filters on Camelot codes (`1A`..`12B`)
//! and on a list of BPM windows, but the *tags* on a track carry whatever the
//! tagger wrote — "A minor", "Am", "8A", "Gbm". Converting a tag to a code and
//! then to its harmonic neighbours is the client's job, so it lives here,
//! pure and testable.

/// A position on the Camelot wheel: `number` 1..=12, `minor` picking the A
/// (minor) or B (major) ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Camelot {
    pub number: u8,
    pub minor: bool,
}

impl Camelot {
    pub fn code(&self) -> String {
        format!("{}{}", self.number, if self.minor { 'A' } else { 'B' })
    }

    /// The classic harmonic-mixing neighbourhood: the same code, one step
    /// either way around the wheel, and the relative major/minor. Anything
    /// further afield tends to clash.
    pub fn compatible(&self) -> Vec<Camelot> {
        let step = |n: u8, delta: i8| -> u8 {
            let zero_based = (n as i8 - 1 + delta).rem_euclid(12);
            (zero_based + 1) as u8
        };
        vec![
            *self,
            Camelot { number: step(self.number, -1), minor: self.minor },
            Camelot { number: step(self.number, 1), minor: self.minor },
            Camelot { number: self.number, minor: !self.minor },
        ]
    }
}

/// Camelot number for each major key, indexed by pitch class (C = 0).
/// Minor keys borrow their relative major's number.
const MAJOR_NUMBERS: [u8; 12] = [8, 3, 10, 5, 12, 7, 2, 9, 4, 11, 6, 1];

/// Parse whatever a tagger wrote into a Camelot code.
///
/// Accepts a Camelot code directly ("8A"), or a note plus an optional
/// quality ("A minor", "Am", "F# maj", "Bbm", "C"). Returns `None` for
/// anything unrecognised — callers then simply skip the key constraint
/// rather than sending garbage the server would reject.
pub fn to_camelot(raw: &str) -> Option<Camelot> {
    let cleaned: String = raw
        .trim()
        .to_ascii_lowercase()
        .replace(['♯', '♭'], "")
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    // The unicode accidentals were stripped above only to normalise width;
    // re-derive them from the original so "F♯" still reads as sharp.
    let normalised = raw
        .trim()
        .to_ascii_lowercase()
        .replace('♯', "#")
        .replace('♭', "b")
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
        .collect::<String>();

    parse_camelot_code(&normalised).or_else(|| parse_note(&normalised))
}

fn parse_camelot_code(s: &str) -> Option<Camelot> {
    let bytes = s.as_bytes();
    let split = bytes.iter().position(|c| c.is_ascii_alphabetic())?;
    if split == 0 {
        return None; // starts with a letter — a note name, not a code
    }
    let (digits, letter) = s.split_at(split);
    if letter.len() != 1 {
        return None;
    }
    let number: u8 = digits.parse().ok()?;
    if !(1..=12).contains(&number) {
        return None;
    }
    match letter {
        "a" => Some(Camelot { number, minor: true }),
        "b" => Some(Camelot { number, minor: false }),
        _ => None,
    }
}

fn parse_note(s: &str) -> Option<Camelot> {
    let mut chars = s.chars();
    let letter = chars.next()?;
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();

    // An accidental, if the next character is one. Note the ambiguity: in
    // "bb" the first 'b' is the note and the second is the flat.
    let (pitch_class, rest) = match rest.strip_prefix('#') {
        Some(tail) => ((base + 1) % 12, tail.to_string()),
        None => match rest.strip_prefix('b') {
            Some(tail) => ((base + 11) % 12, tail.to_string()),
            None => (base, rest),
        },
    };

    // "maj" has to be checked before the bare "m" of "minor".
    let minor = if rest.starts_with("maj") || rest.is_empty() {
        false
    } else if rest.starts_with('m') {
        true
    } else {
        return None; // trailing junk we don't understand
    };

    let number = if minor {
        MAJOR_NUMBERS[((pitch_class + 3) % 12) as usize]
    } else {
        MAJOR_NUMBERS[pitch_class as usize]
    };
    Some(Camelot { number, minor })
}

/// Compatible Camelot codes for a track's key tag, ready to send as
/// `musicalKeys`. Empty when the tag is missing or unparseable.
pub fn compatible_keys(raw: Option<&str>) -> Vec<String> {
    raw.and_then(to_camelot)
        .map(|c| c.compatible().iter().map(Camelot::code).collect())
        .unwrap_or_default()
}

/// Only the seed's own key — for people who want a set that never leaves it.
pub fn exact_keys(raw: Option<&str>) -> Vec<String> {
    raw.and_then(to_camelot).map(|c| vec![c.code()]).unwrap_or_default()
}

// ── Sonic pool ──────────────────────────────────────────────────────────────

/// Ends of the useful cosine band for the sonic pool.
///
/// The embeddings do not spread over 0..1 in any useful way: the server's own
/// calibration puts same-artist pairs around .6–.9 and cross-artist ones
/// around .3–.7. A slider over the raw value would spend most of its travel
/// selecting either everything or nothing, so the band is what gets exposed.
pub const SONIC_LOOSEST: f64 = 0.30;
pub const SONIC_TIGHTEST: f64 = 0.85;

/// Map the panel's 1–100 tightness onto `minSimilarity`. Zero means the sonic
/// pool is switched off, and returns `None` — the caller must then send
/// neither `similarTo` nor `minSimilarity`, which the server requires as a
/// pair.
pub fn sonic_threshold(tightness: u32) -> Option<f64> {
    if tightness == 0 {
        return None;
    }
    let t = f64::from(tightness.clamp(1, 100));
    let raw = SONIC_LOOSEST + (t - 1.0) / 99.0 * (SONIC_TIGHTEST - SONIC_LOOSEST);
    Some((raw * 100.0).round() / 100.0)
}

// ── Settings ────────────────────────────────────────────────────────────────

/// How hard to hold the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyMatching {
    Off,
    /// The Camelot neighbourhood — what harmonic mixing normally means.
    #[default]
    Compatible,
    /// The seed's own key and nothing else.
    Strict,
}

/// What the sonic pool measures distance from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SonicAnchor {
    /// Just what's playing — the set follows each track in turn.
    Current,
    /// Recent picks averaged into a centroid, so the set drifts as a whole
    /// instead of walking away one song at a time.
    #[default]
    Session,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GenreMode {
    #[default]
    Off,
    /// Only these genres. Note this also excludes untagged tracks — the
    /// server treats "only these" as the stricter promise.
    Whitelist,
    /// Anything but these. Untagged tracks pass.
    Blacklist,
}

// Labels are the on-disk spelling as well as the on-screen one. Anything
// unrecognised falls back to the default rather than refusing to load a
// config someone hand-edited.
impl KeyMatching {
    pub fn label(self) -> &'static str {
        match self {
            KeyMatching::Off => "off",
            KeyMatching::Compatible => "compatible",
            KeyMatching::Strict => "strict",
        }
    }
    pub fn from_label(raw: &str) -> Self {
        match raw {
            "off" => KeyMatching::Off,
            "strict" => KeyMatching::Strict,
            _ => KeyMatching::Compatible,
        }
    }
    pub fn next(self) -> Self {
        match self {
            KeyMatching::Off => KeyMatching::Compatible,
            KeyMatching::Compatible => KeyMatching::Strict,
            KeyMatching::Strict => KeyMatching::Off,
        }
    }
}

impl SonicAnchor {
    pub fn label(self) -> &'static str {
        match self {
            SonicAnchor::Current => "current",
            SonicAnchor::Session => "session",
        }
    }
    pub fn from_label(raw: &str) -> Self {
        match raw {
            "current" => SonicAnchor::Current,
            _ => SonicAnchor::Session,
        }
    }
    pub fn next(self) -> Self {
        match self {
            SonicAnchor::Current => SonicAnchor::Session,
            SonicAnchor::Session => SonicAnchor::Current,
        }
    }
}

impl GenreMode {
    pub fn label(self) -> &'static str {
        match self {
            GenreMode::Off => "off",
            GenreMode::Whitelist => "whitelist",
            GenreMode::Blacklist => "blacklist",
        }
    }
    pub fn from_label(raw: &str) -> Self {
        match raw {
            "whitelist" => GenreMode::Whitelist,
            "blacklist" => GenreMode::Blacklist,
            _ => GenreMode::Off,
        }
    }
    pub fn next(self) -> Self {
        match self {
            GenreMode::Off => GenreMode::Whitelist,
            GenreMode::Whitelist => GenreMode::Blacklist,
            GenreMode::Blacklist => GenreMode::Off,
        }
    }
}

/// Everything the Auto-DJ panel controls.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Percent either side of the seed tempo. Zero drops the tempo filter.
    pub tempo_tolerance: u32,
    pub key_matching: KeyMatching,
    /// 1–10, or zero for no floor.
    pub min_rating: u32,
    /// How many recently-played artists to keep out.
    pub artist_cooldown: u32,
    /// 1–100, or zero for no sonic pool.
    pub sonic_tightness: u32,
    pub sonic_anchor: SonicAnchor,
    pub genre_mode: GenreMode,
    pub genres: Vec<String>,
}

/// Bounds for the panel's adjustable numbers. A tempo tolerance past ~30%
/// stops meaning anything, and the server caps ratings at 10.
pub const TEMPO_TOLERANCE_MAX: u32 = 30;
pub const RATING_MAX: u32 = 10;
pub const ARTIST_COOLDOWN_MAX: u32 = 20;

impl Default for Settings {
    fn default() -> Self {
        Settings::from_prefs(&crate::config::AutoDjPrefs::default())
    }
}

impl Settings {
    pub fn from_prefs(prefs: &crate::config::AutoDjPrefs) -> Self {
        Settings {
            tempo_tolerance: prefs.tempo_tolerance.min(TEMPO_TOLERANCE_MAX),
            key_matching: KeyMatching::from_label(&prefs.key_matching),
            min_rating: prefs.min_rating.min(RATING_MAX),
            artist_cooldown: prefs.artist_cooldown.min(ARTIST_COOLDOWN_MAX),
            sonic_tightness: prefs.sonic_tightness.min(100),
            sonic_anchor: SonicAnchor::from_label(&prefs.sonic_anchor),
            genre_mode: GenreMode::from_label(&prefs.genre_mode),
            genres: prefs.genres.clone(),
        }
    }

    pub fn to_prefs(&self) -> crate::config::AutoDjPrefs {
        crate::config::AutoDjPrefs {
            tempo_tolerance: self.tempo_tolerance,
            key_matching: self.key_matching.label().to_string(),
            min_rating: self.min_rating,
            artist_cooldown: self.artist_cooldown,
            sonic_tightness: self.sonic_tightness,
            sonic_anchor: self.sonic_anchor.label().to_string(),
            genre_mode: self.genre_mode.label().to_string(),
            genres: self.genres.clone(),
        }
    }

    /// Tempo tolerance as the fraction `bpm_windows` takes.
    pub fn tolerance(&self) -> f64 {
        f64::from(self.tempo_tolerance) / 100.0
    }
}

// ── Request building ────────────────────────────────────────────────────────

use crate::api::types::{BpmWindow, RandomSongRequest, Track};

/// Turn the panel's settings and the current session into a random-songs
/// request.
///
/// Pure on purpose: what gets asked of the server is the part worth pinning
/// down in tests, and it depends on enough moving pieces — a seed that may
/// lack tags, capabilities that may withhold the sonic pool, filters that
/// must vanish rather than go out empty — that reading the JSON is the only
/// honest way to know.
///
/// `anchors` are recent track paths, newest first, and `recent_artists` the
/// names to keep out; both are trimmed here to whatever the settings allow.
pub fn build_random_request(
    settings: &Settings,
    seed: Option<&Track>,
    ignore_list: Vec<u32>,
    anchors: &[String],
    recent_artists: &[String],
    sonic_available: bool,
) -> (RandomSongRequest, Option<String>) {
    let mut request = RandomSongRequest { ignore_list, ..Default::default() };
    let mut note = None;

    if let Some(seed) = seed {
        if settings.tempo_tolerance > 0 {
            if let Some(bpm) = seed.metadata.bpm {
                let to_window = |r: BpmRange| BpmWindow { min: r.min, max: r.max };
                let tolerance = settings.tolerance();
                request.bpm_ranges =
                    bpm_windows(f64::from(bpm), tolerance).into_iter().map(to_window).collect();
                // The server relaxes to the wide set before dropping tempo
                // altogether; twice the tolerance is that second chance.
                request.bpm_ranges_wide = bpm_windows(f64::from(bpm), tolerance * 2.0)
                    .into_iter()
                    .map(to_window)
                    .collect();
            }
        }
        request.musical_keys = match settings.key_matching {
            KeyMatching::Off => Vec::new(),
            KeyMatching::Compatible => compatible_keys(seed.metadata.musical_key.as_deref()),
            KeyMatching::Strict => exact_keys(seed.metadata.musical_key.as_deref()),
        };

        // Be honest when there was nothing to match on: the pick is really
        // just random, and the user should know that rather than assume the
        // tempo matching is broken.
        let wanted_continuity =
            settings.tempo_tolerance > 0 || settings.key_matching != KeyMatching::Off;
        if wanted_continuity && request.bpm_ranges.is_empty() && request.musical_keys.is_empty() {
            note = Some("no tempo or key tags on this track".to_string());
        }
    }

    if settings.min_rating > 0 {
        request.min_rating = Some(settings.min_rating.min(RATING_MAX));
    }

    if settings.artist_cooldown > 0 {
        request.ignore_artists = recent_artists
            .iter()
            .take(settings.artist_cooldown as usize)
            .cloned()
            .collect();
    }

    if settings.genre_mode != GenreMode::Off && !settings.genres.is_empty() {
        request.genres = settings.genres.clone();
        request.genre_mode = Some(settings.genre_mode.label().to_string());
    }

    // The pool needs the index; without it the request would 403 as a whole,
    // taking the ordinary tempo/key pick down with it.
    let threshold = sonic_available.then(|| sonic_threshold(settings.sonic_tightness)).flatten();
    let seeds: Vec<String> = match settings.sonic_anchor {
        SonicAnchor::Current => anchors.iter().take(1).cloned().collect(),
        SonicAnchor::Session => anchors.to_vec(),
    };
    let request = request.with_sonic_pool(&seeds, threshold);

    // Finish the "nothing to match on" note now that it is settled whether a
    // pool is carrying the pick. "Picking at random" would be a lie when the
    // choice is still confined to tracks that sound like the seed.
    let note = note.map(|reason| {
        if request.min_similarity.is_some() {
            format!("{reason} — picking from the sonic pool")
        } else {
            format!("{reason} — picking at random")
        }
    });
    (request, note)
}

/// A tempo window, inclusive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpmRange {
    pub min: f64,
    pub max: f64,
}

/// Plausible tempo for a music track. Half/double-time windows outside this
/// are dropped rather than sent as noise.
const BPM_FLOOR: f64 = 40.0;
const BPM_CEILING: f64 = 220.0;

pub const TIGHT_TOLERANCE: f64 = 0.06;
pub const WIDE_TOLERANCE: f64 = 0.12;

/// Windows around a seed tempo at the same, half and double time.
///
/// Sending all three is what the server's docs recommend: a 140 BPM track
/// mixes naturally into 70 BPM, and matching only the literal number would
/// reject those.
pub fn bpm_windows(bpm: f64, tolerance: f64) -> Vec<BpmRange> {
    if !bpm.is_finite() || bpm <= 0.0 {
        return Vec::new();
    }
    [bpm, bpm / 2.0, bpm * 2.0]
        .into_iter()
        .filter(|center| (BPM_FLOOR..=BPM_CEILING).contains(center))
        .map(|center| BpmRange {
            min: (center * (1.0 - tolerance) * 10.0).round() / 10.0,
            max: (center * (1.0 + tolerance) * 10.0).round() / 10.0,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(raw: &str) -> Option<String> {
        to_camelot(raw).map(|c| c.code())
    }

    #[test]
    fn parses_camelot_codes_directly() {
        assert_eq!(code("8A"), Some("8A".into()));
        assert_eq!(code("8a"), Some("8A".into()));
        assert_eq!(code("12B"), Some("12B".into()));
        assert_eq!(code("1A"), Some("1A".into()));
        assert_eq!(code("13A"), None, "off the wheel");
        assert_eq!(code("0B"), None);
    }

    #[test]
    fn parses_spelled_out_keys() {
        // The anchor: A minor is 8A, C major is 8B.
        assert_eq!(code("A minor"), Some("8A".into()));
        assert_eq!(code("Am"), Some("8A".into()));
        assert_eq!(code("Amin"), Some("8A".into()));
        assert_eq!(code("a-minor"), Some("8A".into()));
        assert_eq!(code("C major"), Some("8B".into()));
        assert_eq!(code("C"), Some("8B".into()), "no quality means major");
        assert_eq!(code("Cmaj"), Some("8B".into()), "'maj' is not 'minor'");
    }

    #[test]
    fn handles_accidentals_including_the_bb_ambiguity() {
        assert_eq!(code("Bb"), Some("6B".into()), "B-flat major");
        assert_eq!(code("Bbm"), Some("3A".into()), "B-flat minor");
        assert_eq!(code("B"), Some("1B".into()), "plain B major");
        assert_eq!(code("Bm"), Some("10A".into()));
        assert_eq!(code("F#"), Some("2B".into()));
        assert_eq!(code("F#m"), Some("11A".into()));
        assert_eq!(code("Gb"), Some("2B".into()), "enharmonic with F#");
        assert_eq!(code("F♯m"), Some("11A".into()), "unicode sharp");
        assert_eq!(code("E♭"), Some("5B".into()), "unicode flat");
    }

    #[test]
    fn rejects_junk() {
        assert_eq!(code(""), None);
        assert_eq!(code("   "), None);
        assert_eq!(code("unknown"), None);
        assert_eq!(code("H minor"), None, "no such note");
        assert_eq!(code("Cx"), None, "trailing junk");
    }

    #[test]
    fn every_minor_key_maps_to_its_relative_major_number() {
        // The relative major of a minor key shares its Camelot number.
        for (minor, major) in [
            ("Am", "C"),
            ("Em", "G"),
            ("Bm", "D"),
            ("F#m", "A"),
            ("C#m", "E"),
            ("G#m", "B"),
            ("D#m", "F#"),
            ("Bbm", "Db"),
            ("Fm", "Ab"),
            ("Cm", "Eb"),
            ("Gm", "Bb"),
            ("Dm", "F"),
        ] {
            let m = to_camelot(minor).unwrap();
            let j = to_camelot(major).unwrap();
            assert_eq!(m.number, j.number, "{minor} vs {major}");
            assert!(m.minor && !j.minor);
        }
    }

    #[test]
    fn compatible_keys_are_the_neighbours_and_the_relative() {
        let mut got = compatible_keys(Some("8A"));
        got.sort();
        assert_eq!(got, ["7A", "8A", "8B", "9A"]);
    }

    #[test]
    fn compatible_keys_wrap_around_the_wheel() {
        let mut got = compatible_keys(Some("1A"));
        got.sort();
        assert_eq!(got, ["12A", "1A", "1B", "2A"], "1 wraps down to 12");

        let mut got = compatible_keys(Some("12B"));
        got.sort();
        assert_eq!(got, ["11B", "12A", "12B", "1B"], "12 wraps up to 1");
    }

    #[test]
    fn compatible_keys_are_empty_without_a_usable_tag() {
        assert!(compatible_keys(None).is_empty());
        assert!(compatible_keys(Some("gibberish")).is_empty());
    }

    #[test]
    fn bpm_windows_cover_half_and_double_time() {
        let windows = bpm_windows(100.0, TIGHT_TOLERANCE);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], BpmRange { min: 94.0, max: 106.0 });
        assert_eq!(windows[1], BpmRange { min: 47.0, max: 53.0 }, "half time");
        assert_eq!(windows[2], BpmRange { min: 188.0, max: 212.0 }, "double time");
    }

    #[test]
    fn bpm_windows_drop_implausible_centers() {
        // Half of 70 is 35 — below anything a track is actually tagged at, so
        // a slow seed only gets its own window and the double-time one.
        let windows = bpm_windows(70.0, TIGHT_TOLERANCE);
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().all(|w| w.min > BPM_FLOOR));

        // Symmetrically, a fast seed loses double-time: 120 → 240 is past
        // anything real libraries tag, while 60 is perfectly ordinary.
        let windows = bpm_windows(120.0, TIGHT_TOLERANCE);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1], BpmRange { min: 56.4, max: 63.6 }, "half time survives");
        assert_eq!(bpm_windows(170.0, TIGHT_TOLERANCE).len(), 2);
    }

    #[test]
    fn wide_windows_are_wider_than_tight_ones() {
        let tight = bpm_windows(128.0, TIGHT_TOLERANCE)[0];
        let wide = bpm_windows(128.0, WIDE_TOLERANCE)[0];
        assert!(wide.min < tight.min && wide.max > tight.max);
    }

    /// A seed with the tags a well-kept library has.
    fn seed(bpm: Option<u32>, key: Option<&str>) -> Track {
        Track {
            filepath: "lib/seed.mp3".into(),
            metadata: crate::api::types::TrackMetadata {
                artist: Some("Seed Artist".into()),
                bpm,
                musical_key: key.map(str::to_string),
                ..Default::default()
            },
        }
    }

    /// The request as JSON — what the server actually receives, which is the
    /// only place absent-vs-empty is visible.
    fn body(request: &RandomSongRequest) -> serde_json::Value {
        serde_json::to_value(request).unwrap()
    }

    #[test]
    fn a_default_session_asks_for_tempo_and_key_around_the_seed() {
        let settings = Settings::default();
        let (request, note) = build_random_request(
            &settings,
            Some(&seed(Some(128), Some("A minor"))),
            vec![3, 4],
            &[],
            &[],
            false,
        );
        assert!(note.is_none(), "nothing to explain when the tags are there");

        let body = body(&request);
        assert_eq!(body["ignoreList"], serde_json::json!([3, 4]));
        assert_eq!(body["musicalKeys"], serde_json::json!(["8A", "7A", "9A", "8B"]));
        // Tight windows, plus the wider set the server relaxes to first.
        assert_eq!(body["bpmRanges"][0]["min"], 120.3);
        assert_eq!(body["bpmRangesWide"][0]["min"], 112.6);
        // Nothing else was asked for, so nothing else is sent.
        for absent in ["minRating", "genres", "genreMode", "similarTo", "minSimilarity"] {
            assert!(body.get(absent).is_none(), "{absent} should be absent: {body}");
        }
    }

    #[test]
    fn switching_a_filter_off_removes_it_rather_than_sending_an_empty_one() {
        // The server branches on field presence — an empty list is not the
        // same as no list, and would put the request in continuity mode.
        let settings = Settings {
            tempo_tolerance: 0,
            key_matching: KeyMatching::Off,
            min_rating: 0,
            artist_cooldown: 0,
            genre_mode: GenreMode::Off,
            genres: vec!["Ambient".into()],
            ..Settings::default()
        };
        let (request, note) = build_random_request(
            &settings,
            Some(&seed(Some(128), Some("A minor"))),
            Vec::new(),
            &["lib/a.mp3".into()],
            &["Someone".into()],
            true,
        );
        assert_eq!(body(&request), serde_json::json!({}), "an unfiltered pick asks for nothing");
        assert!(note.is_none(), "not matching on tags was the instruction, not a surprise");
    }

    #[test]
    fn an_untagged_seed_says_so_when_matching_was_wanted() {
        let (request, note) =
            build_random_request(&Settings::default(), Some(&seed(None, None)), Vec::new(), &[], &[], false);
        assert!(request.bpm_ranges.is_empty() && request.musical_keys.is_empty());
        let note = note.unwrap();
        assert!(note.contains("no tempo or key tags"));
        assert!(note.ends_with("picking at random"), "got: {note}");
    }

    #[test]
    fn an_untagged_seed_inside_a_sonic_pool_is_not_called_random() {
        // Found live: the pick was confined to 37 tracks that sound like the
        // seed, and the player still called it random.
        let settings = Settings { sonic_tightness: 40, ..Settings::default() };
        let (request, note) = build_random_request(
            &settings,
            Some(&seed(None, None)),
            Vec::new(),
            &["lib/seed.mp3".into()],
            &[],
            true,
        );
        assert!(request.min_similarity.is_some());
        assert!(note.unwrap().ends_with("picking from the sonic pool"));
    }

    #[test]
    fn the_sonic_pool_follows_the_anchor_setting() {
        let anchors: Vec<String> =
            (0..12).map(|i| format!("lib/{i}.mp3")).collect();
        let tight = Settings { sonic_tightness: 50, ..Settings::default() };

        // Session: the whole rolling window, trimmed to what the server averages.
        let (request, _) = build_random_request(
            &Settings { sonic_anchor: SonicAnchor::Session, ..tight.clone() },
            None,
            Vec::new(),
            &anchors,
            &[],
            true,
        );
        assert_eq!(request.similar_to.len(), 8);
        assert!(request.min_similarity.is_some());

        // Current: only what is playing right now.
        let (request, _) = build_random_request(
            &Settings { sonic_anchor: SonicAnchor::Current, ..tight },
            None,
            Vec::new(),
            &anchors,
            &[],
            true,
        );
        assert_eq!(request.similar_to, vec!["lib/0.mp3"], "newest first");
    }

    #[test]
    fn the_sonic_pool_is_withheld_when_the_server_has_no_index() {
        // Sending it anyway would 403 the whole call and take the ordinary
        // tempo/key pick down with it.
        let settings = Settings { sonic_tightness: 80, ..Settings::default() };
        let (request, _) = build_random_request(
            &settings,
            Some(&seed(Some(128), Some("8A"))),
            Vec::new(),
            &["lib/a.mp3".into()],
            &[],
            false,
        );
        assert!(request.similar_to.is_empty());
        assert!(request.min_similarity.is_none());
        assert!(!request.musical_keys.is_empty(), "the rest of the pick is unaffected");
    }

    #[test]
    fn a_pool_with_nothing_to_anchor_on_is_not_half_sent() {
        // First pick of a session: tightness is set but nothing has played.
        let settings = Settings { sonic_tightness: 60, ..Settings::default() };
        let (request, _) =
            build_random_request(&settings, None, Vec::new(), &[], &[], true);
        let body = body(&request);
        assert!(body.get("similarTo").is_none(), "got {body}");
        assert!(body.get("minSimilarity").is_none(), "half a pool is a 400");
    }

    #[test]
    fn the_cooldown_sends_only_as_many_artists_as_asked_for() {
        let recent: Vec<String> = (0..10).map(|i| format!("Artist {i}")).collect();
        let settings = Settings { artist_cooldown: 3, ..Settings::default() };
        let (request, _) =
            build_random_request(&settings, None, Vec::new(), &[], &recent, false);
        assert_eq!(request.ignore_artists, vec!["Artist 0", "Artist 1", "Artist 2"]);

        let settings = Settings { artist_cooldown: 0, ..Settings::default() };
        let (request, _) =
            build_random_request(&settings, None, Vec::new(), &[], &recent, false);
        assert!(request.ignore_artists.is_empty(), "off means the field is gone");
    }

    #[test]
    fn genres_and_rating_ride_along_when_set() {
        let settings = Settings {
            genre_mode: GenreMode::Blacklist,
            genres: vec!["Spoken Word".into()],
            min_rating: 4,
            ..Settings::default()
        };
        let (request, _) = build_random_request(&settings, None, Vec::new(), &[], &[], false);
        let body = body(&request);
        assert_eq!(body["genres"], serde_json::json!(["Spoken Word"]));
        assert_eq!(body["genreMode"], "blacklist");
        assert_eq!(body["minRating"], 4);
    }

    #[test]
    fn a_genre_mode_with_no_genres_chosen_filters_nothing() {
        let settings =
            Settings { genre_mode: GenreMode::Whitelist, genres: Vec::new(), ..Settings::default() };
        let (request, _) = build_random_request(&settings, None, Vec::new(), &[], &[], false);
        let body = body(&request);
        assert!(body.get("genres").is_none());
        assert!(body.get("genreMode").is_none(), "a mode alone would filter everything out");
    }

    #[test]
    fn strict_key_matching_narrows_what_gets_asked_for() {
        let compatible = build_random_request(
            &Settings { key_matching: KeyMatching::Compatible, ..Settings::default() },
            Some(&seed(None, Some("A minor"))),
            Vec::new(),
            &[],
            &[],
            false,
        )
        .0;
        let strict = build_random_request(
            &Settings { key_matching: KeyMatching::Strict, ..Settings::default() },
            Some(&seed(None, Some("A minor"))),
            Vec::new(),
            &[],
            &[],
            false,
        )
        .0;
        assert_eq!(compatible.musical_keys.len(), 4);
        assert_eq!(strict.musical_keys, vec!["8A"]);
    }

    #[test]
    fn settings_survive_a_round_trip_through_the_config_file() {
        let settings = Settings {
            tempo_tolerance: 9,
            key_matching: KeyMatching::Strict,
            min_rating: 7,
            artist_cooldown: 5,
            sonic_tightness: 45,
            sonic_anchor: SonicAnchor::Current,
            genre_mode: GenreMode::Blacklist,
            genres: vec!["Ambient".into()],
        };
        assert_eq!(Settings::from_prefs(&settings.to_prefs()), settings);

        // A value from a newer player, or a typo in a hand-edited file, falls
        // back rather than refusing to load.
        let prefs = crate::config::AutoDjPrefs {
            key_matching: "quantum".into(),
            genre_mode: "maybe".into(),
            tempo_tolerance: 9000,
            min_rating: 99,
            ..Default::default()
        };
        let settings = Settings::from_prefs(&prefs);
        assert_eq!(settings.key_matching, KeyMatching::Compatible);
        assert_eq!(settings.genre_mode, GenreMode::Off);
        assert_eq!(settings.tempo_tolerance, TEMPO_TOLERANCE_MAX, "clamped, not wild");
        assert_eq!(settings.min_rating, RATING_MAX);
    }

    #[test]
    fn strict_key_matching_keeps_only_the_seeds_own_key() {
        assert_eq!(exact_keys(Some("A minor")), vec!["8A"]);
        assert_eq!(compatible_keys(Some("A minor")).len(), 4, "the wheel neighbourhood");
        assert!(exact_keys(Some("gibberish")).is_empty());
        assert!(exact_keys(None).is_empty());
    }

    #[test]
    fn the_tightness_slider_spans_the_band_the_embeddings_actually_use() {
        // Zero is off, and off must produce no threshold at all: the server
        // rejects minSimilarity without similarTo and vice versa.
        assert_eq!(sonic_threshold(0), None);

        assert_eq!(sonic_threshold(1), Some(SONIC_LOOSEST));
        assert_eq!(sonic_threshold(100), Some(SONIC_TIGHTEST));
        // Monotonic, and inside the band the whole way.
        let mut previous = 0.0;
        for t in 1..=100 {
            let v = sonic_threshold(t).unwrap();
            assert!(v >= previous, "tightness {t} went backwards");
            assert!((SONIC_LOOSEST..=SONIC_TIGHTEST).contains(&v), "tightness {t} → {v}");
            previous = v;
        }
        // Out-of-range input is clamped rather than extrapolated past the band.
        assert_eq!(sonic_threshold(500), Some(SONIC_TIGHTEST));
    }

    #[test]
    fn bpm_windows_reject_nonsense_input() {
        assert!(bpm_windows(0.0, TIGHT_TOLERANCE).is_empty());
        assert!(bpm_windows(-5.0, TIGHT_TOLERANCE).is_empty());
        assert!(bpm_windows(f64::NAN, TIGHT_TOLERANCE).is_empty());
    }
}
