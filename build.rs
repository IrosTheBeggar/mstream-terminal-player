// Embed the mStream icon into the Windows executable: Explorer, shortcuts,
// pinned taskbar entries, and a classic conhost window all display the exe's
// own icon group, and without one every surface shows the generic binary
// glyph. winresource also stamps default VersionInfo from the Cargo metadata
// while it's in there (consumers that re-stamp VersionInfo, like mStream's
// bundler, simply replace it).
//
// Resource compilation needs a Windows resource compiler (rc.exe). The
// release CI builds win32 on a windows runner where that always holds; a
// CROSS-host check (mac/linux running clippy against the msvc target) skips
// with a warning instead of failing the whole build over a cosmetic
// resource. On a real windows host a failure is a build break on purpose —
// CI must never silently ship an iconless exe again.
fn main() {
    println!("cargo:rerun-if-changed=assets/mstream-logo.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/mstream-logo.ico");
    match res.compile() {
        Ok(()) => {}
        Err(e) if !cfg!(windows) => {
            println!("cargo:warning=windows icon not embedded (cross-host, no resource compiler): {e}");
        }
        Err(e) => panic!("failed to embed the windows icon: {e}"),
    }
}
