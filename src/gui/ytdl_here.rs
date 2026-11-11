//! Add from YouTube in Files: the door word on an own-server folder's
//! browse bar, the 76-wide window it opens — Keep a copy's shape and
//! place — that adds a song right there (or into a subfolder of it), and,
//! once a job into the folder on screen settles, the folder listed again
//! with the new row under the cursor.
//!
//! Design: the door is the earlier canvas's TermFiles board (F1, the
//! spot at 80 and 47 cells); the window is the "Add from YouTube · GUI"
//! canvas's Files board. The window's form is the room's own [`Draft`],
//! read through [`Form::Window`], and it draws with the room's rows
//! ([`ytdl::form_rows`], [`ytdl::draw_row`]): the link's band with
//! `Paste`, the song's card, `Saves here` — whose sheet holds the
//! subfolder radios, for this window only (not remembered) — and
//! `Format`; the primary is a bracketed `[ Add here ]` — `[ Add to
//! library ]` past the folder itself — at the bottom right. It closes
//! once the server starts the job; a refusal stays inside.
//!
//! The door is drawn by [`draw_line`], one hook in `draw_files_bar`, so
//! the browse bar's own line one is untouched where the door is absent: at
//! a library root, on a peer, under the gates (hidden, never greyed),
//! while a window or the filter has the keys.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::kit::theme::th;
use crate::kit::{dim, width};
use crate::tui::app::{Effect, Focus};
use crate::tui::worker::{ApiCmd, Event};
use crate::ytdl::jobs::{JobState, Origin};
use crate::ytdl::{FolderSort, Target, YTDL_VARS, folder_target, layout_check, layout_problem, path_crumbs};

use super::keep;
use super::ytdl::{self, Draft, Form, Keys, Row, YtAct, yt};
use super::{Act, Gui, Screen, accent, bright_bold, put, ytdl_sheets};

/// The cells between the crumb and the door's word (the spot's gap).
const SPOT_GAP: u16 = 3;
/// The crumb clips no shorter while the word can still go.
const CRUMB_FLOOR: usize = 12;
/// `✓ added` holds the door's spot this long after the song landed; then
/// the door's own word is back.
const ADDED_SPOT_MS: i64 = 8_000;
/// A re-list whose listing never came is given up after this long.
const RELIST_MS: i64 = 30_000;
/// The door's and the window's controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HereAct {
    /// The door's word (or `y`): the window over the folder on screen.
    Open,
    /// `Esc close` on the border.
    Close,
    /// The `Saves here` row (`s`): its sheet, the subfolder radios.
    Subfolders,
    /// The sheet's `[ Done ]` (Enter, Esc, a click outside it).
    SheetDone,
    /// A radio: where under the folder the song goes.
    Sort(FolderSort),
    /// A click in Custom's field: it takes the keys.
    Custom,
    /// A variable under Custom's field: in at the caret.
    Var(usize),
}

fn here(act: HereAct) -> Act {
    yt(YtAct::Here(act))
}

/// The window while it is up.
#[derive(Debug, Clone)]
struct Window {
    /// The folder it adds into, as the browse bar's path ("Music/Daft
    /// Punk"): its jobs' origin.
    folder: String,
    /// The `Saves here` sheet is up: the radios, for this window only.
    sorts: bool,
    sort: FolderSort,
    /// Custom's layout, and whether its line editor has the keys.
    custom: Input,
    custom_keys: bool,
}

/// A folder asked again after a song landed in it: the row to seat once
/// its listing is in.
#[derive(Debug, Clone, PartialEq)]
struct Relist {
    folder: String,
    /// The new file or subfolder, picked up; `None`: a song this window
    /// didn't add — the folder lists again, the cursor back on the row it
    /// was on (`kept`), held or not as it was.
    name: Option<String>,
    kept: Option<String>,
    /// A listing of the folder came in since.
    landed: bool,
    asked: i64,
}

/// The door, the window and the re-list.
#[derive(Default)]
pub(crate) struct HereUi {
    /// The window's form (fresh at every open).
    pub(super) draft: Draft,
    window: Option<Window>,
    /// The origin the tracker was last told is showing.
    showing: Option<String>,
    relist: Option<Relist>,
}

// ── Where it stands ─────────────────────────────────────────────────────────

/// Files is on screen, with a session.
pub(super) fn in_files(gui: &Gui) -> bool {
    gui.screen == Screen::Library && gui.active == super::FILES_NAV && gui.app.connected
}

/// The browse bar's folder is below a library root on the session's own
/// server: a library root itself takes no song by this door.
fn folder_ok(gui: &Gui) -> bool {
    let path = gui.app.path.trim_matches('/');
    path.split_once('/')
        .is_some_and(|(library, rest)| !rest.is_empty() && gui.app.libraries.iter().any(|l| l == library))
}

/// The door works here: Files on an own-server folder below a library
/// root, under Add from YouTube's gates, nothing over it and the filter
/// not taking text (`y` types there).
pub(crate) fn door_live(gui: &Gui) -> bool {
    in_files(gui)
        && ytdl::available(gui)
        && folder_ok(gui)
        && !gui.modal_open()
        && !gui.servers.drop_open
        && !(gui.app.filtering && gui.browse_room())
}

pub(super) fn window_up(gui: &Gui) -> bool {
    gui.ytdl.here.window.is_some() && in_files(gui)
}

/// Another field of the window's took the keys: Custom's gives them up.
pub(super) fn keys_taken(gui: &mut Gui) {
    if let Some(w) = gui.ytdl.here.window.as_mut() {
        w.custom_keys = false;
    }
}

/// The surface the window's presses come from.
pub(super) fn origin(gui: &Gui) -> Origin {
    let folder = gui.ytdl.here.window.as_ref().map_or_else(|| gui.app.path.clone(), |w| w.folder.clone());
    Origin::Folder(folder)
}

/// Where the window's song lands: the folder on screen, or under it by
/// the radio's layout — or the folder Choose a folder chose.
pub(super) fn target(gui: &Gui) -> Target {
    let draft = &gui.ytdl.here.draft;
    let song = draft.tags().song();
    if let Some(dir) = &draft.instead {
        return folder_target(dir, None, &song);
    }
    let Some(w) = gui.ytdl.here.window.as_ref() else {
        return folder_target(&gui.app.path, None, &song);
    };
    let layout = match w.sort {
        FolderSort::Custom => Some(w.custom.value().trim()),
        sort => sort.layout(),
    };
    folder_target(&w.folder, layout, &song)
}

/// The target is the folder on screen itself: the primary reads Add here.
pub(super) fn is_here(gui: &Gui, target: &Target) -> bool {
    let folder = gui.ytdl.here.window.as_ref().map_or(gui.app.path.as_str(), |w| w.folder.as_str());
    lower(&target.crumbs()) == lower(&path_crumbs(folder))
}

fn lower(crumbs: &[String]) -> Vec<String> {
    crumbs.iter().map(|c| c.to_lowercase()).collect()
}

// ── Opening and closing ─────────────────────────────────────────────────────

/// The window over the folder on screen: empty, the link field holding
/// the keys (F2) — a paste is the way in.
pub(crate) fn open(gui: &mut Gui) {
    if !door_live(gui) {
        return;
    }
    let ui = &mut gui.ytdl.here;
    ui.draft = Draft::default();
    ui.window = Some(Window {
        folder: gui.app.path.trim_matches('/').to_string(),
        sorts: false,
        sort: FolderSort::Here,
        custom: Input::default(),
        custom_keys: false,
    });
    gui.ui.dismiss_tooltip();
    if let Some(jobs) = gui.ytdl.jobs_mut() {
        jobs.opened();
    }
}

/// The window goes (Esc, its job started, the folder or the gate gone): a
/// press still out goes on without it; a Format sheet opened from it goes
/// with it.
pub(super) fn close(gui: &mut Gui) {
    let ui = &mut gui.ytdl.here;
    if ui.window.take().is_some() {
        ui.draft = Draft::default();
        ytdl::drop_form_formats(gui);
    }
}

/// The window's turn, once a frame: it closes when what it stood over
/// went; its form's timers, and the folder its song lands in checked once
/// the tags rest; the tracker told what shows; a re-list seated. True
/// when anything shown changed.
pub(super) fn turn(gui: &mut Gui) -> bool {
    let mut changed = false;
    let stale = gui.ytdl.here.window.as_ref().is_some_and(|w| w.folder != gui.app.path.trim_matches('/'));
    if gui.ytdl.here.window.is_some() && (!in_files(gui) || !ytdl::available(gui) || stale) {
        close(gui);
        changed = true;
    }
    let now = gui.ytdl.now();
    let mut seq = gui.ytdl.seq;
    let (timed, work) = gui.ytdl.here.draft.timers(now, &mut seq);
    gui.ytdl.seq = seq;
    changed |= timed;
    if let Some(work) = work {
        ytdl::send(gui, work);
    }
    if window_up(gui) && (gui.ytdl.here.draft.found() || gui.ytdl.here.draft.manual) {
        let target = target(gui);
        let mut seq = gui.ytdl.seq;
        let work = gui.ytdl.here.draft.folder_work(&target, now, &mut seq);
        gui.ytdl.seq = seq;
        if let Some(work) = work {
            ytdl::send(gui, work);
            changed = true;
        }
        ytdl::want_thumb(gui, Form::Window);
    }
    // A start this window pressed that the server refuses is shown inside
    // while it is up (and leaves no row).
    let showing = window_up(gui).then(|| match origin(gui) {
        Origin::Folder(folder) => folder,
        Origin::Room => String::new(),
    });
    if showing != gui.ytdl.here.showing {
        let was = std::mem::replace(&mut gui.ytdl.here.showing, showing.clone());
        if let Some(jobs) = gui.ytdl.jobs_mut() {
            if let Some(folder) = was {
                jobs.set_showing(Origin::Folder(folder), false);
            }
            if let Some(folder) = showing {
                jobs.set_showing(Origin::Folder(folder), true);
            }
        }
    }
    changed | seat(gui, now)
}

