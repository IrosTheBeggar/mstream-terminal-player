//! The mini player (docs/ux-contracts/mini-player.md): what the GUI draws
//! in a terminal smaller than the hundred by twenty-four its screens need —
//! the playing track's cover, its words the way the bar's card says them,
//! prev · play · next in the bar's own frames, and a line saying the full
//! player is a larger window away. The rooms, the queue and the modals wait
//! for the room they need; the keys the bar names (Space, `p`, `n`, `-`
//! `+`) keep working, as they always did here.
//!
//! The parts share the window by what they are worth: the title and the
//! artist before the cover, the cover before the album, and the file's
//! spec and the stars only in room the cover cannot use. The cover is
//! stacked above the rest in a tall or narrow window and beside it in a
//! short wide one, whichever draws it larger. Below the transport's own
//! size only the line is left, which is what the GUI drew here before there
//! was a mini player.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use super::bar::{Now, TallKind, clip, cover_slot, facts, play_glyphs, tall_compact};
use super::{Act, Gui, draw_card_cover, playing_cover_ready, put};
use crate::kit::dim;
use crate::kit::theme::th;
use rust_i18n::t;

/// The transport: three frames six cells wide — a two-cell glyph, a cell of
/// padding either side, the frame — and a cell between them.
const TRANSPORT_W: u16 = 20;
const TRANSPORT_H: u16 = 3;
/// The least cover worth drawing: the bar card's, eight cells by four.
const MIN_COVER: u16 = 4;
/// The song's lines that come before the cover (the title, the artist),
/// and the count once the album, which comes after it, joins them.
const BEFORE_COVER: usize = 2;
const WITH_COVER: usize = 3;
/// The air between the parts: a row above and below the transport, two
/// cells beside the cover, a cell or a row at the window's edges.
const GAP: u16 = 1;
const SIDE_GAP: u16 = 2;
const EDGE: u16 = 1;

/// One line of the song's words: runs of text, each in its own style.
pub(super) type Words = Vec<(String, Style)>;

/// Where the parts stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Mini {
    /// The cover, twice as wide as tall — square, in terminal cells.
    pub cover: Option<Rect>,
    /// Where the song's lines go, when any do.
    pub words: Option<Block>,
    /// The transport's top-left, when its frames fit.
    pub transport: Option<(u16, u16)>,
    /// The line, wrapped: each piece and where it starts.
    pub lines: Vec<(u16, u16, String)>,
}

/// The song's lines' place: a span `width` cells wide from `x`, rows from
/// `y`, and how many of the lines fit — the first `count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Block {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub count: usize,
}

/// The song's lines the way the bar's card says them — the title, the
/// artist, the album and the year, the file's spec, the stars with the key
/// and the tempo — each left out when it has nothing to say. Nothing
/// playing is the card's one dim line.
pub(super) fn words(now: Option<&Now>) -> Vec<Words> {
    let Some(now) = now else {
        return vec![vec![(t!("gui.nothing_playing").to_string(), dim())]];
    };
    let mut lines: Vec<Words> = vec![vec![(now.title.clone(), Style::default().add_modifier(Modifier::BOLD))]];
    if !now.artist.is_empty() {
        lines.push(vec![(now.artist.clone(), Style::default().fg(th().text))]);
    }
    let album = match now.year {
        Some(year) if !now.album.is_empty() => format!("{} · {year}", now.album),
        Some(year) => year.to_string(),
        None => now.album.clone(),
    };
    if !album.is_empty() {
        lines.push(vec![(album, dim())]);
    }
    if !now.spec.is_empty() {
        lines.push(vec![(now.spec.clone(), dim())]);
    }
    let mut last: Words = Vec::new();
    if let Some(rating) = now.rating.filter(|r| *r > 0) {
        last.push((super::actions::stars(Some(rating)), Style::default().fg(th().gold)));
    }
    if let Some(facts) = facts(now) {
        if !last.is_empty() {
            last.push(("  ".to_string(), dim()));
        }
        last.push((facts, dim()));
    }
    if !last.is_empty() {
        lines.push(last);
    }
    lines
}

fn width_of(line: &Words) -> u16 {
    line.iter().map(|(text, _)| text.width()).sum::<usize>() as u16
}

/// `line` cut to `width` cells, the last run that crosses the edge clipped
/// with the kit's mark.
fn fit(line: &Words, width: usize) -> Words {
    let mut out = Words::new();
    let mut used = 0;
    for (text, style) in line {
        let room = width.saturating_sub(used);
        if room == 0 {
            break;
        }
        let piece = clip(text, room).into_owned();
        used += piece.width();
        let cut = piece.width() < text.width();
        out.push((piece, *style));
        if cut {
            break;
        }
    }
    out
}

