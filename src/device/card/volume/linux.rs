//! Linux: the volumes as /proc and /sys show them, with no libudev (the
//! release's linkage check refuses a binary that needs it: test/linkage.sh)
//! and nothing that needs root.
//!
//! - **The mounts:** /proc/self/mountinfo, each by its device number, or by
//!   its source for the filesystems that have none of a disk's (btrfs); and
//!   the swap partitions and files of /proc/swaps.
//! - **Their disks:** /sys/dev/block/M:m is the block device; a
//!   partition's folder sits in its disk's; a mapped or RAID device (dm, md,
//!   LVM, LUKS) names what it is built on in `slaves/`, followed down to the
//!   disks.
//! - **The system's disks:** every disk under `/`, `/boot`, `/boot/firmware`,
//!   `/boot/efi`, `/efi`, `/usr`, `/var` or `/home`, and every disk a swap is
//!   on. A Pi boots from its own SD card (mmcblk0, a FAT boot partition on
//!   MBR beside its root): that card is one of these, never a card to fill.
//! - **A card:** `removable`, or a disk under a USB or an MMC host — a
//!   laptop's built-in slot reports removable=0 — but not an MMC host's
//!   soldered eMMC (`device/type` MMC, not SD).
//! - **A disk of its own:** no other mounted filesystem on it, and no other
//!   partition that lsblk names as anything but FAT; without lsblk, no other
//!   partition at all.
//! - **The rest:** the partition table from lsblk when there is one, else
//!   unknown; the serial from /dev/disk/by-uuid; size, free space and the
//!   cluster from statvfs; the FAT clock from the mount's `time_offset=` or
//!   `tz=UTC`, else the kernel's sys_tz (docs/METADATA.md 2.3.4); and
//!   whether names list as stored: `utf8` or `iocharset=utf8` (2.8.5).
//!
//! [`found`] is the whole rule over a [`Host`], which the tests fake with
//! made-up mount tables and sysfs trees; [`find`] runs it on this machine.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{Bus, Clock, Disk, Found, Fs, Serial, Space, Table, Volume};

/// The machine as the probe reads it: files and links under /proc, /sys and
/// /dev, and two questions for the OS.
pub(crate) trait Host {
    /// A file's text, None when there is none.
    fn read(&self, path: &str) -> Option<String>;
    /// The path with every symlink in it followed.
    fn resolve(&self, path: &str) -> Option<String>;
    /// A symlink's own target, as written.
    fn link(&self, path: &str) -> Option<String>;
    /// A folder's names.
    fn list(&self, dir: &str) -> Vec<String>;
    /// The device number of the filesystem holding `path` (a swap file's).
    fn dev_of(&self, path: &str) -> Option<(u32, u32)>;
    /// lsblk's [`LSBLK`] listing; None without lsblk.
    fn lsblk(&self) -> Option<String>;
    fn space(&self, mount: &str) -> Option<Space>;
    /// The kernel's sys_tz: minutes west of UTC.
    fn minutes_west(&self) -> i64;
}

/// What lsblk is asked: one flat JSON list, sizes in bytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const LSBLK: &[&str] =
    &["-J", "-l", "-b", "-o", "KNAME,PKNAME,TYPE,FSTYPE,PTTYPE,PARTTYPE,UUID,LABEL"];

/// The mount points whose disks are the system's.
const SYSTEM_MOUNTS: &[&str] =
    &["/", "/boot", "/boot/firmware", "/boot/efi", "/efi", "/usr", "/var", "/home"];

/// MBR partition types that hold partitions, not a filesystem.
const EXTENDED: &[&str] = &["0x5", "0xf", "0x85"];

/// One line of /proc/self/mountinfo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mount {
    pub(crate) dev: (u32, u32),
    /// The folder of the filesystem mounted here: `/` for a whole one, a
    /// subfolder for a bind mount.
    pub(crate) root: String,
    pub(crate) point: String,
    /// The mount's own options, then the filesystem's (the last field).
    pub(crate) options: Vec<String>,
    pub(crate) fstype: String,
    pub(crate) source: String,
    pub(crate) super_options: Vec<String>,
}

impl Mount {
    fn has(&self, option: &str) -> bool {
        self.options.iter().chain(&self.super_options).any(|o| o == option)
    }
}

/// /proc/self/mountinfo's lines: `id parent M:m root point options
/// [optional fields] - fstype source super-options`, the paths with their
/// spaces and backslashes escaped in octal.
pub(crate) fn mounts(text: &str) -> Vec<Mount> {
    let split = |s: &str| s.split(',').map(str::to_string).collect::<Vec<_>>();
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let dash = 6 + fields.get(6..)?.iter().position(|f| *f == "-")?;
            let (major, minor) = fields[2].split_once(':')?;
            Some(Mount {
                dev: (major.parse().ok()?, minor.parse().ok()?),
                root: unescape(fields[3]),
                point: unescape(fields[4]),
                options: split(fields[5]),
                fstype: fields.get(dash + 1)?.to_string(),
                source: unescape(fields.get(dash + 2)?),
                super_options: fields.get(dash + 3).map_or_else(Vec::new, |s| split(s)),
            })
        })
        .collect()
}

/// /proc/swaps' entries after its header: (the device or file, whether it
/// is a partition).
pub(crate) fn swaps(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let (name, kind) = (fields.next()?, fields.next()?);
            Some((unescape(name), kind == "partition"))
        })
        .collect()
}

