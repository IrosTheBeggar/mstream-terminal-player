//! The Now Playing view: the facts column with the cover beneath, the tabbed
//! panel, the rule, the waveform band and the modes. It reads app state and
//! draws widgets, with no decisions of its own. The GUI's Now Playing screen
//! (`gui::now`) draws it under its top bar.

use rust_i18n::t;
use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph,
};

use crate::api::types::fmt_duration;

use super::app::{App, DjRow, NowTab, Repeat};
use crate::api::types::TrackMetadata;

/// The glyphs the drawing code varies, resolved once. Process-wide rather
/// than threaded through every helper: the helpers are pure functions of a
/// row, and giving each one a glyph-set argument would be all noise and no
/// meaning.
static GLYPHS: OnceLock<Glyphs> = OnceLock::new();

/// What the terminal's font can be trusted to draw.
///
/// Not decoration: a font either has a character or it prints a box, and the
/// Windows 10 default console is conhost with Consolas, which stops at the
/// old CP437 repertoire. Probed rather than assumed — of what this UI draws,
/// Consolas is missing six of the eight eighth-blocks, `▏`, `▶`, `◆`, `★`,
/// `⏸` and every braille pattern, while keeping `█ ░ ▓ ▄ ─ │`. That is
/// exactly why the bar looked right before it learned to draw a waveform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyphs {
    /// Nine steps from empty to full, for the waveform.
    ///
    /// What the steps *mean* depends on [`Glyphs::mirrored`]: heights when
    /// the font has the eighth blocks, densities when it does not.
    ///
    /// When mirrored, this **must be symmetric under `8 - n`** — the lower
    /// half is drawn by indexing the complement and reversing, so ink(n) +
    /// ink(8-n) has to come to a full cell or the two halves disagree about
    /// where the middle is.
    pub eighths: [&'static str; 9],
    /// Whether the waveform is drawn as a shape mirrored about a centre
    /// line, or as one row.
    ///
    /// Not a preference — an arithmetic consequence. A console font offers
    /// exactly three heights (empty, `▄`, `█`), and the symmetry a mirror
    /// needs then forces every value between to the same half-height: the
    /// whole band comes out as a flat wall of `▄`. Density has four steps
    /// (`░▒▓█`) and needs no symmetry at all, so that is what a legacy
    /// console draws, in the single row it had before waveforms existed.
    pub mirrored: bool,
    /// The row on the speakers, and the caret that marks where a click would
    /// land or where typing is going.
    pub playing: &'static str,
    pub caret: &'static str,
    /// Paused, in the transport.
    pub paused: &'static str,
    /// The two ends of a Sonic Path — the tracks that were chosen, as
    /// against the waypoints between them.
    pub seed: &'static str,
    /// Frames of the "waiting on the server" spinner.
    pub spinner: &'static [&'static str],
}

/// Everything a modern terminal font has.
const RICH: Glyphs = Glyphs {
    eighths: [
        " ", "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}",
        "\u{2588}",
    ],
    mirrored: true,
    playing: "\u{25b6}",
    caret: "\u{258f}",
    paused: "\u{23f8}",
    seed: "\u{25c6}",
    spinner: &["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}"],
};

/// What CP437 had, which is what every console font still carries.
///
/// The waveform is drawn as density rather than height — `░▒▓█`, four steps
/// in one row. Coarse on purpose: a chunky shape that renders beats a
/// precise one that prints boxes.
const LEGACY: Glyphs = Glyphs {
    eighths: [
        " ", "\u{2591}", "\u{2591}", "\u{2592}", "\u{2592}", "\u{2593}", "\u{2593}", "\u{2588}",
        "\u{2588}",
    ],
    mirrored: false,
    playing: ">",
    caret: "|",
    paused: "=",
    seed: "*",
    // A spinner is motion, not shape; the ASCII one has been fine since the
    // seventies.
    spinner: &["|", "/", "-", "\\"],
};

impl Glyphs {
    /// `WT_SESSION` is set by Windows Terminal and not by conhost, which is
    /// the whole of the question on Windows. Everywhere else a modern font
    /// is a safe assumption.
    fn detect() -> Self {
        if !cfg!(windows) || std::env::var_os("WT_SESSION").is_some() { RICH } else { LEGACY }
    }
}

pub(crate) fn glyphs() -> &'static Glyphs {
    GLYPHS.get_or_init(Glyphs::detect)
}

// The three colours the view varies, as palette names rather than RGB: a
// terminal app that hard-codes hues looks wrong inside somebody else's
// colour scheme, and naming the slot lets their scheme answer.

/// What is playing, what is selected, the progress bar.
pub(crate) fn accent() -> Color {
    Color::Cyan
}

/// Labels, durations, rules, hints: everything secondary.
pub(crate) fn dim() -> Color {
    Color::DarkGray
}

/// Folders and library nodes.
pub(crate) fn folder() -> Color {
    Color::Blue
}

/// Which frame the spinner is on. The frame count differs between the two
/// glyph sets — braille turns in ten, ASCII in four — so the modulo has to
/// ask the set rather than a constant.
fn spinner_frame(app: &App) -> &'static str {
    app.spinner_shown.set(true);
    let frames = glyphs().spinner;
    frames[app.spinner % frames.len()]
}

/// Where the full-screen view's rows go, under its one-line title.
///
/// Body, then a rule, then the transport band and the key hints along the
/// foot. The band spans the full width rather than sitting in a column, so
/// the bar is long enough to read as a position rather than a stepper.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NowRegions {
    pub body: Rect,
    pub rule: Rect,
    pub gauge: Rect,
    pub keys: Rect,
}

/// What a caller adds to the shared full-screen view. The GUI's Now
/// Playing screen lays its transport under the cover and names its keys on
/// its own footer (docs/ux-contracts/now-playing.md); the default is the
/// view alone.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NowExtras {
    /// Rows kept free under the cover for the caller's controls.
    pub reserve: u16,
    /// Draw the cover as the mosaic even where pixels could go: an overlay
    /// stood over it last frame, and a picture's cells are skipped by the
    /// terminal writer (the GUI's rule for every cover).
    pub mosaic: bool,
}

/// Where the shared view put the parts a pointer surface wires up.
#[derive(Debug, Clone, Default)]
pub(crate) struct NowLayout {
    /// The cover, when one was drawn.
    pub cover: Option<Rect>,
    /// The column's rows under the card and the cover — the caller's.
    pub spare: Rect,
    /// The tab strip's click targets.
    pub strip: StripHits,
    /// The band: the mirrored shape over the scrubber, one control.
    pub band: Rect,
    /// The modes readout at the keys row's right: a note the caller puts
    /// on that row stops short of it.
    pub modes: Rect,
}

/// Where the tab strip's click targets landed: each tab by index when the
/// strip fits, else the arrows either side of the one tab it names.
#[derive(Debug, Clone, Default)]
pub(crate) struct StripHits {
    pub tabs: Vec<(usize, Rect)>,
    pub arrows: Option<(Rect, Rect)>,
}

/// Rows the full-screen transport gives the waveform *above* the bar.
///
/// The bar row is the other half of a shape mirrored about the line between
/// them, which is what makes it read as a waveform rather than as a bar
/// chart. One, because the halves have to match and the lower one is the
/// scrubber.
const WAVE_HALF_ROWS: u16 = 1;

/// What the full-screen transport claims above its bar.
///
/// Unconditional on height: that view's body is a facts column and a tabbed
/// panel, not a list you scroll, so a row spent here costs nobody a row of
/// music. Nothing at all on a terminal whose font cannot mirror — see
/// [`Glyphs::mirrored`].
fn wave_half_rows() -> u16 {
    if glyphs().mirrored { WAVE_HALF_ROWS } else { 0 }
}

pub(crate) fn now_regions(area: Rect) -> NowRegions {
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    };
    let [body, rule, gauge, keys] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1 + wave_half_rows()),
        Constraint::Length(1),
    ])
    .areas(inner);
    NowRegions { body, rule, gauge, keys }
}

/// The rows a `height`-row list will actually show — the correction `List`
/// applies at render time, applied first, so only these rows need building.
/// Every list here was building all of its rows to draw the thirty that fit:
/// eight milliseconds a frame in a 10,000-entry folder, at ten frames a
/// second, for nothing (audit #46).
///
/// The caller's state keeps the absolute offset and selection; the state
/// returned is for rendering the slice, with both rebased onto it.
fn visible_rows(
    state: &mut ListState,
    len: usize,
    height: u16,
) -> (std::ops::Range<usize>, ListState) {
    let height = height as usize;
    if len == 0 || height == 0 {
        return (0..0, ListState::default());
    }
    let selected = state.selected().map(|selected| selected.min(len - 1));
    let mut offset = state.offset().min(len - 1);
    if let Some(selected) = selected {
        if selected < offset {
            offset = selected;
        } else if selected + 1 > offset + height {
            offset = selected + 1 - height;
        }
    }
    *state.offset_mut() = offset;
    let mut window = ListState::default();
    window.select(selected.map(|selected| selected - offset));
    (offset..(offset + height).min(len), window)
}

/// A whole-queue length, in the coarsest unit that still says something.
pub(crate) fn fmt_span(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0m".to_string();
    }
    let total = seconds.round() as u64;
    match (total / 3600, (total % 3600) / 60) {
        (0, 0) => format!("{total}s"),
        (0, m) => format!("{m}m"),
        (h, m) => format!("{h}h {m:02}m"),
    }
}

/// The full-screen view under its title: the facts column with the cover
/// beneath, the tabbed panel, the rule, the band and the keys row. Shared
/// with the GUI's Now Playing screen (docs/ux-contracts/now-playing.md),
/// which draws it under its own top bar — the row this view spends on its
/// title — and lays its transport in the rows `extras.reserve` keeps free
/// under the cover. Returns where the parts a pointer surface wires up
/// landed.
pub(crate) fn render_now_view(
    frame: &mut Frame,
    regions: &NowRegions,
    app: &mut App,
    extras: &NowExtras,
) -> NowLayout {
    let NowRegions { body, rule, gauge: gauge_area, keys: keys_area } = *regions;

    // The tab strip gets the width it needs and the facts take what is left,
    // down to a floor where the labelled rows stop fitting. Splitting down the
    // middle instead truncated the last tab to "Vis" on an 88-column terminal.
    let strip = tab_strip_width(app);
    let left_width = body
        .width
        .saturating_sub(strip + 2)
        .clamp(FACTS_MIN_WIDTH, FACTS_MAX_WIDTH)
        .min(body.width);
    let [facts_area, panel_area] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);

    // The divider is the facts column's own right border, so it runs the full
    // height of the body and meets the rule below it at a proper junction.
    let divider = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::new().fg(dim()))
        .padding(Padding::horizontal(1));
    let facts_inner = divider.inner(facts_area);
    frame.render_widget(divider, facts_area);
    let card = now_playing_card(app, facts_inner.width as usize);
    // The card takes the rows it filled and the cover gets what is left
    // under it, one blank row apart — pinned under the facts it belongs
    // to rather than floating in the column's leftover space — short of
    // the rows a caller keeps for its own controls. Those begin under the
    // cover, or under the card when no cover was drawn.
    let used = (card.len() as u16).saturating_add(1);
    frame.render_widget(Paragraph::new(card), facts_inner);
    let leftover = Rect {
        y: facts_inner.y + used,
        height: facts_inner.height.saturating_sub(used),
        ..facts_inner
    };
    let room = Rect { height: leftover.height.saturating_sub(extras.reserve), ..leftover };
    let cover = render_facts_cover(frame, room, app, extras.mosaic);
    let spare_y = cover.map_or(leftover.y, |cover| cover.bottom());
    let spare = Rect { y: spare_y, height: leftover.bottom().saturating_sub(spare_y), ..leftover };
    let strip = render_now_panel(frame, panel_area, app);

    frame.render_widget(
        Paragraph::new(Span::styled(
            // The divider is the facts column's right border, which is its
            // last column — not the one after it. A junction at left_width
            // sat one to the right of the line it is meant to join.
            rule_with_junction(rule.width, left_width.saturating_sub(1)),
            Style::new().fg(dim()),
        )),
        rule,
    );

    // The band is a mirrored pair: the shape above the line, the scrubber
    // below it. Both are laid out against the same bar width, so they stack
    // into one object rather than two things that happen to be adjacent.
    let [wave_area, bar_area] =
        Layout::vertical([Constraint::Length(wave_half_rows()), Constraint::Length(1)])
            .areas(gauge_area);
    frame.render_widget(
        Paragraph::new(waveform_top_line(app, gauge_area.width as usize)),
        wave_area,
    );
    frame.render_widget(
        Paragraph::new(progress_line(app, gauge_area.width as usize, hovering(app, gauge_area), glyphs().mirrored)),
        bar_area,
    );

    // The modes, at the last row's right: going full screen should not
    // quietly hide whether shuffle is on. The rest of the row is the
    // caller's, for a note of its own.
    let modes = mode_readout(app, false);
    let [_, right] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length((width_of(&modes) as u16).min(keys_area.width)),
    ])
    .areas(keys_area);
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(dim()))).alignment(Alignment::Right),
        right,
    );
    NowLayout { cover, spare, strip, band: gauge_area, modes: right }
}

/// Bounds on the facts column. Below the floor the labelled rows stop fitting;
/// above the ceiling the tab panel is paying for whitespace nobody asked for.
const FACTS_MIN_WIDTH: u16 = 26;
const FACTS_MAX_WIDTH: u16 = 46;

