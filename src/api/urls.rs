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

// The card transfer (src/api/transfer.rs, and the sync engine after it) is
// the only caller of the items below that carry this allow; until the
// engine lands, only the tests use them.
/// What an SD-card transfer asks `/transcode` for. The card's player decodes
/// it, not this one: the Core2 plays mStream's Opus sample-exact and
/// gapless, so Opus is allowed here, and only here. Kept apart from
/// [`TranscodeCodec`] so that type's rule stands where it matters: no URL
/// built from this one is ever handed to the playback engine.
///
/// mStream's third codec, ADTS AAC, is left out: the card's player reads
/// `.mp3`, `.flac` and `.opus` only.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferCodec {
    Opus,
    Mp3,
}

#[cfg_attr(not(test), allow(dead_code))]
impl TransferCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            TransferCodec::Opus => "opus",
            TransferCodec::Mp3 => "mp3",
        }
    }

    /// The Content-Type mStream answers this codec with (its `codecMap`).
    /// An unknown codec silently becomes the server's default, so this is
    /// the only way to know the bytes are the ones asked for.
    pub fn content_type(self) -> &'static str {
        match self {
            TransferCodec::Opus => "audio/ogg",
            TransferCodec::Mp3 => "audio/mpeg",
        }
    }

    /// Whether a response's `Content-Type` is this codec's: the media type
    /// alone, any parameters and case ignored.
    pub fn answers(self, content_type: &str) -> bool {
        let media = content_type.split(';').next().unwrap_or_default().trim();
        media.eq_ignore_ascii_case(self.content_type())
    }

    /// The extension a converted file takes on the card. Ogg Opus is
    /// `.opus` there, whatever the server's container calls itself.
    pub fn extension(self) -> &'static str {
        match self {
            TransferCodec::Opus => "opus",
            TransferCodec::Mp3 => "mp3",
        }
    }
}

impl std::str::FromStr for TransferCodec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "opus" => Ok(TransferCodec::Opus),
            "mp3" => Ok(TransferCodec::Mp3),
            other => Err(format!("unknown codec '{other}' — use opus or mp3")),
        }
    }
}

/// The bitrates `/transcode` takes. Any other value silently becomes the
/// server's default (96k), so a transfer always names one of these.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferBitrate {
    K64,
    K96,
    K128,
    K192,
}

#[cfg_attr(not(test), allow(dead_code))]
impl TransferBitrate {
    pub const ALL: [TransferBitrate; 4] =
        [TransferBitrate::K64, TransferBitrate::K96, TransferBitrate::K128, TransferBitrate::K192];

    pub fn as_str(self) -> &'static str {
        match self {
            TransferBitrate::K64 => "64k",
            TransferBitrate::K96 => "96k",
            TransferBitrate::K128 => "128k",
            TransferBitrate::K192 => "192k",
        }
    }

    pub fn kbps(self) -> u32 {
        match self {
            TransferBitrate::K64 => 64,
            TransferBitrate::K96 => 96,
            TransferBitrate::K128 => 128,
            TransferBitrate::K192 => 192,
        }
    }

    /// The one with exactly this rate, if the server offers it.
    pub fn from_kbps(kbps: u32) -> Option<TransferBitrate> {
        TransferBitrate::ALL.into_iter().find(|b| b.kbps() == kbps)
    }
}

/// Which copy of a cover `/album-art` serves: the image the scanner kept,
/// or one of the two copies it makes beside it (`scanOptions.compressImage`,
/// on by default). A copy that was never made is answered with the
/// original, so asking for one is always safe.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtSize {
    Full,
    /// `?compress=l`: at most 256 px on a side.
    Large,
    /// `?compress=s`: at most 92 px.
    Small,
}

