//! Asking the OS to let a card go, so it can come out of the reader with
//! nothing left unwritten: the Card view's *Eject* and `device card <drive>
//! eject` (the MP3 Player contract's clauses 54 and 65). A card pulled
//! before the OS lets go can lose its last writes, which is why Done's
//! primary is *Eject ▸* (the user's pick of 2026-10-19, card 07 §7, choice
//! 12).
//!
//! Each OS has its own way, and each is asked of a volume the tab lists
//! only — any removable card, one the player can't read too (an exFAT
//! card's *Eject ▸* is the first step of the route to the player's Format,
//! clause 45), never the computer's own disks or a fixed disk:
//! - **Windows:** the volume locked, dismounted and its medium ejected
//!   (`FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`,
//!   `IOCTL_STORAGE_EJECT_MEDIA`). Many card readers have no eject of their
//!   own: once the volume is dismounted the card can come out all the same.
//! - **Linux:** `udisksctl unmount`, then `power-off` of its disk (the
//!   desktop's own *Safely remove*), else `umount`.
//! - **macOS:** `diskutil eject`.
//!
//! The words the OS gives when it can't (a file open on the card, say) are
//! what the listener reads.

use super::{Found, Refusal};

/// One command a step runs: the program and its arguments.
pub(crate) type Step = Vec<String>;

/// What Linux runs for `found`, in order: the first that works lets the
/// card go; `power-off` after `udisksctl unmount` is best effort (a reader
/// may not power off).
pub(crate) fn linux_steps(found: &Found) -> Vec<Step> {
    let dev = |name: &str| match name.starts_with('/') {
        true => name.to_string(),
        false => format!("/dev/{name}"),
    };
    let volume = dev(&found.volume.device);
    let mount = found.volume.path.to_string_lossy().to_string();
    vec![
        vec!["udisksctl".into(), "unmount".into(), "-b".into(), volume],
        vec!["udisksctl".into(), "power-off".into(), "-b".into(), dev(&found.disk.id)],
        vec!["umount".into(), mount],
    ]
}

/// What macOS runs for `found`: one `diskutil eject` of its volume.
pub(crate) fn macos_steps(found: &Found) -> Vec<Step> {
    let mount = found.volume.path.to_string_lossy().to_string();
    vec![vec!["/usr/sbin/diskutil".into(), "eject".into(), mount]]
}

/// The drive-safety rule's word on ejecting `found`: a removable card,
/// whatever its file system, may be let go; the computer's own disks and
/// fixed disks are never asked of the OS.
pub(crate) fn may_eject(found: &Found) -> Result<(), String> {
    let own = found.refusals.iter().any(|r| matches!(r, Refusal::System(_) | Refusal::Fixed));
    if own {
        return Err("one of this computer's own disks, not a card".into());
    }
    Ok(())
}

/// Lets the card of `found` go, or says why the OS wouldn't. Only a volume
/// [`may_eject`] lets go is ever asked for.
pub(crate) fn eject(found: &Found) -> Result<(), String> {
    may_eject(found)?;
    #[cfg(windows)]
    return windows(found);
    #[cfg(target_os = "linux")]
    return linux(found);
    #[cfg(target_os = "macos")]
    return run(&macos_steps(found)[0]);
    #[allow(unreachable_code)]
    Err("this system has no eject the player knows".into())
}

/// One step run, its own words when it fails.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run(step: &Step) -> Result<(), String> {
    let out = std::process::Command::new(&step[0]).args(&step[1..]).output();
    let out = out.map_err(|e| format!("{}: {e}", step[0]))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if said.is_empty() { format!("{} said no ({})", step[0], out.status) } else { said })
}

#[cfg(target_os = "linux")]
fn linux(found: &Found) -> Result<(), String> {
    let steps = linux_steps(found);
    match run(&steps[0]) {
        Ok(()) => {
            let _ = run(&steps[1]);
            Ok(())
        }
        Err(why) => run(&steps[2]).map_err(|_| why),
    }
}

