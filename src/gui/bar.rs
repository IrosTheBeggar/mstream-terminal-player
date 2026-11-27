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
    pub album: String,
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
pub(crate) struct BarView<'a> {
    pub now: Option<&'a Now>,
    pub paused: bool,
    /// 0.0..=1.0.
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: bool,
    pub autodj: bool,
    /// Quick config is up over the bar: the split button's chevron points
    /// down to it.
    pub quick_open: bool,
    pub queue_open: bool,
    /// The playing track's cover is decoded and ready: the card skips its
    /// empty slot frame and the screen paints the art over those cells
    /// after the bar (pixels where the terminal can, the mosaic elsewhere).
    pub has_art: bool,
    /// The screen keeps its tips line under the bar: the bar sits one row
    /// higher.
    pub footer: bool,
    /// The note on the bar's bottom row and its tone: the shell's own
    /// first, else the App's words (`Gui::note`). The cells' note is the
    /// shell's to draw, after the screen; the egui face draws this one.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub note: Option<(String, NoteTone)>,
    /// The playing track's cover, decoded (`has_art`'s picture), for a face
    /// that draws it itself rather than through the shell's cover slot.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub art: Option<&'a crate::tui::art::Art>,
}

/// A note's tone: the shell's own note Dim, its error and the App's Error
/// in the gold; the App's Info Dim, Busy in the accent, a landing's
/// Success in the ok green, Plain in the text colour (the kit's Status
/// line; discovery-page clause 118).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteTone {
    Dim,
    Error,
    Busy,
    Success,
    Plain,
}

impl NoteTone {
    /// The tone in cells.
    pub(super) fn style(self) -> Style {
        match self {
            NoteTone::Dim => dim(),
            NoteTone::Error => Style::default().fg(th().gold),
            NoteTone::Busy => super::accent(),
            NoteTone::Success => Style::default().fg(th().ok),
            NoteTone::Plain => Style::default(),
        }
    }
}

/// The bar in cells, as the cells face draws it and registers its acts
/// there: every control's box, so a face that paints the bar in another
/// medium paints each control in the very cells a press on it works.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BarLayout {
    /// The bar's five rows, from its seek line.
    pub rect: Rect,
    /// The seek line's parts; None while nothing plays, when the line is
    /// the gold rule across the row.
    pub seek: Option<SeekLayout>,
    /// auto-dj's split: the word `Act::AutoDj`, the chevron
    /// `Act::DjQuick(QuickAct::Toggle)`.
    pub dj: SplitLayout,
    /// Repeat, prev, play/pause, next, shuffle: three rows each.
    pub group: [(Rect, Act); 5],
    pub volume: VolumeLayout,
    /// Where the shell's note stands ([`note_rect`]).
    pub note: Rect,
    pub card: CardLayout,
}

/// The seek line: the elapsed time's slot, the track's cells (a press on
/// cell `i` of `cells` is `Act::Seek(cell_fraction(i, cells))`), the
/// total's slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeekLayout {
    pub elapsed: Rect,
    pub track: Rect,
    pub cells: usize,
    pub total: Rect,
}

/// The split button: its whole frame, the word's half (which takes the
/// rule's column), the chevron's, and the rule's column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SplitLayout {
    pub frame: Rect,
    pub word: Rect,
    pub chevron: Rect,
    pub rule_x: u16,
}

/// The volume group: `-` (`Act::VolDown`), ten cells (`Act::VolSet(i)`),
/// `+` (`Act::VolUp`) and the readout, which takes no press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VolumeLayout {
    pub minus: Rect,
    pub cells: [Rect; 10],
    pub plus: Rect,
    pub readout: Rect,
}

/// The card: its whole rect (`Act::ToggleQueue`, and `Act::NowMore` on a
/// right press while something is up on it), the cover's cells, its four
/// lines of words and the queue's chevron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CardLayout {
    pub rect: Rect,
    pub cover: Rect,
    pub lines: [Rect; 4],
    pub chevron: Rect,
}

/// The auto-dj word, as the cells have always worn it.
const DJ_WORD: &str = " auto-dj ";

