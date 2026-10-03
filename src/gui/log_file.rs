//! Where the server log's download lands, and what it is called: the
//! user's Downloads folder found the platform's way, a file name that says
//! which server and when, a check of the zip's shape before it is trusted,
//! and a write that never replaces a file already there.
//!
//! mStream builds the zip while it streams it, after the headers are out,
//! so an error halfway through truncates the body under a 200. The zip's
//! end-of-central-directory record is what tells a whole archive from a
//! cut one; nothing here parses the archive beyond that. A server that
//! writes no log files (`writeLogs` is off by default) answers an empty
//! zip, and one older than the route answers 404: both still leave the
//! user a file, the lines the log is showing, saved as text.
//!
//! Every function that reads the environment takes it as [`Vars`] and the
//! platform as [`Os`], so a test asks each platform's question from any one
//! of them with folders of its own.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::admin::tz::Zone;
use crate::api::ApiError;
use crate::kit::os::{Os, Vars, process_var};

/// The longest a file label grows: enough for any real host name's
/// telling part, short enough to leave the time stamp in view.
const LABEL_MAX: usize = 40;

/// How far from the end a zip's end record can start: its own 22 bytes
/// plus the longest comment its two-byte length can name.
const EOCD_REACH: usize = 22 + 0xffff;

/// The folders a download may land in, best first. Only the ones that
/// exist count; the caller checks.
pub(super) fn candidates(os: Os, vars: Vars) -> Vec<PathBuf> {
    let home = vars("HOME").map(PathBuf::from);
    match os {
        Os::Mac => home.map(|home| home.join("Downloads")).into_iter().collect(),
        Os::Windows => vars("USERPROFILE").map(|profile| PathBuf::from(profile).join("Downloads")).into_iter().collect(),
        Os::Unix => {
            let mut found = Vec::new();
            // The variable is not part of the freedesktop spec, but some
            // sessions export it from the same file; a relative value
            // would land wherever the player was started.
            if let Some(dir) = vars("XDG_DOWNLOAD_DIR").map(PathBuf::from).filter(|dir| dir.is_absolute()) {
                found.push(dir);
            }
            if let Some(home) = &home {
                let config = vars("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .filter(|dir| dir.is_absolute())
                    .unwrap_or_else(|| home.join(".config"));
                if let Ok(text) = std::fs::read_to_string(config.join("user-dirs.dirs"))
                    && let Some(dir) = user_dirs_download(&text, home)
                {
                    found.push(dir);
                }
                found.push(home.join("Downloads"));
            }
            found
        }
    }
}

/// The Downloads folder `xdg-user-dirs` wrote into `user-dirs.dirs`. The
/// file is shell syntax, and the tool writes only two shapes of value:
/// `"$HOME/…"` and an absolute path. `"$HOME"` alone is how it says the
/// folder is turned off, and a relative path means nothing outside the
/// shell that sources it, so both read as no answer. The last assignment
/// wins, as it would in the shell. Absolute means the file's own idea of
/// it, a leading slash: the file is written on Unix, and the host's rule
/// would call `/srv/drop` relative on Windows, where this parser only
/// ever runs under test.
pub(super) fn user_dirs_download(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| key.trim() == "XDG_DOWNLOAD_DIR")
        .last()?
        .1
        .trim();
    let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
    if let Some(rest) = value.strip_prefix("$HOME/") {
        let rest = rest.trim_matches('/');
        return (!rest.is_empty()).then(|| home.join(rest));
    }
    value.starts_with('/').then(|| PathBuf::from(value))
}

/// The first candidate that is a folder, else what `fallback` gives,
/// asked only then: giving it may make a folder.
pub(super) fn downloads_dir(
    os: Os,
    vars: Vars,
    fallback: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    candidates(os, vars).into_iter().find(|dir| dir.is_dir()).or_else(fallback)
}

