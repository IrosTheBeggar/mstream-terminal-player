//! Auto DJ in the GUI shell (docs/ux-contracts/auto-dj.md): the room, a
//! Library nav room under TOOLS — one row per setting, each opening one
//! sheet (clauses 40–58, re-laid 2026-11-10 from the record's rebuilt
//! screen) — the opening question as a kit modal (clause 2), the
//! opening-song banner (clause 4), the empty queue's openers (clause 16),
//! and quick config over the bar. The bar's `auto-dj` toggle and `A` ride
//! the shared App's toggle (entry point 1); every edit here goes through the
//! App's [`DjEdit`], so both shells change one model.
//!
//! The room never scrolls. Start / Stop is the status; a row is a name, its
//! value at one right edge — DIM for Any, Off and how the session runs, OK
//! while it shapes the picks, GOLD with `!` when a running session had to
//! drop it — and `▸` (a sheet) or `▾` (a list, where a pick applies at
//! once). A setting the server can't do keeps its place, DIM, with its
//! reason where the value would be. The sheets are [`sheets`]; the bar's
//! split button opens [`quick`].

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use rust_i18n::t;

use crate::api::types::MoodFilter;
use crate::config;
use crate::dj::{self, EmptyQueueStart, GenreMode};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, cursor_ring, dim, width, wrap_whole};
use crate::tui::app::{Action, Capture, DjEdit, DjGate};
use crate::tui::worker::DjNote;

use super::{Act, DJ_NAV, Gui, Screen, accent, bright_bold, put, sel, text_button};

mod quick;
mod sheets;

pub(crate) use quick::QuickAct;
pub(crate) use sheets::{Sheet, SheetAct, border_title_value};

/// The rows' cells across: a name, its value ending two cells before the
/// last, the chevron on the last — the room's width at a hundred columns
/// with the queue open, and no wider in a wider room.
const ROW_W: u16 = 47;
/// The opening question's cells across (the sheets' width).
const CHOOSER_W: u16 = 66;

// ── State ───────────────────────────────────────────────────────────────────

/// The room's own state: what the App does not hold because only this shell
/// needs it.
#[derive(Debug, Default)]
pub(crate) struct DjUi {
    /// The keyboard's hand: `None` is stowed (the kit's resting state).
    pub cursor: Option<Item>,
    /// First song's list, when it is down: the row its cursor is on, and
    /// the row of the room it hangs from (set by the room's draw).
    pub first: Option<usize>,
    first_anchor: Option<Rect>,
    /// The open sheet: its draft, and the keyboard's hand in it.
    pub sheet: Option<sheets::Open>,
    /// Quick config, when it is up over the bar.
    pub quick: Option<quick::Quick>,
    /// The empty queue's Recently played list, when it is down: its cursor
    /// row, and where it hangs (set by the panel's draw).
    pub recent: Option<usize>,
    recent_anchor: Option<Rect>,
    /// Whether this emptiness of the armed queue has asked for the songs
    /// played last — once each time the queue runs out.
    recent_asked: bool,
    /// The open sheet's list window: Genres' genres, Play from's servers
    /// and folders.
    pub list: ListView,
}

impl DjUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// A row of the room, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    PlayFrom,
    FirstSong,
    Batch,
    Sonic,
    Smart,
    Genres,
    Rating,
    Length,
    Beat,
    Harmonic,
    SkipWords,
    Cooldown,
}

/// How the session runs; then what every pick must match; then what the DJ
/// leans towards and gives up when it must.
const SESSION: [Row; 3] = [Row::PlayFrom, Row::FirstSong, Row::Batch];
const MUST: [Row; 5] = [Row::Sonic, Row::Smart, Row::Genres, Row::Rating, Row::Length];
const PREFER: [Row; 4] = [Row::Beat, Row::Harmonic, Row::SkipWords, Row::Cooldown];

/// A control the keyboard's hand can rest on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    /// `Preview picks ▸`, on the title row.
    Preview,
    /// Start / Stop.
    Toggle,
    Row(Row),
}

/// How a value reads: for Any, Off or a session setting; while it shapes
/// the picks; when a running session had to drop it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Neutral,
    Active,
    Warning,
}

impl Tone {
    pub(crate) fn style(self) -> Style {
        match self {
            Tone::Neutral => dim(),
            Tone::Active => Style::default().fg(th().ok),
            Tone::Warning => Style::default().fg(th().gold),
        }
    }
}

/// What a press on a row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `▸`: its sheet.
    Sheet,
    /// `▾`: its list, where a pick applies at once.
    List,
    /// The one switch on the page.
    Switch(bool),
    /// Nothing: the server can't do it, and the value is the reason.
    Gated,
}

/// A row as it stands right now.
struct RowView {
    label: String,
    value: String,
    tone: Tone,
    kind: Kind,
    /// The reason in a sentence, where the kit's tooltip has one to give.
    tip: Option<String>,
}

// ── Servers ─────────────────────────────────────────────────────────────────

/// A saved server Play from offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickServer {
    pub id: String,
    pub label: String,
    pub peer: bool,
    /// The one the rows are about: the DJ's while it runs, else Start's.
    pub current: bool,
}

/// A server's name without the scheme: "music.home.arpa",
/// "laptop.local:3000". A peer's is already a name.
pub(crate) fn short_name(name: &str) -> String {
    name.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string()
}

/// Every selectable saved server, grouped — a peer right under the parent
/// it is reached through — with the rows' own marked.
pub(crate) fn pickable_servers(gui: &Gui) -> Vec<PickServer> {
    let target = gui.app.dj_library_target();
    let servers = &gui.config.servers;
    config::grouped_order(servers)
        .into_iter()
        .filter_map(|i| {
            let entry = &servers[i];
            if !config::selectable(entry) {
                return None;
            }
            let label = match &entry.peer {
                Some(peer) => peer.name.clone(),
                None => short_name(&config::display_name(entry)),
            };
            Some(PickServer {
                id: entry.url.clone(),
                label,
                peer: entry.peer.is_some(),
                current: target.as_deref().is_some_and(|t| config::same_server(t, &entry.url)),
            })
        })
        .collect()
}

/// The libraries of the server the rows are about, once it has said.
pub(crate) fn libraries(gui: &Gui) -> Vec<String> {
    gui.app.dj_info.as_ref().map(|info| info.libraries.clone()).unwrap_or_default()
}

