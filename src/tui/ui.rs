//! Rendering. Reads app state, draws widgets — no decisions of its own.

use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};

use crate::api::types::fmt_duration;

use super::app::{
    App, CONNECT_METHODS, ConnectStage, DjRow, Entry, Focus, MessageKind, NowTab, Queue, Repeat,
    SearchNode, SettingsNode, SonicNode, SonicRow, SonicView, Tab,
};
use super::worker::{AutoDjMode, DiscoverNode, LibraryNode};
use crate::api::types::{Track, TrackMetadata};

/// The colours the drawing code varies, resolved once at startup.
///
/// Process-wide rather than threaded through every helper, the same shape the
/// spool directory uses and for the same reason: `entry_line` and friends are
/// pure functions of a row, and giving each one a palette argument would be
/// all noise and no meaning.
static THEME: OnceLock<Theme> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub accent: Color,
    pub dim: Color,
    pub folder: Color,
}

impl Default for Theme {
    /// Palette names, not RGB. A terminal app that hard-codes hues looks wrong
    /// inside somebody else's colour scheme; naming the slot lets their scheme
    /// answer. `[theme]` is there for anyone who would rather pin it.
    fn default() -> Self {
        Theme { accent: Color::Cyan, dim: Color::DarkGray, folder: Color::Blue }
    }
}

impl Theme {
    /// Read a `[theme]` section, reporting what it could not use rather than
    /// refusing to start. One unreadable colour costs that colour and nothing
    /// else — the same bargain `[keys]` makes.
    pub fn from_prefs(prefs: &crate::config::ThemePrefs) -> (Self, Vec<String>) {
        let mut theme = Theme::default();
        let mut warnings = Vec::new();
        let mut set = |slot: &mut Color, raw: &Option<String>, name: &str| {
            let Some(raw) = raw else { return };
            match parse_color(raw) {
                Some(color) => *slot = color,
                None => warnings.push(format!(
                    "theme.{name}: '{raw}' is not a colour name, #rrggbb or 0-255 — keeping the default"
                )),
            }
        };
        set(&mut theme.accent, &prefs.accent, "accent");
        set(&mut theme.dim, &prefs.dim, "dim");
        set(&mut theme.folder, &prefs.folder, "folder");
        (theme, warnings)
    }
}

/// The glyphs the drawing code varies, resolved once at startup — the same
/// shape as [`THEME`] and for the same reason.
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
    /// `auto` asks the terminal, anything else is taken at its word.
    ///
    /// `WT_SESSION` is set by Windows Terminal and not by conhost, which is
    /// the whole of the question on Windows. Everywhere else — including the
    /// browser build, where the page picks the font — a modern font is a
    /// safe assumption.
    pub fn from_prefs(prefs: &crate::config::DisplayPrefs) -> (Self, Vec<String>) {
        match prefs.glyphs.trim().to_ascii_lowercase().as_str() {
            "full" => (RICH, Vec::new()),
            "legacy" => (LEGACY, Vec::new()),
            "auto" => (Glyphs::detect(), Vec::new()),
            other => (
                Glyphs::detect(),
                vec![format!(
                    "display.glyphs: '{other}' is not auto, full or legacy — using auto"
                )],
            ),
        }
    }

    fn detect() -> Self {
        if !cfg!(windows) || std::env::var_os("WT_SESSION").is_some() { RICH } else { LEGACY }
    }
}

pub fn set_glyphs(glyphs: Glyphs) {
    let _ = GLYPHS.set(glyphs);
}

pub(crate) fn glyphs() -> &'static Glyphs {
    GLYPHS.get_or_init(Glyphs::detect)
}

pub fn set_theme(theme: Theme) {
    let _ = THEME.set(theme);
}

fn theme() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

pub(crate) fn accent() -> Color {
    theme().accent
}

pub(crate) fn dim() -> Color {
    theme().dim
}

pub(crate) fn folder() -> Color {
    theme().folder
}

/// A palette name, an exact `#rrggbb`, or an index into the 256-colour cube.
fn parse_color(raw: &str) -> Option<Color> {
    let text = raw.trim().to_ascii_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        return Some(Color::Rgb(byte(0)?, byte(2)?, byte(4)?));
    }
    if let Ok(index) = text.parse::<u8>() {
        return Some(Color::Indexed(index));
    }
    // Both spellings of grey, and both ways of writing the bright half, since
    // there is no reason to make someone guess which one this program wanted.
    Some(match text.replace('_', "-").as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" | "gray" | "grey" => Color::Gray,
        "dark-gray" | "dark-grey" | "bright-black" | "light-black" => Color::DarkGray,
        "bright-red" | "light-red" => Color::LightRed,
        "bright-green" | "light-green" => Color::LightGreen,
        "bright-yellow" | "light-yellow" => Color::LightYellow,
        "bright-blue" | "light-blue" => Color::LightBlue,
        "bright-magenta" | "light-magenta" => Color::LightMagenta,
        "bright-cyan" | "light-cyan" => Color::LightCyan,
        "bright-white" | "light-white" => Color::White,
        _ => return None,
    })
}

/// Left gutter the cursor symbol reserves on every browser row, blank or not.
const CURSOR: &str = "> ";

/// Braille spinner. Ten frames is slow enough to read as turning rather than
/// flickering at the rate the event loop advances it.
/// Which frame the spinner is on. The frame count differs between the two
/// glyph sets — braille turns in ten, ASCII in four — so the modulo has to
/// ask the set rather than a constant.
fn spinner_frame(app: &App) -> &'static str {
    let frames = glyphs().spinner;
    frames[app.spinner % frames.len()]
}

/// The mStream wordmark, character for character as the server prints it at
/// boot (cli-boot-wrapper.js).
const BANNER: [&str; 5] = [
    r"               ____  _",
    r"     _ __ ___ / ___|| |_ _ __ ___  __ _ _ __ ___",
    r"    | '_ ` _ \\___ \| __| '__/ _ \/ _` | '_ ` _ \",
    r"    | | | | | |___) | |_| | |  __/ (_| | | | | | |",
    r"    |_| |_| |_|____/ \__|_|  \___|\__,_|_| |_| |_|",
];

/// Widest banner line; below this the art is dropped rather than wrapped.
const BANNER_WIDTH: u16 = 49;

/// Banner lines, or nothing when the terminal is too small to hold it without
/// mangling the art.
fn banner_lines(area: Rect) -> Vec<Line<'static>> {
    if area.width < BANNER_WIDTH + 2 || area.height < 18 {
        return Vec::new();
    }
    let mut lines: Vec<Line<'static>> =
        BANNER.iter().map(|l| Line::from(Span::styled(*l, Style::new().fg(accent())))).collect();
    lines.push(Line::raw(""));
    lines
}

/// Draw a block of pre-laid-out lines centred on the area, with no border —
/// the startup screens are a splash, not a dialog, so the art keeps its own
/// internal alignment and the whole block is centred as one unit.
fn render_centered_block(frame: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    // A zero-area frame has no centre, and clamp(1, 0) panics. Terminals
    // never report zero columns, but the browser build's first frame arrives
    // before the DOM grid has measured itself, and it is exactly that.
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let width = width.clamp(1, area.width);
    let height = (lines.len() as u16).clamp(1, area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
}

/// Longest a wrapped message line gets. Keeps a long error from stretching
/// the centered block out to the full terminal, which would shove the banner
/// and the fields off to the left every time something went wrong.
const MESSAGE_WIDTH: usize = 66;

/// The trailing status shared by every connect screen, wrapped rather than
/// cut: these messages end in the instruction ("Enter again to send it
/// anyway"), so losing the tail loses the point.
fn connect_message(app: &App, area: Rect) -> Vec<Line<'static>> {
    let Some(message) = app.message.as_ref() else {
        return Vec::new();
    };
    let style = match message.kind {
        MessageKind::Error => Style::new().fg(Color::Red),
        MessageKind::Info => Style::new().fg(dim()),
    };
    wrap(&message.text, MESSAGE_WIDTH.min(area.width as usize))
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect()
}

/// Break text on word boundaries. A word longer than `width` (a URL, say) is
/// left whole and allowed to overhang rather than being chopped mid-token.
///
/// Budgeted in display columns, like everything else that decides how much
/// fits: counting chars sized a CJK message at up to half the room it needs,
/// and the Paragraph that draws it does not wrap, so the overhang was simply
/// cut — taking the instruction these messages end with.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let fits = width_of(&line) + 1 + width_of(word) <= width;
        if line.is_empty() {
            line.push_str(word);
        } else if fits {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Where the main screen's rows go.
pub(crate) struct Regions {
    pub header: Rect,
    pub body: Rect,
    pub rule: Rect,
    pub transport: Rect,
    pub footer: Rect,
}

/// Split the screen into its rows.
///
/// A pure function of the area rather than something the drawing remembers,
/// so a mouse click can ask where a thing was drawn without the render path
/// having to keep notes for it. Both callers get their answer from here, so
/// there is nothing to drift.
/// Where the full-screen view's rows go, under its one-line title.
///
/// Body, then a rule, then the transport band and the key hints along the
/// foot. The band spans the full width rather than sitting in a column, so
/// the bar is long enough to read as a position rather than a stepper.
pub(crate) struct NowRegions {
    pub body: Rect,
    pub rule: Rect,
    pub gauge: Rect,
    pub keys: Rect,
}

/// Rows the full-screen transport gives the waveform *above* the bar.
///
/// The bar row is the other half of a shape mirrored about the line between
/// them, which is what makes it read as a waveform rather than as a bar
/// chart. One, because the halves have to match and the lower one is the
/// scrubber. The compact browser transport gets none of this: its two rows
/// are already an eighth of the list on an 80x24 terminal, and it keeps the
/// single-row version.
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

/// The numbers `[display]` sets that are about *how much fits* rather than
/// which characters to draw with. Resolved once at startup, the same shape
/// as [`THEME`] and [`GLYPHS`] — and for the extra reason that the layout
/// has to be a pure function of the area plus settings that cannot move
/// mid-frame, or [`progress_area`] and the drawing would disagree.
static SIZING: OnceLock<Sizing> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizing {
    pub mirror_min_height: u16,
    pub miller_columns: usize,
}

impl Default for Sizing {
    fn default() -> Self {
        Sizing {
            mirror_min_height: crate::config::DEFAULT_MIRROR_MIN_HEIGHT,
            miller_columns: crate::config::DEFAULT_MILLER_COLUMNS,
        }
    }
}

impl Sizing {
    pub fn from_prefs(prefs: &crate::config::DisplayPrefs) -> Self {
        Sizing {
            mirror_min_height: prefs.mirror_min_height,
            miller_columns: prefs.miller_columns,
        }
    }
}

pub fn set_sizing(sizing: Sizing) {
    let _ = SIZING.set(sizing);
}

fn sizing() -> &'static Sizing {
    SIZING.get_or_init(Sizing::default)
}

/// The same, for the browser screen — where the row does come out of the
/// list, so a short terminal keeps its single bar.
///
/// A pure function of the height and of settings fixed at startup, which is
/// the rule that matters: [`progress_area`] works this out again *after* the
/// frame is drawn, to answer a click, and the two must never disagree.
fn transport_wave_rows(height: u16) -> u16 {
    if height >= sizing().mirror_min_height { wave_half_rows() } else { 0 }
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

/// Where the progress bar is, whichever screen is up — so a click and the
/// drawing cannot disagree about it.
pub(crate) fn progress_area(app: &App, area: Rect) -> Rect {
    if app.fullscreen {
        now_regions(area).gauge
    } else {
        let transport = regions(area).transport;
        // The first row of the band names what is playing; everything under
        // it is the bar, which on a tall terminal is the mirrored pair.
        Rect { y: transport.y + 1, height: transport.height.saturating_sub(1), ..transport }
    }
}

pub(crate) fn regions(area: Rect) -> Regions {
    let [header, body, rule, transport, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        // What is playing, the bar, and — where the terminal has rows to
        // spare — the mirrored half above it.
        Constraint::Length(2 + transport_wave_rows(area.height)),
        Constraint::Length(1),
    ])
    .areas(area);
    Regions { header, body, rule, transport, footer }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    if !app.connected {
        render_connect(frame, area, app);
        return;
    }

    // A view, not an overlay: it replaces the frame rather than sitting on top
    // of it, but input stays in normal mode, so every transport key still does
    // what it does everywhere else.
    if app.fullscreen {
        render_now_playing(frame, area, app);
        // The genre chooser is the one modal left, and it belongs to a tab
        // of this view — so it has to be drawn here as well. It was only
        // ever drawn on the browser screen, which meant the old `D` panel
        // opened invisibly over the full-screen view and ate the keyboard.
        if app.dj_panel.genres.is_some() {
            render_genre_picker(frame, area, app);
        }
        if app.show_help {
            render_help(frame, area, app);
        }
        return;
    }

    // The transport carries two lines of content and no border: the browser and
    // queue panes already close with a rule right above it, so a box of its own
    // would only spend two rows drawing a line next to a line. On an 80x24
    // terminal those two rows are an eighth of the list.
    let Regions { header, body, rule, transport, footer } = regions(area);

    render_header(frame, header, app);
    render_columns(frame, body, app);
    frame.render_widget(
        Paragraph::new(Span::styled("\u{2500}".repeat(rule.width as usize), Style::new().fg(dim()))),
        rule,
    );
    render_transport(frame, transport, app);
    render_footer(frame, footer, app);

    if app.dj_panel.genres.is_some() {
        render_genre_picker(frame, area, app);
    }
    if app.sonic_playlist_name.is_some() {
        render_playlist_prompt(frame, area, app);
    }
    if app.show_help {
        render_help(frame, area, app);
    }
}

/// One labelled key. Brackets mark the one that is on, not just colour — the
/// labels already pad with a space either side, so it costs nothing in width
/// and survives a terminal that has taken the colour away.
fn shortcut(label: String, on: bool) -> Vec<Span<'static>> {
    let (left, right) = if on { ("[", "]") } else { (" ", " ") };
    let style = if on {
        Style::new().fg(accent()).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    vec![Span::styled(left, style), Span::styled(label, style), Span::styled(right, style)]
}

/// The numbered tabs, then the two keys that are not tabs.
///
/// `0` and `Tab` reach a whole screen and a whole column, and nothing else
/// said so — they were in the help and nowhere a user would look first. The
/// tabs are navigation and the extras are a reminder, so the extras shorten
/// and then go entirely rather than push a tab off the row.
fn header_shortcuts(app: &App, width: u16) -> Line<'static> {
    // Only the tabs this server can serve, so the numbers run 1..n with no
    // gaps and none of them lead somewhere empty.
    let open = app.tab_index();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, tab) in app.tabs().iter().enumerate() {
        spans.extend(shortcut(format!("{}:{}", i + 1, tab.title()), i == open));
    }
    let tabs_width = line_width(&spans);
    // Room kept back for the server label: enough that it can still say who
    // as well as where. The scheme is the part worth giving up for a reminder
    // of a key — the username is not, and reserving only the bare host made
    // the header worse at 96 columns than it was at 80.
    let labels = server_labels(app);
    let keep = labels
        .get(labels.len().saturating_sub(2))
        .or_else(|| labels.last())
        .map_or(0, |label| width_of(label) + 1);

    for (now, queue) in [("0:Now Playing", "Tab:Queue"), ("0:Now", "Tab:Queue")] {
        // A gap wider than the one between tabs, so the two families read as
        // two families: these are a view and a toggle, not more tabs.
        let mut extra = vec![Span::raw("  ")];
        extra.extend(shortcut(now.to_string(), false));
        // The queue column really is either open or not, so it says which.
        extra.extend(shortcut(queue.to_string(), app.queue_column));
        if tabs_width + line_width(&extra) + keep <= width as usize {
            spans.extend(extra);
            break;
        }
    }
    Line::from(spans)
}

