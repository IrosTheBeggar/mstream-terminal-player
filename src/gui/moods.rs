//! The mood Filters (mStream #1092 and #1099; library-rooms contract,
//! clauses 30–36; discovery-page contract, clause 33): `mood`, a DIM text
//! button on the line one of the Albums wall, the Artists, Recent and Rated
//! lists and the Discover header, and the modal it opens over the room —
//! the three scales and the tags, the level (how sure every group must be)
//! and, on the song lists, their Sort. The modal edits a draft, and nothing
//! changes until it closes: then the room asks once. While a mood stands,
//! the strip under the controls names it, and its `[X]` lets it go.

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::api::types::{MOOD_TAGS, MoodFilter, MoodLevel, MoodScale, MoodSort};
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{dim, width};
use crate::tui::worker::LibraryNode;

use super::{
    ALBUMS_NAV, ARTISTS_NAV, Act, DISCOVER_NAV, Gui, RATED_NAV, RECENT_NAV, Screen, accent, bright_bold, put, sel,
};

/// One room's choice: its moods and level — as chosen, so a level picked
/// with no mood set is kept for when one comes — and a song list's Sort.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RoomMoods {
    pub filter: MoodFilter,
    pub sort: MoodSort,
}

impl RoomMoods {
    /// The order the list shows: its own while no mood is set — Match has
    /// nothing to rank by then, and comes back with the first mood.
    pub(crate) fn shown_sort(&self) -> MoodSort {
        if self.filter.is_empty() { MoodSort::Own } else { self.sort }
    }
}

/// The Albums, Artists, Recent and Rated rooms' choices — the Discover
/// room's moods are the App's (`taste.moods`), which its asks read, and it
/// has no Sort — kept while the app runs and never saved, and the open
/// modal's draft.
#[derive(Debug, Clone, Default)]
pub(crate) struct MoodRooms {
    pub albums: RoomMoods,
    pub artists: RoomMoods,
    pub recent: RoomMoods,
    pub rated: RoomMoods,
    /// The open modal: closed by default, and dropped by any move to a
    /// room, that one's own row included (clause 32).
    pub draft: Option<MoodDraft>,
    /// While a room's ask under a new choice is out: its nav row and the
    /// choice the rows on screen carry, taken back if the App puts the ask
    /// back ([`follow_ask`]).
    pub undo: Option<(usize, RoomMoods)>,
}

/// The open modal (clause 32): whose it is, the choice it edits — the
/// room's own changes only when it closes — and the keyboard's hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MoodDraft {
    /// The nav row whose choice this is.
    pub nav: usize,
    pub moods: RoomMoods,
    /// The stop the keyboard's hand is on.
    pub hand: MoodStop,
    /// Whether the hand is drawn: the modal opened by its key, or a walking
    /// key pressed since a click stowed it (the kit's list-cursor law).
    pub shown: bool,
    /// The column ↑ and ↓ keep.
    pub col: u8,
}

impl MoodDraft {
    fn opened(nav: usize, moods: RoomMoods, key: bool) -> MoodDraft {
        MoodDraft { nav, moods, hand: FIRST, shown: key, col: 0 }
    }

    /// Whether the room is a song list, whose modal has the SORT row.
    fn sorts(&self) -> bool {
        matches!(self.nav, RECENT_NAV | RATED_NAV)
    }
}

/// One keyboard stop of the modal — and what a click on it presses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoodStop {
    /// A scale's value: chosen, or let go when it was the one chosen.
    Pick(MoodScale, &'static str),
    /// A tag on, or off.
    Tag(&'static str),
    Level(MoodLevel),
    /// A song list's order (Recent and Rated).
    Sort(MoodSort),
    /// `clear`: every scale and tag let go, the level back to Standard.
    Clear,
    /// `[ Done ]`: the draft applied, the modal closed.
    Done,
}

/// The first stop: where the hand opens.
const FIRST: MoodStop = MoodStop::Pick(MoodScale::Valence, "happy");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoodAct {
    /// The opener or the strip's words (`key: false`, the hand stowed), or
    /// `o` (`key: true`, the hand up on the first stop).
    Open { key: bool },
    /// A stop, `clear` or `[ Done ]`, clicked.
    Press(MoodStop),
    /// The strip's `[X]`: every mood let go, the Sort kept.
    Drop,
}

/// What a room's new choice did ([`apply`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// What is asked for changed: the room asked once.
    Asked,
    /// Only a song list's shown order changed: re-ordered in hand.
    Reordered,
    /// Nothing on screen changed — a level with no mood, the same choice,
    /// or a room that does not offer the filters now — and nothing asked.
    Nothing,
}

/// Whether the room at nav row `nav` offers the filters, whatever its pane
/// shows (library-rooms clause 30; discovery-page clause 33): the user's
/// own server — never a peer's lists — with scores to filter by, and for
/// Discover a taste route that takes the key.
pub(crate) fn room_offers(gui: &Gui, nav: usize) -> bool {
    let caps = &gui.app.capabilities;
    let own = gui.app.session.peer.is_none();
    match nav {
        ALBUMS_NAV | ARTISTS_NAV | RECENT_NAV | RATED_NAV => own && caps.mood_filters,
        DISCOVER_NAV => own && caps.taste_moods(),
        _ => false,
    }
}

/// The room's choice, as kept.
pub(crate) fn room(gui: &Gui, nav: usize) -> RoomMoods {
    match nav {
        ALBUMS_NAV => gui.moods.albums,
        ARTISTS_NAV => gui.moods.artists,
        RECENT_NAV => gui.moods.recent,
        RATED_NAV => gui.moods.rated,
        DISCOVER_NAV => RoomMoods { filter: gui.app.taste.moods, sort: MoodSort::Own },
        _ => RoomMoods::default(),
    }
}

/// What the room asks under: its filter's wire form where it offers the
/// filters, and none where it does not — the choice stays kept for a
/// session that offers them again.
pub(crate) fn asked(gui: &Gui, nav: usize) -> MoodFilter {
    if room_offers(gui, nav) { room(gui, nav).filter.wire() } else { MoodFilter::NONE }
}

/// Whether the room's song list stands by match: Recent or Rated, where
/// the filters are offered and a mood is set under Match.
pub(crate) fn by_match(gui: &Gui, nav: usize) -> bool {
    matches!(nav, RECENT_NAV | RATED_NAV)
        && room_offers(gui, nav)
        && room(gui, nav).shown_sort() == MoodSort::Match
}

