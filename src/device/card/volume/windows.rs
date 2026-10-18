//! Windows: the volumes as the volume manager and the storage stack report
//! them, asked with no rights at all (a handle opened for no access answers
//! the queries every user may make).
//!
//! - **Every volume**, letter or not (FindFirstVolumeW): its filesystem,
//!   serial and label (GetVolumeInformationW), its disks
//!   (IOCTL_STORAGE_GET_DEVICE_NUMBER, or IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS
//!   for one that spans several), the disk's removable flag and bus
//!   (IOCTL_STORAGE_QUERY_PROPERTY) and its partition style
//!   (IOCTL_DISK_GET_PARTITION_INFO_EX).
//! - **The system's disks:** the disks of the Windows volume (the boot
//!   volume), of the system partition (the boot files; HKLM\SYSTEM\Setup's
//!   SystemPartition), of every volume a page file is on (the Memory
//!   Management key's ExistingPageFiles, and a `pagefile.sys` or
//!   `swapfile.sys` at a volume's root), and of any EFI system partition.
//! - **A card:** RemovableMedia, or a disk on the USB, SD or MMC bus.
//! - **A disk of its own:** no other volume on the disk with a letter, FAT
//!   or not (as Linux and macOS count any other mounted filesystem), and
//!   none without one that isn't FAT, including one Windows can't read (a
//!   Linux partition beside a card's FAT one reads as RAW).
//! - **The candidates** are the drive letters (GetLogicalDrives) of
//!   removable and fixed drives with their medium in; their size, free space
//!   and cluster from GetDiskFreeSpaceExW and GetDiskFreeSpaceW.
//!
//! [`judge_all`] is the rule over what Windows reported ([`WinVolume`],
//! [`WinSystem`]), which the tests make up; [`find`] asks this machine.
//! Reading a card's stamps back (docs/METADATA.md 2.3.4) is
//! [`local_wall`]'s: FileTimeToLocalFileTime, the bias in force now.

use std::collections::HashMap;

use super::{Bus, Clock, Disk, Found, Fs, Serial, Space, Table, Volume};

/// Why Windows read no filesystem on a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unread {
    /// The drive has no medium in (an empty reader).
    NoMedia,
    /// No filesystem Windows knows: RAW.
    Raw,
    /// Any other failure (a locked BitLocker volume, an EFI partition a
    /// user may not open), by its Win32 error.
    Error(u32),
}

/// One volume, as Windows reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WinVolume {
    /// `\\?\Volume{…}\`.
    pub(crate) guid: String,
    /// Its NT device, `\Device\HarddiskVolume3`.
    pub(crate) nt: String,
    /// Its drive letter roots, `E:\`.
    pub(crate) letters: Vec<String>,
    /// GetDriveTypeW's answer for it.
    pub(crate) drive_type: u32,
    /// The filesystem's name (`FAT32`, `FAT`, `exFAT`, `NTFS`), or why there
    /// is none.
    pub(crate) fs: Result<String, Unread>,
    pub(crate) label: String,
    pub(crate) serial: u32,
    /// GetVolumeInformationW's flags.
    pub(crate) flags: u32,
    /// The disks it is on: one, or several for a spanned volume; none when
    /// Windows wouldn't say.
    pub(crate) disks: Vec<u32>,
    pub(crate) removable: bool,
    /// STORAGE_BUS_TYPE; None unknown.
    pub(crate) bus: Option<i32>,
    /// PARTITION_STYLE (0 MBR, 1 GPT, 2 RAW: no table); None unknown.
    pub(crate) style: Option<i32>,
    /// An MBR partition's type byte.
    pub(crate) mbr_type: u8,
    /// A GPT partition's type GUID, lowercase.
    pub(crate) gpt_type: String,
    pub(crate) space: Option<Space>,
    /// A page file sits at its root (`pagefile.sys`, `swapfile.sys`).
    pub(crate) pagefile: bool,
}

impl Default for WinVolume {
    fn default() -> WinVolume {
        WinVolume {
            guid: String::new(),
            nt: String::new(),
            letters: Vec::new(),
            drive_type: 0,
            fs: Err(Unread::Error(0)),
            label: String::new(),
            serial: 0,
            flags: 0,
            disks: Vec::new(),
            removable: false,
            bus: None,
            style: None,
            mbr_type: 0,
            gpt_type: String::new(),
            space: None,
            pagefile: false,
        }
    }
}

