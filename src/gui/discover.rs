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
//! ROWS ahead of the songs at the floor — the contract's log says why. Since
//! 2026-10-14 the page is simple (the contract's log): every visit asks, the
//! server draws a new random batch of seeds each time and keeps nothing; the
//! window is remembered in `[gui] discover_window`, and pins and drops last
//! for the visit.
//!
//! One answer, two tabs, top to bottom (clause 90): the header with the
//! window dropdown, the state line, the fallback card while nothing is
//! finished yet, the tabs, the seeds line, the tab's album strip where the
//! room is tall enough (clauses 45–49, the 2026-10-11 strip log: the
//! webapp's horizontal scroller of album cards, covers as real pixels
//! through `cover::Slot` the way the queue panel's are), the verbs line
//! with the tab's whole-list verbs ahead of its songs (clause 64, the
//! 2026-10-16 log: the webapp's verbs under its songs' header), the tab's
//! table down to the room's last row. The seeds modal and the window's
//! list draw last, from [`draw_modals`], so they win the pointer; the save
//! prompt is the sonic room's (`sonic::draw_modals`).

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{SeedSource, TasteOutcome, Track};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, frame_at, modal_close, modal_frame_on, scroll_list, width};
use crate::tui::app::taste::{TASTE_STRIP_MAX, TastePeerAlbum};
use crate::tui::app::{App, Capture, Origin, TasteLatch, TastePinRefusal, TasteRow, TasteSlot, TasteTab, TasteVerb};

use super::cover::{Pace, Slot};
use super::{Act, Gui, List, RowVerb, Screen, accent, bright_bold, bullet_glyph, check_glyphs, put, sel};

/// The window dropdown's choices, in days (0 = all time) — clause 15.
use crate::tui::app::taste::TASTE_WINDOWS as WINDOWS;

/// The window the header shows before any was chosen or answered: the
/// server's own default (plan §2.2).
use crate::tui::app::taste::TASTE_WINDOW_DEFAULT as WINDOW_DEFAULT;

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

/// The album strip's tile (clause 45): a cover 12 cells by 5 rows, the
/// album's title and its artist under it, one cell between tiles.
const TILE_W: u16 = 12;
const TILE_COVER_H: u16 = 5;
const TILE_H: u16 = TILE_COVER_H + 2;
const TILE_STRIDE: u16 = TILE_W + 1;
/// The rows the strip takes from the table: its tiles and the gap under
/// them.
const STRIP_ROWS: u16 = TILE_H + 1;
/// The strip stands only where the table keeps at least this many rows
/// under it (clause 49): with the room's six rows above the strip and the
/// verbs line, table header and rule under it, a content area of 20 rows —
/// the installer's 100×30 window has 22 or 23 — and never the 24-row
/// floor's 16 or 17, which keep the album rows.
const STRIP_TABLE_MIN: u16 = 3;
/// The skeleton tiles the first ask draws, the webapp's four cards.
const STRIP_SKELETON: u16 = 4;
/// Covers kept encoded past the strip's view, so a scroll back or a tab
/// switch back finds them warm — both tabs' whole strips.
const STRIP_SLACK: usize = 2 * TASTE_STRIP_MAX;

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
    /// The album strip's view and keyboard hand (clauses 45–49).
    pub strip: StripUi,
    /// The room's content as last drawn: whether the strip stands is the
    /// layout's word for that rect, and the keys walk what the frame showed.
    content: Rect,
    /// The strip's tiles as last drawn — what the wheel scrolls.
    strip_at: Option<Rect>,
    /// The tab in front and whether its strip stood, last frame: a change
    /// on the same tab re-indexes the table under the cursor.
    stood: Option<(TasteTab, bool)>,
    /// A slot per strip cover on view, keyed by the art's id — the queue
    /// panel's pool: an encoded picture draws anywhere for free, so a
    /// scroll moves each cover with its tile and only a cover newly
    /// revealed pays an encode.
    slots: HashMap<u64, Slot>,
}

/// The album strip's own state (clauses 46–47).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StripUi {
    /// The first tile shown — the strip's horizontal scroll.
    pub first: usize,
    /// The keyboard's tile, an index into the tab's [`tiles`].
    pub cursor: usize,
    /// The keyboard's hand is on the strip (lit only then — the list-cursor
    /// law); the table's cursor is stowed meanwhile.
    pub on: bool,
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
            strip: StripUi::default(),
            content: Rect::default(),
            strip_at: None,
            stood: None,
            slots: HashMap::new(),
        }
    }

    /// The keyboard's hand down: no row lit, no tile lit (the kit's
    /// list-cursor law).
    fn stow(&mut self) {
        self.cursor = None;
        self.view.stow();
        self.strip.on = false;
    }

    /// A resize changes the cell-to-pixel mapping every slot encoded
    /// against — the wall's rule, per cover.
    pub(crate) fn on_resize(&mut self) {
        for slot in self.slots.values_mut() {
            slot.on_resize();
        }
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
    /// A strip tile, by its place in the tab's [`tiles`]: the tile itself
    /// (`Open`) or its `[+]` (`Queue`) — clause 48.
    Tile(usize, RowVerb),
    /// A song's `[⋯]` (the track-actions sheet), and a right click — a
    /// library song's, or a connected server's with its peer origin.
    More(usize),
    /// The header's window control, and a pick from its list.
    Window,
    WindowPick(u32),
    /// A click outside the open list closes and keeps (the Dropdown law).
    MenuClose,
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
    /// The verbs line's whole-list verbs (clause 64); save as playlist on
    /// the Library tab only.
    PlayAll,
    QueueAll,
    Shuffle,
    SavePlaylist,
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

/// The tab's table where the album strip stands: its albums left the table
/// for the strip (clause 45); at the floor the table keeps them as rows.
pub(crate) fn table_lines(app: &App, tab: TasteTab, strip: bool) -> Vec<Line> {
    lines(app, tab).into_iter().filter(|line| !(strip && matches!(line, Line::Album(_)))).collect()
}

/// One tile of the album strip (clause 45).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tile {
    /// `library.albums[i]`.
    Library(usize),
    /// [`crate::tui::app::taste::TasteState::peer_albums`]`[i]`.
    Peer(usize),
}

/// The tab's album strip, best match first, capped at twelve (clause 45).
pub(crate) fn tiles(app: &App, tab: TasteTab) -> Vec<Tile> {
    match tab {
        TasteTab::Library => app.taste.library_strip().into_iter().map(Tile::Library).collect(),
        TasteTab::Servers => (0..app.taste.peer_albums().len()).map(Tile::Peer).collect(),
    }
}

/// What the strip would draw on `tab`: the first ask's skeleton tiles, or
/// the tab's albums — `None` where the pane holds no rows (a sentence, a
/// failure, the switched-off line) or the answer carries no album.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StripFill {
    Skeleton,
    Tiles(Vec<Tile>),
}

fn strip_fill(app: &App, tab: TasteTab) -> Option<StripFill> {
    match pane(app, tab, lines(app, tab).is_empty()) {
        Pane::Loading => Some(StripFill::Skeleton),
        Pane::Rows { .. } => Some(tiles(app, tab)).filter(|t| !t.is_empty()).map(StripFill::Tiles),
        _ => None,
    }
}

/// The tiles the strip shows on the tab in front, in the content the last
/// frame laid out — empty where no strip of albums stands (clause 49).
fn strip_tiles(gui: &Gui) -> Vec<Tile> {
    match strip_fill(&gui.app, gui.app.taste.tab) {
        Some(StripFill::Tiles(tiles)) if layout(gui.discover.content, gui.app.taste.no_history, true).strip_y.is_some() => {
            tiles
        }
        _ => Vec::new(),
    }
}

/// The table's lines as the last frame drew them: the cursor, the keys and
/// the clicks index these.
fn rows(gui: &Gui) -> Vec<Line> {
    table_lines(&gui.app, gui.app.taste.tab, !strip_tiles(gui).is_empty())
}

/// How many tiles the strip shows whole in `width` cells — at its end, where
/// nothing follows them.
fn whole_tiles(width: u16) -> usize {
    usize::from(((width + 1) / TILE_STRIDE).max(1))
}

/// How many tiles the strip shows whole while more follow them (clause 47):
/// the last one shown must then be cut. Where the cells end on a tile's own
/// edge or its gap (`width` mod 13 is 12 or 0) nothing would be cut, so the
/// last whole tile gives its place up to a half tile.
fn whole_tiles_overflowing(width: u16) -> usize {
    let whole = whole_tiles(width);
    if cut_width(width, whole) == 0 && whole > 1 { whole - 1 } else { whole }
}

/// The cells left for a cut tile after `whole` tiles and their gaps, at most
/// a tile less a cell; a tile's worth (the edge case above) is cut to half.
fn cut_width(width: u16, whole: usize) -> u16 {
    let rest = width.saturating_sub(whole as u16 * TILE_STRIDE);
    if rest >= TILE_W { TILE_W / 2 } else { rest }
}

/// The strip's view from `first` of `n` tiles in `width` cells: the tiles
/// shown whole, and the cut tile's width after them (0: none) — a cut tile
/// stands exactly while more tiles follow the whole ones.
fn strip_view(width: u16, n: usize, first: usize) -> (usize, u16) {
    let left = n.saturating_sub(first);
    if left <= whole_tiles(width) {
        return (left, 0);
    }
    let whole = whole_tiles_overflowing(width);
    (whole, cut_width(width, whole))
}

/// The strip's scroll, never past the last whole view, and — while the
/// keyboard's hand is on the strip — wherever keeps its tile whole in view
/// (clause 47): a key, a narrowed room (the queue opening, a resize) or a
/// new answer never leaves the lit tile off screen.
fn keep_in_view(strip: &mut StripUi, width: u16, n: usize) {
    let last = n.saturating_sub(whole_tiles(width));
    strip.cursor = strip.cursor.min(n.saturating_sub(1));
    strip.first = strip.first.min(last);
    if !strip.on {
        return;
    }
    if strip.cursor < strip.first {
        strip.first = strip.cursor;
    } else if strip.cursor >= strip.first + strip_view(width, n, strip.first).0 {
        strip.first = (strip.cursor + 1).saturating_sub(whole_tiles_overflowing(width)).min(last);
    }
}

/// Whether a tab holds rows the whole-list verbs can play.
fn playable(app: &App, tab: TasteTab) -> bool {
    match tab {
        TasteTab::Library => app.taste.library.data.as_ref().is_some_and(|l| !l.tracks.is_empty()),
        TasteTab::Servers => !app.taste.peer_lines().is_empty(),
    }
}

/// Whether the tab's songs can be saved as a playlist (clause 64): the
/// Library tab's, while it has songs. A connected server's songs are not
/// this server's to save — the webapp's servers column has no save either.
fn saveable(app: &App, tab: TasteTab) -> bool {
    tab == TasteTab::Library && playable(app, tab)
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
    /// The album strip's first row, where it stands (clause 49).
    pub strip_y: Option<u16>,
    /// The verbs line (clause 64): under the strip where it stands, else
    /// under the seeds line's blank — always right over the table.
    pub verbs_y: u16,
    /// The table's header row; its rule is the next, its rows after that,
    /// down to the room's last row.
    pub table_y: u16,
}

