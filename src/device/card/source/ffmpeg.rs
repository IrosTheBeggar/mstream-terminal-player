//! A local ffmpeg, for a federation peer's conversions (the transfer plan's
//! 2.5, part B9). A key or a guest can't call a peer's `/transcode`
//! (mStream's `federation-auth.js`), so a peer's file the player can't play
//! is fetched as it is and converted on this computer.
//!
//! **Found, never bundled** ([`find`]): mStream's own copy when the terminal
//! runs beside the server as its admin (`GET /api/v1/admin/transcode`'s
//! `ffmpegDirectory`, which mStream fills by downloading into it), else the
//! one on the PATH, else none — and a sync then skips the files that need
//! converting, and lists them.
//!
//! **The conversion is mStream's own** (`transcode.js`'s `spawnTranscode`):
//! the audio alone, in the codec's container (Ogg for Opus), at the
//! bitrate, from a file into a file. What it drops is put back after (the
//! sync's retag), and its length is held to its source's, as a server's
//! conversion is ([`super::length`]).

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::api::urls::{TransferBitrate, TransferCodec};

/// The most of ffmpeg's own words a failure keeps.
const WORDS: usize = 4 * 1024;

/// A conversion on this computer.
pub(crate) trait Convert: Send + Sync {
    /// `from` made into `to` as `codec` at `bitrate`; Err why. A `stop`
    /// set ends it, and it fails.
    fn convert(
        &self,
        from: &Path,
        to: &Path,
        codec: TransferCodec,
        bitrate: TransferBitrate,
        stop: &AtomicBool,
    ) -> Result<(), String>;
    /// Where it was found, as one stable word: `server` (mStream's own
    /// copy) or `path`.
    fn found(&self) -> &'static str;
}

/// ffmpeg's arguments for a conversion: mStream's, into a file.
pub(crate) fn args(
    from: &Path,
    to: &Path,
    codec: TransferCodec,
    bitrate: TransferBitrate,
) -> Vec<OsString> {
    let (format, encoder) = match codec {
        TransferCodec::Opus => ("ogg", "libopus"),
        TransferCodec::Mp3 => ("mp3", "libmp3lame"),
    };
    let words = ["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"];
    let mut out: Vec<OsString> = words.iter().map(OsString::from).collect();
    out.push(from.as_os_str().to_owned());
    for word in ["-vn", "-f", format, "-acodec", encoder, "-ab", bitrate.as_str()] {
        out.push(word.into());
    }
    out.push(to.as_os_str().to_owned());
    out
}

/// ffmpeg's file name on this OS.
fn exe() -> &'static str {
    if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }
}

/// ffmpeg in `folder`, when it is there as a file.
pub(crate) fn in_folder(folder: &Path) -> Option<PathBuf> {
    let bin = folder.join(exe());
    bin.is_file().then_some(bin)
}

/// The first ffmpeg of a `PATH` value's folders.
pub(crate) fn on_path(path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path?).filter(|dir| dir.is_absolute()).find_map(|dir| in_folder(&dir))
}

/// See the module's note: `server_folder` is mStream's `ffmpegDirectory`
/// when the terminal runs beside the server as its admin, `path` the
/// `PATH` to look on.
pub(crate) fn find(server_folder: Option<&Path>, path: Option<&OsStr>) -> Option<Ffmpeg> {
    if let Some(bin) = server_folder.and_then(in_folder) {
        return Some(Ffmpeg { bin, found: "server" });
    }
    on_path(path).map(|bin| Ffmpeg { bin, found: "path" })
}

/// An ffmpeg found on this computer.
#[derive(Clone, Debug)]
pub(crate) struct Ffmpeg {
    bin: PathBuf,
    found: &'static str,
}

