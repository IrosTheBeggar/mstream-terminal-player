//! Rendering. Reads app state, draws widgets — no decisions of its own.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Tabs, Wrap};

use crate::cmd_library::fmt_duration;

use super::app::{
    App, CONNECT_METHODS, ConnectStage, DjRow, Entry, Focus, MessageKind, Queue, Repeat, Tab,
};
use super::worker::{AutoDjMode, DiscoverNode, LibraryNode};
use crate::api::types::Track;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

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
        BANNER.iter().map(|l| Line::from(Span::styled(*l, Style::new().fg(ACCENT)))).collect();
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
        MessageKind::Info => Style::new().fg(DIM),
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
    let [header, body, transport, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, app);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body);
    render_browser(frame, left, app);
    render_queue(frame, right, app);

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

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    // Only the tabs this server can serve, so the numbers run 1..n with no
    // gaps and none of them lead somewhere empty.
    let titles: Vec<Line> = app
        .tabs()
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {}:{} ", i + 1, t.title())))
        .collect();

    // The tabs are how you move around, so they get the space they need and
    // the server label takes what is left — a fixed split truncated "4:Search"
    // clean off at 80 columns. The titles carry their own spacing, so the
    // widget's default padding is turned off below and this sum is exact.
    let tabs_width: u16 =
        titles.iter().map(|line| line.width() as u16).sum::<u16>().min(area.width);
    let [tabs_area, server_area] =
        Layout::horizontal([Constraint::Length(tabs_width), Constraint::Min(0)]).areas(area);

    frame.render_widget(
        Tabs::new(titles)
            .select(app.tab_index())
            .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider("")
            .padding("", ""),
        tabs_area,
    );

    let who = server_label(app, server_area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(who, Style::new().fg(DIM))))
            .alignment(Alignment::Right),
        server_area,
    );
}

/// The most informative form of "who and where" that fits, down to nothing.
/// Dropping the scheme first keeps the host visible on a narrow terminal.
fn server_label(app: &App, width: usize) -> String {
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
        .into_iter()
        .find(|candidate| candidate.chars().count() <= width)
        .unwrap_or_default()
}

fn render_browser(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Browser;
    let title = browser_title(app);

    // Borders take two columns and the cursor gutter takes the width of its
    // symbol on every row; what is left is what a line has to lay out in.
    let content = area.width.saturating_sub(2 + CURSOR.len() as u16) as usize;
    let playing = app.now_playing.as_ref().map(|track| track.filepath.as_str());
    let items: Vec<ListItem> = app
        .pane()
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_line(entry, content, playing)))
        .collect();

    let empty = items.is_empty();
    // Reversed, not accent-coloured: accent now means "this is playing", and
    // one row can be both. Background for where you are, foreground for what
    // you are hearing.
    let list = List::new(items)
        .block(bordered(title, focused))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol(CURSOR);

    let state = match app.tab {
        Tab::Files => &mut app.files.state,
        Tab::Library => &mut app.library.state,
        Tab::Playlists => &mut app.playlists.state,
        Tab::Search => &mut app.search.state,
        Tab::Discover => &mut app.discover.state,
    };
    frame.render_stateful_widget(list, area, state);

    if empty {
        frame.render_widget(
            Paragraph::new(Span::styled(empty_hint(app), Style::new().fg(DIM))),
            inner_first_line(area),
        );
    }
}

