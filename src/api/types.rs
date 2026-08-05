//! Response types for the mStream v1 API.
//!
//! Hand-written against mStream's `docs/openapi.yaml` and verified against a
//! live server. Deliberately tolerant: every struct carries `#[serde(default)]`
//! and unknown fields are ignored, so a client built against one server version
//! keeps working against another that adds or drops fields. Only genuinely
//! load-bearing fields (a track's `filepath`) are required.

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;

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

    // Discovery features. All are off by default in mStream, and an older
    // server omits the field entirely — which `#[serde(default)]` turns into
    // `false`, the same answer for the same reason.
    /// The local audio-embedding index: similar tracks and similar artists.
    pub discovery: bool,
    /// Great-circle paths between two tracks' embeddings (Sonic Journey).
    #[serde(rename = "discoveryPath")]
    pub discovery_path: bool,
    /// Similarity against peers' local snapshot copies.
    #[serde(rename = "discoveryP2p")]
    pub discovery_p2p: bool,
    /// Similarity federated out to paired servers.
    #[serde(rename = "federationDiscovery")]
    pub federation_discovery: bool,
}

/// What this server can actually do, lifted out of [`Ping`].
///
/// The rule everywhere downstream is **no flag, no probe**: every one of these
/// is disabled by default server-side, and asking anyway earns a 403 that
/// looks like a failure but isn't. Carried as a small `Copy` value so any code
/// deciding whether to offer a feature can hold one without ceremony.
///
/// Each feature is gated on its own flag rather than inferred from another —
/// the server reports them separately, so the client believes them separately.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub discovery: bool,
    pub discovery_path: bool,
    pub discovery_p2p: bool,
    pub federation_discovery: bool,
}

impl From<&Ping> for Capabilities {
    fn from(ping: &Ping) -> Self {
        Capabilities {
            discovery: ping.discovery,
            discovery_path: ping.discovery_path,
            discovery_p2p: ping.discovery_p2p,
            federation_discovery: ping.federation_discovery,
        }
    }
}

impl Capabilities {
    /// Feature names to show someone asking what this server offers.
    pub fn enabled_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.discovery {
            names.push("similarity");
        }
        if self.discovery_path {
            names.push("sonic journey");
        }
        if self.discovery_p2p {
            names.push("p2p discovery");
        }
        if self.federation_discovery {
            names.push("federated discovery");
        }
        names
    }
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
    /// Whether the server holds words for this track, and whether they carry
    /// timings. Cheap to read off a listing, and it saves offering a lyrics
    /// view for a track that has none.
    #[serde(rename = "has-lyrics")]
    pub has_lyrics: bool,
    #[serde(rename = "has-synced-lyrics")]
    pub has_synced_lyrics: bool,
    /// What the file is, as the scanner read it — "mp3", "flac". Not the
    /// extension: a mislabelled file is described by what is inside it.
    pub format: Option<String>,
    /// Bits per second, so 320000 rather than 320.
    pub bitrate: Option<u64>,
    #[serde(rename = "sample-rate")]
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    /// Only lossless formats carry one, so its absence is itself a fact.
    #[serde(rename = "bit-depth")]
    pub bit_depth: Option<u16>,
    #[serde(rename = "file-size")]
    pub file_size: Option<u64>,
    /// The scanner returns a list; most files name one.
    pub genres: Vec<String>,
    /// How many tracks the release has, for "3 of 12".
    #[serde(rename = "track-total")]
    pub track_total: Option<u32>,
    #[serde(rename = "disc-total")]
    pub disc_total: Option<u32>,
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

