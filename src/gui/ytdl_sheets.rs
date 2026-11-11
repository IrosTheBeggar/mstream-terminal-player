//! Add from YouTube's sheets: Edit tags — the four tags as bordered text
//! boxes, the card every text input wears — and Format, the formats the
//! server takes as radios with what each is good for. Each stands over
//! the form it was opened from, the room's or the Files window's, the
//! frame beneath inert; [`open_sheet`] is the frame they share with Saves
//! to ([`super::ytdl_where`]) and the window's Saves here
//! ([`super::ytdl_here`]).
//!
//! Design: the "Add from YouTube · GUI" canvas's Tags and Format boards.
//! A sheet is 66 cells wide and stands where a room's window does (Keep
//! a copy's rule): centred in the room where the room is wider than it,
//! over the frame's middle where it is not — so it covers the block it
//! was opened from, whose cells never show cut beside it. Over the Files
//! window it is the window's own 76, on the window's columns. Its title
//! is TEXT BOLD on the top border (what it holds now DIM after it); its
//! text starts two cells in from the border and its one bracketed button
//! ends three before the other. Edit tags edits the form's own tags as they are
//! typed — `[ Done ]`, Enter and Esc all only close it. Format's dot is
//! the choice in the making: `[ Done ]` or Enter takes it, Esc leaves the
//! format as it was.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use rust_i18n::t;

use crate::kit::theme::th;
use crate::kit::{dim, width};
use crate::ytdl::{TagField, field_help, format_help, format_name, year_invalid};

use super::keep;
use super::ytdl::{self, Form, Keys, YtAct, yt};
use super::{Act, Gui, accent, bright_bold, put};

/// A sheet's width (the track sheet's, the kit's).
pub(super) const SHEET_W: u16 = 66;
/// Edit tags' rows inside its border.
const TAGS_ROWS: u16 = 13;
/// The Year's card: four digits, the caret and the card's own four cells.
const YEAR_W: u16 = 9;

/// Where a sheet `h` rows tall stands: the frame's middle rows; a room's
/// window's columns — [`SHEET_W`] of them over the room, the Files
/// window's own over it.
fn sheet_rect(gui: &Gui, area: Rect, h: u16) -> Rect {
    let w = if super::ytdl_here::window_up(gui) { keep::KEEP_W } else { SHEET_W };
    let room = super::content_rect(area.width, area.height, gui.queue_open, gui.footer());
    let (columns, _) = keep::window_rect(area, room, w, h);
    Rect { x: columns.x, width: columns.width, ..crate::kit::modal_rect(area, w, h, h) }
}

/// The cells of text across a sheet opened now: its width less two a side.
pub(super) fn sheet_room(gui: &Gui, area: Rect) -> usize {
    usize::from(sheet_rect(gui, area, 3).width.saturating_sub(4))
}

/// A sheet over the room or the Files window, `rows` tall inside its
/// border — what is beneath inert (the kit's Modal), a click outside it
/// `away`. Returns its rect.
pub(super) fn open_sheet(frame: &mut Frame, gui: &mut Gui, area: Rect, rows: u16, away: Act) -> Rect {
    open_sheet_anchored(frame, gui, area, rows, rows, away)
}

/// [`open_sheet`] for a sheet whose rows come and go as it is used (a
/// path's suggestions): it stands where it would at `most` rows and grows
/// downward from there, so its title and its first rows hold one spot.
pub(super) fn open_sheet_anchored(frame: &mut Frame, gui: &mut Gui, area: Rect, rows: u16, most: u16, away: Act) -> Rect {
    // The controls this sheet draws are the only ones live under it.
    gui.ytdl.drawn.clear();
    gui.ui.clear_registries();
    gui.ui.click(area, away);
    let rect = Rect { height: rows.min(most) + 2, ..sheet_rect(gui, area, most.max(rows) + 2) };
    gui.ui.overlay(rect);
    gui.ui.modal_over();
    keep::modal_box(frame, rect);
    keep::clear_spill(frame, rect);
    gui.ui.click(rect, Act::Guard);
    rect
}

/// Where a sheet's text starts, and the cells across it: two in from each
/// border.
pub(super) fn sheet_text(rect: Rect) -> (u16, u16) {
    (rect.x + 2, rect.width.saturating_sub(4))
}

