//! The Discover room: the history-seeded page — what you finished, and what
//! sounds like it in your library, on the discovery network and on your
//! federated peers — worn terminal on the shared `App` state machine
//! (`App.taste`, src/tui/app/taste.rs), the sonic room's precedent: the
//! room draws the App's row model and calls its verbs, and never touches
//! the TUI's Discover tab, its panel or its capture (clause 67).
//!
//! Contract: docs/ux-contracts/discovery-page.md (clause numbers below cite
//! it); design of record: mStream's docs/discovery-page-plan.md, with the
//! canvas boards `Tui`, `TuiNetwork`, `TuiPeers` and `TuiEmpty` for the
//! picture only. Where the plan and the boards part, the plan is followed
//! and the contract's log says why — album ROWS ahead of the songs, not
//! twelve-by-six cards; the filter at the bottom row's left, not the tab
//! row's right; no STATE column, no "new artists only", no peer `[⋯]` in v1.
//!
//! One answer, three tabs, top to bottom (clause 90): the header with the
//! window dropdown, the state line, the fallback card while nothing is
//! finished yet, the tabs, the seeds line, the tab's table, the bottom row
//! with the library filter and the whole-list verbs. The seeds modal and
//! the hanging lists (the two dropdowns, a lead's menu) draw last, from
//! [`draw_modals`], so they win the pointer.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{SeedSource, TasteFilter, TasteLead, TasteOutcome, Track};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{ListView, dim, frame_at, modal_close, modal_frame_on, scroll_list, width};
use crate::tui::app::{
    App, Capture, TasteLatch, TasteLinks, TastePinRefusal, TasteRow, TasteSlot, TasteTab,
    TasteVerb,
};

use super::opener::{HandOff, open_target};
use super::{Act, Gui, List, RowVerb, Screen, accent, bright_bold, bullet_glyph, check_glyphs, put, sel};

/// The window dropdown's choices, in days (0 = all time) — clause 15.
const WINDOWS: [u32; 4] = [7, 30, 90, 0];

/// The window the header shows before the server or the prefs said one:
/// the server's own default (plan §2.2).
const WINDOW_DEFAULT: u32 = 30;

/// The library filter's choices, in the dropdown's order (clause 65).
const FILTERS: [TasteFilter; 3] = [TasteFilter::Unplayed, TasteFilter::Fresh, TasteFilter::All];

/// MATCH: the meter's cell, a space, the percentage — `▇ 83%`.
const MATCH_W: u16 = 6;
/// The kit table's column gap.
const GAP: u16 = 2;
/// A TRACK (or LEAD) narrower than this gives up the third column first
/// (clause 54).
const TRACK_MIN: u16 = 24;
/// The third column's cap: "LAST PLAYED" and its ages, a server's name.
const THIRD_MAX: u16 = 16;
/// The tabs' spacing — the kit's Tabs, the admin rooms' value.
const TAB_GAP: u16 = 2;
/// The fallback card's widest, and its two heights: whole, and with its
/// body sentence and blank given up at the floor (clause 90).
const CARD_W: u16 = 66;
const CARD_FULL: u16 = 7;
const CARD_COMPACT: u16 = 5;

// ── State ───────────────────────────────────────────────────────────────────

/// The room's own state: the table's cursor and wheel, the hanging list or
/// the seeds modal while one is up, and where the controls they hang from
/// stood last frame. Everything the room shows lives on the shared App.
pub(crate) struct DiscoverUi {
    /// The keyboard cursor, an index into [`lines`] of the tab in front.
    /// None is the kit's resting state — ↓ picks it up, Esc stows it
    /// (clause 57).
    pub cursor: Option<usize>,
    pub view: ListView,
    pub menu: Option<Menu>,
    /// A lead menu's viewport: plug-ins can list more links than the frame
    /// holds, and the wheel scrolls an open menu (clause 58).
    menu_view: ListView,
    pub seeds: Option<SeedsModal>,
    /// The header's window control and the bottom row's filter control as
    /// last drawn: the dropdowns hang from them.
    window_at: Rect,
    filter_at: Rect,
    /// The table rows drawn last frame, by line: a lead's menu hangs from
    /// its row.
    rows_at: Vec<(usize, Rect)>,
    /// Link hand-offs out on their threads: the OS opener can take two
    /// seconds to say whether anything took the link.
    opening: Vec<Receiver<Opened>>,
    /// The local zone for "today" and "yesterday", read once.
    zone: Option<Option<crate::admin::tz::Zone>>,
}

impl DiscoverUi {
    pub(crate) fn new() -> Self {
        DiscoverUi {
            cursor: None,
            view: ListView::default(),
            menu: None,
            menu_view: ListView::default(),
            seeds: None,
            window_at: Rect::default(),
            filter_at: Rect::default(),
            rows_at: Vec::new(),
            opening: Vec::new(),
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
    /// The bottom row's filter list; the cursor indexes [`FILTERS`].
    Filter(usize),
    /// A lead's menu (clause 63), hung from table line `line`, for network
    /// lead `lead`; the cursor indexes its actionable items. `tag` is the
    /// lead's identity ([`lead_tag`]): the index alone would re-target the
    /// menu when a new answer lands under it ([`settle_menu`]).
    Lead { line: usize, lead: usize, cursor: usize, tag: u64 },
}

/// A lead's identity across answers — its key, hashed so [`Menu`] stays
/// `Copy`.
fn lead_tag(lead: &TasteLead) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    lead.key().hash(&mut hasher);
    hasher.finish()
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
    /// A song's `[⋯]` (the track-actions sheet) or a lead's (its menu), and
    /// a right click on either.
    More(usize),
    /// The header's window control, and a pick from its list.
    Window,
    WindowPick(u32),
    /// The bottom row's filter control, and a pick from its list.
    Filter,
    FilterPick(TasteFilter),
    /// A lead menu's actionable item, by its index among them.
    MenuPick(usize),
    /// A click outside an open list closes and keeps (the Dropdown law).
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
    /// Retry and Check again: ask again now (clauses 81, 83).
    Again,
    /// The bottom row's whole-list verbs (clause 64).
    PlayAll,
    QueueAll,
    Shuffle,
}

/// One link hand-off's outcome, for the note line.
type Opened = (String, String, Result<HandOff, String>);

// ── The row model ───────────────────────────────────────────────────────────

/// One line of the tab's table, by what it points at in the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line {
    /// `library.albums[i]`: a container row (clause 61).
    Album(usize),
    /// `library.tracks[i]`.
    Song(usize),
    /// `network.results[i]`.
    Lead(usize),
    /// `peers.peers[g]`'s group line — not a row (clause 71).
    Group(usize),
    /// `peers.peers[group].results[row]`.
    Peer { group: usize, row: usize },
    /// The blank between two peers' groups.
    Gap,
}

impl Line {
    /// Whether the cursor can stand here (group lines and gaps are skipped).
    fn is_row(self) -> bool {
        !matches!(self, Line::Group(_) | Line::Gap)
    }
}

/// The room's gate (clause 1): the ping's two flags, and not a peer's
/// session — a guest has no history there.
pub(crate) fn gate(app: &App) -> bool {
    app.capabilities.taste() && app.session.peer.is_none()
}

/// The tabs the room draws (clause 40): Library always; Network while the
/// server advertises it and its half was not answered `null` for a real
/// seed; Peers likewise.
pub(crate) fn visible_tabs(app: &App) -> Vec<TasteTab> {
    let mut tabs = vec![TasteTab::Library];
    if app.capabilities.discovery_p2p && !app.taste.network.absent {
        tabs.push(TasteTab::Network);
    }
    if app.capabilities.federation_discovery && !app.taste.peers.absent {
        tabs.push(TasteTab::Peers);
    }
    tabs
}

/// The tab's table, in the server's order: albums before songs (clause 51),
/// one group per peer with its playable rows under it (clause 70).
pub(crate) fn lines(app: &App, tab: TasteTab) -> Vec<Line> {
    let taste = &app.taste;
    match tab {
        TasteTab::Library => taste
            .library
            .data
            .iter()
            .flat_map(|l| (0..l.albums.len()).map(Line::Album).chain((0..l.tracks.len()).map(Line::Song)))
            .collect(),
        TasteTab::Network => {
            taste.network.data.iter().flat_map(|n| (0..n.results.len()).map(Line::Lead)).collect()
        }
        TasteTab::Peers => {
            let mut out = Vec::new();
            for (g, group) in taste.peers.data.iter().flat_map(|p| p.peers.iter()).enumerate() {
                if g > 0 {
                    out.push(Line::Gap);
                }
                out.push(Line::Group(g));
                // An unreachable peer is a dim line and no rows (clause 70;
                // the kept-from-last-visit cache is out of scope).
                if group.reachable {
                    for (row, lead) in group.results.iter().enumerate() {
                        if lead.filepath.is_some() {
                            out.push(Line::Peer { group: g, row });
                        }
                    }
                }
            }
            out
        }
    }
}

/// The tab's count (clause 40): Library albums + songs, Network leads,
/// Peers playable songs.
fn count(app: &App, tab: TasteTab) -> usize {
    let taste = &app.taste;
    match tab {
        TasteTab::Library => taste.library.data.as_ref().map_or(0, |l| l.albums.len() + l.tracks.len()),
        TasteTab::Network => taste.network.data.as_ref().map_or(0, |n| n.results.len()),
        TasteTab::Peers => taste.peer_rows().len(),
    }
}

