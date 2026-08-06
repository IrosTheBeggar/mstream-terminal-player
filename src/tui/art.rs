//! Album covers, decoded and held at terminal scale.
//!
//! [`decode`] turns whatever bytes the server sent into a small RGB
//! thumbnail; [`cover_sample`] reads it back out at panel size for the
//! cover visualizer, box-averaged so that shrinking blends detail away
//! instead of keeping whichever single pixel the grid lands on. This
//! module deliberately knows pixels, not cells — how covers become
//! *characters* is the visualizer's business, over in `viz` — and only
//! [`decode`] knows the `image` crate exists.

use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::style::Color;

/// Longest side a decoded cover keeps, in pixels. The visualizer panel is
/// what reads it — a pixel per column, so a hundred-odd on a wide
/// terminal. This keeps a little headroom over what a panel usually asks
/// for, and a cache entry under 50 KB.
const MAX_SIDE: u32 = 128;

/// A decoded cover, already shrunk to terminal scale. Plain bytes rather
/// than an `image` type so only `decode` knows that crate exists.
#[derive(Clone)]
pub struct Art {
    /// Which decode this is, for caches keyed on "same cover as last
    /// frame" — comparing that is one integer where comparing the pixels
    /// is fifty kilobytes. A clone keeps its original's id, which is
    /// right: it *is* the same picture.
    id: u64,
    width: u32,
    height: u32,
    /// Row-major RGB, three bytes a pixel.
    rgb: Vec<u8>,
}

/// Dimensions only. The derived form prints every pixel byte, which turns
/// any test failure that mentions an event into pages of numbers.
impl std::fmt::Debug for Art {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Art({}x{})", self.width, self.height)
    }
}

/// By content, not by id: two decodes of the same bytes are the same
/// picture even though a cache would rebuild between them.
impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.height == other.height && self.rgb == other.rgb
    }
}

impl Art {
    /// Wrap raw RGB rows, refusing shapes that don't add up — the length
    /// check is what lets every index below go unchecked.
    pub fn from_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Option<Art> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if width == 0 || height == 0 || rgb.len() != (width * height * 3) as usize {
            return None;
        }
        Some(Art { id: NEXT.fetch_add(1, Ordering::Relaxed), width, height, rgb })
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

/// Decode whatever the server sent and shrink it to [`MAX_SIDE`]. `None`
/// covers every way bytes fail to be a usable image; the caller treats
/// that the same as having no art at all.
pub fn decode(bytes: &[u8]) -> Option<Art> {
    let image = image::load_from_memory(bytes).ok()?.thumbnail(MAX_SIDE, MAX_SIDE).into_rgb8();
    Art::from_rgb(image.width(), image.height(), image.into_raw())
}

/// The colour at (x, y) when the cover is scaled to *fill* a width × height
/// pixel canvas: both dimensions reach the edges and whichever overflows is
/// cropped evenly, the way `object-fit: cover` crops. For the visualiser,
/// which owns a panel-shaped canvas that a letterboxed square would leave
/// with dead columns — and a bar with no picture over it reads as a bug.
pub fn cover_sample(art: &Art, width: u32, height: u32, x: u32, y: u32) -> Color {
    let scale = (width as f64 / art.width as f64).max(height as f64 / art.height as f64);
    let vw = ((art.width as f64 * scale).round() as u32).max(width);
    let vh = ((art.height as f64 * scale).round() as u32).max(height);
    let vx = (x + (vw - width) / 2).min(vw - 1);
    let vy = (y + (vh - height) / 2).min(vh - 1);
    sample(art, vw, vh, vx, vy)
}

/// The average colour of the source region that target pixel (x, y) covers
/// when the art is drawn `width` × `height`. Box averaging, so shrinking a
/// busy cover blends detail away instead of picking whichever single pixel
/// the grid happens to land on.
fn sample(art: &Art, width: u32, height: u32, x: u32, y: u32) -> Color {
    let x0 = x * art.width / width;
    let x1 = ((x + 1) * art.width / width).max(x0 + 1);
    let y0 = y * art.height / height;
    let y1 = ((y + 1) * art.height / height).max(y0 + 1);

    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let at = ((yy * art.width + xx) * 3) as usize;
            r += u32::from(art.rgb[at]);
            g += u32::from(art.rgb[at + 1]);
            b += u32::from(art.rgb[at + 2]);
        }
    }
    let n = (x1 - x0) * (y1 - y0);
    Color::Rgb((r / n) as u8, (g / n) as u8, (b / n) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_rgb_refuses_shapes_that_do_not_add_up() {
        assert!(Art::from_rgb(2, 2, vec![0; 12]).is_some());
        assert!(Art::from_rgb(2, 2, vec![0; 11]).is_none());
        assert!(Art::from_rgb(0, 2, Vec::new()).is_none());
    }

    #[test]
    fn sampling_averages_the_region_rather_than_picking_from_it() {
        // Alternating black and white columns read at half width: each
        // target pixel covers one of each, and the answer is grey.
        // Nearest-pixel sampling would answer solid black or solid white.
        let mut pixels = Vec::new();
        for _ in 0..40 {
            for x in 0..40u32 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                pixels.extend([v, v, v]);
            }
        }
        let art = Art::from_rgb(40, 40, pixels).unwrap();
        assert_eq!(cover_sample(&art, 20, 20, 0, 0), Color::Rgb(127, 127, 127));
    }

    #[test]
    fn cover_sampling_crops_the_overflow_instead_of_squashing() {
        // 4 wide, 2 tall, one colour per column: filling a 2x2 canvas keeps
        // the middle two columns at full height and crops an end off each
        // side, rather than squeezing all four in.
        let columns = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 255]];
        let pixels = (0..2).flat_map(|_| columns.iter().flatten().copied()).collect();
        let art = Art::from_rgb(4, 2, pixels).unwrap();
        assert_eq!(cover_sample(&art, 2, 2, 0, 0), Color::Rgb(0, 255, 0));
        assert_eq!(cover_sample(&art, 2, 2, 1, 1), Color::Rgb(0, 0, 255));
    }

    #[test]
    fn decode_reads_a_real_png_and_shrinks_it() {
        // Encoded with the same crate that decodes it — the point is the
        // round trip through real image bytes, not the codec.
        let big = image::RgbImage::from_pixel(300, 300, image::Rgb([10, 200, 30]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        big.write_to(&mut bytes, image::ImageFormat::Png).unwrap();

        let art = decode(&bytes.into_inner()).unwrap();
        assert_eq!((art.width, art.height), (MAX_SIDE, MAX_SIDE));
        assert_eq!(&art.rgb[..3], &[10, 200, 30]);

        assert!(decode(b"not an image").is_none());
    }
}
