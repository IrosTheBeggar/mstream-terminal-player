//! The optional debug log: everything the dependencies narrate, in a file.
//!
//! iroh, reqwest, hyper and stream-download all speak `tracing` — relay
//! connects, holepunch attempts, path upgrades, reconnects — and without a
//! subscriber every word of it is discarded. `MSTREAM_LOG` installs one,
//! writing to a file and never to the terminal: the TUI's screen stays a
//! screen, and serve mode's stderr stays the concise log it already is.
//!
//! Two spellings:
//!
//!   MSTREAM_LOG=1                 → the default location, rotated
//!   MSTREAM_LOG=/path/to/file    → exactly that file, truncated
//!
//! The default location is `<cache>/logs/mstream-player.log`, with the last
//! few runs kept beside it as `.1` through `.4` — bounded however long the
//! variable stays exported. `RUST_LOG` chooses what gets said (the usual
//! `tracing` filter grammar); unset, it means `info`.
//!
//! This is the third-party channel. The player's own transition decisions
//! have their own recorder (`MSTREAM_ENGINE_TRACE`, engine::trace), which
//! also collects the `stderrln!` diagnostics the TUI would otherwise
//! silence — the two files answer different questions and stay separate.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// How many rotated predecessors the default location keeps.
const KEEP: usize = 4;

static ACTIVE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// What `MSTREAM_LOG` asked for.
#[derive(Debug, PartialEq)]
enum Target {
    Off,
    /// A truthy switch: log to the default, rotated location.
    Default,
    /// An explicit path: log exactly there, truncating.
    Path(PathBuf),
}

fn wants(value: Option<&str>) -> Target {
    let Some(value) = value else { return Target::Off };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Target::Off;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Target::Off,
        "1" | "true" | "yes" | "on" => Target::Default,
        _ => Target::Path(PathBuf::from(trimmed)),
    }
}

/// Shift `base` → `base.1` → … in `dir`, dropping the oldest, so the file
/// about to be created is the newest of at most KEEP+1.
fn rotate(dir: &Path, base: &str) {
    let _ = fs::remove_file(dir.join(format!("{base}.{KEEP}")));
    for n in (1..KEEP).rev() {
        let _ = fs::rename(dir.join(format!("{base}.{n}")), dir.join(format!("{base}.{}", n + 1)));
    }
    let _ = fs::rename(dir.join(base), dir.join(format!("{base}.1")));
}

/// Resolve the target to a file, creating directories and rotating or
/// truncating as the spelling asks.
fn open_target(target: Target) -> Option<PathBuf> {
    match target {
        Target::Off => None,
        Target::Default => {
            let dir = crate::config::log_dir()?;
            fs::create_dir_all(&dir).ok()?;
            rotate(&dir, "mstream-player.log");
            Some(dir.join("mstream-player.log"))
        }
        Target::Path(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).ok()?;
            }
            Some(path)
        }
    }
}

/// Read `MSTREAM_LOG`, install the subscriber if it asks for one, and
/// remember the path for the goodbye line. Call once, first thing in main —
/// anything that dials before this speaks into the void.
pub fn init() -> Option<&'static Path> {
    let path = ACTIVE
        .get_or_init(|| {
            let target = std::env::var("MSTREAM_LOG").ok();
            let path = open_target(wants(target.as_deref()))?;
            let mut file = File::create(&path).ok()?;
            let _ = writeln!(
                file,
                "── mstream-player v{} — set RUST_LOG to choose what is said ──",
                env!("CARGO_PKG_VERSION")
            );
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            let installed = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .try_init()
                .is_ok();
            installed.then_some(path)
        })
        .as_deref();
    path
}

/// Where the log is going, if anywhere — for the boot and goodbye lines.
pub fn active() -> Option<&'static Path> {
    ACTIVE.get().and_then(|p| p.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spellings_of_mstream_log_mean_what_they_say() {
        assert_eq!(wants(None), Target::Off);
        assert_eq!(wants(Some("")), Target::Off);
        assert_eq!(wants(Some("0")), Target::Off);
        assert_eq!(wants(Some("off")), Target::Off);
        assert_eq!(wants(Some("1")), Target::Default);
        assert_eq!(wants(Some("TRUE")), Target::Default);
        assert_eq!(wants(Some("/tmp/x.log")), Target::Path(PathBuf::from("/tmp/x.log")));
        // A bare filename is a path, not a switch — "log.txt" should not
        // silently mean "default location".
        assert_eq!(wants(Some("log.txt")), Target::Path(PathBuf::from("log.txt")));
    }

    #[test]
    fn rotation_keeps_the_last_runs_and_drops_the_oldest() {
        let dir = std::env::temp_dir().join("mstream-player-test-logrotate");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for name in
            ["mstream-player.log", "mstream-player.log.1", "mstream-player.log.4"]
        {
            fs::write(dir.join(name), name).unwrap();
        }

        rotate(&dir, "mstream-player.log");

        // The live file moved to .1, old .1 to .2, and the .4 fell away.
        assert!(!dir.join("mstream-player.log").exists());
        assert_eq!(fs::read_to_string(dir.join("mstream-player.log.1")).unwrap(),
            "mstream-player.log");
        assert_eq!(fs::read_to_string(dir.join("mstream-player.log.2")).unwrap(),
            "mstream-player.log.1");
        assert!(!dir.join("mstream-player.log.4").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