/// A sheet's title on its top border: the name TEXT BOLD, then what the
/// sheet holds now — drawn by `tail` from the cell it is given, within
/// the cells it is given — after a DIM `·`. The tail's cells are the
/// sheet's own text (never the border's colour, nor its line between two
/// words), and the border takes up again one blank after it.
pub(super) fn sheet_title(frame: &mut Frame, rect: Rect, title: &str, tail: impl FnOnce(&mut Frame, u16, usize) -> u16) {
    keep::border_title(frame, rect, title);
    // ` {title} ` from two cells in; the tail takes the title's own
    // closing blank's place.
    let at = rect.x + 3 + width(title) as u16;
    let x = at + 3;
    let room = rect.right().saturating_sub(3).saturating_sub(x);
    let owned = th().ground.is_some() && crate::kit::theme::ground_owned();
    let text = Style::default().fg(if owned { th().text } else { ratatui::style::Color::Reset });
    let line = Rect { x, y: rect.y, width: room + 1, height: 1 };
    let border: Vec<ratatui::buffer::Cell> = (line.x..line.right()).map(|cx| frame.buffer_mut()[(cx, rect.y)].clone()).collect();
    put(frame, at, rect.y, " · ", dim());
    put(frame, x, rect.y, &" ".repeat(usize::from(line.width)), text);
    let drawn = tail(frame, x, usize::from(room));
    // Past the tail and its one blank, the border's own cells again.
    for (cx, cell) in (line.x..line.right()).zip(border).skip(usize::from(drawn) + 1) {
        frame.buffer_mut()[(cx, rect.y)] = cell;
    }
}

/// A sheet's bracketed button, ending at `end` (three cells before the
/// border): ACCENT BOLD, BRIGHT BOLD under the pointer; off, DIM and no
/// click. Returns where it starts.
pub(super) fn bracket_button(frame: &mut Frame, gui: &mut Gui, end: u16, y: u16, words: &str, on: bool, act: Act) -> u16 {
    let label = format!("[ {words} ]");
    let at = Rect { x: end.saturating_sub(width(&label) as u16), y, width: width(&label) as u16, height: 1 };
    let style = if !on {
        dim()
    } else if gui.ui.hovers(at) {
        bright_bold()
    } else {
        accent().add_modifier(Modifier::BOLD)
    };
    put(frame, at.x, y, &label, style);
    if on {
        gui.ui.click(at, act);
    }
    at.x
}

/// A text input as the kit's bordered card: ACCENT while it has the keys,
/// the caret in it; BRIGHT under the pointer; DIM at rest. A click on it
/// is `act`.
#[allow(clippy::too_many_arguments)]
pub(super) fn field_card(frame: &mut Frame, gui: &mut Gui, card: Rect, value: &str, cursor: usize, focused: bool, act: Act) {
    let tone = if focused {
        th().accent
    } else if gui.ui.hovers(card) {
        th().bright
    } else {
        th().dim
    };
    crate::kit::cursor_ring(frame, card, Style::default().fg(tone));
    let (x, y) = (card.x + 2, card.y + 1);
    let room = card.width.saturating_sub(4);
    if focused {
        super::text_field(frame, &mut gui.ui, x, y, value, cursor, room, Style::default());
    } else {
        put(frame, x, y, &super::bar::clip(value, usize::from(room)), Style::default());
    }
    gui.ui.click(card, act);
}

// ── Edit tags ───────────────────────────────────────────────────────────────

const TAGS: [TagField; 4] = [TagField::Title, TagField::Artist, TagField::Album, TagField::Year];

fn tag_index(field: TagField) -> usize {
    TAGS.iter().position(|f| *f == field).unwrap_or(0)
}

fn tag_key(field: TagField) -> &'static str {
    match field {
        TagField::Title => "gui.ytdl.tag.title",
        TagField::Artist => "gui.ytdl.tag.artist",
        TagField::Album => "gui.ytdl.tag.album",
        TagField::Year => "gui.ytdl.tag.year",
    }
}

