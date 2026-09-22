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

// ── Settings ────────────────────────────────────────────────────────────────

/// What the sonic pool measures distance from (auto-dj contract, clause 23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SonicAnchor {
    /// "Follow the vibe": the last few DJ picks, most recent last, so the
    /// session's sound may slowly evolve.
    #[default]
    Rolling,
    /// "Stay on seed": one pin, set on the lane's first pick and reused for
    /// the whole session.
    Locked,
}

/// What switching the DJ on with nothing queued does (clause 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmptyQueueStart {
    #[default]
    Ask,
    /// "Surprise me": the filtered opener, then followers.
    Random,
    /// "Let me choose": the library, under a banner.
    Pick,
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
impl SonicAnchor {
    pub fn label(self) -> &'static str {
        match self {
            SonicAnchor::Rolling => "rolling",
            SonicAnchor::Locked => "locked",
        }
    }
    /// The player's old spellings ("current", "session") both meant a
    /// moving anchor; they read as rolling.
    pub fn from_label(raw: &str) -> Self {
        match raw {
            "locked" => SonicAnchor::Locked,
            _ => SonicAnchor::Rolling,
        }
    }
    pub fn next(self) -> Self {
        match self {
            SonicAnchor::Rolling => SonicAnchor::Locked,
            SonicAnchor::Locked => SonicAnchor::Rolling,
        }
    }
}

