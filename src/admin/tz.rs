//! The machine's time zone, read the way `date` reads it — the TZif file
//! `/etc/localtime` points at, or the one `TZ` names — so the stats page
//! can hand the server an IANA name for its period arithmetic and put each
//! play's instant on the local clock. No zone library: the file's
//! transition table answers every instant it covers, and its POSIX footer
//! rule the years past the table (RFC 8536). Windows keeps no such file, so
//! there the zone is the one Windows is set to: its rules read year by year
//! from Windows itself, which states them as a POSIX rule does, and its IANA
//! name from CLDR's table of Windows zones (`tz/windows_zones.rs`). A
//! machine with neither — a bare container — gets UTC, and the page says so.

use std::path::{Path, PathBuf};

/// Where the zoneinfo tree lives, in the order to try: Linux and the BSDs,
/// then macOS since Catalina.
const ZONEINFO: &[&str] = &["/usr/share/zoneinfo", "/var/db/timezone/zoneinfo", "/usr/lib/zoneinfo"];

/// One zone: its IANA name, and what it does to the clock.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Zone {
    /// `America/New_York` — what the server's `tz` parameter takes.
    pub name: String,
    /// `(instant, seconds east of UTC from that instant on)`, ascending.
    transitions: Vec<(i64, i32)>,
    /// The offset before the first transition (the file's type 0).
    initial: i32,
    /// The rule for instants past the table's end.
    rule: Option<Rule>,
}

/// The zone this machine keeps time in, or `None` when nothing names one.
/// `TZ` naming a TZif file wins everywhere, as it does for `date`.
pub(crate) fn local() -> Option<Zone> {
    if let Some(tz) = std::env::var_os("TZ") {
        let tz = tz.to_string_lossy();
        let tz = tz.strip_prefix(':').unwrap_or(&tz);
        if !tz.is_empty()
            && let Some(zone) = from_name(tz)
        {
            return Some(zone);
        }
    }
    #[cfg(windows)]
    {
        windows::local()
    }
    #[cfg(not(windows))]
    {
        unix_local()
    }
}

/// The zone `/etc/localtime` links to, or the one `/etc/timezone` names.
#[cfg(not(windows))]
fn unix_local() -> Option<Zone> {
    let link = Path::new("/etc/localtime");
    if let Ok(target) = std::fs::read_link(link)
        && let Some(name) = zone_name_of(&target)
        && let Some(zone) = from_file(&name, &target)
    {
        return Some(zone);
    }
    // Debian keeps the name in a file of its own beside a copied localtime.
    if let Ok(name) = std::fs::read_to_string("/etc/timezone") {
        let name = name.trim();
        if !name.is_empty()
            && let Some(zone) = from_name(name)
        {
            return Some(zone);
        }
    }
    None
}

/// The zone called `name` — a path under a zoneinfo tree, or an absolute
/// path to a TZif file.
fn from_name(name: &str) -> Option<Zone> {
    if name.starts_with('/') {
        let path = PathBuf::from(name);
        return from_file(&zone_name_of(&path)?, &path);
    }
    if name.contains("..") {
        return None;
    }
    ZONEINFO.iter().find_map(|root| from_file(name, &Path::new(root).join(name)))
}

fn from_file(name: &str, path: &Path) -> Option<Zone> {
    let data = std::fs::read(path).ok()?;
    parse(name, &data)
}

/// `…/zoneinfo/America/New_York` → `America/New_York`.
pub(crate) fn zone_name_of(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let (_, name) = text.rsplit_once("zoneinfo/")?;
    let name = name.trim_matches('/');
    (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '+')))
        .then(|| name.to_string())
}

impl Zone {
    /// Seconds east of UTC at `t` (Unix seconds).
    pub(crate) fn offset_at(&self, t: i64) -> i32 {
        match self.transitions.binary_search_by(|(at, _)| at.cmp(&t)) {
            Ok(i) => self.past_table(i, t),
            Err(0) => self.initial,
            Err(i) => self.past_table(i - 1, t),
        }
    }

    /// The offset the transition `i` set — or the footer rule's answer once
    /// the table has run out.
    fn past_table(&self, i: usize, t: i64) -> i32 {
        if i + 1 == self.transitions.len()
            && let Some(rule) = &self.rule
        {
            return rule.offset_at(t);
        }
        self.transitions[i].1
    }
}

// ── The TZif file ─────────────────────────────────────────────────────────