/// The parts laid out in `area`: `widths` are the song's lines' widths, in
/// order of worth, and `text` the line asking for room.
pub(super) fn layout(area: Rect, widths: &[u16], text: &str) -> Mini {
    // Every count of the song's lines, from none to all, each with the best
    // cover it leaves room for — or nothing, where even no cover fails.
    let tries: Vec<Option<Mini>> = (0..=widths.len()).map(|k| arrange(area, &widths[..k], text)).collect();
    let fits = |k: usize| tries[k].is_some();
    let cover = |k: usize| tries[k].as_ref().and_then(|m| m.cover).map_or(0, |c| c.height);

    let n = widths.len();
    let first = n.min(BEFORE_COVER);
    let Some(most) = (0..=first).rev().find(|k| fits(*k)) else {
        return line_alone(area, text);
    };
    let chosen = if most < first || cover(first) == 0 {
        // No cover beside the title and the artist: as many lines as fit.
        (most..=n).rev().find(|k| fits(*k)).unwrap_or(most)
    } else {
        // A cover: the album only while one still fits, and the rest only
        // in room the cover cannot use.
        let base = if n >= WITH_COVER && cover(WITH_COVER) >= MIN_COVER { WITH_COVER } else { first };
        (base + 1..=n).rev().find(|k| fits(*k) && cover(*k) == cover(base)).unwrap_or(base)
    };
    tries[chosen].clone().unwrap_or_else(|| line_alone(area, text))
}

/// Too small for the frames: the line alone, centred — what the GUI drew
/// here before the mini player.
fn line_alone(area: Rect, text: &str) -> Mini {
    let lines = wrap(text, area.width.max(1) as usize);
    let top = area.y + area.height.saturating_sub(lines.len() as u16) / 2;
    Mini { cover: None, words: None, transport: None, lines: centre(lines, area.x, area.width, top) }
}