/// The name of the server the rows are about.
pub(crate) fn target_name(gui: &Gui) -> String {
    match gui.app.dj_library_target() {
        Some(id) => short_name(&gui.app.server_name_of(&id)),
        None => String::new(),
    }
}

/// Whether Play from has anything to choose: another server, or a second
/// library of this one (the record's rule — the row is not drawn for a
/// choice of one).
fn shows_play_from(gui: &Gui) -> bool {
    pickable_servers(gui).len() > 1 || libraries(gui).len() > 1
}

// ── Words ───────────────────────────────────────────────────────────────────

/// "{n} songs" in the locale's plural form: the record's ICU plural, by
/// hand, for the two languages here with more than one plural.
pub(crate) fn songs_words(count: u32) -> String {
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

/// Why the server can't do a row, in the record's few words.
pub(crate) fn gate_words(gate: DjGate) -> String {
    match gate {
        DjGate::NeedsServer(floor) => t!("gui.dj.needs_server", version = dj::floor_label(floor)),
        DjGate::DiscoveryOff => t!("gui.dj.discovery_off"),
        DjGate::StillScanning => t!("gui.dj.still_scanning"),
        DjGate::NoMoodData => t!("gui.dj.no_mood_data"),
        DjGate::NotOnPeers => t!("gui.dj.not_on_peers"),
    }
    .to_string()
}

fn any_words() -> String {
    t!("gui.dj.any_value").to_string()
}

fn off_words() -> String {
    t!("gui.dj.value_off").to_string()
}

pub(crate) fn first_words(choice: EmptyQueueStart) -> String {
    match choice {
        EmptyQueueStart::Ask => t!("gui.dj.ask"),
        EmptyQueueStart::Random => t!("gui.dj.first_random"),
        EmptyQueueStart::Pick => t!("gui.dj.first_choose"),
        EmptyQueueStart::Recent => t!("gui.dj.first_recent"),
    }
    .to_string()
}

pub(crate) fn sonic_value(on: bool, pct: u32) -> (String, Tone) {
    if on { (t!("gui.dj.percent", pct = pct).to_string(), Tone::Active) } else { (off_words(), Tone::Neutral) }
}

/// Why a running session is picking without the pool, in a few words.
pub(crate) fn drop_words(note: &DjNote) -> Option<String> {
    match note {
        DjNote::SonicRange => Some(t!("gui.dj.too_strict").to_string()),
        DjNote::SonicUnscanned => Some(t!("gui.dj.still_scanning").to_string()),
        DjNote::PreviewFailed(_) => None,
    }
}

/// The dropped pool of the running session, while there is one to name.
pub(crate) fn sonic_drop(gui: &Gui) -> Option<String> {
    let app = &gui.app;
    app.lane.sonic_drop.as_ref().filter(|_| app.dj_armed() && app.dj.sonic).and_then(drop_words)
}

pub(crate) fn moods_value(filter: MoodFilter) -> (String, Tone) {
    if filter.is_empty() {
        return (any_words(), Tone::Neutral);
    }
    let words: String =
        super::moods::strip_run(filter, &rust_i18n::locale()).into_iter().map(|(text, _)| text).collect();
    (words, Tone::Active)
}

/// A genre rule in words — sorted, so the same genres read the same
/// whatever order they were ticked in.
pub(crate) fn genres_value(mode: GenreMode, genres: &[String]) -> (String, Tone) {
    if genres.is_empty() {
        return (any_words(), Tone::Neutral);
    }
    let mut sorted: Vec<&str> = genres.iter().map(String::as_str).collect();
    sorted.sort_by_key(|g| g.to_lowercase());
    let list = sorted.join(" · ");
    let words = match mode {
        GenreMode::Whitelist => list,
        GenreMode::Blacklist => t!("gui.dj.genres_skip_value", genres = list).to_string(),
    };
    (words, Tone::Active)
}

/// A rating on the server's half-star 1–10 scale, in stars: 7 is "3.5★+".
pub(crate) fn rating_value(rating: u32) -> (String, Tone) {
    if rating == 0 {
        return (any_words(), Tone::Neutral);
    }
    let stars = if rating % 2 == 0 { format!("{}", rating / 2) } else { format!("{:.1}", f64::from(rating) / 2.0) };
    let words = t!("gui.dj.rating_value", stars = stars).to_string();
    (if legacy_conhost() { words.replace('★', "*") } else { words }, Tone::Active)
}

pub(crate) fn length_value(min: Option<u32>, max: Option<u32>) -> (String, Tone) {
    let words = match (min, max) {
        (None, None) => return (any_words(), Tone::Neutral),
        (Some(min), None) => t!("gui.dj.length_from", min = dj::fmt_length(min)),
        (None, Some(max)) => t!("gui.dj.length_upto", max = dj::fmt_length(max)),
        (Some(min), Some(max)) => t!("gui.dj.length_window", min = dj::fmt_length(min), max = dj::fmt_length(max)),
    };
    (words.to_string(), Tone::Active)
}

pub(crate) fn tempo_value(on: bool, tolerance: u32) -> (String, Tone) {
    if on { (t!("gui.dj.tolerance_value", bpm = tolerance).to_string(), Tone::Active) } else { (off_words(), Tone::Neutral) }
}

pub(crate) fn words_value(words: &[String]) -> (String, Tone) {
    if words.is_empty() { (off_words(), Tone::Neutral) } else { (words.join(" · "), Tone::Active) }
}

pub(crate) fn cooldown_value(artists: u32) -> (String, Tone) {
    if artists == 0 {
        (off_words(), Tone::Neutral)
    } else {
        (t!("gui.dj.cooldown_value", count = artists).to_string(), Tone::Active)
    }
}

fn row_label(row: Row) -> String {
    match row {
        Row::PlayFrom => t!("gui.dj.play_from"),
        Row::FirstSong => t!("gui.dj.first_song"),
        Row::Batch => t!("gui.dj.batch"),
        Row::Sonic => t!("gui.dj.sonic"),
        Row::Smart => t!("gui.dj.smart"),
        Row::Genres => t!("gui.dj.genres"),
        Row::Rating => t!("gui.dj.rating_row"),
        Row::Length => t!("gui.dj.length_row"),
        Row::Beat => t!("gui.dj.beat"),
        Row::Harmonic => t!("gui.dj.harmonic"),
        Row::SkipWords => t!("gui.dj.skip_words"),
        Row::Cooldown => t!("gui.dj.cooldown"),
    }
    .to_string()
}

/// A row as it stands: its value in its tone, or the reason the server
/// can't do it (the version gates, the peer's missing stars, the sonic
/// row's three reasons — clauses 36, 43, 46, 50).
fn row_view(gui: &Gui, row: Row) -> RowView {
    let app = &gui.app;
    let s = &app.dj;
    let label = row_label(row);
    let sheet = |(value, tone): (String, Tone)| RowView { label: label.clone(), value, tone, kind: Kind::Sheet, tip: None };
    let gated = |gate: DjGate, tip: Option<String>| RowView {
        label: label.clone(),
        value: gate_words(gate),
        tone: Tone::Neutral,
        kind: Kind::Gated,
        tip,
    };
    match row {
        Row::PlayFrom => sheet((target_name(gui), Tone::Neutral)),
        Row::FirstSong => RowView { kind: Kind::List, ..sheet((first_words(s.empty_queue), Tone::Neutral)) },
        Row::Batch => match app.dj_floor_gate(dj::FLOOR_BATCH) {
            Some(gate) => gated(gate, None),
            None => sheet((songs_words(s.songs_per_fetch), Tone::Neutral)),
        },
        Row::Sonic => {
            if let Some(gate) = app.dj_sonic_gate() {
                return gated(gate, app.dj_sonic_reason());
            }
            match sonic_drop(gui) {
                Some(why) => sheet((why, Tone::Warning)),
                None => sheet(sonic_value(s.sonic, (s.sonic_min_similarity * 100.0).round() as u32)),
            }
        }
        Row::Smart => match app.dj_mood_gate() {
            Some(gate) => gated(gate, None),
            None => sheet(moods_value(s.moods)),
        },
        Row::Genres => match app.dj_floor_gate(dj::FLOOR_FILTERS) {
            Some(gate) => gated(gate, None),
            None => {
                let library = app.dj_library();
                let none = Vec::new();
                sheet(genres_value(library.genre_mode, if library.genre_filter { &library.genres } else { &none }))
            }
        },
        Row::Rating => {
            if app.dj_target_is_peer() {
                return gated(DjGate::NotOnPeers, None);
            }
            sheet(rating_value(app.dj_library().min_rating))
        }
        Row::Length => match app.dj_floor_gate(dj::FLOOR_LENGTH) {
            Some(gate) => gated(gate, None),
            None => {
                let (min, max) = s.length_bounds().unwrap_or((None, None));
                sheet(length_value(min, max))
            }
        },
        Row::Beat => match app.dj_floor_gate(dj::FLOOR_FILTERS) {
            Some(gate) => gated(gate, None),
            None => sheet(tempo_value(s.bpm, s.bpm_tolerance)),
        },
        Row::Harmonic => match app.dj_floor_gate(dj::FLOOR_FILTERS) {
            Some(gate) => gated(gate, None),
            None => RowView { kind: Kind::Switch(s.harmonic), ..sheet((String::new(), Tone::Neutral)) },
        },
        Row::SkipWords => {
            let none = Vec::new();
            sheet(words_value(if s.keyword_filter { &s.keywords } else { &none }))
        }
        Row::Cooldown => sheet(cooldown_value(s.artist_cooldown)),
    }
}

// ── Layout ──────────────────────────────────────────────────────────────────

/// What the room's height affords beyond its rows (the canvas's "The
/// floor"): the two blank rows between the groups first; then Start / Stop
/// as the 3-row frame, with the air around it as the rows allow; then
/// Harmonic's line. Without the frame, Start is the kit's one-row slab on
/// the title row. Nothing ever scrolls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Layout {
    tall: bool,
    blank_title: bool,
    blank_button: bool,
    hint: bool,
    gap_must: bool,
    gap_prefer: bool,
}