/// The room's choice becomes `next`, and the room follows it once: one ask
/// where what is asked changed — the wall from its first page, a list from
/// its top, Discover a new batch with the visit kept — a re-order in hand
/// where only a song list's shown order did, and nothing otherwise. The
/// rows on screen stand until the answer lands; an ask carries the new
/// Sort, which the answer lands in.
pub(crate) fn apply(gui: &mut Gui, nav: usize, next: RoomMoods) -> Applied {
    let was = room(gui, nav);
    match nav {
        ALBUMS_NAV => gui.moods.albums = next,
        ARTISTS_NAV => gui.moods.artists = next,
        RECENT_NAV => gui.moods.recent = next,
        RATED_NAV => gui.moods.rated = next,
        DISCOVER_NAV => gui.app.taste.moods = next.filter,
        _ => return Applied::Nothing,
    }
    if !room_offers(gui, nav) {
        return Applied::Nothing;
    }
    let wire = next.filter.wire();
    match nav {
        DISCOVER_NAV => {
            if was.filter.wire() == wire {
                return Applied::Nothing;
            }
            let effects = gui.app.taste_ask_again();
            gui.pend(effects);
            Applied::Asked
        }
        ALBUMS_NAV => {
            let effects = switch(gui, nav, was, wire);
            if effects.is_empty() {
                return Applied::Nothing;
            }
            gui.albums.wall.page = 0;
            gui.albums.wall.stow();
            gui.pend(effects);
            Applied::Asked
        }
        _ => {
            let songs = matches!(nav, RECENT_NAV | RATED_NAV);
            let by_match = songs && next.shown_sort() == MoodSort::Match;
            if songs {
                gui.app.songs_by_match = by_match;
            }
            let effects = switch(gui, nav, was, wire);
            let outcome = if !effects.is_empty() {
                Applied::Asked
            } else if songs && was.shown_sort() != next.shown_sort() {
                gui.app.sort_songs(by_match);
                Applied::Reordered
            } else {
                return Applied::Nothing;
            };
            gui.library.view.scroll = 0;
            gui.library.view.reveal = false;
            gui.library.view.stow();
            gui.pend(effects);
            outcome
        }
    }
}

/// The room's list asked again under `wire`, keeping `was` — the choice
/// the rows on screen carry — for [`follow_ask`]; an earlier ask of the
/// room's still out keeps its own.
fn switch(gui: &mut Gui, nav: usize, was: RoomMoods, wire: MoodFilter) -> Vec<crate::tui::app::Effect> {
    let held = gui.moods.undo.filter(|&(room, _)| room == nav && gui.app.mood_ask_out());
    let effects = gui.app.switch_moods(wire);
    if !effects.is_empty() {
        gui.moods.undo = Some(held.unwrap_or((nav, was)));
    }
    effects
}

/// The room's choice once the App's ask under it is over — run with every
/// batch the App hands the shell ([`Gui::pend`]). An answer that landed,
/// or a room left, keeps it. An ask the App put back — it failed, or a
/// drill left from the rows still standing — takes back the choice those
/// rows carry: the strip never names a filter they lack, and Done with the
/// new choice asks again.
pub(crate) fn follow_ask(gui: &mut Gui) {
    let put_back = std::mem::take(&mut gui.app.moods_put_back);
    if gui.app.mood_ask_out() {
        return;
    }
    let Some((nav, was)) = gui.moods.undo.take() else { return };
    if !put_back {
        return;
    }
    match nav {
        ALBUMS_NAV => gui.moods.albums = was,
        ARTISTS_NAV => gui.moods.artists = was,
        RECENT_NAV => gui.moods.recent = was,
        RATED_NAV => gui.moods.rated = was,
        _ => return,
    }
    if matches!(nav, RECENT_NAV | RATED_NAV) {
        gui.app.songs_by_match = by_match(gui, nav);
    }
}

// ── Where they are offered ──────────────────────────────────────────────────

/// Whether the room on screen offers the opener, the strip and `o` (clause
/// 30): where [`room_offers`] holds, at the room's root — the Albums wall,
/// the Artists, Recent and Rated lists, the Discover room — never an
/// artist's wall or an album.
pub(crate) fn offered_here(gui: &Gui) -> bool {
    let here = gui.app.library_stack.here();
    gui.screen == Screen::Library
        && gui.app.connected
        && room_offers(gui, gui.active)
        && match gui.active {
            ALBUMS_NAV => matches!(here, LibraryNode::Albums(_)),
            ARTISTS_NAV => matches!(here, LibraryNode::Artists(_)),
            RECENT_NAV => matches!(here, LibraryNode::Recent(_)),
            RATED_NAV => matches!(here, LibraryNode::Rated(_)),
            DISCOVER_NAV => true,
            _ => false,
        }
}

/// Whether the modal is up: the room on screen's own, where it still
/// offers the filters.
pub(crate) fn is_open(gui: &Gui) -> bool {
    gui.moods.draft.is_some_and(|draft| draft.nav == gui.active) && offered_here(gui)
}

/// Whether the strip stands (clause 35): where the room offers the filters
/// and its committed choice holds a mood or a tag.
pub(crate) fn strip_stands(gui: &Gui) -> bool {
    offered_here(gui) && !room(gui, gui.active).filter.is_empty()
}

// ── Words ───────────────────────────────────────────────────────────────────

/// A choice's word in `locale` — passed, never switched, so a test can
/// lay the modal out in every language while others read English.
fn label(word: &str, locale: &str) -> String {
    match word {
        "happy" => t!("gui.moods.happy", locale = locale),
        "neutral" => t!("gui.moods.neutral", locale = locale),
        "sad" => t!("gui.moods.sad", locale = locale),
        "relaxed" => t!("gui.moods.relaxed", locale = locale),
        "aggressive" => t!("gui.moods.aggressive", locale = locale),
        "instrumental" => t!("gui.moods.instrumental", locale = locale),
        "vocals" => t!("gui.moods.vocals", locale = locale),
        "acoustic" => t!("gui.moods.acoustic", locale = locale),
        "party" => t!("gui.moods.party", locale = locale),
        "danceable" => t!("gui.moods.danceable", locale = locale),
        _ => return word.to_string(),
    }
    .to_string()
}

fn level_label(level: MoodLevel, locale: &str) -> String {
    match level {
        MoodLevel::Strict => t!("gui.moods.strict", locale = locale),
        MoodLevel::Standard => t!("gui.moods.standard", locale = locale),
        MoodLevel::Loose => t!("gui.moods.loose", locale = locale),
    }
    .to_string()
}

/// A Sort radio's word: the list's own order — Rating on Rated, Newest on
/// Recent — or Match.
fn sort_label(nav: usize, sort: MoodSort, locale: &str) -> String {
    match sort {
        MoodSort::Own if nav == RATED_NAV => t!("gui.moods.sort_rating", locale = locale),
        MoodSort::Own => t!("gui.moods.sort_newest", locale = locale),
        MoodSort::Match => t!("gui.moods.sort_match", locale = locale),
    }
    .to_string()
}

/// A stop's word.
fn stop_label(nav: usize, stop: MoodStop, locale: &str) -> String {
    match stop {
        MoodStop::Pick(_, word) | MoodStop::Tag(word) => label(word, locale),
        MoodStop::Level(level) => level_label(level, locale),
        MoodStop::Sort(sort) => sort_label(nav, sort, locale),
        MoodStop::Clear => t!("gui.moods.clear", locale = locale).to_string(),
        MoodStop::Done => format!("[ {} ]", t!("gui.moods.done", locale = locale)),
    }
}

// ── The modal's geometry ────────────────────────────────────────────────────

/// The modal's cells across, in every locale and every room.
const MODAL_W: u16 = 66;
/// The cells its rows take: inside the border, a pad cell each side.
const MODAL_TEXT: u16 = MODAL_W - 4;
/// Cells between two columns at least.
const COL_GAP: u16 = 3;

/// The modal's rows, its border in: 10, or 11 with the SORT row.
pub(crate) fn modal_rows(sorts: bool) -> u16 {
    10 + u16::from(sorts)
}

/// The three columns' x, from the rows' first cell, in `locale` (clause 32):
/// each column as wide as a glyph, a space and its widest word — the Sort
/// words included, so the box is the same in every room — the first at
/// least as wide as SORT, and the slack shared between the two gaps.
pub(crate) fn modal_cols(locale: &str) -> [u16; 3] {
    let widths = col_widths(locale);
    let need = widths.iter().sum::<u16>() + 2 * COL_GAP;
    let slack = MODAL_TEXT.saturating_sub(need);
    let gap1 = COL_GAP + slack / 2;
    let gap2 = COL_GAP + slack - slack / 2;
    [0, widths[0] + gap1, widths[0] + gap1 + widths[1] + gap2]
}

