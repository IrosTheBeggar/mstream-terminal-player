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

    /// Every byte of the test file is a function of its offset, so a read
    /// that came back from the wrong place (or from spool the download never
    /// wrote) cannot pass for the right one.
    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i.wrapping_mul(31) ^ (i >> 10)) as u8).collect()
    }

    /// A Range-capable server for `body` whose pace is chosen per request:
    /// `pace(start)` says how many bytes go out at once and how long the
    /// rest is then held back. Every request's `(start, end)` lands in the
    /// log (end exclusive); an inverted range is logged and refused with a
    /// 416, the way mStream's own server refuses one.
    fn range_server(
        body: Vec<u8>,
        pace: impl Fn(u64) -> (usize, Duration) + Send + Sync + 'static,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<(u64, u64)>>>) {
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let body = Arc::new(body);
        let pace = Arc::new(pace);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (body, pace, seen) = (body.clone(), pace.clone(), seen.clone());
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut head = [0u8; 2048];
                    let n = stream.read(&mut head).unwrap_or(0);
                    let head = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
                    let len = body.len() as u64;
                    let (start, end) = head
                        .lines()
                        .find_map(|line| line.strip_prefix("range: bytes="))
                        .and_then(|spec| spec.trim().split_once('-'))
                        .map(|(a, b)| {
                            let start = a.parse().unwrap_or(0);
                            let end = b.parse::<u64>().map(|b| (b + 1).min(len)).unwrap_or(len);
                            (start, end)
                        })
                        .unwrap_or((0, len));
                    seen.lock().unwrap().push((start, end));
                    if start > end {
                        let _ = stream.write_all(
                            b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        );
                        return;
                    }
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\n\
                             Content-Range: bytes {start}-{}/{len}\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n",
                            end.max(1) - 1,
                            end - start
                        )
                        .as_bytes(),
                    );
                    let part = &body[start as usize..end as usize];
                    let (burst, stall) = pace(start);
                    let burst = burst.min(part.len());
                    let _ = stream.write_all(&part[..burst]);
                    let _ = stream.flush();
                    std::thread::sleep(stall);
                    let _ = stream.write_all(&part[burst..]);
                });
            }
        });
        (format!("http://{addr}/patterned.bin"), log)
    }

    /// Seek, then read `len` bytes, the way a decoder resumes after a seek:
    /// how long that took, and what came back.
    fn seek_and_read(
        reader: &mut HttpReader,
        at: u64,
        len: usize,
    ) -> std::io::Result<(Duration, Vec<u8>)> {
        use std::io::{Read, Seek, SeekFrom};
        let started = std::time::Instant::now();
        reader.seek(SeekFrom::Start(at))?;
        let mut got = vec![0u8; len];
        reader.read_exact(&mut got)?;
        Ok((started.elapsed(), got))
    }

    // The three below pin the vendored stream-download fix (performance
    // audit #73). The held-back parts of the file stall for three seconds,
    // inside stream-download's five-second reconnect watchdog, so a reader
    // parked behind them takes three seconds and a reader woken when its
    // own bytes land takes milliseconds.
    const HELD_BACK: Duration = Duration::from_secs(3);

    #[test]
    fn a_seek_into_the_last_stretch_does_not_wait_for_the_rest_of_the_file() {
        // The tail is shorter than the prefetch, so its Range response ends
        // inside the prefetch window. Unpatched, nothing woke the reader
        // there: the loader went off to back-fill the file from the bottom
        // and the seek returned only when that crossed the tail, after the
        // whole file (29.4s at 8 Mbit/s in the audit's measurement).
        let body = patterned(2_000_000);
        let len = body.len() as u64;
        let tail = len - 20_000;
        let (url, _) = range_server(body.clone(), move |start| {
            if start >= tail { (usize::MAX, Duration::ZERO) } else { (300_000, HELD_BACK) }
        });
        let (mut reader, content_length) = open(&url).unwrap();
        assert_eq!(content_length, Some(len));
        // The probe's read: it waits out the open's prefetch, so the seek
        // below restarts one rather than cutting this one short.
        seek_and_read(&mut reader, 0, 1000).unwrap();

        let (took, got) = seek_and_read(&mut reader, tail, 20_000).unwrap();
        assert!(got == body[tail as usize..], "the tail's own bytes");
        assert!(took < Duration::from_millis(1500), "the seek waited {took:?}");
    }

    #[test]
    fn after_a_short_range_the_download_carries_on_ahead_of_the_reader() {
        // Seek forward (an island starts at 1 MB), then nudge back just
        // below it: the nudge's range fills the sliver up to the island and
        // ends. Unpatched, the loader then refilled the file's LOWEST gap,
        // so a reader that played through the sliver and the island found
        // nothing past it until the whole lower part had arrived (the audit
        // measured a 1s nudge back at 14.1s). The download belongs ahead of
        // the reader.
        let body = patterned(3_000_000);
        let island = 1_000_000u64;
        let sliver = island - 20_000;
        let (url, log) = range_server(body.clone(), move |start| {
            if start == island {
                // Enough for the seek's prefetch, then the island stops
                // growing: the gap above it is what the reader meets next.
                (300_000, Duration::from_secs(30))
            } else if start > island || start == sliver {
                (usize::MAX, Duration::ZERO)
            } else {
                (300_000, HELD_BACK)
            }
        });
        let (mut reader, _) = open(&url).unwrap();
        let (_, got) = seek_and_read(&mut reader, island, 1000).unwrap();
        assert!(got == body[island as usize..island as usize + 1000]);

        // Through the sliver, the island, and 100 KB past the island's end.
        let through = 20_000 + 300_000 + 100_000;
        let (took, got) = seek_and_read(&mut reader, sliver, through).unwrap();
        assert!(got == body[sliver as usize..sliver as usize + through]);
        assert!(took < Duration::from_millis(1500), "the reader waited {took:?}");

        let asked = log.lock().unwrap().clone();
        let after_sliver = asked.iter().skip_while(|r| r.0 != sliver).nth(1).copied();
        assert!(
            after_sliver.is_some_and(|(start, _)| start > island),
            "after the sliver the download went to {after_sliver:?}: {asked:?}"
        );
    }

    #[test]
    fn a_read_that_runs_off_its_island_fetches_what_comes_next() {
        // Islands at 1 MB and 2 MB; the download, done above 2 MB, has
        // wrapped round to the gap under 1 MB, which is slow. The reader
        // seeks back into the 1 MB island (already spooled, so no request
        // goes out) and reads on past its end. Only a seek ever redirected
        // the download, so that read used to wait for everything below the
        // island first: the FLAC bisection's last probe does exactly this
        // (a 90% seek on a FLAC without a SEEKTABLE waited 122s at 2 Mbit/s
        // once the prefetch was 64 KiB).
        let body = patterned(3_000_000);
        let (url, log) = range_server(body.clone(), move |start| match start {
            1_000_000 => (300_000, Duration::from_secs(30)),
            1_300_000..=2_999_999 => (usize::MAX, Duration::ZERO),
            _ => (300_000, HELD_BACK),
        });
        let (mut reader, _) = open(&url).unwrap();
        seek_and_read(&mut reader, 0, 1000).unwrap();
        seek_and_read(&mut reader, 1_000_000, 1000).unwrap();
        // Let the island's 300 KB land before the next seek cuts it off.
        std::thread::sleep(Duration::from_millis(200));
        seek_and_read(&mut reader, 2_000_000, 1000).unwrap();
        // The tail comes at once; give the download the moment it needs to
        // finish it and wrap round to the held-back gap below.
        std::thread::sleep(Duration::from_millis(200));

        let (took, got) = seek_and_read(&mut reader, 1_200_000, 200_000).unwrap();
        assert!(got == body[1_200_000..1_400_000], "the island and what follows it");
        assert!(took < Duration::from_millis(1500), "the read waited {took:?}: {:?}", log.lock());
    }

    #[test]
    fn seeks_around_islands_read_their_own_bytes_and_never_invert_a_range() {
        // A seek to 1 MB leaves an island; a second seek goes back to
        // 500 KB, below it, while the island's bytes are still pouring in;
        // a third lands above the island. Two things went wrong unpatched:
        //
        // * The chunk landing above the reader woke its backward seek
        //   before that seek's range was even asked for (the wake checked
        //   only "written past it"). The read then returned spool nobody
        //   had written, and the third seek, finding the second still
        //   queued, was dropped — the reader sat until the sequential
        //   download happened by. A race, so it runs a few times.
        // * With the download below the island, the gap search started at
        //   the writer, found the gap below the island, and asked for
        //   `bytes=2000000-999999`: a 416, a failed download, a dead track.
        let body = patterned(3_000_000);
        for round in 0..6 {
            let (url, log) = range_server(body.clone(), move |start| match start {
                1_000_000 | 500_000 => (300_000, Duration::from_secs(30)),
                2_000_000 => (usize::MAX, Duration::ZERO),
                _ => (300_000, HELD_BACK),
            });
            let (mut reader, _) = open(&url).unwrap();
            for at in [1_000_000, 500_000] {
                let (_, got) = seek_and_read(&mut reader, at, 1000).unwrap();
                assert!(got == body[at as usize..at as usize + 1000], "round {round}: at {at}");
            }

            let third = seek_and_read(&mut reader, 2_000_000, 10_000);
            let asked = log.lock().unwrap().clone();
            assert!(asked.iter().all(|(start, end)| start <= end), "inverted range: {asked:?}");
            let (took, got) = third.unwrap();
            assert!(got == body[2_000_000..2_010_000], "round {round}");
            assert!(took < Duration::from_millis(1500), "round {round}: waited {took:?}");
        }
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