fn layout(rows: u16, play_from: bool) -> Layout {
    // The title, the session rows, two headers, five and four rows.
    let base = 14 + u16::from(play_from);
    let spare = rows.saturating_sub(base);
    let mut l = Layout { gap_must: spare >= 1, gap_prefer: spare >= 2, ..Layout::default() };
    match spare.saturating_sub(2) {
        0 => {}
        1 | 2 => l.hint = true,
        3 => l.tall = true,
        4 => (l.tall, l.blank_button) = (true, true),
        5 => (l.tall, l.blank_button, l.blank_title) = (true, true, true),
        _ => (l.tall, l.blank_button, l.blank_title, l.hint) = (true, true, true, true),
    }
    l
}

/// The controls the keyboard walks: the title row's, Start / Stop, and the
/// rows drawn that the server can do.
fn items(gui: &Gui) -> Vec<Item> {
    let play_from = shows_play_from(gui);
    let mut out = vec![Item::Preview, Item::Toggle];
    for row in SESSION.into_iter().chain(MUST).chain(PREFER) {
        if row == Row::PlayFrom && !play_from {
            continue;
        }
        if row_view(gui, row).kind != Kind::Gated {
            out.push(Item::Row(row));
        }
    }
    out
}

// ── The room ────────────────────────────────────────────────────────────────

/// The nav's Auto DJ row (entry point 2): the room opens with its cursor
/// stowed and, with the DJ off, asks Play from's server what it offers so
/// the rows fit it (clause 50). The hub calls this from `Act::Nav`.
pub(crate) fn open_room(gui: &mut Gui) {
    gui.dj.cursor = None;
    gui.dj.first = None;
    let effects = gui.app.dj_room_opened();
    gui.pend(effects);
}

/// Whether one of the DJ's layers owns the pointer and keyboard: the
/// opening question, a sheet, First song's list, quick config, or the
/// empty queue's list.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.app.dj_chooser.is_some()
        || gui.dj.sheet.is_some()
        || gui.dj.first.is_some()
        || gui.dj.quick.is_some()
        || gui.dj.recent.is_some()
        || super::moods::dj_open(gui)
}

/// Whether the room is the Library's active one — a nav room, so the
/// Now Playing screen over it counts as away.
fn in_room(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == DJ_NAV
}

fn down_glyph() -> &'static str {
    if legacy_conhost() { "v" } else { "▾" }
}

