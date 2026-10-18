//! An album's cover, as a sync from the user's own server elects it (the
//! firmware repo's docs/METADATA.md 2.14.2): the folder image first, as the
//! device would choose one — `cover`, `folder`, then `front` (`.jpg` or
//! `.jpeg`, any case), and when the folder is on this computer's disk the
//! largest other `.jpg` — looked for in the album folder, else in its first
//! track's folder; else the most common of the album's `album-art` names,
//! mStream's full image (not its `zl-`/`zs-` copies). It makes the album's
//! thumbnail ([`thumbnail`]: MPTH's pixels, which the PC decodes from any
//! image) and the cover put back into a converted file ([`baseline_jpeg`]:
//! the one embedded image kind the device decodes).
//!
//! **Over HTTP** mStream lists no images, so the named files are probed
//! with HEAD on `/media`, at most [`PROBES`] a folder, in rank order, the
//! first that answers taken; a folder the server's own folders on this
//! computer hold is listed instead, and never probed ([`Images`]). A cover
//! is fetched once for the few albums a run is on ([`Covers`] keeps the
//! last ones), so an album's converted files share it.

use std::collections::{BTreeMap, VecDeque};
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageReader, Limits};

use super::contract::mpth::{self, Pixels};
use super::contract::order::{album_folder, split_last};
use super::source::local::Images;
use crate::api::Client;
use crate::api::transfer::fetch::ProbeBy;
use crate::api::urls::ArtSize;

/// The names probed over HTTP, in the device's rank order: the spellings a
/// folder image mostly has, since a probe can't ask "any case".
pub(crate) const PROBES: [&str; 8] = [
    "cover.jpg",
    "Cover.jpg",
    "cover.jpeg",
    "folder.jpg",
    "Folder.jpg",
    "front.jpg",
    "Front.jpg",
    "COVER.JPG",
];
/// The largest image read for a cover: a cover is a few megabytes.
const MAX_IMAGE: u64 = 16 << 20;
/// The covers kept for the albums a run is on.
const KEPT: usize = 8;
/// The longest side of a cover put into a converted file: the player's
/// screen is 320 × 240, and its decoder scales by halves.
const EMBED_SIDE: u32 = 600;
/// The largest cover put into a converted file as it is: a 600-pixel JPEG
/// is a few hundred kilobytes at most, and past that the bytes are not
/// the picture's (metadata segments, bytes after its end), which every
/// converted file of the album would carry.
const EMBED_BYTES: usize = 512 * 1024;

/// An image a cover is made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Image {
    /// Where it came from, for the report: a folder's file, or mStream's art.
    pub(crate) from: String,
    pub(crate) bytes: Vec<u8>,
}

/// The server folders an album's cover is looked for in (2.14.2): the
/// album folder's, then its first track's when that is another. `card` is
/// the first track's card path, `server` its server path, `roots` the
/// card's library roots: the album folder is the card's (2.14.1), and its
/// server folder is the track's with as many folders taken off as the card
/// path has below the album folder.
pub(crate) fn folders(card: &str, server: &str, roots: &[String]) -> Vec<String> {
    let album = album_folder(card, roots);
    let depth = |p: &str| if p.is_empty() { 0 } else { p.split('/').count() };
    let below = depth(split_last(card).0).saturating_sub(depth(album));
    let track_folder = split_last(server).0.to_string();
    let mut album_server = track_folder.as_str();
    for _ in 0..below {
        album_server = split_last(album_server).0;
    }
    let mut out = vec![album_server.to_string()];
    if below > 0 {
        out.push(track_folder);
    }
    out.retain(|f| !f.is_empty());
    out
}

/// The most common of an album's `album-art` names (ties: the first by
/// name).
pub(crate) fn most_common<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in names.into_iter().filter(|n| !n.is_empty()) {
        *counts.entry(name).or_default() += 1;
    }
    let best = counts.values().copied().max()?;
    counts.into_iter().find(|&(_, n)| n == best).map(|(name, _)| name)
}

