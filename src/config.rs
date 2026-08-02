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

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bump when a change can't be read by older players. Adding optional fields
/// doesn't count — everything here is `#[serde(default)]`.
pub const SCHEMA_VERSION: u32 = 1;

const CONFIG_FILE: &str = "config.toml";
const CREDENTIALS_FILE: &str = "credentials.toml";
/// What Phase 3 wrote. Read once, then folded into the two files above.
const LEGACY_SESSION_FILE: &str = "session.json";

// ── Shapes ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub player: PlayerPrefs,
    /// Most recently used first, so the player knows where to reconnect
    /// without needing a timestamp or a "current" pointer.
    #[serde(default, rename = "server")]
    pub servers: Vec<ServerEntry>,
}

impl Default for Config {
    fn default() -> Self {
        Config { version: SCHEMA_VERSION, player: PlayerPrefs::default(), servers: Vec::new() }
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
}

impl Default for PlayerPrefs {
    fn default() -> Self {
        PlayerPrefs {
            volume: 1.0,
            repeat: "off".to_string(),
            shuffle: false,
            autodj: "off".to_string(),
        }
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub version: u32,
    #[serde(default, rename = "token")]
    pub tokens: Vec<TokenEntry>,
}

impl Default for Credentials {
    fn default() -> Self {
        Credentials { version: SCHEMA_VERSION, tokens: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenEntry {
    pub server: String,
    pub token: String,
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

// ── Reading and writing ─────────────────────────────────────────────────────

/// Replace a file in one step: write a sibling, then rename over the target.
/// A rename is atomic on both platforms, so a crash mid-write leaves the old
/// contents rather than a truncated file.
fn write_atomic(path: &Path, contents: &str, owner_only: bool) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let temp = path.with_extension("tmp");
    fs::write(&temp, contents).map_err(|e| format!("could not write {}: {e}", temp.display()))?;
    if owner_only {
        // Tighten before the rename so the file is never briefly readable by
        // others under its real name.
        restrict_permissions(&temp);
    }
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("could not replace {}: {e}", path.display())
    })
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
        credentials.tokens.push(TokenEntry { server: server.to_string(), token });
    }
}

/// Forget every token, keeping the server list — signing out shouldn't make
/// the player forget where your music lives.
pub fn forget_all_tokens() -> Result<bool, String> {
    let mut credentials = load_credentials()?;
    if credentials.tokens.is_empty() {
        return Ok(false);
    }
    credentials.tokens.clear();
    save_credentials(&credentials)?;
    Ok(true)
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

    /// Point the config directory at a scratch path for the duration of a
    /// test. Serialised because the override is process-wide.
    struct Scratch {
        dir: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Scratch {
        fn new(name: &str) -> Self {
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

    #[test]
    fn round_trips_config_and_credentials_separately() {
        let scratch = Scratch::new("split");

        let mut config = Config::default();
        config.player.volume = 0.4;
        config.player.repeat = "all".into();
        config.player.autodj = "similar".into();
        touch_server(&mut config, "http://host:3000", Some("alice".into()));
        set_last_path(&mut config, "http://host:3000", "music/Artist");
        save(&config).unwrap();

        let mut credentials = Credentials::default();
        store_token(&mut credentials, "http://host:3000", Some("secret-token".into()));
        save_credentials(&credentials).unwrap();

        let loaded = load().unwrap();
        assert_eq!(loaded.player.volume, 0.4);
        assert_eq!(loaded.player.repeat, "all");
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