/// `strip`: the tab has a strip to draw. It stands where the table keeps
/// [`STRIP_TABLE_MIN`] rows under it — a content area of 20 rows with no
/// card (clause 49) — and gives way to the card, which comes first.
pub(crate) fn layout(content: Rect, card: bool, strip: bool) -> Layout {
    let y = content.y;
    let mut tabs_y = y + 3;
    let mut card_at = None;
    if card {
        // Header, state, blank · the card and its blank · tabs, seeds,
        // blank · the verbs line · the table's header, rule and one row.
        let whole = 3 + CARD_FULL + 1 + 3 + 1 + 3 <= content.height;
        let height = if whole { CARD_FULL } else { CARD_COMPACT };
        let width = content.width.min(CARD_W);
        let x = content.x + (content.width - width) / 2;
        card_at = Some((Rect { x, y: y + 3, width, height }, !whole));
        tabs_y = y + 3 + height + 1;
    }
    // The rows the table would hold without the strip: under the verbs
    // line, the table's header and rule, to the room's last row.
    let verbs_y = tabs_y + 3;
    let rows = content.bottom().saturating_sub(verbs_y + 3);
    let strip_y = (strip && rows >= STRIP_ROWS + STRIP_TABLE_MIN).then_some(verbs_y);
    let verbs_y = if strip_y.is_some() { verbs_y + STRIP_ROWS } else { verbs_y };
    Layout {
        header_y: y,
        state_y: y + 1,
        card: card_at,
        tabs_y,
        seeds_y: tabs_y + 1,
        strip_y,
        verbs_y,
        table_y: verbs_y + 1,
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
    gui.discover.content = content;
    let fill = strip_fill(&gui.app, gui.app.taste.tab);
    let lay = layout(content, gui.app.taste.no_history, fill.is_some());
    draw_header(frame, gui, content, lay.header_y);
    draw_state(frame, gui, content, lay.state_y);
    if let Some((rect, compact)) = lay.card {
        draw_card(frame, gui, rect, compact);
    }
    draw_tabs(frame, gui, content, lay.tabs_y);
    draw_seeds_line(frame, gui, content, lay.seeds_y);
    gui.discover.strip_at = None;
    // Whether tiles of albums stand this frame; when that changed on the
    // same tab (a resize across the height gate, the queue opening) the
    // keyboard's hand keeps the thing it was on (clause 49).
    let stands = lay.strip_y.is_some() && matches!(fill, Some(StripFill::Tiles(_)));
    let tab = gui.app.taste.tab;
    if let Some((was_tab, was)) = gui.discover.stood.replace((tab, stands))
        && was_tab == tab
        && was != stands
    {
        carry_hand(gui, tab, stands);
    }
    let strip = match (lay.strip_y, fill) {
        (Some(y), Some(fill)) => draw_strip(frame, gui, content, y, fill),
        _ => false,
    };
    // The keyboard's hand cannot rest on a strip that is not there (a tab
    // with no albums, a new answer).
    if !strip {
        gui.discover.strip.on = false;
    }
    draw_verbs(frame, gui, content, lay.verbs_y);
    draw_table(frame, gui, content, lay.table_y, strip);
}

/// The strip came (`stands`) or went on the same tab: the table's rows are
/// re-indexed (its album rows leave for the strip or come back), so the
/// keyboard's hand moves with what it was on — a song stays that song, an
/// album row becomes its tile and a library tile its row again; what has no
/// place on the other side (a connected server's tile at the floor) puts
/// the hand down rather than lighting something else.
fn carry_hand(gui: &mut Gui, tab: TasteTab, stands: bool) {
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    if !stands && gui.discover.strip.on {
        let tile = tiles(&gui.app, tab).get(gui.discover.strip.cursor).copied();
        let row = match tile {
            Some(Tile::Library(i)) => table_lines(&gui.app, tab, false).iter().position(|l| *l == Line::Album(i)),
            _ => None,
        };
        gui.discover.stow();
        if let Some(row) = row {
            gui.discover.cursor = Some(row);
            gui.discover.view.pick_up();
        }
        return;
    }
    let Some(at) = held else {
        // A cursor nobody holds indexes nothing worth keeping.
        if gui.discover.cursor.is_some() {
            gui.discover.stow();
        }
        return;
    };
    let line = table_lines(&gui.app, tab, !stands).get(at).copied();
    gui.discover.stow();
    match line {
        Some(Line::Album(i)) if stands => {
            if let Some(to) = tiles(&gui.app, tab).iter().position(|t| *t == Tile::Library(i)) {
                gui.discover.strip.on = true;
                gui.discover.strip.cursor = to;
                let (width, n) = (gui.discover.content.width, tiles(&gui.app, tab).len());
                keep_in_view(&mut gui.discover.strip, width, n);
            }
        }
        Some(line) => {
            if let Some(to) = table_lines(&gui.app, tab, stands).iter().position(|l| *l == line) {
                gui.discover.cursor = Some(to);
                gui.discover.view.pick_up();
            }
        }
        None => {}
    }
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

// ── The album strip ─────────────────────────────────────────────────────────

/// One tile's words and art file, owned so the drawing can borrow freely.
struct TileFace {
    title: String,
    artist: Option<String>,
    art: Option<String>,
    tip: String,
}

fn tile_face(app: &App, tile: Tile, peers: &[TastePeerAlbum]) -> Option<TileFace> {
    let nonempty = |s: Option<&str>| s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match tile {
        Tile::Library(i) => {
            let album = app.taste.library.data.as_ref()?.albums.get(i)?;
            Some(TileFace {
                title: nonempty(album.name.as_deref()).unwrap_or_else(|| dash().to_string()),
                artist: nonempty(album.artist()),
                art: nonempty(album.album_art_file.as_deref()),
                tip: t!("gui.discover.album_tip").to_string(),
            })
        }
        Tile::Peer(i) => {
            let album = peers.get(i)?;
            Some(TileFace {
                title: album.album.clone(),
                artist: album.artist.clone(),
                art: album.art.clone(),
                tip: t!("gui.discover.peer_album_tip", server = album.server).to_string(),
            })
        }
    }
}

/// The strip (clauses 45–48) at row `y`: the first ask's skeleton tiles, or
/// the tab's albums from the strip's scroll on, the last one cut at the
/// right edge when more follow. True when tiles of albums stand — what the
/// wheel and the keys may then walk.
fn draw_strip(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16, fill: StripFill) -> bool {
    let row = Rect { x: content.x, y, width: content.width, height: TILE_H };
    let tiles = match fill {
        StripFill::Skeleton => {
            draw_strip_skeleton(frame, row);
            return false;
        }
        StripFill::Tiles(tiles) => tiles,
    };
    gui.discover.strip_at = Some(row);
    let n = tiles.len();
    // A new answer can leave the scroll or the hand past the tiles, and a
    // narrowed room the hand's tile out of view; the scroll never leaves
    // blank cells where more tiles could stand.
    keep_in_view(&mut gui.discover.strip, row.width, n);
    let first = gui.discover.strip.first;
    let (whole, cut_w) = strip_view(row.width, n, first);
    // Every tile that shows, whole or cut.
    let shown = whole + usize::from(cut_w > 0);
    let peers = if gui.app.taste.tab == TasteTab::Servers { gui.app.taste.peer_albums() } else { Vec::new() };

    // The covers these tiles still owe the cache, claimed through the App's
    // own fetch — a library album's from the session, a connected server's
    // through the parent's art proxy; hashmap lookups after the first frame.
    let mut fetches = Vec::new();
    for tile in &tiles[first..first + shown] {
        let effect = match *tile {
            Tile::Library(i) => {
                let file = gui.app.taste.library.data.as_ref().and_then(|l| l.albums.get(i)).and_then(|a| a.album_art_file.clone());
                file.filter(|f| !f.is_empty()).and_then(|f| gui.app.fetch_art_file(&f))
            }
            Tile::Peer(i) => peers.get(i).and_then(|album| gui.app.taste_peer_album_art(album)),
        };
        fetches.extend(effect);
    }
    gui.pend(fetches);

    // Split borrows: the faces and covers are READ from the App while each
    // slot's caches are written — the queue panel's move, so a frame never
    // clones a decoded cover.
    let pace = Pace::frame();
    let mut on_view: HashSet<u64> = HashSet::new();
    let [_, _, plus, _] = verb_glyphs();
    {
        let Gui { app, discover, ui, .. } = &mut *gui;
        let StripUi { cursor, on, .. } = discover.strip;
        for (k, index) in (first..first + shown).enumerate() {
            let x = row.x + k as u16 * TILE_STRIDE;
            let w = if k < whole { TILE_W } else { cut_w };
            let cut = w < TILE_W;
            let Some(face) = tile_face(app, tiles[index], &peers) else { continue };
            // The cover: real pixels where the terminal draws them, the
            // ▀-mosaic where it does not, where an overlay stood last frame
            // (the kit's Over pixels) or where the tile is cut — a picture
            // cannot be cut; the note tile where the album has no art.
            let size = Rect { x, y, width: TILE_W, height: TILE_COVER_H };
            let cover = Rect { width: w, ..size };
            let art = face.art.as_deref().and_then(|file| app.art.get(file)).and_then(|art| art.as_ref());
            match art {
                Some(art) => {
                    let slot = discover.slots.entry(art.id()).or_insert_with(|| Slot::new(app.graphics.fork()));
                    on_view.insert(art.id());
                    if cut {
                        slot.draw_mosaic_cut(frame, size, cover, art);
                    } else if ui.covered_last_frame(cover) {
                        slot.draw_mosaic(frame, cover, art);
                    } else {
                        slot.draw_paced(frame, cover, art, &pace);
                    }
                }
                None => note_tile(frame, cover),
            }
            // The title (BOLD and bright under the pointer, the slab under
            // the keyboard's hand) and the artist DIM; a hovered whole tile
            // gives its title's tail to its one verb, `[+]`.
            let tile = Rect { x, y, width: w, height: TILE_H };
            let selected = on && cursor == index;
            let hover = ui.hovers(tile);
            let verb = hover && !selected && !cut;
            let name_y = y + TILE_COVER_H;
            let name = Rect { x, y: name_y, width: w, height: 1 };
            if selected {
                frame.render_widget(ratatui::widgets::Block::default().style(sel()), name);
            }
            let title_style = match (selected, hover) {
                (true, _) => sel().add_modifier(Modifier::BOLD),
                (false, true) => bright_bold(),
                (false, false) => Style::default(),
            };
            let room = if verb { TILE_W - 4 } else { w };
            let title = super::bar::clip(&face.title, usize::from(room)).into_owned();
            put(frame, x, name_y, &title, title_style);
            // The artist stays DIM on the ground under the hand too: the slab
            // is the title line's alone (clause 47), never a ragged second
            // run of accent under it.
            if let Some(artist) = &face.artist {
                put(frame, x, name_y + 1, &super::bar::clip(artist, usize::from(w)), dim());
            }
            // The tile's click first, its verb after: the last registered wins.
            ui.click(tile, Act::Disc(DiscAct::Tile(index, RowVerb::Open)));
            ui.tip_keyed(Rect { width: (width(&title) as u16).max(1), ..name }, face.tip);
            if verb {
                let cell = Rect { x: x + TILE_W - 3, y: name_y, width: 3, height: 1 };
                put(frame, cell.x, name_y, plus, if ui.hovers(cell) { bright_bold() } else { dim() });
                ui.click(cell, Act::Disc(DiscAct::Tile(index, RowVerb::Queue)));
                ui.tip_keyed(cell, t!("gui.discover.album_queue_tip").to_string());
            }
        }
    }
    // Slots for covers that left the view are let go once the pool outgrows
    // both tabs' strips — a scroll or a tab switch back finds them warm.
    if gui.discover.slots.len() > on_view.len() + STRIP_SLACK {
        gui.discover.slots.retain(|id, _| on_view.contains(id));
    }
    // Covers the budget turned away want the very next frame.
    if pace.starved() {
        gui.hot = true;
    }
    true
}

/// The house placeholder for an album with no art (the webapp's note
/// tile): the empty cover slot's DIM rounded frame with the note in it,
/// drawn whole and shown in `cover` — cut at the right edge like any tile.
fn note_tile(frame: &mut Frame, cover: Rect) {
    let size = Rect { x: 0, y: 0, width: TILE_W, height: TILE_COVER_H };
    let mut tile = Buffer::empty(size);
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(dim());
    ratatui::widgets::Widget::render(block, size, &mut tile);
    // No note on the legacy console, which draws it as a question mark.
    if !legacy_conhost() {
        tile[((TILE_W - 1) / 2, TILE_COVER_H / 2)].set_symbol("♪").set_style(dim());
    }
    // Glyph and ink only: the frame's own ground stays under them (the owned
    // ground the shell painted), never the scratch buffer's Reset.
    let area = frame.area();
    let buffer = frame.buffer_mut();
    for dy in 0..cover.height.min(TILE_COVER_H) {
        for dx in 0..cover.width.min(TILE_W) {
            let (x, y) = (cover.x + dx, cover.y + dy);
            if x < area.right() && y < area.bottom() {
                let from = &tile[(dx, dy)];
                let cell = &mut buffer[(x, y)];
                cell.set_symbol(from.symbol()).set_fg(from.fg);
                cell.modifier = from.modifier;
            }
        }
    }
}

/// The first ask's skeleton tiles (clause 32's, for the strip): the empty
/// cover frame and DIM runs where the title and the artist will stand —
/// whole tiles only.
fn draw_strip_skeleton(frame: &mut Frame, row: Rect) {
    let glyph = if legacy_conhost() { "·" } else { "░" };
    for k in 0..STRIP_SKELETON {
        let x = row.x + k * TILE_STRIDE;
        if x + TILE_W > row.right() {
            break;
        }
        super::bar::cover_slot(frame, x, row.y, TILE_W, TILE_COVER_H);
        put(frame, x, row.y + TILE_COVER_H, &glyph.repeat(9 - usize::from(k % 2) * 2), dim());
        put(frame, x, row.y + TILE_COVER_H + 1, &glyph.repeat(6 + usize::from(k % 2)), dim());
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
    /// The library answered nothing new: the sentence alone (clause 80).
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
/// and the kit scrollbar while the rows overflow. `strip`: the album strip
/// stands above it and holds the albums (clause 45).
fn draw_table(frame: &mut Frame, gui: &mut Gui, content: Rect, table_y: u16, strip: bool) {
    let tab = gui.app.taste.tab;
    let list = Rect {
        x: content.x,
        y: table_y + 2,
        width: content.width.saturating_sub(2),
        height: content.bottom().saturating_sub(table_y + 2),
    };
    let all = table_lines(&gui.app, tab, strip);
    // An answer of albums and no songs: the strip holds all of it, and an
    // empty table (its header, its rule, nothing under them) would read as
    // something missing — no table stands under the strip then.
    let pane_now = pane(&gui.app, tab, lines(&gui.app, tab).is_empty());
    if strip && all.is_empty() && matches!(pane_now, Pane::Rows { failed_over: None }) {
        return;
    }
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
    match pane_now {
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
            sentence_lines(frame, list, &t!("gui.discover.empty_library"), dim());
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

/// The verbs line (clause 64, the webapp's whole-list verbs under its
/// songs' header since mStream `141e1a9c`): play · queue all · shuffle, and
/// save as playlist on the Library tab, from the room's left edge right
/// over the table, on a tab that has something to play — the browse bar's
/// grammar: play the one accent, the rest DIM text buttons, the keys in
/// their tooltips, dropped from the tail when squeezed. Hidden, never grey,
/// while the tab has no rows; the line keeps its row so nothing moves.
fn draw_verbs(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let tab = gui.app.taste.tab;
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
    if saveable(&gui.app, tab) {
        verbs.push((t!("gui.discover.save_playlist").to_string(), DiscAct::SavePlaylist, "s"));
    }
    let sep = " · ";
    let width_of = |verbs: &[(String, DiscAct, &str)]| -> u16 {
        let labels: u16 = verbs.iter().map(|(l, _, _)| width(l) as u16).sum();
        labels + width(sep) as u16 * verbs.len().saturating_sub(1) as u16
    };
    while verbs.len() > 1 && content.width.saturating_sub(1) < width_of(&verbs) {
        verbs.pop();
    }
    let mut x = content.x;
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
        // This visit's drops the answer resolved (its `excluded`), never a
        // random pick passed over with "another pick" (clause 21).
        let dropped = taste.dropped_seeds();
        if !dropped.is_empty() {
            out.push(SeedLine::Head(t!("gui.discover.sec_dropped").to_uppercase()));
            out.extend(dropped.iter().map(|e| SeedLine::Dropped(e.hash.clone())));
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
                (None, track_words(&pin.track), t!("gui.discover.seed_pinned").to_string())
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

/// The room is entered — every visit asks (clause 3): a fresh one lets the
/// last visit's pins and drops go; `again` (the nav row or `G` while the
/// room is up, or the way back from a pick) keeps them. Either way the room
/// opens with nothing hung, and the old answer stands until the new lands.
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
        // Only the pointer sends this (the keys call `tile_act` and keep
        // the hand on the strip): a click acts on its tile and stows.
        DiscAct::Tile(index, verb) => {
            gui.discover.stow();
            tile_act(gui, index, verb);
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
            pick_window(gui, window);
        }
        DiscAct::MenuClose => gui.discover.menu = None,
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
        // The house name prompt, the sonic room's (`sonic::draw_modals`
        // draws it, `sonic::handle_key` types into it): the menu folds so
        // one modal stands.
        DiscAct::SavePlaylist => {
            if gui.app.taste_save_playlist() {
                gui.discover.menu = None;
            }
        }
    }
    true
}

/// A window picked (clause 15): asked at once, and remembered in `[gui]
/// discover_window` — the server keeps nothing between calls.
fn pick_window(gui: &mut Gui, window: u32) {
    let effects = gui.app.set_taste_window(window);
    gui.pend(effects);
    if gui.config.gui.discover_window != Some(window) {
        gui.config.gui.discover_window = Some(window);
        gui.save_now();
    }
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
    gui.discover.strip = StripUi::default();
    gui.config.gui.discover_tab = Some(tab.as_str().to_string());
    gui.save_now();
}

/// A row's verb (clauses 60–62): library songs and connected servers' rows
/// through the App's own verbs — a connected server's through your own
/// server, the Listen verb's way — and an album into the Albums room, or
/// onto the queue.
fn row_act(gui: &mut Gui, index: usize, verb: RowVerb) {
    let line = rows(gui).get(index).copied();
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

/// A strip tile's verb (clause 48): a library album opens in the Albums
/// room as its row did (clause 61) and its `[+]` queues it by name; a
/// connected server's album plays (it has no drill here to open into) and
/// its `[+]` queues it, its songs asked of that server through the parent's
/// federation proxy.
fn tile_act(gui: &mut Gui, index: usize, verb: RowVerb) {
    let play = verb != RowVerb::Queue;
    match tiles(&gui.app, gui.app.taste.tab).get(index).copied() {
        Some(Tile::Library(i)) if play => open_album(gui, i),
        Some(Tile::Library(i)) => {
            let effects = gui.app.taste_queue_album(i);
            gui.pend(effects);
        }
        Some(Tile::Peer(i)) => {
            let effects = gui.app.taste_peer_album(i, play);
            gui.pend(effects);
        }
        None => {}
    }
}

/// The strip's scroll by `step` tiles, never past the last whole view
/// (clause 47).
fn scroll_strip(gui: &mut Gui, step: i64) {
    let n = strip_tiles(gui).len();
    let whole = whole_tiles(gui.discover.content.width);
    let last = n.saturating_sub(whole) as i64;
    let strip = &mut gui.discover.strip;
    strip.first = (strip.first as i64 + step).clamp(0, last) as usize;
}

/// The keyboard's tile moves to `to` and the strip scrolls to keep it whole
/// in view (clause 47).
fn strip_to(gui: &mut Gui, to: usize) {
    let n = strip_tiles(gui).len();
    let width = gui.discover.content.width;
    let strip = &mut gui.discover.strip;
    strip.cursor = to;
    keep_in_view(strip, width, n);
}

/// `[⋯]`, `m` or a right click on a song: the track-actions sheet — a
/// connected server's row with its peer origin, so the sheet offers what a
/// peer's track can do, as for a queued peer row (clause 62).
fn more(gui: &mut Gui, index: usize) {
    let (origin, track) = match rows(gui).get(index).copied() {
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

/// The modal closes: the visit's drops (and "use again") change the
/// `exclude` the asks carry, pins came or went — then one ask; nothing
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
    if gui.discover.menu.is_some() {
        return;
    }
    // Over the strip, one tile a notch (clause 47).
    let over_strip = gui.discover.strip_at.zip(gui.ui.pointer).is_some_and(|(at, p)| at.contains(p));
    if over_strip && !strip_tiles(gui).is_empty() {
        scroll_strip(gui, i64::from(delta.signum()));
        return;
    }
    gui.act(Act::ScrollBy(List::Discover, delta));
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
    let all = rows(gui);
    let tiles = strip_tiles(gui);
    if tiles.is_empty() {
        gui.discover.strip.on = false;
    }
    if gui.discover.strip.on
        && let Some(quit) = strip_key(gui, key, tiles.len(), all.len())
    {
        return Some(quit);
    }
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    match key.code {
        // The strip is the room's top stop (clause 47): a stowed ↓ or
        // PageDown lands on its first tile in view, ↑ or PageUp from the
        // table's first row goes back to the tile it left — as do they with
        // no rows under the strip.
        KeyCode::Down | KeyCode::PageDown if held.is_none() && !tiles.is_empty() => {
            let first = gui.discover.strip.first;
            to_strip(gui, first);
        }
        KeyCode::Up | KeyCode::PageUp if !tiles.is_empty() && (held == Some(0) || (held.is_none() && all.is_empty())) => {
            let back = gui.discover.strip.cursor.min(tiles.len() - 1);
            to_strip(gui, back);
        }
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
                pick_window(gui, WINDOWS[next]);
            }
        }
        KeyCode::Char('e') => return Some(gui.act(Act::Disc(DiscAct::Seeds))),
        // Another pick, only while the card stands (clause 21); otherwise
        // `r` stays the transport's repeat.
        KeyCode::Char('r') if gui.app.taste.no_history => return Some(gui.act(Act::Disc(DiscAct::Another))),
        // The whole-list verbs, where the tab has something to play (clause
        // 64) — `s` saves where the tab's songs are saveable, the sonic
        // room's save key; otherwise the keys keep their global meanings.
        KeyCode::Char('p') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::PlayAll))),
        KeyCode::Char('A') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::QueueAll))),
        KeyCode::Char('S') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::Shuffle))),
        KeyCode::Char('s') if saveable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::SavePlaylist))),
        _ => return None,
    }
    Some(false)
}