/// This machine's Downloads folder, or the player's own config folder
/// when it has none: a container or a bare account still gets the file
/// somewhere the user can find it. The config folder is made only on
/// that last resort, never on a download that lands in Downloads.
pub(super) fn downloads_here() -> Option<PathBuf> {
    downloads_dir(Os::HERE, &process_var, || {
        crate::config::config_dir().ok().filter(|dir| std::fs::create_dir_all(dir).is_ok())
    })
}

/// The server's part of a file name. A tunnel's identity is a public key,
/// so it reads as the quick-connect label the rest of the player shows; a
/// URL gives its host, without the port, since a colon is no file-name
/// character on Windows. An IPv6 host is written out in full with dashes,
/// because its shortened form shrinks to almost nothing once its colons
/// are gone. Only letters, digits, dots and dashes are kept.
pub(super) fn file_label(server: &str) -> String {
    use crate::quickconnect::{TUNNEL_ID_PREFIX, is_tunnel_id};
    use reqwest::Url;
    use url::Host;

    let raw = if is_tunnel_id(server) {
        let id: String = server[TUNNEL_ID_PREFIX.len()..].chars().take(12).collect();
        format!("quickconnect-{id}")
    } else if let Ok(url) = Url::parse(server)
        && let Some(host) = url.host()
    {
        match host {
            Host::Ipv6(addr) => {
                addr.segments().iter().map(|part| format!("{part:x}")).collect::<Vec<_>>().join("-")
            }
            Host::Ipv4(addr) => addr.to_string(),
            Host::Domain(name) => name.to_string(),
        }
    } else {
        server.to_string()
    };

    let mut label = String::with_capacity(raw.len());
    for c in raw.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' };
        if !(c == '-' && label.ends_with('-')) {
            label.push(c);
        }
    }
    let trim = |s: &str| s.trim_matches(['-', '.']).to_string();
    let label: String = trim(&label).chars().take(LABEL_MAX).collect();
    let label = trim(&label);
    if label.is_empty() { "server".to_string() } else { label }
}

/// `mstream-logs-{label}-{YYYYMMDD-HHMMSS}`, on the machine's clock (UTC
/// where it names no zone), so the newest download sorts last.
pub(super) fn file_stem(label: &str, unix: i64, zone: Option<&Zone>) -> String {
    let local = unix + zone.map_or(0, |zone| zone.offset_at(unix) as i64);
    let iso = crate::admin::iso_at(local);
    // `YYYY-MM-DDTHH:MM:SS.000Z` → `YYYYMMDD-HHMMSS`.
    let date: String = iso[..10].chars().filter(char::is_ascii_digit).collect();
    let time: String = iso[11..19].chars().filter(char::is_ascii_digit).collect();
    format!("mstream-logs-{label}-{date}-{time}")
}

/// What the download's bytes turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ZipShape {
    /// A zip with entries, whole to its end record.
    Files,
    /// A zip of no entries: the end record alone.
    Empty,
    /// A zip that starts like one and never reaches its end record.
    Cut,
    /// Not a zip at all: a page, or an error some proxy wrote.
    NotZip,
}

/// Tell the download's shape by its signatures. A zip with entries starts
/// with a local file header and ends with the end-of-central-directory
/// record, whose comment length says exactly where the bytes stop; a
/// signature met anywhere else is data that happens to spell it.
pub(super) fn zip_shape(bytes: &[u8]) -> ZipShape {
    const LOCAL: &[u8] = b"PK\x03\x04";
    const END: &[u8] = b"PK\x05\x06";
    if bytes.starts_with(END) {
        return ZipShape::Empty;
    }
    if !bytes.starts_with(LOCAL) {
        return ZipShape::NotZip;
    }
    let len = bytes.len();
    if len < 22 {
        return ZipShape::Cut;
    }
    let ends = (len.saturating_sub(EOCD_REACH)..=len - 22).rev().any(|at| {
        let comment = u16::from_le_bytes([bytes[at + 20], bytes[at + 21]]) as usize;
        &bytes[at..at + 4] == END && at + 22 + comment == len
    });
    if ends { ZipShape::Files } else { ZipShape::Cut }
}

