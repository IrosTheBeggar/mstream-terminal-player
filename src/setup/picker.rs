//! The native folder picker, one backend per platform — and, through
//! [`pick_torrent`], the `.torrent` file picker on the same three backends,
//! shared by the GUI's Add-torrent room (docs/ux-contracts/add-torrent.md,
//! clause 2) and the admin Torrents room's seeding tab; through
//! [`pick_firmware`] and [`pick_build_folder`], the MP3 Player tab's local
//! build (docs/ux-contracts/mp3-player-screen.md, clause 36).
//!
//! Split on purpose (the mStream-side picker spike, 2026-08-21): rfd's Linux
//! backends either link libwayland-client into NEEDED (portal flavor — a
//! load failure on headless boxes, where this binary is the server-audio
//! engine) or link GTK (a Docker-image problem in five cross triples). So
//! Linux speaks to the XDG desktop portal directly through ashpd — pure Rust
//! over D-Bus, nothing linked — while macOS and Windows use rfd's native
//! NSOpenPanel / IFileOpenDialog, which link only system frameworks.
//!
//! The call blocks its caller until the user answers, so every caller runs
//! it on a worker thread and the UI stays live while a dialog is open. On a
//! headless box the Linux portal call fails in about a millisecond (no
//! session bus), which is what routes the wizard to its server-side browser
//! instead. On Windows, when the player runs in a window of its own, the
//! dialogs belong to that window ([`set_owner`]).

#[cfg(windows)]
use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::sync::atomic::{AtomicIsize, Ordering};

/// What came back from asking for a folder — or, through
/// [`pick_torrent`], a file.
pub enum Pick {
    Folder(PathBuf),
    /// [`pick_torrent`]'s answer: the chosen `.torrent`.
    File(PathBuf),
    /// The dialog opened and the user declined it.
    Cancelled,
    /// No dialog could open here (headless, no portal, unsupported OS) —
    /// the wizard offers the server-side browser instead. Only the Linux
    /// and fallback backends construct it; every platform matches it.
    #[allow(dead_code)]
    Unavailable(String),
}

pub const DIALOG_TITLE: &str = "Add a music folder to mStream";
/// The GUI's title for the `.torrent` dialog; the admin room passes its
/// own localized one.
pub const TORRENT_TITLE: &str = "Choose a .torrent file for mStream";

/// `MSTREAM_NO_PICKER`: refuse to open the torrent and firmware dialogs at
/// all — the harness seam that lets a pty smoke reach the typed fallback
/// instead of popping a window on whoever is running it.
fn torrent_dialogs_disabled() -> bool {
    std::env::var("MSTREAM_NO_PICKER").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The AppleScript for the torrent dialog: typed to the extension, and
/// opening in `start` when there is one. The title and the path land
/// inside quoted strings, so their quotes and backslashes are escaped.
#[cfg(any(target_os = "macos", test))]
fn torrent_script(title: &str, start: Option<&Path>) -> String {
    file_script("torrent", title, start)
}

/// The AppleScript for a file dialog typed to `extension`, opening in
/// `start` when there is one: the torrent's and the firmware image's.
#[cfg(any(target_os = "macos", test))]
fn file_script(extension: &str, title: &str, start: Option<&Path>) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let location = start
        .map(|p| format!(" default location (POSIX file \"{}\")", escape(&p.to_string_lossy())))
        .unwrap_or_default();
    format!(
        "POSIX path of (choose file of type {{\"{extension}\"}} with prompt \"{}\"{location})",
        escape(title)
    )
}

/// The AppleScript for a folder dialog under `title`.
#[cfg(any(target_os = "macos", test))]
fn folder_script(title: &str) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("POSIX path of (choose folder with prompt \"{}\")", escape(title))
}

#[cfg(target_os = "macos")]
pub fn pick_folder() -> Pick {
    // Deliberately NOT rfd here: an NSOpenPanel opened by a terminal-launched
    // process stays behind every window without key focus — live testing
    // went through activation policies and activateIgnoringOtherApps and the
    // panel never fronted. osascript runs the chooser in its own app
    // context, which fronts the way every mac shell script relies on. The
    // wizard blocks on the child exactly as it would on a modal panel.
    let script = format!("POSIX path of (choose folder with prompt \"{DIALOG_TITLE}\")");
    match osascript_path(&script) {
        Ok(Some(path)) => Pick::Folder(path),
        Ok(None) => Pick::Cancelled,
        Err(why) => Pick::Unavailable(why),
    }
}

/// The `.torrent` dialog: `choose file` typed to the extension, started
/// in `start` when given — Downloads for the GUI, where a torrent lands
/// from the browser.
#[cfg(target_os = "macos")]
pub fn pick_torrent(title: &str, start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match osascript_path(&torrent_script(title, start)) {
        Ok(Some(path)) => Pick::File(path),
        Ok(None) => Pick::Cancelled,
        Err(why) => Pick::Unavailable(why),
    }
}