impl EmptyQueueStart {
    pub fn label(self) -> &'static str {
        match self {
            EmptyQueueStart::Ask => "ask",
            EmptyQueueStart::Random => "random",
            EmptyQueueStart::Pick => "pick",
        }
    }
    pub fn from_label(raw: &str) -> Self {
        match raw {
            "random" => EmptyQueueStart::Random,
            "pick" => EmptyQueueStart::Pick,
            _ => EmptyQueueStart::Ask,
        }
    }
    pub fn next(self) -> Self {
        match self {
            EmptyQueueStart::Ask => EmptyQueueStart::Random,
            EmptyQueueStart::Random => EmptyQueueStart::Pick,
            EmptyQueueStart::Pick => EmptyQueueStart::Ask,
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

/// Everything Auto DJ's room controls that travels with the user (contract
/// clause 51) — plus the library rules that stand behind a server entry
/// carrying none of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub songs_per_fetch: u32,
    pub sonic: bool,
    /// The raw cosine floor, [`SONIC_MIN_SIMILARITY`]..=[`SONIC_MAX_SIMILARITY`].
    pub sonic_min_similarity: f64,
    pub sonic_anchor: SonicAnchor,
    pub empty_queue: EmptyQueueStart,
    pub bpm: bool,
    /// ± BPM around the playing track, 1–20.
    pub bpm_tolerance: u32,
    pub harmonic: bool,
    /// How many recently-played artists to keep out; zero is off.
    pub artist_cooldown: u32,
    pub length: bool,
    /// Seconds; 0 and [`LENGTH_RAIL_SECONDS`] are the rails and are not sent.
    pub min_seconds: u32,
    pub max_seconds: u32,
    pub allow_unknown_length: bool,
    pub keyword_filter: bool,
    pub keywords: Vec<String>,
    /// The library fallbacks: 1–10 or zero for no floor; the genre filter.
    pub min_rating: u32,
    pub genre_mode: GenreMode,
    pub genres: Vec<String>,
}

/// The bounds the room's controls move within, and the server's own caps.
pub const SONGS_PER_FETCH_MAX: u32 = 25;
pub const DEFAULT_SONGS_PER_FETCH: u32 = 4;
pub const BPM_TOLERANCE_MIN: u32 = 1;
pub const BPM_TOLERANCE_MAX: u32 = 20;
pub const DEFAULT_BPM_TOLERANCE: u32 = 8;
/// The wide set the server relaxes to before dropping tempo altogether.
pub const BPM_WIDE_EXTRA: u32 = 2;
/// The band the embeddings actually use: the server's own calibration puts
/// same-artist pairs around .6–.9 and cross-artist ones around .3–.7.
pub const SONIC_MIN_SIMILARITY: f64 = 0.30;
pub const SONIC_MAX_SIMILARITY: f64 = 0.80;
pub const DEFAULT_SONIC_MIN_SIMILARITY: f64 = 0.55;
/// The strictness bar's step.
pub const SONIC_STEP: f64 = 0.05;
pub const LENGTH_RAIL_SECONDS: u32 = 1200;
pub const LENGTH_STEP_SECONDS: u32 = 15;
pub const KEYWORDS_MAX: usize = 50;
/// The genre picker's cap (clause 48).
pub const GENRES_MAX: usize = 200;
pub const RATING_MAX: u32 = 10;
pub const ARTIST_COOLDOWN_MAX: u32 = 20;
/// How many DJ picks the rolling anchor remembers (clause 23).
pub const HISTORY_LEN: usize = 5;

impl Default for Settings {
    fn default() -> Self {
        Settings::from_prefs(&crate::config::AutoDjPrefs::default())
    }
}

/// A cosine floor rounded to the bar's step and held inside the band.
pub fn clamp_similarity(raw: f64) -> f64 {
    let raw = if raw.is_finite() { raw } else { DEFAULT_SONIC_MIN_SIMILARITY };
    ((raw.clamp(SONIC_MIN_SIMILARITY, SONIC_MAX_SIMILARITY) / SONIC_STEP).round() * SONIC_STEP * 100.0)
        .round()
        / 100.0
}

impl Settings {
    /// From the file — clamped, and migrated from the three-mode panel's
    /// keys when they are still there: a tempo percent above zero means
    /// BPM continuity was wanted, a key matching other than off means
    /// harmonic mixing was, and the perceptual slider is let go (the
    /// record's model is a switch and a raw band).
    pub fn from_prefs(prefs: &crate::config::AutoDjPrefs) -> Self {
        let bpm = prefs.bpm || prefs.tempo_tolerance.is_some_and(|t| t > 0);
        let harmonic = prefs.harmonic || prefs.key_matching.as_deref().is_some_and(|k| k != "off");
        let max_seconds = prefs.max_seconds.clamp(0, LENGTH_RAIL_SECONDS);
        Settings {
            songs_per_fetch: prefs.songs_per_fetch.clamp(1, SONGS_PER_FETCH_MAX),
            sonic: prefs.sonic,
            sonic_min_similarity: clamp_similarity(prefs.sonic_min_similarity),
            sonic_anchor: SonicAnchor::from_label(&prefs.sonic_anchor),
            empty_queue: EmptyQueueStart::from_label(&prefs.empty_queue),
            bpm,
            bpm_tolerance: prefs.bpm_tolerance.clamp(BPM_TOLERANCE_MIN, BPM_TOLERANCE_MAX),
            harmonic,
            artist_cooldown: prefs.artist_cooldown.min(ARTIST_COOLDOWN_MAX),
            length: prefs.length,
            min_seconds: prefs.min_seconds.min(max_seconds),
            max_seconds,
            allow_unknown_length: prefs.allow_unknown_length,
            keyword_filter: prefs.keyword_filter,
            keywords: prefs
                .keywords
                .iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .take(KEYWORDS_MAX)
                .collect(),
            min_rating: prefs.min_rating.min(RATING_MAX),
            genre_mode: GenreMode::from_label(&prefs.genre_mode),
            genres: prefs.genres.clone(),
        }
    }

    pub fn to_prefs(&self) -> crate::config::AutoDjPrefs {
        crate::config::AutoDjPrefs {
            songs_per_fetch: self.songs_per_fetch,
            sonic: self.sonic,
            sonic_min_similarity: self.sonic_min_similarity,
            sonic_anchor: self.sonic_anchor.label().to_string(),
            empty_queue: self.empty_queue.label().to_string(),
            bpm: self.bpm,
            bpm_tolerance: self.bpm_tolerance,
            harmonic: self.harmonic,
            artist_cooldown: self.artist_cooldown,
            length: self.length,
            min_seconds: self.min_seconds,
            max_seconds: self.max_seconds,
            allow_unknown_length: self.allow_unknown_length,
            keyword_filter: self.keyword_filter,
            keywords: self.keywords.clone(),
            min_rating: self.min_rating,
            genre_mode: self.genre_mode.label().to_string(),
            genres: self.genres.clone(),
            // Only the room's own settings are here; whatever a newer
            // player wrote alongside them is held by the loaded prefs and
            // put back by `PlayerPrefs::adopt`. The legacy keys stay gone.
            ..Default::default()
        }
    }

    /// The length window's words (clause 47): a rail means unbounded, so a
    /// bare 0:00–20:00 never looks like a constraint.
    pub fn length_words(&self) -> String {
        let fmt = |s: u32| format!("{}:{:02}", s / 60, s % 60);
        match (self.min_seconds > 0, self.max_seconds < LENGTH_RAIL_SECONDS) {
            (false, false) => "Any length".to_string(),
            (true, false) => format!("Over {}", fmt(self.min_seconds)),
            (false, true) => format!("Under {}", fmt(self.max_seconds)),
            (true, true) => format!("{} to {}", fmt(self.min_seconds), fmt(self.max_seconds)),
        }
    }
}

// ── Composition ─────────────────────────────────────────────────────────────

use crate::api::types::{BpmWindow, RandomSongRequest, Track};

/// The rules of the DJ's library, resolved from its server entry with the
/// session-wide settings behind it (clause 51), and what the server is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryFilters {
    /// The libraries switched OFF (`ignoreVPaths`).
    pub sources_off: Vec<String>,
    /// 1–10, or zero for no floor.
    pub min_rating: u32,
    pub genre_mode: GenreMode,
    pub genres: Vec<String>,
    /// A federated peer: a key or a guest has no stars, so no rating goes
    /// (clause 6).
    pub is_peer: bool,
}