/// The kernel's octal escapes (`\040` a space) undone.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b.get(i + 1..i + 4).filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match (b[i], octal) {
            (b'\\', Some(d)) => {
                out.push(((d[0] - b'0') << 6) | ((d[1] - b'0') << 3) | (d[2] - b'0'));
                i += 4;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// udev's `\xNN` escapes in /dev/disk/by-label names undone.
fn unescape_hex(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i..i + 4).filter(|h| h[0] == b'\\' && h[1] == b'x');
        match hex.and_then(|h| u8::from_str_radix(std::str::from_utf8(&h[2..]).ok()?, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A dev_t as glibc encodes it, split into its major and minor.
pub(crate) fn split_dev(dev: u64) -> (u32, u32) {
    let major = ((dev >> 8) & 0xFFF) | ((dev >> 32) & 0xFFFF_F000);
    let minor = (dev & 0xFF) | ((dev >> 12) & 0xFFFF_FF00);
    (major as u32, minor as u32)
}

/// A block device, by its kernel name and its folder under /sys/devices.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Block {
    name: String,
    sys: String,
    partition: bool,
}

fn block(host: &dyn Host, link: &str) -> Option<Block> {
    let sys = host.resolve(link)?;
    let name = sys.rsplit('/').next().filter(|n| !n.is_empty())?.to_string();
    let partition = host.read(&format!("{sys}/partition")).is_some();
    Some(Block { name, sys, partition })
}

fn block_of_dev(host: &dyn Host, (major, minor): (u32, u32)) -> Option<Block> {
    block(host, &format!("/sys/dev/block/{major}:{minor}"))
}

fn block_of_name(host: &dyn Host, name: &str) -> Option<Block> {
    block(host, &format!("/sys/class/block/{name}"))
}

/// A device node's block device: `/dev/mapper/root` is a link to `dm-0`.
fn block_of_node(host: &dyn Host, node: &str) -> Option<Block> {
    let real = host.resolve(node)?;
    block_of_name(host, real.strip_prefix("/dev/")?)
}

/// The block device a mount is on: by its device number, or for a
/// filesystem whose number is its own (btrfs, major 0) by its source.
fn block_of_mount(host: &dyn Host, m: &Mount) -> Option<Block> {
    if let Some(b) = block_of_dev(host, m.dev) {
        return Some(b);
    }
    m.source.starts_with("/dev/").then(|| block_of_node(host, &m.source)).flatten()
}

/// The whole disk a partition is in; a disk is its own.
fn whole(b: &Block) -> Block {
    if !b.partition {
        return b.clone();
    }
    let sys = b.sys.rsplit_once('/').map_or(b.sys.as_str(), |(dir, _)| dir).to_string();
    let name = sys.rsplit('/').next().unwrap_or_default().to_string();
    Block { name, sys, partition: false }
}

/// The disks under a block device: its own, or what a mapped or RAID device
/// is built on, followed down `slaves/` (a bounded depth: a loop in sysfs
/// can't hang the probe).
fn disks_under(host: &dyn Host, b: &Block, depth: u32) -> Vec<String> {
    let w = whole(b);
    let slaves = host.list(&format!("{}/slaves", w.sys));
    if slaves.is_empty() || depth > 8 {
        return vec![w.name];
    }
    let slaves = slaves.iter().filter_map(|s| block_of_name(host, s));
    slaves.flat_map(|s| disks_under(host, &s, depth + 1)).collect()
}

/// The bus a disk hangs on, from its folder under /sys/devices: a USB root
/// hub (`usb1`) on the way up, an MMC host (the card's `device/type` says
/// SD or MMC), an NVMe or ATA controller, a virtual device.
fn bus_of(host: &dyn Host, disk: &Block) -> Bus {
    let parts: Vec<&str> = disk.sys.split('/').collect();
    let digits = |n: &str| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit());
    let numbered = |prefix: &str| parts.iter().any(|p| p.strip_prefix(prefix).is_some_and(digits));
    if disk.sys.starts_with("/sys/devices/virtual/") {
        Bus::Virtual
    } else if numbered("usb") {
        Bus::Usb
    } else if disk.name.starts_with("mmcblk") || parts.contains(&"mmc_host") {
        match host.read(&format!("{}/device/type", disk.sys)).as_deref().map(str::trim) {
            Some("SD") => Bus::Sd,
            Some("MMC") => Bus::Emmc,
            _ => Bus::Mmc,
        }
    } else if disk.name.starts_with("nvme") {
        Bus::Nvme
    } else if numbered("ata") {
        Bus::Ata
    } else if parts.iter().any(|p| p.starts_with("virtio")) {
        Bus::Virtual
    } else if numbered("host") {
        Bus::Scsi
    } else {
        Bus::Unknown
    }
}

/// One row of lsblk's listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) fstype: String,
    pub(crate) pttype: String,
    pub(crate) parttype: String,
    pub(crate) uuid: String,
    pub(crate) label: String,
}

/// lsblk's `-J -l` listing by kernel name; nulls read as "".
pub(crate) fn lsblk_rows(text: &str) -> HashMap<String, Row> {
    let Ok(value) = serde_json::from_str::<Value>(text) else { return HashMap::new() };
    let field = |row: &Value, key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    value
        .get("blockdevices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|row| {
            let fields = Row {
                fstype: field(row, "fstype"),
                pttype: field(row, "pttype"),
                parttype: field(row, "parttype").to_ascii_lowercase(),
                uuid: field(row, "uuid"),
                label: field(row, "label"),
            };
            (field(row, "kname"), fields)
        })
        .collect()
}

/// Whether a mount lists names as the card stores them: vfat with `utf8` or
/// `iocharset=utf8` (2.8.5). msdos shows short names alone, so never.
fn lists_utf8(m: &Mount) -> bool {
    m.fstype == "vfat"
        && m.super_options.iter().chain(&m.options).any(|o| {
            let o = o.to_ascii_lowercase();
            let utf8 = matches!(o.as_str(), "utf8" | "utf8=1" | "utf8=yes" | "utf8=true");
            utf8 || o == "iocharset=utf8" || o == "iocharset=utf-8"
        })
}

