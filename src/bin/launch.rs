//! The desktop flavour's Windows launcher (packaged as "mStream Player.exe";
//! a Cargo bin name cannot carry the space).
//!
//! `mstream-player.exe` is a console program, because serve, setup, the TUI
//! and every one-shot command are; Windows gives a console program a
//! console before its first instruction runs, so a double-click on it
//! flashes one even though the player lets it go at once (desktop.rs).
//! This stub is a GUI-subsystem program, which Windows starts with no
//! console at all, and it starts the player beside it with
//! CREATE_NO_WINDOW, whose console is never shown: a double-click, a
//! shortcut or a file association opens the window and nothing else.
//!
//! It is std alone but for the one message box, finds the player in its own
//! directory (the packaged zip ships both side by side), and does not wait:
//! the player owns its lifetime, its instance lock and its exit code from
//! there. With no arguments of its own it starts the player with none, so
//! the player's empty-argv rule applies (desktop.rs: the window, under the
//! default instance lock, which a second double-click then finds held);
//! with arguments it passes them after `gui --window`, an explicit argv
//! that means the same in every flavour.

#![cfg_attr(windows, windows_subsystem = "windows")]

/// The app's identity, shared with the player by path: one file, so the
/// AppUserModelID the stub names and the one the player's window names
/// cannot differ (its tests run in this bin's test build too). The stub
/// reads only the AUMID.
#[cfg(any(windows, test))]
#[path = "../identity.rs"]
mod identity;

#[cfg(windows)]
fn main() {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    /// The child gets a console it never shows: the player still has the
    /// console its CLI expects, and no window of it appears. (winbase.h's
    /// value; std has no name for it.)
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // The taskbar identity first, before a message box can show: the
    // player's window names the same one (identity.rs has the pairing
    // rule), so a shortcut to this stub and the window it opens are one
    // taskbar button, and a pin relaunches through the stub.
    identity::set_windows_aumid();
    let player = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("mstream-player.exe")))
        .filter(|path| path.is_file());
    let Some(player) = player else {
        tell("mstream-player.exe is missing beside this program - reinstall mStream Player.");
        std::process::exit(1);
    };
    // Null stdio rather than inherited: the stub has no console handles to
    // hand down, and nobody would read the player's lines here (its debug
    // log keeps them, when one is on).
    let own: Vec<_> = std::env::args_os().skip(1).collect();
    let mut command = Command::new(&player);
    if !own.is_empty() {
        command.args(["gui", "--window"]).args(own);
    }
    let spawned = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match spawned {
        Ok(child) => allow_foreground(child.id()),
        Err(e) => {
            tell(&format!("mStream Player could not start {}: {e}", player.display()));
            std::process::exit(1);
        }
    }
}

/// Lend the player the stub's right to the foreground. Explorer gave it to
/// the stub with the click; a process the stub starts does not inherit it,
/// and a second launch that finds the player open needs it to bring the
/// holder's window forward (desktop.rs) rather than only flash its button.
/// The player is named, not any process (ASFW_ANY), and the right lapses
/// with the user's next input. Granted just after the spawn: the player
/// reaches its instance lock only after its loader, runtime and argv are
/// done, far later than this one call; were it ever earlier, the cost is
/// the flash.
#[cfg(windows)]
fn allow_foreground(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    // SAFETY: no pointers; a refusal (the stub did not have the foreground
    // to lend) is harmless.
    unsafe { AllowSetForegroundWindow(pid) };
}

/// One line in a message box: a GUI-subsystem program has nowhere else to
/// say it.
#[cfg(windows)]
fn tell(line: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, caption) = (wide(line), wide("mStream Player"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call,
    // and a null owner window is the documented "no owner".
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

/// Elsewhere the player is the app (a macOS bundle runs it directly; a
/// Linux desktop entry names it), so the stub only says so — it exists on
/// every target so that a desktop build of any of them compiles.
#[cfg(not(windows))]
fn main() {
    eprintln!("mstream-player-launch is the Windows launcher; run mstream-player instead");
    std::process::exit(1);
}
