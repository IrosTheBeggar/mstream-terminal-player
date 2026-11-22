//! A desktop window's picture put back in place on Windows after the
//! graphics stack restarts: what the window hears of a restart, and the
//! nudge that answers it. Both windows of ours are watched: the player's
//! (`gui --window`, `setup --window` and the desktop flavour's double-click)
//! and the visualizer's (`viz-window`).
//!
//! A restart (a driver reset, Win+Ctrl+Shift+B, or the PC leaving connected
//! standby) leaves a window that presents through a DXGI flip-model swap
//! chain, wgpu's DX12 surface, showing its whole picture offset by the
//! window's frame: 13 px left and 58 px up at 200%, so the top rows are cut
//! off and a black or white band runs along the bottom and the right. It
//! stays so until the window is moved, resized, minimised and restored, or
//! hidden and shown. Nothing in the app's layout moves, no winit event comes
//! at the restart, and a GDI window (Notepad) keeps its picture. A new
//! frame, the surface reconfigured at the same size, SWP_FRAMECHANGED,
//! RedrawWindow and wgpu's DirectComposition path all left it shifted, and
//! so did a window opened visible rather than hidden until its first frame
//! (Windows 10, 2026-11-22: main at e325ba2a and PR 94 alike).
//!
//! What does reach the window is WM_DISPLAYCHANGE, sent to every top-level
//! window about 0.8 s after a restart; on a resume from standby, the console
//! display's power setting (GUID_CONSOLE_DISPLAY_STATE) going from off to
//! on, which comes only to a window registered for it; and on a resume from
//! S3 sleep, PBT_APMRESUMEAUTOMATIC, which comes to every top-level window.
//! On any of them the watch sends for two nudges, one 250 ms later and one
//! at 1.5 s for a resume slower than a restart. A nudge moves the window one
//! pixel right and straight back (SWP_NOSIZE, SWP_NOZORDER, SWP_NOACTIVATE),
//! which puts the picture back: the window ends where it stood, maximised
//! or not, and takes no focus. A minimised or hidden window is left alone,
//! since restoring or showing it puts the picture back too. Nudged from an
//! outside process on WM_DISPLAYCHANGE, at once or up to a second later,
//! the window was put right three times out of three.
//!
//! A nudge keeps the window on the monitor it is on. A window across two
//! monitors takes the scale of the one that holds more of it, so a pixel
//! can hand it to the other when the two shares are within a pixel or two
//! of each other; if their scales differ, winit answers the WM_DPICHANGED
//! that brings with a new size, and may shift the window onto that
//! monitor, and the move back, which keeps the size it finds, can rescale
//! it again: the window would end neither where it stood nor at its size.
//! So a nudge steps right only when MonitorFromRect puts the window, a
//! pixel right, on the monitor MonitorFromWindow names for it now; else it
//! steps left, down or up, the first way that keeps it there ([`step`]),
//! and not at all should none. A step any of those ways and straight back
//! put a shifted picture right on main as a step right does.
//!
//! The nudges are posted back to the window, as a message of the watch's
//! own, by a thread that sleeps out their delays. Not by SetTimer: Windows
//! makes a WM_TIMER only once no WM_PAINT is pending, and a window that
//! draws every frame (the visualizer's asks for its next redraw as it
//! draws) always has one, so its timers never came and its picture stayed
//! shifted. A posted message is taken ahead of WM_PAINT.
//!
//! The watch hears the window's messages through a procedure put in front
//! of winit's: the classic GWLP_WNDPROC chain, which is enough for one
//! window a process, all either host has. SetWindowSubclass would serve as
//! well (the System32 copy of comctl32, 5.82, which a binary with no
//! manifest gets, exports it as 6.0 does), but it would add comctl32.dll
//! to the binary's imports, which name none now. Which messages call for a
//! nudge, and which way it steps, are pure functions ([`heard`],
//! [`Display::hear`], [`due`], [`step`]) tested on every OS; the Win32 half
//! is thin around them, and its procedure never lets a panic unwind into
//! user32.

use std::time::Duration;

