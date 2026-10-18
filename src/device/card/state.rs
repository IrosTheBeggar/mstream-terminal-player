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
//! - `created`: when this identity was set up, in Unix seconds;
//!
//! and the sync's settings ([`Settings`]), written once a sync has stored
//! them: `sources` (each server the card is filled from, by its URL's key
//! and the nickname this computer shows for it, with its libraries and
//! what the sync keeps of its manifest), `layout`, `conversion`, `covers`,
//! `autoDj`, `removal`, and `art` (which art each album's thumbnail was made
//! from: 2.12.1, step 10). Every member this software doesn't know is kept,
//! unchanged, whenever it writes the file again (2.12.7), at the top level
//! and in each object of the settings it rewrites, as config.rs keeps a
//! newer player's settings.
//!
//! **Privacy** (2.10, 2.12.7): no URL, token, user name, host name or IP
//! address is ever written. The free text is the label, a server's
//! nickname and its libraries' names; a label or a nickname that names a
//! place on a network isn't written ([`label_fault`], [`looks_like_address`]).
//!
//! **A copied card** (2.10): a card's files copied onto another volume keep
//! its id, and its state file still names the serial it was written on —
//! another serial, of the same kind, than the one read now ([`copied`]).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use serde_json::{Map, Value};

use super::contract::device_txt::DeviceTxt;
use super::engine::Removal;
use super::volume::{Private, STATE_VERSION, Serial};
use crate::api::urls::{TransferBitrate, TransferCodec};

/// The file's own `format` (2.12.7).
pub(crate) const FORMAT: &str = "mstream-card-state";

/// The members schema 1 reads and writes; every other member is kept.
const KNOWN: [&str; 8] =
    ["format", "version", "seq", "cardId", "label", "volumeSerial", "volumeUuid", "created"];

/// The settings' members ([`Settings`]).
const SETTINGS: [&str; 7] =
    ["sources", "layout", "conversion", "covers", "autoDj", "removal", "art"];

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
    pub(crate) settings: Settings,
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
        let known = |name: &str| KNOWN.contains(&name) || SETTINGS.contains(&name);
        let keep = object.iter().filter(|(name, _)| !known(name));
        Some(State {
            card_id: text("cardId").and_then(from_hex).unwrap_or(0),
            seq: private.seq,
            label: text("label").unwrap_or_default().to_string(),
            fat_serial,
            uuid: text("volumeUuid").map(str::to_string),
            created: object.get("created").and_then(Value::as_u64).unwrap_or(0),
            settings: Settings::read(object),
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
        members.extend(self.settings.members());
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

// ── The sync's settings ─────────────────────────────────────────────────────

/// Where the card keeps each library (2.8.6; the transfer plan's U2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Layout {
    /// As the card has it; for a card with no ledger yet, `/music/<vpath>/`
    /// when the selection holds more than one library.
    #[default]
    Auto,
    /// `/music/<rel>`, the vpath dropped.
    Plain,
    /// `/music/<vpath>/<rel>`, a LIBR root each.
    Vpath,
}

impl Layout {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Layout::Auto => "auto",
            Layout::Plain => "plain",
            Layout::Vpath => "vpath",
        }
    }

    pub(crate) fn of_word(word: &str) -> Option<Layout> {
        [Layout::Auto, Layout::Plain, Layout::Vpath].into_iter().find(|l| l.word() == word)
    }

    /// The engine's knob ([`super::engine::Knobs::vpath`]).
    pub(crate) fn vpath(self) -> Option<bool> {
        match self {
            Layout::Auto => None,
            Layout::Plain => Some(false),
            Layout::Vpath => Some(true),
        }
    }
}

/// Which files are converted (2.5, section 6's default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Convert {
    /// Only what the player can't play.
    #[default]
    Unplayable,
    /// Everything, to save room: but a file already in the codec at no more
    /// than the bitrate.
    Everything,
}

impl Convert {
    pub(crate) fn word(self) -> &'static str {
        match self {
            Convert::Unplayable => "unplayable",
            Convert::Everything => "everything",
        }
    }

    pub(crate) fn of_word(word: &str) -> Option<Convert> {
        [Convert::Unplayable, Convert::Everything].into_iter().find(|c| c.word() == word)
    }
}