/// The columns' widths in `locale`: a glyph, a space and the column's
/// widest word; the first at least SORT's.
fn col_widths(locale: &str) -> [u16; 3] {
    let mut widths = [0u16; 3];
    let mut words: Vec<(usize, String)> = Vec::new();
    for scale in MoodScale::ALL {
        words.extend(scale.choices().iter().enumerate().map(|(c, v)| (c, label(v, locale))));
    }
    words.extend(MOOD_TAGS.iter().enumerate().map(|(c, tag)| (c, label(tag, locale))));
    words.extend(MoodLevel::ALL.iter().enumerate().map(|(c, level)| (c, level_label(*level, locale))));
    words.push((1, sort_label(RATED_NAV, MoodSort::Own, locale)));
    words.push((1, sort_label(RECENT_NAV, MoodSort::Own, locale)));
    words.push((2, sort_label(RATED_NAV, MoodSort::Match, locale)));
    for (c, word) in &words {
        widths[*c] = widths[*c].max(4 + width(word) as u16);
    }
    widths[0] = widths[0].max(width(&t!("gui.moods.sort", locale = locale)) as u16);
    widths
}

/// Where the modal stands, `h` rows tall, and its band (clause 32): where
/// the kit's modal stands over the whole frame — or, with the queue folded,
/// centred in the room, the room's width cleared on its rows — its bottom
/// never under the room's last row.
pub(crate) fn place(area: Rect, room: Rect, h: u16) -> (Rect, Option<Rect>) {
    let mut rect = crate::kit::modal_rect(area, MODAL_W, h, h);
    let banded = room.width >= MODAL_W + 4;
    if banded {
        rect.x = room.x + (room.width - rect.width) / 2;
    }
    rect.y = rect.y.min(room.bottom().saturating_sub(rect.height));
    let band = banded.then_some(Rect { x: room.x, y: rect.y, width: room.width, height: rect.height });
    (rect, band)
}

/// One row of choice stops, by column; a column with none is `None`.
type StopRow = [Option<MoodStop>; 3];

/// Whether Match is awake: a mood to rank by is set.
fn match_awake(filter: MoodFilter) -> bool {
    !filter.is_empty()
}

/// Whether `clear` is drawn: something to clear — a mood, a tag, or a level
/// other than Standard.
fn clear_shown(filter: MoodFilter) -> bool {
    !filter.is_empty() || filter.level() != MoodLevel::Standard
}

/// The modal's stops: its choice rows by column — the scales, the tags,
/// the level and, on a song list, the Sort, whose first column is the SORT
/// word and whose Match is a stop only while awake — and the foot's,
/// `clear` while drawn and `[ Done ]`.
pub(crate) fn stop_rows(draft: &MoodDraft) -> (Vec<StopRow>, Vec<MoodStop>) {
    let mut rows: Vec<StopRow> = MoodScale::ALL
        .iter()
        .map(|&scale| {
            let mut row = [None; 3];
            for (c, &value) in scale.choices().iter().enumerate() {
                row[c] = Some(MoodStop::Pick(scale, value));
            }
            row
        })
        .collect();
    rows.push(MOOD_TAGS.map(|tag| Some(MoodStop::Tag(tag))));
    rows.push(MoodLevel::ALL.map(|level| Some(MoodStop::Level(level))));
    if draft.sorts() {
        let awake = match_awake(draft.moods.filter).then_some(MoodStop::Sort(MoodSort::Match));
        rows.push([None, Some(MoodStop::Sort(MoodSort::Own)), awake]);
    }
    let mut foot = Vec::new();
    if clear_shown(draft.moods.filter) {
        foot.push(MoodStop::Clear);
    }
    foot.push(MoodStop::Done);
    (rows, foot)
}

/// Every stop in reading order: the rows left to right, top to bottom,
/// then the foot.
pub(crate) fn reading_order(draft: &MoodDraft) -> Vec<MoodStop> {
    let (rows, foot) = stop_rows(draft);
    rows.iter().flat_map(|row| row.iter().flatten().copied()).chain(foot).collect()
}

/// Where `stop` stands: its row (the foot is the row after the last) and
/// its column (in the foot, its place there).
fn locate(rows: &[StopRow], foot: &[MoodStop], stop: MoodStop) -> Option<(usize, usize)> {
    rows.iter()
        .enumerate()
        .find_map(|(r, row)| row.iter().position(|s| *s == Some(stop)).map(|c| (r, c)))
        .or_else(|| foot.iter().position(|s| *s == stop).map(|c| (rows.len(), c)))
}

/// The stop ↑ or ↓ lands on in `row` keeping column `col`: that column's,
/// else the nearest to its right, else the row's last.
fn land(row: &StopRow, col: u8) -> Option<MoodStop> {
    let col = usize::from(col).min(row.len() - 1);
    row[col..].iter().flatten().next().or_else(|| row.iter().flatten().last()).copied()
}

/// Where a walking key takes the hand (clause 32), and the column ↑ ↓ keep
/// from there: ↑ ↓ the row above or below in the kept column — ↓ from the
/// last choice row to `[ Done ]`, ↑ from the foot back to that row — no
/// wrap; ← → the row's stop beside, clamped, the foot's between `clear`
/// and `[ Done ]`; Tab and Shift-Tab the next or previous stop in reading
/// order, wrapping.
pub(crate) fn walk(draft: &MoodDraft, key: KeyCode) -> (MoodStop, u8) {
    let (rows, foot) = stop_rows(draft);
    let stay = (draft.hand, draft.col);
    let Some((r, c)) = locate(&rows, &foot, draft.hand) else { return (FIRST, 0) };
    let in_foot = r == rows.len();
    let kept = |stop: Option<MoodStop>| stop.map_or(stay, |stop| (stop, draft.col));
    let col_of = |stop: MoodStop| locate(&rows, &foot, stop).filter(|(r, _)| *r < rows.len()).map(|(_, c)| c as u8);
    match key {
        KeyCode::Up if in_foot => kept(rows.last().and_then(|row| land(row, draft.col))),
        KeyCode::Up if r > 0 => kept(land(&rows[r - 1], draft.col)),
        KeyCode::Down if in_foot => stay,
        KeyCode::Down if r + 1 == rows.len() => (MoodStop::Done, draft.col),
        KeyCode::Down => kept(land(&rows[r + 1], draft.col)),
        KeyCode::Left | KeyCode::Right if in_foot => {
            let at = if key == KeyCode::Left { c.saturating_sub(1) } else { (c + 1).min(foot.len() - 1) };
            (foot[at], draft.col)
        }
        KeyCode::Left => rows[r][..c].iter().flatten().last().map_or(stay, |s| (*s, col_of(*s).unwrap_or(0))),
        KeyCode::Right => rows[r][c + 1..].iter().flatten().next().map_or(stay, |s| (*s, col_of(*s).unwrap_or(2))),
        KeyCode::Tab | KeyCode::BackTab => {
            let order = reading_order(draft);
            let at = order.iter().position(|s| *s == draft.hand).unwrap_or(0);
            let n = order.len();
            let next = if key == KeyCode::Tab { order[(at + 1) % n] } else { order[(at + n - 1) % n] };
            (next, col_of(next).unwrap_or(draft.col))
        }
        _ => stay,
    }
}

