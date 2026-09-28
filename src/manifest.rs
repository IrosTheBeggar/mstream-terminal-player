//! Tests over Cargo.toml and Cargo.lock themselves: the build choices that
//! nothing at run time would notice losing. A debug build that quietly
//! stops optimising its cover crates still passes every other test, only
//! slower; these fail instead.

use std::collections::BTreeSet;

fn manifest() -> toml::Table {
    let raw = include_str!("../Cargo.toml");
    toml::from_str(raw).expect("Cargo.toml parses")
}

/// Every package name in the lockfile, all versions folded together.
fn locked() -> BTreeSet<String> {
    let raw = include_str!("../Cargo.lock");
    let lock: toml::Table = toml::from_str(raw).expect("Cargo.lock parses");
    lock["package"]
        .as_array()
        .expect("the lockfile lists packages")
        .iter()
        .filter_map(|package| package.get("name")?.as_str().map(str::to_string))
        .collect()
}

/// The debug profile's per-crate optimisation (performance audit #133).
/// Cargo only warns about an override naming a crate the tree no longer
/// has — a codec swapped out upstream would leave its successor at
/// opt-level 0 with a line in the build log nobody reads.
#[test]
fn the_debug_profile_optimises_crates_the_tree_has() {
    let manifest = manifest();
    let overrides = manifest["profile"]["dev"]["package"]
        .as_table()
        .expect("the dev profile names its optimised crates");
    assert!(
        !overrides.contains_key("*"),
        "scoped on purpose: \"*\" would optimise naga, wgpu and iroh on every cold build"
    );
    let locked = locked();
    for (name, settings) in overrides {
        assert!(locked.contains(name), "{name} is optimised but not in the tree");
        assert_eq!(settings["opt-level"].as_integer(), Some(3), "{name}");
    }
    // The cover path end to end, and the audio decoders' core.
    for name in ["zune-jpeg", "image", "ratatui-image", "icy_sixel", "quantette", "symphonia-core"] {
        assert!(overrides.contains_key(name), "{name} lost its override");
    }
}
