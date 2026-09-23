//! Auto DJ in the GUI shell (docs/ux-contracts/auto-dj.md): the room, a
//! Library nav room under TOOLS (clauses 40–53), the empty-queue chooser as a kit
//! modal (clause 2), the opening-song banner (clause 4), and the acts the
//! queue panel's empty state sends (clause 16). The bar's `auto-dj` toggle
//! and `A` ride the shared App's toggle (entry point 1); every edit here
//! goes through the App's [`DjEdit`], so both shells change one model.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Block;
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::config;
use crate::dj::{self, EmptyQueueStart, GenreMode, SonicAnchor};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, Surface, cursor_ring, dim, modal_close, modal_frame_on, scroll_list, table_view, tall_button, tall_frame, wrap_words};
use crate::tui::app::{Action, Capture, DjEdit, DjRow};

use super::{Act, DJ_NAV, Gui, List, Screen, accent, bright_bold, put, sel, text_button};

/// The chooser's three rows: the two answers, then the remember box.
const ROWS: usize = 3;
/// A room row's label column, and where its description starts.
const LABEL_W: u16 = 18;
const DESC_X: u16 = 28;
/// The bars' cells — the sonic room's ten.
const CELLS: u16 = 10;
/// The keyword field's width.
const FIELD_W: u16 = 24;

// ── State ───────────────────────────────────────────────────────────────────

/// The room's own state: what the App does not hold because only this shell
/// needs it — the keyboard cursor over the room's controls, the scroll, the
/// text fields, the pickers' cursors.
#[derive(Debug, Default)]
pub(crate) struct DjUi {
    /// The keyboard cursor: None is stowed (the kit's resting state).
    pub cursor: Option<Item>,
    /// The room's body: its first visible row, and whether the next draw
    /// scrolls the cursor into view.
    pub body: ListView,
    /// The keyword field (clause 49).
    pub keyword: Input,
    /// The sub-cursor along a chips row.
    pub chip: usize,
    /// The genre picker's search line and cursor (clause 48).
    pub filter: Input,
    pub pick_row: usize,
    pub genres: ListView,
    /// The server picker (clause 41), when open: its cursor row.
    pub server_pick: Option<usize>,
    /// The body's window from the last draw, for the page keys.
    visible: usize,
    /// How many Preview picks the last draw saw: new ones scroll into view
    /// under their row, which is the body's last.
    samples_seen: usize,
}

impl DjUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// A control the keyboard cursor can rest on, in the room's order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Item {
    /// Start / Stop.
    Toggle,
    /// The DJ's server (clause 41).
    Server,
    /// A shared row: a switch, a bar, or Preview.
    Row(DjRow),
    Anchor(SonicAnchor),
    EmptyQueue(EmptyQueueStart),
    GenreMode(GenreMode),
    GenreChips,
    PickGenres,
    KeywordChips,
    KeywordInput,
    /// One of the DJ server's libraries (clause 42).
    Source(usize),
}

/// One line of the room's body, laid out from the App's rows.
enum L {
    Section(String),
    /// A label with a dim description, over radio lines.
    Label(String, String),
    Blank,
    Text(String, Tone),
    /// A dim hint under a switch, indented like the chips it stands for.
    Hint(String),
    /// Start / Stop: three rows tall.
    Button,
    Server,
    Switch(DjRow),
    Bar(DjRow),
    Radio(Item, String, String),
    Chips(Item),
    PickGenres,
    KeywordInput,
    Source(usize, String, bool),
    Preview,
    Sample(String),
}

#[derive(Clone, Copy)]
enum Tone {
    Bold,
    Dim,
    Gold,
}

impl L {
    fn height(&self) -> usize {
        match self {
            L::Button => 3,
            _ => 1,
        }
    }

    fn item(&self) -> Option<Item> {
        match self {
            L::Button => Some(Item::Toggle),
            L::Server => Some(Item::Server),
            L::Switch(row) | L::Bar(row) => Some(Item::Row(*row)),
            L::Radio(item, ..) | L::Chips(item) => Some(*item),
            L::PickGenres => Some(Item::PickGenres),
            L::KeywordInput => Some(Item::KeywordInput),
            L::Source(i, ..) => Some(Item::Source(*i)),
            L::Preview => Some(Item::Row(DjRow::Sample)),
            L::Section(_) | L::Label(..) | L::Blank | L::Text(..) | L::Hint(_) | L::Sample(_) => None,
        }
    }
}

fn is_bar(row: DjRow) -> bool {
    matches!(
        row,
        DjRow::SongsPerFetch
            | DjRow::Strictness
            | DjRow::Tolerance
            | DjRow::Cooldown
            | DjRow::Rating
            | DjRow::Shortest
            | DjRow::Longest
    )
}

/// The section a shared row sits under (clause 2's order).
fn section_of(row: DjRow) -> Option<&'static str> {
    match row {
        DjRow::Armed | DjRow::Sample => None,
        DjRow::SongsPerFetch => Some("queue"),
        DjRow::Sonic
        | DjRow::Strictness
        | DjRow::Anchor
        | DjRow::EmptyQueue
        | DjRow::Bpm
        | DjRow::Tolerance
        | DjRow::Harmonic
        | DjRow::Cooldown => Some("continuity"),
        DjRow::Rating
        | DjRow::Length
        | DjRow::Shortest
        | DjRow::Longest
        | DjRow::UnknownLength
        | DjRow::Genres
        | DjRow::Keywords => Some("filters"),
        DjRow::Sources => Some("sources"),
    }
}

fn section_title(key: &str) -> String {
    match key {
        "queue" => t!("gui.dj.sec_queue"),
        "continuity" => t!("gui.dj.sec_continuity"),
        "filters" => t!("gui.dj.sec_filters"),
        _ => t!("gui.dj.sec_sources"),
    }
    .to_string()
}

/// A saved server the picker offers (clause 41).
struct PickServer {
    id: String,
    label: String,
    peer: bool,
    current: bool,
}

/// Every selectable saved server, grouped — a peer under its parent, named
/// through it — with the DJ's own marked.
fn pickable_servers(gui: &Gui) -> Vec<PickServer> {
    let dj = gui.app.dj_server.clone();
    let servers = &gui.config.servers;
    config::grouped_order(servers)
        .into_iter()
        .filter_map(|i| {
            let entry = &servers[i];
            if !config::selectable(entry) {
                return None;
            }
            let label = match &entry.peer {
                Some(peer) => {
                    let parent = super::servers::parent_label(gui, &peer.parent);
                    format!("{} · {}", peer.name, t!("gui.srv.via", parent = parent))
                }
                None => config::display_name(entry),
            };
            Some(PickServer {
                id: entry.url.clone(),
                label,
                peer: entry.peer.is_some(),
                current: dj.as_deref().is_some_and(|d| config::same_server(d, &entry.url)),
            })
        })
        .collect()
}

// ── Words ───────────────────────────────────────────────────────────────────

/// "{n} songs" in the locale's plural form: the record's ICU plural, by
/// hand, for the two languages here with more than one plural.
fn songs_words(count: u32) -> String {
    let locale = rust_i18n::locale();
    let lang: &str = &locale;
    let few_many = |n: u32| (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100));
    let form = match &lang[..lang.len().min(2)] {
        "pl" if count == 1 => "one",
        "pl" if few_many(count) => "few",
        "pl" => "many",
        "ru" if count % 10 == 1 && count % 100 != 11 => "one",
        "ru" if few_many(count) => "few",
        "ru" => "many",
        "ja" | "zh" => "other",
        _ if count == 1 => "one",
        _ => "other",
    };
    match form {
        "one" => t!("gui.dj.songs_one", count = count),
        "few" => t!("gui.dj.songs_few", count = count),
        "many" => t!("gui.dj.songs_many", count = count),
        _ => t!("gui.dj.songs_other", count = count),
    }
    .to_string()
}

/// The length window's words (clause 47): a rail means unbounded, so a
/// bare 0:00–20:00 never looks like a constraint.
fn length_words(s: &dj::Settings) -> String {
    match (s.min_seconds > 0, s.max_seconds < dj::LENGTH_RAIL_SECONDS) {
        (false, false) => t!("gui.dj.length_any"),
        (true, false) => t!("gui.dj.length_over", min = crate::api::types::fmt_duration(f64::from(s.min_seconds))),
        (false, true) => t!("gui.dj.length_under", max = crate::api::types::fmt_duration(f64::from(s.max_seconds))),
        (true, true) => t!("gui.dj.length_between", min = crate::api::types::fmt_duration(f64::from(s.min_seconds)), max = crate::api::types::fmt_duration(f64::from(s.max_seconds))),
    }
    .to_string()
}

fn sample_words(track: &crate::api::types::Track) -> String {
    let title = track.title_or_file().to_string();
    match track.metadata.artist.as_deref().filter(|a| !a.is_empty()) {
        Some(artist) => format!("{title} — {artist}"),
        None => title,
    }
}

// ── Layout ──────────────────────────────────────────────────────────────────

