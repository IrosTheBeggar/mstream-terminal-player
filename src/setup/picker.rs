//! The native folder picker, one backend per platform.
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

use std::path::PathBuf;

/// What came back from asking for a folder.
pub enum Pick {
    Folder(PathBuf),
    /// The dialog opened and the user declined it.
    Cancelled,
    /// No dialog could open here (headless, no portal, unsupported OS) —
    /// the wizard offers the server-side browser instead. Only the Linux
    /// and fallback backends construct it; every platform matches it.
    #[allow(dead_code)]
    Unavailable(String),
}

pub const DIALOG_TITLE: &str = "Add a music folder to mStream";

#[cfg(target_os = "macos")]
pub fn pick_folder() -> Pick {
    // Deliberately NOT rfd here: an NSOpenPanel opened by a terminal-launched
    // process stays behind every window without key focus — live testing
    // went through activation policies and activateIgnoringOtherApps and the
    // panel never fronted. osascript runs the chooser in its own app
    // context, which fronts the way every mac shell script relies on. The
    // wizard blocks on the child exactly as it would on a modal panel.
    let script = format!("POSIX path of (choose folder with prompt \"{DIALOG_TITLE}\")");
    match std::process::Command::new("/usr/bin/osascript").arg("-e").arg(&script).output() {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if path.is_empty() {
                Pick::Cancelled
            } else {
                Pick::Folder(PathBuf::from(path))
            }
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            // -128 is AppleScript's "User canceled".
            if err.contains("-128") {
                Pick::Cancelled
            } else {
                Pick::Unavailable(err.trim().to_string())
            }
        }
        Err(e) => Pick::Unavailable(e.to_string()),
    }
}

#[cfg(windows)]
pub fn pick_folder() -> Pick {
    match rfd::FileDialog::new().set_title(DIALOG_TITLE).pick_folder() {
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

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
pub fn pick_folder() -> Pick {
    Pick::Unavailable("no native picker on this platform".to_string())
}