/// What an empty pane should say. "Nothing here" and "not here yet" look
/// identical on screen and mean opposite things, so the answer turns on
/// whether a request is still out — a pane that says "(no playlists)" while
/// the playlists are on the wire has told you something false.
fn empty_hint(app: &App) -> String {
    if app.pane().loading {
        return format!("{} loading…", SPINNER[app.spinner % SPINNER.len()]);
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

fn browser_title(app: &App) -> String {
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
            match (&app.search_summary, app.editing_query) {
                (_, true) => format!(" Search: {query}▏"),
                (Some(summary), _) => format!(" Search: {query} — {summary} "),
                (None, _) => format!(" Search: {query} "),
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
        Entry::Parent => Line::from(Span::styled("..", Style::new().fg(DIM))),
        Entry::Dir { label, .. } => Line::from(Span::styled(
            format!("{label}/"),
            Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        )),
        Entry::Node { label, .. } => Line::from(Span::styled(
            label.clone(),
            Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
        )),
        Entry::Playlist { name } => Line::from(format!("♪ {name}")),
        Entry::Discover { label, detail, .. } => Line::from(vec![
            Span::styled(
                label.clone(),
                Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("   {detail}"), Style::new().fg(DIM)),
        ]),
        Entry::Track { label, track } => {
            let style = if playing.is_some_and(|path| path == track.filepath) {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
                Span::styled(time, Style::new().fg(DIM)),
            ])
        }
    }
}

fn render_queue(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Queue;
    let title = queue_title(&app.queue);

    let current = app.queue.current;
    let items: Vec<ListItem> = app
        .queue
        .items
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let playing = Some(i) == current;
            let marker = if playing { "▶ " } else { "  " };
            let style = if playing {
                Style::new().fg(ACCENT)
            } else {
                Style::new()
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{}", track.display_name()),
                style,
            )))
        })
        .collect();

    let empty = items.is_empty();
    let list = List::new(items)
        .block(bordered(title, focused))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut app.queue.state);

    if empty {
        // Along the bottom edge, where a hint belongs — the top of the pane is
        // where the first track will appear, and putting the prompt there
        // makes an empty queue look like a queue with something in it.
        frame.render_widget(
            Paragraph::new(Span::styled("'a' queues a track", Style::new().fg(DIM)))
                .wrap(Wrap { trim: true }),
            inner_last_line(area),
        );
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

fn render_now_playing(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(DIM))
        .title(" Now Playing ");
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    // The gauge and the status line are pinned to the bottom; the card floats
    // in whatever is left, so it stays centred as the terminal changes size.
    let [card, gauge_area, status_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    render_centered_block(frame, card, now_playing_card(app, card.width as usize));

    let position = fmt_duration(app.status.position);
    let total =
        if app.status.duration > 0.0 { fmt_duration(app.status.duration) } else { "--:--".into() };
    frame.render_widget(
        Gauge::default()
            .ratio(app.status.progress())
            .label(format!("{position} / {total}"))
            .gauge_style(Style::new().fg(ACCENT))
            .use_unicode(true),
        gauge_area,
    );

    // There is no footer down here, so the modes come along — otherwise going
    // full screen would quietly hide whether shuffle or auto-dj is on.
    let modes = mode_readout(app, false);
    let [left, right] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length((width_of(&modes) as u16).min(status_area.width)),
    ])
    .areas(status_area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            fit("0 back", left.width as usize),
            Style::new().fg(DIM),
        )),
        left,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(DIM)))
            .alignment(Alignment::Right),
        right,
    );
}

/// The block of text in the middle of the now-playing screen, laid out to
/// `width` columns. Pure so the shape can be asserted without a terminal.
fn now_playing_card(app: &App, width: usize) -> Vec<Line<'static>> {
    let Some(track) = &app.now_playing else {
        return vec![
            Line::from(Span::styled(fit("nothing playing", width), Style::new().fg(DIM))),
            Line::raw(""),
            Line::from(Span::styled(
                fit("press 0 to go back and queue something", width),
                Style::new().fg(DIM),
            )),
        ];
    };

    let meta = &track.metadata;
    let mut lines = vec![
        Line::from(Span::styled(
            fit(meta.artist.as_deref().unwrap_or("unknown artist"), width),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            fit(meta.display_title().unwrap_or_else(|| track.file_name()), width),
            Style::new().add_modifier(Modifier::BOLD),
        )),
    ];

    let album = match (meta.album.as_deref().filter(|a| !a.is_empty()), meta.year) {
        (Some(album), Some(year)) => Some(format!("{album} · {year}")),
        (Some(album), None) => Some(album.to_string()),
        (None, Some(year)) => Some(year.to_string()),
        (None, None) => None,
    };
    if let Some(album) = album {
        lines.push(Line::from(Span::styled(fit(&album, width), Style::new().fg(DIM))));
    }

    let facts = track_facts(meta);
    if !facts.is_empty() {
        lines.push(Line::from(Span::styled(fit(&facts, width), Style::new().fg(DIM))));
    }

    lines.push(Line::raw(""));
    let state = if !app.audio_available {
        "audio device unavailable"
    } else if app.status.paused {
        "⏸ paused"
    } else if app.status.playing {
        "▶ playing"
    } else {
        "stopped"
    };
    let style = if app.audio_available {
        Style::new().fg(DIM)
    } else {
        Style::new().fg(Color::Red)
    };
    lines.push(Line::from(Span::styled(state, style)));
    lines
}