/// The covers of one server's albums.
pub(crate) struct Covers {
    client: Arc<Client>,
    /// The server's folders on this computer, when a run reads them.
    images: Option<Images>,
    /// The last albums' covers, by the folders they were looked for in and
    /// the art name.
    kept: VecDeque<((Vec<String>, Option<String>), Option<Rc<Image>>)>,
}

impl Covers {
    pub(crate) fn new(client: Arc<Client>, images: Option<Images>) -> Covers {
        Covers { client, images, kept: VecDeque::new() }
    }

    /// An album's cover: the image of the first of `folders` that has one,
    /// else mStream's `art`. None: neither.
    pub(crate) fn image(&mut self, folders: &[String], art: Option<&str>) -> Option<Rc<Image>> {
        let wanted = (folders.to_vec(), art.map(str::to_string));
        if let Some((_, found)) = self.kept.iter().find(|(k, _)| *k == wanted) {
            return found.clone();
        }
        let found = self.find(folders, art).map(Rc::new);
        if self.kept.len() == KEPT {
            self.kept.pop_front();
        }
        self.kept.push_back((wanted, found.clone()));
        found
    }

    fn find(&self, folders: &[String], art: Option<&str>) -> Option<Image> {
        for folder in folders {
            if let Some(local) = self.images.as_ref().and_then(|i| i.folder_image(folder)) {
                return Some(Image { from: format!("{folder}/{}", local.name), bytes: local.bytes });
            }
            if self.images.as_ref().is_some_and(|i| i.lists(folder)) {
                continue;
            }
            for name in PROBES {
                let path = format!("{folder}/{name}");
                let Ok(Some(probed)) = self.client.probe(&path, ProbeBy::Head) else { continue };
                if probed.len.is_some_and(|n| n > MAX_IMAGE) {
                    continue;
                }
                match self.client.read_range(&path, 0, MAX_IMAGE) {
                    Ok(read) if !read.bytes.is_empty() => {
                        return Some(Image { from: path, bytes: read.bytes });
                    }
                    _ => continue,
                }
            }
        }
        let art = art?;
        let bytes = self.client.album_art_sized(art, ArtSize::Full).ok().filter(|b| !b.is_empty())?;
        Some(Image { from: format!("album-art/{art}"), bytes })
    }
}

/// An album's thumbnail (MPTH's two slots), and the image's size; None for
/// an image the PC can't decode.
pub(crate) fn thumbnail(image: &Image) -> Option<(Pixels, u32)> {
    let pixels = mpth::from_image(&image.bytes).ok()?;
    Some((pixels, u32::try_from(image.bytes.len()).unwrap_or(u32::MAX)))
}

/// A JPEG's frame: whether it is baseline (SOF0, the one the device's
/// decoder reads), and its width and height. None: not a JPEG, or no frame
/// found.
fn jpeg_frame(bytes: &[u8]) -> Option<(bool, u32, u32)> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        let frame = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if frame {
            let h = bytes.get(at + 5..at + 7)?;
            let w = bytes.get(at + 7..at + 9)?;
            let (w, h) = (u16::from_be_bytes([w[0], w[1]]), u16::from_be_bytes([h[0], h[1]]));
            return Some((marker == 0xC0, w as u32, h as u32));
        }
        at += 2 + len;
    }
    None
}

