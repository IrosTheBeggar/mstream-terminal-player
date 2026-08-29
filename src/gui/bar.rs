//! The GUI player's bottom bar: the gold rule IS the seek bar, with the
//! tall 3-row controls beneath it and the volume on their center line.
//!
//! It owns the six bottom rows (the last is the screen's tips line): the
//! now-playing card sits right and is the click target that shows and
//! hides the queue, the seek line always agrees with its timestamps, and
//! every control keeps a key named in the tips line. No dwell tooltips
//! here — the tips line already names every key, and a bar is hovered too
//! often for tooltips to earn their draw. (A waveform-seek style existed
//! through 2026-08-29 and was retired to focus this one.)

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::kit::{Surface, dim};
use crate::kit::theme::{legacy_conhost, th};
use rust_i18n::t;

use super::Act;

/// Rows the bar region owns at the bottom of the screen. The bar draws the
/// first five; the sixth (the very last row) is the screen's tips line.
pub(super) const BAR_ROWS: u16 = 6;

/// What is playing, as the bar needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Now {
    pub title: String,
    pub artist: String,
    pub elapsed: f64,
    pub duration: f64,
}

/// The slice of player state the bar draws from.
pub(super) struct BarView<'a> {
    pub now: Option<&'a Now>,
    pub paused: bool,
    /// 0.0..=1.0.
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: bool,
    pub autodj: bool,
    pub queue_open: bool,
    /// The playing track's cover is decoded and ready: the card skips its
    /// empty slot frame and the screen paints the art over those cells
    /// after the bar (pixels where the terminal can, the mosaic elsewhere).
    pub has_art: bool,
}

// ── Pure geometry ───────────────────────────────────────────────────────────

/// Seconds as a person reads them on a transport: m:ss, minutes unbounded.
pub(super) fn fmt_time(seconds: f64) -> String {
    let whole = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// How many of `cells` are played at `elapsed` of `duration` — the drawn
/// span must always agree with the timestamps.
pub(super) fn played_cells(elapsed: f64, duration: f64, cells: usize) -> usize {
    if duration <= 0.0 || cells == 0 {
        return 0;
    }
    let frac = (elapsed / duration).clamp(0.0, 1.0);
    // Floor, so the playhead cell itself is the one being played through.
    ((frac * cells as f64) as usize).min(cells.saturating_sub(1))
}

/// The seek fraction a click on cell `i` of `cells` means: cell centers,
/// so the first cell is the start and the last is the end.
pub(super) fn cell_fraction(i: usize, cells: usize) -> f64 {
    if cells <= 1 {
        return 0.0;
    }
    i as f64 / (cells - 1) as f64
}

/// Volume as filled cells out of ten.
pub(super) fn volume_cells(volume: f32) -> usize {
    ((volume.clamp(0.0, 1.0) * 10.0).round() as usize).min(10)
}

// ── Small drawing helpers ───────────────────────────────────────────────────

fn put(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    frame.render_widget(
        Paragraph::new(Span::styled(text.to_string(), style)),
        Rect { x, y, width, height: 1 },
    );
}

fn hovered(s: &Surface<Act>, rect: Rect) -> bool {
    s.pointer.is_some_and(|p| rect.contains(p))
}

/// What a tall control is, color-wise.
enum TallKind {
    /// The play/pause slot: ACCENT, always BOLD.
    Primary,
    /// Prev/next: DIM, BOLD only under the pointer.
    Secondary,
    /// A state-wearing toggle: DIM off, OK green on (the toggle-card
    /// colors in button form).
    Toggle(bool),
}

/// The bar's 3-row control: the kit frame at ONE space of label padding —
/// the bar's dense form, so six controls, the volume and the card share a
/// hundred columns. Hover brightens frame and label together.
fn tall_compact(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    kind: TallKind,
    act: Act,
) -> u16 {
    let text = format!(" {label} ");
    let width = text.chars().count() as u16 + 2;
    let rect = Rect { x, y, width, height: 3 };
    let hover = hovered(s, rect);
    let color = match (&kind, hover) {
        (_, true) => th().bright,
        (TallKind::Primary, false) => th().accent,
        (TallKind::Secondary, false) => th().dim,
        (TallKind::Toggle(true), false) => th().ok,
        (TallKind::Toggle(false), false) => th().dim,
    };
    let bold = hover || matches!(kind, TallKind::Primary | TallKind::Toggle(true));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let style = if bold {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    frame.render_widget(Paragraph::new(Span::styled(text, style)), inner);
    s.click(rect, act);
    width
}

/// The empty cover slot: a DIM rounded frame holding the cells real pixels
/// will take once art plays (kit rule: images are for album art only).
fn cover_slot(frame: &mut Frame, x: u16, y: u16, width: u16, height: u16) {
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(dim()),
        Rect { x, y, width, height },
    );
}

/// The volume group, 19 cells on the tall controls' center line: `-` and
/// `+` step by 0.05, the ten cells set directly, the readout says where
/// it stands.
fn draw_volume(frame: &mut Frame, s: &mut Surface<Act>, x: u16, y: u16, volume: f32) {
    let legacy = legacy_conhost();
    let (full, empty) = if legacy { ('■', '·') } else { ('▰', '▱') };
    let minus = Rect { x, y, width: 1, height: 1 };
    put(frame, x, y, "-", if hovered(s, minus) { bright_bold() } else { dim() });
    s.click(minus, Act::VolDown);
    let filled = volume_cells(volume);
    for i in 0..10u16 {
        let cell = Rect { x: x + 2 + i, y, width: 1, height: 1 };
        let on = (i as usize) < filled;
        let style = if on { Style::default().fg(th().accent) } else { dim() };
        let glyph = if on { full } else { empty };
        put(frame, cell.x, y, &glyph.to_string(), style);
        s.click(cell, Act::VolSet(i as u8));
    }
    let plus = Rect { x: x + 13, y, width: 1, height: 1 };
    put(frame, x + 13, y, "+", if hovered(s, plus) { bright_bold() } else { dim() });
    s.click(plus, Act::VolUp);
    put(frame, x + 15, y, &format!("{:3}%", (volume * 100.0).round() as u32), dim());
}

fn bright_bold() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
}