impl LibraryFilters {
    /// A server entry's own rules over the session-wide fallbacks.
    pub fn resolve(
        settings: &Settings,
        own: &crate::config::DjLibraryOverrides,
        is_peer: bool,
    ) -> LibraryFilters {
        LibraryFilters {
            sources_off: own.sources_off.clone(),
            min_rating: own.min_rating.unwrap_or(settings.min_rating).min(RATING_MAX),
            genre_mode: own
                .genre_mode
                .as_deref()
                .map(GenreMode::from_label)
                .unwrap_or(settings.genre_mode),
            genres: own.genres.clone().unwrap_or_else(|| settings.genres.clone()),
            is_peer,
        }
    }
}

/// Everything one pick is composed from — built by the App, sent to the
/// worker, and turned into the request body there. Pure on purpose: what
/// gets asked of the server is the part worth pinning in tests, and it
/// depends on enough moving pieces — a playing track that may lack tags,
/// a pool that may be suppressed, filters that must vanish rather than go
/// out empty — that reading the JSON is the only honest way to know.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub settings: Settings,
    pub library: LibraryFilters,
    /// The playing track's tempo and key — numbers, so any server's row
    /// will do (clauses 21 and 22).
    pub playing_bpm: Option<u32>,
    pub playing_key: Option<String>,
    /// The lane's Camelot anchor, as a code, once locked.
    pub camelot_anchor: Option<String>,
    /// The sonic seeds the App's anchor rule resolved (clause 23); empty
    /// means no pool is asked for.
    pub sonic_seeds: Vec<String>,
    /// The round-trip cursor (clause 24).
    pub ignore_list: Vec<u32>,
    /// Recently-played artists, newest first, for the cooldown.
    pub recent_artists: Vec<String>,
    /// The "Surprise me" opener: the library filters and nothing else, one
    /// song (clause 3).
    pub opener: bool,
}

