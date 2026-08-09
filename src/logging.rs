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
/// session in the viewer.
const RING_LINES: usize = 2_000;

/// And how much memory those lines may occupy. The line count alone was not
/// a ceiling: 2 000 lines at the [`RING_LINE_CAP`] limit is 128 MiB, which
/// is not the "few hundred kilobytes" the count implies (PR #5 review).
/// Whichever bound is reached first evicts the oldest.
const RING_BYTES: usize = 2 * 1024 * 1024;

/// A guard on the one pathological input: an event whose formatted line
/// never ends. Past this the pending line is flushed as-is rather than
/// growing until memory complains.
const RING_LINE_CAP: usize = 64 * 1024;

/// Everything the level admitted this session, most recent last — the
/// viewer's source, and the reason a log can be read without being written.
static RING: LazyLock<Mutex<Ring>> = LazyLock::new(|| Mutex::new(Ring::default()));

#[derive(Default)]
struct Ring {
    /// How many lines have ever been pushed — not how many are held. It is
    /// what lets writing resume without either losing the stretch captured
    /// while it was off or repeating the stretch already in the file.
    total: u64,
    /// What the held lines weigh, kept as they come and go so the ceiling
    /// costs no walk.
    bytes: usize,
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
        self.bytes += line.len();
        self.lines.push_back(line);
        self.total += 1;
        // Never below one line: a single event longer than the whole budget
        // should cost the ring its history, not its last word.
        while self.lines.len() > 1 && (self.lines.len() > RING_LINES || self.bytes > RING_BYTES)
        {
            match self.lines.pop_front() {
                Some(gone) => self.bytes -= gone.len(),
                None => break,
            }
        }
    }

    /// The last `max` lines, oldest first — the shape the viewer reads.
    fn tail(&self, max: usize) -> Vec<String> {
        let skip = self.lines.len().saturating_sub(max);
        self.lines.iter().skip(skip).cloned().collect()
    }

    /// The lines pushed at or after `mark` that the ring still holds. A
    /// mark older than the ring's memory yields everything it has, which is
    /// the honest answer: what fell out is gone.
    fn since(&self, mark: u64) -> Vec<String> {
        let oldest = self.total - self.lines.len() as u64;
        let skip = mark.saturating_sub(oldest) as usize;
        self.lines.iter().skip(skip).cloned().collect()
    }
}

