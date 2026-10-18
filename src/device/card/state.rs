//! `/.mstream/state.json`: the card's identity and settings, the software's
//! private file (the firmware repo's docs/METADATA.md 2.12.7; the transfer
//! plan's 2.10). The device never opens it. Schema 1 holds, so far:
//!
//! - `cardId`: the card id of the root it was written beside (2.5.2), as a
//!   hex string; a file naming another is another identity's;
//! - `label`: the listener's name for the card;
//! - `volumeSerial`: the FAT volume serial it was written on, as Windows and
//!   Linux read it (`1A2B-3C4D`), and `volumeUuid`, the stand-in a Mac reads
//!   instead (the two can't be compared, so each is kept beside the other);
//! - `created`: when this identity was set up, in Unix seconds.
//!
//! The sync's settings (sources, layout, conversion, covers, Auto DJ) join
//! schema 1 with the sync (B1). Every member this software doesn't know is
//! kept, unchanged, whenever it writes the file again (2.12.7), as config.rs
//! keeps a newer player's settings.
//!
//! **Privacy** (2.10, 2.12.7): no URL, token, user name, host name or IP
//! address is ever written. The only free text so far is the label, and a
//! label that names a place on a network isn't written ([`label_fault`],
//! [`looks_like_address`]).
//!
//! **A copied card** (2.10): a card's files copied onto another volume keep
//! its id, and its state file still names the serial it was written on —
//! another serial, of the same kind, than the one read now ([`copied`]).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use serde_json::Value;

use super::volume::{Private, STATE_VERSION, Serial};

/// The file's own `format` (2.12.7).
pub(crate) const FORMAT: &str = "mstream-card-state";

/// The members schema 1 reads and writes; every other member is kept.
const KNOWN: [&str; 8] =
    ["format", "version", "seq", "cardId", "label", "volumeSerial", "volumeUuid", "created"];

/// The longest label, in characters.
pub(crate) const LABEL_CHARS: usize = 64;

/// A card's identity and settings, as its state file holds them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    /// The card id it names: the root's, when it is this card's own.
    pub(crate) card_id: u64,
    /// The seq of the file it was read from; 0 for one not on the card yet.
    pub(crate) seq: u64,
    pub(crate) label: String,
    /// The volume serials it was written on: FAT's own, and a Mac's
    /// stand-in.
    pub(crate) fat_serial: Option<u32>,
    pub(crate) uuid: Option<String>,
    /// Unix seconds.
    pub(crate) created: u64,
    /// Every member this software doesn't know, by name.
    pub(crate) keep: BTreeMap<String, Value>,
}

impl State {
    /// A new card's state: set up now, on the volume of `serial`.
    pub(crate) fn new(card_id: u64, label: &str, serial: Option<&Serial>, created: u64) -> State {
        let mut state = State { card_id, label: label.to_string(), created, ..State::default() };
        state.saw(serial);
        state
    }

    /// The state the card's settled file holds (2.12.7's read); None when it
    /// has no valid one.
    pub(crate) fn of(private: &Private) -> Option<State> {
        let object = private.value.as_ref()?.as_object()?;
        let text = |key: &str| object.get(key).and_then(Value::as_str);
        let fat_serial = text("volumeSerial").and_then(|t| match Serial::parse(t) {
            Serial::Fat(n) => Some(n),
            Serial::Uuid(_) => None,
        });
        let keep = object.iter().filter(|(name, _)| !KNOWN.contains(&name.as_str()));
        Some(State {
            card_id: text("cardId").and_then(from_hex).unwrap_or(0),
            seq: private.seq,
            label: text("label").unwrap_or_default().to_string(),
            fat_serial,
            uuid: text("volumeUuid").map(str::to_string),
            created: object.get("created").and_then(Value::as_u64).unwrap_or(0),
            keep: keep.map(|(name, value)| (name.clone(), value.clone())).collect(),
        })
    }

    /// The serial it names of `now`'s kind: the one to compare `now` with.
    pub(crate) fn serial_like(&self, now: &Serial) -> Option<Serial> {
        match now {
            Serial::Fat(_) => self.fat_serial.map(Serial::Fat),
            Serial::Uuid(_) => self.uuid.clone().map(Serial::Uuid),
        }
    }

