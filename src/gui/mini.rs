//! The mini player (docs/ux-contracts/mini-player.md): what the GUI draws
//! in a terminal smaller than the hundred by twenty-four its screens need —
//! the playing track's cover, prev · play · next in the bar's own frames,
//! and a line saying the full player is a larger window away. The rooms,
//! the queue and the modals wait for the room they need; the keys the bar
//! names (Space, `p`, `n`, `-` `+`) keep working, as they always did here.
//!
//! The cover takes whatever the transport and the line leave: stacked
//! above them in a tall or narrow window, beside them in a short wide one,
//! whichever draws it larger. Below the transport's own size only the line
//! is left, which is what the GUI drew here before there was a mini player.

use ratatui::Frame;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use super::bar::{TallKind, cover_slot, play_glyphs, tall_compact};
use super::{Act, Gui, draw_card_cover, playing_cover_ready, put};
use crate::kit::dim;
use rust_i18n::t;

/// The transport: three frames six cells wide — a two-cell glyph, a cell of
/// padding either side, the frame — and a cell between them.
const TRANSPORT_W: u16 = 20;
const TRANSPORT_H: u16 = 3;
/// The least cover worth drawing: the bar card's, eight cells by four.
const MIN_COVER: u16 = 4;
/// The air between the parts: a row above and below the transport, two
/// cells beside the cover, a cell or a row at the window's edges.
const GAP: u16 = 1;
const SIDE_GAP: u16 = 2;
const EDGE: u16 = 1;

/// Where the parts stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mini {
    /// The cover, twice as wide as tall — square, in terminal cells.
    pub cover: Option<Rect>,
    /// The transport's top-left, when its frames fit.
    pub transport: Option<(u16, u16)>,
    /// The line, wrapped: each piece and where it starts.
    pub lines: Vec<(u16, u16, String)>,
}

/// The parts laid out in `area`, with `text` as the line.
pub(super) fn layout(area: Rect, text: &str) -> Mini {
    let inner_w = area.width.saturating_sub(2 * EDGE);
    let inner_h = area.height.saturating_sub(2 * EDGE);

    // Too small for the frames: the line alone, centred.
    let stacked_lines = wrap(text, inner_w.max(1) as usize);
    let stacked_text_h = stacked_lines.len() as u16;
    if inner_w < TRANSPORT_W || inner_h < TRANSPORT_H + GAP + stacked_text_h {
        let lines = wrap(text, area.width.max(1) as usize);
        let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
        return Mini { cover: None, transport: None, lines: centre(lines, area.x, area.width, top) };
    }

    // Stacked: the cover above the transport, the line under it.
    let stacked_room = inner_h.saturating_sub(TRANSPORT_H + GAP + stacked_text_h + GAP);
    let stacked_cover = stacked_room.min(inner_w / 2);

    // Beside: the cover on the left the window's height, the transport and
    // the line in a column to its right, as wide as the line wants and no
    // narrower than the frames.
    let natural = text.width() as u16;
    let beside = |cover: u16| -> Option<(u16, Vec<String>)> {
        let column = inner_w.checked_sub(2 * cover + SIDE_GAP)?.min(natural.max(TRANSPORT_W));
        if column < TRANSPORT_W {
            return None;
        }
        let lines = wrap(text, column as usize);
        (TRANSPORT_H + GAP + lines.len() as u16 <= inner_h).then_some((column, lines))
    };
    let widest = inner_h.min(inner_w.saturating_sub(SIDE_GAP + TRANSPORT_W) / 2);
    let side = (MIN_COVER..=widest).rev().find_map(|cover| beside(cover).map(|fit| (cover, fit)));

    match side {
        Some((cover, (column, lines))) if cover > stacked_cover => {
            let group_w = 2 * cover + SIDE_GAP + column;
            let left = area.x + area.width.saturating_sub(group_w) / 2;
            let cover_rect = Rect { x: left, y: area.y + area.height.saturating_sub(cover) / 2, width: 2 * cover, height: cover };
            let column_x = left + 2 * cover + SIDE_GAP;
            let block_h = TRANSPORT_H + GAP + lines.len() as u16;
            let top = area.y + area.height.saturating_sub(block_h) / 2;
            let transport = (column_x + (column - TRANSPORT_W) / 2, top);
            let lines = centre(lines, column_x, column, top + TRANSPORT_H + GAP);
            Mini { cover: Some(cover_rect), transport: Some(transport), lines }
        }
        _ => {
            let cover = (stacked_cover >= MIN_COVER).then_some(stacked_cover);
            let cover_h = cover.map_or(0, |c| c + GAP);
            let block_h = cover_h + TRANSPORT_H + GAP + stacked_text_h;
            let top = area.y + area.height.saturating_sub(block_h) / 2;
            let cover_rect = cover.map(|c| Rect {
                x: area.x + area.width.saturating_sub(2 * c) / 2,
                y: top,
                width: 2 * c,
                height: c,
            });
            let transport = (area.x + (area.width - TRANSPORT_W) / 2, top + cover_h);
            let lines = centre(stacked_lines, area.x + EDGE, inner_w, top + cover_h + TRANSPORT_H + GAP);
            Mini { cover: cover_rect, transport: Some(transport), lines }
        }
    }
}

