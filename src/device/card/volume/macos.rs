//! macOS: the volumes as `diskutil … -plist` describes them, and statfs
//! for a mounted card's space.
//!
//! - **The system's disks:** the whole disk under `/` (`ParentWholeDisk`,
//!   APFS's synthesized container) and every APFS container's physical
//!   stores, the startup disk among them.
//! - **A card:** RemovableMedia or Ejectable, or a disk whose protocol is
//!   USB or Secure Digital. Never by `Internal` false alone: a Mac's
//!   built-in SD slot may report itself internal, and a Thunderbolt disk
//!   reports itself external.
//! - **A disk of its own:** no other partition on it but FAT ones that
//!   aren't mounted.
//! - **The rest:** the partition scheme (FDisk is MBR), the filesystem and
//!   its FAT width, the volume UUID as the serial's stand-in (diskutil
//!   doesn't show the FAT serial, and the raw disk needs root), and the
//!   platform's local offset as the FAT clock (docs/METADATA.md 2.3.4).
//!
//! [`found`] is the rule over a [`Diskutil`], which the tests fake with
//! made-up plists; [`find`] runs it with `/usr/sbin/diskutil`. [`parse`]
//! reads the XML property lists diskutil writes, nothing more.

use std::collections::HashMap;

use super::{Bus, Clock, Disk, Found, Fs, Serial, Space, Table, Volume};

/// diskutil's answers, and statfs.
pub(crate) trait Diskutil {
    /// `diskutil list -plist`.
    fn list(&self) -> Option<String>;
    /// `diskutil info -plist <what>`: a disk, a partition or a mount point.
    fn info(&self, what: &str) -> Option<String>;
    fn space(&self, mount: &str) -> Option<Space>;
}

/// A property list's value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    Str(String),
    Int(i64),
    Bool(bool),
    /// A real, a date, data: kept as their text.
    Other(String),
}

impl Plist {
    fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A key's string, None when it is missing or empty.
    fn text(&self, key: &str) -> Option<&str> {
        self.str(key).filter(|s| !s.is_empty())
    }

    fn int(&self, key: &str) -> Option<i64> {
        match self.get(key)? {
            Plist::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Plist::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn list(&self, key: &str) -> &[Plist] {
        match self.get(key) {
            Some(Plist::Array(items)) => items,
            _ => &[],
        }
    }
}

/// An XML property list as diskutil writes one: the prolog, `<plist>`, and
/// dict, array, string, integer, true and false, the other scalars kept as
/// text; XML's five entities and numeric references. None for anything
/// else: the probe then finds no card rather than a wrong one.
pub(crate) fn parse(text: &str) -> Option<Plist> {
    let mut p = Reader { s: text, at: 0 };
    let tag = p.tag()?;
    if tag.name != "plist" || tag.end {
        return None;
    }
    let value = if tag.empty { return None } else { p.value()? };
    let close = p.tag()?;
    (close.name == "plist" && close.end).then_some(value)
}

struct Tag<'a> {
    name: &'a str,
    /// `</name>`.
    end: bool,
    /// `<name/>`.
    empty: bool,
}

struct Reader<'a> {
    s: &'a str,
    at: usize,
}

