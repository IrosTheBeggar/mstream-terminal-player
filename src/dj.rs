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

    #[test]
    fn bpm_windows_reject_nonsense_input() {
        assert!(bpm_windows(0.0, TIGHT_TOLERANCE).is_empty());
        assert!(bpm_windows(-5.0, TIGHT_TOLERANCE).is_empty());
        assert!(bpm_windows(f64::NAN, TIGHT_TOLERANCE).is_empty());
    }
}