/// A cover to put into a converted file: a baseline JPEG of at most
/// [`EMBED_SIDE`] a side. A baseline JPEG that size already goes as it
/// is, up to [`EMBED_BYTES`]; any other image is decoded, scaled down when
/// larger, and encoded again. None: an image the PC can't decode.
pub(crate) fn baseline_jpeg(image: &Image) -> Option<Vec<u8>> {
    if let Some((true, w, h)) = jpeg_frame(&image.bytes)
        && w.max(h) <= EMBED_SIDE
        && image.bytes.len() <= EMBED_BYTES
    {
        return Some(image.bytes.clone());
    }
    let mut reader = ImageReader::new(Cursor::new(&image.bytes)).with_guessed_format().ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(10_000);
    limits.max_alloc = Some(96 << 20);
    reader.limits(limits);
    let mut img = reader.decode().ok()?;
    if img.width().max(img.height()) > EMBED_SIDE {
        img = img.resize(EMBED_SIDE, EMBED_SIDE, FilterType::Triangle);
    }
    let rgb = img.into_rgb8();
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 88).encode_image(&rgb).ok()?;
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A test image: `w` × `h` of one colour, as PNG or baseline JPEG.
    pub(crate) fn picture(w: u32, h: u32, jpeg: bool) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 90]));
        let mut out = Vec::new();
        if jpeg {
            JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
        } else {
            img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        }
        out
    }

    #[test]
    fn an_albums_cover_is_looked_for_in_its_album_folder_first() {
        let roots = ["Rock".to_string()];
        // A disc subfolder: the album folder is two above the file's root.
        assert_eq!(
            folders("Artist/Album/CD1/01.flac", "lib/Artist/Album/CD1/01.flac", &[]),
            ["lib/Artist/Album", "lib/Artist/Album/CD1"]
        );
        assert_eq!(folders("Artist/Album/01.flac", "lib/Artist/Album/01.flac", &[]), ["lib/Artist/Album"]);
        // The vpath layout: the root is the library's folder.
        assert_eq!(
            folders("Rock/Artist/Album/CD2/x.mp3", "Rock/Artist/Album/CD2/x.mp3", &roots),
            ["Rock/Artist/Album", "Rock/Artist/Album/CD2"]
        );
        // A loose file at depth 1 has its own folder.
        assert_eq!(folders("Artist/x.mp3", "lib/Artist/x.mp3", &[]), ["lib/Artist"]);
        assert_eq!(most_common(["b.jpg", "a.jpg", "b.jpg", ""]), Some("b.jpg"));
        assert_eq!(most_common(["b.jpg", "a.jpg"]), Some("a.jpg"));
        assert_eq!(most_common([""]), None);
    }

    #[test]
    fn a_converted_files_cover_is_a_small_baseline_jpeg() {
        let small = Image { from: "f".into(), bytes: picture(300, 300, true) };
        assert_eq!(jpeg_frame(&small.bytes), Some((true, 300, 300)));
        assert_eq!(baseline_jpeg(&small).as_deref(), Some(small.bytes.as_slice()), "as it is");
        let big = Image { from: "f".into(), bytes: picture(1200, 800, true) };
        let made = baseline_jpeg(&big).unwrap();
        assert_eq!(jpeg_frame(&made), Some((true, 600, 400)));
        let png = Image { from: "f".into(), bytes: picture(100, 100, false) };
        assert_eq!(jpeg_frame(&baseline_jpeg(&png).unwrap()), Some((true, 100, 100)));
        assert_eq!(baseline_jpeg(&Image { from: "f".into(), bytes: b"no image".to_vec() }), None);
        // A small picture padded past the cap (a large metadata segment
        // after its start): made again, to the picture alone.
        let mut padded = small.bytes[..2].to_vec();
        for _ in 0..12 {
            padded.extend_from_slice(&[0xFF, 0xE1, 0xFF, 0xFF]);
            padded.resize(padded.len() + 0xFFFD, 0);
        }
        padded.extend_from_slice(&small.bytes[2..]);
        let padded = Image { from: "f".into(), bytes: padded };
        assert!(padded.bytes.len() > EMBED_BYTES);
        assert_eq!(jpeg_frame(&padded.bytes), Some((true, 300, 300)));
        let made = baseline_jpeg(&padded).unwrap();
        assert!(made.len() < 64 * 1024, "{} bytes", made.len());
        assert_eq!(jpeg_frame(&made), Some((true, 300, 300)));
        let (pixels, n) = thumbnail(&png).unwrap();
        assert_eq!((pixels.small.len(), n as usize), (40 * 40, png.bytes.len()));
    }
}
