//! A pixel canvas over character cells, two pixels to a cell.
//!
//! `▀` draws its top half in the foreground colour and its bottom half in the
//! background, so one cell carries two independently coloured pixels. That is
//! the whole trick: it doubles the vertical resolution and costs nothing, and
//! it works in every terminal — no Sixel, no Kitty protocol, no capability
//! detection. On a 100×30 terminal it gives a 100×60 picture.
//!
//! Cells are about twice as tall as they are wide, so half-height pixels come
//! out roughly square, which is why the trace of a waveform looks like a
//! waveform rather than something squashed.
//!
//! A canvas is a widget: `frame.render_widget(&canvas, area)` writes each
//! cell's glyph and colours straight into the frame's buffer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
#[cfg(test)]
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

pub const UPPER: &str = "\u{2580}";
pub const LOWER: &str = "\u{2584}";
pub const FULL: &str = "\u{2588}";

pub struct Canvas {
    width: u16,
    /// Twice the cell rows: every cell holds an upper and a lower pixel.
    height: u16,
    pixels: Vec<Option<Color>>,
}

impl Canvas {
    pub fn new(area: Rect) -> Self {
        let (width, height) = (area.width, area.height * 2);
        Canvas { width, height, pixels: vec![None; width as usize * height as usize] }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Anything off the canvas is dropped rather than wrapped or clamped: a
    /// visualiser that overshoots should lose the overshoot, not smear it
    /// down the opposite edge.
    pub fn set(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.pixels[index] = Some(color);
    }

    /// A straight line, by Bresenham. Joining the samples of a waveform reads
    /// as a wave; leaving them as dots reads as static, which is why the
    /// scope draws lines and the vectorscope does not.
    pub fn line(&mut self, from: (i32, i32), to: (i32, i32), color: Color) {
        let (mut x, mut y) = from;
        let (dx, dy) = ((to.0 - x).abs(), -(to.1 - y).abs());
        let (sx, sy) = (if x < to.0 { 1 } else { -1 }, if y < to.1 { 1 } else { -1 });
        let mut error = dx + dy;
        loop {
            self.set(x, y, color);
            if (x, y) == to {
                return;
            }
            let doubled = error * 2;
            if doubled >= dy {
                error += dy;
                x += sx;
            }
            if doubled <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// Fold the pixels back into character rows — the picture as text, for
    /// the tests that read it by span.
    ///
    /// Runs of identical cells share a span. This used to be how every
    /// picture reached the screen, through a Paragraph; the cells go
    /// straight into the buffer now, by the `Widget` impl below.
    #[cfg(test)]
    pub fn into_lines(self) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.height as usize / 2);
        for row in 0..self.height / 2 {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut run: Option<(&'static str, Style, usize)> = None;
            for x in 0..self.width {
                let (glyph, style) = cell(self.at(x, row * 2), self.at(x, row * 2 + 1));
                match &mut run {
                    Some((g, s, count)) if *g == glyph && *s == style => *count += 1,
                    Some((g, s, count)) => {
                        spans.push(Span::styled(g.repeat(*count), *s));
                        run = Some((glyph, style, 1));
                    }
                    None => run = Some((glyph, style, 1)),
                }
            }
            if let Some((glyph, style, count)) = run {
                spans.push(Span::styled(glyph.repeat(count), style));
            }
            lines.push(Line::from(spans));
        }
        lines
    }

    fn at(&self, x: u16, y: u16) -> Option<Color> {
        self.pixels[y as usize * self.width as usize + x as usize]
    }
}

/// Every cell written straight into the buffer: its glyph, then its colours
/// patched over what is there — so an empty cell keeps the ground's colours
/// under its blank and an upper-only one keeps the ground's background under
/// its lower half, as the Paragraph this replaces did.
///
/// The Paragraph cost a String per run of identical cells, and then took
/// every cell apart again — grapheme segmentation, four width lookups — to
/// put the same glyph back. A cover picture has almost no runs, so that was
/// one allocation a cell: ~90 ns a cell against ~10 written directly, ~5% of
/// a core for a 300×90 visualizer in Cover mode, and every mosaic cover on
/// every frame (performance audit #105).
impl Widget for &Canvas {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clipped to the buffer, as the Paragraph clipped: a rect running
        // past the frame loses what hangs over, where indexing would panic.
        let area = area.intersection(buf.area);
        for row in 0..area.height.min(self.height / 2) {
            for x in 0..area.width.min(self.width) {
                let (glyph, style) = cell(self.at(x, row * 2), self.at(x, row * 2 + 1));
                buf[(area.x + x, area.y + row)].set_symbol(glyph).set_style(style);
            }
        }
    }
}

/// Which glyph shows two stacked pixels, and in what colours.
fn cell(upper: Option<Color>, lower: Option<Color>) -> (&'static str, Style) {
    match (upper, lower) {
        (None, None) => (" ", Style::new()),
        (Some(u), None) => (UPPER, Style::new().fg(u)),
        (None, Some(l)) => (LOWER, Style::new().fg(l)),
        // One colour: a solid block, so a terminal that has lost its
        // background still draws it.
        (Some(u), Some(l)) if u == l => (FULL, Style::new().fg(u)),
        // Two: the top half is the foreground and the bottom half is the
        // background of the same glyph.
        (Some(u), Some(l)) => (UPPER, Style::new().fg(u).bg(l)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(width: u16, rows: u16) -> Canvas {
        Canvas::new(Rect { x: 0, y: 0, width, height: rows })
    }

    fn text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn two_pixels_share_a_cell_and_each_keeps_its_colour() {
        let mut canvas = canvas(4, 1);
        canvas.set(0, 0, Color::Red); // upper only
        canvas.set(1, 1, Color::Blue); // lower only
        canvas.set(2, 0, Color::Green); // both, same colour
        canvas.set(2, 1, Color::Green);
        canvas.set(3, 0, Color::Red); // both, different
        canvas.set(3, 1, Color::Blue);

        let lines = canvas.into_lines();
        assert_eq!(text(&lines), vec![format!("{UPPER}{LOWER}{FULL}{UPPER}")]);

        let spans = &lines[0].spans;
        assert_eq!(spans[0].style.fg, Some(Color::Red));
        assert_eq!(spans[1].style.fg, Some(Color::Blue));
        assert_eq!(spans[2].style.fg, Some(Color::Green));
        // The one that earns the trick: two colours out of one cell.
        assert_eq!(spans[3].style.fg, Some(Color::Red));
        assert_eq!(spans[3].style.bg, Some(Color::Blue));
    }

    #[test]
    fn a_row_of_the_same_cell_is_one_span() {
        let mut canvas = canvas(10, 1);
        for x in 0..10 {
            canvas.set(x, 0, Color::Red);
            canvas.set(x, 1, Color::Red);
        }
        let lines = canvas.into_lines();
        assert_eq!(lines[0].spans.len(), 1, "ten identical cells, one span");
        assert_eq!(lines[0].spans[0].content.as_ref(), FULL.repeat(10));
    }

    #[test]
    fn what_falls_off_the_edge_is_dropped_rather_than_wrapped() {
        let mut canvas = canvas(3, 1);
        canvas.set(-1, 0, Color::Red);
        canvas.set(3, 0, Color::Red);
        canvas.set(0, -1, Color::Red);
        canvas.set(0, 2, Color::Red);
        assert_eq!(text(&canvas.into_lines()), vec!["   "], "nothing smeared to the far side");
    }

    /// A picture with every kind of cell in it, runs and lone cells both.
    fn every_kind_of_cell(width: u16, rows: u16) -> Canvas {
        let mut canvas = canvas(width, rows);
        let colours = [Color::Red, Color::Rgb(10, 200, 30), Color::Indexed(33)];
        let mut seed = 7u32;
        for y in 0..i32::from(rows) * 2 {
            for x in 0..i32::from(width) {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                // A quarter of the pixels stay empty.
                if let Some(colour) = colours.get((seed >> 16) as usize % 4) {
                    canvas.set(x, y, *colour);
                }
            }
        }
        canvas
    }

    /// A frame with a ground laid down and old text on it: what a picture
    /// is drawn over.
    fn grounded(area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        buf.set_style(area, Style::new().fg(Color::Gray).bg(Color::Rgb(26, 26, 26)));
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_symbol("x");
            }
        }
        buf
    }

    #[test]
    fn written_straight_in_the_picture_is_the_one_the_paragraph_drew() {
        use ratatui::widgets::Paragraph;
        // Performance audit #105: the widget replaces Paragraph over
        // `into_lines`, cell for cell — glyphs, colours, and the ground
        // showing through wherever a half is empty.
        let frame = Rect::new(0, 0, 20, 8);
        let area = Rect::new(3, 2, 13, 5);
        let mut direct = grounded(frame);
        (&every_kind_of_cell(13, 5)).render(area, &mut direct);
        let mut paragraph = grounded(frame);
        Paragraph::new(every_kind_of_cell(13, 5).into_lines()).render(area, &mut paragraph);
        assert_eq!(direct, paragraph);
        assert_eq!(direct[(0, 0)].symbol(), "x", "nothing outside the rect is touched");
    }

    #[test]
    fn a_picture_running_past_the_frame_is_clipped_as_the_paragraph_clipped_it() {
        use ratatui::widgets::Paragraph;
        // Past the right edge and the bottom: indexing the buffer there
        // would panic, where the Paragraph dropped the overhang.
        let frame = Rect::new(0, 0, 10, 4);
        for area in [Rect::new(6, 1, 9, 6), Rect::new(0, 0, 30, 20), Rect::new(12, 5, 4, 4)] {
            let mut direct = grounded(frame);
            (&every_kind_of_cell(area.width, area.height)).render(area, &mut direct);
            let mut paragraph = grounded(frame);
            Paragraph::new(every_kind_of_cell(area.width, area.height).into_lines())
                .render(area, &mut paragraph);
            assert_eq!(direct, paragraph, "{area:?}");
        }
    }

    /// Not a check, a measurement (performance audit #105): a 250x80 panel
    /// — a 300x90 terminal's visualizer — written into a frame both ways,
    /// as a photo (every cell its own colours: the Cover mode, a mosaic
    /// cover) and as bars (long runs: the spectrum), each frame building
    /// its picture fresh as the visualizer does —
    /// `cargo test --release a_panel_both_ways -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn time_a_panel_both_ways() {
        use ratatui::widgets::Paragraph;
        use std::time::Instant;
        let (width, rows) = (250u16, 80u16);
        let photo = || {
            let mut canvas = canvas(width, rows);
            let mut seed = 1u32;
            for y in 0..i32::from(rows) * 2 {
                for x in 0..i32::from(width) {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    let [r, g, b, _] = seed.to_be_bytes();
                    canvas.set(x, y, Color::Rgb(r, g, b));
                }
            }
            canvas
        };
        let bars = || {
            let mut canvas = canvas(width, rows);
            let height = i32::from(rows) * 2;
            for bar in 0..i32::from(width) / 3 {
                let top = (bar * 37) % height;
                for y in top..height {
                    let colour = Color::Rgb((y * 255 / height) as u8, 80, 200);
                    canvas.set(bar * 3, y, colour);
                    canvas.set(bar * 3 + 1, y, colour);
                }
            }
            canvas
        };
        let area = Rect::new(0, 0, width, rows);
        let frames = 300;
        for (name, picture) in [("photo", &photo as &dyn Fn() -> Canvas), ("bars", &bars)] {
            let mut buf = grounded(area);
            let started = Instant::now();
            for _ in 0..frames {
                std::hint::black_box(picture());
            }
            let build = started.elapsed() / frames;
            let started = Instant::now();
            for _ in 0..frames {
                (&picture()).render(area, &mut buf);
            }
            let direct = started.elapsed() / frames;
            let started = Instant::now();
            for _ in 0..frames {
                Paragraph::new(picture().into_lines()).render(area, &mut buf);
            }
            let paragraph = started.elapsed() / frames;
            println!(
                ">>> {name} {width}x{rows}: {direct:?} a frame written straight in, {paragraph:?} \
                 through the Paragraph ({build:?} of each is building the picture)"
            );
        }
    }

    #[test]
    fn a_line_joins_its_ends_without_gaps() {
        let mut canvas = canvas(8, 2);
        canvas.line((0, 0), (7, 3), Color::Red);
        let lines = canvas.into_lines();
        let drawn = text(&lines);
        // Every column is touched: a diagonal with holes in it reads as dots.
        for (row, line) in drawn.iter().enumerate() {
            assert_eq!(line.chars().count(), 8, "row {row}");
        }
        let lit: usize = drawn.iter().map(|l| l.chars().filter(|c| *c != ' ').count()).sum();
        assert!(lit >= 8, "a line across eight columns lights at least eight: {drawn:?}");
    }

}
