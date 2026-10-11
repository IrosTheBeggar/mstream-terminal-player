//! The Discover room: the history-seeded PLAY-NOW page — what you finished,
//! and what sounds like it that you can play right now, in your library and
//! on your connected servers — worn terminal on the shared `App` state
//! machine (`App.taste`, src/tui/app/taste.rs), the sonic room's precedent:
//! the room draws the App's row model and calls its verbs, and never touches
//! the TUI's Discover tab, its panel or its capture (clause 67).
//!
//! Contract: docs/ux-contracts/discovery-page.md (clause numbers below cite
//! it); design of record: mStream's docs/discovery-page-plan.md §2.13 (the
//! 2026-10-11 play-now page, canvas https://claude.ai/artifact/
//! AyZ2yH6431ajvmWgxjZuxo). The user found the first page too busy and
//! doing too much, so the room shows only songs you can play now — no
//! percentages, no counts, no "never played", no p2p network leads (a later
//! Explore room draws those). Where the webapp and this surface part — album
//! ROWS ahead of the songs, not cards; the library filter one checkbox at
//! the bottom row's left — the contract's log says why.
//!
//! One answer, two tabs, top to bottom (clause 90): the header with the
//! window dropdown, the state line, the fallback card while nothing is
//! finished yet, the tabs, the seeds line, the tab's table, the bottom row
//! with the "Include songs I've played lately" checkbox and the whole-list
//! verbs. The seeds modal and the window's list draw last, from
//! [`draw_modals`], so they win the pointer.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{SeedSource, TasteOutcome, Track};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, frame_at, modal_close, modal_frame_on, scroll_list, width};
use crate::tui::app::{App, Capture, Origin, TasteLatch, TastePinRefusal, TasteRow, TasteSlot, TasteTab, TasteVerb};

use super::{Act, Gui, List, RowVerb, Screen, accent, bright_bold, bullet_glyph, check_glyphs, put, sel};

/// The window dropdown's choices, in days (0 = all time) — clause 15.
const WINDOWS: [u32; 4] = [7, 30, 90, 0];

/// The window the header shows before the server or the prefs said one:
/// the server's own default (plan §2.2).
const WINDOW_DEFAULT: u32 = 30;

/// The kit table's column gap.
const GAP: u16 = 2;
/// The cells before TRACK: the playing row's `▸` and its gap (clause 55).
const LEAD: u16 = 2;
/// TRACK never narrows below this: ALBUM goes first, then ON, then ARTIST
/// (clause 54).
const TRACK_MIN: u16 = 24;
/// ARTIST or ALBUM narrower than this (and than its own words) is not
/// drawn at all.
const FLEX_MIN: u16 = 8;
/// ON's cap: a server's name.
const ON_MAX: u16 = 16;
/// The tabs' spacing — the kit's Tabs, the admin rooms' value.
const TAB_GAP: u16 = 2;
/// The fallback card's widest, and its two heights: whole, and with its
/// body sentence and blank given up at the floor (clause 90).
const CARD_W: u16 = 66;
const CARD_FULL: u16 = 7;
const CARD_COMPACT: u16 = 5;

// ── State ───────────────────────────────────────────────────────────────────

/// The room's own state: the table's cursor and wheel, the window's list or
/// the seeds modal while one is up, and where the window control stood last
/// frame. Everything the room shows lives on the shared App.
pub(crate) struct DiscoverUi {
    /// The keyboard cursor, an index into [`lines`] of the tab in front.
    /// None is the kit's resting state — ↓ picks it up, Esc stows it
    /// (clause 57).
    pub cursor: Option<usize>,
    pub view: ListView,
    pub menu: Option<Menu>,
    pub seeds: Option<SeedsModal>,
    /// The header's window control as last drawn: its list hangs from it.
    window_at: Rect,
    /// The local zone for "today" and "yesterday", read once.
    zone: Option<Option<crate::admin::tz::Zone>>,
}

impl DiscoverUi {
    pub(crate) fn new() -> Self {
        DiscoverUi {
            cursor: None,
            view: ListView::default(),
            menu: None,
            seeds: None,
            window_at: Rect::default(),
            zone: None,
        }
    }

    /// The keyboard's hand down: no row lit (the kit's list-cursor law).
    fn stow(&mut self) {
        self.cursor = None;
        self.view.stow();
    }
}

/// A list hanging from a control (the kit's Dropdown grammar), with its
/// keyboard cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Menu {
    /// The header's window list; the cursor indexes [`WINDOWS`].
    Window(usize),
}

/// The seeds modal's own state (clause 13): which rows were toggled, and
/// whether a pin came or went. Nothing is sent until it closes.
#[derive(Debug, Clone, Default)]
pub(crate) struct SeedsModal {
    pub cursor: Option<usize>,
    /// Hashes whose box was flipped: a history seed unchecked (dropped), a
    /// dropped one checked (restored).
    pub flipped: Vec<String>,
    pub pins_changed: bool,
    pub view: ListView,
}

/// What a pointer or key did in the room — one [`Act`] variant, the admin
/// rooms' habit, so the shell's enum stays readable.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DiscAct {
    /// A tab, clicked (clause 40).
    Tab(TasteTab),
    /// A table line with the verb the click meant: the row itself or one
    /// of its hover verbs.
    Row(usize, RowVerb),
    /// A song's `[⋯]` (the track-actions sheet), and a right click — a
    /// library song's, or a connected server's with its peer origin.
    More(usize),
    /// The header's window control, and a pick from its list.
    Window,
    WindowPick(u32),
    /// A click outside the open list closes and keeps (the Dropdown law).
    MenuClose,
    /// "Include songs I've played lately", ticked or unticked (clause 65) —
    /// the checkbox, and the empty library's button that ticks it.
    Played,
    /// The seeds line's `edit` (and `e`).
    Seeds,
    /// A modal row, clicked: toggle it, or do what it says.
    SeedRow(usize),
    /// A pin's `[X]` in the modal, by its index in the pins.
    SeedUnpin(usize),
    /// Done, Esc or the modal's `[X]`: the changes apply (clause 13).
    SeedsDone,
    /// The card's two ways forward (clauses 21, 14).
    Another,
    Choose,
    /// Try again and Check again: ask again now (clauses 81, 83).
    Again,
    /// The bottom row's whole-list verbs (clause 64).
    PlayAll,
    QueueAll,
    Shuffle,
}

// ── The row model ───────────────────────────────────────────────────────────

/// One line of the tab's table, by what it points at in the answer. Every
/// line is a row: no group lines, no gaps (clause 53).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line {
    /// `library.albums[i]`: a container row (clause 61).
    Album(usize),
    /// `library.tracks[i]`.
    Song(usize),
    /// `peers.peers[group].results[row]`: a connected server's song.
    Peer { group: usize, row: usize },
}

/// The room's gate (clause 1): the ping's two flags, and not a peer's
/// session — a guest has no history there.
pub(crate) fn gate(app: &App) -> bool {
    app.capabilities.taste() && app.session.peer.is_none()
}

/// The tabs the room draws (clause 40): Library always; Connected servers
/// while the server federates discovery, its half was not answered `null`
/// for a real seed, and it has servers to ask at all.
pub(crate) fn visible_tabs(app: &App) -> Vec<TasteTab> {
    let mut tabs = vec![TasteTab::Library];
    let peers = &app.taste.peers;
    let none_connected = peers.data.as_ref().is_some_and(|p| p.searched.peers == 0 && p.peers.is_empty());
    if app.capabilities.federation_discovery && !peers.absent && !none_connected {
        tabs.push(TasteTab::Servers);
    }
    tabs
}

/// The tab's table, in the server's order: albums before songs (clause 51);
/// the connected servers' songs that play now, one list (clause 53).
pub(crate) fn lines(app: &App, tab: TasteTab) -> Vec<Line> {
    let taste = &app.taste;
    match tab {
        TasteTab::Library => taste
            .library
            .data
            .iter()
            .flat_map(|l| (0..l.albums.len()).map(Line::Album).chain((0..l.tracks.len()).map(Line::Song)))
            .collect(),
        TasteTab::Servers => taste.peer_lines().into_iter().map(|(group, row)| Line::Peer { group, row }).collect(),
    }
}

/// Whether a tab holds rows the whole-list verbs can play.
fn playable(app: &App, tab: TasteTab) -> bool {
    match tab {
        TasteTab::Library => app.taste.library.data.as_ref().is_some_and(|l| !l.tracks.is_empty()),
        TasteTab::Servers => !app.taste.peer_lines().is_empty(),
    }
}

// ── Words ───────────────────────────────────────────────────────────────────

/// The plural form a count takes in the running locale — the DJ room's
/// `songs_words` rule, for the two languages here with more than one plural.
/// The room shows no counts; the seeds modal's ages are its only family.
fn plural_form(count: u64) -> &'static str {
    let locale = rust_i18n::locale();
    let lang: &str = &locale;
    let few_many = |n: u64| (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100));
    match &lang[..lang.len().min(2)] {
        "pl" if count == 1 => "one",
        "pl" if few_many(count) => "few",
        "pl" => "many",
        "ru" if count % 10 == 1 && count % 100 != 11 => "one",
        "ru" if few_many(count) => "few",
        "ru" => "many",
        "ja" | "zh" => "other",
        _ if count == 1 => "one",
        _ => "other",
    }
}

/// A `_*` family's words for `count` (the contract's Wording: `%{count}` is
/// the family's placeholder).
fn plural(stem: &str, count: u64) -> String {
    let key = format!("{stem}_{}", plural_form(count));
    t!(key.as_str(), count = count).to_string()
}

/// A relative age on the local clock (the seeds modal's last play, clause
/// 13): today, yesterday, n days, months or years ago. `offset` is the
/// zone's seconds east of UTC.
pub(crate) fn age_words(then: i64, now: i64, offset: i32) -> String {
    let day = |t: i64| (t + i64::from(offset)).div_euclid(86_400);
    let days = (day(now) - day(then)).max(0) as u64;
    match days {
        0 => t!("gui.discover.today").to_string(),
        1 => t!("gui.discover.yesterday").to_string(),
        2..=29 => plural("gui.discover.days_ago", days),
        30..=364 => plural("gui.discover.months_ago", (days / 30).max(1)),
        _ => plural("gui.discover.years_ago", days / 365),
    }
}

/// "Title · Artist" — the title (the file name when there is none) and the
/// artist when there is one.
fn title_artist(title: &str, artist: Option<&str>) -> String {
    match artist.map(str::trim).filter(|a| !a.is_empty()) {
        Some(artist) => format!("{title} · {artist}"),
        None => title.to_string(),
    }
}

fn track_words(track: &Track) -> String {
    title_artist(track.title_or_file(), track.metadata.artist.as_deref())
}

fn window_label(days: u32) -> String {
    if days == 0 {
        t!("gui.discover.window_all").to_string()
    } else {
        t!("gui.discover.window_days", days = days).to_string()
    }
}

/// The hover verbs' glyphs: play now, add next, add to the end, more —
/// the Files rows' four, with their conhost stand-ins.
fn verb_glyphs() -> [&'static str; 4] {
    if legacy_conhost() { ["[>]", "[^]", "[+]", "[.]"] } else { ["[▸]", "[»]", "[+]", "[⋯]"] }
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn dash() -> &'static str {
    if legacy_conhost() { "-" } else { "—" }
}

fn chevron() -> &'static str {
    if legacy_conhost() { "v" } else { "▾" }
}

/// What the header says the answer is based on (clause 11) — words, never
/// a count.
fn based_on(app: &App) -> Option<String> {
    let taste = &app.taste;
    if taste.no_history {
        return Some(t!("gui.discover.nothing_finished").to_string());
    }
    let head = taste.head.as_ref()?;
    Some(match head.seed_source {
        SeedSource::History => t!("gui.discover.based_on_history").to_string(),
        SeedSource::Counters => t!("gui.discover.based_on_counts").to_string(),
        SeedSource::Pinned => t!("gui.discover.based_on_picked").to_string(),
        SeedSource::Random => t!("gui.discover.nothing_finished").to_string(),
        SeedSource::Unknown => return None,
    })
}

/// The widened fact (clause 11), when the server widened the window.
fn widened(app: &App) -> Option<String> {
    let head = app.taste.head.as_ref()?;
    let from = head.widened_from?;
    Some(if head.seed_source == SeedSource::Counters {
        t!("gui.discover.from_counts").to_string()
    } else {
        t!("gui.discover.widened", days = from).to_string()
    })
}

/// The state line's word, its colour and its facts for the tab in front
/// (clause 41) — words only: the state is never clipped, the facts drop
/// from the right.
fn state_line(app: &App, tab: TasteTab) -> (String, Style, Vec<String>) {
    let taste = &app.taste;
    let ok = Style::default().fg(th().ok);
    let gold = Style::default().fg(th().gold);
    let grey = dim();
    let word = |key: &str| t!(key).to_string();
    if taste.latch == TasteLatch::Off {
        return (word("gui.discover.state_off"), grey, Vec::new());
    }
    let (pending, failed) = match tab {
        TasteTab::Library => (taste.library.pending, taste.library.failed.is_some()),
        TasteTab::Servers => (taste.peers.pending, taste.peers.failed.is_some()),
    };
    if pending || taste.latch == TasteLatch::Probing {
        return (word("gui.discover.asking"), gold, Vec::new());
    }
    if taste.scan_pending() {
        return (word("gui.discover.state_pending"), gold, Vec::new());
    }
    if failed {
        return (word("gui.discover.state_failed"), gold, Vec::new());
    }
    if taste.no_history {
        return (word("gui.discover.no_seeds"), grey, vec![word("gui.discover.no_seeds_fact")]);
    }
    let mut facts: Vec<String> = widened(app).into_iter().collect();
    if lines(app, tab).is_empty() {
        return (word("gui.discover.state_empty"), grey, facts);
    }
    // Only a tab holding rows streams anything (clause 41).
    if tab == TasteTab::Servers {
        facts.insert(0, word("gui.discover.through_server"));
    }
    (word("gui.discover.state_ready"), ok, facts)
}

// ── Pure geometry ───────────────────────────────────────────────────────────

/// Where the room's parts stand in its content rect (clause 90).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub header_y: u16,
    pub state_y: u16,
    /// The fallback card, and whether it gave up its body and blank.
    pub card: Option<(Rect, bool)>,
    pub tabs_y: u16,
    pub seeds_y: u16,
    /// The table's header row; its rule is the next, its rows after that.
    pub table_y: u16,
    pub bottom_y: u16,
}

pub(crate) fn layout(content: Rect, card: bool) -> Layout {
    let y = content.y;
    let bottom_y = content.bottom().saturating_sub(1);
    let mut tabs_y = y + 3;
    let mut card_at = None;
    if card {
        // Header, state, blank · the card and its blank · tabs, seeds,
        // blank · the table's header, rule and one row — and still room
        // for the bottom row under them.
        let whole = 3 + CARD_FULL + 1 + 3 + 3 < content.height;
        let height = if whole { CARD_FULL } else { CARD_COMPACT };
        let width = content.width.min(CARD_W);
        let x = content.x + (content.width - width) / 2;
        card_at = Some((Rect { x, y: y + 3, width, height }, !whole));
        tabs_y = y + 3 + height + 1;
    }
    Layout {
        header_y: y,
        state_y: y + 1,
        card: card_at,
        tabs_y,
        seeds_y: tabs_y + 1,
        table_y: tabs_y + 3,
        bottom_y,
    }
}

/// What the table's optional columns would like, in cells — the widest of
/// their words and their heading. `on` is the Connected servers tab's ON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Wants {
    pub artist: u16,
    pub album: u16,
    pub on: Option<u16>,
}

