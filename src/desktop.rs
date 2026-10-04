//! The desktop flavour's launch contract (`--features desktop`): what a
//! bare `mstream-player` means when it is the app a person double-clicked.
//!
//! One rule keeps the two flavours one CLI: an explicit argv means the same
//! thing in both (`gui` is the terminal GUI, `gui --window` the window), and
//! only an EMPTY argv differs. The terminal flavour opens the TUI as it
//! always has; the desktop flavour opens the GUI in its own window, under a
//! default instance lock so a second double-click does not open a second
//! player — unless no window can be expected (no display on a Linux box, a
//! session over SSH at a terminal), where it is the TUI as before.
//!
//! Nothing here exists in the terminal flavour: main.rs reaches it only
//! under `cfg(feature = "desktop")`.

use std::ffi::OsString;
use std::path::PathBuf;

/// The lock an empty-argv desktop launch takes, in the player's config
/// directory (`config::config_dir`, so `MSTREAM_PLAYER_CONFIG_DIR` moves it
/// too), with the launcher's JSON sidecar beside it (`desktop-player.json`).
///
/// Whether this should be the same file the mStream tray launcher passes
/// with `--instance-lock` — one player per machine whichever way it was
/// opened, rather than one per way in — is an open decision: the tray's
/// lock lives in the launcher's data home, which the player cannot find on
/// its own today.
pub const DEFAULT_LOCK: &str = "desktop-player.lock";

/// The argv clap is handed. macOS LaunchServices may start a `.app`'s
/// executable with a `-psn_0_NNN` process serial number (notably on a
/// quarantined first launch from Finder); that is the double-click, not a
/// request, so an argv of nothing else counts as empty. Any other argv is
/// returned untouched, so clap's errors stay v0.9.0's — `-psn_1 gui` among
/// them, which is no launch LaunchServices makes.
pub fn launch_argv(mut args: Vec<OsString>) -> Vec<OsString> {
    let serial = |arg: &OsString| arg.as_encoded_bytes().starts_with(b"-psn_");
    if args.len() > 1 && args[1..].iter().all(serial) {
        args.truncate(1);
    }
    args
}

/// Whether an empty-argv launch can expect a window, from the environment
/// (`get`, passed in so the rule is a pure function): no on a free-desktop
/// Unix (`display_env`: Linux and the BSDs, where winit reads the session
/// from the environment) with neither an X11 nor a Wayland display named,
/// and no on any OS when the process runs under SSH with a terminal on
/// stdin — a person typing `mstream-player` into a remote shell wants the
/// player in that shell, not a window on a desktop they cannot see (or a
/// forwarded X11 window crawling over the link). Under SSH with no terminal
/// (a remote command, a script) the window is still tried: nobody is at a
/// terminal to get the TUI either. Empty values count as unset, as winit
/// reads them; `WAYLAND_SOCKET` is the Wayland session winit also accepts.
pub fn window_expected(
    get: impl Fn(&str) -> Option<OsString>,
    display_env: bool,
    stdin_tty: bool,
) -> bool {
    let set = |name: &str| get(name).is_some_and(|value| !value.is_empty());
    if display_env && !(set("DISPLAY") || set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET")) {
        return false;
    }
    !(stdin_tty && (set("SSH_CONNECTION") || set("SSH_TTY")))
}

/// [`window_expected`] for this process.
pub fn window_expected_here() -> bool {
    use std::io::IsTerminal;
    let display_env = cfg!(all(unix, not(any(target_os = "macos", target_os = "ios"))));
    window_expected(|name| std::env::var_os(name), display_env, std::io::stdin().is_terminal())
}

/// Where [`DEFAULT_LOCK`] goes for this process; `None` when no config
/// directory can be resolved, and the launch is then an unlocked one, as
/// every launch was before.
pub fn default_lock() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|dir| dir.join(DEFAULT_LOCK))
}