/// The room's body, line by line, from the App's rows (which already carry
/// the version gates, the peer's missing rating row and the sources row —
/// clauses 42–50): STATUS, then QUEUE · CONTINUITY · FILTERS · SOURCES,
/// then Preview.
fn lines(gui: &Gui, width: usize) -> Vec<L> {
    let app = &gui.app;
    let s = &app.dj;
    let library = app.dj_library();
    let armed = app.dj_armed();
    let mut v = vec![L::Section(t!("gui.dj.sec_status").to_string())];
    if armed {
        v.push(L::Text(t!("gui.dj.status_on").to_string(), Tone::Bold));
        v.push(L::Text(t!("gui.dj.status_on_detail", server = app.dj_server_name()).to_string(), Tone::Dim));
    } else {
        v.push(L::Text(t!("gui.dj.status_off").to_string(), Tone::Bold));
        v.push(L::Text(t!("gui.dj.status_off_detail").to_string(), Tone::Dim));
    }
    v.push(L::Button);
    if armed && pickable_servers(gui).len() > 1 {
        v.push(L::Server);
    }
    let version = app.dj_info.as_ref().and_then(|i| i.version.as_deref());
    let gated = dj::known_older(version, dj::FLOOR_FILTERS);
    let mut section: Option<&str> = None;
    for row in app.dj_panel.rows.clone() {
        let here = section_of(row);
        if here != section {
            // A server known to predate the filters says so where they
            // would have been (clause 50).
            if section == Some("continuity") && gated {
                v.push(L::Text(t!("gui.dj.needs_newer").to_string(), Tone::Gold));
            }
            if let Some(key) = here {
                v.push(L::Blank);
                v.push(L::Section(section_title(key)));
            }
            section = here;
        }
        match row {
            DjRow::Armed => {}
            DjRow::SongsPerFetch
            | DjRow::Strictness
            | DjRow::Tolerance
            | DjRow::Cooldown
            | DjRow::Rating
            | DjRow::Shortest
            | DjRow::Longest => v.push(L::Bar(row)),
            DjRow::Sonic | DjRow::Bpm | DjRow::Harmonic | DjRow::Length | DjRow::UnknownLength => {
                v.push(L::Switch(row));
            }
            DjRow::Anchor => {
                v.push(L::Label(t!("gui.dj.anchor").to_string(), String::new()));
                v.push(L::Radio(
                    Item::Anchor(SonicAnchor::Rolling),
                    t!("gui.dj.anchor_rolling").to_string(),
                    t!("gui.dj.anchor_rolling_sub").to_string(),
                ));
                v.push(L::Radio(
                    Item::Anchor(SonicAnchor::Locked),
                    t!("gui.dj.anchor_locked").to_string(),
                    t!("gui.dj.anchor_locked_sub").to_string(),
                ));
            }
            DjRow::EmptyQueue => {
                v.push(L::Label(t!("gui.dj.empty_queue").to_string(), t!("gui.dj.empty_queue_sub").to_string()));
                v.push(L::Radio(Item::EmptyQueue(EmptyQueueStart::Ask), t!("gui.dj.ask").to_string(), String::new()));
                v.push(L::Radio(
                    Item::EmptyQueue(EmptyQueueStart::Random),
                    t!("gui.dj.surprise").to_string(),
                    t!("gui.dj.surprise_sub").to_string(),
                ));
                v.push(L::Radio(
                    Item::EmptyQueue(EmptyQueueStart::Pick),
                    t!("gui.dj.pick").to_string(),
                    t!("gui.dj.pick_sub").to_string(),
                ));
            }
            DjRow::Genres => {
                v.push(L::Switch(row));
                if library.genre_filter {
                    v.push(L::Radio(
                        Item::GenreMode(GenreMode::Whitelist),
                        t!("gui.dj.whitelist").to_string(),
                        t!("gui.dj.whitelist_sub").to_string(),
                    ));
                    v.push(L::Radio(
                        Item::GenreMode(GenreMode::Blacklist),
                        t!("gui.dj.blacklist").to_string(),
                        t!("gui.dj.blacklist_sub").to_string(),
                    ));
                    if library.genres.is_empty() {
                        v.push(L::Hint(t!("gui.dj.no_genres_selected").to_string()));
                    } else {
                        v.push(L::Chips(Item::GenreChips));
                    }
                    v.push(L::PickGenres);
                }
            }
            DjRow::Keywords => {
                v.push(L::Switch(row));
                if s.keyword_filter {
                    if s.keywords.is_empty() {
                        v.push(L::Hint(t!("gui.dj.no_keywords").to_string()));
                    } else {
                        v.push(L::Chips(Item::KeywordChips));
                    }
                    v.push(L::KeywordInput);
                }
            }
            DjRow::Sources => {
                let off = app.dj_sources_off();
                let libraries = app.dj_info.as_ref().map(|i| i.libraries.clone()).unwrap_or_default();
                for (i, name) in libraries.into_iter().enumerate() {
                    let on = !off.contains(&name);
                    v.push(L::Source(i, name, on));
                }
            }
            DjRow::Sample => {
                v.push(L::Blank);
                v.push(L::Preview);
                for track in &app.dj_panel.sample {
                    v.push(L::Sample(sample_words(track)));
                }
            }
        }
    }
    // A sentence wraps to the body rather than clipping mid-thought.
    let mut wrapped = Vec::with_capacity(v.len());
    for line in v {
        match line {
            L::Text(text, tone) => {
                for part in wrap_words(&text, width.max(20)) {
                    wrapped.push(L::Text(part, tone));
                }
            }
            L::Hint(text) => {
                for part in wrap_words(&text, width.saturating_sub(4).max(16)) {
                    wrapped.push(L::Hint(part));
                }
            }
            other => wrapped.push(other),
        }
    }
    wrapped
}

/// The controls in the room's order — what ↑↓ walk.
fn items(gui: &Gui) -> Vec<Item> {
    lines(gui, usize::MAX).iter().filter_map(L::item).collect()
}

// ── The room ────────────────────────────────────────────────────────────────

/// The nav's Auto DJ row (entry point 2): the room opens with its cursor
/// stowed and, with the DJ off, asks the session's server what it offers so
/// the rows fit it (clause 50). The hub calls this from `Act::Nav`.
pub(crate) fn open_room(gui: &mut Gui) {
    gui.dj.cursor = None;
    gui.dj.body.scroll = 0;
    let effects = gui.app.dj_room_opened();
    gui.pend(effects);
}

/// Whether one of the DJ's modals owns the pointer and keyboard.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.app.dj_chooser.is_some() || gui.app.dj_panel.genres.is_some() || gui.dj.server_pick.is_some()
}

/// Whether the room is the Library's active one — a nav room, so the
/// Now Playing screen over it counts as away.
fn in_room(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == DJ_NAV
}

/// A row's label: the accent while the keyboard cursor is on it, bright
/// under the pointer (the Settings rows' grammar).
fn row_style(focused: bool, hover: bool) -> Style {
    match (focused, hover) {
        (true, _) => accent().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => Style::default(),
    }
}

/// Where a row's description starts: the shared column, pushed right by a
/// label too wide for it (a translation, the unknown-length switch) rather
/// than written through.
fn desc_x(label: &str, indent: u16) -> u16 {
    (indent + label.chars().count() as u16 + 2).max(DESC_X)
}

/// The room (clauses 40–53): the name, the state at the right; then the
/// body, scrolled, with the kit's scrollbar on overflow. A nav room has no
/// way back — the nav column is right there.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    put(frame, content.x, content.y, &t!("gui.dj.title"), Style::default().add_modifier(Modifier::BOLD));
    let (state, style) = if gui.app.dj_armed() {
        (t!("gui.dj.state_on", server = gui.app.dj_server_name()).to_string(), Style::default().fg(th().ok))
    } else {
        (t!("gui.dj.state_off").to_string(), dim())
    };
    let shown = super::bar::clip(&state, content.width.saturating_sub(14) as usize);
    put(frame, content.right().saturating_sub(shown.chars().count() as u16), content.y, &shown, style);

    let region = Rect {
        x: content.x,
        y: content.y + 2,
        width: content.width,
        height: content.height.saturating_sub(2),
    };
    // Laid out for the narrower body; a room that fits without a scrollbar
    // wraps a cell or two early, which nobody sees.
    let lines = lines(gui, region.width.saturating_sub(2) as usize);
    let mut tops = Vec::with_capacity(lines.len());
    let mut total = 0usize;
    for line in &lines {
        tops.push(total);
        total += line.height();
    }
    let visible = region.height as usize;
    // Preview's picks land after the click, under the body's last row:
    // they come into view on their own while the cursor is on the row.
    let samples = gui.app.dj_panel.sample.len();
    if samples != gui.dj.samples_seen {
        gui.dj.samples_seen = samples;
        if gui.dj.cursor == Some(Item::Row(DjRow::Sample)) {
            gui.dj.body.reveal = true;
        }
    }
    if std::mem::take(&mut gui.dj.body.reveal)
        && let Some(i) = lines.iter().position(|l| l.item().is_some() && l.item() == gui.dj.cursor)
    {
        let (top, mut h) = (tops[i], lines[i].height());
        if lines[i].item() == Some(Item::Row(DjRow::Sample)) {
            h += lines[i + 1..].iter().take_while(|l| matches!(l, L::Sample(_))).count();
        }
        if top < gui.dj.body.scroll {
            gui.dj.body.scroll = top;
        } else if top + h > gui.dj.body.scroll + visible {
            gui.dj.body.scroll = (top + h).saturating_sub(visible);
        }
    }
    gui.dj.body.scroll = gui.dj.body.scroll.min(total.saturating_sub(visible));
    gui.dj.visible = visible;
    let overflow = total > visible;
    let body = Rect {
        x: region.x,
        y: region.y,
        width: if overflow { region.width.saturating_sub(2) } else { region.width },
        height: region.height,
    };
    let scroll = gui.dj.body.scroll;
    for (i, line) in lines.iter().enumerate() {
        let (top, h) = (tops[i], line.height());
        if top + h <= scroll {
            continue;
        }
        if top >= scroll + visible {
            break;
        }
        // A tall control half off the window waits for the scroll.
        if top < scroll || top + h > scroll + visible {
            continue;
        }
        let y = body.y + (top - scroll) as u16;
        draw_line(frame, gui, body, y, line);
    }
    if overflow {
        let bar = Rect { x: region.right().saturating_sub(1), y: region.y, width: 1, height: region.height };
        scroll_list(frame, &mut gui.ui, bar, total, visible, scroll, Act::ScrollBy(List::DjRoom, -1), Act::ScrollBy(List::DjRoom, 1), |first| Act::ScrollTo(List::DjRoom, first));
    }
}