impl Ask {
    /// The body, filter by filter.
    pub fn request(&self) -> RandomSongRequest {
        let s = &self.settings;
        let lib = &self.library;
        let mut r = RandomSongRequest { ignore_list: self.ignore_list.clone(), ..Default::default() };

        // The library filters, shared with the opener (clause 20).
        r.ignore_vpaths = lib.sources_off.clone();
        if lib.min_rating > 0 && !lib.is_peer {
            r.min_rating = Some(lib.min_rating.min(RATING_MAX));
        }
        if lib.genre_mode != GenreMode::Off && !lib.genres.is_empty() {
            r.genres = lib.genres.clone();
            r.genre_mode = Some(lib.genre_mode.label().to_string());
        }
        if s.length {
            if s.min_seconds > 0 {
                r.min_duration = Some(s.min_seconds);
            }
            if s.max_seconds < LENGTH_RAIL_SECONDS {
                r.max_duration = Some(s.max_seconds);
            }
            if (r.min_duration.is_some() || r.max_duration.is_some()) && s.allow_unknown_length {
                r.allow_unknown_duration = Some(true);
            }
        }
        if self.opener {
            return r;
        }

        // The batch (clause 27): at one the key is left off.
        if s.songs_per_fetch > 1 {
            r.limit = Some(s.songs_per_fetch.min(SONGS_PER_FETCH_MAX));
        }
        // BPM continuity (clause 21): windows only around a tagged track.
        if s.bpm && let Some(bpm) = self.playing_bpm.filter(|b| *b > 0) {
            let tolerance = f64::from(s.bpm_tolerance);
            r.bpm_ranges = bpm_windows(f64::from(bpm), tolerance);
            r.bpm_ranges_wide = bpm_windows(f64::from(bpm), tolerance + f64::from(BPM_WIDE_EXTRA));
            r.require_bpm = Some(true);
        }
        // Harmonic mixing (clause 22): the anchor's neighbourhood, and keyed
        // tracks only even before there is one, so the first pick can lock it.
        if s.harmonic {
            if let Some(anchor) = &self.camelot_anchor {
                r.musical_keys = compatible_keys(Some(anchor));
            }
            r.require_musical_key = Some(true);
        }
        // The cooldown (decision 9).
        if s.artist_cooldown > 0 {
            r.ignore_artists =
                self.recent_artists.iter().take(s.artist_cooldown as usize).cloned().collect();
        }
        // The pool (clause 23): both keys or neither.
        let threshold =
            (!self.sonic_seeds.is_empty()).then(|| clamp_similarity(s.sonic_min_similarity));
        r.with_sonic_pool(&self.sonic_seeds, threshold)
    }

    /// Whether this ask carries a sonic pool at all.
    pub fn sonic_asked(&self) -> bool {
        !self.opener && !self.sonic_seeds.is_empty()
    }

    /// The same ask with the pool let go — the degrade of clause 30.
    pub fn without_sonic(&self) -> Ask {
        Ask { sonic_seeds: Vec::new(), ..self.clone() }
    }

    /// The keyword filter (clause 26): a song whose title, artist, album or
    /// filepath contains any word, case-insensitively, is refused.
    pub fn keyword_blocked(&self, track: &Track) -> bool {
        if !self.settings.keyword_filter || self.settings.keywords.is_empty() {
            return false;
        }
        let haystack = [
            track.metadata.title.as_deref().unwrap_or_default(),
            track.metadata.artist.as_deref().unwrap_or_default(),
            track.metadata.album.as_deref().unwrap_or_default(),
            track.filepath.as_str(),
        ]
        .join("\n")
        .to_lowercase();
        self.settings
            .keywords
            .iter()
            .map(|k| k.trim().to_lowercase())
            .filter(|k| !k.is_empty())
            .any(|k| haystack.contains(&k))
    }
}

/// Plausible tempo for a music track. Half/double-time windows outside this
/// are dropped rather than sent as noise.
const BPM_FLOOR: f64 = 40.0;
const BPM_CEILING: f64 = 220.0;

