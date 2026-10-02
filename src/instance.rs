//! One desktop player per install — the launcher's rule, kept by an
//! advisory file lock the player holds for its lifetime.
//!
//! The mStream tray launcher opens the player in a terminal window it keeps
//! no handle on (Terminal.app and Windows Terminal hand the window off and
//! return at once; a factory terminal on Linux does the same), so "is the
//! player still open?" can only be answered by the player itself.
//! `--instance-lock <path>` names a file in the launcher's data home; the
//! player takes an exclusive lock on it before anything else starts and
//! holds it until it exits. The OS releases the lock with the process,
//! crash included, so a stale lock cannot exist. The launcher tries the
//! same lock before spawning: held means "bring forward the window you
//! already have", free means "open one".
//!
//! Beside the lock, a small JSON sidecar — the lock path with a `.json`
//! extension — says who holds it: pid, face, the hosting terminal and the
//! start time, so the launcher can choose how to focus (activate the
//! bundled console on macOS, find the window by its title on Windows). It
//! means something only while the lock is held; a reader checks the lock
//! first. The schema number is there for additions: a control endpoint's
//! port and token, an inbox for hand-offs, whatever a later slice needs.
//!
//! Without the flag nothing here runs: a player started by hand is not the
//! launcher's to count, and a hand-launched second `gui --torrent` keeps
//! the add-torrent contract's v1 behavior (a second instance) for the same
//! reason. The one exception is the desktop flavour's empty argv — the app
//! a person double-clicks — which takes a default lock in the player's own
//! config directory (desktop.rs), so a second double-click does not open a
//! second player.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The sidecar's shape version. Bump when a field changes meaning; add
/// fields freely — readers ignore what they do not know.
pub const SIDECAR_SCHEMA: u32 = 1;

/// The held lock. Dropping it releases the lock and removes the sidecar;
/// a crash releases the lock without the removal, which is why the sidecar
/// is only ever read behind a lock check.
pub struct Instance {
    _lock: fslock::LockFile,
    sidecar: PathBuf,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.sidecar);
    }
}

/// What claiming the lock came to.
pub enum Claim {
    /// This process holds it now.
    Held(Instance),
    /// Another player holds it; its sidecar, when readable, says which.
    Taken(Option<Sidecar>),
    /// No path was given: the launcher's rule does not apply to this run.
    Unlocked,
}

/// Who holds the lock — written beside it while it is held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sidecar {
    pub schema: u32,
    pub pid: u32,
    /// `gui` or `tui`.
    pub face: String,
    /// What hosts the player: `window` when it draws in its own window
    /// (`gui --window`, or the desktop flavour's empty argv), else the
    /// terminal, from its environment: `ghostty`, `apple-terminal`,
    /// `windows-terminal`, `conhost`, `iterm`, `wezterm`, `kitty`, `vte`,
    /// `vscode`, another program's own name, or `unknown`. A launcher
    /// focuses a `window` holder by its own window, not a terminal's.
    pub host: String,
    /// Seconds since the Unix epoch.
    #[serde(rename = "startedAt")]
    pub started_at: u64,
    /// The control API this player hosts on loopback (gui/control.rs:
    /// `gui --serve-port`), and the token every route but `GET /version`
    /// wants in `x-auth-token`. Absent on a player launched without a
    /// port. mStream's server reads both to adopt the desktop player as
    /// its server-audio engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// The sidecar's path for a lock path: the same name with a `.json`
/// extension (`desktop-player.lock` → `desktop-player.json`).
pub fn sidecar_path(lock: &Path) -> PathBuf {
    lock.with_extension("json")
}

/// Take the lock at `path` for this process, or learn who has it. `face`
/// is what this run is (`gui`, `tui`) for the sidecar; `window` whether it
/// draws in its own window rather than a terminal (the sidecar's `host`);
/// `control` the port and token of the control face this run will host,
/// published with it.
/// `None` is the unlocked run. Errors are the lock file itself being unusable (a
/// directory that cannot be created, permissions) — the caller carries on
/// without a lock and says so; the sidecar failing to write is not an
/// error, only a missing hint.
pub fn claim(
    path: Option<&Path>,
    face: &str,
    window: bool,
    control: Option<(u16, &str)>,
) -> Result<Claim, String> {
    let Some(path) = path else { return Ok(Claim::Unlocked) };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let mut lock = fslock::LockFile::open(path.as_os_str())
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let got = lock.try_lock().map_err(|e| format!("cannot lock {}: {e}", path.display()))?;
    let sidecar = sidecar_path(path);
    if !got {
        return Ok(Claim::Taken(read_sidecar(&sidecar)));
    }
    let who = Sidecar {
        schema: SIDECAR_SCHEMA,
        pid: std::process::id(),
        face: face.to_string(),
        host: host_for(window, |name| std::env::var(name).ok()),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        port: control.map(|(port, _)| port),
        token: control.map(|(_, token)| token.to_string()),
    };
    if let Err(e) = write_sidecar(&sidecar, &who) {
        eprintln!("warning: could not write {}: {e}", sidecar.display());
    }
    Ok(Claim::Held(Instance { _lock: lock, sidecar }))
}