/// The keyboard's hand onto the strip's tile `to`, scrolled into view; the
/// table's cursor goes down (one hand, one lit thing).
fn to_strip(gui: &mut Gui, to: usize) {
    gui.discover.cursor = None;
    gui.discover.view.stow();
    gui.discover.strip.on = true;
    strip_to(gui, to);
}

/// A key while the hand is on the strip (clauses 47–48): ← → walk the
/// tiles, ↓ goes to the table's first row, Enter opens (or plays) the tile,
/// `+` and `a` queue it; `None` passes the key on to the room's own keys.
fn strip_key(gui: &mut Gui, key: KeyEvent, tiles: usize, rows: usize) -> Option<bool> {
    let index = gui.discover.strip.cursor.min(tiles - 1);
    match key.code {
        KeyCode::Left => strip_to(gui, index.saturating_sub(1)),
        KeyCode::Right => strip_to(gui, (index + 1).min(tiles - 1)),
        KeyCode::Down | KeyCode::PageDown => {
            if rows > 0 {
                gui.discover.strip.on = false;
                gui.discover.cursor = Some(0);
                gui.discover.view.pick_up();
            }
        }
        // The strip is the top: nothing above it to walk to.
        KeyCode::Up | KeyCode::PageUp => {}
        KeyCode::Esc => gui.discover.stow(),
        // Acted by key: the hand stays on the tile.
        KeyCode::Enter => tile_act(gui, index, RowVerb::Open),
        KeyCode::Char('a' | '+') => tile_act(gui, index, RowVerb::Queue),
        // A tile's one verb is its queue (clause 48).
        KeyCode::Char('N' | 'P' | 'm') => {}
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
    // The strip's tiles (clause 47): ← → walk them here, and switch tabs
    // everywhere else — the tips say which.
    if gui.discover.strip.on {
        let tiles = strip_tiles(gui);
        match tiles.get(gui.discover.strip.cursor.min(tiles.len().saturating_sub(1))) {
            Some(Tile::Library(_)) => return t!("gui.tips.discover_tile").to_string(),
            Some(Tile::Peer(_)) => return t!("gui.tips.discover_peer_tile").to_string(),
            None => {}
        }
    }
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    match held.and_then(|i| rows(gui).get(i).copied()) {
        // Only what works on THIS row (the kit's tips law): an album opens
        // or queues whole (clause 61); a connected server's row has every
        // song verb (clause 62).
        Some(Line::Album(_)) => t!("gui.tips.discover_album").to_string(),
        Some(Line::Song(_) | Line::Peer { .. }) => t!("gui.tips.discover_row").to_string(),
        // At rest: the fallback card's own tip, or over a strip ↓ lands on
        // its albums, not a row; `s` is named wherever the tab's songs can
        // be saved, the fallback card's included (clause 64).
        None => {
            let rest = if gui.app.taste.no_history {
                t!("gui.tips.discover_fallback")
            } else if strip_tiles(gui).is_empty() {
                t!("gui.tips.discover")
            } else {
                t!("gui.tips.discover_strip")
            };
            if saveable(&gui.app, gui.app.taste.tab) {
                format!("{rest} · {}", t!("gui.tips.discover_save"))
            } else {
                rest.to_string()
            }
        }
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
    use crate::api::types::{Capabilities, TasteResponse};
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
        session_gui_with(Config::default())
    }

    /// [`session_gui`] over a given config (a remembered window or tab).
    fn session_gui_with(config: Config) -> Gui {
        let mut gui = Gui::new(config, false, App::new(Some(HOST.into()), None, None));
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

    /// Land the last Main ask's answer, the pended effects cleared first:
    /// what is pended after is what the answer sent — the peers ask
    /// (clause 30), for the next [`land`] of `Peers`.
    fn land_main(gui: &mut Gui, answer: TasteResponse) {
        let (seq, generation) = ask_of(gui, TastePart::Main);
        gui.pending.clear();
        let effects = gui.app.apply_event(Event::Taste { seq, generation, part: TastePart::Main, answer: Ok(Box::new(answer)) });
        gui.pend(effects);
    }

    fn fail(gui: &mut Gui, part: TastePart, failure: TasteFailure) {
        let (seq, generation) = ask_of(gui, part);
        let effects = gui.app.apply_event(Event::Taste { seq, generation, part, answer: Err(failure) });
        gui.pend(effects);
    }

    /// The asks a set of pended effects carries: part, sources, window.
    fn asks(gui: &Gui) -> Vec<(TastePart, Vec<String>, Option<u32>)> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Taste { part, body, .. }) => Some((*part, body.sources.clone(), body.window)),
                _ => None,
            })
            .collect()
    }

    /// The Main ask's body among the pended effects.
    fn main_body(gui: &Gui) -> crate::api::types::TasteRequest {
        gui.pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::Taste { part: TastePart::Main, body, .. }) => Some((**body).clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no Main ask in {:?}", gui.pending))
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

    /// Eight library albums (the server's default cap), best match first
    /// but sent out of order, the even ones with art the cache already
    /// decoded: more than the room's 80 cells show whole.
    fn strip_answer() -> TasteResponse {
        let mut answer = main_answer();
        let albums: Vec<serde_json::Value> = (0..8)
            .map(|i| {
                let mut album = json!({
                    "name": format!("Album {i}"), "album_artist": format!("Band {i}"),
                    "similarity": 0.9 - f64::from(i) * 0.01
                });
                if i % 2 == 0 {
                    album["album_art_file"] = json!(format!("art{i}.jpg"));
                }
                album
            })
            .rev()
            .collect();
        answer.library.as_mut().unwrap().albums = serde_json::from_value(json!(albums)).unwrap();
        answer
    }

    /// A cover with its own colours, decoded the way the worker decodes one.
    fn a_cover(seed: u8) -> crate::tui::art::Art {
        let pixels = image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([seed.wrapping_mul(40), (x * 4) as u8, (y * 4) as u8])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        pixels.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        crate::tui::art::decode(&bytes.into_inner()).unwrap()
    }

    /// The room answered with [`strip_answer`], the even albums' covers in
    /// the cache.
    fn strip_gui() -> Gui {
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        land(&mut gui, TastePart::Main, strip_answer());
        land(&mut gui, TastePart::Peers, peers_answer());
        for i in (0..8).step_by(2) {
            gui.app.art.insert(format!("art{i}.jpg"), Some(a_cover(i as u8)));
        }
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
            vec![(TastePart::Main, vec!["library".to_string()], Some(30))],
            "the library first, never the p2p network, even advertised (clause 30)"
        );
        assert!(gui.pending.iter().all(|e| matches!(e, Effect::Api(ApiCmd::Taste { .. }))), "no prefs: {:?}", gui.pending);
        land_main(&mut gui, main_answer());
        assert_eq!(
            asks(&gui),
            vec![(TastePart::Peers, vec!["peers".to_string()], Some(30))],
            "the connected servers once the library's batch is in, pinned to it (clause 30)"
        );
        land(&mut gui, TastePart::Peers, peers_answer());
        gui.pending.clear();
        // Every visit asks — no answer is reused (clause 3) — and the old
        // rows stand until the new land.
        gui.act(Act::Nav(FILES_NAV));
        gui.act(Act::Nav(DISCOVER_NAV));
        assert_eq!(asks(&gui).len(), 1, "re-entry asks: {:?}", gui.pending);
        assert!(draw(&mut gui).join("\n").contains("Aurora"), "the previous answer stays drawn");
        gui.pending.clear();
        // …and G (or the row) while the room is up asks at once.
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(asks(&gui).len(), 1, "G asks again: {:?}", gui.pending);
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
        // Four skeleton tiles on the strip (clause 45: the webapp's four
        // cards), four skeleton rows under the rule, and no "asking…" line.
        assert_eq!(rows[8].matches('╭').count(), 4, "four empty cover frames:\n{all}");
        assert!(rows[13].contains("░░░") && rows[14].contains("░░░"), "title and artist runs:\n{all}");
        for y in 19..23 {
            assert!(rows[y].contains("░░░"), "a skeleton row at {y}:\n{all}");
        }
        assert!(!rows[23].contains('░'), "four of them:\n{all}");
        assert_eq!(all.matches("asking").count(), 1, "the state word alone says it:\n{all}");
        assert_no_counts(&rows);
        key(&mut gui, KeyCode::Right);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[19].contains("░░░") && rows[17].contains(" ON "), "the Connected servers skeleton:\n{all}");
        assert_eq!(all.matches("asking").count(), 1, "{all}");
        // At the floor the strip gives way: the rows stand under the tabs.
        let rows = draw_sized(&mut gui, 100, 24);
        assert!(rows[11].contains("░░░") && rows[9].contains(" ON ") && !rows[9].contains('╭'), "{}", rows.join("\n"));
        // Nothing to play yet: the verbs line is hidden, never grey, and
        // keeps its row.
        assert!(rows[8].chars().skip(17).all(|c| c == ' '), "{}", rows.join("\n"));
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
        // The album strip under the seeds line (clause 45): the album with
        // no art is the note tile, its title and its artist under it.
        let room = |y: usize| rows[y].chars().skip(17).collect::<String>();
        assert!(room(8).starts_with("╭──────────╮"), "the note tile's frame:\n{all}");
        assert!(rows[10].contains("│    ♪     │"), "the note in it:\n{all}");
        assert_eq!(at(&rows, "Night Album"), Position { x: 17, y: 13 }, "the title under the cover:\n{all}");
        assert_eq!(at(&rows, "Moon"), Position { x: 17, y: 14 }, "the artist under it:\n{all}");
        // The verbs line under the strip, right over the table (clause 64,
        // the webapp's verbs under its songs' header): play the accent,
        // save as playlist on the Library tab.
        assert_eq!(at(&rows, "▸ play · + queue all · ⇄ shuffle · save as playlist…"), Position { x: 17, y: 16 }, "{all}");
        for head in ["TRACK", "ARTIST", "ALBUM", "LENGTH"] {
            assert!(rows[17].contains(head), "{head} in the header:\n{}", rows[17]);
        }
        assert!(!rows[17].contains("MATCH") && !rows[17].contains("LAST PLAYED"), "no meter, no last played:\n{}", rows[17]);
        // The album left the table for the strip; the songs: title, artist,
        // album, length (clauses 50–51).
        assert!(!all.contains("Night Album album"), "no album row where the strip stands:\n{all}");
        for cell in ["Aurora", "Ray", "Dawn", "4:11"] {
            assert!(rows[19].contains(cell), "{cell} on the song row:\n{}", rows[19]);
        }
        assert_eq!(at(&rows, "Ray").x, at(&rows, "ARTIST").x, "ARTIST over its words");
        assert_eq!(at(&rows, "Dawn").x, at(&rows, "ALBUM").x, "ALBUM over its words");
        assert!(rows[21].contains("Cirrus") && rows[21].contains("—"), "a missing artist is the dash:\n{}", rows[21]);
        // No bottom row: the verbs left it for the verbs line, and the
        // checkbox left the page.
        assert_eq!(all.matches("queue all").count(), 1, "the verbs stand once:\n{all}");
        assert!(!all.contains("Include songs") && !all.contains("[ ]"), "the filter left the page:\n{all}");
        assert!(!all.contains("asking"), "nothing asking once both halves landed:\n{all}");
        assert_no_counts(&rows);
    }

    #[test]
    fn the_queue_panel_drops_album_first_then_on() {
        let _en = english();
        let mut gui = answered_gui();
        gui.queue_open = true;
        let rows = draw(&mut gui);
        assert!(rows[17].contains("TRACK") && rows[17].contains("ARTIST") && rows[17].contains("LENGTH"), "{}", rows[17]);
        assert!(!rows[17].contains("ALBUM"), "47 cells: ALBUM goes first:\n{}", rows[17]);
        assert!(rows[2].contains("Last 30 days ▾"), "the window control keeps its home:\n{}", rows[2]);
        // 47 cells: the verbs line drops save as playlist from its tail and
        // keeps its home over the table.
        assert!(rows[16].contains("▸ play · + queue all · ⇄ shuffle"), "the verbs keep their home:\n{}", rows[16]);
        assert!(!rows[16].contains("save as"), "save drops first:\n{}", rows[16]);
        assert_no_counts(&rows);
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        let rows = draw(&mut gui);
        assert!(rows[17].contains("TRACK") && rows[17].contains("ARTIST") && rows[17].contains("LENGTH"), "{}", rows[17]);
        assert!(!rows[17].contains("ALBUM") && !rows[17].contains(" ON "), "then ON:\n{}", rows[17]);
        // With the room's 80 cells, all five stand.
        gui.queue_open = false;
        let rows = draw(&mut gui);
        for head in ["TRACK", "ARTIST", "ALBUM", "ON", "LENGTH"] {
            assert!(rows[17].contains(head), "{head} at 80 cells:\n{}", rows[17]);
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
        // The strip: the one album-tagged row's album, from Sam's server.
        assert_eq!(at(&rows, "Theirs"), Position { x: 17, y: 13 }, "the album's tile:\n{all}");
        assert!(rows[14].contains("Pia"), "{}", rows[14]);
        // The verbs line: play, queue all and shuffle — no save, a
        // connected server's songs are not this server's.
        assert!(rows[16].contains("▸ play · + queue all · ⇄ shuffle"), "{all}");
        assert!(!all.contains("save as playlist"), "no save on this tab:\n{all}");
        // No group lines: the rows start under the rule.
        assert!(rows[19].contains("Peer One") && rows[19].contains("Pia") && rows[19].contains("Theirs"), "{}", rows[19]);
        assert!(rows[19].contains("Sam's server") && rows[19].contains("3:20"), "ON and LENGTH:\n{}", rows[19]);
        assert!(rows[20].contains("Peer Two"), "{}", rows[20]);
        for gone in ["Far box", "Odd model", "Elsewhere", "online", "not answering", "last seen", "sonic model"] {
            assert!(!all.contains(gone), "{gone:?} is not drawn:\n{all}");
        }
        assert_no_counts(&rows);
        // ↓ lands on the strip, ↓ again on the first song; Enter plays it
        // through your server (the Listen verb's way) and queues the tab's
        // rows after it.
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on && gui.discover.cursor.is_none(), "the strip is the top stop");
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
        gui.ui.pointer = Some(Position { x: 30, y: 20 });
        let rows = draw(&mut gui);
        assert!(rows[20].contains("[▸] [»] [+] [⋯]"), "the song verbs, [⋯] among them:\n{}", rows[20]);
        let dots = Position { x: rows[20].find("[⋯]").map(|i| rows[20][..i].chars().count()).unwrap() as u16, y: 20 };
        assert_eq!(gui.ui.hit(dots), Some(Act::Disc(DiscAct::More(1))));
        assert_eq!(gui.ui.hit_context(Position { x: 30, y: 19 }), Some(Act::Disc(DiscAct::More(0))), "a right click too");
        gui.act(Act::Disc(DiscAct::More(1)));
        let sheet = gui.actions.sheet.as_ref().expect("the sheet opens");
        assert_eq!((sheet.origin.server.as_str(), sheet.origin.peer), (HOST, Some(7)), "the peer's origin");
        assert_eq!(sheet.track.filepath, "Music/p2.mp3");
        assert_eq!(sheet.queue_row, None);
        // `m` with the cursor up, too — not on the strip, whose tile has
        // one verb.
        gui.actions.sheet = None;
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('m'));
        assert!(gui.actions.sheet.is_none(), "a tile has no sheet");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('m'));
        assert_eq!(gui.actions.sheet.as_ref().map(|s| s.track.filepath.as_str()), Some("Music/p1.mp3"));
    }

    #[test]
    fn an_album_rows_play_control_queues_the_album() {
        let _en = english();
        let mut gui = answered_gui();
        // At the floor, where the album stays a row (clause 49).
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        let rows = draw_sized(&mut gui, 100, 24);
        assert!(rows[11].contains("[+]") && !rows[11].contains("[»]") && !rows[11].contains("[⋯]"), "one control:\n{}", rows[11]);
        let plus = Position { x: rows[11].find("[+]").map(|i| rows[11][..i].chars().count()).unwrap() as u16, y: 11 };
        assert_eq!(gui.ui.hit(plus), Some(Act::Disc(DiscAct::Row(0, RowVerb::Queue))));
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Queue)));
        let generation = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::TasteAlbum { name, generation, peer: None, reach: None, play: false, .. })
                    if name == "Night Album" =>
                {
                    Some(*generation)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the album's songs are asked by name: {:?}", gui.pending));
        assert_eq!(gui.active, DISCOVER_NAV, "the room stays");
        let tracks: Vec<crate::api::types::Track> = serde_json::from_value(json!([
            { "filepath": "music/n1.mp3", "metadata": { "title": "Night One" } },
            { "filepath": "music/n2.mp3", "metadata": { "title": "Night Two" } }
        ]))
        .unwrap();
        let landed = |generation: u64, tracks: Option<Vec<crate::api::types::Track>>| Event::TasteAlbum {
            generation,
            tracks,
            name: "Night Album".into(),
            artist: None,
            peer: None,
            play: false,
        };
        let effects = gui.app.apply_event(landed(generation, Some(tracks.clone())));
        gui.pend(effects);
        let queued: Vec<_> = gui.app.queue.items.iter().map(|q| (q.filepath.as_str(), q.origin.peer)).collect();
        assert_eq!(queued, vec![("music/n1.mp3", None), ("music/n2.mp3", None)], "appended with the session's origin");
        // Another generation fills nothing; a failure says so.
        gui.app.apply_event(landed(generation + 1, Some(tracks)));
        assert_eq!(gui.app.queue.items.len(), 2);
        gui.app.apply_event(landed(generation, None));
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
        gui.ui.pointer = Some(Position { x: 30, y: 19 });
        let rows_hover = draw(&mut gui);
        assert!(rows_hover[19].contains("[▸] [»] [+] [⋯]"), "the hover verbs:\n{}", rows_hover[19]);
        assert!(rows_hover[19].contains("Aurora") && rows_hover[19].contains("Ray"), "TRACK and ARTIST stay:\n{}", rows_hover[19]);
        assert!(!rows_hover[13].contains("[▸]"), "an album has no play-now:\n{}", rows_hover[13]);
        assert_eq!(gui.discover.cursor, None, "hover never selects");
        let plus = Position { x: rows_hover[19].find("[+]").map(|i| rows_hover[19][..i].chars().count()).unwrap() as u16, y: 19 };
        assert_eq!(gui.ui.hit(plus), Some(Act::Disc(DiscAct::Row(0, RowVerb::Queue))));
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Queue)));
        let item = &gui.app.queue.items[0];
        assert_eq!((item.filepath.as_str(), item.origin.server.as_str(), item.origin.peer), ("music/a.mp3", HOST, None));
        assert!(rows[19].contains("4:11"), "at rest the row keeps its columns");
    }

    #[test]
    fn down_lights_the_slab_and_the_verbs_want_the_cursor_up() {
        let _en = english();
        let mut gui = answered_gui();
        let buffer = draw_buffer(&mut gui);
        assert!((8..25).all(|y| (17..97).all(|x| buffer[(x, y)].bg != th().accent)), "nothing lit on arrival");
        // Stowed, Enter and `a` do nothing (clause 57).
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Char('a'));
        assert!(gui.app.queue.items.is_empty(), "no verb on a row nobody can see");
        key(&mut gui, KeyCode::Down);
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(20, 13)].bg, th().accent, "↓ lights the top: the strip's first tile");
        assert_ne!(buffer[(40, 19)].bg, th().accent, "one thing lit");
        key(&mut gui, KeyCode::Down);
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(40, 19)].bg, th().accent, "↓ again lights the first song");
        assert_ne!(buffer[(20, 13)].bg, th().accent, "and the tile goes dark");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.queue.items.len(), 4, "Enter plays from here: the tab's songs");
        assert_eq!(gui.app.queue.current, Some(0), "the row it was on");
        assert_eq!(gui.discover.cursor, Some(0), "played by key, the cursor stays up");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.discover.cursor, None, "Esc stows it");
        assert_eq!(gui.active, DISCOVER_NAV);
    }

    #[test]
    fn the_fallback_card_stands_while_nothing_is_finished() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        land_main(&mut gui, fallback_answer());
        assert!(asks(&gui).is_empty(), "a random seed asks no connected server (clause 22): {:?}", gui.pending);
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
        // Another pick is the plain ask again, the random picks shown in
        // the visit passed over (clause 21).
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('r'));
        assert_eq!(asks(&gui).len(), 1, "{:?}", gui.pending);
        let body = main_body(&gui);
        assert_eq!((body.exclude, body.pin.len()), (vec!["r1".to_string()], 0));
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
    fn an_empty_library_and_quiet_servers_say_so_in_one_line() {
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
        assert!(!all.contains("Include songs"), "the line alone, no way to widen the filter:\n{all}");
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
        land_main(&mut gui, main_answer());
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
    fn the_seeds_modal_sends_the_visits_drops_once_when_it_closes() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Char('e'));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Based on") && all.contains("FROM YOUR PLAYS"), "{all}");
        assert!(all.contains("[✓] Manor · ALM") && all.contains("finished"), "{all}");
        assert!(!all.contains("plays ·") && !all.contains("4 plays"), "no play counts:\n{all}");
        assert!(all.contains("not analyzed yet"), "{all}");
        // An exclusion the visit did not drop is not offered back.
        assert!(!all.contains("DROPPED") && !all.contains("Old Song"), "nothing dropped yet:\n{all}");
        gui.pending.clear();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char(' '));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("[ ] Manor · ALM"), "Space drops it, in the modal only:\n{all}");
        assert!(gui.pending.is_empty(), "nothing goes out while it is open");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.seeds.is_none());
        assert_eq!(asks(&gui).len(), 1, "one ask, the library's: {:?}", gui.pending);
        assert_eq!(main_body(&gui).exclude, vec!["h1".to_string()]);
        // The answer lists it under DROPPED; checked again ("use again") it
        // leaves the list and the room asks without it.
        let mut answer = main_answer();
        answer.seeds.remove(0);
        answer.excluded = serde_json::from_value(json!([
            { "hash": "h1", "filepath": "music/manor.mp3", "metadata": { "title": "Manor", "artist": "ALM" } }
        ]))
        .unwrap();
        land(&mut gui, TastePart::Main, answer);
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('e'));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("DROPPED") && all.contains("[ ] Manor · ALM"), "{all}");
        let rows = super::seed_lines(&gui.app);
        let at_row = rows.iter().position(|l| *l == super::SeedLine::Dropped("h1".into())).unwrap();
        gui.act(Act::Disc(DiscAct::SeedRow(at_row)));
        key(&mut gui, KeyCode::Esc);
        assert!(main_body(&gui).exclude.is_empty(), "{:?}", gui.pending);
        // A modal closed with nothing changed asks nothing.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('e'));
        key(&mut gui, KeyCode::Esc);
        assert!(gui.pending.is_empty(), "{:?}", gui.pending);
        // A fresh visit lets the drops and pins go.
        gui.app.taste.dropped.push("h2".into());
        gui.act(Act::Nav(FILES_NAV));
        gui.act(Act::Nav(DISCOVER_NAV));
        let body = main_body(&gui);
        assert!(body.exclude.is_empty() && body.pin.is_empty(), "{body:?}");
    }

    #[test]
    fn the_window_is_a_dropdown_remembered_in_the_gui_config() {
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
        assert_eq!(asks(&gui).iter().map(|a| a.2).collect::<Vec<_>>(), vec![Some(0)], "the library's part");
        assert_eq!(gui.config.gui.discover_window, Some(0), "remembered beside the tab");
        let mut answer = main_answer();
        answer.window = Some(0);
        land_main(&mut gui, answer);
        assert_eq!(asks(&gui).iter().map(|a| a.2).collect::<Vec<_>>(), vec![Some(0)], "and the servers' after it");
        // `[` steps it without the list, remembered too.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('['));
        assert_eq!(main_body(&gui).window, Some(90));
        assert_eq!(gui.config.gui.discover_window, Some(90));
        // `f` is nothing here any more.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('f'));
        assert!(asks(&gui).is_empty(), "{:?}", gui.pending);
        // A remembered window is the one a new shell asks with.
        let mut config = Config::default();
        config.gui.discover_window = Some(7);
        let mut gui = session_gui_with(config);
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(main_body(&gui).window, Some(7));
        assert!(draw(&mut gui)[2].contains("Last 7 days ▾"), "shown before any answer");
        // A number not among the four is no memory.
        let mut config = Config::default();
        config.gui.discover_window = Some(14);
        let mut gui = session_gui_with(config);
        key(&mut gui, KeyCode::Char('G'));
        assert_eq!(main_body(&gui).window, Some(30));
    }

    #[test]
    fn an_album_row_opens_in_the_albums_room() {
        let _en = english();
        let mut gui = answered_gui();
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        draw_sized(&mut gui, 100, 24);
        assert_eq!(gui.ui.hit(Position { x: 30, y: 11 }), Some(Act::Disc(DiscAct::Row(0, RowVerb::Open))));
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
        gui.wheel(Position { x: 40, y: 20 }, 3);
        draw(&mut gui);
        assert_eq!(gui.discover.view.scroll, 3, "the wheel scrolls the list under the pointer");
        gui.act(Act::ScrollBy(List::Discover, -1));
        assert_eq!(gui.discover.view.scroll, 2);
    }

    // ── The album strip (clauses 45–49) ─────────────────────────────────

    /// The room's own cells of screen row `y`, right of the nav column.
    fn room(rows: &[String], y: usize) -> String {
        rows[y].chars().skip(17).collect()
    }

    #[test]
    fn the_strip_stands_from_twenty_content_rows_and_the_floor_keeps_the_album_rows() {
        let _en = english();
        // The layout math (clause 49): the strip's seven rows and its gap
        // stand where the table keeps three rows under them — the verbs
        // line, the table's header and rule between, no bottom row.
        let content = |height: u16| Rect { x: 17, y: 2, width: 80, height };
        assert_eq!(layout(content(19), false, true).strip_y, None, "19 rows: the table would keep two");
        let twenty = layout(content(20), false, true);
        assert_eq!((twenty.strip_y, twenty.verbs_y, twenty.table_y), (Some(8), 16, 17), "{twenty:?}");
        assert_eq!(content(20).bottom() - (twenty.table_y + 2), 3, "three rows under the strip");
        assert_eq!(layout(content(20), false, false).strip_y, None, "no albums, no strip");
        // The card comes first: its whole height and the strip want 28.
        assert_eq!(layout(content(27), true, true).strip_y, None);
        assert!(layout(content(28), true, true).strip_y.is_some());
        // 100×30 with the footer on (22 rows): the strip, the verbs line
        // under it, five rows under the table's header and rule — the
        // room's last row is a row (no bottom row).
        let mut gui = answered_gui();
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(room(&rows, 8).starts_with("╭──────────╮") && room(&rows, 13).starts_with("Night Album"), "{all}");
        assert!(room(&rows, 16).starts_with("▸ play · + queue all"), "{all}");
        assert!(rows[17].contains("TRACK") && rows[19].contains("Aurora"), "{all}");
        assert!(!rows[23].contains("play"), "no bottom row:\n{all}");
        // The floor, footer on and off: the album stays a row, no strip.
        for footer in [false, true] {
            gui.config.gui.key_hints = footer;
            gui.ui.key_hints = footer;
            let rows = draw_sized(&mut gui, 100, 24);
            let all = rows.join("\n");
            assert!(room(&rows, 8).starts_with("▸ play · + queue all"), "{footer}: the verbs line over the table:\n{all}");
            assert!(rows[11].contains("Night Album album") && rows[12].contains("Aurora"), "{footer}:\n{all}");
            let room_rows: String = (2..18).map(|y| room(&rows, y)).collect();
            assert!(!room_rows.contains('♪') && !room_rows.contains('╭'), "{footer}: no tile at the floor:\n{all}");
        }
        // The card at 100×30 keeps the table's rows: no strip under it.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = fallback_answer();
        answer.library.as_mut().unwrap().albums = main_answer().library.unwrap().albums;
        land(&mut gui, TastePart::Main, answer);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Night Album album") && !all.contains('♪'), "{all}");
    }

    #[test]
    fn the_strip_scrolls_with_the_wheel_and_cuts_its_last_tile() {
        let _en = english();
        let mut gui = strip_gui();
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        // Best match first; six whole tiles in the room's 80 cells, the
        // seventh cut at the right edge — the cut says there is more.
        assert!(room(&rows, 13).starts_with("Album 0      Album 1"), "{all}");
        assert!(room(&rows, 13).trim_end().ends_with("Album 5      A…"), "the cut tile:\n{all}");
        assert!(room(&rows, 14).trim_end().ends_with("Band 5       B…"), "{all}");
        // Covers as the mosaic here (no pixels under TestBackend), the note
        // tile where an album has no art; the cut cover is cut, not squeezed.
        assert!(room(&rows, 10).starts_with("▀▀▀▀▀▀▀▀▀▀▀▀ │    ♪     │ ▀▀▀▀▀▀▀▀▀▀▀▀"), "{all}");
        assert_eq!(room(&rows, 10).trim_end().chars().rev().take(3).collect::<String>(), "▀▀ ", "{all}");
        // The wheel over the strip: one tile a notch, whatever the notch,
        // never past the last whole view; the table stays.
        gui.wheel(Position { x: 40, y: 10 }, 1);
        assert_eq!(gui.discover.strip.first, 1);
        gui.wheel(Position { x: 40, y: 13 }, 3);
        assert_eq!(gui.discover.strip.first, 2);
        gui.wheel(Position { x: 40, y: 10 }, 1);
        assert_eq!(gui.discover.strip.first, 2, "eight tiles, six whole: two is the end");
        let rows = draw(&mut gui);
        assert!(room(&rows, 13).starts_with("Album 2") && room(&rows, 13).trim_end().ends_with("Album 7"), "{}", rows.join("\n"));
        assert!(!rows[13].contains('…'), "the end: nothing cut");
        gui.wheel(Position { x: 40, y: 10 }, -1);
        assert_eq!(gui.discover.strip.first, 1);
        assert_eq!((gui.discover.view.scroll, gui.discover.cursor, gui.discover.strip.on), (0, None, false), "the wheel lights nothing");
        // The wheel over the table scrolls the table, not the strip.
        gui.wheel(Position { x: 40, y: 19 }, 1);
        assert_eq!(gui.discover.strip.first, 1);
    }

    #[test]
    fn the_cut_tile_stands_whatever_the_width() {
        let _en = english();
        // Content of 90 and 91 cells (110 and 111 columns, the queue
        // folded): seven tiles would end exactly at the edge, so the seventh
        // gives its place up to a half tile — the cut that says there is more.
        for width in [110u16, 111] {
            let mut gui = strip_gui();
            let rows = draw_sized(&mut gui, width, 30);
            let all = rows.join("\n");
            assert_eq!(gui.discover.content.width, width - 20);
            let line = room(&rows, 13);
            assert!(line.trim_end().ends_with("Album 5      Album…"), "{width}: the cut tile:\n{all}");
            assert_eq!(super::strip_view(width - 20, 8, 0), (6, 6), "{width}");
            // At the end nothing follows: seven whole tiles, nothing cut.
            gui.discover.strip.first = 9;
            let rows = draw_sized(&mut gui, width, 30);
            assert_eq!(gui.discover.strip.first, 1, "{width}: clamped to the last whole view");
            let line = room(&rows, 13);
            assert!(line.starts_with("Album 1") && line.trim_end().ends_with("Album 7"), "{width}:\n{}", rows.join("\n"));
            assert!(!line.contains('…'), "{width}: the end, nothing cut");
        }
        // Every width from one tile to twelve: while tiles follow, a cut
        // tile of at least one cell stands, and nothing reaches past the edge.
        for width in 25u16..=170 {
            let (whole, cut) = super::strip_view(width, 20, 0);
            assert!(cut > 0 && cut < super::TILE_W, "{width}: ({whole}, {cut})");
            assert!(whole as u16 * super::TILE_STRIDE + cut <= width, "{width}: ({whole}, {cut})");
        }
    }

    #[test]
    fn a_narrowed_room_keeps_the_lit_tile_in_view() {
        let _en = english();
        let mut gui = strip_gui();
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        for _ in 0..6 {
            key(&mut gui, KeyCode::Right);
        }
        assert_eq!((gui.discover.strip.cursor, gui.discover.strip.first), (6, 1));
        // The queue opens: three whole tiles in 47 cells, and the hand's
        // tile stays among them.
        gui.queue_open = true;
        let rows = draw(&mut gui);
        assert_eq!((gui.discover.strip.cursor, gui.discover.strip.first), (6, 4), "{}", rows.join("\n"));
        let (whole, _) = super::strip_view(gui.discover.content.width, 8, 4);
        assert!(6 < 4 + whole, "the lit tile is whole in view");
        assert!(room(&rows, 13).contains("Album 6"), "{}", rows.join("\n"));
        // Enter acts on the album the user sees lit.
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('a'));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteAlbum { name, .. }) if name == "Album 6")));
    }

    #[test]
    fn the_hand_keeps_its_row_across_the_height_gate() {
        let _en = english();
        let mut gui = answered_gui();
        draw(&mut gui);
        // A song, lit at 100×30 (the strip holds the album): the floor puts
        // the album row back above it, and the hand stays on the song.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.cursor, Some(0));
        draw_sized(&mut gui, 100, 24);
        let held = gui.discover.cursor.map(|c| super::rows(&gui)[c]);
        assert_eq!(held, Some(Line::Song(0)), "Aurora still");
        assert!(gui.discover.view.held);
        draw(&mut gui);
        assert_eq!(gui.discover.cursor, Some(0), "and back");
        assert_eq!(super::rows(&gui)[0], Line::Song(0));
        // The album row at the floor becomes its tile where the strip stands,
        // and its row again at the floor.
        key(&mut gui, KeyCode::Esc);
        draw_sized(&mut gui, 100, 24);
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.cursor.map(|c| super::rows(&gui)[c]), Some(Line::Album(0)));
        draw(&mut gui);
        assert_eq!((gui.discover.strip.on, gui.discover.strip.cursor, gui.discover.cursor), (true, 0, None));
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(18, 13)].bg, th().accent, "the tile wears the slab");
        draw_sized(&mut gui, 100, 24);
        assert!(!gui.discover.strip.on);
        assert_eq!(gui.discover.cursor.map(|c| super::rows(&gui)[c]), Some(Line::Album(0)));
        assert!(gui.discover.view.held);
    }

    #[test]
    fn an_answer_of_albums_alone_draws_no_empty_table() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = main_answer();
        answer.library.as_mut().unwrap().tracks.clear();
        land(&mut gui, TastePart::Main, answer);
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(room(&rows, 13).starts_with("Night Album"), "the strip holds the album:\n{all}");
        assert!(!all.contains("TRACK") && !all.contains("LENGTH"), "no table header under it:\n{all}");
        // ↓ and ↑ find the strip; nothing below it to walk to.
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on);
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on && gui.discover.cursor.is_none());
    }

    #[test]
    fn the_note_tile_keeps_the_ground_under_it() {
        let mut terminal = Terminal::new(TestBackend::new(20, 6)).unwrap();
        let ground = ratatui::style::Color::Rgb(1, 2, 3);
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(ratatui::widgets::Block::default().style(ratatui::style::Style::default().bg(ground)), area);
                super::note_tile(frame, Rect { x: 2, y: 0, width: 12, height: 5 });
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 0)].symbol(), "╭");
        assert!((2..14).all(|x| (0..5).all(|y| buffer[(x, y)].bg == ground)), "the owned ground stays");
    }

    #[test]
    fn the_arrows_walk_the_strip_and_switch_tabs_off_it() {
        let _en = english();
        let mut gui = strip_gui();
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        assert_eq!((gui.discover.strip.on, gui.discover.strip.cursor), (true, 0), "↓ lands on the first tile in view");
        for _ in 0..6 {
            key(&mut gui, KeyCode::Right);
        }
        assert_eq!(gui.app.taste.tab, TasteTab::Library, "← → walk the tiles while the hand is on the strip");
        assert_eq!((gui.discover.strip.cursor, gui.discover.strip.first), (6, 1), "scrolled to keep the tile whole");
        let buffer = draw_buffer(&mut gui);
        let lit = 17 + 5 * 13;
        assert_eq!(buffer[(lit + 1, 13)].bg, th().accent, "the slab on the hand's tile");
        assert_eq!(buffer[(lit, 13)].symbol(), "A", "Album 6, whole at the fifth place");
        assert_ne!(buffer[(lit - 12, 13)].bg, th().accent, "one tile lit");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Right);
        assert_eq!((gui.discover.strip.cursor, gui.discover.strip.first), (7, 2), "the last tile stops the hand");
        for _ in 0..9 {
            key(&mut gui, KeyCode::Left);
        }
        assert_eq!((gui.discover.strip.cursor, gui.discover.strip.first), (0, 0));
        assert_eq!(gui.app.taste.tab, TasteTab::Library);
        let tips = super::tips(&gui);
        assert!(tips.starts_with("← → albums") && tips.contains("Enter open the album") && tips.contains("+ a queue"), "{tips}");
        // Off the strip, ← → switch tabs as they always did — the tips say so.
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.discover.strip.on, "Esc stows the hand");
        assert!(super::tips(&gui).contains("← → tabs"), "{}", super::tips(&gui));
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.app.taste.tab, TasteTab::Servers);
        assert_eq!(gui.discover.strip, super::StripUi::default(), "a tab switch starts its strip from the left");
    }

    #[test]
    fn the_strip_is_a_cursor_stop_above_the_table() {
        let _en = english();
        let mut gui = answered_gui();
        let buffer = draw_buffer(&mut gui);
        assert!((17..29).all(|x| buffer[(x, 13)].bg != th().accent), "nothing lit on arrival");
        assert!(super::tips(&gui).starts_with("↓ pick an album · ← → tabs"), "{}", super::tips(&gui));
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on && gui.discover.cursor.is_none(), "↓ from rest: the strip, the top");
        assert_eq!(super::tips(&gui), "← → albums · Enter open the album · + a queue · ↓ songs · Esc done");
        key(&mut gui, KeyCode::Up);
        assert!(gui.discover.strip.on, "nothing above the strip");
        key(&mut gui, KeyCode::Down);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (false, Some(0)), "↓ from the strip: the first song");
        assert!(super::tips(&gui).contains("m actions"), "{}", super::tips(&gui));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(gui.discover.cursor, Some(0), "↑ walks the table");
        key(&mut gui, KeyCode::Up);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (true, None), "↑ from the first song: back to the strip");
        // PageDown from rest stops on the strip too, PageUp from the first
        // row goes back to it (clause 57).
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::PageDown);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (true, None), "PageDown from rest: the strip");
        key(&mut gui, KeyCode::PageDown);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (false, Some(0)));
        key(&mut gui, KeyCode::PageUp);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (true, None), "PageUp from the first row: the strip");
        // A row's verbs are not a tile's: N, P and m do nothing there.
        key(&mut gui, KeyCode::Char('P'));
        key(&mut gui, KeyCode::Char('m'));
        assert!(gui.app.queue.items.is_empty() && gui.actions.sheet.is_none());
        // A click stows the hand (the list-cursor law).
        gui.act(Act::Disc(DiscAct::Tile(0, RowVerb::Queue)));
        assert!(!gui.discover.strip.on);
        // At the floor the album is a row again, the first stop of ↓.
        draw_sized(&mut gui, 100, 24);
        key(&mut gui, KeyCode::Down);
        assert_eq!((gui.discover.strip.on, gui.discover.cursor), (false, Some(0)));
        assert_eq!(super::tips(&gui), "Enter open the album · a queue · ← → tabs · Esc done");
    }

    #[test]
    fn a_tile_opens_its_album_and_its_plus_queues_it() {
        let _en = english();
        let mut gui = answered_gui();
        draw(&mut gui);
        // Hover brightens the title BOLD and shows the one verb at the
        // tile's corner; it never selects (clause 48).
        gui.ui.pointer = Some(Position { x: 20, y: 10 });
        let buffer = draw_buffer(&mut gui);
        let title = &buffer[(17, 13)];
        assert!(title.modifier.contains(ratatui::style::Modifier::BOLD) && title.fg == th().bright, "{title:?}");
        let corner: String = (26..29).map(|x| buffer[(x, 13)].symbol()).collect();
        assert_eq!(corner, "[+]");
        assert_eq!(buffer[(17 + 7, 13)].symbol(), "…", "the title gives its tail to the verb");
        assert!(!gui.discover.strip.on, "hover never selects");
        assert_eq!(gui.ui.hit(Position { x: 20, y: 10 }), Some(Act::Disc(DiscAct::Tile(0, RowVerb::Open))));
        assert_eq!(gui.ui.hit(Position { x: 27, y: 13 }), Some(Act::Disc(DiscAct::Tile(0, RowVerb::Queue))));
        // [+] queues the album by name, the row's way (clause 61).
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Tile(0, RowVerb::Queue)));
        let album = |gui: &Gui| {
            gui.pending.iter().find_map(|e| match e {
                Effect::Api(ApiCmd::TasteAlbum { name, peer, play, .. }) => Some((name.clone(), *peer, *play)),
                _ => None,
            })
        };
        assert_eq!(album(&gui), Some(("Night Album".to_string(), None, false)));
        assert_eq!(gui.active, DISCOVER_NAV);
        // The keys, with the hand on the tile: `+` and `a` queue, Enter opens.
        for c in ['+', 'a'] {
            gui.pending.clear();
            key(&mut gui, KeyCode::Down);
            key(&mut gui, KeyCode::Char(c));
            assert_eq!(album(&gui), Some(("Night Album".to_string(), None, false)), "{c}");
            assert!(gui.discover.strip.on, "acted by key, the hand stays");
            key(&mut gui, KeyCode::Esc);
        }
        // A click opens it in the Albums room, its row's way (clause 61).
        gui.act(Act::Disc(DiscAct::Tile(0, RowVerb::Open)));
        assert_eq!(gui.active, super::super::ALBUMS_NAV);
        assert!(matches!(
            gui.app.library_stack.here(),
            crate::tui::worker::LibraryNode::Album { name, artist } if name == "Night Album" && artist.is_none()
        ));
        // And Enter by key, the hand on the tile: the same drill.
        let mut gui = answered_gui();
        draw(&mut gui);
        key(&mut gui, KeyCode::Down);
        assert!(gui.discover.strip.on);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.active, super::super::ALBUMS_NAV, "Enter opens the album");
        assert!(matches!(
            gui.app.library_stack.here(),
            crate::tui::worker::LibraryNode::Album { name, .. } if name == "Night Album"
        ));
    }

    #[test]
    fn a_connected_servers_album_plays_and_queues_through_its_server() {
        let _en = english();
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        draw(&mut gui);
        assert_eq!(gui.ui.hit(Position { x: 20, y: 10 }), Some(Act::Disc(DiscAct::Tile(0, RowVerb::Open))));
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Tile(0, RowVerb::Open)));
        let ask = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::TasteAlbum { name, artist, generation, peer, reach, play }) => {
                    Some((name.clone(), artist.clone(), *generation, *peer, reach.as_ref().and_then(|r| r.peer), *play))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the album is asked of its server: {:?}", gui.pending));
        let (_, _, generation, _, _, _) = ask.clone();
        assert_eq!(ask, ("Theirs".into(), Some("Pia".into()), generation, Some(7), Some(7), true), "through the proxy, to play");
        assert_eq!(gui.active, DISCOVER_NAV, "no drill to open into: the room stays");
        // Its songs land: they play, with that server's origin.
        let tracks: Vec<crate::api::types::Track> = serde_json::from_value(json!([
            { "filepath": "Music/t1.mp3", "metadata": { "title": "Theirs One" } },
            { "filepath": "Music/t2.mp3", "metadata": { "title": "Theirs Two" } }
        ]))
        .unwrap();
        let landed = |tracks, play| Event::TasteAlbum {
            generation,
            tracks,
            name: "Theirs".into(),
            artist: Some("Pia".into()),
            peer: Some(7),
            play,
        };
        let effects = gui.app.apply_event(landed(Some(tracks), true));
        gui.pend(effects);
        let queued: Vec<_> = gui.app.queue.items.iter().map(|q| (q.filepath.as_str(), q.origin.peer)).collect();
        assert_eq!(queued, vec![("Music/t1.mp3", Some(7)), ("Music/t2.mp3", Some(7))]);
        assert_eq!(gui.app.queue.current, Some(0));
        let url = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Audio(crate::tui::worker::AudioCmd::Play { url, .. }) => Some(url.clone()),
                _ => None,
            })
            .expect("it plays");
        assert!(url.starts_with("http://host:3000/api/v1/federation/peers/7/stream/Music/t1.mp3"), "{url}");
        // `[+]`: a server that will not list it leaves the rows that matched,
        // appended.
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Tile(0, RowVerb::Queue)));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteAlbum { play: false, .. }))));
        gui.app.apply_event(landed(None, false));
        let last = gui.app.queue.items.last().map(|q| (q.filepath.as_str(), q.origin.peer));
        assert_eq!((gui.app.queue.items.len(), last), (3, Some(("Music/p1.mp3", Some(7)))));
        let tips = {
            draw(&mut gui);
            key(&mut gui, KeyCode::Down);
            super::tips(&gui)
        };
        assert!(tips.contains("Enter play the album"), "{tips}");
        // Enter by key: the album plays through its server, the hand stays.
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(
            gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TasteAlbum { play: true, peer: Some(7), .. }))),
            "{:?}",
            gui.pending
        );
        assert!(gui.discover.strip.on && gui.active == DISCOVER_NAV, "played by key, the hand stays on the tile");
    }

    #[test]
    fn strip_covers_are_claimed_once_from_their_own_servers() {
        let _en = english();
        let mut gui = strip_gui();
        gui.app.art.clear();
        gui.pending.clear();
        draw(&mut gui);
        let claims = |gui: &Gui| -> Vec<(String, Option<i64>)> {
            gui.pending
                .iter()
                .filter_map(|e| match e {
                    Effect::Api(ApiCmd::AlbumArt { file, reach }) => Some((file.clone(), reach.as_ref().and_then(|r| r.peer))),
                    _ => None,
                })
                .collect()
        };
        // The tiles in view, the cut one among them, from the session.
        let expected: Vec<(String, Option<i64>)> = ["art0.jpg", "art2.jpg", "art4.jpg", "art6.jpg"].iter().map(|f| (f.to_string(), None)).collect();
        assert_eq!(claims(&gui), expected);
        gui.pending.clear();
        draw(&mut gui);
        assert!(claims(&gui).is_empty(), "claimed once");
        // A connected server's album with art: through the parent's proxy.
        let mut gui = answered_gui();
        let mut answer = peers_answer();
        answer.peers.as_mut().unwrap().peers[0].results[0].raw["art"] = json!("pc.jpg");
        gui.act(Act::Disc(DiscAct::Again));
        land_main(&mut gui, main_answer());
        land(&mut gui, TastePart::Peers, answer);
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        gui.pending.clear();
        draw(&mut gui);
        assert_eq!(claims(&gui), vec![("pc.jpg".to_string(), Some(7))]);
    }

    #[test]
    fn strip_covers_are_pixels_where_the_terminal_draws_them_and_a_cut_tile_the_mosaic() {
        use ratatui_image::picker::ProtocolType;
        let _en = english();
        for protocol in [ProtocolType::Kitty, ProtocolType::Sixel, ProtocolType::Iterm2] {
            let mut gui = strip_gui();
            gui.app.graphics = crate::tui::graphics::Graphics::forced(protocol);
            let id = |gui: &Gui, i: usize| gui.app.art[&format!("art{i}.jpg")].as_ref().unwrap().id();
            let (whole, cut) = ([id(&gui, 0), id(&gui, 2), id(&gui, 4)], id(&gui, 6));
            // Paced: the frames run until every whole tile's cover is pixels.
            let mut frames = 0;
            let rows = loop {
                frames += 1;
                assert!(frames <= 30, "{protocol:?}: the covers never upgraded");
                let rows = draw(&mut gui);
                if whole.iter().all(|w| gui.discover.slots.get(w).is_some_and(|s| s.key.is_some())) {
                    break rows;
                }
            };
            assert!(gui.discover.slots.get(&cut).is_some_and(|s| s.key.is_none()), "{protocol:?}: the cut tile encoded nothing");
            assert!(!room(&rows, 10).starts_with("▀▀▀"), "{protocol:?}: the first cover is a picture:\n{}", rows[10]);
            assert!(room(&rows, 10).trim_end().ends_with("▀▀"), "{protocol:?}: the cut cover is the mosaic:\n{}", rows[10]);
            // A scroll moves each cover with its tile and encodes nothing new
            // for a cover already encoded at this size.
            let before: u32 = gui.discover.slots.values().map(|s| s.encodes()).sum();
            gui.wheel(Position { x: 40, y: 10 }, 1);
            draw(&mut gui);
            let after: u32 = gui.discover.slots.values().map(|s| s.encodes()).sum();
            assert!(after <= before + 1, "{protocol:?}: only the newly whole cover may encode ({before} → {after})");
        }
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

    // ── The verbs line ──────────────────────────────────────────────────

    /// Click the verb whose label starts with `label` on the frame `rows`,
    /// the way the room's other click tests do: what the pointer would hit
    /// there, acted.
    fn click_verb(gui: &mut Gui, rows: &[String], label: &str) -> DiscAct {
        let Some(Act::Disc(act)) = gui.ui.hit(at(rows, label)) else {
            panic!("{label:?} is no button:\n{}", rows.join("\n"));
        };
        gui.act(Act::Disc(act.clone()));
        act
    }

    fn queued_paths(gui: &Gui) -> Vec<&str> {
        gui.app.queue.items.iter().map(|q| q.filepath.as_str()).collect()
    }

    #[test]
    fn the_verbs_line_stands_under_the_strip_and_over_the_table_with_no_bottom_row() {
        let _en = english();
        for footer in [false, true] {
            let mut gui = answered_gui();
            gui.config.gui.key_hints = footer;
            gui.ui.key_hints = footer;
            // 100×30: the tabs, the seeds line, the strip, then the verbs
            // line right over the table's header (clause 64), and the
            // table down to the room's last row.
            let rows = draw(&mut gui);
            let all = rows.join("\n");
            let last = if footer { 23 } else { 24 };
            assert!(room(&rows, 5).contains(" Library ") && room(&rows, 6).starts_with("seeds"), "{footer}:\n{all}");
            assert_eq!(at(&rows, "▸ play · + queue all · ⇄ shuffle · save as playlist…"), Position { x: 17, y: 16 }, "{footer}:\n{all}");
            assert!(room(&rows, 17).contains("TRACK"), "{footer}: the header right under it:\n{all}");
            assert_eq!(all.matches("queue all").count(), 1, "{footer}: no bottom row:\n{all}");
            // Four songs under the strip — five or six rows of room for
            // them, the room's last row among them.
            let table = || (19..=last).map(|y| room(&rows, y)).collect::<Vec<_>>();
            assert!(table()[3].contains("Delta"), "{footer}:\n{all}");
            // 100×24: no strip; the verbs line under the seeds line's blank,
            // the album row and the songs under the header, seven rows with
            // the footer on and eight off, the last on the room's last row.
            let rows = draw_sized(&mut gui, 100, 24);
            let all = rows.join("\n");
            assert_eq!(at(&rows, "▸ play · + queue all · ⇄ shuffle · save as playlist…"), Position { x: 17, y: 8 }, "{footer}:\n{all}");
            assert!(room(&rows, 9).contains("TRACK") && room(&rows, 11).contains("Night Album album"), "{footer}:\n{all}");
            assert_eq!(all.matches("queue all").count(), 1, "{footer}: no bottom row:\n{all}");
            let room_rows: String = (2..18).map(|y| room(&rows, y)).collect();
            assert!(!room_rows.contains('╭'), "{footer}: no strip at the floor:\n{all}");
        }
    }

    #[test]
    fn the_table_runs_to_the_rooms_last_row_where_the_bottom_row_was() {
        // Twelve songs overflow every size: the last song row drawn sits on
        // the room's last row, and nothing is drawn under it (clause 64).
        let _en = english();
        let mut answer = main_answer();
        let tracks: Vec<serde_json::Value> = (1..=12)
            .map(|i| json!({ "filepath": format!("music/s{i:02}.mp3"), "similarity": 0.9 - f64::from(i) * 0.01,
                             "metadata": { "title": format!("Song {i:02}"), "artist": "Ray" } }))
            .collect();
        answer.library.as_mut().unwrap().tracks = serde_json::from_value(json!(tracks)).unwrap();
        // (height, footer, the room's last row, the last song shown):
        // 100×30 under the strip — rows 19..23 (5) with the footer, 19..24
        // (6) without; 100×24, no strip — the album row then songs on rows
        // 11..17 (7) with the footer, 11..18 (8) without.
        for (height, footer, last, song) in [(30u16, true, 23usize, 5usize), (30, false, 24, 6), (24, true, 17, 6), (24, false, 18, 7)] {
            let mut gui = session_gui();
            gui.config.gui.key_hints = footer;
            gui.ui.key_hints = footer;
            key(&mut gui, KeyCode::Char('G'));
            land(&mut gui, TastePart::Main, answer.clone());
            land(&mut gui, TastePart::Peers, peers_answer());
            let rows = draw_sized(&mut gui, 100, height);
            let all = rows.join("
");
            let first = if height == 30 { 19 } else { 11 };
            assert_eq!(last - first + 1, if height == 30 { song } else { song + 1 }, "{height}/{footer}: the row count");
            assert!(room(&rows, last).contains(&format!("Song {song:02}")), "{height}/{footer}: song {song} on row {last}:\n{all}");
            assert!(!all.contains(&format!("Song {:02}", song + 1)), "{height}/{footer}: nothing past the last row:\n{all}");
            assert_eq!(all.matches("queue all").count(), 1, "{height}/{footer}: no bottom row:\n{all}");
        }
    }

    #[test]
    fn the_verbs_line_wears_one_accent_and_hover_brightens() {
        let _en = english();
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        let play = at(&rows, "▸ play");
        let save = at(&rows, "save as playlist…");
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(play.x + 2, play.y)].fg, th().accent, "play is the one primary");
        assert_ne!(buffer[(save.x, save.y)].fg, th().accent, "the rest are text buttons");
        assert!(!buffer[(save.x, save.y)].modifier.contains(ratatui::style::Modifier::BOLD));
        gui.ui.pointer = Some(Position { x: save.x + 2, y: save.y });
        let buffer = draw_buffer(&mut gui);
        assert!(buffer[(save.x, save.y)].modifier.contains(ratatui::style::Modifier::BOLD), "hover brightens");
        assert_eq!(gui.discover.cursor, None, "and selects nothing");
    }

    #[test]
    fn each_verb_acts_by_click_and_by_key_on_the_library_tab() {
        let _en = english();
        let songs = vec!["music/a.mp3", "music/b.mp3", "music/c.mp3", "music/d.mp3"];
        // play: the tab's songs replace the queue and play from the first.
        for by_key in [false, true] {
            let mut gui = answered_gui();
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('p'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "▸ play"), DiscAct::PlayAll);
            }
            assert_eq!(queued_paths(&gui), songs, "{by_key}: play");
            assert_eq!(gui.app.queue.current, Some(0));
            // queue all: appended after what stands.
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('A'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "+ queue all"), DiscAct::QueueAll);
            }
            assert_eq!(gui.app.queue.items.len(), 8, "{by_key}: queue all appends");
            // shuffle: the same songs, replacing the queue.
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('S'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "⇄ shuffle"), DiscAct::Shuffle);
            }
            let mut shuffled = queued_paths(&gui);
            shuffled.sort_unstable();
            assert_eq!(shuffled, songs, "{by_key}: shuffle plays the tab's songs");
            assert!(!gui.app.queue.shuffle, "{by_key}: and leaves the shuffle mode alone");
            // save as playlist: the house prompt opens.
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('s'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "save as playlist"), DiscAct::SavePlaylist);
            }
            assert_eq!(gui.app.sonic_playlist_name.as_deref(), Some(""), "{by_key}: the prompt, empty as the webapp's");
            assert_eq!(gui.app.playlist_save_files.as_deref(), Some(&songs.iter().map(|s| s.to_string()).collect::<Vec<_>>()[..]));
        }
    }

    #[test]
    fn save_as_playlist_opens_the_house_prompt_and_saves_the_listed_songs() {
        let _en = english();
        // The album carries songs of its own: they stay out of the save.
        let mut answer = main_answer();
        answer.library.as_mut().unwrap().albums[0].tracks = vec!["music/n1.mp3".into(), "music/n2.mp3".into()];
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        land(&mut gui, TastePart::Main, answer);
        land(&mut gui, TastePart::Peers, peers_answer());
        let rows = draw(&mut gui);
        assert!(super::tips(&gui).ends_with("G ask again · s save as playlist"), "{}", super::tips(&gui));
        click_verb(&mut gui, &rows, "save as playlist");
        // The sonic room's prompt, over the Discover room.
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Save as playlist") && all.contains("enter saves · esc cancels"), "the house prompt:\n{all}");
        assert_eq!(gui.active, DISCOVER_NAV, "the room stays under it");
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = draw(&mut gui);
        assert!(rows[29].contains("type a name · Enter saves · Esc cancels"), "its tips:\n{}", rows[29]);
        // Typed into — `s`, `p` and `A` are letters now, not verbs.
        gui.pending.clear();
        for c in "Sap mix".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        assert_eq!(gui.app.sonic_playlist_name.as_deref(), Some("Sap mix"));
        assert!(gui.app.queue.items.is_empty(), "no verb ran while typing");
        key(&mut gui, KeyCode::Enter);
        let saved = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::SavePlaylist { name, files }) => Some((name.clone(), files.clone())),
            _ => None,
        });
        let songs: Vec<String> = ["music/a.mp3", "music/b.mp3", "music/c.mp3", "music/d.mp3"].map(String::from).to_vec();
        assert_eq!(saved, Some(("Sap mix".to_string(), songs)), "the listed songs, in order, albums left out");
        assert!(gui.app.sonic_playlist_name.is_none() && gui.app.playlist_save_files.is_none(), "the prompt closes");
        // Esc cancels and forgets the songs: the sonic room's save saves its
        // own stops again.
        let rows = draw(&mut gui);
        click_verb(&mut gui, &rows, "save as playlist");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.sonic_playlist_name.is_none() && gui.app.playlist_save_files.is_none(), "Esc cancels");
        assert_eq!(gui.active, DISCOVER_NAV);
    }

    #[test]
    fn the_rest_tips_with_save_fit_the_footer_in_every_locale() {
        // The footer has 99 cells at the 100-column floor; the save words
        // ride after either rest tip (clause 64).
        for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
            let _guard = locale(code);
            for strip in [false, true] {
                let mut gui = if strip { strip_gui() } else { answered_gui() };
                draw_sized(&mut gui, 100, if strip { 30 } else { 24 });
                let tips = super::tips(&gui);
                assert!(tips.ends_with(&*rust_i18n::t!("gui.tips.discover_save")), "{code}: {tips}");
                assert!(crate::kit::width(&tips) <= 99, "{code} ({strip}): {} cells: {tips}", crate::kit::width(&tips));
            }
            // The fallback card's tip names `s` too: its Library tab saves.
            let mut gui = session_gui();
            key(&mut gui, KeyCode::Char('G'));
            land_main(&mut gui, fallback_answer());
            draw_sized(&mut gui, 100, 24);
            let tips = super::tips(&gui);
            let want = format!("{} · {}", rust_i18n::t!("gui.tips.discover_fallback"), rust_i18n::t!("gui.tips.discover_save"));
            assert_eq!(tips, want, "{code}: the fallback tip");
            assert!(crate::kit::width(&tips) <= 99, "{code} (fallback): {} cells: {tips}", crate::kit::width(&tips));
            key(&mut gui, KeyCode::Char('s'));
            assert_eq!(gui.app.sonic_playlist_name.as_deref(), Some(""), "{code}: and `s` saves there");
        }
    }

    #[test]
    fn the_connected_servers_tab_has_play_queue_all_and_shuffle_and_no_save() {
        let _en = english();
        let peers = vec!["Music/p1.mp3", "Music/p2.mp3"];
        for by_key in [false, true] {
            let mut gui = answered_gui();
            gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
            let rows = draw(&mut gui);
            let all = rows.join("\n");
            assert!(room(&rows, 16).starts_with("▸ play · + queue all · ⇄ shuffle"), "{all}");
            assert!(!all.contains("save as"), "no save on this tab:\n{all}");
            if by_key {
                key(&mut gui, KeyCode::Char('p'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "▸ play"), DiscAct::PlayAll);
            }
            assert_eq!(queued_paths(&gui), peers, "{by_key}: play");
            assert!(gui.app.queue.items.iter().all(|q| q.origin.peer == Some(7)), "through the peer");
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('A'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "+ queue all"), DiscAct::QueueAll);
            }
            assert_eq!(gui.app.queue.items.len(), 4, "{by_key}: queue all");
            let rows = draw(&mut gui);
            if by_key {
                key(&mut gui, KeyCode::Char('S'));
            } else {
                assert_eq!(click_verb(&mut gui, &rows, "⇄ shuffle"), DiscAct::Shuffle);
            }
            let mut shuffled = queued_paths(&gui);
            shuffled.sort_unstable();
            assert_eq!(shuffled, peers, "{by_key}: shuffle");
            // `s` keeps its global meaning here: the shuffle mode toggles,
            // and no prompt opens.
            let mode = gui.app.queue.shuffle;
            key(&mut gui, KeyCode::Char('s'));
            assert_eq!(gui.app.queue.shuffle, !mode, "{by_key}: `s` is the shuffle mode's toggle off the Library tab");
            assert!(gui.app.sonic_playlist_name.is_none() && gui.app.playlist_save_files.is_none(), "no save from this tab");
            gui.act(Act::Disc(DiscAct::SavePlaylist));
            assert!(gui.app.sonic_playlist_name.is_none(), "nor by its act");
        }
    }

    /// Frames for eyeballing: `cargo test dump_discover_frames -- --ignored
    /// --nocapture` (the shell's dump_* habit).
    #[test]
    #[ignore]
    fn dump_discover_frames() {
        let _en = english();
        let mut gui = answered_gui();
        gui.ui.pointer = Some(Position { x: 30, y: 19 });
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
        // The album strip (the 2026-10-11 strip log): eight albums, covers
        // as the mosaic (no pixels under TestBackend) and the note tile,
        // the last tile cut at the right edge; a hovered tile's `[+]`; the
        // hand on the strip, scrolled; the queue open; the floor's rows.
        let mut gui = strip_gui();
        gui.ui.pointer = Some(Position { x: 32, y: 10 });
        println!("{}", draw(&mut gui).join("\n"));
        gui.ui.pointer = None;
        key(&mut gui, KeyCode::Down);
        for _ in 0..6 {
            key(&mut gui, KeyCode::Right);
        }
        println!("{}", draw(&mut gui).join("\n"));
        gui.queue_open = true;
        println!("{}", draw(&mut gui).join("\n"));
        println!("{}", draw_sized(&mut gui, 100, 24).join("\n"));
        // A width whose whole tiles end at the edge: the half tile's cut.
        let mut gui = strip_gui();
        println!("{}", draw_sized(&mut gui, 110, 30).join("\n"));
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
        let plain = layout(content, false, false);
        assert_eq!((plain.tabs_y, plain.seeds_y, plain.verbs_y, plain.table_y), (5, 6, 8, 9));
        assert_eq!(content.bottom() - (plain.table_y + 2), 7, "seven rows with the footer on (clause 90)");
        let card = layout(content, true, false);
        assert_eq!(card.card.map(|(r, compact)| (r.height, compact)), Some((5, true)), "the card gives up its body");
        assert!(content.bottom() > card.table_y + 2, "the table keeps a row: {card:?}");
        // The card stands whole from 18 rows, as it did with the bottom row.
        let whole = |height: u16| layout(Rect { height, ..content }, true, false).card.is_some_and(|(_, compact)| !compact);
        assert!(!whole(17) && whole(18));
        let tall = layout(Rect { height: 23, ..content }, true, false);
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
        let header = &rows[17];
        for head in ["TITRE", "ARTISTE", "ALBUM", "DURÉE"] {
            assert!(header.contains(head), "{head}:\n{header}");
        }
        assert_eq!(at(&rows, "Aurora").x, at(&rows, "TITRE").x, "the title under its heading:\n{}", rows.join("\n"));
        assert_eq!(at(&rows, "Ray").x, at(&rows, "ARTISTE").x, "the artist under its heading");
        assert!(rows[16].contains("▸ lire · + tout en file · ⇄ aléatoire · enregistrer comme playlist…"), "the verbs line:\n{}", rows[16]);
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
        assert_eq!(find(19, "R"), find(17, "ア"), "ARTIST in cells, not chars");
        let album = (find(17, "ア") + 1..100).find(|x| buffer[(*x, 17)].symbol() == "ア").expect("アルバム");
        assert_eq!(find(19, "D"), album, "ALBUM in cells, not chars");
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
        gui.ui.pointer = Some(Position { x: 30, y: 19 });
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].trim_end().ends_with("Last 30 days v"), "the dropdown's caret:\n{}", rows[2]);
        assert!(rows[19].contains("[>] [^] [+] [.]"), "the hover verbs:\n{}", rows[19]);
        assert!(rows[16].contains("> play · + queue all · shuffle · save as playlist"), "the verbs line's glyphs:\n{}", rows[16]);
        assert!(rows[21].contains(" - "), "the missing artist:\n{}", rows[21]);
        gui.act(Act::Disc(DiscAct::Seeds));
        let modal = draw(&mut gui).join("\n");
        assert!(modal.contains("[x] Manor"), "the checkmark's stand-in:\n{modal}");
        gui.act(Act::Disc(DiscAct::SeedsDone));
        for glyph in ['▾', '▸', '»', '⋯', '▇', '—', '–', '✓', '♪'] {
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
        assert!(!super::tips(&gui).contains("played lately"), "no checkbox, no key: {}", super::tips(&gui));
        // At rest the Library tab names its save (clause 64) after the rest
        // words (no frame drawn yet: no strip).
        assert_eq!(super::tips(&gui), "↓ pick a row · ← → tabs · w window · e seeds · G ask again · s save as playlist");
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), "Enter open the album · a queue · ← → tabs · Esc done", "an album row");
        key(&mut gui, KeyCode::Down);
        assert!(super::tips(&gui).contains("m actions"), "a song row");
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Servers)));
        assert!(!super::tips(&gui).contains("save"), "no save on the Connected servers tab: {}", super::tips(&gui));
        key(&mut gui, KeyCode::Down);
        let tips = super::tips(&gui);
        assert!(tips.contains("P play now") && tips.contains("m actions"), "a connected server's row has the sheet: {tips}");
        key(&mut gui, KeyCode::Char('w'));
        assert_eq!(super::tips(&gui), "↑ ↓ rows · Enter pick · Esc close");
    }
}