/// A refused second launch's line, for the debug log as well as stdout: a
/// double-clicked app's stdout goes nowhere anyone reads. The log is set up
/// as a session's would be (`MSTREAM_LOG`, or the config's `[log]`), which
/// is safe beside the holder's: the default location names each run's file
/// by its pid, and the sweep spares a file in use. `focused` is what
/// [`focus_window_holder`] came to, when it was tried.
pub fn log_refusal(line: &str, focused: Option<bool>) {
    crate::logging::init(crate::logging::Run::Session);
    tracing::info!("{line}");
    if let Some(focused) = focused {
        tracing::info!("brought the holder's window forward: {focused}");
    }
}

/// Bring the window of the player holding the instance lock to the front,
/// for a refused second launch whose holder's sidecar says it draws in a
/// window of its own: the double-click that found the player open shows
/// it, rather than only a line nobody sees. Best effort, and quick on every
/// path: true when the holder's window was asked to the front and the OS
/// took the request; false when this OS has no way to, or the pid has no
/// window to bring (the caller prints its line either way, for whoever has
/// a terminal).
///
/// The pid is the sidecar's, read behind a held lock, so it names a live
/// player in practice; each OS still looks it up before acting (a process
/// that is gone has no application and no windows), so a pid that went
/// stale in between costs nothing and never waits.
///
/// - macOS: NSRunningApplication for the pid, activated with all its
///   windows by the cooperative hand-over macOS 14 requires (see
///   `focus`). The pid lookup answers nil for a process that is gone.
/// - Windows: the pid's visible, unowned top-level window (EnumWindows),
///   restored if minimised and made the foreground window. The foreground
///   is the user's to give: a process the user just started (by Explorer,
///   or by the launcher stub, which passes its right on) may take it; when
///   Windows refuses, the window's taskbar button flashes instead and the
///   answer is false. Nothing here waits on the holder's thread (the
///   restore is ShowWindowAsync), and a holder Windows counts as hung is
///   left alone.
/// - Linux and the BSDs: false. X11 could be asked through a
///   _NET_ACTIVE_WINDOW client message, but x11rb is not a direct
///   dependency (only arboard's, for the clipboard) and a window manager
///   may refuse a request from another client anyway; Wayland has no way
///   for one app to raise another's window without an activation token the
///   launching shell would have to hand over.
pub fn focus_window_holder(pid: u32) -> bool {
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    focus(pid)
}

#[cfg(target_os = "macos")]
fn focus(pid: u32) -> bool {
    use objc2::runtime::NSObjectProtocol;
    use objc2::{MainThreadMarker, sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication};

    let Ok(pid) = i32::try_from(pid) else { return false };
    let Some(holder) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return false;
    };
    if holder.isTerminated() {
        return false;
    }
    let Some(main) = MainThreadMarker::new() else { return false };
    let this = NSRunningApplication::currentApplication();
    if this.respondsToSelector(sel!(activateFromApplication:options:)) {
        // macOS 14 and later, where activation is cooperative: an app may
        // only be activated by one that yields to it, and asking from
        // outside (`activateWithOptions:`, measured: it answers YES and
        // nothing moves) is refused when another app is in front. This
        // process becomes an application first (sharedApplication, with
        // no Dock tile: an unbundled binary's policy stays Prohibited),
        // which is what gives the user's fresh launch its standing to
        // pass the front on, then yields it to the holder and asks for the
        // holder from itself. Measured on macOS 26 from behind Finder:
        // the holder came to the front; without sharedApplication it did
        // not.
        let app = NSApplication::sharedApplication(main);
        app.yieldActivationToApplication(&holder);
        let options = NSApplicationActivationOptions::ActivateAllWindows;
        holder.activateFromApplication_options(&this, options)
    } else {
        // Before macOS 14, a plain request that ignores the frontmost app
        // is honoured (the flag is deprecated, and inert, from 14 on).
        #[allow(deprecated)]
        let options = NSApplicationActivationOptions::ActivateAllWindows
            | NSApplicationActivationOptions::ActivateIgnoringOtherApps;
        holder.activateWithOptions(options)
    }
}