/// The room (clauses 40–53): the name, `Preview picks ▸` at the right;
/// Start / Stop; then the session's rows, what every pick must match, and
/// what the DJ prefers. A nav room has no way back — the nav column is
/// right there.
pub(crate) fn draw_room(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    let w = content.width.min(ROW_W);
    let (x, right, bottom) = (content.x, content.x + w, content.bottom());
    let play_from = shows_play_from(gui);
    let lay = layout(content.height, play_from);
    let mut y = content.y;
    gui.dj.first_anchor = None;

    put(frame, x, y, &t!("gui.dj.title"), Style::default().add_modifier(Modifier::BOLD));
    let preview_end = if lay.tall { right } else { draw_slab(frame, gui, right, y).saturating_sub(2) };
    draw_preview(frame, gui, preview_end, y);
    y += 1;
    if lay.tall {
        y += u16::from(lay.blank_title);
        draw_button(frame, gui, Rect { x, y, width: w, height: 3 });
        y += 3 + u16::from(lay.blank_button);
    }
    let groups: [(Option<String>, bool, &[Row]); 3] = [
        (None, false, &SESSION),
        (Some(t!("gui.dj.must_match").to_uppercase()), lay.gap_must, &MUST),
        (Some(t!("gui.dj.prefer").to_uppercase()), lay.gap_prefer, &PREFER),
    ];
    for (header, gap, rows) in groups {
        y += u16::from(gap);
        if let Some(header) = header {
            if y >= bottom {
                return;
            }
            put(frame, x, y, &super::bar::clip(&header, w as usize), dim());
            y += 1;
        }
        for &row in rows {
            if row == Row::PlayFrom && !play_from {
                continue;
            }
            if y >= bottom {
                return;
            }
            let rect = Rect { x, y, width: w, height: 1 };
            draw_row(frame, gui, rect, row);
            if row == Row::FirstSong {
                gui.dj.first_anchor = Some(rect);
            }
            y += 1;
            if row == Row::Harmonic && lay.hint && y < bottom {
                let hint = t!("gui.dj.harmonic_hint");
                put(frame, x + 2, y, &super::bar::clip(&hint, w.saturating_sub(2) as usize), dim());
                y += 1;
            }
        }
    }
}

/// `Preview picks ▸`, ending at `end` on the title row (clause 53): a text
/// button; the word that says it is asking while the picks are out.
fn draw_preview(frame: &mut Frame, gui: &mut Gui, end: u16, y: u16) {
    if gui.app.dj_panel.sample_pending && gui.dj.sheet.is_none() {
        let words = t!("gui.dj.picking");
        put(frame, end.saturating_sub(width(&words) as u16), y, &words, accent());
        return;
    }
    let label = format!("{} {}", t!("gui.dj.preview"), super::forward_glyph());
    let lw = width(&label) as u16;
    let x = end.saturating_sub(lw);
    if gui.dj.cursor == Some(Item::Preview) {
        let rect = Rect { x, y, width: lw, height: 1 };
        put(frame, x, y, &label, accent().add_modifier(Modifier::BOLD));
        gui.ui.click(rect, Act::DjPreview);
    } else {
        text_button(frame, gui, x, y, &label, false, Act::DjPreview);
    }
}

fn start_label(armed: bool) -> String {
    if armed {
        t!("gui.dj.stop_short").to_string()
    } else {
        format!("{} {}", t!("gui.dj.start_short"), super::forward_glyph())
    }
}

/// Start, or the lit Stop (clause 40): the button is the status. A 3-row
/// frame the rows' width — the accent while off, the OK colour while the
/// DJ runs, never the destructive one — through the same toggle every
/// entry point uses.
fn draw_button(frame: &mut Frame, gui: &mut Gui, rect: Rect) {
    let armed = gui.app.dj_armed();
    let hover = gui.ui.hovers(rect);
    let color = match (hover, armed) {
        (true, _) => th().bright,
        (false, true) => th().ok,
        (false, false) => th().accent,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let label = Span::styled(start_label(armed), Style::default().fg(color).add_modifier(Modifier::BOLD));
    frame.render_widget(Paragraph::new(label).alignment(Alignment::Center), inner);
    gui.ui.click(rect, Act::DjStartStop);
    if gui.dj.cursor == Some(Item::Toggle) && !hover {
        cursor_ring(frame, rect, Style::default().fg(th().bright).add_modifier(Modifier::BOLD));
    }
}

/// The same button where the room is short: the kit's one-row slab ending
/// at `right` on the title row. Returns its first cell.
fn draw_slab(frame: &mut Frame, gui: &mut Gui, right: u16, y: u16) -> u16 {
    let armed = gui.app.dj_armed();
    let label = format!(" {} ", start_label(armed));
    let lw = width(&label) as u16;
    let rect = Rect { x: right.saturating_sub(lw), y, width: lw, height: 1 };
    let lit = gui.ui.hovers(rect) || gui.dj.cursor == Some(Item::Toggle);
    let ground = match (lit, armed) {
        (true, _) => th().bright,
        (false, true) => th().ok,
        (false, false) => th().accent,
    };
    put(frame, rect.x, y, &label, Style::default().bg(ground).fg(th().on_accent).add_modifier(Modifier::BOLD));
    gui.ui.click(rect, Act::DjStartStop);
    rect.x
}

/// One row: its name, its value in its tone ending two cells before the
/// chevron, and `▸` or `▾`; the keyboard's hand as the accent slab; the
/// pointer brightening the name and the chevron. A row the server can't do
/// is DIM whole, takes no click, and says why where the value would be.
fn draw_row(frame: &mut Frame, gui: &mut Gui, rect: Rect, row: Row) {
    let view = row_view(gui, row);
    let gated = view.kind == Kind::Gated;
    let held = !gated && (gui.dj.cursor == Some(Item::Row(row)) || (row == Row::FirstSong && gui.dj.first.is_some()));
    let hover = !gated && !held && gui.ui.hovers(rect);
    if held {
        frame.render_widget(Block::default().style(sel()), rect);
    }
    let value_end = rect.right().saturating_sub(2);
    // A reason never loses its words: where the two do not fit, the name
    // gives way — its row has not moved, so it is still found.
    let reason_w = if gated { width(&view.value) } else { 0 };
    let label_room = (rect.width as usize).saturating_sub(if gated { reason_w + 4 } else { 4 });
    let label_w = width(&view.label).min(label_room);
    match view.kind {
        Kind::Switch(on) => {
            let (check_on, check_off) = super::check_glyphs();
            let style = match (held, on) {
                (true, _) => sel(),
                (false, true) => Style::default().fg(th().ok),
                (false, false) => dim(),
            };
            put(frame, value_end.saturating_sub(3), rect.y, if on { check_on } else { check_off }, style);
        }
        _ => {
            let room = (rect.width as usize).saturating_sub(label_w + 2 + 2);
            let words = if view.tone == Tone::Warning { format!("! {}", view.value) } else { view.value.clone() };
            let shown = super::bar::clip(&words, room);
            let style = if held { sel() } else { view.tone.style() };
            put(frame, value_end.saturating_sub(width(&shown) as u16), rect.y, &shown, style);
            // A value that did not fit says itself whole on a dwell.
            if width(&words) > room && !gated {
                gui.ui.tip(rect, words);
            }
        }
    }
    let label_style = match (gated, held, hover) {
        (true, _, _) => dim(),
        (false, true, _) => sel().add_modifier(Modifier::BOLD),
        (false, false, true) => bright_bold(),
        (false, false, false) => Style::default(),
    };
    put(frame, rect.x, rect.y, &super::bar::clip(&view.label, label_room), label_style);
    let chevron = match view.kind {
        Kind::Sheet => Some(super::forward_glyph()),
        Kind::List => Some(down_glyph()),
        Kind::Switch(_) | Kind::Gated => None,
    };
    if let Some(chevron) = chevron {
        let style = match (held, hover) {
            (true, _) => sel(),
            (false, true) => bright_bold(),
            (false, false) => dim(),
        };
        put(frame, rect.right().saturating_sub(1), rect.y, chevron, style);
    }
    if !gated {
        gui.ui.click(rect, Act::DjRow(row));
    }
    if let Some(tip) = view.tip {
        gui.ui.tip(rect, tip);
    }
}

// ── Layers ──────────────────────────────────────────────────────────────────

/// The DJ's layers, in the order they stack: First song's list, the empty
/// queue's list, quick config, the sheet, the opening question.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    draw_first_list(frame, gui, area);
    draw_recent_list(frame, gui, area);
    quick::draw(frame, gui, area);
    sheets::draw(frame, gui, area);
    draw_chooser(frame, gui, area);
}

