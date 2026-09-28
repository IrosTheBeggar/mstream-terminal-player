//! The demo's colours, on either surface (performance audit #124).
//!
//! In a tab the page is the terminal, so the page answers the colour names
//! the way a terminal's scheme would (the TUI's `Theme` names its slots for
//! exactly that). ratzilla answers them with 1990s VGA, whose
//! half-intensity hues all but vanish on a dark page: the folder blue is
//! navy, 1.2:1 on the page, and the cursor's reverse video turns a folder
//! row into page-coloured ink on that navy — unreadable either way round.
//! [`SCHEME`] is the Campbell palette replay.rs exports frames in, the one
//! this player is developed against, with each colour that falls short of
//! 4.5:1 on the page lifted, hue kept, until it reads. Then every name reads
//! on the page, and the page's colour reads on every name's bar.
//!
//! A `Reset` background settles into the page's own colour as well. The DOM
//! grid left one transparent, so the page showed through; the WebGL renderer
//! paints one black. Settled, both draw the page — and a reversed cell swaps
//! it in as ink, so the cursor row is page-coloured text on its bar, the way
//! a terminal draws reverse video. (The DOM grid used to make a reversed
//! `Reset` white on white, which hid the name on the cursor row.) The
//! mirrored waveform leans on the same swap: its lower half is page-coloured
//! ink on the played and unplayed colours.
//!
//! Pure, and compiled natively for its tests like [`super::pace`].

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// index.html's background, which the DOM grid shows through a `Reset`
/// background. Also the canvas's padding, the sliver the cells leave.
pub const PAGE: Color = Color::Rgb(0x10, 0x10, 0x14);

/// The sixteen system colours as the page paints them, in ANSI order.
/// Campbell's own where they read on the page; the lifted five were
/// 2.3-4.2:1.
const SCHEME: [Color; 16] = [
    Color::Rgb(0x0c, 0x0c, 0x0c), // black: ink for a bar, never text here
    Color::Rgb(0xe4, 0x3b, 0x3a), // red, lifted from #c50f1f
    Color::Rgb(0x13, 0xa1, 0x0e), // green
    Color::Rgb(0xc1, 0x9c, 0x00), // yellow
    Color::Rgb(0x36, 0x72, 0xff), // blue, lifted from #0037da: the folders
    Color::Rgb(0xb9, 0x4f, 0xc9), // magenta, lifted from #881798
    Color::Rgb(0x3a, 0x96, 0xdd), // cyan: the accent
    Color::Rgb(0xcc, 0xcc, 0xcc), // gray
    Color::Rgb(0x7c, 0x7c, 0x7c), // dark gray, lifted from #767676: dim
    Color::Rgb(0xe7, 0x48, 0x56), // light red
    Color::Rgb(0x16, 0xc6, 0x0c), // light green
    Color::Rgb(0xf9, 0xf1, 0xa5), // light yellow
    Color::Rgb(0x3b, 0x78, 0xff), // light blue
    Color::Rgb(0xd3, 0x35, 0xba), // light magenta, lifted from #b4009e
    Color::Rgb(0x61, 0xd6, 0xd6), // light cyan
    Color::Rgb(0xf2, 0xf2, 0xf2), // white
];

/// A frame as the page paints it: every colour name answered from
/// [`SCHEME`], and every `Reset` background the page. A `Reset` foreground
/// stays the renderer's: both surfaces draw it white, as they always have.
pub fn settle(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        cell.fg = scheme(cell.fg);
        cell.bg = match cell.bg {
            Color::Reset => PAGE,
            other => scheme(other),
        };
    }
}