impl<'a> Reader<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.at..]
    }

    /// Past whitespace, the XML declaration, a doctype and comments.
    fn skip(&mut self) {
        loop {
            let trimmed = self.rest().trim_start();
            self.at = self.s.len() - trimmed.len();
            let close = match () {
                _ if trimmed.starts_with("<!--") => "-->",
                _ if trimmed.starts_with("<?") => "?>",
                _ if trimmed.starts_with("<!") => ">",
                _ => return,
            };
            match trimmed.find(close) {
                Some(i) => self.at += i + close.len(),
                None => self.at = self.s.len(),
            }
        }
    }

    fn tag(&mut self) -> Option<Tag<'a>> {
        self.skip();
        let rest = self.rest().strip_prefix('<')?;
        let inner = &rest[..rest.find('>')?];
        self.at += inner.len() + 2;
        let (end, inner) = match inner.strip_prefix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let (empty, inner) = match inner.strip_suffix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let name = inner.split_whitespace().next()?;
        Some(Tag { name, end, empty })
    }

    /// The text up to the next tag, its references undone.
    fn text(&mut self) -> Option<String> {
        let rest = self.rest();
        let raw = &rest[..rest.find('<')?];
        self.at += raw.len();
        let mut out = String::with_capacity(raw.len());
        let mut parts = raw.split('&');
        out.push_str(parts.next()?);
        for part in parts {
            let (entity, after) = part.split_once(';')?;
            let c = match entity {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                _ => {
                    let n = match entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => entity.strip_prefix('#')?.parse().ok()?,
                    };
                    char::from_u32(n)?
                }
            };
            out.push(c);
            out.push_str(after);
        }
        Some(out)
    }

    /// `</name>` next.
    fn end(&mut self, name: &str) -> Option<()> {
        let tag = self.tag()?;
        (tag.end && tag.name == name).then_some(())
    }

    /// Whether `</name>` is next, consuming it if so; None at the end.
    fn closes(&mut self, name: &str) -> Option<bool> {
        self.skip();
        let rest = self.rest();
        if rest.is_empty() {
            return None;
        }
        let closing = rest.strip_prefix("</").and_then(|r| r.strip_prefix(name)).is_some_and(|r| {
            r.trim_start().starts_with('>')
        });
        if closing {
            self.tag()?;
        }
        Some(closing)
    }

    /// A scalar's text: `<x>text</x>`, or "" for `<x/>`.
    fn scalar(&mut self, tag: &Tag) -> Option<String> {
        if tag.empty {
            return Some(String::new());
        }
        let text = self.text()?;
        self.end(tag.name)?;
        Some(text)
    }

    fn value(&mut self) -> Option<Plist> {
        let tag = self.tag()?;
        if tag.end {
            return None;
        }
        Some(match tag.name {
            "dict" => {
                let mut entries = Vec::new();
                while !tag.empty && !self.closes("dict")? {
                    let key = self.tag()?;
                    if key.name != "key" || key.end {
                        return None;
                    }
                    let key = self.scalar(&key)?;
                    entries.push((key, self.value()?));
                }
                Plist::Dict(entries)
            }
            "array" => {
                let mut items = Vec::new();
                while !tag.empty && !self.closes("array")? {
                    items.push(self.value()?);
                }
                Plist::Array(items)
            }
            "true" if tag.empty => Plist::Bool(true),
            "false" if tag.empty => Plist::Bool(false),
            "string" => Plist::Str(self.scalar(&tag)?),
            "integer" => {
                let text = self.scalar(&tag)?;
                text.trim().parse().map_or(Plist::Other(text), Plist::Int)
            }
            "real" | "date" | "data" => Plist::Other(self.scalar(&tag)?),
            _ => return None,
        })
    }
}

/// The whole disk of a partition's identifier: `disk4s1` is on `disk4`.
fn whole_of(id: &str) -> &str {
    let Some(rest) = id.strip_prefix("disk") else { return id };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    &id[..4 + digits]
}

/// A partition's `Content` that is a FAT type (`DOS_FAT_32`,
/// `Windows_FAT_16`).
fn fat_content(content: &str) -> bool {
    content.contains("FAT")
}

fn bus_of(protocol: &str) -> Bus {
    match protocol {
        "USB" => Bus::Usb,
        "Secure Digital" => Bus::Sd,
        "SATA" | "ATA" => Bus::Ata,
        "PCI-Express" | "PCI" | "NVMe" => Bus::Nvme,
        "Disk Image" | "Virtual Interface" => Bus::Virtual,
        "" => Bus::Unknown,
        other => Bus::Other(other.to_string()),
    }
}