/// The transport group's five labels and what each does, left to right.
fn group_labels(v: &BarView) -> [(Cow<'static, str>, Act); 5] {
    let legacy = legacy_conhost();
    let (shuffle, repeat) = if legacy {
        (Cow::Owned(t!("gui.shuffle_word").to_string()), Cow::Owned(t!("gui.repeat_word").to_string()))
    } else {
        (Cow::Borrowed("⇄"), Cow::Borrowed("↻"))
    };
    let (prev, play, next) = play_glyphs(v.paused);
    [
        (repeat, Act::Repeat),
        (Cow::Borrowed(prev), Act::Prev),
        (Cow::Borrowed(play), Act::PlayPause),
        (Cow::Borrowed(next), Act::Next),
        (shuffle, Act::Shuffle),
    ]
}

/// The seek line's parts in the span from `x`, `width` cells wide: both
/// time slots as wide as the total, and the cells between them.
fn seek_spans(x: u16, y: u16, width: u16, now: &Now) -> SeekLayout {
    let slot = fmt_time(now.duration).chars().count().max(4) as u16;
    let cells = width.saturating_sub(2 * (slot + 1));
    SeekLayout {
        elapsed: Rect { x, y, width: slot, height: 1 },
        track: Rect { x: x + slot + 1, y, width: cells, height: 1 },
        cells: cells as usize,
        total: Rect { x: (x + width).saturating_sub(slot), y, width: slot, height: 1 },
    }
}

/// Where every control of the bar stands in `area` for `v`: the boxes the
/// cells face draws and registers.
pub(crate) fn layout(area: Rect, v: &BarView) -> BarLayout {
    let top = top(area, v.footer);
    let rect = Rect { x: 0, y: top, width: area.width, height: BAR_ROWS };
    let seek = v.now.map(|now| seek_spans(1, top, area.width.saturating_sub(2), now));
    let y = top + 1;
    let word_w = DJ_WORD.len() as u16;
    let dj = SplitLayout {
        frame: Rect { x: DJ_X, y, width: word_w + 6, height: 3 },
        word: Rect { x: DJ_X, y, width: word_w + 2, height: 3 },
        chevron: Rect { x: DJ_X + word_w + 2, y, width: 4, height: 3 },
        rule_x: DJ_X + word_w + 1,
    };
    let labels = group_labels(v);
    let widths = labels.each_ref().map(|(label, _)| crate::kit::width(label) as u16 + 4);
    let group_w: u16 = widths.iter().sum::<u16>() + (widths.len() as u16 - 1);
    let free_from = DJ_X + dj.frame.width + 1;
    let free_to = card_x(area).saturating_sub(1);
    let mut x = free_from + free_to.saturating_sub(free_from).saturating_sub(group_w) / 2;
    let group = labels.map(|(label, act)| {
        let width = crate::kit::width(&label) as u16 + 4;
        let rect = Rect { x, y, width, height: 3 };
        x += width + 1;
        (rect, act)
    });
    let vy = top + 4;
    let volume = VolumeLayout {
        minus: Rect { x: 1, y: vy, width: 1, height: 1 },
        cells: std::array::from_fn(|i| Rect { x: 3 + i as u16, y: vy, width: 1, height: 1 }),
        plus: Rect { x: 14, y: vy, width: 1, height: 1 },
        readout: Rect { x: 16, y: vy, width: 4, height: 1 },
    };
    let cx = card_x(area);
    let tx = cx + COVER_W + 2;
    let text_w = area.width.saturating_sub(4).saturating_sub(tx);
    let card = CardLayout {
        rect: Rect { x: cx, y, width: CARD_W, height: COVER_H },
        cover: Rect { x: cx, y, width: COVER_W, height: COVER_H },
        lines: [
            Rect { x: tx, y, width: text_w, height: 1 },
            Rect { x: tx, y: y + 1, width: text_w, height: 1 },
            Rect { x: tx, y: y + 2, width: text_w, height: 1 },
            Rect { x: tx, y: y + 3, width: area.width.saturating_sub(1).saturating_sub(tx), height: 1 },
        ],
        chevron: Rect { x: area.width.saturating_sub(3), y, width: 1, height: 1 },
    };
    BarLayout { rect, seek, dj, group, volume, note: note_rect(area, v.footer), card }
}

/// The card's second line: the artist and the year, or whichever of the
/// two the track has.
pub(crate) fn byline(now: &Now) -> String {
    match now.year {
        Some(year) if !now.artist.is_empty() => format!("{} · {year}", now.artist),
        Some(year) => year.to_string(),
        None => now.artist.clone(),
    }
}

/// The bar's first row — the seek line — for this frame.
pub(super) fn top(area: Rect, footer: bool) -> u16 {
    area.height - BAR_ROWS - u16::from(footer)
}

fn card_x(area: Rect) -> u16 {
    area.width.saturating_sub(CARD_W)
}

/// The card's cover cells — where the screen paints the album art after
/// the bar.
pub(super) fn cover_rect(area: Rect, footer: bool) -> Rect {
    Rect { x: card_x(area), y: top(area, footer) + 1, width: COVER_W, height: COVER_H }
}

/// Where the screen's note (or an armed pick's banner) goes: the bottom
/// row, between the volume group and the card.
pub(super) fn note_rect(area: Rect, footer: bool) -> Rect {
    Rect { x: NOTE_X, y: top(area, footer) + 4, width: note_width(area.width), height: 1 }
}

/// The note's cells in a window `width` wide, for words fitted before
/// they are drawn (a path clipped at its front). Saturating, since a
/// window under the minimum draws the mini player and no note at all.
pub(super) fn note_width(width: u16) -> u16 {
    width.saturating_sub(CARD_W).saturating_sub(NOTE_X + 2)
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
pub(super) enum TallKind {
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
pub(super) fn tall_compact(
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

/// The auto-dj control, split (the record's split button): one rounded
/// frame with a rule between its halves — the word starts and stops as it
/// always has, DIM off and OK green on, and the chevron opens quick config
/// over the bar. Each half brightens under the pointer on its own. Returns
/// the frame's width.
fn dj_split(frame: &mut Frame, s: &mut Surface<Act>, at: &SplitLayout, on: bool, open: bool) -> u16 {
    let (x, y) = (at.frame.x, at.frame.y);
    let rect = at.frame;
    // The word's half takes the rule's column, the chevron's the rest.
    let (word, chevron) = (at.word, at.chevron);
    let (color, bold) = match (on, s.hovers(word)) {
        (_, true) => (th().bright, true),
        (true, false) => (th().ok, true),
        (false, false) => (th().dim, false),
    };
    let frame_style = Style::default().fg(color);
    frame.render_widget(
        Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(frame_style),
        rect,
    );
    let rule = at.rule_x;
    let (tee_down, bar, tee_up) = if legacy_conhost() { ("+", "|", "+") } else { ("┬", "│", "┴") };
    put(frame, rule, y, tee_down, frame_style);
    put(frame, rule, y + 1, bar, frame_style);
    put(frame, rule, y + 2, tee_up, frame_style);
    let word_style = if bold { frame_style.add_modifier(Modifier::BOLD) } else { frame_style };
    put(frame, x + 1, y + 1, DJ_WORD, word_style);
    let glyph = match (legacy_conhost(), open) {
        (false, false) => "▴",
        (false, true) => "▾",
        (true, false) => "^",
        (true, true) => "v",
    };
    let chevron_style = if s.hovers(chevron) || open { bright_bold() } else { dim() };
    put(frame, rule + 2, y + 1, glyph, chevron_style);
    s.click(word, Act::AutoDj);
    s.click(chevron, Act::DjQuick(super::dj::QuickAct::Toggle));
    rect.width
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
fn draw_volume(frame: &mut Frame, s: &mut Surface<Act>, at: &VolumeLayout, volume: f32) {
    let legacy = legacy_conhost();
    let (full, empty) = if legacy { ('■', '·') } else { ('▰', '▱') };
    let minus = at.minus;
    put(frame, minus.x, minus.y, "-", if s.hovers(minus) { bright_bold() } else { dim() });
    s.click(minus, Act::VolDown);
    let filled = volume_cells(volume);
    for (i, cell) in at.cells.iter().enumerate() {
        let on = i < filled;
        let style = if on { Style::default().fg(th().accent) } else { dim() };
        let glyph = if on { full } else { empty };
        put(frame, cell.x, cell.y, &glyph.to_string(), style);
        s.click(*cell, Act::VolSet(i as u8));
    }
    let plus = at.plus;
    put(frame, plus.x, plus.y, "+", if s.hovers(plus) { bright_bold() } else { dim() });
    s.click(plus, Act::VolUp);
    put(frame, at.readout.x, at.readout.y, &volume_words(volume), dim());
}

/// The volume's readout: its percentage, three figures wide.
pub(crate) fn volume_words(volume: f32) -> String {
    format!("{:3}%", (volume * 100.0).round() as u32)
}

/// The play/pause slot glyphs: a fixed two cells, so ▮▮ swaps in place and
/// next never moves between states.
pub(super) fn play_glyphs(paused: bool) -> (&'static str, &'static str, &'static str) {
    if legacy_conhost() {
        ("<<", if paused { "> " } else { "||" }, ">>")
    } else {
        ("◂◂", if paused { "▸ " } else { "▮▮" }, "▸▸")
    }
}

/// The seek line in the span from `x`, `width` cells wide: the elapsed
/// time, the gold rule's cells, the total — the bar's across the screen,
/// the mini player's under the song. Both time slots are as wide as the
/// total: a track past a hundred minutes reads 100:00, and the cells start
/// after it rather than under it. The time under the pointer previews in
/// the elapsed slot.
pub(super) fn seek_line(frame: &mut Frame, s: &mut Surface<Act>, x: u16, y: u16, width: u16, now: &Now) {
    let spans = seek_spans(x, y, width, now);
    let total = fmt_time(now.duration);
    let slot = usize::from(spans.elapsed.width);
    put(frame, x, y, &format!("{:>slot$}", fmt_time(now.elapsed)), dim());
    let preview = draw_seek_cells(frame, s, spans.track.x, y, spans.cells, now);
    if let Some(time) = preview {
        put(frame, x, y, &format!("{time:>slot$}"), bright_bold());
    }
    put(frame, spans.total.x, y, &total, dim());
}

/// The seek line's cells: the gold rule across the playing span, a click
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

/// Draw the bar into its rows of `area` (see [`BAR_ROWS`]), its acts
/// registered on `s`: the layout it drew and registered.
pub(super) fn draw(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, v: &BarView) -> BarLayout {
    let layout = layout(area, v);
    draw_gold_bar(frame, s, area, &layout, v);
    layout
}

fn gold_rule(frame: &mut Frame, y: u16, width: u16) {
    put(frame, 0, y, &"─".repeat(width as usize), Style::default().fg(th().gold));
}

/// The now-playing card: the cover, four lines of words beside it —
/// title, artist and year, the spec line, the rating with key and tempo
/// — and the ▾/▴ chevron, four rows tall at the right edge. One click
/// target; a right click (or `m`) is the playing track's sheet — the card
/// wears no verb of its own.
fn draw_card(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, at: &CardLayout, v: &BarView) {
    let (x, y) = (at.rect.x, at.rect.y);
    let rect = at.rect;
    let hover = s.hovers(rect);
    // With art in hand the frame would only bleed around the picture's
    // edges — the screen paints the cover over these cells after the bar.
    if !v.has_art {
        cover_slot(frame, x, y, at.cover.width, at.cover.height);
    }
    let tx = at.lines[0].x;
    // The words stop short of the chevron's column.
    let text_w = usize::from(at.lines[0].width);
    let (title_style, sub_style) = card_styles(hover, v.now.is_some());
    match v.now {
        Some(now) => {
            put(frame, tx, y, &clip(&now.title, text_w), title_style);
            put(frame, tx, y + 1, &clip(&byline(now), text_w), sub_style);
            put(frame, tx, y + 2, &clip(&now.spec, text_w), dim());
            let mut fx = tx;
            if let Some(rating) = now.rating.filter(|r| *r > 0) {
                let glyphs = super::actions::stars(Some(rating));
                put(frame, fx, y + 3, &glyphs, Style::default().fg(th().gold));
                fx += glyphs.chars().count() as u16 + 2;
            }
            if let Some(facts) = facts(now) {
                let room = (area.width - 1).saturating_sub(fx) as usize;
                put(frame, fx, y + 3, &clip(&facts, room), dim());
            }
        }
        None => put(frame, tx, y, &t!("gui.nothing_playing"), sub_style),
    }
    let chevron = chevron_glyph(v.queue_open);
    put(frame, at.chevron.x, y, chevron, if hover { bright_bold() } else { dim() });
    s.click(rect, Act::ToggleQueue);
    if v.now.is_some() {
        s.context(rect, Act::NowMore);
    }
}

/// The rule is the seek bar; beneath it auto-dj at the left edge and one
/// centred group of frames — repeat, prev, play, next, shuffle — in the
/// span before the card, and the bottom row: the volume, then the screen's
/// note, then the card's last line.
fn draw_gold_bar(frame: &mut Frame, s: &mut Surface<Act>, area: Rect, at: &BarLayout, v: &BarView) {
    let line = at.rect.y;
    match v.now {
        Some(now) => seek_line(frame, s, 1, line, area.width.saturating_sub(2), now),
        // Idle, the line is exactly the wizard's gold rule.
        None => gold_rule(frame, line, area.width),
    }

    // Auto DJ stands near the left edge in its frame; the rest is one group
    // of frames — repeat, then prev, play, next, then shuffle — centred in
    // the span between it and the card, so the group holds the middle at
    // any width (`layout`).
    dj_split(frame, s, &at.dj, v.autodj, v.quick_open);
    let kinds = [TallKind::Toggle(v.repeat), TallKind::Strong, TallKind::Primary, TallKind::Strong, TallKind::Toggle(v.shuffle)];
    for (((label, _), (rect, act)), kind) in group_labels(v).iter().zip(&at.group).zip(kinds) {
        tall_compact(frame, s, rect.x, rect.y, label, kind, act.clone());
    }
    draw_volume(frame, s, &at.volume, v.volume);

    draw_card(frame, s, area, &at.card, v);
}

/// The card's last facts after the stars: the key and the tempo, where the
/// track has them — `♪ 8A · 120 BPM`. The mini player says them the same.
pub(super) fn facts(now: &Now) -> Option<String> {
    let mut facts: Vec<String> = Vec::new();
    if let Some(key) = now.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        facts.push(format!("{} {key}", if legacy_conhost() { "key" } else { "♪" }));
    }
    if let Some(bpm) = now.bpm.filter(|b| *b > 0) {
        facts.push(format!("{bpm} BPM"));
    }
    (!facts.is_empty()).then(|| facts.join(" · "))
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
/// Measured in cells, not characters: a CJK label is two cells a glyph,
/// and a budget counted in characters let it run into its neighbours.
pub(super) fn clip(text: &str, max: usize) -> Cow<'_, str> {
    if crate::kit::width(text) <= max {
        return Cow::Borrowed(text);
    }
    // Keep what fits before the mark's own cell, whole graphemes at the
    // cells ratatui draws them in: cut per character, ❤️ kept its ❤ at one
    // cell and its VS16 free, and drew two, past the label's budget.
    use unicode_segmentation::UnicodeSegmentation;
    let room = max.saturating_sub(1);
    let mut kept = 0;
    let mut cut = 0;
    for (i, grapheme) in text.grapheme_indices(true) {
        let w = crate::kit::grapheme_cells(grapheme);
        if kept + w > room {
            break;
        }
        kept += w;
        cut = i + grapheme.len();
    }
    let mut out = String::with_capacity(cut + 3);
    out.push_str(&text[..cut]);
    out.push(if legacy_conhost() { '»' } else { '…' });
    Cow::Owned(out)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A clipped label takes no more than its budget in the cells ratatui draws it in, and no
    /// fewer than a cell short of it (a two-cell cluster that does not fit beside the mark):
    /// cut per character, ❤️ and 1️⃣ ran a cell past the budget into the label after it, and a
    /// ZWJ family was counted six cells for its two and left out where it fit.
    #[test]
    fn a_clipped_label_stays_inside_its_cells() {
        use ratatui::{Terminal, backend::TestBackend};
        let clusters = [
            "\u{2764}\u{FE0F}",
            "1\u{FE0F}\u{20E3}",
            "\u{1F1FA}\u{1F1F8}",
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
            "\u{1F44D}\u{1F3FD}",
            "\u{D55C}",
            "\u{304B}",
            "\u{65E5}",
            "e\u{301}",
            "music",
        ];
        let mut labels: Vec<String> = clusters.iter().map(|c| c.repeat(12)).collect();
        labels.extend(clusters.iter().map(|c| format!("ab{c}cd{c}ef{c}gh{c}")));
        labels.push(clusters.concat());
        for label in &labels {
            for max in 1..30usize {
                let shown = clip(label, max);
                let cells = crate::kit::width(&shown);
                assert!(cells <= max, "{shown:?} is {cells} cells, over {max}");
                if crate::kit::width(label) > max {
                    assert!(cells + 1 >= max, "{shown:?} is {cells} cells, wasting {max}");
                }
                let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
                terminal
                    .draw(|frame| {
                        let buffer = frame.buffer_mut();
                        for x in 0..40 {
                            buffer[(x, 0)].set_symbol("#");
                        }
                        buffer.set_stringn(0, 0, &shown, usize::MAX, Style::default());
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                for x in max as u16..40 {
                    assert_eq!(buffer[(x, 0)].symbol(), "#", "{shown:?} in {max} wrote cell {x}");
                }
            }
        }
    }

    /// Every box of the bar's layout is where the cells register its act,
    /// every cell of it: the split's halves, the five tall controls, the
    /// volume's ends and cells, the seek line's cells (its two times take no
    /// press), the card whole, and the card's right press while something
    /// is up on it — at the window's opening size, at the floor with the
    /// tips row under the bar, and wide; playing and idle.
    #[test]
    fn the_layout_is_where_the_cells_register_every_act() {
        use ratatui::layout::Position;
        use ratatui::{Terminal, backend::TestBackend};
        let now = Now {
            title: "Cassini IV".into(),
            artist: "Vela".into(),
            album: "Cassini".into(),
            elapsed: 47.0,
            duration: 302.0,
            year: Some(2019),
            spec: "FLAC · 912 kbps".into(),
            rating: Some(8),
            key: Some("8A".into()),
            bpm: Some(120),
        };
        for (w, h, footer) in [(100, 30, false), (100, 24, true), (160, 50, false)] {
            for playing in [true, false] {
                let v = BarView {
                    now: playing.then_some(&now),
                    paused: playing,
                    volume: 0.7,
                    shuffle: false,
                    repeat: true,
                    autodj: false,
                    quick_open: false,
                    queue_open: true,
                    has_art: false,
                    footer,
                    note: None,
                    art: None,
                };
                let mut s: Surface<Act> = Surface::new();
                let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                let mut drawn = None;
                terminal.draw(|frame| drawn = Some(draw(frame, &mut s, frame.area(), &v))).unwrap();
                let at = drawn.unwrap();
                assert_eq!(at, layout(Rect::new(0, 0, w, h), &v), "{w}×{h}: drawn as laid out");
                assert_eq!(at.rect, Rect { x: 0, y: top(Rect::new(0, 0, w, h), footer), width: w, height: BAR_ROWS });
                let every = |rect: Rect, act: Act| {
                    for y in rect.top()..rect.bottom() {
                        for x in rect.left()..rect.right() {
                            assert_eq!(s.hit(Position { x, y }), Some(act.clone()), "{w}×{h} playing {playing}: {x},{y} of {rect:?}");
                        }
                    }
                };
                every(at.dj.word, Act::AutoDj);
                every(at.dj.chevron, Act::DjQuick(super::super::dj::QuickAct::Toggle));
                for (rect, act) in &at.group {
                    every(*rect, act.clone());
                }
                every(at.volume.minus, Act::VolDown);
                every(at.volume.plus, Act::VolUp);
                for (i, cell) in at.volume.cells.iter().enumerate() {
                    every(*cell, Act::VolSet(i as u8));
                }
                assert_eq!(s.hit(Position { x: at.volume.readout.x, y: at.volume.readout.y }), None, "the readout takes no press");
                every(at.card.rect, Act::ToggleQueue);
                let context = s.hit_context(Position { x: at.card.cover.x, y: at.card.cover.y });
                assert_eq!(context, playing.then_some(Act::NowMore), "the card's right press while something is up on it");
                match at.seek {
                    Some(seek) => {
                        assert!(playing);
                        for i in 0..seek.cells {
                            every(Rect { x: seek.track.x + i as u16, width: 1, ..seek.track }, Act::Seek(cell_fraction(i, seek.cells)));
                        }
                        assert_eq!(seek.track.width as usize, seek.cells);
                        assert_eq!(s.hit(Position { x: seek.elapsed.x, y: seek.elapsed.y }), None);
                        assert_eq!(s.hit(Position { x: seek.total.right() - 1, y: seek.total.y }), None);
                    }
                    None => assert!(!playing, "idle, the line is the rule and takes no press"),
                }
                assert!(at.card.chevron.x < w && at.card.lines.iter().all(|line| line.right() <= w));
            }
        }
    }

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