const FIRST: [EmptyQueueStart; 4] =
    [EmptyQueueStart::Ask, EmptyQueueStart::Random, EmptyQueueStart::Pick, EmptyQueueStart::Recent];

/// First song's list (the kit's Dropdown): hanging under its row, its last
/// cell under the chevron's; the current answer wears `•`, the keyboard's
/// cursor the accent slab; a pick applies and closes, and a click anywhere
/// else closes and keeps.
fn draw_first_list(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(cursor) = gui.dj.first else { return };
    let Some(anchor) = gui.dj.first_anchor.filter(|_| in_room(gui)) else {
        gui.dj.first = None;
        return;
    };
    gui.ui.click(area, Act::DjFirstClose);
    let words: Vec<String> = FIRST.iter().map(|choice| first_words(*choice)).collect();
    let inner_w = words.iter().map(|w| width(w)).max().unwrap_or(0) as u16 + 4;
    let w = (inner_w + 2).min(area.width);
    let h = FIRST.len() as u16 + 2;
    let rect = Rect {
        x: (anchor.right() + 1).saturating_sub(w).min(area.width.saturating_sub(w)),
        y: (anchor.y + 1).min(area.height.saturating_sub(h)),
        width: w,
        height: h,
    };
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    gui.ui.click(rect, Act::Guard);
    let now = gui.app.dj.empty_queue;
    for (i, (choice, word)) in FIRST.iter().zip(&words).enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let held = i == cursor;
        let hover = gui.ui.hovers(row);
        if held {
            frame.render_widget(Block::default().style(sel()), row);
        }
        if *choice == now {
            put(frame, row.x + 1, row.y, super::bullet_glyph(), if held { sel() } else { accent() });
        }
        let style = match (held, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        put(frame, row.x + 3, row.y, word, style);
        gui.ui.click(row, Act::DjFirst(*choice));
    }
}

/// A song as Recently played names it: the title, then the artist DIM.
fn song_words(track: &crate::api::types::Track) -> (String, Option<String>) {
    let title = track.title_or_file().to_string();
    let artist = track.metadata.artist.clone().filter(|a| !a.trim().is_empty());
    (title, artist)
}

/// One row of a Recently played list, in `rect`: the keyboard's cursor the
/// accent slab, the pointer BRIGHT.
fn draw_song_row(
    frame: &mut Frame,
    gui: &mut Gui,
    rect: Rect,
    track: &crate::api::types::Track,
    held: bool,
    act: Act,
) {
    let hover = gui.ui.hovers(rect);
    if held {
        frame.render_widget(Block::default().style(sel()), rect);
    }
    let (title, artist) = song_words(track);
    let room = rect.width.saturating_sub(2) as usize;
    let shown = super::bar::clip(&title, room);
    let style = match (held, hover) {
        (true, _) => sel().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => Style::default(),
    };
    put(frame, rect.x + 1, rect.y, &shown, style);
    let used = width(&shown);
    if let Some(artist) = artist
        && used + 4 < room
    {
        let tail = format!(" · {artist}");
        let tail = super::bar::clip(&tail, room - used);
        put(frame, rect.x + 1 + used as u16, rect.y, &tail, if held { sel() } else { dim() });
    }
    gui.ui.click(rect, act);
}

/// A Recently played list in `rect` (a frame around its rows): `cursor`
/// kept in the window, a song a row.
fn draw_songs(frame: &mut Frame, gui: &mut Gui, rect: Rect, cursor: usize) {
    gui.ui.overlay(rect);
    let inner = crate::kit::frame_at(frame, rect, th().accent);
    gui.ui.click(rect, Act::Guard);
    let songs = gui.app.dj_recent_songs().to_vec();
    let visible = inner.height as usize;
    let first = cursor.saturating_sub(visible.saturating_sub(1)).min(songs.len().saturating_sub(visible));
    for (i, track) in songs.iter().enumerate().skip(first).take(visible) {
        let row = Rect { x: inner.x, y: inner.y + (i - first) as u16, width: inner.width, height: 1 };
        draw_song_row(frame, gui, row, track, i == cursor, Act::DjRecentPick(i));
    }
}

/// The empty queue's Recently played list (clause 16): under its opener,
/// the panel's width.
fn draw_recent_list(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(cursor) = gui.dj.recent else { return };
    let songs = gui.app.dj_recent_songs().len();
    let anchor = gui.dj.recent_anchor.take();
    let standing = songs > 0 && gui.app.dj_armed() && gui.app.queue.items.is_empty();
    let (Some(anchor), true) = (anchor, standing) else {
        gui.dj.recent = None;
        return;
    };
    gui.ui.click(area, Act::DjRecentClose);
    let bottom = super::bar::top(area, gui.footer());
    let h = (songs as u16 + 2).min(bottom.saturating_sub(anchor.y + 1)).max(3);
    let rect = Rect {
        x: anchor.x,
        y: anchor.y + 1,
        width: anchor.width.min(area.width.saturating_sub(anchor.x)),
        height: h,
    };
    draw_songs(frame, gui, rect, cursor.min(songs - 1));
}

