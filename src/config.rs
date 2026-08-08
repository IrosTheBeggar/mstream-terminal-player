//! On-disk state, split by how much it matters if it leaks.
//!
//! Two files in the config directory:
//!
//! * `config.toml` — servers and preferences. Readable, diffable, safe to keep
//!   in a dotfiles repo, and fixable in an editor when something goes wrong.
//! * `credentials.toml` — access tokens, nothing else. Owner-only on unix, and
//!   the file you leave out of any sync.
//!
//! Both carry a schema version and are written by rename, so a crash or a full
//! disk leaves the previous file intact rather than a half-written one.
//!
//! This module also answers "where does scratch data go" ([`spool_dir`]) —
//! the settings live in config.toml, but the files themselves go under the
//! platform *cache* directory, which backup tools and tmpfs both treat
//! differently from config.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when a change can't be read by older players. Adding optional fields
/// doesn't count — everything here is `#[serde(default)]`.
///
/// That promise is what makes [`Keep`] load-bearing: a newer player's file
/// passes this gate on an older binary, so the older binary has to write back
/// the settings it didn't understand rather than delete them.
pub const SCHEMA_VERSION: u32 = 1;

/// Whatever a file held that this build doesn't model.
///
/// Every table in these two files carries one. Without it, `save()` writes
/// only the fields this binary knows, so running an older player once — even
/// just starting and quitting it, which saves on exit — silently deletes the
/// newer player's settings. Two players sharing a config directory is the
/// ordinary case: a release and a build from source, or two machines syncing
/// the same dotfiles.
pub type Keep = toml::Table;

/// What a file with no `version` line is read as. The number exists to keep a
/// *newer* player's file from being misread; someone hand-writing a `[theme]`
/// section shouldn't have to know it, and refusing the whole file over the
/// missing line is the opposite of "fixable in an editor". Saving adds it.
fn current_version() -> u32 {
    SCHEMA_VERSION
}

const CONFIG_FILE: &str = "config.toml";
const CREDENTIALS_FILE: &str = "credentials.toml";
/// What Phase 3 wrote. Read once, then folded into the two files above.
const LEGACY_SESSION_FILE: &str = "session.json";

// ── Shapes ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub player: PlayerPrefs,
    #[serde(default, skip_serializing_if = "CachePrefs::is_unset")]
    pub cache: CachePrefs,
    /// `[theme]` — the three colours the drawing code actually varies. Unset
    /// means palette names, so the terminal's own scheme picks the hues.
    #[serde(default, skip_serializing_if = "ThemePrefs::is_unset")]
    pub theme: ThemePrefs,

    /// `[display]` — how much of Unicode the terminal's font can be trusted
    /// with.
    #[serde(default, skip_serializing_if = "DisplayPrefs::is_default")]
    pub display: DisplayPrefs,

    /// `[mouse]` — the wheel and clicking the progress bar.
    #[serde(default, skip_serializing_if = "MousePrefs::is_default")]
    pub mouse: MousePrefs,
    /// `[keys]` — action name to the keys that should fire it. Empty means
    /// the built-in bindings, and only the actions named here are changed.
    /// See `mstream-player keys` for the full list in this format.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub keys: std::collections::BTreeMap<String, Vec<String>>,
    /// Most recently used first, so the player knows where to reconnect
    /// without needing a timestamp or a "current" pointer.
    #[serde(default, rename = "server")]
    pub servers: Vec<ServerEntry>,
    /// Sections and keys from a newer player. Last so that what it holds is
    /// written after the fields above, which is also what TOML requires of
    /// anything that turns out to be a table.
    #[serde(flatten)]
    pub extra: Keep,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: SCHEMA_VERSION,
            player: PlayerPrefs::default(),
            cache: CachePrefs::default(),
            theme: ThemePrefs::default(),
            display: DisplayPrefs::default(),
            mouse: MousePrefs::default(),
            keys: std::collections::BTreeMap::new(),
            servers: Vec::new(),
            extra: Keep::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerPrefs {
    pub volume: f32,
    /// "off", "all" or "one".
    pub repeat: String,
    pub shuffle: bool,
    /// "off", "similar" or "tempo+key".
    pub autodj: String,
    /// Seconds of blend when one track ends and the next begins; 0 is off.
    /// Also adjustable live from the Settings tab (Phase C5).
    pub crossfade_seconds: f32,
    /// Sample-tight track boundaries when no crossfade is set. ON by
    /// default since C6: nothing of Phase C had shipped when the opt-in
    /// stance was written, the legacy serve contract keeps its own
    /// default, and albums playing as they were cut is the better first
    /// impression — the Settings tab turns it off in two keystrokes. The
    /// cost is one track of prefetch near each boundary.
    pub gapless: bool,
    /// Manual skips blend for a second instead of breathing (C6).
    pub blend_skips: bool,
    /// Pause and resume ride a short ramp instead of landing mid-wave (C6).
    pub pause_fade: bool,
    /// How Auto-DJ chooses, beyond the mode.
    pub dj: AutoDjPrefs,
    #[serde(flatten)]
    pub extra: Keep,
}

impl Default for PlayerPrefs {
    fn default() -> Self {
        PlayerPrefs {
            volume: 1.0,
            repeat: "off".to_string(),
            shuffle: false,
            autodj: "off".to_string(),
            crossfade_seconds: 0.0,
            gapless: true,
            blend_skips: false,
            pause_fade: false,
            dj: AutoDjPrefs::default(),
            extra: Keep::new(),
        }
    }
}