/// Where Windows itself lives, as the registry names it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct WinSystem {
    /// The Windows volume's `\\?\Volume{…}\` (the boot volume).
    pub(crate) boot: Option<String>,
    /// The system partition's NT device (`\Device\HarddiskVolume1`).
    pub(crate) system: Option<String>,
    /// The `\\?\Volume{…}\` of every volume a page file is on.
    pub(crate) pages: Vec<String>,
}

/// GetDriveTypeW's removable and fixed drives: a card reader's slot is the
/// one, a USB disk often the other.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

/// GetVolumeInformationW's flag for a write-protected volume.
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

/// The EFI system partition's GPT type, and its MBR type byte.
const ESP_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const ESP_MBR: u8 = 0xEF;

/// STORAGE_BUS_TYPE, as ntddstor.h numbers it.
fn bus_of(bus: Option<i32>) -> Bus {
    match bus {
        Some(7) => Bus::Usb,
        Some(12) => Bus::Sd,
        Some(13) => Bus::Mmc,
        Some(3 | 11) => Bus::Ata,
        Some(17) => Bus::Nvme,
        Some(1 | 10) => Bus::Scsi,
        Some(14..=16) => Bus::Virtual,
        Some(0) | None => Bus::Unknown,
        Some(other) => Bus::Other(format!("bus {other}")),
    }
}

impl WinVolume {
    /// What the listings call it: its letter, else its NT device.
    fn name(&self) -> &str {
        self.letters.first().unwrap_or(&self.nt)
    }

    fn fat(&self) -> bool {
        matches!(self.fs.as_deref(), Ok("FAT32" | "FAT"))
    }

    fn esp(&self) -> bool {
        match self.style {
            Some(0) => self.mbr_type == ESP_MBR,
            Some(1) => self.gpt_type == ESP_GUID,
            _ => false,
        }
    }
}

/// Every lettered volume of a removable or fixed drive with its medium in,
/// judged against every volume Windows reported.
pub(crate) fn judge_all(volumes: &[WinVolume], system: &WinSystem) -> Vec<Found> {
    let same = |a: &str, b: &Option<String>| b.as_deref().is_some_and(|b| a.eq_ignore_ascii_case(b));
    // The system's disks, and why each is.
    let mut system_of: HashMap<u32, Vec<String>> = HashMap::new();
    for v in volumes {
        let mut why = Vec::new();
        if same(&v.guid, &system.boot) {
            why.push(format!("the Windows volume {}", v.name()));
        }
        if same(&v.nt, &system.system) {
            why.push(format!("the system partition {}", v.name()));
        }
        if v.pagefile || system.pages.iter().any(|p| p.eq_ignore_ascii_case(&v.guid)) {
            why.push(format!("a page file on {}", v.name()));
        }
        if v.esp() {
            why.push(format!("an EFI system partition {}", v.name()));
        }
        for d in &v.disks {
            let reasons = system_of.entry(*d).or_default();
            for w in &why {
                if !reasons.contains(w) {
                    reasons.push(w.clone());
                }
            }
        }
    }

    let mut out = Vec::new();
    for v in volumes {
        let drive = matches!(v.drive_type, DRIVE_REMOVABLE | DRIVE_FIXED);
        let candidate = drive && v.fs != Err(Unread::NoMedia);
        let Some(letter) = v.letters.first().filter(|_| candidate) else { continue };
        let mut others = Vec::new();
        let beside = |u: &&WinVolume| u.guid != v.guid && u.disks.iter().any(|d| v.disks.contains(d));
        for u in volumes.iter().filter(beside) {
            match &u.fs {
                // A FAT one with no letter is the unmounted FAT partition
                // Linux and macOS pass too.
                Ok(_) if u.fat() && u.letters.is_empty() => {}
                Err(Unread::NoMedia) => {}
                Ok(fs) => others.push(format!("{fs} {}", u.name())),
                Err(Unread::Raw) => others.push(format!("RAW {}", u.name())),
                Err(Unread::Error(e)) => others.push(format!("unreadable {} (error {e})", u.name())),
            }
        }
        let fs = match v.fs.as_deref() {
            Ok("FAT32") => Fs::Fat32,
            // FAT12 or FAT16, told apart by the count; a count Windows
            // didn't give reads as FAT16.
            Ok("FAT") => match v.space.map(|s| s.clusters) {
                Some(n @ 1..) if Fs::fat_of_clusters(n) == Fs::Fat12 => Fs::Fat12,
                _ => Fs::Fat16,
            },
            Ok("exFAT") => Fs::ExFat,
            Ok("NTFS") => Fs::Ntfs,
            Ok(other) => Fs::Other(other.to_string()),
            Err(Unread::Raw) => Fs::Other("RAW".into()),
            Err(_) => Fs::Other("unreadable".into()),
        };
        let table = match v.style {
            Some(0) => Table::Mbr,
            Some(1) => Table::Gpt,
            Some(2) => Table::None,
            _ => Table::Unknown,
        };
        let id = match v.disks.as_slice() {
            [one] => format!("PhysicalDrive{one}"),
            [] => "unknown".to_string(),
            many => format!("spanned {many:?}"),
        };
        let volume = Volume {
            path: letter.into(),
            device: v.guid.clone(),
            fs,
            table,
            label: v.label.clone(),
            serial: v.fs.is_ok().then_some(Serial::Fat(v.serial)),
            space: v.space,
            read_only: v.flags & FILE_READ_ONLY_VOLUME != 0,
            utf8: None,
            clock: Clock::Bias,
        };
        let mut system: Vec<String> = Vec::new();
        for d in &v.disks {
            for why in system_of.get(d).into_iter().flatten() {
                if !system.contains(why) {
                    system.push(why.clone());
                }
            }
        }
        // A volume on several disks is no card: none of them is its own.
        let bus = if v.disks.len() == 1 { bus_of(v.bus) } else { Bus::Virtual };
        let removable = v.removable && v.disks.len() == 1;
        let disk = Disk { id, removable, bus, read_only: false, system, others };
        out.push(Found::new(volume, disk));
    }
    out
}