/// What a conversion makes: section 6's default, Opus at 192 kbit/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Conversion {
    pub(crate) convert: Convert,
    pub(crate) codec: TransferCodec,
    pub(crate) bitrate: TransferBitrate,
}

impl Default for Conversion {
    fn default() -> Conversion {
        let (codec, bitrate) = (TransferCodec::Opus, TransferBitrate::K192);
        Conversion { convert: Convert::Unplayable, codec, bitrate }
    }
}

/// A server the card is filled from (the transfer plan's 2.10): known by
/// its URL's key (2.5.1) and the nickname this computer shows for it, never
/// by its address.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Source {
    pub(crate) url_key: u64,
    /// As the listener knows it ("Home"); "" for none, and for one that
    /// names a place on a network, which is never written.
    pub(crate) nickname: String,
    /// The selection: its libraries, by vpath.
    pub(crate) libraries: Vec<String>,
    /// The keys the listener said are this server too (2.5.1, 2.11's key
    /// rule): its other addresses.
    pub(crate) confirmed: Vec<u64>,
    /// Its manifest's revision as the last whole sync read it, kept here
    /// when the server isn't server 0, whose revision is the root's
    /// (2.10.2).
    pub(crate) revision: String,
    /// The settings the last whole read planned with ([`Settings::plan_key`]
    /// and the selection): another, and a 304 can't stand for a listing.
    pub(crate) plan_key: u64,
    /// When the manifest was last read whole (Unix seconds), and the runs
    /// since: a 304 is only "probably unchanged", so a run reads it whole
    /// again every 7 days or 10th run (2.10.2).
    pub(crate) full_read: u64,
    pub(crate) runs: u32,
    /// The object as read: the members this software doesn't know stay.
    raw: Kept,
}

/// The sync's settings in a card's state file. A member the file doesn't
/// have reads as section 6's default.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) sources: Vec<Source>,
    pub(crate) layout: Layout,
    pub(crate) conversion: Conversion,
    /// Album covers: thumbnails, and a converted file's cover put back.
    pub(crate) covers: bool,
    pub(crate) auto_dj: bool,
    /// A card made for someone else gets no Auto DJ index: the model's
    /// licence is non-commercial share-alike (AUTODJ-INDEX.md 12.5).
    pub(crate) for_someone_else: bool,
    pub(crate) removal: Removal,
    /// Each album folder's art as its thumbnail was made from it: the
    /// folder's path hash → the art's key (2.12.1, step 10; [`art_key`]),
    /// [`FOLDER_IMAGE`] for a folder's image with no art named, or
    /// [`NO_ART`] for an album looked at with nothing to make one from.
    pub(crate) art: BTreeMap<u64, u64>,
    /// The members are written: the file had them, or a sync stored them.
    /// A set-up's state file has none.
    pub(crate) stored: bool,
    /// `conversion` and `autoDj` as read, their unknown members kept; and
    /// under `sources` the entries this software can't read (no key it
    /// knows), kept whole.
    raw: Kept,
}

/// Members a file held that this software doesn't read, carried into its
/// next write. They aren't what the settings are: two settings that differ
/// only in them are equal.
#[derive(Clone, Debug, Default)]
struct Kept(Map<String, Value>);

impl PartialEq for Kept {
    fn eq(&self, _: &Kept) -> bool {
        true
    }
}

/// Section 6's defaults, not stored.
impl Default for Settings {
    fn default() -> Settings {
        Settings {
            sources: Vec::new(),
            layout: Layout::Auto,
            conversion: Conversion::default(),
            covers: true,
            auto_dj: true,
            for_someone_else: false,
            removal: Removal::Mirror,
            art: BTreeMap::new(),
            stored: false,
            raw: Kept::default(),
        }
    }
}

impl Settings {
    /// Section 6's defaults, to be stored.
    pub(crate) fn defaults() -> Settings {
        Settings { stored: true, ..Settings::default() }
    }

