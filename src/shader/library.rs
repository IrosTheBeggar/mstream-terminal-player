//! The presets this binary carries.
//!
//! The files live in `assets/visualizer/`, vendored from the mobile app and
//! left as the mobile app has them (the README there says what came from
//! where, and the one line that differs). They are compiled in, because the
//! binary is installed as one file.

/// One preset compiled into the binary.
pub struct Builtin {
    /// The vendored file's name, which is also its order: the mobile app
    /// numbers its presets, and the numbers are how both apps name them.
    pub file: &'static str,
    pub source: &'static str,
}

macro_rules! builtin {
    ($file:literal) => {
        Builtin { file: $file, source: include_str!(concat!("../../assets/visualizer/", $file)) }
    };
}

/// All of them but `04-cyber-fuji.glsl`, which is CC BY 3.0 where every
/// other preset is MIT or CC0. It stays vendored, and the tests still hold
/// it to the same bar, but it is not compiled into a GPL-3.0 binary until
/// that combination is settled (PLAN.md, Phase 10 watch items).
pub const BUILTIN: [Builtin; 8] = [
    builtin!("01-spectrum-bars.glsl"),
    builtin!("02-audio-tunnel.glsl"),
    builtin!("03-plasma-pulse.glsl"),
    builtin!("05-hex-marching.glsl"),
    builtin!("06-4d-beats.glsl"),
    builtin!("07-neonwave-sunrise.glsl"),
    builtin!("08-neonwave-sunset.glsl"),
    builtin!("09-mountainbytes.glsl"),
];

/// Every vendored preset, read from disk: the tests' view, so a file dropped
/// into the directory is tested without anyone remembering to list it —
/// the embedded set included, since it is a subset.
#[cfg(test)]
pub fn vendored() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/visualizer");
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .expect("assets/visualizer")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "glsl").then_some(path)
        })
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let source = std::fs::read_to_string(&path).expect("a readable preset");
            (name, source)
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
pub fn vendored_source(file: &str) -> Option<String> {
    vendored().into_iter().find(|(name, _)| name == file).map(|(_, source)| source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_set_is_the_vendored_set_minus_the_one_license_in_question() {
        let vendored: Vec<String> = vendored().into_iter().map(|(name, _)| name).collect();
        let builtin: Vec<&str> = BUILTIN.iter().map(|b| b.file).collect();
        let missing: Vec<&str> =
            vendored.iter().map(String::as_str).filter(|name| !builtin.contains(name)).collect();
        assert_eq!(missing, ["04-cyber-fuji.glsl"]);
        for preset in &BUILTIN {
            assert_eq!(Some(preset.source.to_string()), vendored_source(preset.file), "{}", preset.file);
        }
    }
}