/// The play/pause slot glyphs: a fixed two cells, so ▮▮ swaps in place and
/// next never moves between states.
fn play_glyphs(paused: bool) -> (&'static str, &'static str, &'static str) {
    if legacy_conhost() {
        ("<<", if paused { "> " } else { "||" }, ">>")
    } else {
        ("◂◂", if paused { "▸ " } else { "▮▮" }, "▸▸")
    }
}

/// The seek line: the gold rule's cells across the playing span, a click
/// per cell, the would-land column brightened under the pointer, its time
/// returned so the caller can preview it in the readout.
fn draw_seek_cells(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    cells: usize,
    now: &Now,
) -> Option<String> {
    let played = played_cells(now.elapsed, now.duration, cells);
    let pointer_col = s.pointer.and_then(|p| {
        let over = p.y == y && p.x >= x && (p.x as usize) < x as usize + cells;
        over.then(|| (p.x - x) as usize)
    });
    for i in 0..cells {
        let cls = if Some(i) == pointer_col {
            bright_bold()
        } else if i <= played {
            Style::default().fg(th().gold)
        } else {
            dim()
        };
        let col = x + i as u16;
        let glyph = if i == played {
            "█"
        } else if i < played {
            if legacy_conhost() { "=" } else { "━" }
        } else {
            "─"
        };
        put(frame, col, y, glyph, cls);
        s.click(
            Rect { x: col, y, width: 1, height: 1 },
            Act::Seek(cell_fraction(i, cells)),
        );
    }
    pointer_col.map(|i| fmt_time(cell_fraction(i, cells) * now.duration))
}

// ── The bar ─────────────────────────────────────────────────────────────────

/// Draw the bar into the bottom [`BAR_ROWS`] rows of `area` (minus the
/// last row, which the screen keeps for the tips line).
pub(super) fn draw(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, v: &BarView) {
    let top = area.height - BAR_ROWS;
    draw_gold_bar(frame, s, area, top, v);
}

/// The card's cover cells — where the screen paints the album art after
/// the bar. Identical in both styles.
pub(super) fn cover_rect(area: Rect) -> Rect {
    Rect { x: area.width - 32, y: area.height - BAR_ROWS + 2, width: 6, height: 3 }
}

fn gold_rule(frame: &mut Frame, y: u16, width: u16) {
    put(frame, 0, y, &"─".repeat(width as usize), Style::default().fg(th().gold));
}

/// The now-playing card: cover slot + title/artist + the ▾/▴ chevron,
/// spanning exactly the queue's column, three rows tall. One click target.
fn draw_card(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, y: u16, v: &BarView) {
    let x = area.width - 32;
    let rect = Rect { x, y, width: 31, height: 3 };
    let hover = hovered(s, rect);
    // With art in hand the frame would only bleed around the picture's
    // edges — the screen paints the cover over these cells after the bar.
    if !v.has_art {
        cover_slot(frame, x, y, 6, 3);
    }
    let (title_style, sub_style) = card_styles(hover, v.now.is_some());
    match v.now {
        Some(now) => {
            put(frame, x + 8, y, &clip(&now.title, 20), title_style);
            put(frame, x + 8, y + 1, &clip(&now.artist, 20), sub_style);
        }
        None => put(frame, x + 8, y, &t!("gui.nothing_playing"), sub_style),
    }
    let chevron = chevron_glyph(v.queue_open);
    put(frame, area.width - 3, y, chevron, if hover { bright_bold() } else { dim() });
    s.click(rect, Act::ToggleQueue);
}