/// Write `bytes` to `dir/stem.ext`, or the first of `stem-2.ext` …
/// `stem-99.ext` that is free: a download never replaces a file already
/// there. A file the write fails partway through is removed, so nothing
/// half-written is left to be mistaken for the log. With every name
/// taken, the answer is [`SaveError::Taken`], which the note words itself;
/// the disk's own refusals keep the system's words.
pub(super) fn write_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> Result<PathBuf, SaveError> {
    let io = |e: std::io::Error| SaveError::Io(e.to_string());
    std::fs::create_dir_all(dir).map_err(io)?;
    for n in 1..=99 {
        let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(bytes).and_then(|()| file.flush()) {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(io(e));
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io(e)),
        }
    }
    Err(SaveError::Taken(format!("{stem}.{ext}")))
}

/// Why the lines shown were saved instead of the server's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fallback {
    /// The server has no logs download route, or answered something else.
    NoRoute,
    /// The server writes no log files: its zip was empty.
    NoFiles,
}

/// What a download left on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Saved {
    Zip(PathBuf),
    Text { path: PathBuf, why: Fallback },
}

/// Why a download left nothing on disk.
#[derive(Debug)]
pub(super) enum SaveError {
    /// The server refused, or could not be reached.
    Api(ApiError),
    /// The zip ended before its end record.
    Cut,
    /// A text fallback, with no lines to put in it.
    Nothing,
    /// No folder to save into.
    NoFolder,
    /// The file's name, and every numbered copy of it, already there.
    Taken(String),
    /// The disk said no.
    Io(String),
}

/// Save the download's answer into `dir`: the zip when it holds files,
/// the lines shown (`lines`, already joined) as text when the server has
/// no files or no route to send them by, nothing when the zip was cut.
pub(super) fn save_download(
    answer: Result<Vec<u8>, ApiError>,
    dir: &Path,
    stem: &str,
    lines: &str,
) -> Result<Saved, SaveError> {
    let why = match answer {
        Ok(bytes) => match zip_shape(&bytes) {
            ZipShape::Files => return write_new(dir, stem, "zip", &bytes).map(Saved::Zip),
            ZipShape::Cut => return Err(SaveError::Cut),
            ZipShape::Empty => Fallback::NoFiles,
            ZipShape::NotZip => Fallback::NoRoute,
        },
        Err(ApiError::NotFound(_)) => Fallback::NoRoute,
        Err(e) => return Err(SaveError::Api(e)),
    };
    if lines.is_empty() {
        return Err(SaveError::Nothing);
    }
    let path = write_new(dir, stem, "txt", format!("{lines}\n").as_bytes())?;
    Ok(Saved::Text { path, why })
}