/// A colour name, or an index into the system sixteen, as the page paints
/// it. RGB — the covers, the visualizer's blends — and the rest of the
/// 256-colour cube are exact already.
fn scheme(color: Color) -> Color {
    let index = match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Indexed(index @ 0..=15) => usize::from(index),
        other => return other,
    };
    SCHEME[index]
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{List, ListItem, ListState, StatefulWidget};

    use super::*;
    use crate::tui::ui::Theme;

    fn settled(style: Style) -> (Color, Color, bool) {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer.set_string(0, 0, "x", style);
        settle(&mut buffer);
        let cell = &buffer.content[0];
        (cell.fg, cell.bg, cell.modifier.contains(Modifier::REVERSED))
    }

    /// What the renderer puts on screen for a settled cell, as (ink,
    /// ground): ratzilla's WebGL resolution — a `Reset` foreground is
    /// white, and REVERSED swaps the two.
    fn painted(cell: &ratatui::buffer::Cell) -> ((u8, u8, u8), (u8, u8, u8)) {
        let rgb = |color: Color| match color {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Reset => (0xff, 0xff, 0xff),
            other => panic!("{other:?} reached the renderer unsettled"),
        };
        let (ink, ground) = (rgb(cell.fg), rgb(cell.bg));
        match cell.modifier.contains(Modifier::REVERSED) {
            true => (ground, ink),
            false => (ink, ground),
        }
    }

    /// WCAG's contrast ratio, 1 (none) to 21 (black on white).
    fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let luminance = |(r, g, b): (u8, u8, u8)| {
            let linear = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// WCAG AA for body text.
    const READABLE: f64 = 4.5;

    #[test]
    fn a_reset_background_is_the_page() {
        assert_eq!(settled(Style::new()), (Color::Reset, PAGE, false));
        let cyan = SCHEME[6];
        assert_eq!(settled(Style::new().fg(Color::Cyan)), (cyan, PAGE, false));
        // A colour of its own is kept, as the page paints it.
        assert_eq!(
            settled(Style::new().fg(Color::Cyan).bg(Color::Blue)),
            (cyan, SCHEME[4], false)
        );
        // A cover's half blocks are exact.
        let cover = Style::new().fg(Color::Rgb(1, 2, 3)).bg(Color::Rgb(4, 5, 6));
        assert_eq!(settled(cover), (Color::Rgb(1, 2, 3), Color::Rgb(4, 5, 6), false));
    }

    #[test]
    fn a_reversed_cell_keeps_its_swap_for_the_renderer() {
        // The cursor bar: once swapped, the page's colour is the ink.
        let bar = Style::new().add_modifier(Modifier::REVERSED);
        assert_eq!(settled(bar), (Color::Reset, PAGE, true));
        let highlight = Style::new().fg(Color::Blue).add_modifier(Modifier::REVERSED);
        assert_eq!(settled(highlight), (SCHEME[4], PAGE, true));
    }

    #[test]
    fn the_names_and_their_indexes_are_one_scheme() {
        let names = [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ];
        for (index, name) in names.into_iter().enumerate() {
            assert_eq!(scheme(name), SCHEME[index], "{name:?}");
            assert_eq!(scheme(Color::Indexed(index as u8)), SCHEME[index], "{name:?}");
        }
        // Past the sixteen, the cube is exact already.
        assert_eq!(scheme(Color::Indexed(110)), Color::Indexed(110));
    }

    #[test]
    fn every_colour_reads_on_the_page_and_the_page_reads_on_its_bar() {
        let Color::Rgb(r, g, b) = PAGE else { unreachable!() };
        // Black is the one that is never text on the page: it is ink for a
        // bar, where any terminal's black is as dark.
        for (index, color) in SCHEME.iter().enumerate().skip(1) {
            let Color::Rgb(cr, cg, cb) = *color else { panic!("{index}: {color:?}") };
            let ratio = contrast((cr, cg, cb), (r, g, b));
            assert!(ratio >= READABLE, "colour {index} is {ratio:.2}:1 on the page");
        }
    }

    #[test]
    fn a_folder_under_the_cursor_reads() {
        // The browser's cursor row as ui.rs draws it: a folder's name in the
        // theme's folder colour, the list's REVERSED highlight over it, and
        // the cursor's marker in the gutter. VGA's navy made that
        // page-coloured ink on a navy bar, 1.2:1. A track's row is the
        // default colour, the bar white.
        let theme = Theme::default();
        let rows = ["ALM/", "Come Back Stronger.mp3"].map(|label| {
            let fg = if label.ends_with('/') { theme.folder } else { Color::Reset };
            ListItem::new(Line::from(Span::styled(label, Style::new().fg(fg))))
        });
        let list = List::new(rows)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("> ");
        let area = Rect::new(0, 0, 30, 2);
        for cursor in 0..2 {
            let mut buffer = Buffer::empty(area);
            let mut state = ListState::default().with_selected(Some(cursor));
            StatefulWidget::render(list.clone(), area, &mut buffer, &mut state);
            settle(&mut buffer);
            for y in 0..area.height {
                for x in 0..area.width {
                    let cell = &buffer[(x, y)];
                    if cell.symbol() == " " {
                        continue;
                    }
                    let (ink, ground) = painted(cell);
                    let ratio = contrast(ink, ground);
                    assert!(
                        ratio >= READABLE,
                        "cursor on {cursor}: {:?} at ({x},{y}) is {ratio:.2}:1",
                        cell.symbol()
                    );
                }
            }
        }
        // And the theme's other two slots, plain and as a bar.
        for slot in [theme.accent, theme.dim] {
            for style in [Style::new(), Style::new().add_modifier(Modifier::REVERSED)] {
                let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
                buffer.set_string(0, 0, "x", style.fg(slot));
                settle(&mut buffer);
                let (ink, ground) = painted(&buffer.content[0]);
                assert!(contrast(ink, ground) >= READABLE, "{slot:?} {style:?}");
            }
        }
    }

    #[test]
    fn the_whole_frame_is_settled() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 3));
        buffer.set_string(2, 1, "a row", Style::new().fg(Color::Green));
        settle(&mut buffer);
        assert!(buffer.content.iter().all(|cell| cell.bg == PAGE));
        assert_eq!(buffer[(2, 1)].fg, SCHEME[2]);
    }
}
