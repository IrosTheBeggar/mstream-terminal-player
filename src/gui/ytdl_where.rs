//! Add from YouTube's two place windows: Saves to — the room's `Saves to`
//! row (`d`): the library, the layout and the folders it makes, kept for
//! the server in `config.toml` or used for this song — and Choose a
//! folder — `choose a folder`, and a failed row's Choose a folder: a
//! folder that already exists, for an account that can't make one.
//!
//! Design: Saves to is the "Add from YouTube · GUI" canvas's SavesTo
//! board, a sheet ([`super::ytdl_sheets::open_sheet`]): the place it makes
//! on its title's own row; the libraries side by side only with two or
//! more (the dot OK); Add from YouTube's own presets
//! ([`crate::ytdl::layout_choices`]) as radios in two columns (the dot
//! ACCENT, the chosen label BOLD); Custom's field a bordered card with its
//! variables under it; a bracket trap's GOLD line with its way on; `use
//! for this song` and `[ Save ]`, DIM until something changes. It reads
//! the account's Discover destination and never writes it. Choose a
//! folder keeps the earlier canvas's TermWhere board (W4, W5): 80 wide at
//! Where copies land's place, in the place of the window it was opened
//! from (the Files window), which comes back once it closes.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::ytdl::{YtdlCallError, YtdlListing};
use crate::config;
use crate::kit::theme::th;
use crate::kit::{ListView, dim, width};
use crate::ytdl::jobs::Redo;
use crate::ytdl::{
    Choice, FolderCheck, LayoutKind, TagField, Target, TrapFix, YTDL_VARS, choice_target, choice_to_save,
    layout_check, layout_choices, layout_problem, library_template, path_crumbs, remembered_choice, trap_label,
};

use super::keep::{self, crumb_sep};
use super::ytdl::{self, FolderLook, Form, Work, YtAct, yt};
use super::{Act, Gui, accent, bright_bold, put, sel, ytdl_sheets};

/// Where copies land's width and the height it anchors as (the 100 × 24
/// floor's whole column).
const PLACE_W: u16 = 80;
const PLACE_ANCHOR: u16 = 22;
/// From Choose a folder's left border: the text's column, a folder's
/// name; the last cell anything is drawn in.
const TEXT_X: u16 = 2;
const TREE_X: u16 = 4;
const LAST_X: u16 = 77;
/// The cells between the libraries side by side.
const GAP: u16 = 3;

/// Saves to's and Choose a folder's controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlaceAct {
    /// `Esc close` on the border.
    Close,
    /// A library, side by side.
    Library(usize),
    /// A layout's radio.
    Pick(LayoutKind),
    /// A click in Custom's field: it takes the keys.
    Field,
    /// A variable under Custom's field: in at the caret.
    Var(usize),
    /// `u use for this song`.
    Use,
    /// `[ Save ]`.
    Save,
    /// A bracket trap's way on: `add a year`, `edit tags`.
    Fix,
    /// Choose a folder: `◂ Up to …`, a folder (in), `[ Use this folder ]`,
    /// and its bar.
    Up,
    Into(usize),
    UseFolder,
    ScrollBy(i32),
    ScrollTo(usize),
}

fn place(act: PlaceAct) -> Act {
    yt(YtAct::Place(act))
}

/// The window that is up.
pub(crate) enum Place {
    Where(Box<WhereUi>),
    Choose(ChooseUi),
}

/// Saves to as a form: the library and layout picked, Custom's text —
/// the choice it makes is read from them each frame ([`choice`]).
pub(crate) struct WhereUi {
    library: String,
    kind: LayoutKind,
    custom: Input,
    custom_keys: bool,
    /// The hand on the libraries (← →): the chosen name ACCENT BOLD.
    hand: bool,
    /// The choice it opened on: Same as Discover and Library layout stand
    /// for themselves until the presets are read.
    opening: Choice,
    /// The presets were asked for.
    asked: bool,
    /// The new-folder check for the place it shows.
    folder: Option<FolderLook>,
    due: Option<(String, i64)>,
}

/// Who Choose a folder chooses for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum For {
    /// The room's song ("c choose a folder").
    Room,
    /// The Files window's song: it takes the window's place.
    Window,
    /// A failed row of the room's list, added again there.
    Job(String),
}

/// Choose a folder: the folder it is in, its folders once listed.
pub(crate) struct ChooseUi {
    of: For,
    crumbs: Vec<String>,
    /// `None`: being listed.
    dirs: Option<Vec<String>>,
    failed: bool,
    /// The first listing: a folder that won't list opens its parent (the
    /// path may name a folder never made).
    first: bool,
    seq: u64,
    /// The rows (`◂ Up to …`, then the folders) in the kit's list-cursor
    /// law: stowed until a walking key.
    view: ListView,
    held: usize,
}

// ── Where it stands ─────────────────────────────────────────────────────────

/// A place window is up over what it was opened from: Saves to and the
/// room's Choose a folder (its form's, a row's) over the room, the
/// window's over Files.
pub(super) fn is_open(gui: &Gui) -> bool {
    match &gui.ytdl.place {
        None => false,
        Some(Place::Where(_)) => ytdl::in_room(gui),
        Some(Place::Choose(c)) => match c.of {
            For::Room | For::Job(_) => ytdl::in_room(gui),
            For::Window => super::ytdl_here::window_up(gui),
        },
    }
}

/// Once a frame: a window whose ground went closes; Saves to
/// asks for its presets once and checks the folders its place makes.
pub(super) fn turn(gui: &mut Gui) -> bool {
    if gui.ytdl.place.is_some() && !is_open(gui) {
        gui.ytdl.place = None;
        return true;
    }
    let Some(Place::Where(w)) = &gui.ytdl.place else { return false };
    let ask = !w.asked && gui.ytdl.presets.is_none();
    let target = where_state(gui, w).target;
    let now = gui.ytdl.now();
    let mut seq = gui.ytdl.seq;
    let Some(Place::Where(w)) = gui.ytdl.place.as_mut() else { return false };
    w.asked = true;
    let work = ytdl::watch_folder(&mut w.folder, &mut w.due, &target, now, &mut seq);
    gui.ytdl.seq = seq;
    if ask {
        // The destination is asked only where the server has Discover's
        // plug-ins (it answers 404 elsewhere).
        let discover = gui.app.capabilities.discovery_plugins;
        ytdl::send(gui, Work::Presets { identity: ytdl::identity(gui), discover });
    }
    let sent = work.is_some();
    if let Some(work) = work {
        ytdl::send(gui, work);
    }
    sent
}

/// A folder check's answer, if it is Saves to's.
pub(super) fn checked(gui: &mut Gui, seq: u64, check: &FolderCheck) {
    if let Some(Place::Where(w)) = gui.ytdl.place.as_mut()
        && let Some(folder) = w.folder.as_mut().filter(|f| f.seq == seq)
    {
        folder.check = Some(check.clone());
    }
}

/// A listing's answer, if it is Choose a folder's. One that won't list on
/// the first look opens its parent; else it says so.
pub(super) fn listed(gui: &mut Gui, seq: u64, result: Result<YtdlListing, YtdlCallError>) {
    let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
    if c.seq != seq {
        return;
    }
    match result {
        Ok(listing) => {
            c.dirs = Some(listing.directories);
            c.failed = false;
            c.first = false;
        }
        Err(_) if c.first && c.crumbs.len() > 1 => {
            c.crumbs.pop();
            list(gui);
        }
        Err(_) => {
            c.dirs = Some(Vec::new());
            c.failed = true;
            c.first = false;
        }
    }
}

// ── Opening ─────────────────────────────────────────────────────────────────

/// Saves to, from the room's row (`d`): on the choice the
/// song goes by (a just-this-song one too); on Same as Discover its own
/// library stays the remembered one, for the radios beside it.
pub(super) fn open_where(gui: &mut Gui) {
    if !ytdl::in_room(gui) {
        return;
    }
    let opening = ytdl::room_choice(gui);
    let (saved, _) = ytdl::remembered(gui);
    let libraries = &gui.app.libraries;
    let library = if opening.kind == LayoutKind::Discover {
        saved.library.filter(|l| libraries.contains(l)).or_else(|| libraries.first().cloned()).unwrap_or_default()
    } else {
        opening.library.clone()
    };
    let custom = if opening.kind == LayoutKind::Custom { Input::new(opening.layout.clone()) } else { Input::default() };
    gui.ytdl.formats = None;
    gui.ytdl.place = Some(Place::Where(Box::new(WhereUi {
        library,
        kind: opening.kind,
        custom,
        custom_keys: false,
        hand: libraries.len() > 1,
        opening,
        asked: false,
        folder: None,
        due: None,
    })));
}

/// Choose a folder for a form's song (`c choose a folder`): from the
/// deepest folder of its place that listed, else its library.
pub(super) fn open_choose(gui: &mut Gui, form: Form) {
    let target = ytdl::form_target(gui, form);
    let check = ytdl::checked_for(&ytdl::draft(gui, form).folder, &target);
    let start = match (check.and_then(|c| c.deepest.clone()), form) {
        (Some(deepest), _) => deepest,
        (None, Form::Window) => gui.app.path.clone(),
        (None, Form::Room) => format!("/{}/", target.library),
    };
    let of = if form == Form::Window { For::Window } else { For::Room };
    open_choose_at(gui, of, &start);
}

