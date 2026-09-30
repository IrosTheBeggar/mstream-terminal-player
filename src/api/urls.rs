//! Stream URL construction.
//!
//! Both `/media/<vpath>/<rest>` and `/transcode/<vpath>/<rest>` take the auth
//! token as a `?token=` query parameter, which is what makes a bare URL
//! self-contained enough to hand straight to the playback engine.

use url::Url;

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
        for part in prefix.split('/').filter(|p| !p.is_empty()) {
            segments.push(part);
        }
        for part in vpath.split('/').filter(|p| !p.is_empty()) {
            segments.push(part);
        }
    }
    Ok(url)
}

/// Append the loopback token a tunnel bridge requires (`__lt=…`), when the
/// URL points at one. The shared tunnel client drops any local connection
/// whose first request line lacks it, so no other process on the machine can
/// use the bridge as a proxy — which means every URL the player builds for a
/// bridge, streams and art included, has to carry it.
pub fn with_local_token(url: String, token: Option<&str>) -> String {
    let Some(token) = token else { return url };
    let Ok(mut parsed) = Url::parse(&url) else { return url };
    parsed.query_pairs_mut().append_pair("__lt", token);
    parsed.to_string()
}

/// `{server}/media/{vpath}?token=...` — the raw file, byte-for-byte.
pub fn media_url(server: &str, vpath: &str, token: Option<&str>) -> Result<String, String> {
    let mut url = build(server, "media", vpath)?;
    if let Some(token) = token {
        url.query_pairs_mut().append_pair("token", token);
    }
    Ok(url.to_string())
}

/// `{server}/api/v1/federation/peers/{peer}/stream/{vpath}?token=...` — a
/// federated peer's bytes through its parent's proxy, which forwards Range
/// so seeking works and has no transcode (contract clause 27). The token
/// is the parent's.
pub fn peer_media_url(
    server: &str,
    peer: i64,
    vpath: &str,
    token: Option<&str>,
) -> Result<String, String> {
    let mut url = build(server, &format!("api/v1/federation/peers/{peer}/stream"), vpath)?;
    if let Some(token) = token {
        url.query_pairs_mut().append_pair("token", token);
    }
    Ok(url.to_string())
}

/// `{server}/api/v1/federation/peers/{peer}/art/{file}` — a peer's cover
/// through the parent's art proxy; the token travels in the header, as for
/// [`album_art_url`], and so does `small` (the proxy forwards `compress`).
pub fn peer_art_url(server: &str, peer: i64, file: &str, small: bool) -> Result<String, String> {
    build(server, &format!("api/v1/federation/peers/{peer}/art"), file).map(|url| sized(url, small))
}

/// `{server}/album-art/{file}` — the cover the server extracted and cached,
/// named by the `album-art` field in track metadata.
///
/// No `?token=`: unlike the stream URLs this one is fetched by the client
/// itself, so the token can travel in the header where it stays out of
/// server logs.
///
/// `small` asks for the server's own 256 px copy (`?compress=l`), which
/// mStream writes beside every cover it scans and serves in place of the
/// original — falling back to the original when it has none. A wall cell
/// or a queue row never draws more than that, and the originals are
/// commonly 1000 px and hundreds of kilobytes apiece (performance audit
/// #92). Not `s`: its 92 px is below the 128 px thumbnail kept here.
pub fn album_art_url(server: &str, file: &str, small: bool) -> Result<String, String> {
    build(server, "album-art", file).map(|url| sized(url, small))
}

/// The `compress=l` pair on a small ask — its own pair, ahead of the
/// loopback token [`with_local_token`] appends.
fn sized(mut url: Url, small: bool) -> String {
    if small {
        url.query_pairs_mut().append_pair("compress", "l");
    }
    url.to_string()
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
    fn the_loopback_token_rides_every_shape_and_nothing_else() {
        let media = media_url("http://127.0.0.1:4242", "lib/a.mp3", Some("t")).unwrap();
        assert_eq!(
            with_local_token(media, Some("lt9")),
            "http://127.0.0.1:4242/media/lib/a.mp3?token=t&__lt=lt9"
        );
        let art = album_art_url("http://127.0.0.1:4242", "x.jpg", false).unwrap();
        assert_eq!(with_local_token(art, Some("lt9")), "http://127.0.0.1:4242/album-art/x.jpg?__lt=lt9");
        let art = album_art_url("http://127.0.0.1:4242", "x.jpg", true).unwrap();
        assert_eq!(with_local_token(art, Some("lt9")), "http://127.0.0.1:4242/album-art/x.jpg?compress=l&__lt=lt9");
        // A direct server has no gate, so nothing is appended.
        let plain = media_url("http://host:3000", "lib/a.mp3", None).unwrap();
        assert_eq!(with_local_token(plain.clone(), None), plain);
    }

    #[test]
    fn omits_token_when_absent() {
        // Public-mode servers (no users configured) need no token at all.
        let u = media_url("http://host", "lib/a.mp3", None).unwrap();
        assert_eq!(u, "http://host/media/lib/a.mp3");
    }

    #[test]
    fn album_art_url_carries_no_token() {
        let u = album_art_url("http://host/mstream", "b0445bafc2e9a817.jpeg", false).unwrap();
        assert_eq!(u, "http://host/mstream/album-art/b0445bafc2e9a817.jpeg");
    }

    #[test]
    fn a_small_cover_asks_for_the_servers_own_256px_copy() {
        // Performance audit #92: mStream serves `zl-<file>` for
        // `?compress=l` and the original when it has none.
        let u = album_art_url("http://host/mstream", "b0445bafc2e9a817.jpeg", true).unwrap();
        assert_eq!(u, "http://host/mstream/album-art/b0445bafc2e9a817.jpeg?compress=l");
        let u = peer_art_url("http://parent:3000", 3, "cover.jpeg", true).unwrap();
        assert_eq!(u, "http://parent:3000/api/v1/federation/peers/3/art/cover.jpeg?compress=l");
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

    #[test]
    fn a_peers_bytes_and_art_come_through_the_parents_proxies() {
        let u = peer_media_url("http://parent:3000/", 3, "music/Söng.flac", Some("pt")).unwrap();
        assert_eq!(u, "http://parent:3000/api/v1/federation/peers/3/stream/music/S%C3%B6ng.flac?token=pt");
        let u = peer_art_url("http://parent:3000", 3, "cover.jpeg", false).unwrap();
        assert_eq!(u, "http://parent:3000/api/v1/federation/peers/3/art/cover.jpeg");
    }
}