fn draw_line(frame: &mut Frame, gui: &mut Gui, body: Rect, y: u16, line: &L) {
    let (x, w) = (body.x, body.width);
    let forward = super::forward_glyph();
    let (check_on, check_off) = super::check_glyphs();
    match line {
        L::Section(text) => put(frame, x, y, text, dim()),
        L::Blank => {}
        L::Hint(text) => put(frame, x + 4, y, &super::bar::clip(text, w.saturating_sub(4) as usize), dim()),
        L::Label(label, sub) => {
            put(frame, x, y, label, Style::default());
            let dx = desc_x(label, 0);
            let avail = w.saturating_sub(dx) as usize;
            if !sub.is_empty() && avail >= 10 {
                put(frame, x + dx, y, &super::bar::clip(sub, avail), dim());
            }
        }
        L::Text(text, tone) => {
            let style = match tone {
                Tone::Bold => Style::default().add_modifier(Modifier::BOLD),
                Tone::Dim => dim(),
                Tone::Gold => Style::default().fg(th().gold),
            };
            put(frame, x, y, &super::bar::clip(text, w as usize), style);
        }
        L::Button => {
            let armed = gui.app.dj_armed();
            let at = Rect { x, y, width: w, height: 3 };
            let rect = if armed {
                tall_danger(frame, &mut gui.ui, at, &t!("gui.dj.stop"), Act::DjStartStop)
            } else {
                let label = format!("{} {forward}", t!("gui.dj.start"));
                tall_button(frame, &mut gui.ui, at, &label, true, Act::DjStartStop)
            };
            if gui.dj.cursor == Some(Item::Toggle) {
                cursor_ring(frame, rect, Style::default().fg(th().bright).add_modifier(Modifier::BOLD));
            }
        }
        L::Server => {
            let rect = Rect { x, y, width: w, height: 1 };
            let focused = gui.dj.cursor == Some(Item::Server);
            let hover = gui.ui.hovers(rect);
            put(frame, x, y, &t!("gui.dj.server"), row_style(focused, hover));
            let name = format!("{} {forward}", gui.app.dj_server_name());
            let shown = super::bar::clip(&name, w.saturating_sub(LABEL_W) as usize);
            let style = if hover { bright_bold() } else { Style::default().add_modifier(Modifier::BOLD) };
            put(frame, x + LABEL_W, y, &shown, style);
            gui.ui.click(rect, Act::DjServerPick);
            gui.ui.tip(rect, t!("gui.dj.server_tip").to_string());
        }
        L::Switch(row) => draw_switch(frame, gui, x, y, w, *row, check_on, check_off),
        L::Bar(row) => draw_bar(frame, gui, x, y, w, *row),
        L::Radio(item, label, hint) => {
            let on = radio_on(gui, *item);
            let rect = Rect { x: x + 2, y, width: w.saturating_sub(2), height: 1 };
            let focused = gui.dj.cursor == Some(*item);
            let hover = gui.ui.hovers(rect);
            let glyph = match (on, legacy_conhost()) {
                (true, true) => "(*)",
                (true, false) => "(•)",
                (false, _) => "( )",
            };
            put(frame, x + 2, y, glyph, if on { Style::default().fg(th().ok) } else { dim() });
            let style = match (focused, hover, on) {
                (true, _, _) => accent().add_modifier(Modifier::BOLD),
                (false, true, _) => bright_bold(),
                (false, false, true) => Style::default().add_modifier(Modifier::BOLD),
                (false, false, false) => Style::default(),
            };
            put(frame, x + 6, y, label, style);
            if !hint.is_empty() {
                let hx = x + 6 + label.chars().count() as u16 + 1;
                let avail = (x + w).saturating_sub(hx) as usize;
                if avail >= 10 {
                    put(frame, hx, y, &super::bar::clip(&format!("— {hint}"), avail), dim());
                }
            }
            gui.ui.click(rect, radio_act(*item));
        }
        L::Chips(item) => draw_chips(frame, gui, x + 4, y, w.saturating_sub(4), *item),
        L::PickGenres => {
            let label = format!("{} {forward}", t!("gui.dj.pick_genres"));
            let rect = Rect { x: x + 4, y, width: label.chars().count() as u16, height: 1 };
            let focused = gui.dj.cursor == Some(Item::PickGenres);
            let hover = gui.ui.hovers(rect);
            let style = match (focused, hover) {
                (true, _) => accent().add_modifier(Modifier::BOLD),
                (false, true) => bright_bold(),
                (false, false) => accent(),
            };
            put(frame, rect.x, y, &label, style);
            gui.ui.click(rect, Act::DjPickGenres);
        }
        L::KeywordInput => draw_keyword_input(frame, gui, x + 4, y, w.saturating_sub(4), forward),
        L::Source(i, name, on) => {
            let rect = Rect { x, y, width: w, height: 1 };
            let focused = gui.dj.cursor == Some(Item::Source(*i));
            let hover = gui.ui.hovers(rect);
            put(frame, x, y, if *on { check_on } else { check_off }, if *on { Style::default().fg(th().ok) } else { dim() });
            put(frame, x + 4, y, &super::bar::clip(name, w.saturating_sub(4) as usize), row_style(focused, hover));
            gui.ui.click(rect, Act::DjSource(name.clone()));
        }
        L::Preview => {
            let focused = gui.dj.cursor == Some(Item::Row(DjRow::Sample));
            if gui.app.dj_panel.sample_pending {
                put(frame, x, y, &t!("gui.dj.picking"), accent());
            } else {
                let label = format!("{} {forward}", t!("gui.dj.preview"));
                let rect = Rect { x, y, width: label.chars().count() as u16, height: 1 };
                let hover = gui.ui.hovers(rect);
                let style = match (focused, hover) {
                    (true, _) => accent().add_modifier(Modifier::BOLD),
                    (false, true) => bright_bold(),
                    (false, false) => accent(),
                };
                put(frame, x, y, &label, style);
                gui.ui.click(rect, Act::DjPreview);
            }
            if let Some(pool) = &gui.app.dj_panel.pool {
                let words = t!("gui.dj.pool", n = pool.pool_size).to_string();
                let avail = w.saturating_sub(DESC_X) as usize;
                if avail >= 10 {
                    put(frame, x + DESC_X, y, &super::bar::clip(&words, avail), dim());
                }
            }
        }
        L::Sample(text) => put(frame, x + 2, y, &super::bar::clip(text, w.saturating_sub(2) as usize), Style::default()),
    }
}

fn radio_on(gui: &Gui, item: Item) -> bool {
    match item {
        Item::Anchor(anchor) => gui.app.dj.sonic_anchor == anchor,
        Item::EmptyQueue(choice) => gui.app.dj.empty_queue == choice,
        Item::GenreMode(mode) => gui.app.dj_library().genre_mode == mode,
        _ => false,
    }
}

fn radio_act(item: Item) -> Act {
    match item {
        Item::Anchor(anchor) => Act::DjAnchor(anchor),
        Item::EmptyQueue(choice) => Act::DjEmpty(choice),
        Item::GenreMode(mode) => Act::DjGenreMode(mode),
        other => Act::DjFocus(other),
    }
}

/// A `[✓]` row (clauses 43–49): the glyph, the label, the description
/// clipped at the right — or, for a sonic row the server cannot honour,
/// the reason in gold. The switch stays live either way: a default-on
/// setting that could not be switched off would be a trap.
#[allow(clippy::too_many_arguments)]
fn draw_switch(
    frame: &mut Frame,
    gui: &mut Gui,
    x: u16,
    y: u16,
    w: u16,
    row: DjRow,
    check_on: &str,
    check_off: &str,
) {
    let s = &gui.app.dj;
    let (on, label, desc, reason): (bool, String, String, Option<String>) = match row {
        DjRow::Sonic => (
            s.sonic,
            t!("gui.dj.sonic").to_string(),
            t!("gui.dj.sonic_sub").to_string(),
            gui.app.dj_sonic_reason(),
        ),
        DjRow::Bpm => (s.bpm, t!("gui.dj.bpm").to_string(), t!("gui.dj.bpm_sub").to_string(), None),
        DjRow::Harmonic => {
            let anchored = gui.app.lane.camelot_anchor.as_ref().filter(|_| s.harmonic);
            let desc = match anchored {
                Some(anchor) => format!("{} · {}", t!("gui.dj.anchored_on", key = anchor), t!("gui.dj.harmonic_sub")),
                None => t!("gui.dj.harmonic_sub").to_string(),
            };
            (s.harmonic, t!("gui.dj.harmonic").to_string(), desc, None)
        }
        DjRow::Length => (
            s.length,
            t!("gui.dj.length").to_string(),
            format!("{} · {}", length_words(s), t!("gui.dj.length_sub")),
            None,
        ),
        DjRow::UnknownLength => (
            s.allow_unknown_length,
            t!("gui.dj.unknown_length").to_string(),
            t!("gui.dj.unknown_length_sub").to_string(),
            None,
        ),
        DjRow::Genres => (
            gui.app.dj_library().genre_filter,
            t!("gui.dj.genre").to_string(),
            t!("gui.dj.genre_sub").to_string(),
            None,
        ),
        DjRow::Keywords => (
            s.keyword_filter,
            t!("gui.dj.keyword").to_string(),
            t!("gui.dj.keyword_sub").to_string(),
            None,
        ),
        _ => return,
    };
    let rect = Rect { x, y, width: w, height: 1 };
    let focused = gui.dj.cursor == Some(Item::Row(row));
    let hover = gui.ui.hovers(rect);
    let glyph_style = if on && reason.is_none() { Style::default().fg(th().ok) } else { dim() };
    put(frame, x, y, if on { check_on } else { check_off }, glyph_style);
    put(frame, x + 4, y, &label, row_style(focused, hover));
    let dx = desc_x(&label, 4);
    let avail = w.saturating_sub(dx) as usize;
    if avail >= 10 {
        match &reason {
            Some(reason) => put(frame, x + dx, y, &super::bar::clip(reason, avail), Style::default().fg(th().gold)),
            None => put(frame, x + dx, y, &super::bar::clip(&desc, avail), dim()),
        }
    }
    gui.ui.click(rect, Act::DjStep(row, 1));
    gui.ui.tip(rect, reason.unwrap_or(desc));
}

/// What a bar row shows and moves within.
struct BarSpec {
    label: String,
    lo: u32,
    hi: u32,
    value: u32,
    words: String,
    note: String,
    tip: String,
    /// Seconds snap to the length window's step.
    seconds: bool,
}