/// A firmware image for the MP3 player: `choose file` typed to `.bin`,
/// started in `start` when given (the folder of the last one picked).
#[cfg(target_os = "macos")]
pub fn pick_firmware(title: &str, start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match osascript_path(&file_script("bin", title, start)) {
        Ok(Some(path)) => Pick::File(path),
        Ok(None) => Pick::Cancelled,
        Err(why) => Pick::Unavailable(why),
    }
}

/// A firmware build folder (a PlatformIO `.pio/build/<env>`, or the
/// project above it).
#[cfg(target_os = "macos")]
pub fn pick_build_folder(title: &str) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match osascript_path(&folder_script(title)) {
        Ok(Some(path)) => Pick::Folder(path),
        Ok(None) => Pick::Cancelled,
        Err(why) => Pick::Unavailable(why),
    }
}

/// Run a `choose …` script and read the POSIX path it prints: `Ok(None)`
/// is the user declining (AppleScript's -128), `Err` is no dialog at all.
#[cfg(target_os = "macos")]
fn osascript_path(script: &str) -> Result<Option<PathBuf>, String> {
    match std::process::Command::new("/usr/bin/osascript").arg("-e").arg(script).output() {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok((!path.is_empty()).then(|| PathBuf::from(path)))
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            // -128 is AppleScript's "User canceled".
            if err.contains("-128") { Ok(None) } else { Err(err.trim().to_string()) }
        }
        Err(e) => Err(e.to_string()),
    }
}

// ── The dialogs' owner (Windows) ────────────────────────────────────────────

/// The HWND of the window the Windows dialogs belong to, 0 for none: the
/// player's own window while one is open. Unowned, IFileOpenDialog opened
/// wherever it last stood (the screen's top-left corner the first time),
/// left the window clickable behind it, and on closing handed the focus to
/// whatever window was next in z-order (in the v0.12.0 Windows smoke, an
/// unrelated topmost one). Owned, it is centred over the window, modal to
/// it (Show disables it until the answer) and gives it the focus back as it
/// closes. A number rather than the window itself: winit gives out a
/// window's handle on its loop's thread alone, and every dialog runs on a
/// worker thread. A terminal run never sets one, and its dialogs stay
/// unowned as before.
#[cfg(windows)]
static OWNER: AtomicIsize = AtomicIsize::new(0);

/// Name the window the dialogs open over, or with `None` withdraw it: the
/// window host (`gui::window`) sets its window's HWND as it opens it and
/// withdraws it at its teardown, before the window is destroyed. A quit
/// with a dialog up ends the run as it did unowned: the process's end takes
/// the dialog with it.
///
/// The owner lives on the loop's thread and the dialog on a worker's.
/// Windows allows that (Show disables and later re-enables the owner with
/// cross-thread messages, and joins the two threads' input while the dialog
/// is up), and neither thread waits on the other: the loop keeps drawing,
/// and the worker pumps the dialog's own modal loop.
#[cfg(windows)]
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn set_owner(hwnd: Option<NonZeroIsize>) {
    OWNER.store(hwnd.map_or(0, NonZeroIsize::get), Ordering::Release);
}

/// The published owner, if a window has named one.
#[cfg(windows)]
fn owner() -> Option<Owner> {
    NonZeroIsize::new(OWNER.load(Ordering::Acquire)).map(Owner)
}

/// An HWND in the shape rfd's `set_parent` takes. rfd reads the handle once,
/// as the dialog is built, and hands it to IFileDialog::Show as the owner.
#[cfg(windows)]
struct Owner(NonZeroIsize);

