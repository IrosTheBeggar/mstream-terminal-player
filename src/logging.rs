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
//! The browser build keeps the level type and the switches — the Settings
//! tab that drives them is drawing code wasm compiles unchanged — and drops
//! the half that needs a filesystem. Nothing installs a subscriber there,
//! so Write log honestly answers "no file could be opened", and a browser's
//! own devtools console stays where its diagnostics live.

use std::fs::{self, File};
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
        match &mut *SINK.lock().unwrap() {
            Some(file) => file.write(buf),
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
#[cfg(not(target_arch = "wasm32"))]
fn open_sink(path: &Path) -> bool {
    let Ok(mut file) = File::create(path) else { return false };
    let _ = writeln!(
        file,
        "── mstream-player v{} — RUST_LOG overrides the level when set ──",
        env!("CARGO_PKG_VERSION")
    );
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

/// The filter the current switches mean. Silence when the write switch is
/// off; otherwise the level — unless `RUST_LOG` is set, which knows better.
#[cfg(not(target_arch = "wasm32"))]
fn filter_for(write: bool, level: Level) -> EnvFilter {
    if !write {
        return EnvFilter::new("off");
    }
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

    let (filter_layer, handle) = reload::Layer::new(filter_for(write, level));
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

    let path = path.filter(|p| open_sink(p));
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

/// Point the live subscriber at what the switches now say.
#[cfg(not(target_arch = "wasm32"))]
fn apply_filter(write: bool, level: Level) {
    if let Some(handle) = HANDLE.get() {
        let _ = handle.reload(filter_for(write, level));
    }
}

#[cfg(target_arch = "wasm32")]
fn apply_filter(_write: bool, _level: Level) {}

/// Unreachable in the browser build — `set_write` turns back at
/// [`subscriber_ready`] before any path is resolved — but the shared code
/// path names it, so it exists and refuses.
#[cfg(target_arch = "wasm32")]
fn open_sink(_path: &Path) -> bool {
    false
}

/// Flip whether anything is written, from the Settings tab. Turning it on
/// for the first time opens the file (the default location, rotated) if the
/// session started without one. Returns the file now in use when on.
pub fn set_write(on: bool) -> Option<PathBuf> {
    if !subscriber_ready() {
        return None;
    }
    let mut state = STATE.lock().unwrap();
    if on && state.path.is_none() {
        state.path = resolve_target(Target::Default).filter(|p| open_sink(p));
    }
    state.write = on && state.path.is_some();
    apply_filter(state.write, state.level);
    state.path.clone().filter(|_| state.write)
}

/// Change how loud the log is. Takes hold at once while writing; otherwise
/// it simply waits as the level the write switch will use.
pub fn set_level(level: Level) {
    let mut state = STATE.lock().unwrap();
    state.level = level;
    if state.write {
        apply_filter(true, level);
    }
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

/// The last `max` lines of the log, for the in-app viewer — read fresh each
/// call, from at most the final 256 KiB of the file.
pub fn tail(max: usize) -> Option<(PathBuf, Vec<String>)> {
    let path = STATE.lock().unwrap().path.clone()?;
    let text = read_tail(&path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if lines.len() > max {
        lines.drain(..lines.len() - max);
    }
    Some((path, lines))
}

fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const WINDOW: u64 = 256 * 1024;
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > WINDOW {
        file.seek(SeekFrom::Start(len - WINDOW)).ok()?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
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

    #[test]
    fn the_tail_is_the_end_of_the_file_and_no_more_than_asked() {
        let path = std::env::temp_dir().join("mstream-player-test-tail.log");
        let body: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        fs::write(&path, body).unwrap();

        let text = read_tail(&path).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 40);
        lines.drain(..lines.len() - 5);
        assert_eq!(lines, ["line 36", "line 37", "line 38", "line 39", "line 40"]);
        let _ = fs::remove_file(&path);
    }
}
