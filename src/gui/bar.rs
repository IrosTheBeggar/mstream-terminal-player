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

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::kit::{Surface, dim};
use crate::kit::theme::{legacy_conhost, th};
use rust_i18n::t;

use super::{Act, bright_bold, put};

/// Rows the bar owns at the bottom of the screen: the seek line, three
/// rows of tall frames, and a bottom row holding the volume, the note and
/// the card's fourth line. The screen's tips line, when it shows, is the
/// row beneath (the "Player bar options" canvas, A′).
pub(super) const BAR_ROWS: u16 = 5;

/// The card's width, cover included, at the right edge.
const CARD_W: u16 = 44;
/// The cover's cells: eight by four, square in terminal cells.
const COVER_W: u16 = 8;
const COVER_H: u16 = 4;
/// Where the note begins on the bottom row, past the volume group.
const NOTE_X: u16 = 24;
/// Where the auto-dj frame stands: a few columns in from the left edge,
/// so the frame does not hug it.
pub(super) const DJ_X: u16 = 4;

/// What is playing, as the bar needs it: the words beside the cover, and
/// the facts the card's third and fourth lines wear (the sheet's, so a
/// song reads the same wherever it is met).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Now {
    pub title: String,
    pub artist: String,
    pub elapsed: f64,
    pub duration: f64,
    pub year: Option<i32>,
    /// `FLAC · 912 kbps · 44.1 kHz` — the length is the seek line's.
    pub spec: String,
    pub rating: Option<u32>,
    pub key: Option<String>,
    pub bpm: Option<u32>,
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
    /// The screen keeps its tips line under the bar: the bar sits one row
    /// higher.
    pub footer: bool,
}

/// The bar's first row — the seek line — for this frame.
pub(super) fn top(area: Rect, footer: bool) -> u16 {
    area.height - BAR_ROWS - u16::from(footer)
}

fn card_x(area: Rect) -> u16 {
    area.width - CARD_W
}

/// The card's cover cells — where the screen paints the album art after
/// the bar.
pub(super) fn cover_rect(area: Rect, footer: bool) -> Rect {
    Rect { x: card_x(area), y: top(area, footer) + 1, width: COVER_W, height: COVER_H }
}

