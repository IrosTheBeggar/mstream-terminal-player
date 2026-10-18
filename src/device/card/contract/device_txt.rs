//! `/.player/device.txt`: what the card's firmware reads (docs/METADATA.md
//! 2.15) — the one file under `/.player` the software relies on, and only
//! reads. The device writes it at its own boot, when its content would
//! change; a card fresh from a format has none until it has booted once in
//! a player.
//!
//! ASCII, LF line ends (CRLF tolerated), one `key=value` a line; unknown
//! keys are ignored. A key missing reads as its no-file value, a key with
//! an empty value (`read.mpdj=`) says none, and the `contract` line is what
//! makes a file a `device.txt` at all.

use super::mpdj;

/// The formats whose majors `read.*` lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Msmf,
    Mptg,
    Mpdj,
    Mpth,
}

/// What the firmware reads. Majors are bit masks, bit 0 for major 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceTxt {
    /// The file was there and is a device.txt (it has a `contract` line);
    /// false: the no-file values.
    pub(crate) present: bool,
    pub(crate) contract: u16,
    pub(crate) firmware: String,
    pub(crate) read_msmf: u32,
    pub(crate) read_mptg: u32,
    pub(crate) read_mpdj: u32,
    pub(crate) read_mpth: u32,
    /// What it decodes, and the extensions its walk lists as audio (any
    /// case): comma-separated.
    pub(crate) codecs: String,
    pub(crate) extensions: String,
    pub(crate) max_rate: u32,
    pub(crate) max_channels: u32,
}

/// The longest value kept: a list of every codec there is fits many times.
const VALUE_MAX: usize = 1024;

impl DeviceTxt {
    /// A card with no device.txt (a new card, or firmware before this
    /// design): every format's major 1 except MPDJ, which is none; mp3 and
    /// flac; 48 kHz; two channels. Firmware 0.7.0 plays Opus but can't say
    /// so; the software MAY ask the listener rather than convert.
    pub(crate) fn defaults() -> DeviceTxt {
        DeviceTxt {
            present: false,
            contract: 1,
            firmware: String::new(),
            read_msmf: 1,
            read_mptg: 1,
            read_mpdj: 0,
            read_mpth: 1,
            codecs: "mp3,flac".into(),
            extensions: "mp3,flac".into(),
            max_rate: 48_000,
            max_channels: 2,
        }
    }

    /// The card's file (None: there is none): each key the file has
    /// replaces its default; a file with no `contract=` line isn't a
    /// device.txt, and reads as no file.
    pub(crate) fn read(file: Option<&[u8]>) -> DeviceTxt {
        let mut out = DeviceTxt::defaults();
        let Some(text) = file else { return out };
        let mut contract = false;
        for line in text.split(|&c| c == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(eq) = line.iter().position(|&c| c == b'=') else { continue };
            let (key, value) = (&line[..eq], &line[eq + 1..]);
            match key {
                b"contract" => {
                    out.contract = number(value).map_or(0, |v| v.min(u16::MAX as u32) as u16);
                    contract = true;
                }
                b"firmware" => out.firmware = text_value(value),
                b"read.msmf" => out.read_msmf = majors(value),
                b"read.mptg" => out.read_mptg = majors(value),
                b"read.mpdj" => out.read_mpdj = majors(value),
                b"read.mpth" => out.read_mpth = majors(value),
                b"codecs" => out.codecs = text_value(value),
                b"extensions" => out.extensions = text_value(value),
                b"max_rate" => out.max_rate = number(value).unwrap_or(out.max_rate),
                b"max_channels" => out.max_channels = number(value).unwrap_or(out.max_channels),
                _ => {}
            }
        }
        if !contract {
            return DeviceTxt::defaults();
        }
        out.present = true;
        out
    }

    /// Whether the firmware reads `major` of a format.
    pub(crate) fn reads(&self, format: Format, major: u32) -> bool {
        let mask = match format {
            Format::Msmf => self.read_msmf,
            Format::Mptg => self.read_mptg,
            Format::Mpdj => self.read_mpdj,
            Format::Mpth => self.read_mpth,
        };
        (1..=32).contains(&major) && mask >> (major - 1) & 1 != 0
    }

    /// The software writes MPDJ v2 only when `read.mpdj` lists 2, and no
    /// MPDJ otherwise (AUTODJ-INDEX.md 7.4): no reader of v1 exists, and
    /// firmware 0.8.0-0.10.0 lists 1 without reading it.
    pub(crate) fn writes_mpdj(&self) -> bool {
        mpdj::writes_v2(self.read_mpdj)
    }

    /// `name` is one of `codecs`, ignoring ASCII case.
    pub(crate) fn decodes(&self, codec: &str) -> bool {
        list_has(&self.codecs, codec)
    }

    /// The walk lists files of this extension (no dot) as audio.
    pub(crate) fn lists(&self, extension: &str) -> bool {
        list_has(&self.extensions, extension)
    }

    /// The text the firmware writes for these values, keys in 2.15's order
    /// (the software never writes `/.player`; this is for tests and fakes).
    pub(crate) fn to_text(&self) -> String {
        let majors = |mask: u32| (1..=32u32).filter(move |m| mask >> (m - 1) & 1 != 0).map(|m| m.to_string());
        let list = |mask: u32| majors(mask).collect::<Vec<_>>().join(",");
        format!(
            "contract={}\nfirmware={}\nread.msmf={}\nread.mptg={}\nread.mpdj={}\nread.mpth={}\ncodecs={}\n\
             extensions={}\nmax_rate={}\nmax_channels={}\n",
            self.contract,
            self.firmware,
            list(self.read_msmf),
            list(self.read_mptg),
            list(self.read_mpdj),
            list(self.read_mpth),
            self.codecs,
            self.extensions,
            self.max_rate,
            self.max_channels
        )
    }
}

/// `1,2` as a mask; `` is none; items that aren't a number from 1 to 32
/// (spaces allowed around one) are skipped.
fn majors(value: &[u8]) -> u32 {
    let mut mask = 0;
    for item in value.split(|&c| c == b',') {
        let item: Vec<u8> = item.iter().copied().filter(|&c| c != b' ').collect();
        if item.is_empty() || !item.iter().all(u8::is_ascii_digit) {
            continue;
        }
        // The player's cap: past 99 the value only grows out of range.
        let v: u32 = item.iter().fold(0, |v, &c| if v < 100 { v * 10 + (c - b'0') as u32 } else { v });
        if (1..=32).contains(&v) {
            mask |= 1 << (v - 1);
        }
    }
    mask
}

/// A decimal of at most 9 digits; anything else keeps the default.
fn number(value: &[u8]) -> Option<u32> {
    if value.is_empty() || value.len() > 9 || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(value.iter().fold(0, |v, &c| v * 10 + (c - b'0') as u32))
}

fn text_value(value: &[u8]) -> String {
    String::from_utf8_lossy(&value[..value.len().min(VALUE_MAX)]).into_owned()
}

/// `name` is one of a comma-separated list's items, ignoring ASCII case.
fn list_has(list: &str, name: &str) -> bool {
    !name.is_empty() && list.split(',').any(|item| item.trim().eq_ignore_ascii_case(name))
}