fn bar_spec(gui: &Gui, row: DjRow) -> Option<BarSpec> {
    let s = &gui.app.dj;
    let spec = match row {
        DjRow::SongsPerFetch => BarSpec {
            label: t!("gui.dj.songs_per_fetch").to_string(),
            lo: 1,
            hi: dj::SONGS_PER_FETCH_MAX,
            value: s.songs_per_fetch,
            words: songs_words(s.songs_per_fetch),
            note: String::new(),
            tip: t!("gui.dj.songs_per_fetch_sub").to_string(),
            seconds: false,
        },
        DjRow::Strictness => {
            let pct = (s.sonic_min_similarity * 100.0).round() as u32;
            let mut note = t!("gui.dj.cosine", value = format!("{:.2}", s.sonic_min_similarity)).to_string();
            if let Some(pool) = &gui.app.dj_panel.pool {
                note = format!("{note} · {}", t!("gui.dj.pool_short", n = pool.pool_size));
            }
            BarSpec {
                label: t!("gui.dj.strictness").to_string(),
                lo: (dj::SONIC_MIN_SIMILARITY * 100.0).round() as u32,
                hi: (dj::SONIC_MAX_SIMILARITY * 100.0).round() as u32,
                value: pct,
                words: t!("gui.dj.strictness_value", pct = pct).to_string(),
                note,
                tip: t!("gui.dj.sonic_sub").to_string(),
                seconds: false,
            }
        }
        DjRow::Tolerance => BarSpec {
            label: t!("gui.dj.tolerance").to_string(),
            lo: dj::BPM_TOLERANCE_MIN,
            hi: dj::BPM_TOLERANCE_MAX,
            value: s.bpm_tolerance,
            words: t!("gui.dj.tolerance_value", bpm = s.bpm_tolerance).to_string(),
            note: t!("gui.dj.tolerance_wide", bpm = s.bpm_tolerance + dj::BPM_WIDE_EXTRA).to_string(),
            tip: t!("gui.dj.bpm_sub").to_string(),
            seconds: false,
        },
        DjRow::Cooldown => BarSpec {
            label: t!("gui.dj.cooldown").to_string(),
            lo: 0,
            hi: dj::ARTIST_COOLDOWN_MAX,
            value: s.artist_cooldown,
            words: if s.artist_cooldown == 0 {
                t!("gui.dj.cooldown_off").to_string()
            } else {
                t!("gui.dj.cooldown_value", count = s.artist_cooldown).to_string()
            },
            note: String::new(),
            tip: t!("gui.dj.cooldown_sub").to_string(),
            seconds: false,
        },
        DjRow::Rating => {
            let rating = gui.app.dj_library().min_rating;
            BarSpec {
                label: t!("gui.dj.rating").to_string(),
                lo: 0,
                hi: dj::RATING_MAX,
                value: rating,
                words: if rating == 0 {
                    t!("gui.dj.rating_any").to_string()
                } else {
                    t!("gui.dj.rating_value", stars = format!("{:.1}", f64::from(rating) / 2.0)).to_string()
                },
                note: String::new(),
                tip: t!("gui.dj.rating_sub").to_string(),
                seconds: false,
            }
        }
        DjRow::Shortest => BarSpec {
            label: t!("gui.dj.shortest").to_string(),
            lo: 0,
            hi: dj::LENGTH_RAIL_SECONDS,
            value: s.min_seconds,
            words: if s.min_seconds == 0 { t!("gui.dj.any").to_string() } else { crate::api::types::fmt_duration(f64::from(s.min_seconds)) },
            note: String::new(),
            tip: t!("gui.dj.length_sub").to_string(),
            seconds: true,
        },
        DjRow::Longest => BarSpec {
            label: t!("gui.dj.longest").to_string(),
            lo: 0,
            hi: dj::LENGTH_RAIL_SECONDS,
            value: s.max_seconds,
            words: if s.max_seconds >= dj::LENGTH_RAIL_SECONDS {
                t!("gui.dj.any").to_string()
            } else {
                crate::api::types::fmt_duration(f64::from(s.max_seconds))
            },
            note: String::new(),
            tip: t!("gui.dj.length_sub").to_string(),
            seconds: true,
        },
        _ => return None,
    };
    Some(spec)
}

/// The value a click on cell `k` sets: the bar's range spread over its
/// cells, snapped to the step where the row has one.
fn cell_value(spec: &BarSpec, k: u16) -> u32 {
    let span = (spec.hi - spec.lo) as f64;
    let raw = spec.lo + ((f64::from(k) / f64::from(CELLS - 1)) * span).round() as u32;
    if spec.seconds {
        let step = f64::from(dj::LENGTH_STEP_SECONDS);
        return ((f64::from(raw) / step).round() * step) as u32;
    }
    raw
}

fn filled_cells(spec: &BarSpec) -> u16 {
    let span = (spec.hi - spec.lo).max(1) as f64;
    let part = spec.value.saturating_sub(spec.lo) as f64 / span;
    ((part * f64::from(CELLS)).round() as u16).min(CELLS)
}

/// A bar row (clauses 44, 46, 47, 52 and the sonic strictness): the label,
/// ten cells each a click target, the value in words, a dim note. ←→ step;
/// a click sets.
fn draw_bar(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, w: u16, row: DjRow) {
    let Some(spec) = bar_spec(gui, row) else { return };
    let rect = Rect { x, y, width: w, height: 1 };
    let focused = gui.dj.cursor == Some(Item::Row(row));
    let hover = gui.ui.hovers(rect);
    put(frame, x, y, &super::bar::clip(&spec.label, LABEL_W as usize - 1), row_style(focused, hover));
    let bx = x + LABEL_W;
    let filled = filled_cells(&spec);
    let (full, empty) = if legacy_conhost() { ("#", "-") } else { ("▓", "░") };
    let bar_rect = Rect { x: bx, y, width: CELLS, height: 1 };
    let bar_hover = gui.ui.hovers(bar_rect);
    for k in 0..CELLS {
        let cell = Rect { x: bx + k, y, width: 1, height: 1 };
        let style = if k < filled {
            Style::default().fg(if bar_hover { th().bright } else { th().accent })
        } else {
            dim()
        };
        put(frame, cell.x, y, if k < filled { full } else { empty }, style);
        gui.ui.click(cell, Act::DjSet(row, cell_value(&spec, k)));
    }
    let vx = bx + CELLS + 2;
    put(frame, vx, y, &spec.words, Style::default());
    // The note is a whole detail or nothing: a cosine clipped mid-number
    // would read as a different number.
    if !spec.note.is_empty() {
        let nx = vx + spec.words.chars().count() as u16 + 2;
        let avail = (x + w).saturating_sub(nx) as usize;
        if avail >= spec.note.chars().count() {
            put(frame, nx, y, &spec.note, dim());
        }
    }
    gui.ui.click(Rect { x, y, width: LABEL_W, height: 1 }, Act::DjFocus(Item::Row(row)));
    if !spec.tip.is_empty() {
        gui.ui.tip(Rect { x, y, width: LABEL_W + CELLS, height: 1 }, spec.tip);
    }
}

/// The chosen genres or keywords as an inline list, each with its remove
/// (clauses 48, 49): a click on a chip removes it; on the keyboard ←→ walk
/// the chips and x removes the one under the cursor.
fn draw_chips(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, w: u16, item: Item) {
    let names: Vec<String> = match item {
        Item::GenreChips => gui.app.dj_library().genres,
        Item::KeywordChips => gui.app.dj.keywords.clone(),
        _ => return,
    };
    let focused = gui.dj.cursor == Some(item);
    let sub = gui.dj.chip.min(names.len().saturating_sub(1));
    let mut cx = x;
    let end = x + w;
    for (i, name) in names.iter().enumerate() {
        let name_w = name.chars().count() as u16;
        let width = name_w + 4;
        if cx + width > end {
            if cx < end {
                put(frame, cx, y, "…", dim());
            }
            break;
        }
        let rect = Rect { x: cx, y, width, height: 1 };
        let hover = gui.ui.hovers(rect);
        let selected = focused && i == sub;
        let style = match (selected, hover) {
            (true, _) => accent().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, cx, y, name, style);
        let x_style = if hover || selected {
            Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
        } else {
            dim()
        };
        put(frame, cx + name_w + 1, y, "[x]", x_style);
        let act = match item {
            Item::GenreChips => Act::DjGenre(name.clone()),
            _ => Act::DjKeywordRemove(name.clone()),
        };
        gui.ui.click(rect, act);
        gui.ui.tip(rect, t!("gui.dj.remove_tip").to_string());
        cx += width;
        if i + 1 < names.len() && cx + 3 <= end {
            put(frame, cx, y, " · ", dim());
            cx += 3;
        }
    }
}

/// The keyword field and its Add (clause 49): the kit's line editor, Enter
/// adds, the hint while empty.
fn draw_keyword_input(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, w: u16, forward: &str) {
    let focused = gui.dj.cursor == Some(Item::KeywordInput);
    let label = t!("gui.dj.add_keyword").to_string();
    let row = Rect { x, y, width: w, height: 1 };
    let hover = gui.ui.hovers(row);
    put(frame, x, y, &super::bar::clip(&label, LABEL_W as usize - 1), row_style(focused, hover));
    let fx = x + LABEL_W;
    let fw = FIELD_W.min(w.saturating_sub(LABEL_W + 8)).max(8);
    let field = Rect { x: fx, y, width: fw, height: 1 };
    let value = gui.dj.keyword.value().to_string();
    let cursor = gui.dj.keyword.cursor();
    // The field reads as one: underlined for its whole width.
    let under = Style::default().add_modifier(Modifier::UNDERLINED);
    put(frame, fx, y, &" ".repeat(fw as usize), under);
    if focused {
        super::text_field(frame, &mut gui.ui, fx, y, &value, cursor, fw, under);
    } else if value.is_empty() {
        put(frame, fx, y, &super::bar::clip(&t!("gui.dj.keyword_hint"), fw as usize), dim().add_modifier(Modifier::UNDERLINED));
    } else {
        put(frame, fx, y, &super::bar::clip(&value, fw as usize), under);
    }
    gui.ui.click(field, Act::DjKeywordFocus);
    let add = format!("{} {forward}", t!("gui.dj.add"));
    let ready = !value.trim().is_empty();
    text_button(frame, gui, fx + fw + 2, y, &add, ready, Act::DjKeywordAdd);
}

/// The destructive tall button (Stop Auto DJ): the kit's primary frame in
/// the danger colour, bright under the pointer.
fn tall_danger(frame: &mut Frame, ui: &mut Surface<Act>, at: Rect, label: &str, act: Act) -> Rect {
    tall_frame(frame, ui, at, label, 2, |hovered| (if hovered { th().bright } else { th().danger }, true), Some(act))
}

// ── Modals ──────────────────────────────────────────────────────────────────

/// The DJ's modals, in the order they stack: the opening question, the genre
/// picker, the server picker.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    draw_chooser(frame, gui, area);
    draw_genre_picker(frame, gui, area);
    draw_server_picker(frame, gui, area);
}

