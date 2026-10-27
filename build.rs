// Embed the mStream icon into the Windows executable: Explorer, shortcuts,
// pinned taskbar entries, and a classic conhost window all display the exe's
// own icon group, and without one every surface shows the generic binary
// glyph. winresource also stamps default VersionInfo from the Cargo metadata
// while it's in there (consumers that re-stamp VersionInfo, like mStream's
// bundler, simply replace it). Every bin of the package gets the same
// resource: on msvc winresource links the compiled .res with
// `cargo:rustc-link-arg`, which cargo passes to every binary's link (the
// desktop flavour's launcher stub, src/bin/launch.rs, included), and on gnu
// with a whole-archive `rustc-link-lib`, which with no library target
// reaches each bin as well.
//
// Resource compilation needs a Windows resource compiler (rc.exe). The
// release CI builds win32 on a windows runner where that always holds; a
// CROSS-host check (mac/linux running clippy against the msvc target) skips
// with a warning instead of failing the whole build over a cosmetic
// resource. On a real windows host a failure is a build break on purpose —
// CI must never silently ship an iconless exe again.
fn main() {
    println!("cargo:rerun-if-changed=assets/mstream-logo.ico");
    // The wizard and admin copy is embedded at compile time (rust_i18n::i18n!),
    // and the macro leaves no trace cargo can see: a locale-only edit must
    // still rebuild, or the binary keeps rendering yesterday's strings.
    println!("cargo:rerun-if-changed=locales");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/mstream-logo.ico");
    // The desktop flavour is an app: Task Manager, the taskbar's jump list
    // and "Open with" name a program by its FileDescription, which defaults
    // to the package name. The terminal flavour keeps that default.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        res.set("FileDescription", "mStream Player");
    }
    match res.compile() {
        Ok(()) => {}
        Err(e) if !cfg!(windows) => {
            println!("cargo:warning=windows icon not embedded (cross-host, no resource compiler): {e}");
        }
        Err(e) => panic!("failed to embed the windows icon: {e}"),
    }
}
