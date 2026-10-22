//! The number rule (docs/METADATA.md 5.3), shared by both producers so the
//! ESP32's f32, Rust's f64 and a JS `Math.round` can't disagree: a tag's
//! decimal text is read in exact decimal arithmetic, never as a float, so
//! `1.005 dB` is 101 hundredths where an f64 gives 100.

/// Trims ASCII whitespace: space, tab, LF, FF and CR (`is_ascii_whitespace`,
/// so not VT).
fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// `[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)` at `scale` fraction digits, rounded
/// half away from zero: (negative, magnitude). The magnitude stops growing
/// once it passes `limit` and reads as `limit + 1`, so a long digit string
/// can't overflow. None when the text isn't such a number.
fn decimal(s: &str, scale: usize, limit: u64) -> Option<(bool, u64)> {
    let b = s.as_bytes();
    let negative = b.first() == Some(&b'-');
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    // None once past the limit.
    let push = |m: Option<u64>, d: u64| m.and_then(|v| Some(v * 10 + d).filter(|&v| v <= limit));
    let mut m = Some(0u64);
    let (mut int_digits, mut frac_digits) = (0usize, 0usize);
    while i < b.len() && b[i].is_ascii_digit() {
        m = push(m, (b[i] - b'0') as u64);
        int_digits += 1;
        i += 1;
    }
    let mut round = false;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            if frac_digits < scale {
                m = push(m, (b[i] - b'0') as u64);
            } else if frac_digits == scale {
                round = b[i] >= b'5';
            }
            frac_digits += 1;
            i += 1;
        }
    }
    if i != b.len() || (int_digits == 0 && frac_digits == 0) {
        return None;
    }
    for _ in frac_digits..scale {
        m = push(m, 0);
    }
    if round {
        m = m.and_then(|v| Some(v + 1).filter(|&v| v <= limit));
    }
    Some((negative, m.unwrap_or(limit + 1)))
}

/// bpm10 from a BPM tag: the number rule rounded to an integer, kept only
/// from 20 to 300 (mStream's rule: round first, then the range), times 10.
pub(crate) fn bpm10(text: &str) -> Option<u16> {
    let (negative, m) = decimal(trim(text), 0, 1000)?;
    if negative && m != 0 {
        return None;
    }
    (20..=300).contains(&m).then(|| (m * 10) as u16)
}

/// A ReplayGain gain in hundredths of a dB, one trailing `dB` (any ASCII
/// case) stripped first; outside the i16 range it is absent.
pub(crate) fn gain(text: &str) -> Option<i16> {
    let mut s = trim(text);
    if s.len() >= 2 && s.as_bytes()[s.len() - 2..].eq_ignore_ascii_case(b"db") {
        s = trim(&s[..s.len() - 2]);
    }
    let (negative, m) = decimal(s, 2, 40_000)?;
    let value = if negative { -(m as i64) } else { m as i64 };
    i16::try_from(value).ok()
}

/// A ReplayGain peak in ten-thousandths, saturating at 65,535; negative is
/// absent.
pub(crate) fn peak(text: &str) -> Option<u16> {
    let (negative, m) = decimal(trim(text), 4, 65_535)?;
    if negative && m != 0 {
        return None;
    }
    Some(m.min(65_535) as u16)
}

/// An Opus R128 gain q (a Q7.8 integer at −23 LUFS) as a ReplayGain gain at
/// −18 LUFS in hundredths: `(q + 1280) × 25 / 64`, rounded half away from
/// zero, in integers (2.6.4's RG_FROM_R128).
pub(crate) fn r128_to_gain(q: i16) -> i16 {
    let x = q as i32 + 1280;
    let mag = (x.abs() * 25 + 32) / 64;
    let gain = if x < 0 { -mag } else { mag };
    gain as i16
}

/// An R128 tag's text: an integer `[+-]?[0-9]+` in the i16 range, converted
/// by [`r128_to_gain`].
pub(crate) fn r128(text: &str) -> Option<i16> {
    let s = trim(text);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut m: i32 = 0;
    for c in digits.bytes() {
        if m <= 32_768 {
            m = m * 10 + (c - b'0') as i32;
        }
    }
    let q = i16::try_from(if negative { -m } else { m }).ok()?;
    Some(r128_to_gain(q))
}
