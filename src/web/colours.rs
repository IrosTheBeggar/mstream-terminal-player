//! The demo's colours on the WebGL canvas (performance audit #124).
//!
//! ratzilla's DOM grid left a `Reset` background transparent, so the page
//! showed through; its WebGL renderer paints one black. The shell settles
//! `Reset` backgrounds into the page's own colour before a frame goes out,
//! and the canvas looks as the grid did.
//!
//! Reversed cells are left to the renderer, which swaps the two colours the
//! way a terminal does: a highlighted row's text is the page's colour on
//! the bar. The DOM grid made a reversed `Reset` white instead, which put
//! the cursor row's name white on a white bar — invisible — so that one
//! difference is deliberate.
//!
//! Pure, and compiled natively for its tests like [`super::pace`].

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// index.html's background, which the DOM grid shows through a `Reset`
/// background. Also the canvas's padding, the sliver the cells leave.
pub const PAGE: Color = Color::Rgb(0x10, 0x10, 0x14);

/// Every `Reset` background in the frame, the page's colour.
pub fn settle(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        if cell.bg == Color::Reset {
            cell.bg = PAGE;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};

    use super::*;

    fn settled(style: Style) -> (Color, Color, bool) {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer.set_string(0, 0, "x", style);
        settle(&mut buffer);
        let cell = &buffer.content[0];
        (cell.fg, cell.bg, cell.modifier.contains(Modifier::REVERSED))
    }

    #[test]
    fn a_reset_background_is_the_page() {
        assert_eq!(settled(Style::new()), (Color::Reset, PAGE, false));
        assert_eq!(settled(Style::new().fg(Color::Cyan)), (Color::Cyan, PAGE, false));
        // A colour of its own is left alone.
        assert_eq!(
            settled(Style::new().fg(Color::Cyan).bg(Color::Blue)),
            (Color::Cyan, Color::Blue, false)
        );
    }

    #[test]
    fn a_reversed_cell_keeps_its_swap_for_the_renderer() {
        // The cursor bar: once swapped, the page's colour is the ink.
        let bar = Style::new().add_modifier(Modifier::REVERSED);
        assert_eq!(settled(bar), (Color::Reset, PAGE, true));
        let highlight = Style::new().fg(Color::Blue).add_modifier(Modifier::REVERSED);
        assert_eq!(settled(highlight), (Color::Blue, PAGE, true));
    }

    #[test]
    fn the_whole_frame_is_settled() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 3));
        buffer.set_string(2, 1, "a row", Style::new().fg(Color::Green));
        settle(&mut buffer);
        assert!(buffer.content.iter().all(|cell| cell.bg == PAGE));
    }
}
