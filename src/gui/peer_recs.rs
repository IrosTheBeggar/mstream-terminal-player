//! The Peer recs room: songs the connected peers' shelves suggest from the listener's own taste
//! (peer-recs contract §1). It draws the room's rhythm (peer-recs contract §1.3): the taste
//! bar's header on row 2, the peer strip on row 4, the verbs line or the live clip's band on
//! row 6, the heads and the rule on rows 7 and 8, then the two-column grid of leads or one of
//! the panes. Its parts are children: `tabs` (the strip), `grid` and `panes` (the leads and
//! the words that stand in for them) and `band` (row 6).
//!
//! This file owns what joins them: the cursor and the tabs stop and their laws (peer-recs
//! contract §1.2), the room's keys and mouse acts, the notes, the copy, and the room's
//! lifetime as the App sees it, Shown, Covered or Away (peer-recs contract §1.8). The App's
//! room (`tui::app::peer_recs`) holds every answer; this file holds only where the listener
//! stands.

mod band;
mod grid;
mod panes;
mod tabs;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use rust_i18n::t;

use crate::clock::Instant;
use crate::kit::ListView;
use crate::kit::clipboard::Copied;
use crate::tui::app::peer_recs::previews::PreviewNote;
use crate::tui::app::peer_recs::rules::{RecsPane, StripAct, StripItem};
use crate::tui::app::peer_recs::{PeerRecsWatch, StripView};
use crate::tui::app::taste::TasteState;
use crate::tui::app::{App, Effect, Tab};

use super::discover::taste_bar::{self, BarAct, BarRoom, BarUi, HeadFacts};
use super::discover::{bold, word_refusal};
use super::{Act, Gui, List, Screen};

/// The room's nav row.
const ROOM_NAV: usize = super::PEER_RECS_NAV;

/// A strip item's id: the peer's endpoint id, `None` for All. The tabs stop, its memory and
/// the strip's first tab in view are held by it, never by index, so a poll that reshuffles
/// the strip never moves them (peer-recs contract §1.5).
pub(crate) type TabKey = Option<String>;

/// What a pointer press did in the room. The shell wraps it as `Act::Peer` and routes it back
/// to [`act`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PeerAct {
    /// A tab, by its id (`None` for All); a `+` tab's add, by the peer's id; the strip's
    /// arrows and the wheel over it, by whole tabs.
    Tab(Option<String>),
    Add(String),
    TabsScroll(i32),
    /// A lead's cell (opens its sheet) and its preview slot, by lead index.
    Row(usize),
    Slot(usize),
    /// The verbs line: "▸ preview all" and "copy list".
    PreviewAll,
    CopyList,
    /// The band: play or pause, the next lead, the attribution's link, "[ get it ]" and [X].
    Toggle,
    Next,
    Attribution,
    GetIt,
    Close,
    /// The panes' buttons: "↻ new seeds", "Try again" and "Check again".
    NewSeeds,
    TryAgain,
    CheckAgain,
    /// The taste bar's header and modals.
    Bar(BarAct),
}

/// Where the listener stands in the room. The answers are the App's; this is the cursor, the
/// tabs stop and the strip's scroll, plus the bar's own state.
#[derive(Default)]
pub(crate) struct PeerRecsUi {
    /// The taste bar's window list and seeds modal.
    pub(crate) bar: BarUi,
    /// The grid's scroll, in rows of screen blocks.
    pub(crate) list: ListView,
    /// The held lead, by its index in the shown list; `None`: stowed (peer-recs contract §1.2).
    pub(crate) cursor: Option<usize>,
    /// The tabs stop, by the held tab's id; `None`: no stop.
    pub(crate) stop: Option<TabKey>,
    /// The tab the stop last held, which ↑ from the grid's first row lands on again.
    pub(crate) stop_left: Option<TabKey>,
    /// The strip's first tab in view, by id.
    pub(crate) first_tab: TabKey,
    /// The selected tab the grid was last drawn for: another one opens its list at the top.
    grid_tab: TabKey,
    /// The room was drawn this frame: its watch is Shown.
    drawn: bool,
    /// The first tab in view as last resolved, by index: where the strip stays when the tab
    /// that was first leaves it.
    first_at: usize,
    /// The tabs row and the grid's rows per screen as last drawn, for the keys.
    tabs_row: Rect,
    rows: u16,
}

impl PeerRecsUi {
    /// The keyboard's hand goes down, from the grid and from the strip (the list-cursor law).
    /// The tab the stop last held goes with it: `↑` goes back to it only within one trip from
    /// the strip into the grid (peer-recs contract §1.2.2).
    fn stow(&mut self) {
        self.cursor = None;
        self.list.stow();
        self.stop = None;
        self.stop_left = None;
    }

    /// A fresh visit: nothing held, the grid and the strip at their starts.
    fn fresh(&mut self) {
        self.stow();
        self.list = ListView::default();
        self.stop_left = None;
        self.first_tab = None;
        self.first_at = 0;
        self.grid_tab = None;
    }
}

// ── The room's side of the taste bar ────────────────────────────────────────

/// The room's side of the taste bar: its `BarUi` on `gui.peer_recs`, the room's own
/// `TasteState` (never Discover's), and its verbs. The window is `[gui] discover_window`, which
/// both rooms read on enter (build plan D15).
pub(crate) struct Room;

impl BarRoom for Room {
    const NAV: usize = ROOM_NAV;

    fn bar(gui: &mut Gui) -> &mut BarUi {
        &mut gui.peer_recs.bar
    }

    fn bar_ref(gui: &Gui) -> &BarUi {
        &gui.peer_recs.bar
    }

    fn taste(app: &App) -> &TasteState {
        &app.peer_recs.taste
    }

    fn facts(gui: &Gui) -> HeadFacts {
        facts_in(&gui.app, &rust_i18n::locale())
    }

    fn wrap(act: BarAct) -> Act {
        Act::Peer(PeerAct::Bar(act))
    }

    fn can_pin_playing(app: &App) -> bool {
        app.peer_recs_can_pin_playing()
    }

    fn pin_playing(app: &mut App) -> bool {
        app.peer_recs_pin_playing()
    }

    fn unpin(app: &mut App, path: &str) -> bool {
        app.peer_recs_unpin(path)
    }

    fn seeds_closed(app: &mut App, dropped: &[String], restored: &[String], pins_changed: bool) -> Vec<Effect> {
        app.peer_recs_seeds_closed(dropped, restored, pins_changed)
    }

    /// A window picked: asked at once, keeping the tab, and remembered in `[gui]
    /// discover_window`. A failed ask keeps it, as Discover's does (build plan D15).
    fn set_window(gui: &mut Gui, window: u32) {
        let effects = gui.app.peer_recs_set_window(window);
        gui.pend(effects);
        if gui.config.gui.discover_window != Some(window) {
            gui.config.gui.discover_window = Some(window);
            gui.save_now();
        }
    }

    /// The pick is armed in the sonic room's grammar, over the Files room. The room is
    /// covered, not left, while it is out (build plan D32).
    fn choose_song(gui: &mut Gui) {
        let effects = gui.app.peer_recs_choose_song();
        gui.pend(effects);
        gui.active = super::FILES_NAV;
        gui.app.tab = Tab::Files;
    }

    fn new_seeds(gui: &mut Gui) {
        enter(gui, true);
    }

    /// The window's list and the seeds modal cover row 6: a live clip ends there (peer-recs
    /// contract §1.8).
    fn modal_opened(gui: &mut Gui) {
        if gui.app.peer_recs_clip_live() {
            let effects = gui.app.peer_recs_clip_stop(true);
            gui.pend(effects);
        }
    }
}

/// The header's facts in `locale` (peer-recs contract §1.4): the room's name and its short form
/// (the nav word), the window shown, an ask out, and no mood opener: the room never sends moods.
/// The refresh has no tooltip: Discover's names `G`, and the room's key is `R`.
pub(crate) fn facts_in(app: &App, locale: &str) -> HeadFacts {
    HeadFacts {
        name: t!("gui.peer_recs.name", locale = locale).to_string(),
        short: Some(t!("gui.nav.peer_recs", locale = locale).to_string()),
        window: app.peer_recs_window_shown(),
        widened_from: None,
        widened: None,
        asking: app.peer_recs_asking(),
        moods: None,
        refresh_tip: None,
    }
}

// ── The gate and the room's lifetime ────────────────────────────────────────

/// The nav row's gate (peer-recs contract §1.1, build plan D31): the four flags, the user's own
/// session and no off latch. No `connected` term, as Discover's has none: a short disconnect
/// keeps the row and draws the disconnected words.
pub(crate) fn gate(app: &App) -> bool {
    app.peer_recs_gate()
}

/// A seed pick armed from this room's seeds modal is out (build plan D32).
pub(crate) fn picking(app: &App) -> bool {
    app.peer_recs_picking()
}

/// The room is up: the Library screen, this room in front.
fn up(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == ROOM_NAV
}

/// A press that starts a clip — `l`, `L`, a slot, the sheet's Preview — goes only while the last
/// frame drew the room. Under the mini player or another screen its watch is Covered, and a clip
/// never plays out of sight of its band (peer-recs contract §1.8): the frame that hid the room
/// ended the clip once, and nothing may start another unseen.
pub(crate) fn press(gui: &mut Gui, index: usize, now: Instant) -> Vec<Effect> {
    if gui.app.peer_recs.watch != PeerRecsWatch::Shown {
        return Vec::new();
    }
    gui.app.peer_recs_press(index, now)
}

/// Preview all, on [`press`]'s terms.
fn preview_all(gui: &mut Gui, now: Instant) -> Vec<Effect> {
    if gui.app.peer_recs.watch != PeerRecsWatch::Shown {
        return Vec::new();
    }
    gui.app.peer_recs_preview_all(now)
}

/// The window the room's asks start from: `[gui] discover_window`, read on every enter.
fn remembered_window(gui: &Gui) -> Option<u32> {
    gui.config.gui.discover_window.and_then(crate::tui::app::taste::taste_window)
}

/// Report the room's watch to the App, once per change.
fn watch(gui: &mut Gui, watch: PeerRecsWatch) {
    if gui.app.peer_recs.watch == watch {
        return;
    }
    let effects = gui.app.peer_recs_watch(watch);
    gui.pend(effects);
}

/// The room is entered. `again` (`R` or the nav row while the room is up, a pick's way back):
/// new seeds, the visit kept (R3). Otherwise a fresh visit with the window from `[gui]
/// discover_window`: R1, the first poll and the plug-in gate together, nothing held.
pub(crate) fn enter(gui: &mut Gui, again: bool) {
    taste_bar::close(&mut gui.peer_recs.bar);
    let effects = if again {
        gui.app.peer_recs_new_seeds()
    } else {
        gui.peer_recs.fresh();
        let window = remembered_window(gui);
        gui.app.peer_recs_open(window)
    };
    gui.pend(effects);
}

/// The room is left by the nav: the bar's modals close, and the App ends the visit, its poll
/// and its clip (peer-recs contract §1.8).
pub(crate) fn leave(gui: &mut Gui) {
    taste_bar::close(&mut gui.peer_recs.bar);
    gui.peer_recs.stow();
    gui.peer_recs.drawn = false;
    watch(gui, PeerRecsWatch::Away);
}

/// The room is not drawn but stays the room in front (another screen, the mini player): the
/// poll pauses, a live clip ends, the visit stays.
pub(crate) fn hidden(gui: &mut Gui) {
    gui.peer_recs.drawn = false;
    if gui.active == ROOM_NAV || picking(&gui.app) {
        watch(gui, PeerRecsWatch::Covered);
    }
}

/// The session moved (a connect, a switch): a fresh visit on the new server, or, when the row
/// went with it, the room yields to Files.
pub(crate) fn reopen(gui: &mut Gui) {
    taste_bar::close(&mut gui.peer_recs.bar);
    if !gate(&gui.app) {
        gui.act(Act::Nav(super::FILES_NAV));
        return;
    }
    gui.peer_recs.fresh();
    let window = remembered_window(gui);
    let effects = gui.app.peer_recs_open(window);
    gui.pend(effects);
}

/// A pick armed from the room's seeds modal landed, was refused or was let go: back to the room,
/// which words a refusal in [`settle`]. A landed pin asks new seeds in the App.
pub(crate) fn returned(gui: &mut Gui) {
    gui.act(Act::Screen(Screen::Library));
    gui.active = ROOM_NAV;
    gui.peer_recs.stow();
}