/// A path as a note shows it: under the home folder it starts `~/`, the
/// way a shell writes it. Windows users know no tilde, so there the path
/// is written whole.
pub(super) fn shown_path(path: &Path, home: Option<&Path>, os: Os) -> String {
    if os != Os::Windows
        && let Some(home) = home.filter(|home| home.parent().is_some())
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() { "~".to_string() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;

    /// A folder of the test's own under the system's temp folder, gone
    /// when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("mstream-player-log-file-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn env(pairs: &[(&str, &Path)]) -> HashMap<String, OsString> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.as_os_str().to_owned())).collect()
    }

    fn dir(path: PathBuf) -> PathBuf {
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn downloads_resolve_per_platform() {
        let scratch = Scratch::new("downloads");
        let home = dir(scratch.0.join("home"));
        let fallback = Some(scratch.0.join("config"));

        // macOS and Windows each have one place.
        let downloads = dir(home.join("Downloads"));
        let vars = env(&[("HOME", &home)]);
        let get = |k: &str| vars.get(k).cloned();
        assert_eq!(downloads_dir(Os::Mac, &get, || fallback.clone()), Some(downloads.clone()));
        // The fallback is only asked once no candidate is a folder: it
        // may make one.
        let asked = std::cell::Cell::new(false);
        let found = downloads_dir(Os::Mac, &get, || {
            asked.set(true);
            None
        });
        assert_eq!((found, asked.get()), (Some(downloads.clone()), false));
        let profile = dir(scratch.0.join("profile"));
        let profile_downloads = dir(profile.join("Downloads"));
        let vars = env(&[("USERPROFILE", &profile)]);
        let get = |k: &str| vars.get(k).cloned();
        assert_eq!(downloads_dir(Os::Windows, &get, || fallback.clone()), Some(profile_downloads));

        // The freedesktop order: the variable when absolute, then the
        // user-dirs file, then ~/Downloads.
        let exported = dir(scratch.0.join("exported"));
        let named = dir(home.join("Téléchargements"));
        dir(home.join(".config"));
        std::fs::write(home.join(".config/user-dirs.dirs"), "XDG_DOWNLOAD_DIR=\"$HOME/Téléchargements\"\n").unwrap();
        let vars = env(&[("HOME", &home), ("XDG_DOWNLOAD_DIR", &exported)]);
        let get = |k: &str| vars.get(k).cloned();
        assert_eq!(downloads_dir(Os::Unix, &get, || fallback.clone()), Some(exported));
        let vars = env(&[("HOME", &home), ("XDG_DOWNLOAD_DIR", Path::new("relative/dl"))]);
        let get = |k: &str| vars.get(k).cloned();
        let found = downloads_dir(Os::Unix, &get, || fallback.clone());
        assert_eq!(found, Some(named.clone()), "a relative variable is ignored");
        assert_eq!(candidates(Os::Unix, &get), [named.clone(), downloads.clone()]);

        // XDG_CONFIG_HOME moves the file.
        let config = dir(scratch.0.join("xdg"));
        let elsewhere = dir(scratch.0.join("elsewhere"));
        std::fs::write(config.join("user-dirs.dirs"), format!("XDG_DOWNLOAD_DIR=\"{}\"\n", elsewhere.display())).unwrap();
        let vars = env(&[("HOME", &home), ("XDG_CONFIG_HOME", &config)]);
        let get = |k: &str| vars.get(k).cloned();
        assert_eq!(downloads_dir(Os::Unix, &get, || fallback.clone()), Some(elsewhere));

        // A file naming a folder that is not there falls through to ~/Downloads.
        std::fs::remove_dir_all(&named).unwrap();
        let vars = env(&[("HOME", &home)]);
        let get = |k: &str| vars.get(k).cloned();
        assert_eq!(downloads_dir(Os::Unix, &get, || fallback.clone()), Some(downloads.clone()));

        // No candidate is a folder: the fallback.
        std::fs::remove_dir_all(&downloads).unwrap();
        assert_eq!(downloads_dir(Os::Unix, &get, || fallback.clone()), fallback);
        assert_eq!(downloads_dir(Os::Mac, &get, || fallback.clone()), fallback);
        let nothing = |_: &str| None;
        assert_eq!(downloads_dir(Os::Windows, &nothing, || None), None);
    }

    #[test]
    fn user_dirs_parse_home_relative_and_absolute_and_skip_comments() {
        let home = Path::new("/home/jane");
        let file = "# This file is written by xdg-user-dirs-update\n\
                    # XDG_DOWNLOAD_DIR=\"$HOME/commented\"\n\
                    XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n\
                    XDG_DOWNLOAD_DIR=\"$HOME/Downloads/mine\"\n";
        assert_eq!(user_dirs_download(file, home), Some(PathBuf::from("/home/jane/Downloads/mine")));
        assert_eq!(
            user_dirs_download("XDG_DOWNLOAD_DIR=\"/srv/drop\"\n", home),
            Some(PathBuf::from("/srv/drop"))
        );
        assert_eq!(user_dirs_download("  XDG_DOWNLOAD_DIR=/srv/bare\n", home), Some(PathBuf::from("/srv/bare")));
        assert_eq!(user_dirs_download("XDG_DOWNLOAD_DIR=\"$HOME\"\n", home), None, "turned off");
        assert_eq!(user_dirs_download("XDG_DOWNLOAD_DIR=\"$HOME/\"\n", home), None, "turned off");
        assert_eq!(user_dirs_download("XDG_DOWNLOAD_DIR=\"Downloads\"\n", home), None, "relative");
        assert_eq!(user_dirs_download("XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n", home), None);
        assert_eq!(user_dirs_download("", home), None);
    }

    #[test]
    fn file_labels_keep_only_what_a_file_name_holds() {
        assert_eq!(file_label("https://music.example.com:3000"), "music.example.com");
        assert_eq!(file_label("http://192.168.1.20:3000/"), "192.168.1.20");
        let id = "mstream+iroh://abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrst";
        assert_eq!(file_label(id), "quickconnect-abcdefghijkl");
        assert_eq!(file_label("http://[::1]:3000"), "0-0-0-0-0-0-0-1");
        let long = format!("https://{}.example.com", "a".repeat(60));
        assert_eq!(file_label(&long), "a".repeat(40));
        assert_eq!(file_label("https://x.example.com/"), "x.example.com");
        assert_eq!(file_label("not a url: home/box"), "not-a-url-home-box");
        assert_eq!(file_label("--..--"), "server");
        assert_eq!(file_label(""), "server");
    }

    #[test]
    fn file_stems_name_the_server_and_the_local_time() {
        // 2026-10-02T09:15:04Z.
        let t = crate::admin::iso_unix("2026-10-02T09:15:04Z").unwrap();
        assert_eq!(file_stem("music.example.com", t, None), "mstream-logs-music.example.com-20261002-091504");
        // The machine's own zone, where it has one: the stamp is UTC moved
        // by the zone's offset at that instant.
        if let Some(zone) = crate::admin::tz::local() {
            let moved = t + zone.offset_at(t) as i64;
            assert_eq!(file_stem("x", t, Some(&zone)), file_stem("x", moved, None));
        }
    }

    fn zip_with(entries: &[u8], comment: &[u8]) -> Vec<u8> {
        let mut out = b"PK\x03\x04".to_vec();
        out.extend_from_slice(entries);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        out.extend_from_slice(comment);
        out
    }

    #[test]
    fn zip_shapes_tell_files_empty_cut_and_not_a_zip() {
        assert_eq!(zip_shape(&zip_with(b"server.log contents", b"")), ZipShape::Files);
        assert_eq!(zip_shape(&zip_with(b"server.log contents", b"made by archiver")), ZipShape::Files);
        let mut empty = b"PK\x05\x06".to_vec();
        empty.extend_from_slice(&[0; 18]);
        assert_eq!(empty.len(), 22);
        assert_eq!(zip_shape(&empty), ZipShape::Empty);
        let whole = zip_with(&[7; 300], b"");
        assert_eq!(zip_shape(&whole[..whole.len() - 30]), ZipShape::Cut);
        assert_eq!(zip_shape(b"PK\x03\x04"), ZipShape::Cut);
        assert_eq!(zip_shape(b"<html><body>mStream</body></html>"), ZipShape::NotZip);
        assert_eq!(zip_shape(b""), ZipShape::NotZip);
        // An end signature inside the data, whose comment length does not
        // reach the end exactly, is data.
        let mut fake = b"PK\x03\x04".to_vec();
        fake.extend_from_slice(b"PK\x05\x06");
        fake.extend_from_slice(&[0; 16]);
        fake.extend_from_slice(&5u16.to_le_bytes());
        fake.extend_from_slice(b"more data than five bytes");
        assert_eq!(zip_shape(&fake), ZipShape::Cut);
    }

    #[test]
    fn a_new_file_never_overwrites() {
        let scratch = Scratch::new("write-new");
        let into = scratch.0.join("Downloads");
        let first = write_new(&into, "mstream-logs-x", "zip", b"first").unwrap();
        let second = write_new(&into, "mstream-logs-x", "zip", b"second").unwrap();
        assert_eq!(first, into.join("mstream-logs-x.zip"));
        assert_eq!(second, into.join("mstream-logs-x-2.zip"));
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");

        // With every numbered name taken, the answer names the file, for
        // the note to word; nothing already there is touched.
        for n in 3..=99 {
            std::fs::write(into.join(format!("mstream-logs-x-{n}.zip")), b"older").unwrap();
        }
        match write_new(&into, "mstream-logs-x", "zip", b"third") {
            Err(SaveError::Taken(name)) => assert_eq!(name, "mstream-logs-x.zip"),
            other => panic!("every name taken: {other:?}"),
        }
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 99);
    }

    #[test]
    fn save_download_writes_the_zip_or_the_lines_and_says_why() {
        let scratch = Scratch::new("save");
        let into = scratch.0.as_path();
        let lines = "09:00:01  server started\n09:00:02  warn  slow scan";

        let zip = zip_with(b"server.log", b"");
        match save_download(Ok(zip.clone()), into, "a", lines) {
            Ok(Saved::Zip(path)) => {
                assert_eq!(path, into.join("a.zip"));
                assert_eq!(std::fs::read(path).unwrap(), zip);
            }
            other => panic!("a zip of files: {other:?}"),
        }

        let mut empty = b"PK\x05\x06".to_vec();
        empty.extend_from_slice(&[0; 18]);
        match save_download(Ok(empty.clone()), into, "b", lines) {
            Ok(Saved::Text { path, why: Fallback::NoFiles }) => {
                assert_eq!(path, into.join("b.txt"));
                assert_eq!(std::fs::read_to_string(path).unwrap(), format!("{lines}\n"));
            }
            other => panic!("an empty zip: {other:?}"),
        }

        for (stem, answer) in [
            ("c", Ok(b"<html>".to_vec())),
            ("d", Err(ApiError::NotFound("api/v1/admin/logs/download".into()))),
        ] {
            match save_download(answer, into, stem, lines) {
                Ok(Saved::Text { path, why: Fallback::NoRoute }) => {
                    assert_eq!(path, into.join(format!("{stem}.txt")));
                    let text = std::fs::read_to_string(path).unwrap();
                    assert!(text.ends_with("slow scan\n"), "{text}");
                }
                other => panic!("no route ({stem}): {other:?}"),
            }
        }

        let whole = zip_with(&[1; 64], b"");
        assert!(matches!(save_download(Ok(whole[..40].to_vec()), into, "e", lines), Err(SaveError::Cut)));
        assert!(matches!(save_download(Ok(empty), into, "f", ""), Err(SaveError::Nothing)));
        assert!(matches!(
            save_download(Err(ApiError::NotFound("x".into())), into, "g", ""),
            Err(SaveError::Nothing)
        ));
        assert!(matches!(
            save_download(Err(ApiError::Unauthorized), into, "h", lines),
            Err(SaveError::Api(ApiError::Unauthorized))
        ));
        for stem in ["e", "f", "g", "h"] {
            assert!(!into.join(format!("{stem}.zip")).exists() && !into.join(format!("{stem}.txt")).exists());
        }
    }

    #[test]
    fn the_home_folds_to_a_tilde_off_windows() {
        let home = Path::new("/Users/jane");
        let file = Path::new("/Users/jane/Downloads/mstream-logs-x.zip");
        assert_eq!(shown_path(file, Some(home), Os::Mac), "~/Downloads/mstream-logs-x.zip");
        assert_eq!(shown_path(file, Some(home), Os::Unix), "~/Downloads/mstream-logs-x.zip");
        assert_eq!(shown_path(file, Some(home), Os::Windows), file.display().to_string());
        assert_eq!(shown_path(file, None, Os::Mac), file.display().to_string());
        let elsewhere = Path::new("/srv/drop/mstream-logs-x.zip");
        assert_eq!(shown_path(elsewhere, Some(home), Os::Mac), "/srv/drop/mstream-logs-x.zip");
        assert_eq!(shown_path(Path::new("/Users/janet/x.zip"), Some(home), Os::Mac), "/Users/janet/x.zip");
        assert_eq!(shown_path(file, Some(Path::new("/")), Os::Mac), file.display().to_string(), "a root home folds nothing");
    }
}
