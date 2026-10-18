//! Transfer thumbnails, `/.mstream/thumbs/<h>/<8 HEX>.565` (MPTH v1,
//! docs/METADATA.md 2.14.1): the device's own thumbnail format, byte for
//! byte, so it reads them with the code it has.
//!
//! One file an album folder, keyed by the folder's path hash
//! ([`super::order::thumb_path`]): a 24-byte header, then the cover at
//! 40×40 and at 96×96 in big-endian RGB565, centre-cropped and box-scaled.
//! The PC decodes any image the card's cover can't be — PNG, progressive
//! JPEG — so a thumbnail shows what the device would otherwise draw a
//! placeholder for. The scaling filter isn't pinned (PCs' JPEG decoders
//! differ anyway): two writers agree from given pixels ([`write`]), and
//! [`from_image`] is this one's way from an image's bytes to them.

use std::io::Cursor;

use image::{ImageReader, Limits};

use super::container::MPTH;
use super::prim::{u16_at, u32_at, u64_at};
use super::WriteError;

pub(crate) const VERSION: u16 = 1;
pub(crate) const SMALL: usize = 40;
pub(crate) const LARGE: usize = 96;
pub(crate) const HEADER_BYTES: usize = 24;
/// The whole file: the header and both slots' pixels.
pub(crate) const BYTES: usize = HEADER_BYTES + 2 * (SMALL * SMALL + LARGE * LARGE);
/// The header's two sizes, as a reader checks them.
const SIZES: u32 = SMALL as u32 | (LARGE as u32) << 16;

/// The most an image may hold for [`from_image`] to decode it: a cover is a
/// few megapixels; past this it is not one, or meant to exhaust memory (the
/// decoder's buffers stay under the cap, the RGB copy under three quarters
/// of it).
const MAX_SIDE: u32 = 10_000;
const MAX_DECODE_BYTES: u64 = 96 * 1024 * 1024;

/// One RGB565 value, `(R >> 3) << 11 | (G >> 2) << 5 | (B >> 3)`.
pub(crate) fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3
}

/// A thumbnail's pixels: the 40×40 slot and the 96×96 one, rows top to
/// bottom, as RGB565 values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pixels {
    pub(crate) small: Vec<u16>,
    pub(crate) large: Vec<u16>,
}

/// The thumbnail file of the album folder whose path hash is
/// `folder_hash`, made from an image of `source_bytes` (informational).
/// Flags are 0: bit 0, "no picture", is the device's own marker; with no
/// art there is no file.
pub(crate) fn write(folder_hash: u64, source_bytes: u32, px: &Pixels) -> Result<Vec<u8>, WriteError> {
    if px.small.len() != SMALL * SMALL || px.large.len() != LARGE * LARGE {
        return Err(WriteError("MPTH: a slot of the wrong size"));
    }
    let mut out = Vec::with_capacity(BYTES);
    out.extend_from_slice(&MPTH.to_le_bytes());
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&folder_hash.to_le_bytes());
    out.extend_from_slice(&source_bytes.to_le_bytes());
    out.extend_from_slice(&SIZES.to_le_bytes());
    for v in px.small.iter().chain(&px.large) {
        out.extend_from_slice(&v.to_be_bytes());
    }
    Ok(out)
}

/// A thumbnail on the card is the folder's (2.12.1, step 10's check): it is
/// 21,656 bytes, an MPTH v1 of the two sizes, its header's pathHash the
/// folder's full hash (so a clash of the 32-bit name reads as a miss). A
/// run rewrites any that isn't.
pub(crate) fn check(file: &[u8], folder_hash: u64) -> bool {
    file.len() == BYTES
        && u32_at(file, 0) == MPTH
        && u16_at(file, 4) == VERSION
        && u64_at(file, 8) == folder_hash
        && u32_at(file, 20) == SIZES
}

/// The pixels of a slot of `side` × `side` from an RGB8 image of
/// `width` × `height`: centre-cropped to a square (the shorter side fills,
/// the longer is cropped evenly), then each output pixel the area-weighted
/// mean of the source pixels it covers (a box filter, for growing as well
/// as shrinking).
fn slot(rgb: &[u8], width: usize, height: usize, side: usize) -> Vec<u16> {
    let s = width.min(height);
    let (x0, y0) = ((width - s) / 2, (height - s) / 2);
    // Output pixel i covers source [i·s/side, (i+1)·s/side), in units of
    // 1/side of a source pixel so the edges are integers.
    let spans: Vec<Vec<(usize, u64)>> = (0..side)
        .map(|i| {
            let (lo, hi) = (i * s, (i + 1) * s);
            (lo / side..hi.div_ceil(side))
                .map(|p| {
                    let (a, b) = ((p * side).max(lo), ((p + 1) * side).min(hi));
                    (p, (b - a) as u64)
                })
                .filter(|&(_, w)| w > 0)
                .collect()
        })
        .collect();
    let mut out = Vec::with_capacity(side * side);
    for ys in &spans {
        for xs in &spans {
            let (mut sum, mut total) = ([0u64; 3], 0u64);
            for &(py, wy) in ys {
                let row = (y0 + py) * width;
                for &(px, wx) in xs {
                    let at = 3 * (row + x0 + px);
                    let w = wy * wx;
                    for (c, v) in sum.iter_mut().zip(&rgb[at..at + 3]) {
                        *c += *v as u64 * w;
                    }
                    total += w;
                }
            }
            let mean = |c: u64| ((c + total / 2) / total.max(1)) as u8;
            out.push(rgb565(mean(sum[0]), mean(sum[1]), mean(sum[2])));
        }
    }
    out
}

/// Both slots from an RGB8 image (`rgb` holds width × height × 3 bytes).
pub(crate) fn from_rgb(rgb: &[u8], width: usize, height: usize) -> Option<Pixels> {
    if width == 0 || height == 0 || rgb.len() != width * height * 3 {
        return None;
    }
    Some(Pixels { small: slot(rgb, width, height, SMALL), large: slot(rgb, width, height, LARGE) })
}

/// Why an image made no thumbnail.
#[derive(Debug)]
pub(crate) enum ImageError {
    /// Not an image this player decodes (JPEG or PNG), or a broken one.
    Decode(image::ImageError),
    /// Larger than any cover: refused before it is decoded.
    TooLarge,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::Decode(e) => write!(f, "the image doesn't decode: {e}"),
            ImageError::TooLarge => f.write_str("the image is larger than a cover"),
        }
    }
}

impl std::error::Error for ImageError {}

/// A thumbnail's pixels from an image's bytes — the album folder's cover,
/// or mStream's album art (2.14.2): decoded (JPEG, baseline or progressive,
/// or PNG; its alpha dropped), then [`from_rgb`]. The decoder's memory is
/// bounded, so a hostile file can't exhaust it.
pub(crate) fn from_image(bytes: &[u8]) -> Result<Pixels, ImageError> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format();
    let mut reader = reader.map_err(|e| ImageError::Decode(e.into()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        e => ImageError::Decode(e),
    })?;
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    from_rgb(rgb.as_raw(), w, h).ok_or(ImageError::TooLarge)
}
