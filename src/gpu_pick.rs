//! Which wgpu instance, and which adapter from it, every window and probe
//! of ours draws with: the GUI's window (`gui --window`), the visualizer's
//! (`viz-window`, a process of its own) and the headless `viz-probe`.
//!
//! wgpu brings up every backend in an instance's mask when the instance is
//! made, and `request_adapter` with no preference answers the first adapter
//! it finds, so the choice is made with the mask. On Windows that is DX12
//! alone first, and Vulkan alone only when DX12 has no hardware adapter:
//! never GL, and never the two in one mask. The GL backend costs nothing to
//! draw with and much to have in the process: its hidden window puts a hook
//! on the window procedures (opengl32 in the stack of PR #41's Windows
//! retest), and with it a keyboard-layout change request posted to the
//! player's window — what the taskbar's language indicator posts — never
//! returned from `DefWindowProc`, on Windows 10, every time; with DX12
//! alone or Vulkan alone the same request switches the layout and the
//! window answers. That held for the visualizer's window too, which made
//! every backend's instance, so the rule lives here for both. DX12 before
//! Vulkan because of the stats lever: on a hybrid box the Vulkan instance
//! alone took half a second against DX12's 50 ms, and a mask holding both
//! would have paid for Vulkan and been answered by it.
//!
//! DX12 answers even where no D3D12 hardware is, with WARP, Microsoft's
//! software rasteriser (`DeviceType::Cpu`): taking the first adapter found
//! would put such a machine's every frame on the CPU while a Vulkan driver
//! sat unused. So a software adapter is held, not taken: the next mask is
//! tried, and the software one is the last resort when no mask has
//! hardware ([`settle`]).
//!
//! `WGPU_BACKEND` still decides when it is set, through wgpu's own reading
//! of it, and Linux and macOS keep the env descriptor: one instance, whose
//! display handle is what Wayland and X11 need to make a surface later.

use std::time::{Duration, Instant};

/// What [`choose`] settled on: the instance, what the caller's `find`
/// answered from it if anything (an adapter, and whatever the caller made
/// on the way, such as a surface), and what the instances and the finds
/// took altogether.
pub struct Choice<T> {
    pub instance: wgpu::Instance,
    pub found: Option<(wgpu::Adapter, T)>,
    // Read only by the GUI window's startup stats; two clock reads, so the
    // terminal build measures them too rather than splitting `choose`.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub instance_took: Duration,
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub adapter_took: Duration,
}

/// The instance to draw with and the adapter `find` answers from it. `find`
/// is the caller's request (its own options, a surface to be compatible
/// with): asked once on Linux and macOS, or with `WGPU_BACKEND` set, and on
/// Windows once per mask in [`windows_masks`]' order until it answers with
/// hardware. `display` goes into the env descriptor where there is one to
/// carry (a window's, or the event loop's); `None` for a headless probe.
pub fn choose<T>(
    display: Option<Box<dyn wgpu::wgt::WgpuHasDisplayHandle>>,
    mut find: impl FnMut(&wgpu::Instance) -> Option<(wgpu::Adapter, T)>,
) -> Choice<T> {
    let mut instance_took = Duration::ZERO;
    let mut adapter_took = Duration::ZERO;
    let mut attempt = |descriptor: wgpu::InstanceDescriptor| {
        let started = Instant::now();
        let instance = wgpu::Instance::new(descriptor);
        instance_took += started.elapsed();
        let started = Instant::now();
        let found = find(&instance);
        adapter_took += started.elapsed();
        (instance, found)
    };
    let (instance, found) = match windows_masks() {
        Some(masks) => {
            let tries = masks.iter().map(|&backends| {
                // `with_env` keeps the mask given here, since `WGPU_BACKEND`
                // is unset on this path, and still reads the rest of wgpu's
                // environment (validation, backend options).
                attempt(
                    wgpu::InstanceDescriptor {
                        backends,
                        ..wgpu::InstanceDescriptor::new_without_display_handle()
                    }
                    .with_env(),
                )
            });
            settle(tries, |(_, found)| {
                found.as_ref().map(|(adapter, _)| is_hardware(adapter.get_info().device_type))
            })
            .expect("at least one mask is tried")
        }
        None => attempt(match display {
            Some(display) => wgpu::InstanceDescriptor::new_with_display_handle_from_env(display),
            None => wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
        }),
    };
    Choice { instance, found, instance_took, adapter_took }
}

/// The masks to try one at a time, in order: on Windows when `WGPU_BACKEND`
/// does not say, DX12 then Vulkan; `None` everywhere else, for the one
/// instance of the env descriptor.
fn windows_masks() -> Option<&'static [wgpu::Backends]> {
    const WINDOWS: &[wgpu::Backends] = &[wgpu::Backends::DX12, wgpu::Backends::VULKAN];
    (cfg!(windows) && wgpu::Backends::from_env().is_none()).then_some(WINDOWS)
}

/// Whether an adapter of this kind draws on a GPU. `Other` is what a driver
/// that does not say reports, and is taken at its word; only `Cpu` (WARP,
/// llvmpipe, SwiftShader) is software.
fn is_hardware(kind: wgpu::DeviceType) -> bool {
    kind != wgpu::DeviceType::Cpu
}