#[cfg(windows)]
fn focus(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx, GW_OWNER, GetWindow,
        GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible, SW_RESTORE,
        SetForegroundWindow, ShowWindowAsync,
    };
    use windows_sys::core::BOOL;

    struct Search {
        pid: u32,
        found: HWND,
    }
    /// One top-level window: stop at the first visible, unowned one of the
    /// pid (winit's hidden helper windows are not visible; a dialog is
    /// owned).
    unsafe extern "system" fn visit(hwnd: HWND, search: LPARAM) -> BOOL {
        // SAFETY: `search` is the `&mut Search` EnumWindows was handed,
        // alive for the whole enumeration; the calls take any HWND.
        unsafe {
            let search = &mut *(search as *mut Search);
            let mut owner = 0u32;
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == search.pid
                && IsWindowVisible(hwnd) != 0
                && GetWindow(hwnd, GW_OWNER).is_null()
            {
                search.found = hwnd;
                return 0;
            }
        }
        1
    }

    let mut search = Search { pid, found: std::ptr::null_mut() };
    // SAFETY: the callback reads only the Search behind the pointer, which
    // outlives the call; EnumWindows returns once every window is visited
    // or the callback stops it.
    unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
    let hwnd = search.found;
    if hwnd.is_null() {
        return false;
    }
    // The window belongs to another process, and this refusal must never wait on it: a
    // double-click's launch is an invisible CREATE_NO_WINDOW child of the stub, so a stuck one
    // would pile up unseen with each further click. Hence ShowWindowAsync, not ShowWindow (which
    // waits for the holder's thread to process the show; SetForegroundWindow only posts its
    // activation). A holder Windows already counts as hung (its thread has not taken messages for
    // 5 s) gets nothing at all, so FlashWindowEx, not documented either way, never meets one
    // either; the printed line is the honest answer there.
    // SAFETY: a window handle EnumWindows gave; a window that closed since
    // makes each call fail, which is harmless.
    unsafe {
        if IsHungAppWindow(hwnd) != 0 {
            return false;
        }
        if IsIconic(hwnd) != 0 {
            ShowWindowAsync(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) != 0 {
            return true;
        }
        let flash = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            // The taskbar button only: the caption flash can send the holder's
            // thread a synchronous activation message, and this invisible
            // process must never wait on a holder that has just stopped pumping.
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        FlashWindowEx(&flash);
    }
    false
}

#[cfg(not(any(target_os = "macos", windows)))]
fn focus(_pid: u32) -> bool {
    false
}

