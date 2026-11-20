//! Track actions (docs/ux-contracts/track-actions.md), the GUI's half: the
//! one sheet for a song wherever it is met, its rating on the badge row,
//! the add-to-playlist picker, Song info, and the queue panel's grip drag
//! and keyboard reach. The verbs themselves are the shared App's.
//!
//! The same sheet takes a Peer recs lead in its lead form (peer-recs
//! contract §1.9): a lead has no file and is never "own", so it draws no
//! cover and no stars, and its rows are Preview, Get it (one per acquire
//! plug-in), Open elsewhere… and Copy title, each hidden when its gate
//! fails. The sheet holds its lead by key and finds it again in the room's
//! list on every draw, key and act (build plan D30).

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::types::{JobMark, JobOutcome, TasteLead, Track, TrackMetadata};
use crate::kit::clipboard::Copied;
use crate::kit::theme::{busy_glyph, legacy_conhost, th};
use crate::kit::{Surface, dim, modal_close, modal_frame_on, table_view};
use crate::tui::app::keep::{KeepPin, KeepTarget};
use crate::tui::app::peer_recs::previews::Slot as Clip;
use crate::tui::app::peer_recs::rules;
use crate::tui::app::{Action, App, Entry, Focus, Origin, PlaylistNames, Tab};

use super::cover::{Pace, Slot};
use super::{Act, Gui, SEARCH_NAV, accent, bright_bold, put, sel};

// ── State ───────────────────────────────────────────────────────────────────

/// What the shell keeps: the open sheet, the picker over it, whether Song
/// info is up, and a grip drag in progress.
#[derive(Default)]
pub(crate) struct ActionsUi {
    pub sheet: Option<Sheet>,
    pub picker: Option<Picker>,
    pub info: bool,
    /// The queue row a grip press is dragging (clause 18).
    pub drag: Option<usize>,
    /// The sheet's cover: a slot of its own, pixels while the sheet is the
    /// top layer, the mosaic under the picker, Song Info or a tooltip.
    slot: Option<Slot>,
}

impl ActionsUi {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A resize changes the cell-to-pixel mapping the slot encoded against
    /// — the wall's and the queue's rule, for the sheet's one cover.
    pub(crate) fn on_resize(&mut self) {
        if let Some(slot) = &mut self.slot {
            slot.on_resize();
        }
    }
}

/// The sheet: the track it is for, where it came from, and the cursor over
/// its actions.
#[derive(Debug, Clone)]
pub(crate) struct Sheet {
    pub origin: Origin,
    pub track: Track,
    /// The queue row the sheet was opened from, which gains Remove.
    pub queue_row: Option<usize>,
    pub row: usize,
    /// A Discover room's connected-server row the sheet was opened from:
    /// it gains Keep a copy… and Open elsewhere…, and its badge row names
    /// the server (discovery-page clause 102). Pinned to the row's server
    /// and file, not its place: a peers answer landing while the sheet is
    /// open never retargets them ([`sheet_target`]).
    pub keep: Option<KeepPin>,
    /// Any other connected server's song — a browse row on its session, a
    /// queue item, the playing card — whose own server's gate offered Keep a
    /// copy when the sheet opened (keep-a-copy spec §4.7): the row stands for
    /// as long as the sheet does, and the badge row stays blank.
    pub item: Option<KeepTarget>,
    /// Open elsewhere's list hangs from its row (clause 140).
    pub links: bool,
    /// A Peer recs lead the sheet is for (peer-recs contract §1.9): its
    /// index in the room's shown list and its key. The key is the truth:
    /// [`refind`] follows the lead to its new index, or closes the sheet
    /// once it left the list (build plan D30). `track` is then a stand-in
    /// built from the lead's words, with no file, and every track path
    /// skips it.
    pub lead: Option<(usize, String)>,
    /// The action a lead sheet's cursor is on: the truth `row` is found from again when the
    /// gated rows come or go under it ([`refind`], build plan D30). `None` on a track's sheet.
    pub on: Option<SheetAction>,
}

/// The add-to-playlist picker: its cursor, and the name being typed for a
/// new playlist when New playlist was chosen.
#[derive(Debug, Default)]
pub(crate) struct Picker {
    pub row: usize,
    pub naming: Option<Input>,
}

/// The sheet's rows, in the contract's order (clause 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SheetAction {
    PlayNow,
    AddNext,
    AddEnd,
    AddPlaylist,
    /// A connected server's song in the Discover room: Keep a copy (the
    /// slot Add to playlist holds on your own songs) and Open elsewhere
    /// (the last row: it leaves the app) — discovery-page clause 102.
    KeepCopy,
    Info,
    OpenElsewhere,
    Remove,
    /// A lead's rows (peer-recs contract §1.9): its preview clip, Get it
    /// through the acquire plug-in at its place in the room's Get it rows,
    /// and the lead's `Artist - Title` copied.
    Preview,
    GetIt(usize),
    CopyTitle,
}

impl SheetAction {
    fn label(self) -> String {
        match self {
            SheetAction::PlayNow => t!("gui.act.play_now").to_string(),
            SheetAction::AddNext => t!("gui.act.add_next").to_string(),
            SheetAction::AddEnd => t!("gui.act.add_end").to_string(),
            SheetAction::AddPlaylist => t!("gui.act.add_playlist").to_string(),
            SheetAction::KeepCopy => super::keep::ell(&t!("gui.act.keep_copy")),
            SheetAction::Info => t!("gui.act.info").to_string(),
            SheetAction::OpenElsewhere => super::keep::ell(&t!("gui.act.open_elsewhere")),
            SheetAction::Remove => t!("gui.act.remove").to_string(),
            SheetAction::Preview => t!("gui.peer_recs.preview").to_string(),
            // No ellipsis: the row starts the job at once (peer-recs
            // contract §1.9).
            SheetAction::GetIt(_) => t!("gui.peer_recs.get_it").to_string(),
            SheetAction::CopyTitle => t!("gui.peer_recs.copy").to_string(),
        }
    }
}

/// The sheet's rows (clause 4), and a Discover room connected-server
/// row's two (discovery-page clause 102) — each only while its gate holds,
/// hidden, never grey.
fn actions_for(app: &App, sheet: &Sheet, own: bool) -> Vec<SheetAction> {
    let mut rows = vec![SheetAction::PlayNow, SheetAction::AddNext, SheetAction::AddEnd];
    if own {
        rows.push(SheetAction::AddPlaylist);
    }
    // A new peers answer that left the sheet's row out takes both away —
    // what they would send is gone — except a links list already asked,
    // which hangs on from its row for the row it was asked for. Any other
    // connected server's song keeps the row it opened with (keep-a-copy
    // spec §4.7).
    let target = sheet_target(app, sheet);
    let keep = match sheet.keep {
        Some(_) => target.as_ref().is_some_and(|target| !app.keep_choices(target.clone()).is_empty()),
        None => sheet.item.is_some(),
    };
    if keep {
        rows.push(SheetAction::KeepCopy);
    }
    rows.push(SheetAction::Info);
    if (target.is_some() || (sheet.links && sheet.keep.is_some())) && app.links_gate() {
        rows.push(SheetAction::OpenElsewhere);
    }
    if sheet.queue_row.is_some() {
        rows.push(SheetAction::Remove);
    }
    rows
}

/// Where the sheet's pinned connected-server row stands in the answer on
/// screen now, `None` once a new answer left it out (or for any other
/// sheet).
fn sheet_target(app: &App, sheet: &Sheet) -> Option<KeepTarget> {
    app.keep_find(sheet.keep.as_ref()?)
}

/// What the sheet's Keep a copy… opens on: the Discover row's pin where it
/// still stands, else the connected server's song it opened for.
fn keep_target(app: &App, sheet: &Sheet) -> Option<KeepTarget> {
    match sheet.keep {
        Some(_) => sheet_target(app, sheet),
        None => sheet.item.clone(),
    }
}

pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.actions.sheet.is_some()
}

// ── Opening ─────────────────────────────────────────────────────────────────

/// The sheet for one track (clause 1): the row's own block now, the full
/// one asked of its server (clause 8). `from` is the pane the row is drawn
/// in — `None` for a queue item and the playing card.
pub(crate) fn open_for(gui: &mut Gui, origin: Origin, track: Track, queue_row: Option<usize>, from: Option<Tab>) {
    // A lead's sheet it replaces lets the room's sheet state go.
    if gui.actions.sheet.as_ref().is_some_and(|s| s.lead.is_some()) {
        close(gui);
    }
    let effects = gui.app.fetch_track_info(&origin, &track.filepath);
    gui.pend(effects);
    gui.actions.picker = None;
    gui.actions.info = false;
    // A connected server's song offers Keep a copy… while its own server's
    // gate is open and that server still lists it (keep-a-copy spec §4.7,
    // W2 and W8–W11) — decided now, for as long as the sheet stands; on
    // every screen that opens it (Q9: the playing card's too), the Library
    // screen brought back for the window when it is chosen. A queue item of
    // another own server's connected server, its gate never asked: asked
    // now, for the next time (W10).
    let asked = gui.app.keep_ask_owner(&origin);
    gui.pend(asked);
    let item = gui.app.keep_track(&origin, &track, from).filter(|target| !gui.app.keep_choices(target.clone()).is_empty());
    let sheet = Sheet { origin, track, queue_row, row: 0, keep: None, item, links: false, lead: None, on: None };
    gui.actions.sheet = Some(sheet);
}

// ── The lead form (peer-recs contract §1.9) ─────────────────────────────────

/// The sheet on the Peer recs lead at `index` in the room's shown list:
/// Enter or `m` on a held row, a row's click or right click
/// (`on_get_it` false), and the band's `[ get it ]` (true). Both start on
/// Get it, else Open elsewhere…, else Copy title, so Enter never plays; the
/// band draws `[ get it ]` only with Get it's gate, so its press lands on
/// Get it by that same rule. The room's sheet state (the job lookup) is
/// asked now, keyed by the lead's key.
pub(crate) fn open_lead(gui: &mut Gui, index: usize, on_get_it: bool) {
    let Some(lead) = gui.app.peer_recs_leads().get(index) else { return };
    let key = rules::lead_key(lead);
    let track = stand_in(lead);
    let same = gui.actions.sheet.as_ref().and_then(|s| s.lead.as_ref()).is_some_and(|(_, open)| *open == key);
    if gui.actions.sheet.is_some() && !same {
        close(gui);
    }
    let effects = gui.app.peer_recs_sheet_open(index);
    gui.pend(effects);
    gui.actions.picker = None;
    gui.actions.info = false;
    let rows = lead_rows(&gui.app, false);
    let row = start_row(&rows, on_get_it);
    if same {
        if let Some(sheet) = gui.actions.sheet.as_mut() {
            sheet.lead = Some((index, key));
            if !sheet.links {
                sheet.row = row;
                sheet.on = rows.get(row).copied();
            }
        }
        return;
    }
    let origin = gui.app.origin();
    let lead = Some((index, key));
    let on = rows.get(row).copied();
    gui.actions.sheet =
        Some(Sheet { origin, track, queue_row: None, row, keep: None, item: None, links: false, lead, on });
}

/// The lead the open sheet is for, as its index in the room's shown list
/// now: found again by its key, so a reshuffle under the sheet is read
/// where the lead went. `None` for a track's sheet, no sheet, a lead that
/// has left the list, or a room no longer in front (see [`refind`]).
pub(crate) fn lead_open(gui: &Gui) -> Option<usize> {
    let (_, key) = gui.actions.sheet.as_ref()?.lead.as_ref()?;
    if gui.active != super::PEER_RECS_NAV {
        return None;
    }
    gui.app.peer_recs_lead_index(key)
}

/// A lead's words as a track, for the sheet's `track`: no file, no cover,
/// no rating. Nothing plays, rates, adds or asks the server about it.
fn stand_in(lead: &TasteLead) -> Track {
    Track {
        filepath: String::new(),
        metadata: TrackMetadata {
            title: lead.title.clone(),
            artist: lead.artist.clone(),
            album: lead.album.clone(),
            duration: lead.duration,
            ..TrackMetadata::default()
        },
    }
}

/// A lead's rows (peer-recs contract §1.9), each only while its gate holds,
/// hidden, never grey: Preview with an audio device and a preview plug-in,
/// one Get it per acquire plug-in (the copy plug-in's excepted) with jobs
/// allowed and a destination, Open elsewhere… with a links plug-in (or
/// while its list already hangs), and Copy title always.
fn lead_rows(app: &App, links: bool) -> Vec<SheetAction> {
    let mut rows = Vec::new();
    if app.peer_recs_preview_offered() {
        rows.push(SheetAction::Preview);
    }
    rows.extend((0..app.peer_recs_get_it_rows().len()).map(SheetAction::GetIt));
    if links || app.peer_recs_links_on() {
        rows.push(SheetAction::OpenElsewhere);
    }
    rows.push(SheetAction::CopyTitle);
    rows
}

/// Where a lead's cursor starts: Get it, else Open elsewhere…, else Copy
/// title (peer-recs contract §1.9). The band's `[ get it ]` asks for Get it
/// itself, which is the same row whenever the band could draw its button.
fn start_row(rows: &[SheetAction], on_get_it: bool) -> usize {
    let _ = on_get_it;
    let find = |want: SheetAction| rows.iter().position(|a| *a == want);
    rows.iter()
        .position(|a| matches!(a, SheetAction::GetIt(_)))
        .or_else(|| find(SheetAction::OpenElsewhere))
        .or_else(|| find(SheetAction::CopyTitle))
        .unwrap_or(0)
}

/// A row the cursor can rest on: every lead row but a muted Preview, whose
/// lead has no preview this visit (it reads `No preview`, inert).
fn takes_cursor(app: &App, index: usize, action: SheetAction) -> bool {
    !(action == SheetAction::Preview && app.peer_recs_slot(index) == Clip::Muted)
}

/// The next row from `row` the cursor can rest on, `dir` up or down; the
/// cursor stays where there is none that way.
fn lead_step(app: &App, index: usize, rows: &[SheetAction], row: usize, dir: i64) -> usize {
    let mut at = row as i64 + dir;
    while (0..rows.len() as i64).contains(&at) {
        if takes_cursor(app, index, rows[at as usize]) {
            return at as usize;
        }
        at += dir;
    }
    row
}