/// The FAT clock of a mount (2.3.4): `tz=UTC` none, `time_offset=` its
/// minutes, else the kernel's sys_tz — the kernel's own order.
fn clock_of(m: &Mount, minutes_west: i64) -> Clock {
    for o in &m.super_options {
        if o == "tz=UTC" {
            return Clock::Offset(0);
        }
        if let Some(minutes) = o.strip_prefix("time_offset=").and_then(|n| n.parse::<i64>().ok()) {
            return Clock::Offset(minutes * 60);
        }
    }
    Clock::Offset(-minutes_west * 60)
}

/// The name in a /dev/disk/by-* folder that links to a block device: its
/// UUID (a FAT volume's serial, `1A2B-3C4D`) or its label.
fn named(host: &dyn Host, dir: &str, block: &str) -> Option<String> {
    host.list(dir).into_iter().find(|n| {
        host.link(&format!("{dir}/{n}")).is_some_and(|t| t.rsplit('/').next() == Some(block))
    })
}

/// A FAT-family mount (vfat, msdos), judged further as FAT.
fn is_fat(fstype: &str) -> bool {
    matches!(fstype, "vfat" | "msdos" | "umsdos")
}

/// Every mount that is FAT or sits on a card-shaped disk, judged. None when
/// the mount table can't be read.
pub(crate) fn found(host: &dyn Host) -> Option<Vec<Found>> {
    let all = mounts(&host.read("/proc/self/mountinfo")?);
    let blocks: Vec<Option<Block>> = all.iter().map(|m| block_of_mount(host, m)).collect();
    let disks: Vec<Vec<String>> =
        blocks.iter().map(|b| b.as_ref().map_or_else(Vec::new, |b| disks_under(host, b, 0))).collect();

    // The system's disks, and why each is.
    let mut system: HashMap<String, Vec<String>> = HashMap::new();
    let mut mark = |disk: &str, why: &str| {
        let reasons = system.entry(disk.to_string()).or_default();
        if !reasons.iter().any(|r| r == why) {
            reasons.push(why.to_string());
        }
    };
    for (m, under) in all.iter().zip(&disks) {
        if SYSTEM_MOUNTS.contains(&m.point.as_str()) {
            under.iter().for_each(|d| mark(d, &m.point));
        }
    }
    for (name, partition) in swaps(&host.read("/proc/swaps").unwrap_or_default()) {
        let b = match partition {
            true => block_of_node(host, &name),
            false => host.dev_of(&name).and_then(|dev| block_of_dev(host, dev)),
        };
        for d in b.map_or_else(Vec::new, |b| disks_under(host, &b, 0)) {
            mark(&d, "swap");
        }
    }

    let rows = host.lsblk().map(|text| lsblk_rows(&text));
    let row = |name: &str| rows.as_ref().and_then(|r| r.get(name));
    let minutes_west = host.minutes_west();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut out = Vec::new();
    for (i, m) in all.iter().enumerate() {
        // A filesystem once, where it is mounted whole (a bind mount of a
        // folder shows the same device again).
        let Some(b) = &blocks[i] else { continue };
        if m.root != "/" || !seen.insert(m.dev) {
            continue;
        }
        let w = whole(b);
        let flag = |sys: &str, name: &str| {
            host.read(&format!("{sys}/{name}")).is_some_and(|v| v.trim() == "1")
        };
        let (removable, bus) = (flag(&w.sys, "removable"), bus_of(host, &w));
        if !is_fat(&m.fstype) && !removable && !bus.card() {
            continue;
        }

        // What else the disk holds: other mounts on it, and partitions that
        // lsblk names as no FAT (or can't name).
        let mut others = Vec::new();
        let mut mounted: HashSet<&str> = HashSet::from([b.name.as_str()]);
        for (j, o) in all.iter().enumerate() {
            let on_disk = disks[j].contains(&w.name);
            if j == i || o.dev == m.dev || !on_disk {
                continue;
            }
            if let Some(ob) = &blocks[j] {
                mounted.insert(ob.name.as_str());
            }
            let what = format!("{} at {}", o.fstype, o.point);
            if !others.contains(&what) {
                others.push(what);
            }
        }
        let mut partitions: Vec<String> = host
            .list(&w.sys)
            .into_iter()
            .filter(|p| host.read(&format!("{}/{p}/partition", w.sys)).is_some())
            .collect();
        partitions.sort();
        for p in partitions.iter().filter(|p| !mounted.contains(p.as_str())) {
            match row(p) {
                Some(r) if EXTENDED.contains(&r.parttype.as_str()) => {}
                Some(r) if r.fstype == "vfat" => {}
                Some(r) if !r.fstype.is_empty() => others.push(format!("{} on {p}", r.fstype)),
                _ => others.push(format!("{p} (unknown)")),
            }
        }

        let space = host.space(&m.point);
        let fs = match m.fstype.as_str() {
            t if is_fat(t) => space.map_or(Fs::Fat32, |s| Fs::fat_of_clusters(s.clusters)),
            "exfat" => Fs::ExFat,
            "ntfs" | "ntfs3" => Fs::Ntfs,
            "fuseblk" => match row(&b.name).map(|r| r.fstype.as_str()) {
                Some("exfat") => Fs::ExFat,
                Some("ntfs") => Fs::Ntfs,
                _ => Fs::Other(m.fstype.clone()),
            },
            other => Fs::Other(other.to_string()),
        };
        let table = match row(&w.name).map(|r| r.pttype.as_str()) {
            Some("dos") => Table::Mbr,
            Some("gpt") => Table::Gpt,
            Some(other) if !other.is_empty() => Table::Other(other.to_string()),
            _ if !b.partition => Table::None,
            _ => Table::Unknown,
        };
        let uuid = named(host, "/dev/disk/by-uuid", &b.name)
            .or_else(|| row(&b.name).map(|r| r.uuid.clone()).filter(|u| !u.is_empty()));
        let label = row(&b.name).map(|r| r.label.clone()).filter(|l| !l.is_empty());
        let by_label = named(host, "/dev/disk/by-label", &b.name);
        let volume = Volume {
            path: m.point.clone().into(),
            device: b.name.clone(),
            fs,
            table,
            label: label.or(by_label.map(|l| unescape_hex(&l))).unwrap_or_default(),
            serial: uuid.as_deref().map(Serial::parse),
            space,
            read_only: m.has("ro") || flag(&b.sys, "ro"),
            utf8: is_fat(&m.fstype).then(|| lists_utf8(m)),
            clock: clock_of(m, minutes_west),
        };
        let disk = Disk {
            id: w.name.clone(),
            removable,
            bus,
            read_only: flag(&w.sys, "ro"),
            system: system.get(&w.name).cloned().unwrap_or_default(),
            others,
        };
        out.push(Found::new(volume, disk));
    }
    Some(out)
}

