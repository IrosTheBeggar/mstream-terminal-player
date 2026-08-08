//! The optional debug log: everything the dependencies narrate, in a file.
//!
//! iroh, reqwest, hyper and stream-download all speak `tracing` — relay
//! connects, holepunch attempts, path upgrades, reconnects — and without a
//! subscriber every word of it is discarded. This installs one at startup,
//! writing to a file and never to the terminal: the TUI's screen stays a
//! screen, and serve mode's stderr stays the concise log it already is.
//!
//! The level can change while the player runs — the Settings tab has a Logs
//! room — so the subscriber is always installed, wearing a reloadable filter
//! that costs nothing while it says `off`, over a writer whose file can
//! arrive later than the subscriber did.
//!
//! Ways in, strongest first:
//!
//!   MSTREAM_LOG=/path/to/file    → exactly that file, truncated
//!   MSTREAM_LOG=1                 → the default location, rotated
//!   [log] level = "debug"         → config.toml, written by the Settings tab
//!
//! With only a level (config or Settings), the file is the default location:
//! `<cache>/logs/mstream-player.log`, the last few runs kept beside it as
//! `.1` through `.4`. `RUST_LOG` chooses *what* is said when set (the usual
//! filter grammar); otherwise the level speaks for everything.
//!
//! This is the third-party channel. The player's own transition decisions
//! have their own recorder (`MSTREAM_ENGINE_TRACE`, engine::trace), which
//! also collects the `stderrln!` diagnostics the TUI would otherwise
//! silence — the two files answer different questions and stay separate.
//!
//! Two destinations, one capture. Every event the level admits is kept in a
//! bounded in-memory ring — which is what **View log** reads, so a session
//! can be inspected without writing a byte to disk — and, when **Write log**
//! is on, the same formatted line also goes to the file. The level therefore
//! governs both: it is what gets captured at all, not merely what gets
//! persisted. The ring dies with the process; the file is what survives to
//! be sent to someone.
//!
//! The browser build keeps the level type and the switches — the Settings
//! tab that drives them is drawing code wasm compiles unchanged — and drops
//! the half that needs a filesystem. Nothing installs a subscriber there,
//! so Write log honestly answers "no file could be opened", and a browser's
//! own devtools console stays where its diagnostics live.

use std::collections::VecDeque;
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber::layer::SubscriberExt;
#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber::util::SubscriberInitExt;
#[cfg(not(target_arch = "wasm32"))]
use tracing_subscriber::{EnvFilter, Registry, reload};

/// How many rotated predecessors the default location keeps.
const KEEP: usize = 4;

/// How loud the log is, once something is being written at all — whether
/// anything is written belongs to the separate write switch. Ordered so a
/// step up says more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Default)]
pub enum Level {
    #[default]
    Info,
    Debug,
    Trace,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Info, Level::Debug, Level::Trace];

    /// The word shown in the Settings row and written to config.toml.
    pub fn label(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }

    pub fn parse(text: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.label() == text.trim().to_ascii_lowercase())
    }

    /// One step louder or quieter, clamped at the ends.
    pub fn step(self, delta: i32) -> Level {
        let here = Level::ALL.iter().position(|l| *l == self).unwrap_or(0) as i32;
        let there = (here + delta).clamp(0, Level::ALL.len() as i32 - 1);
        Level::ALL[there as usize]
    }
}

/// How many formatted lines the ring holds. Deep enough to scroll a real
/// session in the viewer, shallow enough that holding it costs a few
/// hundred kilobytes at worst.
const RING_LINES: usize = 2_000;

/// A guard on the one pathological input: an event whose formatted line
/// never ends. Past this the pending line is flushed as-is rather than
/// growing until memory complains.
const RING_LINE_CAP: usize = 64 * 1024;

/// Everything the level admitted this session, most recent last — the
/// viewer's source, and the reason a log can be read without being written.
static RING: LazyLock<Mutex<Ring>> = LazyLock::new(|| Mutex::new(Ring::default()));

#[derive(Default)]
struct Ring {
    lines: VecDeque<String>,
    /// The tail of a line the writer has not finished handing over: the fmt
    /// layer may deliver one event in several writes, and half a line in
    /// the viewer would be a lie about what was logged.
    pending: String,
}

