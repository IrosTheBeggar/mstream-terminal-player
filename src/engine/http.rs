//! HTTP source support: a reqwest client feeding stream-download readers
//! (buffered `Read + Seek` over HTTP range requests, spooled to a temp file).
//!
//! Async work runs on the shared runtime in `crate::runtime`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Once, OnceLock};
use std::time::Duration;

use stream_download::http::HttpStream;
use stream_download::http::reqwest::{Client, Url};
use stream_download::source::SourceStream;
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload};

use crate::runtime;

pub(crate) type HttpReader = StreamDownload<TempStorageProvider>;

// ── Spool placement ─────────────────────────────────────────────────────────
//
// Each playing track spools to one temp file (that is what makes seeking
// instant), deleted when the track stops. This is a scratch buffer, not a
// cache: only the current track has a file, nothing is prefetched, nothing
// persists. By default the file would land in the OS temp dir — RAM-backed
// tmpfs on many Linux systems — so main() points us at a real cache
// directory instead (see config::spool_dir).

/// Filename prefix that makes spool files recognisably ours, so the startup
/// sweep never touches anything else even in a shared directory.
const SPOOL_PREFIX: &str = "mstream-spool-";

/// Where spool files go, decided once at startup by main(). Unset (unit
/// tests, embedding) means the OS temp dir — the pre-A1 behavior.
static SPOOL_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

pub(crate) fn set_spool_dir(dir: Option<PathBuf>) {
    let _ = SPOOL_DIR.set(dir);
}

fn spool_provider() -> TempStorageProvider {
    provider_for(SPOOL_DIR.get().and_then(|dir| dir.as_deref()))
}

/// Storage for one track's spool. Falls back to the OS temp dir rather than
/// failing playback when the configured directory can't be created
/// (unplugged drive, permissions) — a worse location beats no audio.
fn provider_for(dir: Option<&Path>) -> TempStorageProvider {
    if let Some(dir) = dir {
        if fs::create_dir_all(dir).is_ok() {
            return TempStorageProvider::with_prefix_in(SPOOL_PREFIX, dir);
        }
        // Once per process, not per track: this fires mid-playback, where a
        // stderr line already costs a blemish on the raw-mode TUI.
        static WARNED: Once = Once::new();
        WARNED.call_once(|| {
            eprintln!(
                "[engine] cannot create spool dir {} — using the OS temp dir",
                dir.display()
            );
        });
    }
    TempStorageProvider::with_prefix(SPOOL_PREFIX)
}

/// Sweep leftover spool files. NamedTempFiles delete themselves on drop, so
/// under a normal shutdown this finds nothing; anything still wearing our
/// prefix was orphaned by a killed process. A concurrently *running*
/// instance's file also matches, but deleting it is harmless: on unix an
/// unlinked file lives on through its open descriptors, and on Windows the
/// handles were opened with delete sharing (the delete just pends until they
/// close) — or the delete is refused with a sharing violation and skipped.
pub(crate) fn clean_spool_dir(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(SPOOL_PREFIX));
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if ours && is_file && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Bound the time we can stall while opening a stream: the engine holds its
/// state lock during open, so a dead server must fail fast, not hang the
/// control API. (Reads after open happen on the runtime's own threads and
/// don't need a timeout.)
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .map_err(|e| format!("failed to build http client: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// Open a URL as a seekable reader. Returns the reader plus the reported
/// Content-Length (None means the server streamed a response of unknown size —
/// seekable only within what has already been downloaded, which is what
/// mStream's `/transcode` does on a cache miss).
pub(crate) fn open(url_str: &str) -> Result<(HttpReader, Option<u64>), String> {
    let url: Url = url_str.parse().map_err(|e| format!("invalid URL: {e}"))?;
    let client = client()?.clone();
    runtime::block_on(async move {
        let stream = HttpStream::new(client, url)
            .await
            .map_err(|e| format!("request failed: {e}"))?;
        let content_length = stream.content_length();
        let reader = StreamDownload::from_stream(stream, spool_provider(), Settings::default())
            .await
            .map_err(|e| format!("stream init failed: {e}"))?;
        Ok((reader, content_length))
    })?
}

pub(crate) fn is_http_url(source: &str) -> bool {
    let lower = source.get(..8).map(str::to_ascii_lowercase).unwrap_or_default();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Strip the query string from URLs before logging — mStream stream URLs
/// carry the auth token as a query parameter.
pub(crate) fn redact_source(source: &str) -> String {
    if is_http_url(source) {
        if let Some(i) = source.find('?') {
            return format!("{}?<redacted>", &source[..i]);
        }
    }
    source.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_http_urls() {
        assert!(is_http_url("http://x/y.mp3"));
        assert!(is_http_url("https://x/y.mp3"));
        assert!(is_http_url("HTTPS://x/y.mp3"));
        assert!(!is_http_url("C:\\Music\\y.mp3"));
        assert!(!is_http_url("/srv/music/y.mp3"));
        assert!(!is_http_url("ht"));
    }

    #[test]
    fn spool_files_land_in_the_configured_dir_and_vanish_after() {
        use stream_download::storage::StorageProvider;
        let dir = std::env::temp_dir().join("mstream-player-test-spool");
        let _ = fs::remove_dir_all(&dir);

        // provider_for creates the directory itself.
        let (reader, writer) = provider_for(Some(&dir)).into_reader_writer(None).unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(names[0].starts_with(SPOOL_PREFIX), "{names:?}");

        // The NamedTempFile lives inside the reader; dropping it deletes the
        // file — the "nothing persists" half of the contract.
        drop(writer);
        drop(reader);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "spool file should self-delete");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unusable_spool_dir_falls_back_instead_of_failing() {
        use stream_download::storage::StorageProvider;
        let file = std::env::temp_dir().join("mstream-player-test-notadir");
        fs::write(&file, b"x").unwrap();
        // A path *under a file* can't be created on any platform. Playback
        // must still get storage — just not there.
        let impossible = file.join("sub");
        let (_reader, _writer) =
            provider_for(Some(&impossible)).into_reader_writer(None).unwrap();
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn the_sweep_removes_only_our_spool_files() {
        let dir = std::env::temp_dir().join("mstream-player-test-sweep");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mstream-spool-orphan"), b"x").unwrap();
        fs::write(dir.join("keep.flac"), b"x").unwrap();
        fs::create_dir_all(dir.join("mstream-spool-oddly-named-dir")).unwrap();

        assert_eq!(clean_spool_dir(&dir), 1);
        assert!(!dir.join("mstream-spool-orphan").exists());
        assert!(dir.join("keep.flac").exists(), "not ours, not touched");
        assert!(dir.join("mstream-spool-oddly-named-dir").exists(), "dirs are never touched");

        let missing = std::env::temp_dir().join("mstream-player-test-no-such-dir");
        assert_eq!(clean_spool_dir(&missing), 0, "a missing dir is a no-op");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redacts_query_strings_only_for_urls() {
        assert_eq!(
            redact_source("http://h:3000/media/a.flac?token=secret"),
            "http://h:3000/media/a.flac?<redacted>"
        );
        assert_eq!(redact_source("http://h/a.flac"), "http://h/a.flac");
        // Windows paths can legally contain '?'-free anything; never touched.
        assert_eq!(redact_source("C:\\Music\\a?.flac"), "C:\\Music\\a?.flac");
    }
}
