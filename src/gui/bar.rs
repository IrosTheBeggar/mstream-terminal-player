//! The GUI player's bottom bar, in its two styles.
//!
//! Both own the same six bottom rows (the last is the screen's tips line)
//! and share one contract: the now-playing card sits right and is the click
//! target that shows and hides the queue, the seek surface always agrees
//! with its timestamps, and every control keeps a key named in the tips
//! line. Style is a Settings choice:
//!
//! - **Wave**: the compact transport/toggles/times/volume row ABOVE the
//!   gold rule, then the track's waveform over its reflection running from
//!   the left edge to the card.
//! - **GoldLine**: the gold rule IS the seek bar, with the tall 3-row
//!   controls beneath it and the volume on their center line.

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

/// Which bottom bar the player wears. Stored in `[gui] bar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BarStyle {
    /// The waveform bar, controls beneath it.
    Wave,
    /// The gold rule doubles as the seek bar; tall controls beneath.
    GoldLine,
}

impl BarStyle {
    /// Anything unrecognized falls back to the waveform bar, so a config
    /// written by a newer player still draws something.
    pub(crate) fn from_config(name: &str) -> BarStyle {
        match name {
            "gold-line" => BarStyle::GoldLine,
            _ => BarStyle::Wave,
        }
    }

    pub(crate) fn config_name(self) -> &'static str {
        match self {
            BarStyle::Wave => "wave",
            BarStyle::GoldLine => "gold-line",
        }
    }
}

/// What is playing, as the bar needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Now {
    pub title: String,
    pub artist: String,
    pub elapsed: f64,
    pub duration: f64,
    /// Per-track amplitude samples (any length; resampled to the cells the
    /// bar gets). None until the server's waveform arrives — the classic
    /// bar is the loading state.
    pub wave: Option<Vec<u8>>,
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

/// Resample arbitrary-length amplitude data onto `cells` columns, keeping
/// each bucket's peak — a waveform thinned by averaging goes flat.
pub(super) fn resample(wave: &[u8], cells: usize) -> Vec<u8> {
    if wave.is_empty() || cells == 0 {
        return vec![0; cells];
    }
    (0..cells)
        .map(|i| {
            let from = i * wave.len() / cells;
            let to = (((i + 1) * wave.len()) / cells).max(from + 1).min(wave.len());
            wave[from..to].iter().copied().max().unwrap_or(0)
        })
        .collect()
}

/// An amplitude byte as one of the eight block levels (1..=8).
pub(super) fn amp_level(v: u8) -> usize {
    1 + (v as usize * 7) / 255
}

const AMP: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

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

/// A 1-row glyph button: DIM at rest, BRIGHT BOLD under the pointer —
/// `emphasis` (the play/pause slot) wears ACCENT instead of DIM.
fn glyph_button(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    emphasis: bool,
    act: Act,
    tip: String,
) -> u16 {
    let width = label.chars().count() as u16;
    let rect = Rect { x, y, width, height: 1 };
    let style = match (emphasis, hovered(s, rect)) {
        (_, true) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
        (true, false) => Style::default().fg(th().accent).add_modifier(Modifier::BOLD),
        (false, false) => dim(),
    };
    put(frame, x, y, label, style);
    s.click(rect, act);
    s.tip(rect, tip);
    width
}