/// Whether a tab's slot holds rows the whole-list verbs can play.
fn playable(app: &App, tab: TasteTab) -> bool {
    match tab {
        TasteTab::Library => app.taste.library.data.as_ref().is_some_and(|l| !l.tracks.is_empty()),
        TasteTab::Network => false,
        TasteTab::Peers => !app.taste.peer_rows().is_empty(),
    }
}

/// What a tab's count reads on the tabs row (clause 40): none while its
/// half is out, `–` when it was not asked, else the number.
fn count_word<T>(slot: &TasteSlot<T>, n: usize) -> Option<String> {
    if slot.pending {
        return None;
    }
    if slot.not_asked_random || (!slot.asked && slot.data.is_none()) {
        return Some(if legacy_conhost() { "-" } else { "–" }.to_string());
    }
    slot.data.as_ref().map(|_| n.to_string())
}

// ── Words ───────────────────────────────────────────────────────────────────

/// The plural form a count takes in the running locale — the DJ room's
/// `songs_words` rule, for the two languages here with more than one plural.
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

/// "%{answered} of %{count} peers answered", whose form follows the total.
fn answered_words(answered: u64, total: u64) -> String {
    let key = format!("gui.discover.answered_{}", plural_form(total));
    t!(key.as_str(), answered = answered, count = total).to_string()
}

/// A relative age on the local clock (clause 51): today, yesterday, n days,
/// months or years ago. `offset` is the zone's seconds east of UTC.
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

