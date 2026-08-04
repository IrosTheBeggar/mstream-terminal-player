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

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

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

    /// Fold the pixels back into character rows.
    ///
    /// Runs of identical cells share a span. At thirty frames a second over a
    /// hundred columns the difference is thousands of allocations either way.
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