    /// The volume it is written on now: its serial of that kind, the other
    /// kind's kept.
    pub(crate) fn saw(&mut self, serial: Option<&Serial>) {
        match serial {
            Some(Serial::Fat(n)) => self.fat_serial = Some(*n),
            Some(Serial::Uuid(text)) => self.uuid = Some(text.clone()),
            None => {}
        }
    }

    /// The file for its write of `seq` (2.12.7): UTF-8 JSON, no byte-order
    /// mark, one object — this software's members, then every member it
    /// doesn't know, unchanged.
    pub(crate) fn text(&self, seq: u64) -> Vec<u8> {
        let mut members: Vec<(&str, Value)> = vec![
            ("format", FORMAT.into()),
            ("version", STATE_VERSION.into()),
            ("seq", seq.into()),
            ("cardId", hex(self.card_id).into()),
            ("label", self.label.as_str().into()),
        ];
        if let Some(n) = self.fat_serial {
            members.push(("volumeSerial", Serial::Fat(n).to_string().into()));
        }
        if let Some(uuid) = &self.uuid {
            members.push(("volumeUuid", uuid.as_str().into()));
        }
        members.push(("created", self.created.into()));
        members.extend(self.keep.iter().map(|(name, value)| (name.as_str(), value.clone())));
        let json = |v: &Value| serde_json::to_string(v).unwrap_or_default();
        let line = |(name, value): &(&str, Value)| {
            format!("  {}: {}", json(&(*name).into()), json(value))
        };
        let lines: Vec<String> = members.iter().map(line).collect();
        format!("{{\n{}\n}}\n", lines.join(",\n")).into_bytes()
    }
}

/// A card id as the private files spell one (2.12.7: integers that may pass
/// 2^53 are hex strings): `0x5EEDC0DE0A1B2C3D`.
pub(crate) fn hex(id: u64) -> String {
    format!("0x{id:016X}")
}

/// A hex string of at most 16 digits, `0x` first, in either case.
pub(crate) fn from_hex(text: &str) -> Option<u64> {
    let digits = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X"))?;
    let hex = (1..=16).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_hexdigit());
    if hex { u64::from_str_radix(digits, 16).ok() } else { None }
}

// ── Copied cards ────────────────────────────────────────────────────────────

/// A sign that the card in the reader is a copy of another (2.10): one
/// identity on two cards. The listener's answer is a new identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Copied {
    /// The state file, this card id's own, was written on a volume of
    /// another serial: the card's files were copied onto this one.
    Serial { was: Serial, now: Serial },
    /// This computer saw the card id at this generation with another commit
    /// (`cards.json`): a byte-level clone, serial and all, that went its own
    /// way — or a backup of `/.mstream` put back.
    Diverged { generation: u32, seen: u64, now: u64 },
}

/// The first signal, compared at each run: the state file names the root's
/// card id and a serial of the kind read now (`now`), but another one. A
/// serial the OS didn't give, or of a kind the file doesn't name (a card
/// set up on Windows, read on a Mac), says nothing.
pub(crate) fn copied(state: &State, card_id: u64, now: Option<&Serial>) -> Option<Copied> {
    if card_id == 0 || state.card_id != card_id {
        return None;
    }
    let now = now?;
    let was = state.serial_like(now)?;
    (was != *now).then(|| Copied::Serial { was, now: now.clone() })
}

// ── What a private file may say ─────────────────────────────────────────────

/// Why a label isn't written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LabelFault {
    /// It names a place on a network (2.10's privacy rule).
    Address,
    /// A control character.
    Control,
    /// Longer than [`LABEL_CHARS`].
    Long,
}

impl LabelFault {
    /// The reason as one stable word, for the debug command and the tests.
    pub(crate) fn code(self) -> &'static str {
        match self {
            LabelFault::Address => "label-address",
            LabelFault::Control => "label-control",
            LabelFault::Long => "label-long",
        }
    }
}

/// A label the state file may hold, or why not.
pub(crate) fn label_fault(label: &str) -> Option<LabelFault> {
    if label.chars().any(char::is_control) {
        Some(LabelFault::Control)
    } else if label.chars().count() > LABEL_CHARS {
        Some(LabelFault::Long)
    } else if looks_like_address(label) {
        Some(LabelFault::Address)
    } else {
        None
    }
}

