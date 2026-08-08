//! A flight recorder for the transition machinery.
//!
//! Off unless `MSTREAM_ENGINE_TRACE` names a file at startup; then every
//! decision the blend logic makes — prepares, failures with their reasons,
//! gates, handovers, seeks — writes one timestamped line, into a file that
//! is truncated at start so it *is* this run. The `stderrln!` diagnostics
//! land here too: the TUI silences stderr, and without this a listening
//! session that went wrong left nothing to read afterwards. When the
//! variable is unset the cost is one initialized-OnceLock load per call
//! site. Third-party telemetry is the other file's job (`MSTREAM_LOG`,
//! crate::logging).

use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static SINK: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();

/// The recorder's file: truncated, so the file *is* this run — it used to
/// append forever, which read as one endless session and grew without
/// bound when the variable lived in a shell profile. The header names the
/// build, since a trace is usually read next to a bug report.
fn start_file(path: &OsStr) -> Option<File> {
    let mut file = File::create(path).ok()?;
    let _ = writeln!(
        file,
        "── mstream-player v{} — one run per file ──",
        env!("CARGO_PKG_VERSION")
    );
    Some(file)
}

pub(crate) fn line(args: std::fmt::Arguments) {
    // Every decision also goes out as a tracing event, which is what puts
    // the player's own voice in the debug log beside its dependencies'.
    // Cheap when nothing is listening, and the reason the in-memory ring
    // holds a readable session at the default level: iroh and reqwest keep
    // their interesting detail at debug and trace, but a listener wants to
    // know what the *player* decided.
    tracing::info!(target: "mstream", "{args}");

    let Some((file, t0)) = SINK
        .get_or_init(|| {
            let path = std::env::var_os("MSTREAM_ENGINE_TRACE")?;
            Some((Mutex::new(start_file(&path)?), Instant::now()))
        })
        .as_ref()
    else {
        return;
    };
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "[{:9.3}s] {}", t0.elapsed().as_secs_f64(), args);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_run_truncates_and_signs_its_file() {
        let path = std::env::temp_dir().join("mstream-player-test-trace.log");
        std::fs::write(&path, "left over from an earlier run
").unwrap();

        let mut file = start_file(path.as_os_str()).expect("open");
        writeln!(file, "[    0.000s] play something").unwrap();
        drop(file);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("left over"), "the old run must be gone: {text}");
        assert!(text.starts_with("── mstream-player v"), "{text}");
        assert!(text.contains("play something"));
        let _ = std::fs::remove_file(&path);
    }
}

/// `etrace!("fmt", args…)` — one line into the flight recorder, or nothing.
macro_rules! etrace {
    ($($arg:tt)*) => {
        $crate::engine::trace::line(format_args!($($arg)*))
    };
}
pub(crate) use etrace;
