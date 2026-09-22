//! Track actions (docs/ux-contracts/track-actions.md), the GUI's half: the
//! one sheet for a song wherever it is met, its rating on the badge row,
//! the add-to-playlist picker, Song info, and the queue panel's grip drag
//! and keyboard reach. The verbs themselves are the shared App's.

use ratatui::Frame;
use ratatui::crossterm::event::{Event as TermEvent, KeyCode, KeyEvent};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use rust_i18n::t;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::api::types::Track;
use crate::kit::theme::{legacy_conhost, th};
use crate::kit::{Surface, dim, input_display, modal_close, modal_frame_on};
use crate::tui::app::{Action, App, Entry, Focus, Origin, Tab};

use super::cover::Slot;
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
    /// The sheet's cover, a mosaic (a modal draws no pixels).
    slot: Option<Slot>,
}

impl ActionsUi {
    pub(crate) fn new() -> Self {
        Self::default()
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
    Info,
    Remove,
}

impl SheetAction {
    fn label(self) -> String {
        match self {
            SheetAction::PlayNow => t!("gui.act.play_now"),
            SheetAction::AddNext => t!("gui.act.add_next"),
            SheetAction::AddEnd => t!("gui.act.add_end"),
            SheetAction::AddPlaylist => t!("gui.act.add_playlist"),
            SheetAction::Info => t!("gui.act.info"),
            SheetAction::Remove => t!("gui.act.remove"),
        }
        .to_string()
    }
}

fn actions_for(sheet: &Sheet, own: bool) -> Vec<SheetAction> {
    let mut rows = vec![SheetAction::PlayNow, SheetAction::AddNext, SheetAction::AddEnd];
    if own {
        rows.push(SheetAction::AddPlaylist);
    }
    rows.push(SheetAction::Info);
    if sheet.queue_row.is_some() {
        rows.push(SheetAction::Remove);
    }
    rows
}

pub(crate) fn modal_open(gui: &Gui) -> bool {
    gui.actions.sheet.is_some()
}

// ── Opening ─────────────────────────────────────────────────────────────────

/// The sheet for one track (clause 1): the row's own block now, the full
/// one asked of its server (clause 8).
pub(crate) fn open_for(gui: &mut Gui, origin: Origin, track: Track, queue_row: Option<usize>) {
    let effects = gui.app.fetch_track_info(&origin, &track.filepath);
    gui.pend(effects);
    gui.actions.picker = None;
    gui.actions.info = false;
    gui.actions.sheet = Some(Sheet { origin, track, queue_row, row: 0 });
}

/// A browse row's `[⋯]`, `m` or right click: the pane's track at `index`.
fn open_from_pane(gui: &mut Gui, tab: Tab, index: usize) {
    let track = match gui.app.pane_for(tab).entries.get(index) {
        Some(Entry::Track { track, .. }) => (**track).clone(),
        _ => return,
    };
    let origin = gui.app.origin();
    open_for(gui, origin, track, None);
}

fn open_from_queue(gui: &mut Gui, index: usize) {
    let Some(item) = gui.app.queue.items.get(index) else { return };
    let (origin, track) = (item.origin.clone(), item.track.clone());
    open_for(gui, origin, track, Some(index));
}

/// The bar's card: the playing track, with its queue row when it is one.
fn open_for_playing(gui: &mut Gui) {
    let Some(now) = gui.app.now_playing.clone() else { return };
    let current = gui.app.queue.current.filter(|&i| gui.app.queue.items.get(i).is_some_and(|q| q.filepath == now.filepath));
    let origin = current
        .and_then(|i| gui.app.queue.items.get(i).map(|q| q.origin.clone()))
        .unwrap_or_else(|| gui.app.origin());
    open_for(gui, origin, now, current);
}

/// `m`: the queue's highlighted row while the panel has the keys, else the
/// browse room's highlighted track, else what is playing.
fn open_by_key(gui: &mut Gui) {
    if gui.app.focus == Focus::Queue
        && let Some(index) = gui.app.queue.state.selected()
    {
        open_from_queue(gui, index);
        return;
    }
    if gui.browse_room() || gui.active == SEARCH_NAV {
        let tab = if gui.active == SEARCH_NAV { Tab::Search } else { gui.browse_tab() };
        if let Some(index) = gui.app.pane_for(tab).state.selected()
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
    match &app.track_info {
        Some(info) if info.filepath == sheet.track.filepath => info,
        _ => &sheet.track,
    }
}

fn title_of(track: &Track) -> String {
    track.metadata.display_title().unwrap_or_else(|| track.file_name()).to_string()
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
    let m = &track.metadata;
    let mut parts: Vec<String> = Vec::new();
    if let Some(format) = m.format.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        parts.push(format.to_uppercase());
    }
    if let Some(bitrate) = m.bitrate {
        parts.push(t!("gui.info.kbps", n = (bitrate as f64 / 1000.0).round() as u64).to_string());
    }
    if let Some(rate) = m.sample_rate {
        parts.push(khz(rate));
    }
    if let Some(seconds) = m.duration {
        parts.push(super::bar::fmt_time(seconds));
    }
    parts.join(" · ")
}

fn khz(hz: u32) -> String {
    let k = f64::from(hz) / 1000.0;
    let words = if k.fract() == 0.0 { format!("{k:.0}") } else { format!("{k:.1}") };
    t!("gui.info.khz", n = words).to_string()
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
    add("bitrate", m.bitrate.map(|b| t!("gui.info.kbps", n = (b as f64 / 1000.0).round() as u64).to_string()));
    add("sample_rate", m.sample_rate.map(khz));
    add("bit_depth", m.bit_depth.map(|d| t!("gui.info.bits", n = d).to_string()));
    add("channels", m.channels.map(|c| c.to_string()));
    add("size", m.file_size.map(size_words));
    add("plays", m.play_count.map(|p| p.to_string()));
    add("rating", m.rating.filter(|r| *r > 0).map(|r| format!("{} {}", stars(Some(r)), rating_words(Some(r)))));
    add("path", Some(track.filepath.clone()));
    rows
}

fn size_words(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 { format!("{mb:.1} MB") } else { format!("{:.0} KB", bytes as f64 / 1024.0) }
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// The sheet, then whichever of the picker or Song info stands over it.
pub(crate) fn draw_modals(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    if gui.actions.sheet.is_none() {
        return;
    }
    draw_sheet(frame, gui, area);
    if gui.actions.picker.is_some() {
        draw_picker(frame, gui, area);
    } else if gui.actions.info {
        draw_info(frame, gui, area);
    }
}

fn draw_sheet(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    // Drawn from the sheet in place: the track and its decoded cover are
    // read each frame, not copied.
    let Gui { app, actions, ui, .. } = &mut *gui;
    let Some(sheet) = actions.sheet.as_ref() else { return };
    ui.click(area, Act::SheetClose);
    let track = current_track(app, sheet);
    let own = app.track_is_own(&sheet.origin);
    let rows = actions_for(sheet, own);
    let art = track.metadata.album_art.as_deref().and_then(|file| app.art.get(file)).and_then(|a| a.as_ref());
    let width: u16 = 66.min(area.width.saturating_sub(2)).max(44);
    let height = (3 + 1 + 1 + rows.len() + 1) as u16 + 2;
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    modal_close(frame, ui, inner, Act::SheetClose);

    // The header (clause 2): the cover when there is one, the words beside.
    let mut tx = inner.x + 1;
    if let Some(art) = art {
        let cover = Rect { x: inner.x + 1, y: inner.y, width: 6, height: 3 };
        let slot = actions.slot.get_or_insert_with(|| Slot::new(app.graphics.fork()));
        slot.draw_mosaic(frame, cover, art);
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
        let rating = track.metadata.rating;
        let glyphs = stars(rating);
        let v = rating.unwrap_or(0).min(10);
        for (i, glyph) in glyphs.chars().enumerate() {
            let cell = Rect { x: bx + i as u16, y: by, width: 1, height: 1 };
            let full = 2 * (i as u32 + 1);
            let lit = v >= full - 1;
            let hover = ui.pointer.is_some_and(|p| cell.contains(p));
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

    // The actions (clause 4), one row each; a click acts, as the record's
    // rows do.
    for (i, action) in rows.iter().enumerate() {
        let y = inner.y + 5 + i as u16;
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let is_sel = sheet.row == i;
        let hover = ui.pointer.is_some_and(|p| rect.contains(p));
        if is_sel {
            frame.render_widget(ratatui::widgets::Block::default().style(sel()), rect);
        }
        let style = match (is_sel, hover, *action) {
            (true, _, _) => sel().add_modifier(Modifier::BOLD),
            (false, true, _) => bright_bold(),
            (false, false, SheetAction::Remove) => Style::default().fg(th().danger),
            (false, false, _) => Style::default(),
        };
        put(frame, inner.x + 1, y, &action.label(), style);
        ui.click(rect, Act::SheetVerb(*action));
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
    let names: Option<&[String]> = match &app.playlist_names {
        Some(Some(names)) => Some(names.as_slice()),
        _ => None,
    };
    let failed = matches!(app.playlist_names, Some(None));
    let width: u16 = 52.min(area.width.saturating_sub(2)).max(36);
    if let Some((name, cursor)) = naming {
        // New playlist: the name, then the add (clause 12).
        let inner = modal_frame_on(frame, ui, area, width, 7, th().accent);
        put(frame, inner.x + 1, inner.y, &t!("gui.pl.new"), accent().add_modifier(Modifier::BOLD));
        modal_close(frame, ui, inner, Act::PickClose);
        put(frame, inner.x + 1, inner.y + 2, &input_display(&name, cursor, inner.width.saturating_sub(2)), Style::default());
        put(frame, inner.x + 1, inner.y + 4, &t!("gui.act.name_hint", title = title_of(&sheet.track)), dim());
        return;
    }
    let listed = names.map_or(0, <[String]>::len);
    let rows = 1 + listed;
    let max_rows = area.height.saturating_sub(8) as usize;
    let shown_rows = rows.min(max_rows.max(1));
    let height = (shown_rows + 2 + usize::from(listed == 0)) as u16 + 2;
    let inner = modal_frame_on(frame, ui, area, width, height, th().accent);
    put(frame, inner.x + 1, inner.y, &t!("gui.act.add_playlist"), accent().add_modifier(Modifier::BOLD));
    modal_close(frame, ui, inner, Act::PickClose);
    let mut y = inner.y + 2;
    let mut line = |frame: &mut Frame, ui: &mut Surface<Act>, index: usize, label: &str, act: Act, lead: bool| {
        let rect = Rect { x: inner.x, y, width: inner.width, height: 1 };
        let is_sel = row == index;
        let hover = ui.pointer.is_some_and(|p| rect.contains(p));
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
            // The window keeps the cursor's row in view.
            let visible = shown_rows.saturating_sub(1);
            let first = row.saturating_sub(1).saturating_sub(visible.saturating_sub(1)).min(names.len().saturating_sub(visible));
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
    gui.actions.sheet = None;
    gui.actions.picker = None;
    gui.actions.info = false;
}

fn run(gui: &mut Gui, action: SheetAction) {
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
        SheetAction::Remove => {
            if let Some(index) = sheet.queue_row {
                let effects = gui.app.remove_queue_row(index);
                gui.pend(effects);
            }
            close(gui);
        }
    }
}

fn rate(gui: &mut Gui, rating: u32) {
    let Some(sheet) = gui.actions.sheet.clone() else { return };
    let rating = (rating > 0).then_some(rating.min(10));
    let effects = gui.app.rate_track(&sheet.origin, &sheet.track.filepath, rating);
    gui.pend(effects);
    if let Some(open) = gui.actions.sheet.as_mut() {
        open.track.metadata.rating = rating;
    }
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
            if let Some(sheet) = gui.actions.sheet.clone() {
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
        Act::PickClose => gui.actions.picker = None,
        Act::InfoClose => gui.actions.info = false,
        Act::QueueClear => gui.forward(Action::ClearQueue),
        Act::QueueGrip(index) => {
            gui.app.focus = Focus::Queue;
            gui.app.queue.state.select(Some(index));
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
        let listed = match &gui.app.playlist_names {
            Some(Some(names)) => names.len(),
            _ => 0,
        };
        match key.code {
            KeyCode::Esc => gui.actions.picker = None,
            KeyCode::Up => picker.row = picker.row.saturating_sub(1),
            KeyCode::Down => picker.row = (picker.row + 1).min(listed),
            KeyCode::Enter => {
                let row = picker.row;
                if row == 0 {
                    return Some(gui.act(Act::PickNew));
                }
                let name = match &gui.app.playlist_names {
                    Some(Some(names)) => names.get(row - 1).cloned(),
                    _ => None,
                };
                if let Some(name) = name {
                    return Some(gui.act(Act::PickPlaylist(name)));
                }
            }
            _ => {}
        }
        return Some(false);
    }
    let sheet = gui.actions.sheet.as_mut()?;
    let own = sheet.origin.peer.is_none();
    let actions = actions_for(sheet, own);
    let rating = gui.app.rating_of(&sheet.track.filepath).unwrap_or(0).min(10);
    match key.code {
        KeyCode::Esc => close(gui),
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

/// The queue panel's keys while it has the keyboard (clause 19).
pub(crate) fn queue_key(gui: &mut Gui, key: KeyEvent) -> Option<bool> {
    if !(gui.queue_open && gui.app.focus == Focus::Queue) {
        return None;
    }
    match key.code {
        KeyCode::Down => gui.forward(Action::Down),
        KeyCode::Up => gui.forward(Action::Up),
        KeyCode::PageDown => gui.forward(Action::PageDown),
        KeyCode::PageUp => gui.forward(Action::PageUp),
        KeyCode::Enter => gui.forward(Action::Activate),
        KeyCode::Char('d') | KeyCode::Delete => gui.forward(Action::RemoveFromQueue),
        KeyCode::Char('C') => gui.forward(Action::ClearQueue),
        KeyCode::Char('<') => gui.forward(Action::MoveQueueUp),
        KeyCode::Char('>') => gui.forward(Action::MoveQueueDown),
        KeyCode::Char('i') => gui.forward(Action::JumpToPlaying),
        KeyCode::Char('m') => return Some(gui.act(Act::MoreKey)),
        KeyCode::Esc => gui.app.focus = Focus::Browser,
        _ => return None,
    }
    Some(false)
}

/// The footer for the sheet's layers and the focused queue.
pub(crate) fn tips(gui: &Gui) -> Option<String> {
    if gui.actions.sheet.is_some() {
        let tip = if gui.actions.info {
            t!("gui.tips.info")
        } else if gui.actions.picker.as_ref().is_some_and(|p| p.naming.is_some()) {
            t!("gui.tips.pl_dialog")
        } else if gui.actions.picker.is_some() {
            t!("gui.tips.picker")
        } else {
            t!("gui.tips.sheet")
        };
        return Some(tip.to_string());
    }
    if gui.queue_open && gui.app.focus == Focus::Queue {
        return Some(t!("gui.tips.queue").to_string());
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
    use super::SheetAction;
    use crate::api::types::{Track, TrackMetadata};
    use crate::config::Config;
    use crate::tui::app::{App, Effect, Entry, Focus, Origin, Queued, Tab};
    use crate::tui::worker::{ApiCmd, Event};

    const HOST: &str = "http://host:3000";

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
        gui.app.tab = Tab::Files;
        gui.app.files.set(vec![
            Entry::Track { label: "Mysterons".into(), track: Box::new(track("p/mysterons.flac", "Mysterons")) },
            Entry::Track { label: "Sour Times".into(), track: Box::new(track("p/sour.flac", "Sour Times")) },
        ]);
        gui.pending.clear();
        gui
    }

    fn draw(gui: &mut Gui) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).unwrap();
        terminal.draw(|frame| render(frame, gui)).unwrap();
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    fn hit_text(gui: &Gui, rows: &[String], needle: &str) -> Option<Act> {
        let y = rows.iter().position(|r| r.contains(needle))?;
        let x = rows[y].char_indices().position(|(i, _)| rows[y][i..].starts_with(needle))? as u16;
        gui.ui.hit(Position { x, y: y as u16 })
    }

    fn key(gui: &mut Gui, code: KeyCode) {
        super::super::handle_key(gui, KeyEvent::new(code, KeyModifiers::NONE));
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
    fn song_info_lists_only_the_facts_present() {
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

        // A click on a row plays it and hands the panel the keys.
        gui.act(Act::QueueRow(0));
        assert_eq!(gui.app.focus, Focus::Queue);
        assert_eq!(gui.app.queue.current, Some(0));
        key(&mut gui, KeyCode::Down);
        assert_eq!(gui.app.queue.state.selected(), Some(1));
        key(&mut gui, KeyCode::Char('>'));
        assert_eq!(gui.app.queue.items[2].filepath, "q/b.flac", "> moves the row down");
        assert_eq!(gui.app.queue.current, Some(0), "the playing row stays itself");
        key(&mut gui, KeyCode::Char('d'));
        assert_eq!(gui.app.queue.items.len(), 2, "d removes the highlighted row");
        key(&mut gui, KeyCode::Esc);
        assert_eq!(gui.app.focus, Focus::Browser, "Esc hands the keys back");
        gui.act(Act::QueueRow(1));
        key(&mut gui, KeyCode::Char('C'));
        assert!(gui.app.queue.items.is_empty(), "C clears");
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
}