/// This machine's volumes, judged.
#[cfg(windows)]
pub(crate) fn find() -> std::io::Result<Vec<Found>> {
    let (volumes, system) = sys::probe()?;
    Ok(judge_all(&volumes, &system))
}

/// The FAT serial of the volume at `root` (`E:\`), as GetVolumeInformationW
/// gives it now: what a run reads again to be sure the card is still the
/// one it was bound to. None when the drive has no volume now (pulled).
#[cfg(windows)]
pub(crate) fn serial(root: &std::path::Path) -> Option<Serial> {
    sys::serial(root).map(Serial::Fat)
}

/// The card's wall-clock time of a file Windows dates `utc` (Unix seconds):
/// FileTimeToLocalFileTime, which converts with the bias in force now, as
/// the FAT driver did (2.3.4). Elsewhere, unchanged.
pub(crate) fn local_wall(utc: i64) -> i64 {
    #[cfg(windows)]
    return sys::local_wall(utc);
    #[allow(unreachable_code)]
    utc
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_NOT_READY, ERROR_SHARING_VIOLATION, ERROR_UNRECOGNIZED_VOLUME, FILETIME,
        GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_READ, FILE_SHARE_WRITE, FileTimeToLocalFileTime, FindFirstVolumeW, FindNextVolumeW,
        FindVolumeClose, GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW, GetFileAttributesW,
        GetLogicalDrives, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
        INVALID_FILE_ATTRIBUTES, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, QueryDosDeviceW, STORAGE_BUS_TYPE,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetThreadErrorMode,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        IOCTL_DISK_GET_PARTITION_INFO_EX, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
        PARTITION_INFORMATION_EX, PARTITION_STYLE_GPT, PARTITION_STYLE_MBR, PropertyStandardQuery,
        STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_MULTI_SZ, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };

    use super::super::Space;
    use super::{Unread, WinSystem, WinVolume};

    /// NO_MEDIA_IN_DRIVE: another way an empty reader answers.
    const ERROR_NO_MEDIA_IN_DRIVE: u32 = 1112;
    /// FAT's epoch-free conversions: seconds from 1601 (FILETIME's) to 1970.
    const FILETIME_UNIX: i64 = 11_644_473_600;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// The text before the first NUL.
    fn narrow(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// The thread's error mode for the probe: no "insert a disk" dialog for
    /// an empty reader. The old mode comes back on drop.
    struct Quiet(u32);

    impl Quiet {
        fn new() -> Quiet {
            let mut old = 0;
            // SAFETY: no pointers but the out-parameter for the old mode.
            unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX, &mut old) };
            Quiet(old)
        }
    }

    impl Drop for Quiet {
        fn drop(&mut self) {
            // SAFETY: as above; the mode the probe found.
            unsafe { SetThreadErrorMode(self.0, null_mut()) };
        }
    }

    /// Every volume, its letters, and where Windows lives.
    pub(super) fn probe() -> io::Result<(Vec<WinVolume>, WinSystem)> {
        let _quiet = Quiet::new();
        let mut volumes = Vec::new();
        let mut name = [0u16; 64];
        // SAFETY: the buffer outlives the call, its length in characters
        // given.
        let search = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
        if search == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        loop {
            volumes.push(volume(&narrow(&name)));
            // SAFETY: as above, with the search FindFirstVolumeW began.
            if unsafe { FindNextVolumeW(search, name.as_mut_ptr(), name.len() as u32) } == 0 {
                break;
            }
        }
        // SAFETY: the search handle, closed once.
        unsafe { FindVolumeClose(search) };

        // SAFETY: no arguments.
        let drives = unsafe { GetLogicalDrives() };
        for (i, letter) in (b'A'..=b'Z').enumerate() {
            if drives >> i & 1 == 0 {
                continue;
            }
            let root = format!("{}:\\", letter as char);
            let Some(guid) = volume_of(&root) else { continue };
            if let Some(v) = volumes.iter_mut().find(|v| v.guid.eq_ignore_ascii_case(&guid)) {
                v.letters.push(root.clone());
                // SAFETY: a NUL-terminated root path.
                v.drive_type = unsafe { GetDriveTypeW(wide(&root).as_ptr()) };
                let paged = ["pagefile.sys", "swapfile.sys"].iter().any(|f| exists(&format!("{root}{f}")));
                v.pagefile = v.fs.is_ok() && paged;
            }
        }

        let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
        let windows = registry(key, "SystemRoot").and_then(|v| v.into_iter().next());
        let windows = windows.or_else(|| std::env::var("SystemRoot").ok());
        let memory = r"SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management";
        let pages = registry(memory, "ExistingPageFiles").unwrap_or_default();
        let system = WinSystem {
            boot: windows.as_deref().and_then(|path| volume_of(&mount_point(path)?)),
            system: registry(r"SYSTEM\Setup", "SystemPartition").and_then(|v| v.into_iter().next()),
            pages: pages.iter().filter_map(|p| volume_of(&mount_point(p.trim_start_matches(r"\??\"))?)).collect(),
        };
        Ok((volumes, system))
    }

    /// The serial of the volume at a root path (`E:\`, or a folder it is
    /// mounted on), with no dialog for an empty reader.
    pub(super) fn serial(root: &std::path::Path) -> Option<u32> {
        let _quiet = Quiet::new();
        let mut path = root.to_string_lossy().into_owned();
        if !path.ends_with('\\') {
            path.push('\\');
        }
        let mut serial = 0u32;
        // SAFETY: a NUL-terminated root path and one out-parameter; the
        // buffers not asked for are null with a length of 0.
        let read = unsafe {
            GetVolumeInformationW(
                wide(&path).as_ptr(),
                null_mut(),
                0,
                &mut serial,
                null_mut(),
                null_mut(),
                null_mut(),
                0,
            )
        } != 0;
        read.then_some(serial)
    }

    /// What Windows says of one volume, by its `\\?\Volume{…}\` name.
    fn volume(guid: &str) -> WinVolume {
        let mut v = WinVolume { guid: guid.to_string(), ..WinVolume::default() };
        let root = wide(guid);
        let bare = guid.trim_start_matches(r"\\?\").trim_end_matches('\\');
        let mut nt = [0u16; 512];
        // SAFETY: a NUL-terminated device name and a buffer whose length in
        // characters is given.
        if unsafe { QueryDosDeviceW(wide(bare).as_ptr(), nt.as_mut_ptr(), nt.len() as u32) } != 0 {
            v.nt = narrow(&nt);
        }
        // SAFETY: a NUL-terminated root path.
        v.drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };

        let (mut label, mut fs) = ([0u16; 261], [0u16; 261]);
        let (mut serial, mut longest, mut flags) = (0u32, 0u32, 0u32);
        // SAFETY: a NUL-terminated root path, two buffers whose lengths in
        // characters are given, and three out-parameters.
        let read = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut longest,
                &mut flags,
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } != 0;
        v.fs = if read {
            Ok(narrow(&fs))
        } else {
            // SAFETY: no arguments; the failure just above.
            match unsafe { GetLastError() } {
                ERROR_NOT_READY | ERROR_NO_MEDIA_IN_DRIVE => Err(Unread::NoMedia),
                ERROR_UNRECOGNIZED_VOLUME => Err(Unread::Raw),
                e => Err(Unread::Error(e)),
            }
        };
        (v.label, v.serial, v.flags) = (narrow(&label), serial, flags);

        // The queries every user may make, on a handle opened for none.
        let mut no_access = OpenOptions::new();
        no_access.access_mode(0).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        if let Ok(device) = no_access.open(format!(r"\\?\{bare}")) {
            v.disks = disks(&device);
            if let Some((removable, bus)) = storage(&device) {
                (v.removable, v.bus) = (removable, Some(bus));
            }
            let partition = ask::<PARTITION_INFORMATION_EX>(&device, IOCTL_DISK_GET_PARTITION_INFO_EX, &[]);
            if let Some(p) = partition {
                v.style = Some(p.PartitionStyle);
                if p.PartitionStyle == PARTITION_STYLE_MBR {
                    // SAFETY: the union's MBR member, as the style says.
                    v.mbr_type = unsafe { p.Anonymous.Mbr.PartitionType };
                } else if p.PartitionStyle == PARTITION_STYLE_GPT {
                    // SAFETY: the union's GPT member, as the style says.
                    let g = unsafe { p.Anonymous.Gpt.PartitionType };
                    v.gpt_type = format!(
                        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{}",
                        g.data1,
                        g.data2,
                        g.data3,
                        g.data4[0],
                        g.data4[1],
                        g.data4[2..].iter().map(|b| format!("{b:02x}")).collect::<String>()
                    );
                }
            }
        }
        if v.fs.is_ok() {
            v.space = space(&root);
        }
        v
    }

    /// One IOCTL whose answer is a `T` (plain old data), its input `input`.
    fn ask<T: Copy>(device: &File, code: u32, input: &[u8]) -> Option<T> {
        let mut out = MaybeUninit::<T>::zeroed();
        let mut got = 0u32;
        // SAFETY: the handle is open for the call; the input slice and the
        // output buffer, of the sizes given, outlive it; no overlapped I/O.
        let ok = unsafe {
            DeviceIoControl(
                device.as_raw_handle(),
                code,
                if input.is_empty() { null() } else { input.as_ptr().cast::<c_void>() },
                input.len() as u32,
                out.as_mut_ptr().cast::<c_void>(),
                size_of::<T>() as u32,
                &mut got,
                null_mut(),
            )
        } != 0;
        // SAFETY: zeroed, then filled by the driver as far as it answers:
        // any bit pattern is a valid T for these plain structs.
        (ok && got > 0).then(|| unsafe { out.assume_init() })
    }

    /// The disk numbers a volume is on: its device number, or its extents
    /// when it spans several disks (which have no one number).
    fn disks(device: &File) -> Vec<u32> {
        if let Some(n) = ask::<STORAGE_DEVICE_NUMBER>(device, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[]) {
            return vec![n.DeviceNumber];
        }
        // VOLUME_DISK_EXTENTS: a count, then 24-byte extents from offset 8,
        // each led by its disk number.
        let Some(raw) = ask::<[u64; 128]>(device, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &[]) else {
            return Vec::new();
        };
        let bytes: Vec<u8> = raw.iter().flat_map(|w| w.to_le_bytes()).collect();
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap_or_default()) as usize;
        let mut out: Vec<u32> = (0..count.min(42))
            .map(|i| u32::from_le_bytes(bytes[8 + i * 24..12 + i * 24].try_into().unwrap_or_default()))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The disk's removable flag and bus, from its device descriptor.
    fn storage(device: &File) -> Option<(bool, STORAGE_BUS_TYPE)> {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        // SAFETY: the query is plain old data, viewed as its bytes for the
        // length of the call.
        let bytes = (&query as *const STORAGE_PROPERTY_QUERY).cast::<u8>();
        let input = unsafe { std::slice::from_raw_parts(bytes, size_of_val(&query)) };
        // The descriptor's fixed part; its strings past it aren't needed.
        let d = ask::<STORAGE_DEVICE_DESCRIPTOR>(device, IOCTL_STORAGE_QUERY_PROPERTY, input)?;
        Some((d.RemovableMedia, d.BusType))
    }

    fn space(root: &[u16]) -> Option<Space> {
        let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: a NUL-terminated root path and three out-parameters.
        if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
            return None;
        }
        let (mut per_cluster, mut per_sector, mut unused, mut clusters) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: as above, four out-parameters.
        let ok = unsafe {
            GetDiskFreeSpaceW(root.as_ptr(), &mut per_cluster, &mut per_sector, &mut unused, &mut clusters)
        } != 0;
        let cluster = if ok { per_cluster * per_sector } else { 0 };
        Some(Space { size: total, free, cluster, clusters: if ok { clusters as u64 } else { 0 } })
    }

    /// A file's there (or locked, as a page file in use is).
    fn exists(path: &str) -> bool {
        // SAFETY: a NUL-terminated path.
        if unsafe { GetFileAttributesW(wide(path).as_ptr()) } != INVALID_FILE_ATTRIBUTES {
            return true;
        }
        // SAFETY: no arguments; the failure just above.
        matches!(unsafe { GetLastError() }, ERROR_SHARING_VIOLATION | ERROR_ACCESS_DENIED)
    }

    /// The root of the volume a path is on: `C:\Windows` is on `C:\`.
    fn mount_point(path: &str) -> Option<String> {
        let mut out = [0u16; 512];
        // SAFETY: a NUL-terminated path and a buffer whose length in
        // characters is given.
        let path = wide(path);
        let ok = unsafe { GetVolumePathNameW(path.as_ptr(), out.as_mut_ptr(), out.len() as u32) } != 0;
        ok.then(|| narrow(&out))
    }

    /// The `\\?\Volume{…}\` name of a mount point (`E:\`).
    fn volume_of(root: &str) -> Option<String> {
        let mut out = [0u16; 64];
        // SAFETY: a NUL-terminated root path with its trailing backslash, and
        // a buffer whose length in characters is given.
        let root = wide(root);
        let ok = unsafe { GetVolumeNameForVolumeMountPointW(root.as_ptr(), out.as_mut_ptr(), out.len() as u32) };
        (ok != 0).then(|| narrow(&out))
    }

    /// A string or multi-string value under HKEY_LOCAL_MACHINE, each string
    /// of it; None when it isn't there.
    fn registry(key: &str, value: &str) -> Option<Vec<String>> {
        let mut buf = vec![0u16; 8192];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: NUL-terminated key and value names, and a buffer whose size
        // in bytes is given; the call writes at most that and says how much.
        let e = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                wide(key).as_ptr(),
                wide(value).as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_MULTI_SZ | RRF_SUBKEY_WOW6464KEY,
                null_mut(),
                buf.as_mut_ptr().cast::<c_void>(),
                &mut bytes,
            )
        };
        if e != 0 {
            return None;
        }
        let chars = &buf[..(bytes as usize / 2).min(buf.len())];
        Some(chars.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect())
    }

    pub(super) fn local_wall(utc: i64) -> i64 {
        let ticks = ((utc + FILETIME_UNIX) as u64).saturating_mul(10_000_000);
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        let mut local = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        // SAFETY: two FILETIMEs that outlive the call.
        if unsafe { FileTimeToLocalFileTime(&ft, &mut local) } == 0 {
            return utc;
        }
        let ticks = (local.dwHighDateTime as u64) << 32 | local.dwLowDateTime as u64;
        (ticks / 10_000_000) as i64 - FILETIME_UNIX
    }
}