#[cfg(windows)]
impl winit::raw_window_handle::HasWindowHandle for Owner {
    fn window_handle(
        &self,
    ) -> Result<winit::raw_window_handle::WindowHandle<'_>, winit::raw_window_handle::HandleError>
    {
        use winit::raw_window_handle::{RawWindowHandle, Win32WindowHandle, WindowHandle};
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // SAFETY: the handle is only ever passed to Show as its owner, and an
        // HWND is a handle the window manager validates, not a pointer: the
        // teardown withdraws it before the window goes, and even a stale one
        // is a number for Show to look up, never freed memory to read.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(windows)]
impl winit::raw_window_handle::HasDisplayHandle for Owner {
    fn display_handle(
        &self,
    ) -> Result<winit::raw_window_handle::DisplayHandle<'_>, winit::raw_window_handle::HandleError>
    {
        Ok(winit::raw_window_handle::DisplayHandle::windows())
    }
}

/// The dialog, owned by the player's window when one is open.
#[cfg(windows)]
fn owned(dialog: rfd::FileDialog) -> rfd::FileDialog {
    match owner() {
        Some(owner) => dialog.set_parent(&owner),
        None => dialog,
    }
}

/// The folder dialog `pick_folder` opens, built apart so a test can read
/// its owner (rfd's dialog prints its parent in its `Debug`).
#[cfg(windows)]
fn folder_dialog() -> rfd::FileDialog {
    owned(rfd::FileDialog::new().set_title(DIALOG_TITLE))
}

/// The torrent dialog `pick_torrent` opens, built apart as the folder one is.
#[cfg(windows)]
fn torrent_dialog(title: &str, start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new().set_title(title).add_filter("Torrent", &["torrent"]);
    if let Some(start) = start {
        dialog = dialog.set_directory(start);
    }
    owned(dialog)
}

#[cfg(windows)]
pub fn pick_folder() -> Pick {
    match folder_dialog().pick_folder() {
        Some(path) => Pick::Folder(path),
        None => Pick::Cancelled,
    }
}

#[cfg(windows)]
pub fn pick_torrent(title: &str, start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match torrent_dialog(title, start).pick_file() {
        Some(path) => Pick::File(path),
        None => Pick::Cancelled,
    }
}

/// The firmware image dialog `pick_firmware` opens: typed to `.bin`,
/// owned like the others.
#[cfg(windows)]
fn firmware_dialog(title: &str, start: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new().set_title(title).add_filter("Firmware image", &["bin"]);
    if let Some(start) = start {
        dialog = dialog.set_directory(start);
    }
    owned(dialog)
}

#[cfg(windows)]
pub fn pick_firmware(title: &str, start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match firmware_dialog(title, start).pick_file() {
        Some(path) => Pick::File(path),
        None => Pick::Cancelled,
    }
}

#[cfg(windows)]
pub fn pick_build_folder(title: &str) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match owned(rfd::FileDialog::new().set_title(title)).pick_folder() {
        Some(path) => Pick::Folder(path),
        None => Pick::Cancelled,
    }
}

#[cfg(target_os = "linux")]
pub fn pick_folder() -> Pick {
    use ashpd::desktop::file_chooser::SelectedFiles;

    let request = async {
        SelectedFiles::open_file().title(DIALOG_TITLE).directory(true).send().await?.response()
    };
    match crate::runtime::block_on(request) {
        Ok(Ok(files)) => match files.uris().first().and_then(|uri| uri.to_file_path().ok()) {
            Some(path) => Pick::Folder(path),
            // The portal answered with nothing usable (a non-file URI);
            // treat it like a decline rather than an error.
            None => Pick::Cancelled,
        },
        // A Response error is the portal's word for "the user dismissed the
        // dialog"; anything else means no dialog could be shown at all.
        Ok(Err(ashpd::Error::Response(_))) => Pick::Cancelled,
        Ok(Err(e)) => Pick::Unavailable(e.to_string()),
        Err(e) => Pick::Unavailable(e),
    }
}

#[cfg(target_os = "linux")]
pub fn pick_torrent(title: &str, start: Option<&Path>) -> Pick {
    use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    let title = title.to_string();
    let start = start.map(Path::to_path_buf);
    let request = async move {
        let filter = FileFilter::new("Torrent").mimetype("application/x-bittorrent").glob("*.torrent");
        let mut open = SelectedFiles::open_file().title(title.as_str()).filter(filter);
        if let Some(start) = start.as_deref() {
            open = open.current_folder(start)?;
        }
        open.send().await?.response()
    };
    match crate::runtime::block_on(request) {
        Ok(Ok(files)) => match files.uris().first().and_then(|uri| uri.to_file_path().ok()) {
            Some(path) => Pick::File(path),
            None => Pick::Cancelled,
        },
        Ok(Err(ashpd::Error::Response(_))) => Pick::Cancelled,
        Ok(Err(e)) => Pick::Unavailable(e.to_string()),
        Err(e) => Pick::Unavailable(e),
    }
}

/// The firmware image dialog: the portal's file chooser filtered to
/// `.bin`, started in `start` when given.
#[cfg(target_os = "linux")]
pub fn pick_firmware(title: &str, start: Option<&Path>) -> Pick {
    use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    let title = title.to_string();
    let start = start.map(Path::to_path_buf);
    let request = async move {
        let filter = FileFilter::new("Firmware image").glob("*.bin");
        let mut open = SelectedFiles::open_file().title(title.as_str()).filter(filter);
        if let Some(start) = start.as_deref() {
            open = open.current_folder(start)?;
        }
        open.send().await?.response()
    };
    match crate::runtime::block_on(request) {
        Ok(Ok(files)) => match files.uris().first().and_then(|uri| uri.to_file_path().ok()) {
            Some(path) => Pick::File(path),
            None => Pick::Cancelled,
        },
        Ok(Err(ashpd::Error::Response(_))) => Pick::Cancelled,
        Ok(Err(e)) => Pick::Unavailable(e.to_string()),
        Err(e) => Pick::Unavailable(e),
    }
}

/// A firmware build folder, the portal's directory chooser.
#[cfg(target_os = "linux")]
pub fn pick_build_folder(title: &str) -> Pick {
    use ashpd::desktop::file_chooser::SelectedFiles;

    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    let title = title.to_string();
    let request = async move {
        SelectedFiles::open_file().title(title.as_str()).directory(true).send().await?.response()
    };
    match crate::runtime::block_on(request) {
        Ok(Ok(files)) => match files.uris().first().and_then(|uri| uri.to_file_path().ok()) {
            Some(path) => Pick::Folder(path),
            None => Pick::Cancelled,
        },
        Ok(Err(ashpd::Error::Response(_))) => Pick::Cancelled,
        Ok(Err(e)) => Pick::Unavailable(e.to_string()),
        Err(e) => Pick::Unavailable(e),
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_folder() -> Pick {
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_torrent(_title: &str, _start: Option<&Path>) -> Pick {
    let _ = torrent_dialogs_disabled();
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_firmware(_title: &str, _start: Option<&Path>) -> Pick {
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_build_folder(_title: &str) -> Pick {
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_torrent_script_is_typed_started_and_escaped() {
        let script = torrent_script(TORRENT_TITLE, Some(Path::new("/Users/me/My \"Down\"loads")));
        assert!(script.starts_with("POSIX path of (choose file of type {\"torrent\"} with prompt \""));
        assert!(script.contains("default location (POSIX file \"/Users/me/My \\\"Down\\\"loads\")"), "{script}");
        assert!(script.ends_with(')'));
        let bare = torrent_script(TORRENT_TITLE, None);
        assert!(!bare.contains("default location"), "no start, no location clause: {bare}");
        // The admin room's localized title lands in the same quoted string.
        let titled = torrent_script("Add a \"seed\"", None);
        assert!(titled.contains("with prompt \"Add a \\\"seed\\\"\""), "{titled}");
    }

    #[test]
    fn the_firmware_scripts_are_typed_to_bin_and_a_folder_is_a_folder() {
        let file = file_script("bin", "Choose a firmware image", Some(Path::new("/Users/me/core2")));
        assert!(file.starts_with("POSIX path of (choose file of type {\"bin\"} with prompt \""), "{file}");
        assert!(file.contains("default location (POSIX file \"/Users/me/core2\")"), "{file}");
        let folder = folder_script("Choose a \"build\" folder");
        assert_eq!(folder, "POSIX path of (choose folder with prompt \"Choose a \\\"build\\\" folder\")");
    }

    /// A published HWND is handed out as the Win32 window handle rfd's
    /// `set_parent` reads, and the dialogs the two pickers open carry it as
    /// their parent, until it is withdrawn. rfd keeps the parent to itself
    /// but prints it in the dialog's `Debug`, which is what is read here; that
    /// Show then centres and disables the window is the OS's, and the window
    /// checklist (docs/window-spike/checklist.md) has it checked live.
    #[cfg(windows)]
    #[test]
    fn a_published_window_owns_the_dialogs_until_it_is_withdrawn() {
        use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
        use winit::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
        // No test opens a window, so nothing else publishes one meanwhile.
        assert!(owner().is_none(), "a terminal run's dialogs are unowned");
        set_owner(NonZeroIsize::new(0x1234));
        let published = owner().expect("the published window owns the dialogs");
        let raw = published.window_handle().expect("an HWND is always at hand").as_raw();
        assert!(matches!(raw, RawWindowHandle::Win32(h) if h.hwnd.get() == 0x1234), "{raw:?}");
        // rfd reads the display handle too; on Windows there is only the one.
        let display = published.display_handle().expect("Windows has its display").as_raw();
        assert!(matches!(display, RawDisplayHandle::Windows(_)), "{display:?}");
        // The dialogs the pickers open, as rfd prints them (0x1234 is 4660).
        let dialogs = || {
            let torrent = torrent_dialog(TORRENT_TITLE, Some(Path::new("C:\\Users")));
            let firmware = firmware_dialog("Choose a firmware image", None);
            [
                ("folder", format!("{:?}", folder_dialog())),
                ("torrent", format!("{torrent:?}")),
                ("firmware", format!("{firmware:?}")),
            ]
        };
        for (picker, dialog) in dialogs() {
            assert!(dialog.contains("parent: Some(Win32("), "{picker}: unowned: {dialog}");
            assert!(dialog.contains("hwnd: 4660"), "{picker}: not the published window: {dialog}");
        }
        set_owner(None);
        assert!(owner().is_none(), "a closed window's dialogs are unowned again");
        for (picker, dialog) in dialogs() {
            assert!(dialog.contains("parent: None"), "{picker}: owned after the window: {dialog}");
        }
    }
}