/// The table's columns in `list` (clause 50): TRACK from the left after the
/// playing mark, then ARTIST, ALBUM and (Connected servers) ON, and LENGTH
/// flush right. When TRACK would get fewer than 24 cells ALBUM goes first,
/// then ON, then ARTIST (clause 54); LENGTH never goes. Widths are cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cols {
    pub track_x: u16,
    pub track_w: u16,
    pub artist: Option<(u16, u16)>,
    pub album: Option<(u16, u16)>,
    pub on: Option<(u16, u16)>,
    pub len_x: u16,
    pub len_w: u16,
}

pub(crate) fn columns(list: Rect, wants: Wants, len_w: u16) -> Cols {
    let track_x = list.x + LEAD;
    let len_x = list.right().saturating_sub(len_w);
    // TRACK and the optional columns share what LENGTH and its gap leave.
    let room = len_x.saturating_sub(GAP + track_x);
    let on = wants.on.map(|w| w.min(ON_MAX));
    // The sets tried, widest first: ALBUM drops first, then ON, then ARTIST.
    let tries: [(bool, bool, bool); 4] = [(true, true, true), (true, false, true), (true, false, false), (false, false, false)];
    for (artist, album, with_on) in tries {
        let on_w = if with_on { on } else { None };
        let flex: Vec<u16> = [artist.then_some(wants.artist), album.then_some(wants.album)].into_iter().flatten().collect();
        let gaps = GAP * (flex.len() as u16 + u16::from(on_w.is_some()));
        let Some(avail) = room.checked_sub(gaps + on_w.unwrap_or(0)) else { continue };
        let Some(mut spare) = avail.checked_sub(TRACK_MIN) else { continue };
        // TRACK keeps half with two flexible columns beside it, two thirds
        // with one; each of them takes its share whatever its words, so the
        // columns stand where the eye expects them and a hovered row's verbs
        // cover ALBUM, not ARTIST.
        let share = avail / (flex.len() as u16 + 2);
        let mut widths = Vec::new();
        for want in &flex {
            let w = share.min(spare);
            if w < FLEX_MIN.min(*want) {
                break;
            }
            spare -= w;
            widths.push(w);
        }
        if widths.len() != flex.len() {
            continue;
        }
        let track_w = avail - widths.iter().sum::<u16>();
        let mut x = track_x + track_w + GAP;
        let mut place = |w: u16| {
            let at = (x, w);
            x += w + GAP;
            at
        };
        let mut widths = widths.into_iter();
        let artist_at = artist.then(|| widths.next().map(&mut place)).flatten();
        let album_at = album.then(|| widths.next().map(&mut place)).flatten();
        let on_at = on_w.map(&mut place);
        return Cols { track_x, track_w, artist: artist_at, album: album_at, on: on_at, len_x, len_w };
    }
    Cols { track_x, track_w: room, artist: None, album: None, on: None, len_x, len_w }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The room.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        put(frame, content.x, content.y, &t!("gui.nav.discover"), bold());
        super::servers::draw_disconnected(frame, gui, content, 2);
        return;
    }
    // A remembered tab whose source is gone falls back to Library (clause
    // 40); the memory itself is the config's, untouched.
    if !visible_tabs(&gui.app).contains(&gui.app.taste.tab) {
        gui.app.taste.tab = TasteTab::Library;
        gui.discover.stow();
    }
    let lay = layout(content, gui.app.taste.no_history);
    draw_header(frame, gui, content, lay.header_y);
    draw_state(frame, gui, content, lay.state_y);
    if let Some((rect, compact)) = lay.card {
        draw_card(frame, gui, rect, compact);
    }
    draw_tabs(frame, gui, content, lay.tabs_y);
    draw_seeds_line(frame, gui, content, lay.seeds_y);
    draw_table(frame, gui, content, lay.table_y, lay.bottom_y);
    draw_bottom(frame, gui, content, lay.bottom_y);
}

/// A refused pick, worded once (clause 14).
fn settle(gui: &mut Gui) {
    if let Some(refusal) = gui.app.taste.refused.take() {
        word_refusal(gui, refusal);
    }
}

fn word_refusal(gui: &mut Gui, refusal: TastePinRefusal) {
    match refusal {
        TastePinRefusal::Full => gui.note = Some((t!("gui.discover.pins_full").to_string(), true)),
        TastePinRefusal::OtherServer(server) => {
            gui.note = Some((t!("gui.discover.pick_other_server", server = server).to_string(), true));
        }
        // Already a seed or a pin: nothing changed, nothing to say.
        TastePinRefusal::AlreadyThere => {}
    }
}

/// The header: the room's name BOLD, what the answer is based on DIM, the
/// window dropdown at the right (clauses 11, 15) — the value in the row's
/// own weight, the chevron dim, both BRIGHT + BOLD under the pointer (the
/// kit's Dropdown). The phrase clips before the control moves (clause 91).
fn draw_header(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let name = t!("gui.nav.discover").to_string();
    put(frame, content.x, y, &name, Style::default().add_modifier(Modifier::BOLD));
    let window = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
    let label = window_label(window);
    let w = width(&label) as u16;
    let rect = Rect { x: content.right().saturating_sub(w + 2), y, width: w + 2, height: 1 };
    let hover = gui.ui.hovers(rect);
    put(frame, rect.x, y, &label, if hover { bright_bold() } else { Style::default() });
    put(frame, rect.x + w + 1, y, chevron(), if hover { bright_bold() } else { dim() });
    gui.ui.click(rect, Act::Disc(DiscAct::Window));
    gui.ui.tip_keyed(rect, t!("gui.discover.window_tip").to_string());
    gui.discover.window_at = rect;
    if let Some(phrase) = based_on(&gui.app) {
        let x = content.x + width(&name) as u16 + 1;
        let room = rect.x.saturating_sub(x + 2) as usize;
        put(frame, x, y, &super::bar::clip(&format!("· {phrase}"), room), dim());
    }
}

/// The kit's state line for the tab in front (clause 41).
fn draw_state(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let (state, style, facts) = state_line(&gui.app, gui.app.taste.tab);
    let head = format!("{} {state}", bullet_glyph());
    put(frame, content.x, y, &head, style.add_modifier(Modifier::BOLD));
    let mut x = content.x + width(&head) as u16;
    for fact in facts {
        let piece = format!(" · {fact}");
        let w = width(&piece) as u16;
        if x + w > content.right() {
            break;
        }
        put(frame, x, y, &piece, dim());
        x += w;
    }
}

/// The fallback card (clause 20): what the room needs, the random pick it
/// started from, another pick and choose a song.
fn draw_card(frame: &mut Frame, gui: &mut Gui, rect: Rect, compact: bool) {
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(dim());
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let x = inner.x + 2;
    let room = inner.width.saturating_sub(4) as usize;
    let mut y = inner.y;
    // The BOLD lead sentence is the card's point (clause 20): with the
    // queue open the card is 47 cells and no locale's title fits one line,
    // so it wraps into the body's first row, and the body gives that row
    // up. Compact, it has one row and clips. The card keeps its height.
    let title = t!("gui.discover.card_title");
    let title_lines = if compact { Vec::new() } else { crate::kit::wrap_words(&title, room) };
    let title_rows = if compact { 1 } else { title_lines.len().clamp(1, 2) };
    if title_rows == 1 {
        put(frame, x, y, &super::bar::clip(&title, room), bold());
    } else {
        put(frame, x, y, &super::bar::clip(&title_lines[0], room), bold());
        let rest = title_lines[1..].join(" ");
        put(frame, x, y + 1, &super::bar::clip(&rest, room), bold());
    }
    y += title_rows as u16;
    if !compact {
        // The body sentence takes the blank under it when it wraps; what
        // its rows cannot hold ends in the clip's mark, not mid-word.
        let rows = 3 - title_rows;
        let body = crate::kit::wrap_words(&t!("gui.discover.card_body"), room);
        for i in 0..rows.min(body.len()) {
            let line = if i + 1 == rows && body.len() > rows { body[i..].join(" ") } else { body[i].clone() };
            put(frame, x, y + i as u16, &super::bar::clip(&line, room), dim());
        }
        y += rows as u16;
    }
    // Around your pick once a pin replaced the server's random seed.
    let taste = &gui.app.taste;
    let around = match taste.pins.last() {
        Some(pin) => Some(t!("gui.discover.card_around_pick", track = track_words(&pin.track)).to_string()),
        None => taste
            .head
            .as_ref()
            .and_then(|head| head.seeds.first())
            .map(|seed| t!("gui.discover.card_around_random", track = track_words(&seed.to_track())))
            .map(|words| words.to_string()),
    };
    if let Some(around) = around {
        put(frame, x, y, &super::bar::clip(&around, room), Style::default());
    }
    y += 1;
    let another = t!("gui.discover.another").to_string();
    let w = super::text_button(frame, gui, x, y, &another, true, Act::Disc(DiscAct::Another));
    gui.ui.tip_keyed(Rect { x, y, width: w, height: 1 }, t!("gui.discover.another_tip").to_string());
    let choose = t!("gui.discover.choose").to_string();
    super::text_button(frame, gui, x + w + 3, y, &choose, false, Act::Disc(DiscAct::Choose));
}

/// The kit's Tabs (clause 40): the active one a slab, the rest text
/// buttons — names only, never a count.
fn draw_tabs(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let mut x = content.x;
    for tab in visible_tabs(&gui.app) {
        let name = match tab {
            TasteTab::Library => t!("gui.discover.tab_library"),
            TasteTab::Servers => t!("gui.discover.tab_servers"),
        };
        let label = format!(" {name} ");
        let w = (width(&label) as u16).min(content.right().saturating_sub(x));
        let rect = Rect { x, y, width: w, height: 1 };
        let hover = gui.ui.hovers(rect);
        let style = if tab == gui.app.taste.tab {
            Style::default().fg(th().on_accent).bg(th().accent).add_modifier(Modifier::BOLD)
        } else if hover {
            bright_bold()
        } else {
            dim()
        };
        put(frame, x, y, &super::bar::clip(&label, w as usize), style);
        gui.ui.click(rect, Act::Disc(DiscAct::Tab(tab)));
        x = rect.right() + TAB_GAP;
    }
}

/// The seeds line (clause 12): what the answer starts from, pins first,
/// "and more" for the rest — never a count — then `edit`. It never wraps
/// and never scrolls.
fn draw_seeds_line(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let label = t!("gui.discover.seeds").to_string();
    put(frame, content.x, y, &label, dim());
    // (words, marker) in the order the line shows them.
    let taste = &gui.app.taste;
    let random = taste.head.as_ref().is_some_and(|h| h.seed_source == SeedSource::Random);
    let mut seeds: Vec<(String, Option<String>)> = Vec::new();
    match &taste.head {
        Some(head) => {
            let (pinned, rest): (Vec<_>, Vec<_>) = head.seeds.iter().partition(|seed| seed.pinned);
            for seed in pinned {
                seeds.push((track_words(&seed.to_track()), Some(t!("gui.discover.seed_pinned").to_string())));
            }
            for seed in rest {
                let marker = random.then(|| t!("gui.discover.seed_random").to_string());
                seeds.push((track_words(&seed.to_track()), marker));
            }
        }
        None => {
            for pin in &taste.pins {
                seeds.push((track_words(&pin.track), Some(t!("gui.discover.seed_pinned").to_string())));
            }
        }
    }
    let edit = t!("gui.discover.seeds_edit").to_string();
    let edit_w = width(&edit) as u16;
    let more = t!("gui.discover.seeds_more").to_string();
    let more_w = width(&more) as u16 + 2;
    let mut x = content.x + width(&label) as u16 + 2;
    let total = seeds.len();
    for (i, (words, marker)) in seeds.iter().enumerate() {
        let piece_w = width(words) as u16 + marker.as_ref().map_or(0, |m| width(m) as u16 + 1);
        // What must still fit after this seed: "and more" while seeds are
        // left after it, and `edit`.
        let tail = if i + 1 < total { more_w } else { 0 } + edit_w + 2;
        if x + piece_w + tail > content.right() {
            put(frame, x, y, &more, dim());
            x += more_w;
            break;
        }
        put(frame, x, y, words, Style::default());
        x += width(words) as u16;
        if let Some(marker) = marker {
            put(frame, x + 1, y, marker, dim());
            x += width(marker) as u16 + 1;
        }
        x += 2;
    }
    if x + edit_w <= content.right() {
        let w = super::text_button(frame, gui, x, y, &edit, false, Act::Disc(DiscAct::Seeds));
        gui.ui.tip_keyed(Rect { x, y, width: w, height: 1 }, t!("gui.discover.seeds_tip").to_string());
    }
}

/// One row's words, owned so the drawing can borrow the surface freely.
/// No match, no percentage, no last-played: what the row is, by whom, from
/// which album, and how long (the 2026-10-11 log).
struct Cells {
    title: String,
    /// DIM beside the title: an album row's `album`.
    tag: Option<String>,
    artist: String,
    album: String,
    /// ON: the connected server the row plays from.
    on: String,
    /// The length; the kit's DIM `—` for a missing value, empty on an album.
    length: String,
    playing: bool,
    tip: Option<String>,
    verbs: Verbs,
}

/// The hover verbs a row carries (clauses 60–62).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verbs {
    /// An album: `[+]`, its play control, queues it (clause 61).
    Album,
    /// A library song: `[▸] [»] [+] [⋯]`.
    Song,
    /// A connected server's song: `[▸] [»] [+] [⋯]` — `[▸]` through your
    /// server, `[⋯]` the sheet with the peer's origin (clause 62).
    Peer,
}

fn sounds_like(app: &App, hash: Option<&str>) -> Option<String> {
    let seed = app.taste.seed(hash?)?;
    Some(t!("gui.discover.seed_tip", track = track_words(&seed.to_track())).to_string())
}

fn cells(app: &App, line: Line) -> Option<Cells> {
    let taste = &app.taste;
    // The playing track by its path AND its source (clause 55): a connected
    // server's "Music/x.mp3" is not the library's song of that path. The
    // queue's current row says which peer, if any, it streams from.
    let playing_peer = app.queue.current.and_then(|i| app.queue.items.get(i)).map(|q| q.origin.peer);
    let playing = |path: &str, peer: Option<i64>| {
        app.now_playing.as_ref().is_some_and(|t| t.filepath == path) && playing_peer.is_none_or(|p| p == peer)
    };
    let time = |seconds: Option<f64>| seconds.filter(|s| *s > 0.0).map(super::bar::fmt_time);
    let or_dash = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(dash()).to_string();
    match line {
        Line::Album(i) => {
            let album = taste.library.data.as_ref()?.albums.get(i)?;
            Some(Cells {
                title: or_dash(album.name.as_deref()),
                tag: Some(t!("gui.discover.album").to_string()),
                artist: or_dash(album.artist()),
                album: String::new(),
                on: String::new(),
                length: String::new(),
                playing: false,
                tip: Some(t!("gui.discover.album_tip").to_string()),
                verbs: Verbs::Album,
            })
        }
        Line::Song(i) => {
            let song = taste.library.data.as_ref()?.tracks.get(i)?;
            let track = song.to_track();
            Some(Cells {
                title: track.title_or_file().to_string(),
                tag: None,
                artist: or_dash(track.metadata.artist.as_deref()),
                album: or_dash(track.metadata.album.as_deref()),
                on: String::new(),
                length: time(track.metadata.duration).unwrap_or_else(|| dash().to_string()),
                playing: playing(&song.filepath, None),
                tip: sounds_like(app, song.seed_hash.as_deref()),
                verbs: Verbs::Song,
            })
        }
        Line::Peer { group, row } => {
            let (peer, lead) = taste.peer_row(group, row)?;
            Some(Cells {
                title: or_dash(lead.title.as_deref()),
                tag: None,
                artist: or_dash(lead.artist.as_deref()),
                album: or_dash(lead.album.as_deref()),
                on: taste.peer_name(group).unwrap_or_default().to_string(),
                length: time(lead.duration).unwrap_or_else(|| dash().to_string()),
                playing: lead.filepath.as_deref().is_some_and(|path| playing(path, Some(peer))),
                tip: sounds_like(app, lead.seed_hash.as_deref()),
                verbs: Verbs::Peer,
            })
        }
    }
}

