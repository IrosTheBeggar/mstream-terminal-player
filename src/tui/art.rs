//! Album covers, decoded and held at terminal scale.
//!
//! [`decode`] turns whatever bytes the server sent into a small RGB
//! thumbnail; [`cover_sample`] reads it back out at panel size for the
//! cover visualizer, box-averaged so that shrinking blends detail away
//! instead of keeping whichever single pixel the grid lands on. This
//! module deliberately knows pixels, not cells — how covers become
//! *characters* is the visualizer's business, over in `viz` — and only
//! [`decode`] knows the `image` crate exists.
//!
//! The thumbnail is not the whole story any more. A terminal that speaks
//! kitty or sixel wants the cover at hundreds of pixels a side, which is
//! far more than the mosaic ever needs, so an [`Art`] also keeps the bytes
//! it was decoded from. Keeping the *encoded* bytes rather than a second
//! larger buffer is the whole trick: a jpeg cover is tens of kilobytes
//! where the pixels it unpacks to are hundreds, and the cache holds
//! sixty-four of them. Who unpacks them, and how big, is `tui::graphics`.

use std::sync::Arc;
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
    /// The bytes this was decoded from, for a renderer that wants the cover
    /// at more than [`MAX_SIDE`]. Shared rather than copied: the clone that
    /// goes to the cache and the clone the drawing path holds are the same
    /// picture, and this is the larger half of it.
    source: Arc<[u8]>,
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
    ///
    /// For tests, which want a cover of a known four pixels rather than a
    /// round trip through a codec. It carries no source bytes, so an `Art`
    /// built this way can be sampled but not drawn as a picture — which is
    /// the right way round: the tests are the build that has no terminal to
    /// draw one on.
    #[cfg(test)]
    pub fn from_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Option<Art> {
        Art::build(width, height, rgb, Arc::from([].as_slice()))
    }

    fn build(width: u32, height: u32, rgb: Vec<u8>, source: Arc<[u8]>) -> Option<Art> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        if width == 0 || height == 0 || rgb.len() != (width * height * 3) as usize {
            return None;
        }
        Some(Art { id: NEXT.fetch_add(1, Ordering::Relaxed), width, height, rgb, source })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// The bytes this cover arrived as, or empty for one built from pixels.
    pub fn source(&self) -> &[u8] {
        &self.source
    }
}

/// The largest cover whose bytes are worth keeping beside the thumbnail,
/// in source pixels. Two costs ride on this, and the pixel count is the
/// honest measure of the worse one: the pixel renderer decodes the source
/// again at draw time, on the thread the keyboard is waiting on, and a
/// scan-of-the-booklet cover that decodes to twenty megapixels is half a
/// second of frozen UI where a normal cover is ten milliseconds. Four
/// megapixels — past 2000x2000, which streaming art does not reach —
/// bounds that and, with it, the cache's byte weight. Covers past it keep
/// only the thumbnail and draw as mosaic, which is what every cover did
/// before the pixel path existed.
const MAX_SOURCE_PIXELS: u32 = 4_194_304;

/// Decode whatever the server sent and shrink it to [`MAX_SIDE`], keeping
/// the bytes for anyone who wants it bigger. `None` covers every way bytes
/// fail to be a usable image; the caller treats that the same as having no
/// art at all.
pub fn decode(bytes: &[u8]) -> Option<Art> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let source = if keeps_source(decoded.width(), decoded.height()) {
        Arc::from(bytes)
    } else {
        Arc::from([].as_slice())
    };
    let image = decoded.thumbnail(MAX_SIDE, MAX_SIDE).into_rgb8();
    Art::build(image.width(), image.height(), image.into_raw(), source)
}

/// Whether a cover of these source dimensions keeps its bytes. The browser
/// build never does — ratzilla's DOM backend has no pixel protocol, so
/// sixty-four covers' bytes would be memory spent on nothing.
fn keeps_source(width: u32, height: u32) -> bool {
    !cfg!(target_arch = "wasm32") && width.saturating_mul(height) <= MAX_SOURCE_PIXELS
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
        assert!(!art.source().is_empty(), "an ordinary cover keeps its bytes");

        assert!(decode(b"not an image").is_none());
    }

    #[test]
    fn oversized_covers_keep_the_thumbnail_but_not_the_bytes() {
        // The predicate rather than a twenty-megapixel fixture: encoding
        // one to prove it gets dropped would cost the suite more time than
        // the branch is worth.
        assert!(keeps_source(2000, 2000));
        assert!(!keeps_source(2100, 2100), "past four megapixels the bytes go");
        assert!(!keeps_source(u32::MAX, u32::MAX), "and the product must not wrap");

        // Pixels handed over directly never have bytes to keep.
        let art = Art::from_rgb(1, 1, vec![1, 2, 3]).unwrap();
        assert!(art.source().is_empty());
    }
}