    /// The settings a state file's object holds; the defaults for each
    /// member it doesn't, and for a value this software doesn't read.
    fn read(object: &Map<String, Value>) -> Settings {
        let mut s = Settings::defaults();
        s.stored = SETTINGS.iter().any(|m| object.contains_key(*m));
        let word = |key: &str| object.get(key).and_then(Value::as_str);
        let sub = |key: &str| object.get(key).and_then(Value::as_object).cloned();
        let sub = |key: &str| sub(key).unwrap_or_default();
        s.layout = word("layout").and_then(Layout::of_word).unwrap_or_default();
        s.removal = match word("removal") {
            Some("never") => Removal::Never,
            _ => Removal::Mirror,
        };
        s.covers = object.get("covers").and_then(Value::as_bool).unwrap_or(true);
        let conversion = sub("conversion");
        let words = |key: &str| conversion.get(key).and_then(Value::as_str);
        let kbps = conversion.get("kbps").and_then(Value::as_u64);
        let kbps = kbps.and_then(|k| u32::try_from(k).ok());
        s.conversion = Conversion {
            convert: words("convert").and_then(Convert::of_word).unwrap_or_default(),
            codec: words("codec").and_then(|c| c.parse().ok()).unwrap_or(TransferCodec::Opus),
            bitrate: kbps.and_then(TransferBitrate::from_kbps).unwrap_or(TransferBitrate::K192),
        };
        let auto_dj = sub("autoDj");
        let flag = |key: &str| auto_dj.get(key).and_then(Value::as_bool);
        s.auto_dj = flag("on").unwrap_or(true);
        s.for_someone_else = flag("forSomeoneElse") == Some(true);
        s.raw.0.insert("conversion".into(), Value::Object(conversion));
        s.raw.0.insert("autoDj".into(), Value::Object(auto_dj));
        for (folder, key) in object.get("art").and_then(Value::as_object).into_iter().flatten() {
            if let (Some(folder), Some(key)) = (from_hex(folder), key.as_str().and_then(from_hex)) {
                s.art.insert(folder, key);
            }
        }
        let sources = object.get("sources").and_then(Value::as_array).into_iter().flatten();
        let mut others = Vec::new();
        for item in sources {
            let key = item.get("serverUrlKey").and_then(Value::as_str).and_then(from_hex);
            let (Some(raw), Some(url_key)) = (item.as_object(), key) else {
                others.push(item.clone());
                continue;
            };
            let text = |key: &str| raw.get(key).and_then(Value::as_str);
            let strings = |key: &str| -> Vec<String> {
                let items = raw.get(key).and_then(Value::as_array).into_iter().flatten();
                items.filter_map(Value::as_str).map(str::to_string).collect()
            };
            let number = |key: &str| raw.get(key).and_then(Value::as_u64).unwrap_or(0);
            s.sources.push(Source {
                url_key,
                nickname: text("nickname").filter(|n| !looks_like_address(n)).unwrap_or("").into(),
                libraries: strings("libraries"),
                confirmed: strings("confirmed").iter().filter_map(|k| from_hex(k)).collect(),
                revision: text("revision").unwrap_or_default().to_string(),
                plan_key: text("planKey").and_then(from_hex).unwrap_or(0),
                full_read: number("fullRead"),
                runs: u32::try_from(number("runs")).unwrap_or(u32::MAX),
                raw: Kept(raw.clone()),
            });
        }
        s.raw.0.insert("sources".into(), Value::Array(others));
        s
    }

    /// The source of a server, by its URL's key; made when there is none.
    pub(crate) fn source(&mut self, url_key: u64) -> &mut Source {
        match self.sources.iter().position(answers(url_key)) {
            Some(at) => &mut self.sources[at],
            None => {
                self.sources.push(Source { url_key, ..Source::default() });
                self.sources.last_mut().expect("just pushed")
            }
        }
    }