/// One space between tabs; the brackets below do the separating.
const TAB_GAP: &str = " ";

/// What the whole strip wants, so the split can hand it over before the facts
/// take what's left. Every tab is bracketed or space-padded to the same width,
/// so this does not change as the selection moves.
fn tab_strip_width(app: &App) -> u16 {
    let tabs = app.now_tabs();
    // `+ 2` for the brackets, `+ 2` for the "n:" — the number is the key
    // that gets you here, and a key nobody can see is a key nobody presses.
    let names: usize = tabs.iter().map(|t| width_of(t.title()) + 4).sum();
    let gaps = tabs.len().saturating_sub(1) * TAB_GAP.len();
    (names + gaps) as u16
}

/// The tab strip, or — when even the adaptive split can't fit it — just the
/// one you are on, with arrows for the rest. A truncated last tab looks like a
/// bug; naming the current one and pointing at the others does not. Also
/// where each tab (or each arrow) landed in `strip`, for a pointer surface's
/// clicks — laid out here, once, so the hits cannot drift from the drawing.
fn tab_strip(app: &App, strip: Rect) -> (Line<'static>, StripHits) {
    let current = app.now_tab();
    let active = Style::new().fg(accent()).add_modifier(Modifier::BOLD);
    let rest = Style::new();

    let tabs = app.now_tabs();
    let numbered = |tab: NowTab| {
        let at = tabs.iter().position(|t| *t == tab).unwrap_or(0);
        format!("{}:{}", at + 1, tab.title())
    };

    if tab_strip_width(app) > strip.width {
        let label = numbered(current);
        let back = Rect { x: strip.x, y: strip.y, width: 2, height: 1 };
        let forward =
            Rect { x: strip.x + 2 + width_of(&label) as u16, y: strip.y, width: 2, height: 1 };
        let line = Line::from(vec![
            Span::styled("‹ ", Style::new().fg(dim())),
            Span::styled(label, active),
            Span::styled(" ›", Style::new().fg(dim())),
        ]);
        return (line, StripHits { tabs: Vec::new(), arrows: Some((back, forward)) });
    }

    let mut spans = Vec::new();
    let mut hits = Vec::new();
    let mut x = strip.x;
    for (index, tab) in tabs.iter().copied().enumerate() {
        if !spans.is_empty() {
            spans.push(Span::raw(TAB_GAP));
            x += TAB_GAP.len() as u16;
        }
        // Brackets, not just colour. Colour is the first thing a terminal
        // takes away — NO_COLOR is a standard crossterm honours, and plenty of
        // people are running 16 colours or can't tell two hues apart. Which
        // tab you are on is not something the UI can afford to whisper. The
        // inactive ones are padded to the same width so the strip does not
        // shuffle as the selection moves.
        let (open, close) = if tab == current { ("[", "]") } else { (" ", " ") };
        let style = if tab == current { active } else { rest };
        let label = numbered(tab);
        let width = width_of(&label) as u16 + 2;
        hits.push((index, Rect { x, y: strip.y, width, height: 1 }));
        spans.push(Span::styled(open, style));
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(close, style));
        x += width;
    }
    (Line::from(spans), StripHits { tabs: hits, arrows: None })
}

/// A horizontal rule that closes the column divider above it, rather than
/// running past it and leaving the vertical line dangling into the transport.
fn rule_with_junction(width: u16, at: u16) -> String {
    let width = width as usize;
    let at = at as usize;
    if at >= width {
        return "─".repeat(width);
    }
    format!("{}┴{}", "─".repeat(at), "─".repeat(width - at - 1))
}

/// The tab strip, and whichever tab is open under it. Returns the strip's
/// click targets — each tab, or the arrows — for the GUI's pointer.
fn render_now_panel(frame: &mut Frame, area: Rect, app: &mut App) -> StripHits {
    if area.width < 12 || area.height < 3 {
        return StripHits::default();
    }
    let area = Block::default().padding(Padding::horizontal(1)).inner(area);
    let [strip, rule, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
            .areas(area);

    let (line, hits) = tab_strip(app, strip);
    frame.render_widget(Paragraph::new(line), strip);
    frame.render_widget(
        Paragraph::new(Span::styled("─".repeat(rule.width as usize), Style::new().fg(dim()))),
        rule,
    );

    match app.now_tab() {
        NowTab::Queue => render_now_queue(frame, content, app),
        NowTab::AutoDj => render_now_autodj(frame, content, app),
        NowTab::Lyrics => {
            render_now_placeholder(frame, content, "words go here", "not wired up yet")
        }
        NowTab::Discover => render_now_discover(frame, content, app),
        NowTab::Visualizer => render_now_visualizer(frame, content, app),
    }
    hits
}

/// Whichever visualiser is showing, drawn from the audio itself.
///
/// The mode's name goes on the last row rather than the first: it is the
/// least interesting thing on this panel and it should not cost the picture
/// its top. What changes it is in the footer hint, with the other keys.
fn render_now_visualizer(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 2 {
        return;
    }
    let [picture, label] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);

    // Named whether or not there is anything to draw. Pressing `v` with the
    // player paused should still show that something happened — and the same
    // for `.`, which changes only how the samples are joined and would
    // otherwise be a key with no visible effect on a still picture.
    let mut named = vec![Span::styled(app.viz.mode.title(), Style::new().fg(dim()))];
    if app.viz.mode.plots_samples() {
        let how = if app.viz.scatter { "   dots" } else { "   lines" };
        named.push(Span::styled(how, Style::new().fg(dim())));
    }
    frame.render_widget(Paragraph::new(Line::from(named)), label);

    let Some(tap) = &app.tap else {
        return render_now_placeholder(frame, picture, "the visualiser goes here", "no audio thread");
    };
    // Refilled in place: the copy is the same size every frame, so the
    // buffer lives on the App. A miss is the audio thread holding the lock
    // mid-push, or the ~50 ms after a seek cleared the ring — the frame
    // already in hand carries the picture over those, because blinking
    // into placeholder text for one tick reads as a glitch. The
    // placeholder is only for when there is genuinely nothing behind the
    // panel: no track on, or no audio ever handed over.
    let fresh = tap.frame_into(&mut app.heard);
    if !fresh && (app.now_playing.is_none() || app.heard.samples.is_empty()) {
        return render_now_placeholder(frame, picture, "the visualiser goes here", "nothing playing");
    }

    // Pausing stops the clock, the progress bar and the position. The tap
    // goes on holding the last tenth of a second that was played, so left to
    // itself the picture would carry on settling out of audio that is not
    // being played any more — neither moving nor still. Frozen is what the
    // rest of the screen does.
    let sounding = app.status.playing;
    let mut canvas = crate::tui::canvas::Canvas::new(picture);
    if !canvas.is_empty() {
        // Spelled out rather than hidden behind an App accessor, so the
        // borrow checker can see these fields are not the one `draw`
        // mutates.
        let cover = app
            .now_playing
            .as_ref()
            .and_then(|track| track.metadata.album_art.as_deref())
            .and_then(|file| app.art.get(file))
            .and_then(|art| art.as_ref());
        app.viz.draw(&mut canvas, &app.heard, sounding, cover);
        frame.render_widget(&canvas, picture);
    }
}

/// What the track on the speakers sounds like, as neighbours you can play.
///
/// A list rather than the browser tab's drill: this panel follows what is
/// playing and has one question to ask about it. Picking a different seed,
/// or asking about artists instead, is the Discover *tab*'s job — you go
/// there to look something up; you glance at this while the music plays.
fn render_now_discover(frame: &mut Frame, area: Rect, app: &mut App) {
    let [head, rest] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
    // A list of neighbours with no name on it is a list of strangers.
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                fit("Songs similar to what's playing", area.width as usize),
                Style::new().fg(dim()),
            )),
            Line::raw(""),
        ]),
        head,
    );

    let Some(shown) = app.now_discover.as_ref() else {
        return render_now_placeholder(frame, rest, "", "nothing playing to compare against");
    };
    if shown.pending {
        return render_now_placeholder(frame, rest, "", &format!("{} looking…", spinner_frame(app)));
    }
    if shown.tracks.is_empty() {
        // The server's own words when it has them — an unanalysed track and
        // a library with nothing close are different answers.
        let why = shown.note.clone().unwrap_or_else(|| "nothing close in your library".into());
        return render_now_placeholder(frame, rest, "", &why);
    }

    let width = rest.width as usize;
    // The offset goes back to the app, not into a local: `ListState` is
    // `Copy`, so correcting into one that dies with the frame compiles
    // happily and pins the cursor to the bottom row for ever after. Same
    // rule, and the same reason, as `render_now_queue` below.
    let mut state = ListState::default().with_selected(Some(app.now_scroll));
    *state.offset_mut() = app.now_offset;
    let (window, mut visible) = visible_rows(&mut state, shown.tracks.len(), rest.height);
    app.now_offset = state.offset();
    let playing = app.now_playing.as_ref().map(|t| t.filepath.as_str());

    let items: Vec<ListItem> = shown.tracks[window]
        .iter()
        .map(|near| {
            // The seed's nearest neighbour is sometimes the seed.
            let is_playing = playing == Some(near.filepath.as_str());
            let name = near.clone().into_track().display_name();
            let detail = near.metadata.duration.map(fmt_duration).unwrap_or_default();
            near_row(&name, near.similarity, &detail, width, is_playing)
        })
        .collect();

    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        rest,
        &mut visible,
    );
}

/// One neighbour: how close it is, what it is, and one dim fact on the
/// right.
///
/// The percentage leads because it is what the row is *for* — these arrive
/// in order, so a position in the list says nothing a rank could not, while
/// the number says how much of a neighbour it actually is.
fn near_row(
    name: &str,
    similarity: f64,
    detail: &str,
    width: usize,
    playing: bool,
) -> ListItem<'static> {
    let style = if playing {
        Style::new().fg(accent()).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let near = format!("{:>3.0}%  ", (similarity * 100.0).clamp(0.0, 100.0));
    let detail_width = if detail.is_empty() { 0 } else { width_of(detail) + 1 };
    let name = fit(name, width.saturating_sub(width_of(&near) + detail_width));
    let gap = width
        .saturating_sub(width_of(&near) + width_of(&name) + detail_width.saturating_sub(1))
        .max(1);
    let mut spans = vec![
        Span::styled(near, Style::new().fg(dim())),
        Span::styled(name, style),
    ];
    if !detail.is_empty() {
        spans.push(Span::raw(" ".repeat(gap)));
        spans.push(Span::styled(detail.to_string(), Style::new().fg(dim())));
    }
    ListItem::new(Line::from(spans))
}

/// The cover under the facts: real pixels where the terminal has them, and
/// the half-block mosaic everywhere else — which is not a degraded mode
/// but what every terminal without kitty, sixel or iTerm2 sees, and what
/// the tests and the browser build see always. A cover that is missing, or
/// still on its way, draws nothing at all: the facts above are the
/// information, and a placeholder under them would dress absence up as a
/// fact. Returns the cells the cover took.
fn render_facts_cover(frame: &mut Frame, area: Rect, app: &mut App, mosaic: bool) -> Option<Rect> {
    // Spelled out field by field for the reason the visualiser spells its
    // cover out: the borrow checker has to see that the art cache and the
    // fields below are different fields, because those are taken mutably
    // while the cache's borrow is still held.
    let cover = app
        .now_playing
        .as_ref()
        .and_then(|track| track.metadata.album_art.as_deref())
        .and_then(|file| app.art.get(file))
        .and_then(|art| art.as_ref())?;

    // Near-square, pinned to the top. A cell is about twice as tall as it
    // is wide, so width/2 rows is a square box; handed the whole leftover
    // instead, the pixel path's fit would centre the picture vertically —
    // a cover floating mid-column, attached to nothing.
    let height = area.height.min(area.width / 2);
    if height < 3 {
        return None;
    }
    let area = Rect { height, ..area };

    // The mosaic on request: a caller whose overlay stood over these cells
    // last frame needs plain cells there, not a picture's skipped ones.
    if !mosaic && app.graphics.draw(frame, area, cover) {
        return Some(area);
    }

    let mut canvas = crate::tui::canvas::Canvas::new(area);
    if !canvas.is_empty() {
        app.cover_pane.draw(&mut canvas, cover);
        frame.render_widget(&canvas, area);
    }
    Some(area)
}

fn render_now_placeholder(frame: &mut Frame, area: Rect, what: &str, why: &str) {
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(what.to_string(), Style::new().fg(dim()))),
        Line::from(Span::styled(why.to_string(), Style::new().fg(dim()))),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// The queue as the full-screen view draws it. It shares the browser screen's