/// "Start Auto DJ with what?" (clause 2): a kit modal over everything — the
/// two answers with their descriptions, the remember box, Esc to leave the
/// DJ off. The App holds the row and the box; the shell draws and relays.
fn draw_chooser(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(chooser) = gui.app.dj_chooser.clone() else { return };
    // A click anywhere else dismisses — the DJ stays off.
    gui.ui.click(area, Act::DjCancel);
    let width: u16 = 74.min(area.width.saturating_sub(2)).max(40);
    let text_w = width as usize - 4;
    let subtitle = wrap_words(&t!("gui.dj.start_subtitle"), text_w);
    let (check_on, check_off) = super::check_glyphs();
    let rows: [(String, Vec<String>, Act); ROWS] = [
        (
            t!("gui.dj.surprise").to_string(),
            wrap_words(&t!("gui.dj.surprise_sub"), text_w - 2),
            Act::DjChoose(0),
        ),
        (t!("gui.dj.pick").to_string(), wrap_words(&t!("gui.dj.pick_sub"), text_w - 2), Act::DjChoose(1)),
        (
            format!("{} {}", if chooser.remember { check_on } else { check_off }, t!("gui.dj.remember")),
            wrap_words(&t!("gui.dj.remember_sub"), text_w - 2),
            Act::DjRemember,
        ),
    ];
    // Title, subtitle, a gap, the rows (label + detail lines), trailing gap.
    let rows_h: usize = rows.iter().map(|(_, detail, _)| 1 + detail.len()).sum();
    let height = (1 + subtitle.len() + 1 + rows_h + 1) as u16 + 2;
    let inner = modal_frame_on(frame, &mut gui.ui, area, width, height, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.dj.start_title"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, Act::DjCancel);
    let mut y = inner.y + 1;
    for line in &subtitle {
        put(frame, inner.x + 1, y, line, dim());
        y += 1;
    }
    y += 1;
    for (i, (label, detail, act)) in rows.into_iter().enumerate() {
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 + detail.len() as u16 };
        let hover = gui.ui.hovers(rect);
        let is_sel = chooser.row == i;
        if is_sel {
            let line = Rect { x: inner.x, y, width: inner.width, height: 1 };
            frame.render_widget(Block::default().style(sel()), line);
        }
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, inner.x + 1, y, &label, style);
        for line in &detail {
            y += 1;
            put(frame, inner.x + 3, y, line, dim());
        }
        y += 1;
        gui.ui.click(rect, act);
    }
}

/// The genres the picker shows for the search line.
fn filtered_genres(all: &[String], query: &str) -> Vec<String> {
    let query = query.trim().to_lowercase();
    all.iter().filter(|g| query.is_empty() || g.to_lowercase().contains(&query)).cloned().collect()
}

/// The genre under the picker's cursor.
fn picked_genre(gui: &Gui) -> Option<String> {
    let picker = gui.app.dj_panel.genres.as_ref()?;
    let names = filtered_genres(&picker.all, gui.dj.filter.value());
    names.get(gui.dj.pick_row.min(names.len().saturating_sub(1))).cloned()
}

/// The genre picker (clause 48): a kit modal list with a search line,
/// "{count} selected" in the title, checkboxes toggling live; Space toggles,
/// Enter and Esc close.
fn draw_genre_picker(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(picker) = gui.app.dj_panel.genres.clone() else { return };
    gui.ui.click(area, Act::DjGenresClose);
    let library = gui.app.dj_library();
    let width = 60.min(area.width.saturating_sub(2)).max(30);
    let height = 20.min(area.height.saturating_sub(2)).max(8);
    let inner = modal_frame_on(frame, &mut gui.ui, area, width, height, th().accent);
    let title = format!("{} · {}", t!("gui.dj.pick_genres"), t!("gui.dj.selected_count", count = library.genres.len()));
    put(frame, inner.x + 1, inner.y, &title, accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, Act::DjGenresClose);

    let query = gui.dj.filter.value().to_string();
    let fy = inner.y + 2;
    let field_w = inner.width.saturating_sub(2);
    if query.is_empty() {
        put(frame, inner.x + 1, fy, &super::bar::clip(&t!("gui.dj.search_genres"), field_w as usize), dim());
    } else {
        let cursor = gui.dj.filter.cursor();
        super::text_field(frame, &mut gui.ui, inner.x + 1, fy, &query, cursor, field_w, Style::default());
    }

    let list_y = fy + 2;
    let list_h = inner.bottom().saturating_sub(list_y) as usize;
    let (check_on, check_off) = super::check_glyphs();
    if picker.loading {
        put(frame, inner.x + 1, list_y, &t!("gui.dj.genres_loading"), dim());
        return;
    }
    if let Some(err) = &picker.failed {
        put(frame, inner.x + 1, list_y, &t!("gui.dj.genres_failed"), Style::default().fg(th().gold));
        put(frame, inner.x + 1, list_y + 1, &super::bar::clip(err, field_w as usize), dim());
        return;
    }
    if picker.all.is_empty() {
        put(frame, inner.x + 1, list_y, &t!("gui.dj.no_genres"), dim());
        return;
    }
    let names = filtered_genres(&picker.all, &query);
    if names.is_empty() {
        let words = t!("gui.dj.no_genre_match", query = query.trim()).to_string();
        put(frame, inner.x + 1, list_y, &super::bar::clip(&words, field_w as usize), dim());
        return;
    }
    let row = gui.dj.pick_row.min(names.len() - 1);
    gui.dj.pick_row = row;
    let (scroll, _) = table_view(names.len(), Some(row), gui.dj.genres.scroll, list_h);
    gui.dj.genres.scroll = scroll;
    let overflow = names.len() > list_h;
    let row_w = if overflow { inner.width.saturating_sub(1) } else { inner.width };
    for (i, name) in names.iter().enumerate().skip(scroll).take(list_h) {
        let y = list_y + (i - scroll) as u16;
        let rect = Rect { x: inner.x, y, width: row_w, height: 1 };
        let on = library.genres.iter().any(|g| g == name);
        let is_sel = i == row;
        let hover = gui.ui.hovers(rect);
        if is_sel {
            frame.render_widget(Block::default().style(sel()), rect);
        }
        let glyph_style = if is_sel {
            sel()
        } else if on {
            Style::default().fg(th().ok)
        } else {
            dim()
        };
        put(frame, inner.x + 1, y, if on { check_on } else { check_off }, glyph_style);
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, inner.x + 5, y, &super::bar::clip(name, row_w.saturating_sub(6) as usize), style);
        gui.ui.click(rect, Act::DjGenre(name.clone()));
    }
    if overflow {
        let bar = Rect { x: inner.right().saturating_sub(1), y: list_y, width: 1, height: list_h as u16 };
        scroll_list(
            frame,
            &mut gui.ui,
            bar,
            names.len(),
            list_h,
            scroll,
            Act::ScrollBy(List::DjGenres, -1),
            Act::ScrollBy(List::DjGenres, 1),
            |first| Act::ScrollTo(List::DjGenres, first),
        );
    }
}

/// The server picker (clause 41): every selectable saved server, peers under
/// their parent; the DJ's own marked; Enter or a click moves the DJ.
fn draw_server_picker(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(cursor) = gui.dj.server_pick else { return };
    gui.ui.click(area, Act::DjServerClose);
    let servers = pickable_servers(gui);
    let width = 56.min(area.width.saturating_sub(2)).max(30);
    let height = (servers.len() as u16 + 5).min(area.height.saturating_sub(2)).max(6);
    let inner = modal_frame_on(frame, &mut gui.ui, area, width, height, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.dj.server_title"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, Act::DjServerClose);
    let marker = super::forward_glyph();
    let branch = if legacy_conhost() { "+" } else { "└" };
    for (i, server) in servers.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        if y + 1 >= inner.bottom() {
            break;
        }
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let is_sel = i == cursor;
        let hover = gui.ui.hovers(rect);
        if is_sel {
            frame.render_widget(Block::default().style(sel()), rect);
        }
        if server.current {
            let mstyle = if is_sel { sel() } else { accent() };
            put(frame, inner.x + 1, y, marker, mstyle.add_modifier(Modifier::BOLD));
        }
        let label = if server.peer { format!("{branch} {}", server.label) } else { server.label.clone() };
        let style = match (is_sel, server.current, hover) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => accent().add_modifier(Modifier::BOLD),
            (false, false, true) => bright_bold(),
            (false, false, false) => Style::default(),
        };
        put(frame, inner.x + 3, y, &super::bar::clip(&label, inner.width.saturating_sub(4) as usize), style);
        gui.ui.click(rect, Act::DjServer(i));
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The DJ's keys: its modals outrank everything; Esc leaves the
/// opening-song road; then the room's own. `None` when none of them is up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.app.dj_chooser.is_some() {
        match key.code {
            KeyCode::Esc => gui.forward(Action::Cancel),
            KeyCode::Up => gui.forward(Action::Up),
            KeyCode::Down => gui.forward(Action::Down),
            KeyCode::Enter => gui.forward(Action::Activate),
            KeyCode::Char(' ') => gui.forward(Action::PlayPause),
            _ => {}
        }
        return Some(false);
    }
    if gui.app.dj_panel.genres.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => return Some(gui.act(Act::DjGenresClose)),
            KeyCode::Up => gui.dj.pick_row = gui.dj.pick_row.saturating_sub(1),
            KeyCode::Down => gui.dj.pick_row += 1,
            KeyCode::Char(' ') => {
                if let Some(name) = picked_genre(gui) {
                    return Some(gui.act(Act::DjGenre(name)));
                }
            }
            _ => {
                let changed = gui.dj.filter.handle_event(&TermEvent::Key(key)).is_some_and(|change| change.value);
                if changed {
                    gui.dj.pick_row = 0;
                    gui.dj.genres.scroll = 0;
                }
            }
        }
        return Some(false);
    }
    if let Some(row) = gui.dj.server_pick {
        let last = pickable_servers(gui).len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => gui.dj.server_pick = None,
            KeyCode::Up => gui.dj.server_pick = Some(row.saturating_sub(1)),
            KeyCode::Down => gui.dj.server_pick = Some((row + 1).min(last)),
            KeyCode::Enter => return Some(gui.act(Act::DjServer(row))),
            _ => {}
        }
        return Some(false);
    }
    if gui.app.capture == Some(Capture::DjSeed) && key.code == KeyCode::Esc {
        gui.forward(Action::Cancel);
        return Some(false);
    }
    if !in_room(gui) {
        return None;
    }
    room_key(gui, key)
}