/// `stop` pressed on the draft: a scale's value chosen or let go, a tag
/// flipped, a level or a Sort picked — the own order pressed while Match
/// rests does nothing, it is already shown pressed — or `clear`: the moods
/// let go, the level Standard, the Sort kept.
fn press(draft: &mut MoodDraft, stop: MoodStop) {
    let filter = draft.moods.filter;
    match stop {
        MoodStop::Pick(scale, value) => draft.moods.filter = filter.press(scale, value),
        MoodStop::Tag(tag) => draft.moods.filter = filter.toggle_tag(tag),
        MoodStop::Level(level) => draft.moods.filter = filter.with_level(level),
        MoodStop::Sort(sort) if match_awake(filter) => draft.moods.sort = sort,
        MoodStop::Sort(_) => {}
        MoodStop::Clear => draft.moods.filter = filter.cleared(),
        MoodStop::Done => {}
    }
}

/// The hand off a stop that stopped being one: a Match gone to rest to the
/// own order, a `clear` gone to `[ Done ]`.
fn settle(draft: &mut MoodDraft) {
    let order = reading_order(draft);
    if order.contains(&draft.hand) {
        return;
    }
    draft.hand = match draft.hand {
        MoodStop::Sort(_) => MoodStop::Sort(MoodSort::Own),
        MoodStop::Clear => MoodStop::Done,
        _ => FIRST,
    };
}

// ── Acting ──────────────────────────────────────────────────────────────────

/// The draft becomes the room's choice, and the modal closes — Done, Esc,
/// Enter and `o` alike (clause 34).
fn done(gui: &mut Gui) {
    let Some(draft) = gui.moods.draft.take() else { return };
    let _ = apply(gui, draft.nav, draft.moods);
}

/// The modal's keys while it is up — every key but Ctrl-C, which quits
/// first: the room beneath is inert (clause 32). `None` while it is not up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !is_open(gui) {
        return None;
    }
    let mut draft = gui.moods.draft?;
    let code = match key.code {
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
        code => code,
    };
    match code {
        // A stowed hand is only shown by the first walking key.
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
            if draft.shown {
                (draft.hand, draft.col) = walk(&draft, code);
            }
            draft.shown = true;
        }
        KeyCode::Char(' ') if draft.shown && draft.hand == MoodStop::Done => {
            done(gui);
            return Some(false);
        }
        KeyCode::Char(' ') => {
            if draft.shown {
                let held = draft.hand;
                press(&mut draft, held);
            }
            draft.shown = true;
        }
        // `clear` while the hand shows on it; Done from anywhere else.
        KeyCode::Enter if draft.shown && draft.hand == MoodStop::Clear => press(&mut draft, MoodStop::Clear),
        KeyCode::Enter | KeyCode::Esc | KeyCode::Char('o') => {
            done(gui);
            return Some(false);
        }
        _ => {}
    }
    settle(&mut draft);
    gui.moods.draft = Some(draft);
    Some(false)
}

/// The pointer's acts (clauses 31–35). Nothing outside a room that offers
/// the filters.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Mood(mood) = act else { return false };
    match *mood {
        MoodAct::Open { key } => {
            if offered_here(gui) && !is_open(gui) {
                let nav = gui.active;
                gui.moods.draft = Some(MoodDraft::opened(nav, room(gui, nav), key));
            }
        }
        MoodAct::Press(MoodStop::Done) => {
            if is_open(gui) {
                done(gui);
            }
        }
        MoodAct::Press(stop) => {
            if !is_open(gui) {
                return true;
            }
            let Some(mut draft) = gui.moods.draft else { return true };
            let (rows, foot) = stop_rows(&draft);
            let Some((r, c)) = locate(&rows, &foot, stop) else { return true };
            press(&mut draft, stop);
            // Pressed, and the hand stowed there: the next walking key
            // shows it on this stop.
            draft.hand = stop;
            draft.shown = false;
            if r < rows.len() {
                draft.col = c as u8;
            }
            settle(&mut draft);
            gui.moods.draft = Some(draft);
        }
        MoodAct::Drop => {
            if offered_here(gui) && !is_open(gui) {
                let nav = gui.active;
                let next = RoomMoods { filter: MoodFilter::NONE, ..room(gui, nav) };
                let _ = apply(gui, nav, next);
            }
        }
    }
    true
}

// ── The opener ──────────────────────────────────────────────────────────────

/// Where line one's opener stands (clause 31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenerAt {
    /// Two cells after a room's name this many cells wide: the lists.
    AfterName(u16),
    /// Ending 13 cells in from the content's right — clear of a two-digit
    /// pager — or three left of a wider pager's back arrow at this x: the
    /// wall.
    BeforePager(u16),
}

/// The opener's x, `w` cells wide: a fixed column the count never moves.
pub(crate) fn opener_x(content: Rect, at: OpenerAt, w: u16) -> u16 {
    match at {
        OpenerAt::AfterName(name) => content.x + name + 2,
        OpenerAt::BeforePager(back) => {
            let end = content.right().saturating_sub(13).min(back.saturating_sub(3));
            (end + 1).saturating_sub(w)
        }
    }
}

/// The cells the wall's count may take from `count_x`, the opener at
/// `opener_x`: up to two cells before it (clause 31). Only a pager of three
/// digits or more, or a five-digit count in a long locale, leaves it fewer
/// than it needs: its words are clipped by cells, its digits kept.
pub(crate) fn count_cells(count_x: u16, opener_x: u16) -> usize {
    usize::from(opener_x.saturating_sub(2).saturating_sub(count_x))
}

/// The opener's word, `mood`, and its cells.
pub(crate) fn opener_word(locale: &str) -> String {
    t!("gui.moods.mood", locale = locale).to_string()
}

/// `mood` at (`x`, `y`): a DIM text button, BRIGHT BOLD under the pointer;
/// a click opens the modal with the hand stowed. No tooltip (clause 31).
pub(crate) fn draw_opener(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16) {
    let word = opener_word(&rust_i18n::locale());
    super::text_button(frame, gui, x, y, &word, false, Act::Mood(MoodAct::Open { key: false }));
}

// ── The strip ───────────────────────────────────────────────────────────────

/// A part of the strip: a mood or tag's word, the level's, or ` · `.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StripPart {
    Mood,
    Level,
    Sep,
}

/// The strip's words for `filter` (clause 35): the scales' values, the
/// tags in their order, then the level's word when it is not Standard,
/// ` · ` between.
pub(crate) fn strip_run(filter: MoodFilter, locale: &str) -> Vec<(String, StripPart)> {
    let mut words: Vec<(String, StripPart)> = MoodScale::ALL
        .iter()
        .filter_map(|&scale| filter.value_of(scale))
        .chain(filter.tags())
        .map(|word| (label(word, locale), StripPart::Mood))
        .collect();
    if filter.level() != MoodLevel::Standard {
        words.push((level_label(filter.level(), locale), StripPart::Level));
    }
    let mut run = Vec::new();
    for (i, word) in words.into_iter().enumerate() {
        if i > 0 {
            run.push((" · ".to_string(), StripPart::Sep));
        }
        run.push(word);
    }
    run
}

