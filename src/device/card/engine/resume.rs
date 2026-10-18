//! Downloads a later run takes up (docs/METADATA.md 2.12.8). A file whose
//! source says so — a peer's, whose daily quota a second fetch would spend
//! again — is fetched into `/.mstream/resume/<8 hex>.part` from its first
//! byte instead of `stage/`, and listed in `resume.json` (2.12.7's private
//! file) once its first `have` bytes are flushed: its server, its server
//! path, the size it will have, the ETag those bytes came under and
//! mStream's `modified`. Step 4 kept the listed ones on local facts alone
//! (settle's); a run takes one up only when the server's entry still names
//! the file it was listed for, cut back to what was flushed, and asks for
//! the rest under If-Range — a changed file starts over — then renames it,
//! finished, straight into `/music`. One no plan wants any more goes at the
//! end (step 13). The device never reads any of it.
//!
//! Here: the run's table of `resume.json` ([`Parts`]) and its text, and the
//! rule that says where a listed partial goes on from ([`take_up`]).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::super::contract::prim::hex16;
use super::super::contract::server::{NO_INSTANCE, PEER, Server, Servers, Talk, Verdict};
use super::super::state::from_hex;
use super::super::volume;
use crate::admin::tz::{civil_from_days, weekday};

/// An entry's members this software reads and writes, in the order it
/// writes them; any other is kept as it was.
const KNOWN: [&str; 7] = ["name", "server", "serverPath", "size", "etag", "modified", "have"];

/// One partial of `resume.json`, as read: what this software doesn't know
/// of it is written back unchanged.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Entry {
    /// Its name in `resume/`.
    pub(super) name: String,
    /// The server its bytes come from; None: not one this software can
    /// read, whose partial no plan wants.
    pub(super) server: Option<Server>,
    pub(super) server_path: Option<String>,
    /// The size the file will have: the server's `file-size`.
    pub(super) size: u64,
    pub(super) modified: Option<u64>,
    /// None when the entry isn't sound (2.12.8): `have` not an integer from
    /// 0 to `size`, `etag` not a string. Its partial then starts over.
    pub(super) have: Option<u64>,
    pub(super) etag: Option<String>,
    /// Every member as read, or as this run set it.
    members: Map<String, Value>,
}

impl Entry {
    /// An entry of a valid `resume.json`: one with a name and a size (step
    /// 4 keeps no partial of any other).
    fn read(value: &Value) -> Option<Entry> {
        let members = value.as_object()?.clone();
        let name = members.get("name")?.as_str()?.to_string();
        let size = members.get("size")?.as_u64()?;
        let text = |key: &str| members.get(key).and_then(Value::as_str).map(String::from);
        Some(Entry {
            server: members.get("server").and_then(server_of),
            server_path: text("serverPath"),
            modified: members.get("modified").and_then(Value::as_u64),
            have: members.get("have").and_then(Value::as_u64).filter(|&have| have <= size),
            etag: text("etag"),
            name,
            size,
            members,
        })
    }

    /// A new partial's entry: nothing of it flushed yet, no ETag yet.
    pub(super) fn new(
        name: &str,
        server: Server,
        server_path: &str,
        size: u64,
        modified: u64,
    ) -> Entry {
        let mut entry = Entry {
            name: name.to_string(),
            server: Some(server),
            server_path: Some(server_path.to_string()),
            size,
            modified: Some(modified),
            have: Some(0),
            etag: Some(String::new()),
            members: Map::new(),
        };
        entry.members.insert("name".into(), name.into());
        entry.members.insert("server".into(), server_value(&server));
        entry.members.insert("serverPath".into(), server_path.into());
        entry.members.insert("size".into(), size.into());
        entry.members.insert("modified".into(), modified.into());
        entry.set(0, "");
        entry
    }

    /// Its first `have` bytes are on the card, fetched under `etag`.
    fn set(&mut self, have: u64, etag: &str) {
        (self.have, self.etag) = (Some(have), Some(etag.to_string()));
        self.members.insert("have".into(), have.into());
        self.members.insert("etag".into(), etag.into());
    }