fn line_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| width_of(span.content.as_ref())).sum()
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    // The shortcuts are how you move around, so they get the space they need
    // and the server label takes what is left — a fixed split truncated
    // "4:Search" clean off at 80 columns.
    let strip = header_shortcuts(app, area.width);
    let strip_width = (strip.width() as u16).min(area.width);
    let [tabs_area, server_area] =
        Layout::horizontal([Constraint::Length(strip_width), Constraint::Min(0)]).areas(area);

    frame.render_widget(Paragraph::new(strip), tabs_area);

    // One column short of the room it has, so the label never ends up flush
    // against the shortcuts when the two happen to fill the row exactly.
    let who = server_label(app, server_area.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(who, Style::new().fg(dim()))))
            .alignment(Alignment::Right),
        server_area,
    );
}

/// The most informative form of "who and where" that fits, down to nothing.
/// Dropping the scheme first keeps the host visible on a narrow terminal.
/// What the server label can shrink to, longest first. The last is the least
/// it will settle for, and the header keeps that much back.
fn server_labels(app: &App) -> Vec<String> {
    // A tunnel session shows its identity, not the loopback port it happens
    // to be riding on today — and how it is currently reached, because
    // "direct" and "through a relay on another continent" sound different
    // and the listener deserves to know which one they are hearing.
    let shown = match app.tunnel_path {
        Some(path) => format!("{} · {}", app.server_display(), path.label()),
        None => app.server_display(),
    };
    let host = shown
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string();
    let mut candidates = Vec::new();
    if let Some(user) = &app.session.username {
        candidates.push(format!("{user}@{shown}"));
        candidates.push(format!("{user}@{host}"));
    } else {
        candidates.push(shown.clone());
    }
    candidates.push(host);
    candidates
}

fn server_label(app: &App, width: usize) -> String {
    server_labels(app)
        .into_iter()
        .find(|candidate| candidate.chars().count() <= width)
        .unwrap_or_default()
}

/// The browser, as columns: the listings you came through, then the one you
/// are in, then the queue if it is open.
///
/// Borderless, with a rule between columns. Two boxes side by side spent four
/// columns on vertical lines and read as two separate windows; a rule says the
/// same thing in one column and reads as one surface.
fn render_columns(frame: &mut Frame, area: Rect, app: &mut App) {
    // Where you are, once, across the whole width.
    //
    // Every column used to carry its own title, which for the file browser
    // meant three headers that were prefixes of one another and a middle one
    // squeezed down to `/library/10 Ft. G…`. Said once at full width it
    // cannot be truncated, and the columns get on with being lists.
    let [location, area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    render_location(frame, location, app);

    let widths = column_widths(area.width, app.pane().trail.len(), app.queue_column);
    let constraints: Vec<Constraint> =
        widths.iter().map(|w| Constraint::Length(*w)).collect();
    let areas = Layout::horizontal(constraints).split(area);

    // The trail is drawn from the innermost outwards, so when there is only
    // room for one it is the one you just came out of.
    let shown_trail = widths.len() - 1 - usize::from(app.queue_column);
    let skip = app.pane().trail.len().saturating_sub(shown_trail);
    for (slot, step) in app.pane().trail.iter().skip(skip).enumerate() {
        render_trail_column(frame, areas[slot], step);
    }

    let current = shown_trail;
    if app.queue_column {
        divider(frame, areas[current]);
        render_queue_column(frame, areas[current + 1], app);
    }
    render_current_column(frame, areas[current], app);
}

/// The line above the columns: where you are on the left, and what the queue
/// holds on the right when it is open — that column's own heading, kept on
/// this row so every list starts level with every other.
fn render_location(frame: &mut Frame, area: Rect, app: &mut App) {
    let area = inset(area);
    let queue = app.queue_column.then(|| queue_title(&app.queue).trim().to_string());
    let queue_width = queue.as_deref().map_or(0, |t| width_of(t) as u16 + 2);
    // The path is the point, so it is fed first and the queue summary drops
    // off rather than crowd it.
    let queue_width = queue_width.min(area.width.saturating_sub(20));
    let [here, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(queue_width)]).areas(area);

    let focused = app.focus == Focus::Browser;
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(browser_title(app).trim(), here.width as usize),
            if focused {
                Style::new().add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(dim())
            },
        )),
        here,
    );
    if let Some(queue) = queue.filter(|_| queue_width > 0) {
        frame.render_widget(
            Paragraph::new(Span::styled(
                fit(&queue, right.width as usize),
                if focused {
                    Style::new().fg(dim())
                } else {
                    Style::new().add_modifier(Modifier::BOLD)
                },
            ))
            .alignment(Alignment::Right),
            right,
        );
    }
}

/// How wide each column gets. The one you are in is the one you are reading,
/// so it is fed first; the context columns take what is left, and drop off
/// entirely rather than shrink to the point of saying nothing.
///
/// How many of them there can be is [`Sizing::miller_columns`], counting the
/// current one — a ceiling, not a promise. Width decides the rest: the trail
/// is filled innermost-first, so a narrow terminal shows the folder you just
/// came out of and a wide one shows the way back to the top. It used to be
/// pinned at two whatever the width, which on a wide terminal threw away the
/// context it plainly had room for (reported live, browsing by artist).
fn column_widths(total: u16, trail: usize, queue: bool) -> Vec<u16> {
    const CURRENT_MIN: u16 = 28;
    const QUEUE_MIN: u16 = 22;
    /// What a context column needs to be worth drawing, and as wide as one
    /// is worth letting get — past this it is spending width on a listing
    /// nobody is reading.
    const TRAIL_WIDTH: u16 = 20;
    const TRAIL_WIDE: u16 = 32;

    let queue_width = if queue { QUEUE_MIN.min(total.saturating_sub(CURRENT_MIN)) } else { 0 };
    let mut spare = total.saturating_sub(CURRENT_MIN + queue_width);
    // The column you are in is one of them, so the trail may have the rest.
    let cap = sizing().miller_columns.saturating_sub(1);
    let mut shown = 0;
    while shown < trail.min(cap) && spare >= TRAIL_WIDTH {
        spare -= TRAIL_WIDTH;
        shown += 1;
    }

    // What is left once everyone has their minimum. Half of it widens the
    // columns you came through — at twenty they clip most album names, and
    // a column of "Free Instrumentals, Vol…" is context you cannot read —
    // and the column you are actually in keeps the rest.
    let mut widths = vec![TRAIL_WIDTH; shown];
    if shown > 0 && spare > 0 {
        let share = (spare / 2) / shown as u16;
        let grow = share.min(TRAIL_WIDE - TRAIL_WIDTH);
        for width in &mut widths {
            *width += grow;
        }
    }

    let used: u16 = widths.iter().sum();
    widths.push(total.saturating_sub(used + queue_width));
    if queue {
        widths.push(queue_width);
    }
    widths
}

/// Force a style across a whole row, spans included.
///
/// `Line::patch_style` sets the line's own style and leaves the spans alone,
/// and a span that sets its own colour — every folder, every right-flushed
/// duration — paints over it at render time. So a trail column asking to be
/// drawn quietly still came out in full folder colour, and a highlight meant
/// to cover the row would have shown those colours through it.
fn restyle(mut line: Line<'static>, style: Style) -> Line<'static> {
    for span in &mut line.spans {
        span.style = span.style.patch(style);
    }
    line.patch_style(style)
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

/// A column you came through. Never focused, so it is drawn quietly, with the
/// row you took marked rather than a cursor.
fn render_trail_column(frame: &mut Frame, area: Rect, step: &crate::tui::app::Trail) {
    let inner = inset(area);
    let width = area.width.saturating_sub(2) as usize;
    let mut state = ListState::default();
    state.select(Some(step.chosen));
    let (window, mut shown) = visible_rows(&mut state, step.entries.len(), inner.height);
    let items: Vec<ListItem> = step.entries[window.clone()]
        .iter()
        .enumerate()
        .map(|(row, entry)| {
            let line = entry_line(entry, width, None);
            // The row you came through keeps a bar of its own, so a chain of
            // columns reads as the path it is: this artist, then that album,
            // then the track you are on. Quieter than the cursor's bar, and
            // without its symbol, so which column has the keys is still plain.
            let style = if window.start + row == step.chosen {
                Style::new().fg(dim()).add_modifier(Modifier::REVERSED)
            } else {
                Style::new().fg(dim())
            };
            ListItem::new(restyle(line, style))
        })
        .collect();
    frame.render_stateful_widget(List::new(items), inner, &mut shown);
    divider(frame, area);
}

/// The column the cursor is in.
fn render_current_column(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner = inset(area);
    let content = inner.width.saturating_sub(CURSOR.len() as u16) as usize;
    let len = app.pane().entries.len();
    let (window, mut shown) =
        visible_rows(&mut app.pane_for_mut(app.tab).state, len, inner.height);
    let playing = app.now_playing.as_ref().map(|track| track.filepath.as_str());
    let items: Vec<ListItem> = app.pane().entries[window.clone()]
        .iter()
        .map(|entry| ListItem::new(entry_line(entry, content, playing)))
        .collect();
    // A filter never hides the way out, so a pane with nothing left in it is
    // one row rather than none. Both are "nothing to see", and both want the
    // hint — just not drawn over the row that is there. (A pane that small is
    // never scrolled, so the window's row count is the pane's.)
    let rows = items.len();
    let nothing = app.pane().entries.iter().all(|entry| matches!(entry, Entry::Parent));
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol(CURSOR);
    frame.render_stateful_widget(list, inner, &mut shown);

    if nothing && (rows as u16) < inner.height {
        frame.render_widget(
            Paragraph::new(Span::styled(empty_hint(app), Style::new().fg(dim()))),
            Rect { y: inner.y + rows as u16, height: 1, ..inner },
        );
    }
}

/// The queue, as the last column. It is the end of the same chain the other
/// columns walk -- artist, album, track, queued -- so it belongs on the right
/// of them rather than in a pane of its own.
fn render_queue_column(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner = inset(area);
    if app.queue.items.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("'a' queues a track", Style::new().fg(dim())))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let (window, mut shown) =
        visible_rows(&mut app.queue.state, app.queue.items.len(), inner.height);
    let current = app.queue.current;
    let items: Vec<ListItem> = app.queue.items[window.clone()]
        .iter()
        .enumerate()
        .map(|(row, track)| {
            let playing = Some(window.start + row) == current;
            let style = if playing { Style::new().fg(accent()) } else { Style::new() };
            let marker = if playing { format!("{} ", glyphs().playing) } else { "  ".to_string() };
            ListItem::new(Line::from(Span::styled(
                fit(&format!("{marker}{}", track.display_name()), inner.width as usize),
                style,
            )))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        inner,
        &mut shown,
    );
}

/// A column's writable area: one space in from each edge, so rows never sit
/// against the rule on either side.
fn inset(area: Rect) -> Rect {
    Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area }
}

/// The rule down a column's right edge, which is what separates it from the
/// next one now that nothing is boxed.
fn divider(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Block::default().borders(Borders::RIGHT).border_style(Style::new().fg(dim())),
        area,
    );
}

/// What an empty column should say. "Nothing here" and "not here yet"
/// look identical on screen and mean opposite things, so the answer turns
/// on whether a request is still out.
pub(crate) fn empty_hint(app: &App) -> String {
    if app.pane().loading {
        return format!("{} loading\u{2026}", spinner_frame(app));
    }
    // "(empty directory)" under a filter that matched nothing is a lie about
    // the directory.
    let filter = app.pane().filter.trim();
    if !filter.is_empty() {
        return format!("nothing here matches {filter:?}");
    }
    match app.tab {
        Tab::Files => "(empty directory)",
        // Inside an opened playlist the pane is that playlist's tracks;
        // "(no playlists)" there would deny the ones sitting in the list.
        Tab::Library => match app.library_node() {
            LibraryNode::Playlists => "(no playlists)",
            LibraryNode::Playlist(_) => "(empty playlist)",
            _ => "(nothing here)",
        },
        Tab::Search => "type a query and press Enter",
        Tab::Discover => "(nothing similar)",
        Tab::SonicPath => "(nothing to set here)",
        Tab::Settings => "(no settings here yet)",
    }
    .to_string()
}

pub(crate) fn browser_title(app: &App) -> String {
    match app.tab {
        Tab::Files => {
            if app.path.is_empty() {
                " Libraries ".to_string()
            } else {
                format!(" /{} ", app.path)
            }
        }
        Tab::Library => match app.library_node() {
            LibraryNode::Root => " Library ".to_string(),
            LibraryNode::Artists => " Artists ".to_string(),
            LibraryNode::Artist(artist) => format!(" Artist: {artist} "),
            LibraryNode::Albums => " Albums ".to_string(),
            LibraryNode::Album { name, .. } => format!(" Album: {name} "),
            LibraryNode::Genres => " Genres ".to_string(),
            LibraryNode::Genre(genre) => format!(" Genre: {genre} "),
            LibraryNode::Recent => " Recently Added ".to_string(),
            LibraryNode::Playlists => " Playlists ".to_string(),
            LibraryNode::Playlist(name) => format!(" Playlist: {name} "),
        },
        Tab::Search => {
            let query = if app.query.is_empty() { "…" } else { &app.query };
            if app.editing_query {
                return format!(" Search: {query}▏");
            }
            match app.search_node() {
                SearchNode::Root => match &app.search_summary {
                    Some(summary) => format!(" Search: {query} — {summary} "),
                    None => format!(" Search: {query} "),
                },
                SearchNode::Class(class) => format!(" {} ", class.title()),
                SearchNode::Library(LibraryNode::Artist(artist)) => format!(" Artist: {artist} "),
                SearchNode::Library(LibraryNode::Album { name, .. }) => format!(" Album: {name} "),
                SearchNode::Library(_) => format!(" Search: {query} "),
            }
        }
        // Every Discover view hangs off one track, so every title names it —
        // a list of neighbours means nothing without saying neighbours of what.
        Tab::Discover => {
            let seed = app
                .discover_seed
                .as_ref()
                .map_or_else(|| "nothing yet".to_string(), Track::display_name);
            match app.discover_node() {
                DiscoverNode::Root => " Discover · look around from… ".to_string(),
                DiscoverNode::Mode => format!(" Discover · from {seed} "),
                DiscoverNode::Tracks => format!(" Songs similar to {seed} "),
                DiscoverNode::Artists => {
                    let artist = app
                        .discover_seed
                        .as_ref()
                        .and_then(|t| t.metadata.artist.clone())
                        .unwrap_or_else(|| seed.clone());
                    format!(" Artists like {artist} ")
                }
                DiscoverNode::Artist(artist) => format!(" Ways into {artist} "),
            }
        }
        // The title carries what the rows cannot: which end is being chosen,
        // or — once a path exists — the two songs it runs between, which is
        // the one fact the results rows never repeat.
        Tab::SonicPath => match app.sonic_node() {
            SonicNode::Side(side) => format!(" Sonic Path · {} ", side.label()),
            SonicNode::Root if app.sonic.view == SonicView::Results => {
                let end = |track: Option<&Track>| {
                    track.map(Track::display_name).unwrap_or_else(|| "?".to_string())
                };
                format!(
                    " {} \u{2192} {} ",
                    end(app.sonic.start.as_ref()),
                    end(app.sonic.end.as_ref())
                )
            }
            SonicNode::Root => " Sonic Path ".to_string(),
        },
        Tab::Settings => match app.settings_node() {
            SettingsNode::Root => " Settings ".to_string(),
            SettingsNode::Crossfade => " Settings · Crossfade ".to_string(),
        },
    }
}