/// The banner over the library while the room's pick is armed.
pub(crate) fn banner(gui: &Gui) -> Option<String> {
    picking(&gui.app).then(|| t!("gui.discover.pick_banner").to_string())
}

/// After the event fold, every frame (build plan D31, D32). When the gate goes while the room is
/// up (R7's re-ping said off), the bar's modals close and the room yields to Files in the same
/// frame. A room pick's refusal is worded once, with Discover's words.
pub(crate) fn settle(gui: &mut Gui) {
    if up(gui) && !gate(&gui.app) {
        taste_bar::close(&mut gui.peer_recs.bar);
        gui.act(Act::Nav(super::FILES_NAV));
    }
    if let Some(refusal) = gui.app.peer_recs.taste.refused.take() {
        word_refusal(gui, refusal);
    }
}

/// At the end of every frame: what the room was is the App's watch (peer-recs contract §1.8).
/// Drawn is Shown; the room in front but not drawn, or a pick of its own out, is Covered;
/// anything else is Away. The sheet and the bar's modals are drawn over the room, so they are
/// not a cover. A note the previews owe is worded at the bar's note, dim.
pub(crate) fn frame_tick(gui: &mut Gui) {
    let drawn = std::mem::take(&mut gui.peer_recs.drawn);
    // Drawn over a closed visit: a server switch whose `Connected` landed off the Library, where
    // `reopen_room` passes the room by. The room on screen is a fresh visit on this session
    // (peer-recs contract §1.1.8). A visit kept under another screen is still open, and stands.
    if drawn && gui.active == ROOM_NAV && !gui.app.peer_recs_visit_open() && gate(&gui.app) {
        reopen(gui);
    }
    let state = if drawn {
        PeerRecsWatch::Shown
    } else if gui.active == ROOM_NAV || picking(&gui.app) {
        PeerRecsWatch::Covered
    } else {
        PeerRecsWatch::Away
    };
    watch(gui, state);
    if let Some(PreviewNote::NoPreview) = gui.app.peer_recs_take_note() {
        gui.note = Some((t!("gui.peer_recs.no_preview").to_string(), false));
    }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The room (peer-recs contract §1.3), in Discover's rhythm with no card and no strip of
/// albums: the header, the peer strip, the verbs line or the band, the heads and the rule, then
/// the grid or the pane that stands for it.
pub(crate) fn draw(frame: &mut Frame, gui: &mut Gui, content: Rect) {
    if !gui.app.connected {
        // Not drawn: a clip never plays out of sight of its band (peer-recs contract §1.8).
        put_name(frame, content);
        super::servers::draw_disconnected(frame, gui, content, 2);
        return;
    }
    gui.peer_recs.drawn = true;
    let lay = super::discover::layout(content, false, false);
    taste_bar::draw_header::<Room>(frame, gui, content, lay.header_y);
    draw_strip(frame, gui, Rect { x: content.x, y: lay.tabs_y, width: content.width, height: 1 });

    // The sheet follows its lead when a new answer moves it (build plan D30), and the grid's
    // hand, held under the sheet, follows with it; a shorter answer clamps the hand.
    let n = gui.app.peer_recs_leads().len();
    if gui.peer_recs.list.held
        && gui.peer_recs.cursor.is_some()
        && let Some(at) = super::actions::lead_open(gui)
    {
        gui.peer_recs.cursor = Some(at);
    }
    if let Some(at) = gui.peer_recs.cursor
        && at >= n
    {
        if n == 0 {
            gui.peer_recs.stow();
        } else {
            gui.peer_recs.cursor = Some(n - 1);
        }
    }

    // Row 6: the live clip's band, else the verbs line while the tab has rows.
    let pane = gui.app.peer_recs_pane();
    let offered = gui.app.peer_recs_preview_offered();
    if let Some(band) = gui.app.peer_recs_band() {
        let get_it = band.index.is_some() && gui.app.peer_recs_get_it_on();
        band::draw_band(frame, &mut gui.ui, content, lay.verbs_y, &band, get_it);
    } else if pane == RecsPane::Ready {
        band::draw_verbs(frame, &mut gui.ui, content, lay.verbs_y, offered);
    }
    draw_grid(frame, gui, content, lay.table_y, pane, offered);
}

/// The strip on `row`: the skeleton before the first poll answers, nothing when All would stand
/// alone, else the tabs from the first in view, with the tabs stop. The stop and the first tab
/// are held by id and found again here every frame (build plan D30): a stop whose tab left falls
/// back to the selected tab, else the first in view, and the stop is kept in view.
fn draw_strip(frame: &mut Frame, gui: &mut Gui, row: Rect) {
    gui.peer_recs.tabs_row = row;
    let view = gui.app.peer_recs_strip();
    // Another tab selected, by a click, the stop's Enter, a stowed ← →, or the App itself
    // (R5's way back to All): its list opens at the top, as Discover's `set_tab` does. New seeds
    // on the same tab keep the grid's place (R3).
    if gui.peer_recs.grid_tab != view.selected {
        gui.peer_recs.grid_tab = view.selected.clone();
        gui.peer_recs.list.scroll = 0;
    }
    if view.skeleton {
        gui.peer_recs.stop = None;
        tabs::draw_skeleton(frame, row);
        return;
    }
    if !view.drawn || view.items.is_empty() {
        // No strip, so no stop (peer-recs contract §1.2).
        gui.peer_recs.stop = None;
        gui.peer_recs.first_tab = None;
        gui.peer_recs.first_at = 0;
        return;
    }
    let items = &view.items;
    let mut first = first_index(&gui.peer_recs, items);
    let mut stop = None;
    if let Some(key) = gui.peer_recs.stop.clone() {
        let at = index_of(items, &key).or_else(|| tabs::stop_landing(&view, &tabs::run(items, row, first)));
        if let Some(at) = at {
            tabs::reveal(items, row, &mut first, at);
        }
        gui.peer_recs.stop = at.map(|at| items[at].id.clone());
        stop = at;
    }
    gui.peer_recs.first_at = first;
    gui.peer_recs.first_tab = items[first].id.clone();
    tabs::draw(frame, &mut gui.ui, row, &view, first, stop);
}

/// The heads and the rule, then the grid while the tab has rows, else its pane (peer-recs
/// contract §1.7, §1.10). The grid's scroll is in rows, and the hand is a lead: the row it
/// stands on is found from the blocks this frame lays out.
fn draw_grid(frame: &mut Frame, gui: &mut Gui, content: Rect, table_y: u16, pane: RecsPane, slot: bool) {
    let list = Rect {
        x: content.x,
        y: table_y + 2,
        width: content.width.saturating_sub(2),
        height: content.bottom().saturating_sub(table_y + 2),
    };
    let geom = grid::geom(list, slot);
    gui.peer_recs.rows = geom.rows;
    grid::draw_heads(frame, content, &geom, table_y);
    if pane != RecsPane::Ready {
        panes::draw(frame, &mut gui.ui, list, &geom, pane);
        return;
    }
    let n = gui.app.peer_recs_leads().len();
    let total = grid::total_rows(n, geom.rows);
    let held = gui.peer_recs.list.shown(gui.peer_recs.cursor);
    let at_row = gui.peer_recs.cursor.map(|at| grid::cell_of(at, n, geom.rows).0);
    let (first_row, _) = gui.peer_recs.list.window(total, at_row, usize::from(geom.rows));
    grid::draw(frame, &mut gui.ui, &gui.app, &geom, held, first_row);
}

/// The room's name, alone over the disconnected words, as Discover draws its own.
fn put_name(frame: &mut Frame, content: Rect) {
    super::put(frame, content.x, content.y, &t!("gui.nav.peer_recs"), bold());
}

/// The bar's window list and seeds modal, over the room.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    taste_bar::draw_modals::<Room>(frame, gui, area);
}

// ── The strip, by key ───────────────────────────────────────────────────────

/// Where the item keyed `key` stands in the strip.
fn index_of(items: &[StripItem], key: &TabKey) -> Option<usize> {
    items.iter().position(|item| &item.id == key)
}

/// The first tab in view: by its id, else where the strip last stood.
fn first_index(ui: &PeerRecsUi, items: &[StripItem]) -> usize {
    index_of(items, &ui.first_tab).unwrap_or(ui.first_at.min(items.len().saturating_sub(1)))
}

/// Scroll the strip, by whole tabs, so the item keyed `key` is in view.
fn reveal_key(ui: &mut PeerRecsUi, items: &[StripItem], key: &TabKey) {
    let Some(at) = index_of(items, key) else { return };
    let mut first = first_index(ui, items);
    tabs::reveal(items, ui.tabs_row, &mut first, at);
    ui.first_at = first;
    ui.first_tab = items[first].id.clone();
}

/// The strip `delta` whole tabs along: the arrows' clicks and the wheel over the strip.
fn scroll_tabs(gui: &mut Gui, delta: i32) {
    let view = gui.app.peer_recs_strip();
    if !view.drawn || view.items.is_empty() {
        return;
    }
    let first = first_index(&gui.peer_recs, &view.items);
    let first = tabs::scroll(&view.items, gui.peer_recs.tabs_row, first, delta);
    gui.peer_recs.first_at = first;
    gui.peer_recs.first_tab = view.items[first].id.clone();
}

/// The tabs stop onto the tab keyed `key`, kept in view; the grid's hand goes down (one hand).
fn hold_tab(gui: &mut Gui, items: &[StripItem], key: TabKey) {
    gui.peer_recs.cursor = None;
    gui.peer_recs.list.stow();
    reveal_key(&mut gui.peer_recs, items, &key);
    gui.peer_recs.stop_left = Some(key.clone());
    gui.peer_recs.stop = Some(key);
}

/// The keys reach the strip (peer-recs contract §1.2): onto `back` when it is still on the
/// strip, else the selected tab when it is in view, else the first tab in view. Nothing when
/// the strip is not drawn.
fn to_strip(gui: &mut Gui, view: &StripView, back: Option<TabKey>) {
    if !view.drawn || view.items.is_empty() {
        return;
    }
    let back = back.filter(|key| index_of(&view.items, key).is_some());
    let key = back.or_else(|| {
        let first = first_index(&gui.peer_recs, &view.items);
        let run = tabs::run(&view.items, gui.peer_recs.tabs_row, first);
        tabs::stop_landing(view, &run).map(|at| view.items[at].id.clone())
    });
    if let Some(key) = key {
        hold_tab(gui, &view.items, key);
    }
}

// ── Acts ────────────────────────────────────────────────────────────────────

/// Handle the room's acts. True when the act was this room's.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    let Act::Peer(act) = act else { return false };
    match act.clone() {
        PeerAct::Bar(act) => taste_bar::act::<Room>(gui, &act),
        // A click acts and stows the cursor and the tabs stop (the list-cursor law; peer-recs
        // contract §1.2).
        other => {
            gui.peer_recs.stow();
            pointer(gui, other);
        }
    }
    true
}

/// A pointer press in the room, the hand already stowed.
fn pointer(gui: &mut Gui, act: PeerAct) {
    let now = Instant::now();
    let effects = match act {
        PeerAct::Tab(id) => gui.app.peer_recs_select(id),
        PeerAct::Add(id) => gui.app.peer_recs_add(&id),
        PeerAct::TabsScroll(delta) => {
            scroll_tabs(gui, delta);
            Vec::new()
        }
        PeerAct::Row(index) => {
            open(gui, index, false);
            Vec::new()
        }
        PeerAct::Slot(index) => press(gui, index, now),
        PeerAct::PreviewAll => preview_all(gui, now),
        PeerAct::CopyList => {
            let text = gui.app.peer_recs_copy_list();
            copy(gui, text);
            Vec::new()
        }
        PeerAct::Toggle => gui.app.peer_recs_clip_toggle(),
        PeerAct::Next => gui.app.peer_recs_clip_next(now),
        // The attribution opens the catalogue's page, https only (peer-recs contract §1.8).
        PeerAct::Attribution => {
            if let Some(link) = gui.app.peer_recs_band().and_then(|band| band.link) {
                super::keep::open_url(gui, &link);
            }
            Vec::new()
        }
        // "[ get it ]": the live lead's sheet, its cursor on Get it.
        PeerAct::GetIt => {
            if let Some(index) = gui.app.peer_recs_band().and_then(|band| band.index) {
                open(gui, index, true);
            }
            Vec::new()
        }
        PeerAct::Close => gui.app.peer_recs_clip_stop(true),
        PeerAct::NewSeeds => {
            enter(gui, true);
            Vec::new()
        }
        PeerAct::TryAgain => gui.app.peer_recs_retry(),
        PeerAct::CheckAgain => gui.app.peer_recs_check_again(),
        PeerAct::Bar(_) => Vec::new(),
    };
    gui.pend(effects);
}