    /// The entry as `resume.json` holds it: the members this software
    /// writes in their order, then the others.
    fn text(&self) -> String {
        let json = |v: &Value| serde_json::to_string(v).unwrap_or_default();
        let known = KNOWN.iter().filter_map(|k| self.members.get(*k).map(|v| (*k, v)));
        let others = self.members.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str()));
        let others = others.map(|(k, v)| (k.as_str(), v));
        let members: Vec<String> =
            known.chain(others).map(|(k, v)| format!("{}: {}", json(&k.into()), json(v))).collect();
        format!("{{{}}}", members.join(", "))
    }
}

/// An SRVS entry's fields as `resume.json` spells them (2.12.8, 2.5.5):
/// `serverInstance` as UUID text (`""` unknown), `serverUrlKey` and
/// `parentKey` as hex strings (`"0x0"` none), `peerId` (0 none).
fn server_of(value: &Value) -> Option<Server> {
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    let instance = match text("serverInstance")? {
        "" => NO_INSTANCE,
        uuid => hex16(uuid)?,
    };
    let url_key = from_hex(text("serverUrlKey")?)?;
    let parent_key = from_hex(text("parentKey")?)?;
    let peer_id = u32::try_from(value.get("peerId")?.as_u64()?).ok()?;
    let flags = if parent_key != 0 || peer_id != 0 { PEER } else { 0 };
    Some(Server { instance, url_key, parent_key, peer_id, flags })
}

fn server_value(server: &Server) -> Value {
    let key = |k: u64| if k == 0 { "0x0".to_string() } else { format!("0x{k:016X}") };
    let mut members = Map::new();
    members.insert("serverInstance".into(), uuid_text(&server.instance).into());
    members.insert("serverUrlKey".into(), key(server.url_key).into());
    members.insert("parentKey".into(), key(server.parent_key).into());
    members.insert("peerId".into(), server.peer_id.into());
    Value::Object(members)
}