/// The one-line summary of what the tags know. Everything is optional, so this
/// joins what is there rather than laying out a fixed set of slots — a track
/// with no BPM should not leave a gap where the BPM would go.
fn track_facts(meta: &crate::api::types::TrackMetadata) -> String {
    let mut facts = Vec::new();
    if let Some(duration) = meta.duration {
        facts.push(fmt_duration(duration));
    }
    if let Some(bpm) = meta.bpm {
        facts.push(format!("{bpm} BPM"));
    }
    if let Some(key) = meta.musical_key.as_deref().filter(|k| !k.is_empty()) {
        // The Camelot code is what the Auto-DJ panel matches on, so show the
        // same name for the same thing; the tag's own spelling comes after it.
        match crate::dj::to_camelot(key) {
            Some(camelot) => facts.push(format!("{} ({key})", camelot.code())),
            None => facts.push(key.to_string()),
        }
    }
    if let Some(rating) = meta.rating.filter(|r| *r > 0) {
        facts.push(format!("{rating}/10"));
    }
    if let Some(plays) = meta.play_count.filter(|p| *p > 0) {
        facts.push(format!("{plays} plays"));
    }
    facts.join(" · ")
}

fn render_transport(frame: &mut Frame, area: Rect, app: &App) {
    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let (label, style) = match (&app.now_playing, app.audio_available) {
        (_, false) => ("audio device unavailable".to_string(), Style::new().fg(Color::Red)),
        (Some(track), _) => {
            let state = if app.status.paused {
                "paused"
            } else if app.status.playing {
                "playing"
            } else {
                "stopped"
            };
            (
                format!("{} · {}", track.display_name(), state),
                Style::new().add_modifier(Modifier::BOLD),
            )
        }
        (None, _) => ("nothing playing".to_string(), Style::new().fg(DIM)),
    };
    frame.render_widget(Paragraph::new(Span::styled(label, style)), title_area);

    let position = fmt_duration(app.status.position);
    let total = if app.status.duration > 0.0 {
        fmt_duration(app.status.duration)
    } else {
        "--:--".to_string()
    };
    frame.render_widget(
        Gauge::default()
            .ratio(app.status.progress())
            .label(format!("{position} / {total}"))
            .gauge_style(Style::new().fg(ACCENT))
            .use_unicode(true),
        gauge_area,
    );
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

    let message = match &app.message {
        Some(m) => Span::styled(
            fit(&m.text, message_area.width as usize),
            match m.kind {
                MessageKind::Error => Style::new().fg(Color::Red),
                MessageKind::Info => Style::new().fg(DIM),
            },
        ),
        None => Span::styled(
            fit("? help   q quit", message_area.width as usize),
            Style::new().fg(DIM),
        ),
    };
    frame.render_widget(Paragraph::new(Line::from(message)), message_area);
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(DIM))).alignment(Alignment::Right),
        modes_area,
    );
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
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
            Span::styled((*blurb).to_string(), Style::new().fg(DIM)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · Enter continue · Ctrl+C quits",
        Style::new().fg(DIM),
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
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
            spans.push(Span::styled(hint.to_string(), Style::new().fg(DIM)));
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
        Style::new().fg(DIM),
    )));
    lines.push(Line::from(Span::styled(
        "Tab/↑↓ switch fields · Enter connects · Esc back",
        Style::new().fg(DIM),
    )));
    if app.connecting {
        lines.push(Line::from(Span::styled("connecting…", Style::new().fg(ACCENT))));
    }
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

