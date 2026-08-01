//! Saved server + token, so commands other than `login` need no credentials.
//!
//! Only the JWT is persisted — never the password. The file is created
//! owner-only on unix; on Windows it inherits the (already user-scoped)
//! ACL of the roaming profile.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub server: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Absent when the server runs in public mode (no users configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Per-user config directory, honoring `MSTREAM_PLAYER_CONFIG_DIR` first.
///
/// Hand-rolled rather than pulling a dependency: the conventions are stable
/// and this stays testable via the env override.
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

pub fn session_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("session.json"))
}

/// Load the saved session, if any. A corrupt file is an error rather than a
/// silent "logged out" so the user learns why their token stopped working.
pub fn load() -> Result<Option<Session>, String> {
    let path = session_path()?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| format!("{} is corrupt ({e}); run `mstream-player logout`", path.display()))
}

pub fn save(session: &Session) -> Result<PathBuf, String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = dir.join("session.json");
    let body = serde_json::to_string_pretty(session)
        .map_err(|e| format!("could not serialize session: {e}"))?;
    fs::write(&path, body).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    restrict_permissions(&path);
    Ok(path)
}

pub fn clear() -> Result<bool, String> {
    let path = session_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("could not remove {}: {e}", path.display())),
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    // Best effort: a failure here (exotic filesystem) shouldn't break login,
    // but the token file should not be world-readable where we can help it.
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_honors_override() {
        // SAFETY: single-threaded test process for this variable; the override
        // exists precisely so tests don't touch the real profile.
        unsafe { std::env::set_var("MSTREAM_PLAYER_CONFIG_DIR", "/tmp/mstream-test-cfg") };
        assert_eq!(config_dir().unwrap(), PathBuf::from("/tmp/mstream-test-cfg"));
        unsafe { std::env::remove_var("MSTREAM_PLAYER_CONFIG_DIR") };
    }

    #[test]
    fn session_round_trips_without_token() {
        let s = Session {
            server: "http://host:3000".to_string(),
            username: None,
            token: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        // Absent fields stay absent rather than serializing as null.
        assert_eq!(json, r#"{"server":"http://host:3000"}"#);
        let back: Session = serde_json::from_str(&json).unwrap();
        assert!(back.token.is_none());
    }
}