/// The run in `cells`: whole, or clipped by cells with the kit's mark
/// ([`super::bar::clip`]) — and whether it was clipped.
pub(crate) fn strip_fit(run: &[(String, StripPart)], cells: usize) -> (Vec<(String, StripPart)>, bool) {
    if run.iter().map(|(text, _)| width(text)).sum::<usize>() <= cells {
        return (run.to_vec(), false);
    }
    let whole: String = run.iter().map(|(text, _)| text.as_str()).collect();
    let clipped = super::bar::clip(&whole, cells);
    let mark = clipped.chars().last().map_or(0, char::len_utf8);
    let mut kept = clipped.len() - mark;
    let mut out = Vec::new();
    let mut cut_at = StripPart::Sep;
    for (text, part) in run {
        if kept == 0 {
            cut_at = *part;
            break;
        }
        let take = kept.min(text.len());
        out.push((text[..take].to_string(), *part));
        kept -= take;
        if take < text.len() {
            cut_at = *part;
            break;
        }
    }
    out.push((clipped[clipped.len() - mark..].to_string(), cut_at));
    (out, true)
}

/// The strip on row `y` of `content` (clause 35), where it stands: the run
/// ending a cell in from the content's last — clipped before `[X]`, which
/// stays — moods and tags OK, the level TEXT, the separators DIM. The words
/// are one click, the modal opened with the hand stowed; `[X]` lets every
/// mood go.
pub(crate) fn draw_strip(frame: &mut Frame, gui: &mut Gui, content: Rect, y: u16) {
    if !strip_stands(gui) {
        return;
    }
    let run = strip_run(room(gui, gui.active).filter, &rust_i18n::locale());
    let (pieces, clipped) = strip_fit(&run, usize::from(content.width.saturating_sub(5)));
    let words_w: u16 = pieces.iter().map(|(text, _)| width(text) as u16).sum();
    let words = Rect { x: content.x, y, width: words_w, height: 1 };
    let lit = gui.ui.hovers(words);
    let mut x = content.x;
    for (text, part) in &pieces {
        let style = match part {
            _ if lit => bright_bold(),
            StripPart::Mood => Style::default().fg(th().ok),
            StripPart::Level => Style::default(),
            StripPart::Sep => dim(),
        };
        put(frame, x, y, text, style);
        x += width(text) as u16;
    }
    gui.ui.click(words, Act::Mood(MoodAct::Open { key: false }));
    if clipped {
        gui.ui.tip(words, run.iter().map(|(text, _)| text.as_str()).collect::<String>());
    }
    let close = Rect { x: x + 1, y, width: 3, height: 1 };
    put(frame, close.x, y, "[X]", if gui.ui.hovers(close) { bright_bold() } else { dim() });
    gui.ui.click(close, Act::Mood(MoodAct::Drop));
}

// ── The modal ───────────────────────────────────────────────────────────────

/// The radio pair, chosen and not, by terminal.
fn radio_glyphs() -> (&'static str, &'static str) {
    if legacy_conhost() { ("(*)", "( )") } else { ("(•)", "( )") }
}

/// How a choice stop wears its state: chosen as a mood (OK), chosen as a
/// level or a Sort (the Radio group's ACCENT), not chosen, or a resting
/// Match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wears {
    Mood,
    Radio,
    Off,
    Rest,
}

/// The modal over the room (clause 32), drawn and registered over
/// everything beneath: the room's registries dropped, the whole frame
/// inert, the band cleared where it has one; the title on the top border;
/// the rows of choices in the locale's columns; the foot.
pub(crate) fn draw_modal(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if !is_open(gui) {
        return;
    }
    let Some(draft) = gui.moods.draft else { return };
    let locale = rust_i18n::locale();
    let (rect, band) = place(area, gui.room_rect(), modal_rows(draft.sorts()));
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let own = band.unwrap_or(rect);
    gui.ui.overlay(own);
    gui.ui.modal_over();
    if let Some(band) = band {
        crate::kit::blank(frame, band);
    }
    super::keep::modal_box(frame, rect);
    gui.ui.click(own, Act::Guard);
    super::keep::border_title(frame, rect, &t!("gui.moods.title"));

    let cols = modal_cols(&locale);
    let x0 = rect.x + 2;
    let top = rect.y + 1;
    let filter = draft.moods.filter;
    let shown_sort = draft.moods.shown_sort();
    let (rows, foot) = stop_rows(&draft);
    let hand = |stop: MoodStop| draft.shown && draft.hand == stop;
    // The choice rows: the scales and the tags, a blank, the level, and on a
    // song list the Sort.
    for (r, row) in rows.iter().enumerate() {
        let y = top + if r < 4 { r as u16 } else { r as u16 + 1 };
        let stops: [Option<MoodStop>; 3] = if r == 5 {
            // The SORT row: its word in the first column, and Match drawn
            // while it rests, no stop.
            put(frame, x0 + cols[0], y, &t!("gui.moods.sort"), dim());
            [None, row[1], Some(MoodStop::Sort(MoodSort::Match))]
        } else {
            *row
        };
        for (c, stop) in stops.iter().enumerate() {
            let Some(stop) = *stop else { continue };
            let wears = match stop {
                MoodStop::Pick(scale, value) if filter.value_of(scale) == Some(value) => Wears::Mood,
                MoodStop::Tag(tag) if filter.has_tag(tag) => Wears::Mood,
                MoodStop::Level(level) if filter.level() == level => Wears::Radio,
                MoodStop::Sort(MoodSort::Match) if !match_awake(filter) => Wears::Rest,
                MoodStop::Sort(sort) if sort == shown_sort => Wears::Radio,
                _ => Wears::Off,
            };
            let word = stop_label(draft.nav, stop, &locale);
            draw_choice(frame, gui, x0 + cols[c], y, stop, &word, wears, hand(stop));
        }
    }
    // The foot: `[ Done ]` ending three cells in from the right corner, and
    // `clear` three cells left of it while there is something to clear.
    let y = rect.bottom().saturating_sub(2);
    let done_word = stop_label(draft.nav, MoodStop::Done, &locale);
    let done_w = width(&done_word) as u16;
    let done = Rect { x: rect.right().saturating_sub(2 + done_w), y, width: done_w, height: 1 };
    if foot.contains(&MoodStop::Clear) {
        let word = stop_label(draft.nav, MoodStop::Clear, &locale);
        let w = width(&word) as u16;
        let clear = Rect { x: done.x.saturating_sub(3 + w), y, width: w, height: 1 };
        let style = if hand(MoodStop::Clear) {
            accent().add_modifier(Modifier::BOLD)
        } else if gui.ui.hovers(clear) {
            bright_bold()
        } else {
            dim()
        };
        put(frame, clear.x, y, &word, style);
        gui.ui.click(clear, Act::Mood(MoodAct::Press(MoodStop::Clear)));
    }
    let style = if hand(MoodStop::Done) {
        sel().add_modifier(Modifier::BOLD)
    } else if gui.ui.hovers(done) {
        bright_bold()
    } else {
        accent().add_modifier(Modifier::BOLD)
    };
    put(frame, done.x, y, &done_word, style);
    gui.ui.click(done, Act::Mood(MoodAct::Press(MoodStop::Done)));
}