/// Take the secrets out of a formatted log line.
///
/// The player's own lines are careful (`http::redact_source`), but most of
/// what is captured belongs to other crates: stream-download names the URL
/// it is fetching in a span field, reqwest and hyper log request URLs at
/// debug — and an mStream media URL carries `?token=<jwt>`. A log the
/// README invites people to attach to a bug report must not be a way to
/// hand a stranger a working credential, so the scrub happens where both
/// destinations meet rather than at each crate's call site.
///
/// Two shapes go: a query string (everything from `?` to the next
/// delimiter) and userinfo in a URL (`//user:pass@host`, which is how a
/// proxy's credentials would arrive). The cost is a few characters of any
/// prose that happens to contain a question mark, which is the right price.
fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['?', '@']) {
        let (head, tail) = rest.split_at(at);
        if tail.starts_with('?') {
            out.push_str(head);
            out.push_str("?<redacted>");
            let after = &tail[1..];
            let end = after.find([' ', '"', '\'', ')', ',', '\\']).unwrap_or(after.len());
            rest = &after[end..];
            continue;
        }
        // An '@': redact backwards over the userinfo when this is a URL's
        // authority, and otherwise leave the character alone (an '@' in
        // ordinary prose is not a secret).
        match head.rfind("//") {
            // Everything between the `//` and the `@` is userinfo only if
            // nothing has ended the authority in between.
            Some(slashes) if !head[slashes + 2..].contains([' ', '/']) => {
                out.push_str(&head[..slashes + 2]);
                out.push_str("<redacted>@");
            }
            _ => {
                out.push_str(head);
                out.push('@');
            }
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// The last `max` lines the ring holds, oldest first.
pub fn tail(max: usize) -> Vec<String> {
    RING.lock().unwrap().tail(max)
}

/// How many lines have been captured this session.
pub fn captured() -> usize {
    RING.lock().unwrap().lines.len()
}

/// How many lines have ever been captured — the mark writing resumes from.
fn ring_total() -> u64 {
    RING.lock().unwrap().total
}

/// What the ring has held since `mark`.
fn ring_since(mark: u64) -> Vec<String> {
    RING.lock().unwrap().since(mark)
}

/// The file events land in, arriving whenever logging is first turned on.
/// The subscriber is installed before any file may exist, so its writer
/// looks here on every event; empty means the words go nowhere.
#[cfg(not(target_arch = "wasm32"))]
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// How large one run's file may grow before it is rolled aside. Rotation
/// used to happen only at open, which bounded the number of runs kept and
/// never the size of one: a serve instance left writing for a week grew a
/// single file without limit (PR #5 review).
#[cfg(not(target_arch = "wasm32"))]
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The open log file, and enough about it to roll it without reaching for
/// another lock. Keeping the path here rather than in [`STATE`] is what
/// keeps the writer's lock order (RING, then SINK) free of a third.
#[cfg(not(target_arch = "wasm32"))]
struct Sink {
    file: File,
    path: PathBuf,
    written: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl Sink {
    /// Write a line's worth of bytes, rolling the file aside first if it
    /// has grown past its bound. The roll keeps one predecessor — this
    /// process's own `.1`, never another's — so a long session costs at
    /// most twice the bound and loses only its distant past.
    fn write_rolling(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.written + bytes.len() as u64 > MAX_FILE_BYTES {
            let previous = self.path.with_extension("log.1");
            let _ = fs::rename(&self.path, &previous);
            match create_private(&self.path) {
                Ok(fresh) => {
                    self.file = fresh;
                    self.written = 0;
                    let _ = writeln!(
                        self.file,
                        "── rolled at {} MiB; the stretch before this is in {} ──",
                        MAX_FILE_BYTES / (1024 * 1024),
                        previous.display()
                    );
                }
                // Could not open a fresh one: keep writing to the file we
                // have (now named .1) rather than losing the session.
                Err(_) => {
                    let _ = fs::rename(&previous, &self.path);
                }
            }
        }
        self.written += bytes.len() as u64;
        self.file.write_all(bytes)
    }
}

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
        // Scrubbed once, here, before either destination sees it. This is
        // the only place both roads pass through, and the alternative —
        // trusting every crate in the tree to keep secrets out of its own
        // log lines — is not a discipline anyone can hold. See [`scrub`].
        let text = scrub(&String::from_utf8_lossy(buf));
        // The ring first and always: it is what the viewer reads, and it
        // must hold a session whether or not a file is being written.
        RING.lock().unwrap().push_bytes(&text);
        if let Some(sink) = &mut *SINK.lock().unwrap() {
            // write_all, not write: a short write would otherwise leave the
            // caller to retry a remainder the ring has already taken.
            sink.write_rolling(text.as_bytes())?;
        }
        // Every byte of `buf` was accounted for, whatever the scrubbed
        // length turned out to be.
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match &mut *SINK.lock().unwrap() {
            Some(sink) => sink.file.flush(),
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
static STATE: Mutex<State> =
    Mutex::new(State { write: false, level: Level::Info, path: None, gap_mark: None });

struct State {
    write: bool,
    level: Level,
    path: Option<PathBuf>,
    /// Where the file stopped keeping up: the ring's total at the moment
    /// writing was switched off. Resuming pours exactly the lines captured
    /// since, so nothing is lost and nothing is written twice.
    gap_mark: Option<u64>,
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

/// The prefix every default-location log wears, so the sweep can tell ours
/// from anything else that lands in the directory.
const LOG_PREFIX: &str = "mstream-player-";

/// A file another process may still be writing to. Nothing this recently
/// touched is swept, however far down the list it sits: an idle player's
/// log is not litter.
const SWEEP_GRACE: std::time::Duration = std::time::Duration::from_secs(600);

/// This run's own file name. Every process writes to its own, which is what
/// makes the default location safe to share.
///
/// The rename chain that used to live here (`log` → `log.1` → …) assumed
/// one player at a time. A second one renamed the first's open file out
/// from under it — the first went on writing to a path that no longer
/// existed, the UI kept naming the old one, and a few more starts unlinked
/// it altogether (PR #5 review). Nobody renames anybody now.
fn default_log_name() -> String {
    format!("{LOG_PREFIX}{}.log", std::process::id())
}

/// Keep the directory to the last few runs by deleting the oldest, rather
/// than by shuffling names around. Files touched within [`SWEEP_GRACE`] are
/// never candidates, so a running player's log survives however many other
/// players come and go.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut ours: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.file_name().to_str().is_some_and(|n| n.starts_with(LOG_PREFIX))
                && e.file_type().map(|t| t.is_file()).unwrap_or(false)
        })
        .filter_map(|e| {
            let when = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((when, e.path()))
        })
        .collect();
    // Newest first, so everything past KEEP is a candidate.
    ours.sort_by(|a, b| b.0.cmp(&a.0));
    let now = std::time::SystemTime::now();
    for (when, path) in ours.into_iter().skip(KEEP) {
        let idle = now.duration_since(when).unwrap_or_default();
        if idle > SWEEP_GRACE {
            let _ = fs::remove_file(path);
        }
    }
}

/// Resolve a target to a path, creating directories and sweeping as asked.
fn resolve_target(target: Target) -> Option<PathBuf> {
    match target {
        Target::Off => None,
        Target::Default => {
            let dir = crate::config::log_dir()?;
            fs::create_dir_all(&dir).ok()?;
            sweep(&dir);
            Some(dir.join(default_log_name()))
        }
        Target::Path(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent).ok()?;
            }
            Some(path)
        }
    }
}