/// The rule, over tries made lazily in order. `hardware` reads a try:
/// `None` when it found no adapter, else whether the adapter is hardware.
/// The first hardware try is taken, and nothing after it is tried; with
/// none, the first try that found software is taken (it was kept while the
/// later ones were tried), and with none of that either, the last try,
/// whose instance stands for "nothing would draw". `None` only when there
/// was no try at all.
fn settle<C>(tries: impl Iterator<Item = C>, hardware: impl Fn(&C) -> Option<bool>) -> Option<C> {
    let mut software = None;
    let mut last = None;
    for found in tries {
        match hardware(&found) {
            Some(true) => return Some(found),
            Some(false) if software.is_none() => software = Some(found),
            _ => last = Some(found),
        }
    }
    software.or(last)
}

/// A native crash ends the process at once, with its exception code, rather
/// than leaving it suspended behind a Windows Error Reporting dialog. Called
/// first thing by each of our ways into a GPU device, in both flavours: the
/// window host (the GUI, the wizard and Quick Connect), the visualizer's own
/// process and the visualizer probe. The terminal flavour has the last two
/// as well (its GUI starts the same `viz-window` child), so this lives here,
/// beside the choice they all draw with, rather than in the desktop half.
///
/// A crash inside a graphics driver is a fast fail (a /GS stack-cookie
/// check, 0xC0000409) that no handler of ours ever sees. By default WER then
/// suspends the process and shows "has stopped working" until someone closes
/// it, while the window, hidden until its first present, never shows: a
/// launcher that started the player sees it still running, takes the page
/// for up and never falls back, and a GUI that started the visualizer's
/// window takes it for open and sends its V to a suspended process. Measured
/// with the NVIDIA driver that crashes on a long executable path (Windows 10
/// 22H2, GTX 1060, a debug build): without this the process sat behind the
/// dialog until killed; with SEM_NOGPFAULTERRORBOX it was gone 0.0–0.3 s
/// after WerFault started, with 0xC0000409 and no dialog (WER still runs,
/// without any UI, and may queue a report). WerSetFlags
/// (WER_FAULT_REPORTING_NO_UI) was measured too: no dialog either, but the
/// process stayed suspended another 2.3–2.9 s while WER took its dump, and
/// on top of the error mode it changed nothing.
///
/// SEM_FAILCRITICALERRORS is the companion Microsoft recommends for every
/// application: a drive with no disk is an error to the caller, not a system
/// message box. Both bits are inherited by the processes this one starts,
/// which is right for them too; the visualizer still sets them itself, as an
/// older player may be the one that starts it.
#[cfg(windows)]
pub fn quiet_native_crashes() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    // SAFETY: no pointers; the calls read and set this process's error mode.
    unsafe { SetErrorMode(GetErrorMode() | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
}

/// The length of the executable's path, in UTF-16 units as Windows counts
/// it, from which [`long_path_warning`] warns: a little short of where it
/// was seen to matter, so a path a few characters off the edge is named too.
const LONG_EXE_PATH: usize = 250;

/// On Windows, the line each of our ways into a GPU device prints before it
/// makes one, when the executable's path is [`LONG_EXE_PATH`] or longer;
/// `None` for any shorter. `who` starts the line (`gui --window`,
/// `viz-window`, `viz-probe`), as the rest of that command's lines start.
///
/// NVIDIA's driver (31.0.15.3640 on a GTX 1060, DX12 and Vulkan alike) was
/// measured to fast-fail inside itself while the device is made once the
/// path reaches 253 units, 252 being fine: the window never appears, and
/// the process ends ([`quiet_native_crashes`]) with nothing of ours to say
/// why. It lives here, beside the crash quieting, for the same reason that
/// does: all three ways in make their device from the same executable, and
/// the terminal flavour, which has no window host, still starts the
/// visualizer's window and runs the probe. A warning alone: the run goes on
/// as before, since most drivers do not care. It reaches only who keeps it,
/// though: a parent that captures stderr (the mStream launcher's page log;
/// the player's own log for its visualizer child, as `[viz-window] …`, and
/// the "visualizer failed" note when it is the child's last line), the
/// probe's own report, or a debug log the run writes (`MSTREAM_LOG`, or the
/// config's `[log]`); a double-click with neither leaves no trace of it, and
/// no dialog either. Counted through a lossy string, which keeps the count
/// exact: an unpaired surrogate, one unit, is replaced by U+FFFD, one unit
/// too.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn long_path_warning(who: &str, exe: &std::path::Path) -> Option<String> {
    let units = exe.as_os_str().to_string_lossy().encode_utf16().count();
    (units >= LONG_EXE_PATH).then(|| {
        format!(
            "{who}: this player's path is {units} characters long; some graphics drivers \
             (NVIDIA, measured at 253 and up) crash on paths that long, so if no picture \
             appears, move the player to a shorter folder"
        )
    })
}