/// Where the screen's note (or an armed pick's banner) goes: the bottom
/// row, between the volume group and the card.
pub(super) fn note_rect(area: Rect, footer: bool) -> Rect {
    Rect { x: NOTE_X, y: top(area, footer) + 4, width: card_x(area).saturating_sub(NOTE_X + 2), height: 1 }
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

/// What a tall control is, color-wise.
enum TallKind {
    /// The play/pause slot: the THICK frame in GOLD, always BOLD — the one
    /// primary action, in the colour of the seek rule above it (the
    /// "Player bar options" canvas, I).
    Primary,
    /// Prev/next: the rounded frame in the text colour, always BOLD.
    Strong,
    /// A state-wearing toggle in the rounded frame: DIM off, OK green on
    /// (the toggle-card colors in button form).
    Toggle(bool),
}

/// The bar's 3-row control at ONE space of label padding — the bar's dense
/// form, so the controls, the volume and the card share a hundred columns.
/// Hover brightens the control whole.
fn tall_compact(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    kind: TallKind,
    act: Act,
) -> u16 {
    let at = Rect { x, y, width: u16::MAX, height: 3 };
    match kind {
        TallKind::Primary => {
            let tone = |hover: bool| (if hover { th().bright } else { th().gold }, true);
            // CP437 has the double-line box, not the heavy one.
            let border = if legacy_conhost() { BorderType::Double } else { BorderType::Thick };
            crate::kit::tall_frame_bordered(frame, s, at, label, 1, border, tone, Some(act)).width
        }
        TallKind::Strong => {
            let tone = |hover: bool| (if hover { th().bright } else { th().text }, true);
            crate::kit::tall_frame(frame, s, at, label, 1, tone, Some(act)).width
        }
        TallKind::Toggle(on) => {
            let tone = |hover: bool| match (on, hover) {
                (_, true) => (th().bright, true),
                (true, false) => (th().ok, true),
                (false, false) => (th().dim, false),
            };
            crate::kit::tall_frame(frame, s, at, label, 1, tone, Some(act)).width
        }
    }
}

/// The empty cover slot: a DIM rounded frame holding the cells real pixels
/// will take once art plays (kit rule: images are for album art only).
pub(super) fn cover_slot(frame: &mut Frame, x: u16, y: u16, width: u16, height: u16) {
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
    put(frame, x, y, "-", if s.hovers(minus) { bright_bold() } else { dim() });
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
    put(frame, x + 13, y, "+", if s.hovers(plus) { bright_bold() } else { dim() });
    s.click(plus, Act::VolUp);
    put(frame, x + 15, y, &format!("{:3}%", (volume * 100.0).round() as u32), dim());
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

/// Draw the bar into its rows of `area` (see [`BAR_ROWS`]).
pub(super) fn draw(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, v: &BarView) {
    let top = top(area, v.footer);
    draw_gold_bar(frame, s, area, top, v);
}

fn gold_rule(frame: &mut Frame, y: u16, width: u16) {
    put(frame, 0, y, &"─".repeat(width as usize), Style::default().fg(th().gold));
}

/// The now-playing card: the cover, four lines of words beside it —
/// title, artist and year, the spec line, the rating with key and tempo
/// — and the ▾/▴ chevron, four rows tall at the right edge. One click
/// target; a right click (or `m`) is the playing track's sheet — the card
/// wears no verb of its own.
fn draw_card(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, y: u16, v: &BarView) {
    let x = card_x(area);
    let rect = Rect { x, y, width: CARD_W, height: COVER_H };
    let hover = s.hovers(rect);
    // With art in hand the frame would only bleed around the picture's
    // edges — the screen paints the cover over these cells after the bar.
    if !v.has_art {
        cover_slot(frame, x, y, COVER_W, COVER_H);
    }
    let tx = x + COVER_W + 2;
    // The words stop short of the chevron's column.
    let text_w = (area.width - 4).saturating_sub(tx) as usize;
    let (title_style, sub_style) = card_styles(hover, v.now.is_some());
    match v.now {
        Some(now) => {
            put(frame, tx, y, &clip(&now.title, text_w), title_style);
            let byline = match now.year {
                Some(year) if !now.artist.is_empty() => format!("{} · {year}", now.artist),
                Some(year) => year.to_string(),
                None => now.artist.clone(),
            };
            put(frame, tx, y + 1, &clip(&byline, text_w), sub_style);
            put(frame, tx, y + 2, &clip(&now.spec, text_w), dim());
            let mut fx = tx;
            if let Some(rating) = now.rating.filter(|r| *r > 0) {
                let glyphs = super::actions::stars(Some(rating));
                put(frame, fx, y + 3, &glyphs, Style::default().fg(th().gold));
                fx += glyphs.chars().count() as u16 + 2;
            }
            let mut facts: Vec<String> = Vec::new();
            if let Some(key) = now.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
                facts.push(format!("{} {key}", if legacy_conhost() { "key" } else { "♪" }));
            }
            if let Some(bpm) = now.bpm.filter(|b| *b > 0) {
                facts.push(format!("{bpm} BPM"));
            }
            if !facts.is_empty() {
                let room = (area.width - 1).saturating_sub(fx) as usize;
                put(frame, fx, y + 3, &clip(&facts.join(" · "), room), dim());
            }
        }
        None => put(frame, tx, y, &t!("gui.nothing_playing"), sub_style),
    }
    let chevron = chevron_glyph(v.queue_open);
    put(frame, area.width - 3, y, chevron, if hover { bright_bold() } else { dim() });
    s.click(rect, Act::ToggleQueue);
    if v.now.is_some() {
        s.context(rect, Act::NowMore);
    }
}

/// The rule is the seek bar; beneath it auto-dj at the left edge and one
/// centred group of frames — repeat, prev, play, next, shuffle — in the
/// span before the card, and the bottom row: the volume, then the screen's
/// note, then the card's last line.
fn draw_gold_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    let line = top;
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

    // Auto DJ stands near the left edge in its frame; the rest is one group
    // of frames — repeat, then prev, play, next, then shuffle — centred in
    // the span between it and the card, so the group holds the middle at
    // any width.
    let y = top + 1;
    let legacy = legacy_conhost();
    let shuffle_word = t!("gui.shuffle_word").to_string();
    let repeat_word = t!("gui.repeat_word").to_string();
    let (shuffle, repeat) = if legacy { (shuffle_word.as_str(), repeat_word.as_str()) } else { ("⇄", "↻") };
    let dj_w = tall_compact(frame, s, DJ_X, y, "auto-dj", TallKind::Toggle(v.autodj), Act::AutoDj);
    let (prev, play, next) = play_glyphs(v.paused);
    let group = [repeat, prev, play, next, shuffle];
    let group_w: u16 = group.iter().map(|l| l.chars().count() as u16 + 4).sum::<u16>() + (group.len() as u16 - 1);
    let free_from = DJ_X + dj_w + 1;
    let free_to = card_x(area).saturating_sub(1);
    let mut x = free_from + free_to.saturating_sub(free_from).saturating_sub(group_w) / 2;
    x += tall_compact(frame, s, x, y, repeat, TallKind::Toggle(v.repeat), Act::Repeat) + 1;
    x += tall_compact(frame, s, x, y, prev, TallKind::Strong, Act::Prev) + 1;
    x += tall_compact(frame, s, x, y, play, TallKind::Primary, Act::PlayPause) + 1;
    x += tall_compact(frame, s, x, y, next, TallKind::Strong, Act::Next) + 1;
    tall_compact(frame, s, x, y, shuffle, TallKind::Toggle(v.shuffle), Act::Shuffle);
    draw_volume(frame, s, 1, top + 4, v.volume);

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

/// Truncate at the cell edge with the kit's clip mark. Borrowed when it
/// fits — the common case, and it is asked for every label on every frame.
pub(super) fn clip(text: &str, max: usize) -> Cow<'_, str> {
    // A char past the limit is what makes the text too long.
    if text.char_indices().nth(max).is_none() {
        return Cow::Borrowed(text);
    }
    let cut = text.char_indices().nth(max.saturating_sub(1)).map_or(0, |(i, _)| i);
    let mut out = String::with_capacity(cut + 3);
    out.push_str(&text[..cut]);
    out.push(if legacy_conhost() { '»' } else { '…' });
    Cow::Owned(out)
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
