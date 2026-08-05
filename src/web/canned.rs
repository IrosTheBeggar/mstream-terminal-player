//! The demo library: a small record collection that exists only as data.
//!
//! Shapes match what a live mStream sends (see api::types and its captured
//! fixtures) so the App exercises the same paths it does against a real
//! server — tags, durations, BPM and keys included, since the Auto-DJ panel
//! and the queue both read them.

use crate::api::types::{
    Album, DirEntry, DirListing, FileEntry, FileMetadata, Genre, JourneyStop, Ping,
    PlaylistSummary, SearchGroup, SearchResults, SearchTrack, SimilarArtist, Track,
    TrackMetadata, TranscodeInfo,
};
use crate::discovery::DiscoveredServer;
use crate::tui::worker::{LibraryData, LibraryNode};

pub const SERVER: &str = "https://demo.mstream.io";

/// One row of the collection, spelled tersely; `tracks()` inflates it.
struct Row {
    artist: &'static str,
    album: &'static str,
    year: i32,
    genre: &'static str,
    n: u32,
    title: &'static str,
    secs: f64,
    bpm: u32,
    key: &'static str,
}

const fn row(
    artist: &'static str,
    album: &'static str,
    year: i32,
    genre: &'static str,
    n: u32,
    title: &'static str,
    secs: f64,
    bpm: u32,
    key: &'static str,
) -> Row {
    Row { artist, album, year, genre, n, title, secs, bpm, key }
}

#[rustfmt::skip]
const ROWS: &[Row] = &[
    row("Cathode Rays", "Phosphor Burn", 2019, "Synthwave", 1, "Scanline",        243.0, 108, "A minor"),
    row("Cathode Rays", "Phosphor Burn", 2019, "Synthwave", 2, "Vertical Blank",  198.0, 112, "E minor"),
    row("Cathode Rays", "Phosphor Burn", 2019, "Synthwave", 3, "Afterimage",      274.0, 104, "A minor"),
    row("Cathode Rays", "Phosphor Burn", 2019, "Synthwave", 4, "Shadow Mask",     221.0, 110, "C major"),
    row("Cathode Rays", "Phosphor Burn", 2019, "Synthwave", 5, "Degauss",         305.0, 100, "G major"),
    row("The Segfaults", "Undefined Behaviour", 2021, "Punk", 1, "Null Deref",          124.0, 168, "E minor"),
    row("The Segfaults", "Undefined Behaviour", 2021, "Punk", 2, "Stack Smash",         141.0, 176, "A major"),
    row("The Segfaults", "Undefined Behaviour", 2021, "Punk", 3, "Core Dump",           157.0, 160, "D major"),
    row("The Segfaults", "Undefined Behaviour", 2021, "Punk", 4, "Panic at the Kernel", 133.0, 172, "E minor"),
    row("The Segfaults", "Undefined Behaviour", 2021, "Punk", 5, "Double Free",         119.0, 180, "B minor"),
    row("Mono Repo", "Workspace", 2023, "Lo-Fi", 1, "Lockfile",      172.0, 82, "F major"),
    row("Mono Repo", "Workspace", 2023, "Lo-Fi", 2, "Night Build",   204.0, 76, "D minor"),
    row("Mono Repo", "Workspace", 2023, "Lo-Fi", 3, "Vendored",      188.0, 84, "A minor"),
    row("Mono Repo", "Workspace", 2023, "Lo-Fi", 4, "Hot Reload",    166.0, 88, "C major"),
    row("Mono Repo", "Workspace", 2023, "Lo-Fi", 5, "Merge Quietly", 231.0, 72, "F major"),
    row("Fourier & The Transforms", "Frequency Domain", 2018, "Electronic", 1, "Windowing",  262.0, 124, "A minor"),
    row("Fourier & The Transforms", "Frequency Domain", 2018, "Electronic", 2, "Nyquist",    287.0, 128, "G minor"),
    row("Fourier & The Transforms", "Frequency Domain", 2018, "Electronic", 3, "Sidelobe",   244.0, 126, "D minor"),
    row("Fourier & The Transforms", "Frequency Domain", 2018, "Electronic", 4, "Phase Wrap", 296.0, 122, "A minor"),
    row("Fourier & The Transforms", "Frequency Domain", 2018, "Electronic", 5, "Inverse",    318.0, 118, "C major"),
];

fn filepath(r: &Row) -> String {
    format!("library/{}/{}/{:02} {}.mp3", r.artist, r.album, r.n, r.title)
}

fn metadata(r: &Row) -> TrackMetadata {
    TrackMetadata {
        title: Some(r.title.to_string()),
        artist: Some(r.artist.to_string()),
        album: Some(r.album.to_string()),
        track: Some(r.n),
        year: Some(r.year),
        duration: Some(r.secs),
        bpm: Some(r.bpm),
        musical_key: Some(r.key.to_string()),
        genres: vec![r.genre.to_string()],
        format: Some("mp3".to_string()),
        bitrate: Some(320_000),
        sample_rate: Some(44_100),
        channels: Some(2),
        track_total: Some(5),
        ..Default::default()
    }
}