/// One browser row, laid out to `width` columns.
///
/// `playing` is the filepath of the track on the speakers, if any. It is
/// marked in colour rather than with a glyph: the cursor already owns the left
/// gutter, and a row can be both at once — every other player in this class
/// draws selection and playback on separate channels for exactly that reason.
fn entry_line(entry: &Entry, width: usize, playing: Option<&str>) -> Line<'static> {
    match entry {
        Entry::Parent => Line::from(Span::styled("..", Style::new().fg(dim()))),
        // Colour but not bold. A folder listing is mostly folders and the
        // library is nothing but nodes, so bolding them turns a whole pane
        // into emphasis -- which leaves the row that is actually playing
        // nothing to stand out against. The trailing slash carries the same
        // meaning without any styling at all.
        Entry::Dir { label, .. } => {
            Line::from(Span::styled(format!("{label}/"), Style::new().fg(folder())))
        }
        Entry::Node { label, .. } => {
            Line::from(Span::styled(label.clone(), Style::new().fg(folder())))
        }
        // The count is flushed right, the same column the durations make, so
        // the menu reads down the numbers.
        Entry::Search { label, detail, .. } => {
            let count_width = width_of(detail);
            let name = fit(label, width.saturating_sub(count_width + 1));
            let gap = width.saturating_sub(width_of(&name) + count_width).max(1);
            Line::from(vec![
                Span::styled(name, Style::new().fg(folder())),
                Span::raw(" ".repeat(gap)),
                Span::styled(detail.clone(), Style::new().fg(dim())),
            ])
        }
        // Elided rather than left for the terminal to chop: in a narrow trail
        // column an unclipped detail ends mid-word, and a lone letter against
        // the rule reads as a rendering fault rather than as elision.
        Entry::Discover { label, detail, .. } => {
            let name = fit(label, width);
            let room = width.saturating_sub(width_of(&name) + 3);
            let mut spans = vec![Span::styled(name, Style::new().fg(folder()))];
            if room > 1 {
                spans.push(Span::styled(
                    format!("   {}", fit(detail, room)),
                    Style::new().fg(dim()),
                ));
            }
            Line::from(spans)
        }
        Entry::Setting { label, detail, .. } => {
            let name = fit(label, width);
            let room = width.saturating_sub(width_of(&name) + 3);
            let mut spans = vec![Span::styled(name, Style::new().fg(folder()))];
            if room > 1 {
                spans.push(Span::styled(
                    format!("   {}", fit(detail, room)),
                    Style::new().fg(dim()),
                ));
            }
            Line::from(spans)
        }
        // The same two-column shape as a Setting row, with one difference:
        // the rows that *do* something rather than hold a value are drawn in
        // the accent, so the actions read as buttons and not as more settings.
        Entry::Sonic { label, detail, row } => {
            let style = match row {
                // A sentence about how the plot went, not a control.
                SonicRow::Status => Style::new().fg(dim()),
                SonicRow::Build
                | SonicRow::Play
                | SonicRow::QueueAll
                | SonicRow::SavePlaylist
                | SonicRow::Regenerate
                | SonicRow::StartOver
                | SonicRow::UsePlaying
                | SonicRow::PickFromLibrary => Style::new().fg(accent()),
                _ => Style::new().fg(folder()),
            };
            let name = fit(label, width);
            let room = width.saturating_sub(width_of(&name) + 3);
            let mut spans = vec![Span::styled(name, style)];
            if room > 1 {
                spans.push(Span::styled(
                    format!("   {}", fit(detail, room)),
                    Style::new().fg(dim()),
                ));
            }
            Line::from(spans)
        }
        Entry::Track { label, track } => {
            // Bold as well as coloured, so the playing row stays findable
            // where colour has been taken away — NO_COLOR, a 16-colour
            // terminal, or eyes that don't separate these two hues.
            let style = if playing.is_some_and(|path| path == track.filepath) {
                Style::new().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let Some(duration) = track.metadata.duration else {
                return Line::from(Span::styled(label.clone(), style));
            };
            // Flushed to the right edge so the times form a column: a list of
            // tracks is read down the length, not across the titles.
            let time = fmt_duration(duration);
            let time_width = width_of(&time);
            let title = fit(label, width.saturating_sub(time_width + 1));
            let gap = width.saturating_sub(width_of(&title) + time_width).max(1);
            Line::from(vec![
                Span::styled(title, style),
                Span::raw(" ".repeat(gap)),
                Span::styled(time, Style::new().fg(dim())),
            ])
        }
    }
}

/// How many, and how long. A queue that Auto-DJ or a journey built is a set
/// rather than a list, and "how long is this" is the question a set raises.
fn queue_title(queue: &Queue) -> String {
    let total: f64 = queue.items.iter().filter_map(|track| track.metadata.duration).sum();
    if total > 0.0 {
        format!(" Queue ({}) · {} ", queue.items.len(), fmt_span(total))
    } else {
        format!(" Queue ({}) ", queue.items.len())
    }
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

fn render_now_playing(frame: &mut Frame, area: Rect, app: &mut App) {
    // No box. The browser page draws its columns against a rule rather than
    // inside borders, and a full-screen view framed in a rectangle it does not
    // need was the odd one out -- it also spent two rows and two columns
    // drawing the edge of a thing that already fills the screen.
    frame.render_widget(Clear, area);
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(" Now Playing", Style::new().fg(dim()))),
        Rect { height: 1, ..inner },
    );
    let inner = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };

    let NowRegions { body, rule, gauge: gauge_area, keys: keys_area } = now_regions(area);

    // The tab strip gets the width it needs and the facts take what is left,
    // down to a floor where the labelled rows stop fitting. Splitting down the
    // middle instead truncated the last tab to "Vis" on an 88-column terminal.
    let strip = tab_strip_width(app);
    let left_width = inner
        .width
        .saturating_sub(strip + 2)
        .clamp(FACTS_MIN_WIDTH, FACTS_MAX_WIDTH)
        .min(inner.width);
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
    frame.render_widget(
        Paragraph::new(now_playing_card(app, facts_inner.width as usize)),
        facts_inner,
    );
    render_now_panel(frame, panel_area, app);

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

    // The keys that work here, and the modes -- there is no footer down here
    // to carry either, and going full screen should not quietly hide whether
    // shuffle is on.
    let modes = mode_readout(app, false);
    let [left, right] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length((width_of(&modes) as u16).min(keys_area.width)),
    ])
    .areas(keys_area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit(now_keys_hint(app), left.width as usize),
            Style::new().fg(dim()),
        )),
        left,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(dim()))).alignment(Alignment::Right),
        right,
    );
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
/// bug; naming the current one and pointing at the others does not.
fn tab_strip(app: &App, width: u16) -> Line<'static> {
    let current = app.now_tab();
    let active = Style::new().fg(accent()).add_modifier(Modifier::BOLD);
    let rest = Style::new();

    let tabs = app.now_tabs();
    let numbered = |tab: NowTab| {
        let at = tabs.iter().position(|t| *t == tab).unwrap_or(0);
        format!("{}:{}", at + 1, tab.title())
    };

    if tab_strip_width(app) > width {
        return Line::from(vec![
            Span::styled("‹ ", Style::new().fg(dim())),
            Span::styled(numbered(current), active),
            Span::styled(" ›", Style::new().fg(dim())),
        ]);
    }

    let mut spans = Vec::new();
    for tab in tabs.iter().copied() {
        if !spans.is_empty() {
            spans.push(Span::raw(TAB_GAP));
        }
        // Brackets, not just colour. Colour is the first thing a terminal
        // takes away — NO_COLOR is a standard crossterm honours, and plenty of
        // people are running 16 colours or can't tell two hues apart. Which
        // tab you are on is not something the UI can afford to whisper. The
        // inactive ones are padded to the same width so the strip does not
        // shuffle as the selection moves.
        let (open, close) = if tab == current { ("[", "]") } else { (" ", " ") };
        let style = if tab == current { active } else { rest };
        spans.push(Span::styled(open, style));
        spans.push(Span::styled(numbered(tab), style));
        spans.push(Span::styled(close, style));
    }
    Line::from(spans)
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

/// The key hints along the foot. Only what does something on the tab in front
/// of you: offering "Enter play" while the lyrics are up is a small lie.
fn now_keys_hint(app: &App) -> &'static str {
    // The numbers are the navigation on every one of these, which is the
    // point: the tab you can get stuck on and the tab you cannot must not
    // have different ways out.
    match app.now_tab() {
        NowTab::Queue => "1-5 tab   ↑↓ list   Enter play   d remove   0 back",
        NowTab::AutoDj => "1-5 tab   ↑↓ choose   ←→ adjust   Enter set   0 back",
        NowTab::Discover => "1-5 tab   ↑↓ list   Enter play   a queue   0 back",
        NowTab::Visualizer if app.viz.mode.plots_samples() => {
            "1-5 tab   v mode   . dots   0 back"
        }
        NowTab::Visualizer => "1-5 tab   v mode   0 back",
        _ => "1-5 tab   ↑↓ scroll   0 back",
    }
}

/// The tab strip, and whichever tab is open under it.
fn render_now_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    if area.width < 12 || area.height < 3 {
        return;
    }
    let area = Block::default().padding(Padding::horizontal(1)).inner(area);
    let [strip, rule, content] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
            .areas(area);

    frame.render_widget(Paragraph::new(tab_strip(app, strip.width)), strip);
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
        frame.render_widget(Paragraph::new(canvas.into_lines()), picture);
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
    let mut state = ListState::default().with_selected(Some(app.now_scroll));
    let (window, mut visible) = visible_rows(&mut state, shown.tracks.len(), rest.height);
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
    let kbps = meta.bitrate.filter(|b| *b > 0).map(|b| (b as f64 / 1000.0).round() as u64);
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
    if let Some(rate) = meta.sample_rate.filter(|r| *r > 0) {
        let khz = f64::from(rate) / 1000.0;
        // 44.1 and 48 both want to look right, so the decimal only appears
        // when there is something after it.
        parts.push(if (khz.fract() * 10.0).round() == 0.0 {
            format!("{khz:.0} kHz")
        } else {
            format!("{khz:.1} kHz")
        });
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

fn fmt_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = bytes as f64 / MB;
    if mb >= 100.0 {
        format!("{mb:.0} MB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
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
                fit("0 goes back to the browser", width),
                Style::new().fg(dim()),
            )),
        ];
    };

    let meta = &track.metadata;
    let mut lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            fit(meta.display_title().unwrap_or_else(|| track.file_name()), width),
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
        fact_row(&mut lines, "Size", plain(fmt_bytes(size)));
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
fn eighths(amplitude: u8) -> usize {
    (usize::from(amplitude) * 8 / 256).min(8)
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

fn render_transport(frame: &mut Frame, area: Rect, app: &App) {
    // The band is however many rows `regions` gave it: the name, then the
    // bar, and above the bar the mirrored half when there was room. Split
    // off the name and let the rest fall to the pair, so this cannot
    // disagree with the layout about how tall it is.
    let [title_area, band] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let [wave_area, gauge_area] =
        Layout::vertical([Constraint::Length(band.height.saturating_sub(1)), Constraint::Length(1)])
            .areas(band);
    let mirrored = wave_area.height > 0;

    let (label, style) = match (&app.now_playing, app.audio_available) {
        (_, false) => ("audio device unavailable".to_string(), Style::new().fg(Color::Red)),
        (Some(track), _) => {
            let (_, state) = transport_state(app);
            (
                format!("{} · {}", track.display_name(), state),
                Style::new().add_modifier(Modifier::BOLD),
            )
        }
        (None, _) => ("nothing playing".to_string(), Style::new().fg(dim())),
    };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), title_area);

    if mirrored {
        frame.render_widget(
            Paragraph::new(waveform_top_line(app, wave_area.width as usize)),
            wave_area,
        );
    }
    // Hovering is asked about the whole band, so the pointer lights the bar
    // from either row of the pair.
    frame.render_widget(
        Paragraph::new(progress_line(app, gauge_area.width as usize, hovering(app, band), mirrored)),
        gauge_area,
    );
}

/// What the footer says about a narrowed list. The count is the useful part:
/// it says how much is hidden, which is the question a short list raises.
fn filter_readout(app: &App) -> String {
    let pane = app.pane();
    let caret = if app.filtering { glyphs().caret } else { "" };
    let mut out = format!("filter: {}{caret}", pane.filter);
    if !pane.filter.trim().is_empty() {
        let (shown, total) = pane.counts();
        out.push_str(&format!("   {shown} of {total}"));
    }
    if !app.filtering {
        out.push_str("   (f to change)");
    }
    out
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    let full = mode_readout(app, false);
    // Abbreviate rather than let the mode readout crowd the message off the
    // line entirely.
    let compact = mode_readout(app, true);
    let modes = if area.width as usize >= width_of(&full) + 28 { full } else { compact };
    let modes_width = (width_of(&modes) as u16).min(area.width);

    let [message_area, modes_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(modes_width)]).areas(area);

    // An armed picker outranks everything: Enter has stopped meaning play,
    // and nothing else on screen would say so. The webapp floats a banner
    // over the library for the same reason.
    let message = if let Some(who) = app.capture {
        Span::styled(
            fit(
                &format!(
                    "picking the {} song — Enter on any track · Esc cancel",
                    who.shout()
                ),
                message_area.width as usize,
            ),
            Style::new().fg(accent()).add_modifier(Modifier::BOLD),
        )
    // A filter outranks both the message and the hint while one is on: it is
    // the explanation for a list that is shorter than it should be, and
    // leaving that unsaid is how a filtered pane gets mistaken for an empty
    // library.
    } else if app.filtering || !app.pane().filter.is_empty() {
        Span::styled(
            fit(&filter_readout(app), message_area.width as usize),
            Style::new().fg(accent()),
        )
    } else {
        match &app.message {
            Some(m) => Span::styled(
                fit(&m.text, message_area.width as usize),
                match m.kind {
                    MessageKind::Error => Style::new().fg(Color::Red),
                    MessageKind::Info => Style::new().fg(dim()),
                },
            ),
            None => Span::styled(
                fit(idle_hint(app), message_area.width as usize),
                Style::new().fg(dim()),
            ),
        }
    };
    frame.render_widget(Paragraph::new(Line::from(message)), message_area);
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(dim()))).alignment(Alignment::Right),
        modes_area,
    );
}