/// Move the keyboard cursor along the controls that exist right now.
fn move_cursor(gui: &mut Gui, delta: i32) {
    let items = items(gui);
    if items.is_empty() {
        gui.dj.cursor = None;
        return;
    }
    let at = gui.dj.cursor.and_then(|c| items.iter().position(|i| *i == c));
    let next = match at {
        Some(i) => (i as i32 + delta).clamp(0, items.len() as i32 - 1) as usize,
        None if delta < 0 => items.len() - 1,
        None => 0,
    };
    if gui.dj.cursor != Some(items[next]) {
        gui.dj.chip = 0;
    }
    gui.dj.cursor = Some(items[next]);
    gui.dj.body.reveal = true;
}

/// The chips row's names, for the sub-cursor.
fn chips_of(gui: &Gui, item: Item) -> Vec<String> {
    match item {
        Item::GenreChips => gui.app.dj_library().genres,
        Item::KeywordChips => gui.app.dj.keywords.clone(),
        _ => Vec::new(),
    }
}

fn room_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    let page = gui.dj.visible.max(1) as i32 - 1;
    match gui.dj.cursor {
        None => match key.code {
            KeyCode::Down => move_cursor(gui, 1),
            KeyCode::Up => move_cursor(gui, -1),
            KeyCode::PageDown => return Some(gui.act(Act::ScrollBy(List::DjRoom, page))),
            KeyCode::PageUp => return Some(gui.act(Act::ScrollBy(List::DjRoom, -page))),
            _ => return None,
        },
        Some(Item::KeywordInput) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.dj.cursor = None,
            KeyCode::Enter => return Some(gui.act(Act::DjKeywordAdd)),
            _ => {
                // The line editor owns the rest: chars, Backspace, Delete,
                // ←/→, Home/End, the ctrl word ops.
                gui.dj.keyword.handle_event(&TermEvent::Key(key));
            }
        },
        Some(item) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.dj.cursor = None,
            KeyCode::PageDown => return Some(gui.act(Act::ScrollBy(List::DjRoom, page))),
            KeyCode::PageUp => return Some(gui.act(Act::ScrollBy(List::DjRoom, -page))),
            KeyCode::Left | KeyCode::Right => {
                let delta = if key.code == KeyCode::Left { -1 } else { 1 };
                match item {
                    Item::Row(row) if is_bar(row) => return Some(gui.act(Act::DjStep(row, delta))),
                    Item::GenreChips | Item::KeywordChips => {
                        let last = chips_of(gui, item).len().saturating_sub(1) as i32;
                        gui.dj.chip = (gui.dj.chip as i32 + delta).clamp(0, last) as usize;
                    }
                    _ => return None,
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let act = match item {
                    Item::Toggle => Act::DjStartStop,
                    Item::Server => Act::DjServerPick,
                    Item::Row(DjRow::Sample) => Act::DjPreview,
                    Item::Row(row) => Act::DjStep(row, 1),
                    Item::Anchor(anchor) => Act::DjAnchor(anchor),
                    Item::EmptyQueue(choice) => Act::DjEmpty(choice),
                    Item::GenreMode(mode) => Act::DjGenreMode(mode),
                    Item::GenreChips | Item::PickGenres => Act::DjPickGenres,
                    Item::KeywordChips | Item::KeywordInput => Act::DjKeywordFocus,
                    Item::Source(i) => {
                        let name = gui.app.dj_info.as_ref().and_then(|info| info.libraries.get(i).cloned());
                        match name {
                            Some(name) => Act::DjSource(name),
                            None => return Some(false),
                        }
                    }
                };
                return Some(gui.act(act));
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace
                if matches!(item, Item::GenreChips | Item::KeywordChips) =>
            {
                let names = chips_of(gui, item);
                let Some(name) = names.get(gui.dj.chip.min(names.len().saturating_sub(1))).cloned() else {
                    return Some(false);
                };
                let act = if item == Item::GenreChips { Act::DjGenre(name) } else { Act::DjKeywordRemove(name) };
                gui.dj.chip = gui.dj.chip.min(names.len().saturating_sub(2));
                return Some(gui.act(act));
            }
            _ => return None,
        },
    }
    Some(false)
}

// ── Acts ────────────────────────────────────────────────────────────────────

/// One edit, through the App, then saved: the session-wide settings ride
/// the player prefs at once, and a per-library rule comes back as its own
/// effect (clause 51).
fn edit(gui: &mut Gui, edit: DjEdit) {
    let effects = gui.app.dj_edit(edit);
    gui.pend(effects);
    gui.save_now();
}

/// The DJ's side of [`Gui::act`]. True when the act was one of ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act.clone() {
        Act::DjChoose(row) => {
            let remember = gui.app.dj_chooser.as_ref().is_some_and(|c| c.remember);
            let choice = if row == 0 { EmptyQueueStart::Random } else { EmptyQueueStart::Pick };
            let effects = gui.app.answer_dj_start(choice, remember);
            gui.pend(effects);
        }
        Act::DjRemember => {
            if let Some(chooser) = gui.app.dj_chooser.as_mut() {
                chooser.remember = !chooser.remember;
            }
        }
        Act::DjCancel => gui.forward(Action::Cancel),
        // The empty queue's openers (clause 16).
        Act::DjSurprise => gui.forward(Action::DjSurprise),
        Act::DjPick => gui.forward(Action::DjPick),
        // The room. Start rides the toggle (the opening question, the seed rule);
        // Stop stops wherever the DJ is armed — the toggle's "on elsewhere
        // → move here" is the bar's rule, not a Stop button's.
        Act::DjStartStop => {
            gui.dj.cursor = Some(Item::Toggle);
            if gui.app.dj_armed() {
                let mut effects = gui.app.disarm_dj();
                // Off, the room describes the session's server again.
                effects.extend(gui.app.dj_room_opened());
                gui.pend(effects);
                gui.save_now();
            } else {
                gui.forward(Action::ToggleAutoDj);
            }
        }
        Act::DjFocus(item) => {
            gui.dj.cursor = Some(item);
            gui.dj.body.reveal = true;
        }
        Act::DjStep(row, delta) => {
            gui.dj.cursor = Some(Item::Row(row));
            edit(gui, DjEdit::Step(row, delta));
        }
        Act::DjSet(row, value) => {
            gui.dj.cursor = Some(Item::Row(row));
            edit(gui, DjEdit::Set(row, value));
        }
        Act::DjAnchor(anchor) => {
            gui.dj.cursor = Some(Item::Anchor(anchor));
            edit(gui, DjEdit::Anchor(anchor));
        }
        Act::DjEmpty(choice) => {
            gui.dj.cursor = Some(Item::EmptyQueue(choice));
            edit(gui, DjEdit::EmptyQueue(choice));
        }
        Act::DjGenreMode(mode) => {
            gui.dj.cursor = Some(Item::GenreMode(mode));
            edit(gui, DjEdit::GenreMode(mode));
        }
        Act::DjGenre(name) => edit(gui, DjEdit::Genre(name)),
        Act::DjPickGenres => {
            gui.dj.filter = Input::default();
            gui.dj.pick_row = 0;
            gui.dj.genres.scroll = 0;
            let effects = gui.app.open_genre_picker();
            gui.pend(effects);
        }
        Act::DjGenresClose => gui.app.close_dj_picker(),
        Act::DjKeywordFocus => {
            gui.dj.cursor = Some(Item::KeywordInput);
            gui.dj.body.reveal = true;
        }
        Act::DjKeywordAdd => {
            let word = gui.dj.keyword.value().trim().to_string();
            if !word.is_empty() {
                edit(gui, DjEdit::AddKeyword(word));
                gui.dj.keyword = Input::default();
            }
            gui.dj.cursor = Some(Item::KeywordInput);
        }
        Act::DjKeywordRemove(word) => edit(gui, DjEdit::RemoveKeyword(word)),
        Act::DjSource(name) => {
            let effects = gui.app.toggle_dj_source(&name);
            gui.pend(effects);
        }
        Act::DjPreview => {
            gui.dj.cursor = Some(Item::Row(DjRow::Sample));
            let effects = gui.app.sample_dj();
            gui.pend(effects);
        }
        Act::DjServerPick => {
            let current = pickable_servers(gui).iter().position(|s| s.current).unwrap_or(0);
            gui.dj.server_pick = Some(current);
        }
        Act::DjServer(i) => {
            gui.dj.server_pick = None;
            if let Some(server) = pickable_servers(gui).get(i) {
                let id = server.id.clone();
                let effects = gui.app.dj_move_to(id);
                gui.pend(effects);
                gui.save_now();
            }
        }
        Act::DjServerClose => gui.dj.server_pick = None,
        _ => return false,
    }
    true
}

/// The wheel over Settings: the genre picker's list, else the room's body.
/// False when neither is up.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) -> bool {
    if gui.app.dj_panel.genres.is_some() {
        gui.act(Act::ScrollBy(List::DjGenres, delta));
        return true;
    }
    if in_room(gui) {
        gui.act(Act::ScrollBy(List::DjRoom, delta));
        return true;
    }
    false
}

/// The tips line for the DJ's surfaces, when one is up.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    if gui.app.dj_chooser.is_some() {
        return Some(t!("gui.dj.start_keys").to_string());
    }
    if gui.app.dj_panel.genres.is_some() {
        return Some(t!("gui.tips.dj_genres").to_string());
    }
    if gui.dj.server_pick.is_some() {
        return Some(t!("gui.tips.dj_servers").to_string());
    }
    if gui.app.capture == Some(Capture::DjSeed) {
        return Some(t!("gui.tips.dj_pick").to_string());
    }
    if !in_room(gui) {
        return None;
    }
    let tip = match gui.dj.cursor {
        None => t!("gui.tips.dj_room"),
        Some(Item::Toggle | Item::PickGenres) => t!("gui.tips.dj_button"),
        Some(Item::Server) => t!("gui.tips.dj_server"),
        Some(Item::Row(DjRow::Sample)) => t!("gui.tips.dj_preview"),
        Some(Item::Row(row)) if is_bar(row) => t!("gui.tips.dj_bar"),
        Some(Item::Row(_) | Item::Source(_)) => t!("gui.tips.dj_switch"),
        Some(Item::Anchor(_) | Item::EmptyQueue(_) | Item::GenreMode(_)) => t!("gui.tips.dj_radio"),
        Some(Item::GenreChips | Item::KeywordChips) => t!("gui.tips.dj_chips"),
        Some(Item::KeywordInput) => t!("gui.tips.dj_input"),
    };
    Some(tip.to_string())
}

/// The banner the "Let me choose" road wears (clause 4), when it is armed.
pub(crate) fn banner(gui: &Gui) -> Option<String> {
    (gui.app.capture == Some(Capture::DjSeed)).then(|| t!("gui.dj.pick_banner").to_string())
}