impl PlayerPrefs {
    /// Take the settings a running player is holding, and keep the ones it
    /// never knew about.
    ///
    /// The app rebuilds these from its live state on the way out, so an
    /// assignment would put an empty [`Keep`] over the one that was loaded —
    /// preserving unknown keys through the read only to drop them at the
    /// write. Going through here is what makes the round trip complete.
    pub fn adopt(&mut self, fresh: PlayerPrefs) {
        let extra = std::mem::take(&mut self.extra);
        let dj_extra = std::mem::take(&mut self.dj.extra);
        *self = fresh;
        self.extra = extra;
        self.dj.extra = dj_extra;
    }
}

/// `[player.dj]` — the Auto-DJ panel's settings.
///
/// Kept as plain scalars and strings rather than enums so an unrecognised
/// value from a newer player degrades to the default instead of failing the
/// whole config load; the app parses each one leniently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoDjPrefs {
    /// Percent either side of the seed tempo for the tight window. The wide
    /// window the server falls back to is twice this.
    pub tempo_tolerance: u32,
    /// "off", "compatible" (Camelot neighbours) or "strict" (the same key).
    pub key_matching: String,
    /// Minimum rating, 1–10. Zero means no floor, which is also what the
    /// server reads a zero as.
    pub min_rating: u32,
    /// How many recently-played artists to keep out of the next pick.
    pub artist_cooldown: u32,
    /// Perceptual 1–100 slider onto a cosine threshold; 0 switches the sonic
    /// pool off entirely. See `dj::sonic_threshold`.
    pub sonic_tightness: u32,
    /// "current" (just what's playing) or "session" (recent picks averaged
    /// into a centroid, so a set drifts as a whole rather than song by song).
    pub sonic_anchor: String,
    /// "off", "whitelist" (only these) or "blacklist" (anything but these).
    pub genre_mode: String,
    pub genres: Vec<String>,
    #[serde(flatten)]
    pub extra: Keep,
}

impl Default for AutoDjPrefs {
    fn default() -> Self {
        AutoDjPrefs {
            tempo_tolerance: crate::dj::DEFAULT_TEMPO_TOLERANCE,
            key_matching: "compatible".to_string(),
            min_rating: 0,
            // A little variety by default; a session that repeats an artist
            // immediately reads as broken even when the pick was legitimate.
            artist_cooldown: 3,
            sonic_tightness: 0,
            sonic_anchor: "session".to_string(),
            genre_mode: "off".to_string(),
            genres: Vec::new(),
            extra: Keep::new(),
        }
    }
}

/// `[cache]` — where scratch data lives. Today that is only the streaming
/// spool (the playing track's buffer, one file at a time); a persistent
/// track cache would live under the same root if one ever grows.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CachePrefs {
    /// Cache root; spool files go in `spool/` inside it. Unset means the
    /// platform cache directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    #[serde(flatten)]
    pub extra: Keep,
}

impl CachePrefs {
    /// Keeps an empty `[cache]` header out of config.toml. A section holding
    /// only a newer player's keys is not empty — skipping it would delete
    /// them.
    fn is_unset(&self) -> bool {
        self.dir.is_none() && self.extra.is_empty()
    }
}

/// `[theme]` — the colours the player varies.
///
/// Each takes either a palette name (`cyan`, `bright-blue`), which lets the
/// terminal's own scheme decide the hue, or an exact `#rrggbb`, or a 0–255
/// index into the 256-colour cube. Names are the default because a terminal
/// app that hard-codes hues looks wrong in everyone else's colour scheme.
///
/// Red is not here on purpose: an error that isn't red is a trap.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemePrefs {
    /// What is playing, what is selected, the progress bar.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Labels, durations, rules, hints — everything secondary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim: Option<String>,
    /// Directories and library nodes in the browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(flatten)]
    pub extra: Keep,
}

impl ThemePrefs {
    fn is_unset(&self) -> bool {
        self.accent.is_none()
            && self.dim.is_none()
            && self.folder.is_none()
            && self.extra.is_empty()
    }
}

/// `[display]`. Which glyphs the terminal's font can actually draw.
///
/// Not a taste setting — a font either has a character or it prints a box.
/// The Windows 10 default console is conhost with Consolas, which covers the
/// old CP437 shapes (`█ ░ ▓ ▄ ─ │`) and nothing later: no eighth-height
/// blocks, no braille, no `▶`, no `★`. Windows Terminal ships Cascadia Mono,
/// which has all of them, and every ordinary Linux and macOS terminal font
/// does too.
///
/// `auto` reads `WT_SESSION` — Windows Terminal sets it, conhost does not —
/// and assumes anything not Windows is fine. `full` and `legacy` pin it,
/// for a Windows console someone has pointed at a better font, or a Linux
/// one stuck on a bitmap font.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayPrefs {
    /// `auto`, `full` or `legacy`.
    pub glyphs: String,
    /// Rows the terminal needs before the browser screen's progress bar
    /// spends a second one mirroring the waveform.
    ///
    /// The compact transport is two rows — what is playing, then the bar —
    /// and on an 80x24 terminal that is already an eighth of the list. A
    /// third row is worth it when there are rows to spare and not when there
    /// aren't, which is a question only the terminal can answer. `0` always,
    /// something enormous never. The full-screen view mirrors regardless:
    /// its body is a facts column and a panel rather than a list to scroll.
    pub mirror_min_height: u16,
    /// How many listing columns the browser will show at once, the one you
    /// are in included — the Miller columns.
    ///
    /// A ceiling, not a promise: the columns you came through are only drawn
    /// where there is width for them, innermost first, so a narrow terminal
    /// quietly shows fewer. The queue column is not one of these; it is the
    /// end of the chain rather than a step along it.
    pub miller_columns: usize,
    #[serde(flatten)]
    pub extra: Keep,
}