/// A failed row's Choose a folder: from the folder it was to make, else
/// its own; the row adds again into the one used.
pub(super) fn open_choose_for_job(gui: &mut Gui, id: &str) {
    let Some(job) = gui.ytdl.jobs().and_then(|j| j.job(id)) else { return };
    let start = job.mkdir.clone().unwrap_or_else(|| job.directory.clone());
    open_choose_at(gui, For::Job(id.to_string()), &start);
}

fn open_choose_at(gui: &mut Gui, of: For, start: &str) {
    let mut crumbs = path_crumbs(start);
    if crumbs.is_empty() {
        crumbs.extend(gui.app.libraries.first().cloned());
    }
    if crumbs.is_empty() {
        return;
    }
    gui.ytdl.formats = None;
    let c = ChooseUi { of, crumbs, dirs: None, failed: false, first: true, seq: 0, view: ListView::default(), held: 0 };
    gui.ytdl.place = Some(Place::Choose(c));
    list(gui);
}

/// The folder Choose a folder is in, listed (again).
fn list(gui: &mut Gui) {
    let seq = ytdl::next_seq(gui);
    let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
    c.seq = seq;
    c.dirs = None;
    c.failed = false;
    let directory = dir_of(&c.crumbs);
    ytdl::send(gui, Work::Listing { seq, directory });
}

fn dir_of(crumbs: &[String]) -> String {
    format!("/{}/", crumbs.join("/"))
}

fn close(gui: &mut Gui) {
    gui.ytdl.place = None;
}

// ── Saves to: its reading ───────────────────────────────────────────────────

/// Everything Saves to shows, read at once.
struct WhereState {
    /// The layouts it offers, in order.
    kinds: Vec<LayoutKind>,
    choice: Choice,
    target: Target,
    valid: bool,
    can_save: bool,
    /// Custom's GOLD line, while what is typed is refused.
    problem: Option<String>,
    /// A bracket trap's words and its way on.
    trap: Option<(String, Option<TrapFix>)>,
    new_folder: bool,
    /// Same as Discover names its own library: the others can't be picked.
    locked: bool,
}

/// The choice the form makes (the webapp's `whereChoice`): the preset of
/// the picked kind for the picked library — Same as Discover and Library
/// layout as they opened while the presets are still out — else Custom's.
fn choice(gui: &Gui, w: &WhereUi) -> Choice {
    let (saved, _) = ytdl::remembered(gui);
    let presets = gui.ytdl.presets.as_ref();
    if w.kind != LayoutKind::Custom {
        let choices = layout_choices(presets, &w.library, Some(&saved));
        if let Some(p) = choices.iter().find(|p| p.kind == w.kind) {
            let layout = p.layout.clone().unwrap_or_default();
            return Choice { library: p.library.clone(), layout, kind: p.kind, base: p.base.clone() };
        }
        if presets.is_none() && w.kind == w.opening.kind {
            return w.opening.clone();
        }
    }
    Choice { library: w.library.clone(), layout: w.custom.value().to_string(), kind: LayoutKind::Custom, base: String::new() }
}

fn where_state(gui: &Gui, w: &WhereUi) -> WhereState {
    let (saved, _) = ytdl::remembered(gui);
    let presets = gui.ytdl.presets.as_ref();
    let mut kinds: Vec<LayoutKind> = layout_choices(presets, &w.library, Some(&saved)).iter().map(|p| p.kind).collect();
    // Before the presets are read, the kind it opened on keeps its radio.
    if !kinds.contains(&w.opening.kind) && presets.is_none() {
        kinds.push(w.opening.kind);
        kinds.sort_by_key(|k| LayoutKind::ALL.iter().position(|a| a == k));
    }
    let choice = choice(gui, w);
    let song = gui.ytdl.draft.tags().song();
    let target = choice_target(&choice, &song);
    let custom = w.kind == LayoutKind::Custom;
    let typed = w.custom.value();
    let problem = (custom && !typed.is_empty()).then(|| layout_problem(&layout_check(typed))).flatten().map(|l| l.text());
    let valid = !(custom && typed.trim().is_empty()) && problem.is_none() && target.is_valid();
    let remembered = remembered_choice(presets, Some(&saved), &gui.app.libraries);
    // Save is a change from what this server remembers — or makes a
    // just-this-song choice stick.
    let can_save = valid && (choice != remembered || gui.ytdl.draft.choice.is_some());
    let trap = target.trap.as_ref().filter(|_| valid).map(|trap| {
        let (label, fix) = trap_label(trap);
        (label.text(), fix)
    });
    let new_folder =
        ytdl::checked_for(&w.folder, &target).is_some_and(|c: &FolderCheck| c.known && !c.new_segments.is_empty());
    WhereState { kinds, locked: choice.kind == LayoutKind::Discover, choice, target, valid, can_save, problem, trap, new_folder }
}

// ── Saves to: its acts ──────────────────────────────────────────────────────

fn where_mut(gui: &mut Gui) -> Option<&mut WhereUi> {
    match gui.ytdl.place.as_mut() {
        Some(Place::Where(w)) => Some(&mut **w),
        _ => None,
    }
}

/// A layout picked: Custom opens holding the layout in force when nothing
/// was typed in it (the webapp's `wherePickLayout`), and takes the keys.
fn pick_layout(gui: &mut Gui, kind: LayoutKind) {
    let was = match &gui.ytdl.place {
        Some(Place::Where(w)) => choice(gui, w),
        _ => return,
    };
    let Some(w) = where_mut(gui) else { return };
    if kind == LayoutKind::Custom && w.custom.value().is_empty() && !was.layout.is_empty() {
        let layout = if was.kind == LayoutKind::Discover { choice_to_save(&was).layout.unwrap_or_default() } else { was.layout };
        w.custom = Input::new(layout);
    }
    w.kind = kind;
    w.custom_keys = kind == LayoutKind::Custom;
    w.hand = false;
}

/// A library picked: a library's own layout leaving with it stays the
/// layout, in Custom's field (the webapp's `wherePickLibrary`). Same as
/// Discover names its own library: none other is picked then.
fn pick_library(gui: &mut Gui, i: usize) {
    let Some(vpath) = gui.app.libraries.get(i).cloned() else { return };
    let (saved, _) = ytdl::remembered(gui);
    let presets = gui.ytdl.presets.clone();
    let Some(w) = where_mut(gui) else { return };
    w.hand = true;
    if w.kind == LayoutKind::Discover {
        return;
    }
    let before = library_template(presets.as_ref(), &w.library, Some(&saved));
    if w.kind == LayoutKind::Library
        && library_template(presets.as_ref(), &vpath, Some(&saved)).is_none()
        && let Some(template) = before
    {
        w.kind = LayoutKind::Custom;
        w.custom = Input::new(template);
    }
    w.library = vpath;
}

/// The layout's dot one radio on (`delta` > 0) or back, clamped.
fn step_layout(gui: &mut Gui, delta: i32) {
    let Some(Place::Where(w)) = &gui.ytdl.place else { return };
    let kinds = where_state(gui, w).kinds;
    let at = kinds.iter().position(|k| *k == w.kind).unwrap_or(0) as i32;
    let next = (at + delta).clamp(0, kinds.len() as i32 - 1) as usize;
    if let Some(kind) = kinds.get(next) {
        pick_layout(gui, *kind);
    }
}

/// The library one on (← →), clamped.
fn step_library(gui: &mut Gui, delta: i32) {
    let Some(Place::Where(w)) = &gui.ytdl.place else { return };
    let libraries = &gui.app.libraries;
    if libraries.len() < 2 {
        return;
    }
    let at = libraries.iter().position(|l| *l == w.library).unwrap_or(0) as i32;
    let next = (at + delta).clamp(0, libraries.len() as i32 - 1) as usize;
    pick_library(gui, next);
}

/// The room's song goes by `choice` (not remembered); a folder chosen
/// before gives way to it.
fn use_for_song(gui: &mut Gui, choice: Choice) {
    let draft = &mut gui.ytdl.draft;
    draft.choice = Some(choice);
    draft.instead = None;
}

/// `[ Save ]`: the choice kept for this server in `config.toml` — its
/// library, layout and kind, the format left as it was — never as the
/// Discover destination; the room's song goes by it from now on. Nothing
/// written (no config to write, no entry for the server), the song still
/// goes by it.
fn save(gui: &mut Gui, choice: &Choice) {
    let kept = choice_to_save(choice);
    let id = ytdl::identity(gui);
    let mut entry = false;
    let wrote = super::servers::update_config(gui, |c| {
        let mut prefs = config::ytdl_prefs(c, &id);
        prefs.library = kept.library;
        prefs.layout = kept.layout;
        prefs.layout_kind = kept.kind.map(|k| k.as_str().to_string());
        entry = config::set_ytdl_prefs(c, &id, &prefs);
    });
    if !(wrote && entry) {
        use_for_song(gui, choice.clone());
        return;
    }
    let draft = &mut gui.ytdl.draft;
    draft.choice = None;
    draft.instead = None;
}

/// The tag a trap's way on opens in the room: the year for a missing
/// year, else the first tag the layout found empty.
fn fix_field(target: &Target, fix: TrapFix) -> TagField {
    match fix {
        TrapFix::AddYear => TagField::Year,
        TrapFix::EditTags => match target.missing.first().map(String::as_str) {
            Some("ALBUM") => TagField::Album,
            Some("YEAR") => TagField::Year,
            Some(_) => TagField::Artist,
            None => TagField::Title,
        },
    }
}