/// A lead opens its sheet: Enter, `m`, a row click, a right click, the band's "[ get it ]"
/// (`on_get_it`). With no plug-in at all the sheet would hold Copy title alone, so the lead is
/// copied instead and no sheet opens (peer-recs contract §1.9).
fn open(gui: &mut Gui, index: usize, on_get_it: bool) {
    if gui.app.peer_recs_plugins_on() {
        super::actions::open_lead(gui, index, on_get_it);
    } else {
        let text = gui.app.peer_recs_copy_text(index);
        copy(gui, text);
    }
}

/// Copy `text` through the kit's clipboard and say so at the bar's note, dim (peer-recs contract
/// §1.13): `Copied` for the clipboard and for OSC 52, which nothing confirms, and `Couldn't
/// copy` when no route took it. Nothing to copy copies and says nothing.
fn copy(gui: &mut Gui, text: Option<String>) {
    let Some(text) = text else { return };
    let words = match crate::kit::clipboard::copy(&text) {
        Copied::Clipboard | Copied::Terminal => t!("gui.peer_recs.copied"),
        Copied::Failed => t!("gui.peer_recs.copy_failed"),
    };
    gui.note = Some((words.to_string(), false));
}

/// The room's wheel: the bar's seeds modal while it is up (the window's list holds), the strip a
/// tab a notch under the pointer (build plan D23), else the grid.
pub(crate) fn wheel(gui: &mut Gui, delta: i32) {
    if taste_bar::wheel::<Room>(gui, delta) {
        return;
    }
    let row = gui.peer_recs.tabs_row;
    if gui.ui.pointer.is_some_and(|at| row.contains(at)) && gui.app.peer_recs_strip().drawn {
        scroll_tabs(gui, delta.signum());
        return;
    }
    gui.act(Act::ScrollBy(List::PeerRecs, delta));
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The room's keys (peer-recs contract §1.2; build plan D22, D23), and the bar's, which outrank
/// them while one of its modals is up. The sheet's come first, in the shell. `Some(quit)` when
/// the key was the room's.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !up(gui) {
        return None;
    }
    if let Some(quit) = taste_bar::handle_key::<Room>(gui, key) {
        return Some(quit);
    }
    if !gui.app.connected {
        return None;
    }
    let now = Instant::now();
    let live = gui.app.peer_recs_clip_live();
    // While a clip is live Space, `n` and `x` are its own; otherwise they stay main's.
    let clip = match key.code {
        KeyCode::Char(' ') if live => Some(gui.app.peer_recs_clip_toggle()),
        KeyCode::Char('n') if live => Some(gui.app.peer_recs_clip_next(now)),
        KeyCode::Char('x') if live => Some(gui.app.peer_recs_clip_stop(true)),
        _ => None,
    };
    if let Some(effects) = clip {
        gui.pend(effects);
        return Some(false);
    }
    let view = gui.app.peer_recs_strip();
    if gui.peer_recs.stop.is_some()
        && let Some(quit) = stop_key(gui, key, &view)
    {
        return Some(quit);
    }
    let n = gui.app.peer_recs_leads().len();
    let held = gui.peer_recs.list.shown(gui.peer_recs.cursor).filter(|at| *at < n);
    let offered = gui.app.peer_recs_preview_offered();
    match key.code {
        // Esc: the sheet first (the shell's), then the hand, then a live clip.
        KeyCode::Esc => {
            if held.is_some() {
                gui.peer_recs.stow();
            } else if live {
                let effects = gui.app.peer_recs_clip_stop(true);
                gui.pend(effects);
            } else {
                return None;
            }
        }
        // Stowed: ↓ picks the first lead and ↑ the last (the kit's list-cursor law); with no
        // rows under the strip both land on the tabs stop.
        KeyCode::Down | KeyCode::PageDown | KeyCode::Up | KeyCode::PageUp if held.is_none() => {
            let down = matches!(key.code, KeyCode::Down | KeyCode::PageDown);
            if n > 0 {
                pick(gui, if down { 0 } else { n - 1 });
            } else {
                to_strip(gui, &view, None);
            }
        }
        // Stowed ← → step the selected tab at once, its ask 250 ms after the last step (R8),
        // and the strip keeps it in view.
        KeyCode::Left | KeyCode::Right if held.is_none() => {
            let dir = if key.code == KeyCode::Left { -1 } else { 1 };
            let effects = gui.app.peer_recs_step(dir, now);
            gui.pend(effects);
            gui.peer_recs.stop_left = None;
            let view = gui.app.peer_recs_strip();
            reveal_key(&mut gui.peer_recs, &view.items, &view.selected);
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::PageUp | KeyCode::PageDown => {
            let at = held?;
            walk(gui, &view, at, key.code, n);
        }
        // Enter and `m` open the held lead's sheet, and never play; stowed, they do nothing.
        KeyCode::Enter | KeyCode::Char('m') => {
            if let Some(at) = held {
                open(gui, at, false);
            }
        }
        KeyCode::Char('l') => {
            if let Some(at) = held.filter(|_| offered) {
                let effects = press(gui, at, now);
                gui.pend(effects);
            }
        }
        KeyCode::Char('L') => {
            if offered {
                let effects = preview_all(gui, now);
                gui.pend(effects);
            }
        }
        KeyCode::Char('y') => {
            let text = gui.app.peer_recs_copy_list();
            copy(gui, text);
        }
        _ => return None,
    }
    Some(false)
}

/// A walking key on a held lead (build plan D22).
fn walk(gui: &mut Gui, view: &StripView, at: usize, code: KeyCode, n: usize) {
    match grid::walk(at, code, n, gui.peer_recs.rows.max(1)) {
        grid::Walk::To(to) => pick(gui, to),
        // ↑ from a column's first row: back to the tab the stop last held, else per §1.2.
        grid::Walk::Tabs => {
            let back = gui.peer_recs.stop_left.clone();
            to_strip(gui, view, back);
        }
        grid::Walk::Stay => {}
    }
}

/// The keyboard takes lead `at`; the tabs stop goes (one hand).
fn pick(gui: &mut Gui, at: usize) {
    gui.peer_recs.stop = None;
    gui.peer_recs.cursor = Some(at);
    gui.peer_recs.list.pick_up();
}

/// A key while the tabs stop holds a tab (peer-recs contract §1.2): ← → move it over the live
/// tabs, ↓ goes to the grid's first row, Enter acts on the held tab, Esc stows it. `None` passes
/// the key on to the room's own keys.
fn stop_key(gui: &mut Gui, key: KeyEvent, view: &StripView) -> Option<bool> {
    if !view.drawn || view.items.is_empty() {
        gui.peer_recs.stop = None;
        return None;
    }
    let items = &view.items;
    let held = gui.peer_recs.stop.clone()?;
    let at = index_of(items, &held).unwrap_or(first_index(&gui.peer_recs, items));
    match key.code {
        KeyCode::Left | KeyCode::Right => {
            let dir = if key.code == KeyCode::Left { -1 } else { 1 };
            if let Some(to) = tabs::stop_step(items, at, dir) {
                hold_tab(gui, items, items[to].id.clone());
            }
        }
        KeyCode::Down | KeyCode::PageDown => {
            if !gui.app.peer_recs_leads().is_empty() {
                pick(gui, 0);
            }
        }
        // The strip is the room's top: nothing above it.
        KeyCode::Up | KeyCode::PageUp => {}
        KeyCode::Enter => {
            let item = &items[at];
            let effects = match (item.acts(), item.id.clone()) {
                (StripAct::Select, id) => gui.app.peer_recs_select(id),
                (StripAct::Add, Some(id)) => gui.app.peer_recs_add(&id),
                _ => Vec::new(),
            };
            gui.pend(effects);
        }
        KeyCode::Esc => gui.peer_recs.stow(),
        _ => return None,
    }
    Some(false)
}

// ── Tips ────────────────────────────────────────────────────────────────────

/// The tips line for the room's states (build plan D21), first match wins: the bar's modals,
/// the tabs stop, a held row, a live clip, then the stowed lines. The sheet's own line comes
/// first in the shell.
pub(crate) fn tips(gui: &Gui) -> String {
    if let Some(tips) = taste_bar::tips::<Room>(gui) {
        return tips;
    }
    if gui.peer_recs.stop.is_some() {
        return t!("gui.tips.peer_recs_tabs").to_string();
    }
    if gui.peer_recs.cursor.is_some() && gui.peer_recs.list.held {
        let key = if gui.app.peer_recs_preview_offered() { "gui.tips.peer_recs_row" } else { "gui.tips.peer_recs_row_plain" };
        return t!(key).to_string();
    }
    if gui.app.peer_recs_clip_live() {
        return t!("gui.tips.peer_recs_clip").to_string();
    }
    if gui.app.peer_recs_leads().is_empty() {
        t!("gui.tips.peer_recs_empty").to_string()
    } else {
        t!("gui.tips.peer_recs").to_string()
    }
}

/// One of the bar's modals is up over the room. Off the room a modal left standing (a pick's Esc
/// that moved the room in front without the nav) holds nothing: it is not drawn there.
pub(crate) fn modal_open(gui: &Gui) -> bool {
    up(gui) && taste_bar::modal_open(&gui.peer_recs.bar)
}

/// While the room is up every pass draws: the band's bar moves, the poll and the debounce tick,
/// and the first frame drawn without the room ends its clip.
pub(crate) fn moving(gui: &Gui) -> bool {
    up(gui)
}