/// Each line centred in the span from `x`, `width` cells wide, one row
/// apiece from `top`.
fn centre(lines: Vec<String>, x: u16, width: u16, top: u16) -> Vec<(u16, u16, String)> {
    lines
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let left = x + width.saturating_sub(line.width() as u16) / 2;
            (left, top + row as u16, line)
        })
        .collect()
}

/// `text` in lines of at most `width` cells: broken at spaces, and inside a
/// word only where the word is wider than the line — which is every line of
/// Japanese or Chinese, written without them.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word;
        loop {
            let joined = if line.is_empty() { word.width() } else { line.width() + 1 + word.width() };
            if joined <= width {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(word);
                break;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                continue;
            }
            // A word wider than the line: as much of it as fits, and on.
            let mut cut = 0;
            let mut used = 0;
            for (at, c) in word.char_indices() {
                let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if used + w > width && cut > 0 {
                    break;
                }
                used += w;
                cut = at + c.len_utf8();
            }
            lines.push(word[..cut].to_string());
            word = &word[cut..];
            if word.is_empty() {
                break;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// The mini player, in the whole of a window too small for a screen.
pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let text = t!("gui.mini.enlarge").to_string();
    let mini = layout(area, &text);
    if let Some(rect) = mini.cover {
        if playing_cover_ready(&gui.app) {
            // The mosaic where an overlay stood last frame — every cover's
            // rule, though nothing here draws over it.
            let mosaic = gui.ui.covered_last_frame(rect);
            draw_card_cover(frame, rect, &mut gui.app, mosaic);
        } else {
            cover_slot(frame, rect.x, rect.y, rect.width, rect.height);
        }
    }
    if let Some((x, y)) = mini.transport {
        let (prev, play, next) = play_glyphs(gui.bar_paused());
        let mut x = x;
        x += tall_compact(frame, &mut gui.ui, x, y, prev, TallKind::Strong, Act::Prev) + 1;
        x += tall_compact(frame, &mut gui.ui, x, y, play, TallKind::Primary, Act::PlayPause) + 1;
        tall_compact(frame, &mut gui.ui, x, y, next, TallKind::Strong, Act::Next);
    }
    for (x, y, line) in &mini.lines {
        put(frame, *x, *y, line, dim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "Enlarge the terminal for the full player";

    fn area(width: u16, height: u16) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// Every part inside the window, and none over another.
    fn check(mini: &Mini, area: Rect) {
        let transport = mini.transport.map(|(x, y)| Rect { x, y, width: TRANSPORT_W, height: TRANSPORT_H });
        let texts: Vec<Rect> =
            mini.lines.iter().map(|(x, y, l)| Rect { x: *x, y: *y, width: l.width() as u16, height: 1 }).collect();
        let parts: Vec<Rect> = mini.cover.into_iter().chain(transport).chain(texts).collect();
        for (i, a) in parts.iter().enumerate() {
            assert!(area.contains(a.as_position()) && a.right() <= area.right() && a.bottom() <= area.bottom(), "{a:?} leaves {area:?}");
            for b in &parts[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} and {b:?} overlap");
            }
        }
    }

    #[test]
    fn a_tall_window_stacks_the_cover_over_the_transport_and_the_line() {
        // Forty by thirty is taller than wide, a cell being about twice as
        // tall as it is wide.
        let window = area(40, 30);
        let mini = layout(window, LINE);
        check(&mini, window);
        let cover = mini.cover.expect("room for a cover");
        let (tx, ty) = mini.transport.unwrap();
        assert_eq!(cover.width, 2 * cover.height, "square in cells");
        assert!(ty >= cover.bottom() + GAP, "the transport under the cover");
        assert_eq!(tx, (40 - TRANSPORT_W) / 2, "centred");
        assert_eq!(mini.lines.len(), 2, "the line wraps at 38: {:?}", mini.lines);
        assert!(mini.lines[0].1 >= ty + TRANSPORT_H + GAP, "the line under the transport");
        // As wide as the window allows: 38 cells, 19 rows.
        assert_eq!(cover.height, 19);
    }

    #[test]
    fn a_window_as_wide_as_it_is_tall_draws_the_larger_cover_beside() {
        // Sixty by twenty: a stack leaves twelve rows for the cover, the
        // side eighteen — the line wraps in the narrower column to buy it.
        let window = area(60, 20);
        let mini = layout(window, LINE);
        check(&mini, window);
        assert_eq!(mini.cover.map(|c| c.height), Some(18));
        assert_eq!(mini.lines.len(), 2);
    }

    #[test]
    fn a_short_wide_window_puts_the_cover_beside_them() {
        let window = area(99, 12);
        let mini = layout(window, LINE);
        check(&mini, window);
        let cover = mini.cover.expect("room for a cover");
        let (tx, _) = mini.transport.unwrap();
        assert_eq!(cover.height, 10, "the window's height, less its edges");
        assert!(tx >= cover.right() + SIDE_GAP, "the transport to the right of the cover");
    }

    #[test]
    fn a_narrow_window_wraps_the_line_and_a_tiny_one_keeps_only_it() {
        let window = area(26, 16);
        let mini = layout(window, LINE);
        check(&mini, window);
        assert!(mini.transport.is_some());
        assert!(mini.lines.len() > 1, "wrapped: {:?}", mini.lines);
        assert!(mini.lines.iter().all(|(_, _, l)| l.width() <= 24));

        // Narrower than the frames: the line is all there is.
        let window = area(18, 10);
        let mini = layout(window, LINE);
        check(&mini, window);
        assert_eq!((mini.cover, mini.transport), (None, None));
        assert_eq!(mini.lines.iter().map(|(_, _, l)| l.as_str()).collect::<Vec<_>>().join(" "), LINE);

        // Too short for the frames and the line under them: the same.
        let mini = layout(area(80, 4), LINE);
        assert_eq!((mini.cover, mini.transport), (None, None));
    }

    #[test]
    fn no_cover_smaller_than_the_bar_cards() {
        // Room for the frames and the line, none for a cover beside them,
        // and too little above them: no cover at all.
        let window = area(24, 8);
        let mini = layout(window, LINE);
        check(&mini, window);
        assert!(mini.transport.is_some());
        assert_eq!(mini.cover, None, "a sliver of cover says nothing");
        // Thirty by ten would fit three rows beside the frames: still none.
        let mini = layout(area(30, 10), LINE);
        assert_eq!(mini.cover, None);
        assert!(mini.transport.is_some());

        // Where the stack leaves too little, the side may not: 40 by 9
        // draws a cover beside the frames rather than none above them.
        let window = area(40, 9);
        let mini = layout(window, LINE);
        check(&mini, window);
        assert_eq!(mini.cover.map(|c| c.height), Some(7));
    }

    #[test]
    fn a_line_without_spaces_wraps_by_width() {
        let lines = wrap("ターミナルを広げるとフルプレーヤーになります", 10);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.width() <= 10), "{lines:?}");
        assert_eq!(lines.concat(), "ターミナルを広げるとフルプレーヤーになります");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("", 7), [""]);
    }
}