fn filter_label(filter: TasteFilter, stale_days: u32) -> String {
    match filter {
        TasteFilter::Unplayed => t!("gui.discover.filter_unplayed").to_string(),
        TasteFilter::All => t!("gui.discover.filter_all").to_string(),
        TasteFilter::Fresh | TasteFilter::Other => plural("gui.discover.filter_fresh", u64::from(stale_days)),
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

/// What the header says the answer is based on (clause 11).
fn based_on(app: &App) -> Option<String> {
    let taste = &app.taste;
    if taste.no_history {
        return Some(t!("gui.discover.nothing_finished").to_string());
    }
    let head = taste.head.as_ref()?;
    Some(match head.seed_source {
        SeedSource::History => {
            let n = head.seeds.iter().filter(|seed| !seed.pinned).count() as u64;
            plural("gui.discover.based_on", n)
        }
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
/// (clause 41): the state is never clipped, the facts drop from the right.
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
        TasteTab::Network => (taste.network.pending, taste.network.failed.is_some()),
        TasteTab::Peers => (taste.peers.pending, taste.peers.failed.is_some()),
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
    let n = count(app, tab) as u64;
    let mut facts = Vec::new();
    match tab {
        TasteTab::Library => {}
        TasteTab::Network => {
            if let Some(network) = &taste.network.data {
                facts.push(plural("gui.discover.searched", u64::from(network.searched.peers)));
            }
        }
        TasteTab::Peers => {
            if let Some(peers) = &taste.peers.data {
                let total = u64::from(peers.searched.peers);
                let answered = total.saturating_sub(u64::from(peers.searched.unreachable));
                facts.push(answered_words(answered, total));
                facts.push(word("gui.discover.through_server"));
            }
        }
    }
    facts.extend(widened(app));
    if n == 0 {
        return (word("gui.discover.state_empty"), grey, facts);
    }
    let state = match tab {
        TasteTab::Library => plural("gui.discover.similar", n),
        TasteTab::Network => plural("gui.discover.leads", n),
        TasteTab::Peers => plural("gui.discover.playable", n),
    };
    (state, ok, facts)
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

/// The table's columns in `list` (clause 50): MATCH, the title, the third
/// column (LAST PLAYED · FOUND ON · ON) while the title keeps 24 cells
/// (clause 54), LENGTH flush right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cols {
    pub track_x: u16,
    pub track_w: u16,
    pub third: Option<(u16, u16)>,
    pub len_x: u16,
    pub len_w: u16,
}

/// `match_w` is MATCH's width: the meter's [`MATCH_W`], or the heading's
/// when a locale's is wider ("PROXIMITÉ", "AFINIDADE"), so the kit table's
/// 2-cell gap stands in every language.
pub(crate) fn columns(list: Rect, match_w: u16, third_w: u16, len_w: u16) -> Cols {
    let track_x = list.x + match_w.max(MATCH_W) + GAP;
    let len_x = list.right().saturating_sub(len_w);
    let with_third = len_x.saturating_sub(GAP + third_w + GAP + track_x);
    if with_third >= TRACK_MIN {
        Cols { track_x, track_w: with_third, third: Some((len_x - GAP - third_w, third_w)), len_x, len_w }
    } else {
        Cols { track_x, track_w: len_x.saturating_sub(GAP + track_x), third: None, len_x, len_w }
    }
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

/// Answers that landed since the last frame: link hand-offs back from their
/// threads, and a refused pick, worded once (clause 14).
fn settle(gui: &mut Gui) {
    let mut still = Vec::new();
    for rx in std::mem::take(&mut gui.discover.opening) {
        match rx.try_recv() {
            Ok((label, url, outcome)) => {
                let (words, failed) = match outcome {
                    Ok(HandOff::Launched) => (t!("note.link_opened", label = label), false),
                    Ok(HandOff::Headless(_)) => (t!("note.link_headless", label = label, url = url), false),
                    Ok(HandOff::Nothing) => (t!("gui.discover.open_nothing", label = label), true),
                    Err(err) => (t!("gui.discover.open_failed", err = err), true),
                };
                gui.note = Some((words.to_string(), failed));
            }
            Err(TryRecvError::Empty) => still.push(rx),
            Err(TryRecvError::Disconnected) => {}
        }
    }
    gui.discover.opening = still;
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

/// A dropdown's control: the value in the row's own weight, the chevron
/// dim, both BRIGHT + BOLD under the pointer (the kit's Dropdown).
fn dropdown_control(
    frame: &mut Frame,
    gui: &mut Gui,
    (x, y): (u16, u16),
    label: &str,
    act: DiscAct,
    tip: String,
) -> Rect {
    let w = width(label) as u16;
    let rect = Rect { x, y, width: w + 2, height: 1 };
    let hover = gui.ui.hovers(rect);
    put(frame, x, y, label, if hover { bright_bold() } else { Style::default() });
    put(frame, x + w + 1, y, chevron(), if hover { bright_bold() } else { dim() });
    gui.ui.click(rect, Act::Disc(act));
    gui.ui.tip_keyed(rect, tip);
    rect
}

/// The header: the room's name BOLD, what the answer is based on DIM, the
/// window dropdown at the right (clauses 11, 15). The phrase clips before
/// the control moves (clause 91).
fn draw_header(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let name = t!("gui.nav.discover").to_string();
    put(frame, content.x, y, &name, Style::default().add_modifier(Modifier::BOLD));
    let window = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
    let label = window_label(window);
    let control_w = width(&label) as u16 + 2;
    let control_x = content.right().saturating_sub(control_w);
    let tip = t!("gui.discover.window_tip").to_string();
    gui.discover.window_at = dropdown_control(frame, gui, (control_x, y), &label, DiscAct::Window, tip);
    if let Some(phrase) = based_on(&gui.app) {
        let x = content.x + width(&name) as u16 + 1;
        let room = control_x.saturating_sub(x + 2) as usize;
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
/// buttons, each with its count; the pane's note at the right edge, dropped
/// when the row is too narrow (clause 42).
fn draw_tabs(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let tabs = visible_tabs(&gui.app);
    let mut x = content.x;
    for tab in tabs {
        let (name, slot_count) = {
            let taste = &gui.app.taste;
            let n = count(&gui.app, tab);
            match tab {
                TasteTab::Library => (t!("gui.discover.tab_library"), count_word(&taste.library, n)),
                TasteTab::Network => (t!("gui.discover.tab_network"), count_word(&taste.network, n)),
                TasteTab::Peers => (t!("gui.discover.tab_peers"), count_word(&taste.peers, n)),
            }
        };
        let label = match slot_count {
            Some(n) => format!(" {name} {n} "),
            None => format!(" {name} "),
        };
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
    let note = match gui.app.taste.tab {
        TasteTab::Library => return,
        TasteTab::Network => t!("gui.discover.note_network").to_string(),
        TasteTab::Peers => t!("gui.discover.note_peers").to_string(),
    };
    let w = width(&note) as u16;
    if x + 2 + w <= content.right() {
        put(frame, content.right() - w, y, &note, dim());
    }
}

/// The seeds line (clause 12): what the answer starts from, pins first,
/// `+n` for the rest, then `edit`. It never wraps and never scrolls.
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
    let mut x = content.x + width(&label) as u16 + 2;
    let total = seeds.len();
    for (i, (words, marker)) in seeds.iter().enumerate() {
        let piece_w = width(words) as u16 + marker.as_ref().map_or(0, |m| width(m) as u16 + 1);
        // What must still fit after this seed: the `+n` for the rest and
        // `edit`.
        let more = total - i - 1;
        let more_w = if more > 0 { width(&t!("gui.discover.seeds_more", count = more)) as u16 + 2 } else { 0 };
        let tail = more_w + edit_w + 2;
        if x + piece_w + tail > content.right() {
            let rest = total - i;
            let plus = t!("gui.discover.seeds_more", count = rest).to_string();
            put(frame, x, y, &plus, dim());
            x += width(&plus) as u16 + 2;
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
struct Cells {
    similarity: f64,
    title: String,
    /// DIM beside the title: an album row's `album`.
    tag: Option<String>,
    third: String,
    /// The length is the kit's DIM `—` for a missing value.
    length: String,
    playing: bool,
    tip: Option<String>,
    verbs: Verbs,
}

/// The hover verbs a row carries (clauses 60–63).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verbs {
    /// An album: a container, no verbs (clause 61).
    None,
    /// A library song: `[▸] [»] [+] [⋯]`.
    Song,
    /// A peer's song: `[▸] [»] [+]` (clause 62).
    Peer,
    /// A lead: `[⋯]` alone (clause 63).
    Lead,
}

/// LAST PLAYED: the relative age, or "never" for a song never played.
fn ago(iso: Option<&str>, now: i64, offset: i32) -> String {
    match iso.and_then(crate::admin::iso_unix) {
        Some(then) => age_words(then, now, offset),
        None => t!("gui.discover.never").to_string(),
    }
}

fn sounds_like(app: &App, hash: Option<&str>) -> Option<String> {
    let seed = app.taste.seed(hash?)?;
    Some(t!("gui.discover.seed_tip", track = track_words(&seed.to_track())).to_string())
}

fn cells(app: &App, line: Line, now: i64, offset: i32) -> Option<Cells> {
    let taste = &app.taste;
    // The playing track by its path AND its source (clause 55): a peer's
    // "Music/x.mp3" is not the library's song of that path. The queue's
    // current row says which peer, if any, it streams from.
    let playing_peer = app.queue.current.and_then(|i| app.queue.items.get(i)).map(|q| q.origin.peer);
    let playing = |path: &str, peer: Option<i64>| {
        app.now_playing.as_ref().is_some_and(|t| t.filepath == path) && playing_peer.is_none_or(|p| p == peer)
    };
    let time = |seconds: Option<f64>| seconds.filter(|s| *s > 0.0).map(super::bar::fmt_time);
    match line {
        Line::Album(i) => {
            let album = taste.library.data.as_ref()?.albums.get(i)?;
            let name = album.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| dash().to_string());
            let last = if album.played_tracks == 0 {
                t!("gui.discover.never").to_string()
            } else {
                ago(album.last_played.as_deref(), now, offset)
            };
            Some(Cells {
                similarity: album.similarity,
                title: title_artist(&name, album.artist()),
                tag: Some(t!("gui.discover.album").to_string()),
                third: last,
                length: plural("gui.discover.songs", u64::from(album.scoped_tracks)),
                playing: false,
                tip: Some(t!("gui.discover.album_tip").to_string()),
                verbs: Verbs::None,
            })
        }
        Line::Song(i) => {
            let song = taste.library.data.as_ref()?.tracks.get(i)?;
            let track = song.to_track();
            Some(Cells {
                similarity: song.similarity,
                title: track_words(&track),
                tag: None,
                third: ago(song.last_played.as_deref(), now, offset),
                length: time(track.metadata.duration).unwrap_or_else(|| dash().to_string()),
                playing: playing(&song.filepath, None),
                tip: sounds_like(app, song.seed_hash.as_deref()),
                verbs: Verbs::Song,
            })
        }
        Line::Lead(i) => {
            let lead = taste.lead(i)?;
            Some(Cells {
                similarity: lead.similarity,
                title: title_artist(lead.title.as_deref().unwrap_or(dash()), lead.artist.as_deref()),
                tag: None,
                third: lead.peer.name.clone().unwrap_or_else(|| dash().to_string()),
                length: time(lead.duration).unwrap_or_else(|| dash().to_string()),
                playing: false,
                tip: sounds_like(app, lead.seed_hash.as_deref()),
                verbs: Verbs::Lead,
            })
        }
        Line::Peer { group, row } => {
            let group = taste.peers.data.as_ref()?.peers.get(group)?;
            let lead = group.results.get(row)?;
            Some(Cells {
                similarity: lead.similarity,
                title: title_artist(lead.title.as_deref().unwrap_or(dash()), lead.artist.as_deref()),
                tag: None,
                third: group.name.clone(),
                length: time(lead.duration).unwrap_or_else(|| dash().to_string()),
                playing: lead.filepath.as_deref().is_some_and(|path| playing(path, Some(group.id))),
                tip: None,
                verbs: Verbs::Peer,
            })
        }
        Line::Group(_) | Line::Gap => None,
    }
}

/// A peer's group line (clause 70): the state grammar's `•` and name, then
/// its facts — Green online, DarkGray and the whole group dim when it is
/// not answering, Yellow for another sonic model.
fn group_words(app: &App, g: usize, now: i64, offset: i32) -> Option<(String, Style, String)> {
    let group = app.taste.peers.data.as_ref()?.peers.get(g)?;
    let name = format!("{} {}", bullet_glyph(), group.name);
    if !group.reachable {
        let mut facts = t!("gui.discover.peer_unreachable").to_string();
        if let Some(then) = group.last_seen.as_deref().and_then(crate::admin::iso_unix) {
            facts.push_str(" · ");
            facts.push_str(&t!("gui.discover.peer_last_seen", age = age_words(then, now, offset)));
        }
        return Some((name, dim(), facts));
    }
    if group.mismatched {
        return Some((name, Style::default().fg(th().gold), t!("gui.discover.peer_mismatched").to_string()));
    }
    let songs = group.results.iter().filter(|r| r.filepath.is_some()).count() as u64;
    let facts = if songs == 0 {
        format!("{} · {}", t!("gui.discover.peer_online"), t!("gui.discover.state_empty"))
    } else {
        format!("{} · {}", t!("gui.discover.peer_online"), plural("gui.discover.songs", songs))
    };
    Some((name, Style::default().fg(th().ok), facts))
}

/// What stands in the pane instead of rows (clauses 22, 80–83), if
/// anything; `failed_over` says the failure line sits over old rows.
enum Pane {
    Rows { failed_over: Option<String> },
    Off,
    ScanPending,
    Asking(String),
    Failed(String),
    Sentence(String),
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
        TasteTab::Network => slot_facts(&taste.network),
        TasteTab::Peers => slot_facts(&taste.peers),
    };
    let asking = match tab {
        TasteTab::Peers => t!("gui.discover.asking_peers").to_string(),
        _ => t!("gui.discover.asking_library").to_string(),
    };
    if first_wait || (taste.latch == TasteLatch::Probing && !has_data) {
        return Pane::Asking(asking);
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
        let sentence = match tab {
            TasteTab::Library => t!("gui.discover.empty_library").to_string(),
            TasteTab::Network => {
                let peers = taste.network.data.as_ref().map_or(0, |n| n.searched.peers);
                if peers == 0 {
                    t!("gui.discover.empty_network_warming").to_string()
                } else {
                    t!("gui.discover.empty_network").to_string()
                }
            }
            TasteTab::Peers => t!("gui.discover.no_peers").to_string(),
        };
        return Pane::Sentence(sentence);
    }
    Pane::Rows { failed_over: failed }
}

fn slot_facts<T>(slot: &TasteSlot<T>) -> (bool, Option<String>, bool, bool) {
    (slot.first_wait(), slot.failed.clone(), slot.not_asked_random, slot.data.is_some())
}

/// The kit's scan widget: the all-dim bar and a present-progressive line.
fn draw_scan(frame: &mut Frame, x: u16, y: u16, room: usize, sentence: &str) {
    let bar = if legacy_conhost() { "·".repeat(10) } else { "▱".repeat(10) };
    put(frame, x, y, &bar, dim());
    put(frame, x + 11, y, &super::bar::clip(sentence, room.saturating_sub(11)), accent());
}

/// The tab's table: header and rule, then the rows or what stands for them,
/// and the kit scrollbar while the rows overflow.
fn draw_table(frame: &mut Frame, gui: &mut Gui, content: Rect, table_y: u16, bottom_y: u16) {
    gui.discover.rows_at.clear();
    let tab = gui.app.taste.tab;
    let list = Rect {
        x: content.x,
        y: table_y + 2,
        width: content.width.saturating_sub(2),
        height: bottom_y.saturating_sub(table_y + 2),
    };
    let all = lines(&gui.app, tab);
    let now = crate::admin::unix_now();
    let zone = gui.discover.zone.get_or_insert_with(crate::admin::tz::local);
    let offset = zone.as_ref().map_or(0, |z| z.offset_at(now));
    let row_cells: Vec<Option<Cells>> = all.iter().map(|line| cells(&gui.app, *line, now, offset)).collect();

    let head = |key: &str| t!(key).to_uppercase();
    let third_head = match tab {
        TasteTab::Library => head("gui.discover.col_last"),
        TasteTab::Network => head("gui.discover.col_found"),
        TasteTab::Peers => head("gui.discover.col_on"),
    };
    let len_head = head("gui.discover.col_length");
    let widest = |pick: fn(&Cells) -> &str, floor: &str| {
        row_cells.iter().flatten().map(|c| width(pick(c))).chain([width(floor)]).max().unwrap_or(0) as u16
    };
    let third_w = widest(|c| c.third.as_str(), &third_head).min(THIRD_MAX);
    let len_w = widest(|c| c.length.as_str(), &len_head).min(10);
    let match_head = head("gui.discover.col_match");
    let cols = columns(list, width(&match_head) as u16, third_w, len_w);
    let title_head = match tab {
        TasteTab::Network => head("gui.discover.col_lead"),
        _ => head("gui.discover.col_track"),
    };
    put(frame, list.x, table_y, &match_head, dim());
    put(frame, cols.track_x, table_y, &super::bar::clip(&title_head, cols.track_w as usize), dim());
    if let Some((x, w)) = cols.third {
        put(frame, x, table_y, &super::bar::clip(&third_head, w as usize), dim());
    }
    put(frame, cols.len_x + len_w.saturating_sub(width(&len_head) as u16), table_y, &len_head, dim());
    put(frame, list.x, table_y + 1, &"─".repeat(content.width as usize), dim());

    let room = list.width as usize;
    let empty = all.iter().all(|line| !line.is_row()) && tab != TasteTab::Peers
        || (tab == TasteTab::Peers && all.is_empty());
    let mut list = list;
    match pane(&gui.app, tab, empty) {
        Pane::Off => {
            let sentence = crate::kit::wrap_words(&t!("gui.discover.turned_off"), room);
            for (i, line) in sentence.into_iter().enumerate().take(list.height as usize) {
                put(frame, list.x, list.y + i as u16, &line, Style::default().fg(th().gold));
            }
            return;
        }
        Pane::ScanPending => {
            let lines = crate::kit::wrap_words(&t!("gui.discover.scan_pending"), room);
            let shown = lines.len().min(list.height.saturating_sub(1) as usize);
            for (i, line) in lines.into_iter().take(shown).enumerate() {
                put(frame, list.x, list.y + i as u16, &line, dim());
            }
            let label = t!("gui.discover.check_again").to_string();
            again_button(frame, gui, list.x, list.y + shown as u16, &label);
            return;
        }
        Pane::Asking(sentence) => {
            draw_scan(frame, list.x, list.y, room, &sentence);
            return;
        }
        Pane::Failed(err) => {
            failure_line(frame, gui, list, &err);
            return;
        }
        Pane::Sentence(sentence) => {
            let sentence = crate::kit::wrap_words(&sentence, room);
            for (i, line) in sentence.into_iter().enumerate().take(list.height as usize) {
                put(frame, list.x, list.y + i as u16, &line, dim());
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
    // A new answer can leave the cursor past the rows or on a group line:
    // the hand goes down rather than lighting nothing in particular.
    if gui.discover.cursor.is_some_and(|c| all.get(c).is_none_or(|line| !line.is_row())) {
        gui.discover.stow();
    }
    let shown = gui.discover.view.shown(gui.discover.cursor);
    let (first, visible) = gui.discover.view.window(len, gui.discover.cursor, list.height as usize);
    for (row, index) in (first..(first + visible).min(len)).enumerate() {
        let y = list.y + row as u16;
        let rect = Rect { x: list.x, y, width: list.width, height: 1 };
        match all[index] {
            Line::Gap => {}
            Line::Group(g) => {
                if let Some((name, style, facts)) = group_words(&gui.app, g, now, offset) {
                    put(frame, list.x, y, &name, style.add_modifier(Modifier::BOLD));
                    let x = list.x + width(&name) as u16;
                    let room = list.right().saturating_sub(x) as usize;
                    put(frame, x, y, &super::bar::clip(&format!(" · {facts}"), room), dim());
                }
            }
            line => {
                if let Some(cells) = &row_cells[index] {
                    draw_row(frame, gui, rect, cols, (index, line), cells, shown == Some(index));
                    gui.discover.rows_at.push((index, rect));
                }
            }
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

/// Retry and Check again: the accent's text button, the room's one way
/// forward where it stands.
fn again_button(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, label: &str) {
    let rect = Rect { x, y, width: width(label) as u16, height: 1 };
    let hover = gui.ui.hovers(rect);
    put(frame, x, y, label, if hover { bright_bold() } else { accent().add_modifier(Modifier::BOLD) });
    gui.ui.click(rect, Act::Disc(DiscAct::Again));
}

/// "couldn't ask for suggestions: {err}" in the warning colour, with Retry
/// at its right (clause 83).
fn failure_line(frame: &mut Frame, gui: &mut Gui, at: Rect, err: &str) {
    let retry = t!("gui.discover.retry").to_string();
    let retry_w = width(&retry) as u16;
    let words = t!("gui.discover.failed", err = err).to_string();
    let room = at.width.saturating_sub(retry_w + 2) as usize;
    let shown = super::bar::clip(&words, room).into_owned();
    put(frame, at.x, at.y, &shown, Style::default().fg(th().gold));
    again_button(frame, gui, at.x + width(&shown) as u16 + 2, at.y, &retry);
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
    // MATCH: the sonic room's one-cell meter and the bare percentage, the
    // percentage alone on the legacy console (clause 50).
    let pct = format!("{:>3}%", (cells.similarity * 100.0).round().clamp(0.0, 100.0) as u32);
    if let Some(glyph) = super::sonic::meter_glyph(cells.similarity) {
        put(frame, rect.x, y, &glyph.to_string(), if is_sel { sel() } else { accent() });
    }
    put(frame, rect.x + 2, y, &pct, if is_sel { sel() } else { dim() });

    let title_style = match (is_sel, cells.playing, hover) {
        (true, _, _) => sel().add_modifier(Modifier::BOLD),
        (false, true, _) => Style::default().fg(th().ok).add_modifier(Modifier::BOLD),
        (false, false, true) => bright_bold(),
        (false, false, false) => Style::default(),
    };
    let row_act = Act::Disc(DiscAct::Row(index, RowVerb::Open));
    // The hover verbs stand at the row's right in place of its last two
    // columns (the Files rows' grammar); a selected row keeps its columns —
    // the keyboard's hand has its own keys.
    let [now, next, queue, more] = verb_glyphs();
    let tip = |key: &str| t!(key).to_string();
    let verbs: Vec<(&str, DiscAct, String)> = match cells.verbs {
        _ if !hover || is_sel => Vec::new(),
        Verbs::None => Vec::new(),
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
        ],
        Verbs::Lead => vec![(more, DiscAct::More(index), tip("gui.act.more_tip"))],
    };
    let verbs_w = (verbs.len() as u16 * 4).saturating_sub(1);
    let title_room = if verbs.is_empty() {
        cols.track_w
    } else {
        rect.right().saturating_sub(verbs_w + GAP).saturating_sub(cols.track_x)
    };
    let tag_w = cells.tag.as_ref().map_or(0, |t| width(t) as u16 + 1);
    let title = super::bar::clip(&cells.title, title_room.saturating_sub(tag_w) as usize).into_owned();
    put(frame, cols.track_x, y, &title, title_style);
    // The playing row wears the house `▸` as in every room (clause 55; the
    // Files rows' marker), in the gap before its title.
    if cells.playing {
        let style = if is_sel { sel() } else { Style::default().fg(th().ok) };
        put(frame, cols.track_x.saturating_sub(GAP), y, super::forward_glyph(), style.add_modifier(Modifier::BOLD));
    }
    let title_w = width(&title) as u16;
    if let Some(tag) = &cells.tag {
        put(frame, cols.track_x + title_w + 1, y, tag, if is_sel { sel() } else { dim() });
    }
    let detail = if is_sel { sel() } else { dim() };
    // The row's click first, its verbs after: the last registered wins.
    gui.ui.click(rect, row_act);
    if matches!(line, Line::Song(_) | Line::Lead(_)) {
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
    if verbs.is_empty() {
        if let Some((x, w)) = cols.third {
            put(frame, x, y, &super::bar::clip(&cells.third, w as usize), detail);
        }
        let len = super::bar::clip(&cells.length, cols.len_w as usize).into_owned();
        put(frame, cols.len_x + cols.len_w.saturating_sub(width(&len) as u16), y, &len, detail);
        return;
    }
    let mut x = rect.right().saturating_sub(verbs_w);
    for (glyph, act, tip) in verbs {
        let cell = Rect { x, y, width: 3, height: 1 };
        put(frame, x, y, glyph, if gui.ui.hovers(cell) { bright_bold() } else { dim() });
        gui.ui.click(cell, Act::Disc(act));
        gui.ui.tip_keyed(cell, tip);
        x += 4;
    }
}

/// The bottom row (clauses 64–65): the library filter's dropdown at the
/// left on the Library tab, the whole-list verbs at the right on a tab
/// that has something to play, dropped from the tail when squeezed — the
/// filter control never moves (clause 91).
fn draw_bottom(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    let tab = gui.app.taste.tab;
    let mut left = content.x;
    gui.discover.filter_at = Rect::default();
    if tab == TasteTab::Library {
        let label = filter_label(gui.app.taste.shown_filter(), gui.app.taste.stale_days());
        let tip = t!("gui.discover.filter_tip").to_string();
        let rect = dropdown_control(frame, gui, (content.x, y), &label, DiscAct::Filter, tip);
        gui.discover.filter_at = rect;
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

// ── The lists and the modal ─────────────────────────────────────────────────

/// One item of a lead's menu (clause 63).
#[derive(Debug, Clone, PartialEq)]
enum MenuItem {
    Head(String),
    /// "asking…", or a failure in the warning colour (`true`).
    Note(String, bool),
    Link { label: String, url: String },
    Copy(String),
}

impl MenuItem {
    fn actionable(&self) -> bool {
        matches!(self, MenuItem::Link { .. } | MenuItem::Copy(_))
    }
}

fn menu_items(app: &App, lead: usize) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if app.taste_offers_links()
        && let Some(row) = app.taste.lead(lead)
    {
        match app.taste.links.get(&row.key()) {
            Some(TasteLinks::Ready(links)) => {
                let web: Vec<_> = links.iter().filter(|l| l.is_web()).collect();
                if !web.is_empty() {
                    items.push(MenuItem::Head(t!("gui.discover.menu_links").to_uppercase()));
                    for link in web {
                        items.push(MenuItem::Link { label: link.label.clone(), url: link.url.clone() });
                    }
                }
            }
            Some(TasteLinks::Failed(err)) => {
                items.push(MenuItem::Head(t!("gui.discover.menu_links").to_uppercase()));
                items.push(MenuItem::Note(t!("gui.discover.links_failed", err = err).to_string(), true));
            }
            Some(TasteLinks::Asking) | None => {
                items.push(MenuItem::Head(t!("gui.discover.menu_links").to_uppercase()));
                items.push(MenuItem::Note(t!("gui.discover.asking").to_string(), false));
            }
        }
    }
    if let Some(text) = app.taste_copy_text(lead) {
        items.push(MenuItem::Head(t!("gui.discover.menu_lead").to_uppercase()));
        items.push(MenuItem::Copy(text));
    }
    items
}

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

/// The lists and the modal, drawn (and registered) after the base screen so
/// their rects win the pointer.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // Every frame, whatever is up: a link handed off before the room was
    // left still says how it went.
    settle(gui);
    if gui.screen != Screen::Library || gui.active != super::DISCOVER_NAV {
        return;
    }
    settle_menu(gui);
    match gui.discover.menu {
        Some(Menu::Window(cursor)) => {
            let current = gui.app.taste.shown_window().unwrap_or(WINDOW_DEFAULT);
            let rows: Vec<(String, bool, DiscAct)> =
                WINDOWS.iter().map(|w| (window_label(*w), *w == current, DiscAct::WindowPick(*w))).collect();
            let anchor = gui.discover.window_at;
            draw_list(frame, gui, (area, anchor), rows, cursor, true);
        }
        Some(Menu::Filter(cursor)) => {
            let current = gui.app.taste.shown_filter();
            let days = gui.app.taste.stale_days();
            let rows: Vec<(String, bool, DiscAct)> = FILTERS
                .iter()
                .map(|f| (filter_label(*f, days), *f == current, DiscAct::FilterPick(*f)))
                .collect();
            let anchor = gui.discover.filter_at;
            draw_list(frame, gui, (area, anchor), rows, cursor, false);
        }
        Some(Menu::Lead { line, lead, cursor, .. }) => draw_lead_menu(frame, gui, area, line, lead, cursor),
        None => {}
    }
    if gui.discover.seeds.is_some() {
        draw_seeds_modal(frame, gui, area);
    }
}

/// A dropdown's list (the kit's Dropdown): hung under its control — over
/// it when the control sits low (the filter, on the bottom row) — the
/// current value wearing `•`, the cursor the accent slab.
fn draw_list(
    frame: &mut Frame,
    gui: &mut Gui,
    (area, anchor): (Rect, Rect),
    rows: Vec<(String, bool, DiscAct)>,
    cursor: usize,
    below: bool,
) {
    gui.ui.click(area, Act::Disc(DiscAct::MenuClose));
    let widest = rows.iter().map(|(label, _, _)| width(label)).max().unwrap_or(10) as u16;
    let w = (widest + 6).max(anchor.width + 4).min(area.width.saturating_sub(2));
    let h = rows.len() as u16 + 2;
    let x = anchor.x.saturating_sub(1).min(area.right().saturating_sub(w + 1));
    let y = if below { anchor.bottom() } else { anchor.y.saturating_sub(h) };
    let rect = Rect { x, y, width: w, height: h };
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

/// A lead's menu (clause 63), hung from its row in the dropdown's grammar.
fn draw_lead_menu(frame: &mut Frame, gui: &mut Gui, area: Rect, line: usize, lead: usize, cursor: usize) {
    gui.ui.click(area, Act::Disc(DiscAct::MenuClose));
    let items = menu_items(&gui.app, lead);
    let label_of = |item: &MenuItem| match item {
        MenuItem::Head(text) | MenuItem::Note(text, _) => text.clone(),
        MenuItem::Link { label, .. } => label.clone(),
        MenuItem::Copy(text) => t!("gui.discover.copy", lead = text).to_string(),
    };
    // An item sits a cell in from its section's head.
    let indent = |i: &MenuItem| usize::from(!matches!(i, MenuItem::Head(_)));
    let widest = items.iter().map(|i| width(&label_of(i)) + indent(i)).max().unwrap_or(10) as u16;
    let w = (widest + 4).clamp(24, 52).min(area.width.saturating_sub(2));
    let h = (items.len() as u16 + 2).min(area.height.saturating_sub(2));
    let anchor = gui
        .discover
        .rows_at
        .iter()
        .find(|(i, _)| *i == line)
        .map(|(_, r)| *r)
        .unwrap_or(Rect { x: area.x, y: area.y + 2, width: area.width, height: 1 });
    let x = anchor.right().saturating_sub(w).max(area.x);
    // Under the row when it fits above the bar, else over it.
    let y = if anchor.bottom() + h <= area.bottom().saturating_sub(super::bar::BAR_ROWS) {
        anchor.bottom()
    } else {
        anchor.y.saturating_sub(h)
    };
    let rect = Rect { x, y, width: w, height: h };
    gui.ui.overlay(rect);
    let inner = frame_at(frame, rect, th().accent);
    // Each item's index among the actionable ones, so a scrolled menu still
    // picks by the same numbers the keys walk.
    let mut picks = Vec::with_capacity(items.len());
    let mut count = 0;
    for item in &items {
        picks.push(item.actionable().then(|| {
            count += 1;
            count - 1
        }));
    }
    // Plug-ins can list more links than the frame holds: the menu scrolls,
    // by the wheel and to the keyboard's cursor (clause 58).
    let lit = picks.iter().position(|p| *p == Some(cursor));
    let (first, visible) = gui.discover.menu_view.window(items.len(), lit, inner.height as usize);
    for (i, item) in items.iter().enumerate().skip(first).take(visible) {
        let row = Rect { x: inner.x, y: inner.y + (i - first) as u16, width: inner.width, height: 1 };
        let text = super::bar::clip(&label_of(item), inner.width.saturating_sub(2) as usize).into_owned();
        match item {
            MenuItem::Head(_) => put(frame, row.x + 1, row.y, &text, dim()),
            MenuItem::Note(_, warn) => {
                let style = if *warn { Style::default().fg(th().gold) } else { accent() };
                put(frame, row.x + 2, row.y, &text, style);
            }
            MenuItem::Link { .. } | MenuItem::Copy(_) => {
                let actionable = picks[i].unwrap_or_default();
                let on = actionable == cursor;
                let hover = !on && gui.ui.hovers(row);
                if on {
                    frame.render_widget(ratatui::widgets::Block::default().style(sel()), row);
                }
                let style = match (on, hover) {
                    (true, _) => sel().add_modifier(Modifier::BOLD),
                    (false, true) => bright_bold(),
                    (false, false) => Style::default(),
                };
                let text = super::bar::clip(&text, inner.width.saturating_sub(3) as usize).into_owned();
                put(frame, row.x + 2, row.y, &text, style);
                gui.ui.click(row, Act::Disc(DiscAct::MenuPick(actionable)));
            }
        }
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
                if seed.plays > 0 {
                    facts.push(plural("gui.discover.plays", seed.plays));
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

// ── Acting ──────────────────────────────────────────────────────────────────

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
        // Only the pointer sends these (`m` calls `more` itself, keeping
        // the hand up): a click acts on its row and stows (clause 57).
        DiscAct::More(index) => {
            gui.discover.stow();
            more(gui, index);
        }
        DiscAct::Window => {
            gui.discover.menu = match gui.discover.menu {
                Some(Menu::Window(_)) => None,
                _ => {
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
        DiscAct::Filter => {
            gui.discover.menu = match gui.discover.menu {
                Some(Menu::Filter(_)) => None,
                _ if gui.app.taste.tab != TasteTab::Library => None,
                _ => {
                    let current = gui.app.taste.shown_filter();
                    Some(Menu::Filter(FILTERS.iter().position(|f| *f == current).unwrap_or(1)))
                }
            };
        }
        DiscAct::FilterPick(filter) => {
            gui.discover.menu = None;
            let effects = gui.app.set_taste_filter(filter);
            gui.pend(effects);
        }
        DiscAct::MenuPick(i) => menu_pick(gui, i),
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

/// A row's verb (clauses 60–63): songs and peers' rows through the App's
/// own verbs, an album into the Albums room, a lead into its menu.
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
        Some(Line::Lead(_)) if verb == RowVerb::Open => more(gui, index),
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

/// `[⋯]`, `m` or a right click: a song's track-actions sheet, a lead's menu.
fn more(gui: &mut Gui, index: usize) {
    match lines(&gui.app, gui.app.taste.tab).get(index).copied() {
        Some(Line::Song(i)) => {
            let song = gui.app.taste.library.data.as_ref().and_then(|l| l.tracks.get(i));
            let Some(track) = song.map(|t| t.to_track()) else { return };
            let origin = gui.app.origin();
            super::actions::open_for(gui, origin, track, None);
        }
        Some(Line::Lead(lead)) => {
            // The links are asked the first time the menu opens, and kept
            // with the answer (clause 63).
            let effects = gui.app.taste_links_for(lead);
            gui.pend(effects);
            let Some(tag) = gui.app.taste.lead(lead).map(lead_tag) else { return };
            gui.discover.menu = Some(Menu::Lead { line: index, lead, cursor: 0, tag });
            gui.discover.menu_view = ListView::default();
        }
        _ => {}
    }
}

/// A lead's menu follows ITS lead, not a position: a re-ask leaves the
/// rows standing (clause 32), so a menu can be open when the answer lands,
/// and the lead at its index is then another song — whose copy text and
/// links it would offer. Found again, the menu is re-pointed (its links
/// asked anew, the answer having dropped them); gone, it closes.
fn settle_menu(gui: &mut Gui) {
    let Some(Menu::Lead { lead, cursor, tag, .. }) = gui.discover.menu else { return };
    if gui.app.taste.lead(lead).map(lead_tag) == Some(tag) {
        return;
    }
    let found = gui.app.taste.network.data.as_ref().and_then(|n| n.results.iter().position(|l| lead_tag(l) == tag));
    let line = found.and_then(|f| lines(&gui.app, TasteTab::Network).iter().position(|l| *l == Line::Lead(f)));
    match (found, line) {
        (Some(lead), Some(line)) if gui.app.taste.tab == TasteTab::Network => {
            gui.discover.menu = Some(Menu::Lead { line, lead, cursor, tag });
            let effects = gui.app.taste_links_for(lead);
            gui.pend(effects);
        }
        _ => gui.discover.menu = None,
    }
}

fn menu_pick(gui: &mut Gui, i: usize) {
    settle_menu(gui);
    let Some(Menu::Lead { lead, .. }) = gui.discover.menu else { return };
    let items: Vec<MenuItem> = menu_items(&gui.app, lead).into_iter().filter(MenuItem::actionable).collect();
    let Some(item) = items.get(i).cloned() else { return };
    gui.discover.menu = None;
    match item {
        MenuItem::Link { label, url } => open_link(gui, label, url),
        MenuItem::Copy(text) => {
            use crate::kit::clipboard::{Copied, copy};
            let (words, failed) = match copy(&text) {
                Copied::Clipboard | Copied::Terminal => (t!("gui.discover.copied", lead = text), false),
                Copied::Failed => (t!("gui.discover.copy_failed"), true),
            };
            gui.note = Some((words.to_string(), failed));
        }
        MenuItem::Head(_) | MenuItem::Note(..) => {}
    }
}

/// Hand a link to the OS opener on a thread (it can wait two seconds for
/// the opener's word); the note follows on a later frame. Only `http` and
/// `https` links reach here (`PluginLink::is_web`, clause 63).
fn open_link(gui: &mut Gui, label: String, url: String) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let outcome = open_target(&url);
        let _ = tx.send((label, url, outcome));
    });
    gui.discover.opening.push(rx);
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
/// modal's rows while it is up, an open lead menu, else the table.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if let Some(modal) = gui.discover.seeds.as_mut() {
        modal.view.step(delta);
        return;
    }
    match gui.discover.menu {
        Some(Menu::Lead { .. }) => gui.discover.menu_view.step(delta),
        // A dropdown's three or four choices always show whole: nothing to
        // scroll, and the table under it stays put (clause 58's note).
        Some(Menu::Window(_) | Menu::Filter(_)) => {}
        None => {
            gui.act(Act::ScrollBy(List::Discover, delta));
        }
    }
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The room's keys, and its lists' and modal's, which outrank everything
/// while up. `Some(quit)` when the key was consumed.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.screen != Screen::Library || gui.active != super::DISCOVER_NAV {
        return None;
    }
    if gui.discover.seeds.is_some() {
        seeds_key(gui, key);
        return Some(false);
    }
    settle_menu(gui);
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
        KeyCode::Down => walk(gui, &all, 1),
        KeyCode::Up => walk(gui, &all, -1),
        KeyCode::PageDown => walk(gui, &all, 7),
        KeyCode::PageUp => walk(gui, &all, -7),
        KeyCode::Esc if held.is_some() => gui.discover.stow(),
        // The row verbs want the cursor up (clause 57): stowed, they are
        // the room's and do nothing.
        KeyCode::Enter | KeyCode::Char('a' | 'N' | 'P' | 'm') if held.is_none() => {}
        KeyCode::Enter => {
            let index = held?;
            let lead = matches!(all.get(index), Some(Line::Lead(_)));
            gui.act(Act::Disc(DiscAct::Row(index, RowVerb::Open)));
            // Played by key: the cursor stays up (a lead's menu took it).
            if !lead && gui.active == super::DISCOVER_NAV {
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
            if matches!(all.get(index), Some(Line::Song(_) | Line::Peer { .. })) {
                row_act(gui, index, verb);
            }
        }
        KeyCode::Char('m') => {
            let index = held?;
            if matches!(all.get(index), Some(Line::Song(_) | Line::Lead(_))) {
                more(gui, index);
            }
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
        KeyCode::Char('f') if tab == TasteTab::Library => return Some(gui.act(Act::Disc(DiscAct::Filter))),
        // Another pick, only while the card stands (clause 21); otherwise
        // `r` stays the transport's repeat.
        KeyCode::Char('r') if gui.app.taste.no_history => return Some(gui.act(Act::Disc(DiscAct::Another))),
        // The whole-list verbs, where the tab has something to play (clause
        // 64); on Network the keys keep their global meanings.
        KeyCode::Char('p') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::PlayAll))),
        KeyCode::Char('A') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::QueueAll))),
        KeyCode::Char('S') if playable(&gui.app, tab) => return Some(gui.act(Act::Disc(DiscAct::Shuffle))),
        _ => return None,
    }
    Some(false)
}

/// A walking key (the kit's list-cursor law): a stowed cursor is picked up
/// at the top on ↓, the bottom on ↑ — the room has no resting place of its
/// own — and then walks, skipping group lines.
fn walk(gui: &mut Gui, all: &[Line], delta: i32) {
    let rows: Vec<usize> = all.iter().enumerate().filter(|(_, l)| l.is_row()).map(|(i, _)| i).collect();
    if rows.is_empty() {
        return;
    }
    let held = gui.discover.view.held.then_some(gui.discover.cursor).flatten();
    let next = match held.and_then(|c| rows.iter().position(|r| *r == c)) {
        None if delta > 0 => rows[0],
        None => rows[rows.len() - 1],
        Some(at) => {
            let to = (at as i64 + i64::from(delta)).clamp(0, rows.len() as i64 - 1) as usize;
            rows[to]
        }
    };
    gui.discover.cursor = Some(next);
    gui.discover.view.pick_up();
}

fn menu_key(gui: &mut Gui, key: KeyEvent, menu: Menu) {
    let (cursor, len) = match menu {
        Menu::Window(c) => (c, WINDOWS.len()),
        Menu::Filter(c) => (c, FILTERS.len()),
        Menu::Lead { lead, cursor, .. } => {
            (cursor, menu_items(&gui.app, lead).iter().filter(|i| i.actionable()).count())
        }
    };
    let set = |gui: &mut Gui, c: usize| {
        gui.discover.menu = Some(match menu {
            Menu::Window(_) => Menu::Window(c),
            Menu::Filter(_) => Menu::Filter(c),
            Menu::Lead { line, lead, tag, .. } => Menu::Lead { line, lead, cursor: c, tag },
        });
        // A key move brings the lit item into view (a scrolled menu).
        gui.discover.menu_view.reveal = true;
    };
    match key.code {
        KeyCode::Esc => gui.discover.menu = None,
        // The control's own key closes and keeps (the Dropdown law).
        KeyCode::Char('w') if matches!(menu, Menu::Window(_)) => gui.discover.menu = None,
        KeyCode::Char('f') if matches!(menu, Menu::Filter(_)) => gui.discover.menu = None,
        KeyCode::Up => set(gui, cursor.saturating_sub(1)),
        KeyCode::Down if len > 0 => set(gui, (cursor + 1).min(len - 1)),
        KeyCode::Enter => {
            let act = match menu {
                Menu::Window(c) => DiscAct::WindowPick(WINDOWS[c.min(WINDOWS.len() - 1)]),
                Menu::Filter(c) => DiscAct::FilterPick(FILTERS[c.min(FILTERS.len() - 1)]),
                Menu::Lead { cursor, .. } => DiscAct::MenuPick(cursor),
            };
            gui.act(Act::Disc(act));
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
        // and has no queue verbs (clause 61), a peer's row has no sheet
        // (clause 62).
        Some(Line::Lead(_)) => t!("gui.tips.discover_lead").to_string(),
        Some(Line::Album(_)) => t!("gui.tips.discover_album").to_string(),
        Some(Line::Peer { .. }) => t!("gui.tips.discover_peer").to_string(),
        Some(_) => t!("gui.tips.discover_row").to_string(),
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
    use super::{DiscAct, Line, Menu, age_words, columns, layout, lines};
    use crate::api::types::{Capabilities, PluginLink, TasteResponse};
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

    /// The Library + Network half (ask A): three history seeds, one saved
    /// exclusion, an album and four songs, two leads.
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
                      "metadata": { "title": "Aurora", "artist": "Ray", "duration": 251 } },
                    { "filepath": "music/b.mp3", "similarity": 0.80,
                      "metadata": { "title": "Borealis", "artist": "Ray", "duration": 200 }, "lastPlayed": "2024-01-01T00:00:00.000Z" },
                    { "filepath": "music/c.mp3", "similarity": 0.66, "metadata": { "title": "Cirrus", "duration": 180 } },
                    { "filepath": "music/d.mp3", "similarity": 0.61, "metadata": { "title": "Delta", "artist": "Ray" } }
                ]
            },
            "network": {
                "results": [
                    { "artist": "Lead Artist", "title": "Far Away", "similarity": 0.86, "duration": 241, "recordingMbid": "mb-1",
                      "peer": { "endpointId": "e1", "name": "Sam's" }, "source": "p2p", "seedHash": "h2" },
                    { "artist": "Other", "title": "Near", "similarity": 0.7, "peer": { "endpointId": "e2", "name": "Box" }, "source": "p2p" }
                ],
                "searched": { "peers": 3, "tracks": 900 }
            }
        }))
        .unwrap()
    }

    /// The Peers half (ask B): one peer answering with two songs, one not.
    fn peers_answer() -> TasteResponse {
        serde_json::from_value(json!({
            "seedSource": "history",
            "peers": {
                "peers": [
                    { "id": 7, "name": "Sam's server", "reachable": true, "results": [
                        { "artist": "Pia", "title": "Peer One", "filepath": "Music/p1.mp3", "similarity": 0.85, "duration": 200,
                          "peer": { "id": 7, "name": "Sam's server" }, "source": "federation" },
                        { "artist": "Pia", "title": "Peer Two", "filepath": "Music/p2.mp3", "similarity": 0.72,
                          "peer": { "id": 7, "name": "Sam's server" }, "source": "federation" }
                    ] },
                    { "id": 9, "name": "Far box", "reachable": false, "lastSeen": "2026-09-01T00:00:00.000Z", "results": [] }
                ],
                "searched": { "peers": 2, "unreachable": 1, "mismatched": 0 }
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
    fn g_asks_both_halves_and_asks_again_while_the_room_is_up() {
        let _en = english();
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        let asks = |gui: &Gui| {
            gui.pending
                .iter()
                .filter_map(|e| match e {
                    Effect::Api(ApiCmd::Taste { part, body, .. }) => Some((*part, body.sources.clone(), body.remember)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            asks(&gui),
            vec![
                (TastePart::Main, vec!["library".to_string(), "network".to_string()], None),
                (TastePart::Peers, vec!["peers".to_string()], None),
            ],
            "two asks in parallel (clause 30)"
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
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("• asking…"), "the state line says it is asking:\n{all}");
        assert!(all.contains("▱▱▱▱▱▱▱▱▱▱ asking the server…"), "the kit's scan bar:\n{all}");
        assert!(all.contains("MATCH") && all.contains("TRACK"), "the table's header stands:\n{all}");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Right);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("asking your peers… up to 4 s each"), "the Peers pane's own words:\n{all}");
    }

    #[test]
    fn an_answer_fills_the_header_the_tabs_the_seeds_and_the_table() {
        let _en = english();
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(rows[2].contains("Discover · based on 3 songs you finished"), "the header:\n{}", rows[2]);
        assert!(rows[2].trim_end().ends_with("Last 30 days ▾"), "the window dropdown at its right:\n{}", rows[2]);
        assert!(rows[3].contains("• 5 similar"), "the state line:\n{}", rows[3]);
        assert!(rows[5].contains(" Library 5 ") && rows[5].contains("Network 2") && rows[5].contains("Peers 2"), "tabs:\n{}", rows[5]);
        assert!(rows[6].contains("seeds") && rows[6].contains("Manor · ALM") && rows[6].contains("edit"), "seeds:\n{}", rows[6]);
        assert!(rows[8].contains("MATCH") && rows[8].contains("TRACK") && rows[8].contains("LAST PLAYED") && rows[8].contains("LENGTH"));
        // Albums first, with their marker and song count; then the songs, the
        // meter and the percentage leading (clauses 50–51).
        assert!(rows[10].contains("Night Album · Moon album") && rows[10].contains("3 songs"), "album row:\n{}", rows[10]);
        assert!(rows[10].contains("never"), "an album none of whose songs was played:\n{}", rows[10]);
        assert!(rows[11].contains("▇  83%") && rows[11].contains("Aurora · Ray") && rows[11].contains("4:11"), "{}", rows[11]);
        assert!(rows[11].contains("never"), "never played:\n{}", rows[11]);
        assert!(rows[12].contains("years ago"), "an age:\n{}", rows[12]);
        assert!(rows[13].contains("Cirrus") && rows[14].contains("Delta · Ray") && rows[14].contains("—"), "a missing length is the dash");
        // The bottom row: the filter at the left, the verbs at the right.
        assert!(rows[24].contains("Not in 90 days ▾") && rows[24].contains("play · + queue all"), "bottom:\n{}", rows[24]);
        assert!(!all.contains("asking"), "nothing asking once both halves landed:\n{all}");
    }

    #[test]
    fn the_queue_panel_drops_last_played_first() {
        let _en = english();
        let mut gui = answered_gui();
        gui.queue_open = true;
        let rows = draw(&mut gui);
        assert!(rows[8].contains("LENGTH") && !rows[8].contains("LAST PLAYED"), "47 cells: LAST PLAYED goes first:\n{}", rows[8]);
        assert!(rows[2].contains("Last 30 days ▾"), "the window control keeps its home:\n{}", rows[2]);
        assert!(rows[24].contains("Not in 90 days ▾"), "the filter keeps its home:\n{}", rows[24]);
    }

    #[test]
    fn the_peers_tab_groups_its_rows_and_dims_a_peer_that_is_not_answering() {
        let _en = english();
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Peers)));
        let rows = draw(&mut gui);
        assert!(rows[3].contains("• 2 playable · 1 of 2 peers answered · streams through your server"), "{}", rows[3]);
        assert!(rows[5].contains("you don't own these"), "the tab's note:\n{}", rows[5]);
        assert!(rows[10].contains("• Sam's server · online · 2 songs"), "{}", rows[10]);
        assert!(rows[11].contains("Peer One · Pia") && rows[11].contains("Sam's server"), "{}", rows[11]);
        assert!(rows[14].contains("• Far box · not answering · last seen"), "{}", rows[14]);
        let buffer = draw_buffer(&mut gui);
        let x = at(&rows, "Far box").x;
        assert_eq!(buffer[(x, 14)].fg, th().dim, "the unreachable group is dim");
        assert!(!rows[15].contains("Peer"), "and rowless:\n{}", rows[15]);
        // A group line is not a row: ↓ lands on the first song.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.discover.cursor, Some(1));
        // A click plays through the peer's origin (clause 62).
        gui.act(Act::Disc(DiscAct::Row(1, RowVerb::Open)));
        let item = &gui.app.queue.items[0];
        assert_eq!((item.origin.peer, item.filepath.as_str()), (Some(7), "Music/p1.mp3"));
        assert_eq!(gui.app.queue.items.len(), 2, "the tab's playable rows queue after it");
        assert_eq!(gui.discover.cursor, None, "a click stows the cursor");
    }

    #[test]
    fn hover_shows_the_verbs_under_the_pointer_and_never_selects() {
        let _en = english();
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        gui.ui.pointer = Some(Position { x: 30, y: 11 });
        let rows_hover = draw(&mut gui);
        assert!(rows_hover[11].contains("[▸] [»] [+] [⋯]"), "the hover verbs:\n{}", rows_hover[11]);
        assert!(!rows_hover[10].contains("[▸]"), "an album has none:\n{}", rows_hover[10]);
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
    fn the_network_menu_hangs_from_its_row_and_lists_the_links() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.app.taste.tab, TasteTab::Network);
        assert_eq!(gui.config.gui.discover_tab.as_deref(), Some("network"), "the tab is remembered");
        let rows = draw(&mut gui);
        assert!(rows[3].contains("• 2 leads · 3 servers searched"), "{}", rows[3]);
        assert!(rows[8].contains("LEAD") && rows[8].contains("FOUND ON"), "{}", rows[8]);
        assert!(rows[10].contains("Far Away · Lead Artist") && rows[10].contains("Sam's") && rows[10].contains("4:01"), "{}", rows[10]);
        gui.pending.clear();
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Open)));
        assert!(matches!(gui.discover.menu, Some(Menu::Lead { lead: 0, .. })), "a click opens the menu");
        let (generation, key_of) = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::TasteLinks { generation, key, .. }) => Some((*generation, key.clone())),
                _ => None,
            })
            .expect("the links are asked when the menu first opens");
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("FIND ELSEWHERE") && all.contains("asking…"), "asking while out:\n{all}");
        // Hung from its row: the frame's top edge on the row under the lead
        // (row 10), its first item inside it.
        let head = at(&rows, "FIND ELSEWHERE");
        assert_eq!(head.y, 12, "the menu hangs from its row:\n{all}");
        let buffer = draw_buffer(&mut gui);
        assert_eq!(buffer[(head.x - 1, 11)].fg, th().accent, "its frame's top edge sits under the row");
        let link = |label: &str, url: &str| PluginLink { id: label.into(), label: label.into(), url: url.into(), kind: "search".into() };
        gui.app.apply_event(Event::TasteLinks {
            generation,
            key: key_of,
            plugins: Some(vec!["links".into()]),
            answer: Ok(vec![link("Search Bandcamp", "https://bandcamp.com/search?q=x"), link("Run me", "file:///bin/sh")]),
        });
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("Search Bandcamp"), "the plug-in's words:\n{all}");
        assert!(!all.contains("Run me"), "only http and https links are offered");
        assert!(all.contains("THIS LEAD") && all.contains("Copy \"Lead Artist – Far Away\""), "{all}");
        // The keyboard cursor is the accent slab; the copy row is the second.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.discover.menu, None, "a pick closes the menu");
        let note = gui.note.clone().map(|(text, _)| text).unwrap_or_default();
        assert!(note.contains("copied \"Lead Artist – Far Away\""), "confirmed on the note line: {note}");
        // A link goes to the opener (headless under test).
        gui.act(Act::Disc(DiscAct::More(0)));
        gui.act(Act::Disc(DiscAct::MenuPick(0)));
        for _ in 0..200 {
            draw(&mut gui);
            if gui.note.as_ref().is_some_and(|(t, _)| t.contains("bandcamp")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let note = gui.note.clone().map(|(text, _)| text).unwrap_or_default();
        assert_eq!(note, "Search Bandcamp: https://bandcamp.com/search?q=x");
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
        assert!(tabs.contains("Library 1") && tabs.contains("Network –") && tabs.contains("Peers –"), "not asked:\n{tabs}");
        assert!(rows[14].contains("Random One · Dice random"), "the seed wears `random`:\n{}", rows[14]);
        key(&mut gui, KeyCode::Right);
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Asked once you've finished a song"), "{all}");
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
        assert!(all.contains("MATCH") && all.contains("another pick"), "{all}");
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
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("This server hasn't analyzed any music yet") && all.contains("Check again"), "{all}");
        assert!(all.contains("• waiting for the scan"), "{all}");
        // A 403 whose probe lost the gate: switched off, no retry.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        fail(&mut gui, TastePart::Main, TasteFailure::Forbidden);
        assert_eq!(gui.app.taste.latch, TasteLatch::Probing);
        gui.app.apply_event(Event::DiscoveryProbe { available: false, taste: false });
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("• switched off") && all.contains("Sonic discovery has been switched off on this server."), "{all}");
        assert!(!all.contains("Retry") && !all.contains("Check again"), "no retry:\n{all}");
        // A transport failure: the half that failed says so, with Retry.
        let mut gui = session_gui();
        key(&mut gui, KeyCode::Char('G'));
        fail(&mut gui, TastePart::Main, TasteFailure::Failed("boom".into()));
        let rows = draw(&mut gui);
        let all = rows.join("\n");
        assert!(all.contains("couldn't ask for suggestions: boom") && all.contains("Retry"), "{all}");
        assert!(rows[3].contains("• no answer"), "{}", rows[3]);
        gui.pending.clear();
        let retry = at(&rows, "Retry");
        let act = gui.ui.hit(retry).expect("Retry is a click target");
        gui.act(act);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Taste { .. }))), "Retry asks again");
    }

    #[test]
    fn the_seeds_modal_sends_the_exclusions_once_when_it_closes() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Char('e'));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Based on") && all.contains("FROM YOUR PLAYS") && all.contains("DROPPED"), "{all}");
        assert!(all.contains("[✓] Manor · ALM") && all.contains("4 plays · finished"), "{all}");
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
    fn the_window_and_the_filter_are_dropdowns_that_ask() {
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
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('f'));
        assert!(matches!(gui.discover.menu, Some(Menu::Filter(1))), "the current filter under the cursor");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Never played") && all.contains("Everything"), "{all}");
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Enter);
        let parts: Vec<_> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Taste { part, .. }) => Some(*part),
                _ => None,
            })
            .collect();
        assert_eq!(parts, vec![TastePart::Library], "a filter change asks the library alone");
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
        let mut config = Config::default();
        config.gui.discover_tab = Some("peers".into());
        let gui = Gui::new(config, false, App::new(None, None, None));
        assert_eq!(gui.app.taste.tab, TasteTab::Peers);
        let mut gui = answered_gui();
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Peers)));
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
        gui.act(Act::Disc(DiscAct::More(0)));
        println!("{}", draw(&mut gui).join("\n"));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Down);
        println!("{}", draw(&mut gui).join("\n"));
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
    fn narrow_tables_drop_the_third_column_first() {
        let wide = columns(Rect { x: 17, y: 0, width: 78, height: 1 }, 5, 12, 8);
        assert_eq!(wide.track_x, 17 + super::MATCH_W + super::GAP, "the meter's width is the floor");
        assert!(wide.third.is_some() && wide.track_w >= 24, "{wide:?}");
        let narrow = columns(Rect { x: 17, y: 0, width: 45, height: 1 }, 5, 12, 8);
        assert_eq!(narrow.third, None, "{narrow:?}");
        assert_eq!(narrow.len_x + narrow.len_w, 17 + 45, "LENGTH never goes");
        // A heading wider than the meter widens MATCH, gap kept.
        let french = columns(Rect { x: 17, y: 0, width: 78, height: 1 }, 9, 12, 8);
        assert_eq!(french.track_x, 17 + 9 + super::GAP);
    }

    #[test]
    fn the_lines_put_albums_first_and_skip_unreachable_peers_rows() {
        let mut app = App::new(None, None, None);
        app.taste.library.data = main_answer().library;
        app.taste.peers.data = peers_answer().peers;
        let library = lines(&app, TasteTab::Library);
        assert_eq!(library[..2], [Line::Album(0), Line::Song(0)]);
        let peers = lines(&app, TasteTab::Peers);
        assert_eq!(
            peers,
            vec![Line::Group(0), Line::Peer { group: 0, row: 0 }, Line::Peer { group: 0, row: 1 }, Line::Gap, Line::Group(1)]
        );
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
    fn the_match_heading_keeps_its_gap_in_french() {
        let _fr = locale("fr");
        let mut gui = answered_gui();
        let rows = draw(&mut gui);
        let header = &rows[8];
        let at_match = at(&rows, "PROXIMITÉ").x;
        let at_title = at(&rows, "TITRE").x;
        assert_eq!(at_title, at_match + 9 + 2, "the 9-cell heading widens MATCH, the 2-cell gap kept:\n{header}");
        // The rows follow the heading, not the meter's width.
        assert_eq!(at(&rows, "Aurora · Ray").x, at_title, "the title column under its heading:\n{}", rows.join("\n"));
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
        assert!(rows[11].contains("83%") && !rows[11].contains('▇'), "the percentage alone:\n{}", rows[11]);
        assert!(rows[14].contains(" - ") || rows[14].trim_end().ends_with('-'), "the missing length:\n{}", rows[14]);
        for glyph in ['▾', '▸', '»', '⋯', '▇', '—', '–'] {
            assert!(!all.contains(glyph), "{glyph:?} on the legacy console:\n{all}");
        }
    }

    #[test]
    fn an_open_lead_menu_follows_its_lead_when_a_new_answer_lands() {
        let _en = english();
        let mut gui = answered_gui();
        key(&mut gui, KeyCode::Right);
        gui.act(Act::Disc(DiscAct::Row(0, RowVerb::Open)));
        assert!(matches!(gui.discover.menu, Some(Menu::Lead { lead: 0, .. })));
        // G asks again; the rows stand, and the answer lands reordered.
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = main_answer();
        answer.network.as_mut().unwrap().results.reverse();
        land(&mut gui, TastePart::Main, answer);
        let rows = draw(&mut gui);
        assert!(matches!(gui.discover.menu, Some(Menu::Lead { lead: 1, line: 1, .. })), "{:?}", gui.discover.menu);
        assert!(rows.join("\n").contains("Copy \"Lead Artist – Far Away\""), "still the clicked lead's text");
        // An answer without it closes the menu.
        key(&mut gui, KeyCode::Char('G'));
        let mut answer = main_answer();
        answer.network.as_mut().unwrap().results.remove(0);
        land(&mut gui, TastePart::Main, answer);
        draw(&mut gui);
        assert_eq!(gui.discover.menu, None, "its lead is gone, so is the menu");
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
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), "Enter open the album · Esc done", "an album row");
        key(&mut gui, KeyCode::Down);
        assert!(super::tips(&gui).contains("m actions"), "a song row");
        gui.act(Act::Disc(DiscAct::Tab(TasteTab::Peers)));
        key(&mut gui, KeyCode::Down);
        let tips = super::tips(&gui);
        assert!(tips.contains("P play now") && !tips.contains("m actions"), "a peer row has no sheet: {tips}");
    }
}