/// What the footer says when there is no message to show.
///
/// Tab-specific, because the key you want next depends on where you are: with
/// search results on screen the question is how to run another one, and `/`
/// being in the help screen is no use to someone who does not know to look.
fn idle_hint(app: &App) -> &'static str {
    match app.tab {
        Tab::Search if app.search_hits.is_some() => {
            "/ new search   f filter   ? help   q quit"
        }
        _ => "f filter   ? help   q quit",
    }
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
    if app.autodj != AutoDjMode::Off {
        parts.push(format!("dj {}", app.autodj.label()));
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

fn render_connect(frame: &mut Frame, area: Rect, app: &App) {
    // Reconnecting from a saved session: don't flash a login form (and a
    // password field) at someone who never asked to sign in.
    if app.connecting && !app.connect.submitting {
        return render_connecting(frame, area, app);
    }
    match app.connect.stage {
        ConnectStage::Choosing => render_connect_choice(frame, area, app),
        ConnectStage::QuickConnect => render_connect_quick(frame, area, app),
        ConnectStage::Direct => render_connect_direct(frame, area, app),
    }
}

/// Step one: how do you want to reach the server?
fn render_connect_choice(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = banner_lines(area);
    lines.push(Line::from("How do you want to connect?"));
    lines.push(Line::raw(""));

    // Size the name column from the longest name so the descriptions line up
    // however many methods there are, with a gutter between the two.
    let name_column = CONNECT_METHODS
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0)
        + 6;

    for (i, (name, blurb)) in CONNECT_METHODS.iter().enumerate() {
        let selected = app.connect.choice == i;
        let style = if selected {
            Style::new().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{} {name:<width$}",
                    if selected { ">" } else { " " },
                    width = name_column
                ),
                style,
            ),
            Span::styled((*blurb).to_string(), Style::new().fg(dim())),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · Enter continue · Ctrl+C quits",
        Style::new().fg(dim()),
    )));
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

/// Step two, direct branch: address and credentials.
fn render_connect_direct(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = banner_lines(area);

    let field = |index: usize, label: &str, value: String, hint: &str| -> Line<'static> {
        let focused = app.connect.field == index;
        let style = if focused {
            Style::new().fg(accent()).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        let cursor = |on: bool| Span::styled(if on { "▏" } else { "" }, style);
        let mut spans =
            vec![Span::styled(format!("{} {label:<10}", if focused { ">" } else { " " }), style)];
        if value.is_empty() {
            // Nothing typed yet: the cursor sits where typing starts and the
            // hint shows the shape of what goes there.
            spans.push(cursor(focused));
            spans.push(Span::styled(hint.to_string(), Style::new().fg(dim())));
        } else {
            spans.push(Span::raw(value));
            spans.push(cursor(focused));
        }
        Line::from(spans)
    };

    lines.push(field(0, "Server", app.connect.server.clone(), "http://192.168.1.10:3000"));
    lines.push(field(1, "Username", app.connect.username.clone(), ""));
    lines.push(field(
        2,
        "Password",
        "•".repeat(app.connect.password.chars().count()),
        "",
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Leave the username empty for a server in public mode.",
        Style::new().fg(dim()),
    )));
    lines.push(Line::from(Span::styled(
        "Tab/↑↓ switch fields · Enter connects · Esc back",
        Style::new().fg(dim()),
    )));
    if app.connecting {
        lines.push(Line::from(Span::styled("connecting…", Style::new().fg(accent()))));
    }
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

/// Step two, Quick Connect branch: pick a server found on the network, or
/// paste a pairing code to reach one anywhere.
fn render_connect_quick(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = banner_lines(area);
    let form = &app.connect;

    lines.push(Line::from(Span::styled("Servers on your network", Style::new().fg(dim()))));

    if form.found.is_empty() {
        lines.push(Line::from(Span::styled(
            if form.searching { "  searching…" } else { "  none found" },
            Style::new().fg(dim()),
        )));
    } else {
        // Line the columns up the way the method chooser does — in display
        // columns, since `{:<width$}` pads by chars and a name of wide
        // characters would push its own URL out of line with the rest.
        let name_column =
            form.found.iter().map(|server| width_of(&server.name)).max().unwrap_or(0) + 2;
        for (i, server) in form.found.iter().enumerate() {
            let selected = form.row == i;
            let style = if selected {
                Style::new().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let pad = " ".repeat(name_column.saturating_sub(width_of(&server.name)));
            let mut spans = vec![
                Span::styled(
                    format!("{} {}{pad}", if selected { ">" } else { " " }, server.name),
                    style,
                ),
                Span::styled(server.base_url.clone(), Style::new().fg(dim())),
            ];
            // Only advertise pairing where the server says it's available.
            if server.quick_connect {
                spans.push(Span::styled("  · pairing available", Style::new().fg(dim())));
            }
            lines.push(Line::from(spans));
        }
        if form.searching {
            lines.push(Line::from(Span::styled("  searching…", Style::new().fg(dim()))));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Or paste a pairing code to reach a server anywhere",
        Style::new().fg(dim()),
    )));

    let selected = form.on_paste_row();
    let code = form.code.trim();
    // The code is a few hundred characters — show enough to recognise it,
    // plus a length so a paste is visibly confirmed.
    let shown = if code.is_empty() {
        Span::styled("paste the code here", Style::new().fg(dim()))
    } else {
        let head: String = code.chars().take(28).collect();
        Span::raw(format!("{head}…  ({} characters)", code.chars().count()))
    };
    lines.push(Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            Style::new().fg(accent()).add_modifier(Modifier::BOLD),
        ),
        shown,
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "A code opens a tunnel — you'll still sign in afterwards.",
        Style::new().fg(dim()),
    )));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · Enter connects · Esc back",
        Style::new().fg(dim()),
    )));
    if app.connecting {
        lines.push(Line::from(Span::styled("connecting…", Style::new().fg(accent()))));
    }
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

