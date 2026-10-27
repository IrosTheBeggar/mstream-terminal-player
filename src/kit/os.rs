//! Where the player runs, as data: the operating system family and the
//! process environment, each a value a caller can hand in. The clipboard's
//! routes, the Downloads folder and the file opener all decide by them, and
//! a test asks each question for every platform from any one of them,
//! with an environment of its own instead of the process's.

use std::ffi::OsString;

/// The operating system families the player treats differently. Unix is
/// everything that is neither macOS nor Windows: Linux and the BSDs, which
/// share the freedesktop tools and folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Unix,
}

impl Os {
    /// The family this binary was built for.
    pub const HERE: Os = if cfg!(target_os = "macos") {
        Os::Mac
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Unix
    };
}

/// The process environment as a lookup, so a test hands in its own rather
/// than setting variables every other test would see.
pub type Vars<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// The process's own variable, with an empty value read as unset: a shell
/// that exports `DISPLAY=` has no display, and an `SSH_TTY=` left behind by
/// a wrapper is no SSH session.
pub fn process_var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn here_is_the_family_the_binary_was_built_for() {
        let want = if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Unix
        };
        assert_eq!(Os::HERE, want);
    }

    #[test]
    fn a_variable_no_process_sets_reads_as_unset() {
        assert_eq!(process_var("MSTREAM_PLAYER_TEST_NEVER_SET_ANYWHERE"), None);
        assert!(process_var("PATH").is_some(), "every test run has a PATH");
    }
}