impl ArtSize {
    fn compress(self) -> Option<&'static str> {
        match self {
            ArtSize::Full => None,
            ArtSize::Large => Some("l"),
            ArtSize::Small => Some("s"),
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
/// [`album_art_url`].
pub fn peer_art_url(server: &str, peer: i64, file: &str) -> Result<String, String> {
    build(server, &format!("api/v1/federation/peers/{peer}/art"), file).map(|url| url.to_string())
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

/// `{server}/transcode/{vpath}?codec=...&bitrate=...&token=...` for a card
/// transfer: codec and bitrate both pinned, as the server would otherwise
/// put its own defaults in their place without saying so.
#[cfg_attr(not(test), allow(dead_code))]
pub fn transfer_transcode_url(
    server: &str,
    vpath: &str,
    codec: TransferCodec,
    bitrate: TransferBitrate,
    token: Option<&str>,
) -> Result<String, String> {
    let mut url = build(server, "transcode", vpath)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("codec", codec.as_str());
        q.append_pair("bitrate", bitrate.as_str());
        if let Some(token) = token {
            q.append_pair("token", token);
        }
    }
    Ok(url.to_string())
}

/// [`album_art_url`] or [`peer_art_url`] for one [`ArtSize`]. The parent's
/// art proxy forwards `compress` to the peer, and nothing else.
#[cfg_attr(not(test), allow(dead_code))]
pub fn album_art_sized_url(
    server: &str,
    peer: Option<i64>,
    file: &str,
    size: ArtSize,
) -> Result<String, String> {
    let mut url = match peer {
        Some(peer) => build(server, &format!("api/v1/federation/peers/{peer}/art"), file)?,
        None => build(server, "album-art", file)?,
    };
    if let Some(compress) = size.compress() {
        url.query_pairs_mut().append_pair("compress", compress);
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
        let art = album_art_url("http://127.0.0.1:4242", "x.jpg").unwrap();
        assert_eq!(with_local_token(art, Some("lt9")), "http://127.0.0.1:4242/album-art/x.jpg?__lt=lt9");
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

    #[test]
    fn a_transfer_may_ask_for_opus_and_always_pins_both_settings() {
        let u = transfer_transcode_url(
            "http://host/mstream",
            "lib/Artist A/b.flac",
            TransferCodec::Opus,
            TransferBitrate::K192,
            None,
        )
        .unwrap();
        assert_eq!(u, "http://host/mstream/transcode/lib/Artist%20A/b.flac?codec=opus&bitrate=192k");
        let (mp3, low) = (TransferCodec::Mp3, TransferBitrate::K64);
        let u = transfer_transcode_url("http://host", "lib/b.flac", mp3, low, Some("t")).unwrap();
        assert_eq!(u, "http://host/transcode/lib/b.flac?codec=mp3&bitrate=64k&token=t");

        // The transfer's codec parses Opus; the player's still refuses it.
        assert_eq!("OPUS".parse::<TransferCodec>().unwrap(), TransferCodec::Opus);
        assert!("aac".parse::<TransferCodec>().is_err(), "the card's player reads no ADTS");
        assert!("opus".parse::<TranscodeCodec>().is_err());

        // The answer's type is the check the server's silent fallback needs.
        assert!(TransferCodec::Opus.answers("audio/ogg"));
        assert!(TransferCodec::Opus.answers("Audio/OGG; codecs=opus"));
        assert!(!TransferCodec::Opus.answers("audio/mpeg"), "the server's default came back instead");
        assert!(TransferCodec::Mp3.answers("audio/mpeg"));
        assert_eq!(TransferCodec::Opus.extension(), "opus");

        assert_eq!(TransferBitrate::from_kbps(128), Some(TransferBitrate::K128));
        assert_eq!(TransferBitrate::from_kbps(320), None, "the server has no 320k");
        let rates: Vec<&str> = TransferBitrate::ALL.iter().map(|b| b.as_str()).collect();
        assert_eq!(rates, ["64k", "96k", "128k", "192k"]);
    }

    #[test]
    fn art_comes_in_three_sizes_from_the_server_or_a_peers_proxy() {
        let full = album_art_sized_url("http://host", None, "ab12.jpeg", ArtSize::Full).unwrap();
        assert_eq!(full, album_art_url("http://host", "ab12.jpeg").unwrap(), "the full image is the plain URL");
        let large = album_art_sized_url("http://host/m/", None, "ab12.jpeg", ArtSize::Large).unwrap();
        assert_eq!(large, "http://host/m/album-art/ab12.jpeg?compress=l");
        let small = album_art_sized_url("http://parent:3000", Some(3), "ab12.png", ArtSize::Small).unwrap();
        assert_eq!(small, "http://parent:3000/api/v1/federation/peers/3/art/ab12.png?compress=s");
    }

    #[test]
    fn a_peers_bytes_and_art_come_through_the_parents_proxies() {
        let u = peer_media_url("http://parent:3000/", 3, "music/Söng.flac", Some("pt")).unwrap();
        assert_eq!(u, "http://parent:3000/api/v1/federation/peers/3/stream/music/S%C3%B6ng.flac?token=pt");
        let u = peer_art_url("http://parent:3000", 3, "cover.jpeg").unwrap();
        assert_eq!(u, "http://parent:3000/api/v1/federation/peers/3/art/cover.jpeg");
    }
}