/// The one line a refused second player prints before it leaves.
pub fn already_open_line(who: Option<&Sidecar>) -> String {
    match who {
        Some(s) => format!(
            "mStream Player is already open (pid {}, in {}) - switch to that window.",
            s.pid, s.host
        ),
        None => "mStream Player is already open - switch to that window.".to_string(),
    }
}

/// Write the sidecar for this holder's eyes only: it carries the control
/// face's token, and the data home may be readable by other users of the
/// machine (a 0755 home on Linux). Owner-only from the first byte — the
/// mode is set at creation, not after the write — and re-asserted on a
/// file that already existed with a wider mode. Windows keeps the profile
/// directory's own ACL.
fn write_sidecar(path: &Path, who: &Sidecar) -> std::io::Result<()> {
    use std::io::Write;
    let body = serde_json::to_string_pretty(who).map_err(std::io::Error::other)?;
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    let mut file = open.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(body.as_bytes())
}

fn read_sidecar(path: &Path) -> Option<Sidecar> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The sidecar's host: `window` for a player in its own window, whatever
/// terminal (if any) started it — the terminal it came from is not where
/// it is — else the hosting terminal.
pub fn host_for(window: bool, get: impl Fn(&str) -> Option<String>) -> String {
    if window { "window".to_string() } else { host_from_env(get) }
}