/// Rows below which the browser screen keeps its single-row bar.
///
/// Twenty-four rows leaves nineteen for the list; thirty leaves twenty-five,
/// and giving one of those up costs nothing anyone notices.
pub const DEFAULT_MIRROR_MIN_HEIGHT: u16 = 30;

/// Listing columns the browser will show at once, the current one included.
///
/// Four fits an 88-column terminal and is about as far back as the context
/// is still worth reading: artist, album, and the two you came through.
pub const DEFAULT_MILLER_COLUMNS: usize = 4;

impl Default for DisplayPrefs {
    fn default() -> Self {
        DisplayPrefs {
            glyphs: "auto".to_string(),
            mirror_min_height: DEFAULT_MIRROR_MIN_HEIGHT,
            miller_columns: DEFAULT_MILLER_COLUMNS,
            extra: Keep::new(),
        }
    }
}

impl DisplayPrefs {
    fn is_default(&self) -> bool {
        *self == DisplayPrefs::default()
    }
}

/// `[mouse]`. On by default: a wheel that scrolls and a bar you can click at
/// are what anyone expects of a pointer.
///
/// Worth being able to turn off, though. A terminal asked to report the mouse
/// stops doing its own click-drag selection, so copying a path off the screen
/// becomes shift-drag instead of drag. Anyone who copies more often than they
/// scroll should be able to have that back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MousePrefs {
    pub enabled: bool,
    #[serde(flatten)]
    pub extra: Keep,
}

impl Default for MousePrefs {
    fn default() -> Self {
        MousePrefs { enabled: true, extra: Keep::new() }
    }
}

impl MousePrefs {
    fn is_default(&self) -> bool {
        *self == MousePrefs::default()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ServerEntry {
    pub url: String,
    /// Who you sign in as. Not a secret — the token lives elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Where you were last browsing on this server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_path: Option<String>,
    #[serde(flatten)]
    pub extra: Keep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default, rename = "token")]
    pub tokens: Vec<TokenEntry>,
    /// Quick Connect pairing codes, one per tunnel server. These live here
    /// rather than in config.toml because a code carries the 32-byte secret
    /// that opens the tunnel — knowing it is enough to reach the server.
    #[serde(default, rename = "pairing")]
    pub pairings: Vec<PairingEntry>,
    /// Secrets of a kind this build has no name for. Losing one of these is
    /// worse than losing a setting: this file is the only copy, and a token
    /// deleted here is a sign-in that has to be done again on a machine that
    /// may not be the one in front of you.
    #[serde(flatten)]
    pub extra: Keep,
}

impl Default for Credentials {
    fn default() -> Self {
        Credentials {
            version: SCHEMA_VERSION,
            tokens: Vec::new(),
            pairings: Vec::new(),
            extra: Keep::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenEntry {
    pub server: String,
    pub token: String,
    /// A newer player's notes on this row — an expiry, say. The catch-all on
    /// [`Credentials`] covers whole kinds of secret; this covers the level
    /// below it, because storing a token for one server rewrites every row.
    #[serde(flatten)]
    pub extra: Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairingEntry {
    /// The tunnel identity this code reaches — never a URL.
    pub server: String,
    pub code: String,
    #[serde(flatten)]
    pub extra: Keep,
}

// ── Locations ───────────────────────────────────────────────────────────────

/// Per-user config directory, honoring `MSTREAM_PLAYER_CONFIG_DIR` first —
/// which is also how a portable install keeps everything beside the binary.
pub fn config_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("MSTREAM_PLAYER_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }

    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };

    base.map(|b| b.join("mstream-player")).ok_or_else(|| {
        "could not determine a config directory; set MSTREAM_PLAYER_CONFIG_DIR".to_string()
    })
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(CONFIG_FILE))
}

pub fn credentials_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(CREDENTIALS_FILE))
}

/// Where streaming spool files belong: `MSTREAM_PLAYER_CACHE_DIR` first, then
/// `[cache] dir` from config.toml, then the platform cache directory —
/// deliberately *not* the OS temp dir, which is RAM-backed tmpfs on many
/// Linux systems, where a spooled FLAC silently costs its size in memory.
/// `None` (no usable location at all) lets the engine fall back to OS temp.
pub fn spool_dir() -> Option<PathBuf> {
    cache_root().map(|root| root.join("spool"))
}

fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MSTREAM_PLAYER_CACHE_DIR") {
        return Some(expand_home(PathBuf::from(dir)));
    }
    // A broken config file must not decide where scratch files go — the TUI
    // will surface the parse error on its own; here it just means "default".
    if let Ok(config) = load() {
        if let Some(dir) = config.cache.dir {
            return Some(expand_home(dir));
        }
    }
    platform_cache_dir()
}

fn platform_cache_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    };
    base.map(|b| b.join("mstream-player"))
}

/// Expand a leading `~`, so `dir = "~/scratch"` in config.toml means what the
/// person who wrote it meant.
fn expand_home(path: PathBuf) -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    expand_home_from(path, home)
}

