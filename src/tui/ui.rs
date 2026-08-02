//! Rendering. Reads app state, draws widgets — no decisions of its own.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Tabs, Wrap};

use crate::cmd_library::fmt_duration;

use super::app::{App, CONNECT_METHODS, ConnectStage, Entry, Focus, MessageKind, Tab};
use super::worker::LibraryNode;

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;

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

    if app.show_help {
        render_help(frame, area);
    }
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let [tabs_area, server_area] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(40)]).areas(area);

    let titles: Vec<Line> = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| Line::from(format!(" {}:{} ", i + 1, t.title())))
        .collect();
    frame.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .divider(""),
        tabs_area,
    );

    let who = match &app.username {
        Some(user) => format!("{user}@{}", app.server),
        None => app.server.clone(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(who, Style::new().fg(DIM))))
            .alignment(Alignment::Right),
        server_area,
    );
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
    let [message_area, modes_area] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(48)]).areas(area);

    let message = match &app.message {
        Some(m) => Span::styled(
            m.text.clone(),
            match m.kind {
                MessageKind::Error => Style::new().fg(Color::Red),
                MessageKind::Info => Style::new().fg(DIM),
            },
        ),
        None => Span::styled("? help   q quit", Style::new().fg(DIM)),
    };
    frame.render_widget(Paragraph::new(Line::from(message)), message_area);

    let modes = format!(
        "vol {:>3}%  repeat {}  shuffle {}  dj {}",
        (app.volume * 100.0).round() as u32,
        app.queue.repeat.label(),
        if app.queue.shuffle { "on" } else { "off" },
        app.autodj.label()
    );
    frame.render_widget(
        Paragraph::new(Span::styled(modes, Style::new().fg(DIM))).alignment(Alignment::Right),
        modes_area,
    );
}

fn render_connect(frame: &mut Frame, area: Rect, app: &App) {
    // Reconnecting from a saved session: don't flash a login form (and a
    // password field) at someone who never asked to sign in.
    if app.connecting && !app.connect.submitting {
        render_connecting(frame, area, app);
        return;
    }

    match app.connect.stage {
        ConnectStage::Choosing => return render_connect_choice(frame, area, app),
        ConnectStage::QuickConnect => return render_connect_quick(frame, area, app),
        ConnectStage::Direct => {}
    }

    let box_area = centered_rect(70, 13, area);
    frame.render_widget(Clear, box_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(" Connect to mStream ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let field = |index: usize, label: &str, value: String| -> Line<'static> {
        let marker = if app.connect.field == index { "> " } else { "  " };
        let style = if app.connect.field == index {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<10}"), style),
            Span::raw(value),
            Span::styled(if app.connect.field == index { "▏" } else { "" }, style),
        ])
    };

    let mut lines = vec![
        field(0, "Server", app.connect.server.clone()),
        field(1, "Username", app.connect.username.clone()),
        field(2, "Password", "•".repeat(app.connect.password.chars().count())),
        Line::raw(""),
        Line::from(Span::styled(
            "Leave the username empty for a server in public mode.",
            Style::new().fg(DIM),
        )),
        Line::from(Span::styled(
            "Tab/↑↓ switch fields · Enter connects · Esc back · Ctrl+C quits",
            Style::new().fg(DIM),
        )),
    ];

    if app.connecting {
        lines.push(Line::from(Span::styled("connecting…", Style::new().fg(ACCENT))));
    }
    if let Some(message) = &app.message {
        lines.push(Line::from(Span::styled(
            message.text.clone(),
            match message.kind {
                MessageKind::Error => Style::new().fg(Color::Red),
                MessageKind::Info => Style::new().fg(DIM),
            },
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Step one: how do you want to reach the server?
fn render_connect_choice(frame: &mut Frame, area: Rect, app: &App) {
    let box_area = centered_rect(66, 11, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(" Connect to mStream ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let mut lines = vec![Line::from("How do you want to connect?"), Line::raw("")];
    for (i, (name, blurb)) in CONNECT_METHODS.iter().enumerate() {
        let selected = app.connect.choice == i;
        let style = if selected {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} {name:<14}", if selected { ">" } else { " " }), style),
            Span::styled((*blurb).to_string(), Style::new().fg(DIM)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "↑↓ choose · Enter continue · Ctrl+C quits",
        Style::new().fg(DIM),
    )));
    if let Some(message) = &app.message {
        lines.push(Line::from(Span::styled(
            message.text.clone(),
            match message.kind {
                MessageKind::Error => Style::new().fg(Color::Red),
                MessageKind::Info => Style::new().fg(DIM),
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Step two, Quick Connect branch: paste the pairing code.
fn render_connect_quick(frame: &mut Frame, area: Rect, app: &App) {
    let box_area = centered_rect(72, 12, area);
    frame.render_widget(Clear, box_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(" Quick Connect ");
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let code = app.connect.code.trim();
    // The code is a few hundred characters — show enough to recognise it,
    // plus a length so a paste is visibly confirmed.
    let shown = if code.is_empty() {
        Span::styled("paste the code here", Style::new().fg(DIM))
    } else {
        let head: String = code.chars().take(28).collect();
        Span::raw(format!("{head}…  ({} characters)", code.chars().count()))
    };

    let mut lines = vec![
        Line::from(Span::styled(
            "Paste the pairing code from your server's admin page.",
            Style::new().fg(DIM),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("> ", Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
            shown,
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "The code opens a tunnel — you'll still sign in afterwards.",
            Style::new().fg(DIM),
        )),
        Line::from(Span::styled("Enter connects · Esc back", Style::new().fg(DIM))),
    ];
    if app.connecting {
        lines.push(Line::from(Span::styled("dialling…", Style::new().fg(ACCENT))));
    }
    if let Some(message) = &app.message {
        lines.push(Line::from(Span::styled(
            message.text.clone(),
            match message.kind {
                MessageKind::Error => Style::new().fg(Color::Red),
                MessageKind::Info => Style::new().fg(DIM),
            },
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_connecting(frame: &mut Frame, area: Rect, app: &App) {
    let box_area = centered_rect(60, 4, area);
    frame.render_widget(Clear, box_area);

    let block = Block::default().borders(Borders::ALL).border_style(Style::new().fg(ACCENT));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("Connecting to {}…", app.server),
                Style::new().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled("Ctrl+C to quit", Style::new().fg(DIM))),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        inner,
    );
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
        let mut terminal = Terminal::new(TestBackend::new(90, 26)).unwrap();
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

    fn connected_app() -> App {
        let mut app = App::new(Some("http://host:3000".into()), Some("tok".into()), None);
        app.connected = true;
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
        assert!(text.contains("Connect to mStream"));
        assert!(text.contains("alice"));
        assert!(text.contains("••••••"), "password is masked");
        assert!(!text.contains("secret"), "password is never drawn in the clear");
    }

    #[test]
    fn the_first_screen_asks_how_to_connect() {
        let mut app = App::new(None, None, None);
        let text = draw(&mut app);
        assert!(text.contains("How do you want to connect?"));
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
    fn renders_in_a_small_terminal_without_panicking() {
        let mut app = connected_app();
        app.apply_event(Event::Listing(Box::new(listing("/lib/", &[], &["a.mp3"]))));
        for (w, h) in [(20u16, 8u16), (40, 10), (200, 60)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| render(frame, &mut app)).unwrap();
        }
    }
}