/// Every mounted volume of a physical disk, judged. None when diskutil
/// can't list the disks.
pub(crate) fn found(du: &dyn Diskutil) -> Option<Vec<Found>> {
    let list = parse(&du.list()?)?;
    let info = |what: &str| du.info(what).and_then(|t| parse(&t));

    // The system's disks, and why each is.
    let mut system: HashMap<String, Vec<String>> = HashMap::new();
    let mut mark = |id: &str, why: &str| {
        let reasons = system.entry(whole_of(id).to_string()).or_default();
        if !reasons.iter().any(|r| r == why) {
            reasons.push(why.to_string());
        }
    };
    if let Some(root) = info("/") {
        if let Some(disk) = root.text("ParentWholeDisk") {
            mark(disk, "the startup disk");
        }
        for store in root.list("APFSPhysicalStores") {
            let id = store.text("APFSPhysicalStore").or_else(|| store.text("DeviceIdentifier"));
            if let Some(id) = id {
                mark(id, "the startup disk");
            }
        }
    }
    let disks = list.list("AllDisksAndPartitions");
    for disk in disks {
        for store in disk.list("APFSPhysicalStores") {
            if let Some(id) = store.text("DeviceIdentifier") {
                mark(id, "an APFS container");
            }
        }
    }

    let mut out = Vec::new();
    for disk in disks {
        let Some(id) = disk.text("DeviceIdentifier") else { continue };
        // A synthesized APFS container: its volumes are APFS's, its disk
        // the physical store's.
        if !disk.list("APFSVolumes").is_empty() || !disk.list("APFSPhysicalStores").is_empty() {
            continue;
        }
        let partitions = disk.list("Partitions");
        let mounted = |p: &Plist| p.text("MountPoint").is_some();
        let mut volumes: Vec<(&str, &Plist)> = partitions
            .iter()
            .filter(|&p| mounted(p))
            .filter_map(|p| Some((p.text("DeviceIdentifier")?, p)))
            .collect();
        if mounted(disk) {
            volumes.push((id, disk));
        }
        if volumes.is_empty() {
            continue;
        }
        let whole = info(id);
        let flag = |key: &str| whole.as_ref().and_then(|w| w.flag(key)).unwrap_or(false);
        let protocol =
            whole.as_ref().and_then(|w| w.text("BusProtocol").or_else(|| w.text("Protocol")));
        for (device, entry) in volumes {
            let is_whole = device == id;
            let table = match disk.str("Content") {
                _ if is_whole => Table::None,
                Some("FDisk_partition_scheme") => Table::Mbr,
                Some("GUID_partition_scheme") => Table::Gpt,
                Some("Apple_partition_scheme") => Table::Other("apm".into()),
                _ => Table::Unknown,
            };
            let mut others = Vec::new();
            for p in partitions.iter().filter(|p| p.text("DeviceIdentifier") != Some(device)) {
                let name = p.text("DeviceIdentifier").unwrap_or("?");
                let content = p.str("Content").unwrap_or("");
                match p.text("MountPoint") {
                    Some(point) if content.is_empty() => others.push(format!("{name} at {point}")),
                    Some(point) => others.push(format!("{content} at {point}")),
                    None if fat_content(content) => {}
                    None if content.is_empty() => others.push(format!("{name} (unknown)")),
                    None => others.push(format!("{content} on {name}")),
                }
            }
            let vinfo = info(device);
            let v = vinfo.as_ref().unwrap_or(entry);
            let point = entry.text("MountPoint").unwrap_or_default().to_string();
            let space = du.space(&point).or_else(|| {
                let cluster = v.int("VolumeAllocationBlockSize").filter(|c| *c > 0)? as u64;
                let size = v.int("TotalSize").or_else(|| v.int("Size"))? as u64;
                let free = v.int("FreeSpace").unwrap_or(0).max(0) as u64;
                Some(Space { size, free, cluster: cluster as u32, clusters: size / cluster })
            });
            let names = ["FilesystemName", "FilesystemPersonality", "FilesystemUserVisibleName"];
            let width = names.iter().find_map(|k| {
                let name = v.str(k)?;
                ["FAT32", "FAT16", "FAT12"].into_iter().find(|w| name.contains(w))
            });
            let fs = match v.text("FilesystemType") {
                Some("msdos") => match width {
                    Some("FAT32") => Fs::Fat32,
                    Some("FAT16") => Fs::Fat16,
                    Some(_) => Fs::Fat12,
                    None => space.map_or(Fs::Fat32, |s| Fs::fat_of_clusters(s.clusters)),
                },
                Some("exfat") => Fs::ExFat,
                Some("ntfs") => Fs::Ntfs,
                Some(other) => Fs::Other(other.to_string()),
                // diskutil said nothing of the volume: its partition type.
                None => match entry.text("Content") {
                    Some(content) if fat_content(content) => {
                        space.map_or(Fs::Fat32, |s| Fs::fat_of_clusters(s.clusters))
                    }
                    content => Fs::Other(content.unwrap_or("unknown").to_string()),
                },
            };
            let writable = v.flag("WritableVolume").or_else(|| v.flag("Writable")).unwrap_or(true);
            let label = v.text("VolumeName").or_else(|| entry.text("VolumeName")).unwrap_or_default();
            let uuid = v.text("VolumeUUID").or_else(|| entry.text("VolumeUUID"));
            let volume = Volume {
                path: point.into(),
                device: device.to_string(),
                fs,
                table,
                label: label.to_string(),
                serial: uuid.map(|u| Serial::Uuid(u.into())),
                space,
                read_only: !writable,
                utf8: None,
                clock: Clock::Local,
            };
            let on = Disk {
                id: id.to_string(),
                removable: flag("RemovableMedia") || flag("Removable") || flag("Ejectable"),
                bus: bus_of(protocol.unwrap_or("")),
                read_only: whole.as_ref().and_then(|w| w.flag("WritableMedia")) == Some(false),
                system: system.get(id).cloned().unwrap_or_default(),
                others,
            };
            out.push(Found::new(volume, on));
        }
    }
    Some(out)
}

