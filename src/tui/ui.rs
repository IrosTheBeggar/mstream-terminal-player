//! Rendering. Reads app state, draws widgets — no decisions of its own.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Tabs, Wrap};

use crate::cmd_library::fmt_duration;

use super::app::{App, CONNECT_METHODS, ConnectStage, DjRow, Entry, Focus, MessageKind, Tab};
use super::worker::{AutoDjMode, LibraryNode};

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

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

    let [header, body, transport, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
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
    if app.show_help {
        render_help(frame, area);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let titles: Vec<Line> = Tab::ALL
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
            .select(app.tab.index())
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

    let items: Vec<ListItem> = app
        .pane()
        .entries
        .iter()
        .map(|entry| ListItem::new(entry_line(entry)))
        .collect();

    let empty = items.is_empty();
    let list = List::new(items)
        .block(bordered(title, focused))
        .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    let state = match app.tab {
        Tab::Files => &mut app.files.state,
        Tab::Library => &mut app.library.state,
        Tab::Playlists => &mut app.playlists.state,
        Tab::Search => &mut app.search.state,
    };
    frame.render_stateful_widget(list, area, state);

    if empty {
        let hint = match app.tab {
            Tab::Files => "(empty directory)",
            Tab::Library => "loading…",
            Tab::Playlists => "(no playlists)",
            Tab::Search => "type a query and press Enter",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(hint, Style::new().fg(DIM))),
            inner_first_line(area),
        );
    }
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
    }
}

fn entry_line(entry: &Entry) -> Line<'static> {
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
        Entry::Track { label, track } => {
            let mut spans = vec![Span::raw(label.clone())];
            if let Some(duration) = track.metadata.duration {
                spans.push(Span::styled(
                    format!("  [{}]", fmt_duration(duration)),
                    Style::new().fg(DIM),
                ));
            }
            Line::from(spans)
        }
    }
}

fn render_queue(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Queue;
    let title = format!(" Queue ({}) ", app.queue.items.len());

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
        frame.render_widget(
            Paragraph::new(Span::styled("'a' queues a track", Style::new().fg(DIM)))
                .wrap(Wrap { trim: true }),
            inner_first_line(area),
        );
    }
}

fn render_transport(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

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
    let volume = (app.volume * 100.0).round() as u32;
    let shuffle = if app.queue.shuffle { "on" } else { "off" };
    let full = format!(
        "vol {volume:>3}%  repeat {}  shuffle {shuffle}  dj {}",
        app.queue.repeat.label(),
        app.autodj.label()
    );
    // Abbreviate rather than let the mode readout crowd the message off the
    // line entirely.
    let compact = format!(
        "{volume:>3}%  rpt {}  shf {shuffle}  dj {}",
        app.queue.repeat.label(),
        app.autodj.label()
    );
    let modes = if area.width as usize >= full.chars().count() + 28 { full } else { compact };
    let modes_width = (modes.chars().count() as u16).min(area.width);

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

/// Shorten to `width`, marking the cut so it reads as elided rather than
/// broken off mid-word.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return String::new();
    }
    text.chars().take(width - 1).collect::<String>() + "…"
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

fn render_help(frame: &mut Frame, area: Rect) {
    let box_area = centered_rect(62, 18, area);
    frame.render_widget(Clear, box_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(" Keys ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let rows = [
        ("j / k / ↓ ↑", "move"),
        ("g / G", "first / last"),
        ("Enter / l", "open folder, or play from here"),
        ("h", "go back"),
        ("a", "add track to queue"),
        ("Tab", "switch browser / queue"),
        ("1 2 3 4", "Files / Library / Playlists / Search"),
        ("/", "search"),
        ("Space", "play or pause"),
        ("n / p", "next / previous track"),
        ("[ / ]", "seek 5s back / forward"),
        ("- / +", "volume"),
        ("d / C", "remove from queue / clear queue"),
        ("r / s", "repeat / shuffle"),
        ("A", "auto-dj: off / similar / tempo+key"),
        ("D", "auto-dj settings"),
        ("? / Esc", "toggle this help"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(
                    format!("  {keys:<14}"),
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::raw(*what),
            ])
        })
        .collect();
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
                .map(|f| FileEntry { name: (*f).to_string(), kind: Some("mp3".into()) })
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
        assert!(text.contains("repeat off"));
        assert!(text.contains("dj off"));
        assert!(text.contains("▶"), "the queue marks the current track");
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
        assert!(text.contains("[1:35]"));
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
    fn help_overlay_lists_bindings() {
        let mut app = connected_app();
        app.handle_action(Action::ToggleHelp);
        let text = draw(&mut app);
        assert!(text.contains("Keys"));
        assert!(text.contains("play or pause"));
        assert!(text.contains("seek 5s back / forward"));
    }

    #[test]
    fn empty_panes_explain_themselves() {
        let mut app = connected_app();
        let text = draw(&mut app);
        assert!(text.contains("Queue (0)"));
        assert!(text.contains("'a' queues a track"));
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

        // Roomy: the whole thing, scheme and all. Note this still fits at 76
        // columns now that the tabs only take what they need.
        assert!(draw_sized(&mut app, 140, 20).contains("tester@http://host:3000"));
        assert!(draw_sized(&mut app, 76, 20).contains("tester@http://host:3000"));

        // Genuinely tight: the scheme goes first, the host stays, and the
        // tabs are still whole.
        let narrow = draw_sized(&mut app, 62, 20);
        assert!(narrow.contains("tester@host:3000"), "kept the useful part");
        assert!(!narrow.contains("tester@http://host:3000"));
        assert!(narrow.contains("4:Search"));
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
        // And the mode readout abbreviates instead of crowding it out.
        assert!(text.contains("rpt off") || text.contains("repeat off"));
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