/// What stands in the pane instead of rows (clauses 22, 80–83), if
/// anything; `failed_over` says the failure line sits over old rows.
enum Pane {
    Rows { failed_over: Option<String> },
    Off,
    ScanPending,
    /// The first ask: skeleton rows, no words (clause 32).
    Loading,
    Failed(String),
    Sentence(String),
    /// The library answered nothing new: the sentence, and — while the
    /// checkbox is unticked — a button that ticks it (clause 80).
    LibraryEmpty,
}

fn pane(app: &App, tab: TasteTab, empty: bool) -> Pane {
    let taste = &app.taste;
    if taste.latch == TasteLatch::Off {
        return Pane::Off;
    }
    if taste.scan_pending() {
        return Pane::ScanPending;
    }
    let (first_wait, failed, not_asked, has_data) = match tab {
        TasteTab::Library => slot_facts(&taste.library),
        TasteTab::Servers => slot_facts(&taste.peers),
    };
    if first_wait || (taste.latch == TasteLatch::Probing && !has_data) {
        return Pane::Loading;
    }
    if let Some(err) = failed.clone().filter(|_| !has_data) {
        return Pane::Failed(err);
    }
    if not_asked {
        return Pane::Sentence(t!("gui.discover.not_asked").to_string());
    }
    if !has_data {
        return Pane::Rows { failed_over: None };
    }
    if empty {
        return match tab {
            TasteTab::Library => Pane::LibraryEmpty,
            TasteTab::Servers => Pane::Sentence(t!("gui.discover.empty_servers").to_string()),
        };
    }
    Pane::Rows { failed_over: failed }
}

fn slot_facts<T>(slot: &TasteSlot<T>) -> (bool, Option<String>, bool, bool) {
    (slot.first_wait(), slot.failed.clone(), slot.not_asked_random, slot.data.is_some())
}

/// The first ask's skeleton rows (clause 32, the webapp's four): DIM runs
/// where TRACK, ARTIST, ALBUM, ON and LENGTH will stand, of varied lengths
/// so they read as rows to come, never as a bar filling up.
fn draw_skeleton(frame: &mut Frame, list: Rect, cols: Cols) {
    let glyph = if legacy_conhost() { "·" } else { "░" };
    let run = |w: u16, share: u16| glyph.repeat(usize::from((w * share / 10).max(1)));
    const SHARES: [(u16, u16); 4] = [(6, 7), (4, 5), (7, 6), (5, 8)];
    for (row, (track, rest)) in SHARES.into_iter().enumerate().take(list.height as usize) {
        let y = list.y + row as u16;
        put(frame, cols.track_x, y, &run(cols.track_w.min(32), track), dim());
        for (x, w) in [cols.artist, cols.album, cols.on].into_iter().flatten() {
            put(frame, x, y, &run(w, rest), dim());
        }
        put(frame, cols.len_x + cols.len_w.saturating_sub(4), y, &glyph.repeat(4), dim());
    }
}

/// A DIM sentence wrapped into the pane; the rows it took.
fn sentence_lines(frame: &mut Frame, list: Rect, sentence: &str, style: Style) -> u16 {
    let lines = crate::kit::wrap_words(sentence, list.width as usize);
    let shown = lines.len().min(list.height as usize);
    for (i, line) in lines.into_iter().take(shown).enumerate() {
        put(frame, list.x, list.y + i as u16, &line, style);
    }
    shown as u16
}

/// The tab's table: header and rule, then the rows or what stands for them,
/// and the kit scrollbar while the rows overflow.
fn draw_table(frame: &mut Frame, gui: &mut Gui, content: Rect, table_y: u16, bottom_y: u16) {
    let tab = gui.app.taste.tab;
    let list = Rect {
        x: content.x,
        y: table_y + 2,
        width: content.width.saturating_sub(2),
        height: bottom_y.saturating_sub(table_y + 2),
    };
    let all = lines(&gui.app, tab);
    let row_cells: Vec<Option<Cells>> = all.iter().map(|line| cells(&gui.app, *line)).collect();

    let head = |key: &str| t!(key).to_uppercase();
    let (track_head, artist_head, album_head, on_head, len_head) = (
        head("gui.discover.col_track"),
        head("gui.discover.col_artist"),
        head("gui.discover.col_album"),
        head("gui.discover.col_on"),
        head("gui.discover.col_length"),
    );
    let widest = |pick: fn(&Cells) -> &str, floor: &str| {
        row_cells.iter().flatten().map(|c| width(pick(c))).chain([width(floor)]).max().unwrap_or(0) as u16
    };
    let wants = Wants {
        artist: widest(|c| c.artist.as_str(), &artist_head),
        album: widest(|c| c.album.as_str(), &album_head),
        on: (tab == TasteTab::Servers).then(|| widest(|c| c.on.as_str(), &on_head)),
    };
    let len_w = widest(|c| c.length.as_str(), &len_head).min(10);
    let cols = columns(list, wants, len_w);
    let clipped = |text: &str, w: u16| super::bar::clip(text, w as usize).into_owned();
    put(frame, cols.track_x, table_y, &clipped(&track_head, cols.track_w), dim());
    for (at, words) in [(cols.artist, &artist_head), (cols.album, &album_head), (cols.on, &on_head)] {
        if let Some((x, w)) = at {
            put(frame, x, table_y, &clipped(words, w), dim());
        }
    }
    put(frame, cols.len_x + len_w.saturating_sub(width(&len_head) as u16), table_y, &len_head, dim());
    put(frame, list.x, table_y + 1, &"─".repeat(content.width as usize), dim());

    let mut list = list;
    match pane(&gui.app, tab, all.is_empty()) {
        Pane::Off => {
            sentence_lines(frame, list, &t!("gui.discover.turned_off"), Style::default().fg(th().gold));
            return;
        }
        Pane::ScanPending => {
            let short = Rect { height: list.height.saturating_sub(1), ..list };
            let shown = sentence_lines(frame, short, &t!("gui.discover.scan_pending"), dim());
            let label = t!("gui.discover.check_again").to_string();
            again_button(frame, gui, (list.x, list.y + shown), &label, DiscAct::Again);
            return;
        }
        Pane::Loading => {
            draw_skeleton(frame, list, cols);
            return;
        }
        Pane::Failed(err) => {
            failure_line(frame, gui, list, &err);
            return;
        }
        Pane::Sentence(sentence) => {
            sentence_lines(frame, list, &sentence, dim());
            return;
        }
        Pane::LibraryEmpty => {
            let short = Rect { height: list.height.saturating_sub(1), ..list };
            let shown = sentence_lines(frame, short, &t!("gui.discover.empty_library"), dim());
            // The way out is the checkbox's own words, ticking it (clause 80).
            if !gui.app.taste.includes_played() {
                let label = t!("gui.discover.include_played").to_string();
                again_button(frame, gui, (list.x, list.y + shown), &label, DiscAct::Played);
            }
            return;
        }
        Pane::Rows { failed_over } => {
            if let Some(err) = failed_over {
                failure_line(frame, gui, Rect { height: 1, ..list }, &err);
                list = Rect { y: list.y + 1, height: list.height.saturating_sub(1), ..list };
            }
        }
    }

    let len = all.len();
    // A new answer can leave the cursor past the rows: the hand goes down
    // rather than lighting nothing in particular.
    if gui.discover.cursor.is_some_and(|c| c >= len) {
        gui.discover.stow();
    }
    let shown = gui.discover.view.shown(gui.discover.cursor);
    let (first, visible) = gui.discover.view.window(len, gui.discover.cursor, list.height as usize);
    for (row, index) in (first..(first + visible).min(len)).enumerate() {
        let rect = Rect { x: list.x, y: list.y + row as u16, width: list.width, height: 1 };
        if let Some(cells) = &row_cells[index] {
            draw_row(frame, gui, rect, cols, (index, all[index]), cells, shown == Some(index));
        }
    }
    scroll_list(
        frame,
        &mut gui.ui,
        Rect { x: content.right().saturating_sub(1), y: list.y, width: 1, height: list.height },
        len,
        visible,
        first,
        Act::ScrollBy(List::Discover, -1),
        Act::ScrollBy(List::Discover, 1),
        |first| Act::ScrollTo(List::Discover, first),
    );
}

/// Try again, Check again and the empty library's way out: the accent's text
/// button, the room's one way forward where it stands.
fn again_button(frame: &mut Frame, gui: &mut Gui, (x, y): (u16, u16), label: &str, act: DiscAct) {
    let rect = Rect { x, y, width: width(label) as u16, height: 1 };
    let hover = gui.ui.hovers(rect);
    put(frame, x, y, label, if hover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) });
    gui.ui.click(rect, Act::Disc(act));
}

/// "couldn't reach this source" in the warning colour, with Try again at
/// its right (clause 83) — the server's words only under the pointer: the
/// page shows no status codes.
fn failure_line(frame: &mut Frame, gui: &mut Gui, at: Rect, err: &str) {
    let retry = t!("gui.discover.retry").to_string();
    let retry_w = width(&retry) as u16;
    let words = t!("gui.discover.failed").to_string();
    let room = at.width.saturating_sub(retry_w + 2) as usize;
    let shown = super::bar::clip(&words, room).into_owned();
    put(frame, at.x, at.y, &shown, Style::default().fg(th().gold));
    gui.ui.tip(Rect { x: at.x, y: at.y, width: (width(&shown) as u16).max(1), height: 1 }, err.to_string());
    again_button(frame, gui, (at.x + width(&shown) as u16 + 2, at.y), &retry, DiscAct::Again);
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    frame: &mut Frame,
    gui: &mut Gui,
    rect: Rect,
    cols: Cols,
    (index, line): (usize, Line),
    cells: &Cells,
    is_sel: bool,
) {
    let y = rect.y;
    let hover = gui.ui.hovers(rect);
    if is_sel {
        frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
    }
    let title_style = match (is_sel, cells.playing, hover) {
        (true, _, _) => sel().add_modifier(Modifier::BOLD),
        (false, true, _) => Style::default().fg(th().ok).add_modifier(Modifier::BOLD),
        (false, false, true) => bright_bold(),
        (false, false, false) => Style::default(),
    };
    // The hover verbs stand at the row's right, over the columns they would
    // cover (the Files rows' grammar); a selected row keeps its columns —
    // the keyboard's hand has its own keys.
    let [now, next, queue, more] = verb_glyphs();
    let tip = |key: &str| t!(key).to_string();
    let verbs: Vec<(&str, DiscAct, String)> = match cells.verbs {
        _ if !hover || is_sel => Vec::new(),
        Verbs::Album => vec![(queue, DiscAct::Row(index, RowVerb::Queue), tip("gui.discover.album_queue_tip"))],
        Verbs::Song => vec![
            (now, DiscAct::Row(index, RowVerb::Now), tip("gui.files.now_tip")),
            (next, DiscAct::Row(index, RowVerb::Next), tip("gui.files.next_tip")),
            (queue, DiscAct::Row(index, RowVerb::Queue), tip("gui.files.queue_tip")),
            (more, DiscAct::More(index), tip("gui.act.more_tip")),
        ],
        Verbs::Peer => vec![
            (now, DiscAct::Row(index, RowVerb::Now), tip("gui.discover.listen_tip")),
            (next, DiscAct::Row(index, RowVerb::Next), tip("gui.files.next_tip")),
            (queue, DiscAct::Row(index, RowVerb::Queue), tip("gui.files.queue_tip")),
            (more, DiscAct::More(index), tip("gui.act.more_tip")),
        ],
    };
    let verbs_w = (verbs.len() as u16 * 4).saturating_sub(1);
    // Where the verbs begin; a column reaching past it is not drawn.
    let verbs_x = if verbs.is_empty() { rect.right() } else { rect.right().saturating_sub(verbs_w) };
    let fits = |x: u16, w: u16| x + w < verbs_x;
    let track_w = cols.track_w.min(verbs_x.saturating_sub(GAP + cols.track_x));
    let tag_w = cells.tag.as_ref().map_or(0, |t| width(t) as u16 + 1);
    let title = super::bar::clip(&cells.title, track_w.saturating_sub(tag_w) as usize).into_owned();
    put(frame, cols.track_x, y, &title, title_style);
    // The playing row wears the house `▸` as in every room (clause 55; the
    // Files rows' marker), in the lead before its title.
    if cells.playing {
        let style = if is_sel { sel() } else { Style::default().fg(th().ok) };
        put(frame, cols.track_x.saturating_sub(LEAD), y, super::forward_glyph(), style.add_modifier(Modifier::BOLD));
    }
    let title_w = width(&title) as u16;
    let detail = if is_sel { sel() } else { dim() };
    if let Some(tag) = &cells.tag {
        put(frame, cols.track_x + title_w + 1, y, tag, detail);
    }
    for (at, words) in [(cols.artist, &cells.artist), (cols.album, &cells.album), (cols.on, &cells.on)] {
        if let Some((x, w)) = at
            && fits(x, w)
        {
            put(frame, x, y, &super::bar::clip(words, w as usize), detail);
        }
    }
    if fits(cols.len_x, cols.len_w) || verbs.is_empty() {
        let len = super::bar::clip(&cells.length, cols.len_w as usize).into_owned();
        put(frame, cols.len_x + cols.len_w.saturating_sub(width(&len) as u16), y, &len, detail);
    }
    // The row's click first, its verbs after: the last registered wins.
    gui.ui.click(rect, Act::Disc(DiscAct::Row(index, RowVerb::Open)));
    if matches!(line, Line::Song(_) | Line::Peer { .. }) {
        gui.ui.context(rect, Act::Disc(DiscAct::More(index)));
    }
    if let Some(tip) = &cells.tip {
        let target = Rect { x: cols.track_x, y, width: title_w.max(1), height: 1 };
        if matches!(line, Line::Album(_)) {
            gui.ui.tip_keyed(target, tip.clone());
        } else {
            gui.ui.tip(target, tip.clone());
        }
    }
    let mut x = verbs_x;
    for (glyph, act, tip) in verbs {
        let cell = Rect { x, y, width: 3, height: 1 };
        put(frame, x, y, glyph, if gui.ui.hovers(cell) { bright_bold() } else { dim() });
        gui.ui.click(cell, Act::Disc(act));
        gui.ui.tip_keyed(cell, tip);
        x += 4;
    }
}