fn expand_home_from(path: PathBuf, home: Option<PathBuf>) -> PathBuf {
    let Some(home) = home else { return path };
    // Owned so the borrow of `path` ends before we may return it.
    let rest: Option<String> = match path.to_str() {
        Some("~") => Some(String::new()),
        Some(s) => s
            .strip_prefix("~/")
            .or_else(|| s.strip_prefix("~\\"))
            .map(str::to_string),
        None => None,
    };
    match rest {
        Some(rest) if rest.is_empty() => home,
        Some(rest) => home.join(rest),
        None => path,
    }
}

// ── Reading and writing ─────────────────────────────────────────────────────

/// Replace a file in one step: write a sibling, then rename over the target.
/// A rename is atomic on both platforms, so a crash mid-write leaves the old
/// contents rather than a truncated file.
fn write_atomic(path: &Path, contents: &str, owner_only: bool) -> Result<(), String> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let temp = temp_path(path);
    let write = |temp: &Path| -> std::io::Result<()> {
        let mut file = fs::File::create(temp)?;
        file.write_all(contents.as_bytes())?;
        // The rename orders itself against the file's *metadata*, not its
        // contents: without this, a power loss just after the rename can
        // leave the new name pointing at a file whose bytes never landed —
        // an empty config where the crash-safety was supposed to leave the
        // old one.
        file.sync_all()
    };
    write(&temp).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    if owner_only {
        // Tighten before the rename so the file is never briefly readable by
        // others under its real name.
        restrict_permissions(&temp);
    }
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("could not replace {}: {e}", path.display())
    })?;
    sweep_abandoned_temps(dir);
    Ok(())
}

/// A scratch name beside `path` that no other writer will choose.
///
/// One fixed name (`config.tmp`) was a race with teeth: a CLI `login` and the
/// player saving on exit both wrote that file, and whichever renamed second
/// published a file the other was still halfway through — corruption produced
/// by the crash-safety machinery itself. The process id separates the
/// processes and the counter separates writers inside one.
///
/// It stays in the target's own directory because a rename is only atomic
/// within a file system, and the temp directory is often a different one.
fn temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{serial}.tmp", std::process::id()))
}

/// Delete scratch files a crashed writer left behind.
///
/// Unique names mean nothing overwrites them, so without this they would
/// collect in the config directory forever. Only ones a day old go: a live
/// writer's file is seconds old, and deleting one out from under it would
/// turn someone else's save into an error.
fn sweep_abandoned_temps(dir: &Path) {
    const A_DAY: std::time::Duration = std::time::Duration::from_secs(60 * 60 * 24);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_temp = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n.ends_with(".tmp"));
        if !is_temp {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .and_then(|t| t.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age > A_DAY);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

fn read_versioned<T: for<'de> Deserialize<'de>>(
    path: &Path,
    what: &str,
) -> Result<Option<T>, String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };

    // Check the version before deserializing the rest, so a file from a newer
    // player gives a useful answer instead of a field-level parse error.
    #[derive(Deserialize)]
    struct VersionOnly {
        #[serde(default)]
        version: u32,
    }
    if let Ok(VersionOnly { version }) = toml::from_str::<VersionOnly>(&raw) {
        if version > SCHEMA_VERSION {
            return Err(format!(
                "{} is version {version}, and this player understands up to \
                 {SCHEMA_VERSION} — update the player",
                path.display()
            ));
        }
    }

    toml::from_str(&raw)
        .map(Some)
        .map_err(|e| format!("{what} at {} is not valid: {e}", path.display()))
}

pub fn load() -> Result<Config, String> {
    let config: Option<Config> = read_versioned(&config_path()?, "config")?;
    match config {
        Some(config) => Ok(config),
        // Nothing yet: fold in whatever the old session file had, so an
        // upgrade doesn't forget the server you were using.
        None => Ok(migrate_legacy_session()?.unwrap_or_default()),
    }
}

pub fn save(config: &Config) -> Result<(), String> {
    let body = toml::to_string_pretty(config)
        .map_err(|e| format!("could not encode config: {e}"))?;
    write_atomic(&config_path()?, &body, false)
}

pub fn load_credentials() -> Result<Credentials, String> {
    Ok(read_versioned(&credentials_path()?, "credentials")?.unwrap_or_default())
}

pub fn save_credentials(credentials: &Credentials) -> Result<(), String> {
    let body = toml::to_string_pretty(credentials)
        .map_err(|e| format!("could not encode credentials: {e}"))?;
    write_atomic(&credentials_path()?, &body, true)
}

// ── Convenience for callers ─────────────────────────────────────────────────

