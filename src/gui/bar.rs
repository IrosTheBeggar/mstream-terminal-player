//! The GUI player's bottom bar, in its two styles.
//!
//! Both draw into the same five bottom rows and share one contract: the
//! now-playing summary is the click target that shows and hides the queue,
//! the seek surface always agrees with the timestamps, and every control
//! keeps a key named in the tips line. Style is a Settings choice:
//!
//! - **Wave**: the gold rule, then the track's waveform over its reflection,
//!   then the compact 1-row transport/toggles/volume row.
//! - **GoldLine**: the gold rule IS the seek bar, with the song info on the
//!   left and the tall 3-row controls beneath it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::kit::{Surface, dim, tall_secondary};
use crate::kit::theme::{legacy_conhost, th};
use rust_i18n::t;

use super::Act;

/// Rows the bar owns at the bottom of the screen (the tips line above it is
/// the screen's, not the bar's).
pub(super) const BAR_ROWS: u16 = 5;

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

/// The tall toggle for the GoldLine bar: the kit's 3-row Rounded frame
/// wearing the toggle grammar — border and label DIM off, OK green on,
/// BRIGHT under the pointer (the toggle-card colors, in button form).
fn fat_toggle(
    frame: &mut Frame,
    s: &mut Surface<Act>,
    x: u16,
    y: u16,
    label: &str,
    on: bool,
    act: Act,
    tip: String,
) -> u16 {
    let text = format!("  {label}  ");
    let width = text.chars().count() as u16 + 2;
    let rect = Rect { x, y, width, height: 3 };
    let color = match (on, hovered(s, rect)) {
        (_, true) => th().bright,
        (true, false) => th().ok,
        (false, false) => th().dim,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let style = if on || hovered(s, rect) {
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
/// the readout says where it stands. Returns nothing; registers everything.
fn draw_volume(frame: &mut Frame, s: &mut Surface<Act>, x: u16, y: u16, volume: f32) {
    let legacy = legacy_conhost();
    let (full, empty) = if legacy { ('■', '·') } else { ('▰', '▱') };
    put(frame, x, y, "vol", dim());
    let minus = Rect { x: x + 4, y, width: 1, height: 1 };
    put(frame, x + 4, y, "-", if hovered(s, minus) { bright_bold() } else { dim() });
    s.click(minus, Act::VolDown);
    let filled = volume_cells(volume);
    for i in 0..10u16 {
        let cell = Rect { x: x + 6 + i, y, width: 1, height: 1 };
        let on = (i as usize) < filled;
        let style = if on { Style::default().fg(th().accent) } else { dim() };
        let glyph = if on { full } else { empty };
        put(frame, cell.x, y, &glyph.to_string(), style);
        s.click(cell, Act::VolSet(i as u8));
    }
    let plus = Rect { x: x + 17, y, width: 1, height: 1 };
    put(frame, x + 17, y, "+", if hovered(s, plus) { bright_bold() } else { dim() });
    s.click(plus, Act::VolUp);
    put(frame, x + 19, y, &format!("{:3}%", (volume * 100.0).round() as u32), dim());
    s.tip(
        Rect { x, y, width: 23, height: 1 },
        t!("gui.tip.volume").to_string(),
    );
}

fn bright_bold() -> Style {
    Style::default().fg(th().bright).add_modifier(Modifier::BOLD)
}

/// The compact transport + toggles row (the Wave bar's control row).
fn draw_compact_controls(frame: &mut Frame, s: &mut Surface<Act>, y: u16, left_end: u16, v: &BarView) {
    let legacy = legacy_conhost();
    let (prev, next) = if legacy { ("<<", ">>") } else { ("◂◂", "▸▸") };
    // The play/pause slot is a fixed two cells, so ▮▮ swaps in place and
    // next never moves between states.
    let play = match (legacy, v.paused) {
        (false, true) => "▸ ",
        (false, false) => "▮▮",
        (true, true) => "> ",
        (true, false) => "||",
    };
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
    draw_volume(frame, s, left_end.saturating_sub(22), y, v.volume);
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

/// Draw the bar into the bottom [`BAR_ROWS`] rows of `area`.
pub(super) fn draw(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, style: BarStyle, v: &BarView) {
    let top = area.height - BAR_ROWS;
    match style {
        BarStyle::Wave => draw_wave_bar(frame, s, area, top, v),
        BarStyle::GoldLine => draw_gold_bar(frame, s, area, top, v),
    }
}

fn gold_rule(frame: &mut Frame, y: u16, width: u16) {
    put(frame, 0, y, &"─".repeat(width as usize), Style::default().fg(th().gold));
}

/// Wave style: rule · waveform+reflection · controls · (blank). The
/// now-playing card sits right, exactly the queue's width, and toggles it.
fn draw_wave_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    gold_rule(frame, top, area.width);
    let left_end = area.width - 35;
    match v.now {
        Some(now) => {
            put(frame, 1, top + 1, &fmt_time(now.elapsed), dim());
            let cells = (left_end - 12) as usize;
            let preview = draw_seek_cells(frame, s, 6, top + 1, 2, cells, now, false);
            if let Some(time) = preview {
                put(frame, 1, top + 1, &format!("{time:>4}"), bright_bold());
            }
            let total = fmt_time(now.duration);
            put(frame, left_end - total.chars().count() as u16, top + 1, &total, dim());
        }
        None => {
            // Idle: a resting line where the wave will be, and honest silence
            // for the times.
            put(frame, 6, top + 1, &"─".repeat((left_end - 12) as usize), dim());
        }
    }
    draw_compact_controls(frame, s, top + 3, left_end, v);
    draw_card(frame, s, area, top + 1, v);
}

/// The now-playing card (Wave bar): cover slot + title/artist + the ▾/▴
/// chevron, spanning exactly the queue's column. One click target.
fn draw_card(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, y: u16, v: &BarView) {
    let x = area.width - 32;
    let rect = Rect { x, y, width: 31, height: 4 };
    let hover = hovered(s, rect);
    cover_slot(frame, x, y, 8, 4);
    let (title_style, sub_style) = card_styles(hover, v.now.is_some());
    match v.now {
        Some(now) => {
            put(frame, x + 9, y + 1, &clip(&now.title, 20), title_style);
            put(frame, x + 9, y + 2, &clip(&now.artist, 20), sub_style);
        }
        None => put(frame, x + 9, y + 1, &t!("gui.nothing_playing"), sub_style),
    }
    let chevron = chevron_glyph(v.queue_open);
    put(frame, area.width - 3, y, chevron, if hover { bright_bold() } else { dim() });
    s.click(rect, Act::ToggleQueue);
    s.tip(rect, queue_tip(v.queue_open));
}

/// GoldLine style: the rule is the seek bar; song info left; tall controls.
fn draw_gold_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, top: u16, v: &BarView) {
    match v.now {
        Some(now) => {
            put(frame, 1, top, &fmt_time(now.elapsed), dim());
            let cells = (area.width - 12) as usize;
            let preview = draw_seek_cells(frame, s, 6, top, 1, cells, now, true);
            if let Some(time) = preview {
                put(frame, 1, top, &format!("{time:>4}"), bright_bold());
            }
            let total = fmt_time(now.duration);
            put(frame, area.width - 1 - total.chars().count() as u16, top, &total, dim());
        }
        // Idle, the line is exactly the wizard's gold rule.
        None => gold_rule(frame, top, area.width),
    }

    // Song info, left — the queue toggle, chevron on the title row.
    let info = Rect { x: 1, y: top + 1, width: 28, height: 3 };
    let hover = hovered(s, info);
    cover_slot(frame, 1, top + 1, 6, 3);
    let (title_style, sub_style) = card_styles(hover, v.now.is_some());
    match v.now {
        Some(now) => {
            put(frame, 8, top + 1, &clip(&now.title, 18), title_style);
            put(frame, 8, top + 2, &clip(&now.artist, 18), sub_style);
        }
        None => put(frame, 8, top + 1, &t!("gui.nothing_playing"), sub_style),
    }
    put(frame, 27, top + 1, chevron_glyph(v.queue_open), if hover { bright_bold() } else { dim() });
    s.click(info, Act::ToggleQueue);
    s.tip(info, queue_tip(v.queue_open));

    // The fat controls: prev · play · next as kit tall buttons, then the
    // toggles in the same 3-row form. The play slot's label is two cells
    // both ways, so the row never re-seats between paused and playing.
    let legacy = legacy_conhost();
    let (prev, next) = if legacy { ("<<", ">>") } else { ("◂◂", "▸▸") };
    let play = match (legacy, v.paused) {
        (false, true) => "▸ ",
        (false, false) => "▮▮",
        (true, true) => "> ",
        (true, false) => "||",
    };
    let y = top + 1;
    let mut x = 30u16;
    let r = tall_secondary(frame, s, Rect { x, y, width: 10, height: 3 }, prev, Act::Prev);
    s.tip(r, t!("gui.tip.prev").to_string());
    x += r.width + 1;
    let r = play_tall(frame, s, x, y, play, Act::PlayPause);
    s.tip(r, t!("gui.tip.play").to_string());
    x += r.width + 1;
    let r = tall_secondary(frame, s, Rect { x, y, width: 10, height: 3 }, next, Act::Next);
    s.tip(r, t!("gui.tip.next").to_string());
    x += r.width + 1;
    if legacy {
        x += fat_toggle(frame, s, x, y, &t!("gui.shuffle_word"), v.shuffle, Act::Shuffle, t!("gui.tip.shuffle").to_string()) + 1;
        x += fat_toggle(frame, s, x, y, &t!("gui.repeat_word"), v.repeat, Act::Repeat, t!("gui.tip.repeat").to_string()) + 1;
    } else {
        x += fat_toggle(frame, s, x, y, "⇄", v.shuffle, Act::Shuffle, t!("gui.tip.shuffle").to_string()) + 1;
        x += fat_toggle(frame, s, x, y, "↻", v.repeat, Act::Repeat, t!("gui.tip.repeat").to_string()) + 1;
    }
    fat_toggle(frame, s, x, y, "auto-dj", v.autodj, Act::AutoDj, t!("gui.tip.dj").to_string());

    // Volume rides the bar's last row, far right — the tall row keeps its
    // room and auto-dj keeps its word.
    draw_volume(frame, s, area.width - 25, top + 4, v.volume);
}

/// The play/pause tall button: the kit primary's geometry with a caller-set
/// two-cell label (kit `tall_button` appends its own `▸`-less padding; the
/// play glyph IS the label here).
fn play_tall(frame: &mut Frame, s: &mut Surface<Act>, x: u16, y: u16, label: &str, act: Act) -> Rect {
    let text = format!("  {label}  ");
    let width = text.chars().count() as u16 + 2;
    let rect = Rect { x, y, width, height: 3 };
    let color = if hovered(s, rect) { th().bright } else { th().accent };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(color).add_modifier(Modifier::BOLD))),
        inner,
    );
    s.click(rect, act);
    rect
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