/// selection rather than keeping its own, so `d` removes the row under the
/// cursor whichever screen you are looking at.
///
/// The scroll correction lands in `app.queue.state`, where `visible_rows`
/// writes it before the slice is drawn. `ListState` is `Copy`, so a version
/// that corrects into a local compiles happily — and throws the offset away
/// with every frame, pinning the selection to the bottom edge while the
/// list slides underneath it.
fn render_now_queue(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.queue.items.is_empty() {
        render_now_placeholder(frame, area, "nothing queued", "0 back, then 'a' on a track");
        return;
    }

    let width = area.width as usize;
    let (window, mut shown) =
        visible_rows(&mut app.queue.state, app.queue.items.len(), area.height);
    let current = app.queue.current;
    let items: Vec<ListItem> = app.queue.items[window.clone()]
        .iter()
        .enumerate()
        .map(|(row, track)| {
            let playing = Some(window.start + row) == current;
            let style = if playing { Style::new().fg(accent()) } else { Style::new() };
            let marker = if playing { format!("{} ", glyphs().playing) } else { "  ".to_string() };
            let name = format!("{marker}{}", track.display_name());
            let Some(duration) = track.metadata.duration else {
                return ListItem::new(Line::from(Span::styled(name, style)));
            };
            let time = fmt_duration(duration);
            let time_width = width_of(&time);
            let name = fit(&name, width.saturating_sub(time_width + 1));
            let gap = width.saturating_sub(width_of(&name) + time_width).max(1);
            ListItem::new(Line::from(vec![
                Span::styled(name, style),
                Span::raw(" ".repeat(gap)),
                Span::styled(time, Style::new().fg(dim())),
            ]))
        })
        .collect();

    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
        &mut shown,
    );
}

/// The left-hand column: what is playing, then what the tags know about it as
/// a labelled ladder. Pure so the shape can be asserted without a terminal.
///
/// A ladder rather than the one-line run of separators the compact transport
/// uses — there is room here, and a column of labels can be read for one value
/// where `2:54 · 85 BPM · 10B · ★★★★` has to be read whole.
/// "FLAC · 1006 kbps". The container and how much of it there is per second,
/// which together are what people mean when they ask what a file is.
fn file_format(meta: &TrackMetadata) -> Option<String> {
    let format = meta.format.as_deref().map(str::trim).filter(|f| !f.is_empty());
    // Rounded to whole kbps: the server counts in bits per second, and nobody
    // reads a rip as 320.0 kbps rather than 320.
    let kbps = meta.kbps();
    match (format, kbps) {
        (Some(format), Some(kbps)) => Some(format!("{}   {kbps} kbps", format.to_uppercase())),
        (Some(format), None) => Some(format.to_uppercase()),
        (None, Some(kbps)) => Some(format!("{kbps} kbps")),
        (None, None) => None,
    }
}

/// "44.1 kHz · 24-bit · stereo". Bit depth only exists for lossless, so its
/// absence says something too and it is left out rather than guessed at.
fn audio_shape(meta: &TrackMetadata) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(khz) = meta.khz_words() {
        parts.push(format!("{khz} kHz"));
    }
    if let Some(depth) = meta.bit_depth.filter(|d| *d > 0) {
        parts.push(format!("{depth}-bit"));
    }
    match meta.channels {
        Some(1) => parts.push("mono".to_string()),
        Some(2) => parts.push("stereo".to_string()),
        Some(n) if n > 2 => parts.push(format!("{n} channels")),
        _ => {}
    }
    (!parts.is_empty()).then(|| parts.join("   "))
}

fn now_playing_card(app: &App, width: usize) -> Vec<Line<'static>> {
    let Some(track) = &app.now_playing else {
        return vec![
            Line::raw(""),
            Line::from(Span::styled(
                fit("nothing playing", width),
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::from(Span::styled(
                fit("0 goes back to the library", width),
                Style::new().fg(dim()),
            )),
        ];
    };

    let meta = &track.metadata;
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            fit(track.title_or_file(), width),
            Style::new().fg(accent()).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            fit(meta.artist.as_deref().unwrap_or("unknown artist"), width),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    let faint = Style::new().fg(dim());
    let value_width = width.saturating_sub(8);
    let plain = |value: String| vec![Span::raw(fit(&value, value_width))];

    if let Some(album) = meta.album.as_deref().filter(|a| !a.is_empty()) {
        fact_row(&mut lines, "Album", plain(album.to_string()));
    }
    // Where this sits on the release. The total is what makes it worth
    // saying: "3" alone is a number, "3 of 12" is a position.
    if let Some(track) = meta.track {
        let mut value = match meta.track_total {
            Some(total) if total >= track => format!("{track} of {total}"),
            _ => track.to_string(),
        };
        if let Some(disc) = meta.disk.filter(|_| meta.disc_total.is_some_and(|t| t > 1)) {
            value.push_str(&format!("   disc {disc}"));
        }
        fact_row(&mut lines, "Track", plain(value));
    }
    if let Some(year) = meta.year {
        fact_row(&mut lines, "Year", plain(year.to_string()));
    }
    let genres: Vec<&str> =
        meta.genres.iter().map(|g| g.trim()).filter(|g| !g.is_empty()).collect();
    if !genres.is_empty() {
        fact_row(&mut lines, "Genre", plain(genres.join(", ")));
    }
    if let Some(bpm) = meta.bpm {
        fact_row(&mut lines, "Tempo", plain(format!("{bpm} BPM")));
    }
    if let Some(key) = meta.musical_key.as_deref().filter(|k| !k.is_empty()) {
        // The Camelot code is what Auto-DJ matches on, so it gets the accent
        // and the tag's own spelling follows it, quietly.
        let value = match crate::dj::to_camelot(key) {
            Some(camelot) => vec![
                Span::styled(camelot.code().to_string(), Style::new().fg(accent())),
                Span::styled(format!("  {key}"), faint),
            ],
            None => plain(key.to_string()),
        };
        fact_row(&mut lines, "Key", value);
    }
    if let Some(rating) = meta.rating.filter(|r| *r > 0) {
        fact_row(&mut lines, "Rating", plain(format!("{rating}/10")));
    }
    if let Some(plays) = meta.play_count.filter(|p| *p > 0) {
        fact_row(&mut lines, "Plays", plain(plays.to_string()));
    }
    if let Some(format) = file_format(meta) {
        fact_row(&mut lines, "Format", plain(format));
    }
    if let Some(audio) = audio_shape(meta) {
        fact_row(&mut lines, "Audio", plain(audio));
    }
    if let Some(size) = meta.file_size.filter(|b| *b > 0) {
        fact_row(&mut lines, "Size", plain(crate::api::types::fmt_bytes(size)));
    }

    lines.push(Line::raw(""));
    if !app.audio_available {
        lines.push(Line::from(Span::styled(
            "audio device unavailable",
            Style::new().fg(Color::Red),
        )));
        return lines;
    }
    let (glyph, word) = transport_state(app);
    lines.push(Line::from(vec![
        Span::styled(glyph, Style::new().fg(accent())),
        Span::raw(format!(" {word}")),
    ]));
    lines
}

/// One labelled row of the facts column. Takes spans rather than a string so
/// a value can carry its own emphasis -- the Camelot key does.
fn fact_row(lines: &mut Vec<Line<'static>>, label: &str, value: Vec<Span<'static>>) {
    let mut spans = vec![Span::styled(format!("{label:<8}"), Style::new().fg(dim()))];
    spans.extend(value);
    lines.push(Line::from(spans));
}

/// The glyph and the word for what playback is doing.
///
/// "starting" is a state of its own rather than a shade of stopped. Between
/// asking for a track and the engine reporting it there is a real gap — a tick
/// at best, and as long as opening a remote source takes at worst — and
/// calling that stopped printed the word under the name of the track that was
/// about to play. The spinner is the one already used for a pane waiting on a
/// reply, which is the same thing happening.
fn transport_state(app: &App) -> (&'static str, &'static str) {
    if app.is_starting() {
        (spinner_frame(app), "starting")
    } else if app.status.paused {
        (glyphs().paused, "paused")
    } else if app.status.playing {
        (glyphs().playing, "playing")
    } else {
        ("■", "stopped")
    }
}

/// The progress bar, drawn rather than handed to ratatui's `Gauge`.
///
/// The Gauge paints its remainder by swapping the style's foreground and
/// background, so with only a foreground set the unfilled part comes out as
/// bare terminal background and the bar appears to fade into nothing. Giving
/// it a background instead paints a solid slab heavier than the bar itself.
/// Blocks and a lighter shade for the track say the same thing more quietly,
/// and put the time beside the bar where there is room for it to be read.
/// The time as the progress line writes it, and how many columns the bar
/// gets beside it. `None` when there is not enough room for a bar at all.
fn progress_parts(app: &App, width: usize) -> (String, String, Option<usize>) {
    let position = fmt_duration(app.status.position);
    let total =
        if app.status.duration > 0.0 { fmt_duration(app.status.duration) } else { "--:--".into() };
    let bar = width.saturating_sub(width_of(&format!("{position} / {total}")) + 3);
    (position, total, (bar >= 4).then_some(bar))
}

/// Where along the track a click at `column` landed, if it landed on the bar.
///
/// `None` for a click on the time beside it, for a track whose length is not
/// known — seeking into an unknown duration means nothing — and for a bar too
/// narrow to have been drawn.
pub(crate) fn seek_target(app: &App, width: u16, column: u16) -> Option<f64> {
    let (_, _, bar) = progress_parts(app, width as usize);
    let bar = bar?;
    if app.status.duration <= 0.0 || usize::from(column) >= bar {
        return None;
    }
    // The left edge of a column is the moment it starts, so clicking the
    // first column seeks to the beginning rather than to half a column in.
    Some(f64::from(column) / bar as f64 * app.status.duration)
}

/// `hovered` is where the pointer is along the bar, when it is on it.
///
/// A terminal will not change the mouse pointer for us — there is an escape
/// code for it, OSC 22, but the Windows console documents no such sequence
/// and it is the console this runs in. So the affordance goes the other way:
/// the bar lights under the pointer, and shows where a click would land. That
/// works in any terminal that reports the mouse at all, which is the same set
/// that could click it in the first place.
/// Where along `bar` the pointer is, if it is on it at all.
fn hovering(app: &App, bar: Rect) -> Option<u16> {
    app.pointer.filter(|at| bar.contains(*at)).map(|at| at.x - bar.x)
}

/// Squash the server's 800 bars down to the columns actually on screen, and
/// scale the result to fill the height available.
///
/// **Root-mean-square, not peak** — measured, not assumed. The server's bars
/// are already peaks, and taking the peak again over the ~10 that share a
/// column asks "was anything loud in these three seconds?". On a modern
/// master the answer is yes everywhere: against demo.mstream.io that drew 83
/// of 86 columns at full height, a brick with a fade at each end. RMS asks
/// how much energy was in those three seconds, which is the question whose
/// answer is the shape of the song.
///
/// Then **normalised to the track's own loudest column**. Nothing here
/// applies ReplayGain, so levels between tracks vary by more than the eight
/// heights a row can draw; without this a quietly-mastered record is a flat
/// line. The cost is honest and worth naming: this shows a track's shape,
/// not its level, and two tracks drawn side by side say nothing about which
/// is louder.
fn resample_bars(bars: &[u8], columns: usize) -> Vec<u8> {
    if columns == 0 || bars.is_empty() {
        return Vec::new();
    }
    let energy: Vec<f64> = (0..columns)
        .map(|column| {
            let start = column * bars.len() / columns;
            let end = ((column + 1) * bars.len() / columns).max(start + 1).min(bars.len());
            let window = &bars[start..end];
            let sum: f64 = window.iter().map(|b| f64::from(*b).powi(2)).sum();
            (sum / window.len() as f64).sqrt()
        })
        .collect();

    // Stretched onto the band this track actually uses, rather than onto
    // 0..255. Measured against demo.mstream.io: after RMS, a densely
    // mastered track's body sits between 230 and 250, and eight heights
    // cannot show a twenty-wide band — it drew as a brick with a fade at
    // each end. Mapping [quiet, loudest] onto the full height instead spends
    // all eight glyphs on the range that varies, which is what makes a
    // breakdown visible enough to seek to.
    //
    // The floor is a low percentile rather than the minimum, because nearly
    // every track fades in and out: one near-silent column at each end would
    // otherwise anchor the bottom at zero and undo the whole stretch. On a
    // genuinely dynamic recording the band is already wide and this changes
    // almost nothing.
    let loudest = energy.iter().copied().fold(0.0_f64, f64::max);
    if loudest <= 0.0 {
        return vec![0; columns];
    }
    let mut sorted: Vec<f64> = energy.clone();
    sorted.sort_by(f64::total_cmp);
    let quiet = sorted[sorted.len() / 10];
    // A track with no variation to stretch — a test tone, or a bar one
    // column wide — has `quiet == loudest`, and stretching it would map its
    // only height to nothing. Show it at its own level instead.
    let (floor, span) = match loudest - quiet > f64::EPSILON {
        true => (quiet, loudest - quiet),
        false => (0.0, loudest),
    };
    energy.iter().map(|e| (((e - floor) / span * 255.0).round().clamp(0.0, 255.0)) as u8).collect()
}

/// A column's amplitude as eighths of one cell.
///
/// Over 255, not 256: the loudest column of a track *is* 255, and dividing
/// by the count of values rather than the largest one put it at seven
/// eighths — the full block unreachable, the clamp below it dead, and every
/// waveform drawn a notch short of the ceiling it was scaled to.
fn eighths(amplitude: u8) -> usize {
    (usize::from(amplitude) * 8 / 255).min(8)
}

/// The half of the shape *above* the centre line, in the full-screen view.
///
/// The scrubber row below is the other half, which is why [`WAVE_HALF_ROWS`]
/// is one and not a knob: the two have to match, and the lower one is the
/// control itself. A taller band means redesigning that row too.
///
/// This half needs no trick — a waveform grows outward from the centre, and
/// the centre is the *bottom* of this row, which is exactly where a lower
/// block starts. It is the half below that has to hang (see
/// [`progress_line`]'s `hangs`).
fn waveform_top_line(app: &App, width: usize) -> Line<'static> {
    if !glyphs().mirrored {
        return Line::raw("");
    }
    let (_, _, bar_width) = progress_parts(app, width);
    let Some(bar_width) = bar_width else { return Line::raw("") };
    let Some(wave) = playing_waveform(app, bar_width) else {
        // With a track on but no shape for it, the scrubber below is an
        // ordinary progress bar and this half has nothing to add.
        if app.now_playing.is_some() {
            return Line::raw("");
        }
        // With nothing playing at all, the band is a flat line through its
        // own middle — which is what a silent waveform looks like. An empty
        // half over a full one reads as neither silence nor progress.
        return Line::from(Span::styled(
            glyphs().eighths[1].repeat(bar_width),
            Style::new().fg(dim()),
        ));
    };

    let filled = (app.status.progress() * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);

    let spans: Vec<Span<'static>> = (0..bar_width)
        .map(|column| {
            // Matched to the scrubber's `.max(1)`, so a quiet passage is a
            // thin line through the middle rather than a gap in the band.
            let height = eighths(wave.get(column).copied().unwrap_or(0)).max(1);
            let colour = if column < filled { accent() } else { dim() };
            Span::styled(glyphs().eighths[height], Style::new().fg(colour))
        })
        .collect();
    Line::from(spans)
}