// ── The re-list ─────────────────────────────────────────────────────────────

/// A job settled: a song that landed in the folder Files shows, or below
/// it, lists the folder again (F4) — the new row (or its new subfolder)
/// under the cursor once the listing is in, where this folder's window
/// added it. Not while a filter stands: the listing would wipe it.
pub(super) fn settled(gui: &mut Gui, job: &str, state: JobState) {
    if state != JobState::Added || !in_files(gui) || gui.app.session.peer.is_some() {
        return;
    }
    let Some(jobs) = gui.ytdl.jobs() else { return };
    let Some(job_row) = jobs.job(job) else { return };
    let shown = gui.app.path.trim_matches('/').to_string();
    let landed = path_crumbs(&job_row.folder());
    let here = path_crumbs(&shown);
    if here.is_empty() || landed.len() < here.len() || lower(&landed[..here.len()]) != lower(&here) {
        return;
    }
    if !gui.app.files.filter.is_empty() || gui.app.filtering {
        return;
    }
    let mine = jobs.origin_of(job) == Some(&Origin::Folder(shown.clone()));
    let name = mine.then(|| match landed.get(here.len()) {
        Some(subfolder) => subfolder.clone(),
        None => job_row
            .file_path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map_or_else(|| job_row.file_name.clone(), str::to_string),
    });
    let kept = gui.app.files.selected().map(|e| e.label().to_string());
    // Asked as the App asks, its spinner left alone: the rows stay on
    // screen until the new ones replace them.
    gui.pending.push(Effect::Api(ApiCmd::Browse(shown.clone())));
    let asked = gui.ytdl.now();
    gui.ytdl.here.relist = Some(Relist { folder: shown, name, kept, landed: false, asked });
}

/// The event loop's hook, before the App takes the event: the re-list's
/// folder listing on its way in.
pub(super) fn observe(gui: &mut Gui, event: &Event) {
    let Event::Listing(listing) = event else { return };
    if let Some(relist) = gui.ytdl.here.relist.as_mut()
        && listing.path.trim_matches('/') == relist.folder
    {
        relist.landed = true;
    }
}

/// The listing is in: the cursor on the new row, lit and held (the
/// list-cursor law's exception: the row the add made) — or, for a song
/// this window didn't add, back on the row it was on (the listing rests
/// it on the first). True when it moved.
fn seat(gui: &mut Gui, now: i64) -> bool {
    let Some(relist) = gui.ytdl.here.relist.clone() else { return false };
    if gui.app.path.trim_matches('/') != relist.folder || now - relist.asked > RELIST_MS {
        gui.ytdl.here.relist = None;
        return false;
    }
    if !relist.landed || gui.app.files.loading {
        return false;
    }
    gui.ytdl.here.relist = None;
    let find = |name: &str| {
        let wanted = name.to_lowercase();
        gui.app.files.entries.iter().position(|e| e.label().to_lowercase() == wanted)
    };
    match (relist.name.as_deref().map(find), relist.kept.as_deref().map(find)) {
        (Some(Some(i)), _) => {
            gui.app.focus = Focus::Browser;
            gui.app.files.state.select(Some(i));
            gui.files_view.pick_up();
        }
        (None, Some(Some(i))) => gui.app.files.state.select(Some(i)),
        _ => {}
    }
    true
}

// ── The door ────────────────────────────────────────────────────────────────

/// What the door's spot says: its own word, or the job this folder's
/// window started, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Door,
    Adding,
    Added,
    Failed,
}

fn spot(gui: &Gui) -> Spot {
    let Some(jobs) = gui.ytdl.jobs() else { return Spot::Door };
    let Some(job) = jobs.latest_from(&Origin::Folder(gui.app.path.trim_matches('/').to_string())) else {
        return Spot::Door;
    };
    let now = gui.ytdl.now();
    match job.state {
        _ if job.hidden => Spot::Door,
        state if state.open() => Spot::Adding,
        JobState::Added if job.ended_at.is_some_and(|at| now - at < ADDED_SPOT_MS) => Spot::Added,
        state if state.needs_look() && !job.seen => Spot::Failed,
        _ => Spot::Door,
    }
}

/// The spot's word, its token, and what a click on it does: the door's
/// word opens the window, `! failed` the room, where its row is; the
/// job's other words only say.
fn spot_words(spot: Spot) -> (String, Style, Option<Act>) {
    match spot {
        Spot::Door => (t!("gui.ytdl.door").to_string(), accent(), Some(here(HereAct::Open))),
        Spot::Adding => (t!("gui.ytdl.word.adding").to_string(), accent(), None),
        Spot::Added => {
            (format!("{} {}", keep::tick(), t!("gui.ytdl.word.added")), Style::default().fg(th().ok), None)
        }
        Spot::Failed => {
            (format!("! {}", t!("gui.ytdl.word.failed")), Style::default().fg(th().gold), Some(yt(YtAct::List)))
        }
    }
}

/// Line one of the Files bar with the door (F1, Row 2): back ◂, the crumb,
/// the spot's word three cells after it, the count at the right. Short of
/// room the count goes first, then the crumb clips from the left, then
/// the word — `y` still works. False where the door is absent or gave
/// way: the bar draws its own line one.
pub(super) fn draw_line(frame: &mut Frame, gui: &mut Gui, content: Rect) -> bool {
    if !door_live(gui) {
        return false;
    }
    let y = content.y;
    let (word, style, act) = spot_words(spot(gui));
    let count = super::bar_count(gui, super::discover::plural("gui.files.items", gui.app.files.counts().1 as u64));
    let count_x = super::right_at(content, &count);
    // The crumb starts past the back ◂ (a folder below a root has one).
    let x = content.x + 2;
    let crumb = format!("{} {} {}", t!("gui.nav.files"), super::forward_glyph(), gui.app.path);
    let word_w = width(&word) as u16;
    let after = x + width(&crumb) as u16 + SPOT_GAP;
    let (shown, word_x) = if after + word_w + 2 <= count_x {
        put(frame, count_x, y, &count, dim());
        (crumb, after)
    } else {
        let word_x = after.min(super::right_at(content, &word));
        let budget = usize::from(word_x.saturating_sub(x + SPOT_GAP));
        if budget < CRUMB_FLOOR.min(width(&crumb)) {
            return false;
        }
        (super::clip_lead(&crumb, budget), word_x)
    };
    super::draw_bar_back(frame, gui, content.x, y);
    put(frame, x, y, &shown, dim());
    let rect = Rect { x: word_x, y, width: word_w, height: 1 };
    let lit = act.is_some() && gui.ui.hovers(rect);
    put(frame, word_x, y, &word, if lit { bright_bold() } else { style });
    if let Some(act) = act {
        gui.ui.click(rect, act);
    }
    true
}

// ── The window ──────────────────────────────────────────────────────────────

/// The Saves here sheet is up over the window.
pub(super) fn sheet_up(gui: &Gui) -> bool {
    window_up(gui) && gui.ytdl.here.window.as_ref().is_some_and(|w| w.sorts)
}

/// The window (the canvas's Files board): 76 wide, banded in the room or
/// centred over the frame, its title on the top border and `Esc close` on
/// the bottom; inside, the room's own form — the link's band, the song's
/// card, `Saves here` and `Format` — and one bracketed primary at the
/// bottom right. Then the sheet `Saves here` opens, while it is up.
pub(super) fn draw_window(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    gui.ytdl.drawn.clear();
    let room = super::content_rect(area.width, area.height, gui.queue_open, gui.footer());
    let view = ytdl::form_view(gui, Form::Window);
    let text = usize::from(keep::KEEP_W.saturating_sub(4));
    let mut rows = ytdl::form_rows(gui, Form::Window, &view, text);
    // The blank under the link is the room's: here the song sits right
    // under the card.
    if matches!(rows.first(), Some(Row::Blank)) {
        rows.remove(0);
    }
    let above = ytdl::above_rows(&view, text);
    let action = if view.primary.is_some() { 2 } else { 0 };
    // From the top it anchors at down to the room's last row, rising
    // toward row 1 only as far as it must (the window's own rule): short
    // of that the blanks give way from the bottom up — never the place or
    // the primary.
    let most = usize::from(room.bottom().saturating_sub(area.y + 1).saturating_sub(2));
    while 3 + rows.len() + above.len() + action > most {
        let Some(at) = rows.iter().rposition(|row| matches!(row, Row::Blank)) else { break };
        rows.remove(at);
    }
    let total = 3 + rows.len() + above.len() + action;
    let (rect, band) = keep::window_rect(area, room, keep::KEEP_W, total as u16 + 2);
    let (inner, own) = keep::open_window(frame, gui, area, rect, band);
    keep::draw_borders(frame, gui, rect, &t!("gui.ytdl.room.title"), &t!("gui.discover.keep.esc_close"), here(HereAct::Close));
    // The form's block: two cells in from each border.
    let block = Rect { x: rect.x + 2, y: inner.y, width: rect.width.saturating_sub(4), height: inner.height };
    let bottom = inner.bottom();
    // A press out: the controls stand, and do nothing.
    gui.ytdl.quiet = gui.ytdl.here.draft.starting.is_some();
    let keys = gui.ytdl.here.draft.keys == Keys::Link && !gui.ytdl.quiet && !sheet_up(gui);
    let mut y = inner.y;
    ytdl::draw_link_band(frame, gui, Form::Window, Rect { x: block.x, y, width: block.width, height: 3 }, keys);
    y += 3;
    let mut cover = None;
    for row in rows {
        if y >= bottom {
            break;
        }
        if matches!(row, Row::Card(0) | Row::Reading(0)) && y + ytdl::COVER_H <= bottom {
            cover = Some(Rect { x: block.x, y, width: ytdl::COVER_W, height: ytdl::COVER_H });
        }
        ytdl::draw_row(frame, gui, Form::Window, &view, block, y, row);
        y += 1;
    }
    if let Some(rect) = cover {
        ytdl::draw_cover(frame, gui, Form::Window, rect, Some(own));
    }
    // The server's refusal while the window stays (the 409 too), or why
    // the primary is off.
    for (words, style, right) in above {
        if y >= bottom {
            break;
        }
        let words = super::bar::clip(&words, text);
        let at = if right { block.right().saturating_sub(width(&words) as u16) } else { block.x };
        put(frame, at, y, &words, style);
        y += 1;
    }
    if let Some((label, on)) = view.primary.clone().filter(|_| y + 1 < bottom) {
        draw_primary(frame, gui, block, y + 1, &label, on);
    }
    gui.ytdl.quiet = false;
    if sheet_up(gui) {
        draw_sheet(frame, gui, area, &view);
    }
}