pub(super) fn act(gui: &mut Gui, act: PlaceAct) {
    match act {
        PlaceAct::Close => close(gui),
        PlaceAct::Library(i) => pick_library(gui, i),
        PlaceAct::Pick(kind) => pick_layout(gui, kind),
        PlaceAct::Field => {
            if let Some(w) = where_mut(gui) {
                w.custom_keys = true;
                w.hand = false;
            }
        }
        PlaceAct::Var(i) => {
            let Some(name) = YTDL_VARS.get(i) else { return };
            let Some(w) = where_mut(gui) else { return };
            let token = format!("{{{{{name}}}}}");
            let chars: Vec<char> = w.custom.value().chars().collect();
            let at = w.custom.cursor().min(chars.len());
            let value: String = chars[..at].iter().copied().chain(token.chars()).chain(chars[at..].iter().copied()).collect();
            w.custom = Input::new(value).with_cursor(at + token.chars().count());
            w.custom_keys = true;
        }
        PlaceAct::Use | PlaceAct::Save | PlaceAct::Fix => {
            let Some(Place::Where(w)) = &gui.ytdl.place else { return };
            let st = where_state(gui, w);
            match act {
                PlaceAct::Use if st.valid => use_for_song(gui, st.choice),
                PlaceAct::Save if st.can_save => save(gui, &st.choice),
                // `add a year` keeps the choice for this song and opens
                // Edit tags on the year.
                PlaceAct::Fix => {
                    let Some((_, Some(fix))) = st.trap else { return };
                    let field = fix_field(&st.target, fix);
                    use_for_song(gui, st.choice);
                    close(gui);
                    ytdl::open_tags(gui, Form::Room, field);
                    return;
                }
                _ => return,
            }
            close(gui);
        }
        PlaceAct::Up => {
            let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
            if c.crumbs.len() > 1 {
                c.crumbs.pop();
                c.held = 0;
                c.view.scroll = 0;
                list(gui);
            }
        }
        PlaceAct::Into(i) => {
            let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
            let Some(name) = c.dirs.as_ref().and_then(|d| d.get(i)).cloned() else { return };
            c.crumbs.push(name);
            c.held = 0;
            c.view.scroll = 0;
            list(gui);
        }
        PlaceAct::UseFolder => use_folder(gui),
        PlaceAct::ScrollBy(delta) => wheel(gui, delta),
        PlaceAct::ScrollTo(first) => {
            if let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() {
                c.view.scroll = first;
            }
        }
    }
}

/// `[ Use this folder ]`: the song goes there, for this song only — the
/// room's, the Files window's — or the row adds again into it; the window
/// it stood in for comes back.
fn use_folder(gui: &mut Gui) {
    let Some(Place::Choose(c)) = &gui.ytdl.place else { return };
    if c.dirs.is_none() || c.failed {
        return;
    }
    let dir = dir_of(&c.crumbs);
    match c.of.clone() {
        For::Job(id) => {
            let next = gui.ytdl.jobs_mut().and_then(|jobs| jobs.retry(&id, Redo { directory: Some(dir), ..Redo::default() }));
            ytdl::follow_retry(gui, &id, next);
        }
        of => {
            let form = if of == For::Window { Form::Window } else { Form::Room };
            let draft = ytdl::draft_mut(gui, form);
            draft.instead = Some(dir);
            // A folder chosen, not typed: checked at once.
            draft.folder = None;
        }
    }
    close(gui);
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The place window's keys: it owns them all while it is up.
pub(super) fn handle_key(gui: &mut Gui, key: KeyEvent) -> bool {
    match &gui.ytdl.place {
        Some(Place::Where(_)) => where_key(gui, key),
        Some(Place::Choose(_)) => choose_key(gui, key),
        None => {}
    }
    false
}

/// Saves to: ← → the library, ↑ ↓ the layout's dot (↓ on Custom gives
/// its field the keys), Enter saves once it is on, the letters of the
/// controls it drew, Esc closes. Custom's field: Enter or Esc gives the
/// keys back.
fn where_key(gui: &mut Gui, key: KeyEvent) {
    let ctrl = key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let Some(w) = where_mut(gui) else { return };
    if w.custom_keys {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => w.custom_keys = false,
            KeyCode::Up | KeyCode::BackTab => {
                w.custom_keys = false;
                step_layout(gui, -1);
            }
            KeyCode::Tab | KeyCode::Down => {}
            _ => {
                w.custom.handle_event(&TermEvent::Key(key));
            }
        }
        return;
    }
    let custom = w.kind == LayoutKind::Custom;
    match key.code {
        KeyCode::Esc => close(gui),
        KeyCode::Enter => act(gui, PlaceAct::Save),
        KeyCode::Up => step_layout(gui, -1),
        KeyCode::Down if custom => act(gui, PlaceAct::Field),
        KeyCode::Down => step_layout(gui, 1),
        KeyCode::Left => step_library(gui, -1),
        KeyCode::Right => step_library(gui, 1),
        KeyCode::Char(c) if !ctrl => {
            let lower = c.to_ascii_lowercase();
            let drawn = gui.ytdl.drawn.iter().find(|(letter, _)| *letter == lower).map(|(_, a)| a.clone());
            if let Some(a) = drawn {
                gui.act(yt(a));
            }
        }
        _ => {}
    }
}

/// Choose a folder (W4, W5): ↑ ↓ pick up the cursor, then walk it; Enter
/// goes in (or up) — stowed, uses this folder; Esc closes.
fn choose_key(gui: &mut Gui, key: KeyEvent) {
    let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
    let items = items_of(c);
    let held = c.view.held.then(|| items.get(c.held).copied()).flatten();
    match key.code {
        KeyCode::Esc => close(gui),
        KeyCode::Up | KeyCode::Down if !items.is_empty() => {
            // The first walking key shows the cursor on the first row and
            // moves nothing; the next ones walk it.
            if !c.view.held {
                c.held = 0;
            } else if key.code == KeyCode::Down {
                c.held = (c.held + 1).min(items.len() - 1);
            } else {
                c.held = c.held.saturating_sub(1);
            }
            c.view.pick_up();
        }
        KeyCode::Enter => match held {
            Some(Item::Up) => act(gui, PlaceAct::Up),
            Some(Item::Folder(i)) => act(gui, PlaceAct::Into(i)),
            None => use_folder(gui),
        },
        _ => {}
    }
}

pub(super) fn wheel(gui: &mut Gui, delta: i32) {
    if let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() {
        c.view.step(delta);
    }
}

/// Choose a folder's rows the cursor walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Up,
    Folder(usize),
}