/// [`long_path_warning`] for this process's own executable: `None` too when
/// the path cannot be read, since then there is nothing to measure.
#[cfg(windows)]
pub fn this_long_path_warning(who: &str) -> Option<String> {
    std::env::current_exe().ok().and_then(|exe| long_path_warning(who, &exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A try as the rule sees it: which mask, and the adapter's kind if it
    /// found one. `tried` counts how far the lazy iterator was walked.
    fn walk(tries: &[(&'static str, Option<wgpu::DeviceType>)]) -> (Option<&'static str>, usize) {
        let mut tried = 0;
        let picked = settle(
            tries.iter().inspect(|_| tried += 1),
            |(_, kind)| kind.map(is_hardware),
        );
        (picked.map(|(name, _)| *name), tried)
    }

    #[test]
    fn hardware_on_the_first_mask_stops_there() {
        use wgpu::DeviceType::*;
        let got = walk(&[("dx12", Some(DiscreteGpu)), ("vulkan", Some(IntegratedGpu))]);
        assert_eq!(got, (Some("dx12"), 1));
        // A driver that does not say what it is counts as hardware.
        assert_eq!(walk(&[("dx12", Some(Other)), ("vulkan", Some(DiscreteGpu))]).0, Some("dx12"));
    }

    /// WARP is DX12's answer on a machine without D3D12 hardware: Vulkan is
    /// still tried, and takes it when it has a GPU.
    #[test]
    fn software_on_dx12_tries_vulkan() {
        use wgpu::DeviceType::*;
        let got = walk(&[("dx12", Some(Cpu)), ("vulkan", Some(IntegratedGpu))]);
        assert_eq!(got, (Some("vulkan"), 2));
        assert_eq!(walk(&[("dx12", None), ("vulkan", Some(DiscreteGpu))]).0, Some("vulkan"));
    }

    /// With no hardware anywhere, the software adapter held from DX12 is
    /// the last resort, ahead of a Vulkan that found nothing or software
    /// of its own; with nothing anywhere, the last instance tried stands.
    #[test]
    fn software_is_the_last_resort() {
        use wgpu::DeviceType::*;
        assert_eq!(walk(&[("dx12", Some(Cpu)), ("vulkan", None)]), (Some("dx12"), 2));
        assert_eq!(walk(&[("dx12", Some(Cpu)), ("vulkan", Some(Cpu))]).0, Some("dx12"));
        assert_eq!(walk(&[("dx12", None), ("vulkan", Some(Cpu))]).0, Some("vulkan"));
        assert_eq!(walk(&[("dx12", None), ("vulkan", None)]), (Some("vulkan"), 2));
        assert_eq!(walk(&[]), (None, 0));
    }

    /// Off Windows (and on it with `WGPU_BACKEND` set) there is one
    /// instance, the env descriptor's.
    #[test]
    #[cfg(not(windows))]
    fn one_instance_off_windows() {
        assert_eq!(windows_masks(), None);
    }

    /// The long-path warning starts at 250 UTF-16 units, as Windows counts
    /// a path: a character beyond the Basic Multilingual Plane is two, an
    /// accented letter one, and a lone surrogate one as well. The line
    /// starts with the command that says it.
    #[test]
    fn a_path_of_250_units_or_more_is_warned_about() {
        use std::path::{Path, PathBuf};
        // `C:\` and `\mstream-player.exe` are 22 units around the folder.
        let exe = |folder: &str| PathBuf::from(format!("C:\\{folder}\\mstream-player.exe"));
        let ascii = |units: usize| exe(&"d".repeat(units - 22));
        let warn = |path: &Path| long_path_warning("gui --window", path);
        assert_eq!(warn(Path::new("C:\\mStream\\mstream-player.exe")), None);
        assert_eq!(warn(&ascii(249)), None);
        let line = warn(&ascii(250)).expect("250 units is warned about");
        assert!(line.starts_with("gui --window: this player's path is 250 characters"), "{line}");
        assert!(line.contains("shorter folder"), "{line}");
        assert!(warn(&ascii(253)).unwrap().contains(" 253 characters"));

        // 226 letters and a clef (two units) are 250; with an é instead of
        // the clef, 249.
        let folder = "d".repeat(226);
        assert!(warn(&exe(&format!("{folder}\u{1D11E}"))).is_some());
        assert_eq!(warn(&exe(&format!("{folder}é"))), None);

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            let path = ascii(249);
            let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
            wide.insert(3, 0xD800);
            let lone = PathBuf::from(std::ffi::OsString::from_wide(&wide));
            assert!(warn(&lone).unwrap().contains(" 250 characters"));
        }
    }

    /// The visualizer's process and the probe say the same line under their
    /// own names: the terminal flavour, which has no window host, meets the
    /// same driver through them.
    #[test]
    fn the_visualizer_and_the_probe_warn_under_their_own_names() {
        let exe = std::path::PathBuf::from(format!("C:\\{}\\mstream-player.exe", "d".repeat(240)));
        for who in ["viz-window", "viz-probe"] {
            let line = long_path_warning(who, &exe).expect("262 units is warned about");
            assert!(line.starts_with(&format!("{who}: this player's path is 262 ")), "{line}");
        }
    }
}