fn parse(name: &str, data: &[u8]) -> Option<Zone> {
    let head = Header::read(data, 0)?;
    let (transitions, initial, footer_at) = if head.version >= b'2' {
        // Skip the 32-bit block: the 64-bit one repeats it and goes on.
        let second = 44 + head.block_len(4);
        let head2 = Header::read(data, second)?;
        let (t, i) = read_block(data, second + 44, &head2, 8)?;
        (t, i, second + 44 + head2.block_len(8))
    } else {
        let (t, i) = read_block(data, 44, &head, 4)?;
        (t, i, data.len())
    };
    let rule = data
        .get(footer_at..)
        .and_then(|rest| std::str::from_utf8(rest).ok())
        .and_then(|rest| rest.strip_prefix('\n'))
        .and_then(|rest| rest.split('\n').next())
        .filter(|s| !s.is_empty())
        .and_then(Rule::parse);
    Some(Zone { name: name.to_string(), transitions, initial, rule })
}

struct Header {
    version: u8,
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

impl Header {
    fn read(data: &[u8], at: usize) -> Option<Header> {
        let h = data.get(at..at + 44)?;
        if &h[0..4] != b"TZif" {
            return None;
        }
        let count = |i: usize| u32::from_be_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]) as usize;
        Some(Header {
            version: h[4],
            isutcnt: count(20),
            isstdcnt: count(24),
            leapcnt: count(28),
            timecnt: count(32),
            typecnt: count(36),
            charcnt: count(40),
        })
    }

    /// The data block's length for `time_size`-byte times.
    fn block_len(&self, time_size: usize) -> usize {
        self.timecnt * time_size
            + self.timecnt
            + self.typecnt * 6
            + self.charcnt
            + self.leapcnt * (time_size + 4)
            + self.isstdcnt
            + self.isutcnt
    }
}

/// The transitions (instant → offset) and the offset before the first.
fn read_block(data: &[u8], at: usize, head: &Header, time_size: usize) -> Option<(Vec<(i64, i32)>, i32)> {
    let times = data.get(at..at + head.timecnt * time_size)?;
    let idx_at = at + head.timecnt * time_size;
    let idx = data.get(idx_at..idx_at + head.timecnt)?;
    let types_at = idx_at + head.timecnt;
    let types = data.get(types_at..types_at + head.typecnt * 6)?;
    let utoff = |k: usize| -> Option<i32> {
        let b = types.get(k * 6..k * 6 + 4)?;
        Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut transitions = Vec::with_capacity(head.timecnt);
    for (n, &k) in idx.iter().enumerate() {
        let t = times.get(n * time_size..(n + 1) * time_size)?;
        let at = if time_size == 8 {
            i64::from_be_bytes([t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]])
        } else {
            i32::from_be_bytes([t[0], t[1], t[2], t[3]]) as i64
        };
        transitions.push((at, utoff(k as usize)?));
    }
    Some((transitions, utoff(0).unwrap_or(0)))
}

// ── The POSIX rule ────────────────────────────────────────────────────────

/// `EST5EDT,M3.2.0,M11.1.0`: standard time, and when daylight time holds.
#[derive(Debug, Clone, PartialEq)]
struct Rule {
    std: i32,
    dst: Option<Daylight>,
}

#[derive(Debug, Clone, PartialEq)]
struct Daylight {
    offset: i32,
    start: (Day, i32),
    end: (Day, i32),
}

/// A rule's calendar day: `Mm.w.d` (the d-th weekday of week w of month m,
/// week 5 the last), `Jn` (day n of the year, no leap day), `n` (day n
/// from zero, leap day counted).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Day {
    Month(u32, u32, u32),
    Julian1(u32),
    Julian0(u32),
}

impl Rule {
    fn parse(text: &str) -> Option<Rule> {
        let rest = skip_name(text)?;
        let (std_off, rest) = take_offset(rest)?;
        let std = -std_off;
        if rest.is_empty() {
            return Some(Rule { std, dst: None });
        }
        let rest = skip_name(rest)?;
        let (offset, rest) = match rest.chars().next() {
            Some(c) if c == ',' || c.is_ascii_digit() || c == '+' || c == '-' => {
                if c == ',' { (std + 3600, rest) } else { let (o, r) = take_offset(rest)?; (-o, r) }
            }
            None => (std + 3600, rest),
            _ => return None,
        };
        let rest = rest.strip_prefix(',')?;
        let (start, rest) = take_day(rest)?;
        let rest = rest.strip_prefix(',')?;
        let (end, rest) = take_day(rest)?;
        rest.is_empty().then_some(Rule { std, dst: Some(Daylight { offset, start, end }) })
    }