/// The window messages and values the watch reads (winuser.h), by number,
/// so the rule builds and is tested on every OS; the Windows half checks
/// each against windows-sys's.
const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_POWERBROADCAST: u32 = 0x0218;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;
/// GUID_CONSOLE_DISPLAY_STATE, {6FE69556-704A-47A0-8F24-C28D936FDA47}: the
/// display the console session shows, as a DWORD that is 0 off, 1 on and 2
/// dimmed. Written out rather than taken from windows-sys, whose copy is in
/// SystemServices, a module of thousands of bindings this one GUID would
/// bring in.
const CONSOLE_DISPLAY_STATE: u128 = 0x6fe6_9556_704a_47a0_8f24_c28d_936f_da47;
const DISPLAY_OFF: u32 = 0;
const DISPLAY_ON: u32 = 1;

/// What one message to the window tells the watch.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Heard {
    /// WM_DISPLAYCHANGE: a display's mode changed, or the graphics stack
    /// restarted.
    DisplayChanged,
    /// PBT_APMRESUMEAUTOMATIC: the machine is back from sleep or
    /// hibernation.
    Resumed,
    /// The console display's state, as its power setting reports it.
    ConsoleDisplay(u32),
}

/// What a message tells the watch, if anything. `setting` reads a power
/// setting change from the message's lParam: its GUID, as the number
/// `GUID::from_u128` takes, and its first DWORD of data. It is asked only
/// for PBT_POWERSETTINGCHANGE, the one message whose lParam points at one.
fn heard(msg: u32, wparam: usize, setting: impl FnOnce() -> Option<(u128, u32)>) -> Option<Heard> {
    match (msg, wparam) {
        (WM_DISPLAYCHANGE, _) => Some(Heard::DisplayChanged),
        (WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC) => Some(Heard::Resumed),
        (WM_POWERBROADCAST, PBT_POWERSETTINGCHANGE) => match setting()? {
            (CONSOLE_DISPLAY_STATE, state) => Some(Heard::ConsoleDisplay(state)),
            _ => None,
        },
        _ => None,
    }
}

/// What the watch remembers between messages.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Display {
    /// The console display was reported off and has not come on since.
    off: bool,
}

impl Display {
    /// Whether `heard` calls for a nudge. A display change and a resume
    /// always do; the console display only coming on after it was off.
    /// Registering for its setting reports the state it is in at once,
    /// which is no change, and a dimmed display neither went off nor comes
    /// back: an idle screen dims before it goes off, and a dimmed one
    /// brightened is the same picture.
    fn hear(&mut self, heard: Heard) -> bool {
        match heard {
            Heard::DisplayChanged | Heard::Resumed => true,
            Heard::ConsoleDisplay(DISPLAY_OFF) => {
                self.off = true;
                false
            }
            Heard::ConsoleDisplay(DISPLAY_ON) => std::mem::take(&mut self.off),
            Heard::ConsoleDisplay(_) => false,
        }
    }
}

/// When the nudges a signal sends for come, after the signal: one soon
/// (WM_DISPLAYCHANGE comes once a restart is done, and a nudge at once or a
/// second later both put the picture right), and one later, for a resume
/// that takes longer to settle.
const NUDGES: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(1500)];

/// Whether a nudge sent for signal number `sent` is due when it arrives,
/// the latest signal being number `latest`: only the latest signal's are,
/// so a burst of signals is one pair of nudges after its last, and none is
/// once the watch has ended (which counts as a signal of its own).
fn due(sent: usize, latest: usize) -> bool {
    sent == latest
}

/// Whether a nudge that is due moves the window: not when it is hidden (on
/// Windows the player's window is made hidden, and shown once a frame is
/// presented to it) or minimised, since showing or restoring it puts its
/// picture back by itself.
fn moves(visible: bool, minimised: bool) -> bool {
    visible && !minimised
}

/// The ways a nudge may step, as (x, y) in pixels, in the order tried:
/// right, the step the fix was tested with, then left, down and up.
const STEPS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The way a nudge steps: the first of [`STEPS`] that `stays` says leaves
/// the window on the monitor it is on, so its scale stays too; none when
/// every step would take it to another.
fn step(stays: impl Fn((i32, i32)) -> bool) -> Option<(i32, i32)> {
    STEPS.into_iter().find(|&way| stays(way))
}