impl Convert for Ffmpeg {
    fn convert(
        &self,
        from: &Path,
        to: &Path,
        codec: TransferCodec,
        bitrate: TransferBitrate,
        stop: &AtomicBool,
    ) -> Result<(), String> {
        let mut command = Command::new(&self.bin);
        command.args(args(from, to, codec, bitrate));
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console of its own for a player started without one.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().map_err(|e| format!("ffmpeg didn't start: {e}"))?;
        // Its words read as they come, so a full pipe never holds it up.
        let words = child.stderr.take().map(|mut err| {
            std::thread::spawn(move || {
                let mut out = Vec::new();
                let _ = (&mut err).take(WORDS as u64).read_to_end(&mut out);
                let _ = std::io::copy(&mut err, &mut std::io::sink());
                String::from_utf8_lossy(&out).trim().to_string()
            })
        });
        let status = loop {
            if stop.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("stopped".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("ffmpeg: {e}")),
            }
        };
        let words = words.and_then(|w| w.join().ok()).unwrap_or_default();
        match status.success() {
            true => Ok(()),
            false => {
                let last = words.lines().last().unwrap_or("no words");
                Err(format!("ffmpeg failed ({status}): {last}"))
            }
        }
    }

    fn found(&self) -> &'static str {
        self.found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversion_asks_ffmpeg_what_mstream_asks_it() {
        let words = |codec, bitrate| -> Vec<String> {
            let a = args(Path::new("in.flac"), Path::new("out.x"), codec, bitrate);
            a.iter().map(|w| w.to_string_lossy().to_string()).collect()
        };
        let opus = words(TransferCodec::Opus, TransferBitrate::K192);
        let want = "-nostdin -hide_banner -loglevel error -y -i in.flac \
                    -vn -f ogg -acodec libopus -ab 192k out.x";
        assert_eq!(opus.join(" "), want);
        let mp3 = words(TransferCodec::Mp3, TransferBitrate::K320).join(" ");
        assert!(mp3.ends_with("-vn -f mp3 -acodec libmp3lame -ab 320k out.x"), "{mp3}");
    }

    #[test]
    fn ffmpeg_is_mstreams_own_first_then_the_paths_then_none() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("mstream-ffmpeg-find-{pid}"));
        let _ = std::fs::remove_dir_all(&root);
        let (server, first, second) = (root.join("server"), root.join("a"), root.join("b"));
        for dir in [&server, &first, &second] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(second.join(exe()), b"not really").unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();
        // Not in mStream's folder: the PATH's, the first folder that has it.
        let found = find(Some(&server), Some(&path)).expect("on the PATH");
        assert_eq!((found.bin.clone(), found.found()), (second.join(exe()), "path"));
        std::fs::write(server.join(exe()), b"not really").unwrap();
        let found = find(Some(&server), Some(&path)).expect("mStream's own");
        assert_eq!((found.bin.clone(), found.found()), (server.join(exe()), "server"));
        // A folder named by a relative path is never searched.
        let relative = std::env::join_paths([Path::new("b")]).unwrap();
        assert!(find(None, Some(&relative)).is_none());
        assert!(find(None, None).is_none(), "none: the files that need it are skipped");
        let missing = Ffmpeg { bin: root.join("gone").join(exe()), found: "path" };
        let stop = AtomicBool::new(false);
        let (opus, k192) = (TransferCodec::Opus, TransferBitrate::K192);
        let tried = missing.convert(&root.join("x"), &root.join("y"), opus, k192, &stop);
        assert!(tried.unwrap_err().contains("didn't start"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A real ffmpeg, when this computer has one on its PATH: a second of
    /// silence made into Opus, which the length check reads back.
    #[test]
    #[ignore = "needs an ffmpeg on the PATH"]
    fn a_real_ffmpeg_makes_an_opus_file() {
        let Some(ffmpeg) = find(None, std::env::var_os("PATH").as_deref()) else { return };
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("mstream-ffmpeg-real-{pid}"));
        std::fs::create_dir_all(&dir).unwrap();
        // A WAV of one second of 44.1 kHz stereo silence.
        let data = 44_100 * 4;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data as u32).to_le_bytes());
        wav.resize(wav.len() + data, 0);
        let (from, to) = (dir.join("in.wav"), dir.join("out.opus"));
        std::fs::write(&from, wav).unwrap();
        let stop = AtomicBool::new(false);
        ffmpeg.convert(&from, &to, TransferCodec::Opus, TransferBitrate::K96, &stop).expect("converted");
        let at = super::super::length::FileAt::open(&to).unwrap();
        let heard = super::super::length::heard(TransferCodec::Opus, &at).unwrap();
        assert!(heard.ms.abs_diff(1000) <= 30, "{heard:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