/// The console Explorer made for a double-clicked console executable, let
/// go before the window opens, so no console stands beside the player for
/// its lifetime (it still flashes as the process starts; the launcher stub,
/// src/bin/launch.rs, has none at all). Only a console this process alone
/// is attached to: launched from cmd or PowerShell the list holds the shell
/// too, and serve, setup and the TUI typed there keep their console. With
/// no console (CREATE_NO_WINDOW, a detached start) the count is 0 and
/// nothing happens.
///
/// What prints afterwards prints nowhere: the std handles still name the
/// freed console, and std on Windows treats a write to an invalid handle
/// as written, so `println!` neither fails nor panics.
#[cfg(windows)]
pub fn leave_own_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    // Room for two: the count is all that is read, and the call returns the
    // whole count whatever the buffer holds.
    let mut pids = [0u32; 2];
    // SAFETY: the buffer is valid for writes of the length passed.
    let attached = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if attached == 1 {
        // SAFETY: no arguments; detaching from a console this process alone
        // holds closes it, which is the point.
        unsafe { FreeConsole() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_process_serial_number_alone_is_an_empty_argv() {
        let bin = "/Applications/mStream Player.app/Contents/MacOS/mstream-player";
        assert_eq!(launch_argv(argv(&[bin, "-psn_0_1234567"])), argv(&[bin]));
        assert_eq!(launch_argv(argv(&[bin, "-psn_0_1", "-psn_0_2"])), argv(&[bin]));
        // Empty stays empty, and so does no argv at all.
        assert_eq!(launch_argv(argv(&[bin])), argv(&[bin]));
        assert_eq!(launch_argv(Vec::new()), Vec::<OsString>::new());
        // Anything else is clap's, unchanged — errors included.
        for kept in [
            &[bin, "gui"][..],
            &[bin, "-psn_0_1", "gui"],
            &[bin, "gui", "-psn_0_1"],
            &[bin, "--psn_0_1"],
            &[bin, "-psn"],
            &[bin, ""],
        ] {
            assert_eq!(launch_argv(argv(kept)), argv(kept), "{kept:?}");
        }
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let owned: Vec<(String, OsString)> =
            pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| owned.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    #[test]
    fn a_window_is_expected_where_a_display_is_and_nobody_is_at_a_remote_shell() {
        // A desktop session, local or not on a free desktop.
        assert!(window_expected(env(&[("DISPLAY", ":0")]), true, true));
        assert!(window_expected(env(&[("WAYLAND_DISPLAY", "wayland-0")]), true, false));
        assert!(window_expected(env(&[("WAYLAND_SOCKET", "3")]), true, false));
        // No display named, or one named empty: the TUI, as before.
        assert!(!window_expected(env(&[]), true, true));
        assert!(!window_expected(env(&[]), true, false));
        assert!(!window_expected(env(&[("DISPLAY", ""), ("WAYLAND_DISPLAY", "")]), true, false));
        // macOS and Windows name no display in the environment.
        assert!(window_expected(env(&[]), false, true));
        assert!(window_expected(env(&[]), false, false));
        // Over SSH at a terminal: the shell's player, whatever the display
        // (an X11-forwarded session names one).
        for ssh in [("SSH_CONNECTION", "10.0.0.2 52000 10.0.0.1 22"), ("SSH_TTY", "/dev/pts/3")] {
            assert!(!window_expected(env(&[ssh]), false, true), "{ssh:?}");
            assert!(!window_expected(env(&[ssh, ("DISPLAY", "localhost:10.0")]), true, true));
            // With no terminal on stdin nobody would see a TUI either.
            assert!(window_expected(env(&[ssh]), false, false), "{ssh:?}");
        }
        assert!(window_expected(env(&[("SSH_TTY", "")]), false, true), "empty is unset");
    }

    #[test]
    fn focusing_a_holder_that_is_not_there_answers_quickly_and_does_nothing() {
        // This process (never its own holder), the idle pid, and a pid no
        // process has: false at once, whatever the OS.
        let started = std::time::Instant::now();
        assert!(!focus_window_holder(std::process::id()));
        assert!(!focus_window_holder(0));
        let gone = {
            let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
                .args(if cfg!(windows) { &["/c", "exit"][..] } else { &[][..] })
                .spawn()
                .expect("a short-lived child");
            let pid = child.id();
            child.wait().expect("it exits");
            pid
        };
        assert!(!focus_window_holder(gone), "a pid that has exited has no window to bring");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }

    #[test]
    fn the_default_lock_is_in_the_players_config_directory() {
        // The directory itself is config_dir's (other tests move it with
        // MSTREAM_PLAYER_CONFIG_DIR while this one runs); the names are ours.
        let path = default_lock().expect("a config directory resolves here");
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some(DEFAULT_LOCK));
        assert_eq!(
            crate::instance::sidecar_path(&path).file_name().and_then(|n| n.to_str()),
            Some("desktop-player.json"),
            "the launcher's sidecar name beside it"
        );
    }
}
