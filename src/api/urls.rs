//! Stream URL construction.
//!
//! Both `/media/<vpath>/<rest>` and `/transcode/<vpath>/<rest>` take the auth
//! token as a `?token=` query parameter, which is what makes a bare URL
//! self-contained enough to hand straight to the playback engine.

use reqwest::Url;

/// Codecs we are willing to ask mStream to transcode to.
///
/// Deliberately does **not** include opus, even though the server supports it
/// and uses it as its *default*: symphonia (our decoder) cannot decode opus,
/// so a transcode URL without an explicit codec yields an unplayable stream.
/// Making opus unrepresentable here is the enforcement of that rule — see
/// PLAN.md audit finding #14.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscodeCodec {
    Mp3,
    Aac,
}

impl TranscodeCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            TranscodeCodec::Mp3 => "mp3",
            TranscodeCodec::Aac => "aac",
        }
    }
}

impl std::str::FromStr for TranscodeCodec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mp3" => Ok(TranscodeCodec::Mp3),
            "aac" => Ok(TranscodeCodec::Aac),
            "opus" => Err(
                "opus cannot be decoded by this player — use mp3 or aac".to_string(),
            ),
            other => Err(format!("unknown codec '{other}' — use mp3 or aac")),
        }
    }
}

/// Join `prefix` + a library-relative path onto a server base URL, encoding
/// each path segment. Handles bases that live under a subpath (reverse
/// proxies) and bases with or without a trailing slash.
fn build(server: &str, prefix: &str, vpath: &str) -> Result<Url, String> {
    let mut url = Url::parse(server).map_err(|e| format!("invalid server URL: {e}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "invalid server URL: cannot be a base".to_string())?;
        segments.pop_if_empty();
        segments.push(prefix);
        for part in vpath.split('/').filter(|p| !p.is_empty()) {
            segments.push(part);
        }
    }
    Ok(url)
}

/// `{server}/media/{vpath}?token=...` — the raw file, byte-for-byte.
pub fn media_url(server: &str, vpath: &str, token: Option<&str>) -> Result<String, String> {
    let mut url = build(server, "media", vpath)?;
    if let Some(token) = token {
        url.query_pairs_mut().append_pair("token", token);
    }
    Ok(url.to_string())
}

/// `{server}/album-art/{file}` — the cover the server extracted and cached,
/// named by the `album-art` field in track metadata.
///
/// No `?token=`: unlike the stream URLs this one is fetched by the client
/// itself, so the token can travel in the header where it stays out of
/// server logs.
pub fn album_art_url(server: &str, file: &str) -> Result<String, String> {
    build(server, "album-art", file).map(|url| url.to_string())
}

/// `{server}/transcode/{vpath}?codec=...&bitrate=...&token=...`
///
/// The codec is always sent explicitly; see [`TranscodeCodec`].
pub fn transcode_url(
    server: &str,
    vpath: &str,
    codec: TranscodeCodec,
    bitrate: Option<&str>,
    token: Option<&str>,
) -> Result<String, String> {
    let mut url = build(server, "transcode", vpath)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("codec", codec.as_str());
        if let Some(bitrate) = bitrate {
            q.append_pair("bitrate", bitrate);
        }
        if let Some(token) = token {
            q.append_pair("token", token);
        }
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_path_segments() {
        let u = media_url(
            "http://localhost:3000",
            "music/Some Artist/Söng #1.flac",
            Some("tok123"),
        )
        .unwrap();
        assert_eq!(
            u,
            "http://localhost:3000/media/music/Some%20Artist/S%C3%B6ng%20%231.flac?token=tok123"
        );
    }

    #[test]
    fn honors_server_subpath_and_trailing_slash() {
        let u = media_url("http://host/mstream/", "lib/a.mp3", Some("t")).unwrap();
        assert_eq!(u, "http://host/mstream/media/lib/a.mp3?token=t");
        let u = media_url("http://host/mstream", "lib/a.mp3", Some("t")).unwrap();
        assert_eq!(u, "http://host/mstream/media/lib/a.mp3?token=t");
    }

    #[test]
    fn omits_token_when_absent() {
        // Public-mode servers (no users configured) need no token at all.
        let u = media_url("http://host", "lib/a.mp3", None).unwrap();
        assert_eq!(u, "http://host/media/lib/a.mp3");
    }

    #[test]
    fn album_art_url_carries_no_token() {
        let u = album_art_url("http://host/mstream", "b0445bafc2e9a817.jpeg").unwrap();
        assert_eq!(u, "http://host/mstream/album-art/b0445bafc2e9a817.jpeg");
    }

    #[test]
    fn transcode_always_pins_codec() {
        let u = transcode_url(
            "http://host",
            "lib/a.flac",
            TranscodeCodec::Mp3,
            Some("192k"),
            Some("t"),
        )
        .unwrap();
        assert_eq!(u, "http://host/transcode/lib/a.flac?codec=mp3&bitrate=192k&token=t");

        // No bitrate → server default bitrate, but codec is still explicit.
        let u = transcode_url("http://host", "lib/a.flac", TranscodeCodec::Aac, None, None).unwrap();
        assert_eq!(u, "http://host/transcode/lib/a.flac?codec=aac");
    }

    #[test]
    fn opus_is_rejected_at_parse_time() {
        assert!("opus".parse::<TranscodeCodec>().is_err());
        assert_eq!("MP3".parse::<TranscodeCodec>().unwrap(), TranscodeCodec::Mp3);
        assert_eq!("aac".parse::<TranscodeCodec>().unwrap(), TranscodeCodec::Aac);
    }

    #[test]
    fn rejects_bad_server_url() {
        assert!(media_url("not a url", "a.mp3", None).is_err());
    }
}
