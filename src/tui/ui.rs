//! Rendering. Reads app state, draws widgets — no decisions of its own.

use std::sync::OnceLock;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};

use crate::cmd_library::fmt_duration;

use super::app::{
    App, CONNECT_METHODS, ConnectStage, DjRow, Entry, Focus, MessageKind, NowTab, Queue, Repeat,
    SearchNode, Tab,
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
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let fits = line.chars().count() + 1 + word.chars().count() <= width;
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
        if app.show_help {
            render_help(frame, area, app);
        }
        return;
    }

    // The transport carries two lines of content and no border: the browser and
    // queue panes already close with a rule right above it, so a box of its own
    // would only spend two rows drawing a line next to a line. On an 80x24
    // terminal those two rows are an eighth of the list.
    let [header, body, rule, transport, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, app);
    render_columns(frame, body, app);
    frame.render_widget(
        Paragraph::new(Span::styled("\u{2500}".repeat(rule.width as usize), Style::new().fg(dim()))),
        rule,
    );
    render_transport(frame, transport, app);
    render_footer(frame, footer, app);

    if app.dj_panel.is_some() {
        render_dj_panel(frame, area, app);
    }
    if app.journey.is_some() {
        render_journey(frame, area, app);
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
    // to be riding on today.
    let shown = app.server_display();
    let host = shown
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_string();
    let mut candidates = Vec::new();
    if let Some(user) = &app.username {
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
fn column_widths(total: u16, trail: usize, queue: bool) -> Vec<u16> {
    const CURRENT_MIN: u16 = 28;
    const QUEUE_MIN: u16 = 22;
    const TRAIL_WIDTH: u16 = 20;
    const TRAIL_MAX: usize = 2;

    let queue_width = if queue { QUEUE_MIN.min(total.saturating_sub(CURRENT_MIN)) } else { 0 };
    let mut spare = total.saturating_sub(CURRENT_MIN + queue_width);
    let mut shown = 0;
    while shown < trail.min(TRAIL_MAX) && spare >= TRAIL_WIDTH {
        spare -= TRAIL_WIDTH;
        shown += 1;
    }

    let mut widths = vec![TRAIL_WIDTH; shown];
    widths.push(total.saturating_sub(TRAIL_WIDTH * shown as u16 + queue_width));
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

/// A column you came through. Never focused, so it is drawn quietly, with the
/// row you took marked rather than a cursor.
fn render_trail_column(frame: &mut Frame, area: Rect, step: &crate::tui::app::Trail) {
    let width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = step
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let line = entry_line(entry, width, None);
            // The row you came through keeps a bar of its own, so a chain of
            // columns reads as the path it is: this artist, then that album,
            // then the track you are on. Quieter than the cursor's bar, and
            // without its symbol, so which column has the keys is still plain.
            let style = if i == step.chosen {
                Style::new().fg(dim()).add_modifier(Modifier::REVERSED)
            } else {
                Style::new().fg(dim())
            };
            ListItem::new(restyle(line, style))
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(step.chosen));
    frame.render_stateful_widget(List::new(items), inset(area), &mut state);
    divider(frame, area);
}

/// The column the cursor is in.
fn render_current_column(frame: &mut Frame, area: Rect, app: &mut App) {
    let inner = inset(area);
    let content = inner.width.saturating_sub(CURSOR.len() as u16) as usize;
    let playing = app.now_playing.as_ref().map(|track| track.filepath.as_str());
    let items: Vec<ListItem> = app
        .pane()
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_line(entry, content, playing)))
        .collect();
    // A filter never hides the way out, so a pane with nothing left in it is
    // one row rather than none. Both are "nothing to see", and both want the
    // hint — just not drawn over the row that is there.
    let rows = items.len();
    let nothing = app.pane().entries.iter().all(|entry| matches!(entry, Entry::Parent));
    let list = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol(CURSOR);

    let state = match app.tab {
        Tab::Files => &mut app.files.state,
        Tab::Library => &mut app.library.state,
        Tab::Playlists => &mut app.playlists.state,
        Tab::Search => &mut app.search.state,
        Tab::Discover => &mut app.discover.state,
    };
    frame.render_stateful_widget(list, inner, state);

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

    let current = app.queue.current;
    let items: Vec<ListItem> = app
        .queue
        .items
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let playing = Some(i) == current;
            let style = if playing { Style::new().fg(accent()) } else { Style::new() };
            let marker = if playing { "\u{25b6} " } else { "  " };
            ListItem::new(Line::from(Span::styled(
                fit(&format!("{marker}{}", track.display_name()), inner.width as usize),
                style,
            )))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        inner,
        &mut app.queue.state,
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
fn empty_hint(app: &App) -> String {
    if app.pane().loading {
        return format!("{} loading\u{2026}", SPINNER[app.spinner % SPINNER.len()]);
    }
    // "(empty directory)" under a filter that matched nothing is a lie about
    // the directory.
    let filter = app.pane().filter.trim();
    if !filter.is_empty() {
        return format!("nothing here matches {filter:?}");
    }
    match app.tab {
        Tab::Files => "(empty directory)",
        Tab::Library => "(nothing here)",
        Tab::Playlists => "(no playlists)",
        Tab::Search => "type a query and press Enter",
        Tab::Discover => "(nothing similar)",
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
        },
        Tab::Playlists => match &app.playlist_open {
            Some(name) => format!(" Playlist: {name} "),
            None => " Playlists ".to_string(),
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
                DiscoverNode::Root => format!(" Discover · from {seed} "),
                DiscoverNode::Tracks => format!(" Sounds like {seed} "),
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
        Entry::Playlist { name } => Line::from(format!("♪ {name}")),
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
fn fmt_span(seconds: f64) -> String {
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

    // Body, then a rule, then the transport band and the key hints along the
    // foot. The band spans the full width rather than sitting in a column, so
    // the bar is long enough to read as a position rather than a stepper.
    let [body, rule, gauge_area, keys_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

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
            rule_with_junction(rule.width, left_width),
            Style::new().fg(dim()),
        )),
        rule,
    );

    frame.render_widget(
        Paragraph::new(progress_line(app, gauge_area.width as usize)),
        gauge_area,
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
    let names: usize = tabs.iter().map(|t| width_of(t.title()) + 2).sum();
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

    if tab_strip_width(app) > width {
        return Line::from(vec![
            Span::styled("‹ ", Style::new().fg(dim())),
            Span::styled(current.title(), active),
            Span::styled(" ›", Style::new().fg(dim())),
        ]);
    }

    let mut spans = Vec::new();
    for tab in app.now_tabs() {
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
        spans.push(Span::styled(tab.title(), style));
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
    match app.now_tab() {
        NowTab::Queue => "←→ tab   ↑↓ list   Enter play   d remove   0 back",
        NowTab::Visualizer if app.viz.mode.plots_samples() => {
            "←→ tab   v mode   . dots   0 back"
        }
        NowTab::Visualizer => "←→ tab   v mode   0 back",
        _ => "←→ tab   ↑↓ scroll   0 back",
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
        NowTab::AutoDj => frame.render_widget(Paragraph::new(autodj_summary(app)), content),
        NowTab::Lyrics => {
            render_now_placeholder(frame, content, "words go here", "not wired up yet")
        }
        NowTab::Discover => {
            render_now_placeholder(frame, content, "what this sounds like", "not wired up yet")
        }
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
    // No frame means either nothing has played or the audio thread is
    // mid-handover. Both mean "not this tick", and neither is worth a message
    // that would flicker at thirty frames a second.
    let Some(heard) = tap.frame() else {
        return render_now_placeholder(frame, picture, "the visualiser goes here", "nothing playing");
    };

    // Pausing stops the clock, the progress bar and the position. The tap
    // goes on holding the last tenth of a second that was played, so left to
    // itself the picture would carry on settling out of audio that is not
    // being played any more — neither moving nor still. Frozen is what the
    // rest of the screen does.
    let sounding = app.status.playing;
    let mut canvas = crate::tui::canvas::Canvas::new(picture);
    if !canvas.is_empty() {
        app.viz.draw(&mut canvas, &heard, sounding);
        frame.render_widget(Paragraph::new(canvas.into_lines()), picture);
    }
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
fn render_now_queue(frame: &mut Frame, area: Rect, app: &App) {
    if app.queue.items.is_empty() {
        render_now_placeholder(frame, area, "nothing queued", "0 back, then 'a' on a track");
        return;
    }

    let width = area.width as usize;
    let current = app.queue.current;
    let items: Vec<ListItem> = app
        .queue
        .items
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let playing = Some(i) == current;
            let style = if playing { Style::new().fg(accent()) } else { Style::new() };
            let marker = if playing { "\u{25b6} " } else { "  " };
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

    let mut state = app.queue.state;
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

/// What Auto-DJ is set to, read-only. `D` stays the one place it changes: two
/// screens editing one set of settings is two things to keep in step.
fn autodj_summary(app: &App) -> Vec<Line<'static>> {
    let faint = Style::new().fg(dim());
    let row = |label: &str, value: String| {
        Line::from(vec![Span::styled(format!("{label:<9}"), faint), Span::raw(value)])
    };

    let mut lines = vec![Line::raw(""), row("Mode", app.autodj.label().to_string())];
    if app.autodj != AutoDjMode::Off {
        let dj = &app.dj;
        if app.capabilities.discovery {
            lines.push(row(
                "Pool",
                match dj.sonic_tightness {
                    0 => "any".to_string(),
                    tightness => format!("sonic, tightness {tightness}"),
                },
            ));
        }
        lines.push(row("Tempo", format!("±{} BPM", dj.tempo_tolerance)));
        lines.push(row("Key", dj.key_matching.label().to_string()));
        if dj.min_rating > 0 {
            lines.push(row("Rating", format!("{}+", dj.min_rating)));
        }
        if dj.artist_cooldown > 0 {
            lines.push(row("Cooldown", format!("last {} artists", dj.artist_cooldown)));
        }
        if !dj.genres.is_empty() {
            lines.push(row("Genres", format!("{} {}", dj.genre_mode.label(), dj.genres.len())));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        if app.autodj == AutoDjMode::Off {
            "A turns it on \u{b7} D opens the panel"
        } else {
            "D opens the panel to change it"
        },
        faint,
    )));
    lines
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
        (SPINNER[app.spinner % SPINNER.len()], "starting")
    } else if app.status.paused {
        ("⏸", "paused")
    } else if app.status.playing {
        ("▶", "playing")
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
fn progress_line(app: &App, width: usize) -> Line<'static> {
    let position = fmt_duration(app.status.position);
    let total =
        if app.status.duration > 0.0 { fmt_duration(app.status.duration) } else { "--:--".into() };
    let time = format!("{position} / {total}");
    let bar_width = width.saturating_sub(width_of(&time) + 3);
    if bar_width < 4 {
        return Line::from(Span::styled(fit(&time, width), Style::new().fg(dim())));
    }

    let filled = (app.status.progress() * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    Line::from(vec![
        Span::styled("\u{2588}".repeat(filled), Style::new().fg(accent())),
        Span::styled("\u{2591}".repeat(bar_width - filled), Style::new().fg(dim())),
        Span::raw("  "),
        Span::styled(position, Style::new().fg(accent())),
        Span::styled(format!(" / {total}"), Style::new().fg(dim())),
    ])
}

fn render_transport(frame: &mut Frame, area: Rect, app: &App) {
    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

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

    frame.render_widget(
        Paragraph::new(progress_line(app, gauge_area.width as usize)),
        gauge_area,
    );
}

/// What the footer says about a narrowed list. The count is the useful part:
/// it says how much is hidden, which is the question a short list raises.
fn filter_readout(app: &App) -> String {
    let pane = app.pane();
    let caret = if app.filtering { "\u{258F}" } else { "" };
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

    // A filter outranks both the message and the hint while one is on: it is
    // the explanation for a list that is shorter than it should be, and
    // leaving that unsaid is how a filtered pane gets mistaken for an empty
    // library.
    let message = if app.filtering || !app.pane().filter.is_empty() {
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
        // Line the columns up the way the method chooser does.
        let name_column = form
            .found
            .iter()
            .map(|server| server.name.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        for (i, server) in form.found.iter().enumerate() {
            let selected = form.row == i;
            let style = if selected {
                Style::new().fg(accent()).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let mut spans = vec![
                Span::styled(
                    format!(
                        "{} {:<width$}",
                        if selected { ">" } else { " " },
                        server.name,
                        width = name_column
                    ),
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
        format!("Connecting to {}…", app.server),
        Style::new().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled("Ctrl+C to quit", Style::new().fg(dim()))));
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

/// A Sonic Journey: the arc from one track to another, ready to become the
/// queue.
fn render_journey(frame: &mut Frame, area: Rect, app: &App) {
    let Some(journey) = app.journey.as_ref() else { return };
    let faint = Style::new().fg(dim());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  From    ", faint),
            Span::raw(journey.from.display_name()),
        ]),
        Line::from(vec![
            Span::styled("  To      ", faint),
            Span::raw(journey.to.display_name()),
        ]),
        Line::from(vec![
            Span::styled("  Stops   ", faint),
            Span::raw(journey.length.to_string()),
            Span::styled("   ←→ to change", faint),
        ]),
        Line::raw(""),
    ];

    if journey.pending {
        lines.push(Line::from(Span::styled("  plotting the route…", faint)));
    } else if journey.stops.is_empty() {
        lines.push(Line::from(Span::styled("  no route between these two", faint)));
    } else {
        // Leave room for the three header lines, the hint, and the borders.
        let visible = (area.height as usize).saturating_sub(10).max(3);
        let shown = journey.stops.iter().enumerate().skip(journey.offset).take(visible);
        for (index, stop) in shown {
            let position = format!("{:>3.0}%", stop.t * 100.0);
            // The ends are the tracks that were chosen; everything between is
            // the server's pick for that point on the arc.
            let is_end = index == 0 || index + 1 == journey.stops.len();
            let style = if is_end { Style::new().fg(accent()) } else { Style::new() };
            lines.push(Line::from(vec![
                Span::styled(format!("  {:>2}. ", index + 1), faint),
                Span::styled(position, faint),
                Span::styled(format!("  {}", stop.metadata_display()), style),
            ]));
        }
        let remaining = journey.stops.len().saturating_sub(journey.offset + visible);
        if remaining > 0 {
            lines.push(Line::from(Span::styled(format!("      … {remaining} more"), faint)));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ scroll · ←→ stops · Enter queue it · Esc cancel",
        faint,
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(70, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(" Sonic Journey ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The Auto-DJ panel: what the picker is being told to do, and what that
/// actually produces.
fn render_dj_panel(frame: &mut Frame, area: Rect, app: &App) {
    let Some(panel) = app.dj_panel.as_ref() else { return };
    if panel.genres.is_some() {
        return render_genre_picker(frame, area, app);
    }

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

    lines.push(Line::raw(""));
    lines.extend(dj_sample_lines(app));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ choose · ←→ adjust · p sample · Esc close",
        Style::new().fg(dim()),
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(66, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(accent()))
        .title(" Auto-DJ ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
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
    }
}

/// The sample block: what these settings actually pick, and how big a pool
/// they leave to pick from.
fn dj_sample_lines(app: &App) -> Vec<Line<'static>> {
    let Some(panel) = app.dj_panel.as_ref() else { return Vec::new() };
    let faint = Style::new().fg(dim());
    let mut lines = vec![Line::from(Span::styled("  Sample", faint))];

    if let Some(pool) = &panel.pool {
        lines.push(Line::from(Span::styled(
            format!("  {} tracks inside the sonic pool", pool.pool_size),
            faint,
        )));
    }
    if panel.sample_pending {
        lines.push(Line::from(Span::styled("  picking…", faint)));
        return lines;
    }
    if panel.sample.is_empty() {
        lines.push(Line::from(Span::styled("  press p to see what these settings pick", faint)));
        return lines;
    }
    for (index, track) in panel.sample.iter().enumerate() {
        lines.push(Line::from(format!("  {}. {}", index + 1, track.display_name())));
    }
    lines
}

fn render_genre_picker(frame: &mut Frame, area: Rect, app: &App) {
    let Some(picker) = app.dj_panel.as_ref().and_then(|p| p.genres.as_ref()) else { return };

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
    let width = (area.width * width_percent / 100).min(area.width);
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
        assert!(text.contains("▶"), "the queue column marks the current track");
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
    fn the_tabs_move_under_the_arrows_and_come_back_round() {
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        app.handle_action(Action::ToggleNowPlaying);
        assert_eq!(app.input_mode(), InputMode::Now, "the view claims the arrows");

        // The tagged track has no lyrics, so that tab is not in the strip and
        // the arrows must not stop on it.
        assert_eq!(app.now_tabs(), vec![NowTab::Queue, NowTab::Discover, NowTab::AutoDj, NowTab::Visualizer]);

        let right = |app: &mut App| {
            let action = app.keymap.action(key_event(KeyCode::Right), InputMode::Now).unwrap();
            app.handle_action(action);
        };
        right(&mut app);
        assert_eq!(app.now_tab(), NowTab::Discover);
        right(&mut app);
        right(&mut app);
        assert_eq!(app.now_tab(), NowTab::Visualizer);
        right(&mut app);
        assert_eq!(app.now_tab(), NowTab::Queue, "off the end and round again");

        let left = app.keymap.action(key_event(KeyCode::Left), InputMode::Now).unwrap();
        app.handle_action(left);
        assert_eq!(app.now_tab(), NowTab::Visualizer, "and the other way");
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
    fn the_auto_dj_tab_reports_without_offering_to_change_anything() {
        let mut app = connected_app();
        app.now_playing = Some(tagged_track());
        app.handle_action(Action::ToggleNowPlaying);
        app.autodj = AutoDjMode::BpmKey;
        app.now_tab = NowTab::AutoDj;

        let text = draw(&mut app);
        assert!(text.contains("Mode") && text.contains("tempo+key"), "{text}");
        assert!(text.contains("Tempo") && text.contains("BPM"), "{text}");
        assert!(text.contains("D opens the panel"), "one place to change it: {text}");
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

        let line = progress_line(&app, 40);
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
            progress_line(&app, 12).spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(cramped.contains("0:30"), "{cramped}");
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

        // Wide enough: every tab is named.
        let wide = draw_sized(&mut app, 100, 16);
        assert!(wide.contains("[Queue]  Lyrics   Discover   Auto-DJ   Visualizer "), "{wide}");

        // Not wide enough: the one you are on, and arrows for the rest. A tab
        // chopped to "Vis" reads as a bug; this reads as a choice.
        let narrow = draw_sized(&mut app, 64, 16);
        assert!(narrow.contains("‹ Queue ›"), "{narrow}");
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

        app.library_stack = vec![LibraryNode::Root, LibraryNode::Genre("Ambient".into())];
        app.apply_event(crate::tui::worker::Event::Library {
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
        app.files.set(vec![crate::tui::app::Entry::Track {
            label: "seed".into(),
            track: Box::new(Track {
                filepath: "lib/seed.mp3".into(),
                metadata: TrackMetadata {
                    artist: Some("Seed Artist".into()),
                    title: Some("Seed Song".into()),
                    ..Default::default()
                },
            }),
        }]);
        app.files.state.select(Some(0));

        let discover = app.tabs().iter().position(|t| *t == Tab::Discover).unwrap();
        app.handle_action(Action::SelectTab(discover));
        let text = draw(&mut app);
        assert!(text.contains("5:Discover"));
        assert!(text.contains("Seed Artist - Seed Song"), "the title names the seed:\n{text}");
        assert!(text.contains("Similar tracks"));
        assert!(text.contains("like Seed Artist"), "and the artist row names the artist");

        // Artist rows carry how close, how many ways in, and what it sounds
        // like — the three things that decide whether to open one.
        app.discover.state.select(Some(1));
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
        });
        let text = draw(&mut app);
        assert!(text.contains("Artists like Seed Artist"), "got:\n{text}");
        assert!(text.contains("Near One"));
        assert!(text.contains("0.94"));
        assert!(text.contains("1 way in"), "singular, not '1 ways in':\n{text}");
        assert!(text.contains("Dubstep") && !text.contains("Electronic---"));
    }

    #[test]
    fn a_journey_shows_the_arc_it_would_queue() {
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

        // While it's in flight the panel says so rather than looking empty.
        let text = draw_sized(&mut app, 100, 30);
        assert!(text.contains("Sonic Journey"));
        assert!(text.contains("plotting the route"), "got:\n{text}");

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
        });

        let text = draw_sized(&mut app, 100, 30);
        assert!(text.contains("First - Departure"));
        assert!(text.contains("Middle - Somewhere"));
        assert!(text.contains("Last - Arrival"));
        // The arc position is what makes it a journey rather than a playlist.
        assert!(text.contains("0%") && text.contains("50%") && text.contains("100%"), "got:\n{text}");
        assert!(text.contains("Enter queue it"));
    }

    #[test]
    fn a_journey_survives_a_small_terminal() {
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
        });
        // A 32-stop arc in a short terminal must scroll, not overflow.
        let text = draw_sized(&mut app, 60, 20);
        assert!(text.contains("more"), "the rest is accounted for:\n{text}");
        draw_sized(&mut app, 24, 9);
    }

    #[test]
    fn the_dj_panel_shows_each_setting_with_what_it_means() {
        let mut app = connected_app();
        app.dj.sonic_tightness = 60;
        app.handle_action(Action::OpenDjPanel);
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
        assert!(text.contains("press p to see what these settings pick"));
    }

    #[test]
    fn the_dj_panel_hides_the_sonic_rows_without_an_index() {
        let mut app = connected_app();
        app.capabilities = Default::default();
        app.handle_action(Action::OpenDjPanel);
        let text = draw_sized(&mut app, 100, 34);
        assert!(!text.contains("Sonic pool"), "nothing promises a pool that can't exist");
        assert!(!text.contains("Anchor"));
        assert!(text.contains("Tempo window"), "the rest of the panel is still there");
    }

    #[test]
    fn the_dj_panel_reports_the_pool_and_the_sample() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
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
        assert!(text.contains("1247 tracks inside the sonic pool"), "got:\n{text}");
        assert!(text.contains("1. Band - Song"), "got:\n{text}");
    }

    #[test]
    fn the_genre_chooser_marks_what_is_selected() {
        let mut app = connected_app();
        app.dj.genres = vec!["Techno".into()];
        app.handle_action(Action::OpenDjPanel);
        let last = app.dj_panel.as_ref().unwrap().rows.len() - 1;
        app.dj_panel.as_mut().unwrap().row = last;
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
    fn the_dj_panel_survives_a_small_terminal() {
        let mut app = connected_app();
        app.handle_action(Action::OpenDjPanel);
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
        for (keys, what) in crate::tui::app::Keymap::default().help_rows() {
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
    fn the_columns_build_up_as_you_go_in_and_fall_away_as_you_come_out() {
        use crate::tui::worker::{LibraryData, LibraryNode};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(1));
        app.apply_event(Event::Library {
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
        assert!(text.contains("filter: bass\u{258F}"), "{text}");
        assert!(text.contains("2 of 3"), "{text}");
        assert!(text.contains("Bassnectar") && text.contains("Basshunter"), "{text}");
        assert!(!text.contains("Portishead"), "the filtered-out row is gone: {text}");

        // Once typing stops the filter is still on, so it still says so.
        app.handle_action(Action::Submit);
        let text = draw(&mut app);
        assert!(text.contains("filter: bass"), "{text}");
        assert!(!text.contains("filter: bass\u{258F}"), "the caret goes with the prompt: {text}");
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

    #[test]
    fn a_pane_waiting_on_the_server_says_so_instead_of_saying_it_is_empty() {
        let mut app = connected_app();
        // The bug this exists for: opening Playlists showed "(no playlists)"
        // for as long as the round trip took, which is a different claim.
        app.handle_action(Action::SelectTab(2));
        let waiting = draw(&mut app);
        assert!(waiting.contains("loading…"), "{waiting}");
        assert!(!waiting.contains("(no playlists)"), "{waiting}");

        app.apply_event(Event::Playlists(Vec::new()));
        let answered = draw(&mut app);
        assert!(answered.contains("(no playlists)"), "{answered}");
        assert!(!answered.contains("loading…"), "{answered}");
    }

    #[test]
    fn a_failed_request_stops_the_spinner_rather_than_turning_forever() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        app.apply_event(Event::Error("server said no".into()));
        let text = draw(&mut app);
        assert!(!text.contains("loading…"), "{text}");
        assert!(text.contains("(no playlists)"), "{text}");
    }

    #[test]
    fn the_spinner_turns() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(2));
        let first = draw(&mut app);
        app.spinner += 1;
        let second = draw(&mut app);
        assert_ne!(first, second, "advancing the tick changes the frame");
    }

    #[test]
    fn search_tab_shows_the_query_and_result_summary() {
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(3));
        for c in "moon".chars() {
            app.handle_action(Action::Input(c));
        }
        assert!(draw(&mut app).contains("Search: moon"));

        app.handle_action(Action::Submit);
        app.apply_event(Event::SearchResults(Box::new(Default::default())));
        assert!(draw(&mut app).contains("0 matches"));
    }

    #[test]
    fn the_search_menu_says_what_matched_and_how_many() {
        use crate::api::types::{SearchGroup, SearchResults, SearchTrack};
        let mut app = connected_app();
        app.handle_action(Action::SelectTab(3));
        let hit = |name: &str| SearchTrack {
            name: name.into(),
            filepath: format!("lib/{name}.mp3"),
            album_art_file: None,
            metadata: TrackMetadata::default(),
        };
        app.apply_event(Event::SearchResults(Box::new(SearchResults {
            artists: vec![
                SearchGroup { name: "Moon Hooch".into(), album_art_file: None },
                SearchGroup { name: "Moondog".into(), album_art_file: None },
            ],
            albums: vec![],
            title: vec![hit("Moonlight")],
            files: vec![],
            lyrics: vec![hit("Harvest")],
        })));

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
        app.username = Some("tester".into());
        for width in [76, 80, 100, 140] {
            let text = draw_sized(&mut app, width, 20);
            for tab in ["1:Files", "2:Library", "3:Playlists", "4:Search"] {
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
        app.username = Some("tester".into());

        // Roomy: the whole thing, scheme and all, next to both extras.
        let wide = draw_sized(&mut app, 140, 20);
        assert!(wide.contains("tester@http://host:3000"));
        assert!(wide.contains("0:Now Playing") && wide.contains("Tab:Queue"));

        // Less room: the scheme is the first thing worth giving up, and it
        // goes to a reminder of a key. The username is not up for trade, so
        // the extras shorten before it does.
        let mid = draw_sized(&mut app, 96, 20);
        assert!(mid.contains("tester@host:3000"), "kept who as well as where");
        assert!(mid.contains("0:Now") && mid.contains("Tab:Queue"));
        assert!(!mid.contains("0:Now Playing"), "the long form is what gave way: {mid}");

        // Genuinely tight: the extras go entirely, and every tab is still
        // whole — including the fifth, which only exists on a server with
        // discovery and is what made the header longer.
        let narrow = draw_sized(&mut app, 72, 20);
        assert!(!narrow.contains("0:Now"), "a hint is not worth a tab: {narrow}");
        assert!(narrow.contains("tester@host:3000"));
        assert!(narrow.contains("5:Discover"));

        // A server without discovery has no fifth tab, so the same label
        // survives in a narrower terminal.
        let mut plain = connected_app();
        plain.capabilities = Default::default();
        plain.username = Some("tester".into());
        let text = draw_sized(&mut plain, 72, 20);
        assert!(!text.contains("Discover"), "no tab for a feature this server lacks");
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
