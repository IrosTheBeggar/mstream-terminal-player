//! The window's icon: the mStream logo at 256 px (assets/icons/, made from
//! the Windows .ico by scripts/icons.py), embedded so a bare binary carries
//! it with no install step.
//!
//! Where it goes differs by platform. Windows and X11 take it as the
//! window's own icon (the title bar, the taskbar, Alt-Tab), decoded to RGBA
//! by the image crate the album art already uses, on a thread of its own so
//! the decode overlaps the event loop's start instead of delaying it.
//! Wayland ignores a window icon: the compositor finds the icon through the
//! desktop entry the window's app id names (identity.rs). macOS ignores it
//! too, and gives a binary outside an .app bundle the generic executable's
//! icon in the Dock and Cmd-Tab; there the PNG is handed to AppKit as the
//! application's icon instead, which AppKit decodes itself, so no decode
//! thread runs on a Mac. A later .app bundle's icns makes that call
//! redundant, not wrong.

/// The 256 px logo: large enough for a 4K taskbar and Alt-Tab's big tiles,
/// small enough to embed (about 11 KB).
pub(super) const PNG: &[u8] =
    include_bytes!("../../../assets/icons/hicolor/256x256/apps/io.mstream.player.png");

/// The decoded logo, ready for `winit::window::Icon::from_rgba`. Plain data,
/// so it can cross from the decoding thread: a winit `Icon` on Windows
/// holds an HICON, which is not `Send`, and is made on the loop's thread.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) struct Rgba {
    pub(super) pixels: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// [`PNG`] as RGBA; `None` (and a note on stderr) if it will not decode,
/// which leaves the platform's default icon and nothing else.
#[cfg(any(not(target_os = "macos"), test))]
pub(super) fn decode() -> Option<Rgba> {
    match image::load_from_memory_with_format(PNG, image::ImageFormat::Png) {
        Ok(decoded) => {
            let rgba = decoded.into_rgba8();
            let (width, height) = rgba.dimensions();
            Some(Rgba { pixels: rgba.into_raw(), width, height })
        }
        Err(e) => {
            eprintln!("gui --window: the window icon would not decode ({e})");
            None
        }
    }
}

/// [`decode`] on a thread of its own, started before the event loop.
#[cfg(not(target_os = "macos"))]
pub(super) fn decode_early() -> Option<std::thread::JoinHandle<Option<Rgba>>> {
    std::thread::Builder::new().name("window-icon".into()).spawn(decode).ok()
}

/// The winit icon from the decoded pixels, on the loop's thread.
#[cfg(not(target_os = "macos"))]
pub(super) fn winit_icon(rgba: Rgba) -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(rgba.pixels, rgba.width, rgba.height).ok()
}

/// The Dock's and Cmd-Tab's icon for this process: [`PNG`], through
/// `NSApplication.applicationIconImage`, on the main thread as AppKit
/// requires (anywhere else it does nothing). Called from the loop's first
/// `resumed`, which winit delivers once the application has finished
/// launching: set any earlier (straight after `EventLoop::new`, tried)
/// and AppKit's own launch puts the generic executable's icon back, so the
/// Dock showed "exec". It costs about 15 ms on that thread (NSImage
/// decodes the PNG itself), which the window's creation right after gives
/// back: AppKit's connection to the Dock is made here instead of there,
/// and the first present measured the same with it as without.
#[cfg(target_os = "macos")]
pub(super) fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(main) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        eprintln!("gui --window: the Dock icon would not decode");
        return;
    };
    let app = NSApplication::sharedApplication(main);
    // SAFETY: on the main thread (the marker), with a live image; AppKit
    // retains it. (Unsafe only because the binding cannot know whether
    // `None` would be accepted; it is not passed.)
    unsafe { app.setApplicationIconImage(Some(&image)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_decodes_to_256_square() {
        let rgba = decode().expect("the embedded PNG decodes");
        assert_eq!((rgba.width, rgba.height), (256, 256));
        assert_eq!(rgba.pixels.len(), 256 * 256 * 4);
        // A logo on a transparent ground: some pixels clear, some opaque.
        // (The .ico's ground is alpha 1, not 0, in every frame; the script
        // keeps the art as it is.)
        let alphas: Vec<u8> = rgba.pixels.iter().skip(3).step_by(4).copied().collect();
        assert!(alphas.iter().any(|&a| a <= 1) && alphas.contains(&255));
        // And what winit is handed it accepts.
        #[cfg(not(target_os = "macos"))]
        assert!(winit_icon(rgba).is_some());
    }

    #[test]
    fn the_icon_set_on_disk_decodes_to_its_sizes() {
        let icons = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons/hicolor");
        for size in [16u32, 32, 48, 64, 128, 256, 512] {
            let path = icons.join(format!("{size}x{size}/apps/{}.png", crate::identity::APP_ID));
            let decoded = image::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!((decoded.width(), decoded.height()), (size, size), "{}", path.display());
            assert!(decoded.color().has_alpha(), "{}", path.display());
        }
        // The embedded logo is the set's 256 px file, byte for byte.
        let on_disk = std::fs::read(icons.join("256x256/apps/io.mstream.player.png")).unwrap();
        assert_eq!(on_disk, PNG);
    }
}
