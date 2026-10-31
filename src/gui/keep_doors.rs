//! Keep a copy's ways in from the browse rooms, the GUI's half (the
//! design-term-fed spec §4–§5, §6.9, §12.3; the user's 2026-10-27 calls):
//! the shell's hosting of the Keep a copy window, the Copies list and the
//! picker over every room — drawn, keyed, the room inert beneath, the poll
//! watched from wherever they stand; the rooms' one `c` (§4.9) and `C`; the
//! header line's word "keep a copy" in the accent and the copy state in its place,
//! with the line's give-way (§4.1.4); the rows', cards' and folder rows'
//! marks, asked of the App's per-page holder (§5.0); the tips line's keep
//! segment. The window's insides are `keep`'s; every fact is the App's
//! (`tui::app::keep`).

use std::hash::{Hash, Hasher};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use rust_i18n::t;

use crate::api::types::{Album, JobMark, Track};
use crate::kit::theme::legacy_conhost;
use crate::kit::width;
use crate::tui::app::keep::{KeepTarget, KeepWatch, MarkAsk, MarksPage};
use crate::tui::app::{App, Entry};
use crate::tui::worker::LibraryNode;

use super::{Act, DISCOVER_NAV, FILES_NAV, Gui, SEARCH_NAV, Screen, accent, albums, bright_bold, keep, put};

// ── State ───────────────────────────────────────────────────────────────────

/// What the shell keeps of the rooms' marks: the rows the last ask carried,
/// as a hash — a page's rows are copied out of the App and handed to its
/// holder only when they change (a new page, a block scrolled into view, a
/// page turn), never every frame — and whether this frame's room asked.
#[derive(Debug, Default)]
pub(crate) struct DoorsUi {
    seen: Option<u64>,
    asked: bool,
    /// The page whose word stood this visit ([`page_id`]): it stays for the
    /// visit, live, whatever the gate says meanwhile (spec §3.2) — until a
    /// frame draws no page's spot (another room, another screen) or another
    /// page's.
    latched: Option<u64>,
    spotted: bool,
}

// ── The window, hosted by the shell (spec §12.3) ────────────────────────────

/// Keep a copy or the Copies list is up — over whichever room.
pub(super) fn window_open(gui: &Gui) -> bool {
    gui.app.keep.flow.is_some() || gui.discover.copies.is_some()
}

/// Keep a copy (or Where copies land, or the Into a folder chooser in its
/// place) and the Copies list, drawn over the room on screen: centred in the
/// room's content and clearing its width on their rows (the band), the
/// whole frame inert beneath.
pub(super) fn draw_window(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if gui.screen != Screen::Library {
        return;
    }
    let room = super::content_rect(area.width, area.height, gui.queue_open, gui.footer());
    if gui.app.keep.flow.is_some() {
        keep::draw(frame, gui, area, room);
    }
    if gui.discover.copies.is_some() {
        keep::draw_copies(frame, gui, area, room);
    }
}

/// The window's keys while it is up, in any room: it has them all.
pub(super) fn window_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if gui.screen != Screen::Library {
        return None;
    }
    if gui.app.keep.flow.is_some() {
        keep::handle_key(gui, key);
        return Some(false);
    }
    if gui.discover.copies.is_some() {
        keep::copies_key(gui, key);
        return Some(false);
    }
    None
}

/// The wheel over the window: Where copies land's folders, the Copies
/// list's rows; Keep a copy holds its own still.
pub(super) fn wheel(gui: &mut Gui, delta: i32) {
    if gui.app.keep.flow.is_some() {
        keep::dest_wheel(gui, delta);
    } else {
        keep::copies_wheel(gui, delta);
    }
}

/// The tips line while the window is up.
pub(super) fn tips(gui: &Gui) -> String {
    if gui.app.keep.flow.is_some() { keep::tips(gui) } else { keep::copies_tips(gui) }
}

/// The poll's beat from what shows, whichever room (clause 112; the user's
/// 2026-10-27 call on Copies): the quick beat while the Copies list is up
/// or a window's own copy moves; the room's while a window stands, the
/// Discover room shows, or a connected server's page shows a mark or a
/// state still moving (spec §5.0); none otherwise. Set after
/// `keep::frame_tick`, which knows the Discover room alone. A frame whose
/// room asked no page's marks before it drops the page's holder: its
/// marks, and the poll they kept, go with the page.
pub(super) fn watch(gui: &mut Gui) {
    if !std::mem::take(&mut gui.doors.asked) {
        gui.app.keep_marks_drop();
        gui.doors.seen = None;
    }
    if !std::mem::take(&mut gui.doors.spotted) {
        gui.doors.latched = None;
    }
    let library = gui.screen == Screen::Library;
    let modal = library && (gui.discover.copies.is_some() || gui.app.keep_flow_live());
    let marked = gui.app.session.peer.is_some() && (gui.browse_room() || gui.active == SEARCH_NAV) && gui.app.keep_marks_live();
    let room = library && (gui.active == DISCOVER_NAV || gui.app.keep.flow.is_some() || marked);
    let watch = match (modal, room) {
        (true, _) => KeepWatch::Modal,
        (false, true) => KeepWatch::Room,
        (false, false) => KeepWatch::Away,
    };
    gui.app.keep_watch(watch);
}

// ── `c` and `C` in the browse rooms (spec §4.9) ─────────────────────────────

/// The rooms this door serves: every browse room and Search — never the
/// Discover room, whose own `c` and `C` stand.
fn served(gui: &Gui) -> bool {
    gui.screen == Screen::Library
        && gui.app.connected
        && gui.active != DISCOVER_NAV
        && (gui.browse_room() || gui.active == SEARCH_NAV)
}

/// A Library room whose drill this door reads: the Albums room's, an
/// artist's, a genre's, Recent's.
fn library_room(gui: &Gui) -> bool {
    gui.active == super::ALBUMS_NAV || super::library::root_of(gui.active).is_some()
}

/// What the keyboard holds in the room on screen: a wall's card, or a row
/// of its list — by its pane and index. `None` while the cursor is stowed,
/// or holds `..` (the page's own subject then, §4.5). The hovered row or
/// card never counts (rule 10).
enum Held {
    Card,
    Row(crate::tui::app::Tab, usize),
}

fn held(gui: &Gui) -> Option<Held> {
    if !served(gui) {
        return None;
    }
    if albums::wall_up(gui) {
        return albums::wall_ref(gui).held.then_some(Held::Card);
    }
    let list = gui.room_list()?;
    if !gui.list_view(list).held {
        return None;
    }
    let tab = list.tab()?;
    let pane = gui.app.pane_for(tab);
    let index = pane.state.selected()?;
    match pane.entries.get(index)? {
        Entry::Parent => None,
        _ => Some(Held::Row(tab, index)),
    }
}

/// The held thing's target — `None` where it has no way in (an artist row,
/// a library row at `/`, a card whose credit is not known, your own song).
fn held_target(gui: &mut Gui, held: Held) -> Option<KeepTarget> {
    match held {
        Held::Card => {
            let card = albums::held_card(gui)?;
            gui.app.keep_album_card(&card)
        }
        Held::Row(tab, index) => {
            let origin = gui.app.origin();
            match gui.app.pane_for(tab).entries.get(index)? {
                Entry::Track { track, .. } => gui.app.keep_track(&origin, track, Some(tab)),
                // Folder scope comes from the Files room alone (rule 2).
                Entry::Dir { path, .. } if gui.active == FILES_NAV => gui.app.keep_folder(path),
                _ => None,
            }
        }
    }
}

/// The page's own subject, which the header word keeps (W1, W4, W6): the
/// album page, the artist's wall, a Files folder below a library's top.
fn page_target(gui: &Gui) -> Option<KeepTarget> {
    if !served(gui) {
        return None;
    }
    if gui.active == FILES_NAV {
        return gui.app.keep_folder(&gui.app.path);
    }
    if !library_room(gui) {
        return None;
    }
    match gui.app.library_stack.here() {
        LibraryNode::Album { .. } => gui.app.keep_album_page(),
        LibraryNode::Artist(_) => gui.app.keep_artist_wall(),
        _ => None,
    }
}

/// A target that opens a window: its own server's gate open and a choice
/// to offer.
fn opens(gui: &Gui, target: Option<KeepTarget>) -> Option<KeepTarget> {
    target.filter(|t| !gui.app.keep_choices(t.clone()).is_empty())
}

/// The browse rooms' `c` and `C`: Keep a copy on what §4.9 names — the
/// held row or card, else the page's subject; the Copies list while the
/// gate of the own server in front is open (the user's 2026-10-27
/// decision). Where `c` keeps nothing it is inert — no note, no sound.
/// `None` passes the key on.
pub(super) fn room_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !served(gui) || key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char('c') => {
            match held(gui) {
                Some(held) => {
                    let target = held_target(gui, held);
                    if let Some(target) = opens(gui, target) {
                        keep::open(gui, target);
                    }
                }
                None => open_page(gui),
            }
            Some(false)
        }
        KeyCode::Char('C') if gui.app.keep_copies_ready() => {
            keep::open_copies(gui);
            Some(false)
        }
        _ => None,
    }
}

/// The header word, or the state in its place, clicked: what `c` does with
/// the cursor stowed — the page's window, on its copy where it has one; a
/// word that stood this visit, its gate closed since, opens the window on
/// its one plain line and Close (spec §3.2).
pub(super) fn open_page(gui: &mut Gui) {
    let target = page_target(gui);
    if let Some(target) = opens(gui, target.clone()) {
        keep::open(gui, target);
    } else if let Some(target) = target.filter(|_| latched(gui)) {
        keep::open_off(gui, target);
    }
}

/// The page on screen, as its spot remembers the visit: the room, the
/// session, its node or its Files path. `None` off the pages a word serves.
fn page_id(gui: &Gui) -> Option<u64> {
    let files = gui.active == FILES_NAV;
    if !served(gui) || !(files || library_room(gui)) {
        return None;
    }
    Some(sig(|h| {
        (gui.active, gui.app.session_gen()).hash(h);
        if files {
            gui.app.path.hash(h);
        } else {
            hash_node(gui.app.library_stack.here(), h);
        }
    }))
}

/// The page's word stood this visit (spec §3.2).
fn latched(gui: &Gui) -> bool {
    page_id(gui).is_some_and(|id| gui.doors.latched == Some(id))
}

/// The keys are somewhere `c` does not reach the page: the queue panel
/// holds them, or the bar's filter takes text (spec §4.9).
fn keys_elsewhere(gui: &Gui) -> bool {
    (gui.queue_open && gui.app.focus == crate::tui::app::Focus::Queue) || gui.app.filtering
}

/// `c` keeps the held thing here — what the tips line names (§6.9).
fn held_keeps(gui: &mut Gui) -> bool {
    match held(gui) {
        Some(held) => {
            let target = held_target(gui, held);
            opens(gui, target).is_some()
        }
        None => false,
    }
}

// ── The header word and the state in its place (spec §4.1, §5.1) ───────────

/// What stands 3 cells after a header line's left run: the word, or the
/// page's copy state in the tile words — a button (`live`) while the gate
/// lets its press open the window, plain text once it does not.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Spot {
    pub text: String,
    pub style: Style,
    pub live: bool,
}

/// A page's spot this frame: the cells its header line plans for — the
/// word's or the state's whenever the page has a spot at all this visit,
/// drawn or not — and what is drawn there, `None` while it is hidden (the
/// line then stands as planned, its cells blank: nothing shifts as a row is
/// picked up or a modal opens, spec §3.2).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct PageSpot {
    pub cells: u16,
    pub shown: Option<Spot>,
}

/// The spot of the page on screen: the state whenever the page's own copy
/// has one, gate open or closed, cursor stowed or held; else the word,
/// while `c` with the cursor stowed keeps the page — or kept it earlier
/// this visit (spec §3.2: once drawn, it stays for the visit). A cancelled
/// copy, or one with nothing to copy, leaves the word. Never drawn under a
/// modal or the server dropdown; the word never while a row is held, the
/// queue panel holds the keys or the filter takes text — where its key
/// does not work (rule 9).
pub(super) fn page_spot(gui: &mut Gui) -> PageSpot {
    if gui.screen != Screen::Library {
        return PageSpot { cells: 0, shown: None };
    }
    let id = page_id(gui);
    gui.doors.spotted = true;
    if gui.doors.latched != id {
        gui.doors.latched = None;
    }
    let opens = opens(gui, page_target(gui)).is_some();
    if opens {
        gui.doors.latched = id;
    }
    let live = opens || (latched(gui) && page_target(gui).is_some());
    let (spot, word) = match gui.app.keep_page_state().filter(|mark| *mark != JobMark::Cancelled) {
        Some(mark) => (Some(Spot { text: keep::mark_word(mark), style: keep::mark_style(mark), live }), false),
        None if live => (Some(Spot { text: t!("gui.discover.keep.keep_word").to_string(), style: accent(), live: true }), true),
        None => (None, false),
    };
    let hidden = gui.modal_open() || gui.servers.drop_open || (word && (held(gui).is_some() || keys_elsewhere(gui)));
    PageSpot { cells: spot_cells(spot.as_ref()), shown: spot.filter(|_| !hidden) }
}