fn items_of(c: &ChooseUi) -> Vec<Item> {
    let up = (c.crumbs.len() > 1).then_some(Item::Up);
    let folders = c.dirs.as_ref().map_or(0, Vec::len);
    up.into_iter().chain((0..folders).map(Item::Folder)).collect()
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The window, drawn over everything at Where copies land's place:
/// `rows` tall — the frame inert beneath it (the kit's Modal).
fn open_place(frame: &mut Frame, gui: &mut Gui, area: Rect, rows: u16, title: &str) -> Rect {
    gui.ui.clear_registries();
    gui.ui.click(area, Act::Guard);
    let h = rows + 2;
    let rect = crate::kit::modal_rect(area, PLACE_W, h, PLACE_ANCHOR.max(h));
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    keep::modal_box(frame, rect);
    // No band: the room's nav reaches the box's left border.
    keep::clear_spill(frame, rect);
    gui.ui.click(rect, Act::Guard);
    keep::draw_borders(frame, gui, rect, title, &t!("gui.discover.keep.esc_close"), place(PlaceAct::Close));
    rect
}

pub(super) fn draw(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // The controls this window draws are the only ones live under it.
    gui.ytdl.drawn.clear();
    match &gui.ytdl.place {
        Some(Place::Where(_)) => draw_where(frame, gui, area),
        Some(Place::Choose(_)) => draw_choose(frame, gui, area),
        None => {}
    }
}

/// One row of Saves to.
#[derive(Debug, Clone)]
enum WRow {
    /// A bracket trap's GOLD row; the first carries its way on.
    Trap(String, bool),
    Blank,
    Libraries,
    /// Two layouts' radios, side by side.
    Radios(LayoutKind, Option<LayoutKind>),
    /// Custom's field, a card three rows tall: its top, the text, its
    /// bottom.
    Field(u8),
    Problem(String),
    Vars,
    Footer,
}

fn fix_key(fix: Option<TrapFix>) -> Option<(&'static str, PlaceAct)> {
    match fix? {
        TrapFix::AddYear => Some(("gui.ytdl.key.add_year", PlaceAct::Fix)),
        TrapFix::EditTags => Some(("gui.ytdl.key.edit", PlaceAct::Fix)),
    }
}

fn where_rows(gui: &Gui, w: &WhereUi, st: &WhereState, wide: usize) -> Vec<WRow> {
    let mut rows = Vec::new();
    if let Some((text, fix)) = &st.trap {
        let key = fix_key(*fix).map_or(0, |(k, _)| usize::from(ytdl::link_width(&t!(k), true)) + 2);
        for (i, line) in ytdl::gold_rows(text, wide.saturating_sub(key)).into_iter().enumerate() {
            rows.push(WRow::Trap(line, i == 0));
        }
        rows.push(WRow::Blank);
    }
    if gui.app.libraries.len() > 1 {
        rows.push(WRow::Libraries);
        rows.push(WRow::Blank);
    }
    rows.extend(st.kinds.chunks(2).map(|pair| WRow::Radios(pair[0], pair.get(1).copied())));
    if w.kind == LayoutKind::Custom {
        rows.push(WRow::Blank);
        rows.extend((0..3).map(WRow::Field));
        rows.extend(st.problem.iter().flat_map(|p| ytdl::gold_rows(p, wide)).map(WRow::Problem));
        rows.push(WRow::Vars);
    }
    rows.push(WRow::Blank);
    rows.push(WRow::Footer);
    rows
}

/// Saves to (the canvas's SavesTo board): the place on the title's own
/// row; the libraries side by side where there are two or more; the
/// layouts' radios in two columns; Custom's field a card, its variables
/// under it; `use for this song` and `[ Save ]`.
fn draw_where(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(Place::Where(w)) = &gui.ytdl.place else { return };
    let st = where_state(gui, w);
    let wide = ytdl_sheets::sheet_room(gui, area);
    let rows = where_rows(gui, w, &st, wide);
    let (library, kind, hand, custom, custom_keys) = (w.library.clone(), w.kind, w.hand, w.custom.clone(), w.custom_keys);
    let rect = ytdl_sheets::open_sheet(frame, gui, area, rows.len() as u16, place(PlaceAct::Close));
    ytdl_sheets::sheet_title(frame, rect, &t!("gui.ytdl.room.saves_to"), |frame, x, room| {
        if st.valid {
            ytdl::draw_place(frame, x, rect.y, &st.target.crumbs(), st.new_folder, room)
        } else {
            let dash = super::discover::dash();
            put(frame, x, rect.y, dash, dim());
            width(dash) as u16
        }
    });
    let (x, total) = ytdl_sheets::sheet_text(rect);
    let end = x + total;
    let half = total / 2 + 1;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let (dot_on, dot_off) = ytdl::dots();
    let mut card = None;
    for (i, row) in rows.into_iter().enumerate() {
        let y = rect.y + 1 + i as u16;
        match row {
            WRow::Blank => {}
            WRow::Trap(line, first) => {
                put(frame, x, y, &super::bar::clip(&line, usize::from(total)), Style::default().fg(th().gold));
                if let Some((key, act)) = fix_key(st.trap.as_ref().and_then(|(_, f)| *f)).filter(|_| first) {
                    let words = t!(key).to_string();
                    let at = end.saturating_sub(ytdl::link_width(&words, true));
                    ytdl::link(frame, gui, at, y, &words, true, YtAct::Place(act));
                }
            }
            WRow::Libraries => {
                // Same as Discover's library is its own: the dot on it.
                let chosen = if st.locked { st.choice.library.clone() } else { library.clone() };
                let mut at = x;
                for (n, name) in gui.app.libraries.clone().iter().enumerate() {
                    let cells = 4 + width(name) as u16;
                    if at + 5 > end {
                        break;
                    }
                    let w_cells = cells.min(end - at);
                    let cell = Rect { x: at, y, width: w_cells, height: 1 };
                    let on = *name == chosen;
                    put(frame, at, y, if on { dot_on } else { dot_off }, if on { Style::default().fg(th().ok) } else { dim() });
                    let style = match (gui.ui.hovers(cell), on) {
                        (_, true) if hand => accent().add_modifier(Modifier::BOLD),
                        (true, _) => bright_bold(),
                        (false, true) => bold,
                        (false, false) => Style::default(),
                    };
                    put(frame, at + 4, y, &super::bar::clip(name, usize::from(w_cells.saturating_sub(4))), style);
                    gui.ui.click(cell, place(PlaceAct::Library(n)));
                    at += w_cells + GAP;
                }
            }
            WRow::Radios(left, right) => {
                for (radio, at) in [(Some(left), x), (right, x + half)] {
                    let Some(radio) = radio else { continue };
                    let on = kind == radio;
                    let cells = if at == x { half.saturating_sub(2) } else { end.saturating_sub(at) };
                    let pick = Rect { x: at, y, width: cells, height: 1 };
                    // The layout's dot is ACCENT (the library's is OK).
                    put(frame, at, y, if on { dot_on } else { dot_off }, if on { accent() } else { dim() });
                    let style = match (gui.ui.hovers(pick), on) {
                        (true, _) => bright_bold(),
                        (false, true) => bold,
                        (false, false) => Style::default(),
                    };
                    put(frame, at + 4, y, &super::bar::clip(&radio.label().text(), usize::from(cells.saturating_sub(4))), style);
                    gui.ui.click(pick, place(PlaceAct::Pick(radio)));
                }
            }
            WRow::Field(0) => card = Some(Rect { x, y, width: total, height: 3 }),
            WRow::Field(_) => {}
            WRow::Problem(line) => {
                put(frame, x, y, &super::bar::clip(&line, usize::from(total)), Style::default().fg(th().gold));
            }
            WRow::Vars => {
                let mut at = x;
                for (n, name) in YTDL_VARS.iter().enumerate() {
                    if n > 0 {
                        put(frame, at, y, " · ", dim());
                        at += 3;
                    }
                    let cell = Rect { x: at, y, width: width(name) as u16, height: 1 };
                    put(frame, at, y, name, if gui.ui.hovers(cell) { bright_bold() } else { dim() });
                    gui.ui.click(cell, place(PlaceAct::Var(n)));
                    at += cell.width;
                }
            }
            WRow::Footer => {
                let button = rect.right().saturating_sub(3);
                let save = t!("gui.ytdl.where.save").to_string();
                let at = ytdl_sheets::bracket_button(frame, gui, button, y, &save, st.can_save, place(PlaceAct::Save));
                if !st.can_save {
                    // Off: no click, no hover — its tooltip says why.
                    let why = if st.valid { "gui.ytdl.where.save_off_same" } else { "gui.ytdl.where.save_off_bad" };
                    gui.ui.tip(Rect { x: at, y, width: button - at, height: 1 }, t!(why).to_string());
                }
                // Refused, `use for this song` steps aside.
                if st.valid {
                    let words = t!("gui.ytdl.key.use_song").to_string();
                    let lx = at.saturating_sub(3 + ytdl::link_width(&words, false));
                    ytdl::link(frame, gui, lx, y, &words, false, YtAct::Place(PlaceAct::Use));
                }
            }
        }
    }
    if let Some(card) = card {
        ytdl_sheets::field_card(frame, gui, card, custom.value(), custom.cursor(), custom_keys, place(PlaceAct::Field));
    }
}

/// One row of Choose a folder.
#[derive(Debug, Clone)]
enum CRow {
    Head,
    Blank,
    /// A row the cursor walks: `◂ Up to …` or a folder.
    Item(Item),
    /// What stands for the folders: none there, being listed, unlistable.
    Note(String, Style),
    Button,
}

/// Choose a folder (W4, W5): the folder it is in, `◂ Up to …`, its
/// folders — as many as the window holds, the rest under the wheel — and
/// `[ Use this folder ]`.
fn draw_choose(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() else { return };
    let items = items_of(c);
    let up = usize::from(c.crumbs.len() > 1);
    let folders = items.len() - up;
    // At most the anchor's height: the head, two blanks, the button and
    // the borders aside.
    let most = usize::from(PLACE_ANCHOR.min(area.height.saturating_sub(2))).saturating_sub(6);
    let room = most.saturating_sub(up);
    let held = c.view.held.then_some(c.held);
    let (first, visible) = c.view.window(folders, held.and_then(|h| h.checked_sub(up)), room);
    let mut rows = vec![CRow::Head, CRow::Blank];
    if up == 1 {
        rows.push(CRow::Item(Item::Up));
    }
    match &c.dirs {
        None => rows.push(CRow::Note(keep::ell(&t!("busy.listing")), accent())),
        Some(_) if c.failed => {
            rows.push(CRow::Note(format!("! {}", t!("gui.ytdl.choose.failed")), Style::default().fg(th().gold)))
        }
        Some(dirs) if dirs.is_empty() => rows.push(CRow::Note(t!("gui.ytdl.choose.empty").to_string(), dim())),
        Some(_) => rows.extend((first..first + visible).map(|i| CRow::Item(Item::Folder(i)))),
    }
    rows.push(CRow::Blank);
    rows.push(CRow::Button);
    let crumbs = c.crumbs.clone();
    let dirs = c.dirs.clone().unwrap_or_default();
    let usable = c.dirs.is_some() && !c.failed;
    let rect = open_place(frame, gui, area, rows.len() as u16, &t!("gui.ytdl.choose.title"));
    let text_x = rect.x + TEXT_X;
    let tree_x = rect.x + TREE_X;
    let end = rect.x + LAST_X + 1;
    let tree_room = usize::from(end - tree_x);
    let mut bar_at = None;
    for (i, row) in rows.iter().enumerate() {
        let y = rect.y + 1 + i as u16;
        match row {
            CRow::Blank => {}
            CRow::Head => {
                let (clipped, shown) = ytdl::fit_crumbs(&crumbs, 0, usize::from(end - text_x));
                let mut words = shown.join(crumb_sep());
                if clipped {
                    words = format!("{}{}{words}", keep::ell("…"), crumb_sep());
                }
                put(frame, text_x, y, &words, dim());
            }
            CRow::Item(item) => {
                let at = items.iter().position(|it| it == item);
                let lit = held.is_some() && at == held;
                // The slab from the text's column, `◂` on it.
                let slab = Rect { x: text_x, y, width: end - text_x, height: 1 };
                if lit {
                    frame.buffer_mut().set_style(slab, sel());
                }
                let hover = !lit && gui.ui.hovers(slab);
                match item {
                    Item::Up => {
                        let parent = crumbs.iter().rev().nth(1).cloned().unwrap_or_default();
                        let words = t!("gui.ytdl.choose.up", folder = parent).to_string();
                        let style = if lit { sel() } else if hover { Style::default().fg(th().bright) } else { dim() };
                        put(frame, text_x, y, super::back_glyph(), style);
                        put(frame, tree_x, y, &super::bar::clip(&words, tree_room), style);
                        gui.ui.click(slab, place(PlaceAct::Up));
                    }
                    Item::Folder(n) => {
                        let name = dirs.get(*n).cloned().unwrap_or_default();
                        let style = if lit { sel() } else if hover { Style::default().fg(th().bright) } else { Style::default() };
                        put(frame, tree_x, y, &super::bar::clip(&name, tree_room), style);
                        gui.ui.click(slab, place(PlaceAct::Into(*n)));
                        if bar_at.is_none() {
                            bar_at = Some(y);
                        }
                    }
                }
            }
            CRow::Note(words, style) => put(frame, tree_x, y, &super::bar::clip(words, tree_room), *style),
            CRow::Button => {
                let words = format!("[ {} ]", t!("gui.ytdl.choose.use"));
                let cell = Rect { x: end.saturating_sub(width(&words) as u16), y, width: width(&words) as u16, height: 1 };
                let style = if !usable {
                    dim()
                } else if gui.ui.hovers(cell) {
                    bright_bold()
                } else {
                    accent().add_modifier(Modifier::BOLD)
                };
                put(frame, cell.x, y, &words, style);
                if usable {
                    gui.ui.click(cell, place(PlaceAct::UseFolder));
                }
            }
        }
    }
    // More folders than fit: the kit's bar on the right border.
    if let Some(y) = bar_at
        && visible < folders
    {
        let bar = Rect { x: rect.right().saturating_sub(1), y, width: 1, height: visible as u16 };
        crate::kit::scroll_list(frame, &mut gui.ui, bar, folders, visible, first, place(PlaceAct::ScrollBy(-1)), place(PlaceAct::ScrollBy(1)), |n| {
            Act::Yt(YtAct::Place(PlaceAct::ScrollTo(n)))
        });
    }
}

/// The tips line while a place window is up.
pub(super) fn tips(gui: &Gui) -> String {
    match &gui.ytdl.place {
        Some(Place::Choose(_)) => t!("gui.tips.ytdl_choose").to_string(),
        Some(Place::Where(w)) if w.custom_keys => t!("gui.tips.ytdl_where_layout").to_string(),
        Some(Place::Where(w)) => {
            let st = where_state(gui, w);
            // Each part, and whether it is a key word: those give way, the
            // last first, where a translation passes the footer's 99 cells.
            let mut parts: Vec<(String, bool)> = Vec::new();
            if gui.app.libraries.len() > 1 && !st.locked {
                parts.push((t!("gui.tips.ytdl_where_library").to_string(), false));
            }
            parts.push((t!("gui.tips.ytdl_where_rows").to_string(), false));
            if let Some((key, _)) = fix_key(st.trap.as_ref().and_then(|(_, f)| *f)) {
                parts.push((t!(key).to_string(), true));
            }
            if st.can_save {
                parts.push((t!("gui.tips.ytdl_where_save").to_string(), false));
            }
            if st.valid {
                parts.push((t!("gui.ytdl.key.use_song").to_string(), true));
            }
            parts.push((t!("gui.tips.ytdl_close").to_string(), false));
            let line = |parts: &[(String, bool)]| parts.iter().map(|(words, _)| words.as_str()).collect::<Vec<_>>().join(" · ");
            while width(&line(&parts)) > 99 {
                let Some(at) = parts.iter().rposition(|(_, key)| *key) else { break };
                parts.remove(at);
            }
            line(&parts)
        }
        None => String::new(),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::Ping;
    use crate::api::ytdl::{YtdlMetadata, YtdlStarted};
    use crate::ytdl::jobs::{Answer, Call};
    use crate::ytdl::{DiscoverPlace, FolderCheck, Presets};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Position;
    use super::super::ytdl::tests::{
        Cells, HOST, LINK, answer_folders, answer_lookup, assert_frame, at, buffer, calls, connected, discovery_new,
        english, job, key, lines, one_more_time, ping, room, seed, serve, type_text,
    };
    use super::super::ytdl::{Keys, Reply, apply_reply};
    use crate::ytdl::TagField;

    /// The account's Discover destination and Music's own template: every
    /// preset shows.
    fn presets() -> Presets {
        Presets {
            discover: Some(DiscoverPlace { vpath: "Music".into(), base: "Discover".into(), layout: "{{ARTIST}}".into() }),
            templates: vec![("Music".into(), "{{ALBUMARTIST}}/{{ALBUM}} ({{YEAR}})".into())],
        }
    }

    /// The room with `meta` found (its folder checked), the frame drawn.
    fn found_room(meta: YtdlMetadata, check: FolderCheck) -> Gui {
        let mut gui = room();
        ytdl::paste(&mut gui, LINK);
        answer_lookup(&mut gui, Ok(meta));
        answer_folders(&mut gui, check);
        buffer(&mut gui, 100, 30);
        gui
    }

    /// The `Saves to` row's `d`: the sheet, its presets read, its place
    /// checked.
    fn opened(gui: &mut Gui, check: FolderCheck) {
        key(gui, KeyCode::Char('d'));
        assert!(is_open(gui), "Saves to");
        ytdl::poll(gui);
        answer_presets(gui, presets());
        answer_folders(gui, check);
    }

    /// The presets asked for, answered (once a visit: a second opening
    /// finds them read).
    fn answer_presets(gui: &mut Gui, presets: Presets) {
        if let Some(asked) = gui.ytdl.work.iter().position(|w| matches!(w, Work::Presets { .. })) {
            gui.ytdl.work.remove(asked);
            apply_reply(gui, Reply::Presets { identity: HOST.into(), presets });
        }
    }

    /// Choose a folder's listing that's out, answered with `dirs`.
    fn answer_listing(gui: &mut Gui, dirs: &[&str]) -> String {
        let (seq, directory) = gui
            .ytdl
            .work
            .iter()
            .rev()
            .find_map(|w| match w {
                Work::Listing { seq, directory } => Some((*seq, directory.clone())),
                _ => None,
            })
            .expect("a listing out");
        gui.ytdl.work.retain(|w| !matches!(w, Work::Listing { .. }));
        let listing = YtdlListing {
            path: directory.clone(),
            directories: dirs.iter().map(|d| d.to_string()).collect(),
            files: Vec::new(),
        };
        apply_reply(gui, Reply::Listing { seq, result: Ok(listing) });
        directory
    }

    fn new_at(segment: usize) -> FolderCheck {
        FolderCheck { known: true, new_segments: vec![segment], deepest: Some("/Music/Daft Punk/".into()), files: None }
    }

    fn no_year() -> YtdlMetadata {
        YtdlMetadata { year: None, ..one_more_time() }
    }

    /// Choose a folder's 80 columns at x 10, top row 4 (100 × 30) or 1
    /// (100 × 24).
    fn assert_place(gui: &mut Gui, (w, h): (u16, u16), rows: &str, runs: &[&str]) -> Buffer {
        let b = buffer(gui, w, h);
        assert_frame(&b, (10, if h == 30 { 4 } else { 1 }), 80, rows, runs);
        b
    }

    /// The Saves to sheet's 66 columns over 100 × 30: in the room's middle
    /// with the queue folded (x 24), the frame's with it open (x 17).
    fn assert_sheet(gui: &mut Gui, rows: &str, runs: &[&str]) -> Buffer {
        let b = buffer(gui, 100, 30);
        let tall = rows.lines().filter(|l| l.trim_start().starts_with('|')).count() as u16;
        assert_frame(&b, (if gui.queue_open { 17 } else { 24 }, (30 - tall) / 2), 66, rows, runs);
        b
    }

    /// Dump the sheet's frames for a look (`cargo test dump_saves_to -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn dump_saves_to() {
        let _en = english();
        let show = |name: &str, gui: &mut Gui| {
            eprintln!("== {name}");
            for (y, row) in lines(&buffer(gui, 100, 30)).iter().enumerate() {
                eprintln!("{y:2} |{}", row.trim_end());
            }
        };
        let board = super::super::ytdl::tests::board_of;
        let mut gui = found_room(one_more_time(), discovery_new());
        opened(&mut gui, new_at(1));
        show("opened", &mut gui);
        eprintln!("{}", board(&buffer(&mut gui, 100, 30), (24, 10), (66, 9), "W1"));
        gui.act(place(PlaceAct::Pick(LayoutKind::Custom)));
        for _ in 0.."{{ALBUM}}".len() {
            key(&mut gui, KeyCode::Backspace);
        }
        type_text(&mut gui, "{{TITLE}}");
        show("custom", &mut gui);
        eprintln!("{}", board(&buffer(&mut gui, 100, 30), (24, 7), (66, 16), "REFUSED"));
        let mut gui = found_room(no_year(), discovery_new());
        opened(&mut gui, new_at(1));
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        ytdl::poll(&mut gui);
        super::super::ytdl::tests::advance(&gui, 500);
        answer_folders(&mut gui, new_at(1));
        show("trap", &mut gui);
        eprintln!("{}", board(&buffer(&mut gui, 100, 30), (24, 9), (66, 12), "W3"));
    }

    mod boards {
        pub(super) const W1: &str = r#"
            |╭─ Saves to · Music › Daft Punk › Discovery new ─────────────────╮
            |│ (•) Music   ( ) Mixes                                          │
            |│                                                                │
            |│ (•) Artist / Album              ( ) Artist                     │
            |│ ( ) Top of the library          ( ) Same as Discover           │
            |│ ( ) Library layout              ( ) Custom                     │
            |│                                                                │
            |│                                  use for this song   [ Save ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const W1_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-10:Text:B", "0:12-12:Dim", "0:14-18:Text", "0:20-20:Dim", "0:22-30:Text",
            "0:32-32:Dim", "0:34-42:Text", "0:44-46:Dim", "0:48-65:Accent", "1:0-0:Accent", "1:2-4:Ok",
            "1:6-10:Accent:B", "1:14-16:Dim", "1:18-22:Text", "1:65-65:Accent", "2:0-65:Accent", "3:0-4:Accent",
            "3:6-19:Text:B", "3:34-36:Dim", "3:38-43:Text", "3:65-65:Accent", "4:0-0:Accent", "4:2-4:Dim",
            "4:6-23:Text", "4:34-36:Dim", "4:38-53:Text", "4:65-65:Accent", "5:0-0:Accent", "5:2-4:Dim",
            "5:6-19:Text", "5:34-36:Dim", "5:38-43:Text", "5:65-65:Accent", "6:0-65:Accent", "7:0-0:Accent",
            "7:35-62:Dim", "7:65-65:Accent", "8:0-65:Accent",
        ];
        pub(super) const W3: &str = r#"
            |╭─ Saves to · Music › Daft Punk › Discovery () new ──────────────╮
            |│ ! No year for this song, so the folder reads      add a year ▸ │
            |│   “Discovery ()”.                                              │
            |│                                                                │
            |│ (•) Music   ( ) Mixes                                          │
            |│                                                                │
            |│ ( ) Artist / Album              ( ) Artist                     │
            |│ ( ) Top of the library          ( ) Same as Discover           │
            |│ (•) Library layout              ( ) Custom                     │
            |│                                                                │
            |│                                  use for this song   [ Save ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const W3_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-10:Text:B", "0:12-12:Dim", "0:14-18:Text", "0:20-20:Dim", "0:22-30:Text",
            "0:32-32:Dim", "0:34-45:Text", "0:47-49:Dim", "0:51-65:Accent", "1:0-0:Accent", "1:2-45:Gold",
            "1:52-63:Dim", "1:65-65:Accent", "2:0-0:Accent", "2:4-18:Gold", "2:65-65:Accent", "3:0-65:Accent",
            "4:0-0:Accent", "4:2-4:Ok", "4:6-10:Text:B", "4:14-16:Dim", "4:18-22:Text", "4:65-65:Accent",
            "5:0-65:Accent", "6:0-0:Accent", "6:2-4:Dim", "6:6-19:Text", "6:34-36:Dim", "6:38-43:Text",
            "6:65-65:Accent", "7:0-0:Accent", "7:2-4:Dim", "7:6-23:Text", "7:34-36:Dim", "7:38-53:Text",
            "7:65-65:Accent", "8:0-4:Accent", "8:6-19:Text:B", "8:34-36:Dim", "8:38-43:Text", "8:65-65:Accent",
            "9:0-65:Accent", "10:0-0:Accent", "10:35-51:Dim", "10:55-62:Accent:B", "10:65-65:Accent", "11:0-65:Accent",
        ];
        pub(super) const SAVES_TO: &str = r#"
            |╭─ Saves to · Music › Daft Punk › 2001 - Discovery new ──────────╮
            |│ (•) Music   ( ) Mixes                                          │
            |│                                                                │
            |│ ( ) Artist / Album              ( ) Artist                     │
            |│ ( ) Top of the library          ( ) Same as Discover           │
            |│ ( ) Library layout              (•) Custom                     │
            |│                                                                │
            |│ ╭────────────────────────────────────────────────────────────╮ │
            |│ │ {{ARTIST}}/{{YEAR}} - {{ALBUM}}▏                           │ │
            |│ ╰────────────────────────────────────────────────────────────╯ │
            |│ ARTIST · ALBUMARTIST · ALBUM · YEAR                            │
            |│                                                                │
            |│                                  use for this song   [ Save ]  │
            |╰────────────────────────────────────────────────────────────────╯
        "#;
        pub(super) const SAVES_TO_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-10:Text:B", "0:12-12:Dim", "0:14-18:Text", "0:20-20:Dim", "0:22-30:Text",
            "0:32-32:Dim", "0:34-49:Text", "0:51-53:Dim", "0:55-65:Accent", "1:0-0:Accent", "1:2-4:Ok",
            "1:6-10:Text:B", "1:14-16:Dim", "1:18-22:Text", "1:65-65:Accent", "2:0-65:Accent", "3:0-0:Accent",
            "3:2-4:Dim", "3:6-19:Text", "3:34-36:Dim", "3:38-43:Text", "3:65-65:Accent", "4:0-0:Accent", "4:2-4:Dim",
            "4:6-23:Text", "4:34-36:Dim", "4:38-53:Text", "4:65-65:Accent", "5:0-0:Accent", "5:2-4:Dim",
            "5:6-19:Text", "5:34-36:Accent", "5:38-43:Text:B", "5:65-65:Accent", "6:0-65:Accent", "7:0-65:Accent",
            "8:0-2:Accent", "8:4-35:Text", "8:63-65:Accent", "9:0-65:Accent", "10:0-0:Accent", "10:2-36:Dim",
            "10:65-65:Accent", "11:0-65:Accent", "12:0-0:Accent", "12:35-51:Dim", "12:55-62:Accent:B",
            "12:65-65:Accent", "13:0-65:Accent",
        ];
        pub(super) const W4: &str = r#"
            |╭─ Choose a folder ────────────────────────────────────────────────────────────╮
            |│ Music › Daft Punk                                                            │
            |│                                                                              │
            |│ ◂ Up to Music                                                                │
            |│   Homework                                                                   │
            |│   Human After All                                                            │
            |│                                                                              │
            |│                                                          [ Use this folder ] │
            |╰────────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const W4_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-17:Text:B", "0:19-79:Accent", "1:0-0:Accent", "1:2-18:Dim", "1:79-79:Accent", "2:0-0:Accent",
            "2:79-79:Accent", "3:0-0:Accent", "3:2-2:Dim", "3:4-14:Dim", "3:79-79:Accent", "4:0-0:Accent", "4:4-11:Text",
            "4:79-79:Accent", "5:0-0:Accent", "5:4-18:Text", "5:79-79:Accent", "6:0-0:Accent", "6:79-79:Accent",
            "7:0-0:Accent", "7:59-77:Accent:B", "7:79-79:Accent", "8:0-66:Accent", "8:68-76:Dim", "8:78-79:Accent",
        ];
        pub(super) const W5: &str = r#"
            |╭─ Choose a folder ────────────────────────────────────────────────────────────╮
            |│ Music › Daft Punk › Homework                                                 │
            |│                                                                              │
            |│ ◂ Up to Daft Punk                                                            │
            |│   No folders here.                                                           │
            |│                                                                              │
            |│                                                          [ Use this folder ] │
            |╰────────────────────────────────────────────────────────────────── Esc close ─╯
        "#;
        pub(super) const W5_RUNS: &[&str] = &[
            "0:0-1:Accent", "0:3-17:Text:B", "0:19-79:Accent", "1:0-0:Accent", "1:2-29:Dim", "1:79-79:Accent", "2:0-0:Accent",
            "2:79-79:Accent", "3:0-0:Accent", "3:2-2:Dim", "3:4-18:Dim", "3:79-79:Accent", "4:0-0:Accent", "4:4-19:Dim",
            "4:79-79:Accent", "5:0-0:Accent", "5:79-79:Accent", "6:0-0:Accent", "6:59-77:Accent:B", "6:79-79:Accent",
            "7:0-66:Accent", "7:68-76:Dim", "7:78-79:Accent",
        ];
    }

    // ── Where new songs land ───────────────────────────────────────────────

    /// The `Saves to` row (or `d`) opens the sheet on the choice the song
    /// goes by: the place it makes on the title's own row; the libraries
    /// side by side (dot OK, the hand ACCENT BOLD on the chosen name); the
    /// layouts in two columns (dot ACCENT, label BOLD); Save off until
    /// something changes, its tooltip why. It stands over the block it
    /// was opened from — in the room's middle, or the frame's where the
    /// queue leaves the room narrower than it.
    #[test]
    fn saves_to_opens_on_the_choice_in_force() {
        let _en = english();
        let mut gui = found_room(one_more_time(), discovery_new());
        opened(&mut gui, new_at(1));
        let b = assert_sheet(&mut gui, boards::W1, boards::W1_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Mixes")), Some(place(PlaceAct::Library(1))));
        assert_eq!(gui.ui.hit(at(&rows, "Top of the library")), Some(place(PlaceAct::Pick(LayoutKind::Top))));
        assert_eq!(gui.ui.hit(at(&rows, "Same as Discover")), Some(place(PlaceAct::Pick(LayoutKind::Discover))), "the second column");
        assert_eq!(gui.ui.hit(at(&rows, "use for this song")), Some(place(PlaceAct::Use)));
        assert_eq!(gui.ui.hit(at(&rows, "[ Save ]")), Some(Act::Guard), "Save is off: no click");
        assert_eq!(gui.ui.hit(Position { x: 5, y: 5 }), Some(place(PlaceAct::Close)), "a click outside closes it");
        assert_eq!(tips(&gui), "← → library · ↑ ↓ rows · u use for this song · Esc close");
        // The block it was opened from is under it whole: nothing of the
        // room's rows shows beside the sheet.
        for y in 10..19 {
            let beside: String = rows[y].chars().skip(90).take(6).collect();
            assert!(beside.trim().is_empty(), "row {y} beside the sheet: {beside:?}");
        }
        gui.queue_open = true;
        assert_sheet(&mut gui, boards::W1, boards::W1_RUNS);
        gui.queue_open = false;
        let b = buffer(&mut gui, 100, 24);
        assert_frame(&b, (24, 7), 66, boards::W1, boards::W1_RUNS);
        // Without the presets read, Same as Discover and Library layout
        // aren't offered.
        let mut gui = found_room(one_more_time(), discovery_new());
        key(&mut gui, KeyCode::Char('d'));
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(!rows.iter().any(|r| r.contains("Same as Discover")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Esc);
        assert!(!is_open(&gui), "Esc closes");
        assert!(ytdl::in_room(&gui), "the room stays");
    }

    /// The presets are asked once a visit, the Discover destination only
    /// where the server has Discover's plug-ins — and never written.
    #[test]
    fn the_presets_are_read_once_and_the_destination_never_written() {
        let _en = english();
        for plugins in [false, true] {
            let mut gui = found_room(one_more_time(), discovery_new());
            gui.app.capabilities.discovery_plugins = plugins;
            key(&mut gui, KeyCode::Char('d'));
            ytdl::poll(&mut gui);
            ytdl::poll(&mut gui);
            let asked: Vec<&Work> = gui.ytdl.work.iter().filter(|w| matches!(w, Work::Presets { .. })).collect();
            assert_eq!(asked, vec![&Work::Presets { identity: HOST.into(), discover: plugins }], "once");
            assert!(
                gui.ytdl.work.iter().all(|w| matches!(w, Work::Presets { .. } | Work::Folders { .. })),
                "nothing else goes out: {:?}",
                gui.ytdl.work
            );
        }
    }

    /// SavesTo (the canvas's board): Custom chosen — its field a bordered
    /// card with the keys, its variables under it, the place its layout
    /// makes on the title's row, Save on. (The board's sample wrote the
    /// layout without the artist its place shows: the layout that makes
    /// that place stands in its field here.)
    #[test]
    fn saves_to_with_a_custom_layout_is_the_board() {
        let _en = english();
        let mut gui = found_room(one_more_time(), discovery_new());
        gui.queue_open = true;
        buffer(&mut gui, 100, 30);
        opened(&mut gui, new_at(1));
        gui.act(place(PlaceAct::Pick(LayoutKind::Custom)));
        for _ in 0.."{{ALBUMARTIST}}/{{ALBUM}}".len() {
            key(&mut gui, KeyCode::Backspace);
        }
        type_text(&mut gui, "{{ARTIST}}/{{YEAR}} - {{ALBUM}}");
        ytdl::poll(&mut gui);
        super::super::ytdl::tests::advance(&gui, 500);
        answer_folders(&mut gui, new_at(1));
        let b = assert_sheet(&mut gui, boards::SAVES_TO, boards::SAVES_TO_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "{{ARTIST}}")), Some(place(PlaceAct::Field)), "the card takes the keys");
        assert_eq!(gui.ui.hit(at(&rows, "[ Save ]")), Some(place(PlaceAct::Save)));
        assert_eq!(tips(&gui), "type · Enter or Esc gives the keys back");
    }

    /// Custom takes the keys, holding the layout in force; a refused
    /// layout's GOLD lines under its card, the place a DIM `—`, Save off
    /// and `use for this song` stepping aside; a variable goes in at the
    /// caret; Enter or Esc gives the keys back.
    #[test]
    fn custom_refused_reads_a_dash() {
        let _en = english();
        let mut gui = found_room(one_more_time(), discovery_new());
        opened(&mut gui, new_at(1));
        gui.act(place(PlaceAct::Pick(LayoutKind::Custom)));
        let Some(Place::Where(w)) = &gui.ytdl.place else { panic!() };
        assert_eq!((w.kind, w.custom.value(), w.custom_keys), (LayoutKind::Custom, "{{ALBUMARTIST}}/{{ALBUM}}", true));
        for _ in 0.."{{ALBUM}}".len() {
            key(&mut gui, KeyCode::Backspace);
        }
        type_text(&mut gui, "{{TITLE}}");
        let b = buffer(&mut gui, 100, 30);
        let rows = lines(&b);
        let text = rows.join("\n");
        assert!(rows[7].starts_with_at(24, "╭─ Saves to · — ─"), "the place a dash:\n{text}");
        assert!(rows[15].starts_with_at(26, "│ {{ALBUMARTIST}}/{{TITLE}}▏"), "{text}");
        let gold = at(&rows, "! Unknown variable {{TITLE}}. Use ARTIST, ALBUMARTIST, ALBUM,");
        assert_eq!((gold.x, gold.y, b[(gold.x, gold.y)].fg), (26, 17, th().gold), "under the card:\n{text}");
        assert!(rows[18].starts_with_at(26, "  YEAR.") && rows[19].starts_with_at(26, "ARTIST · ALBUMARTIST · ALBUM · YEAR"), "{text}");
        assert!(!text.contains("use for this song"), "refused, it steps aside:\n{text}");
        let save = at(&rows, "[ Save ]");
        assert_eq!((b[(save.x, save.y)].fg, gui.ui.hit(save)), (th().dim, Some(Act::Guard)), "Save is off");
        assert_eq!(tips(&gui), "type · Enter or Esc gives the keys back");
        let year = Position { x: at(&rows, "ALBUM · YEAR").x + 8, y: 19 };
        assert_eq!(gui.ui.hit(year), Some(place(PlaceAct::Var(3))));
        key(&mut gui, KeyCode::Home);
        gui.act(place(PlaceAct::Var(3)));
        let Some(Place::Where(w)) = &gui.ytdl.place else { panic!() };
        assert_eq!(w.custom.value(), "{{YEAR}}{{ALBUMARTIST}}/{{TITLE}}", "in at the caret");
        key(&mut gui, KeyCode::Esc);
        assert!(is_open(&gui), "Esc gave the keys back, the window stays");
        key(&mut gui, KeyCode::Char('u'));
        assert!(is_open(&gui) && gui.ytdl.draft.choice.is_none(), "refused: u is no key");
    }

    /// Library layout with no year — the bracket trap's GOLD lines with
    /// `add a year ▸`, Save on; it (or `y`) keeps the choice for this song
    /// and opens Edit tags on the year.
    #[test]
    fn a_trap_offers_its_way_on() {
        let _en = english();
        let mut gui = found_room(no_year(), discovery_new());
        opened(&mut gui, new_at(1));
        for _ in 0..4 {
            key(&mut gui, KeyCode::Down);
        }
        ytdl::poll(&mut gui);
        super::super::ytdl::tests::advance(&gui, 500);
        answer_folders(&mut gui, new_at(1));
        let b = assert_sheet(&mut gui, boards::W3, boards::W3_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "add a year ▸")), Some(place(PlaceAct::Fix)));
        assert_eq!(gui.ui.hit(at(&rows, "[ Save ]")), Some(place(PlaceAct::Save)));
        assert_eq!(tips(&gui), "← → library · ↑ ↓ rows · y add a year · Enter save · u use for this song · Esc close");
        key(&mut gui, KeyCode::Char('y'));
        assert!(!is_open(&gui));
        let draft = &gui.ytdl.draft;
        assert_eq!(draft.keys, Keys::Tag(TagField::Year), "the year holds the keys");
        assert!(draft.tags_open && ytdl::tags_up(&gui), "in Edit tags");
        assert_eq!(draft.choice.as_ref().map(|c| c.kind), Some(LayoutKind::Library), "for this song");
        type_text(&mut gui, "2001");
        key(&mut gui, KeyCode::Enter);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains("Music › Daft Punk › Discovery (2001)")), "{}", rows.join("\n"));
    }

    /// Save keeps the choice for this server in config.toml — the
    /// library, the layout and its kind, the format left as it was — and
    /// the room's song goes by it; u use for this song only for this song.
    #[test]
    fn save_writes_the_servers_choice_and_use_only_the_songs() {
        let _en = english();
        let _scratch = crate::config::testing::Scratch::new("ytdl-where-save");
        let on_disk = crate::config::Config {
            servers: vec![crate::config::ServerEntry { url: HOST.into(), ytdl_format: Some("m4a".into()), ..Default::default() }],
            ..crate::config::Config::default()
        };
        crate::config::save(&on_disk).unwrap();
        let mut gui = found_room(one_more_time(), discovery_new());
        gui.config = crate::config::load().unwrap();
        gui.config_ok = true;
        opened(&mut gui, new_at(1));
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Down);
        buffer(&mut gui, 100, 30);
        assert!(tips(&gui).contains("Enter save"), "something changed: {}", tips(&gui));
        key(&mut gui, KeyCode::Enter);
        assert!(!is_open(&gui), "saved, it closes");
        let prefs = config::ytdl_prefs(&crate::config::load().unwrap(), HOST);
        assert_eq!(
            (prefs.library.as_deref(), prefs.layout.as_deref(), prefs.layout_kind.as_deref(), prefs.format.as_deref()),
            (Some("Mixes"), Some("{{ALBUMARTIST}}"), Some("artist"), Some("m4a")),
            "only the place changed"
        );
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains("Mixes › Daft Punk")), "{}", rows.join("\n"));

        // Use for this song: not remembered.
        opened(&mut gui, new_at(1));
        key(&mut gui, KeyCode::Down);
        buffer(&mut gui, 100, 30);
        key(&mut gui, KeyCode::Char('u'));
        assert!(!is_open(&gui));
        assert_eq!(gui.ytdl.draft.choice.as_ref().map(|c| c.kind), Some(LayoutKind::Top));
        let prefs = config::ytdl_prefs(&crate::config::load().unwrap(), HOST);
        assert_eq!(prefs.layout_kind.as_deref(), Some("artist"), "the server's choice stands");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains(" Mixes ▸")), "{}", rows.join("\n"));
    }

    /// Save with nothing to write it to (no config file the player may
    /// write, or no entry for this server): the song still goes by the
    /// choice, as Use for this song would.
    #[test]
    fn save_that_wrote_nothing_keeps_the_choice_for_the_song() {
        let _en = english();
        for config_ok in [false, true] {
            let _scratch = crate::config::testing::Scratch::new("ytdl-where-save-none");
            crate::config::save(&crate::config::Config::default()).unwrap();
            let mut gui = found_room(one_more_time(), discovery_new());
            gui.config_ok = config_ok;
            opened(&mut gui, new_at(1));
            key(&mut gui, KeyCode::Right);
            key(&mut gui, KeyCode::Down);
            buffer(&mut gui, 100, 30);
            key(&mut gui, KeyCode::Enter);
            assert!(!is_open(&gui), "{config_ok}: it closes");
            let choice = gui.ytdl.draft.choice.as_ref().unwrap_or_else(|| panic!("{config_ok}: the song's choice dropped"));
            assert_eq!(choice.library, "Mixes", "{config_ok}");
            assert!(config::ytdl_prefs(&crate::config::load().unwrap(), HOST).library.is_none(), "{config_ok}: nothing kept");
        }
    }

    // ── Choose a folder ────────────────────────────────────────────────────

    /// W4 and W5: from the room's `choose a folder ▸` (the account can't
    /// make folders): the deepest folder that listed, `◂ Up to …`, its
    /// folders; the first walking key shows the cursor; a folder goes in;
    /// none inside reads so; Use this folder is for this song.
    #[test]
    fn choose_a_folder_walks_and_uses_one() {
        let _en = english();
        let mut gui = room();
        ytdl::observe(&mut gui, &connected(Ping { no_mkdir: Some(true), ..ping(1) }));
        ytdl::paste(&mut gui, LINK);
        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, discovery_new());
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("choose a folder ▸")), "{}", rows.join("\n"));
        key(&mut gui, KeyCode::Char('c'));
        assert_eq!(answer_listing(&mut gui, &["Homework", "Human After All"]), "/Music/Daft Punk/");
        let b = assert_place(&mut gui, (100, 30), boards::W4, boards::W4_RUNS);
        let rows = lines(&b);
        assert_eq!(gui.ui.hit(at(&rows, "Up to Music")), Some(place(PlaceAct::Up)));
        assert_eq!(gui.ui.hit(at(&rows, "Human After All")), Some(place(PlaceAct::Into(1))));
        assert_eq!(gui.ui.hit(at(&rows, "[ Use this folder ]")), Some(place(PlaceAct::UseFolder)));
        assert_eq!(tips(&gui), "↑ ↓ pick up the cursor · Enter goes in, or uses this folder when stowed · Esc close");
        key(&mut gui, KeyCode::Down);
        let b = buffer(&mut gui, 100, 30);
        let up = "|│ ◂ Up to Music                                                                │";
        assert_frame(&b, (10, 7), 80, up, &["0:2-14:OnAccent"]);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert_eq!(answer_listing(&mut gui, &[]), "/Music/Daft Punk/Homework/");
        if let Some(Place::Choose(c)) = gui.ytdl.place.as_mut() {
            c.view.stow();
        }
        assert_place(&mut gui, (100, 30), boards::W5, boards::W5_RUNS);
        key(&mut gui, KeyCode::Enter);
        assert!(!is_open(&gui), "stowed, Enter uses this folder");
        assert_eq!(gui.ytdl.draft.instead.as_deref(), Some("/Music/Daft Punk/Homework/"));
        answer_folders(&mut gui, FolderCheck { known: true, new_segments: vec![], deepest: Some("/Music/Daft Punk/Homework/".into()), files: Some(vec![]) });
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains("Music › Daft Punk › Homework ▸")), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("can't make folders")), "{}", rows.join("\n"));
        // A folder that won't list on the first look opens its parent.
        key(&mut gui, KeyCode::Char('d'));
        key(&mut gui, KeyCode::Esc);
        gui.ytdl.draft.instead = Some("/Music/Gone/".into());
        open_choose(&mut gui, Form::Room);
        let seq = match gui.ytdl.work.last() {
            Some(Work::Listing { seq, .. }) => *seq,
            other => panic!("{other:?}"),
        };
        listed(&mut gui, seq, Err(YtdlCallError::Answered { status: 404, words: Some("Folder not found".into()), reason: None, file_name: None }));
        assert_eq!(answer_listing(&mut gui, &["Daft Punk"]), "/Music/");
    }

    /// A failed row's Choose a folder stands over the room; the row adds
    /// again into the folder used, and the room is back with it adding.
    #[test]
    fn a_rows_choose_a_folder_adds_again_there() {
        let _en = english();
        let mut gui = room();
        let mut failed = job("f1", "startFailed", "One More Time", "/Music/Daft Punk/Discovery/");
        failed["mkdir"] = "/Music/Daft Punk/Discovery/".into();
        failed["failure"] = serde_json::json!({ "problem": "mkdirOff", "phase": "start" });
        failed["seen"] = false.into();
        seed(&mut gui, vec![failed]);
        let rows = lines(&buffer(&mut gui, 100, 30));
        let choose = at(&rows, "Choose a folder");
        gui.act(gui.ui.hit(choose).expect("the row's way"));
        assert!(is_open(&gui) && ytdl::modal_open(&gui), "over the room");
        // From the folder it was to make: never made, it opens its parent.
        let seq = match gui.ytdl.work.last() {
            Some(Work::Listing { seq, directory }) if directory == "/Music/Daft Punk/Discovery/" => *seq,
            other => panic!("{other:?}"),
        };
        gui.ytdl.work.clear();
        let gone = YtdlCallError::Answered { status: 404, words: Some("Folder not found".into()), reason: None, file_name: None };
        listed(&mut gui, seq, Err(gone));
        assert_eq!(answer_listing(&mut gui, &["Homework"]), "/Music/Daft Punk/");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("─ Choose a folder ─")), "{}", rows.join("\n"));
        gui.act(place(PlaceAct::UseFolder));
        assert!(!is_open(&gui) && ytdl::in_room(&gui), "the room is back");
        let mut reply = |call: &Call| match call {
            Call::Add(_) => Some(Answer::Add(Ok(YtdlStarted { message: None, id: Some("srv2".into()), file_name: None }))),
            Call::Jobs => Some(Answer::Jobs(Ok(Vec::new()))),
            _ => None,
        };
        serve(&mut gui, &mut reply);
        let jobs = gui.ytdl.jobs().unwrap().jobs();
        assert_eq!(jobs[0].directory, "/Music/Daft Punk/", "added again into the folder used");
        assert!(calls(&gui).iter().all(|c| !matches!(c, Call::Mkdir { .. })), "nothing to make");
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("◷ One More Time") && r.contains("starting…")), "its row, on its way:\n{}", rows.join("\n"));
    }

    /// The Files window's Choose a folder takes the window's place and
    /// hands it back with the folder chosen: Add to library, into it.
    #[test]
    fn the_windows_choose_a_folder_comes_back_to_it() {
        let _en = english();
        let mut gui = super::super::ytdl_here::tests::files_gui(1);
        ytdl::observe(&mut gui, &connected(Ping { no_mkdir: Some(true), ..ping(1) }));
        key(&mut gui, KeyCode::Char('y'));
        ytdl::paste(&mut gui, LINK);
        answer_lookup(&mut gui, Ok(one_more_time()));
        answer_folders(&mut gui, super::super::ytdl::tests::there("/Music/Daft Punk/", &[]));
        buffer(&mut gui, 100, 30);
        // Saves here's sheet: the dot on Album, then closed.
        key(&mut gui, KeyCode::Char('s'));
        key(&mut gui, KeyCode::Right);
        key(&mut gui, KeyCode::Enter);
        ytdl::poll(&mut gui);
        super::super::ytdl::tests::advance(&gui, 500);
        answer_folders(&mut gui, new_at(1));
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("! “Discovery” doesn't exist yet and your account can't make folders.")), "{}", rows.join("\n"));
        let choose = at(&rows, "choose a folder ▸");
        assert_eq!(gui.ui.hit(choose), Some(yt(YtAct::Choose)));
        key(&mut gui, KeyCode::Char('c'));
        assert!(is_open(&gui));
        answer_listing(&mut gui, &["Homework", "Human After All"]);
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Choose a folder")) && !rows.iter().any(|r| r.contains("Add from YouTube")));
        gui.act(place(PlaceAct::Into(0)));
        answer_listing(&mut gui, &[]);
        gui.act(place(PlaceAct::UseFolder));
        assert!(super::super::ytdl_here::window_up(&gui) && !is_open(&gui), "the window is back");
        answer_folders(&mut gui, super::super::ytdl::tests::there("/Music/Daft Punk/Homework/", &[]));
        let rows = lines(&buffer(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("Saves to") && r.contains("Music › Daft Punk › Homework ▸")), "{}", rows.join("\n"));
        assert!(rows.iter().any(|r| r.contains("[ Add to library ]")), "{}", rows.join("\n"));
    }
}