/// The rule is the seek bar; the tall controls beneath it with the volume
/// on their center line; the card on the right.
fn draw_gold_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    // Row `top` stays blank — the bar's footprint hasn't moved since the
    // days it shared it with a second style, and the screen above depends
    // on it.
    let line = top + 1;
    match v.now {
        Some(now) => {
            put(frame, 1, line, &fmt_time(now.elapsed), dim());
            let cells = (area.width - 12) as usize;
            let preview = draw_seek_cells(frame, s, 6, line, cells, now);
            if let Some(time) = preview {
                put(frame, 1, line, &format!("{time:>4}"), bright_bold());
            }
            let total = fmt_time(now.duration);
            put(frame, area.width - 1 - total.chars().count() as u16, line, &total, dim());
        }
        // Idle, the line is exactly the wizard's gold rule.
        None => gold_rule(frame, line, area.width),
    }

    // The tall controls, one compact frame each, with the volume centered
    // on their middle row to their left. The play slot's label is two
    // cells both ways, so the row never re-seats.
    let (prev, play, next) = play_glyphs(v.paused);
    let y = top + 2;
    let mut x = 22u16;
    x += tall_compact(frame, s, x, y, prev, TallKind::Secondary, Act::Prev) + 1;
    x += tall_compact(frame, s, x, y, play, TallKind::Primary, Act::PlayPause) + 1;
    x += tall_compact(frame, s, x, y, next, TallKind::Secondary, Act::Next) + 1;
    if legacy_conhost() {
        x += tall_compact(frame, s, x, y, &t!("gui.shuffle_word"), TallKind::Toggle(v.shuffle), Act::Shuffle) + 1;
        x += tall_compact(frame, s, x, y, &t!("gui.repeat_word"), TallKind::Toggle(v.repeat), Act::Repeat) + 1;
    } else {
        x += tall_compact(frame, s, x, y, "⇄", TallKind::Toggle(v.shuffle), Act::Shuffle) + 1;
        x += tall_compact(frame, s, x, y, "↻", TallKind::Toggle(v.repeat), Act::Repeat) + 1;
    }
    tall_compact(frame, s, x, y, "auto-dj", TallKind::Toggle(v.autodj), Act::AutoDj);
    draw_volume(frame, s, 1, y + 1, v.volume);

    draw_card(frame, s, area, y, v);
}

fn card_styles(hover: bool, playing: bool) -> (Style, Style) {
    if hover {
        (bright_bold(), Style::default().fg(th().bright))
    } else if playing {
        (Style::default().add_modifier(Modifier::BOLD), dim())
    } else {
        (dim(), dim())
    }
}

fn chevron_glyph(queue_open: bool) -> &'static str {
    match (legacy_conhost(), queue_open) {
        (false, true) => "▾",
        (false, false) => "▴",
        (true, true) => "▼",
        (true, false) => "▲",
    }
}

/// Truncate at the cell edge with the kit's clip mark.
pub(super) fn clip(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let mut out: String = chars[..max.saturating_sub(1)].iter().collect();
    out.push(if legacy_conhost() { '»' } else { '…' });
    out
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_like_a_transport() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(47.0), "0:47");
        assert_eq!(fmt_time(302.0), "5:02");
        assert_eq!(fmt_time(3725.0), "62:05", "minutes stay minutes past the hour");
        assert_eq!(fmt_time(-3.0), "0:00", "a stray negative clamps");
    }

    #[test]
    fn the_drawn_span_agrees_with_the_timestamps() {
        // The canvas's own math: 0:47 of 5:02 = 15.6%.
        assert_eq!(played_cells(47.0, 302.0, 55), 8);
        assert_eq!(played_cells(47.0, 302.0, 87), 13);
        assert_eq!(played_cells(0.0, 302.0, 55), 0);
        // The end clamps to the last cell rather than one past it.
        assert_eq!(played_cells(302.0, 302.0, 55), 54);
        assert_eq!(played_cells(10.0, 0.0, 55), 0, "an unknown duration plays nothing");
    }

    #[test]
    fn cell_clicks_map_to_the_whole_track() {
        assert_eq!(cell_fraction(0, 55), 0.0);
        assert_eq!(cell_fraction(54, 55), 1.0);
        let mid = cell_fraction(27, 55);
        assert!((mid - 0.5).abs() < 0.01);
        assert_eq!(cell_fraction(0, 1), 0.0, "a one-cell bar cannot divide by zero");
    }

    #[test]
    fn volume_cells_round_honestly() {
        assert_eq!(volume_cells(0.0), 0);
        assert_eq!(volume_cells(0.72), 7);
        assert_eq!(volume_cells(1.0), 10);
        assert_eq!(volume_cells(2.0), 10, "overdrive clamps to the bar");
    }
}