/// Whether the opening question offers Recently played: the songs are in
/// hand and there is one to offer (it is left out otherwise).
fn chooser_offers_recent(gui: &Gui) -> bool {
    !gui.app.dj_recent_songs().is_empty()
}

/// The opening question (clause 2), under the row's own name: Random,
/// Choose, Recently played with its list, and Remember — the kit's modal
/// with its title on the top border; Esc leaves the DJ off. The App holds
/// the row and the box; the shell draws and relays.
fn draw_chooser(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let recent = chooser_offers_recent(gui);
    let Some(chooser) = gui.app.dj_chooser.as_mut() else { return };
    // The rows: the two answers, Recently played where it has songs, a
    // blank, and the box.
    let remember_row = 2 + usize::from(recent);
    // Remembered as the way to start, the list comes down on its own once
    // its songs are in hand — the hand on its row.
    if chooser.recent_open && recent && !chooser.recent_seated {
        chooser.row = 2;
        chooser.recent_seated = true;
    }
    chooser.row = chooser.row.min(remember_row);
    let chooser = chooser.clone();
    let open = chooser.recent_open && recent;

    // The screen beneath is inert; a click anywhere else dismisses — the
    // DJ stays off.
    gui.ui.clear_registries();
    gui.ui.click(area, Act::DjCancel);
    let h = (remember_row as u16 + 2) + 2;
    let rect = crate::kit::modal_rect(area, CHOOSER_W, h, h);
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    super::keep::modal_box(frame, rect);
    gui.ui.click(rect, Act::Guard);
    super::keep::border_title(frame, rect, &t!("gui.dj.first_song"));
    let (x0, top) = (rect.x + 2, rect.y + 1);
    let row_w = rect.width.saturating_sub(4);
    let (check_on, check_off) = super::check_glyphs();
    let mut labels: Vec<(String, Act)> = vec![
        (t!("gui.dj.first_random").to_string(), Act::DjChoose(0)),
        (t!("gui.dj.first_choose").to_string(), Act::DjChoose(1)),
    ];
    if recent {
        labels.push((format!("{} {}", t!("gui.dj.first_recent"), down_glyph()), Act::DjRecentToggle));
    }
    for (i, (label, act)) in labels.into_iter().enumerate() {
        let row = Rect { x: x0, y: top + i as u16, width: row_w, height: 1 };
        let held = chooser.row == i && !open;
        let lit = i == 2 && open;
        let hover = gui.ui.hovers(row);
        if held {
            frame.render_widget(Block::default().style(sel()), row);
        }
        let style = match (held, lit, hover) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => accent().add_modifier(Modifier::BOLD),
            (false, false, true) => bright_bold(),
            (false, false, false) => Style::default(),
        };
        put(frame, x0, row.y, &label, style);
        gui.ui.click(row, act);
    }
    let y = top + remember_row as u16 + 1;
    let row = Rect { x: x0, y, width: row_w, height: 1 };
    let held = chooser.row == remember_row && !open;
    let hover = gui.ui.hovers(row);
    if held {
        frame.render_widget(Block::default().style(sel()), row);
    }
    let glyph_style = match (held, chooser.remember) {
        (true, _) => sel(),
        (false, true) => Style::default().fg(th().ok),
        (false, false) => dim(),
    };
    put(frame, x0, y, if chooser.remember { check_on } else { check_off }, glyph_style);
    let style = match (held, hover) {
        (true, _) => sel().add_modifier(Modifier::BOLD),
        (false, true) => bright_bold(),
        (false, false) => Style::default(),
    };
    put(frame, x0 + 4, y, &t!("gui.dj.remember_short"), style);
    gui.ui.click(row, Act::DjRemember);

    if open {
        // The list hangs from its row, twenty cells in, to the modal's
        // last column of text — over the box's row and past the frame.
        let songs = gui.app.dj_recent_songs().len() as u16;
        let ly = top + 3;
        let lx = rect.x + 20;
        let lh = (songs + 2).min(area.height.saturating_sub(ly + 1)).max(3);
        let list = Rect { x: lx, y: ly, width: rect.right().saturating_sub(lx + 2), height: lh };
        draw_songs(frame, gui, list, chooser.recent_row.min(songs.saturating_sub(1) as usize));
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The DJ's keys: its layers outrank everything, the top one first; Esc
/// leaves the opening-song road; then the room's own. `None` when none of
/// them is up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.app.dj_chooser.is_some() {
        chooser_key(gui, key);
        return Some(false);
    }
    // Smart filters is the mood modal, whatever room it was opened over.
    if super::moods::dj_open(gui) {
        return super::moods::handle_key(gui, key);
    }
    if let Some(quit) = sheets::handle_key(gui, key) {
        return Some(quit);
    }
    if let Some(quit) = quick::handle_key(gui, key) {
        return Some(quit);
    }
    if let Some(row) = gui.dj.first {
        match key.code {
            KeyCode::Esc => gui.dj.first = None,
            KeyCode::Up => gui.dj.first = Some(row.saturating_sub(1)),
            KeyCode::Down => gui.dj.first = Some((row + 1).min(FIRST.len() - 1)),
            KeyCode::Enter | KeyCode::Char(' ') => {
                return Some(gui.act(Act::DjFirst(FIRST[row.min(FIRST.len() - 1)])));
            }
            _ => {}
        }
        return Some(false);
    }
    if let Some(row) = gui.dj.recent {
        let last = gui.app.dj_recent_songs().len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => gui.dj.recent = None,
            KeyCode::Up => gui.dj.recent = Some(row.saturating_sub(1)),
            KeyCode::Down => gui.dj.recent = Some((row + 1).min(last)),
            KeyCode::Enter => return Some(gui.act(Act::DjRecentPick(row.min(last)))),
            _ => {}
        }
        return Some(false);
    }
    // Not on the MP3 Player tab, whose Esc is the page's (mp3-player-screen
    // contract, clause 12); entering the tab lets the pick go anyway.
    if gui.app.capture == Some(Capture::DjSeed)
        && key.code == KeyCode::Esc
        && gui.screen != Screen::Device
    {
        gui.forward(Action::Cancel);
        return Some(false);
    }
    if !in_room(gui) {
        return None;
    }
    room_key(gui, key)
}

