//! The desktop app's identity: the names the operating systems know the
//! player's window by, which a desktop entry, a dock, a taskbar and a
//! code signature must all agree on.
//!
//! Std alone, on purpose: the Windows launcher stub (src/bin/launch.rs)
//! includes this file by path, and it links nothing but std and
//! windows-sys. The player compiles it with the `window` feature only, so
//! the terminal flavour has none of it.

/// The app's reverse-DNS id. One string in four places, which have to stay
/// equal for each to find the others:
/// - the Linux window's app_id (Wayland) and WM_CLASS (X11), which the
///   window module sets, and by which a desktop shell matches a window to
///   its desktop entry for the icon, the name and the dock's grouping;
/// - that desktop entry's file name and `StartupWMClass`
///   (assets/linux/io.mstream.player.desktop) and the icon's name in the
///   hicolor theme (assets/icons/);
/// - the macOS code-signing identifier the release legs already sign the
///   binaries with (`codesign --identifier`, .github/workflows/
///   build-binary.yml), which a later .app bundle's CFBundleIdentifier
///   will repeat;
/// - the base of the Windows AppUserModelID ([`WINDOWS_AUMID`]), which
///   is written in Windows' own `Company.Product` form instead.
// Read by the window on the free-desktop Unixes alone (macOS and Windows
// set no app id at run time), and by the tests everywhere.
#[cfg_attr(
    not(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )),
    allow(dead_code)
)]
pub const APP_ID: &str = "io.mstream.player";

/// The Windows AppUserModelID, by which the taskbar groups windows under
/// one button, pins an app and finds what a pin relaunches.
///
/// The pairing rule: every process that shows the app (the player's
/// window, the stub's message box) and every shortcut that starts it (a
/// Start-menu or desktop .lnk, which carries the id as its
/// System.AppUserModel.ID property, the installer's job) must name this
/// same id. Without an explicit id Windows derives one from the
/// executable's path, and the stub and the player are two executables:
/// the window would group apart from the shortcut that opened it, and a
/// pin made from the running window would relaunch mstream-player.exe
/// directly, console flash and all, instead of through the stub.
// Set by the desktop flavour on Windows alone; elsewhere only the tests
// read it.
#[cfg_attr(not(all(windows, feature = "desktop")), allow(dead_code))]
pub const WINDOWS_AUMID: &str = "mStream.Player";

/// Name this process [`WINDOWS_AUMID`], before it shows any window: the
/// taskbar reads the id when a window first appears, and a later change
/// does not regroup it. Best effort: a failure leaves the path-derived id,
/// which costs the grouping and nothing else.
#[cfg(all(windows, feature = "desktop"))]
pub fn set_windows_aumid() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let wide: Vec<u16> = WINDOWS_AUMID.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated UTF-16 string that outlives the call; the
    // shell copies it.
    unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Linux desktop entry, not packaged yet (the deb and rpm are the
    /// terminal flavour's; a desktop tarball is a later step), checked here
    /// so it cannot drift from the id the window sets.
    const DESKTOP_ENTRY: &str = include_str!("../assets/linux/io.mstream.player.desktop");

    /// `Key=value` lines of the `[Desktop Entry]` group, in order.
    fn entry_keys() -> Vec<(&'static str, &'static str)> {
        let mut lines = DESKTOP_ENTRY.lines().map(str::trim);
        assert_eq!(lines.find(|l| !l.is_empty() && !l.starts_with('#')), Some("[Desktop Entry]"));
        lines
            .take_while(|l| !l.starts_with('['))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.split_once('=').expect("every entry line is Key=value"))
            .collect()
    }

    #[test]
    fn the_desktop_entry_names_the_app_by_its_id() {
        let keys = entry_keys();
        let get = |key: &str| {
            let found: Vec<_> = keys.iter().filter(|(k, _)| *k == key).collect();
            assert_eq!(found.len(), 1, "{key} appears once");
            found[0].1
        };
        assert_eq!(get("Type"), "Application");
        assert_eq!(get("Name"), "mStream Player");
        assert!(!get("Comment").is_empty());
        assert_eq!(get("Exec"), "mstream-player");
        assert_eq!(get("Terminal"), "false");
        assert_eq!(get("StartupNotify"), "true");
        // The shell matches the window to this entry by WM_CLASS / app_id,
        // and finds the icon by name in the hicolor theme: both are APP_ID.
        assert_eq!(get("StartupWMClass"), APP_ID);
        assert_eq!(get("Icon"), APP_ID);
        // Lists end in a semicolon; the main category is a registered one.
        for list in ["Categories", "Keywords"] {
            assert!(get(list).ends_with(';'), "{list} ends with ';'");
        }
        let categories: Vec<_> = get("Categories").split(';').collect();
        for wanted in ["AudioVideo", "Audio", "Player"] {
            assert!(categories.contains(&wanted), "Categories has {wanted}");
        }
    }

    #[test]
    fn the_icon_files_are_named_by_the_app_id() {
        let icons = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        for size in [16, 32, 48, 64, 128, 256, 512] {
            let png = icons.join(format!("hicolor/{size}x{size}/apps/{APP_ID}.png"));
            let bytes = std::fs::read(&png).unwrap_or_else(|e| panic!("{}: {e}", png.display()));
            // The PNG signature, then IHDR's width and height: the size the
            // directory promises, without a decoder (the window's tests
            // decode them in full).
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{}", png.display());
            assert_eq!(&bytes[12..16], b"IHDR");
            let dimension = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
            assert_eq!((dimension(16), dimension(20)), (size, size), "{}", png.display());
        }
        let icns = std::fs::read(icons.join(format!("{APP_ID}.icns"))).expect("the macOS icon");
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
    }

    #[test]
    fn the_aumid_is_the_one_the_shortcuts_will_carry() {
        // This module is compiled into both the player and the launcher
        // stub, so this runs in both test builds: the two name the one id,
        // and it is the literal an installer's shortcut must carry.
        assert_eq!(WINDOWS_AUMID, "mStream.Player");
        assert_eq!(APP_ID, "io.mstream.player");
    }

    #[test]
    fn the_aumid_is_one_windows_accepts() {
        // At most 128 characters, no spaces, and Microsoft's
        // `Company.Product` shape: dotted, each part non-empty.
        assert!(WINDOWS_AUMID.len() <= 128);
        assert!(!WINDOWS_AUMID.contains(' '));
        let parts: Vec<_> = WINDOWS_AUMID.split('.').collect();
        assert!(parts.len() >= 2 && parts.iter().all(|p| !p.is_empty()), "{WINDOWS_AUMID}");
        // Its base is the app id's, in Windows' casing.
        assert_eq!(WINDOWS_AUMID.to_ascii_lowercase().replace('.', ""), "mstreamplayer");
        assert!(APP_ID.ends_with("mstream.player"));
    }
}