/// The lead sheet's lead found again by its key (build plan D30), at the
/// start of every draw, key and act on it. Found at another index, the
/// sheet follows it, and the room's sheet state (keyed by the same key)
/// stands. Gone from the list, the sheet closes, and so it does once the
/// room is no longer the room in front (its yield to Files when the gate
/// goes, build plan D31): the visit and its list are over then, though the
/// last answer still stands in the App. The cursor stays on its action as
/// the gated rows come and go under it; when its own row went, it lands by
/// the start rule (never on Preview). It is held on a row that takes it: a
/// muted Preview passes it on. False once the sheet closed; true for a
/// track's sheet.
fn refind(gui: &mut Gui) -> bool {
    let Some((_, key)) = gui.actions.sheet.as_ref().and_then(|s| s.lead.clone()) else { return true };
    let found = (gui.active == super::PEER_RECS_NAV).then(|| gui.app.peer_recs_lead_index(&key)).flatten();
    let Some(index) = found else {
        close(gui);
        return false;
    };
    let links = gui.actions.sheet.as_ref().is_some_and(|s| s.links);
    let rows = lead_rows(&gui.app, links);
    let Some(sheet) = gui.actions.sheet.as_mut() else { return false };
    sheet.lead = Some((index, key));
    let found = sheet.on.and_then(|on| rows.iter().position(|a| *a == on));
    let mut row = found.unwrap_or_else(|| start_row(&rows, false));
    if !takes_cursor(&gui.app, index, rows[row]) {
        row = lead_step(&gui.app, index, &rows, row, 1);
    }
    sheet.row = row;
    sheet.on = Some(rows[row]);
    true
}

/// The lead index of the open sheet, as [`refind`] last placed it.
fn lead_index(gui: &Gui) -> Option<usize> {
    gui.actions.sheet.as_ref()?.lead.as_ref().map(|(index, _)| *index)
}

/// Copy `text` through the kit's clipboard and say so at the bar's note,
/// dim (peer-recs contract §1.13): `Copied` for the clipboard and for OSC
/// 52, which nothing confirms, `Couldn't copy` when no route took it.
fn copy_noted(gui: &mut Gui, text: Option<String>) {
    let Some(text) = text else { return };
    let words = match crate::kit::clipboard::copy(&text) {
        Copied::Clipboard | Copied::Terminal => t!("gui.peer_recs.copied"),
        Copied::Failed => t!("gui.peer_recs.copy_failed"),
    };
    gui.note = Some((words.to_string(), false));
}

/// A browse row's `[⋯]`, `m` or right click: the pane's track at `index`.
fn open_from_pane(gui: &mut Gui, tab: Tab, index: usize) {
    let track = match gui.app.pane_for(tab).entries.get(index) {
        Some(Entry::Track { track, .. }) => (**track).clone(),
        _ => return,
    };
    let origin = gui.app.origin();
    open_for(gui, origin, track, None, Some(tab));
}

fn open_from_queue(gui: &mut Gui, index: usize) {
    let Some(item) = gui.app.queue.items.get(index) else { return };
    let (origin, track) = (item.origin.clone(), item.track.clone());
    open_for(gui, origin, track, Some(index), None);
}

/// The bar's card: the playing track, with its queue row when it is one.
fn open_for_playing(gui: &mut Gui) {
    let Some(now) = gui.app.now_playing.clone() else { return };
    let current = gui.app.queue.current.filter(|&i| gui.app.queue.items.get(i).is_some_and(|q| q.filepath == now.filepath));
    let origin = current
        .and_then(|i| gui.app.queue.items.get(i).map(|q| q.origin.clone()))
        .unwrap_or_else(|| gui.app.origin());
    open_for(gui, origin, now, current, None);
}

/// `m`: the queue's highlighted row while the panel has the keys, else the
/// browse room's highlighted track, else what is playing — "highlighted"
/// meaning the keyboard holds the cursor (the kit's list-cursor law):
/// nothing lit, nothing aimed at.
fn open_by_key(gui: &mut Gui) {
    if gui.app.focus == Focus::Queue
        && gui.queue_view.held
        && let Some(index) = gui.app.queue.state.selected()
    {
        open_from_queue(gui, index);
        return;
    }
    if gui.browse_room() || gui.active == SEARCH_NAV {
        let tab = if gui.active == SEARCH_NAV { Tab::Search } else { gui.browse_tab() };
        if gui.room_list().is_some_and(|list| gui.list_view(list).held)
            && let Some(index) = gui.app.pane_for(tab).state.selected()
            && matches!(gui.app.pane_for(tab).entries.get(index), Some(Entry::Track { .. }))
        {
            open_from_pane(gui, tab, index);
            return;
        }
    }
    open_for_playing(gui);
}

// ── Words ───────────────────────────────────────────────────────────────────

/// The track the sheet shows: the server's full block once it landed, the
/// row's own until then.
fn current_track<'a>(app: &'a App, sheet: &'a Sheet) -> &'a Track {
    if sheet.lead.is_some() {
        return &sheet.track;
    }
    match &app.track_info {
        Some(info) if info.filepath == sheet.track.filepath => info,
        _ => &sheet.track,
    }
}

fn title_of(track: &Track) -> String {
    track.title_or_file().to_string()
}

fn byline_of(track: &Track) -> String {
    [track.metadata.artist.as_deref(), track.metadata.album.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `FLAC · 320 kbps · 44.1 kHz · 4:12`, each part on its own (clause 2).
fn spec_of(track: &Track) -> String {
    let mut parts = spec_parts(&track.metadata);
    if let Some(seconds) = track.metadata.duration {
        parts.push(super::bar::fmt_time(seconds));
    }
    parts.join(" · ")
}

/// What the file is — format, bitrate, sample rate — as the sheet and the
/// bar's card both word it; the length is each surface's own.
pub(crate) fn spec_parts(m: &TrackMetadata) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(format) = m.format.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        parts.push(format.to_uppercase());
    }
    if let Some(kbps) = m.kbps() {
        parts.push(t!("gui.info.kbps", n = kbps).to_string());
    }
    if let Some(khz) = m.khz_words() {
        parts.push(t!("gui.info.khz", n = khz).to_string());
    }
    parts
}

/// Five stars in halves from the wire's 0–10 (clause 10).
pub(crate) fn stars(rating: Option<u32>) -> String {
    let v = rating.unwrap_or(0).min(10);
    let (full, half, empty) = if legacy_conhost() { ('*', 'h', '-') } else { ('★', '½', '☆') };
    (0..5)
        .map(|i| {
            let full_at = 2 * i + 2;
            let half_at = 2 * i + 1;
            if v >= full_at {
                full
            } else if v >= half_at {
                half
            } else {
                empty
            }
        })
        .collect()
}

/// "3.5", "4" — the record's compact label; empty when unrated.
fn rating_words(rating: Option<u32>) -> String {
    match rating.unwrap_or(0).min(10) {
        0 => String::new(),
        v if v % 2 == 0 => format!("{}", v / 2),
        v => format!("{}.5", v / 2),
    }
}

/// Song info's rows (clause 15): only the facts present, the path last.
fn info_rows(track: &Track) -> Vec<(String, String)> {
    let m = &track.metadata;
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut add = |key: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            let name = format!("gui.info.{key}");
            rows.push((t!(name.as_str()).to_string(), value));
        }
    };
    add("title", Some(title_of(track)));
    add("artist", m.artist.clone());
    add("album", m.album.clone());
    add("year", m.year.filter(|y| *y > 0).map(|y| y.to_string()));
    let of = |n: Option<u32>, total: Option<u32>| match (n, total) {
        (Some(n), Some(total)) => Some(t!("gui.info.of", n = n, total = total).to_string()),
        (Some(n), None) => Some(n.to_string()),
        _ => None,
    };
    add("track", of(m.track, m.track_total));
    add("disc", of(m.disk, m.disc_total));
    add("length", m.duration.map(super::bar::fmt_time));
    add("bpm", m.bpm.filter(|b| *b > 0).map(|b| b.to_string()));
    add("key", m.musical_key.clone());
    add("genre", (!m.genres.is_empty()).then(|| m.genres.join(", ")));
    add("format", m.format.as_deref().map(str::to_uppercase));
    add("bitrate", m.kbps().map(|k| t!("gui.info.kbps", n = k).to_string()));
    add("sample_rate", m.khz_words().map(|k| t!("gui.info.khz", n = k).to_string()));
    add("bit_depth", m.bit_depth.map(|d| t!("gui.info.bits", n = d).to_string()));
    add("channels", m.channels.map(|c| c.to_string()));
    add("size", m.file_size.map(crate::api::types::fmt_bytes));
    add("plays", m.play_count.map(|p| p.to_string()));
    add("rating", m.rating.filter(|r| *r > 0).map(|r| format!("{} {}", stars(Some(r)), rating_words(Some(r)))));
    add("path", Some(track.filepath.clone()));
    rows
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The sheet, then whichever of the picker or Song info stands over it.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // The sheet's cover goes on record while the sheet stands, so the
    // cache keeps it (performance audit #91); a comparison per frame, and
    // a name only when the sheet opens, changes track or closes.
    let cover = gui
        .actions
        .sheet
        .as_ref()
        .filter(|sheet| sheet.lead.is_none())
        .and_then(|sheet| current_track(&gui.app, sheet).metadata.album_art.as_deref());
    if !gui.app.sheet_on_view_is(cover) {
        let cover = cover.map(str::to_string);
        gui.app.set_sheet_on_view(cover);
    }
    gui.discover.links.anchor = None;
    if gui.actions.sheet.is_none() || !refind(gui) {
        return;
    }
    match lead_index(gui) {
        Some(index) => draw_lead(frame, gui, area, index),
        None => draw_sheet(frame, gui, area),
    }
    if gui.actions.picker.is_some() {
        draw_picker(frame, gui, area);
    } else if gui.actions.info {
        draw_info(frame, gui, area);
    } else if gui.actions.sheet.as_ref().is_some_and(|s| s.links) {
        // Open elsewhere's list hangs from its row (discovery-page clause
        // 140), over the sheet.
        super::keep::draw_links(frame, gui);
    }
}

/// The modal's own cells are inert: a click INSIDE the frame — on its
/// title, a badge, the typed name — does nothing, and only the backdrop
/// closes it (track-actions clause 9). Registered right after the frame so
/// the rows and buttons drawn after it win their cells.
fn guard_frame(ui: &mut crate::kit::Surface<Act>, inner: Rect) {
    let frame = Rect {
        x: inner.x.saturating_sub(1),
        y: inner.y.saturating_sub(1),
        width: inner.width + 2,
        height: inner.height + 2,
    };
    ui.click(frame, Act::Guard);
}

fn draw_sheet(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // Drawn from the sheet in place: the track and its decoded cover are
    // read each frame, not copied.
    let Gui { app, actions, ui, hot, discover, .. } = &mut *gui;
    let Some(sheet) = actions.sheet.as_ref() else { return };
    ui.click(area, Act::SheetClose);
    let track = current_track(app, sheet);
    let own = app.track_is_own(&sheet.origin);
    let rows = actions_for(app, sheet, own);
    let art = track.metadata.album_art.as_deref().and_then(|file| app.art.get(file)).and_then(|a| a.as_ref());
    let width: u16 = 66.min(area.width.saturating_sub(2)).max(44);
    let height = (3 + 1 + 1 + rows.len() + 1) as u16 + 2;
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    guard_frame(ui, inner);
    modal_close(frame, ui, inner, Act::SheetClose);

    // The header (clause 2): the cover when there is one, the words beside.
    let mut tx = inner.x + 1;
    if let Some(art) = art {
        let cover = Rect { x: inner.x + 1, y: inner.y, width: 6, height: 3 };
        let slot = actions.slot.get_or_insert_with(|| Slot::new(app.graphics.fork()));
        // Pixels while the sheet is the top layer. Under the picker, Song
        // Info or a tooltip the mosaic stands in: a picture's cells are
        // skipped for the terminal writer, so text over them would never
        // land. The sheet's own frame always covers its cover, which is
        // why the question excludes it.
        let own = crate::kit::modal_rect(area, width, height, height);
        let under = actions.picker.is_some() || actions.info || ui.covered_last_frame_by_another(cover, own);
        if under {
            slot.draw_mosaic(frame, cover, art);
        } else {
            let pace = Pace::frame();
            slot.draw_paced(frame, cover, art, &pace);
            // A slot the budget turned away wants the very next frame.
            if pace.starved() {
                *hot = true;
            }
        }
        tx = inner.x + 8;
    }
    let text_w = inner.right().saturating_sub(tx + 4) as usize;
    put(frame, tx, inner.y, &super::bar::clip(&title_of(track), text_w), Style::default().add_modifier(Modifier::BOLD));
    let byline = byline_of(track);
    if !byline.is_empty() {
        put(frame, tx, inner.y + 1, &super::bar::clip(&byline, text_w), dim());
    }
    let spec = spec_of(track);
    if !spec.is_empty() {
        put(frame, tx, inner.y + 2, &super::bar::clip(&spec, text_w), dim());
    }

    // The badge row (clause 3): the rating first, then the facts.
    let by = inner.y + 3;
    let mut bx = inner.x + 1;
    if own {
        // The App's copy: a write lands there at once and a refused one
        // reverts there (clause 11). The sheet's own block speaks only for
        // a track the App no longer holds a copy of.
        let rating = app.rating_known(&track.filepath).unwrap_or(track.metadata.rating);
        let glyphs = stars(rating);
        let v = rating.unwrap_or(0).min(10);
        for (i, glyph) in glyphs.chars().enumerate() {
            let cell = Rect { x: bx + i as u16, y: by, width: 1, height: 1 };
            let full = 2 * (i as u32 + 1);
            let lit = v >= full - 1;
            let hover = ui.hovers(cell);
            let style = match (hover, lit) {
                (true, _) => bright_bold(),
                (false, true) => Style::default().fg(th().gold),
                (false, false) => dim(),
            };
            put(frame, cell.x, by, &glyph.to_string(), style);
            // The star that is the rating clears it; any other sets it.
            ui.click(cell, Act::Rate(if v == full { 0 } else { full }));
            ui.tip(cell, t!("gui.act.rate").to_string());
        }
        bx += 6;
        let words = rating_words(rating);
        if words.is_empty() {
            put(frame, bx, by, &t!("gui.act.rate"), dim());
            bx += t!("gui.act.rate").chars().count() as u16 + 3;
        } else {
            put(frame, bx, by, &words, Style::default().fg(th().gold));
            bx += words.chars().count() as u16 + 3;
        }
    }
    let mut badge = |frame: &mut Frame, text: String| {
        if bx + text.chars().count() as u16 + 1 < inner.right() {
            put(frame, bx, by, &text, dim());
            bx += text.chars().count() as u16 + 3;
        }
    };
    if let Some(key) = track.metadata.musical_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        badge(frame, format!("{} {key}", if legacy_conhost() { "key" } else { "♪" }));
    }
    if let Some(bpm) = track.metadata.bpm.filter(|b| *b > 0) {
        badge(frame, format!("{bpm} BPM"));
    }
    if track.metadata.has_lyrics && own {
        badge(frame, t!("gui.act.lyrics").to_string());
    }
    // A Discover room connected-server song names its server, as the
    // webapp's window does: "from Stage Peer B   • online" — the dot OK,
    // the words DIM; no stars, a rating is your own server's (clause 102).
    if let Some(server) = sheet.keep.as_ref().map(|pin| pin.server().to_string()) {
        let from = t!("gui.act.from_server", server = server).to_string();
        let from_w = crate::kit::width(&from) as u16;
        if bx + from_w < inner.right() {
            put(frame, bx, by, &from, dim());
            bx += from_w + 3;
            if bx + 2 + crate::kit::width(&t!("gui.act.online")) as u16 <= inner.right() {
                put(frame, bx, by, super::bullet_glyph(), Style::default().fg(th().ok));
                put(frame, bx + 2, by, &t!("gui.act.online"), dim());
            }
        }
    }

    // The actions (clause 4), one row each; a click acts, as the record's
    // rows do.
    for (i, action) in rows.iter().enumerate() {
        let y = inner.y + 5 + i as u16;
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        // Open elsewhere's label wears the accent while its list hangs
        // from it, the list holding the keyboard's slab (clause 140).
        let open_list = *action == SheetAction::OpenElsewhere && sheet.links;
        let is_sel = sheet.row == i && !open_list;
        let hover = ui.hovers(rect);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover, *action) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => bright_bold(),
            (false, false, SheetAction::Remove) => Style::default().fg(th().danger),
            (false, false, _) if open_list => accent().add_modifier(Modifier::BOLD),
            // Keep a copy stands out in the accent, its key unnamed.
            (false, false, SheetAction::KeepCopy) => accent(),
            (false, false, _) => Style::default(),
        };
        let label = action.label();
        put(frame, inner.x + 1, y, &label, style);
        // Where Open elsewhere's list hangs from: this row, beside its
        // label (clause 140).
        if open_list {
            discover.links.anchor = Some((inner, y, inner.x + 1 + crate::kit::width(&label) as u16));
        }
        ui.click(rect, Act::SheetVerb(*action));
    }
}