/// The song's first `widths.len()` lines, the transport and the line, and
/// the largest cover they leave room for, above them or beside them. None
/// when they do not fit even without a cover.
fn arrange(area: Rect, widths: &[u16], text: &str) -> Option<Mini> {
    let inner_w = area.width.saturating_sub(2 * EDGE);
    let inner_h = area.height.saturating_sub(2 * EDGE);
    let k = widths.len();
    // The column's height over `lines` rows of the line, and the air under
    // the song's lines: a row where the window has one to give, none where
    // it is that row short — the words matter more than the air.
    let column = |lines: usize| -> Option<(u16, u16)> {
        let bare = k as u16 + TRANSPORT_H + GAP + lines as u16;
        if k > 0 && bare + GAP <= inner_h {
            Some((bare + GAP, GAP))
        } else {
            (bare <= inner_h).then_some((bare, 0))
        }
    };

    // Stacked: the column under the cover, the window's width.
    let stacked_lines = wrap(text, inner_w.max(1) as usize);
    if inner_w < TRANSPORT_W {
        return None;
    }
    // A narrower column only wraps the line onto more rows: what does not
    // fit here fits nowhere.
    let (stacked_column, stacked_air) = column(stacked_lines.len())?;
    let stacked = inner_h.saturating_sub(stacked_column + GAP).min(inner_w / 2);
    let stacked = if stacked >= MIN_COVER { stacked } else { 0 };

    // Beside: the cover on the left, the column to its right as wide as its
    // widest line wants and no narrower than the frames.
    let natural = widths.iter().copied().chain([text.width() as u16, TRANSPORT_W]).max().unwrap_or(TRANSPORT_W);
    let widest = inner_h.min(inner_w.saturating_sub(SIDE_GAP + TRANSPORT_W) / 2);
    let side = (MIN_COVER..=widest).rev().find_map(|cover| {
        let width = inner_w.checked_sub(2 * cover + SIDE_GAP)?.min(natural);
        let lines = wrap(text, width as usize);
        let (height, air) = column(lines.len())?;
        (width >= TRANSPORT_W).then_some((cover, width, lines, height, air))
    });

    Some(match side {
        Some((cover, column, lines, height, air)) if cover > stacked => {
            let group_w = 2 * cover + SIDE_GAP + column;
            let left = area.x + area.width.saturating_sub(group_w) / 2;
            let cover_rect =
                Rect { x: left, y: area.y + area.height.saturating_sub(cover) / 2, width: 2 * cover, height: cover };
            let column_x = left + 2 * cover + SIDE_GAP;
            let top = area.y + area.height.saturating_sub(height) / 2;
            let words = (k > 0).then_some(Block { x: column_x, y: top, width: column, count: k });
            let transport_y = top + k as u16 + air;
            Mini {
                cover: Some(cover_rect),
                words,
                transport: Some((column_x + (column - TRANSPORT_W) / 2, transport_y)),
                lines: centre(lines, column_x, column, transport_y + TRANSPORT_H + GAP),
            }
        }
        _ => {
            let cover = (stacked > 0).then_some(stacked);
            let cover_h = cover.map_or(0, |c| c + GAP);
            let top = area.y + area.height.saturating_sub(cover_h + stacked_column) / 2;
            let cover_rect = cover.map(|c| Rect {
                x: area.x + area.width.saturating_sub(2 * c) / 2,
                y: top,
                width: 2 * c,
                height: c,
            });
            let words_y = top + cover_h;
            let words = (k > 0).then_some(Block { x: area.x + EDGE, y: words_y, width: inner_w, count: k });
            let transport_y = words_y + k as u16 + stacked_air;
            Mini {
                cover: cover_rect,
                words,
                transport: Some((area.x + (area.width - TRANSPORT_W) / 2, transport_y)),
                lines: centre(stacked_lines, area.x + EDGE, inner_w, transport_y + TRANSPORT_H + GAP),
            }
        }
    })
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

/// The song's lines for what the GUI plays now, and where everything goes
/// in `area` — what [`draw`] draws, and what the tests click.
pub(super) fn plan(gui: &Gui, area: Rect) -> (Vec<Words>, Mini) {
    let words = words(gui.bar_now().as_ref());
    let widths: Vec<u16> = words.iter().map(width_of).collect();
    let mini = layout(area, &widths, &t!("gui.mini.enlarge"));
    (words, mini)
}

/// The mini player, in the whole of a window too small for a screen.
pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let (words, mini) = plan(gui, area);
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
    if let Some(block) = mini.words {
        for (row, line) in words.iter().take(block.count).enumerate() {
            let line = fit(line, block.width as usize);
            let mut x = block.x + block.width.saturating_sub(width_of(&line)) / 2;
            for (text, style) in &line {
                put(frame, x, block.y + row as u16, text, *style);
                x += text.width() as u16;
            }
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
    /// The demo seat's five lines' widths: "Cassini IV", "Vela — Cassini",
    /// "Cassini · 2019", "FLAC · 912 kbps · 44.1 kHz" and
    /// "★★★★☆  ♪ 8A · 120 BPM".
    const SONG: [u16; 5] = [10, 14, 14, 26, 21];

    fn area(width: u16, height: u16) -> Rect {
        Rect { x: 0, y: 0, width, height }
    }

    /// Every part inside the window, and none over another.
    fn check(mini: &Mini, area: Rect, widths: &[u16]) {
        let transport = mini.transport.map(|(x, y)| Rect { x, y, width: TRANSPORT_W, height: TRANSPORT_H });
        let texts =
            mini.lines.iter().map(|(x, y, l)| Rect { x: *x, y: *y, width: l.width() as u16, height: 1 });
        let words = mini.words.map(|b| Rect { x: b.x, y: b.y, width: b.width, height: b.count as u16 });
        let parts: Vec<Rect> = mini.cover.into_iter().chain(words).chain(transport).chain(texts).collect();
        for (i, a) in parts.iter().enumerate() {
            assert!(
                area.contains(a.as_position()) && a.right() <= area.right() && a.bottom() <= area.bottom(),
                "{a:?} leaves {area:?}"
            );
            for b in &parts[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} and {b:?} overlap");
            }
        }
        if let Some(block) = mini.words {
            assert!(block.count <= widths.len());
        }
    }

    fn shown(mini: &Mini) -> usize {
        mini.words.map_or(0, |b| b.count)
    }

    #[test]
    fn a_tall_window_stacks_the_cover_over_the_words_the_transport_and_the_line() {
        // Forty by thirty is taller than wide, a cell being about twice as
        // tall as it is wide.
        let window = area(40, 30);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        let cover = mini.cover.expect("room for a cover");
        let block = mini.words.expect("the song's lines");
        let (tx, ty) = mini.transport.unwrap();
        assert_eq!(cover.width, 2 * cover.height, "square in cells");
        assert!(block.y >= cover.bottom() + GAP, "the words under the cover");
        assert!(ty >= block.y + block.count as u16 + GAP, "the transport under the words");
        assert_eq!(tx, (40 - TRANSPORT_W) / 2, "centred");
        assert!(mini.lines[0].1 >= ty + TRANSPORT_H + GAP, "the line under the transport");
        // The title, the artist and the album; the spec and the stars would
        // cost the cover rows.
        assert_eq!(block.count, 3);
        assert_eq!(cover.height, 28 - (3 + GAP) - TRANSPORT_H - GAP - 2 - GAP);
    }

    #[test]
    fn a_short_wide_window_puts_the_cover_beside_them() {
        let window = area(99, 12);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        let cover = mini.cover.expect("room for a cover");
        let block = mini.words.unwrap();
        assert_eq!(cover.height, 10, "the window's height, less its edges");
        assert!(block.x >= cover.right() + SIDE_GAP, "the words to the right of the cover");
        // The column holds all five lines beside a full-height cover by
        // giving up the row of air over the frames — words before air.
        assert_eq!(block.count, 5);
        assert_eq!(mini.transport.unwrap().1, block.y + 5);
    }

    #[test]
    fn a_roomy_window_says_everything() {
        let window = area(70, 20);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert_eq!(shown(&mini), 5);
        assert_eq!(mini.cover.map(|c| c.height), Some(18));
    }

    #[test]
    fn the_title_and_the_artist_come_before_the_cover_and_the_cover_before_the_album() {
        // Thirty by sixteen: the title, the artist and an 8×4 cover fit; the
        // album would leave the cover no room, so it waits.
        let window = area(30, 16);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert_eq!(mini.cover.map(|c| c.height), Some(MIN_COVER));
        assert_eq!(shown(&mini), 2);

        // Too short for a cover at all: the lines that fit, from the top.
        let window = area(40, 12);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert!(mini.transport.is_some());
        assert!(shown(&mini) >= 2, "{mini:?}");

        // Shorter still, the title keeps its place over the frames by giving
        // up the row of air under it, and the cover takes the side.
        let window = area(60, 8);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert_eq!(shown(&mini), 1);
        let block = mini.words.unwrap();
        assert_eq!(mini.transport.unwrap().1, block.y + 1, "no air between the title and the frames");
        assert_eq!(mini.cover.map(|c| c.height), Some(6));
    }

    #[test]
    fn a_narrow_window_wraps_the_line_and_a_tiny_one_keeps_only_it() {
        let window = area(26, 20);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert!(mini.transport.is_some());
        assert!(mini.lines.len() > 1, "wrapped: {:?}", mini.lines);
        assert!(mini.lines.iter().all(|(_, _, l)| l.width() <= 24));

        // Narrower than the frames: the line is all there is.
        let window = area(18, 10);
        let mini = layout(window, &SONG, LINE);
        check(&mini, window, &SONG);
        assert_eq!((mini.cover, mini.words, mini.transport), (None, None, None));
        assert_eq!(mini.lines.iter().map(|(_, _, l)| l.as_str()).collect::<Vec<_>>().join(" "), LINE);

        // Too short for the frames and the line under them: the same.
        let mini = layout(area(80, 4), &SONG, LINE);
        assert_eq!((mini.cover, mini.words, mini.transport), (None, None, None));
    }

    #[test]
    fn no_cover_smaller_than_the_bar_cards() {
        let window = area(24, 8);
        let mini = layout(window, &[], LINE);
        check(&mini, window, &[]);
        assert!(mini.transport.is_some());
        assert_eq!(mini.cover, None, "a sliver of cover says nothing");
    }

    #[test]
    fn a_long_title_is_clipped_to_its_span_not_wrapped() {
        let line: Words = vec![("A title far longer than the column it has to live in".into(), Style::default())];
        let cut = fit(&line, 20);
        assert_eq!(width_of(&cut), 20);
        assert!(cut[0].0.ends_with('…'));
        // The stars keep their place and the facts after them give way.
        let rated: Words = vec![
            ("★★★★☆".into(), Style::default()),
            ("  ".into(), Style::default()),
            ("♪ 8A · 120 BPM".into(), Style::default()),
        ];
        let cut = fit(&rated, 12);
        assert_eq!(cut[0].0, "★★★★☆");
        assert_eq!(width_of(&cut), 12);
    }

    #[test]
    fn nothing_playing_is_one_dim_line_and_every_word_is_the_cards() {
        assert_eq!(words(None).len(), 1);
        let now = Now {
            title: "Cassini IV".into(),
            artist: "Vela".into(),
            album: "Cassini".into(),
            elapsed: 0.0,
            duration: 302.0,
            year: Some(2019),
            spec: "FLAC · 912 kbps · 44.1 kHz".into(),
            rating: Some(8),
            key: Some("8A".into()),
            bpm: Some(120),
        };
        let text: Vec<String> =
            words(Some(&now)).iter().map(|l| l.iter().map(|(t, _)| t.as_str()).collect()).collect();
        assert_eq!(text[..4], ["Cassini IV", "Vela", "Cassini · 2019", "FLAC · 912 kbps · 44.1 kHz"]);
        assert!(text[4].ends_with("8A · 120 BPM"), "{text:?}");
        // A track that says little: its title, and nothing made up.
        let bare = Now {
            artist: String::new(),
            album: String::new(),
            year: None,
            spec: String::new(),
            rating: None,
            key: None,
            bpm: None,
            ..now
        };
        assert_eq!(words(Some(&bare)).len(), 1);
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