pub fn tracks() -> Vec<Track> {
    ROWS.iter().map(|r| Track { filepath: filepath(r), metadata: metadata(r) }).collect()
}

/// Stable per-track id for the Auto-DJ ignore-list round trip.
pub fn track_id(track: &Track) -> Option<u32> {
    ROWS.iter().position(|r| filepath(r) == track.filepath).map(|i| i as u32)
}

pub fn ping() -> Ping {
    Ping {
        vpaths: vec!["library".to_string()],
        transcode: Some(TranscodeInfo {
            default_codec: Some("opus".to_string()),
            default_bitrate: Some("96k".to_string()),
        }),
        no_file_modify: true,
        no_upload: true,
        // What demo.mstream.io really reports: the embedding index and the
        // journey arc, nothing federated.
        discovery: true,
        discovery_path: true,
        ..Default::default()
    }
}

// ── File explorer ───────────────────────────────────────────────────────────

/// Resolve any browsed path against the artist/album tree. Tolerant of the
/// different spellings the app can ask with ("~", "", absolute, trailing
/// slash) — a stub that 404s on a slash defeats the demo.
pub fn listing(path: &str) -> DirListing {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != "~" && *p != "library")
        .collect();

    let artists = || {
        let mut names: Vec<&str> = ROWS.iter().map(|r| r.artist).collect();
        names.dedup();
        names
    };

    match parts.as_slice() {
        [] => DirListing {
            path: "/library/".to_string(),
            directories: artists().iter().map(|a| DirEntry { name: (*a).to_string() }).collect(),
            files: Vec::new(),
        },
        [artist] => {
            let mut albums: Vec<&Row> =
                ROWS.iter().filter(|r| r.artist == *artist && r.n == 1).collect();
            albums.dedup_by_key(|r| r.album);
            DirListing {
                path: format!("/library/{artist}/"),
                directories: albums
                    .iter()
                    .map(|r| DirEntry { name: r.album.to_string() })
                    .collect(),
                files: Vec::new(),
            }
        }
        [artist, album, ..] => DirListing {
            path: format!("/library/{artist}/{album}/"),
            directories: Vec::new(),
            files: ROWS
                .iter()
                .filter(|r| r.artist == *artist && r.album == *album)
                .map(|r| FileEntry {
                    name: format!("{:02} {}.mp3", r.n, r.title),
                    kind: Some("mp3".to_string()),
                    metadata: Some(FileMetadata {
                        filepath: filepath(r),
                        metadata: Some(metadata(r)),
                    }),
                })
                .collect(),
        },
    }
}

// ── Tag browsing ────────────────────────────────────────────────────────────

pub fn library_data(node: &LibraryNode) -> LibraryData {
    match node {
        LibraryNode::Root => LibraryData::Artists(Vec::new()),
        LibraryNode::Artists => {
            let mut names: Vec<String> = ROWS.iter().map(|r| r.artist.to_string()).collect();
            names.dedup();
            LibraryData::Artists(names)
        }
        LibraryNode::Artist(name) => LibraryData::Albums(albums_of(Some(name))),
        LibraryNode::Albums => LibraryData::Albums(albums_of(None)),
        LibraryNode::Album { name, artist } => LibraryData::Tracks(
            tracks()
                .into_iter()
                .filter(|t| {
                    t.metadata.album.as_deref() == Some(name)
                        && artist
                            .as_deref()
                            .is_none_or(|a| t.metadata.artist.as_deref() == Some(a))
                })
                .collect(),
        ),
        LibraryNode::Genres => LibraryData::Genres(genres()),
        LibraryNode::Genre(name) => LibraryData::Tracks(
            tracks().into_iter().filter(|t| t.metadata.genres.iter().any(|g| g == name)).collect(),
        ),
        LibraryNode::Recent => {
            // "Recently added": the newest album plus a scattering.
            let mut recent: Vec<Track> =
                tracks().into_iter().filter(|t| t.metadata.year == Some(2023)).collect();
            recent.extend(tracks().into_iter().filter(|t| t.metadata.track == Some(1)).take(3));
            LibraryData::Tracks(recent)
        }
    }
}

fn albums_of(artist: Option<&str>) -> Vec<Album> {
    let mut albums: Vec<Album> = Vec::new();
    for r in ROWS {
        if r.n != 1 || artist.is_some_and(|a| a != r.artist) {
            continue;
        }
        albums.push(Album {
            name: Some(r.album.to_string()),
            artist: Some(r.artist.to_string()),
            year: Some(r.year),
            album_art_file: None,
        });
    }
    albums
}