#[cfg(test)]
mod tests {
    use super::super::Refusal;
    use super::*;

    fn v(guid: &str, nt: &str, letter: Option<&str>, fs: Result<&str, Unread>, disk: u32) -> WinVolume {
        WinVolume {
            guid: format!(r"\\?\Volume{{{guid}}}\"),
            nt: format!(r"\Device\HarddiskVolume{nt}"),
            letters: letter.map(String::from).into_iter().collect(),
            drive_type: if letter.is_some() { DRIVE_FIXED } else { 0 },
            fs: fs.map(String::from),
            serial: 0x1A2B_3C4D,
            disks: vec![disk],
            bus: Some(17),
            style: Some(1),
            space: Some(Space { size: 1 << 30, free: 1 << 29, cluster: 4096, clusters: 262_144 }),
            ..WinVolume::default()
        }
    }

    /// A Windows PC: disk 0 is its NVMe system disk (GPT: the EFI partition,
    /// C:, a recovery partition, and a FAT32 data partition D:); disk 1 a
    /// card in a USB reader (E:, FAT32 on MBR); disk 2 a USB disk with exFAT
    /// (F:); disk 3 a reader slot with no card (G:).
    fn pc() -> (Vec<WinVolume>, WinSystem) {
        let mut esp = v("e0", "1", None, Err(Unread::Error(5)), 0);
        esp.gpt_type = ESP_GUID.into();
        let c = v("c0", "3", Some("C:\\"), Ok("NTFS"), 0);
        let recovery = v("r0", "4", None, Ok("NTFS"), 0);
        let d = v("d0", "5", Some("D:\\"), Ok("FAT32"), 0);
        let mut e = v("e1", "7", Some("E:\\"), Ok("FAT32"), 1);
        (e.drive_type, e.removable, e.bus, e.style) = (DRIVE_REMOVABLE, true, Some(7), Some(0));
        e.space =
            Some(Space { size: 63_850_000_000, free: 63_000_000_000, cluster: 32_768, clusters: 1_948_547 });
        let mut f = v("f2", "8", Some("F:\\"), Ok("exFAT"), 2);
        (f.bus, f.style) = (Some(7), Some(0));
        let mut g = v("g3", "9", Some("G:\\"), Err(Unread::NoMedia), 3);
        (g.drive_type, g.removable, g.bus, g.style, g.space) = (DRIVE_REMOVABLE, true, Some(7), None, None);
        let system = WinSystem {
            boot: Some(c.guid.clone()),
            system: Some(esp.nt.clone()),
            pages: vec![c.guid.to_ascii_uppercase()],
        };
        (vec![esp, c, recovery, d, e, f, g], system)
    }

    fn at<'a>(found: &'a [Found], letter: &str) -> &'a Found {
        let at = |f: &&Found| f.volume.path == std::path::Path::new(letter);
        found.iter().find(at).unwrap_or_else(|| panic!("no {letter}"))
    }

    fn codes(f: &Found) -> Vec<&'static str> {
        f.refusals.iter().map(Refusal::code).collect()
    }

    #[test]
    fn the_system_disk_is_refused_and_the_card_in_the_reader_taken() {
        let (volumes, system) = pc();
        let found = judge_all(&volumes, &system);
        assert_eq!(found.len(), 4, "C: D: E: F:; G: is empty and the rest have no letter: {found:#?}");

        let c = at(&found, "C:\\");
        assert_eq!(c.disk.id, "PhysicalDrive0");
        assert_eq!(
            c.disk.system,
            [
                "the system partition \\Device\\HarddiskVolume1",
                "an EFI system partition \\Device\\HarddiskVolume1",
                "the Windows volume C:\\",
                "a page file on C:\\",
            ]
        );
        assert_eq!(codes(c), ["system-disk", "fixed-disk", "shared-disk", "ntfs", "gpt"]);

        // A FAT32 partition on the system disk is no card, whatever it is.
        let d = at(&found, "D:\\");
        let others = [r"unreadable \Device\HarddiskVolume1 (error 5)", r"NTFS C:\", r"NTFS \Device\HarddiskVolume4"];
        assert_eq!(d.disk.others, others);
        assert_eq!(codes(d), ["system-disk", "fixed-disk", "shared-disk", "gpt"]);

        let e = at(&found, "E:\\");
        assert!(e.accepted(), "{e:#?}");
        assert_eq!((e.disk.bus.clone(), e.disk.removable), (Bus::Usb, true));
        assert_eq!(e.volume.serial, Some(Serial::Fat(0x1A2B_3C4D)));
        assert_eq!(e.volume.serial.as_ref().unwrap().to_string(), "1A2B-3C4D");
        assert_eq!(e.volume.clock, Clock::Bias);

        // A USB disk passes by its bus but holds exFAT.
        assert_eq!(codes(at(&found, "F:\\")), ["exfat"]);
    }

    #[test]
    fn a_card_with_a_partition_windows_cant_read_is_refused() {
        // A Pi's card in the reader: its FAT boot partition as E:, its ext4
        // root as a RAW volume with no letter.
        let (mut volumes, system) = pc();
        let mut root = v("r1", "10", None, Err(Unread::Raw), 1);
        root.style = Some(0);
        volumes.push(root);
        let found = judge_all(&volumes, &system);
        let e = at(&found, "E:\\");
        assert_eq!(e.disk.others, ["RAW \\Device\\HarddiskVolume10"]);
        assert_eq!(codes(e), ["shared-disk"]);
    }

    #[test]
    fn a_second_fat_partition_shares_the_card_when_it_has_a_letter() {
        // As on Linux and macOS, where another mounted FAT filesystem on the
        // card's disk refuses it and an unmounted one doesn't.
        let (mut volumes, system) = pc();
        let mut second = v("s1", "12", Some("K:\\"), Ok("FAT32"), 1);
        (second.drive_type, second.removable, second.bus, second.style) =
            (DRIVE_REMOVABLE, true, Some(7), Some(0));
        volumes.push(second);
        let found = judge_all(&volumes, &system);
        assert_eq!(at(&found, "E:\\").disk.others, ["FAT32 K:\\"]);
        assert_eq!(codes(at(&found, "E:\\")), ["shared-disk"]);
        assert_eq!(codes(at(&found, "K:\\")), ["shared-disk"]);

        volumes.last_mut().unwrap().letters.clear();
        let found = judge_all(&volumes, &system);
        assert!(at(&found, "E:\\").accepted(), "{found:#?}");
    }

    #[test]
    fn a_stick_windows_boots_from_is_the_systems_and_locked_or_fat12_cards_say_why() {
        // The PC boots through an EFI partition on a USB stick (FAT32, MBR
        // type 0xEF, lettered by mistake) that Windows calls its system
        // partition.
        let (mut volumes, mut system) = pc();
        let mut boot = v("b4", "11", Some("H:\\"), Ok("FAT32"), 4);
        (boot.drive_type, boot.removable, boot.bus, boot.style, boot.mbr_type) =
            (DRIVE_REMOVABLE, true, Some(7), Some(0), ESP_MBR);
        system.system = Some(boot.nt.clone());
        volumes.push(boot);
        let found = judge_all(&volumes, &system);
        assert_eq!(
            at(&found, "H:\\").disk.system,
            ["the system partition H:\\", "an EFI system partition H:\\"]
        );

        let (mut volumes, system) = pc();
        let e = volumes.iter_mut().find(|v| v.letters == ["E:\\"]).unwrap();
        e.flags |= FILE_READ_ONLY_VOLUME;
        e.fs = Ok("FAT".into());
        e.space = Some(Space { size: 16 << 20, free: 0, cluster: 4096, clusters: 4000 });
        let found = judge_all(&volumes, &system);
        assert_eq!(codes(at(&found, "E:\\")), ["fat12", "read-only"]);

        let (mut volumes, system) = pc();
        let e = volumes.iter_mut().find(|v| v.letters == ["E:\\"]).unwrap();
        (e.fs, e.space) = (Ok("FAT".into()), None);
        assert_eq!(at(&judge_all(&volumes, &system), "E:\\").volume.fs, Fs::Fat16);
    }

    #[test]
    fn a_spanned_volume_is_no_card() {
        let (mut volumes, system) = pc();
        let e = volumes.iter_mut().find(|v| v.letters == ["E:\\"]).unwrap();
        e.disks = vec![1, 5];
        let found = judge_all(&volumes, &system);
        let e = at(&found, "E:\\");
        assert_eq!(e.disk.id, "spanned [1, 5]");
        assert_eq!(codes(e), ["fixed-disk"]);
    }

    #[test]
    fn bus_types_read_as_ntddstor_numbers_them() {
        assert_eq!(bus_of(Some(7)), Bus::Usb);
        assert_eq!(bus_of(Some(12)), Bus::Sd);
        assert_eq!(bus_of(Some(13)), Bus::Mmc);
        assert_eq!(bus_of(Some(17)), Bus::Nvme);
        assert_eq!(bus_of(Some(15)), Bus::Virtual);
        assert_eq!(bus_of(None), Bus::Unknown);
        assert_eq!(bus_of(Some(8)), Bus::Other("bus 8".into()));
    }
}