/// The sheet's lead form (peer-recs contract §1.9; DS M3 and L1–L4): the
/// track sheet's frame, `[X]` and rows, with a lead's head and rows. No
/// cover (a lead has no art) and no stars (a lead is never "own"): the
/// title BOLD, the byline and the length dim, then `from {peer}` and, only
/// while the peers poll says so, `• online`.
fn draw_lead(frame: &mut Frame, gui: &mut Gui, area: Rect, index: usize) {
    let Gui { app, actions, ui, discover, .. } = &mut *gui;
    let Some(sheet) = actions.sheet.as_ref() else { return };
    let Some(lead) = app.peer_recs_leads().get(index) else { return };
    ui.click(area, Act::SheetClose);
    let rows = lead_rows(app, sheet.links);
    let width: u16 = 66.min(area.width.saturating_sub(2)).max(44);
    let height = (3 + 1 + 1 + rows.len() + 1) as u16 + 2;
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    guard_frame(ui, inner);
    modal_close(frame, ui, inner, Act::SheetClose);

    let tx = inner.x + 1;
    let text_w = inner.right().saturating_sub(tx + 4) as usize;
    let words = |v: &Option<String>| v.as_deref().map(str::trim).filter(|w| !w.is_empty()).map(str::to_string);
    let title = words(&lead.title).unwrap_or_default();
    put(frame, tx, inner.y, &super::bar::clip(&title, text_w), Style::default().add_modifier(Modifier::BOLD));
    let byline = [words(&lead.artist), words(&lead.album)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    if !byline.is_empty() {
        put(frame, tx, inner.y + 1, &super::bar::clip(&byline, text_w), dim());
    }
    // The spec line is the length alone: a lead has no format.
    if let Some(seconds) = lead.duration.filter(|d| d.is_finite() && *d > 0.0) {
        put(frame, tx, inner.y + 2, &super::bar::fmt_time(seconds), dim());
    }
    // The badge row: the peer that has it, then, 3 cells on, `• online`
    // while the latest poll says the peer is online, one cell short of the
    // frame. A peer's label is a stranger's, of any length: it is clipped
    // to what the row has left once the mark's cells are kept, so both
    // always stand.
    let by = inner.y + 3;
    if let Some(peer) = words(&lead.peer.label).or_else(|| words(&lead.peer.name)) {
        let online = lead.peer.endpoint_id.as_deref().is_some_and(|id| app.peer_recs_online(id));
        let online_word = t!("gui.act.online");
        let mark_w = 1 + if online { 3 + 2 + crate::kit::width(&online_word) } else { 0 };
        let row_w = usize::from(inner.right().saturating_sub(tx));
        let frame_w = crate::kit::width(&t!("gui.act.from_server", server = ""));
        let label_w = row_w.saturating_sub(mark_w + frame_w);
        let from = t!("gui.act.from_server", server = super::bar::clip(&peer, label_w)).to_string();
        let from_w = crate::kit::width(&from) as u16;
        if label_w > 0 && tx + from_w < inner.right() {
            put(frame, tx, by, &from, dim());
            let ox = tx + from_w + 3;
            if online && ox + 2 + (crate::kit::width(&online_word) as u16) < inner.right() {
                put(frame, ox, by, super::bullet_glyph(), Style::default().fg(th().ok));
                put(frame, ox + 2, by, &online_word, dim());
            }
        }
    }

    let get_its = app.peer_recs_get_it_rows();
    for (i, action) in rows.iter().enumerate() {
        let y = inner.y + 5 + i as u16;
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let open_list = *action == SheetAction::OpenElsewhere && sheet.links;
        let live = takes_cursor(app, index, *action);
        let is_sel = sheet.row == i && !open_list && live;
        let hover = live && ui.hovers(rect);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let on = sel().add_modifier(Modifier::BOLD);
        let style = match (is_sel, hover, *action) {
            (true, _, _) => on,
            (false, true, _) => bright_bold(),
            (false, false, _) if open_list => accent().add_modifier(Modifier::BOLD),
            // Get it stands out in the accent at rest, as Keep a copy… does.
            (false, false, SheetAction::GetIt(_)) => accent(),
            (false, false, _) => Style::default(),
        };
        match *action {
            SheetAction::Preview => {
                // The glyph follows the clip, at the row's first cell; the
                // word 3 cells on. Muted, the row reads `▸ No preview` dim.
                let (glyph, token) = match app.peer_recs_slot(index) {
                    Clip::Idle => (super::forward_glyph(), accent()),
                    Clip::Muted => (super::forward_glyph(), dim()),
                    Clip::Busy => (busy_glyph(), Style::default().fg(th().gold)),
                    Clip::Playing => (super::bar::play_glyphs(false).1, accent().add_modifier(Modifier::BOLD)),
                    Clip::Paused => (super::forward_glyph(), accent().add_modifier(Modifier::BOLD)),
                };
                let (word, word_style) = if live {
                    (action.label(), style)
                } else {
                    (t!("gui.peer_recs.no_preview").to_string(), dim())
                };
                put(frame, inner.x + 1, y, glyph, if is_sel { on } else { token });
                let room = inner.width.saturating_sub(5) as usize;
                put(frame, inner.x + 4, y, &super::bar::clip(&word, room), word_style);
            }
            SheetAction::GetIt(n) => {
                let label = action.label();
                put(frame, inner.x + 1, y, &label, style);
                let row = get_its.get(n);
                // With several acquire plug-ins, each row names its own, dim
                // 2 cells after the word (DS §8.6).
                if let Some(name) = row.and_then(|r| r.title.as_deref()) {
                    let x = inner.x + 1 + crate::kit::width(&label) as u16 + 2;
                    let room = inner.right().saturating_sub(x + 16) as usize;
                    put(frame, x, y, &super::bar::clip(name, room), if is_sel { on } else { dim() });
                }
                // The job's word stands at the right, ending 2 cells before
                // the border: `downloading…` for a running job (Get it is
                // never a federation copy), every other mark in its own
                // word and token. Not while Open elsewhere's list hangs,
                // which would cut it. A finished job whose result is the
                // plug-in's own reads kept: a Get it lands in the
                // collection destination (the web's "done").
                let mark = row.and_then(|r| r.mark.as_ref()).and_then(|outcome| match outcome {
                    JobOutcome::Done => Some(JobMark::Kept),
                    other => other.mark(),
                });
                if let Some(mark) = mark.filter(|_| !sheet.links) {
                    let word = match mark {
                        JobMark::Copying => super::keep::ell(&t!("gui.peer_recs.downloading")),
                        other => super::keep::mark_word(other),
                    };
                    let x = inner.right().saturating_sub(1 + crate::kit::width(&word) as u16);
                    put(frame, x, y, &word, if is_sel { on } else { super::keep::mark_style(mark) });
                }
            }
            _ => {
                let label = action.label();
                put(frame, inner.x + 1, y, &super::bar::clip(&label, inner.width.saturating_sub(2) as usize), style);
                // Where Open elsewhere's list hangs from (clause 140).
                if open_list {
                    discover.links.anchor = Some((inner, y, inner.x + 1 + crate::kit::width(&label) as u16));
                }
            }
        }
        // A muted Preview takes no press: the frame's guard holds its cells.
        if live {
            ui.click(rect, Act::SheetVerb(*action));
        }
    }
}

fn draw_picker(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Gui { app, actions, ui, .. } = &mut *gui;
    let Some(sheet) = actions.sheet.as_ref() else { return };
    let (row, naming) = match &actions.picker {
        Some(p) => (p.row, p.naming.as_ref().map(|i| (i.value().to_string(), i.cursor()))),
        None => return,
    };
    ui.click(area, Act::PickClose);
    let names = app.playlist_names.listed();
    let failed = matches!(app.playlist_names, PlaylistNames::Failed);
    let width: u16 = 52.min(area.width.saturating_sub(2)).max(36);
    if let Some((name, cursor)) = naming {
        // New playlist: the name, then the add (clause 12) — Enter, or the
        // Create button beside the field for the pointer.
        let inner = modal_frame_on(frame, ui, area, width, 7, th().accent);
        guard_frame(ui, inner);
        put(frame, inner.x + 1, inner.y, &t!("gui.pl.new"), accent().add_modifier(Modifier::BOLD));
        modal_close(frame, ui, inner, Act::PickClose);
        let label = t!("gui.pl.create").to_string();
        let button_w = crate::kit::width(&label) as u16 + 4;
        let field_w = inner.width.saturating_sub(3 + button_w);
        super::text_field(frame, ui, inner.x + 1, inner.y + 2, &name, cursor, field_w, Style::default());
        let at = Rect { x: inner.x + 2 + field_w, y: inner.y + 2, width: button_w, height: 1 };
        crate::kit::button(frame, ui, at, &label, true, Act::PickCreate);
        put(frame, inner.x + 1, inner.y + 4, &t!("gui.act.name_hint", title = title_of(&sheet.track)), dim());
        return;
    }
    let listed = names.map_or(0, <[String]>::len);
    let rows = 1 + listed;
    let max_rows = area.height.saturating_sub(8) as usize;
    let shown_rows = rows.min(max_rows.max(1));
    let height = (shown_rows + 2 + usize::from(listed == 0)) as u16 + 2;
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    guard_frame(ui, inner);
    put(frame, inner.x + 1, inner.y, &t!("gui.act.add_playlist"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, ui, inner, Act::PickClose);
    let mut y = inner.y + 2;
    let mut line = |frame: &mut Frame, ui: &mut Surface<Act>, index: usize, label: &str, act: Act, lead: bool| {
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let is_sel = row == index;
        let hover = ui.hovers(rect);
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover, lead) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => bright_bold(),
            (false, false, true) => accent(),
            (false, false, false) => Style::default(),
        };
        put(frame, inner.x + 1, y, &super::bar::clip(label, inner.width as usize - 2), style);
        ui.click(rect, act);
        y += 1;
    };
    line(frame, ui, 0, &format!("+ {}", t!("gui.pl.new")), Act::PickNew, true);
    match names {
        Some(names) => {
            // The window keeps the cursor's row in view — the kit's
            // viewport, revealed from the top every frame.
            let (first, visible) = table_view(names.len(), Some(row.saturating_sub(1)), 0, shown_rows.saturating_sub(1));
            for (i, name) in names.iter().enumerate().skip(first).take(visible) {
                line(frame, ui, i + 1, name, Act::PickPlaylist(name.clone()), false);
            }
            if names.is_empty() {
                put(frame, inner.x + 1, y, &super::bar::clip(&t!("gui.act.no_playlists"), inner.width as usize - 2), dim());
            }
        }
        None => {
            let words = if failed { t!("gui.act.playlists_failed") } else { t!("busy.listing") };
            put(frame, inner.x + 1, y, &words, dim());
        }
    }
}