    fn offset_at(&self, t: i64) -> i32 {
        let Some(dst) = &self.dst else { return self.std };
        let (year, _, _) = civil_from_days((t + self.std as i64).div_euclid(86_400));
        let start = day_instant(year, dst.start) - self.std as i64;
        let end = day_instant(year, dst.end) - dst.offset as i64;
        let in_dst = if start <= end { start <= t && t < end } else { !(end <= t && t < start) };
        if in_dst { dst.offset } else { self.std }
    }
}

/// The wall-clock instant (as seconds from the epoch, ignoring the zone) of
/// a rule day and its time in `year`.
fn day_instant(year: i64, (day, time): (Day, i32)) -> i64 {
    let days = match day {
        Day::Month(m, w, d) => {
            let first = days_from_civil(year, m, 1);
            let first_d = (d as i64 - weekday(first) as i64).rem_euclid(7);
            let mut day = first_d + (w as i64 - 1) * 7;
            let last = days_in_month(year, m) as i64 - 1;
            while day > last {
                day -= 7;
            }
            first + day
        }
        Day::Julian1(n) => {
            let leap = is_leap(year) && n >= 60;
            days_from_civil(year, 1, 1) + n as i64 - 1 + leap as i64
        }
        Day::Julian0(n) => days_from_civil(year, 1, 1) + n as i64,
    };
    days * 86_400 + time as i64
}

/// Past a zone name: alphabetic, or anything in angle brackets.
fn skip_name(s: &str) -> Option<&str> {
    if let Some(rest) = s.strip_prefix('<') {
        let end = rest.find('>')?;
        return Some(&rest[end + 1..]);
    }
    let n = s.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    (n >= 3).then(|| &s[n..])
}

/// `[+-]hh[:mm[:ss]]` → seconds, and the rest.
fn take_offset(s: &str) -> Option<(i32, &str)> {
    let (sign, s) = match s.chars().next()? {
        '-' => (-1, &s[1..]),
        '+' => (1, &s[1..]),
        _ => (1, s),
    };
    let n = s.chars().take_while(|c| c.is_ascii_digit() || *c == ':').count();
    if n == 0 {
        return None;
    }
    let mut parts = s[..n].split(':');
    let hours: i32 = parts.next()?.parse().ok()?;
    let mins: i32 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    let secs: i32 = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((sign * (hours * 3600 + mins * 60 + secs), &s[n..]))
}

/// `Mm.w.d[/time]`, `Jn[/time]`, `n[/time]` → the day, its time (2:00 by
/// default), and the rest.
fn take_day(s: &str) -> Option<((Day, i32), &str)> {
    let end = s.find(',').unwrap_or(s.len());
    let (spec, rest) = (&s[..end], &s[end..]);
    let (day, time) = match spec.split_once('/') {
        Some((d, t)) => (d, take_offset(t)?.0),
        None => (spec, 7200),
    };
    let day = if let Some(m) = day.strip_prefix('M') {
        let mut p = m.split('.');
        let month: u32 = p.next()?.parse().ok()?;
        let week: u32 = p.next()?.parse().ok()?;
        let wd: u32 = p.next()?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=5).contains(&week) || wd > 6 || p.next().is_some() {
            return None;
        }
        Day::Month(month, week, wd)
    } else if let Some(j) = day.strip_prefix('J') {
        let n: u32 = j.parse().ok()?;
        (1..=365).contains(&n).then_some(Day::Julian1(n))?
    } else {
        let n: u32 = day.parse().ok()?;
        (n <= 365).then_some(Day::Julian0(n))?
    };
    Some(((day, time), rest))
}

// ── Windows' rules ────────────────────────────────────────────────────────
//
// Compiled on Windows, and everywhere for the tests: the reading of
// Windows' own structs is the only part that needs Windows.

#[cfg(any(windows, test))]
mod windows_zones;

/// One of Windows' transition dates (a `SYSTEMTIME`). With `year` 0 it is
/// the `day`-th `weekday` of `month`, 5 the last — POSIX's `Mm.w.d`; with a
/// year, that calendar date. `month` 0 means no daylight time at all.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct WinDate {
    year: u16,
    month: u16,
    weekday: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    millis: u16,
}

