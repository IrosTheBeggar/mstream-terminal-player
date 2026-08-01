//! Response types for the mStream v1 API.
//!
//! Hand-written against mStream's `docs/openapi.yaml` and verified against a
//! live server. Deliberately tolerant: every struct carries `#[serde(default)]`
//! and unknown fields are ignored, so a client built against one server version
//! keeps working against another that adds or drops fields. Only genuinely
//! load-bearing fields (a track's `filepath`) are required.

use serde::Deserialize;
use serde::Deserializer;

/// Accept an explicit JSON `null` where the server documents a nullable object,
/// yielding `T::default()` instead of failing the whole response.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(default)]
    pub vpaths: Vec<String>,
}

/// `GET /api/v1/ping` — the one-shot capability bootstrap.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ping {
    pub vpaths: Vec<String>,
    pub transcode: Option<TranscodeInfo>,
    #[serde(rename = "noFileModify")]
    pub no_file_modify: bool,
    #[serde(rename = "noUpload")]
    pub no_upload: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TranscodeInfo {
    /// The server's *default* codec — frequently `opus`, which this player
    /// cannot decode. Informational only: always request a codec explicitly
    /// (see `api::urls::TranscodeCodec`).
    #[serde(rename = "defaultCodec")]
    pub default_codec: Option<String>,
    #[serde(rename = "defaultBitrate")]
    pub default_bitrate: Option<String>,
}

/// Track metadata, as nested under `metadata` on library responses.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub track: Option<u32>,
    pub disk: Option<u32>,
    pub year: Option<i32>,
    /// Seconds. Feeds the engine's duration hint so remote tracks skip the
    /// probe fetch.
    pub duration: Option<f64>,
    pub rating: Option<u32>,
    pub bpm: Option<u32>,
    pub hash: Option<String>,
    #[serde(rename = "album-art")]
    pub album_art: Option<String>,
    #[serde(rename = "musical-key")]
    pub musical_key: Option<String>,
    #[serde(rename = "play-count")]
    pub play_count: Option<u64>,
}

impl TrackMetadata {
    /// Best available display title: the tag, else nothing (callers fall back
    /// to the filename).
    pub fn display_title(&self) -> Option<&str> {
        self.title.as_deref().filter(|s| !s.is_empty())
    }
}

/// A library track. `filepath` is the vpath-prefixed path used to build
/// `/media/...` URLs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    pub filepath: String,
    #[serde(default, deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl Track {
    /// Filename component of the vpath, for display when tags are missing.
    pub fn file_name(&self) -> &str {
        self.filepath.rsplit('/').next().unwrap_or(&self.filepath)
    }

    pub fn display_name(&self) -> String {
        match (self.metadata.artist.as_deref(), self.metadata.display_title()) {
            (Some(a), Some(t)) if !a.is_empty() => format!("{a} - {t}"),
            (_, Some(t)) => t.to_string(),
            _ => self.file_name().to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Album {
    pub name: Option<String>,
    pub artist: Option<String>,
    pub year: Option<i32>,
    pub album_art_file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ArtistsResponse {
    pub artists: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AlbumsResponse {
    pub albums: Vec<Album>,
}

// ── File explorer ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DirListing {
    pub path: String,
    pub directories: Vec<DirEntry>,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DirEntry {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileEntry {
    pub name: String,
    /// File extension as classified by the server ("mp3", "flac", ...).
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

// ── Search ──────────────────────────────────────────────────────────────────

/// `POST /api/v1/db/search` — five parallel result categories.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchResults {
    pub artists: Vec<SearchGroup>,
    pub albums: Vec<SearchGroup>,
    /// Tracks whose *title* matched.
    pub title: Vec<SearchTrack>,
    /// Tracks whose *filepath* matched.
    pub files: Vec<SearchTrack>,
    /// Tracks whose stored lyrics matched.
    pub lyrics: Vec<SearchTrack>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.artists.is_empty()
            && self.albums.is_empty()
            && self.title.is_empty()
            && self.files.is_empty()
            && self.lyrics.is_empty()
    }
}

/// An artist or album hit. (The server also sends `filepath: false` on these
/// rows as a "not a track" sentinel; we distinguish by type instead.)
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchGroup {
    pub name: String,
    pub album_art_file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchTrack {
    pub name: String,
    pub filepath: String,
    pub album_art_file: Option<String>,
    /// Documented nullable — the track row can vanish between match and
    /// enrichment.
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

// ── Playlists ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PlaylistSummary {
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_search_shape() {
        // Captured verbatim from a live mStream response.
        let json = r#"{"artists":[],"albums":[],"title":[],"files":[{"name":"testlib/sine-60s.mp3",
            "album_art_file":null,"filepath":"testlib/sine-60s.mp3","metadata":{"title":null,
            "artist":null,"album":null,"album-art":null,"year":null,"track":null,"disk":null,
            "duration":60.029,"rating":null,"bpm":null,"musical-key":"A minor","genres":[],
            "has-lyrics":false,"has-synced-lyrics":false,"replaygain-track":null}}],"lyrics":[]}"#;
        let r: SearchResults = serde_json::from_str(json).unwrap();
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].filepath, "testlib/sine-60s.mp3");
        assert_eq!(r.files[0].metadata.duration, Some(60.029));
        assert_eq!(r.files[0].metadata.musical_key.as_deref(), Some("A minor"));
        assert!(!r.is_empty());
    }

    #[test]
    fn tolerates_null_metadata_object() {
        let t: SearchTrack =
            serde_json::from_str(r#"{"name":"x","filepath":"lib/x.mp3","metadata":null}"#).unwrap();
        assert!(t.metadata.title.is_none());
    }

    #[test]
    fn tolerates_missing_and_unknown_fields() {
        // Only filepath is load-bearing; everything else may be absent, and
        // fields we don't model must not break parsing.
        let t: Track =
            serde_json::from_str(r#"{"filepath":"lib/a.mp3","brand_new_field":{"x":1}}"#).unwrap();
        assert_eq!(t.file_name(), "a.mp3");
        assert_eq!(t.display_name(), "a.mp3");
    }

    #[test]
    fn display_name_prefers_tags() {
        let t: Track = serde_json::from_str(
            r#"{"filepath":"lib/a.mp3","metadata":{"title":"Song","artist":"Band"}}"#,
        )
        .unwrap();
        assert_eq!(t.display_name(), "Band - Song");

        let t: Track =
            serde_json::from_str(r#"{"filepath":"lib/a.mp3","metadata":{"title":"Solo"}}"#).unwrap();
        assert_eq!(t.display_name(), "Solo");
    }

    #[test]
    fn parses_live_file_explorer_shape() {
        let json = r#"{"path":"/testlib/","files":[{"type":"flac","name":"noise-60s.flac"}],
            "directories":[{"name":"Terminal Test"}]}"#;
        let d: DirListing = serde_json::from_str(json).unwrap();
        assert_eq!(d.path, "/testlib/");
        assert_eq!(d.files[0].kind.as_deref(), Some("flac"));
        assert_eq!(d.directories[0].name, "Terminal Test");
    }

    #[test]
    fn parses_ping_transcode_defaults() {
        let json = r#"{"vpaths":["testlib"],"transcode":{"defaultCodec":"opus",
            "defaultBitrate":"96k"},"noMkdir":false,"noUpload":false}"#;
        let p: Ping = serde_json::from_str(json).unwrap();
        assert_eq!(p.vpaths, vec!["testlib"]);
        assert_eq!(p.transcode.unwrap().default_codec.as_deref(), Some("opus"));
    }
}
