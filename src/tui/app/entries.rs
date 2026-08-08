//! Turning what a server said into the rows a pane draws.
//!
//! Pure data in, `Vec<Entry>` out, with one exception: the two search views
//! read the hits the app is holding, which is state rather than an argument.
//! These carry their own history of server quirks -- a listing that reports
//! a folder as a file, tags written hierarchically, playlist files indexed
//! alongside audio -- and that history reads better collected than
//! interleaved between the event handler and the keymap (audit #61).

use super::*;
/// The Library tab's mode menu — static, so opening the tab costs no request.
pub(super) fn library_root_entries() -> Vec<Entry> {
    [
        ("Artists", LibraryNode::Artists),
        ("Albums", LibraryNode::Albums),
        ("Genres", LibraryNode::Genres),
        ("Recently Added", LibraryNode::Recent),
        // Last because the four above are ways the tags cut the library and
        // this is the one you cut yourself — not because it matters least.
        ("Playlists", LibraryNode::Playlists),
    ]
    .into_iter()
    .map(|(label, node)| Entry::Node { label: label.to_string(), node })
    .collect()
}

pub(super) fn album_label(album: &Album) -> String {
    let name = album.name.as_deref().unwrap_or("(untitled album)");
    let year = album.year.map(|y| format!(" ({y})")).unwrap_or_default();
    match album.artist.as_deref() {
        Some(artist) if !artist.is_empty() => format!("{artist} — {name}{year}"),
        _ => format!("{name}{year}"),
    }
}

pub(super) fn genre_label(genre: &Genre) -> String {
    match genre.track_count {
        Some(count) => format!("{} ({count})", genre.name),
        None => genre.name.clone(),
    }
}

/// Rows for a loaded library view. Every one of these sits below the mode
/// menu, so they all get a ".." to climb back out.
/// How many hits each class holds, in menu order.
pub(super) fn search_counts(results: &crate::api::types::SearchResults) -> [usize; 5] {
    [
        results.artists.len(),
        results.albums.len(),
        results.title.len(),
        results.files.len(),
        results.lyrics.len(),
    ]
}

impl App {
    /// The class menu: what matched, and how many. Classes that matched
    /// nothing are left out -- a row saying zero is a row you have to read to
    /// learn it was not worth reading.
    pub(super) fn search_root_entries(&self) -> Vec<Entry> {
        let Some(hits) = &self.search_hits else {
            return Vec::new();
        };
        let counts = search_counts(hits);
        SEARCH_CLASSES
            .iter()
            .zip(counts)
            .filter(|(_, n)| *n > 0)
            .map(|(class, n)| Entry::Search {
                label: class.title().to_string(),
                detail: n.to_string(),
                node: SearchNode::Class(*class),
            })
            .collect()
    }

    /// The hits inside whichever class is open. Artists and albums become the
    /// same nodes the Library tab drills, because that is what they are.
    pub(super) fn search_class_entries(&self) -> Vec<Entry> {
        let (Some(hits), SearchNode::Class(class)) = (&self.search_hits, self.search_node())
        else {
            return Vec::new();
        };
        let track_rows = |rows: &[crate::api::types::SearchTrack]| {
            rows.iter()
                .map(|hit| {
                    let track =
                        Track { filepath: hit.filepath.clone(), metadata: hit.metadata.clone() };
                    Entry::Track { label: track.display_name(), track: Box::new(track) }
                })
                .collect::<Vec<_>>()
        };

        let mut entries = vec![Entry::Parent];
        match class {
            SearchClass::Artists => entries.extend(hits.artists.iter().map(|group| Entry::Node {
                label: group.name.clone(),
                node: LibraryNode::Artist(group.name.clone()),
            })),
            SearchClass::Albums => entries.extend(hits.albums.iter().map(|group| Entry::Node {
                label: group.name.clone(),
                node: LibraryNode::Album { name: group.name.clone(), artist: None },
            })),
            SearchClass::Titles => entries.extend(track_rows(&hits.title)),
            SearchClass::Files => entries.extend(track_rows(&hits.files)),
            SearchClass::Lyrics => entries.extend(track_rows(&hits.lyrics)),
        }
        entries
    }
}