/// One year as Windows states it (`TIME_ZONE_INFORMATION`). The biases are
/// minutes *west* of UTC; `daylight` is when daylight time starts, on the
/// standard clock, and `standard` when it ends, on the daylight clock —
/// POSIX's start and end, read the same way round.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct WinYear {
    bias: i32,
    standard: WinDate,
    standard_bias: i32,
    daylight: WinDate,
    daylight_bias: i32,
}

#[cfg(any(windows, test))]
impl WinDate {
    /// The rule day, and its time on the clock. A switch Windows writes as
    /// 23:59:59.999 is the day's end, as POSIX writes it (24:00).
    fn day(&self) -> Option<(Day, i32)> {
        let (month, day) = (self.month as u32, self.day as u32);
        if !(1..=12).contains(&month) {
            return None;
        }
        let time = self.hour as i32 * 3600 + self.minute as i32 * 60 + self.second as i32 + (self.millis >= 500) as i32;
        let day = if self.year == 0 {
            if !(1..=5).contains(&day) || self.weekday > 6 {
                return None;
            }
            Day::Month(month, day, self.weekday as u32)
        } else {
            let year = self.year as i64;
            if day == 0 || day > days_in_month(year, month) {
                return None;
            }
            Day::Julian0((days_from_civil(year, month, day) - days_from_civil(year, 1, 1)) as u32)
        };
        Some((day, time))
    }
}

#[cfg(any(windows, test))]
impl WinYear {
    /// The year as a POSIX rule. `no_dst` is Windows' "Adjust for daylight
    /// saving time automatically" switched off: standard time all year.
    fn rule(&self, no_dst: bool) -> Option<Rule> {
        let std = -(self.bias + self.standard_bias) * 60;
        if no_dst || self.daylight.month == 0 || self.standard.month == 0 {
            return Some(Rule { std, dst: None });
        }
        let offset = -(self.bias + self.daylight_bias) * 60;
        Some(Rule { std, dst: Some(Daylight { offset, start: self.daylight.day()?, end: self.standard.day()? }) })
    }
}

/// The zone of Windows zone `key` (`Eastern Standard Time`), from its rule
/// for each year. With daylight time switched off in a zone that keeps it,
/// the IANA name would have the server count by clocks this machine does
/// not keep, so the fixed offset's name comes first; a zone CLDR has no name
/// for gets that one too. Only a half-hour offset has no fixed name, and then
/// the IANA name is the nearer of the two.
#[cfg(any(windows, test))]
fn windows_zone(key: &str, years: &[(i64, WinYear)], no_dst: bool) -> Option<Zone> {
    let rules: Vec<(i64, Rule)> = years.iter().map(|(year, w)| Some((*year, w.rule(no_dst)?))).collect::<Option<_>>()?;
    let keeps_dst = years.iter().any(|(_, w)| w.daylight.month != 0 && w.standard.month != 0);
    let iana = iana_of_windows(key).map(str::to_string);
    let fixed = || fixed_name(rules.last().map_or(0, |(_, rule)| rule.std));
    let name = if no_dst && keeps_dst { fixed().or(iana) } else { iana.or_else(fixed) }?;
    zone_from_years(name, &rules)
}

/// Year-by-year rules laid out as the transitions they make: the first
/// year's opening offset stands before them all, and the last year's rule is
/// the footer — as Windows applies its first and last entries to the years
/// beyond them.
#[cfg(any(windows, test))]
fn zone_from_years(name: String, years: &[(i64, Rule)]) -> Option<Zone> {
    let mut transitions: Vec<(i64, i32)> = Vec::new();
    for (year, rule) in years {
        // The year's first instant on its standard clock, then its two switches.
        let opens = days_from_civil(*year, 1, 1) * 86_400 - rule.std as i64;
        let mut events = vec![(opens, rule.offset_at(opens))];
        if let Some(dst) = &rule.dst {
            events.push((day_instant(*year, dst.start) - rule.std as i64, dst.offset));
            events.push((day_instant(*year, dst.end) - dst.offset as i64, rule.std));
        }
        events.sort_by_key(|&(at, _)| at);
        for (at, offset) in events {
            if transitions.last().is_none_or(|&(_, last)| last != offset) {
                transitions.push((at, offset));
            }
        }
    }
    let initial = transitions.first()?.1;
    let rule = years.last().map(|(_, rule)| rule.clone());
    Some(Zone { name, transitions, initial, rule })
}