/// The one bracketed primary at the window's bottom right: ACCENT BOLD,
/// BRIGHT BOLD under the pointer; off, DIM, no click — the row above it
/// says why.
fn draw_primary(frame: &mut Frame, gui: &mut Gui, block: Rect, y: u16, label: &str, on: bool) {
    let w = width(label) as u16;
    let at = Rect { x: block.right().saturating_sub(w), y, width: w, height: 1 };
    let quiet = gui.ytdl.quiet;
    let style = if !on || quiet {
        dim()
    } else if gui.ui.hovers(at) {
        bright_bold()
    } else {
        accent().add_modifier(Modifier::BOLD)
    };
    put(frame, at.x, y, label, style);
    if on && !quiet {
        gui.ui.click(at, yt(YtAct::Add));
    }
}

/// The sheet `Saves here` opens: where under the folder on screen the
/// song goes — right here, or in folders its tags name — for this window
/// only, never remembered. The place it makes on the title's own row;
/// the radios in two columns; Custom's field a card with its variables
/// under it; `[ Done ]`.
fn draw_sheet(frame: &mut Frame, gui: &mut Gui, area: Rect, view: &ytdl::RoomView) {
    let Some(w) = gui.ytdl.here.window.clone() else { return };
    let wide = ytdl_sheets::sheet_room(gui, area);
    let custom = w.sort == FolderSort::Custom;
    let typed = w.custom.value();
    let problem: Vec<String> = (custom && !typed.trim().is_empty())
        .then(|| layout_problem(&layout_check(typed)))
        .flatten()
        .map(|p| ytdl::gold_rows(&p.text(), wide))
        .unwrap_or_default();
    let radios = FolderSort::ALL.len().div_ceil(2) as u16;
    let field = if custom { 1 + 3 + problem.len() as u16 + 1 } else { 0 };
    let rect = ytdl_sheets::open_sheet(frame, gui, area, radios + field + 2, here(HereAct::SheetDone));
    let title = t!(if is_here(gui, &view.target) { "gui.ytdl.here.saves_here" } else { "gui.ytdl.room.saves_to" }).to_string();
    ytdl_sheets::sheet_title(frame, rect, &title, |frame, x, room| {
        if view.target.is_valid() {
            ytdl::draw_place(frame, x, rect.y, &view.target.crumbs(), view.new_folder, room)
        } else {
            let dash = super::discover::dash();
            put(frame, x, rect.y, dash, dim());
            width(dash) as u16
        }
    });
    let (x, total) = ytdl_sheets::sheet_text(rect);
    let end = x + total;
    let half = total / 2 + 1;
    let (dot_on, dot_off) = ytdl::dots();
    let mut y = rect.y + 1;
    for pair in FolderSort::ALL.chunks(2) {
        for (sort, at) in pair.iter().zip([x, x + half]) {
            let on = w.sort == *sort;
            let cells = if at == x { half.saturating_sub(2) } else { end.saturating_sub(at) };
            let pick = Rect { x: at, y, width: cells, height: 1 };
            put(frame, at, y, if on { dot_on } else { dot_off }, if on { accent() } else { dim() });
            let style = match (gui.ui.hovers(pick), on) {
                (true, _) => bright_bold(),
                (false, true) => Style::default().add_modifier(Modifier::BOLD),
                (false, false) => Style::default(),
            };
            put(frame, at + 4, y, &super::bar::clip(&sort.label().text(), usize::from(cells.saturating_sub(4))), style);
            gui.ui.click(pick, here(HereAct::Sort(*sort)));
        }
        y += 1;
    }
    if custom {
        let card = Rect { x, y: y + 1, width: total, height: 3 };
        ytdl_sheets::field_card(frame, gui, card, w.custom.value(), w.custom.cursor(), w.custom_keys, here(HereAct::Custom));
        y += 4;
        for line in &problem {
            put(frame, x, y, &super::bar::clip(line, wide), Style::default().fg(th().gold));
            y += 1;
        }
        let mut at = x;
        for (i, name) in YTDL_VARS.iter().enumerate() {
            if i > 0 {
                put(frame, at, y, " · ", dim());
                at += 3;
            }
            let cell = Rect { x: at, y, width: width(name) as u16, height: 1 };
            put(frame, at, y, name, if gui.ui.hovers(cell) { bright_bold() } else { dim() });
            gui.ui.click(cell, here(HereAct::Var(i)));
            at += cell.width;
        }
        y += 1;
    }
    ytdl_sheets::bracket_button(frame, gui, rect.right().saturating_sub(3), y + 1, &t!("gui.ytdl.done"), true, here(HereAct::SheetDone));
}

// ── Acts and keys ───────────────────────────────────────────────────────────

pub(super) fn act(gui: &mut Gui, act: HereAct) {
    match act {
        HereAct::Open => open(gui),
        HereAct::Close => close(gui),
        // The rest is the form's: nothing while its press is out.
        _ if gui.ytdl.here.draft.starting.is_some() => {}
        HereAct::Subfolders => {
            if let Some(w) = gui.ytdl.here.window.as_mut() {
                w.sorts = true;
                w.custom_keys = w.sort == FolderSort::Custom;
                gui.ytdl.here.draft.keys = Keys::None;
            }
        }
        HereAct::SheetDone => {
            if let Some(w) = gui.ytdl.here.window.as_mut() {
                w.sorts = false;
                w.custom_keys = false;
            }
        }
        HereAct::Sort(sort) => pick_sort(gui, sort),
        HereAct::Custom => {
            if let Some(w) = gui.ytdl.here.window.as_mut() {
                w.custom_keys = true;
                gui.ytdl.here.draft.keys = Keys::None;
            }
        }
        HereAct::Var(i) => {
            let Some(name) = YTDL_VARS.get(i) else { return };
            let token = format!("{{{{{name}}}}}");
            let Some(w) = gui.ytdl.here.window.as_mut() else { return };
            let chars: Vec<char> = w.custom.value().chars().collect();
            let at = w.custom.cursor().min(chars.len());
            let value: String = chars[..at].iter().copied().chain(token.chars()).chain(chars[at..].iter().copied()).collect();
            w.custom = Input::new(value).with_cursor(at + token.chars().count());
            w.custom_keys = true;
            gui.ytdl.here.draft.keys = Keys::None;
            gui.ytdl.here.draft.instead = None;
        }
    }
}

/// A radio picked: the song goes there; Custom's field takes the keys.
fn pick_sort(gui: &mut Gui, sort: FolderSort) {
    let ui = &mut gui.ytdl.here;
    let Some(w) = ui.window.as_mut() else { return };
    w.sort = sort;
    w.custom_keys = sort == FolderSort::Custom;
    ui.draft.instead = None;
    if w.custom_keys {
        ui.draft.keys = Keys::None;
    }
}

/// The dot one radio on (`delta` > 0) or back, clamped at the ends.
fn step_sort(gui: &mut Gui, delta: i32) {
    let Some(w) = gui.ytdl.here.window.as_ref() else { return };
    let at = FolderSort::ALL.iter().position(|s| *s == w.sort).unwrap_or(0) as i32;
    let next = (at + delta).clamp(0, FolderSort::ALL.len() as i32 - 1) as usize;
    pick_sort(gui, FolderSort::ALL[next]);
}

/// The Saves here sheet's keys: ↑ ↓ or ← → move the dot (onto Custom,
/// its field takes the keys — Enter or Esc gives them back, ↑ walks back
/// up); Enter or Esc closes the sheet.
fn sheet_key(gui: &mut Gui, key: KeyEvent) {
    let custom = gui.ytdl.here.window.as_ref().is_some_and(|w| w.custom_keys);
    if custom {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => keys_taken(gui),
            KeyCode::Up | KeyCode::BackTab => {
                keys_taken(gui);
                step_sort(gui, -1);
            }
            _ => {
                let Some(w) = gui.ytdl.here.window.as_mut() else { return };
                let changed = w.custom.handle_event(&TermEvent::Key(key)).is_some_and(|c| c.value);
                if changed {
                    gui.ytdl.here.draft.instead = None;
                }
            }
        }
        return;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Enter => act(gui, HereAct::SheetDone),
        KeyCode::Left | KeyCode::Up => step_sort(gui, -1),
        KeyCode::Right | KeyCode::Down => step_sort(gui, 1),
        _ => {}
    }
}

/// The window's keys: it owns them all while it is up. Esc closes it —
/// from the link and with the keys free. While its press is out only Esc
/// acts, and the press still goes through.
pub(super) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let now = gui.ytdl.now();
    if gui.ytdl.here.draft.starting.is_some() {
        if key.code == KeyCode::Esc {
            close(gui);
        }
        return false;
    }
    if sheet_up(gui) {
        sheet_key(gui, key);
        return false;
    }
    match gui.ytdl.here.draft.keys {
        Keys::Link => match key.code {
            KeyCode::Esc => close(gui),
            KeyCode::Enter => ytdl::link_enter(gui, Form::Window),
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {}
            _ => ytdl::link_typed(gui, Form::Window, key, now),
        },
        _ => match key.code {
            KeyCode::Esc => close(gui),
            KeyCode::Enter => {
                let on = ytdl::form_view(gui, Form::Window).primary.is_some_and(|(_, on)| on);
                let draft = &gui.ytdl.here.draft;
                let idle = draft.parsed.kind == crate::ytdl::LinkKind::Video && matches!(draft.look, ytdl::Look::Idle);
                if on {
                    ytdl::press(gui);
                } else if idle {
                    ytdl::ask(gui);
                }
            }
            // A key acts only where its control stood; a capital is the
            // window's too.
            KeyCode::Char(c) if !ctrl => {
                let lower = c.to_ascii_lowercase();
                let drawn = gui.ytdl.drawn.iter().find(|(letter, _)| *letter == lower).map(|(_, act)| act.clone());
                if let Some(act) = drawn {
                    return gui.act(yt(act));
                }
            }
            _ => {}
        },
    }
    false
}