/// The opening question's keys: ↑↓ walk its rows, Enter answers — on
/// Recently played it lets the list down, and inside the list picks the
/// song — Space ticks Remember from anywhere, Esc folds the list, then
/// leaves the DJ off.
fn chooser_key(gui: &mut Gui, key: KeyEvent) {
    let songs = gui.app.dj_recent_songs().len();
    let Some(chooser) = gui.app.dj_chooser.as_mut() else { return };
    let recent = songs > 0;
    let remember_row = 2 + usize::from(recent);
    if chooser.recent_open && recent {
        match key.code {
            KeyCode::Up => chooser.recent_row = chooser.recent_row.saturating_sub(1),
            KeyCode::Down => chooser.recent_row = (chooser.recent_row + 1).min(songs - 1),
            KeyCode::Esc | KeyCode::Left => chooser.recent_open = false,
            KeyCode::Char(' ') => chooser.remember = !chooser.remember,
            KeyCode::Enter => {
                let row = chooser.recent_row.min(songs - 1);
                gui.act(Act::DjRecentPick(row));
            }
            _ => {}
        }
        return;
    }
    match key.code {
        KeyCode::Esc => {
            gui.act(Act::DjCancel);
        }
        KeyCode::Up => chooser.row = chooser.row.saturating_sub(1),
        KeyCode::Down => chooser.row = (chooser.row + 1).min(remember_row),
        KeyCode::Char(' ') => chooser.remember = !chooser.remember,
        KeyCode::Enter | KeyCode::Right => {
            let act = match chooser.row {
                0 => Act::DjChoose(0),
                1 => Act::DjChoose(1),
                row if row == remember_row => Act::DjRemember,
                _ => Act::DjRecentToggle,
            };
            // → only lets the list down; Enter answers.
            if key.code == KeyCode::Enter || act == Act::DjRecentToggle {
                gui.act(act);
            }
        }
        _ => {}
    }
}

/// Move the keyboard's hand along the controls that exist right now.
fn move_cursor(gui: &mut Gui, delta: i32) {
    let items = items(gui);
    let at = gui.dj.cursor.and_then(|c| items.iter().position(|i| *i == c));
    let next = match at {
        Some(i) => (i as i32 + delta).clamp(0, items.len() as i32 - 1) as usize,
        // From rest: ↓ takes Start / Stop, ↑ the last row.
        None if delta < 0 => items.len() - 1,
        None => 1,
    };
    gui.dj.cursor = Some(items[next]);
}

fn room_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    match gui.dj.cursor {
        None => match key.code {
            KeyCode::Down => move_cursor(gui, 1),
            KeyCode::Up => move_cursor(gui, -1),
            _ => return None,
        },
        Some(item) => match key.code {
            KeyCode::Up | KeyCode::BackTab => move_cursor(gui, -1),
            KeyCode::Down | KeyCode::Tab => move_cursor(gui, 1),
            KeyCode::Esc => gui.dj.cursor = None,
            KeyCode::Enter | KeyCode::Char(' ') => {
                let act = match item {
                    Item::Preview => Act::DjPreview,
                    Item::Toggle => Act::DjStartStop,
                    Item::Row(row) => Act::DjRowKey(row),
                };
                return Some(gui.act(act));
            }
            _ => return None,
        },
    }
    Some(false)
}

// ── Acts ────────────────────────────────────────────────────────────────────

/// One edit, through the App, then saved: the session-wide settings ride
/// the player prefs, written once the edits rest — a held ← on a bar is a
/// step per key repeat, and each used to be a write of its own (performance
/// audit #86) — and a per-library rule comes back as its own effect,
/// written at once (clause 51): the book of servers is rebuilt from the
/// file on every reload, and a rule still waiting would be undone by one.
pub(crate) fn edit(gui: &mut Gui, edit: DjEdit) {
    let effects = gui.app.dj_edit(edit);
    gui.pend(effects);
    gui.save_soon();
}

/// A row pressed: its sheet, its list, or — Harmonic mixing — its switch.
/// `key` says the keyboard pressed it, so the layer opens with its hand up.
fn press_row(gui: &mut Gui, row: Row, key: bool) {
    if row_view(gui, row).kind == Kind::Gated {
        return;
    }
    // A click seats the hand only where the keyboard already had it up.
    if key || gui.dj.cursor.is_some() {
        gui.dj.cursor = Some(Item::Row(row));
    }
    match row {
        Row::FirstSong => {
            let now = gui.app.dj.empty_queue;
            gui.dj.first = Some(FIRST.iter().position(|c| *c == now).unwrap_or(0));
        }
        Row::Harmonic => edit(gui, DjEdit::Step(crate::tui::app::DjRow::Harmonic, 1)),
        Row::Smart => super::moods::open_for_dj(gui, key),
        Row::PlayFrom => sheets::open(gui, Sheet::PlayFrom, key),
        Row::Batch => sheets::open(gui, Sheet::Batch, key),
        Row::Sonic => sheets::open(gui, Sheet::Sonic, key),
        Row::Genres => sheets::open(gui, Sheet::Genres, key),
        Row::Rating => sheets::open(gui, Sheet::Rating, key),
        Row::Length => sheets::open(gui, Sheet::Length, key),
        Row::Beat => sheets::open(gui, Sheet::Tempo, key),
        Row::SkipWords => sheets::open(gui, Sheet::SkipWords, key),
        Row::Cooldown => sheets::open(gui, Sheet::Cooldown, key),
    }
}