/// One choice: its glyph, a space and its word — the glyph in its state's
/// colour whatever lights the word, the word BRIGHT BOLD under the pointer
/// and ACCENT BOLD under the keyboard's hand, which wins. A click on the
/// glyph through the word presses it; a resting Match takes none.
#[allow(clippy::too_many_arguments)]
fn draw_choice(
    frame: &mut Frame,
    gui: &mut Gui,
    x: u16,
    y: u16,
    stop: MoodStop,
    word: &str,
    wears: Wears,
    hand: bool,
) {
    let (on, off) = if matches!(stop, MoodStop::Tag(_)) { super::check_glyphs() } else { radio_glyphs() };
    let ok = Style::default().fg(th().ok);
    let (glyph, glyph_style, word_style) = match wears {
        Wears::Mood => (on, ok, ok.add_modifier(Modifier::BOLD)),
        Wears::Radio => (on, accent(), Style::default().add_modifier(Modifier::BOLD)),
        Wears::Off => (off, dim(), Style::default()),
        Wears::Rest => (off, dim(), dim()),
    };
    let rect = Rect { x, y, width: 4 + width(word) as u16, height: 1 };
    let word_style = if wears == Wears::Rest {
        word_style
    } else if hand {
        accent().add_modifier(Modifier::BOLD)
    } else if gui.ui.hovers(rect) {
        bright_bold()
    } else {
        word_style
    };
    put(frame, x, y, glyph, glyph_style);
    put(frame, x + 4, y, word, word_style);
    if wears != Wears::Rest {
        gui.ui.click(rect, Act::Mood(MoodAct::Press(stop)));
    }
}

// ── Tips ────────────────────────────────────────────────────────────────────

/// The tips line while the modal is up.
pub(crate) fn tips() -> String {
    t!("gui.tips.moods").to_string()
}

/// A room's resting tips line with ` · o mood` at its end, where the room
/// offers the opener and the line still fits `cells` (the kit's hint law:
/// hints compress, never disappear) — not while the filter's field takes
/// the letters.
pub(crate) fn tips_tail<'a>(gui: &Gui, base: Cow<'a, str>, cells: usize) -> Cow<'a, str> {
    if !offered_here(gui) || gui.app.filtering {
        return base;
    }
    tailed(base, &t!("gui.tips.mood_key"), cells)
}