/// The spot at (x, y): the word in the ACCENT (a state in its token),
/// BRIGHT BOLD under the pointer while live; its cells a click that is `c`
/// with the cursor stowed. No tooltip: the word says itself.
pub(super) fn draw_spot(frame: &mut Frame, gui: &mut Gui, x: u16, y: u16, spot: &Spot) {
    let rect = Rect { x, y, width: width(&spot.text) as u16, height: 1 };
    let hover = spot.live && gui.ui.hovers(rect);
    put(frame, x, y, &spot.text, if hover { bright_bold() } else { spot.style });
    if spot.live {
        gui.ui.click(rect, Act::KeepWord);
    }
}

/// The spot's width in cells, for the give-way.
fn spot_cells(spot: Option<&Spot>) -> u16 {
    spot.map_or(0, |spot| width(&spot.text) as u16)
}

// ── The header line's give-way (spec §4.1.4) ────────────────────────────────

/// What a header line's subject is, as step 4 clips it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Subject<'a> {
    /// A title, an artist, a room's name: clipped from its tail.
    Title(&'a str),
    /// A Files path: whole leading segments go first ("…/Borrowed Tapes"),
    /// the open folder's own name whole while it fits.
    Path(&'a str),
}

impl Subject<'_> {
    fn text(&self) -> &str {
        match self {
            Subject::Title(text) | Subject::Path(text) => text,
        }
    }
}

/// A header line's pieces, in cells.
#[derive(Debug, Clone, Copy)]
pub(super) struct Head<'a> {
    /// The content's width.
    pub width: u16,
    /// The cells before the subject: the way back whole, and shrunk to its
    /// glyph (step 3) — "◂ Albums  " / "◂  ", "◂ Files ▸ " / "◂ ".
    pub lead: (u16, u16),
    pub subject: Subject<'a>,
    /// An album's credit, drawn " — {credit}" after its name (the user's
    /// 2026-10-27 call): it gives way before the name does.
    pub credit: Option<&'a str>,
    /// " · read-only" (step 1).
    pub tail: u16,
    /// The count (step 2): right-aligned, or riding the left run two cells
    /// after the subject (`inline`, the artist's wall's "4 albums").
    pub count: u16,
    pub inline: bool,
    /// Cells at the line's right that never give way: the pager.
    pub pager: u16,
    /// The word's or the state's (step 5).
    pub spot: u16,
}

/// How a header line stands once it fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HeadPlan {
    pub tail: bool,
    pub count: bool,
    /// The way back as its glyph alone.
    pub short: bool,
    pub subject: String,
    /// " — {credit}" as drawn, clipped, while it stands.
    pub credit: Option<String>,
    pub spot: bool,
}

impl<'a> Head<'a> {
    /// The album page's line (§4.2): `back` its way back whole ("◂ Albums",
    /// "◂ {artist}"), the name two cells after it, its credit beside it,
    /// the count at the right.
    pub(super) fn album_page(width: u16, back: &str, title: &'a str, credit: Option<&'a str>, count: &str, spot: u16) -> Head<'a> {
        let glyph = super::back_glyph();
        Head {
            width,
            lead: (cells(back) + 2, cells(glyph) + 2),
            subject: Subject::Title(title),
            credit,
            tail: 0,
            count: cells(count),
            inline: false,
            pager: 0,
            spot,
        }
    }

    /// The artist's wall's line (§4.3): `back` "◂ Artists", `way` "▸ " and
    /// the artist one cell after, the count riding two cells after it, the
    /// pager's `pager` cells at the right.
    pub(super) fn artist_wall(width: u16, back: &str, way: &str, artist: &'a str, count: Option<&str>, pager: u16, spot: u16) -> Head<'a> {
        let glyph = super::back_glyph();
        Head {
            width,
            lead: (cells(back) + 1 + cells(way), cells(glyph) + 2),
            subject: Subject::Title(artist),
            credit: None,
            tail: 0,
            count: count.map_or(0, cells),
            inline: true,
            pager,
            spot,
        }
    }

    /// A connected server's Files crumb (§4.5): back ◂ and a space, then
    /// `prefix` ("Files ▸ ") and the path — at `/` the room's name alone —
    /// then `tail` (" · read-only"), the count at the right.
    pub(super) fn crumb(width: u16, prefix: &str, path: &'a str, files: &'a str, tail: &str, count: &str, spot: u16) -> Head<'a> {
        let back = !path.is_empty();
        Head {
            width,
            lead: if back { (2 + cells(prefix), 2) } else { (0, 0) },
            subject: if back { Subject::Path(path) } else { Subject::Title(files) },
            credit: None,
            tail: cells(tail),
            count: cells(count),
            inline: false,
            pager: 0,
            spot,
        }
    }
}

/// The cells between a line's left run and its spot.
pub(super) const SPOT_GAP: u16 = 3;
/// The cells kept free before a right-aligned count or the pager.
const RIGHT_FREE: u16 = 2;
/// A subject (and a credit) clips no shorter while the spot can still go.
const SUBJECT_FLOOR: usize = 12;
/// What joins an album's name and its credit — the album page's own
/// "{album} — {artist}"; the legacy console's " - " (no em dash in its
/// code page).
fn credit_sep() -> &'static str {
    if legacy_conhost() { " - " } else { " — " }
}

fn cells(text: &str) -> u16 {
    width(text) as u16
}

/// The cells a plan takes, its right reserve included.
fn used(head: &Head<'_>, plan: &HeadPlan) -> u16 {
    let lead = if plan.short { head.lead.1 } else { head.lead.0 };
    let mut sum = lead + cells(&plan.subject) + plan.credit.as_deref().map_or(0, cells);
    if plan.tail {
        sum += head.tail;
    }
    if plan.count && head.inline {
        sum += 2 + head.count;
    }
    if plan.spot {
        sum += SPOT_GAP + head.spot;
    }
    sum + if head.pager > 0 {
        head.pager + RIGHT_FREE
    } else if plan.count && !head.inline {
        head.count + RIGHT_FREE
    } else {
        0
    }
}

/// The give-way: while the line does not fit — 2 cells kept free before
/// its count or pager — " · read-only" goes, then the count, then the way
/// back shrinks to its glyph, then the subject clips to a floor of 12 cells
/// (the credit first, then the name), then the word (or the state) goes and
/// the subject takes the room it frees — each only as far as needed: a
/// credit that went whole gives the way back and the count back as far as
/// they fit without it.
pub(super) fn fit(head: &Head<'_>) -> HeadPlan {
    let plan = fit_with(head);
    if head.credit.is_some() && plan.credit.is_none() {
        return fit_with(&Head { credit: None, ..*head });
    }
    plan
}

/// [`fit`] with the credit as it stands.
fn fit_with(head: &Head<'_>) -> HeadPlan {
    let credit = head.credit.map(|credit| format!("{}{credit}", credit_sep()));
    let spot = head.spot > 0;
    for (tail, count, short) in [(true, true, false), (false, true, false), (false, false, false), (false, false, true)] {
        let plan = HeadPlan {
            tail: tail && head.tail > 0,
            count: count && head.count > 0,
            short,
            subject: head.subject.text().to_string(),
            credit: credit.clone(),
            spot,
        };
        if used(head, &plan) <= head.width {
            return plan;
        }
    }
    let bare = HeadPlan { tail: false, count: false, short: true, subject: String::new(), credit: None, spot };
    let room = |spot: bool| head.width.saturating_sub(used(head, &HeadPlan { spot, ..bare.clone() }));
    if spot && let Some((subject, credit)) = clipped(head, room(true), SUBJECT_FLOOR) {
        return HeadPlan { subject, credit, ..bare };
    }
    let (subject, credit) = clipped(head, room(false), 0).unwrap_or_default();
    HeadPlan { subject, credit, spot: false, ..bare }
}

/// [`fit`] for an album page whose way back already names its credit (an
/// artist's wall opened it, "◂ Boukmanflow"): the artist is said once —
/// the credit stands only once the way back has shrunk to its glyph.
pub(super) fn fit_album(head: &Head<'_>, named: bool) -> HeadPlan {
    if named {
        let bare = fit(&Head { credit: None, ..*head });
        if !bare.short {
            return bare;
        }
    }
    fit(head)
}

/// The subject and its credit in `room` cells: the credit clipped to the
/// floor, else gone; the name clipped — a title from its tail, a path by
/// its leading segments — to `floor` at least, else `None`.
fn clipped(head: &Head<'_>, room: u16, floor: usize) -> Option<(String, Option<String>)> {
    let room = usize::from(room);
    let name = head.subject.text();
    let name_w = width(name);
    let sep_w = width(credit_sep());
    if let Some(credit) = head.credit
        && name_w + sep_w + SUBJECT_FLOOR.min(width(credit)) <= room
    {
        let shown = clip_tail(credit, room - name_w - sep_w);
        return Some((name.to_string(), Some(format!("{}{shown}", credit_sep()))));
    }
    if name_w <= room {
        return Some((name.to_string(), None));
    }
    if room < floor.max(1) {
        return None;
    }
    let shown = match head.subject {
        Subject::Title(title) => clip_tail(title, room),
        Subject::Path(path) => clip_path(path, room),
    };
    Some((shown, None))
}

/// `text` in `room` cells, clipped from its tail with the kit's mark — a
/// space the cut left before the mark trimmed.
fn clip_tail(text: &str, room: usize) -> String {
    let clipped = super::bar::clip(text, room).into_owned();
    let mark = if legacy_conhost() { '»' } else { '…' };
    match clipped.strip_suffix(mark) {
        Some(body) if body.ends_with(' ') => format!("{}{mark}", body.trim_end()),
        _ => clipped,
    }
}

/// A path in `room` cells (§4.1.4 step 4): its whole leading segments go —
/// "…/Boukmanflow/Borrowed Tapes", "…/Borrowed Tapes" — the open folder's
/// own name kept whole while it fits, else "…/" and that name clipped from
/// its tail. Measured in cells.
pub(super) fn clip_path(path: &str, room: usize) -> String {
    if width(path) <= room {
        return path.to_string();
    }
    let mark = if legacy_conhost() { "»" } else { "…" };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for at in 1..segments.len() {
        let shown = format!("{mark}/{}", segments[at..].join("/"));
        if width(&shown) <= room {
            return shown;
        }
    }
    match segments.last() {
        Some(last) if segments.len() > 1 && room > 2 => format!("{mark}/{}", clip_tail(last, room - 2)),
        _ => clip_tail(path, room),
    }
}

// ── Marks (spec §5) ─────────────────────────────────────────────────────────

/// A row a page asks its mark of, copied out of the App for the ask.
pub(super) enum Row {
    Song(Box<Track>),
    Card(Box<Album>),
    Folder(String),
}

/// The page's own ask, first in its first lookup.
pub(super) enum Own {
    None,
    AlbumPage,
    ArtistWall,
    Folder(String),
}

/// A hash of what a page shows, fed by the caller: its identity and its
/// rows' (cheap — no row is copied to hash it).
pub(super) fn sig(feed: impl FnOnce(&mut std::collections::hash_map::DefaultHasher)) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    feed(&mut hasher);
    hasher.finish()
}

/// A page's identity in a hash: its node, the parts that tell one page
/// from another.
pub(super) fn hash_node(node: &LibraryNode, hasher: &mut impl Hasher) {
    std::mem::discriminant(node).hash(hasher);
    match node {
        LibraryNode::Album { name, artist, album_artist, year, .. } => (name, artist, album_artist, year).hash(hasher),
        LibraryNode::Artist(name) => name.hash(hasher),
        LibraryNode::Genre(name, source) => (name, source).hash(hasher),
        _ => {}
    }
}