    /// The server at `url_key`'s other keys: its source's own and those
    /// confirmed as it, the ones a card's server may have for it (2.11's
    /// key rule).
    pub(crate) fn keys(&self, url_key: u64) -> Vec<u64> {
        let Some(source) = self.sources.iter().find(|s| answers(url_key)(s)) else {
            return Vec::new();
        };
        let mut keys = Vec::new();
        for key in std::iter::once(source.url_key).chain(source.confirmed.iter().copied()) {
            if key != 0 && key != url_key && !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// The listener says the server at `url_key` is the one at `other`:
    /// one source answers both from now on, `other`'s when there is one,
    /// which takes the keys of any source `url_key` had of its own (its
    /// selection and schedule give way to `other`'s).
    pub(crate) fn confirm(&mut self, url_key: u64, other: u64) {
        if self.sources.iter().all(|s| !answers(other)(s)) {
            let source = self.source(url_key);
            if !source.confirmed.contains(&other) {
                source.confirmed.push(other);
            }
            return;
        }
        let mut keys = vec![url_key];
        let mine = self.sources.iter().position(|s| answers(url_key)(s) && !answers(other)(s));
        if let Some(mine) = mine {
            let gone = self.sources.remove(mine);
            keys.push(gone.url_key);
            keys.extend(gone.confirmed);
        }
        let source = self.source(other);
        for key in keys {
            if key != 0 && key != source.url_key && !source.confirmed.contains(&key) {
                source.confirmed.push(key);
            }
        }
    }

    /// What a plan depends on beside the server's files, as a key: the
    /// selection, the layout, the conversion, and what the player plays. A
    /// 304 stands for the last whole listing only under the same key.
    pub(crate) fn plan_key(&self, libraries: &[String], device: &DeviceTxt) -> u64 {
        let c = self.conversion;
        let mut libraries = libraries.to_vec();
        libraries.sort();
        let text = format!(
            "{}\n{} {} {} {}\n{} {} {} {}",
            libraries.join("\u{1f}"),
            self.layout.word(),
            c.convert.word(),
            c.codec.as_str(),
            c.bitrate.as_str(),
            device.codecs,
            device.extensions,
            device.max_rate,
            device.max_channels,
        );
        super::contract::prim::fnv1a64(text.as_bytes())
    }

    /// The members as written, in schema 1's order; none for settings never
    /// stored.
    fn members(&self) -> Vec<(&'static str, Value)> {
        if !self.stored {
            return Vec::new();
        }
        let merged = |key: &str, fields: Vec<(&str, Value)>| {
            let old = self.raw.0.get(key).and_then(Value::as_object);
            let mut object = old.cloned().unwrap_or_default();
            for (name, value) in fields {
                object.insert(name.to_string(), value);
            }
            Value::Object(object)
        };
        let mut sources: Vec<Value> = self.sources.iter().map(Source::object).collect();
        if let Some(Value::Array(others)) = self.raw.0.get("sources") {
            sources.extend(others.iter().cloned());
        }
        let c = self.conversion;
        let conversion = merged(
            "conversion",
            vec![
                ("convert", c.convert.word().into()),
                ("codec", c.codec.as_str().into()),
                ("kbps", c.bitrate.kbps().into()),
            ],
        );
        let auto_dj = merged(
            "autoDj",
            vec![("on", self.auto_dj.into()), ("forSomeoneElse", self.for_someone_else.into())],
        );
        let removal = match self.removal {
            Removal::Mirror => "mirror",
            Removal::Never => "never",
        };
        let art = self.art.iter().map(|(f, k)| (hex(*f), hex(*k).into()));
        let art: Map<String, Value> = art.collect();
        vec![
            ("sources", Value::Array(sources)),
            ("layout", self.layout.word().into()),
            ("conversion", conversion),
            ("covers", self.covers.into()),
            ("autoDj", auto_dj),
            ("removal", removal.into()),
            ("art", Value::Object(art)),
        ]
    }
}

impl Source {
    fn object(&self) -> Value {
        let mut object = self.raw.0.clone();
        let nickname = if looks_like_address(&self.nickname) { "" } else { self.nickname.as_str() };
        let mut set = |name: &str, value: Value| {
            object.insert(name.to_string(), value);
        };
        set("serverUrlKey", hex(self.url_key).into());
        set("nickname", nickname.into());
        set("libraries", self.libraries.clone().into());
        set("confirmed", self.confirmed.iter().map(|k| hex(*k)).collect::<Vec<_>>().into());
        set("revision", self.revision.as_str().into());
        set("planKey", hex(self.plan_key).into());
        set("fullRead", self.full_read.into());
        set("runs", self.runs.into());
        Value::Object(object)
    }
}

/// A source answers a key: its own, or one confirmed as it.
fn answers(key: u64) -> impl Fn(&Source) -> bool {
    move |s| s.url_key == key || s.confirmed.contains(&key)
}

/// [`art_key`] of an album whose tracks name no art.
pub(crate) const NO_ART: u64 = 0;
/// Not a key [`art_key`] gives: the `art` entry of an album whose thumbnail
/// was made from its folder's image while mStream named no art.
pub(crate) const FOLDER_IMAGE: u64 = 1;

/// An album's art as the server names it: the key of its tracks' distinct
/// `album-art` names (mStream's cache names, which change with the image),
/// or [`NO_ART`]. The thumbnail is made again when it changes (2.12.1,
/// step 10).
pub(crate) fn art_key<'a>(names: impl IntoIterator<Item = &'a str>) -> u64 {
    let mut names: Vec<&str> = names.into_iter().filter(|n| !n.is_empty()).collect();
    names.sort_unstable();
    names.dedup();
    match names.is_empty() {
        true => NO_ART,
        false => super::contract::prim::fnv1a64(names.join("\n").as_bytes()).max(FOLDER_IMAGE + 1),
    }
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
        assert_eq!(state.keep.keys().collect::<Vec<_>>(), ["zz"]);
        let home = &state.settings.sources[0];
        assert_eq!((home.url_key, home.nickname.as_str()), (0xED90_1EA3_EE76_3AC7, "Home"));
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
    fn the_syncs_settings_read_back_and_keep_what_they_dont_know() {
        // A set-up's state file stores no settings, and reads as the defaults.
        let fat = Serial::Fat(0x1A2B_3C4D);
        let set_up = State::new(9, "Car", Some(&fat), 1);
        let text = String::from_utf8(set_up.text(1)).unwrap();
        assert!(!text.contains("sources") && !text.contains("conversion"), "{text}");
        let read_back = read(text.as_bytes()).unwrap();
        assert!(!read_back.settings.stored);
        let s = read_back.settings;
        let defaults = (s.layout, s.conversion, s.covers, s.auto_dj, s.for_someone_else, s.removal);
        let section6 = (Layout::Auto, Conversion::default(), true, true, false, Removal::Mirror);
        assert_eq!(defaults, section6);

        // A sync's: every member, read back as written.
        let mut state = State::new(9, "Car", Some(&fat), 1);
        state.settings = Settings::defaults();
        let s = &mut state.settings;
        (s.layout, s.removal, s.covers, s.for_someone_else) = (Layout::Vpath, Removal::Never, false, true);
        s.conversion = Conversion {
            convert: Convert::Everything,
            codec: TransferCodec::Mp3,
            bitrate: TransferBitrate::K128,
        };
        s.art.insert(0xB1F7_E69F_BD46_6B59, 0x0123_4567_89AB_CDEF);
        let home = s.source(0xED90_1EA3_EE76_3AC7);
        home.nickname = "Home".into();
        home.libraries = vec!["Rock".into(), "Jazz".into()];
        (home.full_read, home.runs, home.plan_key) = (1_791_900_000, 3, 0x42);
        let text = String::from_utf8(state.text(2)).unwrap();
        let again = read(text.as_bytes()).unwrap();
        assert_eq!(again.settings, state.settings, "{text}");
        assert!(text.contains("\"0xB1F7E69FBD466B59\":\"0x0123456789ABCDEF\""), "{text}");
        // Another address of the same server is the same source once confirmed.
        let mut s = again.settings.clone();
        s.sources[0].confirmed.push(0x77);
        assert_eq!(s.source(0x77).nickname, "Home");
        assert_eq!(s.sources.len(), 1);

        // A newer software's members, at the top and inside the settings'
        // objects, stay; a nickname that is an address is never written.
        let newer = r#"{"format": "mstream-card-state", "version": 1, "seq": 4, "cardId": "0x09",
            "label": "Car", "created": 1, "layout": "plain", "removal": "mirror",
            "conversion": {"convert": "everything", "codec": "opus", "kbps": 96, "dither": "tpdf"},
            "autoDj": {"on": false, "forSomeoneElse": false, "seed": 7},
            "sources": [{"serverUrlKey": "0x01", "nickname": "music.example:3000", "libraries": ["Rock"],
                         "peers": [3]}, {"nickname": "no key: no source"}],
            "art": {"0x02": "0x03", "bad": "0x04"}, "covers": true, "zz": 1}"#;
        let mut state = read(newer.as_bytes()).unwrap();
        let s = &state.settings;
        assert!(s.stored);
        assert_eq!((s.layout, s.conversion.bitrate, s.auto_dj), (Layout::Plain, TransferBitrate::K96, false));
        assert_eq!(s.sources.len(), 1);
        assert_eq!(s.sources[0].nickname, "", "an address is no nickname");
        assert_eq!(s.art, BTreeMap::from([(2, 3)]));
        state.settings.sources[0].nickname = "alex-box:3000".into();
        state.settings.conversion.bitrate = TransferBitrate::K64;
        let text = String::from_utf8(state.text(5)).unwrap();
        let v: Value = serde_json::from_str(&text).unwrap();
        let kept = (&v["conversion"]["dither"], &v["autoDj"]["seed"], &v["sources"][0]["peers"], &v["zz"]);
        assert_eq!(kept, (&"tpdf".into(), &7.into(), &serde_json::json!([3]), &1.into()), "{text}");
        assert_eq!(v["sources"][1]["nickname"], "no key: no source", "an entry it can't read, kept");
        assert!(!text.contains("music.example") && !text.contains("alex-box"), "{text}");
        assert_eq!(read(text.as_bytes()).unwrap().settings.conversion.bitrate, TransferBitrate::K64);

        // The plan's key follows the settings, the selection and the player.
        let device = DeviceTxt::defaults();
        let plain = Settings::defaults();
        let rock = vec!["Rock".to_string()];
        let key = plain.plan_key(&rock, &device);
        assert_eq!(key, plain.plan_key(&rock, &device));
        assert_ne!(key, plain.plan_key(&["Jazz".to_string()], &device));
        let mut opus = device.clone();
        opus.codecs = "mp3,flac,opus".into();
        assert_ne!(key, plain.plan_key(&rock, &opus));
        let mut mp3 = plain.clone();
        mp3.conversion.codec = TransferCodec::Mp3;
        assert_ne!(key, mp3.plan_key(&rock, &device));

        assert_eq!(art_key(["b.jpg", "a.jpg", "b.jpg", ""]), art_key(["a.jpg", "b.jpg"]));
        assert_ne!(art_key(["a.jpg"]), art_key(["b.jpg"]));
        assert_eq!(art_key(Vec::<&str>::new()), NO_ART);
        assert!(art_key(["a.jpg"]) > FOLDER_IMAGE);
    }

    #[test]
    fn a_server_confirmed_at_another_address_is_one_source() {
        let mut s = Settings::defaults();
        s.source(0xA).libraries = vec!["Rock".into()];
        assert_eq!(s.keys(0xA), Vec::<u64>::new());
        // A second address, said to be the first's server: each answers
        // for the other.
        s.confirm(0xB, 0xA);
        assert_eq!((s.sources.len(), s.keys(0xB), s.keys(0xA)), (1, vec![0xA], vec![0xB]));
        assert_eq!(s.source(0xB).libraries, ["Rock"]);
        // A third that had a source of its own: its keys join the first's.
        s.source(0xC).libraries = vec!["Jazz".into()];
        s.sources[1].confirmed.push(0xD);
        s.confirm(0xC, 0xB);
        assert_eq!((s.sources.len(), s.keys(0xD)), (1, vec![0xA, 0xB, 0xC]));
        assert_eq!(s.source(0xC).libraries, ["Rock"]);
        // A card's server with no source on it: this address's source
        // takes its key.
        let mut fresh = Settings::defaults();
        fresh.confirm(0xE, 0xF);
        assert_eq!((fresh.sources[0].url_key, fresh.keys(0xE)), (0xE, vec![0xF]));
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