pub fn genres() -> Vec<Genre> {
    let mut genres: Vec<Genre> = Vec::new();
    for r in ROWS {
        match genres.iter_mut().find(|g| g.name == r.genre) {
            Some(genre) => genre.track_count = genre.track_count.map(|c| c + 1),
            None => genres.push(Genre { name: r.genre.to_string(), track_count: Some(1) }),
        }
    }
    genres
}

// ── Search, playlists, discovery ────────────────────────────────────────────

pub fn search(query: &str) -> SearchResults {
    let q = query.to_lowercase();
    let hit = |s: &str| s.to_lowercase().contains(&q);

    let mut results = SearchResults::default();
    if q.is_empty() {
        return results;
    }
    for r in ROWS {
        if hit(r.artist) && r.n == 1 && !results.artists.iter().any(|a| a.name == r.artist) {
            results
                .artists
                .push(SearchGroup { name: r.artist.to_string(), album_art_file: None });
        }
        if hit(r.album) && r.n == 1 {
            results.albums.push(SearchGroup { name: r.album.to_string(), album_art_file: None });
        }
        if hit(r.title) {
            results.title.push(SearchTrack {
                name: r.title.to_string(),
                filepath: filepath(r),
                album_art_file: None,
                metadata: metadata(r),
            });
        }
    }
    results
}

pub fn playlists() -> Vec<PlaylistSummary> {
    vec![
        PlaylistSummary { name: "Late Night Ops".to_string() },
        PlaylistSummary { name: "Ship It".to_string() },
    ]
}

pub fn playlist_tracks(name: &str) -> Vec<Track> {
    let wanted: &[&str] = match name {
        "Late Night Ops" => &["Night Build", "Afterimage", "Merge Quietly", "Phase Wrap", "Degauss"],
        "Ship It" => &["Null Deref", "Stack Smash", "Panic at the Kernel", "Hot Reload"],
        _ => &[],
    };
    wanted
        .iter()
        .filter_map(|title| {
            ROWS.iter()
                .find(|r| r.title == *title)
                .map(|r| Track { filepath: filepath(r), metadata: metadata(r) })
        })
        .collect()
}

/// Similar tracks for the Discover tab: same genre first, then neighbours by
/// BPM — close enough to look like an embedding space for a demo.
pub fn similar_tracks(seed: &Track) -> Vec<Track> {
    let genre = seed.metadata.genres.first().cloned().unwrap_or_default();
    let bpm = seed.metadata.bpm.unwrap_or(120) as i64;
    let mut rest: Vec<Track> =
        tracks().into_iter().filter(|t| t.filepath != seed.filepath).collect();
    rest.sort_by_key(|t| {
        let same_genre = t.metadata.genres.first().is_some_and(|g| *g == genre);
        let drift = (t.metadata.bpm.unwrap_or(120) as i64 - bpm).abs();
        (if same_genre { 0 } else { 1 }, drift)
    });
    rest.truncate(12);
    rest
}

pub fn similar_artists(seed: &Track) -> Vec<SimilarArtist> {
    let seed_artist = seed.metadata.artist.clone().unwrap_or_default();
    let mut names: Vec<&str> =
        ROWS.iter().map(|r| r.artist).filter(|a| *a != seed_artist).collect();
    names.dedup();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| SimilarArtist {
            artist: (*name).to_string(),
            similarity: 0.88 - 0.09 * i as f64,
            analyzed_count: 5,
            genre_tags: ROWS
                .iter()
                .find(|r| r.artist == *name)
                .map(|r| vec![r.genre.to_lowercase()])
                .unwrap_or_default(),
            entry_points: tracks()
                .into_iter()
                .filter(|t| t.metadata.artist.as_deref() == Some(name))
                .take(2)
                .collect(),
        })
        .collect()
}

pub fn journey(length: u32) -> Vec<JourneyStop> {
    let all = tracks();
    let n = (length as usize).clamp(2, all.len());
    (0..n)
        .map(|i| {
            let track = &all[i * (all.len() - 1) / (n - 1)];
            JourneyStop {
                filepath: track.filepath.clone(),
                t: i as f64 / (n - 1) as f64,
                similarity: 0.92 - 0.25 * (i as f64 / (n - 1) as f64),
                metadata: track.metadata.clone(),
            }
        })
        .collect()
}

pub fn lan_servers() -> Vec<DiscoveredServer> {
    vec![
        DiscoveredServer {
            name: "Living Room".to_string(),
            base_url: "http://192.168.1.71:3000".to_string(),
            version: Some("5.13.0".to_string()),
            quick_connect: true,
        },
        DiscoveredServer {
            name: "Attic NAS".to_string(),
            base_url: "http://192.168.1.4:3000".to_string(),
            version: Some("5.12.2".to_string()),
            quick_connect: false,
        },
    ]
}
