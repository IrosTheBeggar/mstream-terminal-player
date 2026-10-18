//! The FAT timestamp and the uniform-skew rule (docs/METADATA.md 2.3.4).
//!
//! `fatTime` is the raw directory fields, `(fdate << 16) | ftime`, local
//! wall-clock time with no zone at 2 s. A PC's OS converts them to and from
//! UTC on its own, so the software reads every stamp back from the card
//! after the file is in its final place and turns what the OS says into the
//! raw fields (the per-OS recipes are the volume layer's); a stamp that came
//! back off by a whole time zone is the skew, found here, and every other
//! doubt falls to the fingerprint.

use std::collections::HashMap;

/// The stamp of a wall-clock time, its seconds halved and rounded down: 0
/// when there is none (a year outside 1980-2107, or an invalid date or
/// time).
pub(crate) fn fat_time(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> u32 {
    if !(1980..=2107).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return 0;
    }
    let date = ((year - 1980) as u32) << 9 | month << 5 | day;
    let time = hour << 11 | minute << 5 | second / 2;
    date << 16 | time
}

/// A stamp's fields: (year, month, day, hour, minute, sec2), as stored.
pub(crate) fn fat_fields(t: u32) -> (i32, u32, u32, u32, u32, u32) {
    let (date, time) = (t >> 16, t & 0xFFFF);
    (1980 + (date >> 9) as i32, date >> 5 & 15, date & 31, time >> 11, time >> 5 & 63, time & 31)
}

/// Non-zero and valid: a month of 1-12, a day within its month, an hour
/// under 24, a minute under 60, a sec2 under 30. A 0 or an invalid stamp
/// never matches by time.
pub(crate) fn fat_valid(t: u32) -> bool {
    fat_wall(t).is_some()
}

/// W: the stamp's seconds since 1980-01-01 00:00 with no time zone, for a
/// valid non-zero stamp.
pub(crate) fn fat_wall(t: u32) -> Option<i64> {
    if t == 0 {
        return None;
    }
    let (year, month, day, hour, minute, sec2) = fat_fields(t);
    let date = (1..=12).contains(&month) && day >= 1 && day <= month_days(year, month);
    if !date || hour > 23 || minute > 59 || sec2 > 29 {
        return None;
    }
    let mut days: i64 = (1980..year).map(|y| if leap(y) { 366 } else { 365 }).sum();
    days += (1..month).map(|m| month_days(year, m) as i64).sum::<i64>() + day as i64 - 1;
    Some(days * 86_400 + hour as i64 * 3600 + minute as i64 * 60 + 2 * sec2 as i64)
}

/// The stamp of a W (an odd W rounds down); 0 outside 1980-2107.
pub(crate) fn fat_from_wall(w: i64) -> u32 {
    if w < 0 {
        return 0;
    }
    let (mut days, rem) = (w / 86_400, w % 86_400);
    let mut year = 1980;
    loop {
        let len = if leap(year) { 366 } else { 365 };
        if days < len {
            break;
        }
        days -= len;
        year += 1;
        if year > 2107 {
            return 0;
        }
    }
    let mut month = 1;
    while days >= month_days(year, month) as i64 {
        days -= month_days(year, month) as i64;
        month += 1;
    }
    let rem = rem as u32;
    fat_time(year, month, days as u32 + 1, rem / 3600, rem / 60 % 60, rem % 60)
}

fn leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_days(y: i32, m: u32) -> u32 {
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if m == 2 && leap(y) { 29 } else { DAYS[m.wrapping_sub(1) as usize % 12] }
}

/// The least pairs a skew needs, and its bounds (2.3.4).
pub(crate) const SKEW_MIN_PAIRS: u32 = 8;
pub(crate) const SKEW_MAX: i64 = 86_400;
pub(crate) const SKEW_STEP: i64 = 900;

/// The uniform-skew rule over the (recorded, observed) stamps of the files
/// of one tags file whose sizes match: D, in seconds, or 0 for no skew.
///
/// A pair with a 0 or an invalid stamp on either side is left out. D is the
/// most frequent non-zero delta W(observed) − W(recorded) (ties: the
/// smaller |D|, then the negative one), and a skew only when at least 8
/// pairs have it, they are at least half of the pairs, |D| ≤ 86,400 and D is
/// a multiple of 900. Counted exactly, so the answer is the rule's whatever
/// order the pairs come in; the memory is one counter per distinct delta,
/// at most one per pair.
pub(crate) fn skew(pairs: impl IntoIterator<Item = (u32, u32)>) -> i32 {
    let mut counts: HashMap<i64, u32> = HashMap::new();
    let mut total: u64 = 0;
    for (recorded, observed) in pairs {
        let (Some(r), Some(o)) = (fat_wall(recorded), fat_wall(observed)) else { continue };
        total += 1;
        if o != r {
            *counts.entry(o - r).or_insert(0) += 1;
        }
    }
    let best = counts.iter().min_by_key(|&(d, c)| (std::cmp::Reverse(*c), d.abs(), *d));
    match best {
        Some((&d, &c))
            if c >= SKEW_MIN_PAIRS && 2 * c as u64 >= total && d.abs() <= SKEW_MAX && d % SKEW_STEP == 0 =>
        {
            d as i32
        }
        _ => 0,
    }
}

/// A file's time matches its record's: both stamps valid and non-zero, and
/// the delta 0 or the tags file's skew (when it has one).
pub(crate) fn time_matches(recorded: u32, observed: u32, skew: i32) -> bool {
    match (fat_wall(recorded), fat_wall(observed)) {
        (Some(r), Some(o)) => o == r || (skew != 0 && o - r == skew as i64),
        _ => false,
    }
}