fn draw_info(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Gui { app, actions, ui, .. } = &mut *gui;
    let Some(sheet) = actions.sheet.as_ref() else { return };
    ui.click(area, Act::InfoClose);
    let track = current_track(app, sheet);
    let rows = info_rows(track);
    let width: u16 = 70.min(area.width.saturating_sub(2)).max(40);
    let height = (rows.len() as u16 + 2 + 2).min(area.height.saturating_sub(2));
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    guard_frame(ui, inner);
    put(frame, inner.x + 1, inner.y, &t!("gui.act.info"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, ui, inner, Act::InfoClose);
    let label_w: u16 = 14;
    for (i, (label, value)) in rows.iter().enumerate() {
        let y = inner.y + 2 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        put(frame, inner.x + 1, y, &super::bar::clip(label, label_w as usize - 1), dim());
        let avail = inner.width.saturating_sub(label_w + 2) as usize;
        put(frame, inner.x + 1 + label_w, y, &super::bar::clip(value, avail), Style::default());
    }
}

// ── Acting ──────────────────────────────────────────────────────────────────

fn close(gui: &mut Gui) {
    if gui.actions.sheet.as_ref().is_some_and(|s| s.links) {
        gui.app.links_close();
    }
    // A lead's sheet: the room's sheet state goes, and a re-ask the sheet
    // held runs now (peer-recommendations plan §3.5 R4).
    if gui.actions.sheet.as_ref().is_some_and(|s| s.lead.is_some()) {
        let effects = gui.app.peer_recs_sheet_close();
        gui.pend(effects);
    }
    gui.actions.sheet = None;
    gui.actions.picker = None;
    gui.actions.info = false;
}

fn run(gui: &mut Gui, action: SheetAction) {
    if !refind(gui) {
        return;
    }
    if let Some(index) = lead_index(gui) {
        run_lead(gui, action, index);
        return;
    }
    let Some(sheet) = gui.actions.sheet.clone() else { return };
    let track = current_track(&gui.app, &sheet).clone();
    match action {
        SheetAction::PlayNow => {
            let effects = gui.app.queue_track_next(&sheet.origin, track, true);
            gui.pend(effects);
            close(gui);
        }
        SheetAction::AddNext => {
            let effects = gui.app.queue_track_next(&sheet.origin, track, false);
            gui.pend(effects);
            close(gui);
        }
        SheetAction::AddEnd => {
            let effects = gui.app.queue_track_end(&sheet.origin, track);
            gui.pend(effects);
            close(gui);
        }
        SheetAction::AddPlaylist => {
            let effects = gui.app.fetch_playlist_names(&sheet.origin);
            gui.pend(effects);
            gui.actions.picker = Some(Picker::default());
        }
        SheetAction::Info => gui.actions.info = true,
        // The sheet gives way to the Keep a copy modal (clause 102).
        SheetAction::KeepCopy => {
            if let Some(target) = keep_target(&gui.app, &sheet) {
                close(gui);
                // The window stands over the Library's rooms: the playing
                // card's sheet on another screen brings that screen back
                // first (Q9).
                if gui.screen != super::Screen::Library {
                    gui.act(Act::Screen(super::Screen::Library));
                }
                if gui.screen == super::Screen::Library {
                    super::keep::open(gui, target);
                }
            }
        }
        // Open elsewhere's list hangs from its row, asked now (clause 140).
        SheetAction::OpenElsewhere => {
            if let Some(target) = sheet_target(&gui.app, &sheet) {
                let effects = gui.app.links_open(target);
                gui.pend(effects);
                // Opened by the pointer: nothing held (the keys hold the
                // first link, `handle_key`).
                gui.discover.links.reset();
                if let Some(sheet) = gui.actions.sheet.as_mut() {
                    sheet.links = true;
                }
            }
        }
        SheetAction::Remove => {
            if let Some(index) = sheet.queue_row {
                let effects = gui.app.remove_queue_row(index);
                gui.pend(effects);
            }
            close(gui);
        }
        // A lead's rows: never on a track's sheet.
        SheetAction::Preview | SheetAction::GetIt(_) | SheetAction::CopyTitle => {}
    }
}

/// A lead's row, its lead at `index` as [`refind`] just found it. Preview
/// toggles the clip and Get it starts its job, the sheet staying; Open
/// elsewhere… hangs Keep's list fed the raw lead (build plan D7); Copy
/// title copies and closes. A track's verbs do nothing here.
fn run_lead(gui: &mut Gui, action: SheetAction, index: usize) {
    match action {
        SheetAction::Preview => {
            // Only while the room is drawn under the sheet (peer-recs contract §1.8).
            let effects = super::peer_recs::press(gui, index, crate::clock::Instant::now());
            gui.pend(effects);
        }
        SheetAction::GetIt(row) => {
            let effects = gui.app.peer_recs_get_it(row);
            gui.pend(effects);
        }
        SheetAction::OpenElsewhere => {
            let effects = gui.app.peer_recs_open_elsewhere(index);
            if effects.is_empty() {
                return;
            }
            gui.pend(effects);
            // Opened by the pointer: nothing held (the keys hold the
            // first link, `lead_key`).
            gui.discover.links.reset();
            if let Some(sheet) = gui.actions.sheet.as_mut() {
                sheet.links = true;
            }
        }
        SheetAction::CopyTitle => {
            let text = gui.app.peer_recs_copy_text(index);
            copy_noted(gui, text);
            close(gui);
        }
        SheetAction::PlayNow
        | SheetAction::AddNext
        | SheetAction::AddEnd
        | SheetAction::AddPlaylist
        | SheetAction::KeepCopy
        | SheetAction::Info
        | SheetAction::Remove => {}
    }
}

fn rate(gui: &mut Gui, rating: u32) {
    // A lead is never "own": it has no file to rate.
    let Some(sheet) = gui.actions.sheet.clone().filter(|s| s.lead.is_none()) else { return };
    let rating = (rating > 0).then_some(rating.min(10));
    // No copy of its own: the App's is what the sheet shows, so a refused
    // write's revert reaches the stars too (clause 11).
    let effects = gui.app.rate_track(&sheet.origin, &sheet.track.filepath, rating);
    gui.pend(effects);
}

/// The verbs' side of [`Gui::act`]. True when the act was ours.
pub(crate) fn act(gui: &mut Gui, act: &Act) -> bool {
    match act.clone() {
        Act::More(tab, index) => open_from_pane(gui, tab, index),
        Act::QueueMore(index) => open_from_queue(gui, index),
        Act::NowMore => open_for_playing(gui),
        Act::MoreKey => open_by_key(gui),
        Act::SheetVerb(action) => run(gui, action),
        Act::Rate(rating) => rate(gui, rating),
        Act::SheetClose => close(gui),
        Act::PickPlaylist(name) => {
            if let Some(sheet) = gui.actions.sheet.clone().filter(|s| s.lead.is_none()) {
                let effects = gui.app.add_to_playlist(&sheet.origin, &sheet.track.filepath, &name);
                gui.pend(effects);
            }
            close(gui);
        }
        Act::PickNew => {
            if let Some(picker) = gui.actions.picker.as_mut() {
                picker.naming = Some(Input::default());
            }
        }
        Act::PickCreate => {
            let name = gui
                .actions
                .picker
                .as_ref()
                .and_then(|p| p.naming.as_ref())
                .map(|input| input.value().trim().to_string())
                .filter(|name| !name.is_empty());
            if let Some(name) = name {
                gui.act(Act::PickPlaylist(name));
            }
        }
        Act::PickClose => gui.actions.picker = None,
        Act::InfoClose => gui.actions.info = false,
        // Saved soon: Clear pins when the queue is cleared may have let
        // the seed lists' pins go (auto-dj clause 66).
        Act::QueueClear => {
            gui.forward(Action::ClearQueue);
            gui.save_soon();
        }
        Act::QueueGrip(index) => {
            gui.app.focus = Focus::Queue;
            gui.app.queue.state.select(Some(index));
            // The pointer's act: nothing lit (the kit's list-cursor law).
            gui.queue_view.stow();
            gui.actions.drag = Some(index);
        }
        _ => return false,
    }
    true
}

/// A grip drag crossing rows (clause 18).
pub(crate) fn drag_to(gui: &mut Gui, at: Position) {
    let Some(from) = gui.actions.drag else { return };
    if !gui.queue_open || at.x < gui.queue_panel_x() {
        return;
    }
    let Some(to) = super::queue::row_at(gui, at.y) else { return };
    if to != from {
        gui.app.drag_queue_row(from, to);
        gui.actions.drag = Some(to);
    }
}

pub(crate) fn drop(gui: &mut Gui) {
    gui.actions.drag = None;
}

// ── Keys ────────────────────────────────────────────────────────────────────

/// The sheet's keys and its layers' — they own the keyboard while up.
pub(crate) fn handle_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    gui.actions.sheet.as_ref()?;
    if gui.actions.info {
        if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
            gui.actions.info = false;
        }
        return Some(false);
    }
    if let Some(picker) = gui.actions.picker.as_mut() {
        if let Some(input) = picker.naming.as_mut() {
            match key.code {
                KeyCode::Esc => picker.naming = None,
                KeyCode::Enter => {
                    let name = input.value().trim().to_string();
                    if !name.is_empty() {
                        return Some(gui.act(Act::PickPlaylist(name)));
                    }
                }
                _ => {
                    input.handle_event(&TermEvent::Key(key));
                }
            }
            return Some(false);
        }
        let listed = gui.app.playlist_names.listed().map_or(0, <[String]>::len);
        match key.code {
            KeyCode::Esc => gui.actions.picker = None,
            KeyCode::Up => picker.row = picker.row.saturating_sub(1),
            KeyCode::Down => picker.row = (picker.row + 1).min(listed),
            KeyCode::Enter => {
                let row = picker.row;
                if row == 0 {
                    return Some(gui.act(Act::PickNew));
                }
                let name = gui.app.playlist_names.listed().and_then(|names| names.get(row - 1).cloned());
                if let Some(name) = name {
                    return Some(gui.act(Act::PickPlaylist(name)));
                }
            }
            _ => {}
        }
        return Some(false);
    }
    if !refind(gui) {
        return Some(false);
    }
    if let Some(index) = lead_index(gui) {
        return lead_key(gui, key, index);
    }
    let own = gui.actions.sheet.as_ref()?.origin.peer.is_none();
    let actions = actions_for(&gui.app, gui.actions.sheet.as_ref()?, own);
    let sheet = gui.actions.sheet.as_mut()?;
    let rating = gui.app.rating_known(&sheet.track.filepath).unwrap_or(sheet.track.metadata.rating).unwrap_or(0).min(10);
    let on_links_row = actions.get(sheet.row.min(actions.len() - 1)) == Some(&SheetAction::OpenElsewhere);
    match key.code {
        // Open elsewhere's list goes first: Esc or ← folds it, the sheet
        // stays; ↑ ↓ walk its links and Enter opens the held one (clause
        // 140).
        KeyCode::Esc | KeyCode::Left if sheet.links => {
            sheet.links = false;
            gui.app.links_close();
        }
        KeyCode::Up if sheet.links => super::keep::links_walk(gui, -1),
        KeyCode::Down if sheet.links => super::keep::links_walk(gui, 1),
        KeyCode::Enter if sheet.links => super::keep::links_enter(gui),
        // Enter or → on Open elsewhere… opens its list, the first link
        // held.
        KeyCode::Enter | KeyCode::Right if on_links_row => {
            let quit = gui.act(Act::SheetVerb(SheetAction::OpenElsewhere));
            gui.discover.links.held = Some(0);
            return Some(quit);
        }
        KeyCode::Esc => close(gui),
        // Keep a copy from anywhere in the sheet (clause 102).
        KeyCode::Char('c') if actions.contains(&SheetAction::KeepCopy) => {
            return Some(gui.act(Act::SheetVerb(SheetAction::KeepCopy)));
        }
        KeyCode::Up => sheet.row = sheet.row.saturating_sub(1),
        KeyCode::Down => sheet.row = (sheet.row + 1).min(actions.len().saturating_sub(1)),
        KeyCode::Enter => {
            let action = actions[sheet.row.min(actions.len() - 1)];
            return Some(gui.act(Act::SheetVerb(action)));
        }
        // The stars (clause 10): a whole star down or up, digits set, 0 clears.
        KeyCode::Left if own => return Some(gui.act(Act::Rate(rating.saturating_sub(2) / 2 * 2))),
        KeyCode::Right if own => return Some(gui.act(Act::Rate(((rating + 2) / 2 * 2).min(10)))),
        KeyCode::Char('0') if own => return Some(gui.act(Act::Rate(0))),
        KeyCode::Char(c @ '1'..='5') if own => {
            return Some(gui.act(Act::Rate(2 * (c as u32 - '0' as u32))));
        }
        _ => {}
    }
    Some(false)
}

/// A lead sheet's keys (peer-recs contract §1.9): the track sheet's, less
/// the stars and Keep a copy's `c` — a lead is never "own", so ← → and
/// 0–5 rate nothing. ↑ ↓ pass over a muted Preview. Open elsewhere's list
/// goes first while it hangs, as on a track's sheet. The room's own keys
/// do nothing here: the Preview row is the clip's control.
fn lead_key(gui: &mut Gui, key: KeyEvent, index: usize) -> Option<bool> {
    let links = gui.actions.sheet.as_ref()?.links;
    let rows = lead_rows(&gui.app, links);
    let row = gui.actions.sheet.as_ref()?.row.min(rows.len() - 1);
    let on_links_row = rows.get(row) == Some(&SheetAction::OpenElsewhere);
    match key.code {
        KeyCode::Esc | KeyCode::Left if links => {
            if let Some(sheet) = gui.actions.sheet.as_mut() {
                sheet.links = false;
            }
            gui.app.links_close();
        }
        KeyCode::Up if links => super::keep::links_walk(gui, -1),
        KeyCode::Down if links => super::keep::links_walk(gui, 1),
        KeyCode::Enter if links => super::keep::links_enter(gui),
        // Enter or → on Open elsewhere… opens its list, the first link
        // held.
        KeyCode::Enter | KeyCode::Right if on_links_row => {
            let quit = gui.act(Act::SheetVerb(SheetAction::OpenElsewhere));
            if gui.actions.sheet.as_ref().is_some_and(|s| s.links) {
                gui.discover.links.held = Some(0);
            }
            return Some(quit);
        }
        KeyCode::Esc => close(gui),
        KeyCode::Up | KeyCode::Down => {
            let dir = if key.code == KeyCode::Up { -1 } else { 1 };
            let next = lead_step(&gui.app, index, &rows, row, dir);
            if let Some(sheet) = gui.actions.sheet.as_mut() {
                sheet.row = next;
                sheet.on = Some(rows[next]);
            }
        }
        KeyCode::Enter => return Some(gui.act(Act::SheetVerb(rows[row]))),
        _ => {}
    }
    Some(false)
}