/// Create (or truncate) a log file only its owner can read.
///
/// The scrub in [`scrub`] is the first defence and the load-bearing one,
/// but a diagnostic log is still a record of what someone listened to and
/// when, and this machine may not be theirs alone. `credentials.toml` is
/// owner-only for the same reason; on Windows the inherited ACL of a
/// per-user cache directory is the equivalent.
#[cfg(not(target_arch = "wasm32"))]
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Whether a file is being started or picked back up.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq)]
enum Opening {
    /// A file this session has not written to: truncate and sign it.
    Fresh,
    /// A file this session already wrote and let go of. It is opened for
    /// APPEND — `File::create` here would truncate away everything already
    /// written, which is exactly what turning the switch off and on again
    /// used to do (PR #5 review).
    Resume,
}

/// Point the writer at `path`, pouring `backlog` in behind it.
///
/// On a fresh file the backlog is what the ring already holds, because the
/// reason someone turns writing on is usually the thing they just watched
/// happen; without it the file would begin at the keystroke and miss
/// exactly that. On a resume the backlog is only the stretch captured while
/// writing was off — the rest is already in the file.
#[cfg(not(target_arch = "wasm32"))]
fn open_sink(path: &Path, backlog: &[String], how: Opening) -> bool {
    let opened = match how {
        Opening::Fresh => create_private(path),
        Opening::Resume => fs::OpenOptions::new().append(true).create(true).open(path),
    };
    let Ok(mut file) = opened else { return false };
    match how {
        Opening::Fresh => {
            let _ = writeln!(
                file,
                "── mstream-player v{} — RUST_LOG overrides the level when set ──",
                env!("CARGO_PKG_VERSION")
            );
            if !backlog.is_empty() {
                let _ =
                    writeln!(file, "── {} lines captured before writing began ──", backlog.len());
            }
        }
        Opening::Resume => {
            let _ = writeln!(
                file,
                "── writing resumed — {} lines captured while it was off ──",
                backlog.len()
            );
        }
    }
    for line in backlog {
        let _ = writeln!(file, "{line}");
    }
    let written = file.metadata().map(|m| m.len()).unwrap_or(0);
    *SINK.lock().unwrap() = Some(Sink { file, path: path.to_path_buf(), written });
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

/// Which file, if any, this run should write to.
///
/// Split out from [`init`] because it is the part worth testing: `init`
/// installs a process-global subscriber and can only run once.
fn decide(env_target: Target, config_write: bool, run: Run) -> Option<Target> {
    match (env_target, run) {
        // Asking for a log by name works everywhere, one-shots included.
        (Target::Path(p), _) => Some(Target::Path(p)),
        (Target::Default, _) => Some(Target::Default),
        // A config switch is the player's own setting; a one-shot command
        // has no session to log and must not disturb the directory.
        (Target::Off, Run::Session) if config_write => Some(Target::Default),
        (Target::Off, _) => None,
    }
}

/// Install the subscriber and apply whatever the environment and config ask
/// for. Call once, first thing in main — anything that dials before this
/// speaks into the void. Returns the active path, for the boot line.
/// What kind of run is starting. A player or a jukebox is a session worth
/// a file of its own; `keys`, `ls` and the rest are one-shots that answer
/// and exit.
#[derive(Clone, Copy, PartialEq)]
pub enum Run {
    Session,
    OneShot,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn init(run: Run) -> Option<PathBuf> {
    let env_target = wants(std::env::var("MSTREAM_LOG").ok().as_deref());
    let (config_write, level) = crate::config::load()
        .ok()
        .map(|c| settled_from_config(c.log.write, &c.log.level))
        .unwrap_or((false, Level::Info));

    // Environment outranks config; a bare config switch gets the default
    // location the same as MSTREAM_LOG=1 would.
    //
    // Except for a one-shot, where the config switch is ignored. `[log]
    // write = true` is set in the player's Settings and means "keep a log
    // of my listening"; honouring it in `mstream-player keys` meant every
    // such invocation opened, swept and rotated the default location, so a
    // handful of them deleted the session someone was trying to keep
    // (PR #5 review). An explicit MSTREAM_LOG still works everywhere —
    // asking for a log by name is asking for it here too.
    let path = decide(env_target, config_write, run).and_then(resolve_target);
    let write = path.is_some();

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
    let path = path.filter(|p| open_sink(p, &[], Opening::Fresh));
    let write = write && path.is_some();
    // A file opened at boot is already up to date with the ring; one that
    // was not opened owes everything, which is what a None mark means to
    // the first turn-on.
    *STATE.lock().unwrap() = State { write, level, path: path.clone(), gap_mark: None };
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
fn open_sink(_path: &Path, _backlog: &[String], _how: Opening) -> bool {
    false
}

/// The browser build never opens anything, but the shared setter names the
/// distinction, so the type exists there too.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, PartialEq)]
enum Opening {
    Fresh,
    Resume,
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
    // The ring's locks are taken and released before the sink's: the writer
    // takes them in that order too, and one order is what makes it safe.
    let mut state = STATE.lock().unwrap();
    if on {
        match state.path.clone() {
            // A path means this session already has a file. Whatever the
            // marks say, it is picked back up rather than started over:
            // truncating a session's own log is never the right answer.
            Some(path) => {
                let missed = state.gap_mark.map(ring_since).unwrap_or_default();
                state.path = Some(path).filter(|p| open_sink(p, &missed, Opening::Resume));
            }
            // The first time: a new file, carrying everything so far.
            None => {
                let backlog = tail(usize::MAX);
                state.path = resolve_target(Target::Default)
                    .filter(|p| open_sink(p, &backlog, Opening::Fresh));
            }
        }
        state.gap_mark = None;
    }
    state.write = on && state.path.is_some();
    // Turning writing off leaves the filter alone: the ring goes on
    // holding the session, which is the whole point of being able to read
    // a log without keeping one. The mark is where the file stopped
    // keeping up, so resuming knows exactly what it owes.
    if !state.write {
        close_sink();
        state.gap_mark = Some(ring_total());
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

    /// `mstream-player keys` has nothing to log, and used to open, sweep
    /// and rotate the default location anyway — a few invocations deleted
    /// the session someone was keeping (PR #5 review).
    #[test]
    fn a_one_shot_command_leaves_the_session_log_alone() {
        // The config switch belongs to the player, not to `keys`.
        assert_eq!(decide(Target::Off, true, Run::OneShot), None);
        assert_eq!(decide(Target::Off, true, Run::Session), Some(Target::Default));
        assert_eq!(decide(Target::Off, false, Run::Session), None);

        // But asking for a log by name works from anywhere.
        let named = PathBuf::from("/tmp/asked-for.log");
        assert_eq!(
            decide(Target::Path(named.clone()), false, Run::OneShot),
            Some(Target::Path(named))
        );
        assert_eq!(decide(Target::Default, false, Run::OneShot), Some(Target::Default));
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

    /// The sweep bounds how many runs the directory keeps — by deleting
    /// the oldest, never by renaming anyone's open file.
    #[test]
    fn the_sweep_keeps_the_recent_runs_and_spares_the_living() {
        let dir = std::env::temp_dir().join("mstream-player-test-logsweep");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Six old runs and one file that is not ours.
        let old = std::time::SystemTime::now() - SWEEP_GRACE * 2;
        let mut names = Vec::new();
        for pid in 1..=6 {
            let name = format!("{LOG_PREFIX}{pid}.log");
            let path = dir.join(&name);
            fs::write(&path, &name).unwrap();
            filetime_set(&path, old);
            names.push(name);
        }
        fs::write(dir.join("notes.txt"), "not ours").unwrap();

        sweep(&dir);

        let left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            left.iter().filter(|n| n.starts_with(LOG_PREFIX)).count(),
            KEEP,
            "the oldest beyond KEEP are gone: {left:?}"
        );
        assert!(left.contains(&"notes.txt".to_string()), "and nothing else is touched");

        // A file touched just now is spared however far down the list it
        // sits — that is a player still running.
        let fresh = dir.join(format!("{LOG_PREFIX}999.log"));
        fs::write(&fresh, "live").unwrap();
        for pid in 10..=20 {
            let path = dir.join(format!("{LOG_PREFIX}{pid}.log"));
            fs::write(&path, "old").unwrap();
            filetime_set(&path, old);
        }
        sweep(&dir);
        assert!(fresh.exists(), "a live player's log survives the sweep");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Backdate a file so the sweep sees it as an old run.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(when).unwrap();
    }

    /// A long session must not grow one file without end.
    #[test]
    fn a_file_past_its_bound_rolls_aside_and_keeps_going() {
        let dir = std::env::temp_dir().join("mstream-player-test-logroll");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.log");

        // Just enough room left for one more line, and not for the next.
        let file = create_private(&path).unwrap();
        let mut sink = Sink { file, path: path.clone(), written: MAX_FILE_BYTES - 100 };
        sink.write_rolling(b"the last of the old file\n").unwrap();
        sink.write_rolling(&[b'x'; 200]).unwrap();
        sink.write_rolling(b"and the first of the new\n").unwrap();
        drop(sink);

        let rolled = path.with_extension("log.1");
        assert!(rolled.exists(), "the predecessor is kept beside it");
        let current = fs::read_to_string(&path).unwrap();
        assert!(current.contains("rolled at"), "the new file says why: {current}");
        assert!(current.contains("and the first of the new"));
        assert!(!current.contains("the last of the old"), "that line is in the predecessor");
        assert!(fs::read_to_string(&rolled).unwrap().contains("the last of the old"));

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

    /// The log is a thing people attach to bug reports. Nothing in it may
    /// be a working credential — and the lines that carry them come from
    /// other crates, not ours.
    #[test]
    fn a_captured_line_never_carries_a_token_or_a_password() {
        // stream-download's span field, the exact shape that leaked.
        let line = r#"DEBUG new{url="https://host/media/a.mp3?token=eyJhbGciOi"}: requesting"#;
        let scrubbed = scrub(line);
        assert!(!scrubbed.contains("eyJhbGciOi"), "{scrubbed}");
        assert!(scrubbed.contains("a.mp3?<redacted>"), "{scrubbed}");
        assert!(scrubbed.contains("requesting"), "the rest of the line survives: {scrubbed}");

        // A proxy's credentials arrive as userinfo.
        assert_eq!(
            scrub("connecting via http://alice:hunter2@proxy.corp:8080"),
            "connecting via http://<redacted>@proxy.corp:8080"
        );
        // Several on one line, and a token that ends at a quote or a comma.
        let many = scrub(r#"a=http://h/x?token=A, b=http://h/y?token=B"#);
        assert!(!many.contains('A') || !many.contains("token"), "{many}");
        assert_eq!(many, "a=http://h/x?<redacted>, b=http://h/y?<redacted>");

        // Ordinary prose keeps its punctuation: an '@' that is not a URL's
        // authority, and a '?' that is not a query.
        assert_eq!(scrub("ask sherika@example about it"), "ask sherika@example about it");
        assert_eq!(scrub("what now?"), "what now?<redacted>");
        assert_eq!(scrub("no secrets here"), "no secrets here");
    }

    /// Writing off and on again must not cost the file its history — and
    /// must not write the same stretch twice.
    #[test]
    fn resuming_a_file_appends_the_gap_and_keeps_what_was_there() {
        let dir = std::env::temp_dir().join("mstream-player-test-resume");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.log");

        // A first stretch, written.
        assert!(open_sink(&path, &["one".into(), "two".into()], Opening::Fresh));
        *SINK.lock().unwrap() = None;
        let after_first = fs::read_to_string(&path).unwrap();
        assert!(after_first.contains("one") && after_first.contains("two"));

        // Writing was off for a while; the ring kept "three".
        assert!(open_sink(&path, &["three".into()], Opening::Resume));
        *SINK.lock().unwrap() = None;
        let after_resume = fs::read_to_string(&path).unwrap();
        assert!(after_resume.contains("one"), "the earlier stretch survives: {after_resume}");
        assert!(after_resume.contains("three"), "the gap is poured in: {after_resume}");
        assert_eq!(after_resume.matches("two").count(), 1, "and nothing is doubled");
        assert!(after_resume.contains("writing resumed"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// The mark is what makes the gap exact: everything captured while the
    /// file was closed, and nothing that was already in it.
    #[test]
    fn the_ring_can_say_what_happened_since_a_mark() {
        let mut ring = Ring::default();
        for n in 1..=5 {
            ring.push_bytes(&format!("line {n}\n"));
        }
        let mark = ring.total;
        for n in 6..=8 {
            ring.push_bytes(&format!("line {n}\n"));
        }
        assert_eq!(ring.since(mark), ["line 6", "line 7", "line 8"]);
        assert_eq!(ring.since(0).len(), 8, "a mark at the beginning is everything");

        // A mark older than the ring's memory yields what is left, not a panic.
        let mut small = Ring::default();
        for n in 0..RING_LINES + 50 {
            small.push_bytes(&format!("{n}\n"));
        }
        assert_eq!(small.since(0).len(), RING_LINES);
        assert_eq!(small.since(small.total).len(), 0);
    }

    /// On a local ring, never the process-global one: the app tests assert
    /// that a session with no subscriber has captured nothing, and a test
    /// that filled the shared ring made that a coin toss under parallel
    /// execution (PR #5 review).
    #[test]
    fn the_tail_reads_the_end_of_the_ring_and_no_more_than_asked() {
        let mut ring = Ring::default();
        for n in 1..=40 {
            ring.push_bytes(&format!("line {n}\n"));
        }

        assert_eq!(ring.tail(5), ["line 36", "line 37", "line 38", "line 39", "line 40"]);
        assert_eq!(ring.lines.len(), 40);
        // Asking for more than exists is not an error, just everything.
        assert_eq!(ring.tail(500).len(), 40);
        assert_eq!(ring.tail(0).len(), 0);
    }

    #[test]
    fn the_ring_holds_a_bounded_amount_of_memory() {
        let mut ring = Ring::default();
        // Lines far too fat for the budget: the count bound never fires,
        // the byte bound does.
        let fat = "x".repeat(64 * 1024);
        for _ in 0..200 {
            ring.push_bytes(&format!("{fat}\n"));
        }
        assert!(ring.lines.len() < RING_LINES, "evicted by weight, not by count");
        assert!(ring.bytes <= RING_BYTES, "{} bytes held", ring.bytes);

        // The accounting stays honest as lines come and go.
        assert_eq!(ring.bytes, ring.lines.iter().map(String::len).sum::<usize>());

        // An event with no newline at all cannot escape either bound: the
        // line cap chops it into pieces and the byte budget evicts them, so
        // a runaway writer costs a fixed amount of memory rather than all
        // of it.
        let mut ring = Ring::default();
        ring.push_bytes(&"y".repeat(RING_BYTES * 3));
        assert!(ring.bytes <= RING_BYTES, "{} bytes held", ring.bytes);
        assert!(!ring.lines.is_empty(), "and it still holds the recent part");
    }
}