/// Ask a connected server's page's marks (spec §5.0; the user's 2026-10-27
/// call): the page's own ask every frame — one comparison when it stands —
/// and its rows on screen or near (whole blocks, [`near`]) whenever
/// `rows_sig` moved since the last ask, copied out then; the App's holder
/// batches them (at most 100 a lookup, one lookup out at a time), keeps
/// them for the page and asks again only a row whose copy was live and
/// ended. Never on the user's own server.
pub(super) fn ask(gui: &mut Gui, page: MarksPage, own: Own, rows_sig: u64, rows: impl FnOnce(&App) -> Vec<Row>) {
    if gui.app.session.peer.is_none() {
        return;
    }
    gui.doors.asked = true;
    // A move — the state in front stamped anew — the marks' flag landing,
    // an album page's credit asked no longer, or rows gone stale, is news
    // for the same rows.
    gui.app.keep_sync();
    let resolving = gui.app.album_credit_resolving();
    let seen = sig(|h| {
        (rows_sig, gui.app.keep.epoch, &gui.app.keep.owner, gui.app.keep_caps().discovery_plugins, resolving, gui.app.keep_marks_stirred()).hash(h)
    });
    let rows = if gui.doors.seen != Some(seen) { rows(&gui.app) } else { Vec::new() };
    gui.doors.seen = Some(seen);
    let mut asks: Vec<MarkAsk<'_>> = Vec::with_capacity(rows.len() + 1);
    match &own {
        Own::None => {}
        Own::AlbumPage => asks.push(MarkAsk::AlbumPage),
        Own::ArtistWall => asks.push(MarkAsk::ArtistWall),
        Own::Folder(path) => asks.push(MarkAsk::Folder(path)),
    }
    asks.extend(rows.iter().map(|row| match row {
        Row::Song(track) => MarkAsk::Song(track),
        Row::Card(card) => MarkAsk::Card(card),
        Row::Folder(path) => MarkAsk::Folder(path),
    }));
    let effects = gui.app.keep_marks_ask(&page, &asks);
    gui.pend(effects);
}

/// The rows of a list a page asks marks of: the whole blocks that hold the
/// `visible` rows from `first` on (the App's
/// [`marks_block`](crate::tui::app::keep::marks_block)) — so a scroll asks
/// a block at a time, never a row at a time.
pub(super) fn near(first: usize, visible: usize, len: usize) -> std::ops::Range<usize> {
    crate::tui::app::keep::marks_block(first, visible, len)
}

/// The rows a list page asks: its songs, and — in Files — its folders.
pub(super) fn list_rows(rows: &[(usize, &Entry)], folders: bool) -> Vec<Row> {
    rows.iter()
        .filter_map(|(_, entry)| match entry {
            Entry::Track { track, .. } => Some(Row::Song(track.clone())),
            Entry::Dir { path, .. } if folders => Some(Row::Folder(path.clone())),
            _ => None,
        })
        .collect()
}

/// A list's rows in a hash, by what each one is.
pub(super) fn hash_rows(rows: &[(usize, &Entry)], hasher: &mut impl Hasher) {
    for (_, entry) in rows {
        match entry {
            Entry::Track { track, .. } => track.filepath.hash(hasher),
            Entry::Dir { path, .. } => path.hash(hasher),
            other => other.label().hash(hasher),
        }
    }
}

/// The marks the drawn rows wear in their lead (§5.3, §5.4; the user's
/// 2026-10-27 call): a song's — the recording, from any server — and, in
/// Files, a folder row's folder copies alone. Nothing on the user's own
/// server, nothing before the holder answered.
pub(super) fn row_marks(app: &App, rows: &[(usize, &Entry)], folders: bool) -> Vec<Option<JobMark>> {
    if app.session.peer.is_none() {
        return Vec::new();
    }
    rows.iter()
        .map(|(_, entry)| match entry {
            Entry::Track { track, .. } => app.keep_song_mark(track),
            Entry::Dir { path, .. } if folders => app.keep_folder_mark(path),
            _ => None,
        })
        .collect()
}

/// A row's mark in its lead's first cell — one BOLD glyph in its token, the
/// selection's colours under the hand — and its words under the pointer
/// (Discover's row mark, clause 113).
pub(super) fn draw_lead_mark(frame: &mut Frame, ui: &mut crate::kit::Surface<Act>, x: u16, y: u16, mark: JobMark, held: bool) {
    let style = if held { super::sel() } else { keep::mark_style(mark) };
    put(frame, x, y, keep::mark_glyph(mark), style.add_modifier(ratatui::style::Modifier::BOLD));
    ui.tip(Rect { x, y, width: 1, height: 1 }, keep::mark_word(mark));
}

// ── Tips (spec §6.9) ────────────────────────────────────────────────────────

/// The footer's cells at 100 columns (it is drawn from x 1).
const TIPS_CELLS: usize = 99;

/// A room's tips line with the keep segment, where `c` keeps the held row
/// or card and no header word is drawn: "c keep a copy" right after the
/// line's Enter segment; while the line outgrows its cells, segments drop
/// from its tail — the keep segment and every one before it stay.
pub(super) fn with_keep(line: &str) -> String {
    keep_segment(line, &t!("gui.discover.keep.key_keep"))
}

/// [`with_keep`] with the segment's words given.
fn keep_segment(line: &str, words: &str) -> String {
    let mut parts: Vec<String> = line.split(" · ").map(str::to_string).collect();
    let at = parts.iter().position(|part| part.starts_with("Enter")).map_or(parts.len().min(1), |at| at + 1);
    parts.insert(at, words.to_string());
    while parts.len() > at + 1 && width(&parts.join(" · ")) > TIPS_CELLS {
        parts.pop();
    }
    parts.join(" · ")
}