/// The bottom row (clauses 64–65): on the Library tab the one checkbox at
/// the left — "Include songs I've played lately" — and the whole-list verbs
/// at the right on a tab that has something to play, dropped from the tail
/// when squeezed; the checkbox never moves (clause 91).
fn draw_bottom(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let tab = gui.app.taste.tab;
    let mut left = content.x;
    if tab == TasteTab::Library {
        let (on_glyph, off_glyph) = check_glyphs();
        let on = gui.app.taste.includes_played();
        let label = t!("gui.discover.include_played").to_string();
        let room = content.width.saturating_sub(4) as usize;
        let label = super::bar::clip(&label, room).into_owned();
        let rect = Rect { x: content.x, y, width: 4 + width(&label) as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        let glyph_style = match (hover, on) {
            (true, _) => bright_bold(),
            (false, true) => Style::default().fg(th().ok),
            (false, false) => dim(),
        };
        put(frame, rect.x, y, if on { on_glyph } else { off_glyph }, glyph_style);
        put(frame, rect.x + 4, y, &label, if hover { bright_bold() } else { Style::default() });
        gui.ui.click(rect, Act::Disc(DiscAct::Played));
        gui.ui.tip_keyed(rect, t!("gui.discover.include_tip").to_string());
        left = rect.right() + 2;
    }
    if !playable(&gui.app, tab) {
        return;
    }
    let forward = super::forward_glyph();
    let shuffle = if legacy_conhost() { "" } else { "⇄ " };
    let mut verbs: Vec<(String, DiscAct, &str)> = vec![
        (format!("{forward} {}", t!("gui.bar.play")), DiscAct::PlayAll, "p"),
        (format!("+ {}", t!("gui.bar.queue")), DiscAct::QueueAll, "A"),
        (format!("{shuffle}{}", t!("gui.shuffle_word")), DiscAct::Shuffle, "S"),
    ];
    let sep = " · ";
    let width_of = |verbs: &[(String, DiscAct, &str)]| -> u16 {
        let labels: u16 = verbs.iter().map(|(l, _, _)| width(l) as u16).sum();
        labels + width(sep) as u16 * verbs.len().saturating_sub(1) as u16
    };
    while !verbs.is_empty() && content.right().saturating_sub(left) < width_of(&verbs) + 1 {
        verbs.pop();
    }
    let mut x = content.right().saturating_sub(width_of(&verbs) + 1);
    for (i, (label, act, key)) in verbs.into_iter().enumerate() {
        if i > 0 {
            put(frame, x, y, sep, dim());
            x += width(sep) as u16;
        }
        let rect = Rect { x, y, width: width(&label) as u16, height: 1 };
        let hover = gui.ui.hovers(rect);
        let style = match (hover, act == DiscAct::PlayAll) {
            (true, _) => bright_bold(),
            (false, true) => accent(),
            (false, false) => dim(),
        };
        put(frame, x, y, &label, style);
        gui.ui.click(rect, Act::Disc(act));
        gui.ui.tip_keyed(rect, format!("{label} — {key}"));
        x += rect.width;
    }
}

// ── The window's list and the modal ─────────────────────────────────────────

/// The seeds modal's lines (clause 13).
#[derive(Debug, Clone, PartialEq)]
enum SeedLine {
    Head(String),
    Pin(usize),
    Seed(String),
    Dropped(String),
    PinPlaying,
    PinSong,
}

impl SeedLine {
    fn is_row(&self) -> bool {
        !matches!(self, SeedLine::Head(_))
    }
}

fn seed_lines(app: &App) -> Vec<SeedLine> {
    let taste = &app.taste;
    let mut out = Vec::new();
    if !taste.pins.is_empty() {
        out.push(SeedLine::Head(t!("gui.discover.sec_pinned").to_uppercase()));
        out.extend((0..taste.pins.len()).map(SeedLine::Pin));
    }
    if let Some(head) = &taste.head {
        let history: Vec<_> = if head.seed_source == SeedSource::Random {
            Vec::new()
        } else {
            head.seeds.iter().filter(|s| !s.pinned).map(|s| SeedLine::Seed(s.hash.clone())).collect()
        };
        if !history.is_empty() {
            out.push(SeedLine::Head(t!("gui.discover.sec_seeds").to_uppercase()));
            out.extend(history);
        }
        if !head.excluded.is_empty() {
            out.push(SeedLine::Head(t!("gui.discover.sec_dropped").to_uppercase()));
            out.extend(head.excluded.iter().map(|e| SeedLine::Dropped(e.hash.clone())));
        }
    }
    // Hidden, never grey, when it cannot be done (clause 14).
    if app.taste_can_pin_playing() {
        out.push(SeedLine::PinPlaying);
    }
    out.push(SeedLine::PinSong);
    out
}

/// The window's list and the seeds modal, drawn (and registered) after the
/// base screen so their rects win the pointer.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // Every frame, whatever is up: a pick refused from the library is
    // worded on the way back.
    settle(gui);
    if gui.screen != Screen::Library || gui.active != super::DISCOVER_NAV {
        return;
    }
    if let Some(Menu::Window(cursor)) = gui.discover.menu {
        let current = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
        let rows: Vec<(String, bool, DiscAct)> =
            WINDOWS.iter().map(|w| (window_label(*w), *w == current, DiscAct::WindowPick(*w))).collect();
        let anchor = gui.discover.window_at;
        draw_list(frame, gui, (area, anchor), rows, cursor);
    }
    if gui.discover.seeds.is_some() {
        draw_seeds_modal(frame, gui, area);
    }
}

/// A dropdown's list (the kit's Dropdown): hung under its control, the
/// current value wearing `•`, the cursor the accent slab.
fn draw_list(
    frame: &mut Frame,
    gui: &mut Gui,
    (area, anchor): (Rect, Rect),
    rows: Vec<(String, bool, DiscAct)>,
    cursor: usize,
) {
    gui.ui.click(area, Act::Disc(DiscAct::MenuClose));
    let widest = rows.iter().map(|(label, _, _)| width(label)).max().unwrap_or(10) as u16;
    let w = (widest + 6).max(anchor.width + 4).min(area.width.saturating_sub(2));
    let h = rows.len() as u16 + 2;
    let x = anchor.x.saturating_sub(1).min(area.right().saturating_sub(w + 1));
    let rect = Rect { x, y: anchor.bottom(), width: w, height: h };
    gui.ui.overlay(rect);
    let inner = frame_at(frame, rect, th().accent);
    for (i, (label, current, act)) in rows.into_iter().enumerate() {
        let row = Rect { x: inner.x, y: inner.y + i as u16, width: inner.width, height: 1 };
        let on = i == cursor;
        let hover = !on && gui.ui.hovers(row);
        if on {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
        }
        let mark = if current { bullet_glyph() } else { " " };
        put(frame, row.x + 1, row.y, mark, if on { sel() } else { accent() });
        let style = match (on, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let label = super::bar::clip(&label, inner.width.saturating_sub(4) as usize).into_owned();
        put(frame, row.x + 3, row.y, &label, style);
        gui.ui.click(row, Act::Disc(act));
    }
}

/// The seeds modal (clause 13): "Based on", its sections, the pin rows and
/// Done.
fn draw_seeds_modal(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ui.click(area, Act::Guard);
    let rows = seed_lines(&gui.app);
    let w = 72.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 7).min(area.height.saturating_sub(2));
    let inner = modal_frame_on(frame, &mut gui.ui, area, w, h, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.discover.seeds_title"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, &mut gui.ui, inner, Act::Disc(DiscAct::SeedsDone));
    let sub = t!("gui.discover.seeds_sub").to_string();
    put(frame, inner.x + 1, inner.y + 1, &super::bar::clip(&sub, inner.width.saturating_sub(2) as usize), dim());
    let list = Rect { y: inner.y + 3, height: inner.height.saturating_sub(5), ..inner };
    let now = crate::admin::unix_now();
    let zone = gui.discover.zone.get_or_insert_with(crate::admin::tz::local);
    let offset = zone.as_ref().map_or(0, |z| z.offset_at(now));
    let Some(modal) = gui.discover.seeds.clone() else { return };
    let mut view = modal.view;
    let shown = view.shown(modal.cursor);
    let (first, visible) = view.window(rows.len(), modal.cursor, list.height as usize);
    let (check_on, check_off) = check_glyphs();
    for (row, index) in (first..(first + visible).min(rows.len())).enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        let line = &rows[index];
        if !line.is_row() {
            if let SeedLine::Head(text) = line {
                put(frame, rect.x + 1, y, text, dim());
            }
            continue;
        }
        let is_sel = shown == Some(index);
        let hover = !is_sel && gui.ui.hovers(rect);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover) {
            (true, _) => sel().add_modifier(Modifier::BOLD),
            (false, true) => bright_bold(),
            (false, false) => Style::default(),
        };
        let facts_style = if is_sel { sel() } else { dim() };
        let taste = &gui.app.taste;
        let room = rect.width.saturating_sub(2) as usize;
        let (check, words, facts): (Option<bool>, String, String) = match line {
            SeedLine::Pin(i) => {
                let pin = &taste.pins[*i];
                let marker = t!(if pin.random { "gui.discover.seed_random" } else { "gui.discover.seed_pinned" });
                (None, track_words(&pin.track), marker.to_string())
            }
            SeedLine::Seed(hash) => {
                let Some(seed) = taste.seed(hash) else { continue };
                let mut facts = Vec::new();
                if let Some(then) = seed.last_played.as_deref().and_then(crate::admin::iso_unix) {
                    facts.push(age_words(then, now, offset));
                }
                match seed.outcome {
                    TasteOutcome::Completed => facts.push(t!("gui.discover.outcome_completed").to_string()),
                    TasteOutcome::Stopped => facts.push(t!("gui.discover.outcome_stopped").to_string()),
                    TasteOutcome::Other => {}
                }
                if !seed.analyzed {
                    facts.push(t!("gui.discover.not_analyzed").to_string());
                }
                (Some(!modal.flipped.contains(hash)), track_words(&seed.to_track()), facts.join(" · "))
            }
            SeedLine::Dropped(hash) => {
                let found = taste.head.as_ref().and_then(|h| h.excluded.iter().find(|e| &e.hash == hash));
                let Some(excluded) = found else { continue };
                let track = Track { filepath: excluded.filepath.clone(), metadata: excluded.metadata.clone() };
                (Some(modal.flipped.contains(hash)), track_words(&track), String::new())
            }
            SeedLine::PinPlaying => (None, format!("+ {}", t!("gui.discover.pin_playing")), String::new()),
            SeedLine::PinSong => (None, format!("+ {}", t!("gui.discover.pin_song")), String::new()),
            SeedLine::Head(_) => continue,
        };
        let mut x = rect.x + 1;
        if let Some(on) = check {
            let (glyph, gstyle) = if on {
                (check_on, if is_sel { sel() } else { Style::default().fg(th().ok) })
            } else {
                (check_off, facts_style)
            };
            put(frame, x, y, glyph, gstyle);
            x += 4;
        }
        let unpin = matches!(line, SeedLine::Pin(_));
        let tail = if unpin { 5 } else { 1 };
        let avail = (rect.right().saturating_sub(x + tail)) as usize;
        let words = super::bar::clip(&words, avail.min(room)).into_owned();
        put(frame, x, y, &words, style);
        let after = x + width(&words) as u16 + 2;
        if !facts.is_empty() && after < rect.right().saturating_sub(tail) {
            let room = rect.right().saturating_sub(after + tail) as usize;
            put(frame, after, y, &super::bar::clip(&facts, room), facts_style);
        }
        gui.ui.click(rect, Act::Disc(DiscAct::SeedRow(index)));
        if let SeedLine::Pin(i) = line {
            let cell = Rect { x: rect.right().saturating_sub(4), y, width: 3, height: 1 };
            let style = if gui.ui.hovers(cell) {
                Style::default().fg(th().danger).add_modifier(Modifier::BOLD)
            } else {
                facts_style
            };
            put(frame, cell.x, y, "[X]", style);
            gui.ui.click(cell, Act::Disc(DiscAct::SeedUnpin(*i)));
            gui.ui.tip_keyed(cell, t!("gui.discover.unpin_tip").to_string());
        }
    }
    if let Some(modal) = gui.discover.seeds.as_mut() {
        modal.view = view;
    }
    // The modal primary, the safe choice: Done applies (clause 13).
    let done = t!("gui.discover.seeds_done").to_string();
    let done_w = width(&done) as u16;
    let rect = Rect {
        x: inner.right().saturating_sub(done_w + 1),
        y: inner.bottom().saturating_sub(1),
        width: done_w,
        height: 1,
    };
    let style = if gui.ui.hovers(rect) { bright_bold() } else { accent().add_modifier(Modifier::BOLD) };
    put(frame, rect.x, rect.y, &done, style);
    gui.ui.click(rect, Act::Disc(DiscAct::SeedsDone));
}

pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.discover.menu.is_some() || gui.discover.seeds.is_some()
}

/// The room is entered — a fresh visit reuses an answer under five minutes
/// old; `again` (the nav row or `G` while the room is up) asks now (clause
/// 3). Either way the room opens with its cursor stowed and nothing hung.
pub(crate) fn enter(gui: &mut Gui, again: bool) {
    gui.discover.menu = None;
    // The list stays where it was left (clause 61's way back); only the
    // keyboard's hand goes down.
    if !again {
        gui.discover.stow();
    }
    let effects = if again { gui.app.taste_ask_again() } else { gui.app.taste_open() };
    gui.pend(effects);
}

/// The session moved (a connect, a switch): the room follows it, or yields
/// to Files when the row went with the ping (clause 1).
pub(crate) fn reopen(gui: &mut Gui) {
    if !gate(&gui.app) {
        gui.discover.menu = None;
        gui.discover.seeds = None;
        gui.act(Act::Nav(super::FILES_NAV));
        return;
    }
    gui.discover.menu = None;
    gui.discover.seeds = None;
    gui.discover.stow();
    let effects = gui.app.taste_open();
    gui.pend(effects);
}

/// A pick armed from the room landed (or was refused): back to the room,
/// which words a refusal on its next frame.
pub(crate) fn returned(gui: &mut Gui) {
    gui.screen = Screen::Library;
    gui.active = super::DISCOVER_NAV;
    gui.discover.stow();
}

/// Handle the room's acts. Returns true when the act was this room's.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Disc(act) = act else { return false };
    match act.clone() {
        DiscAct::Tab(tab) => set_tab(gui, tab),
        DiscAct::Row(index, verb) => {
            row_act(gui, index, verb);
            // A click leaves the highlight to the keyboard (the kit's
            // list-cursor law); the keys put theirs back after this.
            gui.discover.stow();
        }
        // Only the pointer sends this (`m` calls `more` itself, keeping the
        // hand up): a click acts on its row and stows (clause 57).
        DiscAct::More(index) => {
            gui.discover.stow();
            more(gui, index);
        }
        DiscAct::Window => {
            gui.discover.menu = match gui.discover.menu {
                Some(Menu::Window(_)) => None,
                None => {
                    let current = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
                    Some(Menu::Window(WINDOWS.iter().position(|w| *w == current).unwrap_or(1)))
                }
            };
        }
        DiscAct::WindowPick(window) => {
            gui.discover.menu = None;
            let effects = gui.app.set_taste_window(window);
            gui.pend(effects);
        }
        DiscAct::MenuClose => gui.discover.menu = None,
        DiscAct::Played => {
            gui.discover.menu = None;
            let effects = gui.app.toggle_taste_played();
            gui.pend(effects);
        }
        DiscAct::Seeds => {
            gui.discover.menu = None;
            gui.discover.seeds = Some(SeedsModal::default());
        }
        // A click, never a key (Space and Enter call `seed_row` and keep the
        // cursor up): the modal's hand goes down (the list-cursor law).
        DiscAct::SeedRow(index) => {
            if let Some(modal) = gui.discover.seeds.as_mut() {
                modal.view.stow();
            }
            seed_row(gui, index);
        }
        DiscAct::SeedUnpin(i) => {
            let path = gui.app.taste.pins.get(i).map(|pin| pin.track.filepath.clone());
            if let Some(path) = path
                && gui.app.taste_unpin(&path)
                && let Some(modal) = gui.discover.seeds.as_mut()
            {
                modal.pins_changed = true;
            }
        }
        DiscAct::SeedsDone => close_seeds(gui),
        DiscAct::Another => {
            let effects = gui.app.taste_another_pick();
            gui.pend(effects);
        }
        DiscAct::Choose => choose(gui),
        DiscAct::Again => {
            let effects = gui.app.taste_ask_again();
            gui.pend(effects);
        }
        DiscAct::PlayAll => {
            let effects = gui.app.taste_play_all(false);
            gui.pend(effects);
        }
        DiscAct::Shuffle => {
            let effects = gui.app.taste_play_all(true);
            gui.pend(effects);
        }
        DiscAct::QueueAll => {
            let effects = gui.app.taste_queue_all();
            gui.pend(effects);
        }
    }
    true
}