/// What the sheet says under the cards, for the tag with the keys: a
/// year that is no year (GOLD), where the tag's value came from, and what
/// leaving it blank does — three rows at most.
fn tag_help(gui: &Gui, form: Form, field: TagField, room: usize) -> Vec<(String, Style)> {
    let draft = ytdl::draft(gui, form);
    let i = tag_index(field);
    let value = draft.fields[i].value();
    let album = draft.meta().and_then(|m| m.album.as_deref());
    let mut lines: Vec<(String, Style)> = Vec::new();
    if year_invalid(draft.fields[tag_index(TagField::Year)].value()) {
        let gold = Style::default().fg(th().gold);
        lines.extend(ytdl::gold_rows(&t!("gui.ytdl.year_invalid"), room).into_iter().map(|l| (l, gold)));
    }
    let wrap = |text: String| crate::kit::wrap_whole(&text, room.max(1)).into_iter().map(|l| (l, dim()));
    lines.extend(wrap(field_help(field, &draft.guess, &draft.gstate, draft.edited[i], value, album).text()));
    if !value.trim().is_empty() {
        lines.extend(wrap(field_help(field, &draft.guess, &draft.gstate, false, "", album).text()));
    }
    lines.truncate(3);
    lines
}

/// The Edit tags sheet (the canvas's Tags board): TITLE over a card the
/// sheet's width; ARTIST, ALBUM and YEAR over three side by side; the
/// focused tag's help; `[ Done ]`.
pub(super) fn draw_tags(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let form = ytdl::active_form(gui);
    let rect = open_sheet(frame, gui, area, TAGS_ROWS, yt(YtAct::TagsDone));
    keep::border_title(frame, rect, &t!("gui.ytdl.tags.title"));
    let (x, total) = sheet_text(rect);
    let focus = match ytdl::draft(gui, form).keys {
        Keys::Tag(field) => field,
        _ => TagField::Title,
    };
    let year_w = YEAR_W.min(total);
    let rest = total.saturating_sub(year_w + 4);
    let album_w = rest * 3 / 7;
    let artist_w = rest - album_w;
    let cards = [
        (TagField::Title, Rect { x, y: rect.y + 2, width: total, height: 3 }),
        (TagField::Artist, Rect { x, y: rect.y + 6, width: artist_w, height: 3 }),
        (TagField::Album, Rect { x: x + artist_w + 2, y: rect.y + 6, width: album_w, height: 3 }),
        (TagField::Year, Rect { x: x + artist_w + album_w + 4, y: rect.y + 6, width: year_w, height: 3 }),
    ];
    for (field, card) in cards {
        put(frame, card.x, card.y - 1, &super::bar::clip(&t!(tag_key(field)), usize::from(card.width)), dim());
        let input = &ytdl::draft(gui, form).fields[tag_index(field)];
        let (value, cursor) = (input.value().to_string(), input.cursor());
        field_card(frame, gui, card, &value, cursor, focus == field, yt(YtAct::Tag(field)));
    }
    for (y, (line, style)) in (rect.y + 10..).zip(tag_help(gui, form, focus, usize::from(total))) {
        put(frame, x, y, &super::bar::clip(&line, usize::from(total)), style);
    }
    bracket_button(frame, gui, rect.right().saturating_sub(3), rect.y + TAGS_ROWS, &t!("gui.ytdl.done"), true, yt(YtAct::TagsDone));
}

/// The sheet's keys: Enter and Esc close it — the tags are the form's as
/// they are typed; Tab and ↓, Shift-Tab and ↑ walk the tags; the rest is
/// the focused tag's line editor's.
pub(super) fn tags_key(gui: &mut Gui, key: KeyEvent) {
    let form = ytdl::active_form(gui);
    let field = match ytdl::draft(gui, form).keys {
        Keys::Tag(field) => field,
        _ => TagField::Title,
    };
    match key.code {
        KeyCode::Esc | KeyCode::Enter => ytdl::close_tags(gui, form),
        _ => {
            ytdl::draft_mut(gui, form).keys = Keys::Tag(field);
            ytdl::tag_typed(gui, form, field, key);
        }
    }
}

// ── Format ──────────────────────────────────────────────────────────────────