/// Seconds as `m:ss`, and `--:--` for anything that isn't a length.
///
/// Here beside [`Track::display_name`] because it is the same kind of thing:
/// how the client renders a fact about a track. It lived in `cmd_library`,
/// which meant the whole render layer imported the CLI smoke-test harness to
/// print a duration (finding #66).
pub fn fmt_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".to_string();
    }
    let total = seconds.round() as u64;
    format!("{}:{:02}", total / 60, total % 60)
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

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Genre {
    pub name: String,
    pub track_count: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GenresResponse {
    pub genres: Vec<Genre>,
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
    /// Present only when the listing asked for metadata, and doubly wrapped on
    /// the wire — see [`FileMetadata`].
    pub metadata: Option<FileMetadata>,
}

/// The `metadata` a file-explorer listing carries when `pullMetadata` is set.
///
/// Two layers on purpose, and both matter: the outer one is the server's own
/// canonical `<vpath>/<relative path>` for the file, and the inner one is the
/// tags — `null` for a file that is on disk but not in the database, which is
/// every playlist file and anything added since the last scan.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FileMetadata {
    pub filepath: String,
    pub metadata: Option<TrackMetadata>,
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

// ── Auto-DJ ─────────────────────────────────────────────────────────────────

/// A tempo window, inclusive. Also what [`crate::dj::bpm_windows`] works in —
/// it had its own identical `BpmRange` and a closure to convert between them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BpmWindow {
    pub min: f64,
    pub max: f64,
}

/// Request body for `POST /api/v1/db/random-songs`, which returns a single
/// track. Empty fields are omitted so the server's filter waterfall only sees
/// constraints we actually mean.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RandomSongRequest {
    /// Round-trip cursor: echo back what the previous response returned, and
    /// the server keeps the session from repeating itself.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_list: Vec<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bpm_ranges: Vec<BpmWindow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bpm_ranges_wide: Vec<BpmWindow>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub musical_keys: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignore_artists: Vec<String>,
    /// 1–10. Omitted when zero, which the server also reads as "no floor".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_rating: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub genres: Vec<String>,
    /// Only meaningful alongside `genres`: "whitelist" (default) or
    /// "blacklist". Note the deliberate asymmetry server-side — a whitelist
    /// blocks untagged tracks, a blacklist lets them through.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre_mode: Option<String>,

    // The sonic pool. The server's Joi schema binds these with `.and(...)`,
    // so sending one without the other is a 400 — see `with_sonic_pool`.
    /// 1–8 seed paths, averaged into a centroid when there is more than one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub similar_to: Vec<String>,
    /// Raw cosine floor, 0..1. A hard constraint: the server's waterfall
    /// relaxes tempo, key and artist *within* the pool and never widens past
    /// it, failing loudly instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_similarity: Option<f64>,
}

/// Most seed paths the server will average into a centroid.
pub const SIMILAR_TO_MAX: usize = 8;