/// The empty queue panel's openers (clause 16): while the DJ is armed, the
/// hint and the two ways to give it an opening song, under the label.
pub(crate) fn draw_empty_queue(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, width: usize) {
    if !gui.app.dj_armed() {
        return;
    }
    let forward = super::forward_glyph();
    // The hint wraps in the narrow panel; the buttons follow it.
    let mut y = y + 1;
    for line in wrap_words(&t!("gui.queue.empty_dj_hint"), width).into_iter().take(3) {
        put(frame, x, y, &line, dim());
        y += 1;
    }
    let random = format!("{} {forward}", t!("gui.queue.empty_dj_random"));
    let choose = format!("{} {forward}", t!("gui.queue.empty_dj_choose"));
    text_button(frame, gui, x, y + 1, &random, true, Act::DjSurprise);
    text_button(frame, gui, x, y + 2, &choose, false, Act::DjPick);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::super::{Act, DJ_NAV, FILES_NAV, Gui, Screen, render};
    use super::Item;
    use crate::api::types::{Genre, Track};
    use crate::config::{self, Config};
    use crate::dj::{EmptyQueueStart, GenreMode, SonicAnchor};
    use crate::tui::app::{App, Capture, DjMark, DjRow, Effect, KnownServer, Origin, Queued};
    use crate::tui::worker::{ApiCmd, DjServerInfo, Event};

    const HOST: &str = "http://host:3000";
    const ATTIC: &str = "http://attic:3000";

    /// A connected session with a server to arm the DJ for.
    fn session_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui
    }

    fn known(id: &str, name: &str) -> KnownServer {
        KnownServer {
            id: id.into(),
            name: name.into(),
            token: None,
            self_signed: false,
            peer: None,
            pairing: None,
            dj: Default::default(),
        }
    }

    fn track(path: &str) -> Track {
        Track { filepath: path.into(), metadata: Default::default() }
    }

    fn queued(path: &str, dj: Option<DjMark>) -> Queued {
        Queued { origin: Origin { server: HOST.into(), peer: None }, dj, track: track(path) }
    }

    /// The session, with the room open.
    fn room_gui() -> Gui {
        let mut gui = session_gui();
        gui.act(Act::Nav(DJ_NAV));
        assert_eq!(gui.active, DJ_NAV, "the nav row opens the room");
        gui
    }

    fn draw_sized(gui: &mut Gui, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        draw_sized(gui, 100, 30)
    }

    /// The room needs the height: the body is long and scrolls.
    fn draw_tall(gui: &mut Gui) -> Vec<String> {
        draw_sized(gui, 110, 70)
    }

    /// The act under the first cell of `needle` on the drawn screen.
    fn hit_text(gui: &Gui, rows: &[String], needle: &str) -> Option<Act> {
        let y = rows.iter().position(|r| r.contains(needle))?;
        let x = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with(needle))? as u16;
        gui.ui.hit(Position { x, y: y as u16 })
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn opener_asked(gui: &Gui) -> bool {
        gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDj(request)) if request.ask.opener))
    }

    #[test]
    fn the_toggle_on_an_empty_queue_draws_the_chooser_and_a_click_answers_it() {
        let mut gui = session_gui();
        gui.act(Act::AutoDj);
        assert!(gui.app.dj_chooser.is_some(), "the opening question");
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Start Auto DJ with what?"), "{all}");
        assert!(all.contains("Surprise me") && all.contains("Let me choose") && all.contains("Remember this"));

        // Esc leaves the DJ off.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.dj_chooser.is_none() && !gui.app.dj_armed());

        // Back again; the remember box by click, then the first answer.
        gui.act(Act::AutoDj);
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Remember this"), Some(Act::DjRemember));
        gui.act(Act::DjRemember);
        assert!(gui.app.dj_chooser.as_ref().unwrap().remember);
        assert_eq!(hit_text(&gui, &rows, "Surprise me"), Some(Act::DjChoose(0)));
        gui.act(Act::DjChoose(0));
        assert!(gui.app.dj_chooser.is_none());
        assert!(opener_asked(&gui), "the filtered opener goes out: {:?}", gui.pending);
        assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Random, "remembered");
    }

    /// `cargo test dump_dj -- --ignored --nocapture` to eyeball the chooser,
    /// the armed empty queue, and the room in both states.
    #[test]
    #[ignore]
    fn dump_dj() {
        let mut gui = session_gui();
        gui.act(Act::AutoDj);
        println!("{}", draw(&mut gui).join("\n"));
        gui.forward(crate::tui::app::Action::Cancel);
        gui.app.dj_server = Some(HOST.into());
        println!("{}", draw(&mut gui).join("\n"));
        gui.app.dj_server = None;
        println!("{}", draw(&mut gui).join("\n"));
        gui.act(Act::Nav(DJ_NAV));
        println!("{}", draw_tall(&mut gui).join("\n"));
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.app.servers.push(known(HOST, "host"));
        gui.act(Act::AutoDj);
        gui.act(Act::DjStep(DjRow::Keywords, 1));
        gui.act(Act::DjGenreMode(GenreMode::Whitelist));
        gui.act(Act::DjGenre("Ambient".into()));
        println!("{}", draw_tall(&mut gui).join("\n"));
    }

    #[test]
    fn the_empty_queue_offers_the_openers_only_while_the_dj_is_armed() {
        let mut gui = session_gui();
        let all = draw(&mut gui).join("\n");
        assert!(!all.contains("Pick a random song"), "off: just the label");

        gui.app.dj_server = Some(HOST.into());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Auto DJ is on and needs an") && all.contains("opening song."), "wrapped hint: {all}");
        assert!(all.contains("Pick a random song") && all.contains("Choose a song"));
        assert_eq!(hit_text(&gui, &rows, "Pick a random"), Some(Act::DjSurprise));
        gui.act(Act::DjSurprise);
        assert!(opener_asked(&gui), "the button asks the opener of the armed DJ");
    }

    #[test]
    fn dj_picked_rows_wear_the_badge_and_the_road_wears_its_banner() {
        let mut gui = session_gui();
        gui.app.queue.items = vec![
            queued("mine.mp3", None),
            queued("random.mp3", Some(DjMark { sonic: false })),
            queued("sonic.mp3", Some(DjMark { sonic: true })),
        ];
        let rows = draw(&mut gui);
        let title_at = |name: &str| rows.iter().find(|r| r.contains(name)).unwrap().clone();
        assert!(title_at("mine.mp3").contains(" mine.mp3"), "no badge on the user's row");
        assert!(title_at("random.mp3").contains("∞ random.mp3"), "{}", title_at("random.mp3"));
        assert!(title_at("sonic.mp3").contains("≈ sonic.mp3"), "{}", title_at("sonic.mp3"));

        gui.app.capture = Some(Capture::DjSeed);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Pick the opening song"), "{all}");
    }

    // ── The room ────────────────────────────────────────────────────────

    #[test]
    fn the_nav_row_opens_the_room_and_esc_only_stows_the_cursor() {
        let mut gui = session_gui();
        let rows = draw(&mut gui);
        // The TOOLS group under Search: Auto DJ, then the sonic room's slot.
        assert!(rows[13].contains("TOOLS"), "the group:\n{}", rows[13]);
        assert_eq!(hit_text(&gui, &rows, "Auto DJ"), Some(Act::Nav(DJ_NAV)));
        assert!(!rows[14].contains('•'), "off: the row wears no state dot:\n{}", rows[14]);
        assert!(!rows.iter().any(|r| r.contains("LISTEN")), "Settings has no doorway any more");
        gui.act(Act::Nav(DJ_NAV));
        assert_eq!(gui.active, DJ_NAV);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].starts_with(" ".repeat(17).as_str()) || !rows[2].contains('◂'), "no way back:\n{}", rows[2]);
        assert!(all.contains("• off") && all.contains("Auto DJ is off"), "{all}");
        assert!(all.contains("Start Auto DJ ▸"), "{all}");
        assert!(all.contains("CONTINUITY") && all.contains("Sonic similarity"), "{all}");

        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.dj.cursor, Some(Item::Toggle), "↓ picks the cursor up on the button");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.dj.cursor, None, "Esc stows it");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.active, DJ_NAV, "and the room stays: a nav room has no back");
    }

    #[test]
    fn d_opens_the_room_from_anywhere_and_the_nav_row_wears_the_armed_dot() {
        let mut gui = session_gui();
        gui.app.servers.push(known(HOST, "host"));
        gui.act(Act::Screen(Screen::NowPlaying));
        key(&mut gui, KeyCode::Char('D'));
        assert_eq!((gui.screen, gui.active), (Screen::Library, DJ_NAV), "the tenth room's key");
        // Armed with a queue (no opening question), the nav row says so.
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.act(Act::DjStartStop);
        assert!(gui.app.dj_armed());
        gui.act(Act::Nav(FILES_NAV));
        let rows = draw(&mut gui);
        assert!(rows[14].contains("Auto DJ •"), "the dot while armed:\n{}", rows[14]);
        assert_eq!(hit_text(&gui, &rows, "Auto DJ •"), Some(Act::Nav(DJ_NAV)), "one click target with its dot");
        gui.act(Act::Nav(DJ_NAV));
        gui.act(Act::DjStartStop);
        let rows = draw(&mut gui);
        assert!(!rows[14].contains('•'), "off again, no dot:\n{}", rows[14]);
    }

    #[test]
    fn the_button_starts_and_stops_through_the_shared_toggle() {
        let mut gui = room_gui();
        gui.app.servers.push(known(HOST, "host"));
        gui.app.queue.items = vec![queued("a.mp3", None)];
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Start Auto DJ"), Some(Act::DjStartStop));
        gui.act(Act::DjStartStop);
        assert!(gui.app.dj_armed());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("• on · picking from host"), "{all}");
        assert!(all.contains("Auto DJ is on") && all.contains("Stop Auto DJ"), "{all}");
        assert_eq!(hit_text(&gui, &rows, "Stop Auto DJ"), Some(Act::DjStartStop));
        // Armed elsewhere, Stop still stops (the bar's toggle would move it).
        gui.app.servers.push(known(ATTIC, "attic"));
        let effects = gui.app.dj_move_to(ATTIC.into());
        gui.pend(effects);
        assert_eq!(gui.app.dj_server.as_deref(), Some(ATTIC));
        gui.act(Act::DjStartStop);
        assert!(!gui.app.dj_armed(), "off, not moved");
    }

    #[test]
    fn switches_bars_and_radios_edit_the_running_settings() {
        let mut gui = room_gui();
        let rows = draw_tall(&mut gui);
        // The strictness bar: a click on a cell sets, ←→ step by .05.
        let y = rows.iter().position(|r| r.contains("Match strictness")).unwrap();
        let bx = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with('▓') || rows[y][i..].starts_with('░')).unwrap() as u16;
        let last_cell = gui.ui.hit(Position { x: bx + 9, y: y as u16 });
        assert_eq!(last_cell, Some(Act::DjSet(DjRow::Strictness, 80)), "the last cell is the top of the band");
        gui.act(Act::DjSet(DjRow::Strictness, 80));
        assert!((gui.app.dj.sonic_min_similarity - 0.80).abs() < 1e-9);
        key(&mut gui, KeyCode::Left);
        assert!((gui.app.dj.sonic_min_similarity - 0.75).abs() < 1e-9, "← steps down");

        // Radios choose.
        assert_eq!(hit_text(&gui, &rows, "Stay on seed"), Some(Act::DjAnchor(SonicAnchor::Locked)));
        gui.act(Act::DjAnchor(SonicAnchor::Locked));
        assert_eq!(gui.app.dj.sonic_anchor, SonicAnchor::Locked);
        assert_eq!(hit_text(&gui, &rows, "Let me choose"), Some(Act::DjEmpty(EmptyQueueStart::Pick)));
        gui.act(Act::DjEmpty(EmptyQueueStart::Pick));
        assert_eq!(gui.app.dj.empty_queue, EmptyQueueStart::Pick);

        // A switch toggles by click and by Space; its detail rows follow.
        assert_eq!(hit_text(&gui, &rows, "Sonic similarity"), Some(Act::DjStep(DjRow::Sonic, 1)));
        gui.act(Act::DjStep(DjRow::Sonic, 1));
        assert!(!gui.app.dj.sonic);
        let all = draw_tall(&mut gui).join("\n");
        assert!(!all.contains("Match strictness"), "off: the band and the anchor fold away");
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.app.dj.sonic, "Space on the focused switch");

        // The length window: on, then the shortest bound reads in words.
        gui.act(Act::DjStep(DjRow::Length, 1));
        gui.act(Act::DjSet(DjRow::Shortest, 90));
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("Over 1:30"), "{all}");
        assert!(all.contains("Include tracks of unknown length"), "a real bound reveals the checkbox");
    }

    #[test]
    fn keywords_are_added_by_typing_and_removed_by_their_chip() {
        let mut gui = room_gui();
        gui.act(Act::DjStep(DjRow::Keywords, 1));
        assert!(gui.app.dj.keyword_filter);
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("No keywords. Add words below"), "{all}");
        gui.act(Act::DjKeywordFocus);
        for c in "live".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.dj.keywords, vec!["live"]);
        assert!(gui.dj.keyword.value().is_empty(), "the field clears for the next word");
        let rows = draw_tall(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "live [x]"), Some(Act::DjKeywordRemove("live".into())));
        gui.act(Act::DjKeywordRemove("live".into()));
        assert!(gui.app.dj.keywords.is_empty());
    }

    #[test]
    fn genres_are_picked_in_the_modal_and_written_to_the_dj_servers_entry() {
        let mut gui = room_gui();
        gui.app.servers.push(known(HOST, "host"));
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.act(Act::AutoDj);
        gui.pending.clear();
        gui.act(Act::DjPickGenres);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Genres))), "the list is asked for");
        let effects = gui.app.apply_event(Event::Genres(vec![
            Genre { name: "Ambient".into(), track_count: Some(4) },
            Genre { name: "Techno".into(), track_count: Some(9) },
        ]));
        gui.pend(effects);
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("Pick genres · 0 selected") && all.contains("Ambient") && all.contains("Techno"), "{all}");
        for c in "tec".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("Techno") && !all.contains("Ambient"), "the search line narrows: {all}");
        gui.pending.clear();
        key(&mut gui, KeyCode::Char(' '));
        let entry = &gui.app.servers[0].dj;
        assert_eq!(entry.genres.as_deref(), Some(&["Techno".to_string()][..]), "written to the server's entry");
        assert_eq!((entry.genre_filter, entry.genre_mode.as_deref()), (Some(true), Some("whitelist")), "choosing switches the filter on");
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::SaveDjLibrary { .. })), "and saved");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.dj_panel.genres.is_none());
        let rows = draw_tall(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("(•) Whitelist") && all.contains("Techno [x]"), "{all}");
        assert_eq!(hit_text(&gui, &rows, "Techno [x]"), Some(Act::DjGenre("Techno".into())));
    }

    #[test]
    fn the_genre_switch_and_the_mode_are_two_controls() {
        let mut gui = room_gui();
        gui.app.servers.push(known(HOST, "host"));
        let rows = draw_tall(&mut gui);
        assert!(!rows.join("\n").contains("(•) Whitelist"), "off: no radios");
        assert_eq!(hit_text(&gui, &rows, "Genre filter"), Some(Act::DjStep(DjRow::Genres, 1)), "the switch row");
        gui.act(Act::DjStep(DjRow::Genres, 1));
        assert!(gui.app.dj_library().genre_filter);
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("[✓] Genre filter") && all.contains("(•) Whitelist"), "on, whitelist by default: {all}");
        // Blacklist chosen, then ONE click on the switch: off, and the
        // choice is kept for the next switch on. (It used to step the
        // mode first, so a checked whitelist landed on blacklist.)
        gui.act(Act::DjGenreMode(GenreMode::Blacklist));
        assert_eq!(gui.app.dj_library().genre_mode, GenreMode::Blacklist);
        gui.act(Act::DjStep(DjRow::Genres, 1));
        assert!(!gui.app.dj_library().genre_filter, "one click switches off");
        assert_eq!(gui.app.dj_library().genre_mode, GenreMode::Blacklist, "the mode is kept while off");
        let entry = &gui.app.servers[0].dj;
        assert_eq!((entry.genre_filter, entry.genre_mode.as_deref()), (Some(false), Some("blacklist")), "two fields");
        gui.act(Act::DjStep(DjRow::Genres, 1));
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("(•) Blacklist"), "on again as blacklist: {all}");
    }

    #[test]
    fn a_failed_genres_load_says_so_in_the_picker_instead_of_loading_forever() {
        let mut gui = room_gui();
        gui.act(Act::DjPickGenres);
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("loading genres…"), "{all}");
        let effects = gui.app.apply_event(Event::GenresFailed("boom".into()));
        gui.pend(effects);
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("Could not load genres") && all.contains("boom"), "{all}");
        assert!(!all.contains("loading genres…"));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.dj_panel.genres.is_none());
        // A fresh open asks again, clean.
        gui.act(Act::DjPickGenres);
        assert!(gui.app.dj_panel.genres.as_ref().is_some_and(|p| p.loading && p.failed.is_none()));
    }

    #[test]
    fn the_server_picker_moves_the_dj_and_reselecting_is_a_no_op() {
        let mut gui = room_gui();
        gui.config.servers.push(config::ServerEntry { url: HOST.into(), ..Default::default() });
        gui.config.servers.push(config::ServerEntry { url: ATTIC.into(), ..Default::default() });
        gui.app.servers.push(known(HOST, "host"));
        gui.app.servers.push(known(ATTIC, "attic"));
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.act(Act::AutoDj);
        let rows = draw_tall(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "host ▸"), Some(Act::DjServerPick), "the server row, several servers, while on");
        gui.act(Act::DjServerPick);
        assert_eq!(gui.dj.server_pick, Some(0), "opens on the DJ's own");
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("Auto DJ picks from") && all.contains("attic"), "{all}");
        let epoch = gui.app.lane.epoch;
        gui.act(Act::DjServer(0));
        assert_eq!(gui.app.lane.epoch, epoch, "re-selecting is a no-op");
        gui.act(Act::DjServerPick);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.dj_server.as_deref(), Some(ATTIC), "moved");
        assert_ne!(gui.app.lane.epoch, epoch, "a new lane");
        assert!(gui.dj.server_pick.is_none());
    }

    #[test]
    fn a_server_known_to_predate_the_filters_hides_them_and_says_why() {
        let mut gui = room_gui();
        gui.app.servers.push(known(HOST, "host"));
        gui.app.queue.items = vec![queued("a.mp3", None)];
        gui.act(Act::AutoDj);
        let effects = gui.app.apply_event(Event::DjProbed {
            identity: HOST.into(),
            info: Some(DjServerInfo {
                version: Some("6.5.0".into()),
                discovery: false,
                discovery_ready: None,
                libraries: vec!["Music".into(), "Pods".into()],
            }),
        });
        gui.pend(effects);
        let rows = draw_tall(&mut gui);
        let all = rows.join("\n");
        // The rows go (the gate sentence still names them).
        assert!(!all.contains("] BPM continuity") && !all.contains("] Genre filter") && !all.contains("Track length"), "{all}");
        assert!(all.contains("Update to get them."), "the gate sentence, wrapped to the body: {all}");
        assert!(all.contains("Needs server 6.15.2 or newer"), "the sonic row says why: {all}");
        assert!(all.contains("SOURCES") && all.contains("Pods"), "{all}");
        // Sources: one off is fine, the last one is refused.
        assert_eq!(hit_text(&gui, &rows, "Pods"), Some(Act::DjSource("Pods".into())));
        gui.pending.clear();
        gui.act(Act::DjSource("Pods".into()));
        assert_eq!(gui.app.dj_sources_off(), vec!["Pods"]);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::SaveDjLibrary { .. })));
        gui.act(Act::DjSource("Music".into()));
        assert_eq!(gui.app.dj_sources_off(), vec!["Pods"], "refused");
        assert_eq!(gui.app.message.as_ref().map(|m| m.text.as_str()), Some("At least one source is required."));
    }

    #[test]
    fn preview_asks_three_picks_without_queueing() {
        let mut gui = room_gui();
        gui.pending.clear();
        gui.act(Act::DjPreview);
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AutoDjSample { count: 3, .. }))),
            "{:?}",
            gui.pending
        );
        let all = draw_tall(&mut gui).join("\n");
        assert!(all.contains("picking…"), "{all}");
        assert!(gui.app.queue.items.is_empty());
    }
}