impl Ring {
    fn push_bytes(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.pending);
                self.push_line(line);
            } else {
                self.pending.push(ch);
                if self.pending.len() >= RING_LINE_CAP {
                    let line = std::mem::take(&mut self.pending);
                    self.push_line(line);
                }
            }
        }
    }

    fn push_line(&mut self, line: String) {
        if self.lines.len() == RING_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

/// The last `max` lines the ring holds, oldest first.
pub fn tail(max: usize) -> Vec<String> {
    let ring = RING.lock().unwrap();
    let skip = ring.lines.len().saturating_sub(max);
    ring.lines.iter().skip(skip).cloned().collect()
}

/// How many lines have been captured this session.
pub fn captured() -> usize {
    RING.lock().unwrap().lines.len()
}

/// The file events land in, arriving whenever logging is first turned on.
/// The subscriber is installed before any file may exist, so its writer
/// looks here on every event; empty means the words go nowhere.
#[cfg(not(target_arch = "wasm32"))]
static SINK: Mutex<Option<File>> = Mutex::new(None);

#[cfg(not(target_arch = "wasm32"))]
struct LateWriter;

#[cfg(not(target_arch = "wasm32"))]
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LateWriter {
    type Writer = LateGuard;
    fn make_writer(&'a self) -> LateGuard {
        LateGuard
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct LateGuard;

#[cfg(not(target_arch = "wasm32"))]
impl Write for LateGuard {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // The ring first and always: it is what the viewer reads, and it
        // must hold a session whether or not a file is being written.
        RING.lock().unwrap().push_bytes(&String::from_utf8_lossy(buf));
        match &mut *SINK.lock().unwrap() {
            Some(file) => file.write(buf),
            // Nothing on disk to take it, and nothing wrong with that.
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match &mut *SINK.lock().unwrap() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// The handle that swaps the filter while the player runs, and the state
/// the Settings rows read back: whether we are writing, how loud, and to
/// which file (which outlives turning writing off, so the viewer can still
/// show what was captured).
#[cfg(not(target_arch = "wasm32"))]
static HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static STATE: Mutex<State> = Mutex::new(State { write: false, level: Level::Info, path: None });

struct State {
    write: bool,
    level: Level,
    path: Option<PathBuf>,
}

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

/// Resolve a target to a path, creating directories and rotating as asked.
fn resolve_target(target: Target) -> Option<PathBuf> {
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

/// Truncate `path` into a fresh signed file and point the writer at it.
///
/// `backlog` is what the ring already holds — poured in first, because the
/// reason someone turns writing on is usually the thing they just watched
/// happen. Without it the file would begin at the keystroke and miss
/// exactly that.
#[cfg(not(target_arch = "wasm32"))]
fn open_sink(path: &Path, backlog: &[String]) -> bool {
    let Ok(mut file) = File::create(path) else { return false };
    let _ = writeln!(
        file,
        "── mstream-player v{} — RUST_LOG overrides the level when set ──",
        env!("CARGO_PKG_VERSION")
    );
    if !backlog.is_empty() {
        let _ = writeln!(file, "── {} lines captured before writing began ──", backlog.len());
        for line in backlog {
            let _ = writeln!(file, "{line}");
        }
    }
    *SINK.lock().unwrap() = Some(file);
    true
}

/// What a config's `[log]` section means. The write key is young; configs
/// from the one-field days said everything with `level` alone, where a
/// parseable level meant writing and "off" meant not — honoured here so an
/// upgrade never silently changes what a session logs.
fn settled_from_config(write: Option<bool>, level_text: &str) -> (bool, Level) {
    let level = Level::parse(level_text).unwrap_or_default();
    let write = write.unwrap_or_else(|| Level::parse(level_text).is_some());
    (write, level)
}

/// What the level admits — unless `RUST_LOG` is set, which knows better.
/// Note this does not consult the write switch: capture feeds the ring, and
/// the ring is readable whether or not anything reaches a file.
#[cfg(not(target_arch = "wasm32"))]
fn filter_for(level: Level) -> EnvFilter {
    if let Ok(custom) = EnvFilter::try_from_default_env() {
        return custom;
    }
    EnvFilter::new(level.label())
}

/// Install the subscriber and apply whatever the environment and config ask
/// for. Call once, first thing in main — anything that dials before this
/// speaks into the void. Returns the active path, for the boot line.
#[cfg(not(target_arch = "wasm32"))]
pub fn init() -> Option<PathBuf> {
    let env_target = wants(std::env::var("MSTREAM_LOG").ok().as_deref());
    let (config_write, level) = crate::config::load()
        .ok()
        .map(|c| settled_from_config(c.log.write, &c.log.level))
        .unwrap_or((false, Level::Info));

    // Environment outranks config; a bare config switch gets the default
    // location the same as MSTREAM_LOG=1 would.
    let (write, path) = match env_target {
        Target::Off if !config_write => (false, None),
        Target::Off => (true, resolve_target(Target::Default)),
        target => (true, resolve_target(target)),
    };

    // Installed at the level, not at the write switch: the ring starts
    // filling from boot, so a session that goes wrong before anyone thought
    // to turn writing on can still be read.
    let (filter_layer, handle) = reload::Layer::new(filter_for(level));
    let installed = tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(LateWriter)
                .with_ansi(false),
        )
        .try_init()
        .is_ok();
    if !installed {
        return None;
    }
    let _ = HANDLE.set(handle);

    // Nothing has been captured yet at boot, so the file opens empty.
    let path = path.filter(|p| open_sink(p, &[]));
    let write = write && path.is_some();
    *STATE.lock().unwrap() = State { write, level, path: path.clone() };
    path.filter(|_| write)
}

/// Whether a subscriber exists to hear anything at all. False in the
/// browser build (no filesystem to write to), and in unit tests and
/// embeddings where `init` never ran — a file opened for either would
/// receive nothing, so the switch honestly refuses rather than littering.
#[cfg(not(target_arch = "wasm32"))]
fn subscriber_ready() -> bool {
    HANDLE.get().is_some()
}

#[cfg(target_arch = "wasm32")]
fn subscriber_ready() -> bool {
    false
}

/// Let go of the file, leaving the ring capturing.
#[cfg(not(target_arch = "wasm32"))]
fn close_sink() {
    *SINK.lock().unwrap() = None;
}

#[cfg(target_arch = "wasm32")]
fn close_sink() {}

/// Point the live subscriber at the level now chosen.
#[cfg(not(target_arch = "wasm32"))]
fn apply_filter(level: Level) {
    if let Some(handle) = HANDLE.get() {
        let _ = handle.reload(filter_for(level));
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_filter(_level: Level) {}

/// Unreachable in the browser build — `set_write` turns back at
/// [`subscriber_ready`] before any path is resolved — but the shared code
/// path names it, so it exists and refuses.
#[cfg(target_arch = "wasm32")]
fn open_sink(_path: &Path, _backlog: &[String]) -> bool {
    false
}

/// Flip whether the captured log is also *written*, from the Settings tab.
/// Turning it on for the first time opens the file (the default location,
/// rotated) if the session started without one; turning it off lets go of
/// the file and leaves the ring filling. Returns the file now in use when
/// on.
pub fn set_write(on: bool) -> Option<PathBuf> {
    if !subscriber_ready() {
        return None;
    }
    // The ring's lock is taken and released before the sink's: the writer
    // takes them in that order too, and one order is what makes it safe.
    let backlog = if on { tail(usize::MAX) } else { Vec::new() };
    let mut state = STATE.lock().unwrap();
    if on && state.path.is_none() {
        state.path = resolve_target(Target::Default).filter(|p| open_sink(p, &backlog));
    } else if on {
        // A file this session already opened and let go of: re-point the
        // writer at it, carrying over what has been captured since.
        if let Some(path) = state.path.clone() {
            state.path = Some(path.clone()).filter(|p| open_sink(p, &backlog));
        }
    }
    state.write = on && state.path.is_some();
    // Turning writing off leaves the filter alone: the ring goes on
    // holding the session, which is the whole point of being able to read
    // a log without keeping one.
    if !state.write {
        close_sink();
    }
    state.path.clone().filter(|_| state.write)
}

/// Change how loud the log is. Takes hold at once, written or not — the
/// level is what the ring captures as much as what the file receives.
pub fn set_level(level: Level) {
    let mut state = STATE.lock().unwrap();
    state.level = level;
    // Always: the level is what the ring captures as much as what the file
    // receives.
    apply_filter(level);
}

/// Whether anything is being written right now.
pub fn writing() -> bool {
    STATE.lock().unwrap().write
}

/// How loud the log is set to be, written or not.
pub fn level() -> Level {
    STATE.lock().unwrap().level
}

/// Where the log is going, if anywhere — for the boot and goodbye lines.
pub fn active() -> Option<PathBuf> {
    let state = STATE.lock().unwrap();
    if state.write { state.path.clone() } else { None }
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
    fn levels_step_between_their_words_and_stop_at_the_ends() {
        assert_eq!(Level::Info.step(1), Level::Debug);
        assert_eq!(Level::Debug.step(1), Level::Trace);
        assert_eq!(Level::Trace.step(1), Level::Trace);
        assert_eq!(Level::Debug.step(-1), Level::Info);
        assert_eq!(Level::Info.step(-1), Level::Info);
        assert_eq!(Level::default(), Level::Info);
        for level in Level::ALL {
            assert_eq!(Level::parse(level.label()), Some(level));
        }
        assert_eq!(Level::parse("TRACE "), Some(Level::Trace));
        // "off" was a level in the one-field days; it is not one now — the
        // migration reads it as the write switch being off.
        assert_eq!(Level::parse("off"), None);
        assert_eq!(Level::parse("loud"), None);
    }

    #[test]
    fn old_one_field_configs_keep_their_meaning() {
        // A set level meant writing at that level…
        assert_eq!(settled_from_config(None, "trace"), (true, Level::Trace));
        // …"off" and absence meant silence…
        assert_eq!(settled_from_config(None, "off"), (false, Level::Info));
        assert_eq!(settled_from_config(None, ""), (false, Level::Info));
        // …and the new key speaks for itself, whatever the level says.
        assert_eq!(settled_from_config(Some(false), "debug"), (false, Level::Debug));
        assert_eq!(settled_from_config(Some(true), ""), (true, Level::Info));
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

    /// The ring is what the viewer reads, so it has to hold whole lines,
    /// hold the *recent* ones, and never grow without bound.
    #[test]
    fn the_ring_keeps_whole_recent_lines_and_forgets_the_rest() {
        let mut ring = Ring::default();

        // The fmt layer may hand one event over in several writes; a line
        // is only a line once its newline arrives.
        ring.push_bytes("first half");
        assert!(ring.lines.is_empty(), "half a line is not a line yet");
        ring.push_bytes(" and second half\n");
        assert_eq!(ring.lines, ["first half and second half"]);

        // Several lines in one write land as several lines.
        ring.push_bytes("two\nthree\n");
        assert_eq!(ring.lines.len(), 3);

        // Past its depth the ring drops the oldest and keeps the newest.
        for n in 0..RING_LINES {
            ring.push_bytes(&format!("line {n}\n"));
        }
        assert_eq!(ring.lines.len(), RING_LINES);
        assert_eq!(ring.lines.back().unwrap(), &format!("line {}", RING_LINES - 1));
        assert!(!ring.lines.contains(&"first half and second half".to_string()));

        // A line that never ends is flushed rather than eating memory.
        let mut ring = Ring::default();
        ring.push_bytes(&"x".repeat(RING_LINE_CAP + 10));
        assert_eq!(ring.lines.len(), 1);
        assert!(ring.pending.len() < RING_LINE_CAP);
    }

    #[test]
    fn the_tail_reads_the_end_of_the_ring_and_no_more_than_asked() {
        let mut ring = RING.lock().unwrap();
        ring.lines.clear();
        ring.pending.clear();
        for n in 1..=40 {
            ring.push_bytes(&format!("line {n}\n"));
        }
        drop(ring);

        assert_eq!(tail(5), ["line 36", "line 37", "line 38", "line 39", "line 40"]);
        assert_eq!(captured(), 40);
        // Asking for more than exists is not an error, just everything.
        assert_eq!(tail(500).len(), 40);

        let mut ring = RING.lock().unwrap();
        ring.lines.clear();
    }
}