/// Step two, Quick Connect branch: pick a server found on the network, or
/// paste a pairing code to reach one anywhere.
fn render_connect_quick(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = banner_lines(area);
    let form = &app.connect;

    lines.push(Line::from(Span::styled("Servers on your network", Style::new().fg(DIM))));

    if form.found.is_empty() {
        lines.push(Line::from(Span::styled(
            if form.searching { "  searching…" } else { "  none found" },
            Style::new().fg(DIM),
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
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
                Span::styled(server.base_url.clone(), Style::new().fg(DIM)),
            ];
            // Only advertise pairing where the server says it's available.
            if server.quick_connect {
                spans.push(Span::styled("  · pairing available", Style::new().fg(DIM)));
            }
            lines.push(Line::from(spans));
        }
        if form.searching {
            lines.push(Line::from(Span::styled("  searching…", Style::new().fg(DIM))));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Or paste a pairing code to reach a server anywhere",
        Style::new().fg(DIM),
    )));

    let selected = form.on_paste_row();
    let code = form.code.trim();
    // The code is a few hundred characters — show enough to recognise it,
    // plus a length so a paste is visibly confirmed.
    let shown = if code.is_empty() {
        Span::styled("paste the code here", Style::new().fg(DIM))
    } else {
        let head: String = code.chars().take(28).collect();
        Span::raw(format!("{head}…  ({} characters)", code.chars().count()))
    };
    lines.push(Line::from(vec![
        Span::styled(
            if selected { "> " } else { "  " },
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        shown,
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "A code opens a tunnel — you'll still sign in afterwards.",
        Style::new().fg(DIM),
    )));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · Enter connects · Esc back",
        Style::new().fg(DIM),
    )));
    if app.connecting {
        lines.push(Line::from(Span::styled("connecting…", Style::new().fg(ACCENT))));
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
    lines.push(Line::from(Span::styled("Ctrl+C to quit", Style::new().fg(DIM))));
    lines.extend(connect_message(app, area));
    render_centered_block(frame, area, lines);
}