/// The IANA zone CLDR names for a Windows zone.
#[cfg(any(windows, test))]
fn iana_of_windows(key: &str) -> Option<&'static str> {
    use windows_zones::WINDOWS_ZONES;
    WINDOWS_ZONES.binary_search_by(|(windows, _)| windows.cmp(&key)).ok().map(|i| WINDOWS_ZONES[i].1)
}

/// IANA's name for a whole-hour fixed offset: `Etc/GMT+5` is five hours
/// *west*, the sign the POSIX way round.
#[cfg(any(windows, test))]
fn fixed_name(offset: i32) -> Option<String> {
    (offset % 3600 == 0).then(|| match -offset / 3600 {
        0 => "Etc/UTC".to_string(),
        hours => format!("Etc/GMT{hours:+}"),
    })
}

/// The zone Windows is set to (`GetDynamicTimeZoneInformation`), with its
/// rule for every year Windows keeps one (`GetTimeZoneInformationForYear`).
#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation, GetDynamicTimeZoneInformationEffectiveYears,
        GetTimeZoneInformationForYear, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
    };

    use super::{WinDate, WinYear, Zone};

    /// The first year laid out as transitions. Windows applies its first
    /// entry's rule to the years before it, and nothing the player shows is
    /// older than this.
    const FIRST_YEAR: u32 = 1970;

    pub(super) fn local() -> Option<Zone> {
        let mut dtzi = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: a valid out-pointer to a struct of the type the call fills.
        if unsafe { GetDynamicTimeZoneInformation(&mut dtzi) } == TIME_ZONE_ID_INVALID {
            return None;
        }
        let (mut first, mut last) = (0u32, 0u32);
        // SAFETY: the struct just filled, and two valid out-pointers. Zero is
        // ERROR_SUCCESS; a zone without yearly entries answers otherwise.
        let yearly = unsafe { GetDynamicTimeZoneInformationEffectiveYears(&dtzi, &mut first, &mut last) } == 0
            && first <= last
            && last <= u16::MAX as u32;
        let years: Vec<(i64, WinYear)> = if yearly {
            let first_rule = for_year(&dtzi, first)?;
            (FIRST_YEAR.min(first)..first)
                .map(|year| Some((year as i64, first_rule)))
                .chain((first..=last).map(|year| Some((year as i64, for_year(&dtzi, year)?))))
                .collect::<Option<_>>()?
        } else {
            let one = win_year(dtzi.Bias, dtzi.StandardDate, dtzi.StandardBias, dtzi.DaylightDate, dtzi.DaylightBias);
            vec![(FIRST_YEAR as i64, one)]
        };
        super::windows_zone(&wide(&dtzi.TimeZoneKeyName), &years, dtzi.DynamicDaylightTimeDisabled)
    }

    fn for_year(dtzi: &DYNAMIC_TIME_ZONE_INFORMATION, year: u32) -> Option<WinYear> {
        let mut tzi = TIME_ZONE_INFORMATION::default();
        // SAFETY: both pointers are to live structs of the types the call takes.
        if unsafe { GetTimeZoneInformationForYear(year as u16, dtzi, &mut tzi) } == 0 {
            return None;
        }
        Some(win_year(tzi.Bias, tzi.StandardDate, tzi.StandardBias, tzi.DaylightDate, tzi.DaylightBias))
    }

    fn win_year(bias: i32, standard: SYSTEMTIME, standard_bias: i32, daylight: SYSTEMTIME, daylight_bias: i32) -> WinYear {
        WinYear { bias, standard: date(standard), standard_bias, daylight: date(daylight), daylight_bias }
    }

    fn date(t: SYSTEMTIME) -> WinDate {
        WinDate {
            year: t.wYear,
            month: t.wMonth,
            weekday: t.wDayOfWeek,
            day: t.wDay,
            hour: t.wHour,
            minute: t.wMinute,
            second: t.wSecond,
            millis: t.wMilliseconds,
        }
    }

    /// A NUL-terminated UTF-16 field.
    fn wide(s: &[u16]) -> String {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    }
}

// ── Civil dates (Howard Hinnant, proleptic Gregorian) ─────────────────────

pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since the epoch → `(year, month, day)`.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 0 = Sunday.
pub(crate) fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

