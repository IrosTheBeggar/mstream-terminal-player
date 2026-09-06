//! The native folder picker, one backend per platform — and, through
//! [`pick_torrent`], the GUI's `.torrent` file picker on the same three
//! backends (docs/ux-contracts/add-torrent.md, clause 2).
//!
//! Split on purpose (the mStream-side picker spike, 2026-08-21): rfd's Linux
//! backends either link libwayland-client into NEEDED (portal flavor — a
//! load failure on headless boxes, where this binary is the server-audio
//! engine) or link GTK (a Docker-image problem in five cross triples). So
//! Linux speaks to the XDG desktop portal directly through ashpd — pure Rust
//! over D-Bus, nothing linked — while macOS and Windows use rfd's native
//! NSOpenPanel / IFileOpenDialog, which link only system frameworks.
//!
//! The dialog is modal from the wizard's point of view: the call blocks the
//! event loop until the user answers. That is the behavior a picker should
//! have, and on a headless box the Linux portal call fails in about a
//! millisecond (no session bus), which is what routes the wizard to its
//! server-side browser instead.

use std::path::{Path, PathBuf};

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
pub const TORRENT_TITLE: &str = "Choose a .torrent file for mStream";

/// `MSTREAM_NO_PICKER`: refuse to open the torrent dialog at all — the
/// harness seam that lets a pty smoke reach the typed fallback instead
/// of popping a window on whoever is running it.
fn torrent_dialogs_disabled() -> bool {
    std::env::var("MSTREAM_NO_PICKER").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The AppleScript for the torrent dialog: typed to the extension, and
/// opening in `start` when there is one. A path lands inside a quoted
/// string, so its quotes and backslashes are escaped.
#[cfg(any(target_os = "macos", test))]
fn torrent_script(start: Option<&Path>) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let location = start
        .map(|p| format!(" default location (POSIX file \"{}\")", escape(&p.to_string_lossy())))
        .unwrap_or_default();
    format!(
        "POSIX path of (choose file of type {{\"torrent\"}} with prompt \"{}\"{location})",
        escape(TORRENT_TITLE)
    )
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

/// The `.torrent` dialog (the GUI's Add-torrent room): `choose file`
/// typed to the extension, started in `start` — Downloads, where a
/// torrent lands from the browser.
#[cfg(target_os = "macos")]
pub fn pick_torrent(start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    match osascript_path(&torrent_script(start)) {
        Ok(Some(path)) => Pick::File(path),
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

#[cfg(windows)]
pub fn pick_folder() -> Pick {
    match rfd::FileDialog::new().set_title(DIALOG_TITLE).pick_folder() {
        Some(path) => Pick::Folder(path),
        None => Pick::Cancelled,
    }
}

#[cfg(windows)]
pub fn pick_torrent(start: Option<&Path>) -> Pick {
    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    let mut dialog = rfd::FileDialog::new().set_title(TORRENT_TITLE).add_filter("Torrent", &["torrent"]);
    if let Some(start) = start {
        dialog = dialog.set_directory(start);
    }
    match dialog.pick_file() {
        Some(path) => Pick::File(path),
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
pub fn pick_torrent(start: Option<&Path>) -> Pick {
    use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

    if torrent_dialogs_disabled() {
        return Pick::Unavailable("dialogs are switched off".to_string());
    }
    let start = start.map(Path::to_path_buf);
    let request = async move {
        let filter = FileFilter::new("Torrent").mimetype("application/x-bittorrent").glob("*.torrent");
        let mut open = SelectedFiles::open_file().title(TORRENT_TITLE).filter(filter);
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

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_folder() -> Pick {
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_torrent(_start: Option<&Path>) -> Pick {
    let _ = torrent_dialogs_disabled();
    Pick::Unavailable("no native picker on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_torrent_script_is_typed_started_and_escaped() {
        let script = torrent_script(Some(Path::new("/Users/me/My \"Down\"loads")));
        assert!(script.starts_with("POSIX path of (choose file of type {\"torrent\"} with prompt \""));
        assert!(script.contains("default location (POSIX file \"/Users/me/My \\\"Down\\\"loads\")"), "{script}");
        assert!(script.ends_with(')'));
        let bare = torrent_script(None);
        assert!(!bare.contains("default location"), "no start, no location clause: {bare}");
    }
}