#[cfg(windows)]
fn windows(found: &Found) -> Result<(), String> {
    use std::ffi::c_void;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::{
        FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_STORAGE_EJECT_MEDIA,
        IOCTL_STORAGE_MEDIA_REMOVAL,
    };

    let root = found.volume.path.to_string_lossy().to_string();
    let letter = root.trim_end_matches(['\\', '/']);
    if !(letter.len() == 2 && letter.ends_with(':')) {
        return Err(format!("{root} is not a drive letter"));
    }
    let device = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(format!(r"\\.\{letter}"))
        .map_err(|e| e.to_string())?;
    let ask = |code: u32, input: &[u8]| -> Result<(), String> {
        let mut got = 0u32;
        // SAFETY: the handle is open for the call; the input slice, of the
        // size given, outlives it; these codes return no output; no
        // overlapped I/O.
        let ok = unsafe {
            DeviceIoControl(
                device.as_raw_handle(),
                code,
                if input.is_empty() { null() } else { input.as_ptr().cast::<c_void>() },
                input.len() as u32,
                null_mut(),
                0,
                &mut got,
                null_mut(),
            )
        } != 0;
        if ok { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
    };
    // A file open on the card refuses the lock: the OS's words say so.
    ask(FSCTL_LOCK_VOLUME, &[])?;
    ask(FSCTL_DISMOUNT_VOLUME, &[])?;
    // Let the medium go where the reader can: not every reader can, and a
    // dismounted card comes out safely all the same.
    let _ = ask(IOCTL_STORAGE_MEDIA_REMOVAL, &[0]);
    let _ = ask(IOCTL_STORAGE_EJECT_MEDIA, &[]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::card::volume::{Bus, Clock, Disk, Fs, Serial, Table, Volume};

    fn card(path: &str, device: &str, disk: &str) -> Found {
        let volume = Volume {
            path: path.into(),
            device: device.into(),
            fs: Fs::Fat32,
            table: Table::Mbr,
            label: "CARD".into(),
            serial: Some(Serial::Fat(0x1A2B_3C4D)),
            space: None,
            read_only: false,
            utf8: Some(true),
            clock: Clock::Offset(0),
        };
        let disk = Disk {
            id: disk.into(),
            removable: true,
            bus: Bus::Usb,
            read_only: false,
            system: Vec::new(),
            others: Vec::new(),
        };
        Found::new(volume, disk)
    }

    #[test]
    fn each_system_lets_the_card_go_its_own_way_and_a_refused_volume_is_never_asked() {
        let found = card("/media/card", "sdb1", "sdb");
        let steps = linux_steps(&found);
        assert_eq!(steps[0], ["udisksctl", "unmount", "-b", "/dev/sdb1"]);
        assert_eq!(steps[1], ["udisksctl", "power-off", "-b", "/dev/sdb"]);
        assert_eq!(steps[2], ["umount", "/media/card"]);
        let mac = card("/Volumes/CARD", "disk4s1", "disk4");
        assert_eq!(macos_steps(&mac), [["/usr/sbin/diskutil", "eject", "/Volumes/CARD"]]);

        // A volume the drive-safety rule refuses is never asked of the OS.
        let mut refused = card("/media/card", "sdb1", "sdb");
        refused.disk.system = vec!["/".into()];
        refused.refusals = crate::device::card::volume::judge(&refused.disk, &refused.volume);
        assert!(eject(&refused).is_err());
    }

    #[test]
    fn a_card_the_player_cant_read_may_be_ejected_and_a_fixed_disk_never() {
        let judged = |found: &mut Found| {
            found.refusals = crate::device::card::volume::judge(&found.disk, &found.volume);
        };
        // An exFAT card, a GPT one, a write-protected one: Eject ▸ is their
        // way to the player's Format (clause 45).
        let kinds = [
            (Fs::ExFat, Table::Mbr, false),
            (Fs::Fat32, Table::Gpt, false),
            (Fs::Fat32, Table::Mbr, true),
        ];
        for (fs, table, locked) in kinds {
            let mut found = card("/media/card", "sdb1", "sdb");
            (found.volume.fs, found.volume.table, found.volume.read_only) = (fs, table, locked);
            judged(&mut found);
            assert!(!found.accepted(), "{:?}", found.refusals);
            assert_eq!(may_eject(&found), Ok(()), "{:?}", found.refusals);
        }
        let mut fixed = card("/media/data", "sda2", "sda");
        (fixed.disk.removable, fixed.disk.bus) = (false, Bus::Ata);
        judged(&mut fixed);
        assert!(may_eject(&fixed).is_err() && eject(&fixed).is_err(), "never asked of the OS");
    }
}