pub(crate) fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub(crate) fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i64, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60
    }

    #[test]
    fn civil_dates_round_trip_and_know_their_weekday() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(weekday(0), 4, "the epoch was a Thursday");
        let d = days_from_civil(2026, 9, 13);
        assert_eq!(civil_from_days(d), (2026, 9, 13));
        assert_eq!(weekday(d), 0, "2026-09-13 is a Sunday");
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
    }

    #[test]
    fn the_new_york_rule_switches_at_two_in_the_morning_both_ways() {
        let rule = Rule::parse("EST5EDT,M3.2.0,M11.1.0").expect("rule");
        assert_eq!(rule.offset_at(at(2026, 1, 15, 12, 0)), -5 * 3600);
        assert_eq!(rule.offset_at(at(2026, 7, 1, 12, 0)), -4 * 3600);
        // 2026-03-08 02:00 EST → 03:00 EDT, i.e. 07:00Z.
        assert_eq!(rule.offset_at(at(2026, 3, 8, 6, 59)), -5 * 3600);
        assert_eq!(rule.offset_at(at(2026, 3, 8, 7, 0)), -4 * 3600);
        // 2026-11-01 02:00 EDT → 01:00 EST, i.e. 06:00Z.
        assert_eq!(rule.offset_at(at(2026, 11, 1, 5, 59)), -4 * 3600);
        assert_eq!(rule.offset_at(at(2026, 11, 1, 6, 0)), -5 * 3600);
    }

    #[test]
    fn southern_and_fixed_rules_parse_too() {
        let sydney = Rule::parse("AEST-10AEDT,M10.1.0,M4.1.0/3").expect("rule");
        assert_eq!(sydney.offset_at(at(2026, 7, 1, 0, 0)), 10 * 3600);
        assert_eq!(sydney.offset_at(at(2026, 1, 15, 0, 0)), 11 * 3600);
        let tehran = Rule::parse("<+0330>-3:30").expect("rule");
        assert_eq!(tehran.offset_at(at(2026, 7, 1, 0, 0)), 3 * 3600 + 1800);
        let berlin = Rule::parse("CET-1CEST,M3.5.0,M10.5.0/3").expect("rule");
        assert_eq!(berlin.offset_at(at(2026, 3, 29, 0, 59)), 3600);
        assert_eq!(berlin.offset_at(at(2026, 3, 29, 1, 0)), 7200);
        assert!(Rule::parse("nonsense,,").is_none());
        assert!(Rule::parse("XX5").is_none(), "a zone name is three letters or more");
    }

    /// A TZif v2 file by hand: two transitions in the 64-bit block, a footer.
    fn tzif(transitions: &[(i64, u8)], types: &[(i32, u8)], footer: &str) -> Vec<u8> {
        let header = |time_size: usize| -> Vec<u8> {
            let mut h = b"TZif2".to_vec();
            h.extend_from_slice(&[0; 15]);
            for count in [0u32, 0, 0, transitions.len() as u32, types.len() as u32, 4] {
                h.extend_from_slice(&count.to_be_bytes());
            }
            let _ = time_size;
            h
        };
        let block = |time_size: usize| -> Vec<u8> {
            let mut b = Vec::new();
            for (t, _) in transitions {
                if time_size == 8 { b.extend_from_slice(&t.to_be_bytes()) } else { b.extend_from_slice(&(*t as i32).to_be_bytes()) }
            }
            for (_, i) in transitions {
                b.push(*i);
            }
            for (off, dst) in types {
                b.extend_from_slice(&off.to_be_bytes());
                b.push(*dst);
                b.push(0);
            }
            b.extend_from_slice(b"ABC\0");
            b
        };
        let mut out = header(4);
        out.extend(block(4));
        out.extend(header(8));
        out.extend(block(8));
        out.extend_from_slice(format!("\n{footer}\n").as_bytes());
        out
    }

    #[test]
    fn a_tzif_file_answers_from_its_table_then_from_its_footer() {
        let march = at(2026, 3, 8, 7, 0);
        let november = at(2026, 11, 1, 6, 0);
        let data = tzif(&[(march, 1), (november, 0)], &[(-18_000, 0), (-14_400, 1)], "EST5EDT,M3.2.0,M11.1.0");
        let zone = parse("America/Test", &data).expect("parses");
        assert_eq!(zone.name, "America/Test");
        assert_eq!(zone.offset_at(march - 1), -18_000, "before the first transition: type 0");
        assert_eq!(zone.offset_at(march), -14_400);
        assert_eq!(zone.offset_at(november - 1), -14_400);
        assert_eq!(zone.offset_at(november), -18_000, "the table's last entry");
        assert_eq!(zone.offset_at(at(2027, 7, 1, 0, 0)), -14_400, "past the table: the footer rule");
        assert!(parse("x", b"not a zone file").is_none());
    }

    #[test]
    fn the_zone_name_is_the_path_under_zoneinfo() {
        assert_eq!(zone_name_of(Path::new("/var/db/timezone/zoneinfo/America/New_York")).as_deref(), Some("America/New_York"));
        assert_eq!(zone_name_of(Path::new("/usr/share/zoneinfo/UTC")).as_deref(), Some("UTC"));
        assert_eq!(zone_name_of(Path::new("/etc/something")), None);
    }

    /// Windows' Eastern Standard Time as `TIME_ZONE_INFORMATION` states a
    /// year: daylight time from the given Sunday (month, week) to the given
    /// one, both at 2:00.
    fn eastern(daylight: (u16, u16), standard: (u16, u16)) -> WinYear {
        let date = |(month, day): (u16, u16)| WinDate { month, day, hour: 2, ..WinDate::default() };
        WinYear { bias: 300, standard: date(standard), standard_bias: 0, daylight: date(daylight), daylight_bias: -60 }
    }

    #[test]
    fn a_windows_year_is_the_posix_rule_it_spells() {
        let rule = eastern((3, 2), (11, 1)).rule(false).expect("rule");
        assert_eq!(Some(rule), Rule::parse("EST5EDT,M3.2.0,M11.1.0"));
        let india = WinYear { bias: -330, ..WinYear::default() };
        assert_eq!(india.rule(false), Some(Rule { std: 5 * 3600 + 1800, dst: None }), "month 0: no daylight time");
    }

    #[test]
    fn windows_midnight_and_fixed_dates_read_as_posix_reads_them() {
        let end_of_day = WinDate { month: 3, day: 5, weekday: 6, hour: 23, minute: 59, second: 59, millis: 999, ..WinDate::default() };
        assert_eq!(end_of_day.day(), Some((Day::Month(3, 5, 6), 24 * 3600)), "23:59:59.999 is the day's end");
        let fixed = WinDate { year: 2026, month: 3, day: 1, ..WinDate::default() };
        assert_eq!(fixed.day(), Some((Day::Julian0(59), 0)), "1 March 2026, counted from zero");
        assert_eq!(WinDate { month: 13, day: 1, ..WinDate::default() }.day(), None);
        assert_eq!(WinDate { month: 3, day: 6, ..WinDate::default() }.day(), None, "there is no sixth week");
    }

    #[test]
    fn a_windows_zone_follows_its_years_and_then_the_last_one() {
        // The US rule before 2007: the first Sunday of April to the last of October.
        let years = [(2006, eastern((4, 1), (10, 5))), (2007, eastern((3, 2), (11, 1)))];
        let zone = windows_zone("Eastern Standard Time", &years, false).expect("zone");
        assert_eq!(zone.name, "America/New_York");
        assert_eq!(zone.offset_at(at(2006, 3, 20, 12, 0)), -5 * 3600, "2006 kept standard time into April");
        assert_eq!(zone.offset_at(at(2006, 4, 10, 12, 0)), -4 * 3600);
        assert_eq!(zone.offset_at(at(2006, 10, 30, 12, 0)), -5 * 3600, "and left daylight time on 29 October");
        assert_eq!(zone.offset_at(at(2007, 3, 20, 12, 0)), -4 * 3600, "2007's began on 11 March");
        assert_eq!(zone.offset_at(at(2007, 10, 30, 12, 0)), -4 * 3600, "and ran to 4 November");
        // Past the years Windows lists: the last one's rule, switches and all.
        assert_eq!(zone.offset_at(at(2026, 3, 8, 6, 59)), -5 * 3600);
        assert_eq!(zone.offset_at(at(2026, 3, 8, 7, 0)), -4 * 3600);
        assert_eq!(zone.offset_at(at(2026, 10, 29, 22, 0)), -4 * 3600, "8 pm in New York, late October");
        assert_eq!(zone.offset_at(at(2026, 11, 1, 6, 0)), -5 * 3600);
    }

    #[test]
    fn a_southern_windows_zone_keeps_daylight_time_over_new_year() {
        let sydney = WinYear {
            bias: -600,
            standard: WinDate { month: 4, day: 1, hour: 3, ..WinDate::default() },
            standard_bias: 0,
            daylight: WinDate { month: 10, day: 1, hour: 2, ..WinDate::default() },
            daylight_bias: -60,
        };
        let zone = windows_zone("AUS Eastern Standard Time", &[(2025, sydney), (2026, sydney)], false).expect("zone");
        assert_eq!(zone.name, "Australia/Sydney");
        assert_eq!(zone.offset_at(at(2026, 1, 15, 0, 0)), 11 * 3600);
        assert_eq!(zone.offset_at(at(2026, 7, 1, 0, 0)), 10 * 3600);
        assert_eq!(zone.offset_at(at(2027, 1, 15, 0, 0)), 11 * 3600, "the footer carries the summer on");
    }

    #[test]
    fn daylight_time_switched_off_keeps_standard_time_under_a_fixed_name() {
        let zone = windows_zone("Eastern Standard Time", &[(2026, eastern((3, 2), (11, 1)))], true).expect("zone");
        assert_eq!(zone.name, "Etc/GMT+5", "America/New_York would have the server count summer hours");
        assert_eq!(zone.offset_at(at(2026, 7, 1, 12, 0)), -5 * 3600);
        let india = WinYear { bias: -330, ..WinYear::default() };
        let zone = windows_zone("India Standard Time", &[(2026, india)], true).expect("zone");
        assert_eq!(zone.name, "Asia/Calcutta", "no daylight time to switch off: the name stands");
        assert_eq!(zone.offset_at(at(2026, 7, 1, 12, 0)), 5 * 3600 + 1800);
        // Newfoundland, -3:30, has no fixed name: its own beats none at all.
        let st_johns = WinYear { bias: 210, ..eastern((3, 2), (11, 1)) };
        let zone = windows_zone("Newfoundland Standard Time", &[(2026, st_johns)], true).expect("zone");
        assert_eq!(zone.name, "America/St_Johns");
        assert_eq!(zone.offset_at(at(2026, 7, 1, 12, 0)), -(3 * 3600 + 1800));
    }

    #[test]
    fn windows_zones_carry_their_cldr_names() {
        assert_eq!(iana_of_windows("Eastern Standard Time"), Some("America/New_York"));
        assert_eq!(iana_of_windows("W. Europe Standard Time"), Some("Europe/Berlin"));
        assert_eq!(iana_of_windows("UTC"), Some("Etc/UTC"));
        assert_eq!(iana_of_windows("Nowhere Standard Time"), None);
        let zones = windows_zones::WINDOWS_ZONES;
        assert!(zones.windows(2).all(|pair| pair[0].0 < pair[1].0), "sorted, for the search by halves");
        // A zone CLDR has not named yet still gets a name the server takes.
        let unnamed = WinYear { bias: -120, ..WinYear::default() };
        assert_eq!(windows_zone("Nowhere Standard Time", &[(2026, unnamed)], false).map(|z| z.name).as_deref(), Some("Etc/GMT-2"));
        assert_eq!(fixed_name(0).as_deref(), Some("Etc/UTC"));
        assert_eq!(fixed_name(-5 * 3600).as_deref(), Some("Etc/GMT+5"));
        assert_eq!(fixed_name(5 * 3600 + 1800), None, "no Etc zone for a half hour");
    }

    /// The machine's own zone agrees with the clock Windows keeps now.
    #[cfg(windows)]
    #[test]
    fn windows_names_its_zone_and_agrees_with_its_own_clock() {
        use windows_sys::Win32::System::Time::{DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation};
        let zone = windows::local().expect("Windows is always set to a zone");
        assert!(!zone.name.is_empty());
        let mut dtzi = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: a valid out-pointer to a struct of the type the call fills.
        let now_in = unsafe { GetDynamicTimeZoneInformation(&mut dtzi) };
        // TIME_ZONE_ID_STANDARD (1) and TIME_ZONE_ID_DAYLIGHT (2) name the bias in force.
        let bias = dtzi.Bias
            + match now_in {
                1 => dtzi.StandardBias,
                2 => dtzi.DaylightBias,
                _ => 0,
            };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("after 1970").as_secs() as i64;
        assert_eq!(zone.offset_at(now), -bias * 60, "{zone:?}");
    }
}