/// Start through the shared toggle (the opening question, the seed rule),
/// Stop wherever the DJ is armed — one button, one meaning.
pub(crate) fn start_stop(gui: &mut Gui) {
    if gui.app.dj_armed() {
        let mut effects = gui.app.disarm_dj();
        // Off, the rows describe Play from's server again.
        effects.extend(gui.app.dj_room_opened());
        gui.pend(effects);
    } else {
        gui.forward(Action::ToggleAutoDj);
    }
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
            gui.save_soon();
        }
        Act::DjRemember => {
            if let Some(chooser) = gui.app.dj_chooser.as_mut() {
                chooser.remember = !chooser.remember;
            }
        }
        Act::DjCancel => gui.forward(Action::Cancel),
        Act::DjRecentToggle => {
            if let Some(chooser) = gui.app.dj_chooser.as_mut() {
                chooser.recent_open = !chooser.recent_open;
                chooser.recent_seated = true;
                chooser.recent_row = 0;
                chooser.row = 2;
            }
        }
        // One of Recently played's songs: the opening question's list, or
        // the armed empty queue's.
        Act::DjRecentPick(index) => {
            let remember = gui.app.dj_chooser.as_ref().is_some_and(|c| c.remember);
            gui.dj.recent = None;
            let effects = gui.app.answer_dj_start_recent(index, remember);
            gui.pend(effects);
            gui.save_now();
        }
        // The empty queue's openers (clause 16).
        Act::DjSurprise => gui.forward(Action::DjSurprise),
        Act::DjPick => gui.forward(Action::DjPick),
        Act::DjRecentOpen => gui.dj.recent = Some(0),
        Act::DjRecentClose => gui.dj.recent = None,
        Act::DjStartStop => {
            if gui.dj.cursor.is_some() {
                gui.dj.cursor = Some(Item::Toggle);
            }
            start_stop(gui);
        }
        Act::DjRow(row) => press_row(gui, row, false),
        Act::DjRowKey(row) => press_row(gui, row, true),
        Act::DjPreview => {
            if gui.dj.cursor.is_some() {
                gui.dj.cursor = Some(Item::Preview);
            }
            sheets::open(gui, Sheet::Preview, false);
            let effects = gui.app.sample_dj();
            gui.pend(effects);
        }
        Act::DjFirst(choice) => {
            gui.dj.first = None;
            edit(gui, DjEdit::EmptyQueue(choice));
        }
        Act::DjFirstClose => gui.dj.first = None,
        Act::DjSheet(sheet_act) => sheets::act(gui, sheet_act),
        Act::DjQuick(quick_act) => quick::act(gui, quick_act),
        _ => return false,
    }
    true
}

/// The wheel over the DJ's surfaces: a sheet's list, and a Recently played
/// list — which walks under it, its window following the cursor, so every
/// song of the twenty is the pointer's to reach. True when a layer of the
/// DJ's is up or the room is — the room itself has nothing to scroll.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) -> bool {
    if sheets::wheel(gui, delta) {
        return true;
    }
    let last = gui.app.dj_recent_songs().len().saturating_sub(1);
    let step = |row: usize| row.saturating_add_signed(delta as isize).min(last);
    if let Some(chooser) = gui.app.dj_chooser.as_mut() {
        if chooser.recent_open {
            chooser.recent_row = step(chooser.recent_row);
        }
        return true;
    }
    if let Some(row) = gui.dj.recent {
        gui.dj.recent = Some(step(row));
        return true;
    }
    modal_open(gui) || in_room(gui)
}

/// What every batch of the App's is followed by ([`Gui::pend`]): an armed
/// DJ whose queue has run out asks, once, for the songs played last on its
/// server — the empty state's third opener (clause 16).
pub(crate) fn follow(gui: &mut Gui) {
    let empty = gui.app.dj_armed() && gui.app.queue.items.is_empty();
    if !empty {
        gui.dj.recent_asked = false;
        gui.dj.recent = None;
        return;
    }
    if std::mem::replace(&mut gui.dj.recent_asked, true) {
        return;
    }
    if let Some(identity) = gui.app.dj_server.clone() {
        let effects = gui.app.dj_want_recent(&identity);
        gui.app.note_pending(&effects);
        gui.pending.extend(effects);
    }
}

/// The tips line for the DJ's surfaces, when one is up.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    if gui.app.dj_chooser.is_some() {
        return Some(t!("gui.dj.start_keys").to_string());
    }
    if let Some(tip) = sheets::tips(gui) {
        return Some(tip);
    }
    if gui.dj.quick.is_some() {
        return Some(t!("gui.tips.dj_quick").to_string());
    }
    if gui.dj.first.is_some() || gui.dj.recent.is_some() {
        return Some(t!("gui.tips.dj_list").to_string());
    }
    if gui.app.capture == Some(Capture::DjSeed) {
        return Some(t!("gui.tips.dj_pick").to_string());
    }
    if !in_room(gui) {
        return None;
    }
    let tip = match gui.dj.cursor {
        None => t!("gui.tips.dj_room"),
        Some(Item::Toggle | Item::Preview) => t!("gui.tips.dj_button"),
        Some(Item::Row(Row::Harmonic)) => t!("gui.tips.dj_switch"),
        Some(Item::Row(_)) => t!("gui.tips.dj_row"),
    };
    Some(tip.to_string())
}

/// The banner the "Let me choose" road wears (clause 4), when it is armed.
pub(crate) fn banner(gui: &Gui) -> Option<String> {
    (gui.app.capture == Some(Capture::DjSeed)).then(|| t!("gui.dj.pick_banner").to_string())
}

/// The empty queue panel's openers (clause 16): while the DJ is armed, the
/// hint and the ways to give it an opening song, under the label — a random
/// one, one from the library, and, where the server has some, one of the
/// songs played last.
pub(crate) fn draw_empty_queue(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, width: usize) {
    gui.dj.recent_anchor = None;
    if !gui.app.dj_armed() {
        return;
    }
    let forward = super::forward_glyph();
    // The hint wraps in the narrow panel; the buttons follow it.
    let mut y = y + 1;
    for line in wrap_whole(&t!("gui.queue.empty_dj_hint"), width).into_iter().take(3) {
        put(frame, x, y, &line, dim());
        y += 1;
    }
    let random = format!("{} {forward}", t!("gui.queue.empty_dj_random"));
    let choose = format!("{} {forward}", t!("gui.queue.empty_dj_choose"));
    text_button(frame, gui, x, y + 1, &random, true, Act::DjSurprise);
    text_button(frame, gui, x, y + 2, &choose, false, Act::DjPick);
    if !gui.app.dj_recent_songs().is_empty() {
        let recent = format!("{} {}", t!("gui.dj.first_recent"), down_glyph());
        if gui.dj.recent.is_some() {
            put(frame, x, y + 3, &recent, accent().add_modifier(Modifier::BOLD));
        } else {
            text_button(frame, gui, x, y + 3, &recent, false, Act::DjRecentOpen);
        }
        gui.dj.recent_anchor = Some(Rect { x, y: y + 3, width: width as u16, height: 1 });
    }
}

#[cfg(test)]
mod tests;