/// The hosting terminal, from the variables terminals set for the programs
/// they run. Windows Terminal is `WT_SESSION`; most others announce
/// themselves in `TERM_PROGRAM`; kitty and the VTE family (GNOME Terminal,
/// Xfce Terminal…) use their own markers. Anything unrecognised keeps its
/// own name, lowercased and trimmed to a safe token, so the launcher log
/// can still say where the player lives. `get` is the environment, passed
/// in so the mapping is a pure function.
pub fn host_from_env(get: impl Fn(&str) -> Option<String>) -> String {
    if get("WT_SESSION").is_some() {
        return "windows-terminal".to_string();
    }
    match get("TERM_PROGRAM").as_deref() {
        Some("ghostty") => "ghostty".to_string(),
        Some("Apple_Terminal") => "apple-terminal".to_string(),
        Some("iTerm.app") => "iterm".to_string(),
        Some("WezTerm") => "wezterm".to_string(),
        Some("vscode") => "vscode".to_string(),
        Some(other) => {
            let token: String = other
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
                .take(32)
                .collect();
            if token.is_empty() { "unknown".to_string() } else { token }
        }
        None if get("KITTY_WINDOW_ID").is_some() => "kitty".to_string(),
        None if get("VTE_VERSION").is_some() => "vte".to_string(),
        None if cfg!(windows) => "conhost".to_string(),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mstream-player-instance-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    #[test]
    fn the_sidecar_is_the_holders_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let lock = dir.join("desktop-player.lock");
        let sidecar = sidecar_path(&lock);
        // A sidecar left behind wide open (a crash under an older build, a
        // hand-edited file) is tightened, not trusted.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&sidecar, "{}").unwrap();
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o644)).unwrap();
        let held = match claim(Some(&lock), "gui", false, Some((3333, "tok-en"))).unwrap() {
            Claim::Held(h) => h,
            _ => panic!("the claim must hold"),
        };
        let mode = std::fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the token inside is nobody else's business: {mode:o}");
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_admits_one_player_and_the_sidecar_names_it() {
        let dir = scratch("one");
        let lock = dir.join("desktop-player.lock");
        // The directory does not exist yet: claim makes it.
        let first = claim(Some(&lock), "gui", false, Some((3333, "tok-en"))).unwrap();
        let held = match first {
            Claim::Held(h) => h,
            _ => panic!("the first claim must hold"),
        };
        let sidecar = sidecar_path(&lock);
        assert_eq!(sidecar, dir.join("desktop-player.json"));
        let who: Sidecar = serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(who.schema, SIDECAR_SCHEMA);
        assert_eq!(who.pid, std::process::id());
        assert_eq!(who.face, "gui");
        assert!(!who.host.is_empty());
        assert!(who.started_at > 1_700_000_000, "{}", who.started_at);
        assert_eq!((who.port, who.token.as_deref()), (Some(3333), Some("tok-en")));

        // A second claim, from another handle on the same file, is refused
        // and told who holds it.
        match claim(Some(&lock), "tui", false, None).unwrap() {
            Claim::Taken(Some(s)) => assert_eq!(s, who),
            Claim::Taken(None) => panic!("the sidecar should be readable"),
            _ => panic!("the second claim must be refused"),
        }
        assert_eq!(
            already_open_line(Some(&who)),
            format!("mStream Player is already open (pid {}, in {}) - switch to that window.", who.pid, who.host)
        );
        assert_eq!(already_open_line(None), "mStream Player is already open - switch to that window.");

        // Dropping the holder releases the lock and removes the sidecar.
        drop(held);
        assert!(!sidecar.exists(), "the sidecar is gone with the holder");
        let again = match claim(Some(&lock), "gui", false, None).unwrap() {
            Claim::Held(held) => held,
            _ => panic!("the lock is free again"),
        };
        // A run without a control face writes no port and no token — read
        // while the holder lives, since its drop takes the sidecar with it.
        let text = std::fs::read_to_string(&sidecar).unwrap();
        let bare: Sidecar = serde_json::from_str(&text).unwrap();
        assert_eq!((bare.port, bare.token), (None, None));
        assert!(!text.contains("port"), "absent, not null: {text}");
        drop(again);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_path_means_no_lock() {
        assert!(matches!(claim(None, "gui", false, None).unwrap(), Claim::Unlocked));
    }

    #[test]
    fn hosts_are_read_from_what_terminals_set() {
        let env = |pairs: &[(&str, &str)]| {
            let owned: Vec<(String, String)> =
                pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |name: &str| owned.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
        };
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "ghostty")])), "ghostty");
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "Apple_Terminal")])), "apple-terminal");
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "iTerm.app")])), "iterm");
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "WezTerm")])), "wezterm");
        // Windows Terminal sets no TERM_PROGRAM; its session id wins over
        // anything a shell profile might export.
        assert_eq!(host_from_env(env(&[("WT_SESSION", "abc"), ("TERM_PROGRAM", "vscode")])), "windows-terminal");
        assert_eq!(host_from_env(env(&[("KITTY_WINDOW_ID", "1")])), "kitty");
        assert_eq!(host_from_env(env(&[("VTE_VERSION", "7400")])), "vte");
        // An unknown program keeps a safe token of its own name.
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "Some Term/2.0 <x>")])), "someterm2.0x");
        assert_eq!(host_from_env(env(&[("TERM_PROGRAM", "///")])), "unknown");
        let bare = host_from_env(env(&[]));
        assert_eq!(bare, if cfg!(windows) { "conhost" } else { "unknown" });

        // A player in its own window is hosted by it, whichever terminal
        // started it; the terminal faces keep the terminal's name.
        let wt = || env(&[("WT_SESSION", "abc"), ("TERM_PROGRAM", "ghostty")]);
        assert_eq!(host_for(true, wt()), "window");
        assert_eq!(host_for(true, env(&[])), "window");
        assert_eq!(host_for(false, wt()), "windows-terminal");
        assert_eq!(host_for(false, env(&[("TERM_PROGRAM", "ghostty")])), "ghostty");
    }

    #[test]
    fn a_window_holders_sidecar_says_window() {
        let dir = scratch("window");
        let lock = dir.join("desktop-player.lock");
        let held = match claim(Some(&lock), "gui", true, None).unwrap() {
            Claim::Held(h) => h,
            _ => panic!("the claim must hold"),
        };
        let who: Sidecar =
            serde_json::from_str(&std::fs::read_to_string(sidecar_path(&lock)).unwrap()).unwrap();
        assert_eq!((who.face.as_str(), who.host.as_str()), ("gui", "window"));
        assert_eq!(already_open_line(Some(&who)), format!(
            "mStream Player is already open (pid {}, in window) - switch to that window.",
            who.pid
        ));
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