/// An instance as UUID text, lowercase with its hyphens; `""` unknown.
fn uuid_text(id: &[u8; 16]) -> String {
    if *id == NO_INSTANCE {
        return String::new();
    }
    let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// 2.11's test of a partial's server against a server the run reads, with
/// `card` the card's servers as the run found them: the same, by the
/// instances when both are known, else by the key rule; a peer's parent
/// the card's server of its parentKey (known by its instance, it passes at
/// a new URL, and with another it fails), else parentKey by the key rule.
pub(super) fn same_server(card: &Servers, server: &Server, talk: &Talk) -> bool {
    card.same_as(server, talk) == Verdict::Same
}

/// Where a listed partial goes on from at fetch time (2.12.8), once its
/// server and server path are found to be the file's: the smaller of its
/// size on the card and `have`, and the validator the rest is asked for
/// under (If-Range: its ETag, else the HTTP date of `modified`) — when its
/// entry is sound and the server's entry still has the size and `modified`
/// it was listed with. None: it starts over.
pub(super) fn take_up(
    entry: &Entry,
    part_size: u64,
    file_size: u64,
    modified: u64,
) -> Option<(u64, String)> {
    let (have, etag) = (entry.have?, entry.etag.as_deref()?);
    if entry.size != file_size || entry.modified != Some(modified) {
        return None;
    }
    let validator = if etag.is_empty() { http_date(modified) } else { etag.to_string() };
    Some((part_size.min(have), validator))
}

/// An HTTP date (RFC 9110's IMF-fixdate) of a time in Unix ms, its seconds
/// as they are: 1790000000123 is `Mon, 21 Sep 2026 14:13:20 GMT`.
pub(super) fn http_date(ms: u64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (weekday, month) = (DAYS[weekday(days) as usize], MONTHS[month as usize - 1]);
    let (h, m, s) = (rem / 3600, rem / 60 % 60, rem % 60);
    format!("{weekday}, {day:02} {month} {year:04} {h:02}:{m:02}:{s:02} GMT")
}

/// `resume.json` as a run keeps it: the entries of the partials step 4
/// left, and what the next write of the file says.
#[derive(Clone, Debug, Default)]
pub(crate) struct Parts {
    /// The settled file's seq (0: none was valid), and its top-level members
    /// this software doesn't know, kept.
    seq: u64,
    others: Vec<(String, Value)>,
    entries: Vec<Entry>,
    /// Every name the card's `resume.json` lists or `resume/` holds: a new
    /// partial is named none of them, so no listing on the card ever names
    /// bytes it didn't list.
    taken: BTreeSet<String>,
    /// The entries aren't what the card's file says.
    dirty: bool,
}

impl Parts {
    /// The table of a settled `resume.json` (`value`, its `seq`) over the
    /// partials step 4 kept (`files`, name to size): an entry whose file is
    /// gone is dropped, which the next write says (2.12.8).
    pub(super) fn read(value: Option<&Value>, seq: u64, files: &BTreeMap<String, u64>) -> Parts {
        let mut parts = Parts { seq, ..Parts::default() };
        parts.taken.extend(files.keys().cloned());
        let Some(object) = value.and_then(Value::as_object) else { return parts };
        let fixed = ["format", "version", "seq", "parts"];
        let others = object.iter().filter(|(k, _)| !fixed.contains(&k.as_str()));
        parts.others = others.map(|(k, v)| (k.clone(), v.clone())).collect();
        let listed = object.get("parts").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for item in listed {
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                parts.taken.insert(name.to_string());
            }
            match Entry::read(item).filter(|e| files.contains_key(&e.name)) {
                Some(entry) => parts.entries.push(entry),
                None => parts.dirty = true,
            }
        }
        parts
    }

    pub(super) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub(super) fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The bytes an entry says are flushed: 0 for none, or an unsound one.
    pub(super) fn have(&self, name: &str) -> u64 {
        self.get(name).and_then(|e| e.have).unwrap_or(0)
    }

    /// A partial's entry holds what it does now: the first `have` bytes on
    /// the card, under `etag`.
    pub(super) fn set(&mut self, name: &str, have: u64, etag: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name)
            && (e.have, e.etag.as_deref()) != (Some(have), Some(etag))
        {
            e.set(have, etag);
            self.dirty = true;
        }
    }

    pub(super) fn add(&mut self, entry: Entry) {
        self.taken.insert(entry.name.clone());
        self.entries.retain(|e| e.name != entry.name);
        self.entries.push(entry);
        self.dirty = true;
    }

    /// An entry gone: its partial deleted, or renamed into `/music`.
    pub(super) fn remove(&mut self, name: &str) {
        let before = self.entries.len();
        self.entries.retain(|e| e.name != name);
        self.dirty |= self.entries.len() != before;
    }

    /// A name for a new partial (2.12.8): 8 lowercase hex digits, one past
    /// the highest taken, then `.part`.
    pub(super) fn fresh_name(&mut self) -> String {
        let number = |n: &String| {
            n.strip_suffix(".part").and_then(|hex| u32::from_str_radix(hex, 16).ok())
        };
        let mut next = self.taken.iter().filter_map(number).max().map_or(1, |n| n.wrapping_add(1));
        let name = loop {
            let name = format!("{next:08x}.part");
            if !self.taken.contains(&name) {
                break name;
            }
            next = next.wrapping_add(1);
        };
        self.taken.insert(name.clone());
        name
    }

    pub(super) fn dirty(&self) -> bool {
        self.dirty
    }

    /// The file is written again at the run's next chance, whatever moved.
    pub(super) fn rewrite(&mut self) {
        self.dirty = true;
    }

    /// The seq the next write gives the file.
    pub(super) fn next_seq(&self) -> u64 {
        volume::next_seq(self.seq)
    }

    /// The text of the next `resume.json` (2.12.7's form, 2.12.8's members):
    /// its seq one more than the file's, the members this software doesn't
    /// know kept at the top and in every entry.
    pub(super) fn text(&self) -> Vec<u8> {
        let json = |v: &Value| serde_json::to_string(v).unwrap_or_default();
        let mut lines = vec![
            "  \"format\": \"mstream-card-resume\"".to_string(),
            "  \"version\": 1".to_string(),
            format!("  \"seq\": {}", self.next_seq()),
        ];
        let member = |(k, v): &(String, Value)| {
            format!("  {}: {}", json(&k.as_str().into()), json(v))
        };
        lines.extend(self.others.iter().map(member));
        let entries = self.entries.iter().map(|e| format!("    {}", e.text()));
        let entries: Vec<String> = entries.collect();
        lines.push(match entries.is_empty() {
            true => "  \"parts\": []".to_string(),
            false => format!("  \"parts\": [\n{}\n  ]", entries.join(",\n")),
        });
        format!("{{\n{}\n}}\n", lines.join(",\n")).into_bytes()
    }

    /// The file on the card says what the table does, at the next seq.
    pub(super) fn written(&mut self) {
        self.seq = self.next_seq();
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::contract::server::TalkPeer;
    use super::super::super::volume::{RESUME_VERSION, settle_private};
    use super::*;

    fn vectors() -> Value {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = root.join("test/fixtures/card/vectors.json");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// vectors.json's privateFiles.resumeFetch: where a listed partial goes
    /// on from, and the request's Range and If-Range, or that it starts over
    /// without asking. (A 200 to the ranged ask starting it over is the
    /// fetch's: A6's stream_part.)
    #[test]
    fn a_listed_partial_goes_on_from_have_or_starts_over_as_the_vectors_say() {
        let v = vectors();
        let cases = v["privateFiles"]["resumeFetch"].as_array().unwrap();
        assert_eq!(cases.len(), 11);
        for c in cases {
            let note = c["note"].as_str().unwrap();
            let entry = Entry::read(&c["entry"]).unwrap_or_else(|| panic!("{note}: an entry"));
            let server = &c["server"];
            assert_eq!(entry.server_path.as_deref(), server["serverPath"].as_str(), "{note}");
            let size = server["file-size"].as_u64().unwrap();
            let modified = server["modified"].as_u64().unwrap();
            let got = take_up(&entry, c["partSize"].as_u64().unwrap(), size, modified);
            match c["headers"].as_object() {
                None => assert_eq!(got, None, "{note}: starts over without asking"),
                Some(headers) => {
                    let (from, validator) = got.unwrap_or_else(|| panic!("{note}: taken up"));
                    assert_eq!(Some(from), c["from"].as_u64(), "{note}");
                    assert_eq!(headers["Range"], format!("bytes={from}-"), "{note}");
                    assert_eq!(headers["If-Range"], validator.as_str(), "{note}");
                }
            }
        }
    }

    #[test]
    fn an_http_date_is_the_contracts() {
        assert_eq!(http_date(1_790_000_000_123), "Mon, 21 Sep 2026 14:13:20 GMT");
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(http_date(951_782_400_999), "Tue, 29 Feb 2000 00:00:00 GMT");
    }

    /// vectors.json's privateFiles.resumeExample: read over the partials a
    /// card holds, written back valid at the next seq, every member this
    /// software doesn't know kept, and an entry whose partial is gone
    /// dropped.
    #[test]
    fn the_resume_example_reads_and_writes_back_what_it_does_not_know() {
        let v = vectors();
        let mut example = v["privateFiles"]["resumeExample"].clone();
        example["note"] = "a newer software's".into();
        example["parts"][0]["hint"] = serde_json::json!({"from": "a newer software"});
        let files = [("0000002a.part", 10_485_760), ("0000002c.part", 100)];
        let files: BTreeMap<String, u64> = files.map(|(n, size)| (n.to_string(), size)).into();
        let mut parts = Parts::read(Some(&example), 7, &files);
        let names: Vec<&str> = parts.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["0000002a.part", "0000002c.part"], "0000002b's partial is gone");
        assert!(parts.dirty(), "which the next write says");
        let peer = parts.get("0000002a.part").unwrap();
        assert_eq!(peer.server, Some(Server::peer(NO_INSTANCE, 0xED90_1EA3_EE76_3AC7, 3)));
        let etag = "W/\"1e00000-19a1b2c3d4e\"";
        assert_eq!((peer.have, peer.etag.as_deref()), (Some(10_485_760), Some(etag)));
        assert_eq!(parts.get("0000002c.part").unwrap().have, None, "have past size: unsound");

        parts.set("0000002a.part", 12_000_000, "W/\"1e00000-19a1b2c3d4f\"");
        let text = parts.text();
        let settled = settle_private(Some(&text), None, "resume", RESUME_VERSION);
        let value = settled.value.expect("a valid resume.json");
        assert_eq!(settled.seq, 8);
        assert_eq!(value["note"], "a newer software's");
        let entry = &value["parts"][0];
        assert_eq!(entry["hint"]["from"], "a newer software");
        let etag = "W/\"1e00000-19a1b2c3d4f\"";
        let said = (entry["have"].as_u64(), entry["etag"].as_str());
        assert_eq!(said, (Some(12_000_000), Some(etag)));
        assert_eq!(entry["server"], v["privateFiles"]["resumeExample"]["parts"][0]["server"]);
        // The unsound entry written back as it was, have and all.
        assert_eq!(value["parts"][1], v["privateFiles"]["resumeExample"]["parts"][2]);
        let again = Parts::read(Some(&value), settled.seq, &files);
        assert_eq!(again.entries().len(), 2);
        assert!(!again.dirty());

        // A new partial is named past every name the card shows, listed or
        // not.
        let mut parts = Parts::read(Some(&example), 7, &files);
        assert_eq!(parts.fresh_name(), "0000002d.part");
        assert_eq!(parts.fresh_name(), "0000002e.part");
        let server = Server::direct([0x11; 16], 0x42);
        parts.add(Entry::new("0000002e.part", server, "Lib/A/01.flac", 9, 1_790_000_000_000));
        let value: Value = serde_json::from_slice(&parts.text()).unwrap();
        let written = &value["parts"][2];
        let want = serde_json::json!({
            "serverInstance": "11111111-1111-1111-1111-111111111111",
            "serverUrlKey": "0x0000000000000042",
            "parentKey": "0x0",
            "peerId": 0,
        });
        assert_eq!(written["server"], want);
        let read = Entry::read(written).unwrap();
        let read = (read.server, read.have, read.etag.as_deref());
        assert_eq!(read, (Some(server), Some(0), Some("")));
    }

    /// 2.11's test of a partial's server against the card's servers: a
    /// peer's parent is the card's server of its parentKey, known by its
    /// instance at a new URL and failing with another instance whatever the
    /// key; with no such server on the card, parentKey by the key rule
    /// alone. A server reached by its own URL is tested as before.
    #[test]
    fn a_peer_partials_parent_is_the_cards_server_of_its_key() {
        let (old, new, instance) = (0x0A0A, 0x0B0B, [0x11; 16]);
        let card = Servers { root: Server::root(instance, old), entries: Vec::new() };
        let partial = Server::peer(NO_INSTANCE, old, 3);
        let talk = |url_key: u64, instance: [u8; 16], id: u32| Talk {
            url_key,
            instance,
            confirmed: Vec::new(),
            peer: Some(TalkPeer { id, instance: NO_INSTANCE }),
        };
        assert!(same_server(&card, &partial, &talk(new, instance, 3)), "its parent at a new URL");
        assert!(!same_server(&card, &partial, &talk(old, [0x22; 16], 3)), "another at its URL");
        assert!(!same_server(&card, &partial, &talk(old, instance, 4)), "another peer of it");
        let bare = Servers::default();
        assert!(same_server(&bare, &partial, &talk(old, [0x22; 16], 3)), "the key rule alone");
        assert!(!same_server(&bare, &partial, &talk(new, instance, 3)));
        let own = Talk { peer: None, ..talk(old, NO_INSTANCE, 0) };
        assert!(same_server(&card, &Server::direct(NO_INSTANCE, old), &own));
        assert!(!same_server(&card, &partial, &own), "reading a server isn't reading its peer");
    }

    /// A table read at the last seq (2^53 − 1) writes its next file at 1,
    /// valid, and goes on from there.
    #[test]
    fn the_write_after_the_last_seq_is_at_one() {
        let last = (1u64 << 53) - 1;
        assert_eq!((volume::next_seq(0), volume::next_seq(last - 1)), (1, last));
        let value = serde_json::json!({
            "format": "mstream-card-resume", "version": 1, "seq": last, "parts": [],
        });
        let mut parts = Parts::read(Some(&value), last, &BTreeMap::new());
        let settled = settle_private(Some(&parts.text()), None, "resume", RESUME_VERSION);
        assert_eq!((settled.value.is_some(), settled.seq), (true, 1));
        parts.written();
        assert_eq!(parts.next_seq(), 2);
    }
}