/// Text that names a place on a network: a URL, an IP address, `localhost`,
/// a lowercase host name with a domain (`music.example`), any name with a
/// port (`alex-box:3000`), or a user at a host (`alex@music.example`). Such text
/// is never written into a card's private files (2.10). The test is
/// word by word, so a label that only holds one in its middle is caught; a
/// name in capitals with a dot (`Mr.Big`) is a name, not a host.
pub(crate) fn looks_like_address(text: &str) -> bool {
    if text.contains("://") {
        return true;
    }
    let apart = |c: char| c.is_whitespace() || ",;()<>\"'".contains(c);
    let words = text.split(apart).map(|w| w.trim_end_matches(['.', '/']));
    words.filter(|w| !w.is_empty()).any(address_word)
}

fn address_word(word: &str) -> bool {
    if let Some((user, host)) = word.split_once('@')
        && !user.is_empty()
        && host_like(host)
    {
        return true;
    }
    if word.parse::<IpAddr>().is_ok() || word.parse::<SocketAddr>().is_ok() {
        return true;
    }
    let bracketed = word.strip_prefix('[').and_then(|w| w.strip_suffix(']'));
    bracketed.is_some_and(|w| w.parse::<Ipv6Addr>().is_ok()) || host_like(word)
}

/// `host[:port][/path]`: `localhost`, an IP address, a lowercase dotted
/// name whose last part is letters (a domain), or any name with a port.
fn host_like(word: &str) -> bool {
    let word = word.split('/').next().unwrap_or_default();
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    let (host, port) = match word.rsplit_once(':') {
        Some((host, port)) if digits(port) => (host, true),
        _ => (word, false),
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok() {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    let name = |b: u8| b.is_ascii_alphanumeric() || b == b'-';
    let part = |p: &&str| !p.is_empty() && p.bytes().all(name);
    if !parts.iter().all(part) {
        return false;
    }
    let last = parts.last().copied().unwrap_or_default();
    let tld = last.len() >= 2 && last.bytes().all(|b| b.is_ascii_alphabetic());
    let lower = !host.bytes().any(|b| b.is_ascii_uppercase());
    (parts.len() >= 2 && tld && lower) || port
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::card::volume::settle_private;

    fn read(bytes: &[u8]) -> Option<State> {
        State::of(&settle_private(Some(bytes), None, "state", STATE_VERSION))
    }

    #[test]
    fn a_state_file_is_written_as_2_12_7_reads_it_and_keeps_what_it_doesnt_know() {
        let fat = Serial::Fat(0x1A2B_3C4D);
        let mut state = State::new(0x5EED_C0DE_0A1B_2C3D, "Car", Some(&fat), 1_791_900_000);
        let text = String::from_utf8(state.text(1)).unwrap();
        let want = "{\n  \"format\": \"mstream-card-state\",\n  \"version\": 1,\n  \"seq\": 1,\n  \
                    \"cardId\": \"0x5EEDC0DE0A1B2C3D\",\n  \"label\": \"Car\",\n  \
                    \"volumeSerial\": \"1A2B-3C4D\",\n  \"created\": 1791900000\n}\n";
        assert_eq!(text, want);
        let back = read(text.as_bytes()).expect("valid");
        assert_eq!(back, State { seq: 1, ..state.clone() });

        // A newer software's members, at the top level, stay as they were;
        // a Mac's stand-in joins the FAT serial, which stays.
        let newer = r#"{"format": "mstream-card-state", "version": 1, "seq": 4, "cardId": "0x5eedc0de0a1b2c3d",
            "label": "Car \"A\"", "volumeSerial": "1A2B-3C4D", "created": 7,
            "sources": [{"serverUrlKey": "0xED901EA3EE763AC7", "nickname": "Home"}], "zz": null}"#;
        state = read(newer.as_bytes()).expect("valid");
        let read_back = (state.card_id, state.seq, state.label.as_str(), state.created);
        assert_eq!(read_back, (0x5EED_C0DE_0A1B_2C3D, 4, "Car \"A\"", 7));
        assert_eq!(state.keep.keys().collect::<Vec<_>>(), ["sources", "zz"]);
        state.saw(Some(&Serial::Uuid("0F1E2D3C-4B5A-4968-8796-A5B4C3D2E1F0".into())));
        let again = read(&state.text(5)).expect("valid");
        assert_eq!(again.keep, state.keep);
        assert_eq!(again.fat_serial, Some(0x1A2B_3C4D));
        assert_eq!(again.uuid.as_deref(), Some("0F1E2D3C-4B5A-4968-8796-A5B4C3D2E1F0"));
        assert_eq!(again.label, "Car \"A\"");

        // The vectors' minimal files: a state with no identity yet.
        let bare = br#"{"format": "mstream-card-state", "version": 1, "seq": 3, "label": "Card A"}"#;
        let bare = read(bare).expect("valid");
        assert_eq!((bare.card_id, bare.label.as_str(), bare.fat_serial), (0, "Card A", None));
        assert!(read(br#"{"format": "mstream-card-state", "version": 1}"#).is_none(), "no seq: invalid");
        let hexes = ["0x00", "0Xff", "12", "0x", "0x1FFFFFFFFFFFFFFFF", "0x-1"].map(from_hex);
        assert_eq!(hexes, [Some(0), Some(255), None, None, None, None]);
    }

    #[test]
    fn a_serial_of_the_same_kind_says_a_card_was_copied() {
        let fat = Serial::Fat(0x1A2B_3C4D);
        let state = State::new(9, "Car", Some(&fat), 1);
        assert_eq!(copied(&state, 9, Some(&fat)), None, "the card it was set up on");
        let other = Serial::Fat(0x5E6F_7A8B);
        let sign = Copied::Serial { was: fat.clone(), now: other.clone() };
        assert_eq!(copied(&state, 9, Some(&other)), Some(sign));
        // Another identity's state, no serial now, a kind the file doesn't
        // name: no signal.
        assert_eq!(copied(&state, 10, Some(&other)), None);
        assert_eq!(copied(&state, 9, None), None);
        let mac = Serial::Uuid("0F1E2D3C-4B5A-4968-8796-A5B4C3D2E1F0".into());
        assert_eq!(copied(&state, 9, Some(&mac)), None);
        let mut both = state.clone();
        both.saw(Some(&mac));
        let other_mac = Serial::Uuid("11111111-2222-4333-8444-555555555555".into());
        assert_eq!(copied(&both, 9, Some(&mac)), None);
        assert!(matches!(copied(&both, 9, Some(&other_mac)), Some(Copied::Serial { .. })));
        assert_eq!(copied(&both, 9, Some(&fat)), None, "the FAT serial is still the one it was");
    }

    #[test]
    fn a_label_that_names_a_place_on_a_network_is_never_written() {
        let addresses = [
            "http://music.example:3000",
            "Home (https://music.example/mstream)",
            "music.example",
            "alex-box.local",
            "my-server.example.org",
            "music.example:3000",
            "alex-box:3000",
            "192.0.2.20",
            "192.0.2.20:3000",
            "Car 198.51.100.7",
            "fe80::1",
            "[::1]:3000",
            "[2001:db8::1]",
            "localhost",
            "LOCALHOST:3000",
            "alex@music.example",
            "music.example/",
        ];
        for label in addresses {
            assert!(looks_like_address(label), "{label}");
            assert_eq!(label_fault(label), Some(LabelFault::Address), "{label}");
        }
        let names = [
            "Car music",
            "Rock & Jazz",
            "Mr.Big",
            "Vol. 2",
            "Mix 2024.1",
            "Alex's card",
            "Jazz/Blues",
            "Card 1 of 2",
            "",
            "Caf\u{E9} del Mar",
            "No. 5",
            "@home",
        ];
        for label in names {
            assert!(!looks_like_address(label), "{label}");
            assert_eq!(label_fault(label), None, "{label}");
        }
        assert_eq!(label_fault("Car\nmusic"), Some(LabelFault::Control));
        assert_eq!(label_fault(&"x".repeat(LABEL_CHARS)), None);
        assert_eq!(label_fault(&"x".repeat(LABEL_CHARS + 1)), Some(LabelFault::Long));
        assert_eq!(LabelFault::Address.code(), "label-address");
    }
}