/// The shape of what is playing, resampled to `columns`, when there is one.
///
/// Absent for every ordinary reason — a server with no ffmpeg, a track the
/// scan has not reached, a federated track, or simply a request still out —
/// and every one of them draws the plain bar instead. It is a decoration on
/// a control that works without it.
fn playing_waveform(app: &App, columns: usize) -> Option<Vec<u8>> {
    let filepath = &app.now_playing.as_ref()?.filepath;
    let bars = app.waveforms.get(filepath)?.as_ref()?;
    Some(resample_bars(bars, columns))
}

/// The scrubber: the shape, the playhead, the hover marker and the time.
///
/// `hangs` is what makes the full-screen band a mirror rather than two bar
/// charts. On its own — the compact transport — the shape grows up from the
/// bottom of the row, which is what a single-row waveform should do. As the
/// lower half of a pair it has to grow *down from the centre*, so it is
/// drawn `REVERSED`: that swaps the two colours at render time and turns
/// "bottom `n` eighths inked" into "top `n` inked", without this code ever
/// needing to know the terminal's background. Unicode has no eighth-
/// resolution blocks that hang, which is why the trick is needed at all.
fn progress_line(app: &App, width: usize, hovered: Option<u16>, hangs: bool) -> Line<'static> {
    let (position, total, bar_width) = progress_parts(app, width);
    let Some(bar_width) = bar_width else {
        let time = format!("{position} / {total}");
        return Line::from(Span::styled(fit(&time, width), Style::new().fg(dim())));
    };

    let filled = (app.status.progress() * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    // Only where a click would do something: hovering a track whose length we
    // do not know should not promise a seek we will refuse.
    let at = hovered
        .filter(|_| app.status.duration > 0.0)
        .map(usize::from)
        .filter(|at| *at < bar_width);

    // The unplayed part brightens under the pointer, so the bar reads as a
    // control rather than a readout, and one column carries a marker for
    // where a click would land. Built a column at a time and then run
    // together — splicing the marker into three or five segments took more
    // arithmetic than it saved and got the edges wrong. (With a waveform the
    // runs mostly stop merging, since neighbouring columns rarely share a
    // height. That costs a span per column on one line, which is nothing;
    // the merging still earns its keep on the plain bar underneath.)
    let rest = if at.is_some() { folder() } else { dim() };
    let played = Style::new().fg(accent());
    let unplayed = Style::new().fg(rest);
    let marker = Style::new().fg(Color::White).add_modifier(Modifier::BOLD);

    // The shape of the track, when the server has one. Only the glyph
    // changes: what is behind the playhead, what is ahead of it and where a
    // click would land are the same three channels either way, so the bar
    // goes on being the same control whether or not this arrived.
    let wave = playing_waveform(app, bar_width);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run: Option<(&'static str, Style, usize)> = None;
    for column in 0..bar_width {
        let (glyph, style) = if Some(column) == at {
            (glyphs().caret, marker)
        } else if let Some(peak) = wave.as_ref().and_then(|w| w.get(column).copied()) {
            // 0..=255 onto the eight heights. Never empty: a hole in the
            // middle of a waveform reads as a rendering fault, where a low
            // column reads as a quiet passage.
            let height = eighths(peak).max(1);
            let style = if column < filled { played } else { unplayed };
            match hangs {
                // `8 - height` because REVERSED inks the part the glyph
                // leaves blank.
                true => (glyphs().eighths[8 - height], style.add_modifier(Modifier::REVERSED)),
                false => (glyphs().eighths[height], style),
            }
        } else if hangs && app.now_playing.is_none() {
            // The other half of the silent line the upper row is drawing.
            // Only in the mirrored band: a single-row bar with nothing
            // playing is a track nobody has started, and the flat track it
            // has always drawn says that better than a hairline would.
            (glyphs().eighths[7], unplayed.add_modifier(Modifier::REVERSED))
        } else if column < filled {
            ("\u{2588}", played)
        } else {
            ("\u{2591}", unplayed)
        };
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

    spans.push(Span::raw("  "));
    spans.push(Span::styled(position, Style::new().fg(accent())));
    spans.push(Span::styled(format!(" / {total}"), Style::new().fg(dim())));
    Line::from(spans)
}

/// The modes worth a reader's attention: the ones that are on.
///
/// Spelling out `repeat off  shuffle off  dj off` spends most of the line
/// saying nothing happened, and it is the state nearly all of the time. What
/// is left goes to the message, which is the half of the footer that changes.
/// Volume stays whatever it reads, because it is the one that gets nudged.
fn mode_readout(app: &App, compact: bool) -> String {
    let volume = (app.volume * 100.0).round() as u32;
    let mut parts =
        vec![if compact { format!("{volume:>3}%") } else { format!("vol {volume:>3}%") }];
    if app.queue.repeat != Repeat::Off {
        let label = app.queue.repeat.label();
        parts.push(if compact { format!("rpt {label}") } else { format!("repeat {label}") });
    }
    if app.queue.shuffle {
        parts.push(if compact { "shf".to_string() } else { "shuffle".to_string() });
    }
    if app.dj_armed() {
        // Where it is picking from, when that is not the room you are in.
        let name = app.dj_server_name();
        let here = app.session_identity().is_some_and(|s| crate::config::same_server(&s, app.dj_server.as_deref().unwrap_or_default()));
        parts.push(if compact || here { "dj on".to_string() } else { format!("dj on · {name}") });
    }
    parts.join("  ")
}

/// Width in terminal columns, which is not the character count once anything
/// wide or combining turns up in a tag.
fn width_of(text: &str) -> usize {
    Span::raw(text).width()
}

fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    Span::raw(&*c.encode_utf8(&mut buf)).width()
}

/// Shorten to `width` columns, marking the cut so it reads as elided rather
/// than broken off mid-word.
fn fit(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = char_width(c);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// The Auto-DJ tab: what the picker is being told to do, and what that
/// actually produces.
///
/// The player's one settings surface for Auto-DJ. It was a modal behind `D`
/// as well as a read-only summary here, which meant two screens describing
/// one set of values — and the modal was never drawn over this view at all,
/// so pressing `D` in the full-screen player opened something invisible that
/// swallowed the keyboard.
fn render_now_autodj(frame: &mut Frame, area: Rect, app: &App) {
    let panel = &app.dj_panel;
    let mut lines: Vec<Line> = Vec::new();
    // Widest label plus a gutter, so the values form a column.
    let label_width = panel.rows.iter().map(|r| r.label().len()).max().unwrap_or(0) + 2;

    for (index, row) in panel.rows.iter().enumerate() {
        let focused = index == panel.row;
        let marker = if focused { "> " } else { "  " };
        let label_style = if focused {
            Style::new().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let mut spans = vec![
            Span::styled(marker, label_style),
            Span::styled(format!("{:<label_width$}", row.label()), label_style),
        ];
        spans.extend(dj_value_spans(*row, app));
        lines.push(Line::from(spans));
    }

    lines.extend(dj_sample_lines(app));
    frame.render_widget(Paragraph::new(lines), area);
}

/// The right-hand side of a settings row: the value, plus whatever context
/// makes it meaningful.
fn dj_value_spans(row: DjRow, app: &App) -> Vec<Span<'static>> {
    let faint = Style::new().fg(dim());
    let value = |text: String| Span::raw(text);
    // Two cells of air, not three: the tab is a half-width column and the
    // strictness row's cosine was losing its last digit at 100 columns.
    let note = |text: String| Span::styled(format!("  {text}"), faint);
    let switch = |on: bool| value(if on { "on".to_string() } else { "off".to_string() });
    let clock = |seconds: u32| crate::api::types::fmt_duration(f64::from(seconds));
    let s = &app.dj;
    match row {
        DjRow::Armed => {
            if app.dj_armed() {
                vec![value("on".into()), note(format!("picking from {}", app.dj_server_name()))]
            } else {
                // Where Enter starts it: here — or, in the shell that has a
                // Play from (the GUI hosts this tab on its Now Playing
                // screen), on its home when that is another server.
                let here = app.session_identity();
                let elsewhere = app
                    .dj_play_from()
                    .filter(|home| here.as_deref().is_none_or(|here| !crate::config::same_server(home, here)));
                let hint = match elsewhere {
                    Some(home) => format!("Enter starts it on {}", app.server_name_of(&home)),
                    None => "Enter starts it here".to_string(),
                };
                vec![value("off".into()), note(hint)]
            }
        }
        DjRow::SongsPerFetch => {
            vec![value(s.songs_per_fetch.to_string()), note("songs each turn".into())]
        }
        DjRow::Sonic => {
            let mut spans = vec![switch(s.sonic)];
            if let Some(reason) = app.dj_sonic_reason() {
                spans.push(note(reason));
            }
            spans
        }
        DjRow::Strictness => {
            let band = crate::dj::SONIC_MAX_SIMILARITY - crate::dj::SONIC_MIN_SIMILARITY;
            let filled = (((s.sonic_min_similarity - crate::dj::SONIC_MIN_SIMILARITY) / band) * 10.0)
                .round()
                .clamp(0.0, 10.0) as usize;
            let bar: String = "▓".repeat(filled) + &"░".repeat(10 - filled);
            let pct = (s.sonic_min_similarity * 100.0).round() as u32;
            let mut spans = vec![
                value(format!("{bar} {pct}%")),
                // The raw number is what the server actually filters on, and
                // seeing it is how the bar stops being a mystery.
                note(format!("cosine ≥ {:.2}", s.sonic_min_similarity)),
            ];
            if let Some(pool) = &app.dj_panel.pool {
                spans.push(Span::styled(format!(" · {} in the pool", pool.pool_size), faint));
            }
            spans
        }
        DjRow::Anchor => {
            let (label, what) = match s.sonic_anchor {
                crate::dj::SonicAnchor::Rolling => {
                    ("Follow the vibe", "each pick follows the session's recent sound")
                }
                // Stay close freezes the seed lists as they stand — the last
                // picks, the pins — or holds to one song when they are
                // empty (auto-dj contract, clause 23).
                crate::dj::SonicAnchor::Locked => {
                    ("Stay on seed", "picks stay close to the seeds as they stand")
                }
            };
            vec![value(label.into()), note(what.into())]
        }
        DjRow::EmptyQueue => {
            let label = match s.empty_queue {
                crate::dj::EmptyQueueStart::Ask => "Ask",
                crate::dj::EmptyQueueStart::Random => "Surprise me",
                crate::dj::EmptyQueueStart::Pick => "Let me choose",
                crate::dj::EmptyQueueStart::Recent => "Recently played",
            };
            vec![value(label.into()), note("when switched on with nothing queued".into())]
        }
        DjRow::Bpm => vec![switch(s.bpm)],
        DjRow::Tolerance => vec![
            value(format!("± {} BPM", s.bpm_tolerance)),
            note(format!("wide set ± {}", s.bpm_tolerance + crate::dj::BPM_WIDE_EXTRA)),
        ],
        DjRow::Harmonic => {
            let mut spans = vec![switch(s.harmonic)];
            match &app.lane.camelot_anchor {
                Some(anchor) if s.harmonic => spans.push(note(format!("anchored on {anchor}"))),
                _ if s.harmonic => spans.push(note("the first keyed pick sets the anchor".into())),
                _ => {}
            }
            spans
        }
        DjRow::Cooldown => {
            if s.artist_cooldown == 0 {
                return vec![value("off".into())];
            }
            vec![value(format!("{} artists", s.artist_cooldown)), note("recently played, skipped".into())]
        }
        // The library rows read the target server's own rules (clause 51).
        DjRow::Rating => {
            let library = app.dj_library();
            if library.min_rating == 0 {
                return vec![value("Any".into())];
            }
            vec![value(format!("{:.1} ★ or above", f64::from(library.min_rating) / 2.0))]
        }
        DjRow::Length => vec![switch(s.length), note(s.length_words())],
        DjRow::Shortest => {
            if s.min_seconds == 0 {
                return vec![value("any".into())];
            }
            vec![value(clock(s.min_seconds))]
        }
        DjRow::Longest => {
            if s.max_seconds >= crate::dj::LENGTH_RAIL_SECONDS {
                return vec![value("any".into())];
            }
            vec![value(clock(s.max_seconds))]
        }
        DjRow::UnknownLength => {
            vec![switch(s.allow_unknown_length), note("include tracks with no length read".into())]
        }
        DjRow::Genres => {
            let library = app.dj_library();
            if !library.genre_filter {
                return vec![value("off".to_string()), note("Enter to choose".into())];
            }
            let mode = library.genre_mode.label().to_string();
            let chosen = if library.genres.is_empty() {
                "none chosen — Enter to pick".to_string()
            } else {
                library.genres.join(", ")
            };
            let mut spans = vec![value(format!("{mode}  ")), Span::styled(chosen, faint)];
            // The asymmetry bites people: "only these" is a stricter promise
            // than "anything but these", and it drops untagged tracks.
            if library.genre_mode == crate::dj::GenreMode::Whitelist && !library.genres.is_empty() {
                spans.push(Span::styled("  (untagged excluded)", faint));
            }
            spans
        }
        DjRow::Keywords => {
            if let Some(text) = &app.dj_keyword {
                return vec![value(format!("{text}▏")), note("Enter adds the word · Esc done".into())];
            }
            let mut spans = vec![switch(s.keyword_filter)];
            if s.keywords.is_empty() {
                spans.push(note("no keywords · Enter types one".into()));
            } else {
                // The Now Playing screen's remove key (`gui::now`) takes
                // the last keyword on this row.
                spans.push(note(format!("{}: {} · d removes the last", s.keywords.len(), s.keywords.join(", "))));
            }
            spans
        }
        DjRow::Sources => {
            let total = app.dj_info.as_ref().map(|i| i.libraries.len()).unwrap_or(0);
            let off = app.dj_sources_off().len();
            vec![value(format!("{} of {total}", total.saturating_sub(off))), note("Enter to choose".into())]
        }
        // The row's own value stays short — this panel is a column beside
        // the facts, not a full-width modal any more. What the sample
        // actually picked goes underneath, where a title has room.
        DjRow::Sample => {
            let panel = &app.dj_panel;
            if panel.sample_pending {
                return vec![Span::styled("picking…", faint)];
            }
            match panel.sample.len() {
                0 => vec![Span::styled("Enter to preview", faint)],
                n => vec![Span::raw(format!("{n} picks"))],
            }
        }
    }
}

/// The sample itself: how big a pool these settings leave, and what came
/// out of it, under the rows that produced them.
fn dj_sample_lines(app: &App) -> Vec<Line<'static>> {
    let panel = &app.dj_panel;
    let faint = Style::new().fg(dim());
    let mut lines = Vec::new();
    // The pool size is the number that makes the tightness slider tunable,
    // so it survives the sample it arrived with.
    if let Some(pool) = &panel.pool {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("  {} tracks in the sonic pool", pool.pool_size),
            faint,
        )));
    }
    if panel.sample.is_empty() {
        return lines;
    }
    if lines.is_empty() {
        lines.push(Line::raw(""));
    }
    for (index, track) in panel.sample.iter().enumerate() {
        lines.push(Line::from(Span::styled(
            format!("  {}. {}", index + 1, track.display_name()),
            faint,
        )));
    }
    lines
}

