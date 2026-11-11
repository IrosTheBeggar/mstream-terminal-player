//! Names, paths and the canonical order (docs/METADATA.md 2.4.3, 2.6.8,
//! 2.8.2), and the album folder a thumbnail is keyed by (2.14.1).
//!
//! The order is a walk that, in each folder, lists its files by name and
//! then descends into its subfolders by name, names compared by their bytes
//! (`memcmp`, the shorter first on a common prefix). It is not the byte
//! order of the whole path strings: `A/x.mp3` comes before `A B/y.mp3`
//! (`41 2F` against `41 20`), and a file in path-string order is absent.
//! The rules for making a name (NFC, illegal characters, the length cut)
//! are the transfer's names layer's; these are the ones every contract file
//! is checked against.

use std::cmp::Ordering;

use super::prim::path_hash;

/// The longest path the device sees: `/music/` and the path, at most 255
/// bytes of UTF-8 (2.8.2).
pub(crate) const MAX_CARD_PATH: usize = 255;
/// The longest path relative to `/music` (`/music/` is 7 bytes).
pub(crate) const MAX_REL_PATH: usize = MAX_CARD_PATH - 7;

/// A name: non-empty, no `/`, not `.` or `..` (2.4.3). A NUL can't be in
/// one: names are NUL-terminated strings.
pub(crate) fn valid_name(name: &[u8]) -> bool {
    !name.is_empty() && name != b"." && name != b".." && !name.contains(&b'/') && !name.contains(&0)
}

/// A path relative to `/music`: non-empty, every component a valid name,
/// and `/music/` and it at most 255 bytes.
pub(crate) fn valid_rel_path(path: &[u8]) -> bool {
    !path.is_empty() && path.len() <= MAX_REL_PATH && path.split(|&c| c == b'/').all(valid_name)
}

/// Names by their bytes: `memcmp`, the shorter first on a common prefix.
pub(crate) fn compare_names(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

fn components(path: &str) -> impl Iterator<Item = &[u8]> {
    path.split('/').filter(move |_| !path.is_empty()).map(str::as_bytes)
}

/// Folders in pre-order: an ancestor before its descendants, siblings by
/// their names' bytes; `""` (`/music`) first.
pub(crate) fn compare_folder_paths(a: &str, b: &str) -> Ordering {
    components(a).cmp(components(b))
}

/// Two files' paths in the canonical order: their folders' pre-order
/// positions first, then their names. Within a folder its files come
/// before its subfolders.
pub(crate) fn compare_file_paths(a: &str, b: &str) -> Ordering {
    // Each folder component is (1, name) and the file's own name (0, name):
    // a file sorts before every subfolder of the folder it is in.
    fn key(path: &str) -> impl Iterator<Item = (u8, &[u8])> {
        let last = path.rfind('/').map_or(0, |k| k + 1);
        let folders = &path[..last.saturating_sub(1)];
        components(folders).map(|c| (1u8, c)).chain(std::iter::once((0u8, path[last..].as_bytes())))
    }
    key(a).cmp(key(b))
}

/// The folder a path is in (`""` for `/music`), and its own name.
pub(crate) fn split_last(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(k) => (&path[..k], &path[k + 1..]),
        None => ("", path),
    }
}

/// The album folder of a file (2.14.1, the device's `Album.folder`): the
/// folder at depth 2 below the file's root — the longest of `roots` (LIBR,
/// relative to `/music`) that contains it, else `/music` — so disc
/// subfolders belong to their album; a file at depth 0 or 1 has its own
/// folder. `Artist/Album/CD1/01.flac` gives `Artist/Album`; `x.mp3` gives
/// `""`, `/music`.
pub(crate) fn album_folder<'a>(rel: &'a str, roots: &[impl AsRef<str>]) -> &'a str {
    let mut root = "";
    for r in roots {
        let r = r.as_ref();
        let inside = rel.len() > r.len() && rel.starts_with(r) && rel.as_bytes()[r.len()] == b'/';
        if inside && r.len() > root.len() {
            root = r;
        }
    }
    let below = if root.is_empty() { rel } else { &rel[root.len() + 1..] };
    let folders: Vec<&str> = below.split('/').collect();
    let folders = &folders[..folders.len() - 1];
    if folders.len() <= 1 {
        return split_last(rel).0;
    }
    let depth2 = folders[0].len() + 1 + folders[1].len();
    let start = if root.is_empty() { 0 } else { root.len() + 1 };
    &rel[..start + depth2]
}

/// The transfer thumbnail of an album folder (2.14.1), inside `/.mstream`:
/// the folder named by its path hash's first hex digit, then the hash's
/// upper 32 bits as 8 uppercase hex digits — `/music/Artist/Album`
/// (B1F7E69FBD466B59) is `thumbs/B/B1F7E69F.565`. 8.3 names, so no long
/// name entries slow the folder.
pub(crate) fn thumb_path(folder_hash: u64) -> String {
    format!("thumbs/{:X}/{:08X}.565", folder_hash >> 60, folder_hash >> 32)
}

/// The thumbnail of the album folder `folder` (relative to `/music`).
pub(crate) fn thumb_path_of(folder: &str) -> String {
    thumb_path(path_hash(folder))
}