/// The tips line while the window is up: Enter's primary first, the
/// controls the frame drew by their key words, the paste, the way out —
/// the radios' dot alone while the sheet is up.
pub(super) fn tips(gui: &Gui) -> String {
    let close = t!("gui.tips.ytdl_close").to_string();
    if gui.ytdl.here.draft.starting.is_some() {
        return close;
    }
    if gui.ytdl.here.window.as_ref().is_some_and(|w| w.custom_keys) {
        return t!("gui.tips.ytdl_where_layout").to_string();
    }
    if sheet_up(gui) {
        return format!("{} · {close}", t!("gui.tips.ytdl_here_dot"));
    }
    if gui.ytdl.here.draft.keys == Keys::Link {
        return t!("gui.tips.ytdl_here_field").to_string();
    }
    let view = ytdl::form_view(gui, Form::Window);
    let enter = view.primary.as_ref().is_some_and(|(_, on)| *on).then(|| {
        let key = if view.anyway {
            "gui.tips.ytdl_enter_anyway"
        } else if is_here(gui, &view.target) {
            "gui.tips.ytdl_enter_here"
        } else {
            "gui.tips.ytdl_enter_library"
        };
        t!(key).to_string()
    });
    let mut keys: Vec<String> = Vec::new();
    for (letter, act) in &gui.ytdl.drawn {
        let words = match act {
            YtAct::Edit => t!("gui.ytdl.key.edit"),
            YtAct::Format => t!("gui.tips.ytdl_format"),
            YtAct::Retry => t!("gui.ytdl.key.retry"),
            YtAct::Type => t!("gui.ytdl.key.type"),
            YtAct::Use => t!("gui.ytdl.key.use"),
            YtAct::Undo => t!("gui.ytdl.key.undo"),
            YtAct::Choose => t!("gui.ytdl.key.choose"),
            YtAct::Here(HereAct::Subfolders) => t!("gui.ytdl.key.subfolders"),
            _ => continue,
        };
        if words.starts_with(*letter) && !keys.iter().any(|k| k.as_str() == words) {
            keys.push(words.to_string());
        }
    }
    let tail = [t!("gui.tips.ytdl_paste").to_string(), close];
    let line = |keys: &[String]| enter.iter().chain(keys).chain(tail.iter()).cloned().collect::<Vec<_>>().join(" · ");
    // At most the footer's 99 cells: the last key words give way.
    while !keys.is_empty() && width(&line(&keys)) > 99 {
        keys.pop();
    }
    line(&keys)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::api::types::{DirEntry, DirListing, FileEntry, FileMetadata, Track, TrackMetadata};
    use crate::api::ytdl::{YtdlAddedRead, YtdlEntry, YtdlRecordPage, YtdlStarted};
    use crate::tui::app::{Entry, Tab};
    use crate::ytdl::jobs::{Answer, Call};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use super::super::ytdl::tests::{
        Cells, LINK, START, WATCH, advance, answer_folders, answer_lookup, assert_frame, at, buffer, english, key, lines, one_more_time, seated, serve, shift, type_text,
    };

    const SONGS: [(&str, f64); 3] =
        [("Aerodynamic.mp3", 207.0), ("Digital_Love.mp3", 298.0), ("Harder_Better_Faster_Stronger.mp3", 224.0)];

    fn track(name: &str, seconds: f64) -> Entry {
        Entry::Track {
            label: name.into(),
            track: Box::new(Track {
                filepath: format!("Music/Daft Punk/{name}"),
                metadata: TrackMetadata { duration: Some(seconds), ..TrackMetadata::default() },
            }),
        }
    }

    /// Files on Music › Daft Punk with F1's rows, Digital Love playing, on a
    /// server of `generation` — no window yet.
    pub(crate) fn files_gui(generation: u32) -> Gui {
        let mut gui = seated(generation);
        gui.active = super::super::FILES_NAV;
        gui.app.tab = Tab::Files;
        gui.app.path = "Music/Daft Punk".into();
        let mut entries = vec![
            Entry::Parent,
            Entry::Dir { label: "Homework".into(), path: "Music/Daft Punk/Homework".into() },
            Entry::Dir { label: "Human After All".into(), path: "Music/Daft Punk/Human After All".into() },
        ];
        entries.extend(SONGS.iter().map(|(name, s)| track(name, *s)));
        gui.app.files.set(entries);
        gui.app.now_playing =
            Some(Track { filepath: "Music/Daft Punk/Digital_Love.mp3".into(), metadata: TrackMetadata::default() });
        ytdl::poll(&mut gui);
        gui
    }

    /// The folder's listing as the server sends it, with `more` rows.
    fn listing(more_dirs: &[&str], more_files: &[(&str, f64)]) -> DirListing {
        let dirs = ["Homework", "Human After All"].iter().chain(more_dirs);
        let files = SONGS.iter().chain(more_files);
        DirListing {
            path: "/Music/Daft Punk/".into(),
            directories: dirs.map(|d| DirEntry { name: d.to_string() }).collect(),
            files: files
                .map(|(name, s)| FileEntry {
                    name: name.to_string(),
                    kind: Some("mp3".into()),
                    metadata: Some(FileMetadata {
                        filepath: format!("Music/Daft Punk/{name}"),
                        metadata: Some(TrackMetadata { duration: Some(*s), ..TrackMetadata::default() }),
                    }),
                })
                .collect(),
        }
    }

    /// The listing on its way in, as the event loop takes it: the hook, the
    /// App, the pumps.
    fn land(gui: &mut Gui, listing: DirListing) {
        ytdl::observe(gui, &Event::Listing(Box::new(listing.clone())));
        let effects = gui.app.apply_event(Event::Listing(Box::new(listing)));
        gui.pend(effects);
        ytdl::poll(gui);
    }

    /// The window open, a link pasted and found, the folder there with F1's
    /// files and `more` (F3).
    fn found_here(gui: &mut Gui, more: &[&str]) {
        key(gui, KeyCode::Char('y'));
        ytdl::paste(gui, LINK);
        answer_lookup(gui, Ok(one_more_time()));
        let mut files: Vec<&str> = SONGS.iter().map(|(n, _)| *n).collect();
        files.extend(more);
        answer_folders(gui, there_files("/Music/Daft Punk/", &files));
        // A key acts where its word was drawn.
        buffer(gui, 100, 30);
    }

    fn there_files(directory: &str, files: &[&str]) -> crate::ytdl::FolderCheck {
        super::super::ytdl::tests::there(directory, files)
    }

    /// A new server taking the add into `directory`: the reads before the
    /// POST, the POST's id, then the entry as `status` ("complete": in the
    /// folder as One_More_Time.mp3).
    fn server(directory: &'static str, status: Option<&'static str>) -> impl FnMut(&Call) -> Option<Answer> {
        follow(directory, status, false)
    }

    /// [`server`] for an add the server took earlier (`posted`).
    fn follow(directory: &'static str, status: Option<&'static str>, posted: bool) -> impl FnMut(&Call) -> Option<Answer> {
        let mut posted = posted;
        move |call| match call {
            Call::Mkdir { .. } => Some(Answer::Mkdir(Ok(()))),
            Call::Records { .. } => Some(Answer::Records(Ok(Some(YtdlRecordPage { limit: 1, ..Default::default() })))),
            Call::Jobs if !posted => Some(Answer::Jobs(Ok(Vec::new()))),
            Call::Jobs => status.map(|status| {
                Answer::Jobs(Ok(vec![YtdlEntry {
                    id: Some("srv1".into()),
                    url: WATCH.into(),
                    directory: directory.into(),
                    output_codec: "mp3".into(),
                    status: status.into(),
                    start_time: Some(START),
                    end_time: Some(START + 60_000),
                    file_name: Some("One_More_Time.mp3".into()),
                    file_path: (status == "complete")
                        .then(|| format!("{}One_More_Time.mp3", directory.trim_start_matches('/'))),
                    error: (status == "error").then(|| "Sign in to confirm you’re not a bot.".into()),
                    ..Default::default()
                }]))
            }),
            Call::Add(_) => {
                posted = true;
                Some(Answer::Add(Ok(YtdlStarted {
                    message: Some("Download started".into()),
                    id: Some("srv1".into()),
                    file_name: Some("One_More_Time.mp3".into()),
                })))
            }
            Call::Added { .. } => Some(Answer::Added(Ok(Some(YtdlAddedRead { end: true, ..Default::default() })))),
            _ => None,
        }
    }

    /// The typing rests: a moved place's folder check goes out.
    fn settle(gui: &mut Gui) {
        ytdl::poll(gui);
        advance(gui, 500);
    }

    fn pointer_at(gui: &mut Gui, x: u16, y: u16) {
        gui.ui.pointer = Some(Position { x, y });
    }

    fn browsed(gui: &Gui) -> Vec<String> {
        gui.pending
            .iter()
            .filter_map(|e| match e {
                Effect::Api(ApiCmd::Browse(path)) => Some(path.clone()),
                _ => None,
            })
            .collect()
    }

    mod boards {
        pub(super) const F1_BAR: &str = r#"
            |◂ Files ▸ Music/Daft Punk   add from YouTube                            5 items
            |/ filter                                       ▸ play · + queue all · ⇄ shuffle
        "#;
        pub(super) const F1_BAR_RUNS: &[&str] =
            &["0:0-0:Dim", "0:2-24:Dim", "0:28-43:Accent", "0:72-78:Dim", "1:0-7:Dim", "1:47-52:Accent", "1:54-78:Dim"];
        pub(super) const SPOT_HOVER: &str = r#"
            |◂ Files ▸ Music/Daft Punk   add from YouTube                            5 items
        "#;
        pub(super) const SPOT_ADDING: &str = r#"
            |◂ Files ▸ Music/Daft Punk   adding                                      5 items
        "#;
        pub(super) const SPOT_ADDED: &str = r#"
            |◂ Files ▸ Music/Daft Punk   ✓ added                                     6 items
        "#;
        pub(super) const SPOT_FAILED: &str = r#"
            |◂ Files ▸ Music/Daft Punk   ! failed                                    5 items
        "#;
        pub(super) const NARROW: &str = r#"
            |◂ Files ▸ Music/Daft Punk   add from YouTube
        "#;
        pub(super) const NARROW_DEEP: &str = r#"
            |◂ …Music/Daft Punk/Homework   add from YouTube
        "#;
        pub(super) const F2: &str = r#"
            |╭─ Add from YouTube ───────────────────────────────────────────────────────╮
            |│ ╭──────────────────────────────────────────────────────────────────────╮ │
            |│ │ ▏Paste a YouTube link                                                │ │
            |│ ╰──────────────────────────────────────────────────────────────────────╯ │
            |│                                                                          │
            |│ Saves here                                           Music › Daft Punk ▸ │
            |│ Format                                                             MP3 ▸ │
            |╰────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const F2_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-18:Text:B", "0:20-75:Accent", "1:0-75:Accent", "2:0-2:Accent", "2:4-4:Text",
            "2:5-24:Dim", "2:73-75:Accent", "3:0-75:Accent", "4:0-75:Accent", "5:0-0:Accent", "5:2-59:Text",
            "5:61-61:Dim", "5:63-71:Text", "5:73-73:Dim", "5:75-75:Accent", "6:0-0:Accent", "6:2-7:Text",
            "6:69-73:Dim", "6:75-75:Accent", "7:0-62:Accent", "7:64-72:Dim", "7:74-75:Accent",
        ];
        pub(super) const SHEET: &str = r#"
            |╭─ Saves to · Music › Daft Punk › Discovery new ───────────────────────────╮
            |│ ( ) Right here                       (•) Album                           │
            |│ ( ) Artist / Album                   ( ) Custom                          │
            |│                                                                          │
            |│                                                                [ Done ]  │
            |╰──────────────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const SHEET_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-10:Text:B", "0:12-12:Dim", "0:14-18:Text", "0:20-20:Dim", "0:22-30:Text",
            "0:32-32:Dim", "0:34-42:Text", "0:44-46:Dim", "0:48-75:Accent", "1:0-0:Accent", "1:2-4:Dim",
            "1:6-15:Text", "1:39-41:Accent", "1:43-47:Text:B", "1:75-75:Accent", "2:0-0:Accent", "2:2-4:Dim",
            "2:6-19:Text", "2:39-41:Dim", "2:43-48:Text", "2:75-75:Accent", "3:0-75:Accent", "4:0-0:Accent",
            "4:65-72:Accent:B", "4:75-75:Accent", "5:0-75:Accent",
        ];
        pub(super) const FILES: &str = r#"
            |╭─ Add from YouTube ───────────────────────────────────────────────────────╮
            |│ ╭────────────────────────────────────────────────────────────╮ ╭───────╮ │
            |│ │ https://youtu.be/FGBhQbmPwH8                           [X] │ │ Paste │ │
            |│ ╰────────────────────────────────────────────────────────────╯ ╰───────╯ │
            |│ ╭──────╮ One More Time                                       edit tags ▸ │
            |│ │      │ Daft Punk                                                       │
            |│ │      │ Discovery · 2001                                                │
            |│ ╰──────╯ YouTube · Daft Punk · 5:20                                      │
            |│ Tags from YouTube Music                                                  │
            |│                                                                          │
            |│ Saves here                                           Music › Daft Punk ▸ │
            |│ Format                                                             MP3 ▸ │
            |│                                                                          │
            |│                                                             [ Add here ] │
            |╰────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const FILES_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-18:Text:B", "0:20-75:Accent", "1:0-0:Accent", "1:2-73:Dim", "1:75-75:Accent",
            "2:0-0:Accent", "2:2-2:Dim", "2:4-31:Text", "2:59-73:Dim", "2:75-75:Accent", "3:0-0:Accent",
            "3:2-73:Dim", "3:75-75:Accent", "4:0-0:Accent", "4:2-9:Dim", "4:11-23:Text:B", "4:63-73:Dim",
            "4:75-75:Accent", "5:0-0:Accent", "5:2-9:Dim", "5:11-19:Text", "5:75-75:Accent", "6:0-0:Accent",
            "6:2-26:Dim", "6:75-75:Accent", "7:0-0:Accent", "7:2-36:Dim", "7:75-75:Accent", "8:0-0:Accent",
            "8:2-24:Dim", "8:75-75:Accent", "9:0-75:Accent", "10:0-0:Accent", "10:2-59:Text", "10:61-61:Dim",
            "10:63-71:Text", "10:73-73:Dim", "10:75-75:Accent", "11:0-0:Accent", "11:2-7:Text", "11:69-73:Dim",
            "11:75-75:Accent", "12:0-75:Accent", "13:0-0:Accent", "13:62-73:Accent:B", "13:75-75:Accent",
            "14:0-62:Accent", "14:64-72:Dim", "14:74-75:Accent",
        ];
        pub(super) const F4: &str = r#"
            |◂ Files ▸ Music/Daft Punk   ✓ added                                     6 items
            |/ filter                                       ▸ play · + queue all · ⇄ shuffle
            |
            |  ..
            |  Homework
            |  Human After All
            |  Aerodynamic.mp3                                                        3:27
            |▸ Digital_Love.mp3                                                       4:58
            |  Harder_Better_Faster_Stronger.mp3                                      3:44
            |  One_More_Time.mp3                                                      5:20
            |
        "#;
        pub(super) const F4_RUNS: &[&str] = &[
            "0:0-0:Dim", "0:2-24:Dim", "0:28-34:Ok", "0:72-78:Dim", "1:0-7:Dim", "1:47-52:Accent", "1:54-78:Dim",
            "3:2-3:Dim", "4:2-9:Text", "5:2-16:Text", "6:2-16:Text", "6:73-76:Dim", "7:0-0:Ok:B", "7:2-17:Ok:B",
            "7:73-76:Dim", "8:2-34:Text", "8:73-76:Dim", "9:2-18:OnAccent:B", "9:73-76:OnAccent",
        ];
        pub(super) const F4_NOTE: &str = r#"
            |Added “One More Time”
        "#;
    }

    /// The window's 76 × rows at (x, y) of a 100 × `h` frame.
    fn assert_window(gui: &mut Gui, (w, h): (u16, u16), (x, y): (u16, u16), rows: &str, runs: &[&str]) -> Buffer {
        let b = buffer(gui, w, h);
        assert_frame(&b, (x, y), 76, rows, runs);
        b
    }

    // ── The door ───────────────────────────────────────────────────────────

    /// F1 and Row 2's hover: the door word ACCENT three cells after the
    /// crumb, no letter on it, BRIGHT BOLD under the pointer; it and `y`
    /// open the window.
    #[test]
    fn the_door_word_stands_three_cells_after_the_crumb() {
        let _en = english();
        let mut gui = files_gui(1);
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::F1_BAR, boards::F1_BAR_RUNS);
        assert_eq!(gui.ui.hit(Position { x: 45, y: 2 }), Some(here(HereAct::Open)));
        assert_eq!(gui.ui.hit(Position { x: 60, y: 2 }), Some(here(HereAct::Open)));
        assert_ne!(gui.ui.hit(Position { x: 61, y: 2 }), Some(here(HereAct::Open)), "the word's cells alone");
        pointer_at(&mut gui, 50, 2);
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::SPOT_HOVER, &["0:28-43:Bright:B"]);
        gui.act(here(HereAct::Open));
        assert!(window_up(&gui), "a click opens the window");
        close(&mut gui);
        gui.ui.pointer = None;
        key(&mut gui, KeyCode::Char('y'));
        assert!(window_up(&gui), "and so does y, with no letter on screen");
        assert_eq!(gui.ytdl.here.draft.keys, Keys::Link, "the link field holds the keys");
    }

    /// Absent, never greyed: at a library root, on a peer, with uploads or
    /// conversion off, while the filter takes text — and `y` is no key then
    /// (the filter's own letter while it types).
    #[test]
    fn the_door_is_absent_where_it_cant_work() {
        let _en = english();
        let plain = |gui: &mut Gui| {
            let rows = lines(&buffer(gui, 100, 30));
            assert!(!rows[2].contains("add from YouTube"), "no door:\n{}", rows[2]);
            key(gui, KeyCode::Char('y'));
            assert!(!window_up(gui), "y opens nothing");
        };
        let mut gui = files_gui(1);
        gui.app.path = "Music".into();
        plain(&mut gui);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows[2].contains("◂ Files ▸ Music") && rows[2].contains("5 items"), "the bar as it was:\n{}", rows[2]);

        let mut gui = files_gui(1);
        gui.app.session.peer = Some(("Friend".into(), 7));
        plain(&mut gui);

        for ping in [
            crate::api::types::Ping { no_upload: Some(true), ..super::super::ytdl::tests::ping(1) },
            crate::api::types::Ping { transcode: None, ..super::super::ytdl::tests::ping(1) },
        ] {
            let mut gui = files_gui(1);
            ytdl::observe(&mut gui, &super::super::ytdl::tests::connected(ping));
            plain(&mut gui);
        }

        let mut gui = files_gui(1);
        gui.act(Act::BarFilter);
        assert!(gui.app.filtering);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(!rows[2].contains("add from YouTube"), "the filter types: no door\n{}", rows[2]);
        key(&mut gui, KeyCode::Char('y'));
        assert!(!window_up(&gui));
        assert_eq!(gui.app.pane().filter, "y", "y typed into the filter");
    }

    /// Row 2 at 47 cells (the queue open): the count goes first, then the
    /// crumb clips from the left — the word ending one cell in — and past a
    /// floor the word goes too, `y` still opening the window.
    #[test]
    fn the_spot_gives_way_at_forty_seven_cells() {
        let _en = english();
        let mut gui = files_gui(1);
        gui.queue_open = true;
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 47, boards::NARROW, &["0:0-0:Dim", "0:2-24:Dim", "0:28-43:Accent"]);
        gui.app.path = "Music/Daft Punk/Homework".into();
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 47, boards::NARROW_DEEP, &["0:0-0:Dim", "0:2-26:Dim", "0:30-45:Accent"]);
        // Narrower than the floor (a long word in a narrow column): the
        // word gives way, the bar is its own — and y still works.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        let content = Rect { x: 17, y: 2, width: 30, height: 20 };
        let drawn = std::cell::Cell::new(true);
        terminal.draw(|frame| drawn.set(draw_line(frame, &mut gui, content))).unwrap();
        assert!(!drawn.get(), "past the floor the word goes");
        key(&mut gui, KeyCode::Char('y'));
        assert!(window_up(&gui));
    }

    /// Row 2's states: while this folder's job runs the spot says `adding`
    /// (ACCENT), then `✓ added` (OK) a while, then the door's word again;
    /// a failure says `! failed` (GOLD) and a click on it opens the From
    /// YouTube list over Files. `y` opens the window under every word.
    #[test]
    fn the_spot_carries_the_job_in_one_word() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        let mut reply = server("/Music/Daft Punk/", None);
        serve(&mut gui, &mut reply);
        assert!(!window_up(&gui), "the server took it: the window went");
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::SPOT_ADDING, &["0:0-0:Dim", "0:2-24:Dim", "0:28-33:Accent", "0:72-78:Dim"]);
        assert_eq!(gui.ui.hit(Position { x: 46, y: 2 }), None, "adding only says");
        key(&mut gui, KeyCode::Char('y'));
        assert!(window_up(&gui), "y opens the window under adding");
        close(&mut gui);

        let mut reply = follow("/Music/Daft Punk/", Some("complete"), true);
        serve(&mut gui, &mut reply);
        land(&mut gui, listing(&[], &[("One_More_Time.mp3", 320.0)]));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::SPOT_ADDED, &["0:0-0:Dim", "0:2-24:Dim", "0:28-34:Ok", "0:72-78:Dim"]);
        advance(&gui, ADDED_SPOT_MS);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows[2].contains("add from YouTube"), "then the door's word again:\n{}", rows[2]);

        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        let mut reply = server("/Music/Daft Punk/", Some("error"));
        serve(&mut gui, &mut reply);
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::SPOT_FAILED, &["0:0-0:Dim", "0:2-24:Dim", "0:28-35:Gold", "0:72-78:Dim"]);
        assert_eq!(gui.ui.hit(Position { x: 47, y: 2 }), Some(yt(YtAct::List)));
        // `! failed` opens the room, where the row and its ways on are.
        gui.act(yt(YtAct::List));
        assert!(ytdl::in_room(&gui), "the room: its list has the row");
        ytdl::poll(&mut gui);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("! One More Time") && r.contains("failed")), "{}", rows.join("\n"));
        gui.act(Act::Nav(super::super::FILES_NAV));
        ytdl::poll(&mut gui);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows[2].contains("add from YouTube"), "seen in the room: the door's word again\n{}", rows[2]);
    }

    // ── The window ─────────────────────────────────────────────────────────

    /// Dump the window's frames for a look (`cargo test dump_files_window -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn dump_files_window() {
        let _en = english();
        let show = |name: &str, gui: &mut Gui, h: u16| {
            eprintln!("== {name}");
            for (y, row) in lines(&buffer(gui, 100, h)).iter().enumerate() {
                eprintln!("{y:2} |{}", row.trim_end());
            }
        };
        let mut gui = files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        show("empty", &mut gui, 30);
        eprintln!("{}", super::super::ytdl::tests::board_of(&buffer(&mut gui, 100, 30), (19, 6), (76, 8), "F2"));
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        show("found", &mut gui, 30);
        show("floor", &mut gui, 24);
        key(&mut gui, KeyCode::Char('s'));
        key(&mut gui, KeyCode::Right);
        settle(&mut gui);
        let check = crate::ytdl::FolderCheck { known: true, new_segments: vec![1], deepest: Some("/Music/Daft Punk/".into()), files: None };
        answer_folders(&mut gui, check);
        show("sheet", &mut gui, 30);
        eprintln!("{}", super::super::ytdl::tests::board_of(&buffer(&mut gui, 100, 30), (19, 12), (76, 6), "SHEET"));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        show("custom", &mut gui, 30);
    }

    /// Empty: 76 wide, banded at x 19–94 from row 6; the link's card with
    /// the caret and the placeholder, where the song will save and as
    /// what; the bar's word steps aside while it is up; the room beneath
    /// inert. Typing goes to the field — `q` doesn't quit — and Esc closes.
    #[test]
    fn the_window_opens_empty_with_the_field_holding_the_keys() {
        let _en = english();
        let mut gui = files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        let b = assert_window(&mut gui, (100, 30), (19, 6), boards::F2, boards::F2_RUNS);
        let rows = lines(&b);
        assert!(rows[2].contains("◂ Files ▸ Music/Daft Punk") && rows[2].ends_with("5 items    "), "{}", rows[2]);
        assert!(!rows[2].contains("add from YouTube"), "the word steps aside");
        // The band clears the room's width on the window's rows.
        for y in 6..=13 {
            for x in [17, 18, 95, 96] {
                assert_eq!(b[(x, y)].symbol(), " ", "the band at ({x}, {y})");
            }
        }
        assert_eq!(gui.ui.hit(Position { x: 30, y: 4 }), Some(Act::Guard), "beneath is inert");
        assert_eq!(gui.ui.hit(at(&rows, "Esc close")), Some(here(HereAct::Close)), "Esc close on the border");
        assert_eq!(gui.ui.hit(at(&rows, "Saves here")), Some(yt(YtAct::Here(HereAct::Subfolders))), "the rows work before a link");
        assert_eq!(ytdl::tips(&gui), "a paste fills the link and looks it up · typing works too · Esc close");
        assert!(ytdl::wants_paste(&gui), "bracketed paste while the window is up");
        assert!(!super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert_eq!(gui.ytdl.here.draft.link.value(), "q", "q is the field's");
        key(&mut gui, KeyCode::Esc);
        assert!(!window_up(&gui), "Esc closes");
        assert!(!ytdl::wants_paste(&gui));
    }

    /// Files (the canvas's board): over the queue's 47-cell room the
    /// window centres on the frame at x 12–87 — the room's own form inside
    /// it: the link's band with `Paste`, the song's card, `Saves here` and
    /// `Format`, one bracketed primary at the bottom right. Every control
    /// clicks, and the tips name their keys.
    #[test]
    fn a_found_song_is_the_files_board() {
        let _en = english();
        let mut gui = files_gui(1);
        gui.queue_open = true;
        gui.ytdl.clipboard = Some(|| Some(LINK.to_string()));
        found_here(&mut gui, &[]);
        assert_eq!(gui.ytdl.here.draft.keys, Keys::None);
        let b = assert_window(&mut gui, (100, 30), (12, 6), boards::FILES, boards::FILES_RUNS);
        let rows = lines(&b);
        for (text, act) in [
            ("edit tags ▸", YtAct::Edit),
            ("Saves here", YtAct::Here(HereAct::Subfolders)),
            ("Music › Daft Punk ▸", YtAct::Here(HereAct::Subfolders)),
            ("Format", YtAct::Format),
            ("[X]", YtAct::Clear),
            ("│ Paste │", YtAct::PasteClip),
            ("[ Add here ]", YtAct::Add),
        ] {
            assert_eq!(gui.ui.hit(at(&rows, text)), Some(yt(act)), "{text}");
        }
        assert_eq!(gui.ui.hit(Position { x: 30, y: 8 }), Some(yt(YtAct::Field)), "the link's card takes the keys");
        assert_eq!(
            ytdl::tips(&gui),
            "Enter add here · e edit tags · s subfolders · f format · a paste replaces the link · Esc close"
        );
        // `Paste` pastes into the window's own link.
        gui.act(yt(YtAct::Clear));
        gui.act(yt(YtAct::PasteClip));
        assert_eq!((gui.ytdl.here.draft.link.value(), gui.ytdl.draft.link.value()), (LINK, ""));
    }

    /// With the queue folded the window is banded in the room at x 19–94;
    /// at the 100 × 24 floor it stands on rows 3–17, nothing given way.
    /// In a terminal — no clipboard to read — the link's card is the
    /// window's whole width.
    #[test]
    fn the_window_in_the_room_and_at_the_floor() {
        let _en = english();
        let mut gui = files_gui(1);
        gui.ytdl.clipboard = Some(|| None);
        found_here(&mut gui, &[]);
        assert_window(&mut gui, (100, 30), (19, 6), boards::FILES, boards::FILES_RUNS);
        assert_window(&mut gui, (100, 24), (19, 3), boards::FILES, boards::FILES_RUNS);
        gui.ytdl.clipboard = None;
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows[7].starts_with_at(19, &format!("│ ╭{}╮ │", "─".repeat(70))), "{}", rows.join("\n"));
        assert!(!rows.join("\n").contains("Paste"));
    }

    /// `Saves here` (or `s`) opens its sheet: where under the folder the
    /// song goes, as radios in two columns — for this window only. ↑ ↓ or
    /// ← → move the dot; the place follows on the sheet's title and the
    /// window's row; past Right here the primary reads Add to library.
    /// Custom's field is a card that takes the keys, the variables go in
    /// at its caret, a refused layout says so in GOLD. Enter or Esc closes
    /// the sheet; then Enter adds.
    #[test]
    fn saves_here_opens_its_sheet() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Char('s'));
        assert!(sheet_up(&gui) && ytdl::modal_open(&gui));
        assert!(!ytdl::wants_paste(&gui), "a paste is the sheet's field's now, typed");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert_eq!(ytdl::tips(&gui), "↑ ↓ or ← → move the dot · Esc close");
        key(&mut gui, KeyCode::Right);
        settle(&mut gui);
        let check = crate::ytdl::FolderCheck {
            known: true,
            new_segments: vec![1],
            deepest: Some("/Music/Daft Punk/".into()),
            files: None,
        };
        answer_folders(&mut gui, check.clone());
        let b = buffer(&mut gui, 100, 30);
        // The window's own columns: nothing of it shows cut beside the sheet.
        assert_frame(&b, (19, 12), 76, boards::SHEET, boards::SHEET_RUNS);
        let _ = rows;
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Custom")), Some(here(HereAct::Sort(FolderSort::Custom))));
        assert_eq!(gui.ui.hit(at(&rows, "[ Done ]")), Some(here(HereAct::SheetDone)));
        assert_eq!(gui.ui.hit(Position { x: 5, y: 3 }), Some(here(HereAct::SheetDone)), "a click outside closes it");
        assert!(rows[19].contains("[ Add to library ]"), "the window's primary follows, beneath:\n{}", rows.join("\n"));

        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        assert!(gui.ytdl.here.window.as_ref().is_some_and(|w| w.sort == FolderSort::Custom && w.custom_keys), "Custom's field took the keys");
        gui.act(here(HereAct::Var(3)));
        type_text(&mut gui, " - ");
        gui.act(here(HereAct::Var(2)));
        settle(&mut gui);
        answer_folders(&mut gui, crate::ytdl::FolderCheck { new_segments: vec![1], ..check });
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        assert!(rows[9].starts_with_at(19, "╭─ Saves to · Music › Daft Punk › 2001 - Discovery new ─"), "{text}");
        assert!(rows[14].starts_with_at(21, "│ {{YEAR}} - {{ALBUM}}▏") && b[(21, 13)].fg == th().accent, "the field's card, with the keys:\n{text}");
        assert!(rows[16].starts_with_at(21, "ARTIST · ALBUMARTIST · ALBUM · YEAR"), "{text}");
        assert_eq!(gui.ui.hit(at(&rows, "ALBUMARTIST")), Some(here(HereAct::Var(1))));
        assert_eq!(ytdl::tips(&gui), "type · Enter or Esc gives the keys back");
        // A refused layout: its GOLD line under the field, the place a dash.
        let held = gui.ytdl.here.window.as_ref().unwrap().custom.clone();
        type_text(&mut gui, "/{{TITLE}}");
        let rows = lines(&buffer(&mut gui, 100, 30));
        let text = rows.join("\n");
        assert!(text.contains("! Unknown variable {{TITLE}}. Use ARTIST, ALBUMARTIST, ALBUM,") && text.contains("╭─ Saves to · — ─"), "{text}");
        gui.ytdl.here.window.as_mut().unwrap().custom = held;
        key(&mut gui, KeyCode::Esc);
        assert!(sheet_up(&gui), "Esc gave the keys back; the sheet stays");
        key(&mut gui, KeyCode::Esc);
        assert!(!sheet_up(&gui) && window_up(&gui), "then closes the sheet, not the window");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains("Music › Daft Punk › 2001 - Discovery new ▸")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Enter);
        assert!(gui.ytdl.here.draft.starting.is_some(), "then Enter adds");
        let req = ytdl::tests::calls(&gui);
        assert!(req.iter().any(|c| matches!(c, Call::Mkdir { directory } if directory == "/Music/Daft Punk/2001 - Discovery/")), "{req:?}");
        // Not remembered: the next window opens on Right here, its sheet shut.
        close(&mut gui);
        key(&mut gui, KeyCode::Char('y'));
        let w = gui.ytdl.here.window.as_ref().unwrap();
        assert!(!w.sorts && w.sort == FolderSort::Here);
    }

    /// Edit tags and Format open over the window as they do over the room,
    /// for the window's own song; Esc closes the sheet first, then the
    /// window.
    #[test]
    fn the_sheets_stand_over_the_window() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Char('e'));
        assert!(ytdl::tags_up(&gui) && window_up(&gui));
        let rows = lines(&buffer(&mut gui, 100, 30));
        let text = rows.join("\n");
        assert!(rows[7].starts_with_at(19, "╭─ Edit tags ─") && rows[6].contains("─ Add from YouTube ─"), "over the window, on its columns:\n{text}");
        assert!(rows[7].ends_with_at(95, "─╮") && rows[9].starts_with_at(21, &format!("╭{}╮", "─".repeat(70))), "the title's card its width:\n{text}");
        type_text(&mut gui, "!");
        assert_eq!((gui.ytdl.here.draft.tags().title.as_str(), gui.ytdl.draft.tags().title.as_str()), ("One More Time!", ""), "the window's song");
        key(&mut gui, KeyCode::Esc);
        assert!(!ytdl::tags_up(&gui) && window_up(&gui), "Esc closes the sheet first");
        buffer(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('f'));
        assert!(ytdl::formats_up(&gui));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!((gui.ytdl.here.draft.codec.as_deref(), gui.ytdl.draft.codec.as_deref()), (Some("m4a"), None));
        assert!(lines(&buffer(&mut gui, 100, 30))[17].contains("M4A ▸"), "the window's row follows");
        key(&mut gui, KeyCode::Esc);
        assert!(!window_up(&gui));
    }

    /// A predicted clash — GOLD under the rows, the primary Add anyway;
    /// the server's 409 refusal is shown inside, the window staying.
    #[test]
    fn a_clash_and_the_servers_refusal_stay_inside() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &["One_More_Time.mp3"]);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        let clash = at(&rows, "! Something like “One_More_Time.mp3” looks to be in this folder already.");
        assert_eq!((clash.x, clash.y, b[(clash.x, clash.y)].fg), (21, 18, th().gold), "under the two rows:\n{text}");
        assert_eq!(gui.ui.hit(at(&rows, "[ Add anyway ]")), Some(yt(YtAct::Add)));
        assert_eq!(ytdl::tips(&gui), "Enter add anyway · e edit tags · s subfolders · f format · a paste replaces the link · Esc close");
        key(&mut gui, KeyCode::Enter);
        let mut reply = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Err(crate::api::ytdl::YtdlCallError::Answered {
                status: 409,
                words: Some("exists".into()),
                reason: Some("exists".into()),
                file_name: Some("One_More_Time.mp3".into()),
            }))),
            Call::Jobs => Some(Answer::Jobs(Ok(Vec::new()))),
            Call::Records { .. } => Some(Answer::Records(Ok(Some(YtdlRecordPage { limit: 1, ..Default::default() })))),
            _ => None,
        };
        serve(&mut gui, &mut reply);
        assert!(window_up(&gui), "refused: the window stays");
        assert!(gui.ytdl.here.draft.starting.is_none());
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(
            rows.iter().any(|r| r.contains("! “One_More_Time.mp3” is already in this folder, so nothing was added.")),
            "the refusal inside:\n{}",
            rows.join("\n")
        );
        assert!(!rows.join("\n").contains("looks to be in this folder"), "the warning it confirms is gone");
        assert!(gui.ytdl.jobs().unwrap().jobs().is_empty(), "no row is left");
    }

    /// The Format sheet opened from the window goes with the window (here
    /// a re-ping saying uploads are off), never left over Files to take a
    /// pick for the room's form.
    #[test]
    fn the_format_sheet_goes_with_the_window() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        buffer(&mut gui, 100, 30);
        gui.act(yt(YtAct::Format));
        assert!(gui.ytdl.formats.is_some());
        let ping = super::super::ytdl::tests::ping;
        ytdl::observe(&mut gui, &super::super::ytdl::tests::connected(crate::api::types::Ping { no_upload: Some(true), ..ping(1) }));
        ytdl::poll(&mut gui);
        assert!(!window_up(&gui), "the gate closed the window");
        assert!(gui.ytdl.formats.is_none(), "and the sheet over it");
        let codec = gui.ytdl.draft.codec.clone();
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.ytdl.draft.codec, codec, "the room's format untouched");
    }

    /// Enter pressed — nothing moves, only the primary goes off: DIM, no
    /// click and no hover, the controls do nothing; Esc closes and the
    /// press still goes through, its word on the door.
    #[test]
    fn the_press_goes_quiet_and_esc_lets_it_through() {
        let _en = english();
        let mut gui = files_gui(1);
        gui.ytdl.clipboard = Some(|| Some(LINK.to_string()));
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        pointer_at(&mut gui, 85, 19);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let primary = at(&rows, "[ Adding… ]");
        assert_eq!((primary.x + 11, primary.y, b[(primary.x, primary.y)].fg), (93, 19, th().dim), "{}", rows.join("\n"));
        for text in ["[ Adding… ]", "edit tags ▸", "Saves here", "Format", "[X]", "│ Paste │"] {
            assert_eq!(gui.ui.hit(at(&rows, text)), Some(Act::Guard), "{text} does nothing");
        }
        assert_eq!(gui.ui.hit(at(&rows, "Esc close")), Some(here(HereAct::Close)));
        assert_eq!(ytdl::tips(&gui), "Esc close");
        key(&mut gui, KeyCode::Char('e'));
        assert!(!gui.ytdl.here.draft.tags_open, "keys wait for the server");
        shift(&mut gui, 'S');
        assert!(!sheet_up(&gui));
        key(&mut gui, KeyCode::Esc);
        assert!(!window_up(&gui), "Esc closes");
        let mut reply = server("/Music/Daft Punk/", None);
        serve(&mut gui, &mut reply);
        let job = gui.ytdl.jobs().unwrap().jobs()[0].clone();
        assert_eq!(job.state, JobState::Adding, "the press went through");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows[2].contains("   adding"), "{}", rows[2]);
        assert!(rows[29].contains("Adding “One More Time”…"), "{}", rows[29]);
    }

    /// F4: once the song landed the folder lists again — the rows stay up
    /// meanwhile — and the cursor rests on the new row, lit; the note line
    /// says it; a song in a new subfolder seats on the subfolder.
    #[test]
    fn a_landed_song_relists_the_folder_with_its_row_under_the_cursor() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        gui.pending.clear();
        let mut reply = server("/Music/Daft Punk/", Some("complete"));
        serve(&mut gui, &mut reply);
        assert_eq!(browsed(&gui), vec!["Music/Daft Punk".to_string()], "the folder asked again");
        assert!(!gui.app.files.loading, "the rows stay until the new ones come");
        land(&mut gui, listing(&[], &[("One_More_Time.mp3", 320.0)]));
        let b = buffer(&mut gui, 100, 30);
        assert_frame(&b, (17, 2), 80, boards::F4, boards::F4_RUNS);
        assert_frame(&b, (24, 29), 30, boards::F4_NOTE, &["0:0-20:Ok"]);
        assert!(gui.files_view.held, "↑ ↓ walk from the new row");

        // Into a subfolder: the cursor on the folder it made.
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Char('s'));
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Enter);
        settle(&mut gui);
        answer_folders(
            &mut gui,
            crate::ytdl::FolderCheck { known: true, new_segments: vec![1], deepest: Some("/Music/Daft Punk/".into()), files: None },
        );
        key(&mut gui, KeyCode::Enter);
        let mut reply = server("/Music/Daft Punk/Discovery/", Some("complete"));
        serve(&mut gui, &mut reply);
        land(&mut gui, listing(&["Discovery"], &[]));
        let selected = gui.app.files.state.selected().and_then(|i| gui.app.files.entries.get(i)).map(|e| e.label().to_string());
        assert_eq!(selected.as_deref(), Some("Discovery"));

        // A song the room added into the folder on screen: it lists again,
        // the cursor back where it was, not picked up.
        let mut gui = files_gui(1);
        gui.app.files.state.select(Some(2));
        let mut theirs = super::super::ytdl::tests::job("r1", "adding", "One More Time", "/Music/Daft Punk/");
        theirs["serverId"] = "srv1".into();
        super::super::ytdl::tests::seed(&mut gui, vec![theirs]);
        gui.pending.clear();
        let mut reply = follow("/Music/Daft Punk/", Some("complete"), true);
        serve(&mut gui, &mut reply);
        assert_eq!(browsed(&gui), vec!["Music/Daft Punk".to_string()]);
        land(&mut gui, listing(&[], &[("One_More_Time.mp3", 320.0)]));
        let selected = gui.app.files.selected().map(|e| e.label().to_string());
        assert_eq!(selected.as_deref(), Some("Human After All"));
        assert!(!gui.files_view.held);

        // A listing for a folder since left seats nothing.
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        let mut reply = server("/Music/Daft Punk/", Some("complete"));
        serve(&mut gui, &mut reply);
        gui.app.path = "Music".into();
        ytdl::poll(&mut gui);
        assert!(gui.ytdl.here.relist.is_none());
    }

    /// The room's other stages read the same inside the window: the
    /// lookup out (ACCENT, beside the cover's slot), a server that can't
    /// be reached (GOLD, `try again`), and why the primary is off on the
    /// row above it (DIM).
    #[test]
    fn the_rooms_other_stages_read_the_same_in_the_window() {
        let _en = english();
        let mut gui = files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        ytdl::paste(&mut gui, LINK);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let reading = at(&rows, "Reading the video…");
        assert_eq!((reading.x, reading.y), (30, 10), "right under the link's card, past the slot:\n{}", rows.join("\n"));
        assert_eq!(b[(reading.x, reading.y)].fg, th().accent);
        assert!(rows[10].starts_with_at(21, "╭──────╮") && rows[15].starts_with_at(21, "Saves here"), "{}", rows.join("\n"));
        assert_eq!(gui.ui.hit(at(&rows, "[ Add here ]")), Some(Act::Guard), "the primary is there, and off");

        let refused = crate::api::ytdl::YtdlCallError::Unanswered {
            why: crate::api::ytdl::NoAnswer::Refused,
            detail: String::new(),
        };
        answer_lookup(&mut gui, Err(refused));
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let gold = at(&rows, "! Can't reach Home server.");
        assert_eq!((gold.y, b[(gold.x, gold.y)].fg), (10, th().gold), "{}", rows.join("\n"));
        let again = rows.iter().position(|r| r.starts_with_at(21, "try again")).expect("its way on, on a row of its own");
        assert_eq!(gui.ui.hit(Position { x: 22, y: again as u16 }), Some(yt(YtAct::Retry)));
        assert!(!rows.join("\n").contains("[ Add"), "no primary");

        // Already adding the same link to this folder: the primary off, why
        // on the row above it.
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        key(&mut gui, KeyCode::Enter);
        let mut reply = server("/Music/Daft Punk/", None);
        serve(&mut gui, &mut reply);
        found_here(&mut gui, &[]);
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let why = at(&rows, "Already adding to this folder");
        assert_eq!((why.x + 29, b[(why.x, why.y)].fg), (93, th().dim), "ending at the block's edge, DIM");
        let primary = at(&rows, "[ Add here ]");
        assert_eq!((primary.y, b[(primary.x, primary.y)].fg), (why.y + 2, th().dim), "off");
        assert_ne!(gui.ui.hit(primary), Some(yt(YtAct::Add)), "no click");
        key(&mut gui, KeyCode::Enter);
        assert_eq!(gui.ytdl.jobs().unwrap().jobs().len(), 1, "never twice for the same link and folder");
    }

    /// The window goes with what it stood over: Files left, or the gate
    /// closing under it (hidden, never greyed).
    #[test]
    fn the_window_closes_when_files_goes() {
        let _en = english();
        let mut gui = files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        gui.act(Act::Nav(super::super::SETTINGS_NAV));
        ytdl::poll(&mut gui);
        assert!(gui.ytdl.here.window.is_none());
        let mut gui = files_gui(1);
        key(&mut gui, KeyCode::Char('y'));
        let off = crate::api::types::Ping { no_upload: Some(true), ..super::super::ytdl::tests::ping(1) };
        ytdl::observe(&mut gui, &super::super::ytdl::tests::connected(off));
        ytdl::poll(&mut gui);
        assert!(gui.ytdl.here.window.is_none());
    }

    /// The window keeps every key while it is up: D, T and A — with Shift
    /// or Caps Lock — neither open Auto DJ or Stats nor toggle the DJ, and
    /// a capital of a control it drew is that control's (S: Saves here).
    #[test]
    fn the_windows_capitals_stay_in_it() {
        let _en = english();
        let mut gui = files_gui(1);
        found_here(&mut gui, &[]);
        let armed = gui.app.dj_armed();
        for c in ['D', 'T', 'A'] {
            for modifiers in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
                super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char(c), modifiers));
                assert!(window_up(&gui), "{c} ({modifiers:?}) kept the window up");
                assert_eq!((gui.screen, gui.active), (Screen::Library, super::super::FILES_NAV), "{c}");
                assert_eq!(gui.app.dj_armed(), armed, "{c}");
                buffer(&mut gui, 100, 30);
            }
        }
        super::super::handle_key(&mut gui, KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
        assert!(sheet_up(&gui), "S is the Saves here row");
    }

    /// Every language's door words where the browse bar's give-way still
    /// draws them at 47 cells (TermFiles row 2): past the back ◂, the
    /// crumb's floor and the spot's gap, the word ending one cell in.
    #[test]
    fn every_locales_door_words_fit_the_bar_at_forty_seven_cells() {
        for code in super::super::ytdl::tests::LOCALES {
            let t = |key: &str| rust_i18n::t!(key, locale = code).to_string();
            for word in [
                t("gui.ytdl.door"),
                t("gui.ytdl.word.adding"),
                format!("{} {}", keep::tick(), t("gui.ytdl.word.added")),
                format!("! {}", t("gui.ytdl.word.failed")),
            ] {
                let cells = 2 + CRUMB_FLOOR + usize::from(SPOT_GAP) + width(&word) + 1;
                assert!(cells <= 47, "{code}: {word} needs {cells} cells");
            }
        }
    }

    /// Every language drawn in the window, then its sheet: the primary,
    /// the card's control, the two rows' labels and the four radios whole,
    /// the tips line in the footer's 99 with Enter and Esc kept.
    #[test]
    fn every_locale_draws_the_window_whole() {
        let Some(code) = crate::setup::tests::in_own_process(&super::super::ytdl::tests::LOCALES) else { return };
        let whole = super::super::ytdl::tests::whole;
        let mut gui = files_gui(1);
        gui.ytdl.clipboard = Some(|| None);
        found_here(&mut gui, &[]);
        let rows = lines(&buffer(&mut gui, 100, 30));
        let primary = format!("[ {} ]", t!("gui.ytdl.add.here"));
        let edit = ytdl::page_words(&t!("gui.ytdl.key.edit")).1.to_string();
        for words in [primary, edit, t!("gui.ytdl.here.saves_here").to_string(), t!("gui.ytdl.room.format").to_string(), t!("gui.ytdl.room.paste_button").to_string()] {
            assert!(whole(&rows, &words, (19, 95)), "{code}: {words} whole:\n{}", rows.join("\n"));
        }
        let line = tips(&gui);
        assert!(width(&line) <= 99, "{code}: {line}");
        assert!(line.starts_with(&*t!("gui.tips.ytdl_enter_here")) && line.ends_with(&*t!("gui.tips.ytdl_close")), "{code}: {line}");
        key(&mut gui, KeyCode::Char('s'));
        let rows = lines(&buffer(&mut gui, 100, 30));
        for sort in FolderSort::ALL {
            let words = sort.label().text();
            assert!(whole(&rows, &words, (19, 95)), "{code}: {words} in the sheet:\n{}", rows.join("\n"));
        }
        assert!(whole(&rows, &t!("gui.ytdl.done"), (19, 95)), "{code}: Done:\n{}", rows.join("\n"));
        assert!(width(&tips(&gui)) <= 99, "{code}");
    }
}