/// The queue panel's keys while it has the keyboard (clause 19).
pub(crate) fn queue_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !(gui.queue_open && gui.app.focus == Focus::Queue) {
        return None;
    }
    // A dialog taking text owns the keyboard outright: a playlist's name
    // must not clear the queue because it held a `C`. So does a Discover
    // room's list or modal — its ↑ ↓ and Esc are its own (discovery-page
    // clauses 103, 106).
    if super::playlists::modal_open(gui)
        || super::sonic::modal_open(gui)
        || (gui.screen == super::Screen::Library && gui.active == super::DISCOVER_NAV && super::discover::modal_open(gui))
        || (gui.screen == super::Screen::Library
            && gui.active == super::PEER_RECS_NAV
            && super::peer_recs::modal_open(gui))
        || super::ytdl::modal_open(gui)
    {
        return None;
    }
    // The kit's list-cursor law: a walking key picks the cursor up on the
    // row the panel rests on, then walks; the row verbs want it up; Esc
    // stows it with the hand-back.
    let held = gui.queue_view.held;
    match key.code {
        KeyCode::Down => walk_queue(gui, Action::Down),
        KeyCode::Up => walk_queue(gui, Action::Up),
        KeyCode::PageDown => walk_queue(gui, Action::PageDown),
        KeyCode::PageUp => walk_queue(gui, Action::PageUp),
        KeyCode::Enter if held => gui.forward(Action::Activate),
        KeyCode::Char('d') | KeyCode::Delete if held => gui.forward(Action::RemoveFromQueue),
        KeyCode::Char('C') => {
            gui.forward(Action::ClearQueue);
            gui.save_soon();
        }
        KeyCode::Char('<') if held => gui.forward(Action::MoveQueueUp),
        KeyCode::Char('>') if held => gui.forward(Action::MoveQueueDown),
        KeyCode::Char('i') => {
            gui.queue_view.pick_up();
            gui.forward(Action::JumpToPlaying);
        }
        KeyCode::Char('m') => return Some(gui.act(Act::MoreKey)),
        // Not bound on the held queue row: `C`, one Shift away, clears the
        // queue without asking — a queue item's Keep a copy is its sheet's
        // row (keep-a-copy spec §4.8). Nor the room's `c` beneath.
        KeyCode::Char('c') => {}
        KeyCode::Esc => {
            gui.queue_view.stow();
            gui.app.focus = Focus::Browser;
        }
        // The row verbs with the cursor stowed: nothing — and not the
        // room's keys either, the panel has the keyboard.
        KeyCode::Enter | KeyCode::Char('d' | '<' | '>') | KeyCode::Delete => {}
        _ => return None,
    }
    Some(false)
}

/// A walking key in the panel (the kit's list-cursor law): a stowed cursor
/// is picked up where the panel rests it — the row a click played, or the
/// playing row — and nothing moves; a held one walks. A queue that never
/// seated one forwards, so the App does.
fn walk_queue(gui: &mut Gui, action: Action) {
    let stowed = !gui.queue_view.held;
    gui.queue_view.pick_up();
    if !stowed || gui.app.queue.state.selected().is_none() {
        gui.forward(action);
    }
}

