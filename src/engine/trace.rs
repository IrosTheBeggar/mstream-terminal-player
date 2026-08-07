//! A flight recorder for the transition machinery.
//!
//! Off unless `MSTREAM_ENGINE_TRACE` names a file at startup; then every
//! decision the blend logic makes — prepares, failures with their reasons,
//! gates, handovers, seeks — appends one timestamped line. It exists
//! because the TUI silences stderr (see `stderrln!`), so a listening
//! session that goes wrong otherwise leaves nothing to read afterwards.
//! When the variable is unset the cost is one initialized-OnceLock load
//! per call site.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static SINK: OnceLock<Option<(Mutex<File>, Instant)>> = OnceLock::new();

pub(crate) fn line(args: std::fmt::Arguments) {
    let Some((file, t0)) = SINK
        .get_or_init(|| {
            let path = std::env::var_os("MSTREAM_ENGINE_TRACE")?;
            let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
            Some((Mutex::new(file), Instant::now()))
        })
        .as_ref()
    else {
        return;
    };
    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "[{:9.3}s] {}", t0.elapsed().as_secs_f64(), args);
    }
}

/// `etrace!("fmt", args…)` — one line into the flight recorder, or nothing.
macro_rules! etrace {
    ($($arg:tt)*) => {
        $crate::engine::trace::line(format_args!($($arg)*))
    };
}
pub(crate) use etrace;