/// Compare server URLs the way the rest of the client does: trailing slashes
/// don't make two entries different, but nothing else is assumed equivalent.
pub fn same_server(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The server to reconnect to, if any.
pub fn most_recent_server(config: &Config) -> Option<&ServerEntry> {
    config.servers.first()
}

pub fn token_for(credentials: &Credentials, server: &str) -> Option<String> {
    credentials
        .tokens
        .iter()
        .find(|entry| same_server(&entry.server, server))
        .map(|entry| entry.token.clone())
}

/// Record a server as most recently used, keeping any details already known
/// about it.
pub fn touch_server(config: &mut Config, url: &str, username: Option<String>) {
    let position = config.servers.iter().position(|s| same_server(&s.url, url));
    let mut entry = match position {
        Some(index) => config.servers.remove(index),
        None => ServerEntry { url: url.to_string(), ..Default::default() },
    };
    entry.url = url.to_string();
    if username.is_some() {
        entry.username = username;
    }
    config.servers.insert(0, entry);
}

pub fn set_last_path(config: &mut Config, url: &str, path: &str) {
    if let Some(entry) = config.servers.iter_mut().find(|s| same_server(&s.url, url)) {
        entry.last_path = Some(path.to_string());
    }
}

pub fn store_token(credentials: &mut Credentials, server: &str, token: Option<String>) {
    credentials.tokens.retain(|entry| !same_server(&entry.server, server));
    if let Some(token) = token {
        credentials.tokens.push(TokenEntry {
            server: server.to_string(),
            token,
            extra: Keep::new(),
        });
    }
}

pub fn pairing_for(credentials: &Credentials, server: &str) -> Option<String> {
    credentials
        .pairings
        .iter()
        .find(|entry| same_server(&entry.server, server))
        .map(|entry| entry.code.clone())
}

pub fn store_pairing(credentials: &mut Credentials, server: &str, code: Option<String>) {
    credentials.pairings.retain(|entry| !same_server(&entry.server, server));
    if let Some(code) = code {
        credentials.pairings.push(PairingEntry {
            server: server.to_string(),
            code,
            extra: Keep::new(),
        });
    }
}

/// Forget every token, keeping the server list — signing out shouldn't make
/// the player forget where your music lives.
///
/// Pairing codes are kept for the same reason, and a stronger one: a code can
/// only be fetched over an existing connection by an admin, so discarding it
/// on a routine sign-out could leave someone away from home with no way back
/// in. Removing a tunnel server outright is what should drop its code.
pub fn forget_all_tokens() -> Result<bool, String> {
    let mut credentials = load_credentials()?;
    if credentials.tokens.is_empty() {
        return Ok(false);
    }
    credentials.tokens.clear();
    save_credentials(&credentials)?;
    Ok(true)
}

/// Test scaffolding, here rather than in a test module because the config
/// directory is set by an environment variable — one process-wide switch that
/// every test touching a config file has to take turns holding, whichever
/// module the test lives in.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Point the config directory at a scratch path for the duration of a
    /// test. Serialised because the override is process-wide.
    pub(crate) struct Scratch {
        pub(crate) dir: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!("mstream-player-test-{name}"));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            // SAFETY: the lock serialises tests that touch this variable.
            unsafe { std::env::set_var("MSTREAM_PLAYER_CONFIG_DIR", &dir) };
            Scratch { dir, _guard: guard }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            unsafe { std::env::remove_var("MSTREAM_PLAYER_CONFIG_DIR") };
            let _ = fs::remove_dir_all(&self.dir);
        }
    }
}

// ── Migration ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LegacySession {
    server: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