/// The grid's viewport, for the shell's scrollbar acts.
pub(crate) fn list_mut(gui: &mut Gui) -> &mut ListView {
    &mut gui.peer_recs.list
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// The room's test kit, shared with its parts' tests (build plan §2.Q "Test scaffolding").
#[cfg(test)]
pub(crate) mod kit {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    use super::super::{Act, Gui, PEER_RECS_NAV, bar, render};
    use crate::api::peer_recs_door::{PeerRecsCmd, PeerRecsEvent};
    use crate::api::types::fixtures;
    use crate::api::types::{
        Capabilities, CollectionDestinationView, DiscoveryPluginsResponse, Ping, PluginPreview, TasteResponse,
        TranscodeInfo,
    };
    use crate::config::Config;
    use crate::tui::app::{App, Effect};
    use crate::tui::worker::{ApiCmd, Event};

    /// The stage's server, as the boards' top bar names it.
    pub(crate) const HOST: &str = "http://127.0.0.1:3995";

    /// The tests read English words.
    pub(crate) fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    /// The room's gate: the four flags, and the plug-ins route the gate reads.
    pub(crate) fn caps() -> Capabilities {
        Capabilities {
            discovery: true,
            discovery_path: true,
            discovery_p2p: true,
            discovery_taste: 3,
            discovery_p2p_peers: 1,
            discovery_plugins: true,
            ..Capabilities::default()
        }
    }

    /// The stage's plug-in listing: links, the Deezer preview and noop-acquire's Get it
    /// (TERM-DECISIONS §2 "Other plug-ins").
    pub(crate) fn plugins() -> DiscoveryPluginsResponse {
        serde_json::from_value(json!({
            "plugins": [
                { "name": "links", "title": "Links", "available": true, "capabilities": ["links"] },
                { "name": "deezer", "title": "Deezer", "available": true, "capabilities": ["preview"] },
                { "name": "noop-acquire", "title": "Noop acquire", "available": true, "capabilities": ["acquire"] }
            ],
            "jobs": { "allowed": true }
        }))
        .expect("the stage's plug-in listing parses")
    }

    /// The stage admin's collection destination, where a Get it lands.
    pub(crate) fn destination() -> CollectionDestinationView {
        serde_json::from_value(json!({
            "destination": { "vpath": "music", "base": "Downloads", "layout": "{artist}/{album}", "source": "default" },
            "libraries": [ { "vpath": "music" } ]
        }))
        .expect("a destination parses")
    }

    /// Main's song on every board (TERM-DECISIONS §1.11): Glass Hours at 1:24 of 3:52.
    pub(crate) fn glass_hours() -> bar::Now {
        bar::Now {
            title: "Glass Hours".into(),
            artist: "Color Out".into(),
            album: "Low Light".into(),
            elapsed: 84.0,
            duration: 232.0,
            year: None,
            spec: "MP3 · 320 kbps · 44.1 kHz".into(),
            rating: None,
            key: None,
            bpm: None,
        }
    }

    /// A connected session on the stage's server with the room's gate: the queue folded, key
    /// hints off and no pointer, Glass Hours in the bar, as the boards draw.
    pub(crate) fn peer_gui() -> Gui {
        crate::kit::theme::pin_modern_terminal();
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui.app.capabilities = caps();
        gui.queue_open = false;
        gui.config.gui.key_hints = false;
        gui.ui.key_hints = false;
        gui.ui.pointer = None;
        gui.demo = Some(glass_hours());
        gui.app.volume = 0.8;
        // Add from YouTube shows, as on the boards: a library, uploads and audio conversion.
        gui.app.libraries = vec!["music".into()];
        let ping = Ping {
            vpaths: vec!["music".into()],
            transcode: Some(TranscodeInfo::default()),
            no_upload: Some(false),
            ytdl: 1,
            ..Ping::default()
        };
        let connected =
            Event::Connected { server: HOST.into(), id: HOST.into(), username: None, token: None, ping: Box::new(ping) };
        super::super::ytdl::observe(&mut gui, &connected);
        gui
    }

    /// The room's asks among `effects`.
    pub(crate) fn door(effects: &[Effect]) -> Vec<PeerRecsCmd> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::PeerRecs { cmd, .. }) => Some(cmd.clone()),
                _ => None,
            })
            .collect()
    }

    /// An answer through the App's own event path, as the worker delivers it.
    pub(crate) fn land(gui: &mut Gui, event: PeerRecsEvent) {
        let generation = gui.app.session_gen();
        let effects = gui.app.apply_event(Event::PeerRecs { generation, event: Box::new(event) });
        gui.pend(effects);
    }

    /// Every room ask pended so far answered from the stage's fixtures: the taste asks with
    /// `taste` (a fixture name), the polls with `peers`, the gate with [`plugins`] and
    /// [`destination`]. What the answers asked in turn stays pended.
    pub(crate) fn answer(gui: &mut Gui, peers: &str, taste: &str) {
        answer_with(gui, peers, &fixtures::taste(taste), true);
    }

    /// [`answer`] with a taste answer of the test's own. With `gate` false the gate's ask is
    /// left unanswered, and its seq is returned.
    pub(crate) fn answer_with(gui: &mut Gui, peers: &str, taste: &TasteResponse, gate: bool) -> Option<u64> {
        let mut held = None;
        for cmd in door(&std::mem::take(&mut gui.pending)) {
            match cmd {
                PeerRecsCmd::Gate { seq } if gate => {
                    let plugins = Ok(Box::new(plugins()));
                    let destination = Some(Ok(Box::new(destination())));
                    land(gui, PeerRecsEvent::Gate { seq, plugins, destination });
                }
                PeerRecsCmd::Gate { seq } => held = Some(seq),
                PeerRecsCmd::Taste { seq, .. } => {
                    land(gui, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(taste.clone())) });
                }
                PeerRecsCmd::Peers { poll } => {
                    land(gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(fixtures::peers(peers))) });
                }
                _ => {}
            }
        }
        held
    }

    /// A taste answer from the fixture `name`, its network list changed by `edit` (the JSON's
    /// `network.results`).
    pub(crate) fn taste_edited(name: &str, edit: impl FnOnce(&mut Vec<serde_json::Value>)) -> TasteResponse {
        let mut value = fixtures::json(name);
        let results = value["network"]["results"].as_array_mut().expect("a network list");
        edit(results);
        serde_json::from_value(value).expect("the edited answer parses")
    }

    /// The room up on the stage's answers: `peers` and `taste` answer the opening asks, and
    /// nothing is left pended.
    pub(crate) fn room(peers: &str, taste: &str) -> Gui {
        let mut gui = peer_gui();
        gui.act(Act::Nav(PEER_RECS_NAV));
        assert_eq!(gui.active, PEER_RECS_NAV, "the gate lets the room in");
        answer(&mut gui, peers, taste);
        gui.pending.clear();
        gui
    }

    /// ALL PEERS · IDLE's room: the admin's strip and All's twelve leads.
    pub(crate) fn admin_room() -> Gui {
        room("peers_admin", "taste_all")
    }

    /// The press's resolve answered with the stage's Deezer clip for that lead; the ticket the
    /// engine was handed back, if the App played it.
    pub(crate) fn resolve(gui: &mut Gui) -> Option<u64> {
        let ticket = door(&gui.pending).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Preview { ticket, .. } => Some(ticket),
            _ => None,
        })?;
        gui.pending.clear();
        let found = fixtures::preview("preview_deezer");
        let preview = PluginPreview::from_value(&found["result"]["preview"], Some("deezer")).map(Box::new);
        land(gui, PeerRecsEvent::Preview { ticket, answer: Ok(preview) });
        gui.pending.iter().find_map(|e| match e {
            Effect::Audio(crate::tui::worker::AudioCmd::AsidePlay { ticket, .. }) => Some(*ticket),
            _ => None,
        })
    }

    /// The engine says where clip `ticket` stands.
    pub(crate) fn sounding(gui: &mut Gui, ticket: u64, phase: crate::player::AsidePhase, position: f64) {
        let status = crate::player::AsideStatus { ticket, phase, position, duration: 30.0 };
        let effects = gui.app.apply_event(Event::Aside(Some(status)));
        gui.pend(effects);
    }

    /// One frame at `width` × `height`, through the shell's own render.
    pub(crate) fn frame(gui: &mut Gui, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal.draw(|frame| render(frame, gui)).expect("a frame");
        terminal.backend().buffer().clone()
    }

    /// The text of every row of `buf`.
    pub(crate) fn rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (0..area.height).map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect()
    }

    /// A key, through the shell's own key path.
    pub(crate) fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyCode;

    use super::super::{Act, List, RowVerb, Screen};
    use super::kit::*;
    use super::{Gui, PeerAct};
    use crate::api::peer_recs_door::{PeerRecsCmd, PeerRecsEvent, RecsFailure};
    use crate::api::types::fixtures::{self, ATTIC, KITCHEN};
    use crate::kit::boards::{Area, assert_frame};
    use crate::player::AsidePhase;
    use crate::tui::app::Effect;
    use crate::tui::worker::{ApiCmd, AudioCmd};

    /// Hold a whole frame to a board. The bar's transport buttons are the bar's, and they draw
    /// their frames plain where the boards draw them BOLD, so the three button rows leave out x
    /// 27–46 (the previous, play and next buttons) and keep the play button's glyphs (x 35–38),
    /// which say whether main is paused. `footer`: the tips line is drawn under the bar.
    #[track_caller]
    fn assert_board(buf: &Buffer, board: &str, label: &str, footer: bool) {
        let h = buf.area.height;
        let t = h - 5 - u16::from(footer);
        assert_frame(buf, board, label, Area { rows: Some(0..=t), ..Area::whole() });
        assert_frame(buf, board, label, Area { rows: Some(t + 1..=t + 3), cols: Some(0..=26), y_at: None });
        assert_frame(buf, board, label, Area { rows: Some(t + 1..=t + 3), cols: Some(47..=99), y_at: None });
        assert_frame(buf, board, label, Area { rows: Some(t + 2..=t + 2), cols: Some(35..=38), y_at: None });
        assert_frame(buf, board, label, Area { rows: Some(t + 4..=h - 1), ..Area::whole() });
    }

    /// The last taste ask pended answered with the fixture `name`.
    fn land_taste(gui: &mut Gui, name: &str) {
        let seq = door(&gui.pending).into_iter().rev().find_map(|cmd| match cmd {
            PeerRecsCmd::Taste { seq, .. } => Some(seq),
            _ => None,
        });
        let seq = seq.expect("a taste ask is out");
        gui.pending.clear();
        land(gui, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(fixtures::taste(name))) });
    }

    /// A press on the lead at `index` answered: with the stage's Deezer clip, or with no preview
    /// for the visit (`None`).
    fn press(gui: &mut Gui, index: usize, found: bool) -> Option<u64> {
        gui.act(Act::Peer(PeerAct::Slot(index)));
        if found {
            return resolve(gui);
        }
        let ticket = door(&gui.pending).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Preview { ticket, .. } => Some(ticket),
            _ => None,
        });
        gui.pending.clear();
        land(gui, PeerRecsEvent::Preview { ticket: ticket.expect("a press resolves"), answer: Ok(None) });
        gui.pending.clear();
        None
    }

    /// PREVIEW PLAYING's room: Arcade Rain answered no preview, Signal Fires sounds at 12 s of
    /// 30, main paused in place.
    fn playing_room(phase: AsidePhase) -> Gui {
        let mut gui = admin_room();
        press(&mut gui, 7, false);
        let ticket = press(&mut gui, 2, true).expect("Signal Fires plays");
        sounding(&mut gui, ticket, phase, 12.0);
        gui.demo_paused = true;
        gui.pending.clear();
        gui
    }

    // ── The boards ──────────────────────────────────────────────────────────

    /// ALL PEERS · IDLE (TERM-DECISIONS §3, M1): the admin's strip, All's twelve leads 6 | 6 with
    /// idle slots, the cursor stowed and no clip.
    #[test]
    fn m1_all_peers_idle() {
        let _en = english();
        let mut gui = admin_room();
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "M1", "", false);
    }

    /// PREVIEW PLAYING (M2): the band over the verbs line, Signal Fires accent BOLD with `▮▮`,
    /// Arcade Rain muted, main's play frame `▸`.
    #[test]
    fn m2_preview_playing() {
        let _en = english();
        let mut gui = playing_room(AsidePhase::Playing);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "M2", "", false);
    }

    /// KITCHEN RADIO TAB (M4): two `›` clicks scroll the strip two tabs, then the Kitchen Radio
    /// tab is clicked and its five leads read 3 | 2.
    #[test]
    fn m4_kitchen_radio_tab() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        gui.act(Act::Peer(PeerAct::TabsScroll(1)));
        gui.act(Act::Peer(PeerAct::TabsScroll(1)));
        gui.act(Act::Peer(PeerAct::Tab(Some(KITCHEN.into()))));
        land_taste(&mut gui, "taste_kitchen");
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "M4", "", false);
    }

    /// TOOLS · FIRST ASK (W1): the room just opened, nothing answered: `↻ asking…`, the strip's
    /// skeleton, no verbs line, four skeleton rows a column.
    #[test]
    fn w1_first_ask() {
        let _en = english();
        let mut gui = peer_gui();
        gui.act(Act::Nav(super::super::PEER_RECS_NAV));
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "W1", "", false);
    }

    /// FLOOR · KEY HINTS (W2): 100 × 24 with the tips line, the nav's fourth rise.
    #[test]
    fn w2_floor_key_hints() {
        let _en = english();
        let mut gui = admin_room();
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let buf = frame(&mut gui, 100, 24);
        assert_board(&buf, "W2", "", true);
    }

    /// FLOOR · HINTS OFF (W3): 100 × 24, rise 3.
    #[test]
    fn w3_floor_hints_off() {
        let _en = english();
        let mut gui = admin_room();
        let buf = frame(&mut gui, 100, 24);
        assert_board(&buf, "W3", "", false);
    }

    /// NO PEERS YET (S1): the shelf emptied; a stowed ↓ with no rows lands the tabs stop on All,
    /// and → moves it to `+ Lighthouse Tapes`, All still selected in accent BOLD.
    #[test]
    fn s1_no_peers_yet() {
        let _en = english();
        let mut gui = room("peers_admin_empty_shelf", "taste_empty_shelf");
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Right);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S1", "", false);
    }

    /// NOTHING NEW (S2): the Attic Tapes tab answers nothing new.
    #[test]
    fn s2_nothing_new() {
        let _en = english();
        let mut gui = admin_room();
        gui.act(Act::Peer(PeerAct::Tab(Some(ATTIC.into()))));
        land_taste(&mut gui, "taste_attic");
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S2", "", false);
    }

    /// TRY AGAIN (S3): All's taste ask failed.
    #[test]
    fn s3_try_again() {
        let _en = english();
        let mut gui = peer_gui();
        gui.act(Act::Nav(super::super::PEER_RECS_NAV));
        let seq = door(&gui.pending).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Taste { seq, .. } => Some(seq),
            _ => None,
        });
        land(&mut gui, PeerRecsEvent::Taste { seq: seq.expect("R1 is out"), answer: Err(RecsFailure::Failed) });
        // The poll and the gate answer as the stage does; the taste ask has had its answer.
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::PeerRecs { cmd: PeerRecsCmd::Taste { .. }, .. })));
        answer(&mut gui, "peers_admin", "taste_all");
        gui.pending.clear();
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S3", "", false);
    }

    /// SCAN PENDING (S4): Discover's sentence over two rows, then `Check again`.
    #[test]
    fn s4_scan_pending() {
        let _en = english();
        let mut gui = room("peers_admin", "taste_scan_pending");
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S4", "", false);
    }

    /// ADD STATES (S5): the strip scrolled to its end: no room, adding…, try again; All selected
    /// out of view, its grid below.
    #[test]
    fn s5_add_states() {
        let _en = english();
        let mut gui = room("peers_admin_add_states", "taste_all");
        frame(&mut gui, 100, 30);
        gui.act(Act::Peer(PeerAct::TabsScroll(10)));
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S5", "", false);
    }

    /// PREVIEW PAUSED (S6): as M2 with the clip paused: the toggle `▸` accent BOLD, the bar held.
    #[test]
    fn s6_preview_paused() {
        let _en = english();
        let mut gui = playing_room(AsidePhase::Paused);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "S6", "", false);
    }

    /// MEMBER ACCOUNT (V1): the shelf alone, no `›`.
    #[test]
    fn v1_member() {
        let _en = english();
        let mut gui = room("peers_member", "taste_all");
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "V1", "", false);
    }

    /// NO PREVIEW PLUG-IN (V2): no slots, ARTIST to the cell's end, `copy list` alone, and `y`
    /// just pressed: `Copied` at the note.
    #[test]
    fn v2_no_preview_plugin() {
        let _en = english();
        let mut gui = peer_gui();
        gui.act(Act::Nav(super::super::PEER_RECS_NAV));
        for cmd in door(&std::mem::take(&mut gui.pending)) {
            match cmd {
                PeerRecsCmd::Gate { seq } => {
                    let mut listing = plugins();
                    listing.plugins.retain(|p| p.name != "deezer");
                    let destination = Some(Ok(Box::new(destination())));
                    land(&mut gui, PeerRecsEvent::Gate { seq, plugins: Ok(Box::new(listing)), destination });
                }
                PeerRecsCmd::Taste { seq, .. } => {
                    land(&mut gui, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(fixtures::taste("taste_all"))) });
                }
                PeerRecsCmd::Peers { poll } => {
                    land(&mut gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(fixtures::peers("peers_admin"))) });
                }
                _ => {}
            }
        }
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('y'));
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "V2", "", false);
    }

    /// QUEUE OPEN (V3): the room at 47 cells: the header at its second step, `›` at x 63, the
    /// 21-cell track-only columns. The queue panel right of x 65 is the panel's, not the room's,
    /// and is left out.
    #[test]
    fn v3_queue_open() {
        let _en = english();
        let mut gui = admin_room();
        gui.queue_open = true;
        let buf = frame(&mut gui, 100, 30);
        assert_frame(&buf, "V3", "", Area { rows: Some(0..=24), cols: Some(0..=65), y_at: None });
    }

    /// NATIVE WINDOW (V4): the window draws the room's buffer, so ALL PEERS · IDLE's buffer holds
    /// to it cell for cell.
    #[test]
    fn v4_is_m1s_buffer() {
        let _en = english();
        let mut gui = admin_room();
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "V4", "", false);
    }

    // ── Keys ────────────────────────────────────────────────────────────────

    /// The held lead, or `None` when the hand is down.
    fn held(gui: &Gui) -> Option<usize> {
        gui.peer_recs.list.shown(gui.peer_recs.cursor)
    }

    /// The tab the tabs stop holds, by id.
    fn stop(gui: &Gui) -> Option<Option<String>> {
        gui.peer_recs.stop.clone()
    }

    #[test]
    fn stowed_down_picks_the_first_lead_and_up_the_last() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        assert_eq!(held(&gui), Some(0), "↓ picks the first lead");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(held(&gui), None, "Esc stows");
        key(&mut gui, KeyCode::Up);
        assert_eq!(held(&gui), Some(11), "↑ picks the last lead");
        assert_eq!(stop(&gui), None, "no tabs stop while a row is held");
    }

    #[test]
    fn stowed_keys_land_on_the_tabs_with_no_rows() {
        let _en = english();
        let mut gui = room("peers_admin", "taste_attic");
        frame(&mut gui, 100, 30);
        assert!(gui.app.peer_recs_leads().is_empty());
        key(&mut gui, KeyCode::Down);
        assert_eq!(stop(&gui), Some(None), "↓ lands on the selected tab, All");
        assert_eq!(held(&gui), None);
        key(&mut gui, KeyCode::Esc);
        assert_eq!(stop(&gui), None, "Esc stows the stop");
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(None), "↑ lands there too");
    }

    #[test]
    fn up_from_the_first_row_climbs_to_the_tabs_and_back() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!((stop(&gui), held(&gui)), (Some(None), None), "↑ from the first row: the selected tab");
        key(&mut gui, KeyCode::Right);
        assert_eq!(stop(&gui), Some(Some(fixtures::BASEMENT.into())), "→ moves the stop");
        key(&mut gui, KeyCode::Down);
        assert_eq!((stop(&gui), held(&gui)), (None, Some(0)), "↓ goes to the grid's first row");
        key(&mut gui, KeyCode::Right);
        assert_eq!(held(&gui), Some(6), "→ to column B's first row");
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(Some(fixtures::BASEMENT.into())), "back to the tab the stop left");
        // Drawn: the held tab is the slab.
        let buf = frame(&mut gui, 100, 30);
        let x = x_of(&buf, 4, "Basement");
        assert_eq!(buf[(x, 4)].bg, crate::kit::theme::th().accent, "the stop's slab");
    }

    #[test]
    fn up_from_the_first_row_does_nothing_when_the_strip_is_not_drawn() {
        let _en = english();
        let mut gui = peer_gui();
        gui.act(Act::Nav(super::super::PEER_RECS_NAV));
        let alone = crate::api::types::P2pPeersAnswer::from_value(&serde_json::json!({
            "canAdd": false, "shelf": { "held": 1, "searchable": 1, "running": false }, "peers": []
        }));
        for cmd in door(&std::mem::take(&mut gui.pending)) {
            match cmd {
                PeerRecsCmd::Peers { poll } => {
                    land(&mut gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(alone.clone())) })
                }
                PeerRecsCmd::Taste { seq, .. } => {
                    land(&mut gui, PeerRecsEvent::Taste { seq, answer: Ok(Box::new(fixtures::taste("taste_all"))) })
                }
                _ => {}
            }
        }
        frame(&mut gui, 100, 30);
        assert!(!gui.app.peer_recs_strip().drawn, "All alone is not drawn");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!((stop(&gui), held(&gui)), (None, Some(0)), "nothing above the first row");
    }

    /// The App's next poll answered with `answer`.
    fn land_poll(gui: &mut Gui, answer: crate::api::types::P2pPeersAnswer) {
        let start = crate::clock::Instant::now();
        let mut poll = None;
        for wait in [0, 16, 32] {
            let effects = gui.app.tick_at(start + std::time::Duration::from_secs(wait));
            poll = door(&effects).into_iter().find_map(|cmd| match cmd {
                PeerRecsCmd::Peers { poll } => Some(poll),
                _ => None,
            });
            if poll.is_some() {
                break;
            }
        }
        let poll = poll.expect("the room polls");
        land(gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(answer)) });
        gui.pending.clear();
    }

    /// Found in the live walk: a stop stowed on Kitchen Radio brought a later `↑` from the grid
    /// back to Kitchen Radio, though the selected tab was All (peer-recs contract §1.2.2).
    #[test]
    fn up_forgets_the_stops_tab_once_the_hand_goes_down() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(None), "the selected tab, All");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Right);
        assert_eq!(stop(&gui), Some(Some(KITCHEN.into())));
        // One trip into the grid and back: the stop comes back to the tab it left.
        key(&mut gui, KeyCode::Down);
        assert_eq!(stop(&gui), None, "the grid holds the hand");
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(Some(KITCHEN.into())), "the tab the stop left");
        // Esc puts the hand down: the next trip up lands on the selected tab.
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(None), "All, not Kitchen Radio");
        // Stowed again, a step selects Basement Hi-Fi, and the next trip up lands there, not on
        // All, the tab the stop last held.
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.app.peer_recs_strip().selected.as_deref(), Some(fixtures::BASEMENT));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(Some(fixtures::BASEMENT.into())), "the selected tab");
    }

    #[test]
    fn the_stop_holds_its_tab_when_a_poll_moves_the_rows() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        // Two arrow clicks: Kitchen Radio first in view; then the keys bring the stop up there
        // (All is out of view) and over to mStream · 3f9a.
        gui.act(Act::Peer(PeerAct::TabsScroll(1)));
        gui.act(Act::Peer(PeerAct::TabsScroll(1)));
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(Some(KITCHEN.into())), "the first tab in view");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Right);
        assert_eq!(stop(&gui), Some(Some(fixtures::F3A.into())));
        let at = |gui: &Gui| gui.app.peer_recs_strip().items.iter().position(|i| i.id.as_deref() == Some(fixtures::F3A));
        let before = at(&gui);
        // An Add landed: Lighthouse Tapes joins the shelf, and Attic Tapes and Basement Hi-Fi
        // leave it, so every tab after All moves.
        let mut moved = fixtures::json("peers_admin");
        let peers = moved["peers"].as_array_mut().expect("the peers");
        peers.retain(|p| p["endpointId"] != ATTIC && p["endpointId"] != fixtures::BASEMENT);
        let i = peers.iter().position(|p| p["endpointId"] == fixtures::LIGHTHOUSE).expect("Lighthouse Tapes");
        let mut lighthouse = peers.remove(i);
        lighthouse["onShelf"] = true.into();
        peers.insert(0, lighthouse);
        land_poll(&mut gui, crate::api::types::P2pPeersAnswer::from_value(&moved));
        assert_ne!(before, at(&gui), "the poll moved mStream · 3f9a's tab");
        let buf = frame(&mut gui, 100, 30);
        assert_eq!(stop(&gui), Some(Some(fixtures::F3A.into())), "the stop stays on its tab");
        assert_eq!(gui.peer_recs.first_tab, Some(KITCHEN.into()), "and the strip on its first tab");
        let x = x_of(&buf, 4, "mStream");
        assert_eq!(buf[(x, 4)].bg, crate::kit::theme::th().accent, "the slab follows: {:?}", rows(&buf)[4]);
    }

    #[test]
    fn the_tabs_stop_selects_adds_and_retries() {
        let _en = english();
        let mut gui = room("peers_admin_add_states", "taste_all");
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.app.peer_recs_strip().selected.as_deref(), Some(fixtures::BASEMENT), "Enter selects");
        let asked = door(&gui.pending).into_iter().any(|cmd| matches!(cmd, PeerRecsCmd::Taste { .. }));
        assert!(asked, "the tab asks at once: {:?}", gui.pending);
        gui.pending.clear();
        // Kitchen, Attic, mStream · 3f9a; the two inert tabs are skipped; Rooftop FM tries again.
        for _ in 0..4 {
            key(&mut gui, KeyCode::Right);
        }
        assert_eq!(stop(&gui), Some(Some(fixtures::ROOFTOP.into())), "the stop skips no room and adding…");
        key(&mut gui, KeyCode::Enter);
        let retry = PeerRecsCmd::Add { endpoint_id: fixtures::ROOFTOP.into() };
        assert!(door(&gui.pending).contains(&retry), "try again posts the Add: {:?}", gui.pending);
        // A `+` tab Adds.
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        for _ in 0..5 {
            key(&mut gui, KeyCode::Right);
        }
        assert_eq!(stop(&gui), Some(Some(fixtures::LIGHTHOUSE.into())));
        key(&mut gui, KeyCode::Enter);
        let add = PeerRecsCmd::Add { endpoint_id: fixtures::LIGHTHOUSE.into() };
        assert!(door(&gui.pending).contains(&add), "`+` posts the Add: {:?}", gui.pending);
        // The tab turns inert under the stop: it stays there, and Enter does nothing.
        gui.pending.clear();
        frame(&mut gui, 100, 30);
        assert_eq!(stop(&gui), Some(Some(fixtures::LIGHTHOUSE.into())));
        key(&mut gui, KeyCode::Enter);
        assert!(door(&gui.pending).is_empty(), "adding… is inert: {:?}", gui.pending);
    }

    /// The audio commands among the pended effects.
    fn audio(gui: &Gui) -> Vec<AudioCmd> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Audio(cmd) => Some(cmd.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn space_n_x_are_the_clips_only_while_one_is_live() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        let paused = gui.demo_paused;
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.demo_paused, !paused, "with no clip, Space is main's play/pause");
        let mut gui = playing_room(AsidePhase::Playing);
        let paused = gui.demo_paused;
        key(&mut gui, KeyCode::Char(' '));
        assert_eq!(gui.demo_paused, paused, "main is untouched");
        let paused = audio(&gui).iter().any(|c| matches!(c, AudioCmd::AsidePause { .. }));
        assert!(paused, "Space pauses the clip: {:?}", gui.pending);
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('n'));
        let next = door(&gui.pending).into_iter().any(|cmd| matches!(cmd, PeerRecsCmd::Preview { .. }))
            || audio(&gui).iter().any(|c| matches!(c, AudioCmd::AsidePlay { .. }));
        assert!(next, "n plays the next preview: {:?}", gui.pending);
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('x'));
        let stop = AudioCmd::AsideStop { ticket: None, restore: true };
        assert!(audio(&gui).contains(&stop), "x ends the clip: {:?}", gui.pending);
        assert!(!gui.app.peer_recs_clip_live());
    }

    #[test]
    fn l_previews_the_held_row_and_capital_l_chains() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('l'));
        assert!(door(&gui.pending).is_empty(), "nothing held: `l` previews nothing");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('l'));
        let raw = gui.app.peer_recs_leads()[1].raw.clone();
        let asked = door(&gui.pending).into_iter().any(|cmd| match cmd {
            PeerRecsCmd::Preview { plugin, rec, .. } => plugin == "deezer" && *rec == raw,
            _ => false,
        });
        assert!(asked, "the held lead resolves, raw: {:?}", gui.pending);
        assert_eq!(held(&gui), Some(1), "the hand stays");
        // `L` runs Preview all from the top, and again stops it.
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('L'));
        let first = gui.app.peer_recs_leads()[0].raw.clone();
        let chained = door(&gui.pending).into_iter().any(|cmd| matches!(cmd, PeerRecsCmd::Preview { rec, .. } if *rec == first));
        assert!(chained, "L starts at the first lead: {:?}", gui.pending);
        assert!(gui.app.peer_recs_clip_live());
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('L'));
        assert!(!gui.app.peer_recs_clip_live(), "L again stops it");
    }

    #[test]
    fn y_copies_the_list_and_notes_copied() {
        use crate::kit::clipboard::{Copied, catch, caught};
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        caught();
        catch(Copied::Terminal);
        key(&mut gui, KeyCode::Char('y'));
        let copied = caught();
        assert_eq!(copied.len(), 1);
        let lines: Vec<&str> = copied[0].lines().collect();
        assert_eq!(lines.len(), 12, "the twelve leads, in rank order");
        assert_eq!((lines[0], lines[11]), ("Hollow Coast - Salt Road", "Ada Merrow - Field Notes"));
        assert!(!copied[0].ends_with('\n'), "no trailing newline");
        assert_eq!(gui.note, Some(("Copied".to_string(), false)), "OSC 52 notes Copied, dim");
        catch(Copied::Failed);
        key(&mut gui, KeyCode::Char('y'));
        assert_eq!(gui.note, Some(("Couldn't copy".to_string(), false)), "dim too");
        catch(Copied::Clipboard);
        caught();
    }

    #[test]
    fn w_and_e_end_a_live_clip() {
        let _en = english();
        let stop = AudioCmd::AsideStop { ticket: None, restore: true };
        let mut gui = playing_room(AsidePhase::Playing);
        key(&mut gui, KeyCode::Char('w'));
        assert!(gui.peer_recs.bar.menu.is_some(), "the window list opens");
        assert_eq!(audio(&gui), vec![stop.clone()], "and the clip ends, main restored");
        let mut gui = playing_room(AsidePhase::Playing);
        key(&mut gui, KeyCode::Char('e'));
        assert!(gui.peer_recs.bar.seeds.is_some(), "the seeds modal opens");
        assert_eq!(audio(&gui), vec![stop], "and the clip ends");
        // With no clip, nothing is stopped.
        let mut gui = admin_room();
        key(&mut gui, KeyCode::Char('w'));
        assert!(audio(&gui).is_empty());
    }

    #[test]
    fn r_asks_new_seeds() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('R'));
        assert_eq!(gui.active, super::super::PEER_RECS_NAV);
        let asked = door(&gui.pending).into_iter().filter(|cmd| matches!(cmd, PeerRecsCmd::Taste { .. })).count();
        assert_eq!(asked, 1, "one new batch: {:?}", gui.pending);
        assert!(gui.app.peer_recs_asking(), "the header reads asking…");
        assert_eq!(gui.app.peer_recs_leads().len(), 12, "the old rows stand until the new land");
    }

    /// `n` leads from the fixture `name`, titled `{word} 00`, `{word} 01`, … in rank order.
    fn long_taste(name: &str, word: &str, n: usize) -> crate::api::types::TasteResponse {
        taste_edited(name, |results| {
            let base = results.clone();
            *results = (0..n)
                .map(|i| {
                    let mut lead = base[i % base.len()].clone();
                    lead["title"] = format!("{word} {i:02}").into();
                    lead["exportId"] = format!("anon:{word} {i:02}").into();
                    lead
                })
                .collect();
        })
    }

    /// The last taste ask pended answered with `answer`.
    fn land_answer(gui: &mut Gui, answer: crate::api::types::TasteResponse) {
        let seq = door(&gui.pending).into_iter().rev().find_map(|cmd| match cmd {
            PeerRecsCmd::Taste { seq, .. } => Some(seq),
            _ => None,
        });
        gui.pending.clear();
        land(gui, PeerRecsEvent::Taste { seq: seq.expect("a taste ask is out"), answer: Ok(Box::new(answer)) });
        gui.pending.clear();
    }

    /// The grid's first row on screen at 100 × 24 (y 8), and its scroll.
    fn top_row(gui: &mut Gui) -> (usize, String) {
        let buf = frame(gui, 100, 24);
        (gui.peer_recs.list.scroll, rows(&buf)[8].clone())
    }

    /// The grid wheeled to its end: 32 leads at 100 × 24 are 16 rows, 11 in view.
    fn wheel_to_end(gui: &mut Gui) {
        gui.act(Act::ScrollBy(List::PeerRecs, 10));
        let (scroll, row) = top_row(gui);
        assert_eq!(scroll, 5, "wheeled to the end: {row:?}");
    }

    /// A new tab's list opens at its top, whichever way the tab was chosen, as Discover's
    /// `set_tab` does; new seeds on the same tab keep the visit's place (R3).
    #[test]
    fn a_new_tab_opens_its_grid_at_the_top() {
        let _en = english();
        let mut gui = peer_gui();
        gui.act(Act::Nav(super::super::PEER_RECS_NAV));
        answer_with(&mut gui, "peers_admin", &long_taste("taste_all", "Lead", 32), true);
        gui.pending.clear();
        top_row(&mut gui);
        wheel_to_end(&mut gui);
        assert!(top_row(&mut gui).1.contains("Lead 05"), "All from its row 5");
        // A click on Kitchen Radio's tab, its long answer landed.
        gui.act(Act::Peer(PeerAct::Tab(Some(KITCHEN.into()))));
        land_answer(&mut gui, long_taste("taste_kitchen", "Kitchen", 32));
        let (scroll, row) = top_row(&mut gui);
        assert_eq!(scroll, 0, "a clicked tab opens at its top: {row:?}");
        assert!(row.contains("Kitchen 00"), "{row:?}");
        // The tabs stop's Enter on All, whose answer is kept: drawn at once, from its top.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert_eq!(stop(&gui), Some(Some(KITCHEN.into())));
        wheel_to_end(&mut gui);
        key(&mut gui, KeyCode::Left);
        key(&mut gui, KeyCode::Left);
        assert_eq!(stop(&gui), Some(None));
        key(&mut gui, KeyCode::Enter);
        let (scroll, row) = top_row(&mut gui);
        assert_eq!(scroll, 0, "the stop's Enter opens All at its top: {row:?}");
        assert!(row.contains("Lead 00"), "{row:?}");
        // Stowed →: Basement Hi-Fi, its ask 250 ms on.
        key(&mut gui, KeyCode::Esc);
        wheel_to_end(&mut gui);
        key(&mut gui, KeyCode::Right);
        assert_eq!(gui.app.peer_recs_strip().selected.as_deref(), Some(fixtures::BASEMENT));
        assert_eq!(top_row(&mut gui).0, 0, "a stepped tab opens at its top");
        // R on All: new seeds, the visit and its place kept.
        key(&mut gui, KeyCode::Left);
        assert_eq!(top_row(&mut gui).0, 0, "All again, at its top");
        wheel_to_end(&mut gui);
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('R'));
        land_answer(&mut gui, long_taste("taste_all", "Lead", 32));
        assert_eq!(top_row(&mut gui).0, 5, "new seeds keep the grid's place");
    }

    // ── The watch, the pick and the bar ─────────────────────────────────────

    /// Whether the App's tick asks the peers `after` seconds on; a poll it sends is answered, so
    /// the cadence goes on.
    fn polls_at(gui: &mut Gui, after: u64) -> bool {
        let now = crate::clock::Instant::now();
        let mut effects = gui.app.tick_at(now);
        effects.extend(gui.app.tick_at(now + std::time::Duration::from_secs(after)));
        let poll = door(&effects).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Peers { poll } => Some(poll),
            _ => None,
        });
        if let Some(poll) = poll {
            land(gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(fixtures::peers("peers_admin"))) });
            gui.pending.clear();
        }
        poll.is_some()
    }

    #[test]
    fn a_screen_switch_covers_the_room_and_the_return_polls_once() {
        use crate::tui::app::peer_recs::PeerRecsWatch;
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Shown);
        gui.act(Act::Screen(Screen::Stats));
        frame(&mut gui, 100, 30);
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Covered, "another screen covers the room");
        assert!(!polls_at(&mut gui, 40), "covered polls nothing");
        assert_eq!(gui.app.peer_recs_leads().len(), 12, "the visit stays");
        gui.act(Act::Screen(Screen::Library));
        frame(&mut gui, 100, 30);
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Shown);
        let effects = gui.app.tick_at(crate::clock::Instant::now());
        let polls = door(&effects).into_iter().filter(|cmd| matches!(cmd, PeerRecsCmd::Peers { .. })).count();
        assert_eq!(polls, 1, "one poll on return: {effects:?}");
    }

    #[test]
    fn a_pick_armed_from_the_room_pauses_the_poll_keeps_the_batch_and_lands_as_r3() {
        use crate::tui::app::peer_recs::PeerRecsWatch;
        let _en = english();
        let mut gui = admin_room();
        let picked = crate::api::types::Track {
            filepath: "music/picked.mp3".into(),
            metadata: crate::api::types::TrackMetadata { title: Some("Picked".into()), ..Default::default() },
        };
        gui.app.files.set(vec![crate::tui::app::Entry::Track { label: "Picked".into(), track: Box::new(picked) }]);
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('e'));
        super::taste_bar::choose::<super::Room>(&mut gui);
        assert_eq!(gui.active, super::super::FILES_NAV, "the pick happens in Files");
        assert!(super::picking(&gui.app));
        let all = rows(&frame(&mut gui, 100, 30)).join("\n");
        assert!(all.contains("Pick a song to start from"), "the banner:\n{all}");
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Covered, "covered, not left");
        assert!(!polls_at(&mut gui, 40), "the poll pauses");
        assert_eq!(gui.app.peer_recs_leads().len(), 12, "the batch stays");
        gui.pending.clear();
        gui.act(Act::PaneRow(List::Files, 0, RowVerb::Open));
        assert_eq!(gui.app.capture, None, "the pick landed");
        assert_eq!(gui.active, super::super::PEER_RECS_NAV, "home to the room");
        let pinned = door(&gui.pending).into_iter().any(|cmd| match cmd {
            PeerRecsCmd::Taste { body, .. } => serde_json::to_value(&*body).expect("a body")["pin"]
                .as_array()
                .is_some_and(|pins| pins.iter().any(|p| p == "music/picked.mp3")),
            _ => false,
        });
        assert!(pinned, "R3 with the pick pinned: {:?}", gui.pending);
        assert!(gui.app.taste.pins.is_empty(), "Discover's pins are untouched");
        frame(&mut gui, 100, 30);
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Shown);
    }

    #[test]
    fn a_refused_pick_is_worded_in_the_room() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('e'));
        super::taste_bar::choose::<super::Room>(&mut gui);
        // The room's App half kept a refusal of the room's pick (build plan D32).
        gui.app.peer_recs.taste.refused = Some(crate::tui::app::TastePinRefusal::Full);
        super::settle(&mut gui);
        let words = rust_i18n::t!("gui.discover.pins_full").to_string();
        assert_eq!(gui.note, Some((words, true)), "Discover's words for it");
        assert_eq!(gui.app.peer_recs.taste.refused, None, "worded once");
        gui.note = None;
        super::settle(&mut gui);
        assert_eq!(gui.note, None, "and nothing more is said");
        assert_eq!(gui.app.taste.refused, None);
    }

    #[test]
    fn the_seeds_modal_draws_the_batchs_seeds_not_the_tabs() {
        let _en = english();
        let mut gui = admin_room();
        gui.act(Act::Peer(PeerAct::Tab(Some(KITCHEN.into()))));
        let mut kitchen = fixtures::json("taste_kitchen");
        kitchen["seeds"] = serde_json::json!([
            { "hash": "k1", "filepath": "music/kitchen-only.mp3", "metadata": { "title": "Kitchen Only", "artist": "Nobody" } }
        ]);
        let seq = door(&gui.pending).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Taste { seq, .. } => Some(seq),
            _ => None,
        });
        gui.pending.clear();
        let answer = Ok(Box::new(serde_json::from_value(kitchen).expect("an answer")));
        land(&mut gui, PeerRecsEvent::Taste { seq: seq.expect("R2 is out"), answer });
        assert_eq!(gui.app.peer_recs_leads().len(), 5, "Kitchen Radio's leads show");
        key(&mut gui, KeyCode::Char('e'));
        let all = rows(&frame(&mut gui, 100, 30)).join("\n");
        assert!(all.contains("Highway") && all.contains("Neon"), "the batch's seeds:\n{all}");
        assert!(!all.contains("Kitchen Only"), "never the tab's:\n{all}");
    }

    // ── The mouse ───────────────────────────────────────────────────────────

    /// Where `text` starts on row `y` of `buf`, in cells.
    fn x_of(buf: &Buffer, y: u16, text: &str) -> u16 {
        let row = &rows(buf)[usize::from(y)];
        let i = row.find(text).unwrap_or_else(|| panic!("{text:?} not on row {y}: {row:?}"));
        crate::kit::width(&row[..i]) as u16
    }

    fn hit(gui: &Gui, x: u16, y: u16) -> Option<Act> {
        gui.ui.hit(ratatui::layout::Position { x, y })
    }

    #[test]
    fn clicks_reach_tabs_rows_slots_verbs_band_and_panes() {
        let _en = english();
        for (height, tabs_y, verbs_y, grid_y) in [(30, 4, 6, 9), (24, 4, 5, 8)] {
            let mut gui = admin_room();
            let buf = frame(&mut gui, 100, height);
            let at = |text| x_of(&buf, tabs_y, text);
            assert_eq!(hit(&gui, at("All"), tabs_y), Some(Act::Peer(PeerAct::Tab(None))), "{height}");
            let kitchen = Some(Act::Peer(PeerAct::Tab(Some(KITCHEN.into()))));
            assert_eq!(hit(&gui, at("Kitchen"), tabs_y), kitchen, "{height}");
            assert_eq!(hit(&gui, 96, tabs_y), Some(Act::Peer(PeerAct::TabsScroll(1))), "{height}: ›");
            assert_eq!(hit(&gui, 17, verbs_y), Some(Act::Peer(PeerAct::PreviewAll)), "{height}");
            let copy = x_of(&buf, verbs_y, "copy list");
            assert_eq!(hit(&gui, copy, verbs_y), Some(Act::Peer(PeerAct::CopyList)), "{height}");
            assert_eq!(hit(&gui, 20, grid_y), Some(Act::Peer(PeerAct::Row(0))), "{height}: a cell");
            assert_eq!(hit(&gui, 53, grid_y), Some(Act::Peer(PeerAct::Slot(0))), "{height}: its slot");
            assert_eq!(hit(&gui, 60, grid_y), Some(Act::Peer(PeerAct::Row(6))), "{height}: column B");
            assert_eq!(hit(&gui, 93, grid_y + 1), Some(Act::Peer(PeerAct::Slot(7))), "{height}");
        }
        // The band.
        let mut gui = playing_room(AsidePhase::Playing);
        let buf = frame(&mut gui, 100, 30);
        assert_eq!(hit(&gui, 17, 6), Some(Act::Peer(PeerAct::Toggle)));
        assert_eq!(hit(&gui, 20, 6), Some(Act::Peer(PeerAct::Next)));
        assert_eq!(hit(&gui, x_of(&buf, 6, "[X]"), 6), Some(Act::Peer(PeerAct::Close)));
        // The panes.
        let mut gui = room("peers_admin", "taste_attic");
        let buf = frame(&mut gui, 100, 30);
        assert_eq!(hit(&gui, x_of(&buf, 9, "new seeds"), 9), Some(Act::Peer(PeerAct::NewSeeds)));
        let mut gui = room("peers_admin", "taste_scan_pending");
        let buf = frame(&mut gui, 100, 30);
        assert_eq!(hit(&gui, x_of(&buf, 11, "Check again"), 11), Some(Act::Peer(PeerAct::CheckAgain)));
        let mut gui = room("peers_admin", "taste_error");
        let buf = frame(&mut gui, 100, 30);
        assert_eq!(hit(&gui, x_of(&buf, 9, "Try again"), 9), Some(Act::Peer(PeerAct::TryAgain)));
    }

    #[test]
    fn a_click_stows_the_cursor_and_the_stop() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Down);
        assert_eq!(held(&gui), Some(0));
        gui.act(Act::Peer(PeerAct::Slot(3)));
        assert_eq!(held(&gui), None, "a slot click stows the hand");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Up);
        assert!(stop(&gui).is_some());
        gui.act(Act::Peer(PeerAct::Tab(Some(KITCHEN.into()))));
        assert_eq!(stop(&gui), None, "a tab click stows the stop");
    }

    // ── Layout, tips, locales ───────────────────────────────────────────────

    #[test]
    fn a_resize_or_queue_toggle_holds_the_cursor_on_its_lead() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        // Paper Moons: column B's third row at 100 × 30.
        for code in [KeyCode::Down, KeyCode::Down, KeyCode::Down, KeyCode::Right] {
            key(&mut gui, code);
        }
        assert_eq!(held(&gui), Some(8));
        for (height, queue) in [(24, false), (30, true), (30, false)] {
            gui.queue_open = queue;
            let buf = frame(&mut gui, 100, height);
            assert_eq!(held(&gui), Some(8), "100×{height} queue {queue}");
            let y = rows(&buf).iter().position(|r| r.contains("Paper Moons")).expect("Paper Moons") as u16;
            let x = x_of(&buf, y, "Paper Moons");
            assert_eq!(buf[(x, y)].bg, crate::kit::theme::th().accent, "the slab stays on Paper Moons");
        }
    }

    #[test]
    fn no_plugins_at_all_enter_copies() {
        let _en = english();
        let mut gui = admin_room();
        gui.app.capabilities.discovery_plugins = false;
        frame(&mut gui, 100, 30);
        crate::kit::clipboard::caught();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(crate::kit::clipboard::caught(), vec!["Hollow Coast - Salt Road".to_string()]);
        assert_eq!(gui.note, Some(("Copied".to_string(), false)));
        assert_eq!(super::super::actions::lead_open(&gui), None, "no sheet opens");
    }

    #[test]
    fn tips_follow_the_rooms_state() {
        let _en = english();
        let tip = |key: &str| rust_i18n::t!(key).to_string();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs"), "stowed, with rows");
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_row"), "a row held");
        key(&mut gui, KeyCode::Up);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_tabs"), "the tabs stop");
        key(&mut gui, KeyCode::Char('w'));
        assert_eq!(super::tips(&gui), tip("gui.tips.discover_menu"), "the window list");
        // A clip: its line while nothing is held; a held row's wins over it.
        let mut gui = playing_room(AsidePhase::Playing);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_clip"));
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_row"));
        // No preview offered: the held row's line names no `l`.
        let mut gui = admin_room();
        gui.app.audio_available = false;
        key(&mut gui, KeyCode::Down);
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_row_plain"));
        // No rows.
        let gui = room("peers_admin", "taste_attic");
        assert_eq!(super::tips(&gui), tip("gui.tips.peer_recs_empty"));
    }

    /// At the queue's 47 cells the header's controls stand whole, apart, inside the room, in
    /// every locale and on the legacy console; at 80 the name and "more options" stand whole.
    /// Each language is read through `locale =`, so no process-wide switch is needed.
    #[test]
    fn the_header_steps_hold_at_47_cells_in_every_locale() {
        use super::taste_bar::{HeadWords, head_plan};
        let gui = admin_room();
        let narrow = super::super::content_rect(100, 30, true, false);
        let wide = super::super::content_rect(100, 30, false, false);
        assert_eq!((narrow.width, wide.width), (47, 80));
        for legacy in [false, true] {
            let _tier = legacy.then(crate::kit::theme::legacy_glyphs_here);
            let glyph = super::super::discover::refresh_glyph();
            for content in [narrow, wide] {
                for code in ["de", "en", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"] {
                    let words = HeadWords::of(&super::facts_in(&gui.app, code), code);
                    let plan = head_plan(&words, content, glyph);
                    let what = format!("{code}/legacy {legacy} at {}: {plan:?}", content.width);
                    let (wx, ww) = plan.window;
                    let (mx, mw) = plan.more;
                    let (rx, rw) = plan.refresh;
                    let name = words.name_at(plan.step);
                    let name_w = if plan.step.name { crate::kit::width(name) as u16 + 2 } else { 0 };
                    assert!(content.x + name_w <= wx, "{what}");
                    assert!(plan.step.more && wx + ww + 2 <= mx && mx + mw + 2 <= rx, "{what}");
                    assert!(rx + rw <= content.right(), "{what}");
                    if content == wide {
                        assert!(plan.step.name && plan.step.long, "{what}");
                    }
                }
            }
        }
        // English at 47 cells: the short name, "more options" whole (REG-HEADER's 47-cell slice).
        let words = HeadWords::of(&super::facts_in(&gui.app, "en"), "en");
        let plan = head_plan(&words, narrow, Some("↻"));
        assert!(plan.step.short && plan.step.long && plan.step.name, "{plan:?}");
    }

    // ── The sheet beside the room ───────────────────────────────────────────

    /// The sheet's lookup answered with no jobs, as a lead nobody got yet.
    fn looked_up(gui: &mut Gui) {
        for cmd in door(&std::mem::take(&mut gui.pending)) {
            if let PeerRecsCmd::Lookup { seq, .. } = cmd {
                land(gui, PeerRecsEvent::Lookup { seq, answer: Ok(Box::default()) });
            }
        }
        gui.pending.clear();
    }

    /// The keys walk the hand to lead `at` of All's twelve (6 | 6 at 100 × 30).
    fn hold(gui: &mut Gui, at: usize) {
        key(gui, KeyCode::Down);
        let (row, col) = (at % 6, at / 6);
        for _ in 0..row {
            key(gui, KeyCode::Down);
        }
        if col == 1 {
            key(gui, KeyCode::Right);
        }
        assert_eq!(held(gui), Some(at));
    }

    /// Get it pressed in the sheet, its start answered with a running job.
    fn get_it_running(gui: &mut Gui) {
        key(gui, KeyCode::Enter);
        let seq = door(&gui.pending).into_iter().find_map(|cmd| match cmd {
            PeerRecsCmd::Start { seq, .. } => Some(seq),
            _ => None,
        });
        let seq = seq.unwrap_or_else(|| panic!("Get it starts a job: {:?}", gui.pending));
        gui.pending.clear();
        let job: crate::api::types::JobStarted = serde_json::from_value(serde_json::json!({
            "job": { "id": 9, "plugin": "noop-acquire", "state": "running" },
            "created": true
        }))
        .expect("a started job");
        land(gui, PeerRecsEvent::Started { seq, answer: Ok(Box::new(job)) });
        gui.pending.clear();
    }

    /// LEAD SHEET's room: Enter on Long Exposure, held in column B's first row.
    fn sheet_on_long_exposure() -> Gui {
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        hold(&mut gui, 6);
        key(&mut gui, KeyCode::Enter);
        looked_up(&mut gui);
        gui
    }

    /// LEAD SHEET (M3), the whole frame: the sheet over the grid, Long Exposure's held cell
    /// beside it at x 83–94, the strip and the verbs line above it.
    #[test]
    fn m3_full_frame() {
        let _en = english();
        let mut gui = sheet_on_long_exposure();
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "M3", "", false);
    }

    /// GET IT · DOWNLOADING (L1), the whole frame: Get it pressed, then ↓ to Open elsewhere….
    #[test]
    fn l1_full_frame() {
        let _en = english();
        let mut gui = sheet_on_long_exposure();
        get_it_running(&mut gui);
        key(&mut gui, KeyCode::Down);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "L1", "", false);
    }

    /// OPEN ELSEWHERE (L2), the whole frame: the links list hangs from its row.
    #[test]
    fn l2_full_frame() {
        let _en = english();
        let mut gui = sheet_on_long_exposure();
        get_it_running(&mut gui);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        let link = |label: &str, kind: &str| crate::api::types::PluginLink {
            id: label.to_lowercase().replace(' ', "-"),
            label: label.into(),
            url: format!("https://example.org/{}", label.to_lowercase().replace(' ', "-")),
            kind: kind.into(),
        };
        let links = vec![
            link("MusicBrainz ISRC", "canonical"),
            link("Search MusicBrainz", "search"),
            link("Search Deezer", "search"),
            link("Search Apple Music", "search"),
            link("Search YouTube", "search"),
            link("Search Bandcamp", "search"),
            link("Search Discogs", "search"),
        ];
        let (seq, generation) = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::PluginLinks { seq, generation, .. }) => Some((*seq, *generation)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Open elsewhere asks the links plug-in: {:?}", gui.pending));
        gui.pending.clear();
        let effects = gui.app.apply_event(crate::tui::worker::Event::PluginLinks { seq, generation, answer: Ok(links) });
        gui.pend(effects);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "L2", "", false);
    }

    /// OFFLINE · ASKING (L3), the whole frame: Paper Moons' slot clicked (its resolve still out),
    /// then its row: the band and the slot read `◌`, and no slab covers the slot.
    #[test]
    fn l3_full_frame() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        gui.act(Act::Peer(PeerAct::Slot(8)));
        assert!(door(&gui.pending).iter().any(|c| matches!(c, PeerRecsCmd::Preview { .. })), "the press resolves");
        gui.pending.clear();
        gui.act(Act::Peer(PeerAct::Row(8)));
        looked_up(&mut gui);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "L3", "", false);
    }

    /// NO PREVIEW (L4), the whole frame: Arcade Rain answered no preview, then held and opened.
    #[test]
    fn l4_full_frame() {
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        press(&mut gui, 7, false);
        hold(&mut gui, 7);
        key(&mut gui, KeyCode::Enter);
        looked_up(&mut gui);
        let buf = frame(&mut gui, 100, 30);
        assert_board(&buf, "L4", "", false);
    }

    /// Whether anything of `effects` plays: main, or a clip, or a clip's resolve.
    fn plays(effects: &[Effect]) -> bool {
        effects.iter().any(|e| {
            matches!(
                e,
                Effect::Audio(AudioCmd::Play { .. } | AudioCmd::AsidePlay { .. } | AudioCmd::Resume)
                    | Effect::Api(ApiCmd::PeerRecs { cmd: PeerRecsCmd::Preview { .. }, .. })
            )
        })
    }

    #[test]
    fn enter_and_m_open_the_sheet_and_never_play() {
        let _en = english();
        let open = |gui: &Gui| super::super::actions::lead_open(gui);
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        // Stowed, both do nothing.
        key(&mut gui, KeyCode::Enter);
        key(&mut gui, KeyCode::Char('m'));
        assert_eq!(open(&gui), None, "stowed: no sheet");
        hold(&mut gui, 2);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(open(&gui), Some(2), "Enter opens the held lead's sheet");
        assert!(!plays(&gui.pending), "and plays nothing: {:?}", gui.pending);
        key(&mut gui, KeyCode::Esc);
        assert_eq!(open(&gui), None);
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('m'));
        assert_eq!(open(&gui), Some(2), "so does m");
        assert!(!plays(&gui.pending), "{:?}", gui.pending);
        assert!(gui.app.queue.items.is_empty());
    }

    #[test]
    fn esc_closes_then_stows_then_ends_the_clip() {
        let _en = english();
        let mut gui = playing_room(AsidePhase::Playing);
        frame(&mut gui, 100, 30);
        hold(&mut gui, 0);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(super::super::actions::lead_open(&gui), Some(0));
        assert!(gui.app.peer_recs_clip_live(), "the sheet keeps the clip");
        gui.pending.clear();
        key(&mut gui, KeyCode::Esc);
        assert_eq!(super::super::actions::lead_open(&gui), None, "Esc closes the sheet first");
        assert_eq!(held(&gui), Some(0), "the hand stays");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(held(&gui), None, "then stows");
        assert!(gui.app.peer_recs_clip_live());
        assert!(audio(&gui).is_empty(), "{:?}", gui.pending);
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.app.peer_recs_clip_live(), "then ends the clip");
        assert_eq!(audio(&gui), vec![AudioCmd::AsideStop { ticket: None, restore: true }]);
    }

    #[test]
    fn the_cursor_comes_back_to_its_row_when_the_sheet_closes() {
        let _en = english();
        let mut gui = sheet_on_long_exposure();
        frame(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Esc);
        assert_eq!(held(&gui), Some(6), "back on its index");
        // The sheet follows its lead when the list moves, and the hand with it: the Kitchen
        // Radio tab, whose list puts Long Exposure third.
        key(&mut gui, KeyCode::Enter);
        looked_up(&mut gui);
        let effects = gui.app.peer_recs_select(Some(KITCHEN.into()));
        gui.pend(effects);
        land_taste(&mut gui, "taste_kitchen");
        frame(&mut gui, 100, 30);
        assert_eq!(super::super::actions::lead_open(&gui), Some(2), "the sheet followed Long Exposure");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(held(&gui), Some(2), "on the lead's new index");
    }

    #[test]
    fn a_right_click_on_a_row_opens_the_sheet() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let _en = english();
        let mut gui = admin_room();
        frame(&mut gui, 100, 30);
        let at = ratatui::layout::Position { x: 60, y: 10 };
        assert_eq!(gui.ui.hit_context(at), Some(Act::Peer(PeerAct::Row(7))), "the cell's context act");
        mouse(&mut gui, MouseEventKind::Down(MouseButton::Right), at);
        mouse(&mut gui, MouseEventKind::Up(MouseButton::Right), at);
        assert_eq!(super::super::actions::lead_open(&gui), Some(7), "Arcade Rain's sheet");
        assert!(!plays(&gui.pending), "{:?}", gui.pending);
    }

    fn mouse(gui: &mut Gui, kind: ratatui::crossterm::event::MouseEventKind, at: ratatui::layout::Position) {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent};
        let event = MouseEvent { kind, column: at.x, row: at.y, modifiers: KeyModifiers::NONE };
        let mut ctx = super::super::tests::quiet_ctx(gui);
        super::super::input(gui, &mut ctx, ratatui::crossterm::event::Event::Mouse(event));
    }

    /// Apple Terminal's click: the pointer arrives, the press comes as an instant pair, and the
    /// release sends the same pair again where the hand let go (the kit's `EchoFilter`).
    #[test]
    fn the_apple_echo_presses_nothing_in_the_sheet() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let _en = english();
        let mut gui = admin_room();
        gui.ui.set_echo_dialect(true);
        let at = ratatui::layout::Position { x: 60, y: 10 };
        let (down, up) = (MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left));
        mouse(&mut gui, MouseEventKind::Moved, at);
        frame(&mut gui, 100, 30);
        mouse(&mut gui, down, at);
        mouse(&mut gui, up, at);
        frame(&mut gui, 100, 30);
        assert_eq!(super::super::actions::lead_open(&gui), Some(7), "the click opened the sheet");
        looked_up(&mut gui);
        let row = gui.actions.sheet.as_ref().map(|s| s.row);
        mouse(&mut gui, down, at);
        mouse(&mut gui, up, at);
        frame(&mut gui, 100, 30);
        assert_eq!(super::super::actions::lead_open(&gui), Some(7), "the echo pressed nothing: the sheet stands");
        assert_eq!(gui.actions.sheet.as_ref().map(|s| s.row), row, "on the row it opened on");
        assert!(!plays(&gui.pending), "nothing played: {:?}", gui.pending);
        let started = door(&gui.pending).into_iter().any(|cmd| matches!(cmd, PeerRecsCmd::Start { .. }));
        assert!(!started, "no job started: {:?}", gui.pending);
    }

    #[test]
    fn the_sheet_and_the_bar_modals_are_not_a_cover() {
        use crate::tui::app::peer_recs::PeerRecsWatch;
        let _en = english();
        let mut gui = sheet_on_long_exposure();
        frame(&mut gui, 100, 30);
        assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Shown, "under the sheet");
        assert!(polls_at(&mut gui, 16), "the poll goes on under the sheet");
        key(&mut gui, KeyCode::Esc);
        for open in ['w', 'e'] {
            key(&mut gui, KeyCode::Char(open));
            frame(&mut gui, 100, 30);
            assert_eq!(gui.app.peer_recs.watch, PeerRecsWatch::Shown, "under `{open}`");
            assert!(polls_at(&mut gui, 16), "the poll goes on under `{open}`");
            key(&mut gui, KeyCode::Esc);
        }
    }

    /// Every board state the tests hold, printed for the eye: `cargo test dump_peer_recs_frames
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_peer_recs_frames() {
        let _en = english();
        let mut first = peer_gui();
        first.act(Act::Nav(super::super::PEER_RECS_NAV));
        let states: Vec<(&str, Gui, u16)> = vec![
            ("TOOLS · FIRST ASK", first, 30),
            ("ALL PEERS · IDLE", admin_room(), 30),
            ("PREVIEW PLAYING", playing_room(AsidePhase::Playing), 30),
            ("PREVIEW PAUSED", playing_room(AsidePhase::Paused), 30),
            ("LEAD SHEET", sheet_on_long_exposure(), 30),
            ("FLOOR · HINTS OFF", admin_room(), 24),
            ("NO PEERS YET", room("peers_admin_empty_shelf", "taste_empty_shelf"), 30),
            ("NOTHING NEW", room("peers_admin", "taste_attic"), 30),
            ("SCAN PENDING", room("peers_admin", "taste_scan_pending"), 30),
            ("ADD STATES", room("peers_admin_add_states", "taste_all"), 30),
            ("MEMBER ACCOUNT", room("peers_member", "taste_all"), 30),
        ];
        for (caption, mut gui, height) in states {
            println!("── {caption} ──");
            for (y, row) in rows(&frame(&mut gui, 100, height)).iter().enumerate() {
                println!("{y:2}  {row}");
            }
        }
    }
}
