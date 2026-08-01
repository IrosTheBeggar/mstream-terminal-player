//! HTTP source support: a lazily-initialized tokio runtime + reqwest client
//! feeding stream-download readers (buffered `Read + Seek` over HTTP range
//! requests, spooled to a temp file).
//!
//! The runtime lives in a process-lifetime static so download tasks can never
//! outlive it; it is only spun up on the first HTTP source, so pure-local
//! serve mode (the jukebox) never starts tokio at all.

use std::sync::OnceLock;
use std::time::Duration;

use stream_download::http::reqwest::{Client, Url};
use stream_download::http::HttpStream;
use stream_download::source::SourceStream;
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload};

pub(crate) type HttpReader = StreamDownload<TempStorageProvider>;

/// Bound the time we can stall while opening a stream: the engine holds its
/// state lock during open, so a dead server must fail fast, not hang the
/// control API. (Reads after open happen on the runtime's own threads and
/// don't need a timeout.)
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct HttpOpener {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

impl HttpOpener {
    fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mstream-http")
            .enable_all()
            .build()
            .map_err(|e| format!("failed to start async runtime: {}", e))?;
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| format!("failed to build http client: {}", e))?;
        Ok(HttpOpener { runtime, client })
    }

    /// Open a URL as a seekable reader. Returns the reader plus the reported
    /// Content-Length (None means the server streamed a response of unknown
    /// size — seekable only within what has already downloaded).
    pub(crate) fn open(&self, url_str: &str) -> Result<(HttpReader, Option<u64>), String> {
        let url: Url = url_str.parse().map_err(|e| format!("invalid URL: {}", e))?;
        self.runtime.block_on(async {
            let stream = HttpStream::new(self.client.clone(), url)
                .await
                .map_err(|e| format!("request failed: {}", e))?;
            let content_length = stream.content_length();
            let reader =
                StreamDownload::from_stream(stream, TempStorageProvider::new(), Settings::default())
                    .await
                    .map_err(|e| format!("stream init failed: {}", e))?;
            Ok((reader, content_length))
        })
    }
}

/// Process-lifetime opener. An init failure (no runtime / no client) is
/// unrecoverable, so caching the error is fine.
pub(crate) fn opener() -> Result<&'static HttpOpener, String> {
    static OPENER: OnceLock<Result<HttpOpener, String>> = OnceLock::new();
    OPENER
        .get_or_init(HttpOpener::new)
        .as_ref()
        .map_err(|e| e.clone())
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