/// A Sonic Journey: the arc from one track to another, ready to become the
/// queue.
fn render_journey(frame: &mut Frame, area: Rect, app: &App) {
    let Some(journey) = app.journey.as_ref() else { return };
    let dim = Style::new().fg(DIM);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("  From    ", dim),
            Span::raw(journey.from.display_name()),
        ]),
        Line::from(vec![
            Span::styled("  To      ", dim),
            Span::raw(journey.to.display_name()),
        ]),
        Line::from(vec![
            Span::styled("  Stops   ", dim),
            Span::raw(journey.length.to_string()),
            Span::styled("   ←→ to change", dim),
        ]),
        Line::raw(""),
    ];

    if journey.pending {
        lines.push(Line::from(Span::styled("  plotting the route…", dim)));
    } else if journey.stops.is_empty() {
        lines.push(Line::from(Span::styled("  no route between these two", dim)));
    } else {
        // Leave room for the three header lines, the hint, and the borders.
        let visible = (area.height as usize).saturating_sub(10).max(3);
        let shown = journey.stops.iter().enumerate().skip(journey.offset).take(visible);
        for (index, stop) in shown {
            let position = format!("{:>3.0}%", stop.t * 100.0);
            // The ends are the tracks that were chosen; everything between is
            // the server's pick for that point on the arc.
            let is_end = index == 0 || index + 1 == journey.stops.len();
            let style = if is_end { Style::new().fg(ACCENT) } else { Style::new() };
            lines.push(Line::from(vec![
                Span::styled(format!("  {:>2}. ", index + 1), dim),
                Span::styled(position, dim),
                Span::styled(format!("  {}", stop.metadata_display()), style),
            ]));
        }
        let remaining = journey.stops.len().saturating_sub(journey.offset + visible);
        if remaining > 0 {
            lines.push(Line::from(Span::styled(format!("      … {remaining} more"), dim)));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ scroll · ←→ stops · Enter queue it · Esc cancel",
        dim,
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(70, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
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
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
        Style::new().fg(DIM),
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(66, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(" Auto-DJ ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The right-hand side of a settings row: the value, plus whatever context
/// makes it meaningful.
fn dj_value_spans(row: DjRow, app: &App) -> Vec<Span<'static>> {
    let dim = Style::new().fg(DIM);
    let value = |text: String| Span::raw(text);
    match row {
        DjRow::Mode => {
            let mut spans = vec![value(app.autodj.label().to_string())];
            // Worth saying out loud: in this mode the pick comes straight
            // from the neighbour list, so the filters below are not consulted.
            if app.autodj == AutoDjMode::Similar {
                spans.push(Span::styled("   filters below don't apply", dim));
            }
            spans
        }
        DjRow::Tightness => {
            if app.dj.sonic_tightness == 0 {
                return vec![value("off".into()), Span::styled("   any track", dim)];
            }
            let filled = (app.dj.sonic_tightness / 10) as usize;
            let bar: String =
                "▓".repeat(filled) + &"░".repeat(10usize.saturating_sub(filled));
            let cosine = crate::dj::sonic_threshold(app.dj.sonic_tightness).unwrap_or(0.0);
            vec![
                value(format!("{bar} {:>3}%", app.dj.sonic_tightness)),
                // The raw number is what the server actually filters on, and
                // seeing it is how the slider stops being a mystery.
                Span::styled(format!("   cosine ≥ {cosine:.2}"), dim),
            ]
        }
        DjRow::Anchor => {
            let (label, what) = match app.dj.sonic_anchor {
                crate::dj::SonicAnchor::Current => ("current", "follows each track"),
                crate::dj::SonicAnchor::Session => ("session", "averages recent picks"),
            };
            vec![value(label.into()), Span::styled(format!("   {what}"), dim)]
        }
        DjRow::Tempo => {
            if app.dj.tempo_tolerance == 0 {
                return vec![value("off".into())];
            }
            vec![
                value(format!("±{}%", app.dj.tempo_tolerance)),
                Span::styled(
                    format!("   widens to ±{}% before giving up", app.dj.tempo_tolerance * 2),
                    dim,
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
                Span::styled(format!("   {what}"), dim),
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
                Span::styled("   recently played, skipped", dim),
            ]
        }
        DjRow::Genres => {
            let mode = app.dj.genre_mode.label().to_string();
            if app.dj.genre_mode == crate::dj::GenreMode::Off {
                return vec![value(mode), Span::styled("   Enter to choose", dim)];
            }
            let chosen = if app.dj.genres.is_empty() {
                "none chosen — Enter to pick".to_string()
            } else {
                app.dj.genres.join(", ")
            };
            let mut spans = vec![value(format!("{mode}  ")), Span::styled(chosen, dim)];
            // The asymmetry bites people: "only these" is a stricter promise
            // than "anything but these", and it drops untagged tracks.
            if app.dj.genre_mode == crate::dj::GenreMode::Whitelist
                && !app.dj.genres.is_empty()
            {
                spans.push(Span::styled("  (untagged excluded)", dim));
            }
            spans
        }
    }
}

/// The sample block: what these settings actually pick, and how big a pool
/// they leave to pick from.
fn dj_sample_lines(app: &App) -> Vec<Line<'static>> {
    let Some(panel) = app.dj_panel.as_ref() else { return Vec::new() };
    let dim = Style::new().fg(DIM);
    let mut lines = vec![Line::from(Span::styled("  Sample", dim))];

    if let Some(pool) = &panel.pool {
        lines.push(Line::from(Span::styled(
            format!("  {} tracks inside the sonic pool", pool.pool_size),
            dim,
        )));
    }
    if panel.sample_pending {
        lines.push(Line::from(Span::styled("  picking…", dim)));
        return lines;
    }
    if panel.sample.is_empty() {
        lines.push(Line::from(Span::styled("  press p to see what these settings pick", dim)));
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
        lines.push(Line::from(Span::styled("  loading genres…", Style::new().fg(DIM))));
    } else if picker.all.is_empty() {
        lines.push(Line::from(Span::styled("  no genres tagged", Style::new().fg(DIM))));
    } else {
        // Keep the highlighted row on screen for long lists.
        let visible = (area.height.saturating_sub(8)) as usize;
        let first = picker.row.saturating_sub(visible.saturating_sub(1));
        for (index, name) in picker.all.iter().enumerate().skip(first).take(visible) {
            let chosen = app.dj.genres.iter().any(|g| g == name);
            let focused = index == picker.row;
            let style = if focused {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
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
        Style::new().fg(DIM),
    )));

    let height = (lines.len() as u16 + 2).min(area.height);
    let box_area = centered_rect(50, height, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
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
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
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
        .border_style(Style::new().fg(ACCENT))
        .title(" Keys ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn bordered(title: String, focused: bool) -> Block<'static> {
    let style = if focused { Style::new().fg(ACCENT) } else { Style::new().fg(DIM) };
    Block::default().borders(Borders::ALL).border_style(style).title(title)
}

/// First writable row inside a bordered block — used for placeholder text in
/// empty lists.
fn inner_first_line(area: Rect) -> Rect {
    Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(3),
        height: 1.min(area.height.saturating_sub(2)),
    }
}

/// Last writable row inside a bordered block, for a hint that belongs at the
/// foot of a pane rather than in the middle of where its contents will go.
fn inner_last_line(area: Rect) -> Rect {
    let first = inner_first_line(area);
    Rect { y: area.y + area.height.saturating_sub(2).max(1), ..first }
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
    use crate::tui::app::Action;
    use crate::tui::worker::Event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
        assert_eq!(lit.spans[0].style.fg, Some(ACCENT));
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
        assert!(text.contains("▶"), "the queue marks the current track");
        assert!(
            !text.contains("repeat off") && !text.contains("dj off"),
            "modes that are off are not worth the width: {text}"
        );
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
        assert!(text.contains("Divergent Spectrum · 2011"), "{text}");
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
        let full = now_playing_card(&app, 60);
        let text: String =
            full.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
        assert!(text.contains("3:29 · 174 BPM · 8A (A minor) · 7/10 · 23 plays"), "{text}");

        // A bare file: no gaps where the missing facts would have gone.
        app.now_playing = Some(Track {
            filepath: "lib/Artist/untagged.mp3".into(),
            metadata: TrackMetadata::default(),
        });
        let bare = now_playing_card(&app, 60);
        let text: String =
            bare.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
        assert!(text.contains("unknown artist"), "{text}");
        assert!(text.contains("untagged.mp3"), "falls back to the filename: {text}");
        assert!(!text.contains("·"), "no separators with nothing to separate: {text}");
    }

    #[test]
    fn the_key_reuses_the_camelot_code_the_dj_panel_matches_on() {
        assert_eq!(
            track_facts(&TrackMetadata {
                musical_key: Some("A minor".into()),
                ..Default::default()
            }),
            "8A (A minor)"
        );
        // An unparseable tag is shown as written rather than dropped.
        assert_eq!(
            track_facts(&TrackMetadata {
                musical_key: Some("wonky".into()),
                ..Default::default()
            }),
            "wonky"
        );
    }

    #[test]
    fn the_now_playing_screen_says_so_when_nothing_is() {
        let mut app = connected_app();
        app.handle_action(Action::ToggleNowPlaying);
        let text = draw(&mut app);
        assert!(text.contains("nothing playing"), "{text}");
        assert!(text.contains("press 0 to go back"), "{text}");
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
        let text = draw(&mut app);
        assert!(text.contains("Queue (0)"));
        assert!(text.contains("'a' queues a track"));
    }

    #[test]
    fn the_queue_hint_sits_on_the_last_row_of_the_pane() {
        let mut app = connected_app();
        let text = draw(&mut app);
        let rows: Vec<&str> = text.lines().collect();
        let hint = rows
            .iter()
            .position(|row| row.contains("'a' queues a track"))
            .expect("the hint is drawn");
        let bottom = rows
            .iter()
            .rposition(|row| row.contains("└"))
            .expect("the queue pane has a bottom border");
        assert_eq!(hint + 1, bottom, "the hint is the row above the pane's bottom edge");
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
        assert!(draw(&mut app).contains("0 tracks"));
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
    fn the_server_label_sheds_detail_before_the_tabs_do() {
        let mut app = connected_app();
        app.username = Some("tester".into());

        // Roomy: the whole thing, scheme and all.
        assert!(draw_sized(&mut app, 140, 20).contains("tester@http://host:3000"));
        assert!(draw_sized(&mut app, 88, 20).contains("tester@http://host:3000"));

        // Genuinely tight: the scheme goes first, the host stays, and every
        // tab is still whole — including the fifth, which only exists on a
        // server with discovery and is what made the header longer.
        let narrow = draw_sized(&mut app, 72, 20);
        assert!(narrow.contains("tester@host:3000"), "kept the useful part");
        assert!(!narrow.contains("tester@http://host:3000"));
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
