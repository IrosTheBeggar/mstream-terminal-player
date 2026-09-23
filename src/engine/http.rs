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
// cache: nothing persists. At most two *live* tracks have files at once —
// the one playing and, while a crossfade prepares or blends, the one
// coming up (Phase C) — plus, briefly, the spools of cancelled prepares:
// cancellation is dropping the opener's receiver, and the opener holds
// its file until the open completes or OPEN_TIMEOUT expires, so queue
// churn inside the prepare window can hold three or four for a few
// seconds. By default the files would land in the OS temp dir —
// RAM-backed tmpfs on many Linux systems — so main() points us at a real
// cache directory instead (see config::spool_dir).

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
        // Once per process, not per track — and through `stderrln!`, so the
        // one telling costs nothing when the TUI owns the terminal.
        static WARNED: Once = Once::new();
        WARNED.call_once(|| {
            crate::stderrln!(
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

// ── Timeouts ────────────────────────────────────────────────────────────────
//
// The engine holds its state lock while a stream opens, so every wait in
// open() needs a floor under it — pause, stop and seek all queue behind that
// lock, and an unbounded wait here is a player that can't even be told to
// stop (finding #18). Two bounds, because a dead server stalls us at two
// points:
//
//   * CONNECT_TIMEOUT — the TCP connect. No help against Quick Connect,
//     where the connect is to our own loopback bridge and succeeds even
//     when the tunnel behind it is gone.
//   * OPEN_TIMEOUT — all of open(): request, response headers, download
//     start. This is the one a dead tunnel actually hits.
//
// Deliberately absent: a read or total timeout on the reqwest client. The
// body is stream-download's job — its watchdog abandons a read that goes 5s
// without a byte and reconnects (`Settings::retry_timeout`), so a client
// read_timeout would either lose that race or, set tighter, turn every
// recoverable blip into a dead track. A total timeout is simply wrong for
// streaming: a ten-minute track takes ten minutes to download. What stays
// unbounded here is a stream that goes silent *after* the headers —
// reconnection retries forever by design. The one place that patience must
// not reach is the audio thread's own open: `START_TIMEOUT` in the engine
// puts the deadline on that whole attempt, probe included, rather than a
// clock on reads here that would race the watchdog.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(not(test))]
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// The test build waits this out against a socket that really stalls, and
/// nobody wants the full ten seconds in every run of the suite.
#[cfg(test)]
const OPEN_TIMEOUT: Duration = Duration::from_millis(400);

/// Hosts allowed to present a certificate the OS won't vouch for — written
/// when a session whose saved entry opted in connects (`tui::dispatch`
/// sees the flag ride past on the Connect/Login command), read per open.
/// Host-scoped, never process-wide: every other server's streams stay on
/// the verified client below.
static TRUSTED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();

pub(crate) fn trust_server(server_url: &str) {
    let Ok(url) = server_url.parse::<Url>() else { return };
    let Some(host) = url.host_str() else { return };
    let set = TRUSTED.get_or_init(Default::default);
    set.lock().unwrap_or_else(|e| e.into_inner()).insert(host.to_ascii_lowercase());
}

fn trusted(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    TRUSTED.get().is_some_and(|set| {
        set.lock().unwrap_or_else(|e| e.into_inner()).contains(&host.to_ascii_lowercase())
    })
}

/// The verified client's twin for trusted hosts, kept apart so a
/// self-signed server never loosens anyone else's TLS. Same pool rule —
/// see the comment below for why streams never reuse a connection.
fn insecure_client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .pool_max_idle_per_host(0)
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(|e| format!("failed to build http client: {e}"))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                // Never reuse a kept-alive connection. A pooled connection
                // assumes the far end is still there, and through the Quick
                // Connect bridge that assumption failed silently: the tunnel
                // held the client-side TCP open after the server's side was
                // gone, the pool offered the corpse to the next open, and
                // the request sat waiting for headers until OPEN_TIMEOUT —
                // which read as "crossfade didn't happen" whenever a prepare
                // fired within the pool's idle window of the previous
                // download finishing (the listening-session trace, fixed
                // alongside the bridge itself). Streams gain nothing from
                // reuse — an open per track, a connection per open.
                .pool_max_idle_per_host(0)
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
    let client =
        if trusted(&url) { insecure_client()?.clone() } else { client()?.clone() };
    runtime::block_on(async move {
        let open = async {
            let stream = HttpStream::new(client, url)
                .await
                // Redacted: reqwest embeds the full URL in its error text,
                // and stream URLs carry the auth token as a query parameter
                // — which would otherwise walk into the flight recorder and
                // the UI toast (pre-merge review).
                .map_err(|e| redact_queries(&format!("request failed: {e}")))?;
            let content_length = stream.content_length();
            let reader =
                StreamDownload::from_stream(stream, spool_provider(), Settings::default())
                    .await
                    .map_err(|e| redact_queries(&format!("stream init failed: {e}")))?;
            Ok((reader, content_length))
        };
        // Timing out abandons the future, which aborts the request in
        // flight. Safe to abandon: every await in there runs before the
        // download task is spawned (spawn-to-return has no await point), so
        // a timeout can't orphan a task or its spool file.
        match tokio::time::timeout(OPEN_TIMEOUT, open).await {
            Ok(opened) => opened,
            Err(_) => Err(format!(
                "no answer from the server after {}s",
                OPEN_TIMEOUT.as_secs()
            )),
        }
    })?
}

/// Strip query strings from URLs embedded in third-party error text. Our
/// own messages go through [`redact_source`]; this covers the messages we
/// only relay — reqwest and stream-download print the URL they failed on,
/// token and all. Anything from a `?` to the next delimiter goes; a `?`
/// in prose costs a few characters of someone else's sentence, which is
/// the right price for never writing a token to disk.
fn redact_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(q) = rest.find('?') {
        out.push_str(&rest[..q]);
        out.push_str("?<redacted>");
        let after = &rest[q + 1..];
        let end = after.find([' ', ')', '"', '\'']).unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
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
    fn trust_is_scoped_to_the_one_host_that_opted_in() {
        trust_server("https://Attic.local:3000");
        let at = |u: &str| trusted(&u.parse::<Url>().unwrap());
        assert!(at("https://attic.local:3000/media/a.mp3?token=t"), "case-folded");
        assert!(at("https://attic.local:8443/x"), "trust names the host, not the port");
        assert!(!at("https://office.local:3000/media/a.mp3"), "no one else loosens");

        // Junk registers nothing — and breaks nothing.
        trust_server("not a url");
        assert!(!at("https://office.local:3000/media/a.mp3"));
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
    fn a_second_open_never_reuses_the_first_connection() {
        use std::io::{Read, Write};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        // A server that answers exactly one request per connection and then
        // holds the socket open in silence — the shape the Quick Connect
        // bridge presented when the server behind it had hung up. A pooled
        // client offers that connection to its next request and waits out
        // its whole timeout; a pool-free client opens fresh and succeeds.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let conns = Arc::new(AtomicUsize::new(0));
        let counter = conns.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let _ = stream.read(&mut head);
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nRIFF",
                    );
                    // Hold the socket open, deaf: no FIN for the pool to
                    // notice, no answer for a reused request.
                    std::thread::sleep(Duration::from_secs(5));
                });
            }
        });

        let url = format!("http://{addr}/one.wav");
        let first = open(&url);
        assert!(first.is_ok(), "{:?}", first.err());
        drop(first);
        let second = open(&url);
        assert!(
            second.is_ok(),
            "second open hung on a pooled connection: {:?}",
            second.err()
        );
        assert_eq!(conns.load(Ordering::SeqCst), 2, "each open dials fresh");
    }

    #[test]
    fn a_server_that_never_answers_fails_the_open_instead_of_hanging() {
        // Bound but never accepted: on loopback the handshake still
        // completes into the listen backlog, so the connect succeeds and
        // then nothing ever comes back — the shape of a Quick Connect
        // bridge whose tunnel has died, the case CONNECT_TIMEOUT can never
        // catch. Without OPEN_TIMEOUT this call does not return.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let started = std::time::Instant::now();
        let err = open(&format!("http://{addr}/track.flac")).unwrap_err();
        let waited = started.elapsed();

        assert!(err.contains("no answer from the server"), "{err}");
        // OPEN_TIMEOUT is 400ms in the test build; the ceiling is loose
        // because a busy CI box wakes timers late, and the claim being
        // tested is bounded-at-all, not sharp-at-400.
        assert!(waited < Duration::from_secs(5), "took {waited:?}");
        drop(listener);
    }

    #[test]
    fn relayed_error_text_loses_its_query_strings() {
        assert_eq!(
            redact_queries("request failed: error sending request for url \
                            (http://127.0.0.1:9/a.mp3?token=SECRET)"),
            "request failed: error sending request for url \
                            (http://127.0.0.1:9/a.mp3?<redacted>)"
        );
        assert_eq!(redact_queries("no urls here"), "no urls here");
        assert_eq!(redact_queries("odd? prose survives"), "odd?<redacted> prose survives");
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
