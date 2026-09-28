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
    // The wizard and admin copy is embedded at compile time (the locale
    // table below): a locale-only edit must rebuild, or the binary keeps
    // rendering yesterday's strings.
    println!("cargo:rerun-if-changed=locales");
    write_locale_table();
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

// The locale table as one sorted static (src/locale_table.rs says why the
// i18n! macro no longer builds it). Read through the macro's own loader, so
// the key flattening and order are exactly what `i18n!("locales")` produced;
// a BTreeMap iterates keys in byte order, which the backend's binary search
// relies on.
fn write_locale_table() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let locales = rust_i18n_support::load_locales(&format!("{dir}/locales"), |_| false);
    assert!(!locales.is_empty(), "no locales found under {dir}/locales");
    let mut out = String::with_capacity(1 << 20);
    out.push_str("pub static LOCALES: &[(&str, &[(&str, &str)])] = &[\n");
    for (locale, keys) in &locales {
        out.push_str(&format!("    ({locale:?}, &[\n"));
        for (key, value) in keys {
            out.push_str(&format!("        ({key:?}, {value:?}),\n"));
        }
        out.push_str("    ]),\n");
    }
    out.push_str("];\n");
    let out_dir = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    std::fs::write(std::path::Path::new(&out_dir).join("locale_table.rs"), out)
        .expect("write the locale table");
}