/// The room's tips line as the keys stand: with the keep segment where it
/// works on the held thing; and — `C` opening Copies in every browse room
/// (Q1) — "C copies" while a copy moves or one failed, where the line has
/// the cells, as the Discover room names it. Never over a line that is
/// taking text.
pub(super) fn room_tips(gui: &mut Gui, line: &str) -> String {
    if gui.app.filtering || gui.app.editing_query {
        return line.to_string();
    }
    let mut line = if held_keeps(gui) { with_keep(line) } else { line.to_string() };
    if served(gui) && gui.app.keep_indicator().is_some() {
        let with = format!("{line} · {}", t!("gui.tips.discover_copies"));
        if width(&with) <= TIPS_CELLS {
            line = with;
        }
    }
    line
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;
    use serde_json::{Value, json};

    use super::super::{ALBUMS_NAV, ARTISTS_NAV, Act, DISCOVER_NAV, FILES_NAV, Gui, RECENT_NAV, SEARCH_NAV, render};
    use super::{Head, HeadPlan, Subject};
    use crate::api::types::{Album, AlbumCredit, JobLookup, Ping, PluginJob, PluginJobList, Track, TrackMetadata};
    use crate::config::Config;
    use crate::tui::app::keep::{KeepChoice, KeepFocus, KeepWatch};
    use crate::tui::app::{App, Effect, Entry, Focus, KnownServer, Origin, Queued, Session, Tab};
    use crate::tui::worker::{ApiCmd, Event, LibraryData, LibraryNode};

    const HOST: &str = "http://host:3000";
    const B: i64 = 1;
    const C: i64 = 4;
    const LOCALES: [&str; 10] = ["en", "de", "es", "fr", "it", "ja", "pl", "pt", "ru", "zh"];

    fn english() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        crate::kit::theme::pin_modern_terminal();
        guard
    }

    fn listing() -> Value {
        json!({
            "plugins": [{ "name": "federation-copy", "capabilities": ["acquire"], "scopes": ["song", "album", "artist", "artist-missing", "folder"] }],
            "jobs": { "allowed": true, "maxFolderSongs": 1000 }
        })
    }

    fn destination() -> Value {
        json!({
            "destination": { "vpath": "copies", "base": "", "layout": "{{ALBUMARTIST}}/{{ALBUM}}", "source": "default" },
            "libraries": [{ "vpath": "copies" }], "defaultLayout": "{{ALBUMARTIST}}/{{ALBUM}}", "variables": ["ARTIST", "ALBUM"]
        })
    }

    fn book() -> Vec<KnownServer> {
        let server = |id: String, name: &str, peer: Option<(String, i64)>| KnownServer {
            id,
            name: name.into(),
            token: if peer.is_none() { Some("tok".into()) } else { None },
            username: None,
            self_signed: false,
            peer,
            pairing: None,
            dj: Default::default(),
        };
        vec![
            server(HOST.into(), HOST, None),
            server(crate::config::peer_identity(HOST, B), "Stage Peer B", Some((HOST.into(), B))),
            server(crate::config::peer_identity(HOST, C), "Stage Peer C", Some((HOST.into(), C))),
        ]
    }

    fn apply(gui: &mut Gui, event: Event) {
        let effects = gui.app.apply_event(event);
        gui.pend(effects);
    }

    /// The gate's asks out, answered open; the jobs list asked with them,
    /// answered with `jobs`.
    fn answer_gate(gui: &mut Gui, jobs: &[PluginJob]) {
        let asks: Vec<u64> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::KeepGate { generation, .. }) => Some(*generation),
                _ => None,
            })
            .collect();
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::KeepGate { .. })));
        for generation in asks {
            apply(
                gui,
                Event::KeepGate {
                    generation,
                    plugins: Ok(Box::new(serde_json::from_value(listing()).unwrap())),
                    destination: Some(Ok(Box::new(serde_json::from_value(destination()).unwrap()))),
                },
            );
        }
        land_jobs(gui, jobs);
    }

    fn land_jobs(gui: &mut Gui, jobs: &[PluginJob]) {
        let asks: Vec<(u64, u64)> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Jobs { seq, generation, .. }) => Some((*seq, *generation)),
                _ => None,
            })
            .collect();
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::Jobs { .. })));
        for (seq, generation) in asks {
            apply(gui, Event::Jobs { seq, generation, answer: Ok(Box::new(PluginJobList { jobs: jobs.to_vec(), runner: None })) });
        }
    }

    /// A rec's keys, as the server builds them (enough for a test).
    fn keys_of(rec: &Value) -> Vec<(String, String)> {
        let text = |k: &str| rec.get(k).and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let credit = rec.get("albumArtist").and_then(Value::as_str).map(|s| s.trim().to_lowercase()).unwrap_or_else(|| text("artist"));
        vec![
            ("song".into(), format!("text:{}", text("title"))),
            ("album".into(), format!("album:{credit}|{}", text("album"))),
            ("artist".into(), format!("artist:{}", text("artist"))),
            ("artist-missing".into(), format!("artist-missing:{}", text("artist"))),
            ("folder".into(), format!("folder:{}", text("filepath"))),
        ]
    }

    /// The lookups out, answered with `jobs` by key; the recs they asked.
    fn land_lookups(gui: &mut Gui, jobs: &[PluginJob]) -> Vec<Value> {
        let asks: Vec<(u64, u64, Vec<Value>)> = gui
            .pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::JobsLookup { seq, generation, recs, .. }) => Some((*seq, *generation, recs.clone())),
                _ => None,
            })
            .collect();
        gui.pending.retain(|e| !matches!(e, Effect::Api(ApiCmd::JobsLookup { .. })));
        let mut asked = Vec::new();
        for (seq, generation, recs) in asks {
            let answer: Vec<JobLookup> = recs
                .iter()
                .map(|rec| {
                    let keys = keys_of(rec);
                    JobLookup {
                        key: Some(keys[0].1.clone()),
                        keys: keys.iter().map(|(s, k)| (s.clone(), json!(k))).collect(),
                        jobs: jobs.iter().filter(|j| keys.iter().any(|(_, k)| j.key.as_deref() == Some(k.as_str()))).cloned().collect(),
                    }
                })
                .collect();
            asked.extend(recs);
            apply(gui, Event::JobsLookup { seq, generation, answer: Ok(answer) });
        }
        asked
    }

    fn job(id: i64, key: &str, scope: Option<&str>, state: &str, peer: i64, result: Value) -> PluginJob {
        serde_json::from_value(json!({
            "id": id, "plugin": "federation-copy", "key": key, "state": state,
            "params": scope.map(|s| json!({ "scope": s })),
            "recommendation": { "artist": "Boukmanflow", "title": "Late Train Verse", "album": "Borrowed Tapes",
                "peer": { "id": peer, "name": if peer == B { "Stage Peer B" } else { "Stage Peer C" } } },
            "result": result,
        }))
        .unwrap()
    }

    /// The user's own server, its gate open, Stage Peer B and C in its book;
    /// the queue folded (the room 80 cells, x 17–96).
    fn own_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui.app.session.token = Some("tok".into());
        gui.app.capabilities.discovery_plugins = true;
        gui.app.capabilities.uploads = true;
        gui.app.servers = book();
        gui.queue_open = false;
        let asked = gui.app.keep_connected();
        gui.pend(asked);
        answer_gate(&mut gui, &[]);
        gui
    }

    /// Stage Peer B browsed: the move, its `Connected`, the gate asked again
    /// of the own server and answered.
    fn peer_gui() -> Gui {
        let mut gui = own_gui();
        let session = Session {
            server: HOST.into(),
            server_id: crate::config::peer_identity(HOST, B),
            token: Some("tok".into()),
            peer: Some((HOST.into(), B)),
            ..Default::default()
        };
        let moved = gui.app.adopt_server(session, None);
        gui.pend(moved);
        let ping = Ping { vpaths: vec!["peerlib".into()], ..Ping::default() };
        apply(&mut gui, Event::Connected { server: HOST.into(), id: HOST.into(), username: None, token: None, ping: Box::new(ping) });
        answer_gate(&mut gui, &[]);
        gui.pending.clear();
        assert!(gui.app.keep_gate(), "the own server's gate, on its connected server");
        gui
    }

    fn song(file: &str, title: &str, credit: Option<&str>) -> Track {
        Track {
            filepath: format!("peerlib/Boukmanflow/Borrowed Tapes/{file}"),
            metadata: TrackMetadata {
                title: Some(title.into()),
                artist: Some("Boukmanflow".into()),
                album: Some("Borrowed Tapes".into()),
                album_artist: credit.map(str::to_string),
                year: Some(2019),
                duration: Some(8.0),
                ..Default::default()
            },
        }
    }

    fn tapes(credit: Option<&str>) -> Vec<Track> {
        vec![
            song("01 - Rooftop Cypher.mp3", "Rooftop Cypher", credit),
            song("02 - Late Train Verse.mp3", "Late Train Verse", credit),
            song("03 - Corner Store Loop.mp3", "Corner Store Loop", credit),
        ]
    }

    fn tapes_node(credit: AlbumCredit) -> LibraryNode {
        LibraryNode::Album { name: "Borrowed Tapes".into(), artist: None, album_artist: credit, year: Some(2019), compilation: false }
    }

    /// The Albums room on an album page, its songs landed.
    fn album_page(gui: &mut Gui, node: LibraryNode, tracks: Vec<Track>) {
        gui.active = ALBUMS_NAV;
        let asked = gui.app.open_library_node(node.clone(), true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        apply(gui, Event::Library { generation, dest: Tab::Library, node, data: LibraryData::Tracks(tracks) });
    }

    fn card(name: Option<&str>, credit: Value, year: i32, compilation: bool) -> Album {
        serde_json::from_value(json!({ "name": name, "year": year, "album_artist": credit, "compilation": compilation,
            "artists": ["Boukmanflow"] }))
        .unwrap()
    }

    /// Boukmanflow's wall on Stage Peer B (spec §2): three of his, the
    /// compilation he appears on.
    fn bouk_wall() -> Vec<Album> {
        vec![
            card(Some("Borrowed Tapes"), json!("Boukmanflow"), 2019, false),
            card(Some("Harbour Lights"), json!("Boukmanflow"), 2021, false),
            card(Some("Second Shift"), json!("Boukmanflow"), 2023, false),
            card(Some("Night Market Sampler"), json!("Various Artists"), 2024, true),
        ]
    }

    /// The Artists room on an artist's wall, its listing landed.
    fn artist_wall(gui: &mut Gui, artist: &str, albums: Vec<Album>) {
        gui.active = ARTISTS_NAV;
        let asked = gui.app.open_library_node(LibraryNode::Artist(artist.into()), true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        apply(gui, Event::Library { generation, dest: Tab::Library, node: LibraryNode::Artist(artist.into()), data: LibraryData::Albums(albums) });
    }

    /// The Files room at `path` on Stage Peer B, with these rows.
    fn files_at(gui: &mut Gui, path: &str, entries: Vec<Entry>) {
        gui.active = FILES_NAV;
        gui.app.tab = Tab::Files;
        gui.app.path = path.into();
        gui.app.files.set(entries);
    }

    fn dir(path: &str) -> Entry {
        Entry::Dir { label: path.rsplit('/').next().unwrap().into(), path: path.into() }
    }

    fn file_row(track: &Track) -> Entry {
        Entry::Track { label: track.filepath.rsplit('/').next().unwrap().into(), track: Box::new(track.clone()) }
    }

    fn bouk_folder(gui: &mut Gui) {
        let rows = vec![
            Entry::Parent,
            dir("peerlib/Boukmanflow/Borrowed Tapes"),
            dir("peerlib/Boukmanflow/Harbour Lights"),
            dir("peerlib/Boukmanflow/Second Shift"),
        ];
        files_at(gui, "peerlib/Boukmanflow", rows);
    }

    fn buffer(gui: &mut Gui, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buffer: &Buffer) -> Vec<String> {
        let area = *buffer.area();
        (0..area.height).map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
    }

    /// Row `y` as it reads: a wide glyph once, not with the cell it covers.
    fn text_of(buffer: &Buffer, y: u16) -> String {
        let mut out = String::new();
        let mut x = 0;
        while x < buffer.area().width {
            let symbol = buffer[(x, y)].symbol();
            out.push_str(symbol);
            x += crate::kit::width(symbol).max(1) as u16;
        }
        out
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        rows(&buffer(gui, 100, 30))
    }

    /// The cells x..x+n of row y.
    fn cells(rows: &[String], y: usize, x: usize, n: usize) -> String {
        rows[y].chars().skip(x).take(n).collect()
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn focus(gui: &Gui) -> Option<KeepFocus> {
        gui.app.keep_view().map(|view| view.subject.focus)
    }

    // ── The header word (spec §4.1) ─────────────────────────────────────

    #[test]
    fn the_album_pages_word_stands_three_cells_after_its_name_and_credit_and_opens_album_focus() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        // M1, with the album's credit beside its name (the user's 2026-10-27
        // call): "◂ Albums" x 17–24, the name at 27, " — Boukmanflow", the
        // word 3 cells after, the count right-aligned to x 96.
        assert_eq!(cells(&shown, 2, 17, 8), "◂ Albums");
        assert_eq!(cells(&shown, 2, 27, 28), "Borrowed Tapes — Boukmanflow");
        assert_eq!(cells(&shown, 2, 55, 14), "   keep a copy");
        assert_eq!(cells(&shown, 2, 90, 7), "3 items");
        assert!(cells(&shown, 2, 71, 19).trim().is_empty(), "{}", shown[2]);
        // A click on its cells is `c` with the cursor stowed; the ACCENT at
        // rest.
        let buf = buffer(&mut gui, 100, 30);
        assert_eq!(buf[(58, 2)].fg, crate::kit::theme::th().accent);
        assert_eq!(gui.ui.hit(Position { x: 58, y: 2 }), Some(Act::KeepWord));
        assert_eq!(gui.ui.hit(Position { x: 68, y: 2 }), Some(Act::KeepWord));
        assert_eq!(gui.ui.hit(Position { x: 69, y: 2 }), None);
        gui.act(Act::KeepWord);
        assert_eq!(focus(&gui), Some(KeepFocus::Album));
        assert_eq!(gui.app.keep_view().unwrap().choice, KeepChoice::Album);
        // The window over the room hides the word (rule 9), the name stays.
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Borrowed Tapes") && !shown[2].contains("keep a copy"), "{}", shown[2]);
        assert!(shown.iter().any(|r| r.contains("╭─ Keep a copy ")), "the window, at the shell's level:\n{}", shown.join("\n"));
        // Its keys are the window's: Esc closes it, the word returns.
        key(&mut gui, KeyCode::Esc);
        assert!(gui.app.keep.flow.is_none());
        assert!(draw(&mut gui)[2].contains("keep a copy"));
    }

    #[test]
    fn the_album_pages_word_waits_for_a_known_credit_and_its_credit_shows_beside_the_name() {
        let _guard = english();
        // Resolving: nothing beside the name, no word.
        let mut gui = peer_gui();
        let old = |title: &str| Track { metadata: TrackMetadata { album_artist: None, ..song(&format!("{title}.mp3"), title, None).metadata }, ..song(&format!("{title}.mp3"), title, None) };
        album_page(&mut gui, tapes_node(AlbumCredit::Unknown), vec![old("Rooftop Cypher"), old("Late Train Verse")]);
        assert!(gui.app.album_credit_resolving(), "step 3 asked of the peer");
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 25), "◂ Albums  Borrowed Tapes ");
        assert!(!shown[2].contains('—') && !shown[2].contains("keep a copy"), "{}", shown[2]);
        // An opener that named an artist (one track's): nothing either while
        // the album's own credit is asked.
        let mut gui = peer_gui();
        let named = LibraryNode::Album { name: "Borrowed Tapes".into(), artist: Some("Boukmanflow".into()), album_artist: AlbumCredit::Unknown, year: Some(2019), compilation: false };
        album_page(&mut gui, named, vec![old("Rooftop Cypher"), old("Late Train Verse")]);
        assert!(gui.app.album_credit_resolving());
        let shown = draw(&mut gui);
        assert!(!shown[2].contains('—'), "{}", shown[2]);
        // Credits that disagree: the listing's artists joined ", ", no word.
        let mut gui = peer_gui();
        let split = LibraryNode::Album { name: "Split Series".into(), artist: None, album_artist: AlbumCredit::Disagree(vec!["ALM".into(), "Boukmanflow".into()]), year: Some(2022), compilation: false };
        album_page(&mut gui, split, tapes(None));
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Split Series — ALM, Boukmanflow"), "{}", shown[2]);
        assert!(!shown[2].contains("keep a copy"));
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "`c` stowed is inert there");
        // The songs' own credit agrees (step 2): known at once — the word.
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Unknown), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Borrowed Tapes — Boukmanflow   keep a copy"), "{}", shown[2]);
        // The singles bucket: no word.
        let mut gui = peer_gui();
        album_page(&mut gui, LibraryNode::album("", None), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Singles") && !shown[2].contains("keep a copy"), "{}", shown[2]);
        // The user's own server: its credit, never a word.
        let mut gui = own_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Borrowed Tapes — Boukmanflow") && !shown[2].contains("keep a copy"), "{}", shown[2]);
    }

    #[test]
    fn the_word_hides_while_a_row_is_held_but_not_on_the_way_out_and_never_under_a_modal() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        assert!(draw(&mut gui)[2].contains("keep a copy"));
        // The cursor up on the `..` row: the word stays.
        gui.app.library.state.select(Some(0));
        gui.albums.wall.tracks.held = true;
        assert!(draw(&mut gui)[2].contains("keep a copy"), "`..` held is stowed for `c`");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.app.library.state.selected(), Some(1));
        assert!(!draw(&mut gui)[2].contains("keep a copy"), "a song held: `c` keeps it, so no word");
        // `c` on the held song: its song window.
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Song));
        assert_eq!(gui.app.keep_view().unwrap().subject.title.as_deref(), Some("Rooftop Cypher"));
        key(&mut gui, KeyCode::Esc);
        key(&mut gui, KeyCode::Esc);
        assert!(draw(&mut gui)[2].contains("keep a copy"), "stowed again");
        // The track sheet over the page: no word.
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('m'));
        assert!(gui.actions.sheet.is_some());
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
        // The server dropdown over it: no word either.
        key(&mut gui, KeyCode::Esc);
        gui.act(Act::SrvMenu);
        assert!(gui.servers.drop_open);
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
    }

    #[test]
    fn the_artists_wall_wears_its_word_after_its_count_while_the_artist_has_albums_of_theirs() {
        let _guard = english();
        let mut gui = peer_gui();
        artist_wall(&mut gui, "Boukmanflow", bouk_wall());
        let shown = draw(&mut gui);
        // M3: "◂ Artists ▸ Boukmanflow  4 albums" x 17–49, the word x 53–65,
        // the pager x 89–95.
        assert_eq!(cells(&shown, 2, 17, 33), "◂ Artists ▸ Boukmanflow  4 albums");
        assert_eq!(cells(&shown, 2, 50, 14), "   keep a copy");
        assert_eq!(cells(&shown, 2, 89, 7), "◂ 1/1 ▸");
        assert_eq!(gui.ui.hit(Position { x: 53, y: 2 }), Some(Act::KeepWord));
        // Stowed `c`: artist focus.
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Artist));
        key(&mut gui, KeyCode::Esc);
        // A held card: `c` keeps that album, the word gone.
        key(&mut gui, KeyCode::Down);
        let shown = draw(&mut gui);
        assert!(!shown[2].contains("keep a copy"), "{}", shown[2]);
        key(&mut gui, KeyCode::Char('c'));
        let view = gui.app.keep_view().expect("album focus");
        assert_eq!((view.subject.focus, view.subject.album.as_deref()), (KeepFocus::Album, Some("Borrowed Tapes")));
        key(&mut gui, KeyCode::Esc);
        // Various Artists, and an artist with none of theirs: no word.
        for (artist, wall) in [
            ("Various Artists", vec![card(Some("Night Market Sampler"), json!("Various Artists"), 2024, true)]),
            ("Lantern Choir", vec![card(Some("Night Market Sampler"), json!("Various Artists"), 2024, true)]),
        ] {
            let mut gui = peer_gui();
            artist_wall(&mut gui, artist, wall);
            let shown = draw(&mut gui);
            assert!(shown[2].contains(artist) && !shown[2].contains("keep a copy"), "{artist}: {}", shown[2]);
            key(&mut gui, KeyCode::Char('c'));
            assert!(gui.app.keep.flow.is_none(), "{artist}: `c` inert");
        }
        // An older peer's listing names no credits: no word.
        let mut gui = peer_gui();
        let old: Vec<Album> = vec![serde_json::from_value(json!({ "name": "Borrowed Tapes", "year": 2019 })).unwrap()];
        artist_wall(&mut gui, "Boukmanflow", old);
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
    }

    #[test]
    fn a_files_folder_below_the_library_wears_the_word_after_its_crumb_and_read_only_alone() {
        let _guard = english();
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        let shown = draw(&mut gui);
        // M4: "◂ Files ▸ peerlib/Boukmanflow · read-only" x 17–57, the word
        // x 61–73, the count right; the clause about playlists is gone.
        assert_eq!(cells(&shown, 2, 17, 41), "◂ Files ▸ peerlib/Boukmanflow · read-only");
        assert_eq!(cells(&shown, 2, 58, 14), "   keep a copy");
        assert_eq!(cells(&shown, 2, 90, 7), "3 items");
        assert!(!shown.join("\n").contains("playlists and ratings"));
        // Stowed, or on `..`: the open folder.
        gui.app.files.state.select(Some(0));
        gui.files_view.held = true;
        assert!(draw(&mut gui)[2].contains("keep a copy"));
        key(&mut gui, KeyCode::Char('c'));
        let view = gui.app.keep_view().expect("folder focus");
        assert_eq!((view.subject.focus, view.subject.folder.as_ref().map(|f| f.path.as_str())), (KeepFocus::Folder, Some("peerlib/Boukmanflow")));
        assert_eq!(view.choice, KeepChoice::Folder(crate::tui::app::keep::FolderLanding::Mirror));
        key(&mut gui, KeyCode::Esc);
        // A held folder row: that folder, the word gone.
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.app.files.state.selected(), Some(1));
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
        key(&mut gui, KeyCode::Char('c'));
        let view = gui.app.keep_view().expect("folder focus");
        assert_eq!(view.subject.folder.as_ref().map(|f| f.name.as_str()), Some("Borrowed Tapes"));
        key(&mut gui, KeyCode::Esc);
        // The library's top: no word, `c` stowed inert; its folder rows keep.
        let mut gui = peer_gui();
        files_at(&mut gui, "peerlib", vec![dir("peerlib/Boukmanflow"), dir("peerlib/Color Out")]);
        let shown = draw(&mut gui);
        assert!(shown[2].contains("◂ Files ▸ peerlib · read-only") && !shown[2].contains("keep a copy"), "{}", shown[2]);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none());
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Folder));
        key(&mut gui, KeyCode::Esc);
        // `/` of a multi-library peer: library rows keep nothing.
        let mut gui = peer_gui();
        files_at(&mut gui, "", vec![dir("peerlib"), dir("other")]);
        let shown = draw(&mut gui);
        assert!(shown[2].contains("Files · read-only") && !shown[2].contains("keep a copy"), "{}", shown[2]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "a library row at `/`: inert");
    }

    #[test]
    fn the_artists_genres_and_recent_headings_say_read_only_alone_and_keep_off_the_queue() {
        let _guard = english();
        let mut gui = peer_gui();
        gui.queue_open = true;
        gui.active = ARTISTS_NAV;
        let asked = gui.app.open_library_node(LibraryNode::Artists, true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::Library { generation, dest: Tab::Library, node: LibraryNode::Artists, data: LibraryData::Artists(vec!["ALM".into(), "Boukmanflow".into()]) });
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 19), "Artists · read-only");
        assert!(cells(&shown, 2, 57, 9).starts_with("2 items  "), "the count right-aligned, short of the panel: {}", shown[2]);
    }

    // ── The give-way (spec §4.1.4) ──────────────────────────────────────

    fn head<'a>(width: u16, subject: Subject<'a>, credit: Option<&'a str>, spot: u16) -> Head<'a> {
        Head { width, lead: (10, 3), subject, credit, tail: 12, count: 7, inline: false, pager: 0, spot }
    }

    #[test]
    fn the_line_gives_way_read_only_then_the_count_then_the_way_back_then_the_subject_then_the_word() {
        let plan = |h: &Head| super::fit(h);
        let full = HeadPlan { tail: true, count: true, short: false, subject: "Borrowed Tapes".into(), credit: None, spot: true };
        assert_eq!(plan(&head(80, Subject::Title("Borrowed Tapes"), None, 13)), full);
        // 10 + 14 + 12 + 3 + 13 + (7 + 2) = 61: one cell short drops read-only first.
        assert_eq!(plan(&head(60, Subject::Title("Borrowed Tapes"), None, 13)), HeadPlan { tail: false, ..full.clone() });
        assert_eq!(plan(&head(48, Subject::Title("Borrowed Tapes"), None, 13)), HeadPlan { tail: false, count: false, ..full.clone() });
        assert_eq!(plan(&head(39, Subject::Title("Borrowed Tapes"), None, 13)), HeadPlan { tail: false, count: false, short: true, ..full.clone() });
        // The subject clips to its floor of 12, then the word goes and the
        // subject takes the room it frees.
        let clipped = plan(&head(31, Subject::Title("Borrowed Tapes"), None, 13));
        assert_eq!((clipped.subject.as_str(), clipped.spot), ("Borrowed Ta…", true));
        let gone = plan(&head(27, Subject::Title("Borrowed Tapes"), None, 13));
        assert_eq!((gone.subject.as_str(), gone.spot), ("Borrowed Tapes", false));
        // The credit gives way before the name, clipped to the floor first.
        let both = plan(&head(80, Subject::Title("Borrowed Tapes"), Some("Boukmanflow"), 13));
        assert_eq!(both.credit.as_deref(), Some(" — Boukmanflow"));
        let short = plan(&head(48, Subject::Title("Borrowed Tapes"), Some("The Boukmanflow Collective"), 13));
        assert_eq!((short.subject.as_str(), short.credit.as_deref(), short.spot), ("Borrowed Tapes", Some(" — The Boukman…"), true));
        let dropped = plan(&head(40, Subject::Title("Borrowed Tapes"), Some("Boukmanflow"), 13));
        assert_eq!((dropped.subject.as_str(), dropped.credit, dropped.spot), ("Borrowed Tapes", None, true));
        // A path goes by whole leading segments, the folder's name whole.
        let path = "peerlib/Boukmanflow/Borrowed Tapes";
        assert_eq!(plan(&Head { lead: (10, 2), ..head(47, Subject::Path(path), None, 13) }).subject, "…/Boukmanflow/Borrowed Tapes");
        assert_eq!(plan(&Head { lead: (10, 2), ..head(37, Subject::Path(path), None, 13) }).subject, "…/Borrowed Tapes");
        assert_eq!(plan(&Head { lead: (10, 2), ..head(31, Subject::Path(path), None, 13) }).subject, "…/Borrowed T…");
        assert_eq!(plan(&Head { lead: (10, 2), ..head(30, Subject::Path(path), None, 13) }).subject, "…/Borrowed…", "no space before the mark");
        // The pager never gives way, and keeps its 2 free cells.
        let wall = Head { width: 47, lead: (12, 3), subject: Subject::Title("Boukmanflow"), credit: None, tail: 0, count: 8, inline: true, pager: 8, spot: 13 };
        let p = plan(&wall);
        assert_eq!((p.count, p.short, p.spot), (false, true, true));
        assert!(super::used(&wall, &p) <= 47);
    }

    /// The five §4.1.4 lines, English, drawn: the album page from the wall
    /// and from the artist, the artist's wall, Files at a folder and one
    /// deeper — each line's text, its cells, and what must stand whole.
    fn five_lines(queue: bool) -> Vec<(String, String, String, Vec<String>)> {
        let mut out = Vec::new();
        let mut push = |what: &str, gui: &mut Gui, whole: &[&str]| {
            gui.queue_open = queue;
            let buf = buffer(gui, 100, 30);
            let mut must: Vec<String> = whole.iter().map(|w| w.to_string()).collect();
            must.push("keep a copy".into());
            out.push((what.to_string(), text_of(&buf, 2), rows(&buf)[2].clone(), must));
        };
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        push("album page", &mut gui, &["Borrowed Tapes"]);
        let mut gui = peer_gui();
        artist_wall(&mut gui, "Boukmanflow", bouk_wall());
        let opened = gui.app.open_library_node(LibraryNode::album_of(&bouk_wall()[0]).unwrap(), false);
        gui.pend(opened);
        let generation = gui.app.session_gen();
        let node = gui.app.library_stack.here().clone();
        apply(&mut gui, Event::Library { generation, dest: Tab::Library, node, data: LibraryData::Tracks(tapes(Some("Boukmanflow"))) });
        push("album page from the artist", &mut gui, &["◂ Boukmanflow", "Borrowed Tapes"]);
        let mut gui = peer_gui();
        artist_wall(&mut gui, "Boukmanflow", bouk_wall());
        push("artist's wall", &mut gui, &["Boukmanflow"]);
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        push("Files", &mut gui, &["peerlib/Boukmanflow"]);
        let mut gui = peer_gui();
        files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tapes(None)[0]), file_row(&tapes(None)[1]), file_row(&tapes(None)[2])]);
        push("Files deeper", &mut gui, &["…/Boukmanflow/Borrowed Tapes"]);
        out
    }

    #[test]
    fn the_five_lines_drawn_keep_their_titles_and_words_and_stay_inside_the_room() {
        let _guard = english();
        for (queue, right) in [(true, 64), (false, 97)] {
            for (what, line, cells, must) in five_lines(queue) {
                for whole in &must {
                    // At 80 cells the deeper path stands whole.
                    let whole = if queue { whole.clone() } else { whole.replace('…', "peerlib") };
                    assert!(line.contains(whole.as_str()), "{what} at {}: {whole:?} whole in {line:?}", right - 17);
                }
                let past: String = cells.chars().skip(right).take(if queue { 2 } else { 3 }).collect();
                assert!(past.trim().is_empty(), "{what}: nothing past the room's right: {line:?}");
            }
        }
    }

    /// The room's cells of the header line (row 2).
    fn room_line(gui: &mut Gui) -> String {
        let width = if gui.queue_open { 47 } else { 80 };
        cells(&draw(gui), 2, 17, width)
    }

    #[test]
    fn the_header_line_stands_as_planned_while_its_word_hides() {
        let _guard = english();
        let blank = |line: &str| line.replace("keep a copy", &" ".repeat(11));
        for queue in [true, false] {
            // The album page: a row held, the track sheet, the server
            // dropdown — the word goes, nothing else moves (spec §3.2).
            let mut gui = peer_gui();
            gui.queue_open = queue;
            album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
            let stowed = room_line(&mut gui);
            assert!(stowed.contains("keep a copy"), "{stowed}");
            key(&mut gui, KeyCode::Down);
            key(&mut gui, KeyCode::Down);
            assert_eq!(room_line(&mut gui), blank(&stowed), "a row held, queue {queue}");
            key(&mut gui, KeyCode::Char('m'));
            assert!(gui.actions.sheet.is_some());
            assert_eq!(room_line(&mut gui), blank(&stowed), "the sheet over it");
            key(&mut gui, KeyCode::Esc);
            key(&mut gui, KeyCode::Esc);
            assert_eq!(room_line(&mut gui), stowed, "stowed again");
            // The dropdown hangs over the line's right: its left as planned.
            gui.act(Act::SrvMenu);
            let left = |line: &str| line.chars().take(40).collect::<String>();
            assert_eq!(left(&room_line(&mut gui)), left(&blank(&stowed)), "the dropdown over it");
            // The artist's wall: a card held.
            let mut gui = peer_gui();
            gui.queue_open = queue;
            artist_wall(&mut gui, "Boukmanflow", bouk_wall());
            let stowed = room_line(&mut gui);
            key(&mut gui, KeyCode::Down);
            assert_eq!(room_line(&mut gui), blank(&stowed), "a card held, queue {queue}");
            // Files one folder deeper: a file row held keeps the plan —
            // " · read-only" no more back than it stood (the spec's
            // fragment) — and the window over it too.
            let mut gui = peer_gui();
            gui.queue_open = queue;
            files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tapes(None)[0]), file_row(&tapes(None)[1])]);
            let stowed = room_line(&mut gui);
            key(&mut gui, KeyCode::Down);
            key(&mut gui, KeyCode::Down);
            assert_eq!(room_line(&mut gui), blank(&stowed), "a file row held, queue {queue}");
            assert_eq!(room_line(&mut gui).contains("read-only"), stowed.contains("read-only"), "queue {queue}");
            key(&mut gui, KeyCode::Char('c'));
            assert!(gui.app.keep.flow.is_some());
            assert_eq!(room_line(&mut gui), blank(&stowed), "the window over it");
        }
        // The 80-cell line, a row held: the word's cells blank, " · read-only"
        // whole before them.
        let mut gui = peer_gui();
        files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tapes(None)[0]), file_row(&tapes(None)[1]), file_row(&tapes(None)[2])]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 80), "◂ Files ▸ peerlib/Boukmanflow/Borrowed Tapes · read-only                 3 items");
    }

    #[test]
    fn the_word_is_not_drawn_where_c_does_not_reach_the_page() {
        let _guard = english();
        let mut gui = peer_gui();
        gui.queue_open = true;
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        let stowed = room_line(&mut gui);
        let word = stowed.find("keep a copy").expect("the word");
        // The queue panel holds the keys: `c` is the queue's (inert), so no
        // word — its cells kept.
        gui.app.queue.push(Queued { origin: Origin { server: HOST.into(), peer: Some(B) }, dj: None, track: tapes(Some("Boukmanflow"))[0].clone() });
        gui.app.focus = Focus::Queue;
        let line = room_line(&mut gui);
        assert_eq!(line, stowed.replace("keep a copy", &" ".repeat(11)));
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none());
        // The keys back in the room: the word in the same cells.
        gui.app.focus = Focus::Browser;
        assert_eq!(room_line(&mut gui).find("keep a copy"), Some(word));
        // The filter taking text: `c` is a letter of it — no word.
        gui.app.filtering = true;
        assert!(!room_line(&mut gui).contains("keep a copy"));
    }

    #[test]
    fn the_word_stays_for_the_visit_and_a_press_after_its_gate_closed_says_why() {
        let _guard = english();
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        assert!(draw(&mut gui)[2].contains("keep a copy"));
        // The gate closes mid-visit (Save in Where copies land, a re-ask
        // after a refusal): the word stays, live, until the next visit.
        gui.app.keep.gate.view.as_mut().unwrap().destination = None;
        assert!(!gui.app.keep_gate());
        let shown = draw(&mut gui);
        assert!(shown[2].contains("keep a copy"), "{}", shown[2]);
        assert_eq!(gui.ui.hit(Position { x: 62, y: 2 }), Some(Act::KeepWord));
        gui.act(Act::KeepWord);
        let view = gui.app.keep_view().expect("its window");
        assert_eq!(view.stage, crate::tui::app::keep::KeepStage::Off(crate::tui::app::keep::OffWhy::Uploads));
        let window = draw(&mut gui).join("\n");
        assert!(window.contains("[ Close ]") && !window.contains("As on Stage Peer B"), "its one plain line: {window}");
        key(&mut gui, KeyCode::Esc);
        // Another page is another visit: no word there, `c` inert.
        files_at(&mut gui, "peerlib/Color Out", vec![Entry::Parent, dir("peerlib/Color Out/Peer Sessions")]);
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none());
        // And back on the first: a new visit, the gate's word now.
        bouk_folder(&mut gui);
        assert!(!draw(&mut gui)[2].contains("keep a copy"));
    }

    #[test]
    fn in_every_locale_at_47_and_80_cells_no_title_clips_and_no_word_goes() {
        // Each locale's words asked for by name: the process's locale — and
        // every test reading English — is left alone.
        crate::kit::theme::pin_modern_terminal();
        for code in LOCALES {
            let t = |key: &str| super::t!(key, locale = code).to_string();
            let album = |head: &Head, named: bool| super::fit_album(head, named);
            let spot = super::cells(&t("gui.discover.keep.keep_word"));
            let albums = format!("◂ {}", t("gui.nav.albums"));
            let artists = format!("◂ {}", t("gui.nav.artists"));
            let items = super::t!("gui.files.items", locale = code, count = 3).to_string();
            let count = super::t!("gui.alb.count", locale = code, count = 4).to_string();
            let files = t("gui.nav.files");
            let prefix = format!("{files} ▸ ");
            let tail = format!(" · {}", t("gui.srv.read_only"));
            assert!(!tail.contains('—'), "{code}: read-only alone: {tail}");
            let deep = "peerlib/Boukmanflow/Borrowed Tapes";
            for width in [47, 80] {
                let lines = [
                    ("album page", Head::album_page(width, &albums, "Borrowed Tapes", Some("Boukmanflow"), &items, spot), "Borrowed Tapes"),
                    ("from the artist", Head::album_page(width, "◂ Boukmanflow", "Borrowed Tapes", Some("Boukmanflow"), &items, spot), "Borrowed Tapes"),
                    ("artist's wall", Head::artist_wall(width, &artists, "▸ ", "Boukmanflow", Some(&count), 8, spot), "Boukmanflow"),
                    ("Files", Head::crumb(width, &prefix, "peerlib/Boukmanflow", &files, &tail, &items, spot), "peerlib/Boukmanflow"),
                    ("Files deeper", Head::crumb(width, &prefix, deep, &files, &tail, &items, spot), "/Borrowed Tapes"),
                ];
                for (what, head, whole) in lines {
                    let plan = if what == "from the artist" { album(&head, true) } else { super::fit(&head) };
                    let at = format!("{code} {what} at {width}: {plan:?}");
                    assert!(plan.spot, "no word goes: {at}");
                    // A credit that went whole gives the way back back (each
                    // step only as far as needed).
                    if what == "album page" && plan.credit.is_none() {
                        assert!(!plan.short, "the way back whole without the credit: {at}");
                    }
                    assert!(plan.subject == whole || plan.subject.ends_with(whole), "no title clipped: {at}");
                    assert!(super::used(&head, &plan) <= width, "within the room: {at}");
                    if width == 80 {
                        // At 80 only " · read-only" gives way — on Files —
                        // and Spanish's deepest path drops its count too.
                        assert!(!plan.short && plan.subject.ends_with(whole), "{at}");
                        assert!(plan.count || (what == "Files deeper" && code == "es"), "{at}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_credit_that_goes_gives_the_way_back_and_the_count_back() {
        // RecStates ROW 2 · DEUTSCH: at 47 cells the credit does not fit
        // even clipped, so it goes — and "◂ Alben" stands whole.
        crate::kit::theme::pin_modern_terminal();
        let t = |key: &str| super::t!(key, locale = "de").to_string();
        let back = format!("◂ {}", t("gui.nav.albums"));
        let word = t("gui.discover.keep.keep_word");
        let items = super::t!("gui.files.items", locale = "de", count = 3).to_string();
        let head = Head::album_page(47, &back, "Borrowed Tapes", Some("Boukmanflow"), &items, super::cells(&word));
        let plan = super::fit(&head);
        assert_eq!((plan.short, plan.count, plan.credit.as_deref(), plan.spot), (false, false, None, true));
        assert_eq!(format!("{back}  {}   {word}", plan.subject), "◂ Alben  Borrowed Tapes   Kopie behalten");
        // English keeps its credit at 47, the way back shrunk (step 3
        // comes before the subject clips).
        let head = Head::album_page(47, "◂ Albums", "Borrowed Tapes", Some("Boukmanflow"), "3 items", 11);
        let plan = super::fit(&head);
        assert_eq!((plan.short, plan.credit.as_deref()), (true, Some(" — Boukmanflow")));
        // A compilation from the wall: " — Various Artists" goes, "◂ Albums"
        // and the name stand (46 cells).
        let head = Head::album_page(47, "◂ Albums", "Night Market Sampler", Some("Various Artists"), "3 items", 11);
        let plan = super::fit(&head);
        assert_eq!((plan.short, plan.credit, plan.subject.as_str()), (false, None, "Night Market Sampler"));
    }

    // ── `c` and `C` (spec §4.9; the user's 2026-10-27 decision) ─────────

    #[test]
    fn c_keeps_the_held_thing_else_the_page_and_never_the_one_under_the_pointer() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        // The pointer on a song row: `c` still keeps the page's album.
        let y = shown.iter().position(|r| r.contains("Late Train Verse")).unwrap() as u16;
        gui.ui.pointer = Some(Position { x: 40, y });
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Album), "the hovered row never counts");
        key(&mut gui, KeyCode::Esc);
        // Recent: a held song, its window; stowed, nothing.
        let mut gui = peer_gui();
        gui.active = RECENT_NAV;
        let asked = gui.app.open_library_node(LibraryNode::Recent, true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::Library { generation, dest: Tab::Library, node: LibraryNode::Recent, data: LibraryData::Tracks(tapes(Some("Boukmanflow"))) });
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "Recent stowed: nothing");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Song));
        key(&mut gui, KeyCode::Esc);
        // The Albums wall: stowed nothing, a held card its album — a card
        // whose credits disagree and the singles bucket nothing.
        let mut gui = peer_gui();
        gui.active = ALBUMS_NAV;
        let asked = gui.app.open_library_node(LibraryNode::Albums, true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        let wall = vec![card(Some("Split Series"), Value::Null, 2022, false), card(Some("Borrowed Tapes"), json!("Boukmanflow"), 2019, false)];
        apply(&mut gui, Event::Library { generation, dest: Tab::Library, node: LibraryNode::Albums, data: LibraryData::Albums(wall) });
        draw(&mut gui);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "the wall stowed: nothing");
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "credits that disagree: inert");
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Down);
        gui.albums.wall.cursor = 1;
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(gui.app.keep_view().map(|v| v.subject.album.clone()), Some(Some("Borrowed Tapes".into())));
        key(&mut gui, KeyCode::Esc);
        // Search: a held song hit, its window; the class menu nothing.
        let mut gui = peer_gui();
        gui.active = SEARCH_NAV;
        gui.app.tab = Tab::Search;
        gui.app.search.set(vec![file_row(&tapes(Some("Boukmanflow"))[1])]);
        gui.search_view.pick_up();
        gui.app.search.state.select(Some(0));
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(focus(&gui), Some(KeepFocus::Song));
        key(&mut gui, KeyCode::Esc);
        // Files: a held file row, its song window — no Folder choice.
        let mut gui = peer_gui();
        files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tapes(None)[0])]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        let view = gui.app.keep_view().expect("a song window");
        assert_eq!(view.subject.focus, KeepFocus::Song);
        assert!(view.choices.iter().all(|(c, _)| !matches!(c, KeepChoice::Folder(_))), "no Folder from a file row");
    }

    #[test]
    fn c_is_inert_on_the_held_queue_row_and_on_the_users_own_server() {
        let _guard = english();
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        gui.queue_open = true;
        let origin = Origin { server: HOST.into(), peer: Some(B) };
        gui.app.queue.push(Queued { origin, dj: None, track: tapes(Some("Boukmanflow"))[1].clone() });
        gui.app.focus = Focus::Queue;
        gui.app.queue.state.select(Some(0));
        gui.queue_view.held = true;
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "not bound on the held queue row, nor the room's beneath");
        assert_eq!(gui.app.queue.items.len(), 1);
        // The user's own server's rooms: `c` keeps nothing anywhere.
        let mut gui = own_gui();
        files_at(&mut gui, "music/Boukmanflow", vec![Entry::Parent, dir("music/Boukmanflow/Borrowed Tapes")]);
        key(&mut gui, KeyCode::Char('c'));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none());
    }

    #[test]
    fn capital_c_opens_the_copies_list_in_a_browse_room_while_the_gate_is_open() {
        let _guard = english();
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        draw(&mut gui);
        // Borrowed Tapes is copying: its row's mark is live.
        let folder = job(80, "folder:peerlib/boukmanflow/borrowed tapes", Some("folder"), "running", B, Value::Null);
        land_lookups(&mut gui, std::slice::from_ref(&folder));
        gui.pending.clear();
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_some(), "Copies over the Files room");
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::Jobs { reach: Some(r), .. }) if r.peer.is_none())), "asked of the own server: {:?}", gui.pending);
        land_jobs(&mut gui, std::slice::from_ref(&folder));
        let shown = draw(&mut gui);
        assert!(shown.iter().any(|r| r.contains("Copies")), "{}", shown.join("\n"));
        assert_eq!(gui.app.keep.watch, KeepWatch::Modal, "its poll runs while it shows");
        // Its keys are its own; Esc closes it — the room's beat again while
        // the page shows a copy moving (spec §5.0).
        key(&mut gui, KeyCode::Esc);
        assert!(gui.discover.copies.is_none());
        draw(&mut gui);
        assert_eq!(gui.app.keep.watch, KeepWatch::Room);
        // A page that shows nothing moving: no beat.
        files_at(&mut gui, "peerlib/Color Out", vec![Entry::Parent, dir("peerlib/Color Out/Peer Sessions")]);
        draw(&mut gui);
        land_lookups(&mut gui, &[]);
        draw(&mut gui);
        assert_eq!(gui.app.keep.watch, KeepWatch::Away);
        // The gate closed: `C` does nothing.
        gui.app.keep.gate.plugins.as_mut().unwrap().jobs.allowed = false;
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_none());
        // Off a connected server too, in a browse room.
        let mut gui = own_gui();
        files_at(&mut gui, "music", vec![dir("music/Boukmanflow")]);
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.discover.copies.is_some());
        // The Discover room keeps its own `C`.
        assert!(!super::served(&Gui { active: DISCOVER_NAV, ..own_gui() }));
    }

    // ── The track sheet's row (spec §4.7) ───────────────────────────────

    #[test]
    fn the_sheet_offers_keep_a_copy_for_a_connected_servers_song_only_its_badge_row_blank() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        gui.act(Act::More(Tab::Library, 2));
        let shown = draw(&mut gui);
        let y = shown.iter().position(|r| r.contains("Keep a copy…")).expect("the row");
        assert!(!shown.join("\n").contains("from Stage Peer B"), "the badge row stays blank");
        assert!(y > 0);
        // `c` from anywhere in the sheet: the song window, the sheet gone.
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.actions.sheet.is_none());
        assert_eq!(gui.app.keep_view().map(|v| v.subject.title.clone()), Some(Some("Late Train Verse".into())));
        key(&mut gui, KeyCode::Esc);
        // A queue item of Stage Peer B, the user's own server browsed: its row.
        let mut gui = own_gui();
        gui.queue_open = true;
        let peer = Origin { server: HOST.into(), peer: Some(B) };
        let own = Origin { server: HOST.into(), peer: None };
        gui.app.queue.push(Queued { origin: peer, dj: None, track: tapes(Some("Boukmanflow"))[0].clone() });
        let mut mine = tapes(None)[2].clone();
        mine.filepath = "music/host.mp3".into();
        gui.app.queue.push(Queued { origin: own.clone(), dj: None, track: mine.clone() });
        gui.act(Act::QueueMore(0));
        assert!(draw(&mut gui).iter().any(|r| r.contains("Keep a copy…")), "a connected server's queue item");
        gui.act(Act::SheetVerb(super::super::actions::SheetAction::KeepCopy));
        let view = gui.app.keep_view().expect("its window on its own server");
        assert_eq!((view.subject.peer, view.subject.title.as_deref()), (B, Some("Rooftop Cypher")));
        key(&mut gui, KeyCode::Esc);
        gui.act(Act::QueueMore(1));
        assert!(!draw(&mut gui).iter().any(|r| r.contains("Keep a copy…")), "your own song: none");
        key(&mut gui, KeyCode::Esc);
        // The playing card's sheet, when the playing track is a connected
        // server's (Q9).
        gui.app.now_playing = Some(tapes(Some("Boukmanflow"))[0].clone());
        gui.app.queue.current = Some(0);
        gui.act(Act::NowMore);
        assert!(draw(&mut gui).iter().any(|r| r.contains("Keep a copy…")), "the playing card");
        key(&mut gui, KeyCode::Esc);
        // The gate closed: no row.
        gui.app.keep.gate.plugins.as_mut().unwrap().jobs.allowed = false;
        gui.act(Act::QueueMore(0));
        assert!(!draw(&mut gui).iter().any(|r| r.contains("Keep a copy…")));
    }

    #[test]
    fn the_playing_cards_sheet_keeps_a_copy_on_every_screen_and_its_window_brings_the_library_back() {
        let _guard = english();
        let mut gui = own_gui();
        gui.queue_open = true;
        let peer = Origin { server: HOST.into(), peer: Some(B) };
        gui.app.queue.push(Queued { origin: peer, dj: None, track: tapes(Some("Boukmanflow"))[0].clone() });
        gui.app.now_playing = Some(tapes(Some("Boukmanflow"))[0].clone());
        gui.app.queue.current = Some(0);
        // Q9: the card's sheet on the Admin screen offers it too.
        gui.act(Act::Screen(super::super::Screen::Admin));
        gui.act(Act::NowMore);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| s.item.is_some()), "the row stands off the Library screen");
        assert!(draw(&mut gui).iter().any(|r| r.contains("Keep a copy…")));
        // `c`: the Library screen back, the song window over it.
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(gui.screen, super::super::Screen::Library);
        assert_eq!(focus(&gui), Some(KeepFocus::Song));
        assert!(draw(&mut gui).iter().any(|r| r.contains("╭─ Keep a copy ")));
    }

    #[test]
    fn the_tips_are_the_windows_over_a_queue_that_held_the_keys_and_a_connected_song_names_no_rating_keys() {
        let _guard = english();
        let mut gui = own_gui();
        gui.config.gui.key_hints = true;
        gui.queue_open = true;
        let peer = Origin { server: HOST.into(), peer: Some(B) };
        gui.app.queue.push(Queued { origin: peer.clone(), dj: None, track: tapes(Some("Boukmanflow"))[0].clone() });
        gui.app.queue.push(Queued { origin: peer, dj: None, track: tapes(Some("Boukmanflow"))[1].clone() });
        gui.app.focus = Focus::Queue;
        gui.app.queue.state.select(Some(0));
        gui.queue_view.held = true;
        gui.act(Act::QueueMore(0));
        assert_eq!(draw(&mut gui)[29].trim(), "↑ ↓ rows · Enter choose · c keep a copy · Esc close");
        // Keep a copy… from it: the window's keys, the window's tips —
        // never the queue's (Enter starts the copy, `d` is Where files land).
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_some() && gui.app.focus == Focus::Queue);
        assert_eq!(draw(&mut gui)[29].trim(), super::super::keep::tips(&gui));
        assert!(!draw(&mut gui)[29].contains("Enter play"));
        key(&mut gui, KeyCode::Esc);
        assert_eq!(draw(&mut gui)[29].trim(), "↑ ↓ rows · Enter play · < > move · d remove · C clear · m actions · i playing · Esc back");
        // Its gate closed: no row, and no rating keys for a song of a
        // connected server.
        gui.app.keep.gate.plugins.as_mut().unwrap().jobs.allowed = false;
        gui.act(Act::QueueMore(0));
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| s.item.is_none()));
        assert_eq!(draw(&mut gui)[29].trim(), "↑ ↓ rows · Enter choose · Esc close");
    }

    #[test]
    fn a_wide_nav_label_never_hides_the_windows_left_border_with_the_queue_open() {
        let _guard = english();
        struct English;
        impl Drop for English {
            fn drop(&mut self) {
                rust_i18n::set_locale("en");
            }
        }
        let _back = English;
        // The queue open (the default): Keep a copy and Copies centre over
        // the frame, their left border at x 12 — inside "アーティスト".
        for code in ["ja", "zh"] {
            rust_i18n::set_locale(code);
            let mut gui = peer_gui();
            gui.queue_open = true;
            album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
            draw(&mut gui);
            for open in ['c', 'C'] {
                key(&mut gui, KeyCode::Char(open));
                if open == 'C' {
                    land_jobs(&mut gui, &[]);
                }
                let buf = buffer(&mut gui, 100, 30);
                let all = rows(&buf).join("\n");
                let top = (0..30).find(|&y| buf[(12, y)].symbol() == "╭").unwrap_or_else(|| panic!("{code} {open}: the window\n{all}"));
                let bottom = (top..30).find(|&y| buf[(12, y)].symbol() == "╰").unwrap_or_else(|| panic!("{code} {open}: its bottom\n{all}"));
                for y in top..=bottom {
                    assert!(crate::kit::width(buf[(11, y)].symbol()) <= 1, "{code} {open}: row {y} spills onto the border\n{all}");
                }
                key(&mut gui, KeyCode::Esc);
            }
        }
    }

    #[test]
    fn the_legacy_console_draws_the_lines_and_the_marks_in_its_own_glyphs() {
        let _guard = english();
        let _legacy = crate::kit::theme::legacy_glyphs_here();
        // Spec §11: the way back "<", the crumb arrow ">", no em dash, the
        // clip mark "»"; the states and leads in the legacy tile words.
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 80), "< Albums  Borrowed Tapes - Boukmanflow   keep a copy                     3 items");
        let tracks = tapes(Some("Boukmanflow"));
        let account = json!({ "scope": "album", "stopped": null, "songs": { "total": 3,
            "copied": [{ "from": tracks[0].filepath, "filepath": "copies/a.mp3" }, { "from": tracks[1].filepath, "filepath": "copies/b.mp3" }],
            "skipped": [{ "from": tracks[2].filepath, "why": "owned", "at": "music/c.mp3" }], "failed": [] } });
        land_lookups(&mut gui, &[job(50, "album:boukmanflow|borrowed tapes", Some("album"), "done", B, account)]);
        let shown = draw(&mut gui);
        assert!(cells(&shown, 2, 17, 80).starts_with("< Albums  Borrowed Tapes - Boukmanflow   + kept"), "{}", shown[2]);
        let y = shown.iter().position(|r| r.contains("Rooftop Cypher")).unwrap();
        assert_eq!(cells(&shown, y, 17, 2), "+ ");
        // The artist's wall, its card's year line.
        let mut gui = peer_gui();
        artist_wall(&mut gui, "Boukmanflow", bouk_wall());
        let shown = draw(&mut gui);
        assert!(cells(&shown, 2, 17, 80).starts_with("< Artists > Boukmanflow  4 albums   keep a copy"), "{}", shown[2]);
        let kept = job(53, "album:boukmanflow|borrowed tapes", Some("album"), "done", B,
            json!({ "scope": "album", "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null }));
        land_lookups(&mut gui, &[kept]);
        let shown = draw(&mut gui);
        let year_y = shown.iter().position(|r| r.contains("2021")).expect("the year line");
        assert_eq!(cells(&shown, year_y, 17, 6), "+ kept");
        // Files: the crumb, its rows' leads; one deeper with the queue open,
        // the path by whole segments behind "»".
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 55), "< Files > peerlib/Boukmanflow · read-only   keep a copy");
        land_lookups(&mut gui, &[job(55, "folder:peerlib/boukmanflow/borrowed tapes", Some("folder"), "running", B, Value::Null)]);
        let shown = draw(&mut gui);
        let row = shown.iter().position(|r| r.contains("Borrowed Tapes")).unwrap();
        assert_eq!(cells(&shown, row, 17, 2), "v ");
        let mut gui = peer_gui();
        gui.queue_open = true;
        files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tapes(None)[0])]);
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 17, 47).trim_end(), "< »/Boukmanflow/Borrowed Tapes   keep a copy");
    }

    // ── Marks (spec §5; the user's 2026-10-27 call) ─────────────────────

    #[test]
    fn the_album_pages_rows_and_header_wear_this_servers_copies_asked_in_one_lookup() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        draw(&mut gui);
        let lookups = gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::JobsLookup { .. }))).count();
        assert_eq!(lookups, 1, "one lookup for the page: {:?}", gui.pending);
        // The album copied: 2 copied, 1 already yours — kept; rows ✓.
        let tracks = tapes(Some("Boukmanflow"));
        let account = json!({ "scope": "album", "stopped": null, "songs": { "total": 3,
            "copied": [{ "from": tracks[0].filepath, "filepath": "copies/a.mp3" }, { "from": tracks[1].filepath, "filepath": "copies/b.mp3" }],
            "skipped": [{ "from": tracks[2].filepath, "why": "owned", "at": "music/c.mp3" }], "failed": [] } });
        let kept = job(50, "album:boukmanflow|borrowed tapes", Some("album"), "done", B, account);
        let recs = land_lookups(&mut gui, std::slice::from_ref(&kept));
        assert_eq!(recs.len(), 4, "the album first, then its three songs");
        assert_eq!(recs[0]["album"], "Borrowed Tapes");
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 55, 9), "   ✓ kept");
        for (title, glyph) in [("Rooftop Cypher", "✓"), ("Late Train Verse", "✓"), ("Corner Store Loop", "✓")] {
            let y = shown.iter().position(|r| r.contains(title)).unwrap();
            assert_eq!(cells(&shown, y, 17, 1), glyph, "{title}: {}", shown[y]);
        }
        // The state is a button with the gate open: it opens that copy.
        assert_eq!(gui.ui.hit(Position { x: 60, y: 2 }), Some(Act::KeepWord));
        // Nothing more is asked while nothing changes.
        gui.pending.clear();
        draw(&mut gui);
        assert!(!gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobsLookup { .. }))), "kept for the page");
        // The gate closed meanwhile: the spot stays for the visit (spec
        // §3.2) — a press opens the window on its one plain line and Close.
        gui.app.keep.gate.plugins.as_mut().unwrap().jobs.allowed = false;
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 55, 9), "   ✓ kept");
        assert_eq!(gui.ui.hit(Position { x: 60, y: 2 }), Some(Act::KeepWord));
        key(&mut gui, KeyCode::Char('c'));
        let view = gui.app.keep_view().expect("the window, on its plain line");
        assert_eq!(view.stage, crate::tui::app::keep::KeepStage::Off(crate::tui::app::keep::OffWhy::Jobs));
        let window = draw(&mut gui).join("\n");
        assert!(window.contains("Your account can't start copies on this server") && window.contains("[ Close ]"), "{window}");
        key(&mut gui, KeyCode::Enter);
        assert!(gui.app.keep.flow.is_none(), "Enter closes it");
        // The next visit — its marks asked afresh (the holder went with the
        // page): the state plain — no click — the rows' marks kept, every
        // way in gone.
        gui.act(Act::Screen(super::super::Screen::Admin));
        draw(&mut gui);
        gui.act(Act::Screen(super::super::Screen::Library));
        draw(&mut gui);
        land_lookups(&mut gui, std::slice::from_ref(&kept));
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 55, 9), "   ✓ kept");
        assert_ne!(gui.ui.hit(Position { x: 60, y: 2 }), Some(Act::KeepWord));
        let y = shown.iter().position(|r| r.contains("Rooftop Cypher")).unwrap();
        assert_eq!(cells(&shown, y, 17, 1), "✓");
        key(&mut gui, KeyCode::Char('c'));
        assert!(gui.app.keep.flow.is_none(), "every way in gone");
    }

    #[test]
    fn a_live_album_copy_says_copying_in_the_header_and_leaves_the_rows_unmarked() {
        let _guard = english();
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        draw(&mut gui);
        let live = job(51, "album:boukmanflow|borrowed tapes", Some("album"), "running", B, Value::Null);
        land_lookups(&mut gui, &[live]);
        let shown = draw(&mut gui);
        assert_eq!(cells(&shown, 2, 55, 11), "   copying…");
        let y = shown.iter().position(|r| r.contains("Rooftop Cypher")).unwrap();
        assert_eq!(cells(&shown, y, 17, 2), "  ", "rows unmarked while the album copy runs");
        assert_eq!(gui.app.keep.watch, KeepWatch::Room, "the room's beat while a mark is live");
        // A room that asks no page's marks: the page's holder goes, and the
        // beat its live mark kept with it (spec §5.0).
        gui.active = super::super::DJ_NAV;
        draw(&mut gui);
        draw(&mut gui);
        assert_eq!(gui.app.keep.watch, KeepWatch::Away);
        assert!(!gui.app.keep_marks_live());
        gui.active = ALBUMS_NAV;
        // Another server's album copy never marks this server's page.
        let mut gui = peer_gui();
        album_page(&mut gui, tapes_node(AlbumCredit::Known("Boukmanflow".into())), tapes(Some("Boukmanflow")));
        draw(&mut gui);
        let theirs = job(52, "album:boukmanflow|borrowed tapes", Some("album"), "running", C, Value::Null);
        land_lookups(&mut gui, &[theirs]);
        let shown = draw(&mut gui);
        assert!(shown[2].contains("keep a copy"), "the word as it is: {}", shown[2]);
    }

    #[test]
    fn cards_wear_their_copy_in_the_year_line_and_folder_rows_their_folder_copy() {
        let _guard = english();
        let mut gui = peer_gui();
        artist_wall(&mut gui, "Boukmanflow", bouk_wall());
        draw(&mut gui);
        let kept = job(53, "album:boukmanflow|borrowed tapes", Some("album"), "done", B,
            json!({ "scope": "album", "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null }));
        let failed = job(54, "album:boukmanflow|second shift", Some("album"), "failed", B, Value::Null);
        land_lookups(&mut gui, &[kept, failed]);
        let shown = draw(&mut gui);
        let year_y = shown.iter().position(|r| r.contains("2021")).expect("the year line");
        assert_eq!(cells(&shown, year_y, 17, 6), "✓ kept");
        assert_eq!(cells(&shown, year_y, 31, 4), "2021", "no mark: the year returns");
        assert_eq!(cells(&shown, year_y, 45, 8), "! failed");
        // Folder rows: their folder copies alone, the glyph in the lead.
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        draw(&mut gui);
        let copying = job(55, "folder:peerlib/boukmanflow/borrowed tapes", Some("folder"), "running", B, Value::Null);
        let as_album = job(56, "album:boukmanflow|second shift", Some("album"), "done", B,
            json!({ "scope": "album", "counts": { "total": 3, "copied": 3, "skipped": 0, "failed": 0 }, "stopped": null }));
        land_lookups(&mut gui, &[copying, as_album]);
        let shown = draw(&mut gui);
        let row = |title: &str| shown.iter().position(|r| r.contains(title)).unwrap();
        assert_eq!(cells(&shown, row("Borrowed Tapes"), 17, 2), "↓ ");
        assert_eq!(cells(&shown, row("Second Shift"), 17, 2), "  ", "kept as an album: its card says so, not its row");
    }

    #[test]
    fn song_rows_wear_their_copy_in_recent_search_and_files_on_a_connected_server_only() {
        let _guard = english();
        let tracks = tapes(Some("Boukmanflow"));
        let kept = job(57, "text:late train verse", None, "done", C, json!({ "copied": { "filepath": "copies/x.mp3" } }));
        // Files' file rows: asked with the folder, the song's own copy — the
        // recording, from any server.
        let mut gui = peer_gui();
        files_at(&mut gui, "peerlib/Boukmanflow/Borrowed Tapes", vec![Entry::Parent, file_row(&tracks[0]), file_row(&tracks[1])]);
        draw(&mut gui);
        let recs = land_lookups(&mut gui, std::slice::from_ref(&kept));
        assert_eq!(recs.len(), 3, "the open folder and its two files, one lookup: {recs:?}");
        let shown = draw(&mut gui);
        let y = shown.iter().position(|r| r.contains("02 - Late Train Verse.mp3")).unwrap();
        assert_eq!(cells(&shown, y, 17, 1), "✓");
        // Recent.
        let mut gui = peer_gui();
        gui.active = RECENT_NAV;
        let asked = gui.app.open_library_node(LibraryNode::Recent, true);
        gui.pend(asked);
        let generation = gui.app.session_gen();
        apply(&mut gui, Event::Library { generation, dest: Tab::Library, node: LibraryNode::Recent, data: LibraryData::Tracks(tracks.clone()) });
        draw(&mut gui);
        land_lookups(&mut gui, std::slice::from_ref(&kept));
        let shown = draw(&mut gui);
        let y = shown.iter().position(|r| r.contains("Late Train Verse")).unwrap();
        assert_eq!(cells(&shown, y, 17, 1), "✓");
        // Search's song hits.
        let mut gui = peer_gui();
        gui.active = SEARCH_NAV;
        gui.app.tab = Tab::Search;
        gui.app.query = "late".into();
        gui.app.search_hits = Some(Box::default());
        gui.app.search.set(vec![Entry::Track { label: "Boukmanflow - Late Train Verse".into(), track: Box::new(tracks[1].clone()) }]);
        draw(&mut gui);
        land_lookups(&mut gui, std::slice::from_ref(&kept));
        let shown = draw(&mut gui);
        let y = shown.iter().position(|r| r.contains("Late Train Verse") && !r.contains('╭')).unwrap();
        assert_eq!(cells(&shown, y, 17, 1), "✓", "{}", shown.join("\n"));
        // The user's own server: no lookups, no marks.
        let mut gui = own_gui();
        files_at(&mut gui, "music", vec![file_row(&tracks[1])]);
        draw(&mut gui);
        assert!(!gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobsLookup { .. }))));
    }

    #[test]
    fn a_long_folder_scrolled_a_row_at_a_time_asks_its_marks_a_block_at_a_time() {
        let _guard = english();
        let mut gui = peer_gui();
        let rows: Vec<Entry> = std::iter::once(Entry::Parent)
            .chain((0..1000).map(|i| {
                let mut track = tapes(None)[0].clone();
                track.filepath = format!("peerlib/Boukmanflow/Big/{i:04}.mp3");
                file_row(&track)
            }))
            .collect();
        files_at(&mut gui, "peerlib/Boukmanflow/Big", rows);
        let mut sent = 0;
        for _ in 0..1001 {
            draw(&mut gui);
            sent += gui.pending.iter().filter(|e| matches!(e, Effect::Api(ApiCmd::JobsLookup { .. }))).count();
            land_lookups(&mut gui, &[]);
            key(&mut gui, KeyCode::Down);
        }
        assert_eq!(gui.app.files.state.selected(), Some(1000), "scrolled to the end");
        assert!(sent <= 11, "{sent} lookups for 1,000 rows");
    }

    #[test]
    fn a_page_asks_its_rows_once_and_again_only_when_they_change() {
        let _guard = english();
        let mut gui = peer_gui();
        bouk_folder(&mut gui);
        draw(&mut gui);
        land_lookups(&mut gui, &[]);
        for _ in 0..3 {
            draw(&mut gui);
        }
        assert!(!gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::JobsLookup { .. }))), "frames ask nothing new");
        // Another folder: its rows, one lookup.
        files_at(&mut gui, "peerlib/Color Out", vec![Entry::Parent, dir("peerlib/Color Out/Peer Sessions")]);
        draw(&mut gui);
        let recs = land_lookups(&mut gui, &[]);
        assert_eq!(recs.len(), 2);
    }

    // ── Tips (spec §6.9) ─────────────────────────────────────────────────

    #[test]
    fn the_tips_line_names_c_after_enter_where_it_keeps_the_held_row_within_its_cells() {
        let _guard = english();
        let mut gui = peer_gui();
        gui.config.gui.key_hints = true;
        bouk_folder(&mut gui);
        // Stowed with the word drawn: no segment.
        let shown = draw(&mut gui);
        assert!(!shown[29].contains("c keep a copy"), "{}", shown[29]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        let shown = draw(&mut gui);
        assert_eq!(shown[29].trim_end(), " ↑ ↓ move · Enter open/play · c keep a copy · h back · a queue · Tab queue · Space play/pause");
        // A copy moving: `C` (Copies, in every browse room) named where the
        // line has the cells, as the Discover room does — the stowed line's.
        key(&mut gui, KeyCode::Esc);
        let asked = gui.app.copies_open();
        gui.pend(asked);
        land_jobs(&mut gui, &[job(81, "folder:peerlib/boukmanflow/borrowed tapes", Some("folder"), "running", B, Value::Null)]);
        assert!(gui.app.keep_indicator().is_some());
        let shown = draw(&mut gui);
        assert!(shown[29].trim_end().ends_with(" · C copies"), "{}", shown[29]);
        assert!(crate::kit::width(shown[29].trim()) <= 99);
        // Every locale keeps the segment, and the line its 99 cells (the
        // words asked for by name).
        for code in LOCALES {
            let words = super::t!("gui.discover.keep.key_keep", locale = code).to_string();
            for line in ["gui.tips.files", "gui.tips.album_tracks", "gui.tips.albums", "gui.tips.search"] {
                let with = super::keep_segment(&super::t!(line, locale = code), &words);
                assert!(crate::kit::width(&with) <= 99, "{code} {line}: {with}");
                assert!(with.contains(&words), "{code} {line}: {with}");
            }
        }
    }

    // ── Words in every locale (spec §9, §10) ────────────────────────────

    #[test]
    fn the_header_word_is_each_locales_tips_segment_without_its_key_and_fits_its_spot() {
        // Each locale's words asked for by name: the process's locale is
        // left alone.
        for code in LOCALES {
            let t = |key: &str| super::t!(key, locale = code).to_string();
            let word = t("gui.discover.keep.key_keep");
            assert_ne!(word, "gui.discover.keep.key_keep", "{code}: the tips segment is missing");
            // Its key never translates, and its words are the segment the
            // locale's keep tips already say.
            assert!(word.starts_with("c "), "{code}: {word}");
            for line in ["gui.tips.discover_sheet_keep", "gui.tips.discover_tile_list_keep"] {
                assert!(t(line).contains(&format!(" · {word} · ")), "{code}: {word} is no segment of {}", t(line));
            }
            // The header's word is the segment's words, its key unnamed.
            let header = t("gui.discover.keep.keep_word");
            assert_eq!(Some(header.as_str()), word.strip_prefix("c "), "{code}: the header word");
            // §10: the widest is Spanish's and Portuguese's, 17 cells.
            assert!(super::cells(&header) <= 17, "{code}: {header} is {} cells", super::cells(&header));
            // "read-only" alone: the locale's words before the cut clause.
            let read_only = t("gui.srv.read_only");
            assert!(!read_only.contains('—') && !read_only.contains(" · "), "{code}: {read_only}");
            // The segment lands right after the line's Enter segment, so
            // every room line that takes it names Enter by that word.
            for key in ["gui.tips.files", "gui.tips.album_tracks", "gui.tips.albums", "gui.tips.search"] {
                let line = t(key);
                let enter = line.split(" · ").position(|part| part.starts_with("Enter"));
                let enter = enter.unwrap_or_else(|| panic!("{code}: {key} has no Enter segment: {line}"));
                let with = super::keep_segment(&line, &word);
                assert_eq!(with.split(" · ").nth(enter + 1), Some(word.as_str()), "{code} {key}: {with}");
            }
        }
    }
}