/// A state-wearing toggle: off DIM, on OK BOLD, hover BRIGHT BOLD.
fn toggle_button(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    on: bool,
    act: Act,
    tip: String,
) -> u16 {
    let width = label.chars().count() as u16;
    let rect = Rect { x, y, width, height: 1 };
    let style = match (on, hovered(s, rect)) {
        (_, true) => Style::default().fg(th().bright).add_modifier(Modifier::BOLD),
        (true, false) => Style::default().fg(th().ok).add_modifier(Modifier::BOLD),
        (false, false) => dim(),
    };
    put(frame, x, y, label, style);
    s.click(rect, act);
    s.tip(rect, tip);
    width
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

/// The GoldLine bar's 3-row control: the kit frame at ONE space of label
/// padding — the bar's dense form, so six controls, the volume and the
/// card share a hundred columns. Hover brightens frame and label together.
#[allow(clippy::too_many_arguments)]
fn tall_compact(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    kind: TallKind,
    act: Act,
    tip: String,
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
    s.tip(rect, tip);
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

/// The volume group: `-` and `+` step by 0.05, the ten cells set directly,
/// the readout says where it stands. `with_label` prepends the `vol` word
/// (the Wave bar's roomy row); without it the group is 19 cells (the
/// GoldLine bar's center-line form).
fn draw_volume(frame: &mut Frame, s: &mut Surface<Act>, x: u16, y: u16, volume: f32, with_label: bool) {
    let legacy = legacy_conhost();
    let (full, empty) = if legacy { ('■', '·') } else { ('▰', '▱') };
    let body = if with_label {
        put(frame, x, y, "vol", dim());
        x + 4
    } else {
        x
    };
    let minus = Rect { x: body, y, width: 1, height: 1 };
    put(frame, body, y, "-", if hovered(s, minus) { bright_bold() } else { dim() });
    s.click(minus, Act::VolDown);
    let filled = volume_cells(volume);
    for i in 0..10u16 {
        let cell = Rect { x: body + 2 + i, y, width: 1, height: 1 };
        let on = (i as usize) < filled;
        let style = if on { Style::default().fg(th().accent) } else { dim() };
        let glyph = if on { full } else { empty };
        put(frame, cell.x, y, &glyph.to_string(), style);
        s.click(cell, Act::VolSet(i as u8));
    }
    let plus = Rect { x: body + 13, y, width: 1, height: 1 };
    put(frame, body + 13, y, "+", if hovered(s, plus) { bright_bold() } else { dim() });
    s.click(plus, Act::VolUp);
    put(frame, body + 15, y, &format!("{:3}%", (volume * 100.0).round() as u32), dim());
    let width = if with_label { 23 } else { 19 };
    s.tip(Rect { x, y, width, height: 1 }, t!("gui.tip.volume").to_string());
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

/// The Wave bar's control row, full width above the gold rule: transport,
/// toggles, the time readout, the volume at the right edge. `times` is
/// (elapsed-or-preview, its style, total) — the elapsed half brightens to
/// preview the would-land time while the pointer rides the wave, which is
/// what lets the wave itself run to the card unbroken.
fn draw_compact_controls(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    y: u16,
    width: u16,
    v: &BarView,
    times: Option<(String, Style, String)>,
) {
    let legacy = legacy_conhost();
    let (prev, play, next) = play_glyphs(v.paused);
    glyph_button(frame, s, 2, y, prev, false, Act::Prev, t!("gui.tip.prev").to_string());
    glyph_button(frame, s, 7, y, play, true, Act::PlayPause, t!("gui.tip.play").to_string());
    glyph_button(frame, s, 11, y, next, false, Act::Next, t!("gui.tip.next").to_string());
    // Symbol toggles, each trailed by a space: ⇄ and ↻ are East-Asian-
    // ambiguous width, so a double-wide draw lands on the space and damages
    // nothing. The conhost floor uses the words, re-seating inside this gap.
    if legacy {
        let x = toggle_button(frame, s, 18, y, &t!("gui.shuffle_word"), v.shuffle, Act::Shuffle, t!("gui.tip.shuffle").to_string());
        let x2 = toggle_button(frame, s, 18 + x + 2, y, &t!("gui.repeat_word"), v.repeat, Act::Repeat, t!("gui.tip.repeat").to_string());
        toggle_button(frame, s, 18 + x + 2 + x2 + 2, y, "auto-dj", v.autodj, Act::AutoDj, t!("gui.tip.dj").to_string());
    } else {
        toggle_button(frame, s, 18, y, "⇄", v.shuffle, Act::Shuffle, t!("gui.tip.shuffle").to_string());
        toggle_button(frame, s, 21, y, "↻", v.repeat, Act::Repeat, t!("gui.tip.repeat").to_string());
        toggle_button(frame, s, 24, y, "auto-dj", v.autodj, Act::AutoDj, t!("gui.tip.dj").to_string());
    }
    if let Some((elapsed, elapsed_style, total)) = times {
        put(frame, 40, y, &format!("{elapsed:>5}"), elapsed_style);
        put(frame, 46, y, "/", dim());
        put(frame, 48, y, &total, dim());
    }
    draw_volume(frame, s, width - 25, y, v.volume, true);
}

/// The seek surface: waveform (or the classic bar while it loads) across
/// `cells` columns starting at `x`. Registers a per-cell click, brightens
/// the would-land column under the pointer, and previews its time by
/// returning it for the elapsed cell.
#[allow(clippy::too_many_arguments)]
fn draw_seek_cells(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    rows: u16,
    cells: usize,
    now: &Now,
    gold: bool,
) -> Option<String> {
    let played = played_cells(now.elapsed, now.duration, cells);
    let pointer_col = s.pointer.and_then(|p| {
        let over = p.y >= y && p.y < y + rows && p.x >= x && (p.x as usize) < x as usize + cells;
        over.then(|| (p.x - x) as usize)
    });
    // The gold line stays a LINE — amplitude belongs to the Wave bar only
    // (and to capable glyph sets; the floor draws the classic bar).
    let amps = (!gold)
        .then(|| now.wave.as_ref().filter(|_| !legacy_conhost()).map(|w| resample(w, cells)))
        .flatten();
    for i in 0..cells {
        let cls = if Some(i) == pointer_col {
            bright_bold()
        } else if gold && i <= played {
            Style::default().fg(th().gold)
        } else if i < played {
            Style::default().fg(th().accent)
        } else if !gold && i == played {
            Style::default().fg(th().bright)
        } else {
            dim()
        };
        let col = x + i as u16;
        match &amps {
            Some(levels) => {
                let level = amp_level(levels[i]);
                put(frame, col, y, &AMP[level - 1].to_string(), cls);
                put(frame, col, y + 1, if level <= 3 { "▔" } else { "▀" }, cls);
            }
            None => {
                let glyph = if i == played {
                    "█"
                } else if i < played {
                    if legacy_conhost() { "=" } else { "━" }
                } else {
                    "─"
                };
                put(frame, col, y, glyph, cls);
            }
        }
        for row in 0..rows {
            s.click(
                Rect { x: col, y: y + row, width: 1, height: 1 },
                Act::Seek(cell_fraction(i, cells)),
            );
        }
    }
    pointer_col.map(|i| fmt_time(cell_fraction(i, cells) * now.duration))
}

// ── The two bars ────────────────────────────────────────────────────────────

/// Draw the bar into the bottom [`BAR_ROWS`] rows of `area` (minus the
/// last row, which the screen keeps for the tips line).
pub(super) fn draw(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, style: BarStyle, v: &BarView) {
    let top = area.height - BAR_ROWS;
    match style {
        BarStyle::Wave => draw_wave_bar(frame, s, area, top, v),
        BarStyle::GoldLine => draw_gold_bar(frame, s, area, top, v),
    }
}

/// The card's cover cells — where the screen paints the album art after
/// the bar. Identical in both styles.
pub(super) fn cover_rect(area: Rect) -> Rect {
    Rect { x: area.width - 32, y: area.height - BAR_ROWS + 2, width: 6, height: 3 }
}

fn gold_rule(frame: &mut Frame, y: u16, width: u16) {
    put(frame, 0, y, &"─".repeat(width as usize), Style::default().fg(th().gold));
}

/// Wave style: controls · gold rule · waveform+reflection beside the card.
/// The times live on the control row, which is what lets the wave run from
/// the left edge to the card unbroken.
fn draw_wave_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    let left_end = area.width - 35;
    let cells = (left_end - 1) as usize;
    // The wave draws before the control row so the pointer's would-land
    // preview can ride the row's time readout.
    let times = match v.now {
        Some(now) => {
            let preview = draw_seek_cells(frame, s, 1, top + 2, 2, cells, now, false);
            let (elapsed, style) = match preview {
                Some(time) => (time, bright_bold()),
                None => (fmt_time(now.elapsed), dim()),
            };
            Some((elapsed, style, fmt_time(now.duration)))
        }
        None => {
            // Idle: a resting line where the wave will be, and honest
            // silence for the times.
            put(frame, 1, top + 2, &"─".repeat(cells), dim());
            None
        }
    };
    draw_compact_controls(frame, s, top, area.width, v, times);
    gold_rule(frame, top + 1, area.width);
    draw_card(frame, s, area, top + 2, v);
}

/// The now-playing card, both bars: cover slot + title/artist + the ▾/▴
/// chevron, spanning exactly the queue's column, three rows tall. One
/// click target.
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
    s.tip(rect, queue_tip(v.queue_open));
}

/// GoldLine style: the rule is the seek bar; the tall controls beneath it
/// with the volume on their center line; the card on the right like the
/// Wave bar's.
fn draw_gold_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    // Row `top` stays blank — the two bars keep the same footprint, so
    // toggling styles never reflows the screen above.
    let line = top + 1;
    match v.now {
        Some(now) => {
            put(frame, 1, line, &fmt_time(now.elapsed), dim());
            let cells = (area.width - 12) as usize;
            let preview = draw_seek_cells(frame, s, 6, line, 1, cells, now, true);
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
    x += tall_compact(frame, s, x, y, prev, TallKind::Secondary, Act::Prev, t!("gui.tip.prev").to_string()) + 1;
    x += tall_compact(frame, s, x, y, play, TallKind::Primary, Act::PlayPause, t!("gui.tip.play").to_string()) + 1;
    x += tall_compact(frame, s, x, y, next, TallKind::Secondary, Act::Next, t!("gui.tip.next").to_string()) + 1;
    if legacy_conhost() {
        x += tall_compact(frame, s, x, y, &t!("gui.shuffle_word"), TallKind::Toggle(v.shuffle), Act::Shuffle, t!("gui.tip.shuffle").to_string()) + 1;
        x += tall_compact(frame, s, x, y, &t!("gui.repeat_word"), TallKind::Toggle(v.repeat), Act::Repeat, t!("gui.tip.repeat").to_string()) + 1;
    } else {
        x += tall_compact(frame, s, x, y, "⇄", TallKind::Toggle(v.shuffle), Act::Shuffle, t!("gui.tip.shuffle").to_string()) + 1;
        x += tall_compact(frame, s, x, y, "↻", TallKind::Toggle(v.repeat), Act::Repeat, t!("gui.tip.repeat").to_string()) + 1;
    }
    tall_compact(frame, s, x, y, "auto-dj", TallKind::Toggle(v.autodj), Act::AutoDj, t!("gui.tip.dj").to_string());
    draw_volume(frame, s, 1, y + 1, v.volume, false);

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

fn queue_tip(open: bool) -> String {
    if open { t!("gui.queue.hide_tip").to_string() } else { t!("gui.queue.show_tip").to_string() }
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
    fn resampling_keeps_the_peaks() {
        // Averaging would flatten the spike; the bucket keeps its peak.
        let wave = vec![10, 10, 250, 10, 10, 10, 10, 10];
        let out = resample(&wave, 4);
        assert_eq!(out.len(), 4);
        assert_eq!(out[1], 250);
        // Upsampling repeats rather than inventing.
        assert_eq!(resample(&[100, 200], 4), vec![100, 100, 200, 200]);
        assert_eq!(resample(&[], 3), vec![0, 0, 0]);
    }

    #[test]
    fn amplitude_levels_span_the_eight_blocks() {
        assert_eq!(amp_level(0), 1);
        assert_eq!(amp_level(255), 8);
        assert!(amp_level(128) >= 4 && amp_level(128) <= 5);
    }

    #[test]
    fn volume_cells_round_honestly() {
        assert_eq!(volume_cells(0.0), 0);
        assert_eq!(volume_cells(0.72), 7);
        assert_eq!(volume_cells(1.0), 10);
        assert_eq!(volume_cells(2.0), 10, "overdrive clamps to the bar");
    }

    #[test]
    fn bar_style_round_trips_and_falls_back() {
        assert_eq!(BarStyle::from_config("wave"), BarStyle::Wave);
        assert_eq!(BarStyle::from_config("gold-line"), BarStyle::GoldLine);
        assert_eq!(BarStyle::from_config("holographic"), BarStyle::Wave, "unknown falls back");
        for style in [BarStyle::Wave, BarStyle::GoldLine] {
            assert_eq!(BarStyle::from_config(style.config_name()), style);
        }
    }
}