/// The local time zone's offset from UTC now, in seconds east: the FAT
/// clock of a Mac (2.3.4). 0 where there is no Mac to ask.
pub(crate) fn local_offset() -> i64 {
    #[cfg(target_os = "macos")]
    {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let now = now.map_or(0, |d| d.as_secs());
        let t = now as libc::time_t;
        // SAFETY: tm is plain old data; localtime_r fills it.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: a time and a tm that outlive the call, which is reentrant.
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return tm.tm_gmtoff as i64;
        }
    }
    0
}

/// This Mac's volumes, judged.
#[cfg(target_os = "macos")]
pub(crate) fn find() -> std::io::Result<Vec<Found>> {
    found(&sys::Tool).ok_or_else(|| std::io::Error::other("diskutil can't list the disks"))
}

#[cfg(target_os = "macos")]
mod sys {
    use std::ffi::CString;

    use super::{Diskutil, Space};

    /// /usr/sbin/diskutil, and statfs.
    pub(super) struct Tool;

    fn diskutil(args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("/usr/sbin/diskutil").args(args).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    impl Diskutil for Tool {
        fn list(&self) -> Option<String> {
            diskutil(&["list", "-plist"])
        }

        fn info(&self, what: &str) -> Option<String> {
            diskutil(&["info", "-plist", what])
        }

        fn space(&self, mount: &str) -> Option<Space> {
            let path = CString::new(mount).ok()?;
            // SAFETY: statfs is plain old data; the call fills it.
            let mut st: libc::statfs = unsafe { std::mem::zeroed() };
            // SAFETY: a NUL-terminated path that outlives the call, and the
            // struct above to write into.
            if unsafe { libc::statfs(path.as_ptr(), &mut st) } != 0 {
                return None;
            }
            let unit = st.f_bsize as u64;
            Some(Space {
                size: st.f_blocks as u64 * unit,
                free: st.f_bavail as u64 * unit,
                cluster: st.f_bsize as u32,
                clusters: st.f_blocks as u64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::Refusal;
    use super::*;

    #[derive(Default)]
    struct Fake {
        list: String,
        info: BTreeMap<String, String>,
        spaces: BTreeMap<String, Space>,
    }

    impl Diskutil for Fake {
        fn list(&self) -> Option<String> {
            Some(self.list.clone())
        }

        fn info(&self, what: &str) -> Option<String> {
            self.info.get(what).cloned()
        }

        fn space(&self, mount: &str) -> Option<Space> {
            self.spaces.get(mount).copied()
        }
    }

    fn plist(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n{body}\n</plist>\n"
        )
    }

    /// `<key>k</key>` and a value, for each pair.
    fn dict(pairs: &[(&str, String)]) -> String {
        let body: String = pairs.iter().map(|(k, v)| format!("<key>{k}</key>{v}")).collect();
        format!("<dict>{body}</dict>")
    }

    fn s(text: &str) -> String {
        format!("<string>{text}</string>")
    }

    fn n(value: i64) -> String {
        format!("<integer>{value}</integer>")
    }

    fn b(value: bool) -> String {
        if value { "<true/>".into() } else { "<false/>".into() }
    }

    fn array(items: &[String]) -> String {
        format!("<array>{}</array>", items.concat())
    }

    fn partition(id: &str, content: &str, mount: Option<&str>, name: &str) -> String {
        let mut pairs =
            vec![("Content", s(content)), ("DeviceIdentifier", s(id)), ("Size", n(63_850_000_000))];
        if let Some(mount) = mount {
            pairs.push(("MountPoint", s(mount)));
        }
        pairs.push(("VolumeName", s(name)));
        dict(&pairs)
    }

    /// A MacBook with its built-in SD slot: the system on disk0 (an EFI
    /// partition and an APFS store, synthesized as disk3), a card in the
    /// slot as disk4 (MBR, one FAT32 partition), which reports itself
    /// internal.
    fn macbook(slot: &[(&str, String)]) -> Fake {
        let disk0 = dict(&[
            ("Content", s("GUID_partition_scheme")),
            ("DeviceIdentifier", s("disk0")),
            ("OSInternal", b(false)),
            (
                "Partitions",
                array(&[
                    partition("disk0s1", "EFI", None, "EFI"),
                    partition("disk0s2", "Apple_APFS", None, ""),
                ]),
            ),
        ]);
        let disk3 = dict(&[
            ("APFSPhysicalStores", array(&[dict(&[("DeviceIdentifier", s("disk0s2"))])])),
            (
                "APFSVolumes",
                array(&[dict(&[
                    ("DeviceIdentifier", s("disk3s1")),
                    ("MountPoint", s("/System/Volumes/Data")),
                ])]),
            ),
            ("Content", s("EF57347C-0000-11AA-AA11-00306543ECAC")),
            ("DeviceIdentifier", s("disk3")),
            ("Partitions", array(&[])),
        ]);
        let disk4 = dict(&[
            ("Content", s("FDisk_partition_scheme")),
            ("DeviceIdentifier", s("disk4")),
            (
                "Partitions",
                array(&[partition("disk4s1", "Windows_FAT_32", Some("/Volumes/MUSIC &amp; MORE"), "MUSIC")]),
            ),
        ]);
        let mut f = Fake {
            list: plist(&dict(&[
                ("AllDisks", array(&["disk0", "disk0s1", "disk0s2", "disk3", "disk4", "disk4s1"].map(s))),
                ("AllDisksAndPartitions", array(&[disk0, disk3, disk4])),
            ])),
            ..Fake::default()
        };
        let root = dict(&[
            ("APFSPhysicalStores", array(&[dict(&[("APFSPhysicalStore", s("disk0s2"))])])),
            ("FilesystemType", s("apfs")),
            ("MountPoint", s("/")),
            ("ParentWholeDisk", s("disk3")),
        ]);
        f.info.insert("/".into(), plist(&root));
        f.info.insert("disk4".into(), plist(&dict(slot)));
        let volume = dict(&[
            ("DeviceIdentifier", s("disk4s1")),
            ("FilesystemName", s("MS-DOS FAT32")),
            ("FilesystemType", s("msdos")),
            ("MountPoint", s("/Volumes/MUSIC &amp; MORE")),
            ("ParentWholeDisk", s("disk4")),
            ("VolumeAllocationBlockSize", n(32_768)),
            ("VolumeName", s("MUSIC")),
            ("VolumeUUID", s("8D6B1E2A-0C1B-3F4E-9A7D-112233445566")),
            ("WritableVolume", b(true)),
        ]);
        f.info.insert("disk4s1".into(), plist(&volume));
        f.spaces.insert(
            "/Volumes/MUSIC & MORE".into(),
            Space { size: 63_850_000_000, free: 1_000_000_000, cluster: 32_768, clusters: 1_948_547 },
        );
        f
    }

    /// The built-in slot as such Macs report it: internal, removable media,
    /// the Secure Digital protocol.
    fn slot() -> Vec<(&'static str, String)> {
        vec![
            ("BusProtocol", s("Secure Digital")),
            ("Content", s("FDisk_partition_scheme")),
            ("DeviceIdentifier", s("disk4")),
            ("Ejectable", b(true)),
            ("Internal", b(true)),
            ("RemovableMedia", b(true)),
            ("WritableMedia", b(true)),
        ]
    }

    fn codes(f: &Found) -> Vec<&'static str> {
        f.refusals.iter().map(Refusal::code).collect()
    }

    #[test]
    fn a_macs_built_in_sd_slot_is_a_card_though_it_says_internal() {
        let found = found(&macbook(&slot())).unwrap();
        assert_eq!(found.len(), 1, "the APFS volumes are no cards: {found:#?}");
        let card = &found[0];
        assert!(card.accepted(), "{card:#?}");
        assert_eq!(card.volume.path, std::path::Path::new("/Volumes/MUSIC & MORE"));
        assert_eq!(card.volume.fs, Fs::Fat32);
        assert_eq!(card.volume.table, Table::Mbr);
        let uuid = "8D6B1E2A-0C1B-3F4E-9A7D-112233445566";
        assert_eq!(card.volume.serial, Some(Serial::Uuid(uuid.into())));
        assert_eq!(card.volume.label, "MUSIC");
        assert_eq!(card.volume.clock, Clock::Local);
        assert_eq!(card.disk.bus, Bus::Sd);

        // Not removable, not ejectable: the protocol alone lets it through.
        let mut quiet = slot();
        quiet.retain(|(k, _)| !matches!(*k, "Ejectable" | "RemovableMedia"));
        assert!(found_with(&quiet).accepted());
    }

    fn found_with(slot: &[(&str, String)]) -> Found {
        found(&macbook(slot)).unwrap().remove(0)
    }

    #[test]
    fn not_internal_alone_is_no_card_and_the_startup_disk_never_is() {
        // An external SATA disk that is neither removable nor ejectable.
        let external = vec![
            ("BusProtocol", s("SATA")),
            ("DeviceIdentifier", s("disk4")),
            ("Internal", b(false)),
            ("RemovableMedia", b(false)),
        ];
        assert_eq!(codes(&found_with(&external)), ["fixed-disk"]);

        // A FAT partition on the startup disk itself (disk0, the APFS
        // store's), mounted: refused as the system's.
        let mut f = macbook(&slot());
        f.list = f.list.replace(
            &partition("disk0s1", "EFI", None, "EFI"),
            &partition("disk0s1", "DOS_FAT_32", Some("/Volumes/OLD"), "OLD"),
        );
        f.info.insert(
            "disk0".into(),
            plist(&dict(&[("BusProtocol", s("USB")), ("Ejectable", b(true)), ("DeviceIdentifier", s("disk0"))])),
        );
        let found = found(&f).unwrap();
        let old = found.iter().find(|x| x.volume.device == "disk0s1").unwrap();
        assert_eq!(old.disk.system, ["the startup disk", "an APFS container"]);
        assert_eq!(old.disk.others, ["Apple_APFS on disk0s2"]);
        assert_eq!(codes(old), ["system-disk", "shared-disk", "gpt"]);
    }

    #[test]
    fn exfat_read_only_and_locked_cards_say_why() {
        let mut f = macbook(&slot());
        let v = f.info["disk4s1"].replace("<string>msdos</string>", "<string>exfat</string>");
        let v = v.replace("<key>WritableVolume</key><true/>", "<key>WritableVolume</key><false/>");
        f.info.insert("disk4s1".into(), v);
        assert_eq!(codes(&found(&f).unwrap()[0]), ["exfat", "read-only"]);

        let mut locked = slot();
        locked.retain(|(k, _)| *k != "WritableMedia");
        locked.push(("WritableMedia", b(false)));
        assert_eq!(codes(&found_with(&locked)), ["read-only"]);
    }

    #[test]
    fn plists_parse_as_diskutil_writes_them() {
        let text = plist(&dict(&[
            ("A", s("x &lt;&amp;&gt; &#233; &#xE9;")),
            ("B", n(-12)),
            ("C", array(&[b(true), b(false), "<string/>".into(), "<real>1.5</real>".into()])),
            ("D", "<dict/>".into()),
            ("E", "<array/>".into()),
        ]));
        let p = parse(&text).unwrap();
        assert_eq!(p.str("A"), Some("x <&> é é"));
        assert_eq!(p.int("B"), Some(-12));
        assert_eq!(
            p.list("C"),
            [Plist::Bool(true), Plist::Bool(false), Plist::Str(String::new()), Plist::Other("1.5".into())]
        );
        assert_eq!(p.get("D"), Some(&Plist::Dict(Vec::new())));
        assert!(p.list("E").is_empty());
        assert!(parse("<plist><dict><key>A</key></dict></plist>").is_none(), "a key with no value");
        assert!(parse("<plist><dict>").is_none());
        assert!(parse("<plist><wat/></plist>").is_none());
        assert_eq!(whole_of("disk4s1"), "disk4");
        assert_eq!(whole_of("disk12"), "disk12");
    }
}