/// Switch tabs (clause 40): asks nothing; the tab is remembered.
fn set_tab(gui: &mut Gui, tab: TasteTab) {
    gui.discover.menu = None;
    if gui.app.taste.tab == tab {
        return;
    }
    gui.app.taste.tab = tab;
    gui.discover.stow();
    gui.discover.view.scroll = 0;
    gui.config.gui.discover_tab = Some(tab.as_str().to_string());
    gui.save_now();
}

/// A row's verb (clauses 60–62): library songs and connected servers' rows
/// through the App's own verbs — a connected server's through your own
/// server, the Listen verb's way — and an album into the Albums room, or
/// onto the queue.
fn row_act(gui: &mut Gui, index: usize, verb: RowVerb) {
    let line = lines(&gui.app, gui.app.taste.tab).get(index).copied();
    let taste_verb = match verb {
        RowVerb::Open => TasteVerb::PlayFrom,
        RowVerb::Now => TasteVerb::Now,
        RowVerb::Next => TasteVerb::Next,
        RowVerb::Queue => TasteVerb::Queue,
    };
    match line {
        Some(Line::Song(i)) => {
            let effects = gui.app.activate_taste_row(TasteRow::Song(i), taste_verb);
            gui.pend(effects);
        }
        Some(Line::Peer { group, row }) => {
            let effects = gui.app.activate_taste_row(TasteRow::Peer { group, row }, taste_verb);
            gui.pend(effects);
        }
        Some(Line::Album(i)) if verb == RowVerb::Open => open_album(gui, i),
        // The album's play control queues it (clause 61).
        Some(Line::Album(i)) if verb == RowVerb::Queue => {
            let effects = gui.app.taste_queue_album(i);
            gui.pend(effects);
        }
        _ => {}
    }
}

/// An album row opens in the Albums room's drill, its Back leading to the
/// wall (clause 61); `G` brings this room back as it was left.
fn open_album(gui: &mut Gui, index: usize) {
    let Some(node) = gui.app.taste_album_node(index) else { return };
    gui.act(Act::Nav(super::ALBUMS_NAV));
    gui.albums.wall.held = false;
    gui.albums.wall.tracks.scroll = 0;
    gui.albums.wall.tracks.stow();
    let effects = gui.app.open_library_node(node, false);
    gui.pend(effects);
}

/// `[⋯]`, `m` or a right click on a song: the track-actions sheet — a
/// connected server's row with its peer origin, so the sheet offers what a
/// peer's track can do, as for a queued peer row (clause 62).
fn more(gui: &mut Gui, index: usize) {
    let (origin, track) = match lines(&gui.app, gui.app.taste.tab).get(index).copied() {
        Some(Line::Song(i)) => {
            let song = gui.app.taste.library.data.as_ref().and_then(|l| l.tracks.get(i));
            let Some(track) = song.map(|t| t.to_track()) else { return };
            (gui.app.origin(), track)
        }
        Some(Line::Peer { group, row }) => {
            let Some((peer, lead)) = gui.app.taste.peer_row(group, row) else { return };
            let Some(track) = lead.to_track() else { return };
            (Origin { server: gui.app.origin().server, peer: Some(peer) }, track)
        }
        _ => return,
    };
    super::actions::open_for(gui, origin, track, None);
}

/// A modal row, clicked or toggled by key (clause 13).
fn seed_row(gui: &mut Gui, index: usize) {
    let rows = seed_lines(&gui.app);
    let Some(line) = rows.get(index).cloned() else { return };
    match line {
        SeedLine::Seed(hash) | SeedLine::Dropped(hash) => {
            if let Some(modal) = gui.discover.seeds.as_mut() {
                if let Some(at) = modal.flipped.iter().position(|h| *h == hash) {
                    modal.flipped.remove(at);
                } else {
                    modal.flipped.push(hash);
                }
            }
        }
        SeedLine::Pin(i) => {
            let path = gui.app.taste.pins.get(i).map(|pin| pin.track.filepath.clone());
            if let Some(path) = path
                && gui.app.taste_unpin(&path)
                && let Some(modal) = gui.discover.seeds.as_mut()
            {
                modal.pins_changed = true;
            }
        }
        SeedLine::PinPlaying => {
            if gui.app.taste_pin_playing()
                && let Some(modal) = gui.discover.seeds.as_mut()
            {
                modal.pins_changed = true;
            } else if gui.app.taste.pins.len() >= crate::tui::app::TASTE_MAX_PINS {
                word_refusal(gui, TastePinRefusal::Full);
            }
        }
        SeedLine::PinSong => choose(gui),
        SeedLine::Head(_) => {}
    }
    // Rows come and go (a pin left): the cursor stays on a row that exists.
    let len = seed_lines(&gui.app).len();
    if let Some(modal) = gui.discover.seeds.as_mut()
        && let Some(cursor) = modal.cursor
        && cursor >= len
    {
        modal.cursor = len.checked_sub(1);
    }
}

/// The modal closes: newly dropped seeds go out as `exclude` with
/// `remember`, a restore is a prefs write first, then one ask; nothing
/// changed asks nothing (clause 13).
fn close_seeds(gui: &mut Gui) {
    let Some(modal) = gui.discover.seeds.take() else { return };
    let (mut dropped, mut restored) = (Vec::new(), Vec::new());
    if let Some(head) = &gui.app.taste.head {
        for hash in &modal.flipped {
            if head.seeds.iter().any(|s| &s.hash == hash && !s.pinned) {
                dropped.push(hash.clone());
            } else if head.excluded.iter().any(|e| &e.hash == hash) {
                restored.push(hash.clone());
            }
        }
    }
    let effects = gui.app.taste_seeds_closed(&dropped, &restored, modal.pins_changed);
    gui.pend(effects);
}

/// "Pin a song…" / "choose a song…" (clauses 14, 21): the modal's changes
/// apply first, then the pick is armed in the sonic room's grammar — the
/// banner over the library, the pick consumed, Esc back here.
fn choose(gui: &mut Gui) {
    if gui.discover.seeds.is_some() {
        close_seeds(gui);
    }
    gui.discover.menu = None;
    let effects = gui.app.taste_choose_song();
    gui.pend(effects);
    gui.active = super::FILES_NAV;
    gui.app.tab = crate::tui::app::Tab::Files;
}

/// The room's wheel (clause 58): the list under the pointer — the seeds
/// modal's rows while it is up, else the table. The window's four choices
/// always show whole: over its list the wheel moves nothing, and not the
/// table under it either.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if let Some(modal) = gui.discover.seeds.as_mut() {
        modal.view.step(delta);
        return;
    }
    if gui.discover.menu.is_none() {
        gui.act(Act::ScrollBy(List::Discover, delta));
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The room's keys, and its list's and modal's, which outrank everything
/// while up. `Some(quit)` when the key was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.screen != Screen::Library || gui.active != super::DISCOVER_NAV {
        return None;
    }
    if gui.discover.seeds.is_some() {
        seeds_key(gui, key);
        return Some(false);
    }
    if let Some(menu) = gui.discover.menu {
        menu_key(gui, key, menu);
        return Some(false);
    }
    if !gui.app.connected {
        return None;
    }
    let tab = gui.app.taste.tab;
    let all = lines(&gui.app, tab);
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    match key.code {
        KeyCode::Down => walk(gui, all.len(), 1),
        KeyCode::Up => walk(gui, all.len(), -1),
        KeyCode::PageDown => walk(gui, all.len(), 7),
        KeyCode::PageUp => walk(gui, all.len(), -7),
        KeyCode::Esc if held.is_some() => gui.discover.stow(),
        // The row verbs want the cursor up (clause 57): stowed, they are
        // the room's and do nothing.
        KeyCode::Enter | KeyCode::Char('a' | 'N' | 'P' | 'm') if held.is_none() => {}
        KeyCode::Enter => {
            let index = held?;
            gui.act(Act::Disc(DiscAct::Row(index, RowVerb::Open)));
            // Played by key: the cursor stays up.
            if gui.active == super::DISCOVER_NAV {
                gui.discover.cursor = Some(index);
                gui.discover.view.held = true;
            }
        }
        KeyCode::Char(c @ ('a' | 'N' | 'P')) => {
            let index = held?;
            let verb = match c {
                'a' => RowVerb::Queue,
                'N' => RowVerb::Next,
                _ => RowVerb::Now,
            };
            match all.get(index) {
                Some(Line::Song(_) | Line::Peer { .. }) => row_act(gui, index, verb),
                // An album's one queue verb (clause 61).
                Some(Line::Album(_)) if verb == RowVerb::Queue => row_act(gui, index, verb),
                _ => {}
            }
        }
        KeyCode::Char('m') => {
            let index = held?;
            more(gui, index);
        }
        KeyCode::Left | KeyCode::Right => {
            let tabs = visible_tabs(&gui.app);
            let at = tabs.iter().position(|t| *t == tab).unwrap_or(0);
            let next =
                if key.code == KeyCode::Left { at.saturating_sub(1) } else { (at + 1).min(tabs.len() - 1) };
            set_tab(gui, tabs[next]);
        }
        KeyCode::Char('w') => return Some(gui.act(Act::Disc(DiscAct::Window))),
        KeyCode::Char(c @ ('[' | ']')) => {
            let current = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
            let at = WINDOWS.iter().position(|w| *w == current).unwrap_or(1);
            let next = if c == '[' { at.saturating_sub(1) } else { (at + 1).min(WINDOWS.len() - 1) };
            if next != at {
                let effects = gui.app.set_taste_window(WINDOWS[next]);
                gui.pend(effects);
            }
        }
        KeyCode::Char('e') => return Some(gui.act(Act::Disc(DiscAct::Seeds))),
        // The checkbox, on the tab that draws it (clause 65).
        KeyCode::Char('f') if tab == TasteTab::Library => return Some(gui.act(Act::Disc(DiscAct::Played))),
        // Another pick, only while the card stands (clause 21); otherwise
        // `r` stays the transport's repeat.
        KeyCode::Char('r') if gui.app.taste.no_history => return Some(gui.act(Act::Disc(DiscAct::Another))),
        // The whole-list verbs, where the tab has something to play (clause
        // 64); otherwise the keys keep their global meanings.
        KeyCode::Char('p') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::PlayAll))),
        KeyCode::Char('A') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::QueueAll))),
        KeyCode::Char('S') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::Shuffle))),
        _ => return None,
    }
    Some(false)
}

/// A walking key (the kit's list-cursor law): a stowed cursor is picked up
/// at the top on ↓, the bottom on ↑ — the room has no resting place of its
/// own — and then walks.
fn walk(gui: &mut Gui, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    let next = match held.filter(|c| *c < len) {
        None if delta > 0 => 0,
        None => len - 1,
        Some(at) => (at as i64 + i64::from(delta)).clamp(0, len as i64 - 1) as usize,
    };
    gui.discover.cursor = Some(next);
    gui.discover.view.pick_up();
}

fn menu_key(gui: &mut Gui, key: KeyEvent, menu: Menu) {
    let Menu::Window(cursor) = menu;
    let set = |gui: &mut Gui, c: usize| gui.discover.menu = Some(Menu::Window(c));
    match key.code {
        // Esc, or the control's own key, closes and keeps (the Dropdown law).
        KeyCode::Esc | KeyCode::Char('w') => gui.discover.menu = None,
        KeyCode::Up => set(gui, cursor.saturating_sub(1)),
        KeyCode::Down => set(gui, (cursor + 1).min(WINDOWS.len() - 1)),
        KeyCode::Enter => {
            gui.act(Act::Disc(DiscAct::WindowPick(WINDOWS[cursor.min(WINDOWS.len() - 1)])));
        }
        _ => {}
    }
}

fn seeds_key(gui: &mut Gui, key: KeyEvent) {
    let rows = seed_lines(&gui.app);
    let picks: Vec<usize> = rows.iter().enumerate().filter(|(_, l)| l.is_row()).map(|(i, _)| i).collect();
    let Some(modal) = gui.discover.seeds.as_mut() else { return };
    let held = modal.view.held.then_some(modal.cursor).flatten();
    match key.code {
        KeyCode::Esc => close_seeds(gui),
        KeyCode::Down | KeyCode::Up if !picks.is_empty() => {
            let down = key.code == KeyCode::Down;
            let next = match held.and_then(|c| picks.iter().position(|p| *p == c)) {
                None if down => picks[0],
                None => picks[picks.len() - 1],
                Some(at) if down => picks[(at + 1).min(picks.len() - 1)],
                Some(at) => picks[at.saturating_sub(1)],
            };
            modal.cursor = Some(next);
            modal.view.pick_up();
        }
        KeyCode::Char(' ') | KeyCode::Enter => {
            if let Some(index) = held {
                seed_row(gui, index);
            }
        }
        KeyCode::Char('x') => {
            if let Some(SeedLine::Pin(i)) = held.and_then(|index| rows.get(index)) {
                gui.act(Act::Disc(DiscAct::SeedUnpin(*i)));
            }
        }
        _ => {}
    }
}

// ── Tips ────────────────────────────────────────────────────────────────────

/// The tips line for this room's states.
pub(crate) fn tips(gui: &Gui) -> String {
    if gui.discover.seeds.is_some() {
        return t!("gui.tips.discover_seeds").to_string();
    }
    if gui.discover.menu.is_some() {
        return t!("gui.tips.discover_menu").to_string();
    }
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    match held.and_then(|i| lines(&gui.app, gui.app.taste.tab).get(i).copied()) {
        // Only what works on THIS row (the kit's tips law): an album opens
        // or queues whole (clause 61); a connected server's row has every
        // song verb (clause 62).
        Some(Line::Album(_)) => t!("gui.tips.discover_album").to_string(),
        Some(Line::Song(_) | Line::Peer { .. }) => t!("gui.tips.discover_row").to_string(),
        None if gui.app.taste.no_history => t!("gui.tips.discover_fallback").to_string(),
        None => t!("gui.tips.discover").to_string(),
    }
}