fn render_connecting(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = banner_lines(area);
    lines.push(Line::from(Span::styled(
        format!("Connecting to {}…", app.session.server),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled("Ctrl+C to quit", Style::new().fg(dim()))));
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
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
    match row {
        DjRow::Mode => {
            let mut spans = vec![value(app.autodj.label().to_string())];
            // Worth saying out loud: in this mode the pick comes straight
            // from the neighbour list, so the filters below are not consulted.
            if app.autodj == AutoDjMode::Similar {
                spans.push(Span::styled("   filters below don't apply", faint));
            }
            spans
        }
        DjRow::Tightness => {
            if app.dj.sonic_tightness == 0 {
                return vec![value("off".into()), Span::styled("   any track", faint)];
            }
            let filled = (app.dj.sonic_tightness / 10) as usize;
            let bar: String =
                "▓".repeat(filled) + &"░".repeat(10usize.saturating_sub(filled));
            let cosine = crate::dj::sonic_threshold(app.dj.sonic_tightness).unwrap_or(0.0);
            vec![
                value(format!("{bar} {:>3}%", app.dj.sonic_tightness)),
                // The raw number is what the server actually filters on, and
                // seeing it is how the slider stops being a mystery.
                Span::styled(format!("   cosine ≥ {cosine:.2}"), faint),
            ]
        }
        DjRow::Anchor => {
            let (label, what) = match app.dj.sonic_anchor {
                crate::dj::SonicAnchor::Current => ("current", "follows each track"),
                crate::dj::SonicAnchor::Session => ("session", "averages recent picks"),
            };
            vec![value(label.into()), Span::styled(format!("   {what}"), faint)]
        }
        DjRow::Tempo => {
            if app.dj.tempo_tolerance == 0 {
                return vec![value("off".into())];
            }
            vec![
                value(format!("±{}%", app.dj.tempo_tolerance)),
                Span::styled(
                    format!("   widens to ±{}% before giving up", app.dj.tempo_tolerance * 2),
                    faint,
                ),
            ]
        }
        DjRow::Key => {
            let what = match app.dj.key_matching {
                crate::dj::KeyMatching::Off => "any key",
                crate::dj::KeyMatching::Compatible => "the Camelot neighbourhood",
                crate::dj::KeyMatching::Strict => "the same key only",
            };
            vec![
                value(app.dj.key_matching.label().to_string()),
                Span::styled(format!("   {what}"), faint),
            ]
        }
        DjRow::Rating => {
            if app.dj.min_rating == 0 {
                return vec![value("off".into())];
            }
            vec![value(format!("≥ {}", app.dj.min_rating))]
        }
        DjRow::Cooldown => {
            if app.dj.artist_cooldown == 0 {
                return vec![value("off".into())];
            }
            vec![
                value(format!("{} artists", app.dj.artist_cooldown)),
                Span::styled("   recently played, skipped", faint),
            ]
        }
        DjRow::Genres => {
            let mode = app.dj.genre_mode.label().to_string();
            if app.dj.genre_mode == crate::dj::GenreMode::Off {
                return vec![value(mode), Span::styled("   Enter to choose", faint)];
            }
            let chosen = if app.dj.genres.is_empty() {
                "none chosen — Enter to pick".to_string()
            } else {
                app.dj.genres.join(", ")
            };
            let mut spans = vec![value(format!("{mode}  ")), Span::styled(chosen, faint)];
            // The asymmetry bites people: "only these" is a stricter promise
            // than "anything but these", and it drops untagged tracks.
            if app.dj.genre_mode == crate::dj::GenreMode::Whitelist
                && !app.dj.genres.is_empty()
            {
                spans.push(Span::styled("  (untagged excluded)", faint));
            }
            spans
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

fn render_genre_picker(frame: &mut Frame, area: Rect, app: &App) {
    let Some(picker) = app.dj_panel.genres.as_ref() else { return };

    let mut lines: Vec<Line> = Vec::new();
    if picker.loading {
        lines.push(Line::from(Span::styled("  loading genres…", Style::new().fg(dim()))));
    } else if picker.all.is_empty() {
        lines.push(Line::from(Span::styled("  no genres tagged", Style::new().fg(dim()))));
    } else {
        // Keep the highlighted row on screen for long lists.
        let visible = (area.height.saturating_sub(8)) as usize;
        let first = picker.row.saturating_sub(visible.saturating_sub(1));
        for (index, name) in picker.all.iter().enumerate().skip(first).take(visible) {
            let chosen = app.dj.genres.iter().any(|g| g == name);
            let focused = index == picker.row;
            let style = if focused {
                Style::new().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            lines.push(Line::from(Span::styled(
                format!("{}[{}] {name}", if focused { "> " } else { "  " }, if chosen { 'x' } else { ' ' }),
                style,
            )));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ move · Space toggle · Enter done",
        Style::new().fg(dim()),
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(50, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(format!(" Genres · {} ", app.dj.genre_mode.label()));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Naming the playlist a path is about to become. A box rather than a footer
/// prompt: unlike the filter and the search box there is nothing behind it
/// that reacts as you type, so the typing wants somewhere of its own.
fn render_playlist_prompt(frame: &mut Frame, area: Rect, app: &App) {
    let Some(name) = app.sonic_playlist_name.as_ref() else { return };
    let lines = vec![
        Line::raw(""),
        Line::from(vec![Span::raw(format!("  {name}")), Span::styled(glyphs().caret, Style::new().fg(accent()))]),
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {} stops \u{b7} Enter save \u{b7} Esc cancel", app.sonic.stops.len()),
            Style::new().fg(dim()),
        )),
    ];

    let box_area = centered_rect(60, lines.len() as u16 + 2, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(" Save as playlist ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Widths of the two halves of a help entry.
const HELP_KEYS_WIDTH: usize = 11;
const HELP_TEXT_WIDTH: usize = 24;

/// The keys, read straight off the keymap.
///
/// Rendering the real bindings is the point: a hand-written copy of this list
/// drifted from the truth within a day of the last key being added, and a
/// help screen that lies is worse than none.
fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let entries = app.keymap.help_rows();

    // Lay out in as few columns as fit the height, so a tall terminal gets a
    // single readable list and a short one gets two or three.
    let column_width = (HELP_KEYS_WIDTH + HELP_TEXT_WIDTH + 2) as u16;
    let usable_rows = area.height.saturating_sub(4).max(1) as usize;
    let columns = entries.len().div_ceil(usable_rows).max(1);
    let columns = columns.min((area.width / column_width).max(1) as usize);
    let per_column = entries.len().div_ceil(columns);

    let lines: Vec<Line> = (0..per_column)
        .map(|row| {
            let mut spans = Vec::new();
            for column in 0..columns {
                let Some((keys, what)) = entries.get(column * per_column + row) else {
                    break;
                };
                spans.push(Span::styled(
                    format!("  {keys:<HELP_KEYS_WIDTH$}"),
                    Style::new().fg(accent()).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(format!("{what:<HELP_TEXT_WIDTH$}")));
            }
            Line::from(spans)
        })
        .collect();

    let width = (columns as u16 * column_width + 2).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, box_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(" Keys ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
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
    use crate::api::types::{DirEntry, DirListing, FileEntry, Track, TrackMetadata};
    use crate::tui::app::{Action, InputMode};
    use crate::tui::worker::Event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
        };
        // What a real ping does on the way in: the Auto-DJ rows depend on it.
        app.dj_panel.rebuild(app.capabilities);
        app
    }

    fn listing(path: &str, dirs: &[&str], files: &[&str]) -> DirListing {
        DirListing {
            path: path.to_string(),
            directories: dirs.iter().map(|d| DirEntry { name: (*d).to_string() }).collect(),
            files: files
                .iter()
                .map(|f| FileEntry {
                    name: (*f).to_string(),
                    kind: Some("mp3".into()),
                    ..Default::default()
                })
                .collect(),
        }
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
    fn connect_screen_masks_the_password() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.connect.username = "alice".into();
        app.connect.password = "secret".into();

        let text = draw(&mut app);
        assert!(text.contains("alice"));
        // An empty server field shows the shape of what belongs there rather
        // than a guess the user has to delete.
        assert!(text.contains("http://192.168.1.10:3000"), "placeholder shown");
        assert!(text.contains("••••••"), "password is masked");
        assert!(!text.contains("secret"), "password is never drawn in the clear");
    }

    #[test]
    fn a_long_connect_message_wraps_instead_of_losing_its_ending() {
        // The plaintext warning ends in the instruction, so a hard cut at the
        // terminal edge threw away the only actionable part of it.
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::Direct;
        app.connect.server = "http://music.example.com".into();
        app.connect.username = "alice".into();
        app.connect.password = "pw".into();
        app.handle_action(Action::Submit);

        let text = draw(&mut app);
        assert!(text.contains("unencrypted"));
        assert!(text.contains("Enter again to send it anyway"), "the instruction survives");

        // Narrow enough to drop the banner: still wraps, still complete.
        let text = draw_sized(&mut app, 50, 16);
        assert!(text.contains("Enter again to send it anyway"));
        // And it took more rows to say it, rather than running off the edge.
        let rows = text.lines().filter(|line| line.contains("unencrypted")).count()
            + text.lines().filter(|line| line.contains("Enter again")).count();
        assert_eq!(rows, 2, "the message occupies two wrapped rows");
    }

    #[test]
    fn a_remembered_tunnel_with_no_code_left_says_so_where_you_land() {
        // Noticed during #62's smoke round: the error is set while deciding
        // there is nothing to dial, and what shows next is the method
        // chooser. If that screen didn't draw messages, the session would
        // just be gone with no explanation.
        let mut app = App::new(Some("mstream+iroh://endpointabc".into()), None, None);
        let effects = app.start();
        assert!(effects.is_empty(), "there is no code to dial with");

        let text = draw(&mut app);
        assert!(text.contains("Quick Connect"), "the chooser is what follows: {text}");
        // Wrapped over two rows at this width, so the sentence is checked in
        // the halves the screen really shows.
        assert!(text.contains("the pairing code for the last server is gone"), "{text}");
        assert!(text.contains("reconnect"), "and what to do about it: {text}");
    }

    #[test]
    fn wrapping_breaks_on_words_and_keeps_every_one() {
        let wrapped = wrap("the quick brown fox jumps", 10);
        assert_eq!(wrapped, vec!["the quick", "brown fox", "jumps"]);
        assert!(wrapped.iter().all(|line| line.chars().count() <= 10));

        // A word longer than the width is kept whole rather than mangled.
        assert_eq!(wrap("http://a.very.long.host:3000/path is down", 10), vec![
            "http://a.very.long.host:3000/path",
            "is down"
        ]);
        assert!(wrap("", 10).is_empty());
    }

    #[test]
    fn the_first_screen_shows_the_banner_and_asks_how_to_connect() {
        let mut app = App::new(None, None, None);
        let text = draw(&mut app);
        // The wordmark the server prints at boot, not a bordered dialog.
        assert!(text.contains(r"|_| |_| |_|____/"), "banner is drawn");
        assert!(!text.contains("┌"), "no border box on the startup screen");
        assert!(text.contains("How do you want to connect?"));

        // The descriptions form a second column: same start position on every
        // row, with a clear gutter after the longest name.
        let columns: Vec<usize> = text
            .lines()
            .filter_map(|line| {
                let name = line.find("Direct").or_else(|| line.find("Quick Connect"))?;
                let blurb = line.find("server address").or_else(|| line.find("pairing code"))?;
                Some(blurb - name)
            })
            .collect();
        assert_eq!(columns.len(), 2, "both methods are listed");
        assert_eq!(columns[0], columns[1], "descriptions share a column");
        assert!(
            columns[0] >= "Quick Connect".len() + 4,
            "gutter after the longest name, got offset {}",
            columns[0]
        );
        assert!(text.contains("Direct"));
        assert!(text.contains("Quick Connect"));
        assert!(!text.contains("Password"), "credentials come after the choice");
    }

    #[test]
    fn the_quick_connect_screen_confirms_a_pasted_code() {
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        let text = draw(&mut app);
        assert!(text.contains("paste the code here"));
        // Pairing is not login, and the screen has to say so.
        assert!(text.contains("still sign in"));

        app.connect.code = format!("mstr1:{}", "x".repeat(300));
        let text = draw(&mut app);
        assert!(text.contains("306 characters"), "a long paste is visibly confirmed");
    }

    #[test]
    fn the_quick_connect_screen_lists_servers_found_on_the_network() {
        use crate::discovery::DiscoveredServer;
        let mut app = App::new(None, None, None);
        app.connect.stage = ConnectStage::QuickConnect;
        app.connect.searching = true;

        assert!(draw(&mut app).contains("searching…"));

        app.apply_event(crate::tui::worker::Event::ServersDiscovered(vec![
            DiscoveredServer {
                name: "Living Room".into(),
                base_url: "http://192.168.1.71:3999".into(),
                version: Some("6.19.2".into()),
                quick_connect: true,
            },
            DiscoveredServer {
                name: "Attic".into(),
                base_url: "http://192.168.1.9:3000".into(),
                version: None,
                quick_connect: false,
            },
        ]));

        let text = draw(&mut app);
        assert!(text.contains("Servers on your network"));
        assert!(text.contains("Living Room"));
        assert!(text.contains("http://192.168.1.71:3999"));
        assert!(text.contains("Attic"));
        assert!(!text.contains("searching…"), "the spinner clears once results land");

        // Pairing is only advertised where the server said it's available, and
        // only once — the second server didn't advertise it.
        assert_eq!(text.matches("pairing available").count(), 1);
    }

    #[test]
    fn reconnecting_from_a_session_shows_no_login_form() {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.start();

        let text = draw(&mut app);
        assert!(text.contains("Connecting to http://host:3000"));
        assert!(!text.contains("Password"), "no credential prompt when a session exists");
    }

    #[test]
    fn durations_line_up_against_the_right_edge() {
        let track = |name: &str, seconds: f64| Entry::Track {
            label: name.to_string(),
            track: Box::new(Track {
                filepath: format!("lib/{name}"),
                metadata: TrackMetadata { duration: Some(seconds), ..Default::default() },
            }),
        };
        for (name, seconds) in [("short", 61.0), ("a much longer title indeed", 3599.0)] {
            let line = entry_line(&track(name, seconds), 30, None);
            assert_eq!(line.width(), 30, "'{name}' fills the row exactly");
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.ends_with(&fmt_duration(seconds)), "time is flush right: '{text}'");
        }
    }

    #[test]
    fn a_track_with_no_length_still_gets_a_row() {
        let entry = Entry::Track {
            label: "Untagged".into(),
            track: Box::new(Track {
                filepath: "lib/x.mp3".into(),
                metadata: TrackMetadata::default(),
            }),
        };
        let line = entry_line(&entry, 30, None);
        assert_eq!(line.width(), "Untagged".len(), "no padding without a time to pad to");
    }

    #[test]
    fn the_playing_track_is_marked_wherever_it_is_listed() {
        let entry = Entry::Track {
            label: "Trio - Moonlight".into(),
            track: Box::new(Track {
                filepath: "lib/a.mp3".into(),
                metadata: TrackMetadata { duration: Some(200.0), ..Default::default() },
            }),
        };
        // Colour, not a glyph: the cursor owns the gutter and a row can be
        // both selected and playing at once.
        let lit = entry_line(&entry, 40, Some("lib/a.mp3"));
        assert_eq!(lit.spans[0].style.fg, Some(accent()));
        assert!(lit.spans[0].style.add_modifier.contains(Modifier::BOLD));

        let other = entry_line(&entry, 40, Some("lib/b.mp3"));
        assert_eq!(other.spans[0].style.fg, None);
        assert_eq!(lit.width(), other.width(), "marking it does not move the row");
    }

    #[test]
    fn the_queue_title_carries_how_long_as_well_as_how_many() {
        let track = |seconds: Option<f64>| Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata { duration: seconds, ..Default::default() },
        };
        let mut queue = Queue::default();
        assert_eq!(queue_title(&queue), " Queue (0) ");

        queue.replace(vec![track(Some(2400.0)), track(Some(1800.0))]);
        assert_eq!(queue_title(&queue), " Queue (2) · 1h 10m ");

        queue.replace(vec![track(None)]);
        assert_eq!(queue_title(&queue), " Queue (1) ", "no total when nothing knows its length");
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
    fn browser_shows_directories_and_tracks() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/Artist/",
            &["Album"],
            &["song.mp3"],
        ))));

        let text = draw(&mut app);
        assert!(text.contains("/lib/Artist"), "path is in the title");
        assert!(text.contains("Album/"), "directories are marked with a slash");
        assert!(text.contains("song.mp3"));
        assert!(text.contains(".."), "parent entry is offered below the root");
        assert!(text.contains("1:Files"), "tabs are visible");
    }

    #[test]
    fn transport_shows_track_position_and_modes() {
        let mut app = connected_app();
        app.queue.replace(vec![Track {
            filepath: "lib/a.mp3".into(),
            metadata: TrackMetadata {
                title: Some("Moonlight".into()),
                artist: Some("Trio".into()),
                duration: Some(200.0),
                ..Default::default()
            },
        }]);
        app.play_index(0);
        app.status.position = 50.0;
        app.status.duration = 200.0;
        app.status.playing = true;

        let text = draw(&mut app);
        assert!(text.contains("Trio - Moonlight"));
        assert!(text.contains("playing"));
        assert!(text.contains("0:50 / 3:20"), "elapsed and total are shown");
        assert!(text.contains("vol 100%"));
        app.handle_action(Action::CycleFocus);
        let text = draw(&mut app);
        assert!(text.contains(glyphs().playing), "the queue column marks the current track");
        assert!(
            !text.contains("repeat off") && !text.contains("dj off"),
            "modes that are off are not worth the width: {text}"
        );
    }

    #[test]
    fn the_gap_before_a_track_starts_reads_as_starting_not_stopped() {
        let mut app = connected_app();
        app.queue.replace(vec![Track {
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

        let text = draw(&mut app);
        assert!(text.contains("Trio - Moonlight · starting"), "{text}");
        assert!(!text.contains("stopped"), "the word that made it look broken: {text}");
        // The length came from the library, so the bar has its total already.
        assert!(text.contains("0:00 / 3:20"), "{text}");

        // The full-screen view says the same thing.
        app.handle_action(Action::ToggleNowPlaying);
        let text = draw(&mut app);
        assert!(text.contains("starting"), "{text}");
        assert!(!text.contains("stopped"), "{text}");
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
    fn the_now_playing_screen_replaces_the_frame() {
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.status.playing = true;
        app.status.position = 71.0;
        app.status.duration = 209.0;

        assert!(!app.fullscreen);
        app.handle_action(Action::ToggleNowPlaying);
        let text = draw(&mut app);

        assert!(text.contains("Now Playing"), "{text}");
        assert!(text.contains("Bassnectar") && text.contains("Rewind The Track"), "{text}");
        assert!(text.contains("Album   Divergent Spectrum"), "{text}");
        assert!(text.contains("Year    2011"), "{text}");
        assert!(text.contains("1:11 / 3:29"), "the gauge is still there: {text}");
        // The browser and queue are gone -- this is a view, not an overlay.
        assert!(!text.contains("1:Files"), "{text}");
        assert!(!text.contains("Queue ("), "{text}");
        // But the modes come along, since there is no footer to carry them.
        assert!(text.contains("vol 100%"), "{text}");
        assert!(text.contains("0 back"), "{text}");

        app.handle_action(Action::ToggleNowPlaying);
        assert!(draw(&mut app).contains("1:Files"), "and it toggles back");
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
        app.handle_action(Action::ToggleNowPlaying);
        assert_eq!(app.input_mode(), InputMode::Now, "the view claims the keys");

        // The tagged track has no lyrics, so that tab is not in the strip —
        // and the numbers close over the gap rather than leaving a dead key.
        assert_eq!(app.now_tabs(), vec![NowTab::Queue, NowTab::Discover, NowTab::AutoDj, NowTab::Visualizer]);

        let press = |app: &mut App, code| {
            let action = app.keymap.action(key_event(code), InputMode::Now).unwrap();
            app.handle_action(action);
        };
        for (digit, expected) in [
            ('3', NowTab::AutoDj),
            ('1', NowTab::Queue),
            ('4', NowTab::Visualizer),
            ('2', NowTab::Discover),
        ] {
            press(&mut app, KeyCode::Char(digit));
            assert_eq!(app.now_tab(), expected, "{digit} is {expected:?}");
        }

        // Every tab is reachable from Auto-DJ, which is the whole point.
        press(&mut app, KeyCode::Char('3'));
        assert_eq!(app.now_tab(), NowTab::AutoDj);
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.now_tab(), NowTab::Queue, "and back out again");

        // A number past the end is nothing, not a panic — the strip is
        // shorter here than its five slots.
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.now_tab(), NowTab::Queue);

        // Tab and Shift+Tab stay as the way that needs no counting.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.now_tab(), NowTab::Discover);
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.now_tab(), NowTab::Queue);

        // And ←→ no longer move between tabs at all: on Auto-DJ they adjust,
        // everywhere else they are simply free.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.now_tab(), NowTab::Queue, "→ is not navigation any more");
        press(&mut app, KeyCode::Left);
        assert_eq!(app.now_tab(), NowTab::Queue);
    }

    #[test]
    fn the_lyrics_tab_turns_on_the_track_not_the_server() {
        let mut app = connected_app();
        app.handle_action(Action::ToggleNowPlaying);

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
    fn only_the_window_around_the_cursor_is_drawn_and_it_tracks_the_cursor() {
        // The windowing that keeps huge folders cheap has to agree exactly
        // with what List used to show: the cursor visible, the rows around
        // it the right ones, and the window holding still on the way back
        // up rather than re-deriving from the top.
        let names: Vec<String> = (0..100).map(|i| format!("dir-{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &refs, &[]))));
        app.files.state.select(Some(80)); // "dir-79", behind the ".." row

        let text = draw(&mut app);
        assert!(text.contains("> dir-79"), "the cursor row is drawn, with the cursor");
        assert!(!text.contains("dir-09"), "the top of the folder scrolled away");

        for _ in 0..3 {
            app.handle_action(Action::Up);
        }
        let text = draw(&mut app);
        assert!(text.contains("> dir-76"), "the cursor climbs");
        assert!(text.contains("dir-79"), "inside a window that holds still");

        app.handle_action(Action::First);
        let text = draw(&mut app);
        assert!(text.contains(".."), "back at the top, the parent row is the window");
        assert!(!text.contains("dir-79"), "and the deep rows are gone");
    }

    #[test]
    fn the_trail_column_scrolls_to_the_row_you_took() {
        // The trail marks the row you came through; with the fix it also
        // only builds what shows, so the marked row must still be brought
        // into the window when it sits deep in a big folder.
        let names: Vec<String> = (0..100).map(|i| format!("dir-{i:02}")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &refs, &[]))));
        app.files.state.select(Some(80));
        app.handle_action(Action::Activate); // into dir-79
        app.apply_event(Event::Listing(Box::new(listing("/lib/dir-79/", &[], &["a.mp3"]))));

        let text = draw(&mut app);
        assert!(text.contains("dir-79"), "the trail shows the row you took");
        assert!(!text.contains("dir-19"), "not the top of the folder it lives in");
    }

    /// Not a check, a measurement — the audit's throwaway benchmark, kept:
    /// `cargo test --release render_ten_thousand -- --ignored --nocapture`
    #[test]
    #[ignore = "a measurement, not a check; run --release with --nocapture"]
    fn render_ten_thousand_rows_and_time_it() {
        let names: Vec<String> = (0..10_000).map(|i| format!("dir-{i:05}")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &refs, &[]))));
        app.files.state.select(Some(5_000));

        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
        let frames = 200;
        let started = std::time::Instant::now();
        for _ in 0..frames {
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        }
        let per_frame = started.elapsed() / frames;
        println!(">>> {per_frame:?} per frame at 10,000 entries");
    }

    #[test]
    fn the_fullscreen_queue_keeps_the_scroll_its_render_worked_out() {
        // `ListState` is `Copy`, so rendering into a local copy threw the
        // corrected offset away with every frame: the selection stuck to
        // the bottom edge and the list slid underneath it.
        let mut app = connected_app();
        app.queue.replace(
            (0..40)
                .map(|i| Track {
                    filepath: format!("lib/{i:02}.mp3"),
                    metadata: Default::default(),
                })
                .collect(),
        );
        app.handle_action(Action::ToggleNowPlaying);
        app.now_tab = NowTab::Queue;
        app.queue.state.select(Some(39));

        draw_sized(&mut app, 60, 12);
        assert!(app.queue.state.offset() > 0, "the render's scroll correction survives it");
    }

    #[test]
    fn the_view_keeps_the_transport_keys_it_did_not_claim() {
        // The whole reason this is a fall-through and not its own table: play,
        // skip and volume have to go on working, and go on obeying [keys].
        let app = connected_app();
        for (code, expected) in [
            (KeyCode::Char(' '), Action::PlayPause),
            (KeyCode::Char('n'), Action::NextTrack),
            (KeyCode::Char('+'), Action::VolumeUp),
            (KeyCode::Char('?'), Action::ToggleHelp),
        ] {
            assert_eq!(
                app.keymap.action(key_event(code), InputMode::Now),
                Some(expected.clone()),
                "{code:?} should still work in the full-screen view"
            );
        }
        // And the ones it did claim mean something else here than outside.
        assert_eq!(
            app.keymap.action(key_event(KeyCode::Right), InputMode::Normal),
            Some(Action::Activate)
        );
    }

    #[test]
    fn the_auto_dj_tab_is_where_auto_dj_is_changed() {
        // It reported and pointed at a modal behind `D` — a second screen
        // describing one set of values, and one that never drew over this
        // view at all. The tab is the panel now.
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        app.handle_action(Action::ToggleNowPlaying);
        app.autodj = AutoDjMode::BpmKey;
        app.now_tab = NowTab::AutoDj;

        let text = draw(&mut app);
        assert!(text.contains("Mode") && text.contains("tempo+key"), "{text}");
        assert!(text.contains("Tempo window"), "{text}");
        assert!(!text.contains("opens the panel"), "there is no other panel: {text}");

        app.handle_action(Action::NowRight);
        assert_ne!(app.autodj, AutoDjMode::BpmKey, "the row under the cursor moved");
    }

    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn a_theme_colour_can_be_a_name_a_hex_or_an_index() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color(" Bright-Blue "), Some(Color::LightBlue));
        // Both spellings of grey, and both ways of saying the bright half.
        assert_eq!(parse_color("dark-grey"), Some(Color::DarkGray));
        assert_eq!(parse_color("bright_black"), Some(Color::DarkGray));
        assert_eq!(parse_color("light-cyan"), Some(Color::LightCyan));

        assert_eq!(parse_color("#4fc3d6"), Some(Color::Rgb(0x4f, 0xc3, 0xd6)));
        assert_eq!(parse_color("33"), Some(Color::Indexed(33)));

        assert_eq!(parse_color("#4fc3"), None, "a short hex is a mistake, not a colour");
        assert_eq!(parse_color("#gggggg"), None);
        assert_eq!(parse_color("chartreuse"), None);
        assert_eq!(parse_color("300"), None, "past the 256-colour cube");
    }

    #[test]
    fn an_unreadable_theme_costs_that_colour_and_nothing_else() {
        let (theme, warnings) = Theme::from_prefs(&crate::config::ThemePrefs {
            accent: Some("#4fc3d6".into()),
            dim: Some("chartreuse".into()),
            folder: None,
            extra: Default::default(),
        });
        assert_eq!(theme.accent, Color::Rgb(0x4f, 0xc3, 0xd6), "the good one applies");
        assert_eq!(theme.dim, Theme::default().dim, "the bad one keeps the default");
        assert_eq!(theme.folder, Theme::default().folder);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("theme.dim") && warnings[0].contains("chartreuse"));
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
        for set in [RICH, LEGACY].into_iter().filter(|s| s.mirrored) {
            for height in 0..=8usize {
                let complement = 8 - height;
                assert_eq!(
                    set.eighths[height] == set.eighths[8],
                    set.eighths[complement] == set.eighths[0],
                    "height {height} does not mirror in {:?}",
                    set.eighths
                );
            }
        }
    }

    #[test]
    fn the_glyph_set_is_pinned_by_config_and_guessed_otherwise() {
        use crate::config::DisplayPrefs;
        let prefs = |value: &str| DisplayPrefs { glyphs: value.into(), ..Default::default() };

        assert_eq!(Glyphs::from_prefs(&prefs("full")).0, RICH);
        assert_eq!(Glyphs::from_prefs(&prefs("legacy")).0, LEGACY);
        assert_eq!(Glyphs::from_prefs(&prefs("  FULL  ")).0, RICH, "trimmed and case-folded");

        // An unreadable value costs that value and nothing else — the same
        // bargain `[theme]` and `[keys]` make.
        let (set, warnings) = Glyphs::from_prefs(&prefs("fancy"));
        assert_eq!(set, Glyphs::detect());
        assert!(warnings[0].contains("display.glyphs"), "{warnings:?}");

        // And the guess: anything not Windows has these glyphs, and on
        // Windows the question is whether this is Windows Terminal.
        assert_eq!(Glyphs::from_prefs(&prefs("auto")).0, Glyphs::detect());
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
    fn which_tab_is_open_survives_a_terminal_with_no_colour() {
        // Both tab bars said "this one" in colour alone. NO_COLOR is a
        // standard crossterm honours, so that is a state a terminal can
        // simply refuse to show — and which screen you are on is not
        // something the UI can afford to whisper.
        let mut app = connected_app();
        let text = draw(&mut app);
        assert!(text.contains("[1:Files] 2:Library"), "{text}");

        app.handle_action(Action::SelectTab(1));
        let text = draw(&mut app);
        assert!(text.contains(" 1:Files [2:Library]"), "{text}");

        // Bracketing must not shift the row: the titles already padded with a
        // space either side, so the brackets take that space rather than new
        // space, and the header's width calculation is untouched.
        let widths: Vec<usize> =
            [0usize, 1, 2].iter().map(|tab| {
                let mut app = connected_app();
                app.handle_action(Action::SelectTab(*tab));
                draw(&mut app).lines().next().unwrap().trim_end().len()
            }).collect();
        assert_eq!(widths[0], widths[1], "the header does not move as tabs change");
        assert_eq!(widths[1], widths[2]);
    }

    #[test]
    fn a_folder_listing_is_coloured_but_not_shouted() {
        // Every row in the library is a node, so bolding them all marked
        // nothing and left the playing row nothing to stand out against.
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Album"], &["a.mp3"]))));

        let dir = entry_line(
            &Entry::Dir { label: "Album".into(), path: "lib/Album".into() },
            40,
            None,
        );
        assert_eq!(dir.spans[0].style.fg, Some(folder()));
        assert!(
            !dir.spans[0].style.add_modifier.contains(Modifier::BOLD),
            "colour is enough for a whole pane of these"
        );
        // The slash says it too, which is what carries when colour cannot.
        let text: String = dir.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.ends_with('/'), "{text}");
    }

    #[test]
    fn the_strip_gives_up_its_names_before_it_truncates_one() {
        let mut app = connected_app();
        let mut track = tagged_track();
        track.metadata.has_lyrics = true;
        app.now_playing = Some(track);
        app.handle_action(Action::ToggleNowPlaying);

        // Wide enough: every tab is named, and wearing the number that
        // reaches it.
        let wide = draw_sized(&mut app, 110, 16);
        assert!(
            wide.contains("[1:Queue]  2:Lyrics   3:Discover   4:Auto-DJ   5:Visualizer "),
            "{wide}"
        );

        // Not wide enough: the one you are on, and arrows for the rest. A tab
        // chopped to "Vis" reads as a bug; this reads as a choice. The number
        // survives the squeeze — it is the way out of wherever you are.
        let narrow = draw_sized(&mut app, 64, 16);
        assert!(narrow.contains("‹ 1:Queue ›"), "{narrow}");
        assert!(!narrow.contains("Discover"), "{narrow}");
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
        app.handle_action(Action::ToggleNowPlaying);
        let text = draw(&mut app);
        assert!(text.contains("nothing playing"), "{text}");
        assert!(text.contains("0 goes back to the browser"), "{text}");
    }

    #[test]
    fn the_rule_joins_the_divider_it_is_drawn_under() {
        // The junction exists to make the facts column's right border meet
        // the rule below it. Off by one it is a dangling join: a ┴ under
        // nothing, and the border stopping at a blank cell.
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.handle_action(Action::ToggleNowPlaying);

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
    fn a_wrapped_message_is_measured_in_the_columns_it_will_occupy() {
        // wrap() budgeted by chars while everything around it measures
        // display columns, so a CJK message was sized at up to twice its
        // budget and the un-wrapped Paragraph cut the tail off — losing the
        // instruction these messages end with.
        // Two words of 26 columns each. By char count they are 13 apiece, so
        // both used to be packed onto one 30-column line — 53 columns drawn
        // into 30, with the second half clipped away by the Paragraph.
        let cjk = "这个服务器使用的是明文连接 再按一次回车键仍然发送密码";
        // 26 + a space + 26 is 53, so anything under that needs two lines.
        for width in [30usize, 40, 52] {
            let lines = wrap(cjk, width);
            for line in &lines {
                assert!(
                    width_of(line) <= width,
                    "{:?} is {} columns wide, budget was {width}",
                    line,
                    width_of(line)
                );
            }
            assert_eq!(lines.len(), 2, "at {width} columns the two words need two lines");
        }
        // With room for both they share a line, as they always would have.
        assert_eq!(wrap(cjk, 53).len(), 1);
        assert_eq!(wrap(cjk, MESSAGE_WIDTH).len(), 1);

        // A word longer than the budget still goes out whole rather than
        // being chopped mid-token, which is the rule that keeps a URL
        // copyable — the documented exception, in columns as it was in chars.
        assert_eq!(wrap("http://a-very-long-host.example/path", 10).len(), 1);
        assert_eq!(wrap("这个服务器使用的是明文连接", 10).len(), 1);

        // And an ordinary ASCII message wraps where it always did.
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
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
        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.handle_action(Action::ToggleNowPlaying);
        for (w, h) in [(20u16, 4u16), (32, 6), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        }
    }

    #[test]
    fn help_still_opens_over_the_now_playing_screen() {
        let mut app = connected_app();
        app.handle_action(Action::ToggleNowPlaying);
        app.handle_action(Action::ToggleHelp);
        let text = draw(&mut app);
        assert!(text.contains("Keys"), "{text}");
        assert!(text.contains("full-screen now playing"), "and it lists the new key: {text}");
    }

    #[test]
    fn the_footer_names_only_the_modes_that_are_on() {
        let mut app = connected_app();
        assert_eq!(mode_readout(&app, false), "vol 100%", "all defaults says nothing extra");

        app.handle_action(Action::ToggleRepeat);
        app.handle_action(Action::ToggleShuffle);
        app.autodj = AutoDjMode::Similar;
        let full = mode_readout(&app, false);
        assert!(full.contains("repeat all"), "{full}");
        assert!(full.contains("shuffle"), "{full}");
        assert!(full.contains("dj similar"), "{full}");

        let compact = mode_readout(&app, true);
        assert!(width_of(&compact) < width_of(&full), "the short form is shorter: {compact}");
        assert!(compact.contains("rpt all") && compact.contains("shf"), "{compact}");
    }

    #[test]
    fn unknown_duration_renders_a_placeholder_total() {
        let mut app = connected_app();
        app.now_playing = Some(Track {
            filepath: "lib/live.mp3".into(),
            metadata: TrackMetadata::default(),
        });
        app.status.position = 12.0;
        app.status.duration = 0.0; // live transcode: length not known yet

        let text = draw(&mut app);
        assert!(text.contains("0:12 / --:--"));
    }

    #[test]
    fn library_tab_renders_the_mode_menu_and_drill_down_titles() {
        use crate::tui::worker::{LibraryData, LibraryNode};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));

        let text = draw(&mut app);
        assert!(text.contains("2:Library"), "the tab is offered");
        assert!(text.contains("Artists"));
        assert!(text.contains("Genres"));
        assert!(text.contains("Recently Added"));

        app.library_stack.restart();

        app.library_stack.enter(LibraryNode::Genre("Ambient".into()));
        app.apply_event(crate::tui::worker::Event::Library {
            dest: Tab::Library,
            node: LibraryNode::Genre("Ambient".into()),
            data: LibraryData::Tracks(vec![Track {
                filepath: "lib/a.mp3".into(),
                metadata: TrackMetadata {
                    title: Some("Drift".into()),
                    duration: Some(95.0),
                    ..Default::default()
                },
            }]),
        });
        let text = draw(&mut app);
        assert!(text.contains("Genre: Ambient"), "the title tracks the drill-down");
        assert!(text.contains("Drift"));
        assert!(text.contains("1:35"));
    }

    #[test]
    fn the_discover_tab_names_what_it_is_working_from() {
        // A list of neighbours means nothing without saying neighbours of
        // what, so every view in the tab carries the seed in its title.
        let mut app = connected_app();
        app.queue.replace(vec![Track {
            filepath: "lib/seed.mp3".into(),
            metadata: TrackMetadata {
                artist: Some("Seed Artist".into()),
                title: Some("Seed Song".into()),
                ..Default::default()
            },
        }]);
        app.play_index(0);

        let discover = app.tabs().iter().position(|t| *t == Tab::Discover).unwrap();
        app.handle_action(Action::SelectTab(discover));
        let text = draw(&mut app);
        assert!(text.contains("4:Discover"));
        // The tab opens on the question, not on a guess about the answer.
        assert!(text.contains("look around from"), "got:\n{text}");
        assert!(text.contains("What's playing") && text.contains("Choose a song"));

        // Take what's playing, and the title names it from then on.
        app.handle_action(Action::Activate);
        let text = draw(&mut app);
        assert!(text.contains("Seed Artist - Seed Song"), "the title names the seed:\n{text}");
        assert!(text.contains("Similar tracks"));
        assert!(text.contains("like Seed Artist"), "and the artist row names the artist");

        // Artist rows carry how close, how many ways in, and what it sounds
        // like — the three things that decide whether to open one.
        app.discover.state.select(Some(2));
        app.handle_action(Action::Activate);
        app.apply_event(crate::tui::worker::Event::Discover {
            node: crate::tui::worker::DiscoverNode::Artists,
            data: crate::tui::worker::DiscoverData::Artists(vec![
                crate::api::types::SimilarArtist {
                    artist: "Near One".into(),
                    similarity: 0.94,
                    analyzed_count: 20,
                    genre_tags: vec!["Electronic---Dubstep".into()],
                    entry_points: vec![Track {
                        filepath: "lib/way.mp3".into(),
                        metadata: Default::default(),
                    }],
                },
            ]),
            note: None,
            dest: crate::tui::worker::DiscoverDest::Browser,
            seed: String::new(),
        });
        let text = draw(&mut app);
        assert!(text.contains("Artists like Seed Artist"), "got:\n{text}");
        assert!(text.contains("Near One"));
        assert!(text.contains("0.94"));
        assert!(text.contains("1 way in"), "singular, not '1 ways in':\n{text}");
        assert!(text.contains("Dubstep") && !text.contains("Electronic---"));
    }

    #[test]
    fn a_sonic_path_shows_the_arc_it_would_queue() {
        let mut app = connected_app();
        app.queue.replace(vec![Track {
            filepath: "lib/start.mp3".into(),
            metadata: TrackMetadata {
                artist: Some("First".into()),
                title: Some("Departure".into()),
                ..Default::default()
            },
        }]);
        app.play_index(0);
        app.files.set(vec![crate::tui::app::Entry::Track {
            label: "end".into(),
            track: Box::new(Track {
                filepath: "lib/end.mp3".into(),
                metadata: TrackMetadata {
                    artist: Some("Last".into()),
                    title: Some("Arrival".into()),
                    ..Default::default()
                },
            }),
        }]);
        app.files.state.select(Some(0));
        app.handle_action(Action::StartJourney);

        // The tab names both ends while the plot is in flight, so the wait
        // is not a blank screen.
        let text = draw_sized(&mut app, 100, 30);
        assert!(text.contains("First - Departure"), "got:\n{text}");
        assert!(text.contains("Last - Arrival"), "got:\n{text}");

        app.apply_event(crate::tui::worker::Event::Journey {
            stops: vec![
                crate::api::types::JourneyStop {
                    filepath: "lib/start.mp3".into(),
                    t: 0.0,
                    similarity: 1.0,
                    metadata: TrackMetadata {
                        artist: Some("First".into()),
                        title: Some("Departure".into()),
                        ..Default::default()
                    },
                },
                crate::api::types::JourneyStop {
                    filepath: "lib/mid.mp3".into(),
                    t: 0.5,
                    similarity: 0.8,
                    metadata: TrackMetadata {
                        artist: Some("Middle".into()),
                        title: Some("Somewhere".into()),
                        ..Default::default()
                    },
                },
                crate::api::types::JourneyStop {
                    filepath: "lib/end.mp3".into(),
                    t: 1.0,
                    similarity: 1.0,
                    metadata: TrackMetadata {
                        artist: Some("Last".into()),
                        title: Some("Arrival".into()),
                        ..Default::default()
                    },
                },
            ],
            note: None,
            length: app.sonic.length,
        });

        let text = draw_sized(&mut app, 100, 30);
        assert!(text.contains("First - Departure"));
        assert!(text.contains("Middle - Somewhere"));
        assert!(text.contains("Last - Arrival"));
        // The arc position is what makes it a path rather than a playlist.
        assert!(text.contains("0%") && text.contains("50%") && text.contains("100%"), "got:\n{text}");
        // And what can be done with it, in the webapp's own order.
        assert!(text.contains("Play the path"), "got:\n{text}");
        assert!(text.contains("Queue all") && text.contains("Save as playlist"));
        assert!(text.contains("Start over"));
    }

    #[test]
    fn a_sonic_path_survives_a_small_terminal() {
        let mut app = connected_app();
        app.queue.replace(vec![Track { filepath: "a".into(), metadata: Default::default() }]);
        app.play_index(0);
        app.files.set(vec![crate::tui::app::Entry::Track {
            label: "b".into(),
            track: Box::new(Track { filepath: "b".into(), metadata: Default::default() }),
        }]);
        app.files.state.select(Some(0));
        app.handle_action(Action::StartJourney);
        app.apply_event(crate::tui::worker::Event::Journey {
            stops: (0..32)
                .map(|i| crate::api::types::JourneyStop {
                    filepath: format!("lib/{i}.mp3"),
                    t: f64::from(i) / 31.0,
                    similarity: 0.9,
                    metadata: Default::default(),
                })
                .collect(),
            note: None,
            length: app.sonic.length,
        });
        // A 32-stop path in a short terminal scrolls like any other list.
        draw_sized(&mut app, 60, 20);
        draw_sized(&mut app, 24, 9);
    }

    /// The Auto-DJ tab, as its own screen rather than as a modal over the
    /// browser: `0` opens the full-screen view, then the tab.
    fn on_the_dj_tab(app: &mut App) {
        app.handle_action(Action::ToggleNowPlaying);
        while app.now_tab() != NowTab::AutoDj {
            app.handle_action(Action::NowTabNext);
        }
    }

    #[test]
    fn the_dj_tab_shows_each_setting_with_what_it_means() {
        let mut app = connected_app();
        app.dj.sonic_tightness = 60;
        on_the_dj_tab(&mut app);
        let text = draw_sized(&mut app, 100, 34);

        assert!(text.contains("Auto-DJ"));
        for row in ["Mode", "Sonic pool", "Anchor", "Tempo window", "Key matching", "Genres"] {
            assert!(text.contains(row), "missing {row} in:\n{text}");
        }
        // The slider shows the raw threshold it maps to, so the number the
        // server filters on is never a mystery.
        assert!(text.contains("cosine ≥ 0.63"), "got:\n{text}");
        assert!(text.contains("60%"));
        // And the tempo row says what the fallback widens to.
        assert!(text.contains("±6%") && text.contains("±12%"), "got:\n{text}");
        assert!(text.contains("Sample") && text.contains("Enter to preview"), "got:\n{text}");
        // ←→ mean adjust here, so the hint has to say what moves between
        // tabs — the same thing it says on every other one.
        assert!(text.contains("1-5 tab") && text.contains("←→ adjust"), "got:\n{text}");
    }

    #[test]
    fn the_dj_tab_hides_the_sonic_rows_without_an_index() {
        let mut app = connected_app();
        app.capabilities = Default::default();
        app.dj_panel = Default::default();
        on_the_dj_tab(&mut app);
        let text = draw_sized(&mut app, 100, 34);
        assert!(!text.contains("Sonic pool"), "nothing promises a pool that can't exist");
        assert!(!text.contains("Anchor"));
        assert!(text.contains("Tempo window"), "the rest of the tab is still there");
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
    fn the_help_screen_is_the_keymap() {
        // The whole point of deriving it: a key that works but isn't listed,
        // or a listed key that no longer works, is now unrepresentable. The
        // hand-written version drifted within a day of the last key landing.
        let mut app = connected_app();
        app.handle_action(Action::ToggleHelp);
        let text = draw_sized(&mut app, 120, 44);
        assert!(text.contains("Keys"), "the overlay drew at all");

        for (keys, what) in app.keymap.help_rows() {
            // Key and description on one line, exactly as laid out — not
            // merely present somewhere on screen.
            let row = format!("  {keys:<HELP_KEYS_WIDTH$}{what}");
            assert!(text.contains(&row), "help is missing {row:?}\n{text}");
        }
    }

    #[test]
    fn the_help_follows_a_rebound_key() {
        // The point of holding the keymap on the app: rebinding `n` has to
        // change what the help says, not just what the key does.
        let mut app = connected_app();
        app = app.with_keys(
            &[("next-track".to_string(), vec!["b".to_string()])].into_iter().collect(),
        );
        app.handle_action(Action::ToggleHelp);
        let text = draw_sized(&mut app, 120, 44);
        assert!(text.contains("  b          next track"), "got:\n{text}");
        assert!(!text.contains("  n          next track"));
    }

    #[test]
    fn every_help_entry_fits_its_column() {
        // The columns are laid out by padding to a fixed width, so one
        // over-long entry shunts everything to its right out of line — which
        // is exactly what a 25-character description did.
        for (keys, what) in crate::tui::keymap::Keymap::default().help_rows() {
            assert!(
                keys.chars().count() <= HELP_KEYS_WIDTH,
                "keys {keys:?} are wider than the column ({HELP_KEYS_WIDTH})"
            );
            assert!(
                what.chars().count() <= HELP_TEXT_WIDTH,
                "{what:?} is wider than the column ({HELP_TEXT_WIDTH})"
            );
        }
    }

    #[test]
    fn the_help_folds_into_columns_when_it_cannot_fit_in_one() {
        let mut app = connected_app();
        app.handle_action(Action::ToggleHelp);

        // Tall and wide: one column, and the last binding is still on screen.
        let tall = draw_sized(&mut app, 120, 44);
        assert!(tall.contains("quit"));

        // Short: the same content has to fold rather than fall off the end.
        let short = draw_sized(&mut app, 120, 24);
        assert!(short.contains("quit"), "nothing is lost when it folds:\n{short}");
        assert!(short.contains("move down"), "including the first row");
        for line in short.lines() {
            assert!(line.chars().count() <= 120);
        }
    }

    #[test]
    fn empty_panes_explain_themselves() {
        let mut app = connected_app();
        // The queue is a column you open now, not a pane that is always there.
        assert!(!draw(&mut app).contains("Queue ("), "closed by default");
        app.handle_action(Action::CycleFocus);
        let text = draw(&mut app);
        assert!(text.contains("Queue (0)"), "{text}");
        assert!(text.contains("'a' queues a track"), "{text}");
    }

    #[test]
    fn the_queue_column_opens_and_closes_on_tab() {
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        assert!(!draw(&mut app).contains("Queue ("));

        app.handle_action(Action::CycleFocus);
        assert!(app.queue_column);
        assert_eq!(app.focus, Focus::Queue, "opening it moves the cursor there too");
        assert!(draw(&mut app).contains("Queue (1)"));

        app.handle_action(Action::CycleFocus);
        assert!(!app.queue_column);
        assert_eq!(app.focus, Focus::Browser, "and closing it hands the cursor back");
    }

    #[test]
    fn the_path_is_said_once_above_the_columns() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist"], &[]))));
        app.handle_action(Action::Activate);
        app.apply_event(Event::Listing(Box::new(listing("/lib/Artist/", &["Album"], &[]))));
        app.handle_action(Action::Activate);
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/Artist/Album/",
            &[],
            &["a.mp3"],
        ))));
        assert_eq!(app.files.trail.len(), 2, "two columns behind");

        let text = draw(&mut app);
        assert_eq!(
            text.matches("/lib/Artist/Album").count(),
            1,
            "every column used to carry its own copy of this: {text}"
        );

        let lines: Vec<&str> = text.lines().collect();
        let path = lines.iter().position(|l| l.contains("/lib/Artist/Album")).unwrap();
        assert!(!lines[path].contains('\u{2502}'), "it runs the full width: {}", lines[path]);
        assert!(lines[path + 1].contains('\u{2502}'), "and the columns start under it");
    }

    #[test]
    fn the_queue_heading_shares_that_line_so_the_lists_start_level() {
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist"], &[]))));
        app.handle_action(Action::CycleFocus);

        let text = draw(&mut app);
        let lines: Vec<&str> = text.lines().collect();
        let path = lines.iter().position(|l| l.contains("/lib")).unwrap();
        assert!(lines[path].contains("Queue (1)"), "{}", lines[path]);
        // The first row under it is a list row in both columns, not a heading.
        assert!(lines[path + 1].contains(".."), "{}", lines[path + 1]);
    }

    #[test]
    fn the_trail_takes_the_width_it_is_given_up_to_the_cap() {
        // Reported live, browsing by artist on a wide terminal: the first
        // column fell off the screen after a couple of levels, with plenty
        // of room left. The count was pinned at two whatever the width.
        let cap = sizing().miller_columns;
        let deep = 8;

        // Wide enough for the lot: the cap is what stops it, and the current
        // column is one of them.
        let wide = column_widths(240, deep, false);
        assert_eq!(wide.len(), cap, "the cap counts the column you are in");
        assert_eq!(wide.iter().sum::<u16>(), 240);
        // Surplus width is shared rather than dumped on the current column:
        // context columns pinned at twenty clip most album names.
        assert!(wide[..cap - 1].iter().all(|w| *w > 20), "the trail grew: {wide:?}");
        assert!(wide[..cap - 1].iter().all(|w| *w <= 32), "but only so far: {wide:?}");
        assert!(*wide.last().unwrap() > wide[0], "and the one being read keeps the most");

        // Narrower: as many as fit, innermost first, and never at the cost
        // of the column being read.
        for total in [48u16, 68, 88, 108] {
            let widths = column_widths(total, deep, false);
            assert!(widths.len() <= cap, "{total} columns wide gave {}", widths.len());
            assert_eq!(widths.iter().sum::<u16>(), total, "every column is accounted for");
            assert!(*widths.last().unwrap() >= 28, "the current column keeps its floor");
        }

        // Shallow: you cannot show more context than there is.
        assert_eq!(column_widths(240, 1, false).len(), 2);
        assert_eq!(column_widths(240, 0, false).len(), 1);

        // The queue is not one of the Miller columns — it is the end of the
        // chain rather than a step along it — so it takes its width from
        // outside the count.
        let with_queue = column_widths(240, deep, true);
        assert_eq!(with_queue.len(), cap + 1);
        assert_eq!(*with_queue.last().unwrap(), 22);
        assert_eq!(with_queue.iter().sum::<u16>(), 240);
    }

    #[test]
    fn the_columns_build_up_as_you_go_in_and_fall_away_as_you_come_out() {
        use crate::tui::worker::{LibraryData, LibraryNode};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));
        app.apply_event(Event::Library {
            dest: Tab::Library,
            node: LibraryNode::Root,
            data: LibraryData::Artists(vec!["Bassnectar".into(), "ill Gates".into()]),
        });
        assert!(app.library.trail.is_empty(), "one column at the top");

        app.handle_action(Action::Down);
        app.handle_action(Action::Activate);
        // Captured on the way in, from what was already on screen — going a
        // level deeper costs one request, not two.
        assert_eq!(app.library.trail.len(), 1);
        assert_eq!(app.library.trail[0].entries.len(), 3, "the menu we came through");
        let text = draw(&mut app);
        assert!(text.contains("Bassnectar"), "the column behind is still drawn: {text}");

        app.handle_action(Action::Back);
        assert!(app.library.trail.is_empty(), "and it falls away coming out");
    }

    fn draw_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The row of the leftmost column whose text starts with `label`.
    fn trail_row(buffer: &ratatui::buffer::Buffer, label: &str) -> Option<u16> {
        (0..buffer.area.height).find(|y| {
            let row: String = (0..20).map(|x| buffer[(x, *y)].symbol()).collect();
            row.trim_start().starts_with(label)
        })
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
    fn the_browser_bar_mirrors_only_where_there_are_rows_to_spare() {
        // The transport's rows come out of the list, so the second one is
        // worth having on a tall terminal and not on a short one. The
        // full-screen view is not part of this bargain — its body is a facts
        // column and a panel, not a list to scroll.
        if !glyphs().mirrored {
            return; // a console font has no mirror to spend a row on
        }
        let floor = crate::config::DEFAULT_MIRROR_MIN_HEIGHT;
        let at = |height: u16| Rect { x: 0, y: 0, width: 80, height };

        let short = regions(at(floor - 1)).transport;
        assert_eq!(short.height, 2, "what is playing, then the bar");

        let tall = regions(at(floor)).transport;
        assert_eq!(tall.height, 3, "and the mirrored half above it");

        // The list keeps everything the transport did not take, either way.
        for height in [floor - 1, floor] {
            let r = regions(at(height));
            assert_eq!(
                r.header.height + r.body.height + r.rule.height + r.transport.height + r.footer.height,
                height,
                "the rows all go somewhere at {height}"
            );
        }
    }

    #[test]
    fn the_click_target_follows_the_bar_up_the_screen() {
        // `progress_area` works the layout out again *after* the frame is
        // drawn, to answer a click. If it and `regions` ever disagreed about
        // how tall the band is, clicks would land on the wrong second.
        let mut app = connected_app();
        let floor = crate::config::DEFAULT_MIRROR_MIN_HEIGHT;

        for height in [floor - 1, floor, floor + 20] {
            let area = Rect { x: 0, y: 0, width: 80, height };
            let transport = regions(area).transport;
            let bar = progress_area(&app, area);
            assert_eq!(bar.y, transport.y + 1, "the name keeps the first row at {height}");
            assert_eq!(
                bar.y + bar.height,
                transport.y + transport.height,
                "and the bar owns the rest at {height}"
            );
        }

        // Whatever the browser screen spends, the full-screen view is
        // unchanged by the height.
        app.handle_action(Action::ToggleNowPlaying);
        for height in [floor - 1, floor] {
            let area = Rect { x: 0, y: 0, width: 80, height };
            assert_eq!(progress_area(&app, area).height, 1 + wave_half_rows());
        }
    }

    #[test]
    fn the_bar_is_where_the_click_handler_looks_for_it() {
        // Both screens draw a progress bar in a different place, and the one
        // function that answers "where" is the one the drawing uses.
        let mut app = connected_app();
        let area = Rect { x: 0, y: 0, width: 80, height: 24 };

        let bar = progress_area(&app, area);
        assert_eq!(bar.height, 1);
        assert_eq!(bar.y, regions(area).transport.y + 1, "the row under what is playing");

        app.handle_action(Action::ToggleNowPlaying);
        let full = progress_area(&app, area);
        // Taller here, and deliberately: the full-screen band is the mirrored
        // waveform plus the scrubber under it, and the whole of it is one
        // control. The click handler is row-agnostic — it takes the column
        // and asks `contains` about the rest — so the extra row is simply a
        // bigger target rather than anything to special-case.
        assert_eq!(full.height, 1 + wave_half_rows());
        assert_eq!(full, now_regions(area).gauge, "and the one the view itself lays out");
        // The full-screen view insets by a column either side; the rows the
        // two land on happen to coincide at some heights, which is why this
        // checks the width rather than the row.
        assert_eq!((full.x, full.width), (bar.x + 1, bar.width - 2));
    }

    #[test]
    fn the_visualizer_names_its_mode_and_v_moves_between_them() {
        use crate::tui::viz::{VIZ_MODES, VizMode};
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.handle_action(Action::ToggleNowPlaying);
        while app.now_tab() != NowTab::Visualizer {
            app.handle_action(Action::NowTabNext);
        }

        // No tap in a test, so the panel says so rather than drawing noise.
        let text = draw(&mut app);
        assert!(text.contains("no audio thread"), "{text}");
        assert!(text.contains("spectrum"), "the mode is named: {text}");
        assert!(text.contains("v mode"), "and the key that changes it: {text}");

        for expected in VIZ_MODES.iter().skip(1).chain(std::iter::once(&VizMode::Bars)) {
            app.handle_action(Action::CycleViz);
            assert_eq!(app.viz.mode, *expected);
            assert!(draw(&mut app).contains(expected.title()));
        }
        // A mode change is not news anyone needs in the browser's footer.
        assert!(app.message.is_none());

        // `.` says which way it is drawing, and only where that means
        // something — a bar chart has no samples to join.
        app.viz.mode = VizMode::Scope;
        let text = draw(&mut app);
        assert!(text.contains("lines") && text.contains(". dots"), "{text}");
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
    fn the_visualizer_only_costs_extra_redraws_while_it_is_showing() {
        let mut app = connected_app();
        assert!(!app.drawing_audio(), "the browser does not need thirty frames a second");

        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.handle_action(Action::ToggleNowPlaying);
        assert!(!app.drawing_audio(), "nor does the queue tab of the now-playing view");

        while app.now_tab() != NowTab::Visualizer {
            app.handle_action(Action::NowTabNext);
        }
        assert!(app.drawing_audio());

        app.handle_action(Action::ToggleNowPlaying);
        assert!(!app.drawing_audio(), "and leaving the view stops it");
    }

    #[test]
    fn the_card_says_what_the_file_is() {
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);
        app.play_index(0);
        app.handle_action(Action::ToggleNowPlaying);

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

    #[test]
    fn a_filtered_list_says_so_and_says_how_much_it_is_hiding() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing(
            "/lib/",
            &["Bassnectar", "Basshunter", "Portishead"],
            &[],
        ))));

        app.handle_action(Action::StartFilter);
        for c in "bass".chars() {
            app.handle_action(Action::Input(c));
        }
        let text = draw(&mut app);
        // The caret says the keys are going here; the count says why the list
        // is short. Both matter more than whatever the footer held before.
        assert!(text.contains(&format!("filter: bass{}", glyphs().caret)), "{text}");
        assert!(text.contains("2 of 3"), "{text}");
        assert!(text.contains("Bassnectar") && text.contains("Basshunter"), "{text}");
        assert!(!text.contains("Portishead"), "the filtered-out row is gone: {text}");

        // Once typing stops the filter is still on, so it still says so.
        app.handle_action(Action::Submit);
        let text = draw(&mut app);
        assert!(text.contains("filter: bass"), "{text}");
        assert!(!text.contains(&format!("filter: bass{}", glyphs().caret)), "the caret goes with the prompt: {text}");
        assert!(text.contains("f to change"), "{text}");
    }

    #[test]
    fn a_filter_that_matches_nothing_says_that_rather_than_looking_empty() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Alpha"], &[]))));

        app.handle_action(Action::StartFilter);
        for c in "zzz".chars() {
            app.handle_action(Action::Input(c));
        }
        let text = draw(&mut app);
        assert!(text.contains("nothing here matches \"zzz\""), "{text}");
        // Drawn under the way out rather than over it — the row that is still
        // there is the one that gets you out of here.
        let lines: Vec<&str> = text.lines().collect();
        let out = lines.iter().position(|l| l.contains("..")).expect("the way out");
        let said = lines.iter().position(|l| l.contains("nothing here matches")).unwrap();
        assert_eq!(said, out + 1, "{text}");
    }

    #[test]
    fn the_row_you_came_through_keeps_a_mark_of_its_own() {
        // A chain of columns should read as the path it is: this artist, then
        // that album. Bold alone barely registered against a column of folders
        // that were all in colour anyway.
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist", "Other"], &[]))));
        app.handle_action(Action::Activate);
        app.apply_event(Event::Listing(Box::new(listing("/lib/Artist/", &["Album"], &[]))));
        assert_eq!(app.files.trail.len(), 1);

        let buffer = draw_buffer(&mut app, 90, 26);
        let came_through = trail_row(&buffer, "Artist/").expect("the row we went in by");
        let passed_over = trail_row(&buffer, "Other/").expect("a row we did not");

        assert!(
            buffer[(2, came_through)].modifier.contains(Modifier::REVERSED),
            "the way in is marked with a bar of its own"
        );
        assert!(
            !buffer[(2, passed_over)].modifier.contains(Modifier::REVERSED),
            "and only that row is"
        );
        assert!(
            buffer[(15, came_through)].modifier.contains(Modifier::REVERSED),
            "the bar runs the width of the column, not the width of the word"
        );
        // The cursor's bar keeps its symbol, so which column has the keys is
        // still plain even with two bars on screen.
        let row: String = (0..20).map(|x| buffer[(x, came_through)].symbol()).collect();
        assert!(!row.contains(CURSOR.trim()), "the trail carries no cursor: {row:?}");

        // The rest of the column is quiet, folders included -- their own
        // colour used to paint straight over the line style asking for dim.
        assert_eq!(buffer[(2, passed_over)].fg, dim(), "a trail column is context, not content");
    }

    #[test]
    fn the_columns_are_still_there_when_you_come_back_to_a_tab() {
        // Leaving a tab and returning put you back several folders deep with
        // nothing behind you: the position was remembered and the context was
        // thrown away. The trail belongs to the pane, not to the app.
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &["Artist"], &[]))));
        app.handle_action(Action::Down);
        app.handle_action(Action::Activate);
        app.apply_event(Event::Listing(Box::new(listing("/lib/Artist/", &["Album"], &[]))));
        assert_eq!(app.files.trail.len(), 1);

        app.handle_action(Action::SelectTab(1));
        app.handle_action(Action::SelectTab(0));
        assert_eq!(app.files.trail.len(), 1, "the way in came back with the place");
        assert!(draw(&mut app).contains("/lib"), "and it is drawn");

        // Each tab keeps its own, so one does not inherit another's columns.
        app.handle_action(Action::SelectTab(1));
        assert!(app.library.trail.is_empty());
    }

    /// Put the cursor inside the Library tab's Playlists node.
    fn on_the_playlists_node(app: &mut App) {
        app.handle_action(Action::SelectTab(1));
        let at =
            app.library.entries.iter().position(|e| e.label() == "Playlists").unwrap();
        app.library.state.select(Some(at));
        app.handle_action(Action::Activate);
    }

    #[test]
    fn a_pane_waiting_on_the_server_says_so_instead_of_saying_it_is_empty() {
        let mut app = connected_app();
        // The bug this exists for: opening Playlists showed "(no playlists)"
        // for as long as the round trip took, which is a different claim.
        on_the_playlists_node(&mut app);
        let waiting = draw(&mut app);
        assert!(waiting.contains("loading…"), "{waiting}");
        assert!(!waiting.contains("(no playlists)"), "{waiting}");

        app.apply_event(Event::Library {
            node: LibraryNode::Playlists,
            dest: Tab::Library,
            data: crate::tui::worker::LibraryData::Playlists(Vec::new()),
        });
        let answered = draw(&mut app);
        assert!(answered.contains("(no playlists)"), "{answered}");
        assert!(!answered.contains("loading…"), "{answered}");
    }

    #[test]
    fn an_empty_playlist_is_not_a_claim_about_the_list() {
        use crate::api::types::PlaylistSummary;
        use crate::tui::worker::LibraryData;
        let mut app = connected_app();
        on_the_playlists_node(&mut app);
        app.apply_event(Event::Library {
            node: LibraryNode::Playlists,
            dest: Tab::Library,
            data: LibraryData::Playlists(vec![PlaylistSummary { name: "phone".into() }]),
        });
        // Open the (empty) playlist: the pane now shows its tracks, and the
        // message must be about them — the list plainly has an entry.
        app.library.state.select(Some(1));
        app.handle_action(Action::Activate);
        app.apply_event(Event::Library {
            node: LibraryNode::Playlist("phone".into()),
            dest: Tab::Library,
            data: LibraryData::Tracks(Vec::new()),
        });
        let text = draw(&mut app);
        assert!(text.contains("(empty playlist)"), "{text}");
        assert!(!text.contains("(no playlists)"), "{text}");
    }

    #[test]
    fn a_zero_area_frame_is_survived_rather_than_centred() {
        // The browser build's first frame arrives before its grid has
        // measured itself: zero columns, zero rows. A terminal never reports
        // that, and clamp(1, 0) in the splash centring panicked (min > max)
        // — which in a tab is the whole player gone at boot.
        let backend = ratatui::backend::TestBackend::new(0, 0);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut app = App::new(Some("http://host:3000".into()), None, None);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
    }

    #[test]
    fn a_failed_request_stops_the_spinner_rather_than_turning_forever() {
        let mut app = connected_app();
        on_the_playlists_node(&mut app);
        app.apply_event(Event::Error("server said no".into()));
        let text = draw(&mut app);
        assert!(!text.contains("loading…"), "{text}");
        assert!(text.contains("(no playlists)"), "{text}");
    }

    #[test]
    fn the_spinner_turns() {
        let mut app = connected_app();
        on_the_playlists_node(&mut app);
        let first = draw(&mut app);
        app.spinner += 1;
        let second = draw(&mut app);
        assert_ne!(first, second, "advancing the tick changes the frame");
    }

    #[test]
    fn search_tab_shows_the_query_and_result_summary() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        for c in "moon".chars() {
            app.handle_action(Action::Input(c));
        }
        assert!(draw(&mut app).contains("Search: moon"));

        app.handle_action(Action::Submit);
        app.apply_event(Event::SearchResults {
            query: "moon".into(),
            results: Box::default(),
        });
        assert!(draw(&mut app).contains("0 matches"));
    }

    #[test]
    fn the_search_menu_says_what_matched_and_how_many() {
        use crate::api::types::{SearchGroup, SearchResults, SearchTrack};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        for c in "moon".chars() {
            app.handle_action(Action::Input(c));
        }
        app.handle_action(Action::Submit);
        let hit = |name: &str| SearchTrack {
            name: name.into(),
            filepath: format!("lib/{name}.mp3"),
            album_art_file: None,
            metadata: TrackMetadata::default(),
        };
        app.apply_event(Event::SearchResults {
            query: "moon".into(),
            results: Box::new(SearchResults {
                artists: vec![
                    SearchGroup { name: "Moon Hooch".into(), album_art_file: None },
                    SearchGroup { name: "Moondog".into(), album_art_file: None },
                ],
                albums: vec![],
                title: vec![hit("Moonlight")],
                files: vec![],
                lyrics: vec![hit("Harvest")],
            }),
        });

        let text = draw(&mut app);
        assert!(text.contains("Artists"), "{text}");
        assert!(text.contains("Lyrics"), "{text}");
        assert!(!text.contains("Albums"), "a class with no hits is not a row: {text}");
        // Counts flush right, the column the durations make elsewhere.
        assert!(text.lines().any(|row| row.contains("Artists") && row.trim_end().ends_with('2')));
    }

    #[test]
    fn the_banner_is_dropped_when_it_would_not_fit() {
        let mut app = App::new(None, None, None);
        let text = draw_sized(&mut app, 40, 12);
        assert!(!text.contains(r"|_| |_| |_|____/"), "art is skipped rather than mangled");
        assert!(text.contains("Direct"), "the choice is still usable");
    }

    #[test]
    fn every_tab_survives_a_narrow_terminal() {
        // Found by walking the flow at 76 columns: a fixed 40-column server
        // label squeezed the tab bar and cut "4:Search" off entirely.
        let mut app = connected_app();
        app.session.username = Some("tester".into());
        for width in [76, 80, 100, 140] {
            let text = draw_sized(&mut app, width, 20);
            for tab in ["1:Files", "2:Library", "3:Search", "4:Discover"] {
                assert!(text.contains(tab), "{tab} missing at {width} columns");
            }
        }
    }

    #[test]
    fn the_queue_key_says_whether_the_column_is_open() {
        let mut app = connected_app();
        app.queue.replace(vec![tagged_track()]);

        let closed = draw_sized(&mut app, 140, 20);
        assert!(closed.contains(" Tab:Queue "), "{}", closed.lines().next().unwrap());

        app.handle_action(Action::CycleFocus);
        let open = draw_sized(&mut app, 140, 20);
        assert!(open.contains("[Tab:Queue]"), "{}", open.lines().next().unwrap());
    }

    #[test]
    fn the_server_label_sheds_detail_before_the_tabs_do() {
        let mut app = connected_app();
        app.session.username = Some("tester".into());

        // Roomy: the whole thing, scheme and all, next to both extras.
        let wide = draw_sized(&mut app, 140, 20);
        assert!(wide.contains("tester@http://host:3000"));
        assert!(wide.contains("0:Now Playing") && wide.contains("Tab:Queue"));

        // Less room: the scheme is the first thing worth giving up, and it
        // goes to a reminder of a key. The username is not up for trade, so
        // the extras shorten before it does. (Widths came back in when
        // Playlists left the strip for the Library tab.)
        let mid = draw_sized(&mut app, 110, 20);
        assert!(mid.contains("tester@host:3000"), "kept who as well as where");
        assert!(mid.contains("0:Now") && mid.contains("Tab:Queue"));
        assert!(!mid.contains("0:Now Playing"), "the long form is what gave way: {mid}");

        // Genuinely tight: the extras go entirely, and every tab is still
        // whole — including the two that only exist on a server with a
        // discovery index, and Settings behind them.
        let narrow = draw_sized(&mut app, 86, 20);
        assert!(!narrow.contains("0:Now"), "a hint is not worth a tab: {narrow}");
        assert!(narrow.contains("tester@host:3000"));
        assert!(narrow.contains("4:Discover"));
        assert!(narrow.contains("5:Sonic Path"));
        assert!(narrow.contains("6:Settings"));

        // A server without a discovery index has neither, so the same label
        // survives in a much narrower terminal.
        let mut plain = connected_app();
        plain.capabilities = Default::default();
        plain.session.username = Some("tester".into());
        let text = draw_sized(&mut plain, 72, 20);
        assert!(!text.contains("Discover"), "no tab for a feature this server lacks");
        assert!(!text.contains("Sonic Path"), "nor for the path it cannot plot");
        assert!(text.contains("4:Settings"), "Settings slides onto the freed number");
        assert!(text.contains("tester@http://host:3000"), "and the freed width shows");
    }

    #[test]
    fn a_long_message_is_elided_not_chopped() {
        let mut app = connected_app();
        app.apply_event(crate::tui::worker::Event::Connected {
            server: "http://averylongservername.example.com:3000".into(),
            id: "http://averylongservername.example.com:3000".into(),
            username: None,
            token: None,
            ping: Box::new(Default::default()),
        });
        let text = draw_sized(&mut app, 76, 20);
        assert!(text.contains('…'), "the cut is marked rather than looking broken");
        // And the mode readout is still there beside it, abbreviated if need be.
        assert!(text.contains("vol 100%") || text.contains("100%"));
    }

    #[test]
    fn renders_in_a_small_terminal_without_panicking() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &["a.mp3"]))));
        for (w, h) in [(20u16, 8u16), (40, 10), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        }
    }
}
