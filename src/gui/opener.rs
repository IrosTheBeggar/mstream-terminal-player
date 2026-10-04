//! Handing things to the operating system's opener: a torrent or a magnet
//! link to whatever client takes it, and a saved file shown in the file
//! manager. Both go through one launcher, so both honour the
//! `MSTREAM_NO_OPEN` seam the end-to-end harness sets, and neither ever
//! spawns anything under test.
//!
//! A saved file is shown rather than opened: opening the logs zip on macOS
//! starts Archive Utility, which unpacks it beside itself in Downloads.
//! Showing the file is what a browser does when its download finishes.

use std::ffi::OsString;
use std::path::Path;

use crate::kit::os::Os;

/// How a hand-off went.
pub(super) enum HandOff {
    /// The opener ran (or is still running — a client took the file).
    Launched,
    /// The opener said nothing will take it.
    Nothing,
    /// `MSTREAM_NO_OPEN`: the staged file's path, for the note.
    Headless(String),
}

/// A command for the opener, as data, so a test can read what each
/// platform would run without running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Opener {
    pub program: &'static str,
    pub args: Vec<OsString>,
    /// One argument passed exactly as written, quotes and all. Explorer
    /// parses its own command line, and `/select,"path"` is the only
    /// spelling it reads for a path with spaces; Rust's quoting of a
    /// plain argument would wrap the whole thing in quotes of its own.
    pub raw: Option<String>,
    /// Whether the exit code says anything. Explorer exits 1 when it has
    /// done what it was asked, so its code is ignored.
    pub trust_exit: bool,
}

/// Hand a file or a magnet link to the OS's opener and see whether
/// anything took it. `MSTREAM_NO_OPEN` is the test seam — headless
/// callers put the staged path in the note instead.
pub(super) fn open_target(target: &str) -> Result<HandOff, String> {
    launch(open_command(Os::HERE, target), target)
}

/// Show a saved file in the platform's file manager: selected in Finder
/// or Explorer, or its folder opened where only the folder can be named.
/// It waits up to two seconds for the opener's answer, so call it on a
/// thread.
pub(super) fn reveal(path: &Path) -> Result<HandOff, String> {
    launch(reveal_command(Os::HERE, path), &path.display().to_string())
}

/// The opener for a file or a link, per platform.
fn open_command(os: Os, target: &str) -> Opener {
    let program = match os {
        Os::Mac => "open",
        // Not `cmd /c start`: cmd.exe reads `&`, `|`, `^` and `%VAR%` in
        // the target as its own syntax, so a magnet's `&dn=…` became a
        // command. Explorer hands the file or URL to its registered
        // handler untouched.
        Os::Windows => "explorer.exe",
        Os::Unix => "xdg-open",
    };
    Opener { program, args: vec![target.into()], raw: None, trust_exit: true }
}

/// The command that shows `path` where the platform can: `open -R`
/// selects it in Finder, Explorer's `/select,` does the same, and the
/// freedesktop opener can only open its folder.
pub(super) fn reveal_command(os: Os, path: &Path) -> Opener {
    match os {
        Os::Mac => Opener {
            program: "open",
            args: vec!["-R".into(), path.as_os_str().to_owned()],
            raw: None,
            trust_exit: true,
        },
        Os::Windows => Opener {
            program: "explorer.exe",
            args: Vec::new(),
            raw: Some(format!("/select,\"{}\"", path.display())),
            trust_exit: false,
        },
        Os::Unix => Opener {
            program: "xdg-open",
            args: vec![path.parent().unwrap_or(path).as_os_str().to_owned()],
            raw: None,
            trust_exit: true,
        },
    }
}

/// Run an opener and watch it for a moment. `seam` is what a headless
/// caller shows instead: under `MSTREAM_NO_OPEN`, and always under test,
/// nothing is spawned.
fn launch(opener: Opener, seam: &str) -> Result<HandOff, String> {
    let headless = std::env::var("MSTREAM_NO_OPEN").is_ok_and(|v| !v.is_empty() && v != "0");
    if cfg!(test) || headless {
        return Ok(HandOff::Headless(seam.to_string()));
    }
    let mut command = std::process::Command::new(opener.program);
    command.args(&opener.args);
    if let Some(raw) = &opener.raw {
        #[cfg(windows)]
        std::os::windows::process::CommandExt::raw_arg(&mut command, raw);
        #[cfg(not(windows))]
        command.arg(raw);
    }
    // Explorer and the other openers are not ours: they keep their own
    // crash dialogs, not the window host's quieting.
    #[cfg(windows)]
    crate::gpu_pick::default_error_mode(&mut command, 0);
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // The openers answer fast when nothing can take the file (macOS's
    // `open` exits 1, xdg-open 3); one that is still running after a
    // moment has handed it to a client that is now starting up.
    for _ in 0..20 {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let took = status.success() || !opener.trust_exit;
            return Ok(if took { HandOff::Launched } else { HandOff::Nothing });
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(HandOff::Launched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reveal_commands_show_the_file_where_the_os_can() {
        let mac = reveal_command(Os::Mac, Path::new("/Users/jane/Downloads/x.zip"));
        assert_eq!(mac.program, "open");
        assert_eq!(mac.args, [OsString::from("-R"), OsString::from("/Users/jane/Downloads/x.zip")]);
        assert_eq!(mac.raw, None);
        assert!(mac.trust_exit);

        // Built from a string, so the test reads the same on every host:
        // a backslash is a separator only on Windows.
        let windows = reveal_command(Os::Windows, &PathBuf::from(r"C:\Users\Jane Doe\x.zip"));
        assert_eq!(windows.program, "explorer.exe");
        assert!(windows.args.is_empty());
        assert_eq!(windows.raw.as_deref(), Some(r#"/select,"C:\Users\Jane Doe\x.zip""#));
        assert!(!windows.trust_exit, "explorer exits 1 when it did what it was asked");

        let unix = reveal_command(Os::Unix, Path::new("/home/jane/Downloads/x.zip"));
        assert_eq!(unix.program, "xdg-open");
        assert_eq!(unix.args, [OsString::from("/home/jane/Downloads")]);
        assert!(unix.trust_exit);
    }

    #[test]
    fn the_opener_for_a_target_is_the_platforms_own() {
        assert_eq!(open_command(Os::Mac, "magnet:?xt=1&dn=a").program, "open");
        assert_eq!(open_command(Os::Windows, "magnet:?xt=1&dn=a").program, "explorer.exe");
        let unix = open_command(Os::Unix, "magnet:?xt=1&dn=a");
        assert_eq!(unix.program, "xdg-open");
        assert_eq!(unix.args, [OsString::from("magnet:?xt=1&dn=a")], "the link whole, one argument");
    }

    #[test]
    fn under_test_the_opener_never_launches() {
        match open_target("magnet:?xt=urn:btih:abc") {
            Ok(HandOff::Headless(seam)) => assert_eq!(seam, "magnet:?xt=urn:btih:abc"),
            _ => panic!("open_target spawned under test"),
        }
        let path = std::env::temp_dir().join("mstream-logs-never-made.zip");
        match reveal(&path) {
            Ok(HandOff::Headless(seam)) => assert_eq!(seam, path.display().to_string()),
            _ => panic!("reveal spawned under test"),
        }
    }
}