/// The Format sheet (the canvas's Format board): what the sheet chooses,
/// the formats the server takes — the dot ACCENT on the one in the
/// making, its name BOLD, what each is good for DIM beside it (wrapped in
/// its column where a language's words are longer than it) — then who
/// remembers it and `[ Done ]`.
pub(super) fn draw_formats(frame: &mut Frame, gui: &mut Gui, area: Rect) {
    let Some(sheet) = gui.ytdl.formats.clone() else { return };
    let options = ytdl::formats_offered(gui);
    let current = match &sheet.job {
        Some(id) => gui.ytdl.jobs().and_then(|j| j.job(id)).map(|j| j.codec.clone()).unwrap_or_default(),
        None => ytdl::form_codec(gui, ytdl::active_form(gui)),
    };
    let names: Vec<String> = options.iter().map(|f| format_name(f)).collect();
    // The names' column: the widest and four cells, eight at least.
    let name_w = (names.iter().map(|n| width(n)).max().unwrap_or(4) as u16 + 4).max(8);
    let help_w = sheet_room(gui, area).saturating_sub(usize::from(4 + name_w)).max(1);
    let helps: Vec<Vec<String>> = options
        .iter()
        .map(|f| format_help(f).map(|help| crate::kit::wrap_whole(&help.text(), help_w)).unwrap_or_default())
        .collect();
    let heights: Vec<u16> = helps.iter().map(|lines| lines.len().max(1) as u16).collect();
    let rect = open_sheet(frame, gui, area, heights.iter().sum::<u16>() + 3, yt(YtAct::FormatClose));
    let name = format_name(&current);
    sheet_title(frame, rect, &t!("gui.ytdl.room.format"), |frame, x, room| {
        let name = super::bar::clip(&name, room);
        put(frame, x, rect.y, &name, dim());
        width(&name) as u16
    });
    let (x, total) = sheet_text(rect);
    put(frame, x, rect.y + 1, &super::bar::clip(&t!("gui.ytdl.format.best"), usize::from(total)), dim());
    let (dot_on, dot_off) = ytdl::dots();
    let mut y = rect.y + 2;
    for (i, codec) in options.iter().enumerate() {
        let row = Rect { x, y, width: total, height: heights[i] };
        let on = i == sheet.cursor;
        put(frame, x, y, if on { dot_on } else { dot_off }, if on { accent() } else { dim() });
        let style = match (on, gui.ui.hovers(row)) {
            (_, true) => bright_bold(),
            (true, false) => Style::default().add_modifier(Modifier::BOLD),
            (false, false) => Style::default(),
        };
        put(frame, x + 4, y, &names[i], style);
        for (line, ly) in helps[i].iter().zip(y..) {
            put(frame, x + 4 + name_w, ly, &super::bar::clip(line, help_w), dim());
        }
        gui.ui.click(row, yt(YtAct::FormatPick(codec.to_string())));
        y += heights[i];
    }
    let foot = y + 1;
    let done = bracket_button(frame, gui, rect.right().saturating_sub(3), foot, &t!("gui.ytdl.done"), true, yt(YtAct::FormatDone));
    // The song's own format is the server's from now on; a row's is its own.
    if sheet.job.is_none() {
        let words = t!("gui.ytdl.format.remembered", server = ytdl::server_name(gui)).to_string();
        put(frame, x, foot, &super::bar::clip(&words, usize::from(done.saturating_sub(x + 2))), dim());
    }
}

/// The sheet's keys: ↑ ↓ move the dot, Enter takes it, Esc (or `f`
/// again) closes with the format as it was.
pub(super) fn formats_key(gui: &mut Gui, key: KeyEvent) {
    let last = ytdl::formats_offered(gui).len().saturating_sub(1);
    let Some(sheet) = gui.ytdl.formats.as_mut() else { return };
    match key.code {
        KeyCode::Up => sheet.cursor = sheet.cursor.saturating_sub(1),
        KeyCode::Down => sheet.cursor = (sheet.cursor + 1).min(last),
        KeyCode::Enter => {
            gui.act(yt(YtAct::FormatDone));
        }
        KeyCode::Esc | KeyCode::Char('f' | 'F') => gui.ytdl.formats = None,
        _ => {}
    }
}