/// The footer for the sheet's layers and the focused queue.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    if gui.actions.sheet.is_some() {
        let keep = gui.actions.sheet.as_ref().filter(|s| s.keep.is_some());
        let tip = if gui.actions.info {
            t!("gui.tips.info")
        } else if gui.actions.picker.as_ref().is_some_and(|p| p.naming.is_some()) {
            t!("gui.tips.pl_dialog")
        } else if gui.actions.picker.is_some() {
            t!("gui.tips.picker")
        } else if gui.actions.sheet.as_ref().is_some_and(|s| s.links) {
            // Open elsewhere's list (clause 140).
            return Some(super::keep::links_tips(gui));
        } else if gui.actions.sheet.as_ref().is_some_and(|s| s.lead.is_some()) {
            // A lead's sheet (peer-recs contract §1.9): rows, choose, close.
            t!("gui.tips.discover_sheet")
        } else if let Some(sheet) = keep {
            // A connected server's song: no rating keys; `c` while Keep a
            // copy stands (clause 119).
            if sheet_target(&gui.app, sheet).is_none_or(|target| gui.app.keep_choices(target).is_empty()) {
                t!("gui.tips.discover_sheet")
            } else {
                t!("gui.tips.discover_sheet_keep")
            }
        } else if gui.actions.sheet.as_ref().is_some_and(|s| s.item.is_some()) {
            // Any other connected server's song with its Keep row (keep-a-
            // copy spec §4.7): no rating keys, `c` named.
            t!("gui.tips.discover_sheet_keep")
        } else if gui.actions.sheet.as_ref().is_some_and(|s| s.origin.peer.is_some()) {
            // A connected server's song without it: no rating keys either —
            // they rate nothing there.
            t!("gui.tips.discover_sheet")
        } else {
            t!("gui.tips.sheet")
        };
        return Some(tip.to_string());
    }
    // Keep a copy (or a chooser of its) opened from a queue item's sheet
    // holds every key: its tips, never the queue's.
    if gui.queue_open && gui.app.focus == Focus::Queue && !super::keep_doors::window_open(gui) {
        // Stowed, how to pick the cursor up and what works without a row.
        let tip = if gui.queue_view.held { t!("gui.tips.queue") } else { t!("gui.tips.queue_stowed") };
        return Some(tip.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Position;

    use super::super::{Act, FILES_NAV, Gui, render};
    use ratatui::buffer::Buffer;
    use super::SheetAction;
    use crate::api::types::{Track, TrackMetadata};
    use crate::config::Config;
    use crate::tui::app::{App, Effect, Entry, Focus, Origin, Queued, Tab};
    use crate::tui::worker::{ApiCmd, Event};

    const HOST: &str = "http://host:3000";

    /// English words under assertion: the locale lock, and English pinned —
    /// the locale is process-wide, and the tests that switch it hold the
    /// lock while they draw in another language.
    fn english() -> std::sync::RwLockReadGuard<'static, ()> {
        let guard = crate::setup::tests::LOCALE_LOCK.read().unwrap_or_else(|e| e.into_inner());
        rust_i18n::set_locale("en");
        guard
    }

    fn session_gui() -> Gui {
        let mut gui = Gui::new(Config::default(), false, App::new(Some(HOST.into()), None, None));
        gui.app.connected = true;
        gui.app.session.server = HOST.into();
        gui.app.session.server_id = HOST.into();
        gui
    }

    fn track(path: &str, title: &str) -> Track {
        Track {
            filepath: path.into(),
            metadata: TrackMetadata {
                title: Some(title.into()),
                artist: Some("Portishead".into()),
                album: Some("Dummy".into()),
                duration: Some(306.0),
                format: Some("flac".into()),
                bitrate: Some(912_000),
                sample_rate: Some(44_100),
                rating: Some(6),
                bpm: Some(70),
                musical_key: Some("8A".into()),
                ..Default::default()
            },
        }
    }

    fn queued(path: &str, title: &str) -> Queued {
        Queued { origin: Origin { server: HOST.into(), peer: None }, dj: None, track: track(path, title) }
    }

    /// The Files room listing two tracks, the pointer resting on the first.
    fn files_gui() -> Gui {
        let mut gui = session_gui();
        gui.queue_open = false;
        gui.act(Act::Nav(FILES_NAV));
        gui.app.files.set(vec![
            Entry::Track { label: "Mysterons".into(), track: Box::new(track("p/mysterons.flac", "Mysterons")) },
            Entry::Track { label: "Sour Times".into(), track: Box::new(track("p/sour.flac", "Sour Times")) },
        ]);
        gui.pending.clear();
        gui
    }

    fn draw_buffer(gui: &mut Gui) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn lines(buffer: &Buffer) -> Vec<String> {
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        lines(&draw_buffer(gui))
    }

    fn hit_text(gui: &Gui, rows: &[String], needle: &str) -> Option<Act> {
        let y = rows.iter().position(|r| r.contains(needle))?;
        let x = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with(needle))? as u16;
        gui.ui.hit(Position { x, y: y as u16 })
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_sheets_cover_is_pixels_while_the_sheet_is_the_top_layer() {
        let _en = english();
        use ratatui_image::picker::ProtocolType;
        // The wall's and the queue's rule for the sheet's one cover: real
        // pixels where the terminal draws them, the mosaic only while
        // something stands over the sheet (the picker here), and the
        // pixels back — from the warm cache — once it leaves.
        let mut gui = files_gui();
        gui.app.graphics = crate::tui::graphics::Graphics::forced(ProtocolType::Kitty);
        if let Some(Entry::Track { track, .. }) = gui.app.files.entries.get_mut(0) {
            track.metadata.album_art = Some("aa.jpeg".into());
        }
        let png = image::RgbImage::from_pixel(64, 64, image::Rgb([200, 40, 40]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        png.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        gui.app.art.insert("aa.jpeg".into(), Some(crate::tui::art::decode(&bytes.into_inner()).unwrap()));

        gui.act(Act::More(Tab::Files, 0));
        let buffer = draw_buffer(&mut gui);
        let rows = lines(&buffer);
        // The cover's top-left cell sits five rows above "Play now", in its
        // column — found on a text row, because a kitty placeholder cell's
        // symbol carries the whole picture and defeats string positions.
        let py = rows.iter().position(|r| r.contains("Play now")).unwrap();
        let px = rows[py].char_indices().position(|(i, _)| rows[py][i..].starts_with("Play now")).unwrap() as u16;
        let cover = (px, py as u16 - 5);
        let placeholders = |buffer: &Buffer| buffer[cover].symbol().contains('\u{10EEEE}');
        let mosaic = |buffer: &Buffer| "█▀▄".contains(buffer[cover].symbol());
        assert!(placeholders(&buffer), "the sheet's cover is pixels: {:?}", buffer[cover].symbol());

        gui.act(Act::SheetVerb(SheetAction::AddPlaylist));
        let buffer = draw_buffer(&mut gui);
        assert!(mosaic(&buffer), "under the picker the cover is text: {:?}", buffer[cover].symbol());

        gui.act(Act::PickClose);
        draw_buffer(&mut gui);
        let buffer = draw_buffer(&mut gui);
        assert!(placeholders(&buffer), "the pixels return once nothing stands over the sheet");
    }

    #[test]
    fn the_sheets_cover_is_on_record_while_the_sheet_stands() {
        // The review of performance audit #91: the cache keeps what is on
        // screen, and the sheet's cover is on screen.
        let mut gui = files_gui();
        if let Some(Entry::Track { track, .. }) = gui.app.files.entries.get_mut(0) {
            track.metadata.album_art = Some("aa.jpeg".into());
        }
        draw(&mut gui);
        assert!(gui.app.sheet_on_view_is(None));
        gui.act(Act::More(Tab::Files, 0));
        draw(&mut gui);
        assert!(gui.app.sheet_on_view_is(Some("aa.jpeg")));
        gui.act(Act::SheetClose);
        draw(&mut gui);
        assert!(gui.app.sheet_on_view_is(None), "closed, it lets go");
    }

    /// `cargo test dump_actions -- --ignored --nocapture` to eyeball the
    /// sheet, the picker, Song info and the queue's hover verbs.
    #[test]
    #[ignore]
    fn dump_actions() {
        let mut gui = files_gui();
        gui.app.queue.items = vec![queued("q/a.flac", "First"), queued("q/b.flac", "Second"), queued("q/c.flac", "Third")];
        gui.queue_open = true;
        let rows = draw(&mut gui);
        let y = rows.iter().position(|r| r.contains("Second")).unwrap() as u16;
        gui.ui.pointer = Some(Position { x: 80, y });
        println!("{}", draw(&mut gui).join("\n"));
        gui.ui.pointer = None;
        gui.act(Act::More(Tab::Files, 0));
        println!("{}", draw(&mut gui).join("\n"));
        gui.act(Act::SheetVerb(SheetAction::AddPlaylist));
        let effects = gui.app.apply_event(Event::PlaylistNames { names: Some(vec!["Mix".into(), "Late night".into()]) });
        gui.pend(effects);
        println!("{}", draw(&mut gui).join("\n"));
        gui.act(Act::PickClose);
        gui.act(Act::SheetVerb(SheetAction::Info));
        println!("{}", draw(&mut gui).join("\n"));
    }

    #[test]
    fn the_more_verb_on_a_row_opens_the_sheet_for_that_track() {
        let _en = english();
        let mut gui = files_gui();
        let rows = draw(&mut gui);
        // The second row: the pane's cursor rests on the first, and a
        // selected row shows its length, not the verbs.
        let y = rows.iter().position(|r| r.contains("Sour Times")).unwrap() as u16;
        gui.ui.pointer = Some(Position { x: 30, y });
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "[⋯]"), Some(Act::More(Tab::Files, 1)), "the fourth hover verb");
        assert_eq!(gui.ui.hit_context(Position { x: 30, y }), Some(Act::More(Tab::Files, 1)), "a right click too");
        gui.ui.pointer = None;
        gui.act(Act::More(Tab::Files, 0));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::TrackInfo { filepath, .. }) if filepath == "p/mysterons.flac")), "the block is asked for");
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Mysterons") && all.contains("Portishead · Dummy"), "{all}");
        assert!(all.contains("FLAC · 912 kbps · 44.1 kHz · 5:06"), "the spec line: {all}");
        assert!(all.contains("★★★☆☆ 3") && all.contains("♪ 8A") && all.contains("70 BPM"), "the badges: {all}");
        for label in ["Play now", "Add next", "Add to end of queue", "Add to playlist", "Song Info"] {
            assert!(all.contains(label), "{label} missing: {all}");
        }
        assert!(!all.contains("Remove from queue"), "a browse row has no Remove");
    }

    #[test]
    fn the_sheets_verbs_are_the_apps_and_close_it() {
        let _en = english();
        let mut gui = files_gui();
        gui.act(Act::More(Tab::Files, 1));
        gui.act(Act::SheetVerb(SheetAction::AddEnd));
        assert!(gui.actions.sheet.is_none(), "the sheet closes");
        assert_eq!(gui.app.queue.items.len(), 1);
        assert_eq!(gui.app.queue.current, Some(0), "an empty idle queue starts on Add to end");
        gui.act(Act::More(Tab::Files, 0));
        gui.act(Act::SheetVerb(SheetAction::AddNext));
        assert_eq!(gui.app.queue.items[1].filepath, "p/mysterons.flac", "after the playing row");
        gui.act(Act::More(Tab::Files, 0));
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Play now"), Some(Act::SheetVerb(SheetAction::PlayNow)));
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Down);
        key(&mut gui, KeyCode::Enter);
        assert!(gui.actions.picker.is_some(), "Enter on the fourth row opens the picker");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.actions.picker.is_none() && gui.actions.sheet.is_some(), "Esc leaves the picker, keeps the sheet");
        key(&mut gui, KeyCode::Esc);
        assert!(gui.actions.sheet.is_none());
    }

    #[test]
    fn a_peers_track_has_neither_stars_nor_a_playlist_row() {
        let _en = english();
        let mut gui = session_gui();
        let mut item = queued("q/a.flac", "Peer song");
        item.origin.peer = Some(3);
        gui.app.queue.items = vec![item];
        gui.act(Act::QueueMore(0));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Peer song") && all.contains("Remove from queue"), "{all}");
        assert!(!all.contains("Add to playlist") && !all.contains("★"), "{all}");
    }

    #[test]
    fn a_star_sets_the_rating_and_the_current_star_clears_it() {
        let mut gui = files_gui();
        gui.act(Act::More(Tab::Files, 0));
        let rows = draw(&mut gui);
        let y = rows.iter().position(|r| r.contains("★★★☆☆")).unwrap();
        let x = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with('★')).unwrap() as u16;
        assert_eq!(gui.ui.hit(Position { x: x + 4, y: y as u16 }), Some(Act::Rate(10)), "the fifth star sets 5");
        assert_eq!(gui.ui.hit(Position { x: x + 2, y: y as u16 }), Some(Act::Rate(0)), "the third star is the rating: it clears");
        gui.pending.clear();
        gui.act(Act::Rate(10));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::RateSong { rating: Some(10), .. }))));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("★★★★★ 5"), "at once: {all}");
        key(&mut gui, KeyCode::Left);
        assert!(gui.app.rating_of("p/mysterons.flac") == Some(8), "← a whole star down");
        key(&mut gui, KeyCode::Char('0'));
        assert!(gui.app.rating_of("p/mysterons.flac").is_none(), "0 clears");
    }

    #[test]
    fn the_picker_lists_the_playlists_and_new_adds_through_a_name() {
        let _en = english();
        let mut gui = files_gui();
        gui.act(Act::More(Tab::Files, 0));
        gui.pending.clear();
        gui.act(Act::SheetVerb(SheetAction::AddPlaylist));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::PlaylistNames { .. }))));
        let effects = gui.app.apply_event(Event::PlaylistNames { names: Some(vec!["Mix".into(), "Late".into()]) });
        gui.pend(effects);
        let rows = draw(&mut gui);
        assert!(rows.iter().any(|r| r.contains("+ New playlist")) && rows.iter().any(|r| r.contains("Mix")));
        assert_eq!(hit_text(&gui, &rows, "Late"), Some(Act::PickPlaylist("Late".into())));
        gui.pending.clear();
        gui.act(Act::PickPlaylist("Late".into()));
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AddToPlaylist { playlist, song, .. }) if playlist == "Late" && song == "p/mysterons.flac")));
        assert!(gui.actions.sheet.is_none(), "added: the sheet closes");

        gui.act(Act::More(Tab::Files, 0));
        gui.act(Act::SheetVerb(SheetAction::AddPlaylist));
        key(&mut gui, KeyCode::Enter); // the first row: New playlist
        for c in "Fresh".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        gui.pending.clear();
        key(&mut gui, KeyCode::Enter);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AddToPlaylist { playlist, .. }) if playlist == "Fresh")));
    }

    #[test]
    fn the_name_box_has_a_create_button_beside_the_field() {
        let _en = english();
        // The review's finding: Enter was the only way to add through a
        // new name — no clickable Create for the pointer.
        let mut gui = files_gui();
        gui.act(Act::More(Tab::Files, 0));
        gui.act(Act::SheetVerb(SheetAction::AddPlaylist));
        gui.act(Act::PickNew);
        for c in "Fresh".chars() {
            key(&mut gui, KeyCode::Char(c));
        }
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "Create"), Some(Act::PickCreate), "{}", rows.join("\n"));
        gui.pending.clear();
        gui.act(Act::PickCreate);
        assert!(gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::AddToPlaylist { playlist, .. }) if playlist == "Fresh")));
        assert!(gui.actions.sheet.is_none(), "added: the sheet closes");
    }

    #[test]
    fn song_info_lists_only_the_facts_present() {
        let _en = english();
        let mut gui = files_gui();
        gui.act(Act::More(Tab::Files, 0));
        gui.act(Act::SheetVerb(SheetAction::Info));
        let all = draw(&mut gui).join("\n");
        assert!(all.contains("Song Info"), "{all}");
        for label in ["Title", "Artist", "Album", "Length", "BPM", "Key", "Format", "Bitrate", "Sample rate", "Rating", "Path"] {
            assert!(all.contains(label), "{label} missing: {all}");
        }
        assert!(!all.contains("Bit depth") && !all.contains("Year"), "absent facts stay absent: {all}");
        assert!(all.contains("p/mysterons.flac"));
        key(&mut gui, KeyCode::Esc);
        assert!(!gui.actions.info && gui.actions.sheet.is_some());
    }

    #[test]
    fn the_queue_rows_offer_the_sheet_the_grip_and_clear_and_the_keys_shape_it() {
        let _en = english();
        let mut gui = session_gui();
        gui.app.queue.items = vec![queued("q/a.flac", "First"), queued("q/b.flac", "Second"), queued("q/c.flac", "Third")];
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "clear"), Some(Act::QueueClear), "the header's clear");
        let y = rows.iter().position(|r| r.contains("Second")).unwrap() as u16;
        gui.ui.pointer = Some(Position { x: 80, y });
        let rows = draw(&mut gui);
        assert_eq!(hit_text(&gui, &rows, "[⋯]"), Some(Act::QueueMore(1)));
        assert_eq!(hit_text(&gui, &rows, "≡"), Some(Act::QueueGrip(1)));
        assert_eq!(gui.ui.hit_context(Position { x: 80, y }), Some(Act::QueueMore(1)));

        // A click on a row plays it and hands the panel the keys — and
        // lights nothing (the kit's list-cursor law): the row verbs wait,
        // ↓ picks the cursor up on the row it played, the next ↓ walks.
        gui.act(Act::QueueRow(0));
        assert_eq!(gui.app.focus, Focus::Queue);
        assert_eq!(gui.app.queue.current, Some(0));
        assert!(!gui.queue_view.held, "the click lit no row");
        key(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.app.queue.items.len(), 3, "d waits for the cursor to be up");
        key(&mut gui, KeyCode::Down);
        assert!(gui.queue_view.held, "↓ picked the cursor up");
        assert_eq!(gui.app.queue.state.selected(), Some(0), "on the row the click played");
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.app.queue.state.selected(), Some(1));
        key(&mut gui, KeyCode::Char('>'));
        assert_eq!(gui.app.queue.items[2].filepath, "q/b.flac", "> moves the row down");
        assert_eq!(gui.app.queue.current, Some(0), "the playing row stays itself");
        key(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.app.queue.items.len(), 2, "d removes the highlighted row");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.app.focus, Focus::Browser, "Esc hands the keys back");
        assert!(!gui.queue_view.held, "and stows the cursor");
        gui.act(Act::QueueRow(1));
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.app.queue.items.is_empty(), "C clears with the cursor stowed");
    }

    #[test]
    fn a_grip_drag_moves_the_row_under_the_pointer_and_keeps_the_playing_one() {
        let mut gui = session_gui();
        gui.app.queue.items = vec![queued("q/a.flac", "First"), queued("q/b.flac", "Second"), queued("q/c.flac", "Third")];
        gui.act(Act::QueueRow(0));
        draw(&mut gui);
        gui.act(Act::QueueGrip(2));
        assert_eq!(gui.actions.drag, Some(2));
        let panel_x = gui.queue_panel_x();
        super::drag_to(&mut gui, Position { x: panel_x + 5, y: super::super::queue::TOP });
        assert_eq!(gui.app.queue.items[0].filepath, "q/c.flac", "dragged to the top");
        assert_eq!(gui.app.queue.current, Some(1), "the playing row moved with its track");
        super::drop(&mut gui);
        assert!(gui.actions.drag.is_none());
    }

    // ── The lead form (peer-recs contract §1.9; DS M3, L1–L4) ───────────

    use super::super::PEER_RECS_NAV;
    use super::super::discover::taste_bar::{BarAct, Menu};
    use super::super::peer_recs::PeerAct;
    use super::super::peer_recs::kit as room;
    use crate::api::peer_recs_door::{PeerRecsCmd, PeerRecsEvent};
    use crate::api::types::fixtures::{BASEMENT, KITCHEN};
    use crate::api::types::{JobStarted, P2pPeersAnswer, PluginLink};
    use crate::kit::boards::{Area, assert_frame};
    use crate::kit::clipboard::{Copied, catch, caught};
    use crate::kit::theme::th;

    /// The stage's leads in All's rank order (TERM-DECISIONS §2).
    const LONG_EXPOSURE: usize = 6;
    const ARCADE_RAIN: usize = 7;
    const PAPER_MOONS: usize = 8;

    /// The sheet's cells at 100 × 30: x 17–82, y 9–20 (DS §1.9).
    fn sheet_rect() -> Area {
        Area::rect(17, 82, 9, 20)
    }

    /// ALL PEERS · IDLE's room drawn once, nothing pended.
    fn lead_room() -> Gui {
        let mut gui = room::admin_room();
        room::frame(&mut gui, 100, 30);
        gui.pending.clear();
        gui
    }

    /// The sheet on the lead at `index`, its one-row lookup answered with
    /// no jobs.
    fn open_on(gui: &mut Gui, index: usize) {
        super::open_lead(gui, index, false);
        for cmd in room::door(&std::mem::take(&mut gui.pending)) {
            if let PeerRecsCmd::Lookup { seq, .. } = cmd {
                room::land(gui, PeerRecsEvent::Lookup { seq, answer: Ok(Box::default()) });
            }
        }
    }

    fn lead_sheet(index: usize) -> Gui {
        let mut gui = lead_room();
        open_on(&mut gui, index);
        gui
    }

    fn lead_key(gui: &mut Gui, code: KeyCode) {
        room::key(gui, code);
    }

    /// Get it's start, answered with a job of `state` (`running` reads
    /// `downloading…`).
    fn started(gui: &mut Gui, state: &str) {
        let seq = room::door(&gui.pending)
            .into_iter()
            .find_map(|cmd| match cmd {
                PeerRecsCmd::Start { seq, .. } => Some(seq),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Get it starts a job: {:?}", gui.pending));
        gui.pending.clear();
        let job: JobStarted = serde_json::from_value(serde_json::json!({
            "job": { "id": 9, "plugin": "noop-acquire", "state": state },
            "created": true
        }))
        .unwrap();
        room::land(gui, PeerRecsEvent::Started { seq, answer: Ok(Box::new(job)) });
    }

    /// The press's resolve out, answered `preview: null`: the lead is muted
    /// for the visit.
    fn no_preview(gui: &mut Gui) {
        let ticket = room::door(&gui.pending)
            .into_iter()
            .find_map(|cmd| match cmd {
                PeerRecsCmd::Preview { ticket, .. } => Some(ticket),
                _ => None,
            })
            .expect("a press resolves the lead");
        gui.pending.clear();
        room::land(gui, PeerRecsEvent::Preview { ticket, answer: Ok(None) });
    }

    /// L2's links: one canonical, six searches (nine lines, seven shown).
    fn l2_links() -> Vec<PluginLink> {
        let link = |label: &str, url: &str, kind: &str| PluginLink {
            id: label.to_lowercase().replace(' ', "-"),
            label: label.into(),
            url: url.into(),
            kind: kind.into(),
        };
        vec![
            link("MusicBrainz ISRC", "https://musicbrainz.org/isrc/USX9P1800001", "canonical"),
            link("Search MusicBrainz", "https://musicbrainz.org/search?q=x", "search"),
            link("Search Deezer", "https://www.deezer.com/search/x", "search"),
            link("Search Apple Music", "https://music.apple.com/search?term=x", "search"),
            link("Search YouTube", "https://www.youtube.com/results?search_query=x", "search"),
            link("Search Bandcamp", "https://bandcamp.com/search?q=x", "search"),
            link("Search Discogs", "https://www.discogs.com/search?q=x", "search"),
        ]
    }

    /// Open elsewhere's ask answered.
    fn land_links(gui: &mut Gui, links: Vec<PluginLink>) {
        let (seq, generation) = gui
            .pending
            .iter()
            .find_map(|e| match e {
                Effect::Api(ApiCmd::PluginLinks { seq, generation, plugin, .. }) if plugin == "links" => {
                    Some((*seq, *generation))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("Open elsewhere asks the links plug-in: {:?}", gui.pending));
        gui.pending.clear();
        let effects = gui.app.apply_event(Event::PluginLinks { seq, generation, answer: Ok(links) });
        gui.pend(effects);
    }

    fn sheet_row(gui: &Gui) -> usize {
        gui.actions.sheet.as_ref().expect("the sheet is open").row
    }

    /// The text of row `y`'s cells `x0..=x1`.
    fn cells(rows: &[String], y: usize, x0: usize, x1: usize) -> String {
        rows[y].chars().skip(x0).take(x1 + 1 - x0).collect()
    }

    #[test]
    fn m3_lead_sheet_rows_9_to_20() {
        let _en = room::english();
        // Enter on Long Exposure: the cursor on Get it, Preview idle, the
        // last row Copy title.
        let mut gui = lead_sheet(LONG_EXPOSURE);
        let buf = room::frame(&mut gui, 100, 30);
        assert_frame(&buf, "M3", "", sheet_rect());
    }

    #[test]
    fn l1_get_it_downloading() {
        let _en = room::english();
        // Enter on Get it, then ↓: `downloading…` accent at x 69–80, the
        // cursor on Open elsewhere….
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Enter);
        started(&mut gui, "running");
        lead_key(&mut gui, KeyCode::Down);
        let buf = room::frame(&mut gui, 100, 30);
        assert_frame(&buf, "L1", "", sheet_rect());
    }

    #[test]
    fn l2_open_elsewhere() {
        let _en = room::english();
        // Enter on Open elsewhere…: its list hangs from the row, the first
        // link held, and the job's word is not drawn while it does.
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Enter);
        started(&mut gui, "running");
        lead_key(&mut gui, KeyCode::Down);
        lead_key(&mut gui, KeyCode::Enter);
        land_links(&mut gui, l2_links());
        let buf = room::frame(&mut gui, 100, 30);
        assert_frame(&buf, "L2", "", sheet_rect());
    }

    #[test]
    fn l3_offline_asking() {
        let _en = room::english();
        // `l` on Paper Moons, its resolve still out, then its row clicked:
        // the Preview row's `◌` gold, no `• online` for an offline peer.
        let mut gui = lead_room();
        gui.act(Act::Peer(PeerAct::Slot(PAPER_MOONS)));
        assert!(room::door(&gui.pending).iter().any(|c| matches!(c, PeerRecsCmd::Preview { .. })), "the press resolves");
        gui.pending.clear();
        open_on(&mut gui, PAPER_MOONS);
        let buf = room::frame(&mut gui, 100, 30);
        assert_frame(&buf, "L3", "", sheet_rect());
    }

    #[test]
    fn l4_no_preview() {
        let _en = room::english();
        // Arcade Rain answered no preview this visit: `▸ No preview` dim,
        // inert, the cursor on Get it.
        let mut gui = lead_room();
        gui.act(Act::Peer(PeerAct::Slot(ARCADE_RAIN)));
        no_preview(&mut gui);
        open_on(&mut gui, ARCADE_RAIN);
        let buf = room::frame(&mut gui, 100, 30);
        assert_frame(&buf, "L4", "", sheet_rect());
    }

    #[test]
    fn the_sheet_follows_its_lead_when_the_list_moves_and_closes_when_it_leaves() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Enter);
        started(&mut gui, "running");
        // The Kitchen Radio tab: Long Exposure moves to index 2.
        let effects = gui.app.peer_recs_select(Some(KITCHEN.into()));
        gui.pend(effects);
        room::answer(&mut gui, "peers_admin", "taste_kitchen");
        assert_eq!(gui.app.peer_recs_leads()[2].title.as_deref(), Some("Long Exposure"));
        assert_eq!(super::lead_open(&gui), Some(2), "read where the lead went, before any draw");
        let buf = room::frame(&mut gui, 100, 30);
        let rows = room::rows(&buf);
        assert!(rows[10].contains("Long Exposure"), "{}", rows.join("\n"));
        assert_eq!(gui.actions.sheet.as_ref().and_then(|s| s.lead.as_ref()).map(|(i, _)| *i), Some(2), "the sheet followed");
        assert!(rows[16].contains("downloading…"), "the room's sheet state stood: {}", rows[16]);
        assert!(
            !room::door(&gui.pending).iter().any(|c| matches!(c, PeerRecsCmd::Lookup { .. })),
            "a follow asks nothing again"
        );
        // A key acts on the lead where it now stands: Copy title copies it.
        let _ = caught();
        for _ in 0..2 {
            lead_key(&mut gui, KeyCode::Down);
        }
        assert_eq!(sheet_row(&gui), 3);
        gui.act(Act::SheetVerb(SheetAction::CopyTitle));
        assert_eq!(caught(), ["Marlowe Fields - Long Exposure"]);
        // Re-opened, then the Basement Hi-Fi tab, which has no Long
        // Exposure: the sheet closes, and the room's sheet state with it.
        open_on(&mut gui, 2);
        let effects = gui.app.peer_recs_select(Some(BASEMENT.into()));
        gui.pend(effects);
        room::answer(&mut gui, "peers_admin", "taste_basement");
        assert_eq!(super::lead_open(&gui), None);
        room::frame(&mut gui, 100, 30);
        assert!(gui.actions.sheet.is_none(), "the lead left the list: the sheet closed");
        assert!(gui.app.peer_recs_get_it_rows().is_empty(), "through peer_recs_sheet_close");
    }

    #[test]
    fn a_lead_sheet_closes_when_the_room_is_left() {
        let _en = room::english();
        // The room's yield to Files when its gate goes (build plan D31)
        // leaves the App's last answer standing; the sheet goes with the
        // room all the same.
        let mut gui = lead_sheet(LONG_EXPOSURE);
        gui.act(Act::Nav(FILES_NAV));
        assert!(!gui.app.peer_recs_leads().is_empty(), "the last answer still stands");
        assert_eq!(super::lead_open(&gui), None);
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(gui.actions.sheet.is_none(), "{}", rows.join("\n"));
        assert!(!rows.iter().any(|r| r.contains("Long Exposure")), "{}", rows.join("\n"));
    }

    #[test]
    fn the_queue_panel_leaves_the_rooms_modals_their_keys() {
        let _en = room::english();
        let mut gui = lead_room();
        gui.app.queue.items = vec![queued("q/a.flac", "First"), queued("q/b.flac", "Second")];
        gui.queue_open = true;
        gui.app.focus = Focus::Queue;
        gui.act(Act::Peer(PeerAct::Bar(BarAct::Window)));
        let Some(Menu::Window(at)) = gui.peer_recs.bar.menu else { panic!("the window list opens") };
        lead_key(&mut gui, KeyCode::Down);
        assert_eq!(gui.peer_recs.bar.menu, Some(Menu::Window(at + 1)), "↓ walks the window list");
        assert!(!gui.queue_view.held, "and not the queue");
        lead_key(&mut gui, KeyCode::Esc);
        assert!(gui.peer_recs.bar.menu.is_none(), "Esc closes the list");
        assert_eq!(gui.app.focus, Focus::Queue, "the panel keeps the keys it was handed");
        // With the list gone the panel has its keys again.
        lead_key(&mut gui, KeyCode::Down);
        assert!(gui.queue_view.held);
    }

    #[test]
    fn the_lead_sheet_starts_on_get_it_and_never_plays() {
        let _en = room::english();
        let mut gui = lead_room();
        super::open_lead(&mut gui, LONG_EXPOSURE, false);
        let opened = std::mem::take(&mut gui.pending);
        let asked = room::door(&opened);
        assert!(matches!(asked[..], [PeerRecsCmd::Lookup { .. }]), "one lookup, nothing played: {asked:?}");
        assert!(!opened.iter().any(|e| matches!(e, Effect::Audio(_))), "{opened:?}");
        // Every track path skips a lead (clause 31): no Song info, no Keep a copy owner ask
        // and no cover for the stand-in's empty file. The room's own asks are all it sends.
        let track_path = |e: &Effect| {
            matches!(e, Effect::Api(ApiCmd::TrackInfo { .. } | ApiCmd::KeepGate { .. } | ApiCmd::AlbumArt { .. }))
        };
        assert!(!opened.iter().any(track_path), "no track path on open: {opened:?}");
        assert!(opened.iter().all(|e| matches!(e, Effect::Api(ApiCmd::PeerRecs { .. }))), "{opened:?}");
        let rows = super::lead_rows(&gui.app, false);
        assert_eq!(rows, [SheetAction::Preview, SheetAction::GetIt(0), SheetAction::OpenElsewhere, SheetAction::CopyTitle]);
        assert_eq!(sheet_row(&gui), 1, "the cursor on Get it");
        lead_key(&mut gui, KeyCode::Enter);
        let pressed = std::mem::take(&mut gui.pending);
        assert!(room::door(&pressed).iter().any(|c| matches!(c, PeerRecsCmd::Start { plugin, .. } if plugin == "noop-acquire")));
        assert!(!room::door(&pressed).iter().any(|c| matches!(c, PeerRecsCmd::Preview { .. })), "Enter never previews");
        assert!(!pressed.iter().any(|e| matches!(e, Effect::Audio(_))), "nor plays: {pressed:?}");
        assert!(gui.app.queue.items.is_empty() && gui.actions.sheet.is_some(), "no queue add; the sheet stays");
        // The band's `[ get it ]` opens on Get it too.
        gui.act(Act::SheetClose);
        super::open_lead(&mut gui, 0, true);
        assert_eq!(sheet_row(&gui), 1);
        // Without Get it: Open elsewhere…; without that too: Copy title.
        gui.act(Act::SheetClose);
        let seq = room::door(&gui.app.peer_recs_regate())
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Gate { seq } => Some(seq),
                _ => None,
            })
            .unwrap();
        let mut links_only = room::plugins();
        links_only.plugins.retain(|p| p.name != "noop-acquire");
        room::land(&mut gui, PeerRecsEvent::Gate { seq, plugins: Ok(Box::new(links_only.clone())), destination: None });
        super::open_lead(&mut gui, 0, false);
        assert_eq!(super::lead_rows(&gui.app, false)[sheet_row(&gui)], SheetAction::OpenElsewhere);
        gui.act(Act::SheetClose);
        let seq = room::door(&gui.app.peer_recs_regate())
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Gate { seq } => Some(seq),
                _ => None,
            })
            .unwrap();
        links_only.plugins.retain(|p| p.name != "links");
        room::land(&mut gui, PeerRecsEvent::Gate { seq, plugins: Ok(Box::new(links_only)), destination: None });
        super::open_lead(&mut gui, 0, false);
        assert_eq!(super::lead_rows(&gui.app, false), [SheetAction::Preview, SheetAction::CopyTitle]);
        assert_eq!(sheet_row(&gui), 1, "Copy title");
    }

    #[test]
    fn the_cursor_skips_a_muted_preview() {
        let _en = room::english();
        let mut gui = lead_room();
        gui.act(Act::Peer(PeerAct::Slot(ARCADE_RAIN)));
        no_preview(&mut gui);
        open_on(&mut gui, ARCADE_RAIN);
        assert_eq!(sheet_row(&gui), 1, "on Get it");
        lead_key(&mut gui, KeyCode::Up);
        assert_eq!(sheet_row(&gui), 1, "↑ passes over the muted Preview: nothing above takes the cursor");
        lead_key(&mut gui, KeyCode::Down);
        assert_eq!(sheet_row(&gui), 2);
        lead_key(&mut gui, KeyCode::Up);
        assert_eq!(sheet_row(&gui), 1);
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(rows[15].contains("▸  No preview"), "{}", rows[15]);
        assert_eq!(gui.ui.hit(Position { x: 30, y: 15 }), Some(Act::Guard), "a muted Preview takes no press");
        // A Preview that turns muted under the cursor passes it on.
        gui.act(Act::SheetClose);
        open_on(&mut gui, LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Up);
        assert_eq!(sheet_row(&gui), 0, "an idle Preview takes the cursor");
        lead_key(&mut gui, KeyCode::Enter);
        no_preview(&mut gui);
        room::frame(&mut gui, 100, 30);
        assert_eq!(sheet_row(&gui), 1, "the muted row gave the cursor to Get it");
    }

    /// The sheet stays open under the mini player, but the room is not
    /// drawn: its Preview row starts no clip there (peer-recs contract
    /// §1.8), and does once the room is drawn again.
    #[test]
    fn the_sheets_preview_starts_no_clip_under_the_mini_player() {
        let _en = room::english();
        let mut gui = lead_room();
        open_on(&mut gui, LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Up);
        assert_eq!(sheet_row(&gui), 0, "on Preview");
        room::frame(&mut gui, 80, 20);
        room::frame(&mut gui, 80, 20);
        gui.pending.clear();
        lead_key(&mut gui, KeyCode::Enter);
        let resolve = |c: &PeerRecsCmd| matches!(c, PeerRecsCmd::Preview { .. });
        let resolves = room::door(&gui.pending).iter().filter(|c| resolve(c)).count();
        assert_eq!(resolves, 0, "nothing resolves: {:?}", gui.pending);
        assert!(!gui.app.peer_recs_clip_live(), "no clip session");
        room::frame(&mut gui, 100, 30);
        gui.pending.clear();
        lead_key(&mut gui, KeyCode::Enter);
        let resolve = |c: &PeerRecsCmd| matches!(c, PeerRecsCmd::Preview { .. });
        let resolves = room::door(&gui.pending).iter().filter(|c| resolve(c)).count();
        assert_eq!(resolves, 1, "drawn, Preview resolves its lead: {:?}", gui.pending);
    }

    #[test]
    fn a_lead_sheet_draws_no_stars_and_sends_no_rating() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        let sheet: String = rows[9..21].iter().map(|r| r.chars().skip(17).take(66).collect::<String>() + "\n").collect();
        assert!(!sheet.contains('★') && !sheet.contains('☆') && !sheet.contains("Rate"), "{sheet}");
        for code in [KeyCode::Left, KeyCode::Right, KeyCode::Char('0'), KeyCode::Char('3'), KeyCode::Char('5')] {
            lead_key(&mut gui, code);
        }
        gui.act(Act::Rate(10));
        gui.act(Act::PickPlaylist("Mix".into()));
        assert!(
            !gui.pending.iter().any(|e| matches!(e, Effect::Api(ApiCmd::RateSong { .. } | ApiCmd::AddToPlaylist { .. }))),
            "{:?}",
            gui.pending
        );
        // The open's own effects are held in `the_lead_sheet_starts_on_get_it_and_never_plays`.
        let info = |e: &Effect| matches!(e, Effect::Api(ApiCmd::TrackInfo { .. }));
        assert!(!gui.pending.iter().any(info), "no Song info asked by the sheet's keys");
    }

    /// ALL PEERS · IDLE's room drawn once, `taste` its answer, and its plug-in gate still out:
    /// the gate's seq.
    fn lead_room_with(taste: &crate::api::types::TasteResponse, gate: bool) -> (Gui, Option<u64>) {
        let mut gui = room::peer_gui();
        gui.act(Act::Nav(PEER_RECS_NAV));
        let seq = room::answer_with(&mut gui, "peers_admin", taste, gate);
        gui.pending.clear();
        room::frame(&mut gui, 100, 30);
        gui.pending.clear();
        (gui, seq)
    }

    fn ungated_room() -> (Gui, u64) {
        let (gui, seq) = lead_room_with(&crate::api::types::fixtures::taste("taste_all"), false);
        (gui, seq.expect("the gate is asked"))
    }

    /// The gate's answer: the stage's plug-ins and destination.
    fn land_gate(gui: &mut Gui, seq: u64) {
        let plugins = Ok(Box::new(room::plugins()));
        let destination = Some(Ok(Box::new(room::destination())));
        room::land(gui, PeerRecsEvent::Gate { seq, plugins, destination });
    }

    /// The action under the sheet's cursor.
    fn sheet_at(gui: &Gui) -> SheetAction {
        let links = gui.actions.sheet.as_ref().is_some_and(|s| s.links);
        super::lead_rows(&gui.app, links)[sheet_row(gui)]
    }

    /// The cursor stays on its action when the gated rows come or go under it, and lands by
    /// the start rule, never on Preview, when its own row goes (build plan D30).
    #[test]
    fn the_lead_cursor_keeps_its_action_when_the_rows_change() {
        let _en = room::english();
        let (mut gui, seq) = ungated_room();
        super::open_lead(&mut gui, LONG_EXPOSURE, false);
        assert_eq!(super::lead_rows(&gui.app, false), [SheetAction::CopyTitle], "the gate is out");
        assert_eq!(sheet_at(&gui), SheetAction::CopyTitle);
        gui.pending.clear();
        land_gate(&mut gui, seq);
        gui.pending.clear();
        room::frame(&mut gui, 100, 30);
        let rows = super::lead_rows(&gui.app, false);
        assert_eq!(rows, [SheetAction::Preview, SheetAction::GetIt(0), SheetAction::OpenElsewhere, SheetAction::CopyTitle]);
        assert_eq!(sheet_at(&gui), SheetAction::CopyTitle, "the gate landing keeps the cursor on Copy title");
        // A short disconnect hides Get it and Open elsewhere…, and the reconnect brings them back.
        for connected in [false, true] {
            gui.app.connected = connected;
            assert!(super::refind(&mut gui));
            assert_eq!(sheet_at(&gui), SheetAction::CopyTitle, "connected {connected}");
        }
        // On Get it, which the disconnect takes away: the start rule's Copy title, not Preview.
        lead_key(&mut gui, KeyCode::Up);
        lead_key(&mut gui, KeyCode::Up);
        assert_eq!(sheet_at(&gui), SheetAction::GetIt(0));
        gui.app.connected = false;
        assert!(super::refind(&mut gui));
        assert_eq!(sheet_at(&gui), SheetAction::CopyTitle, "Get it gone: never Preview");
        gui.app.connected = true;
        assert!(super::refind(&mut gui));
        assert_eq!(sheet_at(&gui), SheetAction::CopyTitle, "and it stays where it went");
    }

    /// A sheet opened before Get it's gate held asks its one-row lookup once the gate lands, so a
    /// song already being got reads so, and Get it starts no second job (peer-recs contract
    /// §1.9).
    #[test]
    fn a_sheet_opened_before_the_gate_looks_its_jobs_up_when_get_it_comes() {
        let _en = room::english();
        let (mut gui, seq) = ungated_room();
        super::open_lead(&mut gui, LONG_EXPOSURE, false);
        let lookup = |gui: &Gui| {
            room::door(&gui.pending)
                .into_iter()
                .filter_map(|c| match c {
                    PeerRecsCmd::Lookup { seq, .. } => Some(seq),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(lookup(&gui).is_empty(), "no lookup without Get it: {:?}", gui.pending);
        gui.pending.clear();
        land_gate(&mut gui, seq);
        let asked = lookup(&gui);
        assert_eq!(asked.len(), 1, "the gate landing asks the sheet's lookup: {:?}", gui.pending);
        gui.pending.clear();
        let jobs: crate::api::types::JobLookup = serde_json::from_value(serde_json::json!({
            "jobs": [ { "id": 5, "plugin": "noop-acquire", "state": "running" } ]
        }))
        .unwrap();
        room::land(&mut gui, PeerRecsEvent::Lookup { seq: asked[0], answer: Ok(Box::new(jobs)) });
        let mark = gui.app.peer_recs_get_it_rows().first().and_then(|row| row.mark.clone());
        assert!(matches!(mark, Some(crate::api::types::JobOutcome::Running { .. })), "{mark:?}");
        gui.pending.clear();
        let pressed = gui.app.peer_recs_get_it(0);
        assert!(!room::door(&pressed).iter().any(|c| matches!(c, PeerRecsCmd::Start { .. })), "{pressed:?}");
        // A later gate read asks no second lookup.
        let seq = room::door(&gui.app.peer_recs_regate())
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Gate { seq } => Some(seq),
                _ => None,
            })
            .unwrap();
        land_gate(&mut gui, seq);
        assert!(lookup(&gui).is_empty(), "one lookup per sheet: {:?}", gui.pending);
    }

    /// A peer's label is a stranger's, of any length: it is clipped, and the badge row keeps
    /// `from …` and `• online` inside the frame (peer-recs contract §1.9.3).
    #[test]
    fn a_long_peer_label_is_clipped_and_keeps_the_online_mark() {
        let _en = room::english();
        let latin = format!("Kitchen Radio {}", "x".repeat(56));
        for label in [latin.as_str(), &"東京".repeat(18)] {
            let taste = room::taste_edited("taste_all", |results| {
                results[LONG_EXPOSURE]["peer"]["label"] = label.into();
            });
            let (mut gui, _) = lead_room_with(&taste, true);
            open_on(&mut gui, LONG_EXPOSURE);
            let rows = room::rows(&room::frame(&mut gui, 100, 30));
            let badge = cells(&rows, 13, 17, 82);
            // A wide character's second cell reads as a space here.
            let head: String = label.chars().take(4).collect();
            assert!(badge.replace(' ', "").contains(&format!("from{head}")), "{badge:?}");
            let end = badge.trim_end_matches('│');
            assert!(end.trim_end().ends_with("• online") && end.ends_with(' '), "inside the frame: {badge:?}");
            assert_eq!(badge.chars().last(), cells(&rows, 12, 17, 82).chars().last(), "the frame stands: {badge:?}");
        }
    }

    #[test]
    fn the_badge_says_online_only_while_the_poll_does() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(rows[13].contains("from Kitchen Radio   • online"), "{}", rows[13]);
        // The next poll says Kitchen Radio went offline.
        let t0 = crate::clock::Instant::now();
        let effects = gui.app.tick_at(t0);
        gui.pend(effects);
        let effects = gui.app.tick_at(t0 + std::time::Duration::from_secs(16));
        gui.pend(effects);
        let poll = room::door(&gui.pending)
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Peers { poll } => Some(poll),
                _ => None,
            })
            .expect("the poll's beat");
        gui.pending.clear();
        let mut offline = crate::api::types::fixtures::json("peers_admin");
        for peer in offline["peers"].as_array_mut().unwrap() {
            if peer["endpointId"] == KITCHEN {
                peer["online"] = false.into();
            }
        }
        let view = P2pPeersAnswer::from_value(&offline);
        room::land(&mut gui, PeerRecsEvent::Peers { poll, answer: Ok(Box::new(view)) });
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(rows[13].contains("from Kitchen Radio") && !rows[13].contains("online"), "{}", rows[13]);
    }

    #[test]
    fn copy_title_copies_artist_dash_title_and_closes() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        let _ = caught();
        lead_key(&mut gui, KeyCode::Down);
        lead_key(&mut gui, KeyCode::Down);
        assert_eq!(super::lead_rows(&gui.app, false)[sheet_row(&gui)], SheetAction::CopyTitle);
        lead_key(&mut gui, KeyCode::Enter);
        assert_eq!(caught(), ["Marlowe Fields - Long Exposure"]);
        assert!(gui.actions.sheet.is_none(), "the sheet closes");
        assert!(gui.app.peer_recs_get_it_rows().is_empty(), "and the room's sheet state with it");
        assert_eq!(gui.note, Some(("Copied".to_string(), false)));
    }

    #[test]
    fn couldnt_copy_notes_dim() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        catch(Copied::Failed);
        gui.act(Act::SheetVerb(SheetAction::CopyTitle));
        assert_eq!(gui.note, Some(("Couldn't copy".to_string(), false)));
        assert!(gui.actions.sheet.is_none());
        let buf = room::frame(&mut gui, 100, 30);
        let rows = room::rows(&buf);
        let y = rows.iter().position(|r| r.contains("Couldn't copy")).expect("the note is drawn");
        let x = rows[y].find("Couldn't copy").map(|at| rows[y][..at].chars().count()).unwrap() as u16;
        assert_eq!(x, 24, "at the bar's note");
        assert_eq!(buf[(x, y as u16)].fg, th().dim, "dim, not the gold error note");
        catch(Copied::Clipboard);
    }

    #[test]
    fn right_opens_the_links_list_and_left_folds_it() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Down);
        lead_key(&mut gui, KeyCode::Right);
        let asked = gui.pending.iter().find_map(|e| match e {
            Effect::Api(ApiCmd::PluginLinks { rec, .. }) => Some((**rec).clone()),
            _ => None,
        });
        assert_eq!(asked.as_ref(), Some(&gui.app.peer_recs_leads()[LONG_EXPOSURE].raw), "the raw lead, as it came");
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| s.links));
        assert_eq!(gui.discover.links.held, Some(0), "the keys hold the first link");
        land_links(&mut gui, l2_links());
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(rows[12].contains("MusicBrainz ISRC"), "{}", rows.join("\n"));
        assert_eq!(super::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter open · Esc back"));
        lead_key(&mut gui, KeyCode::Left);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| !s.links), "← folds it, the sheet stays");
        assert!(gui.app.keep.links.is_none());
        // ← and → elsewhere in the sheet do nothing: no stars to move.
        lead_key(&mut gui, KeyCode::Up);
        lead_key(&mut gui, KeyCode::Right);
        assert!(gui.actions.sheet.as_ref().is_some_and(|s| !s.links));
    }

    #[test]
    fn the_job_word_hides_while_the_links_list_hangs() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Enter);
        started(&mut gui, "running");
        lead_key(&mut gui, KeyCode::Down);
        let buf = room::frame(&mut gui, 100, 30);
        let rows = room::rows(&buf);
        assert_eq!(cells(&rows, 16, 19, 82), format!("Get it{}downloading… │", " ".repeat(44)), "{}", rows[16]);
        assert_eq!(buf[(69, 16)].fg, th().accent, "in its mark's token, the cursor elsewhere");
        lead_key(&mut gui, KeyCode::Enter);
        land_links(&mut gui, l2_links());
        let buf = room::frame(&mut gui, 100, 30);
        let rows = room::rows(&buf);
        assert!(!cells(&rows, 16, 17, 82).contains("downloading"), "{}", rows[16]);
        assert_eq!((buf[(79, 16)].symbol(), buf[(80, 16)].symbol()), (" ", " "));
        lead_key(&mut gui, KeyCode::Esc);
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert_eq!(cells(&rows, 16, 69, 80), "downloading…", "back once the list folds: {}", rows[16]);
        // Other marks take their own words: a finished job reads `✓ kept`
        // (noop-acquire's result is its own, which no copy word reads).
        let mut gui = lead_sheet(LONG_EXPOSURE);
        lead_key(&mut gui, KeyCode::Enter);
        let seq = room::door(&gui.pending).into_iter().find_map(|c| match c {
            PeerRecsCmd::Start { seq, .. } => Some(seq),
            _ => None,
        });
        gui.pending.clear();
        let job: JobStarted = serde_json::from_value(serde_json::json!({
            "job": {
                "id": 9, "plugin": "noop-acquire", "state": "done",
                "result": { "echoed": "Long Exposure", "steps": 3, "scope": "song" }
            },
            "created": true
        }))
        .unwrap();
        room::land(&mut gui, PeerRecsEvent::Started { seq: seq.unwrap(), answer: Ok(Box::new(job)) });
        let buf = room::frame(&mut gui, 100, 30);
        assert_eq!(cells(&room::rows(&buf), 16, 75, 82), "✓ kept │");
        assert_eq!(buf[(75, 16)].bg, th().accent, "on the cursor's slab");
        lead_key(&mut gui, KeyCode::Down);
        let buf = room::frame(&mut gui, 100, 30);
        assert_eq!(buf[(75, 16)].fg, th().ok, "ok at rest");
        // A kept song is inert: Get it again starts nothing.
        lead_key(&mut gui, KeyCode::Up);
        lead_key(&mut gui, KeyCode::Enter);
        assert!(!room::door(&gui.pending).iter().any(|c| matches!(c, PeerRecsCmd::Start { .. })));
    }

    #[test]
    fn several_acquire_plugins_get_a_row_each_with_their_title() {
        let _en = room::english();
        let mut gui = lead_room();
        let seq = room::door(&gui.app.peer_recs_regate())
            .into_iter()
            .find_map(|c| match c {
                PeerRecsCmd::Gate { seq } => Some(seq),
                _ => None,
            })
            .unwrap();
        let mut two = room::plugins();
        two.plugins.push(
            serde_json::from_value(serde_json::json!({
                "name": "lan-fetch", "title": "LAN fetch", "available": true, "capabilities": ["acquire"]
            }))
            .unwrap(),
        );
        let destination = Some(Ok(Box::new(room::destination())));
        room::land(&mut gui, PeerRecsEvent::Gate { seq, plugins: Ok(Box::new(two)), destination });
        open_on(&mut gui, LONG_EXPOSURE);
        assert_eq!(super::lead_rows(&gui.app, false), [
            SheetAction::Preview,
            SheetAction::GetIt(0),
            SheetAction::GetIt(1),
            SheetAction::OpenElsewhere,
            SheetAction::CopyTitle
        ]);
        let buf = room::frame(&mut gui, 100, 30);
        let rows = room::rows(&buf);
        let y = rows.iter().position(|r| r.contains("Get it  Noop acquire")).unwrap_or_else(|| panic!("{}", rows.join("\n")));
        assert_eq!(cells(&rows, y, 19, 38).trim_end(), "Get it  Noop acquire");
        assert_eq!(cells(&rows, y + 1, 19, 38).trim_end(), "Get it  LAN fetch", "{}", rows.join("\n"));
        assert_eq!(buf[(27, y as u16 + 1)].fg, th().dim, "the plug-in's title dim, 2 cells after the word");
        assert_eq!(buf[(19, y as u16 + 1)].fg, th().accent, "Get it accent at rest");
        let at = Position { x: 30, y: y as u16 + 1 };
        assert_eq!(gui.ui.hit(at), Some(Act::SheetVerb(SheetAction::GetIt(1))));
        gui.act(Act::SheetVerb(SheetAction::GetIt(1)));
        assert!(room::door(&gui.pending).iter().any(|c| matches!(c, PeerRecsCmd::Start { plugin, .. } if plugin == "lan-fetch")));
    }

    #[test]
    fn a_lead_sheets_tips_are_discover_sheets() {
        let _en = room::english();
        let mut gui = lead_sheet(LONG_EXPOSURE);
        assert_eq!(super::tips(&gui).as_deref(), Some("↑ ↓ rows · Enter choose · Esc close"));
        gui.config.gui.key_hints = true;
        gui.ui.key_hints = true;
        let rows = room::rows(&room::frame(&mut gui, 100, 30));
        assert!(rows.iter().any(|r| r.contains("↑ ↓ rows · Enter choose · Esc close")), "{}", rows.join("\n"));
        assert_eq!(gui.active, PEER_RECS_NAV);
    }
}