impl RandomSongRequest {
    /// Attach the sonic pool, or leave it off entirely.
    ///
    /// Both fields go on together or neither does — the server enforces that
    /// with `.and('similarTo', 'minSimilarity')`, so half a pool is a 400
    /// rather than a looser search.
    pub fn with_sonic_pool(mut self, seeds: &[String], threshold: Option<f64>) -> Self {
        let seeds: Vec<String> = seeds.iter().take(SIMILAR_TO_MAX).cloned().collect();
        match (seeds.is_empty(), threshold) {
            (false, Some(threshold)) => {
                self.similar_to = seeds;
                self.min_similarity = Some(threshold);
            }
            _ => {
                self.similar_to.clear();
                self.min_similarity = None;
            }
        }
        self
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RandomSongsResponse {
    pub songs: Vec<Track>,
    #[serde(rename = "ignoreList")]
    pub ignore_list: Vec<u32>,
    /// Present only when the request carried a sonic pool.
    pub sonic: Option<SonicReport>,
}

/// What the sonic pool did on this pick — the feedback that makes the
/// tightness slider tunable rather than guesswork.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SonicReport {
    /// Cosine of the pick against the seed or centroid. Null when the pick
    /// somehow has no vector.
    pub similarity: Option<f64>,
    /// How many analysed tracks sit inside the threshold at all, before the
    /// other filters cut it down.
    #[serde(rename = "poolSize")]
    pub pool_size: u32,
}

/// `POST /api/v1/discovery/local/similar/tracks` — nearest neighbours in the
/// embedding space the discovery worker builds.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarTracksResponse {
    /// True when the seed exists but hasn't been embedded yet. Transient, not
    /// an error — the results list is simply empty.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: bool,
    pub results: Vec<SimilarTrack>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarTrack {
    pub filepath: String,
    /// Cosine similarity to the seed, 0..1.
    pub similarity: f64,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl SimilarTrack {
    pub fn into_track(self) -> Track {
        Track { filepath: self.filepath, metadata: self.metadata }
    }
}

/// `POST /api/v1/discovery/local/similar/artists` — artists whose overall
/// sound sits near the seed's, each with a way in.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarArtistsResponse {
    /// The seed artist has no embedded tracks yet.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: bool,
    /// The server stopped walking the ranking before filling the limit, so a
    /// short list means "we stopped looking", not "there is no more".
    pub capped: bool,
    pub results: Vec<SimilarArtist>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SimilarArtist {
    pub artist: String,
    pub similarity: f64,
    /// How many of this artist's tracks the discovery worker has embedded —
    /// a similarity drawn from three tracks deserves less trust than one
    /// drawn from fifty.
    #[serde(rename = "analyzedCount")]
    pub analyzed_count: u32,
    /// The model's own style guesses for this artist, not file tags.
    #[serde(rename = "genreTags", deserialize_with = "null_default")]
    pub genre_tags: Vec<String>,
    /// Up to two of this artist's tracks that sit closest to the *seed's*
    /// sound — playable doorways that continue what you were listening to,
    /// rather than whatever the artist is best known for.
    #[serde(rename = "entryPoints")]
    pub entry_points: Vec<Track>,
}

// ── Sonic Journey ───────────────────────────────────────────────────────────

/// `POST /api/v1/discovery/local/path` — a walk along the great-circle arc
/// between two tracks' embeddings, each waypoint snapped to a real track.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JourneyResponse {
    /// Which ends the discovery worker hasn't embedded yet. Per-end rather
    /// than one flag, so a message can name the end that is still waiting.
    #[serde(rename = "notAnalyzed")]
    pub not_analyzed: NotAnalyzed,
    /// Both seeds are included, so this *is* the queue. It can come up short
    /// when the library runs out of visible tracks — short is an answer, not
    /// an error.
    pub results: Vec<JourneyStop>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct NotAnalyzed {
    pub start: bool,
    pub end: bool,
}

impl NotAnalyzed {
    pub fn any(&self) -> bool {
        self.start || self.end
    }

    /// Which end to name in a message, when one of them is holding things up.
    pub fn which(&self) -> &'static str {
        match (self.start, self.end) {
            (true, true) => "neither end has",
            (true, false) => "the starting track hasn't",
            (false, true) => "the destination hasn't",
            (false, false) => "both ends have",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JourneyStop {
    pub filepath: String,
    /// Position along the arc: 0 at the start, 1 at the destination.
    pub t: f64,
    /// How close this track sits to the waypoint it stands in for.
    pub similarity: f64,
    #[serde(deserialize_with = "null_default")]
    pub metadata: TrackMetadata,
}

impl JourneyStop {
    pub fn to_track(&self) -> Track {
        Track { filepath: self.filepath.clone(), metadata: self.metadata.clone() }
    }

    /// Artist and title where they exist, the filename otherwise — the same
    /// rule [`Track::display_name`] follows.
    pub fn metadata_display(&self) -> String {
        self.to_track().display_name()
    }
}

/// Bounds the server puts on the journey length, which counts both seeds.
pub const JOURNEY_MIN_LENGTH: u32 = 4;
pub const JOURNEY_MAX_LENGTH: u32 = 32;
pub const JOURNEY_DEFAULT_LENGTH: u32 = 14;

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
    fn formats_durations() {
        assert_eq!(fmt_duration(0.0), "0:00");
        assert_eq!(fmt_duration(59.6), "1:00");
        assert_eq!(fmt_duration(60.029), "1:00");
        assert_eq!(fmt_duration(3725.0), "62:05");
        assert_eq!(fmt_duration(f64::NAN), "--:--");
        assert_eq!(fmt_duration(-1.0), "--:--");
    }

    #[test]
    fn parses_live_file_explorer_metadata() {
        // Captured verbatim from a live mStream with `pullMetadata: true`,
        // trimmed to the fields this player reads. Two layers of `metadata`
        // is not a typo — see [`FileMetadata`] — and the inner one really is
        // null for the playlist file sitting in the same folder.
        let json = r#"{"path":"/library/3-1 Remixes/13 Horrible Remixes/","directories":[],
            "files":[{"type":"mp3","name":"01 - My Rave.mp3","metadata":{
            "filepath":"library/3-1 Remixes/13 Horrible Remixes/01 - My Rave.mp3",
            "metadata":{"artist":"3-1 Remixes","album":"13 Horrible Remixes By 3-1","track":1,
            "disk":null,"title":"My Rave (Nid & Sancy)","duration":238.655,"year":2009,
            "album-art":null,"rating":null,"play-count":null,"bpm":130,"musical-key":"G major",
            "genres":["Progressive Electronic"],"bitrate":271000,"format":"mp3",
            "sample-rate":44100,"channels":2,"bit-depth":null,"file-size":8087296,
            "track-total":13,"disc-total":1,"audio-hash":"51dbe5b4","bpm-source":"essentia",
            "has-lyrics":false,"has-synced-lyrics":false}}},
            {"type":"m3u","name":"13 Horrible Remixes.m3u","metadata":{
            "filepath":"library/3-1 Remixes/13 Horrible Remixes/13 Horrible Remixes.m3u",
            "metadata":null}}]}"#;
        let listing: DirListing = serde_json::from_str(json).unwrap();

        let tags = listing.files[0].metadata.as_ref().unwrap();
        assert_eq!(tags.filepath, "library/3-1 Remixes/13 Horrible Remixes/01 - My Rave.mp3");
        let meta = tags.metadata.as_ref().unwrap();
        assert_eq!(meta.duration, Some(238.655));
        assert_eq!(meta.bpm, Some(130));
        assert_eq!(meta.musical_key.as_deref(), Some("G major"));
        assert_eq!(meta.title.as_deref(), Some("My Rave (Nid & Sancy)"));
        assert_eq!(meta.year, Some(2009));

        // What the file is. Every one of these rode along on the listing and
        // was thrown away for months because the struct did not name it —
        // `bitrate` and `genres` were sitting in this very capture, unread.
        assert_eq!(meta.format.as_deref(), Some("mp3"));
        assert_eq!(meta.bitrate, Some(271000), "bits per second, not kilobits");
        assert_eq!(meta.sample_rate, Some(44100));
        assert_eq!(meta.channels, Some(2));
        assert_eq!(meta.bit_depth, None, "lossy, so there is none to give");
        assert_eq!(meta.file_size, Some(8087296));
        assert_eq!(meta.genres, vec!["Progressive Electronic".to_string()]);
        assert_eq!(meta.track_total, Some(13));
        assert_eq!(meta.disc_total, Some(1));

        // Present but empty: on disk, not in the database.
        assert!(listing.files[1].metadata.as_ref().unwrap().metadata.is_none());
    }

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
        // The pre-`pullMetadata` shape, which is also what the fallback
        // request gets back: no `metadata` key at all.
        assert!(d.files[0].metadata.is_none());
    }

    #[test]
    fn parses_ping_transcode_defaults() {
        let json = r#"{"vpaths":["testlib"],"transcode":{"defaultCodec":"opus",
            "defaultBitrate":"96k"},"noMkdir":false,"noUpload":false}"#;
        let p: Ping = serde_json::from_str(json).unwrap();
        assert_eq!(p.vpaths, vec!["testlib"]);
        assert_eq!(p.transcode.unwrap().default_codec.as_deref(), Some("opus"));
    }

    #[test]
    fn the_sonic_pool_is_sent_whole_or_not_at_all() {
        let seeds = vec!["lib/a.mp3".to_string()];

        let body = serde_json::to_value(
            RandomSongRequest::default().with_sonic_pool(&seeds, Some(0.62)),
        )
        .unwrap();
        assert_eq!(body["similarTo"], serde_json::json!(["lib/a.mp3"]));
        assert_eq!(body["minSimilarity"], 0.62);

        // A threshold with no seed, or a seed with no threshold, would be a
        // 400 from the server's `.and(...)` — neither may reach the wire.
        for half in [
            RandomSongRequest::default().with_sonic_pool(&seeds, None),
            RandomSongRequest::default().with_sonic_pool(&[], Some(0.62)),
        ] {
            let body = serde_json::to_value(half).unwrap();
            assert!(body.get("similarTo").is_none(), "got {body}");
            assert!(body.get("minSimilarity").is_none(), "got {body}");
        }
    }

    #[test]
    fn more_seeds_than_the_server_averages_are_trimmed() {
        let seeds: Vec<String> = (0..20).map(|i| format!("lib/{i}.mp3")).collect();
        let request = RandomSongRequest::default().with_sonic_pool(&seeds, Some(0.5));
        assert_eq!(request.similar_to.len(), SIMILAR_TO_MAX, "the server caps this at 8");
        // The most recent picks are the ones worth anchoring to, and callers
        // pass them newest-first.
        assert_eq!(request.similar_to[0], "lib/0.mp3");
    }

    #[test]
    fn an_empty_request_sends_an_empty_body() {
        // Every filter is optional, and a filter we don't mean must not
        // appear — the server's waterfall branches on field presence.
        let body = serde_json::to_value(RandomSongRequest::default()).unwrap();
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn reads_the_sonic_report_when_the_server_sends_one() {
        let json = r#"{"songs":[],"ignoreList":[4,5],
            "sonic":{"similarity":0.7213,"poolSize":1247}}"#;
        let r: RandomSongsResponse = serde_json::from_str(json).unwrap();
        let sonic = r.sonic.unwrap();
        assert_eq!(sonic.similarity, Some(0.7213));
        assert_eq!(sonic.pool_size, 1247);

        // And copes when it doesn't: non-sonic picks omit the whole object.
        let r: RandomSongsResponse =
            serde_json::from_str(r#"{"songs":[],"ignoreList":[]}"#).unwrap();
        assert!(r.sonic.is_none());
    }

    #[test]
    fn reads_discovery_flags_from_a_live_ping() {
        // Captured verbatim from demo.mstream.io, which runs the local
        // embedding index but neither p2p nor federation.
        let json = r#"{"vpaths":["library"],"transcode":{"defaultCodec":"opus",
            "defaultBitrate":"96k"},"noMkdir":true,"noUpload":true,"noFileModify":true,
            "allowYoutubeDownload":false,"discovery":true,"discoveryPath":true,
            "discoveryP2p":false,"federationDiscovery":false}"#;
        let caps = Capabilities::from(&serde_json::from_str::<Ping>(json).unwrap());
        assert_eq!(
            caps,
            Capabilities {
                discovery: true,
                discovery_path: true,
                discovery_p2p: false,
                federation_discovery: false,
            }
        );
        assert_eq!(caps.enabled_names(), vec!["similarity", "sonic journey"]);
    }

    #[test]
    fn a_server_that_reports_nothing_is_treated_as_having_nothing() {
        // An older mStream omits these fields entirely. Absent must mean off,
        // or the client probes routes that answer 403 — which is exactly the
        // failure feature detection exists to avoid.
        let caps = Capabilities::from(&serde_json::from_str::<Ping>(r#"{"vpaths":[]}"#).unwrap());
        assert_eq!(caps, Capabilities::default());
        assert!(caps.enabled_names().is_empty());
    }
}