/// Import the Phase 3 `session.json`, then take it out of the way so this only
/// happens once. Failure is not fatal: the worst case is signing in again.
fn migrate_legacy_session() -> Result<Option<Config>, String> {
    let legacy_path = config_dir()?.join(LEGACY_SESSION_FILE);
    let Ok(raw) = fs::read_to_string(&legacy_path) else {
        return Ok(None);
    };
    let Ok(session) = serde_json::from_str::<LegacySession>(&raw) else {
        return Ok(None);
    };

    let mut config = Config::default();
    touch_server(&mut config, &session.server, session.username);
    save(&config)?;

    if session.token.is_some() {
        let mut credentials = Credentials::default();
        store_token(&mut credentials, &session.server, session.token);
        save_credentials(&credentials)?;
    }

    let _ = fs::rename(&legacy_path, legacy_path.with_extension("json.migrated"));
    Ok(Some(config))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    // Best effort: an exotic filesystem shouldn't break signing in, but the
    // token file should not be world-readable where we can help it.
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use super::testing::Scratch;

    #[test]
    fn round_trips_config_and_credentials_separately() {
        let scratch = Scratch::new("split");

        let mut config = Config::default();
        config.player.volume = 0.4;
        config.player.repeat = "all".into();
        config.player.autodj = "similar".into();
        config.player.crossfade_seconds = 6.0;
        config.player.gapless = true;
        touch_server(&mut config, "http://host:3000", Some("alice".into()));
        set_last_path(&mut config, "http://host:3000", "music/Artist");
        save(&config).unwrap();

        let mut credentials = Credentials::default();
        store_token(&mut credentials, "http://host:3000", Some("secret-token".into()));
        save_credentials(&credentials).unwrap();

        let loaded = load().unwrap();
        assert_eq!(loaded.player.volume, 0.4);
        assert_eq!(loaded.player.repeat, "all");
        assert_eq!(loaded.player.crossfade_seconds, 6.0);
        assert!(loaded.player.gapless);
        assert_eq!(loaded.servers[0].username.as_deref(), Some("alice"));
        assert_eq!(loaded.servers[0].last_path.as_deref(), Some("music/Artist"));

        // The secret is in the other file, and only in the other file.
        let config_text = fs::read_to_string(scratch.dir.join(CONFIG_FILE)).unwrap();
        assert!(config_text.contains("alice"), "the username is a setting");
        assert!(!config_text.contains("secret-token"), "the token is not");
        let credentials_text = fs::read_to_string(scratch.dir.join(CREDENTIALS_FILE)).unwrap();
        assert!(credentials_text.contains("secret-token"));

        assert_eq!(
            token_for(&load_credentials().unwrap(), "http://host:3000/"),
            Some("secret-token".into()),
            "a trailing slash is the same server"
        );
    }

    #[test]
    fn settings_this_player_has_never_heard_of_survive_its_save() {
        let scratch = Scratch::new("unknown-keys");
        // A file written by a newer player. The schema version is deliberately
        // unchanged, because the policy at the top of this file promises that
        // added optional fields don't bump it — which is exactly what puts
        // this file in front of this binary.
        // This test's example future key keeps coming true: it was
        // `crossfade_seconds` until Phase C3 shipped it, then `gapless`
        // until C4 did. `replaygain` now carries the torch.
        fs::write(
            scratch.dir.join(CONFIG_FILE),
            "version = 1\n\
             lyrics_offset = 250\n\
             \n\
             [player]\n\
             volume = 0.5\n\
             crossfade_seconds = 4\n\
             replaygain = \"album\"\n\
             \n\
             [player.dj]\n\
             energy_curve = \"rising\"\n\
             \n\
             [visualizer]\n\
             mode = \"bars\"\n",
        )
        .unwrap();

        // Exactly what a run does: read at startup, write back on the way
        // out — except that the player rebuilds the settings it owns from
        // its live state, which is where a preserved key gets dropped again
        // if `adopt` isn't in the path.
        let mut config = load().unwrap();
        // A hand-written integer where the field is an f32: the file's
        // stated design goal is "fixable in an editor", and a person writes
        // 4, not 4.0. Pinned here, on the load, so a stricter future
        // deserializer cannot quietly turn that into a whole-file refusal.
        assert_eq!(config.player.crossfade_seconds, 4.0);
        // This file predates the gapless key entirely, so it gets the C6
        // default: on. A file that explicitly said false would keep false.
        assert!(config.player.gapless, "a keyless config gets gapless by default");
        config.player.adopt(PlayerPrefs { volume: 0.8, ..PlayerPrefs::default() });
        save(&config).unwrap();

        // Read it back the way the next run would. Each key has to be in the
        // table it came from: "somewhere in the file" would also be true of a
        // key that had fallen into the wrong section.
        let again = load().unwrap();
        let int = |t: &Keep, k: &str| t.get(k).and_then(toml::Value::as_integer);
        assert_eq!(int(&again.extra, "lyrics_offset"), Some(250));
        assert!(again.extra.contains_key("visualizer"), "a whole unknown section survived");
        assert_eq!(
            again.player.extra.get("replaygain").and_then(toml::Value::as_str),
            Some("album")
        );
        assert_eq!(
            again.player.dj.extra.get("energy_curve").and_then(toml::Value::as_str),
            Some("rising")
        );
        // And this player's own setting is the one it just chose.
        assert_eq!(again.player.volume, 0.8);

        let raw = fs::read_to_string(scratch.dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("[visualizer]"), "written as a section, not inlined:\n{raw}");
    }

    #[test]
    fn an_unknown_credential_kind_is_not_dropped_on_sign_out() {
        let scratch = Scratch::new("unknown-credentials");
        fs::write(
            scratch.dir.join(CREDENTIALS_FILE),
            "version = 1\n\
             \n\
             [[token]]\n\
             server = \"http://host:3000\"\n\
             token = \"t\"\n\
             \n\
             [[api_key]]\n\
             server = \"http://host:3000\"\n\
             key = \"newer-players-secret\"\n",
        )
        .unwrap();

        assert!(forget_all_tokens().unwrap(), "there was a token to forget");
        let raw = fs::read_to_string(scratch.dir.join(CREDENTIALS_FILE)).unwrap();
        assert!(!raw.contains("token = \"t\""), "signing out forgets the token");
        assert!(
            raw.contains("newer-players-secret"),
            "and nothing else — this is the only copy:\n{raw}"
        );
    }

    #[test]
    fn a_newer_players_note_on_a_token_survives_storing_another() {
        // The catch-all on Credentials covers kinds of secret this build has
        // no name for; this is the level below it. A newer player annotating
        // its rows — an expiry on a token, a label on a pairing — hands this
        // binary a row it half-understands, and storing a token for some
        // *other* server rewrites every row it is holding.
        let scratch = Scratch::new("token-notes");
        fs::write(
            scratch.dir.join(CREDENTIALS_FILE),
            "version = 1\n\
             \n\
             [[token]]\n\
             server = \"http://host:3000\"\n\
             token = \"t\"\n\
             expires = 1234567890\n\
             \n\
             [[pairing]]\n\
             server = \"mstream+iroh://endpointabc\"\n\
             code = \"c\"\n\
             label = \"the attic machine\"\n",
        )
        .unwrap();

        let mut credentials = load_credentials().unwrap();
        store_token(&mut credentials, "http://elsewhere:3000", Some("t2".into()));
        save_credentials(&credentials).unwrap();

        let raw = fs::read_to_string(scratch.dir.join(CREDENTIALS_FILE)).unwrap();
        assert!(raw.contains("expires = 1234567890"), "the note went with the row:\n{raw}");
        assert!(raw.contains("the attic machine"), "the pairing's label too:\n{raw}");
        assert!(raw.contains("token = \"t\""), "and the row itself is still there");
    }

    #[test]
    fn two_writers_never_share_a_temp_file() {
        // One fixed temp name means the second writer renames the first
        // writer's half-written file into place: corruption produced by the
        // crash-safety machinery itself. A CLI `login` while the player exits
        // is enough.
        let target = std::env::temp_dir().join("mstream-player-test-temps/config.toml");
        let first = temp_path(&target);
        let second = temp_path(&target);
        assert_ne!(first, second, "two writers picked the same temp file");
        assert_eq!(first.parent(), target.parent(), "the rename has to stay on one volume");
    }

    #[test]
    fn a_newer_schema_is_refused_with_advice() {
        let scratch = Scratch::new("newer");
        fs::write(scratch.dir.join(CONFIG_FILE), "version = 99\n").unwrap();
        let err = load().unwrap_err();
        assert!(err.contains("update the player"), "got: {err}");
    }

    #[test]
    fn a_missing_config_is_not_an_error() {
        let _scratch = Scratch::new("missing");
        let config = load().unwrap();
        assert!(config.servers.is_empty());
        assert_eq!(config.player, PlayerPrefs::default());
    }

    #[test]
    fn a_hand_written_file_without_a_version_line_still_loads() {
        let scratch = Scratch::new("versionless");
        // What someone reaching for an editor actually writes: the section
        // they came for, and nothing else. Refusing this would make every
        // later start fail over a line the docs never told them to add.
        fs::write(
            scratch.dir.join(CONFIG_FILE),
            "[theme]\naccent = \"cyan\"\n\n[[server]]\nurl = \"http://host:3000\"\n",
        )
        .unwrap();
        let config = load().unwrap();
        assert_eq!(config.version, SCHEMA_VERSION, "read as the current schema");
        assert_eq!(config.theme.accent.as_deref(), Some("cyan"));
        assert_eq!(config.servers.len(), 1, "the rest of the file survived");

        fs::write(
            scratch.dir.join(CREDENTIALS_FILE),
            "[[token]]\nserver = \"http://host:3000\"\ntoken = \"t\"\n",
        )
        .unwrap();
        assert_eq!(load_credentials().unwrap().tokens.len(), 1);

        // Saving puts the line back, so the file self-heals on the way out.
        save(&load().unwrap()).unwrap();
        let text = fs::read_to_string(scratch.dir.join(CONFIG_FILE)).unwrap();
        assert!(text.contains("version = 1"), "got: {text}");
    }

    #[test]
    fn writing_leaves_no_temp_file_behind() {
        let scratch = Scratch::new("atomic");
        save(&Config::default()).unwrap();
        let leftovers: Vec<_> = fs::read_dir(&scratch.dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "found {leftovers:?}");
    }

    #[test]
    fn the_most_recent_server_comes_first() {
        let mut config = Config::default();
        touch_server(&mut config, "http://one:3000", None);
        touch_server(&mut config, "http://two:3000", None);
        touch_server(&mut config, "http://one:3000", Some("alice".into()));

        assert_eq!(config.servers.len(), 2, "revisiting doesn't duplicate");
        assert_eq!(most_recent_server(&config).unwrap().url, "http://one:3000");
        assert_eq!(config.servers[0].username.as_deref(), Some("alice"));
    }

    #[test]
    fn touching_a_server_keeps_what_was_already_known() {
        let mut config = Config::default();
        touch_server(&mut config, "http://one:3000", Some("alice".into()));
        set_last_path(&mut config, "http://one:3000", "music");
        // Reconnecting without naming a user must not wipe the username.
        touch_server(&mut config, "http://one:3000", None);
        assert_eq!(config.servers[0].username.as_deref(), Some("alice"));
        assert_eq!(config.servers[0].last_path.as_deref(), Some("music"));
    }

    #[test]
    fn tokens_are_replaced_not_stacked() {
        let mut credentials = Credentials::default();
        store_token(&mut credentials, "http://one:3000", Some("first".into()));
        store_token(&mut credentials, "http://one:3000", Some("second".into()));
        assert_eq!(credentials.tokens.len(), 1);
        assert_eq!(token_for(&credentials, "http://one:3000"), Some("second".into()));

        store_token(&mut credentials, "http://one:3000", None);
        assert!(credentials.tokens.is_empty());
    }

    #[test]
    fn a_tunnel_server_survives_a_restart() {
        // End to end for the thing that was broken: what a Quick Connect
        // session writes must be enough to reach the same server next run.
        let scratch = Scratch::new("tunnel");
        let id = "mstream+iroh://endpointabc";

        let mut config = Config::default();
        touch_server(&mut config, id, Some("alice".into()));
        set_last_path(&mut config, id, "music/Artist");
        save(&config).unwrap();

        let mut credentials = Credentials::default();
        store_token(&mut credentials, id, Some("jwt-token".into()));
        store_pairing(&mut credentials, id, Some("mstr1:thecode".into()));
        save_credentials(&credentials).unwrap();

        // Next launch: the identity is remembered, and both secrets come back
        // with it — the token to stay signed in, the code to get there at all.
        let reloaded = load().unwrap();
        assert_eq!(most_recent_server(&reloaded).unwrap().url, id);
        assert_eq!(reloaded.servers[0].last_path.as_deref(), Some("music/Artist"));
        let credentials = load_credentials().unwrap();
        assert_eq!(token_for(&credentials, id), Some("jwt-token".into()));
        assert_eq!(pairing_for(&credentials, id), Some("mstr1:thecode".into()));

        // The code is a secret, so it belongs in the owner-only file and
        // nowhere near the one that's safe to sync.
        let config_text = fs::read_to_string(scratch.dir.join(CONFIG_FILE)).unwrap();
        assert!(config_text.contains(id), "the identity itself is not secret");
        assert!(!config_text.contains("mstr1:thecode"), "but the code is");
    }

    #[test]
    fn a_pairing_code_is_replaced_not_stacked() {
        let mut credentials = Credentials::default();
        let id = "mstream+iroh://endpointabc";
        store_pairing(&mut credentials, id, Some("first".into()));
        // Re-pairing after a rotation: same server, new code, one entry.
        store_pairing(&mut credentials, id, Some("second".into()));
        assert_eq!(credentials.pairings.len(), 1);
        assert_eq!(pairing_for(&credentials, id), Some("second".into()));
        assert_eq!(pairing_for(&credentials, "mstream+iroh://other"), None);

        store_pairing(&mut credentials, id, None);
        assert!(credentials.pairings.is_empty());
    }

    #[test]
    fn signing_out_keeps_the_way_back_to_a_tunnel_server() {
        // A code can only be fetched over an existing connection by an admin,
        // so dropping it on a routine sign-out could strand someone away from
        // home with no way to re-pair.
        let scratch = Scratch::new("logout-tunnel");
        let id = "mstream+iroh://endpointabc";
        let mut credentials = Credentials::default();
        store_token(&mut credentials, id, Some("jwt".into()));
        store_pairing(&mut credentials, id, Some("mstr1:thecode".into()));
        save_credentials(&credentials).unwrap();

        assert!(forget_all_tokens().unwrap());
        let after = load_credentials().unwrap();
        assert!(after.tokens.is_empty(), "the sign-in is gone");
        assert_eq!(pairing_for(&after, id), Some("mstr1:thecode".into()), "the route is not");
        drop(scratch);
    }

    #[test]
    fn signing_out_forgets_tokens_but_keeps_servers() {
        let scratch = Scratch::new("logout");
        let mut config = Config::default();
        touch_server(&mut config, "http://host:3000", Some("alice".into()));
        save(&config).unwrap();
        let mut credentials = Credentials::default();
        store_token(&mut credentials, "http://host:3000", Some("t".into()));
        save_credentials(&credentials).unwrap();

        assert!(forget_all_tokens().unwrap());
        assert!(load_credentials().unwrap().tokens.is_empty());
        assert_eq!(load().unwrap().servers.len(), 1, "the server is still known");
        assert!(!forget_all_tokens().unwrap(), "signing out twice is not an error");
        drop(scratch);
    }

    #[test]
    fn a_cache_dir_round_trips_and_stays_out_of_the_file_when_unset() {
        let scratch = Scratch::new("cache");
        save(&Config::default()).unwrap();
        let text = fs::read_to_string(scratch.dir.join(CONFIG_FILE)).unwrap();
        assert!(!text.contains("[cache]"), "an unset cache dir should not clutter the file: {text}");

        let mut config = Config::default();
        config.cache.dir = Some(PathBuf::from("D:/scratch"));
        save(&config).unwrap();
        assert_eq!(load().unwrap().cache.dir, Some(PathBuf::from("D:/scratch")));
    }

    #[test]
    fn the_spool_dir_prefers_the_env_var_then_the_config_then_the_platform() {
        // Not "spool": a Scratch name is a directory name, and engine::http's
        // spool test hard-codes mstream-player-test-spool. Sharing it meant
        // one test writing a config into the directory the other was
        // counting the files in, or removing it under a save in progress —
        // a flake in whichever of the two lost the race.
        let scratch = Scratch::new("spool-prefs");

        // Nothing configured: the platform cache dir, plus our spool folder.
        let fallback = spool_dir().expect("this machine has a home directory");
        assert!(fallback.ends_with("spool"), "{}", fallback.display());
        assert!(fallback.to_string_lossy().contains("mstream-player"));

        let mut config = Config::default();
        config.cache.dir = Some(scratch.dir.join("from-config"));
        save(&config).expect("saving into a scratch dir");
        assert_eq!(spool_dir(), Some(scratch.dir.join("from-config").join("spool")));

        // SAFETY: the Scratch guard's lock serialises env manipulation.
        unsafe { std::env::set_var("MSTREAM_PLAYER_CACHE_DIR", scratch.dir.join("from-env")) };
        let from_env = spool_dir();
        unsafe { std::env::remove_var("MSTREAM_PLAYER_CACHE_DIR") };
        assert_eq!(from_env, Some(scratch.dir.join("from-env").join("spool")));
    }

    #[test]
    fn a_leading_tilde_means_home() {
        let home = Some(PathBuf::from("/home/paul"));
        assert_eq!(expand_home_from("~/x".into(), home.clone()), PathBuf::from("/home/paul/x"));
        assert_eq!(expand_home_from("~".into(), home.clone()), PathBuf::from("/home/paul"));
        assert_eq!(expand_home_from(r"~\x".into(), home.clone()), PathBuf::from("/home/paul/x"));
        assert_eq!(expand_home_from("/abs".into(), home.clone()), PathBuf::from("/abs"));
        assert_eq!(expand_home_from("not~/x".into(), home.clone()), PathBuf::from("not~/x"));
        assert_eq!(expand_home_from("~/x".into(), None), PathBuf::from("~/x"));
    }

    #[test]
    fn an_old_session_file_is_folded_in_once() {
        let scratch = Scratch::new("migrate");
        fs::write(
            scratch.dir.join(LEGACY_SESSION_FILE),
            r#"{"server":"http://old:3000","username":"bob","token":"legacy-token"}"#,
        )
        .unwrap();

        let config = load().unwrap();
        assert_eq!(config.servers[0].url, "http://old:3000");
        assert_eq!(config.servers[0].username.as_deref(), Some("bob"));
        assert_eq!(
            token_for(&load_credentials().unwrap(), "http://old:3000"),
            Some("legacy-token".into())
        );
        // Moved aside, so the import can't run again and undo later changes.
        assert!(!scratch.dir.join(LEGACY_SESSION_FILE).exists());
    }
}