/// Windows of ± `tolerance` BPM around a tempo at the same, half and
/// double time (clause 21). Sending all three is what the server's docs
/// recommend: a 140 BPM track mixes naturally into 70 BPM, and matching
/// only the literal number would reject those.
pub fn bpm_windows(bpm: f64, tolerance: f64) -> Vec<BpmWindow> {
    if !bpm.is_finite() || bpm <= 0.0 || !tolerance.is_finite() || tolerance < 0.0 {
        return Vec::new();
    }
    [bpm, bpm / 2.0, bpm * 2.0]
        .into_iter()
        .filter(|center| (BPM_FLOOR..=BPM_CEILING).contains(center))
        .map(|center| BpmWindow {
            min: ((center - tolerance).max(0.0) * 10.0).round() / 10.0,
            max: ((center + tolerance) * 10.0).round() / 10.0,
        })
        .collect()
}

/// A version string against a floor (clause 50): `Some(true)` when the
/// server is at least this new, `Some(false)` when it is KNOWN to be older,
/// `None` when the string does not read as a version — a fork, a dev build
/// — which keeps every control and lets the learner decide.
pub fn version_at_least(version: &str, floor: (u32, u32, u32)) -> Option<bool> {
    let digits: Vec<u32> = version
        .trim()
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .take_while(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    if digits.len() < 2 {
        return None;
    }
    let got = (digits[0], digits[1], digits.get(2).copied().unwrap_or(0));
    Some(got >= floor)
}

/// The record's version floors: the BPM / key / genre block, the length
/// window, and batch picks.
pub const FLOOR_FILTERS: (u32, u32, u32) = (6, 7, 1);
pub const FLOOR_SONIC: (u32, u32, u32) = (6, 15, 2);
pub const FLOOR_LENGTH: (u32, u32, u32) = (6, 25, 0);
pub const FLOOR_BATCH: (u32, u32, u32) = (6, 26, 0);

/// True when a server is KNOWN to predate `floor` — an unknown version
/// hides nothing.
pub fn known_older(version: Option<&str>, floor: (u32, u32, u32)) -> bool {
    version.and_then(|v| version_at_least(v, floor)) == Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::TrackMetadata;

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
        // ± 8 BPM at each centre (clause 21).
        let windows = bpm_windows(100.0, 8.0);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0], BpmWindow { min: 92.0, max: 108.0 });
        assert_eq!(windows[1], BpmWindow { min: 42.0, max: 58.0 }, "half time");
        assert_eq!(windows[2], BpmWindow { min: 192.0, max: 208.0 }, "double time");
    }

    #[test]
    fn bpm_windows_drop_implausible_centers() {
        // Half of 70 is 35 — below anything a track is actually tagged at, so
        // a slow seed only gets its own window and the double-time one.
        let windows = bpm_windows(70.0, 8.0);
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().all(|w| w.min > 30.0));
        // Symmetrically, a fast seed loses double-time.
        let windows = bpm_windows(120.0, 8.0);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1], BpmWindow { min: 52.0, max: 68.0 }, "half time survives");
    }

    #[test]
    fn the_wide_set_is_two_bpm_wider() {
        let tight = bpm_windows(128.0, 8.0)[0];
        let wide = bpm_windows(128.0, 10.0)[0];
        assert_eq!((wide.min, wide.max), (tight.min - 2.0, tight.max + 2.0));
    }

    #[test]
    fn bpm_windows_reject_nonsense_input() {
        assert!(bpm_windows(0.0, 8.0).is_empty());
        assert!(bpm_windows(-5.0, 8.0).is_empty());
        assert!(bpm_windows(f64::NAN, 8.0).is_empty());
        assert!(bpm_windows(120.0, -1.0).is_empty());
    }

    // ── Composition ────────────────────────────────────────────────────────

    fn ask(settings: Settings) -> Ask {
        Ask {
            settings,
            library: LibraryFilters::default(),
            playing_bpm: Some(120),
            playing_key: Some("Am".into()),
            camelot_anchor: None,
            sonic_seeds: Vec::new(),
            ignore_list: vec![4, 5],
            recent_artists: vec!["Alpha".into(), "Beta".into(), "Gamma".into()],
            opener: false,
        }
    }

    fn json(ask: &Ask) -> serde_json::Value {
        serde_json::to_value(ask.request()).unwrap()
    }

    #[test]
    fn a_fresh_install_asks_for_a_batch_and_nothing_it_cannot_anchor() {
        // Defaults: sonic on but no seeds yet (a cold start stays plain
        // random), BPM and harmonic off, cooldown three, four songs.
        let body = json(&ask(Settings::default()));
        assert_eq!(body["limit"], 4);
        assert_eq!(body["ignoreList"], serde_json::json!([4, 5]));
        assert_eq!(body["ignoreArtists"], serde_json::json!(["Alpha", "Beta", "Gamma"]));
        for absent in ["bpmRanges", "requireBpm", "musicalKeys", "requireMusicalKey", "similarTo",
                       "minSimilarity", "minRating", "genres", "minDuration", "maxDuration",
                       "allowUnknownDuration", "ignoreVPaths"] {
            assert!(body.get(absent).is_none(), "{absent} went out: {body}");
        }
    }

    #[test]
    fn continuity_rides_the_playing_tracks_tags_and_requires_them() {
        let mut s = Settings::default();
        s.bpm = true;
        s.harmonic = true;
        let mut a = ask(s);
        let body = json(&a);
        assert_eq!(body["bpmRanges"].as_array().unwrap().len(), 2, "120: same and half time");
        assert_eq!(body["bpmRanges"][0]["min"], 112.0);
        assert_eq!(body["bpmRangesWide"][0]["min"], 110.0, "the wide set is + 2");
        assert_eq!(body["requireBpm"], true);
        // No anchor yet: keyed tracks only, so the first pick can lock one.
        assert!(body.get("musicalKeys").is_none());
        assert_eq!(body["requireMusicalKey"], true);

        a.camelot_anchor = Some("8A".into());
        let body = json(&a);
        let mut keys: Vec<String> =
            body["musicalKeys"].as_array().unwrap().iter().map(|k| k.as_str().unwrap().into()).collect();
        keys.sort();
        assert_eq!(keys, ["7A", "8A", "8B", "9A"], "the anchor's neighbourhood");

        // An untagged playing track sends no windows, and no requirement.
        a.playing_bpm = None;
        let body = json(&a);
        assert!(body.get("bpmRanges").is_none() && body.get("requireBpm").is_none());
    }

    #[test]
    fn the_pool_goes_out_whole_or_not_at_all() {
        let mut a = ask(Settings::default());
        a.sonic_seeds = vec!["lib/a.mp3".into(), "lib/b.mp3".into()];
        let body = json(&a);
        assert_eq!(body["similarTo"], serde_json::json!(["lib/a.mp3", "lib/b.mp3"]));
        assert_eq!(body["minSimilarity"], 0.55);
        assert!(a.sonic_asked());

        let relaxed = a.without_sonic();
        let body = json(&relaxed);
        assert!(body.get("similarTo").is_none() && body.get("minSimilarity").is_none());
        assert!(!relaxed.sonic_asked());

        // The floor rides the band's step and stays inside it.
        a.settings.sonic_min_similarity = 0.93;
        assert_eq!(json(&a)["minSimilarity"], 0.8);
        a.settings.sonic_min_similarity = 0.12;
        assert_eq!(json(&a)["minSimilarity"], 0.3);
    }

    #[test]
    fn the_library_filters_are_the_openers_whole_body() {
        let mut s = Settings::default();
        s.bpm = true;
        s.length = true;
        s.min_seconds = 90;
        s.max_seconds = LENGTH_RAIL_SECONDS;
        s.allow_unknown_length = true;
        let mut a = ask(s);
        a.library = LibraryFilters {
            sources_off: vec!["Audiobooks".into()],
            min_rating: 6,
            genre_mode: GenreMode::Blacklist,
            genres: vec!["Podcast".into()],
            is_peer: false,
        };
        a.sonic_seeds = vec!["lib/a.mp3".into()];
        a.opener = true;
        let body = json(&a);
        assert_eq!(body["ignoreVPaths"], serde_json::json!(["Audiobooks"]));
        assert_eq!(body["minRating"], 6);
        assert_eq!(body["genres"], serde_json::json!(["Podcast"]));
        assert_eq!(body["genreMode"], "blacklist");
        assert_eq!(body["minDuration"], 90);
        assert!(body.get("maxDuration").is_none(), "the rail is not a bound");
        assert_eq!(body["allowUnknownDuration"], true, "beside a real bound");
        for absent in ["limit", "bpmRanges", "requireBpm", "similarTo", "ignoreArtists"] {
            assert!(body.get(absent).is_none(), "the opener carries no {absent}: {body}");
        }
        assert!(!a.sonic_asked(), "an opener never asks for the pool");

        // The same filters on a pick, less the rating for a peer.
        a.opener = false;
        a.library.is_peer = true;
        let body = json(&a);
        assert!(body.get("minRating").is_none(), "a key has no stars");
        assert_eq!(body["limit"], 4);
        assert_eq!(body["minDuration"], 90);
    }

    #[test]
    fn rails_and_switched_off_filters_send_nothing() {
        let mut s = Settings::default();
        s.length = true; // both bounds on their rails
        s.allow_unknown_length = true;
        s.songs_per_fetch = 1;
        s.artist_cooldown = 0;
        s.genre_mode = GenreMode::Whitelist; // but no genres chosen
        let body = json(&ask(s));
        for absent in ["minDuration", "maxDuration", "allowUnknownDuration", "limit",
                       "ignoreArtists", "genres", "genreMode"] {
            assert!(body.get(absent).is_none(), "{absent} went out: {body}");
        }
    }

    #[test]
    fn the_cooldown_sends_only_as_many_artists_as_asked_for() {
        let mut s = Settings::default();
        s.artist_cooldown = 2;
        let body = json(&ask(s));
        assert_eq!(body["ignoreArtists"], serde_json::json!(["Alpha", "Beta"]));
    }

    #[test]
    fn a_servers_own_rules_stand_over_the_fallbacks() {
        let mut s = Settings::default();
        s.min_rating = 4;
        s.genre_mode = GenreMode::Whitelist;
        s.genres = vec!["Jazz".into()];
        let own = crate::config::DjLibraryOverrides {
            sources_off: vec!["Kids".into()],
            min_rating: Some(8),
            genre_mode: None,
            genres: None,
        };
        let lib = LibraryFilters::resolve(&s, &own, false);
        assert_eq!(lib.sources_off, vec!["Kids"]);
        assert_eq!(lib.min_rating, 8, "the entry's own floor");
        assert_eq!(lib.genre_mode, GenreMode::Whitelist, "the fallback where it set none");
        assert_eq!(lib.genres, vec!["Jazz"]);
    }

    #[test]
    fn the_keyword_filter_reads_every_field_case_blind() {
        let mut s = Settings::default();
        s.keyword_filter = true;
        s.keywords = vec!["LIVE".into(), " remix ".into(), "".into()];
        let a = ask(s);
        let track = |title: &str, path: &str| Track {
            filepath: path.into(),
            metadata: TrackMetadata { title: Some(title.into()), ..Default::default() },
        };
        assert!(a.keyword_blocked(&track("Song (Live at Roxy)", "lib/x.mp3")));
        assert!(a.keyword_blocked(&track("Song", "lib/Remixes/x.mp3")));
        assert!(!a.keyword_blocked(&track("Song", "lib/x.mp3")));
        let mut off = a.clone();
        off.settings.keyword_filter = false;
        assert!(!off.keyword_blocked(&track("Song (Live)", "lib/x.mp3")), "the switch is the switch");
    }

    #[test]
    fn settings_survive_a_round_trip_and_the_old_panel_migrates() {
        let mut s = Settings::default();
        s.songs_per_fetch = 7;
        s.sonic_anchor = SonicAnchor::Locked;
        s.empty_queue = EmptyQueueStart::Random;
        s.bpm = true;
        s.bpm_tolerance = 12;
        s.length = true;
        s.min_seconds = 60;
        s.max_seconds = 600;
        s.keyword_filter = true;
        s.keywords = vec!["live".into()];
        let back = Settings::from_prefs(&s.to_prefs());
        assert_eq!(back, s);
        assert!(s.to_prefs().tempo_tolerance.is_none(), "the legacy keys are never written");

        // The three-mode panel's file: a tempo percent and a key matching
        // both mean the switches were on; the slider is let go.
        let legacy = crate::config::AutoDjPrefs {
            tempo_tolerance: Some(6),
            key_matching: Some("compatible".into()),
            sonic_tightness: Some(40),
            ..Default::default()
        };
        let migrated = Settings::from_prefs(&legacy);
        assert!(migrated.bpm && migrated.harmonic);
        assert_eq!(migrated.bpm_tolerance, DEFAULT_BPM_TOLERANCE, "a percent cannot be a BPM");
        assert!(migrated.sonic, "the record's default stands");
        let off = crate::config::AutoDjPrefs {
            tempo_tolerance: Some(0),
            key_matching: Some("off".into()),
            ..Default::default()
        };
        let migrated = Settings::from_prefs(&off);
        assert!(!migrated.bpm && !migrated.harmonic);
    }

    #[test]
    fn hand_edited_values_are_clamped_not_refused() {
        let prefs = crate::config::AutoDjPrefs {
            songs_per_fetch: 99,
            bpm_tolerance: 0,
            sonic_min_similarity: f64::NAN,
            min_seconds: 5000,
            max_seconds: 9000,
            min_rating: 40,
            sonic_anchor: "sideways".into(),
            ..Default::default()
        };
        let s = Settings::from_prefs(&prefs);
        assert_eq!(s.songs_per_fetch, SONGS_PER_FETCH_MAX);
        assert_eq!(s.bpm_tolerance, BPM_TOLERANCE_MIN);
        assert_eq!(s.sonic_min_similarity, DEFAULT_SONIC_MIN_SIMILARITY);
        assert_eq!(s.max_seconds, LENGTH_RAIL_SECONDS);
        assert_eq!(s.min_seconds, LENGTH_RAIL_SECONDS, "the floor cannot pass the ceiling");
        assert_eq!(s.min_rating, RATING_MAX);
        assert_eq!(s.sonic_anchor, SonicAnchor::Rolling);
    }

    #[test]
    fn the_length_window_reads_in_words() {
        let mut s = Settings::default();
        s.length = true;
        assert_eq!(s.length_words(), "Any length");
        s.min_seconds = 90;
        assert_eq!(s.length_words(), "Over 1:30");
        s.max_seconds = 480;
        assert_eq!(s.length_words(), "1:30 to 8:00");
        s.min_seconds = 0;
        assert_eq!(s.length_words(), "Under 8:00");
    }

    #[test]
    fn version_floors_hide_only_what_is_known_to_be_older() {
        assert_eq!(version_at_least("6.28.0", FLOOR_BATCH), Some(true));
        assert_eq!(version_at_least("6.25.0", FLOOR_BATCH), Some(false));
        assert_eq!(version_at_least("v6.26.0-beta", FLOOR_BATCH), Some(true));
        assert_eq!(version_at_least("6.7", FLOOR_FILTERS), Some(false), "6.7 is short of 6.7.1");
        assert_eq!(version_at_least("fork", FLOOR_FILTERS), None);
        assert!(known_older(Some("6.6.0"), FLOOR_FILTERS));
        assert!(!known_older(Some("weird"), FLOOR_FILTERS), "an unknown version hides nothing");
        assert!(!known_older(None, FLOOR_FILTERS));
    }
}