/// `base` with ` · {tail}` at its end where the whole still fits `cells`,
/// else `base` as it was.
fn tailed<'a>(base: Cow<'a, str>, tail: &str, cells: usize) -> Cow<'a, str> {
    let line = format!("{base} · {tail}");
    if width(&line) <= cells { Cow::Owned(line) } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [&str; 10] = ["en", "de", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

    fn draft(nav: usize, filter: MoodFilter) -> MoodDraft {
        MoodDraft { nav, moods: RoomMoods { filter, sort: MoodSort::Own }, hand: FIRST, shown: true, col: 0 }
    }

    fn relaxed() -> MoodFilter {
        MoodFilter::NONE.press(MoodScale::Energy, "relaxed")
    }

    fn text(run: &[(String, StripPart)]) -> String {
        run.iter().map(|(t, _)| t.as_str()).collect()
    }

    #[test]
    fn the_modal_columns_come_from_the_widest_options() {
        // Instrumental, Standard and Aggressive are English's widest: the
        // columns at x19, x45 and x67 of a box at x17.
        assert_eq!(modal_cols("en"), [0, 26, 48]);
        assert_eq!((modal_rows(false), modal_rows(true)), (10, 11));
        for code in LOCALES {
            let cols = modal_cols(code);
            assert!(cols[1] > cols[0] && cols[2] > cols[1], "{code}: {cols:?}");
        }
    }

    #[test]
    fn the_modal_sits_over_the_frame_at_47_and_in_its_band_at_80() {
        let frame = Rect::new(0, 0, 100, 30);
        // The queue open: 47 cells of room, the kit's place over the frame.
        let room = super::super::content_rect(100, 30, true, false);
        assert_eq!(place(frame, room, 10), (Rect::new(17, 10, 66, 10), None));
        assert_eq!(place(frame, room, 11), (Rect::new(17, 9, 66, 11), None));
        let small = Rect::new(0, 0, 100, 24);
        let room = super::super::content_rect(100, 24, true, true);
        assert_eq!(place(small, room, 10).0, Rect::new(17, 7, 66, 10));
        assert_eq!(place(small, room, 11).0, Rect::new(17, 6, 66, 11));
        assert!(place(small, room, 11).0.bottom() <= room.bottom());
        // The queue folded: centred in the room's 80 cells, the band its
        // whole width on the modal's rows.
        let room = super::super::content_rect(100, 30, false, false);
        assert_eq!(place(frame, room, 10), (Rect::new(24, 10, 66, 10), Some(Rect::new(17, 10, 80, 10))));
    }

    #[test]
    fn the_stops_walk_in_reading_order_skipping_a_resting_match_and_a_hidden_clear() {
        use MoodStop::*;
        let resting = draft(RECENT_NAV, MoodFilter::NONE);
        let order = reading_order(&resting);
        assert_eq!(order.first(), Some(&Pick(MoodScale::Valence, "happy")));
        assert_eq!(&order[order.len() - 3..], &[Level(MoodLevel::Loose), Sort(MoodSort::Own), Done]);
        assert!(!order.contains(&Sort(MoodSort::Match)) && !order.contains(&Clear));
        // Tab wraps from Done to the first stop, Shift-Tab back.
        let at_done = MoodDraft { hand: Done, ..resting };
        assert_eq!(walk(&at_done, KeyCode::Tab).0, FIRST);
        assert_eq!(walk(&resting, KeyCode::BackTab).0, Done);
        // A mood wakes Match and brings `clear`.
        let awake = draft(RATED_NAV, relaxed());
        let order = reading_order(&awake);
        assert_eq!(&order[order.len() - 4..], &[Sort(MoodSort::Own), Sort(MoodSort::Match), Clear, Done]);
        // A lone level brings `clear` but leaves Match resting; Discover and
        // the lists of names have no SORT row.
        let lone = draft(RECENT_NAV, MoodFilter::NONE.with_level(MoodLevel::Strict));
        assert_eq!(&reading_order(&lone)[14..], &[Sort(MoodSort::Own), Clear, Done]);
        let albums = reading_order(&draft(ALBUMS_NAV, MoodFilter::NONE));
        assert_eq!(albums.len(), 14 + 1);
        assert!(!albums.iter().any(|s| matches!(s, Sort(_))));
    }

    #[test]
    fn the_arrows_keep_the_column_clamp_and_land_on_own_from_a_resting_match() {
        use MoodStop::*;
        let at = |nav: usize, filter: MoodFilter, hand: MoodStop, col: u8| MoodDraft {
            hand,
            col,
            ..draft(nav, filter)
        };
        // ↓ keeps the column; Vocals' row has two, so the third column
        // clamps to its last — and the column is kept for the row after.
        let sad = at(RECENT_NAV, MoodFilter::NONE, Pick(MoodScale::Valence, "sad"), 2);
        let (hand, col) = walk(&sad, KeyCode::Down);
        assert_eq!((hand, col), (Pick(MoodScale::Energy, "aggressive"), 2));
        let (hand, col) = walk(&MoodDraft { hand, col, ..sad }, KeyCode::Down);
        assert_eq!((hand, col), (Pick(MoodScale::Voice, "vocals"), 2));
        let (hand, _) = walk(&MoodDraft { hand, col, ..sad }, KeyCode::Down);
        assert_eq!(hand, Tag("danceable"));
        // ↓ from the tags is the level; from Loose onto a resting Match it
        // lands on the own order; from Strict, SORT's column, on the stop
        // to its right.
        let tag = at(RECENT_NAV, MoodFilter::NONE, Tag("acoustic"), 0);
        assert_eq!(walk(&tag, KeyCode::Down).0, Level(MoodLevel::Strict));
        let loose = at(RECENT_NAV, MoodFilter::NONE, Level(MoodLevel::Loose), 2);
        assert_eq!(walk(&loose, KeyCode::Down).0, Sort(MoodSort::Own));
        let strict = at(RECENT_NAV, MoodFilter::NONE, Level(MoodLevel::Strict), 0);
        assert_eq!(walk(&strict, KeyCode::Down).0, Sort(MoodSort::Own));
        let awake = at(RECENT_NAV, relaxed(), Level(MoodLevel::Loose), 2);
        assert_eq!(walk(&awake, KeyCode::Down).0, Sort(MoodSort::Match));
        // ↓ from the last choice row is Done; ↑ from the foot is that row
        // at the remembered column; no wrap either way.
        let own = at(RECENT_NAV, relaxed(), Sort(MoodSort::Match), 2);
        assert_eq!(walk(&own, KeyCode::Down).0, Done);
        let done = at(RECENT_NAV, relaxed(), Done, 2);
        assert_eq!(walk(&done, KeyCode::Down).0, Done);
        assert_eq!(walk(&done, KeyCode::Up), (Sort(MoodSort::Match), 2));
        let done = at(ALBUMS_NAV, relaxed(), Done, 1);
        assert_eq!(walk(&done, KeyCode::Up).0, Level(MoodLevel::Standard));
        assert_eq!(walk(&at(ALBUMS_NAV, relaxed(), FIRST, 0), KeyCode::Up).0, FIRST);
        // ← → walk the row, clamped; on the foot between clear and Done.
        let happy = at(ALBUMS_NAV, relaxed(), FIRST, 0);
        assert_eq!(walk(&happy, KeyCode::Left), (FIRST, 0));
        assert_eq!(walk(&happy, KeyCode::Right), (Pick(MoodScale::Valence, "neutral"), 1));
        assert_eq!(walk(&done, KeyCode::Left).0, Clear);
        assert_eq!(walk(&MoodDraft { hand: Clear, ..done }, KeyCode::Left).0, Clear);
        assert_eq!(walk(&MoodDraft { hand: Clear, ..done }, KeyCode::Right).0, Done);
    }

    #[test]
    fn a_press_edits_the_draft_and_the_hand_leaves_a_stop_that_went() {
        use MoodStop::*;
        let mut d = MoodDraft { hand: Sort(MoodSort::Match), ..draft(RATED_NAV, relaxed()) };
        press(&mut d, Sort(MoodSort::Match));
        assert_eq!(d.moods.sort, MoodSort::Match);
        // The last mood let go: Match rests, and the hand goes to the own
        // order; the choice of Match stays for the first mood.
        press(&mut d, Pick(MoodScale::Energy, "relaxed"));
        settle(&mut d);
        assert_eq!((d.hand, d.moods.sort, d.moods.shown_sort()), (Sort(MoodSort::Own), MoodSort::Match, MoodSort::Own));
        // The own order pressed while Match rests does nothing.
        press(&mut d, Sort(MoodSort::Own));
        assert_eq!(d.moods.sort, MoodSort::Match);
        // clear lets the moods go and the level back to Standard, the Sort
        // kept; the hand on clear goes to Done.
        let mut d = MoodDraft { hand: Clear, ..draft(RATED_NAV, relaxed().with_level(MoodLevel::Strict)) };
        d.moods.sort = MoodSort::Match;
        press(&mut d, Clear);
        settle(&mut d);
        assert_eq!((d.moods.filter, d.moods.sort, d.hand), (MoodFilter::NONE, MoodSort::Match, Done));
    }

    #[test]
    fn the_strip_names_scales_then_tags_then_a_non_standard_level() {
        let filter = MoodFilter::NONE
            .toggle_tag("party")
            .press(MoodScale::Voice, "instrumental")
            .press(MoodScale::Energy, "relaxed")
            .with_level(MoodLevel::Strict);
        let run = strip_run(filter, "en");
        assert_eq!(text(&run), "Relaxed · Instrumental · Party · Strict");
        let parts: Vec<StripPart> = run.iter().map(|(_, p)| *p).collect();
        use StripPart::*;
        assert_eq!(parts, [Mood, Sep, Mood, Sep, Mood, Sep, Level]);
        assert_eq!(text(&strip_run(relaxed(), "en")), "Relaxed", "Standard says nothing");
    }

    #[test]
    fn the_strip_clips_its_words_before_x_and_keeps_x() {
        crate::kit::theme::pin_modern_terminal();
        // The board's run fits the queue's 47 cells whole.
        let set = relaxed().press(MoodScale::Voice, "instrumental").with_level(MoodLevel::Strict);
        let (fit, clipped) = strip_fit(&strip_run(set, "en"), 42);
        assert!(!clipped);
        assert_eq!(text(&fit), "Relaxed · Instrumental · Strict");
        // Everything set: clipped by cells with the mark, in every locale.
        let all = MoodFilter::NONE
            .press(MoodScale::Valence, "happy")
            .press(MoodScale::Energy, "aggressive")
            .press(MoodScale::Voice, "instrumental")
            .toggle_tag("acoustic")
            .toggle_tag("party")
            .toggle_tag("danceable")
            .with_level(MoodLevel::Loose);
        for code in LOCALES {
            let run = strip_run(all, code);
            let (fit, clipped) = strip_fit(&run, 42);
            let shown = text(&fit);
            assert!(clipped && shown.ends_with('…'), "{code}: {shown}");
            assert!(width(&shown) <= 42, "{code}: {shown}");
            assert!(text(&run).starts_with(shown.trim_end_matches('…')), "{code}: {shown}");
        }
    }

    #[test]
    fn the_opener_stands_after_the_name_or_ends_13_in_or_left_of_a_wide_pager() {
        let at47 = super::super::content_rect(100, 30, true, false);
        let at80 = super::super::content_rect(100, 30, false, false);
        // The lists: two cells after the name — Rated x24, Recent x25,
        // Artists x26.
        for (name, x) in [("Rated", 24), ("Recent", 25), ("Artists", 26)] {
            assert_eq!(opener_x(at47, OpenerAt::AfterName(width(name) as u16), 4), x);
        }
        // The wall: ending at x51 (x84 at 80 cells), clear of `◂ 1/1 ▸` and
        // `◂ 10/12 ▸`, whose back arrow is at x56 and x54; a wider pager
        // pulls it left.
        assert_eq!(opener_x(at47, OpenerAt::BeforePager(56), 4), 48);
        assert_eq!(opener_x(at47, OpenerAt::BeforePager(54), 4), 48);
        assert_eq!(opener_x(at47, OpenerAt::BeforePager(52), 4), 46);
        assert_eq!(opener_x(at80, OpenerAt::BeforePager(89), 4), 81);
    }

    // ── Every locale (no lock: the words are asked in their locale) ──────

    #[test]
    fn every_locale_fits_the_modal_in_66_cells() {
        let awake = draft(RATED_NAV, relaxed());
        let recent = draft(RECENT_NAV, relaxed());
        for code in LOCALES {
            let widths = col_widths(code);
            let cols = modal_cols(code);
            // The three columns and their two gaps in the rows' 62 cells.
            let need = widths.iter().sum::<u16>() + 2 * COL_GAP;
            assert!(need <= MODAL_TEXT, "{code}: {need} cells: {widths:?}");
            assert!(cols[2] + widths[2] <= MODAL_TEXT, "{code}: {cols:?} {widths:?}");
            // Every stop's glyph, space and word in its own column, the next
            // three cells or more away — the Sort words of both lists too.
            for d in [awake, recent] {
                for row in stop_rows(&d).0 {
                    for (c, stop) in row.iter().enumerate() {
                        let Some(stop) = *stop else { continue };
                        let end = cols[c] + 4 + width(&stop_label(d.nav, stop, code)) as u16;
                        let limit = if c < 2 { cols[c + 1] - COL_GAP } else { MODAL_TEXT };
                        assert!(end <= limit, "{code}: {stop:?} ends at {end}, past {limit}");
                    }
                }
            }
            // SORT within the first column.
            let sort = width(&t!("gui.moods.sort", locale = code)) as u16;
            assert!(sort <= widths[0] && sort + COL_GAP <= cols[1], "{code}: SORT {sort}, {cols:?}");
            // The title on the top border with a rule cell each side of its
            // spaces; the foot, `clear` three cells from `[ Done ]`.
            let title = width(&t!("gui.moods.title", locale = code)) as u16;
            assert!(title + 4 <= MODAL_W - 2, "{code}: title {title}");
            let foot = width(&stop_label(RATED_NAV, MoodStop::Clear, code))
                + 3
                + width(&stop_label(RATED_NAV, MoodStop::Done, code));
            assert!(foot <= usize::from(MODAL_TEXT), "{code}: foot {foot}");
        }
    }

    #[test]
    fn every_locales_strip_fits_47_cells() {
        crate::kit::theme::pin_modern_terminal();
        let content = super::super::content_rect(100, 30, true, false);
        // The words' cells, as the strip draws them: `[X]` and its space,
        // and the cell the run keeps from the content's last.
        let cells = usize::from(content.width - 5);
        let set = relaxed().press(MoodScale::Voice, "instrumental").with_level(MoodLevel::Strict);
        let all = MoodFilter::NONE
            .press(MoodScale::Valence, "happy")
            .press(MoodScale::Energy, "aggressive")
            .press(MoodScale::Voice, "instrumental")
            .toggle_tag("acoustic")
            .toggle_tag("party")
            .toggle_tag("danceable")
            .with_level(MoodLevel::Strict);
        for code in LOCALES {
            // The board's set whole, `[X]` and all, in 46 cells (x17–x62).
            let (fit, clipped) = strip_fit(&strip_run(set, code), cells);
            let x_end = content.x + width(&text(&fit)) as u16 + 1 + 3 - 1;
            assert!(!clipped && x_end <= content.right() - 2, "{code}: {} ends at x{x_end}", text(&fit));
            // Everything set: clipped before `[X]`, which still ends at x62.
            let (fit, clipped) = strip_fit(&strip_run(all, code), cells);
            let x_end = content.x + width(&text(&fit)) as u16 + 1 + 3 - 1;
            assert!(clipped && x_end <= content.right() - 2, "{code}: {} ends at x{x_end}", text(&fit));
        }
    }

    #[test]
    fn line_one_keeps_two_cells_between_its_halves_in_every_locale() {
        use super::super::discover::plural_in;
        let content = super::super::content_rect(100, 30, true, false);
        let of = |shown: u64, total: u64, code: &str| t!("gui.bar.of", shown = shown, total = total, locale = code).to_string();
        for code in LOCALES {
            let mood = width(&opener_word(code)) as u16;
            // The wall: the count two after the name, the opener ending at
            // x51 (clear of a two-digit pager, `◂ 10/12 ▸`); a four-digit
            // count, or `999 of 9999`, keeps two cells to it.
            let name = width(&t!("gui.nav.albums", locale = code)) as u16;
            let opener = opener_x(content, OpenerAt::BeforePager(54), mood);
            assert_eq!(opener + mood - 1, 51, "{code}");
            let count_x = content.x + name + 2;
            for count in [plural_in("gui.alb.count", 9999, code), of(999, 9999, code)] {
                let end = count_x + width(&count) as u16 - 1;
                assert!(end + 2 < opener, "{code}: {count:?} ends at x{end}, the opener at x{opener}");
                assert!(width(&count) <= count_cells(count_x, opener), "{code}: {count:?} clipped");
            }
            // A three- or four-digit pager (`◂ 100/167 ▸`, `◂ 1000/1667 ▸`,
            // back arrow at x52 and x50) pulls the opener left, and a
            // five-digit count is wider: the count, as drawn, still ends
            // two cells before it, its digits whole.
            for (back, n) in [(52, 9999), (50, 9999), (54, 99_999), (50, 99_999)] {
                let opener = opener_x(content, OpenerAt::BeforePager(back), mood);
                assert_eq!(opener + mood - 1 + 3, back.min(54), "{code}: the opener three before the pager");
                for (count, digits) in
                    [(plural_in("gui.alb.count", n, code), true), (of(999, n, code), false)]
                {
                    let drawn = super::super::bar::clip(&count, count_cells(count_x, opener));
                    let end = count_x + width(&drawn) as u16 - 1;
                    assert!(end + 2 < opener, "{code} at x{back}: {drawn:?} to x{end}, the opener at x{opener}");
                    assert!(!digits || drawn.starts_with(&n.to_string()), "{code} at x{back}: {drawn:?}");
                }
            }
            // The lists: the opener two after the name, the count ending at
            // x62 — five digits whole, and five of five under the text filter.
            for nav in ["gui.nav.artists", "gui.nav.recent", "gui.nav.rated"] {
                let name = width(&t!(nav, locale = code)) as u16;
                let opener = opener_x(content, OpenerAt::AfterName(name), mood);
                for count in [plural_in("gui.files.items", 99_999, code), of(99_999, 99_999, code)] {
                    let x = super::super::right_at(content, &count);
                    assert_eq!(x + width(&count) as u16 - 1, 62, "{code}: {count:?}");
                    assert!(opener + mood - 1 + 2 < x, "{code} {nav}: the opener at x{opener}, {count:?} at x{x}");
                }
            }
        }
    }

    #[test]
    fn the_rated_label_fits_the_nav_column_in_every_locale() {
        // Twelve cells, x3–x14, or `▸ ` and the label from x1 while active.
        for code in LOCALES {
            let label = t!("gui.nav.rated", locale = code);
            assert!(width(&label) <= 12, "{code}: {label}");
        }
    }

    #[test]
    fn the_moods_tips_lines_fit_ninety_nine_cells_in_every_locale() {
        // The footer's cells at the 100-column floor, drawn from x1.
        const CELLS: usize = 99;
        let resting = [
            "gui.tips.albums",
            "gui.tips.files",
            "gui.tips.files_stowed",
            "gui.tips.discover",
            "gui.tips.discover_strip",
            "gui.tips.discover_fallback",
        ];
        for code in LOCALES {
            let up = t!("gui.tips.moods", locale = code);
            assert!(width(&up) <= CELLS, "{code}: {up}");
            let tail = t!("gui.tips.mood_key", locale = code);
            for key in resting {
                let base = t!(key, locale = code);
                let line = tailed(base.clone(), &tail, CELLS);
                // Kept where the whole line fits, dropped where it would not,
                // never clipped.
                let fits = width(&base) + 3 + width(&tail) <= CELLS;
                assert_eq!(line.ends_with(&format!(" · {tail}")), fits, "{code} {key}: {line}");
                assert!(!fits || width(&line) <= CELLS, "{code} {key}: {line}");
            }
            // The wall's resting line has the cells in every locale.
            let wall = tailed(t!("gui.tips.albums", locale = code), &tail, CELLS);
            assert!(wall.ends_with(tail.as_ref()), "{code}: {wall}");
        }
    }
}
