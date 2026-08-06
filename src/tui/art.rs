//! Album art, drawn in text cells.
//!
//! Each cell is a `▀` with the top half-pixel as its foreground and the
//! bottom as its background, which makes a cell two pixels tall — and since
//! a terminal cell is about twice as tall as it is wide, those two pixels
//! come out square. No graphics protocol, no escape sequences ratatui can't
//! see: the picture is ordinary styled cells, so it redraws, resizes and
//! scrolls exactly like everything else on the screen.
//!
//! The bargain is resolution. A cover gets at most the facts column's width
//! in pixels, which reads as a mosaic — recognisably *that* cover, not a
//! reproduction of it. The crisp version of this feature is a terminal
//! graphics protocol, which is a different order of dependency and
//! per-terminal trouble; the mosaic is what art costs when it has to work
//! everywhere the rest of this UI works.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Longest side a decoded cover keeps, in pixels. The widest the facts
/// column ever gets is 46 cells, so 46 pixels across is the most that can
/// reach the screen; twice that and change is kept so the box averaging
/// below has real neighbourhoods to average, rather than being a resample
/// of a resample.
const MAX_SIDE: u32 = 128;

/// Below this the picture stops being a picture. Dropped entirely rather
/// than shrunk to noise — the same bargain the connect screen's banner
/// makes with a small terminal.
const MIN_WIDTH: u16 = 10;
const MIN_ROWS: u16 = 5;

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

/// The cover as rows of cells, fitted inside `max_width` columns and
/// `max_rows` rows. Empty when the space is too small to say anything, or
/// when colour is turned off — a colourless halfblock mosaic is a solid
/// grey rectangle, which is worse than nothing.
pub fn lines(art: &Art, max_width: u16, max_rows: u16) -> Vec<Line<'static>> {
    if !colors_allowed() {
        return Vec::new();
    }
    draw(art, max_width, max_rows)
}

/// `NO_COLOR` set and non-empty, the convention crossterm itself honours.
/// Read once: the environment does not change mid-run, and this is asked
/// on every frame.
fn colors_allowed() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()))
}

/// The drawing itself, split from the gate so tests can reach it in an
/// environment that has `NO_COLOR` set — which the test runner here does.
fn draw(art: &Art, max_width: u16, max_rows: u16) -> Vec<Line<'static>> {
    if max_width < MIN_WIDTH || max_rows < MIN_ROWS {
        return Vec::new();
    }
    let (width, height) = fit(art.width, art.height, u32::from(max_width), u32::from(max_rows) * 2);

    let mut rows = Vec::with_capacity(height.div_ceil(2) as usize);
    for row in 0..height.div_ceil(2) {
        let mut spans = Vec::with_capacity(width as usize);
        for x in 0..width {
            let mut style = Style::new().fg(sample(art, width, height, x, row * 2));
            // An odd final row has no bottom pixel; leaving the background
            // unset lets the terminal's own show through the lower half,
            // which is what "nothing there" should look like.
            if row * 2 + 1 < height {
                style = style.bg(sample(art, width, height, x, row * 2 + 1));
            }
            spans.push(Span::styled("\u{2580}", style));
        }
        rows.push(Line::from(spans));
    }
    rows
}

/// Scale (width, height) to fill the box while keeping the cover's shape,
/// never reaching zero in either direction.
fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = (max_width as f64 / width as f64).min(max_height as f64 / height as f64);
    (
        ((width as f64 * scale).round() as u32).clamp(1, max_width),
        ((height as f64 * scale).round() as u32).clamp(1, max_height),
    )
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

    /// A solid block of one colour.
    fn solid(width: u32, height: u32, rgb: [u8; 3]) -> Art {
        let pixels = (0..width * height).flat_map(|_| rgb).collect();
        Art::from_rgb(width, height, pixels).unwrap()
    }

    #[test]
    fn from_rgb_refuses_shapes_that_do_not_add_up() {
        assert!(Art::from_rgb(2, 2, vec![0; 12]).is_some());
        assert!(Art::from_rgb(2, 2, vec![0; 11]).is_none());
        assert!(Art::from_rgb(0, 2, Vec::new()).is_none());
    }

    #[test]
    fn a_square_cover_fills_the_box_and_stays_square() {
        // 20 columns and 10 rows is a 20x20 pixel box: every column, half
        // as many rows, two pixels per cell.
        let rows = draw(&solid(64, 64, [200, 10, 10]), 20, 10);
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].spans.len(), 20);
        let style = rows[0].spans[0].style;
        assert_eq!(style.fg, Some(Color::Rgb(200, 10, 10)));
        assert_eq!(style.bg, Some(Color::Rgb(200, 10, 10)));
    }

    #[test]
    fn a_wide_cover_keeps_its_shape_instead_of_filling_the_height() {
        // Twice as wide as tall: the width is the limit, so the picture
        // should come out 20 pixels across and 10 tall — five rows.
        let rows = draw(&solid(100, 50, [1, 2, 3]), 20, 20);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].spans.len(), 20);
    }

    #[test]
    fn an_odd_final_row_leaves_the_background_unset() {
        // 15 pixels tall is 7 full cells and a top half: the last row's
        // lower half is the terminal's own background, not a guess.
        let rows = draw(&solid(30, 30, [9, 9, 9]), 15, 20);
        assert_eq!(rows.len(), 8);
        let last = rows.last().unwrap().spans[0].style;
        assert_eq!(last.fg, Some(Color::Rgb(9, 9, 9)));
        assert_eq!(last.bg, None);
    }

    #[test]
    fn the_halves_of_a_cell_come_from_the_right_pixels() {
        // Red over blue, blown up to a 10-pixel column. The seam falls
        // inside the middle row of cells: its foreground is the last red
        // pixel and its background the first blue one — and averaging must
        // never bleed the two into purple anywhere.
        let art = Art::from_rgb(1, 2, vec![255, 0, 0, 0, 0, 255]).unwrap();
        let rows = draw(&art, 10, 5);
        assert_eq!(rows.len(), 5);
        let seam = rows[2].spans[0].style;
        assert_eq!(seam.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(seam.bg, Some(Color::Rgb(0, 0, 255)));
        assert_eq!(rows[0].spans[0].style.bg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(rows[4].spans[0].style.fg, Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn shrinking_averages_rather_than_picks() {
        // Alternating black and white columns: at half width each target
        // pixel covers one of each, and the average is grey. Nearest-pixel
        // sampling would answer solid black or solid white.
        let mut pixels = Vec::new();
        for _ in 0..40 {
            for x in 0..40u32 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                pixels.extend([v, v, v]);
            }
        }
        let art = Art::from_rgb(40, 40, pixels).unwrap();
        let rows = draw(&art, 20, 10);
        let Some(Color::Rgb(r, g, b)) = rows[0].spans[0].style.fg else {
            panic!("expected an rgb foreground");
        };
        assert_eq!((r, g, b), (127, 127, 127));
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
    fn too_small_a_space_shows_nothing_rather_than_noise() {
        let art = solid(64, 64, [5, 5, 5]);
        assert!(draw(&art, MIN_WIDTH - 1, 20).is_empty());
        assert!(draw(&art, 20, MIN_ROWS - 1).is_empty());
        assert!(!draw(&art, MIN_WIDTH, MIN_ROWS).is_empty());
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