/// The serial of the filesystem mounted whole at `point` (the last mount
/// there, the one on top), as /dev/disk/by-uuid names it: what a run reads
/// again to be sure the card is still the one it was bound to. A few small
/// reads, no lsblk. None when nothing is mounted there.
pub(crate) fn serial_at(host: &dyn Host, point: &str) -> Option<Serial> {
    let all = mounts(&host.read("/proc/self/mountinfo")?);
    let m = all.iter().rev().find(|m| m.point == point && m.root == "/")?;
    let b = block_of_mount(host, m)?;
    named(host, "/dev/disk/by-uuid", &b.name).map(|uuid| Serial::parse(&uuid))
}

/// This machine's volumes, judged.
#[cfg(target_os = "linux")]
pub(crate) fn find() -> std::io::Result<Vec<Found>> {
    found(&sys::Machine).ok_or_else(|| std::io::Error::other("/proc/self/mountinfo can't be read"))
}

/// [`serial_at`] on this machine.
#[cfg(target_os = "linux")]
pub(crate) fn serial(point: &str) -> Option<Serial> {
    serial_at(&sys::Machine, point)
}

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::CString;

    use super::{Host, LSBLK, Space, split_dev};

    /// The real /proc, /sys and /dev, statvfs, the raw clock syscall, lsblk.
    pub(super) struct Machine;

    impl Host for Machine {
        fn read(&self, path: &str) -> Option<String> {
            std::fs::read_to_string(path).ok()
        }

        fn resolve(&self, path: &str) -> Option<String> {
            std::fs::canonicalize(path).ok()?.to_str().map(str::to_string)
        }

        fn link(&self, path: &str) -> Option<String> {
            std::fs::read_link(path).ok()?.to_str().map(str::to_string)
        }

        fn list(&self, dir: &str) -> Vec<String> {
            let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
            entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect()
        }

        fn dev_of(&self, path: &str) -> Option<(u32, u32)> {
            use std::os::unix::fs::MetadataExt;
            Some(split_dev(std::fs::metadata(path).ok()?.dev()))
        }

        fn lsblk(&self) -> Option<String> {
            let out = std::process::Command::new("lsblk").args(LSBLK).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
        }

        fn space(&self, mount: &str) -> Option<Space> {
            let path = CString::new(mount).ok()?;
            // SAFETY: statvfs is plain old data; the call fills it.
            let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
            // SAFETY: a NUL-terminated path that outlives the call, and the
            // struct above to write into.
            if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
                return None;
            }
            // vfat reports its cluster as the block, and its clusters as the
            // blocks (fs/fat/inode.c, fat_statfs).
            let unit = if st.f_frsize != 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
            Some(Space {
                size: st.f_blocks as u64 * unit,
                free: st.f_bavail as u64 * unit,
                cluster: st.f_bsize as u32,
                clusters: st.f_blocks as u64,
            })
        }

        fn minutes_west(&self) -> i64 {
            #[repr(C)]
            struct Timezone {
                minutes_west: libc::c_int,
                dst_time: libc::c_int,
            }
            // SAFETY: timeval is plain old data; the call fills it.
            let mut tv: libc::timeval = unsafe { std::mem::zeroed() };
            let mut tz = Timezone { minutes_west: 0, dst_time: 0 };
            // The raw call: glibc's wrapper writes zeros into the zone since
            // 2.31, and the zone is what vfat converts with.
            let (time, zone) = (&mut tv as *mut libc::timeval, &mut tz as *mut Timezone);
            // SAFETY: the syscall writes one timeval and one timezone, into
            // the two locals above.
            let ok = unsafe { libc::syscall(libc::SYS_gettimeofday, time, zone) } == 0;
            if ok { tz.minutes_west as i64 } else { 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::super::Refusal;
    use super::*;

    /// A made-up machine: files by path, symlinks by path (relative targets
    /// resolved as the kernel would), folders implied by both.
    #[derive(Default)]
    struct Fake {
        files: BTreeMap<String, String>,
        links: BTreeMap<String, String>,
        devs: BTreeMap<String, (u32, u32)>,
        lsblk: Option<String>,
        spaces: BTreeMap<String, Space>,
        minutes_west: i64,
    }

    impl Fake {
        fn file(&mut self, path: &str, text: &str) -> &mut Self {
            self.files.insert(path.into(), text.into());
            self
        }

        fn symlink(&mut self, path: &str, target: &str) -> &mut Self {
            self.links.insert(path.into(), target.into());
            self
        }

        /// A block device at its sysfs folder, reachable by number and by
        /// name, with its attributes.
        fn block(&mut self, sys: &str, dev: (u32, u32), attrs: &[(&str, &str)]) -> &mut Self {
            let name = sys.rsplit('/').next().unwrap().to_string();
            let rel = format!("../../{}", sys.trim_start_matches("/sys/"));
            self.symlink(&format!("/sys/dev/block/{}:{}", dev.0, dev.1), &rel);
            self.symlink(&format!("/sys/class/block/{name}"), &rel);
            self.file(&format!("{sys}/dev"), &format!("{}:{}\n", dev.0, dev.1));
            for (k, v) in attrs {
                self.file(&format!("{sys}/{k}"), v);
            }
            self.symlink(&format!("/dev/{name}"), &format!("/dev/{name}"))
        }

        /// The path with the links in it followed, `..` folded.
        fn walk(&self, path: &str) -> String {
            let mut parts: Vec<String> = Vec::new();
            let parts_of = |p: &str| {
                p.split('/').filter(|p| !p.is_empty()).rev().map(String::from).collect::<Vec<_>>()
            };
            let mut todo = parts_of(path);
            let mut hops = 0;
            while let Some(part) = todo.pop() {
                match part.as_str() {
                    "." => continue,
                    ".." => {
                        parts.pop();
                        continue;
                    }
                    _ => parts.push(part),
                }
                let here = format!("/{}", parts.join("/"));
                if let Some(target) = self.links.get(&here).filter(|t| **t != here) {
                    hops += 1;
                    assert!(hops < 64, "a link loop at {here}");
                    parts.pop();
                    if target.starts_with('/') {
                        parts.clear();
                    }
                    todo.extend(parts_of(target));
                }
            }
            format!("/{}", parts.join("/"))
        }
    }

    impl Host for Fake {
        fn read(&self, path: &str) -> Option<String> {
            self.files.get(&self.walk(path)).cloned()
        }

        fn resolve(&self, path: &str) -> Option<String> {
            let real = self.walk(path);
            let under = format!("{real}/");
            let mut keys = self.files.keys().chain(self.links.keys());
            let exists = keys.any(|k| *k == real || k.starts_with(&under));
            exists.then_some(real)
        }

        fn link(&self, path: &str) -> Option<String> {
            self.links.get(path).cloned()
        }

        fn list(&self, dir: &str) -> Vec<String> {
            let dir = self.walk(dir);
            let prefix = format!("{dir}/");
            let names: BTreeSet<String> = self
                .files
                .keys()
                .chain(self.links.keys())
                .filter_map(|k| k.strip_prefix(&prefix)?.split('/').next().map(String::from))
                .collect();
            names.into_iter().collect()
        }

        fn dev_of(&self, path: &str) -> Option<(u32, u32)> {
            self.devs.get(path).copied()
        }

        fn lsblk(&self) -> Option<String> {
            self.lsblk.clone()
        }

        fn space(&self, mount: &str) -> Option<Space> {
            self.spaces.get(mount).copied()
        }

        fn minutes_west(&self) -> i64 {
            self.minutes_west
        }
    }

    /// A 64 GB card at 32 KB clusters: FAT32 by its count.
    const CARD: Space =
        Space { size: 63_850_000_000, free: 60_000_000_000, cluster: 32_768, clusters: 1_948_547 };
    /// A Pi's 512 MB boot partition at 4 KB clusters: FAT32 too.
    const BOOT: Space =
        Space { size: 532_676_608, free: 450_000_000, cluster: 4096, clusters: 130_048 };

    const PI_MMC: &str =
        "/sys/devices/platform/emmc2bus/fe340000.mmc/mmc_host/mmc0/mmc0:aaaa/block/mmcblk0";
    const PI_USB: &str = "/sys/devices/platform/scb/fd500000.pcie/pci0000:00/0000:00:00.0/0000:01:00.0/usb2/2-1/\
                          2-1:1.0/host0/target0:0:0/0:0:0:0/block/sda";

    /// A Pi booted from its own SD card (mmcblk0: FAT32 boot beside an ext4
    /// root, a swap file on the root) with a card in a USB reader (sda).
    fn pi() -> Fake {
        let mut f = Fake::default();
        f.file(
            "/proc/self/mountinfo",
            "22 1 179:2 / / rw,noatime shared:1 - ext4 /dev/root rw\n\
             23 22 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw\n\
             29 22 179:1 / /boot/firmware rw,relatime shared:19 - vfat /dev/mmcblk0p1 rw,fmask=0022,dmask=0022,\
             codepage=437,iocharset=ascii,shortname=mixed,errors=remount-ro\n\
             41 22 8:1 / /media/user/CARD\\040ONE rw,nosuid,nodev,relatime shared:31 - vfat /dev/sda1 rw,uid=1000,\
             gid=1000,fmask=0022,dmask=0022,codepage=437,iocharset=ascii,shortname=mixed,showexec,utf8,flush,\
             errors=remount-ro\n",
        );
        f.file("/proc/swaps", "Filename\tType\tSize\tUsed\tPriority\n/var/swap\tfile\t102396\t0\t-2\n");
        f.devs.insert("/var/swap".into(), (179, 2));
        f.block(PI_MMC, (179, 0), &[("removable", "0\n"), ("ro", "0\n"), ("device/type", "SD\n")]);
        f.block(&format!("{PI_MMC}/mmcblk0p1"), (179, 1), &[("partition", "1\n"), ("ro", "0\n")]);
        f.block(&format!("{PI_MMC}/mmcblk0p2"), (179, 2), &[("partition", "2\n"), ("ro", "0\n")]);
        f.block(PI_USB, (8, 0), &[("removable", "1\n"), ("ro", "0\n")]);
        f.block(&format!("{PI_USB}/sda1"), (8, 1), &[("partition", "1\n"), ("ro", "0\n")]);
        f.symlink("/dev/disk/by-uuid/1A2B-3C4D", "../../sda1");
        f.symlink("/dev/disk/by-uuid/5E6F-7081", "../../mmcblk0p1");
        f.symlink("/dev/disk/by-uuid/0f1e2d3c-aaaa-bbbb-cccc-000000000001", "../../mmcblk0p2");
        f.symlink("/dev/disk/by-label/CARD\\x20ONE", "../../sda1");
        // lsblk's rows as it prints them, nulls and all, with the columns
        // the probe doesn't read left out.
        f.lsblk = Some(
            r#"{"blockdevices": [
                {"kname":"mmcblk0","pkname":null,"type":"disk","fstype":null,"pttype":"dos"},
                {"kname":"mmcblk0p1","type":"part","fstype":"vfat","parttype":"0xc","label":"bootfs"},
                {"kname":"mmcblk0p2","type":"part","fstype":"ext4","parttype":"0x83","label":"rootfs"},
                {"kname":"sda","pkname":null,"type":"disk","fstype":null,"pttype":"dos","uuid":null},
                {"kname":"sda1","pkname":"sda","type":"part","fstype":"vfat","uuid":"1A2B-3C4D","label":null}
            ]}"#
            .into(),
        );
        f.spaces.insert("/boot/firmware".into(), BOOT);
        f.spaces.insert("/media/user/CARD ONE".into(), CARD);
        f
    }

    fn at<'a>(found: &'a [Found], point: &str) -> &'a Found {
        let at = |f: &&Found| f.volume.path == std::path::Path::new(point);
        found.iter().find(at).unwrap_or_else(|| panic!("no {point}"))
    }

    fn codes(f: &Found) -> Vec<&'static str> {
        f.refusals.iter().map(Refusal::code).collect()
    }

    #[test]
    fn a_pi_refuses_its_own_boot_card_and_takes_the_one_in_the_reader() {
        let found = found(&pi()).unwrap();
        assert_eq!(found.len(), 3, "the two FAT mounts, and / on a card-shaped disk: {found:#?}");
        assert_eq!(codes(at(&found, "/")), ["system-disk", "shared-disk", "filesystem"]);

        let boot = at(&found, "/boot/firmware");
        assert_eq!(boot.disk.id, "mmcblk0");
        assert_eq!(boot.disk.bus, Bus::Sd);
        assert_eq!(boot.disk.system, ["/", "/boot/firmware", "swap"]);
        assert_eq!(boot.disk.others, ["ext4 at /"]);
        assert_eq!(codes(boot), ["system-disk", "shared-disk", "not-utf8"]);

        let card = at(&found, "/media/user/CARD ONE");
        assert!(card.accepted(), "{card:#?}");
        assert_eq!(card.disk.id, "sda");
        assert_eq!(card.disk.bus, Bus::Usb);
        assert!(card.disk.removable);
        assert_eq!(card.volume.fs, Fs::Fat32);
        assert_eq!(card.volume.table, Table::Mbr);
        assert_eq!(card.volume.serial, Some(Serial::Fat(0x1A2B_3C4D)));
        assert_eq!(card.volume.label, "CARD ONE");
        assert_eq!(card.volume.utf8, Some(true));
        assert_eq!(card.volume.space, Some(CARD));
        assert_eq!(card.volume.clock, Clock::Offset(0));

        // What a run reads again before it writes: the same serial, by the
        // mount point alone; nothing at a folder that isn't one.
        let pi = pi();
        assert_eq!(serial_at(&pi, "/media/user/CARD ONE"), Some(Serial::Fat(0x1A2B_3C4D)));
        assert_eq!(serial_at(&pi, "/boot/firmware"), Some(Serial::Fat(0x5E6F_7081)));
        assert_eq!(serial_at(&pi, "/media/user"), None);
    }

    #[test]
    fn a_pi_card_in_a_reader_is_refused_while_its_root_partition_is_there() {
        // The same Pi with another Pi's card in the reader: sda1 FAT, sda2
        // its ext4 root — mounted by the desktop, or not mounted at all.
        let mut f = pi();
        f.block(&format!("{PI_USB}/sda2"), (8, 2), &[("partition", "2\n"), ("ro", "0\n")]);
        let rows = f.lsblk.take().unwrap().replace(
            r#"{"kname":"sda1""#,
            r#"{"kname":"sda2","type":"part","fstype":"ext4","parttype":"0x83","label":"rootfs"},
               {"kname":"sda1""#,
        );
        f.lsblk = Some(rows);
        let unmounted = found(&f).unwrap();
        let card = at(&unmounted, "/media/user/CARD ONE");
        assert_eq!(codes(card), ["shared-disk"]);
        assert_eq!(card.disk.others, ["ext4 on sda2"]);

        let mut info = f.files["/proc/self/mountinfo"].clone();
        info.push_str("42 22 8:2 / /media/user/rootfs rw,nosuid,relatime shared:32 - ext4 /dev/sda2 rw\n");
        f.file("/proc/self/mountinfo", &info);
        let mounted = found(&f).unwrap();
        assert_eq!(at(&mounted, "/media/user/CARD ONE").disk.others, ["ext4 at /media/user/rootfs"]);
        // The rootfs itself is listed (the disk is a card's shape), refused.
        let rootfs = at(&mounted, "/media/user/rootfs");
        assert_eq!(codes(rootfs), ["shared-disk", "filesystem"]);

        // With no lsblk, a partition no one can name refuses the card too.
        f.lsblk = None;
        let unmounted_again = info.replace("42 22 8:2 / /media/user/rootfs", "garbage");
        f.file("/proc/self/mountinfo", &unmounted_again);
        let blind = found(&f).unwrap();
        let card = at(&blind, "/media/user/CARD ONE");
        assert_eq!(card.disk.others, ["sda2 (unknown)"]);
        assert_eq!(card.volume.table, Table::Unknown, "no lsblk, no table");
    }

    const LAPTOP_NVME: &str = "/sys/devices/pci0000:00/0000:00:1d.0/0000:3d:00.0/nvme/nvme0/nvme0n1";
    const LAPTOP_SLOT: &str = "/sys/devices/pci0000:00/0000:00:1c.0/0000:3b:00.0/rtsx_pci_sdmmc.0/mmc_host/mmc0/\
                               mmc0:59b4/block/mmcblk0";

    /// A laptop: the system on nvme0n1 (EFI, an ext4 root, a swap
    /// partition), a card in its built-in slot (mmcblk0, removable=0).
    fn laptop() -> Fake {
        let mut f = Fake::default();
        f.file(
            "/proc/self/mountinfo",
            "26 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro\n\
             27 26 259:2 /home /home rw,relatime shared:2 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro\n\
             30 26 259:1 / /boot/efi rw,relatime shared:3 - vfat /dev/nvme0n1p1 rw,fmask=0077,dmask=0077,\
             codepage=437,iocharset=iso8859-1,shortname=mixed,errors=remount-ro\n\
             40 26 7:0 / /snap/core22/1 ro,nodev,relatime shared:4 - squashfs /dev/loop0 ro\n\
             51 26 179:1 / /run/media/user/MUSIC rw,nosuid,nodev,relatime shared:40 - vfat /dev/mmcblk0p1 rw,\
             uid=1000,gid=1000,fmask=0022,dmask=0022,codepage=437,iocharset=utf8,shortname=mixed,time_offset=120,\
             errors=remount-ro\n",
        );
        let swaps = "Filename Type Size Used Priority\n/dev/nvme0n1p3 partition 8388604 0 -2\n";
        f.file("/proc/swaps", swaps);
        f.block(LAPTOP_NVME, (259, 0), &[("removable", "0\n"), ("ro", "0\n")]);
        for (n, minor) in [(1, 1), (2, 2), (3, 3)] {
            let partition = format!("{n}\n");
            f.block(&format!("{LAPTOP_NVME}/nvme0n1p{n}"), (259, minor), &[("partition", &partition)]);
        }
        f.block("/sys/devices/virtual/block/loop0", (7, 0), &[("removable", "0\n"), ("ro", "1\n")]);
        f.block(LAPTOP_SLOT, (179, 0), &[("removable", "0\n"), ("ro", "0\n"), ("device/type", "SD\n")]);
        f.block(&format!("{LAPTOP_SLOT}/mmcblk0p1"), (179, 1), &[("partition", "1\n"), ("ro", "0\n")]);
        f.symlink("/dev/disk/by-uuid/C0DE-F00D", "../../mmcblk0p1");
        f.spaces.insert("/run/media/user/MUSIC".into(), CARD);
        f.minutes_west = -60;
        f
    }

    #[test]
    fn a_laptops_built_in_slot_passes_by_its_bus_and_its_system_disk_never_does() {
        let found = found(&laptop()).unwrap();
        assert_eq!(found.len(), 2, "/boot/efi and the card; the rest are no cards: {found:#?}");

        let efi = at(&found, "/boot/efi");
        assert_eq!(efi.disk.id, "nvme0n1");
        assert_eq!(efi.disk.bus, Bus::Nvme);
        assert_eq!(efi.disk.system, ["/", "/home", "/boot/efi", "swap"]);
        assert_eq!(codes(efi), ["system-disk", "fixed-disk", "shared-disk", "not-utf8"]);

        let card = at(&found, "/run/media/user/MUSIC");
        assert!(card.accepted(), "{card:#?}");
        assert!(!card.disk.removable, "the slot says removable=0; its bus lets it through");
        assert_eq!(card.disk.bus, Bus::Sd);
        assert_eq!(card.volume.utf8, Some(true), "iocharset=utf8");
        assert_eq!(card.volume.table, Table::Unknown, "no lsblk on this one");
        assert_eq!(card.volume.serial, Some(Serial::Fat(0xC0DE_F00D)));
        assert_eq!(card.volume.clock, Clock::Offset(7200), "time_offset=120 minutes");
    }

    #[test]
    fn a_card_mounted_without_utf8_is_refused_and_the_kernel_zone_is_its_clock() {
        let mut f = laptop();
        let options = "iocharset=utf8,shortname=mixed,time_offset=120,";
        let info = f.files["/proc/self/mountinfo"].replace(options, "");
        f.file("/proc/self/mountinfo", &info);
        let found = found(&f).unwrap();
        let card = at(&found, "/run/media/user/MUSIC");
        assert_eq!(codes(card), ["not-utf8"]);
        assert_eq!(card.volume.clock, Clock::Offset(3600), "sys_tz 60 minutes east");

        let msdos = info.replace("- vfat /dev/mmcblk0p1", "- msdos /dev/mmcblk0p1");
        f.file("/proc/self/mountinfo", &msdos.replace("codepage=437,", "utf8,"));
        let found = found_of(&f);
        assert_eq!(codes(at(&found, "/run/media/user/MUSIC")), ["not-utf8"], "msdos lists short names");
    }

    fn found_of(f: &Fake) -> Vec<Found> {
        found(f).unwrap()
    }

    #[test]
    fn soldered_emmc_is_not_a_card_and_a_system_on_lvm_marks_its_disk() {
        // A board booted from its eMMC through LVM: / on dm-0, built on
        // mmcblk1p2; a FAT data partition on the same eMMC is mounted.
        let emmc = "/sys/devices/platform/soc/fe330000.mmc/mmc_host/mmc1/mmc1:0001/block/mmcblk1";
        let mut f = Fake::default();
        f.file(
            "/proc/self/mountinfo",
            "25 1 253:0 / / rw,relatime shared:1 - ext4 /dev/mapper/vg-root rw\n\
             31 25 179:3 / /data rw,relatime shared:2 - vfat /dev/mmcblk1p3 rw,utf8\n",
        );
        f.block("/sys/devices/virtual/block/dm-0", (253, 0), &[("removable", "0\n")]);
        f.symlink("/sys/devices/virtual/block/dm-0/slaves/mmcblk1p2", &format!("{emmc}/mmcblk1p2"));
        f.symlink("/dev/mapper/vg-root", "../dm-0");
        f.block(emmc, (179, 0), &[("removable", "0\n"), ("ro", "0\n"), ("device/type", "MMC\n")]);
        f.block(&format!("{emmc}/mmcblk1p2"), (179, 2), &[("partition", "2\n")]);
        f.block(&format!("{emmc}/mmcblk1p3"), (179, 3), &[("partition", "3\n")]);
        let found = found(&f).unwrap();
        let data = at(&found, "/data");
        assert_eq!(data.disk.bus, Bus::Emmc);
        assert_eq!(data.disk.system, ["/"], "through dm-0's slave");
        assert_eq!(codes(data), ["system-disk", "fixed-disk", "shared-disk"]);
    }

    #[test]
    fn btrfs_roots_are_traced_by_their_source_and_a_whole_disk_stick_has_no_table() {
        // btrfs mounts carry an anonymous device number (0:N); the source
        // names the partition. Its subvolumes are mounted by a folder (/@),
        // so none is a whole mount to judge, but each marks its disk.
        let ssd = "/sys/devices/pci0000:00/0000:00:17.0/ata2/host1/target1:0:0/1:0:0:0/block/sdb";
        let stick = "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0/host4/target4:0:0/4:0:0:0/block/sdc";
        let mut f = Fake::default();
        f.file(
            "/proc/self/mountinfo",
            "28 1 0:27 /@ / rw,relatime shared:1 - btrfs /dev/sdb2 rw,ssd,subvol=/@\n\
             29 28 0:27 /@home /home rw,relatime shared:2 - btrfs /dev/sdb2 rw,ssd,subvol=/@home\n\
             30 28 8:17 / /boot/efi rw,relatime shared:3 - vfat /dev/sdb1 rw,utf8\n\
             31 28 8:32 / /mnt/stick rw,relatime shared:4 - vfat /dev/sdc rw,utf8\n",
        );
        f.block(ssd, (8, 16), &[("removable", "0\n")]);
        f.block(&format!("{ssd}/sdb1"), (8, 17), &[("partition", "1\n")]);
        f.block(&format!("{ssd}/sdb2"), (8, 18), &[("partition", "2\n")]);
        // A USB stick with no partition table: the FAT volume is the disk.
        f.block(stick, (8, 32), &[("removable", "1\n")]);
        f.lsblk = Some(
            r#"{"blockdevices":[{"kname":"sdb","pttype":"gpt"},{"kname":"sdb1","fstype":"vfat"},
               {"kname":"sdb2","fstype":"btrfs"},{"kname":"sdc","pttype":null,"fstype":"vfat"}]}"#
                .into(),
        );
        let found = found(&f).unwrap();
        assert_eq!(found.len(), 2, "{found:#?}");
        let efi = at(&found, "/boot/efi");
        assert_eq!(efi.disk.bus, Bus::Ata);
        assert_eq!(efi.disk.system, ["/", "/home", "/boot/efi"]);
        assert_eq!(efi.disk.others, ["btrfs at /", "btrfs at /home"]);
        assert_eq!(efi.volume.table, Table::Gpt);
        assert_eq!(codes(efi), ["system-disk", "fixed-disk", "shared-disk", "gpt"]);
        let stick = at(&found, "/mnt/stick");
        assert!(stick.accepted(), "{stick:#?}");
        assert_eq!(stick.volume.table, Table::None);
    }

    #[test]
    fn read_only_cards_and_exfat_cards_are_refused_with_their_reasons() {
        let mut f = pi();
        f.file(&format!("{PI_USB}/ro"), "1\n");
        let found = found(&f).unwrap();
        assert_eq!(codes(at(&found, "/media/user/CARD ONE")), ["read-only"]);

        let mut f = pi();
        let info = f.files["/proc/self/mountinfo"].replace("- vfat /dev/sda1", "- exfat /dev/sda1");
        f.file("/proc/self/mountinfo", &info);
        assert_eq!(codes(at(&found_of(&f), "/media/user/CARD ONE")), ["exfat"]);
    }

    #[test]
    fn mountinfo_and_swaps_parse_with_their_escapes() {
        let line = "36 35 98:0 /mnt1 /mnt\\0402 rw,noatime master:1 shared:2 - ext3 /dev/root rw,errors=continue\n";
        let m = &mounts(line)[0];
        assert_eq!(m.dev, (98, 0));
        assert_eq!((m.root.as_str(), m.point.as_str()), ("/mnt1", "/mnt 2"));
        assert_eq!((m.fstype.as_str(), m.source.as_str()), ("ext3", "/dev/root"));
        assert_eq!(m.super_options, ["rw", "errors=continue"]);
        assert!(mounts("garbage\n1 2 3\n").is_empty());
        let swap = swaps("Filename Type\n/swap\\040file file 1 0 -2\n");
        assert_eq!(swap, [("/swap file".to_string(), false)]);
        assert_eq!(unescape_hex("CARD\\x20ONE"), "CARD ONE");
        assert_eq!(split_dev(0x0000_0000_0000_0801), (8, 1));
        assert_eq!(split_dev((259u64 << 8) | 3), (259, 3));
        // glibc's encoding, for numbers past the old 8-bit fields.
        let encode = |major: u64, minor: u64| {
            (major & 0xFFF) << 8 | (major & !0xFFF) << 32 | (minor & 0xFF) | (minor & !0xFF) << 12
        };
        assert_eq!(split_dev(encode(0x1234, 0x5_6789)), (0x1234, 0x5_6789));
    }
}