/// The banner over the library while the room's pick is armed (clause 14).
pub(crate) fn banner(gui: &Gui) -> Option<String> {
    (gui.app.capture == Some(Capture::TasteSeed)).then(|| t!("gui.discover.pick_banner").to_string())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::{Position, Rect};
    use serde_json::json;

    use super::super::{Act, DISCOVER_NAV, FILES_NAV, Gui, List, RowVerb, Screen, render};
    use super::{DiscAct, Line, Wants, age_words, columns, layout, lines};
    use crate::api::types::{Capabilities, TasteFilter, TasteResponse};
    use crate::config::Config;
    use crate::kit::theme::th;
    use crate::tui::app::{App, Effect, TasteLatch, TasteTab};
    use crate::tui::worker::{ApiCmd, Event, TasteFailure, TastePart};

    const HOST: &str = "http://host:3000";

    /// The tests read English words.
    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn caps() -> Capabilities {
        Capabilities {
            discovery: true,
            discovery_path: true,
            discovery_p2p: true,
            federation_discovery: true,
            discovery_taste: 1,
            discovery_plugins: true,
            ..Capabilities::default()
        }
    }

    /// A connected session whose server carries the room's gate, the queue
    /// panel folded so the room has its 80 cells.
    fn session_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui.app.capabilities = caps();
        gui.queue_open = false;
        gui
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn draw_sized(gui: &mut Gui, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height).map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        draw_sized(gui, 100, 30)
    }

    fn draw_buffer(gui: &mut Gui) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Where `text` stands on screen.
    fn at(rows: &[String], text: &str) -> Position {
        for (y, row) in rows.iter().enumerate() {
            if let Some(i) = row.find(text) {
                let x = row[..i].chars().count();
                return Position { x: x as u16, y: y as u16 };
            }
        }
        panic!("{text:?} is not on screen:\n{}", rows.join("\n"));
    }

    /// The room's own rows (the content right of the nav column, above the
    /// footer), for the play-now page's laws: no count, no meter, no
    /// percentage and no "never played" anywhere on the page (the
    /// 2026-10-11 log). The only digits allowed are a row's m:ss length and
    /// the window's days. (The fallback card's "Skips never count." is the
    /// webapp's own sentence; the seeds modal behind `edit` keeps its ages.)
    fn assert_no_counts(rows: &[String]) {
        let time = regex::Regex::new(r"\b\d+:\d\d\b").unwrap();
        let window = regex::Regex::new(r"Last \d+ days").unwrap();
        for (y, row) in rows.iter().enumerate().take(26).skip(2) {
            let room: String = row.chars().skip(17).collect();
            let rest = window.replace_all(&time.replace_all(&room, ""), "").into_owned();
            assert!(!rest.chars().any(|c| c.is_ascii_digit()), "a number on row {y}:\n{}", rows.join("\n"));
            for word in ['%', '▇', '▆', '▅', '▄'] {
                assert!(!room.contains(word), "{word:?} on row {y}:\n{}", rows.join("\n"));
            }
            assert!(!room.to_lowercase().contains("never played"), "\"never played\" on row {y}:\n{}", rows.join("\n"));
        }
    }

    /// Ask A, the library: three history seeds, one saved exclusion, an album
    /// and four songs. The server's network half rides along, as an older
    /// answer could carry it; the room draws none of it.
    fn main_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "window": 30, "seedSource": "history", "fallback": false,
            "seeds": [
                { "hash": "h1", "filepath": "music/manor.mp3", "metadata": { "title": "Manor", "artist": "ALM" },
                  "outcome": "completed", "lastPlayed": "2026-10-01T10:00:00.000Z", "plays": 4 },
                { "hash": "h2", "filepath": "music/kite.mp3", "metadata": { "title": "Kite", "artist": "Boukmanflow" },
                  "outcome": "stopped", "plays": 1, "analyzed": false },
                { "hash": "h3", "filepath": "music/dune.mp3", "metadata": { "title": "Dune", "artist": "Color Out" } }
            ],
            "excluded": [ { "hash": "h9", "filepath": "music/old.mp3", "metadata": { "title": "Old Song", "artist": "Gone" } } ],
            "library": {
                "filter": "fresh",
                "albums": [ { "name": "Night Album", "album_artist": "Moon", "similarity": 0.86, "scopedTracks": 3, "playedTracks": 0 } ],
                "tracks": [
                    { "filepath": "music/a.mp3", "similarity": 0.83, "seedHash": "h1",
                      "metadata": { "title": "Aurora", "artist": "Ray", "album": "Dawn", "duration": 251 } },
                    { "filepath": "music/b.mp3", "similarity": 0.80,
                      "metadata": { "title": "Borealis", "artist": "Ray", "album": "Dawn", "duration": 200 }, "lastPlayed": "2024-01-01T00:00:00.000Z" },
                    { "filepath": "music/c.mp3", "similarity": 0.66, "metadata": { "title": "Cirrus", "duration": 180 } },
                    { "filepath": "music/d.mp3", "similarity": 0.61, "metadata": { "title": "Delta", "artist": "Ray" } }
                ]
            },
            "network": {
                "results": [ { "artist": "Lead Artist", "title": "Far Away", "similarity": 0.86, "source": "p2p",
                               "peer": { "endpointId": "e1", "name": "Sam's" } } ],
                "searched": { "peers": 3, "tracks": 900 }
            }
        }))
        .unwrap()
    }

    /// Ask B, the connected servers: one answering with two songs, one not
    /// answering, one on another sonic model with a row of its own.
    fn peers_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "seedSource": "history",
            "peers": {
                "peers": [
                    { "id": 7, "name": "Sam's server", "reachable": true, "results": [
                        { "artist": "Pia", "title": "Peer One", "album": "Theirs", "filepath": "Music/p1.mp3", "similarity": 0.85,
                          "duration": 200, "peer": { "id": 7, "name": "Sam's server" }, "source": "federation" },
                        { "artist": "Pia", "title": "Peer Two", "filepath": "Music/p2.mp3", "similarity": 0.72,
                          "peer": { "id": 7, "name": "Sam's server" }, "source": "federation" }
                    ] },
                    { "id": 9, "name": "Far box", "reachable": false, "lastSeen": "2026-09-01T00:00:00.000Z", "results": [] },
                    { "id": 11, "name": "Odd model", "reachable": true, "mismatched": true, "results": [
                        { "artist": "Zed", "title": "Elsewhere", "filepath": "Music/z.mp3", "similarity": 0.5,
                          "peer": { "id": 11, "name": "Odd model" }, "source": "federation" }
                    ] }
                ],
                "searched": { "peers": 3, "unreachable": 1, "mismatched": 1 }
            }
        }))
        .unwrap()
    }

    /// Nothing finished yet: the server's random seed, the library around it.
    fn fallback_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "seedSource": "random", "fallback": true, "window": 30,
            "seeds": [ { "hash": "r1", "filepath": "music/random.mp3", "metadata": { "title": "Random One", "artist": "Dice" } } ],
            "library": { "filter": "fresh", "tracks": [
                { "filepath": "music/a.mp3", "similarity": 0.79, "metadata": { "title": "Aurora", "artist": "Ray", "duration": 251 } }
            ] },
            "network": null
        }))
        .unwrap()
    }

    /// The seq and generation of the last ask of `part` the room pended.
    fn ask_of(gui: &Gui, part: TastePart) -> (u64, u64) {
        gui.pending
            .iter()
            .rev()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::Taste { seq, generation, part: p, .. }) if *p == part => Some((*seq, *generation)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {part:?} ask in {:?}", gui.pending))
    }

    fn land(gui: &mut Gui, part: TastePart, answer: TasteResponse) {
        let (seq, generation) = ask_of(gui, part);
        let effects = gui.app.apply_event(Event::Taste { seq, generation, part, answer: Ok(Box::new(answer)) });
        gui.pend(effects);
    }

    fn fail(gui: &mut Gui, part: TastePart, failure: TasteFailure) {
        let (seq, generation) = ask_of(gui, part);
        let effects = gui.app.apply_event(Event::Taste { seq, generation, part, answer: Err(failure) });
        gui.pend(effects);
    }

    /// The asks a set of pended effects carries: part, sources, filter,
    /// remember.
    fn asks(gui: &Gui) -> Vec<(TastePart, Vec<String>, Option<TasteFilter>, Option<bool>)> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Taste { part, body, .. }) => {
                    Some((*part, body.sources.clone(), body.filter, body.remember))
                }
                _ => None,
            })
            .collect()
    }

    /// The room entered with `G` and both halves answered.
    fn answered_gui() -> Gui {
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(gui.active, DISCOVER_NAV, "G opens the room");
        land(&mut gui, TastePart::Main, main_answer());
        land(&mut gui, TastePart::Peers, peers_answer());
        gui
    }

    // ── The row ─────────────────────────────────────────────────────────

    #[test]
    fn the_row_sits_at_16_under_sonic_and_never_meets_settings() {
        let _en = english();
        for (height, footer, settings) in [(30u16, false, 24u16), (30, true, 23), (24, false, 18), (24, true, 17)] {
            let mut gui = session_gui();
            gui.config.gui.key_hints = footer;
            gui.ui.key_hints = footer;
            let rows = draw_sized(&mut gui, 100, height);
            let nav = |y: u16| rows[y as usize].chars().take(15).collect::<String>();
            assert!(nav(13).contains("TOOLS"), "{height}/{footer}: TOOLS at 13:\n{}", rows.join("\n"));
            assert!(nav(14).contains("Auto DJ"), "{height}/{footer}: Auto DJ at 14");
            assert!(nav(15).contains("Sonic path"), "{height}/{footer}: Sonic at 15");
            assert!(nav(16).contains("Discover"), "{height}/{footer}: Discover at 16");
            assert!(nav(settings).contains("Settings"), "{height}/{footer}: Settings at {settings}:\n{}", rows.join("\n"));
            assert!(
                !nav(settings).contains("Discover") && !nav(16).contains("Settings"),
                "{height}/{footer}: Discover and Settings never share a row"
            );
            let pos = at(&rows, "Discover");
            assert_eq!(gui.ui.hit(pos), Some(Act::Nav(DISCOVER_NAV)), "the row is a click target");
        }
    }

    #[test]
    fn the_row_is_absent_without_the_gate_and_for_a_peer() {
        let _en = english();
        let nav_has = |gui: &mut Gui| draw(gui).iter().any(|r| r.chars().take(15).collect::<String>().contains("Discover"));
        // No taste generation: no row, and G is as dead as the row.
        let mut gui = session_gui();
        gui.app.capabilities.discovery_taste = 0;
        assert!(!nav_has(&mut gui), "absent is absent without discoveryTaste");
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(gui.active, FILES_NAV, "G goes nowhere while the row is absent");
        assert!(gui.pending.is_empty(), "and asks nothing: {:?}", gui.pending);
        // The index off: no row either.
        let mut gui = session_gui();
        gui.app.capabilities.discovery = false;
        assert!(!nav_has(&mut gui));
        // A peer's session: the guest has no history there.
        let mut gui = session_gui();
        gui.app.session.peer = Some((HOST.into(), 7));
        assert!(!nav_has(&mut gui), "hidden for a peer like the play lists");
        // The room up when the gate goes: it yields to Files.
        let mut gui = answered_gui();
        gui.app.capabilities.discovery_taste = 0;
        gui.reopen_room();
        assert_eq!(gui.active, FILES_NAV, "the room yields to Files with the row gone");
    }

    #[test]
    fn g_asks_the_library_and_the_connected_servers_and_never_the_network() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(
            asks(&gui),
            vec![(TastePart::Main, vec!["library".to_string()], None, None), (TastePart::Peers, vec!["peers".to_string()], None, None)],
            "two asks in parallel, the p2p network in neither, even advertised (clause 30)"
        );
        land(&mut gui, TastePart::Main, main_answer());
        land(&mut gui, TastePart::Peers, peers_answer());
        gui.pending.clear();
        // A fresh visit within five minutes reuses the answer…
        gui.act(Act::Nav(FILES_NAV));
        gui.act(Act::Nav(DISCOVER_NAV));
        assert!(asks(&gui).is_empty(), "re-entry reuses a fresh answer: {:?}", gui.pending);
        // …and G (or the row) while the room is up asks at once.
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(asks(&gui).len(), 2, "G asks again: {:?}", gui.pending);
    }

    // ── States ──────────────────────────────────────────────────────────

    #[test]
    fn the_first_ask_shows_the_scan_bar_and_asking() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[3].contains("• asking…"), "the state word says it is asking:\n{all}");
        assert!(all.contains("TRACK") && all.contains("ARTIST"), "the table's header stands:\n{all}");
        // Four skeleton rows under the rule, and no "asking…" line in the pane.
        for y in 10..14 {
            assert!(rows[y].contains("░░░"), "a skeleton row at {y}:\n{all}");
        }
        assert!(!rows[14].contains('░'), "four of them:\n{all}");
        assert_eq!(all.matches("asking").count(), 1, "the state word alone says it:\n{all}");
        assert_no_counts(&rows);
        key(&mut gui, KeyCode::Right);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[10].contains("░░░") && rows[8].contains(" ON "), "the Connected servers skeleton:\n{all}");
        assert_eq!(all.matches("asking").count(), 1, "{all}");
    }

    #[test]
    fn an_answer_fills_the_header_the_tabs_the_seeds_and_the_table_with_no_counts() {
        let _en = english();
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].contains("Discover · based on songs you finished"), "the header:\n{}", rows[2]);
        assert!(rows[2].trim_end().ends_with("Last 30 days ▾"), "the window dropdown at its right:\n{}", rows[2]);
        assert!(rows[3].contains("• ready to play"), "the state line, words only:\n{}", rows[3]);
        assert!(rows[5].contains(" Library ") && rows[5].contains(" Connected servers "), "two tabs:\n{}", rows[5]);
        assert!(!all.contains("Network") && !all.contains("Peers"), "the network and the old Peers name are gone:\n{all}");
        assert!(rows[6].contains("seeds") && rows[6].contains("Manor · ALM") && rows[6].contains("edit"), "seeds:\n{}", rows[6]);
        for head in ["TRACK", "ARTIST", "ALBUM", "LENGTH"] {
            assert!(rows[8].contains(head), "{head} in the header:\n{}", rows[8]);
        }
        assert!(!rows[8].contains("MATCH") && !rows[8].contains("LAST PLAYED"), "no meter, no last played:\n{}", rows[8]);
        // Albums first, a container with its marker and its artist; then the
        // songs: title, artist, album, length (clauses 50–51).
        assert!(rows[10].contains("Night Album album") && rows[10].contains("Moon"), "album row:\n{}", rows[10]);
        assert!(!rows[10].contains("songs"), "no song count on an album:\n{}", rows[10]);
        for cell in ["Aurora", "Ray", "Dawn", "4:11"] {
            assert!(rows[11].contains(cell), "{cell} on the song row:\n{}", rows[11]);
        }
        assert_eq!(at(&rows, "Ray").x, at(&rows, "ARTIST").x, "ARTIST over its words");
        assert_eq!(at(&rows, "Dawn").x, at(&rows, "ALBUM").x, "ALBUM over its words");
        assert!(rows[13].contains("Cirrus") && rows[13].contains("—"), "a missing artist is the dash:\n{}", rows[13]);
        // The bottom row: the checkbox at the left, the verbs at the right.
        assert!(rows[24].contains("[ ] Include songs I've played lately"), "the checkbox, unticked:\n{}", rows[24]);
        assert!(rows[24].contains("play · + queue all"), "bottom:\n{}", rows[24]);
        assert!(!all.contains("asking"), "nothing asking once both halves landed:\n{all}");
        assert_no_counts(&rows);
    }

    #[test]
    fn the_queue_panel_drops_album_first_then_on() {
        let _en = english();
        let mut gui = answered_gui();
        gui.queue_open = true;
        let rows = draw(&mut gui);
        assert!(rows[8].contains("TRACK") && rows[8].contains("ARTIST") && rows[8].contains("LENGTH"), "{}", rows[8]);
        assert!(!rows[8].contains("ALBUM"), "47 cells: ALBUM goes first:\n{}", rows[8]);
        assert!(rows[2].contains("Last 30 days ▾"), "the window control keeps its home:\n{}", rows[2]);
        assert!(rows[24].contains("[ ] Include songs"), "the checkbox keeps its home:\n{}", rows[24]);
        assert_no_counts(&rows);
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        let rows = draw(&mut gui);
        assert!(rows[8].contains("TRACK") && rows[8].contains("ARTIST") && rows[8].contains("LENGTH"), "{}", rows[8]);
        assert!(!rows[8].contains("ALBUM") && !rows[8].contains(" ON "), "then ON:\n{}", rows[8]);
        // With the room's 80 cells, all five stand.
        gui.queue_open = false;
        let rows = draw(&mut gui);
        for head in ["TRACK", "ARTIST", "ALBUM", "ON", "LENGTH"] {
            assert!(rows[8].contains(head), "{head} at 80 cells:\n{}", rows[8]);
        }
    }

    #[test]
    fn the_connected_servers_tab_lists_only_what_plays_now_and_plays_through_your_server() {
        let _en = english();
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        assert_eq!(gui.config.gui.discover_tab.as_deref(), Some("servers"), "the tab is remembered");
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[3].contains("• ready to play · streams through your server"), "{}", rows[3]);
        // No group lines: the rows start under the rule.
        assert!(rows[10].contains("Peer One") && rows[10].contains("Pia") && rows[10].contains("Theirs"), "{}", rows[10]);
        assert!(rows[10].contains("Sam's server") && rows[10].contains("3:20"), "ON and LENGTH:\n{}", rows[10]);
        assert!(rows[11].contains("Peer Two"), "{}", rows[11]);
        for gone in ["Far box", "Odd model", "Elsewhere", "online", "not answering", "last seen", "sonic model"] {
            assert!(!all.contains(gone), "{gone:?} is not drawn:\n{all}");
        }
        assert_no_counts(&rows);
        // ↓ lands on the first song; Enter plays it through your server (the
        // Listen verb's way) and queues the tab's rows after it.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.cursor, Some(0));
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        let item = &gui.app.queue.items[0];
        assert_eq!((item.origin.server.as_str(), item.origin.peer, item.filepath.as_str()), (HOST, Some(7), "Music/p1.mp3"));
        assert_eq!(gui.app.queue.items.len(), 2, "the tab's rows queue after it, the mismatched server's not among them");
        let url = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Audio(crate::tui::worker::AudioCmd::Play { url, .. }) => Some(url.clone()),
                _ => None,
            })
            .expect("Enter plays");
        assert!(url.starts_with("http://host:3000/api/v1/federation/peers/7/stream/Music/p1.mp3"), "through the proxy: {url}");
        // [»] and [+] queue it.
        gui.act(Act::Disc(DiscAct::Row(1, RowVerb::Queue)));
        assert_eq!(gui.app.queue.items.last().map(|q| (q.filepath.as_str(), q.origin.peer)), Some(("Music/p2.mp3", Some(7))));
    }

    #[test]
    fn a_connected_servers_row_has_the_sheet_with_its_peer_origin() {
        let _en = english();
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        draw(&mut gui);
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        let rows = draw(&mut gui);
        assert!(rows[11].contains("[▸] [»] [+] [⋯]"), "the song verbs, [⋯] among them:\n{}", rows[11]);
        let dots = Position { x: rows[11].find("[⋯]").map(|i| rows[11][..i].chars().count()).unwrap() as u16, y: 11 };
        assert_eq!(gui.ui.hit(dots), Some(Act::Disc(DiscAct::More(1))));
        assert_eq!(gui.ui.hit_context(Position { x: 30, y: 10 }), Some(Act::Disc(DiscAct::More(0))), "a right click too");
        gui.act(Act::Disc(DiscAct::More(1)));
        let sheet = gui.actions.sheet.as_ref().expect("the sheet opens");
        assert_eq!((sheet.origin.server.as_str(), sheet.origin.peer), (HOST, Some(7)), "the peer's origin");
        assert_eq!(sheet.track.filepath, "Music/p2.mp3");
        assert_eq!(sheet.queue_row, None);
        // `m` with the cursor up, too.
        gui.actions.sheet = None;
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('m'));
        assert_eq!(gui.actions.sheet.as_ref().map(|s| s.track.filepath.as_str()), Some("Music/p1.mp3"));
    }

    #[test]
    fn an_album_rows_play_control_queues_the_album() {
        let _en = english();
        let mut gui = answered_gui();
        gui.ui.pointer = Some(Position { x: 30, y: 10 });
        let rows = draw(&mut gui);
        assert!(rows[10].contains("[+]") && !rows[10].contains("[»]") && !rows[10].contains("[⋯]"), "one control:\n{}", rows[10]);
        let plus = Position { x: rows[10].find("[+]").map(|i| rows[10][..i].chars().count()).unwrap() as u16, y: 10 };
        assert_eq!(gui.ui.hit(plus), Some(Act::Disc(DiscAct::Row(0, RowVerb::Queue))));
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Queue)));
        let generation = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::TasteAlbum { name, generation }) if name == "Night Album" => Some(*generation),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the album's songs are asked by name: {:?}", gui.pending));
        assert_eq!(gui.active, DISCOVER_NAV, "the room stays");
        let tracks: Vec<crate::api::types::Track> = serde_json::from_value(json!([
            { "filepath": "music/n1.mp3", "metadata": { "title": "Night One" } },
            { "filepath": "music/n2.mp3", "metadata": { "title": "Night Two" } }
        ]))
        .unwrap();
        let effects = gui.app.apply_event(Event::TasteAlbum { generation, tracks: Some(tracks.clone()) });
        gui.pend(effects);
        let queued: Vec<_> = gui.app.queue.items.iter().map(|q| (q.filepath.as_str(), q.origin.peer)).collect();
        assert_eq!(queued, vec![("music/n1.mp3", None), ("music/n2.mp3", None)], "appended with the session's origin");
        // Another generation fills nothing; a failure says so.
        gui.app.apply_event(Event::TasteAlbum { generation: generation + 1, tracks: Some(tracks) });
        assert_eq!(gui.app.queue.items.len(), 2);
        gui.app.apply_event(Event::TasteAlbum { generation, tracks: None });
        assert_eq!(gui.app.message.as_ref().map(|m| m.text.as_str()), Some("couldn't load the album"));
        // `a` on the album row with the cursor up asks the same.
        gui.pending.clear();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('a'));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteAlbum { .. }))), "{:?}", gui.pending);
    }

    #[test]
    fn hover_shows_the_verbs_under_the_pointer_and_never_selects() {
        let _en = english();
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        let rows_hover = draw(&mut gui);
        assert!(rows_hover[11].contains("[▸] [»] [+] [⋯]"), "the hover verbs:\n{}", rows_hover[11]);
        assert!(rows_hover[11].contains("Aurora") && rows_hover[11].contains("Ray"), "TRACK and ARTIST stay:\n{}", rows_hover[11]);
        assert!(!rows_hover[10].contains("[▸]"), "an album has no play-now:\n{}", rows_hover[10]);
        assert_eq!(gui.discover.cursor, None, "hover never selects");
        let plus = Position { x: rows_hover[11].find("[+]").map(|i| rows_hover[11][..i].chars().count()).unwrap() as u16, y: 11 };
        assert_eq!(gui.ui.hit(plus), Some(Act::Disc(DiscAct::Row(1, RowVerb::Queue))));
        gui.act(Act::Disc(DiscAct::Row(1, RowVerb::Queue)));
        let item = &gui.app.queue.items[0];
        assert_eq!((item.filepath.as_str(), item.origin.server.as_str(), item.origin.peer), ("music/a.mp3", HOST, None));
        assert!(rows[11].contains("4:11"), "at rest the row keeps its columns");
    }

    #[test]
    fn down_lights_the_slab_and_the_verbs_want_the_cursor_up() {
        let _en = english();
        let mut gui = answered_gui();
        let buffer = draw_buffer(&mut gui);
        assert!((10..16).all(|y| buffer[(40, y)].bg != th().accent), "nothing lit on arrival");
        // Stowed, Enter and `a` do nothing (clause 57).
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Char('a'));
        assert!(gui.app.queue.items.is_empty(), "no verb on a row nobody can see");
        key(&mut gui, KeyCode::Down);
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(40, 10)].bg, th().accent, "↓ lights the top row");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.queue.items.len(), 4, "Enter plays from here: the tab's songs");
        assert_eq!(gui.app.queue.current, Some(0), "the row it was on");
        assert_eq!(gui.discover.cursor, Some(1), "played by key, the cursor stays up");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.discover.cursor, None, "Esc stows it");
        assert_eq!(gui.active, DISCOVER_NAV);
    }

    #[test]
    fn the_fallback_card_stands_while_nothing_is_finished() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        land(&mut gui, TastePart::Main, fallback_answer());
        land(&mut gui, TastePart::Peers, serde_json::from_value(json!({ "seedSource": "random", "peers": null })).unwrap());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].contains("Discover · nothing finished yet"), "{}", rows[2]);
        assert!(rows[3].contains("• no seeds · suggestions around a random pick"), "{}", rows[3]);
        assert!(all.contains("Finish a few songs and this room learns your taste"), "{all}");
        assert!(all.contains("Skips never count."), "the body at the design size:\n{all}");
        assert!(all.contains("Until then, around a random pick: Random One · Dice"), "{all}");
        assert!(all.contains("another pick") && all.contains("choose a song…"), "{all}");
        let tabs = &rows[13];
        assert!(tabs.contains("Library") && tabs.contains("Connected servers") && !tabs.contains('–'), "no counts:\n{tabs}");
        assert!(rows[14].contains("Random One · Dice random"), "the seed wears `random`:\n{}", rows[14]);
        assert_no_counts(&rows);
        key(&mut gui, KeyCode::Right);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Asked once you've finished a song"), "{all}");
        assert_no_counts(&rows);
        // Another pick asks random-songs (clause 21).
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('r'));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteRandom { .. }))), "{:?}", gui.pending);
        // At the floor with the footer on, the card gives up its body and
        // the table keeps its header, rule and a row.
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = draw_sized(&mut gui, 100, 24);
        let all = rows.join("\n");
        assert!(all.contains("Finish a few songs") && !all.contains("Skips never count."), "{all}");
        assert!(all.contains("TRACK") && all.contains("another pick"), "{all}");
    }

    #[test]
    fn scan_pending_switched_off_and_failure_say_so() {
        let _en = english();
        // The answer says the scan has not run.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = main_answer();
        answer.not_analyzed = true;
        land(&mut gui, TastePart::Main, answer);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("This server hasn't analyzed any music yet") && all.contains("Check again"), "{all}");
        assert!(all.contains("• waiting for the scan"), "{all}");
        assert_no_counts(&rows);
        // A 403 whose probe lost the gate: switched off, no retry.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        fail(&mut gui, TastePart::Main, TasteFailure::Forbidden);
        assert_eq!(gui.app.taste.latch, TasteLatch::Probing);
        gui.app.apply_event(Event::DiscoveryProbe { available: false, taste: false });
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("• switched off") && all.contains("Sonic discovery has been switched off on this server."), "{all}");
        assert!(!all.contains("Try again") && !all.contains("Check again"), "no retry:\n{all}");
        // A transport failure: the half that failed says so, with Try again.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        fail(&mut gui, TastePart::Main, TasteFailure::Failed("server error 502: bad gateway".into()));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("couldn't reach this source") && all.contains("Try again"), "{all}");
        assert!(!all.contains("502") && !all.contains("bad gateway"), "no status code on the page:\n{all}");
        assert!(rows[3].contains("• no answer"), "{}", rows[3]);
        assert_no_counts(&rows);
        gui.pending.clear();
        let retry = at(&rows, "Try again");
        let act = gui.ui.hit(retry).expect("Try again is a click target");
        gui.act(act);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Taste { .. }))), "Try again asks again");
    }

    #[test]
    fn an_empty_library_offers_the_checkbox_and_quiet_servers_say_so() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = main_answer();
        let library = answer.library.as_mut().unwrap();
        library.tracks.clear();
        library.albums.clear();
        land(&mut gui, TastePart::Main, answer);
        land(
            &mut gui,
            TastePart::Peers,
            serde_json::from_value(json!({ "seedSource": "history", "peers": {
                "peers": [ { "id": 7, "name": "Sam's server", "reachable": true, "results": [] } ],
                "searched": { "peers": 1, "unreachable": 0, "mismatched": 0 } } }))
            .unwrap(),
        );
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Nothing new in your library sounds like these songs."), "{all}");
        assert!(rows[3].contains("• nothing new"), "{}", rows[3]);
        assert_no_counts(&rows);
        let button = at(&rows, "Include songs I've played lately");
        assert!(button.y < 24, "the button stands under the sentence, not the bottom row's checkbox:\n{all}");
        assert_eq!(gui.ui.hit(button), Some(Act::Disc(DiscAct::Played)));
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Played));
        assert_eq!(asks(&gui), vec![(TastePart::Library, vec!["library".to_string()], Some(TasteFilter::All), Some(true))]);
        let rows = draw(&mut gui);
        assert!(rows[24].contains("[✓] Include songs I've played lately"), "ticked:\n{}", rows[24]);
        assert_eq!(rows.iter().filter(|r| r.contains("Include songs")).count(), 1, "ticked, the button goes");
        // A connected server answering nothing: one quiet line, and the
        // state line claims nothing streams (clause 41).
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Nothing from your connected servers right now."), "{all}");
        assert!(rows[3].contains("• nothing new") && !rows[3].contains("streams"), "{}", rows[3]);
        assert_no_counts(&rows);
        // No connected servers at all: no tab.
        gui.act(Act::Disc(DiscAct::Again));
        land(
            &mut gui,
            TastePart::Peers,
            serde_json::from_value(json!({ "seedSource": "history", "peers": { "peers": [], "searched": { "peers": 0 } } })).unwrap(),
        );
        let rows = draw(&mut gui);
        assert!(!rows[5].contains("Connected servers"), "nothing connected, no tab:\n{}", rows[5]);
        assert_eq!(gui.app.taste.tab, TasteTab::Library);
    }

    #[test]
    fn the_seeds_modal_sends_the_exclusions_once_when_it_closes() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Char('e'));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Based on") && all.contains("FROM YOUR PLAYS") && all.contains("DROPPED"), "{all}");
        assert!(all.contains("[✓] Manor · ALM") && all.contains("finished"), "{all}");
        assert!(!all.contains("plays ·") && !all.contains("4 plays"), "no play counts:\n{all}");
        assert!(all.contains("not analyzed yet") && all.contains("[ ] Old Song · Gone"), "{all}");
        gui.pending.clear();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("[ ] Manor · ALM"), "Space drops it, in the modal only:\n{all}");
        assert!(gui.pending.is_empty(), "nothing goes out while it is open");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.seeds.is_none());
        let bodies: Vec<_> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Taste { part: TastePart::Main, body, .. }) => Some(body.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(bodies.len(), 1, "one ask: {:?}", gui.pending);
        assert_eq!((bodies[0].exclude.clone(), bodies[0].remember), (vec!["h1".to_string()], Some(true)));
        // A modal closed with nothing changed asks nothing.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('e'));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
    }

    #[test]
    fn the_window_is_a_dropdown_and_the_filter_one_checkbox() {
        let _en = english();
        let mut gui = answered_gui();
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('w'));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("• Last 30 days") && all.contains("Last 7 days") && all.contains("All time"), "{all}");
        let pick = at(&rows, "All time");
        assert_eq!(gui.ui.hit(pick), Some(Act::Disc(DiscAct::WindowPick(0))));
        gui.act(Act::Disc(DiscAct::WindowPick(0)));
        let main = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::Taste { part: TastePart::Main, body, .. }) => Some(body.clone()),
            _ => None,
        });
        let main = main.expect("a window change asks");
        assert_eq!((main.window, main.remember), (Some(0), Some(true)));
        // The checkbox: a click (or `f`) ticks it — `all`, the library alone,
        // remembered; again unticks it — `fresh` (clause 65).
        let rows = draw(&mut gui);
        let check = at(&rows, "[ ] Include songs I've played lately");
        assert_eq!(gui.ui.hit(Position { x: check.x + 6, y: check.y }), Some(Act::Disc(DiscAct::Played)));
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('f'));
        assert_eq!(asks(&gui), vec![(TastePart::Library, vec!["library".to_string()], Some(TasteFilter::All), Some(true))]);
        assert!(draw(&mut gui)[24].contains("[✓] Include songs I've played lately"));
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Played));
        assert_eq!(asks(&gui), vec![(TastePart::Library, vec!["library".to_string()], Some(TasteFilter::Fresh), Some(true))]);
        assert!(gui.discover.menu.is_none(), "no list hangs from the checkbox");
        // A session's leftover "Never played" reads as unticked.
        gui.app.taste.filter = Some(TasteFilter::Unplayed);
        assert!(draw(&mut gui)[24].contains("[ ] Include songs"));
    }

    #[test]
    fn a_saved_never_played_reads_as_unticked_before_any_answer() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        // The prefs read landed first: only they say the filter.
        let prefs = |filter| crate::api::types::TastePrefs { filter: Some(filter), ..Default::default() };
        gui.app.taste.prefs = Some(prefs(TasteFilter::Unplayed));
        assert!(gui.app.taste.library.data.is_none() && gui.app.taste.filter.is_none());
        assert!(draw(&mut gui)[24].contains("[ ] Include songs I've played lately"), "a saved unplayed is unticked");
        gui.app.taste.prefs = Some(prefs(TasteFilter::All));
        assert!(draw(&mut gui)[24].contains("[✓] Include songs I've played lately"), "a saved all is ticked");
    }

    #[test]
    fn an_album_row_opens_in_the_albums_room() {
        let _en = english();
        let mut gui = answered_gui();
        gui.ui.pointer = Some(Position { x: 30, y: 10 });
        draw(&mut gui);
        assert_eq!(gui.ui.hit(Position { x: 30, y: 10 }), Some(Act::Disc(DiscAct::Row(0, RowVerb::Open))));
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Open)));
        assert_eq!(gui.active, super::super::ALBUMS_NAV);
        assert!(matches!(
            gui.app.library_stack.here(),
            crate::tui::worker::LibraryNode::Album { name, artist } if name == "Night Album" && artist.is_none()
        ));
    }

    #[test]
    fn the_wheel_scrolls_the_table() {
        let _en = english();
        let mut gui = answered_gui();
        let mut answer = main_answer();
        let base = answer.library.as_ref().unwrap().tracks[0].clone();
        for i in 0..30 {
            let mut more = base.clone();
            more.filepath = format!("music/more{i}.mp3");
            answer.library.as_mut().unwrap().tracks.push(more);
        }
        gui.act(Act::Disc(DiscAct::Again));
        land(&mut gui, TastePart::Main, answer);
        draw(&mut gui);
        gui.wheel(Position { x: 40, y: 12 }, 3);
        draw(&mut gui);
        assert_eq!(gui.discover.view.scroll, 3, "the wheel scrolls the list under the pointer");
        gui.act(Act::ScrollBy(List::Discover, -1));
        assert_eq!(gui.discover.view.scroll, 2);
    }

    #[test]
    fn the_remembered_tab_comes_back_and_falls_back_when_its_source_is_gone() {
        let _en = english();
        for (word, tab) in [("servers", TasteTab::Servers), ("peers", TasteTab::Servers), ("network", TasteTab::Library)] {
            let mut config = Config::default();
            config.gui.discover_tab = Some(word.into());
            let gui = Gui::new(config, false, App::new(None, None, None));
            assert_eq!(gui.app.taste.tab, tab, "{word:?}");
        }
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        gui.app.capabilities.federation_discovery = false;
        draw(&mut gui);
        assert_eq!(gui.app.taste.tab, TasteTab::Library, "a tab whose source is gone falls back to Library");
    }

    #[test]
    fn the_pick_goes_home_to_the_room() {
        let _en = english();
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Choose));
        assert_eq!(gui.active, FILES_NAV, "the pick happens in the library");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Pick a song to start from"), "the banner:\n{all}");
        key(&mut gui, KeyCode::Esc);
        assert_eq!((gui.active, gui.app.capture), (DISCOVER_NAV, None), "Esc goes home");
    }

    #[test]
    fn the_now_playing_tab_reads_similar() {
        let _en = english();
        let mut gui = session_gui();
        gui.act(Act::Screen(Screen::NowPlaying));
        let all = draw_sized(&mut gui, 120, 34).join("\n");
        assert!(all.contains(":Similar") && !all.contains(":Discover"), "the per-song lens is Similar now:\n{all}");
    }

    /// Frames for eyeballing: `cargo test dump_discover_frames -- --ignored
    /// --nocapture` (the shell's dump_* habit).
    #[test]
    #[ignore]
    fn dump_discover_frames() {
        let _en = english();
        let mut gui = answered_gui();
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        println!("{}", draw(&mut gui).join("\n"));
        gui.ui.pointer = None;
        gui.queue_open = true;
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        println!("{}", draw_sized(&mut gui, 100, 24).join("\n"));
        gui.queue_open = false;
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Down);
        println!("{}", draw(&mut gui).join("\n"));
        gui.queue_open = true;
        println!("{}", draw(&mut gui).join("\n"));
        gui.queue_open = false;
        key(&mut gui, KeyCode::Char('e'));
        println!("{}", draw(&mut gui).join("\n"));
    }

    // ── Pure pieces ─────────────────────────────────────────────────────

    #[test]
    fn ages_read_on_the_local_clock() {
        let _en = english();
        let day = 86_400;
        let now = 1_759_600_000; // an afternoon, UTC
        assert_eq!(age_words(now - 60, now, 0), "today");
        assert_eq!(age_words(now - day, now, 0), "yesterday");
        assert_eq!(age_words(now - 5 * day, now, 0), "5 days ago");
        assert_eq!(age_words(now - 40 * day, now, 0), "1 month ago");
        assert_eq!(age_words(now - 800 * day, now, 0), "2 years ago");
    }

    #[test]
    fn the_layout_keeps_a_table_row_at_the_floor() {
        // 100×24 with the footer on: 16 content rows.
        let content = Rect { x: 17, y: 2, width: 80, height: 16 };
        let plain = layout(content, false);
        assert_eq!((plain.tabs_y, plain.seeds_y, plain.table_y, plain.bottom_y), (5, 6, 8, 17));
        assert_eq!(plain.bottom_y - (plain.table_y + 2), 7, "seven rows with the footer on (clause 90)");
        let card = layout(content, true);
        assert_eq!(card.card.map(|(r, compact)| (r.height, compact)), Some((5, true)), "the card gives up its body");
        assert!(card.bottom_y > card.table_y + 2, "the table keeps a row: {card:?}");
        let tall = layout(Rect { height: 23, ..content }, true);
        assert_eq!(tall.card.map(|(r, compact)| (r.height, compact)), Some((7, false)));
    }

    #[test]
    fn narrow_tables_drop_album_then_on_then_artist() {
        let wants = Wants { artist: 20, album: 20, on: Some(12) };
        // The room's 80 cells (78 of list): every column, TRACK the widest.
        let wide = columns(Rect { x: 17, y: 0, width: 78, height: 1 }, wants, 6);
        assert!(wide.artist.is_some() && wide.album.is_some() && wide.on.is_some(), "{wide:?}");
        assert!(wide.track_w >= super::TRACK_MIN, "{wide:?}");
        assert_eq!(wide.len_x + wide.len_w, 17 + 78, "LENGTH flush right");
        let (on_x, on_w) = wide.on.unwrap();
        assert!(on_x + on_w + super::GAP <= wide.len_x, "ON stops a gap short of LENGTH: {wide:?}");
        // The queue open (45 of list): ALBUM goes, then ON; ARTIST stays.
        let narrow = columns(Rect { x: 17, y: 0, width: 45, height: 1 }, wants, 6);
        assert_eq!((narrow.album, narrow.on), (None, None), "{narrow:?}");
        assert!(narrow.artist.is_some() && narrow.track_w >= super::TRACK_MIN, "{narrow:?}");
        // The library at the same width loses ALBUM alone.
        let library = columns(Rect { x: 17, y: 0, width: 45, height: 1 }, Wants { on: None, ..wants }, 6);
        assert!(library.album.is_none() && library.artist.is_some(), "{library:?}");
        // Narrower still: TRACK alone, LENGTH never goes.
        let tiny = columns(Rect { x: 17, y: 0, width: 32, height: 1 }, wants, 6);
        assert_eq!((tiny.artist, tiny.album, tiny.on), (None, None, None), "{tiny:?}");
        assert_eq!(tiny.len_x + tiny.len_w, 17 + 32);
        // ARTIST and ALBUM take their share whatever their words: TRACK half
        // of what is left with both, the columns where the eye expects them.
        let short = columns(Rect { x: 17, y: 0, width: 78, height: 1 }, Wants { artist: 6, album: 7, on: None }, 6);
        assert_eq!((short.artist.map(|(_, w)| w), short.album.map(|(_, w)| w)), (Some(16), Some(16)), "{short:?}");
        assert_eq!(short.track_w, 32);
    }

    #[test]
    fn the_lines_put_albums_first_and_show_only_what_plays_now() {
        let mut app = App::new(None, None, None);
        app.taste.library.data = main_answer().library;
        app.taste.peers.data = peers_answer().peers;
        let library = lines(&app, TasteTab::Library);
        assert_eq!(library[..2], [Line::Album(0), Line::Song(0)]);
        let servers = lines(&app, TasteTab::Servers);
        assert_eq!(servers, vec![Line::Peer { group: 0, row: 0 }, Line::Peer { group: 0, row: 1 }], "no unreachable, no mismatched");
    }

    /// The tests that read another language's words: the lock, then
    /// `code`, and English again when the guard drops.
    struct Locale(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Drop for Locale {
        fn drop(&mut self) {
            rust_i18n::set_locale("en");
        }
    }

    fn locale(code: &str) -> Locale {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale(code);
        Locale(guard)
    }

    /// The cell a glyph stands in, by the buffer (a wide glyph's second
    /// cell joins a row's string as nothing, so a string index lies).
    fn cell_of(buffer: &ratatui::buffer::Buffer, symbol: &str) -> Position {
        let area = *buffer.area();
        for y in 0..area.height {
            for x in 0..area.width {
                if buffer[(x, y)].symbol() == symbol {
                    return Position { x, y };
                }
            }
        }
        panic!("{symbol:?} is not on screen");
    }

    #[test]
    fn the_headings_stand_over_their_columns_in_french() {
        let _fr = locale("fr");
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        let header = &rows[8];
        for head in ["TITRE", "ARTISTE", "ALBUM", "DURÉE"] {
            assert!(header.contains(head), "{head}:\n{header}");
        }
        assert_eq!(at(&rows, "Aurora").x, at(&rows, "TITRE").x, "the title under its heading:\n{}", rows.join("\n"));
        assert_eq!(at(&rows, "Ray").x, at(&rows, "ARTISTE").x, "the artist under its heading");
        assert!(rows[24].contains("[ ] Inclure les morceaux écoutés récemment"), "{}", rows[24]);
    }

    #[test]
    fn the_columns_are_laid_out_in_cells_in_japanese() {
        let _ja = locale("ja");
        let mut gui = answered_gui();
        let buffer = draw_buffer(&mut gui);
        // The header row: 曲 (two cells), then アーティスト — six glyphs, twelve
        // cells. The artist's first letter stands under the heading's first
        // cell, and the album's under アルバム's, whatever the glyph widths.
        let find = |y: u16, symbol: &str| (17..100).find(|x| buffer[(*x, y)].symbol() == symbol).expect(symbol);
        assert_eq!(find(11, "R"), find(8, "ア"), "ARTIST in cells, not chars");
        let album = (find(8, "ア") + 1..100).find(|x| buffer[(*x, 8)].symbol() == "ア").expect("アルバム");
        assert_eq!(find(11, "D"), album, "ALBUM in cells, not chars");
    }

    #[test]
    fn the_cards_buttons_are_laid_out_in_cells_in_japanese() {
        let _ja = locale("ja");
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        land(&mut gui, TastePart::Main, fallback_answer());
        let buffer = draw_buffer(&mut gui);
        // 別の曲 is three glyphs, six cells: the click covers all six, then
        // three cells of gap, then 曲を選ぶ….
        let another = cell_of(&buffer, "別");
        let cell = |dx: u16| Position { x: another.x + dx, y: another.y };
        assert_eq!(gui.ui.hit(cell(5)), Some(Act::Disc(DiscAct::Another)), "the label's last cell is the button");
        assert!(!matches!(gui.ui.hit(cell(6)), Some(Act::Disc(_))), "the gap is no button");
        assert_eq!(buffer[cell(9)].symbol(), "曲", "choose stands after the gap");
        assert_eq!(gui.ui.hit(cell(9)), Some(Act::Disc(DiscAct::Choose)));
    }

    #[test]
    fn the_cards_title_wraps_rather_than_clips_with_the_queue_open() {
        let _en = english();
        let mut gui = session_gui();
        gui.queue_open = true;
        key(&mut gui, KeyCode::Char('G'));
        land(&mut gui, TastePart::Main, fallback_answer());
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        let first = at(&rows, "Finish a few songs");
        assert!(rows[first.y as usize + 1].contains("taste"), "the lead sentence's end on the next row:\n{all}");
        assert!(!rows[first.y as usize].contains('…'), "not clipped:\n{all}");
        assert!(all.contains("another pick"), "{all}");
    }

    #[test]
    fn the_glyph_fallbacks_on_the_legacy_console() {
        let _en = english();
        let mut gui = answered_gui();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].trim_end().ends_with("Last 30 days v"), "the dropdown's caret:\n{}", rows[2]);
        assert!(rows[11].contains("[>] [^] [+] [.]"), "the hover verbs:\n{}", rows[11]);
        assert!(rows[13].contains(" - "), "the missing artist:\n{}", rows[13]);
        gui.act(Act::Disc(DiscAct::Played));
        let rows = draw(&mut gui);
        assert!(rows[24].contains("[x] Include songs"), "the checkmark's stand-in:\n{}", rows[24]);
        for glyph in ['▾', '▸', '»', '⋯', '▇', '—', '–', '✓'] {
            assert!(!all.contains(glyph), "{glyph:?} on the legacy console:\n{all}");
        }
    }

    #[test]
    fn a_pointer_act_stows_the_cursor_and_a_key_keeps_it() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.cursor, Some(1));
        // `[⋯]` or a right click on another row.
        gui.act(Act::Disc(DiscAct::More(2)));
        assert_eq!(gui.discover.cursor, None, "a click acts on its row and stows (clause 57)");
        // The seeds modal: a row click stows its cursor, Space keeps it.
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Seeds));
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.seeds.as_ref().is_some_and(|m| m.view.held));
        key(&mut gui, KeyCode::Char(' '));
        assert!(gui.discover.seeds.as_ref().is_some_and(|m| m.view.held), "Space keeps the hand up");
        gui.act(Act::Disc(DiscAct::SeedRow(1)));
        assert!(gui.discover.seeds.as_ref().is_some_and(|m| !m.view.held), "a click puts it down");
    }

    #[test]
    fn the_tips_name_only_what_works_on_the_row() {
        let _en = english();
        let mut gui = answered_gui();
        assert!(super::tips(&gui).contains("f played lately"), "the checkbox's key: {}", super::tips(&gui));
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), "Enter open the album · a queue · Esc done", "an album row");
        key(&mut gui, KeyCode::Down);
        assert!(super::tips(&gui).contains("m actions"), "a song row");
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        key(&mut gui, KeyCode::Down);
        let tips = super::tips(&gui);
        assert!(tips.contains("P play now") && tips.contains("m actions"), "a connected server's row has the sheet: {tips}");
        key(&mut gui, KeyCode::Char('w'));
        assert_eq!(super::tips(&gui), "↑ ↓ rows · Enter pick · Esc close");
    }
}
