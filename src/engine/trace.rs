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
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static SINK: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();

/// The recorder's file: opened for APPEND, and each run signs itself.
///
/// It briefly truncated instead, so a file would be exactly one run — which
/// was wrong in the case this facility is most used in. Two players sharing
/// one `MSTREAM_ENGINE_TRACE` (a variable that lives in a shell profile is
/// inherited by every player started from that shell) each truncated the
/// other's file and then wrote at their own offsets, filling the overlap
/// with NULs and destroying both accounts (PR #5 review). O_APPEND puts
/// every line at the end whoever is writing, so the worst two players do to
/// each other now is interleave.
///
/// The cost is a file that grows across runs, which is the right price
/// here: this facility is opt-in and short-lived, and the rotating,
/// size-managed channel is the other one (`MSTREAM_LOG`, crate::logging).
fn start_file(path: &OsStr) -> Option<File> {
    let mut file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    let _ = writeln!(
        file,
        "\n── mstream-player v{} — run started ──",
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

    /// Two players may share one MSTREAM_ENGINE_TRACE, so a run signs
    /// itself and appends — it must never destroy an account already there.
    #[test]
    fn a_run_signs_itself_and_keeps_what_came_before() {
        let path = std::env::temp_dir().join("mstream-player-test-trace.log");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "[    0.000s] an earlier run\n").unwrap();

        let mut file = start_file(path.as_os_str()).expect("open");
        writeln!(file, "[    0.000s] play something").unwrap();
        drop(file);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("an earlier run"), "the earlier account survives: {text}");
        assert!(text.contains("── mstream-player v"), "and this run signs itself: {text}");
        assert!(text.contains("play something"));

        // A second player opening the same path writes after it, not over it.
        let mut second = start_file(path.as_os_str()).expect("open again");
        writeln!(second, "[    0.000s] the other player").unwrap();
        drop(second);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("play something"), "the first player's lines stand: {text}");
        assert!(text.contains("the other player"));
        assert!(!text.contains('\0'), "and nothing is filled with NULs: {text:?}");
        assert_eq!(text.matches("run started").count(), 2, "two runs, two banners");

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