#[cfg(windows)]
pub use win32::watch;
// The player's window ends its watch at its teardown; the visualizer's
// (the terminal flavour's one window) leaves that to its destruction.
#[cfg(all(windows, feature = "window"))]
pub use win32::unwatch;

#[cfg(windows)]
mod win32 {
    use std::mem::offset_of;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering::Relaxed};
    use std::time::Instant;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        MONITOR_DEFAULTTONEAREST, MonitorFromRect, MonitorFromWindow,
    };
    use windows_sys::Win32::System::Power::{
        POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
        UnregisterPowerSettingNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        self as wm, CallWindowProcW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, GWLP_WNDPROC,
        GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible, PostMessageW,
        RegisterWindowMessageW, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW,
        SetWindowPos, WM_NCDESTROY, WNDPROC,
    };
    use windows_sys::core::{GUID, w};
    use winit::window::Window;

    use super::{CONSOLE_DISPLAY_STATE, Display, NUDGES, due, heard, moves, step};

    // The numbers the rule is written with are the platform's.
    const _: () = assert!(
        super::WM_DISPLAYCHANGE == wm::WM_DISPLAYCHANGE
            && super::WM_POWERBROADCAST == wm::WM_POWERBROADCAST
            && super::PBT_APMRESUMEAUTOMATIC == wm::PBT_APMRESUMEAUTOMATIC as usize
            && super::PBT_POWERSETTINGCHANGE == wm::PBT_POWERSETTINGCHANGE as usize
    );

    // The watch's state. Each is read and written on the window's thread
    // alone (the procedure runs there, and so do `watch` and `unwatch`), so
    // the atomics are for a static's sake, not for ordering.
    /// The watched window, as a number; 0 while none is.
    static WATCHED: AtomicIsize = AtomicIsize::new(0);
    /// The procedure the watch's passes every message on to: the window's
    /// own (winit's) as the watch found it. Kept past the watch's end when
    /// another procedure was put in front of the watch's meanwhile, since
    /// that one still passes messages on through the watch's; no window is
    /// watched again while it is kept.
    static PREV: AtomicIsize = AtomicIsize::new(0);
    /// The console display setting's registration; 0 when none.
    static NOTIFY: AtomicIsize = AtomicIsize::new(0);
    /// [`Display`] between messages.
    static OFF: AtomicBool = AtomicBool::new(false);
    /// The watch's own message, which a nudge comes back to the window as
    /// (RegisterWindowMessageW, so no other procedure answers to it); 0
    /// until the first watch names it.
    static NUDGE: AtomicU32 = AtomicU32::new(0);
    /// The latest signal's number, which a nudge carries in its wParam
    /// ([`due`]); the watch's end counts one too.
    static LATEST: AtomicUsize = AtomicUsize::new(0);

    /// Watch `window` from now until [`unwatch`] or its destruction: on the
    /// loop's thread, as the window is made (winit gives out the handle on
    /// that thread alone). A second window in the process is not watched,
    /// nor one winit names no HWND for.
    pub fn watch(window: &Window) {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return };
        if let RawWindowHandle::Win32(handle) = handle.as_raw() {
            watch_hwnd(handle.hwnd.get() as HWND);
        }
    }

    fn watch_hwnd(hwnd: HWND) {
        if PREV.load(Relaxed) != 0 {
            return;
        }
        if NUDGE.load(Relaxed) == 0 {
            // SAFETY: a static NUL-terminated name. 0 is a refusal (the
            // atom table full), and a watch with no message to nudge by is
            // not begun.
            let message = unsafe { RegisterWindowMessageW(w!("mstream-player display watch")) };
            if message == 0 {
                return;
            }
            NUDGE.store(message, Relaxed);
        }
        // SAFETY: a live window of this thread's. The procedure put in front
        // is ours, which passes every message on to the one returned, and no
        // message reaches the window between the swap and PREV's store: none
        // is dispatched on this thread meanwhile.
        let prev = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, ours()) };
        if prev == 0 {
            return;
        }
        PREV.store(prev, Relaxed);
        OFF.store(false, Relaxed);
        WATCHED.store(hwnd as isize, Relaxed);
        let setting = GUID::from_u128(CONSOLE_DISPLAY_STATE);
        // SAFETY: the window's handle as the recipient, which
        // DEVICE_NOTIFY_WINDOW_HANDLE says it is; the GUID outlives the call.
        // A registration refused (0) leaves the other signals.
        let notify = unsafe {
            RegisterPowerSettingNotification(hwnd, &setting, DEVICE_NOTIFY_WINDOW_HANDLE)
        };
        NOTIFY.store(notify, Relaxed);
    }

    /// End the watch: the nudges on their way no longer due, the
    /// registration withdrawn, and the window's own procedure put back if
    /// the watch's is still in front of it. The window host calls it at its
    /// teardown, before the window is destroyed; the window's destruction
    /// ends it the same way (WM_NCDESTROY), which is all the visualizer's
    /// window has.
    pub fn unwatch() {
        let hwnd = WATCHED.swap(0, Relaxed) as HWND;
        if hwnd.is_null() {
            return;
        }
        LATEST.fetch_add(1, Relaxed);
        // SAFETY: the watched window, on its thread, and not destroyed past
        // WM_NCDESTROY (which ends the watch); a registration is withdrawn
        // once.
        unsafe {
            let notify = NOTIFY.swap(0, Relaxed);
            if notify != 0 {
                UnregisterPowerSettingNotification(notify);
            }
            if GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == ours() {
                SetWindowLongPtrW(hwnd, GWLP_WNDPROC, PREV.swap(0, Relaxed));
            }
        }
    }

    /// The watch's procedure, as the number GWLP_WNDPROC holds.
    fn ours() -> isize {
        let ours: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = procedure;
        ours as usize as isize
    }

    /// The procedure in front of the window's: what the watch reads, and
    /// every message passed on to the window's own procedure but the
    /// watch's own nudges. A panic in the watch's part is caught there, and
    /// the message goes on as if the watch were not there.
    unsafe extern "system" fn procedure(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // Read first: the watch's end on WM_NCDESTROY clears it.
        let prev = PREV.load(Relaxed);
        if hwnd as isize == WATCHED.load(Relaxed) {
            // SAFETY: the message as the window's procedure was handed it.
            let answer = AssertUnwindSafe(|| unsafe { hear(hwnd, msg, wparam, lparam) });
            if catch_unwind(answer).unwrap_or(false) {
                return 0;
            }
            if msg == WM_NCDESTROY {
                unwatch();
            }
        }
        // SAFETY: `prev` is 0 or the procedure GWLP_WNDPROC held, which
        // CallWindowProcW calls as Windows would have; WNDPROC is an Option
        // of a function pointer, the size of an isize with 0 its None.
        unsafe {
            match std::mem::transmute::<isize, WNDPROC>(prev) {
                Some(prev) => CallWindowProcW(Some(prev), hwnd, msg, wparam, lparam),
                None => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    /// The watch's part of a message to the watched window: true for one of
    /// its own nudges, which it answers and the window's procedure never
    /// sees.
    ///
    /// SAFETY: the caller hands the message as Windows handed it, on the
    /// window's thread.
    unsafe fn hear(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
        let message = NUDGE.load(Relaxed);
        if message != 0 && msg == message {
            // SAFETY: the watched window, on its thread.
            unsafe {
                if due(wparam, LATEST.load(Relaxed))
                    && moves(IsWindowVisible(hwnd) != 0, IsIconic(hwnd) != 0)
                {
                    nudge(hwnd);
                }
            }
            return true;
        }
        // SAFETY: asked only for PBT_POWERSETTINGCHANGE, whose lParam is
        // null or a POWERBROADCAST_SETTING.
        let Some(heard) = heard(msg, wparam, || unsafe { setting(lparam) }) else { return false };
        let mut display = Display { off: OFF.load(Relaxed) };
        let nudges = display.hear(heard);
        OFF.store(display.off, Relaxed);
        if nudges {
            send_for_nudges(hwnd, message);
        }
        tracing::debug!(
            "display watch: {heard:?}{}",
            if nudges { ", the window is nudged in 250 ms and 1.5 s" } else { "" }
        );
        false
    }

    /// The nudges for a new signal, posted back to the window as `message`
    /// carrying the signal's number, by a thread that sleeps out their
    /// delays ([`NUDGES`]) and ends; or one at once, should no thread start.
    /// Posted rather than timed: see the module's notes.
    fn send_for_nudges(hwnd: HWND, message: u32) {
        let signal = LATEST.fetch_add(1, Relaxed).wrapping_add(1);
        let window = hwnd as isize;
        // SAFETY: PostMessageW takes any window handle: one destroyed since
        // makes it fail, and a nudge that comes once the watch has ended is
        // not due, and then not the watch's to answer at all.
        let post = move || unsafe { PostMessageW(window as HWND, message, signal, 0) };
        let from = Instant::now();
        let sender = std::thread::Builder::new().name("display-watch".into()).spawn(move || {
            for after in NUDGES {
                std::thread::sleep(after.saturating_sub(from.elapsed()));
                post();
            }
        });
        if sender.is_err() {
            post();
        }
    }

    /// One pixel and straight back: right, or the first other way that
    /// keeps the window on its monitor ([`step`]); the window where it
    /// stood, its size, scale, z-order, state and focus untouched, and its
    /// picture back in place. The step taken, or `None` when the window
    /// was not moved.
    ///
    /// SAFETY: a live window of this thread's.
    unsafe fn nudge(hwnd: HWND) -> Option<(i32, i32)> {
        let mut at = RECT::default();
        let flags = SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
        // SAFETY: the rects are valid for the write and the reads; the
        // moves are this thread's window's, which winit answers as it
        // answers any move, and none of them changes its monitor.
        let taken = unsafe {
            if GetWindowRect(hwnd, &mut at) == 0 {
                return None;
            }
            let here = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let stays = |(x, y): (i32, i32)| {
                let to = RECT {
                    left: at.left.saturating_add(x),
                    top: at.top.saturating_add(y),
                    right: at.right.saturating_add(x),
                    bottom: at.bottom.saturating_add(y),
                };
                MonitorFromRect(&to, MONITOR_DEFAULTTONEAREST) == here
            };
            let taken = step(stays);
            if let Some((x, y)) = taken {
                let null = std::ptr::null_mut();
                let (left, top) = (at.left.saturating_add(x), at.top.saturating_add(y));
                SetWindowPos(hwnd, null, left, top, 0, 0, flags);
                SetWindowPos(hwnd, null, at.left, at.top, 0, 0, flags);
            }
            taken
        };
        let (left, top) = (at.left, at.top);
        if let Some((x, y)) = taken {
            tracing::debug!("display watch: the window nudged at {left},{top} by {x},{y}");
        } else {
            tracing::debug!("display watch: no nudge at {left},{top}: no step keeps its monitor");
        }
        taken
    }

    /// A power setting change's GUID, as a number, and its first DWORD of
    /// data; `None` for a null pointer or less data than a DWORD.
    ///
    /// SAFETY: `lparam` is a PBT_POWERSETTINGCHANGE's: null, or a
    /// POWERBROADCAST_SETTING followed by its DataLength bytes of data.
    unsafe fn setting(lparam: LPARAM) -> Option<(u128, u32)> {
        let at = lparam as *const u8;
        if at.is_null() {
            return None;
        }
        // SAFETY: the fields where the struct puts them, read unaligned,
        // and the data read only as far as DataLength says it runs.
        unsafe {
            let guid = at.add(offset_of!(POWERBROADCAST_SETTING, PowerSetting)).cast::<GUID>();
            let length = at.add(offset_of!(POWERBROADCAST_SETTING, DataLength)).cast::<u32>();
            if length.read_unaligned() < 4 {
                return None;
            }
            let data = at.add(offset_of!(POWERBROADCAST_SETTING, Data)).cast::<u32>();
            Some((number(&guid.read_unaligned()), data.read_unaligned()))
        }
    }

    /// A GUID as the number `GUID::from_u128` makes it from.
    fn number(guid: &GUID) -> u128 {
        (u128::from(guid.data1) << 96)
            | (u128::from(guid.data2) << 80)
            | (u128::from(guid.data3) << 64)
            | u128::from(u64::from_be_bytes(guid.data4))
    }

    #[cfg(test)]
    mod tests {
        use std::cell::Cell;
        use std::time::Duration;

        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, MsgWaitForMultipleObjects,
            PM_REMOVE, PeekMessageW, QS_ALLINPUT, SendMessageW,
        };

        use super::*;

        #[test]
        fn a_guid_reads_back_as_its_number() {
            assert_eq!(number(&GUID::from_u128(CONSOLE_DISPLAY_STATE)), CONSOLE_DISPLAY_STATE);
            assert_eq!(number(&GUID::from_u128((1 << 127) | 0xff)), (1 << 127) | 0xff);
        }

        /// The setting as Windows lays it out: the GUID, the length, and the
        /// data from the struct's one byte of it on, which a short length
        /// leaves unread.
        #[test]
        fn a_power_setting_is_read_from_where_windows_puts_it() {
            let mut change = POWERBROADCAST_SETTING::default();
            change.PowerSetting = GUID::from_u128(CONSOLE_DISPLAY_STATE);
            change.DataLength = 4;
            // A DWORD of data fits in the struct: its one byte, and the
            // padding after it.
            let data = offset_of!(POWERBROADCAST_SETTING, Data);
            assert!(data + 4 <= size_of::<POWERBROADCAST_SETTING>());
            // SAFETY: four bytes inside the struct, written unaligned.
            unsafe { (&raw mut change).cast::<u8>().add(data).cast::<u32>().write_unaligned(1) };
            let lparam = (&raw const change) as LPARAM;
            // SAFETY: a POWERBROADCAST_SETTING with its four bytes of data.
            assert_eq!(unsafe { setting(lparam) }, Some((CONSOLE_DISPLAY_STATE, 1)));
            change.DataLength = 1;
            let lparam = (&raw const change) as LPARAM;
            // SAFETY: as above, with less data than a DWORD.
            assert_eq!(unsafe { setting(lparam) }, None);
            // SAFETY: null is the one other lParam allowed.
            assert_eq!(unsafe { setting(0) }, None);
        }

        /// A hidden window of the system's STATIC class, on this thread: a
        /// real window procedure to go in front of, without a class to
        /// register.
        fn window() -> HWND {
            let null = std::ptr::null_mut();
            // SAFETY: a system class by name, no parent, menu or instance;
            // the strings are static.
            let hwnd = unsafe {
                CreateWindowExW(0, w!("STATIC"), w!(""), 0, 0, 0, 80, 60, null, null, null, null)
            };
            assert!(!hwnd.is_null(), "a STATIC window");
            hwnd
        }

        /// The procedure of `hwnd`, as GWLP_WNDPROC holds it.
        fn procedure_of(hwnd: HWND) -> isize {
            // SAFETY: a window of this thread's.
            unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) }
        }

        /// This thread's messages dispatched until `done` says so or ten
        /// seconds pass, `seen` shown each one first.
        fn pump(mut seen: impl FnMut(&MSG), done: impl Fn() -> bool) {
            let until = Instant::now() + Duration::from_secs(10);
            while !done() && Instant::now() < until {
                let mut msg = MSG::default();
                // SAFETY: this thread's queue, into a MSG it may write.
                unsafe {
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        seen(&msg);
                        DispatchMessageW(&msg);
                    }
                    MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 50, QS_ALLINPUT);
                }
            }
        }

        /// The rect `hwnd` stands in.
        fn rect_of(hwnd: HWND) -> RECT {
            let mut at = RECT::default();
            // SAFETY: a window of this thread's, into a RECT it may write.
            assert_ne!(unsafe { GetWindowRect(hwnd, &mut at) }, 0);
            at
        }

        /// A nudge on a real window wholly on one monitor (the primary's top
        /// left corner, where `window` makes it): a step right, and the
        /// window back where it stood, at its size.
        #[test]
        fn a_nudge_steps_right_and_leaves_the_window_where_it_stood() {
            let hwnd = window();
            let before = rect_of(hwnd);
            // SAFETY: a live window of this thread's.
            assert_eq!(unsafe { nudge(hwnd) }, Some((1, 0)));
            let after = rect_of(hwnd);
            let corners = |at: RECT| (at.left, at.top, at.right, at.bottom);
            assert_eq!(corners(after), corners(before), "where it stood");
            // SAFETY: as above.
            unsafe { DestroyWindow(hwnd) };
        }

        /// The whole watch on a real window: in front of its procedure, one
        /// window at a time; two display changes in a row sending for two
        /// pairs of nudges, which come back to the window as the watch's own
        /// message, the second signal's carrying the number that makes them
        /// due; then out again, by `unwatch` and by the window's destruction.
        /// One test, since the watch is one a process.
        #[test]
        fn the_watch_goes_in_front_of_a_window_and_steps_back_out() {
            let hwnd = window();
            let own = procedure_of(hwnd);
            watch_hwnd(hwnd);
            assert_eq!(procedure_of(hwnd), ours(), "the watch is in front");
            let other = window();
            watch_hwnd(other);
            assert_ne!(procedure_of(other), ours(), "a second window is not watched");
            // SAFETY: windows of this thread's, made above.
            unsafe { DestroyWindow(other) };

            // SAFETY: the watched window, on its thread.
            unsafe {
                SendMessageW(hwnd, wm::WM_DISPLAYCHANGE, 32, 0);
                SendMessageW(hwnd, wm::WM_DISPLAYCHANGE, 32, 0);
            }
            let (message, latest) = (NUDGE.load(Relaxed), LATEST.load(Relaxed));
            let (latest_came, earlier_came) = (Cell::new(0), Cell::new(0));
            pump(
                |msg| {
                    if msg.hwnd == hwnd && msg.message == message {
                        let came = if msg.wParam == latest { &latest_came } else { &earlier_came };
                        came.set(came.get() + 1);
                    }
                },
                || latest_came.get() == NUDGES.len() && earlier_came.get() == NUDGES.len(),
            );
            assert_eq!((latest_came.get(), earlier_came.get()), (2, 2), "both pairs came");

            unwatch();
            assert_eq!(LATEST.load(Relaxed), latest + 1, "the watch's end is a signal too");
            assert_eq!(procedure_of(hwnd), own, "the window's own procedure is back");
            assert_eq!((WATCHED.load(Relaxed), PREV.load(Relaxed), NOTIFY.load(Relaxed)), (0, 0, 0));

            watch_hwnd(hwnd);
            assert_eq!(procedure_of(hwnd), ours(), "watched again");
            // SAFETY: as above.
            unsafe { DestroyWindow(hwnd) };
            assert_eq!(
                (WATCHED.load(Relaxed), PREV.load(Relaxed), NOTIFY.load(Relaxed)),
                (0, 0, 0),
                "the window's destruction ended the watch"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message as the watch hears it, with a power setting of `setting`
    /// behind its lParam.
    fn hear(msg: u32, wparam: usize, setting: Option<(u128, u32)>) -> Option<Heard> {
        heard(msg, wparam, || setting)
    }

    #[test]
    fn a_display_change_and_a_resume_are_heard() {
        assert_eq!(hear(WM_DISPLAYCHANGE, 32, None), Some(Heard::DisplayChanged));
        assert_eq!(hear(WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC, None), Some(Heard::Resumed));
        // Suspending (PBT_APMSUSPEND, 4) and other messages are not.
        assert_eq!(hear(WM_POWERBROADCAST, 0x0004, None), None);
        assert_eq!(hear(0x0005, 0, None), None);
    }

    /// Only the console display's setting is read, and only from a setting
    /// change: the setting is not asked for on any other message, where
    /// lParam points at nothing of the kind.
    #[test]
    fn the_console_display_is_heard_from_its_setting_alone() {
        let change = |setting| hear(WM_POWERBROADCAST, PBT_POWERSETTINGCHANGE, setting);
        assert_eq!(change(Some((CONSOLE_DISPLAY_STATE, 0))), Some(Heard::ConsoleDisplay(0)));
        assert_eq!(change(Some((CONSOLE_DISPLAY_STATE, 2))), Some(Heard::ConsoleDisplay(2)));
        // GUID_MONITOR_POWER_ON, another display setting, and an unreadable one.
        assert_eq!(change(Some((0x0273_1015_4510_4526_99e6_e5a1_7ebd_1aea, 1))), None);
        assert_eq!(change(None), None);
        let asked = std::cell::Cell::new(false);
        let setting = || {
            asked.set(true);
            Some((CONSOLE_DISPLAY_STATE, 1))
        };
        assert_eq!(heard(WM_DISPLAYCHANGE, 0, setting), Some(Heard::DisplayChanged));
        assert!(!asked.get(), "a display change's lParam is not a setting");
    }

    #[test]
    fn a_display_change_and_a_resume_always_nudge() {
        let mut display = Display::default();
        assert!(display.hear(Heard::DisplayChanged));
        assert!(display.hear(Heard::DisplayChanged), "every time");
        assert!(display.hear(Heard::Resumed));
        // Whatever the console display last said.
        display.hear(Heard::ConsoleDisplay(DISPLAY_OFF));
        assert!(display.hear(Heard::DisplayChanged));
    }

    /// The console display nudges coming back on after it was off, once;
    /// its first report (registering sends the state it is in), and a
    /// dimmed display brightened, do not.
    #[test]
    fn the_console_display_nudges_only_coming_back_on() {
        let mut display = Display::default();
        assert!(!display.hear(Heard::ConsoleDisplay(DISPLAY_ON)), "the first report");
        assert!(!display.hear(Heard::ConsoleDisplay(2)), "dimmed");
        assert!(!display.hear(Heard::ConsoleDisplay(DISPLAY_ON)), "dimmed, then on");
        assert!(!display.hear(Heard::ConsoleDisplay(DISPLAY_OFF)), "off");
        assert!(display.hear(Heard::ConsoleDisplay(DISPLAY_ON)), "off, then on");
        assert!(!display.hear(Heard::ConsoleDisplay(DISPLAY_ON)), "and on again is no change");
        // Off, dimmed on the way back, then on: still back from off.
        display.hear(Heard::ConsoleDisplay(DISPLAY_OFF));
        assert!(!display.hear(Heard::ConsoleDisplay(2)));
        assert!(display.hear(Heard::ConsoleDisplay(DISPLAY_ON)));
        // A window opened while the display is off hears that first.
        let mut opened_dark = Display::default();
        assert!(!opened_dark.hear(Heard::ConsoleDisplay(DISPLAY_OFF)));
        assert!(opened_dark.hear(Heard::ConsoleDisplay(DISPLAY_ON)));
    }

    /// Only the latest signal's nudges are due: an earlier signal's, still
    /// on their way, are not, nor any once the watch's end has counted.
    #[test]
    fn only_the_latest_signal_s_nudges_are_due() {
        assert!(due(7, 7));
        assert!(!due(6, 7), "a burst's earlier signal");
        assert!(!due(7, 8), "the watch ended after it");
        assert!(NUDGES[0] < NUDGES[1], "the soon one first");
    }

    #[test]
    fn a_nudge_moves_only_a_window_on_screen() {
        assert!(moves(true, false));
        assert!(!moves(false, false), "hidden");
        assert!(!moves(true, true), "minimised");
        assert!(!moves(false, true));
    }

    /// A nudge steps right as a rule, and another way only where a pixel
    /// right would hand the window to the next monitor (and so, perhaps,
    /// another scale); not at all where every way would.
    #[test]
    fn a_nudge_steps_the_first_way_that_keeps_the_window_s_monitor() {
        assert_eq!(step(|_| true), Some((1, 0)), "right");
        assert_eq!(step(|(x, _)| x < 1), Some((-1, 0)), "left, the next monitor on the right");
        assert_eq!(step(|(x, _)| x == 0), Some((0, 1)), "down, monitors to both sides");
        assert_eq!(step(|(x, y)| x == 0 && y < 1), Some((0, -1)), "up, and one below");
        assert_eq!(step(|_| false), None, "every way another monitor");
        // Each way one pixel along one axis.
        assert!(STEPS.iter().all(|&(x, y)| x.abs() + y.abs() == 1));
    }
}