pub(super) fn entries_from_library(data: LibraryData) -> Vec<Entry> {
    let mut entries = vec![Entry::Parent];
    match data {
        LibraryData::Artists(artists) => entries.extend(artists.into_iter().map(|name| {
            Entry::Node { label: name.clone(), node: LibraryNode::Artist(name) }
        })),
        LibraryData::Albums(albums) => entries.extend(albums.into_iter().map(|album| {
            let label = album_label(&album);
            let node = LibraryNode::Album {
                name: album.name.unwrap_or_default(),
                artist: album.artist,
            };
            Entry::Node { label, node }
        })),
        LibraryData::Genres(genres) => entries.extend(genres.into_iter().map(|genre| {
            let label = genre_label(&genre);
            Entry::Node { label, node: LibraryNode::Genre(genre.name) }
        })),
        // A row that opens into its tracks — the same shape an artist or a
        // genre is, which is the whole reason playlists moved in here.
        LibraryData::Playlists(playlists) => {
            entries.extend(playlists.into_iter().map(|playlist| Entry::Node {
                label: playlist.name.clone(),
                node: LibraryNode::Playlist(playlist.name),
            }))
        }
        LibraryData::Tracks(tracks) => entries.extend(tracks.into_iter().map(|track| {
            Entry::Track { label: track.display_name(), track: Box::new(track) }
        })),
    }
    entries
}

/// The model writes hierarchical tags — "Electronic---Dubstep". In a list of
/// artists similar to each other the prefix is the same on every row, so only
/// the leaf carries information; it is also the difference between two tags
/// fitting on a line and none of them fitting.
pub(crate) fn tidy_tag(tag: &str) -> &str {
    tag.rsplit("---").next().unwrap_or(tag).trim()
}

/// Join a directory prefix and an entry name into a library path.
pub(super) fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// `root` is the path with nothing above it worth offering — empty for the
/// list of libraries, or the one library on a server that has only one.
pub(super) fn entries_from_listing(listing: &DirListing, root: &str) -> Vec<Entry> {
    let prefix = listing.path.trim_matches('/');
    let mut entries = Vec::new();
    if !prefix.is_empty() && prefix != root {
        entries.push(Entry::Parent);
    }
    for dir in &listing.directories {
        entries.push(Entry::Dir {
            label: dir.name.clone(),
            path: qualify(prefix, &dir.name),
        });
    }
    for file in &listing.files {
        // A playlist file is a list of tracks, not a track. The server
        // indexes them all the same, and `Enter` queues everything on screen,
        // so leaving one here puts something undecodable in the queue.
        if !is_audio(file.kind.as_deref()) {
            continue;
        }
        // The server's own filepath when it sent one: it is the canonical
        // form, and it is what these tags were looked up under. Falling back
        // to the joined path keeps listings without metadata working exactly
        // as before.
        let tags = file.metadata.as_ref();
        let filepath = tags
            .map(|m| m.filepath.clone())
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| qualify(prefix, &file.name));
        // The label stays the filename — this is the view of what is on disk,
        // and that is what people are looking for here. The tags ride along
        // for the queue, the now-playing screen and Auto-DJ, which all read
        // them off the track rather than the row.
        entries.push(Entry::Track {
            label: file.name.clone(),
            track: Box::new(Track {
                filepath,
                metadata: tags.and_then(|m| m.metadata.clone()).unwrap_or_default(),
            }),
        });
    }
    entries
}

/// Whether the file explorer should offer this as something to play.
///
/// mStream indexes playlist files alongside audio — its ping even reports
/// `m3u: false` under `supportedAudioFiles`, and its own Auto-DJ picker
/// excludes them with the note that a client cannot stream one. The file
/// browser is the one place they still reach a queue.
///
/// Anything unrecognised is treated as audio: a format this player cannot
/// decode should fail loudly when played, not vanish from the listing.
pub(super) fn is_audio(kind: Option<&str>) -> bool {
    !matches!(
        kind.map(str::to_ascii_lowercase).as_deref(),
        Some("m3u" | "m3u8" | "pls" | "cue" | "xspf" | "asx")
    )
}