/// Whether a picker row is switched on, however the picker decides that.
type Chosen<'a> = Box<dyn Fn(&str) -> bool + 'a>;

/// The list to toggle through — the genres, or the DJ server's libraries —
/// centred over `area`. Returns the box it drew, for a shell that lends it
/// (the GUI's Now Playing screen) to register over what it covers.
pub(crate) fn render_dj_picker(frame: &mut Frame, area: Rect, app: &App) -> Option<Rect> {
    let sources_off = app.dj_sources_off();
    let (picker, title, empty, chosen): (&crate::tui::app::GenrePicker, String, &str, Chosen<'_>) =
        if let Some(picker) = app.dj_panel.sources.as_ref() {
        (
            picker,
            format!(" Sources · {} ", app.dj_server_name()),
            "  no libraries to choose from",
            Box::new(move |name: &str| !sources_off.iter().any(|s| s == name)),
        )
    } else if let Some(picker) = app.dj_panel.genres.as_ref() {
        // The DJ's library's rule, not the session-wide fallback: a server
        // entry with rules of its own keeps its chosen genres there.
        let library = app.dj_library();
        let title = format!(" Genres · {} ", if library.genre_filter { library.genre_mode.label() } else { "off" });
        let genres = library.genres;
        (picker, title, "  no genres tagged", Box::new(move |name: &str| genres.iter().any(|g| g == name)))
    } else {
        return None;
    };

    let mut lines: Vec<Line> = Vec::new();
    if picker.loading {
        lines.push(Line::from(Span::styled("  loading genres…", Style::new().fg(dim()))));
    } else if let Some(err) = &picker.failed {
        lines.push(Line::from(Span::styled(format!("  could not load genres — {err}"), Style::new().fg(dim()))));
    } else if picker.all.is_empty() {
        lines.push(Line::from(Span::styled(empty, Style::new().fg(dim()))));
    } else {
        // Keep the highlighted row on screen for long lists.
        let visible = (area.height.saturating_sub(8)) as usize;
        let first = picker.row.saturating_sub(visible.saturating_sub(1));
        for (index, name) in picker.all.iter().enumerate().skip(first).take(visible) {
            let on = chosen(name);
            let focused = index == picker.row;
            let style = if focused {
                Style::new().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            lines.push(Line::from(Span::styled(
                format!("{}[{}] {name}", if focused { "> " } else { "  " }, if on { 'x' } else { ' ' }),
                style,
            )));
        }
    }
    lines.push(Line::raw(""));
    // Through the locale table: the GUI lends this picker on its Now
    // Playing screen, where the rest of the words are the user's language;
    // the TUI boots no language, so here it stays English.
    lines.push(Line::from(Span::styled(
        format!("  {}", t!("gui.dj.picker_keys")),
        Style::new().fg(dim()),
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(50, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(title);
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
    Some(box_area)
}

fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    // In u32: 937 columns times 70 already passes what a u16 holds, which is
    // a panic in debug and a wrapped-round width in release.
    let width = (u32::from(area.width) * u32::from(width_percent) / 100) as u16;
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{Track, TrackMetadata, fmt_bytes};
    use crate::tui::app::Action;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// English words under assertion: the locale lock, and English pinned —
    /// the locale is process-wide, and the tests that switch it hold the
    /// lock while they draw in another language.
    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    /// The view alone on a frame, as these tests draw it: under the row a
    /// shell's top bar takes, and with the Auto-DJ tab's picker over it when
    /// one is open.
    fn render(frame: &mut Frame, app: &mut App) {
        let area = frame.area();
        app.spinner_shown.set(false);
        frame.render_widget(Clear, area);
        render_now_view(frame, &now_regions(area), app, &NowExtras::default());
        render_dj_picker(frame, area, app);
        crate::tui::graphics::verify_sent(frame.buffer_mut());
    }

    /// Render one frame and flatten the buffer to text for assertions.
    fn draw(app: &mut App) -> String {
        draw_sized(app, 90, 26)
    }

    fn draw_sized(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// A session against a fully-featured server. Capabilities are set
    /// explicitly because they decide which rows the Auto-DJ panel draws; a
    /// default (empty) set would quietly be testing the degraded layout.
    fn connected_app() -> App {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
        app.capabilities = crate::api::types::Capabilities {
            discovery: true,
            discovery_path: true,
            discovery_p2p: false,
            federation_discovery: false,
            federation_browse: false,
            federation_direct: false,
            stats: true,
            discovery_taste: 0,
            discovery_p2p_peers: 0,
            discovery_plugins: false,
            uploads: false,
            mood_filters: false,
        };
        // What a real ping does on the way in: the Auto-DJ rows depend on it.
        app.dj_panel.rebuild(&app.dj, None, false);
        app
    }

    #[test]
    fn a_missed_tap_tick_keeps_the_picture_instead_of_blinking_text() {
        let mut app = connected_app();
        app.fullscreen = true;
        app.now_tab = NowTab::Visualizer;
        app.now_playing = Some(tagged_track());
        // An empty tap misses, which is exactly what a read that lands
        // mid-push gets. With a track playing and a frame already in hand,
        // that tick must not turn into placeholder text.
        app.tap = Some(crate::engine::tap::AudioTap::new());
        app.heard = crate::engine::tap::TapFrame {
            samples: vec![0.5; 2048],
            rate: 44100,
            channels: 1,
        };
        let screen = draw(&mut app);
        assert!(
            !screen.contains("nothing playing"),
            "the held frame should carry the picture over the gap"
        );

        // With nothing playing behind it, the placeholder is the truth —
        // the held frame is history, not a picture worth keeping up.
        app.now_playing = None;
        let screen = draw(&mut app);
        assert!(screen.contains("nothing playing"), "idle should still say so");
    }

    #[test]
    fn a_span_is_given_in_the_coarsest_unit_that_still_says_something() {
        assert_eq!(fmt_span(45.0), "45s");
        assert_eq!(fmt_span(150.0), "2m");
        assert_eq!(fmt_span(3600.0), "1h 00m");
        assert_eq!(fmt_span(8130.0), "2h 15m");
        assert_eq!(fmt_span(0.0), "0m");
    }

    #[test]
    fn the_gap_before_a_track_starts_reads_as_starting_not_stopped() {
        let mut app = connected_app();
        app.replace_queue(vec![Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata {
                title: Some("Moonlight".into()),
                artist: Some("Trio".into()),
                duration: Some(200.0),
                ..Default::default()
            },
        }]);
        // Through the wrapper, so the ask is recorded the way it is in the app.
        app.handle_action(Action::PlayPause);
        app.fullscreen = true;

        let text = draw(&mut app);
        assert!(text.contains("starting"), "{text}");
        assert!(!text.contains("stopped"), "the word that made it look broken: {text}");
        // The length came from the library, so the bar has its total already.
        assert!(text.contains("0:00 / 3:20"), "{text}");
    }

    /// A track with every tag mStream can give us, for the now-playing card.
    fn tagged_track() -> Track {
        Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata {
                title: Some("Rewind The Track".into()),
                artist: Some("Bassnectar".into()),
                album: Some("Divergent Spectrum".into()),
                year: Some(2011),
                duration: Some(209.0),
                bpm: Some(174),
                musical_key: Some("A minor".into()),
                rating: Some(7),
                play_count: Some(23),
                track: Some(3),
                track_total: Some(12),
                genres: vec!["Dubstep".into()],
                format: Some("flac".into()),
                bitrate: Some(1025000),
                sample_rate: Some(44100),
                channels: Some(2),
                bit_depth: Some(16),
                file_size: Some(39691544),
                ..Default::default()
            },
        }
    }

    #[test]
    fn the_view_shows_the_track_the_gauge_and_the_modes() {
        let mut app = connected_app();
        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.status.playing = true;
        app.status.position = 71.0;
        app.status.duration = 209.0;

        app.fullscreen = true;
        let text = draw(&mut app);

        assert!(text.contains("Bassnectar") && text.contains("Rewind The Track"), "{text}");
        assert!(text.contains("Album   Divergent Spectrum"), "{text}");
        assert!(text.contains("Year    2011"), "{text}");
        assert!(text.contains("1:11 / 3:29"), "the gauge is there: {text}");
        // The modes come along: the view's last row carries them.
        assert!(text.contains("vol 100%"), "{text}");
    }

    #[test]
    fn the_now_playing_card_skips_the_tags_a_track_does_not_have() {
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        let text = card_text(&app);
        for row in ["Album", "Year", "Tempo", "Key", "Rating", "Plays"] {
            assert!(text.contains(row), "{row} is missing from:\n{text}");
        }
        assert!(text.contains("174 BPM"), "{text}");
        // The Camelot code leads, because that is what Auto-DJ matches on,
        // and the tag's own spelling follows it.
        assert!(text.contains("8A  A minor"), "{text}");

        // A bare file: no labels standing over empty values.
        app.now_playing = Some(Track {
            filepath: "lib/Artist/untagged.mp3".into(),
            metadata: TrackMetadata::default(),
        });
        let text = card_text(&app);
        assert!(text.contains("unknown artist"), "{text}");
        assert!(text.contains("untagged.mp3"), "falls back to the filename: {text}");
        for row in ["Album", "Year", "Tempo", "Key", "Rating", "Plays"] {
            assert!(!text.contains(row), "{row} has nothing to show:\n{text}");
        }
    }

    fn card_text(app: &App) -> String {
        now_playing_card(app, 60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_numbers_reach_every_tab_including_the_one_you_can_get_stuck_on() {
        // ←→ used to navigate, with Auto-DJ as the exception because its
        // rows are values — which put the only escape that mattered on the
        // only screen where it was different. Reported live as "once you go
        // to Auto-DJ you can't go back".
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        app.fullscreen = true;

        // The tagged track has no lyrics, so that tab is not in the strip —
        // and the numbers close over the gap rather than leaving a dead key.
        assert_eq!(app.now_tabs(), vec![NowTab::Queue, NowTab::Discover, NowTab::AutoDj, NowTab::Visualizer]);

        // The strip's numbers, as the screen sends them: the digit less one.
        let number = |app: &mut App, digit: usize| {
            app.handle_action(Action::SelectNowTab(digit - 1));
        };
        for (digit, expected) in [
            (3, NowTab::AutoDj),
            (1, NowTab::Queue),
            (4, NowTab::Visualizer),
            (2, NowTab::Discover),
        ] {
            number(&mut app, digit);
            assert_eq!(app.now_tab(), expected, "{digit} is {expected:?}");
        }

        // Every tab is reachable from Auto-DJ, which is the whole point.
        number(&mut app, 3);
        assert_eq!(app.now_tab(), NowTab::AutoDj);
        number(&mut app, 1);
        assert_eq!(app.now_tab(), NowTab::Queue, "and back out again");

        // A number past the end is nothing, not a panic — the strip is
        // shorter here than its five slots.
        number(&mut app, 5);
        assert_eq!(app.now_tab(), NowTab::Queue);

        // Tab and Shift+Tab stay as the way that needs no counting.
        app.handle_action(Action::NowTabNext);
        assert_eq!(app.now_tab(), NowTab::Discover);
        app.handle_action(Action::NowTabPrev);
        assert_eq!(app.now_tab(), NowTab::Queue);

        // And ←→ no longer move between tabs at all: on Auto-DJ they adjust,
        // everywhere else they are simply free.
        app.handle_action(Action::NowRight);
        assert_eq!(app.now_tab(), NowTab::Queue, "→ is not navigation any more");
        app.handle_action(Action::NowLeft);
        assert_eq!(app.now_tab(), NowTab::Queue);
    }

    #[test]
    fn the_lyrics_tab_turns_on_the_track_not_the_server() {
        let mut app = connected_app();
        app.fullscreen = !app.fullscreen;

        let mut track = tagged_track();
        track.metadata.has_lyrics = true;
        app.now_playing = Some(track);
        assert!(app.now_tabs().contains(&NowTab::Lyrics));
        assert!(draw(&mut app).contains("Lyrics"));

        // The track carrying the tab can end while you are reading it, so the
        // view must fall back rather than sit on a tab that no longer exists.
        app.now_tab = NowTab::Lyrics;
        app.now_playing = Some(tagged_track());
        assert_eq!(app.now_tab(), NowTab::Queue);
        assert!(!draw(&mut app).contains("Lyrics"));
    }

    #[test]
    fn the_fullscreen_queue_keeps_the_scroll_its_render_worked_out() {
        // `ListState` is `Copy`, so rendering into a local copy threw the
        // corrected offset away with every frame: the selection stuck to
        // the bottom edge and the list slid underneath it.
        let mut app = connected_app();
        app.replace_queue(
            (0..40)
                .map(|i| Track {
                    filepath: format!("lib/{i:02}.mp3"),
                    metadata: Default::default(),
                })
                .collect(),
        );
        app.fullscreen = !app.fullscreen;
        app.now_tab = NowTab::Queue;
        app.queue.state.select(Some(39));

        draw_sized(&mut app, 60, 12);
        assert!(app.queue.state.offset() > 0, "the render's scroll correction survives it");
    }

    #[test]
    fn the_auto_dj_tab_is_where_auto_dj_is_changed() {
        // It reported and pointed at a modal behind `D` — a second screen
        // describing one set of values, and one that never drew over this
        // view at all. The tab is the panel now.
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        app.replace_queue(vec![tagged_track()]);
        app.fullscreen = !app.fullscreen;
        app.dj_server = Some("http://host:3000".into());
        app.now_tab = NowTab::AutoDj;

        let text = draw(&mut app);
        assert!(text.contains("Auto DJ") && text.contains("picking from"), "{text}");
        assert!(text.contains("Songs per fetch"), "{text}");
        assert!(!text.contains("opens the panel"), "there is no other panel: {text}");

        app.handle_action(Action::NowRight);
        assert!(!app.dj_armed(), "the row under the cursor toggled it");
    }

    #[test]
    fn the_progress_bar_draws_a_track_rather_than_fading_into_nothing() {
        let mut app = connected_app();
        app.status.position = 30.0;
        app.status.duration = 60.0;

        let line = progress_line(&app, 40, None, false);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("0:30 / 1:00"), "{text}");
        // Half filled, and the other half is a visible track: ratatui's Gauge
        // paints its remainder with the background, so a bar with only a
        // foreground set ran off into nothing.
        let filled = text.chars().filter(|c| *c == '█').count();
        let track = text.chars().filter(|c| *c == '░').count();
        assert_eq!(filled, track, "{text}");
        assert!(track > 0, "the unfilled half has to be visible: {text}");

        // No room for a bar: the time still gets said.
        let cramped: String =
            progress_line(&app, 12, None, false).spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(cramped.contains("0:30"), "{cramped}");
    }

    #[test]
    fn the_legacy_glyphs_are_what_a_console_font_actually_has() {
        // Probed, not assumed: of what this UI draws, Consolas — the Windows
        // 10 default console font — is missing six of the eight eighth
        // blocks, ▏, ▶, ◆, ⏸ and every braille pattern. It keeps the old
        // CP437 shapes, so those are what the fallback is built from.
        let cp437 = [
            " ", "\u{2591}", "\u{2592}", "\u{2593}", "\u{2584}", "\u{2588}", ">", "|", "=", "*",
            "/", "-", "\\",
        ];
        for glyph in LEGACY.eighths {
            assert!(cp437.contains(&glyph), "{glyph:?} is not something every console has");
        }
        for glyph in [LEGACY.playing, LEGACY.caret, LEGACY.paused, LEGACY.seed] {
            assert!(cp437.contains(&glyph), "{glyph:?} is not something every console has");
        }
        assert!(LEGACY.spinner.iter().all(|g| cp437.contains(g)));

        // Density, not height: three heights cannot carry a mirror. The
        // symmetry one needs (ink(n) + ink(8-n) = a full cell) forces every
        // value between empty and full onto the same half-height, and the
        // whole band comes out as a flat wall of ▄ — measured, on this
        // machine, before the fallback changed to shading.
        assert!(!LEGACY.mirrored);
        assert!(LEGACY.eighths.contains(&"\u{2591}") && LEGACY.eighths.contains(&"\u{2593}"));

        // Where a set does mirror, that symmetry has to hold, or the two
        // halves disagree about where the middle is — a shape with a step
        // in it down the centre line.
        //
        // Measured against how much of a cell each glyph actually inks. An
        // earlier version compared each entry with the two endpoints, which
        // is true of any nine glyphs with different ends and so said nothing
        // about the seven in between — the only ones that can be wrong.
        fn ink(glyph: &str) -> usize {
            match glyph {
                " " => 0,
                "\u{2581}" => 1,
                "\u{2582}" => 2,
                "\u{2583}" => 3,
                "\u{2584}" => 4,
                "\u{2585}" => 5,
                "\u{2586}" => 6,
                "\u{2587}" => 7,
                "\u{2588}" => 8,
                other => panic!("{other:?} has no height a mirror could measure"),
            }
        }
        for set in [RICH, LEGACY].into_iter().filter(|s| s.mirrored) {
            for height in 0..=8usize {
                assert_eq!(
                    ink(set.eighths[height]),
                    height,
                    "eighths[{height}] does not fill {height} eighths in {:?}",
                    set.eighths
                );
                assert_eq!(
                    ink(set.eighths[height]) + ink(set.eighths[8 - height]),
                    8,
                    "height {height} and its complement do not make one cell in {:?}",
                    set.eighths
                );
            }
        }
    }

    #[test]
    fn the_glyph_set_is_guessed_from_the_terminal() {
        // Anything not Windows has these glyphs, and on Windows the
        // question is whether this is Windows Terminal.
        assert_eq!(*glyphs(), Glyphs::detect());
        if !cfg!(windows) {
            assert_eq!(Glyphs::detect(), RICH);
        }
    }

    #[test]
    fn the_waveform_shows_energy_rather_than_saturating_on_peaks() {
        // Found live against demo.mstream.io: resampling by peak drew 83 of
        // 86 columns at full height. The server's bars are already peaks, so
        // peak-of-peaks over the ~10 sharing a column asks "was anything
        // loud in these three seconds", and on a modern master that is yes
        // everywhere. A column of alternating loud and quiet is a quieter
        // passage than a column that is loud throughout, and only an energy
        // measure says so.
        let solid = vec![250; 20];
        let sparse: Vec<u8> = (0..20).map(|i| if i % 2 == 0 { 250 } else { 40 }).collect();
        let a = resample_bars(&solid, 1)[0];
        let b = resample_bars(&[solid.clone(), sparse].concat(), 2);
        assert_eq!(a, 255, "a column with nothing but signal is full height");
        assert!(b[1] < b[0], "and the half-empty one is shorter: {b:?}");

        // Scaled to the band each track actually uses, since nothing here
        // applies ReplayGain: a quiet master and a loud one both fill the
        // row, and both keep their own shape.
        let quiet = resample_bars(&[10, 10, 10, 40, 40, 40], 2);
        let loud = resample_bars(&[100, 100, 100, 255, 255, 255], 2);
        assert_eq!((quiet[0], quiet[1]), (0, 255), "level is not what this draws");
        assert_eq!((loud[0], loud[1]), (0, 255));

        // A densely mastered body — everything between 230 and 250 — is the
        // case a plain 0..255 scaling could not show at all: eight heights
        // over a twenty-wide band is one height. Stretched, it has shape.
        let dense: Vec<u8> = (0..80).map(|i| 230 + (i % 3) as u8 * 10).collect();
        let drawn = resample_bars(&dense, 8);
        assert!(drawn.iter().any(|h| *h > 200) && drawn.iter().any(|h| *h < 60), "{drawn:?}");

        // No variation at all is drawn at its own level, not at nothing.
        assert_eq!(resample_bars(&[200; 16], 4), vec![255, 255, 255, 255]);

        // Exactly as many columns as asked for, whichever way the ratio goes.
        // 800 is what the server sends, whatever the track's length.
        let full: Vec<u8> = (0..800).map(|i| (i % 256) as u8).collect();
        assert_eq!(resample_bars(&full, 73).len(), 73);
        assert_eq!(resample_bars(&full, 1).len(), 1);
        // More columns than bars: every column still gets a value rather
        // than an empty range panicking on `start..start`.
        assert_eq!(resample_bars(&[9, 4], 5).len(), 5);
        // Silence has no loudest column to scale against — flat, not a
        // division by zero.
        assert_eq!(resample_bars(&[0; 16], 4), vec![0, 0, 0, 0]);
        // And the degenerate ends are answers, not panics.
        assert!(resample_bars(&full, 0).is_empty());
        assert!(resample_bars(&[], 10).is_empty());

        // The floor is the *tenth percentile*, not the minimum, and every
        // case above is too narrow to tell the two apart: `sorted[len / 10]`
        // collapses to `sorted[0]` under ten columns. Twenty columns puts
        // the floor on the third-quietest, so the two below it are what
        // proves which of the two rules is in force — under a minimum floor
        // the third would scale to something above zero.
        let mut body: Vec<u8> = (0..240).map(|i| 150 + (i % 7) as u8 * 15).collect();
        body[..24].fill(3); // two whole columns of near-silence
        body[24..36].fill(60); // and the one that becomes the floor
        let wide = resample_bars(&body, 20);
        assert_eq!(wide.len(), 20);
        assert_eq!(
            (wide[0], wide[1], wide[2]),
            (0, 0, 0),
            "everything at or under the tenth percentile is the bottom: {wide:?}"
        );
        assert!(wide[3..].iter().any(|h| *h == 255), "and the loudest still reaches the top");
    }

    #[test]
    fn the_loudest_column_reaches_the_top_of_the_cell() {
        // `amplitude * 8 / 256` cannot return 8 — 255 is the largest input
        // and lands on 7 — so the full block was unreachable and every
        // waveform drew a notch short of the height it had been scaled to.
        assert_eq!(eighths(255), 8);
        assert_eq!(eighths(0), 0);
        // Still monotonic, and still eight steps rather than nine crammed in.
        let steps: Vec<usize> = (0..=255u8).map(eighths).collect();
        assert!(steps.windows(2).all(|w| w[0] <= w[1]), "heights only go up");
        assert_eq!(*steps.iter().max().unwrap(), 8);
        assert!(steps.iter().all(|h| *h <= 8), "and never off the end of the glyph set");
    }

    #[test]
    fn the_waveform_decorates_the_bar_without_taking_it_over() {
        let mut app = connected_app();
        app.status.position = 30.0;
        app.status.duration = 60.0;
        app.now_playing = Some(Track { filepath: "lib/a.mp3".into(), metadata: Default::default() });

        // Nothing cached yet: the plain bar, exactly as before.
        let plain: String =
            progress_line(&app, 40, None, false).spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(plain.contains('█') && plain.contains('░'), "{plain}");

        // A shape that starts silent and ends loud.
        let bars: Vec<u8> = (0..800).map(|i| (i / 4) as u8).collect();
        app.waveforms.insert("lib/a.mp3".into(), Some(bars));
        let line = progress_line(&app, 40, None, false);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with("0:30 / 1:00"), "the time is untouched: {text}");
        assert!(
            text.chars().any(|c| glyphs().eighths[1..8].contains(&c.to_string().as_str())),
            "the shape is drawn: {text}"
        );
        // The plain bar has exactly two glyphs, played and not. Anything
        // wearing a shape has more than that, whether the steps are heights
        // or densities.
        let bar: String = text.chars().take_while(|c| !c.is_ascii_digit()).collect();
        let distinct: std::collections::HashSet<char> = bar.trim().chars().collect();
        assert!(distinct.len() > 2, "it replaces the flat track: {text}");

        // The three channels are unchanged: played is the accent, unplayed
        // is not, and the bar is still a control rather than a picture.
        let bar_spans: Vec<&Span> =
            line.spans.iter().take_while(|s| !s.content.starts_with("  ")).collect();
        assert!(bar_spans.iter().any(|s| s.style.fg == Some(accent())), "{text}");
        assert!(bar_spans.iter().any(|s| s.style.fg == Some(dim())), "{text}");

        // A track the server has no shape for keeps the plain bar rather
        // than drawing nothing.
        app.waveforms.insert("lib/a.mp3".into(), None);
        let none: String =
            progress_line(&app, 40, None, false).spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(none.contains('█') && none.contains('░'), "{none}");
    }

    #[test]
    fn the_full_screen_band_mirrors_the_shape_above_the_scrubber() {
        // One row of block glyphs is eight heights, which is a bar chart
        // rather than a waveform. The full-screen view has room for the
        // other half, and a shape mirrored about the line between them is
        // what reads as a recording.
        let mut app = connected_app();
        app.status.position = 30.0;
        app.status.duration = 60.0;
        app.now_playing = Some(Track { filepath: "lib/a.mp3".into(), metadata: Default::default() });
        // Quiet at one end, loud at the other.
        let bars: Vec<u8> = (0..800).map(|i| (i / 4) as u8).collect();
        app.waveforms.insert("lib/a.mp3".into(), Some(bars));

        // Whether there is a mirror at all is the font's call, so this
        // asserts whichever answer is in force on the machine running it —
        // both are correct, and CI runs one of each.
        if !glyphs().mirrored {
            assert!(
                waveform_top_line(&app, 40).spans.is_empty(),
                "a console font draws one row of density, not two of height"
            );
            assert_eq!(wave_half_rows(), 0, "and claims no row for the half it cannot draw");
            return;
        }

        // The shape grows *outward* from the line between the two rows, so
        // the upper half is an ordinary lower block — the centre is the
        // bottom of that row, which is where a lower block already starts.
        let top = waveform_top_line(&app, 40);
        assert!(!top.spans.is_empty(), "the upper half is drawn");
        assert!(
            top.spans.iter().all(|s| !s.style.add_modifier.contains(Modifier::REVERSED)),
            "the upper half needs no trick"
        );

        // The half below has to hang from the centre instead, and Unicode
        // has no eighth-resolution block that does — so it is the same glyph
        // REVERSED, which swaps which part of the cell is inked.
        let bottom = progress_line(&app, 40, None, true);
        let shape: Vec<&Span> = bottom
            .spans
            .iter()
            .filter(|s| glyphs().eighths[1..].iter().any(|g| s.content.contains(g)))
            .collect();
        assert!(!shape.is_empty(), "the lower half is drawn");
        assert!(
            shape.iter().all(|s| s.style.add_modifier.contains(Modifier::REVERSED)),
            "and it hangs"
        );

        // Both wear the same two colours, split at the same place, so the
        // pair reads as one object rather than two adjacent pictures.
        for half in [&top, &bottom] {
            assert!(half.spans.iter().any(|s| s.style.fg == Some(accent())));
            assert!(half.spans.iter().any(|s| s.style.fg == Some(dim())));
        }

        // With no shape to draw there is no half to draw it in — and the
        // scrubber falls back to the flat bar it always was.
        app.waveforms.insert("lib/a.mp3".into(), None);
        assert!(waveform_top_line(&app, 40).spans.is_empty());
        let plain: String = progress_line(&app, 40, None, true)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(plain.contains('█') && plain.contains('░'), "{plain}");
    }

    #[test]
    fn silence_draws_a_flat_line_rather_than_half_a_band() {
        // Nothing playing used to leave the upper half blank over a full
        // lower one, which reads as neither silence nor progress. A silent
        // waveform is a line through the middle.
        let mut app = connected_app();
        assert!(app.now_playing.is_none());
        if !glyphs().mirrored {
            return; // no band to be half of
        }

        let top = waveform_top_line(&app, 40);
        let text: String = top.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.is_empty(), "the upper half is drawn");
        assert!(
            text.chars().all(|c| c.to_string() == glyphs().eighths[1]),
            "and it is the thinnest glyph, all the way across: {text:?}"
        );

        // The half below meets it, hanging from the same centre line.
        let bottom = progress_line(&app, 40, None, true);
        let shape: Vec<&Span> = bottom
            .spans
            .iter()
            .filter(|s| s.content.contains(glyphs().eighths[7]))
            .collect();
        assert!(!shape.is_empty(), "the lower half is drawn: {bottom:?}");
        assert!(shape.iter().all(|s| s.style.add_modifier.contains(Modifier::REVERSED)));
        let drawn: String = bottom.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!drawn.contains('░'), "and the flat track is gone with it: {drawn}");
        assert!(drawn.ends_with("0:00 / --:--"), "the time still says its piece: {drawn}");

        // A track that is simply stopped is not silence — it has a shape, or
        // failing that a progress bar, and either is more use than a line.
        app.now_playing =
            Some(Track { filepath: "lib/a.mp3".into(), metadata: Default::default() });
        assert!(waveform_top_line(&app, 40).spans.is_empty(), "no half without a shape");
        let stopped: String = progress_line(&app, 40, None, true)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(stopped.contains('░'), "the flat track is back: {stopped}");
    }

    #[test]
    fn the_hover_marker_survives_the_waveform() {
        // The marker is the one column that says where a click would land,
        // so the shape must not be allowed to paint over it.
        let mut app = connected_app();
        app.status.position = 0.0;
        app.status.duration = 60.0;
        app.now_playing = Some(Track { filepath: "lib/a.mp3".into(), metadata: Default::default() });
        app.waveforms.insert("lib/a.mp3".into(), Some(vec![255; 800]));

        let text: String =
            progress_line(&app, 40, Some(3), false).spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains(glyphs().caret), "the marker is still there: {text}");
    }

    #[test]
    fn the_strip_gives_up_its_names_before_it_truncates_one() {
        let mut app = connected_app();
        let mut track = tagged_track();
        track.metadata.has_lyrics = true;
        app.now_playing = Some(track);
        app.fullscreen = !app.fullscreen;

        // Wide enough: every tab is named, and wearing the number that
        // reaches it.
        let wide = draw_sized(&mut app, 110, 16);
        assert!(
            wide.contains("[1:Queue]  2:Lyrics   3:Similar   4:Auto-DJ   5:Visualizer "),
            "{wide}"
        );

        // Not wide enough: the one you are on, and arrows for the rest. A tab
        // chopped to "Vis" reads as a bug; this reads as a choice. The number
        // survives the squeeze — it is the way out of wherever you are.
        let narrow = draw_sized(&mut app, 64, 16);
        assert!(narrow.contains("‹ 1:Queue ›"), "{narrow}");
        assert!(!narrow.contains("Similar"), "{narrow}");
    }

    #[test]
    fn an_unreadable_key_is_shown_as_written_rather_than_dropped() {
        let mut app = connected_app();
        app.now_playing = Some(Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata { musical_key: Some("wonky".into()), ..Default::default() },
        });
        assert!(card_text(&app).contains("wonky"));
    }

    #[test]
    fn the_now_playing_screen_says_so_when_nothing_is() {
        let mut app = connected_app();
        app.fullscreen = !app.fullscreen;
        let text = draw(&mut app);
        assert!(text.contains("nothing playing"), "{text}");
        assert!(text.contains("0 goes back to the library"), "{text}");
    }

    #[test]
    fn the_rule_joins_the_divider_it_is_drawn_under() {
        // The junction exists to make the facts column's right border meet
        // the rule below it. Off by one it is a dangling join: a ┴ under
        // nothing, and the border stopping at a blank cell.
        let mut app = connected_app();
        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.fullscreen = !app.fullscreen;

        for width in [80u16, 90, 120] {
            let mut terminal = Terminal::new(TestBackend::new(width, 26)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer().clone();

            let column_of = |row: u16, symbol: &str| {
                (0..buffer.area.width).find(|x| buffer[(*x, row)].symbol() == symbol)
            };
            // The rule is the row holding the junction; the divider is the
            // vertical border on the row above it.
            let rule_row = (0..buffer.area.height)
                .find(|y| column_of(*y, "┴").is_some())
                .unwrap_or_else(|| panic!("no junction drawn at {width} columns"));
            let junction = column_of(rule_row, "┴").unwrap();
            let divider = column_of(rule_row - 1, "│")
                .unwrap_or_else(|| panic!("no divider above the rule at {width} columns"));
            assert_eq!(
                junction, divider,
                "at {width} columns the junction is in {junction} and the divider in {divider}"
            );
        }
    }

    #[test]
    fn a_wide_terminal_does_not_overflow_the_panel_arithmetic() {
        // centered_rect multiplied a u16 width by a percent: at 937 columns
        // 937 * 70 passes 65535, which is a panic in debug and a nonsense
        // width in release. Every other size calculation here saturates.
        for width in [937u16, 2000, 10_000, u16::MAX] {
            let area = Rect { x: 0, y: 0, width, height: 40 };
            let box_area = centered_rect(70, 20, area);
            assert!(box_area.width <= width);
            assert!(box_area.x + box_area.width <= width, "the box left the screen");
            // 70% of the width, worked out without wrapping round.
            assert_eq!(box_area.width, (u32::from(width) * 70 / 100) as u16);
        }
    }

    #[test]
    fn the_now_playing_screen_survives_a_small_terminal() {
        let mut app = connected_app();
        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.fullscreen = !app.fullscreen;
        for (w, h) in [(20u16, 4u16), (32, 6), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        }
    }

    #[test]
    fn the_footer_names_only_the_modes_that_are_on() {
        let mut app = connected_app();
        assert_eq!(mode_readout(&app, false), "vol 100%", "all defaults says nothing extra");

        app.handle_action(Action::ToggleRepeat);
        app.handle_action(Action::ToggleShuffle);
        app.dj_server = Some("http://elsewhere:3000".into());
        let full = mode_readout(&app, false);
        assert!(full.contains("repeat all"), "{full}");
        assert!(full.contains("shuffle"), "{full}");
        assert!(full.contains("dj on · http://elsewhere:3000"), "where it picks from, when not here: {full}");

        let compact = mode_readout(&app, true);
        assert!(width_of(&compact) < width_of(&full), "the short form is shorter: {compact}");
        assert!(compact.contains("rpt all") && compact.contains("shf"), "{compact}");
    }

    /// The Auto-DJ tab, as its own screen rather than as a modal over the
    /// browser: `0` opens the full-screen view, then the tab.
    fn on_the_dj_tab(app: &mut App) {
        app.fullscreen = !app.fullscreen;
        while app.now_tab() != NowTab::AutoDj {
            app.handle_action(Action::NowTabNext);
        }
    }

    #[test]
    fn the_dj_tab_shows_each_setting_with_what_it_means() {
        let mut app = connected_app();
        app.dj.sonic_min_similarity = 0.6;
        app.dj.bpm = true;
        app.dj.bpm_tolerance = 6;
        app.dj_panel.rebuild(&app.dj, None, false);
        on_the_dj_tab(&mut app);
        let text = draw_sized(&mut app, 100, 34);

        assert!(text.contains("Auto-DJ"));
        for row in ["Auto DJ", "Sonic similarity", "Match strictness", "Anchor", "BPM continuity",
                    "Harmonic mixing", "Genre filter", "Preview"] {
            assert!(text.contains(row), "missing {row} in:\n{text}");
        }
        // The bar shows the raw floor it stands at, so the number the
        // server filters on is never a mystery.
        assert!(text.contains("cosine ≥ 0.60"), "got:\n{text}");
        assert!(text.contains("60%"));
        // And the tolerance row says what the fallback widens to.
        assert!(text.contains("± 6 BPM") && text.contains("wide set ± 8"), "got:\n{text}");
        assert!(text.contains("Enter to preview"), "got:\n{text}");
        // The strip is the one this session has: this app has no Lyrics
        // tab (the track carries none), so it names four and not a fifth.
        assert_eq!(app.now_tabs().len(), 4);
        assert!(text.contains("4:Visualizer") && !text.contains("5:Visualizer"), "got:\n{text}");
    }

    /// Off, the first row says where Enter starts the DJ: here — and, in
    /// the shell that has a Play from (the GUI hosts this tab on its Now
    /// Playing screen), on its home when that is another server.
    #[test]
    fn the_dj_tab_says_where_enter_starts_it() {
        use crate::tui::app::KnownServer;
        let mut app = connected_app();
        let here = app.session_identity().expect("a session");
        let known = |id: &str, name: &str| KnownServer {
            id: id.into(),
            name: name.into(),
            token: None,
            username: None,
            self_signed: false,
            peer: None,
            pairing: None,
            dj: Default::default(),
        };
        app.servers = vec![known(&here, "here"), known("http://attic:3000", "attic")];
        app.dj_home = Some("http://attic:3000".into());
        on_the_dj_tab(&mut app);
        let text = draw_sized(&mut app, 100, 34);
        assert!(text.contains("Enter starts it here"), "no Play from on this shell's screen: {text}");
        app.dj_shows_play_from = true;
        let text = draw_sized(&mut app, 100, 34);
        assert!(text.contains("Enter starts it on attic"), "the home, where the shell has one: {text}");
        app.dj_home = Some(here);
        let text = draw_sized(&mut app, 100, 34);
        assert!(text.contains("Enter starts it here"), "{text}");
    }

    #[test]
    fn the_dj_tab_hides_the_sonic_rows_while_the_switch_is_off() {
        let mut app = connected_app();
        app.dj.sonic = false;
        app.dj_panel.rebuild(&app.dj, None, false);
        on_the_dj_tab(&mut app);
        let text = draw_sized(&mut app, 100, 34);
        assert!(!text.contains("Match strictness"), "a switch off hides its details");
        assert!(!text.contains("Anchor"));
        assert!(text.contains("BPM continuity"), "the rest of the tab is still there");
    }

    #[test]
    fn the_dj_tab_reports_the_pool_and_the_sample() {
        let mut app = connected_app();
        on_the_dj_tab(&mut app);
        app.apply_event(crate::tui::worker::Event::AutoDjSample {
            tracks: vec![Track {
                filepath: "lib/x.mp3".into(),
                metadata: TrackMetadata {
                    artist: Some("Band".into()),
                    title: Some("Song".into()),
                    ..Default::default()
                },
            }],
            pool: Some(crate::api::types::SonicReport {
                similarity: Some(0.71),
                pool_size: 1247,
            }),
            note: None,
        });
        let text = draw_sized(&mut app, 100, 34);
        // The pool size is the number that makes the slider tunable.
        assert!(text.contains("1247 tracks in the sonic pool"), "got:\n{text}");
        assert!(text.contains("1. Band - Song"), "got:\n{text}");
    }

    #[test]
    fn the_genre_chooser_marks_what_is_selected() {
        let _en = english();
        let mut app = connected_app();
        app.dj.genres = vec!["Techno".into()];
        on_the_dj_tab(&mut app);
        app.dj_panel.row =
            app.dj_panel.rows.iter().position(|r| *r == DjRow::Genres).unwrap();
        app.handle_action(Action::Activate);
        app.apply_event(crate::tui::worker::Event::Genres(vec![
            crate::api::types::Genre { name: "Ambient".into(), track_count: None },
            crate::api::types::Genre { name: "Techno".into(), track_count: None },
        ]));

        let text = draw_sized(&mut app, 100, 34);
        assert!(text.contains("[ ] Ambient"), "got:\n{text}");
        assert!(text.contains("[x] Techno"), "already chosen:\n{text}");
        assert!(text.contains("Space toggle"));
    }

    #[test]
    fn the_dj_tab_survives_a_small_terminal() {
        let mut app = connected_app();
        on_the_dj_tab(&mut app);
        // Must not panic, and must not try to draw outside the frame.
        draw_sized(&mut app, 40, 12);
        draw_sized(&mut app, 20, 8);
    }

    #[test]
    fn the_bar_lights_up_under_the_pointer() {
        let mut app = connected_app();
        app.status.position = 30.0;
        app.status.duration = 60.0;
        let text = |line: Line<'static>| -> String {
            line.spans.iter().map(|s| s.content.as_ref()).collect()
        };

        // No pointer on it: half played, half track, and no marker.
        let cold = progress_line(&app, 40, None, false);
        assert!(!text(cold.clone()).contains(glyphs().caret), "{}", text(cold.clone()));
        let track_colour = cold.spans.iter().find(|s| s.content.contains('\u{2591}')).unwrap();
        assert_eq!(track_colour.style.fg, Some(dim()));

        // Pointer on it: a marker in that column, and the unplayed part comes
        // up out of the dim so the bar reads as something you can press.
        let warm = progress_line(&app, 40, Some(20), false);
        let drawn = text(warm.clone());
        assert_eq!(drawn.matches(glyphs().caret).count(), 1, "{drawn}");
        assert_eq!(drawn.chars().count(), text(cold).chars().count(), "same width either way");
        let track_colour = warm.spans.iter().find(|s| s.content.contains('\u{2591}')).unwrap();
        assert_eq!(track_colour.style.fg, Some(folder()));

        // The marker sits where the pointer is, not where the playhead is.
        let early = text(progress_line(&app, 40, Some(2), false));
        // By column, not by byte — the block glyphs either side are three
        // bytes each.
        let caret = glyphs().caret.chars().next().unwrap();
        assert_eq!(early.chars().position(|c| c == caret), Some(2), "{early}");

        // A length we do not know is a seek we would refuse, so it does not
        // offer one.
        app.status.duration = 0.0;
        assert!(!text(progress_line(&app, 40, Some(20), false)).contains(glyphs().caret));
    }

    #[test]
    fn a_click_lands_where_the_bar_says_it_should() {
        let mut app = connected_app();
        app.status.duration = 200.0;
        app.status.source = "http://host/a.mp3".into();

        // At width 90 the time reads "0:00 / 3:20", eleven columns, and the
        // bar takes what is left after it and its gap.
        let width = 90;
        assert_eq!(seek_target(&app, width, 0), Some(0.0), "the first column is the start");
        let (_, _, bar) = progress_parts(&app, width as usize);
        let bar = bar.unwrap();
        // The last column of the bar is one column short of the end, not the
        // end: a bar of n columns divides the track into n, and the last one
        // starts at (n-1)/n through it.
        let last = seek_target(&app, width, bar as u16 - 1).unwrap();
        let expected = 200.0 * (bar as f64 - 1.0) / bar as f64;
        assert!((last - expected).abs() < 1e-6, "{last} vs {expected}");
        assert!(last < 200.0, "clicking the far end is not past the end");

        // Past the bar is the clock, which is not a seek control.
        assert_eq!(seek_target(&app, width, bar as u16), None);
        assert_eq!(seek_target(&app, width, width - 1), None);

        // A length we do not know is a length we cannot seek into.
        app.status.duration = 0.0;
        assert_eq!(seek_target(&app, width, 4), None);
        // ...and a terminal too narrow to have drawn a bar has none to click.
        app.status.duration = 200.0;
        assert_eq!(seek_target(&app, 12, 1), None);
    }

    #[test]
    fn the_visualizer_names_its_mode_and_v_moves_between_them() {
        use crate::tui::viz::{VIZ_MODES, VizMode};
        let mut app = connected_app();
        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.fullscreen = !app.fullscreen;
        while app.now_tab() != NowTab::Visualizer {
            app.handle_action(Action::NowTabNext);
        }

        // No tap in a test, so the panel says so rather than drawing noise.
        let text = draw(&mut app);
        assert!(text.contains("no audio thread"), "{text}");
        assert!(text.contains("spectrum"), "the mode is named: {text}");

        for expected in VIZ_MODES.iter().skip(1).chain(std::iter::once(&VizMode::Bars)) {
            app.handle_action(Action::CycleViz);
            assert_eq!(app.viz.mode, *expected);
            assert!(draw(&mut app).contains(expected.title()));
        }
        // A mode change is not news anyone needs in the message line.
        assert!(app.message.is_none());

        // `.` says which way it is drawing, and only where that means
        // something — a bar chart has no samples to join.
        app.viz.mode = VizMode::Scope;
        let text = draw(&mut app);
        assert!(text.contains("lines"), "{text}");
        app.handle_action(Action::ToggleScatter);
        assert!(app.viz.scatter);
        assert!(draw(&mut app).contains("dots"));

        app.viz.mode = VizMode::Bars;
        let text = draw(&mut app);
        assert!(!text.contains("dots") && !text.contains("lines"), "nothing to join: {text}");
        // ...but the preference is still held for when you go back to one.
        assert!(app.viz.scatter);
    }

    #[test]
    fn the_cover_sits_under_the_facts_as_pixels_or_as_the_mosaic() {
        let tall = |app: &mut App| draw_sized(app, 90, 44);
        let full = |s: &str| s.matches(crate::tui::canvas::FULL).count();
        // Kitty from the start, so every absence asserted below is the
        // layout's doing and not a disabled pixel path making the
        // assertion true for free.
        let kitty = || {
            crate::tui::graphics::Graphics::forced(ratatui_image::picker::ProtocolType::Kitty)
        };

        let mut app = connected_app();
        app.fullscreen = true;
        app.graphics = kitty();
        let mut track = tagged_track();
        track.metadata.album_art = Some("aa.jpeg".into());
        app.now_playing = Some(track);

        // Nothing fetched yet: a blank corner, not a placeholder — absence
        // dressed up as a fact is how a column starts lying. The pixel
        // path is live; what is missing is only the cover.
        let empty = tall(&mut app);
        assert!(!empty.contains('\u{10EEEE}'), "{empty}");

        // A real decode, so the art carries the bytes the pixel path wants.
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let art = crate::tui::art::decode(&bytes.into_inner()).unwrap();
        app.art.insert("aa.jpeg".into(), Some(art));

        // A terminal that can: kitty's placeholder cells under the facts,
        // on the queue tab, with no visualiser anywhere in the frame — and
        // no thirty-a-second poll held open for a picture that never moves.
        let pixels = tall(&mut app);
        assert!(pixels.contains('\u{10EEEE}'), "kitty draws by placeholder cells: {pixels}");
        assert!(!app.drawing_audio(), "a still picture must not cost thirty frames a second");

        // The same frame on a terminal that cannot: the mosaic fills the
        // same box, and the kitty debris is nowhere in the cells.
        app.graphics = crate::tui::graphics::Graphics::disabled();
        let mosaic = tall(&mut app);
        assert!(!mosaic.contains('\u{10EEEE}'), "{mosaic}");
        assert!(
            full(&mosaic) > full(&pixels),
            "the mosaic is drawn where the pixels were: {mosaic}"
        );

        // At the boundary height this card leaves exactly three spare rows
        // — enough for a small cover, and it draws one. One row shorter
        // and the leftover is under the three-row floor, so the card wins
        // the column and the picture is dropped rather than squeezed. The
        // boundary is 26 rows on mirrored glyphs and 25 on the legacy set,
        // whose band spends one row fewer — derived, so the test says the
        // same thing on both. Both sides drawn with kitty live, so the
        // absence leg cannot pass by the pixel path being off.
        app.graphics = kitty();
        let boundary = 25 + wave_half_rows();
        let short = draw_sized(&mut app, 90, boundary);
        assert!(short.contains('\u{10EEEE}'), "three rows are a cover: {short}");
        let shorter = draw_sized(&mut app, 90, boundary - 1);
        assert!(!shorter.contains('\u{10EEEE}'), "two rows are not: {shorter}");
    }

    #[test]
    fn the_visualizer_only_costs_extra_redraws_while_it_is_showing() {
        let mut app = connected_app();
        assert!(!app.drawing_audio(), "the library does not need thirty frames a second");

        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.fullscreen = !app.fullscreen;
        assert!(!app.drawing_audio(), "nor does the queue tab of the now-playing view");

        while app.now_tab() != NowTab::Visualizer {
            app.handle_action(Action::NowTabNext);
        }
        assert!(app.drawing_audio());

        app.fullscreen = !app.fullscreen;
        assert!(!app.drawing_audio(), "and leaving the view stops it");
    }

    #[test]
    fn the_card_says_what_the_file_is() {
        let mut app = connected_app();
        app.replace_queue(vec![tagged_track()]);
        app.play_index(0);
        app.fullscreen = !app.fullscreen;

        let text = draw(&mut app);
        assert!(text.contains("FLAC   1025 kbps"), "{text}");
        assert!(text.contains("44.1 kHz   16-bit   stereo"), "{text}");
        assert!(text.contains("37.9 MB"), "{text}");
        assert!(text.contains("3 of 12"), "a position, not a bare number: {text}");
        assert!(text.contains("Dubstep"), "{text}");
    }

    #[test]
    fn the_file_facts_leave_out_what_the_server_did_not_say() {
        // Every one of these is optional and a good few come back null on a
        // thin scan, so a row missing has to mean the row is absent, not
        // "unknown" or "0".
        let bare = TrackMetadata::default();
        assert_eq!(file_format(&bare), None);
        assert_eq!(audio_shape(&bare), None);

        // A lossy file has no bit depth; saying nothing is the fact.
        let lossy = TrackMetadata {
            format: Some("mp3".into()),
            bitrate: Some(320000),
            sample_rate: Some(44100),
            channels: Some(2),
            ..Default::default()
        };
        assert_eq!(file_format(&lossy).as_deref(), Some("MP3   320 kbps"));
        assert_eq!(audio_shape(&lossy).as_deref(), Some("44.1 kHz   stereo"));

        // A whole number of kHz drops the decimal; one channel is mono.
        let mono = TrackMetadata {
            sample_rate: Some(48000),
            channels: Some(1),
            bit_depth: Some(24),
            ..Default::default()
        };
        assert_eq!(audio_shape(&mono).as_deref(), Some("48 kHz   24-bit   mono"));

        // Either half of the format row stands on its own.
        let format_only = TrackMetadata { format: Some("opus".into()), ..Default::default() };
        assert_eq!(file_format(&format_only).as_deref(), Some("OPUS"));
        let rate_only = TrackMetadata { bitrate: Some(96000), ..Default::default() };
        assert_eq!(file_format(&rate_only).as_deref(), Some("96 kbps"));
    }

    #[test]
    fn sizes_read_the_way_a_file_manager_says_them() {
        assert_eq!(fmt_bytes(7886891), "7.5 MB");
        assert_eq!(fmt_bytes(39691544), "37.9 MB");
        assert_eq!(fmt_bytes(524_288_000), "500 MB", "no decimal once it stops mattering");
        assert_eq!(fmt_bytes(48_000), "47 KB", "and below a megabyte it is not 0.0 MB");
    }

}
